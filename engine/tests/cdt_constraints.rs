use std::collections::HashSet;
use topor_engine::cdt::Cdt;
use topor_engine::geom::Vec2;
use topor_engine::model::{BoardModel, Options};
use topor_engine::obstacle::{ObsKind, ObstacleSet};
use topor_engine::sketch::{build_layer_cdt, ConstraintSet};

const MM: f64 = 1_000_000.0;

fn pts(p: &[(f64, f64)]) -> Vec<Vec2> {
    p.iter()
        .map(|&(x, y)| Vec2 {
            x: x * MM,
            y: y * MM,
        })
        .collect()
}

#[test]
fn crossing_constraints_are_split() {
    let c = Cdt::build(
        &pts(&[(0., 0.), (10., 0.), (5., -5.), (5., 5.)]),
        &[(0, 1), (2, 3)],
    )
    .unwrap();
    assert_eq!(c.num_vertices(), 5);
    assert_eq!(c.num_inputs(), 4);
    let p = c.vertex_pos(4);
    assert!((p.x - 5.0 * MM).abs() < 1.0 && p.y.abs() < 1.0);
    assert_eq!(c.constraint_edges(), vec![(0, 4), (1, 4), (2, 4), (3, 4)]);
    assert!(c.is_split_vertex(4));
    assert!((0..4).all(|v| !c.is_split_vertex(v)));
}

#[test]
fn collinear_overlapping_constraints_do_not_fail() {
    let c = Cdt::build(
        &pts(&[(0., 0.), (10., 0.), (4., 0.), (6., 0.)]),
        &[(0, 1), (2, 3)],
    )
    .unwrap();
    assert_eq!(c.num_vertices(), 4);
    assert_eq!(c.constraint_edges(), vec![(0, 2), (1, 3), (2, 3)]);
    assert!((0..4).all(|v| !c.is_split_vertex(v)));
}

#[test]
fn duplicate_constraint_is_idempotent() {
    let c = Cdt::build(
        &pts(&[(0., 0.), (10., 0.), (5., 5.)]),
        &[(0, 1), (0, 1), (1, 0)],
    )
    .unwrap();
    assert_eq!(c.constraint_edges(), vec![(0, 1)]);
}

#[test]
fn t_junction_uses_existing_vertex() {
    let c = Cdt::build(
        &pts(&[(0., 0.), (10., 0.), (5., 0.), (5., 5.)]),
        &[(0, 1), (2, 3)],
    )
    .unwrap();
    assert_eq!(c.num_vertices(), 4);
    assert_eq!(c.constraint_edges(), vec![(0, 2), (1, 2), (2, 3)]);
}

#[test]
fn zero_length_constraint_is_skipped() {
    let c = Cdt::build(&pts(&[(0., 0.), (10., 0.), (0., 0.)]), &[(0, 2)]).unwrap();
    assert!(c.constraint_edges().is_empty());
}

#[test]
fn non_finite_point_is_an_error() {
    let p = vec![
        Vec2 { x: 0.0, y: 0.0 },
        Vec2 {
            x: f64::NAN,
            y: 0.0,
        },
    ];
    assert!(Cdt::build(&p, &[]).is_err());
}

fn model(body: &str) -> BoardModel {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0 }}, {{ "id": 2 }} ], {body},
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#
    );
    BoardModel::from_json(&s).unwrap()
}

fn seg(a: (i64, i64), b: (i64, i64)) -> String {
    format!(
        r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
        a.0 * 1_000_000,
        a.1 * 1_000_000,
        b.0 * 1_000_000,
        b.1 * 1_000_000
    )
}

fn outline50x40() -> String {
    [
        ((0, 0), (50, 0)),
        ((50, 0), (50, 40)),
        ((50, 40), (0, 40)),
        ((0, 40), (0, 0)),
    ]
    .iter()
    .map(|&(a, b)| seg(a, b))
    .collect::<Vec<_>>()
    .join(",")
}

#[test]
fn routing_constraints_include_pad_hulls_and_kept_tracks() {
    // KiCad frame; the kept track runs into the pad hull (pad spans x 9.5..10.5).
    let body = format!(
        r#""pads": [ {{ "uuid": "p", "ref": "U1", "number": "1", "net": 1,
              "center": [10000000, 10000000], "layers": [0],
              "shapes": {{ "0": {{ "kind": "rect", "w": 1000000, "h": 2000000 }} }} }} ],
           "kept": [ {{ "uuid": "k1", "net": 2, "layer": 0, "width": 250000,
              "prim": {{ "kind": "seg", "a": [10300000, 10000000], "b": [20000000, 10000000] }} }},
              {{ "uuid": "k2", "net": 2, "layer": 0, "width": 250000,
              "prim": {{ "kind": "seg", "a": [30000000, 10000000], "b": [40000000, 10000000] }} }} ],
           "outline": [ {} ]"#,
        outline50x40()
    );
    let m = model(&body);
    let obs = ObstacleSet::build(&m, &Options::default(), &HashSet::new(), &[]);
    let l = obs.layer(0);
    let routing = build_layer_cdt(l, &[], ConstraintSet::Routing).unwrap();
    let edge_only = build_layer_cdt(l, &[], ConstraintSet::EdgeOnly).unwrap();
    let mut n_pad = 0;
    let mut n_kept = 0;
    for w in &l.walls {
        let (a, b) = (w.ends[0], w.ends[1]);
        let (ca, cb) = (routing.cdt.vertex_index(a), routing.cdt.vertex_index(b));
        match w.kind {
            ObsKind::Pad => {
                // the hull side pierced by kept:k1 is split into pieces (checked below)
                assert!(!edge_only.cdt.is_constraint(a, b));
                if routing.cdt.is_constraint(a, b) {
                    n_pad += 1;
                    let o = &routing.constraint_owner[&(ca.min(cb), ca.max(cb))];
                    assert_eq!((o.0, o.1, o.3.as_str()), (ObsKind::Pad, 1, "pad:U1:1"));
                }
            }
            ObsKind::Track if w.label == "kept:k2" => {
                n_kept += 1;
                assert!(routing.cdt.is_constraint(a, b));
                assert!(!edge_only.cdt.is_constraint(a, b));
                let o = &routing.constraint_owner[&(ca.min(cb), ca.max(cb))];
                assert_eq!(o.3, "kept:k2");
            }
            _ => {}
        }
    }
    assert_eq!((n_pad, n_kept), (3, 1));
    // The track k1 pierces the pad hull: its pieces are owned by the track, hull pieces by the pad.
    let mut kept_pieces = 0;
    let mut pad_pieces = 0;
    for (e, o) in &routing.constraint_owner {
        assert!(routing.cdt.constraint_edges().contains(e));
        match o.3.as_str() {
            "kept:k1" => kept_pieces += 1,
            "pad:U1:1" => pad_pieces += 1,
            _ => {}
        }
    }
    assert!(kept_pieces >= 2, "{kept_pieces}");
    assert!(pad_pieces >= 4, "{pad_pieces}");
    assert!((0..routing.cdt.num_vertices())
        .filter(|&v| routing.cdt.is_split_vertex(v))
        .all(|v| routing.point_to_anchor[v].is_none()));
    assert_eq!(routing.point_to_anchor.len(), routing.cdt.num_vertices());
    assert!(edge_only
        .constraint_owner
        .values()
        .all(|o| o.0 == ObsKind::Edge));
}

#[test]
fn layer_with_crossing_outline_builds() {
    let body = format!(
        r#""outline": [ {}, {} ]"#,
        seg((0, 0), (20, 20)),
        seg((0, 20), (20, 0))
    );
    let m = model(&body);
    let obs = ObstacleSet::build(&m, &Options::default(), &HashSet::new(), &[]);
    let ls = build_layer_cdt(obs.layer(0), &[], ConstraintSet::EdgeOnly).unwrap();
    assert_eq!(ls.cdt.constraint_edges().len(), 4);
    assert!(ls.constraint_owner.values().all(|o| o.3 == "edge"));
}
