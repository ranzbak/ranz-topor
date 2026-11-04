//! P2-lite initial routing in one plane (plan "Pipeline", P2): pad-to-pad connections routed
//! one at a time, widest first then shortest, by A* over the gap graph of the projection
//! (node = CDT edge + gap between its passes); crossings are allowed and priced. Afterwards
//! double crossings are removed by swapping the stretch between them.
//!
//! Complexity: one search is O(S log S) in the gap-graph states S it reaches (bounded by
//! `max_expansions`); `uncross_all` is O(rounds · Σ crossings²) per wire, small in practice.

use super::demand::Item;
use super::factors::{Event, Registry, WireCtx};
use super::state::{interleave, EdgeId, ObsId, State, TriId, VertId, Wire, WireId};
use crate::geom::Vec2;
use std::cmp::Ordering;
use std::collections::BinaryHeap;

const MM: f64 = 1e6;

/// A searched path: the gaps `(edge, g)` it takes and the triangles it traverses.
pub type Path = (Vec<(EdgeId, usize)>, Vec<TriId>);

/// P2 costs (mm) of one wire class, resolved from the factor registry (nothing is hard-coded
/// here: plan "Default weights" live in `topo/factors`).
#[derive(Clone, Copy, Debug)]
pub struct InitWeights {
    /// Per mm of centre line, before the wire's (w+s)/(w0+s0) scale (`length`).
    pub length_mm: f64,
    /// Per different-net crossing created (`crossing`).
    pub cross_mm: f64,
    /// Per same-net crossing (`same_net_crossing`, electrically harmless).
    pub same_net_cross_mm: f64,
    /// Per triangle entered over another net's SMD pad (it fixes the layer: up to one `via`).
    pub smd_over_mm: f64,
    /// Per pass through an edge whose room is exceeded (`overflow`).
    pub overflow_mm: f64,
    /// Per pass through a gap too narrow for this wire on some layer (fixes the layer, as above).
    pub forced_layer_mm: f64,
    /// Search cap per connection.
    pub max_expansions: usize,
}

impl InitWeights {
    /// The prices `reg` puts on the events P2 creates, for a wire of class `w`.
    pub fn from_registry(reg: &Registry, w: &WireCtx) -> InitWeights {
        let forced = reg.price(&Event::LayerForced { wire: 0, edge: 0 }, w);
        InitWeights {
            length_mm: reg.price(&Event::Length { wire: 0, mm: 1.0, scale: 1.0 }, w),
            cross_mm: reg.price(&Event::Crossing { a: 0, b: 1, tri: 0, same_net: false }, w),
            same_net_cross_mm: reg.price(&Event::Crossing { a: 0, b: 1, tri: 0, same_net: true }, w),
            smd_over_mm: forced,
            overflow_mm: reg.price(&Event::Overflow { wire: 0, edge: 0 }, w),
            forced_layer_mm: forced,
            max_expansions: 2_000_000,
        }
    }
}

impl Default for InitWeights {
    /// The standard factor set at its default weights.
    fn default() -> Self {
        InitWeights::from_registry(&Registry::standard(), &WireCtx::default())
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
struct Key(f64, f64, usize);
impl Eq for Key {}
impl PartialOrd for Key {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Key {
    fn cmp(&self, o: &Self) -> Ordering {
        // Min-heap on f, then the smaller heuristic (nearer the goal: far fewer expansions on the
        // plateaus of equal f), then FIFO (the node index grows with the push order).
        o.0.total_cmp(&self.0).then(o.1.total_cmp(&self.1)).then(o.2.cmp(&self.2))
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
pub(crate) enum End {
    Corner(VertId),
    Gap(EdgeId, usize),
}

struct Search<'s, 'p> {
    st: &'s State<'p>,
    c: Conn,
    wt: InitWeights,
    scale: f64,
    nodes: Vec<Node>,
    /// Best node per (edge, gap, side of the edge entered): flat table, `off[e]` + 2 g + side.
    best: Vec<u32>,
    off: Vec<usize>,
    /// Chords `(a, b, net)` of the traversals of a triangle, computed on first use in this search.
    tc: Vec<Option<Vec<(f64, f64, i32)>>>,
    needs: Vec<f64>,
    items: Vec<Item>,
    heap: BinaryHeap<Key>,
    /// (cost, parent node or MAX, last triangle)
    goal: Option<(f64, usize, TriId)>,
}

impl Search<'_, '_> {
    fn h(&self, p: Vec2) -> f64 {
        (self.st.proj.pos[self.c.b] - p).len() / MM * self.scale * self.wt.length_mm
    }

    fn slot(&self, e: EdgeId, g: usize, into: TriId) -> usize {
        self.off[e] + 2 * g + usize::from(self.st.proj.edge_tris[e][0] != Some(into))
    }

    fn push(&mut self, n: Node) {
        let slot = self.slot(n.e, n.g, n.into);
        if self.best[slot] != u32::MAX && self.nodes[self.best[slot] as usize].cost <= n.cost {
            return;
        }
        let h = self.h(n.pt);
        let f = n.cost + h;
        // Behind the best goal found so far: never popped.
        if self.goal.is_some_and(|g| f >= g.0) {
            return;
        }
        self.nodes.push(n);
        self.best[slot] = (self.nodes.len() - 1) as u32;
        self.heap.push(Key(f, h, self.nodes.len() - 1));
    }

    /// Arcs from `x` (point `px`, cost so far `base`) through triangle `t`.
    fn expand(&mut self, t: TriId, x: End, in_e: Option<EdgeId>, px: Vec2, base: f64, parent: usize) {
        let (st, c) = (self.st, self.c);
        let pr = st.proj;
        if pr.tris[t].contains(&c.b) {
            let cost = base
                + (pr.pos[c.b] - px).len() / MM * self.scale * self.wt.length_mm
                + st.arc_cross(t, x, End::Corner(c.b), c.net, &self.wt);
            if self.goal.is_none_or(|g| cost < g.0) {
                self.goal = Some((cost, parent, t));
            }
        }
        // The chords of the traversals of `t` once per expansion, with whether they are of this net.
        if self.tc[t].is_none() {
            let v: Vec<(f64, f64, i32)> = st.tri_trav[t]
                .iter()
                .flat_map(|&(o, ok)| {
                    let net = st.wires[o].net;
                    st.chords(o, ok).into_iter().map(move |(b1, b2)| (b1, b2, net))
                })
                .collect();
            self.tc[t] = Some(v);
        }
        let xb = st.end_bpos(t, x);
        let mask_t = pr.allowed(t, c.net);
        let me = Item { net: c.net, w: c.w, s: c.s };
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
            let Some(mask_nt) = pr.allowed(nt, c.net) else { continue };
            let alone = pr.edge_demand(e, &[me]);
            let extra = st.over_cost(nt, c.net, &self.wt)
                + if (0..pr.n_layers).any(|l| alone > pr.room_for(e, l, c.net)) { self.wt.forced_layer_mm } else { 0.0 };
            let room = mask_t.map(|m| {
                let both = m.0 & mask_nt.0;
                (0..pr.n_layers).filter(|&l| (both >> l) & 1 == 1).map(|l| pr.room_for(e, l, c.net)).sum::<f64>()
            });
            self.needs.clear();
            if room.is_some_and(f64::is_finite) {
                st.edge_need_gaps(e, me, alone, &mut self.items, &mut self.needs);
            }
            // Per-edge geometry of the gaps (see `State::gap_point` / `gap_bpos`), hoisted.
            let n = st.edge_order[e].len();
            let [ea, eb] = pr.edges[e];
            let (pa, pb) = (pr.pos[ea], pr.pos[eb]);
            let side = pr.side(t, e);
            let fwd = pr.tris[t][side] == ea;
            for g in 0..=n {
                let pt = pa + (pb - pa) * ((g + 1) as f64 / (n + 2) as f64);
                let f = (2.0 * g as f64 + 1.0) / (2.0 * n as f64 + 2.0);
                let yb = side as f64 + if fwd { f } else { 1.0 - f };
                let cross: f64 = self.tc[t]
                    .as_ref()
                    .expect("filled above")
                    .iter()
                    .filter(|&&(b1, b2, _)| interleave(xb, yb, b1, b2))
                    .map(|&(_, _, net)| if net == c.net { self.wt.same_net_cross_mm } else { self.wt.cross_mm })
                    .sum();
                let cost = base
                    + (pt - px).len() / MM * self.scale * self.wt.length_mm
                    + cross
                    + if room.is_some_and(|r| r.is_finite() && self.needs[g] > r) { self.wt.overflow_mm } else { 0.0 }
                    + extra;
                self.push(Node { e, g, into: nt, from_tri: t, parent, cost, pt });
            }
        }
    }
}

impl<'p> State<'p> {
    pub(crate) fn end_bpos(&self, t: TriId, x: End) -> f64 {
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
        let mut off = Vec::with_capacity(pr.edges.len() + 1);
        let mut acc = 0;
        for e in 0..pr.edges.len() {
            off.push(acc);
            acc += 2 * (self.edge_order[e].len() + 1);
        }
        off.push(acc);
        let mut s = Search {
            st: self,
            c: *c,
            wt: *wt,
            scale,
            nodes: Vec::new(),
            best: vec![u32::MAX; off.last().copied().unwrap_or(0)],
            off: off[..off.len() - 1].to_vec(),
            tc: vec![None; pr.tris.len()],
            needs: Vec::new(),
            items: Vec::new(),
            heap: BinaryHeap::new(),
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
            if s.best[s.slot(n.e, n.g, n.into)] != i as u32 {
                continue;
            }
            expansions += 1;
            if expansions > wt.max_expansions {
                break;
            }
            let (t, e, g, pt, cost) = (n.into, n.e, n.g, n.pt, n.cost);
            s.expand(t, End::Gap(e, g), Some(e), pt, cost, i);
        }
        if std::env::var("P2_PROF").is_ok() {
            eprintln!("conn {}: expansions {expansions}, nodes {}, dist {:.1} mm, cost {:.1}, w {:.2}", c.net, s.nodes.len(), (self.proj.pos[c.b] - self.proj.pos[c.a]).len() / MM, s.goal.map_or(-1.0, |g| g.0), c.w / MM);
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

/// Route all connections (widest first, then shortest), then remove double crossings, with the
/// same prices for every wire. `ws0` = w0 + s0 of the default net class (the length unit).
pub fn route_all(st: &mut State, conns: &[Conn], wt: &InitWeights, ws0: f64) -> InitReport {
    route_all_with(st, conns, &|_| *wt, ws0)
}

/// [`route_all`] with the prices of every connection from the factor registry (scope, tier).
pub fn route_all_reg(st: &mut State, conns: &[Conn], reg: &Registry, max_expansions: usize, ws0: f64) -> InitReport {
    route_all_with(st, conns, &|c| InitWeights { max_expansions, ..InitWeights::from_registry(reg, &WireCtx { net: c.net, tier: 3 }) }, ws0)
}

fn route_all_with(st: &mut State, conns: &[Conn], weights: &dyn Fn(&Conn) -> InitWeights, ws0: f64) -> InitReport {
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
        match st.route_conn(c, &weights(c), (c.w + c.s) / ws0) {
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
