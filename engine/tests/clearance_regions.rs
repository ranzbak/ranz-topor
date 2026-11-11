//! Custom clearance regions (`.kicad_dru` courtyard / rule-area rules, `Rules::clearance_regions`)
//! in the checker: a copper pair of which either item matches a region needs the region's
//! clearance instead of the nets' (KiCad `DRC_ENGINE::EvalRules`: the matching custom rule wins,
//! smaller or larger; `drc_rule_condition.cpp` tries `(A, B)` and `(B, A)`).
use topor_engine::check::Checker;
use topor_engine::geom::TrackPrim;
use topor_engine::model::{BoardModel, Item, Options, TrackItem};

/// A 1 x 1 mm net-2 pad at the origin whose class needs 0.5 mm; items are net 1 (0.2 mm).
fn model(regions: &str) -> BoardModel {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "routable": true }}, {{ "id": 2, "routable": true }} ],
            "nets": [ {{ "code": 1, "name": "A" }}, {{ "code": 2, "name": "B" }} ],
            "pads": [ {{ "uuid": "pb", "ref": "U1", "number": "1", "net": 2, "attr": "smd",
                 "center": [0, 0], "rotation_deg": 0.0, "layers": [0],
                 "shapes": {{ "0": {{ "kind": "rect", "w": 1000000, "h": 1000000 }} }} }} ],
            "rules": {{ "per_net": {{ "2": {{ "clearance": 500000 }} }}, "clearance_regions": [{regions}] }} }}"#
    );
    BoardModel::from_json(&s).unwrap()
}

fn square(c: (i64, i64), half: i64) -> String {
    let (x, y) = c;
    format!("[[{},{}],[{},{}],[{},{}],[{},{}]]", x - half, y - half, x + half, y - half, x + half, y + half, x - half, y + half)
}

fn region(poly: &str, clearance: i64, extra: &str) -> String {
    format!(r#"{{ "name": "r", "ref": "U1", "poly": {poly}, "clearance": {clearance}{extra} }}"#)
}

/// Horizontal net-1 track, 0.2 mm wide, its copper `gap` nm above the pad's top edge, from x0 to x1.
fn track(gap: i32, x0: i32, x1: i32) -> Item {
    let y: i32 = 500_000 + gap + 100_000;
    Item::Track(TrackItem { net: 1, layer: 0, width: 200_000, prim: TrackPrim::Seg { a: [x0, y].into(), b: [x1, y].into() }, group: None, wire: None })
}

fn n_viol(m: &BoardModel, it: Item) -> usize {
    Checker::new(m, &Options::default()).check_items(&[it]).violations.len()
}

#[test]
fn no_region_keeps_the_net_class_clearance() {
    assert_eq!(n_viol(&model(""), track(300_000, -3_000_000, 3_000_000)), 1);
}

#[test]
fn a_looser_region_around_the_pad_lets_the_track_closer() {
    // the pad touches the courtyard: every pair with it needs 0.2 mm (bornhack neck_down)
    let m = model(&region(&square((0, 0), 800_000), 200_000, ""));
    assert_eq!(n_viol(&m, track(300_000, -3_000_000, 3_000_000)), 0);
    assert_eq!(n_viol(&m, track(150_000, -3_000_000, 3_000_000)), 1);
}

#[test]
fn a_region_elsewhere_changes_nothing() {
    let m = model(&region(&square((10_000_000, 10_000_000), 800_000), 200_000, ""));
    assert_eq!(n_viol(&m, track(300_000, -3_000_000, 3_000_000)), 1);
}

#[test]
fn a_stricter_region_the_track_touches_applies_to_the_pair() {
    // only the track's far end lies in the region; the pair still needs 0.8 mm
    let m = model(&region(&square((2_900_000, 1_200_000), 400_000), 800_000, ""));
    assert_eq!(n_viol(&m, track(600_000, -3_000_000, 3_000_000)), 1);
    // a track that stays out of it keeps the 0.5 mm of the pad class: 0.6 mm is clean
    assert_eq!(n_viol(&m, track(600_000, -3_000_000, 2_000_000)), 0);
}

#[test]
fn enclosed_needs_the_item_wholly_inside() {
    // the region encloses the pad: the pad matches, a track that leaves the region does not need to
    let around = square((0, 0), 800_000);
    let m = model(&region(&around, 200_000, r#", "enclosed": true"#));
    assert_eq!(n_viol(&m, track(300_000, -3_000_000, 3_000_000)), 0, "the pad lies wholly inside");
    let half = "[[-800000,-800000],[0,-800000],[0,800000],[-800000,800000]]";
    let m2 = model(&region(half, 200_000, r#", "enclosed": true"#));
    assert_eq!(n_viol(&m2, track(300_000, -3_000_000, 3_000_000)), 1, "the pad is only partly inside");
}

#[test]
fn a_region_on_another_layer_does_not_apply() {
    let m = model(&region(&square((0, 0), 800_000), 200_000, r#", "layers": [2]"#));
    assert_eq!(n_viol(&m, track(300_000, -3_000_000, 3_000_000)), 1);
}

#[test]
fn the_later_region_wins() {
    let a = region(&square((0, 0), 800_000), 200_000, "");
    let b = region(&square((0, 0), 800_000), 400_000, "");
    assert_eq!(n_viol(&model(&format!("{a},{b}")), track(300_000, -3_000_000, 3_000_000)), 1);
    assert_eq!(n_viol(&model(&format!("{b},{a}")), track(300_000, -3_000_000, 3_000_000)), 0);
}
