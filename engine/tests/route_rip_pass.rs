//! Gradual blocker rip-up (`Options::rip_pass`): the routed connection that blocks the most
//! failed connections is ripped with its vias, its victims are routed first, then it again;
//! the result is kept only when more connections route.
//!
//! Fixture: the dcsgone USB-C crop (`fixtures/dcsgone-usb-crop.json`, see `route_steiner.rs`).

use topor_engine::check::Checker;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route, RouteResult};

const CROP: &str = include_str!("fixtures/dcsgone-usb-crop.json");

fn opts(rip: bool, threads: usize) -> Options {
    Options {
        wall_clock_s: 600.0,
        threads,
        rip_pass: rip,
        ..Options::default()
    }
}

fn strip(r: &RouteResult) -> String {
    let mut v = serde_json::to_value(r).unwrap();
    let o = v.as_object_mut().unwrap();
    o.remove("runtime_s");
    o.remove("threads_used");
    o.remove("profile");
    serde_json::to_string(&v).unwrap()
}

/// The pass routes more of the crop than the negotiation alone, every connection routed
/// without it stays routed, and the result is clean at margin 0.
#[test]
fn rip_pass_routes_more_and_loses_none() {
    let m = BoardModel::from_json(CROP).unwrap();
    let off = route(&m, &opts(false, 1));
    let on = route(&m, &opts(true, 1));
    assert_eq!(off.rip_pass, "off");
    assert!(on.rip_pass.starts_with("routed "), "{}", on.rip_pass);
    assert!(on.routed > off.routed, "{} -> {} ({})", off.routed, on.routed, on.rip_pass);
    for (a, b) in off.connections.iter().zip(&on.connections) {
        assert!(a.status != "routed" || b.status == "routed", "{} lost: {}", a.id, b.reason);
    }
    let rep = Checker::new(&m, &Options { margin_nm: 0, ..Options::default() }).check_items(&on.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

/// Seeded order: threads 1 / 2 / 4 give byte-identical results.
#[test]
fn rip_pass_threads_byte_identical() {
    let m = BoardModel::from_json(CROP).unwrap();
    let want = strip(&route(&m, &opts(true, 1)));
    for t in [2usize, 4] {
        assert_eq!(strip(&route(&m, &opts(true, t))), want, "threads {t}");
    }
}

/// On by default (older option JSON too).
#[test]
fn rip_pass_on_by_default() {
    assert!(Options::default().rip_pass);
    assert!(Options::from_json("{}").unwrap().rip_pass);
}
