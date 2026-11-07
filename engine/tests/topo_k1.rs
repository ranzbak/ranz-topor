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

/// robust-cdt: the same fixture through the plugin's fail-fast probe. spade's own splitting still
/// panics on it; the noded path builds the projection (no constraint left out) and the note names
/// the copper graphic where spade failed, with its layer, net, uuid and place.
#[test]
fn degenerate_crossing_constraints_take_the_noded_path() {
    let model = BoardModel::from_json(include_str!("fixtures/cdt-degenerate-copper-shapes.json")).expect("fixture parses");
    let p = topor_engine::topo::core::probe(&model, &Options::default());
    assert!(p.ok, "{:?}", p.error);
    assert_eq!(p.path, "noded");
    assert_eq!(p.notes.len(), 1, "{:?}", p.notes);
    let n = &p.notes[0];
    assert!(n.contains("copper graphic on B.Cu [GND] (uuid 671c67bf-4fe3-4d9d-a2a8-c45279f5fdf4)"), "{n}");
    assert!(n.contains("(64.1708, 51.1086) mm"), "{n}");
    assert!(n.contains("built by the noded path"), "{n}");
}
