use topor_engine::check::Checker;
use topor_engine::model::{BoardModel, Item, Options, PadRef, PadShape, Scope};

const MIN_MODEL: &str = r#"{
  "units": "nm", "board": "t",
  "layers": [ { "id": 0, "name": "F.Cu", "kind": "signal", "routable": true },
              { "id": 2, "name": "B.Cu", "kind": "signal", "routable": true } ],
  "nets": [ { "code": 1, "name": "+5V" } ],
  "pads": [ { "uuid": "p1", "ref": "U1", "number": "1", "net": 1, "attr": "pth",
              "center": [1000000, 2000000], "rotation_deg": 0.0, "layers": [0, 2],
              "shapes": { "0": { "kind": "circle", "d": 1600000 } },
              "drill": { "w": 800000, "h": 800000 }, "local_clearance": null, "net_tie_group": null } ],
  "kept": [ { "uuid": "k1", "net": 1, "layer": 0, "width": 250000,
              "prim": { "kind": "seg", "a": [0, 0], "b": [5000000, 0] }, "locked": true } ],
  "rules": {}
}"#;

fn circle_model(extra: &str, rules: &str) -> BoardModel {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0 }} ],
        "pads": [ {{ "uuid": "p", "ref": "U1", "number": "3", "net": 2, "center": [0, 0],
                    "layers": [0], "shapes": {{ "0": {{ "kind": "circle", "d": 1600000 }} }}
                    {extra} }} ],
        "rules": {rules} }}"#
    );
    BoardModel::from_json(&s).unwrap()
}

fn seg_item(net: i32, y: i64, w: i64) -> Item {
    serde_json::from_str(&format!(
        r#"{{ "net": {net}, "layer": 0, "width": {w},
             "prim": {{ "kind": "seg", "a": [-3000000, {y}], "b": [3000000, {y}] }}, "group": null }}"#
    ))
    .unwrap()
}

#[test]
fn model_roundtrip_minimal() {
    let m = BoardModel::from_json(MIN_MODEL).unwrap();
    assert_eq!(m.pads[0].shapes[&0], PadShape::Circle { d: 1_600_000 });
    assert_eq!(m.pads[0].center.x, 1_000_000);
    assert_eq!(m.pads[0].center.y, -2_000_000); // negated: math frame
    assert_eq!(m.kept.len(), 1);
    assert_eq!(m.rules.default.clearance, 200_000);
}

#[test]
fn model_with_slotted_hole_parses() {
    let m = BoardModel::from_json(
        r#"{ "holes": [ { "at": [10, 20], "d": 1000000, "kind": "pth", "slot": [1000000, 2000000] },
                         { "at": [1, 2], "d": 3200000, "kind": "npth" } ] }"#,
    )
    .unwrap();
    assert_eq!(m.holes[0].slot, Some([1_000_000, 2_000_000]));
    assert_eq!(m.holes[0].at.y, -20);
    assert_eq!(m.holes[1].slot, None);
}

#[test]
fn options_defaults() {
    let o = Options::from_json("{}").unwrap();
    assert_eq!(o.margin_nm, 5000);
    assert!(!o.partial_routes);
    assert_eq!(o.pathfinder_iter_cap, 50);
    assert_eq!(o.drc_loop_cap, 5);
    assert_eq!(o.wall_clock_s, 60.0);
    assert_eq!(
        o.existing_copper,
        topor_engine::model::ExistingCopper::Fixed
    );
    assert!(o.scope.is_whole_board());
    assert!(o.effective_threads() >= 1);
}

#[test]
fn scope_pad_ref_parses() {
    let o = Options::from_json(
        r#"{"scope":{"pads":["U1.3","J2.7"],"footprints":["R5"]},"existing_copper":"rip"}"#,
    )
    .unwrap();
    assert_eq!(
        o.scope.pads[0],
        PadRef {
            ref_: "U1".into(),
            number: "3".into()
        }
    );
    assert!(!o.scope.is_whole_board());
    let m = BoardModel::from_json(MIN_MODEL).unwrap();
    let mut pad = m.pads[0].clone();
    pad.number = "3".into();
    assert!(o.scope.selects_pad(&pad));
    pad.number = "4".into();
    assert!(!o.scope.selects_pad(&pad));
    pad.ref_ = "R5".into();
    assert!(o.scope.selects_pad(&pad));
    assert!(Scope::default().selects_pad(&pad));
    assert!(Options::from_json(r#"{"scope":{"pads":["U1"]}}"#).is_err());
}

#[test]
fn required_clearance_uses_local_override() {
    let m = circle_model(
        r#", "local_clearance": 300000"#,
        r#"{ "default": { "clearance": 200000 } }"#,
    );
    assert_eq!(m.required_clearance(1, &m.pads[0]), 300_000.0);
    let m = circle_model("", r#"{ "default": { "clearance": 200000 } }"#);
    assert_eq!(m.required_clearance(1, &m.pads[0]), 200_000.0);
}

// Pad radius 800 000, track half width 125 000, clearance 200 000 + margin 5 000:
// centreline distance 1 130 000 is exactly the requirement.
#[test]
fn checker_flags_gap_below_required() {
    let m = circle_model("", "{}");
    let c = Checker::new(&m, &Options::default());
    let rep = c.check_items(&[seg_item(1, 1_130_000 - 10, 250_000)]);
    assert_eq!(rep.checked, 1);
    assert_eq!(rep.violations.len(), 1);
    let v = &rep.violations[0];
    assert_eq!(v.against, "pad:U1:3");
    assert_eq!((v.gap, v.required), (204_990, 205_000));
    let rep = c.check_items(&[seg_item(1, 1_130_000 + 20, 250_000)]);
    assert!(rep.violations.is_empty());
}

#[test]
fn checker_same_net_pad_is_not_obstacle() {
    let m = circle_model("", "{}");
    let c = Checker::new(&m, &Options::default());
    assert!(c
        .check_items(&[seg_item(2, 0, 250_000)])
        .violations
        .is_empty());
    assert_eq!(
        c.check_items(&[seg_item(1, 0, 250_000)]).violations.len(),
        1
    );
}

#[test]
fn checker_net_tie_internal_copper_skipped_for_tie_nets() {
    let m = BoardModel::from_json(
        r#"{ "layers": [ { "id": 0 } ],
             "copper_shapes": [ { "uuid": "s", "net": 0, "layer": 0, "owner_ref": "TP1",
                 "poly": [[0,0],[1000000,0],[1000000,1000000],[0,1000000]] } ],
             "net_ties": [ { "ref": "TP1", "groups": [["1","2"]], "nets": [[1, 2]] } ] }"#,
    )
    .unwrap();
    let c = Checker::new(&m, &Options::default());
    let item = |net| {
        serde_json::from_str::<Item>(&format!(
            r#"{{ "net": {net}, "layer": 0, "width": 200000,
                 "prim": {{ "kind": "seg", "a": [-1000000, 500000], "b": [500000, -100000] }} }}"#
        ))
        .unwrap()
    };
    assert!(c.check_items(&[item(1)]).violations.is_empty());
    assert!(c.check_items(&[item(2)]).violations.is_empty());
    assert!(!c.check_items(&[item(3)]).violations.is_empty());
}

#[test]
fn checker_checks_earlier_items_and_holes_and_edge() {
    let m = BoardModel::from_json(
        r#"{ "layers": [ { "id": 0 } ],
             "holes": [ { "at": [0, 0], "d": 3200000, "kind": "npth" } ],
             "outline": [ { "kind": "seg", "a": [-9000000, 9000000], "b": [9000000, 9000000] } ] }"#,
    )
    .unwrap();
    let c = Checker::new(&m, &Options::default());
    // Item 1 runs 0.3 mm beside item 0 (same layer, different nets, 0.2 mm width).
    let a = seg_item(1, 5_000_000, 200_000);
    let b = seg_item(2, 5_300_000, 200_000); // gap 100 000 < 205 000
    let rep = c.check_items(&[a, b]);
    assert!(rep
        .violations
        .iter()
        .any(|v| v.against == "item:0" && v.item == 1));
    // Hole edge at 1.6 mm; track at y=1.7 mm w=0.2 -> gap 0 < 250 000 + 5 000.
    let rep = c.check_items(&[seg_item(1, 1_700_000, 200_000)]);
    assert!(rep
        .violations
        .iter()
        .any(|v| v.against.starts_with("hole:")));
    // Edge at y = 9 mm (KiCad frame): track 0.3 mm away < 0.5 mm + margin.
    let rep = c.check_items(&[seg_item(1, 8_600_000, 200_000)]);
    assert!(rep.violations.iter().any(|v| v.against == "edge"));
}

#[test]
fn checker_flags_via_item() {
    let m = circle_model("", "{}");
    let c = Checker::new(&m, &Options::default());
    let via: Item = serde_json::from_str(
        r#"{ "kind": "via", "net": 1, "at": [0, 1300000], "d": 600000, "drill": 300000,
             "layers": [0, 2], "group": null }"#,
    )
    .unwrap();
    let rep = c.check_items(&[via]);
    // 1.3 - 0.8 - 0.3 = 0.2 mm gap < 0.2 + margin
    assert_eq!(rep.violations.len(), 1);
    assert_eq!(rep.violations[0].gap, 200_000);
}

#[test]
fn checker_uses_grid_not_all_pairs() {
    let mut kept = String::new();
    for i in 0..10_000 {
        if i > 0 {
            kept.push(',');
        }
        let x = 100_000_000 + (i % 100) * 2_000_000;
        let y = 100_000_000 + (i / 100) * 2_000_000;
        kept.push_str(&format!(
            r#"{{ "uuid": "k{i}", "net": 1, "layer": 0, "width": 200000,
                 "prim": {{ "kind": "seg", "a": [{x}, {y}], "b": [{}, {y}] }} }}"#,
            x + 1_000_000
        ));
    }
    let m = BoardModel::from_json(&format!(
        r#"{{ "layers": [ {{ "id": 0 }} ], "kept": [{kept}] }}"#
    ))
    .unwrap();
    let c = Checker::new(&m, &Options::default());
    let rep = c.check_items(&[seg_item(2, 0, 200_000)]);
    assert!(
        rep.candidates_visited <= 100,
        "visited {}",
        rep.candidates_visited
    );
    assert!(rep.violations.is_empty());
}

// ---- fix round 1: containment in foreign copper, keepouts, via drill ----

fn tr(net: i32, layer: u32, a: [i64; 2], b: [i64; 2], w: i64) -> Item {
    serde_json::from_str(&format!(
        r#"{{ "net": {net}, "layer": {layer}, "width": {w},
             "prim": {{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }} }}"#,
        a[0], a[1], b[0], b[1]
    ))
    .unwrap()
}

const SQ: &str = "[[0,0],[10000000,0],[10000000,10000000],[0,10000000]]";

#[test]
fn checker_item_inside_foreign_shape_flagged() {
    let m = BoardModel::from_json(&format!(
        r#"{{ "layers": [ {{ "id": 0 }} ],
             "copper_shapes": [ {{ "uuid": "s", "net": 1, "layer": 0, "poly": {SQ} }} ] }}"#
    ))
    .unwrap();
    let c = Checker::new(&m, &Options::default());
    let inside = |net| {
        tr(
            net,
            0,
            [4_000_000, 5_000_000],
            [6_000_000, 5_000_000],
            200_000,
        )
    };
    let rep = c.check_items(&[inside(2)]);
    assert_eq!(rep.violations.len(), 1);
    assert_eq!(rep.violations[0].against, "shape:s");
    assert_eq!(rep.violations[0].gap, 0);
    assert!(c.check_items(&[inside(1)]).violations.is_empty());
}

fn keepout_model() -> BoardModel {
    BoardModel::from_json(&format!(
        r#"{{ "layers": [ {{ "id": 0 }}, {{ "id": 2 }} ],
             "keepouts": [ {{ "name": "ko", "layers": [0], "poly": {SQ},
                              "no_tracks": true, "no_vias": true }} ] }}"#
    ))
    .unwrap()
}

#[test]
fn checker_keepout_tracks() {
    let c = Checker::new(&keepout_model(), &Options::default());
    let inside = tr(
        1,
        0,
        [4_000_000, 5_000_000],
        [6_000_000, 5_000_000],
        200_000,
    );
    let rep = c.check_items(&[inside]);
    assert_eq!(rep.violations.len(), 1);
    assert_eq!(rep.violations[0].against, "keepout:ko");
    let crossing = tr(
        1,
        0,
        [-5_000_000, 5_000_000],
        [5_000_000, 5_000_000],
        200_000,
    );
    assert_eq!(c.check_items(&[crossing]).violations.len(), 1);
    let outside = tr(
        1,
        0,
        [-5_000_000, -3_000_000],
        [15_000_000, -3_000_000],
        200_000,
    );
    assert!(c.check_items(&[outside]).violations.is_empty());
    // layer 2 is not covered
    let other = tr(
        1,
        2,
        [4_000_000, 5_000_000],
        [6_000_000, 5_000_000],
        200_000,
    );
    assert!(c.check_items(&[other]).violations.is_empty());
}

#[test]
fn checker_keepout_via_and_flags() {
    let c = Checker::new(&keepout_model(), &Options::default());
    let via: Item = serde_json::from_str(
        r#"{ "kind": "via", "net": 1, "at": [5000000, 5000000], "d": 600000, "drill": 300000,
             "layers": [0, 2] }"#,
    )
    .unwrap();
    assert_eq!(c.check_items(&[via]).violations.len(), 1);
    // no_vias only: tracks pass
    let m = BoardModel::from_json(&format!(
        r#"{{ "layers": [ {{ "id": 0 }} ],
             "keepouts": [ {{ "name": "v", "layers": [0], "poly": {SQ}, "no_vias": true }} ] }}"#
    ))
    .unwrap();
    let c = Checker::new(&m, &Options::default());
    let t = tr(
        1,
        0,
        [4_000_000, 5_000_000],
        [6_000_000, 5_000_000],
        200_000,
    );
    assert!(c.check_items(&[t]).violations.is_empty());
}

#[test]
fn checker_via_drill_to_hole() {
    let m = BoardModel::from_json(
        r#"{ "layers": [ { "id": 0 }, { "id": 2 } ],
             "holes": [ { "at": [0, 0], "d": 1000000, "kind": "npth" } ] }"#,
    )
    .unwrap();
    let o = Options {
        margin_nm: 0,
        ..Options::default()
    };
    let c = Checker::new(&m, &o);
    let mk = |y: i64, drill: i64| -> Item {
        serde_json::from_str(&format!(
            r#"{{ "kind": "via", "net": 1, "at": [0, {y}], "d": 400000, "drill": {drill},
                 "layers": [0, 2] }}"#
        ))
        .unwrap()
    };
    // copper gap 0.5 mm, drill gap 0.6 mm: both fine with the defaults
    assert!(c
        .check_items(&[mk(1_200_000, 200_000)])
        .violations
        .is_empty());
    // hole_to_hole larger than the copper hole clearance: only the drill check fires.
    let m2 = BoardModel::from_json(
        r#"{ "layers": [ { "id": 0 }, { "id": 2 } ],
             "holes": [ { "at": [0, 0], "d": 1000000, "kind": "npth" } ],
             "rules": { "hole_to_hole": 600000, "hole_clearance": 100000 } }"#,
    )
    .unwrap();
    let c2 = Checker::new(&m2, &o);
    // copper gap 0.5 >= 0.1 ok; drill gap 1.2-0.5-0.1 = 0.6 -> borderline ok; at 1.1: 0.5 < 0.6
    let rep = c2.check_items(&[mk(1_100_000, 200_000)]);
    // one violation per layer (F.Cu, B.Cu), all against the hole
    assert_eq!(rep.violations.len(), 2, "{:?}", rep.violations);
    assert!(rep.violations.iter().all(|v| v.against == "hole:0,0"));
    assert_eq!(rep.violations[0].required, 600_000);
}

#[test]
fn pour_fills_and_via_options_round_trip() {
    let m = BoardModel::from_json(
        r#"{ "pours": [ { "uuid": "z", "net": 1, "layers": [2],
             "fills": [ { "layer": 2, "outer": [[1000, 2000], [3000, -4000], [5, 6]] } ] } ],
             "rules": {} }"#,
    )
    .unwrap();
    let f = &m.pours[0].fills[0];
    assert_eq!(f.layer, 2);
    assert_eq!(f.outer.len(), 3);
    assert_eq!((f.outer[0].x, f.outer[0].y), (1000, -2000));
    assert_eq!((f.outer[1].x, f.outer[1].y), (3000, 4000));
    let o = Options::from_json("{}").unwrap();
    assert_eq!(o.via_cost_nm, 2_000_000);
    assert_eq!(o.max_vias_per_connection, 4);
}
