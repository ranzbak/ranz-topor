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

use super::factors::squeeze_amount;
use super::pack::{infl_dir, layer_items, wire_discs, Disc, PackItem, Packer};
use super::cutmodel::CutPrice;
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

/// The via intervals of a wire's events: `(ka, kb)` per layer change (the traversals of the two
/// events around it). O(events).
pub fn via_intervals(ev: &[LEv]) -> Vec<(usize, usize)> {
    ev.windows(2).filter(|x| x[0].2 != x[1].2).map(|x| (x[0].0, x[1].0)).collect()
}

/// 5.4 (`[vias] between_crossings`): the traversals a via of each interval may take, and whether
/// the interval has one. `strict` = false: the interval `(ka, kb)` itself (as before). `strict`:
/// a via sits on the stretch strictly between its two events, never in the triangle of a
/// crossing, a tee or a one-layer triangle (Polubasov 2001 p. 4: the domain graph's vertices are
/// the crossings; a layer change lies on the wire piece between two of them); only the start of
/// the wire (`k = 0, s = 0`: its terminal) keeps its own triangle, and an event at `s = 1` (a
/// pass too narrow on the other layer, at the triangle's exit edge, or the end terminal) keeps
/// the triangle before it. So `lo = ka + 1` (`ka` at the start) and `hi = kb - 1` (`kb` when
/// `sb = 1`). An interval with `lo > hi` has no traversal between its events: it keeps
/// `(ka, kb)` for the drawing (which still tries, costly not failed) and reports `false`, a via
/// without a site for the plan. O(events).
pub fn via_ranges(ev: &[LEv], strict: bool) -> (Vec<(usize, usize)>, Vec<bool>) {
    let mut iv = Vec::new();
    let mut ok = Vec::new();
    for x in ev.windows(2).filter(|x| x[0].2 != x[1].2) {
        let ((ka, sa, _), (kb, sb, _)) = (x[0], x[1]);
        if !strict {
            iv.push((ka, kb));
            ok.push(true);
            continue;
        }
        let lo = if ka == 0 && sa <= 0.0 { 0 } else { ka + 1 };
        let hi = if sb >= 1.0 { Some(kb) } else { kb.checked_sub(1) };
        match hi.filter(|&h| h >= lo) {
            Some(h) => {
                iv.push((lo, h));
                ok.push(true);
            }
            None => {
                iv.push((ka, kb));
                ok.push(false);
            }
        }
    }
    (iv, ok)
}

/// Cache of [`end_site`] per (wire, end events as (traversal, fraction bits) pairs).
type EndSites = std::collections::HashMap<(WireId, usize, u64, usize, u64), Option<(usize, Vec2)>>;

/// 5.2 (`[vias] between_crossings`): a site for the via of the interval between the events `a`
/// and `b` of wire `w` inside an end traversal, on the via's own side of the event, for an
/// interval that has no traversal strictly between its events ([`via_ranges`] `false`).
/// Polubasov 2001 (`polubasov2001-global-via-minimization.pdf`, printed p. 4): the layer change
/// lies on the wire piece between two neighbouring crossings, and that piece includes the part
/// of a crossing's triangle before (after) the crossing point; the triangle is only the
/// triangulation's unit, so leaving it out says "no site" where the drawing finds one
/// (`planned_crossing_window`: VCC's via between the closed B.Cu area and its crossing with SDA,
/// drawn at (12.28, 7.9) with SDA moved aside). An end event inside its traversal (`0 < s < 1`:
/// a crossing or a tee) gives that traversal; an event at `s = 0` holds its whole triangle to
/// one layer and gives nothing. The traversal is a site when a candidate centre (the triangle's
/// static candidate points, [`tri_site_points`](super::pack::tri_site_points), a barycentric
/// grid of the triangle and points of the wire's chord on the via's side of the event) passes
/// `free` (the static board, as the plan's other via sites; the callers also ask the triangle
/// to hold the via's disc with its clearance, inradius >= via radius + clearance: the via
/// stands between two events in that one triangle, and a thinner one is no site, e.g. two
/// crossings in neighbouring slivers, `interleaved_boundary`). The centre returned lies furthest
/// on the via's side of the wires crossed in that traversal (the side of the piece's far end
/// of their present chords; graded, the least of those signed distances), then nearest the far
/// end. Whether the crossed wire still passes the disc is room, priced by the capacity model
/// like every planned via's disc, not a site test. A traversal that turns at a junction is
/// left out (its chord is not one segment). Returns the traversal and the centre.
/// O(candidates x (crossings + cost of `free`)).
pub fn end_site(st: &State, w: WireId, a: LEv, b: LEv, free: &dyn Fn(usize, Vec2) -> bool) -> Option<(usize, Vec2)> {
    const SAMPLES: usize = 8;
    const GRID: usize = 8;
    let wi = &st.wires[w];
    let pts = st.polyline(w);
    let at = |k: usize, s: f64| {
        let j = (k + 1).min(pts.len() - 1);
        pts[k] + (pts[j] - pts[k]) * s
    };
    let inside = |s: f64| s > 1e-9 && s < 1.0 - 1e-9;
    // Pieces (traversal, far end, event point): the via lies between the far end and the event.
    let mut pieces: Vec<(usize, Vec2, Vec2)> = Vec::new();
    if a.0 == b.0 && b.1 > a.1 {
        pieces.push((a.0, at(a.0, a.1), at(b.0, b.1)));
    } else {
        if inside(b.1) {
            pieces.push((b.0, at(b.0, 0.0), at(b.0, b.1)));
        }
        if inside(a.1) {
            pieces.push((a.0, at(a.0, 1.0), at(a.0, a.1)));
        }
    }
    let pr = st.proj;
    // (side clearance, -distance to the far end, traversal, centre): larger first.
    let mut best: Option<(f64, f64, usize, Vec2)> = None;
    for (k, far, ev) in pieces {
        if k >= wi.tris.len() || wi.turns.iter().any(|t| t.0 == k) {
            continue;
        }
        let t = wi.tris[k];
        if pr.tri_out[t] {
            continue;
        }
        // The present chords of the other nets' wires crossed in this traversal, oriented so
        // that the far end's side is positive.
        let chords: Vec<(Vec2, Vec2, f64)> = st
            .crossings(w)
            .into_iter()
            .filter(|x| x.k == k && st.wires[x.other].net != wi.net)
            .filter_map(|x| {
                let po = st.polyline(x.other);
                let (a0, a1) = (po[x.other_k], po[(x.other_k + 1).min(po.len() - 1)]);
                let d = a1 - a0;
                let l = d.len();
                (l > 0.0).then(|| (a0, d * (1.0 / l), if d.cross(far - a0) >= 0.0 { 1.0 } else { -1.0 }))
            })
            .collect();
        let side = |c: Vec2| chords.iter().map(|&(a0, u, sg)| sg * u.cross(c - a0)).fold(f64::INFINITY, f64::min);
        let [p, q, r] = pr.tris[t].map(|v| pr.pos[v]);
        let grid = (0..GRID).flat_map(|i| (0..GRID - i).map(move |j| (i, j))).map(|(i, j)| {
            let (u, v) = ((i as f64 + 0.5) / GRID as f64, (j as f64 + 0.5) / GRID as f64);
            p * u + q * v + r * (1.0 - u - v)
        });
        let cands = super::pack::tri_site_points(p, q, r).into_iter().chain(grid).chain((0..SAMPLES).map(|i| far + (ev - far) * (i as f64 / SAMPLES as f64)));
        for c in cands {
            let key = (side(c), -(c - far).len());
            if best.is_none_or(|b| key.0 > b.0 + 1.0 || ((key.0 - b.0).abs() <= 1.0 && key.1 > b.1)) && free(t, c) {
                best = Some((key.0, key.1, k, c));
            }
        }
    }
    best.map(|b| (b.2, b.3))
}

/// The default traversal of a via in interval `(ka, kb)`: `(ka + kb) / 2` rounded up (at `ka`
/// when both events are in one traversal).
pub fn mid_k(ka: usize, kb: usize) -> usize {
    if kb > ka {
        (ka + kb).div_ceil(2)
    } else {
        ka
    }
}

/// Layer of every pass of a wire with `n` passes from its labelled events `ev`; `bottom` when it
/// has none. A via interval `(ka, kb)` puts the via at traversal `(ka + kb) / 2` rounded up (at
/// `ka` when both events are in one traversal), as the drawing does. O(events + n).
pub fn pass_layers(n: usize, ev: &[LEv], bottom: i32) -> Vec<i32> {
    let ks: Vec<usize> = via_intervals(ev).into_iter().map(|(a, b)| mid_k(a, b)).collect();
    pass_layers_at(n, ev, bottom, &ks)
}

/// [`pass_layers`] with via `i` at traversal `ks[i]`: the passes before it are on the layer
/// before the layer change. O(events + n).
pub fn pass_layers_at(n: usize, ev: &[LEv], bottom: i32, ks: &[usize]) -> Vec<i32> {
    let Some(&(_, _, first)) = ev.first() else { return vec![bottom; n] };
    let mut out = vec![first; n];
    let (mut start, mut cur, mut i) = (0usize, first, 0usize);
    for x in ev.windows(2) {
        let ((ka, _, la), (kb, _, lb)) = (x[0], x[1]);
        if la == lb {
            continue;
        }
        let km = ks.get(i).copied().unwrap_or_else(|| mid_k(ka, kb));
        i += 1;
        let km = km.min(n).max(start);
        out[start..km].fill(cur);
        start = km;
        cur = lb;
    }
    out[start..].fill(cur);
    out
}

/// The via intervals and traversals of a wire before a change (kept where unchanged).
type OldVias<'x> = (&'x [(usize, usize)], &'x [usize]);

/// Measurement of the cost per move (shopping list 3.1): process-wide sums of the wall time and
/// count of [`try_move`] calls (one label move: relayout, via placement, pricing, keep or undo)
/// and of the via-position evaluations inside them (one traversal tried for one via). Read by the
/// plan metrics; never change the routing.
pub mod meter {
    use std::sync::atomic::{AtomicU64, Ordering};
    pub static MOVE_NS: AtomicU64 = AtomicU64::new(0);
    pub static MOVES: AtomicU64 = AtomicU64::new(0);
    pub static VIAPOS_NS: AtomicU64 = AtomicU64::new(0);
    pub static VIAPOS: AtomicU64 = AtomicU64::new(0);
    /// Cut model split of the time: the wire's stretches (`runs_of`), the walks of the changed
    /// parts, and putting them in (repricing the dirty cuts); counts: parts walked, cuts priced.
    pub static STRETCH_NS: AtomicU64 = AtomicU64::new(0);
    pub static FLUSHES: AtomicU64 = AtomicU64::new(0);
    pub static PART_NS: AtomicU64 = AtomicU64::new(0);
    pub static PARTS: AtomicU64 = AtomicU64::new(0);
    pub static INSTALL_NS: AtomicU64 = AtomicU64::new(0);
    pub static CUTS_PRICED: AtomicU64 = AtomicU64::new(0);
    pub fn add(ns: &AtomicU64, n: &AtomicU64, t: std::time::Instant) {
        ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        n.fetch_add(1, Ordering::Relaxed);
    }
    /// `(moves, us per move, via positions, us per via position)`.
    pub fn read() -> (u64, f64, u64, f64) {
        let (mn, m) = (MOVE_NS.load(Ordering::Relaxed), MOVES.load(Ordering::Relaxed));
        let (vn, v) = (VIAPOS_NS.load(Ordering::Relaxed), VIAPOS.load(Ordering::Relaxed));
        (m, mn as f64 / 1e3 / m.max(1) as f64, v, vn as f64 / 1e3 / v.max(1) as f64)
    }
    /// Cut model split: `(wire flushes, ms stretches, parts walked, ms walks, cuts priced, ms install)`.
    pub fn read_split() -> (u64, f64, u64, f64, u64, f64) {
        let ms = |a: &AtomicU64| a.load(Ordering::Relaxed) as f64 / 1e6;
        (FLUSHES.load(Ordering::Relaxed), ms(&STRETCH_NS), PARTS.load(Ordering::Relaxed), ms(&PART_NS), CUTS_PRICED.load(Ordering::Relaxed), ms(&INSTALL_NS))
    }
}

/// Order of a via traversal: (no site, overflow + squeeze change in um-mm, other wires in the
/// triangle, -inradius, traversal).
type SiteKey = (bool, i64, usize, i64, usize);

/// Per edge-layer overflow price of a labelling, kept incrementally.
///
/// A planned via takes room (D3; Popov & Popov 2012 p. 118: vias are round pads, i.e. vertices
/// of the triangulation, and p. 117: the room of a passage counts the widths and clearances of
/// what passes it): the via of a wire in traversal `k` (triangle `tris[k]`) sits on the wire's
/// chord there, so on the two edges of that triangle the wire crosses (its passes `k - 1` and
/// `k`) it is packed as its disc on every layer ([`layer_items`]), in the wire's place of the
/// edge order. Where in its interval the via goes is chosen here ("move the via"): the
/// traversal with a static via site ([`Packer::via_site`]) and the least overflow price, then
/// the drawing's order (fewest other wires in the triangle, widest triangle, first). A via
/// whose interval has no site at all pays one overflow price (`nosite`), so P3/P4 act on it.
pub struct CapModel<'a> {
    st: &'a State<'a>,
    packer: &'a Packer<'a>,
    /// Crossing direction of every pass from its wire's taut path ([`Packer::taut_dirs`]).
    dirs: &'a [Vec<Vec2>],
    nl: usize,
    price: f64,
    bottom: i32,
    /// Position of every pass in its wire.
    pass_k: Vec<usize>,
    /// Layer of every pass (-1: wire dead).
    pub pass_layer: Vec<i32>,
    /// Via disc diameter at every pass next to a via of its wire (0: none).
    pub pass_disc: Vec<Disc>,
    /// Per wire: the via intervals of its events and the traversal of each via.
    pub via_iv: Vec<Vec<(usize, usize)>>,
    pub via_k: Vec<Vec<usize>>,
    /// Per wire: its vias without a static site in their interval.
    nosite: Vec<usize>,
    /// 5.2: the site in an end traversal ([`end_site`]) of an interval without a traversal
    /// strictly between its events, per (wire, event a, event b) (`f64` fractions as bits); the
    /// state is borrowed for the model's life, so the geometry is fixed.
    end_sites: std::cell::RefCell<EndSites>,
    /// Price of edge `e`, layer `l` at `e * nl + l`.
    cost: Vec<f64>,
    /// Via squeeze price of edge `e`, layer `l` (the `via_squeeze` factor, [`Packer::via_squeeze`]).
    sq: Vec<f64>,
    /// Undo log of `cost` / `sq` entries while a move is tried.
    log: Option<Vec<(usize, f64, f64)>>,
    /// P1a (`[capacity] model = "cuts"`, [`super::cutmodel`]): the over-full price of the
    /// per-layer cuts replaces the per-edge price `cost` (still kept, for the reports and the P4
    /// rip-up); `None` = the per-edge model.
    pub cut: Option<CutPrice<'a>>,
    /// Cut model: the events of every wire as last applied, and the wires applied since the
    /// last repricing.
    evs: Vec<Vec<LEv>>,
    pending: Vec<WireId>,
}

impl<'a> CapModel<'a> {
    /// The model of the labelling `evl` (events per wire). O(passes x margin lookups + vias x
    /// interval length x layers).
    pub fn new(st: &'a State<'a>, packer: &'a Packer<'a>, dirs: &'a [Vec<Vec2>], evl: &[Vec<LEv>], bottom: i32, price: f64) -> Self {
        Self::build(st, packer, dirs, evl, bottom, price, false)
    }

    /// As [`CapModel::new`], priced by the per-layer cuts when `[capacity] model = "cuts"`
    /// ([`Packer::cuts`]; the P3 refinement). A sheet build that fails is recorded in
    /// [`super::cutmodel::CutCtx::problems`] and the model stays per-edge.
    pub fn new_cut(st: &'a State<'a>, packer: &'a Packer<'a>, dirs: &'a [Vec<Vec2>], evl: &[Vec<LEv>], bottom: i32, price: f64) -> Self {
        Self::build(st, packer, dirs, evl, bottom, price, true)
    }

    fn build(st: &'a State<'a>, packer: &'a Packer<'a>, dirs: &'a [Vec<Vec2>], evl: &[Vec<LEv>], bottom: i32, price: f64, cuts: bool) -> Self {
        let nl = st.proj.n_layers;
        let nw = st.wires.len();
        let mut pass_k = vec![0; st.passes.len()];
        for wi in st.wires.iter().filter(|wi| wi.alive) {
            for (k, &p) in wi.passes.iter().enumerate() {
                pass_k[p] = k;
            }
        }
        let mut m = CapModel {
            st,
            packer,
            dirs,
            nl,
            price,
            bottom,
            pass_k,
            pass_layer: vec![-1; st.passes.len()],
            pass_disc: vec![[0.0; 2]; st.passes.len()],
            via_iv: vec![Vec::new(); nw],
            via_k: vec![Vec::new(); nw],
            nosite: vec![0; nw],
            end_sites: std::cell::RefCell::new(std::collections::HashMap::new()),
            cost: vec![0.0; st.proj.edges.len() * nl],
            sq: vec![0.0; st.proj.edges.len() * nl],
            log: None,
            cut: None,
            evs: Vec::new(),
            pending: Vec::new(),
        };
        if let Some(ctx) = packer.cuts.as_ref().filter(|_| cuts) {
            match ctx.sheets(st) {
                Ok(sh) => {
                    m.cut = Some(CutPrice::new(st.proj, sh, ctx.o, price, nw));
                    m.evs = vec![Vec::new(); nw];
                }
                Err(e) => ctx.problems.borrow_mut().push(e),
            }
        }
        for (w, ev) in evl.iter().enumerate().take(nw) {
            if st.wires[w].alive {
                let iv = via_ranges(ev, packer.via_between).0;
                let ks = m.default_ks(w, ev, &iv, None);
                m.via_iv[w] = iv;
                m.apply(w, ev, ks);
            }
        }
        for e in 0..st.proj.edges.len() {
            for l in 0..nl {
                (m.cost[e * nl + l], m.sq[e * nl + l]) = m.eval(e, l);
            }
        }
        m.flush();
        for (w, ev) in evl.iter().enumerate().take(nw) {
            if st.wires[w].alive && !m.via_iv[w].is_empty() {
                let all: Vec<usize> = (0..m.via_iv[w].len()).collect();
                m.optimise(w, ev, &all);
            }
        }
        m
    }

    /// [`CapModel::rank`] (debug output).
    pub fn rank_of(&self, w: WireId, k: usize) -> (bool, usize, i64, usize) {
        self.rank(w, k)
    }

    /// Via radius plus clearance of wire `w` (0 when the packer has no via sizes).
    fn need_r(&self, w: WireId) -> f64 {
        let wi = &self.st.wires[w];
        let d = self.packer.via_d(wi.net);
        if d > 0.0 {
            0.5 * d + wi.s
        } else {
            0.0
        }
    }

    /// The drawing's preference of traversal `k` of `w` for a via: `(no site, other wires in
    /// the triangle, -inradius, k)`, smaller first.
    fn rank(&self, w: WireId, k: usize) -> (bool, usize, i64, usize) {
        let (st, pr) = (self.st, self.st.proj);
        let t = st.wires[w].tris[k.min(st.wires[w].tris.len() - 1)];
        let wi = &st.wires[w];
        let d = self.packer.via_d(wi.net);
        let site = d <= 0.0 || (self.packer.via_site(t, wi.net, 0.5 * d, wi.s) && !self.site_taken(w, t));
        let others = st.tri_trav[t].iter().filter(|&&(o, _)| o != w).count();
        (!site, others, -(pr.inradius(t) as i64), k)
    }

    /// Whether triangle `t` is taken for a via of `w` (M-C via room, Polubasov 2001 pp. 3-9: a via
    /// lies in its interval, at a site of its own): another net's wire already has a planned via
    /// in `t` and `t` cannot hold two discs (inradius below twice the via radius plus clearance).
    /// Derived from `via_k` alone, so the undo of a move restores it. O(traversals of `t`).
    fn site_taken(&self, w: WireId, t: super::state::TriId) -> bool {
        if !self.packer.reserve_sites {
            return false;
        }
        let st = self.st;
        let net = st.wires[w].net;
        st.tri_trav[t].iter().any(|&(o, ko)| o != w && st.wires[o].net != net && self.via_k.get(o).is_some_and(|ks| ks.contains(&ko)))
            && st.proj.inradius(t) < 2.0 * self.need_r(w)
    }

    /// 5.2: the end-traversal site ([`end_site`]) of via interval `i` of `w` under events `ev`
    /// when the interval has no traversal strictly between its events (`[vias]
    /// between_crossings`); `None` otherwise or when there is none. Cached.
    fn end_site_of(&self, w: WireId, ev: &[LEv], i: usize) -> Option<(usize, Vec2)> {
        if !self.packer.via_between {
            return None;
        }
        let (a, b) = ev.windows(2).filter(|x| x[0].2 != x[1].2).nth(i).map(|x| (x[0], x[1]))?;
        let key = (w, a.0, a.1.to_bits(), b.0, b.1.to_bits());
        if let Some(&x) = self.end_sites.borrow().get(&key) {
            return x;
        }
        let wi = &self.st.wires[w];
        let d = self.packer.via_d(wi.net);
        let (pr, packer, net, s) = (self.st.proj, self.packer, wi.net, wi.s);
        let need = self.need_r(w);
        let x = end_site(self.st, w, a, b, &|t, q| pr.inradius(t) >= need && !self.site_taken(w, t) && super::cutflow::point_free(pr, packer, q, net, s) >= 0.5 * d);
        self.end_sites.borrow_mut().insert(key, x);
        x
    }

    /// Via traversals by the drawing's preference alone (no room), keeping the traversal of a
    /// via whose interval is unchanged from `old` (intervals, traversals); a via of an interval
    /// with no traversal strictly between its events takes the traversal of its end site
    /// ([`CapModel::end_site_of`]) when it has one.
    fn default_ks(&self, w: WireId, ev: &[LEv], iv: &[(usize, usize)], old: Option<OldVias>) -> Vec<usize> {
        let n = self.st.wires[w].passes.len();
        let between = if self.packer.via_between { via_ranges(ev, true).1 } else { Vec::new() };
        let mut ks: Vec<usize> = Vec::with_capacity(iv.len());
        for (i, &(ka, kb)) in iv.iter().enumerate() {
            let lo = ka.max(ks.last().copied().unwrap_or(0)).min(n);
            let hi = kb.min(n).max(lo);
            if between.get(i) == Some(&false) {
                if let Some((k, _)) = self.end_site_of(w, ev, i).filter(|&(k, _)| (lo..=hi).contains(&k)) {
                    ks.push(k);
                    continue;
                }
            }
            if let Some((oiv, oks)) = old {
                if oiv.get(i) == Some(&(ka, kb)) && oks.get(i).is_some_and(|&k| (lo..=hi).contains(&k)) {
                    ks.push(oks[i]);
                    continue;
                }
            }
            ks.push((lo..=hi).min_by_key(|&k| self.rank(w, k)).unwrap_or(lo));
        }
        ks
    }

    /// Set the pass layers and via discs of `w` for via traversals `ks` (no pricing); returns the
    /// passes whose layer or disc changed. O(passes of the wire).
    fn apply(&mut self, w: WireId, ev: &[LEv], ks: Vec<usize>) -> Vec<usize> {
        let wi = &self.st.wires[w];
        let n = wi.passes.len();
        let pl = pass_layers_at(n, ev, self.bottom, &ks);
        let d = self.packer.via_d(wi.net);
        let disc = wire_discs(self.st, w, &ks, &vec![d; ks.len()]);
        let mut changed = Vec::new();
        for (k, &p) in wi.passes.iter().enumerate() {
            if self.pass_layer[p] != pl[k] || self.pass_disc[p] != disc[k] {
                self.pass_layer[p] = pl[k];
                self.pass_disc[p] = disc[k];
                changed.push(p);
            }
        }
        let nr = self.need_r(w);
        // 5.4 `[vias] between_crossings`: a via of an interval with no traversal between its
        // events has a site only on its own side of an end event ([`end_site`], 5.2); the others
        // have one when their traversal does.
        let between = if self.packer.via_between { via_ranges(ev, true).1 } else { Vec::new() };
        self.nosite[w] = if nr > 0.0 {
            ks.iter().enumerate().filter(|&(i, &k)| if between.get(i) == Some(&false) { self.end_site_of(w, ev, i).is_none() } else { self.rank(w, k).0 }).count()
        } else {
            0
        };
        self.via_k[w] = ks;
        if self.cut.is_some() {
            self.evs[w] = ev.to_vec();
            self.pending.push(w);
        }
        changed
    }

    /// Weight of the per-edge overflow price in the total: 0 when the cut model prices (the
    /// per-edge cost is then only kept for the reports).
    fn edge_w(&self) -> f64 {
        if self.cut.is_some() {
            0.0
        } else {
            1.0
        }
    }

    /// Cut model: price the wires applied since the last call again; returns the change of the
    /// cut price. O(runs of those wires, see [`CutPrice::set_wire`]).
    fn flush(&mut self) -> f64 {
        let Some(mut cut) = self.cut.take() else { return 0.0 };
        let mut ws = std::mem::take(&mut self.pending);
        ws.sort_unstable();
        ws.dedup();
        let mut d = 0.0;
        for w in ws {
            let t0 = std::time::Instant::now();
            let (mut runs, vias) = cut.runs_of(self.st, self.packer, w, &self.evs[w], &self.via_k[w], self.bottom);
            meter::add(&meter::STRETCH_NS, &meter::FLUSHES, t0);
            d += cut.set_wire(w, &mut runs, &vias);
        }
        self.cut = Some(cut);
        d
    }

    /// Price again the edge-layers of the passes `changed`; returns the change of the total.
    fn reprice(&mut self, changed: &[usize]) -> f64 {
        let mut es: Vec<usize> = changed.iter().map(|&p| self.st.passes[p].edge).collect();
        es.sort_unstable();
        es.dedup();
        let mut delta = 0.0;
        for e in es {
            for l in 0..self.nl {
                let i = e * self.nl + l;
                let (c, q) = self.eval(e, l);
                if c != self.cost[i] || q != self.sq[i] {
                    if let Some(log) = self.log.as_mut() {
                        log.push((i, self.cost[i], self.sq[i]));
                    }
                    delta += self.edge_w() * (c - self.cost[i]) + (q - self.sq[i]);
                    self.cost[i] = c;
                    self.sq[i] = q;
                }
            }
        }
        delta + self.flush()
    }

    /// Move the vias `which` of `w` through their intervals (one sweep each, in order) to the
    /// traversal with a site and the least overflow price, ties by the drawing's preference.
    /// O(vias x interval length x layers x passes of an edge).
    fn optimise(&mut self, w: WireId, ev: &[LEv], which: &[usize]) {
        let n = self.st.wires[w].passes.len();
        for &i in which {
            let mut ks = self.via_k[w].clone();
            let (ka, kb) = self.via_iv[w][i];
            let lo = ka.max(if i > 0 { ks[i - 1] } else { 0 }).min(n);
            let hi = kb.min(if i + 1 < ks.len() { ks[i + 1] } else { n }).min(n).max(lo);
            if lo == hi {
                continue;
            }
            let cur = ks[i];
            let mut run = 0.0;
            let mut best: Option<(SiteKey, usize)> = None;
            for k in lo..=hi {
                let t0 = std::time::Instant::now();
                ks[i] = k;
                let ch = self.apply(w, ev, ks.clone());
                run += self.reprice(&ch);
                meter::add(&meter::VIAPOS_NS, &meter::VIAPOS, t0);
                let r = self.rank(w, k);
                let key = (r.0, (run * 1e3).round() as i64, r.1, r.2, r.3);
                if best.as_ref().is_none_or(|b| key < b.0) {
                    best = Some((key, k));
                }
            }
            let k = best.map_or(cur, |b| b.1);
            ks[i] = k;
            let ch = self.apply(w, ev, ks);
            self.reprice(&ch);
        }
    }

    /// Lay out wire `w` again after its events changed: vias of unchanged intervals stay, the
    /// others are placed by the drawing's preference and then moved for room. Every changed
    /// cost entry goes to the undo log. O(passes of the wire + moved vias x interval).
    fn relayout(&mut self, w: WireId, ev: &[LEv]) {
        let iv = via_ranges(ev, self.packer.via_between).0;
        let (oiv, oks) = (std::mem::take(&mut self.via_iv[w]), self.via_k[w].clone());
        let ks = self.default_ks(w, ev, &iv, Some((&oiv, &oks)));
        let moved: Vec<usize> = (0..iv.len()).filter(|&i| oiv.get(i) != Some(&iv[i])).collect();
        self.via_iv[w] = iv;
        let ch = self.apply(w, ev, ks);
        self.reprice(&ch);
        self.optimise(w, ev, &moved);
    }

    /// The pack items of edge `e` on layer `l`, without the wires in `gone`.
    fn items(&self, e: usize, l: usize, gone: Option<&std::collections::BTreeSet<WireId>>) -> Vec<PackItem> {
        let pr = self.st.proj;
        let order: Vec<usize> = match gone {
            Some(g) => self.st.edge_order[e].iter().copied().filter(|&p| !g.contains(&self.st.passes[p].wire)).collect(),
            None => self.st.edge_order[e].clone(),
        };
        layer_items(
            &order,
            l,
            |p| self.pass_layer[p],
            |p| self.pass_disc[p],
            |p| {
                let w = self.st.passes[p].wire;
                let wi = &self.st.wires[w];
                PackItem { net: wi.net, w: self.st.pass_w.get(p).copied().unwrap_or(wi.w), s: wi.s, infl: infl_dir(pr, e, self.dirs[w][self.pass_k[p]]), disc: [0.0; 2] }
            },
        )
        .1
    }

    /// Overflow price and via squeeze price of edge `e` on layer `l` under the current pass
    /// layers: every via disc there pays the `via_squeeze` weight times the squeeze of the slack
    /// the packing leaves (the least `rmax - lmin`) against one via diameter.
    fn eval(&self, e: usize, l: usize) -> (f64, f64) {
        let items = self.items(e, l, None);
        if items.is_empty() {
            return (0.0, 0.0);
        }
        let over = self.packer.bounds(e, l, &items).worst();
        let sq_w = self.packer.via_squeeze;
        let sq = if sq_w > 0.0 { sq_w * items.iter().filter(|i| i.dmax() > 0.0).map(|i| squeeze_amount(-over, i.dmax())).sum::<f64>() } else { 0.0 };
        if over <= 1e-6 {
            return (0.0, sq);
        }
        let pitch = (items.iter().map(|i| i.w.max(i.dmax()) + i.s).sum::<f64>() / items.len() as f64).max(1.0);
        (self.price * (1.0 + over / pitch), sq)
    }

    /// Whether the passes on edge `e`, layer `l` fit when the wires in `gone` are taken out.
    pub fn fits_without(&self, e: usize, l: usize, gone: &std::collections::BTreeSet<WireId>) -> bool {
        let items = self.items(e, l, Some(gone));
        items.is_empty() || self.packer.bounds(e, l, &items).fits()
    }

    /// Total overflow price of the labelling (mm), vias without a site and via squeeze
    /// included.
    pub fn total(&self) -> f64 {
        let cut = self.cut.as_ref().map_or(0.0, |c| c.total());
        self.edge_w() * self.cost.iter().sum::<f64>() + self.sq.iter().sum::<f64>() + self.price * self.nosite.iter().sum::<usize>() as f64 + cut
    }

    /// Via squeeze price of the labelling (mm).
    pub fn squeeze(&self) -> f64 {
        self.sq.iter().sum()
    }

    /// Over-full edge-layers (cut model: over-full cuts) plus vias without a site.
    pub fn overfull(&self) -> usize {
        let over = match self.cut.as_ref() {
            Some(c) => c.overfull(),
            None => self.cost.iter().filter(|&&c| c > 0.0).count(),
        };
        over + self.nosite.iter().sum::<usize>()
    }

    /// The predicted violations of this labelling as the TopoR loop's archive and every loop move
    /// judge them (shopping list 5.2): `(count, graded)`. The count is [`CapModel::overfull`]
    /// (over-full edge-layers, or over-full cuts with `[capacity] model = "cuts"`, plus vias
    /// without a site); the graded amount gives each of them 1 plus how far it is over, as the
    /// `violation` factor does (1 per violation plus its depth): an edge-layer its price factor
    /// `1 + over / pitch` (the per-edge price, `CapModel::eval`), a cut `1 - slack / pitch`
    /// ([`CutPrice::over_amount`]), a via without a site 1. So `graded >= count`, and one more
    /// over-full item always adds at least 1. O(edges x layers + cuts).
    pub fn violations(&self) -> (usize, f64) {
        let nosite = self.nosite.iter().sum::<usize>();
        let over = match self.cut.as_ref() {
            Some(c) => c.over_amount(),
            None => self.cost.iter().filter(|&&c| c > 0.0).map(|&c| c / self.price).sum(),
        };
        (self.overfull(), over + nosite as f64)
    }

    /// Shopping list 2.4: the cut model's shrinkage level, `(cuts that fit only necked, graded
    /// amount in pitches of width reduction, price mm)` ([`CutPrice::shrinkage`]); zeros for the
    /// per-edge model (it necks in P2, `State::refit_wire`). O(cuts).
    pub fn shrinkage(&self) -> (usize, f64, f64) {
        self.cut.as_ref().map_or((0, 0.0, 0.0), |c| c.shrinkage())
    }

    /// The cut model's necks `(wire, traversal, width)` ([`CutPrice::necks`]); empty for the
    /// per-edge model.
    pub fn cut_necks(&self) -> Vec<(WireId, usize, f64)> {
        self.cut.as_ref().map_or_else(Vec::new, |c| c.necks())
    }

    /// The over-full `(edge, layer)` pairs, ascending.
    pub fn overfull_list(&self) -> Vec<(usize, usize)> {
        (0..self.cost.len()).filter(|&i| self.cost[i] > 0.0).map(|i| (i / self.nl, i % self.nl)).collect()
    }

    /// Wires with a via that has no site in its interval, ascending.
    pub fn nosite_wires(&self) -> Vec<WireId> {
        (0..self.nosite.len()).filter(|&w| self.nosite[w] > 0).collect()
    }

    /// Debug (`TOPO_LOOPDBG`): the vias of wire `w` under events `ev`, each with its interval, its
    /// traversal and per traversal of the interval the triangle, whether it has a static site,
    /// whether it is taken, its inradius and the free radius against the static board (mm).
    pub fn nosite_debug(&self, w: WireId, ev: &[LEv]) -> String {
        let wi = &self.st.wires[w];
        let d = self.packer.via_d(wi.net);
        let (iv, ok) = via_ranges(ev, self.packer.via_between);
        let mut out = format!("wire {w} net {} via_d {:.3} need_r {:.3} nosite {} ev {:?}:", wi.net, d / 1e6, self.need_r(w) / 1e6, self.nosite[w], ev);
        for (i, &(ka, kb)) in iv.iter().enumerate() {
            out += &format!(" [via {i} iv ({ka},{kb}) between_ok {} end_site {:?} k {:?}:", ok[i], self.end_site_of(w, ev, i).map(|(k, c)| (k, (c.x / 1e6 * 1e3).round() / 1e3, (-c.y / 1e6 * 1e3).round() / 1e3)), self.via_k[w].get(i));
            for k in ka..=kb.min(wi.tris.len().saturating_sub(1)) {
                let t = wi.tris[k];
                let site = d <= 0.0 || self.packer.via_site(t, wi.net, 0.5 * d, wi.s);
                out += &format!(" k{k} t{t} site {site} taken {} inr {:.3} out {} obs {};", self.site_taken(w, t), self.st.proj.inradius(t) / 1e6, self.st.proj.tri_out[t], self.st.proj.tri_obs[t].len());
            }
            out += "]";
        }
        out
    }

    /// Vias without a site.
    pub fn nosite(&self) -> usize {
        self.nosite.iter().sum()
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
    dirs: &[Vec<Vec2>],
    evl: &mut [Vec<LEv>],
    mask: &[Vec<u64>],
    partner: &[Vec<Option<(WireId, usize)>>],
    bottom: i32,
    trans: &dyn Fn(usize, usize) -> f64,
    unary: &dyn Fn(WireId, usize, usize) -> f64,
    p: &RefineParams,
) -> RefineStats {
    // Switched off: the model is built with a unit price only to count the over-full edge-layers.
    let mut cm = CapModel::new_cut(st, packer, dirs, evl, bottom, if p.overflow_mm > 0.0 { p.overflow_mm } else { 1.0 });
    let mut stats = RefineStats { before: cm.overfull(), ..RefineStats::default() };
    if p.overflow_mm <= 0.0 {
        stats.after = stats.before;
        return stats;
    }
    // The cut model may add vias when its setting says so (`[capacity] cut_vias`).
    let add_vias = p.add_vias || (cm.cut.is_some() && packer.cuts.as_ref().is_some_and(|c| c.add_vias));
    let mut evals = 0usize;
    let ctx = Around { st, mask, partner, bottom, trans, unary, add_vias, max_evals: p.max_evals };
    for _sweep in 0..8 {
        let mut improved = false;
        if cm.cut.is_some() {
            // Cut model: the wires of every over-full cut, most over-full first.
            let list = cm.cut.as_ref().map(|c| c.overfull_list()).unwrap_or_default();
            for (l, ci, _) in list {
                let mut seen = std::collections::BTreeSet::new();
                let its: Vec<(WireId, usize)> = cm.cut.as_ref().map(|c| c.items_at(l, ci)).unwrap_or_default().into_iter().filter(|x| seen.insert(*x)).collect();
                for (w, k) in its {
                    if evals >= p.max_evals || !cm.cut.as_ref().is_some_and(|c| c.is_over(l, ci)) {
                        break;
                    }
                    if !st.wires[w].alive {
                        continue;
                    }
                    improved |= ctx.moves_around(&mut cm, evl, w, k, &mut evals, &mut stats);
                }
            }
        } else {
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
                    improved |= ctx.moves_around(&mut cm, evl, w, k, &mut evals, &mut stats);
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

/// The fixed inputs of the P3 moves around one pass.
struct Around<'r> {
    st: &'r State<'r>,
    mask: &'r [Vec<u64>],
    partner: &'r [Vec<Option<(WireId, usize)>>],
    bottom: i32,
    trans: &'r dyn Fn(usize, usize) -> f64,
    unary: &'r dyn Fn(WireId, usize, usize) -> f64,
    add_vias: bool,
    max_evals: usize,
}

impl Around<'_> {
    /// Try the label moves of the events around traversal `k` of wire `w` (two before, two
    /// after): the swap with the crossing partner and every other allowed layer; the first move
    /// that lowers the price is kept per event. Returns whether one was kept.
    fn moves_around(&self, cm: &mut CapModel, evl: &mut [Vec<LEv>], w: WireId, k: usize, evals: &mut usize, stats: &mut RefineStats) -> bool {
        let (mask, partner) = (self.mask, self.partner);
        let mut improved = false;
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
                if *evals >= self.max_evals {
                    break;
                }
                *evals += 1;
                if try_move(self.st, cm, evl, &mv, self.bottom, self.trans, self.unary, self.add_vias, false) {
                    stats.moves += 1;
                    improved = true;
                    break;
                }
            }
        }
        improved
    }
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
    _bottom: i32,
    trans: &dyn Fn(usize, usize) -> f64,
    unary: &dyn Fn(WireId, usize, usize) -> f64,
    add_vias: bool,
    strict: bool,
) -> bool {
    let t0 = std::time::Instant::now();
    let r = try_move_in(st, cm, evl, mv, trans, unary, add_vias, strict);
    meter::add(&meter::MOVE_NS, &meter::MOVES, t0);
    r
}

#[allow(clippy::too_many_arguments)]
fn try_move_in(
    st: &State,
    cm: &mut CapModel,
    evl: &mut [Vec<LEv>],
    mv: &[((WireId, usize), i32)],
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
    // Lay the touched wires out again (pass layers, via traversals and discs), logging every
    // changed price for the undo.
    struct Snap {
        w: WireId,
        layer: Vec<i32>,
        disc: Vec<Disc>,
        iv: Vec<(usize, usize)>,
        ks: Vec<usize>,
        nosite: usize,
    }
    let snaps: Vec<Snap> = wires
        .iter()
        .map(|&w| Snap {
            w,
            layer: st.wires[w].passes.iter().map(|&p| cm.pass_layer[p]).collect(),
            disc: st.wires[w].passes.iter().map(|&p| cm.pass_disc[p]).collect(),
            iv: cm.via_iv[w].clone(),
            ks: cm.via_k[w].clone(),
            nosite: cm.nosite[w],
        })
        .collect();
    let old_ns: usize = wires.iter().map(|&w| cm.nosite[w]).sum();
    let old_evs: Vec<Vec<LEv>> = if cm.cut.is_some() { wires.iter().map(|&w| cm.evs[w].clone()).collect() } else { Vec::new() };
    let cut0 = cm.cut.as_mut().map(|c| {
        c.begin();
        c.total()
    });
    cm.log = Some(Vec::new());
    for &w in &wires {
        cm.relayout(w, &evl[w]);
    }
    let log = cm.log.take().unwrap_or_default();
    // First logged value of every entry = its price before the move.
    let mut before: std::collections::BTreeMap<usize, (f64, f64)> = std::collections::BTreeMap::new();
    for &(i, c, q) in &log {
        before.entry(i).or_insert((c, q));
    }
    let new_ns: usize = wires.iter().map(|&w| cm.nosite[w]).sum();
    let ew = cm.edge_w();
    let d_cut = match (cm.cut.as_ref(), cut0) {
        (Some(c), Some(t0)) => c.total() - t0,
        _ => 0.0,
    };
    let d_oc: f64 = before.iter().map(|(&i, &(c, q))| ew * (cm.cost[i] - c) + (cm.sq[i] - q)).sum::<f64>() + cm.price * (new_ns as f64 - old_ns as f64) + d_cut;
    let worse_edge = match cm.cut.as_ref() {
        Some(c) => c.worse_somewhere(),
        None => before.iter().any(|(&i, &(c, _))| cm.cost[i] > c + EPS),
    };
    let worse_somewhere = strict && (new_ns > old_ns || worse_edge);
    if (new_wc - old_wc) + d_oc < -EPS && (add_vias || new_wc <= old_wc + EPS) && !worse_somewhere {
        if let Some(c) = cm.cut.as_mut() {
            c.commit();
        }
        return true;
    }
    if let Some(c) = cm.cut.as_mut() {
        c.rollback();
        for (&w, e) in wires.iter().zip(old_evs) {
            cm.evs[w] = e;
        }
    }
    for (&i, &(c, q)) in &before {
        cm.cost[i] = c;
        cm.sq[i] = q;
    }
    for s in snaps {
        for (k, &p) in st.wires[s.w].passes.iter().enumerate() {
            cm.pass_layer[p] = s.layer[k];
            cm.pass_disc[p] = s.disc[k];
        }
        cm.via_iv[s.w] = s.iv;
        cm.via_k[s.w] = s.ks;
        cm.nosite[s.w] = s.nosite;
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
    fn via_ranges_exclude_the_triangles_of_the_events() {
        // Old rule: the interval itself.
        let ev = [(0, 0.0, 0), (3, 0.4, 0), (5, 0.6, 1), (9, 1.0, 1)];
        assert_eq!(via_ranges(&ev, false), (vec![(3, 5)], vec![true]));
        // Between two crossings at traversals 3 and 5: only traversal 4.
        assert_eq!(via_ranges(&ev, true), (vec![(4, 4)], vec![true]));
        // Q15: crossings in neighbouring triangles (3, 4): no traversal between them, a via
        // without a site; the drawing keeps (3, 4) to try.
        let ev = [(3, 0.4, 0), (4, 0.6, 1)];
        assert_eq!(via_ranges(&ev, true), (vec![(3, 4)], vec![false]));
        // Two crossings in one triangle: no site either.
        let ev = [(3, 0.2, 0), (3, 0.7, 1)];
        assert_eq!(via_ranges(&ev, true).1, vec![false]);
        // The start terminal keeps its triangle; a crossing in triangle 1 bounds the range to 0.
        let ev = [(0, 0.0, 1), (1, 0.5, 0)];
        assert_eq!(via_ranges(&ev, true), (vec![(0, 0)], vec![true]));
        // An event at s = 1 (narrow pass at the exit edge, or the end terminal) keeps the
        // triangle before it; as the first event of an interval it starts after its pass.
        let ev = [(2, 0.5, 0), (4, 1.0, 1), (7, 1.0, 0)];
        assert_eq!(via_ranges(&ev, true), (vec![(3, 4), (5, 7)], vec![true, true]));
        // A one-layer triangle (s = 0) bounds like a crossing.
        let ev = [(2, 0.0, 0), (6, 0.0, 1)];
        assert_eq!(via_ranges(&ev, true), (vec![(3, 5)], vec![true]));
    }

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
