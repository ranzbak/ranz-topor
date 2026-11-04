use topor_engine::cdt::{Cdt, Crossing, MAX_SLEEVE};
use topor_engine::geom::Vec2;

fn mm(x: f64, y: f64) -> Vec2 {
    Vec2 {
        x: x * 1e6,
        y: y * 1e6,
    }
}

/// A=0 B=1 C=2 D=3 E=4; triangles ABD, BDE, BCE.
fn base() -> (Cdt, [usize; 5]) {
    let pts = [
        mm(0., 0.),
        mm(10., 0.),
        mm(20., 0.),
        mm(5., 10.),
        mm(15., 10.),
    ];
    let c = Cdt::build(&pts, &[]).unwrap();
    let v = [0, 1, 2, 3, 4].map(|i| c.vertex_index(i));
    (c, v)
}

#[test]
fn build_dedups_coincident_points() {
    let pts = [mm(0., 0.), mm(10., 0.), mm(0., 0.)];
    let c = Cdt::build(&pts, &[]).unwrap();
    assert_eq!(c.num_vertices(), 2);
    assert_eq!(c.vertex_index(2), c.vertex_index(0));
}

#[test]
fn polyline_crossings_in_order_with_sides() {
    let (c, v) = base();
    let x = c.crossings(&[mm(3., 4.), mm(17., 4.)]).unwrap();
    assert_eq!(x.len(), 2);
    assert_eq!((x[0].left, x[0].right), (v[3], v[1]));
    assert_eq!((x[1].left, x[1].right), (v[4], v[1]));
    assert!((x[0].t - 0.6).abs() < 1e-6 && (x[1].t - 0.6).abs() < 1e-6);
}

#[test]
fn reverse_direction_swaps_sides() {
    let (c, v) = base();
    let x = c.crossings(&[mm(17., 4.), mm(3., 4.)]).unwrap();
    assert_eq!(x.len(), 2);
    assert_eq!((x[0].left, x[0].right), (v[1], v[4]));
    assert_eq!((x[1].left, x[1].right), (v[1], v[3]));
}

#[test]
fn back_and_forth_reduces() {
    let (c, _) = base();
    let straight = c.crossings(&[mm(3., 4.), mm(17., 4.)]).unwrap();
    let raw = c
        .crossings(&[mm(3., 4.), mm(9., 4.), mm(3., 4.), mm(17., 4.)])
        .unwrap();
    assert_eq!(raw.len(), 4);
    let r = Cdt::reduce(&raw);
    assert_eq!(r.len(), 2);
    for (a, b) in r.iter().zip(&straight) {
        assert_eq!((a.left, a.right), (b.left, b.right));
        assert!((a.t - b.t).abs() < 1e-9);
    }
}

#[test]
fn polyline_starting_on_vertex_is_ok_but_interior_vertex_is_err() {
    let (c, _) = base();
    assert!(c.crossings(&[mm(0., 0.), mm(17., 4.)]).is_ok());
    assert!(c
        .crossings(&[mm(3., 4.), mm(10., 0.), mm(17., 4.)])
        .is_err());
}

#[test]
fn constraint_edge_is_kept() {
    let pts = [
        mm(0., 0.),
        mm(10., 0.),
        mm(10., 10.),
        mm(0., 10.),
        mm(5., 5.),
    ];
    let cons = [(0, 1), (1, 2), (2, 3), (3, 0)];
    let c = Cdt::build(&pts, &cons).unwrap();
    for &(a, b) in &cons {
        assert!(c.is_edge(a, b) && c.is_constraint(a, b));
    }
}

#[test]
fn sleeve_cap() {
    let (c, _) = base();
    // zig-zag across edge BD (y=4 -> x=8), 2001 crossings
    let mut pl = Vec::new();
    for i in 0..=MAX_SLEEVE + 1 {
        pl.push(if i % 2 == 0 { mm(7., 4.) } else { mm(9., 4.) });
    }
    assert_eq!(pl.len(), MAX_SLEEVE + 2);
    let e = c.crossings(&pl).unwrap_err();
    assert!(e.contains("MAX_SLEEVE"), "{e}");
    // exactly MAX_SLEEVE crossings is fine
    assert_eq!(c.crossings(&pl[..=MAX_SLEEVE]).unwrap().len(), MAX_SLEEVE);
}

#[test]
fn reduce_keeps_same_direction_repeats() {
    let a = Crossing {
        left: 1,
        right: 2,
        t: 0.5,
    };
    let b = Crossing {
        left: 2,
        right: 1,
        t: 0.5,
    };
    assert_eq!(Cdt::reduce(&[a, a]).len(), 2);
    assert_eq!(Cdt::reduce(&[a, b, a]), vec![a]);
}
