//! M3 step a (shopping list 6.2, Q1; design note `docs/progress/notes/m3-radii-spokes-design.md`
//! §4.1): the topological order of the items on every sheet cut, and the attachment stacks.
//!
//! The rubber bands of [`super::cutflow::band`] are taut, so bands that wrap one vertex, or run
//! as a parallel bundle between two wrapped vertices, coincide: their crossing `t` on a cut ties
//! and [`super::cutflow`] orders the tie by stretch id. That order is not the plan's. The plan's
//! order is the order of the projection curves (terminal, pass points, terminal): the pass points
//! are spread in `edge_order` along every projection edge ([`super::state::State::pass_point`]),
//! so two curves that do not cross keep their order along every cut between two features
//! (Dayan 1997, PDF p. 125: the attachment list of a vertex is kept inside-out; the rubber-band
//! sketch takes the order from the topological route, not from the geometry).
//!
//! 1. **Curve key.** For every item (stretch `s`, cut `pq`): where `s`'s curve crosses `pq`, as
//!    a fraction from `p`. A crossing item takes the curve crossing nearest to its band crossing;
//!    a wrap at `p` (at `q`) the crossing nearest to `p` (to `q`). A curve that does not cross
//!    the cut (the band and the curve disagree on the cut, a walk detour) keeps its band `t` and
//!    is counted (`keyless`).
//! 2. **Order.** The items of a cut sorted by key (then band `t`, stretch, kind: total, so the
//!    result does not depend on the input order).
//! 3. **Disagreement.** Item pairs whose band order, outside ties, contradicts the key order. On
//!    a planar plan it is 0; a pair here is a same-layer crossing of the curves the band does not
//!    show (the curves cross twice), reported, never repaired ("costly, not failed").
//! 4. **Stacks.** Per (sheet vertex, side): the wraps of every cut at the vertex, inside-out on
//!    each cut by key, plus one ranking along the members' mean outward bisector (so a wrap whose
//!    wedge holds no cut still gets its place), merged into one order by a topological sort of
//!    the "inside of" pairs. A
//!    cycle (cuts that disagree: an inconsistent plan) is broken at the node of least mean depth
//!    and counted.
//!
//! Nothing reads this yet (step b computes the radii from the stacks and the per-cut order).
//! Debug: `TOPO_RADII=1` with a cut report prints the summary of the plan handed to the drawing.
//!
//! Complexity per layer: O(items x curve segments of their stretch) for the keys (a bounding-box
//! reject per segment), O(n log n) per cut for the order and the disagreement count, O(e log e)
//! per stack.

use super::cutflow::{flow_items_into, in_wedge, FlowItem, Measure, Stretch};
use super::sheet::{LayerSheet, SegScratch};
use crate::geom::Vec2;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Disagreement examples kept per layer for the debug output.
const EXAMPLES: usize = 20;

/// One stretch on one cut, with its topological key.
#[derive(Clone, Copy, Debug)]
pub struct CutItem {
    pub stretch: usize,
    /// 0 crosses the open segment, 1 wraps `p`, 2 wraps `q` ([`FlowItem::kind`]).
    pub kind: u8,
    /// The band's crossing (0 / 1 for wraps).
    pub t_band: f64,
    /// Where the stretch's curve crosses the cut (0..1 from `p`); `t_band` when it does not.
    pub key: f64,
    /// Whether `key` comes from the curve.
    pub keyed: bool,
    /// The band corner a wrap belongs to (`None` for a crossing).
    pub corner: Option<usize>,
    pub w: f64,
    pub s: f64,
    pub net: i32,
}

/// The items of one cut in topological order from `p` to `q`.
#[derive(Clone, Debug)]
pub struct CutItems {
    pub cut: usize,
    pub items: Vec<CutItem>,
}

/// Attachment stack (Dayan PDF 125): the bands wrapping sheet vertex `v` with `v` on the given
/// side, inner-most first.
#[derive(Clone, Debug)]
pub struct Stack {
    pub v: usize,
    /// `v` lies left of the bands (the bands pass right of it), as [`Stretch::wraps`].
    pub left: bool,
    /// Stretch ids, inside-out.
    pub order: Vec<usize>,
    /// The band corner of each (a stretch can wrap one vertex more than once).
    pub corner: Vec<usize>,
    /// Mean distance from `v` along the cuts it was ranked on (nm, Euclidean, curve keys).
    pub depth: Vec<f64>,
    /// Forced picks: a cycle of "inside of" pairs broken at the least mean depth.
    pub cycles: usize,
}

/// A pair whose band order contradicts the key order: `a` is before `b` by key, after by band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Disagree {
    pub cut: usize,
    pub a: usize,
    pub b: usize,
}

/// Counts of one layer.
#[derive(Clone, Debug, Default)]
pub struct OrderReport {
    pub cuts: usize,
    pub items: usize,
    /// Items whose curve does not cross their cut (key = band t).
    pub keyless: usize,
    /// Item pairs whose band crossings tie (within 1 nm): the key orders them.
    pub tied_pairs: usize,
    /// Item pairs whose band order outside ties contradicts the key order.
    pub disagree: usize,
    pub examples: Vec<Disagree>,
    pub stacks: usize,
    pub stack_entries: usize,
    pub stack_cycles: usize,
}

/// The order of one layer.
#[derive(Clone, Debug, Default)]
pub struct LayerOrder {
    pub cuts: Vec<CutItems>,
    pub stacks: Vec<Stack>,
    pub report: OrderReport,
}

/// The fractions along `p q` (0..1 from `p`) where the polyline `curve` meets the segment
/// (a vertex on the segment counts; a piece collinear with it is skipped, its ends count).
fn curve_hits(curve: &[Vec2], p: Vec2, q: Vec2, out: &mut Vec<f64>) {
    out.clear();
    let e = q - p;
    let el = e.len();
    if el <= 0.0 {
        return;
    }
    let (lo, hi) = (Vec2 { x: p.x.min(q.x), y: p.y.min(q.y) }, Vec2 { x: p.x.max(q.x), y: p.y.max(q.y) });
    for k in 0..curve.len().saturating_sub(1) {
        let (a, b) = (curve[k], curve[k + 1]);
        if a.x.max(b.x) < lo.x || a.x.min(b.x) > hi.x || a.y.max(b.y) < lo.y || a.y.min(b.y) > hi.y {
            continue;
        }
        let d = b - a;
        let den = d.cross(e);
        if den.abs() <= 1e-12 * d.len() * el {
            continue;
        }
        let w0 = p - a;
        let sp = w0.cross(e) / den;
        let tc = w0.cross(d) / den;
        // 1e-9 of a segment: a curve vertex exactly on the cut meets it from both pieces.
        if (-1e-9..=1.0 + 1e-9).contains(&sp) && (-1e-9..=1.0 + 1e-9).contains(&tc) {
            out.push(tc.clamp(0.0, 1.0));
        }
    }
}

/// The key of an item from the hits of its curve on the cut.
fn pick_key(hits: &[f64], kind: u8, t_band: f64) -> Option<f64> {
    let it = hits.iter().copied();
    match kind {
        1 => it.min_by(f64::total_cmp),
        2 => it.max_by(f64::total_cmp),
        _ => it.min_by(|a, b| (a - t_band).abs().total_cmp(&(b - t_band).abs()).then(a.total_cmp(b))),
    }
}

/// The band corner of a wrap item at vertex `v`: the corner wrapping `v` whose wedge holds the
/// cut's direction `u` from `v` (the first such corner, as [`flow_items_into`] emits one item per
/// corner and cut).
fn wrap_corner(s: &Stretch, v: usize, u: Vec2, used: &[usize]) -> Option<(usize, bool)> {
    (1..s.band.len().saturating_sub(1)).find_map(|k| {
        let (wv, left) = s.wraps[k]?;
        if wv != v || used.contains(&k) {
            return None;
        }
        let (din, dout) = (s.band[k] - s.band[k - 1], s.band[k + 1] - s.band[k]);
        in_wedge(din, dout, left, u).then_some((k, left))
    })
}

/// The outward bisector at a wrap (from the vertex into the reflex side, towards the band): the
/// difference of the unit legs; at a corner without a turn the normal on the band's side.
fn outward(din: Vec2, dout: Vec2, left: bool) -> Vec2 {
    let (a, b) = (din.unit().unwrap_or_default(), dout.unit().unwrap_or_default());
    // The vertex lies left of the band: the band lies on the vertex's right of the travel.
    let side = if left { -a.perp() } else { a.perp() };
    match (a - b).unit() {
        Some(u) if u.dot(side) > 0.0 => u,
        _ => side,
    }
}

/// Pairs of `ts` (in key order) whose order contradicts the order of their values beyond `tol`:
/// the count, and the first contradicting pairs (indices, earlier first). O(n log n).
fn inversions(ts: &[f64], tol: f64, examples: usize) -> (usize, Vec<(usize, usize)>) {
    let n = ts.len();
    let mut sorted: Vec<f64> = ts.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mut bit = vec![0usize; n + 1];
    let mut count = 0;
    let mut ex = Vec::new();
    for (j, &t) in ts.iter().enumerate() {
        // Earlier items with a value above t + tol.
        let ub = sorted.partition_point(|&x| x <= t + tol);
        let mut le = 0;
        let mut i = ub;
        while i > 0 {
            le += bit[i];
            i &= i - 1;
        }
        let above = j - le;
        count += above;
        if above > 0 && ex.len() < examples {
            if let Some(i) = (0..j).find(|&i| ts[i] > t + tol) {
                ex.push((i, j));
            }
        }
        let mut i = sorted.partition_point(|&x| x < t) + 1;
        while i <= n {
            bit[i] += 1;
            i += i & i.wrapping_neg();
        }
    }
    (count, ex)
}

/// Pairs of values within `tol` of each other. O(n log n).
fn tied_pairs(ts: &[f64], tol: f64) -> usize {
    let mut s: Vec<f64> = ts.to_vec();
    s.sort_by(f64::total_cmp);
    let mut lo = 0;
    let mut n = 0;
    for j in 0..s.len() {
        while s[j] - s[lo] > tol {
            lo += 1;
        }
        n += j - lo;
    }
    n
}

/// One node of a stack merge: (stretch, band corner).
type Node = (usize, usize);

/// Per-cut inside-out rankings of a stack's nodes, with their distance from the vertex (nm).
type Rankings = Vec<Vec<(Node, f64)>>;

/// Merge per-cut inside-out rankings (each a list of (node, distance from the vertex)) into one
/// inside-out order: a topological sort of the consecutive "inside of" pairs, ready nodes taken
/// by least mean distance (then id); a cycle is broken at the remaining node of least mean
/// distance and counted. Returns (order, mean distances, cycles). Deterministic.
pub fn merge_stack(rankings: &[Vec<(Node, f64)>]) -> (Vec<Node>, Vec<f64>, usize) {
    let mut depth: BTreeMap<Node, (f64, usize)> = BTreeMap::new();
    let mut succ: BTreeMap<Node, Vec<Node>> = BTreeMap::new();
    let mut indeg: BTreeMap<Node, usize> = BTreeMap::new();
    for r in rankings {
        for (i, &(nd, d)) in r.iter().enumerate() {
            let e = depth.entry(nd).or_insert((0.0, 0));
            e.0 += d;
            e.1 += 1;
            indeg.entry(nd).or_insert(0);
            if i > 0 {
                let a = r[i - 1].0;
                if a != nd {
                    succ.entry(a).or_default().push(nd);
                    *indeg.entry(nd).or_insert(0) += 1;
                }
            }
        }
    }
    let mean = |nd: &Node| depth.get(nd).map_or(0.0, |&(s, c)| s / c.max(1) as f64);
    let by_mean = |a: &Node, b: &Node| mean(a).total_cmp(&mean(b)).then(a.cmp(b));
    let mut order = Vec::with_capacity(indeg.len());
    let mut cycles = 0;
    let mut left: BTreeMap<Node, usize> = indeg;
    while !left.is_empty() {
        let pick = match left.iter().filter(|(_, &d)| d == 0).map(|(n, _)| *n).min_by(by_mean) {
            Some(n) => n,
            None => {
                cycles += 1;
                *left.keys().min_by(|a, b| by_mean(a, b)).expect("nonempty")
            }
        };
        left.remove(&pick);
        if let Some(ss) = succ.get(&pick) {
            for s in ss {
                if let Some(d) = left.get_mut(s) {
                    *d = d.saturating_sub(1);
                }
            }
        }
        order.push(pick);
    }
    let means = order.iter().map(&mean).collect();
    (order, means, cycles)
}

/// The topological order of the items of stretches `mine` on the cuts of sheet `sh` (their bands
/// computed), and the attachment stacks. `via_r` as [`flow_items_into`].
pub fn order_layer(sh: &LayerSheet, sts: &[Stretch], mine: &[usize], via_r: f64) -> LayerOrder {
    let mut raw: Vec<(usize, FlowItem)> = Vec::new();
    flow_items_into(sh, sts, mine, via_r, &mut SegScratch::default(), &mut raw);
    let mut rep = OrderReport::default();
    let mut cuts: Vec<CutItems> = Vec::new();
    let mut hits = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let ci = raw[i].0;
        let j = i + raw[i..].iter().take_while(|x| x.0 == ci).count();
        let c = &sh.cuts[ci];
        let mut used: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
        let mut items: Vec<CutItem> = raw[i..j]
            .iter()
            .map(|&(_, x)| {
                let s = &sts[x.stretch];
                curve_hits(&s.curve, c.p, c.q, &mut hits);
                let key = pick_key(&hits, x.kind, x.t);
                let corner = match x.kind {
                    0 => None,
                    k => {
                        let (v, u) = if k == 1 { (c.vp, c.q - c.p) } else { (c.vq.unwrap_or(usize::MAX), c.p - c.q) };
                        let us = used.entry((x.stretch, v)).or_default();
                        let r = wrap_corner(s, v, u, us).map(|(k, _)| k);
                        if let Some(k) = r {
                            us.push(k);
                        }
                        r
                    }
                };
                CutItem { stretch: x.stretch, kind: x.kind, t_band: x.t, key: key.unwrap_or(x.t), keyed: key.is_some(), corner, w: x.w, s: s.s, net: s.net }
            })
            .collect();
        items.sort_by(|a, b| a.key.total_cmp(&b.key).then(a.t_band.total_cmp(&b.t_band)).then(a.stretch.cmp(&b.stretch)).then(a.kind.cmp(&b.kind)));
        let tol = 1.0 / (c.q - c.p).len().max(1.0);
        let ts: Vec<f64> = items.iter().map(|x| x.t_band).collect();
        rep.tied_pairs += tied_pairs(&ts, tol);
        let (n, ex) = inversions(&ts, tol, EXAMPLES.saturating_sub(rep.examples.len()));
        rep.disagree += n;
        rep.examples.extend(ex.into_iter().map(|(a, b)| Disagree { cut: ci, a: items[a].stretch, b: items[b].stretch }));
        rep.items += items.len();
        rep.keyless += items.iter().filter(|x| !x.keyed).count();
        cuts.push(CutItems { cut: ci, items });
        i = j;
    }
    rep.cuts = cuts.len();
    // Stacks: per (vertex, side) the per-cut inside-out rankings of the wraps at that vertex.
    let mut rank: BTreeMap<(usize, bool), Rankings> = BTreeMap::new();
    for ci in &cuts {
        let c = &sh.cuts[ci.cut];
        let len = (c.q - c.p).len();
        for (kind, v) in [(1u8, Some(c.vp)), (2u8, c.vq)] {
            let Some(v) = v else { continue };
            let mut per: BTreeMap<bool, Vec<(Node, f64)>> = BTreeMap::new();
            for x in ci.items.iter().filter(|x| x.kind == kind) {
                let Some(k) = x.corner else { continue };
                let Some((_, left)) = sts[x.stretch].wraps[k] else { continue };
                let d = if kind == 1 { x.key * len } else { (1.0 - x.key) * len };
                per.entry(left).or_default().push(((x.stretch, k), d));
            }
            for (left, mut r) in per {
                if kind == 2 {
                    r.reverse();
                }
                rank.entry((v, left)).or_default().push(r);
            }
        }
    }
    // Plus one ranking per stack along the members' mean outward bisector from the vertex: every
    // wrap gets a place even where no cut lies in its wedge (a corner facing open board, the
    // outer bands of a nested set whose wedges hold no cut). A member whose curve misses the ray
    // ranks alone at the ray's length (outer-most unless a cut places it).
    let mut own: Vec<usize> = mine.to_vec();
    own.sort_unstable();
    own.dedup();
    let mut members: BTreeMap<(usize, bool), Vec<(Node, Vec2)>> = BTreeMap::new();
    for &si in &own {
        let s = &sts[si];
        for k in 1..s.band.len().saturating_sub(1) {
            let Some((v, left)) = s.wraps[k] else { continue };
            members.entry((v, left)).or_default().push(((si, k), outward(s.band[k] - s.band[k - 1], s.band[k + 1] - s.band[k], left)));
        }
    }
    for ((v, left), ms) in members {
        let at = sh.pos[v];
        let Some(u) = ms.iter().fold(Vec2::default(), |a, m| a + m.1).unit() else { continue };
        let reach = ms.iter().flat_map(|m| sts[m.0 .0].curve.iter()).map(|&p| (p - at).len()).fold(0.0, f64::max) + 1.0;
        let end = at + u * reach;
        let mut r: Vec<(Node, f64)> = Vec::new();
        let rs = rank.entry((v, left)).or_default();
        for (nd, _) in &ms {
            curve_hits(&sts[nd.0].curve, at, end, &mut hits);
            match hits.iter().copied().min_by(f64::total_cmp) {
                Some(t) => r.push((*nd, t * reach)),
                None => rs.push(vec![(*nd, reach)]),
            }
        }
        r.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        rs.push(r);
    }
    let mut stacks = Vec::with_capacity(rank.len());
    for ((v, left), rs) in rank {
        let (order, depth, cycles) = merge_stack(&rs);
        rep.stack_entries += order.len();
        rep.stack_cycles += cycles;
        stacks.push(Stack { v, left, order: order.iter().map(|n| n.0).collect(), corner: order.iter().map(|n| n.1).collect(), depth, cycles });
    }
    rep.stacks = stacks.len();
    LayerOrder { cuts, stacks, report: rep }
}

/// The order of every measured layer (the bands of the measurement, its sheets).
pub fn from_measure(m: &Measure) -> Vec<LayerOrder> {
    m.layers.iter().map(|lf| order_layer(&lf.sheet, &m.stretches, &lf.stretches, 0.0)).collect()
}

/// Summary per layer (debug output).
pub fn summary(m: &Measure, ords: &[LayerOrder], layer_names: &dyn Fn(usize) -> String) -> Value {
    let mm = |p: Vec2| json!([(p.x / 1e6 * 1e4).round() / 1e4, (-p.y / 1e6 * 1e4).round() / 1e4]);
    Value::Array(
        m.layers
            .iter()
            .zip(ords)
            .map(|(lf, o)| {
                let r = &o.report;
                let l = lf.stretches.first().map_or(0, |&s| m.stretches[s].layer);
                let ex: Vec<Value> = r
                    .examples
                    .iter()
                    .map(|d| {
                        let c = &lf.sheet.cuts[d.cut];
                        json!({"p": mm(c.p), "q": mm(c.q), "a": m.stretches[d.a].wire, "b": m.stretches[d.b].wire, "net_a": m.stretches[d.a].net, "net_b": m.stretches[d.b].net})
                    })
                    .collect();
                json!({
                    "layer": layer_names(l), "cuts": r.cuts, "items": r.items, "keyless": r.keyless,
                    "tied_pairs": r.tied_pairs, "disagree": r.disagree, "stacks": r.stacks,
                    "stack_entries": r.stack_entries, "stack_cycles": r.stack_cycles, "examples": ex,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topo::cutflow::band;
    use crate::topo::sheet::{Norm, SheetInput, SheetOpts};
    use crate::topo::state::{LayerMask, ObsKind, Obstacle, Projection, VertKind};

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x: x * 1e6, y: y * 1e6 }
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Vec2> {
        vec![v(x0, y0), v(x1, y0), v(x1, y1), v(x0, y1)]
    }

    fn pad(x: f64, y: f64) -> Vec<Vec2> {
        rect(x - 0.1, y - 0.1, x + 0.1, y + 0.1)
    }

    /// A projection of the given obstacles on layer 0 (each its own net).
    fn proj(obs: Vec<Vec<Vec2>>) -> Projection {
        let obstacles: Vec<Obstacle> = obs
            .into_iter()
            .enumerate()
            .map(|(i, poly)| {
                let c = poly.iter().fold(Vec2::default(), |a, &p| a + p) * (1.0 / poly.len() as f64);
                Obstacle { kind: ObsKind::Pad, poly, center: c, layers: LayerMask(1), drilled: false, net: Some(i as i32 + 1), clear: 0.2e6, bare: false, sag: 0.0 }
            })
            .collect();
        let mut pts = Vec::new();
        let mut cons = Vec::new();
        for (i, ob) in obstacles.iter().enumerate() {
            let s = pts.len();
            for &p in &ob.poly {
                pts.push((p, VertKind::Outline(i)));
            }
            for j in 0..ob.poly.len() {
                cons.push((s + j, s + (j + 1) % ob.poly.len()));
            }
        }
        Projection::build(2, obstacles, &pts, &cons, |_| true).expect("projection")
    }

    /// The sheet of layer 0 with the curve ends as terminals, and the stretches (curves given,
    /// bands computed). Stretch `i` has net 100 + i.
    fn setup(pr: &Projection, curves: &[Vec<Vec2>], norm: Norm) -> (LayerSheet, Vec<Stretch>) {
        let terms: Vec<Vec2> = curves.iter().flat_map(|c| [c[0], c[c.len() - 1]]).collect();
        let inp = SheetInput { proj: pr, outline: &[], vias: &[], junctions: &[], terminals: &terms };
        let sh = LayerSheet::build(&inp, 0, &SheetOpts { norm, ..SheetOpts::default() }).expect("sheet");
        let sts = curves
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let (b, w) = band(&sh, c).expect("band");
                Stretch {
                    wire: i,
                    conn: None,
                    net: 100 + i as i32,
                    layer: 0,
                    s: 0.1e6,
                    w: 0.1e6,
                    w_floor: 0.1e6,
                    curve: c.clone(),
                    curve_k: vec![0; c.len()],
                    pass_w: vec![0.1e6; c.len().saturating_sub(2)],
                    nosite: false,
                    via_ends: [false; 2],
                    band: b,
                    wraps: w,
                }
            })
            .collect();
        (sh, sts)
    }

    fn vert(sh: &LayerSheet, p: Vec2) -> usize {
        sh.pos.iter().position(|&q| (q - p).len() < 10.0).expect("vertex")
    }

    fn stack(o: &LayerOrder, v: usize) -> &Stack {
        let s: Vec<&Stack> = o.stacks.iter().filter(|s| s.v == v).collect();
        assert_eq!(s.len(), 1, "one stack at {v}: {:?}", o.stacks);
        s[0]
    }

    /// Three bands wrapping the corner (2, 2) of a block, ids reversed (stretch 0 outer-most):
    /// the stack is inside-out by the curves, not by id; the band t of the wraps ties.
    #[test]
    fn nested_wraps_inside_out() {
        let ds = [1.3, 0.9, 0.5];
        let mut obs = vec![rect(0.0, 0.0, 2.0, 2.0)];
        let mut curves = Vec::new();
        for d in ds {
            obs.push(pad(0.5, 2.0 + d));
            obs.push(pad(2.0 + d, 0.5));
            curves.push(vec![v(0.5, 2.0 + d), v(2.0 + 0.7 * d, 2.0 + 0.7 * d), v(2.0 + d, 0.5)]);
        }
        let pr = proj(obs);
        for norm in [Norm::Octagonal, Norm::Euclid] {
            let (sh, sts) = setup(&pr, &curves, norm);
            let a = vert(&sh, v(2.0, 2.0));
            for s in &sts {
                assert!(s.wraps.iter().flatten().any(|w| w.0 == a), "{norm:?} band {:?}", s.band);
            }
            let o = order_layer(&sh, &sts, &[0, 1, 2], 0.0);
            let st = stack(&o, a);
            assert_eq!(st.order, vec![2, 1, 0], "{norm:?} {st:?}");
            assert_eq!(st.cycles, 0);
            assert!(st.depth.windows(2).all(|w| w[0] < w[1]), "{:?}", st.depth);
            assert_eq!(o.report.disagree, 0, "{:?}", o.report);
            assert_eq!(o.report.keyless, 0, "{:?}", o.report);
            assert!(o.report.tied_pairs > 0, "{:?}", o.report);
        }
    }

    /// The curves of lane `j` of the S bundle over block O1 (2..3 x 0..3) and under block O2
    /// (5..6 x 2..5): lane 0 hugs O1 and is the outer-most under O2.
    fn s_bundle() -> (Vec<Vec<Vec2>>, Vec<Vec<Vec2>>) {
        let mut obs = vec![rect(2.0, 0.0, 3.0, 3.0), rect(5.0, 2.0, 6.0, 5.0)];
        let mut curves = Vec::new();
        for j in 0..3 {
            let jf = j as f64;
            let (x0, y1) = (1.7 - 0.3 * jf, 3.3 + 0.3 * jf);
            let (y2, x3) = (2.0 - 0.3 * (3.0 - jf), 6.0 + 0.3 * (3.0 - jf));
            obs.push(pad(x0, 0.6));
            obs.push(pad(x3, 4.4));
            curves.push(vec![v(x0, 0.6), v(x0, y1), v(3.0 + 0.1 * (jf + 1.0), y1), v(5.0 - 0.1 * (3.0 - jf), y2), v(x3, y2), v(x3, 4.4)]);
        }
        (obs, curves)
    }

    /// The S bundle with lanes 0, 1, 2 as stretches 1, 2, 0 (neither id order nor its reverse):
    /// the bands coincide from (2, 3) to (6, 2); the stacks at O1's corners are lanes 0, 1, 2
    /// inside-out, at O2's corners 2, 1, 0; every tie broken by the curves, no disagreement.
    #[test]
    fn parallel_bundle_topological() {
        let (obs, lanes) = s_bundle();
        let ids = [1usize, 2, 0];
        let mut curves = vec![Vec::new(); 3];
        for (j, c) in lanes.into_iter().enumerate() {
            curves[ids[j]] = c;
        }
        let pr = proj(obs);
        let (sh, sts) = setup(&pr, &curves, Norm::Octagonal);
        let o = order_layer(&sh, &sts, &[0, 1, 2], 0.0);
        let lane = |js: [usize; 3]| js.iter().map(|&j| ids[j]).collect::<Vec<_>>();
        // (6, 2) is left out: `pack::funnel` misses that corner (see `funnel_sharp_turn`).
        for (p, want) in [((2.0, 3.0), [0, 1, 2]), ((3.0, 3.0), [0, 1, 2]), ((5.0, 2.0), [2, 1, 0])] {
            let st = stack(&o, vert(&sh, v(p.0, p.1)));
            assert_eq!(st.order, lane(want), "at {p:?}: {st:?}");
            assert_eq!(st.cycles, 0);
        }
        assert_eq!(o.report.disagree, 0, "{:?}", o.report);
        assert_eq!(o.report.stack_cycles, 0);
        assert!(o.report.tied_pairs > 0);
        // Every cut: the items of one band position in lane order along the curves.
        for ci in &o.cuts {
            for w in ci.items.windows(2) {
                assert!(w[0].key <= w[1].key);
            }
        }
    }

    /// The S-bundle lanes turn up round O2's corner (6, 2) after wrapping (5, 2). Every lane's
    /// curve passes (6, 2) on its left, but `cutflow::band` returns (5, 2) -> end pad straight,
    /// through O2 and with (6, 2) on the right: in `pack::funnel` the right side's tighten test
    /// `(rp - apex) x (r - apex) >= 0` reads a portal end more than 180 degrees round from the
    /// right boundary (rp = (3, 0) south-west of the apex (5, 2), r = (7, 4.3) north-east) as
    /// widening, so the crossing over the left side (6, 2) is never seen. Reported (M3 step a);
    /// the fix changes the cut model's bands (planner pricing), so it waits for the lead.
    #[test]
    #[ignore = "pack::funnel misses a corner when the funnel opens past 180 degrees (reported)"]
    fn funnel_sharp_turn() {
        let (obs, curves) = s_bundle();
        let pr = proj(obs);
        let (sh, sts) = setup(&pr, &curves, Norm::Octagonal);
        let c = vert(&sh, v(6.0, 2.0));
        for s in &sts {
            assert!(s.wraps.iter().flatten().any(|w| w.0 == c), "band {:?}", s.band);
        }
    }

    /// Stretch 1's curve crosses stretch 0's curve twice inside the gap between two blocks; the
    /// bands are straight and do not cross. On the cuts between the two crossings the key order
    /// (the plan) contradicts the band order: reported, and the key order kept.
    #[test]
    fn planned_double_crossing_reported() {
        let obs = vec![rect(0.0, 0.0, 2.0, 2.0), rect(4.0, 0.0, 6.0, 2.0), pad(2.6, 4.0), pad(2.6, -2.0), pad(3.4, 4.0), pad(3.4, -2.0)];
        let curves = vec![vec![v(2.6, 4.0), v(2.6, -2.0)], vec![v(3.4, 4.0), v(2.3, 2.5), v(2.3, -0.5), v(3.4, -2.0)]];
        let pr = proj(obs);
        let (sh, sts) = setup(&pr, &curves, Norm::Octagonal);
        let o = order_layer(&sh, &sts, &[0, 1], 0.0);
        assert!(o.report.disagree > 0, "{:?}", o.report);
        assert!(o.report.examples.iter().all(|d| (d.a, d.b) == (1, 0) || (d.a, d.b) == (0, 1)));
        // Every cut across the gap (a vertex of the left block's side x = 2 to one of the right
        // block's side x = 4) between the two crossings carries stretch 1 nearer x = 2 (the
        // curves' order), and its bands the other way round.
        let mut seen = 0;
        for ci in &o.cuts {
            let c = &sh.cuts[ci.cut];
            let side = |p: Vec2, x: f64| (p.x - x * 1e6).abs() < 10.0 && p.y > -1.0 && p.y < 2.0 * 1e6 + 10.0;
            let from_left = if side(c.p, 2.0) && side(c.q, 4.0) {
                true
            } else if side(c.p, 4.0) && side(c.q, 2.0) {
                false
            } else {
                continue;
            };
            if ci.items.len() != 2 || ci.items.iter().any(|x| x.kind != 0) {
                continue;
            }
            seen += 1;
            let ids: Vec<usize> = ci.items.iter().map(|x| x.stretch).collect();
            assert_eq!(ids, if from_left { vec![1, 0] } else { vec![0, 1] }, "{ci:?}");
        }
        assert!(seen > 0, "no cut across the gap: {:?}", o.cuts);
    }

    #[test]
    fn merge_consistent_and_cycle() {
        let (a, b, c) = ((5, 1), (3, 1), (9, 2));
        let (o, d, cy) = merge_stack(&[vec![(a, 1.0), (b, 2.0)], vec![(b, 1.5), (c, 3.0)], vec![(a, 0.5), (c, 2.5)]]);
        assert_eq!(o, vec![a, b, c]);
        assert_eq!(cy, 0);
        assert_eq!(d, vec![0.75, 1.75, 2.75]);
        // a in b, b in c, c in a: one forced pick at the least mean depth (c), then a, b.
        let (o, _, cy) = merge_stack(&[vec![(a, 1.0), (b, 2.0)], vec![(b, 1.0), (c, 2.0)], vec![(c, 0.1), (a, 3.0)]]);
        assert_eq!(cy, 1);
        assert_eq!(o, vec![c, a, b]);
    }

    #[test]
    fn inversions_and_ties() {
        let (n, ex) = inversions(&[0.1, 0.3, 0.2, 0.2, 0.0], 1e-9, 10);
        // (0.1, 0.0), (0.3, 0.2), (0.3, 0.2), (0.3, 0.0), (0.2, 0.0), (0.2, 0.0).
        assert_eq!(n, 6);
        assert_eq!(ex[0], (1, 2));
        assert_eq!(inversions(&[0.5, 0.5, 0.5], 1e-9, 10).0, 0);
        assert_eq!(inversions(&[0.5 + 1e-12, 0.5], 1e-9, 10).0, 0);
        assert_eq!(tied_pairs(&[0.5, 0.5, 0.5, 0.7], 1e-9), 3);
    }

    /// The result does not depend on the order of `mine`.
    #[test]
    fn deterministic() {
        let (obs, curves) = s_bundle();
        let pr = proj(obs);
        let (sh, sts) = setup(&pr, &curves, Norm::Octagonal);
        let a = order_layer(&sh, &sts, &[0, 1, 2], 0.0);
        let b = order_layer(&sh, &sts, &[2, 0, 1], 0.0);
        assert_eq!(format!("{:?}", a.cuts), format!("{:?}", b.cuts));
        assert_eq!(format!("{:?}", a.stacks), format!("{:?}", b.stacks));
        assert_eq!(format!("{:?}", a.report), format!("{:?}", b.report));
    }
}
