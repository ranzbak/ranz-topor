//! Via reduction after drawing (plan P6 "after drawing: remove redundant vias, re-optimize"; the
//! drawn counterpart of the local via minimization of Luzin, Popov and Popov 2013).
//!
//! A via is redundant when the stretch between it and its neighbour via (or the pad) can be drawn
//! on the other layer without breaking a rule. Per wire with vias, each layer run is tried on the
//! layer of an adjacent run (the run with the largest gain first, then the shortest; the runs that
//! touch an SMD pad stay on the pad's layer, plane layers are not touched): the vias between runs
//! that now share a layer are dropped, the passes get the new layers (and their edge-layers are
//! packed again), and the wire is drawn again, in the same sleeve, by [`Drawer::draw_wire`]. The
//! checker decides: a run that would cross copper of its new layer is rejected there, and then
//! the old copper and vias are put back. A wire is tried at most [`TRIES_PER_WIRE`] times per
//! sweep, sweeps repeat while one removed a via ([`MAX_SWEEPS`]), the clock is polled between wires.
//!
//! Complexity: one wire drawing per try (O(passes) funnel plus checker queries); at most
//! `TRIES_PER_WIRE x MAX_SWEEPS x wires` drawings. Deterministic (wire id order).

use super::draw::{via_item, Drawer, Plan, WireOut};
use super::state::WireId;
use crate::geom::Vec2;
use crate::model::Item;
use crate::vias::ViaSize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// Most layer-run relabels tried on one wire in one sweep.
pub const TRIES_PER_WIRE: usize = 6;
/// Most sweeps over all wires.
pub const MAX_SWEEPS: usize = 3;

/// What the reduction did.
#[derive(Clone, Debug, Default)]
pub struct ReduceStats {
    pub wires_with_vias: usize,
    pub tried: usize,
    pub accepted: usize,
    pub vias_removed: usize,
    pub sweeps: usize,
    pub timed_out: bool,
}

type Placed = Vec<(usize, Vec2, ViaSize)>;

/// Reduce the vias of the drawn wires. `end_fixed(w)` tells for the two end pads of wire `w`
/// whether the pad pins the end run's layer (an SMD pad). `via_h` holds the handles of the vias
/// of every wire (parallel to `placed`). A `frozen` wire keeps its layers: its net has a pour, which
/// the wire may be what connects to (a track over the pour joins it; on another layer it would not). Returns the statistics and the wires whose drawing changed.
#[allow(clippy::too_many_arguments)]
pub fn reduce_vias(
    dr: &mut Drawer,
    drawn: &mut BTreeMap<WireId, WireOut>,
    placed: &mut BTreeMap<WireId, Placed>,
    plans: &mut BTreeMap<WireId, Plan>,
    via_h: &mut BTreeMap<WireId, Vec<usize>>,
    end_fixed: &dyn Fn(WireId) -> [bool; 2],
    frozen: &dyn Fn(WireId) -> bool,
    n_signal: usize,
    deadline: Option<Instant>,
    stop: &dyn Fn() -> bool,
) -> (ReduceStats, BTreeSet<WireId>) {
    let mut stats = ReduceStats::default();
    let mut changed: BTreeSet<WireId> = BTreeSet::new();
    let (diag_n, st_stats, over) = (dr.diag.len(), dr.stats, dr.over.clone());
    dr.quiet = true;
    stats.wires_with_vias = placed.iter().filter(|(w, v)| !v.is_empty() && drawn.contains_key(w) && !frozen(**w)).count();
    'sweeps: for _ in 0..MAX_SWEEPS {
        stats.sweeps += 1;
        let mut progress = false;
        let ws: Vec<WireId> = placed.iter().filter(|(w, v)| !v.is_empty() && drawn.contains_key(w) && !frozen(**w)).map(|(&w, _)| w).collect();
        for w in ws {
            // The time limit or (cancel-fix) a Cancel: every relabel accepted so far stays (anytime).
            if deadline.is_some_and(|d| Instant::now() >= d) || stop() {
                stats.timed_out = true;
                break 'sweeps;
            }
            let mut tries = 0;
            let mut skip: BTreeSet<Vec<usize>> = BTreeSet::new();
            while tries < TRIES_PER_WIRE {
                let plan = plans[&w].clone();
                let pl = placed[&w].clone();
                if pl.is_empty() || plan.layers.iter().any(|&l| n_signal > 0 && l >= n_signal) {
                    break;
                }
                let fixed = end_fixed(w);
                let n = plan.layers.len();
                // Candidate relabels of one run onto the layer of a neighbour.
                let mut cands: Vec<(usize, usize, Vec<usize>)> = Vec::new(); // (-gain, span, new layers)
                for r in 0..n {
                    if (r == 0 && fixed[0]) || (r == n - 1 && fixed[1]) {
                        continue;
                    }
                    let k0 = if r == 0 { 0 } else { pl[r - 1].0 };
                    let k1 = if r == n - 1 { usize::MAX } else { pl[r].0 };
                    let span = k1.saturating_sub(k0);
                    let mut targets = Vec::new();
                    if r > 0 {
                        targets.push(plan.layers[r - 1]);
                    }
                    if r + 1 < n && !targets.contains(&plan.layers[r + 1]) {
                        targets.push(plan.layers[r + 1]);
                    }
                    for t in targets {
                        let mut nl = plan.layers.clone();
                        nl[r] = t;
                        if skip.contains(&nl) {
                            continue;
                        }
                        let gain = (0..n - 1).filter(|&i| nl[i] == nl[i + 1]).count();
                        cands.push((usize::MAX - gain, span, nl));
                    }
                }
                // Every run on one layer (a chain A-B-A-B-A in one go), when the pads allow it.
                if n > 2 {
                    let mut ts: Vec<usize> = plan.layers.clone();
                    ts.sort_unstable();
                    ts.dedup();
                    for t in ts {
                        let ok_ends = (!fixed[0] || plan.layers[0] == t) && (!fixed[1] || plan.layers[n - 1] == t);
                        let nl = vec![t; n];
                        if ok_ends && !skip.contains(&nl) {
                            let span = pl.last().map_or(0, |v| v.0).saturating_sub(pl[0].0);
                            cands.push((usize::MAX - (n - 1), span, nl));
                        }
                    }
                }
                cands.sort();
                let Some((_, _, nl)) = cands.into_iter().next() else { break };
                tries += 1;
                skip.insert(nl.clone());
                stats.tried += 1;
                // New plan: runs on one layer are joined, the via between them goes.
                let mut layers = vec![nl[0]];
                let mut keep: Vec<usize> = Vec::new();
                for i in 0..n - 1 {
                    if nl[i] != nl[i + 1] {
                        keep.push(i);
                        layers.push(nl[i + 1]);
                    }
                }
                let new_placed: Placed = keep.iter().map(|&i| pl[i]).collect();
                let new_plan = Plan { layers: layers.clone(), vias: keep.iter().map(|&i| plan.vias[i]).collect(), at: keep.iter().filter_map(|&i| plan.at.get(i).copied()).collect() };
                let cuts: Vec<usize> = new_placed.iter().map(|v| v.0).collect();
                let old_h = via_h[&w].clone();
                let old_wo = drawn[&w].clone();
                // Take the old copper out: the tracks and the vias that go.
                for &h in &old_wo.handles {
                    dr.uncommit(h);
                }
                for (i, &h) in old_h.iter().enumerate() {
                    if !keep.contains(&i) {
                        dr.uncommit(h);
                    }
                }
                let old_layers = dr.relayer(w, &layers, &cuts);
                let mut ids = Vec::new();
                let wo = dr.draw_wire(w, &new_plan, &new_placed, &mut ids);
                if wo.ok {
                    let new_h: Vec<usize> = keep.iter().map(|&i| old_h[i]).collect();
                    stats.accepted += 1;
                    stats.vias_removed += pl.len() - new_placed.len();
                    via_h.insert(w, new_h);
                    placed.insert(w, new_placed);
                    plans.insert(w, new_plan);
                    drawn.insert(w, wo);
                    changed.insert(w);
                    progress = true;
                } else {
                    // Put back: pass layers, the vias that went, the tracks.
                    dr.restore_layers(w, &old_layers);
                    let net = dr.st.wires[w].net;
                    let mut h2 = old_h.clone();
                    for (i, h) in h2.iter_mut().enumerate() {
                        if !keep.contains(&i) {
                            *h = dr.commit(&via_item(net, pl[i].1, pl[i].2, Some(w)));
                        }
                    }
                    let mut hs = Vec::new();
                    for it in old_wo.items.iter().filter(|i| matches!(i, Item::Track(_))) {
                        hs.push(dr.commit(it));
                    }
                    via_h.insert(w, h2);
                    if let Some(x) = drawn.get_mut(&w) {
                        x.handles = hs;
                    }
                }
            }
        }
        if !progress {
            break;
        }
    }
    dr.quiet = false;
    dr.diag.truncate(diag_n);
    dr.stats = st_stats;
    dr.over = over;
    (stats, changed)
}
