use proptest::prelude::*;
use std::f64::consts::{FRAC_PI_2, TAU};
use topor_engine::geom::{
    arc_arc, copper_gap, copper_gap_skeleton, dist_prim, dist_prim_skeleton, point_arc,
    point_circle, point_seg, seg_arc, seg_seg, Arc, Circle, Orient, Prim, Seg, Skeleton, Track,
    Vec2,
};

const N: usize = 2000;

fn v(x: f64, y: f64) -> Vec2 {
    Vec2 { x, y }
}
fn orient() -> impl Strategy<Value = Orient> {
    prop_oneof![Just(Orient::Ccw), Just(Orient::Cw)]
}
fn vec2() -> impl Strategy<Value = Vec2> {
    (-2_000_000.0f64..2_000_000.0, -2_000_000.0f64..2_000_000.0).prop_map(|(x, y)| v(x, y))
}
/// Centres from a coarse grid so that concentric / touching cases occur often.
fn grid_vec2() -> impl Strategy<Value = Vec2> {
    (-2i32..=2, -2i32..=2).prop_map(|(x, y)| v(x as f64 * 400_000.0, y as f64 * 400_000.0))
}
fn seg() -> impl Strategy<Value = Seg> {
    (vec2(), vec2()).prop_map(|(a, b)| Seg { a, b })
}
fn arc_with(c: impl Strategy<Value = Vec2>) -> impl Strategy<Value = Arc> {
    (
        c,
        1_000.0f64..2_000_000.0,
        orient(),
        0.0f64..TAU,
        0.01f64..(TAU - 0.01),
    )
        .prop_map(|(c, r, sigma, a_start, sweep)| Arc {
            c,
            r,
            sigma,
            a_start,
            sweep,
        })
}
fn arc() -> impl Strategy<Value = Arc> {
    prop_oneof![arc_with(vec2()), arc_with(grid_vec2())]
}
fn prim() -> impl Strategy<Value = Prim> {
    prop_oneof![seg().prop_map(Prim::Seg), arc().prop_map(Prim::Arc)]
}

fn samples(p: &Prim) -> Vec<Vec2> {
    (0..N)
        .map(|i| {
            let f = i as f64 / (N - 1) as f64;
            match p {
                Prim::Seg(s) => s.a + (s.b - s.a) * f,
                Prim::Arc(a) => a.point_at(f),
            }
        })
        .collect()
}
/// Dense-sampling oracle (upper bound on the true distance, approximate from below).
fn sampled(a: &Prim, b: &Prim) -> f64 {
    let sb = samples(b);
    let mut best = f64::INFINITY;
    for p in samples(a) {
        for q in &sb {
            best = best.min((p - *q).len2());
        }
    }
    best.sqrt()
}
fn max_len(a: &Prim, b: &Prim) -> f64 {
    a.length().max(b.length())
}
fn check(a: Prim, b: Prim, d: f64) -> Result<(), TestCaseError> {
    let sd = sampled(&a, &b);
    let tol = 2.0 * max_len(&a, &b) / N as f64;
    prop_assert!(d <= sd + 1e-6, "d={d} sampled={sd}");
    prop_assert!(d >= sd - tol, "d={d} sampled={sd} tol={tol}");
    Ok(())
}

proptest! {
  #![proptest_config(ProptestConfig::with_cases(512))]
  #[test] fn prop_seg_arc_matches_sampling(s in seg(), a in arc()) {
    check(Prim::Seg(s), Prim::Arc(a), seg_arc(&s, &a))?;
  }
  #[test] fn prop_arc_arc_matches_sampling(a in arc(), b in arc()) {
    check(Prim::Arc(a), Prim::Arc(b), arc_arc(&a, &b))?;
  }
  #[test] fn prop_seg_seg_matches_sampling(a in seg(), b in seg()) {
    check(Prim::Seg(a), Prim::Seg(b), seg_seg(&a, &b))?;
  }
  #[test] fn prop_point_arc_matches_sampling(p in vec2(), a in arc()) {
    let pt = Prim::Seg(Seg { a: p, b: p });
    check(pt, Prim::Arc(a), point_arc(p, &a))?;
  }
  #[test] fn prop_dist_symmetric(a in prim(), b in prim()) {
    prop_assert!((dist_prim(&a, &b) - dist_prim(&b, &a)).abs() < 1e-6);
  }
  #[test] fn prop_dist_nonnegative_and_self_zero(a in prim()) {
    prop_assert!(dist_prim(&a, &a) < 1e-6);
  }
}

fn arc_deg(c: Vec2, r: f64, a0: f64, a1: f64) -> Arc {
    Arc {
        c,
        r,
        sigma: Orient::Ccw,
        a_start: a0.to_radians(),
        sweep: (a1 - a0).to_radians(),
    }
}

#[test]
fn example_concentric_arcs_overlapping_spans() {
    let c = v(1.0e6, -2.0e6);
    let a = arc_deg(c, 1.0e6, 0.0, 120.0);
    let b = arc_deg(c, 1.5e6, 90.0, 300.0);
    assert!((arc_arc(&a, &b) - 500_000.0).abs() < 1e-6);
    // span > pi containing the other entirely
    let big = arc_deg(c, 1.5e6, 10.0, 350.0);
    let small = arc_deg(c, 1.0e6, 100.0, 110.0);
    assert!((arc_arc(&small, &big) - 500_000.0).abs() < 1e-6);
    assert!((arc_arc(&big, &small) - 500_000.0).abs() < 1e-6);
    // CW arc against CCW arc, same centre
    let cw = Arc {
        sigma: Orient::Cw,
        ..arc_deg(c, 1.5e6, 0.0, 0.0)
    };
    let cw = Arc {
        sweep: 200f64.to_radians(),
        a_start: 100f64.to_radians(),
        ..cw
    };
    assert!((arc_arc(&a, &cw) - 500_000.0).abs() < 1e-6);
}

#[test]
fn example_concentric_arcs_disjoint_spans_use_endpoints() {
    let c = v(0.0, 0.0);
    let a = arc_deg(c, 1.0e6, 0.0, 90.0);
    let b = arc_deg(c, 1.5e6, 180.0, 270.0);
    let want = [a.start(), a.end()]
        .iter()
        .flat_map(|p| [b.start(), b.end()].map(|q| (*p - q).len()))
        .fold(f64::INFINITY, f64::min);
    assert!((arc_arc(&a, &b) - want).abs() < 1e-6);
    assert!(want > 500_000.0);
    // equal radii, adjacent spans touch
    let t1 = arc_deg(c, 1.0e6, 0.0, 90.0);
    let t2 = arc_deg(c, 1.0e6, 90.0, 180.0);
    assert!(arc_arc(&t1, &t2) < 1e-6);
}

#[test]
fn example_intersecting_seg_arc_is_zero() {
    let a = arc_deg(v(0.0, 0.0), 1.0e6, 0.0, 180.0);
    // chord through the arc interior (crosses at (0, 1e6))
    let s = Seg {
        a: v(0.0, 0.5e6),
        b: v(0.0, 1.5e6),
    };
    assert_eq!(seg_arc(&s, &a), 0.0);
    // same line but the crossing is in the missing half
    let s2 = Seg {
        a: v(0.0, -0.5e6),
        b: v(0.0, -1.5e6),
    };
    // nearest arc points are the endpoints (+-1e6, 0)
    let want = (v(1e6, 0.0) - v(0.0, -0.5e6)).len();
    assert!((seg_arc(&s2, &a) - want).abs() < 1e-6);
    // a segment lying inside the circle of a > pi arc that misses it
    let big = arc_deg(v(0.0, 0.0), 1.0e6, 0.0, 300.0);
    let inner = Seg {
        a: v(-1e5, -1e5),
        b: v(1e5, 1e5),
    };
    assert!((seg_arc(&inner, &big) - (1.0e6 - 2f64.sqrt() * 1e5)).abs() < 1e-3);
}

#[test]
fn example_seg_arc_missing_half_uses_endpoint() {
    // upper semicircle, segment below the centre: nearest arc points are the endpoints
    let a = arc_deg(v(0.0, 0.0), 1.0e6, 0.0, 180.0);
    let s = Seg {
        a: v(-3e5, -2e5),
        b: v(3e5, -2e5),
    };
    let want = (v(1e6, 0.0) - v(3e5, -2e5)).len();
    assert!((seg_arc(&s, &a) - want).abs() < 1e-6);
}

#[test]
fn example_point_at_centre_is_radius() {
    let c = v(123_456.0, -789_000.0);
    for (a0, a1) in [(0.0, 90.0), (10.0, 350.0), (0.0, 0.0)] {
        let a = arc_deg(c, 750_000.0, a0, a1);
        assert_eq!(point_arc(c, &a), a.r);
    }
    // segment through the centre of a full-ish arc: crosses it
    let a = arc_deg(c, 750_000.0, 5.0, 355.0);
    let s = Seg {
        a: c + v(-1e6, 0.0),
        b: c + v(1e6, 0.0),
    };
    assert_eq!(seg_arc(&s, &a), 0.0);
    // zero-length segment at the centre
    let p = Seg { a: c, b: c };
    assert!((seg_arc(&p, &a) - a.r).abs() < 1e-9);
}

#[test]
fn example_arc_arc_intersecting_and_nested() {
    let a = arc_deg(v(0.0, 0.0), 1.0e6, 0.0, 180.0);
    let b = arc_deg(v(1.0e6, 0.0), 1.0e6, 0.0, 180.0);
    assert_eq!(arc_arc(&a, &b), 0.0); // circles meet at (5e5, ±866025), upper one in both spans
    let lower = Arc {
        sigma: Orient::Cw,
        ..b
    };
    // lower half of b vs upper half of a: only the shared point-free side remains
    let d = arc_arc(&a, &lower);
    assert!(d >= 0.0 && d <= sampled(&Prim::Arc(a), &Prim::Arc(lower)) + 1e-6);
    // nested, far from touching: |R1 - R2| - d
    let inner = arc_deg(v(1e5, 0.0), 2e5, 0.0, 359.0);
    let outer = arc_deg(v(0.0, 0.0), 1e6, 0.0, 359.0);
    assert!((arc_arc(&inner, &outer) - (1e6 - 1e5 - 2e5)).abs() < 1e-3);
    // identical arcs
    assert!(arc_arc(&a, &a) < 1e-9);
    // tangent externally
    let t1 = arc_deg(v(0.0, 0.0), 1e6, 0.0, 359.0);
    let t2 = arc_deg(v(3e6, 0.0), 2e6, 0.0, 359.0);
    assert!(arc_arc(&t1, &t2) < 1e-6);
}

#[test]
fn example_point_helpers() {
    let s = Seg {
        a: v(0.0, 0.0),
        b: v(10.0, 0.0),
    };
    assert_eq!(point_seg(v(5.0, 3.0), &s), 3.0);
    assert_eq!(point_seg(v(-3.0, 4.0), &s), 5.0);
    assert_eq!(
        point_seg(
            v(1.0, 1.0),
            &Seg {
                a: v(0.0, 0.0),
                b: v(0.0, 0.0)
            }
        ),
        2f64.sqrt()
    );
    let c = Circle {
        c: v(0.0, 0.0),
        r: 4.0,
    };
    assert_eq!(point_circle(v(5.0, 0.0), &c), 1.0);
    assert_eq!(point_circle(v(1.0, 0.0), &c), -3.0);
}

fn track(a: Vec2, b: Vec2, w: f64) -> Track {
    Track {
        prim: Prim::Seg(Seg { a, b }),
        w,
    }
}

#[test]
fn example_skeleton_rect_pad() {
    let sk = Skeleton::rect(v(0.0, 0.0), 1_000_000.0, 2_000_000.0, 0.0);
    // long side at x = 500_000; track at x = 900_000
    let t = track(v(900_000.0, -3e6), v(900_000.0, 3e6), 250_000.0);
    assert!((copper_gap_skeleton(&t, &sk) - (400_000.0 - 125_000.0)).abs() < 1e-6);
    // diagonal approach to a corner
    let t2 = track(v(1.5e6, 2.0e6), v(3.0e6, 2.0e6), 0.0);
    assert!((copper_gap_skeleton(&t2, &sk) - (1.0e6f64.hypot(1.0e6 - 0.0))).abs() < 1e-6);
}

#[test]
fn example_skeleton_oval_pad() {
    match Skeleton::oval(v(0.0, 0.0), 1_000_000.0, 2_000_000.0, 0.0) {
        Skeleton::Seg { s, r } => {
            assert!((s.len() - 1_000_000.0).abs() < 1e-6);
            assert_eq!(r, 500_000.0);
            assert_eq!(s.a.x, 0.0);
        }
        o => panic!("{o:?}"),
    }
    match Skeleton::oval(v(0.0, 0.0), 1_000_000.0, 1_000_000.0, 0.3) {
        Skeleton::Point { r, .. } => assert_eq!(r, 500_000.0),
        o => panic!("{o:?}"),
    }
    let sk = Skeleton::oval(v(0.0, 0.0), 1_000_000.0, 2_000_000.0, FRAC_PI_2);
    let t = track(v(2e6, -3e6), v(2e6, 3e6), 0.0);
    // rotated 90 deg: axis along x, half-length 500k, r 500k -> extent x = 1e6
    assert!((copper_gap_skeleton(&t, &sk) - 1.0e6).abs() < 1e-6);
}

#[test]
fn example_skeleton_roundrect_pad() {
    match Skeleton::round_rect(v(0.0, 0.0), 1_000_000.0, 2_000_000.0, 250_000.0, 0.0) {
        Skeleton::Poly { pts, r } => {
            assert_eq!(r, 250_000.0);
            let xs: Vec<f64> = pts.iter().map(|p| p.x).collect();
            let ys: Vec<f64> = pts.iter().map(|p| p.y).collect();
            let w = xs.iter().cloned().fold(f64::MIN, f64::max)
                - xs.iter().cloned().fold(f64::MAX, f64::min);
            let h = ys.iter().cloned().fold(f64::MIN, f64::max)
                - ys.iter().cloned().fold(f64::MAX, f64::min);
            assert_eq!((w, h), (500_000.0, 1_500_000.0));
            // CCW
            let area: f64 = (0..4).map(|i| pts[i].cross(pts[(i + 1) % 4])).sum();
            assert!(area > 0.0);
        }
        o => panic!("{o:?}"),
    }
}

#[test]
fn example_centreline_inside_polygon_is_nonpositive() {
    let sk = Skeleton::rect(v(0.0, 0.0), 1_000_000.0, 2_000_000.0, 0.0);
    // crossing the boundary
    let t = track(v(0.0, 0.0), v(2e6, 0.0), 0.0);
    assert!(dist_prim_skeleton(&t.prim, &sk) <= 0.0);
    // fully inside, 200k from the nearest edge
    let inside = Prim::Seg(Seg {
        a: v(-300_000.0, -500_000.0),
        b: v(300_000.0, 500_000.0),
    });
    assert!((dist_prim_skeleton(&inside, &sk) + 200_000.0).abs() < 1e-6);
    // arc inside
    let a = Prim::Arc(arc_deg(v(0.0, 0.0), 300_000.0, 0.0, 200.0));
    assert!(dist_prim_skeleton(&a, &sk) < 0.0);
    // roundrect: r is subtracted as well
    let rr = Skeleton::round_rect(v(0.0, 0.0), 1_000_000.0, 2_000_000.0, 250_000.0, 0.0);
    let tiny = Prim::Seg(Seg {
        a: v(-100_000.0, -500_000.0),
        b: v(100_000.0, 500_000.0),
    });
    // 150k from the inset polygon's long edge, minus r
    assert!((dist_prim_skeleton(&tiny, &rr) + 150_000.0 + 250_000.0).abs() < 1e-6);
}

#[test]
fn example_copper_gap_tracks() {
    let a = track(v(0.0, 0.0), v(1e6, 0.0), 200_000.0);
    let b = track(v(0.0, 1e6), v(1e6, 1e6), 300_000.0);
    assert!((copper_gap(&a, &b) - (1e6 - 100_000.0 - 150_000.0)).abs() < 1e-6);
}
