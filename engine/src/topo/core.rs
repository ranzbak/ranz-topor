//! The `route()` entry of the topological core (two or more signal layers): plane stubs placed once
//! on the static board, then `[optimize] runs` seeded runs of whole-route cycles (`route_round`):
//! decomposition (diff pairs planned as one wire) -> projection (`board`) -> P2 initial routing
//! (`init`, net trees, widest first) -> P3 layer assignment (`layers`, Polubasov, capacity
//! refinement) -> crossing rules and P4 negotiated rip-up and reroute (optional moves) -> drawing
//! (`draw`: vias, then wires; feedback redraw; never drop) -> via reduction, fuse, octilinear polish
//! -> diff pair realisation -> `RouteResult`. The decomposition and the projection depend only on
//! the fixed items: built once per call ([`Fixed`], shopping list 4.2) and read by every cycle.
//! Each cycle's result is pour-stitched and offered to the run's archive; the drawing feeds back
//! into the next cycle. `[run] stop_after = "plan"` ends a run after the plan of its first cycle.
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
//!   unsupported terminal, a differential pair with `[diffpair] enabled = false` ("not in the rough
//!   version"), time limit.
//!
//! Boards with fewer than two signal layers report every connection failed with a reason.
//! Deterministic: no randomness besides the seeded P3 local search and the seeded fluctuation of
//! the factor weights per cycle, Vec + index ids throughout.
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
use crate::control::{cancelled, CutStep, RunControl};
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

/// forced-draw (docs/dev/design/forced-draw.md): the never-drop pass's octilinear count, so a
/// forced run's free-angle stretch is never silent: `None` when the octilinear style is off or
/// nothing was drawn by force.
fn forced_free_line(octi_on: bool, s: &draw::DrawStats) -> Option<String> {
    let runs = s.forced_relaxed + s.forced_funnel + s.forced_attempt;
    (octi_on && runs > 0).then(|| {
        format!(
            "never drop, octilinear: {} of {runs} forced runs drawn in their octilinear form; {} forced runs keep {} free-angle stretches ({:.1} mm), where the octilinear form costs more violation price or there is none (a layer change for them is the plan's decision)",
            s.forced_octi, s.forced_free_runs, s.forced_free_stretches, s.forced_free_mm
        )
    })
}

/// neck-regrow r6 ([`draw::Drawer`]'s `monotone_necks`): the drawings whose end necks had to be
/// joined into one neck over the whole wire (never wider again after a neck), so the join is not
/// silent; `None` when none. Counts every drawing of the round (trial redraws too).
fn mono_line(s: &draw::DrawStats) -> Option<String> {
    (s.mono_wires > 0).then(|| {
        let stubs = if s.mono_stubs > 0 { format!("; {} narrowed pad-end stubs moved out of KiCad's in-pad delete (the trace keeps its pad-centre end)", s.mono_stubs) } else { String::new() };
        format!(
            "neck profile (never wider again after a neck): {} wire drawings that would widen again after a neck (necks at both ends, or a neck in mid-run) were narrowed on to one end, one neck stretch per wire ({:.2} mm narrowed){stubs}",
            s.mono_wires, s.mono_mm
        )
    })
}

/// first-draw-45 r9 forensics (`TOPO_FREEDBG`, copper-neutral): the free-angle segments (not at
/// 0/45/90 degrees) of every drawn wire after the step `stage` of the drawing, one stderr line
/// each (mm, KiCad frame), so a segment of the final copper can be traced to the step that wrote
/// it (the r9 forensics script). Read only.
fn free_snap(stage: &str, drawn: &BTreeMap<WireId, WireOut>, st: &State) {
    if std::env::var_os("TOPO_FREEDBG").is_none() {
        return;
    }
    eprintln!("FREESTAGE {stage}");
    for (w, wo) in drawn {
        for (layer, pts) in &wo.runs {
            for s in pts.windows(2) {
                if (s[1] - s[0]).len() >= 1_000.0 && super::octi::octi_dir(s[1] - s[0]).is_none() {
                    eprintln!("FREESNAP {stage} w{w} net {} L{layer} {:.6} {:.6} {:.6} {:.6}", st.wires[*w].net, s[0].x / MM, -s[0].y / MM, s[1].x / MM, -s[1].y / MM);
                }
            }
        }
    }
}

/// draw-absorb A1 r2: the grade of a neck from width `w` to `to` over `len` (nm) on a supply-width
/// net: (the share of its width left, the severity = width share lost x length in mm). Graded, no
/// cutoff: a deeper or longer neck is more severe. O(1).
pub(crate) fn a1_supply_grade(w: f64, to: f64, len: f64) -> (f64, f64) {
    let left = if w > 0.0 { (to / w).clamp(0.0, 1.0) } else { 1.0 };
    (left, (1.0 - left) * len / MM)
}

/// pick-necks: a neck to `to` (nm) over `len` (nm) of a wire of full width `w`, as graded mm:
/// `len x (1 - to / w)`, 0 for a stretch not narrower. Graded, no cutoff. O(1).
pub(crate) fn neck_grade(w: f64, to: f64, len: f64) -> f64 {
    if w <= 0.0 {
        return 0.0;
    }
    len / MM * (1.0 - (to / w).clamp(0.0, 1.0))
}

/// Record the drawn wire `wo` as the outcome of connection `ci`.
/// neck-regrow: a neck to `to` (nm) over `neck_len` of a wire of nominal width `w` and drawn
/// length `wire_len` is a whole-wire neck of a supply net when it spans the wire (within 1 um) and
/// the wire is wider than the board's default width `dflt_w` (lead 2026-11-26: such necks get a
/// warning of their own; POWER-NECK-MONOTONE will care about the direction from the source).
pub(crate) fn whole_supply_neck(w: f64, to: f64, neck_len: f64, wire_len: f64, dflt_w: f64) -> bool {
    to < w - 0.5 && neck_len >= wire_len - 1_000.0 && w > dflt_w + 0.5
}

/// `octi`: the octilinear style is on; every stretch not at 0/45/90 degrees and every acute bend
/// of the drawn runs is an event (`any_angle`, `acute_bend`, M-45) next to the necked stretches.
#[allow(clippy::too_many_arguments)]
fn settle(octi: bool, out: &mut [Outcome], necking: &mut Vec<(usize, Event)>, ci: usize, w: WireId, wo: &WireOut, b: &board::Board, st: &State, dflt_w: f64) {
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
    o.neck_graded_mm = wo.necks.iter().map(|n| neck_grade(st.wires[w].w, n.1, n.2)).sum();
    o.neck_w = wo.necks.iter().map(|n| n.1).fold(None, |a: Option<f64>, x| Some(a.map_or(x, |y| y.min(x))));
    o.viol = wo.viol.clone();
    o.runs = wo.runs.iter().map(|(l, p)| (b.signal_layers[*l], p.clone())).collect();
    let pad = |ob: ObsId| if ob == super::state::NO_OBS { usize::MAX } else { b.pad_of.get(ob).copied().unwrap_or(usize::MAX) };
    o.ends = (pad(st.wires[w].pa), pad(st.wires[w].pb));
    for n in &wo.necks {
        // neck-regrow: a neck over the whole wire (both ends needed it, joined) of a wire wider
        // than the board's default width (a supply net) is named apart in the warning. r6: a
        // neck over several runs (vias) is one stretch (`Drawer::monotone_necks`).
        let supply = whole_supply_neck(st.wires[w].w, n.1, n.2, wo.length_nm, dflt_w);
        necking.push((ci, Event::Necking { wire: w, from_nm: st.wires[w].w, to_nm: n.1, mm: n.2 / MM, whole_supply: supply }));
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

/// loop-local (docs/dev/design/loop-local.md): the TopoR loop's deterministic work counters,
/// summed over the run and read per cycle ([`super::optimise::CycleStats`]).
#[derive(Default)]
struct LoopCtr {
    layer_global: std::cell::Cell<usize>,
    measure_global: std::cell::Cell<usize>,
    layer_local: std::cell::Cell<usize>,
    local_free: std::cell::Cell<usize>,
    local_model: std::cell::Cell<usize>,
    /// Wires a local move changed besides its own (trunk turns, wires that died).
    local_changed: std::cell::Cell<usize>,
    /// `TOPO_LOCALCHECK` (debug): partner links of the local layerings that point nowhere.
    local_faults: std::cell::Cell<usize>,
    /// `TOPO_LOCALCHECK`: the held-held crossings the local layerings leave unpriced
    /// ([`layers::unpriced_crossings`]; 0 when the judge is exact).
    local_unpriced: std::cell::Cell<usize>,
    /// `TOPO_LOCALCHECK`: the held events whose crossing partner the move killed
    /// ([`layers::stale_held_events`]; 0 when the judge is exact).
    local_stale: std::cell::Cell<usize>,
    /// site-price-cost: the candidate evaluations of every layering's capacity refinement
    /// (`RefineStats::evals`: equal for any lane count), and the deterministic cost counter of the
    /// loop's steps ([`super::escape::cost`]; `[optimize] escape_budget = "cost"`).
    evals: std::cell::Cell<usize>,
    cost: std::cell::Cell<f64>,
}

impl LoopCtr {
    fn add(c: &std::cell::Cell<usize>, n: usize) {
        c.set(c.get() + n);
    }
    /// site-price-cost: `x` more on the cost counter.
    fn spend(&self, x: f64) {
        self.cost.set(self.cost.get() + x);
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
    /// pick-necks: graded necked length, mm: sum of `len x (1 - w_neck / w_full)` over the necks.
    neck_graded_mm: f64,
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
/// `post` runs on the result before its run time and limit report are taken (the caller's
/// finishing of the copper, `route::route_with`: ignored pads, edge trims, the Cleanup-safe tidy),
/// so a wait of the drawing counts to the copper handed back.
pub fn route_topo(model: &BoardModel, opts: &Options, ctl: &RunControl, post: &mut dyn FnMut(&mut RouteResult)) -> RouteResult {
    super::prof::begin();
    let t0 = Instant::now();
    // tw4 T3 (user 2026-11-18): with `[search] dangling_lead_choice` the `dangling_turn` factor
    // defaults to `DANGLING_TURN_WITH_CHOICE` (a weight the user sets wins).
    let with_turn;
    let opts = match super::factors::with_lead_choice_turn(opts) {
        Some(o) => {
            with_turn = o;
            &with_turn
        }
        None => opts,
    };
    let n = opts.runs.max(1);
    let obj = Objective::from_options(&opts.objective, opts.mm_per_via, &opts.pick).with_best_penalty(opts.best_penalty);
    ctl.update("setup", 0, 0, 0, 0, 0.0);
    ctl.with_progress(|p| {
        p.runs = n as u32;
        p.rounds = opts.topo_rounds.max(1) as u32;
    });
    // The soft step the wait counts (TIME_LIMIT_WAIT): with loop = "cycles" the whole-route cycle.
    ctl.limit.set_whole_cycle(opts.opt_loop != "topor");
    ctl.stage(1, opts.topo_rounds.max(1) as u32, "plane stubs (pads of plane nets onto their pour)");
    ctl.limit.init(opts.wall_clock_s); // live limit (control::TimeLimit): the caller may change it during the run
    ctl.limit.set_hard(opts.timeout_mode == "hard");
    // F3 `[run] timeout_overrun_s` (the engine's own wall-clock limit; the plugin's CPU clock
    // hardens the limit itself, RouteJob.harden): past the limit by this much, the soft limit acts
    // as the hard one. "auto" (the engine's default, user 2026-11-13) = `wall_clock_s`.
    ctl.limit.set_overrun_cap(if opts.timeout_mode == "hard" { 0.0 } else { opts.overrun_cap_s() });
    // `[run] limit_grace_s`: how long the TopoR loop step may run on past the soft limit;
    // `[run] drawing_grace_s`: how long the drawing's optional passes may (soft limit only).
    ctl.limit.set_grace(opts.limit_grace_s);
    ctl.limit.set_draw_grace(if opts.timeout_mode == "hard" { -1.0 } else { opts.drawing_grace_s });
    // pour-pad-join (fail fast): a board whose every connection in scope is already made (KiCad's
    // connectivity: pads, tracks, vias, the pours' fill) has nothing to route. End here, before the
    // plane stubs and the projection (minutes on a large board) and before any TopoR loop: the
    // dialog's loop has no cycle cap and no stop at "Routes stable", so an empty plan spun through
    // empty cycles until the time limit (40 075 cycles per run in 20 s on a complete small board).
    // The plane stubs come from the decomposition's connections (none here); the pours' stitching
    // (classes 2-4: dropped islands, thin necks, `[planes] stitch_area`) works on the pours as
    // given, so it runs here as it runs on the picked result of a full route (same call, no
    // routed copper): with its vias the call hands them back (stop reason `stitch_only`), the
    // NOTHING_TO_ROUTE end is only for a call that adds nothing at all.
    let routable: Vec<u32> = model.layers.iter().filter(|l| l.routable).map(|l| l.id).collect();
    if crate::decompose::decompose(model, opts, &routable).connections.is_empty() {
        let mut r = unrouted(model, opts, NOTHING_TO_ROUTE);
        r.stop_reason = "nothing_to_route".into();
        r.converged = true;
        if opts.stitch_islands {
            stitch_regions_on(model, opts, &mut r, &|| ctl.is_cancelled());
        }
        if !r.items.is_empty() {
            r.warnings[0] = STITCH_ONLY.into();
            r.stop_reason = "stitch_only".into();
        }
        post(&mut r);
        r.runtime_s = t0.elapsed().as_secs_f64();
        r.limit = limit_report(ctl, opts, &mut r);
        return r;
    }
    let t_stubs = Instant::now();
    let stubs = place_plane_stubs(model, opts, ctl);
    if std::env::var_os("TOPO_TIMES").is_some() {
        eprintln!("[topo call] plane stubs: {:.3} s", t_stubs.elapsed().as_secs_f64());
    }
    ctl.stage(1, opts.topo_rounds.max(1) as u32, "projection of the board");
    super::prof::mark("stubs");
    let t_fx = Instant::now();
    // Cancel / OK during the projection (cancel-fix; the cut room of `[capacity] model = "cuts"`
    // took 150-200 s on qmtech and did not look at Cancel): the sheets stop at once and no run begins.
    let fx = crate::control::with_cancel_of(ctl, || Fixed::build(model, opts, &stubs));
    if std::env::var_os("TOPO_TIMES").is_some() {
        eprintln!("[topo call] projection built once (decomposition, triangulation, cut room): {:.3} s", t_fx.elapsed().as_secs_f64());
    }
    super::prof::mark("projection built once");
    if ctl.is_cancelled() && opts.capacity_model == "cuts" {
        // M2.TL (fail fast): no plan kept yet, so nothing to draw, said loudly.
        let why = format!("{NOTHING_TO_DRAW}: Cancel / OK came during the projection of the board (the cut room of [capacity] model = \"cuts\"), before any run began, so no plan was kept and no copper is written");
        let mut r = nothing_to_draw(model, opts, ctl, &why, t0, &mut *post);
        r.converged = false;
        return r;
    }
    let mut coord = Coord::new(ctl, n, obj, opts.draw_candidates);
    if opts.opt_loop == "topor" {
        // stable_gain_pct = 0: no gain is below 0 %, so the table is never stable (as the run's meter)
        let window = if opts.stable_gain_pct <= 0.0 { usize::MAX } else { opts.loop_stable_window.max(1) };
        coord = coord.with_table(super::table::SharedTable::new(n, opts.variants, obj, window).with_lag(opts.table_lag));
    }
    let (mut outs, skipped) = run_all(model, opts, ctl, &coord, &stubs, &fx, t0, n);
    if ctl.is_discarded() {
        return discarded(model, opts, t0);
    }
    if outs.is_empty() {
        // M2.TL (fail fast): Cancel / OK before any run kept a plan: nothing to draw, said loudly
        // (never a quiet empty result). Without a Cancel this cannot happen (the first run to begin
        // always keeps its initial plan); never a panic.
        let why = if ctl.is_cancelled() {
            format!("{NOTHING_TO_DRAW}: Cancel / OK came before the initial routing of any run had ended, so no plan was kept and no copper is written ({})", skipped.iter().map(|(k, w)| format!("run {}: {w}", k + 1)).collect::<Vec<_>>().join("; "))
        } else {
            format!("{NOTHING_TO_DRAW}: no run of the call kept a plan ({})", skipped.iter().map(|(k, w)| format!("run {}: {w}", k + 1)).collect::<Vec<_>>().join("; "))
        };
        return nothing_to_draw(model, opts, ctl, &why, t0, &mut *post);
    }
    // The best run: fewest errors, then the objective, then shrinkages; ties: the lower run.
    let mut pick = 0;
    for k in 1..outs.len() {
        if obj.better(&outs[k].crit, &outs[pick].crit) {
            pick = k;
        }
    }
    // TW3: the four bests of the one variant table, drawn: compared at 100 % drawn by the same
    // roles ([`super::archive::bests`] on the drawn criteria); `[optimize] pick` names the one that
    // goes to KiCad ("best" = the overall best, the pick above).
    let (bests_json, bests_line, role_result) = four_bests(&outs, &obj, &opts.pick);
    if let Some((k, _)) = role_result.as_ref() {
        pick = *k;
    }
    let lines: Vec<String> = outs.iter().map(|o| o.line.clone()).collect();
    let cycle_lines: Vec<String> = outs.iter().map(|o| o.cycles_line.clone()).collect();
    // OK vs Cancel = what the first stop request was, at the press ([`RunControl::request_stop`]):
    // not whether the runs in `outs` (since draw once: the drawn run only) are stable at the end.
    let ok_stop = ctl.ok_at_stable();
    // "stable" as a stop reason = every run ENDED because its routes were stable; a dialog run that
    // went on past stable to the cycle cap ends at the cap (stop_reason "round cap").
    let stable_stop_all = outs.iter().all(|o| o.stable_stop);
    // TW3 D2 (R1 verify r2): read before the pick is taken out of `outs` below
    let relay_stop_any = outs.iter().any(|o| o.relay_stop);
    let cancel = ctl.is_cancelled();
    let time_hit = outs.iter().any(|o| o.time_hit);
    // verify-sl3 F1: the limit (acting as hard) cut the drawing of the picked run's plan: its board
    // is drawn only in part, so the call is never "stable" then, even after an OK at Routes stable
    // (the user took the stable plan; the limit, not the OK, ended what is drawn of it).
    let (draw_cut, drawn_stable) = (outs[pick].draw_cut, outs[pick].stable);
    let ok_kept = ok_stop && !draw_cut;
    // time-limit-fix (user 2026-11-22: "Stopped: timeout_wall ... 2357.5 s ... limit 3600 s wall"):
    // the runs' TopoR loops ended at their own time share ([optimize] loop_time_share) and nothing
    // else did: the time limit was not what stopped the run, so the stop reason and the text say
    // the share (before, the share's stop was reported as the limit's "timeout").
    let share_stop = time_hit && !cancel && !draw_cut && outs.iter().filter(|o| o.time_hit).all(|o| o.share_stop);
    let share_end_s = outs.iter().filter(|o| o.share_stop).map(|o| o.loop_end_s).fold(0.0f64, f64::max);
    // the picked run's cycles (TopoR loop cycles, or whole-route cycles with loop = "cycles")
    let picked_cycles = outs[pick].loop_cycles;
    // Every cycle's variants were stitched before they were judged (pour pieces still cut off
    // count as errors), so the picked board is the one the progress showed.
    let pick_k = outs[pick].k;
    let mut best = match role_result {
        Some((_, r)) => r,
        None => outs.swap_remove(pick).result,
    };
    best.warnings.extend(cycle_lines);
    if let Some(l) = bests_line {
        best.warnings.push(l);
    }
    if let Some(t) = coord.gate.table.as_ref() {
        let (rows, offers, _) = t.summary();
        let (restarts, new_row) = t.restart_stats();
        best.warnings.push(format!("TopoR variant table (one for all {n} runs, TW3; committed in cycle order): {offers} offers, {restarts} restarts ({new_row} led to a new row); {}", table_text(&rows, &obj)));
    }
    best.bests = bests_json;
    if n > 1 {
        let skipped_txt = if skipped.is_empty() {
            String::new()
        } else {
            format!("; without a result of their own (Cancel / OK / the time limit came first): {}", skipped.iter().map(|(k, why)| format!("run {} ({why})", k + 1)).collect::<Vec<_>>().join(", "))
        };
        best.warnings.push(format!("best of {n} seeded runs ([optimize] runs): run {} picked; {}{skipped_txt}", pick_k + 1, lines.join("; ")));
    }
    best.stop_reason = if cancel {
        if ok_kept {
            "stable"
        } else if ok_stop {
            "timeout"
        } else {
            "cancel"
        }
    } else if share_stop {
        "loop_time_share"
    } else if time_hit || draw_cut {
        "timeout"
    } else if stable_stop_all {
        "stable"
    } else if relay_stop_any {
        // TW3 D2: a work cap like rounds, named for what ended it
        "re-lay budget"
    } else {
        "round cap"
    }
    .into();
    if opts.stop_after == "plan" {
        best.stop_reason = "stop_after plan".into();
    }
    best.timed_out = (time_hit || draw_cut) && !(cancel && ok_kept);
    best.converged = !best.timed_out;
    // stitch-vias classes 2-4, once on the picked result, before the hand-off (the plugin
    // refills the zones after it).
    if opts.stitch_islands {
        stitch_regions_on(model, opts, &mut best, &|| ctl.is_cancelled() && !ok_kept);
    }
    post(&mut best);
    best.runtime_s = t0.elapsed().as_secs_f64();
    if best.timed_out {
        let limit = ctl.limit.seconds().unwrap_or(opts.wall_clock_s);
        // Name the limit and its clock. The engine's own limit is the wall clock; a caller that counts
        // another clock (the plugin's [run] timeout_clock = cpu) reaches the limit itself and gives the engine
        // none (`wall_clock_s` 0), so the seconds `limit` are then wall seconds at the reach, not its limit.
        let why = if draw_cut && (ok_stop || drawn_stable && !cancel) {
            format!(
                "{}, but the time limit then cut the drawing of the stable plan: the board is drawn only in part (see the time limit line)",
                if ok_stop { "OK came at Routes stable" } else { "the routes were stable" }
            )
        } else if cancel {
            "Cancel stopped the run before its routes were stable".to_string()
        } else if share_stop {
            let at = ctl.limit.elapsed_s(); // the limit's own clock (optimizing time), as `limit`
            // timeout_scope = "all" only: the limit can still pass after the share, while the drawing
            // runs (soft: it is drawn to its end); with "optimizing" the clock stops at the drawing
            let after = if at >= limit {
                // soft: the drawing runs to its end; past [run] drawing_grace_s only its optional passes stop
                let what = if ctl.limit.draw_grace_hit() { "every wire was drawn, only its optional passes stopped at [run] drawing_grace_s (see the time limit wait line)" } else { "it cut nothing" };
                format!("the time limit of {limit:.0} s (wall clock) passed only after the loop, during the drawing: {what}")
            } else {
                format!("the time limit of {limit:.0} s (wall clock, optimizing time) was not reached, {:.0} s of it were left", limit - at)
            };
            format!(
                "the TopoR loop's own time share stopped it, not the time limit: [optimize] loop_time_share = {:.2} (below the default 1.0) lets the loop use at most that share of the time left when it began; the last loop ended at {share_end_s:.0} s, the optimizing at {at:.0} s; {after}",
                opts.loop_time_share.clamp(0.05, 1.0)
            )
        } else if ctl.limit.changed() {
            format!("the time limit of {limit:.0} s wall clock (changed during the run; [run] timeout_s = {:.0} s at this engine job's start) stopped the run before its routes were stable", opts.wall_clock_s)
        } else if opts.wall_clock_s <= 0.0 {
            format!("the caller's own time limit ([run] timeout_s on its clock, e.g. [run] timeout_clock = cpu; reached {limit:.0} s into the run, wall clock) stopped the run before its routes were stable")
        } else {
            format!("the wall-clock limit of {limit:.0} s for this engine job ([run] timeout_s, less the time the plugin used before the job) stopped the run before its routes were stable")
        };
        best.warnings.push(format!("{TIME_HIT} ({why})"));
    }
    best.limit = limit_report(ctl, opts, &mut best);
    // time-limit-fix: the result's own figures, not the classic engine's placeholders (1 and 1):
    // the engine's thread pool (the compute slots the runs share) and the picked run's cycles.
    best.threads_used = rayon::current_num_threads().max(1);
    best.iterations = u32::try_from(picked_cycles).unwrap_or(u32::MAX);
    best
}

/// TW3: the drawn rows of the four bests over all runs ([`RunOut::rows_drawn`]): their JSON for
/// `result.json` (`bests`), the report line, and when `pick` names a role other than the overall
/// best, the run index (into `outs`) and the result of the drawn row holding that role among the
/// drawn rows. The overall best is the objective's pick of the drawn variants (TopoR 6.3 manual pp.
/// 72-73: violations first, then length at `mm_per_via`). O(rows).
fn four_bests(outs: &[RunOut], obj: &Objective, pick: &str) -> (Vec<serde_json::Value>, Option<String>, Option<(usize, RouteResult)>) {
    use super::archive::{bests, Role};
    let rows: Vec<(usize, &Assigned, &Criteria, &RouteResult)> = outs.iter().enumerate().flat_map(|(i, o)| o.rows_drawn.iter().map(move |(a, c, r)| (i, a, c, r))).collect();
    if rows.is_empty() {
        return (Vec::new(), None, None);
    }
    let cs: Vec<&Criteria> = rows.iter().map(|r| r.2).collect();
    let db = bests(&cs, obj);
    let role = Role::of_pick(pick);
    let ri = Role::ALL.iter().position(|&r| r == role).expect("role");
    let applied = db[ri];
    let drawn_roles = |j: usize| -> Vec<&'static str> { Role::ALL.iter().zip(db).filter(|(_, b)| *b == Some(j)).map(|(r, _)| r.name()).collect() };
    let crit_json = |c: &Criteria| serde_json::json!({"errors": c.errors, "shorts": c.shorts, "open": c.open, "violations": c.violations, "vias": c.vias, "length_mm": (c.length_mm * 1e3).round() / 1e3, "shrinkages": c.shrinkages, "neck_mm": (c.neck_mm * 1e3).round() / 1e3 + 0.0, "neck_vias": c.neck_vias, "necking": (c.necking * 1e3).round() / 1e3, "penalty": (c.penalty * 1e3).round() / 1e3});
    let json: Vec<serde_json::Value> = rows
        .iter()
        .enumerate()
        .map(|(j, (i, a, c, _))| {
            serde_json::json!({"roles": a.roles.iter().map(|r| r.name()).collect::<Vec<_>>(), "drawn_roles": drawn_roles(j), "row": a.label, "run": outs[*i].k + 1, "origin_run": a.row.run + 1, "predicted": crit_json(&a.crit), "drawn": crit_json(c), "applied": applied == Some(j)})
        })
        .collect();
    let line = format!(
        "four bests of the variant table drawn ([drawing] candidates; [optimize] pick = \"{}\" applied): {}",
        role.name(),
        rows.iter()
            .enumerate()
            .map(|(j, (i, a, c, _))| format!(
                "[{}{}] {} (run {}): predicted {} errors / {} vias / {:.1} mm, drawn {} errors ({} shorts, {} open) / {} vias / {:.1} mm / necking {:.1} ({:.1} mm necked, {} neck vias){}",
                if applied == Some(j) { "APPLIED " } else { "" },
                a.roles.iter().map(|r| r.name()).collect::<Vec<_>>().join(", "),
                a.label,
                outs[*i].k + 1,
                a.crit.errors,
                a.crit.vias,
                a.crit.length_mm,
                c.errors,
                c.shorts,
                c.open,
                c.vias,
                c.length_mm,
                c.necking,
                c.neck_mm,
                c.neck_vias,
                { let d = drawn_roles(j); if d.is_empty() { String::new() } else { format!(", drawn best: {}", d.join(", ")) } }
            ))
            .collect::<Vec<_>>()
            .join("; ")
    );
    let role_result = (role != Role::Overall).then_some(applied).flatten().map(|j| (rows[j].0, rows[j].3.clone()));
    (json, Some(line), role_result)
}

/// M2.TL: the start of the loud error when Cancel / OK came before any plan was kept.
pub const NOTHING_TO_DRAW: &str = "nothing to draw yet";

/// pour-pad-join: the report line (and its prefix, which the plugin reads) of a call with no
/// connection to route ([`route_topo`] ends before the plane stubs; stop reason `nothing_to_route`).
pub const NOTHING_TO_ROUTE: &str = "nothing to route: every connection in scope is already made (pads, tracks, vias and pour fills joined as KiCad's connectivity has them); no plane stub, projection or TopoR loop was run and no copper is written";

/// pour-pad-join r2: the report line of a call with no connection to route whose pours still
/// needed stitching vias ([`route_topo`]: stop reason `stitch_only`; the vias are the call's items).
pub const STITCH_ONLY: &str = "nothing to route: every connection in scope is already made (pads, tracks, vias and pour fills joined as KiCad's connectivity has them); only the pours' stitching ran ([planes] stitch_dropped / stitch_thin_mm / stitch_area), no plane stub, projection or TopoR loop";

/// limit-p3p4: the progress stage once P2 gave every connection its place (the first plan): the
/// layer assignment, rip-up and moves (P3/P4) run, and Accept draws the plan so far.
/// The words come first (verify r1 F2): the narrow progress dialog's phase label ends in "..."
/// past its width, so the part the user acts on must not be the tail.
pub const PLAN_READY_STAGE: &str = "first plan ready, Accept will draw it - layer assignment, rip-up and reroute";

/// limit-p3p4: the round's warning when Cancel / OK came during P3/P4.
pub const PLAN_KEPT_STOP: &str = "initial plan: Cancel / OK came during the layer assignment, rip-up and reroute (P3/P4), after every connection had its place; the plan so far (its layering as far as the refinement got) is kept and drawn";

/// limit-p3p4: the round's warning when the soft limit plus its grace ended P3/P4.
pub const PLAN_KEPT_LIMIT: &str = "initial plan: the time limit plus its grace passed during the layer assignment, rip-up and reroute (P3/P4); its refinement and moves stopped at their next check and the plan so far is kept and drawn";

/// limit-p3p4 (lead 2026-11-23, user rule "Cancel stops there and then and takes no results"):
/// the call's result after the dialog's Cancel ([`RunControl::request_discard`]): the job is
/// abandoned, so nothing is drawn, judged or stitched.
pub const DISCARDED: &str = "Cancel: the run was discarded (the dialog's Cancel takes no results); nothing is drawn";

/// [`DISCARDED`] as the call's result: every connection unrouted with it, no copper, stop reason
/// "cancel". O(connections).
fn discarded(model: &BoardModel, opts: &Options, t0: Instant) -> RouteResult {
    let mut r = unrouted(model, opts, DISCARDED);
    r.warnings.insert(0, DISCARDED.to_string());
    r.stop_reason = "cancel".into();
    r.timed_out = false;
    r.converged = false;
    r.runtime_s = t0.elapsed().as_secs_f64();
    r
}

/// M2.TL (fail fast): the call's result when no plan was kept (`why` starts with
/// [`NOTHING_TO_DRAW`]): every connection unrouted with `why`, an `ERROR:` line first, no copper.
/// O(connections).
fn nothing_to_draw(model: &BoardModel, opts: &Options, ctl: &RunControl, why: &str, t0: Instant, post: &mut dyn FnMut(&mut RouteResult)) -> RouteResult {
    let mut r = unrouted(model, opts, why);
    r.warnings.insert(0, format!("ERROR: {why}"));
    r.stop_reason = if ctl.is_cancelled() { "cancel" } else { "timeout" }.into();
    r.timed_out = true;
    post(&mut r);
    r.runtime_s = t0.elapsed().as_secs_f64();
    r.limit = limit_report(ctl, opts, &mut r);
    r
}

/// M2.TL: the call's time limit at its end ([`crate::route::LimitReport`]): reached or not, the
/// step that ran then and the time past it (`r.runtime_s` minus the limit), with report lines in
/// `r.warnings` that name what the limit did: with the soft limit the step that ran finished; when
/// it acted as hard (`hard`, or past the overrun cap) the step that ran was cut and the kept plan
/// drawn in full (one path for both, user 2026-11-13), or a step of the drawing itself was cut
/// (the drawing: connections left unrouted; the via reduction; the pour stitching), or nothing
/// was cut (it passed after the steps it stops had ended). What was cut is what the stop tests
/// noted ([`crate::control::TimeLimit::cut_steps`], verify-m2c 4), not the connections' reasons
/// (a Cancel leaves its own). `None` without a limit. O(connections).
fn limit_report(ctl: &RunControl, opts: &Options, r: &mut RouteResult) -> Option<crate::route::LimitReport> {
    let limit_s = ctl.limit.seconds()?;
    // The limit's own clock (its `start`, taken when the caller built the RunControl), not `r.runtime_s`
    // (from `route_topo`'s `t0`, later): a limit reached just before the run ended would read "not
    // reached" while a stop test had already cut a step on it (limit-flake 2026-11-18: cut_steps
    // named the via reduction with reached = false and no step).
    // time-limit-fix r2: once the final drawing ran off the clock, the limit's clock stood still
    // there (optimizing time); `r.runtime_s` then holds the drawing too.
    let runtime_s = if ctl.limit.in_drawing() { ctl.limit.elapsed_s() } else { ctl.limit.elapsed_s().max(r.runtime_s) };
    let mode = if opts.timeout_mode == "hard" { "hard" } else { "soft" };
    let reached = runtime_s >= limit_s;
    let step = if reached { ctl.snapshot().limit_step } else { String::new() };
    let overrun_s = if reached { ((runtime_s - limit_s) * 1e3).round() / 1e3 } else { 0.0 };
    let cap_s = ctl.limit.overrun_cap_s();
    let cap_hit = mode == "soft" && ctl.limit.cap_hit();
    let steps = ctl.limit.cut_steps();
    let drawn_cut: Vec<CutStep> = steps.iter().copied().filter(|s| s.in_drawing()).collect();
    let cut = if steps.is_empty() { "none" } else if drawn_cut.is_empty() { "plan" } else { "drawing" };
    let undrawn = r.connections.iter().filter(|c| c.reason.starts_with(DRAW_CUT)).count();
    // What the limit did when it acted as hard (`hard`, or the soft limit past its overrun cap).
    let plan_text = |words: &str| format!("the step that ran then was cut and the kept plan (the best variant so far, or the initial plan) was drawn in full (cut: {words}; a cut initial routing leaves the connections it had not reached unrouted; a cut TopoR loop cycle is not kept)");
    let plan_words = steps.iter().filter(|s| !s.in_drawing()).map(|s| s.words()).collect::<Vec<_>>().join(", ");
    let draw_text = || {
        let parts: Vec<String> = drawn_cut
            .iter()
            .map(|s| match s {
                CutStep::Drawing => format!("the drawing of the traces was cut: {} of {} connections drawn, {undrawn} left undrawn by the cut", r.routed, r.connections_total),
                CutStep::ViaReduction => "the via reduction was cut (the vias it had not removed yet stay)".to_string(),
                _ => "the pour stitching was cut (pour pieces it had not joined stay apart: open connections in KiCad's DRC)".to_string(),
            })
            .collect();
        format!("the drawing was the step that ran then: {}", parts.join("; "))
    };
    let as_hard = match cut {
        "drawing" if plan_words.is_empty() => draw_text(),
        "drawing" => format!("{}; in a run whose drawing ran then {}", plan_text(&plan_words), draw_text()),
        "plan" => plan_text(&plan_words),
        _ => format!("it passed only after the steps it stops had ended{}: nothing was cut", if ctl.snapshot().stage.is_empty() { String::new() } else { format!(" (during {})", ctl.snapshot().stage) }),
    };
    if reached {
        r.warnings.push(format!(
            "time limit ({mode}, [run] timeout_mode): {limit_s:.0} s reached during {}; {}; {} {overrun_s:.1} s past the limit (wall clock{})",
            if step.is_empty() { "the run" } else { step.as_str() },
            if cap_hit { format!("the step that ran went on until the overrun cap, then {as_hard}") } else if mode == "soft" { "the step that ran finished, no new one began and the best kept variant was drawn".to_string() } else { as_hard.clone() },
            // time-limit-fix r2: with the final drawing off the clock the overrun ends with the optimizing
            if ctl.limit.in_drawing() { "the optimizing ended" } else { "the result was ready" },
            if ctl.limit.in_drawing() { format!("; the final drawing after it took {:.1} s and is not counted", ctl.limit.drawing_s()) } else { String::new() }
        ));
    }
    if cap_hit {
        // F3 (user 2026-11-13): loud; says what the cap did (cut a step, cut the drawing, or nothing)
        r.warnings.push(format!(
            "WARNING: time limit overrun cap reached ({}): past it the soft limit acted as the hard one: {as_hard}; raise [run] timeout_overrun_s or timeout_s, or use Cancel instead",
            cap_s.map_or("the caller's cap on its own clock".to_string(), |c| format!("[run] timeout_overrun_s = {c:.0} s past the {limit_s:.0} s limit, wall clock"))
        ));
    }
    if !drawn_cut.is_empty() {
        // verify-m2c 3 (decided behaviour, user decision on a change pending): a limit acting as
        // hard that passes while the drawing runs cuts it; the board is then drawn only in part.
        let what = drawn_cut.iter().map(|s| s.words()).collect::<Vec<_>>().join(", ");
        r.warnings.push(format!(
            "{DRAWING_CUT_LINE}{} ({what}): {} of {} connections drawn{}; {}",
            if cap_hit { " (the soft limit past its overrun cap acts as the hard one)" } else { "" },
            r.routed,
            r.connections_total,
            if undrawn > 0 { format!(", {undrawn} left undrawn by the cut") } else { String::new() },
            if cap_hit { "raise [run] timeout_overrun_s or timeout_s for a complete board (the drawing began before the cap and ran past it)" } else { "use [run] timeout_mode = \"soft\" or a longer [run] timeout_s for a complete board (the drawing began before the limit and ran past it)" }
        ));
    }
    let cut_steps = steps.iter().map(|s| s.name().to_string()).collect();
    // The wait of the soft step the limit fell in (the initial routing, the TopoR loop, the drawing;
    // with loop = "cycles" the whole-route cycle; lead 2026-11-17), always named (TIME_LIMIT_WAIT),
    // in hard mode too. A step no soft-step change ended after the limit (the last one) ran to the
    // result.
    let grace_s = ctl.limit.grace_s().map(|g| (g * 1e3).round() / 1e3);
    let grace_hit = mode == "soft" && ctl.limit.grace_hit();
    let cycle_dropped = reached && ctl.limit.cycle_dropped();
    let draw_grace_s = ctl.limit.draw_grace_s().map(|g| (g * 1e3).round() / 1e3);
    let draw_grace_hit = reached && ctl.limit.draw_grace_hit();
    let draw_grace_erbs = if reached { ctl.limit.draw_grace_erbs() } else { 0 };
    let wait_raw = if !reached { 0.0 } else if ctl.limit.step_end_noted() || ctl.limit.wait_s() > 0.0 { ctl.limit.wait_s() } else { (runtime_s - limit_s).max(0.0) };
    let wait_s = (wait_raw * 1e3).round() / 1e3;
    if wait_line_due(reached, wait_raw, cycle_dropped) {
        let w = WaitFacts {
            step: step.clone(),
            whole_cycle: opts.opt_loop != "topor",
            hard: mode == "hard",
            cap_hit,
            step_cut: ctl.limit.cut_in_wait(),
            cut_words: steps.iter().map(|s| s.words()).collect::<Vec<_>>().join(", "),
            grace_s,
            grace_hit,
            cycle_dropped,
            draw_grace_s,
            draw_grace_hit,
            draw_grace_erbs,
            draw_cut: steps.contains(&CutStep::Drawing),
            wait_s: wait_raw,
        };
        r.warnings.push(wait_line(&w));
    }
    Some(crate::route::LimitReport { limit_s, mode: mode.into(), reached, step, overrun_s, cap_s, cap_hit, cut: cut.into(), cut_steps, grace_s, grace_hit, wait_s, cycle_dropped, draw_grace_s, draw_grace_hit, draw_grace_erbs, optimize_s: (runtime_s * 1e3).round() / 1e3, draw_s: (ctl.limit.drawing_s() * 1e3).round() / 1e3, scope: if ctl.limit.in_drawing() || opts.timeout_scope != "all" { "optimizing" } else { "all" }.into() })
}

/// What the `time limit wait:` line says ([`wait_line`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct WaitFacts {
    /// The stage text(s) the limit fell in (`Progress::limit_step`).
    pub step: String,
    /// `[optimize] loop = "cycles"`: the soft step is the whole-route cycle.
    pub whole_cycle: bool,
    /// `[run] timeout_mode = "hard"`.
    pub hard: bool,
    /// The soft limit's overrun cap fired (or the plugin's CPU clock hardened the limit).
    pub cap_hit: bool,
    /// The limit, acting as hard, cut the soft step it fell in ([`crate::control::TimeLimit::cut_in_wait`]).
    pub step_cut: bool,
    /// The cut steps in words (`CutStep::words`).
    pub cut_words: String,
    pub grace_s: Option<f64>,
    pub grace_hit: bool,
    pub cycle_dropped: bool,
    pub draw_grace_s: Option<f64>,
    pub draw_grace_hit: bool,
    pub draw_grace_erbs: usize,
    /// The limit, acting as hard, cut the drawing of the traces ([`CutStep::Drawing`]): not every
    /// wire was drawn, even after a drawing-bound hit.
    pub draw_cut: bool,
    /// The unrounded wait (s).
    pub wait_s: f64,
}

/// Seconds as the report lines write a setting: whole numbers without decimals. O(1).
fn secs_text(g: f64) -> String {
    if g == g.round() { format!("{g:.0}") } else { format!("{g}") }
}

/// The `time limit wait:` line (TIME_LIMIT_WAIT): where the limit fell (stage and soft step), how
/// long that soft step went on past it, and why, true for the mode (soft: what bounded or did not
/// bound it; hard / past the overrun cap: that it was cut, or had no stop check left). O(len).
pub(crate) fn wait_line(w: &WaitFacts) -> String {
    let first = w.step.split("; ").next().unwrap_or("");
    let first = first.strip_prefix("run ").and_then(|t| t.split_once(": ")).map_or(first, |(_, s)| s);
    let soft = crate::control::soft_step_of(first);
    let in_loop = soft == Some(crate::control::SoftStep::Loop) || w.step.contains("TopoR loop");
    let in_drawing = soft == Some(crate::control::SoftStep::Drawing);
    // limit-p3p4: the layer assignment, rip-up and moves of the initial plan (P3/P4) are bounded by
    // `[run] limit_grace_s` like the TopoR loop step; the plan so far is kept.
    // With `[optimize] loop = "cycles"` (verify r1 F1) the bound holds too: the cycle that runs has
    // its P3/P4 cut at limit + grace; only the texts of a step that finished differ (the cycle is
    // the soft step there).
    let plan_step = soft == Some(crate::control::SoftStep::Initial) && (first.starts_with("layer assignment") || first.starts_with("first plan ready"));
    let in_plan = plan_step && !w.whole_cycle;
    let part = if w.whole_cycle && soft.is_some() { " (part of its whole-route cycle, [optimize] loop = \"cycles\")".to_string() } else { soft.map_or(String::new(), |s| format!(" (part of {})", s.words())) };
    let as_hard = w.hard || w.cap_hit;
    let who = if w.hard { "[run] timeout_mode = hard" } else { "the overrun cap ([run] timeout_overrun_s, or the plugin's CPU-clock cap)" };
    let shown = if w.wait_s < 0.1 { format!("{:.3}", w.wait_s) } else { format!("{:.1}", w.wait_s) };
    let dropped = "the cycle it was in was dropped, the best variant so far was drawn";
    let draw_note = |lead: &str| -> String {
        match (w.draw_grace_hit, w.draw_grace_s) {
            (true, Some(g)) => format!(
                "{lead}{}[run] drawing_grace_s = {} s ran out, so the drawing's optional passes stopped (the via reduction{}); raise drawing_grace_s to let them finish{}",
                if w.draw_cut { "" } else { "every wire was drawn; " },
                secs_text(g),
                if w.draw_grace_erbs > 0 { format!("; {} wire(s) drawn the sequential way instead of by their ERBS", w.draw_grace_erbs) } else { String::new() },
                if w.draw_cut { "; then the limit, acting as hard, cut the drawing of the traces (wires it had not drawn stay undrawn)" } else { "" }
            ),
            _ => String::new(),
        }
    };
    let why = if in_loop && w.grace_hit && w.grace_s.is_some() {
        format!("[run] limit_grace_s = {} s ran out: the step was stopped at its next check, {dropped}; raise limit_grace_s to let a cycle finish", secs_text(w.grace_s.unwrap_or(0.0)))
    } else if plan_step && w.grace_hit && w.grace_s.is_some() && !as_hard {
        format!(
            "[run] limit_grace_s = {} s ran out: the layer assignment, rip-up and moves (P3/P4){} stopped at their next check, the plan so far (every connection placed) was kept and drawn; raise limit_grace_s to let them run longer",
            secs_text(w.grace_s.unwrap_or(0.0)),
            if w.whole_cycle { " of the whole-route cycle that ran" } else { "" }
        )
    } else if w.cycle_dropped && as_hard {
        format!("{who}: the step was cut at its next check, {dropped}{}", draw_note("; "))
    } else if w.cycle_dropped {
        format!("the step was cut at its next check, {dropped}{}", draw_note("; "))
    } else if w.step_cut && as_hard {
        format!("{who}: the limit cut this step at its next check (cut: {})", if w.cut_words.is_empty() { "the step that ran" } else { w.cut_words.as_str() })
    } else if as_hard {
        format!("{who}: the limit did not cut this step, it ran to its end (it had no stop check left after the limit, or ran within the 1 s each round gets)")
    } else if in_loop {
        match w.grace_s {
            Some(g) if w.wait_s <= g => format!("the step finished within [run] limit_grace_s = {} s", secs_text(g)),
            Some(g) => format!("the step ran past [run] limit_grace_s = {} s because it was between two checks or finishing its last cycle; no cycle was dropped", secs_text(g)),
            None => "[run] limit_grace_s is unbounded (negative): the step ran to the end of its cycle".to_string(),
        }
    } else if in_plan {
        match w.grace_s {
            Some(g) if w.wait_s <= g => format!("the layer assignment, rip-up and moves (P3/P4) finished within [run] limit_grace_s = {} s", secs_text(g)),
            Some(g) => format!("the layer assignment, rip-up and moves (P3/P4) ran past [run] limit_grace_s = {} s because they were between two checks", secs_text(g)),
            None => "[run] limit_grace_s is unbounded (negative): the layer assignment, rip-up and moves (P3/P4) ran to their end".to_string(),
        }
    } else if in_drawing && !w.whole_cycle {
        let d = draw_note("");
        if !d.is_empty() {
            d
        } else {
            match w.draw_grace_s {
                Some(g) => format!("every wire was drawn and the drawing's optional passes finished within [run] drawing_grace_s = {} s (it bounds only those)", secs_text(g)),
                None => "every wire was drawn; [run] drawing_grace_s is unbounded (negative): the optional passes ran to their end".to_string(),
            }
        }
    } else {
        "the soft limit lets this step run to its end ([run] limit_grace_s bounds only the layer assignment and rip-up after the topological search and the TopoR loop step, drawing_grace_s only the drawing's optional passes)".to_string()
    };
    let tail = if in_drawing && !w.whole_cycle { String::new() } else { draw_note("; then the drawing: ") };
    format!(
        "time limit wait: the limit was reached during {}{part}; {} {shown} s ({why}){tail}",
        if w.step.is_empty() { "the run" } else { w.step.as_str() },
        if as_hard && (w.step_cut || w.cycle_dropped) { "that step stopped after" } else { "waited" }
    )
}

/// Whether the report carries a `time limit wait:` line: the limit was reached and the step it fell
/// in ran on past it (the unrounded wait) or a TopoR loop cycle was dropped (a cut that lands a
/// moment after the limit has a wait that rounds to 0 but still lost the cycle). O(1).
pub(crate) fn wait_line_due(reached: bool, wait_s: f64, cycle_dropped: bool) -> bool {
    reached && (wait_s > 0.0 || cycle_dropped)
}

/// The reason of a connection the time limit left undrawn (the drawing was the step that ran).
const DRAW_CUT: &str = "time limit reached before this connection was drawn";

/// The reason of a connection a Cancel / OK left undrawn in a drawing that it ends (a whole-route
/// cycle of `[optimize] loop = "cycles"`; the TopoR loop's one drawing runs through a Cancel).
const DRAW_CANCEL: &str = "Cancel / OK came before this connection was drawn";

/// The start of the loud line when the time limit, acting as hard, cut a step of the drawing.
pub const DRAWING_CUT_LINE: &str = "WARNING: the drawing was cut by the hard time limit";

/// One run of a best-of-N call, as [`route_topo`] needs it back.
struct RunOut {
    /// The run (0-based).
    k: usize,
    /// TW3: the rows of the four bests this run drew, each with the better of its drawn variants
    /// (pour-stitched, judged by the objective) and that variant's result.
    rows_drawn: Vec<(Assigned, Criteria, RouteResult)>,
    result: RouteResult,
    crit: Criteria,
    /// Cancel or the time budget ended it before its routes were stable.
    time_hit: bool,
    /// The run ended BECAUSE its routes were stable (the loop's or the run's stop at stable), not
    /// because it reached the cycle cap with stable recorded (a dialog run goes on past stable).
    stable_stop: bool,
    /// TW3 D2: its TopoR loop ended at the re-lay budget (`[optimize] loop_relays`).
    relay_stop: bool,
    /// time-limit-fix: its TopoR loop ended at the loop's own time share
    /// (`[optimize] loop_time_share`) before the time limit passed, and nothing cut it after.
    share_stop: bool,
    /// The cycles of its TopoR loop (0 without one), and when the loop ended (s on the limit's clock).
    loop_cycles: usize,
    loop_end_s: f64,
    /// The time limit, acting as hard, cut the one drawing of its TopoR loop pick (the traces, the
    /// via reduction or the pour stitching): its board is drawn only in part, whatever ended the
    /// loop (verify-sl3 F1: an OK at Routes stable then a hard limit in the drawing).
    draw_cut: bool,
    /// Its routes were stable (at some cycle), however the run ended.
    stable: bool,
    /// Its summary for the report, and its cycle by cycle line.
    line: String,
    cycles_line: String,
}

/// The coordinator of the runs of one call: the progress fields of M-B0 over all runs.
struct Coord<'a> {
    ctl: &'a RunControl,
    obj: Objective,
    st: std::sync::Mutex<Vec<RunState>>,
    /// cancel-fix: criteria of the call's best pick so far ([`Coord::publish`]) and whether it is
    /// of a complete cycle (not cut short).
    snap: std::sync::Mutex<Option<(Criteria, bool)>>,
    /// M2: which runs draw their TopoR loop pick ([`DrawGate`]).
    gate: DrawGate,
    /// cancel-fix: after Cancel / OK the cut-short cycles of the runs are stitched and judged one at
    /// a time, and only one that beats the cut-short cycles judged before it (their criteria before
    /// the stitching, held here) while no complete result is published ([`Coord::worth_judging`]):
    /// on qmtech four runs stitched their cut-short cycles side by side (1.5-2 s each).
    judge: std::sync::Mutex<Option<Criteria>>,
}

/// M2 = 6.1 draw once, `[optimize] loop = "topor"`: which runs draw their TopoR loop pick. Every
/// run offers its pick with its predicted criteria (the plan's measure, nothing drawn) or
/// withdraws (it was not begun, or Cancel came before its initial routing ended: nothing kept);
/// when no run can offer any more, the best `[drawing] candidates` picks by the objective draw
/// (ties: the lower run), after Cancel / OK only the best one. TopoR judges its variants on the
/// topology and draws the one picked (6.3 manual pp. 72-73; Polubasov 2010: the exact geometry
/// «на метрическом этапе»). Every run has a thread of its own ([`run_all`]), so the decision sees
/// every run's pick and does not depend on the thread count; only `slots` of them compute at a
/// time ([`Slots`]: the engine's share of the cores), a run waiting here gives its slot back.
/// `opt_loop = "cycles"` draws every cycle and does not use the gate.
struct DrawGate {
    st: std::sync::Mutex<GateState>,
    cv: std::sync::Condvar,
    /// The runs that compute at once (and the refinements' spare lanes).
    slots: std::sync::Arc<Slots>,
    /// TW3: the call's one variant table (`[optimize] loop = "topor"`); its four bests are drawn
    /// ([`DrawGate::assign`]). `None`: the gate picks runs by their own picks (before TW3).
    table: Option<std::sync::Arc<super::table::SharedTable<LoopVar>>>,
}

/// TW3: a run's door to the shared table ([`Shared`](super::optimise::Shared)): its index, its
/// pre-loop key, and the compute slot it gives back while it waits at a sync point.
struct Port<'g> {
    table: &'g super::table::SharedTable<LoopVar>,
    k: usize,
    compat: u64,
    slots: &'g Slots,
    /// sync-wait: the per-cycle log line ([`super::optimise::Shared::cycle_done`]).
    note: &'g dyn Fn(&super::optimise::CycleNote),
}

/// TW3 (condition 1 of the lead): who draws which of the four bests' distinct rows. `rows` =
/// (origin run, pre-loop key) per row in the role order; only the first `quota` rows are drawn
/// (`[drawing] candidates`; 1 after Cancel / OK). A row goes to its origin run when that run has
/// the same pre-loop state (`compat`: run -> key of the runs that reached the gate) and no row yet,
/// else to the compatible run with the fewest rows so far (lowest run first), so the drawings run
/// in parallel where runs are free (a restored row draws the same copper on any run,
/// tests/topo_tw3_drawonly.rs); `None` = no compatible run (undrawable). Result: one entry per
/// drawn row (`quota` or fewer). O(rows x runs).
fn assign_runs(rows: &[(usize, u64)], compat: &BTreeMap<usize, u64>, quota: usize) -> Vec<Option<usize>> {
    let mut load: BTreeMap<usize, usize> = BTreeMap::new();
    let mut who = Vec::new();
    for &(origin, key) in rows.iter().take(quota) {
        let cands: Vec<usize> = compat.iter().filter(|(_, &k)| k == key).map(|(&r, _)| r).collect();
        let free = |r: &usize| load.get(r).copied().unwrap_or(0);
        let w = if cands.contains(&origin) && free(&origin) == 0 { Some(origin) } else { cands.iter().copied().min_by_key(|r| (free(r), *r)) };
        if let Some(r) = w {
            *load.entry(r).or_default() += 1;
        }
        who.push(w);
    }
    who
}

impl super::optimise::Shared<LoopVar> for Port<'_> {
    fn offer(&self, cycle: usize, label: String, gen: usize, crit: Criteria, var: LoopVar, fp: u64, check: u8) {
        let row = super::table::Row { var: std::sync::Arc::new(var), run: self.k, cycle, gen, fp, compat: self.compat };
        self.table.offer(self.k, cycle, label, crit, row, check);
    }
    fn offer_mid(&self, cycle: usize, label: String, gen: usize, crit: Criteria, var: LoopVar, fp: u64) {
        let row = super::table::Row { var: std::sync::Arc::new(var), run: self.k, cycle, gen, fp, compat: self.compat };
        self.table.offer_mid(self.k, cycle, label, crit, row);
    }
    fn sync(&self, cycle: usize, stop: &mut dyn FnMut() -> bool) -> Option<super::table::View<LoopVar>> {
        // the wait gives the slot back; it goes on at its place in the fair order (cycles finished)
        self.table.sync(self.k, cycle, stop, &|| self.slots.release(), &|| self.slots.acquire_as(1, cycle.saturating_sub(1), self.k))
    }
    fn cycle_done(&self, note: &super::optimise::CycleNote) {
        (self.note)(note);
    }
    fn end(&self) {
        self.table.end(self.k);
    }
    fn me(&self) -> (usize, u64) {
        (self.k, self.compat)
    }
}

/// A counting semaphore: at most `n` runs compute at once (the others' threads wait idle, so the
/// busy threads stay at the engine's share, `min(runs, rayon threads)`). A run that waits for its
/// FIRST slot (not begun yet) goes before one that waits to go on.
///
/// sync-wait (docs/dev/design/sync-wait.md): fair slots. Every waiter has a key (class, clock,
/// run): class 0 = its first slot, 1 = to go on; clock = the TopoR loop cycles it has finished. A
/// free slot goes to the waiter with the lowest key, and a loop run at each cycle boundary gives
/// its slot to a waiter with a lower key ([`Slots::turn`]): more runs than slots then share the
/// slots round-robin in cycles (no run begins late), a run waiting at the variant table hands its
/// slot to the run it waits for, and a run that gave its slot (stable or not) gets one back as soon
/// as it is the lowest. Copper-neutral: a run's cycles depend only on its own state and its table
/// views, never on when it computes. User 2026-11-17 "free the slot" (a stable loop gives its slot
/// to a run not yet begun) is the case class 0 < class 1. O(log waiters) per call.
///
/// par-refine (docs/design/47-parallel-refine.md §6): there are as many slots as rayon threads;
/// a run takes one for its steps and its refinements borrow the free ones as lanes
/// (`[optimize] refine_lanes = "auto"`, [`super::refine_par::lanes::Pool`]) for one epoch at a
/// time, never while any run waits for a slot (its first one or one to go on): a slot a loop
/// yields to a run not yet begun is never borrowed as a lane before that run has it.
///
/// It also measures the slot idle time of the TopoR loops: slot-seconds free while at least one
/// run is in its loop ([`Slots::enter_loop`], [`Slots::idle`]).
struct Slots {
    st: std::sync::Mutex<SlotState>,
    cv: std::sync::Condvar,
}

/// A waiter's place: (class, clock, run, ticket); the ticket keeps keys distinct.
type SlotKey = (u8, usize, usize, u64);

#[derive(Debug)]
struct SlotState {
    /// The slots in all.
    n: usize,
    /// Free slots; below 0 only for a moment after [`Slots::take_now`] (a slot owed).
    free: isize,
    /// Runs waiting for their first slot ([`Slots::acquire_first`]).
    first: usize,
    /// Runs waiting in [`Slots::acquire`] to go on (par-refine: no lane is lent while one waits).
    waiting: usize,
    /// Every waiter's key; the lowest takes the next free slot.
    queue: BTreeSet<SlotKey>,
    ticket: u64,
    /// Runs in their TopoR loop now; the free slot-seconds while it is above 0, and those seconds.
    loops: usize,
    idle_s: f64,
    span_s: f64,
    last: Instant,
}

impl SlotState {
    /// Add the free slot-time since the last change (while a loop runs). O(1).
    fn tick(&mut self) {
        let now = Instant::now();
        if self.loops > 0 {
            let dt = now.duration_since(self.last).as_secs_f64();
            self.idle_s += self.free.max(0) as f64 * dt;
            self.span_s += dt;
        }
        self.last = now;
    }
}

impl Slots {
    fn new(n: usize) -> Slots {
        let n = n.max(1);
        Slots { st: std::sync::Mutex::new(SlotState { n, free: n as isize, first: 0, waiting: 0, queue: BTreeSet::new(), ticket: 0, loops: 0, idle_s: 0.0, span_s: 0.0, last: Instant::now() }), cv: std::sync::Condvar::new() }
    }

    /// Waits until a slot is free and `(class, clock, run)` is the lowest waiter, and takes it.
    /// O(log waiters) + the wait.
    fn acquire_as(&self, class: u8, clock: usize, run: usize) {
        let Ok(mut g) = self.st.lock() else { return };
        g.tick();
        let key = Self::enqueue(&mut g, class, clock, run);
        self.wait_for(g, key);
    }

    /// Puts `(class, clock, run)` in the waiter queue (lock held). O(log waiters).
    fn enqueue(g: &mut SlotState, class: u8, clock: usize, run: usize) -> SlotKey {
        let key = (class, clock, run, g.ticket);
        g.ticket += 1;
        g.queue.insert(key);
        if class == 0 { g.first += 1 } else { g.waiting += 1 }
        key
    }

    /// Waits (lock held, queued as `key`) until a slot is free and `key` is the lowest waiter, and
    /// takes it. O(log waiters) + the wait.
    fn wait_for(&self, mut g: std::sync::MutexGuard<'_, SlotState>, key: SlotKey) {
        let class = key.0;
        while !(g.free > 0 && g.queue.first() == Some(&key)) {
            g = match self.cv.wait(g) {
                Ok(g) => g,
                Err(_) => return,
            };
        }
        g.tick();
        g.queue.remove(&key);
        if class == 0 { g.first -= 1 } else { g.waiting -= 1 }
        g.free -= 1;
        let more = g.free > 0 && !g.queue.is_empty();
        drop(g);
        if more {
            // the next lowest waiter may take the next free slot
            self.cv.notify_all();
        }
    }

    /// Waits to go on (after a wait that gave the slot back), behind every run waiting for its
    /// first slot. O(log waiters) + the wait.
    fn acquire(&self) {
        self.acquire_as(1, 0, usize::MAX);
    }

    /// A run's first slot: waits for a free one (before the runs that wait to go on). O(log
    /// waiters) + the wait.
    fn acquire_first(&self) {
        self.acquire_as(0, 0, usize::MAX);
    }

    /// The caller (loop run `run`, `clock` cycles finished) holds a slot: when a lower waiter
    /// waits (a run not begun, or a loop run with fewer cycles finished, or as many and a lower
    /// index), gives the slot to it and waits to go on at `(clock, run)`; returns whether it did.
    /// O(log waiters) + the wait.
    fn turn(&self, clock: usize, run: usize) -> bool {
        let Ok(mut g) = self.st.lock() else { return false };
        let lower = g.queue.first().is_some_and(|k| (k.0, k.1, k.2) < (1, clock, run));
        if !lower {
            return false;
        }
        g.tick();
        g.free += 1;
        // queued under the same lock that frees the slot: no waiter behind this run can take the
        // slot back in between (the lower waiter takes it, then this run is next)
        let key = Self::enqueue(&mut g, 1, clock, run);
        self.cv.notify_all();
        self.wait_for(g, key);
        true
    }

    /// Takes a slot without waiting (a slot owed while none is free): a panic unwinding through
    /// [`DrawGate::offer`], whose [`SlotGuard`] gives the slot back once more. O(1).
    fn take_now(&self) {
        if let Ok(mut g) = self.st.lock() {
            g.tick();
            g.free -= 1;
        }
    }

    /// Gives a slot back. O(1).
    fn release(&self) {
        self.give_back(1);
    }

    /// Gives `k` slots back. O(1).
    fn give_back(&self, k: usize) {
        if let Ok(mut g) = self.st.lock() {
            g.tick();
            g.free += k as isize;
        }
        // waiters of several keys: wake all, each tests whether it is the lowest
        self.cv.notify_all();
    }

    /// A run begins its TopoR loop (the idle measure runs while one does). O(1).
    fn enter_loop(&self) {
        if let Ok(mut g) = self.st.lock() {
            g.tick();
            g.loops += 1;
        }
    }

    /// A run ended its TopoR loop. O(1).
    fn leave_loop(&self) {
        if let Ok(mut g) = self.st.lock() {
            g.tick();
            g.loops = g.loops.saturating_sub(1);
        }
    }

    /// (free slot-seconds while a loop ran, seconds a loop ran, slots), up to now. O(1).
    fn idle(&self) -> (f64, f64, usize) {
        self.st.lock().map_or((0.0, 0.0, 1), |mut g| {
            g.tick();
            (g.idle_s, g.span_s, g.n)
        })
    }
}

impl super::refine_par::lanes::Pool for Slots {
    /// Up to `k` free slots, none while a run waits for one (its first slot or one to go on: a
    /// run's step goes first, also before a yielded slot is borrowed). O(1).
    fn try_take(&self, k: usize) -> usize {
        let Ok(mut g) = self.st.lock() else { return 0 };
        if g.waiting > 0 || g.first > 0 || g.free <= 0 {
            return 0;
        }
        g.tick();
        let t = k.min(g.free as usize);
        g.free -= t as isize;
        t
    }

    fn give(&self, k: usize) {
        if k > 0 {
            self.give_back(k);
        }
    }
}

/// Holds a [`Slots`] slot for its scope (also while a panic unwinds). O(1).
struct SlotGuard<'s>(&'s Slots);

impl<'s> SlotGuard<'s> {
    /// A run's first slot ([`Slots::acquire_first`]): with the TopoR loop a run takes one slot,
    /// for its one cycle. O(1) + the wait.
    fn take(s: &'s Slots) -> SlotGuard<'s> {
        s.acquire_first();
        SlotGuard(s)
    }
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct GateState {
    /// `[drawing] candidates` (at least 1).
    quota: usize,
    /// Runs that may still offer.
    open: BTreeSet<usize>,
    offered: BTreeMap<usize, Criteria>,
    /// The runs that draw, once decided.
    decided: Option<Vec<usize>>,
    /// TW3: the offering runs' pre-loop keys, the rows each run draws (once decided) and the rows
    /// of the four bests no run could draw (no run with the row's pre-loop key; said in the report).
    compat: BTreeMap<usize, u64>,
    rows: Option<BTreeMap<usize, Vec<Assigned>>>,
    undrawable: usize,
}

impl DrawGate {
    fn new(n: usize, quota: usize, slots: usize) -> DrawGate {
        DrawGate {
            st: std::sync::Mutex::new(GateState { quota: quota.max(1), open: (0..n).collect(), offered: BTreeMap::new(), decided: None, compat: BTreeMap::new(), rows: None, undrawable: 0 }),
            cv: std::sync::Condvar::new(),
            slots: std::sync::Arc::new(Slots::new(slots)),
            table: None,
        }
    }

    /// TW3: run `k` (pre-loop key `compat`, its own pick `crit`) ended its loop; waits until no
    /// run can offer any more, then the distinct rows of the table's four bests (TopoR 6.3 manual
    /// pp. 72-73: the user picks among the table's variants; here they are drawn and compared at
    /// 100 % drawn) are given out, `[drawing] candidates` of them (after Cancel / OK the overall
    /// best only): each row to its own run when that run offered, else to the offering run with
    /// the same pre-loop key that has the fewest rows (ties: the lower run). Returns the rows run
    /// `k` draws, in role order (empty: none). O(runs x rows) + the wait.
    fn assign(&self, k: usize, compat: u64, crit: Criteria, obj: &Objective, cancel: bool) -> Vec<Assigned> {
        self.slots.release();
        struct Retake<'s>(&'s Slots);
        impl Drop for Retake<'_> {
            fn drop(&mut self) {
                if std::thread::panicking() { self.0.take_now() } else { self.0.acquire() }
            }
        }
        let _retake = Retake(&self.slots);
        const POISONED: &str = "TW3 draw gate: its lock is poisoned (a run panicked while the four bests were given out)";
        let mut g = self.st.lock().expect(POISONED);
        g.open.remove(&k);
        g.offered.insert(k, crit);
        g.compat.insert(k, compat);
        self.cv.notify_all();
        while g.rows.is_none() && !g.open.is_empty() {
            g = self.cv.wait_timeout(g, std::time::Duration::from_millis(200)).expect(POISONED).0;
        }
        if g.rows.is_none() {
            let table = self.table.as_ref().expect("assign needs the table");
            let bests = table.final_bests();
            let keys: Vec<(usize, u64)> = bests.iter().map(|b| (b.3.run, b.3.compat)).collect();
            let who = assign_runs(&keys, &g.compat, if cancel { 1 } else { g.quota });
            let mut out: BTreeMap<usize, Vec<Assigned>> = BTreeMap::new();
            let mut undrawable = 0;
            for ((roles, label, c, row), w) in bests.into_iter().zip(who) {
                match w {
                    Some(r) => out.entry(r).or_default().push(Assigned { roles, label, crit: c, row }),
                    None => undrawable += 1,
                }
            }
            // a run with its own row draws it first; the rows of one run keep the role order
            g.rows = Some(out);
            g.undrawable = undrawable;
            let _ = obj;
        }
        g.rows.as_ref().and_then(|m| m.get(&k).cloned()).unwrap_or_default()
    }

    /// TW3: why a run draws nothing ([`DrawGate::assign`] gave it no row). O(runs).
    fn why_not(&self, cancel: bool) -> String {
        let g = self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let runs: Vec<String> = g.rows.as_ref().map(|m| m.keys().map(|r| (r + 1).to_string()).collect()).unwrap_or_default();
        if cancel {
            format!("after Cancel / OK only the overall best of the variant table is drawn, by run {}", runs.join(", "))
        } else {
            format!("[drawing] candidates = {}: the rows of the variant table's four bests are drawn by run(s) {}", g.quota, runs.join(", "))
        }
    }

    /// Run `k` will not offer (not begun, nothing kept, or it offered already: then nothing
    /// changes). Idempotent. O(log runs).
    fn withdraw(&self, k: usize) {
        // also from a run's Drop while a panic unwinds: never panic here, take the state as it is
        // (the gate's state is a set and two maps, consistent between its statements)
        let mut g = self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.open.remove(&k);
        drop(g);
        // TW3: a run that withdraws holds no cycle of the table back
        if let Some(t) = &self.table {
            t.end(k);
        }
        self.cv.notify_all();
    }

    /// Run `k` offers its pick with predicted criteria `crit`; `cancel`: Cancel / OK came (one
    /// drawing only). Waits until no run can offer any more, then decides once. `None` = run `k`
    /// draws; else why not. O(runs log runs) + the wait for the other runs' loops.
    fn offer(&self, k: usize, crit: Criteria, obj: &Objective, cancel: bool) -> Option<String> {
        // the wait gives this run's compute slot back, so a run without one can finish its loop
        self.slots.release();
        // ... and takes it again after the wait; when `decide` panics (a poisoned gate) without
        // a wait, since this run's SlotGuard gives it back once more on the unwind.
        struct Retake<'s>(&'s Slots);
        impl Drop for Retake<'_> {
            fn drop(&mut self) {
                if std::thread::panicking() { self.0.take_now() } else { self.0.acquire() }
            }
        }
        let _retake = Retake(&self.slots);
        self.decide(k, crit, obj, cancel)
    }

    /// [`DrawGate::offer`] without the slot: waits until no run can offer, decides once. O(runs log runs) + the wait.
    fn decide(&self, k: usize, crit: Criteria, obj: &Objective, cancel: bool) -> Option<String> {
        // A poisoned lock = a run panicked while it decided: loud (the call's thread scope passes the
        // panic on to the caller), never a quiet "draw every pick" (verify-m2 F7c).
        const POISONED: &str = "M2 draw gate: its lock is poisoned (a run panicked while the runs' picks were decided)";
        let mut g = self.st.lock().expect(POISONED);
        g.open.remove(&k);
        g.offered.insert(k, crit);
        self.cv.notify_all();
        while g.decided.is_none() && !g.open.is_empty() {
            g = self.cv.wait_timeout(g, std::time::Duration::from_millis(200)).expect(POISONED).0;
        }
        if g.decided.is_none() {
            let mut ks: Vec<usize> = g.offered.keys().copied().collect();
            ks.sort_by(|&a, &b| {
                let (ca, cb) = (&g.offered[&a], &g.offered[&b]);
                if obj.better(ca, cb) {
                    std::cmp::Ordering::Less
                } else if obj.better(cb, ca) {
                    std::cmp::Ordering::Greater
                } else {
                    a.cmp(&b)
                }
            });
            ks.truncate(if cancel { 1 } else { g.quota });
            g.decided = Some(ks);
        }
        let d = g.decided.as_ref().expect("decided");
        if d.contains(&k) {
            return None;
        }
        let runs = d.iter().map(|j| (j + 1).to_string()).collect::<Vec<_>>().join(", ");
        Some(if cancel {
            format!("after Cancel / OK only one pick is drawn: run {runs}'s, predicted better")
        } else {
            format!("[drawing] candidates = {}: the pick of run {runs} is predicted better", g.quota)
        })
    }
}

/// Withdraws its run from the [`DrawGate`] when dropped (also while a panic unwinds the run's
/// thread, so the other runs never wait for it).
struct GateGuard<'g>(&'g DrawGate, usize);

impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        self.0.withdraw(self.1);
    }
}

#[derive(Clone, Default)]
struct RunState {
    /// cancel-fix: the run has begun its first cycle ([`Coord::claim_first`]).
    started: bool,
    cycle: usize,
    stable: bool,
    done: bool,
    gain: Option<f64>,
    /// Best so far: criteria, routed, total and the round (cycle) it came from.
    best: Option<RunBest>,
    /// The TopoR loop's pick so far, predicted (the plan's measure, nothing drawn; reported live
    /// per loop cycle, [`LiveLoop`]): *Best so far* while no run has drawn its pick (draw once:
    /// the drawing waits for every loop), so "Routes stable" has a pick to keep.
    predicted: Option<RunBest>,
}

/// The TopoR loop's live report to the progress after its initial plan and every cycle
/// ([`route_round`] -> [`Coord::report`]): its pick's predicted criteria, the connections in the
/// plan (every one is drawn, M-B1 never drop) of all, and whether it recorded stable.
#[derive(Clone, Copy, Debug)]
struct LiveLoop {
    pick: Criteria,
    planned: usize,
    total: usize,
    stable: bool,
}

/// The pick of a run's variant table so far ([`RunState::best`]).
#[derive(Clone, Copy, Debug)]
struct RunBest {
    crit: Criteria,
    routed: usize,
    total: usize,
    cycle: usize,
}

/// A round as the variant table judged it: the better of its variants (as drawn, after the via
/// reduction), with the pour stitching counted; `as_drawn`: that one is the variant as drawn.
#[derive(Clone, Copy, Debug)]
struct Judged {
    crit: Criteria,
    routed: usize,
    total: usize,
    as_drawn: bool,
}

/// How a round ended for the score display and the per-round log line ([`Coord::ended`]).
enum RoundEnd<'s> {
    Judged(Judged),
    /// Not judged, why (cut short, Cancel / OK came first, ...).
    NotJudged(&'s str),
}

impl<'a> Coord<'a> {
    fn new(ctl: &'a RunControl, n: usize, obj: Objective, candidates: usize) -> Coord<'a> {
        Coord { ctl, obj, st: std::sync::Mutex::new(vec![RunState::default(); n]), snap: std::sync::Mutex::new(None), gate: DrawGate::new(n, candidates, rayon::current_num_threads()), judge: std::sync::Mutex::new(None) }
    }

    /// TW3: the call's one variant table (`[optimize] loop = "topor"`). O(1).
    fn with_table(mut self, table: super::table::SharedTable<LoopVar>) -> Coord<'a> {
        self.gate.table = Some(std::sync::Arc::new(table));
        self
    }

    /// cancel-fix: note a run's pick with criteria `crit` (`complete`: of a cycle not cut short)
    /// as the call's best so far when nothing was noted yet or it beats the one noted (a complete
    /// one beats one cut short). After Cancel / OK the other runs read only whether there is one
    /// ([`Coord::has_result`]) and the TopoR loops whether a complete one is there. O(1).
    fn publish(&self, crit: Criteria, complete: bool) {
        let Ok(mut s) = self.snap.lock() else { return };
        if let Some((c, done)) = s.as_ref() {
            if (*done && !complete) || (*done == complete && !self.obj.better(&crit, c)) {
                return;
            }
        }
        *s = Some((crit, complete));
    }

    /// cancel-fix: may run `k` begin its first cycle? Not after Cancel / the time limit (`stopping`)
    /// when another run has begun one already: that run's result is what the call returns, and a
    /// first cycle cannot be cut short as cheaply as later ones (it is offered even when
    /// interrupted, and its stitching runs). Marks `k` as begun when it may. O(runs) under the lock.
    fn claim_first(&self, k: usize, stopping: bool) -> bool {
        let Ok(mut s) = self.st.lock() else { return true };
        if stopping && s.iter().enumerate().any(|(j, r)| j != k && r.started) {
            return false;
        }
        s[k].started = true;
        true
    }

    /// cancel-fix: some run has published a result ([`Coord::publish`]); `complete`: one of a
    /// cycle not cut short. O(1).
    /// cancel-fix: is a cut-short cycle with criteria `crit` (before its stitching) worth stitching
    /// and judging after Cancel / OK? Not when a complete result is published, nor when a cut-short
    /// cycle judged before (`judged`: its criteria before the stitching) is at least as good. O(1).
    fn worth_judging(&self, crit: &Criteria, judged: &Option<Criteria>) -> bool {
        let complete = self.snap.lock().is_ok_and(|s| s.as_ref().is_some_and(|(_, done)| *done));
        !complete && judged.as_ref().is_none_or(|j| self.obj.better(crit, j))
    }

    fn has_result(&self, complete: bool) -> bool {
        self.snap.lock().is_ok_and(|s| s.as_ref().is_some_and(|(_, done)| *done || !complete))
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

    /// The run whose pick is the call's best now: the best by the objective over the runs' picks,
    /// ties to the lower run (what [`route_topo`] picks at the end). O(runs).
    fn best_of(&self, s: &[RunState]) -> Option<usize> {
        self.best_by(s, |r| r.best.as_ref())
    }

    /// [`Coord::best_of`] over the picks `pick` gives (the drawn ones, or the predicted ones). O(runs).
    fn best_by(&self, s: &[RunState], pick: impl Fn(&RunState) -> Option<&RunBest>) -> Option<usize> {
        let mut bi: Option<(usize, &RunBest)> = None;
        for (i, r) in s.iter().enumerate() {
            if let Some(b) = pick(r) {
                if bi.is_none_or(|(_, c)| self.obj.better(&b.crit, &c.crit)) {
                    bi = Some((i, b));
                }
            }
        }
        bi.map(|(i, _)| i)
    }

    /// *Best so far* as the progress shows it: the best drawn pick, else (draw once: no run has
    /// drawn yet) the best of the TopoR loops' predicted picks. O(runs).
    fn shown_best(&self, s: &[RunState]) -> Option<(usize, RunBest)> {
        self.best_of(s).map(|i| (i, s[i].best.expect("best"))).or_else(|| self.best_by(s, |r| r.predicted.as_ref()).map(|i| (i, s[i].predicted.expect("predicted"))))
    }

    /// "Routes stable" ([`Progress::stable`], the dialog's Cancel turns into OK): every run stable
    /// or done, one of them stable, and a pick to keep (*Best so far* not empty, user 2026-11-17).
    /// Monotone: none of its terms goes back. O(runs).
    fn shows_stable(&self, s: &[RunState]) -> bool {
        s.iter().any(|r| r.stable) && s.iter().all(|r| r.stable || r.done) && self.shown_best(s).is_some()
    }

    /// Run `k` ended a cycle (or the run, `done`); publish the M-B0 progress over all runs.
    fn report(&self, k: usize, upd: impl FnOnce(&mut RunState)) {
        self.ended(k, upd, None);
    }

    /// [`Coord::report`], and for `round = Some((cycle, end))` run `k`'s round `cycle` ended: its
    /// score against the best goes to the progress (`last_*`) and its line to the round log
    /// ([`Progress::push_round_line`]). One lock for both, so the best it is compared with is the
    /// best before and after this very round. O(runs) + the line.
    fn ended(&self, k: usize, upd: impl FnOnce(&mut RunState), round: Option<(usize, RoundEnd)>) {
        let Ok(mut s) = self.st.lock() else { return };
        let before = self.best_of(&s).map(|i| (i, s[i].best.expect("best")));
        let was_stable = s[k].stable;
        upd(&mut s[k]);
        // A run's stable never goes back (the dialog's contract, ui_wx set_stable): the TopoR loop
        // reports it live, and the round's end after the drawing must not clear it before the
        // run's own end report, or an OK in between would not be "OK at Routes stable".
        s[k].stable |= was_stable;
        let n = s.len();
        let stable_n = s.iter().filter(|r| r.stable).count();
        let shows_stable = self.shows_stable(&s);
        let shown = self.shown_best(&s);
        let gain = s.iter().filter(|r| !r.done).map(|r| r.gain).try_fold(None::<f64>, |acc, g| g.map(|g| Some(acc.map_or(g, |a: f64| a.max(g)))));
        let bi = self.best_of(&s).map(|i| (i, s[i].best.expect("best")));
        let at = |i: usize, cycle: usize| format!("run {}, round {cycle}", i + 1);
        // The round against the best: (verdict, decider and delta, the best compared with).
        let round = round.map(|(cycle, end)| match end {
            RoundEnd::NotJudged(why) => (cycle, None, format!("{}: not judged ({why})", at(k, cycle))),
            RoundEnd::Judged(j) => {
                let is_best = bi.is_some_and(|(i, b)| i == k && b.cycle == cycle);
                let reference = if is_best { before } else { bi };
                let dec = reference.and_then(|(_, b)| self.obj.decider(&j.crit, &b.crit));
                let vs = if is_best && reference.is_some() && dec.is_none() {
                    "tie"
                } else if is_best {
                    "new best"
                } else if dec.is_none() {
                    "same"
                } else if dec.is_some_and(|(_, d)| d < 0.0) {
                    "better"
                } else {
                    "worse"
                };
                let c = &j.crit;
                let delta = |(name, d): (&str, f64)| match name {
                    "length" => format!("length {:+.3} m", d / 1000.0),
                    "score" => format!("weighted score {d:+.1} mm"),
                    "area" => format!("area {d:+.1} mm²"),
                    _ => format!("{name} {d:+.0}"),
                };
                let against = match (reference, dec) {
                    (None, _) => "new best (the first result)".to_string(),
                    (Some((i, b)), None) if is_best => format!("the best now: the same score as {}, and ties go to the lower run", at(i, b.cycle)),
                    (Some((i, b)), None) => format!("same score as the best ({}): the earlier one stays", at(i, b.cycle)),
                    (Some((i, b)), Some(d)) if is_best => format!("new best: {} against {}", delta(d), at(i, b.cycle)),
                    (Some((i, b)), Some(d)) if vs == "better" => format!("better than the best ({}): {}, but the variant table did not keep it", at(i, b.cycle), delta(d)),
                    (Some((i, b)), Some(d)) => format!("worse than the best ({}): {}", at(i, b.cycle), delta(d)),
                };
                let line = format!(
                    "{}: errors {}, open {}, vias {}, length {:.3} m, shrinkages {}; rule violations {}{}, routed {} of {}{}: {against}",
                    at(k, cycle),
                    c.errors,
                    c.open,
                    c.vias,
                    c.length_mm / 1000.0,
                    c.shrinkages,
                    c.violations,
                    if c.shorts > 0 { format!(" ({} connections with a short)", c.shorts) } else { String::new() },
                    j.routed,
                    j.total,
                    if j.as_drawn { " (as drawn, before the via reduction)" } else { "" },
                );
                (cycle, Some((j, vs, dec)), line)
            }
        });
        self.ctl.with_progress(|p| {
            p.runs = n as u32;
            p.runs_stable = stable_n as u32;
            p.runs_done = s.iter().filter(|r| r.done).count() as u32;
            p.stable = shows_stable;
            p.gain_pct = gain.flatten();
            if let Some((i, b)) = shown {
                p.best_routed = b.routed;
                p.best_total = b.total;
                p.best_length_mm = b.crit.length_mm;
                p.best_vias = b.crit.vias;
                p.best_run = i as u32 + 1;
                p.best_violations = b.crit.violations;
                p.best_errors = b.crit.errors;
                p.best_open = b.crit.open;
                p.best_shrinkages = b.crit.shrinkages;
                p.best_round = b.cycle as u32;
                // last-preview: drawn (a run's drawn pick) or predicted (a TopoR loop's pick)
                p.best_kind = if self.best_of(&s).is_some() { "copper" } else { "plan" }.into();
            }
            if let Some((cycle, judged, line)) = round {
                if let Some((j, vs, dec)) = judged {
                    p.last_run = k as u32 + 1;
                    p.last_round = cycle as u32;
                    p.last_errors = j.crit.errors;
                    p.last_open = j.crit.open;
                    p.last_violations = j.crit.violations;
                    p.last_vias = j.crit.vias;
                    p.last_length_mm = j.crit.length_mm;
                    p.last_shrinkages = j.crit.shrinkages;
                    p.last_routed = j.routed;
                    p.last_vs_best = vs.to_string();
                    p.last_decider = dec.map_or(String::new(), |d| d.0.to_string());
                    p.last_delta = dec.map_or(0.0, |d| d.1);
                    p.last_at_s = self.ctl.since_start_s();
                }
                p.push_round_line(line);
            }
        });
    }
}

/// last-preview: what a run's loop remembers for the progress dialog's sketches: its pick at the
/// last cycle's end (a change = its new best) and the mid states of the cycle running now that
/// may become its pick ([`Space::after_cycle`]).
#[derive(Default)]
struct SketchBook {
    pick: Option<Criteria>,
    mid: Vec<(Criteria, crate::control::Sketch)>,
}

/// last-preview: the judged variant's criteria, copper and violation places.
type JudgedSketch = (Criteria, Vec<Item>, Vec<[i64; 2]>);

/// last-preview: a sketch of `items` with `crit`'s numbers. O(1) + the move.
fn sketch_of(run: u32, cycle: usize, kind: &str, crit: &Criteria, items: Vec<Item>, errors_at: Vec<[i64; 2]>) -> crate::control::Sketch {
    crate::control::Sketch { run, cycle: cycle as u32, kind: kind.into(), errors: crit.errors, open: crit.open, violations: crit.violations, vias: crit.vias, length_mm: crit.length_mm, items, errors_at, ..Default::default() }
}

/// last-preview: the places of a drawn result's rule violations (the dialog's yellow dots); the
/// positions the drawing already reported, nothing computed. O(violations).
fn errors_at_of(r: &RouteResult) -> Vec<[i64; 2]> {
    places_of(&r.violations)
}

fn places_of(v: &[crate::route::ViolationReport]) -> Vec<[i64; 2]> {
    v.iter().map(|v| v.at).collect()
}

/// last-preview: loop cycle `cycle` of run `run` (1-based) ended at `at` s with its own state's
/// predicted criteria `c` (the Last round row while the loop runs). O(1).
fn note_loop_cycle(p: &mut crate::control::Progress, run: u32, cycle: usize, restarts: usize, at: f64, c: &Criteria) {
    if cycle > 0 {
        p.cycles_total += 1;
    }
    p.loop_restarts = restarts as u32;
    p.loop_run = run;
    p.loop_cycle = cycle as u32;
    p.loop_at_s = at;
    p.loop_errors = c.errors;
    p.loop_open = c.open;
    p.loop_violations = c.violations;
    p.loop_vias = c.vias;
    p.loop_length_mm = c.length_mm;
    p.loop_shrinkages = c.shrinkages;
}

/// last-preview: run `run`'s own pick changed (the Last run row): it came from cycle `cycle`,
/// `kind` "plan" (predicted) or "copper" (drawn). O(1).
fn note_run_pick(p: &mut crate::control::Progress, run: u32, cycle: usize, at: f64, kind: &str, c: &Criteria) {
    let k = (run as usize).saturating_sub(1);
    if p.pick_cycles.len() <= k {
        p.pick_cycles.resize(k + 1, 0);
    }
    p.pick_cycles[k] = cycle as u32;
    p.runbest_run = run;
    p.runbest_cycle = cycle as u32;
    p.runbest_at_s = at;
    p.runbest_kind = kind.into();
    p.runbest_errors = c.errors;
    p.runbest_open = c.open;
    p.runbest_violations = c.violations;
    p.runbest_vias = c.vias;
    p.runbest_length_mm = c.length_mm;
    p.runbest_shrinkages = c.shrinkages;
}

/// Seed of run `k` of a best-of-N call: run 0 keeps the configured seed.
fn run_seed(seed: u64, k: usize) -> u64 {
    if k == 0 {
        seed
    } else {
        seed.wrapping_add(0x9E37_79B9_7F4A_7C15u64.wrapping_mul(k as u64)).rotate_left(17) ^ 0xD1B5_4A32_D192_ED03
    }
}

/// A drawn result in a run's variant table, with the round (cycle) it came from.
#[derive(Clone, Debug)]
struct Drawn {
    cycle: usize,
    result: RouteResult,
}

/// One run of a best-of-N call while it iterates: its archive, schedule, stable meter, drawing
/// feedback and negotiation memory. A cycle ([`Run::step`]) depends only on this state, so the
/// run's result does not depend on how the cycles of the runs are interleaved on the threads.
struct Run {
    k: usize,
    seed: u64,
    arch: Archive<Drawn>,
    sched: super::schedule::Schedule,
    meter: super::schedule::Meter,
    /// Drawing feedback (plan S2c item 4): the rooms the drawing found too small, lowered by what
    /// was missing, in every later cycle.
    all: BTreeMap<(usize, usize), f64>,
    mem: Memory,
    notes: Vec<String>,
    time_hit: bool,
    stable_stop: bool,
    relay_stop: bool,
    /// time-limit-fix: see [`RunOut::share_stop`], [`RunOut::loop_cycles`], [`RunOut::loop_end_s`].
    share_stop: bool,
    loop_cycles: usize,
    loop_end_s: f64,
    draw_cut: bool,
    stable_at: Option<usize>,
    first: Option<usize>,
    cycles: usize,
    picked_at: usize,
    /// Router time of its cycles (s).
    busy_s: f64,
    done: bool,
    /// TW3: rows of the four bests this run still draws (draw-only rounds) and the ones it drew.
    queue: Vec<Assigned>,
    rows_drawn: Vec<(Assigned, Criteria, RouteResult)>,
    /// last-preview: the run's drawn pick after its last judged round (a change = its new best).
    drawn_pick: Option<Criteria>,
    /// TW3-R5: the loop's own counters lines of this run's first round ("TopoR loop judge", "TopoR
    /// loop escapes (TW5)"). A draw-only round runs no loop, so its result would carry empty counts;
    /// its drawn rows take these lines (the counts of this run's loop) instead.
    loop_lines: Vec<String>,
}

/// TW3-R5: the warnings that hold a loop's work counters.
fn is_loop_counts(w: &str) -> bool {
    w.starts_with("TopoR loop judge (") || w.starts_with("TopoR loop escapes (TW5)")
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
            stable_stop: false,
            relay_stop: false,
            share_stop: false,
            loop_cycles: 0,
            loop_end_s: 0.0,
            draw_cut: false,
            stable_at: None,
            first: None,
            cycles: 0,
            picked_at: 0,
            busy_s: 0.0,
            done: false,
            queue: Vec::new(),
            rows_drawn: Vec::new(),
            drawn_pick: None,
            loop_lines: Vec::new(),
        }
    }

    /// One whole-route cycle; sets `done` when the run ends (stable for a run that stops by
    /// itself, the safety cap, Cancel / OK, the time budget).
    #[allow(clippy::too_many_arguments)]
    fn step(&mut self, model: &BoardModel, opts: &Options, ctl: &RunControl, coord: &Coord, stubs: &PlaneStubs, fx: &Fixed, _t_call: Instant) {
        let t = Instant::now();
        let obj = coord.obj;
        let cycle = self.cycles + 1;
        // M2.TL: past the limit no new step begins (soft: the one running finished; hard: it was cut).
        let over_budget = ctl.limit_reached();
        // M2: a TopoR-loop run offers its pick or withdraws exactly once ([`DrawGate`]); the guard
        // withdraws it on every way out, also a panic.
        let topor = opts.opt_loop == "topor";
        let _gate = topor.then(|| GateGuard(&coord.gate, self.k));
        // C0: `[run] stop_after = "plan"` plans one cycle (the next would need the drawing's feedback).
        let plan_only = cycle > 1 && opts.stop_after == "plan";
        // TW3: a row of the four bests still to draw (draw-only round); Cancel / OK or the limit
        // end the run instead (the rows drawn so far stay).
        let draw_row = if topor && cycle > 1 && !self.queue.is_empty() && !(ctl.is_cancelled() || cancelled() || over_budget) { Some(self.queue.remove(0)) } else { None };
        if topor && cycle > 1 && draw_row.is_none() {
            // TW3: the rows not drawn (Cancel / OK, the limit) stay undrawn; the run's flags stay.
            self.queue.clear();
            self.done = true;
            return;
        }
        if draw_row.is_none() && (plan_only || cycle > opts.topo_rounds.max(1) || (cycle > 1 && (ctl.is_cancelled() || cancelled() || over_budget))) {
            self.time_hit = !plan_only && cycle <= opts.topo_rounds.max(1) && self.stable_at.is_none();
            self.done = true;
            return;
        }
        if cycle == 1 && !coord.claim_first(self.k, ctl.is_cancelled() || cancelled() || over_budget) {
            // Cancel / the time limit before this run began, while another run routes: no result of its own.
            self.notes.push("not begun".into());
            self.time_hit = true;
            self.done = true;
            return;
        }
        // The first cycle of the first run routes with the plain weights (the reference).
        let reference = self.k == 0 && cycle == 1;
        let width = if reference { 0.0 } else { self.sched.width };
        coord.begin(self.k, cycle, width);
        let mut o2 = opts.clone();
        if !opts.cut_report.is_empty() {
            o2.cut_report = format!("{}/run{}", opts.cut_report, self.k + 1);
        }
        o2.seed = self.seed;
        o2.fluctuation = width;
        o2.wall_clock_s = ctl.limit.remaining_s().map_or(0.0, |r| r.max(1.0));
        // 5.1 `[optimize] loop = "topor"`: one cycle per run: the initial routing once, the TopoR
        // loop on the topology, the picked variant drawn once. M2 (6.1): nothing of the drawing
        // feeds back into the plan on this path: no room hints from a drawing reach a plan (the
        // plan is decided on the topology, the drawing only realises it; Polubasov 2010,
        // introduction). `drawing_feedback` there only says whether the drawing redraws the
        // blockers of a trace it could not fit, inside that one drawing (M0: "auto" = off with
        // this loop; removing that rescue is M6).
        if topor {
            o2.initial = "once".into();
        }
        let hv: Hints = if o2.drawing_feedback && !topor { self.all.iter().map(|(&(e, l), &r)| (e, l, r)).collect() } else { Vec::new() };
        let k = self.k;
        // The TopoR loop's live report: its predicted pick is Best so far until a pick is drawn,
        // and its stable shows at once ("Report stable live", user 2026-11-17).
        let live = |l: &LiveLoop| {
            coord.report(k, |r| {
                r.predicted = Some(RunBest { crit: l.pick, routed: l.planned, total: l.total, cycle });
                r.stable |= l.stable;
            })
        };
        let fresh = Memory::default();
        let (mut ro, h2, mem2) = route_round(model, &o2, ctl, &hv, if draw_row.is_some() { &fresh } else { &self.mem }, cycle, stubs, fx, opts.fluctuation, (self.k, &coord.gate, &obj, &live), draw_row.as_ref());
        let drew = ro.drew.take();
        if cycle == 1 {
            // TW3-R5: the counters of this run's loop (taken before any early return below).
            self.loop_lines = ro.result.warnings.iter().filter(|w| is_loop_counts(w)).cloned().collect();
        }
        self.queue.extend(std::mem::take(&mut ro.extra));
        self.cycles = cycle;
        self.mem = mem2;
        for (e, l, r) in h2 {
            let v = self.all.entry((e, l)).or_insert(r);
            *v = v.min(r);
        }
        let interrupted = ro.result.timed_out;
        let loop_end = ro.lp;
        let routed = ro.result.routed;
        let (mut cyc_err, mut cyc_viol) = (ro.crit.errors, ro.crit.violations);
        let total = ro.result.connections_total;
        self.first.get_or_insert(routed);
        // An interrupted cycle is offered only when nothing else is there: what OK returns is the
        // best of the cycles that finished.
        // Errors of the best result before this cycle (for the soft cuts below).
        let best_before = self.arch.pick(&obj).map(|i| self.arch.items[i].crit.errors);
        // cancel-fix: after Cancel / OK a cycle is not stitched and judged any more when there is a
        // result already (this run's, or another run's): what OK / Cancel returns is the best of the
        // cycles that finished before; the stitching of a big board takes seconds.
        let judged_elsewhere = if interrupted { !self.arch.items.is_empty() || coord.has_result(false) } else { coord.has_result(true) };
        if ro.skipped.is_some() || ro.nothing.is_some() || ctl.is_cancelled() && judged_elsewhere {
            let why = if ro.skipped.is_some() {
                "its TopoR loop pick not drawn: another run's is"
            } else if ro.nothing.is_some() {
                "nothing kept to draw: Cancel / OK came before its initial routing ended"
            } else {
                "Cancel / OK came first"
            };
            self.notes.push(match (&ro.skipped, &ro.nothing) {
                (Some(w), _) => format!("{cycle}: its TopoR loop pick not drawn (another run's is: {w})"),
                (_, Some(w)) => format!("{cycle}: nothing kept to draw ({w})"),
                _ => format!("{cycle}: not judged (Cancel / OK came first)"),
            });
            if !self.queue.is_empty() && ro.skipped.is_some() {
                // TW3: its rows follow in draw-only rounds (the TOPO_TW3_DRAWONLY test hook)
                if let Some(e) = loop_end {
                    self.stable_at = self.stable_at.or(e.stable.then_some(cycle));
                    self.stable_stop = e.stopped_stable;
                    self.relay_stop = e.relay_stop;
                    self.time_hit = e.time_hit;
                    (self.share_stop, self.loop_cycles, self.loop_end_s) = (e.share_stop, e.cycles, e.at_s);
                }
                self.busy_s += t.elapsed().as_secs_f64();
                return;
            }
            if let Some(e) = loop_end.filter(|_| ro.skipped.is_some()) {
                // A run whose pick another run draws still ended its loop: stable or cut by the limit.
                if e.stable {
                    self.stable_at.get_or_insert(cycle);
                }
                (self.share_stop, self.loop_cycles, self.loop_end_s) = (e.share_stop, e.cycles, e.at_s);
                coord.report(self.k, |r| r.stable = e.stable);
            }
            self.time_hit = self.stable_at.is_none();
            self.done = true;
            self.busy_s += t.elapsed().as_secs_f64();
            coord.ended(self.k, |r| r.done = true, Some((cycle, RoundEnd::NotJudged(why))));
            return;
        }
        // cancel-fix: after Cancel / OK the cut-short cycles of the runs are stitched one at a time
        // (the lock is held up to publish), only one that beats those judged before it, and only
        // as routed (not also its "as drawn" variant: each stitching takes seconds on a big board).
        let cancel_cut = interrupted && ctl.is_cancelled() && self.arch.items.is_empty();
        let _judging = if cancel_cut {
            let mut g = coord.judge.lock().ok();
            if !g.as_ref().is_none_or(|j| coord.worth_judging(&ro.crit, j)) {
                self.notes.push(format!("{cycle}: cut short by Cancel / OK and not better than the cycle another run judged: not judged"));
                self.time_hit = self.stable_at.is_none();
                self.done = true;
                self.busy_s += t.elapsed().as_secs_f64();
                coord.ended(self.k, |r| r.done = true, Some((cycle, RoundEnd::NotJudged("cut short by Cancel / OK, not better than a round another run judged"))));
                return;
            }
            if let Some(j) = g.as_mut() {
                **j = Some(ro.crit);
            }
            g
        } else {
            None
        };
        // The round's score: the better of the variants it offered (`None`: none offered).
        let mut judged: Option<Judged> = None;
        let mut note_judged = |crit: Criteria, routed: usize, as_drawn: bool| {
            if judged.is_none_or(|j| obj.better(&crit, &j.crit)) {
                judged = Some(Judged { crit, routed, total, as_drawn });
            }
        };
        // last-preview: the copper of the variant the round is judged by (only while the caller
        // shows sketches: the last round's sketch).
        // The same choice as `note_judged`: the better of the variants by the objective.
        let judged_items: std::cell::RefCell<Option<JudgedSketch>> = std::cell::RefCell::new(None);
        let keep_items = |c: &Criteria, r: &RouteResult| {
            let mut ji = judged_items.borrow_mut();
            if ctl.sketches.enabled() && ji.as_ref().is_none_or(|(b, _, _)| obj.better(c, b)) {
                *ji = Some((*c, r.items.clone(), errors_at_of(r)));
            }
        };
        // The limit cut this cycle's pour stitching (set below; verify-sl3 F1).
        let mut cycle_stitch_cut = false;
        if !interrupted || self.arch.items.is_empty() {
            // Pour stitching (D4) on each variant before it is judged: a pour piece the copper cut
            // off that no stitch joins back is an open connection in KiCad's DRC, so it counts as an
            // error of the variant (else the pick prefers short copper that islands a pour).
            // M2.TL soft limit: the stitching belongs to the drawing of this variant and finishes
            // (Cancel still ends it); `hard` (or the soft limit past its overrun cap, F3, read
            // live): a limit that passes while the drawing or the stitching runs cuts it. A limit
            // that acted as hard before the drawing began does not cut it: the plan that exists is
            // drawn in full (user 2026-11-13, one path for `hard` and the cap). The decision is
            // the drawing's (verify-m2c 5): after a drawing the limit cut, the stitching is cut too
            // (before, it re-decided at its own start and so ran in full).
            let exempt = ro.draw_exempt;
            // Whether the limit cut a stitching of this cycle (verify-sl3 F1: the result is then
            // drawn only in part, never "stable", and the stitching did not run to its end).
            let stitch_cut = std::cell::Cell::new(false);
            let cut = || {
                let c = !exempt && ctl.limit.cuts_in(CutStep::Stitching);
                if c {
                    stitch_cut.set(true);
                }
                c
            };
            // The same test without noting a cut (the stitching may have ended before it): whether
            // a variant counts as cut short below; only the stitching's own stop test reports one.
            let past = || !exempt && ctl.limit.is_hard() && ctl.limit.passed();
            // OK at Routes stable (user 2026-11-17): a stop request while the progress showed
            // "Routes stable" is the user taking the stable result, so the stitching of the TopoR
            // loop's one drawing runs to its end as at a natural stable end (the time limit still
            // cuts it); a Cancel before stable still ends it after the pass in progress. What the
            // request was is latched at the request ([`RunControl::request_stop`]), never judged
            // here (verify-sl2 V1: after a Cancel the other runs end, and "every run stable or
            // done" turned true while this one stitched). The dialog disables its button after
            // the first press, so there is no second request to honour.
            let ok_at_stable = || topor && ctl.ok_at_stable();
            let stitched = |mut r: RouteResult, mut c: Criteria| -> (RouteResult, Criteria) {
                if opts.stitch_islands {
                    ctl.stage(cycle as u32, opts.topo_rounds.max(1) as u32, "pour stitching");
                    let cut_before = stitch_cut.replace(false);
                    c.errors += stitch_pours(model, opts, &mut r, None, &|| (ctl.is_cancelled() && !ok_at_stable()) || cut());
                    let this_cut = stitch_cut.get();
                    stitch_cut.set(cut_before || this_cut);
                    // Only a stitching the limit did not cut ran to its end (verify-sl3 F1).
                    if ok_at_stable() && !this_cut {
                        r.warnings.push("OK at Routes stable: the pour stitching ran to its end, as at a natural stable end (a Cancel before Routes stable ends it after the pass in progress)".into());
                    }
                }
                (r, c)
            };
            // A variant whose stitching the time budget cut short has its cut-off pour pieces
            // uncounted, so it would look better than it is: offered only when nothing else is
            // there, like an interrupted cycle (M-B2: measurement runs cut at 600 s picked such
            // variants and KiCad found the GND pieces open).
            let empty = self.arch.items.is_empty();
            let RoundOut { result, crit, pre, .. } = ro;
            // TW3-R5: a draw-only round ran no loop: its rows carry the counts of this run's loop.
            let lines = self.loop_lines.clone();
            let with_counts = |mut r: RouteResult| -> RouteResult {
                if cycle > 1 && !lines.is_empty() {
                    r.warnings.retain(|w| !is_loop_counts(w));
                    r.warnings.extend(lines.iter().cloned());
                }
                r
            };
            // TW3: the row's drawn value = the better of its drawn variants (each pour-stitched),
            // by the objective; TW2's further drawn variants are more sources of the same row.
            let mut row_best: Option<(Criteria, RouteResult)> = None;
            let mut note_row = |c: Criteria, r: &RouteResult| {
                if drew.is_some() && row_best.as_ref().is_none_or(|(b, _)| obj.better(&c, b)) {
                    row_best = Some((c, r.clone()));
                }
            };
            let tag = drew.as_ref().map(|d| format!(" (table row {}: {})", d.label, d.roles.iter().map(|r| r.name()).collect::<Vec<_>>().join(", "))).unwrap_or_default();
            if let Some((c, r)) = pre.filter(|_| !cancel_cut) {
                if std::env::var_os("TOPO_FUSEDBG").is_some() {
                    eprintln!("STITCH variant: round {cycle} as drawn");
                }
                let (r, c) = stitched(with_counts(r), c);
                if !past() || empty {
                    note_judged(c, r.routed, true);
                    keep_items(&c, &r);
                    note_row(c, &r);
                    self.arch.offer(Variant { label: format!("round {cycle} as drawn{tag}"), crit: c, payload: Drawn { cycle, result: r } }, &obj);
                }
            }
            if std::env::var_os("TOPO_FUSEDBG").is_some() {
                eprintln!("STITCH variant: round {cycle} (reduced + fused)");
            }
            let (r, c) = stitched(with_counts(result), crit);
            (cyc_err, cyc_viol) = (c.errors, c.violations);
            if !past() || self.arch.items.is_empty() {
                note_judged(c, r.routed, false);
                keep_items(&c, &r);
                note_row(c, &r);
                self.arch.offer(Variant { label: format!("round {cycle}{tag}"), crit: c, payload: Drawn { cycle, result: r } }, &obj);
            }
            if let (Some(d), Some((c, r))) = (drew.clone(), row_best) {
                self.rows_drawn.push((d, c, r));
            }
            cycle_stitch_cut = stitch_cut.get();
        }
        let pick = self.arch.pick(&obj).expect("a cycle was offered");
        let pc = self.arch.items[pick].crit;
        coord.publish(pc, !self.arch.items[pick].payload.result.timed_out);
        // last-preview: the drawn round's copper (the variant it is judged by) is the last round's
        // sketch, and the run's pick, when it changed, its best one (the Last run row); the copper
        // is copied only while the caller shows sketches.
        {
            let run = ctl.run.max(1);
            let at = ctl.since_start_s();
            let changed = self.drawn_pick.as_ref() != Some(&pc);
            if changed {
                self.drawn_pick = Some(pc);
                let bc = self.arch.items[pick].payload.cycle;
                ctl.with_progress(|p| note_run_pick(p, run, bc, at, "copper", &pc));
            }
            if ctl.sketches.enabled() {
                let items_of = |i: usize| self.arch.items[i].payload.result.items.clone();
                if let (Some(j), Some((c, items, at))) = (judged, judged_items.borrow_mut().take()) {
                    if c == j.crit {
                        ctl.publish_sketch(sketch_of(run, cycle, "copper", &j.crit, items, at), crate::control::SketchTo { last: true, best: false });
                    }
                }
                if changed {
                    let bc = self.arch.items[pick].payload.cycle;
                    ctl.publish_sketch(sketch_of(run, bc, "copper", &pc, items_of(pick), errors_at_of(&self.arch.items[pick].payload.result)), crate::control::SketchTo { last: false, best: true });
                }
            }
        }
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
        let gained = self.meter.push(super::schedule::Score { incomplete: pc.errors, shorts: pc.shorts, ranked: pc.ranked(), value: obj.value(&pc), scale: obj.base(&pc) });
        if gained {
            self.picked_at = cycle;
        }
        if self.meter.stable() && self.stable_at.is_none() {
            self.stable_at = Some(cycle);
        }
        let (best_routed, best_cycle) = (self.arch.items[pick].payload.result.routed, self.arch.items[pick].payload.cycle);
        let gain = self.meter.gain_pct();
        self.notes.push(format!(
            "{cycle}: w {width:.3}, routed {routed}, errors {cyc_err} ({cyc_viol} violations){}{}",
            if gained { " (new best)" } else { "" },
            gain.map(|g| format!(", gain {g:.2} %")).unwrap_or_default()
        ));
        let stable = self.stable_at.is_some();
        let end = match judged {
            Some(j) => RoundEnd::Judged(j),
            None if interrupted => RoundEnd::NotJudged("cut short by the time limit or Cancel / OK"),
            None => RoundEnd::NotJudged("its pour stitching was cut short by the time limit"),
        };
        coord.ended(
            self.k,
            |r| {
                r.stable = stable;
                r.gain = gain;
                r.best = Some(RunBest { crit: pc, routed: best_routed, total, cycle: best_cycle });
            },
            Some((cycle, end)),
        );
        self.busy_s += t.elapsed().as_secs_f64();
        super::prof::mark("round tail");
        if std::env::var_os("TOPO_TIMES").is_some() {
            eprintln!("[topo run {} cycle {cycle}] cycle: {:.3} s", self.k + 1, t.elapsed().as_secs_f64());
        }
        if topor && cycle > 1 {
            // TW3 draw-only round: the loop's flags are those of the first round.
            self.draw_cut |= interrupted || cycle_stitch_cut;
            self.time_hit |= interrupted;
            self.share_stop &= !interrupted;
            self.done = self.queue.is_empty();
            return;
        }
        if topor {
            // The loop's own end says whether the run was stable or the time limit cut it.
            if let Some(e) = loop_end {
                if e.stable {
                    self.stable_at.get_or_insert(cycle);
                }
                self.stable_stop = e.stopped_stable && !interrupted;
                self.relay_stop = e.relay_stop && !interrupted;
                self.time_hit = e.time_hit || interrupted;
                // a drawing the limit or a Cancel cut: the share is no longer what ended the run
                (self.share_stop, self.loop_cycles, self.loop_end_s) = (e.share_stop && !interrupted, e.cycles, e.at_s);
            } else {
                self.time_hit = interrupted;
            }
            // The loop's one drawing (draw once): the limit cut its traces / via reduction
            // (`interrupted`; a Cancel cuts nothing of it, `draw_through`) or its pour stitching.
            self.draw_cut = interrupted || cycle_stitch_cut;
            self.done = self.queue.is_empty();
            coord.report(self.k, |r| r.stable = loop_end.is_some_and(|e| e.stable));
            return;
        }
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
                self.stable_stop = true;
            }
        }
    }

    /// The run's result: its archive's pick, with the variant table and its cycle line; for a run
    /// that Cancel / the time limit ended before it had a result (cancel-fix), why.
    fn finish(mut self, opts: &Options, coord: &Coord) -> Result<RunOut, (usize, String)> {
        let obj = coord.obj;
        let Some(pick) = self.arch.pick(&obj) else {
            coord.report(self.k, |r| r.done = true);
            return Err((self.k, self.notes.join("; ")));
        };
        let crit = self.arch.items[pick].crit;
        let mut best = self.arch.items[pick].payload.result.clone();
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
        Ok(RunOut { k: self.k, rows_drawn: std::mem::take(&mut self.rows_drawn), result: best, crit, time_hit: self.time_hit, stable_stop: self.stable_stop, relay_stop: self.relay_stop, share_stop: self.share_stop, loop_cycles: if opts.opt_loop == "topor" { self.loop_cycles } else { self.cycles }, loop_end_s: self.loop_end_s, draw_cut: self.draw_cut, stable: self.stable_at.is_some(), line, cycles_line })
    }
}

/// The runs' cycles on `workers` threads: a free thread takes the run with the fewest cycles
/// that no other thread is stepping (so every run progresses, also when there are more runs than
/// threads, and "Routes stable" can come from all of them). The threads are the engine's share
/// for the runs (`min(runs, rayon threads)`; one per run with the TopoR loop), the checks inside a
/// cycle use the rayon pool.
#[allow(clippy::too_many_arguments)]
fn run_all(model: &BoardModel, opts: &Options, ctl: &RunControl, coord: &Coord, stubs: &PlaneStubs, fx: &Fixed, t_call: Instant, n: usize) -> (Vec<RunOut>, Vec<(usize, String)>) {
    use std::sync::Mutex;
    let topor = opts.opt_loop == "topor";
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
        // M2.TL: with the soft limit only Cancel / OK stops the moves inside a step (M0's stop checks)
        let _slot = topor.then(|| SlotGuard::take(&coord.gate.slots));
        // par-refine: the refinements of this step take their lanes from the free slots (TopoR
        // loop; whole-route cycles: none) or the fixed `[optimize] refine_lanes`.
        let pool: Option<std::sync::Arc<dyn super::refine_par::lanes::Pool>> = topor.then(|| coord.gate.slots.clone() as std::sync::Arc<dyn super::refine_par::lanes::Pool>);
        let mode = super::refine_par::lanes::mode_of(opts.refine_lanes);
        crate::control::with_control_mode(&c, opts.timeout_mode == "hard", || super::refine_par::lanes::with(mode, pool, || super::refine_par::lanes::with_window(opts.refine_test_window, || r.step(model, opts, &c, coord, stubs, fx, t_call))));
        c.idle(); // between its cycles or done: the soft limit's notice no longer names its step (F6)
        let st = (r.cycles, r.done);
        drop(r);
        if let (Ok(mut tk), Ok(mut cy)) = (taken.lock(), cycles.lock()) {
            tk[k] = false;
            cy[k] = st;
        }
    };
    // M2: with the TopoR loop every run has a thread of its own: a run waits after its loop until
    // every run offered its pick ([`DrawGate`]), so the decision sees all of them and never waits
    // for a run that has no thread. Only `min(runs, rayon threads)` of them compute at once (the
    // gate's slots; a run waiting at the gate gives its slot back, and a loop run gives its slot at
    // each cycle boundary to a waiting run that is behind it, [`Slots::turn`]): the busy threads
    // stay as before.
    let workers = if topor { n } else { n.min(rayon::current_num_threads()).max(1) };
    if workers == 1 {
        worker();
    } else {
        std::thread::scope(|s| {
            for _ in 0..workers {
                s.spawn(worker);
            }
        });
    }
    let (mut outs, mut none) = (Vec::new(), Vec::new());
    for m in runs {
        match m.into_inner().expect("run state").finish(opts, coord) {
            Ok(o) => outs.push(o),
            Err(e) => none.push(e),
        }
    }
    (outs, none)
}

/// Pour stitching (D4, [`super::stitch`]) on the picked result: the stitches join its items
/// (`item_conn` = `usize::MAX`: they belong to no connection) and a summary line plus one line
/// per cut-off piece go to the warnings. Returns the pieces still cut off afterwards. `stop`
/// (Cancel) ends the stitching after the pass in progress ([`super::stitch::stitch_until`]).
fn stitch_pours(model: &BoardModel, opts: &Options, best: &mut RouteResult, deadline: Option<Instant>, stop: &dyn Fn() -> bool) -> usize {
    let pour_nets: BTreeSet<i32> = model.pours.iter().map(|z| z.net).collect();
    if pour_nets.is_empty() {
        return 0;
    }
    let routed_pads = super::stitch::routed_pour_pads(model, &best.connections);
    let (reg, _) = Registry::from_config(standard_factors(), &opts.factors);
    let t = Instant::now();
    let s = super::stitch::stitch_until(model, opts, &reg, &best.items, &routed_pads, deadline, stop);
    if s.before == 0 {
        return 0;
    }
    best.warnings.push(format!(
        "pour stitching: {} pour pieces cut off by the routed copper before, {} after; {} island vias, {} pad stubs, {} island joins, {} dog-bones ({:.2} s); these vias come after routing, the plan has no price for the pour pieces its copper cuts off (vias, plan vs drawn: class \"pour stitching\")",
        s.before,
        s.after,
        s.island_vias,
        s.pad_stubs,
        s.island_joins,
        s.dogbones,
        t.elapsed().as_secs_f64()
    ));
    best.warnings.extend(s.notes);
    for it in s.items {
        best.items.push(it);
        best.item_conn.push(usize::MAX);
    }
    s.after
}

/// stitch-vias region stitching ([`super::stitch::stitch_regions`]) on the picked result: its vias
/// join the items (`item_conn` = `usize::MAX`) and its lines go to the warnings. `stop` (Cancel)
/// ends it between pieces.
fn stitch_regions_on(model: &BoardModel, opts: &Options, best: &mut RouteResult, stop: &dyn Fn() -> bool) {
    if model.pours.is_empty() {
        return;
    }
    let routed_pads = super::stitch::routed_pour_pads(model, &best.connections);
    let (reg, _) = Registry::from_config(standard_factors(), &opts.factors);
    let t = Instant::now();
    let s = super::stitch::stitch_regions(model, opts, &reg, &best.items, &routed_pads, None, stop);
    if s.notes.is_empty() {
        return;
    }
    best.warnings.push(format!("{} ({:.2} s)", s.notes[0], t.elapsed().as_secs_f64()));
    best.warnings.extend(s.notes.into_iter().skip(1));
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
    /// `[optimize] loop = "topor"`: how the loop ended.
    lp: Option<LoopEnd>,
    /// M2 draw once ([`DrawGate`]): this run's TopoR loop pick is not drawn (another run's is), why;
    /// no result of this run.
    skipped: Option<String>,
    /// M2.TL: nothing kept to draw (Cancel / OK came before the run's initial routing ended), why.
    nothing: Option<String>,
    /// The limit acted as hard before the drawing began (`draw_exempt` in [`route_round`]): it
    /// cut a planning step, and the drawing of the kept plan is not cut, its pour stitching
    /// neither (verify-m2c 5: the stitching carries the drawing's decision, not its own).
    draw_exempt: bool,
    /// TW3: the row of the shared table this round drew, and the rows of the four bests this run
    /// draws next in draw-only rounds.
    drew: Option<Assigned>,
    extra: Vec<Assigned>,
}

/// One topological variant of the TopoR loop: the routed state, the wire -> connection map, the
/// layering, the connections placed without a legal path, the reroute counters and the predicted
/// violations of the layering; TW3: what its run decided before the loop ([`PreLoop`]).
#[derive(Clone)]
struct LoopVar {
    topo: super::state::Topology,
    wire_conn: Vec<usize>,
    lr: layers::LayerReport,
    forced: BTreeSet<usize>,
    reroutes: Vec<u32>,
    pred: super::optimise::Predicted,
    /// loop-local: `pred` is a sum of local changes (`Sp::resync`).
    pred_local: bool,
    pre: std::sync::Arc<PreLoop>,
}

/// TW3: what a run decided before its TopoR loop that the drawing of its variants reads: the
/// connections failed before the loop (index into the outcomes, reason), the terminals the P4
/// moves moved (`term_over`) and every connection's straight pad-to-pad distance. A row of the
/// shared table is restored or drawn only by a run with the same key ([`PreLoop::key`]); a
/// draw-only round ([`route_round`] with a row) takes them from the row.
#[derive(Clone, Debug, Default)]
struct PreLoop {
    failed: Vec<(usize, String)>,
    term_over: BTreeMap<usize, (TermRef, TermRef)>,
    ghost: Vec<f64>,
}

impl PreLoop {
    /// A hash of everything in it (equal pre-loop decisions, equal keys). O(size).
    fn key(&self) -> u64 {
        let mut h: u64 = 0xC0FF_EE00_1234_5678;
        let mut add = |x: u64| {
            h ^= x.wrapping_add(0x9E37_79B9_7F4A_7C15).wrapping_add(h << 6).wrapping_add(h >> 2);
        };
        for (i, r) in &self.failed {
            add(*i as u64);
            r.bytes().for_each(|b| add(u64::from(b)));
        }
        for (i, (f, t)) in &self.term_over {
            add(*i as u64);
            format!("{f:?}{t:?}").bytes().for_each(|b| add(u64::from(b)));
        }
        for g in &self.ghost {
            add(g.to_bits());
        }
        h
    }
}

/// TW3: a row of the shared table one run draws: the roles it holds (its four bests), its label
/// and predicted criteria, and the row.
#[derive(Clone)]
struct Assigned {
    roles: Vec<super::archive::Role>,
    label: String,
    crit: Criteria,
    row: super::table::Row<LoopVar>,
}

/// How the TopoR loop of a round ended (`[optimize] loop = "topor"`).
#[derive(Clone, Copy, Debug, Default)]
struct LoopEnd {
    stable: bool,
    /// The loop stopped at stable (not at the cycle cap with stable recorded).
    stopped_stable: bool,
    time_hit: bool,
    /// TW3 D2: the loop stopped at its re-lay budget (`[optimize] loop_relays`).
    relay_stop: bool,
    /// time-limit-fix: the loop's own time share (`[optimize] loop_time_share`) ended it, the
    /// time limit had not passed then (`time_hit` is set too: a time bound ended the search).
    share_stop: bool,
    /// The cycles the loop ran (the result's `iterations` when its run is the pick).
    cycles: usize,
    /// When the loop ended: seconds since the limit's start (the engine job's clock).
    at_s: f64,
}

impl RoundOut {
    fn unrouted(r: RouteResult) -> RoundOut {
        RoundOut { crit: Criteria { errors: r.failed, open: r.failed, ..Criteria::default() }, result: r, pre: None, lp: None, skipped: None, nothing: None, draw_exempt: false, drew: None, extra: Vec::new() }
    }

    /// A round that draws nothing: `skipped` (another run's pick is drawn) or `nothing` (no plan
    /// kept), with the loop's end. O(connections).
    fn not_drawn(model: &BoardModel, opts: &Options, skipped: Option<String>, nothing: Option<String>, lp: Option<LoopEnd>) -> RoundOut {
        let why = skipped.clone().or_else(|| nothing.clone()).unwrap_or_default();
        RoundOut { lp, skipped, nothing, ..RoundOut::unrouted(unrouted(model, opts, &why)) }
    }
}

/// The fuse pass's report line (M6 F-3) from the fuses applied, each `(price gain, length gain
/// mm)` as [`draw::fuse_candidates`] gives them: the length the moved ends saved in mm, and apart
/// from it the drop of the `junction_angle` price and, in the octilinear style, of the DP's style
/// price (bends, jogs, `any_angle`; octi-bundle r2) (price gain minus length gain; a price: it
/// grows without bound for a branch that leaves its trunk tangentially, so it is never added to a
/// length). O(fuses).
pub fn fuse_line(fused: &[(f64, f64)]) -> String {
    let n_fused = fused.len();
    let len_gain_mm: f64 = fused.iter().map(|f| f.1).sum();
    let angle_gain: f64 = fused.iter().map(|f| f.0 - f.1).sum();
    format!(
        "fuse after drawing: {n_fused} trace ends moved onto their trunk's copper where the two part: {:.1} mm {} in length, junction-angle and style price ([factors] junction_angle, any_angle; [drawing] bend_mm) {} by {:.1}",
        len_gain_mm.abs(),
        if len_gain_mm >= 0.0 { "saved" } else { "added" },
        if angle_gain >= 0.0 { "lower" } else { "higher" },
        angle_gain.abs()
    )
}

/// The split guard's report line (fuse-octi r4, H1 part 2), `None` when it had nothing to do:
/// `fuse` fuse and `polish` polish steps not taken because they would have split copper of a net
/// that was joined, and `split` the nets whose copper joined before the fuse pass ends apart
/// (an engine bug: some path the guard does not see, e.g. the fuse pour check's undo). O(nets).
pub fn split_line(fuse: usize, polish: usize, split: &[String]) -> Option<String> {
    if fuse == 0 && polish == 0 && split.is_empty() {
        return None;
    }
    let mut l = format!("split guard after drawing: {fuse} fuse and {polish} polish steps not taken (each would have split copper of a net that was joined or left a new dangling end: a dangling branch)");
    if !split.is_empty() {
        l.push_str(&format!("; engine bug: {} with copper apart or a new dangling end after the steps after the drawing: {}", if split.len() == 1 { "1 net" } else { "nets" }, split.join(", ")));
    }
    Some(l)
}

/// first-draw-45 P4: most passes of the straightening over the draw order (a work bound; a pass
/// that changes nothing ends it earlier, at a fixpoint). r7: the qmtech USB corner reaches its
/// fixpoint in the 3rd pass (`straightening_reaches_its_fixpoint`); the boards take 3 (esp,
/// wiccon) to 4 (qmtech, at the bound) passes.
const STRAIGHTEN_PASSES: usize = 4;

/// first-draw-45 P4: the straightening pass's report line: `n` stretches of two or more bends
/// redrawn as one straight, elbow or Z form, the length that changed (mm, + added), the steps the
/// split guard refused, the passes and whether the last one changed nothing (`fixpoint`; else the
/// loop stopped at [`STRAIGHTEN_PASSES`]), the time (s).
pub fn straighten_line(n: usize, added_mm: f64, refused: usize, passes: usize, fixpoint: bool, secs: f64) -> String {
    format!(
        "octilinear straightening after drawing (draw order, {passes} passes, {}): {n} stretches of two or more bends redrawn as one straight, elbow or Z form, {:.1} mm {} in length; {refused} refused by the split guard ({secs:.1} s)",
        if fixpoint { "the last without a change".to_string() } else { format!("stopped at the bound of {STRAIGHTEN_PASSES} passes with changes in the last") },
        added_mm.abs(),
        if added_mm > 0.0 { "added" } else { "saved" }
    )
}

/// The octilinear polish's report line: `n` segments redrawn as an elbow or Z, which changed the
/// copper's length by `added_mm` (an elbow is never shorter than the straight segment it
/// replaces: after many fuses the polish gives back part of the length the fuse line reports
/// saved, e.g. 3.0 of 8.5 mm on the esp USB crop with 48 fuses, hotfix-main 2026-11-18). O(1).
pub fn polish_line(n: usize, added_mm: f64) -> String {
    format!(
        "octilinear polish after drawing: {n} segments not at 0/45/90 degrees (fused branch ends) redrawn as an elbow or Z: {:.1} mm {} in length",
        added_mm.abs(),
        if added_mm >= 0.0 { "added" } else { "saved" }
    )
}

/// Start of the line (`[optimize] loop = "cycles"` only) that says what a whole-route cycle hands
/// the next one: room hints, connections with their own history, edges with history (M6 R2: the
/// TopoR loop computes none of it).
pub const CYCLES_MEMORY: &str = "cycles memory for the next cycle:";

/// Warning when `drawing_feedback` is set with the TopoR loop, where it has no effect (M6, 6b).
pub const FEEDBACK_IGNORED: &str = "[search] drawing_feedback is ignored with [optimize] loop = \"topor\": the picked plan is drawn once, no blocker is ripped and redrawn (only loop = \"cycles\" redraws them; M6, 2026-11-18)";

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
    /// `[optimize] initial = "once"` (4.3): the plan of the cycle before (its topology before the
    /// drawing), which the next cycle starts from instead of routing again. Never fades.
    plan: Option<PlanCarry>,
}

/// The plan one cycle hands to the next with `[optimize] initial = "once"` (4.3): the topology
/// (wires, their passes in edge order), the wire -> connection map, the connections' pad pairs
/// (the P4 moves may attach a connection to another pad pair of its tree) and the terminals the
/// report gives such a connection.
#[derive(Clone, Debug)]
struct PlanCarry {
    topo: super::state::Topology,
    wire_conn: Vec<usize>,
    conns: Vec<Conn>,
    term_over: BTreeMap<usize, (TermRef, TermRef)>,
}

/// Weight of a pass at distance `d` from a violation whose stretch has the radius `r` (nm):
/// 1 on the stretch, falling smoothly with the distance (no cutoff; M-B2 local history).
fn near_weight(d: f64, r: f64) -> f64 {
    let x = d / r.max(1.0);
    1.0 / (1.0 + x * x)
}

/// The P4 score of a plan: (objective, predicted violations, vias, graded over-full price of the
/// cut model, 0 for the per-edge model; predicted shorts among the violations: crossings no layer
/// separates and traversals through foreign copper, 0 with `[optimize] pick_shorts` off).
type Score = (f64, usize, usize, f64, usize);

/// The projection edges of the passes nearest to cut items `(wire, traversal)` (where a wire
/// crosses or wraps the cut), ascending.
fn item_edges(st: &State, items: &[(WireId, usize)]) -> Vec<usize> {
    let mut v: Vec<usize> = items
        .iter()
        .filter_map(|&(w, k)| {
            let wi = st.wires.get(w).filter(|wi| wi.alive && !wi.passes.is_empty())?;
            Some(st.passes[wi.passes[k.min(wi.passes.len() - 1)]].edge)
        })
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// TW6 r4 (`[optimize] site_move_detour`): the graded detour price (in violations) of the
/// connections `cis`: per connection the length of its alive wires ([`State::wire_len`]) over its
/// straight pad-to-pad distance, the run report's detour ratio
/// ([`super::escape::detour_price`] on the excess-ratio scale `scale`; 0 = no price). O(wires).
pub fn conn_detour_price(st: &State, wire_conn: &[usize], conns: &[Conn], cis: &BTreeSet<usize>, scale: f64) -> f64 {
    if scale <= 0.0 || cis.is_empty() {
        return 0.0;
    }
    let mut len: BTreeMap<usize, f64> = cis.iter().filter(|&&c| c < conns.len()).map(|&c| (c, 0.0)).collect();
    for (w, _) in st.wires.iter().enumerate().filter(|(_, wi)| wi.alive) {
        if let Some(l) = wire_conn.get(w).and_then(|c| len.get_mut(c)) {
            *l += st.wire_len(w);
        }
    }
    let pos = &st.proj.pos;
    len.iter().map(|(&c, &l)| super::escape::detour_price(l, (pos[conns[c].b] - pos[conns[c].a]).len(), scale)).sum()
}

/// TW6 r4: the keep judgement of a site or branch relay ([`conn_detour_price`]): the board's
/// evaluation `e0` -> `e1` plus the relaid connections' detour price `det_old` -> `det_new` (mm)
/// must drop. Returns (kept, refused for the detour: the evaluation alone would have kept it). A
/// price, not a gate: a move that shortens a detour gains by as much. O(1).
pub fn site_keep(e0: f64, e1: f64, det_old: f64, det_new: f64) -> (bool, bool) {
    let kept = e1 + det_new < e0 + det_old - 1e-6;
    (kept, !kept && e1 < e0 - 1e-6)
}

/// TW6 r4: whether a reroute not kept by its judgement (`old` / `new` prices with the detour
/// prices `det_old` / `det_new` in them, `extra` the re-lay counter's price; `new` infinite = not
/// placed) would have been kept without the detour price. O(1).
pub fn reroute_detour_refused(kept: bool, old: f64, new: f64, det_old: f64, det_new: f64, extra: f64) -> bool {
    !kept && new.is_finite() && (new - det_new) + extra < (old - det_old) - 1e-6
}

/// TW6 part E: the run report's line for `[optimize] site_move` / `branch_move` (`site`, `branch`:
/// which is on; `detour`: `[optimize] site_move_detour`, 0 = no detour price) with the loop's
/// counts `st`. O(1).
pub fn site_move_line(site: bool, branch: bool, detour: f64, st: &super::optimise::CycleStats) -> String {
    let mut parts = Vec::new();
    if site {
        parts.push(format!("{} of {} site moves kept (a trace with a via without room laid again, else the traces closing its stretch in; [optimize] site_move)", st.site_kept, st.site_moved));
    }
    if branch {
        parts.push(format!("{} of {} branch moves kept (the branch of a junction at such a stretch laid again; [optimize] branch_move)", st.branch_kept, st.branch_moved));
    }
    if detour > 0.0 {
        parts.push(format!("{} refused for their detour ([optimize] site_move_detour = {detour})", st.site_detour_refused));
    } else {
        parts.push("no detour price ([optimize] site_move_detour = 0)".to_string());
    }
    format!("TopoR loop, vias without room (TopoR Shift Vias with rerouting): {}", parts.join("; "))
}

/// The victims of one negotiated P4 rip-up round (the pre-loop rounds and TW5 (f) `[optimize]
/// loop_p4` inside the loop, one choice for both), read from the capacity model `cm` of the present
/// layering: the wires with a via without a site, a bad via cut's wire and its crosser routed last,
/// `pre` (the caller's own, e.g. the first pre-loop round's via-rose wires), then per over-full
/// edge-layer (per-edge model) or cut (cut model) its wires routed last first until the rest fits.
/// `None` when nothing is over-full (no over-full item at all, `pre` aside).
/// Each victim carries a graded weight, the sum over the items that chose it of `(1 + d) / (2 + d)`
/// with `d` the item's over-fullness in default pitches `ws0` (cuts: their missing room; edge-layers:
/// their graded depth; a via without a site or a bad via cut: 0, so 1/2; `pre`: 1/2), so the
/// in-loop round can keep the heaviest under its safety cap. `history` (the pre-loop rounds): +1 on
/// the bad items' edges as before TW5 (f). O(items x wires on them).
fn p4_victims(st: &State, cm: &CapModel, rank: &[usize], wire_conn: &[usize], pre: &BTreeSet<WireId>, ws0: f64, mut history: Option<&mut Vec<f64>>) -> Option<BTreeMap<WireId, f64>> {
    let g = |d: f64| { let d = if d.is_finite() { d.max(0.0) } else { 0.0 }; (1.0 + d) / (2.0 + d) };
    // Cut model: the over-full cuts (most over-full first) and the projection edges where
    // their wires cross them; per-edge model: the over-full edge-layers.
    let bad_cuts: Vec<(usize, usize, f64)> = cm.cut.as_ref().map(|c| c.overfull_list()).unwrap_or_default();
    let bad = if cm.cut.is_some() { Vec::new() } else { cm.overfull_list() };
    let nosite = cm.nosite_wires();
    // CM1: vias whose gap / via-to-via cuts are over-full, with the wires crossing them.
    let bad_vias = cm.via_cut_list();
    if bad.is_empty() && bad_cuts.is_empty() && nosite.is_empty() && bad_vias.is_empty() {
        return None;
    }
    if let Some(history) = history.as_deref_mut() {
        let mut bad_edges: Vec<usize> = bad.iter().map(|x| x.0).collect();
        if let Some(c) = cm.cut.as_ref() {
            for &(l, ci, _) in &bad_cuts {
                bad_edges.extend(item_edges(st, &c.items_at(l, ci)));
            }
            bad_edges.sort_unstable();
        }
        bad_edges.dedup();
        for &e in &bad_edges {
            history[e] += 1.0;
        }
    }
    let mut wt: BTreeMap<WireId, f64> = BTreeMap::new();
    let add = |wt: &mut BTreeMap<WireId, f64>, w: WireId, x: f64| *wt.entry(w).or_insert(0.0) += x;
    // A via without a site in its interval: its wire is rerouted, with history on the edges
    // that bound that interval only (M-B2; before: all edges of the wire).
    for &w in &nosite {
        add(&mut wt, w, g(0.0));
        let Some(history) = history.as_deref_mut() else { continue };
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
    // CM1: an over-full via cut rips the via's wire and, routed last first, one crosser
    // (the cut's room is the via's neighbourhood; the next round prices the rest again),
    // with history on the edges of the via's traversals.
    for (w, xs) in &bad_vias {
        add(&mut wt, *w, g(0.0));
        if let Some(history) = history.as_deref_mut() {
            for &k in &cm.via_k[*w] {
                let ps = &st.wires[*w].passes;
                for j in [k.checked_sub(1), Some(k)].into_iter().flatten().filter(|&j| j < ps.len()) {
                    history[st.passes[ps[j]].edge] += 1.0;
                }
            }
        }
        if let Some(&x) = xs.iter().filter(|&&x| st.wires[x].alive && wire_conn.get(x).is_some_and(|&c| c != usize::MAX)).max_by_key(|&&x| (rank[wire_conn[x]], x)) {
            add(&mut wt, x, g(0.0));
        }
    }
    for &w in pre {
        add(&mut wt, w, g(0.0));
    }
    // Per over-full edge-layer, rip the wires routed last until the rest fits.
    if !bad.is_empty() {
        let depth: BTreeMap<usize, f64> = cm.overfull_depths().into_iter().fold(BTreeMap::new(), |mut m, (e, d)| {
            let x = m.entry(e).or_insert(0.0f64);
            *x = x.max(d);
            m
        });
        for &(e, l) in &bad {
            let mut on: Vec<usize> = st.edge_order[e].iter().copied().filter(|&p| cm.pass_layer[p] == l as i32).collect();
            on.sort_by_key(|&p| std::cmp::Reverse(rank[wire_conn[st.passes[p].wire]]));
            let mut gone: BTreeSet<WireId> = wt.keys().copied().collect();
            let d = g(depth.get(&e).copied().unwrap_or(0.0));
            for p in on {
                if cm.fits_without(e, l, &gone) {
                    break;
                }
                let w = st.passes[p].wire;
                gone.insert(w);
                add(&mut wt, w, d);
            }
        }
    }
    // Cut model: per over-full cut, its wires routed last first until the rest fits.
    if let Some(c) = cm.cut.as_ref() {
        for &(l, ci, slack) in &bad_cuts {
            let mut on: Vec<WireId> = c.items_at(l, ci).into_iter().map(|x| x.0).filter(|&w| st.wires[w].alive && wire_conn.get(w).is_some_and(|&x| x != usize::MAX)).collect();
            on.sort_unstable();
            on.dedup();
            on.sort_by_key(|&w| (std::cmp::Reverse(rank[wire_conn[w]]), w));
            let mut gone: BTreeSet<WireId> = wt.keys().copied().collect();
            let d = g(-slack / ws0.max(1.0));
            for w in on {
                if c.fits_without(l, ci, &gone) {
                    break;
                }
                gone.insert(w);
                add(&mut wt, w, d);
            }
        }
    }
    Some(wt)
}

/// TW5 (f): the in-loop P4 round's victims `(weight, rank, wire)` under its safety cap `cap`: the
/// heaviest first (the graded over-fullness of the items that chose them, [`p4_victims`]), among
/// equal weights the wire routed last first, then by id (deterministic). The cap only bounds a
/// round's work; below it every victim is ripped. O(n log n).
fn p4_keep_heaviest(mut v: Vec<(f64, usize, WireId)>, cap: usize) -> Vec<WireId> {
    v.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    v.truncate(cap);
    v.into_iter().map(|x| x.2).collect()
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
/// The engine's reading of a board without routing (the plugin's fail-fast probe, run right
/// after the board is read): the checks that would fail every connection of a route, i.e. the
/// signal layer count and the projection ([`board::build`] with the options a route uses).
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Probe {
    pub ok: bool,
    /// Why a route of this board would fail every connection (the places are in `places`).
    pub error: Option<String>,
    /// The copper where the projection failed, in words (KiCad frame, mm): the constraint the
    /// triangulation failed on, then the nearest other copper.
    pub places: Vec<String>,
    /// Report lines of the build (a triangulation that needed the noded path, constraints left
    /// out).
    pub notes: Vec<String>,
    /// How the triangulation was built (`direct`, `noded`, ...); empty when it was not built.
    pub path: String,
    pub vertices: usize,
    pub obstacles: usize,
    pub seconds: f64,
}

/// See [`Probe`]. O(projection build): seconds on the largest benchmark boards.
pub fn probe(model: &BoardModel, opts: &Options) -> Probe {
    let t0 = Instant::now();
    let mut out = Probe::default();
    let signal = model.layers.iter().filter(|l| l.routable).count();
    if signal < 2 {
        out.error = Some(format!("the topological core routes boards with two or more signal layers; this board has {signal}"));
    } else {
        match board::build_checked(model, &build_opts(opts)) {
            Ok(b) => {
                out.ok = true;
                out.notes = b.notes;
                out.path = b.proj.cdt.path.name().to_string();
                out.vertices = b.proj.pos.len();
                out.obstacles = b.proj.obstacles.len();
            }
            Err(f) => {
                out.error = Some(format!("projection failed: {}", f.error));
                out.places = f.places;
            }
        }
    }
    out.seconds = t0.elapsed().as_secs_f64();
    out
}

/// The projection's build options of a route: `[search] steiner` adds the Steiner grid (4.2).
fn build_opts(opts: &Options) -> BuildOpts {
    let d = BuildOpts::default();
    BuildOpts { steiner_mm: if opts.steiner { d.steiner_mm } else { 0.0 }, existing_vias: true, keepouts: true, plane_layers: opts.plane_signals != "forbidden", copper_shapes: true, kept_tracks: true, lead_margin_nm: opts.margin_nm.max(0) as f64 + super::pack::DRAW_TOL_NM, lead_choice: opts.dangling_lead_choice, ..d }
}

/// The failure reason of a connection whose ends the topological core does not route: an end on
/// a pour other than a pad's (pads join pours through plane stubs, 4.4), or a branch point. Names
/// what the two ends are (exist-term r2: the text was "an existing via or track end joined to a
/// pour" for every pour pair). O(1).
fn unsupported_ends(ta: &TermKind, tb: &TermKind) -> String {
    let what = |t: &TermKind| match t {
        TermKind::Pad(_) => "a pad",
        TermKind::Via(_) => "an existing via",
        TermKind::Copper(_) => "an existing track end",
        TermKind::Dangling(..) => "a dangling track end",
        TermKind::Pour(_) => "a pour",
        TermKind::Junction => "a branch point on a routed trace",
    };
    let pour = |t: &TermKind| matches!(t, TermKind::Pour(_));
    match (pour(ta), pour(tb)) {
        (true, true) => "a pour joined to another pour: not routed by the topological core (a pour joins its net through pads, plane stubs and its own copper)".to_string(),
        (true, false) if matches!(tb, TermKind::Pad(_)) => "a pour joined to a pad with the pour as the first end: not routed by the topological core (it wires a pad to its pour only as pad -> pour, through a plane stub)".to_string(),
        (true, _) | (_, true) => format!(
            "{} joined to a pour: not supported by the topological core yet (existing vias and track ends are terminals towards pads, vias and tracks; a pad joins its pour through a plane stub)",
            what(if pour(ta) { tb } else { ta })
        ),
        _ => format!("{} to {}: a branch point is no terminal the topological core routes from", what(ta), what(tb)),
    }
}

/// exist-term: where a connection end on existing copper attaches (design note
/// docs/dev/design/exist-term.md). TopoR grows new wires from fixed wires and vias (6.3 manual
/// pp. 110-111: "an unfixed wire is created between the object you are moving and the fixed
/// wire").
#[derive(Clone, Debug, PartialEq)]
enum ExistEnd {
    /// A kept-track end inside an own-net pad (index into `model.pads`): that pad.
    Pad(usize),
    /// An existing via, or a kept-track end inside an own-net via (index into `model.vias`).
    Via(usize),
    /// A kept-track end elsewhere, on the track `kept` (index into `model.kept`) at `at` on its
    /// layer. `dangling`: a free end of a hand-started track (the connection starts there);
    /// `dir`: the direction to continue it (`[search] dangling_ends = "continue"`).
    Kept { kept: usize, at: Vec2, dangling: bool, dir: Option<Vec2> },
}

/// exist-term: the [`ExistEnd`] of connection end `t` of net `net`; `None` for a pad, a pour, a
/// junction or a kept item the model does not have. O(pads + vias of the model).
fn exist_end(m: &BoardModel, net: i32, t: &TermRef) -> Option<ExistEnd> {
    let (uuid, dangling, dir) = match &t.kind {
        TermKind::Via(i) => return Some(ExistEnd::Via(*i)),
        TermKind::Copper(u) => (u, false, None),
        TermKind::Dangling(u, d) => (u, true, d.map(|d| Vec2 { x: d[0], y: d[1] })),
        _ => return None,
    };
    let kept = m.kept.iter().position(|k| &k.uuid == uuid)?;
    let layer = m.kept[kept].layer;
    let at = t.at;
    let dot = crate::geom::Prim::Seg(crate::geom::Seg { a: at, b: at });
    if let Some(i) = m.pads.iter().position(|p| p.net == net && p.layers.contains(&layer) && crate::geom::dist::dist_prim_skeleton(&dot, &m.skeleton(p, layer)) <= 0.0) {
        return Some(ExistEnd::Pad(i));
    }
    let all = m.all_layer_ids();
    if let Some(i) = m.vias.iter().position(|v| v.net == net && crate::model::via_layers(&v.layers, &all).contains(&layer) && (v.at.to_vec2() - at).len() <= 0.5 * v.d as f64) {
        return Some(ExistEnd::Via(i));
    }
    Some(ExistEnd::Kept { kept, at, dangling, dir })
}

/// exist-term: the terminals on existing copper the projection gets ([`board::Extra::KeptEnd`])
/// and, per copper cluster of the decomposition (`Decomposition::kept_cluster`), the points of
/// them that lie on that cluster (the candidates a connection from it may start at). A dangling
/// end is one terminal (with its lead when it is continued); a connection from any other kept
/// end may join its cluster's kept copper anywhere (lead 2026-11-17: the whole own-net track is
/// the terminal, a free T at the cheapest point; memory through-own-pad-routing): one disc every
/// `2 (w + s)` along each kept track of the cluster, the track ends included, none inside an
/// own-net pad or via (that copper is a terminal itself). O(kept tracks of the clusters x
/// samples x pads of the net).
fn exist_requests(m: &BoardModel, opts: &Options, dec: &crate::decompose::Decomposition, layers: &[u32]) -> (Vec<board::Extra>, BTreeMap<usize, Vec<Vec2>>) {
    let mut out: Vec<board::Extra> = Vec::new();
    let mut seen: Vec<(i32, Vec2)> = Vec::new();
    let mut cands: BTreeMap<usize, Vec<Vec2>> = BTreeMap::new();
    let mut push = |out: &mut Vec<board::Extra>, x: board::Extra, net: i32, at: Vec2| {
        if !seen.iter().any(|&(n, q)| n == net && (q - at).len() <= 2.0) {
            seen.push((net, at));
            out.push(x);
        }
    };
    let all = m.all_layer_ids();
    let own_copper = |net: i32, layer: u32, p: Vec2, r: f64| {
        let dot = crate::geom::Prim::Seg(crate::geom::Seg { a: p, b: p });
        m.pads.iter().any(|q| q.net == net && q.layers.contains(&layer) && crate::geom::dist::dist_prim_skeleton(&dot, &m.skeleton(q, layer)) <= r)
            || m.vias.iter().any(|v| v.net == net && crate::model::via_layers(&v.layers, &all).contains(&layer) && (v.at.to_vec2() - p).len() <= 0.5 * v.d as f64 + r)
    };
    for c in &dec.connections {
        for t in [&c.from, &c.to] {
            let Some(ExistEnd::Kept { kept, at, dangling, dir }) = exist_end(m, c.net, t) else { continue };
            let k = &m.kept[kept];
            if !layers.contains(&k.layer) {
                continue;
            }
            let (w, s) = (k.width as f64, (c.clearance.max(m.rules.min_clearance)) as f64);
            if dangling {
                let len = if opts.dangling_lead_mm > 0.0 { opts.dangling_lead_mm * MM } else { c.width as f64 + s };
                // r5 (`dangling_lead_via`): where on the lead a via fits, net class via first (the
                // drawing puts it there when the plan changes layer at the lead end)
                let via = if opts.dangling_lead_via { crate::vias::lead_via_sizes(&m.rules, c.net, opts, c.width.max(k.width)) } else { Vec::new() };
                push(&mut out, board::Extra::KeptEnd { net: c.net, layer: k.layer, at, w, s, lead: dir.map(|d| (d, len)), via }, c.net, at);
                continue;
            }
            let Some(&cl) = dec.kept_cluster.get(&kept) else { continue };
            // The planned end itself is always a terminal (a cluster may have several).
            push(&mut out, board::Extra::KeptEnd { net: c.net, layer: k.layer, at, w, s, lead: None, via: Vec::new() }, c.net, at);
            if let Some(pts) = cands.get_mut(&cl) {
                if !pts.iter().any(|&q| (q - at).len() <= 2.0) {
                    pts.push(at);
                }
                continue;
            }
            let mut pts: Vec<Vec2> = vec![at];
            for (kj, kk) in m.kept.iter().enumerate().filter(|(kj, kk)| dec.kept_cluster.get(kj) == Some(&cl) && layers.contains(&kk.layer)) {
                let _ = kj;
                let (a, b) = (kk.prim.start().to_f64(), kk.prim.end().to_f64());
                let wk = kk.width as f64;
                let step = 2.0 * (wk + s);
                // An arc: its two ends only (its chord is no copper).
                let n = if matches!(kk.prim, crate::geom::quant::TrackPrim::Arc { .. }) { 0 } else { ((b - a).len() / step).floor() as usize };
                for i in 0..=n + 1 {
                    let p = if i > n { b } else { a + (b - a) * (i as f64 * step / (b - a).len().max(1.0)) };
                    if pts.iter().any(|&q| (q - p).len() < 0.8 * wk.max(w)) || own_copper(c.net, kk.layer, p, 0.5 * wk) {
                        continue;
                    }
                    pts.push(p);
                    push(&mut out, board::Extra::KeptEnd { net: c.net, layer: kk.layer, at: p, w: wk, s, lead: None, via: Vec::new() }, c.net, p);
                }
            }
            cands.insert(cl, pts);
        }
    }
    (out, cands)
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
    /// 4.4: plane pads no stub serves, wired to their pour's filled copper by a wire of the plan
    /// (their end is an [`board::Extra::Terminal`] of `extra`).
    far: Vec<planes::FarTerm>,
}

/// The projection of one route call (shopping list 4.2, roadmap box 1 "triangulation on fixed
/// items only"): everything a cycle needs that depends only on the fixed items, built **once** by
/// [`Fixed::build`] in [`route_topo`] after the plane stubs and read by every cycle of every run
/// ([`route_round`]). TopoR builds one triangulation for all layers once, from the pads, vias,
/// keepouts and outline, and routes every pass on it (Polubasov 2010, "взгляд изнутри",
/// `docs/literature/INDEX.md`).
///
/// Holds:
/// * `dec`, `pairs`, `alias`: the decomposition of the nets into connections (pads of pour nets
///   that no stub serves joined by signal connections), the diff pairs paired up (D7) and the
///   planning model in which the N half of a coupled pair carries the P net (`None` = the model);
/// * `board`: the projection of that model with the plane stubs as obstacles: the constrained
///   triangulation (pads, existing vias, keepouts, copper graphics, outline), the LCT cuts, the
///   Steiner grid when `[search] steiner` is on, and with `[capacity] model = "cuts"` the
///   per-layer room of the edges from the layer's sheet; `Err` = the build failed (every cycle
///   reports it as the reason, as before);
/// * `notes`: report lines of the build that are not the board's own (the sheet room failing).
///
/// Never in it (per cycle, in [`route_round`]): the drawing feedback rooms (`hints`, applied on a
/// copy of the board), the over-full history (`Memory`), the seed and fluctuation of the weights,
/// and everything that depends on routed wires (the `State`). Deterministic: the same inputs give
/// the same `Fixed`, so building it once gives the cycles exactly what a rebuild per cycle gave.
struct Fixed {
    dec: crate::decompose::Decomposition,
    pairs: Pairs,
    alias: Option<BoardModel>,
    board: Result<board::Board, String>,
    notes: Vec<String>,
    /// exist-term: per copper cluster of `dec` with a connection from a kept end, the points of
    /// its kept-end terminals ([`exist_requests`]).
    exist_cands: BTreeMap<usize, Vec<Vec2>>,
}

impl Fixed {
    /// O(projection build): the CDT over the obstacles (+ grid points), the cuts, the sheets.
    fn build(model: &BoardModel, opts: &Options, stubs: &PlaneStubs) -> Fixed {
        let signal: Vec<u32> = model.layers.iter().filter(|l| l.routable).map(|l| l.id).collect();
        let mut dec = decompose_with(model, opts, &signal, &stubs.signal_pads);
        let pairs: Pairs = if opts.diffpair_route { diffpair::plan(model, &mut dec) } else { Pairs::default() };
        let alias = (!pairs.coupled.is_empty()).then(|| diffpair::alias(model, &pairs, opts));
        let mut notes = Vec::new();
        let mut exist_cands = BTreeMap::new();
        let board = if signal.len() < 2 {
            Err(format!("{} signal layer(s)", signal.len()))
        } else {
            let pm: &BoardModel = alias.as_ref().unwrap_or(model);
            // 5.10 (`[search] pad_exit_stubs`): the planned pad exits join the fixed items.
            let mut extra = stubs.extra.clone();
            if opts.pad_exit_stubs {
                extra.extend(exit_requests(pm, &dec));
            }
            // exist-term: the terminals on existing copper.
            let (xs, cands) = exist_requests(pm, opts, &dec, &signal);
            extra.extend(xs);
            exist_cands = cands;
            board::build_full(pm, &build_opts(opts), &[], &extra).map(|mut b| {
                if !b.exit_stubs.is_empty() {
                    notes.push(format!(
                        "pad exits ([search] pad_exit_stubs): {} planned exit stubs (fine-pitch pads left through their tip along the pad axis, fixed before the topology)",
                        b.exit_stubs.len()
                    ));
                }
                // `[capacity] model = "cuts"` (shopping list 2.3): the per-layer room of the edges the
                // shared projection leaves unbounded comes from the layer's sheet (P2 and the per-edge
                // layer masks).
                if opts.capacity_model == "cuts" {
                    let co = super::cutmodel::CutOpts::from_options(opts);
                    match super::cutmodel::bare_sheets(&b.proj, &board::outline_segments(pm), &co) {
                        Ok(sh) => {
                            for (e, l, room) in super::cutmodel::sheet_room(&b.proj, &sh) {
                                b.proj.room[e][l] = room;
                            }
                        }
                        Err(e) => notes.push(format!("capacity model \"cuts\": the sheets of the P2 room failed, P2 keeps the per-edge room: {e}")),
                    }
                }
                b
            })
        };
        Fixed { dec, pairs, alias, board, notes, exist_cands }
    }
}

/// Shopping list 5.10: one [`board::Extra::Exit`] per pad end of every connection of `dec`
/// (towards its other terminal), in connection order; the build keeps those that are needed and
/// legal ([`board::Extra::Exit`]). O(connections).
fn exit_requests(m: &BoardModel, dec: &crate::decompose::Decomposition) -> Vec<board::Extra> {
    let mut out = Vec::new();
    for c in &dec.connections {
        for (t, o) in [(&c.from, &c.to), (&c.to, &c.from)] {
            if let crate::decompose::TermKind::Pad(pi) = t.kind {
                if pi < m.pads.len() {
                    out.push(board::Extra::Exit { pad: pi, toward: o.at, w: c.width as f64, s: (c.clearance.max(m.rules.min_clearance)) as f64 });
                }
            }
        }
    }
    out
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
fn place_plane_stubs(model: &BoardModel, opts: &Options, ctl: &RunControl) -> PlaneStubs {
    // Cancel ends the placement at once (K1: Cancel was ignored here on a live run); the route that
    // follows sees the same stop. The time limit ends it only with `[run] timeout_mode = "hard"`:
    // the soft limit (M2.TL, default) cuts no step, and the initial routing that follows needs every
    // stub to give a complete plan (verify-m2 F2); past the overrun cap (F3) it acts as hard.
    let stop = || cancelled() || ctl.is_cancelled() || ctl.limit.cuts_in(CutStep::PlaneStubs);
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
    // Planes as terminals (`[planes] terminals`): the pours' connectivity before the run (the KiCad
    // fill of the ripped board, its tracks and vias, the pads; no routed copper). Only fill in its
    // net's main component is the net's copper: a stub or wire ending in a cut-off island
    // connects nothing (user 2026-11-12: "given that the pour is connected to the rest of the net").
    let live = (opts.plane_terminals && model.pours.iter().any(|z| !z.fills.is_empty())).then(|| crate::pourcheck::check_pour_reach(model, &[], &[]));
    let cost = planes::SiteCost { mm: reg.price(&Event::Length { wire: 0, mm: 1.0, scale: 1.0 }, &WireCtx::default()), reg: &reg, live: live.as_ref(), fills: planes::FillIndex::new(model) };
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
        // 4.4 (`[planes] share_vias`): the own-net copper on the plane a stub may end on. The
        // existing vias of the board standing in a fill of their net's pour come first; every
        // stub via placed joins them. Default "outside_bga" (user 2026-11-13: "no shared vias
        // under bga chips, shared vias are okay outside of BGA footprints"; for the balls:
        // "sharing only when not possible in any other way, usually there is a good reason
        // multiple balls are assigned for the same net"): by footprint membership, a share where
        // the pad or the via is a ball's ([`planes::bga_footprints`] pads) is a last resort (the
        // `[planes] bga_share_cost` price, one warning per such via); other pads, also parts
        // under a BGA, share freely among themselves; a shared via is not re-sited into a BGA
        // area none of its pads stands in. "never": no pad shares (empty, so the re-siting and
        // merge rounds below do nothing); "always": every pad shares at the plain price.
        let bga_fp = if opts.plane_share_vias == "outside_bga" { planes::bga_footprints(model, &opts.plane_bga_refs) } else { Vec::new() };
        for f in &bga_fp {
            notes.push(format!(
                "BGA area {}: ({:.2}, {:.2})-({:.2}, {:.2}) mm, {} balls ({}); its plane balls share a via only where no own via site is within reach ([planes] share_vias = \"outside_bga\", bga_share_cost)",
                f.ref_,
                f.area.min.x / 1e6,
                -f.area.max.y / 1e6,
                f.area.max.x / 1e6,
                -f.area.min.y / 1e6,
                f.balls,
                if f.forced { "forced by [planes] bga_refs" } else { "detected; [planes] bga_refs = [\"-REF\"] keeps a footprint out" }
            ));
        }
        let bga: Vec<crate::geom::Aabb> = bga_fp.iter().map(|f| f.area).collect();
        let share_mode = opts.plane_share_vias.as_str();
        if !matches!(share_mode, "never" | "outside_bga" | "always") {
            notes.push(format!("[planes] share_vias = \"{share_mode}\" is not one of never, outside_bga, always: no plane via is shared"));
        }
        let balls: BTreeSet<usize> = bga_fp.iter().flat_map(|f| f.pads.iter().copied()).collect();
        let is_ball = |rq: &planes::Req| balls.contains(&rq.pad);
        let last_resort = opts.plane_bga_share_cost.max(0.0);
        let pad_at = |rq: &planes::Req| model.pads[rq.pad].center.to_vec2();
        let mut shared: Vec<planes::PlaneVia> = if matches!(share_mode, "outside_bga" | "always") { existing_plane_vias(model) } else { Vec::new() };
        let mut outs: Vec<Option<planes::Out>> = Vec::with_capacity(reqs.len());
        let mut results: Vec<Result<planes::Out, String>> = Vec::with_capacity(reqs.len());
        let mut dropped = false;
        // Far terminals (4.4): per plane pad no stub serves, the stub stage's reason and the
        // candidate ends on its net's connected fill; the path search picks one after the stubs.
        let mut pending: Vec<(usize, String, Vec<planes::FarTerm>)> = Vec::new();
        for (i, rq) in reqs.iter().enumerate() {
            if stop() {
                return PlaneStubs::default();
            }
            let r = planes::place(&mut dr, rq, &cost, &vias, &shared, is_ball(rq), last_resort);
            match &r {
                Ok(o) => {
                    if let Some(v) = o.via {
                        vias.push((v, 0.5 * o.size.d as f64));
                        if share_mode != "never" {
                            shared.push(planes::PlaneVia { at: v, size: o.size, net: rq.net, load: rq.w, owner: Some(i), users: Vec::new(), dead: false, ball: is_ball(rq) });
                        }
                    }
                    if let Some(k) = o.shared {
                        shared[k].load += rq.w;
                        shared[k].users.push(i);
                        shared[k].ball |= is_ball(rq);
                    }
                }
                // No clean via site inside the pour's fill near the pad. 4.4: the pour is a
                // terminal of the net, so the pad is wired to its net's connected filled copper
                // (a wire of the plan, ending on it: no length inside the pour); without such a
                // place, or with `[planes] terminals` off, a signal connection joins it to its
                // net's copper instead.
                Err(e) => {
                    let cands = if opts.plane_terminals { planes::far_candidates(&dr, rq, &cost) } else { Vec::new() };
                    if cands.is_empty() {
                        notes.push(format!("{e}: joined to its net's copper by a signal connection instead"));
                        signal_pads.insert(rq.pad);
                        dropped = true;
                    } else {
                        pending.push((i, e.clone(), cands));
                    }
                }
            }
            outs.push(r.as_ref().ok().cloned());
            results.push(r);
        }
        if !dropped {
            // 4.4: a stub via that other pads' stubs share moves to where their stubs are
            // shortest together (plane_via_3r: the three GND stubs meet at the centre). A pad
            // left with a via of its own is then offered the moved vias again (the order of the
            // first pass decided which via it saw), and the groups that grew move again; at most
            // as many rounds as there are vias, each round removes one or ends.
            for _ in 0..=shared.len() {
                for v in shared.iter_mut() {
                    if stop() {
                        return PlaneStubs::default();
                    }
                    if v.dead || v.owner.is_none() || v.users.is_empty() {
                        continue;
                    }
                    let old = v.at;
                    let members: Vec<Vec2> = v.owner.into_iter().chain(v.users.iter().copied()).map(|i| pad_at(&reqs[i])).collect();
                    let site_ok = |p: Vec2| share_mode != "outside_bga" || planes::resite_ok(&bga, &members, p);
                    if planes::resite(&mut dr, &reqs, &mut outs, v, &cost, &vias, &site_ok) {
                        if let Some(e) = vias.iter_mut().find(|e| (e.0 - old).len() <= 1.0) {
                            e.0 = v.at;
                        }
                    }
                }
                let mut merged = false;
                for k in 0..shared.len() {
                    let Some(i) = shared[k].owner.filter(|_| !shared[k].dead && shared[k].users.is_empty()) else { continue };
                    let Some(prev) = outs[i].clone() else { continue };
                    for &h in &prev.handles {
                        dr.uncommit(h);
                    }
                    shared[k].dead = true;
                    let at = shared[k].at;
                    let others: Vec<(Vec2, f64)> = vias.iter().copied().filter(|e| (e.0 - at).len() > 1.0).collect();
                    match planes::place(&mut dr, &reqs[i], &cost, &others, &shared, is_ball(&reqs[i]), last_resort) {
                        Ok(o) if o.shared.is_some() => {
                            let j = o.shared.expect("shared");
                            shared[j].load += reqs[i].w;
                            shared[j].users.push(i);
                            shared[j].ball |= is_ball(&reqs[i]);
                            vias = others;
                            outs[i] = Some(o);
                            merged = true;
                        }
                        // Not cheaper to share: put the pad's own copper back as it was.
                        Ok(o) => {
                            for &h in &o.handles {
                                dr.uncommit(h);
                            }
                            shared[k].dead = false;
                            let mut back = prev;
                            back.handles = back.items.iter().map(|it| dr.commit(it)).collect();
                            outs[i] = Some(back);
                        }
                        Err(_) => {
                            shared[k].dead = false;
                            let mut back = prev;
                            back.handles = back.items.iter().map(|it| dr.commit(it)).collect();
                            outs[i] = Some(back);
                        }
                    }
                }
                if !merged {
                    break;
                }
            }
            for (i, (rq, r)) in reqs.iter().zip(results).enumerate() {
                if pending.iter().any(|f| f.0 == i) {
                    continue; // a wire of the plan (4.4), not a stub
                }
                match &outs[i] {
                    Some(o) => {
                        out.extra.extend(planes::extras(&b, rq, o));
                        out.res.push((rq.conn, Ok(o.clone())));
                    }
                    None => out.res.push((rq.conn, r)),
                }
            }
            // One warning per via a ball shares (the last resort of "outside_bga"): check it by hand.
            for v in shared.iter().filter(|v| !v.dead && v.ball && !v.users.is_empty()) {
                let names: Vec<String> = v.owner.into_iter().chain(v.users.iter().copied()).map(|i| {
                    let p = &model.pads[reqs[i].pad];
                    format!("{}.{}", p.ref_, p.number)
                }).collect();
                notes.push(format!(
                    "BGA ball shares a plane via (last resort, no own via site within reach): the {} at ({:.3}, {:.3}) mm serves {} ([planes] bga_share_cost)",
                    if v.owner.is_some() { "stub via" } else { "existing via" },
                    v.at.x / 1e6,
                    -v.at.y / 1e6,
                    names.join(", ")
                ));
            }
            let nshared = shared.iter().filter(|v| !v.dead && v.owner.is_some()).map(|v| v.users.len()).sum::<usize>();
            let nexist = shared.iter().filter(|v| v.owner.is_none()).map(|v| v.users.len()).sum::<usize>();
            if nshared + nexist > 0 {
                notes.push(format!(
                    "shared plane vias ([planes] share_vias): {nshared} plane stub(s) end on another pad's stub via, {nexist} on an existing via of the board; {} stub via(s) for {} plane pad(s)",
                    shared.iter().filter(|v| !v.dead && v.owner.is_some()).count(),
                    outs.iter().filter(|o| o.is_some()).count()
                ));
            }
            // 4.4: the end of each far terminal's wire, picked by the path search among its
            // candidates on the board with every stub in place.
            let picks = select_far(model, opts, &reg, &dec, &dr, &out.extra, &reqs, &pending, &mut notes);
            if picks.iter().any(Option::is_none) {
                // No candidate clear of the stubs: a signal connection; the decomposition changes,
                // so place again.
                for ((i, e, _), _) in pending.iter().zip(&picks).filter(|(_, p)| p.is_none()) {
                    notes.push(format!("{e}: joined to its net's copper by a signal connection instead"));
                    signal_pads.insert(reqs[*i].pad);
                }
                continue;
            }
            for ((i, e, _), ft) in pending.iter().zip(picks.into_iter().flatten()) {
                let rq = &reqs[*i];
                notes.push(format!(
                    "{e}: wired to its pour's filled copper at ({:.3}, {:.3}) mm instead (planes as terminals)",
                    ft.at.x / 1e6,
                    -ft.at.y / 1e6
                ));
                out.extra.push(board::Extra::Terminal { net: rq.net, at: ft.at, layers: ft.layers.clone(), size: ft.size });
                out.far.push(ft);
            }
            out.signal_pads = signal_pads;
            out.notes = notes;
            return out;
        }
    }
}

/// The far terminal each pending plane pad's wire ends on (4.4; user 2026-11-12: routing over a
/// connected pour has no length, the trace ends there): of its candidates ([`planes::far_candidates`],
/// cheapest straight price first) those that keep clear of the stubs placed, then the one the P2
/// path search on the bare board (every stub and every candidate in the projection, no wire
/// routed yet) reaches cheapest: its taut length, its layer changes and walls priced as in P2,
/// nothing inside the pour. A pad with one candidate left keeps it without a search; `None`: no candidate
/// is clear of the stubs. Then the same search between that end and the candidates the spacing
/// set aside next to it ([`planes::FarTerm::alts`]). Decided once per route (decide early: the
/// planner gets one terminal per pad). O(2 board builds + two P2 searches per pending pad).
#[allow(clippy::too_many_arguments)]
fn select_far(model: &BoardModel, opts: &Options, reg: &Registry, dec: &crate::decompose::Decomposition, dr: &Drawer, stub_extra: &[board::Extra], reqs: &[planes::Req], pending: &[(usize, String, Vec<planes::FarTerm>)], notes: &mut Vec<String>) -> Vec<Option<planes::FarTerm>> {
    // Clear of the stubs on at least one of its layers (those layers kept).
    let clear_of = |net: i32, f: &planes::FarTerm| {
        let layers: Vec<u32> = f.layers.iter().copied().filter(|&l| dr.check_ok(&draw::track_item(net, l, f.size, f.at, f.at + Vec2 { x: 1.0, y: 0.0 }, None))).collect();
        (!layers.is_empty()).then(|| planes::FarTerm { layers, ..f.clone() })
    };
    let pads: Vec<usize> = pending.iter().map(|p| p.0).collect();
    let clear: Vec<Vec<planes::FarTerm>> = pending.iter().map(|(i, _, cands)| cands.iter().filter_map(|f| clear_of(reqs[*i].net, f)).collect()).collect();
    let picks = pick_far(model, opts, reg, dec, stub_extra, reqs, &pads, &clear, notes);
    let mut out: Vec<Option<planes::FarTerm>> = clear.iter().zip(&picks).map(|(c, &k)| c.get(k).cloned()).collect();
    // Second round, by the same path price: the picked end against the candidates its spacing
    // set aside (`FarTerm::alts`, e.g. the corner next to an edge point that was straighter from
    // the pad: pour_zero J3.1). The spacing only spreads the first round over the pour.
    let round2: Vec<Vec<planes::FarTerm>> = out
        .iter()
        .zip(&pads)
        .map(|(p, &i)| p.as_ref().map_or_else(Vec::new, |f| std::iter::once(f.clone()).chain(f.alts.iter().filter_map(|a| clear_of(reqs[i].net, a))).collect()))
        .collect();
    if round2.iter().any(|c| c.len() > 1) {
        let picks2 = pick_far(model, opts, reg, dec, stub_extra, reqs, &pads, &round2, notes);
        for (o, (c, &k)) in out.iter_mut().zip(round2.iter().zip(&picks2)) {
            if c.len() > 1 {
                *o = c.get(k).cloned();
            }
        }
    }
    out
}

/// The index of the candidate per pending pad (`pads[k]` indexes `reqs`, `cands[k]` its
/// candidates) the P2 path search reaches cheapest from the pad, on the board with the stubs and
/// every candidate in the projection; 0 where there is nothing to choose or no search. The pick
/// compares ends whose paths differ by a few mm: always the taut length here. The "even" length
/// overrates straight paths on long triangulation edges (`[search] length`) and picked an end
/// off the straight line (plane_terminal J1.1: 16.74 mm instead of 15.57) or round the far side
/// of a connector row (pour_zero J3.1). One board build plus one search per pad.
#[allow(clippy::too_many_arguments)]
fn pick_far(model: &BoardModel, opts: &Options, reg: &Registry, dec: &crate::decompose::Decomposition, stub_extra: &[board::Extra], reqs: &[planes::Req], pads: &[usize], cands: &[Vec<planes::FarTerm>], notes: &mut Vec<String>) -> Vec<usize> {
    let pad_name = |i: usize| format!("{}.{}", model.pads[reqs[i].pad].ref_, model.pads[reqs[i].pad].number);
    if cands.iter().all(|c| c.len() <= 1) {
        return vec![0; cands.len()];
    }
    let mut extra = stub_extra.to_vec();
    for (&i, cs) in pads.iter().zip(cands) {
        for f in cs {
            extra.push(board::Extra::Terminal { net: reqs[i].net, at: f.at, layers: f.layers.clone(), size: f.size });
        }
    }
    let b2 = match board::build_full(model, &build_opts(opts), &[], &extra) {
        Ok(b2) => b2,
        Err(e) => {
            notes.push(format!(
                "far terminal ends of {}: the board with the candidate ends did not build ({e}); each takes its straightest candidate, unchecked by the path search",
                pads.iter().zip(cands).filter(|(_, c)| c.len() > 1).map(|(&i, _)| pad_name(i)).collect::<Vec<_>>().join(", ")
            ));
            return vec![0; cands.len()];
        }
    };
    let mut st = State::new(&b2.proj);
    st.cross_vias = opts.crossing_vias == "via";
    st.free_crossing = opts.free_crossing;
    st.taut_len = true;
    let ws0 = (model.rules.default.width + model.rules.default.clearance) as f64;
    let term_obs = |net: i32, at: Vec2| (0..b2.proj.obstacles.len()).find(|&o| b2.pad_of[o] == usize::MAX && b2.proj.obstacles[o].net == Some(net) && (b2.proj.obstacles[o].center - at).len() <= 2.0 && b2.centre[o].is_some());
    let end = |o: ObsId| b2.centre[o].map(|v| super::init::TreeEnd { v, obs: o, on: None, adj: 0.0, arms: [None; 4] });
    let mut out = Vec::with_capacity(cands.len());
    for (&i, cs) in pads.iter().zip(cands) {
        let rq = &reqs[i];
        if cs.len() <= 1 {
            out.push(0);
            continue;
        }
        let pad_obs = (0..b2.proj.obstacles.len()).find(|&o| b2.pad_of[o] == rq.pad);
        let ends: Vec<(usize, super::init::TreeEnd)> = cs.iter().enumerate().filter_map(|(k, f)| term_obs(rq.net, f.at).and_then(end).map(|e| (k, e))).collect();
        let (Some(src), false) = (pad_obs.and_then(end), ends.is_empty()) else {
            notes.push(format!("far terminal end of {}: the pad or its candidate ends are not in the projection; it takes its straightest candidate, unchecked by the path search", pad_name(i)));
            out.push(0);
            continue;
        };
        let c = &dec.connections[rq.conn];
        let w = rq.w;
        let s = c.clearance as f64;
        let conn = Conn { net: rq.net, pa: src.obs, pb: ends[0].1.obs, a: src.v, b: ends[0].1.v, w, s };
        let wt = InitWeights { max_expansions: opts.max_expansions.max(1), ..InitWeights::from_registry(reg, &WireCtx { net: rq.net, tier: c.tier }) };
        let dst: Vec<super::init::TreeEnd> = ends.iter().map(|e| e.1).collect();
        match st.search_tree(&conn, &[src], &dst, &wt, (w + s) / ws0, None) {
            Some((_, _, j)) => out.push(ends[j].0),
            None => {
                notes.push(format!("far terminal end of {}: the path search reached none of its candidate ends; it takes its straightest candidate", pad_name(i)));
                out.push(0);
            }
        }
    }
    out
}

/// The existing through vias of the board that stand in a fill of a pour of their own net (4.4): own-net
/// copper already on the plane, which a plane pad's stub may end on like on a stub via. O(vias x
/// pours x fill vertices).
fn existing_plane_vias(model: &BoardModel) -> Vec<planes::PlaneVia> {
    // Through vias only: a blind or buried one may not reach the pad's layer.
    let all = model.all_layer_ids();
    model
        .vias
        .iter()
        .filter(|v| v.net > 0 && (v.layers.is_empty() || crate::model::via_layers(&v.layers, &all).len() == all.len()))
        .filter(|v| model.pours.iter().any(|z| z.net == v.net && planes::in_fill(z, v.at.to_vec2(), 0.5 * v.d as f64)))
        .map(|v| planes::PlaneVia { at: v.at.to_vec2(), size: crate::vias::ViaSize { d: v.d, drill: v.drill }, net: v.net, load: 0.0, owner: None, users: Vec::new(), dead: false, ball: false })
        .collect()
}

/// One whole route. `hist0` is the per-edge over-full history of earlier rounds (empty = none);
/// the third result is the history at the end of this one.
/// `fluct0`: the call's configured fluctuation (the TopoR loop's coefficient schedule starts there;
/// `opts.fluctuation` is this cycle's width).
#[allow(clippy::too_many_arguments)]
/// `gate`: the run (0-based), the call's [`DrawGate`] and objective (cancel-fix: after Cancel only
/// one TopoR loop pick of the call is drawn), and the run's live report ([`LiveLoop`]: the TopoR
/// loop calls it after its initial plan and every cycle, so its pick is Best so far and its stable
/// shows the moment it is recorded, user 2026-11-17 "Report stable live").
///
/// TW3 `draw_row`: a draw-only round: no initial routing, no rip-up rounds, no loop; the row's
/// state and its run's pre-loop decisions ([`PreLoop`]) are restored and drawn (a run draws the
/// rows of the call's four bests that no other run draws, [`DrawGate::assign`]).
///
/// pause-button: the round's thread has the run's pause gate ([`crate::control::with_gate`]), so
/// the pause points of its work without a stop test of their own (the via relocation, the
/// octilinear straightening, the sheets' cancel tests) wait there while paused, also outside a
/// step's `with_control` (the drawing of a pick).
#[allow(clippy::type_complexity)]
fn route_round(model: &BoardModel, opts: &Options, ctl: &RunControl, hints: &[(usize, usize, f64)], mem: &Memory, round: usize, stubs: &PlaneStubs, fx: &Fixed, fluct0: f64, gate: (usize, &DrawGate, &Objective, &dyn Fn(&LiveLoop)), draw_row: Option<&Assigned>) -> (RoundOut, Hints, Memory) {
    crate::control::with_gate(Some(ctl.limit.clone()), || route_round_in(model, opts, ctl, hints, mem, round, stubs, fx, fluct0, gate, draw_row))
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn route_round_in(model: &BoardModel, opts: &Options, ctl: &RunControl, hints: &[(usize, usize, f64)], mem: &Memory, round: usize, stubs: &PlaneStubs, fx: &Fixed, fluct0: f64, gate: (usize, &DrawGate, &Objective, &dyn Fn(&LiveLoop)), draw_row: Option<&Assigned>) -> (RoundOut, Hints, Memory) {
    // TOPO_TIMES=1 (debug): phase times on stderr; never part of the result.
    let times = std::env::var_os("TOPO_TIMES").is_some();
    let tlog = |what: &str, t: Instant| {
        super::prof::mark(what);
        if times {
            eprintln!("[topo round {round}] {what}: {:.2} s", t.elapsed().as_secs_f64());
        }
    };
    let hist0: &[f64] = &mem.edge;
    let t0 = Instant::now();
    // pause-button: the floor is on the run's clock (`TimeLimit::now`: paused time left out), as the
    // limit instants it is compared with
    let floor = ctl.limit.now() + std::time::Duration::from_secs(1); // a round gets at least 1 s, also past the limit
    // `[optimize] loop = "topor"`: after the loop the one drawing runs through an OK / Cancel (only
    // the time limit cuts it), so the loop's picked variant always reaches the copper.
    let draw_through = std::cell::Cell::new(false);
    // M2.TL (`[run] timeout_mode`): soft (default) = the time limit cuts no step: the step that runs
    // finishes, no new one begins (the loop's cycles, the next whole-route cycle) and the best kept
    // variant is drawn to its end; Cancel / OK stops at once. hard = every step stops at the limit
    // (before M2).
    // F3: past `[run] timeout_overrun_s` the soft limit acts as the hard one, read live: it cuts the
    // step that runs. "Draw what exists" (user 2026-11-13, for the cap and for `hard` alike, one
    // path): a limit that acted as hard before the drawing began cuts the planning step that ran
    // and does not cut the drawing of the kept plan (its via reduction included; TopoR judges the
    // variants before any copper and draws the chosen one, 6.3 manual pp. 72-73, p. 237); a limit
    // that passes while the drawing runs cuts it (the drawing is then the step that ran).
    let soft = || !ctl.limit.is_hard();
    let draw_exempt = std::cell::Cell::new(false);
    // verify-m2c 4: the step the stop tests below belong to (a planning step until the drawing
    // begins), so the report names what the limit cut.
    let cut_step = std::cell::Cell::new(CutStep::Plan);
    let stop_now = || cancelled() || ctl.is_cancelled();
    let limit_cut = || !draw_exempt.get() && !soft() && ctl.deadline().is_some_and(|d| ctl.limit.now() >= d.max(floor)) && ctl.limit.note_cut_in(cut_step.get());
    // limit-p3p4 (F10): the layer assignment, rip-up and moves of the initial plan (P3/P4, from the
    // first layering until the TopoR loop) are bounded like the TopoR loop step: with the soft
    // limit, past the limit plus `[run] limit_grace_s` their stop checks end them and the plan so
    // far is kept (P2, the topological search that places every connection, is the first plan and
    // runs to its end). Read only after the limit passed: without it nothing changes.
    let plan_span = std::cell::Cell::new(false);
    let plan_over = || plan_span.get() && soft() && ctl.limit.grace_over();
    // The dialog's Cancel (request_discard) ends every step, the drawing too: nothing is taken.
    let expired = || (!draw_through.get() && stop_now()) || limit_cut() || plan_over() || ctl.is_discarded();
    // The reason of a connection the drawing did not reach: a Cancel / OK's or the limit's (a
    // Cancel wins when both hold; verify-m2c 4: a Cancel wrote the limit's reason before).
    let draw_cut_reason = || if !draw_through.get() && stop_now() { DRAW_CANCEL } else { DRAW_CUT };
    // M2.TL: Cancel / OK cut the initial routing (P2) before every connection had its place: the
    // plan is not complete, so it is no variant (nothing kept to draw).
    let plan_cut = std::cell::Cell::new(false);
    let cut_here = || {
        let e = expired();
        if e && stop_now() {
            plan_cut.set(true);
        }
        e
    };
    let signal: Vec<u32> = model.layers.iter().filter(|l| l.routable).map(|l| l.id).collect();
    if signal.len() < 2 {
        let why = format!("the topological core routes boards with two or more signal layers; this board has {}", signal.len());
        return (RoundOut::unrouted(unrouted(model, opts, &why)), Vec::new(), Memory::default());
    }
    let (rnd, rnds) = (round as u32, opts.topo_rounds.max(1) as u32);
    ctl.update("setup", 0, 0, 0, 0, 0.0);
    ctl.stage(rnd, rnds, "projection of the board");
    // The projection of the call (4.2): built once from the fixed items, read here.
    let dec = &fx.dec;
    let pairs: &Pairs = &fx.pairs;
    let model_real = model;
    let model: &BoardModel = fx.alias.as_ref().unwrap_or(model);
    let mut warnings: Vec<String> = dec.warnings.clone();
    warnings.extend(pairs.notes.iter().cloned());
    warnings.extend(stubs.notes.iter().cloned());
    for (net, r) in &dec.skipped {
        warnings.push(format!("net {net} skipped: {r}"));
    }
    let (reg, problems) = Registry::from_config(standard_factors(), &opts.factors);
    warnings.extend(problems);
    let base = match &fx.board {
        Ok(b) => b,
        Err(e) => return (RoundOut::unrouted(unrouted(model, opts, &format!("projection failed: {e}"))), Vec::new(), Memory::default()),
    };
    warnings.extend(base.notes.iter().cloned());
    warnings.extend(fx.notes.iter().cloned());
    // Drawing feedback (`[optimize] drawing_feedback`) lowers rooms for this cycle only: on a copy
    // of the projection, so nothing that depends on routed wires leaks into the shared one.
    let mut b: std::borrow::Cow<board::Board> = std::borrow::Cow::Borrowed(base);
    if !hints.is_empty() {
        let bm = b.to_mut();
        for &(e, l, room) in hints {
            if let Some(r) = bm.proj.room.get_mut(e).and_then(|r| r.get_mut(l)) {
                *r = r.min(room);
            }
        }
    }
    let b: &board::Board = &b;
    tlog("projection (shared, built once per call; feedback rooms applied)", t0);
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
    // Width that ranks a connection in the routing and drawing order (widest first). A single
    // track: its width. A coupled pair (D7, user 2026-11-08): the whole band it occupies,
    // space + track + gap + track + space ([`diffpair::Coupled::priority_width`]).
    let mut prio_w: Vec<f64> = Vec::new();
    let mut plane_reqs: Vec<planes::Req> = Vec::new();
    // exist-term: per copper cluster with a terminal on existing copper, its net and the vertices
    // of its copper; the dangling-end vertices; the direction each continued end asks for.
    let mut exist_groups: BTreeMap<usize, (i32, Vec<VertId>)> = BTreeMap::new();
    let mut exist_dang: BTreeSet<VertId> = BTreeSet::new();
    let mut cont_dirs: BTreeMap<VertId, Vec2> = BTreeMap::new();
    // tw4 T3 (`[search] dangling_lead_choice`): per continued dangling end's main vertex (its
    // lead's end), the start points its lead offers `(vertex, distance from the hand end, nm)`.
    let mut lead_alt: BTreeMap<VertId, Vec<(VertId, f64)>> = BTreeMap::new();
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
                let (w, s, pw) = match pairs.of_lead(i) {
                    Some(cp) => {
                        let s = model.rules.net_rule(c.net).clearance as f64;
                        (cp.envelope(), s, cp.priority_width(s))
                    }
                    None => (c.width as f64, c.clearance as f64, c.width as f64),
                };
                conns.push(Conn { net: c.net, pa: oa, pb: ob, a: va, b: vb, w, s });
                prio_w.push(pw);
            }
            // 4.4: a plane pad no stub serves is wired to its pour's terminal like to a pad.
            (TermKind::Pad(pa), TermKind::Pour(_)) if stubs.far.iter().any(|f| f.conn == i) => {
                let ft = stubs.far.iter().find(|f| f.conn == i).expect("far terminal");
                let term = (0..b.proj.obstacles.len()).find(|&o| b.pad_of[o] == usize::MAX && b.proj.obstacles[o].net == Some(c.net) && (b.proj.obstacles[o].center - ft.at).len() <= 2.0 && b.centre[o].is_some());
                let (Some(&oa), Some(ot)) = (obs_of_pad.get(pa), term) else {
                    fail(&mut out, i, "a pad has no copper on a signal layer, or its pour terminal is not in the projection");
                    continue;
                };
                let (Some(va), Some(vt)) = (b.centre[oa], b.centre[ot]) else {
                    fail(&mut out, i, "a pad has no net centre vertex");
                    continue;
                };
                conn_of.push(i);
                conns.push(Conn { net: c.net, pa: oa, pb: ot, a: va, b: vt, w: c.width as f64, s: c.clearance as f64 });
                prio_w.push(c.width as f64);
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
            // exist-term: an end on existing copper (a via, a kept-track end) is a terminal like
            // a pad: its pad / via obstacle or its kept-end terminal of the projection.
            (ta, tb) if !matches!(ta, TermKind::Pour(_) | TermKind::Junction) && !matches!(tb, TermKind::Pour(_) | TermKind::Junction) => {
                let res = |t: &TermRef| -> Result<(ObsId, VertId, Option<ExistEnd>), &'static str> {
                    let ex = match t.kind {
                        TermKind::Pad(p) => Some(ExistEnd::Pad(p)),
                        _ => exist_end(model, c.net, t),
                    };
                    let o = match &ex {
                        Some(ExistEnd::Pad(p)) => obs_of_pad.get(p).copied().ok_or("a pad has no copper on a signal layer")?,
                        Some(ExistEnd::Via(v)) => b.via_obs.get(v).copied().ok_or("an existing via is not in the projection")?,
                        Some(ExistEnd::Kept { at, .. }) => b.kept_ends.iter().find(|k| k.net == c.net && (k.at - *at).len() <= 2.0).map(|k| k.obs).ok_or("an existing track end is not on a signal layer of the projection")?,
                        None => return Err("terminal is no copper the topological core knows"),
                    };
                    let v = b.centre[o].ok_or("a terminal has no net centre vertex")?;
                    Ok((o, v, ex))
                };
                match (res(&c.from), res(&c.to)) {
                    (Ok((oa, va, ea)), Ok((ob, vb, eb))) => {
                        for (v, o, e) in [(va, oa, &ea), (vb, ob, &eb)] {
                            if let Some(ExistEnd::Kept { kept, dangling, dir, .. }) = e {
                                // tw4 T3: a lead's start points (empty unless dangling_lead_choice)
                                let ch: &[(VertId, f64)] = if *dangling { b.kept_ends.iter().find(|k| k.obs == o).map_or(&[], |k| &k.choices[..]) } else { &[] };
                                if let Some(&cl) = dec.kept_cluster.get(kept) {
                                    let g = &mut exist_groups.entry(cl).or_insert_with(|| (c.net, Vec::new())).1;
                                    g.push(v);
                                    g.extend(ch.iter().map(|x| x.0));
                                }
                                if *dangling {
                                    exist_dang.insert(v);
                                    if let Some(d) = dir {
                                        cont_dirs.insert(v, *d);
                                        for &(u, _) in ch {
                                            cont_dirs.insert(u, *d);
                                        }
                                    }
                                    if !ch.is_empty() {
                                        lead_alt.insert(v, ch.to_vec());
                                    }
                                }
                            }
                        }
                        conn_of.push(i);
                        conns.push(Conn { net: c.net, pa: oa, pb: ob, a: va, b: vb, w: c.width as f64, s: c.clearance as f64 });
                        prio_w.push(c.width as f64);
                    }
                    (Err(e), _) | (_, Err(e)) => fail(&mut out, i, e),
                }
            }
            (ta, tb) => fail(&mut out, i, unsupported_ends(ta, tb)),
        }
    }
    // exist-term: the copper clusters with terminals on existing copper are one piece of their
    // net's copper before any wire (the hand copper joins them): every kept-end terminal on the
    // cluster, its pads and vias. A connection from it starts at any of them (net trees, D5),
    // except one whose planned end is a dangling end: that one continues the end. The
    // decomposition picks the end only when it is the cluster's nearest port to the goal; a
    // nearer pad or via of the cluster takes the connection and the hand stub stays dangling
    // (known gap, decision D2; test known_gap_a_nearer_cluster_pad_takes_the_root).
    for (&cl, g) in exist_groups.iter_mut() {
        if let Some(pts) = fx.exist_cands.get(&cl) {
            for p in pts {
                if let Some(k) = b.kept_ends.iter().find(|k| k.net == g.0 && (k.at - *p).len() <= 2.0) {
                    g.1.extend(b.centre[k.obs]);
                }
            }
        }
        for (&pi, _) in dec.pad_cluster.iter().filter(|(_, &x)| x == cl) {
            if let Some(&o) = obs_of_pad.get(&pi) {
                g.1.extend(b.centre[o]);
            }
        }
        for (&vi, _) in dec.via_cluster.iter().filter(|(_, &x)| x == cl) {
            if let Some(&o) = b.via_obs.get(&vi) {
                g.1.extend(b.centre[o]);
            }
        }
        g.1.sort_unstable();
        g.1.dedup();
    }
    // 4.3 `initial = "once"`: the connections' pad pairs as the plan of the cycle before left them.
    if let Some(pl) = mem.plan.as_ref().filter(|_| opts.initial == "once") {
        if pl.conns.len() == conns.len() {
            conns.clone_from(&pl.conns);
        }
    }
    ctl.update("route", 0, 0, nconn, 0, t0.elapsed().as_secs_f64());
    ctl.stage(rnd, rnds, "topological search");
    // ---- P2 ----
    let t_p2 = Instant::now();
    // `[capacity] cut_report` (debug, inflated sketch U0/U1): measures, never changes the routing.
    let mut crep = super::cutflow::Recorder::new(model, opts, &b.signal_layers, b.n_signal, round);
    let proj = &b.proj;
    let mut st = State::new(proj);
    st.cross_vias = opts.crossing_vias == "via";
    st.free_crossing = opts.free_crossing;
    // exist-term: the hand copper's connectivity and the continued ends' directions.
    for g in exist_groups.values() {
        st.pre_join.extend(g.1.windows(2).map(|x| (x[0], x[1])));
    }
    st.cont_dir = cont_dirs.clone();
    if std::env::var_os("TOPO_T3DBG").is_some() {
        // Debug (tw4 T3): the kept ends with a lead / with start points, and the dangling ends rooted here.
        let leads = b.kept_ends.iter().filter(|k| k.lead.is_some()).count();
        let ch = b.kept_ends.iter().filter(|k| !k.choices.is_empty()).count();
        eprintln!("T3DBG round {round}: kept ends {}, with a lead {leads}, with start points {ch}; dangling roots {}, with start points {}", b.kept_ends.len(), exist_dang.len(), lead_alt.len());
    }
    // 4.3: the length the search prices (`[search] length`).
    st.taut_len = opts.search_length == "taut";
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
    st.exit_taut = opts.exit_taut;
    let ws0 = (model.rules.default.width + model.rules.default.clearance) as f64;
    let tier_of = |i: usize| dec.connections[i].tier;
    // 4.3 `[optimize] initial = "once"`: a crossing on one layer is a violation (very high price,
    // the wire is still made), and a wire with no legal path is placed anyway (walls at that price).
    let once = opts.initial == "once";
    let viol_mm = reg.price(&Event::Violation { kind: String::new(), at: Vec2::default(), layer: String::new(), depth_nm: 0.0, len_nm: 0.0, against: String::new(), cause: String::new() }, &WireCtx::default());
    let weights = |rg: &Registry, c: &Conn, tier: u8, cap: usize| {
        let w = InitWeights { max_expansions: cap, ..InitWeights::from_registry(rg, &WireCtx { net: c.net, tier }) };
        if once {
            InitWeights { same_layer_mm: viol_mm, ..w }
        } else {
            w
        }
    };
    let len_of = |c: &Conn| (proj.pos[c.b] - proj.pos[c.a]).len();
    let mut order: Vec<usize> = (0..conns.len()).collect();
    order.sort_by(|&i, &j| {
        tier_of(conn_of[i]).cmp(&tier_of(conn_of[j])).then(prio_w[j].total_cmp(&prio_w[i])).then(len_of(&conns[i]).total_cmp(&len_of(&conns[j]))).then(i.cmp(&j))
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
    // exist-term: every vertex of the existing copper a connection may start at is an end too.
    for (net, vs) in exist_groups.values() {
        for &v in vs {
            if let VertKind::Centre(o) = proj.vkind[v] {
                pad_verts.entry(*net).or_default().push((v, o));
            }
        }
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
    let t_rad = Instant::now();
    let rad: Vec<f64> = if tree_free {
        (0..proj.pos.len())
            .map(|v| if matches!(proj.vkind[v], VertKind::Free | VertKind::Node) { proj.free_radius(proj.pos[v], i32::MIN).min(shape_dist(proj.pos[v])) } else { 0.0 })
            .collect()
    } else {
        Vec::new()
    };
    tlog("free radius of the junction candidates", t_rad);
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
                let mut src = tree::ends_of(st, &ecx, c.net, &comp, ca, c.w, c.s, [c.a, c.b]);
                let mut dst = tree::ends_of(st, &ecx, c.net, &comp, cb, c.w, c.s, [c.a, c.b]);
                // exist-term: a connection whose planned end is a dangling end (the decomposition
                // picked it as the cluster's nearest port) continues that end: the search starts
                // at its lead alone, not at another vertex of the cluster that may be cheaper
                // (test a_dangling_end_the_decomposition_picked_stays_the_root_behind_a_wall).
                // tw4 T3 (`dangling_lead_choice`): at any start point its lead offers. The lead up to
                // it is not priced (as the fixed lead: it continues the hand track); the search
                // prices the length from it and the `dangling_turn` there, so a lead pointing
                // past its target is cut where going on costs more than turning.
                for (v, ends) in [(c.a, &mut src), (c.b, &mut dst)] {
                    if !exist_dang.contains(&v) {
                        continue;
                    }
                    match lead_alt.get(&v) {
                        Some(alt) => ends.retain(|e| alt.iter().any(|x| x.0 == e.v)),
                        None => ends.retain(|e| e.v == v),
                    }
                }
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
                if std::env::var_os("TOPO_T3DBG").is_some() {
                    // Debug (tw4 T3): the start a continued dangling end's search took on its lead.
                    for (v, e) in [(c.a, sa), (c.b, sb)] {
                        if let Some(alt) = lead_alt.get(&v) {
                            let t = alt.iter().find(|x| x.0 == e.v).map_or(-1.0, |x| x.1);
                            let l = alt.last().map_or(0.0, |x| x.1);
                            eprintln!("T3DBG net {} end ({:.3}, {:.3}) lead {:.3} mm start t {:.3} mm{}", c.net, proj.pos[v].x / 1e6, -proj.pos[v].y / 1e6, l / 1e6, t / 1e6, if t < l - 1.0 { " SHORTENED" } else { "" });
                        }
                    }
                }
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
    let route_one_raw = |st: &mut State, ci: usize, cap: usize, wire_conn: &mut Vec<usize>, rg: &Registry, wall: f64| -> bool {
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
            w.wall_mm = wall;
            let Some(wid) = insert_conn(st, c, &w) else { return false };
            if wire_conn.len() <= wid {
                wire_conn.resize(wid + 1, usize::MAX);
            }
            wire_conn[wid] = ci;
            let straight = (proj.pos[st.wires[wid].b] - proj.pos[st.wires[wid].a]).len().max(1.0);
            let len: f64 = st.wire_len(wid);
            if opts.detour_factor <= 0.0 || len <= opts.detour_factor * straight || attempt == 2 {
                return true;
            }
            tree::remove_wire(st, wid);
        }
        true
    };
    // A connection whose sleeve the drawing rejected earlier pays a history on its old edges.
    let route_one_w = |st: &mut State, ci: usize, cap: usize, wire_conn: &mut Vec<usize>, rg: &Registry, wall: f64| -> bool {
        let Some(own) = mem.own.get(&conn_of[ci]) else { return route_one_raw(st, ci, cap, wire_conn, rg, wall) };
        let saved = st.history.clone();
        for (&e, &n) in own {
            st.history[e] += n;
        }
        let r = route_one_raw(st, ci, cap, wire_conn, rg, wall);
        st.history = saved;
        r
    };
    let route_one = |st: &mut State, ci: usize, cap: usize, wire_conn: &mut Vec<usize>, rg: &Registry| -> bool { route_one_w(st, ci, cap, wire_conn, rg, 0.0) };
    // 4.3 never fail (`initial = "once"`): the search, the search with four times the cap, the
    // search through walls at the `violation` price, then any sequence of triangles; the wire is
    // always made (it is the optimiser's to improve). Each connection placed without a legal
    // path is listed in `forced` (reported). Then its double crossings are removed (Polubasov
    // 2010: after every wire). False only when the two pads lie in parts of the board no edge
    // joins.
    let forced: std::cell::RefCell<BTreeSet<usize>> = std::cell::RefCell::new(BTreeSet::new());
    let place = |st: &mut State, ci: usize, cap: usize, wire_conn: &mut Vec<usize>, rg: &Registry| -> bool {
        let n0 = st.wires.len();
        let ok = route_one(st, ci, cap, wire_conn, rg) || route_one(st, ci, cap * 4, wire_conn, rg) || {
            let r = route_one_w(st, ci, cap * 4, wire_conn, rg, viol_mm) || {
                let c = &conns[ci];
                match st.any_path(c) {
                    Some((path, tris)) => {
                        let wire = super::state::Wire { net: c.net, w: c.w, s: c.s, a: c.a, b: c.b, pa: c.pa, pb: c.pb, turns: Vec::new(), passes: Vec::new(), tris: Vec::new(), alive: true };
                        let wid = st.insert(wire, &path, tris);
                        if wire_conn.len() <= wid {
                            wire_conn.resize(wid + 1, usize::MAX);
                        }
                        wire_conn[wid] = ci;
                        true
                    }
                    None => false,
                }
            };
            if r {
                forced.borrow_mut().insert(conn_of[ci]);
            }
            r
        };
        if ok && st.wires.len() > n0 {
            let (_, touched) = st.uncross_from_touched(st.wires.len() - 1, 1000);
            // 4.3c: the wires of a double-crossing removal are laid anew, kept when cheaper
            // (Polubasov 2010, «Начальная трассировка»).
            if st.relay_after_uncross && !touched.is_empty() {
                let wc = wire_conn.clone();
                let conn_of = |w: usize| {
                    let ci = wc.get(w).copied().filter(|&ci| ci != usize::MAX)?;
                    let c = conns[ci];
                    Some((c, weights(rg, &c, tier_of(conn_of[ci]), cap), (c.w + c.s) / ws0))
                };
                for (old, new) in st.relay(&touched, 1000, &conn_of) {
                    if wire_conn.len() <= new {
                        wire_conn.resize(new + 1, usize::MAX);
                    }
                    wire_conn[new] = wire_conn[old];
                }
            }
        }
        ok
    };
    let cap = opts.max_expansions.max(1);
    // Fluctuating coefficients of this cycle (M-B0 schedule) also in the initial search.
    let reg_p2 = if opts.fluctuation > 0.0 { reg.fluctuated(seed_of(opts.seed, round, 0), opts.fluctuation) } else { reg.clone() };
    // 4.3 `initial = "once"`: a later cycle starts from the plan of the cycle before and
    // improves it: the connections whose drawing broke a rule (their stretch's history,
    // `Memory::own`) and the wires through a passage the drawing found too narrow (a lowered
    // room, `hints`) are taken out and placed again, the rest stays.
    let carried = once && mem.plan.is_some();
    let mut p2_replaced = 0usize;
    if let Some(pl) = mem.plan.as_ref().filter(|_| once) {
        st.restore(pl.topo.clone());
        wire_conn = pl.wire_conn.clone();
        let mut vic: BTreeSet<WireId> = BTreeSet::new();
        for (w, wi) in st.wires.iter().enumerate().filter(|(_, wi)| wi.alive) {
            let Some(&ci) = wire_conn.get(w).filter(|&&c| c != usize::MAX) else { continue };
            if mem.own.contains_key(&conn_of[ci]) {
                vic.insert(w);
                continue;
            }
            let narrow = wi.passes.iter().any(|&p| {
                let e = st.passes[p].edge;
                hints.iter().any(|&(he, _, r)| he == e && st.edge_need(e, None) > r)
            });
            if narrow {
                vic.insert(w);
            }
        }
        let deps = tree::dependents(&st, &vic);
        vic.extend(deps);
        let mut vic_conns: Vec<usize> = vic.iter().map(|&w| wire_conn[w]).collect();
        vic_conns.sort_by_key(|&ci| order.iter().position(|&x| x == ci));
        for &w in &vic {
            tree::remove_wire(&mut st, w);
        }
        p2_replaced = vic_conns.len();
        for ci in vic_conns {
            if cut_here() {
                timed_out = true;
                fail(&mut out, conn_of[ci], if stop_now() { "Cancel / OK came before this connection was routed again" } else { "time limit reached before this connection was routed again" });
                continue;
            }
            if !place(&mut st, ci, cap, &mut wire_conn, &reg_p2) {
                p2_failed.push(ci);
            }
        }
    } else if draw_row.is_none() {
        for &ci in &order {
            if cut_here() {
                timed_out = true;
                fail(&mut out, conn_of[ci], if stop_now() { "Cancel / OK came before this connection was routed" } else { "time limit reached before this connection was routed" });
                continue;
            }
            let ok = if once { place(&mut st, ci, cap, &mut wire_conn, &reg_p2) } else { route_one(&mut st, ci, cap, &mut wire_conn, &reg_p2) };
            if !ok {
                p2_failed.push(ci);
            }
        }
    }
    let swaps = st.uncross_all(50);
    // One retry of the failed connections with a larger search cap.
    let mut still: Vec<usize> = Vec::new();
    for &ci in &p2_failed {
        if cut_here() || !route_one(&mut st, ci, cap * 4, &mut wire_conn, &reg_p2) {
            still.push(ci);
        }
    }
    if still.len() < p2_failed.len() {
        st.uncross_all(50);
    }
    for &ci in &still {
        let why = if once { "no topological path between the pads: they lie in parts of the board no edge of the projection joins" } else { "no topological path between the pads (search exhausted); the pad is enclosed or the retry cap was reached" };
        fail(&mut out, conn_of[ci], why);
    }
    debug_assert!(st.check().is_ok(), "{:?}", st.check());
    ctl.update("route", 1, 0, nconn, 0, t0.elapsed().as_secs_f64());
    // limit-p3p4: every connection has its place now (the first plan): a stop from here on keeps
    // the plan so far and draws it, so the dialog says that Accept is useful.
    ctl.stage(rnd, rnds, if plan_cut.get() { "layer assignment, rip-up and reroute" } else { PLAN_READY_STAGE });
    plan_span.set(true);
    let plan_scope = crate::control::step_scope();
    ctl.test_point("plan ready");
    let p2_s = t_p2.elapsed().as_secs_f64();
    tlog("P2 initial routing", t_p2);
    // ---- P3 ----
    let lp = LayerParams {
        via_mm: reg.price(&Event::Via { wire: 0, count: 1, at: None }, &WireCtx::default()),
        via_site_r: crate::vias::plan_via_need(&model.rules, opts),
        site_price: opts.via_site_price == "graded",
        n_signal: b.n_signal,
        plane_mm: reg.price(&Event::PlaneLength { wire: 0, layer: b.n_signal, mm: 1.0 }, &WireCtx::default()),
        plane_cut: reg.price(&Event::PlaneCut { wire: 0, layer: b.n_signal }, &WireCtx::default()),
        // TOPO_NOCAP=1 (debug): assign layers for vias only, to compare with the capacity refinement.
        cap_mm: if std::env::var("TOPO_NOCAP").is_ok() { 0.0 } else { reg.price(&Event::Overflow { wire: 0, edge: 0 }, &WireCtx::default()) },
        cap_vias: std::env::var("TOPO_CAPVIA").is_ok(),
        via_between: opts.via_between,
        cap_reference: opts.refine_reference,
        omega: layers::Omega {
            a_w: opts.via_priority_weight.first().copied().unwrap_or(0.0).max(0.0),
            a_t: opts.via_priority_weight.get(1).copied().unwrap_or(0.0).max(0.0),
            w_def: model.rules.default.width as f64,
        },
        ..LayerParams::default()
    };
    let sub = super::capacity::subset_check(&st);
    let mut packer = Packer::for_model(model, opts, proj);
    packer.via_squeeze = reg.price(&Event::ViaSqueeze { wire: 0, at: Vec2::default(), slack_nm: 0.0, via_nm: 1.0 }, &WireCtx::default());
    if packer.site_price {
        packer.lead_ends = b.kept_ends.iter().filter(|k| k.lead.is_some() && k.via_end.is_some()).cloned().collect();
    }
    let packer = packer;
    // P3 with the junctions' tee rule: the trunk keeps one layer through a junction and the
    // branch ends on it. Two layers: inside P3, a tee is one layer domain (D6b); more layers: a
    // fix-up after P3 (D5).
    // `ties` != 0: a random choice among the equal layerings (the TopoR loop, 5.2); 0 = as before.
    // loop-local Step A (profile F9): the untouched wires' stretches and via sites are taken from
    // the last layering instead of searched again (exact, [`layers::StretchCache`]);
    // TOPO_NOSCACHE=1 (debug) switches it off for an A/B check.
    let scache = layers::StretchCache::default();
    let scache_on = std::env::var_os("TOPO_NOSCACHE").is_none();
    let scache_ref = scache_on.then_some(&scache);
    // loop-local: the deterministic work counters of the TopoR loop (whole-board layerings and
    // measures, local layerings and their sizes, the shadow check), read per cycle.
    let lctr = LoopCtr::default();
    // TW5 (b): `walk` > 0 = the layering escape's plateau walk (tries per domain).
    // TW5 (e): the escape's capacity refinement may add a via when it removes more over-full price
    // than the via costs, as the P4 label moves the loop replaces do (`add_vias: priced && cut_vias`
    // there; the pre-loop P4 rounds are off by default with the TopoR loop). Without it a wire that
    // crosses nothing (no event to relabel) never leaves an over-full cut: inner_layer_room with its
    // declared layers, N 0 of 10 seeds at DRC 0 against 3 of 10 with the P4 rounds (tw5r3).
    let esc_cap_vias = (packer.cuts.is_some() || opts.edges_test != "refuse") && opts.cut_vias;
    let assign_tw = |st: &State, ties: u64, walk: usize| {
        lctr.layer_global.set(lctr.layer_global.get() + 1);
        let lp = LayerParams { ties, walk, cap_vias: lp.cap_vias || (walk > 0 && esc_cap_vias), ..lp };
        let mut lr = layers::assign_cached(st, &|ob| b.smd_layer(ob), &lp, Some(&packer), scache_ref);
        LoopCtr::add(&lctr.evals, lr.refine_stream.0);
        lctr.spend(super::escape::cost::global(st.wires.iter().filter(|w| w.alive).count(), lr.refine_stream.0, st.passes.len()));
        if tree_free && proj.n_layers != 2 && st.wires.iter().any(|w| w.alive && !w.turns.is_empty()) {
            let mut evl = lr.events.clone();
            let bottom_l = (if b.n_signal == 0 { proj.n_layers } else { b.n_signal }) as i32 - 1;
            evl.resize(st.wires.len(), Vec::new());
            lr.mask.resize(st.wires.len(), Vec::new());
            lr.partner.resize(st.wires.len(), Vec::new());
            tree::fix_layers(st, &mut evl, &mut lr.mask, &mut lr.partner, bottom_l);
            layers::relabel_cached(st, &evl, &lp, &mut lr, scache_ref);
        }
        lr
    };
    let assign_t = |st: &State, ties: u64| assign_tw(st, ties, 0);
    let assign_esc = |st: &State, ties: u64| assign_tw(st, ties, opts.escape_walk);
    // loop-local Step B: the layering of a state whose wires outside `held.free` keep the layers of
    // `held.prev` ([`layers::assign_local`]); the junction fix-up only on the free trees.
    let assign_l = |st: &State, ties: u64, held: &layers::Held| {
        lctr.layer_local.set(lctr.layer_local.get() + 1);
        let lp = LayerParams { ties, ..lp };
        let mut lr = layers::assign_local(st, &|ob| b.smd_layer(ob), &lp, Some(&packer), scache_ref, held);
        LoopCtr::add(&lctr.evals, lr.refine_stream.0);
        lctr.spend(super::escape::cost::local(lr.refine_stream.0, 0));
        if tree_free && proj.n_layers != 2 && st.wires.iter().enumerate().any(|(w, wi)| wi.alive && !wi.turns.is_empty() && held.is_free(w)) {
            let mut evl = lr.events.clone();
            let bottom_l = (if b.n_signal == 0 { proj.n_layers } else { b.n_signal }) as i32 - 1;
            evl.resize(st.wires.len(), Vec::new());
            lr.mask.resize(st.wires.len(), Vec::new());
            lr.partner.resize(st.wires.len(), Vec::new());
            tree::fix_layers_only(st, &mut evl, &mut lr.mask, &mut lr.partner, bottom_l, Some(held.free));
            layers::relabel_cached(st, &evl, &lp, &mut lr, scache_ref);
        }
        lr
    };
    let assign = |st: &State| assign_t(st, 0);
    // Crossing directions for the capacity model's oblique factor: the taut paths (pack.rs).
    let dirs_of = |st: &State| -> Vec<Vec<Vec2>> { packer.taut_dirs(st) };
    let mut lr = assign(&st);
    if let Some(r) = crep.as_mut() {
        r.static_sheets(proj, p2_s);
        let bottom_l = (if b.n_signal == 0 { proj.n_layers } else { b.n_signal }) as i32 - 1;
        r.stage("after P3", &st, &lr.events, &packer, bottom_l, &|w| wire_conn.get(w).filter(|&&c| c != usize::MAX).map(|&c| conn_of[c]));
    }
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
    // The P4 pick follows the board objective: the objective's own value (`length + mm_per_via x
    // vias`, or the occupied area in mm of default trace) plus the graded price of the predicted
    // violations (M0: one price S = sum k_i x_i, Polubasov 2010 «Оптимизация разводки»; the
    // archive judges the variants). `[capacity] edges_test = "refuse"` (per-edge model only) keeps
    // the pick before M0: the fewest over-full edge-layers first, then the objective (S3,
    // 3814139, measured then: one sum routed fewer connections on bornhack2026; that was a
    // monotone gate, no-monotone-gates).
    let obj = Objective::from_options(&opts.objective, opts.mm_per_via, &opts.pick).with_best_penalty(opts.best_penalty);
    let overflow_mm = reg.price(&Event::Overflow { wire: 0, edge: 0 }, &WireCtx::default()).max(1e-6);
    let via_area_mm = {
        let r = crate::vias::plan_via_need(&model.rules, opts);
        std::f64::consts::PI * r * r * b.signal_layers.len().max(1) as f64 / ws0 / MM
    };
    // `[capacity] model = "cuts"` (shopping list 3.2): P4 is priced by the per-layer cuts: the
    // round's victims come from the over-full cuts, the moves keep their changes on price alone
    // (no hard capacity tests) and the pick of the best round is one graded sum (the objective
    // plus the over-full price of the cuts, vias without a site and unlayerable crossings at the
    // `overflow` weight; design docs/dev/inflated-sketch.md §4.4). The count stays in the report.
    let cuts_on = packer.cuts.is_some();
    // M0: the over-full edge-layers (per-edge model) or cuts (cut model) are priced, never a hard
    // gate, unless `[capacity] edges_test = "refuse"` asks for the per-edge hard test before M0.
    let priced = cuts_on || opts.edges_test != "refuse";
    let score = |st: &State, lr: &layers::LayerReport| -> Score {
        let dirs = dirs_of(st);
        let cm = if cuts_on { CapModel::new_cut(st, &packer, &dirs, &lr.events, bottom, overflow_mm) } else { CapModel::new(st, &packer, &dirs, &lr.events, bottom, overflow_mm) };
        let (mut len_mm, mut area_mm) = (0.0, 0.0);
        for w in (0..st.wires.len()).filter(|&w| st.wires[w].alive) {
            let l = st.wire_len(w) / MM;
            len_mm += l;
            area_mm += l * (st.wires[w].w + st.wires[w].s) / ws0;
        }
        let own = match obj {
            Objective::LengthVias { mm_per_via } | Objective::ErrorsViasLength { mm_per_via } | Objective::Best { mm_per_via, .. } => len_mm + mm_per_via * lr.vias_w,
            Objective::Area => area_mm + via_area_mm * lr.vias_w,
        };
        // Predicted violations: over-full edge-layers, vias without a site, (M-B2) crossings
        // that cannot be layered (both stretches held to one layer: a short when drawn) and
        // (Q14b) traversals through foreign copper (a short when drawn).
        let fc = super::optimise::foreign_copper(st, &super::optimise::layers_now(st, &lr.events, bottom));
        let graded = if priced { cm.total() + overflow_mm * (lr.unlayerable.len() as f64 + fc.1) } else { 0.0 };
        let shorts = if opts.pick_shorts { lr.unlayerable.len() + fc.2 } else { 0 };
        (own, cm.overfull() + lr.unlayerable.len() + fc.0, lr.vias, graded, shorts)
    };
    // One graded sum (priced: the objective plus the over-full price, where a predicted short
    // is already priced at least one violation and `[optimize] short_weight` k adds (k - 1)
    // more at the `overflow` weight, PS1), or lexicographic: the ranked predicted violations (a
    // predicted short counted k times), then the predicted shorts among them as a tie-break (as
    // in the archive pick), then the objective. k = 1 adds nothing to either.
    let short_extra = if opts.pick_shorts { opts.short_weight.max(1.0) - 1.0 } else { 0.0 };
    let p4_ranked = |s: &Score| s.1 as f64 + short_extra * s.4 as f64;
    let p4_better = |a: &Score, b: &Score| {
        if priced {
            let extra = |s: &Score| short_extra * overflow_mm * s.4 as f64;
            return a.0 + a.3 + extra(a) < b.0 + b.3 + extra(b) - 1e-6;
        }
        let (ra, rb) = (p4_ranked(a), p4_ranked(b));
        ra < rb - 1e-9 || ((ra - rb).abs() <= 1e-9 && (a.4 < b.4 || a.4 == b.4 && a.0 < b.0 - 1e-6))
    };
    // Reroute counter per connection: the P4 moves pay the `reroute` factor's price for it.
    // (Measured: ripping the least moved wires first in the negotiated rip-up routed fewer
    // connections on a 2-layer badge and qmtech, so the rip order stays routed-last first.)
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
    let xstats = std::cell::Cell::new([0usize; 4]); // found, tried, resolved, passes kept
    let xadd = |i: usize, n: usize| {
        let mut x = xstats.get();
        x[i] += n;
        xstats.set(x);
    };
    let crossing_pass = |st: &mut State, wire_conn: &mut Vec<usize>, reroutes: &mut Vec<u32>, lr: &mut layers::LayerReport, ripped_total: &mut usize, stage: usize| {
        let _pg = super::prof::g(super::prof::K::CrossingPass);
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
        let pd = super::prof::g(super::prof::K::XrDetect);
        let mut pairs: Vec<super::xrules::Pair> = super::xrules::terminal_clinches(st);
        pairs.extend(super::xrules::double_crossings(st));
        for &(a, b) in &lr.unlayerable_pairs {
            pairs.push(super::xrules::Pair { first: a.min(b), second: a.max(b), kind: super::xrules::Kind::Unlayerable });
        }
        xadd(0, pairs.len());
        let pairs: Vec<_> = pairs.into_iter().filter(|p| plain(st, wire_conn, p.first) && plain(st, wire_conn, p.second)).collect();
        drop(pd);
        if pairs.is_empty() {
            return;
        }
        let pj = super::prof::g(super::prof::K::XrJudge);
        let before = score(st, lr);
        let saved = (st.clone(), wire_conn.clone(), reroutes.clone(), lr.clone(), *ripped_total);
        drop(pj);
        let pr = super::prof::g(super::prof::K::XrReplan);
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
            over_mm: priced.then_some(overflow_mm),
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
            xadd(1, 1);
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
        drop(pr);
        if resolved == 0 {
            return;
        }
        let _pj = super::prof::g(super::prof::K::XrJudge);
        st.uncross_all(50);
        let lr2 = assign(st);
        let after = score(st, &lr2);
        if p4_better(&after, &before) {
            *wire_conn = topo.wire_conn;
            *reroutes = topo.reroutes;
            *ripped_total = topo.ripped;
            *lr = lr2;
            xadd(2, resolved);
            xadd(3, 1);
        } else {
            (*st, *wire_conn, *reroutes, *lr, *ripped_total) = saved;
        }
    };
    crossing_pass(&mut st, &mut wire_conn, &mut reroutes, &mut lr, &mut ripped_total, 0);
    let first_score = score(&st, &lr);
    let mut best_p4 = (st.clone(), wire_conn.clone(), lr.clone(), first_score);
    let (mut p4_rounds, mut p4_stall, mut p4_ripped) = (0usize, 0usize, 0usize);
    // TW5 (e) `[optimize] preloop_p4` (D9): with the TopoR loop the negotiated rip-up rounds
    // before it are folded into the loop (its fading history on over-full edges and the layering
    // escapes do their work, docs/dev/design/tw5-escape.md); `true` runs them as before TW5.
    // TW3: a draw-only round takes its row's plan (no rounds).
    let p4_rounds_max = if draw_row.is_some() || (opts.opt_loop == "topor" && !opts.preloop_p4) { 0 } else { opts.reroute_rounds };
    for _round in 0..p4_rounds_max {
        if expired() {
            timed_out = true;
            break;
        }
        if ripped_total >= opts.max_ripped {
            break;
        }
        let dirs = dirs_of(&st);
        let cm = if cuts_on { CapModel::new_cut(&st, &packer, &dirs, &lr.events, bottom, 1.0) } else { CapModel::new(&st, &packer, &dirs, &lr.events, bottom, 1.0) };
        // First P4 round: the wires whose planned via count rose against the cycle before are
        // rerouted first (Polubasov 2010, «Оптимизация расслоения»), once per cycle.
        let mut pre: BTreeSet<WireId> = BTreeSet::new();
        if p4_rounds == 0 {
            for (w, ci) in via_rose.iter().copied() {
                if st.wires[w].alive && reroutes[ci] == 0 {
                    pre.insert(w);
                }
            }
        }
        let Some(victims) = p4_victims(&st, &cm, &rank, &wire_conn, &pre, ws0, Some(&mut history)) else { break };
        let victims: BTreeSet<WireId> = victims.into_keys().collect();
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
            let ok = !expired() && if once { place(&mut st, ci, cap, &mut wire_conn, &reg_r) } else { route_one(&mut st, ci, cap, &mut wire_conn, &reg_r) || route_one(&mut st, ci, cap * 4, &mut wire_conn, &reg_r) };
            if !ok {
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
        if let Some(r) = crep.as_mut() {
            r.stage(&format!("after P4 round {p4_rounds}"), &st, &lr.events, &packer, bottom, &|w| wire_conn.get(w).filter(|&&c| c != usize::MAX).map(|&c| conn_of[c]));
        }
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
            xstats.get()[0], xstats.get()[1], xstats.get()[2], xstats.get()[3]
        ));
    }
    tlog("P4 rip-up and reroute", t_p4);
    // ---- P4 moves: descent on the topology, then on the layer labels ----
    let mut tally = Tally::default();
    let mut term_over: BTreeMap<usize, (TermRef, TermRef)> = mem.plan.as_ref().filter(|_| once).map(|p| p.term_over.clone()).unwrap_or_default();
    // The topological moves re-search whole pad-to-pad connections; they do not know branch
    // points yet (D5), so a state with branch points skips them.
    let has_junctions = tree_free && st.wires.iter().any(|w| w.alive && !w.turns.is_empty());
    if opts.moves && has_junctions {
        warnings.push("P4 moves skipped: the net trees have branch points on traces, which the moves do not handle yet ([search] net_tree = \"fixed\" to use them)".into());
    }
    if opts.moves && opts.max_moves > 0 && !expired() && !has_junctions && draw_row.is_none() {
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
                let mut t = vec![false; proj.edges.len()];
                if cuts_on {
                    // The projection edges where the wires of an over-full cut cross it.
                    let cm = CapModel::new_cut(&st, &packer, &dirs, &lr.events, bottom, overflow_mm);
                    if let Some(c) = cm.cut.as_ref() {
                        for (l, ci, _) in c.overfull_list() {
                            for e in item_edges(&st, &c.items_at(l, ci)) {
                                t[e] = true;
                            }
                        }
                    }
                } else {
                    let cm = CapModel::new(&st, &packer, &dirs, &lr.events, bottom, overflow_mm);
                    for (e, _) in cm.overfull_list() {
                        t[e] = true;
                    }
                }
                t
            },
            over_mm: priced.then_some(overflow_mm),
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
        let cm = if cuts_on { CapModel::new_cut(&st, &packer, &dirs, &lr.events, bottom, overflow_mm) } else { CapModel::new(&st, &packer, &dirs, &lr.events, bottom, overflow_mm) };
        let (v0, o0) = (lr.vias, cm.overfull());
        let (mask, partner) = (lr.mask.clone(), lr.partner.clone());
        let mut lb = Labels {
            st: &st,
            cm,
            evl: lr.events.clone(),
            mask: &mask,
            partner: &partner,
            bottom,
            via_mm: lp.via_mm,
            omega: lp.omega,
            n_signal: n_sig,
            evals: 0,
            max_evals: opts.max_moves,
            strict: !priced,
            // Priced label moves (cut model; per-edge model with edges_test = "price", M0) may add
            // a via when it removes over-full price worth more than the via ([capacity] cut_vias).
            add_vias: priced && opts.cut_vias,
            tally: Tally::default(),
        };
        let lt = descend_labels(&mut lb);
        // 5.4 `[vias] pair_min`: the local via-pair minimisation (Luzin-Popov 2013) after the
        // label descent.
        let pairs_removed = if opts.via_pair_min {
            let max_len = opts.via_pair_max_mm.max(0.0) * MM;
            let mut n = 0;
            for w in lb.candidates() {
                let ev = lb.evl[w].clone();
                n += lb.pair_lifts(w, max_len, &|w2, i, j| super::viaopt::stretch_len(&st, w2, if w2 == w { &ev } else { &lr.events[w2] }, i, j));
            }
            Some(n)
        } else {
            None
        };
        let o1 = lb.cm.overfull();
        let evl = std::mem::take(&mut lb.evl);
        drop(lb);
        layers::relabel_cached(&st, &evl, &lp, &mut lr, scache_ref);
        if let Some(n) = pairs_removed {
            warnings.push(format!("local via-pair minimisation ([vias] pair_min; Luzin, Popov & Popov 2013): {n} via pair(s) removed by moving the stretch between them onto its neighbours' layer"));
        }
        tally.add(&lt);
        warnings.push(format!("P4 moves on the layers: {}; vias {} -> {}, over-full (edge-layers + vias without a site) {} -> {}", lt.text(), v0, lr.vias, o0, o1));
        tlog("P4 moves", t_mv);
    }
    // ---- 5.1 `[optimize] loop = "topor"`: the TopoR loop on the topology (shopping list 5.1–5.3,
    // 5.5; [`super::optimise`]). After the initial routing (once) and its rip-up rounds: cycles
    // that change only the topology (single-wire reroutes with per-wire coefficients, layer
    // re-optimisation with a random choice among equal layerings, the wires whose vias rose laid
    // again, floating up, the crossing rules), every cycle's plan judged by its predicted criteria
    // and kept in an archive of topological variants with restarts from it; the pick goes on to
    // the one drawing below. No drawing inside the loop, nothing fed back from a drawing.
    let mut loop_end: Option<LoopEnd> = None;
    let mut loop_json: Option<serde_json::Value> = None;
    let (mut drew_row, mut extra_rows): (Option<Assigned>, Vec<Assigned>) = (None, Vec::new());
    // M2.TL: the initial routing (P2-P4) has ended; its plan is the run's first variant (the TopoR
    // loop's generation 0, or this whole-route cycle's plan). Cancel / OK during it (P2 before
    // every connection had its place, or the layer assignment and rip-up rounds of P3/P4, which
    // M0's stop checks end within a move): the step did not end, so it kept no variant (the user's
    // rule: the plan counts once P4 ends); the run keeps what it had (nothing, or the earlier
    // cycles' variants), never half a step.
    // limit-p3p4 (F10, user 2026-11-23: "Accept takes the best result so far, and must always have
    // something to draw once the first plan exists"): a Cancel / OK during P3/P4 (after P2 gave
    // every connection its place) keeps the plan so far: P2's topology with the best layering
    // found (a refinement the stop cut keeps every move it made: each one lowered the price; the
    // crossing pass and the rip-up rounds restore their best). The TopoR loop below ends at once
    // and draws it. Cancel / OK during P2 keeps nothing (there is no first plan yet).
    drop(plan_scope);
    let plan_bounded = plan_over();
    plan_span.set(false);
    if plan_cut.get() {
        let why = "Cancel / OK came during the initial routing, before every connection had its place in the plan";
        return (RoundOut::not_drawn(model, opts, None, Some(why.to_string()), None), Vec::new(), Memory::default());
    }
    if stop_now() && opts.opt_loop != "topor" && round > 1 {
        // `[optimize] loop = "cycles"`: the earlier cycles' drawn variants are the best so far; this
        // cycle's plan is not complete and is dropped (as before limit-p3p4).
        let why = "Cancel / OK came during the initial routing (layer assignment, rip-up and reroute) of a later whole-route cycle; the earlier cycles' variants are kept";
        return (RoundOut::not_drawn(model, opts, None, Some(why.to_string()), None), Vec::new(), Memory::default());
    }
    if stop_now() {
        warnings.push(PLAN_KEPT_STOP.to_string());
        if opts.opt_loop != "topor" {
            // No loop follows: the kept plan is drawn in full (the TopoR loop sets this for its pick).
            draw_through.set(true);
        }
    } else if plan_bounded {
        timed_out = true;
        warnings.push(format!("{PLAN_KEPT_LIMIT} ([run] limit_grace_s = {} s)", ctl.limit.grace_s().map_or("unbounded".to_string(), secs_text)));
    }
    let topor = opts.opt_loop == "topor";
    if topor {
        use super::optimise::{criteria_of, evaluation, foreign_copper, foreign_copper_only, k_osc, layers_now, layers_now_only, optimise_shared, reroute_set, LoopReport, Shared, wire_amp, CycleCtx, CycleStats, Emphasis, Gen, LoopParams, Predicted, Rerouter, Space, VIA_FACTORS};
        let t_loop = Instant::now();
        let t_loop_c = ctl.limit.now(); // pause-button: the loop's share counts on the run's clock
        /// TW5 (a): the factor index of the violation weight's own oscillation (apart from the
        /// registry's factors).
        const VIOL_FACTOR: usize = 1 << 20;
        // The loop's own emergency stop: a share of the time left at the loop's start (the rest is
        // the drawing's). time-limit-fix: read live, so a limit the user changes during the loop
        // (the progress dialog's Time limit, +5 / +15 min) moves the share with it; before, the
        // share kept the limit as it stood at the loop's start.
        // time-limit-fix r2: share >= 1 (the default) = no extra stop, the loop runs to the limit (the
        // final drawing after it is not counted, `[run] timeout_scope = "optimizing"`).
        let share = opts.loop_time_share.clamp(0.05, 1.0);
        let loop_deadline = || if share >= 1.0 { None } else { ctl.deadline().map(|d| t_loop_c + d.saturating_duration_since(t_loop_c).mul_f64(share)) };
        let n_sig = if b.n_signal == 0 { proj.n_layers } else { b.n_signal };
        let tiers: Vec<u8> = (0..conns.len()).map(|ci| tier_of(conn_of[ci])).collect();
        // 5.2 (price = criterion): the predicted violations of a state with its layering, the one
        // measure the archive counts (errors) and every loop move pays at the `violation` weight
        // ([`Predicted`]): the capacity model's over-full edge-layers (cut model: over-full cuts)
        // and vias without a site, graded by how far over; the crossings no layer separates; the
        // connections placed without a legal path; the traversals through foreign copper (Q14b,
        // [`foreign_copper`]: a short when drawn).
        // 2.4 (cut model): a cut that fits only necked is a shrinkage, no violation; its price
        // (`[factors] shrinkage`) is part of what a move pays, and the necked stretches its necks
        // add count as the archive's shrinkages (TopoR 6.3 manual pp. 72-73).
        // par-refine item 3 (`[optimize] measure_reuse`): the per-edge model's violations come from
        // the layer refinement's final model when it was taken for this packer, bottom layer,
        // price, state and labels ([`super::refine::FinalCap`]); otherwise the model is built.
        let reused = |st: &State, lr: &layers::LayerReport| -> Option<(usize, f64)> {
            let f = lr.fin_cap.filter(|_| opts.measure_reuse && !cuts_on)?;
            (f.packer == &packer as *const Packer as usize && f.bottom == bottom && f.price == overflow_mm.to_bits() && f.key == super::refine::cap_key(st, &lr.events)).then_some(f.viol)
        };
        // tw4 T1 (`[factors] neck_depth`): the neck-depth part of the measure, as a share of the
        // violation weight (graded only: price = viol_mm x graded).
        let neck_k = if viol_mm > 0.0 { reg.weight("neck_depth").unwrap_or(0.0).max(0.0) / viol_mm } else { 0.0 };
        // tw4-r2 (3) (`[capacity] pour_cuts`): the pour terminals the plan's copper cuts off
        // ([`super::pourplan`]), against the board's own cut-off pads before routing.
        let pour_base = opts.pour_cuts.then(|| super::pourplan::PourBase::new(model, opts.pour_cut_cells));
        let pour_of = |st: &State, evl: &[Vec<(usize, f64, i32)>]| -> (usize, f64) {
            match pour_base.as_ref() {
                Some(pb) if pb.any => super::pourplan::pour_cuts(model, &super::pourplan::plan_items(st, &packer, b, evl, bottom), pb),
                _ => (0, 0.0),
            }
        };
        // TW7 E (`[optimize] pattern_moves` "pour"): the cut-off pour pieces of the plan with the
        // wires bounding them (the movers of the pour move).
        let pour_comps_fn = |st: &State, evl: &[Vec<(usize, f64, i32)>]| -> Vec<super::pourplan::CutComp> {
            match pour_base.as_ref() {
                Some(pb) if pb.any => super::pourplan::cut_components(model, &super::pourplan::plan_items(st, &packer, b, evl, bottom), pb),
                _ => Vec::new(),
            }
        };
        let measure = |st: &State, lr: &layers::LayerReport| -> Predicted {
            let _pg = super::prof::g(super::prof::K::Measure);
            let pour = if pour_base.is_some() {
                super::pourplan::meter::GLOBAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                pour_of(st, &lr.events)
            } else {
                (0, 0.0)
            };
            let neck = if neck_k > 0.0 { neck_k * st.neck_depth(None) } else { 0.0 };
            // tw4 fix round 2 (verifier F2, test hook): the neck-depth part of the last whole-board measure.
            super::optimise::neckcheck::LAST.store(neck.to_bits(), std::sync::atomic::Ordering::Relaxed);
            let pred = |viol: (usize, f64)| Predicted::of(viol, lr.unlayerable.len(), forced.borrow().len()).with_foreign(foreign_copper(st, &layers_now(st, &lr.events, bottom))).with_neck(neck).with_pour(pour).shorts_ranked(opts.pick_shorts, opts.short_weight);
            if let Some(v) = reused(st, lr) {
                return pred(v);
            }
            LoopCtr::add(&lctr.measure_global, 1);
            lctr.spend(super::escape::cost::measure(st.passes.len()));
            let dirs = dirs_of(st);
            let cm = if cuts_on { CapModel::new_cut(st, &packer, &dirs, &lr.events, bottom, overflow_mm) } else { CapModel::new(st, &packer, &dirs, &lr.events, bottom, overflow_mm) };
            let p = pred(cm.violations());
            if !cuts_on {
                return p;
            }
            let necks = st.cut_neck_widths(&cm.cut_necks());
            let added: usize = (0..st.wires.len()).filter(|&w| st.wires[w].alive).map(|w| st.neck_stretches_with(w, &necks).saturating_sub(st.neck_stretches_with(w, &BTreeMap::new()))).sum();
            p.with_shrink(added, cm.shrinkage().2)
        };
        // plan-dots: the places of a plan's predicted violations, for its sketch (built only while
        // the caller shows sketches, [`Sp::plan_sketch`]): the measure's capacity model built
        // afresh (the refinement's final model, which the measure may reuse, is the same model),
        // the meters off; nothing of it feeds back into the run.
        let places = |st: &State, lr: &layers::LayerReport| -> super::compass::PlanPlaces {
            super::refine::meter::suspended(|| {
                let dirs = dirs_of(st);
                let cm = if cuts_on { CapModel::new_cut(st, &packer, &dirs, &lr.events, bottom, overflow_mm) } else { CapModel::new(st, &packer, &dirs, &lr.events, bottom, overflow_mm) };
                super::compass::plan_places(st, lr, &cm, bottom, forced.borrow().len(), &pour_comps_fn(st, &lr.events))
            })
        };
        // loop-local Step B (`[optimize] loop_judge = "local"`, docs/dev/design/loop-local.md): a
        // move that took the wires `gone` out of the state `saved` (layered as `prev`, measured as
        // `pred`) and laid `new` is layered locally and priced by what changes. F (the free wires) =
        // `new`, the different-net wires `gone` crossed (their layering's partners and unlayerable
        // pairs) and the wires `new` crosses, `[optimize] local_depth` rings deep, closed over the
        // junctions (a tee keeps one layer); every other wire keeps its events and labels
        // ([`layers::assign_local`]; Dayan 1997 pp. 55-57: "only the optimal candidate assignments
        // for the 2-Nets affected by this assignment are recomputed"). The predicted violations
        // change by the difference, before and after, of the capacity model over W (every wire with
        // a pass on an edge an F wire passes, before or after: on those edges the model is
        // complete, on the others W's passes are the same on both sides), of the foreign copper of
        // F, and by the exact unlayerable and forced counts; the vias are the new layering's.
        // Polubasov 2010 «Оптимизация расслоения»: the whole board is layered again once per
        // cycle (stage 2), the escape from the local optima of this local layering.
        let judge_local = opts.loop_judge != "global" && !cuts_on;
        if opts.loop_judge != "global" && cuts_on && opts.opt_loop == "topor" {
            warnings.push("TopoR loop: [optimize] loop_judge = \"local\" prices the per-edge capacity model only; with [capacity] model = \"cuts\" every move is judged by the whole board (loop_judge = \"global\")".to_string());
        }
        let local_depth = opts.local_depth.max(1);
        let local_check = std::env::var_os("TOPO_LOCALCHECK").is_some();
        let local_judge = |st: &mut State, saved: &super::state::Topology, gone: &[WireId], new: &[WireId], prev: &layers::LayerReport, pred: &Predicted, ties: u64| -> (layers::LayerReport, Predicted) {
            let nw = st.wires.len();
            // The wires the move changed besides the ones it took out and laid: laying a branch
            // turns its trunk at the junction (`State::add_turn`), taking one out drops that turn
            // (`tree::remove_wire`), a wire may die. Each counts as taken out and laid again, so
            // its own events and those of the wires it crosses are built afresh.
            let (mut gone_v, mut new_v) = (gone.to_vec(), new.to_vec());
            let mut changed = 0usize;
            for (w, a) in saved.wires().iter().enumerate() {
                if !a.alive || gone.contains(&w) || w >= nw {
                    continue;
                }
                let b = &st.wires[w];
                if !b.alive || a.tris != b.tris || a.turns != b.turns || a.passes.len() != b.passes.len() {
                    if local_check {
                        let how = if !b.alive { "died" } else if a.tris != b.tris { "path" } else if a.turns != b.turns { "turns" } else { "passes" };
                        eprintln!("LOOPLOCAL changed wire {w} (net {}): {how}; move gone {gone:?} new {new:?}", a.net);
                    }
                    changed += 1;
                    gone_v.push(w);
                    if b.alive {
                        new_v.push(w);
                    }
                }
            }
            // A double-crossing swap of the placing (`State::uncross_from_touched`, also when the
            // relay after it keeps the old lay) changes the pass order of the wires that took part
            // and so their crossings with held wires, path and passes unchanged: each is laid out
            // again too, or a held-held crossing would keep its stale events (an unpriced crossing).
            let mut took: Vec<WireId> = st.took_part.iter().copied().filter(|&w| w < nw && w < saved.wires().len() && saved.wires()[w].alive && st.wires[w].alive).collect();
            took.sort_unstable();
            took.dedup();
            for w in took {
                if !gone_v.contains(&w) {
                    if local_check {
                        eprintln!("LOOPLOCAL changed wire {w} (net {}): order; move gone {gone:?} new {new:?}", st.wires[w].net);
                    }
                    changed += 1;
                    gone_v.push(w);
                    new_v.push(w);
                }
            }
            LoopCtr::add(&lctr.local_changed, changed);
            let (gone, new) = (&gone_v[..], &new_v[..]);
            let mut free = vec![false; nw];
            let net_of = |w: WireId| st.wires[w].net;
            let mut ring: Vec<WireId> = Vec::new();
            let mark = |free: &mut Vec<bool>, ring: &mut Vec<WireId>, w: WireId| {
                if w < nw && st.wires[w].alive && !free[w] {
                    free[w] = true;
                    ring.push(w);
                }
            };
            for &n in new {
                if n < nw {
                    free[n] = true;
                }
            }
            for &o in gone {
                let no = net_of(o);
                for &(p, _) in prev.partner.get(o).into_iter().flatten().flatten() {
                    if p < nw && st.wires[p].net != no {
                        mark(&mut free, &mut ring, p);
                    }
                }
            }
            for &(a, c) in &prev.unlayerable_w {
                if gone.contains(&a) {
                    mark(&mut free, &mut ring, c);
                }
                if gone.contains(&c) {
                    mark(&mut free, &mut ring, a);
                }
            }
            let mut front: Vec<WireId> = new.iter().copied().filter(|&n| n < nw && st.wires[n].alive).collect();
            for d in 0..local_depth {
                let mut next = Vec::new();
                for &w in &front {
                    let nwn = net_of(w);
                    for x in st.crossings(w) {
                        if st.wires[x.other].net != nwn {
                            mark(&mut free, &mut next, x.other);
                        }
                    }
                }
                if d == 0 {
                    next.append(&mut ring);
                }
                front = next;
            }
            let js = tree::junctions(st);
            loop {
                let mut changed = false;
                for &(br, _, t, _, _) in &js {
                    if br < nw && t < nw && free[br] != free[t] {
                        free[br] = true;
                        free[t] = true;
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            let held = layers::Held { prev, free: &free };
            let lr = assign_l(st, ties, &held);
            // The model's wires W, after and before.
            let mut w_after = layers::neighbourhood(st, &free);
            // tw4 T2: with the via cuts, every wire their changed terms involve (both states).
            let or = |a: &mut Vec<bool>, b: Vec<bool>| a.iter_mut().zip(b).for_each(|(x, y)| *x |= y);
            let mut vfocus: Vec<bool> = Vec::new();
            if opts.via_cuts {
                let (m, f) = layers::via_neighbourhood_x(st, &free, &lr.events, opts.pad_exits);
                or(&mut w_after, m);
                vfocus = f;
            }
            let after = st.take_topology();
            st.restore(saved.clone());
            let nb = st.wires.len();
            let free_b: Vec<bool> = (0..nb).map(|w| free.get(w).copied().unwrap_or(false) || gone.contains(&w)).collect();
            let mut w_before = layers::neighbourhood(st, &free_b);
            if opts.via_cuts {
                let (m, f) = layers::via_neighbourhood_x(st, &free_b, &prev.events, opts.pad_exits);
                or(&mut w_before, m);
                // One focus for both states (the union), so a term is priced on both sides or on none.
                vfocus.resize(vfocus.len().max(f.len()), false);
                or(&mut vfocus, f);
            }
            let wm: Vec<bool> = (0..nw.max(nb)).map(|w| w_after.get(w).copied().unwrap_or(false) || w_before.get(w).copied().unwrap_or(false)).collect();
            let cap_of = |st: &State, evl: &[Vec<(usize, f64, i32)>], only: &[bool]| -> (usize, f64) {
                let only = &only[..st.wires.len().min(only.len())];
                let dirs = packer.taut_dirs_only(st, only);
                if opts.via_cuts {
                    let fo: Vec<bool> = (0..st.wires.len()).map(|w| vfocus.get(w).copied().unwrap_or(false)).collect();
                    CapModel::new_only_focus(st, &packer, &dirs, evl, bottom, overflow_mm, only, &fo).violations()
                } else {
                    CapModel::new_only(st, &packer, &dirs, evl, bottom, overflow_mm, only).violations()
                }
            };
            let cap_b = cap_of(st, &prev.events, &wm);
            // tw4 fix round 2 (verifier F1, test hook): the same change judged with every model wire in focus.
            let focus_check = opts.via_cuts && super::refine::viacheck::FOCUS_ON.load(std::sync::atomic::Ordering::Relaxed);
            let cap_all = |st: &State, evl: &[Vec<(usize, f64, i32)>]| -> (usize, f64) {
                let only = &wm[..st.wires.len().min(wm.len())];
                let dirs = packer.taut_dirs_only(st, only);
                let fo = vec![true; st.wires.len()];
                CapModel::new_only_focus(st, &packer, &dirs, evl, bottom, overflow_mm, only, &fo).violations()
            };
            let all_b = if focus_check { cap_all(st, &prev.events) } else { (0, 0.0) };
            let nd_b = if neck_k > 0.0 { st.neck_depth(Some(&wm)) } else { 0.0 };
            let fc_b = foreign_copper_only(st, &layers_now_only(st, &prev.events, bottom, Some(&free_b)), Some(&free_b));
            st.restore(after);
            let cap_a = cap_of(st, &lr.events, &wm);
            if focus_check {
                // the focus cut is exact for the change (after - before), not for each side's total
                let all_a = cap_all(st, &lr.events);
                let (dn, dg) = (cap_a.0 as i64 - cap_b.0 as i64, cap_a.1 - cap_b.1);
                let (an, ag) = (all_a.0 as i64 - all_b.0 as i64, all_a.1 - all_b.1);
                use std::sync::atomic::Ordering::Relaxed;
                super::refine::viacheck::FOCUS_CHECKED.fetch_add(1, Relaxed);
                if dn != an || (dg - ag).abs() > 1e-6 * (1.0 + ag.abs()) {
                    super::refine::viacheck::FOCUS_BAD.fetch_add(1, Relaxed);
                }
            }
            let nd_a = if neck_k > 0.0 { st.neck_depth(Some(&wm)) } else { 0.0 };
            let fc_a = foreign_copper_only(st, &layers_now_only(st, &lr.events, bottom, Some(&free)), Some(&free));
            let (nf, nm) = (free.iter().filter(|&&f| f).count(), wm.iter().filter(|&&f| f).count());
            LoopCtr::add(&lctr.local_free, nf);
            LoopCtr::add(&lctr.local_model, nm);
            lctr.spend(super::escape::cost::local(0, nf + nm));
            let pp = pred.parts;
            let du = |base: usize, a: usize, b: usize| (base + a).saturating_sub(b);
            let parts = super::optimise::MeasureParts {
                cap: (du(pp.cap.0, cap_a.0, cap_b.0), (pp.cap.1 + cap_a.1 - cap_b.1).max(0.0)),
                unlayerable: lr.unlayerable.len(),
                forced: forced.borrow().len(),
                foreign: (du(pp.foreign.0, fc_a.0, fc_b.0), (pp.foreign.1 + fc_a.1 - fc_b.1).max(0.0), du(pp.foreign.2, fc_a.2, fc_b.2)),
                neck: if neck_k > 0.0 { (pp.neck + neck_k * (nd_a - nd_b)).max(0.0) } else { 0.0 },
                // tw4-r2 (3): the pour term of the state after the move, whole (a ring is no local
                // thing: the cut-off component is decided by the whole board's copper).
                pour: if pour_base.is_some() {
                    super::pourplan::meter::LOCAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    pour_of(st, &lr.events)
                } else {
                    (0, 0.0)
                },
            };
            let p = Predicted::from_parts(parts, opts.pick_shorts, opts.short_weight);
            if local_check {
                let f = layers::partner_faults(st, &lr);
                LoopCtr::add(&lctr.local_faults, f.len());
                let (_, hh) = layers::unpriced_crossings(st, &lr, bottom, Some(&free));
                LoopCtr::add(&lctr.local_unpriced, hh);
                let stale = layers::stale_held_events(st, prev, &free);
                LoopCtr::add(&lctr.local_stale, stale);
                if stale > 0 {
                    eprintln!("LOOPLOCAL stale held events {stale}; gone {gone:?} new {new:?} changed {changed}");
                }
                if hh > 0 {
                    eprintln!("LOOPLOCAL unpriced held-held crossings {hh}; gone {gone:?} new {new:?} changed {changed}");
                }
                if !f.is_empty() {
                    eprintln!("LOOPLOCAL partner faults {} (first {:?}); gone {gone:?} new {new:?} changed {changed}", f.len(), &f[..f.len().min(5)]);
                }
            }
            (lr, p)
        };
        type PlaceFn<'f, 'p> = &'f dyn Fn(&mut State<'p>, usize, usize, &mut Vec<usize>, &Registry) -> bool;
        type CrossFn<'f, 'p> = &'f dyn Fn(&mut State<'p>, &mut Vec<usize>, &mut Vec<u32>, &mut layers::LayerReport, &mut usize, usize);
        type MeasureFn<'f, 'p> = &'f dyn Fn(&State<'p>, &layers::LayerReport) -> Predicted;
        type AssignFn<'f, 'p> = &'f dyn Fn(&State<'p>, u64) -> layers::LayerReport;
        type LocalFn<'f, 'p> = &'f dyn Fn(&mut State<'p>, &super::state::Topology, &[WireId], &[WireId], &layers::LayerReport, &Predicted, u64) -> (layers::LayerReport, Predicted);
        /// `[optimize] loop_shadow`: one re-lay judged both ways (local, whole board): the price
        /// of its violations and vias, the predicted violations, the vias and the unlayerable
        /// crossings, local then global.
        /// The whole board's judge also layers the state before the move (`base`: its
        /// predicted violations, vias, unlayerable crossings and price), so both sides compare a
        /// change: the whole-board layering of the board as it is differs from the present
        /// (locally evolved) layering on wires the move never touched, and that offset is no part
        /// of the move.
        struct Shadow {
            v: (f64, f64),
            pred: (Predicted, Predicted),
            vias: (usize, usize),
            unl: (usize, usize),
            base: (Predicted, usize, usize, f64),
        }
        /// The single-wire move's price and placement ([`Rerouter`]).
        struct Rr<'a, 'p> {
            topo: &'a Topo,
            /// The try's price registry: its coefficients with the `overflow` and `via` factors at
            /// 0 (the predicted violations and the vias are priced by the board's own measure,
            /// [`Rerouter::violations`]).
            price_reg: &'a Registry,
            wire_conn: &'a mut Vec<usize>,
            place: PlaceFn<'a, 'p>,
            reg: &'a Registry,
            rank: &'a [usize],
            cap: usize,
            forced: &'a std::cell::RefCell<BTreeSet<usize>>,
            conn_of: &'a [usize],
            saved: Option<BTreeSet<usize>>,
            viol_mm: f64,
            /// The predicted violations' price of the state before the try.
            viol_old: f64,
            /// The try's price of one via (the `via` factor's weight with the wire's coefficient
            /// and the cycle's emphasis) and the board's vias before the try (its layering).
            via_mm: f64,
            vias_old: f64,
            assign_t: AssignFn<'a, 'p>,
            ties: u64,
            measure: MeasureFn<'a, 'p>,
            /// The layering and predicted violations of the state after the try.
            pending: Option<(layers::LayerReport, Predicted)>,
            /// loop-local: the local judge (None = every try layers the whole board), the
            /// layering and predicted violations before the try, whether this try is also judged
            /// by the whole board (`[optimize] loop_shadow`) and that comparison.
            local: Option<LocalFn<'a, 'p>>,
            lr_old: &'a layers::LayerReport,
            pred_old: Predicted,
            shadow: bool,
            shadow_out: Option<Shadow>,
            /// TW6 r4 (`[optimize] site_move_detour`): the excess-ratio scale of the judge's graded
            /// detour price (0 = none, every reroute but E's), the connections it is read on, and
            /// its value before and after the try (violations x `viol_mm`, mm).
            det_scale: f64,
            conns: &'a [Conn],
            det_cis: BTreeSet<usize>,
            det: (f64, f64),
        }
        impl<'p> Rerouter<'p> for Rr<'_, 'p> {
            fn judge_extra(&mut self, st: &mut State<'p>, ws: &[WireId], after: bool) -> f64 {
                if self.det_scale <= 0.0 {
                    return 0.0;
                }
                if !after {
                    self.det_cis = ws.iter().filter_map(|&w| self.wire_conn.get(w).copied()).filter(|&c| c != usize::MAX).collect();
                }
                let d = self.viol_mm * conn_detour_price(st, self.wire_conn, self.conns, &self.det_cis, self.det_scale);
                if after {
                    self.det.1 = d;
                } else {
                    self.det.0 = d;
                }
                d
            }
            fn price(&mut self, st: &mut State<'p>, w: WireId) -> f64 {
                let _pg = super::prof::g(super::prof::K::RrPrice);
                let ci = self.wire_conn[w];
                let path = st.detach(w);
                let wire = st.wires[w].clone();
                let c = self.topo.path_price(st, &wire, &path, &wire.tris, ci, self.price_reg);
                st.reattach(w, &path);
                c
            }
            fn place(&mut self, st: &mut State<'p>, gone: &[WireId]) -> Option<Vec<WireId>> {
                let _pg = super::prof::g(super::prof::K::RrPlace);
                let mut cis: Vec<usize> = gone.iter().map(|&g| self.wire_conn[g]).collect();
                cis.sort_by_key(|&ci| self.rank[ci]);
                cis.dedup();
                self.saved = Some(self.forced.borrow().clone());
                // loop-local: the wires this placing's double-crossing swaps take part in.
                st.took_part.clear();
                for &ci in &cis {
                    self.forced.borrow_mut().remove(&self.conn_of[ci]);
                }
                let n0 = st.wires.len();
                for ci in cis {
                    if !(self.place)(st, ci, self.cap, self.wire_conn, self.reg) {
                        return None;
                    }
                }
                Some((n0..st.wires.len()).filter(|&w| st.wires[w].alive).collect())
            }
            fn violations(&mut self, st: &mut State<'p>, after: bool) -> f64 {
                if !after {
                    return self.viol_old + self.via_mm * self.vias_old;
                }
                // The new state is layered as the loop layers it (the current tie seed) and judged
                // by the archive's measure: its predicted violations and its vias (5.2c: the vias
                // the reroute adds or removes on the whole board, also on the wires it crosses,
                // not the wire's own estimate against the present layers).
                let _pg = super::prof::g(super::prof::K::RrViolAfter);
                let lr = (self.assign_t)(st, self.ties);
                let p = (self.measure)(st, &lr);
                let v = p.price(self.viol_mm) + self.via_mm * lr.vias_w;
                self.pending = Some((lr, p));
                v
            }
            fn violations_after(&mut self, st: &mut State<'p>, saved: &super::state::Topology, gone: &[WireId], new: &[WireId]) -> f64 {
                let Some(local) = self.local else {
                    return self.violations(st, true);
                };
                let (lr, p) = local(st, saved, gone, new, self.lr_old, &self.pred_old, self.ties);
                let v = p.price(self.viol_mm) + self.via_mm * lr.vias_w;
                if self.shadow {
                    let lg = (self.assign_t)(st, self.ties);
                    let pg = (self.measure)(st, &lg);
                    let vg = pg.price(self.viol_mm) + self.via_mm * lg.vias_w;
                    let after = st.take_topology();
                    st.restore(saved.clone());
                    let lb = (self.assign_t)(st, self.ties);
                    let pb = (self.measure)(st, &lb);
                    let vb = pb.price(self.viol_mm) + self.via_mm * lb.vias_w;
                    st.restore(after);
                    self.shadow_out = Some(Shadow { v: (v, vg), pred: (p, pg), vias: (lr.vias, lg.vias), unl: (lr.unlayerable.len(), lg.unlayerable.len()), base: (pb, lb.vias, lb.unlayerable.len(), vb) });
                }
                self.pending = Some((lr, p));
                v
            }
            fn undo(&mut self) {
                if let Some(f) = self.saved.take() {
                    *self.forced.borrow_mut() = f;
                }
                self.pending = None;
            }
        }
        /// 5.8: one obstructed via pair of a wire ([`Sp::obstructed_pairs`]): its stretch (events
        /// `span`), the layers before and after it (`lv`, `lu`), the layer `t` it would go onto
        /// and the wires crossing it there; the stretch's length (nm) and its graded price (mm,
        /// [`pair_len_price`]).
        struct PairCand {
            span: (usize, usize),
            t: i32,
            lv: i32,
            lu: i32,
            obs: Vec<WireId>,
            len: f64,
            price: f64,
            /// thick-layer S3 (M2.3, `[vias] pair_one_side`): the wire's whole run from its first
            /// to its last layer change held to `t` (one side), not one stretch; `gain` = ω(w) x
            /// the layer changes the hold removes (the candidates' order).
            one_side: bool,
            gain: f64,
        }
        /// TW7 B/C/E: one found pattern as one move ([`Sp::pattern_cands`]): its kind, the wires
        /// laid first in this order (`first`), the wires laid after them in route order (`obs`),
        /// the layer the first wire is held to while the others are laid (C: the via trade), and
        /// its key in the fruitless-try counts (`[optimize] pattern_stuck`).
        struct PatCand {
            kind: super::xrules::Kind,
            first: Vec<WireId>,
            obs: Vec<WireId>,
            hold: Option<i32>,
            key: (usize, usize),
            /// E: the edges priced for this move only (the search's history, [`State::history`]),
            /// and the price per edge (history units).
            bump: Vec<usize>,
            bump_h: f64,
        }
        /// TW5 ESCAPE (docs/dev/design/tw5-escape.md): the loop's escape settings and its
        /// loop-wide memory (not part of a variant: a restore keeps it).
        /// TW7 (docs/dev/design/tw7-moves.md): part B's pattern kinds that become targeted moves
        /// (empty = the batch crossing pass), whether net trees are free (their wires are not
        /// moved by the patterns), and part D's detour coefficient with its per-connection amounts
        /// (refreshed at each cycle's start), and `[optimize] pattern_share`'s work budget with the
        /// work counters' mark after the last pattern stage (None = before the first one).
        struct Tw7 {
            kinds: Vec<super::xrules::Kind>,
            tree_free: bool,
            detour_k: f64,
            detour_amt: Vec<f64>,
            share: super::escape::Share,
            mark: Option<(usize, usize, usize)>,
            /// `[optimize] pattern_stuck`: the weight of a pattern's fruitless tries in the moves'
            /// order, and per pattern (its two connections, the lower first) the tries since its
            /// last kept move (loop-wide: a restore keeps it).
            stuck_w: f64,
            stuck: BTreeMap<(usize, usize), u32>,
        }
        struct Tw5<'a, 'p> {
            /// (a) `[optimize] oscillation = "bounded"` and its amplitude bounds.
            bounded: bool,
            amp_lo: f64,
            amp_hi: f64,
            /// (b) `[optimize] escape = "layering"`, its layering (with the plateau walk) and the
            /// work throttle; `escape_take = "always"`.
            escape: bool,
            assign_e: AssignFn<'a, 'p>,
            throttle: super::escape::Throttle,
            /// (f) the in-loop P4 rounds' own budget: with the layering escapes on too, each of the
            /// two gets half of `escape_share` (a shared budget starved the rounds: the escapes spent
            /// it in the sweep and `due()` was false after it).
            p4_throttle: super::escape::Throttle,
            /// site-price-cost `[optimize] escape_budget = "cost"`: both throttles count the loop's
            /// cost counter ([`LoopCtr::cost`]) and an escape its measured cost; false = the wire
            /// units with `escape_kappa` per alive wire (before).
            cost_budget: bool,
            /// site-price-cost: the sweep re-lays' work in wire units ([`Sp::work_since`]) whatever
            /// the budget, reported beside the throttle's own currency ("relay_wires").
            relay_wires: f64,
            take_always: bool,
            /// The re-lay counter's fading price per connection (`relay_count_fade` < 1; None =
            /// the plain counter, as before TW5).
            relay_w: Option<(Vec<f64>, f64)>,
            /// (c) the loop history and the history the loop started with.
            hist: Option<super::escape::History>,
            hist_base: Vec<f64>,
            /// (d) floating up per wire.
            float_wire: bool,
            /// (f) one P4 rip-up round per cycle inside the loop (`[optimize] loop_p4`), and the
            /// tests' forced failure of its second placing (`loop_p4_test_fail`).
            loop_p4: bool,
            p4_test_fail: bool,
            /// Escapes so far in this run (their seeds).
            n_esc: u64,
            /// TW5 `[optimize] escape_offer`: the state after an escape / a taken in-loop P4
            /// round is offered to the archive and the table ([`Space::take_offers`]); the
            /// criteria the archive held at the cycle's start plus those offered since (a state
            /// they cover is not copied); the offers of the running cycle.
            offer_on: bool,
            floor: Vec<Criteria>,
            offers: Vec<(String, Criteria, LoopVar, u64)>,
        }
        /// The real topology of this route as the loop's [`Space`].
        struct Sp<'a, 'p> {
            st: &'a mut State<'p>,
            wire_conn: &'a mut Vec<usize>,
            lr: &'a mut layers::LayerReport,
            reroutes: &'a mut Vec<u32>,
            stuck: Vec<u32>,
            tries: Vec<u32>,
            conns: &'a [Conn],
            conn_of: &'a [usize],
            tiers: &'a [u8],
            rank: &'a [usize],
            reg: &'a Registry,
            lp: LayerParams,
            n_sig: usize,
            bottom: i32,
            cap: usize,
            ws0: f64,
            overflow_mm: f64,
            viol_mm: f64,
            cuts_on: bool,
            priced: bool,
            obj: Objective,
            via_area_mm: f64,
            crossing_share: f64,
            max_ripped: usize,
            max_moves: usize,
            cut_vias: bool,
            packer: &'a Packer<'p>,
            forced: &'a std::cell::RefCell<BTreeSet<usize>>,
            place: PlaceFn<'a, 'p>,
            assign_t: AssignFn<'a, 'p>,
            measure: MeasureFn<'a, 'p>,
            cross: CrossFn<'a, 'p>,
            xkept: &'a dyn Fn() -> usize,
            /// The tie seed the current layering came from (0 = the first optimum).
            ties: u64,
            /// The predicted violations of the current state and layering.
            pred: Predicted,
            /// 5.4 `[vias] pair_min`: the longest stretch (nm) between a via pair the loop tries
            /// to remove; `None` = off.
            pair_max: Option<f64>,
            /// 5.8 `[vias] pair_move`: the length scale (nm, `[vias] pair_max_mm`) of the graded
            /// stretch price ([`super::viaopt::pair_len_price`]); `None` = off.
            pair_move: Option<f64>,
            /// thick-layer S3 (`[vias] pair_one_side`): the one-side candidates of the pair move.
            one_side: bool,
            /// thick-layer S3 (`[vias] pair_hold`): the release price of a one-side move's held
            /// stretch per event off its layer, in via prices (x ω of the wire).
            pair_hold: f64,
            /// thick-layer S3: the held wire's layer changes in the layering right after the last
            /// one-side move (0 = the hold kept its whole run on the side), for the cycle stats.
            one_side_left: usize,
            /// thick-layer M1: ω of a wire's own vias (the one-side candidates' gain).
            omega: super::layers::Omega,
            /// TW6 part E: `[optimize] site_move` / `branch_move`.
            site_move: bool,
            branch_move: bool,
            /// TW6 r4 `[optimize] site_move_detour`: the excess-ratio scale of the graded detour
            /// price in E's keep judgements (0 = none), and the moves it refused so far.
            move_detour: f64,
            site_refused: usize,
            /// The objective's price of a via (mm, `mm_per_via`), the unit of that stretch price.
            via_mm: f64,
            /// loop-local Step A: the per-wire stretch cache of the layerings (exact).
            scache: Option<&'a layers::StretchCache>,
            /// loop-local Step B: the local judge of the moves (None = `loop_judge = "global"`),
            /// whether `pred` came from it (then the next whole-board comparison measures the board
            /// first), the shadow interval (0 = off), the re-lays so far, and the work counters.
            local: Option<LocalFn<'a, 'p>>,
            pred_local: bool,
            shadow_k: usize,
            relays: usize,
            lctr: &'a LoopCtr,
            shadow: (usize, usize, f64, usize, usize, f64),
            /// Debug (`TOPO_LOOPDBG`): one line per reroute and per cycle stage.
            dbg: bool,
            dbg_tag: u64,
            /// TW3: this run's pre-loop decisions (every snapshot carries them).
            pre: std::sync::Arc<PreLoop>,
            /// TW7 E: the pour pieces the plan's copper cuts off, with the wires bounding them
            /// ([`super::pourplan::cut_components`] on the layering's plan; None = `[capacity]
            /// pour_cuts` off, so the "pour" pattern kind finds nothing).
            pour_comps: Option<&'a dyn Fn(&State, &[Vec<(usize, f64, i32)>]) -> Vec<super::pourplan::CutComp>>,
            /// The run's control: the test hook's markers between the cycle's stages.
            ctl: &'a RunControl,
            /// TW5 ESCAPE.
            tw5: Tw5<'a, 'p>,
            /// TW7 B + D.
            tw7: Tw7,
            /// last-preview: the board (layers of the plan sketch) and the sketch bookkeeping.
            board: &'a board::Board,
            sk: SketchBook,
            /// plan-dots: the places of the plan's predicted violations (its sketch's dots).
            places: &'a dyn Fn(&State<'p>, &layers::LayerReport) -> super::compass::PlanPlaces,
        }
        impl<'p> Sp<'_, 'p> {
            /// last-preview: the plan sketch of the current state (its layer stretches and planned
            /// vias, [`super::pourplan::plan_items`], KiCad frame), labelled with `crit`, with the
            /// places of its predicted violations (plan-dots, [`super::compass::plan_places`]: one
            /// per counted violation but the forced connections). Reads the state only (the via
            /// site cache and the edge memo it may fill are pure memos). O(passes + a capacity
            /// model build).
            fn plan_sketch(&self, cycle: usize, crit: &Criteria) -> crate::control::Sketch {
                let t0 = std::time::Instant::now();
                let items = super::pourplan::plan_items(self.st, self.packer, self.board, &self.lr.events, self.bottom);
                let at = (self.places)(self.st, self.lr).at;
                let sk = sketch_of(self.ctl.run.max(1), cycle, "plan", crit, items, at);
                let dt = t0.elapsed().as_secs_f64();
                self.ctl.with_progress(|p| p.sketch_build_s += dt);
                sk
            }
            /// Debug: the criteria now as one short text.
            fn dbg_crit(&mut self) -> String {
                let c = self.criteria();
                format!("viol {} (graded {:.3}) vias {} len {:.3} shr {} eval {:.3}", c.errors, self.pred.graded, c.vias, c.length_mm, c.shrinkages, evaluation(&self.obj, &c, self.viol_mm))
            }
            fn via_counts(&self) -> BTreeMap<usize, usize> {
                let _pg = super::prof::g(super::prof::K::ViaCounts);
                let mut m = BTreeMap::new();
                for w in (0..self.st.wires.len()).filter(|&w| self.st.wires[w].alive && w < self.lr.events.len()) {
                    if let Some(&ci) = self.wire_conn.get(w).filter(|&&c| c != usize::MAX) {
                        *m.entry(ci).or_insert(0) += plan(&self.lr.events[w], self.n_sig).vias.len();
                    }
                }
                m
            }
            /// Make `lr` the current layering: the stretches' present layers for the layer-aware
            /// reroute ([`State::layers_now`]) and its predicted violations (`p`, or measured).
            fn set_lr(&mut self, lr: layers::LayerReport, p: Option<Predicted>) {
                *self.lr = lr;
                // The search's view of the layering, all of it at once (TW6 r4: the via intervals
                // of the B search term cannot be left behind, [`State::apply_layering`]).
                self.st.apply_layering(&self.lr.events, self.bottom);
                if p.is_none() {
                    self.pred_local = false;
                }
                self.pred = p.unwrap_or_else(|| (self.measure)(self.st, self.lr));
            }
            /// loop-local: after locally judged moves `pred` is the sum of their changes; a
            /// whole-board comparison (layer re-optimisation, crossing pass) and every offered
            /// variant take the board's measure of the present layering instead (no layering).
            fn resync(&mut self) {
                if self.pred_local {
                    self.pred = (self.measure)(self.st, self.lr);
                    self.pred_local = false;
                }
            }
            /// The evaluation of the current topology every whole-board move is judged by
            /// ([`evaluation`]: the archive's objective value plus the violation weight per
            /// predicted violation).
            fn eval_now(&mut self) -> f64 {
                let c = self.criteria();
                evaluation(&self.obj, &c, self.viol_mm)
            }
            /// A whole-board layering move (layer re-optimisation, floating up): `lr` becomes the
            /// layering when the evaluation does not rise (Polubasov 2010: the layer
            /// re-optimisation "never worsens the quality of the routing"; among equal ones the new
            /// one is taken, so the random choice among equal layerings stays), else the one before
            /// comes back. Whether it was taken.
            fn try_layers(&mut self, lr: layers::LayerReport, ties: u64) -> bool {
                self.resync();
                let _pg = super::prof::g(super::prof::K::TryLayers);
                let e0 = self.eval_now();
                let (lr0, p0) = (self.lr.clone(), self.pred);
                self.set_lr(lr, None);
                if self.eval_now() <= e0 + 1e-6 {
                    self.ties = ties;
                    true
                } else {
                    self.set_lr(lr0, Some(p0));
                    false
                }
            }
            /// The search registry of a try on wire `w`: [`Sp::conn_reg`] of its connection.
            fn try_reg(&self, w: WireId, seed: u64, amp: f64, via_w: f64) -> (Registry, f64) {
                self.conn_reg_w(self.wire_conn[w], Some(w), seed, amp, via_w)
            }
            /// TW5 (a): the badness of wire `w` (connection `ci`): its detour, the loop history on
            /// its passes, its fruitless tries ([`super::escape::badness`]).
            fn badness(&self, ci: usize, w: WireId) -> f64 {
                let wi = &self.st.wires[w];
                let straight = (self.st.proj.pos[wi.b] - self.st.proj.pos[wi.a]).len();
                let crowd = self.tw5.hist.as_ref().map_or(0.0, |h| h.mean_on(wi.passes.iter().map(|&p| self.st.passes[p].edge)));
                super::escape::badness(self.st.wire_len(w), straight, crowd, self.stuck[ci])
            }
            /// The amplitude of a try on connection `ci` (wire `w` when known): TW5 bounded and
            /// graded by its badness, or the cycle's width widened by its fruitless tries.
            fn amp_of(&self, ci: usize, w: Option<WireId>, amp: f64) -> f64 {
                if !self.tw5.bounded {
                    return wire_amp(amp, self.stuck[ci]);
                }
                let w = w.or_else(|| (0..self.st.wires.len()).find(|&x| self.st.wires[x].alive && self.wire_conn.get(x) == Some(&ci)));
                let b = w.map_or_else(|| { let s = f64::from(self.stuck[ci]); s / (1.0 + s) }, |w| self.badness(ci, w));
                super::escape::bounded_amp(self.tw5.amp_lo, self.tw5.amp_hi, b)
            }
            /// TW5 (a): the violation weight of a try (mm per graded violation): oscillating with the
            /// wire's coefficients when bounded (Polubasov 2010: all the k_i oscillate), else the
            /// constant weight. The table / archive keeps the constant one.
            fn viol_k(&self, ci: usize, seed: u64, amp_i: f64) -> f64 {
                if !self.tw5.bounded {
                    return self.viol_mm;
                }
                self.viol_mm * k_osc(seed, self.conn_of[ci], VIOL_FACTOR, self.tries[ci], amp_i)
            }
            /// TW5 (b): work units of the local steps since `k0` (the counters `local_free`,
            /// `local_model`, `layer_global`), a global layering at `kappa` per alive wire.
            fn work_since(&self, k0: (usize, usize, usize)) -> f64 {
                let alive = self.st.wires.iter().filter(|w| w.alive).count();
                let l = self.lctr;
                (l.local_free.get() - k0.0 + l.local_model.get() - k0.1) as f64 + self.tw5.throttle.kappa * ((l.layer_global.get() - k0.2) * alive) as f64
            }
            fn work_mark(&self) -> (usize, usize, usize) {
                (self.lctr.local_free.get(), self.lctr.local_model.get(), self.lctr.layer_global.get())
            }
            /// site-price-cost: the throttles' work since the marks `k0` (wire units) and `c0`
            /// (the cost counter): the cost counter's change with `escape_budget = "cost"`, else
            /// [`Sp::work_since`]. O(alive).
            fn budget_since(&self, k0: (usize, usize, usize), c0: f64) -> f64 {
                if self.tw5.cost_budget {
                    (self.lctr.cost.get() - c0).max(0.0)
                } else {
                    self.work_since(k0)
                }
            }
            /// site-price-cost: one row of the profile's loop work record ([`super::prof::work`]):
            /// `kind`, its wall seconds since `t0` and the counts since the marks `m0` (evaluations,
            /// local free + model wires, whole-board measures) and `c0` (cost). Only with profiling on.
            fn work_row(&self, kind: &str, t0: Option<Instant>, m0: (usize, usize, usize), c0: f64) {
                let Some(t0) = t0 else { return };
                let l = self.lctr;
                super::prof::work(serde_json::json!({
                    "kind": kind,
                    "wall_s": t0.elapsed().as_secs_f64(),
                    "evals": l.evals.get() - m0.0,
                    "local_wires": l.local_free.get() + l.local_model.get() - m0.1,
                    "measures": l.measure_global.get() - m0.2,
                    "alive": self.st.wires.iter().filter(|w| w.alive).count(),
                    "passes": self.st.passes.len(),
                    "cost": l.cost.get() - c0,
                }));
            }
            /// The marks of [`Sp::work_row`]. O(1).
            fn row_mark(&self) -> (Option<Instant>, (usize, usize, usize), f64) {
                let l = self.lctr;
                (super::prof::on().then(Instant::now), (l.evals.get(), l.local_free.get() + l.local_model.get(), l.measure_global.get()), l.cost.get())
            }
            /// The search registry of a try on connection `ci`: its oscillating coefficients (the
            /// amplitude widened by its fruitless tries) with the via weight `via_w`, and that
            /// amplitude.
            fn conn_reg(&self, ci: usize, seed: u64, amp: f64, via_w: f64) -> (Registry, f64) {
                self.conn_reg_w(ci, None, seed, amp, via_w)
            }
            fn conn_reg_w(&self, ci: usize, w: Option<WireId>, seed: u64, amp: f64, via_w: f64) -> (Registry, f64) {
                let id = self.conn_of[ci];
                let (amp_i, visit) = (self.amp_of(ci, w, amp), self.tries[ci]);
                // The search: a pass over the room of its passage (on all layers together) is a
                // likely violation, so the search steers away from it at the `violation` weight
                // ("the errors have the highest weight; once gone they are very unlikely to come
                // back", Luzin & Polubasov 2006 p. 75). A crossing is layer-aware (5.2b): the via
                // it implies against the crossed stretch's present layer is priced as a via
                // ([`State::held_layers`], `cross_vias`), so only `crossing_share` of the
                // `crossing` factor stays for the crossing itself.
                let over_k = (self.viol_mm / self.overflow_mm).max(1.0);
                let share = self.crossing_share;
                let reg_w = self.reg.scaled(|i, name| {
                    let e = if name == "crossing" {
                        via_w * share
                    } else if VIA_FACTORS.contains(&name) {
                        via_w
                    } else if name == "overflow" {
                        over_k
                    } else if name == "length" && self.tw7.detour_k > 0.0 {
                        // TW7 D: the connection's last detour raises its length coefficient.
                        1.0 + self.tw7.detour_k * self.tw7.detour_amt.get(ci).copied().unwrap_or(0.0)
                    } else {
                        1.0
                    };
                    k_osc(seed, id, i, visit, amp_i) * e
                });
                (reg_w, amp_i)
            }
            /// One single-wire rip-up and reroute of `w` (and the branches on its junctions) with the
            /// wire's oscillating coefficients and the via weight `via_w`; whether it was kept.
            fn reroute(&mut self, topo: &mut Topo, w: WireId, seed: u64, amp: f64, via_w: f64) -> bool {
                self.reroute_d(topo, w, seed, amp, via_w, 0.0)
            }
            /// [`Sp::reroute`] with TW6 r4's graded detour price on the excess-ratio scale `det`
            /// in its keep judgement (`[optimize] site_move_detour`, E's own wire; 0 = none: the
            /// plain reroute, bit-equal). A move the judgement without that price would have kept
            /// counts in `site_refused`.
            fn reroute_d(&mut self, topo: &mut Topo, w: WireId, seed: u64, amp: f64, via_w: f64, det: f64) -> bool {
                let _pg = super::prof::g(super::prof::K::Reroute);
                let ci = self.wire_conn[w];
                let (reg_w, amp_i) = self.try_reg(w, seed, amp, via_w);
                // The price the move is kept by (5.2a): the same coefficients without the search's
                // own over-room count; the board's predicted violations are priced by the
                // archive's measure instead ([`Rerouter::violations`]). 5.2c: likewise the vias:
                // the search estimates a path's vias against the stretches' present layers (the
                // `via` factor, layer-aware crossings), but whether the board gains or loses a via
                // is decided by the layering of the whole state after the move (Polubasov 2010
                // «Оптимизация расслоения»: the layering is global; figs 4-5: a wire's detour pays
                // off when another wire then passes without a via). So the kept price counts the
                // board's vias of the layering after the move (the archive's count) at the wire's
                // via coefficient, and not the wire's own estimate.
                let reg_price = reg_w.scaled(|_, name| if name == "overflow" || name == "via" { 0.0 } else { 1.0 });
                let via_mm = reg_w.weight("via").unwrap_or(0.0);
                let gone: BTreeSet<WireId> = std::iter::once(w).collect();
                let mut set: Vec<WireId> = std::iter::once(w).chain(tree::dependents(self.st, &gone)).collect();
                set.sort_by_key(|&x| (self.rank[self.wire_conn[x]], x));
                let extra: f64 = set.iter().map(|&x| {
                    let c = self.wire_conn[x];
                    let ctx = WireCtx { net: self.conns[c].net, tier: self.tiers[c] };
                    match self.tw5.relay_w.as_ref() {
                        // TW5: the counter's price is linear in the count, here a fading count.
                        Some((rw, _)) => reg_w.price(&Event::Rerouted { wire: x, count: 1 }, &ctx) * rw.get(c).copied().unwrap_or(0.0),
                        None => reg_w.price(&Event::Rerouted { wire: x, count: self.reroutes[c] }, &ctx),
                    }
                }).sum();
                let viol_mm = self.viol_k(ci, seed, amp_i);
                topo.reg = reg_w;
                let cis: Vec<usize> = set.iter().map(|&x| self.wire_conn[x]).collect();
                let reg_place = topo.reg.clone();
                let by_old = if self.dbg {
                    let mut m: BTreeMap<&'static str, f64> = BTreeMap::new();
                    for &x in &set {
                        for (k, v) in topo.wire_price_by(self.st, x, self.wire_conn[x], &reg_price) {
                            *m.entry(k).or_insert(0.0) += v;
                        }
                    }
                    m
                } else {
                    BTreeMap::new()
                };
                let vias_before = self.lr.vias;
                let vc_before = if self.dbg { self.via_counts() } else { BTreeMap::new() };
                let len_before: f64 = set.iter().map(|&x| self.st.wire_len(x) / MM).sum();
                let mut rr = Rr {
                    topo: &*topo,
                    price_reg: &reg_price,
                    wire_conn: self.wire_conn,
                    place: self.place,
                    reg: &reg_place,
                    rank: self.rank,
                    cap: self.cap,
                    forced: self.forced,
                    conn_of: self.conn_of,
                    saved: None,
                    viol_mm,
                    viol_old: self.pred.price(viol_mm),
                    via_mm,
                    vias_old: self.lr.vias_w,
                    assign_t: self.assign_t,
                    ties: self.ties,
                    measure: self.measure,
                    pending: None,
                    local: self.local,
                    lr_old: &*self.lr,
                    pred_old: self.pred,
                    shadow: self.local.is_some() && self.shadow_k > 0 && self.relays.is_multiple_of(self.shadow_k),
                    shadow_out: None,
                    det_scale: det,
                    conns: self.conns,
                    det_cis: BTreeSet::new(),
                    det: (0.0, 0.0),
                };
                self.relays += 1;
                let lf0 = self.lctr.local_free.get();
                let out = reroute_set(self.st, &set, &mut rr, extra);
                let pending = rr.pending.take();
                let shadow = rr.shadow_out.take();
                let (det_old, det_new) = rr.det;
                drop(rr);
                if det > 0.0 && reroute_detour_refused(out.kept, out.old, out.new, det_old, det_new, extra) {
                    self.site_refused += 1;
                }
                if let Some(sh) = shadow {
                    // The keep test (`reroute_set`) with the whole board's change of the move
                    // (after - before, both layered whole) on the same base as the local one.
                    let own = out.new - out.viol_new;
                    let keep_g = own + out.viol_old + (sh.v.1 - sh.base.3) + extra < out.old - 1e-6;
                    let agree = keep_g == out.kept;
                    let (pl, pg) = sh.pred;
                    let (pb, vb, ub, _) = sh.base;
                    let (g0, v0, u0) = (self.pred.graded, self.lr.vias as i64, self.lr.unlayerable.len() as i64);
                    let (dgl, dgg) = (pl.graded - g0, pg.graded - pb.graded);
                    let (dvl, dvg) = (sh.vias.0 as i64 - v0, sh.vias.1 as i64 - vb as i64);
                    let (dul, dug) = (sh.unl.0 as i64 - u0, sh.unl.1 as i64 - ub as i64);
                    self.shadow.0 += 1;
                    self.shadow.1 += usize::from(agree);
                    self.shadow.2 += (dgl - dgg).abs();
                    self.shadow.3 += dvl.abs_diff(dvg) as usize;
                    self.shadow.4 += dul.abs_diff(dug) as usize;
                    self.shadow.5 += (pb.graded - g0).abs();
                    eprintln!(
                        "LOOPSHADOW wire {w} conn {ci} free {} kept local {} global {}: d graded local {dgl:.4} global {dgg:.4}; d pred local {} global {}; d vias local {dvl} global {dvg}; d unlayerable local {dul} global {dug}; whole-board layering of the board before vs present: graded {:+.4} vias {:+}",
                        self.lctr.local_free.get() - lf0,
                        out.kept,
                        keep_g,
                        pl.count as i64 - self.pred.count as i64,
                        pg.count as i64 - pb.count as i64,
                        pb.graded - g0,
                        vb as i64 - v0
                    );
                }
                if self.dbg && out.kept {
                    let neww: Vec<WireId> = (0..self.st.wires.len()).filter(|&x| self.st.wires[x].alive && self.wire_conn.get(x).is_some_and(|c| cis.contains(c))).collect();
                    let mut m: BTreeMap<&'static str, f64> = BTreeMap::new();
                    for &x in &neww {
                        for (k, v) in topo.wire_price_by(self.st, x, self.wire_conn[x], &reg_price) {
                            *m.entry(k).or_insert(0.0) += v;
                        }
                    }
                    let len_after: f64 = neww.iter().map(|&x| self.st.wire_len(x) / MM).sum();
                    let keys: BTreeSet<&str> = by_old.keys().chain(m.keys()).copied().collect();
                    let diff: Vec<String> = keys.iter().map(|k| format!("{k} {:.2}->{:.2}", by_old.get(k).copied().unwrap_or(0.0), m.get(k).copied().unwrap_or(0.0))).collect();
                    let vc_after = {
                        let mut m = BTreeMap::new();
                        if let Some((lr2, _)) = pending.as_ref() {
                            for x in (0..self.st.wires.len()).filter(|&x| self.st.wires[x].alive && x < lr2.events.len()) {
                                if let Some(&c) = self.wire_conn.get(x).filter(|&&c| c != usize::MAX) {
                                    *m.entry(c).or_insert(0usize) += plan(&lr2.events[x], self.n_sig).vias.len();
                                }
                            }
                        }
                        m
                    };
                    let ks: BTreeSet<usize> = vc_before.keys().chain(vc_after.keys()).copied().collect();
                    let dv: Vec<String> = ks.into_iter().filter(|c| vc_before.get(c) != vc_after.get(c)).map(|c| format!("c{c}:{}->{}", vc_before.get(&c).copied().unwrap_or(0), vc_after.get(&c).copied().unwrap_or(0))).collect();
                    eprintln!("LOOPDBG[{}] kept wire {w} conn {ci} (net {}): len {len_before:.3}->{len_after:.3} board vias {vias_before}->{} (per conn {}) factors {}", self.dbg_tag, self.conns[ci].net, pending.as_ref().map_or(0, |p| p.0.vias), dv.join(" "), diff.join(", "));
                }
                if self.dbg {
                    let pn = pending.as_ref().map(|p| format!("pred {} graded {:.3} vias {}", p.1.count, p.1.graded, p.0.vias)).unwrap_or_default();
                    let dt = if det > 0.0 { format!(" detour price {det_old:.3} -> {det_new:.3}") } else { String::new() };
                    eprintln!("LOOPDBG[{}] reroute wire {w} conn {ci} set {set:?} via_w {via_w:.3} amp {amp_i:.3} old {:.3} (viol {:.3}) new {:.3} (viol {:.3}) extra {:.3} kept {}{dt} {pn}", self.dbg_tag, out.old, out.viol_old, out.new, out.viol_new, extra, out.kept);
                }
                self.tries[ci] += 1;
                if out.kept {
                    let (lr, p) = pending.expect("a kept reroute was measured");
                    let _pg2 = super::prof::g(super::prof::K::RrSetLr);
                    self.set_lr(lr, Some(p));
                    self.pred_local = self.local.is_some();
                    for c in cis {
                        self.reroutes[c] += 1;
                        if let Some((rw, _)) = self.tw5.relay_w.as_mut() {
                            if let Some(x) = rw.get_mut(c) {
                                *x += 1.0;
                            }
                        }
                    }
                    self.stuck[ci] = 0;
                } else {
                    self.stuck[ci] += 1;
                }
                out.kept
            }
        }
        impl<'p> Sp<'_, 'p> {
            /// A whole-board layering move (floating up, layer re-optimisation): TW5
            /// `escape_take = "always"` takes it as it comes (Appendix F 2-3; the table keeps the
            /// bests), else [`Sp::try_layers`] (undone when the evaluation rises). Whether taken.
            fn take_layers(&mut self, lr: layers::LayerReport, ties: u64) -> bool {
                if !self.tw5.take_always {
                    return self.try_layers(lr, ties);
                }
                self.set_lr(lr, None);
                self.ties = ties;
                true
            }
            /// TW5 (c): one update of the loop history from the present layering's over-full edges
            /// (per-edge model: the over-full edge-layers with their graded depth; cut model: the
            /// edges where an over-full cut's wires cross it, depth = its overflow in default
            /// pitches), written onto the search's per-edge history.
            fn history_step(&mut self, cs: &mut CycleStats) {
                if self.tw5.hist.is_none() {
                    return;
                }
                let dirs = self.packer.taut_dirs(self.st);
                let over: Vec<(usize, f64)> = if self.cuts_on {
                    let cm = CapModel::new_cut(self.st, self.packer, &dirs, &self.lr.events, self.bottom, self.overflow_mm);
                    let mut v = Vec::new();
                    if let Some(c) = cm.cut.as_ref() {
                        for (l, ci, slack) in c.overfull_list() {
                            let d = (-slack / self.ws0.max(1.0)).max(0.0);
                            v.extend(item_edges(self.st, &c.items_at(l, ci)).into_iter().map(|e| (e, d)));
                        }
                    }
                    v
                } else {
                    CapModel::new(self.st, self.packer, &dirs, &self.lr.events, self.bottom, self.overflow_mm).overfull_depths()
                };
                let Some(h) = self.tw5.hist.as_mut() else { return };
                h.update(&over);
                h.apply(&self.tw5.hist_base, &mut self.st.history);
                cs.hist_updates += 1;
            }
            /// TW5 (f) `[optimize] loop_p4`: one negotiated P4 rip-up round inside the loop
            /// (docs/dev/design/tw5-escape.md (f)). The single-wire sweep keeps a move only when its
            /// price drops, so a full cut is a local minimum for each of its wires (Polubasov 2010
            /// «Оптимизация расслоения», fig. 7 point 1); ripped together, the first wire placed again
            /// may take a detour or two vias for the others (fig. 6a-c). The loop history (c) is
            /// updated first (the round's searches price it, McMurchie & Ebeling 1995 eq. 1); the
            /// victims are the pre-loop round's ([`p4_victims`]), the heaviest kept under the safety
            /// cap `(alive / 4).max(10)` (weight = the graded over-fullness of the items that chose
            /// them, then routed last first); placed again in rank order at the cycle's coefficients
            /// (seed of the cycle: deterministic, the same for any thread count); then uncross and one
            /// whole-board layering. Taken as it comes (the archive keeps the bests, Appendix F 2);
            /// when a wire finds no path the state before comes back (costly, not failed: no wire is
            /// dropped). Runs only when its own work budget allows ([`super::escape::Throttle`]: with the
            /// layering escapes on too, half of `escape_share` each, else all of it). Whether a round ran.
            fn loop_p4(&mut self, c: &CycleCtx, cs: &mut CycleStats, stop: &mut dyn FnMut() -> bool) -> bool {
                if !self.tw5.loop_p4 {
                    return false;
                }
                if !self.tw5.p4_throttle.due() {
                    cs.p4_throttled += 1;
                    return false;
                }
                let _pg = super::prof::g(super::prof::K::LoopP4);
                let (rt, _, _) = self.row_mark();
                self.resync();
                let k0 = self.work_mark();
                let c0 = self.lctr.cost.get();
                self.history_step(cs);
                let wt = {
                    let dirs = self.packer.taut_dirs(self.st);
                    let cm = if self.cuts_on { CapModel::new_cut(self.st, self.packer, &dirs, &self.lr.events, self.bottom, 1.0) } else { CapModel::new(self.st, self.packer, &dirs, &self.lr.events, self.bottom, 1.0) };
                    p4_victims(self.st, &cm, self.rank, self.wire_conn, &BTreeSet::new(), self.ws0, None)
                };
                let Some(wt) = wt else { return false };
                let alive = self.st.wires.iter().filter(|w| w.alive).count();
                let (wc, rk) = (&*self.wire_conn, self.rank);
                let rank_of = |w: WireId| wc.get(w).filter(|&&ci| ci != usize::MAX).map_or(0, |&ci| rk[ci]);
                let vic: Vec<(f64, usize, WireId)> = wt.into_iter().filter(|&(w, _)| self.st.wires[w].alive && wc.get(w).is_some_and(|&ci| ci != usize::MAX)).map(|(w, x)| (x, rank_of(w), w)).collect();
                // The safety cap only (the pre-loop round's bound): the heaviest victims first.
                let mut set: BTreeSet<WireId> = p4_keep_heaviest(vic, (alive / 4).max(10).min(self.max_ripped)).into_iter().collect();
                let deps = tree::dependents(self.st, &set);
                set.extend(deps);
                let mut vw: Vec<WireId> = set.into_iter().collect();
                vw.sort_by_key(|&w| (rank_of(w), w));
                if vw.is_empty() {
                    return false;
                }
                let mut cis: Vec<usize> = vw.iter().map(|&w| self.wire_conn[w]).collect();
                cis.dedup();
                let before = self.snapshot();
                let e0 = self.eval_now();
                for &w in &vw {
                    tree::remove_wire(self.st, w);
                }
                let seed = super::optimise::mix_seed(c.seed, 0x4C50_3400_0000 | c.cycle as u64);
                let via_w = c.emphasis.via_at(1.0);
                let mut lost = false;
                for (k, &ci) in cis.iter().enumerate() {
                    // A stop (Cancel, time limit) during the up to (alive/4).max(10) placings undoes
                    // the round like a lost wire does: the state before comes back.
                    if stop() {
                        lost = true;
                        break;
                    }
                    self.forced.borrow_mut().remove(&self.conn_of[ci]);
                    let (reg_w, _) = self.conn_reg(ci, seed, c.amp, via_w);
                    // A placing fails only when the wire has no path at all ([`State::any_path`]
                    // ignores room; the rips only free room, so not in practice): the round is undone.
                    if !(self.place)(self.st, ci, self.cap, self.wire_conn, &reg_w) || (self.tw5.p4_test_fail && k == 1) {
                        lost = true;
                        break;
                    }
                }
                cs.p4_ripped += vw.len();
                if lost {
                    self.restore(&before);
                    cs.p4_undone += 1;
                } else {
                    self.st.uncross_all(50);
                    let lr = (self.assign_t)(self.st, self.ties);
                    self.set_lr(lr, None);
                }
                // Taken as the loop's other whole-board moves are (`escape_take`): "always" as it
                // comes (the archive keeps the bests), "not_worse" undone when the evaluation rises.
                let worse = !lost && !self.tw5.take_always && self.eval_now() > e0 + 1e-6;
                if worse {
                    self.restore(&before);
                    cs.p4_worse += 1;
                } else if !lost {
                    for &ci in &cis {
                        self.reroutes[ci] += 1;
                        if let Some(x) = self.tw5.relay_w.as_mut().and_then(|(rw, _)| rw.get_mut(ci)) {
                            *x += 1.0;
                        }
                    }
                    cs.p4_rounds += 1;
                    self.offer_mid("in-loop P4 round".into(), cs);
                }
                // Its work: the counted layering and measuring, plus each placing at the mean
                // re-lay's work so far (a placing is a re-lay's search without its local judge).
                let relays = self.relays.max(1) as f64;
                let placing = vw.len() as f64 * self.tw5.p4_throttle.relay_work / relays;
                let w = self.budget_since(k0, c0) + placing;
                self.tw5.p4_throttle.ran_work(w);
                // site-price-cost (profile only): the round's throttle charge, its counter change and
                // its placings' part.
                if let Some(t0) = rt {
                    super::prof::work(serde_json::json!({"kind": "p4", "wall_s": t0.elapsed().as_secs_f64(), "cost": self.lctr.cost.get() - c0, "placing": placing, "work": w, "ripped": vw.len()}));
                }
                if self.dbg {
                    let t = self.dbg_crit();
                    eprintln!("LOOPDBG[{}] cycle {} loop P4: {} wires ripped, {}: {t}", self.dbg_tag, c.cycle, vw.len(), if lost { "undone (a wire found no path)" } else if worse { "undone (worse)" } else { "taken" });
                }
                true
            }
            /// TW5: offer the current state (after an escape, a taken in-loop P4 round) to the
            /// archive and the table at the cycle's end ([`Space::take_offers`]; lead 2026-11-19:
            /// "escapes offer their result to the table; the table keeps the bests"), unless the
            /// criteria already held cover it (Polubasov 2010 «Многовариантная оптимизация»: the
            /// table keeps the variants optimal for some criterion). O(state) when offered, O(rows)
            /// when covered.
            fn offer_mid(&mut self, label: String, cs: &mut CycleStats) {
                if !self.tw5.offer_on {
                    return;
                }
                let crit = self.criteria();
                if self.tw5.floor.iter().any(|f| f.covers(&crit)) {
                    cs.mid_covered += 1;
                    return;
                }
                let fp = self.fingerprint();
                let var = self.snapshot();
                // last-preview: a mid state better than everything the archive held may become the
                // run's pick at the cycle's end; its state is gone by then, so its sketch is taken now
                // (only while the caller shows sketches)
                if self.ctl.sketches.enabled() && self.tw5.floor.iter().all(|f| self.obj.better(&crit, f)) {
                    let sk = self.plan_sketch(0, &crit);
                    self.sk.mid.push((crit, sk));
                }
                self.tw5.floor.push(crit);
                self.tw5.offers.push((label, crit, var, fp));
            }
            /// TW5 (b): one layering escape (Polubasov 2010 «Оптимизация расслоения»): a random
            /// via-optimal layering of the whole board (the plateau walk,
            /// [`layers::DomainGraph::plateau_walk`]), taken as it comes (`escape_take`); the wires
            /// whose via count rose go to `front`, laid again first; then the history update.
            fn escape(&mut self, c: &CycleCtx, cs: &mut CycleStats, front: &mut std::collections::VecDeque<WireId>, fronted: &mut Vec<bool>) {
                let _pg = super::prof::g(super::prof::K::Escape);
                let (rt, rm, c0) = self.row_mark();
                // site-price-cost: an escape's fixed part (resync, history, bookkeeping).
                self.lctr.spend(super::escape::cost::ESCAPE_FIXED);
                self.resync();
                self.tw5.n_esc += 1;
                let seed = super::optimise::mix_seed(c.seed, 0xE5C0_0000 + self.tw5.n_esc) | 1;
                let vias0 = self.via_counts();
                let (now0, v0, p0) = (self.st.layers_now.clone(), self.lr.vias, self.pred.count);
                let tl = Instant::now();
                let lr = (self.tw5.assign_e)(self.st, seed);
                cs.esc_walked += lr.walked;
                let taken = self.take_layers(lr, seed);
                cs.layer_kept += usize::from(taken);
                cs.layer_runs += 1;
                cs.escapes += 1;
                let pr = self.st.proj;
                let free: Vec<Vec<bool>> = self
                    .st
                    .wires
                    .iter()
                    .map(|wi| if !wi.alive { Vec::new() } else { wi.tris.iter().map(|&t| pr.allowed(t, wi.net).map_or(pr.n_layers, |m| m.0.count_ones() as usize) > 1).collect() })
                    .collect();
                let (nf, nd) = super::escape::free_diff(&now0, &self.st.layers_now, &free);
                cs.esc_free += nf;
                cs.esc_diff += nd;
                cs.esc_vias_before += v0;
                cs.esc_vias_after += self.lr.vias;
                cs.esc_viol_up += usize::from(self.pred.count > p0);
                self.history_step(cs);
                if taken {
                    self.offer_mid(format!("escape {}", self.tw5.n_esc), cs);
                }
                cs.layer_s += tl.elapsed().as_secs_f64();
                let vias1 = self.via_counts();
                for w in 0..self.st.wires.len() {
                    if let Some(&ci) = self.wire_conn.get(w).filter(|&&ci| ci != usize::MAX && self.st.wires[w].alive) {
                        // Once per cycle per connection (the re-lay counter's role, Polubasov 2010
                        // fig. 11; a re-lay may give the wire a new id, the connection keeps its
                        // index): the front cannot refill forever, the sweep ends within
                        // alive + connections re-lays.
                        if fronted.len() <= ci {
                            fronted.resize(ci + 1, false);
                        }
                        if vias1.get(&ci).copied().unwrap_or(0) > vias0.get(&ci).copied().unwrap_or(0) && !fronted[ci] {
                            fronted[ci] = true;
                            front.push_back(w);
                        }
                    }
                }
                if self.tw5.cost_budget {
                    // site-price-cost: the escape's own measured cost (its layering, measure,
                    // history model), as the in-loop P4 round's.
                    self.tw5.throttle.ran_work(self.lctr.cost.get() - c0);
                } else {
                    let alive = self.st.wires.iter().filter(|w| w.alive).count();
                    self.tw5.throttle.ran(alive);
                }
                self.work_row("escape", rt, rm, c0);
                if self.dbg {
                    let t = self.dbg_crit();
                    eprintln!("LOOPDBG[{}] cycle {} escape {}: free {nf} changed {nd}, vias {v0} -> {}, {} wires via-rose: {t}", self.dbg_tag, c.cycle, self.tw5.n_esc, self.lr.vias, front.len());
                }
            }
            /// TW5 (d): floating up of wire `w` (Polubasov 2010 «Процедура всплытия»: the wire's
            /// stretches and the stretches it crosses may go to any layer): tried with the graded
            /// probability `b / (1 + b)` of its badness; the local layering of `w` and the wires it
            /// crosses (random among equal ones) kept when its price at the wire's coefficients
            /// (violations at the oscillating weight, vias at its via weight) drops.
            fn float_up(&mut self, w: WireId, seed: u64, amp: f64, via_w: f64, cs: &mut CycleStats) {
                let _pg = super::prof::g(super::prof::K::FloatWire);
                let Some(local) = self.local else { return };
                let Some(&ci) = self.wire_conn.get(w).filter(|&&ci| ci != usize::MAX) else { return };
                if !self.st.wires[w].alive {
                    return;
                }
                let b = self.badness(ci, w);
                let u = (super::optimise::mix_seed(seed ^ 0xF10A_7000, (w as u64) << 20 | u64::from(self.tries[ci])) >> 11) as f64 / (1u64 << 53) as f64;
                if u >= b / (1.0 + b) {
                    return;
                }
                cs.float_w_tried += 1;
                let (reg_w, amp_i) = self.try_reg(w, seed, amp, via_w);
                let via_mm = reg_w.weight("via").unwrap_or(0.0);
                let kv = self.viol_k(ci, seed ^ 0xF10A, amp_i);
                let saved = self.st.topology();
                self.st.took_part.clear();
                let ties = super::optimise::mix_seed(seed, 0xF1 << 32 | w as u64) | 1;
                let (lr, p) = local(self.st, &saved, &[w], &[w], &*self.lr, &self.pred, ties);
                let old = self.pred.price(kv) + via_mm * self.lr.vias_w;
                let new = p.price(kv) + via_mm * lr.vias_w;
                if new < old - 1e-6 {
                    self.set_lr(lr, Some(p));
                    self.pred_local = true;
                    cs.float_w_kept += 1;
                }
            }
            /// TW7 D: per connection its detour amount `(r - 1)^2` now, `r` = the projected length
            /// of its wires ([`State::wire_len`], taut with `[search] length = "taut"`) over the straight
            /// distance of its ends (0 for none). O(wires x path).
            fn detour_amounts(&self) -> Vec<f64> {
                let mut len = vec![0.0f64; self.conns.len()];
                for w in (0..self.st.wires.len()).filter(|&w| self.st.wires[w].alive) {
                    if let Some(&ci) = self.wire_conn.get(w).filter(|&&ci| ci < len.len()) {
                        len[ci] += self.st.wire_len(w);
                    }
                }
                self.conns
                    .iter()
                    .zip(&len)
                    .map(|(c, &l)| {
                        let d = (self.st.proj.pos[c.b] - self.st.proj.pos[c.a]).len();
                        if d > 0.0 && l > 0.0 { (l / d - 1.0).max(0.0).powi(2) } else { 0.0 }
                    })
                    .collect()
            }
            /// TW7 B: the patterns of the kinds `[optimize] pattern_moves` lists, between plain
            /// pad-to-pad wires of their own connection (as the batch pass: a wire of a free net
            /// tree keeps its place), each pair once (the first kind found wins: mutual evasion
            /// before U-clinch), the pair with the most excess length first (the graded detour
            /// amounts, [`super::xrules::detour_amount`]), each times `1 / (1 + w x stuck)` (`w` =
            /// `[optimize] pattern_stuck`, `stuck` = the pair's fruitless tries since its last kept
            /// move, [`Sp::pattern_key`]: graded, no cutoff); ties by wire ids. Detection only
            /// reads the state. O(detectors).
            fn pattern_cands(&self) -> Vec<PatCand> {
                use super::xrules::{self as xr, Kind};
                let _pg = super::prof::g(super::prof::K::XrDetect);
                let tree_now: BTreeSet<WireId> = if self.tw7.tree_free { tree::junctions(self.st).iter().flat_map(|j| [j.0, j.2]).collect() } else { BTreeSet::new() };
                let plain = |w: WireId| {
                    let Some(wi) = self.st.wires.get(w) else { return false };
                    let Some(&ci) = self.wire_conn.get(w) else { return false };
                    wi.alive && ci != usize::MAX && wi.turns.is_empty() && wi.pa != NO_OBS && wi.pb != NO_OBS && !tree_now.contains(&w) && {
                        let c = &self.conns[ci];
                        (wi.a, wi.b) == (c.a, c.b) || (wi.a, wi.b) == (c.b, c.a)
                    }
                };
                let has = |k: Kind| self.tw7.kinds.contains(&k);
                let mut v: Vec<xr::Pair> = Vec::new();
                if has(Kind::MutualEvasion) || has(Kind::UClinch) {
                    let uc = xr::u_clinches(self.st, &|w| !plain(w));
                    if has(Kind::MutualEvasion) {
                        v.extend(xr::mutual_evasions(self.st, &uc));
                    }
                    if has(Kind::UClinch) {
                        v.extend(uc);
                    }
                }
                if has(Kind::TerminalClinch) {
                    v.extend(xr::terminal_clinches(self.st));
                }
                if has(Kind::DoubleCrossing) {
                    v.extend(xr::double_crossings(self.st));
                }
                if has(Kind::Unlayerable) {
                    v.extend(self.lr.unlayerable_pairs.iter().map(|&(a, b)| xr::Pair { first: a.min(b), second: a.max(b), kind: Kind::Unlayerable }));
                }
                let rank = |amt: f64, key: &(usize, usize)| pattern_rank(amt, self.tw7.stuck.get(key).copied().unwrap_or(0), self.tw7.stuck_w);
                let mut seen: BTreeSet<(WireId, WireId)> = BTreeSet::new();
                let mut out: Vec<(f64, PatCand)> = Vec::new();
                for p in v {
                    if p.first == p.second || !plain(p.first) || !plain(p.second) || !seen.insert((p.first.min(p.second), p.first.max(p.second))) {
                        continue;
                    }
                    let amt = xr::detour_amount(self.st, p.first) + xr::detour_amount(self.st, p.second);
                    let key = self.pattern_key(p.first, p.second);
                    out.push((rank(amt, &key), PatCand { kind: p.kind, first: vec![p.first, p.second], obs: Vec::new(), hold: None, key, bump: Vec::new(), bump_h: 0.0 }));
                }
                // C (fig. 21b): a plain wire on one layer (no via) going round the wires that
                // cross its straight line; every one of those plain (a net tree's wire keeps its
                // place, so the wire could not go straight); one mover alone is the U-clinch's
                // pair when that kind found it. Ranked by the wire's own excess length.
                if has(Kind::ViaTrade) {
                    let layer = |w: WireId| -> Option<i32> {
                        let ev = self.lr.events.get(w).map_or(&[][..], |e| &e[..]);
                        ev.windows(2).all(|x| x[0].2 == x[1].2).then(|| ev.first().map_or(self.bottom, |e| e.2))
                    };
                    for (w, movers) in xr::via_trades(self.st, &|w| plain(w) && layer(w).is_some()) {
                        if !movers.iter().all(|&o| plain(o)) || (movers.len() == 1 && seen.contains(&(w.min(movers[0]), w.max(movers[0])))) {
                            continue;
                        }
                        let key = self.pattern_key(w, w);
                        out.push((rank(xr::detour_amount(self.st, w), &key), PatCand { kind: Kind::ViaTrade, first: vec![w], obs: movers, hold: layer(w), key, bump: Vec::new(), bump_h: 0.0 }));
                    }
                }
                // E (tw4-r2 (3)): a pour piece the plan cuts off, its plain bounding wires laid
                // again; a predicted violation, so it ranks before the excess lengths (graded by
                // the pads it cuts off, as the measure: 1 + 0.25 per further pad).
                if has(Kind::PourCut) {
                    if let Some(f) = self.pour_comps {
                        for c in f(self.st, &self.lr.events) {
                            let obs: Vec<WireId> = c.bounding.iter().copied().filter(|&w| plain(w)).collect();
                            if obs.is_empty() {
                                continue;
                            }
                            let key = (usize::MAX, c.pads.first().copied().unwrap_or(usize::MAX));
                            let g = 1.0 + 0.25 * c.pads.len().saturating_sub(1) as f64;
                            // The bounding wires' passes near the piece (within twice its radius
                            // plus 1 mm of its centroid) carry the cut's violation price for the
                            // move's search, spread over them (tw7-moves.md E: priced through the
                            // search's history, no new search term), so the wires look for a way
                            // that leaves the piece open; the judge is the board's evaluation.
                            let r = 2.0 * (c.area.max(0.0) / std::f64::consts::PI).sqrt() + MM;
                            let mut bump: Vec<usize> = obs
                                .iter()
                                .flat_map(|&w| self.st.wires[w].passes.iter().filter(|&&p| (self.st.pass_point(p) - c.at).len() <= r).map(|&p| self.st.passes[p].edge))
                                .collect();
                            bump.sort_unstable();
                            bump.dedup();
                            let cong = self.reg.price(&super::factors::Event::Congestion { wire: 0, edge: 0, history: 1.0 }, &super::factors::WireCtx { net: c.net, tier: 3 });
                            let bump_h = if cong > 0.0 && !bump.is_empty() { self.viol_mm * g / (cong * bump.len() as f64) } else { 0.0 };
                            out.push((rank(POUR_RANK * g, &key), PatCand { kind: Kind::PourCut, first: Vec::new(), obs, hold: None, key, bump, bump_h }));
                        }
                    }
                }
                out.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.first.cmp(&b.1.first)).then_with(|| a.1.obs.cmp(&b.1.obs)));
                out.into_iter().map(|x| x.1).collect()
            }
            /// TW7 B: a pattern's key in the fruitless-try counts (`[optimize] pattern_stuck`): its
            /// two wires' connections, the lower first (wire ids change at every re-lay, the
            /// connections stay).
            fn pattern_key(&self, x: WireId, y: WireId) -> (usize, usize) {
                let c = |w: WireId| self.wire_conn.get(w).copied().unwrap_or(usize::MAX);
                let (a, b) = (c(x), c(y));
                (a.min(b), a.max(b))
            }
            /// TW7 B: one targeted pattern move. The wires `order` (and the branches on their
            /// junctions) are taken out together and laid again, `order`'s connections first in
            /// that order (the rule's order: Luzin, Popov & Popov 2012 p. 118, the clinched wire
            /// first, the interfering one after it), the branches' after them in route order, each
            /// with its own try coefficients at the via weight `via_w`; the board is layered again
            /// (the local judge when on) and the move kept when the archive's evaluation of the
            /// whole board drops, else everything comes back exactly. Each connection of `order`
            /// counts the try. A priced move, no gate (as TW6's held re-lay, which lays in route
            /// order). Whether it was kept.
            fn pattern_move(&mut self, order: &[WireId], seed: u64, amp: f64, via_w: f64) -> bool {
                let _pg = super::prof::g(super::prof::K::PatternMove);
                self.held_relay(order, &[], None, 0.0, seed, amp, via_w)
            }
            /// TW7 E (`[optimize] pattern_moves` "pour"): the wires bounding a cut-off pour piece
            /// laid again together in route order, kept by the board's evaluation (with `[capacity]
            /// pour_cuts` the cut is a predicted violation in it). Whether it was kept.
            fn relay_pattern(&mut self, obs: &[WireId], seed: u64, amp: f64, via_w: f64) -> bool {
                let _pg = super::prof::g(super::prof::K::PatternMove);
                self.held_relay(&[], obs, None, 0.0, seed, amp, via_w)
            }
            /// TW7 C (`[optimize] pattern_moves` "via_trade", fig. 21b -> 21c; Luzin 2015 pp. 9-10,
            /// Luzin, Popov & Popov 2013 p. 134 step 8.1): wire `w` (on one layer) laid first, alone
            /// in the way it was going round (its movers taken out with it), then held to `layer`
            /// while the wires `movers` that cross its straight line are laid again in route order:
            /// each prices crossing it as the via it would imply ([`State::held_layers`]) and goes
            /// round its terminal where that is cheaper. Kept by the board's evaluation (as every
            /// pattern move); else everything comes back exactly. Whether it was kept.
            fn via_trade_move(&mut self, w: WireId, movers: &[WireId], layer: i32, seed: u64, amp: f64, via_w: f64) -> bool {
                let _pg = super::prof::g(super::prof::K::PatternMove);
                self.held_relay(&[w], movers, Some(layer), 0.0, seed, amp, via_w)
            }
            /// 5.8: the obstructed via pairs of wire `w` (Luzin, Popov & Popov 2013 p. 134 steps
            /// 1-8): a stretch of events `i..=j` on layer `l` between two layer changes `v`
            /// (from `Lv`, the layer of event `i-1`) and `u` (to `Lu`, event `j+1`; an SMD pad end
            /// counts by its event), whose part that must stay on `l` (events `i..=j`, the vias
            /// may sit anywhere in their intervals; measured where the wire meets the wires it
            /// crosses, [`super::viaopt::crossing_stretch_len`]) carries a price that grows with
            /// its length on the scale `scale` ([`super::viaopt::pair_len_price`]: no cutoff). The
            /// stretch may go onto `T` = `Lv` or `Lu` when every event of it is allowed there
            /// (step 4); the wires crossing it on `T` (`Lv ∩ Lcross`, step 8) obstruct that. With
            /// `Lv = Lu` both vias go, else the one on `T`'s side (the other changes type, step
            /// 8.2). A target crossed on `T` by nothing is the label lift's (`pair_min`), not
            /// this move's. The cheapest stretch first, per stretch the target with fewer
            /// obstructing wires first (step 6). O(events of `w` x path corners).
            fn obstructed_pairs(&self, w: WireId, scale: f64) -> Vec<PairCand> {
                let mut out = Vec::new();
                let has_conn = |x: WireId| self.st.wires.get(x).is_some_and(|wi| wi.alive) && self.wire_conn.get(x).is_some_and(|&c| c != usize::MAX);
                if !has_conn(w) || w >= self.lr.events.len() || w >= self.lr.partner.len() || w >= self.lr.mask.len() {
                    return out;
                }
                let ev = &self.lr.events[w];
                let n_sig = self.n_sig as i32;
                let mut i = 1;
                while i < ev.len() {
                    let l = ev[i].2;
                    let mut j = i;
                    while j + 1 < ev.len() && ev[j + 1].2 == l {
                        j += 1;
                    }
                    let (lv, lu) = (ev[i - 1].2, ev.get(j + 1).map_or(l, |e| e.2));
                    let sig = |x: i32| (0..n_sig).contains(&x);
                    if j + 1 < ev.len() && lv != l && lu != l && sig(l) {
                        let len = super::viaopt::crossing_stretch_len(self.st, w, &self.lr.events, &self.lr.partner[w], i, j);
                        let price = super::viaopt::pair_len_price(len, scale, self.via_mm);
                        let mut cands: Vec<PairCand> = Vec::new();
                        for t in if lv == lu { vec![lv] } else { vec![lv, lu] } {
                            if !sig(t) || !(i..=j).all(|k| self.lr.mask[w].get(k).is_some_and(|m| (m >> t) & 1 == 1)) {
                                continue;
                            }
                            let mut obs: Vec<WireId> = (i..=j)
                                .filter_map(|k| self.lr.partner[w].get(k).copied().flatten())
                                .filter(|&(w2, i2)| w2 != w && self.lr.events.get(w2).and_then(|e| e.get(i2)).is_some_and(|e| e.2 == t))
                                .map(|(w2, _)| w2)
                                .filter(|&w2| has_conn(w2) && self.st.wires[w2].net != self.st.wires[w].net)
                                .collect();
                            obs.sort_unstable();
                            obs.dedup();
                            if !obs.is_empty() {
                                cands.push(PairCand { span: (i, j), t, lv, lu, obs, len, price, one_side: false, gain: 0.0 });
                            }
                        }
                        cands.sort_by_key(|c| c.obs.len());
                        out.extend(cands);
                    }
                    i = j + 1;
                }
                out.sort_by(|a, b| a.price.total_cmp(&b.price));
                // thick-layer S3 (M2.3, design thick-layer.md: the wide trace on one side, its
                // crossers on the other, outcome (b) as a move; Luzin, Popov & Popov 2013 p. 134
                // step 8.1 with the stretch = the wire's whole run): for a wire with >= 2 layer
                // changes, per end layer `t` the events from the first to the last one not on `t`
                // held to `t` and every wire crossing that run on `t` laid again. After the stretch
                // pairs, ordered by gain (ω(w) x layer changes removed, larger first); no cap.
                if self.one_side {
                    let changes = ev.windows(2).filter(|x| x[0].2 != x[1].2).count();
                    let om = self.omega.of(self.st, w);
                    let mut os: Vec<PairCand> = Vec::new();
                    let ends: Vec<i32> = if ev.first().map(|e| e.2) == ev.last().map(|e| e.2) { ev.first().map(|e| e.2).into_iter().collect() } else { ev.first().into_iter().chain(ev.last()).map(|e| e.2).collect() };
                    for t in ends {
                        let sig = |x: i32| (0..n_sig).contains(&x);
                        let (Some(i), Some(j)) = (ev.iter().position(|e| e.2 != t), ev.iter().rposition(|e| e.2 != t)) else { continue };
                        if changes < 2 || !sig(t) || !(i..=j).all(|k| self.lr.mask[w].get(k).is_some_and(|m| (m >> t) & 1 == 1)) {
                            continue;
                        }
                        // Every event outside i..=j is on `t`: the hold removes all the layer changes.
                        let removed = changes;
                        let mut obs: Vec<WireId> = (i..=j)
                            .filter_map(|k| self.lr.partner[w].get(k).copied().flatten())
                            .filter(|&(w2, i2)| w2 != w && self.lr.events.get(w2).and_then(|e| e.get(i2)).is_some_and(|e| e.2 == t))
                            .map(|(w2, _)| w2)
                            .filter(|&w2| has_conn(w2) && self.st.wires[w2].net != self.st.wires[w].net)
                            .collect();
                        obs.sort_unstable();
                        obs.dedup();
                        if obs.is_empty() {
                            continue;
                        }
                        let lv = if i == 0 { t } else { ev[i - 1].2 };
                        let lu = ev.get(j + 1).map_or(t, |e| e.2);
                        let len = super::viaopt::crossing_stretch_len(self.st, w, &self.lr.events, &self.lr.partner[w], i, j);
                        os.push(PairCand { span: (i, j), t, lv, lu, obs, len, price: super::viaopt::pair_len_price(len, scale, self.via_mm), one_side: true, gain: om * removed as f64 });
                    }
                    os.sort_by(|a, b| b.gain.total_cmp(&a.gain).then(a.price.total_cmp(&b.price)));
                    out.extend(os);
                }
                out
            }

            /// 5.8 `[vias] pair_move`, the obstructing-wire half of the local via-pair
            /// minimisation (Luzin, Popov & Popov 2013 p. 134 step 8.1, fig. 1: "on the layers
            /// `Lv ∩ Lcross` lead the wires crossing the wire joining v and u round u"; Luzin 2013
            /// p. 104 fig. 21c: the variant without vias "cannot be obtained by a single reroute
            /// of wires"). Wire `a`'s stretch `c.span` is held to `c.t` for the search (its
            /// present layers, [`State::held_layers`], as if copied onto `c.t`: the via interval
            /// on the other side then joins `c.t` and that side's layer), the wires `c.obs`
            /// crossing it on `c.t` (and the branches on their junctions) are taken out together
            /// and laid again, in route order, each with its own try coefficients (as a single
            /// reroute of it: [`Sp::conn_reg`]) at the via weight `via_w`, so each prices crossing
            /// the stretch as the via it would imply and goes round it where that is cheaper; the
            /// board is layered again (the loop's tie seed) and the change is kept when the
            /// archive's evaluation of the whole board ([`evaluation`]: violations first, then
            /// vias and length by the objective) plus the stretch's graded price (`c.price`,
            /// [`super::viaopt::pair_len_price`]) drops below the evaluation before, else
            /// everything comes back exactly. Every obstructing wire's connection counts the try
            /// (its fruitless tries widen its coefficients, as a reroute's). A priced move, not a
            /// gate: nothing is refused by a threshold. Whether it was kept.
            fn pair_move(&mut self, a: WireId, c: &PairCand, seed: u64, amp: f64, via_w: f64) -> bool {
                let _pg = super::prof::g(super::prof::K::PairMove);
                let e0 = self.eval_now();
                let before = self.snapshot();
                let ev = self.lr.events[a].clone();
                let (i, j) = c.span;
                if let Some(now) = self.st.layers_now.get_mut(a).filter(|n| !n.is_empty()) {
                    let n = now.len();
                    let at = |k: usize| k.min(n - 1);
                    let bit = |l: i32| 1u64 << l.clamp(0, 63);
                    // thick-layer S3: a one-side span may start at the first event or end at the
                    // last one (the run from that end is held to `t`).
                    let (kv, ki, kj, ku) = (if i == 0 { 0 } else { at(ev[i - 1].0) }, at(ev[i].0), at(ev[j].0), ev.get(j + 1).map_or(n - 1, |e| at(e.0)));
                    // The stretch and the via intervals that go onto `t`; the interval on the
                    // side whose layer is not `t` keeps its via (`t` and that layer).
                    let (k0, k1) = (if c.t == c.lv { kv } else { ki }, if c.t == c.lu { ku } else { kj });
                    for x in &mut now[k0.min(k1)..=k1.max(k0)] {
                        *x = bit(c.t);
                    }
                    if c.t != c.lv {
                        for x in &mut now[kv.min(ki)..=ki.max(kv)] {
                            *x = bit(c.lv) | bit(c.t);
                        }
                    }
                    if c.t != c.lu {
                        for x in &mut now[kj.min(ku)..=ku.max(kj)] {
                            *x = bit(c.t) | bit(c.lu);
                        }
                    }
                }
                let obs_cis: BTreeSet<usize> = c.obs.iter().map(|&x| self.wire_conn[x]).collect();
                let gone: BTreeSet<WireId> = c.obs.iter().copied().collect();
                let set: Vec<WireId> = gone.iter().copied().chain(tree::dependents(self.st, &gone)).collect();
                let mut cis: Vec<usize> = set.iter().map(|&x| self.wire_conn[x]).collect();
                cis.sort_by_key(|&ci| (self.rank[ci], ci));
                cis.dedup();
                for &x in &set {
                    tree::remove_wire(self.st, x);
                }
                for &ci in &cis {
                    self.forced.borrow_mut().remove(&self.conn_of[ci]);
                }
                let regs: Vec<Registry> = cis.iter().map(|&ci| self.conn_reg(ci, seed, amp, via_w).0).collect();
                let n0 = self.st.wires.len();
                // loop-local: the wires this placing's double-crossing swaps take part in.
                self.st.took_part.clear();
                let placed = cis.iter().zip(&regs).all(|(&ci, reg)| (self.place)(self.st, ci, self.cap, self.wire_conn, reg));
                let mut kept = false;
                if placed {
                    // thick-layer S3 (lead 2026-11-23): the one-side move's run stays on `c.t` through
                    // the layering that follows, as a held stretch with a graded release price per
                    // event off `t` (`[vias] pair_hold` via prices x ω), not a lock; the
                    // board's evaluation then judges the result as for every pair move.
                    if c.one_side && self.pair_hold > 0.0 {
                        self.st.layer_hold = vec![(a, c.t, self.pair_hold * self.via_mm * self.omega.of(self.st, a))];
                    }
                    if let Some(local) = self.local {
                        // loop-local: the obstructing wires' move layered and priced locally.
                        let new: Vec<WireId> = (n0..self.st.wires.len()).filter(|&x| self.st.wires[x].alive).collect();
                        let (lr, p) = local(self.st, &before.topo, &set, &new, &before.lr, &before.pred, self.ties);
                        self.set_lr(lr, Some(p));
                        self.pred_local = true;
                    } else {
                        let lr = (self.assign_t)(self.st, self.ties);
                        self.set_lr(lr, None);
                    }
                    self.st.layer_hold.clear();
                    if c.one_side {
                        self.one_side_left = self.lr.events.get(a).map_or(0, |e| e.windows(2).filter(|x| x[0].2 != x[1].2).count());
                    }
                    let e1 = self.eval_now();
                    if self.dbg {
                        let c1 = self.criteria();
                        eprintln!("LOOPDBG[{}] pair move wire {a} events {i}..={j} ({} -> {} -> {}) onto {}: stretch {:.3} mm price {:.3}: wires {:?} laid again, eval {e0:.3} -> {e1:.3} (pred {} graded {:.3}, vias {}, len {:.3})", self.dbg_tag, c.lv, ev[i].2, c.lu, c.t, c.len / MM, c.price, c.obs, self.pred.count, self.pred.graded, c1.vias, c1.length_mm);
                    }
                    if e1 + c.price < e0 - 1e-6 {
                        for &ci in &cis {
                            self.reroutes[ci] += 1;
                        }
                        kept = true;
                    }
                }
                if !kept {
                    self.restore(&before);
                }
                for ci in obs_cis {
                    self.tries[ci] += 1;
                    if kept {
                        self.stuck[ci] = 0;
                    } else {
                        self.stuck[ci] += 1;
                    }
                }
                kept
            }
            /// TW6 part E: [`super::viaopt::siteless_intervals`] of wire `w` in the current layering.
            fn siteless(&self, w: WireId) -> Vec<(usize, usize, usize, f64)> {
                self.lr.events.get(w).map_or_else(Vec::new, |ev| super::viaopt::siteless_intervals(self.st, self.packer, w, ev))
            }
            fn has_conn(&self, x: WireId) -> bool {
                self.wire_conn.get(x).is_some_and(|&c| c != usize::MAX)
            }
            /// TW6 part E: [`super::viaopt::bounding_wires`] of events `i`, `i + 1` of wire `w`.
            fn bounding(&self, w: WireId, i: usize) -> Vec<WireId> {
                let p = self.lr.partner.get(w).map_or(&[][..], |p| &p[..]);
                super::viaopt::bounding_wires(self.st, w, p, i, &|x| self.has_conn(x))
            }
            /// TW6 part E: [`super::viaopt::tee_arms`] of the via interval after event `i` of wire
            /// `w` in the current layering. O(wires).
            fn tee_arms(&self, w: WireId, i: usize) -> Vec<WireId> {
                let ev = self.lr.events.get(w).map_or(&[][..], |e| &e[..]);
                super::viaopt::tee_arms(self.st, &tree::junctions(self.st), w, ev, i, &|x| self.has_conn(x))
            }
            /// TW6 part E: the wires `obs` (and the branches on their junctions) taken out together
            /// and laid again in route order, each with its own try coefficients at the via weight
            /// `via_w` (as [`Sp::pair_move`] lays its obstructing wires); the board is layered again
            /// and the change kept when the archive's evaluation of the whole board ([`evaluation`]:
            /// violations first, a via without a site among them, with `[vias] site_price` its
            /// graded deficit too) plus the relaid connections' graded detour price (TW6 r4,
            /// `[optimize] site_move_detour` = `self.move_detour`, [`conn_detour_price`] at the
            /// violation weight; after minus before) drops, else everything comes back exactly.
            /// Each connection counts the try. A priced move, no gate. Whether it was kept.
            fn relay_kept(&mut self, obs: &[WireId], seed: u64, amp: f64, via_w: f64) -> bool {
                self.held_relay(&[], obs, None, self.move_detour, seed, amp, via_w)
            }
            /// TW7 F: the one held re-lay of TW6 E's site / branch moves and TW7's pattern moves.
            /// The wires `first` and `obs` (and the branches on their junctions) are taken out
            /// together; `first`'s connections are laid again first, in that order (a pattern's
            /// rule order), then every other connection in route order, each with its own try
            /// coefficients at the via weight `via_w`. With `hold` the first connection's new
            /// wires are held to that layer for the search of the others ([`State::layers_now`]:
            /// crossing them prices the via it implies). The board is layered again (the local
            /// judge when on) and the change kept when the archive's evaluation of the whole
            /// board ([`evaluation`]) plus the relaid connections' graded detour price (`detour`
            /// = the excess-ratio scale, [`conn_detour_price`] at the violation weight; 0 = none)
            /// drops ([`site_keep`]), else everything comes back exactly. Every connection of
            /// `first` and `obs` counts the try. A priced move, no gate. Whether it was kept.
            #[allow(clippy::too_many_arguments)]
            fn held_relay(&mut self, first: &[WireId], obs: &[WireId], hold: Option<i32>, detour: f64, seed: u64, amp: f64, via_w: f64) -> bool {
                let e0 = self.eval_now();
                let before = self.snapshot();
                let own: Vec<usize> = first.iter().map(|&x| self.wire_conn[x]).collect();
                let try_cis: BTreeSet<usize> = first.iter().chain(obs).map(|&x| self.wire_conn[x]).collect();
                let gone: BTreeSet<WireId> = first.iter().chain(obs).copied().collect();
                let set: Vec<WireId> = gone.iter().copied().chain(tree::dependents(self.st, &gone)).collect();
                let mut rest: Vec<usize> = set.iter().map(|&x| self.wire_conn[x]).filter(|ci| !own.contains(ci)).collect();
                rest.sort_by_key(|&ci| (self.rank[ci], ci));
                rest.dedup();
                let mut cis = own;
                cis.dedup();
                cis.extend(rest);
                let det_cis: BTreeSet<usize> = cis.iter().copied().filter(|&c| c != usize::MAX).collect();
                let det_old = self.viol_mm * conn_detour_price(self.st, self.wire_conn, self.conns, &det_cis, detour);
                for &x in &set {
                    tree::remove_wire(self.st, x);
                }
                for &ci in &cis {
                    self.forced.borrow_mut().remove(&self.conn_of[ci]);
                }
                let regs: Vec<Registry> = cis.iter().map(|&ci| self.conn_reg(ci, seed, amp, via_w).0).collect();
                let n0 = self.st.wires.len();
                self.st.took_part.clear();
                let mut placed = true;
                for (k, (&ci, reg)) in cis.iter().zip(&regs).enumerate() {
                    if !(self.place)(self.st, ci, self.cap, self.wire_conn, reg) {
                        placed = false;
                        break;
                    }
                    if let (0, Some(l)) = (k, hold) {
                        // C: the subject's new wires on its layer for the others' search.
                        let bit = 1u64 << l.clamp(0, 63);
                        for x in n0..self.st.wires.len() {
                            if self.st.wires[x].alive && self.wire_conn.get(x) == Some(&ci) {
                                if self.st.layers_now.len() <= x {
                                    self.st.layers_now.resize(x + 1, Vec::new());
                                }
                                self.st.layers_now[x] = vec![bit; self.st.wires[x].tris.len()];
                            }
                        }
                    }
                }
                let mut kept = false;
                if placed {
                    if let Some(local) = self.local {
                        let new: Vec<WireId> = (n0..self.st.wires.len()).filter(|&x| self.st.wires[x].alive).collect();
                        let (lr, p) = local(self.st, &before.topo, &set, &new, &before.lr, &before.pred, self.ties);
                        self.set_lr(lr, Some(p));
                        self.pred_local = true;
                    } else {
                        let lr = (self.assign_t)(self.st, self.ties);
                        self.set_lr(lr, None);
                    }
                    let e1 = self.eval_now();
                    let det_new = self.viol_mm * conn_detour_price(self.st, self.wire_conn, self.conns, &det_cis, detour);
                    if self.dbg {
                        eprintln!("LOOPDBG[{}] held re-lay: wires {first:?} first (hold {hold:?}), then {obs:?}: eval {e0:.3} -> {e1:.3}, detour price {det_old:.3} -> {det_new:.3}", self.dbg_tag);
                    }
                    let (keep, refused) = site_keep(e0, e1, det_old, det_new);
                    if keep {
                        for &ci in &cis {
                            self.reroutes[ci] += 1;
                        }
                        kept = true;
                    }
                    self.site_refused += usize::from(refused);
                }
                if !kept {
                    self.restore(&before);
                }
                for ci in try_cis {
                    self.tries[ci] += 1;
                    if kept {
                        self.stuck[ci] = 0;
                    } else {
                        self.stuck[ci] += 1;
                    }
                }
                kept
            }
        }
        impl<'p> Space for Sp<'_, 'p> {
            type Var = LoopVar;
            fn criteria(&mut self) -> Criteria {
                let _pg = super::prof::g(super::prof::K::Criteria);
                let (mut len_mm, mut area_mm, mut shr) = (0.0, 0.0, 0usize);
                for w in (0..self.st.wires.len()).filter(|&w| self.st.wires[w].alive) {
                    let l = self.st.wire_len(w) / MM;
                    len_mm += l;
                    area_mm += l * (self.st.wires[w].w + self.st.wires[w].s) / self.ws0;
                    let (na, nb, mid) = self.st.neck_runs(w);
                    shr += usize::from(na > 0) + usize::from(nb > 0) + mid.iter().enumerate().filter(|&(i, &k)| i == 0 || mid[i - 1] + 1 != k).count();
                }
                let area = if matches!(self.obj, Objective::Area) { area_mm + self.via_area_mm * self.lr.vias_w } else { 0.0 };
                // The predicted violations (5.2a): the same measure the moves paid for; the cut
                // model's necks are shrinkages too (2.4).
                criteria_of(self.pred, self.viol_mm, self.lr.vias, len_mm, shr + self.pred.shrink_n, area)
            }
            fn snapshot(&mut self) -> LoopVar {
                let _pg = super::prof::g(super::prof::K::Snapshot);
                LoopVar { topo: self.st.topology(), wire_conn: self.wire_conn.clone(), lr: self.lr.clone(), forced: self.forced.borrow().clone(), reroutes: self.reroutes.clone(), pred: self.pred, pred_local: self.pred_local, pre: self.pre.clone() }
            }
            fn fingerprint(&mut self) -> u64 {
                // TW3: the wires' paths (triangles, turns, the pass points: their order on the
                // edges) and the layering events; equal states give equal values. O(state).
                let _pg = super::prof::g(super::prof::K::Fingerprint);
                let mut h: u64 = 0x5EED_F1A9_0000_0001;
                let mut add = |x: u64| {
                    h = (h ^ x).wrapping_mul(0x0100_0000_01B3).rotate_left(29);
                };
                for (w, wi) in self.st.wires.iter().enumerate().filter(|(_, wi)| wi.alive) {
                    add(w as u64);
                    add(wi.net as u64);
                    wi.tris.iter().for_each(|&t| add(t as u64));
                    wi.turns.iter().for_each(|&(k, v)| add(((k as u64) << 32) ^ v as u64));
                    for &p in &wi.passes {
                        let q = self.st.pass_point(p);
                        add(q.x.to_bits());
                        add(q.y.to_bits());
                    }
                    if let Some(ev) = self.lr.events.get(w) {
                        ev.iter().for_each(|&(k, f, l)| add((k as u64) ^ f.to_bits().rotate_left(7) ^ ((l as u64) << 48)));
                    }
                }
                h
            }
            fn offer_floor(&mut self, crits: Vec<Criteria>) {
                self.tw5.floor = crits;
                self.tw5.offers.clear();
            }
            fn take_offers(&mut self) -> Vec<(String, Criteria, LoopVar, u64)> {
                std::mem::take(&mut self.tw5.offers)
            }
            fn after_cycle(&mut self, cycle: usize, pick: &Criteria, own: &Criteria, restarts: usize) {
                // last-preview: the cycle's own state goes to the progress (the Last round row, the
                // cycle counter); a changed pick is the run's new best (the Last run row). Sketches
                // only while the caller shows them: the run's pick when it changed (its own end
                // state, or a mid state taken in `offer_mid`), the end state when one is asked for.
                let run = self.ctl.run.max(1);
                let at = self.ctl.since_start_s();
                self.ctl.with_progress(|p| note_loop_cycle(p, run, cycle, restarts, at, own));
                let mid = std::mem::take(&mut self.sk.mid);
                let changed = self.sk.pick.as_ref() != Some(pick);
                if changed {
                    self.sk.pick = Some(*pick);
                    self.ctl.with_progress(|p| note_run_pick(p, run, cycle, at, "plan", pick));
                }
                if !self.ctl.sketches.enabled() {
                    return;
                }
                let want = self.ctl.sketches.wanted();
                let mut sent_last = false;
                if changed {
                    let sk = if own == pick {
                        Some(self.plan_sketch(cycle, own))
                    } else {
                        mid.into_iter().find(|(c, _)| c == pick).map(|(_, mut sk)| {
                            sk.cycle = cycle as u32;
                            sk
                        })
                    };
                    if let Some(sk) = sk {
                        // the run's new pick is its own end state: the same sketch is the last one
                        let to_last = want && own == pick;
                        sent_last = to_last;
                        self.ctl.publish_sketch(sk, crate::control::SketchTo { last: to_last, best: true });
                    }
                }
                if want && !sent_last {
                    let sk = self.plan_sketch(cycle, own);
                    self.ctl.publish_sketch(sk, crate::control::SketchTo { last: true, best: false });
                }
            }
            fn restore(&mut self, v: &LoopVar) {
                let _pg = super::prof::g(super::prof::K::SpRestore);
                self.st.restore(v.topo.clone());
                self.wire_conn.clone_from(&v.wire_conn);
                self.reroutes.clone_from(&v.reroutes);
                *self.forced.borrow_mut() = v.forced.clone();
                self.set_lr(v.lr.clone(), Some(v.pred));
                self.pred_local = v.pred_local;
            }
            fn cycle(&mut self, c: &CycleCtx, stop: &mut dyn FnMut() -> bool) -> CycleStats {
                let _pg = super::prof::g(super::prof::K::Cycle);
                if super::prof::on() {
                    super::prof::units(super::prof::K::Cycle, self.st.wires.iter().filter(|w| w.alive).count());
                }
                let t0 = Instant::now();
                let k0 = (self.lctr.layer_global.get(), self.lctr.measure_global.get(), self.lctr.layer_local.get(), self.lctr.local_free.get(), self.lctr.local_model.get(), self.shadow);
                let mut cs = self.cycle_body(c, stop, t0);
                let (l, s0) = (self.lctr, k0.5);
                cs.layer_global = l.layer_global.get() - k0.0;
                cs.measure_global = l.measure_global.get() - k0.1;
                cs.layer_local = l.layer_local.get() - k0.2;
                cs.local_free = l.local_free.get() - k0.3;
                cs.local_model = l.local_model.get() - k0.4;
                cs.shadow_n = self.shadow.0 - s0.0;
                cs.shadow_agree = self.shadow.1 - s0.1;
                cs.shadow_err_graded = self.shadow.2 - s0.2;
                cs.shadow_err_vias = self.shadow.3 - s0.3;
                cs.shadow_unl_diff = self.shadow.4 - s0.4;
                cs.shadow_base_graded = self.shadow.5 - s0.5;
                cs
            }
        }
        impl<'p> Sp<'_, 'p> {
            /// One TopoR loop cycle ([`Space::cycle`]; the counters are taken round it).
            fn cycle_body(&mut self, c: &CycleCtx, stop: &mut dyn FnMut() -> bool, t0: Instant) -> CycleStats {
                let mut plap = Instant::now();
                let mut cs = CycleStats::default();
                let k_cycle = self.work_mark();
                if self.tw7.detour_k > 0.0 {
                    self.tw7.detour_amt = self.detour_amounts();
                }
                if self.dbg {
                    let t = self.dbg_crit();
                    eprintln!("LOOPDBG[{}] cycle {} start (amp {:.3}): {t}", self.dbg_tag, c.cycle, c.amp);
                }
                let mut topo = Topo {
                    reg: self.reg.clone(),
                    conns: self.conns.to_vec(),
                    tiers: self.tiers.to_vec(),
                    wire_conn: self.wire_conn.clone(),
                    reroutes: self.reroutes.clone(),
                    ws0: self.ws0,
                    cap: self.cap,
                    evals: 0,
                    max_evals: usize::MAX,
                    ripped: 0,
                    max_ripped: usize::MAX,
                    // Only the price is read here (the sweep order); the predicted violations are
                    // the board measure's.
                    tight: Vec::new(),
                    over_mm: Some(self.overflow_mm),
                    tally: Tally::default(),
                };
                // The sweep: every wire once, the most to gain first (its price minus the price of its
                // straight length); the via weight rises from the cycle's start to its end (5.3).
                let ids: Vec<WireId> = (0..self.st.wires.len()).filter(|&w| self.st.wires[w].alive && self.wire_conn.get(w).is_some_and(|&ci| ci != usize::MAX)).collect();
                let mut ws: Vec<(f64, WireId)> = ids.into_iter().map(|w| (topo.potential(self.st, w), w)).collect();
                ws.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
                let n = ws.len().max(1);
                super::prof::lap(&mut plap, super::prof::K::TopoSetup);
                let mut ripped = 0usize;
                // TW5: the re-lay counter's price fades per cycle; the wires whose via count rose in
                // a layering escape inside the sweep are laid again first (`front`).
                if let Some((rw, f)) = self.tw5.relay_w.as_mut() {
                    for x in rw.iter_mut() {
                        *x *= *f;
                    }
                }
                let mut front: std::collections::VecDeque<WireId> = std::collections::VecDeque::new();
                let mut fronted: Vec<bool> = vec![false; self.tries.len()];
                let mut j = 0usize;
                loop {
                    let (w, rose) = match front.pop_front() {
                        Some(w) => (w, true),
                        None if j < ws.len() => {
                            j += 1;
                            (ws[j - 1].1, false)
                        }
                        None => break,
                    };
                    if ripped >= self.max_ripped || stop() {
                        break;
                    }
                    if !self.st.wires[w].alive {
                        continue;
                    }
                    let via_w = c.emphasis.via_at(j.saturating_sub(usize::from(!rose)) as f64 / n as f64);
                    let k0 = self.work_mark();
                    let (rt, rm, c0) = self.row_mark();
                    ripped += 1;
                    let kept = if rose {
                        cs.relaid += 1;
                        let k = self.reroute(&mut topo, w, c.seed ^ 0xA5A5, c.amp, via_w);
                        cs.relaid_kept += usize::from(k);
                        k
                    } else {
                        cs.tried += 1;
                        let k = self.reroute(&mut topo, w, c.seed, c.amp, via_w);
                        cs.kept += usize::from(k);
                        k
                    };
                    if !kept && self.tw5.float_wire && self.st.wires[w].alive {
                        self.float_up(w, c.seed, c.amp, via_w, &mut cs);
                    }
                    // site-price-cost: a re-lay's fixed cost (path search, placing) on top of its
                    // counted layerings and judges.
                    let judged = self.lctr.local_free.get() + self.lctr.local_model.get() - rm.1;
                    self.lctr.spend(super::escape::cost::relay(self.st.passes.len(), judged));
                    let units = self.budget_since(k0, c0);
                    self.tw5.relay_wires += self.work_since(k0);
                    self.work_row("relay", rt, rm, c0);
                    self.tw5.throttle.add(units);
                    self.tw5.p4_throttle.add(units);
                    if self.tw5.escape && self.tw5.throttle.due() && !stop() {
                        self.escape(c, &mut cs, &mut front, &mut fronted);
                    }
                }
                if self.dbg {
                    let t = self.dbg_crit();
                    eprintln!("LOOPDBG[{}] cycle {} after sweep ({}/{} kept): {t}", self.dbg_tag, c.cycle, cs.kept, cs.tried);
                }
                // TW5 (f): one P4 rip-up round on what stays over-full after the sweep.
                if ripped < self.max_ripped && !stop() {
                    self.loop_p4(c, &mut cs, stop);
                }
                // limit_grace_s: the cycle's later steps are not begun once the loop is to stop (the
                // caller drops a cycle cut short, so the state it leaves is never offered).
                super::prof::lap(&mut plap, super::prof::K::StSweep);
                self.ctl.test_point("loop stage 1 check");
                if stop() {
                    cs.busy_s = t0.elapsed().as_secs_f64();
                    return cs;
                }
                self.ctl.test_point("loop stage 2 begin");
                if self.st.uncross_all(50) > 0 {
                    let lr = (self.assign_t)(self.st, self.ties);
                    self.set_lr(lr, None);
                    if self.dbg {
                        let t = self.dbg_crit();
                        eprintln!("LOOPDBG[{}] cycle {} after uncross: {t}", self.dbg_tag, c.cycle);
                    }
                }
                // Layer re-optimisation, a random one of the equal layerings (Polubasov 2010),
                // taken when the board's evaluation does not rise. The via counts before it are the
                // baseline of the via-rose relay below: the candidates are the wires whose via count
                // rose in the re-layering (Polubasov 2010, «Оптимизация расслоения»: «Хорошими
                // кандидатами на перекладку выступают те проводники, на которых увеличилось число
                // переходов», fig. 9a), not the ones that paid a via in the sweep.
                // TW5 (b): with the layering escape the layer re-optimisation runs inside the sweep
                // (above), as often as its share of the work allows; this per-cycle one is the
                // escape = "ties" stage.
                if !self.tw5.escape {
                let vias0 = self.via_counts();
                let tl = Instant::now();
                let lr = (self.assign_t)(self.st, c.seed | 1);
                if self.dbg {
                    let p = (self.measure)(self.st, &lr);
                    eprintln!("LOOPDBG[{}] cycle {} layer re-opt candidate: vias {} pred {} graded {:.3}", self.dbg_tag, c.cycle, lr.vias, p.count, p.graded);
                }
                cs.layer_kept += usize::from(self.take_layers(lr, c.seed | 1));
                self.history_step(&mut cs);
                if self.dbg {
                    let t = self.dbg_crit();
                    eprintln!("LOOPDBG[{}] cycle {} after layer re-opt (taken {}): {t}", self.dbg_tag, c.cycle, cs.layer_kept);
                }
                cs.layer_runs += 1;
                cs.layer_s += tl.elapsed().as_secs_f64();
                // The wires whose via count rose are laid again, at the end-of-cycle via weight.
                let vias1 = self.via_counts();
                super::prof::lap(&mut plap, super::prof::K::StUncrossReopt);
                let rose: Vec<WireId> = (0..self.st.wires.len())
                    .filter(|&w| self.st.wires[w].alive && self.wire_conn.get(w).is_some_and(|&ci| ci != usize::MAX && vias1.get(&ci).copied().unwrap_or(0) > vias0.get(&ci).copied().unwrap_or(0)))
                    .collect();
                for w in rose {
                    if ripped >= self.max_ripped || stop() {
                        break;
                    }
                    if !self.st.wires[w].alive {
                        continue;
                    }
                    cs.relaid += 1;
                    ripped += 1;
                    if self.reroute(&mut topo, w, c.seed ^ 0xA5A5, c.amp, c.emphasis.via_at(1.0)) {
                        cs.relaid_kept += 1;
                    }
                }
                if self.dbg {
                    let t = self.dbg_crit();
                    eprintln!("LOOPDBG[{}] cycle {} after via-rose relay ({}/{} kept): {t}", self.dbg_tag, c.cycle, cs.relaid_kept, cs.relaid);
                }
                if cs.relaid_kept > 0 {
                    if self.st.uncross_all(50) > 0 {
                        let lr = (self.assign_t)(self.st, self.ties);
                        self.set_lr(lr, None);
                    }
                    let tl = Instant::now();
                    let t = (c.seed ^ 0x5A5A) | 1;
                    let lr = (self.assign_t)(self.st, t);
                    cs.layer_kept += usize::from(self.take_layers(lr, t));
                    cs.layer_runs += 1;
                    cs.layer_s += tl.elapsed().as_secs_f64();
                }
                }
                super::prof::lap(&mut plap, super::prof::K::StViarose);
                self.ctl.test_point("loop stage 2 check");
                if stop() {
                    cs.busy_s = t0.elapsed().as_secs_f64();
                    return cs;
                }
                self.ctl.test_point("loop stage 3 begin");
                // Floating up (Polubasov 2010 «всплытие»): the label moves (crossing flips, via
                // shifts, segment lifting), each kept when the price drops; the new labelling is
                // taken when the board's evaluation does not rise.
                {
                    let dirs = self.packer.taut_dirs(self.st);
                    let cm = if self.cuts_on { CapModel::new_cut(self.st, self.packer, &dirs, &self.lr.events, self.bottom, self.overflow_mm) } else { CapModel::new(self.st, self.packer, &dirs, &self.lr.events, self.bottom, self.overflow_mm) };
                    let (mask, partner) = (self.lr.mask.clone(), self.lr.partner.clone());
                    let mut lb = Labels {
                        st: self.st,
                        cm,
                        evl: self.lr.events.clone(),
                        mask: &mask,
                        partner: &partner,
                        bottom: self.bottom,
                        via_mm: self.lp.via_mm,
                        omega: self.lp.omega,
                        n_signal: self.n_sig,
                        evals: 0,
                        max_evals: self.max_moves,
                        strict: !self.priced,
                        add_vias: self.priced && self.cut_vias,
                        tally: Tally::default(),
                    };
                    let lt = descend_labels(&mut lb);
                    // 5.4: the local via-pair minimisation's lift (Luzin-Popov 2013 steps 4-10).
                    let mut lifted = 0usize;
                    if let Some(max_len) = self.pair_max {
                        let ev0 = lb.evl.clone();
                        for w in lb.candidates() {
                            lifted += lb.pair_lifts(w, max_len, &|w2, i, j| super::viaopt::stretch_len(lb.st, w2, &ev0[w2], i, j));
                        }
                    }
                    let evl = std::mem::take(&mut lb.evl);
                    drop(lb);
                    let moved: usize = lt.applied.iter().sum::<usize>() + lifted;
                    if moved > 0 {
                        let mut lr = self.lr.clone();
                        layers::relabel_cached(self.st, &evl, &self.lp, &mut lr, self.scache);
                        let t = self.ties;
                        if self.take_layers(lr, t) {
                            cs.float_kept = moved - lifted;
                            cs.pair_lifts = lifted;
                        } else {
                            cs.undone += 1;
                        }
                    }
                }
                if self.dbg {
                    let t = self.dbg_crit();
                    eprintln!("LOOPDBG[{}] cycle {} after floating up (kept {}, undone {}): {t}", self.dbg_tag, c.cycle, cs.float_kept, cs.undone);
                }
                super::prof::lap(&mut plap, super::prof::K::StFloat);
                self.ctl.test_point("loop stage 3 check");
                if stop() {
                    cs.busy_s = t0.elapsed().as_secs_f64();
                    return cs;
                }
                self.ctl.test_point("loop stage 4 begin");
                // The crossing rules (clinches, double crossings, crossings no layer separates),
                // taken when the board's evaluation does not rise.
                if !self.tw7.kinds.is_empty() {
                    // TW7 B (`[optimize] pattern_moves`): each found pattern is one targeted move,
                    // judged alone by the board's evaluation (Luzin 2015 p. 7; Luzin, Popov &
                    // Popov 2012 p. 118 «Устранение клинчей»), instead of the batch pass below.
                    // `[optimize] pattern_share`: the loop's other work since the last pattern stage
                    // earns the moves their budget; each move spends its measured work; the
                    // candidates come most excess length first, the rest wait for a later cycle.
                    let other = self.work_since(self.tw7.mark.unwrap_or(k_cycle));
                    self.tw7.share.earn(other);
                    cs.pat_base += other;
                    let found = self.pattern_cands();
                    cs.pat_found += found.len();
                    for p in &found {
                        cs.pat_kinds[p.kind.idx()][0] += 1;
                    }
                    for (i, p) in found.iter().enumerate() {
                        if ripped >= self.max_ripped || stop() {
                            break;
                        }
                        if !self.tw7.share.allows() {
                            cs.pat_waited += found.len() - i;
                            break;
                        }
                        let live = |x: WireId| self.st.wires.get(x).is_some_and(|wi| wi.alive) && self.wire_conn.get(x).is_some_and(|&ci| ci != usize::MAX);
                        if !p.first.iter().chain(&p.obs).all(|&x| live(x)) {
                            continue;
                        }
                        let k0 = self.work_mark();
                        cs.pat_retried += usize::from(self.tw7.stuck.get(&p.key).is_some_and(|&n| n > 0));
                        let mut tries = 1usize;
                        cs.pat_moved += 1;
                        ripped += p.first.len() + p.obs.len();
                        let via_w = c.emphasis.via_at(1.0);
                        let mut kept = match (p.kind, p.hold) {
                            (super::xrules::Kind::ViaTrade, Some(l)) => self.via_trade_move(p.first[0], &p.obs, l, c.seed ^ 0x7E80, c.amp, via_w),
                            (super::xrules::Kind::PourCut, _) => {
                                let old: Vec<(usize, f64)> = p.bump.iter().filter_map(|&e| self.st.history.get(e).map(|&h| (e, h))).collect();
                                for &(e, h) in &old {
                                    self.st.history[e] = h + p.bump_h;
                                }
                                let kept = self.relay_pattern(&p.obs, c.seed ^ 0x7E81, c.amp, via_w);
                                for &(e, h) in &old {
                                    self.st.history[e] = h;
                                }
                                kept
                            }
                            _ => self.pattern_move(&p.first, c.seed ^ 0x7E7E, c.amp, via_w),
                        };
                        if !kept && p.kind == super::xrules::Kind::MutualEvasion && ripped < self.max_ripped && !stop() {
                            tries += 1;
                            cs.pat_moved += 1;
                            ripped += 2;
                            kept = self.pattern_move(&[p.first[1], p.first[0]], c.seed ^ 0x7E7F, c.amp, via_w);
                        }
                        // Its work: the counted layering and measuring, at least one unit per try
                        // (a try whose placing failed still searched).
                        let w = self.work_since(k0).max(tries as f64);
                        self.tw7.share.spend(w);
                        cs.pat_work += w;
                        cs.pat_kinds[p.kind.idx()][1] += tries;
                        cs.pat_kinds[p.kind.idx()][2] += usize::from(kept);
                        cs.pat_kept += usize::from(kept);
                        // `[optimize] pattern_stuck`: the pattern's fruitless tries (graded order).
                        if kept {
                            self.tw7.stuck.remove(&p.key);
                        } else {
                            *self.tw7.stuck.entry(p.key).or_insert(0) += tries as u32;
                        }
                    }
                    self.tw7.mark = Some(self.work_mark());
                    cs.pat_stuck = self.tw7.stuck.len();
                } else {
                let x0 = (self.xkept)();
                self.resync();
                let before = self.snapshot();
                let e0 = self.eval_now();
                let mut rip = 0usize;
                (self.cross)(self.st, self.wire_conn, self.reroutes, self.lr, &mut rip, 100 + c.cycle);
                if (self.xkept)() > x0 {
                    let lr = self.lr.clone();
                    self.set_lr(lr, None);
                    if self.tw5.take_always || self.eval_now() <= e0 + 1e-6 {
                        cs.cross_kept = (self.xkept)() - x0;
                        self.ties = 0;
                    } else {
                        self.restore(&before);
                        cs.undone += 1;
                    }
                }
                }
                super::prof::lap(&mut plap, super::prof::K::StCross);
                // 5.4 `[vias] pair_min`, the re-lay half (Luzin-Popov 2013 step 8.1, here for the
                // wire itself): a wire that dives to another layer and comes back over a short
                // stretch is laid again at the end-of-cycle via weight, so a way round on its own
                // layer that costs less than the two vias wins (kept by the move's price).
                if let Some(max_len) = self.pair_max {
                    let mut pw: Vec<WireId> = Vec::new();
                    for w in (0..self.st.wires.len()).filter(|&w| self.st.wires[w].alive && w < self.lr.events.len()) {
                        let ev = &self.lr.events[w];
                        let mut i = 1;
                        while i < ev.len() {
                            let l = ev[i].2;
                            let mut j = i;
                            while j + 1 < ev.len() && ev[j + 1].2 == l {
                                j += 1;
                            }
                            if ev[i - 1].2 != l && ev.get(j + 1).is_some_and(|e| e.2 == ev[i - 1].2) && super::viaopt::stretch_len(self.st, w, ev, i - 1, j + 1) <= max_len {
                                pw.push(w);
                                break;
                            }
                            i = j + 1;
                        }
                    }
                    for w in pw {
                        if ripped >= self.max_ripped || stop() || !self.st.wires[w].alive || self.wire_conn.get(w).is_none_or(|&ci| ci == usize::MAX) {
                            continue;
                        }
                        cs.pair_relaid += 1;
                        ripped += 1;
                        if self.reroute(&mut topo, w, c.seed ^ 0x7A7A, c.amp, c.emphasis.via_at(1.0)) {
                            cs.pair_relaid_kept += 1;
                        }
                    }
                }
                // 5.8 `[vias] pair_move`, the obstructing-wire half (Luzin-Popov 2013 step 8.1):
                // per wire (id order) its obstructed via pairs, each tried once at the
                // end-of-cycle via weight; after a kept move the wire's pairs are read again from
                // the new layering (the pair it removed is gone, the next one moves up).
                super::prof::lap(&mut plap, super::prof::K::StPairrelay);
                if let Some(scale) = self.pair_move {
                    let mut w = 0;
                    while w < self.st.wires.len() {
                        let mut skip = 0usize;
                        let mut tries = 0usize;
                        let mut limit = usize::MAX;
                        loop {
                            let pairs = self.obstructed_pairs(w, scale);
                            limit = limit.min(pairs.len());
                            if tries >= limit || ripped >= self.max_ripped || stop() {
                                break;
                            }
                            let Some(pc) = pairs.into_iter().nth(skip) else { break };
                            tries += 1;
                            cs.pair_moved += 1;
                            cs.pair_moved_wires += pc.obs.len();
                            ripped += pc.obs.len();
                            let (t0, c0) = (Instant::now(), super::prof::cpu_ns());
                            let kept = self.pair_move(w, &pc, c.seed ^ 0x3C3C, c.amp, c.emphasis.via_at(1.0));
                            if pc.one_side {
                                cs.one_side_tried += 1;
                                cs.one_side_kept += usize::from(kept);
                                cs.one_side_left += self.one_side_left;
                                cs.one_side_ns += t0.elapsed().as_nanos() as u64;
                                cs.one_side_cpu_ns += super::prof::cpu_ns().saturating_sub(c0);
                            } else {
                                cs.pair_ns += t0.elapsed().as_nanos() as u64;
                                cs.pair_cpu_ns += super::prof::cpu_ns().saturating_sub(c0);
                            }
                            if kept {
                                cs.pair_moved_kept += 1;
                                cs.pair_moved_long += usize::from(pc.len > scale);
                            } else {
                                skip += 1;
                            }
                        }
                        w += 1;
                    }
                }
                super::prof::lap(&mut plap, super::prof::K::StPairmove);
                // TW6 part E (`[optimize] site_move` / `branch_move`; TopoR 6.3 manual p. 179, Shift
                // Vias submode 2: "wires can be rerouted as well"; Luzin, Popov & Popov 2013 p. 134
                // step 8.1): per wire (id order, the wires present at the start) its via interval
                // without a site with the largest amount: (i) the wire laid again (the single
                // reroute, its own judge); not kept: (ii) the wires whose crossings close the
                // interval in laid again together, so it can grow to a site; then the branch arms
                // of the junctions that bound it (their tee events). Each kept by the board's
                // evaluation plus, TW6 r4, the graded detour price of the relaid connections
                // (`[optimize] site_move_detour`).
                if self.site_move || self.branch_move {
                    let refused0 = self.site_refused;
                    let det = self.move_detour;
                    let n_w = self.st.wires.len();
                    for w in 0..n_w {
                        if ripped >= self.max_ripped || stop() {
                            break;
                        }
                        if !self.st.wires[w].alive || self.wire_conn.get(w).is_none_or(|&ci| ci == usize::MAX) {
                            continue;
                        }
                        let Some(&(i0, ..)) = self.siteless(w).first() else { continue };
                        let mut tee = i0;
                        if self.site_move {
                            cs.site_moved += 1;
                            ripped += 1;
                            if self.reroute_d(&mut topo, w, c.seed ^ 0x5173, c.amp, c.emphasis.via_at(1.0), det) {
                                cs.site_kept += 1;
                                continue;
                            }
                            let Some(&(i, ..)) = self.siteless(w).first() else { continue };
                            tee = i;
                            let obs = self.bounding(w, i);
                            if !obs.is_empty() && ripped < self.max_ripped && !stop() {
                                cs.site_moved += 1;
                                ripped += obs.len();
                                if self.relay_kept(&obs, c.seed ^ 0x5174, c.amp, c.emphasis.via_at(1.0)) {
                                    cs.site_kept += 1;
                                    continue;
                                }
                            }
                        }
                        if self.branch_move {
                            let arms: Vec<WireId> = self.tee_arms(w, tee).into_iter().filter(|&b| !(self.site_move && b == w)).collect();
                            for b in arms {
                                if ripped >= self.max_ripped || stop() || !self.st.wires[b].alive {
                                    break;
                                }
                                cs.branch_moved += 1;
                                ripped += 1;
                                if self.relay_kept(&[b], c.seed ^ 0xB7A9, c.amp, c.emphasis.via_at(1.0)) {
                                    cs.branch_kept += 1;
                                    break;
                                }
                            }
                        }
                    }
                    cs.site_detour_refused += self.site_refused - refused0;
                }
                // loop-local: the variant this cycle offers carries the board's measure.
                self.resync();
                cs.busy_s = t0.elapsed().as_secs_f64();
                if self.dbg {
                    let t = self.dbg_crit();
                    eprintln!("LOOPDBG[{}] cycle {} end (crossing passes {}, undone {}): {t}", self.dbg_tag, c.cycle, cs.cross_kept, cs.undone);
                }
                cs
            }
        }
        let place_ref = &place;
        let pour_comps_ref: Option<&dyn Fn(&State, &[Vec<(usize, f64, i32)>]) -> Vec<super::pourplan::CutComp>> = pour_base.as_ref().filter(|pb| pb.any).map(|_| &pour_comps_fn as &dyn Fn(&State, &[Vec<(usize, f64, i32)>]) -> Vec<super::pourplan::CutComp>);
        let assign_ref = &assign_t;
        let assign_esc_ref = &assign_esc;
        let measure_ref = &measure;
        let local_ref: LocalFn = &local_judge;
        let cross_ref = &crossing_pass;
        let xkept = || xstats.get()[3];
        // The loop's search is layer-aware (5.2b): the crossings price the via they imply against
        // the stretches' present layers; both are put back after the loop.
        let cross_vias0 = st.cross_vias;
        st.cross_vias = true;
        st.layers_now = layers_now(&st, &lr.events, bottom);
        // TW5 (4): crossing a wire that crosses nothing is priced by its freedom (free_wire_cross).
        st.free_wire_a = opts.free_wire_cross;
        st.mark_free_wires(&lr.events);
        // TW6 part B search (`[factors] via_site_social` > 0): a crossing where neither wire has a
        // via site is priced by the smaller of their site amounts (the packer's one measure).
        if opts.factors.weight.get("via_site_social").is_some_and(|&w| w > 0.0) {
            st.site_rooms = Some(packer.site_rooms());
            st.site_strict = packer.via_between;
            st.mark_site_k(&lr.events);
        }
        // TW3: the pre-loop decisions of this run (a draw-only round takes its row's).
        let pre_loop: std::sync::Arc<PreLoop> = match draw_row {
            Some(d) => {
                let p = d.row.var.pre.clone();
                for (i, r) in &p.failed {
                    if out[*i].reason.is_empty() {
                        fail(&mut out, *i, r.clone());
                    }
                }
                term_over.clone_from(&p.term_over);
                for (o, &g) in out.iter_mut().zip(&p.ghost) {
                    o.ghost_nm = g;
                }
                p
            }
            None => std::sync::Arc::new(PreLoop { failed: out.iter().enumerate().filter(|(_, o)| !o.reason.is_empty()).map(|(i, o)| (i, o.reason.clone())).collect(), term_over: term_over.clone(), ghost: out.iter().map(|o| o.ghost_nm).collect() }),
        };
        let pred0 = measure(&st, &lr);
        // TW5: the board's alive wires (the throttle's first escape), the re-lay counts and the
        // history the loop starts from.
        let st_alive = st.wires.iter().filter(|w| w.alive).count();
        // site-price-cost: the budget's currency; with "cost" the first escape (or in-loop P4
        // round) is due after (1/s - 1) x the cost of one whole-board layering and measure of this
        // board (the last layering's evaluations), else after kappa x alive wire units.
        let cost_budget = opts.escape_budget == "cost";
        let share_each = if opts.escape == "layering" && opts.loop_p4 { opts.escape_share * 0.5 } else { opts.escape_share };
        let first_cost = super::escape::cost::ESCAPE_FIXED + super::escape::cost::global(st_alive, lr.refine_stream.0, st.passes.len()) + super::escape::cost::measure(st.passes.len());
        if cost_budget && opts.escape == "layering" {
            // site-price-cost (profile only): the first wait's inputs, for the tests.
            super::prof::note("escape_first", serde_json::json!({"first": first_cost, "share": share_each, "alive": st_alive, "evals": lr.refine_stream.0, "passes": st.passes.len()}));
        }
        let throttle0 = || if cost_budget { super::escape::Throttle::new_work(share_each, first_cost) } else { super::escape::Throttle::new(share_each, opts.escape_kappa, st_alive) };
        let reroutes0 = reroutes.clone();
        let hist_base0 = st.history.clone();
        // The connections in the plan: every one not failed so far is drawn (M-B1 never drop), so
        // the live Best so far counts them as routed (predicted).
        let planned = out.iter().filter(|o| o.reason.is_empty()).count();
        let mut sp = Sp {
            ctl,
            st: &mut st,
            wire_conn: &mut wire_conn,
            lr: &mut lr,
            reroutes: &mut reroutes,
            stuck: vec![0; conns.len()],
            tries: vec![0; conns.len()],
            conns: &conns,
            conn_of: &conn_of,
            tiers: &tiers,
            rank: &rank,
            reg: &reg,
            lp,
            n_sig,
            bottom,
            cap,
            ws0,
            overflow_mm,
            viol_mm,
            cuts_on,
            priced,
            obj,
            via_area_mm,
            crossing_share: opts.loop_crossing.max(0.0),
            max_ripped: opts.max_ripped,
            max_moves: opts.max_moves,
            cut_vias: opts.cut_vias,
            packer: &packer,
            forced: &forced,
            place: place_ref,
            assign_t: assign_ref,
            measure: measure_ref,
            cross: cross_ref,
            xkept: &xkept,
            ties: 0,
            pred: pred0,
            pair_max: opts.via_pair_min.then(|| opts.via_pair_max_mm.max(0.0) * MM),
            pair_move: opts.via_pair_move.then(|| opts.via_pair_max_mm.max(0.0) * MM),
            one_side: opts.via_pair_one_side,
            pair_hold: opts.via_pair_hold.max(0.0),
            one_side_left: 0,
            omega: lp.omega,
            site_move: opts.opt_site_move,
            branch_move: opts.opt_branch_move,
            move_detour: opts.opt_site_move_detour.max(0.0),
            site_refused: 0,
            via_mm: opts.mm_per_via,
            scache: scache_ref,
            local: judge_local.then_some(local_ref),
            pred_local: false,
            shadow_k: opts.loop_shadow,
            relays: 0,
            lctr: &lctr,
            shadow: (0, 0, 0.0, 0, 0, 0.0),
            dbg: std::env::var_os("TOPO_LOOPDBG").is_some(),
            dbg_tag: opts.seed,
            tw5: Tw5 {
                bounded: opts.oscillation == "bounded",
                amp_lo: opts.osc_amp_min,
                amp_hi: opts.osc_amp_max,
                escape: opts.escape == "layering",
                assign_e: assign_esc_ref,
                throttle: throttle0(),
                p4_throttle: throttle0(),
                cost_budget,
                relay_wires: 0.0,
                take_always: opts.escape_take == "always",
                relay_w: (opts.relay_count_fade < 1.0).then(|| (reroutes0.iter().map(|&r| f64::from(r)).collect(), opts.relay_count_fade.max(0.0))),
                // TW5 (f): the in-loop P4 rounds price with the loop history, so it is on with them.
                hist: (opts.loop_history || opts.loop_p4).then(|| super::escape::History::new(proj.edges.len(), opts.loop_history_fade)),
                hist_base: hist_base0,
                float_wire: opts.float_wire && judge_local,
                loop_p4: opts.loop_p4,
                p4_test_fail: opts.loop_p4_test_fail,
                n_esc: 0,
                offer_on: opts.escape_offer,
                floor: Vec::new(),
                offers: Vec::new(),
            },
            tw7: Tw7 {
                kinds: opts.pattern_moves.iter().filter_map(|k| super::xrules::Kind::parse(k)).collect(),
                tree_free,
                detour_k: opts.detour_k.max(0.0),
                detour_amt: Vec::new(),
                share: super::escape::Share::new(opts.pattern_share),
                mark: None,
                stuck_w: if opts.pattern_stuck.is_finite() { opts.pattern_stuck.max(0.0) } else { 0.0 },
                stuck: BTreeMap::new(),
            },
            pre: pre_loop.clone(),
            pour_comps: pour_comps_ref,
            board: b,
            sk: SketchBook::default(),
            places: &places,
        };
        let bounded_osc = opts.oscillation == "bounded";
        // TW3 (D6): the dialog run has no cycle cap (OK, Cancel or the time limit end it); headless
        // `rounds` is the work cap of the comparisons.
        let dialog = !(opts.stop_when_stable || opts.finish_when_stable);
        let params = LoopParams {
            // TW3: the dialog's loop has no cycle cap (OK / Cancel / the limit end it); `[run]
            // stop_after = "plan"` (a plan-only debug run, nothing drawn) keeps rounds as its cap
            cycles: if dialog && opts.restart != "stall" && opts.restart != "off" && opts.stop_after != "plan" { usize::MAX - 1 } else { opts.topo_rounds.max(1) },
            restart_after: opts.loop_restart,
            seed: seed_of(opts.seed, round, 5000),
            fluctuation: fluct0,
            // TW5 (a): bounded oscillation = no cooling, no reheat (the per-wire amplitude is
            // graded and bounded instead).
            narrowing: if bounded_osc { 1.0 } else { opts.fluctuation_narrowing },
            stuck: opts.fluctuation_stuck,
            reheat: if bounded_osc { 0.0 } else { opts.fluctuation_reheat },
            stable_window: opts.loop_stable_window.max(1),
            stable_gain_pct: opts.stable_gain_pct,
            // headless: stop at stable (+ stable_patience); dialog: only with finish_when_stable (+ finish_patience)
            stable_patience: if opts.stop_when_stable { opts.stable_patience } else { opts.finish_patience },
            stop_at_stable: opts.stop_when_stable || opts.finish_when_stable,
            emphasis: Emphasis { via_start: opts.loop_via_start, via_end: opts.loop_via_end },
            obj,
            restart: super::optimise::Restart::parse(&opts.restart),
            restart_relays: opts.restart_relays,
            restart_min_cycles: opts.restart_min_cycles,
            relay_budget: opts.loop_relays,
            run: gate.0,
            runs: opts.runs.max(1),
        };
        let mut arch: Archive<Gen<LoopVar>> = Archive::new(opts.variants);
        if let Some(d) = draw_row {
            // TW3 draw-only round: the row is the one variant; no cycle runs.
            arch.offer(Variant { label: d.label.clone(), crit: d.crit, payload: Gen { var: (*d.row.var).clone(), gen: d.row.gen, cycle: d.row.cycle, at_s: 0.0 } }, &obj);
        }
        // M2.TL: between cycles the loop ends at Cancel / OK, the time limit or its share of it
        // (no new step begins); inside a cycle only Cancel / OK stops it with the soft limit (the
        // cycle finishes and is offered), with `hard` the limits too. A cycle Cancel cut short is
        // not offered (Cancel = the best variant kept so far).
        let loop_over = || loop_deadline().is_some_and(|d| ctl.limit.now() >= d);
        // Fair slots (sync-wait; user 2026-11-17 "free the slot"): before each cycle the run gives
        // its compute slot to a waiting run that is behind it (not begun yet, or fewer cycles
        // finished, [`Slots::turn`]) and goes on when it is the lowest waiter again; with fewer
        // engine threads than runs the runs so share the slots in cycles, every run begins at once
        // and every run can become stable while the loops run (a stable loop only waits for OK: a
        // dialog run never ends by itself at stable). Its own result does not change (the cycles
        // depend only on the run's state and its table views). Never past the time limit
        // (limit-wait: bounded waits): the loop stops at this check then, and a yield would wait for
        // a slot, unbounded by `[run] limit_grace_s`.
        let stable_seen = std::cell::Cell::new(false);
        let (yields, yield_s) = (std::cell::Cell::new(0usize), std::cell::Cell::new(0.0f64));
        // cycles finished (the fair order's clock) and the slot wait since the last cycle line
        let (done_cycles, turn_s) = (std::cell::Cell::new(0usize), std::cell::Cell::new(0.0f64));
        let mut stop = || {
            if !stop_now() {
                let stable = stable_seen.get();
                if stable {
                    ctl.test_point("stable yield check");
                }
                let t_y = Instant::now();
                if !ctl.over_limit() && gate.1.slots.turn(done_cycles.get(), gate.0) {
                    let w = t_y.elapsed().as_secs_f64();
                    turn_s.set(turn_s.get() + w);
                    if stable {
                        ctl.test_point("stable slot given");
                        yields.set(yields.get() + 1);
                        yield_s.set(yield_s.get() + w);
                    }
                }
            }
            stop_now() || ctl.limit_reached() || loop_over()
        };
        // `[run] limit_grace_s`: past the limit and its grace the step stops at its next check (the
        // cycle it cut is not offered; the archive's best variant is kept). Read only after the limit
        // passed, so a run without a limit takes the same moves in the same order.
        let mut inner = || ctl.test_point("loop move") || stop_now() || ctl.limit.cuts() || ctl.limit.grace_over() || (!soft() && loop_over());
        let clock = || t_loop.elapsed().as_secs_f64();
        // M0: the progress dialog names the loop (tests/cancel_latency.rs waits for it).
        ctl.stage(rnd, rnds, "TopoR loop on the topology");
        // "Report stable live" (user 2026-11-17): the progress says "Routes stable" while the loop
        // still runs, so the dialog's OK (= Cancel) ends the loop there ("Cancel / OK", labelled
        // "stable" by route_topo); a dialog run never ends BY ITSELF at stable (finish_when_stable off).
        let live = gate.3;
        let total = nconn;
        let mut on_pick = |cycle: usize, pick: &Criteria, stable: bool| {
            done_cycles.set(cycle);
            stable_seen.set(stable);
            live(&LiveLoop { pick: *pick, planned, total, stable });
        };
        // sync-wait: one line per cycle to the run log (log.txt): timing and errors, nothing fed back
        let run_no = gate.0 + 1;
        let note = |n: &super::optimise::CycleNote| {
            let from = n.restart_from.map(|(r, c)| format!(", restart from run {} cycle {c}", r + 1)).unwrap_or_default();
            ctl.cycle_line(format!(
                "loop cycle: run {run_no}, cycle {}, at {:.2} s, cycle {:.3} s, table wait {:.3} s, slot wait {:.3} s, errors {} (shorts {}), pick errors {}, pick {:.1}{from}",
                n.cycle,
                ctl.limit.elapsed_s(),
                n.cycle_s,
                n.sync_s,
                turn_s.replace(0.0),
                n.errors,
                n.shorts,
                n.pick_errors,
                n.pick_value
            ));
        };
        let compat = sp.pre.key();
        let port = gate.1.table.as_ref().filter(|_| draw_row.is_none()).map(|t| Port { table: t, k: gate.0, compat, slots: &gate.1.slots, note: &note });
        let mut rep = if draw_row.is_some() {
            LoopReport { stop: "draw-only".into(), ..LoopReport::default() }
        } else {
            gate.1.slots.enter_loop();
            let rep = crate::control::in_loop_scope(|| optimise_shared(&mut sp, &mut arch, &params, &mut stop, &mut inner, &clock, &mut on_pick, port.as_ref().map(|p| p as &dyn Shared<LoopVar>)));
            gate.1.slots.leave_loop();
            let (idle_s, span_s, n_slots) = gate.1.slots.idle();
            ctl.cycle_line(format!(
                "loop slots: run {run_no} ended its loop after {} cycles; so far {:.1} % of the {n_slots} compute slots' time was idle while a loop ran ({idle_s:.1} slot-s of {:.1} s; [optimize] table_lag = {})",
                rep.cycles,
                100.0 * idle_s / (n_slots as f64 * span_s).max(1e-9),
                span_s,
                opts.table_lag
            ));
            rep
        };
        super::prof::mark("loop-end");
        if yields.get() > 0 {
            warnings.push(format!(
                "TopoR loop: stable after cycle {}, it gave its compute slot to a waiting run {} time(s) (fewer engine threads than runs) and waited {:.1} s for one again",
                rep.stable_at.unwrap_or(0),
                yields.get(),
                yield_s.get()
            ));
        }
        // The TopoR loop step ends here (its wait, when the limit fell in it); the pick and the cut
        // necks belong to the drawing of the picked variant.
        ctl.stage(rnd, rnds, "picking the variant to draw");
        ctl.test_point("loop ended");
        if ctl.limit.passed() && rep.dropped && !stop_now() {
            ctl.limit.note_cycle_dropped(); // a cycle the limit cut: the report names it whatever the wait
        }
        if rep.stop == "time limit" {
            rep.stop = if stop_now() { "Cancel / OK" } else if ctl.over_limit() { "time limit" } else { "loop_time_share" }.into();
        }
        let pick = arch.pick(&obj).expect("the initial plan is always archived");
        // M2 (6.1 draw once): the runs' picks are judged by their predicted criteria; only the
        // `[drawing] candidates` best are drawn (after Cancel / OK the best one).
        ctl.idle(); // waiting at the gate computes nothing: the soft limit names the runs still computing (F6)
        // TW3: the rows of the call's four bests are drawn ([`DrawGate::assign`]); a draw-only
        // round draws its own row.
        let skip_draw = if let Some(d) = draw_row {
            sp.restore(&d.row.var);
            drew_row = Some(d.clone());
            None
        } else if gate.1.table.is_some() {
            let mut rows = gate.1.assign(gate.0, compat, arch.items[pick].crit, gate.2, stop_now());
            if !rows.is_empty() && std::env::var_os("TOPO_TW3_DRAWONLY").is_some() {
                // test hook (tests/topo_tw3_table.rs): every row in a draw-only round, so the
                // draw-only path is compared with the in-round drawing of the same row
                sp.restore(&arch.items[pick].payload.var);
                extra_rows = std::mem::take(&mut rows);
                Some("TOPO_TW3_DRAWONLY: its rows are drawn in draw-only rounds".to_string())
            } else if rows.is_empty() {
                sp.restore(&arch.items[pick].payload.var);
                Some(gate.1.why_not(stop_now()))
            } else {
                let first = rows.remove(0);
                sp.restore(&first.row.var);
                drew_row = Some(first);
                extra_rows = rows;
                None
            }
        } else {
            let why = gate.1.offer(gate.0, arch.items[pick].crit, gate.2, stop_now());
            sp.restore(&arch.items[pick].payload.var);
            why
        };
        // time-limit-fix r2 (user 2026-11-22: the limit is optimizing time, the drawing afterwards is
        // not counted): the gate opened, every run has ended its loop and offered its pick; the final
        // drawing (the picked variants, their via reduction and stitching) runs off the clock. The
        // first run here stops the clock (idempotent); a draw-only round finds it stopped.
        if opts.opt_loop == "topor" && opts.timeout_scope != "all" {
            ctl.limit.enter_drawing();
        }
        if super::prof::on() {
            // site-price-cost: the size of the packer's point_free memo (entries; key 32 B + value 8 B).
            let n = packer.point_free_memo_len();
            super::prof::note("point_free_memo", serde_json::json!({"entries": n, "payload_bytes": 40 * n}));
        }
        let esc_share = {
            let (t, p) = (&sp.tw5.throttle, &sp.tw5.p4_throttle);
            if t.relay_work > 0.0 { (t.esc_work + p.esc_work) / (t.esc_work + p.esc_work + t.relay_work) } else { 0.0 }
        };
        let esc_work = (sp.tw5.throttle.esc_work + sp.tw5.p4_throttle.esc_work, sp.tw5.throttle.relay_work, sp.st.wires.iter().filter(|w| w.alive).count());
        let relay_wires = sp.tw5.relay_wires;
        drop(sp);
        if std::env::var_os("TOPO_LOOPDBG").is_some() {
            let dirs = dirs_of(&st);
            let cm = if cuts_on { CapModel::new_cut(&st, &packer, &dirs, &lr.events, bottom, overflow_mm) } else { CapModel::new(&st, &packer, &dirs, &lr.events, bottom, overflow_mm) };
            eprintln!("LOOPDBG[{}] pick: violations {:?} nosite {} overfull {} unlayerable {} forced {}", opts.seed, cm.violations(), cm.nosite(), cm.overfull(), lr.unlayerable.len(), forced.borrow().len());
            for w in (0..st.wires.len()).filter(|&w| st.wires[w].alive && w < lr.events.len() && !lr.events[w].is_empty()) {
                eprintln!("LOOPDBG[{}] pick via {}", opts.seed, cm.nosite_debug(w, &lr.events[w]));
            }
            if let Some(c) = cm.cut.as_ref() {
                for r in c.report().iter().filter(|r| r.slack < 0.0) {
                    eprintln!("LOOPDBG[{}] pick over-full cut layer {} ({:.3},{:.3})-({:.3},{:.3}) cap {:.3} need {:.3} slack {:.3} wires {:?}", opts.seed, r.layer, r.p.x / MM, -r.p.y / MM, r.q.x / MM, -r.q.y / MM, r.cap / MM, r.need / MM, r.slack / MM, r.wires);
                }
            }
            for (e, l) in cm.overfull_list() {
                eprintln!("LOOPDBG[{}] pick over-full edge {e} layer {l}", opts.seed);
            }
        }
        if std::env::var_os("TOPO_P5DBG").is_some() {
            // Debug: the different-net crossings of the pick whose two stretches share one layer
            // there (by the layering's events, and by the pass layers with the planned vias).
            let dirs = dirs_of(&st);
            let cm = CapModel::new(&st, &packer, &dirs, &lr.events, bottom, overflow_mm);
            let pl = |w: WireId, k: usize| -> Vec<i32> {
                let ps = &st.wires[w].passes;
                [k.checked_sub(1), Some(k)].into_iter().flatten().filter_map(|i| ps.get(i)).map(|&p| cm.pass_layer[p]).collect()
            };
            for w in (0..st.wires.len()).filter(|&w| st.wires[w].alive) {
                for x in st.crossings(w).into_iter().filter(|x| x.other > w && st.wires[x.other].net != st.wires[w].net) {
                    let (a, b) = (st.layers_now[w].get(x.k).copied().unwrap_or(0), st.layers_now[x.other].get(x.other_k).copied().unwrap_or(0));
                    let (pa, pb) = (pl(w, x.k), pl(x.other, x.other_k));
                    let same_mask = a == b && a.count_ones() == 1;
                    let same_pass = pa.iter().any(|l| pb.contains(l));
                    if same_mask || same_pass {
                        eprintln!("P5DBG same-layer? wires {w} (net {}) k {} mask {a:b} passes {pa:?} / {} (net {}) k {} mask {b:b} passes {pb:?} at ({:.3},{:.3}) ev {:?} / {:?}", st.wires[w].net, x.k, x.other, st.wires[x.other].net, x.other_k, proj.tris[x.tri].iter().map(|&v| proj.pos[v].x).sum::<f64>() / 3.0 / MM, -proj.tris[x.tri].iter().map(|&v| proj.pos[v].y).sum::<f64>() / 3.0 / MM, lr.events.get(w), lr.events.get(x.other));
                    }
                }
            }
        }
        st.cross_vias = cross_vias0;
        st.layers_now.clear();
        st.free_wire_a = 1.0;
        st.mark_free_wires(&[]);
        st.site_rooms = None;
        st.site_k.clear();
        let loop_s = t_loop.elapsed().as_secs_f64();
        let time_hit = rep.stop == "time limit" || rep.stop == "loop_time_share";
        loop_end = Some(LoopEnd { stable: rep.stable_at.is_some(), stopped_stable: rep.stop == "stable", time_hit, relay_stop: rep.stop == "re-lay budget", share_stop: rep.stop == "loop_time_share", cycles: rep.cycles, at_s: ctl.limit.elapsed_s() });
        let pc = arch.items[pick].crit;
        let table = table_text(&arch.table(Some(pick)), &obj);
        // TW3: the restarts whose descent then gave the shared table a new row (before the next
        // restart of this run, or its end).
        let restarts_new_row = gate.1.table.as_ref().filter(|_| draw_row.is_none()).map_or(0, |t| {
            let cs: Vec<usize> = rep.restart_from.iter().map(|r| r.0).collect();
            (0..cs.len()).filter(|&i| t.accepted_in(gate.0, cs[i], cs.get(i + 1).copied().unwrap_or(usize::MAX)) > 0).count()
        });
        if rep.checks > 0 {
            warnings.push(format!(
                "TopoR loop restarts ([optimize] restart = \"{}\", every {} re-lays and at least {} cycles; stall after {} cycles without a gain; TW3, Polubasov 2010 «Многовариантная оптимизация»): {} checks, {} restarts ({} by a stall, {} from another run's row, {} led to a new table row), {} times the descent still gained and went on, {} times no other row to go to; {} re-lays",
                opts.restart,
                opts.restart_relays,
                opts.restart_min_cycles,
                opts.loop_restart,
                rep.checks,
                rep.restarts.len(),
                rep.stall_restarts,
                rep.restart_from.iter().filter(|r| r.1 != gate.0).count(),
                restarts_new_row,
                rep.kept_gaining,
                rep.no_row,
                rep.relays
            ));
        }
        warnings.push(format!(
            "TopoR loop ([optimize] loop = \"topor\"; Polubasov 2010): {} cycles in {loop_s:.1} s, ended by {}{}; {} restarts from the archive{}; {} variants offered, {} archived, first loop variant after {}; picked {} (gen {}): predicted {} violations, {} vias, {:.1} mm, {} shrinkages; reroutes kept {}/{}, via-rose laid again {}/{}, floating-up moves {}, crossing passes kept {}, whole-board moves undone {}, layer re-optimisation {} runs, {} taken ({:.0} % of the loop's time); every move priced with the archive's predicted violations at the violation weight ({:.0} mm each), crossings layer-aware ({:.2} of the crossing factor kept); per cycle: {}",
            rep.cycles,
            rep.stop,
            rep.stable_at.map(|s| format!(" (stable after cycle {s})")).unwrap_or_default(),
            rep.restarts.len(),
            if rep.restarts.is_empty() { String::new() } else { format!(" ({})", rep.restarts.iter().map(|(c, l)| format!("cycle {c} from {l}")).collect::<Vec<_>>().join(", ")) },
            rep.offered,
            rep.archived,
            rep.first_variant_s.map_or("-".to_string(), |s| format!("{s:.2} s")),
            arch.items[pick].label,
            arch.items[pick].payload.gen,
            pc.violations,
            pc.vias,
            pc.length_mm,
            pc.shrinkages,
            rep.stats.kept,
            rep.stats.tried,
            rep.stats.relaid_kept,
            rep.stats.relaid,
            rep.stats.float_kept,
            rep.stats.cross_kept,
            rep.stats.undone,
            rep.stats.layer_runs,
            rep.stats.layer_kept,
            100.0 * rep.stats.layer_s / loop_s.max(1e-9),
            viol_mm,
            opts.loop_crossing,
            rep.lines_text()
        ));
        warnings.push(format!("TopoR loop {table}"));
        {
            // loop-local: the deterministic work counters (docs/dev/design/loop-local.md).
            let st_ = &rep.stats;
            let per = |x: usize| x as f64 / rep.cycles.max(1) as f64;
            let mean = |x: usize| x as f64 / st_.layer_local.max(1) as f64;
            warnings.push(format!(
                "TopoR loop judge ([optimize] loop_judge = \"{}\"): per cycle {:.1} whole-board layerings, {:.1} whole-board measures, {:.1} local layerings (mean {:.1} wires laid again, {:.1} in the local capacity model; {} wires a move changed besides its own, laid out again with it){}{}",
                if judge_local { "local" } else { "global" },
                per(st_.layer_global),
                per(st_.measure_global),
                per(st_.layer_local),
                mean(st_.local_free),
                mean(st_.local_model),
                lctr.local_changed.get(),
                if local_check { format!("; TOPO_LOCALCHECK: {} partner faults, {} unpriced held-held crossings, {} stale held events", lctr.local_faults.get(), lctr.local_unpriced.get(), lctr.local_stale.get()) } else { String::new() },
                if st_.shadow_n > 0 {
                    format!("; shadow check ([optimize] loop_shadow = {}): {} moves also judged by the whole board (its change after - before, both layered whole), {} kept alike ({:.1} %), mean |local change - whole-board change| graded violations {:.4}, vias {:.2}, unlayerable crossings {:.2}; mean |whole-board layering - present layering| before the move, graded {:.4}", opts.loop_shadow, st_.shadow_n, st_.shadow_agree, 100.0 * st_.shadow_agree as f64 / st_.shadow_n as f64, st_.shadow_err_graded / st_.shadow_n as f64, st_.shadow_err_vias as f64 / st_.shadow_n as f64, st_.shadow_unl_diff as f64 / st_.shadow_n as f64, st_.shadow_base_graded / st_.shadow_n as f64)
                } else {
                    String::new()
                }
            ));
        }
        {
            // TW5 ESCAPE: what the escapes did (docs/dev/design/tw5-escape.md).
            let st_ = &rep.stats;
            let pct = |a: usize, b: usize| if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 };
            warnings.push(format!(
                "TopoR loop escapes (TW5): oscillation \"{}\" (amplitude {:.2}-{:.2}); layering escape \"{}\": {} escapes ({} archived variants over {} layerings), {:.1} % of the free positions changed layer per escape, vias {} -> {} over them, {} with more predicted violations (taken: {}); work share of the escapes {:.1} % (target {:.0} %, [optimize] escape_budget \"{}\"); floating up per wire {} of {} kept; loop history {} updates; pre-loop P4 rounds {}; in-loop P4 rounds {}; states after escapes / in-loop P4 rounds offered to the variant table {}",
                opts.oscillation,
                opts.osc_amp_min,
                opts.osc_amp_max,
                opts.escape,
                st_.escapes,
                rep.archived,
                st_.layer_runs,
                pct(st_.esc_diff, st_.esc_free),
                st_.esc_vias_before,
                st_.esc_vias_after,
                st_.esc_viol_up,
                opts.escape_take,
                100.0 * esc_share,
                100.0 * opts.escape_share.clamp(0.0, 0.5),
                if opts.escape_budget == "cost" { "cost" } else { "units" },
                st_.float_w_kept,
                st_.float_w_tried,
                st_.hist_updates,
                if opts.preloop_p4 { "on" } else { "folded into the loop" },
                if opts.loop_p4 {
                    format!("{} taken ({} wires ripped), {} undone (a wire found no path), {} undone (worse, escape_take \"not_worse\"), {} waited for the work budget", st_.p4_rounds, st_.p4_ripped, st_.p4_undone, st_.p4_worse, st_.p4_throttled)
                } else {
                    "off".to_string()
                },
                if opts.escape_offer {
                    format!("{} ({} taken into this run's archive, {} not offered: covered by its rows)", rep.mid_offered, rep.mid_archived, st_.mid_covered)
                } else {
                    "off".to_string()
                }
            ));
        }
        if time_hit && rep.stop == "loop_time_share" {
            warnings.push(format!("TopoR loop: its own time share ([optimize] loop_time_share = {share:.2} of the time left when it began) ended the loop at {:.1} s before its plan was stable; the time limit was not reached ({TIME_HIT})", ctl.limit.elapsed_s()));
        } else if time_hit {
            warnings.push(format!("TopoR loop: the time limit ended the loop before its plan was stable ({TIME_HIT})"));
        }
        loop_json = Some(serde_json::json!({
            "escape": {
                "oscillation": opts.oscillation,
                "mode": opts.escape,
                "take": opts.escape_take,
                "escapes": rep.stats.escapes,
                "free_positions": rep.stats.esc_free,
                "changed_positions": rep.stats.esc_diff,
                "vias_before": rep.stats.esc_vias_before,
                "vias_after": rep.stats.esc_vias_after,
                "viol_up": rep.stats.esc_viol_up,
                "walked": rep.stats.esc_walked,
                "relaid": [rep.stats.relaid_kept, rep.stats.relaid],
                "work_share": (esc_share * 1e4).round() / 1e4,
                "budget": if opts.escape_budget == "cost" { "cost" } else { "units" },
                "work_units": [(esc_work.0 * 10.0).round() / 10.0, (esc_work.1 * 10.0).round() / 10.0],
                "relay_wires": (relay_wires * 10.0).round() / 10.0,
                "layer_s": (rep.stats.layer_s * 1e3).round() / 1e3,
                "busy_s": (rep.stats.busy_s * 1e3).round() / 1e3,
                "alive": esc_work.2,
                "float_wire": [rep.stats.float_w_kept, rep.stats.float_w_tried],
                "history_updates": rep.stats.hist_updates,
                "preloop_p4": opts.preloop_p4,
                "loop_p4": {"on": opts.loop_p4, "rounds": rep.stats.p4_rounds, "ripped": rep.stats.p4_ripped, "undone": rep.stats.p4_undone, "worse": rep.stats.p4_worse, "throttled": rep.stats.p4_throttled},
                "escape_offer": {"on": opts.escape_offer, "offered": rep.mid_offered, "archived": rep.mid_archived, "covered": rep.stats.mid_covered},
            },
            "cycles": rep.cycles,
            "stop": rep.stop,
            "stable_at": rep.stable_at,
            "restarts": rep.restarts.len(),
            "restart": {"mode": opts.restart, "relays": opts.restart_relays, "min_cycles": opts.restart_min_cycles, "checks": rep.checks, "stall_restarts": rep.stall_restarts, "relays": rep.relays, "kept_gaining": rep.kept_gaining, "no_row": rep.no_row, "new_row": restarts_new_row, "from_other_run": rep.restart_from.iter().filter(|r| r.1 != gate.0).count()},
            "offered": rep.offered,
            "archived": rep.archived,
            "variants": arch.items.len(),
            "first_variant_s": rep.first_variant_s.map(|s| (s * 1e3).round() / 1e3),
            "loop_s": (loop_s * 1e3).round() / 1e3,
            "picked": arch.items[pick].label,
            "picked_gen": arch.items[pick].payload.gen,
            "predicted": {"violations": pc.violations, "vias": pc.vias, "length_mm": (pc.length_mm * 1e3).round() / 1e3, "shrinkages": pc.shrinkages},
            "initial": arch.items.iter().find(|v| v.payload.cycle == 0).map(|v| serde_json::json!({"violations": v.crit.violations, "vias": v.crit.vias, "length_mm": (v.crit.length_mm * 1e3).round() / 1e3})),
            "reroutes": [rep.stats.kept, rep.stats.tried],
            "via_rose": [rep.stats.relaid_kept, rep.stats.relaid],
            "floating_up": rep.stats.float_kept,
            "crossing_passes": rep.stats.cross_kept,
            "undone": rep.stats.undone,
            "layerings": [rep.stats.layer_kept, rep.stats.layer_runs],
            "crossing_share": opts.loop_crossing,
            "pair_move": [rep.stats.pair_moved_kept, rep.stats.pair_moved, rep.stats.pair_moved_wires, rep.stats.pair_moved_long],
            // thick-layer S3: per candidate type tries, kept, wall and process CPU (ms; never routing input).
            "pair_move_kinds": {
                "stretch": {"tried": rep.stats.pair_moved - rep.stats.one_side_tried, "kept": rep.stats.pair_moved_kept.saturating_sub(rep.stats.one_side_kept), "wall_ms": rep.stats.pair_ns as f64 / 1e6, "cpu_ms": rep.stats.pair_cpu_ns as f64 / 1e6},
                "one_side": {"tried": rep.stats.one_side_tried, "kept": rep.stats.one_side_kept, "left_changes": rep.stats.one_side_left, "wall_ms": rep.stats.one_side_ns as f64 / 1e6, "cpu_ms": rep.stats.one_side_cpu_ns as f64 / 1e6}
            },
            "site_move": [rep.stats.site_kept, rep.stats.site_moved],
            "branch_move": [rep.stats.branch_kept, rep.stats.branch_moved],
            "site_move_detour_refused": rep.stats.site_detour_refused,
            "variants_table": arch.items.iter().map(|v| serde_json::json!({"label": v.label, "gen": v.payload.gen, "cycle": v.payload.cycle, "errors": v.crit.errors, "penalty": (v.crit.penalty * 1e3).round() / 1e3, "vias": v.crit.vias, "length_mm": (v.crit.length_mm * 1e3).round() / 1e3, "shrinkages": v.crit.shrinkages})).collect::<Vec<_>>(),
            "layer_share_pct": (1000.0 * rep.stats.layer_s / loop_s.max(1e-9)).round() / 10.0,
            "judge": {
                "mode": if judge_local { "local" } else { "global" },
                "layer_global": rep.stats.layer_global,
                "measure_global": rep.stats.measure_global,
                "layer_local": rep.stats.layer_local,
                "local_free": rep.stats.local_free,
                "local_model": rep.stats.local_model,
                "shadow": [rep.stats.shadow_n, rep.stats.shadow_agree, rep.stats.shadow_err_graded, rep.stats.shadow_err_vias, rep.stats.shadow_unl_diff, rep.stats.shadow_base_graded],
            },
        }));
        // TW7 B: the pattern moves' entries, added apart (the block above is at serde_json's
        // macro recursion limit).
        if let Some(o) = loop_json.as_mut().and_then(|v| v.as_object_mut()) {
            let tot = rep.stats.pat_work + rep.stats.pat_base;
            o.insert("pattern_moves".into(), serde_json::json!([rep.stats.pat_kept, rep.stats.pat_moved, rep.stats.pat_found]));
            o.insert("pattern_kinds".into(), super::xrules::Kind::ALL.iter().map(|k| (k.name().to_string(), serde_json::json!(rep.stats.pat_kinds[k.idx()]))).collect::<serde_json::Map<_, _>>().into());
            o.insert(
                "pattern_share".into(),
                serde_json::json!({
                    "share": super::escape::Share::new(opts.pattern_share).share,
                    "stuck_w": if opts.pattern_stuck.is_finite() { opts.pattern_stuck.max(0.0) } else { 0.0 },
                    "retried": rep.stats.pat_retried,
                    "stuck_patterns": rep.stats.pat_stuck,
                    "waited": rep.stats.pat_waited,
                    "work": (rep.stats.pat_work * 1e3).round() / 1e3,
                    "other_work": (rep.stats.pat_base * 1e3).round() / 1e3,
                    "measured": if tot > 0.0 { (1e4 * rep.stats.pat_work / tot).round() / 1e4 } else { 0.0 },
                }),
            );
        }
        if !opts.pattern_moves.is_empty() {
            let stuck_w = if opts.pattern_stuck.is_finite() { opts.pattern_stuck.max(0.0) } else { 0.0 };
            warnings.push(pattern_moves_line(&opts.pattern_moves, super::escape::Share::new(opts.pattern_share).share, stuck_w, opts.pour_cuts, &rep.stats));
        }
        if opts.via_pair_min {
            warnings.push(format!(
                "TopoR loop, local via-pair minimisation ([vias] pair_min; Luzin, Popov & Popov 2013): {} via pair(s) lifted onto their neighbours' layer, {} of {} wires with a via pair laid again kept",
                rep.stats.pair_lifts, rep.stats.pair_relaid_kept, rep.stats.pair_relaid
            ));
        }
        if opts.via_pair_move {
            warnings.push(format!(
                "TopoR loop, local via-pair minimisation, obstructing wires ([vias] pair_move; Luzin, Popov & Popov 2013 step 8.1): {} of {} via pairs moved (the wires that cross their stretch on the layer round it laid again together, {} wires in all tries; kept when the board's evaluation dropped by more than the stretch's graded price, one via at a stretch of pair_max_mm, growing with the square of the length)",
                rep.stats.pair_moved_kept, rep.stats.pair_moved, rep.stats.pair_moved_wires
            ));
            if rep.stats.pair_moved_long > 0 {
                warnings.push(format!(
                    "TopoR loop, local via-pair minimisation: {} kept via-pair move(s) over a stretch longer than [vias] pair_max_mm = {} mm (priced, not refused: the crossing wires were laid again round a long stretch because the board gained more than its price; raise pair_max_mm to make long stretches cheaper, lower it to make them dearer)",
                    rep.stats.pair_moved_long, opts.via_pair_max_mm
                ));
            }
        }
        if opts.opt_site_move || opts.opt_branch_move {
            warnings.push(site_move_line(opts.opt_site_move, opts.opt_branch_move, opts.opt_site_move_detour.max(0.0), &rep.stats));
        }
        if let Some(r) = crep.as_mut() {
            r.stage("after the TopoR loop", &st, &lr.events, &packer, bottom, &|w| wire_conn.get(w).filter(|&&c| c != usize::MAX).map(|&c| conn_of[c]));
        }
        tlog("TopoR loop", t_loop);
        // TW7 A (`TOPO_PATDBG`, debug): per wire of the loop's pick with a projected detour over
        // 1.5, the patterns it takes part in (every detector, whatever `pattern_moves` says), so
        // each detour of a run can be attributed to a detector (docs/dev/design/tw7-moves.md A).
        if std::env::var_os("TOPO_PATDBG").is_some() {
            patdbg(model, &st, &lr.unlayerable_pairs, opts.seed);
        }
        // The one drawing of the loop's pick runs to its end (soft limit: nothing cuts it; hard:
        // the time limit); a run whose pick another run draws stops here, before any copper.
        if let Some(why) = skip_draw {
            // TW3-R5: the loop ran, only its pick is drawn elsewhere: its counters go on to the rows this run draws.
            let mut ro = RoundOut { extra: extra_rows, ..RoundOut::not_drawn(model, opts, Some(why), None, loop_end) };
            ro.result.warnings.extend(warnings.iter().filter(|w| is_loop_counts(w)).cloned());
            return (ro, Vec::new(), Memory::default());
        }
        draw_through.set(true);
    }
    let lr = lr;
    let t_draw = Instant::now();
    // pause-button: the drawing's start on the run's clock (the drawing bound, `drawing_grace_s`)
    let t_draw_c = ctl.limit.now();
    // par-draw phase A: the drawing's order-free units run on `[drawing] draw_lanes` lanes (results
    // folded in unit order: the copper does not depend on the lanes), until this round returns.
    let _par = super::par::set_mode(Some(super::refine_par::lanes::mode_of(opts.draw_lanes)), opts.draw_test_reverse);
    // One path for `hard` and the overrun cap (user 2026-11-13): the limit acted as hard before
    // the drawing began, so it cut a planning step; the kept plan is drawn in full.
    draw_exempt.set(ctl.limit.is_hard() && ctl.over_limit());
    cut_step.set(CutStep::Drawing);
    // 4.3 `initial = "once"`: the plan handed to the next cycle (before anything is dropped).
    let plan_carry = once.then(|| PlanCarry { topo: st.topology(), wire_conn: wire_conn.clone(), conns: conns.clone(), term_over: term_over.clone() });
    if once {
        let f = forced.borrow();
        if !f.is_empty() {
            warnings.push(format!(
                "initial routing (once): {} connection(s) had no legal topological path and were placed anyway through other copper, priced as violations so the optimiser works them away: {}",
                f.len(),
                f.iter().map(|&ci| { let net = dec.connections[ci].net; model.nets.iter().find(|n| n.code == net).map_or(net.to_string(), |n| n.name.clone()) }).collect::<Vec<_>>().join(", ")
            ));
        }
        warnings.push(if carried {
            format!("initial routing (once): this cycle started from the plan of the cycle before; {} connection(s) placed again", p2_replaced)
        } else {
            format!("initial routing (once): {} connections laid one at a time, double crossings removed after every wire", conns.len())
        });
    }
    // A different-net crossing still on one layer is an error: the lower-tier wire stays unrouted.
    let wlen: Vec<f64> = (0..st.wires.len()).map(|w| if !st.wires[w].alive { 0.0 } else { st.wire_len(w) }).collect();
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
            if once {
                // 4.3: a crossing on one layer the optimiser could not remove is the user's to
                // see, loudly, with the layer it lies on.
                let p = lr.unlayerable_at.get(pi).copied();
                let layer = p.and_then(|p| {
                    lr.stretches.get(wa)?.iter().map(|(l, pts)| (pts.iter().map(|q| (*q - p).len()).fold(f64::INFINITY, f64::min), *l)).min_by(|a, b| a.0.total_cmp(&b.0)).map(|x| x.1)
                });
                let lname = layer.and_then(|l| b.signal_layers.get(l.max(0) as usize)).and_then(|&id| model.layers.iter().find(|x| x.id == id)).map_or("one layer".to_string(), |x| x.name.clone());
                warnings.push(format!("CRITICAL: traces {} and {} cross on layer {lname}{at}: the optimiser could not separate them; both are drawn and the crossing is a violation (a short) to fix by hand", name(wa), name(wb)));
            }
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
    // Q14b: the plan handed to the drawing, its traversals through foreign copper (each a
    // violation in the loop's measure and the P4 pick; a short if the drawing cannot avoid it).
    let fc_plan = super::optimise::foreign_copper(&st, &super::optimise::layers_now(&st, &lr.events, bottom));
    warnings.push(format!(
        "foreign copper (Q14b): the plan handed to the drawing runs {} times through copper of another net or a no-tracks rule area (graded {:.2}; each a violation in the plan's measure, a short if the drawing cannot avoid it)",
        fc_plan.0, fc_plan.1
    ));
    if let Ok(path) = std::env::var("TOPO_PLANDUMP") {
        // Debug: the plan handed to the drawing, per wire ([`super::plandump::wire_json`]): net,
        // connection, taut points, the layering's stretches, the passes (edge ends, room per
        // layer, need) and crossings, before the unlayerable drops.
        let lnow = super::optimise::layers_now(&st, &lr.events, bottom);
        let wires: Vec<serde_json::Value> = (0..st.wires.len())
            .filter(|&w| st.wires[w].alive)
            .map(|w| super::plandump::wire_json(&st, &lr, &lnow, w, wire_conn.get(w).copied().filter(|&c| c != usize::MAX).map(|c| conn_of[c])))
            .collect();
        let _ = std::fs::write(format!("{path}.s{}.r{round}", opts.seed), serde_json::json!({"wires": wires, "foreign": fc_plan.0}).to_string());
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
    // 2.4 (cut model): the necks the plan priced at the cut level (the cuts that fit only
    // necked) become the plan's pass widths, laid out by the neck rule (`State::cut_neck_widths`),
    // so the drawing necks there (the plan's neck zones, at the widest width its checker accepts)
    // and only there, and the variant's shrinkages count them. The carried plan (`once`) keeps
    // its own widths: the next cycle prices its cuts again.
    if cuts_on {
        let dirs = dirs_of(&st);
        let cm = CapModel::new_cut(&st, &packer, &dirs, &lr.events, bottom, overflow_mm);
        let (n_cut, amount, price) = cm.shrinkage();
        let necks = st.cut_neck_widths(&cm.cut_necks());
        let wires: BTreeSet<WireId> = necks.keys().map(|&p| st.passes[p].wire).collect();
        st.apply_neck_widths(&necks);
        warnings.push(format!(
            "cut shrinkages (2.4): {n_cut} cuts fit only with their traces necked below the netclass width (graded {amount:.2} pitches, {price:.1} mm at [factors] shrinkage); {} passes of {} connections necked for them in the plan",
            necks.len(),
            wires.len()
        ));
    }
    ctl.update("route", 2, 0, nconn, lr.vias, t0.elapsed().as_secs_f64());
    if ctl.is_discarded() {
        // limit-p3p4: the dialog's Cancel: the job is abandoned, its plan is never drawn.
        return (RoundOut::not_drawn(model, opts, None, Some(DISCARDED.to_string()), None), Vec::new(), Memory::default());
    }
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
    let mut dr = Drawer::new(model, opts, b, &st, &checker);
    for cp in &pairs.coupled {
        let r = &cp.rule;
        let (d, _) = diffpair::via_of(model_real, r, opts);
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
    if let Some(r) = crep.as_mut() {
        r.final_plan(&st, &lr.events, &cm_plan.via_k, &packer, bottom, &|w| wire_conn.get(w).filter(|&&c| c != usize::MAX).map(|&c| conn_of[c]), &cm_plan.overfull_list());
        if let Some(e) = r.write() {
            eprintln!("{e}");
        }
    }
    // C0: the plan handed to the drawing, as one machine-readable line (every cycle), and
    // `[run] stop_after = "plan"`: the round ends here, before any copper is drawn.
    // M1 compass: the same plan's predicted items per violation class (structured, measurement
    // only), carried to this round's result below.
    let mut compass_plan: Option<super::compass::Compass>;
    let mut plan_dump: Option<String> = None;
    {
        let alive: Vec<WireId> = (0..st.wires.len()).filter(|&w| st.wires[w].alive).collect();
        let crossings: usize = alive.iter().map(|&w| st.crossings(w).iter().filter(|c| st.wires[c.other].net != st.wires[w].net).count()).sum::<usize>() / 2;
        // Polyline through the evenly spaced pass points (comparable across `[search] length`);
        // `taut_length_mm`: the clearance-keeping shortest paths through the sleeves (4.3).
        let topo_len: f64 = alive.iter().map(|&w| st.polyline(w).windows(2).map(|x| (x[1] - x[0]).len()).sum::<f64>()).sum();
        let taut_len: f64 = alive.iter().map(|&w| st.taut_points(w).windows(2).map(|x| (x[1] - x[0]).len()).sum::<f64>()).sum();
        let vias: usize = alive.iter().map(|&w| cm_plan.via_k[w].len()).sum();
        let m = serde_json::json!({
            "round": round,
            "wires": alive.len(),
            "crossings": crossings,
            "vias": vias,
            "vias_without_site": cm_plan.nosite(),
            "overfull_edge_layers": cm_plan.overfull_list().len(),
            "unlayerable_crossings": unlayerable_errors,
            "dropped_unlayerable": dropped.len(),
            "topo_length_mm": (topo_len / MM * 1e3).round() / 1e3,
            "taut_length_mm": (taut_len / MM * 1e3).round() / 1e3,
        });
        let mut m = m;
        if let Some(lj) = loop_json.take() {
            m["loop"] = lj;
        }
        // `[capacity] model = "cuts"`: the per-layer cuts of the plan (the authority there).
        let cmc_plan = packer.cuts.is_some().then(|| CapModel::new_cut(&st, &packer, &dirs_plan, &lr.events, bottom, 1.0));
        if let Some(cmc) = cmc_plan.as_ref() {
            if let Some(c) = cmc.cut.as_ref() {
                let rows = c.report();
                let over: Vec<&super::cutmodel::CutRow> = rows.iter().filter(|r| r.slack < -super::cutmodel::OVER_TOL).collect();
                m["overfull_cuts"] = serde_json::json!(over.len());
                m["cut_overflow_mm"] = serde_json::json!((over.iter().map(|r| -r.slack).sum::<f64>() / MM * 1e4).round() / 1e4);
                // 2.4: the cuts that fit only necked (over-full counts only beyond the floors).
                let (sn, sa, _) = c.shrinkage();
                m["shrink_cuts"] = serde_json::json!(sn);
                m["cut_shrink_pitches"] = serde_json::json!((sa * 1e3).round() / 1e3);
                if let Ok(path) = std::env::var("TOPO_CUTS") {
                    let nm = |x: f64| (x / MM * 1e4).round() / 1e4;
                    let js: Vec<serde_json::Value> = rows
                        .iter()
                        .map(|r| {
                            let nets: Vec<i32> = r.wires.iter().map(|&w| st.wires[w].net).collect();
                            serde_json::json!({"layer": r.layer, "p": [nm(r.p.x), nm(-r.p.y)], "q": [nm(r.q.x), nm(-r.q.y)], "cap": nm(r.cap), "need": nm(r.need), "need_floor": nm(r.need_floor), "slack": nm(r.slack), "shrink": nm(r.shrink), "nets": nets})
                        })
                        .collect();
                    let _ = std::fs::write(format!("{path}.r{round}.json"), serde_json::to_string(&js).unwrap_or_default());
                }
            }
            if let Some(ctx) = packer.cuts.as_ref() {
                let (b, h) = *super::cutmodel::lockp(&ctx.builds);
                m["cut_sheets"] = serde_json::json!({"builds": b, "hits": h, "problems": super::cutmodel::lockp(&ctx.problems).len()});
                for e in super::cutmodel::lockp(&ctx.problems).iter().take(3) {
                    warnings.push(format!("capacity model \"cuts\": a sheet build failed, that plan was priced per edge: {e}"));
                }
            }
        }
        // Cost per move (shopping list 3.1 measurement; process-wide sums so far).
        let (mv_n, mv_us, vp_n, vp_us) = super::refine::meter::read();
        m["move_cost"] = serde_json::json!({"moves": mv_n, "us_per_move": (mv_us * 10.0).round() / 10.0, "via_positions": vp_n, "us_per_via_position": (vp_us * 10.0).round() / 10.0});
        let (vc_n, vc_ms, bd_n, bd_ms) = super::refine::meter::read_cm1();
        m["move_cost"]["via_cut"] = serde_json::json!({"calls": vc_n, "ms_in": vc_ms.round(), "builds": bd_n, "ms_builds": bd_ms.round()});
        let (pg_n, pl_n, p_ms) = super::pourplan::meter::read();
        m["move_cost"]["pour_cut"] = serde_json::json!({"global": pg_n, "local": pl_n, "ms_in": p_ms.round()});
        // par-refine: the lane executor's epochs, lane-epochs, candidates evaluated past a kept one
        // (wasted) and again for an end-site fill (redone); the build memo's hits and misses.
        {
            use std::sync::atomic::Ordering::Relaxed;
            use super::refine_par::stats as ls;
            m["move_cost"]["lanes"] = serde_json::json!({"epochs": ls::EPOCHS.load(Relaxed), "lane_epochs": ls::LANE_EPOCHS.load(Relaxed), "wasted": ls::WASTED.load(Relaxed), "redone": ls::REDONE.load(Relaxed)});
            m["move_cost"]["build_memo"] = serde_json::json!({"hits": super::refine::meter::MEMO_HIT.load(Relaxed), "misses": super::refine::meter::MEMO_MISS.load(Relaxed)});
            // the plan's refinement: candidates, stream and memo fingerprint (equal for every lane count)
            let (ev, sh, mh) = lr.refine_stream;
            m["move_cost"]["refine_stream"] = serde_json::json!({"evals": ev, "stream": format!("{sh:016x}"), "memo": format!("{mh:016x}")});
        }
        let (fl, st_ms, np, p_ms, nc, i_ms) = super::refine::meter::read_split();
        m["cut_split"] = serde_json::json!({"flushes": fl, "ms_stretches": st_ms.round(), "parts": np, "ms_parts": p_ms.round(), "cuts_priced": nc, "ms_install": i_ms.round()});
        warnings.push(format!("plan metrics: {m}"));
        // M1 compass, after the plan metrics (its read-only cut test must not enter their meters).
        {
            let lname = |l: usize| b.signal_layers.get(l).and_then(|&id| model.layers.iter().find(|x| x.id == id)).map_or(format!("layer {l}"), |x| x.name.clone());
            let wc = |w: WireId| wire_conn.get(w).filter(|&&c| c != usize::MAX).map(|&c| conn_of[c]);
            let fset = forced.borrow().clone();
            // `[capacity] compass_cut_check` (model = "edges"): the read-only cut test runs after the
            // drawing (end of this round), outside the time limit.
            // `pad_exits`: with 5.10's stubs on, the planned stubs and the needed exits no legal stub
            // realises (compass::exit_stub_items); off: no pad-exit plan object (not predicted).
            let mut px = opts.pad_exit_stubs.then(|| super::compass::exit_stub_items(b, &st, &wc, &lname));
            // tw4-r2 (1) `[capacity] pad_exits`: the plan's own exits with an over-full term.
            if packer.pad_exits {
                px.get_or_insert_with(Vec::new).extend(super::compass::plan_exit_items(&cm_plan.exit_items(), &wc, &lname));
            }
            let view = super::compass::PlanView { st: &st, lr: &lr, cm_edge: &cm_plan, cm_cut: cmc_plan.as_ref(), cut_is_plan: cmc_plan.is_some(), bottom, conn: &wc, layer: &lname, forced: &fset, pad_exits: px.as_deref() };
            let mut cp = super::refine::meter::suspended(|| super::compass::plan_compass(&view, if cmc_plan.is_some() { "cuts" } else { "edges" }, &opts.opt_loop, round));
            cp.loop_predicted = m.get("loop").and_then(|l| l.get("predicted")).and_then(|p| p.get("violations")).and_then(|v| v.as_u64()).map(|v| v as usize);
            // thick-layer F4 (`[output] plan_report`): the plan of this row as the drawing gets
            // it, carried with the round's result (the result handed back is a drawn row's, so it
            // carries that row's plan). Measurement only; its time (well under a second on qmtech)
            // is router time: moving the limit by it (`RunControl::measured`) would make the
            // result's `limit.limit_s` differ between two equal runs.
            if opts.plan_report {
                let names: Vec<String> = (0..b.n_signal).map(lname).collect();
                let net_name = |code: i32| model.nets.iter().find(|n| n.code == code).map_or(String::new(), |n| n.name.clone());
                let row = super::plandump::Row {
                    label: drew_row.as_ref().map(|a| a.label.as_str()),
                    roles: drew_row.as_ref().map_or_else(Vec::new, |a| a.roles.iter().map(|r| r.name()).collect()),
                    round,
                    seed: opts.seed,
                    opt_loop: &opts.opt_loop,
                };
                plan_dump = Some(super::refine::meter::suspended(|| super::plandump::plan_json(&st, &lr, bottom, &cm_plan, Some(&cp), &wc, &net_name, &names, fc_plan.0, &row)).to_string());
            }
            compass_plan = Some(cp);
        }
        if opts.stop_after == "plan" {
            let mut r = unrouted(model_real, opts, "not drawn: [run] stop_after = \"plan\" ends each run after the plan of its first cycle");
            r.warnings.extend(warnings);
            r.compass = compass_plan.take();
            r.plan = plan_dump.take();
            return (RoundOut::unrouted(r), Vec::new(), mem.clone());
        }
    }
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
            // 5.4 `[vias] between_crossings`: the drawing searches the traversals strictly
            // between the events (the plan's ranges), never a crossing's own triangle.
            if opts.via_between && cm_plan.via_iv[w].len() == p.vias.len() {
                p.vias = cm_plan.via_iv[w].clone();
            }
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
    // 5.4 `[vias] relocate`: every via of the plan at the force balance of its free interval
    // (equal clearance between its neighbours; [`super::viaopt::relocate_vias`]); the drawing
    // takes that site first. The interval is sampled on the plan's taut path, which is the
    // wire's path only when the search priced the length taut (`[search] length = "taut"`):
    // with "even" the corridor can run round obstacles the drawing then crosses on the other
    // layer (measured 5.4: via_pair_slide, the corridor round the wall ends; relocated vias
    // pulled the drawn wire 0.55 mm longer), so relocation is skipped there, with a warning.
    if opts.via_relocate && opts.search_length != "taut" {
        warnings.push(format!(
            "via relocation ([vias] relocate) skipped: it needs [search] length = \"taut\" (the plan's path); [search] length = \"{}\" plans corridors that are no wire path",
            opts.search_length
        ));
    }
    if opts.via_relocate && opts.search_length == "taut" {
        let t_vr = Instant::now();
        let outline = super::board::outline_segments(model);
        // 5.4 task 1: the via's room is the real rule ([`crate::vias::via_reach`],
        // [`crate::vias::via_via_need`]: copper, hole clearance, hole to hole), plus the margin.
        let first = |net: i32| crate::vias::via_sizes(&model.rules, net, opts).first().copied();
        let via_r = |net: i32| first(net).map_or(0.0, |s| crate::vias::via_reach(&model.rules, net, s));
        let margin = opts.margin_nm as f64;
        let clear = |net: i32| model.rules.net_rule(net).clearance.max(model.rules.min_clearance) as f64 + margin;
        let via_via = |a: i32, b: i32| match (first(a), first(b)) {
            (Some(sa), Some(sb)) => crate::vias::via_via_need(&model.rules, a, sa, b, sb, margin),
            _ => 0.0,
        };
        let cx = super::viaopt::ViaCtx { st: &st, outline: &outline, edge_clear: proj.edge_clear + margin, via_r: &via_r, clear: &clear, via_via: &via_via, pair_room: opts.via_pair_room, grid: super::gridsnap::via_grid(opts), site_slack: if packer.site_planned { packer.site_slack } else { 0.0 }, window_taut: packer.site_window_taut };
        let (mut sites, rr) = super::viaopt::relocate_vias(&cx, &mut plans, &lr.events);
        // grid-snap r2: a via moved onto the grid must not change the plan's pour cut-offs (the
        // pour term the plan priced, [`super::pourplan`]): with the grid moves the plan copper
        // cuts off no more than without them, else the moves of the vias on the wires that bound a
        // new cut-off piece go back, and if it still cuts off more, every grid move goes back.
        if !rr.grid_moves.is_empty() {
            let pb = super::pourplan::PourBase::new(model, opts.pour_cut_cells);
            if pb.any {
                let tracks: Vec<Item> = super::pourplan::plan_items(&st, &packer, b, &lr.events, bottom).into_iter().filter(|it| matches!(it, Item::Track(_))).collect();
                let via = |w: WireId, p: Vec2| first(st.wires[w].net).map(|sz| super::draw::via_item(st.wires[w].net, p, sz, Some(w)));
                let back = super::pourplan::undo_grid_cuts(model, &tracks, &mut sites, &rr.grid_moves, &via, &pb);
                if back > 0 {
                    warnings.push(format!("grid snap: {back} of {} via grid moves undone: they cut off more pour than the sites without them", rr.grid_moves.len()));
                }
            }
        }
        dr.via_site = sites;
        dr.via_site_tight = rr.tight.iter().copied().collect();
        dr.site_slack = cx.site_slack;
        warnings.push(format!(
            "via relocation ([vias] relocate; Dayan & Dai 1991, force balance): {} vias, {} moved in their free interval to the most clearance, smallest gap {:+.3} mm in total{}",
            rr.vias,
            rr.moved,
            rr.gap_gain / MM,
            if rr.grid_moves.is_empty() { String::new() } else { format!(", {} onto the via grid ([drawing] grid_snap)", rr.grid_moves.len()) }
        ));
        if std::env::var("TOPO_VIAREL").is_ok() {
            for &(w, i, a, b, g0, g1) in &rr.moves {
                eprintln!("VIAREL wire {w} via {i} ({:.3},{:.3}) -> ({:.3},{:.3}) gap {:.3} -> {:.3} mm", a.x / MM, -a.y / MM, b.x / MM, -b.y / MM, g0 / MM, g1 / MM);
            }
        }
        tlog("via relocation", t_vr);
    }
    // 5.4 `[search] branch_relocate`: the junctions of the net trees to the Fermat point of their
    // arms, out of a pad (< 120 degrees there) or into it (>= 120 degrees;
    // [`super::viaopt::relocate_junctions`]). The drawing splits the wire through the point and
    // ends the other wire there.
    let mut jpos: BTreeMap<usize, Vec2> = BTreeMap::new();
    // The wires whose junction the plan moved: the fuse after the drawing leaves them alone (the
    // plan decided where they meet).
    let mut jmoved: BTreeSet<WireId> = BTreeSet::new();
    // The point is computed on the plan's taut paths: with `[search] length = "even"` those are
    // corridors, no wire path (the same caveat as the via relocation): skipped, with a warning.
    if opts.branch_relocate && opts.search_length != "taut" {
        warnings.push(format!(
            "branch point relocation ([search] branch_relocate) skipped: it needs [search] length = \"taut\" (the plan's path); [search] length = \"{}\" plans corridors that are no wire path",
            opts.search_length
        ));
    }
    if opts.branch_relocate && opts.search_length == "taut" {
        let t_j = Instant::now();
        let clear = |net: i32| model.rules.net_rule(net).clearance.max(model.rules.min_clearance) as f64;
        let jc = super::viaopt::JCtx { st: &st, clear: &clear, plans: &plans, octi: dr.octi.on };
        let mv = super::viaopt::relocate_junctions(&jc);
        let mut by: BTreeMap<&str, (usize, f64)> = BTreeMap::new();
        for m in &mv {
            let e = by.entry(m.kind).or_default();
            e.0 += 1;
            e.1 += m.gain;
            match m.kind {
                "free" => {
                    jpos.insert(m.vertex, m.at);
                }
                "pad_out" => {
                    dr.end_at.insert(m.branch, m.at);
                }
                _ => {
                    jpos.insert(m.vertex, m.at);
                }
            }
            if let Some((tw, k)) = m.trunk {
                dr.split_at.entry(tw).or_default().push((k, m.at));
                jmoved.insert(tw);
            }
            jmoved.insert(m.branch.0);
            if std::env::var("TOPO_JREL").is_ok() {
                eprintln!("JREL {} trunk {:?} branch {:?} ({:.4},{:.4}) -> ({:.4},{:.4}) t {:.3} gain {:.4} mm", m.kind, m.trunk, m.branch, m.from.x / MM, -m.from.y / MM, m.at.x / MM, -m.at.y / MM, m.t, m.gain / MM);
            }
        }
        warnings.push(format!(
            "branch points ([search] branch_relocate; Popov & Popov 2013, Dayan & Dai 1991): {} moved ({}), {:.3} mm shorter in the plan; {} of them stopped part way by the clearance of the arms",
            mv.len(),
            by.iter().map(|(k, (n, g))| format!("{k} {n}: {:.3} mm", g / MM)).collect::<Vec<_>>().join(", "),
            mv.iter().map(|m| m.gain).sum::<f64>() / MM,
            mv.iter().filter(|m| m.t < 1.0 - 1e-9).count()
        ));
        tlog("branch relocation", t_j);
    }
    // Draw order: widest first (a pair by its whole band, `prio_w`), then shortest.
    let wire_pw = |w: WireId| wire_conn.get(w).and_then(|&ci| prio_w.get(ci)).copied().unwrap_or(st.wires[w].w);
    let mut draw_order: Vec<WireId> = plans.keys().copied().collect();
    draw_order.sort_by(|&x, &y| {
        wire_pw(y).total_cmp(&wire_pw(x)).then(wire_len(x).total_cmp(&wire_len(y))).then(x.cmp(&y))
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
        let pv = jpos.get(&v).copied().unwrap_or(proj.pos[v]);
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
    dr.plan_layers = vec![Vec::new(); st.wires.len()];
    for (&w, pl) in &plans {
        set_layers(pl, None, &st.wires[w].passes, &mut dr.prov_layer);
        dr.plan_layers[w] = pl.layers.clone();
    }
    // Pack with the planned vias (their discs take room, as in P3/P4), so the vias can be put
    // where the packing left them room: on their wire's nominal chord.
    for (&w, pl) in &plans {
        let d = packer.via_d(st.wires[w].net);
        let pts: Vec<super::refine::ViaPt> = (0..pl.at.len()).map(|i| dr.via_site.get(&(w, i)).copied()).collect();
        dr.set_wire_layers(w, pl, &pl.at, &vec![d; pl.at.len()], &pts);
    }
    dr.compute_intervals();
    // 5.4 task 2: the vias the plan hands to the drawing (the plan vs drawn summary below).
    let plan_vias: usize = plans.values().map(|p| p.vias.len()).sum();
    // Phase 1: the vias of every wire (they are obstacles for the wires drawn next).
    let mut placed: BTreeMap<WireId, Vec<(usize, Vec2, crate::vias::ViaSize)>> = BTreeMap::new();
    let mut via_ids: BTreeMap<WireId, Vec<usize>> = BTreeMap::new();
    let mut live: Vec<WireId> = Vec::new();
    let mut via_forced: Vec<(WireId, String)> = Vec::new();
    for &w in &draw_order {
        if expired() {
            timed_out = true;
            fail(&mut out, conn_of[wire_conn[w]], draw_cut_reason());
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
    dr.pass_disc = vec![[0.0; 3]; st.passes.len()];
    for &w in &live {
        let (pl, vs) = (&plans[&w], &placed[&w]);
        let ks: Vec<usize> = vs.iter().map(|v| v.0).collect();
        // The disc as its reach (5.4 task 1: the drill's hole clearance where it reaches further).
        let ds: Vec<f64> = vs.iter().map(|v| 2.0 * crate::vias::via_reach(&model.rules, st.wires[w].net, v.2)).collect();
        let pts: Vec<super::refine::ViaPt> = vs.iter().map(|v| Some((v.0, v.1))).collect();
        dr.set_wire_layers(w, pl, &ks, &ds, &pts);
    }
    dr.compute_intervals();
    // edge-clear: a pad end whose centre breaks a static clearance (a track wider than its pad
    // beside the board edge) ends at the nearest legal point inside the pad, for ERBS and drawing
    // alike (`padend`; static only, so independent of the draw order).
    let pad_ends: Vec<(WireId, [usize; 2])> = live.iter().filter_map(|&w| plans.get(&w).filter(|p| !p.layers.is_empty()).map(|p| (w, [p.layers[0], p.layers[p.layers.len() - 1]]))).collect();
    dr.set_pad_ends(&pad_ends);
    // M3 `[drawing] realise = "erbs"`: every run's extended rubber band from the plan with the
    // placed vias (independent of the draw order), the wires in the precedes order.
    let mut erbs_line: Option<String> = None;
    // Stretches whose sleeve did not reach the end (wire, reason), for the erbs_fallback line.
    let mut erbs_failed: Vec<(usize, String)> = Vec::new();
    let mut erbs_err: Option<String> = None;
    if opts.realise == "erbs" {
        // ERBS is an optional pass of the drawing (a stretch it does not reach is drawn the
        // sequential way, never dropped): with the soft limit its bound is the drawing bound
        // (`[run] drawing_grace_s`, lead 2026-11-17), as hard the limit itself.
        let erbs_soft = !ctl.limit.is_hard();
        // (The round's 1 s floor is the hard cut's and the wall-clock cap's; the soft bound counts from the drawing
        // itself.) ERBS reads the limit live at every stretch (`ErbsOpts::limit`): the overrun cap (wall clock from the
        // floor on, on both clocks; lead 2026-11-17) and, soft, the drawing bound from `t_draw` (a
        // limit the CPU clock reaches during ERBS moves it), and says which stopped it first (`Realised::stopped`).
        let pts: BTreeMap<WireId, Vec<(usize, Vec2)>> = placed.iter().map(|(&w, v)| (w, v.iter().map(|x| (x.0, x.1)).collect())).collect();
        let layers: Vec<usize> = (0..b.n_signal).collect();
        let conn = |w: WireId| wire_conn.get(w).filter(|&&c| c != usize::MAX).map(|&c| conn_of[c]);
        let mo = super::cutflow::MeasureOpts::from_options(opts);
        let stops = super::erbs::ErbsOpts {
            deadline: if erbs_soft { None } else { ctl.deadline().map(|d| d.max(floor)) },
            stop: (!draw_through.get()).then(|| ctl.cancel.clone()),
            limit: Some(ctl.limit.clone()),
            draw_start: erbs_soft.then_some(t_draw_c),
            floor: Some(floor),
            fail_first: ctl.test_point("fail erbs sleeve"),
            octi_order: super::erbs::OctiOrder::parse(&opts.octi_order).unwrap_or_else(|| {
                warnings.push(format!("[drawing] octi_order = {:?} is not \"sweep\" or \"bends\": \"sweep\" (the default) used", opts.octi_order));
                super::erbs::OctiOrder::Sweep
            }),
            band_passes: opts.band_passes,
            ..super::erbs::ErbsOpts::default()
        };
        match super::erbs::realise(&st, &lr.events, &packer, bottom, &conn, &pts, &dr.pad_end, &super::board::outline_segments(model), &mo, &layers, &live, stops) {
            Ok(r) => {
                use super::erbs::ErbsStop;
                // `[run] drawing_grace_s` (soft limit): the stretches the drawing bound cut, for the report: only when
                // the bound stopped ERBS first (not the cap, not a Cancel).
                if r.timed_out > 0 && r.stopped == Some(ErbsStop::Bound) {
                    ctl.limit.note_draw_grace_erbs(r.timed_out);
                }
                if r.timed_out > 0 {
                    let by = match r.stopped {
                        Some(ErbsStop::Cancel) => "the run was cancelled",
                        Some(ErbsStop::Bound) => "[run] drawing_grace_s passed past the time limit",
                        _ if ctl.limit.hard_mode() => "the time limit passed ([run] timeout_mode = hard)",
                        _ => "the overrun cap passed ([run] timeout_overrun_s, or the plugin's CPU-clock cap; the soft limit then acts as the hard one)",
                    };
                    warnings.push(format!("erbs_timeout: {by} before {} stretch(es) had their ERBS; those runs are drawn the sequential way", r.timed_out));
                }
                erbs_failed = r.failed.clone();
                erbs_line = Some(format!(
                    "ERBS drawing ([drawing] realise = \"erbs\"; Dayan 1997 pp. 15-16, shortest homotopic path by the funnel over the sleeve): {} stretches in {:.2} s ({} past the time limit, {} without a sleeve: drawn sequentially), length over the zero-width bands {:.3}, {} obstacle points, {} portals, {} side conflicts; checked: {} predicted violations (over-full cuts, planned crossings, features the band runs through; features at a pad exit only at the exit), {} model gaps (erbs_gap, worst {:.3} mm); {} stretches without a band",
                    r.report.stretches,
                    r.seconds,
                    r.timed_out,
                    r.failed.len(),
                    if r.report.band_len > 0.0 { r.report.len / r.report.band_len } else { 1.0 },
                    r.report.points,
                    r.report.portals,
                    r.report.conflicts,
                    r.report.predicted,
                    r.report.erbs_gap,
                    r.report.worst_gap / 1e6,
                    r.unbanded
                ));
                // draw-absorb A3 (`[drawing] band_passes`): the iterated band realise.
                if opts.band_passes > 0 {
                    let p = &r.passes;
                    if let Some(l) = erbs_line.as_mut() {
                        l.push_str(&format!("; band passes ([drawing] band_passes = {}): {} pass(es), {} yielding bands realised again, {} kept, model gaps between bands {} -> {}", opts.band_passes, p.passes, p.tried, p.moved, p.gaps_before, p.gaps_after));
                    }
                }
                dr.joint.band_pass = r.passes;
                live = r.order.clone();
                // Test probe (fd45 r10): the ERBS draw order by net name, for a test whose
                // precondition is an order (topo_m3_class_keep); nothing is formatted without a hook.
                if ctl.has_test_hook() {
                    let names: Vec<String> = live.iter().map(|&w| model.nets.iter().find(|n| n.code == st.wires[w].net).map_or_else(|| st.wires[w].net.to_string(), |n| n.name.clone())).collect();
                    ctl.test_point(&format!("erbs draw order {}", names.join(",")));
                }
                dr.erbs = r.runs;
                dr.test_no_sequential = ctl.test_point("no sequential run");
                dr.test_no_class_keep = ctl.test_point("erbs class keep off");
                dr.test_straight_seq = ctl.test_point("straight sequential run");
                dr.test_fail_via_wire = ctl.test_point("fail via wire");
                dr.test_joint_conflict = ctl.test_point("joint conflict");
                if ctl.test_point("wind erbs") {
                    // test-only fault: every ERBS runs its path three times (a wound band of the
                    // same copper, legal and three times its band), for the length factor
                    for rb in dr.erbs.values_mut().flatten() {
                        let back: Vec<Vec2> = rb.path.iter().rev().skip(1).copied().collect();
                        let fwd: Vec<Vec2> = rb.path.iter().skip(1).copied().collect();
                        rb.path.extend(back);
                        rb.path.extend(fwd);
                    }
                }
            }
            Err(e) => {
                erbs_line = Some(format!("ERBS drawing ([drawing] realise = \"erbs\") not available, the sequential drawing instead: {e}"));
                erbs_err = Some(e);
            }
        }
        // TW2: every band committed and checked at once, a conflict keeps an independent set
        // (`super::joint`); the kept bands are reserved for their wires, so the order below only
        // says who draws first among the rest.
        if !dr.erbs.is_empty() {
            let mode = super::erbs::KeepMode::parse(&opts.erbs_conflict_keep).unwrap_or_else(|| {
                warnings.push(format!("[drawing] erbs_conflict_keep = {:?} is not \"tier\" or \"count\": \"tier\" used", opts.erbs_conflict_keep));
                super::erbs::KeepMode::Tier
            });
            let rank: BTreeMap<WireId, usize> = draw_order.iter().enumerate().map(|(i, &w)| (w, i)).collect();
            let key = |w: WireId| (wire_conn.get(w).filter(|&&c| c != usize::MAX).map_or(u8::MAX, |&c| tier_of(conn_of[c])), rank.get(&w).copied().unwrap_or(usize::MAX));
            let skip: BTreeSet<(WireId, usize)> = tree_js.iter().filter_map(|&(bw, end, ..)| plans.get(&bw).map(|p| (bw, if end == 0 { 0 } else { p.layers.len().saturating_sub(1) }))).collect();
            if ctl.test_point("reverse draw order") {
                // test-only: the precedes order backwards (the kept bands must not change)
                live.reverse();
            }
            dr.reserve_bands(&live, &plans, &placed, &skip, &key, mode);
            // draw-via-neck C r5: the reserved end runs' tip anchors, before any run is drawn.
            dr.tip_anchor_reserved(&plans);
            // fd45 r10 (mechanism C), debug arm `TOPO_FD45_ON=loserfirst`, off by default: the
            // wires the joint check rejected (no reserved copper) draw first, against the kept
            // bands only. It fixes tw2 swap_vias under the sweep order (X2: drawn after its kept
            // neighbours, whose octilinear redraws left their bands and took its room), but on
            // esp it added 15 DRC items (IO10_SDA/USB_D-/ENABLE, IO8_SCL/+3V3): the lead's rule
            // (keep it only if no board gets worse) reverted it. Shopping list TW2-SWEEP.
            if super::octi::fd45_on("loserfirst") {
                let losers: BTreeSet<WireId> = dr.joint.class.keys().map(|&(o, _)| o).collect();
                let (mut a, b): (Vec<WireId>, Vec<WireId>) = live.iter().partition(|w| losers.contains(w));
                a.extend(b);
                live = a;
            }
        }
    }
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
            dr.release_bands(w);
            fail(&mut out, conn_of[wire_conn[w]], draw_cut_reason());
            continue;
        }
        let ci = conn_of[wire_conn[w]];
        let mut ids = via_ids.remove(&w).unwrap_or_default();
        if ctl.test_point("fail wire drawing") {
            // test-only fault: this wire's drawing fails (no feedback diagnosis), so never drop meets it
            dr.release_bands(w);
            for id in ids {
                dr.uncommit(id);
            }
            fail(&mut out, ci, "drawing: injected by the test hook");
            continue;
        }
        let vh = ids[..placed[&w].len().min(ids.len())].to_vec();
        if let Some((key, q)) = joint_of(w, &drawn, &plans) {
            dr.end_at.insert(key, q);
        }
        let mut wo = dr.draw_wire(w, &plans[&w], &placed[&w], &mut ids);
        if !wo.ok && dr.split_at.remove(&w).is_some() {
            // 5.4: the moved junction did not draw; the wire as before.
            wo = dr.draw_wire(w, &plans[&w], &placed[&w], &mut ids);
        }
        if wo.ok {
            via_h.insert(w, vh);
            drawn_ok += 1;
            settle(octi_on, &mut out, &mut necking, ci, w, &wo, b, &st, model.rules.default.width as f64);
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
    // TW2: every reserved band was adopted or released with its wire; one left over would be copper
    // no wire owns (counted, invariant 0).
    let left: BTreeSet<WireId> = dr.joint.reserved.keys().map(|k| k.0).collect();
    dr.joint.left_reserved = dr.joint.reserved.len();
    for w in left {
        dr.release_bands(w);
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
    // M6 (6b, user 2026-11-18): only the whole-route cycles redraw blockers (`[optimize] loop =
    // "cycles"`). With the TopoR loop the picked plan is drawn once (M2 draw once, Polubasov 2010:
    // the drawing realises the plan); `drawing_feedback = true` there is ignored with a warning.
    let (mut fb_tried, mut fb_rescued) = (0usize, 0usize);
    let feedback = opts.drawing_feedback && opts.opt_loop == "cycles";
    if opts.drawing_feedback && !feedback {
        warnings.push(FEEDBACK_IGNORED.to_string());
    }
    if feedback {
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
                    settle(octi_on, &mut out, &mut necking, cb, bw, &wb, b, &st, model.rules.default.width as f64);
                    drawn.insert(bw, wb);
                }
                out[cf].reason.clear();
                via_h.insert(f, ids[..vias_f.len().min(ids.len())].to_vec());
                placed.insert(f, vias_f);
                settle(octi_on, &mut out, &mut necking, cf, f, &wf, b, &st, model.rules.default.width as f64);
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
        ctl.test_point("never drop");
        for &w in &live {
            if drawn.contains_key(&w) {
                continue;
            }
            let ci = conn_of[wire_conn[w]];
            if expired() {
                // the cut skips it: the reason says so (Cancel's or the limit's) and keeps why its own
                // drawing failed (user 2026-11-14: report both)
                if out[ci].reason.starts_with("drawing:") {
                    out[ci].reason = format!("{} (its own drawing had failed: {})", draw_cut_reason(), out[ci].reason);
                }
                continue;
            }
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
            settle(octi_on, &mut out, &mut necking, ci, w, &wo, b, &st, model.rules.default.width as f64);
            drawn.insert(w, wo);
        }
        dr.force = false;
    }
    // TW2: how the drawing drew each run (`realise = "erbs"`): the ERBS line and result.json route.erbs.
    let mut erbs_runs = (opts.realise == "erbs" && erbs_err.is_none()).then(|| super::joint::tally(drawn.values(), &dr.joint, &opts.erbs_conflict_keep));
    if let Some(rep) = erbs_runs.as_mut() {
        // Only the runs of wires that were drawn; their connection by name.
        rep.no_band_list.retain(|x| drawn.contains_key(&x.wire));
        for x in &mut rep.no_band_list {
            let c = &dec.connections[conn_of[wire_conn[x.wire]]];
            x.net = model.nets.iter().find(|n| n.code == c.net).map_or(c.net.to_string(), |n| n.name.clone());
            (x.from, x.to) = (c.from.label.clone(), c.to.label.clone());
        }
    }
    debug_assert!(erbs_runs.as_ref().is_none_or(|r| r.lost_to_other == 0), "TW2 I1: a reserved band was taken out by another wire");
    // ---- Junctions (D5 net trees): a branch whose trunk was not drawn, or that reaches its
    // junction on another layer than the trunk passes it, is not connected: its copper goes and
    // its connection fails (locally; the rest of the tree stays).
    let mut junction_join_vias = 0usize;
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
                                let mut vs = dr.viols_of(&add, &rep);
                                let base = dr.run_src(bw, "junction_join", "join", "", "", 0, false, false, &add);
                                dr.tag_viols(&mut vs, &base);
                                let mut wo = drawn.remove(&bw).expect("drawn branch");
                                for it in &add {
                                    let h = dr.commit(it);
                                    match it {
                                        Item::Via(v) => {
                                            junction_join_vias += 1;
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
                                settle(octi_on, &mut out, &mut necking, ci, bw, &wo, b, &st, model.rules.default.width as f64);
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
    if ctl.is_discarded() {
        // limit-p3p4: the dialog's Cancel came while the traces were drawn: no via reduction,
        // polish or stitching for a job that is abandoned.
        return (RoundOut::not_drawn(model, opts, None, Some(DISCARDED.to_string()), None), Vec::new(), Memory::default());
    }
    if super::par::meter::on() {
        eprintln!("[topo round {round}] {}", super::par::meter::report());
    }
    // The nets' drawn wires and terminals, and their copper pieces (the split guard of the
    // straightening, the fuse and the polish, [`super::netconn`]).
    let mut net_wires: BTreeMap<i32, Vec<WireId>> = BTreeMap::new();
    for &w in drawn.keys() {
        net_wires.entry(st.wires[w].net).or_default().push(w);
    }
    let mut net_terms: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for (o, ob) in proj.obstacles.iter().enumerate() {
        if let Some(n) = ob.net.filter(|&n| n != 0 && !ob.bare) {
            net_terms.entry(n).or_default().push(o);
        }
    }
    // The pieces of net `net` with the drawn wires `drawn` (`extra`: one wire taken out of it).
    let pieces_of = |net: i32, drawn: &BTreeMap<WireId, WireOut>, extra: Option<(WireId, &WireOut)>| -> super::netconn::Conn {
        let ws = net_wires.get(&net).map_or(&[][..], |v| v.as_slice());
        let pcs: Vec<super::netconn::Piece> = ws
            .iter()
            .filter_map(|&w| {
                let wo = match extra {
                    Some((x, o)) if x == w => o,
                    _ => drawn.get(&w)?,
                };
                Some(super::netconn::Piece { runs: &wo.runs, vias: wo.vias.iter().map(|&(p, sz)| (p, sz.d as f64 / 2.0)).collect(), half_w: st.wires[w].w / 2.0 })
            })
            .collect();
        let ts: Vec<super::netconn::Term> = net_terms
            .get(&net)
            .map_or(&[][..], |v| v.as_slice())
            .iter()
            .map(|&o| {
                let ob = &proj.obstacles[o];
                super::netconn::Term { poly: &ob.poly, layers: ob.layers, center: ob.center }
            })
            .collect();
        super::netconn::conn(&pcs, &ts)
    };
    // first-draw-45 P4 step 2: the straightening pass (Staepelaere 1992, StraightenLayer, ch. 5
    // p. 15), part of the drawing (so the variant as drawn has it): the wires in the draw (sweep)
    // order, each one's jogs redrawn where the neighbours drawn and straightened before it leave
    // room, its middle legs beside them (each segment pushed onto the transformed ones, PDF pp.
    // 16, 33). Keep points: the other wires' ends and vias of the net on it ([`super::netconn::feet_on`]);
    // the split guard as for the fuse and the polish.
    free_snap("draw", &drawn, &st);
    if octi_on && opts.octi_straighten && !super::octi::fd45_off("p4s") {
        let t_str = Instant::now();
        let (mut n_str, mut str_nm, mut str_refused) = (0usize, 0.0, 0usize);
        let mut str_pieces: BTreeMap<i32, super::netconn::Conn> = BTreeMap::new();
        let order: Vec<WireId> = live.iter().copied().filter(|w| drawn.contains_key(w)).collect();
        // Passes over the order until one changes nothing (each step lowers the wire's price, so
        // it ends; at most STRAIGHTEN_PASSES, a work bound): a wire straightened before its
        // neighbours gets another turn once they have moved.
        let mut pass_changed = true;
        let mut passes = 0usize;
        while pass_changed && passes < STRAIGHTEN_PASSES {
            pass_changed = false;
            passes += 1;
            for &w in &order {
                let mut wo = drawn.remove(&w).expect("drawn wire");
                let (len0, net) = (wo.length_nm, st.wires[w].net);
                let mut joined = |pre: &WireOut, now: &WireOut| -> bool {
                    let before = str_pieces.entry(net).or_insert_with(|| pieces_of(net, &drawn, Some((w, pre)))).clone();
                    let after = pieces_of(net, &drawn, Some((w, now)));
                    if super::netconn::worse(&before, &after) {
                        str_refused += 1;
                        return false;
                    }
                    str_pieces.insert(net, after);
                    true
                };
                let keep: Vec<Vec2> = {
                    fn piece(o: &WireOut, half_w: f64) -> super::netconn::Piece<'_> {
                        super::netconn::Piece { runs: &o.runs, vias: o.vias.iter().map(|&(p, sz)| (p, sz.d as f64 / 2.0)).collect(), half_w }
                    }
                    let others: Vec<super::netconn::Piece> = net_wires.get(&net).map_or(&[][..], |v| v.as_slice()).iter().filter(|&&x| x != w).filter_map(|&x| drawn.get(&x).map(|o| piece(o, st.wires[x].w / 2.0))).collect();
                    super::netconn::feet_on(&piece(&wo, st.wires[w].w / 2.0), &others)
                };
                let k = dr.octi_straighten(&mut wo, &keep, &mut joined);
                if k > 0 {
                    pass_changed = true;
                    n_str += k;
                    str_nm += wo.length_nm - len0;
                    let ci = conn_of[wire_conn[w]];
                    necking.retain(|(c, _)| *c != ci);
                    settle(octi_on, &mut out, &mut necking, ci, w, &wo, b, &st, model.rules.default.width as f64);
                }
                drawn.insert(w, wo);
            }
        }
        if n_str > 0 || str_refused > 0 {
            warnings.push(straighten_line(n_str, str_nm / MM, str_refused, passes, !pass_changed, t_str.elapsed().as_secs_f64()));
        }
    }
    free_snap("straighten", &drawn, &st);
    ctl.stage(rnd, rnds, "via reduction and fuse");
    let t_red = Instant::now();
    // The result as drawn is a variant of its own (more vias, maybe shorter).
    let mut pre_snap: Option<Snapshot> = None;
    // TOPO_NOREDUCE=1 (debug): leave the vias as drawn, to compare with the reduction.
    let mut vias_reduced = 0usize;
    if opts.via_reduce && std::env::var("TOPO_NOREDUCE").is_err() {
        pre_snap = Some((out.clone(), necking.clone()));
        // M2.TL soft limit: the via reduction belongs to the one drawing of the picked variant and
        // runs to its end (Cancel still ends it, anytime; verify-m2 F2: past the soft limit it got
        // 0 s, "0 relabels tried"). A limit acting as hard (`hard`, or past the overrun cap, read
        // live) cuts it only when it passed after the drawing began (`draw_exempt`).
        cut_step.set(CutStep::ViaReduction);
        // `[run] drawing_grace_s` (soft limit): the via reduction is an optional pass (anytime: every
        // relabel accepted so far stays); past the drawing bound it stops, every wire stays drawn.
        ctl.test_point("via reduction begin");
        let grace_stop = std::cell::Cell::new(false);
        let draw_grace_over = || {
            let over = !ctl.is_cancelled() && ctl.limit.draw_grace_over(t_draw_c);
            if over {
                grace_stop.set(true);
            }
            over
        };
        let (rs, changed) = super::reduce::reduce_vias(&mut dr, &mut drawn, &mut placed, &mut plans, &mut via_h, &end_fixed, &frozen, b.n_signal, None, &|| ctl.is_cancelled() || limit_cut() || draw_grace_over());
        // Cancel / OK ends the via reduction (cancel-fix; anytime: every relabel accepted so far
        // stays; measured on qmtech it took 12-22 s for 0-10 of ~110 vias). In the TopoR loop pick's
        // one drawing that keeps the result complete; only the time limit makes it a cut-short one.
        vias_reduced = rs.vias_removed;
        // Soft limit (before its overrun cap): the limit stops nothing, so a stopped reduction is a Cancel's.
        let red_limit_cut = rs.timed_out && limit_cut();
        let red_grace_cut = rs.timed_out && !red_limit_cut && grace_stop.get() && !ctl.is_cancelled();
        let cancel_cut = rs.timed_out && !red_limit_cut && !red_grace_cut;
        timed_out |= red_limit_cut || cancel_cut && !draw_through.get();
        for w in changed {
            let ci = conn_of[wire_conn[w]];
            necking.retain(|(c, _)| *c != ci);
            settle(octi_on, &mut out, &mut necking, ci, w, &drawn[&w], b, &st, model.rules.default.width as f64);
        }
        warnings.push(format!(
            "via reduction after drawing: {} wires with vias, {} relabels tried in {} sweeps, {} accepted, {} vias removed ({:.1} s{}); before: {}; after: {}",
            rs.wires_with_vias,
            rs.tried,
            rs.sweeps,
            rs.accepted,
            rs.vias_removed,
            t_red.elapsed().as_secs_f64(),
            if red_limit_cut { ", stopped by the time budget" } else if red_grace_cut { ", stopped by [run] drawing_grace_s (every wire drawn)" } else if cancel_cut { ", stopped by Cancel / OK" } else { "" },
            causes_before,
            causes(&plans, &drawn)
        ));
    } else {
        warnings.push(format!("via reduction after drawing: off; {causes_before}"));
    }
    free_snap("reduce", &drawn, &st);
    // ---- Fuse (D6b): a branch that leaves its junction (or a pad) beside its trunk leaves the
    // trunk's copper where the two part (TopoR FreeStyle auto move of T-points, black-box E8c-e;
    // overlap removal, REPORT section 4.C). Geometric, after everything is drawn: the checker sees
    // all copper. A wire at a pad end fuses only onto a wire not fused there itself (no cycle).
    // (price gain, length gain mm) of every fuse applied, for the report line.
    let mut fused: Vec<(f64, f64)> = Vec::new();
    // Points of a wire's copper that other copper ends on (a planned branch point, a fused foot): fixed
    // vertices for the polish below (fuse-octi r4: a polish that redrew the segment under a foot orphaned
    // the branch, qmtech JS2_9).
    let mut anchors: BTreeMap<WireId, Vec<Vec2>> = BTreeMap::new();
    // Fuse pour check ([`super::fusepour`], after the octilinear polish: the variant table judges
    // the copper the steps after the fuse leave): each applied move with the state it replaced
    // (drawn wire, outcome, necked stretches of its connection), in the order made.
    let pour_check = opts.fuse_pour_check && opts.stitch_islands && model.pours.iter().any(|z| !z.fills.is_empty());
    type Saved = (WireOut, Outcome, Vec<(usize, Event)>);
    let mut moves: Vec<(super::fusepour::Move, Saved)> = Vec::new();
    // The polish's report numbers (segments, length added nm), and per polish move (index in
    // `moves`) its own: the line is written after the check, without the undone moves (V5).
    let mut polish: Option<(usize, f64)> = None;
    let mut polish_of: BTreeMap<usize, (usize, f64)> = BTreeMap::new();
    // fuse-octi r4 (H1 part 2): the split guard of the fuse and the polish ([`super::netconn`]): a
    // step that splits a net's drawn copper and pads that were joined is not taken (partitions, r6) (qmtech JS2_9 r3 and
    // main's JS1_2: a branch fused onto a free-angle trunk segment that the polish then redrew as an
    // elbow without its foot, a dangling branch for KiCad Cleanup). `pieces`: the partition per net as
    // the copper is now (made at the net's first step), `pieces0`: before the fuse pass.
    let mut pieces: BTreeMap<i32, super::netconn::Conn> = BTreeMap::new();
    let mut pieces0: BTreeMap<i32, super::netconn::Conn> = BTreeMap::new();
    // Steps not taken by the guard: (fuse, polish).
    let mut split_refused = (0usize, 0usize);
    let fdbg = std::env::var("TOPO_FUSEDBG").is_ok();
    // Test hook "split guard off": the guard lets every step through (its self-check below then
    // names the net a dropped foot split).
    let guard_off = ctl.test_point("split guard off");
    // Test hook "fuse skips no anchors": the fuse's cut-part skip (bookkeeping anchors and copper
    // KiCad joins to it) is off, so only the split guard keeps the net together at the fuse.
    let no_anchor_skip = ctl.test_point("fuse skips no anchors");
    // Test hook "fuse drops anchors": only the bookkeeping anchors go; the copper found on the
    // geometry still keeps the fuse off its cut part.
    let drop_anchor_skip = ctl.test_point("fuse drops anchors");
    // Debug env TOPOR_FUSE_NO_FEET (ladder base arm, = the fuse before fuse-guard r1: bookkeeping
    // anchors only): the geometric cut-part skip is off. Changes copper: never in a default arm.
    let no_feet = std::env::var_os("TOPOR_FUSE_NO_FEET").is_some();
    if tree_free && opts.fuse_branches {
        let jw = reg.price(&Event::JunctionAngle { at: Vec2::default(), deg: 60.0, ws_mm: 1.0 / super::factors::junction_psi(60.0) }, &WireCtx::default());
        let js = tree::junctions(&st);
        let mut fused_at: BTreeSet<(WireId, usize)> = BTreeSet::new();
        // The candidates a take did not keep (refit refused, split guard): (wire, end, foot, i).
        let mut refused: BTreeSet<(WireId, usize, i64, i64, usize)> = BTreeSet::new();
        // A fuse may not take away the stretch an anchor lies on.
        for &(bw, end, tw, _, v) in &js {
            anchors.entry(tw).or_default().push(dr.end_at.get(&(bw, end)).copied().unwrap_or(proj.pos[v]));
        }
        let near_poly = |p: Vec2, pts: &[Vec2]| pts.windows(2).any(|x| {
            let ab = x[1] - x[0];
            let u = ((p - x[0]).dot(ab) / ab.len2().max(1.0)).clamp(0.0, 1.0);
            (x[0] + ab * u - p).len() < 1_000.0
        });
        let ids: Vec<WireId> = drawn.keys().copied().collect();
        let pad_nm = opts.junction_pad_mm * MM;
        // octi-bundle r2: the links and prices of the octilinear first draw (A1c).
        let ocfg = dr.octi;
        // fuse-octi r4 (M1): the ends go best LEGAL gain first, lazily ([`draw::lazy_next`]): two wires
        // that meet at one pad can fuse only one of them (`fused_at`), and the wire order let the first
        // one's small legal gain (qmtech JS1_2 wire 383, 2.7 mm) take the pair before the other's large
        // one (wire 480, 43 mm: the octilinear link that replaced a free-angle leg); r3's key (the best
        // gain of any candidate, legal or not, on the copper before the pass) went stale after the
        // first fuses (wiccon 3.3v +3.5 mm). Each key is the end's best legal candidate on the copper
        // as it is when the key is made; the keys of the net of the last fuse are made again before
        // the next take (the other nets' stay: a take finds a candidate another net's fuse blocked).
        // The end's trunks: a branch end its planned trunk, a pad end every wire of the net at the same
        // pad whose own end there is not fused (no cycle).
        let trunks_of = |w: WireId, end: usize, fused_at: &BTreeSet<(WireId, usize)>| -> Vec<WireId> {
            let wi = &st.wires[w];
            let (pad, vert) = if end == 0 { (wi.pa, wi.a) } else { (wi.pb, wi.b) };
            if pad == NO_OBS {
                js.iter().filter(|j| j.0 == w && j.1 == end).map(|j| j.2).collect()
            } else {
                ids.iter()
                    .copied()
                    .filter(|&u| u != w && !jmoved.contains(&u) && st.wires[u].net == wi.net && ((st.wires[u].a == vert && !fused_at.contains(&(u, 0))) || (st.wires[u].b == vert && !fused_at.contains(&(u, 1)))))
                    .collect()
            }
        };
        // The best legal fuse of an end on the copper as it is: (price gain, length gain mm, trunk,
        // run index, i, new run as drawn, foot, the cut part). The best over its trunks (the first
        // trunk on a tie).
        type FuseCand = (f64, f64, WireId, usize, usize, Vec<Vec2>, Vec2, Vec<Vec2>);
        let best_of = |w: WireId, end: usize, drawn: &BTreeMap<WireId, WireOut>, dr: &Drawer<'_>, anchors: &BTreeMap<WireId, Vec<Vec2>>, fused_at: &BTreeSet<(WireId, usize)>, refused: &BTreeSet<(WireId, usize, i64, i64, usize)>| -> Option<FuseCand> {
            let wi = &st.wires[w];
            let trunks = trunks_of(w, end, fused_at);
            let wo = drawn.get(&w)?;
            if wo.runs.is_empty() || trunks.is_empty() {
                return None;
            }
            let r = if end == 0 { 0 } else { wo.runs.len() - 1 };
            let (layer, mut run) = wo.runs[r].clone();
            if end == 1 {
                run.reverse();
            }
            let ws_mm = (wi.w + wi.s) / MM;
            let mut best: Option<FuseCand> = None;
            for u in trunks {
                let Some(uo) = drawn.get(&u) else { continue };
                let tp: Vec<Vec<Vec2>> = uo.runs.iter().filter(|(l, _)| *l == layer).map(|(_, p)| p.clone()).collect();
                let tr: Vec<&[Vec2]> = tp.iter().map(|p| p.as_slice()).collect();
                // fuse-guard r1: the other copper of the net as it is drawn, the pads of the net (the
                // wire's own end pad is on the trunk, `trunks_of`, so `keep` exempts it): what KiCad joins to the cut part (`netconn::cut_holds`).
                let nbrs: Vec<super::netconn::Piece> = net_wires
                    .get(&wi.net)
                    .map_or(&[][..], |v| v.as_slice())
                    .iter()
                    .filter(|&&x| x != w)
                    .filter_map(|&x| drawn.get(&x).map(|o| super::netconn::Piece { runs: &o.runs, vias: o.vias.iter().map(|&(p, sz)| (p, sz.d as f64 / 2.0)).collect(), half_w: st.wires[x].w / 2.0 }))
                    .collect();
                let tterms: Vec<super::netconn::Term> = net_terms
                    .get(&wi.net)
                    .map_or(&[][..], |v| v.as_slice())
                    .iter()
                    .map(|&o| {
                        let ob = &proj.obstacles[o];
                        super::netconn::Term { poly: &ob.poly, layers: ob.layers, center: ob.center }
                    })
                    .collect();
                let trunk_piece = super::netconn::Piece { runs: &uo.runs, vias: uo.vias.iter().map(|&(p, sz)| (p, sz.d as f64 / 2.0)).collect(), half_w: st.wires[u].w / 2.0 };
                let mut skip = |gain: f64, i: usize, pts: &[Vec2]| -> bool {
                    let q = pts[0];
                    if !no_anchor_skip && !no_feet {
                        let cut_runs = [(layer, run[..=i].to_vec())];
                        let new_runs = [(layer, pts.to_vec())];
                        let cut = super::netconn::Piece { runs: &cut_runs, vias: Vec::new(), half_w: wi.w / 2.0 };
                        let new = super::netconn::Piece { runs: &new_runs, vias: Vec::new(), half_w: wi.w / 2.0 };
                        let keep = [super::netconn::Piece { runs: trunk_piece.runs, vias: trunk_piece.vias.clone(), half_w: trunk_piece.half_w }, new];
                        if super::netconn::cut_holds(&cut, &nbrs, &keep, &tterms) {
                            if fdbg {
                                eprintln!("FUSE skip wire {w} end {end} onto {u} i {i} gain {gain:.2}: copper joined to the cut part (KiCad overlap)");
                            }
                            return true;
                        }
                    }
                    if !no_anchor_skip && !drop_anchor_skip && anchors.get(&w).is_some_and(|a| a.iter().any(|&p| (p - run[i]).len() > 1_000.0 && near_poly(p, &run[..=i]))) {
                        if fdbg {
                            eprintln!("FUSE skip wire {w} end {end} onto {u} i {i} gain {gain:.2}: an anchor on the cut part");
                        }
                        return true;
                    }
                    // `junction_pad_mm`: no T that close to a pad of the net.
                    if pad_nm > 0.0 && pad_verts.get(&wi.net).is_some_and(|ps| ps.iter().any(|&(v, _)| (proj.pos[v] - q).len() < pad_nm)) {
                        if fdbg {
                            eprintln!("FUSE skip wire {w} end {end} onto {u} i {i} gain {gain:.2}: junction_pad_mm at ({:.3}, {:.3})", q.x / MM, -q.y / MM);
                        }
                        return true;
                    }
                    refused.contains(&(w, end, q.x.round() as i64, q.y.round() as i64, i))
                };
                // pad-centre-price (`[factors] pad_off_centre`): the pad-end price of a run (from
                // the fusing end): its T point inside a pad of the net off its centre, and its far
                // end leg when KiCad's Cleanup deletes it inside the wire's own pad (the run then
                // ends at the leg's far joint, off the centre), graded, capped below one violation.
                let pitch = wi.w + wi.s;
                let cap = 0.8 * ocfg.viol_nm;
                let far = 1 - end;
                let n_runs = wo.runs.len();
                let pad_off = |pts: &[Vec2]| -> f64 {
                    let mut nm = 0.0;
                    if let Some(&q) = pts.first() {
                        if let Some(o) = net_terms.get(&wi.net).and_then(|v| v.iter().map(|&o| &proj.obstacles[o]).find(|ob| ob.layers.has(layer) && ob.poly.len() >= 3 && super::state::point_in_poly(q, &ob.poly))) {
                            nm += (ocfg.pad_off_nm * super::octi::pad_off_amount((q - o.center).len(), pitch)).min(cap);
                        }
                    }
                    if pts.len() >= 2 {
                        let (c, j) = (pts[pts.len() - 1], pts[pts.len() - 2]);
                        let at_end = if far == 0 { r == 0 } else { r + 1 == n_runs };
                        if let Some((poly, c0, ext)) = dr.own_pad_end(w, far, at_end, false, c) {
                            let ob = if far == 0 { wi.pa } else { wi.pb };
                            nm += super::octi::end_stub_price(poly, c0, ext, proj.obstacles[ob].center, wi.w, pitch, j, ocfg.stub_nm(pitch), ocfg.pad_off_nm, cap, model.rules.max_error as f64);
                        }
                    }
                    nm / MM
                };
                let end_mm: draw::EndPrice = (ocfg.pad_off_nm > 0.0 && !super::octi::fd45_off("padoff")).then_some(&pad_off);
                if let Some((gain, len_gain, i, pts)) = dr.fuse_best_legal(wo, r, &run, end == 1, &tr, ws_mm, jw, draw::fuse_style(octi_on, &ocfg, wi.w + wi.s), end_mm, &mut skip) {
                    if fdbg {
                        eprintln!("FUSE trunk wire {w} end {end} onto {u}: best legal gain {gain:.2} i {i}");
                    }
                    if best.as_ref().is_none_or(|b| gain > b.0) {
                        let q = if end == 1 { pts[pts.len() - 1] } else { pts[0] };
                        best = Some((gain, len_gain, u, r, i, pts, q, run[..=i].to_vec()));
                    }
                }
            }
            best
        };
        let mut pool: Vec<draw::FuseKey> = Vec::new();
        let mut cands: Vec<Option<FuseCand>> = Vec::new();
        for &w in &ids {
            if pairs.width.contains_key(&st.wires[w].net) || jmoved.contains(&w) {
                continue;
            }
            for end in 0..2 {
                let c = best_of(w, end, &drawn, &dr, &anchors, &fused_at, &refused);
                if fdbg {
                    if let Some(c) = &c {
                        eprintln!("FUSE key wire {w} net {} end {end}: best legal gain {:.2} mm onto {} at ({:.3}, {:.3})", st.wires[w].net, c.0, c.2, c.6.x / MM, -c.6.y / MM);
                    }
                }
                pool.push(draw::FuseKey { key: c.as_ref().map_or(f64::NEG_INFINITY, |c| c.0), stamp: 0, w, end, net: st.wires[w].net });
                cands.push(c);
            }
        }
        let mut version = 0usize;
        loop {
            let k = match draw::lazy_next(&pool, version) {
                draw::LazyStep::Done => break,
                draw::LazyStep::Reeval(k) => {
                    let (w, end) = (pool[k].w, pool[k].end);
                    let c = best_of(w, end, &drawn, &dr, &anchors, &fused_at, &refused);
                    pool[k].key = c.as_ref().map_or(f64::NEG_INFINITY, |c| c.0);
                    pool[k].stamp = version;
                    cands[k] = c;
                    continue;
                }
                draw::LazyStep::Take(k) => k,
            };
            let (w, end, net) = (pool[k].w, pool[k].end, pool[k].net);
            let Some((gain, len_gain, u, r, i, pts, q, cut)) = cands[k].take() else {
                pool[k].stamp = usize::MAX;
                continue;
            };
            let saved: Option<Saved> = pour_check.then(|| {
                let ci = conn_of[wire_conn[w]];
                (drawn[&w].clone(), out[ci].clone(), necking.iter().filter(|(c, _)| *c == ci).cloned().collect())
            });
            let before = pieces.entry(net).or_insert_with(|| {
                let n = pieces_of(net, &drawn, None);
                pieces0.insert(net, n.clone());
                n
            }).clone();
            let pre = drawn[&w].clone();
            let mut wo = drawn.remove(&w).expect("drawn wire");
            let mut ok = dr.refit_run(&mut wo, r, pts);
            drawn.insert(w, wo);
            if ok {
                // The split guard: the step is not taken when it splits a piece of the net that was
                // joined (partitions, not counts: r5 verifier M2) or leaves a new dangling end.
                let after = pieces_of(net, &drawn, None);
                if super::netconn::worse(&before, &after) && !guard_off {
                    let now = drawn.remove(&w).expect("drawn wire");
                    let back = dr.restore_wire(&now, &pre);
                    drawn.insert(w, back);
                    split_refused.0 += 1;
                    ok = false;
                    if fdbg {
                        eprintln!("FUSE refused wire {w} end {end} onto {u} i {i} gain {gain:.2}: it splits the net or leaves a new dangling end ({} pieces, {} before; split guard)", after.n(), before.n());
                    }
                } else {
                    pieces.insert(net, after);
                }
            }
            if !ok {
                if fdbg {
                    eprintln!("FUSE skip wire {w} end {end} onto {u} i {i} gain {gain:.2}: refit not taken at ({:.3}, {:.3})", q.x / MM, -q.y / MM);
                }
                refused.insert((w, end, q.x.round() as i64, q.y.round() as i64, i));
                pool[k].stamp = usize::MAX;
                continue;
            }
            let ci = conn_of[wire_conn[w]];
            necking.retain(|(c, _)| *c != ci);
            settle(octi_on, &mut out, &mut necking, ci, w, &drawn[&w], b, &st, model.rules.default.width as f64);
            if let Some(sv) = saved {
                // Where the copper changed: the stretch taken away and the new link.
                let mut path = cut.clone();
                path.push(q);
                moves.push((super::fusepour::Move { w, onto: u, path: vec![path] }, sv));
            }
            fused.push((gain, len_gain));
            fused_at.insert((w, end));
            anchors.entry(u).or_default().push(q);
            if fdbg {
                let wi = &st.wires[w];
                let pl = |p: &[Vec2]| p.windows(2).map(|x| (x[1] - x[0]).len()).sum::<f64>() / MM;
                eprintln!("FUSED round {round} wire {w} net {} end {end} onto {u} at ({:.3}, {:.3}) gain {gain:.2} mm, length gain {len_gain:.3} mm, i {i}, cut {:.3} mm, wire now {:.3} mm", wi.net, q.x / MM, -q.y / MM, pl(&cut), drawn[&w].length_nm / MM);
            }
            pool.swap_remove(k);
            cands.swap_remove(k);
            version = draw::lazy_taken(&mut pool, version, net);
        }
    }
    free_snap("fuse", &drawn, &st);
    // M-45: what the steps after the drawing left not at 0/45/90 degrees (a fused branch end)
    // becomes an elbow or Z where the checker accepts it.
    if octi_on {
        let ids: Vec<WireId> = drawn.keys().copied().collect();
        let (mut n_pol, mut pol_nm) = (0usize, 0.0);
        // Test hook "polish drops feet": the polish gets no fixed vertices (the r3 bug), so only the
        // split guard keeps the feet on their trunks.
        let drop_feet = ctl.test_point("polish drops feet");
        // Test hook "polish drops anchors": only the own bookkeeping feet go (planned T points, fused
        // feet); the other copper's feet found on the geometry (r6, `netconn::feet_on`) stay.
        let drop_anchors = ctl.test_point("polish drops anchors");
        for w in ids {
            let mut wo = drawn.remove(&w).expect("drawn wire");
            let len0 = wo.length_nm;
            let ci = conn_of[wire_conn[w]];
            let saved: Option<Saved> = pour_check.then(|| (wo.clone(), out[ci].clone(), necking.iter().filter(|(c, _)| *c == ci).cloned().collect()));
            let net = st.wires[w].net;
            // The split guard (H1 part 2): each redrawn segment keeps the net's pieces.
            let mut joined = |pre: &WireOut, now: &WireOut| -> bool {
                let before = pieces.entry(net).or_insert_with(|| {
                    let n = pieces_of(net, &drawn, Some((w, pre)));
                    pieces0.entry(net).or_insert_with(|| n.clone());
                    n
                }).clone();
                let after = pieces_of(net, &drawn, Some((w, now)));
                if super::netconn::worse(&before, &after) && !guard_off {
                    split_refused.1 += 1;
                    if fdbg {
                        let segs = super::fusepour::changed(&pre.runs, &now.runs);
                        let at: Vec<String> = segs.iter().map(|s| format!("({:.3}, {:.3})->({:.3}, {:.3})", s[0].x / MM, -s[0].y / MM, s[1].x / MM, -s[1].y / MM)).collect();
                        let was: Vec<String> = pre.runs.iter().map(|(l, p)| format!("L{l}: {}", p.iter().map(|q| format!("({:.3}, {:.3})", q.x / MM, -q.y / MM)).collect::<Vec<_>>().join(" "))).collect();
                        eprintln!("POLISH refused wire {w} net {net}: it splits the net or leaves a new dangling end ({} pieces, {} before; split guard); new segments {}; wire before {}; keep {:?}", after.n(), before.n(), at.join(" "), was.join(" | "), anchors.get(&w).map(|a| a.iter().map(|q| format!("({:.3}, {:.3})", q.x / MM, -q.y / MM)).collect::<Vec<_>>()));
                    }
                    return false;
                }
                pieces.insert(net, after);
                true
            };
            // Keep points: the wire's own junction feet (planned T points, fused feet) and, r6 (r5
            // verifier L2), every end or via of the net's other wires lying within KiCad's reach of
            // one of its segments, projected onto it.
            let keep: Vec<Vec2> = if drop_feet {
                Vec::new()
            } else {
                fn piece(o: &WireOut, half_w: f64) -> super::netconn::Piece<'_> {
                    super::netconn::Piece { runs: &o.runs, vias: o.vias.iter().map(|&(p, sz)| (p, sz.d as f64 / 2.0)).collect(), half_w }
                }
                let others: Vec<super::netconn::Piece> = net_wires.get(&net).map_or(&[][..], |v| v.as_slice()).iter().filter(|&&x| x != w).filter_map(|&x| drawn.get(&x).map(|o| piece(o, st.wires[x].w / 2.0))).collect();
                let mut k: Vec<Vec2> = if drop_anchors { Vec::new() } else { anchors.get(&w).cloned().unwrap_or_default() };
                k.extend(super::netconn::feet_on(&piece(&wo, st.wires[w].w / 2.0), &others));
                k
            };
            let k = dr.octi_polish(&mut wo, &keep, &mut joined);
            if k > 0 {
                if let Some(sv) = saved {
                    let path = super::fusepour::changed(&sv.0.runs, &wo.runs);
                    polish_of.insert(moves.len(), (k, wo.length_nm - len0));
                    moves.push((super::fusepour::Move { w, onto: super::fusepour::NO_WIRE, path }, sv));
                }
                n_pol += k;
                pol_nm += wo.length_nm - len0;
                necking.retain(|(c, _)| *c != ci);
                settle(octi_on, &mut out, &mut necking, ci, w, &wo, b, &st, model.rules.default.width as f64);
            }
            drawn.insert(w, wo);
        }
        polish = Some((n_pol, pol_nm));
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
    free_snap("polish", &drawn, &st);
    if !moves.is_empty() {
        use super::fusepour;
        let t_pc = Instant::now();
        let (sreg, _) = Registry::from_config(standard_factors(), &opts.factors);
        // Part of the drawing's last steps: a limit acting as hard cuts its stitching as it cuts
        // the variant table's (verify V6); a cut stitching decides nothing (the moves stay).
        cut_step.set(CutStep::Stitching);
        let pc_cut = std::cell::Cell::new(false);
        // Test hook "fuse pour cut": the stitching is cut at once.
        let test_cut = ctl.test_point("fuse pour cut");
        // Test point (acts, e.g. reaches the limit; the answer is not used): the stitching begins.
        let _ = ctl.test_point("fuse pour check begin");
        let pc_stop = || {
            let c = test_cut || ctl.is_cancelled() || limit_cut();
            if c {
                pc_cut.set(true);
            }
            c
        };
        // The pieces a stitching leaves cut off in the copper of `o` (the variant table's pass).
        let left_of = |o: &[Outcome]| {
            let a = assemble(model, dec, o, &term_over, signal[0], &reg);
            let pads = super::stitch::routed_pour_pads(model, &a.reports);
            super::stitch::stitch_until(model, opts, &sreg, &a.items, &pads, None, &pc_stop).left
        };
        let mut unfused = out.clone();
        for (m, sv) in moves.iter().rev() {
            unfused[conn_of[wire_conn[m.w]]] = sv.1.clone();
        }
        let base = left_of(&unfused);
        let mut cur = left_of(&out);
        let first = cur.len();
        let first_new = fusepour::new_pieces(&cur, &base).len();
        let mv: Vec<fusepour::Move> = moves.iter().map(|(m, _)| m.clone()).collect();
        let mut live = vec![true; mv.len()];
        let mut rounds = 0usize;
        let mut undone_nets: BTreeSet<String> = BTreeSet::new();
        let net_name = |w: WireId| model.nets.iter().find(|n| n.code == st.wires[w].net).map_or_else(|| st.wires[w].net.to_string(), |n| n.name.clone());
        // Undo the moves of `set` that are still kept, latest first: each wire ends at the state
        // its earliest undone move replaced (drawn wire with the drawing's index, outcome, necked
        // stretches). Returns how many it undid.
        let undo = |set: &BTreeSet<usize>, live: &mut Vec<bool>, drawn: &mut BTreeMap<WireId, WireOut>, out: &mut Vec<Outcome>, necking: &mut Vec<(usize, Event)>, dr: &mut Drawer, names: &mut BTreeSet<String>| -> usize {
            let mut n = 0;
            for &i in set.iter().rev() {
                if !std::mem::replace(&mut live[i], false) {
                    continue;
                }
                n += 1;
                let (m, (wo, o, nk)) = &moves[i];
                let ci = conn_of[wire_conn[m.w]];
                let now = drawn.remove(&m.w).expect("moved wire is drawn");
                let back = dr.restore_wire(&now, wo);
                out[ci] = o.clone();
                necking.retain(|(c, _)| *c != ci);
                necking.extend(nk.iter().cloned());
                drawn.insert(m.w, back);
                names.insert(net_name(m.w));
            }
            n
        };
        // Test hooks: judge the moved copper worse whatever it leaves, so every move is undone;
        // undo the first fuse move in one round whatever the counts (a partial undo); report the
        // first kept fuse move's wire as clashing once in the re-check (verify V3).
        let undo_all = ctl.test_point("fuse pour undo all");
        let partial = ctl.test_point("fuse pour partial undo");
        let fake_clash = std::cell::Cell::new(ctl.test_point("fuse pour clash"));
        // Hook "fuse pour worse": the first round judges the copper worse whatever the counts and
        // seeds its undo with the first kept fuse move (the nearest-move choice, which needs a real
        // new piece), so `fuse_pour_rounds` (0: every move at once) decides what is undone.
        let worse = ctl.test_point("fuse pour worse");
        let mut rechecked = 0usize;
        while !pc_cut.get() && (undo_all || (partial && rounds == 0) || (worse && rounds == 0) || !fusepour::fused_ok(&cur, &base)) && !ctl.is_cancelled() {
            let mut seed: BTreeSet<usize> = BTreeSet::new();
            if partial && rounds == 0 {
                seed.extend(mv.iter().position(|m| m.onto != fusepour::NO_WIRE));
            } else if rounds < opts.fuse_pour_rounds && !undo_all {
                if worse {
                    seed.extend(mv.iter().zip(&live).position(|(m, l)| *l && m.onto != fusepour::NO_WIRE));
                }
                for c in fusepour::new_pieces(&cur, &base) {
                    seed.extend(fusepour::nearest_move(&mv, &live, &fusepour::piece_points(model, c)));
                }
            }
            let all = seed.is_empty();
            if all {
                seed = (0..mv.len()).filter(|&i| live[i]).collect();
            }
            // The violations of each wire with a kept move before this round's undo: the re-check
            // undoes a kept move only where the copper that came back adds violations.
            let pre: BTreeMap<WireId, usize> = if all {
                BTreeMap::new()
            } else {
                mv.iter().zip(&live).filter(|(_, l)| **l).map(|(m, _)| m.w).collect::<BTreeSet<_>>().into_iter().map(|w| (w, drawn.get(&w).map_or(0, |wo| dr.wire_viols(wo)))).collect()
            };
            undo(&fusepour::with_dependents(&mv, &seed), &mut live, &mut drawn, &mut out, &mut necking, &mut dr, &mut undone_nets);
            // Re-check (verify V3): the moves kept were legal with the undone copper in place;
            // the copper that came back may clash with them. Undo those too, until none clashes.
            while !all && live.iter().any(|l| *l) {
                let fake_w = fake_clash.get().then(|| mv.iter().zip(&live).find(|(m, l)| **l && m.onto != fusepour::NO_WIRE).map(|(m, _)| m.w)).flatten();
                fake_clash.set(false);
                let clash = fusepour::recheck_seed(&mv, &live, &pre, &|w| drawn.get(&w).map_or(0, |wo| dr.wire_viols(wo)) + if fake_w == Some(w) { 1_000_000 } else { 0 });
                if clash.is_empty() {
                    break;
                }
                rechecked += undo(&clash, &mut live, &mut drawn, &mut out, &mut necking, &mut dr, &mut undone_nets);
            }
            rounds += 1;
            if all || live.iter().all(|l| !l) {
                cur = base.clone();
                break;
            }
            cur = left_of(&out);
        }
        let n_undone = live.iter().filter(|l| !**l).count();
        let n_fuse = mv.iter().filter(|m| m.onto != fusepour::NO_WIRE).count();
        let fuse_undone = mv.iter().zip(&live).filter(|(m, l)| m.onto != fusepour::NO_WIRE && !**l).count();
        // Self-check: the undone wires' tracks are back in the drawing's index.
        let stale = moves.iter().enumerate().filter(|(i, (m, _))| !live[*i] && drawn.get(&m.w).is_some_and(|wo| !dr.handles_hold(wo))).count();
        if stale > 0 {
            warnings.push(format!("fuse pour self-check: {stale} undone moves left wires whose tracks are not in the drawing's index (engine bug)"));
        }
        if n_undone > 0 {
            // The fuse line counts the fuse moves kept (`fused` holds one entry per fuse move, in
            // the order of the fuse moves in `moves`), the polish line the polish moves kept.
            let mut it = mv.iter().zip(&live).filter(|(m, _)| m.onto != fusepour::NO_WIRE).map(|(_, l)| *l);
            fused.retain(|_| it.next().unwrap_or(true));
            if let Some((n, len)) = polish.as_mut() {
                for (i, (k, d)) in &polish_of {
                    if !live[*i] {
                        *n -= k;
                        *len -= d;
                    }
                }
            }
        }
        warnings.push(format!(
            "fuse pour check: the stitching leaves {first} pour pieces cut off in the copper after the fuse and the octilinear polish, {} without their moves ({first_new} of the first not cut off without the moves); {n_undone} of {} moves undone (fuse {fuse_undone} of {n_fuse}, polish {} of {}; {rechecked} by the re-check of the kept moves){}, now {} ({} of them new){} ({:.2} s; these steps never cut off a pour piece the copper without them keeps connected, piece sets not counts, errors first, TopoR 6.3 p. 72)",
            base.len(),
            mv.len(),
            n_undone - fuse_undone,
            mv.len() - n_fuse,
            if undone_nets.is_empty() { String::new() } else { format!(" ({})", undone_nets.iter().cloned().collect::<Vec<_>>().join(", ")) },
            cur.len(),
            fusepour::new_pieces(&cur, &base).len(),
            if pc_cut.get() { "; the time limit or Cancel cut its stitching: the moves not undone by then stay" } else { "" },
            t_pc.elapsed().as_secs_f64()
        ));
    }
    if let Some((n, len)) = polish {
        warnings.push(polish_line(n, len / MM));
    }
    if tree_free && opts.fuse_branches {
        warnings.push(fuse_line(&fused));
    }
    // The split guard's self-check (rule 12: every path, the fuse pour check's undo too): a net
    // the steps after the drawing touched whose copper joined before them ends apart.
    let split_nets: Vec<String> = pieces0
        .iter()
        .filter(|&(&net, p0)| super::netconn::worse(p0, &pieces_of(net, &drawn, None)))
        .map(|(&net, _)| model.nets.iter().find(|n| n.code == net).map_or_else(|| net.to_string(), |n| n.name.clone()))
        .collect();
    warnings.extend(split_line(split_refused.0, split_refused.1, &split_nets));
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
    // M6 (6b, 2026-11-18): only `[optimize] loop = "cycles"` routes again from this memory (and
    // from the room hints below); with the TopoR loop a run is one route_round, so none of it is
    // computed there (F1-F5 of the 6b inventory: drawing feedback into a later plan).
    let cycles = opts.opt_loop == "cycles";
    let fade = opts.history_fade.clamp(0.0, 1.0);
    let mut hist_out: Vec<f64> = if !cycles { Vec::new() } else { history.iter().enumerate().map(|(e, &h)| {
        let h0 = hist0.get(e).copied().unwrap_or(0.0);
        let v = fade * h0 + (h - h0);
        if v < HIST_EPS { 0.0 } else { v }
    }).collect() };
    if cycles {
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
        let _ = std::fs::write(format!("{path}.s{}.r{round}", opts.seed), dr.dump());
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
    for (&ci, m) in mem.own.iter().filter(|_| cycles) {
        let m2: BTreeMap<usize, f64> = m.iter().map(|(&e, &h)| (e, h * fade)).filter(|&(_, h)| h >= HIST_EPS).collect();
        if !m2.is_empty() {
            own_out.insert(ci, m2);
        }
    }
    // Per connection: the places (point, radius) where its copper breaks a rule or its drawing
    // was rejected.
    let mut spots: BTreeMap<WireId, Vec<(Vec2, f64)>> = BTreeMap::new();
    for (w, wo) in drawn.iter().filter(|_| cycles) {
        let pitch = st.wires[*w].w + st.wires[*w].s;
        for v in &wo.viol {
            spots.entry(*w).or_default().push((v.at, (0.5 * v.len_nm).max(pitch)));
        }
    }
    for d in diag.iter().filter(|_| cycles) {
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
    for d in diag.iter().filter(|_| cycles) {
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
    if cycles {
        warnings.push(format!(
            "{CYCLES_MEMORY} {} room hints, {} connections with their own history, {} edges with history",
            new_hints.len(),
            own_out.len(),
            hist_out.iter().filter(|&&h| h > 0.0).count()
        ));
    }
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
    // edge-clear: pad ends moved off their centre (a track end there broke a static clearance).
    warnings.push(format!(
        "pad ends: {} track ends moved inside their pad off the centre (the end's copper at the centre broke a static clearance: board edge, foreign pad, hole, keepout), largest move {:.3} mm; {} pad ends with no legal point in the pad at the wire's width (left at the centre)",
        dr.stats.pad_end_moved,
        dr.stats.pad_end_max / 1e6,
        dr.stats.pad_end_none
    ));
    // Debug (`TOPO_ERBS_DUMP` with `realise = "sequential"`): the plan's ERBS model and over-full
    // cuts dumped after the drawing (read by the packing classifier); nothing reads the result, so
    // the copper and the drawing's time are those of the run without the dump.
    if opts.realise != "erbs" && std::env::var_os("TOPO_ERBS_DUMP").is_some() {
        let pts: BTreeMap<WireId, Vec<(usize, Vec2)>> = placed.iter().map(|(&w, v)| (w, v.iter().map(|x| (x.0, x.1)).collect())).collect();
        let layers: Vec<usize> = (0..b.n_signal).collect();
        let conn = |w: WireId| wire_conn.get(w).filter(|&&c| c != usize::MAX).map(|&c| conn_of[c]);
        let mo = super::cutflow::MeasureOpts::from_options(opts);
        let _ = super::erbs::realise(&st, &lr.events, &packer, bottom, &conn, &pts, &dr.pad_end, &super::board::outline_segments(model), &mo, &layers, &live, super::erbs::ErbsOpts::default());
    }
    if let Some(d) = std::env::var_os("TOPO_ERBS_DUMP") {
        let nets: serde_json::Map<String, serde_json::Value> = model.nets.iter().map(|n| (n.code.to_string(), serde_json::Value::from(n.name.clone()))).collect();
        let _ = std::fs::write(std::path::Path::new(&d).join("nets.json"), serde_json::Value::Object(nets).to_string());
    }
    if let Some(l) = erbs_line {
        let drawn_part = erbs_runs.as_ref().map_or_else(|| "drawn: 0 runs from their ERBS".to_string(), super::joint::drawn_text);
        warnings.push(format!("{l}; {drawn_part}; {} ERBS priced past their band against a sequential run ({} replaced by it, {} kept because the sequential run crossed an undrawn wire's path with another parity: another class)", dr.stats.erbs_long, dr.stats.erbs_long_replaced, dr.stats.erbs_class_kept));
    }
    // M3 length factor (`draw::erbs_length_price`): every ERBS priced past its band, the longest
    // over its band first (plugin finding ERBS_LONG).
    if opts.realise == "erbs" && !dr.erbs_long.is_empty() {
        let mut ls = dr.erbs_long.clone();
        ls.sort_by(|a, b| (b.2 / b.3.max(1.0)).total_cmp(&(a.2 / a.3.max(1.0))));
        let ex: Vec<String> = ls
            .iter()
            .take(5)
            .map(|&(w, l, le, lb, sq, rep)| {
                let what = if rep {
                    format!("drawn sequential ({:.3} mm)", sq / 1e6)
                } else if sq.is_nan() {
                    "ERBS kept (no legal sequential run)".to_string()
                } else {
                    format!("ERBS kept (sequential {:.3} mm)", sq / 1e6)
                };
                format!("wire {w} layer {l}: ERBS {:.3} mm over band {:.3} mm (x{:.2}), {what}", le / 1e6, lb / 1e6, le / lb.max(1.0))
            })
            .collect();
        warnings.push(format!(
            "erbs_long: {} ERBS longer than their band past [drawing] erbs_margin, priced by erbs_length_weight against a sequential run: {} replaced by a cheaper sequential run, {} kept; {}",
            ls.len(),
            ls.iter().filter(|x| x.5).count(),
            ls.iter().filter(|x| !x.5).count(),
            ex.join("; ")
        ));
    }
    // M3: a loud line whenever realise = "erbs" did not draw every run from its ERBS
    // (plugin finding ERBS_FALLBACK; the time limit's share is erbs_timeout's, ERBS_TIMEOUT).
    if let Some(e) = &erbs_err {
        warnings.push(format!("erbs_fallback: no ERBS for the whole run, every trace drawn the sequential way: {e}"));
    } else if let Some(er) = erbs_runs.as_ref().filter(|r| r.cut_back + r.forced_erbs > 0 || !erbs_failed.is_empty()) {
        let ex: Vec<String> = erbs_failed.iter().take(3).map(|(w, m)| format!("wire {w}: {m}")).collect();
        warnings.push(format!(
            "erbs_fallback: {} run(s) drawn by the sequential attempts after the check rejected their ERBS (static {}, predicted {}, gap {}, no band {}, junction {}, redrawn {}), {} on their ERBS by never drop, {} stretch(es) without a sleeve to the end{}{}",
            er.cut_back,
            er.cut_static,
            er.cut_predicted,
            er.cut_gap,
            er.cut_no_band,
            er.cut_junction,
            er.cut_redrawn,
            er.forced_erbs,
            erbs_failed.len(),
            if ex.is_empty() { "" } else { ": " },
            ex.join("; ")
        ));
    }
    // draw-absorb A1 r2 (lead 2026-11-27; POWER-NECK-MONOTONE): every neck of a wire A1 necked
    // (`[drawing] forced_shrink`: a joint-commit band or a forced run) on a supply-width net (wider
    // than the board's default width) gets a warning of its own, graded by the width it loses
    // times its length (its narrowest part sets the current), the most severe first.
    {
        let dflt = model.rules.default.width as f64;
        let net_name = |net: i32| model.nets.iter().find(|n| n.code == net).map_or(String::new(), |n| n.name.clone());
        let mut lines: Vec<(f64, String)> = Vec::new();
        for (&w, wo) in &drawn {
            let wi = &st.wires[w];
            if !dr.joint.a1_wires.contains(&w) || wi.w <= dflt + 0.5 {
                continue;
            }
            let ci = conn_of[wire_conn[w]];
            for &(_, to, len) in &wo.necks {
                if to >= wi.w - 0.5 || len <= 0.0 {
                    continue;
                }
                let (left, sev) = a1_supply_grade(wi.w, to, len);
                lines.push((
                    sev,
                    format!(
                        "{} ({}): supply-width net necked by draw-absorb A1 ([drawing] forced_shrink): {:.3} mm (board default {:.3} mm) to {:.3} mm over {:.2} mm, {:.0} % of its width left (its narrowest part sets its current); severity {:.2} (width lost x mm)",
                        net_name(wi.net),
                        dec.connections[ci].from.label,
                        wi.w / MM,
                        dflt / MM,
                        to / MM,
                        len / MM,
                        100.0 * left,
                        sev
                    ),
                ));
            }
        }
        lines.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        warnings.extend(lines.into_iter().map(|l| l.1));
    }
    if dr.stats.octi_asis > 0 {
        // first-draw-45 P1: counted, not hidden (the free-angle count does not see them: they are
        // octilinear).
        warnings.push(format!(
            "octilinear drawing: {} octilinear pieces of taut runs kept as drawn where no 45 degree connector passed the leg test ({} of them below the clearance in the DP's check, priced as a violation; the full checker still checks every run)",
            dr.stats.octi_asis, dr.stats.octi_asis_below
        ));
    }
    if opts.via_relocate && opts.search_length == "taut" {
        warnings.push(format!(
            "via relocation in the drawing ([vias] relocate): {} vias at their planned site, {} planned sites not free (drawn at the usual search's site)",
            dr.stats.planned_sites, dr.stats.planned_sites_missed
        ));
    }
    // 5.4 task 2 ("decide early, draw once": a via the plan did not price is a bug to explain):
    // the plan's vias against the drawn ones, and every drawn via the plan's wire vias do not
    // hold, by class. The plane stub vias are decided before the topology and enter it as
    // obstacles (priced, `planes.rs`); the pour stitching vias come after routing (reported by
    // the stitching).
    {
        let drawn_vias: usize = drawn.values().map(|wo| wo.vias.len()).sum();
        let plane_vias = plane_res.iter().filter(|(_, r)| r.as_ref().is_ok_and(|o| o.via.is_some())).count();
        let lost: usize = plans.iter().filter(|(w, _)| !drawn.contains_key(w)).map(|(_, p)| p.vias.len()).sum();
        warnings.push(format!(
            "vias, plan vs drawn: plan {plan_vias} wire vias, drawn {drawn_vias} wire vias (+ {plane_vias} plane stub vias decided before the topology); placements: {} at the planned site, {} by the drawing's site search ({} of them where the planned site was not free), {} by force (never drop); {} out of the plan's class (no site in it); {} added joining a branch to its trunk by force; {} removed by the via reduction; {} planned vias on wires not drawn; {} corners removed by the taut pull of the drawn runs (P6 c, [drawing] pull_taut)",
            dr.stats.planned_sites,
            dr.stats.via_searched,
            dr.stats.planned_sites_missed,
            dr.stats.via_forced,
            dr.stats.via_out_of_class,
            junction_join_vias,
            vias_reduced,
            lost,
            dr.stats.pulled
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
        warnings.extend(realise_pairs(model_real, opts, pairs, &mut out, &mut necking, octi_on, &real_checker, dec, &reg));
        tlog("diff pairs", t_dp);
        if let Some((o, nk)) = pre_snap.as_mut() {
            let _ = realise_pairs(model_real, opts, pairs, o, nk, octi_on, &real_checker, dec, &reg);
        }
    }
    let isl = planes::islands(model, b, &out_vias(&out, &plane_reqs, &dec.connections), &plane_reqs);
    warnings.extend(isl);
    // ---- Assemble the result ----
    let first_layer = signal[0];
    let name_of = |net: i32| model.nets.iter().find(|n| n.code == net).map_or(String::new(), |n| n.name.clone());
    let asm = assemble(model, dec, &out, &term_over, first_layer, &reg);
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
    let crit_main = criteria(model, dec, &asm, &out, warn_n(&necking), n_copper, &reg, opts);
    if std::env::var("TOPO_FUSEDBG").is_ok() {
        // Debug: the connections each variant counts as an error (open or violating), to compare.
        let errs = |o: &[Outcome]| -> Vec<usize> { o.iter().enumerate().filter(|(_, x)| !x.routed || !x.viol.is_empty()).map(|(i, _)| i).collect() };
        eprintln!("VARIANT reduced+fused errors {:?}", errs(&out));
        if let Some((o, _)) = pre_snap.as_ref() {
            eprintln!("VARIANT as drawn errors {:?}", errs(o));
        }
    }
    let pre_parts = pre_snap.as_ref().map(|(o, nk)| {
        let a = assemble(model, dec, o, &term_over, first_layer, &reg);
        let c = criteria(model, dec, &a, o, warn_n(nk), n_copper, &reg, opts);
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
    if let Some(line) = mono_line(&forced_stats) {
        warnings.push(line);
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
        if let Some(line) = forced_free_line(octi_on, &forced_stats) {
            warnings.push(line);
        }
        // Q14b: a trace whose copper overlaps copper of another net (a pad, an existing via, a
        // copper shape, a routed track or via) or runs inside a no-tracks rule area is a short
        // the optimiser could not take out; like a crossing on one layer (above), the user sees it
        // loudly, once per trace and object, with the layer and place (costly, not failed: the
        // trace stays drawn).
        let mut seen: BTreeSet<(usize, &str)> = BTreeSet::new();
        for v in viol_list.iter().filter(|v| v.kind == "short") {
            if seen.insert((v.conn, v.against.as_str())) {
                warnings.push(format!(
                    "CRITICAL: trace {} ({} -> {}) runs through {} on {} at ({:.3}, {:.3}) mm: the optimiser could not take it out; it is drawn and the overlap is a violation (a short) to fix by hand",
                    v.name,
                    v.from,
                    v.to,
                    v.against,
                    v.layer,
                    v.at[0] as f64 / MM,
                    v.at[1] as f64 / MM
                ));
            }
        }
        for (ci, ev) in viol_events(model, dec, &out).into_iter().filter(|(_, e)| matches!(e, Event::Violation { .. })) {
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
    // M1 compass, `[capacity] compass_cut_check` (model = "edges"; off by default, harness.score_runs
    // turns it on): the read-only cut test of the plan handed to the drawing, after the drawing; its
    // time goes back to the run's limit ([`RunControl::measured`]), so it never takes loop time.
    if let Some(cp) = compass_plan.as_mut().filter(|_| !cuts_on && opts.compass_cut_check) {
        let t_chk = Instant::now();
        let opts_chk = Options { capacity_model: "cuts".into(), ..opts.clone() };
        let mut pk = Packer::for_model(model, &opts_chk, proj);
        pk.via_squeeze = packer.via_squeeze;
        let lname = |l: usize| b.signal_layers.get(l).and_then(|&id| model.layers.iter().find(|x| x.id == id)).map_or(format!("layer {l}"), |x| x.name.clone());
        let wc = |w: WireId| wire_conn.get(w).filter(|&&c| c != usize::MAX).map(|&c| conn_of[c]);
        let fset = forced.borrow().clone();
        super::refine::meter::suspended(|| {
            let cm_chk = CapModel::new_cut(&st, &pk, &dirs_plan, &lr.events, bottom, 1.0);
            if cm_chk.cut.is_some() {
                let view = super::compass::PlanView { st: &st, lr: &lr, cm_edge: &cm_plan, cm_cut: Some(&cm_chk), cut_is_plan: false, bottom, conn: &wc, layer: &lname, forced: &fset, pad_exits: None };
                super::compass::model_check(&view, cp);
            }
        });
        ctl.measured(t_chk.elapsed().as_secs_f64());
    }
    let result = RouteResult {
        items,
        remove_uuids: Vec::new(),
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
        limit: None,
        bests: Vec::new(),
        erbs: erbs_runs,
        draw_absorb: None,
        compass: compass_plan.map(|mut c| {
            c.drawn_shrinkages = crit_main.shrinkages;
            c
        }),

        plan: plan_dump,
    };
    let pre = pre_parts.map(|(c, a)| (c, with_parts(&result, a)));
    let vias_out: BTreeMap<usize, usize> = plans.iter().filter(|_| cycles).map(|(&w, pl)| (conn_of[wire_conn[w]], pl.vias.len())).collect();
    (RoundOut { result, crit: crit_main, pre, lp: loop_end, skipped: None, nothing: None, draw_exempt: draw_exempt.get(), drew: drew_row, extra: extra_rows }, new_hints, Memory { edge: hist_out, own: own_out, vias: vias_out, plan: plan_carry })
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
        let long = |a: usize, b: usize| diffpair::pad_long(&model.pads[a]).max(diffpair::pad_long(&model.pads[b]));
        let (ea, eb) = (diffpair::Ends { p: pt(pa), n: pt(na), long: long(pa, na) }, diffpair::Ends { p: pt(pb), n: pt(nb), long: long(pb, nb) });
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
        let dp_src = |mut v: Vec<draw::Viol>| {
            for x in v.iter_mut() {
                x.src.mech = "diff_pair";
            }
            v
        };
        let vp = dp_src(diffpair::check(&ctx, &idx, &rz.p_items));
        for it in &rz.p_items {
            idx.insert(it);
        }
        let vn = dp_src(diffpair::check(&ctx, &idx, &rz.n_items));
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
            o.neck_graded_mm = 0.0;
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
        // the tidy that fills it runs after (route.rs finish_copper), on the result handed back
        remove_uuids: Vec::new(),
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
    // cm1-room: the exit pins of every connection's terminal pads (`route::exit_pins`), for the
    // violation class; computed once per connection with a violation.
    let pad_keys: Vec<(&str, &str, Vec2)> = model.pads.iter().map(|p| (p.ref_.as_str(), p.number.as_str(), p.center.to_vec2())).collect();
    let term_pad = |t: &crate::decompose::TermRef| match t.kind {
        crate::decompose::TermKind::Pad(i) => Some(i),
        _ => None,
    };
    let mut exits_of: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    // draw-absorb A0: the connection of each drawn router wire (its items carry the wire), for the
    // connection a violation's copper hits.
    let wire_conn: BTreeMap<usize, usize> = if out.iter().any(|o| o.routed && !o.viol.is_empty()) {
        out.iter().enumerate().filter(|(_, o)| o.routed).flat_map(|(ci, o)| o.items.iter().filter_map(move |it| match it {
            Item::Track(t) => t.wire.map(|w| (w, ci)),
            Item::Via(v) => v.wire.map(|w| (w, ci)),
        })).collect()
    } else {
        BTreeMap::new()
    };
    let srcs: BTreeMap<usize, &[draw::Viol]> = out.iter().enumerate().filter(|(_, o)| o.routed).map(|(ci, o)| (ci, o.viol.as_slice())).collect();
    let mut nth: BTreeMap<usize, usize> = BTreeMap::new();
    for (ci, ev) in viol_events(model, dec, out) {
        let Event::Violation { kind, at, layer, depth_nm, len_nm, against, cause } = &ev else { continue };
        // viol_events lists a connection's violations in `o.viol` order.
        let k = nth.entry(ci).or_insert(0);
        let src = srcs.get(&ci).and_then(|vs| vs.get(*k)).map(|v| &v.src);
        *k += 1;
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
            class: crate::route::violation_class(
                against,
                cause,
                exits_of.entry(ci).or_insert_with(|| [&c.from, &c.to].into_iter().filter_map(&term_pad).flat_map(|i| crate::route::exit_pins(&pad_keys, i)).collect()),
            )
            .into(),
            prov: src.map(|x| prov_of(x, against, &wire_conn)),
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

/// draw-absorb A0: the report of a drawn violation's provenance `x` (`against`: what it is too
/// close to; `wire_conn`: drawn router wire -> connection id). The compass and fault classes are
/// filled from the whole result ([`super::compass::absorb_meter`]). O(log wires).
fn prov_of(x: &draw::ViolSrc, against: &str, wire_conn: &BTreeMap<usize, usize>) -> crate::route::ViolProv {
    let routed = against.starts_with("track of") || against.starts_with("via of") || against.starts_with("arc of");
    crate::route::ViolProv {
        mech: x.mech.into(),
        cand: x.cand.into(),
        form: x.form.into(),
        cut: x.cut.into(),
        run: x.run,
        wrong_side: x.wrong_side,
        plan_neck: x.plan_neck,
        w_nm: x.w_nm.round() as i64,
        w_min_nm: x.w_min_nm.round() as i64,
        drawn_w_nm: x.drawn_w_nm.round() as i64,
        hit: if x.hit_wire.is_some() || routed { "movable" } else { "static" }.into(),
        hit_wire: x.hit_wire,
        hit_conn: x.hit_wire.and_then(|w| wire_conn.get(&w).copied()),
        hit_reserved: x.hit_reserved,
        hit_slack_nm: x.hit_slack_nm.map(|v| v.round() as i64),
        class: String::new(),
        fclass: String::new(),
    }
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
/// factor findings; `opts`: `[optimize] pick_shorts` counts the connections with a short, ranked
/// at `short_weight`).
/// O(items).
#[allow(clippy::too_many_arguments)]
fn criteria(model: &BoardModel, dec: &crate::decompose::Decomposition, a: &Assembled, out: &[Outcome], warnings: usize, n_layers: usize, reg: &Registry, opts: &Options) -> Criteria {
    let shorts_on = opts.pick_shorts;
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
    // Of those: the connections whose copper overlaps another net's ([`draw::viol_key`]'s short:
    // gap < 0, KiCad `shorting_items` / `tracks_crossing`). Each is already one of `errors`.
    let shorts = if shorts_on { out.iter().filter(|o| o.routed && o.viol.iter().any(|v| v.kind == "short")).count() } else { 0 };
    let penalty = viol_events(model, dec, out).iter().map(|(ci, ev)| reg.price(ev, &WireCtx { net: dec.connections[*ci].net, tier: dec.connections[*ci].tier })).sum::<f64>()
        + dec.connections.iter().flat_map(|c| out[c.id].dp_events.iter().map(move |ev| (c, ev))).map(|(c, ev)| reg.price(ev, &WireCtx { net: c.net, tier: c.tier })).sum::<f64>();
    // pick-necks: necked vias = vias smaller than the net class via of their net.
    let mut full_via: BTreeMap<i32, i64> = BTreeMap::new();
    let mut neck_vias = 0usize;
    for c in &dec.connections {
        let o = &out[c.id];
        if !o.routed {
            continue;
        }
        let full = *full_via.entry(c.net).or_insert_with(|| crate::vias::via_sizes(&model.rules, c.net, opts).first().map_or(0, |s| s.d));
        neck_vias += o.vias.iter().filter(|v| v.1.d < full).count();
    }
    let neck_mm: f64 = out.iter().filter(|o| o.routed).map(|o| o.neck_graded_mm).sum();
    Criteria {
        errors: open + violating,
        shorts,
        short_extra: if shorts_on { opts.short_weight.max(1.0) - 1.0 } else { 0.0 },
        open,
        violations,
        penalty,
        vias: a.tot_vias,
        length_mm: a.tot_len as f64 / MM,
        shrinkages: out.iter().filter(|o| o.routed).map(|o| o.neck_n).sum(),
        neck_mm,
        neck_vias,
        necking: neck_mm + opts.neck_via_mm.max(0.0) * neck_vias as f64,
        warnings,
        area_mm2: area / (MM * MM),
    }
}

/// `(connection id, via centre)` of every routed plane connection (for the island check).
fn out_vias(out: &[Outcome], reqs: &[planes::Req], _conns: &[Connection]) -> Vec<(usize, Vec2)> {
    reqs.iter().filter(|r| out[r.conn].routed).filter_map(|r| out[r.conn].vias.first().map(|v| (r.conn, v.0))).collect()
}

/// TW7 A debug dump (`TOPO_PATDBG`): one line per live wire whose projected length is over 1.5
/// times its straight distance, with every pattern it takes part in ([`super::xrules`]: terminal
/// clinch, double crossing, unlayerable, U-clinch, mutual evasion; `loop` = a same-net wire whose
/// projection crosses it) and the other wire's net. Read-only. O(detectors).
fn patdbg(model: &BoardModel, st: &State, unlayerable: &[(WireId, WireId)], seed: u64) {
    use super::xrules::{self as xr, Kind, Pair};
    let mut pairs: Vec<Pair> = xr::terminal_clinches(st);
    pairs.extend(xr::double_crossings(st));
    pairs.extend(unlayerable.iter().map(|&(a, b)| Pair { first: a.min(b), second: a.max(b), kind: Kind::Unlayerable }));
    let uc = xr::u_clinches(st, &|_| false);
    pairs.extend(xr::mutual_evasions(st, &uc));
    pairs.extend(uc);
    let loops = xr::same_net_loops(st);
    let name = |w: WireId| model.nets.iter().find(|n| n.code == st.wires[w].net).map_or_else(|| st.wires[w].net.to_string(), |n| n.name.clone());
    let mm = |v: crate::geom::Vec2| format!("({:.2},{:.2})", v.x / 1e6, -v.y / 1e6);
    for (w, r, with) in xr::attribute(st, &pairs, &loops, 1.5) {
        let wi = &st.wires[w];
        let kinds: Vec<String> = with.iter().map(|&(k, o)| format!("{}:{}", k.map_or("loop", Kind::name), name(o))).collect();
        eprintln!("PATDBG[{seed}] wire {w} net {} {}-{} ratio {r:.2} patterns [{}]", name(w), mm(st.proj.pos[wi.a]), mm(st.proj.pos[wi.b]), kinds.join(", "));
    }
}

/// TW7 E: the rank scale of a cut-off pour piece in the pattern moves' order, in the units of the
/// excess-length amounts `(r - 1)^2` (10 = a wire 4.2 times its straight distance): the measure
/// prices the cut as a predicted violation, so it comes before the excess lengths (graded by its
/// pads, and by its fruitless tries like every pattern).
const POUR_RANK: f64 = 10.0;

/// TW7 B (`[optimize] pattern_stuck` = `w`): a found pattern's place in the moves' order, its
/// excess length `amt` (the graded detour amounts of its two wires) times `1 / (1 + w x stuck)`,
/// `stuck` = its fruitless tries since its last kept move. Graded, no cutoff: it falls with every
/// fruitless try and stays above 0; `w = 0` = `amt`. O(1).
fn pattern_rank(amt: f64, stuck: u32, w: f64) -> f64 {
    amt / (1.0 + w * f64::from(stuck))
}

/// TW7 B: the run report line of `[optimize] pattern_moves` (and its entries no kind matches).
fn pattern_moves_line(kinds: &[String], share: f64, stuck_w: f64, pour_on: bool, st: &super::optimise::CycleStats) -> String {
    use super::xrules::Kind;
    let bad: Vec<&str> = kinds.iter().map(String::as_str).filter(|k| Kind::parse(k).is_none()).collect();
    let per: Vec<String> = Kind::ALL
        .iter()
        .filter(|k| kinds.iter().any(|x| x == k.name()))
        .map(|k| {
            let c = st.pat_kinds[k.idx()];
            format!("{} {}/{} kept, {} found", k.name(), c[2], c[1], c[0])
        })
        .collect();
    let mut s = format!(
        "TopoR loop, pattern moves ([optimize] pattern_moves = {}; Luzin 2015 p. 7, Luzin, Popov & Popov 2012 p. 118): {} of {} moves kept ({} patterns found; {}; each pattern's wires laid again in the rule's order, kept when the board's evaluation dropped; the batch crossing pass is off in the loop)",
        kinds.join(", "),
        st.pat_kept,
        st.pat_moved,
        st.pat_found,
        if per.is_empty() { "no known kind".to_string() } else { per.join("; ") }
    );
    let tot = st.pat_work + st.pat_base;
    let measured = if tot > 0.0 { 100.0 * st.pat_work / tot } else { 0.0 };
    if share < 1.0 {
        s.push_str(&format!(
            "; work budget [optimize] pattern_share = {share}: the moves took {measured:.1} % of the loop's work, {} found pattern(s) waited for a later cycle",
            st.pat_waited
        ));
    } else {
        s.push_str(&format!("; no work budget ([optimize] pattern_share = 1): the moves took {measured:.1} % of the loop's work"));
    }
    if stuck_w > 0.0 {
        s.push_str(&format!("; order: excess length x 1 / (1 + {stuck_w} x the pattern's fruitless tries) ([optimize] pattern_stuck = {stuck_w})"));
    } else {
        s.push_str("; order: excess length alone ([optimize] pattern_stuck = 0)");
    }
    if !pour_on && kinds.iter().any(|k| k == Kind::PourCut.name()) {
        s.push_str("; \"pour\" finds nothing without [capacity] pour_cuts = true (the plan's pour check)");
    }
    if !bad.is_empty() {
        s.push_str(&format!("; unknown entries ignored: {} (known: {})", bad.join(", "), Kind::known()));
    }
    s
}

#[cfg(test)]
mod tests {
    /// neck-regrow (lead 2026-11-26): only a neck over the whole wire of a wire wider than the
    /// default is a supply whole-wire neck.
    #[test]
    fn whole_supply_neck_needs_the_whole_wire_and_a_supply_width() {
        use super::whole_supply_neck as f;
        let (dflt, wire) = (250_000.0, 10_000_000.0);
        assert!(f(500_000.0, 300_000.0, wire, wire, dflt));
        assert!(f(500_000.0, 300_000.0, wire - 999.0, wire, dflt), "within 1 um of the wire's length");
        assert!(!f(500_000.0, 300_000.0, wire - 1_001.0, wire, dflt), "a neck short of the wire");
        assert!(!f(250_000.0, 127_000.0, wire, wire, dflt), "a default-width wire");
        assert!(!f(250_000.4, 127_000.0, wire, wire, dflt), "within 0.5 nm of the default");
        assert!(f(250_400.0, 127_000.0, wire, wire, dflt), "0.4 um above the default is wider");
        assert!(!f(500_000.0, 499_999.6, wire, wire, dflt), "no neck (within 0.5 nm of the width)");
    }

    #[test]
    fn sketch_of_carries_the_violation_places_it_is_given() {
        let viols = vec![
            crate::route::ViolationReport { at: [1_000, 2_000], ..Default::default() },
            crate::route::ViolationReport { at: [-3_000, 4_000], ..Default::default() },
        ];
        let at = super::places_of(&viols);
        assert_eq!(at, vec![[1_000, 2_000], [-3_000, 4_000]]);
        let c = Criteria { errors: 2, violations: 2, ..Default::default() };
        let sk = super::sketch_of(1, 3, "copper", &c, Vec::new(), at.clone());
        assert_eq!((sk.errors, sk.errors_at.clone()), (2, at));
        // plan-dots: a plan sketch carries the places it is given (its plan's, `compass::plan_places`)
        assert_eq!(super::sketch_of(1, 0, "plan", &c, Vec::new(), vec![[5, 6]]).errors_at, vec![[5, 6]]);
        // an older sketch JSON without the field still parses
        let js = serde_json::to_string(&sk).unwrap().replace(",\"errors_at\":[[1000,2000],[-3000,4000]]", "");
        assert!(serde_json::from_str::<crate::control::Sketch>(&js).unwrap().errors_at.is_empty());
    }

    /// forced-draw (T5): the never-drop octilinear line, every case of its text: no line with the
    /// octilinear style off or no forced run; the counts of each candidate kind summed; zero and
    /// nonzero free-angle counts.
    #[test]
    fn forced_free_line_counts_every_forced_run() {
        let mut s = super::draw::DrawStats::default();
        assert_eq!(super::forced_free_line(true, &s), None, "no forced run, no line");
        s.forced_relaxed = 2;
        s.forced_funnel = 1;
        s.forced_attempt = 1;
        assert_eq!(super::forced_free_line(false, &s), None, "octilinear style off, no line");
        assert_eq!(
            super::forced_free_line(true, &s).as_deref(),
            Some("never drop, octilinear: 0 of 4 forced runs drawn in their octilinear form; 0 forced runs keep 0 free-angle stretches (0.0 mm), where the octilinear form costs more violation price or there is none (a layer change for them is the plan's decision)")
        );
        s.forced_octi = 3;
        s.forced_free_runs = 1;
        s.forced_free_stretches = 2;
        s.forced_free_mm = 12.34;
        let line = super::forced_free_line(true, &s).expect("a line");
        assert!(line.starts_with("never drop, octilinear: 3 of 4 forced runs drawn in their octilinear form; 1 forced runs keep 2 free-angle stretches (12.3 mm)"), "{line}");
    }

    /// neck-regrow r6: the neck-profile line, both cases of its text (no join: no line).
    #[test]
    fn mono_line_names_the_joined_necks() {
        let mut s = super::draw::DrawStats::default();
        assert_eq!(super::mono_line(&s), None);
        s.mono_wires = 2;
        s.mono_mm = 12.345;
        assert_eq!(
            super::mono_line(&s).as_deref(),
            Some("neck profile (never wider again after a neck): 2 wire drawings that would widen again after a neck (necks at both ends, or a neck in mid-run) were narrowed on to one end, one neck stretch per wire (12.35 mm narrowed)")
        );
        s.mono_stubs = 1;
        assert!(super::mono_line(&s).unwrap().ends_with("(12.35 mm narrowed); 1 narrowed pad-end stubs moved out of KiCad's in-pad delete (the trace keeps its pad-centre end)"));
    }

    /// TW7 B `[optimize] pattern_stuck`: the order falls with every fruitless try, never to 0,
    /// and weight 0 is the excess length alone.
    #[test]
    fn the_pattern_rank_is_graded_by_fruitless_tries() {
        let r = super::pattern_rank;
        assert_eq!(r(0.8, 0, 1.0), 0.8);
        assert_eq!(r(0.8, 7, 0.0), 0.8, "weight 0 = the excess alone");
        assert!((r(0.8, 1, 1.0) - 0.4).abs() < 1e-12 && (r(0.8, 3, 0.5) - 0.32).abs() < 1e-12);
        let mut last = f64::MAX;
        for k in 0..200 {
            let x = r(0.8, k, 1.0);
            assert!(x < last && x > 0.0, "graded, no cutoff at {k}: {x}");
            last = x;
        }
        assert!(r(1.0, 3, 1.0) < r(0.3, 0, 1.0), "a pattern that keeps failing gives way to a fresh smaller one");
    }

    /// TW7 B: the pattern moves' report line, per kind, with and without the work budget, with
    /// unknown entries and with no known kind.
    #[test]
    fn the_pattern_moves_line_is_true_for_every_case() {
        let mut st = super::super::optimise::CycleStats { pat_found: 9, pat_moved: 5, pat_kept: 2, pat_waited: 4, pat_work: 25.0, pat_base: 75.0, ..Default::default() };
        st.pat_kinds[super::super::xrules::Kind::UClinch.idx()] = [6, 3, 1];
        st.pat_kinds[super::super::xrules::Kind::MutualEvasion.idx()] = [3, 2, 1];
        let k = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let a = super::pattern_moves_line(&k(&["u_clinch", "evasion"]), 0.25, 1.0, false, &st);
        assert!(a.contains("2 of 5 moves kept (9 patterns found; u_clinch 1/3 kept, 6 found; evasion 1/2 kept, 3 found;"), "{a}");
        assert!(a.contains("pattern_share = 0.25: the moves took 25.0 % of the loop's work, 4 found pattern(s) waited"), "{a}");
        assert!(!a.contains("unknown") && !a.contains("no work budget"), "{a}");
        assert!(a.ends_with("; order: excess length x 1 / (1 + 1 x the pattern's fruitless tries) ([optimize] pattern_stuck = 1)"), "{a}");
        let b = super::pattern_moves_line(&k(&["u_clinch", "bogus"]), 1.0, 0.0, false, &st);
        assert!(b.contains("u_clinch 1/3 kept, 6 found;") && !b.contains("evasion 1/2"), "{b}");
        assert!(b.contains("no work budget ([optimize] pattern_share = 1): the moves took 25.0 %"), "{b}");
        assert!(b.ends_with("; order: excess length alone ([optimize] pattern_stuck = 0); unknown entries ignored: bogus (known: clinch, double, unlayerable, u_clinch, evasion, via_trade, pour)"), "{b}");
        let h = super::pattern_moves_line(&k(&["u_clinch"]), 0.25, 0.5, false, &st);
        assert!(h.contains("(1 + 0.5 x the pattern's fruitless tries) ([optimize] pattern_stuck = 0.5)"), "{h}");
        assert!(!a.contains("pour") && !h.contains("pour"), "{a}");
        let mut sp = st;
        sp.pat_kinds[super::super::xrules::Kind::PourCut.idx()] = [2, 2, 1];
        sp.pat_kinds[super::super::xrules::Kind::ViaTrade.idx()] = [4, 3, 0];
        let p_off = super::pattern_moves_line(&k(&["via_trade", "pour"]), 0.25, 1.0, false, &sp);
        assert!(p_off.contains("via_trade 0/3 kept, 4 found; pour 1/2 kept, 2 found;") && p_off.contains("; \"pour\" finds nothing without [capacity] pour_cuts = true"), "{p_off}");
        let p_on = super::pattern_moves_line(&k(&["via_trade", "pour"]), 0.25, 1.0, true, &sp);
        assert!(!p_on.contains("finds nothing"), "{p_on}");
        let c = super::pattern_moves_line(&k(&["bogus"]), 1.0, 1.0, false, &super::super::optimise::CycleStats::default());
        assert!(c.contains("0 of 0 moves kept (0 patterns found; no known kind;") && c.contains("took 0.0 %"), "{c}");
    }

    /// TW5 (f): the in-loop P4 cap keeps the heaviest victims, then the routed-last ones, then by id;
    /// below the cap it keeps them all.
    #[test]
    fn p4_keep_heaviest_orders_by_weight_then_rank() {
        let v = vec![(0.5, 3, 10), (0.9, 1, 11), (0.5, 7, 12), (0.5, 7, 9), (0.2, 9, 13)];
        assert_eq!(super::p4_keep_heaviest(v.clone(), 3), vec![11, 9, 12]);
        assert_eq!(super::p4_keep_heaviest(v.clone(), 10), vec![11, 9, 12, 10, 13]);
        assert!(super::p4_keep_heaviest(v, 0).is_empty());
    }

    #[test]
    fn the_wait_line_is_true_for_every_case() {
        let loop_ = WaitFacts { step: "TopoR loop on the topology".into(), grace_s: Some(30.0), draw_grace_s: Some(60.0), wait_s: 2.0, ..WaitFacts::default() };
        let l = wait_line(&loop_);
        assert!(l.starts_with("time limit wait: the limit was reached during TopoR loop on the topology (part of the TopoR loop); waited 2.0 s"), "{l}");
        assert!(l.contains("finished within [run] limit_grace_s = 30 s"), "{l}");
        let past = wait_line(&WaitFacts { wait_s: 31.5, ..loop_.clone() });
        assert!(past.contains("ran past [run] limit_grace_s = 30 s") && !past.contains("finished within"), "{past}");
        let hit = wait_line(&WaitFacts { wait_s: 30.2, grace_hit: true, cycle_dropped: true, ..loop_.clone() });
        assert!(hit.contains("limit_grace_s = 30 s ran out") && hit.contains("was dropped"), "{hit}");
        let half = wait_line(&WaitFacts { grace_s: Some(0.5), wait_s: 0.7, grace_hit: true, cycle_dropped: true, ..loop_.clone() });
        assert!(half.contains("limit_grace_s = 0.5 s ran out"), "{half}");
        let unb = wait_line(&WaitFacts { grace_s: None, ..loop_.clone() });
        assert!(unb.contains("unbounded"), "{unb}");
        // the overrun cap (grace not hit) is not called a grace run-out
        let cap = wait_line(&WaitFacts { cap_hit: true, cycle_dropped: true, step_cut: true, ..loop_.clone() });
        assert!(cap.contains("overrun cap") && cap.contains("was dropped") && !cap.contains("ran out") && cap.contains("that step stopped after"), "{cap}");
        // hard mode: cut, or no stop check left; never "the soft limit"
        let hard = wait_line(&WaitFacts { hard: true, step: "projection of the board".into(), step_cut: true, cut_words: "a planning step".into(), ..loop_.clone() });
        assert!(hard.contains("timeout_mode = hard: the limit cut this step") && hard.contains("(part of the initial routing)") && hard.contains("a planning step"), "{hard}");
        let hard_end = wait_line(&WaitFacts { hard: true, step: "via reduction and fuse".into(), ..loop_.clone() });
        assert!(hard_end.contains("did not cut this step") && !hard_end.contains("soft limit"), "{hard_end}");
        // soft, the initial routing: runs to its end
        let init = wait_line(&WaitFacts { step: "topological search".into(), ..loop_.clone() });
        assert!(init.contains("(part of the initial routing)") && init.contains("soft limit lets this step run to its end"), "{init}");
        // limit-p3p4: the layer assignment after the topological search, bounded by limit_grace_s
        let plan = WaitFacts { step: super::PLAN_READY_STAGE.into(), ..loop_.clone() };
        let p = wait_line(&plan);
        assert!(p.contains("(part of the initial routing)") && p.contains("(P3/P4) finished within [run] limit_grace_s = 30 s"), "{p}");
        let pp = wait_line(&WaitFacts { wait_s: 31.5, ..plan.clone() });
        assert!(pp.contains("(P3/P4) ran past [run] limit_grace_s = 30 s"), "{pp}");
        let ph = wait_line(&WaitFacts { wait_s: 30.2, grace_hit: true, ..plan.clone() });
        assert!(ph.contains("limit_grace_s = 30 s ran out") && ph.contains("the plan so far (every connection placed) was kept and drawn") && !ph.contains("dropped"), "{ph}");
        let pu = wait_line(&WaitFacts { grace_s: None, ..plan.clone() });
        assert!(pu.contains("unbounded") && pu.contains("(P3/P4) ran to their end"), "{pu}");
        let pm = wait_line(&WaitFacts { step: format!("run 2: {}; run 3: topological search", super::PLAN_READY_STAGE), grace_hit: true, ..plan.clone() });
        assert!(pm.contains("(P3/P4) stopped at their next check"), "{pm}");
        // hard mode and whole-route cycles keep their own texts
        let phard = wait_line(&WaitFacts { hard: true, step_cut: true, grace_hit: true, ..plan.clone() });
        assert!(phard.contains("timeout_mode = hard: the limit cut this step") && !phard.contains("ran out"), "{phard}");
        let pcyc = wait_line(&WaitFacts { whole_cycle: true, ..plan.clone() });
        assert!(pcyc.contains("whole-route cycle") && pcyc.contains("run to its end"), "{pcyc}");
        // verify r1 F1: loop = "cycles" with the grace run out in P3/P4: the cycle's P3/P4 were cut
        let pcyc_hit = wait_line(&WaitFacts { whole_cycle: true, wait_s: 30.2, grace_hit: true, ..plan.clone() });
        assert!(
            pcyc_hit.contains("limit_grace_s = 30 s ran out") && pcyc_hit.contains("(P3/P4) of the whole-route cycle that ran stopped at their next check") && !pcyc_hit.contains("run to its end"),
            "{pcyc_hit}"
        );
        // the stage before the plan-ready words came first counts the same
        let old = wait_line(&WaitFacts { step: "layer assignment, rip-up and reroute".into(), grace_hit: true, ..plan.clone() });
        assert!(old.contains("(P3/P4) stopped at their next check"), "{old}");
        // the drawing: its bound
        let draw = WaitFacts { step: "drawing the traces".into(), ..loop_.clone() };
        let d = wait_line(&draw);
        assert!(d.contains("(part of the drawing)") && d.contains("every wire was drawn") && d.contains("within [run] drawing_grace_s = 60 s"), "{d}");
        let dh = wait_line(&WaitFacts { draw_grace_hit: true, draw_grace_erbs: 3, ..draw.clone() });
        assert!(dh.contains("drawing_grace_s = 60 s ran out") && dh.contains("3 wire(s) drawn the sequential way"), "{dh}");
        let du = wait_line(&WaitFacts { draw_grace_s: None, ..draw.clone() });
        assert!(du.contains("drawing_grace_s is unbounded"), "{du}");
        // a limit in the loop, then the drawing bound ran out in the drawing after it
        let then = wait_line(&WaitFacts { draw_grace_hit: true, ..loop_.clone() });
        assert!(then.contains("finished within [run] limit_grace_s") && then.contains("; then the drawing: every wire was drawn; [run] drawing_grace_s = 60 s ran out"), "{then}");
        // a dropped cycle that is also a drawing step keeps the drawing bound's note (defensive)
        let dd = wait_line(&WaitFacts { cycle_dropped: true, draw_grace_hit: true, draw_grace_erbs: 2, ..draw.clone() });
        assert!(dd.contains("was dropped") && dd.contains("drawing_grace_s = 60 s ran out") && dd.contains("2 wire(s)"), "{dd}");
        let ddh = wait_line(&WaitFacts { cycle_dropped: true, hard: true, draw_grace_hit: true, ..draw.clone() });
        assert!(ddh.contains("was dropped") && ddh.contains("drawing_grace_s = 60 s ran out"), "{ddh}");
        // r5 finding 1: the bound ran out, then the cap cut the drawing of the traces: no "every wire was drawn"
        for dc in [WaitFacts { cycle_dropped: true, cap_hit: true, draw_grace_hit: true, draw_cut: true, ..draw.clone() }, WaitFacts { draw_grace_hit: true, draw_cut: true, ..loop_.clone() }] {
            let t = wait_line(&dc);
            assert!(t.contains("drawing_grace_s = 60 s ran out") && !t.contains("every wire was drawn") && t.contains("cut the drawing of the traces"), "{t}");
        }
        // whole-route cycles
        let cyc = wait_line(&WaitFacts { whole_cycle: true, step: "drawing the traces".into(), ..loop_.clone() });
        assert!(cyc.contains("whole-route cycle") && cyc.contains("run to its end"), "{cyc}");
        // a run of many
        let many = wait_line(&WaitFacts { step: "run 1: TopoR loop on the topology; run 2: topological search".into(), ..loop_.clone() });
        assert!(many.contains("(part of the TopoR loop)") && many.contains("finished within"), "{many}");
    }

    #[test]
    fn the_wait_line_comes_whenever_a_cycle_was_dropped_or_the_step_waited() {
        assert!(wait_line_due(true, 0.0004, false), "any wait is reported");
        assert!(wait_line_due(true, 0.0, true), "a dropped cycle is reported though the wait rounds to 0");
        assert!(!wait_line_due(true, 0.0, false));
        assert!(!wait_line_due(false, 5.0, true), "no limit reached: nothing to say");
    }

    use super::*;

    fn coord_obj() -> Objective {
        let o = Options::default();
        Objective::from_options(&o.objective, o.mm_per_via, &o.pick)
    }

    fn predicted(errors: usize) -> RunBest {
        RunBest { crit: Criteria { errors, ..Criteria::default() }, routed: 3, total: 3, cycle: 1 }
    }

    #[test]
    fn a_runs_stable_never_goes_back_in_the_progress() {
        // "Report stable live": the loop's live report, then the round's end (which knows only its
        // own meter) and the run's end; "Routes stable" stays on throughout (ui_wx: never goes back).
        let ctl = RunControl::new();
        let coord = Coord::new(&ctl, 1, coord_obj(), 1);
        assert!(!ctl.snapshot().stable);
        coord.report(0, |r| {
            r.stable = true;
            r.predicted = Some(predicted(0));
        });
        assert!(ctl.snapshot().stable);
        coord.ended(0, |r| r.stable = false, None);
        assert!(ctl.snapshot().stable, "the round's end cleared the live stable");
        coord.report(0, |r| r.done = true);
        assert!(ctl.snapshot().stable);
        // two runs: stable only when every run is stable or done
        let ctl = RunControl::new();
        let coord = Coord::new(&ctl, 2, coord_obj(), 1);
        coord.report(0, |r| {
            r.stable = true;
            r.predicted = Some(predicted(0));
        });
        assert!(!ctl.snapshot().stable);
        coord.report(1, |r| r.done = true);
        assert!(ctl.snapshot().stable);
    }

    /// User 2026-11-17: "Routes stable" (the button turns into OK) only once there is a pick to
    /// keep: a live stable without any pick (drawn or predicted) is not shown; the loop's predicted
    /// pick is Best so far until a pick is drawn, and the drawn one replaces it.
    #[test]
    fn routes_stable_waits_for_the_first_pick() {
        let ctl = RunControl::new();
        let coord = Coord::new(&ctl, 1, coord_obj(), 1);
        coord.report(0, |r| r.stable = true);
        let p = ctl.snapshot();
        assert!(!p.stable && p.best_total == 0 && p.runs_stable == 1, "stable, but Best so far empty: not shown");
        coord.report(0, |r| r.predicted = Some(RunBest { routed: 2, ..predicted(1) }));
        let p = ctl.snapshot();
        assert!(p.stable && p.best_total == 3 && p.best_routed == 2 && p.best_errors == 1 && p.best_round == 1, "{p:?}");
        // the drawn pick replaces the predicted one in Best so far
        let drawn = RunBest { crit: Criteria { errors: 2, ..Criteria::default() }, routed: 3, total: 3, cycle: 1 };
        coord.ended(0, |r| r.best = Some(drawn), None);
        let p = ctl.snapshot();
        assert!(p.stable && p.best_errors == 2 && p.best_routed == 3, "{p:?}");
        // a drawn pick alone (the cycles loop) is enough too
        let ctl = RunControl::new();
        let coord = Coord::new(&ctl, 1, coord_obj(), 1);
        coord.report(0, |r| r.stable = true);
        assert!(!ctl.snapshot().stable);
        coord.ended(0, |r| r.best = Some(drawn), None);
        assert!(ctl.snapshot().stable);
        // two runs: the better predicted pick is Best so far
        let ctl = RunControl::new();
        let coord = Coord::new(&ctl, 2, coord_obj(), 1);
        coord.report(0, |r| r.predicted = Some(predicted(3)));
        coord.report(1, |r| r.predicted = Some(predicted(1)));
        let p = ctl.snapshot();
        assert!(!p.stable && p.best_run == 2 && p.best_errors == 1, "{p:?}");
    }

    /// verify-sl2 V1: OK vs Cancel is what the progress showed at the first stop request, latched
    /// there: a run that turns stable (or "every run stable or done") after a Cancel does not make
    /// it an OK, and a second request does not change it.
    #[test]
    fn the_stop_request_is_latched_at_the_press() {
        let ctl = RunControl::new();
        let coord = Coord::new(&ctl, 2, coord_obj(), 1);
        coord.report(0, |r| {
            r.stable = true;
            r.predicted = Some(predicted(0));
        });
        assert!(!ctl.snapshot().stable && !ctl.ok_at_stable());
        ctl.request_cancel(); // 1 of 2 runs stable: Cancel
        coord.report(1, |r| r.done = true); // the other run ends after the Cancel
        assert!(ctl.snapshot().stable, "premise: the progress turns stable after the press");
        assert!(ctl.is_cancelled() && !ctl.ok_at_stable(), "a Cancel stays a Cancel");
        ctl.request_stop(Some(true));
        assert!(!ctl.ok_at_stable(), "only the first request counts");
        // OK: the progress shows Routes stable at the press
        let ctl = RunControl::new();
        let coord = Coord::new(&ctl, 1, coord_obj(), 1);
        coord.report(0, |r| {
            r.stable = true;
            r.predicted = Some(predicted(0));
        });
        assert!(!ctl.ok_at_stable(), "no stop request yet");
        ctl.request_cancel();
        assert!(ctl.ok_at_stable());
        // the caller's own view wins (the plugin passes what its button read)
        let ctl = RunControl::new();
        ctl.request_stop(Some(true));
        assert!(ctl.ok_at_stable());
        let ctl = RunControl::new();
        let coord = Coord::new(&ctl, 1, coord_obj(), 1);
        coord.report(0, |r| {
            r.stable = true;
            r.predicted = Some(predicted(0));
        });
        ctl.request_stop(Some(false));
        assert!(ctl.is_cancelled() && !ctl.ok_at_stable());
        // a direct store of the flag is a Cancel
        let ctl = RunControl::new();
        ctl.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(!ctl.ok_at_stable());
    }

    /// User 2026-11-17 "free the slot": a run waiting for its first slot goes before one that
    /// waits to go on, and a loop run gives its slot to it at a cycle boundary ([`Slots::turn`]).
    #[test]
    fn a_stable_run_gives_its_slot_to_a_run_not_yet_begun() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let slots = Slots::new(1);
        assert!(!slots.turn(5, 0), "nobody waits: no yield");
        let begun = AtomicUsize::new(0);
        std::thread::scope(|s| {
            slots.acquire_first(); // run 1 computes
            let h = s.spawn(|| {
                slots.acquire_first(); // run 2 waits for its first slot
                begun.store(1, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(50));
                begun.store(2, Ordering::SeqCst);
                slots.release();
            });
            while slots.st.lock().expect("slots").first == 0 {
                std::thread::yield_now();
            }
            let gave = slots.turn(5, 0);
            let at = begun.load(Ordering::SeqCst);
            // run 1 holds one slot either way: release it before asserting, so a failed check ends
            // the test instead of leaving run 2 waiting for ever
            slots.release();
            h.join().expect("run 2");
            assert!(gave, "run 2 waits: run 1 gives its slot");
            assert_eq!(at, 2, "run 1 went on only after run 2 gave the slot back");
        });
        let g = slots.st.lock().expect("slots");
        assert_eq!((g.free, g.first), (1, 0));
    }

    /// sync-wait: a free slot goes to the lowest waiter (first slot, then fewest cycles finished,
    /// then the lower run), whatever order they came in.
    #[test]
    fn a_free_slot_goes_to_the_lowest_waiter() {
        use std::sync::Mutex;
        let slots = Slots::new(1);
        slots.acquire_first(); // the holder
        let order = Mutex::new(Vec::new());
        let keys = [(1u8, 7usize, 0usize), (1, 3, 2), (0, 0, 9), (1, 3, 1), (1, 9, 0)];
        std::thread::scope(|s| {
            for (i, &(c, k, r)) in keys.iter().enumerate() {
                let (slots, order) = (&slots, &order);
                s.spawn(move || {
                    slots.acquire_as(c, k, r);
                    order.lock().unwrap().push(i);
                    slots.release();
                });
                // each waiter queued before the next one comes (arrival order = index order)
                while slots.st.lock().unwrap().queue.len() < i + 1 {
                    std::thread::yield_now();
                }
            }
            slots.release();
        });
        assert_eq!(*order.lock().unwrap(), vec![2, 3, 1, 0, 4], "first slot, then (3, run 1), (3, run 2), 7, 9");
        let g = slots.st.lock().unwrap();
        assert_eq!((g.free, g.first, g.waiting, g.queue.len()), (1, 0, 0, 0));
    }

    /// sync-wait (d9402d48): at the moment the lower waiter takes the slot, the run that turns is already
    /// queued (one lock frees the slot and queues it); with two locks a waiter behind it could take the slot in the gap.
    /// Observes the invariant at the hand-over, so it catches the two-lock turn (an end-state order test did not).
    #[test]
    fn a_turning_run_is_queued_when_the_lower_waiter_takes_its_slot() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let missing = AtomicUsize::new(0);
        for _ in 0..200 {
            let slots = Slots::new(1);
            slots.acquire_as(1, 4, 0);
            std::thread::scope(|s| {
                s.spawn(|| {
                    slots.acquire_as(1, 3, 2);
                    if !slots.st.lock().unwrap().queue.iter().any(|k| k.2 == 0) {
                        missing.fetch_add(1, Ordering::SeqCst);
                    }
                    slots.release();
                });
                while slots.st.lock().unwrap().queue.is_empty() {
                    std::thread::yield_now();
                }
                assert!(slots.turn(5, 0));
                slots.release();
            });
        }
        assert_eq!(missing.load(Ordering::SeqCst), 0, "the turning run was not queued when its slot was taken");
    }

    /// sync-wait: a loop run gives its slot at a cycle boundary only to a waiter behind it, and the
    /// run that gave it gets it back once it is the lowest (the stable yield is not one-way).
    #[test]
    fn a_run_that_gave_its_slot_gets_it_back() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let slots = Slots::new(1);
        slots.acquire_as(1, 4, 0); // run 0 computes, 4 cycles finished
        let (b_ran, a_back, ahead_late) = (AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false));
        let (kept, gave, b_first) = std::thread::scope(|s| {
            // run 1 waits to go on with 6 cycles finished: ahead of run 0, so no turn to it
            let ahead = s.spawn(|| {
                slots.acquire_as(1, 6, 1);
                ahead_late.store(a_back.load(Ordering::SeqCst), Ordering::SeqCst);
                slots.release();
            });
            while slots.st.lock().unwrap().queue.is_empty() {
                std::thread::yield_now();
            }
            let kept = !slots.turn(4, 0);
            // run 2 waits with 3 cycles finished: behind run 0
            let behind = s.spawn(|| {
                slots.acquire_as(1, 3, 2);
                b_ran.store(true, Ordering::SeqCst);
                slots.release();
            });
            while !slots.st.lock().unwrap().queue.iter().any(|k| k.2 == 2) {
                std::thread::yield_now();
            }
            let gave = slots.turn(5, 0);
            let b_first = b_ran.load(Ordering::SeqCst);
            a_back.store(true, Ordering::SeqCst);
            // run 0 holds one slot either way: give it back before asserting, so a failed check ends
            // the test instead of leaving a waiter waiting for ever
            slots.release();
            behind.join().unwrap();
            ahead.join().unwrap();
            (kept, gave, b_first)
        });
        assert!(kept, "the only waiter is ahead: run 0 keeps its slot");
        assert!(gave, "run 2 is behind: run 0 gives its slot");
        assert!(b_first, "run 2 computed before run 0 got the slot back");
        assert!(ahead_late.load(Ordering::SeqCst), "run 1 (6 cycles) comes after run 0 (5 cycles) is back");
        assert_eq!(slots.st.lock().unwrap().free, 1);
    }

    /// sync-wait: the idle measure counts free slot-time only while a loop runs.
    #[test]
    fn slot_idle_counts_free_slots_while_a_loop_runs() {
        let slots = Slots::new(2);
        slots.acquire_first();
        std::thread::sleep(std::time::Duration::from_millis(150)); // no loop yet: not counted
        slots.enter_loop();
        std::thread::sleep(std::time::Duration::from_millis(60)); // 1 of 2 slots free
        slots.leave_loop();
        std::thread::sleep(std::time::Duration::from_millis(150)); // no loop: not counted
        let (idle, span, n) = slots.idle();
        assert_eq!(n, 2);
        assert!((0.055..0.2).contains(&span), "only the loop's time counts: span {span}");
        assert!((idle - span).abs() < 0.01, "one free slot over the span: idle {idle} span {span}");
        slots.release();
    }

    /// par-refine x stable-label: a stable yield while lanes hold slots. The lanes borrow free
    /// slots only while no run waits: a slot a lane gives back while a run waits for its first
    /// slot goes to that run, the slot a stable run yields goes to the run not yet begun, and a
    /// slot a lane gives back while the yielder waits to go on goes to the yielder, never to
    /// another lane. (A wrong loan is given back at once, so a failed check ends the test.)
    #[test]
    fn a_stable_yield_while_lanes_hold_slots() {
        use super::super::refine_par::lanes::Pool;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let slots = Slots::new(3);
        let state = |s: &Slots| {
            let g = s.st.lock().expect("slots");
            (g.free, g.first, g.waiting)
        };
        slots.acquire_first(); // run 1 computes
        // the guards alone, without the thread timing: free slots, but a run waits for its first
        // slot / to go on
        for (first, waiting) in [(1, 0), (0, 1)] {
            {
                let mut g = slots.st.lock().expect("slots");
                (g.first, g.waiting) = (first, waiting);
            }
            let lent = slots.try_take(8);
            slots.give(lent);
            {
                let mut g = slots.st.lock().expect("slots");
                (g.first, g.waiting) = (0, 0);
            }
            assert_eq!(lent, 0, "free slots while a run waits (first {first}, waiting {waiting}): no lane borrows");
        }
        assert_eq!(slots.try_take(8), 2, "nobody waits: the lanes borrow both free slots");
        let (begun, done, tried) = (AtomicUsize::new(0), AtomicBool::new(false), AtomicBool::new(false));
        std::thread::scope(|s| {
            let run = || {
                slots.acquire_first(); // a run waits for its first slot
                begun.fetch_add(1, Ordering::SeqCst);
                while !done.load(Ordering::SeqCst) {
                    std::thread::yield_now();
                }
                slots.release();
            };
            let run2 = s.spawn(run);
            while state(&slots).1 == 0 {
                std::thread::yield_now();
            }
            slots.give(1); // one lane ends its epoch: its slot is for run 2
            let lent2 = slots.try_take(8);
            slots.give(lent2);
            while begun.load(Ordering::SeqCst) < 1 {
                std::thread::yield_now();
            }
            // run 1, run 2 and one lane compute; run 3 waits for its first slot
            let run3 = s.spawn(run);
            while state(&slots) != (0, 1, 0) {
                std::thread::yield_now();
            }
            let run1 = s.spawn(|| {
                // run 1 is stable: it gives its slot to run 3 and waits to go on
                let gave = slots.turn(5, 0);
                // run 1 goes on: it keeps its slot until the lane below has tried (CI 2026-11-18:
                // a run 1 that went on and ended at once gave the slot back, and the lane rightly
                // borrowed that free slot)
                while !tried.load(Ordering::SeqCst) {
                    std::thread::yield_now();
                }
                slots.release();
                gave
            });
            while !(begun.load(Ordering::SeqCst) == 2 && state(&slots) == (0, 0, 1)) {
                std::thread::yield_now();
            }
            slots.give(1); // the other lane ends: its slot is for run 1
            let lent1 = slots.try_take(8);
            tried.store(true, Ordering::SeqCst);
            slots.give(lent1);
            let gave = run1.join().expect("run 1");
            done.store(true, Ordering::SeqCst);
            run2.join().expect("run 2");
            run3.join().expect("run 3");
            assert_eq!(lent2, 0, "run 2 waited for its first slot: a lane borrowed the slot given back");
            assert!(gave, "run 3 waited for its first slot: run 1 gave its slot");
            assert_eq!(lent1, 0, "run 1 waited to go on: a lane borrowed the slot given back");
        });
        assert_eq!(state(&slots), (3, 0, 0));
    }

    fn eprom() -> BoardModel {
        BoardModel::from_json(include_str!("../../tests/fixtures/eprom-to-kernal-ripped.json")).expect("fixture parses")
    }

    #[test]
    fn unsupported_ends_names_both_ends_for_every_pour_and_branch_pair() {
        // exist-term r2 (verifier F7, DoD 13): every value the reason can take. The pour arm got
        // "an existing via or track end joined to a pour" for (Pour, Pad), (Pour, Pour) and
        // (Junction, Pour) too.
        let kinds = [TermKind::Pad(0), TermKind::Via(0), TermKind::Copper("k".into()), TermKind::Dangling("k".into(), None), TermKind::Junction];
        let names = ["a pad", "an existing via", "an existing track end", "a dangling track end", "a branch point on a routed trace"];
        let z = TermKind::Pour(0);
        for (k, n) in kinds.iter().zip(names) {
            let back = unsupported_ends(k, &z);
            if matches!(k, TermKind::Pad(_)) {
                // (Pad, Pour) is routed by the plane-stub arms; the text exists for completeness
                assert!(back.starts_with("a pad joined to a pour: not supported"), "{back}");
            } else {
                assert_eq!(back, format!("{n} joined to a pour: not supported by the topological core yet (existing vias and track ends are terminals towards pads, vias and tracks; a pad joins its pour through a plane stub)"));
            }
            let front = unsupported_ends(&z, k);
            if matches!(k, TermKind::Pad(_)) {
                assert!(front.starts_with("a pour joined to a pad with the pour as the first end"), "{front}");
            } else {
                assert_eq!(front, back, "the pour's position does not change the text for {n}");
            }
        }
        assert!(unsupported_ends(&z, &z).starts_with("a pour joined to another pour"));
        assert_eq!(unsupported_ends(&TermKind::Junction, &TermKind::Pad(0)), "a branch point on a routed trace to a pad: a branch point is no terminal the topological core routes from");
        assert_eq!(unsupported_ends(&TermKind::Via(0), &TermKind::Junction), "an existing via to a branch point on a routed trace: a branch point is no terminal the topological core routes from");
        for k in &kinds {
            assert!(!unsupported_ends(k, &z).contains("S2a"), "no stale S2a text");
        }
    }

    #[test]
    fn exist_end_maps_track_ends_inside_own_copper_to_that_copper() {
        // exist-term (docs/dev/design/exist-term.md, model 1): a kept-track end inside an own-net
        // pad is that pad, inside an own-net via that via, elsewhere a kept-end terminal; another
        // net's pad around the end does not take it; an existing via is itself.
        let mm = |v: f64| (v * 1e6) as i64;
        let s = format!(
            r#"{{"units":"nm","board":"t","layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}}],
            "pads":[{{"uuid":"p1","ref":"R1","number":"1","net":1,"attr":"smd","center":[{},{}],"rotation_deg":0.0,"layers":[0],"shapes":{{"0":{{"kind":"rect","w":1000000,"h":1000000}}}}}},
                    {{"uuid":"p2","ref":"R2","number":"1","net":2,"attr":"smd","center":[{},{}],"rotation_deg":0.0,"layers":[0],"shapes":{{"0":{{"kind":"rect","w":1000000,"h":1000000}}}}}}],
            "kept":[{{"uuid":"a","net":1,"layer":0,"width":250000,"prim":{{"kind":"seg","a":[{},{}],"b":[{},{}]}}}},
                    {{"uuid":"b","net":1,"layer":0,"width":250000,"prim":{{"kind":"seg","a":[{},{}],"b":[{},{}]}}}},
                    {{"uuid":"c","net":1,"layer":0,"width":250000,"prim":{{"kind":"seg","a":[{},{}],"b":[{},{}]}}}}],
            "vias":[{{"uuid":"v1","net":1,"at":[{},{}],"d":600000,"drill":300000,"layers":[0,2]}}],
            "outline":[],"rules":{{"default":{{"clearance":200000,"width":250000}},"min_connection":127000,"max_error":5000,"per_net":{{}}}}}}"#,
            mm(3.0), mm(6.0), mm(3.0), mm(9.0),
            mm(3.3), mm(6.0), mm(8.0), mm(6.0),
            mm(18.1), mm(6.0), mm(12.0), mm(6.0),
            mm(3.2), mm(9.0), mm(6.0), mm(9.0),
            mm(18.0), mm(6.0)
        );
        let m = BoardModel::from_json(&s).expect("model");
        let t = |kind: TermKind, at: Vec2| TermRef { kind, label: String::new(), at, layers: vec![0] };
        let (a0, a1) = (m.kept[0].prim.start().to_f64(), m.kept[0].prim.end().to_f64());
        let (b0, c0) = (m.kept[1].prim.start().to_f64(), m.kept[2].prim.start().to_f64());
        assert_eq!(exist_end(&m, 1, &t(TermKind::Copper("a".into()), a0)), Some(ExistEnd::Pad(0)));
        assert_eq!(exist_end(&m, 1, &t(TermKind::Copper("b".into()), b0)), Some(ExistEnd::Via(0)));
        assert_eq!(exist_end(&m, 1, &t(TermKind::Copper("a".into()), a1)), Some(ExistEnd::Kept { kept: 0, at: a1, dangling: false, dir: None }));
        assert!(matches!(exist_end(&m, 1, &t(TermKind::Copper("c".into()), c0)), Some(ExistEnd::Kept { kept: 2, .. })), "a foreign pad does not take the end");
        assert_eq!(exist_end(&m, 1, &t(TermKind::Via(0), m.vias[0].at.to_vec2())), Some(ExistEnd::Via(0)));
        let d = Vec2 { x: 1.0, y: 0.0 };
        assert_eq!(exist_end(&m, 1, &t(TermKind::Dangling("a".into(), Some([1.0, 0.0])), a1)), Some(ExistEnd::Kept { kept: 0, at: a1, dangling: true, dir: Some(d) }));
    }

    #[test]
    fn projection_is_deterministic_so_one_build_serves_every_cycle() {
        // 4.2: the cycles read the call's `Fixed` (a shared reference, no interior mutability, so no
        // cycle can change it). Building it again gives exactly the same projection and
        // decomposition: one build per call gives each cycle what a rebuild per cycle gave.
        let m = eprom();
        for (cap, steiner) in [("edges", false), ("cuts", false), ("edges", true), ("cuts", true)] {
            let opts = Options { capacity_model: cap.into(), steiner, ..Options::default() };
            let stubs = PlaneStubs::default();
            let (a, b) = (Fixed::build(&m, &opts, &stubs), Fixed::build(&m, &opts, &stubs));
            assert!(a.board.is_ok(), "{cap}: {:?}", a.board.as_ref().err());
            assert_eq!(format!("{:?}", a.board), format!("{:?}", b.board), "{cap}");
            assert_eq!(format!("{:?}", a.dec), format!("{:?}", b.dec), "{cap}");
            assert_eq!(a.notes, b.notes, "{cap}");
        }
    }

    #[test]
    fn steiner_grid_is_an_option_default_off() {
        // 4.2: by default the projection's vertices are the fixed items (TopoR, Polubasov 2010);
        // `[search] steiner` adds the free grid points (junction candidates of a Y).
        assert!(!Options::default().steiner);
        let m = eprom();
        let free = |steiner: bool| {
            let fx = Fixed::build(&m, &Options { steiner, ..Options::default() }, &PlaneStubs::default());
            let b = fx.board.expect("projection");
            (b.proj.vkind.iter().filter(|k| matches!(k, VertKind::Free)).count(), b.proj.pos.len())
        };
        let ((f_off, n_off), (f_on, n_on)) = (free(false), free(true));
        assert!(f_on > f_off + 50 && n_on - n_off == f_on - f_off, "grid on {f_on}/{n_on}, off {f_off}/{n_off}");
        assert_eq!(build_opts(&Options::default()).steiner_mm, 0.0);
        assert_eq!(build_opts(&Options { steiner: true, ..Options::default() }).steiner_mm, BuildOpts::default().steiner_mm);
    }

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

    /// The score display: every ended round publishes its criteria (`last_*`) against the best,
    /// and the best (`best_*`) is the pick of [`Objective::better`] over the runs' picks, ties to
    /// the lower run, exactly what [`route_topo`] picks at the end.
    #[test]
    fn progress_carries_the_last_round_and_the_best_in_the_pick_order() {
        let ctl = RunControl::new();
        let obj = Objective::from_options("length_vias", 25.0, "errors_vias_length");
        let coord = Coord::new(&ctl, 2, obj, 1);
        let crit = |errors: usize, open: usize, vias: usize, length_mm: f64| Criteria { errors, open, violations: errors - open, vias, length_mm, shrinkages: 1, ..Criteria::default() };
        let round = |k: usize, cycle: usize, c: Criteria, best: Criteria, best_cycle: usize| {
            let j = Judged { crit: c, routed: 20 - c.open, total: 20, as_drawn: false };
            coord.ended(k, |r| r.best = Some(RunBest { crit: best, routed: 20 - best.open, total: 20, cycle: best_cycle }), Some((cycle, RoundEnd::Judged(j))));
            ctl.snapshot()
        };
        // Run 1, round 1: the first result is the best.
        let a = crit(3, 0, 166, 7380.0);
        let p = round(0, 1, a, a, 1);
        assert_eq!((p.last_run, p.last_round, p.last_errors, p.last_open, p.last_vias, p.last_shrinkages), (1, 1, 3, 0, 166, 1));
        assert_eq!(p.last_vs_best, "new best");
        assert_eq!((p.last_decider.as_str(), p.last_delta), ("", 0.0), "the first result has nothing to beat");
        assert_eq!((p.best_run, p.best_round, p.best_errors, p.best_open, p.best_vias, p.best_violations), (1, 1, 3, 0, 166, 3));
        // Run 2, round 1: fewer vias but more errors: worse, on errors (the first criterion).
        let b = crit(5, 0, 150, 7000.0);
        let p = round(1, 1, b, b, 1);
        assert_eq!((p.last_run, p.last_round, p.last_vs_best.as_str(), p.last_decider.as_str(), p.last_delta), (2, 1, "worse", "errors", 2.0));
        assert_eq!((p.best_run, p.best_round, p.best_errors), (1, 1, 3), "fewer vias do not outweigh errors");
        // Run 1, round 2: worse on vias; its run keeps round 1.
        let p = round(0, 2, crit(3, 0, 168, 7300.0), a, 1);
        assert_eq!((p.last_vs_best.as_str(), p.last_decider.as_str(), p.last_delta), ("worse", "vias", 2.0));
        assert_eq!((p.best_run, p.best_round), (1, 1));
        // Run 2, round 2: the same score as run 1's best: the lower run stays the best.
        let p = round(1, 2, a, a, 2);
        assert_eq!((p.last_vs_best.as_str(), p.last_decider.as_str()), ("same", ""));
        assert_eq!((p.best_run, p.best_round), (1, 1), "ties go to the lower run");
        // Run 2, round 3: shorter at the same errors, open and vias: the new best, against run 1.
        let c = crit(3, 0, 166, 7350.0);
        let p = round(1, 3, c, c, 3);
        assert_eq!((p.last_vs_best.as_str(), p.last_decider.as_str()), ("new best", "length"));
        assert!((p.last_delta + 30.0).abs() < 1e-9, "{}", p.last_delta);
        assert_eq!((p.best_run, p.best_round, p.best_length_mm), (2, 3, 7350.0));
        // A round not judged: a line, the last judged round stays on show.
        coord.ended(0, |r| r.done = true, Some((3, RoundEnd::NotJudged("cut short by the time limit or Cancel / OK"))));
        let p = ctl.snapshot();
        assert_eq!((p.last_run, p.last_round), (2, 3));
        assert_eq!(p.rounds_ended, 6);
        assert_eq!(p.round_lines.len(), 6);
        assert_eq!(p.round_lines[0], "run 1, round 1: errors 3, open 0, vias 166, length 7.380 m, shrinkages 1; rule violations 3, routed 20 of 20: new best (the first result)");
        assert!(p.round_lines[1].ends_with(": worse than the best (run 1, round 1): errors +2"), "{}", p.round_lines[1]);
        assert!(p.round_lines[2].ends_with(": worse than the best (run 1, round 1): vias +2"), "{}", p.round_lines[2]);
        assert!(p.round_lines[3].ends_with(": same score as the best (run 1, round 1): the earlier one stays"), "{}", p.round_lines[3]);
        assert!(p.round_lines[4].ends_with(": new best: length -0.030 m against run 1, round 1"), "{}", p.round_lines[4]);
        assert_eq!(p.round_lines[5], "run 1, round 3: not judged (cut short by the time limit or Cancel / OK)");
        // Only the last ROUND_LINES lines stay; the count goes on.
        for i in 0..crate::control::ROUND_LINES {
            coord.ended(1, |_| {}, Some((10 + i, RoundEnd::NotJudged("test"))));
        }
        let p = ctl.snapshot();
        assert_eq!((p.rounds_ended as usize, p.round_lines.len()), (6 + crate::control::ROUND_LINES, crate::control::ROUND_LINES));
        assert!(p.round_lines[0].starts_with("run 2, round 10:"), "{}", p.round_lines[0]);
        // Run 1, round 4: the same score as the best of run 2: run 1 is the best now (ties go to the
        // lower run, as in the final pick), which the verdict says.
        let p = round(0, 4, c, c, 4);
        assert_eq!((p.last_vs_best.as_str(), p.last_decider.as_str()), ("tie", ""));
        assert_eq!((p.best_run, p.best_round), (1, 4));
        assert!(p.round_lines.last().expect("line").ends_with(": the best now: the same score as run 2, round 3, and ties go to the lower run"), "{:?}", p.round_lines.last());
    }

    /// M2: the gate decides once, when no run can offer any more: the best `candidates` picks by
    /// the objective draw (ties: the lower run), after Cancel only the best one; a withdrawn run
    /// (not begun, nothing kept) is not waited for.
    #[test]
    fn the_draw_gate_draws_the_best_picks_over_all_runs() {
        let obj = Objective::from_options("length_vias", 25.0, "errors_vias_length");
        let crit = |errors: usize, vias: usize| Criteria { errors, vias, length_mm: 100.0, ..Criteria::default() };
        for (quota, cancel, want) in [(1usize, false, vec![2usize]), (2, false, vec![0, 2]), (2, true, vec![2]), (9, false, vec![0, 2, 3])] {
            let g = DrawGate::new(4, quota, 2);
            let offers = [(0usize, crit(0, 5)), (2, crit(0, 3)), (3, crit(1, 0))];
            let drawn: Vec<usize> = std::thread::scope(|s| {
                let hs: Vec<_> = offers.iter().map(|&(k, c)| { let g = &g; let obj = &obj; s.spawn(move || (k, g.offer(k, c, obj, cancel))) }).collect();
                std::thread::sleep(std::time::Duration::from_millis(20));
                g.withdraw(1); // run 2 never begun: the decision does not wait for it
                let mut d: Vec<usize> = hs.into_iter().map(|h| h.join().expect("offer")).filter(|(_, w)| w.is_none()).map(|(k, _)| k).collect();
                d.sort_unstable();
                d
            });
            assert_eq!(drawn, want, "quota {quota}, cancel {cancel}");
        }
    }

    #[test]
    fn a_poisoned_draw_gate_is_loud_never_draw_every_pick() {
        // verify-m2 F7c: a run that panicked while deciding poisons the gate; the others panic
        // (the call's thread scope passes it on) instead of each drawing its pick quietly
        let obj = Objective::from_options("length_vias", 25.0, "errors_vias_length");
        let g = DrawGate::new(2, 1, 2);
        let _ = std::thread::scope(|s| s.spawn(|| {
            let _held = g.st.lock().expect("fresh lock");
            panic!("a run panics while it decides");
        }).join());
        assert!(g.st.is_poisoned());
        g.withdraw(1); // from a Drop while unwinding: never panics
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| g.decide(0, Criteria::default(), &obj, false)));
        let msg = r.expect_err("a poisoned gate panics").downcast::<String>().map(|m| *m).unwrap_or_default();
        assert!(msg.contains("draw gate"), "{msg}");
    }

    /// TW3 condition 1: the distinct rows of the four bests go to their origin run when it is
    /// compatible, else to the least loaded compatible run; `quota` rows at most; no compatible
    /// run = undrawable.
    #[test]
    fn the_four_bests_rows_go_to_their_origin_run_first_and_quota_rows_at_most() {
        use std::collections::BTreeMap;
        let compat: BTreeMap<usize, u64> = [(0, 7), (1, 7), (2, 9)].into_iter().collect();
        // rows: origin 1 (compatible), origin 3 (ended, key 7), origin 3 (key 7), origin 0 (key 5: nobody)
        let rows = [(1usize, 7u64), (3, 7), (3, 7), (0, 5)];
        assert_eq!(super::assign_runs(&rows, &compat, 4), vec![Some(1), Some(0), Some(0), None]);
        // origin 2 (key 9) draws its own row; its second row has no other compatible run
        assert_eq!(super::assign_runs(&[(2, 9), (2, 9)], &compat, 4), vec![Some(2), Some(2)]);
        // two rows of origin 1: the second goes to the free compatible run 0 (parallel drawings)
        assert_eq!(super::assign_runs(&[(1, 7), (1, 7), (1, 7)], &compat, 4), vec![Some(1), Some(0), Some(0)]);
        // least loaded: run 0 gets the first foreign row, run 1 the next, then run 0 again
        assert_eq!(super::assign_runs(&[(5, 7), (5, 7), (5, 7)], &compat, 4), vec![Some(0), Some(1), Some(0)]);
        // the quota: candidates = 1 / Cancel draws the overall best only; 2 = the first two rows
        assert_eq!(super::assign_runs(&rows, &compat, 1), vec![Some(1)]);
        assert_eq!(super::assign_runs(&rows, &compat, 2).len(), 2);
    }
}

