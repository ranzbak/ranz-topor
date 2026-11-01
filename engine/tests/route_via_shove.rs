//! Via shove: a planned trace that does not fit at its width pushes another connection's via
//! aside (`Options::via_shove`).
//!
//! Fixture `qmtech-usb-crop.json`: the qmtech-minimig USB-C corner (J11 receptacle, U1 switch,
//! x 78..118 / y 46..71 mm, In1 / In2 planes not routable), cropped from the plugin model with
//! `fixtures/crop_model.py`. Without the shove `U1.8 -> J11.A6` (D+) fails metrization pinched
//! between another connection's via and pad J11.A7.

use topor_engine::check::Checker;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route, RouteResult};

const CROP: &str = include_str!("fixtures/qmtech-usb-crop.json");

fn opts(shove: bool, threads: usize) -> Options {
    Options {
        wall_clock_s: 600.0,
        threads,
        via_shove: shove,
        ..Options::default()
    }
}

fn conn<'a>(r: &'a RouteResult, from: &str, to: &str) -> &'a topor_engine::route::ConnReport {
    r.connections
        .iter()
        .find(|c| c.from.label == from && c.to.label == to)
        .unwrap_or_else(|| panic!("{from} -> {to} not in the result"))
}

fn strip(r: &RouteResult) -> String {
    let mut v = serde_json::to_value(r).unwrap();
    let o = v.as_object_mut().unwrap();
    o.remove("runtime_s");
    o.remove("threads_used");
    o.remove("profile");
    serde_json::to_string(&v).unwrap()
}

/// The pinched D+ connection routes once the via in its way is shoved; nothing that routed
/// without the shove is lost, and the result is clean at margin 0.
#[test]
fn shove_routes_the_pinched_connection() {
    let m = BoardModel::from_json(CROP).unwrap();
    let off = route(&m, &opts(false, 1));
    let on = route(&m, &opts(true, 1));
    let (d_off, d_on) = (conn(&off, "U1.8", "J11.A6"), conn(&on, "U1.8", "J11.A6"));
    assert_eq!(d_off.status, "failed");
    assert!(d_off.reason.starts_with("infeasible: ") && d_off.reason.contains("site:"), "{}", d_off.reason);
    assert_eq!(d_on.status, "routed", "{}", d_on.reason);
    assert_eq!(off.via_shove, "off");
    assert_eq!(on.via_shove, "routed 1 of 1 pinched connections, shoved 1 vias");
    assert_eq!(on.routed, off.routed + 1);
    for (a, b) in off.connections.iter().zip(&on.connections) {
        assert!(a.status != "routed" || b.status == "routed", "{} lost: {}", a.id, b.reason);
    }
    let rep = Checker::new(&m, &Options { margin_nm: 0, ..Options::default() }).check_items(&on.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

/// Threads 1 / 2 / 4: byte-identical results with the shove on.
#[test]
fn shove_threads_byte_identical() {
    let m = BoardModel::from_json(CROP).unwrap();
    let want = strip(&route(&m, &opts(true, 1)));
    for t in [2usize, 4] {
        assert_eq!(strip(&route(&m, &opts(true, t))), want, "threads {t}");
    }
}
