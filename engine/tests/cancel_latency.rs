//! cancel-fix (2026-11-10): Cancel / OK ends a run quickly at any time with the best result so
//! far, in every mode.

use std::time::{Duration, Instant};
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};

fn eprom() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/eprom-to-kernal-ripped.json")).expect("fixture parses")
}

/// The dialog's run (no stop at stable) that never ends by itself: only Cancel ends it.
fn opts(extra: &str) -> Options {
    Options::from_json(&format!(
        r#"{{"wall_clock_s": 600.0, "stop_when_stable": false, "finish_when_stable": false, "topo_rounds": 100000, "stable_window": 100000, "loop_stable_window": 100000{extra}}}"#
    ))
    .expect("options parse")
}

/// Cancel `o` after `after`; the result, the seconds from the Cancel to the end, and the control.
fn cancel_after(o: Options, after: Duration) -> (RouteResult, f64) {
    let m = eprom();
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let c2 = ctl.clone();
    let h = std::thread::spawn(move || route_with(&m, &o, &c2));
    std::thread::sleep(after);
    assert!(!h.is_finished(), "the run ended before the Cancel at {after:?}");
    let t = Instant::now();
    ctl.request_cancel();
    let r = h.join().expect("run thread");
    (r, t.elapsed().as_secs_f64())
}

/// Generous for a loaded test machine: the engine stops within a second or two on eprom.
const BOUND_S: f64 = 5.0;

#[test]
fn cancel_at_any_time_ends_quickly_with_the_best_so_far() {
    for ms in [30u64, 300, 1200, 2500] {
        let (r, s) = cancel_after(opts(""), Duration::from_millis(ms));
        assert!(s < BOUND_S, "Cancel at {ms} ms took {s:.2} s to end the run");
        assert_eq!(r.stop_reason, "cancel", "{:?}", r.warnings);
        if ms >= 1200 {
            assert!(r.routed > 0, "Cancel at {ms} ms: the best result so far is kept");
        }
    }
}

#[test]
fn cancel_in_the_topor_loop_draws_one_pick_quickly() {
    // the initial routing of eprom takes well under a second: at 1.5 s the five loops run
    let (r, s) = cancel_after(opts(r#", "opt_loop": "topor", "search_length": "taut""#), Duration::from_millis(1500));
    assert!(s < BOUND_S, "Cancel in the TopoR loop took {s:.2} s to end the run");
    assert_eq!(r.routed, r.connections_total, "the loop's pick is drawn completely: {:?}", r.warnings);
    let skipped = r.warnings.iter().filter(|w| w.contains("pick not drawn (another run")).count();
    assert!(skipped >= 1, "only one loop pick is drawn after Cancel: {:?}", r.warnings);
}

#[test]
fn cancel_before_a_run_begins_skips_it() {
    // more runs than threads: the runs not begun when Cancel comes have no result of their own
    let (r, s) = cancel_after(opts(r#", "runs": 64"#), Duration::from_millis(200));
    assert!(s < BOUND_S, "took {s:.2} s");
    let line = r.warnings.iter().find(|w| w.starts_with("best of 64 seeded runs")).expect("best-of line");
    assert!(line.contains("not begun") && line.contains("without a result of their own"), "{line}");
}
