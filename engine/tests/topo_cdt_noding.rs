//! robust-cdt: the board whose projection spade could not triangulate (bornhack2026, user run
//! 2026-11-07 20:39: 0 of 352 connections, "projection failed: triangulation failed on degenerate
//! crossing constraints"). The fixture is the minimal set of copper graphics of the NFC coil
//! footprint AE1 (lcsc:NFC_2.8uH_COIL, rotated -45°) that still makes every old attempt fail
//! (spade's splitting, reversed order, 128 nm snap), cut out of the plugin's own engine input
//! (TOPOR_DUMP_ENGINE_INPUT, KiCad IPC model): 0.3 mm stroked arcs on B.Cu / In1.Cu and lines on
//! In2.Cu / F.Cu that meet end to end on different layers, so their round end caps (polygonised
//! with different vertex sets) overlap nearly exactly in the all-layer projection and cross each
//! other at nearly degenerate positions.

use topor_engine::model::{BoardModel, Options};

fn fixture() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/bh-nfc-coil-ae1.json")).expect("fixture parses")
}

#[test]
fn nfc_coil_projection_takes_the_noded_path() {
    let p = topor_engine::topo::core::probe(&fixture(), &Options::default());
    assert!(p.ok, "{:?}", p.error);
    assert_eq!(p.path, "noded");
    assert_eq!(p.notes.len(), 1, "nothing left out: {:?}", p.notes);
    let n = &p.notes[0];
    assert!(n.contains("assertion failed: !previous_region.is_empty() || from == final_vertex"), "{n}");
    assert!(n.contains("copper graphic of footprint AE1"), "{n}");
    assert!(n.contains("built by the noded path"), "{n}");
}

#[test]
fn nfc_coil_route_returns() {
    let opts = Options::from_json(r#"{"wall_clock_s": 5.0, "threads": 2}"#).expect("options parse");
    let r = topor_engine::route::route(&fixture(), &opts);
    assert!(!r.warnings.iter().any(|w| w.starts_with("projection failed")), "{:?}", r.warnings);
}
