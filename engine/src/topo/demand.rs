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
    let own = |b: &Bound, it: &Item| matches!(*b, Bound::Pad { net: Some(n), .. } if n == it.net);
    // The run of own-net passes at an end as one item of half the widest width.
    let half = |run: &[Item]| -> Option<Item> {
        let f = run.first()?;
        Some(Item { net: f.net, w: 0.5 * run.iter().map(|x| x.w).fold(0.0, f64::max), s: run.iter().map(|x| x.s).fold(0.0, f64::max) })
    };
    let skip_lo = items.iter().take_while(|x| own(&lo, x)).count();
    let run_lo = half(&items[..skip_lo]);
    let items = &items[skip_lo..];
    let skip_hi = items.iter().rev().take_while(|x| own(&hi, x)).count();
    let run_hi = half(&items[items.len() - skip_hi..]);
    let items = &items[..items.len() - skip_hi];
    let all: Vec<Item> = run_lo.into_iter().chain(items.iter().copied()).chain(run_hi).collect();
    let Some((first, last)) = all.first().zip(all.last()) else {
        return 0.0;
    };
    let mut n: f64 = all.iter().map(|x| x.w).sum();
    for p in all.windows(2) {
        if p[0].net != p[1].net {
            n += p[0].s.max(p[1].s);
        }
    }
    // An own run at an end needs no clearance to it (its copper may touch the pad).
    let lo_c = if run_lo.is_some() { 0.0 } else { lo.clearance(first) };
    let hi_c = if run_hi.is_some() { 0.0 } else { hi.clearance(last) };
    n + lo_c + hi_c
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
