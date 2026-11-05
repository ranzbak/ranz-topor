//! Capacity-aware refinement of a P3 labelling (plan "Pipeline" P4, first half).
//!
//! P3 minimises vias only; the drawing then finds that some passages hold more copper on one
//! layer than fits. This module prices that: from the labelled events of every wire it derives
//! the layer of each pass (the same rule the drawing uses: a via sits inside its interval, the
//! passes before it are on the first layer), packs the passes of every edge per layer with the
//! drawing's own packer ([`Packer`]: end margins, neighbour spacing, oblique-chord factor) and
//! charges an over-full edge-layer `overflow_mm * (1 + over / pitch)` (mm of copper, the
//! `overflow` factor of the registry; `over` = how far the leftmost packing exceeds the
//! rightmost, `pitch` = mean track width + clearance on the edge). A local search then moves crossings: a *flip* swaps the layers of the two events of a
//! crossing, a *relabel* moves one event to another layer allowed there. A move is kept when
//! (layer changes priced by the caller + overflow price) drops. Deterministic: moves are tried
//! in edge, pass, event order.
//!
//! Complexity: building the model is O(passes); one move evaluation is O(passes of the edges it
//! touches); the search stops after `max_evals` evaluations.

use super::pack::{infl_of, PackItem, Packer};
use super::state::{State, WireId};
use crate::geom::Vec2;

const EPS: f64 = 1e-9;

/// One labelled event of a wire: `(traversal k, fraction s, layer)`.
pub type LEv = (usize, f64, i32);

/// Parameters of the refinement.
#[derive(Clone, Copy, Debug)]
pub struct RefineParams {
    /// Price (mm) of an over-full edge-layer; 0 switches the refinement off.
    pub overflow_mm: f64,
    /// Most move evaluations.
    pub max_evals: usize,
    /// A move may add layer changes (vias) when it removes more overflow than they cost; false =
    /// only moves that keep or lower the layer-change price (capacity for free).
    pub add_vias: bool,
}

/// What the refinement did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RefineStats {
    /// Over-full edge-layers (demand model) before and after.
    pub before: usize,
    pub after: usize,
    /// Moves kept.
    pub moves: usize,
}

/// Layer of every pass of a wire with `n` passes from its labelled events `ev`; `bottom` when it
/// has none. A via interval `(ka, kb)` puts the via at traversal `(ka + kb) / 2` rounded up (at
/// `ka` when both events are in one traversal), as the drawing does. O(events + n).
pub fn pass_layers(n: usize, ev: &[LEv], bottom: i32) -> Vec<i32> {
    let Some(&(_, _, first)) = ev.first() else { return vec![bottom; n] };
    let mut out = vec![first; n];
    let (mut start, mut cur) = (0usize, first);
    for x in ev.windows(2) {
        let ((ka, _, la), (kb, _, lb)) = (x[0], x[1]);
        if la == lb {
            continue;
        }
        let km = if kb > ka { (ka + kb).div_ceil(2) } else { ka };
        let km = km.min(n).max(start);
        out[start..km].fill(cur);
        start = km;
        cur = lb;
    }
    out[start..].fill(cur);
    out
}

/// Per edge-layer overflow price of a labelling, kept incrementally.
pub struct CapModel<'a> {
    st: &'a State<'a>,
    packer: &'a Packer<'a>,
    polys: &'a [Vec<Vec2>],
    nl: usize,
    price: f64,
    /// Position of every pass in its wire.
    pass_k: Vec<usize>,
    /// Layer of every pass (-1: wire dead).
    pub pass_layer: Vec<i32>,
    /// Price of edge `e`, layer `l` at `e * nl + l`.
    cost: Vec<f64>,
}

impl<'a> CapModel<'a> {
    /// The model of the labelling `evl` (events per wire). O(passes x margin lookups).
    pub fn new(st: &'a State<'a>, packer: &'a Packer<'a>, polys: &'a [Vec<Vec2>], evl: &[Vec<LEv>], bottom: i32, price: f64) -> Self {
        let nl = st.proj.n_layers;
        let mut pass_layer = vec![-1; st.passes.len()];
        let mut pass_k = vec![0; st.passes.len()];
        for (w, wi) in st.wires.iter().enumerate().filter(|(_, wi)| wi.alive) {
            let pl = pass_layers(wi.passes.len(), &evl[w], bottom);
            for (k, &p) in wi.passes.iter().enumerate() {
                pass_layer[p] = pl[k];
                pass_k[p] = k;
            }
        }
        let mut m = CapModel { st, packer, polys, nl, price, pass_k, pass_layer, cost: vec![0.0; st.proj.edges.len() * nl] };
        for e in 0..st.proj.edges.len() {
            for l in 0..nl {
                m.cost[e * nl + l] = m.eval(e, l);
            }
        }
        m
    }

    /// Price of edge `e` on layer `l` under the current pass layers.
    fn eval(&self, e: usize, l: usize) -> f64 {
        let pr = self.st.proj;
        let items: Vec<PackItem> = self.st.edge_order[e]
            .iter()
            .filter(|&&p| self.pass_layer[p] == l as i32)
            .map(|&p| {
                let w = self.st.passes[p].wire;
                let wi = &self.st.wires[w];
                PackItem { net: wi.net, w: wi.w, s: wi.s, infl: infl_of(pr, e, &self.polys[w], self.pass_k[p]) }
            })
            .collect();
        if items.is_empty() {
            return 0.0;
        }
        let over = self.packer.bounds(e, l, &items).worst();
        if over <= 1e-6 {
            return 0.0;
        }
        let pitch = (items.iter().map(|i| i.w + i.s).sum::<f64>() / items.len() as f64).max(1.0);
        self.price * (1.0 + over / pitch)
    }

    /// Whether the passes on edge `e`, layer `l` fit when the wires in `gone` are taken out.
    pub fn fits_without(&self, e: usize, l: usize, gone: &std::collections::BTreeSet<WireId>) -> bool {
        let pr = self.st.proj;
        let items: Vec<PackItem> = self.st.edge_order[e]
            .iter()
            .filter(|&&p| self.pass_layer[p] == l as i32 && !gone.contains(&self.st.passes[p].wire))
            .map(|&p| {
                let w = self.st.passes[p].wire;
                let wi = &self.st.wires[w];
                PackItem { net: wi.net, w: wi.w, s: wi.s, infl: infl_of(pr, e, &self.polys[w], self.pass_k[p]) }
            })
            .collect();
        items.is_empty() || self.packer.bounds(e, l, &items).fits()
    }

    /// Total overflow price of the labelling (mm).
    pub fn total(&self) -> f64 {
        self.cost.iter().sum()
    }

    /// Over-full edge-layers.
    pub fn overfull(&self) -> usize {
        self.cost.iter().filter(|&&c| c > 0.0).count()
    }

    /// The over-full `(edge, layer)` pairs, ascending.
    pub fn overfull_list(&self) -> Vec<(usize, usize)> {
        (0..self.cost.len()).filter(|&i| self.cost[i] > 0.0).map(|i| (i / self.nl, i % self.nl)).collect()
    }
}

/// Cost of the layer changes and per-event extras of wire `w`.
fn wire_cost(evl: &[Vec<LEv>], w: WireId, trans: &dyn Fn(usize, usize) -> f64, unary: &dyn Fn(WireId, usize, usize) -> f64) -> f64 {
    let l = &evl[w];
    let t: f64 = l.windows(2).map(|x| trans(x[0].2 as usize, x[1].2 as usize)).sum();
    t + l.iter().enumerate().map(|(i, e)| unary(w, i, e.2 as usize)).sum::<f64>()
}

/// Refine `evl` in place. `mask[w][i]` is the set of layers event `i` of wire `w` may take,
/// `partner[w][i]` the event of the other wire of its crossing (the two must differ in layer).
/// `trans(a, b)` prices a layer change, `unary(w, i, l)` the extra price of event `i` on `l`.
/// O(`max_evals` x passes of the touched edges).
#[allow(clippy::too_many_arguments)]
pub fn refine(
    st: &State,
    packer: &Packer,
    polys: &[Vec<Vec2>],
    evl: &mut [Vec<LEv>],
    mask: &[Vec<u64>],
    partner: &[Vec<Option<(WireId, usize)>>],
    bottom: i32,
    trans: &dyn Fn(usize, usize) -> f64,
    unary: &dyn Fn(WireId, usize, usize) -> f64,
    p: &RefineParams,
) -> RefineStats {
    // Switched off: the model is built with a unit price only to count the over-full edge-layers.
    let mut cm = CapModel::new(st, packer, polys, evl, bottom, if p.overflow_mm > 0.0 { p.overflow_mm } else { 1.0 });
    let mut stats = RefineStats { before: cm.overfull(), ..RefineStats::default() };
    if p.overflow_mm <= 0.0 {
        stats.after = stats.before;
        return stats;
    }
    let mut evals = 0usize;
    for _sweep in 0..8 {
        let mut improved = false;
        for (e, l) in cm.overfull_list() {
            if cm.cost[e * cm.nl + l] <= 0.0 {
                continue;
            }
            let passes: Vec<usize> = st.edge_order[e].iter().copied().filter(|&q| cm.pass_layer[q] == l as i32).collect();
            for pid in passes {
                if evals >= p.max_evals || cm.cost[e * cm.nl + l] <= 0.0 {
                    break;
                }
                let w = st.passes[pid].wire;
                let k = st.wires[w].passes.iter().position(|&x| x == pid).expect("pass of its wire");
                // The events around the pass (position (k, 1.0)): two before, two after.
                let j1 = evl[w].iter().take_while(|x| x.0 < k || (x.0 == k && x.1 <= 1.0)).count();
                let lo = j1.saturating_sub(2);
                let hi = (j1 + 2).min(evl[w].len());
                for i in lo..hi {
                    let cur = evl[w][i].2;
                    let mut moves: Vec<Vec<((WireId, usize), i32)>> = Vec::new();
                    if let Some((w2, i2)) = partner[w][i] {
                        let c2 = evl[w2][i2].2;
                        if c2 != cur && (mask[w][i] >> c2) & 1 == 1 && (mask[w2][i2] >> cur) & 1 == 1 {
                            moves.push(vec![((w, i), c2), ((w2, i2), cur)]);
                        }
                    }
                    for nl in 0..cm.nl as i32 {
                        if nl == cur || (mask[w][i] >> nl) & 1 == 0 {
                            continue;
                        }
                        if partner[w][i].is_some_and(|(w2, i2)| evl[w2][i2].2 == nl) {
                            continue;
                        }
                        moves.push(vec![((w, i), nl)]);
                    }
                    for mv in moves {
                        if evals >= p.max_evals {
                            break;
                        }
                        evals += 1;
                        if try_move(st, &mut cm, evl, &mv, bottom, trans, unary, p.add_vias, false) {
                            stats.moves += 1;
                            improved = true;
                            break;
                        }
                    }
                }
            }
        }
        if !improved || evals >= p.max_evals {
            break;
        }
    }
    stats.after = cm.overfull();
    stats
}

/// Apply `mv` (label changes) and keep it when the total price drops, else undo. `strict`: also
/// refuse it when any touched edge-layer gets more over-full (the drawing's capacity test; the P4
/// label moves). O(touched edges).
#[allow(clippy::too_many_arguments)]
pub fn try_move(
    st: &State,
    cm: &mut CapModel,
    evl: &mut [Vec<LEv>],
    mv: &[((WireId, usize), i32)],
    bottom: i32,
    trans: &dyn Fn(usize, usize) -> f64,
    unary: &dyn Fn(WireId, usize, usize) -> f64,
    add_vias: bool,
    strict: bool,
) -> bool {
    let mut wires: Vec<WireId> = mv.iter().map(|x| x.0 .0).collect();
    wires.sort_unstable();
    wires.dedup();
    let old_wc: f64 = wires.iter().map(|&w| wire_cost(evl, w, trans, unary)).sum();
    let old_lab: Vec<i32> = mv.iter().map(|&((w, i), _)| evl[w][i].2).collect();
    for &((w, i), l) in mv {
        evl[w][i].2 = l;
    }
    let new_wc: f64 = wires.iter().map(|&w| wire_cost(evl, w, trans, unary)).sum();
    // Pass layers that change.
    let mut changed: Vec<(usize, i32, i32)> = Vec::new();
    for &w in &wires {
        let pl = pass_layers(st.wires[w].passes.len(), &evl[w], bottom);
        for (k, &p) in st.wires[w].passes.iter().enumerate() {
            if cm.pass_layer[p] != pl[k] {
                changed.push((p, cm.pass_layer[p], pl[k]));
            }
        }
    }
    let mut touched: Vec<(usize, usize)> = Vec::new();
    for &(p, a, b) in &changed {
        let e = st.passes[p].edge;
        for l in [a, b] {
            if l >= 0 {
                touched.push((e, l as usize));
            }
        }
    }
    touched.sort_unstable();
    touched.dedup();
    let nl = cm.nl;
    let old_oc: f64 = touched.iter().map(|&(e, l)| cm.cost[e * nl + l]).sum();
    for &(p, _, b) in &changed {
        cm.pass_layer[p] = b;
    }
    let new: Vec<f64> = touched.iter().map(|&(e, l)| cm.eval(e, l)).collect();
    let new_oc: f64 = new.iter().sum();
    let worse_somewhere = strict && touched.iter().zip(&new).any(|(&(e, l), &c)| c > cm.cost[e * nl + l] + EPS);
    if (new_wc - old_wc) + (new_oc - old_oc) < -EPS && (add_vias || new_wc <= old_wc + EPS) && !worse_somewhere {
        for (&(e, l), &c) in touched.iter().zip(&new) {
            cm.cost[e * nl + l] = c;
        }
        return true;
    }
    for &(p, a, _) in &changed {
        cm.pass_layer[p] = a;
    }
    for (&((w, i), _), &l) in mv.iter().zip(&old_lab) {
        evl[w][i].2 = l;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_layers_put_the_via_in_the_middle_of_its_interval() {
        // Events at traversal 0 (layer 0) and traversal 4 (layer 1): via at traversal 2.
        let ev = [(0, 0.5, 0), (4, 0.5, 1)];
        assert_eq!(pass_layers(6, &ev, 1), vec![0, 0, 1, 1, 1, 1]);
        // Same traversal: the via sits inside it, passes 0..k stay on the first layer.
        let ev = [(2, 0.2, 1), (2, 0.8, 0)];
        assert_eq!(pass_layers(5, &ev, 0), vec![1, 1, 0, 0, 0]);
        assert_eq!(pass_layers(3, &[], 1), vec![1, 1, 1]);
    }
}
