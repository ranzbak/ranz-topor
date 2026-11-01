//! Layered slot A* per connection with a Lazy-Theta*-style length estimate, via moves at via
//! sites and pour goals, and the slot-free ghost search (M2 / T2.3a, M3 / T3.1; design notes
//! `docs/progress/notes/m2-search-design.md` §1-3 and `m3-layers-vias-design.md` §2).
//!
//! State `(layer, edge, slot, dir)`: the new wire crosses `edge` of layer `layer` at insertion
//! position `slot` and enters the face `edges[edge].faces[dir]`. Transitions follow Rule T (no two
//! strands cross inside a face; port order is the cyclic order of the face boundary). Cost is the
//! string-pulled length (apex shortcut, <= `LOS_PORTAL_CAP` portals per line-of-sight) scaled by
//! history / present congestion; `h` is the Euclidean distance to the nearest goal.
//!
//! *Via moves (M3).* A state entering a face that has an unowned via-site vertex may stand on the
//! site (`AtSite`) at the price `via_cost * (1 + omega * site_history)`; from there the search
//! leaves on every other layer through the faces around the site's vertex of that layer (the M2
//! start rules). A path is cut into [`Leg`]s at the sites. A pour goal is reached by the via itself
//! (P1): the path ends at a site inside the pour and `sites.len() == legs.len()`.
//!
//! Complexity: `O(V' log V')`, `V' = sum_L 2 (E_L + X_L) + 2 S` states (times `max_vias + 1` for
//! the via counter), each expansion `O(n_max + LOS_PORTAL_CAP^2 + (C + 1) local)`; expansions are
//! capped at `8 sum_L (E_L + X_L) + 4 S + 64`. Only lookups touch hash containers; ties are
//! broken by the heap key.
//!
//! *Topological ghost (M3 / F2).* The ghost of a blocked connection is a Dijkstra over the states
//! `(layer, edge, dir, vias)` plus `AtSite` nodes on the same adjacency, with no geometry at all
//! (no line of sight, no clearance candidates, no slots): key `(soft, length)` lexicographic,
//! `soft` = edges that are not passable or do not fit, owned / blocked sites; `length` = the sum of
//! hops between edge points (the edge point nearest the straight line to the goal); only a face
//! outside the outline is hard. `est_len` of the result is the
//! funnel length of the found crossing sequence (`O(k)`). `O(sum_L E_L (max_vias + 1))` per run.
//!
//! The single-layer entry points of M2 (`planar::search` & co) run the same core on one
//! `RouteGraph` without sites; [`SlotPath::single_layer`] adapts their [`planar::PlanarPath`].

use crate::decompose::Connection;
use crate::geom::{point_seg, seg_arc, seg_seg, Aabb, Seg, Vec2};
use crate::graph::{Port, RouteGraph};
use crate::multigraph::MultiGraph;
use crate::obstacle::{Core, LayerObstacles, ObsKind, ObstacleSet};
use crate::vias::{via_sizes, ViaSize};
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering as AtOrd};
use std::time::Instant;

/// Sub-chords of a clearance query are merged up to this many query radii long.
const MERGE_LEN_FACTOR: f64 = 4.0;
/// Maximum portals one line-of-sight test walks (note §2, `C`).
pub const LOS_PORTAL_CAP: usize = 32;
/// A bend of `los` "approaches" the target when it moves the apex at least this much (nm) closer
/// than every apex before it (A2).
const LOS_TOL: f64 = 1.0;
/// A line leaving a terminal is inadmissible after this many consecutive bends of `los` that do
/// not approach the target (A2 / M4-D2). 2 = one non-approaching bend is tolerated (a neighbour's
/// corner beside the exit); the bound only decides how early a pinched terminal exit is given up
/// (U7.4 at 0.2 mm: after two bends instead of a 5-9 mm spiral).
const LOS_STALL_CAP: usize = 2;
/// Present-congestion factor of a non-fitting edge (note §2).
pub const PRESENT_OVERUSE: f64 = 4.0;

/// History weight `omega` of a priority tier (tier 1 feels congestion least).
pub fn tier_weight(tier: u8) -> f64 {
    match tier {
        0 | 1 => 0.5,
        2 => 0.75,
        3 => 1.0,
        _ => 1.25,
    }
}

/// Via-cost multiplier of a priority tier (P8): tiers 0/1 pay double, tier 4 pays 0.75x. The
/// caller multiplies `Options.via_cost_nm` by it to get [`SearchParams::via_cost`].
pub fn via_weight(tier: u8) -> f64 {
    match tier {
        0 | 1 => 2.0,
        2 | 3 => 1.0,
        _ => 0.75,
    }
}

/// Expansion cap `8 sum_L (E_L + X_L) + 4 S + 64` of the layered slot search. O(sum_L E_L).
pub fn default_expansion_cap(mg: &MultiGraph) -> usize {
    let gs: Vec<&RouteGraph> = mg.layers.iter().map(|c| &c.g).collect();
    cap_of(&gs, mg.sites.len(), false)
}

/// `8 sum (E + X) + 4 S + 64`, or `8 sum E + 4 S + 64` for the ghost. O(sum E).
fn cap_of(gs: &[&RouteGraph], sites: usize, ghost: bool) -> usize {
    let mut n = 0usize;
    for g in gs {
        let x: usize = if ghost {
            0
        } else {
            (0..g.edges.len()).map(|e| g.slot_count(e)).sum()
        };
        n += 8 * (g.edges.len() + x);
    }
    n + 4 * sites + 64
}

/// One run of a path on a single layer, from the vertex `start` to the vertex `goal` of that
/// layer (a terminal, or the vertex of a via site).
#[derive(Clone, Debug)]
pub struct Leg {
    pub layer_idx: usize,
    pub start: usize,
    pub goal: usize,
    /// `(edge, slot)` in travel order, slots as seen by the search (the graph before the insert).
    pub crossings: Vec<(usize, usize)>,
    /// Start face first; `faces.len() == crossings.len() + 1` (empty for a direct leg).
    pub faces: Vec<usize>,
    pub est_len: f64,
}

#[derive(Clone, Debug)]
pub struct SlotPath {
    pub conn: usize,
    pub legs: Vec<Leg>,
    /// `(site, size)` per via; `legs.len() - 1` of them, or `legs.len()` for a pour path (the
    /// last via lands in the pour, P1).
    pub sites: Vec<(usize, ViaSize)>,
    /// Sum of the legs' `est_len`.
    pub est_len: f64,
    /// Search cost at the goal, including the via costs.
    pub cost: f64,
    pub expansions: usize,
    /// Sum of the clearance-query candidates (vertices + walls) examined over the whole search.
    pub candidates: usize,
}

impl SlotPath {
    /// Number of vias of the path. O(1).
    pub fn vias(&self) -> usize {
        self.sites.len()
    }

    /// Crossings of the legs on layer `layer_idx`, in travel order. O(crossings).
    pub fn crossings_on(&self, layer_idx: usize) -> Vec<(usize, usize)> {
        self.legs
            .iter()
            .filter(|l| l.layer_idx == layer_idx)
            .flat_map(|l| l.crossings.iter().copied())
            .collect()
    }

    /// Adapter of a single-layer (layer 0) planar path from `start` to `goal`: one leg, no
    /// site, `cost = est_len`. O(crossings).
    pub fn single_layer(p: &planar::PlanarPath, start: usize, goal: usize) -> SlotPath {
        SlotPath {
            conn: p.conn,
            legs: vec![Leg {
                layer_idx: 0,
                start,
                goal,
                crossings: p.crossings.clone(),
                faces: p.faces.clone(),
                est_len: p.est_len,
            }],
            sites: Vec::new(),
            est_len: p.est_len,
            cost: p.est_len,
            expansions: p.expansions,
            candidates: p.candidates,
        }
    }
}

#[derive(Clone, Debug)]
pub enum SearchOutcome {
    Found(SlotPath),
    Blocked {
        ghost: Option<SlotPath>,
        /// First soft-blocked edge of the ghost in travel order, `(layer_idx, edge)`.
        blocked_edge: Option<(usize, usize)>,
        /// First soft-blocked element is an owned / blocked site (P5).
        blocked_site: Option<usize>,
        /// Crossing before the blocked element, `(leg, index in leg.crossings)` (P5).
        last_reached: Option<(usize, usize)>,
        /// Blocked before any search by a static fact of the board (F3): a terminal has no
        /// inside face with a statically passable edge or a via site, so it can never be left /
        /// entered whatever the other connections do. The negotiator marks such a connection
        /// `dead`.
        static_reason: Option<String>,
    },
    Timeout,
}

/// F3: vertex `v` of `g` can be left / entered by `net` as far as the static graph goes: some
/// inside face around it has an edge that is statically passable for the net, or a via-site
/// vertex (a via move leaves through the other layer). O(deg). Every first move of the slot
/// search (`leave_vertex`, `push_goal`) needs one of these, so a terminal without them is
/// blocked in every iteration, whatever the other connections do; the converse does not hold
/// (geometry may still refuse every move: dcsgone's U7 at 0.5 mm pitch).
pub fn terminal_open(g: &RouteGraph, v: usize, net: i32) -> bool {
    g.faces_of_vertex(v).iter().any(|&f| {
        let face = &g.faces[f];
        face.inside
            && (face.e.iter().any(|&e| g.passable_static(e, net))
                || face.v.iter().any(|&vv| vv != v && g.verts[vv].site.is_some()))
    })
}

/// F3 pre-check of a search: `Some(reason)` when the start cannot be left or the goal (terminal
/// goals only; a pour goal is a site) cannot be entered on any layer. O(Σ deg).
fn static_block(
    env: &Env,
    conn: &Connection,
    start: &[(usize, usize)],
    goal: &Goal,
) -> Option<String> {
    let open = |l: usize, v: usize| terminal_open(env.layers[l].0, v, conn.net);
    let start_ok = start.iter().any(|&(l, v)| open(l, v));
    let goal_ok = goal.vertices.is_empty() || goal.vertices.iter().any(|&(l, v)| open(l, v));
    let what = match (start_ok, goal_ok) {
        (true, true) => return None,
        (false, true) => "left",
        (true, false) => "entered",
        (false, false) => "left/entered",
    };
    Some(format!(
        "blocked: terminal cannot be {what} (width {:.3} mm, clearance {:.3} mm)",
        conn.width as f64 * 1e-6,
        conn.clearance as f64 * 1e-6
    ))
}

/// Where the connection ends: terminal vertices `(layer_idx, vertex)` on every layer of the
/// target and, for a pour target, the sites inside the pour (sorted).
#[derive(Clone, Debug, Default)]
pub struct Goal {
    pub vertices: Vec<(usize, usize)>,
    pub pour_sites: Vec<usize>,
    /// Informational (P1).
    pub pour_layer: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
pub struct SearchParams<'a> {
    /// Per-edge history cost `[layer_idx][edge]` (missing entries are 0).
    pub history: &'a [Vec<f64>],
    /// Per-site history cost (missing entries are 0).
    pub site_history: &'a [f64],
    /// `omega`: history / present weight of the connection's tier.
    pub weight: f64,
    pub present_overuse: f64,
    pub expansion_cap: usize,
    pub deadline: Instant,
    /// Build margin ε of the admissibility and geometry tests; `None` = the graphs'
    /// `rules.eps`. The zero-slack retry (R6) searches one connection at `Some(0.0)`.
    pub eps: Option<f64>,
    /// Final cost of one via, nm (P8: the caller applied `via_weight(tier)`).
    pub via_cost: f64,
    pub max_vias: u32,
    /// D-1 (`Options.strict_unfit_edges`): the layered search rejects an edge that is unfit even
    /// when empty (`!fits(e)`) in `push_state`; the ghost keeps pricing it as soft.
    pub strict_unfit_edges: bool,
}

// ---------------------------------------------------------------------------------------------
// Face cycle: ports in cyclic order and Rule T
// ---------------------------------------------------------------------------------------------

/// Cyclic port order of a face boundary; positions are doubled so a virtual port can sit in a gap.
struct Cyc {
    f: usize,
    vs: [usize; 3],
    es: [usize; 3],
    fwd: [bool; 3],
    n: [usize; 3],
    vpos: [i64; 3],
    epos: [i64; 3],
    total: i64,
}

impl Cyc {
    fn new(g: &RouteGraph, f: usize) -> Cyc {
        let fc = &g.faces[f];
        let mut c = Cyc {
            f,
            vs: fc.v,
            es: [0; 3],
            fwd: [false; 3],
            n: [0; 3],
            vpos: [0; 3],
            epos: [0; 3],
            total: 0,
        };
        let mut idx = 0i64;
        for j in 0..3 {
            c.vpos[j] = 2 * idx;
            idx += 1;
            let e = fc.e[(j + 2) % 3];
            c.es[j] = e;
            c.fwd[j] = g.edges[e].a == fc.v[j];
            c.n[j] = g.slot_count(e);
            c.epos[j] = 2 * idx;
            idx += c.n[j] as i64;
        }
        c.total = 2 * idx;
        c
    }

    fn vertex_pos(&self, v: usize) -> Option<i64> {
        (0..3).find(|&j| self.vs[j] == v).map(|j| self.vpos[j])
    }

    fn entry_pos(&self, j: usize, i: usize) -> i64 {
        let w = if self.fwd[j] { i } else { self.c_n(j) - 1 - i };
        self.epos[j] + 2 * w as i64
    }

    fn c_n(&self, j: usize) -> usize {
        self.n[j]
    }

    fn pos(&self, p: Port) -> Option<i64> {
        match p {
            Port::Vertex(v) => self.vertex_pos(v),
            Port::Cross(e, i) => (0..3)
                .find(|&j| self.es[j] == e)
                .map(|j| self.entry_pos(j, i)),
            Port::Detached => None,
        }
    }

    /// Position of the gap before index `slot` (insertion position) of edge `e`.
    fn gap_pos(&self, e: usize, slot: usize) -> Option<i64> {
        let j = (0..3).find(|&j| self.es[j] == e)?;
        let w = if self.fwd[j] { slot } else { self.n[j] - slot };
        Some(self.epos[j] + 2 * w as i64 - 1)
    }

    /// `(entry position, partner position)` of every entry whose strand lies in this face.
    fn strands(&self, g: &RouteGraph) -> Vec<(i64, i64)> {
        let mut v = Vec::new();
        for j in 0..3 {
            for i in 0..self.n[j] {
                let partner = g.strand(self.es[j], i, self.f);
                if let Some(b) = self.pos(partner) {
                    v.push((self.entry_pos(j, i), b));
                }
            }
        }
        v
    }

    /// Slot (insertion index) for a chord from position `p0` to a new port on `e_out`, or `None`
    /// when some strand blocks it. `near_v` is the vertex whose side of `e_out` the new port
    /// prefers; strands ending at the chord's own start vertex nest nearest `near_v`.
    fn cross_slot(
        &self,
        strands: &[(i64, i64)],
        p0: i64,
        e_out: usize,
        near_v: usize,
    ) -> Option<usize> {
        let jb = (0..3).find(|&j| self.es[j] == e_out)?;
        let nb = self.n[jb];
        let rel = |x: i64| (x - p0).rem_euclid(self.total);
        let b0 = rel(self.epos[jb]);
        let b1 = b0 + 2 * (nb as i64 - 1);
        let in_block = |r: i64| nb > 0 && r >= b0 && r <= b1;
        let near_start = near_v == self.vs[jb];
        let (mut lo, mut hi) = (0usize, nb);
        for &(a, b) in strands {
            let (ra, rb) = (rel(a), rel(b));
            let (ia, ib) = (in_block(ra), in_block(rb));
            if ia && ib {
                continue;
            }
            if !ia && !ib {
                if ra != 0 && rb != 0 && (ra < b0) != (rb < b0) {
                    return None;
                }
                continue;
            }
            let (rj, rq) = if ia { (ra, rb) } else { (rb, ra) };
            let w = ((rj - b0) / 2) as usize;
            if rq == 0 {
                // strand from the chord's own start vertex: nests nearest `near_v`
                if near_start {
                    lo = lo.max(w + 1);
                } else {
                    hi = hi.min(w);
                }
            } else if rq < b0 {
                lo = lo.max(w + 1);
            } else {
                hi = hi.min(w);
            }
        }
        if lo > hi {
            return None;
        }
        let gap = if near_start { lo } else { hi };
        Some(if self.fwd[jb] { gap } else { nb - gap })
    }

    /// Like [`Cyc::strands`], with the owning connection of every strand.
    fn strands_conn(&self, g: &RouteGraph) -> Vec<(i64, i64, usize)> {
        let mut v = Vec::new();
        for j in 0..3 {
            for i in 0..self.n[j] {
                let partner = g.strand(self.es[j], i, self.f);
                if let Some(b) = self.pos(partner) {
                    v.push((self.entry_pos(j, i), b, g.edges[self.es[j]].cross[i].conn));
                }
            }
        }
        v
    }

    /// Connections whose strands make [`Cyc::cross_slot`] fail for a chord from `p0` to a new
    /// port on `e_out` (empty when the chord fits).
    fn edge_blockers(
        &self,
        strands: &[(i64, i64, usize)],
        p0: i64,
        e_out: usize,
        near_v: usize,
    ) -> Vec<usize> {
        let Some(jb) = (0..3).find(|&j| self.es[j] == e_out) else {
            return Vec::new();
        };
        let nb = self.n[jb];
        let rel = |x: i64| (x - p0).rem_euclid(self.total);
        let b0 = rel(self.epos[jb]);
        let b1 = b0 + 2 * (nb as i64 - 1);
        let in_block = |r: i64| nb > 0 && r >= b0 && r <= b1;
        let near_start = near_v == self.vs[jb];
        let mut out: Vec<usize> = Vec::new();
        // (bound, conn): lower bounds `w + 1`, upper bounds `w` on the insertion gap
        let mut los: Vec<(usize, usize)> = Vec::new();
        let mut his: Vec<(usize, usize)> = Vec::new();
        for &(a, b, conn) in strands {
            let (ra, rb) = (rel(a), rel(b));
            let (ia, ib) = (in_block(ra), in_block(rb));
            if ia && ib {
                continue;
            }
            if !ia && !ib {
                if ra != 0 && rb != 0 && (ra < b0) != (rb < b0) {
                    out.push(conn);
                }
                continue;
            }
            let (rj, rq) = if ia { (ra, rb) } else { (rb, ra) };
            let w = ((rj - b0) / 2) as usize;
            if rq == 0 {
                if near_start {
                    los.push((w + 1, conn));
                } else {
                    his.push((w, conn));
                }
            } else if rq < b0 {
                los.push((w + 1, conn));
            } else {
                his.push((w, conn));
            }
        }
        let lo = los.iter().map(|x| x.0).max().unwrap_or(0);
        let hi = his.iter().map(|x| x.0).min().unwrap_or(nb);
        if lo > hi {
            out.extend(los.iter().filter(|x| x.0 == lo).map(|x| x.1));
            out.extend(his.iter().filter(|x| x.0 == hi).map(|x| x.1));
        }
        out
    }

    /// Connections whose strands make [`Cyc::goal_ok`] fail.
    fn vertex_blockers(&self, strands: &[(i64, i64, usize)], p0: i64, q: usize) -> Vec<usize> {
        let Some(qp) = self.vertex_pos(q) else {
            return Vec::new();
        };
        let rel = |x: i64| (x - p0).rem_euclid(self.total);
        let rq = rel(qp);
        let mut out = Vec::new();
        for &(a, b, conn) in strands {
            let (ra, rb) = (rel(a), rel(b));
            if ra == 0 || rb == 0 || ra == rq || rb == rq {
                continue;
            }
            if (ra < rq) != (rb < rq) {
                out.push(conn);
            }
        }
        out
    }

    /// May a chord from `p0` end at vertex `q` of this face without crossing a strand?
    fn goal_ok(&self, strands: &[(i64, i64)], p0: i64, q: usize) -> bool {
        let Some(qp) = self.vertex_pos(q) else {
            return false;
        };
        let rel = |x: i64| (x - p0).rem_euclid(self.total);
        let rq = rel(qp);
        for &(a, b) in strands {
            let (ra, rb) = (rel(a), rel(b));
            if ra == 0 || rb == 0 || ra == rq || rb == rq {
                continue;
            }
            if (ra < rq) != (rb < rq) {
                return false;
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------------------------------

fn dist(a: Vec2, b: Vec2) -> f64 {
    (a - b).len()
}

fn closest_on_seg(a: Vec2, b: Vec2, p: Vec2) -> Vec2 {
    let d = b - a;
    let l2 = d.len2();
    if l2 == 0.0 {
        return a;
    }
    a + d * ((p - a).dot(d) / l2).clamp(0.0, 1.0)
}

/// Point of segment `s0 s1` nearest to segment `a b`.
fn seg_closest(s0: Vec2, s1: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let r = s1 - s0;
    let q = b - a;
    let den = r.cross(q);
    if den.abs() > 1e-12 {
        let t = (a - s0).cross(q) / den;
        let u = (a - s0).cross(r) / den;
        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
            return s0 + r * t;
        }
    }
    let ab = Seg { a, b };
    let cands = [s0, s1, closest_on_seg(s0, s1, a), closest_on_seg(s0, s1, b)];
    let mut best = cands[0];
    let mut bd = point_seg(best, &ab);
    for &c in &cands[1..] {
        let d = point_seg(c, &ab);
        if d < bd {
            bd = d;
            best = c;
        }
    }
    best
}

/// Chord `a -> t` passes strictly through the interior of portal `pa pb`.
fn crosses(a: Vec2, t: Vec2, pa: Vec2, pb: Vec2) -> bool {
    let r = t - a;
    let d1 = r.cross(pa - a);
    let d2 = r.cross(pb - a);
    if d1 * d2 >= 0.0 {
        return false;
    }
    let q = pb - pa;
    q.cross(a - pa) * q.cross(t - pa) <= 0.0
}

// ---------------------------------------------------------------------------------------------
// Search machinery
// ---------------------------------------------------------------------------------------------

const K_ROOT: u8 = 0;
const K_EDGE: u8 = 1;
const K_SITE: u8 = 2;
const K_GOAL: u8 = 3;

#[derive(Clone, Debug)]
struct Node {
    kind: u8,
    layer: usize,
    /// Root / goal: the vertex of `layer`; site: the site id.
    v: usize,
    edge: usize,
    slot: usize,
    dir: u8,
    parent: usize,
    /// Face the wire was in when it crossed `edge`.
    face_in: usize,
    apex: Vec2,
    apex_len: f64,
    /// Portals (including this node's edge) since the apex.
    since: usize,
    /// State point on `edge` (the site / goal point for those nodes).
    p: Vec2,
    g_len: f64,
    g_cost: f64,
    /// Vias used up to and including this node.
    vias: u32,
    /// Latest `K_SITE` ancestor (a site node: the previous one); `usize::MAX` if none.
    last_site: usize,
    size: Option<ViaSize>,
}

/// Closed-set / best-cost key `(layer, edge, slot, dir, vias_used)`; a site node uses
/// `edge = usize::MAX, slot = site, dir = 0`.
type Key = (usize, usize, usize, u8, u32);

#[derive(Clone, Copy, Debug)]
struct Item {
    f: f64,
    h: f64,
    layer: usize,
    edge: usize,
    slot: usize,
    dir: u8,
    id: usize,
}

impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.total_cmp(&self.f)
            .then(o.h.total_cmp(&self.h))
            .then(o.layer.cmp(&self.layer))
            .then(o.edge.cmp(&self.edge))
            .then(o.slot.cmp(&self.slot))
            .then(o.dir.cmp(&self.dir))
            .then(o.id.cmp(&self.id))
    }
}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl PartialEq for Item {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for Item {}

#[derive(Default)]
struct State {
    nodes: Vec<Node>,
    heap: BinaryHeap<Item>,
    best: HashMap<Key, f64>,
    closed: HashSet<Key>,
}

enum Run {
    Found(Box<SlotPath>),
    Blocked,
    Timeout,
}

/// Search inputs shared by the layered and the planar entry points.
#[derive(Clone, Copy)]
struct Common<'a> {
    site_history: &'a [f64],
    weight: f64,
    present_overuse: f64,
    expansion_cap: usize,
    deadline: Instant,
    eps: Option<f64>,
    via_cost: f64,
    max_vias: u32,
    strict_unfit: bool,
}

/// The graphs a search runs on: one `(graph, obstacles)` per layer index, plus the multigraph
/// (sites, ownership) when there are via sites.
struct Env<'a> {
    layers: Vec<(&'a RouteGraph, &'a LayerObstacles)>,
    mg: Option<&'a MultiGraph>,
}

impl<'a> Env<'a> {
    fn of(mg: &'a MultiGraph, obs: &'a ObstacleSet) -> Env<'a> {
        Env {
            layers: mg
                .layers
                .iter()
                .map(|c| (&c.g, obs.layer(c.layer)))
                .collect(),
            mg: Some(mg),
        }
    }

    fn sites(&self) -> usize {
        self.mg.map_or(0, |m| m.sites.len())
    }

    fn graphs(&self) -> Vec<&'a RouteGraph> {
        self.layers.iter().map(|l| l.0).collect()
    }
}

/// The layered A*: one [`Search`] helper per layer, the node arena, the via moves.
struct Layered<'a> {
    ls: Vec<Search<'a>>,
    mg: Option<&'a MultiGraph>,
    start: &'a [(usize, usize)],
    goal: &'a Goal,
    goal_set: HashSet<(usize, usize)>,
    /// Positions of the goal vertices.
    targets: Vec<Vec2>,
    /// Bounding box of the pour sites.
    pour_bb: Option<Aabb>,
    sizes: Vec<ViaSize>,
    site_history: &'a [f64],
    via_cost: f64,
    max_vias: u32,
    omega: f64,
    net: i32,
    conn_id: usize,
    ghost: bool,
    /// Expansions so far of the running `run` (statistics).
    exps: Cell<usize>,
}

fn other_dir(g: &RouteGraph, e: usize, f: usize) -> u8 {
    u8::from(g.edges[e].faces[0] == Some(f))
}

impl<'a> Layered<'a> {
    fn new(
        env: &Env<'a>,
        hists: &[&'a [f64]],
        conn: &'a Connection,
        start: &'a [(usize, usize)],
        goal: &'a Goal,
        p: &Common<'a>,
        ghost: bool,
    ) -> Self {
        let ls: Vec<Search<'a>> = env
            .layers
            .iter()
            .enumerate()
            .map(|(i, &(g, obs))| {
                let mut s = Search::new(
                    g,
                    obs,
                    conn,
                    hists.get(i).copied().unwrap_or(&[]),
                    p.weight,
                    p.present_overuse,
                    p.eps,
                    ghost,
                );
                s.strict_unfit = p.strict_unfit && !ghost;
                s
            })
            .collect();
        let targets = goal
            .vertices
            .iter()
            .map(|&(l, v)| env.layers[l].0.verts[v].at)
            .collect();
        let pour_bb = env.mg.and_then(|mg| {
            let mut it = goal.pour_sites.iter().map(|&s| mg.sites[s].at);
            let first = it.next()?;
            Some(it.fold(
                Aabb {
                    min: first,
                    max: first,
                },
                |b, q| Aabb {
                    min: Vec2 {
                        x: b.min.x.min(q.x),
                        y: b.min.y.min(q.y),
                    },
                    max: Vec2 {
                        x: b.max.x.max(q.x),
                        y: b.max.y.max(q.y),
                    },
                },
            ))
        });
        let sizes = env
            .mg
            .map(|mg| via_sizes(&mg.rules, conn.net))
            .unwrap_or_default();
        Layered {
            ls,
            mg: env.mg,
            start,
            goal,
            goal_set: goal.vertices.iter().copied().collect(),
            targets,
            pour_bb,
            sizes,
            site_history: p.site_history,
            via_cost: p.via_cost,
            max_vias: p.max_vias,
            omega: p.weight,
            net: conn.net,
            conn_id: conn.id,
            ghost,
            exps: Cell::new(0),
        }
    }

    /// Goal point nearest to `p` (`p` itself when there is no goal). O(|goal vertices|).
    fn nearest(&self, p: Vec2) -> Vec2 {
        let mut best = p;
        let mut bd = f64::INFINITY;
        for &t in &self.targets {
            let d = dist(p, t);
            if d < bd {
                bd = d;
                best = t;
            }
        }
        if let Some(bb) = self.pour_bb {
            let q = Vec2 {
                x: p.x.clamp(bb.min.x, bb.max.x),
                y: p.y.clamp(bb.min.y, bb.max.y),
            };
            if dist(p, q) < bd {
                best = q;
            }
        }
        best
    }

    /// Admissible heuristic: Euclidean distance to the nearest goal vertex or to the bounding
    /// box of the pour sites (ignores via cost). O(|goal vertices|).
    fn h(&self, p: Vec2) -> f64 {
        dist(p, self.nearest(p))
    }

    /// The site cannot take any via at the moment (owned, blocked, or too small for every size).
    fn site_soft_blocked(&self, s: usize) -> bool {
        let Some(mg) = self.mg else {
            return true;
        };
        mg.owner[s].is_some()
            || mg.blocked[s] > 0
            || !self
                .sizes
                .iter()
                .any(|z| mg.sites[s].max_d >= z.d && mg.sites[s].max_drill >= z.drill)
    }

    /// Largest via size for site `s`: the first of `via_sizes` with `site_free` (the ghost
    /// ignores occupancy). O(|sizes|).
    fn via_choice(&self, s: usize) -> Option<ViaSize> {
        let mg = self.mg?;
        let site = &mg.sites[s];
        if self.ghost {
            self.sizes
                .iter()
                .copied()
                .find(|z| site.max_d >= z.d && site.max_drill >= z.drill)
                .or_else(|| self.sizes.last().copied())
        } else {
            self.sizes
                .iter()
                .copied()
                .find(|&z| mg.site_free(s, self.net, z))
        }
    }

    fn inherit_site(st: &State, parent: usize) -> usize {
        if st.nodes[parent].kind == K_SITE {
            parent
        } else {
            st.nodes[parent].last_site
        }
    }

    fn node_key(&self, n: &Node) -> Key {
        match n.kind {
            K_SITE => (n.layer, usize::MAX, n.v, 0, n.vias),
            _ => (n.layer, n.edge, n.slot, n.dir, n.vias),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_state(
        &self,
        st: &mut State,
        parent: usize,
        layer: usize,
        e: usize,
        slot: usize,
        dir: u8,
        fin: usize,
    ) {
        let sr = &self.ls[layer];
        let vias = st.nodes[parent].vias;
        let key = (layer, e, slot, dir, vias);
        if st.closed.contains(&key) {
            return;
        }
        // A4 transit pinch: the face transit from the parent's crossing (same layer) around
        // their shared vertex must have room (refused, not priced: the geometry cannot realise
        // it at any cost; the margin-0 zero-slack search re-tests at its own margin)
        let pn = &st.nodes[parent];
        if pn.kind == K_EDGE && pn.layer == layer && !sr.transit_ok(fin, pn.edge, e) {
            return;
        }
        if sr.strict_unfit && !sr.fits(e) {
            return;
        }
        let s = &st.nodes[parent];
        let p = sr.state_point(e, s.apex, self.nearest(s.apex));
        let (apex, apex_len, kept, ok) = sr.los(&st.nodes, parent, p, Some(e));
        if !ok {
            return;
        }
        let g_len = apex_len + dist(apex, p);
        let delta = (g_len - s.g_len).max(0.0);
        let g_cost = s.g_cost + sr.step_cost(e, delta);
        if st.best.get(&key).is_some_and(|&b| b <= g_cost) {
            return;
        }
        st.best.insert(key, g_cost);
        let h = self.h(p);
        let id = st.nodes.len();
        let last_site = Self::inherit_site(st, parent);
        st.nodes.push(Node {
            kind: K_EDGE,
            layer,
            v: 0,
            edge: e,
            slot,
            dir,
            parent,
            face_in: fin,
            apex,
            apex_len,
            since: if sr.apex_past(e, dir, apex) {
                0
            } else {
                kept + 1
            },
            p,
            g_len,
            g_cost,
            vias,
            last_site,
            size: None,
        });
        st.heap.push(Item {
            f: g_cost + h,
            h,
            layer,
            edge: e,
            slot,
            dir,
            id,
        });
    }

    fn push_goal(&self, st: &mut State, parent: usize, layer: usize, v: usize) {
        let sr = &self.ls[layer];
        let goal = sr.g.verts[v].at;
        let (apex, apex_len, _, ok) = sr.los(&st.nodes, parent, goal, None);
        if !ok {
            return;
        }
        let sn = &st.nodes[parent];
        let g_len = apex_len + dist(apex, goal);
        let g_cost = sn.g_cost + (g_len - sn.g_len).max(0.0);
        let id = st.nodes.len();
        let (vias, last_site) = (sn.vias, Self::inherit_site(st, parent));
        st.nodes.push(Node {
            kind: K_GOAL,
            layer,
            v,
            edge: usize::MAX,
            slot: v,
            dir: 1,
            parent,
            face_in: 0,
            apex,
            apex_len,
            since: 0,
            p: goal,
            g_len,
            g_cost,
            vias,
            last_site,
            size: None,
        });
        st.heap.push(Item {
            f: g_cost,
            h: 0.0,
            layer,
            edge: usize::MAX,
            slot: v,
            dir: 1,
            id,
        });
    }

    /// Push the node "standing on the via of site `s`" reached from `parent` on `layer`.
    fn push_via(&self, st: &mut State, parent: usize, layer: usize, s: usize, size: ViaSize) {
        let Some(mg) = self.mg else {
            return;
        };
        let sr = &self.ls[layer];
        let at = mg.sites[s].at;
        let (apex, apex_len, _, ok) = sr.los(&st.nodes, parent, at, None);
        if !ok {
            return;
        }
        let sn = &st.nodes[parent];
        let g_len = apex_len + dist(apex, at);
        let hist = self.site_history.get(s).copied().unwrap_or(0.0);
        let cost = (g_len - sn.g_len).max(0.0) + self.via_cost * (1.0 + self.omega * hist);
        let g_cost = sn.g_cost + cost;
        let vias = sn.vias + 1;
        let key = (layer, usize::MAX, s, 0, vias);
        if st.closed.contains(&key) || st.best.get(&key).is_some_and(|&b| b <= g_cost) {
            return;
        }
        st.best.insert(key, g_cost);
        let last_site = Self::inherit_site(st, parent);
        let id = st.nodes.len();
        let h = self.h(at);
        st.nodes.push(Node {
            kind: K_SITE,
            layer,
            v: s,
            edge: usize::MAX,
            slot: s,
            dir: 0,
            parent,
            face_in: 0,
            apex: at,
            apex_len: g_len,
            since: 0,
            p: at,
            g_len,
            g_cost,
            vias,
            last_site,
            size: Some(size),
        });
        st.heap.push(Item {
            f: g_cost + h,
            h,
            layer,
            edge: usize::MAX,
            slot: s,
            dir: 0,
            id,
        });
    }

    /// Via move from `parent` (on `layer`, entering a face of `cyc`) to the site vertex `v` of
    /// that face: `vias < max_vias`, the site is not in the ancestor chain, some size is free,
    /// and the M2 goal rule `goal_ok` holds (note §2). O(vias + |sizes|) plus the `los` test.
    #[allow(clippy::too_many_arguments)]
    fn try_via(
        &self,
        st: &mut State,
        parent: usize,
        layer: usize,
        v: usize,
        cyc: &Cyc,
        strands: &[(i64, i64)],
        p0: i64,
    ) {
        let Some(s) = self.ls[layer].g.verts[v].site else {
            return;
        };
        if self.mg.is_none() || st.nodes[parent].vias >= self.max_vias {
            return;
        }
        let mut id = Self::inherit_site(st, parent);
        while id != usize::MAX {
            if st.nodes[id].v == s {
                return;
            }
            id = st.nodes[id].last_site;
        }
        let Some(size) = self.via_choice(s) else {
            return;
        };
        if !cyc.goal_ok(strands, p0, v) {
            return;
        }
        self.push_via(st, parent, layer, s, size);
    }

    /// The M2 start rules from vertex `v` of `layer`: leave every inside face around `v`
    /// through each of its edges. A root (`root`) also tries via moves to the site vertices of
    /// those faces; a via arrival tries the goal vertices of its faces. O(deg * n_max).
    fn leave_vertex(&self, st: &mut State, parent: usize, layer: usize, v: usize, root: bool) {
        let g = self.ls[layer].g;
        for &f in g.faces_of_vertex(v) {
            if !g.faces[f].inside {
                continue;
            }
            let cyc = Cyc::new(g, f);
            let strands = cyc.strands(g);
            let p0 = cyc.vertex_pos(v).unwrap_or(0);
            for &e_out in &g.faces[f].e {
                let dir = other_dir(g, e_out, f);
                if !g.passable(e_out, self.net) {
                    continue;
                }
                let ge = &g.edges[e_out];
                let near_v = if ge.a == v || ge.b == v { v } else { ge.a };
                if let Some(slot) = cyc.cross_slot(&strands, p0, e_out, near_v) {
                    self.push_state(st, parent, layer, e_out, slot, dir, f);
                }
            }
            for &vv in &g.faces[f].v {
                if vv == v {
                    continue;
                }
                if root {
                    self.try_via(st, parent, layer, vv, &cyc, &strands, p0);
                } else if self.goal_set.contains(&(layer, vv)) && cyc.goal_ok(&strands, p0, vv) {
                    self.push_goal(st, parent, layer, vv);
                }
            }
        }
    }

    fn finish(&self, st: &State, end_id: usize, expansions: usize) -> SlotPath {
        let mut chain = vec![end_id];
        while st.nodes[*chain.last().unwrap()].kind != K_ROOT {
            let p = st.nodes[*chain.last().unwrap()].parent;
            chain.push(p);
        }
        chain.reverse();
        let root = &st.nodes[chain[0]];
        let (mut cur_layer, mut cur_start) = (root.layer, root.v);
        let mut legs: Vec<Leg> = Vec::new();
        let mut sites: Vec<(usize, ViaSize)> = Vec::new();
        let (mut cross, mut faces): (Vec<(usize, usize)>, Vec<usize>) = (Vec::new(), Vec::new());
        let mut start_len = 0.0;
        let mut pending: Option<usize> = None;
        for &i in &chain[1..] {
            let n = &st.nodes[i];
            if let (Some(s), Some(mg)) = (pending.take(), self.mg) {
                cur_layer = n.layer;
                cur_start = mg.site_vertex[s][n.layer];
            }
            match n.kind {
                K_EDGE => {
                    if cross.is_empty() {
                        faces.push(n.face_in);
                    }
                    cross.push((n.edge, n.slot));
                    faces.push(
                        self.ls[n.layer].g.edges[n.edge].faces[n.dir as usize]
                            .unwrap_or(usize::MAX),
                    );
                }
                K_SITE => {
                    let goal_v = self.mg.map_or(0, |mg| mg.site_vertex[n.v][cur_layer]);
                    legs.push(Leg {
                        layer_idx: cur_layer,
                        start: cur_start,
                        goal: goal_v,
                        crossings: std::mem::take(&mut cross),
                        faces: std::mem::take(&mut faces),
                        est_len: n.g_len - start_len,
                    });
                    start_len = n.g_len;
                    sites.push((n.v, n.size.unwrap_or(ViaSize { d: 0, drill: 0 })));
                    pending = Some(n.v);
                }
                _ => {
                    legs.push(Leg {
                        layer_idx: n.layer,
                        start: cur_start,
                        goal: n.v,
                        crossings: std::mem::take(&mut cross),
                        faces: std::mem::take(&mut faces),
                        est_len: n.g_len - start_len,
                    });
                }
            }
        }
        let end = &st.nodes[end_id];
        SlotPath {
            conn: self.conn_id,
            legs,
            sites,
            est_len: end.g_len,
            cost: end.g_cost,
            expansions,
            candidates: self.ls.iter().map(|s| s.cands.get()).sum(),
        }
    }

    /// [`Layered::run`] with the per-process search counters ([`search_totals`]): two clock
    /// reads and a few relaxed atomic adds per search, none in the expansion loop.
    fn run_timed(&self, cap: usize, deadline: Instant) -> Run {
        let t0 = Instant::now();
        let r = if self.ghost {
            self.run_topo(cap, deadline)
        } else {
            self.run(cap, deadline)
        };
        let ns = t0.elapsed().as_nanos() as u64;
        let k = usize::from(self.ghost);
        TOT[k].ns.fetch_add(ns, AtOrd::Relaxed);
        TOT[k].calls.fetch_add(1, AtOrd::Relaxed);
        TOT[k]
            .expansions
            .fetch_add(self.exps.get() as u64, AtOrd::Relaxed);
        let cands: usize = self.ls.iter().map(|s| s.cands.get()).sum();
        let bends: usize = self.ls.iter().map(|s| s.bends.get()).sum();
        TOT[k].candidates.fetch_add(cands as u64, AtOrd::Relaxed);
        TOT[k].bends.fetch_add(bends as u64, AtOrd::Relaxed);
        let skipped: usize = self.ls.iter().map(|s| s.skipped.get()).sum();
        let hits: usize = self.ls.iter().map(|s| s.memo_hits.get()).sum();
        TOT[k].skipped.fetch_add(skipped as u64, AtOrd::Relaxed);
        TOT[k].memo_hits.fetch_add(hits as u64, AtOrd::Relaxed);
        r
    }

    /// The A* loop. `O(V' log V')`; see the module docs.
    fn run(&self, cap: usize, deadline: Instant) -> Run {
        let mut st = State::default();
        for &(l, v) in self.start {
            let at_p = self.ls[l].g.verts[v].at;
            st.nodes.push(Node {
                kind: K_ROOT,
                layer: l,
                v,
                edge: usize::MAX,
                slot: 0,
                dir: 0,
                parent: 0,
                face_in: 0,
                apex: at_p,
                apex_len: 0.0,
                since: 0,
                p: at_p,
                g_len: 0.0,
                g_cost: 0.0,
                vias: 0,
                last_site: usize::MAX,
                size: None,
            });
        }
        // start states: leave every face around each start vertex through each edge
        for (i, &(l, v)) in self.start.iter().enumerate() {
            self.leave_vertex(&mut st, i, l, v, true);
        }
        let mut expansions = 0usize;
        while let Some(it) = st.heap.pop() {
            let node = st.nodes[it.id].clone();
            if node.kind == K_GOAL {
                return Run::Found(Box::new(self.finish(&st, it.id, expansions)));
            }
            if !st.closed.insert(self.node_key(&node)) {
                continue;
            }
            if node.kind == K_SITE && self.goal.pour_sites.binary_search(&node.v).is_ok() {
                return Run::Found(Box::new(self.finish(&st, it.id, expansions)));
            }
            if expansions >= cap {
                return Run::Blocked;
            }
            expansions += 1;
            self.exps.set(expansions);
            if expansions.is_multiple_of(16) && crate::control::expired(deadline) {
                return Run::Timeout;
            }
            if node.kind == K_SITE {
                let Some(mg) = self.mg else {
                    continue;
                };
                for l2 in 0..self.ls.len() {
                    if l2 != node.layer {
                        let v = mg.site_vertex[node.v][l2];
                        self.leave_vertex(&mut st, it.id, l2, v, false);
                    }
                }
                continue;
            }
            let (l, e, slot, dir) = (node.layer, node.edge, node.slot, node.dir);
            let g = self.ls[l].g;
            let Some(f) = g.edges[e].faces[dir as usize] else {
                continue;
            };
            let cyc = Cyc::new(g, f);
            let strands = cyc.strands(g);
            let p0 = cyc.gap_pos(e, slot).unwrap_or(0);
            for &e_out in &g.faces[f].e {
                if e_out == e {
                    continue;
                }
                let nd = other_dir(g, e_out, f);
                if !g.passable(e_out, self.net) {
                    continue;
                }
                let (x, y) = (&g.edges[e], &g.edges[e_out]);
                let Some(v) = [x.a, x.b].into_iter().find(|&v| v == y.a || v == y.b) else {
                    continue;
                };
                if let Some(s2) = cyc.cross_slot(&strands, p0, e_out, v) {
                    self.push_state(&mut st, it.id, l, e_out, s2, nd, f);
                }
            }
            for &vv in &g.faces[f].v {
                if self.goal_set.contains(&(l, vv)) && cyc.goal_ok(&strands, p0, vv) {
                    self.push_goal(&mut st, it.id, l, vv);
                }
                self.try_via(&mut st, it.id, l, vv, &cyc, &strands, p0);
            }
        }
        Run::Blocked
    }
}

/// A `clearance_bend` result: the bend point and the vertex it bends at (`None` for a wall).
type BendResult = Option<(Vec2, Option<usize>)>;

/// The exact-preserving speed-ups of [`Search::clearance_bend`] (A1); every combination returns
/// the same bend, the all-off mode is the reference of the equality tests.
#[derive(Clone, Copy, Debug)]
#[doc(hidden)]
pub struct BendMode {
    /// Skip the candidate scan of a sub-chord inside a face's free disk.
    pub skip: bool,
    /// Memoise per search on the chord's end points.
    pub memo: bool,
    /// Test candidates in ascending chord parameter and stop at the first violation.
    pub early: bool,
}

impl BendMode {
    pub const FAST: BendMode = BendMode {
        skip: true,
        memo: true,
        early: true,
    };
    pub const EXACT: BendMode = BendMode {
        skip: false,
        memo: false,
        early: false,
    };
}

/// Per-layer geometry / cost helper of one search: the graph and obstacles of one layer, the
/// connection's parameters and the layer's history. The A* itself is [`Layered`].
struct Search<'a> {
    g: &'a RouteGraph,
    obs: &'a LayerObstacles,
    conn: &'a Connection,
    net: i32,
    w: f64,
    c: f64,
    eps: f64,
    hist: &'a [f64],
    omega: f64,
    present_overuse: f64,
    ghost: bool,
    /// Reject statically unfit edges in `push_state` (D-1; never in the ghost).
    strict_unfit: bool,
    /// Candidates returned by `clearance_bend` queries (statistics).
    cands: Cell<usize>,
    /// `clearance_bend` calls so far (statistics).
    bends: Cell<usize>,
    /// Sub-chord scans skipped by the face clear radius (statistics).
    skipped: Cell<usize>,
    /// `clearance_bend` calls answered from `memo` (statistics).
    memo_hits: Cell<usize>,
    /// Which exact-preserving speed-ups `clearance_bend` uses (all on; tests switch them off).
    mode: BendMode,
    /// `clearance_bend` results of this search keyed by the chord's end points (bit patterns).
    /// The result is a function of `(a, t)` alone (the portals only restrict the candidate
    /// query to a superset of the chord's sleeve), so membership is the only use: no
    /// iteration order reaches the output.
    memo: RefCell<HashMap<[u64; 4], BendResult>>,
    /// `bend_ok` results of this search keyed by `(a, u, t, side)` (a pure function of its
    /// arguments within one search; membership only).
    ok_memo: RefCell<HashMap<[u64; 7], bool>>,
    /// Per wall index: a graph vertex sits on this point wall (`bend_ok` skips those).
    pw_vertex: RefCell<HashMap<usize, bool>>,
    /// Candidate lists of grid-aligned query boxes, `[vertices, walls]` (see `cached_near`).
    near: [RefCell<NearCache>; 2],
    /// Wall term of the capacity rule (`RouteGraph::wall_free_len`) of this search's net /
    /// width / clearance, keyed by `(edge, lo, hi, c_wall bits)` (membership only). About one
    /// entry per own-end edge the search tests: `O(Σ_{v own} deg(v))` entries, `O(k log k)` each.
    wall_memo: RefCell<HashMap<(usize, u64, u64, u64), f64>>,
    /// `RouteGraph::transit_fits` per `(face, edge, edge)` (membership only).
    transit_memo: RefCell<HashMap<(usize, usize, usize), bool>>,
    /// Usable parameter intervals of an edge for the state point (`state_point`), keyed by the
    /// edge: a function of the edge and the search's net / width / clearance alone (the graph is
    /// not mutated during a search), so membership is the only use.
    sp_memo: RefCell<HashMap<usize, Vec<(f64, f64)>>>,
}

impl<'a> Search<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        g: &'a RouteGraph,
        obs: &'a LayerObstacles,
        conn: &'a Connection,
        hist: &'a [f64],
        weight: f64,
        present_overuse: f64,
        eps: Option<f64>,
        ghost: bool,
    ) -> Self {
        Search {
            g,
            obs,
            conn,
            net: conn.net,
            w: conn.width as f64,
            c: conn.clearance as f64,
            eps: eps.unwrap_or(g.rules.eps),
            hist,
            omega: weight,
            present_overuse,
            ghost,
            strict_unfit: false,
            cands: Cell::new(0),
            bends: Cell::new(0),
            skipped: Cell::new(0),
            memo_hits: Cell::new(0),
            mode: BendMode::FAST,
            memo: RefCell::new(HashMap::new()),
            ok_memo: RefCell::new(HashMap::new()),
            pw_vertex: RefCell::new(HashMap::new()),
            near: [RefCell::new(HashMap::new()), RefCell::new(HashMap::new())],
            wall_memo: RefCell::new(HashMap::new()),
            transit_memo: RefCell::new(HashMap::new()),
            sp_memo: RefCell::new(HashMap::new()),
        }
    }

    /// `u` is an obstacle for this search's net: an owned vertex whose nets do not include it;
    /// a DRC spot (`owner` `spot:*`) blocks only the nets listed in its anchor's `only_nets`
    /// (empty = every net) (D7). O(1).
    fn foreign(&self, u: usize) -> bool {
        let vi = &self.g.verts[u];
        if vi.owner.is_none() {
            return false;
        }
        if vi.owner.as_deref().is_some_and(|o| o.starts_with("spot:")) {
            if let Some(a) = vi.anchor.and_then(|ai| self.obs.anchors.get(ai)) {
                return a.only_nets.is_empty() || a.only_nets.contains(&self.net);
            }
        }
        !vi.nets.contains(&self.net)
    }

    /// Wall not an obstacle for this net: same-net / tie exemption, or a DRC spot (anchor with
    /// `only_nets`) that does not list the net (D7). O(1).
    fn wall_exempt(&self, wl: &crate::obstacle::Wall) -> bool {
        ObstacleSet::exempt(wl.net, &wl.tie_nets, self.net)
            || self
                .obs
                .anchors
                .get(wl.ends[0])
                .is_some_and(|a| !a.only_nets.is_empty() && !a.only_nets.contains(&self.net))
    }

    /// Distance from a vertex a wire's centreline must keep along an edge end.
    fn margin_of(&self, v: usize) -> f64 {
        let vi = &self.g.verts[v];
        if vi.owner.is_none() {
            0.0
        } else if !self.foreign(v) {
            vi.r
        } else {
            vi.r + vi.clr.max(self.c) + 0.5 * self.w + self.eps
        }
    }

    /// State point of edge `e`: usable sub-segment point nearest to the segment `apex -> goal`.
    /// The usable set is the edge minus its end margins (`margin_of`) minus, for every other
    /// foreign vertex `u` within `r_u + max(clr_u, c) + w/2 + eps` of the edge, the parameter
    /// interval where the centreline would come closer than that to `u` (A2 / M4-D2: a 0.3 mm
    /// pad tip 0.2 mm from a foreign pad keeps only the window the wire fits through, instead of
    /// the own corner next to the neighbour). When nothing is left (an unfit edge, crossed as
    /// overuse) the end-margin point is kept. Intervals memoised per edge (`edge_window`); per
    /// call O(k) for k intervals, the first call per edge O(q) (one lattice-cached vertex query).
    fn state_point(&self, e: usize, apex: Vec2, goal: Vec2) -> Vec2 {
        let ge = &self.g.edges[e];
        let (pa, pb) = (self.g.verts[ge.a].at, self.g.verts[ge.b].at);
        let len = dist(pa, pb);
        if len <= 0.0 {
            return pa;
        }
        let d = pb - pa;
        let win = self.edge_window(e);
        let mut best: Option<(f64, Vec2)> = None;
        for &(t0, t1) in win.iter() {
            let q = seg_closest(pa + d * t0, pa + d * t1, apex, goal);
            let qd = point_seg(q, &Seg { a: apex, b: goal });
            if best.is_none_or(|b| qd < b.0) {
                best = Some((qd, q));
            }
        }
        best.map(|b| b.1).unwrap_or(pa)
    }

    /// Usable parameter intervals of edge `e` for the state point (see `state_point`), sorted,
    /// never empty (the end-margin interval when every point is pinched). O(q log q) once per
    /// edge and search, then a memo hit.
    fn edge_window(&self, e: usize) -> Vec<(f64, f64)> {
        if let Some(w) = self.sp_memo.borrow().get(&e) {
            return w.clone();
        }
        let ge = &self.g.edges[e];
        let (pa, pb) = (self.g.verts[ge.a].at, self.g.verts[ge.b].at);
        let len = dist(pa, pb).max(1e-9);
        let (ra, rb) = (self.margin_of(ge.a), self.margin_of(ge.b));
        let base = if ra + rb >= len {
            let t = ra / (ra + rb).max(1e-9);
            (t, t)
        } else {
            (ra / len, 1.0 - rb / len)
        };
        let half = 0.5 * self.w + self.eps;
        let reach = self.g.rho_max() + self.c + half;
        let d = pb - pa;
        let bb = Aabb {
            min: Vec2 {
                x: pa.x.min(pb.x),
                y: pa.y.min(pb.y),
            },
            max: Vec2 {
                x: pa.x.max(pb.x),
                y: pa.y.max(pb.y),
            },
        }
        .grow(reach);
        let mut cand: Vec<usize> = Vec::new();
        self.cached_near(0, bb, reach, &mut cand);
        cand.sort_unstable();
        cand.dedup();
        // excluded parameter intervals `(lo, hi)` of the foreign vertices pinching the edge
        let mut cut: Vec<(f64, f64)> = Vec::new();
        for u in cand {
            if u == ge.a || u == ge.b || !self.foreign(u) {
                continue;
            }
            let vi = &self.g.verts[u];
            let need = vi.r + vi.clr.max(self.c) + half;
            let rel = vi.at - pa;
            let h = d.cross(rel).abs() / len;
            if h >= need {
                continue;
            }
            let tc = rel.dot(d) / (len * len);
            let s = (need * need - h * h).sqrt() / len;
            cut.push((tc - s, tc + s));
        }
        cut.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.total_cmp(&y.1)));
        let mut win: Vec<(f64, f64)> = Vec::new();
        let mut lo = base.0;
        for (c0, c1) in cut {
            if c0 > lo {
                win.push((lo, c0.min(base.1)));
            }
            lo = lo.max(c1);
            if lo > base.1 {
                break;
            }
        }
        if lo <= base.1 {
            win.push((lo, base.1));
        }
        win.retain(|w| w.0 <= w.1);
        if win.is_empty() {
            win.push(base);
        }
        self.sp_memo.borrow_mut().insert(e, win.clone());
        win
    }

    /// A2 / M4-D2: the apex returned for a crossing of edge `e` into `faces[dir]` is the entered
    /// face's third vertex (a clearance bend at a foreign corner across `e`): the string has
    /// passed `e` already, so `e` and the portals before it are no portal of the next line of
    /// sight (they sent it back to the near end of `e`). Only that vertex: an apex farther away
    /// keeps the portal chain (dropping it could let later chords cut corners). O(1).
    fn apex_past(&self, e: usize, dir: u8, apex: Vec2) -> bool {
        let ge = &self.g.edges[e];
        let Some(f) = ge.faces[dir as usize] else {
            return false;
        };
        self.g.faces[f]
            .v
            .iter()
            .any(|&w| w != ge.a && w != ge.b && self.g.verts[w].at == apex)
    }

    /// `a` is the position of a foreign vertex (the apex of a clearance / portal bend of `los`,
    /// as opposed to a terminal or a state point). One lattice-cached point query of pitch `q`;
    /// O(1) amortised.
    fn wrap_apex(&self, a: Vec2, q: f64) -> bool {
        let mut at: Vec<usize> = Vec::new();
        self.cached_near(0, Aabb { min: a, max: a }, q, &mut at);
        at.into_iter()
            .any(|u| self.g.verts[u].at == a && self.foreign(u))
    }

    /// Statically unusable edge: a wire runs along it, a face is outside the outline, or it is
    /// unfit even when empty, `cap < w + eps + gaps * max(c, c_e)` (D2; `gaps` = 2 minus the free
    /// ends). Used by the pinch test of `bend_ok` (D10); the search itself keeps such an edge as
    /// a costly overuse crossing (the board `eprom-to-kernal` routes +5V through one, zero-slack
    /// retry, `route_engine`) and the topological ghost counts it as soft. O(1).
    fn hard(&self, e: usize) -> bool {
        let ge = &self.g.edges[e];
        ge.along.is_some()
            || !self
                .g
                .fits_empty_with(e, self.net, self.w, self.c, self.eps, |lo, hi, cw| {
                    self.wall_free(e, lo, hi, cw)
                })
            || !ge
                .faces
                .iter()
                .all(|f| f.is_some_and(|f| self.g.faces[f].inside))
    }

    /// The edge still fits this connection (`RouteGraph::fits_for`, own-net / same-net,
    /// wall-aware; the wall term memoised per search). O(n_max) + O(k log k) once per edge.
    fn fits(&self, e: usize) -> bool {
        self.g
            .fits_for_with(e, self.net, self.w, self.c, self.eps, |lo, hi, cw| {
                self.wall_free(e, lo, hi, cw)
            })
    }

    /// Memoised `RouteGraph::transit_fits` of this search's net / width / clearance for the
    /// transit of face `f` from `e1` to `e2`. O(k) once per transit.
    fn transit_ok(&self, f: usize, e1: usize, e2: usize) -> bool {
        let key = (f, e1.min(e2), e1.max(e2));
        if let Some(&v) = self.transit_memo.borrow().get(&key) {
            return v;
        }
        let v = self
            .g
            .transit_fits(f, e1, e2, self.net, self.w, self.c, self.eps);
        self.transit_memo.borrow_mut().insert(key, v);
        v
    }

    /// Memoised `RouteGraph::wall_free_len` of edge `e` over `[lo, hi]` at wall clearance `cw`.
    fn wall_free(&self, e: usize, lo: f64, hi: f64, cw: f64) -> f64 {
        let key = (e, lo.to_bits(), hi.to_bits(), cw.to_bits());
        if let Some(&v) = self.wall_memo.borrow().get(&key) {
            return v;
        }
        let v = self.g.wall_free_len(e, self.net, cw, lo, hi);
        self.wall_memo.borrow_mut().insert(key, v);
        v
    }

    /// Soft element of the topological ghost: the slot search cannot cross `e` as it stands
    /// (not passable for the net, or no capacity left; the planar search keeps an unfit edge as
    /// a costly overuse crossing, so soft is never hard). O(n_max).
    fn soft_blocked(&self, e: usize) -> bool {
        !self.g.passable(e, self.net) || !self.fits(e)
    }

    fn wall_clearance(&self, kind: ObsKind, own_clr: f64) -> f64 {
        let r = &self.g.rules;
        match kind {
            ObsKind::Hole => r.hole_clearance,
            ObsKind::Edge => r.edge_clearance,
            ObsKind::Keepout => 0.0,
            _ => own_clr.max(self.c).max(r.min_clearance),
        }
    }

    /// Pieces covering the chord `a -> t` grown by `grow`: one box per sub-chord between
    /// consecutive portal crossing parameters (`crossings`, sorted `(param, portal edge)`), so a
    /// long diagonal chord queries `<= portals.len() + 1` local boxes instead of its full bbox.
    /// Each piece is `(box, p0, p1)`, the chord parameters of its end points. O(C).
    fn sleeve_pieces(
        a: Vec2,
        t: Vec2,
        crossings: &[(f64, usize)],
        grow: f64,
    ) -> Vec<(Aabb, f64, f64)> {
        let r = t - a;
        let mut params: Vec<f64> = crossings.iter().map(|c| c.0).collect();
        params.push(0.0);
        params.push(1.0);
        params.sort_by(f64::total_cmp);
        params.dedup();
        let boxed = |p: Vec2, q: Vec2| {
            Aabb {
                min: Vec2 {
                    x: p.x.min(q.x),
                    y: p.y.min(q.y),
                },
                max: Vec2 {
                    x: p.x.max(q.x),
                    y: p.y.max(q.y),
                },
            }
            .grow(grow)
        };
        // Sub-chords shorter than `MERGE_LEN_FACTOR * grow` are queried together: a grid query
        // has a fixed cost per box, so one box per tiny sub-chord would cost more than it saves.
        let max_t = MERGE_LEN_FACTOR * grow / r.len().max(1.0);
        let mut out = Vec::new();
        let mut i = 0;
        while i + 1 < params.len() {
            let mut j = i + 1;
            while j + 1 < params.len() && params[j + 1] - params[i] <= max_t {
                j += 1;
            }
            out.push((
                boxed(a + r * params[i], a + r * params[j]),
                params[i],
                params[j],
            ));
            i = j;
        }
        out
    }

    /// Append the vertices (`kind` 0) or walls (`kind` 1) near `bb` to `out`. The box is rounded
    /// outwards to a lattice of pitch `q` (the query radius) and the candidate list of that box
    /// is memoised for the lifetime of the search: expansions of one search probe overlapping
    /// corridors again and again, and a grid query costs far more than the exact tests on its
    /// result. The lists are supersets of the exact box's, in the same ascending order, so
    /// the first bend found is unchanged. O(1) amortised per hit.
    fn cached_near(&self, kind: usize, bb: Aabb, q: f64, out: &mut Vec<usize>) {
        let q = q.max(1.0);
        let key = (
            (bb.min.x / q).floor() as i64,
            (bb.min.y / q).floor() as i64,
            (bb.max.x / q).ceil() as i64,
            (bb.max.y / q).ceil() as i64,
        );
        let mut cache = self.near[kind].borrow_mut();
        let list = cache.entry(key).or_insert_with(|| {
            let big = Aabb {
                min: Vec2 {
                    x: key.0 as f64 * q,
                    y: key.1 as f64 * q,
                },
                max: Vec2 {
                    x: key.2 as f64 * q,
                    y: key.3 as f64 * q,
                },
            };
            if kind == 0 {
                self.g.verts_near(big)
            } else {
                self.obs.query_walls(big)
            }
        });
        out.extend_from_slice(list);
    }

    /// A face whose free disk contains the whole sub-chord `p0..p1` of the chord `a -> a + r`
    /// (A1). `radius(f)` is the free radius of face `f` for the obstacle kind being scanned
    /// (`clear_r - rho` for vertices, `clear_w - (c + w/2 + eps)` for walls); the disk is
    /// centred on the face's incentre and convex, so both end points inside means every point
    /// of the sub-chord is inside, i.e. at least the required clearance from every obstacle of
    /// that kind that is not an own corner / own wall of the face (those the caller still tests
    /// exactly). Candidate faces are the ones next to the portals crossed within `[p0, p1]`;
    /// at most 4 portals (8 faces) are tried. A 1 nm margin absorbs rounding. O(1).
    fn free_face(
        &self,
        a: Vec2,
        r: Vec2,
        crossings: &[(f64, usize)],
        p0: f64,
        p1: f64,
        radius: impl Fn(usize) -> f64,
    ) -> Option<usize> {
        let (pl, ql) = (a + r * p0, a + r * p1);
        for &(_, pe) in crossings.iter().filter(|c| c.0 >= p0 && c.0 <= p1).take(4) {
            for f in self.g.edges[pe].faces.iter().flatten() {
                let rad = radius(*f) - 1.0;
                if rad > 0.0 {
                    let c = self.g.incentre[*f];
                    if (pl - c).len2() <= rad * rad && (ql - c).len2() <= rad * rad {
                        return Some(*f);
                    }
                }
            }
        }
        None
    }

    /// First obstacle (along the chord) closer than its required distance: the bend point.
    /// `portals` are the edges the chord crosses (the `since` chain of [`Search::los`]; empty for
    /// a direct chord): the obstacle queries run on the sleeve around the chord, the sub-chord
    /// boxes between consecutive portal crossings, merged into one sorted, dedup'd candidate
    /// list. Three exact-preserving speed-ups ([`BendMode`]): (a) a sub-chord inside a face's
    /// free disk is not scanned (the face's own corners / walls are tested instead), (b) the
    /// result is memoised per search on `(a, t)`, (c) candidates are tested in ascending chord
    /// parameter and the scan stops once no later candidate can beat the best bend.
    /// Per call `O((C + 1) * local)`, `C = portals.len()`; a memo hit is `O(1)`.
    fn clearance_bend(&self, a: Vec2, t: Vec2, portals: &[usize]) -> Option<(Vec2, Option<usize>)> {
        self.bends.set(self.bends.get() + 1);
        if !self.mode.memo {
            return self.scan_bend(a, t, portals);
        }
        let key = [a.x.to_bits(), a.y.to_bits(), t.x.to_bits(), t.y.to_bits()];
        if let Some(hit) = self.memo.borrow().get(&key) {
            self.memo_hits.set(self.memo_hits.get() + 1);
            return *hit;
        }
        let res = self.scan_bend(a, t, portals);
        self.memo.borrow_mut().insert(key, res);
        res
    }

    fn scan_bend(&self, a: Vec2, t: Vec2, portals: &[usize]) -> Option<(Vec2, Option<usize>)> {
        const TOL: f64 = 1.0;
        let chord = Seg { a, b: t };
        let r = t - a;
        let l2 = r.len2().max(1.0);
        let mut crossings: Vec<(f64, usize)> = portals
            .iter()
            .map(|&pe| {
                let ge = &self.g.edges[pe];
                let x = seg_closest(a, t, self.g.verts[ge.a].at, self.g.verts[ge.b].at);
                (((x - a).dot(r) / l2).clamp(0.0, 1.0), pe)
            })
            .collect();
        crossings.sort_by(|x, y| x.0.total_cmp(&y.0));
        let half = 0.5 * self.w + self.eps;
        let rho = self.g.rho_max() + self.c + half;
        // candidates `(lower bound of the offered parameter, kind 0 vertex / 1 wall, index,
        // need)`: the order key `(kind, index)` reproduces the original tie-break
        let mut order: Vec<(f64, u8, usize, f64)> = Vec::new();
        let mut vcand: Vec<usize> = Vec::new();
        for (bb, p0, p1) in Self::sleeve_pieces(a, t, &crossings, rho) {
            if self.mode.skip {
                let free = self.free_face(a, r, &crossings, p0, p1, |f| self.g.clear_r[f] - rho);
                if let Some(f) = free {
                    self.skipped.set(self.skipped.get() + 1);
                    vcand.extend_from_slice(&self.g.faces[f].v);
                    continue;
                }
            }
            self.cached_near(0, bb, rho, &mut vcand);
        }
        vcand.sort_unstable();
        vcand.dedup();
        self.cands.set(self.cands.get() + vcand.len());
        for u in vcand {
            let vi = &self.g.verts[u];
            if !self.foreign(u) || vi.at == a {
                continue;
            }
            let need = vi.r + vi.clr.max(self.c) + half;
            order.push(((vi.at - a).dot(r) / l2, 0, u, need));
        }
        let wr = self
            .g
            .rho_max()
            .max(self.g.rules.edge_clearance)
            .max(self.g.rules.hole_clearance)
            + self.c
            + half;
        let wfree = self.c.max(0.0) + half;
        let mut wcand: Vec<usize> = Vec::new();
        for (bb, p0, p1) in Self::sleeve_pieces(a, t, &crossings, wr) {
            if self.mode.skip {
                let free = self.free_face(a, r, &crossings, p0, p1, |f| self.g.clear_w[f] - wfree);
                if let Some(f) = free {
                    self.skipped.set(self.skipped.get() + 1);
                    wcand.extend_from_slice(&self.g.face_walls[f]);
                    continue;
                }
            }
            self.cached_near(1, bb, wr, &mut wcand);
        }
        wcand.sort_unstable();
        wcand.dedup();
        self.cands.set(self.cands.get() + wcand.len());
        let cb = (a.x.min(t.x), a.y.min(t.y), a.x.max(t.x), a.y.max(t.y));
        for wi in wcand {
            let wl = &self.obs.walls[wi];
            if self.wall_exempt(wl) {
                continue;
            }
            let need = wl.r_core + self.wall_clearance(wl.kind, wl.own_clr) + half;
            // the core is at least its bbox's gap to the chord's bbox away from the chord
            let wb = match &wl.core {
                Core::Point(p) => Some((p.x, p.y, p.x, p.y)),
                Core::Seg(s) => Some((
                    s.a.x.min(s.b.x),
                    s.a.y.min(s.b.y),
                    s.a.x.max(s.b.x),
                    s.a.y.max(s.b.y),
                )),
                Core::Arc(_) => None,
            };
            if let Some(wb) = wb {
                let gx = (cb.0 - wb.2).max(wb.0 - cb.2).max(0.0);
                let gy = (cb.1 - wb.3).max(wb.1 - cb.3).max(0.0);
                if gx * gx + gy * gy >= need * need {
                    continue;
                }
            }
            // every bend point lies on the core, inside its bbox: the smallest corner
            // projection bounds the offered parameter from below
            let bx = wl.core.bbox();
            let mut lb = f64::INFINITY;
            for x in [bx.min.x, bx.max.x] {
                for y in [bx.min.y, bx.max.y] {
                    lb = lb.min(((x - a.x) * r.x + (y - a.y) * r.y) / l2);
                }
            }
            order.push((lb, 1, wi, need));
        }
        order.sort_unstable_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)).then(x.2.cmp(&y.2)));
        let mut best: Option<(f64, u8, usize, Vec2, Option<usize>)> = None;
        for (lb, kind, idx, need) in order {
            if self.mode.early && best.is_some_and(|b| lb > b.0 + 1e-9) {
                break;
            }
            let found: Option<(f64, Vec2, Option<usize>)> = if kind == 0 {
                let vi = &self.g.verts[idx];
                (point_seg(vi.at, &chord) < need - TOL).then_some((lb, vi.at, Some(idx)))
            } else {
                let wl = &self.obs.walls[idx];
                let (d, d_apex, bend) = match &wl.core {
                    Core::Point(p) => (point_seg(*p, &chord), dist(*p, a), *p),
                    Core::Seg(s) => (
                        seg_seg(&chord, s),
                        point_seg(a, s),
                        seg_closest(s.a, s.b, a, t),
                    ),
                    Core::Arc(ar) => {
                        let pts = [ar.point_at(0.0), ar.point_at(0.5), ar.point_at(1.0)];
                        let mut bp = pts[0];
                        for &p in &pts[1..] {
                            if point_seg(p, &chord) < point_seg(bp, &chord) {
                                bp = p;
                            }
                        }
                        (seg_arc(&chord, ar), crate::geom::point_arc(a, ar), bp)
                    }
                };
                if d_apex <= TOL || d >= need - TOL {
                    None
                } else {
                    Some(((bend - a).dot(r) / l2, bend, None))
                }
            };
            if let Some((param, p, v)) = found {
                // A2 / M4-D2: from a wrap apex (the string bends at a foreign vertex and leaves
                // its clearance circle tangentially) an obstacle at or behind the apex is not on
                // the way to `t`: bending there sent the string back (#208's spiral around a
                // 0.3 mm pad tip, the other corner of the same pad 0.3 mm behind the apex)
                if param <= 0.0 && self.wrap_apex(a, rho) {
                    continue;
                }
                let better =
                    best.is_none_or(|b| param < b.0 || (param == b.0 && (kind, idx) < (b.1, b.2)));
                if better {
                    best = Some((param, kind, idx, p, v));
                }
            }
        }
        best.map(|b| (b.3, b.4))
    }

    /// Side the wire passes vertex `u`: the middle direction of the fan of portal edges incident
    /// to `u` that the wire crossed in a row (the chain ending at node `s`, then `via`).
    fn side_hint(&self, u: usize, nodes: &[Node], s: usize, via: Option<usize>) -> Option<Vec2> {
        let dir = |pe: usize| -> Option<Vec2> {
            let ge = &self.g.edges[pe];
            let far = if ge.a == u {
                ge.b
            } else if ge.b == u {
                ge.a
            } else {
                return None;
            };
            let d = self.g.verts[far].at - self.g.verts[u].at;
            (d.len() > 1.0).then(|| d * (1.0 / d.len()))
        };
        let mut fan: Vec<Vec2> = Vec::new();
        let mut id = s;
        while nodes[id].kind == K_EDGE {
            match dir(nodes[id].edge) {
                Some(d) => fan.push(d),
                None => break,
            }
            id = nodes[id].parent;
        }
        fan.reverse();
        if let Some(d) = via.and_then(dir) {
            fan.push(d);
        }
        let first = *fan.first()?;
        let total: f64 = fan
            .windows(2)
            .map(|w| w[0].cross(w[1]).atan2(w[0].dot(w[1])))
            .sum();
        let (sn, cs) = (0.5 * total).sin_cos();
        Some(Vec2 {
            x: first.x * cs - first.y * sn,
            y: first.x * sn + first.y * cs,
        })
    }

    /// Fix A: the string bending at vertex `u` (coming from `a`, leaving towards `t`) wraps the
    /// pad at its bend radius `rho_u`; the arc on the outer side of the turn (sampled at its two
    /// tangent points, its middle and the quarter points) must keep its band from every foreign
    /// wall and vertex. The chord tests alone never see a pad pinched against a wall (note
    /// `m2-route-diagnosis` §4 A). `side` is the unit direction from `u` to the side the wire passes (decided by the
    /// topology: the chord's side, or the fan of crossed portals). O(q) grid queries on the
    /// search's lattice-cached candidate lists; memoised per search on `(a, u, t, side)` (A1:
    /// the same string is re-tested for every slot of an edge), a hit is `O(1)`.
    fn bend_ok(&self, a: Vec2, u: usize, t: Vec2, side: Option<Vec2>) -> bool {
        if self.mode.memo {
            let sd = side.unwrap_or(Vec2 {
                x: f64::NAN,
                y: f64::NAN,
            });
            let key = [
                a.x.to_bits(),
                a.y.to_bits(),
                t.x.to_bits(),
                t.y.to_bits(),
                u as u64,
                sd.x.to_bits(),
                sd.y.to_bits(),
            ];
            let hit = self.ok_memo.borrow().get(&key).copied();
            match hit {
                Some(h) => h,
                None => {
                    let r = self.bend_ok_i(a, u, t, side);
                    self.ok_memo.borrow_mut().insert(key, r);
                    r
                }
            }
        } else {
            self.bend_ok_i(a, u, t, side)
        }
    }

    /// A graph vertex sits exactly on the point wall `wi` at `p` (memoised per search). O(1)
    /// amortised.
    fn point_wall_is_vertex(&self, wi: usize, p: Vec2) -> bool {
        if !self.mode.memo {
            return !self.g.verts_near(Aabb { min: p, max: p }).is_empty();
        }
        *self
            .pw_vertex
            .borrow_mut()
            .entry(wi)
            .or_insert_with(|| !self.g.verts_near(Aabb { min: p, max: p }).is_empty())
    }

    fn bend_ok_i(&self, a: Vec2, u: usize, t: Vec2, side: Option<Vec2>) -> bool {
        let side = side.filter(|h| h.len() > 1e-9).map(|h| h * (1.0 / h.len()));
        const TOL: f64 = 1.0;
        let vi = &self.g.verts[u];
        if !self.foreign(u) {
            return true;
        }
        let (d1, d2) = (vi.at - a, t - vi.at);
        if d1.len() < 1.0 || d2.len() < 1.0 {
            return true;
        }
        let (d1, d2) = (d1 * (1.0 / d1.len()), d2 * (1.0 / d2.len()));
        let perp = |d: Vec2| Vec2 { x: -d.y, y: d.x };
        let Some(mid) = side else {
            return true;
        };
        let toward = |p: Vec2| if p.dot(mid) >= 0.0 { p } else { p * -1.0 };
        let (n_in, n_out) = (toward(perp(d1)), toward(perp(d2)));
        let unit = |p: Vec2| {
            let l = p.len();
            if l < 1e-9 {
                mid
            } else {
                p * (1.0 / l)
            }
        };
        let dirs = [n_in, unit(n_in + mid), mid, unit(n_out + mid), n_out];
        let rho = vi.r + vi.clr.max(self.c) + 0.5 * self.w + self.eps;
        let reach = self.g.rho_max() + self.c + 0.5 * self.w + self.eps;
        let wr = reach
            .max(self.g.rules.edge_clearance + self.c + 0.5 * self.w + self.eps)
            .max(self.g.rules.hole_clearance + self.c + 0.5 * self.w + self.eps);
        for nd in dirs {
            let w = vi.at + nd * rho;
            let wbb = Aabb {
                min: Vec2 {
                    x: w.x - wr,
                    y: w.y - wr,
                },
                max: Vec2 {
                    x: w.x + wr,
                    y: w.y + wr,
                },
            };
            // foreign graph vertices pinching the arc sample (D10)
            let mut vc: Vec<usize> = Vec::new();
            self.cached_near(0, wbb, wr, &mut vc);
            for j in vc {
                let vj = &self.g.verts[j];
                // a pinch against a vertex joined to `u` by an unfit edge is that edge's capacity
                // (D2); only the other, non-adjacent vertices need this test
                if j == u || !self.foreign(j) || self.g.edge(u, j).is_some_and(|e| self.hard(e)) {
                    continue;
                }
                let need = vj.r + vj.clr.max(self.c) + 0.5 * self.w + self.eps;
                if dist(w, vj.at) < need - TOL {
                    return false;
                }
            }
            let mut wc: Vec<usize> = Vec::new();
            self.cached_near(1, wbb, wr, &mut wc);
            for wi in wc {
                let wl = &self.obs.walls[wi];
                if self.wall_exempt(wl) {
                    continue;
                }
                let (d, d_u) = match &wl.core {
                    // point walls that are graph vertices were tested above; the others
                    // (e.g. a keepout point) are tested here
                    Core::Point(p) => {
                        if self.point_wall_is_vertex(wi, *p) {
                            continue;
                        }
                        (dist(w, *p), dist(vi.at, *p))
                    }
                    Core::Seg(s) => (point_seg(w, s), point_seg(vi.at, s)),
                    Core::Arc(ar) => (
                        crate::geom::point_arc(w, ar),
                        crate::geom::point_arc(vi.at, ar),
                    ),
                };
                // u's own shapes (a hole inside its pad, a pad's outline) lie within `rho`
                if d_u + wl.r_core <= rho + TOL {
                    continue;
                }
                let need =
                    wl.r_core + self.wall_clearance(wl.kind, wl.own_clr) + 0.5 * self.w + self.eps;
                if d < need - TOL {
                    return false;
                }
            }
        }
        true
    }

    /// Line of sight from the apex of node `s` to `target` through the portals pushed since:
    /// returns `(apex, apex_len, portals_kept, admissible)`; a pad bend pinched against a wall makes
    /// the line inadmissible (never in ghost mode). A2 / M4-D2: a line leaving a terminal (`s` the
    /// root) is also inadmissible when its bend loop does not converge, i.e. `LOS_STALL_CAP` bends
    /// in a row do not bring the apex closer to the target, or `LOS_PORTAL_CAP + 4` bends end
    /// without a clear chord (U7.4 at 0.2 mm: the exit is pinched at the pad itself, the string
    /// used to spiral 5-9 mm around the neighbours first). Later lines keep the old behaviour (a
    /// pinched gap between two obstacles makes the string oscillate; its accumulated length is
    /// the price of crossing it, which the F.Cu-only eprom negotiation relies on). The #208 runaway
    /// itself is removed at its cause (`edge_window`, `wrap_apex`, `apex_past`). O(LOS_PORTAL_CAP
    /// * (C + bend)).
    fn los(
        &self,
        nodes: &[Node],
        s: usize,
        target: Vec2,
        via: Option<usize>,
    ) -> (Vec2, f64, usize, bool) {
        let sn = &nodes[s];
        let (mut apex, mut len, mut since) = (sn.apex, sn.apex_len, sn.since);
        if since >= LOS_PORTAL_CAP {
            apex = sn.p;
            len = sn.g_len;
            since = 0;
        }
        let mut portals: Vec<usize> = Vec::with_capacity(since);
        let mut id = s;
        for _ in 0..since {
            portals.push(nodes[id].edge);
            id = nodes[id].parent;
        }
        portals.reverse();
        let strict = sn.kind == K_ROOT && !self.ghost;
        let mut ok = true;
        if (target - apex).len2() < 1.0 {
            return (apex, len, portals.len(), ok);
        }
        // bends since the apex last came closer to the target
        let (mut best, mut stalls) = (f64::INFINITY, 0usize);
        for _ in 0..LOS_PORTAL_CAP + 4 {
            let d_t = dist(apex, target);
            if d_t < best - LOS_TOL {
                best = d_t;
                stalls = 0;
            } else {
                stalls += 1;
                if strict && stalls >= LOS_STALL_CAP {
                    return (apex, len, portals.len(), false);
                }
            }
            let chord = Seg { a: apex, b: target };
            let mut miss: Option<(usize, usize)> = None;
            for (k, &pe) in portals.iter().enumerate() {
                let ge = &self.g.edges[pe];
                let (pa, pb) = (self.g.verts[ge.a].at, self.g.verts[ge.b].at);
                if pa == apex || pb == apex {
                    continue;
                }
                if !crosses(apex, target, pa, pb) {
                    let bend = if point_seg(pa, &chord) <= point_seg(pb, &chord) {
                        ge.a
                    } else {
                        ge.b
                    };
                    miss = Some((k, bend));
                    break;
                }
            }
            if let Some((k, bv)) = miss {
                let bend = self.g.verts[bv].at;
                ok &=
                    self.ghost || self.bend_ok(apex, bv, target, self.side_hint(bv, nodes, s, via));
                len += dist(apex, bend);
                apex = bend;
                portals.drain(..=k);
                continue;
            }
            if let Some((bend, bv)) = self.clearance_bend(apex, target, &portals) {
                if let Some(bv) = bv {
                    // the wire keeps the chord's side of `u` (the chord lies in the corridor)
                    let ub = self.g.verts[bv].at;
                    let toward = closest_on_seg(apex, target, ub) - ub;
                    let side = if toward.len() > 1.0 {
                        Some(toward)
                    } else {
                        self.side_hint(bv, nodes, s, via)
                    };
                    ok &= self.ghost || self.bend_ok(apex, bv, target, side);
                }
                let r = target - apex;
                let tb = (bend - apex).dot(r) / r.len2();
                portals.retain(|&pe| {
                    let ge = &self.g.edges[pe];
                    let (pa, pb) = (self.g.verts[ge.a].at, self.g.verts[ge.b].at);
                    let q = pb - pa;
                    let den = r.cross(q);
                    den.abs() < 1e-12 || (pa - apex).cross(q) / den > tb
                });
                len += dist(apex, bend);
                apex = bend;
                continue;
            }
            return (apex, len, portals.len(), ok);
        }
        // the bend loop hit its cap without reaching a clear chord
        (apex, len, portals.len(), ok && !strict)
    }

    fn step_cost(&self, e: usize, delta: f64) -> f64 {
        let hist = self.hist.get(e).copied().unwrap_or(0.0);
        let present = if self.fits(e) {
            1.0
        } else {
            let cap = self.g.edges[e].cap.max(1.0);
            self.present_overuse * (1.0 + self.g.overuse(e) / cap)
        };
        delta * (1.0 + self.omega * hist) * (1.0 + self.omega * (present - 1.0))
    }
}

/// F4 (design note §7): re-validate a path found against a frozen graph in `O(k)` against the
/// current `mg` instead of searching again. Per leg, walking its faces from the previous
/// crossing's gap: every crossed edge passable and fitting, its slot recomputed with
/// `Cyc::cross_slot` against the current strands (the slot search's own rule, including the
/// start rule from a via arrival on the other layer); the goal / site vertex reachable by
/// `goal_ok`; every via site still `site_free`; a direct leg's edge still free as `direct`
/// requires. `Some(path with the recomputed slots)` or `None` (search again). Conservative: an
/// unfit edge (which the search may cross as overuse) is rejected, so the result is never
/// looser than a fresh search.
pub fn revalidate_path(mg: &MultiGraph, c: &Connection, p: &SlotPath) -> Option<SlotPath> {
    let mut q = p.clone();
    let (w, clr) = (c.width as f64, c.clearance as f64);
    for (j, leg) in q.legs.iter_mut().enumerate() {
        let g = &mg.layers[leg.layer_idx].g;
        let n = leg.crossings.len();
        if n == 0 {
            let e = g.edge(leg.start, leg.goal)?;
            if !g.passable(e, c.net) || g.slot_count(e) != 0 || g.along(e).is_some() {
                return None;
            }
        } else {
            if leg.faces.len() != n + 1 || leg.faces.iter().any(|&f| f >= g.faces.len()) {
                return None;
            }
            for k in 0..=n {
                let cyc = Cyc::new(g, leg.faces[k]);
                let strands = cyc.strands(g);
                let p0 = if k == 0 {
                    cyc.vertex_pos(leg.start)?
                } else {
                    cyc.gap_pos(leg.crossings[k - 1].0, leg.crossings[k - 1].1)?
                };
                if k == n {
                    if !cyc.goal_ok(&strands, p0, leg.goal) {
                        return None;
                    }
                    break;
                }
                let e_out = leg.crossings[k].0;
                if !g.passable(e_out, c.net) || !g.fits_for(e_out, c.net, w, clr) {
                    return None;
                }
                if k > 0
                    && !g.transit_fits(
                        leg.faces[k],
                        leg.crossings[k - 1].0,
                        e_out,
                        c.net,
                        w,
                        clr,
                        g.rules.eps,
                    )
                {
                    return None;
                }
                let near_v = leg_near_v(g, leg, k)?;
                leg.crossings[k].1 = cyc.cross_slot(&strands, p0, e_out, near_v)?;
            }
        }
        if let Some(&(s, size)) = q.sites.get(j) {
            if !mg.site_free(s, c.net, size) {
                return None;
            }
        }
    }
    Some(q)
}

/// Shared vertex of the entry and exit edges of face `k` of `leg` (the start vertex for the
/// first face); `None` when the two edges share no vertex.
fn leg_near_v(g: &RouteGraph, leg: &Leg, k: usize) -> Option<usize> {
    let e_out = leg.crossings[k].0;
    if k == 0 {
        let ge = &g.edges[e_out];
        let sv = leg.start;
        return Some(if ge.a == sv || ge.b == sv { sv } else { ge.a });
    }
    let (x, y) = (&g.edges[leg.crossings[k - 1].0], &g.edges[e_out]);
    [x.a, x.b].into_iter().find(|&v| v == y.a || v == y.b)
}

/// Slots of a ghost leg as the slot search would choose them: per face, from the previous
/// crossing's gap, `Cyc::cross_slot` against the face's strands; where no slot is feasible (a
/// strand blocks the chord: that face's `leg_strand_blockers`) the append position
/// `slot_count(e)`. A leg without a complete face list keeps the append positions. O(k · n_max).
fn assign_ghost_slots(g: &RouteGraph, leg: &mut Leg) {
    let n = leg.crossings.len();
    let walkable = leg.faces.len() == n + 1 && leg.faces.iter().all(|&f| f < g.faces.len());
    for k in 0..n {
        let e_out = leg.crossings[k].0;
        let mut slot = g.slot_count(e_out);
        if walkable {
            let cyc = Cyc::new(g, leg.faces[k]);
            let p0 = if k == 0 {
                cyc.vertex_pos(leg.start)
            } else {
                cyc.gap_pos(leg.crossings[k - 1].0, leg.crossings[k - 1].1)
            };
            if let (Some(p0), Some(near_v)) = (p0, leg_near_v(g, leg, k)) {
                if let Some(s) = cyc.cross_slot(&cyc.strands(g), p0, e_out, near_v) {
                    slot = s;
                }
            }
        }
        leg.crossings[k].1 = slot;
    }
}

/// Connections whose strands (Rule T) block the ghost `leg` on its layer graph `g`: walking
/// the leg's faces from its slots (`assign_ghost_slots`), the strands that make `cross_slot`
/// fail for the chord into the next crossing, or `goal_ok` fail at the goal (Fix B,
/// `m2-route-diagnosis` §4 B). Sorted, deduplicated; O(strands in the leg's faces).
pub fn leg_strand_blockers(g: &RouteGraph, leg: &Leg) -> Vec<usize> {
    let (start_v, goal_v) = (leg.start, leg.goal);
    let mut out: Vec<usize> = Vec::new();
    let n = leg.crossings.len();
    if n == 0 {
        // direct ghost along the terminals' edge: whoever crosses that edge blocks it
        if let Some(e) = g.edge(start_v, goal_v) {
            out.extend(g.edges[e].cross.iter().map(|x| x.conn));
        }
        out.sort_unstable();
        out.dedup();
        return out;
    }
    if leg.faces.len() != n + 1 {
        return out;
    }
    for k in 0..=n {
        let f = leg.faces[k];
        if f >= g.faces.len() {
            continue;
        }
        let cyc = Cyc::new(g, f);
        let strands = cyc.strands_conn(g);
        let p0 = if k == 0 {
            cyc.vertex_pos(start_v)
        } else {
            let (e, slot) = leg.crossings[k - 1];
            cyc.gap_pos(e, slot)
        };
        let Some(p0) = p0 else {
            continue;
        };
        if k == n {
            out.extend(cyc.vertex_blockers(&strands, p0, goal_v));
            continue;
        }
        let e_out = leg.crossings[k].0;
        let Some(near_v) = leg_near_v(g, leg, k) else {
            continue;
        };
        out.extend(cyc.edge_blockers(&strands, p0, e_out, near_v));
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Leg ids (P2) whose strands block the slot-free `ghost` path: [`leg_strand_blockers`] per
/// ghost leg on its layer. Sorted, deduplicated; O(strands in the ghost's faces).
pub fn strand_blockers(mg: &MultiGraph, ghost: &SlotPath) -> Vec<usize> {
    let mut out: Vec<usize> = ghost
        .legs
        .iter()
        .flat_map(|leg| leg_strand_blockers(&mg.layers[leg.layer_idx].g, leg))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Zero-crossing path along the edge between the terminals `s` and `t` of layer `layer`, if
/// free. Rejected when the chord bends around a third obstacle inside its clearance band (D3).
/// O(local) for one chord query.
fn direct(sr: &Search, layer: usize, s: usize, t: usize) -> Option<SlotPath> {
    let (g, conn) = (sr.g, sr.conn);
    let e = g.edge(s, t)?;
    if g.passable(e, conn.net) && g.slot_count(e) == 0 && g.along(e).is_none() {
        let (p, q) = (g.verts[s].at, g.verts[t].at);
        if sr.clearance_bend(p, q, &[]).is_some() {
            return None;
        }
        let len = dist(p, q);
        return Some(SlotPath {
            conn: conn.id,
            legs: vec![Leg {
                layer_idx: layer,
                start: s,
                goal: t,
                crossings: Vec::new(),
                faces: Vec::new(),
                est_len: len,
            }],
            sites: Vec::new(),
            est_len: len,
            cost: len,
            expansions: 0,
            candidates: sr.cands.get(),
        });
    }
    None
}

/// Candidate lists keyed by a lattice-aligned query box.
type NearCache = HashMap<(i64, i64, i64, i64), Vec<usize>>;

struct Tot {
    ns: AtomicU64,
    calls: AtomicU64,
    expansions: AtomicU64,
    candidates: AtomicU64,
    bends: AtomicU64,
    skipped: AtomicU64,
    memo_hits: AtomicU64,
}
impl Tot {
    const fn new() -> Tot {
        Tot {
            ns: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            expansions: AtomicU64::new(0),
            candidates: AtomicU64::new(0),
            bends: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            memo_hits: AtomicU64::new(0),
        }
    }
}
static TOT: [Tot; 2] = [Tot::new(), Tot::new()];

/// Per-process totals of the A* runs: `[planar, ghost]` as `(nanoseconds, runs, expansions,
/// candidates, clearance_bend calls, skipped sub-chord scans, memo hits)`. Diagnostic counters for `route_probe`; reset with [`search_totals_reset`].
pub fn search_totals() -> [(u64, u64, u64, u64, u64, u64, u64); 2] {
    let f = |t: &Tot| {
        (
            t.ns.load(AtOrd::Relaxed),
            t.calls.load(AtOrd::Relaxed),
            t.expansions.load(AtOrd::Relaxed),
            t.candidates.load(AtOrd::Relaxed),
            t.bends.load(AtOrd::Relaxed),
            t.skipped.load(AtOrd::Relaxed),
            t.memo_hits.load(AtOrd::Relaxed),
        )
    };
    [f(&TOT[0]), f(&TOT[1])]
}

pub fn search_totals_reset() {
    for t in &TOT {
        for a in [
            &t.ns,
            &t.calls,
            &t.expansions,
            &t.candidates,
            &t.bends,
            &t.skipped,
            &t.memo_hits,
        ] {
            a.store(0, AtOrd::Relaxed);
        }
    }
}

/// `direct` per layer in index order (D3 included), starts and goals in the given order; the
/// first free one wins. O(layers * starts * goals * local).
fn find_direct(lay: &Layered) -> Option<SlotPath> {
    for (li, sr) in lay.ls.iter().enumerate() {
        for &(l, s) in lay.start.iter().filter(|x| x.0 == li) {
            let _ = l;
            for &(_, t) in lay.goal.vertices.iter().filter(|x| x.0 == li) {
                if let Some(d) = direct(sr, li, s, t) {
                    return Some(d);
                }
            }
        }
    }
    None
}

/// First soft element of a ghost: `(edge (layer_idx, edge), site, crossing before it)`.
type SoftHit = (Option<(usize, usize)>, Option<usize>, Option<(usize, usize)>);

type GhostOut = (
    SlotPath,
    Option<(usize, usize)>,
    Option<usize>,
    Option<(usize, usize)>,
);

fn search_in(
    env: &Env,
    hists: &[&[f64]],
    conn: &Connection,
    start: &[(usize, usize)],
    goal: &Goal,
    p: &Common,
) -> SearchOutcome {
    if crate::control::expired(p.deadline) {
        return SearchOutcome::Timeout;
    }
    if let Some(reason) = static_block(env, conn, start, goal) {
        return SearchOutcome::Blocked {
            ghost: None,
            blocked_edge: None,
            blocked_site: None,
            last_reached: None,
            static_reason: Some(reason),
        };
    }
    let lay = Layered::new(env, hists, conn, start, goal, p, false);
    if let Some(d) = find_direct(&lay) {
        return SearchOutcome::Found(d);
    }
    let cap = p
        .expansion_cap
        .min(cap_of(&env.graphs(), env.sites(), false));
    match lay.run_timed(cap, p.deadline) {
        Run::Found(path) => SearchOutcome::Found(*path),
        Run::Timeout => SearchOutcome::Timeout,
        Run::Blocked => match ghost_in(env, hists, conn, start, goal, p) {
            Some((path, blocked_edge, blocked_site, last_reached)) => SearchOutcome::Blocked {
                ghost: Some(path),
                blocked_edge,
                blocked_site,
                last_reached,
                static_reason: None,
            },
            None => SearchOutcome::Blocked {
                ghost: None,
                blocked_edge: None,
                blocked_site: None,
                last_reached: None,
                static_reason: None,
            },
        },
    }
}

/// Node of the topological ghost (F2): an edge crossing, a site, a goal, or a root.
#[derive(Clone, Debug)]
struct TNode {
    kind: u8,
    layer: usize,
    /// Root / goal: the vertex; site: the site id.
    v: usize,
    edge: usize,
    dir: u8,
    parent: usize,
    /// Face the wire was in when it crossed `edge`.
    face_in: usize,
    /// Edge midpoint, site point, terminal point.
    p: Vec2,
    soft: u32,
    len: f64,
    vias: u32,
    /// Latest `K_SITE` ancestor; `usize::MAX` if none.
    last_site: usize,
    size: Option<ViaSize>,
}

/// Heap item of the topological ghost: `(soft, length)` lexicographic, then the state for a
/// total order.
#[derive(Clone, Copy, Debug)]
struct TItem {
    soft: u32,
    len: f64,
    layer: usize,
    edge: usize,
    dir: u8,
    id: usize,
}

impl Ord for TItem {
    fn cmp(&self, o: &Self) -> Ordering {
        o.soft
            .cmp(&self.soft)
            .then(o.len.total_cmp(&self.len))
            .then(o.layer.cmp(&self.layer))
            .then(o.edge.cmp(&self.edge))
            .then(o.dir.cmp(&self.dir))
            .then(o.id.cmp(&self.id))
    }
}
impl PartialOrd for TItem {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl PartialEq for TItem {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for TItem {}

#[derive(Default)]
struct TState {
    nodes: Vec<TNode>,
    heap: BinaryHeap<TItem>,
    /// Best node per key; a popped item whose node is no longer the best is stale.
    best: HashMap<Key, usize>,
}

impl TState {
    fn better(&self, key: Key, soft: u32, len: f64) -> bool {
        match self.best.get(&key) {
            None => true,
            Some(&id) => {
                let b = &self.nodes[id];
                (soft, len) < (b.soft, b.len)
            }
        }
    }
}

/// Topological-ghost moves of a [`Layered`] (F2): the same adjacency as the slot search, no
/// geometry.
impl<'a> Layered<'a> {
    /// Point of edge `e` where the ghost crosses it coming from `from`: the point of the edge
    /// nearest to the segment `from -> nearest goal` (the slot search's state point without the
    /// vertex margins). A midpoint would misjudge long edges of a coarse mesh. O(1).
    fn edge_point(&self, layer: usize, e: usize, from: Vec2) -> Vec2 {
        let g = self.ls[layer].g;
        let ge = &g.edges[e];
        seg_closest(g.verts[ge.a].at, g.verts[ge.b].at, from, self.nearest(from))
    }

    /// Push the crossing of `e` out of face `fin` into `edges[e].faces[dir]` (must be inside).
    #[allow(clippy::too_many_arguments)]
    fn tpush_edge(
        &self,
        st: &mut TState,
        parent: usize,
        layer: usize,
        e: usize,
        dir: u8,
        fin: usize,
    ) {
        let g = self.ls[layer].g;
        let Some(f) = g.edges[e].faces[dir as usize] else {
            return;
        };
        if !g.faces[f].inside {
            return;
        }
        let pn = &st.nodes[parent];
        let p = self.edge_point(layer, e, pn.p);
        let soft = pn.soft + u32::from(self.ls[layer].soft_blocked(e));
        let len = pn.len + dist(pn.p, p);
        let (vias, last_site) = (pn.vias, Self::tinherit(st, parent));
        let key = (layer, e, 0, dir, vias);
        if !st.better(key, soft, len) {
            return;
        }
        let id = st.nodes.len();
        st.best.insert(key, id);
        st.nodes.push(TNode {
            kind: K_EDGE,
            layer,
            v: 0,
            edge: e,
            dir,
            parent,
            face_in: fin,
            p,
            soft,
            len,
            vias,
            last_site,
            size: None,
        });
        st.heap.push(TItem {
            soft,
            len,
            layer,
            edge: e,
            dir,
            id,
        });
    }

    /// Push the goal vertex `v` of `layer`; `soft_edge` is the direct terminal edge of a root
    /// (soft when not passable), `None` otherwise.
    fn tpush_goal(&self, st: &mut TState, parent: usize, layer: usize, v: usize, extra_soft: u32) {
        let g = self.ls[layer].g;
        let at = g.verts[v].at;
        let pn = &st.nodes[parent];
        let (soft, len) = (pn.soft + extra_soft, pn.len + dist(pn.p, at));
        let (vias, last_site) = (pn.vias, Self::tinherit(st, parent));
        let id = st.nodes.len();
        st.nodes.push(TNode {
            kind: K_GOAL,
            layer,
            v,
            edge: usize::MAX,
            dir: 1,
            parent,
            face_in: 0,
            p: at,
            soft,
            len,
            vias,
            last_site,
            size: None,
        });
        st.heap.push(TItem {
            soft,
            len,
            layer,
            edge: usize::MAX,
            dir: 1,
            id,
        });
    }

    /// Via move to the site vertex `v` of a face entered on `layer` from `parent`: `vias <
    /// max_vias`, the site not in the ancestor chain, some size fits the site (occupancy is
    /// soft). O(vias + |sizes|).
    fn ttry_via(&self, st: &mut TState, parent: usize, layer: usize, v: usize) {
        let Some(s) = self.ls[layer].g.verts[v].site else {
            return;
        };
        let Some(mg) = self.mg else {
            return;
        };
        if st.nodes[parent].vias >= self.max_vias {
            return;
        }
        let mut id = Self::tinherit(st, parent);
        while id != usize::MAX {
            if st.nodes[id].v == s {
                return;
            }
            id = st.nodes[id].last_site;
        }
        let Some(size) = self.via_choice(s) else {
            return;
        };
        let at = mg.sites[s].at;
        let pn = &st.nodes[parent];
        let soft = pn.soft + u32::from(self.site_soft_blocked(s));
        let len = pn.len + dist(pn.p, at);
        let vias = pn.vias + 1;
        let key = (layer, usize::MAX, s, 0, vias);
        if !st.better(key, soft, len) {
            return;
        }
        let last_site = Self::tinherit(st, parent);
        let id = st.nodes.len();
        st.best.insert(key, id);
        st.nodes.push(TNode {
            kind: K_SITE,
            layer,
            v: s,
            edge: usize::MAX,
            dir: 0,
            parent,
            face_in: 0,
            p: at,
            soft,
            len,
            vias,
            last_site,
            size: Some(size),
        });
        st.heap.push(TItem {
            soft,
            len,
            layer,
            edge: usize::MAX,
            dir: 0,
            id,
        });
    }

    fn tinherit(st: &TState, parent: usize) -> usize {
        if st.nodes[parent].kind == K_SITE {
            parent
        } else {
            st.nodes[parent].last_site
        }
    }

    /// Leave vertex `v` of `layer` (a root or a via arrival): every edge of every inside face
    /// around it; a root also tries the direct terminal edge and the via sites of those faces, a
    /// via arrival the goal vertices. O(deg).
    fn tleave_vertex(&self, st: &mut TState, parent: usize, layer: usize, v: usize, root: bool) {
        let g = self.ls[layer].g;
        for &f in g.faces_of_vertex(v) {
            if !g.faces[f].inside {
                continue;
            }
            for &e_out in &g.faces[f].e {
                let dir = other_dir(g, e_out, f);
                self.tpush_edge(st, parent, layer, e_out, dir, f);
            }
            for &vv in &g.faces[f].v {
                if vv == v {
                    continue;
                }
                if self.goal_set.contains(&(layer, vv)) {
                    let extra = if root {
                        u32::from(!g.edge(v, vv).is_some_and(|e| g.passable(e, self.net)))
                    } else {
                        0
                    };
                    self.tpush_goal(st, parent, layer, vv, extra);
                }
                if root {
                    self.ttry_via(st, parent, layer, vv);
                }
            }
        }
    }

    /// The topological ghost (F2): Dijkstra on `(soft, length)`; `cap` bounds the pops, the
    /// deadline is checked every 16 pops. Deterministic: index-order adjacency, total heap order.
    fn run_topo(&self, cap: usize, deadline: Instant) -> Run {
        let mut st = TState::default();
        for &(l, v) in self.start {
            st.nodes.push(TNode {
                kind: K_ROOT,
                layer: l,
                v,
                edge: usize::MAX,
                dir: 0,
                parent: 0,
                face_in: 0,
                p: self.ls[l].g.verts[v].at,
                soft: 0,
                len: 0.0,
                vias: 0,
                last_site: usize::MAX,
                size: None,
            });
        }
        for (i, &(l, v)) in self.start.iter().enumerate() {
            self.tleave_vertex(&mut st, i, l, v, true);
        }
        let mut pops = 0usize;
        while let Some(it) = st.heap.pop() {
            let node = st.nodes[it.id].clone();
            if node.kind == K_GOAL {
                return Run::Found(Box::new(self.tfinish(&st, it.id, pops)));
            }
            let key = match node.kind {
                K_SITE => (node.layer, usize::MAX, node.v, 0, node.vias),
                _ => (node.layer, node.edge, 0, node.dir, node.vias),
            };
            if st.best.get(&key) != Some(&it.id) {
                continue; // stale
            }
            if node.kind == K_SITE && self.goal.pour_sites.binary_search(&node.v).is_ok() {
                return Run::Found(Box::new(self.tfinish(&st, it.id, pops)));
            }
            if pops >= cap {
                return Run::Blocked;
            }
            pops += 1;
            self.exps.set(pops);
            if pops.is_multiple_of(16) && crate::control::expired(deadline) {
                return Run::Timeout;
            }
            if node.kind == K_SITE {
                let Some(mg) = self.mg else {
                    continue;
                };
                for l2 in 0..self.ls.len() {
                    if l2 != node.layer {
                        let v = mg.site_vertex[node.v][l2];
                        self.tleave_vertex(&mut st, it.id, l2, v, false);
                    }
                }
                continue;
            }
            let (l, e, dir) = (node.layer, node.edge, node.dir);
            let g = self.ls[l].g;
            let Some(f) = g.edges[e].faces[dir as usize] else {
                continue;
            };
            for &e_out in &g.faces[f].e {
                if e_out != e {
                    self.tpush_edge(&mut st, it.id, l, e_out, other_dir(g, e_out, f), f);
                }
            }
            for &vv in &g.faces[f].v {
                if self.goal_set.contains(&(l, vv)) {
                    self.tpush_goal(&mut st, it.id, l, vv, 0);
                }
                self.ttry_via(&mut st, it.id, l, vv);
            }
        }
        Run::Blocked
    }

    /// Path of the topological ghost ending at `end_id`: legs cut at the sites, ghost slots
    /// (`slot_count`), `est_len` = funnel length per leg. O(k).
    fn tfinish(&self, st: &TState, end_id: usize, pops: usize) -> SlotPath {
        let mut chain = vec![end_id];
        while st.nodes[*chain.last().unwrap()].kind != K_ROOT {
            let p = st.nodes[*chain.last().unwrap()].parent;
            chain.push(p);
        }
        chain.reverse();
        let root = &st.nodes[chain[0]];
        let (mut cur_layer, mut cur_start) = (root.layer, root.v);
        let mut legs: Vec<Leg> = Vec::new();
        let mut sites: Vec<(usize, ViaSize)> = Vec::new();
        let (mut cross, mut faces): (Vec<(usize, usize)>, Vec<usize>) = (Vec::new(), Vec::new());
        let mut pending: Option<usize> = None;
        let mut hops = 0.0;
        let cut = |legs: &mut Vec<Leg>,
                   layer: usize,
                   start: usize,
                   goal: usize,
                   cross: &mut Vec<(usize, usize)>,
                   faces: &mut Vec<usize>| {
            let g = self.ls[layer].g;
            let crossings = std::mem::take(cross);
            let faces = std::mem::take(faces);
            let est_len = funnel_len(g, start, goal, &crossings, &faces);
            legs.push(Leg {
                layer_idx: layer,
                start,
                goal,
                crossings,
                faces,
                est_len,
            });
        };
        for &i in &chain[1..] {
            let n = &st.nodes[i];
            if let (Some(s), Some(mg)) = (pending.take(), self.mg) {
                cur_layer = n.layer;
                cur_start = mg.site_vertex[s][n.layer];
            }
            match n.kind {
                K_EDGE => {
                    if cross.is_empty() {
                        faces.push(n.face_in);
                    }
                    let g = self.ls[n.layer].g;
                    cross.push((n.edge, g.slot_count(n.edge)));
                    faces.push(g.edges[n.edge].faces[n.dir as usize].unwrap_or(usize::MAX));
                }
                K_SITE => {
                    let goal_v = self.mg.map_or(0, |mg| mg.site_vertex[n.v][cur_layer]);
                    cut(&mut legs, cur_layer, cur_start, goal_v, &mut cross, &mut faces);
                    sites.push((n.v, n.size.unwrap_or(ViaSize { d: 0, drill: 0 })));
                    pending = Some(n.v);
                    hops = n.len;
                }
                _ => {
                    cut(&mut legs, n.layer, cur_start, n.v, &mut cross, &mut faces);
                    hops = n.len;
                }
            }
        }
        let est_len: f64 = legs.iter().map(|l| l.est_len).sum();
        let _ = hops;
        SlotPath {
            conn: self.conn_id,
            legs,
            sites,
            est_len,
            cost: est_len,
            expansions: pops,
            candidates: 0,
        }
    }
}

/// Funnel (string-pulled) length of a leg `start -> crossings -> goal` on `g`: the shortest
/// string through the crossed edges in that homotopy class, `O(k)` (`sketch::funnel`). A direct
/// leg is the straight distance.
fn funnel_len(
    g: &RouteGraph,
    start: usize,
    goal: usize,
    crossings: &[(usize, usize)],
    faces: &[usize],
) -> f64 {
    let (a, b) = (g.verts[start].at, g.verts[goal].at);
    if crossings.is_empty() || faces.len() != crossings.len() + 1 {
        return dist(a, b);
    }
    let portals: Vec<(Vec2, Vec2)> = crossings
        .iter()
        .zip(faces)
        .map(|(&(e, _), &prev)| {
            let (l, r) = g.crossing_lr(e, prev);
            (g.verts[l].at, g.verts[r].at)
        })
        .collect();
    let bends = crate::sketch::funnel(a, b, &portals);
    let mut len = 0.0;
    let mut last = a;
    for &(i, left, _) in &bends {
        let q = if left { portals[i].0 } else { portals[i].1 };
        len += dist(last, q);
        last = q;
    }
    len + dist(last, b)
}

/// The topological ghost (F2, note `m3-gate-diagnosis` RC2a): Dijkstra over `(layer, edge,
/// dir, vias)` with key `(soft, length)`, no geometry. Returns the path (ghost slots, funnel
/// `est_len`) and its first soft element in travel order (P5). `None` only when no path exists
/// at all (faces outside the outline, `max_vias`), or at the deadline / pop cap.
fn ghost_in(
    env: &Env,
    hists: &[&[f64]],
    conn: &Connection,
    start: &[(usize, usize)],
    goal: &Goal,
    p: &Common,
) -> Option<GhostOut> {
    if static_block(env, conn, start, goal).is_some() {
        return None;
    }
    let lay = Layered::new(env, hists, conn, start, goal, p, true);
    let cap = cap_of(&env.graphs(), env.sites(), true);
    let Run::Found(mut path) = lay.run_timed(cap, p.deadline) else {
        return None;
    };
    for leg in &mut path.legs {
        assign_ghost_slots(env.layers[leg.layer_idx].0, leg);
    }
    // the first soft-blocked element in travel order: a crossing, or the site ending a leg
    let mut first: Option<SoftHit> = None;
    for (j, leg) in path.legs.iter().enumerate() {
        let sr = &lay.ls[leg.layer_idx];
        if leg.crossings.is_empty() {
            // direct leg along the terminals' edge: soft when the edge is not passable
            if let Some(e) = sr.g.edge(leg.start, leg.goal) {
                if !sr.g.passable(e, conn.net) {
                    first = Some((Some((leg.layer_idx, e)), None, None));
                    break;
                }
            }
        }
        if let Some(k) = leg.crossings.iter().position(|&(e, _)| sr.soft_blocked(e)) {
            let edge = (leg.layer_idx, leg.crossings[k].0);
            first = Some((Some(edge), None, k.checked_sub(1).map(|k| (j, k))));
            break;
        }
        if let Some(&(s, _)) = path.sites.get(j) {
            if lay.site_soft_blocked(s) {
                let last = leg.crossings.len().checked_sub(1).map(|k| (j, k));
                first = Some((None, Some(s), last));
                break;
            }
        }
    }
    let (edge, site, last) = first.unwrap_or((None, None, None));
    Some((*path, edge, site, last))
}

fn hist_refs(history: &[Vec<f64>], layers: usize) -> Vec<&[f64]> {
    (0..layers)
        .map(|i| history.get(i).map_or(&[][..], |v| v.as_slice()))
        .collect()
}

fn common<'a>(p: &SearchParams<'a>) -> Common<'a> {
    Common {
        site_history: p.site_history,
        weight: p.weight,
        present_overuse: p.present_overuse,
        expansion_cap: p.expansion_cap,
        deadline: p.deadline,
        eps: p.eps,
        via_cost: p.via_cost,
        max_vias: p.max_vias,
        strict_unfit: p.strict_unfit_edges,
    }
}

/// Layered slot search for `conn` from the `start` vertices `(layer_idx, vertex)` (one per
/// layer of the source terminal) to `goal`. A blocked search runs [`ghost`] under the same
/// deadline and reports its path and first blocked edge / site.
///
/// Complexity: `O(V' log V')`, `V' = sum_L 2 (E_L + X_L) + 2 S`; per expansion
/// `O(n_max + C^2 + (C + 1) local)`; expansions are capped by
/// `min(p.expansion_cap, default_expansion_cap(mg))`.
pub fn search(
    mg: &MultiGraph,
    obs: &ObstacleSet,
    conn: &Connection,
    start: &[(usize, usize)],
    goal: &Goal,
    p: &SearchParams,
) -> SearchOutcome {
    let env = Env::of(mg, obs);
    let hists = hist_refs(p.history, env.layers.len());
    search_in(&env, &hists, conn, start, goal, &common(p))
}

/// Topological ghost search (F2): a Dijkstra over `(layer, edge, dir, vias)` keyed by
/// `(soft elements, length)`, where a soft element is an edge that is not passable or does not
/// fit, or an owned / blocked site (vias and layers are otherwise free, `max_vias` still
/// applies); only faces outside the outline are hard. Returns `(path, first blocked edge
/// (layer_idx, edge), first blocked site, crossing before it (leg, index))`: the first
/// soft-blocked element in travel order (P5); ghost crossings carry the slot the slot search
/// would choose (`cross_slot`; the append position `slot_count(edge)` where no slot is
/// feasible), `est_len` is the funnel length of the crossing sequence. Cap `8 sum_L E_L + 4 S +
/// 64` pops; `O(sum_L E_L (max_vias + 1))`, no clearance candidates. `None` also for a
/// terminal that cannot be left / entered statically ([`terminal_open`], F3).
pub fn ghost(
    mg: &MultiGraph,
    obs: &ObstacleSet,
    conn: &Connection,
    start: &[(usize, usize)],
    goal: &Goal,
    p: &SearchParams,
) -> Option<GhostOut> {
    let env = Env::of(mg, obs);
    let hists = hist_refs(p.history, env.layers.len());
    ghost_in(&env, &hists, conn, start, goal, &common(p))
}

/// Test hook: the first clearance bend of the chord `a -> t` of `conn`, with the sleeve query
/// (`portals` = edges crossed) or, with `full`, the single full-bbox query it replaced.
#[doc(hidden)]
pub fn probe_clearance_bend(
    g: &RouteGraph,
    obs: &LayerObstacles,
    conn: &Connection,
    a: Vec2,
    t: Vec2,
    portals: &[usize],
    full: bool,
) -> Option<(Vec2, Option<usize>)> {
    let s = Search::new(g, obs, conn, &[], 1.0, PRESENT_OVERUSE, None, false);
    // no portals = one box over the chord's whole bbox
    s.clearance_bend(a, t, if full { &[] } else { portals })
}

/// Test hook: `clearance_bend` of `a -> t` under `mode` on a fresh search, called `repeat` times
/// (a memoising mode answers the repeats from the memo); returns the last result, the number of
/// skipped sub-chord scans and the memo hits.
#[doc(hidden)]
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn probe_clearance_bend_mode(
    g: &RouteGraph,
    obs: &LayerObstacles,
    conn: &Connection,
    a: Vec2,
    t: Vec2,
    portals: &[usize],
    mode: BendMode,
    repeat: usize,
) -> (Option<(Vec2, Option<usize>)>, usize, usize) {
    let mut s = Search::new(g, obs, conn, &[], 1.0, PRESENT_OVERUSE, None, false);
    s.mode = mode;
    let mut last = None;
    for _ in 0..repeat.max(1) {
        last = s.clearance_bend(a, t, portals);
    }
    (last, s.skipped.get(), s.memo_hits.get())
}

/// Test hook: [`Search::bend_ok`] of a string `a -> u -> t` passing `u` on the `side` direction.
#[doc(hidden)]
pub fn probe_bend_ok(
    g: &RouteGraph,
    obs: &LayerObstacles,
    conn: &Connection,
    a: Vec2,
    u: usize,
    t: Vec2,
    side: Vec2,
) -> bool {
    Search::new(g, obs, conn, &[], 1.0, PRESENT_OVERUSE, None, false).bend_ok(a, u, t, Some(side))
}

/// Test hook: `bend_ok` under `mode` on a fresh search, called `repeat` times (the memo answers
/// the repeats); the last result.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn probe_bend_ok_mode(
    g: &RouteGraph,
    obs: &LayerObstacles,
    conn: &Connection,
    a: Vec2,
    u: usize,
    t: Vec2,
    side: Vec2,
    mode: BendMode,
    repeat: usize,
) -> bool {
    let mut s = Search::new(g, obs, conn, &[], 1.0, PRESENT_OVERUSE, None, false);
    s.mode = mode;
    let mut last = true;
    for _ in 0..repeat.max(1) {
        last = s.bend_ok(a, u, t, Some(side));
    }
    last
}

/// Test hook (A2): the first step of the slot search leaving vertex `v` through edge `e` towards
/// `goal`: the state point on `e` (`state_point`) and the line of sight to it from `v`
/// (`los`). Returns `(state point, apex, apex_len, admissible)`. O(q + LOS_PORTAL_CAP * bend).
#[doc(hidden)]
pub fn probe_entry(
    g: &RouteGraph,
    obs: &LayerObstacles,
    conn: &Connection,
    v: usize,
    e: usize,
    goal: Vec2,
) -> (Vec2, Vec2, f64, bool) {
    let s = Search::new(g, obs, conn, &[], 1.0, PRESENT_OVERUSE, None, false);
    let at = g.verts[v].at;
    let root = Node {
        kind: K_ROOT,
        layer: 0,
        v,
        edge: usize::MAX,
        slot: 0,
        dir: 0,
        parent: 0,
        face_in: 0,
        apex: at,
        apex_len: 0.0,
        since: 0,
        p: at,
        g_len: 0.0,
        g_cost: 0.0,
        vias: 0,
        last_site: usize::MAX,
        size: None,
    };
    let p = s.state_point(e, at, goal);
    let (apex, apex_len, _, ok) = s.los(&[root], 0, p, Some(e));
    (p, apex, apex_len, ok)
}

/// The single-layer (M2) entry points: the layered core on one [`RouteGraph`] without via
/// sites. `negotiate` / `route` use them until the multi-layer negotiation lands (Task 9);
/// [`SlotPath::single_layer`] turns a [`PlanarPath`] into the layered type.
pub mod planar {
    use super::*;

    /// A single-layer path: one run of `(edge, slot)` crossings.
    #[derive(Clone, Debug)]
    pub struct PlanarPath {
        pub conn: usize,
        /// `(edge, slot)` in travel order, slots as seen by the search (the graph before the insert).
        pub crossings: Vec<(usize, usize)>,
        /// Start face first; `faces.len() == crossings.len() + 1` (empty for a direct path).
        pub faces: Vec<usize>,
        pub est_len: f64,
        pub expansions: usize,
        /// Sum of the clearance-query candidates (vertices + walls) examined over the whole search.
        pub candidates: usize,
    }

    impl PlanarPath {
        fn of(p: SlotPath) -> PlanarPath {
            let leg = p.legs.into_iter().next();
            let (crossings, faces) =
                leg.map_or((Vec::new(), Vec::new()), |l| (l.crossings, l.faces));
            PlanarPath {
                conn: p.conn,
                crossings,
                faces,
                est_len: p.est_len,
                expansions: p.expansions,
                candidates: p.candidates,
            }
        }
    }

    #[derive(Clone, Debug)]
    pub enum SearchOutcome {
        Found(PlanarPath),
        Blocked {
            ghost: Option<PlanarPath>,
            blocked_edge: Option<usize>,
            last_reached: Option<(usize, usize)>,
        },
        Timeout,
    }

    #[derive(Clone, Copy, Debug)]
    pub struct SearchParams<'a> {
        /// Per-edge history cost (indexed by edge; missing entries are 0).
        pub history: &'a [f64],
        /// `omega`: history / present weight of the connection's tier.
        pub weight: f64,
        pub present_overuse: f64,
        pub expansion_cap: usize,
        pub deadline: Instant,
        /// Build margin ε of the admissibility and geometry tests; `None` = the graph's
        /// `rules.eps`. The zero-slack retry (R6) searches one connection at `Some(0.0)`.
        pub eps: Option<f64>,
    }

    fn common(p: &SearchParams) -> Common<'static> {
        Common {
            site_history: &[],
            weight: p.weight,
            present_overuse: p.present_overuse,
            expansion_cap: p.expansion_cap,
            deadline: p.deadline,
            eps: p.eps,
            via_cost: 0.0,
            max_vias: 0,
            strict_unfit: false,
        }
    }

    /// Expansion cap `8 (E + X) + 64` of the planar slot search. O(E).
    pub fn default_expansion_cap(g: &RouteGraph) -> usize {
        cap_of(&[g], 0, false)
    }

    fn goal_of(t: usize) -> Goal {
        Goal {
            vertices: vec![(0, t)],
            pour_sites: Vec::new(),
            pour_layer: None,
        }
    }

    /// Planar slot search for `conn` from vertex `start_v` to `goal_v`. A blocked search runs
    /// [`ghost`] under the same deadline and reports its path and first blocked edge.
    /// Complexity `O(V log V)`, `V = 2 (E + X)`.
    pub fn search(
        g: &RouteGraph,
        obs: &LayerObstacles,
        conn: &Connection,
        start_v: usize,
        goal_v: usize,
        p: &SearchParams,
    ) -> SearchOutcome {
        let env = Env {
            layers: vec![(g, obs)],
            mg: None,
        };
        let goal = goal_of(goal_v);
        let start = [(0usize, start_v)];
        match search_in(&env, &[p.history], conn, &start, &goal, &common(p)) {
            super::SearchOutcome::Found(path) => SearchOutcome::Found(PlanarPath::of(path)),
            super::SearchOutcome::Timeout => SearchOutcome::Timeout,
            super::SearchOutcome::Blocked {
                ghost,
                blocked_edge,
                last_reached,
                ..
            } => {
                let last = ghost
                    .as_ref()
                    .zip(last_reached)
                    .map(|(gp, (leg, k))| gp.legs[leg].crossings[k]);
                SearchOutcome::Blocked {
                    ghost: ghost.map(PlanarPath::of),
                    blocked_edge: blocked_edge.map(|x| x.1),
                    last_reached: last,
                }
            }
        }
    }

    /// Topological ghost search (see [`super::ghost`]) on one layer. Returns `(path, first
    /// blocked edge, crossing before it)`; ghost crossings carry the slot the slot search would
    /// choose (the append position `slot_count(edge)` where none is feasible).
    #[allow(clippy::type_complexity)]
    pub fn ghost(
        g: &RouteGraph,
        obs: &LayerObstacles,
        conn: &Connection,
        start_v: usize,
        goal_v: usize,
        p: &SearchParams,
    ) -> Option<(PlanarPath, Option<usize>, Option<(usize, usize)>)> {
        let env = Env {
            layers: vec![(g, obs)],
            mg: None,
        };
        let goal = goal_of(goal_v);
        let start = [(0usize, start_v)];
        let (path, edge, _, last) = ghost_in(&env, &[p.history], conn, &start, &goal, &common(p))?;
        let last = last.map(|(leg, k)| path.legs[leg].crossings[k]);
        Some((PlanarPath::of(path), edge.map(|x| x.1), last))
    }

    /// Connections whose strands (Rule T) block the slot-free `ghost` path of a connection from
    /// `start_v` to `goal_v` ([`leg_strand_blockers`]). Sorted, deduplicated.
    pub fn strand_blockers(
        g: &RouteGraph,
        ghost: &PlanarPath,
        start_v: usize,
        goal_v: usize,
    ) -> Vec<usize> {
        leg_strand_blockers(
            g,
            &Leg {
                layer_idx: 0,
                start: start_v,
                goal: goal_v,
                crossings: ghost.crossings.clone(),
                faces: ghost.faces.clone(),
                est_len: ghost.est_len,
            },
        )
    }
}
