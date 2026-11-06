//! S3b learning loop (plan "Social factors: learnable and pluggable", *Distilled from hand
//! routings*): factor weights fitted to hand routings by maximum margin planning (Ratliff, Bagnell
//! and Zinkevich 2006), solved with the paper's subgradient method. Output only: learned weights
//! are written as a `topor.toml [factors]` snippet and never applied by default.
//!
//! **One board, one projection.** The hand routing is mapped into the State by the R0 witness
//! ([`super::witness::run`], `load_all`) on the witness board: the board with the hand's chain
//! nodes (free ends, layer changes) inserted as vertices. The engine side of a training board is
//! routed on *that same* board ([`route_lite`]), so hand and engine stretches are sequences of the
//! same CDT edges and can be compared edge by edge. `route_lite` is the engine's P2 (same costs,
//! same order, same detour retry and cap retry as `core::route_round`) plus `uncross_all` and P3
//! (`layers::assign_with` with the packer); it skips P4 (rip-up), the drawing and the via
//! reduction. The full engine (`topo_route`) is used only for evaluation.
//!
//! **Factor totals f(routing)** ([`routing_events`], [`Totals`]): for every alive wire of a
//! complete State, the events it has with all other wires present, summed per factor with the
//! factor's own `amount` (the same `standard_factors` the registry prices with):
//! * `Length`: the State polyline (terminal, pass points, terminal), scale (w+s)/(w0+s0);
//! * `Via`: the hand chain's vias / P3's layer changes of the wire;
//! * `Crossing`: one per interleaving chord pair, counted once per pair (lower wire id);
//! * `LayerForced`: as P2's search prices it (`InitWeights::smd_over_mm` / `forced_layer_mm`): a
//!   change between triangles over and not over another net's SMD pad (or of the layers allowed
//!   over it), and a pass through a gap too narrow for the wire on some layer;
//! * `Overflow`, `Squeeze`, `Congestion`, `IgnoredRuleViolated`, `BlocksPad`: as
//!   [`super::factors::events`] defines them for an insert, but for a pass already in the edge
//!   order (its own item is not added a second time; a squeeze needs a pass on both sides).
//!
//! Only nets both sides route count: nets with a hand wire whose connections all reach the lite
//! engine (pad to pad, no pour terminal, no differential pair). The totals are per board divided
//! by the number of hand wires counted.
//!
//! **Loss** ([`loss`]). A wire's topology relative to the obstacles is written as a *word*
//! ([`Words`]), read along the wire:
//! * a pass through a *cut*, a CDT edge whose two ends belong to two different obstacles (pad /
//!   keepout outline or centre, or the board edge), neither of them copper of the wire's own net,
//!   is a letter signed by the direction of the pass; an edge with a Steiner or chain-node end is
//!   no cut, because a wire may pass such a point on either side without changing its topology;
//! * the obstacle vertices joined by such edges form components; a component not joined to the
//!   board edge (an *island*, e.g. a footprint in open board space) gets a vertical ray from its
//!   highest vertex, and a crossing of that ray is a letter signed left to right / back, so a
//!   wire passing an island on the left differs from one passing it on the right;
//! * a letter followed by its inverse cancels (free reduction).
//!
//! The State polyline runs inside the triangles of the wire, so its ray crossings are those of the
//! wire up to homotopy relative to the obstacles. A hand *stretch* is a maximal run of consecutive
//! passes of one hand wire on one layer, `(net, layer, word)`, the word made of the letters of
//! those passes and of the pieces leading into them; a stretch whose word is empty is not counted.
//! The engine *reproduces* a stretch when some alive engine wire of the same net has the stretch's
//! word as a contiguous part of its own reduced word (or the inverse word, read backwards), and
//! every letter of that part may be on the stretch's layer. The layer of an engine pass comes from
//! P3's labelled events of its wire: between two events of equal label it is that label; between
//! events of different labels (a via interval: the via can be anywhere in it) it is either; before
//! the first / after the last event it is that event's label; a wire with no event may be on any
//! layer. `loss = (hand stretches not reproduced) / (hand stretches)`, in [0, 1].
//!
//! **Loss-augmented routing.** The stretch loss does not decompose along a search path, so P2
//! uses a per-pass surrogate through the search hook ([`super::init::PassHook`]): a pass of a
//! net through an edge the hand's wires of that net pass through costs `margin_mm` more. Up to a
//! constant this is `margin_mm x` (hand edges of the net the engine does not reuse), a Hamming
//! loss on the hand's edge set, so `cost + margin x reused` is `cost - loss`: the search looks for
//! the cheapest routing that is unlike the hand's. The extra price is >= 0, so A* stays admissible.
//!
//! **Subgradient step** ([`learn_loop`]), per iteration t over the training boards b:
//! `g = lambda w + mean_b (f_b(hand) - f_b(engine)) / n_b`, `w <- max(0, w - eta_t g)` with the
//! diminishing step `eta_t = eta / sqrt(t + 1)`; the learned weights are the average of the
//! iterates (the usual output of a subgradient method; the last iterate is kept too). `n_b` is
//! the number of hand wires of b (so a board weighs the same whatever its size), f is taken
//! only for the learned factors and only over wires of the learned tier. `length` is the unit of
//! every weight (mm of copper per unit) and stays 1; the other factors are learned only if
//! listed. Stop when `max |delta w| < eps` or after `iters` iterations.
//!
//! **Tiers.** Learned weights only apply within a tier: with `tier = Some(t)` only wires of tier
//! t count in f and the loss, and only connections of tier t are priced with the learned weights
//! in training (P3's via price stays the default then). The tier order of the routing never
//! changes. `[factors] scope` cannot hold a per-tier weight next to a board-wide one, so a
//! tier-scoped result is written as a commented snippet.
//!
//! Deterministic: Vec + index ids, BTreeMap only, P3's seeded local search; the training boards of
//! one iteration route in parallel threads and are merged in board order.
//!
//! Complexity per iteration: one P2 + P3 per training board (dominant: P2's A*, see `init`);
//! totals O(passes x traversals per triangle); words O(passes x rays in a piece's x range); loss
//! O(hand letters x engine runs of the same letter).

use super::board::Board;
use super::factors::{standard_factors, Event, FactorConfig, Registry, WireCtx};
use super::init::{Conn, InitWeights, PassHook};
use super::layers::{self, LayerParams, LayerReport};
use super::pack::Packer;
use super::state::{EdgeId, Projection, State, VertKind, WireId};
use super::witness::{self, Spots, Witness, WitnessOpts};
use crate::decompose::{decompose, TermKind};
use crate::model::{BoardModel, Options};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

const MM: f64 = 1e6;

/// Event-kind counts that are not factor amounts, for the event statistics (item 4).
pub const EVENT_KINDS: [&str; 13] = [
    "length_mm",
    "excess_length_mm",
    "passes",
    "vias",
    "layer_forced",
    "crossings",
    "same_net_crossings",
    "squeezes",
    "squeeze_amount",
    "overflows",
    "blocks_pad",
    "congestion",
    "ignored_rule",
];

/// Factor totals of one routing: `amount[i]` is the sum of `standard_factors()[i].amount` over
/// the routing's events; `events` the raw counts per [`EVENT_KINDS`]; `wires` the wires counted.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Totals {
    pub amount: Vec<f64>,
    pub events: BTreeMap<String, f64>,
    pub wires: usize,
}

/// A hand stretch: a run of one wire on one layer; `edges` is its reduced word (letters, see
/// [`Words`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stretch {
    pub net: i32,
    pub layer: usize,
    pub edges: Vec<i64>,
}

/// The obstacle a vertex belongs to (`usize::MAX` = the board edge); `None` for a Steiner, split
/// or chain-node vertex.
fn owner(pr: &Projection, v: usize) -> Option<usize> {
    match pr.vkind[v] {
        VertKind::Outline(o) | VertKind::Centre(o) => Some(o),
        VertKind::Edge => Some(usize::MAX),
        VertKind::Free | VertKind::Node => None,
    }
}

/// Whether edge `e` is a cut for a wire of `net`: its ends belong to two different obstacles,
/// neither of them copper of `net` itself (module doc).
pub fn is_cut(pr: &Projection, e: EdgeId, net: i32) -> bool {
    let [p, q] = pr.edges[e];
    let foreign = |o: usize| o == usize::MAX || pr.obstacles[o].net != Some(net);
    matches!((owner(pr, p), owner(pr, q)), (Some(a), Some(b)) if a != b && foreign(a) && foreign(b))
}

/// The letters a wire's topology is written in (module doc): a pass through cut `e` is
/// `+(e + 1)` from the side of `edge_tris[e][0]`, `-(e + 1)` the other way; a crossing of island
/// ray `i` is `+(E + 1 + i)` left to right, `-(E + 1 + i)` back (E = number of edges).
pub struct Words {
    /// Ray origins (vertex ids), one per vertex that is the top of an island for some net.
    rays: Vec<usize>,
    /// Per net: its island rays, sorted by origin x.
    of_net: BTreeMap<i32, Vec<usize>>,
    n_edges: usize,
}

impl Words {
    /// The island rays of `pr`, per net N: the vertices of obstacles that are not N's copper,
    /// joined by the cuts for N and the edges inside one obstacle, form components; a component
    /// not joined to the board edge is an island for N and gets a vertical ray (+y) from its
    /// highest (then rightmost) vertex. O(nets x (V + E)).
    pub fn new(pr: &Projection) -> Words {
        let n = pr.pos.len();
        fn find(p: &mut [usize], mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        let nets: BTreeSet<i32> = pr.obstacles.iter().filter_map(|o| o.net).filter(|&n| n > 0).collect();
        let mut ray_of: BTreeMap<usize, usize> = BTreeMap::new();
        let mut rays: Vec<usize> = Vec::new();
        let mut of_net: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
        let mut parent: Vec<usize> = Vec::new();
        for &net in &nets {
            let solid = |v: usize| owner(pr, v).is_some_and(|o| o == usize::MAX || pr.obstacles[o].net != Some(net));
            parent.clear();
            parent.extend(0..=n); // n = the board edge
            for v in 0..n {
                if pr.vkind[v] == VertKind::Edge {
                    let a = find(&mut parent, v);
                    parent[a] = n;
                }
            }
            for &[p, q] in &pr.edges {
                if solid(p) && solid(q) {
                    let (a, b) = (find(&mut parent, p), find(&mut parent, q));
                    if a != b {
                        parent[a.min(b)] = a.max(b);
                    }
                }
            }
            let root_edge = find(&mut parent, n);
            let mut top: BTreeMap<usize, usize> = BTreeMap::new(); // component root -> highest vertex
            for v in (0..n).filter(|&v| solid(v) && pr.vkind[v] != VertKind::Edge) {
                let r = find(&mut parent, v);
                if r == root_edge {
                    continue;
                }
                let e = top.entry(r).or_insert(v);
                let (a, b) = (pr.pos[v], pr.pos[*e]);
                if a.y > b.y || (a.y == b.y && a.x > b.x) {
                    *e = v;
                }
            }
            let mut ids: Vec<usize> = top
                .values()
                .map(|&v| {
                    *ray_of.entry(v).or_insert_with(|| {
                        rays.push(v);
                        rays.len() - 1
                    })
                })
                .collect();
            ids.sort_by(|&a, &b| pr.pos[rays[a]].x.total_cmp(&pr.pos[rays[b]].x).then(a.cmp(&b)));
            of_net.insert(net, ids);
        }
        Words { rays, of_net, n_edges: pr.edges.len() }
    }

    /// Islands of `net` (rays its wires are read against).
    pub fn islands(&self, net: i32) -> usize {
        self.of_net.get(&net).map_or(0, Vec::len)
    }

    /// The letters of alive wire `w` of `st`, each with the index of the pass it belongs to (a
    /// ray crossing in traversal k belongs to pass min(k, n - 1)); rays are those of the wire's
    /// net. O(passes x rays of the net in the x range of a piece).
    pub fn wire(&self, st: &State, w: WireId) -> Vec<(i64, usize)> {
        let pr = st.proj;
        let wi = &st.wires[w];
        let n = wi.passes.len();
        let pts = st.polyline(w);
        let by_x: &[usize] = self.of_net.get(&wi.net).map_or(&[], |v| v.as_slice());
        let mut out = Vec::new();
        for k in 0..pts.len() - 1 {
            let (a, b) = (pts[k], pts[k + 1]);
            let tag = k.min(n.saturating_sub(1));
            let (lo, hi) = if a.x <= b.x { (a.x, b.x) } else { (b.x, a.x) };
            let start = by_x.partition_point(|&i| pr.pos[self.rays[i]].x <= lo);
            let mut hits: Vec<(f64, i64)> = Vec::new();
            for &i in &by_x[start..] {
                let o = pr.pos[self.rays[i]];
                if o.x > hi {
                    break;
                }
                // Half-open in x: (lo, hi], so a piece ending on the ray line counts once.
                let t = (o.x - a.x) / (b.x - a.x);
                let y = a.y + (b.y - a.y) * t;
                if y > o.y {
                    let id = (self.n_edges + 1 + i) as i64;
                    hits.push((t, if b.x > a.x { id } else { -id }));
                }
            }
            hits.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            out.extend(hits.into_iter().map(|h| (h.1, tag)));
            if k < n {
                let e = st.passes[wi.passes[k]].edge;
                if is_cut(pr, e, wi.net) {
                    let id = (e + 1) as i64;
                    out.push((if pr.edge_tris[e][0] == Some(wi.tris[k]) { id } else { -id }, k));
                }
            }
        }
        out
    }
}

/// Free reduction of a word: a letter followed by its inverse cancels. O(word).
pub fn reduce<T: Copy>(run: impl IntoIterator<Item = (i64, T)>) -> Vec<(i64, T)> {
    let mut out: Vec<(i64, T)> = Vec::new();
    for (l, t) in run {
        if out.last().is_some_and(|x| x.0 == -l) {
            out.pop();
        } else {
            out.push((l, t));
        }
    }
    out
}

/// What the learning needs of the hand routing of one board (computed once).
pub struct HandRef {
    pub totals: Totals,
    pub stretches: Vec<Stretch>,
    /// Per edge: the nets (sorted, unique) whose hand wires pass through it (the loss-augmentation
    /// hook prices a pass of such a net through it).
    pub on_hand: Vec<Vec<i32>>,
}

/// One training board: the witness (hand chains on the witness board), the engine connections
/// on that board, and the hand reference.
pub struct Sample {
    pub name: String,
    pub w: Witness,
    /// The letters of wire topology on the witness board (island rays).
    pub words: Words,
    /// Pad-to-pad connections of the stripped board with their tier, in decompose order.
    pub conns: Vec<(Conn, u8)>,
    /// w0 + s0 of the default net class (the length unit, nm).
    pub ws0: f64,
    /// Net -> tier (lowest tier of its connections; 3 when it has none).
    pub tier_of_net: BTreeMap<i32, u8>,
    /// The nets that count in the totals and the loss: every connection reaches the lite engine,
    /// and the net is of the learned tier.
    pub counted: BTreeSet<i32>,
    pub model: BoardModel,
    /// Engine options of the lite routing (defaults; `seed` seeds P3's local search).
    pub opts: Options,
    pub hand: HandRef,
    /// Connections dropped from the lite engine (pour terminals, other terminals, diff pairs).
    pub skipped: usize,
}

impl Sample {
    pub fn tier(&self, net: i32) -> u8 {
        self.tier_of_net.get(&net).copied().unwrap_or(3)
    }
}

/// The events of every alive wire of a complete `st` that `keep` accepts (see the module doc).
/// `vias[w]` = vias of wire `w`. O(passes x traversals per triangle).
pub fn routing_events(st: &State, vias: &[u32], ws0: f64, keep: &dyn Fn(WireId) -> bool) -> Vec<Event> {
    let pr = st.proj;
    let mut out = Vec::new();
    for w in 0..st.wires.len() {
        let wi = &st.wires[w];
        if !wi.alive || !keep(w) {
            continue;
        }
        let pl = st.polyline(w);
        let mm = pl.windows(2).map(|p| (p[1] - p[0]).len()).sum::<f64>() / MM;
        out.push(Event::Length { wire: w, mm, scale: (wi.w + wi.s) / ws0 });
        let nv = vias.get(w).copied().unwrap_or(0);
        if nv > 0 {
            out.push(Event::Via { wire: w, count: nv, at: None });
        }
        for c in st.crossings(w) {
            if c.other > w || !keep(c.other) {
                out.push(Event::Crossing { a: w, b: c.other, tri: c.tri, same_net: st.wires[c.other].net == wi.net });
            }
        }
        let me = super::demand::Item { net: wi.net, w: wi.w, s: wi.s };
        for &t in &wi.tris {
            for pad in st.blocked_pads(t, wi.net) {
                out.push(Event::BlocksPad { wire: w, tri: t, pad });
            }
        }
        // P2's layer pin: a via implied where a stretch fixed to some layers meets one fixed to
        // others (or the end pad's layer), once per change.
        let edges: Vec<usize> = wi.passes.iter().map(|&p| st.passes[p].edge).collect();
        let end = |v, obs| super::init::TreeEnd { v, obs, on: None, adj: 0.0, arms: [None; 4] };
        // D6b: with the different-net stretches it crosses (their held layers).
        let mut xl: Vec<Vec<(u64, u64)>> = vec![Vec::new(); wi.tris.len()];
        for c in st.crossings(w) {
            let on = st.wires[c.other].net;
            if on != wi.net {
                xl[c.k].push((st.stretch_layers(c.other)[c.other_k], pr.allowed(c.tri, on).map_or(0, |m| m.0)));
            }
        }
        let (forced, unl) = st.layer_walk(wi.net, wi.w, wi.s, &end(wi.a, wi.pa), &end(wi.b, wi.pb), &edges, &wi.tris, &|k| xl[k].clone());
        for e in forced {
            out.push(Event::LayerForced { wire: w, edge: e });
        }
        for e in unl {
            out.push(Event::Overflow { wire: w, edge: e });
        }
        for (j, &p) in wi.passes.iter().enumerate() {
            let e = st.passes[p].edge;
            let (t0, t1) = (wi.tris[j], wi.tris[j + 1]);
            let alone = pr.edge_demand(e, &[me]);
            let need = st.edge_need(e, None);
            if st.room_across(e, t0, t1, wi.net).is_some_and(|room| need > room) {
                out.push(Event::Overflow { wire: w, edge: e });
            }
            if st.history[e] > 0.0 {
                out.push(Event::Congestion { wire: w, edge: e, history: st.history[e] });
            }
            if let (Some(m0), Some(m1)) = (pr.allowed(t0, wi.net), pr.allowed(t1, wi.net)) {
                if let Some(amount_nm) = st.ignored_violation(e, m0.0 & m1.0, wi.net, alone) {
                    let [a, b] = pr.edges[e];
                    out.push(Event::IgnoredRuleViolated { rule: "copper_edge_clearance".into(), at: (pr.pos[a] + pr.pos[b]) * 0.5, amount_nm });
                }
            }
            if let Some(room) = st.room_best_layer(e, t0, t1, wi.net) {
                let ord = &st.edge_order[e];
                let g = ord.iter().position(|&x| x == p).unwrap_or(0);
                if g > 0 && g + 1 < ord.len() {
                    out.push(Event::Squeeze { wire: w, edge: e, slack_nm: room - need, pitch_nm: wi.w + wi.s });
                }
            }
        }
    }
    out
}

/// Factor totals and event counts of `evs` (wires counted: `wires`); `excess_mm` and `passes`
/// are routing quantities that are no factor event.
pub fn totals(evs: &[Event], wires: usize, excess_mm: f64, passes: usize) -> Totals {
    let fs = standard_factors();
    let amount = fs.iter().map(|f| evs.iter().map(|e| f.amount(e)).sum()).collect();
    let mut ev: BTreeMap<String, f64> = EVENT_KINDS.iter().map(|k| (k.to_string(), 0.0)).collect();
    let mut add = |k: &str, v: f64| *ev.get_mut(k).expect("known kind") += v;
    for e in evs {
        match e {
            Event::Length { mm, .. } => add("length_mm", *mm),
            Event::Via { count, .. } => add("vias", f64::from(*count)),
            Event::LayerForced { .. } => add("layer_forced", 1.0),
            Event::Crossing { same_net: false, .. } => add("crossings", 1.0),
            Event::Crossing { same_net: true, .. } => add("same_net_crossings", 1.0),
            Event::Squeeze { slack_nm, pitch_nm, .. } => {
                add("squeezes", 1.0);
                add("squeeze_amount", super::factors::squeeze_amount(*slack_nm, *pitch_nm));
            }
            Event::Overflow { .. } => add("overflows", 1.0),
            Event::BlocksPad { .. } => add("blocks_pad", 1.0),
            Event::Congestion { history, .. } => add("congestion", *history),
            Event::IgnoredRuleViolated { .. } => add("ignored_rule", 1.0),
            _ => {}
        }
    }
    add("excess_length_mm", excess_mm);
    add("passes", passes as f64);
    Totals { amount, events: ev, wires }
}

/// Totals of all alive wires of `st` that `keep` accepts.
pub fn state_totals(st: &State, vias: &[u32], ws0: f64, keep: &dyn Fn(WireId) -> bool) -> Totals {
    let evs = routing_events(st, vias, ws0, keep);
    let mut wires = 0;
    let mut excess = 0.0;
    let mut passes = 0;
    for w in 0..st.wires.len() {
        let wi = &st.wires[w];
        if !wi.alive || !keep(w) {
            continue;
        }
        wires += 1;
        passes += wi.passes.len();
        let pl = st.polyline(w);
        let len: f64 = pl.windows(2).map(|p| (p[1] - p[0]).len()).sum();
        excess += (len - (st.proj.pos[wi.b] - st.proj.pos[wi.a]).len()).max(0.0) / MM;
    }
    totals(&evs, wires, excess, passes)
}

/// Per engine wire and pass: the layers (bit mask) the pass may be on after P3 (module doc).
pub fn engine_layer_masks(st: &State, lr: &LayerReport) -> Vec<Vec<u64>> {
    let all: u64 = (1u64 << st.proj.n_layers) - 1;
    (0..st.wires.len())
        .map(|w| {
            let n = st.wires[w].passes.len();
            let ev: &[(usize, f64, i32)] = lr.events.get(w).map_or(&[], |v| v.as_slice());
            (0..n)
                .map(|j| {
                    let prev = ev.iter().rev().find(|x| x.0 <= j).map(|x| x.2);
                    let next = ev.iter().find(|x| x.0 > j).map(|x| x.2);
                    let bit = |l: i32| if l >= 0 { 1u64 << l } else { all };
                    match (prev, next) {
                        (Some(a), Some(b)) => bit(a) | bit(b),
                        (Some(a), None) | (None, Some(a)) => bit(a),
                        (None, None) => all,
                    }
                })
                .collect()
        })
        .collect()
}

/// Vias per wire from P3's labelled events: the label changes along the wire.
pub fn engine_vias(st: &State, lr: &LayerReport) -> Vec<u32> {
    (0..st.wires.len())
        .map(|w| lr.events.get(w).map_or(0, |ev| ev.windows(2).filter(|p| p[0].2 != p[1].2).count() as u32))
        .collect()
}

/// The loss of the engine routing `st` against the hand stretches `hand` (module doc), the
/// number of reproduced stretches and the number counted. `masks` from [`engine_layer_masks`];
/// `keep` selects the hand stretches that count. O(hand letters x engine runs of the first
/// letter).
pub fn loss(words: &Words, hand: &[Stretch], st: &State, masks: &[Vec<u64>], keep: &dyn Fn(i32) -> bool) -> (f64, usize, usize) {
    // Reduced word per engine wire (letters with the layer mask of their pass), and
    // (net, |letter|) -> (wire, index in the word).
    let all: u64 = (1u64 << st.proj.n_layers) - 1;
    let ws: Vec<Vec<(i64, u64)>> = (0..st.wires.len())
        .map(|w| {
            if !st.wires[w].alive {
                return Vec::new();
            }
            let m = |j: usize| masks.get(w).and_then(|v| v.get(j)).copied().unwrap_or(all);
            reduce(words.wire(st, w).into_iter().map(|(l, j)| (l, m(j))))
        })
        .collect();
    let mut at: BTreeMap<(i32, i64), Vec<(WireId, usize)>> = BTreeMap::new();
    for (w, word) in ws.iter().enumerate() {
        for (j, &(l, _)) in word.iter().enumerate() {
            at.entry((st.wires[w].net, l.abs())).or_default().push((w, j));
        }
    }
    let (mut n, mut hit) = (0usize, 0usize);
    for h in hand.iter().filter(|h| keep(h.net) && !h.edges.is_empty()) {
        n += 1;
        let bit = 1u64 << h.layer;
        let found = at.get(&(h.net, h.edges[0].abs())).is_some_and(|starts| {
            starts.iter().any(|&(w, j)| {
                let wd = &ws[w];
                let ok = |j: usize, l: i64| wd[j].0 == l && wd[j].1 & bit != 0;
                let m = h.edges.len();
                // Forward: the same letters; backward: the inverse letters in reverse order.
                let fwd = j + m <= wd.len() && h.edges.iter().enumerate().all(|(i, &l)| ok(j + i, l));
                let bwd = j + 1 >= m && h.edges.iter().enumerate().all(|(i, &l)| ok(j - i, -l));
                fwd || bwd
            })
        });
        hit += usize::from(found);
    }
    let l = if n == 0 { 0.0 } else { (n - hit) as f64 / n as f64 };
    (l, hit, n)
}

/// The hand stretches of the witness's State wires: maximal runs of passes on one layer, each
/// with the reduced word of its letters.
pub fn hand_stretches(words: &Words, w: &Witness, st: &State) -> Vec<Stretch> {
    let mut out = Vec::new();
    for (ci, wire) in w.wire_of.iter().enumerate() {
        let Some(wid) = *wire else { continue };
        let wk = &w.walked[ci];
        let net = st.wires[wid].net;
        let letters = words.wire(st, wid);
        let mut j = 0;
        while j < wk.passes.len() {
            let layer = wk.passes[j].layer;
            let mut k = j;
            while k + 1 < wk.passes.len() && wk.passes[k + 1].layer == layer {
                k += 1;
            }
            let word = reduce(letters.iter().filter(|x| x.1 >= j && x.1 <= k).copied()).into_iter().map(|x| x.0).collect();
            out.push(Stretch { net, layer, edges: word });
            j = k + 1;
        }
    }
    out
}

/// Prepare one training board: the R0 witness of `hand`, the connections of `input` (the same
/// board stripped of tracks and vias) on the witness board, and the hand reference. `tier`:
/// count only wires of this tier.
pub fn prepare(name: &str, hand: &BoardModel, input: &BoardModel, tier: Option<u8>) -> Result<Sample, String> {
    if hand.pads.len() != input.pads.len() || hand.pads.iter().zip(&input.pads).any(|(a, b)| a.center != b.center || a.net != b.net) {
        return Err(format!("{name}: the pads of the hand and input boards differ"));
    }
    let w = witness::run(hand, None, &Spots::default(), &WitnessOpts::default())?;
    let opts = Options::default();
    let b: &Board = &w.board;
    let signal: Vec<u32> = input.layers.iter().filter(|l| l.routable).map(|l| l.id).collect();
    let dec = decompose(input, &opts, &signal);
    let diff = super::core::diff_pair_nets(input);
    let mut obs_of_pad: BTreeMap<usize, usize> = BTreeMap::new();
    for (o, &p) in b.pad_of.iter().enumerate().filter(|(_, &p)| p != usize::MAX) {
        obs_of_pad.insert(p, o);
    }
    let mut conns = Vec::new();
    let mut skipped = 0;
    let mut tier_of_net: BTreeMap<i32, u8> = BTreeMap::new();
    let mut partial: BTreeSet<i32> = BTreeSet::new();
    for c in &dec.connections {
        let t = tier_of_net.entry(c.net).or_insert(c.tier);
        *t = (*t).min(c.tier);
        let pads = match (&c.from.kind, &c.to.kind) {
            (TermKind::Pad(pa), TermKind::Pad(pb)) if !diff.contains(&c.net) => obs_of_pad.get(pa).zip(obs_of_pad.get(pb)),
            _ => None,
        };
        let Some((&oa, &ob)) = pads else {
            skipped += 1;
            partial.insert(c.net);
            continue;
        };
        let (Some(va), Some(vb)) = (b.centre[oa], b.centre[ob]) else {
            skipped += 1;
            partial.insert(c.net);
            continue;
        };
        conns.push((Conn { net: c.net, pa: oa, pb: ob, a: va, b: vb, w: c.width as f64, s: c.clearance as f64 }, c.tier));
    }
    let ws0 = (input.rules.default.width + input.rules.default.clearance) as f64;
    let (st, _, _) = witness::sub_state(b, hand, &w.chains, &w.walked);
    let mut vias = vec![0u32; st.wires.len()];
    for (ci, wire) in w.wire_of.iter().enumerate() {
        if let Some(wid) = *wire {
            vias[wid] = w.chains[ci].vias as u32;
        }
    }
    let counted: BTreeSet<i32> =
        conns.iter().map(|c| c.0.net).filter(|n| !partial.contains(n) && tier.is_none_or(|t| tier_of_net.get(n).copied().unwrap_or(3) == t)).collect();
    let totals = state_totals(&st, &vias, ws0, &|wid| counted.contains(&st.wires[wid].net));
    let words = Words::new(&b.proj);
    let stretches = hand_stretches(&words, &w, &st);
    let mut on_hand: Vec<Vec<i32>> = vec![Vec::new(); b.proj.edges.len()];
    for p in &st.passes {
        on_hand[p.edge].push(st.wires[p.wire].net);
    }
    for v in &mut on_hand {
        v.sort_unstable();
        v.dedup();
    }
    drop(st);
    let hand = HandRef { totals, stretches, on_hand };
    Ok(Sample { name: name.to_string(), w, words, conns, ws0, tier_of_net, counted, model: input.clone(), opts, hand, skipped })
}

/// The engine's routing of a sample on the witness board, as far as learning needs it.
#[derive(Clone, Debug, Serialize)]
pub struct EngineRun {
    pub totals: Totals,
    pub loss: f64,
    pub reproduced: usize,
    pub stretches: usize,
    pub routed: usize,
    pub failed: usize,
    pub vias: usize,
    pub same_layer: usize,
    pub unlayerable: usize,
}

/// P2 + uncross + P3 of `s` on the witness board with the prices of `reg` (learned tier:
/// `learned_tier`, the other connections priced by `base`), with the loss-augmentation hook when
/// `margin_mm > 0`. See the module doc for what is and is not the engine's pipeline.
pub fn route_lite(s: &Sample, reg: &Registry, base: &Registry, tier: Option<u8>, margin_mm: f64) -> EngineRun {
    let b = &s.w.board;
    let proj = &b.proj;
    let mut st = State::new(proj);
    let len_of = |c: &Conn| (proj.pos[c.b] - proj.pos[c.a]).len();
    let mut order: Vec<usize> = (0..s.conns.len()).collect();
    let cs = &s.conns;
    order.sort_by(|&i, &j| cs[j].0.w.total_cmp(&cs[i].0.w).then(len_of(&cs[i].0).total_cmp(&len_of(&cs[j].0))).then(i.cmp(&j)));
    let on_hand = &s.hand.on_hand;
    let hook = move |net: i32, e: EdgeId| if on_hand[e].binary_search(&net).is_ok() { margin_mm } else { 0.0 };
    let hook_ref: Option<&PassHook> = if margin_mm > 0.0 { Some(&hook) } else { None };
    let mut wire_conn: Vec<usize> = Vec::new();
    let detour = s.opts.detour_factor;
    let route_one = |st: &mut State, ci: usize, cap: usize, wire_conn: &mut Vec<usize>| -> bool {
        let (c, t) = &s.conns[ci];
        let r = if tier.is_none_or(|x| x == *t) { reg } else { base };
        let straight = len_of(c).max(1.0);
        for (attempt, cross_scale) in [1.0, 0.25, 0.0].into_iter().enumerate() {
            let mut w = InitWeights { max_expansions: cap, ..InitWeights::from_registry(r, &WireCtx { net: c.net, tier: *t }) };
            w.cross_mm *= cross_scale;
            w.smd_over_mm *= cross_scale;
            w.forced_layer_mm *= cross_scale;
            let Some((wid, _)) = st.route_conn_hooked(c, &w, (c.w + c.s) / s.ws0, hook_ref) else { return false };
            wire_conn.push(ci);
            let len: f64 = st.polyline(wid).windows(2).map(|x| (x[1] - x[0]).len()).sum();
            if detour <= 0.0 || len <= detour * straight || attempt == 2 {
                return true;
            }
            st.remove(wid);
        }
        true
    };
    let cap = 2_000_000usize;
    let mut failed = Vec::new();
    for &ci in &order {
        if !route_one(&mut st, ci, cap, &mut wire_conn) {
            failed.push(ci);
        }
    }
    st.uncross_all(50);
    let mut still = 0;
    for &ci in &failed {
        if !route_one(&mut st, ci, cap * 4, &mut wire_conn) {
            still += 1;
        }
    }
    st.uncross_all(50);
    let p3 = if tier.is_none() { reg } else { base };
    let lp = LayerParams {
        via_mm: p3.price(&Event::Via { wire: 0, count: 1, at: None }, &WireCtx::default()),
        via_site_r: 0.5 * s.model.rules.default.via_d as f64 + s.model.rules.default.clearance as f64,
        n_signal: b.n_signal,
        plane_mm: p3.price(&Event::PlaneLength { wire: 0, layer: b.n_signal, mm: 1.0 }, &WireCtx::default()),
        plane_cut: p3.price(&Event::PlaneCut { wire: 0, layer: b.n_signal }, &WireCtx::default()),
        cap_mm: p3.price(&Event::Overflow { wire: 0, edge: 0 }, &WireCtx::default()),
        seed: s.opts.seed,
        ..LayerParams::default()
    };
    let packer = Packer::for_model(&s.model, &s.opts, proj);
    let lr = layers::assign_with(&st, &|ob| b.smd_layer(ob), &lp, Some(&packer));
    let vias = engine_vias(&st, &lr);
    let tier_net = |net: i32| s.counted.contains(&net);
    let totals = state_totals(&st, &vias, s.ws0, &|w| tier_net(st.wires[w].net));
    let masks = engine_layer_masks(&st, &lr);
    let (l, hit, n) = loss(&s.words, &s.hand.stretches, &st, &masks, &tier_net);
    EngineRun {
        totals,
        loss: l,
        reproduced: hit,
        stretches: n,
        routed: st.wires.iter().filter(|w| w.alive).count(),
        failed: still,
        vias: lr.vias,
        same_layer: lr.same_layer,
        unlayerable: lr.unlayerable.len(),
    }
}

/// Settings of the subgradient loop.
#[derive(Clone, Debug, Serialize)]
pub struct LearnOpts {
    /// Step size (mm of weight per unit of per-wire total difference), divided by sqrt(t + 1).
    pub eta: f64,
    /// Regularizer (the ½ lambda ||w||² of the QP).
    pub lambda: f64,
    /// Stop when no weight moves more than this (mm).
    pub eps: f64,
    /// Most iterations.
    pub iters: usize,
    /// Loss-augmentation price (mm) of a pass through an edge the hand's net uses; 0 = off.
    pub margin_mm: f64,
    /// Factors to learn (the others keep their default weight).
    pub learn: Vec<String>,
    /// Only wires of this tier count, and only their connections take the learned weights.
    pub tier: Option<u8>,
}

impl Default for LearnOpts {
    fn default() -> Self {
        LearnOpts {
            eta: 5.0,
            lambda: 1e-3,
            eps: 0.05,
            iters: 12,
            margin_mm: 2.0,
            learn: ["via", "crossing", "squeeze"].iter().map(|s| s.to_string()).collect(),
            tier: None,
        }
    }
}

/// One iteration of [`learn`].
#[derive(Clone, Debug, Serialize)]
pub struct Iteration {
    /// Weights the engine routed with in this iteration.
    pub weights: BTreeMap<String, f64>,
    /// Per training board: loss of the loss-augmented engine routing.
    pub loss: Vec<f64>,
    /// Per learned factor: the subgradient.
    pub grad: BTreeMap<String, f64>,
    pub step: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct LearnResult {
    pub boards: Vec<String>,
    pub iterations: Vec<Iteration>,
    /// The learned weights: the average of the iterates.
    pub weights: BTreeMap<String, f64>,
    /// The last iterate.
    pub last: BTreeMap<String, f64>,
    pub converged: bool,
    pub opts: LearnOpts,
}

/// The weights of the factors in `names` from `reg`.
fn weights_of(reg: &Registry, names: &[String]) -> BTreeMap<String, f64> {
    names.iter().filter_map(|n| reg.weight(n).map(|w| (n.clone(), w))).collect()
}

fn registry(w: &BTreeMap<String, f64>) -> Registry {
    let cfg = FactorConfig { weight: w.clone(), ..FactorConfig::default() };
    Registry::from_config(standard_factors(), &cfg).0
}

/// Every engine run of one iteration, the boards in parallel threads, merged in board order.
fn route_all(samples: &[Sample], reg: &Registry, base: &Registry, o: &LearnOpts) -> Vec<EngineRun> {
    std::thread::scope(|sc| {
        let hs: Vec<_> = samples.iter().map(|s| sc.spawn(move || route_lite(s, reg, base, o.tier, o.margin_mm))).collect();
        hs.into_iter().map(|h| h.join().expect("route thread")).collect()
    })
}

/// The subgradient loop of maximum margin planning over `samples` (module doc).
pub fn learn(samples: &[Sample], o: &LearnOpts) -> Result<LearnResult, String> {
    let hand: Vec<(Vec<f64>, usize)> = samples.iter().map(|s| (s.hand.totals.amount.clone(), s.hand.totals.wires)).collect();
    let base = Registry::standard();
    let route = |reg: &Registry| -> Vec<(Vec<f64>, f64)> { route_all(samples, reg, &base, o).into_iter().map(|r| (r.totals.amount, r.loss)).collect() };
    let mut r = learn_loop(&hand, &route, o)?;
    r.boards = samples.iter().map(|s| s.name.clone()).collect();
    Ok(r)
}

/// The engine side of [`learn_loop`]: for the registry of the current weights, the factor totals
/// and the loss per training board.
pub type RouteFn<'a> = dyn Fn(&Registry) -> Vec<(Vec<f64>, f64)> + 'a;

/// The loop itself: `hand[b]` = (factor totals, wires counted) of training board b; `route` gives,
/// for the registry of the current weights, the (loss-augmented) engine's factor totals and loss
/// per board. See the module doc for the step.
pub fn learn_loop(hand: &[(Vec<f64>, usize)], route: &RouteFn, o: &LearnOpts) -> Result<LearnResult, String> {
    let fs = standard_factors();
    let idx: Vec<usize> = o
        .learn
        .iter()
        .map(|n| fs.iter().position(|f| f.name() == n).ok_or_else(|| format!("unknown factor {n:?}")))
        .collect::<Result<_, _>>()?;
    let mut w = weights_of(&Registry::standard(), &o.learn);
    let mut sum: BTreeMap<String, f64> = w.keys().map(|k| (k.clone(), 0.0)).collect();
    let mut iterations = Vec::new();
    let mut converged = false;
    for t in 0..o.iters {
        let runs = route(&registry(&w));
        let mut grad = BTreeMap::new();
        for (n, &i) in o.learn.iter().zip(&idx) {
            let d: f64 = hand.iter().zip(&runs).map(|((h, nw), (e, _))| (h[i] - e[i]) / (*nw).max(1) as f64).sum::<f64>() / hand.len().max(1) as f64;
            grad.insert(n.clone(), o.lambda * w[n] + d);
        }
        let eta = o.eta / ((t + 1) as f64).sqrt();
        let mut step: f64 = 0.0;
        let before = w.clone();
        for (n, g) in &grad {
            let v = w.get_mut(n).expect("learned");
            let nv = (*v - eta * g).max(0.0);
            step = step.max((nv - *v).abs());
            *v = nv;
        }
        for (k, v) in &w {
            *sum.get_mut(k).expect("learned") += v;
        }
        iterations.push(Iteration { weights: before, loss: runs.iter().map(|r| r.1).collect(), grad, step });
        if step < o.eps {
            converged = true;
            break;
        }
    }
    let n = iterations.len().max(1) as f64;
    let weights = sum.into_iter().map(|(k, v)| (k, v / n)).collect();
    Ok(LearnResult { boards: Vec::new(), iterations, weights, last: w, converged, opts: o.clone() })
}

/// The learned weights as a `topor.toml [factors]` snippet (the existing weight format). Never
/// applied by default; a tier-scoped result is commented out (module doc).
pub fn toml_snippet(w: &BTreeMap<String, f64>, tier: Option<u8>, note: &str) -> String {
    let body = w.iter().map(|(k, v)| format!("{k} = {v:.3}")).collect::<Vec<_>>().join(", ");
    let mut s = format!("# Learned by the S3b learning loop ({note}). Not applied by default:\n# copy into topor.toml to try it.\n");
    match tier {
        None => s.push_str(&format!("[factors]\nweight = {{ {body} }}\n")),
        Some(t) => s.push_str(&format!(
            "# Learned for tier {t} only. [factors] scope = \"tier:{t}\" would turn these factors off for\n# the other tiers, so this is not written as a setting:\n# weight = {{ {body} }}\n"
        )),
    }
    s
}
