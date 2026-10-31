//! Per-layer routing graph on the CDT (M2 / T2.1; docs 10 §3-4, design note
//! `m2-search-design.md`): vertices with radius/clearance, edges with cut capacity and an
//! ordered list of crossing slots, faces with an inside flag.
//!
//! Capacity model (conservative, 10 §4): `cap(e) = |pq| - r_p - r_q`, `c_e = max(c_p, c_q)`.
//! Every connection crossing `e` occupies a *slot* in `GEdge::cross`, ordered `a -> b`.
//! Build is O(V) after the CDT; `insert` / `remove_conn` cost O(n_max) per touched edge
//! (`n_max` = entries on one edge). No `HashMap` iteration reaches any output.

use crate::cdt::Cdt;
use crate::geom::{Aabb, Grid, Prim, Vec2};
use crate::obstacle::{LayerObstacles, ObsKind};
use crate::sketch::LayerSketch;
use std::collections::{BTreeMap, HashMap};

/// Cell size (nm) of the vertex grid.
const VERT_CELL: f64 = 5_000_000.0;

#[derive(Clone, Debug)]
pub struct VertexInfo {
    pub at: Vec2,
    /// Index into `LayerObstacles.anchors`; `None` for terminals, split vertices, frame corners.
    pub anchor: Option<usize>,
    pub owner: Option<String>,
    /// Anchor net plus tie nets (net 0 excluded), sorted.
    pub nets: Vec<i32>,
    /// Core radius (0 for non-anchors).
    pub r: f64,
    /// Vertex clearance.
    pub clr: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CrossEntry {
    pub conn: usize,
    pub width: f64,
    pub clr: f64,
    /// Index of this crossing in its connection's path (`insert_path`); `None` for plain
    /// `insert` (capacity-only, `Port::Detached`).
    pub k: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct GEdge {
    /// `a < b`.
    pub a: usize,
    pub b: usize,
    /// `faces[0]` is the face on the left of `a -> b`, `faces[1]` on the right; `None` outside the
    /// triangulation hull.
    pub faces: [Option<usize>; 2],
    pub constraint: bool,
    pub owner_nets: Vec<i32>,
    pub cap: f64,
    pub c_e: f64,
    /// Crossing entries ordered `a -> b`.
    pub cross: Vec<CrossEntry>,
    /// Connection whose wire runs along this edge (empty crossing path between its terminals).
    pub along: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Face {
    /// Counter-clockwise.
    pub v: [usize; 3],
    /// `e[i]` is the edge opposite `v[i]`.
    pub e: [usize; 3],
    pub inside: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct GraphRules {
    pub eps: f64,
    pub min_clearance: f64,
    pub edge_clearance: f64,
    pub hole_clearance: f64,
}

/// Terminal vertices that are not anchors: (position, net, owner label) — pad centres of
/// non-round pads, junctions.
#[derive(Clone, Debug)]
pub struct TerminalInfo {
    pub at: Vec2,
    pub net: i32,
    pub owner: String,
}

/// One end of a strand inside a face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Port {
    Vertex(usize),
    /// Crossing at `(edge, index in cross)`.
    Cross(usize, usize),
    /// Entry from a plain `insert`: capacity only, no topology.
    Detached,
}

#[derive(Clone, Debug)]
struct PathRec {
    start: usize,
    goal: usize,
    faces: Vec<usize>,
    along: Option<usize>,
}

/// `(crossings (edge, slot), faces, (start, goal))` of an inserted path.
pub type PathView<'a> = (&'a [(usize, usize)], &'a [usize], (usize, usize));

#[derive(Clone)]
pub struct RouteGraph {
    pub verts: Vec<VertexInfo>,
    pub edges: Vec<GEdge>,
    pub faces: Vec<Face>,
    pub rules: GraphRules,
    edge_of: HashMap<(usize, usize), usize>,
    faces_of_vertex: Vec<Vec<usize>>,
    /// Per connection: `(edge, slot)`; in travel order for `insert_path`.
    occupancy: Vec<Vec<(usize, usize)>>,
    paths: Vec<Option<PathRec>>,
    vgrid: Grid<usize>,
    rho_max: f64,
}

fn key(p: Vec2) -> (u64, u64) {
    ((p.x + 0.0).to_bits(), (p.y + 0.0).to_bits())
}

fn chord(p: &Prim) -> (Vec2, Vec2) {
    match p {
        Prim::Seg(s) => (s.a, s.b),
        Prim::Arc(a) => (a.point_at(0.0), a.point_at(1.0)),
    }
}

/// Ray-casting parity of `p` against the outline chords; empty outline => inside.
fn inside_outline(p: Vec2, chords: &[(Vec2, Vec2)]) -> bool {
    if chords.is_empty() {
        return true;
    }
    let mut inside = false;
    for &(a, b) in chords {
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if x > p.x {
                inside = !inside;
            }
        }
    }
    inside
}

fn vertex_clr(kind: ObsKind, own_clr: f64, r: &GraphRules) -> f64 {
    match kind {
        ObsKind::Hole => r.hole_clearance,
        ObsKind::Edge => r.edge_clearance,
        ObsKind::Keepout => 0.0,
        ObsKind::Pad | ObsKind::Via | ObsKind::Track | ObsKind::Shape | ObsKind::Terminal => {
            own_clr.max(r.min_clearance)
        }
    }
}

fn sorted_nets(net: i32, tie: &[i32]) -> Vec<i32> {
    let mut v: Vec<i32> = std::iter::once(net)
        .chain(tie.iter().copied())
        .filter(|&n| n != 0)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

impl RouteGraph {
    pub fn build(
        ls: &LayerSketch,
        obs: &LayerObstacles,
        outline: &[Prim],
        terminals: &[TerminalInfo],
        rules: &GraphRules,
    ) -> RouteGraph {
        let cdt: &Cdt = &ls.cdt;
        let nv = cdt.num_vertices();
        let mut term_at: HashMap<(u64, u64), usize> = HashMap::new();
        for (i, t) in terminals.iter().enumerate() {
            term_at.entry(key(t.at)).or_insert(i);
        }
        let mut verts = Vec::with_capacity(nv);
        let mut vgrid = Grid::new(VERT_CELL);
        let mut rho_max = 0.0f64;
        for v in 0..nv {
            let at = cdt.vertex_pos(v);
            let anchor = ls.point_to_anchor.get(v).copied().flatten();
            let info = if let Some(ai) = anchor {
                let a = &obs.anchors[ai];
                VertexInfo {
                    at,
                    anchor,
                    owner: Some(a.label.clone()),
                    nets: sorted_nets(a.net, &a.tie_nets),
                    r: a.r_core,
                    clr: vertex_clr(a.kind, a.own_clr, rules),
                }
            } else if let Some(&ti) = (!cdt.is_split_vertex(v))
                .then(|| term_at.get(&key(at)))
                .flatten()
            {
                let t = &terminals[ti];
                VertexInfo {
                    at,
                    anchor: None,
                    owner: Some(t.owner.clone()),
                    nets: sorted_nets(t.net, &[]),
                    r: 0.0,
                    clr: rules.min_clearance,
                }
            } else {
                VertexInfo {
                    at,
                    anchor: None,
                    owner: None,
                    nets: Vec::new(),
                    r: 0.0,
                    clr: 0.0,
                }
            };
            rho_max = rho_max.max(info.r + info.clr);
            vgrid.insert(Aabb { min: at, max: at }, v);
            verts.push(info);
        }

        let topo = cdt.topology();
        let mut edges: Vec<GEdge> = Vec::with_capacity(topo.edges.len());
        let mut edge_of = HashMap::with_capacity(topo.edges.len());
        for (i, &(a, b, constraint)) in topo.edges.iter().enumerate() {
            let owner_nets = if constraint {
                match ls.constraint_owner.get(&(a, b)) {
                    Some((ObsKind::Edge | ObsKind::Keepout, ..)) | None => Vec::new(),
                    Some((_, net, tie, _)) => sorted_nets(*net, tie),
                }
            } else {
                Vec::new()
            };
            let (pa, pb) = (&verts[a], &verts[b]);
            edges.push(GEdge {
                a,
                b,
                faces: [None, None],
                constraint,
                owner_nets,
                cap: (pb.at - pa.at).len() - pa.r - pb.r,
                c_e: pa.clr.max(pb.clr),
                cross: Vec::new(),
                along: None,
            });
            edge_of.insert((a, b), i);
        }

        let chords: Vec<(Vec2, Vec2)> = outline.iter().map(chord).collect();
        let mut faces = Vec::with_capacity(topo.faces.len());
        let mut faces_of_vertex = vec![Vec::new(); nv];
        for (fi, &v) in topo.faces.iter().enumerate() {
            let find = |x: usize, y: usize| edge_of[&(x.min(y), x.max(y))];
            let e = [find(v[1], v[2]), find(v[2], v[0]), find(v[0], v[1])];
            for j in 0..3 {
                let (x, y) = (v[j], v[(j + 1) % 3]);
                // the face lies on the left of the directed edge x -> y
                let slot = usize::from(x > y);
                edges[find(x, y)].faces[slot] = Some(fi);
                faces_of_vertex[x].push(fi);
            }
            let c = (verts[v[0]].at + verts[v[1]].at + verts[v[2]].at) * (1.0 / 3.0);
            faces.push(Face {
                v,
                e,
                inside: inside_outline(c, &chords),
            });
        }

        RouteGraph {
            verts,
            edges,
            faces,
            rules: *rules,
            edge_of,
            faces_of_vertex,
            occupancy: Vec::new(),
            paths: Vec::new(),
            vgrid,
            rho_max,
        }
    }

    pub fn edge(&self, a: usize, b: usize) -> Option<usize> {
        self.edge_of.get(&(a.min(b), a.max(b))).copied()
    }

    pub fn faces_of_vertex(&self, v: usize) -> &[usize] {
        &self.faces_of_vertex[v]
    }

    /// `(left, right)` vertices of edge `e` for a path crossing it out of face `prev`: entering
    /// from the face left of `a -> b` means travelling clockwise of `a -> b`, so `b` is then on
    /// the left of the direction of travel.
    pub fn crossing_lr(&self, e: usize, prev: usize) -> (usize, usize) {
        let ge = &self.edges[e];
        if ge.faces[0] == Some(prev) {
            (ge.b, ge.a)
        } else {
            (ge.a, ge.b)
        }
    }

    /// Both faces inside the outline, no wire running along it, and (constraint edges) `net`
    /// is an owner net.
    pub fn passable(&self, e: usize, net: i32) -> bool {
        let g = &self.edges[e];
        if g.along.is_some() {
            return false;
        }
        let inside = |f: Option<usize>| f.is_some_and(|f| self.faces[f].inside);
        inside(g.faces[0]) && inside(g.faces[1]) && (!g.constraint || g.owner_nets.contains(&net))
    }

    /// A further connection of width `w`, clearance `c` still fits edge `e` (10 §4).
    pub fn fits(&self, e: usize, w: f64, c: f64) -> bool {
        self.fits_at(e, w, c, self.rules.eps)
    }

    /// [`RouteGraph::fits`] with the build margin `eps` instead of the graph's (the zero-slack
    /// retry tests a corridor at margin 0).
    pub fn fits_at(&self, e: usize, w: f64, c: f64, eps: f64) -> bool {
        let g = &self.edges[e];
        let used: f64 = g.cross.iter().map(|x| x.width + eps + x.clr.max(c)).sum();
        used + w + eps + 2.0 * c.max(g.c_e) <= g.cap
    }

    /// `(need_all - cap).max(0)`.
    pub fn overuse(&self, e: usize) -> f64 {
        let g = &self.edges[e];
        let (Some(first), Some(last)) = (g.cross.first(), g.cross.last()) else {
            return 0.0;
        };
        let eps = self.rules.eps;
        let mut need: f64 = g.cross.iter().map(|x| x.width + eps).sum();
        need += g
            .cross
            .windows(2)
            .map(|p| p[0].clr.max(p[1].clr))
            .sum::<f64>();
        need += first.clr.max(g.c_e) + last.clr.max(g.c_e);
        (need - g.cap).max(0.0)
    }

    pub fn slot_count(&self, e: usize) -> usize {
        self.edges[e].cross.len()
    }

    /// Connection with a wire running along edge `e`, if any.
    pub fn along(&self, e: usize) -> Option<usize> {
        self.edges[e].along
    }

    pub fn occupancy(&self, conn: usize) -> &[(usize, usize)] {
        self.occupancy.get(conn).map_or(&[], |v| v.as_slice())
    }

    fn ensure(&mut self, conn: usize) {
        if self.occupancy.len() <= conn {
            self.occupancy.resize(conn + 1, Vec::new());
            self.paths.resize(conn + 1, None);
        }
    }

    /// Record that entry `x` (formerly at index `old`) now sits at index `new` on edge `e`.
    fn move_entry(&mut self, e: usize, x: CrossEntry, old: usize, new: usize) {
        let occ = &mut self.occupancy[x.conn];
        let pos = match x.k {
            Some(k) => Some(k),
            None => occ.iter().position(|&p| p == (e, old)),
        };
        if let Some(p) = pos {
            occ[p] = (e, new);
        }
    }

    /// Plain insertion at `slot` (capacity-only unless `entry.k` is set): later slots shift.
    /// The occupancy list of `conn` gets `(e, slot)` appended.
    pub fn insert(&mut self, conn: usize, e: usize, slot: usize, entry: CrossEntry) {
        debug_assert_eq!(conn, entry.conn);
        self.ensure(conn);
        let slot = slot.min(self.edges[e].cross.len());
        self.edges[e].cross.insert(slot, entry);
        for i in (slot + 1..self.edges[e].cross.len()).rev() {
            let x = self.edges[e].cross[i];
            self.move_entry(e, x, i - 1, i);
        }
        match entry.k {
            Some(k) => {
                let occ = &mut self.occupancy[conn];
                debug_assert_eq!(occ.len(), k);
                occ.push((e, slot));
            }
            None => self.occupancy[conn].push((e, slot)),
        }
    }

    /// Insert a routed path: `crossings` are `(edge, slot)` in travel order with slots as seen
    /// by the search (the graph state before this call); `faces.len() == crossings.len() + 1`
    /// (the faces entered, start face first). An empty path runs along `edge(start_v, goal_v)`.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_path(
        &mut self,
        conn: usize,
        start_v: usize,
        goal_v: usize,
        crossings: &[(usize, usize)],
        faces: &[usize],
        width: f64,
        clr: f64,
    ) {
        self.ensure(conn);
        debug_assert!(self.paths[conn].is_none() && self.occupancy[conn].is_empty());
        let mut along = None;
        if crossings.is_empty() {
            along = self.edge(start_v, goal_v);
            if let Some(e) = along {
                self.edges[e].along = Some(conn);
            }
        } else {
            debug_assert_eq!(faces.len(), crossings.len() + 1);
        }
        // An edge crossed twice: the second slot refers to the state before this call.
        let mut earlier: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (k, &(e, s0)) in crossings.iter().enumerate() {
            let prev = earlier.entry(e).or_default();
            let slot = s0 + prev.iter().filter(|&&p| p <= s0).count();
            prev.push(s0);
            self.insert(
                conn,
                e,
                slot,
                CrossEntry {
                    conn,
                    width,
                    clr,
                    k: Some(k),
                },
            );
        }
        self.paths[conn] = Some(PathRec {
            start: start_v,
            goal: goal_v,
            faces: faces.to_vec(),
            along,
        });
    }

    /// The other end of the strand of entry `i` on edge `e` inside `face` (one of the two faces
    /// of `e`). `Detached` for entries from a plain `insert`.
    pub fn strand(&self, e: usize, i: usize, face: usize) -> Port {
        let x = self.edges[e].cross[i];
        let (Some(k), Some(Some(p))) = (x.k, self.paths.get(x.conn)) else {
            return Port::Detached;
        };
        let occ = &self.occupancy[x.conn];
        if p.faces.get(k) == Some(&face) {
            if k == 0 {
                Port::Vertex(p.start)
            } else {
                Port::Cross(occ[k - 1].0, occ[k - 1].1)
            }
        } else if p.faces.get(k + 1) == Some(&face) {
            if k + 1 == occ.len() {
                Port::Vertex(p.goal)
            } else {
                Port::Cross(occ[k + 1].0, occ[k + 1].1)
            }
        } else {
            Port::Detached
        }
    }

    /// `(crossings (edge, current slot), faces, (start, goal))` of a path inserted with
    /// `insert_path`.
    pub fn path_of(&self, conn: usize) -> Option<PathView<'_>> {
        let p = self.paths.get(conn)?.as_ref()?;
        Some((&self.occupancy[conn], &p.faces, (p.start, p.goal)))
    }

    /// Remove every entry of `conn`, clear its `along`, and renumber the slots of the other
    /// connections on the touched edges. O(occupancy · n_max).
    pub fn remove_conn(&mut self, conn: usize) {
        if conn >= self.occupancy.len() {
            return;
        }
        let mut touched: Vec<usize> = self.occupancy[conn].iter().map(|&(e, _)| e).collect();
        touched.sort_unstable();
        touched.dedup();
        for e in touched {
            let old = std::mem::take(&mut self.edges[e].cross);
            let mut kept = Vec::with_capacity(old.len());
            for (i, x) in old.into_iter().enumerate() {
                if x.conn == conn {
                    continue;
                }
                let new = kept.len();
                kept.push(x);
                if new != i {
                    self.move_entry(e, x, i, new);
                }
            }
            self.edges[e].cross = kept;
        }
        self.occupancy[conn].clear();
        if let Some(p) = self.paths[conn].take() {
            if let Some(e) = p.along {
                self.edges[e].along = None;
            }
        }
    }

    /// Vertices whose position lies inside `bb`, sorted.
    pub fn verts_near(&self, bb: Aabb) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .vgrid
            .query(bb)
            .into_iter()
            .filter(|&i| {
                let p = self.verts[i].at;
                p.x >= bb.min.x && p.x <= bb.max.x && p.y >= bb.min.y && p.y <= bb.max.y
            })
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Largest `r + clr` over all vertices.
    pub fn rho_max(&self) -> f64 {
        self.rho_max
    }
}
