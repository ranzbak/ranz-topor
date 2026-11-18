//! par-draw phase A (docs/dev/design/par-draw.md): the drawing's order-free units (the extended
//! rubber band of every stretch, the cut measurement of every layer) run on lanes and are folded
//! in unit order, so the copper is the same bits for every thread count, every `[drawing]
//! draw_lanes` and every completion order. The octilinear DP's successors of a node are units too. The reverse hook (`draw_test_reverse`) makes the first
//! units of every call finish last; a fold in completion order would change the result.
//!
//! Boards: eprom-to-kernal (2 layers) and the esp-rust-board / qmtech-minimig USB corners when
//! their fixtures are there; realise = "erbs" (the path with the units), one TopoR loop cycle.

use std::sync::atomic::Ordering;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};
use topor_engine::topo::{octi, par};

fn fixture(name: &str) -> Option<BoardModel> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let s = std::fs::read_to_string(path).ok()?;
    Some(BoardModel::from_json(&s).expect("fixture parses"))
}

/// Headless, one run, one cycle, realise = "erbs"; `extra` adds keys. The wall limit is a day:
/// nothing here may be cut by the clock (a cut run differs by timing).
fn opts(extra: &str) -> Options {
    let mut o: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 1, "runs": 1, "realise": "erbs"}"#).expect("base");
    let more: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&format!("{{{}}}", extra.trim_start_matches(','))).expect("extra options");
    o.extend(more);
    Options::from_json(&serde_json::Value::Object(o).to_string()).expect("options parse")
}

/// The route on a pool of `threads` threads (the engine's thread count, as RAYON_NUM_THREADS).
fn run(m: &BoardModel, o: &Options, threads: usize) -> RouteResult {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
    pool.install(|| {
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        route_with(m, o, &ctl)
    })
}

/// The copper and the connections.
fn key(r: &RouteResult) -> String {
    format!("{}\n{}", serde_json::to_string(&r.items).expect("items"), serde_json::to_string(&r.connections).expect("connections"))
}

fn first_diff(a: &str, b: &str) -> String {
    let i = a.bytes().zip(b.bytes()).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    let lo = i.saturating_sub(120);
    format!("at byte {i}: reference ...{}... vs ...{}...", &a[lo..(i + 120).min(a.len())], &b[lo..(i + 120).min(b.len())])
}

fn check(name: &str) {
    let Some(m) = fixture(name) else {
        eprintln!("{name}: fixture missing, skipped");
        return;
    };
    // the reference: one thread, one lane, no hook (the units one after the other)
    let reference = key(&run(&m, &opts(r#", "draw_lanes": 1"#), 1));
    for threads in [1usize, 2, 4, 8] {
        let l0 = par::UNITS_LANED.load(Ordering::Relaxed);
        let o0 = octi::OCTI_NODES_LANED.load(Ordering::Relaxed);
        let k = key(&run(&m, &opts(r#", "draw_test_reverse": true"#), threads));
        assert!(k == reference, "{name}: {threads} threads (draw_lanes auto, reverse hook) differ from one lane, {}", first_diff(&reference, &k));
        if threads > 1 {
            assert!(par::UNITS_LANED.load(Ordering::Relaxed) > l0, "{name}: {threads} threads ran no unit on lanes (the test would prove nothing)");
            assert!(octi::OCTI_NODES_LANED.load(Ordering::Relaxed) > o0, "{name}: {threads} threads ran no octilinear node on lanes");
        }
    }
    // a fixed lane count above the threads (lanes wait for each other)
    let k = key(&run(&m, &opts(r#", "draw_lanes": 5, "draw_test_reverse": true"#), 2));
    assert!(k == reference, "{name}: draw_lanes 5 on 2 threads differs from one lane, {}", first_diff(&reference, &k));
}

#[test]
fn eprom_copper_is_the_same_for_every_lane_count() {
    check("eprom-to-kernal-ripped.json");
}

#[test]
fn esp_corner_copper_is_the_same_for_every_lane_count() {
    check("esp-rust-board-usb-crop.json");
}

#[test]
fn qmtech_corner_copper_is_the_same_for_every_lane_count() {
    check("qmtech-usb-crop.json");
}
