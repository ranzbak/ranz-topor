//! The P4 descent (plan "Pipeline" P4): sweeps of the [`moves`](super::moves) over the topology
//! state, then over the layer labelling, each move kept only when it lowers the registry price.
//!
//! * [`descend_topo`]: the wires with the most to gain first (their registry price where they lie
//!   minus the price of their straight length; below [`MIN_GAIN_MM`] a wire is not tried); per wire
//!   the first move that pays is kept: reroute, attachment point, merge with a parallel stretch
//!   of its net, channel swap of a crossing, joint move with a crossing partner. After every
//!   sweep the double crossings are swapped away ([`State::uncross_all`]). Every sweep prices
//!   with fluctuating coefficients (plan "coefficients fluctuate"): the factor weights times
//!   `1 + fluctuation x u`, `u` uniform in [-1, 1] from a ChaCha8 stream seeded by `seed` and the
//!   sweep, so the search leaves local minima and stays reproducible.
//! * [`descend_labels`]: crossing flips, via shifts and segment lifting on every wire with a layer
//!   change, until a sweep keeps nothing.
//!
//! Work caps, never time: `max_evals` move evaluations and `max_ripped` wires routed again.
//!
//! Complexity: a sweep prices every wire once (O(Σ path x traversals)) plus one evaluation per
//! candidate; a reroute evaluation is one capped search.

use super::factors::Registry;
use super::moves::{Kind, Labels, Tally, Topo};
use super::state::{State, WireId};

/// Most sweeps of each descent.
pub const MAX_SWEEPS: usize = 3;
/// Alternative pad pairs an attachment move tries.
pub const ATTACH_PAIRS: usize = 3;
/// A wire whose price is within this of its straight length's price (mm) is not tried.
pub const MIN_GAIN_MM: f64 = 1.0;
/// Crossing partners a joint move tries per wire.
pub const JOINT_PARTNERS: usize = 2;

/// Topological descent on `st` (see the module doc). `base` is the unperturbed registry.
pub fn descend_topo(st: &mut State, t: &mut Topo, base: &Registry, seed: u64, fluctuation: f64) -> Tally {
    let start = t.tally;
    for sweep in 0..MAX_SWEEPS {
        t.reg = base.fluctuated(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(sweep as u64 + 1), fluctuation);
        let applied0: usize = t.tally.applied.iter().sum();
        let ids: Vec<WireId> = (0..st.wires.len()).filter(|&w| st.wires[w].alive && t.wire_conn.get(w).is_some_and(|&c| c != usize::MAX)).collect();
        // Only wires that could gain more than the least move is worth, the most first.
        let mut ws: Vec<(f64, WireId)> = ids.into_iter().map(|w| (t.potential(st, w), w)).filter(|x| x.0 > MIN_GAIN_MM).collect();
        ws.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        for (_, w) in ws {
            if t.evals >= t.max_evals {
                break;
            }
            if !st.wires[w].alive {
                continue;
            }
            if t.try_reroute(st, w).is_some() || t.try_attach(st, w, ATTACH_PAIRS).is_some() || t.try_merge(st, w) || t.try_channel_swap(st, w) {
                continue;
            }
            let mut partners: Vec<WireId> = st.crossings(w).iter().map(|x| x.other).filter(|&o| st.wires[o].net != st.wires[w].net).collect();
            partners.dedup();
            partners.truncate(JOINT_PARTNERS);
            for o in partners {
                if st.wires[o].alive && st.wires[w].alive && t.wire_conn.get(o).is_some_and(|&c| c != usize::MAX) && t.try_joint(st, w, o).is_some() {
                    break;
                }
            }
        }
        t.tally.tried[Kind::DoubleSwap as usize] += 1;
        let swaps = st.uncross_all(50);
        t.tally.applied[Kind::DoubleSwap as usize] += swaps;
        let applied: usize = t.tally.applied.iter().sum();
        if applied == applied0 || t.evals >= t.max_evals {
            break;
        }
    }
    t.reg = base.clone();
    let mut d = t.tally;
    for i in 0..9 {
        d.tried[i] -= start.tried[i];
        d.applied[i] -= start.applied[i];
    }
    d
}

/// Label descent: flips, via shifts and lifts on every wire with a layer change, until a sweep
/// keeps nothing or the evaluation cap is reached.
pub fn descend_labels(lb: &mut Labels) -> Tally {
    for _ in 0..MAX_SWEEPS {
        let mut kept = 0;
        for w in lb.candidates() {
            if lb.evals >= lb.max_evals {
                break;
            }
            kept += lb.flips(w) + lb.via_shifts(w) + lb.lifts(w);
        }
        if kept == 0 || lb.evals >= lb.max_evals {
            break;
        }
    }
    lb.tally
}
