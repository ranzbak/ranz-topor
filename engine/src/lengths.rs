//! Length helpers shared by the stubs and the reports: copper length outside the terminal
//! pads and hand-copper length of a wire.

use crate::geom::{dist_prim_skeleton, Prim, Seg, Skeleton, Vec2};
use crate::model::BoardModel;
use crate::wires::{Terminal, Wire};
use std::f64::consts::PI;

/// Inside interval `[t0, t1]` of the segment `a→b` within pad skeleton `sk` (distance to a convex
/// skeleton is convex along the segment: ternary search for the minimum, bisection for the ends).
/// Every loop runs a fixed 48 iterations.
pub(crate) fn inside_interval(a: Vec2, b: Vec2, sk: &Skeleton) -> Option<(f64, f64)> {
    let g = |t: f64| {
        let p = a + (b - a) * t;
        dist_prim_skeleton(&Prim::Seg(Seg { a: p, b: p }), sk)
    };
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..48 {
        let (m1, m2) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
        if g(m1) <= g(m2) {
            hi = m2;
        } else {
            lo = m1;
        }
    }
    let tm = 0.5 * (lo + hi);
    if g(tm) > 0.0 {
        return None;
    }
    let edge = |mut out: f64, mut inn: f64| {
        for _ in 0..48 {
            let m = 0.5 * (out + inn);
            if g(m) <= 0.0 {
                inn = m;
            } else {
                out = m;
            }
        }
        inn
    };
    let t0 = if g(0.0) <= 0.0 { 0.0 } else { edge(0.0, tm) };
    let t1 = if g(1.0) <= 0.0 { 1.0 } else { edge(1.0, tm) };
    Some((t0, t1))
}

/// Length of the chord `a→b` (representing `len` of copper) outside every skeleton.
pub(crate) fn chord_outside(a: Vec2, b: Vec2, len: f64, sks: &[Skeleton]) -> f64 {
    let mut iv: Vec<(f64, f64)> = sks
        .iter()
        .filter_map(|s| inside_interval(a, b, s))
        .collect();
    iv.sort_by(|x, y| x.0.total_cmp(&y.0));
    let (mut covered, mut end) = (0.0, 0.0f64);
    for (t0, t1) in iv {
        let t0 = t0.max(end);
        if t1 > t0 {
            covered += t1 - t0;
            end = t1;
        }
    }
    len * (1.0 - covered).max(0.0)
}

/// Length of `prims` outside the terminal pads' copper (arcs chorded into ≤ 64 pieces).
pub(crate) fn outside_len(prims: &[Prim], sks: &[Skeleton]) -> f64 {
    if sks.is_empty() {
        return prims.iter().map(Prim::length).sum();
    }
    let mut total = 0.0;
    for p in prims {
        match p {
            Prim::Seg(s) => total += chord_outside(s.a, s.b, (s.b - s.a).len(), sks),
            Prim::Arc(a) => {
                let n = ((a.sweep / (PI / 32.0)).ceil() as usize).clamp(1, 64);
                let pt = |k: usize| {
                    let ang = a.a_start + a.sigma.sign() * a.sweep * (k as f64 / n as f64);
                    a.c + Vec2 {
                        x: ang.cos(),
                        y: ang.sin(),
                    } * a.r
                };
                let piece = a.length() / n as f64;
                for k in 0..n {
                    total += chord_outside(pt(k), pt(k + 1), piece, sks);
                }
            }
        }
    }
    total
}

pub(crate) fn terminal_skeletons(model: &BoardModel, w: &Wire) -> Vec<Skeleton> {
    [&w.start, &w.goal]
        .into_iter()
        .filter_map(|t| match t {
            Terminal::Pad { pad, .. } => model.pads.get(*pad).map(|p| model.skeleton(p, w.layer)),
            _ => None,
        })
        .collect()
}

pub(crate) fn hand_outside(w: &Wire, sks: &[Skeleton]) -> f64 {
    let segs: Vec<Prim> = w
        .hand
        .windows(2)
        .map(|v| Prim::Seg(Seg { a: v[0], b: v[1] }))
        .collect();
    outside_len(&segs, sks)
}
