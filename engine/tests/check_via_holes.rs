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

/// An integer offset (dx, dy), dy >= 0.1 mm, whose length is `off + f` nm with `f` in `lo..hi`
/// (a fraction of a nm off a whole number: the geometry an exact-clearance placement rounds to).
fn offset_at(off: f64, lo: f64, hi: f64) -> (i64, i64, f64) {
    for dy in (100_000..400_000).step_by(997) {
        let dx = (off * off - (dy * dy) as f64).sqrt();
        for x in [dx.floor() as i64 - 1, dx.floor() as i64, dx.ceil() as i64, dx.ceil() as i64 + 1] {
            let f = ((x * x + dy * dy) as f64).sqrt() - off;
            if (lo..hi).contains(&f) {
                return (x, dy, f);
            }
        }
    }
    panic!("no offset {off} + {lo}..{hi}");
}

/// erbs-margin: the drill-to-drill rule (hole_to_hole 0.25 mm) passes a gap short by up to
/// `CHECK_TOL_NM` (1 nm) and rejects one short by more: drill gaps at +0.4, -0.6 nm pass, -1.6 nm
/// does not.
#[test]
fn hole_to_hole_tolerance_is_a_fraction_of_a_nm() {
    let m = model("[0, 31]");
    let c = Checker::new(&m, &Options::default());
    for (lo, hi, ok) in [(0.3, 0.5, true), (-0.7, -0.5, true), (-1.7, -1.5, false)] {
        let (dx, dy, f) = offset_at(550_000.0, lo, hi);
        let v = c.check_items(&[via(1, 5_000_000 + dx, 5_000_000 + dy)]).violations;
        let h2h: Vec<_> = v.iter().filter(|x| x.required == 250_000).collect();
        assert_eq!(h2h.is_empty(), ok, "drill gap 250000 {f:+.3} nm: {v:?}");
    }
}

/// erbs-margin: a placed via's drill to foreign copper (via hole clearance 0.5 mm, here to the
/// corner of a pad) passes a gap short by up to 1 nm, not more.
#[test]
fn via_drill_to_copper_tolerance_is_a_fraction_of_a_nm() {
    let s = r#"{ "layers": [ { "id": 0, "routable": true }, { "id": 31, "routable": true } ],
        "nets": [ { "code": 1, "name": "SIG" }, { "code": 2, "name": "GND" } ],
        "pads": [ { "uuid": "pb", "ref": "U1", "number": "1", "net": 2, "attr": "smd",
             "center": [0, 0], "rotation_deg": 0.0, "layers": [0],
             "shapes": { "0": { "kind": "rect", "w": 1000000, "h": 1000000 } } } ],
        "rules": { "min_clearance": 0, "hole_clearance": 500000, "hole_to_hole": 250000,
                   "default": { "clearance": 200000 } } }"#;
    let m = BoardModel::from_json(s).unwrap();
    let c = Checker::new(&m, &Options::default());
    for (lo, hi, ok) in [(0.3, 0.5, true), (-0.7, -0.5, true), (-1.7, -1.5, false)] {
        // drill radius 0.15 + 0.5 from the pad's corner (0.5, 0.5)
        let (dx, dy, f) = offset_at(650_000.0, lo, hi);
        let v = c.check_items(&[via(1, 500_000 + dx, 500_000 + dy)]).violations;
        let hole: Vec<_> = v.iter().filter(|x| x.required == 500_000 && x.against == "pad:U1:1").collect();
        assert_eq!(hole.is_empty(), ok, "drill gap 500000 {f:+.3} nm: {v:?}");
        assert!(v.iter().all(|x| x.required == 500_000), "only the drill rule is in reach: {v:?}");
    }
}

/// grid-snap r3: with the copper clearance equal to the hole clearance (0.2 mm both), a new via
/// next to an existing foreign via gets two violations against it, as KiCad reports them: its
/// copper (gap 0.02 mm, `clearance`) and its drill against the via's copper (drill edge, gap
/// 0.17 mm, `hole_clearance`, `drill`). Merged by rule value, the drill's gap was lost under the
/// copper's (esp: KiCad 0.007 mm below the hole rule, the engine 0.107 mm).
#[test]
fn via_drill_and_copper_violations_stay_apart_at_equal_rules() {
    let s = r#"{ "layers": [ { "id": 0, "routable": true }, { "id": 31, "routable": true } ],
            "nets": [ { "code": 1, "name": "SIG" }, { "code": 2, "name": "GND" } ],
            "vias": [ { "uuid": "v1", "net": 2, "at": [5000000, 5000000], "d": 600000,
                         "drill": 300000, "layers": [0, 31] } ],
            "rules": { "min_clearance": 0, "hole_clearance": 200000, "hole_to_hole": 250000,
                        "default": { "clearance": 200000 } } }"#;
    let m = BoardModel::from_json(s).unwrap();
    let c = Checker::new(&m, &Options::default());
    let rep = c.check_items(&[via(1, 5_620_000, 5_000_000)]);
    let on = |drill: bool| rep.violations.iter().filter(|v| v.against == "via:v1" && v.layer == 0 && v.drill == drill && v.required == 200_000).map(|v| v.gap).collect::<Vec<_>>();
    assert_eq!(on(false), vec![20_000], "{:?}", rep.violations);
    assert_eq!(on(true), vec![170_000], "{:?}", rep.violations);
}
