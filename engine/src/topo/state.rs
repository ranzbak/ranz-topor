//! Topology state (plan "Rough multilayer router: one-shot spec"): one projection CDT over the
//! obstacles of all layers, the passes of every wire through its edges (ordered per edge) and
//! the crossings, which are derived from interleaving inside a triangle, never stored.
//!
//! Coordinates are nm (math frame, Y up). Storage is `Vec` with index ids and nothing iterates a
//! `HashMap`, so every result is deterministic.
//!
//! Complexity: building the projection is O(V log V) for the CDT plus O(T · obstacles) for the
//! region labels (bbox prefilter); `crossings(w)` is O(Σ traversals in w's triangles); `insert` and
//! `remove` are O(passes · edge load).

use super::demand::{need_in, Bound, Item};
use crate::cdt::Cdt;
use crate::geom::Vec2;

pub type VertId = usize;
pub type EdgeId = usize;
pub type TriId = usize;
pub type WireId = usize;
pub type PassId = usize;
pub type ObsId = usize;

/// One bit per copper (signal) layer index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerMask(pub u64);

impl LayerMask {
    pub fn all(n: usize) -> Self {
        LayerMask(if n >= 64 { u64::MAX } else { (1u64 << n) - 1 })
    }
    pub fn has(self, l: usize) -> bool {
        (self.0 >> l) & 1 == 1
    }
    pub fn count(self) -> u32 {
        self.0.count_ones()
    }
    /// The layer when exactly one is set.
    pub fn single(self) -> Option<usize> {
        (self.count() == 1).then(|| self.0.trailing_zeros() as usize)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObsKind {
    Pad,
    Drill,
}

#[derive(Clone, Debug)]
pub struct Obstacle {
    pub kind: ObsKind,
    /// Closed outline, counter-clockwise.
    pub poly: Vec<Vec2>,
    pub center: Vec2,
    /// Copper layers it occupies.
    pub layers: LayerMask,
    /// A drilled obstacle is a wall on every layer for other nets.
    pub drilled: bool,
    /// `None`: no net, an obstacle for every wire.
    pub net: Option<i32>,
    /// Clearance the copper itself demands from any other net's track (nm): `max(pad net
    /// clearance, pad local clearance, board minimum, NPTH hole clearance)`.
    pub clear: f64,
    /// A bare hole without copper (NPTH inside its drill): only `clear` applies, not the net's.
    pub bare: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VertKind {
    /// Steiner or split vertex: not an obstacle.
    Free,
    /// Vertex of an obstacle outline.
    Outline(ObsId),
    /// Centre of a pad: a wire terminal.
    Centre(ObsId),
    /// Board edge.
    Edge,
}

/// The projection: one CDT over all obstacles of all layers, with per-triangle regions and
/// per-edge room.
#[derive(Clone, Debug)]
pub struct Projection {
    pub n_layers: usize,
    pub obstacles: Vec<Obstacle>,
    pub pos: Vec<Vec2>,
    pub vkind: Vec<VertKind>,
    /// Vertex ids ordered by x (for [`Projection::vertex_at`]).
    pub by_x: Vec<VertId>,
    /// Canonical `[p, q]` with `p < q`.
    pub edges: Vec<[VertId; 2]>,
    /// Triangles on either side of each edge (one for a hull edge).
    pub edge_tris: Vec<[Option<TriId>; 2]>,
    /// Counter-clockwise vertices.
    pub tris: Vec<[VertId; 3]>,
    /// `tri_edges[t][i]` is the side `(v_i, v_{i+1})`.
    pub tri_edges: Vec<[EdgeId; 3]>,
    pub tri_out: Vec<bool>,
    /// Obstacles containing the triangle.
    pub tri_obs: Vec<Vec<ObsId>>,
    pub vert_tris: Vec<Vec<TriId>>,
    /// Free length of each edge per layer (nm); `INFINITY` where the edge is not a cut between
    /// two obstacles present on that layer.
    pub room: Vec<Vec<f64>>,
    /// Copper-to-board-edge clearance (nm); set by the builder.
    pub edge_clear: f64,
    /// Custom courtyard clearance rules `(polygon, clearance nm)`, in file order (the last match
    /// wins); set by the builder.
    pub regions: Vec<(Vec<Vec2>, f64)>,
}

pub fn point_in_poly(p: Vec2, poly: &[Vec2]) -> bool {
    let n = poly.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

/// Even-odd test against an unordered set of segments (the board outline).
pub fn point_in_segments(p: Vec2, segs: &[(Vec2, Vec2)]) -> bool {
    let mut inside = false;
    for &(a, b) in segs {
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

impl Projection {
    /// `points` with their kinds (coincident points share a vertex; the first non-free kind
    /// wins), `constraints` as index pairs into `points`, `inside` = inside the board.
    pub fn build(
        n_layers: usize,
        obstacles: Vec<Obstacle>,
        points: &[(Vec2, VertKind)],
        constraints: &[(usize, usize)],
        inside: impl Fn(Vec2) -> bool,
    ) -> Result<Projection, String> {
        let pts: Vec<Vec2> = points.iter().map(|p| p.0).collect();
        let cdt = Cdt::build(&pts, constraints)?;
        let nv = cdt.num_vertices();
        let mut vkind = vec![VertKind::Free; nv];
        for (i, &(_, k)) in points.iter().enumerate() {
            let c = cdt.vertex_index(i);
            if vkind[c] == VertKind::Free {
                vkind[c] = k;
            }
        }
        let pos: Vec<Vec2> = (0..nv).map(|v| cdt.vertex_pos(v)).collect();
        let mut by_x: Vec<VertId> = (0..nv).collect();
        by_x.sort_by(|&a, &b| pos[a].x.total_cmp(&pos[b].x).then(a.cmp(&b)));
        let topo = cdt.topology();
        let edges: Vec<[VertId; 2]> = topo.edges.iter().map(|&(a, b, _)| [a, b]).collect();
        let edge_id = |a: usize, b: usize| -> EdgeId {
            let k = [a.min(b), a.max(b)];
            edges.binary_search(&k).expect("triangle side is an edge")
        };
        let tris: Vec<[VertId; 3]> = topo.faces.clone();
        let mut edge_tris = vec![[None, None]; edges.len()];
        let mut tri_edges = Vec::with_capacity(tris.len());
        let mut vert_tris = vec![Vec::new(); nv];
        for (t, f) in tris.iter().enumerate() {
            let mut te = [0; 3];
            for i in 0..3 {
                let (a, b) = (f[i], f[(i + 1) % 3]);
                let e = edge_id(a, b);
                te[i] = e;
                // Slot 0: the triangle left of p->q (CCW side a->b with a == p).
                let slot = if a < b { 0 } else { 1 };
                edge_tris[e][slot] = Some(t);
                vert_tris[f[i]].push(t);
            }
            tri_edges.push(te);
        }
        // Regions: board inside test and containing obstacles at the centroid.
        let bbox: Vec<(Vec2, Vec2)> = obstacles
            .iter()
            .map(|o| {
                let mut lo = Vec2 { x: f64::INFINITY, y: f64::INFINITY };
                let mut hi = Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY };
                for p in &o.poly {
                    lo = Vec2 { x: lo.x.min(p.x), y: lo.y.min(p.y) };
                    hi = Vec2 { x: hi.x.max(p.x), y: hi.y.max(p.y) };
                }
                (lo, hi)
            })
            .collect();
        let mut tri_out = Vec::with_capacity(tris.len());
        let mut tri_obs = Vec::with_capacity(tris.len());
        for f in &tris {
            let c = (pos[f[0]] + pos[f[1]] + pos[f[2]]) * (1.0 / 3.0);
            tri_out.push(!inside(c));
            let ino: Vec<ObsId> = (0..obstacles.len())
                .filter(|&o| {
                    let (lo, hi) = bbox[o];
                    c.x >= lo.x && c.x <= hi.x && c.y >= lo.y && c.y <= hi.y
                        && point_in_poly(c, &obstacles[o].poly)
                })
                .collect();
            tri_obs.push(ino);
        }
        let blocks = |v: VertId, l: usize| match vkind[v] {
            VertKind::Outline(o) => obstacles[o].drilled || obstacles[o].layers.has(l),
            VertKind::Edge => true,
            _ => false,
        };
        let room = edges
            .iter()
            .map(|&[p, q]| {
                let same = matches!((vkind[p], vkind[q]),
                    (VertKind::Outline(a), VertKind::Outline(b)) if a == b);
                let len = (pos[q] - pos[p]).len();
                (0..n_layers)
                    .map(|l| {
                        if !same && blocks(p, l) && blocks(q, l) {
                            len
                        } else {
                            f64::INFINITY
                        }
                    })
                    .collect()
            })
            .collect();
        Ok(Projection {
            n_layers,
            obstacles,
            pos,
            vkind,
            by_x,
            edges,
            edge_tris,
            tris,
            tri_edges,
            tri_out,
            tri_obs,
            vert_tris,
            room,
            edge_clear: 0.0,
            regions: Vec::new(),
        })
    }

    /// Layers a wire of `net` may use inside triangle `t`; `None` = wall (outside the board,
    /// inside another net's drilled obstacle, or no layer left).
    pub fn allowed(&self, t: TriId, net: i32) -> Option<LayerMask> {
        if self.tri_out[t] {
            return None;
        }
        let mut m = LayerMask::all(self.n_layers);
        for &o in &self.tri_obs[t] {
            let ob = &self.obstacles[o];
            if ob.net == Some(net) {
                continue;
            }
            if ob.drilled {
                return None;
            }
            m.0 &= !ob.layers.0;
        }
        (m.0 != 0).then_some(m)
    }

    /// The vertex within `tol` (nm) of `p`, if any. O(log V + hits).
    pub fn vertex_at(&self, p: Vec2, tol: f64) -> Option<VertId> {
        let i = self.by_x.partition_point(|&v| self.pos[v].x < p.x - tol);
        self.by_x[i..]
            .iter()
            .take_while(|&&v| self.pos[v].x <= p.x + tol)
            .copied()
            .find(|&v| (self.pos[v] - p).len() <= tol)
    }

    /// The other triangle across edge `e` from `t`.
    pub fn across(&self, e: EdgeId, t: TriId) -> Option<TriId> {
        match self.edge_tris[e] {
            [Some(a), b] if a == t => b,
            [a, Some(b)] if b == t => a,
            _ => None,
        }
    }

    /// Index of side `e` in triangle `t`.
    pub fn side(&self, t: TriId, e: EdgeId) -> usize {
        self.tri_edges[t].iter().position(|&x| x == e).expect("edge of triangle")
    }

    /// Boundary coordinate in `[0, 3)` of fraction `f` (from the canonical `p` to `q`) along
    /// side `e` of `t`; corner `v_i` is `i`.
    pub fn bpos(&self, t: TriId, e: EdgeId, f: f64) -> f64 {
        let i = self.side(t, e);
        let fwd = self.tris[t][i] == self.edges[e][0];
        i as f64 + if fwd { f } else { 1.0 - f }
    }

    pub fn corner_pos(&self, t: TriId, v: VertId) -> f64 {
        self.tris[t].iter().position(|&x| x == v).expect("corner of triangle") as f64
    }

    /// Room of edge `e` on layer `l` for a wire of `net`: an end on the wire's own net's
    /// obstacle is no limit, so the edge is no cut for it.
    pub fn room_for(&self, e: EdgeId, l: usize, net: i32) -> f64 {
        let own = |v: VertId| matches!(self.vkind[v], VertKind::Outline(o) if self.obstacles[o].net == Some(net));
        let [p, q] = self.edges[e];
        if own(p) || own(q) {
            f64::INFINITY
        } else {
            self.room[e][l]
        }
    }

    /// What bounds a cut at vertex `v`, for the demand model.
    pub fn bound(&self, v: VertId) -> Bound {
        match self.vkind[v] {
            VertKind::Outline(o) if self.obstacles[o].bare => Bound::Hole { clear: self.obstacles[o].clear },
            VertKind::Outline(o) => Bound::Pad { net: self.obstacles[o].net, clear: self.obstacles[o].clear },
            VertKind::Edge => Bound::Edge { clear: self.edge_clear },
            _ => Bound::Open,
        }
    }

    /// The custom clearance that replaces the net classes' at point `p`, if a courtyard rule
    /// covers it.
    pub fn regime_at(&self, p: Vec2) -> Option<f64> {
        self.regions.iter().rev().find(|(poly, _)| point_in_poly(p, poly)).map(|r| r.1)
    }

    /// The regime of edge `e` (at its midpoint).
    pub fn regime(&self, e: EdgeId) -> Option<f64> {
        if self.regions.is_empty() {
            return None;
        }
        let [p, q] = self.edges[e];
        self.regime_at((self.pos[p] + self.pos[q]) * 0.5)
    }

    /// Space `items` (ordered from end `p` to end `q`) need across edge `e` on one layer.
    pub fn edge_demand(&self, e: EdgeId, items: &[Item]) -> f64 {
        let [p, q] = self.edges[e];
        need_in(self.regime(e), items, self.bound(p), self.bound(q))
    }

    /// Whether `it` alone fits through edge `e` on layer `l`.
    pub fn fits_alone(&self, e: EdgeId, l: usize, it: Item) -> bool {
        self.edge_demand(e, &[it]) <= self.room_for(e, l, it.net)
    }

    /// Distance from `p` to the nearest obstacle not of `net` (0 inside one). O(obstacles).
    pub fn free_radius(&self, p: Vec2, net: i32) -> f64 {
        let mut d = f64::INFINITY;
        for ob in self.obstacles.iter().filter(|o| o.net != Some(net)) {
            if point_in_poly(p, &ob.poly) {
                return 0.0;
            }
            let n = ob.poly.len();
            for i in 0..n {
                let (a, b) = (ob.poly[i], ob.poly[(i + 1) % n]);
                let e = b - a;
                let t = ((p - a).dot(e) / e.len2().max(1e-12)).clamp(0.0, 1.0);
                d = d.min((p - (a + e * t)).len());
            }
        }
        d
    }

    /// Radius of the inscribed circle of `t`.
    pub fn inradius(&self, t: TriId) -> f64 {
        let [a, b, c] = self.tris[t].map(|v| self.pos[v]);
        let area = 0.5 * (b - a).cross(c - a).abs();
        let per = (b - a).len() + (c - b).len() + (a - c).len();
        if per > 0.0 {
            2.0 * area / per
        } else {
            0.0
        }
    }
}

/// Chords `(a1, a2)` and `(b1, b2)` on a triangle boundary cross iff exactly one end of `b` lies
/// strictly between the ends of `a`. Shared ends (a common corner) never cross.
pub fn interleave(a1: f64, a2: f64, b1: f64, b2: f64) -> bool {
    if a1 == b1 || a1 == b2 || a2 == b1 || a2 == b2 {
        return false;
    }
    let (lo, hi) = (a1.min(a2), a1.max(a2));
    let inside = |x: f64| x > lo && x < hi;
    inside(b1) != inside(b2)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pass {
    pub wire: WireId,
    pub edge: EdgeId,
}

#[derive(Clone, Debug)]
pub struct Wire {
    pub net: i32,
    /// Width and clearance (nm).
    pub w: f64,
    pub s: f64,
    /// Terminal vertices (pad centres) and their pad obstacles.
    pub a: VertId,
    pub b: VertId,
    pub pa: ObsId,
    pub pb: ObsId,
    /// Free terminals (dead end, junction): position inside the first / last triangle of the wire
    /// (`pa` / `pb` is `usize::MAX` then; `a` / `b` hold a stand-in corner for the chord
    /// bookkeeping). The reserved shape of the plan's `Terminal::Branch`: the chord of such a
    /// traversal ends at this point, not at a corner.
    pub free: [Option<Vec2>; 2],
    pub passes: Vec<PassId>,
    /// Triangle of traversal `k` (`0..=passes.len()`): between pass `k-1` and pass `k`.
    pub tris: Vec<TriId>,
    pub alive: bool,
}

/// A crossing seen from one wire: its traversal `k`, the other wire and its traversal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crossing {
    pub k: usize,
    pub other: WireId,
    pub other_k: usize,
    pub tri: TriId,
}

/// A via interval (plan `ViaSlot`): the stretch of `wire` between two consecutive events
/// (crossings or terminals) whose layers differ, as `(traversal, fraction)` positions; `at` is
/// where G0 draws it. The exact position is fixed only in P6.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViaSlot {
    pub wire: WireId,
    pub from: (usize, f64),
    pub to: (usize, f64),
    pub at: (usize, f64),
}

/// A wire to load with its passes `(edge, position along the edge)` and triangles.
pub type LoadItem = (Wire, Vec<(EdgeId, f64)>, Vec<TriId>);

#[derive(Clone, Debug)]
pub struct State<'p> {
    pub proj: &'p Projection,
    pub passes: Vec<Pass>,
    /// Per edge, ordered from `p` to `q`.
    pub edge_order: Vec<Vec<PassId>>,
    pub wires: Vec<Wire>,
    /// Per triangle: the traversals `(wire, k)` through it.
    pub tri_trav: Vec<Vec<(WireId, usize)>>,
}

impl<'p> State<'p> {
    pub fn new(proj: &'p Projection) -> Self {
        State {
            proj,
            passes: Vec::new(),
            edge_order: vec![Vec::new(); proj.edges.len()],
            wires: Vec::new(),
            tri_trav: vec![Vec::new(); proj.tris.len()],
        }
    }

    /// The pass `p` as a demand item.
    pub fn item(&self, p: PassId) -> Item {
        let w = &self.wires[self.passes[p].wire];
        Item { net: w.net, w: w.w, s: w.s }
    }

    /// Space the passes of edge `e` need on one layer, optionally with `extra` inserted at gap
    /// `g` (before the pass at index `g` of the edge order).
    pub fn edge_need(&self, e: EdgeId, extra: Option<(usize, Item)>) -> f64 {
        let mut items: Vec<Item> = self.edge_order[e].iter().map(|&p| self.item(p)).collect();
        if let Some((g, it)) = extra {
            items.insert(g.min(items.len()), it);
        }
        self.proj.edge_demand(e, &items)
    }

    fn order_index(&self, p: PassId) -> usize {
        let e = self.passes[p].edge;
        self.edge_order[e].iter().position(|&x| x == p).expect("pass in its edge order")
    }

    /// Fraction along the edge (from `p`) of pass `p`, the passes spread evenly.
    pub fn pass_frac(&self, p: PassId) -> f64 {
        let n = self.edge_order[self.passes[p].edge].len();
        (self.order_index(p) + 1) as f64 / (n + 1) as f64
    }

    pub fn pass_point(&self, p: PassId) -> Vec2 {
        let [a, b] = self.proj.edges[self.passes[p].edge];
        let (pa, pb) = (self.proj.pos[a], self.proj.pos[b]);
        pa + (pb - pa) * self.pass_frac(p)
    }

    /// Boundary coordinates of pass `p` and of gap `g` (before inserting) on edge `e` of `t`.
    pub fn pass_bpos(&self, t: TriId, p: PassId) -> f64 {
        let e = self.passes[p].edge;
        let n = self.edge_order[e].len() as f64;
        let i = self.order_index(p) as f64;
        self.proj.bpos(t, e, (2.0 * i + 2.0) / (2.0 * n + 2.0))
    }

    pub fn gap_bpos(&self, t: TriId, e: EdgeId, g: usize) -> f64 {
        let n = self.edge_order[e].len() as f64;
        self.proj.bpos(t, e, (2.0 * g as f64 + 1.0) / (2.0 * n + 2.0))
    }

    /// Geometric point of gap `g` on edge `e` (the gaps spread evenly).
    pub fn gap_point(&self, e: EdgeId, g: usize) -> Vec2 {
        let [a, b] = self.proj.edges[e];
        let (pa, pb) = (self.proj.pos[a], self.proj.pos[b]);
        let n = self.edge_order[e].len();
        pa + (pb - pa) * ((g + 1) as f64 / (n + 2) as f64)
    }

    /// Boundary chord of traversal `k` of wire `w`.
    pub fn chord(&self, w: WireId, k: usize) -> (f64, f64) {
        let wi = &self.wires[w];
        let t = wi.tris[k];
        let n = wi.passes.len();
        let a = if k == 0 {
            self.proj.corner_pos(t, wi.a)
        } else {
            self.pass_bpos(t, wi.passes[k - 1])
        };
        let b = if k == n {
            self.proj.corner_pos(t, wi.b)
        } else {
            self.pass_bpos(t, wi.passes[k])
        };
        (a, b)
    }

    /// Terminal, pass points, terminal.
    pub fn polyline(&self, w: WireId) -> Vec<Vec2> {
        let wi = &self.wires[w];
        let mut v = Vec::with_capacity(wi.passes.len() + 2);
        v.push(self.proj.pos[wi.a]);
        v.extend(wi.passes.iter().map(|&p| self.pass_point(p)));
        v.push(self.proj.pos[wi.b]);
        v
    }

    /// Crossings of `w` with other wires, in `w`'s traversal order.
    pub fn crossings(&self, w: WireId) -> Vec<Crossing> {
        let wi = &self.wires[w];
        let mut out = Vec::new();
        for (k, &t) in wi.tris.iter().enumerate() {
            let (a1, a2) = self.chord(w, k);
            for &(o, ok) in &self.tri_trav[t] {
                if o == w {
                    continue;
                }
                let (b1, b2) = self.chord(o, ok);
                if interleave(a1, a2, b1, b2) {
                    out.push(Crossing { k, other: o, other_k: ok, tri: t });
                }
            }
        }
        out
    }

    /// Different-net crossings in triangle `t`.
    pub fn tri_crossings(&self, t: TriId) -> usize {
        let tr = &self.tri_trav[t];
        let mut n = 0;
        for i in 0..tr.len() {
            let (a1, a2) = self.chord(tr[i].0, tr[i].1);
            for j in i + 1..tr.len() {
                if self.wires[tr[i].0].net == self.wires[tr[j].0].net {
                    continue;
                }
                let (b1, b2) = self.chord(tr[j].0, tr[j].1);
                if interleave(a1, a2, b1, b2) {
                    n += 1;
                }
            }
        }
        n
    }

    /// Total different-net crossings.
    pub fn total_crossings(&self) -> usize {
        (0..self.tri_trav.len()).map(|t| self.tri_crossings(t)).sum()
    }

    /// Insert `wire` (its `passes`, `tris` and `alive` are filled here) along `path`: the gaps
    /// `(edge, g)` in the edge orders before this insert, and the traversed triangles.
    pub fn insert(&mut self, mut wire: Wire, path: &[(EdgeId, usize)], tris: Vec<TriId>) -> WireId {
        assert_eq!(tris.len(), path.len() + 1, "one more triangle than passes");
        let id = self.wires.len();
        wire.passes.clear();
        for (j, &(e, g)) in path.iter().enumerate() {
            // Earlier passes of this wire on the same edge shift the gap.
            let shift = path[..j].iter().filter(|&&(e0, g0)| e0 == e && g0 <= g).count();
            let pid = self.passes.len();
            self.passes.push(Pass { wire: id, edge: e });
            let at = (g + shift).min(self.edge_order[e].len());
            self.edge_order[e].insert(at, pid);
            wire.passes.push(pid);
        }
        for (k, &t) in tris.iter().enumerate() {
            self.tri_trav[t].push((id, k));
        }
        wire.tris = tris;
        wire.alive = true;
        self.wires.push(wire);
        id
    }

    /// Load complete wires at once (the hand topology of R0): per wire its passes as
    /// `(edge, position along the edge from p, 0..1)` and its triangles. The per-edge order is
    /// taken from the positions (ties: load order), so no gap search is needed.
    pub fn load_all(&mut self, items: Vec<LoadItem>) {
        let mut at: Vec<Vec<(f64, PassId)>> = vec![Vec::new(); self.edge_order.len()];
        for (mut wire, passes, tris) in items {
            assert_eq!(tris.len(), passes.len() + 1, "one more triangle than passes");
            let id = self.wires.len();
            wire.passes.clear();
            for &(e, u) in &passes {
                let pid = self.passes.len();
                self.passes.push(Pass { wire: id, edge: e });
                at[e].push((u, pid));
                    wire.passes.push(pid);
            }
            for (k, &t) in tris.iter().enumerate() {
                self.tri_trav[t].push((id, k));
            }
            wire.tris = tris;
            wire.alive = true;
            self.wires.push(wire);
        }
        for (e, v) in at.iter_mut().enumerate() {
            v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            self.edge_order[e].extend(v.iter().map(|x| x.1));
        }
    }

    pub fn remove(&mut self, w: WireId) {
        for &p in &self.wires[w].passes {
            let e = self.passes[p].edge;
            self.edge_order[e].retain(|&x| x != p);
        }
        for &t in &self.wires[w].tris {
            self.tri_trav[t].retain(|&(x, _)| x != w);
        }
        self.wires[w].alive = false;
    }

    /// The passes of `a` and `b` between two of their crossings, paired edge by edge, when
    /// both run through the same edges (the bigon holds no obstacle); `None` otherwise.
    pub fn double_stretch(
        &self,
        a: WireId,
        (ka1, ka2): (usize, usize),
        b: WireId,
        (kb1, kb2): (usize, usize),
    ) -> Option<Vec<(PassId, PassId)>> {
        if ka2 <= ka1 {
            return None;
        }
        let pa = &self.wires[a].passes[ka1..ka2];
        let (lo, hi) = (kb1.min(kb2), kb1.max(kb2));
        let mut pb: Vec<PassId> = self.wires[b].passes[lo..hi].to_vec();
        if kb1 > kb2 {
            pb.reverse();
        }
        if pa.len() != pb.len()
            || pa.iter().zip(&pb).any(|(&x, &y)| self.passes[x].edge != self.passes[y].edge)
        {
            return None;
        }
        Some(pa.iter().copied().zip(pb).collect())
    }

    /// Swap the positions of paired passes (self-inverse): removes a double crossing when the
    /// pairs come from [`State::double_stretch`].
    pub fn uncross_double(&mut self, pairs: &[(PassId, PassId)]) {
        for &(x, y) in pairs {
            let e = self.passes[x].edge;
            let ix = self.order_index(x);
            let iy = self.order_index(y);
            self.edge_order[e].swap(ix, iy);
        }
    }

    /// Invariants: each live wire's passes form a connected triangle strip from `a` to `b`,
    /// and the edge orders agree with the wires' pass lists.
    pub fn check(&self) -> Result<(), String> {
        let pr = self.proj;
        for (w, wi) in self.wires.iter().enumerate() {
            if !wi.alive {
                continue;
            }
            let n = wi.passes.len();
            if wi.tris.len() != n + 1 {
                return Err(format!("wire {w}: {} tris for {n} passes", wi.tris.len()));
            }
            if !pr.tris[wi.tris[0]].contains(&wi.a) || !pr.tris[wi.tris[n]].contains(&wi.b) {
                return Err(format!("wire {w}: terminal triangles do not hold the terminals"));
            }
            for (k, &p) in wi.passes.iter().enumerate() {
                let e = self.passes[p].edge;
                if self.passes[p].wire != w {
                    return Err(format!("wire {w}: pass {p} owned by another wire"));
                }
                if pr.across(e, wi.tris[k]) != Some(wi.tris[k + 1]) {
                    return Err(format!("wire {w}: pass {k} breaks the triangle strip"));
                }
                if !self.edge_order[e].contains(&p) {
                    return Err(format!("wire {w}: pass {p} missing from edge {e}"));
                }
            }
        }
        for (e, ord) in self.edge_order.iter().enumerate() {
            for &p in ord {
                let wi = &self.wires[self.passes[p].wire];
                if !wi.alive || self.passes[p].edge != e || !wi.passes.contains(&p) {
                    return Err(format!("edge {e}: stale pass {p}"));
                }
            }
        }
        Ok(())
    }
}
