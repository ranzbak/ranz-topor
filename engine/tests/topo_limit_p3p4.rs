//! limit-p3p4 (F10, private board P from scratch: the soft time limit was not kept, the layer
//! assignment and rip-up of the initial plan (P3/P4) ran for more than 20 minutes past a 600 s limit, and Accept gave nothing to
//! draw). On a generated dense board whose P3/P4 take seconds (far longer than its P2):
//! - the soft limit plus `[run] limit_grace_s` ends P3/P4 at their next check, also inside a
//!   capacity refinement, and the plan so far is drawn in full;
//! - Accept (a stop request) during P3/P4, also in the middle of a capacity refinement, draws the
//!   plan so far (never NOTHING_TO_DRAW once P2 placed every connection);
//! - a stop before P2 ends still keeps no plan (the loud NOTHING_TO_DRAW).
//!
//! TopoR 6.3 manual p. 73: the first variant is saved "after the first optimization cycle has been
//! completed or 20 seconds after the start of the optimization"; p. 65: stop at any time.

use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};

const MM: i64 = 1_000_000;

/// A dense 2-layer board: `n` two-pad nets on a 1 mm grid (pads d 0.6 mm), each pad paired with a
/// random free cell within 5 cells; every second net ends on a B.Cu SMD pad (a via each). The
/// pairs cross a lot, so P4's crossing rules and rip-up rounds take seconds while P2 takes well
/// under one (release; the same model as `tests/scaling/synth.py` with a denser grid).
fn dense(n: usize, seed: u64) -> BoardModel {
    let cols = ((2 * n) as f64).sqrt() as i64 + 1;
    let cells: Vec<(i64, i64)> = (0..2 * n as i64).map(|i| (i % cols, i / cols)).collect();
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut order: Vec<usize> = (0..cells.len()).collect();
    order.shuffle(&mut rng);
    let mut free: std::collections::BTreeSet<usize> = order.iter().copied().collect();
    let mut pairs = Vec::new();
    for &i in &order {
        if !free.remove(&i) {
            continue;
        }
        let near: Vec<usize> = free.iter().copied().filter(|&j| (((cells[i].0 - cells[j].0).pow(2) + (cells[i].1 - cells[j].1).pow(2)) as f64).sqrt() <= 5.0).collect();
        if near.is_empty() {
            continue;
        }
        let j = near[rng.gen_range(0..near.len())];
        free.remove(&j);
        pairs.push((i, j));
    }
    let mut pads = Vec::new();
    for (k, &(a, b)) in pairs.iter().enumerate() {
        let net = k + 1;
        for (num, c) in [(1, a), (2, b)] {
            let lay = if num == 2 && net % 2 == 0 { 31 } else { 0 };
            pads.push(serde_json::json!({"uuid": format!("p{net}-{num}"), "ref": format!("P{net}"), "number": num.to_string(), "net": net, "attr": "smd",
                "center": [cells[c].0 * MM, cells[c].1 * MM], "rotation_deg": 0.0, "layers": [lay],
                "shapes": {lay.to_string(): {"kind": "circle", "d": 600_000}}, "drill": null, "local_clearance": null, "net_tie_group": null}));
        }
    }
    let (lo, hi) = (-2 * MM, (cols - 1) * MM + 2 * MM);
    let hy = (2 * n as i64 / cols) * MM + 2 * MM;
    let cs = [(lo, lo), (hi, lo), (hi, hy), (lo, hy)];
    let outline: Vec<_> = (0..4).map(|k| serde_json::json!({"kind": "seg", "a": [cs[k].0, cs[k].1], "b": [cs[(k + 1) % 4].0, cs[(k + 1) % 4].1]})).collect();
    let rule = serde_json::json!({"clearance": 150_000, "width": 150_000, "via_d": 500_000, "via_drill": 250_000, "netclass": "Default", "width_source": "default"});
    let m = serde_json::json!({"units": "nm", "board": format!("dense-{n}-{seed}"),
        "layers": [{"id": 0, "name": "F.Cu", "kind": "signal", "routable": true}, {"id": 31, "name": "B.Cu", "kind": "signal", "routable": true}],
        "nets": (1..=pairs.len()).map(|c| serde_json::json!({"code": c, "name": format!("N{c}")})).collect::<Vec<_>>(),
        "pads": pads, "vias": [], "kept": [], "copper_shapes": [], "holes": [], "outline": outline, "keepouts": [], "net_ties": [], "dangling_ends": [],
        "rules": {"min_clearance": 150_000, "min_track_width": 150_000, "min_via_diameter": 500_000, "min_via_drill": 250_000, "min_annular": 125_000,
            "hole_clearance": 250_000, "hole_to_hole": 250_000, "edge_clearance": 250_000, "max_error": 1500, "default": rule, "per_net": {}}});
    BoardModel::from_json(&m.to_string()).expect("dense model parses")
}

/// The dialog's run: no stop at stable, no overrun cap (the KiCad dialog's `auto`), one run.
fn opts(grace: f64, limit_s: f64) -> Options {
    Options::from_json(&format!(
        r#"{{"wall_clock_s": {limit_s}, "limit_grace_s": {grace}, "timeout_overrun_s": "none", "stop_when_stable": false, "finish_when_stable": false, "runs": 1}}"#
    ))
    .expect("options parse")
}

/// [`opts`] with the capacity refinement on one lane (`[optimize] refine_lanes = 1`): all its
/// candidates are checked on the run's thread, so its checks per refinement repeat run to run.
fn opts_one_lane(grace: f64, limit_s: f64) -> Options {
    let mut o = opts(grace, limit_s);
    o.refine_lanes = 1;
    o
}

/// Debug builds are several times slower (scale bounds with the build profile).
const SLACK_S: f64 = if cfg!(debug_assertions) { 20.0 } else { 1.5 };

/// What the hook does at its point.
#[derive(Clone, Copy, PartialEq)]
enum Act {
    /// reach the soft limit (the plugin's CPU clock path; the wall clock alike)
    Reach,
    /// Accept / Cancel (a stop request)
    Stop,
}

/// Run `m` with `act` at the `nth` call of the test point `at`; the result and how many times `at`
/// was asked in all.
fn run_at(m: &BoardModel, o: &Options, at: &'static str, nth: usize, act: Act) -> (RouteResult, usize) {
    let mut ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let (c2, calls, done) = (ctl.clone(), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
    let (calls2, done2) = (calls.clone(), done.clone());
    ctl.set_test_hook(move |p| {
        if p == at && calls2.fetch_add(1, Ordering::SeqCst) + 1 == nth && !done2.swap(true, Ordering::SeqCst) {
            match act {
                Act::Reach => c2.limit.reach(),
                Act::Stop => c2.request_stop(Some(false)),
            }
        }
        false
    });
    let r = route_with(m, o, &ctl);
    assert!(done.load(Ordering::SeqCst), "the test point {at:?} was asked only {} times (wanted {nth})", calls.load(Ordering::SeqCst));
    (r, calls.load(Ordering::SeqCst))
}

fn kept_all_placed(r: &RouteResult, what: &str) {
    assert!(!r.warnings.iter().any(|w| w.starts_with("ERROR: nothing to draw yet")), "{what}: a plan exists after P2: {:?}", &r.warnings[..r.warnings.len().min(4)]);
    assert!(!r.items.is_empty(), "{what}: copper is drawn");
    assert_eq!(r.routed, r.connections_total, "{what}: the plan so far is drawn in full: {:?}", r.connections.iter().filter(|c| c.status != "routed").map(|c| &c.reason).collect::<Vec<_>>());
}

fn optimize_overrun(r: &RouteResult) -> f64 {
    r.limit.as_ref().expect("limit report").overrun_s
}

#[test]
fn the_soft_limit_in_p3_p4_is_kept_and_the_plan_so_far_is_drawn() {
    // The limit is reached the moment P2 placed every connection; with limit_grace_s = 0 P3/P4
    // stop at their next check. Main ran P3/P4 to their end: 4-5 s past the limit here (release),
    // > 1000 s on private board P.
    let m = dense(90, 1);
    let (r, _) = run_at(&m, &opts(0.0, 86400.0), "plan ready", 1, Act::Reach);
    kept_all_placed(&r, "limit in P3/P4");
    assert!(r.warnings.iter().any(|w| w.starts_with("initial plan: the time limit plus its grace passed") && w.contains("limit_grace_s = 0 s")), "{:?}", r.warnings);
    let l = r.limit.as_ref().expect("limit report");
    assert!(l.reached && l.grace_hit && l.cut == "none", "the grace is no hard cut: {l:?}");
    assert!(l.step.contains("layer assignment"), "{l:?}");
    assert!(optimize_overrun(&r) < SLACK_S, "P3/P4 ended {:.2} s past the limit (bound {SLACK_S} s)", optimize_overrun(&r));
    assert!(r.warnings.iter().any(|w| w.starts_with("time limit wait:") && w.contains("(P3/P4) stopped at their next check")), "{:?}", r.warnings);
    assert!(r.timed_out && r.stop_reason.starts_with("timeout"), "{} {}", r.timed_out, r.stop_reason);
}

/// The move checks of each capacity refinement of P3/P4 (from "plan ready" to the TopoR loop's
/// first check; a refinement = its "refine start" and its "refine move" checks) of a run of `m`;
/// with `reach = Some((k, j))` the soft limit is reached at the `j`th check of the `k`th of them,
/// else when the TopoR loop begins (it then ends at once: `opts(0.0, ..)`, no stop at stable).
/// Returns the result and the checks per refinement.
fn refinements(m: &BoardModel, o: &Options, reach: Option<(usize, usize)>) -> (RouteResult, Vec<usize>) {
    let mut ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let c2 = ctl.clone();
    // (in P3/P4 now, the refinement that runs counts, checks per refinement)
    let seen = Arc::new(std::sync::Mutex::new((false, false, Vec::<usize>::new())));
    let seen2 = seen.clone();
    ctl.set_test_hook(move |p| {
        let mut g = seen2.lock().unwrap();
        let (plan, counts, v) = &mut *g;
        match p {
            "plan ready" => *plan = true,
            "loop move" | "loop stage 1 check" | "loop ended" => {
                // P3/P4 are over: end the run (no stop at stable; the loop stops at the limit)
                *plan = false;
                c2.limit.reach();
            }
            "refine start" => {
                *counts = *plan;
                if *plan {
                    v.push(0);
                }
            }
            "refine move" if *counts => {
                let k = v.len().checked_sub(1).expect("a refinement began");
                v[k] += 1;
                if reach == Some((k, v[k])) {
                    c2.limit.reach();
                }
            }
            _ => {}
        }
        false
    });
    let r = route_with(m, o, &ctl);
    let v = seen.lock().unwrap().2.clone();
    (r, v)
}

#[test]
fn the_soft_limit_inside_a_capacity_refinement_keeps_its_layering_so_far() {
    // The limit is reached in the middle of the longest capacity refinement of P3/P4:
    // with limit_grace_s = 0 that refinement stops at its next check (its moves so far are kept),
    // every later refinement at its first check, and the run draws that plan in full.
    // One lane: the checks per refinement repeat between the free run and the cut run (on
    // several lanes the run thread's share of the candidates varies with timing).
    let m = dense(90, 1);
    let o = opts_one_lane(0.0, 86400.0);
    let (_, free) = refinements(&m, &o, None);
    let (k, &long) = free.iter().enumerate().max_by_key(|&(i, c)| (*c, std::cmp::Reverse(i))).expect("the run refines");
    assert!(long >= 10, "a refinement with enough checks to stop in its middle: {free:?}");
    let at = long / 2;
    let (r, cut) = refinements(&m, &o, Some((k, at)));
    assert_eq!(cut[..k], free[..k], "the run up to the cut refinement repeats");
    kept_all_placed(&r, "limit in a refinement");
    assert!(cut.len() > k && cut[k] <= at + 2, "the refinement went on past the limit: {} checks of {long}, limit at {at} ({cut:?})", cut[k]);
    assert!(cut[k + 1..].iter().all(|&c| c <= 2), "a later refinement went on past the limit: {cut:?} (free {free:?})");
    assert!(optimize_overrun(&r) < SLACK_S, "P3/P4 ended {:.2} s past the limit", optimize_overrun(&r));
    assert!(r.warnings.iter().any(|w| w.starts_with("initial plan: the time limit plus its grace passed")), "{:?}", r.warnings);
    eprintln!("refinement checks: free {free:?}, limit at check {at} of refinement {k}: {cut:?}");
}

#[test]
fn accept_inside_a_capacity_refinement_draws_the_plan_so_far() {
    // Accept (a stop request) at the 20th move check of the first capacity refinement: before
    // limit-p3p4 the run kept no plan (NOTHING_TO_DRAW); now the layering as it stands is drawn.
    let m = dense(90, 1);
    let t = Instant::now();
    let (r, calls) = run_at(&m, &opts(30.0, 0.0), "refine move", 20, Act::Stop);
    kept_all_placed(&r, "Accept in a refinement");
    assert!(calls < 20 + 50, "the refinements stopped after the Accept: {calls} move checks in all");
    assert!(r.warnings.iter().any(|w| w.starts_with("initial plan: Cancel / OK came during the layer assignment")), "{:?}", r.warnings);
    assert_eq!(r.stop_reason, "cancel", "{:?}", r.warnings);
    eprintln!("Accept in P3: run {:.2} s", t.elapsed().as_secs_f64());
}

#[test]
fn accept_when_the_first_plan_is_ready_draws_it() {
    // The progress names the first plan (the dialog shows it) and Accept there draws it.
    let m = dense(90, 1);
    let (r, _) = run_at(&m, &opts(30.0, 0.0), "plan ready", 1, Act::Stop);
    kept_all_placed(&r, "Accept at plan ready");
}

#[test]
fn the_progress_says_when_the_first_plan_is_ready() {
    let m = dense(90, 1);
    let o = opts(30.0, 0.0);
    let mut ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let (c2, seen) = (ctl.clone(), Arc::new(std::sync::Mutex::new(String::new())));
    let seen2 = seen.clone();
    ctl.set_test_hook(move |p| {
        if p == "plan ready" {
            *seen2.lock().unwrap() = c2.snapshot().stage;
            c2.request_stop(Some(false));
        }
        false
    });
    let _ = route_with(&m, &o, &ctl);
    let s = seen.lock().unwrap().clone();
    assert!(s.contains("first plan ready, Accept will draw it") && s.starts_with("layer assignment"), "{s:?}");
}

#[test]
fn a_stop_before_p2_ends_keeps_no_plan() {
    // There is no first plan before P2 placed every connection: still the loud error.
    let m = dense(90, 1);
    let o = opts(30.0, 0.0);
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let c2 = ctl.clone();
    let m2 = m.clone();
    let h = std::thread::spawn(move || route_with(&m2, &o, &c2));
    let t0 = Instant::now();
    while !ctl.snapshot().stage.contains("topological search") && !h.is_finished() {
        assert!(t0.elapsed().as_secs() < 120, "never reached P2");
        std::thread::sleep(Duration::from_millis(1));
    }
    ctl.request_stop(Some(false));
    let r = h.join().expect("run thread");
    if !r.items.is_empty() {
        // P2 ended before the stop was seen (a loaded machine): then the plan is drawn, never a half one.
        kept_all_placed(&r, "stop just after P2");
    } else {
        assert!(r.warnings.first().is_some_and(|w| w.starts_with("ERROR: nothing to draw yet")) && r.items.is_empty(), "{:?}", r.warnings);
    }
}

#[test]
fn whole_route_cycles_keep_the_first_plan_and_drop_a_later_cut_one() {
    // `[optimize] loop = "cycles"`: a stop in the first cycle's P3/P4 draws that plan in full (no loop
    // draws it through); a stop in a later cycle's P3/P4 drops that cycle (its earlier cycles are the
    // best so far, as before limit-p3p4).
    let m = dense(40, 2);
    let o = Options::from_json(r#"{"wall_clock_s": 0.0, "stop_when_stable": false, "finish_when_stable": false, "runs": 1, "opt_loop": "cycles", "topo_rounds": 3}"#).expect("options");
    let (r1, _) = run_at(&m, &o, "plan ready", 1, Act::Stop);
    kept_all_placed(&r1, "cycles, stop in cycle 1");
    assert!(r1.warnings.iter().any(|w| w.starts_with("initial plan: Cancel / OK came during the layer assignment")), "{:?}", r1.warnings);
    let (r2, _) = run_at(&m, &o, "plan ready", 2, Act::Stop);
    kept_all_placed(&r2, "cycles, stop in cycle 2");
    assert!(r2.warnings.iter().any(|w| w.contains("of a later whole-route cycle; the earlier cycles' variants are kept")), "{:?}", r2.warnings);
}

/// Debug builds are several times slower: the bound for a discarded run to end.
const DISCARD_S: f64 = if cfg!(debug_assertions) { 30.0 } else { 3.0 };

/// Run `m` and press the dialog's Cancel (`RunControl::request_discard`) at the `nth` call of the
/// test point `at`: the result, the seconds from the Cancel to the end of the call and the test
/// points asked after it.
fn discard_at(m: &BoardModel, o: &Options, at: &'static str, nth: usize) -> (RouteResult, f64, Vec<String>) {
    let mut ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let c2 = ctl.clone();
    let (calls, when) = (Arc::new(AtomicUsize::new(0)), Arc::new(std::sync::Mutex::new(None::<Instant>)));
    let (calls2, when2) = (calls.clone(), when.clone());
    let after = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let after2 = after.clone();
    ctl.set_test_hook(move |p| {
        let mut w = when2.lock().unwrap();
        if w.is_some() {
            after2.lock().unwrap().push(p.to_string());
        } else if p == at && calls2.fetch_add(1, Ordering::SeqCst) + 1 == nth {
            *w = Some(Instant::now());
            c2.request_discard();
        }
        false
    });
    let r = route_with(m, o, &ctl);
    let t = when.lock().unwrap().unwrap_or_else(|| panic!("the test point {at:?} was asked only {} times (wanted {nth})", calls.load(Ordering::SeqCst)));
    let asked = after.lock().unwrap().clone();
    (r, t.elapsed().as_secs_f64(), asked)
}

/// The steps a discarded run must not begin after its Cancel: the drawing (its first test point)
/// and the via reduction after it.
fn began_after(asked: &[String]) -> Vec<&String> {
    asked.iter().filter(|p| *p == "no sequential run" || *p == "via reduction begin").collect()
}

fn nothing_taken(r: &RouteResult, what: &str) {
    assert!(r.items.is_empty(), "{what}: Cancel takes no results, yet {} items were drawn", r.items.len());
    assert_eq!(r.routed, 0, "{what}: nothing is drawn");
    assert!(r.warnings.first().is_some_and(|w| w.starts_with("Cancel: the run was discarded")), "{what}: {:?}", &r.warnings[..r.warnings.len().min(4)]);
    assert!(!r.warnings.iter().any(|w| w.starts_with("ERROR:")), "{what}: a discard is no error: {:?}", &r.warnings[..r.warnings.len().min(4)]);
    assert_eq!(r.stop_reason, "cancel");
}

#[test]
fn cancel_in_mid_refinement_ends_the_engine_at_once_and_draws_nothing() {
    // Lead 2026-11-23 (user rule: "Cancel stops there and then and takes no results"): the
    // dialog's Cancel in the middle of a capacity refinement (P3) ends the call within seconds,
    // with no drawing (Accept there draws the plan so far, the test above).
    let m = dense(90, 1);
    let (r, after, asked) = discard_at(&m, &opts(30.0, 0.0), "refine move", 20);
    nothing_taken(&r, "Cancel in P3");
    assert!(began_after(&asked).is_empty(), "after the Cancel the run began {:?}", began_after(&asked));
    assert!(after < DISCARD_S, "the engine ran {after:.2} s after the Cancel (bound {DISCARD_S} s)");
    eprintln!("Cancel in P3: the call ended {after:.2} s after it");
}

#[test]
fn cancel_while_the_traces_are_drawn_ends_the_drawing_and_takes_nothing() {
    // The dialog's Cancel once the drawing began ("no sequential run" is asked as it begins):
    // the drawing stops at its next check, no via reduction or stitching follows, nothing is taken.
    let m = dense(90, 1);
    let o = opts(30.0, 0.0);
    let mut ctl = RunControl::new();
    ctl.limit.init(0.0);
    let c2 = ctl.clone();
    let when = Arc::new(std::sync::Mutex::new(None::<Instant>));
    let when2 = when.clone();
    let asked = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let asked2 = asked.clone();
    ctl.set_test_hook(move |p| {
        let mut w = when2.lock().unwrap();
        if w.is_some() {
            asked2.lock().unwrap().push(p.to_string());
        }
        match p {
            // end the TopoR loop at once (no stop at stable): Accept at the first plan
            "plan ready" => c2.request_stop(Some(false)),
            "no sequential run" if w.is_none() => {
                *w = Some(Instant::now());
                c2.request_discard();
            }
            _ => {}
        }
        false
    });
    let r = route_with(&m, &o, &ctl);
    let t = when.lock().unwrap().expect("the drawing began");
    let after = t.elapsed().as_secs_f64();
    nothing_taken(&r, "Cancel in the drawing");
    let asked = asked.lock().unwrap().clone();
    assert!(!asked.iter().any(|p| p == "via reduction begin"), "after the Cancel the via reduction began: {asked:?}");
    assert!(after < DISCARD_S, "the engine ran {after:.2} s after the Cancel (bound {DISCARD_S} s)");
    eprintln!("Cancel in the drawing: the call ended {after:.2} s after it");
}
