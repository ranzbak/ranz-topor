//! M6 F-1 (2026-11-18): a run for which no attempt of the drawing ladder produced copper to check
//! (the exact taut path found none and no later attempt ran) failed with an empty reason, so the
//! never-drop pass, which redraws the wires whose reason starts with "drawing:", skipped it: the
//! connection was dropped silently and result.json showed a blank reason (bornhack, phase 1
//! measurement: 56 of 104 lost connections).
//!
//! Reached here with the measurement switch `TOPO_M6_OFF=ladder` (the funnel + repair attempts
//! off; the only attempt left is the exact path, which has no legal result in the narrow gap).
//! Its own test binary: the switch is read once per process.

use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route;

fn gap() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/fx-narrow-gap.json")).expect("fixture parses")
}

fn opts(json: &str) -> Options {
    Options::from_json(json).expect("options parse")
}

#[test]
fn a_run_without_any_attempt_is_never_dropped_silently() {
    std::env::set_var("TOPO_M6_OFF", "ladder");
    // Legal-only output: the connection fails, and says why.
    let r = route(&gap(), &opts(r#"{"runs": 1, "wall_clock_s": 60.0, "allow_violations": false}"#));
    assert_eq!(r.routed, 0, "{:?}", r.warnings);
    let reason = &r.connections[0].reason;
    assert!(reason.starts_with("drawing: no run found on layer "), "{reason:?}");
    // Never drop (the default): the never-drop pass draws it, with its violations reported.
    let r = route(&gap(), &opts(r#"{"runs": 1, "wall_clock_s": 60.0}"#));
    assert_eq!((r.routed, r.connections_total), (1, 1), "{:?} {:?}", r.connections, r.warnings);
    assert!(!r.violations.is_empty());
    assert!(r.connections.iter().all(|c| c.status == "routed" || !c.reason.is_empty()), "{:?}", r.connections);
}
