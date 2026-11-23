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

/// What a Cancel waits for before its delay starts (M0: the TopoR method's routing, and with the
/// cut model the projection's cut room before any run begins, take seconds in a debug build, so a delay from the call's start would often cancel an earlier
/// phase than the test means; `cancel_during_the_projection_ends_quickly` covers the projection).
#[derive(Clone, Copy, PartialEq)]
enum After {
    /// the first run began routing (progress phase "route")
    Routing,
    /// a run began its TopoR loop (progress stage "TopoR loop ..."; a Cancel from there on draws
    /// the loop's pick; before it, during P3/P4, it draws the plan so far, limit-p3p4; during P2
    /// it leaves no copper and says so loudly, M2.TL "nothing to draw yet")
    Loop,
}

/// Cancel `o` `after` the state `from` is reached; the result and the seconds from the Cancel
/// to the end.
fn cancel_after(o: Options, from: After, after: Duration) -> (RouteResult, f64) {
    let m = eprom();
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let c2 = ctl.clone();
    let h = std::thread::spawn(move || route_with(&m, &o, &c2));
    let t0 = Instant::now();
    let reached = |p: &topor_engine::control::Progress| match from {
        After::Routing => p.phase == "route",
        After::Loop => p.stage.contains("TopoR loop"),
    };
    while !reached(&ctl.snapshot()) && !h.is_finished() {
        assert!(t0.elapsed().as_secs() < 300, "never reached the state to cancel in");
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(after);
    assert!(!h.is_finished(), "the run ended before the Cancel at {after:?}");
    let t = Instant::now();
    ctl.request_cancel();
    let r = h.join().expect("run thread");
    (r, t.elapsed().as_secs_f64())
}

/// Generous for a loaded test machine: the engine stops within a second or two on eprom.
/// Debug builds are several times slower (user 2026-11-14: scale bounds with the build profile).
const BOUND_S: f64 = if cfg!(debug_assertions) { 60.0 } else { 5.0 };

#[test]
fn cancel_at_any_time_ends_quickly_with_the_best_so_far() {
    for ms in [30u64, 300, 1200, 2500] {
        let (r, s) = cancel_after(opts(""), if ms >= 1200 { After::Loop } else { After::Routing }, Duration::from_millis(ms));
        assert!(s < BOUND_S, "Cancel at {ms} ms took {s:.2} s to end the run");
        assert_eq!(r.stop_reason, "cancel", "{:?}", r.warnings);
        if ms >= 1200 {
            assert!(r.routed > 0, "Cancel at {ms} ms: the best result so far is kept");
        }
    }
}

#[test]
fn cancel_in_the_topor_loop_draws_one_pick_quickly() {
    // 1.5 s after a run began its loop (M0: with the cut model P3/P4 takes seconds in a debug build)
    let (r, s) = cancel_after(opts(r#", "opt_loop": "topor", "search_length": "taut""#), After::Loop, Duration::from_millis(1500));
    assert!(s < BOUND_S, "Cancel in the TopoR loop took {s:.2} s to end the run");
    assert_eq!(r.routed, r.connections_total, "the loop's pick is drawn completely: {:?}", r.warnings);
    let skipped = r.warnings.iter().filter(|w| w.contains("pick not drawn (another run")).count();
    assert!(skipped >= 1, "only one loop pick is drawn after Cancel: {:?}", r.warnings);
}

#[test]
fn cancel_during_the_projection_ends_quickly() {
    // Cancel at once, before the projection (with the cut model's cut room) is done: the call
    // ends quickly as a Cancel, with no copper and the reason in the warnings.
    let m = eprom();
    let o = opts("");
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    ctl.request_cancel();
    let t = Instant::now();
    let r = route_with(&m, &o, &ctl);
    assert!(t.elapsed().as_secs_f64() < BOUND_S, "took {:.2} s", t.elapsed().as_secs_f64());
    assert_eq!(r.stop_reason, "cancel", "{:?}", r.warnings);
}

#[test]
fn cancel_before_a_run_begins_skips_it() {
    // more runs than threads: the runs not begun when Cancel comes have no result of their own
    let (r, s) = cancel_after(opts(r#", "runs": 64"#), After::Routing, Duration::from_millis(200));
    assert!(s < BOUND_S, "took {s:.2} s");
    // Some run has a result by the Cancel: the best-of line names the runs not begun. None has
    // (debug build, or a release one under VM load, PK-DBG / verifier radii-m3 round i: the first
    // runs are still in the initial routing 200 ms in, before any plan): the same facts (runs
    // 9..64 "not begun", none run) are in the loud "nothing to draw yet" line instead.
    let line = r.warnings.iter().find(|w| w.starts_with("best of 64 seeded runs") || w.starts_with("ERROR: nothing to draw yet"))
        .unwrap_or_else(|| panic!("best-of line: {} {:?}", r.stop_reason, r.warnings));
    if line.starts_with("best of") {
        assert!(line.contains("not begun") && line.contains("without a result of their own"), "{line}");
    } else {
        assert!(line.contains("run 64: not begun") && !line.contains("run 64: 1:"), "{line}");
    }
}

/// M0 stop latency (verifier 2026-11-13): with the cut model (`[capacity] model = "cuts"`) a Cancel / the
/// time limit during P3/P4 (layer assignment, rip-up and the topology and label moves, where the
/// cut prices are computed per move) ends the call within seconds, on crops of real boards: a 4-layer
/// one (qmtech-minimig) and a 2-layer one (esp-rs/esp-rust-board, CERN-OHL-P-2.0: USB-C, ESD diodes,
/// charger; `source` in the fixture).
#[test]
fn cancel_in_p3_p4_with_the_cut_model_ends_quickly() {
    for fx in ["qmtech-usb-crop.json", "esp-rust-board-usb-crop.json"] {
        let path = format!("{}/tests/fixtures/{fx}", env!("CARGO_MANIFEST_DIR"));
        let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).expect("fixture parses");
        let o = opts(r#", "runs": 1, "capacity_model": "cuts""#);
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        let c2 = ctl.clone();
        let h = std::thread::spawn(move || route_with(&m, &o, &c2));
        let t0 = Instant::now();
        while !ctl.snapshot().stage.contains("layer assignment") && !h.is_finished() {
            assert!(t0.elapsed().as_secs() < 600, "{fx}: never reached P3");
            std::thread::sleep(Duration::from_millis(2));
        }
        std::thread::sleep(Duration::from_millis(300));
        let t = Instant::now();
        ctl.request_cancel();
        // limit-p3p4: the stop ends the planning within a move; the plan so far is then drawn (not
        // counted here: the drawing of a kept plan runs to its end, as the TopoR loop's pick does).
        while !(h.is_finished() || ["picking", "drawing", "via reduction", "pour stitching"].iter().any(|k| ctl.snapshot().stage.contains(k))) {
            std::thread::sleep(Duration::from_millis(2));
        }
        let planned_s = t.elapsed().as_secs_f64();
        let r = h.join().expect("run thread");
        let s = planned_s;
        eprintln!("{fx}: stopped {s:.2} s after the Cancel ({} s into the call)", t0.elapsed().as_secs_f64() - s);
        assert!(s < BOUND_S, "{fx}: Cancel in P3/P4 took {s:.2} s to end the planning");
        assert_eq!(r.stop_reason, "cancel", "{fx}: {:?}", r.warnings);
        // limit-p3p4: after P2 the plan so far is kept and drawn (was M2.TL: no copper)
        assert!(!r.warnings.iter().any(|w| w.starts_with("ERROR: nothing to draw yet")) && !r.items.is_empty(), "{fx}: {:?}", r.warnings);
    }
}

/// M3 verifier finding (cancel drops the ERBS): after a Cancel in the TopoR loop the loop's pick
/// is drawn through (`draw_through`), so the Cancel must not stop the ERBS of that drawing; only
/// the time limit does. The run draws from its ERBS, no `erbs_timeout` line.
#[test]
fn cancel_in_the_topor_loop_keeps_the_erbs() {
    let (r, _) = cancel_after(opts(r#", "opt_loop": "topor", "search_length": "taut", "runs": 1, "realise": "erbs""#), After::Loop, Duration::from_millis(1500));
    assert_eq!(r.routed, r.connections_total, "{:?}", r.warnings);
    assert!(!r.warnings.iter().any(|w| w.starts_with("erbs_timeout")), "{:?}", r.warnings);
    let line = r.warnings.iter().find(|w| w.starts_with("ERBS drawing")).unwrap_or_else(|| panic!("{:?}", r.warnings));
    assert!(line.contains("(0 past the time limit"), "{line}");
}
