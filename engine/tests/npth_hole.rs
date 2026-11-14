//! NPTH holes keep `hole_clearance` in the `Checker`: a slot hole is modelled as its capsule.

use topor_engine::check::Checker;
use topor_engine::model::{BoardModel, Item, Options};

#[test]
fn checker_models_a_slot_hole_as_its_capsule_like_the_router() {
    // NPTH slot 1.8 x 2.4 mm (long axis y). A vertical track whose copper edge is 0.36 mm from
    // the slot's flat side is legal for hole_clearance 0.35; a circle of d 2.4 mm would call it
    // a violation (the slot cases of a 2-layer badge).
    let s = r#"{ "layers": [ { "id": 0, "routable": true } ],
        "nets": [ { "code": 1, "name": "N1" } ],
        "pads": [ { "uuid": "d", "ref": "D", "number": "", "net": 0, "attr": "npth",
                    "center": [0, 0], "layers": [0], "drill": { "w": 1800000, "h": 2400000 },
                    "shapes": { "0": { "kind": "oval", "w": 1800000, "h": 2400000 } } } ],
        "holes": [ { "at": [0, 0], "d": 2400000, "kind": "npth", "slot": [1800000, 2400000] } ],
        "rules": { "min_clearance": 0, "hole_clearance": 350000, "default": { "clearance": 200000 } } }"#;
    let m = BoardModel::from_json(s).unwrap();
    let o = Options {
        margin_nm: 0,
        ..Options::default()
    };
    let c = Checker::new(&m, &o);
    let x = 900_000 + 350_000 + 125_000 + 10_000;
    let item = |x: i64| -> Item {
        serde_json::from_str(&format!(
            r#"{{ "net": 1, "layer": 0, "width": 250000,
                 "prim": {{ "kind": "seg", "a": [{x}, -1000000], "b": [{x}, 1000000] }}, "group": null }}"#
        ))
        .unwrap()
    };
    let ok = c.check_items(&[item(x)]);
    assert!(ok.violations.is_empty(), "{:?}", ok.violations);
    let bad = c.check_items(&[item(x - 30_000)]);
    assert!(!bad.violations.is_empty());
}
