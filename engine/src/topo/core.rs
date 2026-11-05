//! The `route()` entry of the topological core, stage S2a ("first copper"; S2b: any number of signal layers):
//! BoardModel -> projection (`board`) -> P2 initial routing (`init`, pad to pad, widest first)
//! -> P3 layer assignment (`layers`, Polubasov) -> minimal drawing (`draw`) -> `RouteResult`.
//!
//! Every connection ends in one of three states, none of them silent:
//! * routed: copper emitted, checked by the [`Checker`] against the board and all earlier copper;
//! * failed with a reason (P7 wording): no topological path, a crossing that cannot be layered,
//!   no via site, a drawing the checker rejects, a plane pad without a via site, an unsupported
//!   terminal, a differential pair ("not in the rough version"), time limit;
//! * never emitted: a different-net crossing on one layer (the lower-tier wire stays unrouted).
//!
//! Boards with fewer than two signal layers report every connection failed with a reason.
//! Deterministic: no randomness besides the seeded P3 local search, Vec + index ids throughout.
//!
//! Complexity: dominated by P2 (one A* per connection over the gap graph, capped by
//! `max_expansions`) and P3 (reductions linear in the domain graph, local search on the rest).

use super::archive::{table_text, Archive, Criteria, Objective, Variant};
use super::board::{self, BuildOpts};
use super::descent::{descend_labels, descend_topo};
use super::moves::{Labels, Tally, Topo};
use super::draw::{plan, Diag, Drawer, Plan, WireOut};
use super::factors::{standard_factors, Event, Finding, Registry, Report, WireCtx};
use super::init::{Conn, InitWeights};
use super::pack::Packer;
use super::refine::CapModel;
use super::layers::{self, LayerParams};
use super::planes;
use super::state::{ObsId, State, WireId};
use crate::check::Checker;
use crate::control::{cancelled, RunControl};
use crate::decompose::{decompose, Connection, TermKind, TermRef};
use crate::geom::Vec2;
use crate::model::{BoardModel, Item, Options};
use crate::route::{kicad, term_out, unrouted, ConnReport, NetRouteReport, RipReport, RouteResult};
use crate::vias::via_sizes;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

const MM: f64 = 1e6;

/// Failure reason of a differential pair (counted separately in the gate).
pub const DIFF_PAIR: &str = "diff pair: not in the rough version";

/// Nets that are halves of a differential pair by name (`_P`/`_N`, `+`/`-`, `P`/`N` suffix; both
/// halves present): the plugin's own rule (`priority._pair_key`). O(N log N).
pub fn diff_pair_nets(model: &BoardModel) -> BTreeSet<i32> {
    let key = |name: &str| -> Option<(String, u8)> {
        let seg = name.rsplit('/').next().unwrap_or(name);
        let mut s = seg.to_string();
        if let Some(at) = s.find('@') {
            let digits = s[at + 1..].chars().take_while(char::is_ascii_digit).count();
            s.replace_range(at..at + 1 + digits, "");
        }
        for (pos, neg) in [("_P", "_N"), ("+", "-"), ("P", "N")] {
            if let Some(k) = s.strip_suffix(pos) {
                return Some((k.to_string(), 0));
            }
            if let Some(k) = s.strip_suffix(neg) {
                return Some((k.to_string(), 1));
            }
        }
        None
    };
    let mut by: BTreeMap<String, [Option<i32>; 2]> = BTreeMap::new();
    for n in &model.nets {
        if let Some((k, side)) = key(&n.name) {
            by.entry(k).or_default()[side as usize] = Some(n.code);
        }
    }
    by.values().filter_map(|h| h[0].zip(h[1])).flat_map(|(a, b)| [a, b]).collect()
}

/// Record the drawn wire `wo` as the outcome of connection `ci`.
fn settle(out: &mut [Outcome], necking: &mut Vec<(usize, Event)>, ci: usize, w: WireId, wo: &WireOut, b: &board::Board, st: &State) {
    let o = &mut out[ci];
    o.routed = true;
    o.reason.clear();
    o.length_nm = wo.length_nm;
    o.layers = wo.runs.iter().map(|(l, _)| b.signal_layers[*l]).collect();
    o.layers.dedup();
    o.vias = wo.vias.clone();
    o.items = wo.items.clone();
    o.neck_nm = wo.necks.iter().map(|n| n.2).sum();
    o.neck_n = wo.necks.len();
    o.neck_w = wo.necks.iter().map(|n| n.1).fold(None, |a: Option<f64>, x| Some(a.map_or(x, |y| y.min(x))));
    for n in &wo.necks {
        necking.push((ci, Event::Necking { wire: w, from_nm: st.wires[w].w, to_nm: n.1, mm: n.2 / MM }));
    }
}

/// What happened to one connection.
#[derive(Clone, Debug, Default)]
struct Outcome {
    routed: bool,
    reason: String,
    items: Vec<Item>,
    vias: Vec<(Vec2, crate::vias::ViaSize)>,
    layers: Vec<u32>,
    length_nm: f64,
    stub_nm: f64,
    /// Necked pad exits: total length (nm) and the narrowest width (nm).
    neck_nm: f64,
    neck_w: Option<f64>,
    /// Necked stretches (TopoR's shrinkages).
    neck_n: usize,
    /// Straight pad-to-pad distance (nm), the detour reference.
    ghost_nm: f64,
}

fn fail(o: &mut [Outcome], i: usize, reason: impl Into<String>) {
    if !o[i].routed && !o[i].reason.is_empty() {
        return;
    }
    o[i].routed = false;
    o[i].reason = reason.into();
}

/// Route `model` with the topological core (see the module doc).
///
/// Reproducible: every stop that changes the result is a work cap (`topo_rounds`, `max_ripped`,
/// `max_expansions`, `reroute_rounds`, the move cap of the descent). The time budget
/// (`wall_clock_s`) is only an emergency stop; when it fires the result says so in a warning
/// ([`TIME_HIT`]), because then it depends on the machine's speed and load.
pub fn route_topo(model: &BoardModel, opts: &Options, ctl: &RunControl) -> RouteResult {
    let t0 = Instant::now();
    let obj = Objective::parse(&opts.objective, opts.mm_per_via);
    let mut arch: Archive<RouteResult> = Archive::new(opts.variants);
    let offer = |arch: &mut Archive<RouteResult>, round: usize, ro: &RoundOut| {
        if let Some((c, r)) = &ro.pre {
            arch.offer(Variant { label: format!("round {round} as drawn"), crit: *c, payload: r.clone() }, &obj);
        }
        arch.offer(Variant { label: format!("round {round}"), crit: ro.crit, payload: ro.result.clone() }, &obj);
    };
    let (r1, h0, mut mem) = route_round(model, opts, ctl, &[], &Memory::default(), 1);
    offer(&mut arch, 1, &r1);
    let mut time_hit = r1.result.timed_out;
    let mut stop_reason = r1.result.stop_reason.clone();
    let first = r1.result.routed;
    let mut best_routed = first;
    let total = r1.result.connections_total;
    drop(r1);
    let over_budget = || opts.wall_clock_s > 0.0 && t0.elapsed().as_secs_f64() >= opts.wall_clock_s;
    // Feedback to the topology (plan S2c item 4): the rooms the drawing found too small are
    // lowered by what was missing, and the whole route is made again (at most `topo_rounds`
    // rounds in all, while each round routes more). Every round offers its variants.
    let mut all: BTreeMap<(usize, usize), f64> = h0.iter().map(|&(e, l, r)| ((e, l), r)).collect();
    let mut round_note = Vec::new();
    let mut stall = 0usize;
    for round in 2..=opts.topo_rounds.max(1) {
        if !opts.drawing_feedback || all.is_empty() || stop_reason != "converged" {
            break;
        }
        if over_budget() {
            time_hit = true;
            break;
        }
        let mut o2 = opts.clone();
        if opts.wall_clock_s > 0.0 {
            o2.wall_clock_s = (opts.wall_clock_s - t0.elapsed().as_secs_f64()).max(1.0);
        }
        let hv: Hints = all.iter().map(|(&(e, l), &r)| (e, l, r)).collect();
        let (r2, h2, mem2) = route_round(model, &o2, ctl, &hv, &mem, round);
        time_hit |= r2.result.timed_out;
        stop_reason = r2.result.stop_reason.clone();
        mem = mem2;
        round_note.push(format!("round {round}: {} cut rooms lowered, routed {} of {}", hv.len(), r2.result.routed, r2.result.connections_total));
        for (e, l, r) in h2 {
            let v = all.entry((e, l)).or_insert(r);
            *v = v.min(r);
        }
        let routed = r2.result.routed;
        offer(&mut arch, round, &r2);
        if routed > best_routed {
            best_routed = routed;
            stall = 0;
        } else {
            // The history stays: a repeat that routes no more can still open the way for the next.
            stall += 1;
            if stall >= 2 {
                break;
            }
        }
    }
    let pick = arch.pick(&obj).expect("round 1 is always offered");
    let mut best = arch.items[pick].payload.clone();
    if opts.stitch_islands {
        stitch_pours(model, opts, &mut best, (opts.wall_clock_s > 0.0).then(|| t0 + std::time::Duration::from_secs_f64(opts.wall_clock_s)));
    }
    best.variants = arch.table(Some(pick));
    best.warnings.push(format!("{} ({} offered)", table_text(&best.variants, &obj), arch.offered));
    if !round_note.is_empty() {
        best.warnings.push(format!(
            "topology feedback (rooms lowered by the violation depth, whole route repeated): round 1 routed {} of {}; {}; picked {} (routed {}, rescued {})",
            first,
            total,
            round_note.join("; "),
            arch.items[pick].label,
            best.routed,
            best.routed as i64 - first as i64
        ));
    }
    best.runtime_s = t0.elapsed().as_secs_f64();
    if time_hit {
        best.warnings.push(format!("{TIME_HIT} (run.timeout_s = {:.0} s stopped the run before its work caps)", opts.wall_clock_s));
    }
    best
}

/// Pour stitching (D4, [`super::stitch`]) on the picked result: the stitches join its items
/// (`item_conn` = `usize::MAX`: they belong to no connection) and a summary line plus one line
/// per cut-off piece go to the warnings.
fn stitch_pours(model: &BoardModel, opts: &Options, best: &mut RouteResult, deadline: Option<Instant>) {
    let pour_nets: BTreeSet<i32> = model.pours.iter().map(|z| z.net).collect();
    if pour_nets.is_empty() {
        return;
    }
    // A label can name several pads (a switch with two pads "2"): the one at the terminal.
    let pad_of = |label: &str, at: [i64; 2]| {
        let q = Vec2 { x: at[0] as f64, y: -(at[1] as f64) };
        (0..model.pads.len())
            .filter(|&i| format!("{}.{}", model.pads[i].ref_, model.pads[i].number) == label)
            .min_by(|&a, &b| (model.pads[a].center.to_vec2() - q).len().total_cmp(&(model.pads[b].center.to_vec2() - q).len()).then(a.cmp(&b)))
    };
    let mut routed_pads: Vec<usize> = Vec::new();
    for c in best.connections.iter().filter(|c| c.status == "routed" && pour_nets.contains(&c.net)) {
        for t in [&c.from, &c.to] {
            if t.kind == "pad" {
                if let Some(p) = pad_of(&t.label, t.at) {
                    routed_pads.push(p);
                }
            }
        }
    }
    routed_pads.sort_unstable();
    routed_pads.dedup();
    let (reg, _) = Registry::from_config(standard_factors(), &opts.factors);
    let t = Instant::now();
    let s = super::stitch::stitch(model, opts, &reg, &best.items, &routed_pads, deadline);
    if s.before == 0 {
        return;
    }
    best.warnings.push(format!(
        "pour stitching: {} pour pieces cut off by the routed copper before, {} after; {} island vias, {} pad stubs ({:.2} s)",
        s.before,
        s.after,
        s.island_vias,
        s.pad_stubs,
        t.elapsed().as_secs_f64()
    ));
    best.warnings.extend(s.notes);
    for it in s.items {
        best.items.push(it);
        best.item_conn.push(usize::MAX);
    }
}

/// Outcomes and necked stretches of a route at one moment (the variant before the via reduction).
type Snapshot = (Vec<Outcome>, Vec<(usize, Event)>);

/// One whole route: the final result, the variant before the via reduction (when it ran), and
/// their criteria.
struct RoundOut {
    result: RouteResult,
    crit: Criteria,
    pre: Option<(Criteria, RouteResult)>,
}

impl RoundOut {
    fn unrouted(r: RouteResult) -> RoundOut {
        RoundOut { crit: Criteria { errors: r.failed, ..Criteria::default() }, result: r, pre: None }
    }
}

/// Warning when the time budget, the emergency stop, ended any part of the run.
pub const TIME_HIT: &str = "time budget hit: result may differ between runs";

/// What a whole route remembers for the next one (negotiated rip-up and reroute across repeats).
#[derive(Clone, Debug, Default)]
struct Memory {
    /// Per edge: rounds in which it was over-full (the `congestion` history all wires pay).
    edge: Vec<f64>,
    /// Per connection id: the edges of the sleeve of its rejected drawing and how often, which
    /// only that connection pays for (it is the one being ripped up and rerouted).
    own: BTreeMap<usize, BTreeMap<usize, f64>>,
}

/// Lowered rooms `(edge, layer, room nm)` that a second round applies.
type Hints = Vec<(usize, usize, f64)>;

/// One whole route. `hist0` is the per-edge over-full history of earlier rounds (empty = none);
/// the third result is the history at the end of this one.
fn route_round(model: &BoardModel, opts: &Options, ctl: &RunControl, hints: &[(usize, usize, f64)], mem: &Memory, round: usize) -> (RoundOut, Hints, Memory) {
    // TOPO_TIMES=1 (debug): phase times on stderr; never part of the result.
    let times = std::env::var_os("TOPO_TIMES").is_some();
    let tlog = |what: &str, t: Instant| {
        if times {
            eprintln!("[topo round {round}] {what}: {:.2} s", t.elapsed().as_secs_f64());
        }
    };
    let hist0: &[f64] = &mem.edge;
    let t0 = Instant::now();
    let deadline = (opts.wall_clock_s > 0.0).then(|| t0 + std::time::Duration::from_secs_f64(opts.wall_clock_s));
    let expired = || cancelled() || ctl.is_cancelled() || deadline.is_some_and(|d| Instant::now() >= d);
    let signal: Vec<u32> = model.layers.iter().filter(|l| l.routable).map(|l| l.id).collect();
    if signal.len() < 2 {
        let why = format!("the topological core routes boards with two or more signal layers; this board has {}", signal.len());
        return (RoundOut::unrouted(unrouted(model, opts, &why)), Vec::new(), Memory::default());
    }
    ctl.update("setup", 0, 0, 0, 0, 0.0);
    let dec = decompose(model, opts, &signal);
    let mut warnings: Vec<String> = dec.warnings.clone();
    for (net, r) in &dec.skipped {
        warnings.push(format!("net {net} skipped: {r}"));
    }
    let (reg, problems) = Registry::from_config(standard_factors(), &opts.factors);
    warnings.extend(problems);
    let mut b = match board::build(model, &BuildOpts { existing_vias: true, keepouts: true, plane_layers: opts.plane_signals != "forbidden", ..BuildOpts::default() }) {
        Ok(b) => b,
        Err(e) => return (RoundOut::unrouted(unrouted(model, opts, &format!("projection failed: {e}"))), Vec::new(), Memory::default()),
    };
    for &(e, l, room) in hints {
        if let Some(r) = b.proj.room.get_mut(e).and_then(|r| r.get_mut(l)) {
            *r = r.min(room);
        }
    }
    let nconn = dec.connections.len();
    let mut out: Vec<Outcome> = vec![Outcome::default(); nconn];
    let diff = diff_pair_nets(model);
    let mut obs_of_pad: BTreeMap<usize, ObsId> = BTreeMap::new();
    for (o, &p) in b.pad_of.iter().enumerate().filter(|(_, &p)| p != usize::MAX) {
        obs_of_pad.insert(p, o);
    }
    // Sort the connections: pad-to-pad ones into P2, pour ones into the plane stage.
    let mut conns: Vec<Conn> = Vec::new();
    let mut conn_of: Vec<usize> = Vec::new();
    let mut plane_reqs: Vec<planes::Req> = Vec::new();
    for c in &dec.connections {
        let i = c.id;
        let (ta, tb) = (&c.from.kind, &c.to.kind);
        out[i].ghost_nm = (c.from.at - c.to.at).len();
        if diff.contains(&c.net) {
            fail(&mut out, i, DIFF_PAIR);
            continue;
        }
        match (ta, tb) {
            (TermKind::Pad(pa), TermKind::Pad(pb)) => {
                let (Some(&oa), Some(&ob)) = (obs_of_pad.get(pa), obs_of_pad.get(pb)) else {
                    fail(&mut out, i, "a pad has no copper on a signal layer");
                    continue;
                };
                let (Some(va), Some(vb)) = (b.centre[oa], b.centre[ob]) else {
                    fail(&mut out, i, "a pad has no net centre vertex");
                    continue;
                };
                conn_of.push(i);
                conns.push(Conn { net: c.net, pa: oa, pb: ob, a: va, b: vb, w: c.width as f64, s: c.clearance as f64 });
            }
            (TermKind::Pad(pa), TermKind::Pour(zi)) => match obs_of_pad.get(pa) {
                Some(&oa) => plane_reqs.push(planes::Req {
                    conn: i,
                    pad: *pa,
                    obs: oa,
                    net: c.net,
                    w: c.width as f64,
                    pour: *zi,
                }),
                None => fail(&mut out, i, "a pad has no copper on a signal layer"),
            },
            _ => fail(
                &mut out,
                i,
                "terminal is an existing via, track end or stub: not supported by the rough topological core (S2a)",
            ),
        }
    }
    ctl.update("route", 0, 0, nconn, 0, t0.elapsed().as_secs_f64());
    // ---- P2 ----
    let proj = &b.proj;
    let mut st = State::new(proj);
    let ws0 = (model.rules.default.width + model.rules.default.clearance) as f64;
    let tier_of = |i: usize| dec.connections[i].tier;
    let weights = |rg: &Registry, c: &Conn, tier: u8, cap: usize| InitWeights { max_expansions: cap, ..InitWeights::from_registry(rg, &WireCtx { net: c.net, tier }) };
    let len_of = |c: &Conn| (proj.pos[c.b] - proj.pos[c.a]).len();
    let mut order: Vec<usize> = (0..conns.len()).collect();
    order.sort_by(|&i, &j| {
        conns[j].w.total_cmp(&conns[i].w).then(len_of(&conns[i]).total_cmp(&len_of(&conns[j]))).then(i.cmp(&j))
    });
    let mut wire_conn: Vec<usize> = Vec::new(); // wire id -> index into conns
    let mut p2_failed: Vec<usize> = Vec::new();
    let mut timed_out = false;
    let route_one_raw = |st: &mut State, ci: usize, cap: usize, wire_conn: &mut Vec<usize>, rg: &Registry| -> bool {
        let c = &conns[ci];
        let straight = len_of(c).max(1.0);
        // A path longer than `detour_factor` x the straight distance is a detour: search again with
        // cheaper crossings and layer fixings (P3 resolves a crossing with a via; a detour of many times the
        // connection's length is worse than that), the cheapest last.
        for (attempt, cross_scale) in [1.0, 0.25, 0.0].into_iter().enumerate() {
            let mut w = weights(rg, c, tier_of(conn_of[ci]), cap);
            w.cross_mm *= cross_scale;
            w.smd_over_mm *= cross_scale;
            w.forced_layer_mm *= cross_scale;
            let Some((wid, _)) = st.route_conn(c, &w, (c.w + c.s) / ws0) else { return false };
            wire_conn.push(ci);
            let len: f64 = st.polyline(wid).windows(2).map(|x| (x[1] - x[0]).len()).sum();
            if opts.detour_factor <= 0.0 || len <= opts.detour_factor * straight || attempt == 2 {
                return true;
            }
            st.remove(wid);
        }
        true
    };
    // A connection whose sleeve the drawing rejected earlier pays a history on its old edges.
    let route_one = |st: &mut State, ci: usize, cap: usize, wire_conn: &mut Vec<usize>, rg: &Registry| -> bool {
        let Some(own) = mem.own.get(&conn_of[ci]) else { return route_one_raw(st, ci, cap, wire_conn, rg) };
        let saved = st.history.clone();
        for (&e, &n) in own {
            st.history[e] += n;
        }
        let r = route_one_raw(st, ci, cap, wire_conn, rg);
        st.history = saved;
        r
    };
    let cap = opts.max_expansions.max(1);
    for &ci in &order {
        if expired() {
            timed_out = true;
            fail(&mut out, conn_of[ci], "time limit reached before this connection was routed");
            continue;
        }
        if !route_one(&mut st, ci, cap, &mut wire_conn, &reg) {
            p2_failed.push(ci);
        }
    }
    let swaps = st.uncross_all(50);
    // One retry of the failed connections with a larger search cap.
    let mut still: Vec<usize> = Vec::new();
    for &ci in &p2_failed {
        if expired() || !route_one(&mut st, ci, cap * 4, &mut wire_conn, &reg) {
            still.push(ci);
        }
    }
    if still.len() < p2_failed.len() {
        st.uncross_all(50);
    }
    for &ci in &still {
        fail(&mut out, conn_of[ci], "no topological path between the pads (search exhausted); the pad is enclosed or the retry cap was reached");
    }
    debug_assert!(st.check().is_ok(), "{:?}", st.check());
    ctl.update("route", 1, 0, nconn, 0, t0.elapsed().as_secs_f64());
    // ---- P3 ----
    let lp = LayerParams {
        via_mm: reg.price(&Event::Via { wire: 0, count: 1, at: None }, &WireCtx::default()),
        via_site_r: 0.5 * model.rules.default.via_d as f64 + model.rules.default.clearance as f64,
        n_signal: b.n_signal,
        plane_mm: reg.price(&Event::PlaneLength { wire: 0, layer: b.n_signal, mm: 1.0 }, &WireCtx::default()),
        plane_cut: reg.price(&Event::PlaneCut { wire: 0, layer: b.n_signal }, &WireCtx::default()),
        // TOPO_NOCAP=1 (debug): assign layers for vias only, to compare with the capacity refinement.
        cap_mm: if std::env::var("TOPO_NOCAP").is_ok() { 0.0 } else { reg.price(&Event::Overflow { wire: 0, edge: 0 }, &WireCtx::default()) },
        cap_vias: std::env::var("TOPO_CAPVIA").is_ok(),
        ..LayerParams::default()
    };
    let sub = super::capacity::subset_check(&st);
    let mut packer = Packer::for_model(model, opts, proj);
    packer.via_squeeze = reg.price(&Event::ViaSqueeze { wire: 0, at: Vec2::default(), slack_nm: 0.0, via_nm: 1.0 }, &WireCtx::default());
    let packer = packer;
    let assign = |st: &State| layers::assign_with(st, &|ob| b.smd_layer(ob), &lp, Some(&packer));
    let polys_of = |st: &State| -> Vec<Vec<Vec2>> { (0..st.wires.len()).map(|w| if st.wires[w].alive { st.polyline(w) } else { Vec::new() }).collect() };
    // Crossing directions for the capacity model's oblique factor: the taut paths (pack.rs).
    let dirs_of = |st: &State| -> Vec<Vec<Vec2>> { packer.taut_dirs(st) };
    let mut lr = assign(&st);
    tlog("P2 + P3", t0);
    let t_p4 = Instant::now();
    // ---- P4: negotiated rip-up and reroute ----
    let mut history: Vec<f64> = (0..proj.edges.len()).map(|e| hist0.get(e).copied().unwrap_or(0.0)).collect();
    let bottom = (if b.n_signal == 0 { proj.n_layers } else { b.n_signal }) as i32 - 1;
    let rank: Vec<usize> = {
        let mut r = vec![0usize; conns.len()];
        for (pos, &ci) in order.iter().enumerate() {
            r[ci] = pos;
        }
        r
    };
    // The P4 pick follows the board objective, violations first: the fewest over-full
    // edge-layers under the drawing's packer (the predicted violations), then the objective's own
    // value (`length + mm_per_via x vias`, or the occupied area in mm of default trace) instead
    // of the bare via count. (Measured: pricing the over-full edge-layers into one sum with the
    // `overflow` weight routed fewer connections on bornhack2026, so violations stay lexicographic.)
    let obj = Objective::parse(&opts.objective, opts.mm_per_via);
    let overflow_mm = reg.price(&Event::Overflow { wire: 0, edge: 0 }, &WireCtx::default()).max(1e-6);
    let via_area_mm = {
        let r = 0.5 * model.rules.default.via_d as f64 + model.rules.default.clearance as f64;
        std::f64::consts::PI * r * r * b.signal_layers.len().max(1) as f64 / ws0 / MM
    };
    let score = |st: &State, lr: &layers::LayerReport| -> (f64, usize, usize) {
        let polys = polys_of(st);
        let dirs = dirs_of(st);
        let cm = CapModel::new(st, &packer, &dirs, &lr.events, bottom, overflow_mm);
        let (mut len_mm, mut area_mm) = (0.0, 0.0);
        for (w, pl) in polys.iter().enumerate().filter(|(w, _)| st.wires[*w].alive) {
            let l = pl.windows(2).map(|x| (x[1] - x[0]).len()).sum::<f64>() / MM;
            len_mm += l;
            area_mm += l * (st.wires[w].w + st.wires[w].s) / ws0;
        }
        let own = match obj {
            Objective::LengthVias { mm_per_via } => len_mm + mm_per_via * lr.vias as f64,
            Objective::Area => area_mm + via_area_mm * lr.vias as f64,
        };
        (own, cm.overfull(), lr.vias)
    };
    // Lexicographic: over-full edge-layers, then the objective.
    let p4_better = |a: &(f64, usize, usize), b: &(f64, usize, usize)| a.1 < b.1 || (a.1 == b.1 && a.0 < b.0 - 1e-6);
    // Reroute counter per connection: the P4 moves pay the `reroute` factor's price for it.
    // (Measured: ripping the least moved wires first in the negotiated rip-up routed fewer
    // connections on dcsgonebadge and qmtech, so the rip order stays routed-last first.)
    let mut reroutes: Vec<u32> = vec![0; conns.len()];
    let first_score = score(&st, &lr);
    let mut best_p4 = (st.clone(), wire_conn.clone(), lr.clone(), first_score);
    let (mut p4_rounds, mut p4_stall, mut p4_ripped) = (0usize, 0usize, 0usize);
    // Wires taken out and routed again in this whole route (the `max_ripped` work cap).
    let mut ripped_total = 0usize;
    for _round in 0..opts.reroute_rounds {
        if expired() {
            timed_out = true;
            break;
        }
        if ripped_total >= opts.max_ripped {
            break;
        }
        let dirs = dirs_of(&st);
        let cm = CapModel::new(&st, &packer, &dirs, &lr.events, bottom, 1.0);
        let bad = cm.overfull_list();
        let nosite = cm.nosite_wires();
        if bad.is_empty() && nosite.is_empty() {
            break;
        }
        let mut bad_edges: Vec<usize> = bad.iter().map(|x| x.0).collect();
        bad_edges.dedup();
        for &e in &bad_edges {
            history[e] += 1.0;
        }
        // Per over-full edge-layer, rip the wires routed last until the rest fits.
        // A via without a site in its interval: its wire is rerouted (history on its edges).
        let mut victims: BTreeSet<WireId> = nosite.iter().copied().collect();
        for &w in &nosite {
            for &p in &st.wires[w].passes {
                history[st.passes[p].edge] += 1.0;
            }
        }
        for &(e, l) in &bad {
            let mut on: Vec<usize> = st.edge_order[e].iter().copied().filter(|&p| cm.pass_layer[p] == l as i32).collect();
            on.sort_by_key(|&p| std::cmp::Reverse(rank[wire_conn[st.passes[p].wire]]));
            let mut gone: BTreeSet<WireId> = victims.clone();
            for p in on {
                if cm.fits_without(e, l, &gone) {
                    break;
                }
                let w = st.passes[p].wire;
                gone.insert(w);
                victims.insert(w);
            }
        }
        let alive = st.wires.iter().filter(|w| w.alive).count();
        let max_rip = (alive / 4).max(10);
        let mut vic: Vec<WireId> = victims.into_iter().collect();
        vic.sort_by_key(|&w| std::cmp::Reverse(rank[wire_conn[w]]));
        vic.truncate(max_rip.min(opts.max_ripped - ripped_total));
        vic.sort_by_key(|&w| rank[wire_conn[w]]);
        if vic.is_empty() {
            break;
        }
        p4_ripped += vic.len();
        ripped_total += vic.len();
        let vic_conns: Vec<usize> = vic.iter().map(|&w| wire_conn[w]).collect();
        for &w in &vic {
            st.remove(w);
        }
        for &ci in &vic_conns {
            reroutes[ci] += 1;
        }
        st.history = history.clone();
        // Fluctuating coefficients: this round's searches price with perturbed weights.
        let reg_r = reg.fluctuated(seed_of(opts.seed, round, 1 + p4_rounds), opts.fluctuation);
        let mut lost = false;
        for &ci in &vic_conns {
            if expired() || !(route_one(&mut st, ci, cap, &mut wire_conn, &reg_r) || route_one(&mut st, ci, cap * 4, &mut wire_conn, &reg_r)) {
                timed_out |= expired();
                lost = true;
                break;
            }
        }
        if lost {
            // A wire found no path any more: this round is worth nothing, go back to the best.
            break;
        }
        st.uncross_all(50);
        lr = assign(&st);
        p4_rounds += 1;
        let sc = score(&st, &lr);
        if p4_better(&sc, &best_p4.3) {
            best_p4 = (st.clone(), wire_conn.clone(), lr.clone(), sc);
            p4_stall = 0;
        } else {
            p4_stall += 1;
            if p4_stall >= 2 {
                break;
            }
        }
    }
    let (st_b, wc_b, lr_b, last_score) = best_p4;
    st = st_b;
    wire_conn = wc_b;
    lr = lr_b;
    if p4_rounds > 0 {
        warnings.push(format!(
            "P4 negotiated rip-up and reroute: {p4_rounds} rounds, {p4_ripped} wires ripped; objective {:.0} -> {:.0}, over-full edge-layers {} -> {}, vias {} -> {}",
            first_score.0, last_score.0, first_score.1, last_score.1, first_score.2, last_score.2
        ));
    }
    tlog("P4 rip-up and reroute", t_p4);
    // ---- P4 moves: descent on the topology, then on the layer labels ----
    let mut tally = Tally::default();
    let mut term_over: BTreeMap<usize, (TermRef, TermRef)> = BTreeMap::new();
    if opts.moves && opts.max_moves > 0 && !expired() {
        let t_mv = Instant::now();
        let s0 = score(&st, &lr);
        let mut topo = Topo {
            reg: reg.clone(),
            conns: conns.clone(),
            tiers: (0..conns.len()).map(|ci| tier_of(conn_of[ci])).collect(),
            wire_conn: wire_conn.clone(),
            reroutes: reroutes.clone(),
            ws0,
            cap,
            evals: 0,
            max_evals: opts.max_moves,
            ripped: ripped_total,
            max_ripped: opts.max_ripped,
            tight: {
                let dirs = dirs_of(&st);
                let cm = CapModel::new(&st, &packer, &dirs, &lr.events, bottom, overflow_mm);
                let mut t = vec![false; proj.edges.len()];
                for (e, _) in cm.overfull_list() {
                    t[e] = true;
                }
                t
            },
            tally: Tally::default(),
        };
        let mut st2 = st.clone();
        let tt = descend_topo(&mut st2, &mut topo, &reg, seed_of(opts.seed, round, 1000), opts.fluctuation);
        let lr2 = assign(&st2);
        let s1 = score(&st2, &lr2);
        let kept = p4_better(&s1, &s0);
        warnings.push(format!(
            "P4 moves on the topology: {} evaluations; {}; objective {:.0} -> {:.0} ({}); over-full edge-layers {} -> {}, vias {} -> {}",
            topo.evals,
            tt.text(),
            s0.0,
            s1.0,
            if kept { "kept" } else { "not better: the state before the moves stays" },
            s0.1,
            s1.1,
            s0.2,
            s1.2
        ));
        if kept {
            st = st2;
            lr = lr2;
            wire_conn = topo.wire_conn.clone();
            ripped_total = topo.ripped;
            tally.add(&tt);
            // Connections now attached to another pad pair of their tree: report the new pads.
            let tref = |x: ObsId| -> Option<TermRef> {
                let p = *b.pad_of.get(x)?;
                dec.connections.iter().flat_map(|c| [&c.from, &c.to]).find(|t| matches!(t.kind, TermKind::Pad(q) if q == p)).cloned()
            };
            for (ci, (c0, c1)) in conns.iter().zip(&topo.conns).enumerate() {
                if (c0.pa, c0.pb) != (c1.pa, c1.pb) {
                    if let (Some(f), Some(t)) = (tref(c1.pa), tref(c1.pb)) {
                        out[conn_of[ci]].ghost_nm = (proj.pos[c1.b] - proj.pos[c1.a]).len();
                        term_over.insert(conn_of[ci], (f, t));
                    }
                }
            }
        }
        // Label moves against the drawing's packer: flips, via shifts, segment lifting.
        let dirs = dirs_of(&st);
        let n_sig = if b.n_signal == 0 { proj.n_layers } else { b.n_signal };
        let cm = CapModel::new(&st, &packer, &dirs, &lr.events, bottom, overflow_mm);
        let (v0, o0) = (lr.vias, cm.overfull());
        let (mask, partner) = (lr.mask.clone(), lr.partner.clone());
        let mut lb = Labels { st: &st, cm, evl: lr.events.clone(), mask: &mask, partner: &partner, bottom, via_mm: lp.via_mm, n_signal: n_sig, evals: 0, max_evals: opts.max_moves, tally: Tally::default() };
        let lt = descend_labels(&mut lb);
        let o1 = lb.cm.overfull();
        let evl = std::mem::take(&mut lb.evl);
        drop(lb);
        layers::relabel(&st, &evl, &lp, &mut lr);
        tally.add(&lt);
        warnings.push(format!("P4 moves on the layers: {}; vias {} -> {}, over-full edge-layers {} -> {}", lt.text(), v0, lr.vias, o0, o1));
        tlog("P4 moves", t_mv);
    }
    let lr = lr;
    let t_draw = Instant::now();
    // A different-net crossing still on one layer is an error: the lower-tier wire stays unrouted.
    let wlen: Vec<f64> = (0..st.wires.len()).map(|w| if !st.wires[w].alive { 0.0 } else { st.polyline(w).windows(2).map(|x| (x[1] - x[0]).len()).sum::<f64>() }).collect();
    let wire_len = |w: WireId| wlen[w];
    let mut dropped: BTreeSet<WireId> = BTreeSet::new();
    let mut unlayerable_errors = 0usize;
    for &(wa, wb) in &lr.unlayerable_pairs {
        if dropped.contains(&wa) || dropped.contains(&wb) {
            continue;
        }
        unlayerable_errors += 1;
        let (ca, cb) = (conn_of[wire_conn[wa]], conn_of[wire_conn[wb]]);
        let key = |w: WireId, c: usize| (tier_of(c), std::cmp::Reverse((wire_len(w) * 1e3) as i64), w);
        let victim = if key(wa, ca) >= key(wb, cb) { wa } else { wb };
        let other = if victim == wa { wb } else { wa };
        let cv = conn_of[wire_conn[victim]];
        let name = |w: WireId| model.nets.iter().find(|n| n.code == st.wires[w].net).map_or(String::new(), |n| n.name.clone());
        let at = lr
            .unlayerable
            .first()
            .map_or(String::new(), |p| format!(" near ({:.3}, {:.3}) mm", p.x / MM, -p.y / MM));
        let msg = format!(
            "crossing of {} and {}{at} cannot be layered: both stretches are fixed to the same layer (error, not emitted)",
            name(victim),
            name(other)
        );
        warnings.push(msg.clone());
        fail(&mut out, cv, msg);
        dropped.insert(victim);
    }
    for &w in &dropped {
        st.remove(w);
    }
    ctl.update("route", 2, 0, nconn, lr.vias, t0.elapsed().as_secs_f64());
    let _ = &t_draw;
    // ---- Drawing ----
    // An ignored copper-to-edge check is not hard: the checker keeps only "no overlap" there and
    // the violation is priced and reported afterwards (ignored_rule factor).
    let edge_ignored = model.rules.ignored_checks.iter().any(|c| c == "copper_edge_clearance");
    let relaxed;
    let check_model: &BoardModel = if edge_ignored {
        let mut m = model.clone();
        m.rules.edge_clearance = 0;
        relaxed = m;
        &relaxed
    } else {
        model
    };
    let checker = Checker::new(check_model, opts);
    let mut dr = Drawer::new(model, opts, &b, &st, &checker);
    // The plan's capacity model: where P3/P4 put each via in its interval (the drawing tries
    // that triangle first) and, after the drawing, which edge-layers it planned over-full.
    let dirs_plan = dirs_of(&st);
    let cm_plan = CapModel::new(&st, &packer, &dirs_plan, &lr.events, bottom, 1.0);
    if std::env::var("TOPO_VIADBG").is_ok() {
        eprintln!("VIAPLAN round {round}: over-full {} (no site {}), via squeeze {:.1} mm", cm_plan.overfull(), cm_plan.nosite(), cm_plan.squeeze());
        for w in 0..st.wires.len() {
            if st.wires[w].alive && !cm_plan.via_k[w].is_empty() {
                eprintln!("VIAPLAN wire {w} net {} intervals {:?} at {:?}", st.wires[w].net, cm_plan.via_iv[w], cm_plan.via_k[w]);
                for &(ka, kb) in &cm_plan.via_iv[w] {
                    let r: Vec<String> = (ka..=kb.min(st.wires[w].passes.len())).map(|k| format!("{k}:{:?}", cm_plan.rank_of(w, k))).collect();
                    eprintln!("VIAPLAN   ranks {}", r.join(" "));
                }
            }
        }
    }
    let mut plans: BTreeMap<WireId, Plan> = BTreeMap::new();
    for w in 0..st.wires.len() {
        if st.wires[w].alive {
            let mut p = plan(&lr.events[w], b.n_signal);
            p.at = cm_plan.via_k[w].clone();
            plans.insert(w, p);
        }
    }
    // Draw order: widest first, then shortest.
    let mut draw_order: Vec<WireId> = plans.keys().copied().collect();
    draw_order.sort_by(|&x, &y| {
        st.wires[y].w.total_cmp(&st.wires[x].w).then(wire_len(x).total_cmp(&wire_len(y))).then(x.cmp(&y))
    });
    // Plane stubs and vias go right after the wide wires: before the first default-width wire.
    let wide = draw_order.iter().take_while(|&&w| st.wires[w].w > model.rules.default.width as f64).count();
    // Pass layers as far as the plans fix them (a via interval leaves its passes open): the via
    // side test of `place_vias` reads them; a wire's become exact once its vias are placed.
    let set_layers = |pl: &Plan, vs: Option<&[(usize, Vec2, crate::vias::ViaSize)]>, passes: &[usize], out: &mut Vec<i32>| {
        for (k, &pid) in passes.iter().enumerate() {
            let mut l = pl.layers[0] as i32;
            for (i, &(ka, kb)) in pl.vias.iter().enumerate() {
                let kv = vs.map(|v| v[i].0);
                let after = match kv {
                    Some(kv) => k >= kv,
                    None if k >= kb => true,
                    None if k < ka => false,
                    None => {
                        l = -1;
                        break;
                    }
                };
                if after {
                    l = pl.layers[i + 1] as i32;
                }
            }
            out[pid] = l;
        }
    };
    dr.prov_layer = vec![-1; st.passes.len()];
    for (&w, pl) in &plans {
        set_layers(pl, None, &st.wires[w].passes, &mut dr.prov_layer);
    }
    // Pack with the planned vias (their discs take room, as in P3/P4), so the vias can be put
    // where the packing left them room: on their wire's nominal chord.
    for (&w, pl) in &plans {
        let d = packer.via_d(st.wires[w].net);
        dr.set_wire_layers(w, pl, &pl.at, &vec![d; pl.at.len()]);
    }
    dr.compute_intervals();
    // Phase 1: the vias of every wire (they are obstacles for the wires drawn next).
    let mut placed: BTreeMap<WireId, Vec<(usize, Vec2, crate::vias::ViaSize)>> = BTreeMap::new();
    let mut via_ids: BTreeMap<WireId, Vec<usize>> = BTreeMap::new();
    let mut live: Vec<WireId> = Vec::new();
    for &w in &draw_order {
        if expired() {
            timed_out = true;
            fail(&mut out, conn_of[wire_conn[w]], "time limit reached before this connection was drawn");
            continue;
        }
        let mut ids = Vec::new();
        match dr.place_vias(w, &plans[&w], &mut ids) {
            Ok(v) => {
                set_layers(&plans[&w], Some(&v), &st.wires[w].passes, &mut dr.prov_layer);
                placed.insert(w, v);
                via_ids.insert(w, ids);
                live.push(w);
            }
            Err(r) => {
                for &pid in &st.wires[w].passes {
                    dr.prov_layer[pid] = -1;
                }
                for id in ids {
                    dr.uncommit(id);
                }
                fail(&mut out, conn_of[wire_conn[w]], r);
            }
        }
    }
    // Pass layers, via discs and intervals from the placed vias.
    dr.pass_layer = vec![-1; st.passes.len()];
    dr.pass_disc = vec![[0.0; 2]; st.passes.len()];
    for &w in &live {
        let (pl, vs) = (&plans[&w], &placed[&w]);
        let ks: Vec<usize> = vs.iter().map(|v| v.0).collect();
        let ds: Vec<f64> = vs.iter().map(|v| v.2.d as f64).collect();
        dr.set_wire_layers(w, pl, &ks, &ds);
    }
    dr.compute_intervals();
    let over_n = dr.over.iter().filter(|&&o| o).count();
    // Planning-time check (D0 item 4): an edge-layer the plan packs without overflow (P3 labels,
    // capacity model) must fit in the drawing's packing too (same packer, pass layers from the
    // placed vias). `.0` counts the violations "planned fits, packer over-full"; `.1` the reverse.
    let plan_vs_draw = {
        let planned: BTreeSet<(usize, usize)> = cm_plan.overfull_list().into_iter().collect();
        let nl = proj.n_layers;
        let (mut fits_over, mut over_fits) = (0usize, 0usize);
        for (i, &o) in dr.over.iter().enumerate() {
            let p = planned.contains(&(i / nl, i % nl));
            if o && !p {
                fits_over += 1;
            } else if p && !o {
                over_fits += 1;
            }
        }
        (fits_over, over_fits)
    };
    // Phase 2: plane stubs, then the wires. Plane pads go before the first default-width wire.
    let mut plane_res: Vec<(usize, Result<planes::Out, String>)> = Vec::new();
    let mut plane_done = false;
    let run_planes = |dr: &mut Drawer, out: &mut Vec<Outcome>, plane_res: &mut Vec<(usize, Result<planes::Out, String>)>| {
        for rq in &plane_reqs {
            let r = planes::place(dr, rq);
            if let Ok(o) = &r {
                out[rq.conn].routed = true;
                out[rq.conn].items = o.items.clone();
                out[rq.conn].vias = vec![(o.via, o.size)];
                out[rq.conn].layers = vec![o.layer];
                out[rq.conn].length_nm = o.stub_nm;
                out[rq.conn].stub_nm = o.stub_nm;
            } else if let Err(e) = &r {
                fail(out, rq.conn, e.clone());
            }
            plane_res.push((rq.conn, r));
        }
    };
    let mut drawn_ok = 0usize;
    let mut necking: Vec<(usize, Event)> = Vec::new();
    let mut drawn: BTreeMap<WireId, WireOut> = BTreeMap::new();
    let mut via_h: BTreeMap<WireId, Vec<usize>> = BTreeMap::new();
    let mut failed_draw: Vec<WireId> = Vec::new();
    for (pos, &w) in live.iter().enumerate() {
        if pos == wide.min(live.len()) && !plane_done {
            run_planes(&mut dr, &mut out, &mut plane_res);
            plane_done = true;
        }
        if expired() {
            timed_out = true;
            fail(&mut out, conn_of[wire_conn[w]], "time limit reached before this connection was drawn");
            continue;
        }
        let ci = conn_of[wire_conn[w]];
        let mut ids = via_ids.remove(&w).unwrap_or_default();
        let vh = ids[..placed[&w].len().min(ids.len())].to_vec();
        let wo = dr.draw_wire(w, &plans[&w], &placed[&w], &mut ids);
        if wo.ok {
            via_h.insert(w, vh);
            drawn_ok += 1;
            settle(&mut out, &mut necking, ci, w, &wo, &b, &st);
            drawn.insert(w, wo);
        } else {
            for id in ids {
                dr.uncommit(id);
            }
            fail(&mut out, ci, wo.reason);
            failed_draw.push(w);
        }
        if drawn_ok.is_multiple_of(50) {
            ctl.update("route", 3, drawn_ok, nconn, 0, t0.elapsed().as_secs_f64());
        }
    }
    if !plane_done {
        run_planes(&mut dr, &mut out, &mut plane_res);
    }
    // Debug (D0): TOPO_D0AUDIT=<file> writes the neighbour audit of the drawing (packer
    // positions against the drawn crossings and copper gaps, `dwitness::audit`) as JSON.
    if let Ok(path) = std::env::var("TOPO_D0AUDIT") {
        let a = super::dwitness::audit(&dr, &drawn);
        if let Ok(j) = serde_json::to_string(&a) {
            let _ = std::fs::write(format!("{path}.r{round}"), j);
        }
    }
    // ---- Feedback (plan S2c item 4): a wire the drawing could not fit is drawn before the
    // wires that block its sleeve, which are then redrawn around it; at most one round per wire.
    let (mut fb_tried, mut fb_rescued) = (0usize, 0usize);
    if opts.drawing_feedback {
        let rejected: Vec<Diag> = dr.diag.clone();
        for &f in &failed_draw {
            if expired() {
                timed_out = true;
                break;
            }
            let Some(d) = rejected.iter().rev().find(|d| d.wire == f) else { continue };
            let cf = conn_of[wire_conn[f]];
            if !out[cf].reason.starts_with("drawing:") {
                continue;
            }
            let blockers = dr.blockers(d, drawn.iter().map(|(&w, wo)| (w, wo)));
            if blockers.len() > opts.feedback_max_rip || ripped_total + blockers.len() > opts.max_ripped {
                continue;
            }
            ripped_total += blockers.len();
            fb_tried += 1;
            // Rip the blockers' tracks, draw the rejected wire, then redraw the blockers.
            for bw in &blockers {
                for &h in &drawn[bw].handles {
                    dr.uncommit(h);
                }
            }
            // The vias are placed again: copper drawn since the first placement may be in the way.
            let mut ids: Vec<usize> = Vec::new();
            let vias_f = match dr.place_vias(f, &plans[&f], &mut ids) {
                Ok(v) => v,
                Err(_) => {
                    for h in ids {
                        dr.uncommit(h);
                    }
                    // Put the ripped blockers' copper back.
                    for bw in &blockers {
                        let mut hs = Vec::new();
                        for it in drawn[bw].items.iter().filter(|i| matches!(i, Item::Track(_))) {
                            hs.push(dr.commit(it));
                        }
                        if let Some(x) = drawn.get_mut(bw) {
                            x.handles = hs;
                        }
                    }
                    continue;
                }
            };
            let wf = dr.draw_wire(f, &plans[&f], &vias_f, &mut ids);
            let mut redrawn: Vec<(WireId, WireOut)> = Vec::new();
            let mut ok = wf.ok;
            if ok {
                for &bw in &blockers {
                    let mut bids = Vec::new();
                    let wb = dr.draw_wire(bw, &plans[&bw], &placed[&bw], &mut bids);
                    if !wb.ok {
                        ok = false;
                        break;
                    }
                    redrawn.push((bw, wb));
                }
            }
            if ok {
                fb_rescued += 1;
                drawn_ok += 1;
                for (bw, wb) in redrawn {
                    let cb = conn_of[wire_conn[bw]];
                    necking.retain(|(c, _)| *c != cb);
                    settle(&mut out, &mut necking, cb, bw, &wb, &b, &st);
                    drawn.insert(bw, wb);
                }
                out[cf].reason.clear();
                via_h.insert(f, ids[..vias_f.len().min(ids.len())].to_vec());
                placed.insert(f, vias_f);
                settle(&mut out, &mut necking, cf, f, &wf, &b, &st);
                drawn.insert(f, wf);
            } else {
                // Undo: whatever was drawn in this round goes, the old copper comes back.
                for &h in &ids {
                    dr.uncommit(h);
                }
                if wf.ok {
                    for &h in &wf.handles {
                        dr.uncommit(h);
                    }
                }
                for (_, wb) in &redrawn {
                    for &h in &wb.handles {
                        dr.uncommit(h);
                    }
                }
                for bw in &blockers {
                    let mut hs = Vec::new();
                    for it in drawn[bw].items.iter().filter(|i| matches!(i, Item::Track(_))) {
                        hs.push(dr.commit(it));
                    }
                    if let Some(x) = drawn.get_mut(bw) {
                        x.handles = hs;
                    }
                }
            }
        }
    }
    // ---- Via reduction (S4, plan P6): measure the vias by cause, then drop the redundant ones ----
    let end_pads = |w: WireId| [st.wires[w].pa, st.wires[w].pb];
    let end_fixed = |w: WireId| {
        let [pa, pb] = end_pads(w);
        [b.smd_layer(pa).is_some(), b.smd_layer(pb).is_some()]
    };
    let frozen = |w: WireId| model.pours.iter().any(|z| z.net == st.wires[w].net);
    let causes = |plans: &BTreeMap<WireId, Plan>, drawn: &BTreeMap<WireId, WireOut>| -> String {
        let (mut total, mut pad_forced, mut crossing, mut close, mut one_via, mut pairs_aba) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
        for (w, wo) in drawn {
            let v = &wo.vias;
            if v.is_empty() {
                continue;
            }
            let [pa, pb] = end_pads(*w);
            let forced = match (b.smd_layer(pa), b.smd_layer(pb)) {
                (Some(x), Some(y)) if x != y => 1,
                _ => 0,
            };
            total += v.len();
            pad_forced += forced.min(v.len());
            crossing += v.len() - forced.min(v.len());
            if v.len() == 1 {
                one_via += 1;
            }
            if let Some(pl) = plans.get(w) {
                if pl.layers.len() >= 3 && pl.layers.windows(3).any(|x| x[0] == x[2]) {
                    pairs_aba += 1;
                }
            }
            close += v.windows(2).filter(|x| (x[1].0 - x[0].0).len() < 3.0 * MM).count();
        }
        format!(
            "{total} wire vias: {pad_forced} forced by SMD pads on different layers, {crossing} by crossings or capacity (the capacity refinement relabels, it adds no via), {one_via} wires with one via, {pairs_aba} wires with a layer pair A-B-A, {close} via pairs of one wire closer than 3 mm; {} plane stub vias",
            plane_res.iter().filter(|(_, r)| r.is_ok()).count()
        )
    };
    let causes_before = causes(&plans, &drawn);
    tlog("drawing + feedback", t_draw);
    let t_red = Instant::now();
    // The result as drawn is a variant of its own (more vias, maybe shorter).
    let mut pre_snap: Option<Snapshot> = None;
    // TOPO_NOREDUCE=1 (debug): leave the vias as drawn, to compare with the reduction.
    if opts.via_reduce && std::env::var("TOPO_NOREDUCE").is_err() {
        pre_snap = Some((out.clone(), necking.clone()));
        let (rs, changed) = super::reduce::reduce_vias(&mut dr, &mut drawn, &mut placed, &mut plans, &mut via_h, &end_fixed, &frozen, b.n_signal, deadline);
        timed_out |= rs.timed_out;
        for w in changed {
            let ci = conn_of[wire_conn[w]];
            necking.retain(|(c, _)| *c != ci);
            settle(&mut out, &mut necking, ci, w, &drawn[&w], &b, &st);
        }
        warnings.push(format!(
            "via reduction after drawing: {} wires with vias, {} relabels tried in {} sweeps, {} accepted, {} vias removed ({:.1} s{}); before: {}; after: {}",
            rs.wires_with_vias,
            rs.tried,
            rs.sweeps,
            rs.accepted,
            rs.vias_removed,
            t_red.elapsed().as_secs_f64(),
            if rs.timed_out { ", stopped by the time budget" } else { "" },
            causes_before,
            causes(&plans, &drawn)
        ));
    } else {
        warnings.push(format!("via reduction after drawing: off; {causes_before}"));
    }
    // Vias in a narrow pass (the `via_squeeze` factor's report): a drawn wire via whose
    // passages keep less slack than half its diameter in the drawing's packing.
    {
        let (mut n_vias, mut narrow) = (0usize, 0usize);
        for &w in drawn.keys() {
            let Some(vs) = placed.get(&w) else { continue };
            let ci = conn_of[wire_conn[w]];
            let ctx = WireCtx { net: dec.connections[ci].net, tier: dec.connections[ci].tier };
            for &(k, p, sz) in vs {
                n_vias += 1;
                let slack = dr.via_slack(w, k);
                if slack < 0.5 * sz.d as f64 {
                    narrow += 1;
                    let ev = Event::ViaSqueeze { wire: w, at: p, slack_nm: slack, via_nm: sz.d as f64 };
                    for f in reg.findings(std::slice::from_ref(&ev), &ctx) {
                        if f.level >= Report::Warning {
                            warnings.push(format!("{} ({}): {}", model.nets.iter().find(|n| n.code == ctx.net).map_or("", |n| n.name.as_str()), dec.connections[ci].from.label, f.text));
                        }
                    }
                }
            }
        }
        warnings.push(format!("via_squeeze: {narrow} of {n_vias} wire vias in a narrow pass (less slack than half the via diameter next to them)"));
    }
    // History for the next whole route: every edge that was over-full under the drawing's packer.
    let mut hist_out = history.clone();
    {
        let nl = proj.n_layers;
        let mut over_edges: Vec<usize> = dr.over.iter().enumerate().filter(|(_, &o)| o).map(|(i, _)| i / nl).collect();
        over_edges.dedup();
        for e in over_edges {
            hist_out[e] += 1.0;
        }
    }
    tlog("via reduction", t_red);
    if let Ok(path) = std::env::var("TOPO_DUMP") {
        // One file per whole-route round, like `TOPO_DIAG`.
        let _ = std::fs::write(format!("{path}.r{round}"), dr.dump());
    }
    let diag = std::mem::take(&mut dr.diag);
    // Hints for a second round: per cut of a still-failed wire, the room is lowered by the
    // violation depth (what was really missing) at the cut nearest to the violation.
    let mut hint_map: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    let mut sleeve: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut own_out = mem.own.clone();
    for d in &diag {
        let ci = conn_of[wire_conn[d.wire]];
        if out[ci].routed || d.passes == 0 {
            continue;
        }
        let wi = &st.wires[d.wire];
        let short = ((d.req_nm - d.gap_nm).max(0) as f64).max(20_000.0);
        let best = (d.k0..d.k1)
            .map(|k| st.passes[wi.passes[k]].edge)
            .filter(|&e| proj.room[e][d.layer].is_finite())
            .map(|e| {
                let [p, q] = proj.edges[e];
                let (a, c) = (proj.pos[p], proj.pos[q]);
                let ab = c - a;
                let t = ((d.at - a).dot(ab) / ab.len2().max(1.0)).clamp(0.0, 1.0);
                (((a + ab * t) - d.at).len(), e)
            })
            .min_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        // The wire's whole sleeve on that layer is remembered: the repeat routes it elsewhere.
        for e in (d.k0..d.k1).map(|k| st.passes[wi.passes[k]].edge) {
            if sleeve.insert((ci, e)) {
                *own_out.entry(ci).or_default().entry(e).or_insert(0.0) += 1.0;
            }
        }
        if let Some((_, e)) = best {
            let v = hint_map.entry((e, d.layer)).or_insert(0.0);
            *v = v.max(short);
        }
    }
    let new_hints: Hints = hint_map.into_iter().map(|((e, l), sh)| (e, l, (proj.room[e][l] - sh).max(0.0))).collect();
    {
        let cnt = |c: &str| diag.iter().filter(|d| d.class == c).count();
        warnings.push(format!(
            "layers: over-full edge-layers (packer model) {} before the capacity refinement, {} after ({} moves); drawing packer over-full {}; plan vs drawing packer: {} edge-layers planned to fit but over-full when drawn, {} planned over-full but fitting when drawn; drawing yield: {} wires entered drawing, {} drawn ({} lost at the via stage); rejected runs {}: packing {}, repair {}, chord {}, of which infeasible against the static board alone {}; runs accepted by the exact path {}, by the S2a funnel+repair fallback {}, exact path without result {} ({:.1} s in the exact search); feedback tried {}, rescued {}",
            lr.overfull_before,
            lr.overfull_after,
            lr.refine_moves,
            over_n,
            plan_vs_draw.0,
            plan_vs_draw.1,
            plans.len(),
            drawn_ok,
            plans.len() - live.len(),
            diag.len(),
            cnt("packing"),
            cnt("repair"),
            cnt("chord"),
            diag.iter().filter(|d| d.static_infeasible).count(),
            dr.stats.exact_ok,
            dr.stats.fallback_ok,
            dr.stats.exact_none,
            dr.stats.exact_s,
            fb_tried,
            fb_rescued
        ));
    }
    if let Ok(path) = std::env::var("TOPO_DIAG") {
        let rows: Vec<String> = diag
            .iter()
            .map(|d| {
                let nm = |x: f64| (x / 1e3).round() / 1e3;
                format!(
                    "{{\"wire\":{},\"net\":{},\"layer\":{},\"w\":{},\"passes\":{},\"over_passes\":{},\"class\":\"{}\",\"against\":{:?},\"gap\":{},\"req\":{},\"at\":[{},{}],\"from\":[{},{}],\"to\":[{},{}],\"pre_bad\":{:?},\"bumps\":{},\"static_infeasible\":{},\"against_via\":{}}}",
                    d.wire, d.net, d.layer, nm(d.width_nm), d.passes, d.over_passes, d.class, d.against, d.gap_nm, d.req_nm,
                    nm(d.at.x), nm(-d.at.y), nm(d.from.x), nm(-d.from.y), nm(d.to.x), nm(-d.to.y), d.pre_bad, d.bumps, d.static_infeasible, d.against_via
                )
            })
            .collect();
        // One file per whole-route round (`<path>.r<round>`): the feedback rounds repeat the route.
        let _ = std::fs::write(format!("{path}.r{round}"), format!("[{}]", rows.join(",\n")));
    }
    drop(dr);
    let isl = planes::islands(model, &b, &out_vias(&out, &plane_reqs, &dec.connections), &plane_reqs);
    warnings.extend(isl);
    // ---- Assemble the result ----
    let first_layer = signal[0];
    let name_of = |net: i32| model.nets.iter().find(|n| n.code == net).map_or(String::new(), |n| n.name.clone());
    let asm = assemble(model, &dec, &out, &term_over, first_layer);
    // Warning-level factor findings (necking) of a set of necked stretches.
    let warn_n = |nk: &[(usize, Event)]| -> usize {
        nk.iter()
            .map(|(ci, ev)| {
                let ctx = WireCtx { net: dec.connections[*ci].net, tier: dec.connections[*ci].tier };
                reg.findings(std::slice::from_ref(ev), &ctx).iter().filter(|f| f.level >= Report::Warning).count()
            })
            .sum()
    };
    let n_copper = b.signal_layers.len().max(1);
    let crit_main = criteria(model, &dec, &asm, &out, warn_n(&necking), n_copper);
    let pre_parts = pre_snap.as_ref().map(|(o, nk)| {
        let a = assemble(model, &dec, o, &term_over, first_layer);
        let c = criteria(model, &dec, &a, o, warn_n(nk), n_copper);
        (c, a)
    });
    let Assembled { items, item_conn, reports, nets, length_by_layer, tot_len, tot_vias } = asm;
    if edge_ignored {
        let rule = model.rules.edge_clearance as f64;
        let outline = board::outline_segments(model);
        let ctx = WireCtx::default();
        let mut n = 0usize;
        for it in &items {
            let Item::Track(t) = it else { continue };
            let crate::geom::quant::TrackPrim::Seg { a, b } = t.prim else { continue };
            let (a, b) = (Vec2 { x: f64::from(a.x), y: -f64::from(a.y) }, Vec2 { x: f64::from(b.x), y: -f64::from(b.y) });
            let near = outline
                .iter()
                .map(|&(c, d)| super::draw::seg_seg_dist(a, b, c, d))
                .fold(f64::INFINITY, f64::min);
            let gap = near - 0.5 * t.width as f64;
            if gap < rule - 1.0 {
                let ev = Event::IgnoredRuleViolated { rule: "copper_edge_clearance".into(), at: (a + b) * 0.5, amount_nm: rule - gap };
                n += 1;
                for f in reg.findings(std::slice::from_ref(&ev), &ctx) {
                    if f.level >= Report::Warning {
                        warnings.push(f.text);
                    }
                }
            }
        }
        if n > 0 {
            warnings.push(format!("ignored_rule: {n} track segments violate the copper edge clearance (check set to ignore in KiCad); priced by the ignored_rule factor"));
        }
    }
    let routed_n = reports.iter().filter(|r| r.status == "routed").count();
    let nets_total = nets.iter().filter(|n| n.connections > 0).count();
    let nets_routed = nets.iter().filter(|n| n.connections > 0 && n.status == "routed").count();
    let diff_failed = reports.iter().filter(|r| r.reason == DIFF_PAIR).count();
    if diff_failed > 0 {
        warnings.push(format!("diff pairs skipped: {diff_failed} connections ({DIFF_PAIR})"));
    }
    if unlayerable_errors > 0 {
        warnings.push(format!("layering errors: {unlayerable_errors} different-net crossings cannot be layered (not emitted)"));
    }
    let mut p2r: Vec<f64> = (0..st.wires.len())
        .filter(|&w| st.wires[w].alive || dropped.contains(&w))
        .map(|w| wlen[w] / (proj.pos[st.wires[w].b] - proj.pos[st.wires[w].a]).len().max(1.0))
        .collect();
    if std::env::var("TOPO_P2DEBUG").is_ok() {
        let mut v: Vec<(f64, WireId)> = (0..st.wires.len())
            .filter(|&w| st.wires[w].alive || dropped.contains(&w))
            .map(|w| (wlen[w] / (proj.pos[st.wires[w].b] - proj.pos[st.wires[w].a]).len().max(1.0), w))
            .collect();
        v.sort_by(|a, b| b.0.total_cmp(&a.0));
        for &(r, w) in v.iter().take(12) {
            let wi = &st.wires[w];
            let (pa, pb) = (proj.pos[wi.a], proj.pos[wi.b]);
            // Other-net obstacles the straight segment crosses, per layer set.
            let mut single = [0usize; 2];
            let mut both = 0usize;
            for ob in proj.obstacles.iter().filter(|o| o.net != Some(wi.net)) {
                let n = ob.poly.len();
                let hit = (0..n).any(|i| {
                    let (a, b) = (ob.poly[i], ob.poly[(i + 1) % n]);
                    let (d1, d2) = (pb - pa, b - a);
                    let den = d1.cross(d2);
                    den.abs() > 1e-9 && {
                        let t = (a - pa).cross(d2) / den;
                        let u = (a - pa).cross(d1) / den;
                        (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)
                    }
                });
                if hit {
                    if ob.drilled || ob.layers.count() == 2 {
                        both += 1;
                    } else if let Some(l) = ob.layers.single() {
                        single[l] += 1;
                    }
                }
            }
            {
                let est = State::new(proj);
                let c = Conn { net: wi.net, pa: wi.pa, pb: wi.pb, a: wi.a, b: wi.b, w: wi.w, s: wi.s };
                let wt = weights(&reg, &c, 3, 2_000_000);
                let alone = est.search(&c, &wt, (c.w + c.s) / ws0);
                if let Some((path, tris)) = alone {
                    let mut e2 = State::new(proj);
                    let wire = super::state::Wire { net: c.net, w: c.w, s: c.s, a: c.a, b: c.b, pa: c.pa, pb: c.pb, turns: Vec::new(), passes: Vec::new(), tris: Vec::new(), alive: true };
                    let id = e2.insert(wire, &path, tris);
                    let l: f64 = e2.polyline(id).windows(2).map(|x| (x[1] - x[0]).len()).sum();
                    eprintln!("  alone on an empty state: {:.2} mm, {} passes", l / MM, path.len());
                }
            }
            eprintln!(
                "P2 outlier {r:.1}: {} -> {} net {} straight {:.2} mm path {:.2} mm crossings {} straight crosses obstacles on both layers {both}, only F {}, only B {}, passes {}",
                { let p = &model.pads[b.pad_of[wi.pa]]; format!("{}.{} {:?}", p.ref_, p.number, p.center) },
                { let p = &model.pads[b.pad_of[wi.pb]]; format!("{}.{} {:?}", p.ref_, p.number, p.center) },
                wi.net,
                (pb - pa).len() / MM,
                wlen[w] / MM,
                st.crossings(w).len(),
                single[0],
                single[1],
                wi.passes.len()
            );
        }
    }
    p2r.sort_by(f64::total_cmp);
    let q = |f: f64| p2r.get(((p2r.len() as f64 - 1.0) * f).round() as usize).copied().unwrap_or(1.0);
    warnings.push(format!(
        "P2 topology before drawing: path length over straight distance median {:.2}, p90 {:.2}, max {:.2}, {} wires above 3",
        q(0.5),
        q(0.9),
        q(1.0),
        p2r.iter().filter(|&&r| r > 3.0).count()
    ));
    warnings.push(format!(
        "capacity: before P3 {} of {} edges break the layer-subset (Hall) check; after P3 {} edge-layers are overfull when packed (a wire that does not fit is rejected by the checker, never emitted)",
        sub.violated.iter().map(|v| v.0).collect::<BTreeSet<_>>().len(),
        sub.edges,
        over_n
    ));
    if b.signal_layers.len() > b.n_signal {
        let (mut nm, mut cuts) = (0.0, 0usize);
        for (_, pts) in drawn.values().flat_map(|wo| wo.runs.iter()).filter(|(l, _)| *l >= b.n_signal) {
            nm += pts.windows(2).map(|x| (x[1] - x[0]).len()).sum::<f64>();
            cuts += 1;
        }
        warnings.push(format!(
            "planes ([planes] signals = {}): {:.1} mm of signal copper on plane layers, {} plane cuts (P3 planned {} wires with a plane stretch)",
            opts.plane_signals,
            nm / MM,
            cuts,
            lr.stretches.iter().filter(|s| s.iter().any(|(l, _)| *l as usize >= b.n_signal)).count()
        ));
    }
    warnings.push(format!(
        "topological core S2a: {} wires in P2 ({} double-crossing swaps), P3 vias {}, {} triangles",
        st.wires.iter().filter(|w| w.alive).count() + dropped.len(),
        swaps,
        lr.vias,
        proj.tris.len()
    ));
    let mut findings: Vec<Finding> = Vec::new();
    for (ci, ev) in &necking {
        let ctx = WireCtx { net: dec.connections[*ci].net, tier: dec.connections[*ci].tier };
        for f in reg.findings(std::slice::from_ref(ev), &ctx) {
            findings.push(Finding { text: format!("{} ({}): {}", name_of(ctx.net), dec.connections[*ci].from.label, f.text), ..f });
        }
    }
    for f in findings.iter().filter(|f| f.level >= Report::Warning) {
        warnings.push(f.text.clone());
    }
    let stop = if cancelled() || ctl.is_cancelled() {
        "cancel"
    } else if timed_out {
        "timeout"
    } else {
        "converged"
    };
    ctl.update("done", 4, routed_n, nconn, tot_vias, t0.elapsed().as_secs_f64());
    let result = RouteResult {
        items,
        connections_total: reports.len(),
        routed: routed_n,
        partial: 0,
        failed: reports.len() - routed_n,
        connections: reports,
        nets,
        nets_total,
        nets_routed,
        total_length_nm: tot_len,
        length_outside_pads_nm: tot_len,
        vias: tot_vias,
        length_by_layer_nm: length_by_layer,
        sites: 0,
        vias_in_pads: Vec::new(),
        via_reduction: "not run".into(),
        via_relaxation: "not run".into(),
        via_shove: "not run".into(),
        rip_pass: "not run".into(),
        ripup: RipReport::default(),
        iterations: 1,
        converged: !timed_out,
        overuse_edges: 0,
        clinches: Default::default(),
        relax_iterations: 0,
        self_check_rounds: 0,
        check_candidates_visited: 0,
        timed_out,
        stop_reason: stop.into(),
        runtime_s: t0.elapsed().as_secs_f64(),
        threads_used: 1,
        warnings,
        zero_slack_wires: Vec::new(),
        zero_slack: "not run".into(),
        profile: None,
        item_conn,
        stubs: Vec::new(),
        variants: Vec::new(),
        moves: super::moves::KINDS.iter().map(|&k| (k.name().to_string(), tally.applied_of(k))).collect(),
    };
    let pre = pre_parts.map(|(c, a)| (c, with_parts(&result, a)));
    (RoundOut { result, crit: crit_main, pre }, new_hints, Memory { edge: hist_out, own: own_out })
}

/// Seed of a fluctuation stream: the run's seed, the whole-route round and the stage.
fn seed_of(seed: u64, round: usize, stage: usize) -> u64 {
    seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ ((round as u64) << 32) ^ stage as u64
}

/// `r` with the connections, items, nets and totals of `a` (the variant before the via reduction).
fn with_parts(r: &RouteResult, a: Assembled) -> RouteResult {
    let routed = a.reports.iter().filter(|c| c.status == "routed").count();
    RouteResult {
        items: a.items,
        item_conn: a.item_conn,
        connections_total: a.reports.len(),
        routed,
        failed: a.reports.len() - routed,
        connections: a.reports,
        nets_routed: a.nets.iter().filter(|n| n.connections > 0 && n.status == "routed").count(),
        nets: a.nets,
        total_length_nm: a.tot_len,
        length_outside_pads_nm: a.tot_len,
        vias: a.tot_vias,
        length_by_layer_nm: a.length_by_layer,
        ..r.clone()
    }
}

/// The connection reports, items, net reports and totals of the outcomes `out`.
struct Assembled {
    items: Vec<Item>,
    item_conn: Vec<usize>,
    reports: Vec<ConnReport>,
    nets: Vec<NetRouteReport>,
    length_by_layer: BTreeMap<String, i64>,
    tot_len: i64,
    tot_vias: usize,
}

/// Assemble the result parts of `out`; `term_over` replaces the terminals of a connection the
/// attachment move connected to another pad pair. O(connections + items).
fn assemble(model: &BoardModel, dec: &crate::decompose::Decomposition, out: &[Outcome], term_over: &BTreeMap<usize, (TermRef, TermRef)>, first_layer: u32) -> Assembled {
    let name_of = |net: i32| model.nets.iter().find(|n| n.code == net).map_or(String::new(), |n| n.name.clone());
    let layer_name = |id: u32| model.layers.iter().find(|l| l.id == id).map_or(id.to_string(), |l| l.name.clone());
    let nconn = dec.connections.len();
    let mut items: Vec<Item> = Vec::new();
    let mut item_conn: Vec<usize> = Vec::new();
    let mut reports: Vec<ConnReport> = Vec::with_capacity(nconn);
    let mut length_by_layer: BTreeMap<String, i64> = BTreeMap::new();
    let (mut tot_len, mut tot_vias) = (0i64, 0usize);
    for c in &dec.connections {
        let o = &out[c.id];
        let (from, to) = term_over.get(&c.id).map_or_else(|| (term_out(&c.from), term_out(&c.to)), |(f, t)| (term_out(f), term_out(t)));
        let mut r = ConnReport::blank(
            c.id,
            c.net,
            name_of(c.net),
            first_layer,
            c.width,
            if o.routed { "routed" } else { "failed" },
            if o.routed { "" } else { &o.reason },
            from,
            to,
        );
        if o.routed {
            r.vias = o.vias.len();
            r.layers = o.layers.iter().map(|&l| layer_name(l)).collect();
            r.layer = o.layers.first().copied().unwrap_or(first_layer);
            r.via_sites = o.vias.iter().map(|v| kicad(v.0)).collect();
            r.length_nm = o.length_nm.round() as i64;
            r.length_outside_pads_nm = r.length_nm;
            r.stub_nm = o.stub_nm.round() as i64;
            r.neck_nm = o.neck_nm.round() as i64;
            r.neck_width_nm = o.neck_w.map(|x| x.round() as i64);
            // A stub into a plane has no straight-line reference: it is not a detour.
            r.ghost_len_nm = (o.stub_nm == 0.0).then_some(o.ghost_nm.round() as i64);
            r.pattern = if o.vias.is_empty() { "topo".into() } else { "topo+via".into() };
            tot_len += r.length_nm;
            tot_vias += r.vias;
            for it in &o.items {
                if let Item::Track(t) = it {
                    let len = match t.prim {
                        crate::geom::quant::TrackPrim::Seg { a, b } => ((b.x - a.x) as f64).hypot((b.y - a.y) as f64),
                        _ => 0.0,
                    };
                    *length_by_layer.entry(layer_name(t.layer)).or_insert(0) += len.round() as i64;
                }
            }
            for it in &o.items {
                items.push(it.clone());
                item_conn.push(c.id);
            }
        }
        reports.push(r);
    }
    let nets: Vec<NetRouteReport> = dec
        .nets
        .iter()
        .map(|n| {
            let count = n.connections.len();
            let routed = n.connections.iter().filter(|&&i| out[i].routed).count();
            let vias: usize = n.connections.iter().map(|&i| out[i].vias.len()).sum();
            let length: i64 = n.connections.iter().map(|&i| reports[i].length_nm).sum();
            let status = if n.excluded {
                "excluded"
            } else if n.pour_served {
                "pour_served"
            } else if !n.in_scope {
                "out_of_scope"
            } else if count == 0 {
                "complete"
            } else if routed == count {
                "routed"
            } else if routed == 0 {
                "failed"
            } else {
                "partial"
            };
            NetRouteReport {
                net: n.net,
                name: n.name.clone(),
                status: status.into(),
                connections: count,
                routed,
                partial: 0,
                failed: count - routed,
                vias,
                length_nm: length,
                length_outside_pads_nm: length,
            }
        })
        .collect();
    Assembled { items, item_conn, reports, nets, length_by_layer, tot_len, tot_vias }
}

/// The archive criteria of the assembled result `a` of outcomes `out` (`warnings` = warning-level
/// factor findings). O(items).
fn criteria(model: &BoardModel, dec: &crate::decompose::Decomposition, a: &Assembled, out: &[Outcome], warnings: usize, n_layers: usize) -> Criteria {
    let routed = a.reports.iter().filter(|r| r.status == "routed").count();
    let mut area = 0.0;
    for (it, &ci) in a.items.iter().zip(&a.item_conn) {
        let s = dec.connections[ci].clearance as f64;
        match it {
            Item::Track(t) => {
                if let crate::geom::quant::TrackPrim::Seg { a, b } = t.prim {
                    area += ((b.x - a.x) as f64).hypot((b.y - a.y) as f64) * (t.width as f64 + s);
                }
            }
            Item::Via(v) => {
                let r = 0.5 * v.d as f64 + s;
                area += std::f64::consts::PI * r * r * n_layers as f64;
            }
        }
    }
    let _ = model;
    Criteria {
        errors: a.reports.len() - routed,
        vias: a.tot_vias,
        length_mm: a.tot_len as f64 / MM,
        shrinkages: out.iter().filter(|o| o.routed).map(|o| o.neck_n).sum(),
        warnings,
        area_mm2: area / (MM * MM),
    }
}

/// `(connection id, via centre)` of every routed plane connection (for the island check).
fn out_vias(out: &[Outcome], reqs: &[planes::Req], _conns: &[Connection]) -> Vec<(usize, Vec2)> {
    reqs.iter().filter(|r| out[r.conn].routed).filter_map(|r| out[r.conn].vias.first().map(|v| (r.conn, v.0))).collect()
}

/// Unused import guard for the via size lookup used by the plane stage.
#[allow(dead_code)]
fn _sizes(model: &BoardModel, opts: &Options, net: i32) -> Vec<crate::vias::ViaSize> {
    via_sizes(&model.rules, net, opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_pair_halves_by_name() {
        let mut m = BoardModel::default();
        for (i, n) in ["USB_D+", "USB_D-", "CLK_P", "CLK_N", "VCC", "ONLY_P"].iter().enumerate() {
            m.nets.push(serde_json::from_value(serde_json::json!({"code": i as i32 + 1, "name": n})).expect("net"));
        }
        let d = diff_pair_nets(&m);
        assert_eq!(d.into_iter().collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    }
}
