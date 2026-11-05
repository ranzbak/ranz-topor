//! Capacity of the cuts for N layers (plan "Capacity for N layers").
//!
//! Before P3 the layers of a pass are not known, only the layers its wire may use at the edge (the
//! allowed masks of the two triangles beside it). Hall's condition over the masks that actually
//! occur on an edge is then necessary: for every union `S` of those masks, the passes whose mask
//! lies inside `S` must fit in the rooms of the layers of `S` together
//! ([`need_lower`] is a lower bound of their demand). After P3 the exact per-layer check is the
//! drawing's packing (`Drawer::over`).
//!
//! Complexity: O(passes) per edge plus 2^k subsets of the k <= 10 distinct masks on it.

use super::demand::{need_lower, Item};
use super::state::State;

/// Result of the subset check.
#[derive(Clone, Debug, Default)]
pub struct SubsetReport {
    /// Edges with at least one pass.
    pub edges: usize,
    /// `(edge, layer set)` whose passes cannot fit however the layers are assigned.
    pub violated: Vec<(usize, u64)>,
}

/// Hall subset check of every edge of `st` over the allowed masks that occur.
pub fn subset_check(st: &State) -> SubsetReport {
    let pr = st.proj;
    let mut rep = SubsetReport::default();
    for e in 0..pr.edges.len() {
        if st.edge_order[e].is_empty() {
            continue;
        }
        rep.edges += 1;
        let [Some(t1), Some(t2)] = pr.edge_tris[e] else { continue };
        let mut passes: Vec<(u64, Item)> = Vec::new();
        for &p in &st.edge_order[e] {
            let wi = &st.wires[st.passes[p].wire];
            let (Some(m1), Some(m2)) = (pr.allowed(t1, wi.net), pr.allowed(t2, wi.net)) else { continue };
            passes.push((m1.0 & m2.0, Item { net: wi.net, w: wi.w, s: wi.s }));
        }
        let mut masks: Vec<u64> = passes.iter().map(|p| p.0).collect();
        masks.sort_unstable();
        masks.dedup();
        masks.truncate(10);
        let mut seen: Vec<u64> = Vec::new();
        for sub in 1u32..(1 << masks.len()) {
            let s = (0..masks.len()).filter(|&i| (sub >> i) & 1 == 1).fold(0u64, |a, i| a | masks[i]);
            if s == 0 || seen.contains(&s) {
                continue;
            }
            seen.push(s);
            let items: Vec<Item> = passes.iter().filter(|p| p.0 & !s == 0).map(|p| p.1).collect();
            let room: f64 = (0..pr.n_layers).filter(|&l| (s >> l) & 1 == 1).map(|l| pr.room_of(e, l)).sum();
            if room.is_finite() && need_lower(&items, s.count_ones() as usize) > room {
                rep.violated.push((e, s));
            }
        }
    }
    rep
}
