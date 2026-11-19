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
    /// TW7 (sign 2, U-clinch): another net's wire crosses the straight line between a wire's
    /// terminals and not the wire itself, so the wire goes round that wire's end.
    UClinch,
    /// TW7 (sign 4, mutual evasion): two wires each going round the other's end.
    MutualEvasion,
}

impl Kind {
    /// Short name (`[optimize] pattern_moves` entries, debug dump).
    pub fn name(self) -> &'static str {
        match self {
            Kind::TerminalClinch => "clinch",
            Kind::DoubleCrossing => "double",
            Kind::Unlayerable => "unlayerable",
            Kind::UClinch => "u_clinch",
            Kind::MutualEvasion => "evasion",
        }
    }
    /// Every kind, in [`Kind::idx`] order.
    pub const ALL: [Kind; 5] = [Kind::TerminalClinch, Kind::DoubleCrossing, Kind::Unlayerable, Kind::UClinch, Kind::MutualEvasion];
    /// The kind of a `[optimize] pattern_moves` entry.
    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == s)
    }
    /// The kind's index in [`Kind::ALL`] (the per-kind counters).
    pub fn idx(self) -> usize {
        self as usize
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

type V2 = super::super::geom::Vec2;
/// Axis-aligned box `(x0, y0, x1, y1)`.
type Bb = (f64, f64, f64, f64);

/// Length of a polyline (nm).
pub fn poly_len(pl: &[V2]) -> f64 {
    pl.windows(2).map(|x| (x[1] - x[0]).len()).sum()
}

/// TW7: the graded detour amount of wire `w` (Luzin 2015 p. 7: loops, half-loops and clinches
/// "lead to excess length"): `(r - 1)^2` with `r` = its projected length over the straight
/// distance of its two ends (0 for a straight wire or ends that coincide). No threshold: a wire
/// 10 % long has 0.01, one twice as long 1. The length is the loop's ([`State::wire_len`]: taut
/// with `[search] length = "taut"`). O(path).
pub fn detour_amount(st: &State, w: WireId) -> f64 {
    let wi = &st.wires[w];
    let d = (st.proj.pos[wi.b] - st.proj.pos[wi.a]).len();
    if d <= 0.0 {
        return 0.0;
    }
    let r = st.wire_len(w) / d;
    (r - 1.0).max(0.0).powi(2)
}

/// Axis-aligned box of a polyline.
fn bbox(pl: &[V2]) -> Bb {
    pl.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |b, p| (b.0.min(p.x), b.1.min(p.y), b.2.max(p.x), b.3.max(p.y)))
}

fn boxes_meet(a: Bb, b: Bb) -> bool {
    a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
}

/// How often the polyline `pl` passes from one side of the segment `a`-`b` to the other within
/// it, half-open (a point on the line counts as the negative side, so a pass point on a
/// triangulation edge between the two terminals, which the projection has, counts once, not
/// zero or two times; a touch counts an even number, so the parity is exact). O(segments).
pub fn line_crossings(pl: &[V2], a: V2, b: V2) -> usize {
    let d = b - a;
    let dd = d.x * d.x + d.y * d.y;
    if dd <= 0.0 {
        return 0;
    }
    let side = |p: V2| d.cross(p - a);
    pl.windows(2)
        .filter(|s| {
            let (sp, sq) = (side(s[0]), side(s[1]));
            if (sp > 0.0) == (sq > 0.0) {
                return false;
            }
            let x = s[0] + (s[1] - s[0]) * (sp / (sp - sq));
            let t = ((x.x - a.x) * d.x + (x.y - a.y) * d.y) / dd;
            (0.0..=1.0).contains(&t)
        })
        .count()
}

/// U-clinches (Luzin, Popov & Popov 2012 p. 117 sign 2, Fig. 4: "an edge between equipotential
/// objects is crossed by wires of other nets while their own wire does not run along it"; Luzin
/// 2015 p. 8 b, fig. 19), taken on the projection: a wire `o` of another net that does not cross
/// `w` but crosses the straight line between `w`'s two ends an odd number of times has one end
/// inside the region `w` and that line bound, so `w` goes round it. One pair per (`w`, `o`), `w`
/// first (removal, 2012 p. 118: the interfering wire to the other side of `w`'s terminal, `w`
/// along the edge, i.e. `w` laid first, `o` after it). Wires `skip` says are left out (the
/// caller's tree wires). O(wires^2) box tests + O(segments) per overlapping box.
pub fn u_clinches(st: &State, skip: &dyn Fn(WireId) -> bool) -> Vec<Pair> {
    let alive: Vec<WireId> = (0..st.wires.len()).filter(|&w| st.wires[w].alive && !skip(w)).collect();
    let polys: BTreeMap<WireId, (Vec<V2>, Bb)> = alive
        .iter()
        .map(|&w| {
            let pl = st.polyline(w);
            let b = bbox(&pl);
            (w, (pl, b))
        })
        .collect();
    let mut out = Vec::new();
    for &w in &alive {
        let wi = &st.wires[w];
        let (ca, cb) = (st.proj.pos[wi.a], st.proj.pos[wi.b]);
        if (cb - ca).len() <= 0.0 {
            continue;
        }
        let lb = bbox(&[ca, cb]);
        let mut crossed: Option<BTreeSet<WireId>> = None;
        for &o in &alive {
            let oi = &st.wires[o];
            if o == w || oi.net == wi.net || !boxes_meet(lb, polys[&o].1) {
                continue;
            }
            let n = line_crossings(&polys[&o].0, ca, cb);
            if n % 2 == 1 && !crossed.get_or_insert_with(|| st.crossings(w).into_iter().map(|x| x.other).collect()).contains(&o) {
                out.push(Pair { first: w, second: o, kind: Kind::UClinch });
            }
        }
    }
    out
}

/// Mutual evasion (2012 p. 117 sign 4; Luzin 2015 p. 8 c, fig. 20): two wires each a U-clinch of
/// the other (each goes round the other's end). From the U-clinches `uc`; the shorter wire first
/// (the move tries both orders). O(|uc| log |uc|).
pub fn mutual_evasions(st: &State, uc: &[Pair]) -> Vec<Pair> {
    let set: BTreeSet<(WireId, WireId)> = uc.iter().map(|p| (p.first, p.second)).collect();
    let len = |w: WireId| st.wire_len(w);
    set.iter()
        .filter(|&&(a, b)| a < b && set.contains(&(b, a)))
        .map(|&(a, b)| {
            let (first, second) = if len(a) <= len(b) { (a, b) } else { (b, a) };
            Pair { first, second, kind: Kind::MutualEvasion }
        })
        .collect()
}

/// Same-net loops (Luzin 2015 p. 7, fig. 15: "projections of wires of the same net intersect"):
/// pairs of wires of one net whose projections cross properly (detection only: the move is
/// TW7 part C, after TW6 E's branch move). O(wires^2) box tests + segments of overlapping boxes.
pub fn same_net_loops(st: &State) -> Vec<(WireId, WireId)> {
    let alive: Vec<WireId> = (0..st.wires.len()).filter(|&w| st.wires[w].alive).collect();
    let polys: Vec<(WireId, Vec<V2>, Bb)> = alive.iter().map(|&w| {
        let pl = st.polyline(w);
        let b = bbox(&pl);
        (w, pl, b)
    }).collect();
    let mut out = Vec::new();
    for (i, (a, pa, ba)) in polys.iter().enumerate() {
        for (b, pb, bb) in &polys[i + 1..] {
            if st.wires[*a].net != st.wires[*b].net || !boxes_meet(*ba, *bb) {
                continue;
            }
            if pa.windows(2).any(|s| pb.windows(2).any(|t| seg_cross(s[0], s[1], t[0], t[1]))) {
                out.push((*a, *b));
            }
        }
    }
    out
}

/// TW7 A debug attribution (`TOPO_PATDBG`): per live wire whose projected length is more than
/// `ratio` times its straight distance, `(wire, ratio, [(kind, other wire)])` for every pattern
/// it takes part in (`pairs`: the detectors' output; `loops`: [`same_net_loops`], reported with
/// kind `None`). O(pairs + wires x path).
#[allow(clippy::type_complexity)]
pub fn attribute(st: &State, pairs: &[Pair], loops: &[(WireId, WireId)], ratio: f64) -> Vec<(WireId, f64, Vec<(Option<Kind>, WireId)>)> {
    let mut out = Vec::new();
    for (w, wi) in st.wires.iter().enumerate().filter(|(_, wi)| wi.alive) {
        let d = (st.proj.pos[wi.b] - st.proj.pos[wi.a]).len();
        if d <= 0.0 {
            continue;
        }
        let r = st.wire_len(w) / d;
        if r <= ratio {
            continue;
        }
        let mut with: Vec<(Option<Kind>, WireId)> = pairs
            .iter()
            .filter_map(|p| if p.first == w { Some((Some(p.kind), p.second)) } else if p.second == w { Some((Some(p.kind), p.first)) } else { None })
            .collect();
        with.extend(loops.iter().filter_map(|&(a, b)| if a == w { Some((None, b)) } else if b == w { Some((None, a)) } else { None }));
        with.sort();
        with.dedup();
        out.push((w, r, with));
    }
    out
}
