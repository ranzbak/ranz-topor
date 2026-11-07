//! The noded path of the triangulation (`cdt::Cdt::build_reported`, `noding`): constraints noded by
//! snap rounding before they reach spade, so near-degenerate crossings cannot panic it.

use proptest::prelude::*;
use topor_engine::cdt::{Cdt, CdtPath};
use topor_engine::geom::Vec2;

const MM: f64 = 1_000_000.0;

fn pts(p: &[(f64, f64)]) -> Vec<Vec2> {
    p.iter().map(|&(x, y)| Vec2 { x: x * MM, y: y * MM }).collect()
}

fn noded(p: &[Vec2], c: &[(usize, usize)]) -> Cdt {
    let (cdt, rep) = Cdt::build_forced(p, c, CdtPath::Noded).expect("noded build");
    assert!(rep.dropped.is_empty(), "dropped {:?}", rep.dropped);
    cdt
}

/// Total length of the constraint edges.
fn constraint_len(c: &Cdt) -> f64 {
    c.constraint_edges().iter().map(|&(a, b)| (c.vertex_pos(b) - c.vertex_pos(a)).len()).sum()
}

#[test]
fn noded_crossing_constraints_are_split() {
    let c = noded(&pts(&[(0., 0.), (10., 0.), (5., -5.), (5., 5.)]), &[(0, 1), (2, 3)]);
    assert_eq!(c.num_vertices(), 5);
    assert_eq!(c.num_inputs(), 4);
    let p = c.vertex_pos(4);
    assert!((p.x - 5.0 * MM).abs() < 1.0 && p.y.abs() < 1.0);
    assert_eq!(c.constraint_edges(), vec![(0, 4), (1, 4), (2, 4), (3, 4)]);
    assert!(c.is_split_vertex(4));
}

#[test]
fn noded_overlap_duplicate_t_junction_and_zero_length() {
    let c = noded(&pts(&[(0., 0.), (10., 0.), (4., 0.), (6., 0.)]), &[(0, 1), (2, 3)]);
    assert_eq!(c.constraint_edges(), vec![(0, 2), (1, 3), (2, 3)]);
    let c = noded(&pts(&[(0., 0.), (10., 0.), (5., 5.)]), &[(0, 1), (0, 1), (1, 0)]);
    assert_eq!(c.constraint_edges(), vec![(0, 1)]);
    let c = noded(&pts(&[(0., 0.), (10., 0.), (5., 0.), (5., 5.)]), &[(0, 1), (2, 3)]);
    assert_eq!(c.constraint_edges(), vec![(0, 2), (1, 2), (2, 3)]);
    let c = noded(&pts(&[(0., 0.), (10., 0.), (0., 0.)]), &[(0, 2)]);
    assert!(c.constraint_edges().is_empty());
}

/// Many segments through nearly the same point at shallow angles (a fan of near-parallel
/// constraints, the shape of arcs flattened outward that overlap): every one is kept, split at
/// the shared hot pixels, and the triangulation is valid.
#[test]
fn near_degenerate_fan_is_noded_without_drops() {
    let mut p = Vec::new();
    let mut c = Vec::new();
    for i in 0..40 {
        let a = 1e-7 * i as f64; // radians: the segments differ by nm over mm
        let (s, co) = a.sin_cos();
        let k = p.len();
        p.push(Vec2 { x: -5.0 * MM * co + 0.3 * i as f64, y: -5.0 * MM * s });
        p.push(Vec2 { x: 5.0 * MM * co, y: 5.0 * MM * s + 0.7 * i as f64 });
        c.push((k, k + 1));
    }
    let (cdt, rep) = Cdt::build_forced(&p, &c, CdtPath::Noded).expect("noded");
    assert!(rep.dropped.is_empty());
    assert!(cdt.num_vertices() >= 80);
    // The union of 40 overlapping 10 mm segments is about 10 mm of constraint edges (more where
    // the near-parallel ones part).
    assert!(constraint_len(&cdt) >= 10.0 * MM - 10.0);
}

/// `build` (all paths in order) on crossing input: the direct path when spade copes.
#[test]
fn build_reports_its_path() {
    let (_, rep) = Cdt::build_reported(&pts(&[(0., 0.), (10., 0.), (5., -5.), (5., 5.)]), &[(0, 1), (2, 3)]).expect("build");
    assert_eq!(rep.path, CdtPath::Direct);
    assert!(rep.panic.is_none());
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, .. ProptestConfig::default() })]

    /// Random segments on a fine grid around a small area (many crossings, shared and near
    /// points): the noded path never fails or drops, and keeps every segment's length within the
    /// rounding (each piece end moves ≤ 0.71 nm).
    #[test]
    fn random_segments_are_noded(seed in proptest::collection::vec((0i64..4000, 0i64..4000, 0i64..4000, 0i64..4000), 2..60)) {
        let mut p = Vec::new();
        let mut c = Vec::new();
        for &(x0, y0, x1, y1) in &seed {
            let k = p.len();
            // nm coordinates with fractions: not on the grid
            p.push(Vec2 { x: x0 as f64 * 1.37, y: y0 as f64 * 0.91 });
            p.push(Vec2 { x: x1 as f64 * 1.37, y: y1 as f64 * 0.91 });
            c.push((k, k + 1));
        }
        let (cdt, rep) = Cdt::build_forced(&p, &c, CdtPath::Noded).expect("noded");
        prop_assert!(rep.dropped.is_empty(), "dropped {:?}", rep.dropped);
        // Every input segment is covered by constraint edges: its midpoint lies within 2 nm of one.
        let edges = cdt.constraint_edges();
        for &(a, b) in &c {
            let (pa, pb) = (p[a], p[b]);
            if (pb - pa).len() < 3.0 {
                continue;
            }
            let m = (pa + pb) * 0.5;
            let near = edges.iter().any(|&(u, v)| {
                let (q0, q1) = (cdt.vertex_pos(u), cdt.vertex_pos(v));
                let d = q1 - q0;
                let t = if d.len2() > 0.0 { ((m - q0).dot(d) / d.len2()).clamp(0.0, 1.0) } else { 0.0 };
                (m - (q0 + d * t)).len() <= 2.0
            });
            prop_assert!(near, "segment {:?}-{:?} not covered", pa, pb);
        }
    }
}
