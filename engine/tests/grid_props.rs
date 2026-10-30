use proptest::prelude::*;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::HashSet;
use std::time::Instant;
use topor_engine::geom::{Aabb, Grid, Vec2};

fn aabb() -> impl Strategy<Value = Aabb> {
    (
        -2_000_000.0f64..2_000_000.0,
        -2_000_000.0f64..2_000_000.0,
        0.0f64..800_000.0,
        0.0f64..800_000.0,
    )
        .prop_map(|(x, y, w, h)| Aabb {
            min: Vec2 { x, y },
            max: Vec2 { x: x + w, y: y + h },
        })
}

proptest! {
    #[test]
    fn prop_query_is_superset_of_bruteforce(items in prop::collection::vec(aabb(), 1..300), q in aabb()) {
        let mut g = Grid::new(500_000.0);
        for (i, b) in items.iter().enumerate() { g.insert(*b, i); }
        let brute: HashSet<usize> = items.iter().enumerate()
            .filter(|(_, b)| b.grow(0.0).overlaps(&q)).map(|(i, _)| i).collect();
        let res = g.query(q);
        let got: HashSet<usize> = res.iter().copied().collect();
        prop_assert!(brute.is_subset(&got));
        prop_assert_eq!(res.len(), got.len(), "query must be deduplicated");
    }

    #[test]
    fn prop_remove_then_query_excludes(items in prop::collection::vec(aabb(), 1..100), idx in 0usize..100) {
        let mut g = Grid::new(500_000.0);
        for (i, b) in items.iter().enumerate() { g.insert(*b, i); }
        let k = idx % items.len();
        g.remove(items[k], k);
        prop_assert!(!g.query(items[k]).contains(&k));
        prop_assert_eq!(g.len(), items.len() - 1);
    }
}

/// n unit (10 um) boxes on a jittered 100 um lattice (constant density), 1000 random queries.
fn run(n: usize) -> (f64, std::time::Duration) {
    let mut rng = ChaCha8Rng::seed_from_u64(7);
    let side = (n as f64).sqrt().ceil() as usize;
    let pitch = 100_000.0;
    let mut g = Grid::new(500_000.0);
    for i in 0..n {
        let x = (i % side) as f64 * pitch + rng.gen_range(0.0..50_000.0);
        let y = (i / side) as f64 * pitch + rng.gen_range(0.0..50_000.0);
        g.insert(
            Aabb {
                min: Vec2 { x, y },
                max: Vec2 {
                    x: x + 10_000.0,
                    y: y + 10_000.0,
                },
            },
            i,
        );
    }
    let extent = side as f64 * pitch;
    let qs: Vec<Aabb> = (0..1000)
        .map(|_| {
            let x = rng.gen_range(0.0..extent);
            let y = rng.gen_range(0.0..extent);
            Aabb {
                min: Vec2 { x, y },
                max: Vec2 {
                    x: x + 10_000.0,
                    y: y + 10_000.0,
                },
            }
        })
        .collect();
    let t = Instant::now();
    let mut visited = 0usize;
    for q in &qs {
        visited += g.query_counted(*q).1;
    }
    let dt = t.elapsed();
    (visited as f64 / qs.len() as f64, dt)
}

#[test]
#[cfg_attr(debug_assertions, ignore = "timing assertion needs --release")]
fn scaling_query_cost_is_sublinear() {
    let (c_small, t_small) = run(1_000);
    let _ = run(10_000);
    let (c_big, t_big) = run(100_000);
    assert!(
        c_big <= 4.0 * c_small.max(1.0),
        "candidates {c_small} -> {c_big}"
    );
    let floor = std::time::Duration::from_micros(200);
    assert!(
        t_big <= 20 * t_small.max(floor),
        "wall {t_small:?} -> {t_big:?}"
    );
}

// ---- robustness (fix round 1) ----

fn bx(x0: f64, y0: f64, x1: f64, y1: f64) -> Aabb {
    Aabb {
        min: Vec2 { x: x0, y: y0 },
        max: Vec2 { x: x1, y: y1 },
    }
}

fn fast<R>(f: impl FnOnce() -> R) -> R {
    let t = Instant::now();
    let r = f();
    assert!(t.elapsed().as_millis() < 100, "too slow: {:?}", t.elapsed());
    r
}

#[test]
fn nonfinite_boxes_rejected_and_query_empty() {
    use topor_engine::geom::GridError;
    let bad = [
        bx(f64::NAN, 0.0, 1.0, 1.0),
        bx(0.0, 0.0, f64::INFINITY, 1.0),
        bx(f64::NEG_INFINITY, 0.0, 1.0, 1.0),
    ];
    let mut g = Grid::new(500_000.0);
    g.insert(bx(0.0, 0.0, 10.0, 10.0), 1usize);
    for b in bad {
        assert_eq!(fast(|| g.try_insert(b, 2)), Err(GridError::NonFinite));
        assert!(fast(|| g.query(b)).is_empty());
        assert!(!g.remove(b, 1));
    }
    assert_eq!(g.len(), 1);
}

#[test]
#[should_panic(expected = "non-finite")]
fn insert_nan_panics() {
    let mut g = Grid::new(500_000.0);
    g.insert(bx(f64::NAN, 0.0, 1.0, 1.0), 1usize);
}

#[test]
fn huge_finite_boxes_terminate_fast() {
    for b in [
        bx(-1e300, -1e300, 1e300, 1e300),
        bx(-1e9, -1e9, 1e9, 1e9),
        bx(1e9, 1e9, 1e9, 1e9),
        bx(-1e9, -1e9, -1e9, -1e9),
    ] {
        let mut g = Grid::new(500_000.0);
        g.insert(bx(0.0, 0.0, 10.0, 10.0), 0usize);
        fast(|| g.insert(b, 1));
        let r = fast(|| g.query(b));
        assert!(r.contains(&1));
        let r = fast(|| g.query(bx(0.0, 0.0, 1.0, 1.0)));
        assert!(r.contains(&0));
        assert!(fast(|| g.remove(b, 1)));
        assert_eq!(g.len(), 1);
    }
    // tiny cell, world-size box
    let mut g = Grid::new(1.0);
    fast(|| g.insert(bx(-1e300, 0.0, 1e300, 1.0), 5usize));
    assert!(fast(|| g.query(bx(0.0, 0.0, 1.0, 1.0))).contains(&5));
}

#[test]
fn world_box_query_bounded_by_occupied_cells() {
    let mut g = Grid::new(1_000.0);
    for (i, x) in [0.0, 50_000.0, -90_000.0].into_iter().enumerate() {
        g.insert(bx(x, x, x + 10.0, x + 10.0), i);
    }
    let (mut r, visited) = fast(|| g.query_counted(bx(-1e9, -1e9, 1e9, 1e9)));
    r.sort();
    assert_eq!(r, vec![0, 1, 2]);
    assert!(visited <= 3, "visited {visited}");
}

#[test]
fn remove_with_wrong_bbox_reports_false() {
    let mut g = Grid::new(1_000.0);
    let b = bx(0.0, 0.0, 10.0, 10.0);
    g.insert(b, 7usize);
    // release builds report false; debug builds also debug_assert
    let wrong = bx(50_000.0, 0.0, 50_010.0, 10.0);
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| g.remove(wrong, 7)));
    assert!(matches!(res, Ok(false) | Err(_)));
    assert_eq!(g.len(), 1);
    assert!(g.remove(b, 7));
    assert!(g.is_empty());
}
