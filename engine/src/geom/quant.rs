//! Quantisation to integer nm and the KiCad three-point arc centre (23 §4).
use super::prim::{Arc, Prim, Seg};
use super::vec2::Vec2;
use super::{MIN_CHORD_NM, MIN_SAGITTA_NM};
use serde::{Deserialize, Serialize};

pub type Nm = i32;

/// Integer-nm point (KiCad internal units).
/// Serialised as a two-element array `[x, y]` (the wire schema of the board model).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(from = "[Nm; 2]", into = "[Nm; 2]")]
pub struct IVec2 {
    pub x: Nm,
    pub y: Nm,
}

impl From<[Nm; 2]> for IVec2 {
    fn from(a: [Nm; 2]) -> Self {
        IVec2 { x: a[0], y: a[1] }
    }
}

impl From<IVec2> for [Nm; 2] {
    fn from(v: IVec2) -> Self {
        [v.x, v.y]
    }
}

impl IVec2 {
    pub fn to_f64(self) -> Vec2 {
        Vec2 {
            x: self.x as f64,
            y: self.y as f64,
        }
    }
}

/// Round half away from zero (KiCad `KiROUND`); saturates at the `i32` range.
pub fn round_nm(v: Vec2) -> IVec2 {
    IVec2 {
        x: v.x.round() as Nm,
        y: v.y.round() as Nm,
    }
}

/// Circle centre through three integer points (perpendicular-bisector
/// intersection in `f64`), rounded; `None` if collinear or the centre is
/// outside the `i32` range.
pub fn calc_arc_center(s: IVec2, m: IVec2, e: IVec2) -> Option<IVec2> {
    let (ux, uy) = (m.x as i128 - s.x as i128, m.y as i128 - s.y as i128);
    let (vx, vy) = (e.x as i128 - s.x as i128, e.y as i128 - s.y as i128);
    if ux * vy - uy * vx == 0 {
        return None; // exactly collinear (also covers coincident points)
    }
    let c = circle_center(s.to_f64(), m.to_f64(), e.to_f64())?;
    let lim = Nm::MAX as f64;
    if !(c.x.abs() < lim && c.y.abs() < lim) {
        return None;
    }
    Some(round_nm(c))
}

fn circle_center(s: Vec2, m: Vec2, e: Vec2) -> Option<Vec2> {
    let u = m - s;
    let v = e - s;
    let cr = u.cross(v);
    if cr == 0.0 || !cr.is_finite() {
        return None;
    }
    let off = Vec2 {
        x: v.y * u.len2() - u.y * v.len2(),
        y: u.x * v.len2() - v.x * u.len2(),
    } * (0.5 / cr);
    Some(s + off)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrackPrim {
    Seg {
        a: IVec2,
        b: IVec2,
    },
    Arc {
        start: IVec2,
        mid: IVec2,
        end: IVec2,
    },
}

impl TrackPrim {
    pub fn start(&self) -> IVec2 {
        match *self {
            TrackPrim::Seg { a, .. } => a,
            TrackPrim::Arc { start, .. } => start,
        }
    }
    pub fn end(&self) -> IVec2 {
        match *self {
            TrackPrim::Seg { b, .. } => b,
            TrackPrim::Arc { end, .. } => end,
        }
    }
}

/// Whether an arc is emitted as a segment (23 §4.3).
///
/// Only arcs with `sweep <= π` can be degenerate: the sagitta of a bigger arc
/// exceeds `r`, and its chord (→ 0 as sweep → 2π) says nothing about flatness.
/// Sagitta is evaluated as `2r·sin²(sweep/4)` to avoid `1 − cos` cancellation.
pub fn is_degenerate_arc(a: &Arc) -> bool {
    if a.sweep > std::f64::consts::PI {
        return false;
    }
    let sag = 2.0 * a.r * (a.sweep * 0.25).sin().powi(2);
    sag < MIN_SAGITTA_NM || a.chord() < MIN_CHORD_NM
}

/// `None` when the primitive collapses to a point (`start == end` after
/// rounding and not a representable arc); the caller drops it.
fn quantise_with(p: &Prim, start: IVec2, end: IVec2) -> Option<TrackPrim> {
    let seg = || (start != end).then_some(TrackPrim::Seg { a: start, b: end });
    match p {
        Prim::Seg(_) => seg(),
        Prim::Arc(a) => {
            let mid = round_nm(a.mid());
            if is_degenerate_arc(a) || calc_arc_center(start, mid, end).is_none() {
                seg()
            } else {
                Some(TrackPrim::Arc { start, mid, end })
            }
        }
    }
}

fn ends(p: &Prim) -> (Vec2, Vec2) {
    match p {
        Prim::Seg(s) => (s.a, s.b),
        Prim::Arc(a) => (a.start(), a.end()),
    }
}

/// Quantise one primitive. `None` means it is zero-length at integer-nm
/// resolution and must be dropped (never emit a zero-length segment). Note a
/// full circle whose start and end round to the same point also lands here:
/// three points cannot represent it.
pub fn quantise(p: &Prim) -> Option<TrackPrim> {
    let (s, e) = ends(p);
    quantise_with(p, round_nm(s), round_nm(e))
}

/// Quantise a chain; each shared endpoint is rounded once and reused
/// bit-identically (23 §4.4). Zero-length items are omitted; since such an
/// item has `start == end`, the neighbours still share their endpoint.
pub fn quantise_chain(prims: &[Prim]) -> Vec<TrackPrim> {
    let mut out = Vec::with_capacity(prims.len());
    let mut prev: Option<IVec2> = None;
    for p in prims {
        let (s, e) = ends(p);
        let start = prev.unwrap_or_else(|| round_nm(s));
        let end = round_nm(e);
        out.extend(quantise_with(p, start, end));
        prev = Some(end);
    }
    out
}

/// What KiCad will see: arcs are rebuilt from the three integer points.
pub fn dequantise(t: &TrackPrim) -> Prim {
    match *t {
        TrackPrim::Seg { a, b } => Prim::Seg(Seg {
            a: a.to_f64(),
            b: b.to_f64(),
        }),
        TrackPrim::Arc { start, mid, end } => {
            let arc = calc_arc_center(start, mid, end)
                .and_then(|_| Arc::from_three_points(start.to_f64(), mid.to_f64(), end.to_f64()));
            match arc {
                Some(a) => Prim::Arc(a),
                None => Prim::Seg(Seg {
                    a: start.to_f64(),
                    b: end.to_f64(),
                }),
            }
        }
    }
}
