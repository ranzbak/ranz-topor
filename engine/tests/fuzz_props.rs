use topor_engine::check::Checker;
use topor_engine::fuzz::fuzz_items;
use topor_engine::geom::TrackPrim;
use topor_engine::model::{BoardModel, Item, Options};

fn model() -> BoardModel {
    let pad = |n: u32, net: i32, x: i64, y: i64, shape: &str| {
        format!(
            r#"{{ "uuid": "p{n}", "ref": "U1", "number": "{n}", "net": {net}, "attr": "smd",
                 "center": [{x}, {y}], "rotation_deg": 0.0, "layers": [0, 2],
                 "shapes": {{ "0": {shape}, "2": {shape} }} }}"#
        )
    };
    let circle = r#"{ "kind": "circle", "d": 1600000 }"#;
    let rect = r#"{ "kind": "rect", "w": 1000000, "h": 2000000 }"#;
    let oval = r#"{ "kind": "oval", "w": 1000000, "h": 2000000 }"#;
    let rr = r#"{ "kind": "roundrect", "w": 1000000, "h": 2000000, "rc": 250000 }"#;
    let pads = [
        pad(1, 1, 20_000_000, -20_000_000, circle),
        pad(2, 2, 25_000_000, -20_000_000, rect),
        pad(3, 2, 30_000_000, -20_000_000, rr),
        pad(4, 3, 35_000_000, -20_000_000, oval),
        pad(5, 2, 20_000_000, -28_000_000, rect),
        pad(6, 1, 35_000_000, -28_000_000, rect),
        pad(7, 3, 28_000_000, -26_000_000, circle),
    ]
    .join(",");
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "routable": true }}, {{ "id": 2, "routable": true }} ],
            "nets": [ {{ "code": 1, "name": "VCC" }}, {{ "code": 2, "name": "SIG" }},
                      {{ "code": 3, "name": "GND" }} ],
            "pads": [ {pads} ],
            "outline": [
              {{ "kind": "seg", "a": [0, 0], "b": [50000000, 0] }},
              {{ "kind": "seg", "a": [50000000, 0], "b": [50000000, -40000000] }},
              {{ "kind": "seg", "a": [50000000, -40000000], "b": [0, -40000000] }},
              {{ "kind": "seg", "a": [0, -40000000], "b": [0, 0] }} ],
            "rules": {{}} }}"#
    );
    BoardModel::from_json(&s).unwrap()
}

#[test]
fn fuzz_1000_items_self_check_clean() {
    let m = model();
    let o = Options::default();
    let r = fuzz_items(&m, &o, 1, 1000);
    assert!(!r.truncated, "truncated after {} attempts", r.attempts);
    assert_eq!(r.items.len(), 1000);
    let rep = Checker::new(&m, &o).check_items(&r.items);
    assert!(
        rep.violations.is_empty(),
        "{:?}",
        &rep.violations[..rep.violations.len().min(5)]
    );
    let flat = r
        .items
        .iter()
        .zip(&r.from_arc)
        .filter(|(i, a)| {
            **a && matches!(i, Item::Track(t) if matches!(t.prim, TrackPrim::Seg { .. }))
        })
        .count();
    assert!(flat >= 10, "only {flat} arcs fell back to a segment");
    let arcs = r
        .items
        .iter()
        .filter(|i| matches!(i, Item::Track(t) if matches!(t.prim, TrackPrim::Arc { .. })))
        .count();
    assert!(arcs > 100, "{arcs} arcs");
}

#[test]
fn fuzz_deterministic_and_seed_sensitive() {
    let m = model();
    let o = Options::default();
    let j = |s| serde_json::to_string(&fuzz_items(&m, &o, s, 200)).unwrap();
    assert_eq!(j(5), j(5));
    assert_ne!(j(5), j(6));
}

#[test]
fn fuzz_attempt_cap_truncates() {
    let m = BoardModel::from_json(r#"{ "rules": {} }"#).unwrap();
    let r = fuzz_items(&m, &Options::default(), 1, 10);
    assert!(r.truncated && r.items.is_empty());
}
