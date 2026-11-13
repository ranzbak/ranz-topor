//! P1a / P1b of the abstract-first roadmap (shopping list 2.2 / 2.3): the per-layer cut test as
//! the fit authority of the plan. "Does it fit?" is the flow of every critical cut of a layer
//! sheet ([`super::sheet`], U0) against its capacity, priced by a graded over-full price.
//!
//! **Interface.** [`flow`]`(sheet, runs, vias, opts)` -> the flow of every nonempty cut (items,
//! capacity, need, slack): the standalone test feeds wires (per-layer polylines) straight in. The
//! planner uses [`CutPrice`], the same flow kept per wire so a layer move is priced by the change
//! of `sum price x phi` over the cuts it touches.
//!
//! **Model** (docs/dev/inflated-sketch.md §1.2, §4.3, §4.4; Maley 1987 pp. 29, 32-33, 40):
//!
//! - Per layer one *static* sheet: the obstacles present on the layer (pads on it, every drilled
//!   pad: drill to drill on an inner layer, P1b), the board outline, the junction points of the
//!   State and the pad centres as terminals. Its critical cuts are measured in the planning norm
//!   (octagonal for the octilinear style: Staepelaere 1992 eq. 1.2, PDF p. 12; Euclidean for
//!   smooth). An edge of the shared projection whose end is another layer's pad is no cut here:
//!   the room on layer `l` is the sheet's (the per-edge model's `state.rs` room is infinite there
//!   and `pack.rs` gives that end no margin).
//! - A wire's runs ([`super::cutflow::wire_stretches`]) are pulled taut in the sheet
//!   ([`super::cutflow::band`]); a run crosses a cut (an item at its crossing position) or wraps a
//!   cut end inside its wrapped wedge ([`super::cutflow::flow_items`], U1).
//! - A planned via is copper on every layer. It is no sheet feature here (the sheet stays static,
//!   so a move costs a walk of the moved wire, not a rebuild): it is an item of every cut whose
//!   segment passes within `r + s` of its centre, of width `L - 2 s`, `L = 2 sqrt((r + s)^2 -
//!   d^2)` the stretch of the cut its disc and clearance block (`d` = distance of the centre to
//!   the cut; `L - 2 s = 2 r` on the cut). The item enters at `L = s`, where its two gaps equal the
//!   one gap it splits: no jump. The exact model (the via a feature that splits the cut into two
//!   cuts, U1's measurement) needs a sheet update per via move: the incremental model of shopping
//!   list 3.1.
//! - Need = [`super::cutflow::needs`] (widths, clearances between different nets, the end
//!   clearances; no oblique factor: Maley's congestion is a plain width sum, p. 32); slack =
//!   capacity - need (the planner: capacity - need at the neck floors, see "Three levels").
//!
//! **Price** (design §4.4): with `u = -slack / pitch` (overflow in track pitches, `pitch` = mean
//! width + clearance of the cut's items) and the softness `tau` (`[capacity] overflow_softness`),
//! `phi(u) = logistic(u / tau) + tau ln(1 + e^(u / tau))`; a cut costs `overflow_mm x phi`. For
//! `u >> tau` this is `1 + u`, the per-edge price `overflow_mm (1 + over / pitch)` of
//! `refine.rs`, so the weights carry over; at `u = 0` it is about 0.57; for `u << 0` it goes to 0
//! smoothly. No step, no cutoff (graded factors): a nearly full cut already costs a little.
//!
//! **Three levels** (shopping list 2.4; TopoR 6.3 manual p. 56: the minimum width "will be used
//! in bottlenecks unless a different path is available"; pp. 72-73: shrinkages, "width reductions
//! in nominally-wide wires", are a criterion of their own next to violations, vias and length).
//! The planner ([`CutPrice`]) needs every cut twice: `N` with every run at its nominal width, `F`
//! with every run at its neck floor (Board Setup minimum track width or the net's custom rule;
//! vias do not neck). With capacity `C`: `C >= N` costs 0 (nominal fit); `F <= C < N` fits only
//! necked and costs the shrinkage price `shrink_mm x red / pitch`, `red = N - C` the width
//! reduction ([`shrink_need`]); beyond the floors the shrinkage is all of `N - F` and the
//! violation is the rest, priced by `phi` of the slack `C - F`. Both parts are continuous in `C`
//! (no jump where the nominal widths stop fitting nor at the floors). The over-full count and
//! [`CutPrice::over_amount`] count only slack below the floors. The widths a shrinking cut needs
//! are [`CutPrice::necks`]; the plan writes them into its pass widths before the drawing, so the
//! drawing necks where the plan priced it and the variant's shrinkages count them.
//!
//! Complexity: building the sheets is U0's; a wire costs a walk and a funnel per run plus a grid
//! query per band segment and per via; repricing a cut is O(items log items).

use super::cutflow::{band, flow_items_into, needs, wire_stretches, FlowItem, NearIdx, Stretch};
use super::demand::{need_in_by, Bound, Item};
use super::pack::Packer;
use super::refine::LEv;
use super::sheet::{SegScratch, FeatKind, JunctionFeat, LayerSheet, Norm, SheetInput, SheetOpts};
use super::state::{point_in_poly, Projection, State, VertId, VertKind, WireId, NO_OBS};
use crate::geom::Vec2;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

/// Slack below this (nm) is over-full (the counts; the price has no threshold).
pub const OVER_TOL: f64 = 1.0;

/// Options of the cut model.
#[derive(Clone, Copy, Debug)]
pub struct CutOpts {
    pub sheet: SheetOpts,
    /// `mixed_clearance = "pairwise"`.
    pub pairwise: bool,
    /// Softness `tau` of the over-full price, in track pitches (> 0).
    pub tau: f64,
    /// Shopping list 2.4: the shrinkage price, mm per track pitch of width reduction a cut needs
    /// below the nominal widths (`[factors] shrinkage`, default the `necking` weight).
    pub shrink_mm: f64,
}

impl CutOpts {
    pub fn from_options(o: &crate::model::Options) -> CutOpts {
        CutOpts {
            sheet: SheetOpts { norm: Norm::for_style(&o.draw_style), radius_nm: o.cut_radius_mm.max(0.0) * 1e6, via_steps: 16, prune_walk: true },
            pairwise: o.mixed_clearance == "pairwise",
            tau: o.overflow_softness.max(1e-3),
            shrink_mm: super::factors::Registry::from_config(super::factors::standard_factors(), &o.factors).0.weight("shrinkage").unwrap_or(30.0),
        }
    }
}

/// Shopping list 2.4: the width reduction (nm) a cut with capacity `cap` needs below the nominal
/// need `need` when its wires may neck down to the need `floor` (TopoR 6.3 manual p. 56: the
/// minimum width "in bottlenecks"): 0 while the nominal widths fit, `need - cap` while it fits
/// necked, `need - floor` (all the necking there is) beyond the floors, where the rest is the
/// violation. Continuous in `cap` (piecewise linear: no jump at either boundary). O(1).
pub fn shrink_need(cap: f64, need: f64, floor: f64) -> f64 {
    (need - cap).clamp(0.0, (need - floor).max(0.0))
}

/// The graded over-full price factor of a cut with `slack` and item `pitch` (nm), softness
/// `tau` (pitches): `logistic(u / tau) + tau softplus(u / tau)`, `u = -slack / pitch`. Smooth and
/// increasing; `1 + u` for `u >> tau`, 0 for `u << 0`. O(1).
pub fn phi(slack: f64, pitch: f64, tau: f64) -> f64 {
    let u = -slack / pitch.max(1.0);
    let z = u / tau;
    let logistic = if z >= 0.0 { 1.0 / (1.0 + (-z).exp()) } else { z.exp() / (1.0 + z.exp()) };
    let softplus = if z > 30.0 { z } else if z < -30.0 { z.exp() } else { z.exp().ln_1p() };
    logistic + tau * softplus
}

/// A planned via as an item of the cuts it blocks (on every layer).
#[derive(Clone, Copy, Debug)]
pub struct ViaItem {
    pub wire: WireId,
    pub at: Vec2,
    /// Copper radius and clearance (nm).
    pub r: f64,
    pub s: f64,
    pub net: i32,
    /// Traversal of the via in its wire.
    pub k: usize,
}

/// How a via disc (centre `at`, radius `r`, clearance `s`) blocks the cut `p q`: its position
/// along the cut (0..1) and its item width `L - 2 s` (`L` the blocked stretch, clipped to the
/// cut), or `None` when it blocks less than one clearance (`L <= s`: its two gaps would not
/// exceed the one gap it splits) or its foot is not inside the cut. O(1).
pub fn via_block(p: Vec2, q: Vec2, at: Vec2, r: f64, s: f64) -> Option<(f64, f64)> {
    let e = q - p;
    let len = e.len();
    if len <= 1.0 {
        return None;
    }
    let t = (at - p).dot(e) / (len * len);
    if t <= 0.0 || t >= 1.0 {
        return None;
    }
    let reach = r + s;
    let d = (at - (p + e * t)).len();
    if d >= reach {
        return None;
    }
    let h = (reach * reach - d * d).sqrt();
    let (a, b) = ((t * len - h).max(0.0), (t * len + h).min(len));
    let w = (b - a) - 2.0 * s;
    (w > -s).then_some((t, w))
}

/// The cuts of `sh` a via blocks: `(cut, position along it 0..1, item width)` ([`via_block`]).
/// O(cuts in the grid cells of the via's reach).
pub fn via_hits(sh: &LayerSheet, v: &ViaItem, buf: &mut Vec<u32>) -> Vec<(usize, f64, f64)> {
    let reach = v.r + v.s;
    let pad = Vec2 { x: reach, y: reach };
    sh.cut_grid.query(v.at - pad, v.at + pad, buf);
    buf.iter().filter_map(|&ci| {
        let c = &sh.cuts[ci as usize];
        via_block(c.p, c.q, v.at, v.r, v.s).map(|(t, w)| (ci as usize, t, w))
    }).collect()
}

/// The items of one wire on one sheet: `(cut, item)`, and its junction ends `(feature, half
/// width)`.
type WireItems = (Vec<(usize, Itm)>, Vec<(usize, f64)>);

/// Cached static sheets and the junction points they were built with.
type SheetCache = Option<(Vec<(i64, i64, i32)>, Rc<Vec<LayerSheet>>)>;

/// Per junction feature of a layer: (wire, part, half width) of the runs ending there.
type JHalf = BTreeMap<usize, Vec<(WireId, u32, f64)>>;

/// Via site points per (triangle, net, r, s).
type SiteCache = HashMap<(usize, i32, i64, i64), (Vec2, bool)>;

/// One item of a cut.
#[derive(Clone, Copy, Debug)]
pub struct Itm {
    pub wire: WireId,
    /// Position along the cut from `p` (0..1).
    pub t: f64,
    /// 0 = crossing, 1 / 2 = wraps `p` / `q`, 3 = a via.
    pub kind: u8,
    pub it: Item,
    pub floor: f64,
    /// The nominal width (nm): the wire's netclass width (a via: its item width). The cut is
    /// priced at the nominal widths and at the floors (2.4), never at the width the plan necked a
    /// pass to (`it.w`), so the shrinkage price stays put when the necks are written.
    pub nom: f64,
    /// Traversal of the wire nearest to the item (the place a layer move acts on).
    pub k: usize,
    /// The run or via of the wire that put it there ([`CutPrice`] bookkeeping; 0 in [`flow`]).
    pub src: u32,
}

/// The flow of one nonempty cut.
#[derive(Clone, Debug)]
pub struct Flow {
    pub cut: usize,
    pub items: Vec<Itm>,
    /// Capacity after the half widths of junction ends (nm).
    pub cap: f64,
    pub need: f64,
    pub need_other: f64,
    pub need_floor: f64,
    pub slack: f64,
    /// `phi` of the slack (the price per `overflow_mm`).
    pub phi: f64,
}

/// The junction feature of every junction point of a sheet, by position.
fn junction_feats(sh: &LayerSheet) -> HashMap<(i64, i64), usize> {
    sh.feats
        .iter()
        .enumerate()
        .filter(|(_, f)| matches!(f.kind, FeatKind::Junction(_)))
        .map(|(i, f)| (key(f.poly[0]), i))
        .collect()
}

fn key(p: Vec2) -> (i64, i64) {
    (p.x.round() as i64, p.y.round() as i64)
}

/// Nearest point of `curve` to `x`: its traversal (`curve_k`).
fn nearest_k(s: &Stretch, x: Vec2) -> usize {
    let mut best = (f64::INFINITY, 0usize);
    for (i, &p) in s.curve.iter().enumerate() {
        let d = (p - x).len2();
        if d < best.0 {
            best = (d, s.curve_k.get(i).copied().unwrap_or(0));
        }
    }
    best.1
}

/// [`nearest_k`] through an index of the curve points: the same traversal.
fn nearest_k_idx(s: &Stretch, idx: Option<&NearIdx>, x: Vec2) -> usize {
    let Some(idx) = idx else { return nearest_k(s, x) };
    idx.nearest(|i| s.curve[i], x).map_or(0, |i| s.curve_k.get(i).copied().unwrap_or(0))
}

/// The items one wire adds to a sheet: its runs on the sheet's layer (`runs` with bands) and its
/// vias. `(cut, item)`, and the junction features its runs end at with the half width.
fn wire_items(sh: &LayerSheet, jf: &HashMap<(i64, i64), usize>, runs: &[Stretch], vias: &[ViaItem], buf: &mut Vec<u32>) -> WireItems {
    let via_r = vias.iter().map(|v| v.r).fold(0.0, f64::max);
    let (mut out, jends) = run_items(sh, jf, runs, via_r);
    for v in vias {
        for (ci, t, w) in via_hits(sh, v, buf) {
            out.push((ci, Itm { wire: v.wire, t, kind: 3, it: Item { net: v.net, w, s: v.s }, floor: w, nom: w, k: v.k, src: 0 }));
        }
    }
    (out, jends)
}

/// The items of the runs `runs` on the sheet's layer (bands pulled) for a wire whose vias have
/// radius `via_r`, and the junction features they end at with the half width. Each run's
/// items depend on that run alone. O(band segments x cuts in their grid cells + wraps).
fn run_items(sh: &LayerSheet, jf: &HashMap<(i64, i64), usize>, runs: &[Stretch], via_r: f64) -> WireItems {
    run_items_with(sh, jf, runs, via_r, &mut SegScratch::default(), &mut Vec::new())
}

/// [`run_items`] with the caller's scratch.
fn run_items_with(sh: &LayerSheet, jf: &HashMap<(i64, i64), usize>, runs: &[Stretch], via_r: f64, sc: &mut SegScratch, per: &mut Vec<(usize, FlowItem)>) -> WireItems {
    let mine: Vec<usize> = (0..runs.len()).filter(|&i| runs[i].layer == sh.l).collect();
    flow_items_into(sh, runs, &mine, via_r, sc, per);
    let mut out = Vec::with_capacity(per.len());
    let near: Vec<Option<NearIdx>> = (0..runs.len()).map(|i| mine.contains(&i).then(|| NearIdx::new(runs[i].curve.len(), |j| runs[i].curve[j]))).collect();
    for &(ci, x) in per.iter() {
        let c = &sh.cuts[ci];
        let s = &runs[x.stretch];
        let at = match x.kind {
            1 => c.p,
            2 => c.q,
            _ => c.p + (c.q - c.p) * x.t,
        };
        out.push((ci, Itm { wire: s.wire, t: x.t, kind: x.kind, it: Item { net: s.net, w: x.w, s: s.s }, floor: s.w_floor.min(s.w), nom: s.w, k: nearest_k_idx(s, near[x.stretch].as_ref(), at), src: 0 }));
    }
    let mut jends = Vec::new();
    for &i in &mine {
        let s = &runs[i];
        for end in [s.curve.first(), s.curve.last()].into_iter().flatten() {
            if let Some(&f) = jf.get(&key(*end)) {
                jends.push((f, 0.5 * s.w));
            }
        }
    }
    (out, jends)
}

/// The order of the items of a cut: along it, ties by wire, kind and traversal. Every pricing
/// sorts by it (stable), so the incremental model and a full recompute see one order.
fn item_order(a: &Itm, b: &Itm) -> std::cmp::Ordering {
    a.t.total_cmp(&b.t).then(a.wire.cmp(&b.wire)).then(a.kind.cmp(&b.kind)).then(a.k.cmp(&b.k))
}

/// Whether two sorted item lists price the same: equal (net, width, clearance, floor, nominal
/// width, kind) item by item ([`CutPrice`] install). O(items).
fn same_keys(a: &[Itm], b: &[Itm]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| {
        x.it.net == y.it.net && x.it.w.to_bits() == y.it.w.to_bits() && x.it.s.to_bits() == y.it.s.to_bits() && x.floor.to_bits() == y.floor.to_bits() && x.nom.to_bits() == y.nom.to_bits() && x.kind == y.kind
    })
}

/// The ends of cut `ci` (bounds and the half widths of junction ends). `jhalf(f)`: the half
/// width carried by a junction feature on this layer (`None`: no run ends there, the end is open).
fn cut_ends(sh: &LayerSheet, ci: usize, jhalf: &dyn Fn(usize) -> Option<f64>) -> ((Bound, f64), (Bound, f64)) {
    let c = &sh.cuts[ci];
    let end = |f: usize| -> (Bound, f64) {
        if matches!(sh.feats[f].kind, FeatKind::Junction(_)) {
            match jhalf(f) {
                Some(h) => (sh.bound(f), h),
                None => (Bound::Open, 0.0),
            }
        } else {
            (sh.bound(f), 0.0)
        }
    };
    (end(c.fp), end(c.fq))
}

/// Need, slack and price of a cut with its items sorted ([`item_order`]). `jhalf(f)`: see
/// [`cut_ends`].
fn evaluate(sh: &LayerSheet, ci: usize, items: &mut [Itm], regime: Option<f64>, jhalf: &dyn Fn(usize) -> Option<f64>, o: &CutOpts) -> (f64, Needs3) {
    items.sort_by(item_order);
    let c = &sh.cuts[ci];
    let ((bp, hp), (bq, hq)) = cut_ends(sh, ci, jhalf);
    let cap = c.cap - hp - hq;
    let its: Vec<Item> = items.iter().map(|x| x.it).collect();
    let floors: Vec<f64> = items.iter().map(|x| x.floor).collect();
    let n = needs(regime, &its, &floors, bp, bq, o.pairwise);
    (cap, Needs3 { need: n.need, other: n.other, floor: n.floor })
}

/// The planner's pricing of a cut (shopping list 2.4): capacity, the need at the nominal
/// widths and the need at the neck floors, with the selected clearance rule only (the other rule
/// is the standalone test's report), without building item lists (the same operations as
/// [`needs`], so the same floats). Without a neckable item the floor need is the nominal one.
/// O(items log items).
fn evaluate_need(sh: &LayerSheet, ci: usize, items: &mut [Itm], regime: Option<f64>, jhalf: &dyn Fn(usize) -> Option<f64>, o: &CutOpts) -> (f64, f64, f64) {
    items.sort_by(item_order);
    let ((bp, hp), (bq, hq)) = cut_ends(sh, ci, jhalf);
    let cap = sh.cuts[ci].cap - hp - hq;
    let items = &*items;
    let smax = if o.pairwise { 0.0 } else { items.iter().map(|x| x.it.s).fold(0.0, f64::max) };
    let at = |i: usize, w: f64| -> Item {
        let x = &items[i];
        Item { net: x.it.net, w, s: if o.pairwise { x.it.s } else { smax } }
    };
    let need = need_in_by(regime, items.len(), |i| at(i, items[i].nom), bp, bq);
    let floor = if items.iter().any(|x| x.floor < x.nom) { need_in_by(regime, items.len(), |i| at(i, items[i].floor), bp, bq) } else { need };
    (cap, need, floor)
}

#[derive(Clone, Copy, Debug)]
struct Needs3 {
    need: f64,
    other: f64,
    floor: f64,
}

/// The mean pitch (nominal width + clearance) of a cut's run items (vias only: theirs). O(items).
fn pitch_of(items: &[Itm]) -> f64 {
    let (mut sum, mut n) = (0.0, 0usize);
    for x in items.iter().filter(|x| x.kind != 3) {
        sum += x.nom + x.it.s;
        n += 1;
    }
    if n == 0 {
        items.iter().map(|x| x.nom.max(0.0) + x.it.s).sum::<f64>() / items.len().max(1) as f64
    } else {
        sum / n as f64
    }
}

/// The standalone cut test (P1a): the flow of every nonempty cut of `sh` under the runs `runs`
/// (stretches with their per-layer curves; the bands are pulled here) and the vias `vias`.
/// `regime(p)`: the courtyard clearance rule at a point. Cuts sorted by id. O(runs x walk +
/// crossings + vias x grid).
pub fn flow(sh: &LayerSheet, runs: &mut [Stretch], vias: &[ViaItem], regime: &dyn Fn(Vec2) -> Option<f64>, o: &CutOpts) -> Result<Vec<Flow>, String> {
    for s in runs.iter_mut().filter(|s| s.layer == sh.l) {
        let (b, w) = band(sh, &s.curve)?;
        s.band = b;
        s.wraps = w;
    }
    let jf = junction_feats(sh);
    let mut buf = Vec::new();
    let (items, jends) = wire_items(sh, &jf, runs, vias, &mut buf);
    let mut jh: BTreeMap<usize, f64> = BTreeMap::new();
    for (f, h) in jends {
        let e = jh.entry(f).or_insert(0.0);
        *e = e.max(h);
    }
    let mut per: BTreeMap<usize, Vec<Itm>> = BTreeMap::new();
    for (ci, it) in items {
        per.entry(ci).or_default().push(it);
    }
    let mut out = Vec::with_capacity(per.len());
    for (ci, mut its) in per {
        let c = &sh.cuts[ci];
        let (cap, n) = evaluate(sh, ci, &mut its, regime((c.p + c.q) * 0.5), &|f| jh.get(&f).copied(), o);
        let slack = cap - n.need;
        let ph = phi(slack, pitch_of(&its), o.tau);
        out.push(Flow { cut: ci, items: its, cap, need: n.need, need_other: n.other, need_floor: n.floor, slack, phi: ph });
    }
    Ok(out)
}

/// The static sheets of a State: per projection layer the obstacles on it, the outline, every
/// junction point of the alive wires (their branch terminals) and the pad centres as terminals.
/// Planned vias are not features (they are items, [`via_hits`]). O(layers x U0 sheet build).
pub fn static_sheets(st: &State, outline: &[(Vec2, Vec2)], o: &CutOpts) -> Result<Vec<LayerSheet>, String> {
    let pr = st.proj;
    let junc = junctions(st);
    let terms: Vec<Vec2> = pr.obstacles.iter().filter(|ob| ob.net.is_some()).map(|ob| ob.center).collect();
    let jl: Vec<JunctionFeat> = junc.values().copied().collect();
    (0..pr.n_layers)
        .map(|l| {
            let inp = SheetInput { proj: pr, outline, vias: &[], junctions: &jl, terminals: &terms };
            LayerSheet::build(&inp, l, &o.sheet)
        })
        .collect()
}

/// Shopping list 2.3 (P1b in P2 and the per-edge layer masks, `[capacity] model = "cuts"`): the
/// per-layer room of the projection edges that the shared projection leaves unbounded, from the
/// layer's sheet. Returns `(edge, layer, room)` for every edge-layer whose room changes.
///
/// The per-edge model (`state.rs` `Projection::build`) gives an edge the length between its
/// ends on a layer where both ends are copper or a wall, and *no bound* where an end is another
/// layer's obstacle, a free (Steiner) vertex or a node (design §3.2 item 1). Here such an end
/// does not bound the gap on layer `l`: the edge's line goes on past it to the first side of a
/// layer-`l` feature (pad, drilled wall, keepout, board edge; [`LayerSheet::ray_hit`]), so the
/// edge stands for the layer-`l` gap `p' q'` between the features `F1`, `F2` it meets. Its room is
/// the smaller of `|p' q'|` (Euclidean, as the per-edge room of two blocking ends) and the
/// capacity of every critical cut of the sheet that joins the same two features and crosses
/// `p' q'` (the same gap measured where it is narrowest: a wire through `p' q'` that does not end
/// inside the region between the two must cross such a cut too; Maley 1987 p. 40). A cut that
/// only touches an end of `p' q'` is not counted: it leaves sideways and bounds another gap. Where the gap
/// runs inside a keepout of the layer (a feature without a net), the layer is closed: room 0.
/// Edges with both ends blocking, along one obstacle, or ending at a pad centre (inside its own
/// pad) keep their room. A ray that meets nothing (no outline) keeps the edge unbounded.
///
/// O(edges x layers x (ray march + cuts in the gap's box)).
pub fn sheet_room(pr: &Projection, sheets: &[LayerSheet]) -> Vec<(usize, usize, f64)> {
    let mut out = Vec::new();
    let mut buf: Vec<u32> = Vec::new();
    for sh in sheets {
        let l = sh.l;
        let of_obs: HashMap<usize, usize> = sh.feats.iter().enumerate().filter_map(|(i, f)| if let FeatKind::Obstacle(o) = f.kind { Some((o, i)) } else { None }).collect();
        // Keepouts of the layer (features without a net): a gap inside one is closed.
        let keep: Vec<usize> = sh.feats.iter().enumerate().filter(|(_, f)| matches!(f.kind, FeatKind::Obstacle(o) if pr.obstacles[o].net.is_none() && !pr.obstacles[o].drilled)).map(|(i, _)| i).collect();
        let (mut lo, mut hi) = (Vec2 { x: f64::INFINITY, y: f64::INFINITY }, Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY });
        for &p in &pr.pos {
            lo = Vec2 { x: lo.x.min(p.x), y: lo.y.min(p.y) };
            hi = Vec2 { x: hi.x.max(p.x), y: hi.y.max(p.y) };
        }
        for (a, b) in &sh.outline {
            for p in [a, b] {
                lo = Vec2 { x: lo.x.min(p.x), y: lo.y.min(p.y) };
                hi = Vec2 { x: hi.x.max(p.x), y: hi.y.max(p.y) };
            }
        }
        let reach = (hi - lo).len() + 1.0;
        // Some(Some(f)): the end blocks on the layer as feature f; Some(None): it does not
        // block; None: a pad centre, or a blocking end without a feature (left alone).
        let feat_of = |v: VertId| -> Option<Option<usize>> {
            match pr.vkind[v] {
                VertKind::Outline(o) if pr.obstacles[o].drilled || pr.obstacles[o].layers.has(l) => of_obs.get(&o).map(|&f| Some(f)),
                VertKind::Edge => sh.edge_feat.map(Some),
                VertKind::Centre(_) => None,
                _ => Some(None),
            }
        };
        for (e, &[p, q]) in pr.edges.iter().enumerate() {
            if crate::control::cancelled() {
                return out; // cancel-fix: Cancel / OK during the projection; the room is not used
            }
            if !pr.room[e][l].is_infinite() {
                continue;
            }
            if matches!((pr.vkind[p], pr.vkind[q]), (VertKind::Outline(a), VertKind::Outline(b)) if a == b) {
                continue;
            }
            let (Some(fp), Some(fq)) = (feat_of(p), feat_of(q)) else { continue };
            let (pp, pq) = (pr.pos[p], pr.pos[q]);
            let len = (pq - pp).len();
            if len <= 0.0 {
                continue;
            }
            let dir = (pq - pp) * (1.0 / len);
            let mut end = |at: Vec2, f: Option<usize>, d: Vec2| -> Option<(Vec2, Option<usize>)> {
                match f {
                    Some(f) => Some((at, Some(f))),
                    None => sh.ray_hit(at, d, reach, &mut buf).map(|(t, f)| (at + d * t, Some(f))),
                }
            };
            let (Some((a, f1)), Some((b, f2))) = (end(pp, fp, dir * -1.0), end(pq, fq, dir)) else { continue };
            let mid = (a + b) * 0.5;
            let closed = keep.iter().any(|&k| point_in_poly(mid, &sh.feats[k].poly));
            let mut room = if closed { 0.0 } else { (b - a).len() };
            if let (Some(f1), Some(f2), false) = (f1, f2, closed) {
                if f1 != f2 {
                    let lo = Vec2 { x: a.x.min(b.x), y: a.y.min(b.y) };
                    let hi = Vec2 { x: a.x.max(b.x), y: a.y.max(b.y) };
                    sh.cut_grid.query(lo, hi, &mut buf);
                    for &ci in buf.iter() {
                        let c = &sh.cuts[ci as usize];
                        if ((c.fp, c.fq) == (f1, f2) || (c.fp, c.fq) == (f2, f1)) && cut_meets_gap(c.p, c.q, a, b) {
                            room = room.min(c.cap);
                        }
                    }
                }
            }
            out.push((e, l, room));
        }
    }
    out
}

/// Whether the cut `c d` meets the gap `a b` away from the gap's ends: it crosses it, or lies
/// along it (a cut that only touches an end of the gap leaves it sideways and bounds another
/// gap). 1 nm tolerance. O(1).
fn cut_meets_gap(c: Vec2, d: Vec2, a: Vec2, b: Vec2) -> bool {
    const TOL: f64 = 1.0;
    let u = b - a;
    let len = u.len();
    if len <= TOL {
        return false;
    }
    let v = d - c;
    let den = u.cross(v);
    let (dc, dd) = (u.cross(c - a) / len, u.cross(d - a) / len);
    if dc.abs() <= TOL && dd.abs() <= TOL {
        // Along the gap: an overlap longer than the tolerance.
        let (t1, t2) = ((c - a).dot(u) / len, (d - a).dot(u) / len);
        return t1.max(t2).min(len) - t1.min(t2).max(0.0) > TOL;
    }
    if den.abs() <= 1e-12 * len * v.len().max(1.0) {
        return false;
    }
    // a + t u = c + s v.
    let t = (c - a).cross(v) / den;
    let s = (c - a).cross(u) / den;
    let vl = v.len().max(1e-9);
    s >= -TOL / vl && s <= 1.0 + TOL / vl && t * len > TOL && (1.0 - t) * len > TOL
}

/// The static sheets of a projection without any wire (no junctions): the sheets of the P2 room
/// ([`sheet_room`]). O(layers x U0 sheet build).
pub fn bare_sheets(pr: &Projection, outline: &[(Vec2, Vec2)], o: &CutOpts) -> Result<Vec<LayerSheet>, String> {
    let terms: Vec<Vec2> = pr.obstacles.iter().filter(|ob| ob.net.is_some()).map(|ob| ob.center).collect();
    (0..pr.n_layers)
        .take_while(|_| !crate::control::cancelled()) // cancel-fix: the sheets are not used then
        .map(|l| {
            let inp = SheetInput { proj: pr, outline, vias: &[], junctions: &[], terminals: &terms };
            LayerSheet::build(&inp, l, &o.sheet)
        })
        .collect()
}

/// Junction points of the alive wires (node ends, ends without an obstacle, turns), by vertex.
fn junctions(st: &State) -> BTreeMap<VertId, JunctionFeat> {
    let pr = st.proj;
    let mut junc: BTreeMap<VertId, JunctionFeat> = BTreeMap::new();
    for wi in st.wires.iter().filter(|w| w.alive) {
        for (v, ob) in [(wi.a, wi.pa), (wi.b, wi.pb)] {
            if ob == NO_OBS || matches!(pr.vkind[v], VertKind::Node) {
                junc.entry(v).or_insert(JunctionFeat { at: pr.pos[v], net: wi.net, clear: wi.s });
            }
        }
        for &(_, v) in &wi.turns {
            junc.entry(v).or_insert(JunctionFeat { at: pr.pos[v], net: wi.net, clear: wi.s });
        }
    }
    junc
}

/// The cut-model context of a packer (`[capacity] model = "cuts"`): options, the outline and a
/// cache of the static sheets keyed by the junction points they hold (the sheets change only
/// when a reroute moves a junction). Single-threaded, like the packer's other caches.
pub struct CutCtx {
    pub o: CutOpts,
    /// `[capacity] cut_vias`: the layer refinement may add vias to remove cut overflow.
    pub add_vias: bool,
    outline: Vec<(Vec2, Vec2)>,
    cache: RefCell<SheetCache>,
    /// Sheet builds and cache hits (report).
    pub builds: RefCell<(usize, usize)>,
    /// Sheet builds that failed (the capacity model of that plan fell back to the per-edge one).
    pub problems: RefCell<Vec<String>>,
}

impl CutCtx {
    pub fn new(o: CutOpts, add_vias: bool, outline: Vec<(Vec2, Vec2)>) -> CutCtx {
        CutCtx { o, add_vias, outline, cache: RefCell::new(None), builds: RefCell::new((0, 0)), problems: RefCell::new(Vec::new()) }
    }

    /// The static sheets of `st` (cached while its junction points stay).
    pub fn sheets(&self, st: &State) -> Result<Rc<Vec<LayerSheet>>, String> {
        let k: Vec<(i64, i64, i32)> = junctions(st).values().map(|j| (j.at.x.round() as i64, j.at.y.round() as i64, j.net)).collect();
        if let Some((ck, sh)) = self.cache.borrow().as_ref() {
            if *ck == k && sh.len() == st.proj.n_layers {
                self.builds.borrow_mut().1 += 1;
                return Ok(sh.clone());
            }
        }
        let sh = Rc::new(static_sheets(st, &self.outline, &self.o)?);
        self.builds.borrow_mut().0 += 1;
        *self.cache.borrow_mut() = Some((k, sh.clone()));
        Ok(sh)
    }
}

/// State of one cut in [`CutPrice`].
#[derive(Clone, Debug, Default)]
struct CutSt {
    items: Vec<Itm>,
    cap: f64,
    /// Need at the nominal widths and at the neck floors (2.4).
    need: f64,
    need_floor: f64,
    /// `cap - need_floor`: over-full (a violation) only beyond the floors.
    slack: f64,
    /// The width reduction below the nominal widths the cut needs ([`shrink_need`], nm) and the
    /// pitch of its items.
    red: f64,
    pitch: f64,
    /// `price x phi(slack) + shrink_mm x red / pitch`.
    cost: f64,
    /// Whether it held items when it was last priced (`slack` and `cost` are of that state; the
    /// items may have changed since, until the next pricing).
    live: bool,
    /// The id of the last move that recorded its slack ([`Txn::slack`]; 0 = none).
    txn: u32,
}

/// What one run or one via of a wire contributes: `(layer, cut, item)` and the junction ends
/// `(layer, feature, half width)`, under the id `src` its items carry.
#[derive(Clone, Debug, Default)]
struct Part {
    src: u32,
    items: Vec<(usize, usize, Itm)>,
    jends: Vec<(usize, usize, f64)>,
}

/// What a run's items depend on (besides the static sheet): equal keys give equal items.
#[derive(Clone, Debug, PartialEq)]
struct RunKey {
    layer: usize,
    net: i32,
    s: f64,
    w: f64,
    w_floor: f64,
    curve: Vec<Vec2>,
    curve_k: Vec<usize>,
    pass_w: Vec<f64>,
    via_ends: [bool; 2],
    /// The wire's via radius (a crossing next to an own via end is the via's, [`flow_items`]).
    via_r: f64,
}

impl RunKey {
    fn of(s: &Stretch, via_r: f64) -> RunKey {
        RunKey { layer: s.layer, net: s.net, s: s.s, w: s.w, w_floor: s.w_floor, curve: s.curve.clone(), curve_k: s.curve_k.clone(), pass_w: s.pass_w.clone(), via_ends: s.via_ends, via_r }
    }
}

/// What one via's items depend on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ViaKey {
    at: Vec2,
    r: f64,
    s: f64,
    net: i32,
    k: usize,
}

/// What one wire contributes: its runs and its vias, each a [`Part`] with its key.
#[derive(Clone, Debug, Default)]
struct WireSt {
    /// Shared, so the undo copy of a wire and the reuse of its parts are pointer copies.
    runs: Vec<Rc<(RunKey, Part)>>,
    vias: Vec<Rc<(ViaKey, Part)>>,
}

impl WireSt {
    fn parts(&self) -> impl Iterator<Item = &Part> {
        self.runs.iter().map(|x| &x.1).chain(self.vias.iter().map(|x| &x.1))
    }
}

/// An open move (the first state of every wire and cut it touched).
#[derive(Default)]
struct Txn {
    wires: BTreeMap<WireId, WireSt>,
    /// The slack of every cut priced since [`CutPrice::begin`] as it was before (first touch,
    /// [`CutSt::txn`]).
    slack: Vec<(usize, usize, f64)>,
    /// Its id ([`CutSt::txn`]).
    id: u32,
    total: f64,
    over: usize,
}

/// The cut model kept per wire for the planner (P3 / P4 label moves): the total over-full price
/// `sum price x phi` and the over-full count, with per-wire updates and an undo.
pub struct CutPrice<'a> {
    pr: &'a Projection,
    pub sheets: Rc<Vec<LayerSheet>>,
    o: CutOpts,
    price: f64,
    jf: Vec<HashMap<(i64, i64), usize>>,
    /// Per layer: junction feature -> its sheet vertex.
    jv: Vec<HashMap<usize, usize>>,
    regime: Vec<Vec<Option<Option<f64>>>>,
    cuts: Vec<Vec<CutSt>>,
    wires: Vec<WireSt>,
    /// Per layer: junction feature -> (wire, part, half width) of the runs ending there.
    jhalf: Vec<JHalf>,
    /// Next part id.
    next_src: u32,
    /// Shopping list 3.1: an edit reprices only the runs and vias that changed (true, the
    /// default); false = the whole wire every time (the measurement's reference, debug
    /// `TOPO_CUT_FULL=1`). Both give the same prices.
    pub incremental: bool,
    /// Runs and vias whose items were computed / reused (report).
    pub parts: (usize, usize),
    /// Cuts priced since the last install (added to the process-wide meter once per install).
    priced: u64,
    /// Touched cuts whose pricing was skipped (same key sequence, [`CutPrice::install`]; report).
    pub skipped: usize,
    total: f64,
    over: usize,
    txn: Option<Txn>,
    /// Id of the last move ([`Txn::id`]).
    txn_id: u32,
    buf: Vec<u32>,
    /// A cut's item list being rebuilt ([`CutPrice::install`]).
    scratch: Vec<Itm>,
    /// The walk's scratch ([`flow_items_into`]).
    seg: SegScratch,
    flow: Vec<(usize, FlowItem)>,
    /// Via site points per (triangle, net, r, s) ([`super::cutflow::via_point`]).
    sites: RefCell<SiteCache>,
}

/// One row of [`CutPrice::report`].
#[derive(Clone, Debug)]
pub struct CutRow {
    pub layer: usize,
    pub p: Vec2,
    pub q: Vec2,
    pub cap: f64,
    /// Need at the nominal widths and at the neck floors; `slack = cap - need_floor` (2.4: over-full
    /// only beyond the floors); `shrink` the width reduction the cut needs (nm, [`shrink_need`]).
    pub need: f64,
    pub need_floor: f64,
    pub slack: f64,
    pub shrink: f64,
    pub wires: Vec<WireId>,
}

impl<'a> CutPrice<'a> {
    pub fn new(pr: &'a Projection, sheets: Rc<Vec<LayerSheet>>, o: CutOpts, price: f64, n_wires: usize) -> Self {
        let cuts = sheets.iter().map(|s| vec![CutSt::default(); s.cuts.len()]).collect();
        let regime = sheets.iter().map(|s| vec![None; s.cuts.len()]).collect();
        let jf = sheets.iter().map(junction_feats).collect();
        let jv = sheets
            .iter()
            .map(|sh| (0..sh.pos.len()).filter_map(|v| sh.vfeat[v].filter(|&f| matches!(sh.feats[f].kind, FeatKind::Junction(_)) && sh.feats[f].poly[0] == sh.pos[v]).map(|f| (f, v))).collect())
            .collect();
        let nl = sheets.len();
        CutPrice {
            pr,
            sheets,
            o,
            price,
            jf,
            jv,
            regime,
            cuts,
            wires: vec![WireSt::default(); n_wires],
            jhalf: vec![BTreeMap::new(); nl],
            next_src: 1,
            incremental: std::env::var("TOPO_CUT_FULL").is_err(),
            parts: (0, 0),
            priced: 0,
            skipped: 0,
            total: 0.0,
            over: 0,
            txn: None,
            txn_id: 0,
            buf: Vec::new(),
            scratch: Vec::new(),
            seg: SegScratch::default(),
            flow: Vec::new(),
            sites: RefCell::new(HashMap::new()),
        }
    }

    /// The runs and vias of wire `w` under events `ev` and via traversals `ks`.
    pub fn runs_of(&self, st: &State, packer: &Packer, w: WireId, ev: &[LEv], ks: &[usize], bottom: i32) -> (Vec<Stretch>, Vec<ViaItem>) {
        let site = |t: usize, net: i32, r: f64, s: f64| -> (Vec2, bool) {
            let key = (t, net, r as i64, s as i64);
            if let Some(&x) = self.sites.borrow().get(&key) {
                return x;
            }
            let x = super::cutflow::via_point(st.proj, packer, t, net, r, s);
            self.sites.borrow_mut().insert(key, x);
            x
        };
        let r = wire_stretches(st, packer, w, ev, ks, bottom, None, &site);
        let n = st.wires[w].passes.len();
        let mut ks2: Vec<usize> = ks.to_vec();
        let iv = super::refine::via_ranges(ev, packer.via_between).0;
        ks2.truncate(iv.len());
        for &(ka, kb) in &iv[ks2.len()..] {
            ks2.push(super::refine::mid_k(ka, kb));
        }
        let vias = r.vias.iter().zip(ks2.iter()).map(|((v, _), &k)| ViaItem { wire: w, at: v.at, r: v.r, s: v.clear, net: v.net, k: k.min(n) }).collect();
        (r.runs, vias)
    }

    fn regime(&mut self, l: usize, ci: usize) -> Option<f64> {
        if let Some(r) = self.regime[l][ci] {
            return r;
        }
        let c = &self.sheets[l].cuts[ci];
        let r = self.pr.regime_at((c.p + c.q) * 0.5);
        self.regime[l][ci] = Some(r);
        r
    }

    fn reprice(&mut self, l: usize, ci: usize) {
        self.priced += 1;
        let regime = self.regime(l, ci);
        let sh = self.sheets.clone();
        let jh = &self.jhalf[l];
        let half = |f: usize| jh.get(&f).filter(|v| !v.is_empty()).map(|v| v.iter().map(|x| x.2).fold(0.0, f64::max));
        let cs = &mut self.cuts[l][ci];
        // The state as last priced (the items have changed since).
        let old = (cs.cost, cs.live && cs.slack < -OVER_TOL);
        if let Some(t) = self.txn.as_mut() {
            if cs.txn != t.id {
                cs.txn = t.id;
                t.slack.push((l, ci, if cs.live { cs.slack } else { f64::INFINITY }));
            }
        }
        if cs.items.is_empty() {
            *cs = CutSt { txn: cs.txn, ..CutSt::default() };
        } else {
            let (cap, need, floor) = evaluate_need(&sh[l], ci, &mut cs.items, regime, &half, &self.o);
            let pitch = pitch_of(&cs.items);
            cs.cap = cap;
            cs.need = need;
            cs.need_floor = floor;
            cs.slack = cap - floor;
            cs.red = shrink_need(cap, need, floor);
            cs.pitch = pitch;
            cs.cost = self.price * phi(cs.slack, pitch, self.o.tau);
            if cs.red > 0.0 {
                cs.cost += self.o.shrink_mm * cs.red / pitch.max(1.0);
            }
            cs.live = true;
        }
        let new = (cs.cost, cs.live && cs.slack < -OVER_TOL);
        self.total += new.0 - old.0;
        self.over = (self.over as isize + new.1 as isize - old.1 as isize) as usize;
    }

    /// Put wire `w`'s contribution `ws` in place of its current one: the parts (runs, vias) of
    /// the old one that `ws` does not hold leave their cuts, the new ones enter, the parts both
    /// hold stay. Returns the change of the total.
    ///
    /// Shopping list 3.1: a cut's price depends on its items only through their order along the
    /// cut and their (net, width, clearance, floor, kind) (`need` is a function of that sequence,
    /// the pitch a sum over it), not on their positions `t` or traversals `k`. An edit moves most
    /// of a re-pulled band's crossings along their cuts without passing another item (Dayan 1997
    /// p. 68: a rubber-band edit changes the order only where it sweeps over other wires), so a
    /// cut whose sorted sequence of those keys is unchanged and whose ends are untouched keeps its
    /// price exactly: its items are replaced, the pricing is skipped. Every other touched cut is
    /// priced again, in ascending (layer, cut) order as before, so the totals are the same floats
    /// as a full recompute's. O(items of the changed parts + their cuts' items x log).
    fn install(&mut self, w: WireId, ws: WireSt) -> f64 {
        let t0 = self.total;
        if w >= self.wires.len() {
            self.wires.resize(w + 1, WireSt::default());
        }
        let old = std::mem::replace(&mut self.wires[w], ws);
        if let Some(t) = self.txn.as_mut() {
            t.wires.entry(w).or_insert_with(|| old.clone());
        }
        let new = self.wires[w].clone();
        let new_src: Vec<u32> = new.parts().map(|p| p.src).collect();
        let old_src: Vec<u32> = old.parts().map(|p| p.src).collect();
        let removed: Vec<&Part> = old.parts().filter(|p| !new_src.contains(&p.src)).collect();
        let added: Vec<&Part> = new.parts().filter(|p| !old_src.contains(&p.src)).collect();
        let gone: Vec<u32> = removed.iter().map(|p| p.src).collect();
        // Junction ends: their half widths change the capacity of every cut at the junction.
        let mut jdirty: Vec<(usize, usize)> = Vec::new();
        for part in &removed {
            for &(l, f, _) in &part.jends {
                if let Some(v) = self.jhalf[l].get_mut(&f) {
                    v.retain(|x| !(x.0 == w && x.1 == part.src));
                }
                jdirty.extend(self.junction_cuts(l, f));
            }
        }
        for part in &added {
            for &(l, f, h) in &part.jends {
                self.jhalf[l].entry(f).or_default().push((w, part.src, h));
                jdirty.extend(self.junction_cuts(l, f));
            }
        }
        jdirty.sort_unstable();
        jdirty.dedup();
        // The cuts the removed parts leave (each part's items come by ascending cut, so the
        // stable sort merges presorted runs).
        let mut rems: Vec<(usize, usize)> = removed.iter().flat_map(|p| p.items.iter().map(|x| (x.0, x.1))).collect();
        rems.sort();
        rems.dedup();
        // Added items by cut, in the order they are pushed (part, then item).
        let mut adds: Vec<(usize, usize, Itm)> = added.iter().flat_map(|p| p.items.iter().copied()).collect();
        adds.sort_by_key(|x| (x.0, x.1));
        let mut scratch = std::mem::take(&mut self.scratch);
        let (mut ri, mut ai, mut ji) = (0usize, 0usize, 0usize);
        loop {
            // The next touched cut: the least of the three sorted lists' heads.
            let next = [rems.get(ri).copied(), adds.get(ai).map(|x| (x.0, x.1)), jdirty.get(ji).copied()].into_iter().flatten().min();
            let Some((l, ci)) = next else { break };
            let rem = rems.get(ri) == Some(&(l, ci));
            if rem {
                ri += 1;
            }
            let jd = jdirty.get(ji) == Some(&(l, ci));
            if jd {
                ji += 1;
            }
            let a0 = ai;
            while adds.get(ai).is_some_and(|x| (x.0, x.1) == (l, ci)) {
                ai += 1;
            }
            let add = &adds[a0..ai];
            if !rem && add.is_empty() {
                self.reprice(l, ci);
                continue;
            }
            let cs = &mut self.cuts[l][ci];
            // Fast path: one item of the wire replaced by one that stays in its place (strictly
            // between its neighbours, so the stable sort of the general path puts it there too).
            if rem && add.len() == 1 && cs.live {
                let mut mine = cs.items.iter().enumerate().filter(|(_, x)| x.wire == w && gone.contains(&x.src)).map(|(i, _)| i);
                if let (Some(p), None) = (mine.next(), mine.next()) {
                    let n = add[0].2;
                    let lt = |a: &Itm, b: &Itm| item_order(a, b) == std::cmp::Ordering::Less;
                    if (p == 0 || lt(&cs.items[p - 1], &n)) && (p + 1 == cs.items.len() || lt(&n, &cs.items[p + 1])) {
                        let same = !jd && same_keys(std::slice::from_ref(&cs.items[p]), std::slice::from_ref(&n));
                        cs.items[p] = n;
                        if same {
                            self.skipped += 1;
                        } else {
                            self.reprice(l, ci);
                        }
                        continue;
                    }
                }
            }
            scratch.clear();
            if rem {
                scratch.extend(cs.items.iter().copied().filter(|x| !(x.wire == w && gone.contains(&x.src))));
            } else {
                scratch.extend_from_slice(&cs.items);
            }
            scratch.extend(add.iter().map(|x| x.2));
            scratch.sort_by(item_order);
            let same = cs.live && !jd && same_keys(&scratch, &cs.items);
            std::mem::swap(&mut cs.items, &mut scratch);
            if same {
                self.skipped += 1;
            } else {
                self.reprice(l, ci);
            }
        }
        self.scratch = scratch;
        let priced = std::mem::take(&mut self.priced);
        if super::refine::meter::on() {
            super::refine::meter::CUTS_PRICED.fetch_add(priced, std::sync::atomic::Ordering::Relaxed);
        }
        self.total - t0
    }

    fn junction_cuts(&self, l: usize, f: usize) -> Vec<(usize, usize)> {
        let Some(&v) = self.jv[l].get(&f) else { return Vec::new() };
        self.sheets[l].vcuts[v].iter().map(|&ci| (l, ci)).collect()
    }

    /// The items of one run of wire `w` (its band is pulled here) under the id `src`.
    fn run_part(&mut self, run: &mut Stretch, via_r: f64, src: u32) -> Part {
        let t0 = std::time::Instant::now();
        let part = self.run_part_in(run, via_r, src);
        super::refine::meter::add(&super::refine::meter::PART_NS, &super::refine::meter::PARTS, t0);
        part
    }

    fn run_part_in(&mut self, run: &mut Stretch, via_r: f64, src: u32) -> Part {
        let l = run.layer;
        let mut part = Part { src, ..Part::default() };
        let sheets = self.sheets.clone();
        let Some(sh) = sheets.get(l) else { return part };
        if let Ok((b, wr)) = band(sh, &run.curve) {
            run.band = b;
            run.wraps = wr;
        }
        let (items, jends) = run_items_with(sh, &self.jf[l], std::slice::from_ref(run), via_r, &mut self.seg, &mut self.flow);
        part.items = items.into_iter().map(|(ci, mut it)| {
            it.src = src;
            (l, ci, it)
        }).collect();
        part.jends = jends.into_iter().map(|(f, h)| (l, f, h)).collect();
        part
    }

    /// The items of one via on every layer under the id `src`.
    fn via_part(&mut self, v: &ViaItem, src: u32) -> Part {
        let t0 = std::time::Instant::now();
        let part = self.via_part_in(v, src);
        super::refine::meter::add(&super::refine::meter::PART_NS, &super::refine::meter::PARTS, t0);
        part
    }

    fn via_part_in(&mut self, v: &ViaItem, src: u32) -> Part {
        let mut part = Part { src, ..Part::default() };
        let sheets = self.sheets.clone();
        for (l, sh) in sheets.iter().enumerate() {
            for (ci, t, wv) in via_hits(sh, v, &mut self.buf) {
                part.items.push((l, ci, Itm { wire: v.wire, t, kind: 3, it: Item { net: v.net, w: wv, s: v.s }, floor: wv, nom: wv, k: v.k, src }));
            }
        }
        part
    }

    /// Set wire `w` to the runs `runs` (bands are pulled here) and the vias `vias`; returns the
    /// change of the total price. Incremental (shopping list 3.1): a run or via equal to one the
    /// wire has now (same layer, curve, widths, ends; same point and size) keeps its items, so an
    /// edit costs the walks of the runs it changes, not of the whole wire. A run whose walk
    /// fails adds nothing.
    pub fn set_wire(&mut self, w: WireId, runs: &mut [Stretch], vias: &[ViaItem]) -> f64 {
        let via_r = vias.iter().map(|v| v.r).fold(0.0, f64::max);
        let mut old = if self.incremental { self.wires.get(w).cloned().unwrap_or_default() } else { WireSt::default() };
        let mut ws = WireSt::default();
        for run in runs.iter_mut() {
            let key = RunKey::of(run, via_r);
            if let Some(j) = old.runs.iter().position(|x| x.0 == key) {
                ws.runs.push(old.runs.swap_remove(j));
                self.parts.1 += 1;
                continue;
            }
            let src = self.next_src;
            self.next_src += 1;
            let part = self.run_part(run, via_r, src);
            self.parts.0 += 1;
            ws.runs.push(Rc::new((key, part)));
        }
        for v in vias {
            let key = ViaKey { at: v.at, r: v.r, s: v.s, net: v.net, k: v.k };
            if let Some(j) = old.vias.iter().position(|x| x.0 == key) {
                ws.vias.push(old.vias.swap_remove(j));
                self.parts.1 += 1;
                continue;
            }
            let src = self.next_src;
            self.next_src += 1;
            let part = self.via_part(v, src);
            self.parts.0 += 1;
            ws.vias.push(Rc::new((key, part)));
        }
        let t0 = std::time::Instant::now();
        let d = self.install(w, ws);
        if super::refine::meter::on() {
            super::refine::meter::INSTALL_NS.fetch_add(t0.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        }
        d
    }

    /// Take wire `w` out (a dead wire); returns the change of the total.
    pub fn clear_wire(&mut self, w: WireId) -> f64 {
        self.install(w, WireSt::default())
    }

    /// Open a move: every later change can be undone by [`CutPrice::rollback`].
    pub fn begin(&mut self) {
        self.txn_id = self.txn_id.wrapping_add(1).max(1);
        if self.txn_id == 1 {
            // Ids wrapped (or the first move): no cut may carry a stale id.
            self.cuts.iter_mut().flatten().for_each(|c| c.txn = 0);
        }
        self.txn = Some(Txn { total: self.total, over: self.over, id: self.txn_id, ..Txn::default() });
    }

    /// Keep the changes since [`CutPrice::begin`].
    pub fn commit(&mut self) {
        self.txn = None;
    }

    /// Undo the changes since [`CutPrice::begin`].
    pub fn rollback(&mut self) {
        let Some(t) = self.txn.take() else { return };
        for (w, ws) in t.wires {
            self.install(w, ws);
        }
        // The prices are recomputed from the same items, so they come back exactly; the sums are
        // restored to avoid float drift.
        self.total = t.total;
        self.over = t.over;
    }

    /// Whether some cut touched since [`CutPrice::begin`] is over-full and more over-full than
    /// before (the strict test of the P4 label moves).
    pub fn worse_somewhere(&self) -> bool {
        let Some(t) = self.txn.as_ref() else { return false };
        t.slack.iter().any(|&(l, ci, s0)| {
            let c = &self.cuts[l][ci];
            !c.items.is_empty() && c.slack < -OVER_TOL && c.slack < s0 - 1.0
        })
    }

    /// Total over-full price (mm).
    pub fn total(&self) -> f64 {
        self.total
    }

    /// Over-full cuts (slack below [`OVER_TOL`]).
    pub fn overfull(&self) -> usize {
        self.over
    }

    /// The graded amount of the over-full cuts (the TopoR loop's predicted violations, shopping
    /// list 5.2): every cut [`CutPrice::overfull`] counts gives 1 plus how far it is over, in
    /// pitches of its items (`-slack / pitch`, the `u` of [`phi`]): the `violation` factor's own
    /// amount, 1 per violation plus its depth. At least [`CutPrice::overfull`]. The slack is the
    /// one beyond the neck floors (2.4): a cut that fits necked is a shrinkage, no violation.
    /// O(cuts).
    pub fn over_amount(&self) -> f64 {
        let mut a = 0.0;
        for cs in &self.cuts {
            for c in cs.iter().filter(|c| c.live && !c.items.is_empty() && c.slack < -OVER_TOL) {
                a += 1.0 + -c.slack / pitch_of(&c.items).max(1.0);
            }
        }
        a
    }

    /// Shopping list 2.4, the shrinkage level of the price: the cuts that fit only necked (or not
    /// even at the floors: their whole necking), `(count, sum of red / pitch, price)` with `red`
    /// the width reduction below the nominal widths ([`shrink_need`]) and the price `shrink_mm x
    /// red / pitch` per cut (part of [`CutPrice::total`]). O(cuts).
    pub fn shrinkage(&self) -> (usize, f64, f64) {
        let (mut n, mut a) = (0usize, 0.0);
        for cs in &self.cuts {
            for c in cs.iter().filter(|c| c.live && !c.items.is_empty() && c.red > 0.0) {
                n += usize::from(c.red > OVER_TOL);
                a += c.red / c.pitch.max(1.0);
            }
        }
        (n, a, self.o.shrink_mm * a)
    }

    /// The cut-level necks (2.4): for every cut that needs a width reduction `red` below its
    /// nominal need `N` (its floor need `F`), every neckable run item of it narrowed by the same
    /// share `alpha = red / (N - F)` of its own range, `w = nom - alpha (nom - floor)`: the widest
    /// common share that fits (need is linear in the widths except the own-net end run's maximum,
    /// which is convex, so the need at these widths is at most `cap`; Dayan 1997 pp. 15-16: a cut
    /// holds the widths of the wires that cross it). Beyond the floors `alpha = 1` (the floor;
    /// the rest is the violation). `(wire, traversal, width)`, sorted, one per item. O(items of
    /// the shrinking cuts).
    pub fn necks(&self) -> Vec<(WireId, usize, f64)> {
        let mut out = Vec::new();
        for cs in &self.cuts {
            for c in cs.iter().filter(|c| c.live && !c.items.is_empty() && c.red > 0.0) {
                let span = c.need - c.need_floor;
                if span <= 0.0 {
                    continue;
                }
                let alpha = (c.red / span).clamp(0.0, 1.0);
                for x in c.items.iter().filter(|x| x.kind != 3 && x.floor < x.nom) {
                    out.push((x.wire, x.k, x.nom - alpha * (x.nom - x.floor)));
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.total_cmp(&b.2)));
        out
    }

    /// The over-full cuts `(layer, cut, slack)`, most over-full first, ties by layer and cut.
    pub fn overfull_list(&self) -> Vec<(usize, usize, f64)> {
        let mut v: Vec<(usize, usize, f64)> = Vec::new();
        for (l, cs) in self.cuts.iter().enumerate() {
            for (ci, c) in cs.iter().enumerate() {
                if !c.items.is_empty() && c.slack < -OVER_TOL {
                    v.push((l, ci, c.slack));
                }
            }
        }
        v.sort_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
        v
    }

    /// Whether cut `ci` of layer `l` is over-full.
    pub fn is_over(&self, l: usize, ci: usize) -> bool {
        let c = &self.cuts[l][ci];
        !c.items.is_empty() && c.slack < -OVER_TOL
    }

    /// Whether cut `ci` of layer `l` is not over-full with the wires `gone` taken out (the P4
    /// rip-up's stop test, the per-edge `fits_without` of the cut model). O(items log items).
    pub fn fits_without(&self, l: usize, ci: usize, gone: &BTreeSet<WireId>) -> bool {
        let c = &self.cuts[l][ci];
        let mut items: Vec<Itm> = c.items.iter().copied().filter(|x| !gone.contains(&x.wire)).collect();
        if items.is_empty() {
            return true;
        }
        let jh = &self.jhalf[l];
        let half = |f: usize| {
            jh.get(&f).map(|v| v.iter().filter(|x| !gone.contains(&x.0)).map(|x| x.2).fold(f64::NEG_INFINITY, f64::max)).filter(|h| h.is_finite())
        };
        let regime = self.regime[l][ci].unwrap_or_else(|| {
            let cut = &self.sheets[l].cuts[ci];
            self.pr.regime_at((cut.p + cut.q) * 0.5)
        });
        let (cap, _, floor) = evaluate_need(&self.sheets[l], ci, &mut items, regime, &half, &self.o);
        cap - floor >= -OVER_TOL
    }

    /// The items of cut `ci` on layer `l` as last installed (sorted along the cut).
    pub fn cut_items(&self, l: usize, ci: usize) -> &[Itm] {
        &self.cuts[l][ci].items
    }

    /// The (wire, traversal) of every item of cut `ci` on layer `l` (vias and runs).
    pub fn items_at(&self, l: usize, ci: usize) -> Vec<(WireId, usize)> {
        self.cuts[l][ci].items.iter().map(|x| (x.wire, x.k)).collect()
    }

    /// Every cut that carries items: `(layer, p, q, capacity, need, slack, wires)` (nm, engine
    /// frame), most over-full first (the plan report and the review pictures).
    pub fn report(&self) -> Vec<CutRow> {
        let mut v: Vec<CutRow> = Vec::new();
        for (l, cs) in self.cuts.iter().enumerate() {
            for (ci, c) in cs.iter().enumerate().filter(|(_, c)| !c.items.is_empty()) {
                let cut = &self.sheets[l].cuts[ci];
                let mut ws: Vec<WireId> = c.items.iter().map(|x| x.wire).collect();
                ws.sort_unstable();
                ws.dedup();
                v.push(CutRow { layer: l, p: cut.p, q: cut.q, cap: c.cap, need: c.need, need_floor: c.need_floor, slack: c.slack, shrink: c.red, wires: ws });
            }
        }
        v.sort_by(|a, b| a.slack.total_cmp(&b.slack).then(a.layer.cmp(&b.layer)));
        v
    }

    /// Capacity, nominal need and slack (beyond the floors) of a cut (nm).
    pub fn cut_state(&self, l: usize, ci: usize) -> (f64, f64, f64) {
        let c = &self.cuts[l][ci];
        (c.cap, c.need, c.slack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phi_is_smooth_graded_and_matches_the_edge_price_far_out() {
        let tau = 0.1;
        let p = 450_000.0;
        // Far over-full: 1 + u (the per-edge price of refine.rs).
        let u = 2.0;
        assert!((phi(-u * p, p, tau) - (1.0 + u)).abs() < 1e-6);
        // At zero slack about 0.57; slack of a pitch: almost free; increasing everywhere.
        assert!((phi(0.0, p, tau) - (0.5 + tau * 2f64.ln())).abs() < 1e-12);
        assert!(phi(p, p, tau) < 1e-3);
        let mut last = 0.0;
        for i in -200..=200 {
            let v = phi(-(i as f64) * 0.01 * p, p, tau);
            assert!(v > last || v == 0.0, "not increasing at {i}");
            last = v;
        }
        // No step at the threshold: the price changes continuously across slack 0.
        assert!((phi(1.0, p, tau) - phi(-1.0, p, tau)).abs() < 1e-4);
    }

    #[test]
    fn a_via_blocks_its_diameter_on_the_cut_and_enters_without_a_jump() {
        let v = |x: f64, y: f64| Vec2 { x, y };
        let (p, q) = (v(0.0, 0.0), v(2_000_000.0, 0.0));
        let (r, s) = (300_000.0, 200_000.0);
        // Centred on the cut: the disc's diameter.
        let (t, w) = via_block(p, q, v(1_000_000.0, 0.0), r, s).expect("blocks");
        assert!((t - 0.5).abs() < 1e-12 && (w - 2.0 * r).abs() < 1e-6, "{w}");
        // Off the cut by d: L = 2 sqrt((r + s)^2 - d^2); it enters at L = s (w = -s: the two
        // gaps of the via equal the one gap it splits), so there is no jump.
        let d_in = ((r + s).powi(2) - (0.5 * s).powi(2)).sqrt();
        assert!(via_block(p, q, v(1_000_000.0, d_in + 1.0), r, s).is_none());
        let (_, w_in) = via_block(p, q, v(1_000_000.0, d_in - 1.0), r, s).expect("just in");
        assert!((w_in + s).abs() < 2_000.0, "{w_in}");
        // Beyond the cut's ends: none.
        assert!(via_block(p, q, v(-100_000.0, 0.0), r, s).is_none());
    }
}
