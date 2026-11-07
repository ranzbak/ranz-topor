//! The `route()` entry of the topological core, stage S2a ("first copper"; S2b: any number of signal layers):
//! BoardModel -> projection (`board`) -> P2 initial routing (`init`, pad to pad, widest first)
//! -> P3 layer assignment (`layers`, Polubasov) -> minimal drawing (`draw`) -> `RouteResult`.
//!
//! Every connection ends in one of three states, none of them silent:
//! * routed: copper emitted, checked by the [`Checker`] against the board and all earlier copper;
//! * routed with violations (M-B1 never drop, `[optimize] allow_violations`, the default): a
//!   crossing that cannot be layered, a via without a legal site, a drawing the checker rejects or
//!   a branch that meets its trunk on another layer is drawn anyway on its least violating
//!   geometry; every rule it breaks is priced by the `violation` factor and reported with place,
//!   type and cause (`RouteResult::violations`). With `allow_violations = false` these fail as
//!   below (the legal-only output);
//! * failed with a reason (P7 wording): no topological path, a plane pad without a via site, an
//!   unsupported terminal, a differential pair ("not in the rough version"), time limit.
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
use super::diffpair::{self, Pairs};
use super::draw::{self, plan, Diag, Drawer, Plan, WireOut};
use super::factors::{standard_factors, Event, Finding, Registry, Report, WireCtx};
use super::init::{Conn, InitWeights};
use super::pack::Packer;
use super::refine::CapModel;
use super::layers::{self, LayerParams};
use super::planes;
use super::state::{ObsId, State, VertId, VertKind, WireId, NO_OBS};
use super::tree;
use crate::check::Checker;
use crate::control::{cancelled, RunControl};
use crate::decompose::{decompose_with, Connection, TermKind, TermRef};
use crate::geom::Vec2;
use crate::model::{BoardModel, Item, Options};
use crate::route::{kicad, term_out, unrouted, ConnReport, NetRouteReport, RipReport, RouteResult};
use crate::vias::via_sizes;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

const MM: f64 = 1e6;

/// Failure reason of a differential pair (counted separately in the gate).
pub const DIFF_PAIR: &str = "diff pair: not in the rough version";

/// Nets that are halves of a differential pair (D7): the pairs the plugin put into the model
/// (`rules.diff_pairs`, KiCad's `MatchDpSuffix` on the full net name in `toporlib.diffpair`, the one
/// definition). O(P log P).
pub fn diff_pair_nets(model: &BoardModel) -> BTreeSet<i32> {
    model.rules.diff_pairs.iter().filter(|p| p.p != p.n).flat_map(|p| [p.p, p.n]).collect()
}

/// Record the drawn wire `wo` as the outcome of connection `ci`.
/// `octi`: the octilinear style is on; every stretch not at 0/45/90 degrees and every acute bend
/// of the drawn runs is an event (`any_angle`, `acute_bend`, M-45) next to the necked stretches.
#[allow(clippy::too_many_arguments)]
fn settle(octi: bool, out: &mut [Outcome], necking: &mut Vec<(usize, Event)>, ci: usize, w: WireId, wo: &WireOut, b: &board::Board, st: &State) {
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
    o.viol = wo.viol.clone();
    o.runs = wo.runs.iter().map(|(l, p)| (b.signal_layers[*l], p.clone())).collect();
    let pad = |ob: ObsId| if ob == super::state::NO_OBS { usize::MAX } else { b.pad_of.get(ob).copied().unwrap_or(usize::MAX) };
    o.ends = (pad(st.wires[w].pa), pad(st.wires[w].pb));
    for n in &wo.necks {
        necking.push((ci, Event::Necking { wire: w, from_nm: st.wires[w].w, to_nm: n.1, mm: n.2 / MM }));
    }
    if octi {
        for (_, pts) in &wo.runs {
            let (any, acute) = super::octi::breaks(pts);
            for (at, deg, len) in any {
                necking.push((ci, Event::AnyAngle { wire: w, at, deg, mm: len / MM }));
            }
            for (at, interior_deg) in acute {
                necking.push((ci, Event::AcuteBend { wire: w, at, interior_deg }));
            }
        }
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
    /// Rules its written copper breaks (M-B1 never drop); empty for a legal connection.
    viol: Vec<draw::Viol>,
    /// The drawn centre line per run `(layer id, polyline)` and the model pads at the wire's two
    /// ends (`usize::MAX`: no pad), for the D7 pair realisation.
    runs: Vec<(u32, Vec<Vec2>)>,
    ends: (usize, usize),
    /// D7: statistics of a realised pair connection (on the P connection) and the pair factors'
    /// events (priced in the result's penalty).
    dp: Option<diffpair::ConnStats>,
    dp_events: Vec<Event>,
    /// Why its drawing found no legal geometry (the failed attempt's reason), for the report.
    cause: String,
}

impl Outcome {
    /// Routed with legal copper.
    fn clean(&self) -> bool {
        self.routed && self.viol.is_empty()
    }
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
/// Iterations like TopoR (M-B0, [`super::schedule`]): `[optimize] runs` independent seeded runs
/// (in parallel on the engine's thread pool), each a sequence of whole-route cycles with a
/// fluctuation width that narrows and reheats, until its routes are stable (headless: then it
/// ends) or the user's OK / Cancel, the safety cap `topo_rounds` or the emergency stop
/// `wall_clock_s`. The best result by the objective over all runs is returned.
///
/// Reproducible: every stop that changes the result is a work cap or the stable test, which
/// count cycles, never time. The time budget (`wall_clock_s`) is only an emergency stop; when
/// it fires the result says so in a warning ([`TIME_HIT`]), because then it depends on the
/// machine's speed and load. The user's OK after "Routes stable" ends a run on purpose.
pub fn route_topo(model: &BoardModel, opts: &Options, ctl: &RunControl) -> RouteResult {
    let t0 = Instant::now();
    let n = opts.runs.max(1);
    let obj = Objective::parse(&opts.objective, opts.mm_per_via);
    ctl.update("setup", 0, 0, 0, 0, 0.0);
    ctl.with_progress(|p| {
        p.runs = n as u32;
        p.rounds = opts.topo_rounds.max(1) as u32;
    });
    ctl.stage(1, opts.topo_rounds.max(1) as u32, "plane stubs (pads of plane nets onto their pour)");
    let deadline = (opts.wall_clock_s > 0.0).then(|| t0 + std::time::Duration::from_secs_f64(opts.wall_clock_s));
    let stubs = place_plane_stubs(model, opts, ctl, deadline);
    let coord = Coord::new(ctl, n, obj);
    let mut outs: Vec<RunOut> = run_all(model, opts, ctl, &coord, &stubs, t0, n);
    // The best run: fewest errors, then the objective, then shrinkages; ties: the lower run.
    let mut pick = 0;
    for k in 1..outs.len() {
        if obj.better(&outs[k].crit, &outs[pick].crit) {
            pick = k;
        }
    }
    let lines: Vec<String> = outs.iter().map(|o| o.line.clone()).collect();
    let cycle_lines: Vec<String> = outs.iter().map(|o| o.cycles_line.clone()).collect();
    let stable_all = outs.iter().all(|o| o.stable_at.is_some());
    let cancel = ctl.is_cancelled();
    let time_hit = outs.iter().any(|o| o.time_hit);
    // Every cycle's variants were stitched before they were judged (pour pieces still cut off
    // count as errors), so the picked board is the one the progress showed.
    let mut best = outs.swap_remove(pick).result;
    best.warnings.extend(cycle_lines);
    if n > 1 {
        best.warnings.push(format!("best of {n} seeded runs ([optimize] runs): run {} picked; {}", pick + 1, lines.join("; ")));
    }
    best.stop_reason = if cancel {
        if stable_all { "stable" } else { "cancel" }
    } else if time_hit {
        "timeout"
    } else if stable_all {
        "stable"
    } else {
        "round cap"
    }
    .into();
    best.timed_out = time_hit && !(cancel && stable_all);
    best.converged = !best.timed_out;
    best.runtime_s = t0.elapsed().as_secs_f64();
    if best.timed_out {
        let why = if cancel { "Cancel stopped the run before its routes were stable".to_string() } else { format!("run.timeout_s = {:.0} s stopped the run before its routes were stable", opts.wall_clock_s) };
        best.warnings.push(format!("{TIME_HIT} ({why})"));
    }
    best
}

/// One run of a best-of-N call, as [`route_topo`] needs it back.
struct RunOut {
    result: RouteResult,
    crit: Criteria,
    /// The cycle after which its routes were first stable.
    stable_at: Option<usize>,
    /// Cancel or the time budget ended it before its routes were stable.
    time_hit: bool,
    /// Its summary for the report, and its cycle by cycle line.
    line: String,
    cycles_line: String,
}

/// The coordinator of the runs of one call: the progress fields of M-B0 over all runs.
struct Coord<'a> {
    ctl: &'a RunControl,
    obj: Objective,
    st: std::sync::Mutex<Vec<RunState>>,
}

#[derive(Clone, Default)]
struct RunState {
    cycle: usize,
    stable: bool,
    done: bool,
    gain: Option<f64>,
    /// Best so far: criteria, routed, total.
    best: Option<(Criteria, usize, usize)>,
}

impl<'a> Coord<'a> {
    fn new(ctl: &'a RunControl, n: usize, obj: Objective) -> Coord<'a> {
        Coord { ctl, obj, st: std::sync::Mutex::new(vec![RunState::default(); n]) }
    }

    /// Run `k` starts cycle `cycle` with fluctuation `width`.
    fn begin(&self, k: usize, cycle: usize, width: f64) {
        let Ok(mut s) = self.st.lock() else { return };
        s[k].cycle = cycle;
        let round = s.iter().map(|r| r.cycle).max().unwrap_or(cycle) as u32;
        self.ctl.with_progress(|p| {
            p.round = round;
            p.width = width;
        });
    }

    /// Run `k` ended a cycle (or the run, `done`); publish the M-B0 progress over all runs.
    fn report(&self, k: usize, upd: impl FnOnce(&mut RunState)) {
        let Ok(mut s) = self.st.lock() else { return };
        upd(&mut s[k]);
        let n = s.len();
        let stable_n = s.iter().filter(|r| r.stable).count();
        let settled = s.iter().all(|r| r.stable || r.done);
        let gain = s.iter().filter(|r| !r.done).map(|r| r.gain).try_fold(None::<f64>, |acc, g| g.map(|g| Some(acc.map_or(g, |a: f64| a.max(g)))));
        let mut bi: Option<usize> = None;
        for (i, r) in s.iter().enumerate() {
            if let Some((c, _, _)) = &r.best {
                if bi.is_none_or(|j| self.obj.better(c, &s[j].best.as_ref().expect("best").0)) {
                    bi = Some(i);
                }
            }
        }
        self.ctl.with_progress(|p| {
            p.runs = n as u32;
            p.runs_stable = stable_n as u32;
            p.runs_done = s.iter().filter(|r| r.done).count() as u32;
            p.stable = settled && stable_n > 0;
            p.gain_pct = gain.flatten();
            if let Some(i) = bi {
                let (c, routed, total) = s[i].best.expect("best");
                p.best_routed = routed;
                p.best_total = total;
                p.best_length_mm = c.length_mm;
                p.best_vias = c.vias;
                p.best_run = i as u32 + 1;
                p.best_violations = c.violations;
            }
        });
    }
}

/// Seed of run `k` of a best-of-N call: run 0 keeps the configured seed.
fn run_seed(seed: u64, k: usize) -> u64 {
    if k == 0 {
        seed
    } else {
        seed.wrapping_add(0x9E37_79B9_7F4A_7C15u64.wrapping_mul(k as u64)).rotate_left(17) ^ 0xD1B5_4A32_D192_ED03
    }
}

/// One run of a best-of-N call while it iterates: its archive, schedule, stable meter, drawing
/// feedback and negotiation memory. A cycle ([`Run::step`]) depends only on this state, so the
/// run's result does not depend on how the cycles of the runs are interleaved on the threads.
struct Run {
    k: usize,
    seed: u64,
    arch: Archive<RouteResult>,
    sched: super::schedule::Schedule,
    meter: super::schedule::Meter,
    /// Drawing feedback (plan S2c item 4): the rooms the drawing found too small, lowered by what
    /// was missing, in every later cycle.
    all: BTreeMap<(usize, usize), f64>,
    mem: Memory,
    notes: Vec<String>,
    time_hit: bool,
    stable_at: Option<usize>,
    first: Option<usize>,
    cycles: usize,
    picked_at: usize,
    /// Router time of its cycles (s).
    busy_s: f64,
    done: bool,
}

impl Run {
    fn new(opts: &Options, k: usize) -> Run {
        Run {
            k,
            seed: run_seed(opts.seed, k),
            arch: Archive::new(opts.variants),
            sched: super::schedule::Schedule::new(opts.fluctuation, opts.fluctuation_narrowing, opts.fluctuation_stuck, opts.fluctuation_reheat),
            meter: super::schedule::Meter::new(opts.stable_window, opts.stable_gain_pct),
            all: BTreeMap::new(),
            mem: Memory::default(),
            notes: Vec::new(),
            time_hit: false,
            stable_at: None,
            first: None,
            cycles: 0,
            picked_at: 0,
            busy_s: 0.0,
            done: false,
        }
    }

    /// One whole-route cycle; sets `done` when the run ends (stable for a run that stops by
    /// itself, the safety cap, Cancel / OK, the time budget).
    #[allow(clippy::too_many_arguments)]
    fn step(&mut self, model: &BoardModel, opts: &Options, ctl: &RunControl, coord: &Coord, stubs: &PlaneStubs, t_call: Instant) {
        let t = Instant::now();
        let obj = coord.obj;
        let cycle = self.cycles + 1;
        let over_budget = opts.wall_clock_s > 0.0 && t_call.elapsed().as_secs_f64() >= opts.wall_clock_s;
        if cycle > opts.topo_rounds.max(1) || (cycle > 1 && (ctl.is_cancelled() || cancelled() || over_budget)) {
            self.time_hit = cycle <= opts.topo_rounds.max(1) && self.stable_at.is_none();
            self.done = true;
            return;
        }
        // The first cycle of the first run routes with the plain weights (the reference).
        let reference = self.k == 0 && cycle == 1;
        let width = if reference { 0.0 } else { self.sched.width };
        coord.begin(self.k, cycle, width);
        let mut o2 = opts.clone();
        o2.seed = self.seed;
        o2.fluctuation = width;
        if opts.wall_clock_s > 0.0 {
            o2.wall_clock_s = (opts.wall_clock_s - t_call.elapsed().as_secs_f64()).max(1.0);
        }
        let hv: Hints = if opts.drawing_feedback { self.all.iter().map(|(&(e, l), &r)| (e, l, r)).collect() } else { Vec::new() };
        let (ro, h2, mem2) = route_round(model, &o2, ctl, &hv, &self.mem, cycle, stubs);
        self.cycles = cycle;
        self.mem = mem2;
        for (e, l, r) in h2 {
            let v = self.all.entry((e, l)).or_insert(r);
            *v = v.min(r);
        }
        let interrupted = ro.result.timed_out;
        let routed = ro.result.routed;
        let (mut cyc_err, mut cyc_viol) = (ro.crit.errors, ro.crit.violations);
        let total = ro.result.connections_total;
        self.first.get_or_insert(routed);
        // An interrupted cycle is offered only when nothing else is there: what OK returns is the
        // best of the cycles that finished.
        // Errors of the best result before this cycle (for the soft cuts below).
        let best_before = self.arch.pick(&obj).map(|i| self.arch.items[i].crit.errors);
        if !interrupted || self.arch.items.is_empty() {
            // Pour stitching (D4) on each variant before it is judged: a pour piece the copper cut
            // off that no stitch joins back is an open connection in KiCad's DRC, so it counts as an
            // error of the variant (else the pick prefers short copper that islands a pour).
            let deadline = (opts.wall_clock_s > 0.0).then(|| t_call + std::time::Duration::from_secs_f64(opts.wall_clock_s));
            let stitched = |mut r: RouteResult, mut c: Criteria| -> (RouteResult, Criteria) {
                if opts.stitch_islands {
                    ctl.stage(cycle as u32, opts.topo_rounds.max(1) as u32, "pour stitching");
                    c.errors += stitch_pours(model, opts, &mut r, deadline);
                }
                (r, c)
            };
            // A variant whose stitching the time budget cut short has its cut-off pour pieces
            // uncounted, so it would look better than it is: offered only when nothing else is
            // there, like an interrupted cycle (M-B2: measurement runs cut at 600 s picked such
            // variants and KiCad found the GND pieces open).
            let cut = || deadline.is_some_and(|d| Instant::now() >= d);
            let empty = self.arch.items.is_empty();
            let RoundOut { result, crit, pre } = ro;
            if let Some((c, r)) = pre {
                let (r, c) = stitched(r, c);
                if !cut() || empty {
                    self.arch.offer(Variant { label: format!("round {cycle} as drawn"), crit: c, payload: r }, &obj);
                }
            }
            let (r, c) = stitched(result, crit);
            (cyc_err, cyc_viol) = (c.errors, c.violations);
            if !cut() || self.arch.items.is_empty() {
                self.arch.offer(Variant { label: format!("round {cycle}"), crit: c, payload: r }, &obj);
            }
        }
        let pick = self.arch.pick(&obj).expect("a cycle was offered");
        let pc = self.arch.items[pick].crit;
        // Soft cuts (M-B2, Haas 2026 §VI-E: "every hard cut so far becomes a soft cost"): a cycle
        // worse than the best so far turns the lowered rooms into a price on their passages (edge
        // history, which fades like the rest), so they no longer forbid what the plan needs.
        if opts.soft_cuts && !self.all.is_empty() && best_before.is_some_and(|b| cyc_err > b) {
            let n = self.all.len();
            for &(e, _) in self.all.keys() {
                if self.mem.edge.len() <= e {
                    self.mem.edge.resize(e + 1, 0.0);
                }
                self.mem.edge[e] += 1.0;
            }
            self.all.clear();
            self.notes.push(format!("{cycle}: worse than the best ({cyc_err} > {} errors): {n} lowered rooms turned soft", best_before.unwrap_or(0)));
        }
        let gained = self.meter.push(super::schedule::Score { incomplete: pc.errors, value: obj.value(&pc), scale: obj.base(&pc) });
        if gained {
            self.picked_at = cycle;
        }
        if self.meter.stable() && self.stable_at.is_none() {
            self.stable_at = Some(cycle);
        }
        let best_routed = self.arch.items[pick].payload.routed;
        let gain = self.meter.gain_pct();
        self.notes.push(format!(
            "{cycle}: w {width:.3}, routed {routed}, errors {cyc_err} ({cyc_viol} violations){}{}",
            if gained { " (new best)" } else { "" },
            gain.map(|g| format!(", gain {g:.2} %")).unwrap_or_default()
        ));
        let stable = self.stable_at.is_some();
        coord.report(self.k, |r| {
            r.stable = stable;
            r.gain = gain;
            r.best = Some((pc, best_routed, total));
        });
        self.busy_s += t.elapsed().as_secs_f64();
        if interrupted {
            self.time_hit = self.stable_at.is_none();
            self.done = true;
            return;
        }
        if !reference && self.sched.after_cycle(gained, pc.errors > 0) {
            self.notes.push(format!("{cycle}: stuck {} cycles, reheat to {:.3}", opts.fluctuation_stuck, self.sched.width));
        }
        if let Some(s) = self.stable_at {
            // Headless: stop at stable (+ patience). With the dialog: OK, or finish by itself when the
            // best has not improved for `finish_patience` cycles after stable.
            if opts.stop_when_stable && cycle >= s + opts.stable_patience
                || !opts.stop_when_stable && opts.finish_when_stable && cycle >= s.max(self.picked_at) + opts.finish_patience
            {
                self.done = true;
            }
        }
    }

    /// The run's result: its archive's pick, with the variant table and its cycle line.
    fn finish(self, opts: &Options, coord: &Coord) -> RunOut {
        let obj = coord.obj;
        let pick = self.arch.pick(&obj).expect("round 1 is always offered");
        let crit = self.arch.items[pick].crit;
        let mut best = self.arch.items[pick].payload.clone();
        best.variants = self.arch.table(Some(pick));
        best.warnings.push(format!("{} ({} offered)", table_text(&best.variants, &obj), self.arch.offered));
        let run = if opts.runs.max(1) == 1 { String::new() } else { format!("run {} ", self.k + 1) };
        let stable_txt = match self.stable_at {
            Some(s) => format!("stable after cycle {s}"),
            None => "not stable".into(),
        };
        let cycles = self.cycles;
        let cycles_line = format!(
            "{run}cycles (fluctuation schedule {:.2} x {:.2}/cycle, reheat {:.2} after {} without gain; stable below {:.2} % over {} cycles): {cycles} cycles, {stable_txt}, {} reheats; first cycle routed {} of {}, picked {} (routed {}); {}",
            opts.fluctuation,
            opts.fluctuation_narrowing,
            opts.fluctuation_reheat,
            opts.fluctuation_stuck,
            opts.stable_gain_pct,
            opts.stable_window,
            self.sched.reheats,
            self.first.unwrap_or(0),
            best.connections_total,
            self.arch.items[pick].label,
            best.routed,
            self.notes.join("; ")
        );
        coord.report(self.k, |r| r.done = true);
        let line = format!(
            "run {} (seed {}): routed {} of {}, objective {:.0}, {cycles} cycles, {stable_txt}, best at cycle {}, {:.0} s",
            self.k + 1,
            self.seed,
            best.routed,
            best.connections_total,
            obj.value(&crit),
            self.picked_at,
            self.busy_s
        );
        RunOut { result: best, crit, stable_at: self.stable_at, time_hit: self.time_hit, line, cycles_line }
    }
}

/// The runs' cycles on `workers` threads: a free thread takes the run with the fewest cycles
/// that no other thread is stepping (so every run progresses, also when there are more runs than
/// threads, and "Routes stable" can come from all of them). The threads are the engine's share
/// for the runs (`min(runs, rayon threads)`), the checks inside a cycle use the rayon pool.
fn run_all(model: &BoardModel, opts: &Options, ctl: &RunControl, coord: &Coord, stubs: &PlaneStubs, t_call: Instant, n: usize) -> Vec<RunOut> {
    use std::sync::Mutex;
    let runs: Vec<Mutex<Run>> = (0..n).map(|k| Mutex::new(Run::new(opts, k))).collect();
    let taken: Mutex<Vec<bool>> = Mutex::new(vec![false; n]);
    let cycles: Mutex<Vec<(usize, bool)>> = Mutex::new(vec![(0, false); n]);
    let worker = || loop {
        let k = {
            let (Ok(mut tk), Ok(cy)) = (taken.lock(), cycles.lock()) else { return };
            let Some(k) = (0..n).filter(|&k| !tk[k] && !cy[k].1).min_by_key(|&k| (cy[k].0, k)) else { return };
            tk[k] = true;
            k
        };
        let Ok(mut r) = runs[k].lock() else { return };
        let c = if n == 1 { ctl.clone() } else { ctl.for_run(k as u32 + 1) };
        r.step(model, opts, &c, coord, stubs, t_call);
        let st = (r.cycles, r.done);
        drop(r);
        if let (Ok(mut tk), Ok(mut cy)) = (taken.lock(), cycles.lock()) {
            tk[k] = false;
            cy[k] = st;
        }
    };
    let workers = n.min(rayon::current_num_threads()).max(1);
    if workers == 1 {
        worker();
    } else {
        std::thread::scope(|s| {
            for _ in 0..workers {
                s.spawn(worker);
            }
        });
    }
    runs.into_iter().map(|m| m.into_inner().expect("run state").finish(opts, coord)).collect()
}

/// Pour stitching (D4, [`super::stitch`]) on the picked result: the stitches join its items
/// (`item_conn` = `usize::MAX`: they belong to no connection) and a summary line plus one line
/// per cut-off piece go to the warnings. Returns the pieces still cut off afterwards.
fn stitch_pours(model: &BoardModel, opts: &Options, best: &mut RouteResult, deadline: Option<Instant>) -> usize {
    let pour_nets: BTreeSet<i32> = model.pours.iter().map(|z| z.net).collect();
    if pour_nets.is_empty() {
        return 0;
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
        return 0;
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
    s.after
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
        RoundOut { crit: Criteria { errors: r.failed, open: r.failed, ..Criteria::default() }, result: r, pre: None }
    }
}

/// Warning when the time budget, the emergency stop, ended any part of the run.
pub const TIME_HIT: &str = "time budget hit: result may differ between runs";

/// What a whole route remembers for the next one (negotiated rip-up and reroute across repeats).
/// Everything here fades by `[optimize] history_fade` per cycle (M-B2, Dr. CU §IV-C).
#[derive(Clone, Debug, Default)]
struct Memory {
    /// Per edge: rounds in which it was over-full (the `congestion` history all wires pay), plus
    /// the soft cuts (lowered rooms turned into a price, Haas 2026 §VI-E).
    edge: Vec<f64>,
    /// Per connection id: the edges of the stretch where its drawing broke a rule (or was
    /// rejected), weighted by how near they are to the violation, which only that connection pays
    /// for (it is the one being ripped up and rerouted). M-B2: the violating stretch only, not
    /// the whole sleeve.
    own: BTreeMap<usize, BTreeMap<usize, f64>>,
    /// Per connection id: its planned via count in the cycle before (Polubasov 2010: the wires
    /// whose via count rose after the layer assignment are rerouted first).
    vias: BTreeMap<usize, usize>,
}

/// Weight of a pass at distance `d` from a violation whose stretch has the radius `r` (nm):
/// 1 on the stretch, falling smoothly with the distance (no cutoff; M-B2 local history).
fn near_weight(d: f64, r: f64) -> f64 {
    let x = d / r.max(1.0);
    1.0 / (1.0 + x * x)
}

/// Distance from `p` to the segment `a`-`b`.
fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.len2().max(1.0)).clamp(0.0, 1.0);
    (a + ab * t - p).len()
}

/// Smallest local history a cycle keeps (a pass weighted below it, or faded below it, is
/// forgotten: memory size, not a judgement on the copper).
const HIST_EPS: f64 = 0.02;

/// Lowered rooms `(edge, layer, room nm)` that a second round applies.
type Hints = Vec<(usize, usize, f64)>;

/// The projection options of a route (the same in every round, so edge ids stay comparable).
fn build_opts(opts: &Options) -> BuildOpts {
    BuildOpts { existing_vias: true, keepouts: true, plane_layers: opts.plane_signals != "forbidden", copper_shapes: true, ..BuildOpts::default() }
}

/// The plane stubs of a route, placed once on the static board before any topology (D6a): the
/// planner sees them as obstacles (`extra`), so it leaves them room and no signal is planned
/// where a stub goes (wiccon-2023 GPIO0 vs the MODE1.2 GND stub, D5). `res` per plane
/// connection id, in the plane stage's order.
#[derive(Default)]
struct PlaneStubs {
    res: Vec<(usize, Result<planes::Out, String>)>,
    extra: Vec<board::Extra>,
    /// Pads of pour nets that no stub via inside the pour's fill serves: the decomposition joins
    /// them by signal connections ([`crate::decompose::decompose_with`], D6b).
    signal_pads: BTreeSet<usize>,
    /// One report line per such pad.
    notes: Vec<String>,
}

/// The pad-to-pour connections of `dec` (the plane stage's requests), as `route_round` sorts them.
fn plane_requests(model: &BoardModel, b: &board::Board, dec: &crate::decompose::Decomposition) -> Vec<planes::Req> {
    let diff = diff_pair_nets(model);
    let mut obs_of_pad: BTreeMap<usize, ObsId> = BTreeMap::new();
    for (o, &p) in b.pad_of.iter().enumerate().filter(|(_, &p)| p != usize::MAX) {
        obs_of_pad.insert(p, o);
    }
    let mut out = Vec::new();
    for c in &dec.connections {
        if diff.contains(&c.net) {
            continue;
        }
        if let (TermKind::Pad(pa), TermKind::Pour(zi)) = (&c.from.kind, &c.to.kind) {
            if let Some(&oa) = obs_of_pad.get(pa) {
                out.push(planes::Req { conn: c.id, pad: *pa, obs: oa, net: c.net, w: c.width as f64, pour: *zi });
            }
        }
    }
    out
}

/// Place every plane stub on the static board (pads, holes, outline, keepouts, copper shapes,
/// existing copper; the stubs placed before it), each at its cheapest site ([`planes::place`]).
fn place_plane_stubs(model: &BoardModel, opts: &Options, ctl: &RunControl, deadline: Option<Instant>) -> PlaneStubs {
    // Cancel / the time limit end the placement at once (K1: Cancel was ignored here on a live run); the
    // route that follows sees the same stop and returns the best result so far.
    let stop = || cancelled() || ctl.is_cancelled() || deadline.is_some_and(|d| Instant::now() >= d);
    let signal: Vec<u32> = model.layers.iter().filter(|l| l.routable).map(|l| l.id).collect();
    if signal.len() < 2 {
        return PlaneStubs::default();
    }
    let Ok(b) = board::build(model, &build_opts(opts)) else { return PlaneStubs::default() };
    let (reg, _) = Registry::from_config(standard_factors(), &opts.factors);
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
    let cost = planes::SiteCost { mm: reg.price(&Event::Length { wire: 0, mm: 1.0, scale: 1.0 }, &WireCtx::default()), reg: &reg };
    // A pad whose stub finds no via site inside the pour's fill is joined by a signal connection
    // instead; the decomposition then changes (connection ids), so place again until no new pad
    // drops out (each pass drops at least one pad, so at most one pass per plane pad).
    let mut signal_pads: BTreeSet<usize> = BTreeSet::new();
    let mut notes: Vec<String> = Vec::new();
    loop {
        let dec = decompose_with(model, opts, &signal, &signal_pads);
        let reqs = plane_requests(model, &b, &dec);
        let st = State::new(&b.proj);
        let mut dr = Drawer::new(model, opts, &b, &st, &checker);
        let mut out = PlaneStubs::default();
        let mut vias: Vec<(Vec2, f64)> = Vec::new();
        let mut dropped = false;
        for rq in &reqs {
            if stop() {
                return PlaneStubs::default();
            }
            let r = planes::place(&mut dr, rq, &cost, &vias);
            match &r {
                Ok(o) => {
                    out.extra.extend(planes::extras(&b, rq, o));
                    if let Some(v) = o.via {
                        vias.push((v, 0.5 * o.size.d as f64));
                    }
                }
                // No clean via site inside the pour's fill near the pad: a signal connection
                // joins it to its net's copper instead.
                Err(e) => {
                    notes.push(format!("{e}: joined to its net's copper by a signal connection instead"));
                    signal_pads.insert(rq.pad);
                    dropped = true;
                }
            }
            out.res.push((rq.conn, r));
        }
        if !dropped {
            out.signal_pads = signal_pads;
            out.notes = notes;
            return out;
        }
    }
}

/// One whole route. `hist0` is the per-edge over-full history of earlier rounds (empty = none);
/// the third result is the history at the end of this one.
fn route_round(model: &BoardModel, opts: &Options, ctl: &RunControl, hints: &[(usize, usize, f64)], mem: &Memory, round: usize, stubs: &PlaneStubs) -> (RoundOut, Hints, Memory) {
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
    let (rnd, rnds) = (round as u32, opts.topo_rounds.max(1) as u32);
    ctl.update("setup", 0, 0, 0, 0, 0.0);
    ctl.stage(rnd, rnds, "projection of the board");
    let mut dec = decompose_with(model, opts, &signal, &stubs.signal_pads);
    // D7: the pair connections paired up (the N tree mirrors the P tree) and the planning model in
    // which the N half carries the P net (the pair is one wire, the lead).
    let pairs: Pairs = if opts.diffpair_route { diffpair::plan(model, &mut dec) } else { Pairs::default() };
    let model_real = model;
    let model_alias;
    let model: &BoardModel = if pairs.coupled.is_empty() {
        model
    } else {
        model_alias = diffpair::alias(model, &pairs);
        &model_alias
    };
    let dec = dec;
    let mut warnings: Vec<String> = dec.warnings.clone();
    warnings.extend(pairs.notes.iter().cloned());
    warnings.extend(stubs.notes.iter().cloned());
    for (net, r) in &dec.skipped {
        warnings.push(format!("net {net} skipped: {r}"));
    }
    let (reg, problems) = Registry::from_config(standard_factors(), &opts.factors);
    warnings.extend(problems);
    let mut b = match board::build_full(model, &build_opts(opts), &[], &stubs.extra) {
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
        if diff.contains(&c.net) && !opts.diffpair_route {
            fail(&mut out, i, DIFF_PAIR);
            continue;
        }
        // D7: the N connection of a coupled pair is drawn with its lead (the P connection).
        if pairs.carried.contains_key(&i) {
            fail(&mut out, i, "diff pair: carried by its P connection");
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
                // D7: a pair's lead is one wire as wide as the pair (envelope), at the larger
                // clearance of the two halves (the alias model's rule).
                let (w, s) = match pairs.of_lead(i) {
                    Some(cp) => (cp.envelope(), model.rules.net_rule(c.net).clearance as f64),
                    None => (c.width as f64, c.clearance as f64),
                };
                conns.push(Conn { net: c.net, pa: oa, pb: ob, a: va, b: vb, w, s });
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
    ctl.stage(rnd, rnds, "topological search");
    // ---- P2 ----
    let proj = &b.proj;
    let mut st = State::new(proj);
    st.cross_vias = opts.crossing_vias == "via";
    // M-C: the plan necks a wire where its nominal width does not fit (width in P2).
    if opts.neck_to_pad {
        for c in &conns {
            let f = draw::Drawer::neck_floor(&model.rules, c.net, c.w);
            if f < c.w {
                let e = st.floors.entry(c.net).or_insert(f);
                *e = e.min(f);
            }
        }
    }
    st.neck_from_pad = opts.neck_from_pad;
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
    // Net trees that follow the routing (D5, `tree`): the pad centre vertices per net and the free
    // radius of every free / node vertex (a junction candidate).
    let tree_free = opts.net_tree != "fixed";
    let mut pad_verts: BTreeMap<i32, Vec<(VertId, ObsId)>> = BTreeMap::new();
    for c in &conns {
        pad_verts.entry(c.net).or_default().extend([(c.a, c.pa), (c.b, c.pb)]);
    }
    for v in pad_verts.values_mut() {
        v.sort_unstable();
        v.dedup();
    }
    // Copper graphics are no obstacles of the projection; a branch point keeps off them too.
    let shapes: Vec<Vec<Vec2>> = model.copper_shapes.iter().filter(|c| c.poly.len() >= 3).map(|c| c.poly.iter().map(|p| p.to_vec2()).collect()).collect();
    let shape_dist = |p: Vec2| -> f64 {
        let mut d = f64::INFINITY;
        for poly in &shapes {
            if super::state::point_in_poly(p, poly) {
                return 0.0;
            }
            for i in 0..poly.len() {
                let (a, c) = (poly[i], poly[(i + 1) % poly.len()]);
                let e = c - a;
                let t = ((p - a).dot(e) / e.len2().max(1e-12)).clamp(0.0, 1.0);
                d = d.min((p - (a + e * t)).len());
            }
        }
        d
    };
    let rad: Vec<f64> = if tree_free {
        (0..proj.pos.len())
            .map(|v| if matches!(proj.vkind[v], VertKind::Free | VertKind::Node) { proj.free_radius(proj.pos[v], i32::MIN).min(shape_dist(proj.pos[v])) } else { 0.0 })
            .collect()
    } else {
        Vec::new()
    };
    let smd_obs: Vec<bool> = (0..proj.obstacles.len()).map(|o| b.smd_layer(o).is_some()).collect();
    let br_reward = reg.price(&Event::Branch { wire: 0, at: Vec2::default(), on_pad: false, smd: false }, &WireCtx::default());
    let ecx = tree::EndsCtx {
        pads: &pad_verts,
        rad: &rad,
        junctions: tree_free,
        reward: br_reward,
        smd_cost: reg.price(&Event::Branch { wire: 0, at: Vec2::default(), on_pad: true, smd: true }, &WireCtx::default()) - br_reward,
        smd: &smd_obs,
        pad_nm: opts.junction_pad_mm * MM,
    };
    // One connection into the state: with a free net tree it joins the two components of its net
    // that its pads lie in (any pad or junction of them, multi-source search); else pad to pad.
    let insert_conn = |st: &mut State, c: &Conn, w: &InitWeights| -> Option<WireId> {
        // D7: a pair's wire runs pad pair to pad pair (no branch onto the pair's own copper).
        if tree_free && !pairs.width.contains_key(&c.net) {
            let comp = tree::components(st, c.net);
            let (ca, cb) = (tree::comp_of(&comp, c.a), tree::comp_of(&comp, c.b));
            if ca != cb {
                let src = tree::ends_of(st, &ecx, c.net, &comp, ca, c.w, c.s, [c.a, c.b]);
                let dst = tree::ends_of(st, &ecx, c.net, &comp, cb, c.w, c.s, [c.a, c.b]);
                // The branch reward, the SMD assembly cost and the junction angle are priced in the
                // search (social factors): a sharp or costly branch loses to a separate path.
                if std::env::var("TOPO_TREEDBG").is_ok() {
                    // Debug: the cost of every single source end on its own.
                    for (k, e) in src.iter().enumerate() {
                        let one = st.search_tree(c, std::slice::from_ref(e), &dst, w, (c.w + c.s) / ws0, None);
                        eprintln!("TREEDBG net {} src {k} v {} at ({:.3},{:.3}) on {:?} adj {:.2}: {:?}", c.net, e.v, proj.pos[e.v].x / 1e6, -proj.pos[e.v].y / 1e6, e.on, e.adj, one.map(|p| (p.0 .0.len(), p.2)));
                    }
                }
                let ((path, tris), i, j) = st.search_tree(c, &src, &dst, w, (c.w + c.s) / ws0, None)?;
                let (sa, sb) = (src[i], dst[j]);
                for e in [sa, sb] {
                    if let Some((tw, k)) = e.on {
                        st.add_turn(tw, k, e.v);
                    }
                }
                let wire = super::state::Wire { net: c.net, w: c.w, s: c.s, a: sa.v, b: sb.v, pa: sa.obs, pb: sb.obs, turns: Vec::new(), passes: Vec::new(), tris: Vec::new(), alive: true };
                return Some(st.insert(wire, &path, tris));
            }
        }
        st.route_conn(c, w, (c.w + c.s) / ws0).map(|x| x.0)
    };
    let route_one_raw = |st: &mut State, ci: usize, cap: usize, wire_conn: &mut Vec<usize>, rg: &Registry| -> bool {
        let c = &conns[ci];
        // A path longer than `detour_factor` x the straight distance is a detour: search again with
        // cheaper crossings and layer fixings (P3 resolves a crossing with a via; a detour of many times the
        // connection's length is worse than that), the cheapest last.
        // Present cost kept level with the history (BoxRouter 2.0, Cho et al. 2009, §4.2 eq. 4,
        // p. 32:8-9: alpha = max history / present weight each round), so a passage over-full
        // now never gets cheaper than one that was over-full before (M-B2).
        let h_max = st.history.iter().copied().fold(0.0, f64::max);
        for (attempt, cross_scale) in [1.0, 0.25, 0.0].into_iter().enumerate() {
            let mut w = weights(rg, c, tier_of(conn_of[ci]), cap);
            w.overflow_mm = w.overflow_mm.max(w.congestion_mm * h_max);
            w.cross_mm *= cross_scale;
            w.smd_over_mm *= cross_scale;
            w.forced_layer_mm *= cross_scale;
            let Some(wid) = insert_conn(st, c, &w) else { return false };
            if wire_conn.len() <= wid {
                wire_conn.resize(wid + 1, usize::MAX);
            }
            wire_conn[wid] = ci;
            let straight = (proj.pos[st.wires[wid].b] - proj.pos[st.wires[wid].a]).len().max(1.0);
            let len: f64 = st.polyline(wid).windows(2).map(|x| (x[1] - x[0]).len()).sum();
            if opts.detour_factor <= 0.0 || len <= opts.detour_factor * straight || attempt == 2 {
                return true;
            }
            tree::remove_wire(st, wid);
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
    // Fluctuating coefficients of this cycle (M-B0 schedule) also in the initial search.
    let reg_p2 = if opts.fluctuation > 0.0 { reg.fluctuated(seed_of(opts.seed, round, 0), opts.fluctuation) } else { reg.clone() };
    for &ci in &order {
        if expired() {
            timed_out = true;
            fail(&mut out, conn_of[ci], "time limit reached before this connection was routed");
            continue;
        }
        if !route_one(&mut st, ci, cap, &mut wire_conn, &reg_p2) {
            p2_failed.push(ci);
        }
    }
    let swaps = st.uncross_all(50);
    // One retry of the failed connections with a larger search cap.
    let mut still: Vec<usize> = Vec::new();
    for &ci in &p2_failed {
        if expired() || !route_one(&mut st, ci, cap * 4, &mut wire_conn, &reg_p2) {
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
    ctl.stage(rnd, rnds, "layer assignment, rip-up and reroute");
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
    // P3 with the junctions' tee rule: the trunk keeps one layer through a junction and the
    // branch ends on it. Two layers: inside P3, a tee is one layer domain (D6b); more layers: a
    // fix-up after P3 (D5).
    let assign = |st: &State| {
        let mut lr = layers::assign_with(st, &|ob| b.smd_layer(ob), &lp, Some(&packer));
        if tree_free && proj.n_layers != 2 && st.wires.iter().any(|w| w.alive && !w.turns.is_empty()) {
            let mut evl = lr.events.clone();
            let bottom_l = (if b.n_signal == 0 { proj.n_layers } else { b.n_signal }) as i32 - 1;
            evl.resize(st.wires.len(), Vec::new());
            lr.mask.resize(st.wires.len(), Vec::new());
            lr.partner.resize(st.wires.len(), Vec::new());
            tree::fix_layers(st, &mut evl, &mut lr.mask, &mut lr.partner, bottom_l);
            layers::relabel(st, &evl, &lp, &mut lr);
        }
        lr
    };
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
        // Predicted violations: over-full edge-layers, vias without a site and (M-B2) crossings
        // that cannot be layered (both stretches held to one layer: a short when drawn).
        (own, cm.overfull() + lr.unlayerable.len(), lr.vias)
    };
    // Lexicographic: over-full edge-layers, then the objective.
    let p4_better = |a: &(f64, usize, usize), b: &(f64, usize, usize)| a.1 < b.1 || (a.1 == b.1 && a.0 < b.0 - 1e-6);
    // Reroute counter per connection: the P4 moves pay the `reroute` factor's price for it.
    // (Measured: ripping the least moved wires first in the negotiated rip-up routed fewer
    // connections on dcsgonebadge and qmtech, so the rip order stays routed-last first.)
    let mut reroutes: Vec<u32> = vec![0; conns.len()];
    // Wires taken out and routed again in this whole route (the `max_ripped` work cap).
    let mut ripped_total = 0usize;
    let plan_vias = |st: &State, lr: &layers::LayerReport| -> Vec<(WireId, usize)> {
        (0..st.wires.len()).filter(|&w| st.wires[w].alive && w < lr.events.len()).map(|w| (w, plan(&lr.events[w], b.n_signal).vias.len())).collect()
    };
    // Wires whose planned via count rose against the cycle before (wire, conns index).
    let via_rose: Vec<(WireId, usize)> = plan_vias(&st, &lr)
        .into_iter()
        .filter(|&(w, n)| mem.vias.get(&conn_of[wire_conn[w]]).is_some_and(|&n0| n > n0))
        .map(|(w, _)| (w, wire_conn[w]))
        .collect();
    // ---- Crossing rules (M-B2, [`super::xrules`]): clinches at terminals, double crossings that
    // enclose an obstacle and crossings that cannot be layered are re-planned locally: the pair
    // is taken out and routed again in the rule's order (`Topo::try_joint`, kept when the pair's
    // social-factor price drops); the whole pass is kept when the plan's score gets better.
    let tree_wires: BTreeSet<WireId> = if tree_free { tree::junctions(&st).iter().flat_map(|j| [j.0, j.2]).collect() } else { BTreeSet::new() };
    let mut xstats = [0usize; 4]; // found, tried, resolved, passes kept
    let mut crossing_pass = |st: &mut State, wire_conn: &mut Vec<usize>, reroutes: &mut Vec<u32>, lr: &mut layers::LayerReport, ripped_total: &mut usize, stage: usize| {
        if !opts.crossing_rules || expired() {
            return;
        }
        let tree_now: BTreeSet<WireId> = if tree_free { tree::junctions(st).iter().flat_map(|j| [j.0, j.2]).chain(tree_wires.iter().copied()).collect() } else { BTreeSet::new() };
        // Only plain pad-to-pad wires of their own connection are re-planned (a wire of a net
        // tree, or one that ends at another pad of its tree, keeps its place).
        let plain = |st: &State, wc: &[usize], w: WireId| {
            let wi = &st.wires[w];
            let Some(&ci) = wc.get(w) else { return false };
            wi.alive && ci != usize::MAX && wi.turns.is_empty() && wi.pa != NO_OBS && wi.pb != NO_OBS && !tree_now.contains(&w) && {
                let c = &conns[ci];
                (wi.a, wi.b) == (c.a, c.b) || (wi.a, wi.b) == (c.b, c.a)
            }
        };
        let mut pairs: Vec<super::xrules::Pair> = super::xrules::terminal_clinches(st);
        pairs.extend(super::xrules::double_crossings(st));
        for &(a, b) in &lr.unlayerable_pairs {
            pairs.push(super::xrules::Pair { first: a.min(b), second: a.max(b), kind: super::xrules::Kind::Unlayerable });
        }
        xstats[0] += pairs.len();
        let pairs: Vec<_> = pairs.into_iter().filter(|p| plain(st, wire_conn, p.first) && plain(st, wire_conn, p.second)).collect();
        if pairs.is_empty() {
            return;
        }
        let before = score(st, lr);
        let saved = (st.clone(), wire_conn.clone(), reroutes.clone(), lr.clone(), *ripped_total);
        let mut topo = Topo {
            reg: if opts.fluctuation > 0.0 { reg.fluctuated(seed_of(opts.seed, round, 2000 + stage), opts.fluctuation) } else { reg.clone() },
            conns: conns.clone(),
            tiers: (0..conns.len()).map(|ci| tier_of(conn_of[ci])).collect(),
            wire_conn: wire_conn.clone(),
            reroutes: reroutes.clone(),
            ws0,
            cap,
            evals: 0,
            max_evals: opts.max_moves.max(1),
            ripped: *ripped_total,
            max_ripped: opts.max_ripped,
            tight: Vec::new(),
            tally: Tally::default(),
        };
        let mut resolved = 0usize;
        let mut gone: BTreeSet<WireId> = BTreeSet::new();
        for p in &pairs {
            if expired() {
                break;
            }
            if gone.contains(&p.first) || gone.contains(&p.second) || !st.wires[p.first].alive || !st.wires[p.second].alive {
                continue;
            }
            xstats[1] += 1;
            // Route the rule's first wire first, the interfering one after it (try_joint routes
            // its second argument first).
            if topo.try_joint(st, p.second, p.first).is_some() {
                gone.extend([p.first, p.second]);
                resolved += 1;
            } else if topo.try_reroute(st, p.second).is_some() {
                gone.insert(p.second);
                resolved += 1;
            }
        }
        if resolved == 0 {
            return;
        }
        st.uncross_all(50);
        let lr2 = assign(st);
        let after = score(st, &lr2);
        if p4_better(&after, &before) {
            *wire_conn = topo.wire_conn;
            *reroutes = topo.reroutes;
            *ripped_total = topo.ripped;
            *lr = lr2;
            xstats[2] += resolved;
            xstats[3] += 1;
        } else {
            (*st, *wire_conn, *reroutes, *lr, *ripped_total) = saved;
        }
    };
    crossing_pass(&mut st, &mut wire_conn, &mut reroutes, &mut lr, &mut ripped_total, 0);
    let first_score = score(&st, &lr);
    let mut best_p4 = (st.clone(), wire_conn.clone(), lr.clone(), first_score);
    let (mut p4_rounds, mut p4_stall, mut p4_ripped) = (0usize, 0usize, 0usize);
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
        // A via without a site in its interval: its wire is rerouted, with history on the edges
        // that bound that interval only (M-B2; before: all edges of the wire).
        let mut victims: BTreeSet<WireId> = nosite.iter().copied().collect();
        for &w in &nosite {
            let n = st.wires[w].passes.len();
            for (i, &(ka, kb)) in cm.via_iv[w].iter().enumerate() {
                let Some(&k) = cm.via_k[w].get(i) else { continue };
                if !cm.rank_of(w, k).0 {
                    continue;
                }
                for kk in ka.saturating_sub(1)..(kb + 1).min(n) {
                    history[st.passes[st.wires[w].passes[kk]].edge] += 1.0;
                }
            }
        }
        // First P4 round: the wires whose planned via count rose against the cycle before are
        // rerouted first (Polubasov 2010, «Оптимизация расслоения»), once per cycle.
        if p4_rounds == 0 {
            for (w, ci) in via_rose.iter().copied() {
                if st.wires[w].alive && reroutes[ci] == 0 {
                    victims.insert(w);
                }
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
        let rose: BTreeSet<WireId> = if p4_rounds == 0 { via_rose.iter().map(|x| x.0).collect() } else { BTreeSet::new() };
        vic.sort_by_key(|&w| (!rose.contains(&w), std::cmp::Reverse(rank[wire_conn[w]])));
        vic.truncate(max_rip.min(opts.max_ripped - ripped_total));
        // A ripped trunk takes the branches that end on its junctions with it (D5 net trees).
        let vic_set: BTreeSet<WireId> = vic.iter().copied().collect();
        vic.extend(tree::dependents(&st, &vic_set));
        vic.sort_by_key(|&w| rank[wire_conn[w]]);
        if vic.is_empty() {
            break;
        }
        p4_ripped += vic.len();
        ripped_total += vic.len();
        let vic_conns: Vec<usize> = vic.iter().map(|&w| wire_conn[w]).collect();
        for &w in &vic {
            tree::remove_wire(&mut st, w);
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
            "P4 negotiated rip-up and reroute: {p4_rounds} rounds, {p4_ripped} wires ripped; objective {:.0} -> {:.0}, over-full (edge-layers + vias without a site) {} -> {}, vias {} -> {}",
            first_score.0, last_score.0, first_score.1, last_score.1, first_score.2, last_score.2
        ));
    }
    crossing_pass(&mut st, &mut wire_conn, &mut reroutes, &mut lr, &mut ripped_total, 1);
    if opts.crossing_rules {
        warnings.push(format!(
            "crossing rules (clinch at a terminal, double crossing, crossing that cannot be layered; Luzin & Popov 2012, Polubasov 2010): {} patterns found, {} pairs re-planned, {} resolved in {} kept passes",
            xstats[0], xstats[1], xstats[2], xstats[3]
        ));
    }
    tlog("P4 rip-up and reroute", t_p4);
    // ---- P4 moves: descent on the topology, then on the layer labels ----
    let mut tally = Tally::default();
    let mut term_over: BTreeMap<usize, (TermRef, TermRef)> = BTreeMap::new();
    // The topological moves re-search whole pad-to-pad connections; they do not know branch
    // points yet (D5), so a state with branch points skips them.
    let has_junctions = tree_free && st.wires.iter().any(|w| w.alive && !w.turns.is_empty());
    if opts.moves && has_junctions {
        warnings.push("P4 moves skipped: the net trees have branch points on traces, which the moves do not handle yet ([search] net_tree = \"fixed\" to use them)".into());
    }
    if opts.moves && opts.max_moves > 0 && !expired() && !has_junctions {
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
            "P4 moves on the topology: {} evaluations; {}; objective {:.0} -> {:.0} ({}); over-full (edge-layers + vias without a site) {} -> {}, vias {} -> {}",
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
        warnings.push(format!("P4 moves on the layers: {}; vias {} -> {}, over-full (edge-layers + vias without a site) {} -> {}", lt.text(), v0, lr.vias, o0, o1));
        tlog("P4 moves", t_mv);
    }
    let lr = lr;
    let t_draw = Instant::now();
    // A different-net crossing still on one layer is an error: the lower-tier wire stays unrouted.
    let wlen: Vec<f64> = (0..st.wires.len()).map(|w| if !st.wires[w].alive { 0.0 } else { st.polyline(w).windows(2).map(|x| (x[1] - x[0]).len()).sum::<f64>() }).collect();
    let wire_len = |w: WireId| wlen[w];
    let mut dropped: BTreeSet<WireId> = BTreeSet::new();
    let mut unlayerable_errors = 0usize;
    for (pi, &(wa, wb)) in lr.unlayerable_pairs.iter().enumerate() {
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
        // The pair's own crossing (D4: every pair used to name the first one's place).
        let at = lr.unlayerable_at.get(pi).map_or(String::new(), |p| format!(" near ({:.3}, {:.3}) mm", p.x / MM, -p.y / MM));
        let msg = format!(
            "crossing of {} and {}{at} cannot be layered: both stretches are fixed to the same layer (error, not emitted)",
            name(victim),
            name(other)
        );
        if opts.allow_violations {
            // Never drop (M-B1): both stay; the drawing writes the crossing as a violation.
            warnings.push(format!("{msg}; drawn anyway ([optimize] allow_violations): the crossing is a violation"));
            for c in [ca, cb] {
                if out[c].cause.is_empty() {
                    out[c].cause = format!("crossing of {} and {}{at} cannot be layered (both stretches fixed to the same layer)", name(wa), name(wb));
                }
            }
            continue;
        }
        warnings.push(msg.clone());
        fail(&mut out, cv, msg);
        dropped.insert(victim);
    }
    if std::env::var("TOPO_VIADBG").is_ok() {
        let dirs = dirs_of(&st);
        let cm = CapModel::new(&st, &packer, &dirs, &lr.events, bottom, 1.0);
        eprintln!("PLANDRAW round {round}: before the unlayerable drops ({} wires): over-full edge-layers {}, vias without a site {}", dropped.len(), cm.overfull_list().len(), cm.nosite());
    }
    // A dropped trunk takes the branches on its junctions with it (D5 net trees).
    for w in tree::dependents(&st, &dropped) {
        fail(&mut out, conn_of[wire_conn[w]], "its branch point lies on a connection dropped as unlayerable (error, not emitted)");
        dropped.insert(w);
    }
    for &w in &dropped {
        tree::remove_wire(&mut st, w);
    }
    ctl.update("route", 2, 0, nconn, lr.vias, t0.elapsed().as_secs_f64());
    ctl.stage(rnd, rnds, "drawing the traces");
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
    for cp in &pairs.coupled {
        let r = &cp.rule;
        let (d, _) = diffpair::via_of(model_real, r);
        dr.pair_keep.insert(r.p, 3.0 * (2 * r.width + r.gap) as f64 + 0.5 * (d + r.via_gap) as f64);
    }
    // The plane stubs (placed before the topology, which routed around them) are copper before
    // anything of the drawing: the wire vias and wires keep their clearance to them.
    let mut plane_res: Vec<(usize, Result<planes::Out, String>)> = Vec::new();
    for (conn, r) in &stubs.res {
        match r {
            Ok(o) => {
                for it in &o.items {
                    dr.commit(it);
                }
                out[*conn].routed = true;
                out[*conn].items = o.items.clone();
                out[*conn].vias = o.via.map(|v| (v, o.size)).into_iter().collect();
                out[*conn].layers = vec![o.layer];
                out[*conn].length_nm = o.stub_nm;
                out[*conn].stub_nm = o.stub_nm;
            }
            Err(e) => fail(&mut out, *conn, e.clone()),
        }
        plane_res.push((*conn, r.clone()));
    }
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
            // A junction's traversal holds no via of its trunk (the tee keeps one layer there),
            // except at the junction itself on two layers, where P3 makes the tee one domain and
            // the via may stand at the tee (D6b; the drawing tries only the turn vertex there).
            for &(kt, _) in st.wires[w].turns.iter().filter(|_| proj.n_layers != 2) {
                for (i, iv) in p.vias.iter_mut().enumerate() {
                    if iv.0 == kt && iv.1 > kt {
                        iv.0 = kt + 1;
                    }
                    if iv.1 == kt && iv.0 < kt {
                        iv.1 = kt - 1;
                    }
                    if let Some(a) = p.at.get_mut(i) {
                        *a = (*a).clamp(iv.0, iv.1);
                    }
                }
            }
            plans.insert(w, p);
        }
    }
    // Draw order: widest first, then shortest.
    let mut draw_order: Vec<WireId> = plans.keys().copied().collect();
    draw_order.sort_by(|&x, &y| {
        st.wires[y].w.total_cmp(&st.wires[x].w).then(wire_len(x).total_cmp(&wire_len(y))).then(x.cmp(&y))
    });
    // A trunk is drawn before the branches on it (D5 net trees: a branch ends on its drawn copper).
    let tree_js = if tree_free { tree::junctions(&st) } else { Vec::new() };
    if !tree_js.is_empty() {
        let mut depth: BTreeMap<WireId, usize> = BTreeMap::new();
        for _ in 0..tree_js.len() {
            for &(bw, _, tw, _, _) in &tree_js {
                let d = depth.get(&tw).copied().unwrap_or(0) + 1;
                if depth.get(&bw).copied().unwrap_or(0) < d {
                    depth.insert(bw, d);
                }
            }
        }
        draw_order.sort_by_key(|w| depth.get(w).copied().unwrap_or(0));
    }
    // Where each branch meets its drawn trunk: the trunk run's point nearest the junction vertex,
    // on the layer the branch ends on.
    let joint_of = |bw: WireId, drawn: &BTreeMap<WireId, WireOut>, plans: &BTreeMap<WireId, Plan>| -> Option<((WireId, usize), Vec2)> {
        let &(_, end, tw, _, v) = tree_js.iter().find(|j| j.0 == bw)?;
        let pl = plans.get(&bw)?;
        let layer = if end == 0 { *pl.layers.first()? } else { *pl.layers.last()? };
        let pv = proj.pos[v];
        let mut best: Option<(f64, Vec2)> = None;
        for (l, pts) in &drawn.get(&tw)?.runs {
            if *l != layer {
                continue;
            }
            for x in pts.windows(2) {
                let ab = x[1] - x[0];
                let u = ((pv - x[0]).dot(ab) / ab.len2().max(1.0)).clamp(0.0, 1.0);
                let q = x[0] + ab * u;
                let d = (q - pv).len();
                if best.is_none_or(|b| d < b.0) {
                    best = Some((d, q));
                }
            }
        }
        best.map(|b| ((bw, end), b.1))
    };
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
    let mut via_forced: Vec<(WireId, String)> = Vec::new();
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
                if opts.allow_violations {
                    via_forced.push((w, r));
                } else {
                    fail(&mut out, conn_of[wire_conn[w]], r);
                }
            }
        }
    }
    // Never drop (M-B1): a via without a legal site goes to its least violating site, after every
    // legal via is placed (so it takes no legal site from another wire).
    let n_via_forced = via_forced.len();
    if !via_forced.is_empty() {
        dr.force = true;
        for (w, r) in via_forced {
            let mut ids = Vec::new();
            match dr.place_vias(w, &plans[&w], &mut ids) {
                Ok(v) => {
                    set_layers(&plans[&w], Some(&v), &st.wires[w].passes, &mut dr.prov_layer);
                    placed.insert(w, v);
                    via_ids.insert(w, ids);
                    live.push(w);
                    let ci = conn_of[wire_conn[w]];
                    if out[ci].cause.is_empty() {
                        out[ci].cause = r;
                    }
                }
                Err(e) => {
                    for id in ids {
                        dr.uncommit(id);
                    }
                    fail(&mut out, conn_of[wire_conn[w]], e);
                }
            }
        }
        dr.force = false;
        // Draw order again (the forced ones were appended).
        let pos: BTreeMap<WireId, usize> = draw_order.iter().enumerate().map(|(i, &w)| (w, i)).collect();
        live.sort_by_key(|w| pos.get(w).copied().unwrap_or(usize::MAX));
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
    // The plan as the drawing gets it (D5: the layers line used to show P3's count, which
    // includes the vias without a site and the wires dropped as unlayerable afterwards).
    let (plan_over, plan_nosite, n_dropped) = (cm_plan.overfull_list().len(), cm_plan.nosite(), dropped.len());
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
    if std::env::var("TOPO_VIADBG").is_ok() {
        // Planning counter per round (D4): planned over-full edge-layers and vias without a static
        // site against the drawing's packing and its via placement.
        let (mut ns, mut ns_same, mut ns_other) = (0usize, 0usize, 0usize);
        for (&w, pl) in &plans {
            for (i, &k) in pl.at.iter().enumerate() {
                if cm_plan.rank_of(w, k).0 {
                    ns += 1;
                    match placed.get(&w).and_then(|v| v.get(i)) {
                        Some(&(kk, _, _)) if kk == k => ns_same += 1,
                        Some(_) => ns_other += 1,
                        None => {}
                    }
                }
            }
        }
        eprintln!(
            "PLANDRAW round {round}: planned over-full edge-layers {}, vias without a static site {ns} (drawing placed {ns_same} in the planned traversal, {ns_other} in another); drawing over-full {over_n}; planned fits -> drawn over {}, planned over -> drawn fits {}",
            cm_plan.overfull_list().len(),
            plan_vs_draw.0,
            plan_vs_draw.1
        );
    }
    let mut drawn_ok = 0usize;
    // Necked stretches and (M-45) the octilinear style's breaks, per connection.
    let mut necking: Vec<(usize, Event)> = Vec::new();
    let octi_on = dr.octi.on;
    let mut drawn: BTreeMap<WireId, WireOut> = BTreeMap::new();
    let mut via_h: BTreeMap<WireId, Vec<usize>> = BTreeMap::new();
    let mut failed_draw: Vec<WireId> = Vec::new();
    for &w in &live {
        if expired() {
            timed_out = true;
            fail(&mut out, conn_of[wire_conn[w]], "time limit reached before this connection was drawn");
            continue;
        }
        let ci = conn_of[wire_conn[w]];
        let mut ids = via_ids.remove(&w).unwrap_or_default();
        let vh = ids[..placed[&w].len().min(ids.len())].to_vec();
        if let Some((key, q)) = joint_of(w, &drawn, &plans) {
            dr.end_at.insert(key, q);
        }
        let wo = dr.draw_wire(w, &plans[&w], &placed[&w], &mut ids);
        if wo.ok {
            via_h.insert(w, vh);
            drawn_ok += 1;
            settle(octi_on, &mut out, &mut necking, ci, w, &wo, &b, &st);
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
        // M-B2: whether the blockers are ripped is a graded price, not a count cutoff: ripping
        // costs the `reroute` price per blocker (more for one redrawn before in this cycle);
        // keeping the rejected wire costs the `violation` price it would be drawn with (or left
        // open with). The cheapest rescues go first; `feedback_max_rip` and `max_ripped` remain
        // as work caps.
        let rr = |n: u32| reg.price(&Event::Rerouted { wire: 0, count: n }, &WireCtx::default());
        let mut redrawn_n: BTreeMap<WireId, u32> = BTreeMap::new();
        let mut cand: Vec<(f64, f64, WireId, Vec<WireId>)> = Vec::new();
        for &f in &failed_draw {
            let Some(d) = rejected.iter().rev().find(|d| d.wire == f) else { continue };
            let cf = conn_of[wire_conn[f]];
            if !out[cf].reason.starts_with("drawing:") {
                continue;
            }
            let blockers = dr.blockers(d, drawn.iter().map(|(&w, wo)| (w, wo)));
            let keep = reg.price(
                &Event::Violation {
                    kind: "clearance".into(),
                    at: d.at,
                    layer: String::new(),
                    depth_nm: (d.req_nm - d.gap_nm).max(1) as f64,
                    len_nm: (d.to - d.from).len(),
                    against: d.against.clone(),
                    cause: String::new(),
                },
                &WireCtx { net: d.net, tier: tier_of(cf) },
            );
            let rip: f64 = blockers.iter().map(|_| rr(1)).sum();
            if rip < keep {
                cand.push((rip / keep.max(1e-9), keep, f, blockers));
            }
        }
        cand.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
        for (_, keep, f, blockers) in cand {
            if expired() {
                timed_out = true;
                break;
            }
            let cf = conn_of[wire_conn[f]];
            // The blockers' price grows with each earlier redraw of them in this cycle.
            let rip: f64 = blockers.iter().map(|b| rr(1 + redrawn_n.get(b).copied().unwrap_or(0))).sum();
            if rip >= keep || blockers.len() > opts.feedback_max_rip || ripped_total + blockers.len() > opts.max_ripped {
                continue;
            }
            for b in &blockers {
                *redrawn_n.entry(*b).or_insert(0) += 1;
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
            if let Some((key, q)) = joint_of(f, &drawn, &plans) {
                dr.end_at.insert(key, q);
            }
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
                    settle(octi_on, &mut out, &mut necking, cb, bw, &wb, &b, &st);
                    drawn.insert(bw, wb);
                }
                out[cf].reason.clear();
                via_h.insert(f, ids[..vias_f.len().min(ids.len())].to_vec());
                placed.insert(f, vias_f);
                settle(octi_on, &mut out, &mut necking, cf, f, &wf, &b, &st);
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
    // ---- Never drop (M-B1): every wire still without copper (its drawing found no legal
    // geometry, the feedback did not rescue it) is drawn on its best geometry against all copper
    // drawn so far; the rules it breaks are recorded with it and priced by the `violation` factor.
    let mut n_forced = 0usize;
    if opts.allow_violations {
        dr.force = true;
        for &w in &live {
            if drawn.contains_key(&w) || expired() {
                continue;
            }
            let ci = conn_of[wire_conn[w]];
            if !out[ci].reason.starts_with("drawing:") {
                continue;
            }
            // Its vias went with its failed drawing: placed again (legal sites first).
            let mut ids: Vec<usize> = Vec::new();
            dr.via_viol.remove(&w);
            let vs = match dr.place_vias(w, &plans[&w], &mut ids) {
                Ok(v) => v,
                Err(_) => {
                    for id in ids {
                        dr.uncommit(id);
                    }
                    continue;
                }
            };
            let vh = ids[..vs.len().min(ids.len())].to_vec();
            if let Some((key, q)) = joint_of(w, &drawn, &plans) {
                dr.end_at.insert(key, q);
            }
            let wo = dr.draw_wire(w, &plans[&w], &vs, &mut ids);
            if !wo.ok {
                for id in ids {
                    dr.uncommit(id);
                }
                continue;
            }
            n_forced += 1;
            if out[ci].cause.is_empty() {
                out[ci].cause = out[ci].reason.clone();
            }
            placed.insert(w, vs);
            via_h.insert(w, vh);
            settle(octi_on, &mut out, &mut necking, ci, w, &wo, &b, &st);
            drawn.insert(w, wo);
        }
        dr.force = false;
    }
    // ---- Junctions (D5 net trees): a branch whose trunk was not drawn, or that reaches its
    // junction on another layer than the trunk passes it, is not connected: its copper goes and
    // its connection fails (locally; the rest of the tree stays).
    if tree_free {
        let js = tree::junctions(&st);
        let (mut lost_trunk, mut lost_layer) = (0usize, 0usize);
        let mut junction_joins = 0usize;
        let mut gone: BTreeSet<WireId> = BTreeSet::new();
        loop {
            let mut more = false;
            for &(bw, end, tw, _, v) in &js {
                if gone.contains(&bw) || !drawn.contains_key(&bw) {
                    continue;
                }
                let why = if !drawn.contains_key(&tw) || gone.contains(&tw) {
                    lost_trunk += 1;
                    "drawing: its branch point lies on a connection that was not drawn"
                } else {
                    let pv = dr.end_at.get(&(bw, end)).copied().unwrap_or(proj.pos[v]);
                    let on = |pts: &[Vec2]| pts.windows(2).any(|x| {
                        let ab = x[1] - x[0];
                        let u = ((pv - x[0]).dot(ab) / ab.len2().max(1.0)).clamp(0.0, 1.0);
                        (x[0] + ab * u - pv).len() < 2.0
                    });
                    let tl = drawn[&tw].runs.iter().find(|(_, pts)| on(pts)).map(|x| x.0);
                    let bl = if end == 0 { drawn[&bw].runs.first() } else { drawn[&bw].runs.last() }.map(|x| x.0);
                    // Any trunk run there on the branch's layer (a via at the junction ends one
                    // run and starts the next on the other layer, D6b).
                    if bl.is_some() && drawn[&tw].runs.iter().any(|(l, pts)| Some(*l) == bl && on(pts)) {
                        continue;
                    }
                    if std::env::var("TOPO_TREEDBG").is_ok() {
                        eprintln!(
                            "TREE layer mismatch round {round}: branch {bw} end {end} trunk {tw} at ({:.3}, {:.3}): trunk run layer {tl:?} (runs {:?}, plan {:?}, events {:?}, turns {:?}), branch end layer {bl:?} (runs {:?}, events {:?})",
                            pv.x / MM,
                            -pv.y / MM,
                            drawn[&tw].runs.iter().map(|r| (r.0, r.1.len())).collect::<Vec<_>>(),
                            plans.get(&tw).map(|p| (p.layers.clone(), p.vias.clone())),
                            lr.events.get(tw),
                            st.wires[tw].turns,
                            drawn[&bw].runs.iter().map(|r| (r.0, r.1.len())).collect::<Vec<_>>(),
                            lr.events.get(bw)
                        );
                    }
                    // Never drop (M-B1): the branch is joined to its trunk where they part, by a
                    // via at the branch end (the trunk passes there on another layer) or a track to
                    // the trunk's nearest copper (and a via there when that is on another layer).
                    if opts.allow_violations {
                        if let Some(bl) = bl {
                            let net = st.wires[bw].net;
                            let sz = via_sizes(&model.rules, net, opts)[0];
                            let mut add: Vec<Item> = Vec::new();
                            if tl.is_some() {
                                add.push(draw::via_item(net, pv, sz, Some(bw)));
                            } else {
                                let near = |only: Option<usize>| -> Option<(f64, usize, Vec2)> {
                                    drawn[&tw]
                                        .runs
                                        .iter()
                                        .filter(|(l, _)| only.is_none_or(|o| o == *l))
                                        .flat_map(|(l, pts)| pts.windows(2).map(move |x| (*l, x[0], x[1])))
                                        .map(|(l, a, c)| {
                                            let ab = c - a;
                                            let u = ((pv - a).dot(ab) / ab.len2().max(1.0)).clamp(0.0, 1.0);
                                            let q = a + ab * u;
                                            ((q - pv).len(), l, q)
                                        })
                                        .min_by(|x, y| x.0.total_cmp(&y.0))
                                };
                                if let Some((_, l, q)) = near(Some(bl)).or_else(|| near(None)) {
                                    add.push(draw::track_item(net, b.signal_layers[bl], st.wires[bw].w, pv, q, Some(bw)));
                                    if l != bl {
                                        add.push(draw::via_item(net, q, sz, Some(bw)));
                                    }
                                }
                            }
                            if !add.is_empty() {
                                let rep = dr.checker.check_against(&dr.idx, &add, &[]);
                                let vs = dr.viols_of(&add, &rep);
                                let mut wo = drawn.remove(&bw).expect("drawn branch");
                                for it in &add {
                                    let h = dr.commit(it);
                                    match it {
                                        Item::Via(v) => {
                                            via_h.entry(bw).or_default().push(h);
                                            wo.vias.push((Vec2 { x: v.at.x as f64, y: -(v.at.y as f64) }, sz));
                                        }
                                        Item::Track(_) => wo.handles.push(h),
                                    }
                                }
                                // Tracks before vias (the fuse step expects a wire's tracks first).
                                let n_tr = wo.items.iter().filter(|i| matches!(i, Item::Track(_))).count();
                                for it in add {
                                    if matches!(it, Item::Track(_)) {
                                        wo.items.insert(n_tr, it);
                                    } else {
                                        wo.items.push(it);
                                    }
                                }
                                wo.length_nm = wo.items.iter().map(|i| match i {
                                    Item::Track(t) => match t.prim {
                                        crate::geom::quant::TrackPrim::Seg { a, b } => ((b.x - a.x) as f64).hypot((b.y - a.y) as f64),
                                        _ => 0.0,
                                    },
                                    _ => 0.0,
                                }).sum();
                                wo.viol.extend(vs);
                                let ci = conn_of[wire_conn[bw]];
                                if out[ci].cause.is_empty() {
                                    out[ci].cause = "the branch reaches its junction on another layer than its trunk (joined by force)".into();
                                }
                                settle(octi_on, &mut out, &mut necking, ci, bw, &wo, &b, &st);
                                drawn.insert(bw, wo);
                                junction_joins += 1;
                                continue;
                            }
                        }
                    }
                    lost_layer += 1;
                    "drawing: it does not meet the trace it branches from (another layer, or the trace was redrawn elsewhere)"
                };
                for &h in &drawn[&bw].handles {
                    dr.uncommit(h);
                }
                for h in via_h.remove(&bw).unwrap_or_default() {
                    dr.uncommit(h);
                }
                drawn.remove(&bw);
                placed.remove(&bw);
                out[conn_of[wire_conn[bw]]].routed = false;
                fail(&mut out, conn_of[wire_conn[bw]], why);
                gone.insert(bw);
                more = true;
            }
            if !more {
                break;
            }
        }
        let mut junction_note = format!(
            "net trees ([search] net_tree = free): {} branch points on traces; {} branches lost with an undrawn trunk, {} on another layer than their trunk, {} joined to their trunk by force (never drop)",
            js.len(),
            lost_trunk,
            lost_layer,
            junction_joins
        );
        // The report names the ends the wires really have (a junction, or another pad of the
        // tree than the decomposition planned).
        let pad_term = |o: ObsId| -> Option<TermRef> {
            let p = *b.pad_of.get(o)?;
            dec.connections.iter().flat_map(|c| [&c.from, &c.to]).find(|t| matches!(t.kind, TermKind::Pad(q) if q == p)).cloned()
        };
        let (mut at_junction, mut other_pad) = (0usize, 0usize);
        // Per net: connections ending at a branch point, at another pad of the tree, as planned.
        let mut per_net: BTreeMap<i32, [usize; 3]> = BTreeMap::new();
        for (w, wi) in st.wires.iter().enumerate().filter(|(_, x)| x.alive) {
            let Some(&cw) = wire_conn.get(w) else { continue };
            let ci = conn_of[cw];
            let term = |o: ObsId, v: VertId| -> Option<TermRef> {
                if o == NO_OBS {
                    let p = proj.pos[v];
                    Some(TermRef { kind: TermKind::Junction, label: format!("branch point ({:.3}, {:.3})", p.x / MM, -p.y / MM), at: p, layers: b.signal_layers.clone() })
                } else {
                    pad_term(o)
                }
            };
            let (Some(f), Some(t)) = (term(wi.pa, wi.a), term(wi.pb, wi.b)) else { continue };
            let c = &dec.connections[ci];
            let same = (f.label == c.from.label && t.label == c.to.label) || (f.label == c.to.label && t.label == c.from.label);
            let slot = if wi.pa == NO_OBS || wi.pb == NO_OBS { 0 } else if !same { 1 } else { 2 };
            per_net.entry(wi.net).or_default()[slot] += 1;
            if !same {
                if wi.pa == NO_OBS || wi.pb == NO_OBS {
                    at_junction += 1;
                } else {
                    other_pad += 1;
                }
                out[ci].ghost_nm = (proj.pos[wi.b] - proj.pos[wi.a]).len();
                term_over.insert(ci, (f, t));
            }
        }
        junction_note.push_str(&format!("; {at_junction} connections end at a branch point, {other_pad} at another pad of their net's tree than planned"));
        warnings.push(junction_note);
        let name_of = |net: i32| model.nets.iter().find(|n| n.code == net).map_or(String::new(), |n| n.name.clone());
        let rows: Vec<String> = per_net.iter().filter(|(_, c)| c[0] + c[1] > 0).map(|(&n, c)| format!("{} {}/{}/{}", name_of(n), c[0], c[1], c[2])).collect();
        if !rows.is_empty() {
            warnings.push(format!("net trees per net (branch point / another pad of the tree / separate trace to the planned pad): {}", rows.join(", ")));
        }
    }
    // ---- Via reduction (S4, plan P6): measure the vias by cause, then drop the redundant ones ----
    let end_pads = |w: WireId| [st.wires[w].pa, st.wires[w].pb];
    let end_fixed = |w: WireId| {
        let [pa, pb] = end_pads(w);
        [b.smd_layer(pa).is_some(), b.smd_layer(pb).is_some()]
    };
    // Wires at a junction keep their layers (the tee rule holds as drawn).
    let in_tree: BTreeSet<WireId> = if tree_free { tree::junctions(&st).iter().flat_map(|j| [j.0, j.2]).collect() } else { BTreeSet::new() };
    let frozen = |w: WireId| in_tree.contains(&w) || model.pours.iter().any(|z| z.net == st.wires[w].net);
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
            plane_res.iter().filter(|(_, r)| r.as_ref().is_ok_and(|o| o.via.is_some())).count()
        )
    };
    let causes_before = causes(&plans, &drawn);
    tlog("drawing + feedback", t_draw);
    ctl.stage(rnd, rnds, "via reduction and fuse");
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
            settle(octi_on, &mut out, &mut necking, ci, w, &drawn[&w], &b, &st);
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
    // ---- Fuse (D6b): a branch that leaves its junction (or a pad) beside its trunk leaves the
    // trunk's copper where the two part (TopoR FreeStyle auto move of T-points, black-box E8c-e;
    // overlap removal, REPORT section 4.C). Geometric, after everything is drawn: the checker sees
    // all copper. A wire at a pad end fuses only onto a wire not fused there itself (no cycle).
    if tree_free && opts.fuse_branches {
        let jw = reg.price(&Event::JunctionAngle { at: Vec2::default(), deg: 60.0, ws_mm: 1.0 / super::factors::junction_psi(60.0) }, &WireCtx::default());
        let js = tree::junctions(&st);
        let mut fused_at: BTreeSet<(WireId, usize)> = BTreeSet::new();
        // Points of a wire's copper that other copper ends on (a branch's junction, a fused
        // end): a fuse may not take away the stretch they lie on.
        let mut anchors: BTreeMap<WireId, Vec<Vec2>> = BTreeMap::new();
        for &(bw, end, tw, _, v) in &js {
            anchors.entry(tw).or_default().push(dr.end_at.get(&(bw, end)).copied().unwrap_or(proj.pos[v]));
        }
        let near_poly = |p: Vec2, pts: &[Vec2]| pts.windows(2).any(|x| {
            let ab = x[1] - x[0];
            let u = ((p - x[0]).dot(ab) / ab.len2().max(1.0)).clamp(0.0, 1.0);
            (x[0] + ab * u - p).len() < 1_000.0
        });
        let (mut n_fused, mut gain_mm) = (0usize, 0.0f64);
        let ids: Vec<WireId> = drawn.keys().copied().collect();
        let pad_nm = opts.junction_pad_mm * MM;
        for &w in &ids {
            if pairs.width.contains_key(&st.wires[w].net) {
                continue;
            }
            for end in 0..2 {
                let wi = &st.wires[w];
                let (pad, vert) = if end == 0 { (wi.pa, wi.a) } else { (wi.pb, wi.b) };
                let trunks: Vec<WireId> = if pad == NO_OBS {
                    js.iter().filter(|j| j.0 == w && j.1 == end).map(|j| j.2).collect()
                } else {
                    ids.iter()
                        .copied()
                        .filter(|&u| u != w && st.wires[u].net == wi.net && ((st.wires[u].a == vert && !fused_at.contains(&(u, 0))) || (st.wires[u].b == vert && !fused_at.contains(&(u, 1)))))
                        .collect()
                };
                let Some(wo) = drawn.get(&w) else { continue };
                if wo.runs.is_empty() {
                    continue;
                }
                let r = if end == 0 { 0 } else { wo.runs.len() - 1 };
                let (layer, mut run) = wo.runs[r].clone();
                if end == 1 {
                    run.reverse();
                }
                let ws_mm = (wi.w + wi.s) / MM;
                let mut done = false;
                for u in trunks {
                    let Some(uo) = drawn.get(&u) else { continue };
                    let tp: Vec<Vec<Vec2>> = uo.runs.iter().filter(|(l, _)| *l == layer).map(|(_, p)| p.clone()).collect();
                    let tr: Vec<&[Vec2]> = tp.iter().map(|p| p.as_slice()).collect();
                    for (gain, i, mut pts) in draw::fuse_candidates(&run, &tr, ws_mm, jw).into_iter().take(8) {
                        let cut = &run[..=i];
                        if anchors.get(&w).is_some_and(|a| a.iter().any(|&p| (p - run[i]).len() > 1_000.0 && near_poly(p, cut))) {
                            continue;
                        }
                        let q = pts[0];
                        // `junction_pad_mm`: no T that close to a pad of the net.
                        if pad_nm > 0.0 && pad_verts.get(&wi.net).is_some_and(|ps| ps.iter().any(|&(v, _)| (proj.pos[v] - q).len() < pad_nm)) {
                            continue;
                        }
                        if end == 1 {
                            pts.reverse();
                        }
                        let mut wo = drawn.remove(&w).expect("drawn wire");
                        let ok = dr.refit_run(&mut wo, r, pts);
                        if ok {
                            let ci = conn_of[wire_conn[w]];
                            necking.retain(|(c, _)| *c != ci);
                            settle(octi_on, &mut out, &mut necking, ci, w, &wo, &b, &st);
                        }
                        drawn.insert(w, wo);
                        if ok {
                            n_fused += 1;
                            gain_mm += gain;
                            fused_at.insert((w, end));
                            anchors.entry(u).or_default().push(q);
                            if std::env::var("TOPO_FUSEDBG").is_ok() {
                                eprintln!("FUSED round {round} wire {w} net {} end {end} onto {u} at ({:.3}, {:.3}) gain {gain:.2} mm", wi.net, q.x / MM, -q.y / MM);
                            }
                            done = true;
                            break;
                        }
                    }
                    if done {
                        break;
                    }
                }
            }
        }
        warnings.push(format!("fuse after drawing: {n_fused} trace ends moved onto their trunk's copper where the two part, {gain_mm:.1} mm cheaper (length and junction angle)"));
    }
    // M-45: what the steps after the drawing left not at 0/45/90 degrees (a fused branch end)
    // becomes an elbow or Z where the checker accepts it.
    if octi_on {
        let ids: Vec<WireId> = drawn.keys().copied().collect();
        let mut n_pol = 0usize;
        for w in ids {
            let mut wo = drawn.remove(&w).expect("drawn wire");
            let k = dr.octi_polish(&mut wo);
            if k > 0 {
                n_pol += k;
                let ci = conn_of[wire_conn[w]];
                necking.retain(|(c, _)| *c != ci);
                settle(octi_on, &mut out, &mut necking, ci, w, &wo, &b, &st);
            }
            drawn.insert(w, wo);
        }
        warnings.push(format!("octilinear polish after drawing: {n_pol} segments not at 0/45/90 degrees (fused branch ends) redrawn as an elbow or Z"));
        // Bundles (M-45 item 4): bends a wire takes together with a parallel neighbour, and the
        // skew each such bend gives the outer wire (2 d tan(a/2): 0.83 x pitch per 45 degrees).
        let wires = &st.wires;
        let runs: Vec<(usize, i32, usize, &[Vec2])> = drawn.iter().flat_map(|(&w, wo)| wo.runs.iter().map(move |(l, p)| (w, wires[w].net, *l, p.as_slice()))).collect();
        let bb = super::octi::bundle_bends(&runs, &|w| st.wires[w].w + st.wires[w].s);
        if !bb.is_empty() {
            let med = |mut v: Vec<f64>| {
                v.sort_by(f64::total_cmp);
                v[v.len() / 2]
            };
            let d_med = med(bb.iter().map(|x| x.d_nm).collect()) / MM;
            let per45 = med(bb.iter().map(|x| x.skew_nm / (x.deg / 45.0)).collect()) / MM;
            let n45 = bb.iter().filter(|x| (x.deg - 45.0).abs() < 1.0).count();
            warnings.push(format!(
                "bundles (octilinear): {} bends taken together with a parallel neighbour ({n45} of 45 degrees), centre distance median {d_med:.3} mm; skew of the outer wire per bend {:.3} mm total, per 45 degrees median {per45:.3} mm (2 x distance x tan 22.5 = 0.83 x pitch)",
                bb.len(),
                bb.iter().map(|x| x.skew_nm).sum::<f64>() / MM
            ));
        }
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
    // History for the next whole route (M-B2: fading, Dr. CU §IV-C): what came in fades by
    // `history_fade`, this cycle's P4 increments and every edge that was over-full under the
    // drawing's packer are added in full.
    let fade = opts.history_fade.clamp(0.0, 1.0);
    let mut hist_out: Vec<f64> = history.iter().enumerate().map(|(e, &h)| {
        let h0 = hist0.get(e).copied().unwrap_or(0.0);
        let v = fade * h0 + (h - h0);
        if v < HIST_EPS { 0.0 } else { v }
    }).collect();
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
    let forced_stats = dr.stats;
    // Hints for a second round: per cut of a still-failed wire, the room is lowered by the
    // violation depth (what was really missing) at the cut nearest to the violation.
    let mut hint_map: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    // M-B2: the carried stretch history fades; this cycle's goes on the violating stretch only
    // (Dr. CU §IV-C: "the intervals with violations"), each pass weighted by its distance to the
    // nearest violation (1 on it, falling smoothly; at most 1 per pass and cycle).
    let mut own_out: BTreeMap<usize, BTreeMap<usize, f64>> = BTreeMap::new();
    for (&ci, m) in &mem.own {
        let m2: BTreeMap<usize, f64> = m.iter().map(|(&e, &h)| (e, h * fade)).filter(|&(_, h)| h >= HIST_EPS).collect();
        if !m2.is_empty() {
            own_out.insert(ci, m2);
        }
    }
    // Per connection: the places (point, radius) where its copper breaks a rule or its drawing
    // was rejected.
    let mut spots: BTreeMap<WireId, Vec<(Vec2, f64)>> = BTreeMap::new();
    for (w, wo) in &drawn {
        let pitch = st.wires[*w].w + st.wires[*w].s;
        for v in &wo.viol {
            spots.entry(*w).or_default().push((v.at, (0.5 * v.len_nm).max(pitch)));
        }
    }
    for d in &diag {
        let ci = conn_of[wire_conn[d.wire]];
        if !out[ci].clean() && d.passes > 0 && !drawn.contains_key(&d.wire) {
            let pitch = st.wires[d.wire].w + st.wires[d.wire].s;
            spots.entry(d.wire).or_default().push((d.at, (0.5 * (d.to - d.from).len()).max(pitch)));
        }
    }
    for (&w, sp) in &spots {
        let ci = conn_of[wire_conn[w]];
        let wi = &st.wires[w];
        let m = own_out.entry(ci).or_default();
        for &p in &wi.passes {
            let e = st.passes[p].edge;
            let [a, c] = proj.edges[e];
            let (pa, pc) = (proj.pos[a], proj.pos[c]);
            let miss: f64 = sp.iter().map(|&(at, r)| 1.0 - near_weight(seg_dist(at, pa, pc), r)).product();
            let wgt = 1.0 - miss;
            if wgt >= HIST_EPS {
                *m.entry(e).or_insert(0.0) += wgt;
            }
        }
    }
    own_out.retain(|_, m| !m.is_empty());
    for d in &diag {
        let ci = conn_of[wire_conn[d.wire]];
        // A connection drawn with violations (never drop) feeds back like an unrouted one.
        if out[ci].clean() || d.passes == 0 {
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
        if let Some((_, e)) = best {
            let v = hint_map.entry((e, d.layer)).or_insert(0.0);
            *v = v.max(short);
        }
    }
    let new_hints: Hints = hint_map.into_iter().map(|((e, l), sh)| (e, l, (proj.room[e][l] - sh).max(0.0))).collect();
    {
        let cnt = |c: &str| diag.iter().filter(|d| d.class == c).count();
        warnings.push(format!(
            "layers: P3 capacity refinement over-full (edge-layers + vias without a site, on the state before the unlayerable drops) {} before, {} after ({} moves); plan handed to the drawing: {} over-full edge-layers, {} vias without a site ({} wires dropped as unlayerable); drawing packer over-full edge-layers {}; plan vs drawing packer: {} edge-layers planned to fit but over-full when drawn, {} planned over-full but fitting when drawn; drawing yield: {} wires entered drawing, {} drawn ({} lost at the via stage); rejected runs {}: packing {}, repair {}, chord {}, of which infeasible against the static board alone {}; runs accepted by the exact path {}, by the S2a funnel+repair fallback {}, exact path without result {} ({:.1} s in the exact search); feedback tried {}, rescued {}",
            lr.overfull_before,
            lr.overfull_after,
            lr.refine_moves,
            plan_over,
            plan_nosite,
            n_dropped,
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
    // ---- D7: the pairs' wires become two coupled tracks each ----
    if !pairs.coupled.is_empty() || !pairs.single.is_empty() {
        let t_dp = Instant::now();
        let real_checker = Checker::new(model_real, opts);
        warnings.extend(realise_pairs(model_real, opts, &pairs, &mut out, &mut necking, octi_on, &real_checker, &dec, &reg));
        tlog("diff pairs", t_dp);
        if let Some((o, nk)) = pre_snap.as_mut() {
            let _ = realise_pairs(model_real, opts, &pairs, o, nk, octi_on, &real_checker, &dec, &reg);
        }
    }
    let isl = planes::islands(model, &b, &out_vias(&out, &plane_reqs, &dec.connections), &plane_reqs);
    warnings.extend(isl);
    // ---- Assemble the result ----
    let first_layer = signal[0];
    let name_of = |net: i32| model.nets.iter().find(|n| n.code == net).map_or(String::new(), |n| n.name.clone());
    let asm = assemble(model, &dec, &out, &term_over, first_layer, &reg);
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
    let crit_main = criteria(model, &dec, &asm, &out, warn_n(&necking), n_copper, &reg);
    let pre_parts = pre_snap.as_ref().map(|(o, nk)| {
        let a = assemble(model, &dec, o, &term_over, first_layer, &reg);
        let c = criteria(model, &dec, &a, o, warn_n(nk), n_copper, &reg);
        (c, a)
    });
    let Assembled { items, item_conn, reports, nets, length_by_layer, tot_len, tot_vias, violations: viol_list, diff_pairs: dp_list } = asm;
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
        warnings.push(if opts.allow_violations {
            format!("layering errors: {unlayerable_errors} different-net crossings cannot be layered (drawn anyway: violations, never drop)")
        } else {
            format!("layering errors: {unlayerable_errors} different-net crossings cannot be layered (not emitted)")
        });
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
        "capacity: before P3 {} of {} edges break the layer-subset (Hall) check; after P3 {} edge-layers are overfull when packed (a wire that does not fit is drawn on its best geometry as a violation, or with allow_violations = false not emitted)",
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
    if octi_on {
        let (mut n_any, mut mm_any, mut n_acute) = (0usize, 0.0f64, 0usize);
        for (_, ev) in &necking {
            match ev {
                Event::AnyAngle { mm, .. } => {
                    n_any += 1;
                    mm_any += mm;
                }
                Event::AcuteBend { .. } => n_acute += 1,
                _ => {}
            }
        }
        warnings.push(format!(
            "octilinear drawing ([drawing] style): {} runs redrawn at 0/45/90 degrees, {} octilinear runs rejected by the clearance check (taut run kept), {:.1} s; {n_any} stretches not at 0/45/90 degrees ({mm_any:.1} mm, any_angle), {n_acute} acute bends (acute_bend)",
            forced_stats.octi_ok, forced_stats.octi_rejected, forced_stats.octi_s
        ));
    }
    // Never drop (M-B1): a summary and one line per rule the written copper breaks (place, type,
    // cause), at the `violation` factor's report level.
    if opts.allow_violations {
        let mut by: BTreeMap<&str, usize> = BTreeMap::new();
        for v in &viol_list {
            *by.entry(v.kind.as_str()).or_default() += 1;
        }
        let conns: BTreeSet<usize> = viol_list.iter().map(|v| v.conn).collect();
        let mut by_class: BTreeMap<&str, (usize, BTreeSet<usize>)> = BTreeMap::new();
        for v in &viol_list {
            let e = by_class.entry(v.class.as_str()).or_default();
            e.0 += 1;
            e.1.insert(v.conn);
        }
        if !by_class.is_empty() {
            warnings.push(format!(
                "violation classes (M-B2): {} (pad_exit = a neighbour pin of its own terminal: pad exit / necking, M-C)",
                by_class.iter().map(|(k, (n, c))| format!("{k} {n} on {} connections", c.len())).collect::<Vec<_>>().join(", ")
            ));
        }
        warnings.push(format!(
            "never drop ([optimize] allow_violations): {} connections written with {} rule violations ({}); {} wires drawn by force ({} runs: {} on the exact path with the clearance relaxed, {} on a funnel, {} on a normal attempt), {} wires with a via placed by force, {} crossings that cannot be layered kept",
            conns.len(),
            viol_list.len(),
            if by.is_empty() { "none".to_string() } else { by.iter().map(|(k, n)| format!("{k} {n}")).collect::<Vec<_>>().join(", ") },
            n_forced,
            forced_stats.forced_relaxed + forced_stats.forced_funnel + forced_stats.forced_attempt,
            forced_stats.forced_relaxed,
            forced_stats.forced_funnel,
            forced_stats.forced_attempt,
            n_via_forced,
            unlayerable_errors
        ));
        for (ci, ev) in viol_events(model, &dec, &out).into_iter().filter(|(_, e)| matches!(e, Event::Violation { .. })) {
            let c = &dec.connections[ci];
            for f in reg.findings(std::slice::from_ref(&ev), &WireCtx { net: c.net, tier: c.tier }) {
                if f.level >= Report::Warning {
                    warnings.push(format!("{} ({} -> {}): {}", name_of(c.net), c.from.label, c.to.label, f.text));
                }
            }
        }
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
        violations: viol_list,
        diff_pairs: dp_list,
    };
    let pre = pre_parts.map(|(c, a)| (c, with_parts(&result, a)));
    let vias_out: BTreeMap<usize, usize> = plans.iter().map(|(&w, pl)| (conn_of[wire_conn[w]], pl.vias.len())).collect();
    (RoundOut { result, crit: crit_main, pre }, new_hints, Memory { edge: hist_out, own: own_out, vias: vias_out })
}

/// D7: realise every coupled pair connection of `out`: the lead's drawn centre line becomes the P
/// and N tracks ([`diffpair::realise`]), checked against the real model and all other copper; the N
/// connection gets its copper, both get their violations, the lead its statistics and the pair
/// factors' events (also pushed to `necking`, the report's factor findings). The lead's own
/// necking events (the envelope narrowed at its pads: the fan-out zone) are dropped. Returns the
/// report lines. O(pair connections x zip candidates x checker queries).
#[allow(clippy::too_many_arguments)]
fn realise_pairs(model: &BoardModel, opts: &Options, pairs: &Pairs, out: &mut [Outcome], necking: &mut Vec<(usize, Event)>, octi: bool, checker: &Checker, dec: &crate::decompose::Decomposition, reg: &Registry) -> Vec<String> {
    let mut lines = Vec::new();
    let mut idx = crate::check::ItemIndex::new(checker.cell());
    for (ci, o) in out.iter().enumerate() {
        if o.routed && !pairs.lead.contains_key(&ci) && !pairs.carried.contains_key(&ci) {
            for it in &o.items {
                idx.insert(it);
            }
        }
    }
    necking.retain(|(ci, ev)| !(pairs.lead.contains_key(ci) && matches!(ev, Event::Necking { .. })));
    let ctx = diffpair::Ctx { model, opts, checker, octi };
    let label = |p: usize| model.pads.get(p).map_or("?".to_string(), |x| format!("{}.{}", x.ref_, x.number));
    for (&cp_ci, &(k, cn)) in &pairs.lead {
        let cp = &pairs.coupled[k];
        let r = &cp.rule;
        if !out[cp_ci].routed {
            let why = out[cp_ci].reason.clone();
            fail(out, cn, format!("diff pair: its {} connection is not routed ({why})", r.p_name));
            continue;
        }
        let (mut pa, mut pb) = out[cp_ci].ends;
        let runs = out[cp_ci].runs.clone();
        // The runs go from the wire's first end; the connection report keeps its own order.
        if let (Some(f), Some(pbp)) = (runs.first().and_then(|r| r.1.first()), model.pads.get(pb)) {
            if model.pads.get(pa).is_some_and(|x| (x.center.to_vec2() - *f).len() > (pbp.center.to_vec2() - *f).len()) {
                std::mem::swap(&mut pa, &mut pb);
            }
        }
        let (Some(&na), Some(&nb)) = (cp.partner.get(&pa), cp.partner.get(&pb)) else {
            fail(out, cn, format!("diff pair: the {} wire does not end at the pair's pads", r.p_name));
            out[cp_ci].routed = false;
            out[cp_ci].reason = "diff pair: the pair's wire does not end at its pads".into();
            out[cp_ci].items.clear();
            continue;
        };
        let pt = |p: usize| model.pads[p].center.to_vec2();
        let (ea, eb) = (diffpair::Ends { p: pt(pa), n: pt(na) }, diffpair::Ends { p: pt(pb), n: pt(nb) });
        // Centred on the pad pairs first; the centre line as drawn when that breaks more rules.
        let score = |rz: &diffpair::Realised| {
            let (a, x) = diffpair::viol_count(&ctx, &idx, &rz.p_items, None);
            let (b, y) = diffpair::viol_count(&ctx, &idx, &rz.n_items, Some(&diffpair::index_of(&ctx, &rz.p_items)));
            (a + b, x + y)
        };
        let rc = diffpair::realise(&ctx, cp, &runs, ea, eb, &idx, true);
        let sc = score(&rc);
        let rz = if sc.0 == 0 {
            rc
        } else {
            let ru = diffpair::realise(&ctx, cp, &runs, ea, eb, &idx, false);
            let su = score(&ru);
            if su.0 < sc.0 || (su.0 == sc.0 && su.1 < sc.1) { ru } else { rc }
        };
        let vp = diffpair::check(&ctx, &idx, &rz.p_items);
        for it in &rz.p_items {
            idx.insert(it);
        }
        let vn = diffpair::check(&ctx, &idx, &rz.n_items);
        for it in &rz.n_items {
            idx.insert(it);
        }
        let st = &rz.stats;
        let layers = out[cp_ci].layers.clone();
        {
            let o = &mut out[cp_ci];
            o.items = rz.p_items.clone();
            o.vias = rz.p_vias.clone();
            o.length_nm = st.length_p;
            o.viol = vp;
            o.neck_nm = 0.0;
            o.neck_n = 0;
            o.neck_w = None;
        }
        {
            let ghost = out[cn].ghost_nm;
            let o = &mut out[cn];
            o.routed = true;
            o.reason.clear();
            o.items = rz.n_items.clone();
            o.vias = rz.n_vias.clone();
            o.length_nm = st.length_n;
            o.layers = layers;
            o.viol = vn;
            o.ghost_nm = ghost;
        }
        // Pair factors (graded) and the report.
        let len = st.length_p.max(st.length_n);
        let unc = (len - st.coupled).max(0.0);
        let mut evs = vec![
            Event::DpUncoupled { mm: unc / MM, max_mm: r.max_uncoupled as f64 / MM },
            Event::DpSkew { mm: st.skew / MM, max_mm: r.max_skew as f64 / MM },
        ];
        if st.gap_dev_len > 0.0 {
            evs.push(Event::DpGapDeviation { mm: st.gap_dev_len / MM, dev_mm: st.gap_dev_max / MM, tol_mm: r.tolerance as f64 / MM });
        }
        if st.polarity_cross {
            evs.push(Event::DpPolarityCross { at: pt(pb) });
        }
        for ev in &evs {
            necking.push((cp_ci, ev.clone()));
        }
        out[cp_ci].dp_events = evs;
        out[cp_ci].dp = Some(diffpair::ConnStats { conn_n: cn, ..st.clone() });
        let c = &dec.connections[cp_ci];
        let _ = (c, reg);
        lines.push(format!(
            "diff pair {} / {} {} -> {}: length P {:.3} mm, N {:.3} mm, skew {:+.3} mm (bend estimate {:+.3}), coupled {:.1} % ({:.3} mm) at gap {:.3} mm{}, uncoupled at {} P {:.3} / N {:.3} mm, at {} P {:.3} / N {:.3} mm{}, vias {} / {}{}",
            r.p_name,
            r.n_name,
            label(pa),
            label(pb),
            st.length_p / MM,
            st.length_n / MM,
            st.skew / MM,
            st.skew_estimate / MM,
            st.coupled_pct,
            st.coupled / MM,
            r.gap as f64 / MM,
            if st.gap_dev_len > 0.0 { format!(" ({:.3} mm parallel at a gap up to {:.3} mm off)", st.gap_dev_len / MM, st.gap_dev_max / MM) } else { " (gap held)".into() },
            label(pa),
            st.fan_start[0] / MM,
            st.fan_start[1] / MM,
            label(pb),
            st.fan_end[0] / MM,
            st.fan_end[1] / MM,
            if st.fan_vias > 0.0 { format!(", at via pairs {:.3} mm", st.fan_vias / MM) } else { String::new() },
            st.vias_p,
            st.vias_n,
            if st.polarity_cross { "; POLARITY: P and N swap sides between the ends" } else { "" },
        ));
        // KiCad's own wording, only where a matching custom rule exists (doc section 6.4).
        if r.max_uncoupled > 0 && unc > r.max_uncoupled as f64 {
            lines.push(format!("Differential uncoupled length too long ({} maximum uncoupled length {:.4} mm; actual {:.4} mm): {} / {} {} -> {}", r.rule_uncoupled, r.max_uncoupled as f64 / MM, unc / MM, r.p_name, r.n_name, label(pa), label(pb)));
        }
        if r.max_skew > 0 && st.skew.abs() > r.max_skew as f64 {
            lines.push(format!("Skew between tracks out of range (rule '{}' max skew {:.4} mm; actual {:.4} mm): {} / {} {} -> {}", r.rule_skew, r.max_skew as f64 / MM, st.skew.abs() / MM, r.p_name, r.n_name, label(pa), label(pb)));
        }
        if !r.rule_gap.is_empty() && st.gap_dev_len > 0.0 {
            let actual = r.gap as f64 + st.gap_dev_max;
            if r.gap_max > 0 && actual > r.gap_max as f64 {
                lines.push(format!("Differential pair gap out of range ({} maximum gap {:.4} mm; actual {:.4} mm): {} / {}", r.rule_gap, r.gap_max as f64 / MM, actual / MM, r.p_name, r.n_name));
            }
        }
    }
    // Connection pairs routed as two single tracks (P and N swap sides): their vias are unpaired.
    for &(ip, jn, ri) in &pairs.single {
        let r = &model.rules.diff_pairs[ri];
        let (vp, vn) = (out[ip].vias.len(), out[jn].vias.len());
        if out[ip].routed && out[jn].routed {
            lines.push(format!(
                "diff pair {} / {} {} -> {}: routed as two single tracks (P and N swap sides between the ends), length P {:.3} mm, N {:.3} mm, skew {:+.3} mm, vias {vp} / {vn}",
                r.p_name, r.n_name, dec.connections[ip].from.label, dec.connections[ip].to.label, out[ip].length_nm / MM, out[jn].length_nm / MM, (out[ip].length_nm - out[jn].length_nm) / MM
            ));
            let ev = Event::DpViaUnpaired { count: vp.abs_diff(vn) as u32 };
            if vp != vn {
                necking.push((ip, ev.clone()));
                out[ip].dp_events.push(ev);
            }
        }
    }
    lines
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
        violations: a.violations,
        diff_pairs: a.diff_pairs,
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
    violations: Vec<crate::route::ViolationReport>,
    diff_pairs: Vec<crate::route::DiffPairReport>,
}

/// Assemble the result parts of `out`; `term_over` replaces the terminals of a connection the
/// attachment move connected to another pad pair. O(connections + items).
fn assemble(model: &BoardModel, dec: &crate::decompose::Decomposition, out: &[Outcome], term_over: &BTreeMap<usize, (TermRef, TermRef)>, first_layer: u32, reg: &Registry) -> Assembled {
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
            r.violations = o.viol.len();
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
    let mut violations = Vec::new();
    for (ci, ev) in viol_events(model, dec, out) {
        let Event::Violation { kind, at, layer, depth_nm, len_nm, against, cause } = &ev else { continue };
        let c = &dec.connections[ci];
        let r = &reports[ci];
        violations.push(crate::route::ViolationReport {
            conn: ci,
            net: c.net,
            name: name_of(c.net),
            from: r.from.label.clone(),
            to: r.to.label.clone(),
            kind: kind.clone(),
            layer: layer.clone(),
            at: kicad(*at),
            depth_nm: depth_nm.round() as i64,
            length_nm: len_nm.round() as i64,
            against: against.clone(),
            cause: cause.clone(),
            price_mm: (reg.price(&ev, &WireCtx { net: c.net, tier: c.tier }) * 10.0).round() / 10.0,
            class: crate::route::violation_class(against, cause, &r.from.label, &r.to.label).into(),
        });
    }
    let mut diff_pairs = Vec::new();
    for dp in &model.rules.diff_pairs {
        for c in dec.connections.iter().filter(|c| c.net == dp.p) {
            let Some(st) = &out[c.id].dp else { continue };
            let cn = st.conn_n;
            let r = |x: f64| x.round() as i64;
            diff_pairs.push(crate::route::DiffPairReport {
                conn_p: c.id,
                conn_n: cn,
                p_name: dp.p_name.clone(),
                n_name: dp.n_name.clone(),
                from: reports[c.id].from.label.clone(),
                to: reports[c.id].to.label.clone(),
                length_p_nm: r(st.length_p),
                length_n_nm: r(st.length_n),
                skew_nm: r(st.skew),
                skew_estimate_nm: r(st.skew_estimate),
                coupled_nm: r(st.coupled),
                coupled_pct: (st.coupled_pct * 10.0).round() / 10.0,
                gap_nm: dp.gap,
                gap_held: st.gap_dev_len <= 0.0,
                gap_dev_nm: r(st.gap_dev_max),
                uncoupled_start_nm: [r(st.fan_start[0]), r(st.fan_start[1])],
                uncoupled_end_nm: [r(st.fan_end[0]), r(st.fan_end[1])],
                uncoupled_vias_nm: r(st.fan_vias),
                vias_p: st.vias_p,
                vias_n: st.vias_n,
                polarity_cross: st.polarity_cross,
            });
        }
    }
    Assembled { items, item_conn, reports, nets, length_by_layer, tot_len, tot_vias, violations, diff_pairs }
}

/// The `violation` factor's events of the outcomes `out`, per connection id: one per rule the
/// written copper breaks (never drop), and one `Open` per connection without copper (the diff
/// pairs the core skips by design are no violation: a measurement gap, reported apart).
fn viol_events(model: &BoardModel, dec: &crate::decompose::Decomposition, out: &[Outcome]) -> Vec<(usize, Event)> {
    let layer_name = |id: u32| if id == u32::MAX { "all layers".to_string() } else { model.layers.iter().find(|l| l.id == id).map_or(id.to_string(), |l| l.name.clone()) };
    let mut evs = Vec::new();
    for c in &dec.connections {
        let o = &out[c.id];
        if o.routed {
            let cause = if o.cause.is_empty() { "no legal geometry in the drawing".to_string() } else { o.cause.clone() };
            for v in &o.viol {
                evs.push((
                    c.id,
                    Event::Violation {
                        kind: v.kind.to_string(),
                        at: v.at,
                        layer: layer_name(v.layer),
                        depth_nm: v.depth_nm,
                        len_nm: v.len_nm,
                        against: v.against.clone(),
                        cause: cause.clone(),
                    },
                ));
            }
        } else if o.reason != DIFF_PAIR {
            evs.push((c.id, Event::Open { pitch_nm: (c.width + c.clearance) as f64, len_nm: o.ghost_nm, cause: o.reason.clone() }));
        }
    }
    evs
}

/// The archive criteria of the assembled result `a` of outcomes `out` (`warnings` = warning-level
/// factor findings). O(items).
fn criteria(model: &BoardModel, dec: &crate::decompose::Decomposition, a: &Assembled, out: &[Outcome], warnings: usize, n_layers: usize, reg: &Registry) -> Criteria {
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
    let open = a.reports.len() - routed;
    let violations: usize = out.iter().filter(|o| o.routed).map(|o| o.viol.len()).sum();
    // Connections not complete: without copper, or with violations (one error per connection,
    // however many rules it breaks: an open connection never counts less than a drawn one).
    let violating = out.iter().filter(|o| o.routed && !o.viol.is_empty()).count();
    let penalty = viol_events(model, dec, out).iter().map(|(ci, ev)| reg.price(ev, &WireCtx { net: dec.connections[*ci].net, tier: dec.connections[*ci].tier })).sum::<f64>()
        + dec.connections.iter().flat_map(|c| out[c.id].dp_events.iter().map(move |ev| (c, ev))).map(|(c, ev)| reg.price(ev, &WireCtx { net: c.net, tier: c.tier })).sum::<f64>();
    Criteria {
        errors: open + violating,
        open,
        violations,
        penalty,
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
    fn diff_pair_halves_from_the_model() {
        // D7: the pairs come from the model (the plugin's KiCad rule), not from a second rule here.
        let mut m = BoardModel::default();
        for (i, n) in ["USB_D+", "USB_D-", "CLK_P", "CLK_N", "VCC", "ONLY_P"].iter().enumerate() {
            m.nets.push(serde_json::from_value(serde_json::json!({"code": i as i32 + 1, "name": n})).expect("net"));
        }
        assert!(diff_pair_nets(&m).is_empty());
        m.rules.diff_pairs.push(crate::model::DiffPairRule { p: 1, n: 2, ..Default::default() });
        m.rules.diff_pairs.push(crate::model::DiffPairRule { p: 3, n: 4, ..Default::default() });
        assert_eq!(diff_pair_nets(&m).into_iter().collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    }
}
