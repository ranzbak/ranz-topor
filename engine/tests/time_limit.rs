//! The live wall-clock limit (`control::TimeLimit`, the plugin's progress window): raised while
//! the core runs, the run goes on past the start limit; lowered below the time spent, it stops at
//! the next check with the time limit's stop reason and says the limit was changed.

use std::time::{Duration, Instant};
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route_with;

fn eprom() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/eprom-to-kernal-ripped.json")).expect("fixture parses")
}

/// Never stable (a huge stable window) and no stop at stable: only the limit or Cancel ends it.
/// The whole-route cycles (`opt_loop = "cycles"`, the default before M0): they iterate until the
/// limit; TopoR's loop (the default since M0) ends by itself when its plan is stable.
fn opts(limit_s: f64) -> Options {
    Options::from_json(&format!(
        r#"{{"runs": 1, "wall_clock_s": {limit_s}, "stop_when_stable": false, "finish_when_stable": false, "topo_rounds": 100000, "stable_window": 100000, "threads": 2, "opt_loop": "cycles"}}"#
    ))
    .expect("options parse")
}

#[test]
fn a_raised_limit_lets_the_run_continue_and_a_lowered_one_stops_it() {
    let m = eprom();
    let o = opts(2.0);
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s); // as route_async does at the job's start
    let c2 = ctl.clone();
    let t = Instant::now();
    let h = std::thread::spawn(move || route_with(&m, &o, &c2));
    std::thread::sleep(Duration::from_millis(300));
    ctl.set_time_limit(300.0); // the user adds time
    std::thread::sleep(Duration::from_millis(3500));
    assert!(!h.is_finished(), "the run stopped at the start limit although it was raised");
    let spent = t.elapsed().as_secs_f64();
    ctl.set_time_limit(spent - 1.0); // lowered below the time spent: stop now
    let r = h.join().expect("run thread");
    let stop_s = t.elapsed().as_secs_f64() - spent;
    assert!(stop_s < 30.0, "stopped {stop_s:.1} s after the limit was lowered");
    assert_eq!(r.stop_reason, "timeout", "{:?}", r.warnings);
    assert!(r.timed_out);
    let hit = r.warnings.iter().find(|w| w.starts_with("time budget hit")).expect("time budget hit warning");
    assert!(hit.contains("changed during the run") && hit.contains("run.timeout_s = 2 s"), "{hit}");
    assert!(r.routed > 0, "the best result so far is kept");
}

#[test]
fn no_limit_set_during_the_run_needs_cancel() {
    let m = eprom();
    let o = opts(1.0);
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    ctl.set_time_limit(0.0); // "no limit" before the run reaches its first check
    let c2 = ctl.clone();
    let h = std::thread::spawn(move || route_with(&m, &o, &c2));
    std::thread::sleep(Duration::from_millis(2500));
    assert!(!h.is_finished(), "no limit: the 1 s start limit must not fire");
    ctl.request_cancel();
    let r = h.join().expect("run thread");
    assert_ne!(r.stop_reason, "timeout", "{:?}", r.warnings);
}
