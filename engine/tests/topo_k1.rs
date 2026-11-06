//! K1 tests (functional plugin): the triangulation survives spade's degenerate-crossing panic.

use topor_engine::model::{BoardModel, Options};

/// Four copper graphics of dcsgonebadge as KiCad's IPC API exports them (the plugin's
/// `model_ipc`; arcs flattened outward, overlapping one another) plus the board outline, found by
/// delta debugging from the live plugin run that panicked in spade 2.15's
/// `add_constraint_and_split` ("!previous_region.is_empty() || from == final_vertex"). The
/// projection now retries with the constraints reversed, then with snapped points
/// (`cdt::Cdt::build`), so the route returns instead of panicking.
#[test]
fn degenerate_crossing_constraints_do_not_panic() {
    let text = include_str!("fixtures/cdt-degenerate-copper-shapes.json");
    // As the plugin passes them: KiCad-frame model JSON (from_json flips Y), options JSON.
    let model = BoardModel::from_json(text).expect("fixture parses");
    let opts = Options::from_json(r#"{"wall_clock_s": 5.0, "threads": 2}"#).expect("options parse");
    let r = topor_engine::route::route(&model, &opts);
    assert_eq!(r.connections_total, 0, "the fixture has no pads");
}
