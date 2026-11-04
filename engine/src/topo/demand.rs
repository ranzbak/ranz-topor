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
pub fn need(items: &[Item], lo: Bound, hi: Bound) -> f64 {
    // Passes of the end pad's own net next to the end sit on the pad.
    let own = |b: &Bound, it: &Item| matches!(*b, Bound::Pad { net: Some(n), .. } if n == it.net);
    let skip_lo = items.iter().take_while(|x| own(&lo, x)).count();
    let items = &items[skip_lo..];
    let skip_hi = items.iter().rev().take_while(|x| own(&hi, x)).count();
    let items = &items[..items.len() - skip_hi];
    let Some((first, last)) = items.first().zip(items.last()) else {
        return 0.0;
    };
    let mut n: f64 = items.iter().map(|x| x.w).sum();
    for p in items.windows(2) {
        if p[0].net != p[1].net {
            n += p[0].s.max(p[1].s);
        }
    }
    n + lo.clearance(first) + hi.clearance(last)
}

/// [`need`] where a custom courtyard rule replaces the clearances (`regime`, nm): between passes
/// and to pads alike. The board-edge and hole clearances are separate rules and stay.
pub fn need_in(regime: Option<f64>, items: &[Item], lo: Bound, hi: Bound) -> f64 {
    let Some(r) = regime else { return need(items, lo, hi) };
    let rep = |b: Bound| match b {
        Bound::Pad { net, .. } => Bound::Pad { net, clear: r },
        o => o,
    };
    let its: Vec<Item> = items.iter().map(|x| Item { s: r, ..*x }).collect();
    need(&its, rep(lo), rep(hi))
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
        assert_eq!(need(&[it(1, 100.0, 200.0)], own, edge), 0.0);
        assert_eq!(need(&[it(2, 100.0, 200.0)], own, edge), 600.0);
        // The own-net pass next to the pad lies on it: only the other net's pass is left.
        assert_eq!(need(&[it(1, 100.0, 200.0), it(2, 100.0, 200.0)], own, edge), 600.0);
        assert_eq!(need(&[it(1, 900.0, 200.0)], own, edge), 0.0);
        assert_eq!(need(&[it(2, 100.0, 200.0)], Bound::Open, Bound::Open), 100.0);
    }
}
