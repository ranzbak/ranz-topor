use std::collections::HashSet;
use topor_engine::geom::{Aabb, Vec2};
use topor_engine::model::{BoardModel, Options};
use topor_engine::obstacle::{Core, ObsKind, ObstacleSet, TerminalObstacle};

const W: f64 = 250_000.0;
const MM: f64 = 1_000_000.0;

fn rules() -> &'static str {
    r#"{ "default": { "clearance": 200000, "width": 250000 }, "min_clearance": 0,
         "hole_clearance": 250000, "edge_clearance": 500000 }"#
}

fn model(body: &str) -> BoardModel {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0 }}, {{ "id": 2 }} ], {body}, "rules": {} }}"#,
        rules()
    );
    BoardModel::from_json(&s).unwrap()
}

fn build(m: &BoardModel) -> ObstacleSet {
    ObstacleSet::build(m, &Options::default(), &HashSet::new(), &[])
}

fn pad(net: i32, extra: &str, shapes: &str, layers: &str) -> String {
    format!(
        r#""pads": [ {{ "uuid": "p", "ref": "U1", "number": "1", "net": {net},
            "center": [10000000, 10000000], "layers": {layers},
            "shapes": {shapes} {extra} }} ]"#
    )
}

/// KiCad-frame mm to math-frame nm.
fn kc(x: f64, y: f64) -> Vec2 {
    Vec2 {
        x: x * MM,
        y: -y * MM,
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1.0
}

fn has_pt(set: &ObstacleSet, layer: u32, p: Vec2) -> bool {
    set.layer(layer)
        .anchors
        .iter()
        .any(|a| near(a.c.x, p.x) && near(a.c.y, p.y))
}

#[test]
fn circle_pad_one_anchor() {
    let m = model(&pad(
        2,
        "",
        r#"{ "0": { "kind": "circle", "d": 1600000 } }"#,
        "[0]",
    ));
    let s = build(&m);
    let l = s.layer(0);
    assert_eq!(l.anchors.len(), 1);
    assert!(l.walls.is_empty());
    assert!(near(s.rho_anchor(0, 0, 3, W), 1_130_000.0));
    assert_eq!(l.anchors[0].kind, ObsKind::Pad);
    assert_eq!(l.anchors[0].label, "pad:U1:1");
}

#[test]
fn rect_pad_four_corner_anchors_and_walls() {
    for rot in [0.0, 90.0] {
        let m = model(&pad(
            2,
            &format!(r#", "rotation_deg": {rot}"#),
            r#"{ "0": { "kind": "rect", "w": 1600000, "h": 1600000 } }"#,
            "[0]",
        ));
        let s = build(&m);
        let l = s.layer(0);
        assert_eq!(l.anchors.len(), 4);
        assert_eq!(l.walls.len(), 4);
        for (x, y) in [(9.2, 9.2), (10.8, 9.2), (10.8, 10.8), (9.2, 10.8)] {
            assert!(has_pt(&s, 0, kc(x, y)), "corner {x},{y} rot {rot}");
        }
        assert!(l.anchors.iter().all(|a| a.r_core == 0.0));
        assert!(near(s.rho_anchor(0, 0, 3, W), 330_000.0));
        for w in &l.walls {
            assert_ne!(w.ends[0], w.ends[1]);
        }
    }
}

#[test]
fn roundrect_anchors_carry_corner_radius() {
    let m = model(&pad(
        2,
        "",
        r#"{ "0": { "kind": "roundrect", "w": 2000000, "h": 1000000, "rc": 250000 } }"#,
        "[0]",
    ));
    let s = build(&m);
    let l = s.layer(0);
    assert_eq!(l.anchors.len(), 4);
    for (dx, dy) in [(-0.75, -0.25), (0.75, -0.25), (0.75, 0.25), (-0.75, 0.25)] {
        assert!(has_pt(&s, 0, kc(10.0 + dx, 10.0 + dy)));
    }
    assert!(l.anchors.iter().all(|a| near(a.r_core, 250_000.0)));
    assert!(near(s.rho_anchor(0, 0, 3, W), 580_000.0));
}

#[test]
fn oval_two_anchors_square_oval_one() {
    let m = model(&pad(
        2,
        "",
        r#"{ "0": { "kind": "oval", "w": 1700000, "h": 1000000 } }"#,
        "[0]",
    ));
    let s = build(&m);
    let l = s.layer(0);
    assert_eq!(l.anchors.len(), 2);
    assert_eq!(l.walls.len(), 1);
    assert!(has_pt(&s, 0, kc(10.35, 10.0)) && has_pt(&s, 0, kc(9.65, 10.0)));
    assert!(near(l.anchors[0].r_core, 500_000.0));
    assert!(near(s.rho_anchor(0, 0, 3, W), 830_000.0));
    let m = model(&pad(
        2,
        "",
        r#"{ "0": { "kind": "oval", "w": 1700000, "h": 1700000 } }"#,
        "[0]",
    ));
    let s = build(&m);
    assert_eq!(s.layer(0).anchors.len(), 1);
    assert!(near(s.rho_anchor(0, 0, 3, W), 1_180_000.0));
}

#[test]
fn poly_pad_anchor_per_vertex() {
    let m = model(&pad(
        2,
        "",
        r#"{ "0": { "kind": "poly", "pts": [[9000000,9000000],[11000000,9000000],[10800000,11000000],[9200000,11000000]] } }"#,
        "[0]",
    ));
    let s = build(&m);
    assert_eq!(s.layer(0).anchors.len(), 4);
    assert_eq!(s.layer(0).walls.len(), 4);
}

#[test]
fn same_net_and_net_tie_are_exempt() {
    let m = model(&pad(
        3,
        "",
        r#"{ "0": { "kind": "circle", "d": 1600000 } }"#,
        "[0]",
    ));
    let s = build(&m);
    let a = &s.layer(0).anchors[0];
    assert!(ObstacleSet::exempt(a.net, &a.tie_nets, 3));
    assert!(!ObstacleSet::exempt(a.net, &a.tie_nets, 2));
    assert!(!ObstacleSet::exempt(0, &[], 0));

    let body = format!(
        r#"{}, "net_ties": [ {{ "ref": "U1", "groups": [], "nets": [[2, 4]] }} ]"#,
        pad(
            2,
            r#", "net_tie_group": 0"#,
            r#"{ "0": { "kind": "circle", "d": 1600000 } }"#,
            "[0]",
        )
    );
    let s = build(&model(&body));
    let a = &s.layer(0).anchors[0];
    assert!(ObstacleSet::exempt(a.net, &a.tie_nets, 4));
    assert!(!ObstacleSet::exempt(a.net, &a.tie_nets, 5));
}

#[test]
fn per_layer_padstack_differs() {
    let m = model(&pad(
        2,
        "",
        r#"{ "0": { "kind": "circle", "d": 1600000 }, "2": { "kind": "rect", "w": 1000000, "h": 1000000 } }"#,
        "[0, 2]",
    ));
    let s = build(&m);
    assert_eq!(s.layer(0).anchors.len(), 1);
    assert_eq!(s.layer(2).anchors.len(), 4);
}

#[test]
fn hole_and_edge_use_their_own_clearance() {
    let outline: Vec<String> = [
        ((0, 0), (50, 0)),
        ((50, 0), (50, 40)),
        ((50, 40), (0, 40)),
        ((0, 40), (0, 0)),
    ]
    .iter()
    .map(|((ax, ay), (bx, by))| {
        format!(
            r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
            ax * 1_000_000,
            ay * 1_000_000,
            bx * 1_000_000,
            by * 1_000_000
        )
    })
    .collect();
    let body = format!(
        r#""holes": [ {{ "at": [20000000, 20000000], "d": 3000000, "kind": "npth" }} ],
           "outline": [ {} ]"#,
        outline.join(",")
    );
    let s = build(&model(&body));
    let l = s.layer(0);
    let h = l
        .anchors
        .iter()
        .position(|a| a.kind == ObsKind::Hole)
        .unwrap();
    assert!(near(s.rho_anchor(0, h, 3, W), 1_880_000.0));
    assert_eq!(l.anchors[h].net, 0);
    let edge_walls: Vec<usize> = (0..l.walls.len())
        .filter(|&i| l.walls[i].kind == ObsKind::Edge)
        .collect();
    assert_eq!(edge_walls.len(), 4);
    assert_eq!(
        l.anchors.iter().filter(|a| a.kind == ObsKind::Edge).count(),
        4
    );
    assert!(near(s.rho_wall(0, edge_walls[0], 3, W), 630_000.0));
    assert!(near(s.rho_wall(0, edge_walls[0], 3, 400_000.0), 705_000.0));
    // holes and edges exist on every layer
    assert_eq!(s.layer(2).walls.len(), 4);
}

#[test]
fn pth_hole_belongs_to_pad_net() {
    let body = format!(
        r#"{}, "holes": [ {{ "at": [10000000, 10000000], "d": 1000000, "kind": "pth" }} ]"#,
        pad(
            2,
            r#", "attr": "pth", "drill": { "w": 1000000, "h": 1000000 }"#,
            r#"{ "0": { "kind": "circle", "d": 1600000 }, "2": { "kind": "circle", "d": 1600000 } }"#,
            "[0, 2]",
        )
    );
    let s = build(&model(&body));
    let a = s
        .layer(0)
        .anchors
        .iter()
        .find(|a| a.kind == ObsKind::Hole)
        .unwrap();
    assert_eq!(a.net, 2);
    assert!(ObstacleSet::exempt(a.net, &a.tie_nets, 2));
    assert!(!ObstacleSet::exempt(a.net, &a.tie_nets, 3));
}

#[test]
fn slot_hole_is_wall_with_two_anchors() {
    let m = model(
        r#""holes": [ { "at": [1000000, 1000000], "d": 1000000, "kind": "npth", "slot": [3000000, 1000000] } ]"#,
    );
    let s = build(&m);
    let l = s.layer(0);
    assert_eq!(l.anchors.len(), 2);
    assert_eq!(l.walls.len(), 1);
    assert!(matches!(l.walls[0].core, Core::Seg(_)));
    assert!(near(l.anchors[0].r_core, 500_000.0));
}

#[test]
fn via_is_copper_anchor_and_hole_anchor() {
    let m = BoardModel::from_json(&format!(
        r#"{{ "layers": [ {{ "id": 0 }}, {{ "id": 1 }}, {{ "id": 2 }} ],
             "vias": [ {{ "uuid": "v", "net": 2, "at": [5000000, 5000000], "d": 600000,
                          "drill": 300000, "layers": [0, 2] }} ], "rules": {} }}"#,
        rules()
    ))
    .unwrap();
    let s = build(&m);
    assert_eq!(s.layers(), vec![0, 1, 2]);
    let l = s.layer(1);
    let via = l.anchors.iter().find(|a| a.kind == ObsKind::Via).unwrap();
    let hole = l.anchors.iter().find(|a| a.kind == ObsKind::Hole).unwrap();
    assert!(near(via.r_core, 300_000.0) && via.net == 2);
    assert!(near(hole.r_core, 150_000.0) && hole.net == 2);
}

#[test]
fn kept_track_is_wall_unless_excluded() {
    let m = model(
        r#""kept": [ { "uuid": "k1", "net": 2, "layer": 0, "width": 250000,
             "prim": { "kind": "seg", "a": [0, 0], "b": [5000000, 0] } } ]"#,
    );
    let s = build(&m);
    let l = s.layer(0);
    assert_eq!(l.walls.len(), 1);
    assert_eq!(l.anchors.len(), 2);
    assert_eq!(l.walls[0].kind, ObsKind::Track);
    assert!(near(l.walls[0].r_core, 125_000.0));
    let e = l.walls[0].ends;
    assert!(e[0] < 2 && e[1] < 2 && e[0] != e[1]);
    let ex: HashSet<String> = ["k1".to_string()].into();
    let s = ObstacleSet::build(&m, &Options::default(), &ex, &[]);
    assert!(s.layer(0).walls.is_empty() && s.layer(0).anchors.is_empty());
}

#[test]
fn kept_arc_is_arc_core() {
    let m = model(
        r#""kept": [ { "uuid": "k", "net": 2, "layer": 0, "width": 250000,
             "prim": { "kind": "arc", "start": [0, 0], "mid": [1000000, 1000000], "end": [2000000, 0] } } ]"#,
    );
    let s = build(&m);
    assert!(matches!(s.layer(0).walls[0].core, Core::Arc(_)));
}

#[test]
fn copper_shape_polygon_and_tie_owner() {
    let poly = r#""poly": [[0,0],[3000000,0],[0,3000000]]"#;
    let body = format!(r#""copper_shapes": [ {{ "uuid": "s", "net": 0, "layer": 0, {poly} }} ]"#);
    let s = build(&model(&body));
    let l = s.layer(0);
    assert_eq!((l.anchors.len(), l.walls.len()), (3, 3));
    let a = &l.anchors[0];
    assert!(!ObstacleSet::exempt(a.net, &a.tie_nets, 3));
    assert!(!ObstacleSet::exempt(a.net, &a.tie_nets, 0));

    let body = format!(
        r#""copper_shapes": [ {{ "uuid": "s", "net": 0, "layer": 0, {poly}, "owner_ref": "NT1" }} ],
           "net_ties": [ {{ "ref": "NT1", "groups": [], "nets": [[2, 4]] }} ]"#
    );
    let s = build(&model(&body));
    let a = &s.layer(0).anchors[0];
    assert!(ObstacleSet::exempt(a.net, &a.tie_nets, 2));
    assert!(ObstacleSet::exempt(a.net, &a.tie_nets, 4));
}

#[test]
fn terminal_obstacle_for_other_nets_only() {
    let m = model(r#""nets": []"#);
    let t = [TerminalObstacle {
        layer: 0,
        at: Vec2 { x: 1e6, y: 2e6 },
        net: 3,
        width: 400_000,
        label: "t".into(),
    }];
    let s = ObstacleSet::build(&m, &Options::default(), &HashSet::new(), &t);
    let a = &s.layer(0).anchors[0];
    assert_eq!(a.kind, ObsKind::Terminal);
    assert!(near(a.r_core, 200_000.0));
    assert!(ObstacleSet::exempt(a.net, &a.tie_nets, 3));
    assert!(!ObstacleSet::exempt(a.net, &a.tie_nets, 2));
}

#[test]
fn pad_local_clearance_raises_own_clr() {
    let m = model(&pad(
        2,
        r#", "local_clearance": 300000"#,
        r#"{ "0": { "kind": "circle", "d": 1600000 } }"#,
        "[0]",
    ));
    let s = build(&m);
    assert!(near(s.layer(0).anchors[0].own_clr, 300_000.0));
    assert!(near(
        s.rho_anchor(0, 0, 3, W),
        800_000.0 + 300_000.0 + 125_000.0 + 5_000.0
    ));
}

#[test]
fn query_returns_only_nearby() {
    let pads: Vec<String> = (0..1000)
        .map(|i| {
            format!(
                r#"{{ "uuid": "p{i}", "ref": "U", "number": "{i}", "net": 2,
                    "center": [{}, {}], "layers": [0],
                    "shapes": {{ "0": {{ "kind": "circle", "d": 500000 }} }} }}"#,
                (i % 40) * 1_000_000,
                (i / 40) * 1_000_000
            )
        })
        .collect();
    let m = model(&format!(r#""pads": [ {} ]"#, pads.join(",")));
    let s = build(&m);
    let bb = Aabb {
        min: Vec2 {
            x: 10.5e6,
            y: -12.5e6,
        },
        max: Vec2 {
            x: 12.5e6,
            y: -10.5e6,
        },
    };
    let q = s.query_anchors(0, bb);
    assert!(!q.is_empty() && q.len() <= 16, "got {}", q.len());
    assert!(s.query_walls(0, bb).is_empty());
    let all = Aabb {
        min: Vec2 { x: -1e9, y: -1e9 },
        max: Vec2 { x: 1e9, y: 1e9 },
    };
    assert_eq!(s.query_anchors(0, all).len(), 1000);
}

#[test]
fn track_keepout_is_zero_clearance_polygon_for_every_net() {
    let poly = r#""poly": [[0,0],[3000000,0],[3000000,3000000],[0,3000000]]"#;
    let body = format!(
        r#""keepouts": [ {{ "name": "ko", "layers": [0], "no_tracks": true, {poly} }},
                         {{ "name": "vo", "layers": [0], "no_vias": true, {poly} }} ]"#
    );
    let s = build(&model(&body));
    let l = s.layer(0);
    assert_eq!((l.anchors.len(), l.walls.len()), (4, 4));
    assert!(l
        .walls
        .iter()
        .all(|w| w.kind == ObsKind::Keepout && w.label == "keepout:ko"));
    let a = &l.anchors[0];
    assert!(
        !ObstacleSet::exempt(a.net, &a.tie_nets, 0) && !ObstacleSet::exempt(a.net, &a.tie_nets, 5)
    );
    assert!(near(s.rho_wall(0, 0, 5, W), 125_000.0 + 5_000.0));
    assert!(s.layer(2).walls.is_empty());
}
