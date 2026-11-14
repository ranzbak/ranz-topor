//! Exact orientation of `f64` points: the sign of `(b - a) x (c - a)` of the points as given,
//! never a rounded one. J. R. Shewchuk, "Adaptive Precision Floating-Point Arithmetic and Fast
//! Robust Geometric Predicates", Discrete & Computational Geometry 18(3):305-363, 1997 (local
//! copy docs/literature/shewchuk1997-robust-predicates.pdf, the CMU-CS-96-140R version, whose
//! page numbers are cited): the rounded determinant (approximation A) with its forward error
//! bound `ccwerrboundA = (3 eps + 16 eps^2)(|l| + |r|)` (sec. 4.3 "Orient2D", pp. 40-41,
//! Table 1), and when the bound does not settle the sign the exact value as a floating-point
//! expansion: Two-Sum (sec. 2.3 "Simple Addition", Theorem 7, p. 8), Grow-Expansion (sec. 2.4
//! "Expansion Addition", Theorem 10, p. 10, with zero elimination) and Two-Product (sec. 2.5
//! "Simple Multiplication", Theorem 18, p. 19; here by fused multiply-add instead of Veltkamp's
//! splitting). Exact for every finite input
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

    /// Exact sign of `(b - a) x (c - a)` in integers: every coordinate is a multiple of the
    /// smallest ulp among them, so scaled by its inverse they are integers below 2^62 here, and
    /// the determinant fits an `i128`. Independent of the floating-point code under test.
    fn exact_sign_i128(a: Vec2, b: Vec2, c: Vec2) -> i32 {
        let cs = [a.x, a.y, b.x, b.y, c.x, c.y];
        // x = m 2^e with an integer m below 2^53: e = exponent - 52.
        let ex = |x: f64| ((x.to_bits() >> 52) & 0x7ff) as i32 - 1075;
        let lo = cs.iter().filter(|x| **x != 0.0).map(|&x| ex(x)).min().unwrap();
        let int = |x: f64| -> i128 {
            if x == 0.0 {
                return 0;
            }
            let m = ((x.to_bits() & ((1u64 << 52) - 1)) | (1u64 << 52)) as i128;
            let sh = ex(x) - lo;
            assert!((0..=9).contains(&sh), "spread too wide for i128");
            if x < 0.0 { -(m << sh) } else { m << sh }
        };
        let [ax, ay, bx, by, cx, cy] = cs.map(int);
        ((bx - ax) * (cy - ay) - (by - ay) * (cx - ax)).signum() as i32
    }

    /// Near-collinear points (found by search) on which the rounded determinant has the wrong
    /// sign although it exceeds half of `ccwerrboundA`: a filter with half the bound returns
    /// that wrong sign; the full bound sends them to the exact stage. The expected signs come
    /// from integer arithmetic, not from the code under test.
    #[test]
    fn wrong_rounded_signs_beyond_half_the_filter_bound() {
        let cases = [
            (v(276993121.243269, -27984422.3283785), v(-46854380.34368849, 581143420.1763375), v(129311970.79621474, 249790456.61807504)),
            (v(479452.39791749895, -100607.69462204688), v(1557724.7234042152, 1138560.729161283), v(-509066.7009900735, -1236630.2231795646)),
            (v(366941.6455463168, -228747.2075937671), v(951867.3685483336, -1382105.746427564), v(80099.54117514272, 336849.03371294)),
            (v(-982802368.7584409, 685672380.4583789), v(-350427949.7536838, 1845508380.6645164), v(-349050944.44291157, 1848033941.9204924)),
            (v(-373.5636484590166, 215.3242519048786), v(-1150.351776753227, 479.87963961367564), v(671.479801722148, -140.5924582826675)),
            (v(283.00109126841863, 295.9943521636093), v(808.4506346564488, -1110.8049879443556), v(-212.9042092059312, 1623.6941441647032)),
        ];
        let mut signs = [0usize; 2];
        for (k, &(a, b, c)) in cases.iter().enumerate() {
            let want = exact_sign_i128(a, b, c);
            assert_ne!(want, 0, "case {k}");
            signs[(want > 0) as usize] += 1;
            assert_eq!(orient(a, b, c), want, "case {k}");
            // The case is adversarial: the rounded value has the wrong sign and lies between
            // half the bound and the bound.
            let (l, r) = ((b.x - a.x) * (c.y - a.y), (b.y - a.y) * (c.x - a.x));
            let (det, bound) = (l - r, ERR * (l.abs() + r.abs()));
            assert!(det.signum() as i32 == -want && det.abs() > 0.5 * bound && det.abs() <= bound, "case {k}: {det} {bound}");
        }
        assert!(signs[0] > 0 && signs[1] > 0, "both signs");
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
