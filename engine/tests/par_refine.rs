//! par-refine (docs/design/47-parallel-refine.md): the layer refinement's lane executor keeps the
//! serial refinement's moves in the same order, so the plan and the copper are the same bits for
//! every lane count, with the lanes borrowed from free run slots (`refine_lanes = "auto"`) and
//! under load (other routes on the same threads at the same time). The reference is the serial
//! in-place loop (`Options::refine_reference`, the loop before par-refine).
//!
//! Boards: eprom-to-kernal (2 layers, small), the qmtech-minimig USB corner (4 layers, over-full
//! edges, many kept and rejected moves) and the esp-rust-board USB corner (2 layers) when its
//! fixture is there; the TopoR loop for two cycles per run.

use std::sync::atomic::Ordering;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};
use topor_engine::topo::refine_par::stats;

fn fixture(name: &str) -> Option<BoardModel> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let s = std::fs::read_to_string(path).ok()?;
    Some(BoardModel::from_json(&s).expect("fixture parses"))
}

fn boards() -> Vec<(&'static str, BoardModel)> {
    ["eprom-to-kernal-ripped.json", "qmtech-usb-crop.json", "esp-rust-board-usb-crop.json"].into_iter().filter_map(|f| fixture(f).map(|m| (f, m))).collect()
}

/// The TopoR method's settings (M0 defaults), headless, two loop cycles; `extra` adds keys. The
/// wall limit is a day: nothing here may be cut by the clock (a cut run differs by timing).
fn opts(extra: &str) -> Options {
    let mut o: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 2}"#).expect("base");
    let more: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&format!("{{{}}}", extra.trim_start_matches(','))).expect("extra options");
    o.extend(more);
    Options::from_json(&serde_json::Value::Object(o).to_string()).expect("options parse")
}

fn run(m: &BoardModel, o: &Options) -> RouteResult {
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    route_with(m, o, &ctl)
}

/// What must not depend on the lanes: the copper, the connections and the plan handed to the
/// drawing (its metrics line without timings and process-wide work meters).
fn key(r: &RouteResult) -> String {
    fn untimed(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                m.retain(|k, _| !(k.ends_with("_s") || k.starts_with("ms_") || k.starts_with("us_") || k == "layer_share_pct" || k == "cut_split" || k == "move_cost"));
                m.values_mut().for_each(untimed);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(untimed),
            _ => {}
        }
    }
    let line = r.warnings.iter().find(|w| w.starts_with("plan metrics:")).unwrap_or_else(|| panic!("plan metrics line: {:?}", r.warnings));
    let mut plan: serde_json::Value = serde_json::from_str(line.trim_start_matches("plan metrics:").trim()).expect("plan metrics JSON");
    untimed(&mut plan);
    format!("{}\n{}\n{}", serde_json::to_string(&r.items).expect("items"), serde_json::to_string(&r.connections).expect("connections"), plan)
}

fn first_diff(a: &str, b: &str) -> String {
    let i = a.bytes().zip(b.bytes()).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    let lo = i.saturating_sub(120);
    format!("at byte {i}: reference ...{}... vs ...{}...", &a[lo..(i + 120).min(a.len())], &b[lo..(i + 120).min(b.len())])
}

#[test]
fn every_lane_count_keeps_the_serial_plan_and_copper() {
    let e0 = stats::EPOCHS.load(Ordering::Relaxed);
    let w0 = stats::WASTED.load(Ordering::Relaxed);
    for (name, m) in boards() {
        let reference = key(&run(&m, &opts(r#", "runs": 1, "refine_reference": true"#)));
        for lanes in [1usize, 2, 3, 8] {
            let k = key(&run(&m, &opts(&format!(r#", "runs": 1, "refine_lanes": {lanes}"#))));
            assert!(k == reference, "{name}: refine_lanes = {lanes} differs from the serial refinement, {}", first_diff(&reference, &k));
        }
        // auto with one run: every other thread of the pool is a spare lane
        let k = key(&run(&m, &opts(r#", "runs": 1"#)));
        assert!(k == reference, "{name}: refine_lanes = auto differs from the serial refinement, {}", first_diff(&reference, &k));
        // three runs: lanes from the slots of the runs that wait at the draw gate, contended
        let reference3 = key(&run(&m, &opts(r#", "runs": 3, "refine_reference": true"#)));
        for extra in [r#", "runs": 3"#, r#", "runs": 3, "refine_lanes": 4"#] {
            let k = key(&run(&m, &opts(extra)));
            assert!(k == reference3, "{name}: {extra} differs from the serial refinement, {}", first_diff(&reference3, &k));
        }
        // under load: three routes at once on the same pool, each with its own lanes
        std::thread::scope(|s| {
            let hs: Vec<_> = [r#", "runs": 1"#, r#", "runs": 1, "refine_lanes": 3"#, r#", "runs": 1, "refine_lanes": 16"#]
                .into_iter()
                .map(|extra| {
                    let m = &m;
                    s.spawn(move || (extra, key(&run(m, &opts(extra)))))
                })
                .collect();
            for h in hs {
                let (extra, k) = h.join().expect("a route under load");
                assert!(k == reference, "{name} under load: {extra} differs from the serial refinement, {}", first_diff(&reference, &k));
            }
        });
    }
    // the lanes did run (epochs) and speculated past kept moves (wasted candidates); counters are
    // process-wide and only grow, so this test alone in its binary sees its own counts
    assert!(stats::EPOCHS.load(Ordering::Relaxed) > e0, "no epoch of the lane executor ran");
    assert!(stats::WASTED.load(Ordering::Relaxed) > w0, "no lane evaluated past a kept move: the boards test no speculation");
}
