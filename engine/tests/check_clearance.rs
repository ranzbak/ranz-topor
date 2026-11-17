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

/// B2 (KiCad drc_test_provider_edge_clearance.cpp `testAgainstEdge`: copper is collided with the
/// edge's EFFECTIVE shape, its stroke included, at `max(0, min - epsilon)`): the model's outline is
/// the stroke's centre line, so copper keeps `edge_clearance + outline_half_stroke` from it. With
/// a copper-to-edge clearance of 0 (wiccon, qmtech) a track may touch the stroke, not cross it.
#[test]
fn edge_clearance_counts_half_the_outline_stroke() {
    let mut m = model("");
    // track copper edge 30 µm inside the outline centre line (y = 20 mm): inside a 0.1 mm stroke
    let edge_track = |inside: i32| {
        let y: i32 = 20_000_000 - inside - 100_000;
        Item::Track(TrackItem { net: 1, layer: 0, width: 200_000, prim: TrackPrim::Seg { a: [-3_000_000, y].into(), b: [3_000_000, y].into() }, group: None, wire: None })
    };
    m.rules.edge_clearance = 0;
    m.rules.outline_half_stroke = 0;
    let o = Options::default();
    assert!(Checker::new(&m, &o).check_items(&[edge_track(30_000)]).violations.is_empty(), "0 = 0 without a stroke");
    m.rules.outline_half_stroke = 50_000;
    let v = Checker::new(&m, &o).check_items(&[edge_track(30_000)]).violations;
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(Checker::new(&m, &o).check_items(&[edge_track(60_000)]).violations.is_empty());
    // the stroke adds to a set clearance
    m.rules.edge_clearance = 200_000;
    assert_eq!(m.rules.edge_clear(), 250_000);
    assert_eq!(Checker::new(&m, &o).check_items(&[edge_track(240_000)]).violations.len(), 1);
    assert!(Checker::new(&m, &o).check_items(&[edge_track(260_000)]).violations.is_empty());
    // an ignored copper-to-edge check: the stroke is not added (KiCad does not test it)
    m.rules.ignored_checks.push("copper_edge_clearance".into());
    assert_eq!(m.rules.edge_clear(), 200_000);
}

/// Net-1 track of width 0.2 mm from (-3, y0) to (3, y0 + 1 nm): over the pad its centre line is
/// lowest at the pad's corner x = -0.5 mm, where it is `y0 + 2.5 / 6` nm high, so the copper gap
/// to the pad is `y0 - 600000 + 0.41667` nm: a fraction of a nm off a whole number.
fn slanted_above(y0: i32) -> (Item, f64) {
    let it = Item::Track(TrackItem { net: 1, layer: 0, width: 200_000, prim: TrackPrim::Seg { a: [-3_000_000, y0].into(), b: [3_000_000, y0 + 1].into() }, group: None, wire: None });
    // the exact gap: perpendicular distance from the corner to the centre line, less half the width
    let (dx, dy) = (6_000_000.0_f64, 1.0_f64);
    let d = ((y0 as f64 - 500_000.0) * dx - (-500_000.0 + 3_000_000.0) * dy).abs() / (dx * dx + dy * dy).sqrt();
    (it, d - 100_000.0)
}

/// erbs-margin (user 2026-11-17: "when we use floating point math to do this we'll never have an
/// exact match"): the checker passes a gap short of its requirement by at most
/// `CHECK_TOL_NM` (1 nm, far below KiCad's DRCEpsilon of 0.5 um) and rejects anything shorter:
/// a wire at the clearance + 0.42 nm, at it, at - 0.58 nm and at - 1 nm passes, at - 1.58 nm and
/// - 2 nm does not. The innermost ERBS of every showcase pattern was rejected at 200000 vs 200000
/// nm (short by < 0.5 nm) before (diag-bundle 2026-11-17).
#[test]
fn clearance_tolerance_is_a_fraction_of_a_nm() {
    use topor_engine::check::{short_of, CHECK_TOL_NM};
    let m = model("");
    let o = Options::default();
    let c = Checker::new(&m, &o);
    let req = 500_000.0;
    for (y0, ok) in [(1_100_000, true), (1_099_999, true), (1_099_998, false), (1_099_990, false)] {
        let (it, gap) = slanted_above(y0);
        assert!((gap - req).abs() < 11.0 && (gap - gap.round()).abs() > 0.4, "test geometry: gap {gap}");
        let v = c.check_items(&[it]).violations;
        assert_eq!(v.is_empty(), ok, "gap {gap:.4} nm vs {req} nm (short by {:.4}): {v:?}", req - gap);
    }
    // whole nm: at the clearance, 1 nm short (the tolerance, inclusive), 2 nm short
    assert!(c.check_items(&[track_above(500_000)]).violations.is_empty(), "exactly at the clearance");
    assert!(c.check_items(&[track_above(499_999)]).violations.is_empty(), "1 nm short: within the tolerance");
    assert_eq!(c.check_items(&[track_above(499_998)]).violations.len(), 1, "2 nm short");
    // the predicate: the tolerance is the constant, and none where the requirement is 0 (touching
    // copper of a 0 rule stays a violation, as KiCad's max(0, clearance - epsilon))
    assert_eq!(CHECK_TOL_NM, 1.0);
    assert!(!short_of(req - 0.5, req) && !short_of(req - 1.0, req) && short_of(req - 1.0001, req));
    assert!(!short_of(0.0, 0.0) && short_of(-0.5, 0.0) && !short_of(0.0, 0.5) && short_of(-0.1, 0.5) && !short_of(0.5, 0.5));
}
