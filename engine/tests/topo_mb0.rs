//! M-B0 tests (iterations like TopoR): whole-route cycles until "Routes stable", the user's OK
//! after stable, and the best of N seeded runs (eprom-to-kernal, routed with its tracks ripped).

use std::sync::atomic::Ordering;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route, route_with, RouteResult};

fn eprom() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/eprom-to-kernal-ripped.json")).expect("fixture parses")
}

fn opts(json: &str) -> Options {
    Options::from_json(json).expect("options parse")
}

fn cycles_line(r: &RouteResult) -> &str {
    r.warnings.iter().find(|w| w.contains("cycles (fluctuation schedule")).map(|s| s.as_str()).expect("cycle line")
}

/// A headless run iterates until its routes are stable (no fixed round count) and says so; the
/// stable test needs `stable_window + 1` cycles (TopoR: not before level 5-10). The whole-route
/// cycles' schedule (`opt_loop = "cycles"`, the default before M0; TopoR's loop, the default since
/// M0, is one cycle per run with its own stable test, `loop_stable_window`).
#[test]
fn a_run_iterates_until_stable() {
    let r = route(&eprom(), &opts(r#"{"runs": 1, "wall_clock_s": 120.0, "stable_window": 3, "opt_loop": "cycles"}"#));
    assert_eq!(r.stop_reason, "stable", "{:?}", r.warnings);
    assert!(!r.timed_out);
    let line = cycles_line(&r);
    assert!(line.contains("stable after cycle"), "{line}");
    let n: usize = line.split(": ").nth(1).and_then(|s| s.split(' ').next()).and_then(|s| s.parse().ok()).expect("cycle count");
    assert!(n >= 4, "stable needs a full window after the first cycle: {line}");
    assert!(line.contains("1: w 0.000"), "the first cycle of the first run uses the plain weights: {line}");
    assert!(line.contains("2: w 0.200"), "then the schedule starts wide: {line}");
}

/// Best of N: reproducible, and never worse than its first run (the run with the configured
/// seed), whose result it includes.
#[test]
fn best_of_runs_is_reproducible_and_not_worse_than_one_run() {
    let m = eprom();
    let one = route(&m, &opts(r#"{"runs": 1, "wall_clock_s": 120.0}"#));
    let three = || route(&m, &opts(r#"{"runs": 3, "wall_clock_s": 120.0}"#));
    let (a, b) = (three(), three());
    assert_eq!(a.items.len(), b.items.len());
    assert_eq!(a.total_length_nm, b.total_length_nm);
    assert_eq!(a.vias, b.vias);
    assert!(a.routed >= one.routed);
    let best = a.warnings.iter().find(|w| w.starts_with("best of 3 seeded runs")).expect("best-of line");
    assert!(best.contains("run 1 (seed 1)") && best.contains("run 3 ("), "{best}");
    assert_eq!(a.stop_reason, "stable");
}

/// With the dialog (`stop_when_stable` false) the run keeps going after "Routes stable"; OK
/// (= Cancel) then returns exactly the best result the progress showed, as "stable" and without
/// the time-budget warning. The whole-route cycles (`opt_loop = "cycles"`): TopoR's loop (the
/// default since M0) ends each run after its one loop, so nothing goes on after stable there.
#[test]
fn ok_after_stable_returns_the_best_so_far() {
    let m = eprom();
    let o = opts(r#"{"runs": 2, "wall_clock_s": 300.0, "stop_when_stable": false, "finish_when_stable": false, "topo_rounds": 1000, "opt_loop": "cycles"}"#);
    let ctl = RunControl::new();
    let c2 = ctl.clone();
    let h = std::thread::spawn(move || route_with(&m, &o, &c2));
    let t = std::time::Instant::now();
    let shown = loop {
        let p = ctl.snapshot();
        if p.stable {
            break p;
        }
        assert!(t.elapsed().as_secs() < 240, "never stable: {p:?}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(shown.runs, 2);
    assert!(shown.best_total > 0 && shown.best_run >= 1);
    // The runs go on after stable (the user may wait for marginal gains) ...
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!h.is_finished(), "a run with the dialog does not stop at stable by itself");
    // ... until OK: the progress's best at that moment is the board.
    let before = ctl.snapshot();
    ctl.cancel.store(true, Ordering::Relaxed);
    let r = h.join().expect("run thread");
    assert_eq!(r.stop_reason, "stable");
    assert!(!r.timed_out);
    assert!(!r.warnings.iter().any(|w| w.starts_with("time budget hit")), "{:?}", r.warnings);
    // The best only improves, and a cycle that OK interrupts is not offered: the board is the
    // best the progress shows when the run ends.
    let after = ctl.snapshot();
    assert!(r.routed >= before.best_routed && before.best_routed >= shown.best_routed);
    assert_eq!(r.routed, after.best_routed);
}

/// With the dialog and the defaults (`finish_when_stable`, `finish_patience` = 0) a run stops at
/// the cycle that becomes stable (user 2026-11-07: "When route stable appears it should stop
/// indeed"), as a normal finish.
#[test]
fn a_dialog_run_stops_at_stable_by_default() {
    let o = opts(r#"{"runs": 1, "wall_clock_s": 300.0, "stop_when_stable": false, "topo_rounds": 1000}"#);
    assert!(o.finish_when_stable && o.finish_patience == 0);
    let r = route(&eprom(), &o);
    assert_eq!(r.stop_reason, "stable");
    assert!(!r.timed_out);
    let line = cycles_line(&r);
    let n: usize = line.split(": ").nth(1).and_then(|s| s.split(' ').next()).and_then(|s| s.parse().ok()).expect("cycle count");
    assert!(line.contains(&format!("{n} cycles, stable after cycle {n},")), "stops at the stable cycle, not later: {line}");
}

/// With the dialog and `finish_when_stable` (default) a run also finishes by itself once the
/// best has not improved for `finish_patience` cycles after stable: a normal finish ("stable",
/// no time warning), without OK and long before the safety cap or the time budget.
#[test]
fn a_dialog_run_finishes_by_itself_after_stable() {
    let o = opts(r#"{"runs": 2, "wall_clock_s": 300.0, "stop_when_stable": false, "topo_rounds": 1000, "finish_patience": 3}"#);
    let r = route(&eprom(), &o);
    assert_eq!(r.stop_reason, "stable");
    assert!(!r.timed_out && !r.warnings.iter().any(|w| w.starts_with("time budget hit")));
    let line = cycles_line(&r);
    let n: usize = line.split(": ").nth(1).and_then(|s| s.split(' ').next()).and_then(|s| s.parse().ok()).expect("cycle count");
    assert!(n < 100, "finished by itself, not at the cap: {line}");
}

/// The score display (user 2026-11-11: "see the current score, and the best score in the UI"):
/// every round of every run ends with a line and its criteria in the progress, and the progress's
/// best at the end is the board the call returns; in the cycles mode and in the TopoR loop.
#[test]
fn progress_scores_every_round_and_its_best_is_the_result() {
    for (mode, extra) in [("cycles", ""), ("topor", r#", "opt_loop": "topor", "topo_rounds": 3"#)] {
        let m = eprom();
        let o = opts(&format!(r#"{{"runs": 2, "wall_clock_s": 120.0{extra}}}"#));
        let ctl = RunControl::new();
        let r = route_with(&m, &o, &ctl);
        let p = ctl.snapshot();
        assert!(p.rounds_ended >= 2, "{mode}: {p:?}");
        assert_eq!(p.round_lines.len(), p.rounds_ended as usize, "{mode}: every line kept (fewer than ROUND_LINES)");
        for run in 1..=2 {
            let n = p.round_lines.iter().filter(|l| l.starts_with(&format!("run {run}, round "))).count();
            assert!(n >= 1, "{mode}: run {run} has round lines: {:?}", p.round_lines);
            if mode == "topor" {
                assert_eq!(n, 1, "{mode}: one round per run: {:?}", p.round_lines);
            }
        }
        assert!(p.round_lines[0].contains("new best (the first result)"), "{mode}: {}", p.round_lines[0]);
        assert!(p.last_run >= 1 && p.last_round >= 1 && !p.last_vs_best.is_empty(), "{mode}: {p:?}");
        // The best shown is the board returned.
        assert!(p.best_run >= 1 && p.best_round >= 1, "{mode}: {p:?}");
        assert_eq!((p.best_routed, p.best_vias), (r.routed, r.vias), "{mode}");
        assert_eq!(p.best_open, r.connections_total - r.routed, "{mode}");
        let picked = r.warnings.iter().find(|w| w.starts_with("best of 2 seeded runs")).expect("best-of line");
        assert!(picked.contains(&format!("run {} picked", p.best_run)), "{mode}: {picked}");
    }
}
