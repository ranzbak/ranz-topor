//! Net trees that follow the routing (D5 branching).
//!
//! A connection of the decomposition joins two **components** of its net's routed copper, not
//! two fixed pads (Takahashi-Matsuyama tree growth, as in PathFinder, McMurchie & Ebeling 1995
//! §3.1, and BonnRoute, Ahrens et al. 2023 p. 4): the search starts at any copper of one component
//! and stops at the first copper of the other ([`State::search_tree`]). Copper is
//! * a pad centre of the component (the wire may end at another pad than the decomposition
//!   planned: the tree follows the routing, and a pad passed on the way is a junction);
//! * a junction on a routed wire of the component: a free or node vertex of a triangle the wire
//!   runs through, where the wire is made to turn (`Wire::turns`) and the new wire ends
//!   (`NO_OBS` end). This is the T-point of TopoR (6.3 manual p. 105: routing starts "from
//!   pads, polygons, vias, segments and T-points"); a branch point is a triangulation vertex
//!   (Luzin, Popov & Popov 2012, p. 118 fig. 6).
//!
//! The k-1 searches per net stay k-1 (one per decomposition connection), so no loop can form.
//! A junction couples the layers of the trunk and the branch ([`fix_layers`]: the tee rule of
//! Polubasov 2001 §5, two arms on one layer, a via on the third arm). When a trunk is ripped up
//! its branches go with it ([`dependents`]): their junction is gone.
//!
//! Deterministic: index-ordered vectors and B-tree maps only.

use super::init::TreeEnd;
use super::state::{ObsId, State, VertId, VertKind, WireId, NO_OBS};
use crate::geom::Vec2;
use std::collections::{BTreeMap, BTreeSet};

/// Union-find components of the net `net`'s routed copper: vertex -> component id (the
/// smallest vertex id in it). Vertices that no alive wire touches are absent (their own
/// component). O(wires of the net).
pub fn components(st: &State, net: i32) -> BTreeMap<VertId, VertId> {
    let mut par: BTreeMap<VertId, VertId> = BTreeMap::new();
    fn find(par: &mut BTreeMap<VertId, VertId>, x: VertId) -> VertId {
        let mut r = x;
        while let Some(&p) = par.get(&r) {
            if p == r {
                break;
            }
            r = p;
        }
        let mut y = x;
        while let Some(&p) = par.get(&y) {
            if p == r {
                break;
            }
            par.insert(y, r);
            y = p;
        }
        r
    }
    let union = |par: &mut BTreeMap<VertId, VertId>, a: VertId, b: VertId| {
        par.entry(a).or_insert(a);
        par.entry(b).or_insert(b);
        let (ra, rb) = (find(par, a), find(par, b));
        if ra != rb {
            par.insert(ra.max(rb), ra.min(rb));
        }
    };
    for wi in st.wires.iter().filter(|w| w.alive && w.net == net) {
        union(&mut par, wi.a, wi.b);
        for &(_, v) in &wi.turns {
            union(&mut par, wi.a, v);
        }
    }
    let keys: Vec<VertId> = par.keys().copied().collect();
    keys.into_iter().map(|v| (v, find(&mut par, v))).collect()
}

/// Component id of vertex `v` in `comp` ([`components`]).
pub fn comp_of(comp: &BTreeMap<VertId, VertId>, v: VertId) -> VertId {
    comp.get(&v).copied().unwrap_or(v)
}

/// What the ends of a tree search may be.
pub struct EndsCtx<'a> {
    /// Pad centre vertices of every net with their obstacles, ascending.
    pub pads: &'a BTreeMap<i32, Vec<(VertId, ObsId)>>,
    /// Free radius per vertex (nm; distance to the nearest obstacle of another net), computed
    /// for free and node vertices only (0 for the others).
    pub rad: &'a [f64],
    /// Junctions on wires allowed (`[search] net_tree = "free"`).
    pub junctions: bool,
    /// The `branch` reward (mm) and the `smd_through` cost (mm).
    pub reward: f64,
    pub smd_cost: f64,
    /// Per obstacle: a single-layer (SMD) pad.
    pub smd: &'a [bool],
    /// A branch point on a trace closer than this (nm) to a pad of the component is not used:
    /// the pad is the junction (`[search] junction_pad_mm`).
    pub pad_nm: f64,
}

/// The ends of component `c` of net `net` for a wire of width `w`, clearance `s`: its pad
/// centres, and (when junctions are allowed) the free / node corners of the triangles its wires
/// run through where the wire may turn ([`State::junction_ok`]) and the copper fits (free radius
/// at least w/2 + s). Vertices that already are a wire end or a turn are not junction candidates
/// (a junction has degree 3). O(passes of the component x traversals per triangle).
///
/// `planned` are the connection's own pads (no reward there). A pad of the component other than
/// those that already carries exactly one trace becomes a through-pad: reward, and the SMD
/// assembly cost on a single-layer pad.
#[allow(clippy::too_many_arguments)]
pub fn ends_of(st: &State, cx: &EndsCtx, net: i32, comp: &BTreeMap<VertId, VertId>, c: VertId, w: f64, s: f64, planned: [VertId; 2]) -> Vec<TreeEnd> {
    let mut out: Vec<TreeEnd> = Vec::new();
    let mut pad_pts = Vec::new();
    if let Some(ps) = cx.pads.get(&net) {
        for &(v, o) in ps {
            if comp_of(comp, v) == c {
                let through = cx.junctions && !planned.contains(&v) && st.ends[v] == 1;
                let adj = if through { -cx.reward + if cx.smd.get(o).copied().unwrap_or(false) { cx.smd_cost } else { 0.0 } } else { 0.0 };
                out.push(TreeEnd { v, obs: o, on: None, adj, arms: pad_arms(st, v) });
                pad_pts.push(st.proj.pos[v]);
            }
        }
    }
    if !cx.junctions {
        return out;
    }
    let mut used: BTreeSet<VertId> = BTreeSet::new();
    for wi in st.wires.iter().filter(|x| x.alive) {
        used.insert(wi.a);
        used.insert(wi.b);
        used.extend(wi.turns.iter().map(|t| t.1));
    }
    let need = 0.5 * w + s;
    for (wid, wi) in st.wires.iter().enumerate().filter(|(_, x)| x.alive && x.net == net) {
        if comp_of(comp, wi.a) != c {
            continue;
        }
        for (k, &t) in wi.tris.iter().enumerate() {
            for &v in &st.proj.tris[t] {
                if used.contains(&v) || !matches!(st.proj.vkind[v], VertKind::Free | VertKind::Node) || cx.rad[v] < need {
                    continue;
                }
                if pad_pts.iter().any(|&q| (q - st.proj.pos[v]).len() < cx.pad_nm) {
                    continue;
                }
                if st.junction_ok(wid, k, v) {
                    let pts = st.polyline(wid);
                    let p = st.proj.pos[v];
                    let arms = [unit(pts[k] - p), unit(pts[k + 1] - p), None, None];
                    out.push(TreeEnd { v, obs: NO_OBS, on: Some((wid, k)), adj: -cx.reward, arms });
                }
            }
        }
    }
    out
}

fn unit(d: Vec2) -> Option<Vec2> {
    let l = d.len();
    (l > 1.0).then(|| d * (1.0 / l))
}

/// Unit directions of the traces that already end at vertex `v` (towards their first pass
/// point), at most four. O(wires).
pub fn pad_arms(st: &State, v: VertId) -> [Option<Vec2>; 4] {
    let mut out = [None; 4];
    let mut n = 0;
    let p = st.proj.pos[v];
    for wi in st.wires.iter().filter(|w| w.alive) {
        for (end, q) in [(wi.a, 1usize), (wi.b, usize::MAX)] {
            if end != v || n >= 4 {
                continue;
            }
            let pts: Vec<Vec2> = std::iter::once(st.proj.pos[wi.a]).chain(wi.passes.iter().map(|&x| st.pass_point(x))).chain(std::iter::once(st.proj.pos[wi.b])).collect();
            let towards = if q == 1 { pts[1] } else { pts[pts.len() - 2] };
            if let Some(u) = unit(towards - p) {
                out[n] = Some(u);
                n += 1;
            }
        }
    }
    out
}

/// The trunk wire and traversal that turns at junction vertex `v`, if any. O(wires).
pub fn trunk_of(st: &State, v: VertId) -> Option<(WireId, usize)> {
    st.wires.iter().enumerate().filter(|(_, w)| w.alive).find_map(|(i, w)| w.turns.iter().find(|t| t.1 == v).map(|t| (i, t.0)))
}

/// The wires that lose their junction when the wires of `gone` are taken out (transitively: the
/// branches ending on a turn of a gone wire, then theirs), not including `gone` itself.
/// O(rounds x wires).
pub fn dependents(st: &State, gone: &BTreeSet<WireId>) -> BTreeSet<WireId> {
    let mut all = gone.clone();
    loop {
        let turns: BTreeSet<VertId> = all.iter().flat_map(|&w| st.wires[w].turns.iter().map(|t| t.1)).collect();
        let more: Vec<WireId> = st
            .wires
            .iter()
            .enumerate()
            .filter(|(i, w)| w.alive && !all.contains(i) && ((w.pa == NO_OBS && turns.contains(&w.a)) || (w.pb == NO_OBS && turns.contains(&w.b))))
            .map(|(i, _)| i)
            .collect();
        if more.is_empty() {
            break;
        }
        all.extend(more);
    }
    all.difference(gone).copied().collect()
}

/// Take wire `w` out of the state: its turns go with it, and a junction end of `w` drops the
/// turn from its trunk (the trunk runs straight again).
pub fn remove_wire(st: &mut State, w: WireId) {
    for end in [(st.wires[w].pa, st.wires[w].a), (st.wires[w].pb, st.wires[w].b)] {
        if end.0 == NO_OBS {
            if let Some((t, _)) = trunk_of(st, end.1) {
                if t != w {
                    st.wires[t].turns.retain(|x| x.1 != end.1);
                }
            }
        }
    }
    st.remove(w);
    st.wires[w].turns.clear();
}

/// The junctions of the alive wires: `(branch, end 0|1, trunk, traversal, vertex)`.
pub fn junctions(st: &State) -> Vec<(WireId, usize, WireId, usize, VertId)> {
    let mut turn_at: BTreeMap<VertId, (WireId, usize)> = BTreeMap::new();
    for (i, w) in st.wires.iter().enumerate().filter(|(_, w)| w.alive) {
        for &(k, v) in &w.turns {
            turn_at.insert(v, (i, k));
        }
    }
    let mut out = Vec::new();
    for (i, w) in st.wires.iter().enumerate().filter(|(_, w)| w.alive) {
        for (end, (p, v)) in [(w.pa, w.a), (w.pb, w.b)].into_iter().enumerate() {
            if p == NO_OBS {
                if let Some(&(t, k)) = turn_at.get(&v) {
                    out.push((i, end, t, k, v));
                }
            }
        }
    }
    out
}

/// Couple the layers at the junctions (Polubasov 2001 §5 tee rule): the trunk keeps one layer
/// through the junction (an event pins its label at the turn), and the branch ends on that layer
/// (an end event with the trunk's label; a branch that arrives on the other layer gets a via
/// interval next to the junction). `events` are P3's per-wire `(k, s, label)` lists, `mask` and
/// `partner` the matching per-event lists; `bottom` is the label of a wire without events.
/// Returns how many branch ends had to change layer. O(junctions x events).
pub fn fix_layers(
    st: &State,
    events: &mut [Vec<(usize, f64, i32)>],
    mask: &mut [Vec<u64>],
    partner: &mut [Vec<Option<(WireId, usize)>>],
    bottom: i32,
) -> usize {
    let label_at = |ev: &[(usize, f64, i32)], k: usize, s: f64| -> i32 {
        // The label of the last event at or before (k, s), else of the first event, else bottom.
        let mut l = ev.first().map_or(bottom, |e| e.2);
        for e in ev {
            if (e.0, e.1) <= (k, s) || e.0 < k {
                l = e.2;
            }
        }
        l
    };
    let mut changed = 0usize;
    let fix_partner = |partner: &mut [Vec<Option<(WireId, usize)>>], w: WireId, at: usize| {
        // Indices into w's events at or after `at` moved by one.
        for list in partner.iter_mut() {
            for x in list.iter_mut().flatten() {
                if x.0 == w && x.1 >= at {
                    x.1 += 1;
                }
            }
        }
    };
    for (b, end, t, k, _) in junctions(st) {
        let l = label_at(&events[t], k, 0.5);
        // Pin the trunk at the turn.
        let pos = events[t].iter().position(|e| (e.0, e.1) > (k, 0.5)).unwrap_or(events[t].len());
        if !(pos > 0 && events[t][pos - 1].0 == k && events[t][pos - 1].1 == 0.5) {
            fix_partner(partner, t, pos);
            events[t].insert(pos, (k, 0.5, l));
            mask[t].insert(pos, 1u64 << l);
            partner[t].insert(pos, None);
        }
        // The branch's end on the same layer.
        let n = st.wires[b].passes.len();
        if end == 0 {
            let first = events[b].first().map_or(bottom, |e| e.2);
            if events[b].is_empty() || first != l || (events[b][0].0, events[b][0].1) != (0, 0.0) {
                if first != l {
                    changed += 1;
                }
                fix_partner(partner, b, 0);
                events[b].insert(0, (0, 0.0, l));
                mask[b].insert(0, 1u64 << l);
                partner[b].insert(0, None);
            }
        } else {
            let last = events[b].last().map_or(bottom, |e| e.2);
            if events[b].is_empty() || last != l || (events[b][events[b].len() - 1].0, events[b][events[b].len() - 1].1) != (n, 1.0) {
                if last != l {
                    changed += 1;
                }
                events[b].push((n, 1.0, l));
                mask[b].push(1u64 << l);
                partner[b].push(None);
            }
        }
    }
    changed
}
