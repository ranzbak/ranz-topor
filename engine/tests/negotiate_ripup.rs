//! A16: escalating per-pair rip cost (46 row 17, design note `m4-capacity-steiner-design.md` §6).
//! Two connections that contest one class rip each other; the history each rip leaves on the
//! blocker's path grows `hist_block * growth^(k-1)`, so the contest settles by price before the
//! `ripup_max_per_pair` backstop (a runtime bound only) refuses further rips.
//!
//! The fixture is the sealed cross (A and B contest the only class; no route exists for the
//! loser, so no price can settle it - the stop rule or the backstop must end the exchange).

use topor_engine::model::{BoardModel, Options};
use topor_engine::negotiate::{rip_increment, HIST_BLOCK};
use topor_engine::route::{route, RouteResult};

const MM: f64 = 1_000_000.0;

/// The plus-shaped board of `route_engine::sealed_cross_model` (A along the horizontal bar, B
/// along the vertical one, rect end pads sealing the arms), F.Cu only.
fn cross() -> BoardModel {
    let pts: [(f64, f64); 12] = [
        (20.5, 2.0),
        (23.5, 2.0),
        (23.5, 20.5),
        (42.0, 20.5),
        (42.0, 23.5),
        (23.5, 23.5),
        (23.5, 42.0),
        (20.5, 42.0),
        (20.5, 23.5),
        (2.0, 23.5),
        (2.0, 20.5),
        (20.5, 20.5),
    ];
    let segs: Vec<String> = (0..12)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % 12]);
            format!(
                r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
                (a.0 * MM) as i64,
                (-a.1 * MM) as i64,
                (b.0 * MM) as i64,
                (-b.1 * MM) as i64
            )
        })
        .collect();
    let rect = |r: &str, n: &str, net: i32, x: f64, y: f64, w: f64, h: f64| {
        format!(
            r#"{{ "uuid": "{r}{n}", "ref": "{r}", "number": "{n}", "net": {net},
              "center": [{}, {}], "layers": [0],
              "shapes": {{ "0": {{ "kind": "rect", "w": {}, "h": {} }} }} }}"#,
            (x * MM) as i64,
            (-y * MM) as i64,
            (w * MM) as i64,
            (h * MM) as i64
        )
    };
    let pads = [
        rect("A", "1", 1, 4.0, 22.0, 1.0, 4.0),
        rect("A", "2", 1, 40.0, 22.0, 1.0, 4.0),
        rect("B", "1", 2, 22.0, 4.0, 4.0, 1.0),
        rect("B", "2", 2, 22.0, 40.0, 4.0, 1.0),
    ];
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "N1" }}, {{ "code": 2, "name": "N2" }} ],
          "pads": [ {} ], "outline": [ {} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#,
        pads.join(","),
        segs.join(",")
    );
    let mut m = BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"));
    for l in &mut m.layers {
        l.routable = l.id == 0;
    }
    m
}

fn opts(threads: usize) -> Options {
    Options {
        wall_clock_s: 600.0,
        threads,
        profile: true,
        ..Options::default()
    }
}

fn strip(r: &RouteResult) -> String {
    let mut v = serde_json::to_value(r).unwrap();
    let o = v.as_object_mut().unwrap();
    o.remove("runtime_s");
    o.remove("profile");
    o.remove("threads_used");
    serde_json::to_string(&v).unwrap()
}

fn rips_per_iteration(r: &RouteResult) -> Vec<usize> {
    r.profile.as_ref().unwrap().iter().map(|it| it.ripped).collect()
}

/// With the defaults (growth 1.25, backstop 8, patience 5) the A/B exchange ends by the stop rule
/// after a handful of rips: no pair gets near the backstop, no pair is refused, and the report
/// agrees with the per-iteration profile.
#[test]
fn ping_pong_pair_converges_before_backstop() {
    let r = route(&cross(), &opts(1));
    assert_eq!((r.routed, r.failed), (1, 1));
    let rp = &r.ripup;
    assert!(rp.total > 0, "the sealed cross must rip: {rp:?}");
    assert!(rp.max_pair <= 4, "{rp:?}");
    assert!(rp.max_pair < Options::default().ripup_max_per_pair, "{rp:?}");
    assert!(rp.pairs_at_backstop.is_empty(), "{rp:?}");
    assert_eq!(rp.total as usize, rips_per_iteration(&r).iter().sum::<usize>());
    assert_eq!(rp.rips_per_connection.len(), 2);
    assert_eq!(rp.rips_per_connection.iter().sum::<u32>(), rp.total);
    assert!(r.warnings.iter().any(|w| w.starts_with("no gain:")), "{:?}", r.warnings);
}

/// `max_per_pair = 3` (the M3 cap) with a patience that outlasts it: exactly 3 rips per pair
/// (6 in total), then the backstop refuses, the pair is reported and no later iteration rips.
#[test]
fn backstop_stops_rips() {
    let o = Options { ripup_max_per_pair: 3, patience: 8, ..opts(1) };
    let r = route(&cross(), &o);
    let rp = &r.ripup;
    assert_eq!((rp.total, rp.max_pair), (6, 3), "{rp:?}");
    assert_eq!(rp.rips_per_connection, vec![3, 3]);
    assert!(!rp.pairs_at_backstop.is_empty(), "{rp:?}");
    assert!(rp.pairs_at_backstop.iter().all(|p| p[0] != p[1] && p[0] < 2 && p[1] < 2));
    let rips = rips_per_iteration(&r);
    assert!(rips[..6].iter().all(|&k| k > 0) && rips[6..].iter().all(|&k| k == 0), "{rips:?}");
    // a backstop of 1 refuses from the second exchange on
    let o1 = Options { ripup_max_per_pair: 1, patience: 8, ..opts(1) };
    let r1 = route(&cross(), &o1);
    assert!(r1.ripup.max_pair <= 1 && !r1.ripup.pairs_at_backstop.is_empty(), "{:?}", r1.ripup);
}

/// `growth = 1` is the constant M3 cost; `growth = 2` doubles per rip (0.5, 1, 2, 4 ...); the
/// factor saturates for absurd growth / counts instead of overflowing.
#[test]
fn growth_one_is_constant_cost() {
    for k in 1..=8 {
        assert_eq!(rip_increment(HIST_BLOCK, 1.0, k), HIST_BLOCK);
    }
    let esc: Vec<f64> = (1..=5).map(|k| rip_increment(HIST_BLOCK, 2.0, k)).collect();
    assert_eq!(esc, vec![0.5, 1.0, 2.0, 4.0, 8.0]);
    let cum: f64 = (1..=3).map(|k| rip_increment(HIST_BLOCK, 2.0, k)).sum();
    assert_eq!(cum, 3.5);
    let big = rip_increment(HIST_BLOCK, 10.0, 1000);
    assert!(big.is_finite() && big > 0.0);
    // the route with growth 1 and the same backstop rips exactly like growth 2 on the sealed
    // cross (nothing to price), and reports the same totals
    let a = route(&cross(), &Options { ripup_growth: 1.0, ripup_max_per_pair: 3, patience: 8, ..opts(1) });
    let b = route(&cross(), &Options { ripup_growth: 2.0, ripup_max_per_pair: 3, patience: 8, ..opts(1) });
    assert_eq!(a.ripup, b.ripup);
}

/// Threads 1 / 2 / 4 and repeats give byte-identical results, including the rip report.
#[test]
fn ripup_threads_byte_identical() {
    let m = cross();
    let base = route(&m, &opts(1));
    assert!(base.ripup.total > 0);
    let want = strip(&base);
    for t in [1usize, 2, 4] {
        for _ in 0..2 {
            assert_eq!(strip(&route(&m, &opts(t))), want, "threads {t}");
        }
    }
}
