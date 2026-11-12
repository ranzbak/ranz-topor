//! The P1 demand model: how much of a cut (a CDT edge or an obstacle-pair segment) a set of
//! passes needs, on one layer. One definition for the witness, the P2 overflow test and P3.
//!
//! For passes ordered across the cut: the sum of the widths, plus between two neighbours of
//! different nets the larger of their clearances (neighbours of the same net need none), plus at
//! each end the end object's clearance. A pass of the net of the pad at an end may lie on that
//! pad's copper, so the run of such passes next to the end takes no space and the first pass
//! behind it is at the pad's clearance.
//! An end is a pad (its clearance is `max(pad net clearance, pad local clearance, board minimum,
//! NPTH hole clearance)` and at least the pass's own), the board edge (the copper-to-edge rule
//! only), or open (no object, no term).
//!
//! Complexity: O(passes).

/// One pass across a cut: net, track width, the net's clearance (nm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub net: i32,
    pub w: f64,
    pub s: f64,
}

/// What stops the cut at one end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bound {
    /// Pad copper: its net (`None` = no net) and its own clearance requirement.
    Pad { net: Option<i32>, clear: f64 },
    /// A bare hole (NPTH without copper): only the hole clearance applies, whatever the net.
    Hole { clear: f64 },
    /// Board edge with the copper-to-edge clearance.
    Edge { clear: f64 },
    /// Nothing (a free vertex): no term.
    Open,
}

impl Bound {
    /// Clearance a pass `it` needs to this end (0 for its own net).
    pub fn clearance(&self, it: &Item) -> f64 {
        match *self {
            Bound::Pad { net, clear } => {
                if net == Some(it.net) {
                    0.0
                } else {
                    clear.max(it.s)
                }
            }
            Bound::Edge { clear } | Bound::Hole { clear } => clear,
            Bound::Open => 0.0,
        }
    }
}

/// Space the `items` (ordered across the cut from end `lo` to end `hi`) need on one layer.
///
/// A run of passes of the end pad's own net next to that end may lie on the pad's copper, but
/// its centre lines cross the cut: at best on the end vertex itself, so the run reaches half its
/// widest width into the cut (M-C, the same as the drawing's packer, `Packer::margin`: margin 0
/// at an own-net end, `clearance + w/2` at the other). Before M-C the run took no space, so P2
/// planned pad exits between fine-pitch neighbours that the packer and the drawing reject
/// (Dayan 1997 pp. 15-16: a cut must hold the widths of the wires that cross it).
pub fn need(items: &[Item], lo: Bound, hi: Bound) -> f64 {
    need_by(items.len(), |i| items[i], lo, hi)
}

/// [`need`] of the `n` items `get(0..n)` (ordered across the cut), without building them: the
/// one definition [`need`] and the cut model's repricing share (shopping list 3.1: no
/// allocation per cut; the same operations in the same order, so the same floats). O(n).
pub fn need_by(n: usize, get: impl Fn(usize) -> Item, lo: Bound, hi: Bound) -> f64 {
    let own = |b: &Bound, it: &Item| matches!(*b, Bound::Pad { net: Some(m), .. } if m == it.net);
    // The run `a..b` of own-net passes at an end as one item of half the widest width.
    let half = |a: usize, b: usize| -> Option<Item> {
        if a >= b {
            return None;
        }
        let f = get(a);
        Some(Item { net: f.net, w: 0.5 * (a..b).map(|i| get(i).w).fold(0.0, f64::max), s: (a..b).map(|i| get(i).s).fold(0.0, f64::max) })
    };
    let skip_lo = (0..n).take_while(|&i| own(&lo, &get(i))).count();
    let skip_hi = (skip_lo..n).rev().take_while(|&i| own(&hi, &get(i))).count();
    let run_lo = half(0, skip_lo);
    let run_hi = half(n - skip_hi, n);
    let all = || run_lo.into_iter().chain((skip_lo..n - skip_hi).map(&get)).chain(run_hi);
    let (Some(first), Some(last)) = (all().next(), all().last()) else {
        return 0.0;
    };
    let mut need: f64 = all().map(|x| x.w).sum();
    let mut prev: Option<Item> = None;
    for x in all() {
        if let Some(p) = prev {
            if p.net != x.net {
                need += p.s.max(x.s);
            }
        }
        prev = Some(x);
    }
    // An own run at an end needs no clearance to it (its copper may touch the pad).
    let lo_c = if run_lo.is_some() { 0.0 } else { lo.clearance(&first) };
    let hi_c = if run_hi.is_some() { 0.0 } else { hi.clearance(&last) };
    need + lo_c + hi_c
}

/// [`need_in`] of the `n` items `get(0..n)` ([`need_by`]). O(n).
pub fn need_in_by(regime: Option<f64>, n: usize, get: impl Fn(usize) -> Item, lo: Bound, hi: Bound) -> f64 {
    let Some(r) = regime else { return need_by(n, get, lo, hi) };
    let rep = |b: Bound| match b {
        Bound::Pad { net, .. } => Bound::Pad { net, clear: r },
        o => o,
    };
    need_by(n, |i| Item { s: r, ..get(i) }, rep(lo), rep(hi))
}

/// [`need`] where a custom courtyard rule replaces the clearances (`regime`, nm): between passes
/// and to pads alike. The board-edge and hole clearances are separate rules and stay.
pub fn need_in(regime: Option<f64>, items: &[Item], lo: Bound, hi: Bound) -> f64 {
    need_in_by(regime, items.len(), |i| items[i], lo, hi)
}

/// Lower bound of the space `items` need on `k` layers together (all on the same layers, in any
/// distribution): the widths, plus at least `items.len() - k` neighbour gaps at the smallest
/// clearance. The end terms are left out (a layer may hold no pass next to an end).
pub fn need_lower(items: &[Item], k: usize) -> f64 {
    let w: f64 = items.iter().map(|x| x.w).sum();
    let s = items.iter().map(|x| x.s).fold(f64::INFINITY, f64::min);
    let gaps = items.len().saturating_sub(k);
    // Same-net neighbours need no gap, so the bound only holds for passes of distinct nets.
    let mut nets: Vec<i32> = items.iter().map(|x| x.net).collect();
    nets.sort_unstable();
    nets.dedup();
    let diff = nets.len().saturating_sub(k);
    w + if s.is_finite() { s * gaps.min(diff) as f64 } else { 0.0 }
}

/// Octilinear drawing style (M-45): how much more of a cut one pass takes than at a square
/// crossing (the packer's oblique factor, `>= 1`), when the drawing is octilinear. `ed`: the cut's
/// direction; `d`: the wire's taut crossing direction. A pass where the taut path wraps a corner
/// at the cut's end (`wrapped`, the cut radial from that corner) runs around the corner's
/// octagon, not its circle: the octagon's support along the cut is `1 / cos(delta)`, `delta` the
/// angle between the cut and the nearest octagon edge normal (a multiple of 45 degrees), so up to
/// `1 / cos(22.5) = 1.082` where the any-angle drawing needs 1. A straight pass is drawn with the
/// two octilinear directions that bracket `d` (an elbow `d = a D0 + b D1`); its factor is the
/// two directions' oblique factors weighted by their share `a : b` (exact for an octilinear `d`).
/// The planner (P3 / P4, through the packer) and the drawing count the room this way. O(1).
pub fn octi_stretch(ed: crate::geom::Vec2, d: crate::geom::Vec2, wrapped: bool) -> f64 {
    use std::f64::consts::FRAC_PI_4;
    let (Some(e), Some(u)) = (ed.unit(), d.unit()) else { return 1.0 };
    let obl = |v: crate::geom::Vec2| 1.0 / v.cross(e).abs().clamp(0.4, 1.0);
    if wrapped {
        let a = e.y.atan2(e.x).rem_euclid(FRAC_PI_4);
        let delta = a.min(FRAC_PI_4 - a);
        return 1.0 / delta.cos();
    }
    let ang = u.y.atan2(u.x).rem_euclid(std::f64::consts::TAU);
    let k0 = (ang / FRAC_PI_4).floor();
    let (a0, a1) = (k0 * FRAC_PI_4, (k0 + 1.0) * FRAC_PI_4);
    let (d0, d1) = (crate::geom::Vec2 { x: a0.cos(), y: a0.sin() }, crate::geom::Vec2 { x: a1.cos(), y: a1.sin() });
    let den = d0.cross(d1);
    let (al, be) = ((u.cross(d1) / den).max(0.0), (d0.cross(u) / den).max(0.0));
    if al + be <= 0.0 {
        return obl(u);
    }
    (al * obl(d0) + be * obl(d1)) / (al + be)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it(net: i32, w: f64, s: f64) -> Item {
        Item { net, w, s }
    }

    #[test]
    fn neighbours_of_one_net_need_no_gap() {
        let pad = Bound::Pad { net: None, clear: 200.0 };
        // a | pad: 100 + 200; two of net 1 between two pads of no net: 100+100+200+200
        assert_eq!(need(&[it(1, 100.0, 200.0)], pad, pad), 500.0);
        assert_eq!(need(&[it(1, 100.0, 200.0), it(1, 100.0, 200.0)], pad, pad), 600.0);
        assert_eq!(need(&[it(1, 100.0, 200.0), it(2, 100.0, 200.0)], pad, pad), 800.0);
    }

    #[test]
    fn own_net_end_needs_no_clearance() {
        let own = Bound::Pad { net: Some(1), clear: 200.0 };
        let edge = Bound::Edge { clear: 300.0 };
        // An own-net pass next to its pad: centre on the end vertex, half its width in the cut,
        // then the far end's clearance.
        assert_eq!(need(&[it(1, 100.0, 200.0)], own, edge), 350.0);
        assert_eq!(need(&[it(2, 100.0, 200.0)], own, edge), 600.0);
        // Own run (half width) + gap + the other net's pass + its clearance to the edge.
        assert_eq!(need(&[it(1, 100.0, 200.0), it(2, 100.0, 200.0)], own, edge), 50.0 + 200.0 + 100.0 + 300.0);
        assert_eq!(need(&[it(1, 900.0, 200.0)], own, edge), 750.0);
        assert_eq!(need(&[it(2, 100.0, 200.0)], Bound::Open, Bound::Open), 100.0);
    }

    #[test]
    fn octilinear_room_factor() {
        use crate::geom::Vec2;
        let v = |x: f64, y: f64| Vec2 { x, y };
        // A square crossing at 0 or 45 degrees: 1.
        assert!((octi_stretch(v(0.0, 1.0), v(1.0, 0.0), false) - 1.0).abs() < 1e-12);
        assert!((octi_stretch(v(-1.0, 1.0), v(1.0, 1.0), false) - 1.0).abs() < 1e-12);
        // A taut chord at 22.5 degrees across a vertical cut: the elbow's legs at 0 (factor 1) and
        // 45 degrees (sqrt 2) in equal shares; the any-angle factor would be 1 / cos 22.5.
        let f = octi_stretch(v(0.0, 1.0), v(1.0, (22.5f64).to_radians().tan()), false);
        assert!(f > 1.0 / (22.5f64).to_radians().cos() && f < 2f64.sqrt(), "{f}");
        // Around a wrapped corner: a cut at 22.5 degrees meets the octagon at its vertex.
        let g = octi_stretch(v(1.0, (22.5f64).to_radians().tan()), v(0.0, 1.0), true);
        assert!((g - 1.0 / (22.5f64).to_radians().cos()).abs() < 1e-9, "{g}");
        assert!((octi_stretch(v(1.0, 1.0), v(0.0, 1.0), true) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn pad_exit_between_fine_pitch_neighbours() {
        // M-C: a 0.25 mm wire leaving its pad through the 0.2 mm gap to a neighbour pad (clearance
        // 0.15 mm) needs 0.125 + 0.15 = 0.275 mm there: more than the gap (the old model: 0).
        let own = Bound::Pad { net: Some(1), clear: 150_000.0 };
        let nb = Bound::Pad { net: Some(2), clear: 150_000.0 };
        let n = need(&[it(1, 250_000.0, 150_000.0)], own, nb);
        assert_eq!(n, 275_000.0);
        assert!(n > 200_000.0);
        // Both ends of the wire's own net: it lies on copper all the way.
        assert_eq!(need(&[it(1, 250_000.0, 150_000.0)], own, own), 125_000.0);
    }
}
