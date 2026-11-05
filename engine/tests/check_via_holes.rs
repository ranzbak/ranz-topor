use topor_engine::check::Checker;
use topor_engine::model::{BoardModel, Item, Options};

fn model(via_layers: &str) -> BoardModel {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "routable": true }}, {{ "id": 31, "routable": true }} ],
            "nets": [ {{ "code": 1, "name": "SIG" }}, {{ "code": 2, "name": "GND" }} ],
            "vias": [ {{ "uuid": "v1", "net": 2, "at": [5000000, 5000000], "d": 600000,
                         "drill": 300000, "layers": {via_layers} }} ],
            "rules": {{ "min_clearance": 0, "hole_clearance": 500000, "hole_to_hole": 250000,
                        "default": {{ "clearance": 200000 }} }} }}"#
    );
    BoardModel::from_json(&s).unwrap()
}

fn seg(net: i32, layer: u32, w: i64, a: [i64; 2], b: [i64; 2]) -> Item {
    serde_json::from_str(&format!(
        r#"{{ "net": {net}, "layer": {layer}, "width": {w},
             "prim": {{ "kind": "seg", "a": {a:?}, "b": {b:?} }}, "group": null }}"#
    ))
    .unwrap()
}

fn via(net: i32, x: i64, y: i64) -> Item {
    serde_json::from_str(&format!(
        r#"{{ "kind": "via", "net": {net}, "at": [{x}, {y}], "d": 600000, "drill": 300000,
             "layers": [0, 31], "group": null }}"#
    ))
    .unwrap()
}

#[test]
fn track_near_via_copper_ok_but_inside_hole_clearance() {
    let m = model("[0, 31]");
    let o = Options::default();
    let c = Checker::new(&m, &o);
    let rep = c.check_items(&[seg(1, 0, 250_000, [0, 5_650_000], [10_000_000, 5_650_000])]);
    assert_eq!(rep.violations.len(), 1, "{:?}", rep.violations);
    assert_eq!(rep.violations[0].against, "viahole:v1");
    assert_eq!(rep.violations[0].gap, 375_000);
    assert_eq!(rep.violations[0].required, 500_000);
}

#[test]
fn same_net_track_over_via_hole_is_exempt() {
    let m = model("[0, 31]");
    let o = Options::default();
    let c = Checker::new(&m, &o);
    let rep = c.check_items(&[seg(2, 0, 250_000, [0, 5_000_000], [10_000_000, 5_000_000])]);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

#[test]
fn new_via_drill_vs_existing_via_hole_to_hole() {
    let m = model("[0, 31]");
    let o = Options::default();
    let c = Checker::new(&m, &o);
    let ok = c.check_items(&[via(1, 5_600_000, 5_000_000)]);
    // Via copper still trips hole_clearance (0.5 mm); only the hole-to-hole rule (250 um) must pass.
    assert!(
        ok.violations.iter().all(|v| v.required != 250_000),
        "{:?}",
        ok.violations
    );
    let bad = c.check_items(&[via(1, 5_500_000, 5_000_000)]);
    assert!(
        bad.violations
            .iter()
            .any(|v| v.against == "viahole:v1" && v.gap == 200_000 && v.required == 250_000),
        "{:?}",
        bad.violations
    );
}

#[test]
fn via_hole_is_checked_on_layer_without_via_copper() {
    let m = model("[0]");
    let o = Options::default();
    let c = Checker::new(&m, &o);
    let rep = c.check_items(&[seg(1, 31, 250_000, [0, 5_650_000], [10_000_000, 5_650_000])]);
    assert_eq!(rep.violations.len(), 1, "{:?}", rep.violations);
    assert_eq!(rep.violations[0].against, "viahole:v1");
}
