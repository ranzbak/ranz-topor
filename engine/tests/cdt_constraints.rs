use topor_engine::cdt::Cdt;
use topor_engine::geom::Vec2;

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

