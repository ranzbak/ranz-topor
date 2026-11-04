//! The funnel of the sleeve (11 §2 step 4, Lee-Preparata) and the anchor reference `taut` works
//! on. Restored from `main` for the metrization gate (`topo::metrize`); only `funnel` and
//! `AnchorRef` are kept (the old sketch-of-a-hand-wire machinery stays on `main`).
//!
//! O(k) per wire (k = sleeve length); no randomness.

use crate::geom::{Orient, Vec2};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AnchorRef {
    /// Index into `LayerObstacles.anchors`.
    pub anchor: usize,
    /// `Ccw`: obstacle on the left of the wire; `Cw`: on the right.
    pub sigma: Orient,
}


fn cross3(o: Vec2, a: Vec2, b: Vec2) -> f64 {
    (a - o).cross(b - o)
}

/// Chain entry: position, portal index, on the left chain.
type Ent = (Vec2, usize, bool);

/// Prediction for `prev` of the last chain element: the one before it, or the apex.
fn prev_of(ch: &VecDeque<Ent>, apex: Vec2) -> Vec2 {
    let n = ch.len();
    if n >= 2 {
        ch[n - 2].0
    } else {
        apex
    }
}

/// Funnel (11 §2 step 4, Lee-Preparata) over portals `(left, right)` seen in the direction of
/// travel; returns interior vertices `(portal index, is_left, σ)`. A left bend has the
/// obstacle on the left (`Ccw`), a right bend on the right (`Cw`). Each portal vertex is pushed
/// once and popped at most once: O(k). A portal vertex exactly on the string (a collinear tie,
/// Fix-B1 (c)) is kept on its chain and becomes a bend on its portal side: the slot path passes
/// it on that side, and an inflated anchor on the string is never slack.
pub fn funnel(start: Vec2, goal: Vec2, portals: &[(Vec2, Vec2)]) -> Vec<(usize, bool, Orient)> {
    let k = portals.len();
    let mut out = Vec::new();
    let mut apex = start;
    let mut left: VecDeque<Ent> = VecDeque::new();
    let mut right: VecDeque<Ent> = VecDeque::new();
    let mut steps = 0usize;
    let cap = 8 * (k + 1) + 8;
    for (i, &(l, r)) in portals
        .iter()
        .chain(std::iter::once(&(goal, goal)))
        .enumerate()
    {
        // right vertex
        if r != apex && right.back().is_none_or(|e| e.0 != r) {
            while !right.is_empty()
                && cross3(prev_of(&right, apex), right.back().unwrap().0, r) > 0.0
            {
                right.pop_back();
                steps += 1;
            }
            if right.is_empty() {
                while let Some(&(lp, li, _)) = left.front() {
                    if li == k || cross3(apex, lp, r) < 0.0 || steps > cap {
                        break;
                    }
                    apex = lp;
                    out.push((li, true, Orient::Ccw));
                    left.pop_front();
                    steps += 1;
                }
            }
            right.push_back((r, i, false));
        }
        // left vertex
        if l != apex && left.back().is_none_or(|e| e.0 != l) {
            while !left.is_empty() && cross3(prev_of(&left, apex), left.back().unwrap().0, l) < 0.0
            {
                left.pop_back();
                steps += 1;
            }
            if left.is_empty() {
                while let Some(&(rp, ri, _)) = right.front() {
                    if ri == k || cross3(apex, rp, l) > 0.0 || steps > cap {
                        break;
                    }
                    apex = rp;
                    out.push((ri, false, Orient::Cw));
                    right.pop_front();
                    steps += 1;
                }
            }
            left.push_back((l, i, true));
        }
    }
    debug_assert!(steps <= cap);
    out
}
