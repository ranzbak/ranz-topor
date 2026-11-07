//! Crossing rules (M-B2): the topological patterns that make a plan need more room or vias than
//! the drawing can give, found on the topology state where changing them is still cheap.
//!
//! - **Clinch at a terminal** (Luzin, Popov & Popov 2012, Научно-техн. вестник ИТМО no. 4,
//!   pp. 116–117, sign 1, Fig. 3; Luzin, Popov & Popov 2013, Электроника НТБ no. 1, p. 102,
//!   Figs. 16 and 18): a wire of another net passes next to a wire's terminal pad, between the
//!   pad and where the wire goes (it wraps around the terminal, a half-loop around a terminal),
//!   so the terminal's wire must cross it (a via next to the pad, or a crossing that cannot be
//!   layered) or go around it (the detour of Fig. 3a).
//!   Removal (2012, p. 118, «Устранение клинчей» 1–2): lay the interfering wire on the other side
//!   of the terminal and connect the terminal's wire directly, i.e. route the terminal's wire
//!   first and the interfering one after it.
//! - **Double crossing** (half-loop; 2013, p. 102, Fig. 16 "projections of wires of different
//!   nets intersect more than once"; Polubasov 2010, Электроника НТБ 6/2010, «Начальная
//!   трассировка», Figs. 1–2): two wires of different nets crossing twice. Where their stretches
//!   between the crossings share the same edges, [`super::state::State::uncross_double`] swaps
//!   them; else (an obstacle between them) both are routed again (after removing a crossing,
//!   "all wires involved are rerouted", Polubasov 2010).
//! - **Crossing that cannot be layered** (Polubasov 2001, Технология и конструирование в
//!   электронной аппаратуре no. 2, p. 3: a crossing is a domain with two orientations; both
//!   stretches held to one layer leave none): the pair is routed again in the other order.
//!
//! The detection here only reads the state; the caller re-plans a found pair with
//! [`super::moves::Topo::try_joint`] and keeps it when the pair's price (the social factors)
//! drops, then judges the whole pass on the board objective (P3/P4 score).

use super::state::{State, WireId, NO_OBS};
use std::collections::{BTreeMap, BTreeSet};

/// A pattern found: the two wires, and which of them the rule routes first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pair {
    /// Routed first (the terminal's wire of a clinch).
    pub first: WireId,
    /// Routed second (the interfering wire).
    pub second: WireId,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Another net's wire between a terminal and its wire on an edge from the terminal.
    TerminalClinch,
    /// Two wires of different nets crossing more than once.
    DoubleCrossing,
    /// A crossing whose two stretches are held to the same layer.
    Unlayerable,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::TerminalClinch => "clinch at a terminal",
            Kind::DoubleCrossing => "double crossing",
            Kind::Unlayerable => "crossing that cannot be layered",
        }
    }
}

/// Clinches at terminals (sign 1, generalised to the detour form of Fig. 3a): a wire `w2` of
/// another net passes through the ring of a terminal pad of `w1` (a triangle touching the pad,
/// not ending there) and stands in `w1`'s way: it crosses `w1`, or its path cuts `w1`'s straight
/// pad-to-pad line (so `w1` crosses it or goes around it). One pair per (terminal wire,
/// interfering wire). O(ring traversals x path).
pub fn terminal_clinches(st: &State) -> Vec<Pair> {
    let mut out: BTreeSet<Pair> = BTreeSet::new();
    let polys: BTreeMap<WireId, Vec<super::super::geom::Vec2>> = BTreeMap::new();
    let mut polys = polys;
    for (w, wi) in st.wires.iter().enumerate().filter(|(_, wi)| wi.alive) {
        let (ca, cb) = (st.proj.pos[wi.a], st.proj.pos[wi.b]);
        let mut crosses: Option<BTreeSet<WireId>> = None;
        for pad in [wi.pa, wi.pb] {
            if pad == NO_OBS || pad >= st.ring.len() {
                continue;
            }
            for &t in &st.ring[pad] {
                for &(o, _) in &st.tri_trav[t] {
                    let oi = &st.wires[o];
                    if o == w || oi.net == wi.net || oi.pa == pad || oi.pb == pad {
                        continue;
                    }
                    let pair = Pair { first: w, second: o, kind: Kind::TerminalClinch };
                    if out.contains(&pair) {
                        continue;
                    }
                    let xs = crosses.get_or_insert_with(|| st.crossings(w).into_iter().map(|x| x.other).collect());
                    let pl = polys.entry(o).or_insert_with(|| st.polyline(o));
                    if xs.contains(&o) || pl.windows(2).any(|s| seg_cross(s[0], s[1], ca, cb)) {
                        out.insert(pair);
                    }
                }
            }
        }
    }
    out.into_iter().collect()
}

/// Whether the segments `a`-`b` and `c`-`d` cross properly.
fn seg_cross(a: super::super::geom::Vec2, b: super::super::geom::Vec2, c: super::super::geom::Vec2, d: super::super::geom::Vec2) -> bool {
    let o = |p: super::super::geom::Vec2, q: super::super::geom::Vec2, r: super::super::geom::Vec2| (q - p).cross(r - p);
    let (d1, d2, d3, d4) = (o(c, d, a), o(c, d, b), o(a, b, c), o(a, b, d));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// Different-net pairs crossing more than once (half-loops); the shorter wire is routed first.
pub fn double_crossings(st: &State) -> Vec<Pair> {
    let mut n: BTreeMap<(WireId, WireId), usize> = BTreeMap::new();
    for (w, wi) in st.wires.iter().enumerate().filter(|(_, wi)| wi.alive) {
        for x in st.crossings(w) {
            if x.other > w && st.wires[x.other].net != wi.net {
                *n.entry((w, x.other)).or_default() += 1;
            }
        }
    }
    let len = |w: WireId| st.polyline(w).windows(2).map(|x| (x[1] - x[0]).len()).sum::<f64>();
    n.into_iter()
        .filter(|&(_, c)| c >= 2)
        .map(|((a, b), _)| {
            let (first, second) = if len(a) <= len(b) { (a, b) } else { (b, a) };
            Pair { first, second, kind: Kind::DoubleCrossing }
        })
        .collect()
}
