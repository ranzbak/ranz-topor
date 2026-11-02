//! Failure pressure (`Options::failed_pressure`): before a failed path leaves the graph, the
//! occupants of the overfull edges it crosses (strands and the vias at their ends) search again
//! with history on that passage, so the passage is negotiated instead of only the failed wire
//! trying again.
//!
//! Fixture: the qmtech USB crop (`fixtures/qmtech-usb-crop.json`, see `route_via_shove.rs`).

use topor_engine::check::Checker;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route;

const CROP: &str = include_str!("fixtures/qmtech-usb-crop.json");

fn opts(press: bool) -> Options {
    Options {
        wall_clock_s: 600.0,
        threads: 1,
        failed_pressure: press,
        ..Options::default()
    }
}

/// On the crop the pressure routes three more connections than without, and the result is
/// clean at margin 0.
#[test]
fn failed_pressure_routes_more_of_the_crop() {
    let m = BoardModel::from_json(CROP).unwrap();
    let off = route(&m, &opts(false));
    let on = route(&m, &opts(true));
    assert_eq!((off.routed, off.connections_total), (56, 60));
    assert_eq!((on.routed, on.connections_total), (59, 60));
    let rep = Checker::new(&m, &Options { margin_nm: 0, ..Options::default() }).check_items(&on.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

/// Seeded: threads 1 and 4 give the same result with the pressure on.
#[test]
fn failed_pressure_is_deterministic() {
    let m = BoardModel::from_json(CROP).unwrap();
    let a = route(&m, &opts(true));
    let b = route(&m, &Options { threads: 4, ..opts(true) });
    let key = |r: &topor_engine::route::RouteResult| {
        serde_json::to_string(&r.items).unwrap()
    };
    assert_eq!(key(&a), key(&b));
}
