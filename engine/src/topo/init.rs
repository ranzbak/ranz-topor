//! P2-lite initial routing in one plane (plan "Pipeline", P2): pad-to-pad connections routed
//! one at a time, widest first then shortest, by A* over the gap graph of the projection
//! (node = CDT edge + gap between its passes); crossings are allowed and priced. Afterwards
//! double crossings are removed by swapping the stretch between them.
//!
//! Complexity: one search is O(S log S) in the gap-graph states S it reaches (bounded by
//! `max_expansions`); `uncross_all` is O(rounds · Σ crossings²) per wire, small in practice.

use super::demand::Item;
use super::factors::{squeeze_amount, Event, Registry, WireCtx};
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
    /// Per pass through an edge, times its over-full history (`congestion`).
    pub congestion_mm: f64,
    /// Per fully squeezed pass between two stretches (`squeeze`).
    pub squeeze_mm: f64,
    /// Per triangle taking the last free entry sector of another net's unrouted pad (`blocks_pad`).
    pub blocks_pad_mm: f64,
    /// Per pass that fits only if a KiCad check set to ignore is violated (`ignored_rule`).
    pub ignored_mm: f64,
    /// Per pass through a gap too narrow for this wire on some layer (fixes the layer, as above).
    pub forced_layer_mm: f64,
    /// `junction_angle` price per mm of width + clearance at `psi` = 1 (the factor's weight).
    pub junction_angle_mm: f64,
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
            congestion_mm: reg.price(&Event::Congestion { wire: 0, edge: 0, history: 1.0 }, w),
            squeeze_mm: reg.price(&Event::Squeeze { wire: 0, edge: 0, slack_nm: 0.0, pitch_nm: 1.0 }, w),
            blocks_pad_mm: reg.price(&Event::BlocksPad { wire: 0, tri: 0, pad: 0 }, w),
            ignored_mm: reg.price(&Event::IgnoredRuleViolated { rule: String::new(), at: Vec2 { x: 0.0, y: 0.0 }, amount_nm: 0.0 }, w),
            forced_layer_mm: forced,
            junction_angle_mm: reg.price(&Event::JunctionAngle { at: Vec2 { x: 0.0, y: 0.0 }, deg: 60.0, ws_mm: 1.0 / super::factors::junction_psi(60.0) }, w),
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
    /// Index of the source end the path starts at ([`State::search_tree`]).
    seed: usize,
    /// Layers the wire can have been on since its last implied via (D6a): the layers of its
    /// start pad, narrowed by every pass that fits on some layers only (a narrow gap, another
    /// net's pad on one layer). A pass that leaves none implies a via (`LayerForced`, priced
    /// once) and the mask starts again from that pass's layers.
    pin: u64,
}

/// One end a tree search may start or stop at (D5 branching, [`State::search_tree`]): a vertex of
/// the net's copper. `on: None` is a terminal vertex (pad centre or a node; any triangle around
/// it); `on: Some((w, k))` is a junction on wire `w` at its traversal `k`, whose triangle has `v`
/// as a corner: the wire is made to turn at `v` there and the new wire ends at it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeEnd {
    pub v: VertId,
    /// The pad obstacle of `v`, or [`super::state::NO_OBS`].
    pub obs: ObsId,
    pub on: Option<(WireId, usize)>,
    /// Cost adjustment (mm) of a path that starts or ends here: the `branch` reward is negative,
    /// the `smd_through` assembly cost positive; 0 for a planned pad.
    pub adj: f64,
    /// Unit directions of the arms already at `v` (up to 4: the trunk's two arms at a branch
    /// point, the traces at a pad); the `junction_angle` factor prices the new trace's angle to
    /// the nearest of them.
    pub arms: [Option<Vec2>; 4],
}

/// A chord `(a, b, net, wire, held, allowed)` of a traversal of a triangle: `held` = the layers
/// that wire's stretch is held to there ([`State::stretch_layers`]), `allowed` = the layers the
/// triangle allows it.
type Chord = (f64, f64, i32, WireId, u64, u64);

/// What crossing another net's stretch does to a wire's layers ([`cross_pin`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossLayer {
    /// The two wires keep distinct layers without a via.
    Free,
    /// One of them needs a via at the crossing (this wire when its triangle allows another layer,
    /// else the other).
    Via,
    /// Neither can leave the shared layer in that triangle: no layer assignment separates them.
    Unlayerable,
}

/// A wire held to layers `pin` (its triangle allows `mine`) crosses another net's stretch held to
/// `held` (its triangle allows `oall`): the two need distinct layers there (Polubasov 2001 §3:
/// a crossing joins the two wires' domains with a "different layers" edge; both ends fixed to the
/// same layer is the case no assignment solves). Returns the effect and this wire's layers after
/// the crossing: a stretch held to one layer narrows this wire's pin to the others; when none is
/// left, a via (this wire takes another layer its triangle allows, else the other wire leaves its
/// layer); when neither triangle allows another layer, unlayerable. With `vias` false
/// (`[search] crossing_vias = "crossing"`, the default) only the unlayerable case is reported and
/// the pin is left as it was: the `crossing` price (TopoR: a 13-20 mm detour rather than a via
/// pair) already stands for the layer change. O(1).
pub fn cross_pin(pin: u64, mine: u64, held: u64, oall: u64, vias: bool) -> (CrossLayer, u64) {
    let r = cross_pin_full(pin, mine, held, oall);
    if vias || r.0 == CrossLayer::Unlayerable {
        r
    } else {
        // `[search] crossing_vias = "crossing"`: the crossing price stands for the via pair.
        (CrossLayer::Free, pin)
    }
}

fn cross_pin_full(pin: u64, mine: u64, held: u64, oall: u64) -> (CrossLayer, u64) {
    let pin = if pin & mine != 0 { pin & mine } else { pin };
    if held.count_ones() != 1 {
        return (CrossLayer::Free, pin);
    }
    if pin & !held != 0 {
        return (CrossLayer::Free, pin & !held);
    }
    if mine & !held != 0 {
        (CrossLayer::Via, mine & !held)
    } else if oall & !pin != 0 {
        (CrossLayer::Via, pin)
    } else {
        (CrossLayer::Unlayerable, pin)
    }
}

/// A tree search result: the path, and the indices of the source and target ends it joins.
pub type TreePath = (Path, usize, usize);

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
    /// Chords `(a, b, net, wire)` of the traversals of a triangle, computed on first use in this
    /// search.
    tc: Vec<Option<Vec<Chord>>>,
    /// Per wire met in this search: its stretch layers per traversal ([`State::stretch_layers`]).
    held: std::collections::HashMap<WireId, Vec<u64>>,
    needs: Vec<f64>,
    items: Vec<Item>,
    heap: BinaryHeap<Key>,
    /// (cost, parent node or MAX, last triangle, source end, target end)
    goal: Option<(f64, usize, TriId, usize, usize)>,
    /// The source and target ends and, per triangle, the target ends a path may stop at in it.
    src: Vec<TreeEnd>,
    dst: Vec<TreeEnd>,
    goal_at: std::collections::BTreeMap<TriId, Vec<usize>>,
    /// Target points and their cost adjustments, for the heuristic.
    gpts: Vec<(Vec2, f64)>,
    /// Extra price (mm, >= 0) of a pass of this net through an edge: the loss-augmentation hook
    /// of the S3b learning loop ([`super::learn`]); `None` in routing.
    hook: Option<&'s PassHook<'s>>,
}

/// Extra price (mm, >= 0) of a pass of wire net `net` through edge `e` ([`State::search_hooked`]).
pub type PassHook<'h> = dyn Fn(i32, EdgeId) -> f64 + 'h;

impl Search<'_, '_> {
    /// Lower bound of the cost from `p` to the cheapest target: per target its straight distance
    /// priced as length plus its (exact) end adjustment; every other price is >= 0, so the
    /// minimum over the targets never overestimates and A* stays exact with the branch reward
    /// (which is a goal-cost adjustment, never a negative edge).
    fn h(&self, p: Vec2) -> f64 {
        let k = self.scale * self.wt.length_mm / MM;
        self.gpts.iter().map(|&(q, a)| (q - p).len() * k + a).fold(f64::INFINITY, f64::min)
    }

    /// `junction_angle` price of a trace that leaves end `d` towards `toward`.
    fn angle_cost(&self, d: &TreeEnd, toward: Vec2) -> f64 {
        if self.wt.junction_angle_mm <= 0.0 || d.arms.iter().all(Option::is_none) {
            return 0.0;
        }
        let dv = toward - self.st.proj.pos[d.v];
        let l = dv.len();
        if l < 1.0 {
            return 0.0;
        }
        let u = dv * (1.0 / l);
        let deg = d.arms.iter().flatten().map(|a| a.dot(u).clamp(-1.0, 1.0).acos().to_degrees()).fold(180.0, f64::min);
        self.wt.junction_angle_mm * (self.c.w + self.c.s) / MM * super::factors::junction_psi(deg)
    }

    fn seed_angle(&self, seed: usize, toward: Vec2) -> f64 {
        match self.src.get(seed) {
            Some(d) => self.angle_cost(d, toward),
            None => 0.0,
        }
    }

    /// Fill the chords of the traversals of `t` (once per search), with the layers each crossed
    /// stretch is held to.
    fn fill_chords(&mut self, t: TriId) {
        if self.tc[t].is_some() {
            return;
        }
        let st = self.st;
        let mut v: Vec<Chord> = Vec::new();
        for &(o, ok) in &st.tri_trav[t] {
            let net = st.wires[o].net;
            let held = self.held.entry(o).or_insert_with(|| st.stretch_layers(o))[ok];
            let oall = st.proj.allowed(t, net).map_or(0, |m| m.0);
            v.extend(st.chords(o, ok).into_iter().map(|(b1, b2)| (b1, b2, net, o, held, oall)));
        }
        self.tc[t] = Some(v);
    }

    /// Price of a new chord `xb -> yb` across the chords `tc` of a triangle (not those of the
    /// wires in `skip`) and the wire's layers after it ([`cross_pin`]): per crossing the
    /// `crossing` price, plus a via where both are held to one layer and one can leave it, or
    /// the `overflow` price where neither can.
    #[allow(clippy::too_many_arguments)]
    fn arc_chords(tc: &[Chord], wt: &InitWeights, net: i32, xb: f64, yb: f64, pin: u64, mine: u64, skip: &[Option<WireId>], vias: bool) -> (f64, u64) {
        let (mut cross, mut pg) = (0.0, pin);
        for &(b1, b2, onet, o, held, oall) in tc {
            if skip.contains(&Some(o)) || !interleave(xb, yb, b1, b2) {
                continue;
            }
            if onet == net {
                cross += wt.same_net_cross_mm;
                continue;
            }
            let (k, p2) = cross_pin(pg, mine, held, oall, vias);
            pg = p2;
            cross += wt.cross_mm
                + match k {
                    CrossLayer::Free => 0.0,
                    CrossLayer::Via => wt.forced_layer_mm,
                    CrossLayer::Unlayerable => wt.overflow_mm,
                };
        }
        (cross, pg)
    }

    fn arc(&self, t: TriId, xb: f64, yb: f64, pin: u64, mine: u64, skip: &[Option<WireId>]) -> (f64, u64) {
        Self::arc_chords(self.tc[t].as_deref().expect("filled"), &self.wt, self.c.net, xb, yb, pin, mine, skip, self.st.cross_vias)
    }

    fn slot(&self, e: EdgeId, g: usize, into: TriId) -> usize {
        self.off[e] + 2 * g + usize::from(self.st.proj.edge_tris[e][0] != Some(into))
    }

    fn push(&mut self, n: Node) {
        let slot = self.slot(n.e, n.g, n.into);
        if self.best[slot] != u32::MAX {
            // The node there wins unless the new one is cheaper, or keeps more layers open
            // (a superset of the pin) for less than the via that saves.
            let o = &self.nodes[self.best[slot] as usize];
            let wider = n.pin & o.pin == o.pin && n.pin != o.pin;
            if o.cost <= n.cost && !(wider && n.cost < o.cost + self.wt.forced_layer_mm) {
                return;
            }
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

    /// Arcs from `x` (point `px`, cost so far `base`) through triangle `t`; `seed` is the source
    /// end of the path, `except` a wire whose chords in `t` are not crossed (the trunk a branch
    /// leaves at a junction corner of `t`).
    #[allow(clippy::too_many_arguments)]
    fn expand(&mut self, t: TriId, x: End, in_e: Option<EdgeId>, px: Vec2, base: f64, parent: usize, seed: usize, except: Option<WireId>, pin: u64) {
        let (st, c) = (self.st, self.c);
        let pr = st.proj;
        // Taking the last free entry sector of another net's pad.
        let base = if self.wt.blocks_pad_mm > 0.0 { base + self.wt.blocks_pad_mm * st.blocked_pads(t, c.net).len() as f64 } else { base };
        self.fill_chords(t);
        let mine = pr.allowed(t, c.net).map_or(0, |m| m.0);
        let xb = st.end_bpos(t, x);
        if let Some(js) = self.goal_at.get(&t).cloned() {
            for j in js {
                let d = self.dst[j];
                if matches!(x, End::Corner(v) if v == d.v) {
                    continue;
                }
                let skip = [except, d.on.map(|o| o.0)];
                let (xc, pin_end) = self.arc(t, xb, pr.corner_pos(t, d.v), pin, mine, &skip);
                let cost = base
                    + if pin_end & st.end_mask(&d) == 0 { self.wt.forced_layer_mm } else { 0.0 }
                    + d.adj
                    + self.angle_cost(&d, px)
                    + (pr.pos[d.v] - px).len() / MM * self.scale * self.wt.length_mm
                    + xc;
                if self.goal.is_none_or(|g| cost < g.0) {
                    self.goal = Some((cost, parent, t, seed, j));
                }
            }
        }
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
            // The pass's layer pin, its price and the room of the layers it leaves (a stretch held
            // to one layer has that layer's room only, not the sum over the layers the triangles
            // allow, D6a), for the pin the wire has after its crossings in `t` (D6b).
            let wt = self.wt;
            let hook = self.hook.map_or(0.0, |h| h(c.net, e));
            let through = |p: u64| {
                let (forced, nowhere, pin2) = st.pin_through(p, e, t, nt, c.net, alone);
                let extra = if forced { wt.forced_layer_mm } else { 0.0 } + if nowhere { wt.overflow_mm } else { 0.0 } + hook;
                let pinned = |m: u64| if m & pin2 != 0 { m & pin2 } else { m };
                let layers = mask_t.map(|m| pinned(m.0 & mask_nt.0));
                let room1 = layers.map(|b| (0..pr.n_layers).filter(|&l| (b >> l) & 1 == 1).map(|l| pr.room_for(e, l, c.net)).fold(f64::NEG_INFINITY, f64::max));
                let room = layers.map(|b| (0..pr.n_layers).filter(|&l| (b >> l) & 1 == 1).map(|l| pr.room_for(e, l, c.net)).sum::<f64>());
                (extra, pin2, room1, room)
            };
            let base_pass = through(pin);
            let room = base_pass.3;
            self.needs.clear();
            if room.is_some_and(f64::is_finite) {
                st.edge_need_gaps(e, me, alone, &mut self.items, &mut self.needs);
            }
            // Per-edge geometry of the gaps (see `State::gap_point` / `gap_bpos`), hoisted.
            let n = st.edge_order[e].len();
            let hist = self.wt.congestion_mm * st.history[e];
            let ign = if pr.edge_clear_ignored > 0.0 {
                let both = mask_t.map_or(0, |m| m.0) & mask_nt.0;
                st.ignored_violation(e, both, c.net, alone).map_or(0.0, |_| self.wt.ignored_mm)
            } else {
                0.0
            };
            let pitch = c.w + c.s;
            let [ea, eb] = pr.edges[e];
            let (pa, pb) = (pr.pos[ea], pr.pos[eb]);
            let side = pr.side(t, e);
            let fwd = pr.tris[t][side] == ea;
            for g in 0..=n {
                let pt = pa + (pb - pa) * ((g + 1) as f64 / (n + 2) as f64);
                let f = (2.0 * g as f64 + 1.0) / (2.0 * n as f64 + 2.0);
                let yb = side as f64 + if fwd { f } else { 1.0 - f };
                let (cross, pg) = Self::arc_chords(self.tc[t].as_deref().expect("filled above"), &wt, c.net, xb, yb, pin, mine, &[except], st.cross_vias);
                let (extra, pin2, room1, room) = if pg == pin { base_pass } else { through(pg) };
                // A pin narrowed by a crossing may meet a finite room the base pin did not.
                if self.needs.is_empty() && (room.is_some_and(f64::is_finite) || room1.is_some_and(f64::is_finite)) {
                    st.edge_need_gaps(e, me, alone, &mut self.items, &mut self.needs);
                }
                let cost = base
                    + (pt - px).len() / MM * self.scale * wt.length_mm
                    + cross
                    + if room.is_some_and(|r| r.is_finite() && self.needs[g] > r) { wt.overflow_mm } else { 0.0 }
                    + extra
                    + hist
                    + ign
                    + match room1.filter(|r| r.is_finite()) {
                        Some(r) if g > 0 && g < n => wt.squeeze_mm * squeeze_amount(r - self.needs[g], pitch),
                        _ => 0.0,
                    };
                // Leaving a junction seed: the angle to the arms already there.
                let cost = if parent == usize::MAX { cost + self.seed_angle(seed, pt) } else { cost };
                self.push(Node { e, g, into: nt, from_tri: t, parent, cost, pt, seed, pin: pin2 });
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

    /// Layers of a tree end (D6a): the pad's copper layers (all for a drilled pad), all for a
    /// node or a branch point on a trace (its layer is not fixed before P3).
    pub(crate) fn end_mask(&self, d: &TreeEnd) -> u64 {
        let all = super::state::LayerMask::all(self.proj.n_layers).0;
        match self.proj.obstacles.get(d.obs) {
            Some(ob) if d.on.is_none() && !ob.drilled => ob.layers.0 & all,
            _ => all,
        }
    }

    /// The layers a pass of `net` through `e` from `t` into `nt` may take: allowed in both
    /// triangles (another net's single-layer pad closes its layer) and wide enough for the wire
    /// alone (`alone`); when it fits on none, the layers both allow (the overflow is priced
    /// apart).
    pub(crate) fn pass_mask(&self, e: EdgeId, t: TriId, nt: TriId, net: i32, alone: f64) -> (u64, u64) {
        let pr = self.proj;
        let both = pr.allowed(t, net).map_or(0, |m| m.0) & pr.allowed(nt, net).map_or(0, |m| m.0);
        let fit = (0..pr.n_layers).filter(|&l| (both >> l) & 1 == 1 && alone <= pr.room_for(e, l, net)).fold(0u64, |m, l| m | 1 << l);
        (fit, both)
    }

    /// One pass of the layer pin (see `Node::pin`): whether it implies a via, whether the wire
    /// alone fits on no layer there (an overflow whatever the layers), and the new pin.
    /// A stretch that has to keep one layer (a narrow gap, under another net's pad) costs no
    /// via when the wire is on that layer anyway (its own SMD pad's, or the one the stretch
    /// before already kept); a via is priced once where the layers run out, not per pass.
    pub(crate) fn pin_through(&self, pin: u64, e: EdgeId, t: TriId, nt: TriId, net: i32, alone: f64) -> (bool, bool, u64) {
        let (fit, both) = self.pass_mask(e, t, nt, net, alone);
        if fit == 0 {
            // Too narrow on every layer: the pin keeps to the layers the triangles allow.
            let ok = both;
            return if ok == 0 || pin & ok != 0 { (false, true, if ok == 0 { pin } else { pin & ok }) } else { (true, true, ok) };
        }
        if pin & fit != 0 {
            (false, false, pin & fit)
        } else {
            (true, false, fit)
        }
    }

    /// The passes of a path (`edges`, triangles `tris`, one more) of `net` from end `a` to end
    /// `b` where a via is implied (the layer pin of [`State::pin_through`]); a mismatch at the
    /// end `b` is reported on the last edge. The `LayerForced` events of a routed wire, without
    /// its crossings ([`State::layer_walk`] with them).
    #[allow(clippy::too_many_arguments)]
    pub fn forced_edges(&self, net: i32, w: f64, s: f64, a: &TreeEnd, b: &TreeEnd, edges: &[EdgeId], tris: &[TriId]) -> Vec<EdgeId> {
        self.layer_walk(net, w, s, a, b, edges, tris, &|_| Vec::new()).0
    }

    /// [`State::forced_edges`] with the crossings of the path (D6b): `cross(k)` lists the
    /// different-net stretches the path crosses in its traversal `k`, as `(held, allowed)` layer
    /// masks ([`cross_pin`]), in path order. Returns the edges of the implied vias (a crossing's
    /// via is reported on the pass leaving its triangle, or entering it at the last one) and those
    /// of the crossings no layer assignment separates. O(path · layers).
    #[allow(clippy::too_many_arguments)]
    pub fn layer_walk(&self, net: i32, w: f64, s: f64, a: &TreeEnd, b: &TreeEnd, edges: &[EdgeId], tris: &[TriId], cross: &dyn Fn(usize) -> Vec<(u64, u64)>) -> (Vec<EdgeId>, Vec<EdgeId>) {
        let pr = self.proj;
        let me = Item { net, w, s };
        let (mut out, mut unl) = (Vec::new(), Vec::new());
        let at = |k: usize| edges.get(k).or(edges.last()).copied().unwrap_or_else(|| pr.tri_edges[tris[k]][0]);
        let m0 = tris.first().and_then(|&t| pr.allowed(t, net)).map_or(0, |m| m.0);
        let mut pin = if m0 & self.end_mask(a) != 0 { m0 & self.end_mask(a) } else { m0 };
        if m0 & self.end_mask(a) == 0 && !tris.is_empty() {
            out.push(at(0));
        }
        for (k, &t) in tris.iter().enumerate() {
            let mine = pr.allowed(t, net).map_or(0, |m| m.0);
            for (held, oall) in cross(k) {
                let (x, p2) = cross_pin(pin, mine, held, oall, self.cross_vias);
                pin = p2;
                match x {
                    CrossLayer::Free => {}
                    CrossLayer::Via => out.push(at(k)),
                    CrossLayer::Unlayerable => unl.push(at(k)),
                }
            }
            if let Some(&e) = edges.get(k) {
                let (forced, _, p2) = self.pin_through(pin, e, t, tris[k + 1], net, pr.edge_demand(e, &[me]));
                if forced {
                    out.push(e);
                }
                pin = p2;
            }
        }
        if pin & self.end_mask(b) == 0 && !tris.is_empty() {
            out.push(at(tris.len() - 1));
        }
        (out, unl)
    }

    /// The layers wire `w`'s stretch is held to in each traversal (D6b): the layer pin from its
    /// end `a` forward and from its end `b` backward ([`State::pin_through`], end pads' layers),
    /// intersected; where they share none a via lies in between and the stretch may be on either
    /// (their union). Its own crossings are not counted. O(passes · layers).
    pub fn stretch_layers(&self, w: WireId) -> Vec<u64> {
        let pr = self.proj;
        let wi = &self.wires[w];
        let me = Item { net: wi.net, w: wi.w, s: wi.s };
        let n = wi.passes.len();
        let all = super::state::LayerMask::all(pr.n_layers).0;
        let start = |t: TriId, v: VertId, obs: ObsId| {
            let m = pr.allowed(t, wi.net).map_or(all, |m| m.0);
            let pm = self.end_mask(&TreeEnd { v, obs, on: None, adj: 0.0, arms: [None; 4] });
            if m & pm != 0 { m & pm } else { m }
        };
        let edge = |k: usize| self.passes[wi.passes[k]].edge;
        let mut fwd = vec![0u64; n + 1];
        fwd[0] = start(wi.tris[0], wi.a, wi.pa);
        for k in 0..n {
            let e = edge(k);
            fwd[k + 1] = self.pin_through(fwd[k], e, wi.tris[k], wi.tris[k + 1], wi.net, pr.edge_demand(e, &[me])).2;
        }
        let mut bwd = vec![0u64; n + 1];
        bwd[n] = start(wi.tris[n], wi.b, wi.pb);
        for k in (0..n).rev() {
            let e = edge(k);
            bwd[k] = self.pin_through(bwd[k + 1], e, wi.tris[k + 1], wi.tris[k], wi.net, pr.edge_demand(e, &[me])).2;
        }
        (0..=n).map(|k| if fwd[k] & bwd[k] != 0 { fwd[k] & bwd[k] } else { fwd[k] | bwd[k] }).collect()
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
        self.search_hooked(c, wt, scale, None)
    }

    /// [`State::search`] with an extra price per pass (`hook`, see [`PassHook`]); `None` = `search`.
    pub fn search_hooked(&self, c: &Conn, wt: &InitWeights, scale: f64, hook: Option<&PassHook>) -> Option<Path> {
        let src = [TreeEnd { v: c.a, obs: c.pa, on: None, adj: 0.0, arms: [None; 4] }];
        let dst = [TreeEnd { v: c.b, obs: c.pb, on: None, adj: 0.0, arms: [None; 4] }];
        self.search_tree(c, &src, &dst, wt, scale, hook).map(|x| x.0)
    }

    /// Cheapest path from any end of `src` to any end of `dst` (multi-source, multi-target A*,
    /// Takahashi-Matsuyama tree growth as in PathFinder / BonnRoute: D5 branching): the source
    /// ends are seeded at cost 0, the heuristic is the distance to the nearest target end (a lower
    /// bound, so A* stays exact). `c` gives the net, width and clearance (its terminals are not
    /// used). A junction end (`on: Some((w, k))`) is reached only in its triangle, and the chords
    /// of `w` there are not crossed (the trunk turns at the corner).
    pub fn search_tree(&self, c: &Conn, src: &[TreeEnd], dst: &[TreeEnd], wt: &InitWeights, scale: f64, hook: Option<&PassHook>) -> Option<TreePath> {
        let pr = self.proj;
        let mut off = Vec::with_capacity(pr.edges.len() + 1);
        let mut acc = 0;
        for e in 0..pr.edges.len() {
            off.push(acc);
            acc += 2 * (self.edge_order[e].len() + 1);
        }
        off.push(acc);
        let tris_of = |d: &TreeEnd| -> Vec<TriId> {
            match d.on {
                Some((w, k)) => vec![self.wires[w].tris[k]],
                None => pr.vert_tris[d.v].iter().copied().filter(|&t| pr.allowed(t, c.net).is_some()).collect(),
            }
        };
        let mut goal_at: std::collections::BTreeMap<TriId, Vec<usize>> = std::collections::BTreeMap::new();
        for (j, d) in dst.iter().enumerate() {
            for t in tris_of(d) {
                goal_at.entry(t).or_default().push(j);
            }
        }
        let mut gpts: Vec<(Vec2, f64)> = dst.iter().map(|d| (pr.pos[d.v], d.adj)).collect();
        gpts.sort_by(|a, b| a.0.x.total_cmp(&b.0.x).then(a.0.y.total_cmp(&b.0.y)).then(a.1.total_cmp(&b.1)));
        gpts.dedup_by(|a, b| (a.0 - b.0).len2() < 1.0);
        let mut s = Search {
            st: self,
            c: *c,
            wt: *wt,
            scale,
            nodes: Vec::new(),
            best: vec![u32::MAX; off.last().copied().unwrap_or(0)],
            off: off[..off.len() - 1].to_vec(),
            tc: vec![None; pr.tris.len()],
            held: std::collections::HashMap::new(),
            needs: Vec::new(),
            items: Vec::new(),
            heap: BinaryHeap::new(),
            goal: None,
            src: src.to_vec(),
            dst: dst.to_vec(),
            goal_at,
            gpts,
            hook,
        };
        for (i, d) in src.iter().enumerate() {
            for t in tris_of(d) {
                // A start pad on layers the triangle does not allow: a via at once.
                let (m, pm) = (pr.allowed(t, c.net).map_or(0, |m| m.0), self.end_mask(d));
                let (pin, adj) = if m & pm != 0 { (m & pm, d.adj) } else { (m, d.adj + wt.forced_layer_mm) };
                s.expand(t, End::Corner(d.v), None, pr.pos[d.v], adj, usize::MAX, i, d.on.map(|o| o.0), pin);
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
            let (t, e, g, pt, cost, seed, pin) = (n.into, n.e, n.g, n.pt, n.cost, n.seed, n.pin);
            s.expand(t, End::Gap(e, g), Some(e), pt, cost, i, seed, None, pin);
        }
        if std::env::var("P2_PROF").is_ok() {
            eprintln!("conn {}: expansions {expansions}, nodes {}, cost {:.1}, w {:.2}", c.net, s.nodes.len(), s.goal.map_or(-1.0, |g| g.0), c.w / MM);
        }
        if std::env::var("TOPO_TREEDBG").is_ok() {
            eprintln!("TREEGOAL net {} srcs {} dsts {} cost {:?} expansions {expansions}", c.net, src.len(), dst.len(), s.goal.map(|g| (g.0, g.3, g.4)));
        }
        let (_, mut i, last, seed, j) = s.goal?;
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
        Some(((path, tris), seed, j))
    }

    /// Route `c` and insert it: the wire and the number of passes over room; `None` when no
    /// path exists.
    pub fn route_conn(&mut self, c: &Conn, wt: &InitWeights, scale: f64) -> Option<(WireId, usize)> {
        self.route_conn_hooked(c, wt, scale, None)
    }

    /// [`State::route_conn`] with an extra price per pass (`hook`, see [`PassHook`]).
    pub fn route_conn_hooked(&mut self, c: &Conn, wt: &InitWeights, scale: f64, hook: Option<&PassHook>) -> Option<(WireId, usize)> {
        let (path, tris) = self.search_hooked(c, wt, scale, hook)?;
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
