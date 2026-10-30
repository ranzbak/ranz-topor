use topor_engine::check::Checker;
use topor_engine::fuzz::fuzz_items;
use topor_engine::geom::TrackPrim;
use topor_engine::model::{BoardModel, Item, Options, TrackItem};

/// Pad net 2 sits in a netclass with 0.5 mm clearance, items are net 1 (0.2 mm default).
fn model(pad_local: &str) -> BoardModel {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "routable": true }} ],
            "nets": [ {{ "code": 1, "name": "A" }}, {{ "code": 2, "name": "B" }} ],
            "pads": [ {{ "uuid": "pb", "ref": "U1", "number": "1", "net": 2, "attr": "smd",
                 "center": [0, 0], "rotation_deg": 0.0, "layers": [0]{pad_local},
                 "shapes": {{ "0": {{ "kind": "rect", "w": 1000000, "h": 1000000 }} }} }} ],
            "outline": [
              {{ "kind": "seg", "a": [-20000000, -20000000], "b": [20000000, -20000000] }},
              {{ "kind": "seg", "a": [20000000, -20000000], "b": [20000000, 20000000] }},
              {{ "kind": "seg", "a": [20000000, 20000000], "b": [-20000000, 20000000] }},
              {{ "kind": "seg", "a": [-20000000, 20000000], "b": [-20000000, -20000000] }} ],
            "rules": {{ "per_net": {{ "2": {{ "clearance": 500000 }} }} }} }}"#
    );
    BoardModel::from_json(&s).unwrap()
}

/// Horizontal net-1 track of width 0.2 mm whose copper is `gap` nm above the pad edge (y = 500000).
fn track_above(gap: i32) -> Item {
    let y: i32 = 500_000 + gap + 100_000;
    Item::Track(TrackItem {
        net: 1,
        layer: 0,
        width: 200_000,
        prim: TrackPrim::Seg {
            a: [-3_000_000, y].into(),
            b: [3_000_000, y].into(),
        },
        group: None,
        wire: None,
    })
}

#[test]
fn pad_uses_its_own_netclass_clearance() {
    let m = model("");
    let o = Options::default();
    let c = Checker::new(&m, &o);
    // 0.3 mm gap: fine for the item's own class (0.2), too close for the pad's class (0.5).
    let rep = c.check_items(&[track_above(300_000)]);
    assert_eq!(rep.violations.len(), 1, "{:?}", rep.violations);
    assert_eq!(rep.violations[0].against, "pad:U1:1");
    assert!(rep.violations[0].required >= 500_000);
    // 0.6 mm gap is clean.
    assert!(c.check_items(&[track_above(600_000)]).violations.is_empty());
}

#[test]
fn pad_local_override_still_wins_when_larger() {
    let m = model(r#", "local_clearance": 800000"#);
    let o = Options::default();
    let c = Checker::new(&m, &o);
    assert_eq!(c.check_items(&[track_above(600_000)]).violations.len(), 1);
    assert!(c.check_items(&[track_above(900_000)]).violations.is_empty());
}

#[test]
fn fuzz_respects_pad_netclass_clearance() {
    let m = model("");
    let o = Options::default();
    let r = fuzz_items(&m, &o, 7, 50);
    // Only one pad net other than the item net exists, so items are net 1 around the net-2 pad.
    assert!(!r.items.is_empty());
    let rep = Checker::new(&m, &o).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
    // Every item keeps >= 0.5 mm to the pad: re-check against a model where the pad class is 0.5 and
    // the checker would flag anything closer (self-check is clean), and the placement gap used it.
    let strict = model(r#", "local_clearance": 500000"#);
    let rep2 = Checker::new(&strict, &o).check_items(&r.items);
    assert!(rep2.violations.is_empty(), "{:?}", rep2.violations);
}
