//! C0 tests (`[run] stop_after = "plan"`): the run ends after the plan of its first cycle, writes
//! no copper, and its plan metrics are the ones the full run reports for the same plan.
//! Fixture: eprom-to-kernal with its tracks ripped (as topo_mb0).

use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route, RouteResult};

fn eprom() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/eprom-to-kernal-ripped.json")).expect("fixture parses")
}

fn opts(json: &str) -> Options {
    Options::from_json(json).expect("options parse")
}

/// The `plan metrics:` lines of a result, parsed.
fn plan_lines(r: &RouteResult) -> Vec<serde_json::Value> {
    r.warnings.iter().filter_map(|w| w.strip_prefix("plan metrics: ")).map(|s| serde_json::from_str(s).expect("plan metrics JSON")).collect()
}

#[test]
fn stop_after_plan_reports_the_plan_of_the_full_run() {
    let m = eprom();
    // One run, one cycle: the full run's picked result is the cycle-1 plan, drawn.
    let full = route(&m, &opts(r#"{"runs": 1, "wall_clock_s": 120.0, "topo_rounds": 1}"#));
    let plan = route(&m, &opts(r#"{"runs": 1, "wall_clock_s": 120.0, "topo_rounds": 1, "stop_after": "plan"}"#));
    let (pf, pp) = (plan_lines(&full), plan_lines(&plan));
    assert_eq!(pf.len(), 1, "{:?}", full.warnings);
    assert_eq!(pp, pf, "the plan before the drawing is the same");
    let p = &pp[0];
    // eprom is through-hole: its crossings are layered without vias (each wire on one layer).
    assert!(p["wires"].as_u64().unwrap() > 0 && p["crossings"].as_u64().unwrap() > 0, "{p}");
    assert!(p["topo_length_mm"].as_f64().unwrap() > 0.0, "{p}");
    // Nothing drawn, every connection reported as not drawn, the stop reason says why.
    assert!(full.routed > 0);
    assert_eq!(plan.routed, 0);
    assert!(plan.items.is_empty());
    assert_eq!(plan.stop_reason, "stop_after plan");
    assert!(plan.connections.iter().all(|c| c.reason.starts_with("not drawn: [run] stop_after")), "{:?}", plan.connections.first());
}

#[test]
fn stop_after_plan_runs_one_cycle_per_run() {
    // With the default cycle cap the run still stops after cycle 1.
    let r = route(&eprom(), &opts(r#"{"runs": 2, "wall_clock_s": 120.0, "stop_after": "plan"}"#));
    assert_eq!(r.variants.len(), 1, "{:?}", r.variants);
    assert_eq!(plan_lines(&r).len(), 1);
    assert!(!r.timed_out);
}
