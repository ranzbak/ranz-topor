//! The parallel refinement (par-refine, docs/design/47-parallel-refine.md §2-5): the candidate
//! moves of [`super::refine::refine`] are evaluated by several lanes at once, and the run keeps
//! exactly the moves the serial refinement keeps, in the same order, with the same bits.
//!
//! Ordered speculation. The serial refinement is a stream of candidates (sweep, over-full
//! edge-layer, pass, event, move: [`Cursor`]) where the first candidate that lowers the price is
//! kept and every other one is undone. A rejected candidate leaves the model as it was (I1), and
//! a candidate's verdict and every value it computes are a function of the model and the move
//! (I2). So the candidates up to the next kept one are exactly those the stream generates from
//! the present model assuming every one is rejected. Within an *epoch* the lanes take those
//! candidates in stream order from one generator, each evaluates its candidate on the read-only
//! master ([`super::refine::Arrays`]) through its own [`Overlay`] (epoch-stamped copies of what
//! the candidate writes), and reports the verdict. The first kept candidate ends the epoch: the
//! lanes stop taking candidates after it (and drop the ones they are on), the run thread scans
//! the results in stream order, commits the first kept one by copying what its overlay wrote
//! (bit copies, no arithmetic), advances its own cursor as the serial loop would after that
//! move, and the next epoch starts from the new master. Proof by induction over the stream:
//! before the kept candidate, the serial loop evaluates exactly these candidates on exactly this
//! model (I1), so (I2) the same verdicts, the same first kept move and the same values. Floats
//! are summed inside one lane in the serial order (edge-layers ascending), never across lanes;
//! nothing depends on the lane count or the thread timing.
//!
//! The one memo whose value depends on the model, the end sites (`Arrays::end_sites`: filled
//! when first asked, with the other nets' planned vias of that moment), keeps the serial order of
//! its fills: a lane records what it computed itself; the scan replays the fills in stream order
//! and evaluates a candidate again on the run thread when an earlier candidate of the epoch had
//! filled the same key with another value (what the serial loop would have read). Only the
//! scanned candidates' fills reach the master.
//!
//! Literature: Müller, Radke & Vygen 2011 (docs/literature/mueller-radke-vygen2011-resource-
//! sharing.pdf, printed pp. 9-11 §6): BonnRoute's parallel resource sharing evaluates block
//! solvers concurrently on shared prices and validates each result in a fixed order, recomputing
//! the violated ones; here the order is the serial index (deterministic reservations, Blelloch,
//! Fineman, Gibbons & Shun, PPoPP 2012, §4; not in docs/literature).
//!
//! Lanes come from the run's spare compute slots (`[optimize] refine_lanes`, [`lanes`]); one lane
//! is the serial loop on this code path. A Cancel / time limit is seen by the run thread between
//! candidates (the lanes drop theirs), so a stopped parallel refinement equals a serial one
//! stopped at a candidate boundary. The cut model (`[capacity] model = "cuts"`) keeps the
//! in-place serial loop.

use super::pack::Disc;
use super::refine::{meter, Around, Arrays, CapModel, EndKey, Frame, LEv, Mem, MemRead, RefineStats};
use super::state::WireId;
use crate::fxhash::FxHashMap;
use crate::geom::Vec2;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// One candidate move: the label changes `((wire, event), layer)`.
pub(crate) type Mv = Vec<((WireId, usize), i32)>;

/// An end site value ([`super::refine::end_site`]).
type Site = Option<(usize, Vec2)>;

/// The serial refinement's candidate stream as an explicit cursor (the loops of
/// `refine_reference` + `Around::moves_around` unrolled): `next` gives the candidate the serial
/// loop would try next if the last one was rejected; [`Cursor::accepted`] does what the loop does
/// after a kept move (the event's other moves are skipped, the sweep counts as improved). It only
/// reads the model. Cloning it is cheap (the lists are shared).
#[derive(Clone)]
pub(crate) struct Cursor {
    started: bool,
    done: bool,
    sweep: usize,
    improved: bool,
    /// The over-full edge-layers of this sweep and the next to enter.
    list: Arc<Vec<(usize, usize)>>,
    li: usize,
    /// The edge-layer being walked, its passes on that layer and the next pass.
    el: Option<(usize, usize)>,
    passes: Arc<Vec<usize>>,
    pi: usize,
    /// The events around the pass being walked.
    ev: Option<EvCur>,
    /// Candidates handed out (the serial `evals`).
    pub(crate) evals: usize,
}

#[derive(Clone)]
struct EvCur {
    w: WireId,
    i: usize,
    hi: usize,
    moves: Vec<Mv>,
    mi: usize,
}

impl Cursor {
    pub(crate) fn new() -> Cursor {
        Cursor { started: false, done: false, sweep: 0, improved: false, list: Arc::new(Vec::new()), li: 0, el: None, passes: Arc::new(Vec::new()), pi: 0, ev: None, evals: 0 }
    }

    /// The label moves of event `i` of wire `w` under labels `evl`, in the serial order: the
    /// swap with the crossing partner, then every other allowed layer. O(layers).
    fn moves_for(ctx: &Around, nl: usize, evl: &[Vec<LEv>], w: WireId, i: usize) -> Vec<Mv> {
        let (mask, partner) = (ctx.mask, ctx.partner);
        let cur = evl[w][i].2;
        let mut moves: Vec<Mv> = Vec::new();
        if let Some((w2, i2)) = partner[w][i] {
            let c2 = evl[w2][i2].2;
            if c2 != cur && (mask[w][i] >> c2) & 1 == 1 && (mask[w2][i2] >> cur) & 1 == 1 {
                moves.push(vec![((w, i), c2), ((w2, i2), cur)]);
            }
        }
        for nl in 0..nl as i32 {
            if nl == cur || (mask[w][i] >> nl) & 1 == 0 {
                continue;
            }
            if partner[w][i].is_some_and(|(w2, i2)| evl[w2][i2].2 == nl) {
                continue;
            }
            moves.push(vec![((w, i), nl)]);
        }
        moves
    }

    /// The next candidate of the stream, assuming the previous one was rejected; `None` when the
    /// refinement ends (no sweep improved, 8 sweeps, or `max_evals` candidates). O(1) amortised
    /// (a new sweep scans the edge-layers, a new edge-layer its passes).
    pub(crate) fn next(&mut self, fx: &Frame, a: &Arrays, evl: &[Vec<LEv>], ctx: &Around) -> Option<Mv> {
        let (st, nl) = (fx.st, fx.nl);
        loop {
            if self.done {
                return None;
            }
            if let Some(ec) = self.ev.as_mut() {
                if ec.mi < ec.moves.len() {
                    if self.evals >= ctx.max_evals {
                        self.done = true;
                        return None;
                    }
                    let mv = ec.moves[ec.mi].clone();
                    ec.mi += 1;
                    self.evals += 1;
                    return Some(mv);
                }
                ec.i += 1;
                if ec.i < ec.hi {
                    ec.moves = Self::moves_for(ctx, nl, evl, ec.w, ec.i);
                    ec.mi = 0;
                } else {
                    self.ev = None;
                }
                continue;
            }
            if let Some((e, l)) = self.el {
                if self.pi < self.passes.len() && self.evals < ctx.max_evals && a.cost[e * nl + l] > 0.0 {
                    let pid = self.passes[self.pi];
                    self.pi += 1;
                    let w = st.passes[pid].wire;
                    let k = st.wires[w].passes.iter().position(|&x| x == pid).expect("pass of its wire");
                    // The events around the pass (position (k, 1.0)): two before, two after.
                    let j1 = evl[w].iter().take_while(|x| x.0 < k || (x.0 == k && x.1 <= 1.0)).count();
                    let (lo, hi) = (j1.saturating_sub(2), (j1 + 2).min(evl[w].len()));
                    let moves = if lo < hi { Self::moves_for(ctx, nl, evl, w, lo) } else { Vec::new() };
                    self.ev = Some(EvCur { w, i: lo, hi, moves, mi: 0 });
                } else {
                    self.el = None;
                }
                continue;
            }
            if self.li < self.list.len() {
                let (e, l) = self.list[self.li];
                self.li += 1;
                if a.cost[e * nl + l] <= 0.0 {
                    continue;
                }
                self.passes = Arc::new(st.edge_order[e].iter().copied().filter(|&q| a.pass_layer[q] == l as i32).collect());
                self.pi = 0;
                self.el = Some((e, l));
                continue;
            }
            if self.started {
                if !self.improved || self.evals >= ctx.max_evals {
                    self.done = true;
                    return None;
                }
                self.sweep += 1;
                if self.sweep >= 8 {
                    self.done = true;
                    return None;
                }
            }
            self.started = true;
            self.improved = false;
            self.list = Arc::new((0..a.cost.len()).filter(|&i| a.cost[i] > 0.0).map(|i| (i / nl, i % nl)).collect());
            self.li = 0;
        }
    }

    /// The candidate last handed out was kept: the event's other moves are skipped and the sweep
    /// improved (the serial loop's `break` and `improved = true`). O(1).
    pub(crate) fn accepted(&mut self) {
        if let Some(ec) = self.ev.as_mut() {
            ec.mi = ec.moves.len();
        }
        self.improved = true;
    }
}

/// A lane's private copy of what its candidate writes, over the read-only master: dense arrays
/// stamped with the candidate's epoch (a lookup = one compare, a new candidate = epoch + 1),
/// sized once per run thread and reused across refinements.
#[derive(Default)]
pub(crate) struct Overlay {
    epoch: u32,
    pstamp: Vec<u32>,
    play: Vec<i32>,
    pdisc: Vec<Disc>,
    wstamp: Vec<u32>,
    wiv: Vec<Vec<(usize, usize)>>,
    wks: Vec<Vec<usize>>,
    wns: Vec<usize>,
    cstamp: Vec<u32>,
    cval: Vec<(f64, f64)>,
    /// What the candidate wrote, in first-write order.
    passes: Vec<usize>,
    wires: Vec<WireId>,
    costs: Vec<usize>,
    /// End sites the candidate computed itself (a lookup missed the master), in order.
    sites: FxHashMap<EndKey, Site>,
    fresh: Vec<(EndKey, Site)>,
    /// Via positions tried and their time (meters, counted when the candidate is consumed).
    vp_n: u64,
    vp_ns: u64,
}

impl Overlay {
    /// Grow to `np` passes, `nw` wires, `nc` edge-layers and start a new candidate. O(growth).
    fn begin(&mut self, np: usize, nw: usize, nc: usize) {
        if self.pstamp.len() < np {
            self.pstamp.resize(np, 0);
            self.play.resize(np, 0);
            self.pdisc.resize(np, [0.0; 2]);
        }
        if self.wstamp.len() < nw {
            self.wstamp.resize(nw, 0);
            self.wiv.resize_with(nw, Vec::new);
            self.wks.resize_with(nw, Vec::new);
            self.wns.resize(nw, 0);
        }
        if self.cstamp.len() < nc {
            self.cstamp.resize(nc, 0);
            self.cval.resize(nc, (0.0, 0.0));
        }
        if self.epoch == u32::MAX {
            self.pstamp.iter_mut().chain(self.wstamp.iter_mut()).chain(self.cstamp.iter_mut()).for_each(|s| *s = 0);
            self.epoch = 0;
        }
        self.epoch += 1;
        self.passes.clear();
        self.wires.clear();
        self.costs.clear();
        self.sites.clear();
        self.fresh.clear();
        self.vp_n = 0;
        self.vp_ns = 0;
    }
}

/// A lane's view: the master `base` under the lane's overlay; `mid` = the end sites the epoch's
/// earlier candidates filled (only when the run thread evaluates a candidate again, see the
/// module doc); `stop` = the candidate is no longer wanted.
struct View<'m> {
    base: &'m Arrays,
    ov: &'m mut Overlay,
    mid: Option<&'m FxHashMap<EndKey, Site>>,
    stop: &'m (dyn Fn() -> bool + Sync),
}

impl View<'_> {
    fn touch_wire(&mut self, w: WireId) {
        let ov = &mut *self.ov;
        if ov.wstamp[w] != ov.epoch {
            ov.wstamp[w] = ov.epoch;
            ov.wiv[w].clear();
            ov.wiv[w].extend_from_slice(self.base.via_iv(w));
            ov.wks[w].clear();
            ov.wks[w].extend_from_slice(self.base.via_k(w));
            ov.wns[w] = self.base.nosite(w);
            ov.wires.push(w);
        }
    }
}

impl MemRead for View<'_> {
    fn layer(&self, p: usize) -> i32 {
        if self.ov.pstamp[p] == self.ov.epoch {
            self.ov.play[p]
        } else {
            self.base.pass_layer[p]
        }
    }
    fn disc(&self, p: usize) -> Disc {
        if self.ov.pstamp[p] == self.ov.epoch {
            self.ov.pdisc[p]
        } else {
            self.base.pass_disc[p]
        }
    }
    fn via_iv(&self, w: WireId) -> &[(usize, usize)] {
        if self.ov.wstamp[w] == self.ov.epoch {
            &self.ov.wiv[w]
        } else {
            self.base.via_iv(w)
        }
    }
    fn via_k(&self, w: WireId) -> &[usize] {
        if self.ov.wstamp.get(w) == Some(&self.ov.epoch) {
            &self.ov.wks[w]
        } else {
            self.base.via_k(w)
        }
    }
    fn nosite(&self, w: WireId) -> usize {
        if self.ov.wstamp[w] == self.ov.epoch {
            self.ov.wns[w]
        } else {
            self.base.nosite(w)
        }
    }
    fn cost(&self, i: usize) -> (f64, f64) {
        if self.ov.cstamp[i] == self.ov.epoch {
            self.ov.cval[i]
        } else {
            self.base.cost(i)
        }
    }
    fn end_site(&self, key: &EndKey) -> Option<Site> {
        if let Some(&v) = self.ov.sites.get(key) {
            return Some(v);
        }
        if let Some(&v) = self.mid.and_then(|m| m.get(key)) {
            return Some(v);
        }
        self.base.end_site(key)
    }
}

impl Mem for View<'_> {
    fn set_pass(&mut self, p: usize, l: i32, d: Disc) {
        let ov = &mut *self.ov;
        if ov.pstamp[p] != ov.epoch {
            ov.pstamp[p] = ov.epoch;
            ov.passes.push(p);
        }
        ov.play[p] = l;
        ov.pdisc[p] = d;
    }
    fn set_via_iv(&mut self, w: WireId, iv: Vec<(usize, usize)>) {
        self.touch_wire(w);
        self.ov.wiv[w] = iv;
    }
    fn set_via_k(&mut self, w: WireId, ks: &[usize]) {
        self.touch_wire(w);
        let v = &mut self.ov.wks[w];
        v.clear();
        v.extend_from_slice(ks);
    }
    fn set_nosite(&mut self, w: WireId, n: usize) {
        self.touch_wire(w);
        self.ov.wns[w] = n;
    }
    fn set_cost(&mut self, i: usize, c: f64, q: f64) {
        let ov = &mut *self.ov;
        if ov.cstamp[i] != ov.epoch {
            ov.cstamp[i] = ov.epoch;
            ov.costs.push(i);
        }
        ov.cval[i] = (c, q);
    }
    fn put_end_site(&mut self, key: EndKey, v: Site) {
        self.ov.sites.insert(key, v);
        self.ov.fresh.push((key, v));
    }
    fn viapos(&mut self, t0: std::time::Instant) {
        self.ov.vp_n += 1;
        self.ov.vp_ns += t0.elapsed().as_nanos() as u64;
    }
    fn abandoned(&self) -> bool {
        (self.stop)()
    }
}

/// A wire's via layout as a kept candidate left it: `(wire, intervals, traversals, vias without a site)`.
type WireWrite = (WireId, Vec<(usize, usize)>, Vec<usize>, usize);

/// What a kept candidate writes into the master (copied from its overlay).
struct Writes {
    passes: Vec<(usize, i32, Disc)>,
    wires: Vec<WireWrite>,
    costs: Vec<(usize, f64, f64)>,
}

/// A candidate's result.
struct CandOut {
    keep: bool,
    /// The price change (the keep test's left side; folded into the stream fingerprint).
    delta: f64,
    /// End sites it computed, in order (replayed by the scan).
    fresh: Vec<(EndKey, Site)>,
    writes: Option<Writes>,
    move_ns: u64,
    vp_n: u64,
    vp_ns: u64,
}

/// Cost of the layer changes and per-event extras of wire `w` with events `ev` (`wire_cost`).
fn wire_cost_ev(ev: &[LEv], w: WireId, ctx: &Around) -> f64 {
    let t: f64 = ev.windows(2).map(|x| (ctx.trans)(x[0].2 as usize, x[1].2 as usize)).sum();
    t + ev.iter().enumerate().map(|(i, e)| (ctx.unary)(w, i, e.2 as usize)).sum::<f64>()
}

/// Evaluate candidate `mv` on `base` through `ov` (the serial `try_move_in` with `strict` false
/// and no cut model, the same operations in the same order); `None` when it was abandoned.
#[allow(clippy::too_many_arguments)]
fn evaluate(fx: &Frame, base: &Arrays, ov: &mut Overlay, mid: Option<&FxHashMap<EndKey, Site>>, evl: &[Vec<LEv>], mv: &[((WireId, usize), i32)], ctx: &Around, stop: &(dyn Fn() -> bool + Sync)) -> Option<CandOut> {
    let t0 = std::time::Instant::now();
    let st = fx.st;
    ov.begin(st.passes.len(), st.wires.len(), base.cost.len());
    let mut wires: Vec<WireId> = mv.iter().map(|x| x.0 .0).collect();
    wires.sort_unstable();
    wires.dedup();
    let old_wc: f64 = wires.iter().map(|&w| wire_cost_ev(&evl[w], w, ctx)).sum();
    // The touched wires' events with the move's labels (the serial loop sets them in `evl`).
    let mut evs: Vec<Vec<LEv>> = wires.iter().map(|&w| evl[w].clone()).collect();
    for &((w, i), l) in mv {
        let j = wires.binary_search(&w).expect("a wire of the move");
        evs[j][i].2 = l;
    }
    let new_wc: f64 = wires.iter().zip(&evs).map(|(&w, ev)| wire_cost_ev(ev, w, ctx)).sum();
    let old_ns: usize = wires.iter().map(|&w| base.nosite(w)).sum();
    let mut view = View { base, ov, mid, stop };
    for (&w, ev) in wires.iter().zip(&evs) {
        fx.relayout(&mut view, w, ev);
        if view.abandoned() {
            return None;
        }
    }
    let new_ns: usize = wires.iter().map(|&w| view.nosite(w)).sum();
    let ov = &mut *view.ov;
    // The serial undo log's entries, ascending (its BTreeMap): the price before is the master's.
    ov.costs.sort_unstable();
    let ew = 1.0;
    let d_cut = 0.0;
    let d_oc: f64 = ov.costs.iter().map(|&i| ew * (ov.cval[i].0 - base.cost[i]) + (ov.cval[i].1 - base.sq[i])).sum::<f64>() + fx.price() * (new_ns as f64 - old_ns as f64) + d_cut;
    let keep = super::refine::keep_rule(new_wc, old_wc, d_oc, ctx.add_vias);
    let writes = keep.then(|| Writes {
        passes: ov.passes.iter().map(|&p| (p, ov.play[p], ov.pdisc[p])).collect(),
        wires: ov.wires.iter().map(|&w| (w, ov.wiv[w].clone(), ov.wks[w].clone(), ov.wns[w])).collect(),
        costs: ov.costs.iter().map(|&i| (i, ov.cval[i].0, ov.cval[i].1)).collect(),
    });
    Some(CandOut { keep, delta: (new_wc - old_wc) + d_oc, fresh: std::mem::take(&mut ov.fresh), writes, move_ns: t0.elapsed().as_nanos() as u64, vp_n: ov.vp_n, vp_ns: ov.vp_ns })
}

/// Copy a kept candidate's writes and labels into the master. O(writes).
fn commit(a: &mut Arrays, evl: &mut [Vec<LEv>], w: Writes, mv: &[((WireId, usize), i32)]) {
    for (p, l, d) in w.passes {
        a.pass_layer[p] = l;
        a.pass_disc[p] = d;
    }
    for (wi, iv, ks, ns) in w.wires {
        a.via_iv[wi] = iv;
        a.via_k[wi] = ks;
        a.nosite[wi] = ns;
    }
    for (i, c, q) in w.costs {
        a.cost[i] = c;
        a.sq[i] = q;
    }
    for &((wi, i), l) in mv {
        evl[wi][i].2 = l;
    }
}

/// Whether two end sites are the same bit for bit.
fn same_site(a: &Site, b: &Site) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some((k, p)), Some((k2, p2))) => k == k2 && p.x.to_bits() == p2.x.to_bits() && p.y.to_bits() == p2.y.to_bits(),
        _ => false,
    }
}

/// One slot of an epoch's results.
enum Slot {
    Pending,
    Done(CandOut),
    Dropped,
}

/// The epoch's shared generator and results.
struct Epoch {
    look: Cursor,
    gen: Vec<Mv>,
    res: Vec<Slot>,
    done: bool,
    /// Tests ([`lanes::with_window`]): the lanes evaluate at least this many candidates of the
    /// epoch, also past a kept one (0 = stop at the first kept one).
    window: usize,
}

/// Counters of the parallel refinement (process-wide, never read by the routing): candidates
/// evaluated by lanes and dropped after a kept one (wasted), evaluated again for an end-site
/// fill (redone), epochs and lane-epochs.
pub mod stats {
    use std::sync::atomic::AtomicU64;
    pub static WASTED: AtomicU64 = AtomicU64::new(0);
    pub static REDONE: AtomicU64 = AtomicU64::new(0);
    pub static EPOCHS: AtomicU64 = AtomicU64::new(0);
    pub static LANE_EPOCHS: AtomicU64 = AtomicU64::new(0);
}

/// Tests (tests/par_refine_dense.rs): with `AT` > 0 the run thread's lane sets `FLAG` (the run's
/// cancel flag) right after it evaluated its `AT`-th candidate (counted process-wide in `SEEN`), so
/// a Cancel arrives inside an epoch at a known candidate. Off (`AT` = 0): one relaxed load per
/// candidate.
pub mod test_cancel {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
    use std::sync::{Arc, Mutex};
    pub static AT: AtomicU64 = AtomicU64::new(0);
    pub static SEEN: AtomicU64 = AtomicU64::new(0);
    pub static FLAG: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

    pub(crate) fn tick() {
        let at = AT.load(Relaxed);
        if at > 0 && SEEN.fetch_add(1, Relaxed) + 1 == at {
            if let Some(f) = FLAG.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
                f.store(true, Relaxed);
            }
        }
    }
}

/// The lane loop: take the next candidate of the stream, evaluate it, report; until the
/// generator ends, a candidate before the next one to take was kept, or the run stops.
#[allow(clippy::too_many_arguments)]
fn lane(fx: &Frame, base: &Arrays, evl: &[Vec<LEv>], ctx: &Around, ep: &Mutex<Epoch>, min_keep: &AtomicUsize, stopping: &AtomicBool, ov: &mut Overlay, run_thread: bool) {
    let lock = || ep.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    loop {
        if run_thread && crate::control::stop_requested() {
            stopping.store(true, Ordering::Relaxed);
        }
        let (seq, mv, window) = {
            let mut g = lock();
            let seq = g.gen.len();
            if g.done || (seq > min_keep.load(Ordering::Acquire) && seq >= g.window) || stopping.load(Ordering::Relaxed) {
                return;
            }
            let g = &mut *g;
            match g.look.next(fx, base, evl, ctx) {
                Some(mv) => {
                    g.gen.push(mv.clone());
                    g.res.push(Slot::Pending);
                    (seq, mv, g.window)
                }
                None => {
                    g.done = true;
                    return;
                }
            }
        };
        let stop = || stopping.load(Ordering::Relaxed) || (min_keep.load(Ordering::Relaxed) < seq && seq >= window);
        let out = evaluate(fx, base, ov, None, evl, &mv, ctx, &stop);
        if run_thread {
            test_cancel::tick();
        }
        let mut g = lock();
        g.res[seq] = match out {
            Some(o) => {
                if o.keep {
                    min_keep.fetch_min(seq, Ordering::AcqRel);
                }
                Slot::Done(o)
            }
            None => Slot::Dropped,
        };
    }
}

thread_local! {
    /// The run thread's overlays, reused across refinements.
    static OVERLAYS: std::cell::RefCell<Vec<Overlay>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The refinement's candidate loop on lanes (see the module doc): the same kept moves, in the same
/// order, as the serial loop. O(candidates x their cost / lanes) + the scans.
pub(crate) fn run(ctx: &Around, cm: &mut CapModel, evl: &mut [Vec<LEv>], st_out: &mut RefineStats) {
    let mut cur = Cursor::new();
    let mut ovs = OVERLAYS.with(|o| std::mem::take(&mut *o.borrow_mut()));
    loop {
        if crate::control::stop_requested() {
            break;
        }
        let lease = lanes::lease();
        let w = lease.lanes().max(1);
        if ovs.len() < w {
            ovs.resize_with(w, Overlay::default);
        }
        let (fx, base) = cm.parts();
        let ep = Mutex::new(Epoch { look: cur.clone(), gen: Vec::new(), res: Vec::new(), done: false, window: lanes::window() });
        let (min_keep, stopping) = (AtomicUsize::new(usize::MAX), AtomicBool::new(false));
        {
            let evr: &[Vec<LEv>] = evl;
            let (first, rest) = ovs[..w].split_at_mut(1);
            if rest.is_empty() {
                lane(fx, base, evr, ctx, &ep, &min_keep, &stopping, &mut first[0], true);
            } else {
                let (ep, min_keep, stopping) = (&ep, &min_keep, &stopping);
                rayon::in_place_scope(|s| {
                    for ov in rest.iter_mut() {
                        s.spawn(move |_| lane(fx, base, evr, ctx, ep, min_keep, stopping, ov, false));
                    }
                    lane(fx, base, evr, ctx, ep, min_keep, stopping, &mut first[0], true);
                });
            }
        }
        drop(lease);
        let ep = ep.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
        let stopped = stopping.load(Ordering::Relaxed);
        if meter::on() {
            stats::EPOCHS.fetch_add(1, Ordering::Relaxed);
            stats::LANE_EPOCHS.fetch_add(w as u64, Ordering::Relaxed);
        }
        // Scan in stream order: replay the end-site fills, find the first kept candidate.
        let mut fills: FxHashMap<EndKey, Site> = crate::fxhash::map();
        let mut kept: Option<(usize, CandOut)> = None;
        let mut scanned = 0;
        for (seq, slot) in ep.res.into_iter().enumerate() {
            let Slot::Done(mut out) = slot else { break };
            if out.fresh.iter().any(|(k, v)| fills.get(k).is_some_and(|f| !same_site(f, v))) {
                // An earlier candidate of the epoch filled one of its keys with another value:
                // evaluate it again as the serial loop would (with those fills).
                let (fx, base) = cm.parts();
                out = evaluate(fx, base, &mut ovs[0], Some(&fills), evl, &ep.gen[seq], ctx, &|| false).expect("an evaluation without a stop ends");
                if meter::on() {
                    stats::REDONE.fetch_add(1, Ordering::Relaxed);
                }
            }
            for (k, v) in std::mem::take(&mut out.fresh) {
                fills.entry(k).or_insert(v);
            }
            if meter::on() {
                meter::MOVES.fetch_add(1, Ordering::Relaxed);
                meter::MOVE_NS.fetch_add(out.move_ns, Ordering::Relaxed);
                meter::VIAPOS.fetch_add(out.vp_n, Ordering::Relaxed);
                meter::VIAPOS_NS.fetch_add(out.vp_ns, Ordering::Relaxed);
            }
            let (fx, base) = cm.parts();
            let mv = cur.next(fx, base, evl, ctx).expect("the cursor replays the epoch's stream");
            assert!(mv == ep.gen[seq], "par-refine: the cursor's replay differs from the epoch's stream at candidate {seq}");
            scanned = seq + 1;
            st_out.evals += 1;
            st_out.stream = super::refine::stream_fold(st_out.stream, &mv, out.delta, out.keep);
            if out.keep {
                kept = Some((seq, out));
                break;
            }
        }
        if meter::on() {
            stats::WASTED.fetch_add((ep.gen.len() - scanned) as u64, Ordering::Relaxed);
        }
        for (k, v) in fills {
            cm.a.end_sites.entry(k).or_insert(v);
        }
        match kept {
            Some((seq, out)) => {
                let wr = out.writes.expect("a kept candidate's writes");
                commit(&mut cm.a, evl, wr, &ep.gen[seq]);
                cur.accepted();
                st_out.moves += 1;
            }
            None if stopped || (ep.done && scanned == ep.gen.len()) => break,
            None => {}
        }
    }
    OVERLAYS.with(|o| *o.borrow_mut() = ovs);
}

/// Where the lanes come from: the run's spare compute slots (`[optimize] refine_lanes = "auto"`),
/// or a fixed count. Set per run thread by the caller (the engine's run loop); without it the
/// refinement has one lane.
pub mod lanes {
    use std::sync::Arc;

    /// A pool of compute slots the lanes borrow (the engine's run slots, `core.rs` `Slots`).
    pub trait Pool: Send + Sync {
        /// Take up to `k` free slots without waiting (none while a run waits for one); how many.
        fn try_take(&self, k: usize) -> usize;
        /// Give `k` slots back.
        fn give(&self, k: usize);
    }

    /// How many lanes a refinement gets.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum Mode {
        /// One lane plus the spare slots of the pool (at most the rayon threads).
        Auto,
        /// This many lanes (1 = the serial loop), whatever else runs.
        Fixed(usize),
    }

    type Ctx = (Mode, Option<Arc<dyn Pool>>);

    thread_local! {
        static CTX: std::cell::RefCell<Option<Ctx>> = const { std::cell::RefCell::new(None) };
    }

    /// Runs `f` with `mode` and `pool` as this thread's lane source; the one before comes back
    /// after `f`, also when it panics. O(1) + `f`.
    pub fn with<R>(mode: Mode, pool: Option<Arc<dyn Pool>>, f: impl FnOnce() -> R) -> R {
        struct Restore(Option<Ctx>);
        impl Drop for Restore {
            fn drop(&mut self) {
                let prev = self.0.take();
                CTX.with(|c| *c.borrow_mut() = prev);
            }
        }
        let _r = Restore(CTX.with(|c| c.replace(Some((mode, pool)))));
        f()
    }

    thread_local! {
        static WINDOW: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// Tests (`Options::refine_test_window`): runs `f` with every epoch of this thread evaluating
    /// at least `k` candidates, also past a kept one, so the scan meets later kept candidates and
    /// more end-site fills (the speculation a slow first lane makes, without the timing). The kept
    /// moves do not change (only the first kept one is committed). O(1) + `f`.
    pub fn with_window<R>(k: usize, f: impl FnOnce() -> R) -> R {
        struct Restore(usize);
        impl Drop for Restore {
            fn drop(&mut self) {
                WINDOW.with(|c| c.set(self.0));
            }
        }
        let _r = Restore(WINDOW.with(|c| c.replace(k)));
        f()
    }

    /// This thread's test window (0 = none).
    pub(crate) fn window() -> usize {
        WINDOW.with(|c| c.get())
    }

    /// The `[optimize] refine_lanes` option as a mode: 0 = auto, n = n lanes.
    pub fn mode_of(n: usize) -> Mode {
        if n == 0 {
            Mode::Auto
        } else {
            Mode::Fixed(n)
        }
    }

    /// The lanes of one epoch; the borrowed slots go back when it is dropped.
    pub struct Lease {
        n: usize,
        taken: usize,
        pool: Option<Arc<dyn Pool>>,
    }

    impl Lease {
        pub fn lanes(&self) -> usize {
            self.n
        }
    }

    impl Drop for Lease {
        fn drop(&mut self) {
            if let (Some(p), true) = (self.pool.as_ref(), self.taken > 0) {
                p.give(self.taken);
            }
        }
    }

    /// The lanes for the next epoch on this thread. O(1).
    pub fn lease() -> Lease {
        let ctx = CTX.with(|c| c.borrow().clone());
        match ctx {
            Some((Mode::Fixed(n), _)) => Lease { n: n.max(1), taken: 0, pool: None },
            Some((Mode::Auto, Some(p))) => {
                let cap = rayon::current_num_threads().max(1);
                let taken = p.try_take(cap - 1);
                Lease { n: 1 + taken, taken, pool: Some(p) }
            }
            _ => Lease { n: 1, taken: 0, pool: None },
        }
    }
}
