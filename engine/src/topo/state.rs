//! Topology state (plan "Rough multilayer router: one-shot spec"): one projection CDT over the
//! obstacles of all layers, the passes of every wire through its edges (ordered per edge) and
//! the crossings, which are derived from interleaving inside a triangle, never stored.
//!
//! Points inside triangles (branch points, vias, free ends): such a point is made a vertex of the
//! projection (`Projection` built with nodes, `VertKind::Node`), which splits its triangle into a
//! fan. What is left is plain boundary chords, so the crossing derivation needs no extra rule. A
//! wire that ends at the node has a corner terminal (like a pad centre); a wire that runs through
//! it turns at the corner inside one traversal ([`Wire::turns`]: the chords of that traversal are
//! entry -> corner -> exit) or walks around the fan by zero-length passes at the vertex; every
//! other wire passes the node on one side by crossing the fan edges, which is exactly how it is
//! ordered against the point. For S2: P2 and P3 do not create points (vias are intervals between
//! events, `ViaSlot`); a via is pinned to a vertex only when P4/P6 need it.
//!
//! Coordinates are nm (math frame, Y up). Storage is `Vec` with index ids and nothing iterates a
//! `HashMap`, so every result is deterministic.
//!
//! Complexity: building the projection is O(V log V) for the CDT plus O(T · obstacles) for the
//! region labels (bbox prefilter); `crossings(w)` is O(Σ traversals in w's triangles); `insert` and
//! `remove` are O(passes · edge load).

use super::demand::{need_in, Bound, Item};
use crate::cdt::{Cdt, CdtFailure, CdtReport};
use crate::geom::Vec2;
use std::collections::BTreeMap;

pub type VertId = usize;
pub type EdgeId = usize;
pub type TriId = usize;
pub type WireId = usize;
pub type PassId = usize;
pub type ObsId = usize;
/// `Wire::pa` / `pb` of a node terminal (not a pad).
pub const NO_OBS: ObsId = usize::MAX;

/// Bottleneck passes of one wire at most this many full-width passes apart form one necked
/// passage (M-C, [`State::refit_wire`]).
const NECK_JOIN: usize = 2;

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
    /// How far the true copper may lie outside `poly` (nm): the sagitta of the polygon of its
    /// rounded parts (`r (1 - cos(pi / steps))`), 0 for a straight-sided shape or a keepout.
    pub sag: f64,
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
    /// A point inserted for the State (branch point, via, free end), not an obstacle.
    Node,
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
    /// The copper-to-edge rule (nm) when the project sets that check to `ignore`: the cuts at the
    /// outline then bound with `edge_clear` (0) and a pass that fits only without the rule pays
    /// the `ignored_rule` factor; 0 when the rule is hard.
    pub edge_clear_ignored: f64,
    /// Custom courtyard clearance rules `(polygon, clearance nm)`, in file order (the last match
    /// wins); set by the builder.
    pub regions: Vec<(Vec<Vec2>, f64)>,
    /// [`Projection::regime`] of every edge, filled by [`Projection::set_regions`].
    pub edge_regime: Vec<Option<f64>>,
    /// Whether the edge is a constraint of the triangulation (obstacle outlines, the board edge
    /// and the LCT cuts).
    pub edge_constraint: Vec<bool>,
    /// Keepouts that forbid vias (D6a), on any copper layer: a through via spans every layer,
    /// so none may touch one. Polygons (nm); set by `board::build`.
    pub no_via: Vec<Vec<Vec2>>,
    /// How the triangulation was built ([`Cdt::build_reported`]): the path, and the input
    /// constraints left out (the board builder names them).
    pub cdt: CdtReport,
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
        Self::build_reported(n_layers, obstacles, points, constraints, inside).map_err(|e| e.0)
    }

    /// [`Projection::build`]; a failed triangulation also returns its [`CdtReport`] (the
    /// constraint spade failed on), so the caller can name the copper involved.
    pub fn build_reported(
        n_layers: usize,
        obstacles: Vec<Obstacle>,
        points: &[(Vec2, VertKind)],
        constraints: &[(usize, usize)],
        inside: impl Fn(Vec2) -> bool,
    ) -> Result<Projection, CdtFailure> {
        let pts: Vec<Vec2> = points.iter().map(|p| p.0).collect();
        let (cdt, cdt_report) = Cdt::build_reported(&pts, constraints)?;
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
        let edge_constraint: Vec<bool> = topo.edges.iter().map(|&(_, _, c)| c).collect();
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
            edge_clear_ignored: 0.0,
            regions: Vec::new(),
            edge_regime: Vec::new(),
            edge_constraint,
            no_via: Vec::new(),
            cdt: cdt_report,
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

    /// Room of edge `e` on layer `l` for a wire of `net`: an edge between two obstacles of the
    /// wire's own net is no cut for it (it lies on copper all the way). With one end of its own
    /// net the edge is a cut (M-C pad exits): the wire's centre may sit on that end, so it needs
    /// half its width plus the far end's clearance ([`super::demand::need`]); before M-C such an
    /// edge was no cut either, and P2 planned exits between fine-pitch neighbours.
    pub fn room_for(&self, e: EdgeId, l: usize, net: i32) -> f64 {
        let own = |v: VertId| matches!(self.vkind[v], VertKind::Outline(o) if self.obstacles[o].net == Some(net));
        let [p, q] = self.edges[e];
        if own(p) && own(q) {
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

    /// Set the courtyard clearance rules and precompute the regime of every edge. O(edges ·
    /// regions).
    pub fn set_regions(&mut self, regions: Vec<(Vec<Vec2>, f64)>) {
        self.regions = regions;
        self.edge_regime = if self.regions.is_empty() {
            Vec::new()
        } else {
            self.edges
                .iter()
                .map(|&[p, q]| self.regime_at((self.pos[p] + self.pos[q]) * 0.5))
                .collect()
        };
    }

    /// The regime of edge `e` (at its midpoint).
    pub fn regime(&self, e: EdgeId) -> Option<f64> {
        self.edge_regime.get(e).copied().flatten()
    }

    /// Room of edge `e` on layer `l` (nm; `INFINITY` where the edge is no cut on that layer).
    pub fn room_of(&self, e: EdgeId, l: usize) -> f64 {
        self.room[e][l]
    }

    /// The obstacles' edges between obstacles `o1` and `o2` (CDT edges whose ends lie on the two),
    /// ascending. O(edges).
    pub fn edges_between(&self, o1: ObsId, o2: ObsId) -> Vec<EdgeId> {
        let of = |v: VertId| match self.vkind[v] {
            VertKind::Outline(o) | VertKind::Centre(o) => Some(o),
            _ => None,
        };
        (0..self.edges.len())
            .filter(|&e| {
                let [p, q] = self.edges[e];
                matches!((of(p), of(q)), (Some(a), Some(b)) if (a, b) == (o1, o2) || (a, b) == (o2, o1))
            })
            .collect()
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
    /// `pa` / `pb` is [`NO_OBS`] for a node terminal (dead end, junction, the plan's
    /// `Terminal::Branch`): `a` / `b` is then a `VertKind::Node` vertex.
    /// Corners the wire turns at inside a traversal: `(traversal k, vertex)`, in wire order. The
    /// chords of traversal `k` are entry -> turn corners -> exit.
    pub turns: Vec<(usize, VertId)>,
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
    /// Which chord of traversal `k` / `other_k` (0 unless the traversal turns at a corner).
    pub sub: usize,
    pub other_sub: usize,
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

/// The routed part of a [`State`] without its projection (4.3, `[optimize] initial = "once"`):
/// what one cycle hands to the next so that it improves the plan instead of routing it again. The
/// projection is the call's (built once, 4.2), so the ids stay valid.
#[derive(Clone, Debug, Default)]
pub struct Topology {
    passes: Vec<Pass>,
    edge_order: Vec<Vec<PassId>>,
    wires: Vec<Wire>,
    tri_trav: Vec<Vec<(WireId, usize)>>,
    ends: Vec<u32>,
    pass_w: Vec<f64>,
}

#[derive(Clone, Debug)]
pub struct State<'p> {
    pub proj: &'p Projection,
    pub passes: Vec<Pass>,
    /// Per edge, ordered from `p` to `q`.
    pub edge_order: Vec<Vec<PassId>>,
    pub wires: Vec<Wire>,
    /// Per triangle: the traversals `(wire, k)` through it.
    pub tri_trav: Vec<Vec<(WireId, usize)>>,
    /// Per edge: in how many earlier rounds of the negotiated rip-up-and-reroute it was over-full
    /// (the `congestion` factor prices a pass through it).
    pub history: Vec<f64>,
    /// Per vertex: how many alive wires start or end there.
    pub ends: Vec<u32>,
    /// Per obstacle: its centre vertex, if it has one.
    pub centre: Vec<Option<VertId>>,
    /// Per obstacle: the triangles of its ring (a corner on its outline, not inside it, not a wall).
    pub ring: Vec<Vec<TriId>>,
    /// `[search] crossing_vias = "via"` (D6b): P2 prices the via a crossing of two stretches held
    /// to the same layer implies and narrows the layer pin ([`super::init::cross_pin`]); false
    /// (default, "crossing"): only crossings no layer assignment separates are priced (overflow).
    pub cross_vias: bool,
    /// Width of each pass (nm, parallel to `passes`; M-C width in P2): its wire's nominal width,
    /// or the wire's neck floor where the pass lies in a necked stretch ([`State::refit_wire`]).
    pub pass_w: Vec<f64>,
    /// Neck floor per net (nm; M-C): the narrowest a necked stretch of the net may be (KiCad
    /// minimum track width, custom-rule minimum). Empty: no wire is ever necked in the plan.
    pub floors: std::collections::HashMap<i32, f64>,
    /// A necked stretch runs from the wire's nearest pad end to the bottleneck (user rule: a neck
    /// starts at the footprint and may reach further back; never a neck in the middle of a
    /// full-width run). False: only the bottleneck passes are necked.
    pub neck_from_pad: bool,
    /// `[search] length = "taut"` (shopping list 4.3): the search prices a wire's length as the
    /// shortest path through its sleeve that keeps the clearances ([`State::gap_spans`],
    /// [`super::init`] funnel step; Polubasov 2010, «Начальная трассировка»: «кратчайший путь по
    /// триангуляции с учетом необходимых зазоров»). False ("even", today's default): between
    /// evenly spaced points on each crossed edge ([`State::gap_point`]).
    pub taut_len: bool,
    /// Lay the wires of a double-crossing removal anew ([`State::relay`], 4.3c; `initial = "once"`).
    pub relay_after_uncross: bool,
    /// The TopoR loop's layer-aware reroute (shopping list 5.2, `[optimize] loop = "topor"`): per
    /// wire, the layers of each traversal under the loop's current layer assignment (bit per
    /// layer, both layers of a via interval; [`super::optimise::layers_now`]). A crossing is then
    /// priced by the via it implies against the crossed stretch's present layer
    /// ([`State::held_layers`]). Empty (default, every other caller): the stretch's own held
    /// layers only, as before.
    pub layers_now: Vec<Vec<u64>>,
}

impl<'p> State<'p> {
    pub fn new(proj: &'p Projection) -> Self {
        let mut centre = vec![None; proj.obstacles.len()];
        for (v, k) in proj.vkind.iter().enumerate() {
            if let VertKind::Centre(o) = *k {
                centre[o] = Some(v);
            }
        }
        let mut ring: Vec<Vec<TriId>> = vec![Vec::new(); proj.obstacles.len()];
        for (t, vs) in proj.tris.iter().enumerate().filter(|(t, _)| !proj.tri_out[*t]) {
            for &v in vs {
                if let VertKind::Outline(o) = proj.vkind[v] {
                    if !proj.tri_obs[t].contains(&o) && !ring[o].contains(&t) {
                        ring[o].push(t);
                    }
                }
            }
        }
        State {
            centre,
            ring,
            proj,
            passes: Vec::new(),
            edge_order: vec![Vec::new(); proj.edges.len()],
            wires: Vec::new(),
            tri_trav: vec![Vec::new(); proj.tris.len()],
            history: vec![0.0; proj.edges.len()],
            ends: vec![0; proj.pos.len()],
            cross_vias: false,
            pass_w: Vec::new(),
            floors: std::collections::HashMap::new(),
            neck_from_pad: true,
            taut_len: false,
            relay_after_uncross: true,
            layers_now: Vec::new(),
        }
    }

    /// The routed part of the state ([`Topology`]).
    pub fn topology(&self) -> Topology {
        Topology { passes: self.passes.clone(), edge_order: self.edge_order.clone(), wires: self.wires.clone(), tri_trav: self.tri_trav.clone(), ends: self.ends.clone(), pass_w: self.pass_w.clone() }
    }

    /// Put a [`Topology`] of the same projection back (the wires, their passes in edge order).
    pub fn restore(&mut self, t: Topology) {
        assert_eq!(t.edge_order.len(), self.edge_order.len(), "a topology of the same projection");
        (self.passes, self.edge_order, self.wires, self.tri_trav, self.ends, self.pass_w) = (t.passes, t.edge_order, t.wires, t.tri_trav, t.ends, t.pass_w);
    }

    /// The narrowest width a wire of `net` with nominal width `w` may neck to (`w`: none).
    pub fn w_min(&self, net: i32, w: f64) -> f64 {
        self.floors.get(&net).map_or(w, |&f| f.min(w))
    }

    /// Whether pass `p` lies in a necked stretch of its wire.
    pub fn necked(&self, p: PassId) -> bool {
        self.pass_w[p] < self.wires[self.passes[p].wire].w - 0.5
    }

    /// The necked stretches of wire `w` as counts of passes: from end `a`, from end `b`, and the
    /// necked passes in between that belong to neither (only without `neck_from_pad`).
    pub fn neck_runs(&self, w: WireId) -> (usize, usize, Vec<usize>) {
        let ps = &self.wires[w].passes;
        let n = ps.len();
        let na = ps.iter().take_while(|&&p| self.necked(p)).count();
        if na == n {
            return (n, 0, Vec::new());
        }
        let nb = ps.iter().rev().take_while(|&&p| self.necked(p)).count();
        let mid = (na..n - nb).filter(|&k| self.necked(ps[k])).collect();
        (na, nb, mid)
    }

    /// Space the passes of edge `e` need on one layer with every pass at its wire's nominal
    /// width (`floor`: at its neck floor instead), optionally with `extra` at gap `g`.
    pub fn edge_need_at(&self, e: EdgeId, floor: bool, extra: Option<(usize, Item)>) -> f64 {
        let mut items: Vec<Item> = self.edge_order[e]
            .iter()
            .map(|&p| {
                let w = &self.wires[self.passes[p].wire];
                Item { net: w.net, w: if floor { self.w_min(w.net, w.w) } else { w.w }, s: w.s }
            })
            .collect();
        if let Some((g, it)) = extra {
            items.insert(g.min(items.len()), it);
        }
        self.proj.edge_demand(e, &items)
    }

    /// Decide the width of every pass of wire `w` (M-C, width in P2; TopoR 6.3 manual p. 56: the
    /// minimum width "in bottlenecks unless a different path is available or near pads that are
    /// narrower than the wire"): a pass is a bottleneck when the passes of its edge at their
    /// nominal widths need more than the room of the layers its stretch is held to (the room
    /// the P2 search prices, [`State::stretch_layers`]); a bottleneck pass takes the neck floor,
    /// and with `neck_from_pad` so does the stretch from the nearest pad end up to it (a neck
    /// that frees room on the way costs no extra capacity: the narrowest part sets it anyway).
    /// A deterministic function of the topology. O(passes x edge load).
    pub fn refit_wire(&mut self, w: WireId) {
        let (net, wn) = (self.wires[w].net, self.wires[w].w);
        let ws = self.w_min(net, wn);
        let n = self.wires[w].passes.len();
        if !self.wires[w].alive || ws >= wn {
            for k in 0..n {
                let p = self.wires[w].passes[k];
                self.pass_w[p] = wn;
            }
            return;
        }
        let held = self.stretch_layers(w);
        let pr = self.proj;
        let mut neck = vec![false; n];
        for (k, nk) in neck.iter_mut().enumerate() {
            let p = self.wires[w].passes[k];
            let e = self.passes[p].edge;
            let (t1, t2) = (self.wires[w].tris[k], self.wires[w].tris[k + 1]);
            let both = pr.allowed(t1, net).map_or(0, |m| m.0) & pr.allowed(t2, net).map_or(0, |m| m.0);
            let h = held[k] & held[k + 1];
            let h = if h != 0 { h } else { held[k] | held[k + 1] };
            let m = if both & h != 0 { both & h } else { both };
            let room: f64 = (0..pr.n_layers).filter(|&l| (m >> l) & 1 == 1).map(|l| pr.room_for(e, l, net)).sum();
            if room.is_finite() && self.edge_need_at(e, false, None) > room + 1.0 {
                *nk = true;
            }
            if std::env::var("TOPO_NECKDBG").is_ok() {
                eprintln!("NECKDBG wire {w} k {k} e {e} mask {m:b} held {:b}/{:b} both {both:b} room {room:.0} need {:.0}", held[k], held[k + 1], self.edge_need_at(e, false, None));
            }
        }
        // Pad exits (M-C item 2; TopoR 6.3 manual p. 56: the minimum width "near pads that are
        // narrower than the wire"): an end whose first stretch passes another net's copper closer
        // than half the width plus the clearance necks its first two passes.
        let exit = [n > 0 && self.exit_tight(w, 0), n > 0 && self.exit_tight(w, 1)];
        if self.neck_from_pad {
            let (pa, pb) = (self.wires[w].pa != NO_OBS, self.wires[w].pb != NO_OBS);
            let (mut na, mut nb) = (if exit[0] { n.min(2) } else { 0 }, if exit[1] { n.min(2) } else { 0 });
            // Bottleneck passes in clusters (at most NECK_JOIN full-width passes between two of
            // them: one passage, e.g. the triangles of a slot); a cluster necks from the pad end
            // that gives the shorter stretch (ties: `a`), so no full-width piece is left inside.
            let ks: Vec<usize> = (0..n).filter(|&k| neck[k]).collect();
            let mut i = 0;
            while i < ks.len() {
                let mut j = i;
                while j + 1 < ks.len() && ks[j + 1] - ks[j] <= NECK_JOIN + 1 {
                    j += 1;
                }
                let (lo, hi) = (ks[i], ks[j]);
                let (from_a, from_b) = (hi + 1, n - lo);
                if pa && (!pb || from_a <= from_b) {
                    na = na.max(from_a);
                } else if pb {
                    nb = nb.max(from_b);
                }
                i = j + 1;
            }
            for (k, nk) in neck.iter_mut().enumerate() {
                *nk = *nk || k < na || k + nb >= n;
            }
        } else {
            for (end, &x) in exit.iter().enumerate() {
                if x {
                    for j in 0..n.min(2) {
                        neck[if end == 0 { j } else { n - 1 - j }] = true;
                    }
                }
            }
        }
        for (k, &nk) in neck.iter().enumerate() {
            let p = self.wires[w].passes[k];
            self.pass_w[p] = if nk { ws } else { wn };
        }
    }

    /// Whether the exit of wire `w` from its pad at `end` (0 = `a`, 1 = `b`) is too narrow for its
    /// nominal width (M-C pad exits, pin access): the stretch from the pad centre through its
    /// first two passes (the pass points) comes closer than `w/2 + clearance` to the copper of an
    /// obstacle of another net next to the pad or to those passes' edges (CDT neighbours only).
    /// O(neighbourhood x outline).
    pub fn exit_tight(&self, w: WireId, end: usize) -> bool {
        let pr = self.proj;
        let wi = &self.wires[w];
        let n = wi.passes.len();
        let (pad, v) = if end == 0 { (wi.pa, wi.a) } else { (wi.pb, wi.b) };
        if pad == NO_OBS || n == 0 {
            return false;
        }
        let ks: Vec<usize> = if end == 0 { (0..n.min(2)).collect() } else { (0..n.min(2)).map(|j| n - 1 - j).collect() };
        let mut pts = vec![pr.pos[v]];
        pts.extend(ks.iter().map(|&k| self.pass_point(wi.passes[k])));
        // Candidate obstacles: those with a vertex next to the pad's outline or to the edges.
        let mut verts: Vec<VertId> = ks.iter().flat_map(|&k| pr.edges[self.passes[wi.passes[k]].edge]).collect();
        verts.extend(pr.obstacles[pad].poly.iter().filter_map(|&q| pr.vertex_at(q, 2.0)));
        let mut obs: Vec<ObsId> = Vec::new();
        for &u in &verts {
            for &t in &pr.vert_tris[u] {
                for &x in &pr.tris[t] {
                    if let VertKind::Outline(o) = pr.vkind[x] {
                        if o != pad && !obs.contains(&o) {
                            obs.push(o);
                        }
                    }
                }
            }
        }
        let me = &pr.obstacles[pad];
        for o in obs {
            let ob = &pr.obstacles[o];
            if ob.net == Some(wi.net) || !(ob.drilled || ob.layers.0 & me.layers.0 != 0) {
                continue;
            }
            let need = 0.5 * wi.w + if ob.bare { ob.clear } else { ob.clear.max(wi.s) } - 1.0;
            let m = ob.poly.len();
            for sgm in pts.windows(2) {
                for i in 0..m {
                    if seg_seg_dist(sgm[0], sgm[1], ob.poly[i], ob.poly[(i + 1) % m]) < need {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// [`State::refit_wire`] for `w` and every alive wire that shares an edge with it (its
    /// passes change their bottlenecks). No-op without neck floors.
    pub fn refit_around(&mut self, w: WireId) {
        if self.floors.is_empty() {
            return;
        }
        let mut ws: Vec<WireId> = vec![w];
        for &p in &self.wires[w].passes {
            ws.extend(self.edge_order[self.passes[p].edge].iter().map(|&q| self.passes[q].wire));
        }
        ws.sort_unstable();
        ws.dedup();
        for x in ws {
            if self.wires[x].alive {
                self.refit_wire(x);
            }
        }
    }

    /// Shopping list 2.4: the pass widths the cut model's necks ask for (`(wire, traversal,
    /// width)`, `CutPrice::necks`), laid out by the plan's neck rule ([`State::refit_wire`]): each
    /// neck at the pass of its traversal (traversal `n` = the last pass); with `neck_from_pad`, the
    /// necks of one cluster (at most `NECK_JOIN` passes apart) reach to the pad end that gives the
    /// shorter stretch (ties `a`; a wire without a pad end: the nearer end, its T split), each pass
    /// at the narrowest width of the cluster's necks at or beyond it, so the neck narrows from the
    /// bottleneck to the pad and is never wide in between (route-order-and-neck-rule). Only passes
    /// that become narrower than they are now: `pass -> width`. Deterministic. O(necks + passes
    /// of the necked wires).
    pub fn cut_neck_widths(&self, necks: &[(WireId, usize, f64)]) -> BTreeMap<PassId, f64> {
        let mut by_w: BTreeMap<WireId, BTreeMap<usize, f64>> = BTreeMap::new();
        for &(w, k, wn) in necks {
            let Some(wi) = self.wires.get(w).filter(|wi| wi.alive && !wi.passes.is_empty()) else { continue };
            if wn >= wi.w - 0.5 {
                continue;
            }
            let k = k.min(wi.passes.len() - 1);
            let e = by_w.entry(w).or_default().entry(k).or_insert(wn);
            *e = e.min(wn);
        }
        let mut out = BTreeMap::new();
        for (w, ks) in by_w {
            let wi = &self.wires[w];
            let n = wi.passes.len();
            let mut pw = vec![f64::INFINITY; n];
            let ks: Vec<(usize, f64)> = ks.into_iter().collect();
            if !self.neck_from_pad {
                for &(k, wn) in &ks {
                    pw[k] = pw[k].min(wn);
                }
            } else {
                let (pa, pb) = (wi.pa != NO_OBS, wi.pb != NO_OBS);
                let mut i = 0;
                while i < ks.len() {
                    let mut j = i;
                    while j + 1 < ks.len() && ks[j + 1].0 - ks[j].0 <= NECK_JOIN + 1 {
                        j += 1;
                    }
                    let (lo, hi) = (ks[i].0, ks[j].0);
                    let (from_a, from_b) = (hi + 1, n - lo);
                    let to_a = if pa != pb { pa } else { from_a <= from_b };
                    let cl = &ks[i..=j];
                    if to_a {
                        // Pass m takes the narrowest neck at or beyond it (towards the bottleneck).
                        for (m, x) in pw.iter_mut().enumerate().take(hi + 1) {
                            let wn = cl.iter().filter(|c| c.0 >= m).map(|c| c.1).fold(f64::INFINITY, f64::min);
                            *x = x.min(wn);
                        }
                    } else {
                        for (m, x) in pw.iter_mut().enumerate().skip(lo) {
                            let wn = cl.iter().filter(|c| c.0 <= m).map(|c| c.1).fold(f64::INFINITY, f64::min);
                            *x = x.min(wn);
                        }
                    }
                    i = j + 1;
                }
            }
            for (k, &wn) in pw.iter().enumerate() {
                let p = wi.passes[k];
                if wn < self.pass_w[p] - 0.5 {
                    out.insert(p, wn);
                }
            }
        }
        out
    }

    /// Write the widths of [`State::cut_neck_widths`] into the pass widths (narrower only).
    pub fn apply_neck_widths(&mut self, m: &BTreeMap<PassId, f64>) {
        for (&p, &wn) in m {
            self.pass_w[p] = self.pass_w[p].min(wn);
        }
    }

    /// The necked stretches of wire `w` (the archive's shrinkages, counted as the TopoR loop counts
    /// them from [`State::neck_runs`]: one per necked end run, one per run of necked passes in
    /// between) with the passes of `extra` necked too. O(passes).
    pub fn neck_stretches_with(&self, w: WireId, extra: &BTreeMap<PassId, f64>) -> usize {
        let ps = &self.wires[w].passes;
        let n = ps.len();
        let nk = |p: PassId| self.necked(p) || extra.contains_key(&p);
        let na = ps.iter().take_while(|&&p| nk(p)).count();
        if na == n {
            return usize::from(n > 0);
        }
        let nb = ps.iter().rev().take_while(|&&p| nk(p)).count();
        let mid: Vec<usize> = (na..n - nb).filter(|&k| nk(ps[k])).collect();
        usize::from(na > 0) + usize::from(nb > 0) + mid.iter().enumerate().filter(|&(i, &k)| i == 0 || mid[i - 1] + 1 != k).count()
    }

    /// The pass `p` as a demand item.
    pub fn item(&self, p: PassId) -> Item {
        let w = &self.wires[self.passes[p].wire];
        Item { net: w.net, w: self.pass_w[p], s: w.s }
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

    /// [`State::edge_need`] with `me` inserted at every gap `0..=passes`, in one go, into `out`
    /// (`alone` is the need of `me` on an empty edge; `items` is a scratch buffer).
    /// O(passes²) without allocation per gap.
    pub fn edge_need_gaps(&self, e: EdgeId, me: Item, alone: f64, items: &mut Vec<Item>, out: &mut Vec<f64>) {
        out.clear();
        let ord = &self.edge_order[e];
        if ord.is_empty() {
            out.push(alone);
            return;
        }
        items.clear();
        items.extend(ord.iter().map(|&p| self.item(p)));
        let (regime, lo, hi) = (self.proj.regime(e), self.proj.bound(self.proj.edges[e][0]), self.proj.bound(self.proj.edges[e][1]));
        for g in 0..=ord.len() {
            items.insert(g, me);
            out.push(need_in(regime, items, lo, hi));
            items.remove(g);
        }
    }

    /// [`State::edge_need_gaps`] with every pass of the edge at its wire's neck floor and `me`
    /// as given (M-C: whether the gap fits when everything there necks). O(passes²).
    pub fn edge_need_gaps_floor(&self, e: EdgeId, me: Item, items: &mut Vec<Item>, out: &mut Vec<f64>) {
        out.clear();
        items.clear();
        items.extend(self.edge_order[e].iter().map(|&p| {
            let w = &self.wires[self.passes[p].wire];
            Item { net: w.net, w: self.w_min(w.net, w.w), s: w.s }
        }));
        let (regime, lo, hi) = (self.proj.regime(e), self.proj.bound(self.proj.edges[e][0]), self.proj.bound(self.proj.edges[e][1]));
        for g in 0..=items.len() {
            items.insert(g, me);
            out.push(need_in(regime, items, lo, hi));
            items.remove(g);
        }
    }

    /// What bounds the end vertex `v` of a cut for a pass that may run on the layers `both`: a
    /// pad on only some of those layers does not bound it (the pass may run on another layer
    /// under or beside it), anything else as [`Projection::bound`].
    fn bound_on(&self, v: VertId, both: u64) -> Bound {
        let pr = self.proj;
        match pr.vkind[v] {
            VertKind::Outline(o) if !pr.obstacles[o].drilled && both != 0 && pr.obstacles[o].layers.0 & both != both => Bound::Open,
            _ => pr.bound(v),
        }
    }

    /// Where the centre line of `me` may cross edge `e` at gap `g` keeping its clearances (4.3,
    /// "the shortest path ... keeping the clearances", Polubasov 2010): the interval `(lo, hi)`
    /// in nm along the edge from its end `p`, with the passes before the gap packed towards `p`
    /// and those after it towards `q` at the demand model's spacing ([`super::demand::need`]: the
    /// end object's clearance, the widths, the larger clearance between different nets; a run of
    /// the end pad's own net may lie on the pad). `skip`: a pass of the edge left out (the wire
    /// whose own span this is). Where the gap is over-full (`lo > hi`) the interval collapses to
    /// the middle of the two (the overflow is priced apart). `both`: the layers the pass may take
    /// ([`State::bound_on`]). O(passes²) for all gaps.
    pub fn gap_spans(&self, e: EdgeId, me: Item, both: u64, skip: Option<PassId>, out: &mut Vec<(f64, f64)>) {
        out.clear();
        let pr = self.proj;
        let [vp, vq] = pr.edges[e];
        let len = (pr.pos[vq] - pr.pos[vp]).len();
        let (bp, bq) = (self.bound_on(vp, both), self.bound_on(vq, both));
        let regime = pr.regime(e);
        let items: Vec<Item> = self.edge_order[e].iter().filter(|&&p| Some(p) != skip).map(|&p| self.item(p)).collect();
        let own = |b: &Bound, it: &Item| matches!(*b, Bound::Pad { net: Some(n), .. } if n == it.net);
        let sp = |a: &Item, b: &Item| if a.net == b.net { 0.0 } else { regime.unwrap_or(a.s.max(b.s)) };
        // A courtyard rule replaces the clearances to pads and between passes (`need_in`).
        let end_c = |b: Bound| match (regime, b) {
            (Some(r), Bound::Pad { net, .. }) => Bound::Pad { net, clear: r }.clearance(&Item { s: r, ..me }),
            _ => b.clearance(&me),
        };
        // Centre of `me` from one end, with `before` packed against that end (nearest first). At
        // an open end (a free point: nothing to keep clear of) the first centre line may lie on
        // the vertex itself.
        let from_end = |b: Bound, before: &[Item]| -> f64 {
            if own(&b, &me) && before.iter().all(|x| own(&b, x)) {
                return 0.0;
            }
            let open = matches!(b, Bound::Open);
            match (before.first(), before.last()) {
                (None, _) | (_, None) => if open { 0.0 } else { end_c(b) + 0.5 * me.w },
                (Some(first), Some(last)) => need_in(regime, before, b, Bound::Open) - if open { 0.5 * first.w } else { 0.0 } + sp(last, &me) + 0.5 * me.w,
            }
        };
        let mut rev: Vec<Item> = Vec::with_capacity(items.len());
        for g in 0..=items.len() {
            let lo = from_end(bp, &items[..g]);
            rev.clear();
            rev.extend(items[g..].iter().rev().copied());
            let hi = len - from_end(bq, &rev);
            out.push(if lo <= hi {
                (lo, hi)
            } else {
                let m = (0.5 * (lo + hi)).clamp(0.0, len);
                (m, m)
            });
        }
    }

    /// The span of pass `p` on its edge ([`State::gap_spans`] of its gap with the pass itself
    /// left out), as two points from end `p` to end `q` of the edge. O(passes²) of the edge.
    pub fn pass_span(&self, p: PassId, both: u64) -> (Vec2, Vec2) {
        let e = self.passes[p].edge;
        let g = self.order_index(p);
        let mut sp = Vec::new();
        let it = self.item(p);
        self.gap_spans(e, it, both, Some(p), &mut sp);
        let [vp, vq] = self.proj.edges[e];
        let (a, b) = (self.proj.pos[vp], self.proj.pos[vq]);
        let len = (b - a).len().max(1.0);
        let (lo, hi) = sp[g];
        (a + (b - a) * (lo / len), a + (b - a) * (hi / len))
    }

    /// Length (nm) of wire `w` as the search prices it: with `taut_len`, the shortest path from
    /// terminal to terminal through the spans of its passes and the corners it turns at
    /// ([`State::pass_span`], [`super::pack::funnel`]); else the polyline through the evenly
    /// spaced pass points ([`State::polyline`]). O(passes · edge load²).
    pub fn wire_len(&self, w: WireId) -> f64 {
        if !self.taut_len {
            return self.polyline(w).windows(2).map(|x| (x[1] - x[0]).len()).sum();
        }
        self.taut_points(w).windows(2).map(|x| (x[1] - x[0]).len()).sum()
    }

    /// [`State::wire_len`] with `taut_len` of a path not inserted yet: `wire` (its terminals,
    /// net, width, clearance) through the gaps `path` of the triangles `tris` (one more), each
    /// gap's span as it is now ([`State::gap_spans`]). O(path · edge load²).
    pub fn path_len_taut(&self, wire: &Wire, path: &[(EdgeId, usize)], tris: &[TriId]) -> f64 {
        let pr = self.proj;
        let me = Item { net: wire.net, w: wire.w, s: wire.s };
        let (pa, pb) = (pr.pos[wire.a], pr.pos[wire.b]);
        let mut ports: Vec<(Vec2, Vec2)> = Vec::with_capacity(path.len() + 2);
        ports.push((pa, pa));
        let mut sp = Vec::new();
        for (k, &(e, g)) in path.iter().enumerate() {
            let (t1, t2) = (tris[k], tris[k + 1]);
            let both = pr.allowed(t1, wire.net).map_or(0, |m| m.0) & pr.allowed(t2, wire.net).map_or(0, |m| m.0);
            self.gap_spans(e, me, both, None, &mut sp);
            let [vp, vq] = pr.edges[e];
            let (a, b) = (pr.pos[vp], pr.pos[vq]);
            let len = (b - a).len().max(1.0);
            let (lo, hi) = sp[g.min(sp.len() - 1)];
            let (x, y) = (a + (b - a) * (lo / len), a + (b - a) * (hi / len));
            let t = pr.tris[t1];
            let c = (pr.pos[t[0]] + pr.pos[t[1]] + pr.pos[t[2]]) * (1.0 / 3.0);
            let left_is_q = (b - a).cross(c - a) > 0.0;
            ports.push(if left_is_q { (y, x) } else { (x, y) });
        }
        ports.push((pb, pb));
        super::pack::funnel(&ports).windows(2).map(|x| (x[1].0 - x[0].0).len()).sum()
    }

    /// The corners of the taut path of wire `w` ([`State::wire_len`] with `taut_len`): terminal,
    /// the span ends and turn corners it bends at, terminal.
    pub fn taut_points(&self, w: WireId) -> Vec<Vec2> {
        let pr = self.proj;
        let wi = &self.wires[w];
        let n = wi.passes.len();
        let mut ports: Vec<(Vec2, Vec2)> = Vec::with_capacity(n + wi.turns.len() + 2);
        let (pa, pb) = (pr.pos[wi.a], pr.pos[wi.b]);
        ports.push((pa, pa));
        let mut ti = 0;
        for k in 0..=n {
            while ti < wi.turns.len() && wi.turns[ti].0 == k {
                let v = pr.pos[wi.turns[ti].1];
                ports.push((v, v));
                ti += 1;
            }
            if k == n {
                break;
            }
            let p = wi.passes[k];
            let e = self.passes[p].edge;
            let (t1, t2) = (wi.tris[k], wi.tris[k + 1]);
            let both = pr.allowed(t1, wi.net).map_or(0, |m| m.0) & pr.allowed(t2, wi.net).map_or(0, |m| m.0);
            let (x, y) = self.pass_span(p, both);
            // Left and right as seen coming from the triangle of traversal `k`.
            let [vp, vq] = pr.edges[e];
            let t = pr.tris[t1];
            let c = (pr.pos[t[0]] + pr.pos[t[1]] + pr.pos[t[2]]) * (1.0 / 3.0);
            let left_is_q = (pr.pos[vq] - pr.pos[vp]).cross(c - pr.pos[vp]) > 0.0;
            ports.push(if left_is_q { (y, x) } else { (x, y) });
        }
        ports.push((pb, pb));
        super::pack::funnel(&ports).into_iter().map(|c| c.0).collect()
    }

    fn order_index(&self, p: PassId) -> usize {
        let e = self.passes[p].edge;
        self.edge_order[e].iter().position(|&x| x == p).expect("pass in its edge order")
    }

    /// Event query: space (nm, one layer) the passes through `e` that are restricted to the layers
    /// of `mask` need (the Leiserson-Maley term of the layer subset `mask`): a pass counts when
    /// the layers both its triangles allow its wire are all in `mask`.
    pub fn demand(&self, e: EdgeId, mask: LayerMask) -> f64 {
        let items: Vec<Item> = self.edge_order[e]
            .iter()
            .filter(|&&p| {
                let pa = &self.passes[p];
                let w = &self.wires[pa.wire];
                let Some(k) = w.passes.iter().position(|&x| x == p) else { return false };
                let (t1, t2) = (w.tris[k], w.tris[k + 1]);
                match (self.proj.allowed(t1, w.net), self.proj.allowed(t2, w.net)) {
                    (Some(a), Some(b)) => (a.0 & b.0) & !mask.0 == 0,
                    _ => false,
                }
            })
            .map(|&p| self.item(p))
            .collect();
        self.proj.edge_demand(e, &items)
    }

    /// Event query: room of edge `e` on layer `l` (see [`Projection::room_of`]).
    pub fn room(&self, e: EdgeId, l: usize) -> f64 {
        self.proj.room_of(e, l)
    }

    /// Event query: the passes that run between obstacles `o1` and `o2`, over all cut edges that
    /// join them, each edge in its order from `p` to `q`.
    pub fn passes_between(&self, o1: ObsId, o2: ObsId) -> Vec<PassId> {
        self.proj.edges_between(o1, o2).into_iter().flat_map(|e| self.edge_order[e].iter().copied()).collect()
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

    /// The chords of traversal `k`: entry -> corners the wire turns at -> exit.
    pub fn chords(&self, w: WireId, k: usize) -> Vec<(f64, f64)> {
        let wi = &self.wires[w];
        let (a, b) = self.chord(w, k);
        if wi.turns.is_empty() {
            return vec![(a, b)];
        }
        let t = wi.tris[k];
        let mut pts = vec![a];
        pts.extend(wi.turns.iter().filter(|x| x.0 == k).map(|x| self.proj.corner_pos(t, x.1)));
        pts.push(b);
        pts.windows(2).map(|p| (p[0], p[1])).collect()
    }

    /// Whether wire `w` can turn at corner `v` of its traversal `k` (a junction there, D5
    /// branching): `v` is a free or node vertex of that triangle, `w` has no turn in it yet, and
    /// the two new chords (entry -> `v` -> exit) cross no other traversal of the triangle more often
    /// than the old chord did (no new crossing). O(traversals of the triangle).
    pub fn junction_ok(&self, w: WireId, k: usize, v: VertId) -> bool {
        let wi = &self.wires[w];
        if k >= wi.tris.len() || wi.turns.iter().any(|x| x.0 == k) || v == wi.a || v == wi.b {
            return false;
        }
        let t = wi.tris[k];
        if !self.proj.tris[t].contains(&v) || !matches!(self.proj.vkind[v], VertKind::Free | VertKind::Node) {
            return false;
        }
        let (e1, e2) = self.chord(w, k);
        let c = self.proj.corner_pos(t, v);
        if (c - e1).abs() < 1e-9 || (c - e2).abs() < 1e-9 {
            return false;
        }
        for &(o, ok) in &self.tri_trav[t] {
            if o == w {
                continue;
            }
            for (b1, b2) in self.chords(o, ok) {
                let old = usize::from(interleave(e1, e2, b1, b2));
                let new = usize::from(interleave(e1, c, b1, b2)) + usize::from(interleave(c, e2, b1, b2));
                if new > old {
                    return false;
                }
            }
        }
        true
    }

    /// Make `w` turn at `v` in its traversal `k` ([`State::junction_ok`] must hold).
    pub fn add_turn(&mut self, w: WireId, k: usize, v: VertId) {
        let turns = &mut self.wires[w].turns;
        turns.push((k, v));
        turns.sort_by_key(|x| x.0);
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

    /// Crossings of `w` with other wires, in `w`'s traversal order (one per interleaving pair of
    /// chords).
    pub fn crossings(&self, w: WireId) -> Vec<Crossing> {
        let wi = &self.wires[w];
        let mut out = Vec::new();
        for (k, &t) in wi.tris.iter().enumerate() {
            let mine = self.chords(w, k);
            for &(o, ok) in &self.tri_trav[t] {
                if o == w {
                    continue;
                }
                let theirs = self.chords(o, ok);
                for (i, &(a1, a2)) in mine.iter().enumerate() {
                    for (j, &(b1, b2)) in theirs.iter().enumerate() {
                        if interleave(a1, a2, b1, b2) {
                            out.push(Crossing { k, other: o, other_k: ok, tri: t, sub: i, other_sub: j });
                        }
                    }
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
            let ca = self.chords(tr[i].0, tr[i].1);
            for j in i + 1..tr.len() {
                if self.wires[tr[i].0].net == self.wires[tr[j].0].net {
                    continue;
                }
                let cb = self.chords(tr[j].0, tr[j].1);
                n += ca.iter().flat_map(|&(a1, a2)| cb.iter().map(move |&(b1, b2)| interleave(a1, a2, b1, b2))).filter(|&x| x).count();
            }
        }
        n
    }

    /// Total different-net crossings.
    pub fn total_crossings(&self) -> usize {
        (0..self.tri_trav.len()).map(|t| self.tri_crossings(t)).sum()
    }

    /// Room of the roomiest single layer of edge `e` among those both triangles allow a wire of
    /// `net` (the `squeeze` factor asks how full one layer would be); `None` when a triangle is a
    /// wall or the edge is no cut.
    pub fn room_best_layer(&self, e: EdgeId, t1: TriId, t2: TriId, net: i32) -> Option<f64> {
        let (m1, m2) = (self.proj.allowed(t1, net)?, self.proj.allowed(t2, net)?);
        let m = m1.0 & m2.0;
        let r = (0..self.proj.n_layers).filter(|&l| (m >> l) & 1 == 1).map(|l| self.proj.room_for(e, l, net)).fold(f64::NEG_INFINITY, f64::max);
        (r.is_finite()).then_some(r)
    }

    /// By how much (nm) a single pass needing `alone` through edge `e` for a wire of `net` allowed
    /// on the layers of `mask` violates the copper-to-edge rule that the project set to ignore:
    /// it fits on some layer without the rule and on none with it. `None` when no rule is ignored
    /// there or the pass is fine. O(layers).
    pub fn ignored_violation(&self, e: EdgeId, mask: u64, net: i32, alone: f64) -> Option<f64> {
        let pr = self.proj;
        let extra = pr.edge_clear_ignored * pr.edges[e].iter().filter(|&&v| matches!(pr.vkind[v], VertKind::Edge)).count() as f64;
        if extra <= 0.0 {
            return None;
        }
        let rooms: Vec<f64> = (0..pr.n_layers).filter(|&l| (mask >> l) & 1 == 1).map(|l| pr.room_for(e, l, net)).collect();
        if rooms.iter().any(|&r| alone + extra <= r) || !rooms.iter().any(|&r| alone <= r) {
            return None;
        }
        Some(alone + extra - rooms.iter().cloned().filter(|&r| alone <= r).fold(0.0, f64::max))
    }

    /// The pads of other nets that have no wire yet and for which triangle `t` is the last free
    /// entry sector. The sectors of a pad are the triangles of its ring (triangles with a corner
    /// on the pad's outline, outside the pad); a sector is free when it is no wall and no stretch
    /// of another net than the pad's runs through it. A wire of `net` taking `t` leaves such a
    /// pad no way out without crossing it. O(corners x ring).
    pub fn blocked_pads(&self, t: TriId, net: i32) -> Vec<ObsId> {
        let pr = self.proj;
        let mut out = Vec::new();
        for &v in &pr.tris[t] {
            let VertKind::Outline(o) = pr.vkind[v] else { continue };
            let Some(pn) = pr.obstacles[o].net else { continue };
            let Some(cv) = self.centre[o] else { continue };
            if pn == net || self.ends[cv] > 0 || out.contains(&o) || pr.tri_obs[t].contains(&o) {
                continue;
            }
            let free = |x: TriId| pr.allowed(x, pn).is_some() && self.tri_trav[x].iter().all(|&(w, _)| self.wires[w].net == pn);
            if free(t) && self.ring[o].iter().filter(|&&x| free(x)).count() == 1 {
                out.push(o);
            }
        }
        out
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
            self.pass_w.push(wire.w);
            let at = (g + shift).min(self.edge_order[e].len());
            self.edge_order[e].insert(at, pid);
            wire.passes.push(pid);
        }
        for (k, &t) in tris.iter().enumerate() {
            self.tri_trav[t].push((id, k));
        }
        wire.tris = tris;
        wire.alive = true;
        self.ends[wire.a] += 1;
        self.ends[wire.b] += 1;
        self.wires.push(wire);
        self.refit_around(id);
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
                self.pass_w.push(wire.w);
                at[e].push((u, pid));
                    wire.passes.push(pid);
            }
            for (k, &t) in tris.iter().enumerate() {
                self.tri_trav[t].push((id, k));
            }
            wire.tris = tris;
            wire.alive = true;
            self.ends[wire.a] += 1;
            self.ends[wire.b] += 1;
            self.wires.push(wire);
        }
        for (e, v) in at.iter_mut().enumerate() {
            v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            self.edge_order[e].extend(v.iter().map(|x| x.1));
        }
    }

    pub fn remove(&mut self, w: WireId) {
        self.remove_quiet(w);
        // The wires that shared an edge with it may widen again.
        if !self.floors.is_empty() {
            let mut ws: Vec<WireId> = self.wires[w].passes.iter().flat_map(|&p| self.edge_order[self.passes[p].edge].iter().map(|&q| self.passes[q].wire)).collect();
            ws.sort_unstable();
            ws.dedup();
            for x in ws {
                if self.wires[x].alive {
                    self.refit_wire(x);
                }
            }
        }
    }

    /// [`State::remove`] without refitting the widths of the wires around it.
    fn remove_quiet(&mut self, w: WireId) {
        for &p in &self.wires[w].passes {
            let e = self.passes[p].edge;
            self.edge_order[e].retain(|&x| x != p);
        }
        for &t in &self.wires[w].tris {
            self.tri_trav[t].retain(|&(x, _)| x != w);
        }
        if self.wires[w].alive {
            let (a, b) = (self.wires[w].a, self.wires[w].b);
            self.ends[a] -= 1;
            self.ends[b] -= 1;
        }
        self.wires[w].alive = false;
    }

    /// Take the alive wire `w` out of the edge orders and triangles but keep its record (pass ids,
    /// triangles): returns its path as `(edge, gap)` in the form [`State::insert`] takes, the gap
    /// counting only the other wires' passes. [`State::reattach`] puts it back (on the same or on
    /// changed gaps of the same edges). O(passes x edge load).
    pub fn detach(&mut self, w: WireId) -> Vec<(EdgeId, usize)> {
        let path: Vec<(EdgeId, usize)> = self.wires[w]
            .passes
            .iter()
            .map(|&p| {
                let e = self.passes[p].edge;
                let g = self.edge_order[e].iter().take_while(|&&x| x != p).filter(|&&x| self.passes[x].wire != w).count();
                (e, g)
            })
            .collect();
        self.remove(w);
        path
    }

    /// Put the detached wire `w` back along `path` (gaps as in [`State::insert`], one per pass of
    /// `w`, on its own edges): same id, same pass ids, same triangles. O(passes x edge load).
    pub fn reattach(&mut self, w: WireId, path: &[(EdgeId, usize)]) {
        assert!(!self.wires[w].alive, "reattach of a wire that is in place");
        let passes = self.wires[w].passes.clone();
        assert_eq!(passes.len(), path.len(), "one gap per pass");
        for (j, &(e, g)) in path.iter().enumerate() {
            debug_assert_eq!(self.passes[passes[j]].edge, e, "reattach on the wire's own edges");
            let shift = path[..j].iter().filter(|&&(e0, g0)| e0 == e && g0 <= g).count();
            let at = (g + shift).min(self.edge_order[e].len());
            self.edge_order[e].insert(at, passes[j]);
        }
        for k in 0..self.wires[w].tris.len() {
            let t = self.wires[w].tris[k];
            self.tri_trav[t].push((w, k));
        }
        let (a, b) = (self.wires[w].a, self.wires[w].b);
        self.ends[a] += 1;
        self.ends[b] += 1;
        self.wires[w].alive = true;
        self.refit_around(w);
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

/// Distance between the segments `a-b` and `c-d`.
fn seg_seg_dist(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> f64 {
    let pt = |p: Vec2, u: Vec2, v: Vec2| {
        let e = v - u;
        let l2 = e.len2();
        let t = if l2 > 0.0 { ((p - u).dot(e) / l2).clamp(0.0, 1.0) } else { 0.0 };
        (u + e * t - p).len()
    };
    if interleave_seg(a, b, c, d) {
        return 0.0;
    }
    pt(a, c, d).min(pt(b, c, d)).min(pt(c, a, b)).min(pt(d, a, b))
}

/// Whether the segments `a-b` and `c-d` cross properly.
fn interleave_seg(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let o = |p: Vec2, q: Vec2, r: Vec2| (q - p).cross(r - p);
    let (d1, d2, d3, d4) = (o(c, d, a), o(c, d, b), o(a, b, c), o(a, b, d));
    (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0) && d1 != 0.0 && d2 != 0.0 && d3 != 0.0 && d4 != 0.0
}
