use proptest::prelude::*;
use topor_engine::geom::{
    all_bitangents, bitangent, point_circle_tangent, Arc, Circle, GeomError, Orient, Vec2,
};

fn vec2() -> impl Strategy<Value = Vec2> {
    (
        -50_000_000.0f64..50_000_000.0,
        -50_000_000.0f64..50_000_000.0,
    )
        .prop_map(|(x, y)| Vec2 { x, y })
}
fn circle() -> impl Strategy<Value = Circle> {
    (vec2(), 50_000.0f64..5_000_000.0).prop_map(|(c, r)| Circle { c, r })
}
fn orient() -> impl Strategy<Value = Orient> {
    prop_oneof![Just(Orient::Ccw), Just(Orient::Cw)]
}

proptest! {
  #[test] fn prop_point_tangent_is_perpendicular(p in vec2(), c in circle()) {
    prop_assume!((p - c.c).len() > c.r * 1.001);
    for s in [Orient::Ccw, Orient::Cw] {
      let t = point_circle_tangent(p, c, s).unwrap();
      let scale = 1e8; // coordinate magnitude bound
      prop_assert!(((t - c.c).len() - c.r).abs() < 1e-9 * scale);
      prop_assert!((t - c.c).dot(t - p).abs() < 1e-9 * (t - c.c).len() * (t - p).len());
      prop_assert!(s.sign() * (t - p).cross(c.c - t) > 0.0);
    }
  }
  #[test] fn prop_bitangent_touches_both_and_respects_sigma(c1 in circle(), c2 in circle(), s1 in orient(), s2 in orient()) {
    prop_assume!((c2.c - c1.c).len() > c1.r + c2.r + 1000.0);
    let seg = bitangent(c1, s1, c2, s2).unwrap();
    let t = (seg.b - seg.a).unit().unwrap();
    let scale = 1e8;
    prop_assert!(((seg.a - c1.c).len() - c1.r).abs() < 1e-9 * scale && ((seg.b - c2.c).len() - c2.r).abs() < 1e-9 * scale);
    prop_assert!(s1.sign() * t.cross(c1.c - seg.a) > 0.0 && s2.sign() * t.cross(c2.c - seg.b) > 0.0);
    prop_assert!((seg.a - c1.c).dot(t).abs() < 1e-9 * scale);
    prop_assert!((seg.b - c2.c).dot(t).abs() < 1e-9 * scale);
    // distance from each centre to the line equals its radius
    prop_assert!((t.cross(c1.c - seg.a).abs() - c1.r).abs() < 1e-9 * scale);
    prop_assert!((t.cross(c2.c - seg.a).abs() - c2.r).abs() < 1e-9 * scale);
  }
  #[test] fn prop_inner_bitangent_infeasible_when_overlapping(c1 in circle(), r2 in 50_000.0f64..5_000_000.0, f in 0.0f64..1.0, ang in 0.0f64..std::f64::consts::TAU) {
    // place c2 so that |r1 - r2| + 1000 < d < r1 + r2 - 1000 (overlapping, not nested)
    let (lo, hi) = ((c1.r - r2).abs() + 1000.0, c1.r + r2 - 1000.0);
    prop_assume!(hi > lo + 1.0);
    let d = lo + f * (hi - lo);
    let c2 = Circle { c: c1.c + Vec2 { x: d * ang.cos(), y: d * ang.sin() }, r: r2 };
    prop_assert!(matches!(bitangent(c1, Orient::Ccw, c2, Orient::Cw), Err(GeomError::Infeasible)));
    prop_assert!(bitangent(c1, Orient::Ccw, c2, Orient::Ccw).is_ok());
  }
  #[test] fn prop_arc_from_tangent_points_roundtrip(c in circle(), a1 in 0.0f64..std::f64::consts::TAU, a2 in 0.0f64..std::f64::consts::TAU, s in orient()) {
    let t_in = c.c + Vec2{x: c.r*a1.cos(), y: c.r*a1.sin()};
    let t_out = c.c + Vec2{x: c.r*a2.cos(), y: c.r*a2.sin()};
    let arc = Arc::from_tangent_points(c.c, c.r, s, t_in, t_out);
    prop_assert!((arc.start() - t_in).len() < 1e-3 && (arc.end() - t_out).len() < 1e-3);
    prop_assert!(arc.sweep >= 0.0 && arc.sweep < std::f64::consts::TAU);
    prop_assert!(((arc.mid() - c.c).len() - c.r).abs() < 1e-3);
    prop_assert!(arc.contains_dir(arc.mid()) && (arc.length() - c.r*arc.sweep).abs() < 1e-6);
  }
  #[test] fn prop_three_points_roundtrip(c in circle(), a in 0.0f64..std::f64::consts::TAU, sweep in 0.1f64..6.0, s in orient()) {
    let arc = Arc { c: c.c, r: c.r, sigma: s, a_start: a, sweep };
    let got = Arc::from_three_points(arc.start(), arc.mid(), arc.end()).unwrap();
    prop_assert!((got.c - c.c).len() < 1e-2 * (1.0 + c.r / 1e6) && (got.r - c.r).abs() < 1e-2 * (1.0 + c.r / 1e6));
    prop_assert!(got.sigma == s && (got.sweep - sweep).abs() < 1e-6);
  }
  #[test] fn prop_arc_bbox_contains_samples(c in circle(), a in 0.0f64..std::f64::consts::TAU, sweep in 0.1f64..6.0, s in orient()) {
    let arc = Arc { c: c.c, r: c.r, sigma: s, a_start: a, sweep };
    let bb = arc.bbox().grow(1e-3);
    for i in 0..=16 {
      let p = arc.point_at(i as f64 / 16.0);
      prop_assert!(p.x >= bb.min.x && p.x <= bb.max.x && p.y >= bb.min.y && p.y <= bb.max.y);
    }
  }
}

#[test]
fn example_four_bitangents_of_disjoint_circles() {
    let c1 = Circle {
        c: Vec2 { x: 0.0, y: 0.0 },
        r: 1_000_000.0,
    };
    let c2 = Circle {
        c: Vec2 {
            x: 5_000_000.0,
            y: 0.0,
        },
        r: 1_000_000.0,
    };
    let all = all_bitangents(c1, c2);
    assert_eq!(all.len(), 4);
    for sg in &all {
        let t = sg.dir().unwrap();
        assert!(
            ((sg.a - c1.c).len() - c1.r).abs() < 1e-6 && ((sg.b - c2.c).len() - c2.r).abs() < 1e-6
        );
        assert!((sg.a - c1.c).dot(t).abs() < 1e-6 && (sg.b - c2.c).dot(t).abs() < 1e-6);
    }
    let outer = bitangent(c1, Orient::Ccw, c2, Orient::Ccw).unwrap();
    assert!((outer.a.y + 1_000_000.0).abs() < 1e-6 && (outer.b.y + 1_000_000.0).abs() < 1e-6);
}

#[test]
fn example_point_inside_circle_is_error() {
    assert!(matches!(
        point_circle_tangent(
            Vec2 { x: 0.0, y: 0.0 },
            Circle {
                c: Vec2 { x: 0.0, y: 0.0 },
                r: 1.0
            },
            Orient::Ccw
        ),
        Err(GeomError::InsideObstacle)
    ));
}

#[test]
fn example_sweep_greater_than_pi_is_kept() {
    let c = Vec2 { x: 10.0, y: -5.0 };
    let r = 1000.0;
    let at = |deg: f64| {
        c + Vec2 {
            x: r * deg.to_radians().cos(),
            y: r * deg.to_radians().sin(),
        }
    };
    let arc = Arc::from_tangent_points(c, r, Orient::Ccw, at(0.0), at(-90.0));
    assert!((arc.sweep - 3.0 * std::f64::consts::FRAC_PI_2).abs() < 1e-9);
    assert!(arc.contains_dir(at(135.0)));
    assert!(!arc.contains_dir(at(-45.0)));
}

#[test]
fn example_concentric_and_collinear_degenerate() {
    let c = Circle {
        c: Vec2 { x: 1.0, y: 1.0 },
        r: 5.0,
    };
    assert!(matches!(
        bitangent(c, Orient::Ccw, c, Orient::Cw),
        Err(GeomError::Degenerate)
    ));
    let p = |x: f64| Vec2 { x, y: 0.0 };
    assert!(Arc::from_three_points(p(0.0), p(1.0), p(2.0)).is_none());
}

#[test]
fn example_contained_circles_outer_infeasible() {
    let c1 = Circle {
        c: Vec2 { x: 0.0, y: 0.0 },
        r: 1_000_000.0,
    };
    let c2 = Circle {
        c: Vec2 {
            x: 100_000.0,
            y: 0.0,
        },
        r: 100_000.0,
    };
    assert!(matches!(
        bitangent(c1, Orient::Ccw, c2, Orient::Ccw),
        Err(GeomError::Infeasible)
    ));
    assert!(all_bitangents(c1, c2).is_empty());
}

#[test]
fn example_concentric_all_bitangents_empty() {
    let c = Circle {
        c: Vec2 { x: 3.0, y: 4.0 },
        r: 10.0,
    };
    assert!(all_bitangents(c, Circle { c: c.c, r: 20.0 }).is_empty());
}

#[test]
fn example_contains_dir_degenerate() {
    let c = Vec2 { x: 0.0, y: 0.0 };
    let zero = Arc {
        c,
        r: 10.0,
        sigma: Orient::Ccw,
        a_start: 0.5,
        sweep: 0.0,
    };
    assert!(zero.contains_dir(Vec2 {
        x: 5.0 * 0.5f64.cos(),
        y: 5.0 * 0.5f64.sin()
    }));
    assert!(!zero.contains_dir(Vec2 { x: 0.0, y: 5.0 }));
    assert!(!zero.contains_dir(c));
    let big = Arc {
        c,
        r: 10.0,
        sigma: Orient::Ccw,
        a_start: 0.0,
        sweep: 4.0,
    };
    assert!(!big.contains_dir(c));
}

#[test]
fn example_bad_inputs_rejected() {
    let p = Vec2 { x: 100.0, y: 0.0 };
    let bad = Circle {
        c: Vec2 { x: 0.0, y: 0.0 },
        r: -1.0,
    };
    assert!(matches!(
        point_circle_tangent(p, bad, Orient::Ccw),
        Err(GeomError::Degenerate)
    ));
    let nan = Circle {
        c: Vec2 {
            x: f64::NAN,
            y: 0.0,
        },
        r: 1.0,
    };
    assert!(matches!(
        point_circle_tangent(p, nan, Orient::Ccw),
        Err(GeomError::Degenerate)
    ));
    assert!(matches!(
        bitangent(bad, Orient::Ccw, nan, Orient::Cw),
        Err(GeomError::Degenerate)
    ));
}
