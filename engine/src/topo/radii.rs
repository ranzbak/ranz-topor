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
//! M3 step b (§4.2, Q2; [`radii_layer`]): the spoke tips and the stack radii.
//!
//! 5. **Spoke tips.** Per cut, items in key order, from each end: the end's bound (its clearance
//!    to the first item, the half width of an own-net junction), then per item its width and the
//!    gap to its neighbour; the tip is the item's centre line (`tip_p(i) = end + sum of the
//!    widths and gaps before i + w_i / 2`). The same terms as [`super::demand::need_by`] (an
//!    own-net run at an end sits on the end vertex; `[capacity] mixed_clearance` pairwise or
//!    conservative; a courtyard regime replaces the clearances), so the tips agree with the
//!    plan's need. Distances are in the sheet's planning norm (the cut's `cap`): the octagonal
//!    length for octilinear (Staepelaere eq. 1.2, PDF p. 12), Euclidean for smooth.
//!    **Distant features** (Popov 2012 pp. 120-121): the sheet keeps every critical cut from a
//!    feature vertex to every visible feature (no distance cap), so a stretch that is not next to
//!    a feature still gets its tip from it through the items in between; Popov's stage 2 (the
//!    circle of a segment cut back by the segments before it) is the prefix sum. No new search.
//! 6. **Over-full cuts** (never drop; graded, not thresholds; inflated-sketch §4.6, TopoR manual
//!    p. 56: the minimum width only where needed). First the widths go down towards their neck
//!    floors, all by the same fraction of their neck room; then, if that is not enough, every
//!    gap (end clearances included) by the same factor; past that (widths alone over the cut)
//!    the whole layout is scaled. The shrunk gaps are the cut's predicted violation (`squeeze`),
//!    reported; the tips from both ends then meet (`tip_p + tip_q = cap`).
//! 7. **Stack radii.** Per stack member: the largest tip from the vertex over the cuts at the
//!    vertex that carry it (a crossing item between it and the vertex on one cut pushes it out
//!    there); a member on no cut gets the stack's own layout (the vertex feature's bound, the
//!    members inside it with their widths and gaps). In the planning norm: an octagon's
//!    in-radius for octilinear (Dayan PDF p. 33, octilinear spokes as the arc), a circle for
//!    smooth.
//!
//! Nothing reads this yet (step c draws from the tips and radii).
//! Debug: `TOPO_RADII=1` with a cut report prints the summary of the plan handed to the drawing.
//!
//! Complexity per layer: O(items x curve segments of their stretch) for the keys (a bounding-box
//! reject per segment), O(n log n) per cut for the order and the disagreement count, O(e log e)
//! per stack; the tips O(n) per cut (O(n x 60) on an over-full cut, the neck bisection).

use super::cutflow::{flow_items_into, half_at, in_wedge, FlowItem, Measure, Stretch};
use super::demand::{Bound, Item};
use super::sheet::{Cut, LayerSheet, SegScratch};
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
    /// Step b: the width the tips use (`w`, necked towards the floor on an over-full cut).
    pub w_tip: f64,
    /// Step b: the nearest the centre line may come to `p` / to `q` along the cut (planning norm,
    /// nm). The item's portal is `[tip_p, cap - tip_q]` from `p`.
    pub tip_p: f64,
    pub tip_q: f64,
}

/// The items of one cut in topological order from `p` to `q`.
#[derive(Clone, Debug)]
pub struct CutItems {
    pub cut: usize,
    pub items: Vec<CutItem>,
    /// Step b: the cut's length in the planning norm, the items' need and `cap - need` (nm).
    pub cap: f64,
    pub need: f64,
    pub slack: f64,
    /// Step b, over-full cuts: width taken off by necking, and gap taken off past the necks (the
    /// predicted violation), nm.
    pub neck: f64,
    pub squeeze: f64,
    /// Step b: the junction half widths at the two ends (in `need`; the cut model takes them off
    /// the capacity instead).
    pub halves: f64,
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
    /// Step b: the centre-line radius of each (planning norm, nm, from the vertex).
    pub radius: Vec<f64>,
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
    /// Step b: over-full cuts, their summed deficit, neck and squeeze (nm).
    pub overfull: usize,
    pub deficit: f64,
    pub neck: f64,
    pub squeeze: f64,
    /// Step b: stack members placed by the stack's own layout (on no cut).
    pub radius_own: usize,
    /// Step b: stack members the inside-out pass pushed out past their tip on a cut carrying
    /// them, and by how much in all (nm; graded, reported).
    pub radius_pushed: usize,
    pub radius_push: f64,
    /// Step b: cuts whose need (junction halves off) differs from the cut model's `CutFlow.need`
    /// by more than 1 nm (sizes open decision D1: the key order against the band order).
    pub need_diff: usize,
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

/// An order-preserving match of the sorted values `a` into the sorted values `b` of least total
/// distance (each `a` gets one `b`, strictly increasing; `None` where `b` runs out). O(|a| |b|).
fn match_in_order(a: &[f64], b: &[f64]) -> Vec<Option<f64>> {
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return Vec::new();
    }
    if m < n {
        // Fewer hits than crossings: each nearest (the curve misses the cut where the band crosses).
        return a.iter().map(|&t| b.iter().copied().min_by(|x, y| (x - t).abs().total_cmp(&(y - t).abs()).then(x.total_cmp(y)))).collect();
    }
    // d[i][k]: best cost of a[..=i] with a[i] on b[k].
    let inf = f64::INFINITY;
    let mut d = vec![vec![inf; m]; n];
    let mut from = vec![vec![usize::MAX; m]; n];
    for k in 0..m {
        d[0][k] = (a[0] - b[k]).abs();
    }
    for i in 1..n {
        let (mut best, mut bk) = (inf, usize::MAX);
        for k in 0..m {
            if k > 0 && d[i - 1][k - 1] < best {
                best = d[i - 1][k - 1];
                bk = k - 1;
            }
            if best < inf {
                d[i][k] = best + (a[i] - b[k]).abs();
                from[i][k] = bk;
            }
        }
    }
    let mut k = (0..m).min_by(|&x, &y| d[n - 1][x].total_cmp(&d[n - 1][y]).then(x.cmp(&y))).expect("m >= n >= 1");
    let mut out = vec![None; n];
    for i in (0..n).rev() {
        out[i] = Some(b[k]);
        if i > 0 {
            k = from[i][k];
        }
    }
    out
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
        // Crossing keys per stretch: its band crossings (sorted) paired in order with its curve
        // hits (an order-preserving match of least total distance), so a curve that crosses the
        // cut more often than its band (a reduced back-and-forth) gives each crossing its own hit.
        let mut ckeys: BTreeMap<usize, Vec<(f64, Option<f64>)>> = BTreeMap::new();
        for &(_, x) in raw[i..j].iter().filter(|x| x.1.kind == 0) {
            ckeys.entry(x.stretch).or_default().push((x.t, None));
        }
        for (&si, ts) in ckeys.iter_mut() {
            curve_hits(&sts[si].curve, c.p, c.q, &mut hits);
            hits.sort_by(f64::total_cmp);
            ts.sort_by(|a, b| a.0.total_cmp(&b.0));
            let tb: Vec<f64> = ts.iter().map(|t| t.0).collect();
            for (k, h) in match_in_order(&tb, &hits).into_iter().enumerate() {
                ts[k].1 = h;
            }
        }
        let mut items: Vec<CutItem> = Vec::with_capacity(j - i);
        for &(_, x) in &raw[i..j] {
            let s = &sts[x.stretch];
            let (key, corner) = match x.kind {
                0 => {
                    let ts = &ckeys[&x.stretch];
                    (ts.iter().find(|t| t.0 == x.t).and_then(|t| t.1), None)
                }
                k => {
                    curve_hits(&s.curve, c.p, c.q, &mut hits);
                    let key = pick_key(&hits, k, x.t);
                    let Some((v, u)) = (if k == 1 { Some((c.vp, c.q - c.p)) } else { c.vq.map(|q| (q, c.p - c.q)) }) else { continue };
                    let us = used.entry((x.stretch, v)).or_default();
                    let r = wrap_corner(s, v, u, us).map(|(k, _)| k);
                    if let Some(k) = r {
                        us.push(k);
                    }
                    (key, r)
                }
            };
            items.push(CutItem { stretch: x.stretch, kind: x.kind, t_band: x.t, key: key.unwrap_or(x.t), keyed: key.is_some(), corner, w: x.w, s: s.s, net: s.net, w_tip: x.w, tip_p: 0.0, tip_q: 0.0 });
        }
        items.sort_by(|a, b| a.key.total_cmp(&b.key).then(a.t_band.total_cmp(&b.t_band)).then(a.stretch.cmp(&b.stretch)).then(a.kind.cmp(&b.kind)));
        let tol = 1.0 / (c.q - c.p).len().max(1.0);
        let ts: Vec<f64> = items.iter().map(|x| x.t_band).collect();
        rep.tied_pairs += tied_pairs(&ts, tol);
        let (n, ex) = inversions(&ts, tol, EXAMPLES.saturating_sub(rep.examples.len()));
        rep.disagree += n;
        rep.examples.extend(ex.into_iter().map(|(a, b)| Disagree { cut: ci, a: items[a].stretch, b: items[b].stretch }));
        rep.items += items.len();
        rep.keyless += items.iter().filter(|x| !x.keyed).count();
        cuts.push(CutItems { cut: ci, items, cap: c.cap, need: 0.0, slack: 0.0, neck: 0.0, squeeze: 0.0, halves: 0.0 });
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
        stacks.push(Stack { v, left, order: order.iter().map(|n| n.0).collect(), corner: order.iter().map(|n| n.1).collect(), depth, cycles, radius: Vec::new() });
    }
    rep.stacks = stacks.len();
    LayerOrder { cuts, stacks, report: rep }
}

/// One item for the tips: net, width, neck floor, clearance.
#[derive(Clone, Copy, Debug)]
pub struct TipItem {
    pub net: i32,
    pub w: f64,
    pub floor: f64,
    pub s: f64,
}

/// The rules of one cut's tips: `[capacity] mixed_clearance` and the courtyard regime there.
#[derive(Clone, Copy, Debug)]
pub struct TipRules {
    pub pairwise: bool,
    pub regime: Option<f64>,
}

/// The tips of one cut: from `p`, from `q`, the widths used, and the need, neck, squeeze.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tips {
    pub tip_p: Vec<f64>,
    pub tip_q: Vec<f64>,
    pub w: Vec<f64>,
    pub need: f64,
    pub neck: f64,
    pub squeeze: f64,
}

/// The items and ends with the rules applied: the clearances `s` (regime, conservative), the end
/// bounds (regime on pad clearances).
fn ruled(items: &[TipItem], lo: Bound, hi: Bound, r: TipRules) -> (Vec<TipItem>, Bound, Bound) {
    let smax = items.iter().map(|x| x.s).fold(0.0, f64::max);
    let mut its: Vec<TipItem> = items.to_vec();
    for x in &mut its {
        if let Some(g) = r.regime {
            x.s = g;
        } else if !r.pairwise {
            x.s = smax;
        }
    }
    let reg = |b: Bound| match (b, r.regime) {
        (Bound::Pad { net, .. }, Some(g)) => Bound::Pad { net, clear: g },
        (b, _) => b,
    };
    (its, reg(lo), reg(hi))
}

/// Centre lines from the `lo` end (items in order from `lo`), widths `w`, gaps and end
/// clearances scaled by `g`, the `lo` end holding `half_lo` of own copper: the tips and the far
/// edge plus the `hi` clearance (the need). As [`super::demand::need_by`]: an own-net run at an
/// end sits on that end (no clearance to it, its widest half reaches into the cut, its largest
/// clearance faces the next item); same-net neighbours take no gap; the run at the `hi` end
/// shares one start edge. Caveat: for a member `i` of an own-net end run, `tip_p + tip_q =
/// need - (max_w - w_i) / 2` (the run is one item of its widest half width), so its portal is
/// wider than the slack by that much; every other item has `tip_p + tip_q = need`.
fn layout(its: &[TipItem], w: &[f64], lo: Bound, hi: Bound, half_lo: f64, half_hi: f64, g: f64) -> (Vec<f64>, f64) {
    let n = its.len();
    let own = |b: &Bound, x: &TipItem| matches!(*b, Bound::Pad { net: Some(m), .. } if m == x.net);
    let run_lo = (0..n).take_while(|&i| own(&lo, &its[i])).count();
    let run_hi = (run_lo..n).rev().take_while(|&i| own(&hi, &its[i])).count();
    let mut tip = vec![0.0; n];
    if n == 0 {
        return (tip, 0.0);
    }
    let it = |i: usize| Item { net: its[i].net, w: w[i], s: its[i].s };
    let mut edge;
    if run_lo > 0 {
        for t in tip.iter_mut().take(run_lo) {
            *t = half_lo;
        }
        edge = half_lo + 0.5 * w[..run_lo].iter().copied().fold(0.0, f64::max);
    } else {
        edge = half_lo + g * lo.clearance(&it(0));
    }
    let gap = |a: usize, b: usize| if its[a].net == its[b].net { 0.0 } else { g * its[a].s.max(its[b].s) };
    // An own-net run at an end counts as one item with the largest clearance of the run (as
    // `demand::need_by`).
    let run_s = |r: std::ops::Range<usize>| its[r].iter().map(|x| x.s).fold(0.0, f64::max);
    let mid_end = n - run_hi;
    for i in run_lo..mid_end {
        let tw = if i == run_lo && run_lo > 0 {
            edge + if its[i - 1].net == its[i].net { 0.0 } else { g * run_s(0..run_lo).max(its[i].s) }
        } else if i > 0 {
            edge + gap(i - 1, i)
        } else {
            edge
        };
        tip[i] = tw + 0.5 * w[i];
        edge = tw + w[i];
    }
    if run_hi > 0 {
        let start = if mid_end > 0 {
            let a = mid_end - 1;
            let rs = if a < run_lo { run_s(0..run_lo) } else { its[a].s };
            edge + if its[a].net == its[mid_end].net { 0.0 } else { g * rs.max(run_s(mid_end..n)) }
        } else {
            edge
        };
        for i in mid_end..n {
            tip[i] = start + 0.5 * w[i];
        }
        edge = start + 0.5 * w[mid_end..].iter().copied().fold(0.0, f64::max);
        return (tip, edge + half_hi);
    }
    (tip, edge + g * hi.clearance(&it(n - 1)) + half_hi)
}

/// The spoke tips of the items of one cut (in order from `p`, ends `bp` / `bq` holding `half_p`
/// / `half_q` of own junction copper) in a cut of length `cap` (planning norm). Over-full: necks
/// first (every width by the same fraction of its neck room, bisected), then the gaps by one
/// factor, then the whole layout; the tips from both ends then meet. O(n), O(60 n) over-full.
pub fn cut_tips(items: &[TipItem], bp: Bound, bq: Bound, half_p: f64, half_q: f64, cap: f64, r: TipRules) -> Tips {
    let (its, bp, bq) = ruled(items, bp, bq, r);
    let n = its.len();
    let rev: Vec<TipItem> = its.iter().rev().copied().collect();
    let run = |w: &[f64], g: f64| -> (Vec<f64>, Vec<f64>, f64) {
        let (tp, need) = layout(&its, w, bp, bq, half_p, half_q, g);
        let wr: Vec<f64> = w.iter().rev().copied().collect();
        let (mut tq, _) = layout(&rev, &wr, bq, bp, half_q, half_p, g);
        tq.reverse();
        (tp, tq, need)
    };
    let w0: Vec<f64> = its.iter().map(|x| x.w).collect();
    let (tp, tq, need) = run(&w0, 1.0);
    if need <= cap || n == 0 {
        return Tips { tip_p: tp, tip_q: tq, w: w0, need, neck: 0.0, squeeze: 0.0 };
    }
    // Necks: w(f) = w - f (w - floor), the need falls monotonically in f.
    let wf = |f: f64| -> Vec<f64> { its.iter().map(|x| x.w - f * (x.w - x.floor.min(x.w)).max(0.0)).collect() };
    let need_at = |f: f64, g: f64| layout(&its, &wf(f), bp, bq, half_p, half_q, g).1;
    let f = if need_at(1.0, 1.0) <= cap {
        let (mut a, mut b) = (0.0, 1.0);
        for _ in 0..60 {
            let m = 0.5 * (a + b);
            if need_at(m, 1.0) <= cap {
                b = m;
            } else {
                a = m;
            }
        }
        b
    } else {
        1.0
    };
    let w = wf(f);
    // The need the necks took off (a same-net run counts its widest member once).
    let neck = need - need_at(f, 1.0);
    // Gaps: the need is linear in g (widths fixed).
    let (n1, n0) = (need_at(f, 1.0), need_at(f, 0.0));
    if n1 <= cap {
        let (tp, tq, _) = run(&w, 1.0);
        return Tips { tip_p: tp, tip_q: tq, w, need, neck, squeeze: 0.0 };
    }
    let g = if n1 > n0 { ((cap - n0) / (n1 - n0)).clamp(0.0, 1.0) } else { 0.0 };
    let (mut tp, mut tq, ng) = run(&w, g);
    // Everything past the necks is the predicted violation: gaps, then overlapping widths.
    let squeeze = n1 - cap;
    if ng > cap && ng > 0.0 {
        // Widths alone over the cut: the whole layout scaled.
        let k = cap / ng;
        for t in tp.iter_mut().chain(tq.iter_mut()) {
            *t *= k;
        }
    }
    Tips { tip_p: tp, tip_q: tq, w, need, neck, squeeze }
}

/// A point at planning-norm distance `rho` from `p` along cut `c` (the norm is homogeneous, so
/// the fraction is `rho / cap`).
pub fn tip_point(c: &Cut, rho: f64) -> Vec2 {
    if c.cap <= 0.0 {
        return c.p;
    }
    c.p + (c.q - c.p) * (rho / c.cap)
}

/// Step b on a layer ordered by [`order_layer`]: the spoke tips of every cut and the stack radii.
/// `pairwise`: `[capacity] mixed_clearance`; `regime`: the courtyard clearance at a point.
pub fn radii_layer(sh: &LayerSheet, sts: &[Stretch], mine: &[usize], o: &mut LayerOrder, pairwise: bool, regime: &dyn Fn(Vec2) -> Option<f64>) {
    let mut halves: BTreeMap<usize, f64> = BTreeMap::new();
    let mut half = |f: usize| *halves.entry(f).or_insert_with(|| half_at(sh, f, sts, mine));
    let item = |x: &CutItem| TipItem { net: x.net, w: x.w, floor: sts[x.stretch].w_floor, s: x.s };
    let rep = &mut o.report;
    for ci in &mut o.cuts {
        let c = &sh.cuts[ci.cut];
        let its: Vec<TipItem> = ci.items.iter().map(item).collect();
        let r = TipRules { pairwise, regime: regime((c.p + c.q) * 0.5) };
        let t = cut_tips(&its, sh.bound(c.fp), sh.bound(c.fq), half(c.fp), half(c.fq), c.cap, r);
        for (k, x) in ci.items.iter_mut().enumerate() {
            x.tip_p = t.tip_p[k];
            x.tip_q = t.tip_q[k];
            x.w_tip = t.w[k];
        }
        ci.cap = c.cap;
        ci.need = t.need;
        ci.slack = c.cap - t.need;
        ci.neck = t.neck;
        ci.squeeze = t.squeeze;
        ci.halves = half(c.fp) + half(c.fq);
        if ci.slack < -1.0 {
            rep.overfull += 1;
            rep.deficit -= ci.slack;
            rep.neck += t.neck;
            rep.squeeze += t.squeeze;
        }
    }
    // Stack radii: per member the largest tip from the vertex on the cuts carrying it (the
    // smallest on over-full cuts, design note §4.2.4: the squeezed tips hold there), then an
    // inside-out pass so every member clears the one inside it (Dayan 1997 Fig. 60, pp. 115-116:
    // the attachment list is laid out from the inside; Popov 2012 p. 118: each circle about the
    // pad grows by the wire inside it).
    #[derive(Default)]
    struct MTip {
        free: Option<f64>,
        squeezed: Option<f64>,
        cuts: Vec<(usize, f64)>,
    }
    let mut tips: BTreeMap<(usize, usize, usize), MTip> = BTreeMap::new();
    for (oi, ci) in o.cuts.iter().enumerate() {
        let c = &sh.cuts[ci.cut];
        for x in &ci.items {
            let Some(k) = x.corner else { continue };
            let (v, t, far) = match (x.kind, c.vq) {
                (1, _) => (c.vp, x.tip_p, c.cap - x.tip_q),
                (2, Some(q)) => (q, x.tip_q, c.cap - x.tip_p),
                _ => continue,
            };
            let e = tips.entry((v, x.stretch, k)).or_default();
            if ci.squeeze > 0.0 {
                e.squeezed = Some(e.squeezed.map_or(t, |m| m.min(t)));
            } else {
                e.free = Some(e.free.map_or(t, |m| m.max(t)));
            }
            e.cuts.push((oi, far));
        }
    }
    let mut pidx: BTreeMap<usize, super::cutflow::NearIdx> = BTreeMap::new();
    for st in &mut o.stacks {
        let at = sh.pos[st.v];
        let b = sh.vfeat[st.v].map_or(Bound::Open, |f| sh.bound(f));
        let h = sh.vfeat[st.v].map_or(0.0, &mut half);
        let its: Vec<TipItem> = st
            .order
            .iter()
            .zip(&st.corner)
            .map(|(&s, &k)| {
                let x = &sts[s];
                let idx = pidx.entry(s).or_insert_with(|| x.pass_index());
                TipItem { net: x.net, w: x.width_at_idx(idx, x.band[k]), floor: x.w_floor, s: x.s }
            })
            .collect();
        let r = TipRules { pairwise, regime: regime(at) };
        let (rits, rb, _) = ruled(&its, b, Bound::Open, r);
        let w: Vec<f64> = rits.iter().map(|x| x.w).collect();
        let (own, _) = layout(&rits, &w, rb, Bound::Open, h, 0.0, 1.0);
        let m: Vec<Option<&MTip>> = st.order.iter().zip(&st.corner).map(|(&s, &k)| tips.get(&(st.v, s, k))).collect();
        let mut radius: Vec<f64> = Vec::with_capacity(m.len());
        for (i, mt) in m.iter().enumerate() {
            let base = match mt.and_then(|t| t.free.or(t.squeezed)) {
                Some(t) => t,
                None => {
                    rep.radius_own += 1;
                    own[i]
                }
            };
            let mut r = base;
            if i > 0 {
                // Both on one over-full cut: the squeezed tips already space them.
                let shared_sq = match (m[i - 1], *mt) {
                    (Some(a), Some(b)) => a.cuts.iter().any(|x| o.cuts[x.0].squeeze > 0.0 && b.cuts.iter().any(|y| y.0 == x.0)),
                    _ => false,
                };
                if !shared_sq {
                    let gap = if rits[i - 1].net == rits[i].net { 0.0 } else { rits[i - 1].s.max(rits[i].s) };
                    r = r.max(radius[i - 1] + 0.5 * (w[i - 1] + w[i]) + gap);
                }
            }
            if r > base + 1.0 {
                if let Some(t) = mt {
                    if t.cuts.iter().any(|&(_, far)| r > far + 1.0) {
                        rep.radius_pushed += 1;
                        rep.radius_push += t.cuts.iter().map(|&(_, far)| (r - far).max(0.0)).fold(0.0, f64::max);
                    }
                }
            }
            radius.push(r);
        }
        st.radius = radius;
    }
}

/// The order, tips and radii of every measured layer (the bands of the measurement, its
/// sheets); `regime` the courtyard clearance at a point.
pub fn from_measure(m: &Measure, regime: &dyn Fn(Vec2) -> Option<f64>) -> Vec<LayerOrder> {
    m.layers
        .iter()
        .map(|lf| {
            let mut o = order_layer(&lf.sheet, &m.stretches, &lf.stretches, 0.0);
            radii_layer(&lf.sheet, &m.stretches, &lf.stretches, &mut o, m.opts.pairwise, regime);
            let need: BTreeMap<usize, f64> = lf.flows.iter().map(|f| (f.cut, f.need)).collect();
            o.report.need_diff = o.cuts.iter().filter(|c| need.get(&c.cut).is_some_and(|&n| (c.need - c.halves - n).abs() > 1.0)).count();
            o
        })
        .collect()
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
                let l = lf.sheet.l;
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
                    "overfull": r.overfull, "deficit_mm": r.deficit / 1e6, "neck_mm": r.neck / 1e6,
                    "squeeze_mm": r.squeeze / 1e6, "radius_own": r.radius_own,
                    "radius_pushed": r.radius_pushed, "radius_push_mm": r.radius_push / 1e6, "need_diff": r.need_diff,
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
        let n = obs.len();
        proj_n(obs.into_iter().zip(1..=n as i32).collect())
    }

    /// [`proj`] with the nets given (a stretch `i` of [`setup`] has net 100 + i: its terminal
    /// pads take that net).
    fn proj_n(obs: Vec<(Vec<Vec2>, i32)>) -> Projection {
        let obstacles: Vec<Obstacle> = obs
            .into_iter()
            .map(|(poly, net)| {
                let c = poly.iter().fold(Vec2::default(), |a, &p| a + p) * (1.0 / poly.len() as f64);
                Obstacle { kind: ObsKind::Pad, poly, center: c, layers: LayerMask(1), drilled: false, net: Some(net), clear: 0.2e6, bare: false, sag: 0.0 }
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
        setup_w(pr, curves, norm, &vec![(0.1, 0.1, 0.1); curves.len()])
    }

    /// [`setup`] with per stretch (width, clearance, neck floor) in mm.
    fn setup_w(pr: &Projection, curves: &[Vec<Vec2>], norm: Norm, wsf: &[(f64, f64, f64)]) -> (LayerSheet, Vec<Stretch>) {
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
                    s: wsf[i].1 * 1e6,
                    w: wsf[i].0 * 1e6,
                    w_floor: wsf[i].2 * 1e6,
                    curve: c.clone(),
                    curve_k: vec![0; c.len()],
                    pass_w: vec![wsf[i].0 * 1e6; c.len().saturating_sub(2)],
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

    // ---- Step b ----

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 2.0
    }

    fn mm3(v: &[f64]) -> Vec<f64> {
        v.iter().map(|x| (x / 1e3).round() / 1e3).collect()
    }

    fn ti(net: i32, w: f64, s: f64, floor: f64) -> TipItem {
        TipItem { net, w: w * 1e6, s: s * 1e6, floor: floor * 1e6 }
    }

    const PAD: Bound = Bound::Pad { net: Some(1), clear: 0.2e6 };
    const PAIR: TipRules = TipRules { pairwise: true, regime: None };

    /// Fit pattern 1 (`channel_fit`, approved): three 0.25 mm wires at 0.2 mm through a 1.6 mm
    /// channel. Tips from each wall at minimum clearance: 0.325, 0.775, 1.225; each portal is the
    /// 0.05 mm slack wide and holds the hand's centre line (pitch 0.4625, walls 0.2125).
    #[test]
    fn channel_tips_match_fit_pattern() {
        let xs: Vec<f64> = (0..3).map(|i| 2.0 + 0.3375 + 0.4625 * i as f64).collect();
        let mut obs = vec![rect(0.0, 0.0, 2.0, 2.0), rect(3.6, 0.0, 5.6, 2.0)];
        let mut curves = Vec::new();
        for &x in &xs {
            obs.push(pad(x, 4.0));
            obs.push(pad(x, -2.0));
            curves.push(vec![v(x, 4.0), v(x, -2.0)]);
        }
        let pr = proj(obs);
        for norm in [Norm::Octagonal, Norm::Euclid] {
            let (sh, sts) = setup_w(&pr, &curves, norm, &[(0.25, 0.2, 0.25); 3]);
            let mut o = order_layer(&sh, &sts, &[0, 1, 2], 0.0);
            radii_layer(&sh, &sts, &[0, 1, 2], &mut o, true, &|_| None);
            let mut seen = 0;
            for ci in &o.cuts {
                let c = &sh.cuts[ci.cut];
                let ys = [0.0, 2.0];
                let at = |p: Vec2, x: f64| ys.iter().any(|&y| (p - v(x, y)).len() < 10.0);
                let flip = if at(c.p, 2.0) && at(c.q, 3.6) {
                    false
                } else if at(c.p, 3.6) && at(c.q, 2.0) {
                    true
                } else {
                    continue;
                };
                seen += 1;
                assert!(close(ci.cap, 1.6e6) && close(ci.need, 1.55e6) && close(ci.slack, 0.05e6), "{ci:?}");
                let mut tp: Vec<f64> = ci.items.iter().map(|x| x.tip_p).collect();
                let mut tq: Vec<f64> = ci.items.iter().map(|x| x.tip_q).collect();
                let mut ids: Vec<usize> = ci.items.iter().map(|x| x.stretch).collect();
                if flip {
                    std::mem::swap(&mut tp, &mut tq);
                    tp.reverse();
                    tq.reverse();
                    ids.reverse();
                }
                assert_eq!(ids, vec![0, 1, 2]);
                assert_eq!(mm3(&tp), vec![0.325, 0.775, 1.225], "{norm:?}");
                assert_eq!(mm3(&tq), vec![1.225, 0.775, 0.325], "{norm:?}");
                for (i, &x) in xs.iter().enumerate() {
                    let h = (x - 2.0) * 1e6;
                    assert!(tp[i] <= h && h <= 1.6e6 - tq[i], "hand centre {i} outside its portal");
                    assert!(close(1.6e6 - tq[i] - tp[i], 0.05e6));
                }
            }
            assert!(seen > 0, "no channel cut");
        }
    }

    /// Mixed clearances (the middle net at 0.3 mm): pairwise is KiCad's max of the two
    /// neighbours, conservative every gap and end at the cut's largest; a courtyard regime
    /// replaces them all. The need equals `cutflow::needs` / `demand::need_in`.
    #[test]
    fn mixed_clearance_tips() {
        let its = [ti(10, 0.25, 0.2, 0.25), ti(11, 0.25, 0.3, 0.25), ti(12, 0.25, 0.2, 0.25)];
        let items: Vec<Item> = its.iter().map(|x| Item { net: x.net, w: x.w, s: x.s }).collect();
        let floors: Vec<f64> = its.iter().map(|x| x.floor).collect();
        let t = cut_tips(&its, PAD, PAD, 0.0, 0.0, 3e6, PAIR);
        assert_eq!(mm3(&t.tip_p), vec![0.325, 0.875, 1.425]);
        assert_eq!(mm3(&t.tip_q), vec![1.425, 0.875, 0.325]);
        assert!(close(t.need, super::super::cutflow::needs(None, &items, &floors, PAD, PAD, true).need));
        let c = cut_tips(&its, PAD, PAD, 0.0, 0.0, 3e6, TipRules { pairwise: false, regime: None });
        assert_eq!(mm3(&c.tip_p), vec![0.425, 0.975, 1.525]);
        assert!(close(c.need, super::super::cutflow::needs(None, &items, &floors, PAD, PAD, false).need));
        let r = cut_tips(&its, PAD, PAD, 0.0, 0.0, 3e6, TipRules { pairwise: true, regime: Some(0.15e6) });
        assert_eq!(mm3(&r.tip_p), vec![0.275, 0.675, 1.075]);
        assert!(close(r.need, super::super::demand::need_in(Some(0.15e6), &items, PAD, PAD)));
        // Own-net items: no gap between them, none to their own pad, the run on the vertex.
        let own = [ti(1, 0.25, 0.2, 0.25), ti(1, 0.3, 0.2, 0.3), ti(12, 0.25, 0.2, 0.25)];
        let items: Vec<Item> = own.iter().map(|x| Item { net: x.net, w: x.w, s: x.s }).collect();
        let o = cut_tips(&own, PAD, PAD, 0.0, 0.0, 3e6, PAIR);
        assert_eq!(mm3(&o.tip_p), vec![0.0, 0.0, 0.475]);
        assert!(close(o.need, super::super::demand::need(&items, PAD, PAD)));
    }

    /// Over-full: necks first (all widths by one fraction of their neck room), then every gap by
    /// one factor, then the whole layout; the tips from both ends meet, the rest is the
    /// predicted violation.
    #[test]
    fn overfull_necks_then_gaps() {
        let its = [ti(10, 0.25, 0.2, 0.2), ti(11, 0.25, 0.2, 0.2), ti(12, 0.25, 0.2, 0.2)];
        let t = cut_tips(&its, PAD, PAD, 0.0, 0.0, 1.5e6, PAIR);
        assert!(close(t.need, 1.55e6) && close(t.neck, 0.05e6) && t.squeeze == 0.0, "{t:?}");
        assert!(t.w.iter().all(|&w| (w - 0.25e6 + 0.05e6 / 3.0).abs() < 1.0), "{:?}", t.w);
        for i in 0..3 {
            assert!((t.tip_p[i] + t.tip_q[i] - 1.5e6).abs() < 1.0);
        }
        // No neck room: the four gaps 0.2 -> 0.1875 (0.05 over 0.8).
        let hard: Vec<TipItem> = its.iter().map(|x| TipItem { floor: x.w, ..*x }).collect();
        let t = cut_tips(&hard, PAD, PAD, 0.0, 0.0, 1.5e6, PAIR);
        assert!(t.neck == 0.0 && close(t.squeeze, 0.05e6), "{t:?}");
        assert!(t.tip_p.iter().zip([0.3125e6, 0.75e6, 1.1875e6]).all(|(&a, b)| close(a, b)), "{:?}", t.tip_p);
        for i in 0..3 {
            assert!((t.tip_p[i] + t.tip_q[i] - 1.5e6).abs() < 1.0);
        }
        // Widths alone over the cut: everything scaled, still meeting.
        let t = cut_tips(&hard, PAD, PAD, 0.0, 0.0, 0.5e6, PAIR);
        assert!(close(t.squeeze, 1.05e6), "{t:?}");
        for i in 0..3 {
            assert!((t.tip_p[i] + t.tip_q[i] - 0.5e6).abs() < 1.0);
        }
        assert!(t.tip_p.windows(2).all(|w| w[0] < w[1]));
    }

    /// A tip at planning-norm distance rho: on a cut at 22.5 degrees the octagonal norm puts it
    /// 1 / cos 22.5 further out (Euclidean) than the Euclidean norm.
    #[test]
    fn tip_point_norms() {
        let a = std::f64::consts::FRAC_PI_8;
        let d = Vec2 { x: a.cos(), y: a.sin() } * 2e6;
        let mk = |n: Norm| Cut { p: Vec2::default(), q: d, vp: 0, vq: Some(1), fp: 0, fq: 1, cap: n.len(d), kind: crate::topo::sheet::CutKind::Vertex };
        let (co, ce) = (mk(Norm::Octagonal), mk(Norm::Euclid));
        assert!((tip_point(&ce, 1e6).len() - 1e6).abs() < 1e-3);
        assert!((tip_point(&co, 1e6).len() - 1e6 / a.cos()).abs() < 1e-3);
        assert!((tip_point(&co, co.cap) - d).len() < 1e-6);
    }

    /// The distant-radius check (Popov pp. 120-121): three wires above pad A, the outer one
    /// (W3) two wires away from A. A's cut to the far block carries all three, and W3's tip from
    /// A counts A's clearance and both wires between: 0.2 + 2 x (0.25 + 0.2) + 0.125 = 1.225.
    #[test]
    fn distant_tip_through_wires() {
        let ys = [9.0, 9.5, 10.5];
        let mut obs = vec![rect(7.6, 7.6, 8.4, 8.4), rect(4.6, 7.6, 5.4, 8.4), rect(10.6, 7.6, 11.4, 8.4), rect(6.0, 12.0, 10.0, 13.0)];
        let mut curves = Vec::new();
        for &y in &ys {
            obs.push(pad(1.0, y));
            obs.push(pad(15.0, y));
            curves.push(vec![v(1.0, y), v(15.0, y)]);
        }
        let pr = proj(obs);
        let (sh, sts) = setup_w(&pr, &curves, Norm::Octagonal, &[(0.25, 0.2, 0.25); 3]);
        let a = sh.vfeat[vert(&sh, v(7.6, 8.4))].expect("A");
        let mut o = order_layer(&sh, &sts, &[0, 1, 2], 0.0);
        radii_layer(&sh, &sts, &[0, 1, 2], &mut o, true, &|_| None);
        let mut seen = 0;
        for ci in &o.cuts {
            let c = &sh.cuts[ci.cut];
            if ci.items.len() != 3 || !(c.fp == a || c.fq == a) {
                continue;
            }
            seen += 1;
            let from_a: Vec<(usize, f64)> = ci.items.iter().map(|x| (x.stretch, if c.fp == a { x.tip_p } else { x.tip_q })).collect();
            let w3 = from_a.iter().find(|x| x.0 == 2).expect("W3").1;
            assert!(close(w3, 1.225e6), "{from_a:?}");
        }
        assert!(seen > 0, "no cut from A through all three");
    }

    /// Stack radii at the nested corner: inside-out 0.25, 0.45, 0.65 (block clearance 0.2, widths
    /// and gaps 0.1); the outer member, on no cut, from the stack's own layout.
    #[test]
    fn stack_radii_nested() {
        let ds = [1.3, 0.9, 0.5];
        let mut obs = vec![rect(0.0, 0.0, 2.0, 2.0)];
        let mut curves = Vec::new();
        for d in ds {
            obs.push(pad(0.5, 2.0 + d));
            obs.push(pad(2.0 + d, 0.5));
            curves.push(vec![v(0.5, 2.0 + d), v(2.0 + 0.7 * d, 2.0 + 0.7 * d), v(2.0 + d, 0.5)]);
        }
        let pr = proj(obs);
        let (sh, sts) = setup(&pr, &curves, Norm::Octagonal);
        let mut o = order_layer(&sh, &sts, &[0, 1, 2], 0.0);
        radii_layer(&sh, &sts, &[0, 1, 2], &mut o, true, &|_| None);
        let st = stack(&o, vert(&sh, v(2.0, 2.0)));
        assert_eq!(st.order, vec![2, 1, 0]);
        assert_eq!(mm3(&st.radius), vec![0.25, 0.45, 0.65], "{st:?}");
        assert!(o.report.radius_own >= 1, "{:?}", o.report);
    }

    // ---- Step c (ERBS, `super::super::erbs`) ----

    use crate::topo::erbs::{check_layer, erbs_layer, ErbsOpts, Party};

    /// The channel of fit pattern 1: three straight ERBS on the hand's centre lines' side of
    /// their portals (straight, the pads in line), no violation.
    #[test]
    fn erbs_channel_straight() {
        let xs: Vec<f64> = (0..3).map(|i| 2.0 + 0.3375 + 0.4625 * i as f64).collect();
        let mut obs = vec![(rect(0.0, 0.0, 2.0, 2.0), 1), (rect(3.6, 0.0, 5.6, 2.0), 2)];
        let mut curves = Vec::new();
        for (i, &x) in xs.iter().enumerate() {
            obs.push((pad(x, 4.0), 100 + i as i32));
            obs.push((pad(x, -2.0), 100 + i as i32));
            curves.push(vec![v(x, 4.0), v(x, -2.0)]);
        }
        let pr = proj_n(obs);
        let (sh, sts) = setup_w(&pr, &curves, Norm::Octagonal, &[(0.25, 0.2, 0.25); 3]);
        let mut o = order_layer(&sh, &sts, &[0, 1, 2], 0.0);
        radii_layer(&sh, &sts, &[0, 1, 2], &mut o, true, &|_| None);
        let es = erbs_layer(&sh, &sts, &[0, 1, 2], &o, &ErbsOpts::default());
        assert_eq!(es.len(), 3);
        for (e, &x) in es.iter().zip(&xs) {
            assert!(e.converged);
            assert_eq!(e.path.len(), 2, "{:?}", e.path);
            assert!(e.pts.iter().all(|p| (p.x - x * 1e6).abs() < 1.0), "{:?}", e.pts);
        }
        let vs = check_layer(&sh, &sts, &o, &es);
        assert!(vs.is_empty(), "{vs:?}");
    }

    /// Nested wraps: each ERBS goes round the corner outside its stack radius, as tight as the
    /// arc samples allow (octagon: in-radius to circumradius; circle: to rho / cos(step / 2)),
    /// with no violation between the three or against the block.
    #[test]
    fn erbs_nested_wraps_radii() {
        let ds = [1.3, 0.9, 0.5];
        let mut obs = vec![(rect(0.0, 0.0, 2.0, 2.0), 1)];
        let mut curves = Vec::new();
        for (i, d) in ds.into_iter().enumerate() {
            obs.push((pad(0.5, 2.0 + d), 100 + i as i32));
            obs.push((pad(2.0 + d, 0.5), 100 + i as i32));
            curves.push(vec![v(0.5, 2.0 + d), v(2.0 + 0.7 * d, 2.0 + 0.7 * d), v(2.0 + d, 0.5)]);
        }
        let pr = proj_n(obs);
        for (norm, half) in [(Norm::Octagonal, FRAC_PI_8_), (Norm::Euclid, 11.25f64.to_radians())] {
            let (sh, sts) = setup(&pr, &curves, norm);
            let mut o = order_layer(&sh, &sts, &[0, 1, 2], 0.0);
            radii_layer(&sh, &sts, &[0, 1, 2], &mut o, true, &|_| None);
            let corner = vert(&sh, v(2.0, 2.0));
            let st = stack(&o, corner).clone();
            let es = erbs_layer(&sh, &sts, &[0, 1, 2], &o, &ErbsOpts::default());
            for e in &es {
                assert!(e.converged, "{norm:?}");
                let i = st.order.iter().position(|&s| s == e.stretch).expect("in stack");
                let rho = st.radius[i];
                let d = e.path.windows(2).map(|w| crate::topo::draw::seg_dist(v(2.0, 2.0), w[0], w[1])).fold(f64::INFINITY, f64::min);
                assert!(d >= rho - 1.0 && d <= rho / half.cos() + 1.0, "{norm:?} stretch {}: {d} vs {rho}", e.stretch);
            }
            let vs = check_layer(&sh, &sts, &o, &es);
            if std::env::var_os("TOPO_TEST_DUMP").is_some() {
                for e in &es {
                    eprintln!("{norm:?} st {} band {:?} path {:?}", e.stretch, sts[e.stretch].band.iter().map(|p| (p.x / 1e6, p.y / 1e6)).collect::<Vec<_>>(), e.path.iter().map(|p| ((p.x / 1e3).round() / 1e3, (p.y / 1e3).round() / 1e3)).collect::<Vec<_>>());
                }
            }
            assert!(vs.iter().all(|x| !matches!(x.other, Party::Stretch(_))), "{norm:?} {vs:?}");
            assert!(vs.iter().all(|x| x.predicted), "{norm:?} {vs:?}");
        }
    }

    const FRAC_PI_8_: f64 = std::f64::consts::FRAC_PI_8;

    /// An over-full channel (1.5 mm, no neck room): the ERBS run at the squeezed tips, and every
    /// violation they have is predicted (the over-full cut carries both), none an erbs_gap.
    #[test]
    fn erbs_overfull_predicted() {
        let xs: Vec<f64> = (0..3).map(|i| 2.0 + 0.3125 + 0.4375 * i as f64).collect();
        let mut obs = vec![(rect(0.0, 0.0, 2.0, 2.0), 1), (rect(3.5, 0.0, 5.5, 2.0), 2)];
        let mut curves = Vec::new();
        for (i, &x) in xs.iter().enumerate() {
            obs.push((pad(x, 4.0), 100 + i as i32));
            obs.push((pad(x, -2.0), 100 + i as i32));
            curves.push(vec![v(x, 4.0), v(x, -2.0)]);
        }
        let pr = proj_n(obs);
        let (sh, sts) = setup_w(&pr, &curves, Norm::Octagonal, &[(0.25, 0.2, 0.25); 3]);
        let mut o = order_layer(&sh, &sts, &[0, 1, 2], 0.0);
        radii_layer(&sh, &sts, &[0, 1, 2], &mut o, true, &|_| None);
        assert!(o.report.overfull > 0 && o.report.squeeze > 0.0, "{:?}", o.report);
        let es = erbs_layer(&sh, &sts, &[0, 1, 2], &o, &ErbsOpts::default());
        let vs = check_layer(&sh, &sts, &o, &es);
        assert!(!vs.is_empty());
        assert!(vs.iter().all(|x| x.predicted && x.amount < 0.0125e6 + 2.0), "{vs:?}");
    }

    /// Independent per stretch: the ERBS do not depend on the order of `mine`.
    #[test]
    fn erbs_order_independent() {
        let (obs, curves) = s_bundle();
        let pr = proj(obs);
        let (sh, sts) = setup(&pr, &curves, Norm::Octagonal);
        let mut o = order_layer(&sh, &sts, &[0, 1, 2], 0.0);
        radii_layer(&sh, &sts, &[0, 1, 2], &mut o, true, &|_| None);
        let a = erbs_layer(&sh, &sts, &[0, 1, 2], &o, &ErbsOpts::default());
        let b = erbs_layer(&sh, &sts, &[2, 1, 0], &o, &ErbsOpts::default());
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    /// A violation no over-full cut predicts is an erbs_gap.
    #[test]
    fn erbs_gap_reported() {
        let xs = [2.3375, 3.2625];
        let mut obs = vec![(rect(0.0, 0.0, 2.0, 2.0), 1), (rect(3.6, 0.0, 5.6, 2.0), 2)];
        let mut curves = Vec::new();
        for (i, &x) in xs.iter().enumerate() {
            obs.push((pad(x, 4.0), 100 + i as i32));
            obs.push((pad(x, -2.0), 100 + i as i32));
            curves.push(vec![v(x, 4.0), v(x, -2.0)]);
        }
        let pr = proj_n(obs);
        let (sh, sts) = setup_w(&pr, &curves, Norm::Octagonal, &[(0.25, 0.2, 0.25); 2]);
        let mut o = order_layer(&sh, &sts, &[0, 1], 0.0);
        radii_layer(&sh, &sts, &[0, 1], &mut o, true, &|_| None);
        let mut es = erbs_layer(&sh, &sts, &[0, 1], &o, &ErbsOpts::default());
        // Move the second ERBS 0.2 mm towards the first (a model gap, not the plan's).
        for p in es[1].pts.iter_mut() {
            p.x = 2.4375e6;
        }
        let vs = check_layer(&sh, &sts, &o, &es);
        let g: Vec<_> = vs.iter().filter(|x| x.other == Party::Stretch(1)).collect();
        assert_eq!(g.len(), 1, "{vs:?}");
        assert!(!g[0].predicted && (g[0].amount - 0.35e6).abs() < 2.0, "{g:?}");
    }

    // ---- Verifier findings F1-F3, F6 ----

    /// F2: an own-net run at an end faces the next item with the run's largest clearance (as
    /// `demand::need_by`), not its edge member's.
    #[test]
    fn own_run_gap_uses_run_max_clearance() {
        let its = [ti(1, 0.25, 0.3, 0.25), ti(1, 0.25, 0.1, 0.25), ti(12, 0.25, 0.1, 0.25)];
        let items: Vec<Item> = its.iter().map(|x| Item { net: x.net, w: x.w, s: x.s }).collect();
        let t = cut_tips(&its, PAD, Bound::Open, 0.0, 0.0, 3e6, PAIR);
        // Run on p (tip 0, reaches 0.125), gap 0.3 (the run's max), then 0.125: 0.55.
        assert_eq!(mm3(&t.tip_p), vec![0.0, 0.0, 0.55]);
        assert!(close(t.need, super::super::demand::need(&items, PAD, Bound::Open)));
        let t = cut_tips(&[its[2], its[1], its[0]], Bound::Open, PAD, 0.0, 0.0, 3e6, PAIR);
        // From p: 0.25 of the first wire, the run's gap 0.3, then the run's centre lines.
        assert_eq!(mm3(&t.tip_p), vec![0.125, 0.675, 0.675]);
        assert_eq!(mm3(&t.tip_q), vec![0.55, 0.0, 0.0]);
    }

    /// F3: a junction end's half width moves every tip from that end and adds to the need; an own
    /// run sits on the end at the half width; the caveat sum for run members.
    #[test]
    fn junction_halves_and_own_run_tips() {
        let its = [ti(10, 0.25, 0.2, 0.25), ti(11, 0.25, 0.2, 0.25)];
        let a = cut_tips(&its, PAD, PAD, 0.0, 0.0, 3e6, PAIR);
        let b = cut_tips(&its, PAD, PAD, 0.1e6, 0.05e6, 3e6, PAIR);
        for i in 0..2 {
            assert!(close(b.tip_p[i], a.tip_p[i] + 0.1e6) && close(b.tip_q[i], a.tip_q[i] + 0.05e6));
        }
        assert!(close(b.need, a.need + 0.15e6));
        // Own run (net 1 = the PAD's) at p with a junction half 0.1: the run's tips are 0.1.
        let own = [ti(1, 0.2, 0.2, 0.2), ti(1, 0.3, 0.2, 0.3), ti(12, 0.25, 0.2, 0.25)];
        let t = cut_tips(&own, PAD, Bound::Open, 0.1e6, 0.0, 3e6, PAIR);
        assert_eq!(mm3(&t.tip_p[..2]), vec![0.1, 0.1]);
        // Caveat: tip_p + tip_q = need - (max_w - w_i) / 2 for run members, = need otherwise.
        for (i, x) in own.iter().enumerate() {
            let short = if i < 2 { 0.5 * (0.3e6 - x.w) } else { 0.0 };
            assert!(close(t.tip_p[i] + t.tip_q[i], t.need - short), "{i}: {t:?}");
        }
    }

    /// F1: stack radii take the largest tip over the cuts carrying a member (not the last or the
    /// smallest), and an inside-out pass keeps every member a pitch outside the one inside it.
    #[test]
    fn stack_radii_max_then_inside_out() {
        let ds = [1.3, 0.9, 0.5];
        let mut obs = vec![rect(0.0, 0.0, 2.0, 2.0)];
        let mut curves = Vec::new();
        for d in ds {
            obs.push(pad(0.5, 2.0 + d));
            obs.push(pad(2.0 + d, 0.5));
            curves.push(vec![v(0.5, 2.0 + d), v(2.0 + 0.7 * d, 2.0 + 0.7 * d), v(2.0 + d, 0.5)]);
        }
        let pr = proj(obs);
        let (sh, sts) = setup(&pr, &curves, Norm::Octagonal);
        let corner = vert(&sh, v(2.0, 2.0));
        // A larger clearance on one side of the corner only (the regime covers the midpoints of
        // the cuts towards the upper-left pads): the inner member's tips differ per cut.
        for g in [0.35e6, 0.5e6] {
            let mut o = order_layer(&sh, &sts, &[0, 1, 2], 0.0);
            let reg = move |p: Vec2| if p.x < 2.0e6 && p.y < 2.5e6 && p.y > 2.0e6 { Some(g) } else { None };
            radii_layer(&sh, &sts, &[0, 1, 2], &mut o, true, &reg);
            let st = stack(&o, corner);
            assert_eq!(st.order, vec![2, 1, 0]);
            // The inner member's radius is its largest tip over the cuts at the corner.
            let inner = o
                .cuts
                .iter()
                .filter_map(|ci| {
                    let c = &sh.cuts[ci.cut];
                    ci.items.iter().find(|x| x.stretch == 2 && x.corner.is_some()).map(|x| if x.kind == 1 { x.tip_p } else { x.tip_q }).filter(|_| c.vp == corner || c.vq == Some(corner))
                })
                .collect::<Vec<f64>>();
            assert!(inner.len() >= 2 && inner.iter().any(|&t| (t - inner[0]).abs() > 1.0), "tips do not differ: {inner:?}");
            let mx = inner.iter().copied().fold(0.0, f64::max);
            assert!(close(st.radius[0], mx), "{g}: {:?} vs {inner:?}", st.radius);
            for i in 1..3 {
                assert!(st.radius[i] >= st.radius[i - 1] + 0.2e6 - 1.0, "{g}: {:?}", mm3(&st.radius));
            }
        }
    }

    /// F6: a stretch's crossings of one cut get their own curve hits, in order.
    #[test]
    fn crossings_match_hits_in_order() {
        assert_eq!(match_in_order(&[0.2, 0.25], &[0.1, 0.22, 0.6]), vec![Some(0.1), Some(0.22)]);
        assert_eq!(match_in_order(&[0.2, 0.7], &[0.1, 0.22, 0.6]), vec![Some(0.22), Some(0.6)]);
        assert_eq!(match_in_order(&[0.5], &[0.1, 0.6]), vec![Some(0.6)]);
        assert_eq!(match_in_order(&[0.2, 0.3], &[0.25]), vec![Some(0.25), Some(0.25)]);
    }
}
