//! Exact (closed-form) distances between segments and arcs, pad skeletons (23 §1).
//!
//! Every function returns the minimum Euclidean distance between the two
//! *curves* (centrelines); `0.0` when they touch or intersect. No sampling:
//! each routine takes the minimum over the finite critical-point candidate set
//! (boundary pairs, intersections, interior perpendicular pairs).
use super::prim::{Arc, Circle, Prim, Seg};
use super::vec2::Vec2;

/// Centres closer than this count as concentric (nm).
const CONCENTRIC_EPS: f64 = 1e-9;

/// Signed distance from `p` to the disc `c` (negative inside, `0` on the boundary).
pub fn point_circle(p: Vec2, c: &Circle) -> f64 {
    (p - c.c).len() - c.r
}

/// Distance from `p` to the closed segment.
pub fn point_seg(p: Vec2, s: &Seg) -> f64 {
    let d = s.b - s.a;
    let l2 = d.len2();
    if l2 == 0.0 {
        return (p - s.a).len();
    }
    let t = ((p - s.a).dot(d) / l2).clamp(0.0, 1.0);
    (p - (s.a + d * t)).len()
}

/// Distance between two closed segments.
pub fn seg_seg(a: &Seg, b: &Seg) -> f64 {
    // Proper crossing (also covers T-junctions via the point candidates below).
    let r = a.b - a.a;
    let q = b.b - b.a;
    let d1 = r.cross(b.a - a.a);
    let d2 = r.cross(b.b - a.a);
    let d3 = q.cross(a.a - b.a);
    let d4 = q.cross(a.b - b.a);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return 0.0;
    }
    point_seg(a.a, b)
        .min(point_seg(a.b, b))
        .min(point_seg(b.a, a))
        .min(point_seg(b.b, a))
}

/// Distance from `p` to the arc curve (centre case: `p == c` gives `r`).
pub fn point_arc(p: Vec2, a: &Arc) -> f64 {
    if p == a.c {
        return a.r; // every arc point is exactly r away
    }
    let mut d = (p - a.start()).len().min((p - a.end()).len());
    if a.contains_dir(p) {
        d = d.min(((p - a.c).len() - a.r).abs());
    }
    d
}

/// Unit-direction helper: is the point `c + dir` inside the arc's angular span.
fn dir_in_span(a: &Arc, dir: Vec2) -> bool {
    a.contains_dir(a.c + dir)
}

/// Distance between a closed segment and an arc.
pub fn seg_arc(s: &Seg, a: &Arc) -> f64 {
    let mut best = point_arc(s.a, a)
        .min(point_arc(s.b, a))
        .min(point_seg(a.start(), s))
        .min(point_seg(a.end(), s));
    let len = s.len();
    let Some(u) = s.dir() else {
        return best; // degenerate segment == point
    };
    // Foot of the centre on the segment's line (arc-length parameter).
    let w = s.a - a.c;
    let tf = -w.dot(u);
    // Line–circle intersection inside the segment and the span -> 0.
    let disc = tf * tf - (w.len2() - a.r * a.r);
    if disc >= 0.0 {
        let sq = disc.sqrt();
        for t in [tf - sq, tf + sq] {
            if (0.0..=len).contains(&t) && a.contains_dir(s.a + u * t) {
                return 0.0;
            }
        }
    }
    // Interior perpendicular pair: foot F of the centre and c ± r·n̂.
    if (0.0..=len).contains(&tf) {
        let f = s.a + u * tf;
        let off = f - a.c;
        let n = off.unit().unwrap_or_else(|| u.perp());
        for sg in [1.0, -1.0] {
            let dir = n * sg;
            if dir_in_span(a, dir) {
                best = best.min((f - (a.c + dir * a.r)).len());
            }
        }
    }
    best
}

/// Distance between two arcs.
pub fn arc_arc(a: &Arc, b: &Arc) -> f64 {
    let mut best = point_arc(a.start(), b)
        .min(point_arc(a.end(), b))
        .min(point_arc(b.start(), a))
        .min(point_arc(b.end(), a));
    let dv = b.c - a.c;
    let d = dv.len();
    if d < CONCENTRIC_EPS {
        // Concentric: radial gap if the angular spans share a direction,
        // otherwise only the endpoint candidates (already in `best`).
        let overlap = a.contains_dir(b.start() - b.c + a.c)
            || a.contains_dir(b.end() - b.c + a.c)
            || b.contains_dir(a.start() - a.c + b.c)
            || b.contains_dir(a.end() - a.c + b.c);
        if overlap {
            best = best.min((a.r - b.r).abs());
        }
        return best;
    }
    let u = dv * (1.0 / d);
    // Circle–circle intersection points inside both spans -> 0.
    let along = (d * d + a.r * a.r - b.r * b.r) / (2.0 * d);
    let h2 = a.r * a.r - along * along;
    if h2 >= 0.0 {
        let h = h2.sqrt();
        for sg in [1.0, -1.0] {
            let dir = u * along + u.perp() * (h * sg);
            if dir_in_span(a, dir) && b.contains_dir(a.c + dir) {
                return 0.0;
            }
        }
    }
    // Interior critical pairs lie on the line of centres.
    for s1 in [1.0, -1.0] {
        if !dir_in_span(a, u * s1) {
            continue;
        }
        let p1 = a.c + u * (s1 * a.r);
        for s2 in [1.0, -1.0] {
            if dir_in_span(b, u * s2) {
                let p2 = b.c + u * (s2 * b.r);
                best = best.min((p1 - p2).len());
            }
        }
    }
    best
}

/// Distance between two centreline primitives.
pub fn dist_prim(a: &Prim, b: &Prim) -> f64 {
    match (a, b) {
        (Prim::Seg(x), Prim::Seg(y)) => seg_seg(x, y),
        (Prim::Seg(x), Prim::Arc(y)) | (Prim::Arc(y), Prim::Seg(x)) => seg_arc(x, y),
        (Prim::Arc(x), Prim::Arc(y)) => arc_arc(x, y),
    }
}

fn point_prim(p: Vec2, a: &Prim) -> f64 {
    match a {
        Prim::Seg(s) => point_seg(p, s),
        Prim::Arc(x) => point_arc(p, x),
    }
}

/// Pad skeleton (23 §1 table): the pad is the Minkowski sum of this core with a disc of radius `r`.
#[derive(Clone, Debug, PartialEq)]
pub enum Skeleton {
    Point {
        p: Vec2,
        r: f64,
    },
    Seg {
        s: Seg,
        r: f64,
    },
    /// Convex polygon, counter-clockwise.
    Poly {
        pts: Vec<Vec2>,
        r: f64,
    },
}

impl Skeleton {
    /// Round pad of diameter `d`.
    pub fn circle(c: Vec2, d: f64) -> Skeleton {
        Skeleton::Point { p: c, r: d * 0.5 }
    }

    /// Oval `w × h` rotated CCW by `rot` rad: axis segment of length `|w−h|`, `r = min/2`.
    pub fn oval(c: Vec2, w: f64, h: f64, rot: f64) -> Skeleton {
        let r = 0.5 * w.min(h);
        let half = 0.5 * (w - h).abs();
        let axis = if w >= h {
            Vec2 { x: half, y: 0.0 }
        } else {
            Vec2 { x: 0.0, y: half }
        }
        .rot(rot);
        if half == 0.0 {
            return Skeleton::Point { p: c, r };
        }
        Skeleton::Seg {
            s: Seg {
                a: c - axis,
                b: c + axis,
            },
            r,
        }
    }

    /// Rectangle `w × h` rotated CCW by `rot` rad.
    pub fn rect(c: Vec2, w: f64, h: f64, rot: f64) -> Skeleton {
        Skeleton::round_rect(c, w, h, 0.0, rot)
    }

    /// Rounded rectangle, corner radius `rc` (clamped to `min(w,h)/2`): inset rectangle + `r = rc`.
    pub fn round_rect(c: Vec2, w: f64, h: f64, rc: f64, rot: f64) -> Skeleton {
        let rc = rc.clamp(0.0, 0.5 * w.min(h));
        let hx = 0.5 * w - rc;
        let hy = 0.5 * h - rc;
        let pts = [(-hx, -hy), (hx, -hy), (hx, hy), (-hx, hy)]
            .iter()
            .map(|&(x, y)| c + Vec2 { x, y }.rot(rot))
            .collect();
        Skeleton::Poly { pts, r: rc }
    }
}

/// Distance of a primitive to a convex CCW polygon (region): `0` on contact,
/// negative (minus the distance to the boundary) when the primitive lies inside.
fn prim_poly(p: &Prim, pts: &[Vec2]) -> f64 {
    let n = pts.len();
    if n == 0 {
        return f64::INFINITY;
    }
    let mut d = f64::INFINITY;
    for i in 0..n {
        let e = Seg {
            a: pts[i],
            b: pts[(i + 1) % n],
        };
        d = d.min(dist_prim(p, &Prim::Seg(e)));
    }
    if n >= 3 && d > 0.0 {
        // No boundary contact: the connected primitive is entirely in or out.
        let q = match p {
            Prim::Seg(s) => s.a,
            Prim::Arc(a) => a.start(),
        };
        let inside = (0..n).all(|i| (pts[(i + 1) % n] - pts[i]).cross(q - pts[i]) >= 0.0);
        if inside {
            return -d;
        }
    }
    d
}

/// `dist(centreline, skeleton) − r_skel`; ≤ 0 when the centreline touches the pad's core region.
pub fn dist_prim_skeleton(p: &Prim, sk: &Skeleton) -> f64 {
    match sk {
        Skeleton::Point { p: c, r } => point_prim(*c, p) - r,
        Skeleton::Seg { s, r } => dist_prim(p, &Prim::Seg(*s)) - r,
        Skeleton::Poly { pts, r } => prim_poly(p, pts) - r,
    }
}

/// A routed track: centreline plus width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Track {
    pub prim: Prim,
    pub w: f64,
}

/// Copper-to-copper gap between two tracks.
pub fn copper_gap(a: &Track, b: &Track) -> f64 {
    dist_prim(&a.prim, &b.prim) - 0.5 * a.w - 0.5 * b.w
}

/// Copper gap between a track and a pad skeleton.
pub fn copper_gap_skeleton(t: &Track, sk: &Skeleton) -> f64 {
    dist_prim_skeleton(&t.prim, sk) - 0.5 * t.w
}
