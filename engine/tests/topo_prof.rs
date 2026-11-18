//! TW0 profiling counters (docs/dev/profiling.md): switched on they change nothing in the copper and
//! account for the run's wall time stage by stage; switched off there is no report.
//! One test: the switch is process-wide.

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};
use topor_engine::topo::prof;

fn run(m: &BoardModel, o: &Options) -> RouteResult {
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    route_with(m, o, &ctl)
}

const FIXTURES: [&str; 3] = ["eprom-to-kernal-ripped.json", "qmtech-usb-crop.json", "esp-rust-board-usb-crop.json"];

#[test]
fn counters_do_not_touch_the_copper_and_add_up() {
    let o = Options::from_json(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 2, "runs": 1, "refine_lanes": 1}"#).expect("options");
    let key = |r: &RouteResult| format!("{}\n{}", serde_json::to_string(&r.items).unwrap(), serde_json::to_string(&r.connections).unwrap());
    for f in FIXTURES {
        let path = format!("{}/tests/fixtures/{f}", env!("CARGO_MANIFEST_DIR"));
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let m = BoardModel::from_json(&text).expect("parses");

        prof::set_on(false);
        let t = std::time::Instant::now();
        let off = run(&m, &o);
        let wall_off = t.elapsed().as_secs_f64();
        assert!(prof::report().is_none(), "off: no report");

        prof::set_on(true);
        let t = std::time::Instant::now();
        let on = run(&m, &o);
        let wall = t.elapsed().as_secs_f64();
        let rep = prof::report().expect("on: a report");
        prof::set_on(false);
        eprintln!("{f}: off {wall_off:.2} s, on {wall:.2} s (one run each, not a benchmark)");

        assert_eq!(key(&off), key(&on), "{f}: counters on = the same copper");

        let stages = rep["stages"].as_array().expect("stages");
        let names: Vec<&str> = stages.iter().map(|s| s["stage"].as_str().unwrap()).collect();
        for want in ["stubs", "projection", "p2_p4", "loop", "drawing", "via_reduction", "tail"] {
            assert!(names.contains(&want), "{f}: stage {want} missing in {names:?}");
        }
        let sum: f64 = stages.iter().map(|s| s["wall_s"].as_f64().unwrap()).sum();
        assert!((sum - rep["engine_s"].as_f64().unwrap()).abs() < 1e-9, "engine_s is the sum of the stage windows");
        assert!(sum <= wall + 0.05 && sum >= 0.8 * wall, "{f}: the windows cover the run: {sum} of {wall}");
        let lp = stages.iter().find(|s| s["stage"] == "loop").unwrap();
        let cycles = rep["cycles"].as_array().unwrap();
        assert!(!cycles.is_empty(), "{f}: the loop ran cycles");
        assert!(lp["keys"]["cycle"][0].as_u64().unwrap() as usize >= cycles.len(), "one Sp::cycle call per cycle row");
        assert!(lp["keys"]["reroute"][0].as_u64().unwrap() > 0, "{f}: re-lays were counted");
        // the cycle laps partition the cycle: together they never exceed the loop window
        let laps: f64 = ["topo_setup", "st_sweep", "st_uncross_reopt", "st_viarose", "st_float", "st_cross", "st_pairrelay", "st_pairmove"].iter().map(|k| lp["keys"][*k][1].as_f64().unwrap_or(0.0)).sum();
        let loop_s = lp["wall_s"].as_f64().unwrap();
        // every lap runs once per cycle that reaches the end of cycle_body (early returns skip the later laps)
        let calls = |k: &str| lp["keys"][k][0].as_u64().unwrap_or(0);
        for k in ["topo_setup", "st_sweep", "st_uncross_reopt", "st_viarose", "st_float", "st_cross", "st_pairrelay", "st_pairmove"] {
            assert!(calls(k) > 0 && calls(k) <= calls("cycle"), "{f}: lap {k} has {} calls for {} cycles", calls(k), calls("cycle"));
        }
        // the loop-local judge (the default) is counted as a layering: one call per local layering at least
        // as many as the judged re-lays' price calls
        assert!(calls("assign_with") >= calls("rr_viol_after"), "{f}: assign_with {} < rr_viol_after {} (local layering not counted?)", calls("assign_with"), calls("rr_viol_after"));
        // the closing mark("finish") of report() is in the tail window
        let tail = stages.iter().find(|s| s["stage"] == "tail").unwrap();
        assert!(tail["labels"].as_array().unwrap().iter().any(|l| l == "finish"), "{f}: tail labels {:?}", tail["labels"]);
        assert!(laps <= loop_s * 1.001 && laps >= 0.5 * loop_s, "{f}: laps {laps} s of the loop window {loop_s} s");
    }
}
