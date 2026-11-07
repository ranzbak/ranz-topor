//! The fail-fast probe on a board the engine cannot read: the bornhack2026 NFC coil fixture with the
//! noded path switched off (`TOPOR_CDT_NODING=never`, the behaviour before robust-cdt). Its own test
//! binary: the switch is a process environment variable.

use topor_engine::model::{BoardModel, Options};

#[test]
fn probe_names_the_copper_where_the_triangulation_failed() {
    std::env::set_var("TOPOR_CDT_NODING", "never");
    let m = BoardModel::from_json(include_str!("fixtures/bh-nfc-coil-ae1.json")).expect("fixture parses");
    let p = topor_engine::topo::core::probe(&m, &Options::default());
    std::env::remove_var("TOPOR_CDT_NODING");
    assert!(!p.ok);
    let e = p.error.as_deref().unwrap_or_default();
    assert!(e.starts_with("projection failed: triangulation failed on degenerate crossing constraints"), "{e}");
    assert!(!e.contains(" at a side of"), "the places are separate: {e}");
    assert!(p.places.len() >= 2, "{:?}", p.places);
    assert!(p.places[0].contains("copper graphic of footprint AE1 on In2.Cu"), "{:?}", p.places);
    assert!(p.places[0].contains("uuid 8defc2ce-5eeb-48e7-8dac-526733d92ecf"), "{:?}", p.places);
    assert!(p.places[1].contains("AE1 on B.Cu"), "{:?}", p.places);
    assert!(p.seconds < 10.0);
}
