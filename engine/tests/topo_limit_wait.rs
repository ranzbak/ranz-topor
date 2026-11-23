//! `[run] limit_grace_s`: the soft time limit's wait for the TopoR loop step is bounded. The test hook
//! reaches the limit at the loop's first stop check inside its first cycle; with the grace at 0 the
//! cycle is cut at the next check and dropped, with no bound it finishes and is offered. Without a
//! limit the grace changes nothing (same plan, same copper).
//! time-limit-fix r2: these tests check the time limit inside the final drawing, which only
//! `[run] timeout_scope = "all"` counts (the default "optimizing" keeps it off the clock), so
//! their options set it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};

/// The smallest fixture whose TopoR loop has cycles with moves (the qmtech crop with the cut model took
/// ~130 s a run in release).
fn crop() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/eprom-to-kernal-ripped.json")).expect("fixture parses")
}

fn topor(grace: f64) -> Options {
    Options::from_json(&format!(r#"{{"timeout_scope": "all", "wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "runs": 1, "topo_rounds": 3, "limit_grace_s": {grace}}}"#)).expect("options parse")
}

/// Runs with the limit reached at the loop's first move check (`reach` = false: no limit at all).
fn run(grace: f64, reach: bool) -> RouteResult {
    let o = topor(grace);
    let mut ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let (l2, once) = (ctl.limit.clone(), Arc::new(AtomicBool::new(false)));
    ctl.set_test_hook(move |p| {
        if reach && p == "loop move" && !once.swap(true, Ordering::SeqCst) {
            l2.reach();
            std::thread::sleep(Duration::from_millis(60)); // a measurable wait past the limit
        }
        false
    });
    route_with(&crop(), &o, &ctl)
}

fn loop_line(r: &RouteResult) -> &str {
    r.warnings.iter().find(|w| w.starts_with("TopoR loop (")).unwrap_or_else(|| panic!("loop line: {:?}", r.warnings))
}

#[test]
fn grace_zero_cuts_the_cycle_and_reports_the_wait() {
    let r = run(0.0, true);
    let l = r.limit.as_ref().expect("limit report");
    assert!(l.reached && l.grace_hit && l.grace_s == Some(0.0), "{l:?}");
    assert!(l.wait_s > 0.05, "{l:?}");
    assert!(r.warnings.iter().any(|w| w.starts_with("time limit wait:") && w.contains("limit_grace_s = 0 s ran out")), "{:?}", r.warnings);
    assert!(loop_line(&r).contains("1 variants offered"), "the cut cycle is not offered: {}", loop_line(&r));
    assert_eq!(l.cut, "none", "the grace is no hard cut: {l:?}");
    assert!(r.routed > 0, "the best plan so far (the initial one) is drawn");
}

#[test]
fn without_a_bound_the_cycle_finishes_and_is_offered() {
    let r = run(-1.0, true);
    let l = r.limit.as_ref().expect("limit report");
    assert!(l.reached && !l.grace_hit && l.grace_s.is_none() && l.wait_s > 0.05, "{l:?}");
    assert!(r.warnings.iter().any(|w| w.starts_with("time limit wait:") && w.contains("unbounded")), "{:?}", r.warnings);
    assert!(loop_line(&r).contains("2 variants offered"), "the cycle finished and was offered: {}", loop_line(&r));
}

#[test]
fn a_generous_grace_lets_the_cycle_finish_and_says_so() {
    let r = run(3600.0, true);
    let l = r.limit.as_ref().expect("limit report");
    assert!(l.reached && !l.grace_hit && l.grace_s == Some(3600.0), "{l:?}");
    assert!(r.warnings.iter().any(|w| w.starts_with("time limit wait:") && w.contains("finished within")), "{:?}", r.warnings);
}

/// What a run produced, without any timing or counter that depends on the clock.
fn untimed(r: &RouteResult) -> (usize, usize, String) {
    (r.routed, r.vias, loop_line(r).split(" cycles in ").next().unwrap_or("").to_string())
}

#[test]
fn without_a_limit_the_grace_changes_nothing() {
    let (a, b, c) = (run(30.0, false), run(0.0, false), run(-1.0, false));
    assert!(a.limit.as_ref().is_some_and(|l| !l.reached && l.wait_s == 0.0 && !l.grace_hit), "{:?}", a.limit);
    assert_eq!(untimed(&a), untimed(&b));
    assert_eq!(untimed(&a), untimed(&c));
    assert_eq!(a.items, b.items);
    assert_eq!(a.items, c.items);
}

/// Runs with the limit reached the first time the hook sees `at` (grace as given); `seen` collects the
/// hook points met after that.
fn run_at(grace: f64, at: &'static str, nap_ms: u64) -> (RouteResult, Vec<String>, Option<bool>) {
    let o = topor(grace);
    let mut ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let (l2, reached) = (ctl.limit.clone(), Arc::new(AtomicBool::new(false)));
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let scope = Arc::new(std::sync::Mutex::new(None::<bool>));
    let (s2, sc2) = (seen.clone(), scope.clone());
    ctl.set_test_hook(move |p| {
        if reached.load(Ordering::SeqCst) {
            s2.lock().unwrap().push(p.to_string());
            if p.starts_with("loop stage") {
                // the move-level stop test (stop_requested) sees the grace only inside the loop's scope
                sc2.lock().unwrap().get_or_insert(topor_engine::control::stop_requested());
            }
        } else if p == at {
            reached.store(true, Ordering::SeqCst);
            l2.reach();
            std::thread::sleep(Duration::from_millis(nap_ms));
        }
        false
    });
    let r = route_with(&crop(), &o, &ctl);
    let seen = seen.lock().unwrap().clone();
    let scope = *scope.lock().unwrap();
    (r, seen, scope)
}

/// The loop checks the stop test between its stages: a limit and a grace that ran out at a stage's
/// check ends the cycle there, the next stage never begins.
#[test]
fn each_stage_check_of_the_cycle_ends_it_when_the_grace_ran_out() {
    for i in 1..=3 {
        let (_, seen, _) = run_at(0.0, Box::leak(format!("loop stage {i} check").into_boxed_str()), 30);
        let next = format!("loop stage {} begin", i + 1);
        assert!(!seen.contains(&next), "stage {i}: the cycle went on into stage {}: {seen:?}", i + 1);
    }
}

/// Without the grace the same cycle runs through every stage (the markers exist and are reached).
#[test]
fn without_a_bound_the_cycle_goes_through_every_stage() {
    let (_, seen, _) = run_at(-1.0, "loop stage 1 check", 30);
    for i in 2..=4 {
        assert!(seen.contains(&format!("loop stage {i} begin")), "stage {i} not reached: {seen:?}");
    }
}

/// The loop's moves ask `stop_requested`, which answers the grace only inside `in_loop_scope`.
#[test]
fn the_loop_runs_inside_the_grace_scope() {
    let (_, _, scope) = run_at(0.0, "loop move", 30);
    assert_eq!(scope, Some(true), "stop_requested saw the grace over inside the loop step");
}

/// A limit that falls in the drawing is named so; the wait is that of the whole drawing (its via
/// reduction and pour stitching too, lead 2026-11-17), which is the run's last step: no later step
/// change ends it, so it is the time from the limit to the result.
#[test]
fn a_limit_in_the_drawing_counts_the_whole_drawing_and_is_not_called_a_loop_wait() {
    let (r, _, _) = run_at(30.0, "fail wire drawing", 60);
    let l = r.limit.as_ref().expect("limit report");
    assert!(l.reached && !l.grace_hit && !l.cycle_dropped && l.wait_s > 0.05, "{l:?}");
    assert!((l.wait_s - l.overrun_s).abs() < 0.005, "the drawing ran to the result: wait {} vs overrun {}", l.wait_s, l.overrun_s);
    let w = r.warnings.iter().find(|w| w.starts_with("time limit wait:")).unwrap_or_else(|| panic!("{:?}", r.warnings));
    assert!(!w.contains("TopoR loop on") && w.contains("(part of the drawing)") && w.contains("every wire was drawn") && !w.contains("finished within [run] limit_grace_s"), "{w}");
    assert!(w.contains("drawing_grace_s = 60 s"), "{w}");
}

/// Hook points: the limit is reached the first time `at` is seen (Cancel too when `cancel`), the
/// time of each point is kept (seconds on the limit's clock).
fn run_hooked(o: &Options, at: &'static str, cancel: bool, naps: &'static [(&'static str, u64)]) -> (RouteResult, Vec<(String, f64)>) {
    let model = crop(); // before the limit's clock starts: the run's clock and the limit's then agree
    let mut ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let (l2, c2, once) = (ctl.limit.clone(), ctl.cancel.clone(), Arc::new(AtomicBool::new(false)));
    let seen = Arc::new(std::sync::Mutex::new(Vec::<(String, f64)>::new()));
    let s2 = seen.clone();
    ctl.set_test_hook(move |p| {
        if p == at && !once.swap(true, Ordering::SeqCst) {
            l2.reach();
            if cancel {
                c2.store(true, Ordering::SeqCst);
            }
        }
        if let Some(&(_, ms)) = naps.iter().find(|(n, _)| *n == p) {
            std::thread::sleep(Duration::from_millis(ms));
        }
        s2.lock().unwrap().push((p.to_string(), l2.elapsed_s()));
        false
    });
    let r = route_with(&model, o, &ctl);
    let seen = seen.lock().unwrap().clone();
    (r, seen)
}

/// The TopoR loop step ends where the loop returns, not at the drawing's first stage (the pick and
/// the cut necks between belong to the drawing): a nap after the loop's end is no wait of the loop.
#[test]
fn the_loop_step_ends_where_the_loop_returns() {
    let (r, seen) = run_hooked(&topor(-1.0), "loop move", false, &[("loop ended", 200)]);
    let l = r.limit.as_ref().expect("limit report");
    let t_lim = l.limit_s;
    let t_end = seen.iter().find(|(p, _)| p == "loop ended").map(|x| x.1).expect("loop ended seen") - 0.2;
    assert!(l.reached && l.step.contains("TopoR loop"), "{l:?}");
    assert!((l.wait_s - (t_end - t_lim)).abs() < 0.02, "wait {} vs loop end - limit {}", l.wait_s, t_end - t_lim);
}

/// The drawing, the run's last step, ends with the copper handed back: the finishing after the core
/// (edge trims, the Cleanup-safe tidy) is part of its wait and of the run time.
#[test]
fn the_last_step_counts_to_the_finished_copper() {
    let (r, seen) = run_hooked(&topor(30.0), "fail wire drawing", false, &[("finish copper", 300)]);
    let l = r.limit.as_ref().expect("limit report");
    // the hook notes "finish copper" after its nap: the wait reaches at least that moment
    let t_done = seen.iter().find(|(p, _)| p == "finish copper").map(|s| s.1).expect("finish copper reached");
    assert!(l.reached && l.limit_s + l.wait_s >= t_done - 0.03, "the finishing nap is in the wait: done {t_done:.3} s, {l:?}");
    assert!((l.wait_s - l.overrun_s).abs() < 0.005, "wait {} vs overrun {}", l.wait_s, l.overrun_s);
}

/// A Cancel that ends the loop's cycle is no cycle the limit dropped.
#[test]
fn a_cancel_in_the_loop_is_no_dropped_cycle() {
    let (r, _) = run_hooked(&topor(-1.0), "loop move", true, &[]);
    let l = r.limit.as_ref().expect("limit report");
    assert!(l.reached && !l.cycle_dropped && !l.grace_hit, "{l:?}");
}

fn hard() -> Options {
    Options::from_json(r#"{"timeout_scope": "all", "wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "runs": 1, "topo_rounds": 3, "timeout_mode": "hard"}"#).expect("options parse")
}

/// Hard mode: the line is there too, and says the step was cut (not that it ran to its end).
#[test]
fn in_hard_mode_the_wait_line_says_the_step_was_cut() {
    let (r, _) = run_hooked(&hard(), "loop move", false, &[("loop move", 0)]);
    let l = r.limit.as_ref().expect("limit report");
    assert!(l.reached && l.mode == "hard" && l.cycle_dropped, "{l:?}");
    let w = r.warnings.iter().find(|w| w.starts_with("time limit wait:")).unwrap_or_else(|| panic!("{:?}", r.warnings));
    assert!(w.contains("[run] timeout_mode = hard") && w.contains("was dropped") && w.contains("that step stopped after"), "{w}");
    assert!(!w.contains("ran to its end") && !w.contains("limit_grace_s"), "{w}");
    assert!(l.draw_grace_s.is_none() && !l.draw_grace_hit, "the drawing bound is the soft limit's only: {l:?}");
    // each wire naps past the round's 1 s floor (the hard cut's minimum), so the drawing is cut
    let (r, _) = run_hooked(&hard(), "fail wire drawing", false, &[("fail wire drawing", 400)]);
    let l = r.limit.as_ref().expect("limit report");
    let w = r.warnings.iter().find(|w| w.starts_with("time limit wait:")).unwrap_or_else(|| panic!("{:?}", r.warnings));
    assert!(l.cut == "drawing" && w.contains("the limit cut this step") && w.contains("the drawing of the traces") && !w.contains("ran to its end"), "{l:?} {w}");
}

/// A dropped cycle is always reported, whatever the wait rounds to.
#[test]
fn a_dropped_cycle_is_reported_even_with_no_wait() {
    let (r, _, _) = run_at(0.0, "loop move", 0);
    let l = r.limit.as_ref().expect("limit report");
    assert!(l.cycle_dropped && l.grace_hit, "{l:?}");
    assert!(r.warnings.iter().any(|w| w.starts_with("time limit wait:") && w.contains("was dropped")), "{:?}", r.warnings);
}
