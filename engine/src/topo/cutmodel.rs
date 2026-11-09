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
//!   capacity - need.
//!
//! **Price** (design §4.4): with `u = -slack / pitch` (overflow in track pitches, `pitch` = mean
//! width + clearance of the cut's items) and the softness `tau` (`[capacity] overflow_softness`),
//! `phi(u) = logistic(u / tau) + tau ln(1 + e^(u / tau))`; a cut costs `overflow_mm x phi`. For
//! `u >> tau` this is `1 + u`, the per-edge price `overflow_mm (1 + over / pitch)` of
//! `refine.rs`, so the weights carry over; at `u = 0` it is about 0.57; for `u << 0` it goes to 0
//! smoothly. No step, no cutoff (graded factors): a nearly full cut already costs a little.
//!
//! Complexity: building the sheets is U0's; a wire costs a walk and a funnel per run plus a grid
//! query per band segment and per via; repricing a cut is O(items log items).

use super::cutflow::{band, flow_items, needs, wire_stretches, Stretch};
use super::demand::{Bound, Item};
use super::pack::Packer;
use super::refine::LEv;
use super::sheet::{FeatKind, JunctionFeat, LayerSheet, Norm, SheetInput, SheetOpts};
use super::state::{Projection, State, VertId, VertKind, WireId, NO_OBS};
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
}

impl CutOpts {
    pub fn from_options(o: &crate::model::Options) -> CutOpts {
        CutOpts {
            sheet: SheetOpts { norm: Norm::for_style(&o.draw_style), radius_nm: o.cut_radius_mm.max(0.0) * 1e6, via_steps: 16, prune_walk: true },
            pairwise: o.mixed_clearance == "pairwise",
            tau: o.overflow_softness.max(1e-3),
        }
    }
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
    /// Traversal of the wire nearest to the item (the place a layer move acts on).
    pub k: usize,
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

/// The items one wire adds to a sheet: its runs on the sheet's layer (`runs` with bands) and its
/// vias. `(cut, item)`, and the junction features its runs end at with the half width.
fn wire_items(sh: &LayerSheet, jf: &HashMap<(i64, i64), usize>, runs: &[Stretch], vias: &[ViaItem], buf: &mut Vec<u32>) -> WireItems {
    let mine: Vec<usize> = (0..runs.len()).filter(|&i| runs[i].layer == sh.l).collect();
    let via_r = vias.iter().map(|v| v.r).fold(0.0, f64::max);
    let per = flow_items(sh, runs, &mine, via_r);
    let mut out = Vec::new();
    for (ci, items) in per {
        let c = &sh.cuts[ci];
        for x in items {
            let s = &runs[x.stretch];
            let at = match x.kind {
                1 => c.p,
                2 => c.q,
                _ => c.p + (c.q - c.p) * x.t,
            };
            out.push((ci, Itm { wire: s.wire, t: x.t, kind: x.kind, it: Item { net: s.net, w: x.w, s: s.s }, floor: s.w_floor, k: nearest_k(s, at) }));
        }
    }
    for v in vias {
        for (ci, t, w) in via_hits(sh, v, buf) {
            out.push((ci, Itm { wire: v.wire, t, kind: 3, it: Item { net: v.net, w, s: v.s }, floor: w, k: v.k }));
        }
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

/// Need, slack and price of a cut with its items sorted. `jhalf(f)`: the half width carried by a
/// junction feature on this layer (`None`: no run ends there, the end is open).
fn evaluate(sh: &LayerSheet, ci: usize, items: &mut [Itm], regime: Option<f64>, jhalf: &dyn Fn(usize) -> Option<f64>, o: &CutOpts) -> (f64, Needs3) {
    items.sort_by(|a, b| a.t.total_cmp(&b.t).then(a.wire.cmp(&b.wire)).then(a.kind.cmp(&b.kind)).then(a.k.cmp(&b.k)));
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
    let ((bp, hp), (bq, hq)) = (end(c.fp), end(c.fq));
    let cap = c.cap - hp - hq;
    let its: Vec<Item> = items.iter().map(|x| x.it).collect();
    let floors: Vec<f64> = items.iter().map(|x| x.floor).collect();
    let n = needs(regime, &its, &floors, bp, bq, o.pairwise);
    (cap, Needs3 { need: n.need, other: n.other, floor: n.floor })
}

#[derive(Clone, Copy, Debug)]
struct Needs3 {
    need: f64,
    other: f64,
    floor: f64,
}

fn pitch_of(items: &[Itm]) -> f64 {
    let (mut sum, mut n) = (0.0, 0usize);
    for x in items.iter().filter(|x| x.kind != 3) {
        sum += x.it.w + x.it.s;
        n += 1;
    }
    if n == 0 {
        items.iter().map(|x| x.it.w.max(0.0) + x.it.s).sum::<f64>() / items.len().max(1) as f64
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
}

impl CutCtx {
    pub fn new(o: CutOpts, add_vias: bool, outline: Vec<(Vec2, Vec2)>) -> CutCtx {
        CutCtx { o, add_vias, outline, cache: RefCell::new(None), builds: RefCell::new((0, 0)) }
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
    need: f64,
    slack: f64,
    /// `price x phi`.
    cost: f64,
}

/// What one wire contributes: `(layer, cut, item)` and the junction ends `(layer, feature, half)`.
#[derive(Clone, Debug, Default)]
struct WireSt {
    items: Vec<(usize, usize, Itm)>,
    jends: Vec<(usize, usize, f64)>,
}

/// An open move (the first state of every wire and cut it touched).
#[derive(Default)]
struct Txn {
    wires: BTreeMap<WireId, WireSt>,
    slack: BTreeMap<(usize, usize), f64>,
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
    /// Per layer: junction feature -> (wire, half width) of the runs ending there.
    jhalf: Vec<BTreeMap<usize, Vec<(WireId, f64)>>>,
    total: f64,
    over: usize,
    txn: Option<Txn>,
    buf: Vec<u32>,
    /// Via site points per (triangle, net, r, s) ([`super::cutflow::via_point`]).
    sites: RefCell<SiteCache>,
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
            total: 0.0,
            over: 0,
            txn: None,
            buf: Vec::new(),
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
        let iv = super::refine::via_intervals(ev);
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
        let regime = self.regime(l, ci);
        let sh = self.sheets.clone();
        let jh = &self.jhalf[l];
        let half = |f: usize| jh.get(&f).filter(|v| !v.is_empty()).map(|v| v.iter().map(|x| x.1).fold(0.0, f64::max));
        let cs = &mut self.cuts[l][ci];
        let old = (cs.cost, cs.slack < -OVER_TOL && !cs.items.is_empty());
        if let Some(t) = self.txn.as_mut() {
            t.slack.entry((l, ci)).or_insert(if cs.items.is_empty() { f64::INFINITY } else { cs.slack });
        }
        if cs.items.is_empty() {
            *cs = CutSt::default();
        } else {
            let (cap, n) = evaluate(&sh[l], ci, &mut cs.items, regime, &half, &self.o);
            cs.cap = cap;
            cs.need = n.need;
            cs.slack = cap - n.need;
            cs.cost = self.price * phi(cs.slack, pitch_of(&cs.items), self.o.tau);
        }
        let new = (cs.cost, cs.slack < -OVER_TOL && !cs.items.is_empty());
        self.total += new.0 - old.0;
        self.over = (self.over as isize + new.1 as isize - old.1 as isize) as usize;
    }

    /// Put wire `w`'s contribution `ws` in place of its current one; returns the change of the
    /// total price. O(items of both x their cuts' items).
    fn replace(&mut self, w: WireId, ws: WireSt) -> f64 {
        let t0 = self.total;
        if w >= self.wires.len() {
            self.wires.resize(w + 1, WireSt::default());
        }
        let old = std::mem::replace(&mut self.wires[w], ws.clone());
        if let Some(t) = self.txn.as_mut() {
            t.wires.entry(w).or_insert_with(|| old.clone());
        }
        let mut dirty: BTreeSet<(usize, usize)> = BTreeSet::new();
        for &(l, ci, _) in &old.items {
            dirty.insert((l, ci));
        }
        let mut touched: BTreeSet<(usize, usize)> = BTreeSet::new();
        for &(l, ci, _) in &old.items {
            if touched.insert((l, ci)) {
                self.cuts[l][ci].items.retain(|x| x.wire != w);
            }
        }
        for &(l, f, _) in &old.jends {
            if let Some(v) = self.jhalf[l].get_mut(&f) {
                v.retain(|x| x.0 != w);
            }
            dirty.extend(self.junction_cuts(l, f));
        }
        for &(l, ci, it) in &ws.items {
            self.cuts[l][ci].items.push(it);
            dirty.insert((l, ci));
        }
        for &(l, f, h) in &ws.jends {
            self.jhalf[l].entry(f).or_default().push((w, h));
            dirty.extend(self.junction_cuts(l, f));
        }
        for (l, ci) in dirty {
            self.reprice(l, ci);
        }
        self.total - t0
    }

    fn junction_cuts(&self, l: usize, f: usize) -> Vec<(usize, usize)> {
        let Some(&v) = self.jv[l].get(&f) else { return Vec::new() };
        self.sheets[l].vcuts[v].iter().map(|&ci| (l, ci)).collect()
    }

    /// Set wire `w` to the runs `runs` (bands are pulled here) and the vias `vias`; returns the
    /// change of the total price. A run whose walk fails adds nothing (counted by the caller's
    /// report through [`CutPrice::walk_failed`]).
    pub fn set_wire(&mut self, w: WireId, runs: &mut [Stretch], vias: &[ViaItem]) -> f64 {
        let mut ws = WireSt::default();
        let sheets = self.sheets.clone();
        for (l, sh) in sheets.iter().enumerate() {
            for s in runs.iter_mut().filter(|s| s.layer == l) {
                if let Ok((b, wr)) = band(sh, &s.curve) {
                    s.band = b;
                    s.wraps = wr;
                }
            }
            let (items, jends) = wire_items(sh, &self.jf[l], runs, vias, &mut self.buf);
            ws.items.extend(items.into_iter().map(|(ci, it)| (l, ci, it)));
            ws.jends.extend(jends.into_iter().map(|(f, h)| (l, f, h)));
        }
        self.replace(w, ws)
    }

    /// Take wire `w` out (a dead wire); returns the change of the total.
    pub fn clear_wire(&mut self, w: WireId) -> f64 {
        self.replace(w, WireSt::default())
    }

    /// Open a move: every later change can be undone by [`CutPrice::rollback`].
    pub fn begin(&mut self) {
        self.txn = Some(Txn { total: self.total, over: self.over, ..Txn::default() });
    }

    /// Keep the changes since [`CutPrice::begin`].
    pub fn commit(&mut self) {
        self.txn = None;
    }

    /// Undo the changes since [`CutPrice::begin`].
    pub fn rollback(&mut self) {
        let Some(t) = self.txn.take() else { return };
        for (w, ws) in t.wires {
            self.replace(w, ws);
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
        t.slack.iter().any(|(&(l, ci), &s0)| {
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

    /// The (wire, traversal) of every item of cut `ci` on layer `l` (vias and runs).
    pub fn items_at(&self, l: usize, ci: usize) -> Vec<(WireId, usize)> {
        self.cuts[l][ci].items.iter().map(|x| (x.wire, x.k)).collect()
    }

    /// Capacity, need and slack of a cut (nm).
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
