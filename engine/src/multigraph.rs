//! Multi-layer routing graph with via-site ownership (M3 / T3.2; design note
//! `m3-layers-vias-design.md` §3).
//!
//! One [`RouteGraph`] per routable layer; every via site is a CDT vertex of every layer. An
//! *active* site (a via of some connection) becomes an anchor/vertex of radius `d / 2` on every
//! layer, is exclusive, and blocks the sites too close to it (hole-to-hole / copper clearance).
//! Per-layer graph paths are indexed by *leg id* (P2): `wire_of` / `conn_wires` live here.

use crate::decompose::Connection;
use crate::geom::{dequantise, Aabb, Grid, Prim, Vec2};
use crate::graph::{GraphRules, RouteGraph, TerminalInfo};
use crate::model::BoardModel;
use crate::obstacle::ObstacleSet;
use crate::search::SlotPath;
use crate::sketch::{build_layer_cdt, ConstraintSet, LayerSketch};
use crate::vias::{ViaSite, ViaSize};

pub struct LayerCtx {
    pub layer: u32,
    pub ls: LayerSketch,
    pub g: RouteGraph,
}

pub struct MultiGraph {
    pub layers: Vec<LayerCtx>,
    pub sites: Vec<ViaSite>,
    /// `[site][layer idx]` -> CDT vertex of the site on that layer.
    pub site_vertex: Vec<Vec<usize>>,
    /// `[site][layer idx]` -> index into `ObstacleSet::layer(l).anchors`.
    pub site_anchor: Vec<Vec<usize>>,
    /// Active via (connection, size) per site.
    pub owner: Vec<Option<(usize, ViaSize)>>,
    /// Number of active neighbours that block the site.
    pub blocked: Vec<u32>,
    /// Sorted pour indices whose fill (on a routable layer) contains the site with the via
    /// annulus inside the fill.
    pub in_pour: Vec<Vec<usize>>,
    /// Clock value of the last activation / release / block change of the site.
    pub site_version: Vec<u64>,
    /// Leg id -> `(connection, leg ordinal)`; `None` for a free id (P2).
    pub wire_of: Vec<Option<(usize, usize)>>,
    /// Connection -> its leg ids in leg order (P2).
    pub conn_wires: Vec<Vec<usize>>,
    /// Board rules (via sizes of a net for the layered search).
    pub rules: crate::model::Rules,
    hole_to_hole: f64,
    max_d_all: f64,
    grid: Grid<usize>,
    blocks_of: Vec<Vec<usize>>,
    blocked_by: Vec<Vec<usize>>,
    conn_sites: Vec<Vec<usize>>,
    free_ids: Vec<usize>,
    clock: u64,
}

fn dist_point_seg(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let l2 = ab.len2();
    let t = if l2 > 0.0 {
        ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).len()
}

fn in_ring(p: Vec2, ring: &[Vec2]) -> bool {
    let n = ring.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

impl MultiGraph {
    /// [`MultiGraph::build_on`] with every terminal present on every layer.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        model: &BoardModel,
        obs: &mut ObstacleSet,
        sites: Vec<ViaSite>,
        layers: &[u32],
        terminals: &[TerminalInfo],
        rules: &GraphRules,
    ) -> Result<MultiGraph, String> {
        let t: Vec<(TerminalInfo, Vec<u32>)> = terminals
            .iter()
            .map(|t| (t.clone(), layers.to_vec()))
            .collect();
        Self::build_on(model, obs, sites, layers, &t, rules)
    }

    /// Build the per-layer graphs. `terminals` pair a [`TerminalInfo`] with the layers it has
    /// copper on (a terminal vertex is added to those layers only). First the dormant site
    /// anchors are added to `obs` (P3), then per layer `build_layer_cdt(terminals on the layer,
    /// Routing)` + `RouteGraph::build`; every site must be a CDT vertex of every layer whose
    /// vertex carries `site == Some(s)`, else `Err`.
    ///
    /// Complexity: `O(sum_L N_L log N_L)` over the layers' vertex counts, `O(S * layers)` site
    /// tables, `O(S * sum_P |ring_P|)` pour membership (once).
    pub fn build_on(
        model: &BoardModel,
        obs: &mut ObstacleSet,
        sites: Vec<ViaSite>,
        layers: &[u32],
        terminals: &[(TerminalInfo, Vec<u32>)],
        rules: &GraphRules,
    ) -> Result<MultiGraph, String> {
        let site_anchor = obs.add_site_anchors(&sites, layers);
        let outline: Vec<Prim> = model.outline.iter().map(dequantise).collect();
        let mut ctxs = Vec::with_capacity(layers.len());
        for &l in layers {
            let on: Vec<&TerminalInfo> = terminals
                .iter()
                .filter(|(_, ls)| ls.contains(&l))
                .map(|(t, _)| t)
                .collect();
            let pos: Vec<Vec2> = on.iter().map(|t| t.at).collect();
            let ls = build_layer_cdt(obs.layer(l), &pos, ConstraintSet::Routing)?;
            let infos: Vec<TerminalInfo> = on.into_iter().cloned().collect();
            let g = RouteGraph::build(&ls, obs.layer(l), &outline, &infos, rules);
            ctxs.push(LayerCtx { layer: l, ls, g });
        }
        let mut site_vertex = Vec::with_capacity(sites.len());
        for (s, site) in sites.iter().enumerate() {
            let mut row = Vec::with_capacity(ctxs.len());
            for c in &ctxs {
                let v =
                    c.ls.vertex_at(site.at)
                        .filter(|&v| c.g.verts[v].site == Some(s))
                        .ok_or_else(|| {
                            format!(
                                "via site {s} is not a vertex of layer {} ({:?})",
                                c.layer, site.at
                            )
                        })?;
                row.push(v);
            }
            site_vertex.push(row);
        }
        let max_d_all = sites.iter().map(|s| s.max_d).max().unwrap_or(0) as f64;
        let mut grid = Grid::new((2.0 * max_d_all).max(1_000_000.0));
        for (i, s) in sites.iter().enumerate() {
            grid.insert(
                Aabb {
                    min: s.at,
                    max: s.at,
                },
                i,
            );
        }
        let in_pour = Self::pour_membership(model, &sites, layers);
        let n = sites.len();
        Ok(MultiGraph {
            layers: ctxs,
            sites,
            site_vertex,
            site_anchor,
            owner: vec![None; n],
            blocked: vec![0; n],
            in_pour,
            site_version: vec![0; n],
            wire_of: Vec::new(),
            conn_wires: Vec::new(),
            rules: model.rules.clone(),
            hole_to_hole: model.rules.hole_to_hole as f64,
            max_d_all,
            grid,
            blocks_of: vec![Vec::new(); n],
            blocked_by: vec![Vec::new(); n],
            conn_sites: Vec::new(),
            free_ids: Vec::new(),
            clock: 0,
        })
    }

    /// Pour indices (sorted) whose fill on a routable layer contains the site and keeps the
    /// via annulus (`max_d / 2`) inside it. O(S * sum of ring sizes).
    fn pour_membership(model: &BoardModel, sites: &[ViaSite], layers: &[u32]) -> Vec<Vec<usize>> {
        let rings: Vec<Vec<Vec<Vec2>>> = model
            .pours
            .iter()
            .map(|p| {
                p.fills
                    .iter()
                    .filter(|f| layers.contains(&f.layer))
                    .map(|f| f.outer.iter().map(|q| q.to_vec2()).collect())
                    .collect()
            })
            .collect();
        sites
            .iter()
            .map(|s| {
                let r = 0.5 * s.max_d as f64;
                rings
                    .iter()
                    .enumerate()
                    .filter(|(_, fills)| {
                        fills.iter().any(|ring| {
                            ring.len() >= 3
                                && in_ring(s.at, ring)
                                && (0..ring.len()).all(|i| {
                                    dist_point_seg(s.at, ring[i], ring[(i + 1) % ring.len()]) >= r
                                })
                        })
                    })
                    .map(|(pi, _)| pi)
                    .collect()
            })
            .collect()
    }

    /// Index of `layer` in `layers`. O(layers).
    pub fn layer_index(&self, layer: u32) -> Option<usize> {
        self.layers.iter().position(|l| l.layer == layer)
    }

    /// Site `s` can take a via of `size`: unowned, not blocked, large enough. O(1).
    pub fn site_free(&self, s: usize, _net: i32, size: ViaSize) -> bool {
        self.owner[s].is_none()
            && self.blocked[s] == 0
            && self.sites[s].max_d >= size.d
            && self.sites[s].max_drill >= size.drill
    }

    fn reach(&self, d: f64, clr: f64) -> f64 {
        0.5 * d + 0.5 * self.max_d_all + clr.max(self.hole_to_hole)
    }

    /// Make site `s` the via of connection `conn` (net `net`, `size`, clearance `clr`):
    /// every layer's vertex becomes a copper vertex of radius `d / 2` (capacities of the
    /// incident edges shrink, `edge_version` bumps), the obstacle anchors are enabled, and
    /// every site closer than `d/2 + max_d_t/2 + max(clr, hole_to_hole)` gets `blocked += 1`.
    /// Exclusive: a second activation is a bug (`debug_assert`).
    /// O(layers * deg + neighbours).
    pub fn activate(
        &mut self,
        s: usize,
        conn: usize,
        net: i32,
        size: ViaSize,
        clr: f64,
        obs: &mut ObstacleSet,
    ) {
        debug_assert!(self.owner[s].is_none(), "site {s} activated twice");
        self.owner[s] = Some((conn, size));
        let label = format!("via:{conn}:{s}");
        for (li, c) in self.layers.iter_mut().enumerate() {
            c.g.set_site(
                self.site_vertex[s][li],
                Some((net, 0.5 * size.d as f64, clr, label.clone())),
            );
        }
        obs.set_site(&self.site_anchor[s], Some((net, size)));
        let at = self.sites[s].at;
        let reach = self.reach(size.d as f64, clr);
        let near = self.sites_near(Aabb { min: at, max: at }.grow(reach));
        let mut blocks = Vec::new();
        for t in near {
            if t == s {
                continue;
            }
            let lim =
                0.5 * size.d as f64 + 0.5 * self.sites[t].max_d as f64 + clr.max(self.hole_to_hole);
            if (self.sites[t].at - at).len() < lim {
                self.blocked[t] += 1;
                let pos = self.blocked_by[t].binary_search(&s).unwrap_or_else(|p| p);
                self.blocked_by[t].insert(pos, s);
                self.clock += 1;
                self.site_version[t] = self.clock;
                blocks.push(t);
            }
        }
        self.blocks_of[s] = blocks;
        if self.conn_sites.len() <= conn {
            self.conn_sites.resize(conn + 1, Vec::new());
        }
        self.conn_sites[conn].push(s);
        self.clock += 1;
        self.site_version[s] = self.clock;
    }

    /// Undo [`MultiGraph::activate`] from the stored block list: vertices back to `r 0 / clr 0
    /// / nets [] / owner None`, anchors disabled, neighbours unblocked. O(layers * deg +
    /// neighbours).
    pub fn release(&mut self, s: usize, obs: &mut ObstacleSet) {
        let Some((conn, _)) = self.owner[s].take() else {
            return;
        };
        for (li, c) in self.layers.iter_mut().enumerate() {
            c.g.set_site(self.site_vertex[s][li], None);
        }
        obs.set_site(&self.site_anchor[s], None);
        for t in std::mem::take(&mut self.blocks_of[s]) {
            self.blocked[t] -= 1;
            if let Ok(p) = self.blocked_by[t].binary_search(&s) {
                self.blocked_by[t].remove(p);
            }
            self.clock += 1;
            self.site_version[t] = self.clock;
        }
        if let Some(cs) = self.conn_sites.get_mut(conn) {
            cs.retain(|&x| x != s);
        }
        self.clock += 1;
        self.site_version[s] = self.clock;
    }

    /// Sites inside `bb`, sorted. O(1) grid cells on average.
    pub fn sites_near(&self, bb: Aabb) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .grid
            .query(bb)
            .into_iter()
            .filter(|&i| {
                let p = self.sites[i].at;
                p.x >= bb.min.x && p.x <= bb.max.x && p.y >= bb.min.y && p.y <= bb.max.y
            })
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// CDT vertex of layer `layer_idx` at exactly `p` (an input point). O(1).
    pub fn vertex_of(&self, layer_idx: usize, p: Vec2) -> Option<usize> {
        self.layers.get(layer_idx)?.ls.vertex_at(p)
    }

    /// Sites currently blocking `t`, sorted (the owners to rip to free it). O(1).
    pub fn blocked_by(&self, t: usize) -> &[usize] {
        &self.blocked_by[t]
    }

    /// Sites activated for `conn`, in activation order. O(1).
    pub fn sites_of(&self, conn: usize) -> &[usize] {
        self.conn_sites.get(conn).map_or(&[], |v| v.as_slice())
    }

    /// Allocate `n` leg ids for `conn` (P2): the smallest free ids first, then fresh ones;
    /// recorded in `wire_of` / `conn_wires`. The caller inserts each leg with
    /// `layers[leg.layer_idx].g.insert_path(k, ..)` and activates its sites (or use
    /// [`MultiGraph::insert_path`]).
    /// O(n log F).
    pub fn alloc_legs(&mut self, conn: usize, n: usize) -> Vec<usize> {
        if self.conn_wires.len() <= conn {
            self.conn_wires.resize(conn + 1, Vec::new());
        }
        debug_assert!(
            self.conn_wires[conn].is_empty(),
            "conn {conn} still has legs"
        );
        self.free_ids.sort_unstable();
        let mut ids = Vec::with_capacity(n);
        for ord in 0..n {
            let k = if self.free_ids.is_empty() {
                self.wire_of.len()
            } else {
                self.free_ids.remove(0)
            };
            if k == self.wire_of.len() {
                self.wire_of.push(None);
            }
            self.wire_of[k] = Some((conn, ord));
            ids.push(k);
        }
        self.conn_wires[conn] = ids.clone();
        ids
    }

    /// Insert a routed layered path of `conn` (P2): one leg id per leg
    /// ([`MultiGraph::alloc_legs`]), `layers[leg.layer_idx].g.insert_path(leg_id, ..)` per leg
    /// with the connection's width / clearance, then [`MultiGraph::activate`] per via site (after
    /// the legs, so the crossing slots the search saw stay valid). Returns the leg ids in leg
    /// order. O(legs * path cost + vias * (layers * deg + neighbours)).
    pub fn insert_path(
        &mut self,
        conn: &Connection,
        path: &SlotPath,
        obs: &mut ObstacleSet,
    ) -> Vec<usize> {
        let ids = self.alloc_legs(conn.id, path.legs.len());
        let (w, clr) = (conn.width as f64, conn.clearance as f64);
        for (leg, &k) in path.legs.iter().zip(&ids) {
            self.layers[leg.layer_idx].g.insert_path(
                k,
                leg.start,
                leg.goal,
                &leg.crossings,
                &leg.faces,
                w,
                clr,
            );
        }
        for &(s, size) in &path.sites {
            self.activate(s, conn.id, conn.net, size, clr, obs);
        }
        ids
    }

    /// Remove every leg of `conn` from the layer graphs, release its sites, free its leg ids.
    /// O(legs * layers + path cost + sites).
    pub fn remove_conn(&mut self, conn: usize, obs: &mut ObstacleSet) {
        for s in self.sites_of(conn).to_vec() {
            self.release(s, obs);
        }
        let ids = self
            .conn_wires
            .get_mut(conn)
            .map(std::mem::take)
            .unwrap_or_default();
        for k in ids {
            for c in &mut self.layers {
                c.g.remove_conn(k);
            }
            self.wire_of[k] = None;
            self.free_ids.push(k);
        }
    }

    /// Connections owning the legs `ks` (sorted, deduplicated). O(|ks| log |ks|).
    pub fn conns_of_legs(&self, ks: &[usize]) -> Vec<usize> {
        let mut v: Vec<usize> = ks
            .iter()
            .filter_map(|&k| self.wire_of.get(k).copied().flatten().map(|(c, _)| c))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}
