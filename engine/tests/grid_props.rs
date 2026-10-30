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
