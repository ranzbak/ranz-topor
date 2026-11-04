//! P2-lite initial routing in one plane (plan "Pipeline", P2): pad-to-pad connections routed
//! one at a time, widest first then shortest, by A* over the gap graph of the projection
//! (node = CDT edge + gap between its passes); crossings are allowed and priced. Afterwards
//! double crossings are removed by swapping the stretch between them.
//!
//! Complexity: one search is O(S log S) in the gap-graph states S it reaches (bounded by
//! `max_expansions`); `uncross_all` is O(rounds · Σ crossings²) per wire, small in practice.

use super::demand::Item;
use super::state::{interleave, EdgeId, ObsId, State, TriId, VertId, Wire, WireId};
use crate::geom::Vec2;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

const MM: f64 = 1e6;

/// A searched path: the gaps `(edge, g)` it takes and the triangles it traverses.
pub type Path = (Vec<(EdgeId, usize)>, Vec<TriId>);

/// P2 cost weights (mm; plan "Default weights").
#[derive(Clone, Copy, Debug)]
pub struct InitWeights {
    /// Per different-net crossing created.
    pub cross_mm: f64,
    /// Per same-net crossing (electrically harmless).
    pub same_net_cross_mm: f64,
    /// Per triangle entered over another net's SMD pad (it fixes the layer: up to one via).
    pub smd_over_mm: f64,
    /// Per pass through an edge whose room is exceeded.
    pub overflow_mm: f64,
    /// Per pass through a gap too narrow for this wire on some layer (fixes the layer, as above).
    pub forced_layer_mm: f64,
    /// Search cap per connection.
    pub max_expansions: usize,
}

impl Default for InitWeights {
    fn default() -> Self {
        InitWeights {
            cross_mm: 20.0,
            same_net_cross_mm: 0.0,
            smd_over_mm: 25.0,
            overflow_mm: 50.0,
            forced_layer_mm: 25.0,
            max_expansions: 2_000_000,
        }
    }
}

/// One pad-to-pad connection.
#[derive(Clone, Copy, Debug)]
pub struct Conn {
    pub net: i32,
    pub pa: ObsId,
    pub pb: ObsId,
    pub a: VertId,
    pub b: VertId,
    pub w: f64,
    pub s: f64,
}

#[derive(Clone, Debug, Default)]
pub struct InitReport {
    pub routed: usize,
    pub failed: Vec<usize>,
    /// Passes inserted through an edge already over its room.
    pub overflows: usize,
    pub crossings_before: usize,
    pub crossings_after: usize,
    pub swaps: usize,
}

#[derive(Clone, Copy, PartialEq)]
struct Key(f64, u64, usize);
impl Eq for Key {}
impl PartialOrd for Key {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Key {
    fn cmp(&self, o: &Self) -> Ordering {
        // Min-heap on cost, then FIFO on the sequence number.
        o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
    }
}

struct Node {
    e: EdgeId,
    g: usize,
    into: TriId,
    from_tri: TriId,
    parent: usize,
    cost: f64,
    pt: Vec2,
}

/// Endpoint of an arc inside a triangle: a corner or a gap.
#[derive(Clone, Copy)]
enum End {
    Corner(VertId),
    Gap(EdgeId, usize),
}

struct Search<'s, 'p> {
    st: &'s State<'p>,
    c: Conn,
    wt: InitWeights,
    scale: f64,
    nodes: Vec<Node>,
    best: HashMap<(EdgeId, usize, TriId), usize>,
    heap: BinaryHeap<Key>,
    seq: u64,
    /// (cost, parent node or MAX, last triangle)
    goal: Option<(f64, usize, TriId)>,
}

impl Search<'_, '_> {
    fn h(&self, p: Vec2) -> f64 {
        (self.st.proj.pos[self.c.b] - p).len() / MM * self.scale
    }

    fn push(&mut self, n: Node) {
        let key = (n.e, n.g, n.into);
        if let Some(&i) = self.best.get(&key) {
            if self.nodes[i].cost <= n.cost {
                return;
            }
        }
        let f = n.cost + self.h(n.pt);
        self.nodes.push(n);
        self.best.insert(key, self.nodes.len() - 1);
        self.seq += 1;
        self.heap.push(Key(f, self.seq, self.nodes.len() - 1));
    }

    /// Arcs from `x` (point `px`, cost so far `base`) through triangle `t`.
    fn expand(&mut self, t: TriId, x: End, in_e: Option<EdgeId>, px: Vec2, base: f64, parent: usize) {
        let (st, c) = (self.st, self.c);
        let pr = st.proj;
        if pr.tris[t].contains(&c.b) {
            let cost = base
                + (pr.pos[c.b] - px).len() / MM * self.scale
                + st.arc_cross(t, x, End::Corner(c.b), c.net, &self.wt);
            if self.goal.is_none_or(|g| cost < g.0) {
                self.goal = Some((cost, parent, t));
            }
        }
        for &e in &pr.tri_edges[t] {
            // From a corner, only the opposite side; from a gap, not back.
            let skip = match x {
                End::Corner(v) => pr.edges[e].contains(&v),
                End::Gap(..) => Some(e) == in_e,
            };
            if skip {
                continue;
            }
            let Some(nt) = pr.across(e, t) else { continue };
            if pr.allowed(nt, c.net).is_none() {
                continue;
            }
            let me = Item { net: c.net, w: c.w, s: c.s };
            let alone = pr.edge_demand(e, &[me]);
            let extra = st.over_cost(nt, c.net, &self.wt)
                + if (0..pr.n_layers).any(|l| alone > pr.room_for(e, l, c.net)) { self.wt.forced_layer_mm } else { 0.0 };
            let room = st.room_across(e, t, nt, c.net);
            for g in 0..=st.edge_order[e].len() {
                let pt = st.gap_point(e, g);
                let cost = base
                    + (pt - px).len() / MM * self.scale
                    + st.arc_cross(t, x, End::Gap(e, g), c.net, &self.wt)
                    + if room.is_some_and(|r| r.is_finite() && (if st.edge_order[e].is_empty() { alone } else { st.edge_need(e, Some((g, me))) }) > r) { self.wt.overflow_mm } else { 0.0 }
                    + extra;
                self.push(Node { e, g, into: nt, from_tri: t, parent, cost, pt });
            }
        }
    }
}

impl<'p> State<'p> {
    fn end_bpos(&self, t: TriId, x: End) -> f64 {
        match x {
            End::Corner(v) => self.proj.corner_pos(t, v),
            End::Gap(e, g) => self.gap_bpos(t, e, g),
        }
    }

    /// Crossing cost of a new chord `x -> y` in `t` for a wire of `net`.
    fn arc_cross(&self, t: TriId, x: End, y: End, net: i32, wt: &InitWeights) -> f64 {
        let (a1, a2) = (self.end_bpos(t, x), self.end_bpos(t, y));
        let mut c = 0.0;
        for &(o, ok) in &self.tri_trav[t] {
            for (b1, b2) in self.chords(o, ok) {
                if interleave(a1, a2, b1, b2) {
                    c += if self.wires[o].net == net { wt.same_net_cross_mm } else { wt.cross_mm };
                }
            }
        }
        c
    }

    fn over_cost(&self, t: TriId, net: i32, wt: &InitWeights) -> f64 {
        let over = self.proj.tri_obs[t].iter().any(|&o| self.proj.obstacles[o].net != Some(net));
        if over {
            wt.smd_over_mm
        } else {
            0.0
        }
    }

    /// Room of edge `e` for a wire of `net` allowed on the layers of both triangles: the sum over
    /// the layers they share (the layers are not assigned yet); `None` when a triangle is a wall.
    pub fn room_across(&self, e: EdgeId, t1: TriId, t2: TriId, net: i32) -> Option<f64> {
        let (m1, m2) = (self.proj.allowed(t1, net)?, self.proj.allowed(t2, net)?);
        let m = m1.0 & m2.0;
        Some((0..self.proj.n_layers).filter(|&l| (m >> l) & 1 == 1).map(|l| self.proj.room_for(e, l, net)).sum())
    }

    /// Whether one more pass (width `w`, clearance `s`) inserted at gap `g` overflows edge `e`
    /// for a wire allowed on the layers of both triangles. The demand is that of one layer
    /// ([`State::edge_need`]), the room the sum over the allowed layers.
    #[allow(clippy::too_many_arguments)]
    pub fn overflows(&self, e: EdgeId, g: usize, t1: TriId, t2: TriId, net: i32, w: f64, s: f64) -> bool {
        self.room_across(e, t1, t2, net).is_some_and(|room| self.edge_need(e, Some((g, Item { net, w, s }))) > room)
    }

    /// Cheapest path of `c` in the current state: the gaps `(edge, g)` and the triangles.
    /// `scale` = (w+s)/(w₀+s₀), the length factor of this wire.
    pub fn search(&self, c: &Conn, wt: &InitWeights, scale: f64) -> Option<Path> {
        let pr = self.proj;
        let mut s = Search {
            st: self,
            c: *c,
            wt: *wt,
            scale,
            nodes: Vec::new(),
            best: HashMap::new(),
            heap: BinaryHeap::new(),
            seq: 0,
            goal: None,
        };
        for &t in &pr.vert_tris[c.a] {
            if pr.allowed(t, c.net).is_some() {
                s.expand(t, End::Corner(c.a), None, pr.pos[c.a], 0.0, usize::MAX);
            }
        }
        let mut expansions = 0;
        while let Some(Key(f, _, i)) = s.heap.pop() {
            if s.goal.is_some_and(|g| g.0 <= f) {
                break;
            }
            let n = &s.nodes[i];
            if s.best.get(&(n.e, n.g, n.into)) != Some(&i) {
                continue;
            }
            expansions += 1;
            if expansions > wt.max_expansions {
                break;
            }
            let (t, e, g, pt, cost) = (n.into, n.e, n.g, n.pt, n.cost);
            s.expand(t, End::Gap(e, g), Some(e), pt, cost, i);
        }
        let (_, mut i, last) = s.goal?;
        let mut path = Vec::new();
        let mut tris = vec![last];
        while i != usize::MAX {
            let n = &s.nodes[i];
            path.push((n.e, n.g));
            tris.push(n.from_tri);
            i = n.parent;
        }
        path.reverse();
        tris.reverse();
        Some((path, tris))
    }

    /// Route `c` and insert it: the wire and the number of passes over room; `None` when no
    /// path exists.
    pub fn route_conn(&mut self, c: &Conn, wt: &InitWeights, scale: f64) -> Option<(WireId, usize)> {
        let (path, tris) = self.search(c, wt, scale)?;
        let over = (0..path.len())
            .filter(|&j| self.overflows(path[j].0, path[j].1, tris[j], tris[j + 1], c.net, c.w, c.s))
            .count();
        let wire = Wire {
            net: c.net,
            w: c.w,
            s: c.s,
            a: c.a,
            b: c.b,
            pa: c.pa,
            pb: c.pb,
            turns: Vec::new(),
            passes: Vec::new(),
            tris: Vec::new(),
            alive: true,
        };
        Some((self.insert(wire, &path, tris), over))
    }

    /// Remove double crossings (two crossings of the same pair of different-net wires whose
    /// stretches between them run through the same edges) until none is left; a swap is kept
    /// only when it lowers the different-net crossing count of the triangles it touches.
    pub fn uncross_all(&mut self, max_rounds: usize) -> usize {
        let mut swaps = 0;
        for _ in 0..max_rounds {
            let mut changed = false;
            for a in 0..self.wires.len() {
                if !self.wires[a].alive {
                    continue;
                }
                let xs = self.crossings(a);
                'pairs: for i in 0..xs.len() {
                    for j in i + 1..xs.len() {
                        let (x, y) = (xs[i], xs[j]);
                        if x.other != y.other || self.wires[x.other].net == self.wires[a].net {
                            continue;
                        }
                        let Some(pairs) =
                            self.double_stretch(a, (x.k, y.k), x.other, (x.other_k, y.other_k))
                        else {
                            continue;
                        };
                        let touched: Vec<TriId> = self.wires[a].tris[x.k..=y.k].to_vec();
                        let before: usize = touched.iter().map(|&t| self.tri_crossings(t)).sum();
                        self.uncross_double(&pairs);
                        let after: usize = touched.iter().map(|&t| self.tri_crossings(t)).sum();
                        if after < before {
                            swaps += 1;
                            changed = true;
                            break 'pairs;
                        }
                        self.uncross_double(&pairs);
                    }
                }
            }
            if !changed {
                break;
            }
        }
        swaps
    }
}

/// Route all connections (widest first, then shortest), then remove double crossings.
/// `ws0` = w₀ + s₀ of the default net class (the length unit).
pub fn route_all(st: &mut State, conns: &[Conn], wt: &InitWeights, ws0: f64) -> InitReport {
    let len = |c: &Conn| (st.proj.pos[c.b] - st.proj.pos[c.a]).len();
    let mut order: Vec<usize> = (0..conns.len()).collect();
    order.sort_by(|&i, &j| {
        conns[j].w.total_cmp(&conns[i].w)
            .then(len(&conns[i]).total_cmp(&len(&conns[j])))
            .then(i.cmp(&j))
    });
    let mut rep = InitReport::default();
    for i in order {
        let c = &conns[i];
        match st.route_conn(c, wt, (c.w + c.s) / ws0) {
            Some((_, over)) => {
                rep.routed += 1;
                rep.overflows += over;
            }
            None => rep.failed.push(i),
        }
    }
    rep.crossings_before = st.total_crossings();
    rep.swaps = st.uncross_all(50);
    rep.crossings_after = st.total_crossings();
    rep
}
