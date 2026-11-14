//! Exact orientation of `f64` points: the sign of `(b - a) x (c - a)` of the points as given,
//! never a rounded one. Shewchuk, "Adaptive precision floating-point arithmetic and fast robust
//! geometric predicates", Discrete Comput. Geom. 18 (1997) 305-363: the rounded determinant with
//! his forward error bound `ccwerrboundA` (sec. 4.3, p. 340), and when the bound does not settle
//! the sign the exact value as a floating-point expansion (Two-Sum, Two-Product, Grow-Expansion;
//! secs. 2.4-2.6, pp. 314-324; Two-Product by fused multiply-add). Exact for every finite input
//! whose products neither overflow nor underflow: board coordinates are nm below 2^40, so the
//! products stay below 2^81 and the error terms far above the subnormal range.
//!
//! [`on_ray`] orders three collinear points by comparing coordinates only, so it is exact too.
//! The funnel ([`crate::topo::pack::funnel`]) decides every turn with these, so two tests on
//! the same points never disagree by rounding.

use super::Vec2;

/// Half an ulp of 1 (Shewchuk's epsilon).
const EPS: f64 = f64::EPSILON * 0.5;
/// Shewchuk's `ccwerrboundA`: a rounded `|det| > ERR * (|l| + |r|)` has the exact sign.
const ERR: f64 = (3.0 + 16.0 * EPS) * EPS;

fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let s = a + b;
    let bv = s - a;
    (s, (a - (s - bv)) + (b - bv))
}

fn two_prod(a: f64, b: f64) -> (f64, f64) {
    let p = a * b;
    (p, a.mul_add(b, -p))
}

/// Sign of the exact sum of `terms`: Grow-Expansion with zero elimination keeps a
/// non-overlapping expansion in increasing magnitude, whose sign is that of its last component.
fn sum_sign(terms: &[f64]) -> i32 {
    let mut e: Vec<f64> = Vec::with_capacity(terms.len());
    let mut h: Vec<f64> = Vec::with_capacity(terms.len());
    for &t in terms {
        h.clear();
        let mut q = t;
        for &x in &e {
            let (s, err) = two_sum(q, x);
            if err != 0.0 {
                h.push(err);
            }
            q = s;
        }
        if q != 0.0 {
            h.push(q);
        }
        std::mem::swap(&mut e, &mut h);
    }
    match e.last() {
        Some(&x) if x > 0.0 => 1,
        Some(&x) if x < 0.0 => -1,
        _ => 0,
    }
}

/// Sign of `(b - a) x (c - a)`, exact: +1 when `c` lies left of the line `a -> b`, -1 right, 0 on
/// it. O(1); the expansion stage runs only when the rounded value is within its error bound.
pub fn orient(a: Vec2, b: Vec2, c: Vec2) -> i32 {
    let l = (b.x - a.x) * (c.y - a.y);
    let r = (b.y - a.y) * (c.x - a.x);
    let det = l - r;
    let bound = ERR * (l.abs() + r.abs());
    if det > bound {
        return 1;
    }
    if -det > bound {
        return -1;
    }
    let (d1, e1) = two_sum(b.x, -a.x);
    let (d2, e2) = two_sum(c.y, -a.y);
    let (d3, e3) = two_sum(b.y, -a.y);
    let (d4, e4) = two_sum(c.x, -a.x);
    let mut t = [0.0f64; 16];
    let pairs = [(d1, d2, 1.0), (d1, e2, 1.0), (e1, d2, 1.0), (e1, e2, 1.0), (d3, d4, -1.0), (d3, e4, -1.0), (e3, d4, -1.0), (e3, e4, -1.0)];
    for (k, &(u, v, sg)) in pairs.iter().enumerate() {
        let (p, q) = two_prod(u, v);
        t[2 * k] = sg * p;
        t[2 * k + 1] = sg * q;
    }
    sum_sign(&t)
}

/// Where `p` lies on the line through `a` and `c` (`a != c`, the three collinear, e.g.
/// `orient(a, c, p) == 0`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OnRay {
    /// At `a` or on the far side of `a` from `c`.
    Behind,
    /// Strictly between `a` and `c`.
    Between,
    /// At `c` or beyond it.
    AtOrBeyond,
}

/// [`OnRay`] of `p`: compared along an axis on which `c` differs from `a` (which orders a line's
/// points), so by coordinate comparisons only. O(1).
pub fn on_ray(a: Vec2, c: Vec2, p: Vec2) -> OnRay {
    let (ak, ck, pk) = if c.x != a.x { (a.x, c.x, p.x) } else { (a.y, c.y, p.y) };
    let fwd = ck > ak;
    let before = |u: f64, v: f64| if fwd { u < v } else { u > v };
    if !before(ak, pk) {
        OnRay::Behind
    } else if before(pk, ck) {
        OnRay::Between
    } else {
        OnRay::AtOrBeyond
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x, y }
    }

    #[test]
    fn plain_turns() {
        assert_eq!(orient(v(0.0, 0.0), v(1.0, 0.0), v(0.0, 1.0)), 1);
        assert_eq!(orient(v(0.0, 0.0), v(1.0, 0.0), v(0.0, -1.0)), -1);
        assert_eq!(orient(v(0.0, 0.0), v(1.0, 0.0), v(7.0, 0.0)), 0);
        assert_eq!(orient(v(68e6, -3e6), v(68e6, -70.125e6), v(68e6, -3.2343e6)), 0);
    }

    /// Points a rounded determinant gets wrong (Shewchuk's fig. 1 setting): `a` steps by ulps
    /// near (0.5, 0.5) against the line through (12, 12) and (24, 24). Exactly
    /// `(b - a) x (c - a) = 12 (ay - ax)`, so the sign is that of `j - i`; the rounded formula
    /// misses it for some of the points.
    #[test]
    fn near_collinear_is_exact() {
        let step = f64::EPSILON;
        let (b, c) = (v(12.0, 12.0), v(24.0, 24.0));
        let mut rounded_wrong = 0;
        for i in 0..64 {
            for j in 0..64 {
                let a = v(0.5 + i as f64 * step, 0.5 + j as f64 * step);
                let want = (j as i64 - i as i64).signum() as i32;
                assert_eq!(orient(a, b, c), want, "i {i} j {j}");
                let naive = (b - a).cross(c - a);
                let ns = if naive > 0.0 { 1 } else if naive < 0.0 { -1 } else { 0 };
                rounded_wrong += (ns != want) as usize;
            }
        }
        assert!(rounded_wrong > 0, "the rounded determinant should fail here");
    }

    #[test]
    fn on_ray_orders_collinear_points() {
        let (a, c) = (v(0.0, 0.0), v(0.0, -4.0));
        assert_eq!(on_ray(a, c, v(0.0, 1.0)), OnRay::Behind);
        assert_eq!(on_ray(a, c, a), OnRay::Behind);
        assert_eq!(on_ray(a, c, v(0.0, -1.0)), OnRay::Between);
        assert_eq!(on_ray(a, c, c), OnRay::AtOrBeyond);
        assert_eq!(on_ray(a, c, v(0.0, -9.0)), OnRay::AtOrBeyond);
        let (a, c) = (v(1.0, 1.0), v(3.0, 2.0));
        assert_eq!(on_ray(a, c, v(2.0, 1.5)), OnRay::Between);
        assert_eq!(on_ray(a, c, v(-1.0, 0.0)), OnRay::Behind);
    }
}
