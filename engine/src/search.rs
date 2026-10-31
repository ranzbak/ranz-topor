//! Planar slot A* per connection with a Lazy-Theta*-style length estimate, and the slot-free
//! ghost search (M2 / T2.3a; design note `docs/progress/notes/m2-search-design.md` §1-3).
//!
//! State `(edge, slot, dir)`: the new wire crosses `edge` at insertion position `slot` and enters
//! the face `edges[edge].faces[dir]`. Transitions follow Rule T (no two strands cross inside a
//! face; port order is the cyclic order of the face boundary). Cost is the string-pulled length
//! (apex shortcut, <= `LOS_PORTAL_CAP` portals per line-of-sight) scaled by history / present
//! congestion; `h` is the Euclidean distance to the goal. Complexity: O(V log V) with
//! `V = 2(E + X)` states, each expansion O(`n_max` + `LOS_PORTAL_CAP`^2); expansions are capped at
//! `8 (E + X) + 64`. Only lookups touch hash containers; ties are broken by the heap key.

use crate::decompose::Connection;
use crate::geom::{point_seg, seg_arc, seg_seg, Aabb, Seg, Vec2};
use crate::graph::{Port, RouteGraph};
use crate::obstacle::{Core, LayerObstacles, ObsKind, ObstacleSet};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::time::Instant;

/// Maximum portals one line-of-sight test walks (note §2, `C`).
pub const LOS_PORTAL_CAP: usize = 32;
/// Ghost penalty for crossing a blocked edge, nm (10 m).
pub const GHOST_BLOCK_NM: f64 = 1e10;
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

/// Expansion cap `8 (E + X) + 64` of the slot search.
pub fn default_expansion_cap(g: &RouteGraph) -> usize {
    let x: usize = (0..g.edges.len()).map(|e| g.slot_count(e)).sum();
    8 * (g.edges.len() + x) + 64
}

#[derive(Clone, Debug)]
pub struct SlotPath {
    pub conn: usize,
    /// `(edge, slot)` in travel order, slots as seen by the search (the graph before the insert).
    pub crossings: Vec<(usize, usize)>,
    /// Start face first; `faces.len() == crossings.len() + 1` (empty for a direct path).
    pub faces: Vec<usize>,
    pub est_len: f64,
    pub expansions: usize,
}

#[derive(Clone, Debug)]
pub enum SearchOutcome {
    Found(SlotPath),
    Blocked {
        ghost: Option<SlotPath>,
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

#[derive(Clone, Debug)]
struct Node {
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
    /// State point on `edge`.
    p: Vec2,
    g_len: f64,
    g_cost: f64,
}

#[derive(Clone, Copy, Debug)]
struct Item {
    f: f64,
    h: f64,
    edge: usize,
    slot: usize,
    dir: u8,
    id: usize,
}

impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.total_cmp(&self.f)
            .then(o.h.total_cmp(&self.h))
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
    best: HashMap<(usize, usize, u8), f64>,
    closed: HashSet<(usize, usize, u8)>,
}

enum Run {
    Found(Box<SlotPath>),
    Blocked,
    Timeout,
}

struct Search<'a> {
    g: &'a RouteGraph,
    obs: &'a LayerObstacles,
    conn: &'a Connection,
    net: i32,
    w: f64,
    c: f64,
    eps: f64,
    start_v: usize,
    goal_v: usize,
    goal: Vec2,
    hist: &'a [f64],
    omega: f64,
    present_overuse: f64,
    ghost: bool,
}

impl<'a> Search<'a> {
    fn new(
        g: &'a RouteGraph,
        obs: &'a LayerObstacles,
        conn: &'a Connection,
        start_v: usize,
        goal_v: usize,
        p: &SearchParams<'a>,
        ghost: bool,
    ) -> Self {
        Search {
            g,
            obs,
            conn,
            net: conn.net,
            w: conn.width as f64,
            c: conn.clearance as f64,
            eps: g.rules.eps,
            start_v,
            goal_v,
            goal: g.verts[goal_v].at,
            hist: p.history,
            omega: p.weight,
            present_overuse: p.present_overuse,
            ghost,
        }
    }

    /// Distance from a vertex a wire's centreline must keep along an edge end.
    fn margin_of(&self, v: usize) -> f64 {
        let vi = &self.g.verts[v];
        if vi.owner.is_none() {
            0.0
        } else if vi.nets.contains(&self.net) {
            vi.r
        } else {
            vi.r + vi.clr.max(self.c) + 0.5 * self.w + self.eps
        }
    }

    /// State point of edge `e`: usable sub-segment point nearest to the segment `apex -> goal`.
    fn state_point(&self, e: usize, apex: Vec2) -> Vec2 {
        let ge = &self.g.edges[e];
        let (pa, pb) = (self.g.verts[ge.a].at, self.g.verts[ge.b].at);
        let len = dist(pa, pb);
        if len <= 0.0 {
            return pa;
        }
        let (ra, rb) = (self.margin_of(ge.a), self.margin_of(ge.b));
        let (t0, t1) = if ra + rb >= len {
            let t = ra / (ra + rb).max(1e-9);
            (t, t)
        } else {
            (ra / len, 1.0 - rb / len)
        };
        let d = pb - pa;
        seg_closest(pa + d * t0, pa + d * t1, apex, self.goal)
    }

    fn hard(&self, e: usize) -> bool {
        let ge = &self.g.edges[e];
        ge.along.is_some()
            || !ge
                .faces
                .iter()
                .all(|f| f.is_some_and(|f| self.g.faces[f].inside))
    }

    fn soft_blocked(&self, e: usize) -> bool {
        !self.g.passable(e, self.net) || !self.g.fits(e, self.w, self.c)
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

    /// First obstacle (along the chord) closer than its required distance: the bend point.
    fn clearance_bend(&self, a: Vec2, t: Vec2) -> Option<Vec2> {
        const TOL: f64 = 1.0;
        let chord = Seg { a, b: t };
        let r = t - a;
        let l2 = r.len2().max(1.0);
        let bb = Aabb {
            min: Vec2 {
                x: a.x.min(t.x),
                y: a.y.min(t.y),
            },
            max: Vec2 {
                x: a.x.max(t.x),
                y: a.y.max(t.y),
            },
        };
        let rho = self.g.rho_max() + self.c + 0.5 * self.w + self.eps;
        let mut best: Option<(f64, Vec2)> = None;
        let mut offer = |param: f64, p: Vec2| {
            if best.is_none_or(|(bp, _)| param < bp) {
                best = Some((param, p));
            }
        };
        for u in self.g.verts_near(bb.grow(rho)) {
            let vi = &self.g.verts[u];
            if vi.owner.is_none() || vi.nets.contains(&self.net) || vi.at == a {
                continue;
            }
            let need = vi.r + vi.clr.max(self.c) + 0.5 * self.w + self.eps;
            if point_seg(vi.at, &chord) < need - TOL {
                offer((vi.at - a).dot(r) / l2, vi.at);
            }
        }
        let wr = self
            .g
            .rho_max()
            .max(self.g.rules.edge_clearance)
            .max(self.g.rules.hole_clearance)
            + self.c
            + 0.5 * self.w
            + self.eps;
        for wi in self.obs.query_walls(bb.grow(wr)) {
            let wl = &self.obs.walls[wi];
            if ObstacleSet::exempt(wl.net, &wl.tie_nets, self.net) {
                continue;
            }
            let need =
                wl.r_core + self.wall_clearance(wl.kind, wl.own_clr) + 0.5 * self.w + self.eps;
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
                continue;
            }
            offer((bend - a).dot(r) / l2, bend);
        }
        best.map(|(_, p)| p)
    }

    /// Line of sight from the apex of node `s` to `target` through the portals pushed since:
    /// returns `(apex, apex_len, portals_kept)`.
    fn los(&self, nodes: &[Node], s: usize, target: Vec2) -> (Vec2, f64, usize) {
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
        if (target - apex).len2() < 1.0 {
            return (apex, len, portals.len());
        }
        for _ in 0..LOS_PORTAL_CAP + 4 {
            let chord = Seg { a: apex, b: target };
            let mut miss = None;
            for (k, &pe) in portals.iter().enumerate() {
                let ge = &self.g.edges[pe];
                let (pa, pb) = (self.g.verts[ge.a].at, self.g.verts[ge.b].at);
                if pa == apex || pb == apex {
                    continue;
                }
                if !crosses(apex, target, pa, pb) {
                    let bend = if point_seg(pa, &chord) <= point_seg(pb, &chord) {
                        pa
                    } else {
                        pb
                    };
                    miss = Some((k, bend));
                    break;
                }
            }
            if let Some((k, bend)) = miss {
                len += dist(apex, bend);
                apex = bend;
                portals.drain(..=k);
                continue;
            }
            if let Some(bend) = self.clearance_bend(apex, target) {
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
            break;
        }
        (apex, len, portals.len())
    }

    fn step_cost(&self, e: usize, delta: f64) -> f64 {
        if self.ghost {
            return delta
                + if self.soft_blocked(e) {
                    GHOST_BLOCK_NM
                } else {
                    0.0
                };
        }
        let hist = self.hist.get(e).copied().unwrap_or(0.0);
        let present = if self.g.fits(e, self.w, self.c) {
            1.0
        } else {
            let cap = self.g.edges[e].cap.max(1.0);
            self.present_overuse * (1.0 + self.g.overuse(e) / cap)
        };
        delta * (1.0 + self.omega * hist) * (1.0 + self.omega * (present - 1.0))
    }

    fn push_state(
        &self,
        st: &mut State,
        parent: usize,
        e: usize,
        slot: usize,
        dir: u8,
        fin: usize,
    ) {
        let key = (e, if self.ghost { 0 } else { slot }, dir);
        if st.closed.contains(&key) {
            return;
        }
        let s = &st.nodes[parent];
        let p = self.state_point(e, s.apex);
        let (apex, apex_len, kept) = self.los(&st.nodes, parent, p);
        let g_len = apex_len + dist(apex, p);
        let delta = (g_len - s.g_len).max(0.0);
        let g_cost = s.g_cost + self.step_cost(e, delta);
        if st.best.get(&key).is_some_and(|&b| b <= g_cost) {
            return;
        }
        st.best.insert(key, g_cost);
        let h = dist(p, self.goal);
        let id = st.nodes.len();
        st.nodes.push(Node {
            edge: e,
            slot,
            dir,
            parent,
            face_in: fin,
            apex,
            apex_len,
            since: kept + 1,
            p,
            g_len,
            g_cost,
        });
        st.heap.push(Item {
            f: g_cost + h,
            h,
            edge: e,
            slot,
            dir,
            id,
        });
    }

    fn push_goal(&self, st: &mut State, s: usize) {
        let (apex, apex_len, _) = self.los(&st.nodes, s, self.goal);
        let sn = &st.nodes[s];
        let g_len = apex_len + dist(apex, self.goal);
        let g_cost = sn.g_cost + (g_len - sn.g_len).max(0.0);
        let id = st.nodes.len();
        st.nodes.push(Node {
            edge: usize::MAX,
            slot: 0,
            dir: 0,
            parent: s,
            face_in: 0,
            apex,
            apex_len,
            since: 0,
            p: self.goal,
            g_len,
            g_cost,
        });
        st.heap.push(Item {
            f: g_cost,
            h: 0.0,
            edge: usize::MAX,
            slot: 0,
            dir: 0,
            id,
        });
    }

    fn other_dir(&self, e: usize, f: usize) -> u8 {
        u8::from(self.g.edges[e].faces[0] == Some(f))
    }

    fn finish(&self, st: &State, goal_id: usize, expansions: usize) -> SlotPath {
        let mut chain = Vec::new();
        let mut id = st.nodes[goal_id].parent;
        while id != 0 {
            chain.push(id);
            id = st.nodes[id].parent;
        }
        chain.reverse();
        let mut faces = Vec::with_capacity(chain.len() + 1);
        if let Some(&first) = chain.first() {
            faces.push(st.nodes[first].face_in);
        }
        for &i in &chain {
            let n = &st.nodes[i];
            faces.push(self.g.edges[n.edge].faces[n.dir as usize].unwrap_or(usize::MAX));
        }
        SlotPath {
            conn: self.conn.id,
            crossings: chain
                .iter()
                .map(|&i| (st.nodes[i].edge, st.nodes[i].slot))
                .collect(),
            faces,
            est_len: st.nodes[goal_id].g_len,
            expansions,
        }
    }

    fn run(&self, cap: usize, deadline: Instant) -> Run {
        let g = self.g;
        let at_p = g.verts[self.start_v].at;
        let mut st = State::default();
        st.nodes.push(Node {
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
        });
        // start states: leave every face around the start vertex through each edge
        for &f in g.faces_of_vertex(self.start_v) {
            if !g.faces[f].inside {
                continue;
            }
            let cyc = Cyc::new(g, f);
            let strands = if self.ghost {
                Vec::new()
            } else {
                cyc.strands(g)
            };
            let p0 = cyc.vertex_pos(self.start_v).unwrap_or(0);
            for &e_out in &g.faces[f].e {
                let dir = self.other_dir(e_out, f);
                if self.ghost {
                    if !self.hard(e_out) {
                        self.push_state(&mut st, 0, e_out, g.slot_count(e_out), dir, f);
                    }
                    continue;
                }
                if !g.passable(e_out, self.net) {
                    continue;
                }
                let ge = &g.edges[e_out];
                let near_v = if ge.a == self.start_v || ge.b == self.start_v {
                    self.start_v
                } else {
                    ge.a
                };
                if let Some(slot) = cyc.cross_slot(&strands, p0, e_out, near_v) {
                    self.push_state(&mut st, 0, e_out, slot, dir, f);
                }
            }
        }
        let mut expansions = 0usize;
        while let Some(it) = st.heap.pop() {
            if it.edge == usize::MAX {
                return Run::Found(Box::new(self.finish(&st, it.id, expansions)));
            }
            let key = (it.edge, if self.ghost { 0 } else { it.slot }, it.dir);
            if !st.closed.insert(key) {
                continue;
            }
            if expansions >= cap {
                return Run::Blocked;
            }
            expansions += 1;
            if expansions.is_multiple_of(16) && Instant::now() >= deadline {
                return Run::Timeout;
            }
            let (e, slot, dir) = (it.edge, it.slot, it.dir);
            let Some(f) = g.edges[e].faces[dir as usize] else {
                continue;
            };
            let cyc = Cyc::new(g, f);
            let strands = if self.ghost {
                Vec::new()
            } else {
                cyc.strands(g)
            };
            let p0 = if self.ghost {
                0
            } else {
                cyc.gap_pos(e, slot).unwrap_or(0)
            };
            for &e_out in &g.faces[f].e {
                if e_out == e {
                    continue;
                }
                let nd = self.other_dir(e_out, f);
                if self.ghost {
                    if !self.hard(e_out) {
                        self.push_state(&mut st, it.id, e_out, g.slot_count(e_out), nd, f);
                    }
                    continue;
                }
                if !g.passable(e_out, self.net) {
                    continue;
                }
                let (x, y) = (&g.edges[e], &g.edges[e_out]);
                let Some(v) = [x.a, x.b].into_iter().find(|&v| v == y.a || v == y.b) else {
                    continue;
                };
                if let Some(s2) = cyc.cross_slot(&strands, p0, e_out, v) {
                    self.push_state(&mut st, it.id, e_out, s2, nd, f);
                }
            }
            if g.faces[f].v.contains(&self.goal_v)
                && (self.ghost || cyc.goal_ok(&strands, p0, self.goal_v))
            {
                self.push_goal(&mut st, it.id);
            }
        }
        Run::Blocked
    }
}

/// Zero-crossing path along the edge between the terminals, if free.
fn direct(g: &RouteGraph, conn: &Connection, s: usize, t: usize) -> Option<SlotPath> {
    let e = g.edge(s, t)?;
    if g.passable(e, conn.net) && g.slot_count(e) == 0 && g.along(e).is_none() {
        return Some(SlotPath {
            conn: conn.id,
            crossings: Vec::new(),
            faces: Vec::new(),
            est_len: dist(g.verts[s].at, g.verts[t].at),
            expansions: 0,
        });
    }
    None
}

/// Planar slot search for `conn` from vertex `start_v` to `goal_v`. A blocked search runs
/// [`ghost`] under the same deadline and reports its path and first blocked edge.
pub fn search(
    g: &RouteGraph,
    obs: &LayerObstacles,
    conn: &Connection,
    start_v: usize,
    goal_v: usize,
    p: &SearchParams,
) -> SearchOutcome {
    if Instant::now() >= p.deadline {
        return SearchOutcome::Timeout;
    }
    if let Some(d) = direct(g, conn, start_v, goal_v) {
        return SearchOutcome::Found(d);
    }
    let cap = p.expansion_cap.min(default_expansion_cap(g));
    let s = Search::new(g, obs, conn, start_v, goal_v, p, false);
    match s.run(cap, p.deadline) {
        Run::Found(path) => SearchOutcome::Found(*path),
        Run::Timeout => SearchOutcome::Timeout,
        Run::Blocked => {
            let gh = ghost(g, obs, conn, start_v, goal_v, p);
            match gh {
                Some((path, blocked_edge, last_reached)) => SearchOutcome::Blocked {
                    ghost: Some(path),
                    blocked_edge,
                    last_reached,
                },
                None => SearchOutcome::Blocked {
                    ghost: None,
                    blocked_edge: None,
                    last_reached: None,
                },
            }
        }
    }
}

/// Slot-free ghost search: ignores capacity and foreign constraint edges at a price of
/// [`GHOST_BLOCK_NM`] per blocked edge. Returns `(path, first blocked edge, crossing before it)`;
/// ghost crossings carry `slot = slot_count(edge)` (append position).
#[allow(clippy::type_complexity)]
pub fn ghost(
    g: &RouteGraph,
    obs: &LayerObstacles,
    conn: &Connection,
    start_v: usize,
    goal_v: usize,
    p: &SearchParams,
) -> Option<(SlotPath, Option<usize>, Option<(usize, usize)>)> {
    let s = Search::new(g, obs, conn, start_v, goal_v, p, true);
    if let Some(e) = g.edge(start_v, goal_v) {
        if !s.hard(e) {
            let path = SlotPath {
                conn: conn.id,
                crossings: Vec::new(),
                faces: Vec::new(),
                est_len: dist(g.verts[start_v].at, g.verts[goal_v].at),
                expansions: 0,
            };
            return Some((path, None, None));
        }
    }
    let cap = 8 * g.edges.len() + 64;
    let Run::Found(mut path) = s.run(cap, p.deadline) else {
        return None;
    };
    for c in &mut path.crossings {
        c.1 = g.slot_count(c.0);
    }
    let k = path.crossings.iter().position(|&(e, _)| s.soft_blocked(e));
    let blocked_edge = k.map(|k| path.crossings[k].0);
    let last_reached = k.and_then(|k| k.checked_sub(1)).map(|k| path.crossings[k]);
    Some((*path, blocked_edge, last_reached))
}
