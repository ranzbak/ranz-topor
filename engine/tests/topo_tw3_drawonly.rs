//! TW3 draw-only rounds (its own test binary: the TOPO_TW3_DRAWONLY hook is a process env var).
//! TW3 TABLE (docs/dev/design/tw3-table.md; topor-way plan stage TW3): one variant table for all
//! runs, periodic restarts on the re-lay work clock, the four bests drawn and compared at 100 %
//! drawn (TopoR 6.3 manual pp. 72-73; Polubasov 2010 «Многовариантная оптимизация»).

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};

fn eprom() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/eprom-to-kernal-ripped.json")).expect("fixture parses")
}

/// The TopoR method's settings, headless; `extra` adds keys (`, "key": value ...`).
fn topor(extra: &str) -> Options {
    let mut o: serde_json::Map<String, serde_json::Value> = serde_json::from_str(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut"}"#).expect("base");
    let more: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&format!("{{{}}}", extra.trim_start_matches(','))).expect("extra options");
    o.extend(more);
    Options::from_json(&serde_json::Value::Object(o).to_string()).expect("options parse")
}

fn run(m: &BoardModel, o: &Options, threads: usize) -> RouteResult {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
    pool.install(|| {
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        route_with(m, o, &ctl)
    })
}

/// The copper of a result (segments, arcs, vias) as text.
fn copper(r: &RouteResult) -> String {
    serde_json::to_string(&r.items).expect("items")
}

#[test]
fn a_row_drawn_in_a_draw_only_round_is_the_same_copper() {
    // the draw-only round (no initial routing, no loop: the row and its run's pre-loop decisions
    // restored into a fresh round) draws the same copper as the round that ran the loop
    let m = eprom();
    let o = topor(r#", "runs": 1, "draw_candidates": 1"#);
    let own = run(&m, &o, 2);
    std::env::set_var("TOPO_TW3_DRAWONLY", "1");
    let only = run(&m, &o, 2);
    std::env::remove_var("TOPO_TW3_DRAWONLY");
    assert_eq!(own.bests.len(), 1);
    assert_eq!(only.bests.len(), 1, "{:?}", only.warnings);
    assert_eq!(own.bests[0]["row"], only.bests[0]["row"]);
    assert_eq!(copper(&own), copper(&only));
    assert_eq!(own.bests[0]["drawn"], only.bests[0]["drawn"]);
}


/// TW3-R5 (tw5 r3 finding 4): the applied variant drawn in a draw-only round carries the loop's
/// real counters in result.json's warnings (the loop judge and the TW5 escapes lines), not the
/// zeros of a round that ran no loop. The same run drawn in its own round has the same lines.
#[test]
fn a_draw_only_row_keeps_the_loops_counter_warnings() {
    let m = eprom();
    let o = topor(r#", "runs": 1, "draw_candidates": 1, "escape": "layering", "topo_rounds": 3"#);
    let own = run(&m, &o, 2);
    std::env::set_var("TOPO_TW3_DRAWONLY", "1");
    let only = run(&m, &o, 2);
    std::env::remove_var("TOPO_TW3_DRAWONLY");
    let lines = |r: &RouteResult, p: &str| r.warnings.iter().filter(|w| w.starts_with(p)).cloned().collect::<Vec<_>>();
    for p in ["TopoR loop judge (", "TopoR loop escapes (TW5)"] {
        let (a, b) = (lines(&own, p), lines(&only, p));
        assert_eq!(a.len(), 1, "{p}: one line in the own round: {a:?}");
        assert_eq!(b, a, "{p}: the draw-only round carries the loop's counts");
    }
    let judge = &lines(&only, "TopoR loop judge (")[0];
    assert!(!judge.contains("per cycle 0.0 whole-board layerings, 0.0 whole-board measures"), "real counts, not zeros: {judge}");
}
