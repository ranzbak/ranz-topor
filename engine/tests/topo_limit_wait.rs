//! `[run] limit_grace_s`: the soft time limit's wait for the TopoR loop step is bounded. The test hook
//! reaches the limit at the loop's first stop check inside its first cycle; with the grace at 0 the
//! cycle is cut at the next check and dropped, with no bound it finishes and is offered. Without a
//! limit the grace changes nothing (same plan, same copper).

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
    Options::from_json(&format!(r#"{{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "runs": 1, "topo_rounds": 3, "limit_grace_s": {grace}}}"#)).expect("options parse")
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
    assert!(l.loop_wait_s > 0.05, "{l:?}");
    assert!(r.warnings.iter().any(|w| w.starts_with("time limit wait:") && w.contains("limit_grace_s = 0 s ran out")), "{:?}", r.warnings);
    assert!(loop_line(&r).contains("1 variants offered"), "the cut cycle is not offered: {}", loop_line(&r));
    assert_eq!(l.cut, "none", "the grace is no hard cut: {l:?}");
    assert!(r.routed > 0, "the best plan so far (the initial one) is drawn");
}

#[test]
fn without_a_bound_the_cycle_finishes_and_is_offered() {
    let r = run(-1.0, true);
    let l = r.limit.as_ref().expect("limit report");
    assert!(l.reached && !l.grace_hit && l.grace_s.is_none() && l.loop_wait_s > 0.05, "{l:?}");
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
    assert!(a.limit.as_ref().is_some_and(|l| !l.reached && l.loop_wait_s == 0.0 && !l.grace_hit), "{:?}", a.limit);
    assert_eq!(untimed(&a), untimed(&b));
    assert_eq!(untimed(&a), untimed(&c));
    assert_eq!(a.items, b.items);
    assert_eq!(a.items, c.items);
}
