use proptest::prelude::*;
use std::f64::consts::{PI, TAU};
use topor_engine::geom::{
    calc_arc_center, dequantise, is_degenerate_arc, quantise, quantise_chain, round_nm, Arc, IVec2,
    Orient, Prim, Seg, TrackPrim, Vec2,
};

fn orient() -> impl Strategy<Value = Orient> {
    prop_oneof![Just(Orient::Ccw), Just(Orient::Cw)]
}
fn vec2() -> impl Strategy<Value = Vec2> {
    (
        -50_000_000.0f64..50_000_000.0,
        -50_000_000.0f64..50_000_000.0,
    )
        .prop_map(|(x, y)| Vec2 { x, y })
}
/// Arcs with sagitta >= `min_s` nm, sweep <= pi.
fn arc_min_sagitta(min_s: f64) -> impl Strategy<Value = Arc> {
    (
        vec2(),
        50_000.0f64..5_000_000.0,
        orient(),
        0.0f64..TAU,
        0.05f64..PI,
    )
        .prop_map(|(c, r, sigma, a_start, sweep)| Arc {
            c,
            r,
            sigma,
            a_start,
            sweep,
        })
        .prop_filter("sagitta", move |a| a.sagitta() >= min_s)
}
fn arc_with_chord_and_sagitta(chord: f64, s: f64) -> Arc {
    let r = (chord * chord / 4.0 + s * s) / (2.0 * s);
    Arc {
        c: Vec2 {
            x: 1234.5,
            y: -987.25,
        },
        r,
        sigma: Orient::Ccw,
        a_start: 0.7,
        sweep: 2.0 * (chord / (2.0 * r)).asin(),
    }
}
fn chain(n: std::ops::Range<usize>) -> impl Strategy<Value = Vec<Prim>> {
    let item = (
        any::<bool>(),
        vec2(),
        50_000.0f64..2_000_000.0,
        orient(),
        0.0f64..TAU,
        0.2f64..PI,
    );
    (vec2(), prop::collection::vec(item, n)).prop_map(|(mut p, items)| {
        let mut out = vec![];
        for (is_arc, d, r, sigma, a0, sweep) in items {
            if is_arc {
                let c = p - Vec2 {
                    x: r * a0.cos(),
                    y: r * a0.sin(),
                };
                let a = Arc {
                    c,
                    r,
                    sigma,
                    a_start: a0,
                    sweep,
                };
                p = a.end();
                out.push(Prim::Arc(a));
            } else {
                let q = p + Vec2 {
                    x: d.x * 1e-2,
                    y: d.y * 1e-2,
                };
                out.push(Prim::Seg(Seg { a: p, b: q }));
                p = q;
            }
        }
        out
    })
}

proptest! {
  #[test] fn prop_arc_three_point_roundtrip_center_within_100nm(a in arc_min_sagitta(1000.0)) {
    let q = quantise(&Prim::Arc(a));
    let TrackPrim::Arc{start, mid, end} = q else { prop_assert!(false); return Ok(()); };
    let c = calc_arc_center(start, mid, end).unwrap();
    let back = dequantise(&q);
    let Prim::Arc(b) = back else { unreachable!() };
    prop_assert!((b.c - round_nm(a.c).to_f64()).len() <= (a.r / a.sagitta() + 1.0) * 1.5 + 1.0);
    prop_assert_eq!(c, round_nm(b.c));
    prop_assert_eq!(b.sigma, a.sigma);
  }
  #[test] fn prop_flat_arc_becomes_segment(chord in 1_000_000.0..50_000_000.0f64) {
    let a = arc_with_chord_and_sagitta(chord, 3.0);
    let is_seg = matches!(quantise(&Prim::Arc(a)), TrackPrim::Seg{..});
    prop_assert!(is_seg);
  }
  #[test] fn prop_big_sweep_arc_is_never_degenerate(
      c in vec2(), r in 100_000.0f64..5_000_000.0, sigma in orient(), a0 in 0.0f64..TAU,
      sweep in (PI + 1e-6)..(TAU - 1e-12)) {
    let a = Arc { c, r, sigma, a_start: a0, sweep };
    prop_assert!(!is_degenerate_arc(&a));
    if a.chord() > 1000.0 {
      let is_arc = matches!(quantise(&Prim::Arc(a)), TrackPrim::Arc{..});
      prop_assert!(is_arc);
    }
  }
  #[test] fn prop_chain_shares_endpoints(chain in chain(2..8)) {
    let q = quantise_chain(&chain);
    prop_assert_eq!(q.len(), chain.len());
    for w in q.windows(2) { prop_assert_eq!(w[0].end(), w[1].start()); }
  }
  #[test] fn prop_round_half_away_from_zero(x in -10.0..10.0f64) {
    prop_assert_eq!(round_nm(Vec2{x, y: 0.0}).x, if x >= 0.0 {(x + 0.5).floor() as i32} else {(x - 0.5).ceil() as i32});
  }
}

#[test]
fn example_kicad_center_known_case() {
    assert_eq!(
        calc_arc_center(
            IVec2 {
                x: 10_000_000,
                y: 10_000_000
            },
            IVec2 {
                x: 12_000_000,
                y: 11_000_000
            },
            IVec2 {
                x: 14_000_000,
                y: 10_000_000
            }
        ),
        Some(IVec2 {
            x: 12_000_000,
            y: 8_500_000
        })
    );
}

#[test]
fn example_collinear_has_no_center() {
    let p = |x| IVec2 { x, y: 5 };
    assert_eq!(calc_arc_center(p(0), p(10), p(20)), None);
}

#[test]
fn example_mid_is_angular_midpoint_not_chord_midpoint() {
    let a = Arc {
        c: Vec2::default(),
        r: 1e6,
        sigma: Orient::Ccw,
        a_start: 0.0,
        sweep: PI / 2.0,
    };
    let TrackPrim::Arc { mid, start, end } = quantise(&Prim::Arc(a)) else {
        panic!()
    };
    assert_eq!(
        mid,
        IVec2 {
            x: 707_107,
            y: 707_107
        }
    );
    assert_eq!(
        (start, end),
        (IVec2 { x: 1_000_000, y: 0 }, IVec2 { x: 0, y: 1_000_000 })
    );
}

#[test]
fn example_near_full_circle_stays_arc() {
    let a = Arc {
        c: Vec2::default(),
        r: 1e6,
        sigma: Orient::Ccw,
        a_start: 0.0,
        sweep: TAU - 1e-9,
    };
    assert!(!is_degenerate_arc(&a));
    let a = Arc {
        sweep: TAU - 1e-3,
        ..a
    };
    assert!(matches!(quantise(&Prim::Arc(a)), TrackPrim::Arc { .. }));
}

#[test]
fn example_tiny_chord_becomes_segment() {
    let a = Arc {
        c: Vec2::default(),
        r: 1e6,
        sigma: Orient::Ccw,
        a_start: 0.0,
        sweep: 5e-6,
    };
    assert!(matches!(quantise(&Prim::Arc(a)), TrackPrim::Seg { .. }));
}

#[test]
fn example_serde_tag() {
    let t = TrackPrim::Seg {
        a: IVec2 { x: 1, y: 2 },
        b: IVec2 { x: 3, y: 4 },
    };
    assert_eq!(
        serde_json::to_string(&t).unwrap(),
        r#"{"kind":"seg","a":{"x":1,"y":2},"b":{"x":3,"y":4}}"#
    );
}
