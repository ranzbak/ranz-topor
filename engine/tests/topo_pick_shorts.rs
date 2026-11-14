//! `[optimize] pick_shorts` end to end (user 2026-11-12, "shorts should be in the routing
//! criteria"): with the option on, the variant table counts per variant the connections with a
//! short, the picked one's count is the result's own (its `short` violations), and no variant of
//! the same error count has fewer; with it off, no variant counts a short and the pick takes
//! the fewest errors as before. Fixtures: the qmtech and esp-rust-board USB crops (dense pad
//! exits, where the drawing writes shorts; esp-rs/esp-rust-board, CERN-OHL-P-2.0, `source` in the fixture).

use std::collections::BTreeSet;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route, RouteResult};

fn run(fixture: &str, shorts: bool) -> RouteResult {
    let m = BoardModel::from_json(fixture).expect("fixture parses");
    let o = Options::from_json(&format!(r#"{{"runs": 1, "wall_clock_s": 60.0, "topo_rounds": 3, "pick_shorts": {shorts}}}"#)).expect("options parse");
    route(&m, &o)
}

fn check(name: &str, fixture: &str) {
    let on = run(fixture, true);
    let picked = on.variants.iter().find(|v| v.picked).unwrap_or_else(|| panic!("{name}: a picked variant"));
    let shorted: BTreeSet<usize> = on.violations.iter().filter(|v| v.kind == "short").map(|v| v.conn).collect();
    assert_eq!(picked.shorts, shorted.len(), "{name}: the picked variant counts the result's connections with a short: {:?}", on.variants);
    assert!(on.variants.iter().all(|v| v.shorts <= v.errors), "{name}: shorts are among the errors: {:?}", on.variants);
    assert!(on.variants.iter().all(|v| v.errors > picked.errors || v.errors == picked.errors && v.shorts >= picked.shorts), "{name}: fewest errors, then fewest shorts: {:?}", on.variants);

    let off = run(fixture, false);
    assert!(off.variants.iter().all(|v| v.shorts == 0), "{name}: off: no variant counts a short: {:?}", off.variants);
    let p = off.variants.iter().find(|v| v.picked).unwrap_or_else(|| panic!("{name}: off: a picked variant"));
    assert!(off.variants.iter().all(|v| v.errors >= p.errors), "{name}: off: the pick takes the fewest errors: {:?}", off.variants);
}

#[test]
fn qmtech_usb_crop_ranks_shorts_with_the_option_and_not_without() {
    check("qmtech-usb-crop", include_str!("fixtures/qmtech-usb-crop.json"));
}

#[test]
fn esp_rust_board_usb_crop_ranks_shorts_with_the_option_and_not_without() {
    check("esp-rust-board-usb-crop", include_str!("fixtures/esp-rust-board-usb-crop.json"));
}
