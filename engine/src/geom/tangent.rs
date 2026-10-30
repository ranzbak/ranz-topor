//! Point–circle and circle–circle tangents (21 §1–2).
use super::prim::{Circle, Seg};
use super::vec2::{Orient, Vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GeomError {
    InsideObstacle,
    Infeasible,
    Degenerate,
}

/// Tangent point on `c` for a wire leaving `p` and wrapping `c` with orientation `sigma`.
pub fn point_circle_tangent(p: Vec2, c: Circle, sigma: Orient) -> Result<Vec2, GeomError> {
    let w = p - c.c;
    let d = w.len();
    if d <= c.r {
        return Err(GeomError::InsideObstacle);
    }
    let u = w.unit().ok_or(GeomError::Degenerate)?;
    let alpha = (c.r / d).acos();
    for h in [1.0, -1.0] {
        let t = c.c + u.rot(h * alpha) * c.r;
        if sigma.sign() * (t - p).cross(c.c - t) > 0.0 {
            return Ok(t);
        }
    }
    Err(GeomError::Degenerate)
}

/// Candidate tangent lines for sign choice `(s1, s2)` and `h`; `None` if none exists.
fn candidate(c1: Circle, c2: Circle, s1: f64, s2: f64, h: f64) -> Option<Seg> {
    let dv = c2.c - c1.c;
    let d = dv.len();
    let v = dv.unit()?;
    let (r1, r2) = (s1 * c1.r, s2 * c2.r);
    let k = (r2 - r1) / d;
    if k.abs() > 1.0 {
        return None;
    }
    let n = v * k + v.perp() * (h * (1.0 - k * k).max(0.0).sqrt());
    Some(Seg {
        a: c1.c - n * r1,
        b: c2.c - n * r2,
    })
}

/// The bitangent leaving `c1` with orientation `s1` and arriving at `c2` with `s2`.
pub fn bitangent(c1: Circle, s1: Orient, c2: Circle, s2: Orient) -> Result<Seg, GeomError> {
    let d = (c2.c - c1.c).len();
    if d < 1e-9 {
        return Err(GeomError::Degenerate);
    }
    let sg2 = if s1 == s2 { 1.0 } else { -1.0 };
    if s1 != s2 && d < c1.r + c2.r {
        return Err(GeomError::Infeasible);
    }
    for h in [1.0, -1.0] {
        if let Some(seg) = candidate(c1, c2, 1.0, sg2, h) {
            if let Some(t) = seg.dir() {
                if s1.sign() * t.cross(c1.c - seg.a) > 0.0
                    && s2.sign() * t.cross(c2.c - seg.b) > 0.0
                {
                    return Ok(seg);
                }
            }
        }
    }
    Err(GeomError::Infeasible)
}

/// All distinct bitangents (at most 4), directed from `c1` to `c2`.
pub fn all_bitangents(c1: Circle, c2: Circle) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::with_capacity(4);
    for s2 in [1.0, -1.0] {
        for h in [1.0, -1.0] {
            if let Some(seg) = candidate(c1, c2, 1.0, s2, h) {
                let dup = out
                    .iter()
                    .any(|o| (o.a - seg.a).len() < 1e-9 && (o.b - seg.b).len() < 1e-9);
                if !dup {
                    out.push(seg);
                }
            }
        }
    }
    out
}
