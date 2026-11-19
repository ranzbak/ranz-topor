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
use super::pack::{infl_dir, layer_items, Disc, PackItem, Packer};
use super::cutmodel::CutPrice;
use super::state::{State, TriId, WireId};
use super::viacut::{self, ViaGeom};
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
    /// The serial in-place loop of before par-refine (try, undo) instead of the lane executor
    /// ([`super::refine_par`]); the tests' reference. The cut model always takes it.
    pub reference: bool,
}

/// What the refinement did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RefineStats {
    /// Over-full edge-layers (demand model) before and after.
    pub before: usize,
    pub after: usize,
    /// Moves kept.
    pub moves: usize,
    /// par-refine fingerprint (tests and the plan metrics' `move_cost.refine_stream`): the
    /// candidates evaluated, a hash of their stream in order with each keep decision, and an
    /// order-free hash of the end-site memo at the end. The lane executor promises all three
    /// equal to the serial loop's, not only the labels.
    pub evals: usize,
    pub stream: u64,
    pub memo: u64,
    /// The final capacity model's measure (per-edge model only), for the plan's measure to take.
    pub fin: Option<FinalCap>,
}

/// One step of [`RefineStats::stream`]: the candidate `mv`, its price change `delta` (bit for
/// bit: the lanes promise the serial summation order) and whether it was kept. O(|mv|).
pub(crate) fn stream_fold(h: u64, mv: &[((WireId, usize), i32)], delta: f64, keep: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut x = std::collections::hash_map::DefaultHasher::new();
    (h, mv, delta.to_bits(), keep).hash(&mut x);
    x.finish()
}

/// The keep test of a candidate (serial loop and lanes alike): the price with the wires' layer
/// changes (`new_wc` vs `old_wc`) and the capacity change `d_oc` drops by more than [`EPS`], and
/// without `add_vias` the wires' own price does not grow. O(1).
pub(crate) fn keep_rule(new_wc: f64, old_wc: f64, d_oc: f64, add_vias: bool) -> bool {
    (new_wc - old_wc) + d_oc < -EPS && (add_vias || new_wc <= old_wc + EPS)
}

/// Tests (par-refine): with [`shadow::ON`] every lane refinement ([`refine`] without the cut model
/// and without `reference`) also runs the serial reference loop on its own model of the same
/// labels and panics unless labels, model arrays, end-site memo, evaluations, stream and kept
/// moves are equal bit for bit (a refinement a stop cut is not compared). Process-wide; a test
/// binary sets it for itself. Off = one relaxed load per refinement.
/// CM1 check counters (tw4 T2): with [`viacheck::ON`] (or `TOPO_VIACHECK` set) every refinement
/// with `[capacity] via_cuts` ends with [`CapModel::via_check`] and adds its result here (tests).
#[doc(hidden)]
pub mod viacheck {
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    pub static ON: AtomicBool = AtomicBool::new(false);
    /// Via entries whose stored price differs from a fresh pricing, and the priced entries.
    pub static BAD: AtomicUsize = AtomicUsize::new(0);
    pub static CHECKED: AtomicUsize = AtomicUsize::new(0);
    /// tw4 fix round 2 (verifier F1): with [`FOCUS_ON`] every local judgement under via cuts is priced again with
    /// every model wire in focus; the change (after - before, count and graded) must be the same. Judgements
    /// compared, and those that differ.
    pub static FOCUS_ON: AtomicBool = AtomicBool::new(false);
    pub static FOCUS_CHECKED: AtomicUsize = AtomicUsize::new(0);
    pub static FOCUS_BAD: AtomicUsize = AtomicUsize::new(0);
}

pub mod shadow {
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
    pub static ON: AtomicBool = AtomicBool::new(false);
    /// Refinements compared, and those of them that kept at least one move.
    pub static CHECKED: AtomicU64 = AtomicU64::new(0);
    pub static CHECKED_MOVED: AtomicU64 = AtomicU64::new(0);
    /// The shadow's own negative test: `k` > 0 changes item `k` of [`FIELDS`] in the reference's
    /// result before the comparison, which must then panic naming that item. 0 = off.
    pub static FAULT: AtomicUsize = AtomicUsize::new(0);
    /// What the comparison checks, in its order (`over-full after` follows from the prices, so a
    /// changed price is reported as `prices`; it has no fault of its own).
    pub const FIELDS: [&str; 12] = [
        "labels",
        "evaluations",
        "stream",
        "kept moves",
        "pass layers",
        "pass discs",
        "via intervals",
        "via traversals",
        "vias without a site",
        "prices",
        "end-site memo",
        "over-full after",
    ];
}

/// [`shadow::FAULT`]: changes item `k` (1-based, of [`shadow::FIELDS`]) of a result. O(1).
fn shadow_fault(k: usize, cm: &mut Arrays, evl: &mut [Vec<LEv>], st: &mut RefineStats) {
    match k {
        1 => match evl.iter_mut().find(|e| !e.is_empty()) {
            Some(e) => e[0].2 += 1,
            None => {
                if let Some(e) = evl.first_mut() {
                    e.push((0, 0.0, 0));
                }
            }
        },
        2 => st.evals += 1,
        3 => st.stream ^= 1,
        4 => st.moves += 1,
        5 => cm.pass_layer.push(-1),
        6 => cm.pass_disc.push([0.0, 0.0]),
        7 => cm.via_iv.push(Vec::new()),
        8 => cm.via_k.push(Vec::new()),
        9 => cm.nosite.push(0),
        10 => cm.cost.push(1.0),
        11 => {
            cm.end_sites.insert((usize::MAX, 0, 0, 0, 0), None);
        }
        _ => {}
    }
}

/// [`shadow`]: the first difference between the lanes' result and the reference's, if any.
fn shadow_diff(a: (&CapModel, &[Vec<LEv>], &RefineStats), b: (&CapModel, &[Vec<LEv>], &RefineStats)) -> Option<String> {
    let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<u64>>();
    let disc = |v: &[Disc]| v.iter().map(|d| (d[0].to_bits(), d[1].to_bits())).collect::<Vec<_>>();
    let labels = |e: &[Vec<LEv>]| e.iter().map(|w| w.iter().map(|&(k, s, l)| (k, s.to_bits(), l)).collect::<Vec<_>>()).collect::<Vec<_>>();
    let (ca, ea, sa) = a;
    let (cb, eb, sb) = b;
    let checks: [bool; 12] = [
        labels(ea) == labels(eb),
        sa.evals == sb.evals,
        sa.stream == sb.stream,
        sa.moves == sb.moves,
        ca.pass_layer == cb.pass_layer,
        disc(&ca.pass_disc) == disc(&cb.pass_disc),
        ca.via_iv == cb.via_iv,
        ca.via_k == cb.via_k,
        ca.nosite == cb.nosite,
        bits(&ca.cost) == bits(&cb.cost) && bits(&ca.sq) == bits(&cb.sq),
        memo_hash(&ca.end_sites) == memo_hash(&cb.end_sites) && ca.end_sites.len() == cb.end_sites.len(),
        ca.overfull() == cb.overfull(),
    ];
    let first = checks.iter().position(|ok| !ok)?;
    Some(format!("{} (lanes: {} evaluations, {} kept; reference: {} evaluations, {} kept)", shadow::FIELDS[first], sa.evals, sa.moves, sb.evals, sb.moves))
}

/// [`RefineStats::memo`]: an order-free hash of the end-site memo. O(entries).
fn memo_hash(m: &EndSites) -> u64 {
    use std::hash::{Hash, Hasher};
    m.iter().fold(m.len() as u64, |acc, (k, v)| {
        let mut x = std::collections::hash_map::DefaultHasher::new();
        (k, v.map(|(i, p)| (i, p.x.to_bits(), p.y.to_bits()))).hash(&mut x);
        acc.wrapping_add(x.finish())
    })
}

/// par-refine item 3 (`[optimize] measure_reuse`): the violations of the capacity model the
/// refinement ended with ([`CapModel::violations`]) and what they belong to, so the plan's
/// measure takes them instead of building the model again from the same labels. The two can
/// differ in the last bits and in the via positions: the refinement's model keeps the via
/// positions and end sites of the order its moves came in, a new build lays every wire out once
/// in wire order. Valid for the packer at address `packer`, the `bottom` layer, the price bits
/// `price` and the state + labels whose [`cap_key`] is `key`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FinalCap {
    pub packer: usize,
    pub bottom: i32,
    pub price: u64,
    pub key: u64,
    pub viol: (usize, f64),
}

/// A hash of what a capacity model is built from besides the packer: the live wires (net,
/// width, clearance, terminals, turns, passes with their edges, triangles), the pass order of
/// every edge and the labels, bit for bit. O(passes + events).
pub fn cap_key(st: &State, evl: &[Vec<LEv>]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = crate::fxhash::FxHasher::default();
    (st.wires.len(), st.passes.len()).hash(&mut h);
    st.edge_order.hash(&mut h);
    for (w, wi) in st.wires.iter().enumerate() {
        if !wi.alive {
            continue;
        }
        (w, wi.net, wi.w.to_bits(), wi.s.to_bits(), wi.a, wi.b, wi.pa, wi.pb, &wi.turns, &wi.passes, &wi.tris).hash(&mut h);
        for &p in &wi.passes {
            (st.passes[p].wire, st.passes[p].edge).hash(&mut h);
        }
        let ev = evl.get(w).map_or(&[][..], |v| &v[..]);
        ev.len().hash(&mut h);
        for &(k, s, l) in ev {
            (k, s.to_bits(), l).hash(&mut h);
        }
    }
    h.finish()
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
type EndSites = crate::fxhash::FxHashMap<(WireId, usize, u64, usize, u64), Option<(usize, Vec2)>>;

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
    /// Build memo (par-refine item 2): edge-layers whose packing numbers came from an earlier
    /// build (hits) and those packed again (misses).
    pub static MEMO_HIT: AtomicU64 = AtomicU64::new(0);
    pub static MEMO_MISS: AtomicU64 = AtomicU64::new(0);
    /// tw4 T2 cycle cost: time and calls of the via-cut pricing (CM1: `reprice_vias` and the
    /// build's via prices) and of the model builds (every `CapModel` build, the judge's included).
    pub static VIACUT_NS: AtomicU64 = AtomicU64::new(0);
    pub static VIACUT: AtomicU64 = AtomicU64::new(0);
    pub static BUILD_NS: AtomicU64 = AtomicU64::new(0);
    pub static BUILDS: AtomicU64 = AtomicU64::new(0);
    thread_local! {
        static OFF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    /// The meters count on this thread (not inside [`suspended`]).
    pub fn on() -> bool {
        !OFF.with(|c| c.get())
    }
    /// Runs `f` with the meters off on this thread: the M1 compass's read-only cut test must not
    /// enter the plan metrics of later cycles (the cut code runs on the calling thread).
    pub fn suspended<T>(f: impl FnOnce() -> T) -> T {
        let was = OFF.with(|c| c.replace(true));
        let r = f();
        OFF.with(|c| c.set(was));
        r
    }
    pub fn add(ns: &AtomicU64, n: &AtomicU64, t: std::time::Instant) {
        if !on() {
            return;
        }
        ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        n.fetch_add(1, Ordering::Relaxed);
    }
    /// `(moves, us per move, via positions, us per via position)`.
    pub fn read() -> (u64, f64, u64, f64) {
        let (mn, m) = (MOVE_NS.load(Ordering::Relaxed), MOVES.load(Ordering::Relaxed));
        let (vn, v) = (VIAPOS_NS.load(Ordering::Relaxed), VIAPOS.load(Ordering::Relaxed));
        (m, mn as f64 / 1e3 / m.max(1) as f64, v, vn as f64 / 1e3 / v.max(1) as f64)
    }
    /// tw4 T2: `(via-cut pricings, ms in them, model builds, ms in them)`.
    pub fn read_cm1() -> (u64, f64, u64, f64) {
        let ms = |a: &AtomicU64| a.load(Ordering::Relaxed) as f64 / 1e6;
        (VIACUT.load(Ordering::Relaxed), ms(&VIACUT_NS), BUILDS.load(Ordering::Relaxed), ms(&BUILD_NS))
    }
    /// Cut model split: `(wire flushes, ms stretches, parts walked, ms walks, cuts priced, ms install)`.
    pub fn read_split() -> (u64, f64, u64, f64, u64, f64) {
        let ms = |a: &AtomicU64| a.load(Ordering::Relaxed) as f64 / 1e6;
        (FLUSHES.load(Ordering::Relaxed), ms(&STRETCH_NS), PARTS.load(Ordering::Relaxed), ms(&PART_NS), CUTS_PRICED.load(Ordering::Relaxed), ms(&INSTALL_NS))
    }
}

/// A pass whose layout changed: `(pass, layer before, disc before)`.
type Changed = (usize, i32, Disc);

/// The layers of an edge whose pack items change when one of its passes goes from layer `ol`,
/// disc `od` to layer `nl_`, disc `nd`, as a bit set over the `nl` layers ([`layer_items`]: a
/// pass packs its track and disc on its own layer and its disc alone on every other layer). Same
/// disc (bits): only the old and the new layer; with a disc and a layer of -1 (a dead wire's
/// pass), or a changed disc: every layer. O(1).
fn layers_changed(nl: usize, ol: i32, od: Disc, nl_: i32, nd: Disc) -> u64 {
    let all = if nl >= 64 { u64::MAX } else { (1u64 << nl) - 1 };
    let bit = |l: i32| if l >= 0 && (l as usize) < 64 { 1u64 << l } else { 0 };
    let same = od[0].to_bits() == nd[0].to_bits() && od[1].to_bits() == nd[1].to_bits();
    if !same || (od[0].max(od[1]) > 0.0 && (ol < 0 || nl_ < 0)) {
        return all;
    }
    (bit(ol) | bit(nl_)) & all
}

/// Order of a via traversal: (no site, overflow + squeeze change in um-mm, other wires in the
/// triangle, -inradius, traversal).
type SiteKey = (bool, i64, usize, i64, usize);

/// "Not filled yet" in [`CapModel::margins`] (a NaN pattern no margin takes: margins are finite).
const MARGIN_UNSET: u64 = u64::MAX;

/// Margin table slots per pass: per end, `nl` track margins, `nl` disc-alone margins and the via
/// disc's margin over all layers.
fn margin_slots(nl: usize) -> usize {
    2 * (2 * nl + 1)
}

/// Packing numbers of an edge-layer: `(over, via squeeze price, pitch)` ([`CapModel::numbers_in`]).
pub type EdgeNumbers = (f64, f64, f64);

/// The build memo of a packer ([`CapModel::price_all`]): per edge-layer the items priced there
/// last and their packing numbers. Valid for the packer's projection only.
#[derive(Default)]
pub struct EdgeMemo {
    nl: usize,
    ent: Vec<MemoEnt>,
}

#[derive(Default)]
struct MemoEnt {
    items: Vec<PackItem>,
    nums: Option<EdgeNumbers>,
}

/// Whether two item lists are equal bit for bit (every field; `-0.0` and `0.0` differ). O(items).
fn same_items(a: &[PackItem], b: &[PackItem]) -> bool {
    let bits = |i: &PackItem| (i.net, i.w.to_bits(), i.s.to_bits(), i.infl.to_bits(), i.disc[0].to_bits(), i.disc[1].to_bits());
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| bits(x) == bits(y))
}

/// Per-thread scratch of [`CapModel::eval`].
#[derive(Default)]
struct EvalScratch {
    items: Vec<PackItem>,
    /// The pass of each item and whether it is the pass's track (else its disc alone).
    ids: Vec<(usize, bool)>,
    lo: Vec<f64>,
    hi: Vec<f64>,
    req: Vec<f64>,
    rmax: Vec<f64>,
}

thread_local! {
    static EVAL_SCRATCH: std::cell::RefCell<EvalScratch> = std::cell::RefCell::new(EvalScratch::default());
}

/// The static part of a [`CapModel`] (par-refine §3): the borrowed state, packer and directions
/// and the per-pass tables, read by every lane of the parallel refinement and written by none
/// (the margin table's atomics are a memo of the packer's pure margins). The layout and pricing
/// code lives here, generic over where the labelling-dependent arrays are ([`Mem`]: the model's
/// own arrays in place, or a lane's overlay over them).
pub struct Frame<'a> {
    pub(crate) st: &'a State<'a>,
    pub(crate) packer: &'a Packer<'a>,
    /// Crossing direction of every pass from its wire's taut path ([`Packer::taut_dirs`]).
    dirs: &'a [Vec<Vec2>],
    pub(crate) nl: usize,
    price: f64,
    pub(crate) bottom: i32,
    /// Position of every pass in its wire.
    pass_k: Vec<usize>,
    /// Per pass of a live wire: its pack item without a disc (net, width, clearance, oblique
    /// factor of its taut direction), as [`CapModel::items`] makes it; filled at the build
    /// (static for the model's life: the state and the directions are borrowed).
    item0: Vec<PackItem>,
    /// Per wire: the via disc diameter its passes get ([`Packer::via_d`] of its net).
    wire_d: Vec<f64>,
    /// Per pass: its end margins ([`Packer::margin`] of its edge, both ends, every layer, at its
    /// track width and as a disc alone (width 0), and [`Packer::margin_all`] of its wire's via
    /// disc), filled when first asked ([`MARGIN_UNSET`] until then), [`margin_slots`] per pass.
    /// The values are those of the packer's (pure) cache, so an entry read here equals the
    /// packer's answer bit for bit, whichever lane filled it.
    margins: Vec<std::sync::atomic::AtomicU64>,
    /// CM1 (tw4 T2, `[capacity] via_cuts`): `cost[vbase + w]` is the over-full price of the gap
    /// and via-to-via cuts of wire `w`'s planned vias (weight 1 in both capacity models; the undo
    /// log and a lane's overlay cover it like an edge-layer). `vbase` = edges x layers.
    pub(crate) vbase: usize,
    /// tw4 T2 cost cut (loop-local judge, [`CapModel::new_only_focus`]): only the wires `w` with
    /// `vfocus[w]` own priced via terms and have their vias swept for room in the build; the
    /// others' via terms are 0 and their vias stay where [`Frame::default_ks`] puts them, in both
    /// states of a judgement alike. `None` (every other model): all wires.
    pub(crate) vfocus: Option<Vec<bool>>,
    /// CM1: the gap cuts of a via with their crossers per (triangle, net, point bits): the static
    /// part from the packer's run-wide cache ([`Packer::via_static_of`]), the crossers from the
    /// state, fixed for the model's life. A memo (a pure function of its key), so the order lanes
    /// fill it in changes nothing.
    vgeo: viacut::GeoCache,
    /// CM1: per wire, the triangles that share a vertex with one of its triangles and the other
    /// wires that traverse them ([`Frame::nstar`]); static for the model's life.
    nstar: Vec<std::sync::OnceLock<std::sync::Arc<NStar>>>,
}

/// CM1: the neighbourhood of a wire's path ([`Frame::nstar`]).
struct NStar {
    /// Triangles that share a vertex with a triangle of the wire, ascending.
    tris: Vec<TriId>,
    /// The other live wires that traverse one of them, ascending.
    wires: Vec<WireId>,
}

/// CM1: a planned via's point and its triangle (`None`: no point, the wire has no via size).
pub(crate) type ViaPt = Option<(usize, Vec2)>;

/// The labelling-dependent arrays of a [`CapModel`] (its "master" in the parallel refinement:
/// the lanes read it, only the run thread writes it, between the lanes' epochs).
pub struct Arrays {
    /// Layer of every pass (-1: wire dead).
    pub pass_layer: Vec<i32>,
    /// Via disc diameter at every pass next to a via of its wire (0: none).
    pub pass_disc: Vec<Disc>,
    /// Per wire: the via intervals of its events and the traversal of each via.
    pub via_iv: Vec<Vec<(usize, usize)>>,
    pub via_k: Vec<Vec<usize>>,
    /// Per wire: its vias without a static site in their interval.
    pub(crate) nosite: Vec<usize>,
    /// 5.2: the site in an end traversal ([`end_site`]) of an interval without a traversal
    /// strictly between its events, per (wire, event a, event b) (`f64` fractions as bits); the
    /// state is borrowed for the model's life, so the geometry is fixed. A memo filled when first
    /// asked (its value depends on the planned vias of other nets at that moment, `site_taken`):
    /// the parallel refinement keeps the serial order of its fills (`refine_par`).
    pub(crate) end_sites: EndSites,
    /// Price of edge `e`, layer `l` at `e * nl + l`.
    pub(crate) cost: Vec<f64>,
    /// Via squeeze price of edge `e`, layer `l` (the `via_squeeze` factor, [`Packer::via_squeeze`]).
    pub(crate) sq: Vec<f64>,
    /// Undo log of `cost` / `sq` entries while a move is tried in place.
    log: Option<Vec<(usize, f64, f64)>>,
    /// CM1: per wire, the point of each planned via (the 5.2 end site of an interval without a
    /// traversal between its events, else the drawing's first site of its triangle); empty
    /// without `[capacity] via_cuts`.
    pub(crate) vpts: Vec<Vec<ViaPt>>,
}

impl Arrays {
    /// The edge-layer entries of `cost` / `sq` (the CM1 via entries follow them, one per wire).
    pub(crate) fn n_edge_layers(&self) -> usize {
        self.cost.len() - self.vpts.len()
    }
}

/// Key of [`Arrays::end_sites`].
pub(crate) type EndKey = (WireId, usize, u64, usize, u64);

/// Reads of the labelling-dependent arrays ([`Arrays`] itself, or a lane's overlay over them).
pub(crate) trait MemRead {
    fn layer(&self, p: usize) -> i32;
    fn disc(&self, p: usize) -> Disc;
    fn via_iv(&self, w: WireId) -> &[(usize, usize)];
    fn via_k(&self, w: WireId) -> &[usize];
    fn nosite(&self, w: WireId) -> usize;
    fn cost(&self, i: usize) -> (f64, f64);
    fn end_site(&self, key: &EndKey) -> Option<Option<(usize, Vec2)>>;
    /// CM1: the via points of `w` ([`Arrays::vpts`]).
    fn vpts(&self, w: WireId) -> &[ViaPt];
}

/// Reads and writes of the labelling-dependent arrays, with the hooks of the in-place model (the
/// cut model's repricing, the undo log, the meters) and of a lane (its candidate was dropped).
pub(crate) trait Mem: MemRead {
    fn set_pass(&mut self, p: usize, l: i32, d: Disc);
    fn set_via_iv(&mut self, w: WireId, iv: Vec<(usize, usize)>);
    fn set_via_k(&mut self, w: WireId, ks: &[usize]);
    fn set_nosite(&mut self, w: WireId, n: usize);
    /// Set the price of edge-layer `i` (called only when it changes).
    fn set_cost(&mut self, i: usize, c: f64, q: f64);
    fn put_end_site(&mut self, key: EndKey, v: Option<(usize, Vec2)>);
    /// CM1: set the via points of `w`.
    fn set_vpts(&mut self, w: WireId, pts: &[ViaPt]);
    /// Weight of the per-edge overflow price in the total: 0 when the cut model prices (the
    /// per-edge cost is then only kept for the reports).
    fn edge_w(&self) -> f64 {
        1.0
    }
    /// Wire `w` was laid out with events `ev` (the cut model reprices it at the next flush).
    fn applied(&mut self, _w: WireId, _ev: &[LEv]) {}
    /// Cut model: price the wires applied since the last call again; the change of the cut price.
    fn flush(&mut self, _fx: &Frame) -> f64 {
        0.0
    }
    /// One via position was tried, started at `t0` (the meters).
    fn viapos(&mut self, t0: std::time::Instant);
    /// The candidate this layout belongs to is no longer wanted (a lane's: an earlier one was
    /// kept, or the run stops); the layout may end early, its result is thrown away.
    fn abandoned(&self) -> bool {
        false
    }
}

impl MemRead for Arrays {
    fn layer(&self, p: usize) -> i32 {
        self.pass_layer[p]
    }
    fn disc(&self, p: usize) -> Disc {
        self.pass_disc[p]
    }
    fn via_iv(&self, w: WireId) -> &[(usize, usize)] {
        &self.via_iv[w]
    }
    fn via_k(&self, w: WireId) -> &[usize] {
        self.via_k.get(w).map_or(&[], |v| v.as_slice())
    }
    fn nosite(&self, w: WireId) -> usize {
        self.nosite[w]
    }
    fn cost(&self, i: usize) -> (f64, f64) {
        (self.cost[i], self.sq[i])
    }
    fn end_site(&self, key: &EndKey) -> Option<Option<(usize, Vec2)>> {
        self.end_sites.get(key).copied()
    }
    fn vpts(&self, w: WireId) -> &[ViaPt] {
        self.vpts.get(w).map_or(&[], |v| v.as_slice())
    }
}

/// The model's own arrays, written in place (the build, the in-place moves of [`try_move`] and
/// P4, the cut model, and the commit of a lane's candidate).
pub(crate) struct Master<'m, 'a> {
    pub(crate) a: &'m mut Arrays,
    cut: &'m mut Option<CutPrice<'a>>,
    evs: &'m mut Vec<Vec<LEv>>,
    pending: &'m mut Vec<WireId>,
}

impl MemRead for Master<'_, '_> {
    fn layer(&self, p: usize) -> i32 {
        self.a.layer(p)
    }
    fn disc(&self, p: usize) -> Disc {
        self.a.disc(p)
    }
    fn via_iv(&self, w: WireId) -> &[(usize, usize)] {
        self.a.via_iv(w)
    }
    fn via_k(&self, w: WireId) -> &[usize] {
        self.a.via_k(w)
    }
    fn nosite(&self, w: WireId) -> usize {
        self.a.nosite(w)
    }
    fn cost(&self, i: usize) -> (f64, f64) {
        self.a.cost(i)
    }
    fn end_site(&self, key: &EndKey) -> Option<Option<(usize, Vec2)>> {
        self.a.end_site(key)
    }
    fn vpts(&self, w: WireId) -> &[ViaPt] {
        self.a.vpts(w)
    }
}

impl Mem for Master<'_, '_> {
    fn set_pass(&mut self, p: usize, l: i32, d: Disc) {
        self.a.pass_layer[p] = l;
        self.a.pass_disc[p] = d;
    }
    fn set_via_iv(&mut self, w: WireId, iv: Vec<(usize, usize)>) {
        self.a.via_iv[w] = iv;
    }
    fn set_via_k(&mut self, w: WireId, ks: &[usize]) {
        let v = &mut self.a.via_k[w];
        v.clear();
        v.extend_from_slice(ks);
    }
    fn set_nosite(&mut self, w: WireId, n: usize) {
        self.a.nosite[w] = n;
    }
    fn set_cost(&mut self, i: usize, c: f64, q: f64) {
        let a = &mut *self.a;
        if let Some(log) = a.log.as_mut() {
            log.push((i, a.cost[i], a.sq[i]));
        }
        a.cost[i] = c;
        a.sq[i] = q;
    }
    fn put_end_site(&mut self, key: EndKey, v: Option<(usize, Vec2)>) {
        self.a.end_sites.insert(key, v);
    }
    fn set_vpts(&mut self, w: WireId, pts: &[ViaPt]) {
        let v = &mut self.a.vpts[w];
        v.clear();
        v.extend_from_slice(pts);
    }
    fn edge_w(&self) -> f64 {
        if self.cut.is_some() {
            0.0
        } else {
            1.0
        }
    }
    fn applied(&mut self, w: WireId, ev: &[LEv]) {
        if self.cut.is_some() {
            self.evs[w] = ev.to_vec();
            self.pending.push(w);
        }
    }
    fn flush(&mut self, fx: &Frame) -> f64 {
        let Some(cut) = self.cut.as_mut() else { return 0.0 };
        let mut ws = std::mem::take(self.pending);
        ws.sort_unstable();
        ws.dedup();
        let mut d = 0.0;
        for w in ws {
            let t0 = std::time::Instant::now();
            let (mut runs, vias) = cut.runs_of(fx.st, fx.packer, w, &self.evs[w], &self.a.via_k[w], fx.bottom);
            meter::add(&meter::STRETCH_NS, &meter::FLUSHES, t0);
            d += cut.set_wire(w, &mut runs, &vias);
        }
        d
    }
    fn viapos(&mut self, t0: std::time::Instant) {
        meter::add(&meter::VIAPOS_NS, &meter::VIAPOS, t0);
    }
}

/// Per-thread scratch of [`Frame::apply`] and [`Frame::reprice`].
#[derive(Default)]
struct LayScratch {
    pl: Vec<i32>,
    disc: Vec<Disc>,
    es: Vec<(usize, u64)>,
}

thread_local! {
    static LAY_SCRATCH: std::cell::RefCell<LayScratch> = std::cell::RefCell::new(LayScratch::default());
}

impl<'a> Frame<'a> {
    fn new(st: &'a State<'a>, packer: &'a Packer<'a>, dirs: &'a [Vec<Vec2>], bottom: i32, price: f64, only: Option<&[bool]>) -> Self {
        let nl = st.proj.n_layers;
        let mut pass_k = vec![0; st.passes.len()];
        for wi in st.wires.iter().filter(|wi| wi.alive) {
            for (k, &p) in wi.passes.iter().enumerate() {
                pass_k[p] = k;
            }
        }
        let pr = st.proj;
        let mut item0 = vec![PackItem::default(); st.passes.len()];
        for (w, wi) in st.wires.iter().enumerate().filter(|&(w, wi)| wi.alive && only.is_none_or(|o| o.get(w).copied().unwrap_or(false))) {
            for (k, &p) in wi.passes.iter().enumerate() {
                let e = st.passes[p].edge;
                item0[p] = PackItem { net: wi.net, w: st.pass_w.get(p).copied().unwrap_or(wi.w), s: wi.s, infl: infl_dir(pr, e, dirs[w][k]), disc: [0.0; 2] };
            }
        }
        let wire_d: Vec<f64> = st.wires.iter().map(|wi| packer.via_d(wi.net)).collect();
        let margins = (0..st.passes.len() * margin_slots(nl)).map(|_| std::sync::atomic::AtomicU64::new(MARGIN_UNSET)).collect();
        let vbase = pr.edges.len() * nl;
        let nstar = (0..st.wires.len()).map(|_| std::sync::OnceLock::new()).collect();
        Frame { st, packer, dirs, nl, price, bottom, pass_k, item0, wire_d, margins, vbase, vfocus: None, vgeo: std::sync::Mutex::new(crate::fxhash::map()), nstar }
    }

    /// The price of one over-full edge-layer (mm).
    pub(crate) fn price(&self) -> f64 {
        self.price
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
    fn rank<M: MemRead + ?Sized>(&self, m: &M, w: WireId, k: usize) -> (bool, usize, i64, usize) {
        let (st, pr) = (self.st, self.st.proj);
        let t = st.wires[w].tris[k.min(st.wires[w].tris.len() - 1)];
        let wi = &st.wires[w];
        let d = self.packer.via_d(wi.net);
        let site = d <= 0.0 || (self.packer.via_site(t, wi.net, 0.5 * d, wi.s) && !self.site_taken(m, w, t));
        let others = st.tri_trav[t].iter().filter(|&&(o, _)| o != w).count();
        (!site, others, -(pr.inradius(t) as i64), k)
    }

    /// Whether triangle `t` is taken for a via of `w` (M-C via room, Polubasov 2001 pp. 3-9: a via
    /// lies in its interval, at a site of its own): another net's wire already has a planned via
    /// in `t` and `t` cannot hold two discs (inradius below twice the via radius plus clearance).
    /// Derived from `via_k` alone, so the undo of a move restores it. O(traversals of `t`).
    fn site_taken<M: MemRead + ?Sized>(&self, m: &M, w: WireId, t: super::state::TriId) -> bool {
        if !self.packer.reserve_sites {
            return false;
        }
        let st = self.st;
        let net = st.wires[w].net;
        st.tri_trav[t].iter().any(|&(o, ko)| o != w && st.wires[o].net != net && m.via_k(o).contains(&ko)) && st.proj.inradius(t) < 2.0 * self.need_r(w)
    }

    /// The key of the end site of via interval `i` of `w` under events `ev`, and its two events;
    /// `None` without `[vias] between_crossings` or without such an interval.
    fn end_key(&self, w: WireId, ev: &[LEv], i: usize) -> Option<(EndKey, LEv, LEv)> {
        if !self.packer.via_between {
            return None;
        }
        let (a, b) = ev.windows(2).filter(|x| x[0].2 != x[1].2).nth(i).map(|x| (x[0], x[1]))?;
        Some(((w, a.0, a.1.to_bits(), b.0, b.1.to_bits()), a, b))
    }

    /// [`end_site`] of the interval between events `a` and `b` of `w` under the planned vias of `m`.
    fn end_site_raw<M: MemRead + ?Sized>(&self, m: &M, w: WireId, a: LEv, b: LEv) -> Option<(usize, Vec2)> {
        let wi = &self.st.wires[w];
        let d = self.packer.via_d(wi.net);
        let (pr, packer, net, s) = (self.st.proj, self.packer, wi.net, wi.s);
        let need = self.need_r(w);
        end_site(self.st, w, a, b, &|t, q| pr.inradius(t) >= need && !self.site_taken(m, w, t) && super::cutflow::point_free(pr, packer, q, net, s) >= 0.5 * d)
    }

    /// 5.2: the end-traversal site ([`end_site`]) of via interval `i` of `w` under events `ev`
    /// when the interval has no traversal strictly between its events (`[vias]
    /// between_crossings`); `None` otherwise or when there is none. Cached ([`Arrays::end_sites`]).
    fn end_site_of<M: Mem + ?Sized>(&self, m: &mut M, w: WireId, ev: &[LEv], i: usize) -> Option<(usize, Vec2)> {
        let (key, a, b) = self.end_key(w, ev, i)?;
        if let Some(x) = m.end_site(&key) {
            return x;
        }
        let x = self.end_site_raw(&*m, w, a, b);
        m.put_end_site(key, x);
        x
    }

    /// Via traversals by the drawing's preference alone (no room), keeping the traversal of a
    /// via whose interval is unchanged from `old` (intervals, traversals); a via of an interval
    /// with no traversal strictly between its events takes the traversal of its end site
    /// ([`Frame::end_site_of`]) when it has one.
    fn default_ks<M: Mem + ?Sized>(&self, m: &mut M, w: WireId, ev: &[LEv], iv: &[(usize, usize)], old: Option<OldVias>) -> Vec<usize> {
        let n = self.st.wires[w].passes.len();
        let between = if self.packer.via_between { via_ranges(ev, true).1 } else { Vec::new() };
        let mut ks: Vec<usize> = Vec::with_capacity(iv.len());
        for (i, &(ka, kb)) in iv.iter().enumerate() {
            let lo = ka.max(ks.last().copied().unwrap_or(0)).min(n);
            let hi = kb.min(n).max(lo);
            if between.get(i) == Some(&false) {
                if let Some((k, _)) = self.end_site_of(m, w, ev, i).filter(|&(k, _)| (lo..=hi).contains(&k)) {
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
            ks.push((lo..=hi).min_by_key(|&k| self.rank(&*m, w, k)).unwrap_or(lo));
        }
        ks
    }

    /// Set the pass layers and via discs of `w` for via traversals `ks` (no pricing); appends the
    /// passes whose layer or disc changed, with their layer and disc before, to `changed`.
    /// O(passes of the wire).
    fn apply<M: Mem + ?Sized>(&self, m: &mut M, w: WireId, ev: &[LEv], ks: &[usize], changed: &mut Vec<Changed>) -> Option<WireId> {
        let wi = &self.st.wires[w];
        let n = wi.passes.len();
        let d = self.wire_d[w];
        LAY_SCRATCH.with(|sc| {
            let sc = &mut *sc.borrow_mut();
            pass_layers_into(&mut sc.pl, n, ev, self.bottom, ks);
            wire_discs_into(&mut sc.disc, n, ks, d);
            for (k, &p) in wi.passes.iter().enumerate() {
                let (ol, od) = (m.layer(p), m.disc(p));
                if ol != sc.pl[k] || od != sc.disc[k] {
                    changed.push((p, ol, od));
                    m.set_pass(p, sc.pl[k], sc.disc[k]);
                }
            }
        });
        let nr = self.need_r(w);
        // 5.4 `[vias] between_crossings`: a via of an interval with no traversal between its
        // events has a site only on its own side of an end event ([`end_site`], 5.2); the others
        // have one when their traversal does.
        let between = if self.packer.via_between { via_ranges(ev, true).1 } else { Vec::new() };
        let ns = if nr > 0.0 {
            let mut c = 0;
            for (i, &k) in ks.iter().enumerate() {
                let no = if between.get(i) == Some(&false) { self.end_site_of(m, w, ev, i).is_none() } else { self.rank(&*m, w, k).0 };
                c += no as usize;
            }
            c
        } else {
            0
        };
        m.set_nosite(w, ns);
        m.set_via_k(w, ks);
        let mut moved = false;
        if self.packer.via_cuts.is_some() && d > 0.0 {
            let pts: Vec<ViaPt> = ks
                .iter()
                .enumerate()
                .map(|(i, &k)| {
                    let tl = wi.tris.len().checked_sub(1)?;
                    if between.get(i) == Some(&false) {
                        if let Some((ke, p)) = self.end_site_of(m, w, ev, i) {
                            return Some((wi.tris[ke.min(tl)], p));
                        }
                    }
                    let t = wi.tris[k.min(tl)];
                    Some((t, self.packer.via_point_cached(t, wi.net, d, wi.s)))
                })
                .collect();
            moved = m.vpts(w) != pts.as_slice();
            if moved {
                m.set_vpts(w, &pts);
            }
        }
        m.applied(w, ev);
        // CM1: `w` when its via points changed (its via prices must be repriced even when no pass
        // of it changed layer, e.g. an end site that moved or a wire without passes).
        moved.then_some(w)
    }

    /// Price again the edge-layers of the passes `changed`; returns the change of the total.
    ///
    /// Only the layers whose items changed are evaluated ([`layers_changed`]): an edge-layer
    /// whose items are the same would price the same, log nothing and add nothing to the
    /// change, so leaving it out is exact (par-refine §3). Edges ascending, layers ascending, as
    /// before. O(changed passes x layers x passes of an edge).
    fn reprice<M: Mem + ?Sized>(&self, m: &mut M, changed: &[Changed], moved: Option<WireId>) -> f64 {
        let nl = self.nl;
        let mut es = LAY_SCRATCH.with(|sc| std::mem::take(&mut sc.borrow_mut().es));
        es.clear();
        es.extend(changed.iter().map(|&(p, ol, od)| (self.st.passes[p].edge, layers_changed(nl, ol, od, m.layer(p), m.disc(p)))));
        es.sort_unstable_by_key(|x| x.0);
        let mut delta = 0.0;
        let ew = m.edge_w();
        let mut j = 0;
        while j < es.len() {
            let e = es[j].0;
            let mut mask = 0u64;
            while j < es.len() && es[j].0 == e {
                mask |= es[j].1;
                j += 1;
            }
            for l in (0..nl).filter(|&l| (mask >> l) & 1 == 1) {
                let i = e * nl + l;
                let (c, q) = self.eval(&*m, e, l);
                let (oc, oq) = m.cost(i);
                if c != oc || q != oq {
                    delta += ew * (c - oc) + (q - oq);
                    m.set_cost(i, c, q);
                }
            }
        }
        LAY_SCRATCH.with(|sc| sc.borrow_mut().es = es);
        delta += self.reprice_vias(m, changed, moved);
        delta + m.flush(self)
    }

    /// CM1: the neighbourhood of wire `x`'s path ([`NStar`]). Memo per wire. O(its triangles x
    /// their vertex stars x traversals) on the first call.
    fn nstar(&self, x: WireId) -> std::sync::Arc<NStar> {
        self.nstar[x]
            .get_or_init(|| {
                let (st, pr) = (self.st, self.st.proj);
                let mut tris: Vec<TriId> = st.wires[x].tris.iter().flat_map(|&t| pr.tris[t].iter().flat_map(|&v| pr.vert_tris[v].iter().copied())).collect();
                tris.sort_unstable();
                tris.dedup();
                let mut wires: Vec<WireId> = tris.iter().flat_map(|&t| st.tri_trav[t].iter().map(|x| x.0)).filter(|&u| u != x && st.wires[u].alive).collect();
                wires.sort_unstable();
                wires.dedup();
                std::sync::Arc::new(NStar { tris, wires })
            })
            .clone()
    }

    /// CM1: price again the via cuts the passes `changed` can change: those of the vias of their
    /// wires and of every other wire with a planned via whose triangle shares a vertex with one of
    /// their triangles (a gap cut's crossers, a via-to-via partner and a pair's crossers all lie
    /// in the via triangle's vertex star, and that relation is symmetric). Returns the change of
    /// the total (weight 1). O(owners x their vias x cuts x crossers).
    fn reprice_vias<M: Mem + ?Sized>(&self, m: &mut M, changed: &[Changed], moved: Option<WireId>) -> f64 {
        if self.packer.via_cuts.is_none() || (changed.is_empty() && moved.is_none()) {
            return 0.0;
        }
        let t0 = std::time::Instant::now();
        let st = self.st;
        let mut ws: Vec<WireId> = changed.iter().map(|&(p, _, _)| st.passes[p].wire).chain(moved).collect();
        ws.sort_unstable();
        ws.dedup();
        let mut owners: Vec<WireId> = Vec::new();
        for &x in &ws {
            owners.push(x);
            let ns = self.nstar(x);
            owners.extend(ns.wires.iter().copied().filter(|&u| m.vpts(u).iter().flatten().any(|&(t, _)| ns.tris.binary_search(&t).is_ok())));
        }
        owners.sort_unstable();
        owners.dedup();
        let mut delta = 0.0;
        for u in owners {
            let i = self.vbase + u;
            let c = self.price * self.via_over(&*m, u).graded;
            let (oc, _) = m.cost(i);
            if c != oc {
                delta += c - oc;
                m.set_cost(i, c, 0.0);
            }
        }
        meter::add(&meter::VIACUT_NS, &meter::VIACUT, t0);
        delta
    }

    /// CM1: the gap cuts of a via of `net` at `p` in triangle `t`, with their crossers. Memo.
    fn geo(&self, t: usize, net: i32, p: Vec2, norm: super::sheet::Norm) -> std::sync::Arc<ViaGeom> {
        let key = (t, net, p.x.to_bits(), p.y.to_bits());
        if let Some(g) = self.vgeo.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key) {
            return g.clone();
        }
        let s = self.packer.via_static_of(self.st.proj, t, net, p, norm);
        let g = std::sync::Arc::new(viacut::with_crossers(self.st, self.packer, &s, net));
        self.vgeo.lock().unwrap_or_else(std::sync::PoisonError::into_inner).entry(key).or_insert(g).clone()
    }

    /// CM1: the over-full amount of the gap cuts of `w`'s planned vias ([`viacut::over`]) and of
    /// the via-to-via cuts to the planned vias of other nets whose triangle shares a vertex with
    /// the via's triangle ([`viacut::pair_over`], counted at the lower `(wire, via)` of the pair),
    /// under the pass layers of `m`. Zero without `[capacity] via_cuts`.
    /// O(vias x (cuts x crossers + partners x chords)).
    pub(crate) fn via_over<M: MemRead + ?Sized>(&self, m: &M, w: WireId) -> viacut::Over {
        let mut tot = viacut::Over::default();
        let Some(norm) = self.packer.via_cuts else { return tot };
        if self.vfocus.as_ref().is_some_and(|f| !f.get(w).copied().unwrap_or(false)) {
            return tot;
        }
        let st = self.st;
        let wi = &st.wires[w];
        let d = self.wire_d[w];
        if !wi.alive || d <= 0.0 || m.vpts(w).is_empty() {
            return tot;
        }
        let via = super::demand::Item { net: wi.net, w: d, s: wi.s };
        let on = |x: WireId, k: usize, l: usize| {
            let ps = &st.wires[x].passes;
            let mut any = false;
            let mut hit = false;
            for j in [k.checked_sub(1), Some(k)].into_iter().flatten().filter(|&j| j < ps.len()) {
                any = true;
                hit |= m.layer(ps[j]) == l as i32;
            }
            hit || (!any && self.bottom == l as i32)
        };
        let item = |x: WireId, k: usize| {
            let xi = &st.wires[x];
            let j = k.min(xi.passes.len().saturating_sub(1));
            let wd = xi.passes.get(j).and_then(|&p| st.pass_w.get(p).copied()).unwrap_or(xi.w);
            super::demand::Item { net: xi.net, w: wd, s: xi.s }
        };
        let on1 = |x: WireId, k: usize| (0..self.nl).find(|&l| on(x, k, l));
        for (i, &vp) in m.vpts(w).iter().enumerate() {
            let Some((t, p)) = vp else { continue };
            let g = self.geo(t, wi.net, p, norm);
            let o = viacut::over(&g, via, &on, &item);
            tot.count += o.count;
            tot.graded += o.graded;
            // Via-to-via cuts: the partners in the via triangle's vertex star.
            // (Every partner of another net traverses the star, so it is a chord wire.)
            for &u in &g.wires[g.wires.partition_point(|&u| u <= w)..] {
                let ui = &st.wires[u];
                let du = self.wire_d[u];
                if !ui.alive || du <= 0.0 || ui.net == wi.net {
                    continue;
                }
                let b = super::demand::Item { net: ui.net, w: du, s: ui.s };
                for (j, &vq) in m.vpts(u).iter().enumerate() {
                    let Some((tu, q)) = vq else { continue };
                    if (u, j) <= (w, i) || g.tris.binary_search(&tu).is_err() {
                        continue;
                    }
                    if let Some(f) = viacut::pair_over(&g, via, q, b, norm, &on1, &item) {
                        tot.count += 1;
                        tot.graded += f;
                    }
                }
            }
        }
        tot
    }

    /// Move the vias `which` of `w` through their intervals (one sweep each, in order) to the
    /// traversal with a site and the least overflow price, ties by the drawing's preference.
    /// O(vias x interval length x layers x passes of an edge).
    fn optimise<M: Mem + ?Sized>(&self, m: &mut M, w: WireId, ev: &[LEv], which: &[usize]) {
        let _pg = super::prof::g(super::prof::K::CapmodelOptimise);
        let n = self.st.wires[w].passes.len();
        let mut ch = Vec::new();
        for &i in which {
            let mut ks = m.via_k(w).to_vec();
            let (ka, kb) = m.via_iv(w)[i];
            let lo = ka.max(if i > 0 { ks[i - 1] } else { 0 }).min(n);
            let hi = kb.min(if i + 1 < ks.len() { ks[i + 1] } else { n }).min(n).max(lo);
            if lo == hi {
                continue;
            }
            let cur = ks[i];
            let mut run = 0.0;
            let mut best: Option<(SiteKey, usize)> = None;
            for k in lo..=hi {
                if m.abandoned() {
                    return;
                }
                let t0 = std::time::Instant::now();
                ks[i] = k;
                ch.clear();
                let mv = self.apply(m, w, ev, &ks, &mut ch);
                run += self.reprice(m, &ch, mv);
                m.viapos(t0);
                let r = self.rank(&*m, w, k);
                let key = (r.0, (run * 1e3).round() as i64, r.1, r.2, r.3);
                if best.as_ref().is_none_or(|b| key < b.0) {
                    best = Some((key, k));
                }
            }
            let k = best.map_or(cur, |b| b.1);
            ks[i] = k;
            ch.clear();
            let mv = self.apply(m, w, ev, &ks, &mut ch);
            self.reprice(m, &ch, mv);
        }
    }

    /// Lay out wire `w` again after its events changed: vias of unchanged intervals stay, the
    /// others are placed by the drawing's preference and then moved for room. O(passes of the
    /// wire + moved vias x interval).
    pub(crate) fn relayout<M: Mem + ?Sized>(&self, m: &mut M, w: WireId, ev: &[LEv]) {
        let iv = via_ranges(ev, self.packer.via_between).0;
        let (oiv, oks) = (m.via_iv(w).to_vec(), m.via_k(w).to_vec());
        let ks = self.default_ks(m, w, ev, &iv, Some((&oiv, &oks)));
        let moved: Vec<usize> = (0..iv.len()).filter(|&i| oiv.get(i) != Some(&iv[i])).collect();
        m.set_via_iv(w, iv);
        let mut ch = Vec::new();
        let mv = self.apply(m, w, ev, &ks, &mut ch);
        self.reprice(m, &ch, mv);
        self.optimise(m, w, ev, &moved);
    }

    /// Overflow price and via squeeze price of edge `e` on layer `l` under the pass layers of
    /// `m`: every via disc there pays the `via_squeeze` weight times the squeeze of the slack the
    /// packing leaves (the least `rmax - lmin`) against one via diameter.
    ///
    /// The items, their end margins and the packing come from the per-pass tables and a
    /// per-thread scratch ([`EvalScratch`]): the same items in the same order and the same
    /// operations as [`CapModel::items`] + [`Packer::bounds`] + [`super::pack::Bounds::worst`]
    /// (par-refine §3, exact), without a hash lookup or an allocation per call.
    fn eval<M: MemRead + ?Sized>(&self, m: &M, e: usize, l: usize) -> (f64, f64) {
        let order = &self.st.edge_order[e];
        if order.is_empty() {
            return (0.0, 0.0);
        }
        EVAL_SCRATCH.with(|sc| {
            let sc = &mut *sc.borrow_mut();
            self.items_in(m, l, order, sc);
            if sc.items.is_empty() {
                return (0.0, 0.0);
            }
            let n = self.numbers_in(e, l, sc);
            self.cost_of(n)
        })
    }

    /// The pack items of edge-layer `l` of the passes `order` (an edge's order) into `sc.items`,
    /// their passes into `sc.ids` ([`layer_items`]'s rule: a pass on `l` packs its track and its
    /// disc, a pass on another layer with a disc packs the disc alone). O(passes of the edge).
    fn items_in<M: MemRead + ?Sized>(&self, m: &M, l: usize, order: &[usize], sc: &mut EvalScratch) {
        sc.items.clear();
        sc.ids.clear();
        for &p in order {
            let (pl, d) = (m.layer(p), m.disc(p));
            let it = if pl == l as i32 {
                PackItem { disc: d, ..self.item0[p] }
            } else if pl >= 0 && d[0].max(d[1]) > 0.0 {
                let i0 = &self.item0[p];
                PackItem { net: i0.net, w: 0.0, s: i0.s, infl: 1.0, disc: d }
            } else {
                continue;
            };
            sc.ids.push((p, pl == l as i32));
            sc.items.push(it);
        }
    }

    /// The packing numbers `(over, squeeze price, pitch)` of the items in `sc` on edge `e`,
    /// layer `l` (`over` = [`super::pack::Bounds::worst`]): a function of the items and the
    /// static packer alone (the end margins are the packer's), so equal items give equal bits.
    fn numbers_in(&self, e: usize, l: usize, sc: &mut EvalScratch) -> EdgeNumbers {
        sc.lo.clear();
        sc.hi.clear();
        let len = self.packer.edge_len(e);
        for (&(p, track), it) in sc.ids.iter().zip(&sc.items) {
            sc.lo.push(self.end_margin(p, e, 0, it, l, track));
            sc.hi.push(len - self.end_margin(p, e, 1, it, l, track));
        }
        let items = &sc.items;
        let over = self.packer.worst_into(e, items, &sc.lo, &sc.hi, &mut sc.req, &mut sc.rmax);
        let sq_w = self.packer.via_squeeze;
        let sq = if sq_w > 0.0 { sq_w * items.iter().filter(|i| i.dmax() > 0.0).map(|i| squeeze_amount(-over, i.dmax())).sum::<f64>() } else { 0.0 };
        let pitch = (items.iter().map(|i| i.w.max(i.dmax()) + i.s).sum::<f64>() / items.len() as f64).max(1.0);
        (over, sq, pitch)
    }

    /// Overflow and via squeeze price of an edge-layer from its packing numbers ([`Frame::eval`]). O(1).
    fn cost_of(&self, (over, sq, pitch): EdgeNumbers) -> (f64, f64) {
        if over <= 1e-6 {
            return (0.0, sq);
        }
        (self.price * (1.0 + over / pitch), sq)
    }

    /// The end margin of item `it` of pass `p` at end `end` of edge `e` on layer `l` as
    /// [`Packer::bounds`] takes it (its track's or, `track` = false, its disc-alone item's
    /// [`Packer::margin`], at least the via disc's [`Packer::margin_all`] when it has a disc
    /// there), through the pass's margin table. O(1) once filled.
    fn end_margin(&self, p: usize, e: usize, end: usize, it: &PackItem, l: usize, track: bool) -> f64 {
        use std::sync::atomic::Ordering::Relaxed;
        let nl = self.nl;
        let base = p * margin_slots(nl) + end * (2 * nl + 1);
        let slot = &self.margins[base + if track { l } else { nl + l }];
        let m = match slot.load(Relaxed) {
            MARGIN_UNSET => {
                let m = self.packer.margin(e, end, it.net, it.w, it.s, l);
                slot.store(m.to_bits(), Relaxed);
                m
            }
            b => f64::from_bits(b),
        };
        let d = it.disc[end];
        if d <= 0.0 {
            return m;
        }
        let w = self.st.passes[p].wire;
        if d.to_bits() != self.wire_d[w].to_bits() {
            return m.max(self.packer.margin_all(e, end, it.net, d, it.s));
        }
        let slot = &self.margins[base + 2 * nl];
        let ma = match slot.load(Relaxed) {
            MARGIN_UNSET => {
                let ma = self.packer.margin_all(e, end, it.net, d, it.s);
                slot.store(ma.to_bits(), Relaxed);
                ma
            }
            b => f64::from_bits(b),
        };
        m.max(ma)
    }

    /// The pack items of edge `e` on layer `l` under the pass layers of `m`, without the wires in
    /// `gone` (the reference of [`Frame::eval`] and [`CapModel::fits_without`]).
    fn items<M: MemRead + ?Sized>(&self, m: &M, e: usize, l: usize, gone: Option<&std::collections::BTreeSet<WireId>>) -> Vec<PackItem> {
        let pr = self.st.proj;
        let order: Vec<usize> = match gone {
            Some(g) => self.st.edge_order[e].iter().copied().filter(|&p| !g.contains(&self.st.passes[p].wire)).collect(),
            None => self.st.edge_order[e].clone(),
        };
        layer_items(
            &order,
            l,
            |p| m.layer(p),
            |p| m.disc(p),
            |p| {
                let w = self.st.passes[p].wire;
                let wi = &self.st.wires[w];
                PackItem { net: wi.net, w: self.st.pass_w.get(p).copied().unwrap_or(wi.w), s: wi.s, infl: infl_dir(pr, e, self.dirs[w][self.pass_k[p]]), disc: [0.0; 2] }
            },
        )
        .1
    }
}

/// [`pass_layers_at`] into `out` (the same values). O(events + n).
fn pass_layers_into(out: &mut Vec<i32>, n: usize, ev: &[LEv], bottom: i32, ks: &[usize]) {
    out.clear();
    let Some(&(_, _, first)) = ev.first() else {
        out.resize(n, bottom);
        return;
    };
    out.resize(n, first);
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
}

/// [`super::pack::wire_discs`] of a wire with `n` passes and vias at `ks`, all of diameter `d`, into `out`
/// (the same values). O(passes + vias).
fn wire_discs_into(out: &mut Vec<Disc>, n: usize, ks: &[usize], d: f64) {
    out.clear();
    out.resize(n, [0.0, 0.0]);
    if d <= 0.0 {
        return;
    }
    for &k in ks {
        for j in [k.checked_sub(1), Some(k)].into_iter().flatten().filter(|&j| j < n) {
            out[j] = [out[j][0].max(d), out[j][1].max(d)];
        }
    }
}

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
///
/// The model is its static [`Frame`] and its labelling-dependent [`Arrays`] (reached through
/// `Deref`: `cm.pass_layer`, `cm.via_k`, ...), so the lanes of the parallel refinement can read
/// both while the run thread alone writes the arrays (par-refine §2-3).
pub struct CapModel<'a> {
    pub(crate) fx: Frame<'a>,
    pub(crate) a: Arrays,
    /// P1a (`[capacity] model = "cuts"`, [`super::cutmodel`]): the over-full price of the
    /// per-layer cuts replaces the per-edge price `cost` (still kept, for the reports and the P4
    /// rip-up); `None` = the per-edge model.
    pub cut: Option<CutPrice<'a>>,
    /// Cut model: the events of every wire as last applied, and the wires applied since the
    /// last repricing.
    evs: Vec<Vec<LEv>>,
    pending: Vec<WireId>,
}

impl std::ops::Deref for CapModel<'_> {
    type Target = Arrays;
    fn deref(&self) -> &Arrays {
        &self.a
    }
}

impl std::ops::DerefMut for CapModel<'_> {
    fn deref_mut(&mut self) -> &mut Arrays {
        &mut self.a
    }
}

impl<'a> CapModel<'a> {
    /// The model of the labelling `evl` (events per wire). O(passes x margin lookups + vias x
    /// interval length x layers).
    pub fn new(st: &'a State<'a>, packer: &'a Packer<'a>, dirs: &'a [Vec<Vec2>], evl: &[Vec<LEv>], bottom: i32, price: f64) -> Self {
        Self::build(st, packer, dirs, evl, bottom, price, false, None, None)
    }

    /// loop-local: the per-edge model of the wires `only[w]` alone (the other wires' passes are
    /// not laid out and pack nothing). Exact on every edge whose passes all belong to those wires;
    /// `dirs` needs only their directions ([`Packer::taut_dirs_only`]). O(their passes x margin
    /// lookups + their vias x interval x layers) plus O(passes) for the arrays.
    pub fn new_only(st: &'a State<'a>, packer: &'a Packer<'a>, dirs: &'a [Vec<Vec2>], evl: &[Vec<LEv>], bottom: i32, price: f64, only: &[bool]) -> Self {
        Self::build(st, packer, dirs, evl, bottom, price, false, Some(only), None)
    }

    /// [`CapModel::new_only`] with the via-cut focus `focus` ([`Frame::vfocus`]; tw4 T2: the
    /// local judge's wires whose via terms a move can change). Without `[capacity] via_cuts` the
    /// same as `new_only`. O(as new_only).
    #[allow(clippy::too_many_arguments)]
    pub fn new_only_focus(st: &'a State<'a>, packer: &'a Packer<'a>, dirs: &'a [Vec<Vec2>], evl: &[Vec<LEv>], bottom: i32, price: f64, only: &[bool], focus: &[bool]) -> Self {
        Self::build(st, packer, dirs, evl, bottom, price, false, Some(only), packer.via_cuts.map(|_| focus))
    }

    /// As [`CapModel::new`], priced by the per-layer cuts when `[capacity] model = "cuts"`
    /// ([`Packer::cuts`]; the P3 refinement). A sheet build that fails is recorded in
    /// [`super::cutmodel::CutCtx::problems`] and the model stays per-edge.
    pub fn new_cut(st: &'a State<'a>, packer: &'a Packer<'a>, dirs: &'a [Vec<Vec2>], evl: &[Vec<LEv>], bottom: i32, price: f64) -> Self {
        Self::build(st, packer, dirs, evl, bottom, price, true, None, None)
    }

    #[allow(clippy::too_many_arguments)]
    fn build(st: &'a State<'a>, packer: &'a Packer<'a>, dirs: &'a [Vec<Vec2>], evl: &[Vec<LEv>], bottom: i32, price: f64, cuts: bool, only: Option<&[bool]>, focus: Option<&[bool]>) -> Self {
        let tb = std::time::Instant::now();
        let used = |w: WireId| only.is_none_or(|o| o.get(w).copied().unwrap_or(false));
        let _pg = super::prof::g(super::prof::K::CapmodelBuild);
        super::prof::units(super::prof::K::CapmodelBuild, st.passes.len());
        let nl = st.proj.n_layers;
        let nw = st.wires.len();
        let a = Arrays {
            pass_layer: vec![-1; st.passes.len()],
            pass_disc: vec![[0.0; 2]; st.passes.len()],
            via_iv: vec![Vec::new(); nw],
            via_k: vec![Vec::new(); nw],
            nosite: vec![0; nw],
            end_sites: crate::fxhash::map(),
            cost: vec![0.0; st.proj.edges.len() * nl + nw],
            sq: vec![0.0; st.proj.edges.len() * nl + nw],
            log: None,
            vpts: vec![Vec::new(); nw],
        };
        let mut fx = Frame::new(st, packer, dirs, bottom, price, only);
        fx.vfocus = focus.map(|f| f.to_vec());
        let mut m = CapModel { fx, a, cut: None, evs: Vec::new(), pending: Vec::new() };
        if let Some(ctx) = packer.cuts.as_ref().filter(|_| cuts) {
            match ctx.sheets(st) {
                Ok(sh) => {
                    m.cut = Some(CutPrice::new(st.proj, sh, ctx.o, price, nw));
                    m.evs = vec![Vec::new(); nw];
                }
                Err(e) => super::cutmodel::lockp(&ctx.problems).push(e),
            }
        }
        let mut ch = Vec::new();
        for (w, ev) in evl.iter().enumerate().take(nw) {
            if st.wires[w].alive && used(w) {
                let iv = via_ranges(ev, packer.via_between).0;
                let (fx, mut ms) = m.master();
                let ks = fx.default_ks(&mut ms, w, ev, &iv, None);
                ms.set_via_iv(w, iv);
                ch.clear();
                let _ = fx.apply(&mut ms, w, ev, &ks, &mut ch);
            }
        }
        m.price_all();
        if packer.via_cuts.is_some() {
            let t0 = std::time::Instant::now();
            for w in (0..nw).filter(|&w| st.wires[w].alive && used(w)) {
                let c = m.fx.price * m.fx.via_over(&m.a, w).graded;
                m.a.cost[m.fx.vbase + w] = c;
            }
            meter::add(&meter::VIACUT_NS, &meter::VIACUT, t0);
        }
        {
            let (fx, mut ms) = m.master();
            ms.flush(fx);
        }
        for (w, ev) in evl.iter().enumerate().take(nw) {
            let swept = m.fx.vfocus.as_ref().is_none_or(|f| f.get(w).copied().unwrap_or(false));
            if st.wires[w].alive && used(w) && swept && !m.via_iv[w].is_empty() {
                let all: Vec<usize> = (0..m.via_iv[w].len()).collect();
                let (fx, mut ms) = m.master();
                fx.optimise(&mut ms, w, ev, &all);
            }
        }
        meter::add(&meter::BUILD_NS, &meter::BUILDS, tb);
        m
    }

    /// The static frame and the model's own arrays as a writable [`Mem`]. O(1).
    pub(crate) fn master(&mut self) -> (&Frame<'a>, Master<'_, 'a>) {
        (&self.fx, Master { a: &mut self.a, cut: &mut self.cut, evs: &mut self.evs, pending: &mut self.pending })
    }

    /// The static frame and the arrays, read-only (what the lanes share). O(1).
    pub(crate) fn parts(&self) -> (&Frame<'a>, &Arrays) {
        (&self.fx, &self.a)
    }

    /// Number of layers of the model.
    pub fn n_layers(&self) -> usize {
        self.fx.nl
    }

    /// [`Frame::rank`] (debug output).
    pub fn rank_of(&self, w: WireId, k: usize) -> (bool, usize, i64, usize) {
        self.fx.rank(&self.a, w, k)
    }

    /// Weight of the per-edge overflow price in the total: 0 when the cut model prices.
    fn edge_w(&self) -> f64 {
        if self.cut.is_some() {
            0.0
        } else {
            1.0
        }
    }

    /// The build's price of every edge-layer (par-refine item 2, exact): an edge-layer whose
    /// items are bit for bit those it had in an earlier build with the same packer (the packer's
    /// [`EdgeMemo`], one entry per edge-layer: the last items priced there) takes the packing
    /// numbers of that build instead of packing again; the numbers are a function of the items
    /// and the static packer only ([`Frame::numbers_in`]), the price is applied after. Edges
    /// without passes stay at 0. O(passes x layers) + the packings of the changed edge-layers.
    fn price_all(&mut self) {
        let fx = &self.fx;
        let (st, nl) = (fx.st, fx.nl);
        let ne = st.proj.edges.len();
        let mut memo = fx.packer.edge_memo.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let usable = fx.packer.same_proj(st.proj);
        if usable && (memo.nl != nl || memo.ent.len() != ne * nl) {
            memo.nl = nl;
            memo.ent.clear();
            memo.ent.resize_with(ne * nl, Default::default);
        }
        let (mut hits, mut misses) = (0u64, 0u64);
        let a = &mut self.a;
        EVAL_SCRATCH.with(|sc| {
            let sc = &mut *sc.borrow_mut();
            for e in 0..ne {
                let order = &st.edge_order[e];
                if order.is_empty() {
                    continue;
                }
                for l in 0..nl {
                    fx.items_in(&*a, l, order, sc);
                    if sc.items.is_empty() {
                        continue;
                    }
                    let n = if usable {
                        let ent = &mut memo.ent[e * nl + l];
                        match ent.nums.filter(|_| same_items(&ent.items, &sc.items)) {
                            Some(n) => {
                                hits += 1;
                                n
                            }
                            None => {
                                misses += 1;
                                let n = fx.numbers_in(e, l, sc);
                                ent.items.clear();
                                ent.items.extend_from_slice(&sc.items);
                                ent.nums = Some(n);
                                n
                            }
                        }
                    } else {
                        fx.numbers_in(e, l, sc)
                    };
                    (a.cost[e * nl + l], a.sq[e * nl + l]) = fx.cost_of(n);
                }
            }
        });
        drop(memo);
        if meter::on() {
            meter::MEMO_HIT.fetch_add(hits, std::sync::atomic::Ordering::Relaxed);
            meter::MEMO_MISS.fetch_add(misses, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// [`Frame::eval`] the way it was computed before the per-pass tables (par-refine §3):
    /// [`Frame::items`] + [`Packer::bounds`] + [`super::pack::Bounds::worst`]. The tests'
    /// reference for the exactness of the tables. O(passes of the edge x margin lookups).
    #[doc(hidden)]
    pub fn eval_reference(&self, e: usize, l: usize) -> (f64, f64) {
        let fx = &self.fx;
        let items = fx.items(&self.a, e, l, None);
        if items.is_empty() {
            return (0.0, 0.0);
        }
        let over = fx.packer.bounds(e, l, &items).worst();
        let sq_w = fx.packer.via_squeeze;
        let sq = if sq_w > 0.0 { sq_w * items.iter().filter(|i| i.dmax() > 0.0).map(|i| squeeze_amount(-over, i.dmax())).sum::<f64>() } else { 0.0 };
        if over <= 1e-6 {
            return (0.0, sq);
        }
        let pitch = (items.iter().map(|i| i.w.max(i.dmax()) + i.s).sum::<f64>() / items.len() as f64).max(1.0);
        (fx.price * (1.0 + over / pitch), sq)
    }

    /// [`Frame::eval`] of edge `e`, layer `l` under the current pass layers (tests). O(passes of the edge).
    #[doc(hidden)]
    pub fn eval_at(&self, e: usize, l: usize) -> (f64, f64) {
        self.fx.eval(&self.a, e, l)
    }

    /// The stored price `(cost, squeeze)` of edge `e`, layer `l` (tests). O(1).
    #[doc(hidden)]
    pub fn price_at(&self, e: usize, l: usize) -> (f64, f64) {
        self.a.cost(e * self.fx.nl + l)
    }

    /// Whether the passes on edge `e`, layer `l` fit when the wires in `gone` are taken out.
    pub fn fits_without(&self, e: usize, l: usize, gone: &std::collections::BTreeSet<WireId>) -> bool {
        let items = self.fx.items(&self.a, e, l, Some(gone));
        items.is_empty() || self.fx.packer.bounds(e, l, &items).fits()
    }

    /// Total overflow price of the labelling (mm), vias without a site and via squeeze
    /// included.
    pub fn total(&self) -> f64 {
        let cut = self.cut.as_ref().map_or(0.0, |c| c.total());
        let (edges, vias) = self.cost.split_at(self.fx.vbase);
        self.edge_w() * edges.iter().sum::<f64>() + vias.iter().sum::<f64>() + self.sq.iter().sum::<f64>() + self.fx.price * self.nosite.iter().sum::<usize>() as f64 + cut
    }

    /// Via squeeze price of the labelling (mm).
    pub fn squeeze(&self) -> f64 {
        self.sq.iter().sum()
    }

    /// Over-full edge-layers (cut model: over-full cuts) plus vias without a site.
    pub fn overfull(&self) -> usize {
        let over = match self.cut.as_ref() {
            Some(c) => c.overfull(),
            None => self.cost[..self.fx.vbase].iter().filter(|&&c| c > 0.0).count(),
        };
        over + self.nosite.iter().sum::<usize>() + self.via_cut_count()
    }

    /// CM1: the over-full via cuts (per via the over-full layers of its gap cuts, plus its
    /// over-full via-to-via cuts). O(wires with a priced via cut x their vias x cuts).
    pub fn via_cut_count(&self) -> usize {
        (0..self.vpts.len()).filter(|&w| self.cost[self.fx.vbase + w] > 0.0).map(|w| self.fx.via_over(&self.a, w).count).sum()
    }

    /// CM1: the wires with an over-full via cut, each with the wires that cross its gap cuts or
    /// lie in its vias' neighbourhood (the P4 victims and history), ascending.
    pub fn via_cut_list(&self) -> Vec<(WireId, Vec<WireId>)> {
        let Some(norm) = self.fx.packer.via_cuts else { return Vec::new() };
        (0..self.vpts.len())
            .filter(|&w| self.cost[self.fx.vbase + w] > 0.0)
            .map(|w| {
                let net = self.fx.st.wires[w].net;
                let mut xs: Vec<WireId> = self.vpts[w].iter().flatten().flat_map(|&(t, p)| self.fx.geo(t, net, p, norm).cuts.iter().flat_map(|c| c.cross.iter().map(|x| x.0)).collect::<Vec<_>>()).collect();
                xs.sort_unstable();
                xs.dedup();
                (w, xs)
            })
            .collect()
    }

    /// CM1 check (tw4 T2, `TOPO_VIACHECK`, tests): the via entries whose stored price differs from
    /// a fresh pricing under the current layers, `(mismatches, priced entries)`. O(wires x vias).
    #[doc(hidden)]
    pub fn via_check(&self) -> (usize, usize) {
        let mut bad = 0;
        let mut n = 0;
        for w in 0..self.vpts.len() {
            let fresh = self.fx.price * self.fx.via_over(&self.a, w).graded;
            let c = self.cost[self.fx.vbase + w];
            n += usize::from(c > 0.0 || fresh > 0.0);
            if (fresh - c).abs() > 1e-6 * fresh.abs().max(1.0) {
                bad += 1;
                if bad <= 5 {
                    eprintln!("VIACHECK wire {w} net {} stored {c:.6} fresh {fresh:.6}", self.fx.st.wires[w].net);
                }
            }
        }
        (bad, n)
    }

    /// CM1: the planned vias with an over-full via cut, located: `(wire, via point)` (the compass).
    pub fn via_cut_points(&self) -> Vec<(WireId, Vec2)> {
        (0..self.vpts.len()).filter(|&w| self.cost[self.fx.vbase + w] > 0.0).flat_map(|w| self.vpts[w].iter().flatten().map(move |&(_, p)| (w, p))).collect()
    }

    /// The predicted violations of this labelling as the TopoR loop's archive and every loop move
    /// judge them (shopping list 5.2): `(count, graded)`. The count is [`CapModel::overfull`]
    /// (over-full edge-layers, or over-full cuts with `[capacity] model = "cuts"`, plus vias
    /// without a site); the graded amount gives each of them 1 plus how far it is over, as the
    /// `violation` factor does (1 per violation plus its depth): an edge-layer its price factor
    /// `1 + over / pitch` (the per-edge price, `Frame::eval`), a cut `1 - slack / pitch`
    /// ([`CutPrice::over_amount`]), a via without a site 1. So `graded >= count`, and one more
    /// over-full item always adds at least 1. O(edges x layers + cuts).
    pub fn violations(&self) -> (usize, f64) {
        let nosite = self.nosite.iter().sum::<usize>();
        let over = match self.cut.as_ref() {
            Some(c) => c.over_amount(),
            None => self.cost[..self.fx.vbase].iter().filter(|&&c| c > 0.0).map(|&c| c / self.fx.price).sum(),
        };
        let vias: f64 = self.cost[self.fx.vbase..].iter().map(|&c| c / self.fx.price).sum();
        (self.overfull(), over + nosite as f64 + vias)
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
        let nl = self.fx.nl;
        (0..self.fx.vbase).filter(|&i| self.cost[i] > 0.0).map(|i| (i / nl, i % nl)).collect()
    }

    /// TW5 (c): the over-full edges with their graded depth (the per-edge price factor minus 1:
    /// how far over, in pitches), one entry per over-full edge-layer, ascending.
    pub fn overfull_depths(&self) -> Vec<(usize, f64)> {
        let nl = self.fx.nl;
        let pr = self.fx.price.max(1e-12);
        (0..self.cost.len()).filter(|&i| self.cost[i] > 0.0).map(|i| (i / nl, (self.cost[i] / pr - 1.0).max(0.0))).collect()
    }

    /// Wires with a via that has no site in its interval, ascending.
    pub fn nosite_wires(&self) -> Vec<WireId> {
        (0..self.nosite.len()).filter(|&w| self.nosite[w] > 0).collect()
    }

    /// Debug (`TOPO_LOOPDBG`): the vias of wire `w` under events `ev`, each with its interval, its
    /// traversal and per traversal of the interval the triangle, whether it has a static site,
    /// whether it is taken, its inradius and the free radius against the static board (mm).
    pub fn nosite_debug(&self, w: WireId, ev: &[LEv]) -> String {
        let fx = &self.fx;
        let wi = &fx.st.wires[w];
        let d = fx.packer.via_d(wi.net);
        let (iv, ok) = via_ranges(ev, fx.packer.via_between);
        let mut out = format!("wire {w} net {} via_d {:.3} need_r {:.3} nosite {} ev {:?}:", wi.net, d / 1e6, fx.need_r(w) / 1e6, self.nosite[w], ev);
        for (i, &(ka, kb)) in iv.iter().enumerate() {
            // the cached end site, or computed without filling the cache (debug output changes nothing)
            let es = fx.end_key(w, ev, i).and_then(|(key, a, b)| self.a.end_site(&key).unwrap_or_else(|| fx.end_site_raw(&self.a, w, a, b)));
            out += &format!(" [via {i} iv ({ka},{kb}) between_ok {} end_site {:?} k {:?}:", ok[i], es.map(|(k, c)| (k, (c.x / 1e6 * 1e3).round() / 1e3, (-c.y / 1e6 * 1e3).round() / 1e3)), self.via_k[w].get(i));
            for k in ka..=kb.min(wi.tris.len().saturating_sub(1)) {
                let t = wi.tris[k];
                let site = d <= 0.0 || fx.packer.via_site(t, wi.net, 0.5 * d, wi.s);
                out += &format!(" k{k} t{t} site {site} taken {} inr {:.3} out {} obs {};", fx.site_taken(&self.a, w, t), fx.st.proj.inradius(t) / 1e6, fx.st.proj.tri_out[t], fx.st.proj.tri_obs[t].len());
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
    trans: &(dyn Fn(usize, usize) -> f64 + Sync),
    unary: &(dyn Fn(WireId, usize, usize) -> f64 + Sync),
    p: &RefineParams,
) -> RefineStats {
    refine_only(st, packer, dirs, evl, mask, partner, bottom, trans, unary, p, None)
}

/// [`refine`] over the per-edge model of the wires `only[w]` alone ([`CapModel::new_only`];
/// loop-local: the events the caller lets move belong to them, the others are held by `mask`).
/// No final measure is handed on (the model is not the board's). `only = None` = [`refine`].
#[allow(clippy::too_many_arguments)]
pub fn refine_only(
    st: &State,
    packer: &Packer,
    dirs: &[Vec<Vec2>],
    evl: &mut [Vec<LEv>],
    mask: &[Vec<u64>],
    partner: &[Vec<Option<(WireId, usize)>>],
    bottom: i32,
    trans: &(dyn Fn(usize, usize) -> f64 + Sync),
    unary: &(dyn Fn(WireId, usize, usize) -> f64 + Sync),
    p: &RefineParams,
    only: Option<&[bool]>,
) -> RefineStats {
    let _pg = super::prof::g(super::prof::K::Refine);
    // Switched off: the model is built with a unit price only to count the over-full edge-layers.
    let price = if p.overflow_mm > 0.0 { p.overflow_mm } else { 1.0 };
    let mut cm = match only {
        Some(o) => CapModel::new_only(st, packer, dirs, evl, bottom, price, o),
        None => CapModel::new_cut(st, packer, dirs, evl, bottom, price),
    };
    let mut stats = RefineStats { before: cm.overfull(), ..RefineStats::default() };
    if p.overflow_mm <= 0.0 {
        stats.after = stats.before;
        return stats;
    }
    // The cut model may add vias when its setting says so (`[capacity] cut_vias`).
    let add_vias = p.add_vias || (cm.cut.is_some() && packer.cuts.as_ref().is_some_and(|c| c.add_vias));
    let ctx = Around { st, mask, partner, bottom, trans, unary, add_vias, max_evals: p.max_evals };
    if cm.cut.is_some() || p.reference || only.is_some() {
        refine_reference(&ctx, &mut cm, evl, &mut stats);
    } else if shadow::ON.load(std::sync::atomic::Ordering::Relaxed) {
        let mut evl_ref = evl.to_vec();
        let mut cm_ref = CapModel::new_cut(st, packer, dirs, evl, bottom, if p.overflow_mm > 0.0 { p.overflow_mm } else { 1.0 });
        let mut st_ref = RefineStats { before: cm_ref.overfull(), ..RefineStats::default() };
        refine_reference(&ctx, &mut cm_ref, &mut evl_ref, &mut st_ref);
        super::refine_par::run(&ctx, &mut cm, evl, &mut stats);
        shadow_fault(shadow::FAULT.load(std::sync::atomic::Ordering::Relaxed), &mut cm_ref.a, &mut evl_ref, &mut st_ref);
        if !crate::control::stop_requested() {
            if let Some(d) = shadow_diff((&cm, evl, &stats), (&cm_ref, &evl_ref, &st_ref)) {
                panic!("par-refine shadow: the lanes differ from the serial refinement in {d}");
            }
            use std::sync::atomic::Ordering::Relaxed;
            shadow::CHECKED.fetch_add(1, Relaxed);
            if stats.moves > 0 {
                shadow::CHECKED_MOVED.fetch_add(1, Relaxed);
            }
        }
    } else {
        super::refine_par::run(&ctx, &mut cm, evl, &mut stats);
    }
    stats.after = cm.overfull();
    let vc_env = std::env::var_os("TOPO_VIACHECK").is_some();
    if cm.fx.packer.via_cuts.is_some() && (vc_env || viacheck::ON.load(std::sync::atomic::Ordering::Relaxed)) {
        let (bad, n) = cm.via_check();
        viacheck::BAD.fetch_add(bad, std::sync::atomic::Ordering::Relaxed);
        viacheck::CHECKED.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
        if vc_env {
            eprintln!("VIACHECK refine: {bad} mismatches of {n} priced via entries");
        }
    }
    stats.memo = memo_hash(&cm.a.end_sites);
    if cm.cut.is_none() && only.is_none() {
        stats.fin = Some(FinalCap { packer: packer as *const Packer as usize, bottom, price: cm.fx.price.to_bits(), key: cap_key(st, evl), viol: cm.violations() });
    }
    stats
}

/// The serial refinement loop (in-place tries with undo, as before par-refine): the cut model's
/// loop and the reference of the lane executor ([`super::refine_par`]). The per-edge stream is
/// the one [`super::refine_par::Cursor`] unrolls. O(`max_evals` x passes of the touched edges).
fn refine_reference(ctx: &Around, cm: &mut CapModel, evl: &mut [Vec<LEv>], stats: &mut RefineStats) {
    let (st, max_evals) = (ctx.st, ctx.max_evals);
    let mut evals = 0usize;
    let nl = cm.fx.nl;
    for _sweep in 0..8 {
        let mut improved = false;
        if cm.cut.is_some() {
            // Cut model: the wires of every over-full cut, most over-full first.
            let list = cm.cut.as_ref().map(|c| c.overfull_list()).unwrap_or_default();
            for (l, ci, _) in list {
                let mut seen = std::collections::BTreeSet::new();
                let its: Vec<(WireId, usize)> = cm.cut.as_ref().map(|c| c.items_at(l, ci)).unwrap_or_default().into_iter().filter(|x| seen.insert(*x)).collect();
                for (w, k) in its {
                    if evals >= max_evals || !cm.cut.as_ref().is_some_and(|c| c.is_over(l, ci)) {
                        break;
                    }
                    if !st.wires[w].alive {
                        continue;
                    }
                    improved |= ctx.moves_around(cm, evl, w, k, &mut evals, stats);
                }
            }
        } else {
            for (e, l) in cm.overfull_list() {
                if cm.cost[e * nl + l] <= 0.0 {
                    continue;
                }
                let passes: Vec<usize> = st.edge_order[e].iter().copied().filter(|&q| cm.pass_layer[q] == l as i32).collect();
                for pid in passes {
                    if evals >= max_evals || cm.cost[e * nl + l] <= 0.0 {
                        break;
                    }
                    let w = st.passes[pid].wire;
                    let k = st.wires[w].passes.iter().position(|&x| x == pid).expect("pass of its wire");
                    improved |= ctx.moves_around(cm, evl, w, k, &mut evals, stats);
                }
            }
        }
        if !improved || evals >= max_evals {
            break;
        }
    }
}

/// The fixed inputs of the P3 moves around one pass (shared read-only by the lanes).
pub(crate) struct Around<'r> {
    pub(crate) st: &'r State<'r>,
    pub(crate) mask: &'r [Vec<u64>],
    pub(crate) partner: &'r [Vec<Option<(WireId, usize)>>],
    pub(crate) bottom: i32,
    pub(crate) trans: &'r (dyn Fn(usize, usize) -> f64 + Sync),
    pub(crate) unary: &'r (dyn Fn(WireId, usize, usize) -> f64 + Sync),
    pub(crate) add_vias: bool,
    pub(crate) max_evals: usize,
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
            for nl in 0..cm.fx.nl as i32 {
                if nl == cur || (mask[w][i] >> nl) & 1 == 0 {
                    continue;
                }
                if partner[w][i].is_some_and(|(w2, i2)| evl[w2][i2].2 == nl) {
                    continue;
                }
                moves.push(vec![((w, i), nl)]);
            }
            for mv in moves {
                // M0: a Cancel / the time limit ends the refinement at the next move.
                if *evals >= self.max_evals || crate::control::stop_requested() {
                    break;
                }
                *evals += 1;
                let (keep, delta) = try_move_delta(self.st, cm, evl, &mv, self.bottom, self.trans, self.unary, self.add_vias, false);
                stats.evals += 1;
                stats.stream = stream_fold(stats.stream, &mv, delta, keep);
                if keep {
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
    try_move_delta(st, cm, evl, mv, _bottom, trans, unary, add_vias, strict).0
}

/// [`try_move`], also returning the candidate's price change (the keep test's left side).
#[allow(clippy::too_many_arguments)]
pub(crate) fn try_move_delta(
    st: &State,
    cm: &mut CapModel,
    evl: &mut [Vec<LEv>],
    mv: &[((WireId, usize), i32)],
    _bottom: i32,
    trans: &dyn Fn(usize, usize) -> f64,
    unary: &dyn Fn(WireId, usize, usize) -> f64,
    add_vias: bool,
    strict: bool,
) -> (bool, f64) {
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
) -> (bool, f64) {
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
        // CM1: the via points (Frame::apply sets them; a rejected move restores them with the
        // via prices, or the stored prices no longer match the points: tw4 T2 VIACHECK).
        pts: Vec<ViaPt>,
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
            pts: cm.vpts[w].clone(),
        })
        .collect();
    let old_ns: usize = wires.iter().map(|&w| cm.nosite[w]).sum();
    let old_evs: Vec<Vec<LEv>> = if cm.cut.is_some() { wires.iter().map(|&w| cm.evs[w].clone()).collect() } else { Vec::new() };
    let cut0 = cm.cut.as_mut().map(|c| {
        c.begin();
        c.total()
    });
    cm.a.log = Some(Vec::new());
    {
        let (fx, mut ms) = cm.master();
        for &w in &wires {
            fx.relayout(&mut ms, w, &evl[w]);
        }
    }
    let log = cm.a.log.take().unwrap_or_default();
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
    let d_oc: f64 = before.iter().map(|(&i, &(c, q))| ew * (cm.cost[i] - c) + (cm.sq[i] - q)).sum::<f64>() + cm.fx.price * (new_ns as f64 - old_ns as f64) + d_cut;
    let worse_edge = match cm.cut.as_ref() {
        Some(c) => c.worse_somewhere(),
        None => before.iter().any(|(&i, &(c, _))| cm.cost[i] > c + EPS),
    };
    let worse_somewhere = strict && (new_ns > old_ns || worse_edge);
    let delta = (new_wc - old_wc) + d_oc;
    if keep_rule(new_wc, old_wc, d_oc, add_vias) && !worse_somewhere {
        if let Some(c) = cm.cut.as_mut() {
            c.commit();
        }
        return (true, delta);
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
        cm.vpts[s.w] = s.pts;
    }
    for (&((w, i), _), &l) in mv.iter().zip(&old_lab) {
        evl[w][i].2 = l;
    }
    (false, delta)
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
    fn keep_rule_needs_a_drop_beyond_eps() {
        // a drop smaller than EPS (float noise of a price-neutral move) is not kept
        assert!(!keep_rule(1.0, 1.0, -EPS / 2.0, false));
        assert!(!keep_rule(1.0, 1.0, -EPS, false));
        assert!(keep_rule(1.0, 1.0, -2.0 * EPS, false));
        // without add_vias the wires' own price may not grow, whatever the capacity gain
        assert!(!keep_rule(3.0, 1.0, -5.0, false));
        assert!(keep_rule(3.0, 1.0, -5.0, true));
        assert!(keep_rule(1.0 + EPS / 2.0, 1.0, -1.0, false));
        // ... and the growth it allows is EPS, pinned from both sides (`<=`, not `<`; not more)
        assert!(keep_rule(EPS, 0.0, -1.0, false));
        assert!(!keep_rule(1.001 * EPS, 0.0, -1.0, false));
        assert!(!keep_rule(1.0 + 2.0 * EPS, 1.0, -1.0, false));
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
