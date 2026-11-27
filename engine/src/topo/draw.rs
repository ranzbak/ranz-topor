//! Minimal drawing (plan P6, item A): the abstract topology of [`State`] plus the layer labels
//! of P3 become copper. Per layer, the passes of every CDT edge are packed along the edge in
//! their `edge_order` (the order is the topology), each pass gets an *interval* of the edge it
//! may use (separated from its neighbours by the demand model's spacing and from the obstacle
//! at each end by its clearance), and a wire is the shortest path (funnel / string pulling)
//! through the intervals of its sleeve. Every drawn wire is checked by the [`Checker`] against
//! the board and all copper accepted before it, so each wire is an obstacle for the next; a wire
//! the checker rejects is reported unrouted with the reason. Never drop (M-B1, [`Drawer::force`]):
//! after the legal wires, such a wire is drawn anyway on the candidate with the least `violation`
//! price (its attempts, the exact path with the clearance relaxed by the least amount that gives
//! one, the funnels before the repair), and a via without a legal site goes to its least violating
//! site; the violations go with the wire ([`WireOut::viol`]).
//!
//! Vias are placed first, per wire, in a triangle of the via interval that no other wire
//! traverses where possible (a through via blocks every layer), at the incentre (largest free
//! disc) or the centroid, accepted by the checker.
//!
//! Three attempts per layer run: the funnel through the full intervals, through the intervals
//! halved towards their nominal position (more room against the oblique-chord effect), and the
//! polyline through the nominal points. Chords only (no arcs); the optional `taut` of the plan
//! is not used here.
//!
//! Complexity: intervals O(passes x bisection); a run is O(portals) for the funnel plus one
//! checker query per segment (grid candidates). Deterministic (Vec + index ids).

use super::board::Board;
use super::erbs::poly_len;
use super::pack::{infl_dir, layer_items, wire_discs, Disc, PackItem, Packer};
use super::state::{EdgeId, PassId, State, TriId, VertId, VertKind, WireId};
use crate::check::{CheckReport, Checker, ItemIndex, Violation};
use crate::geom::prim::Aabb;
use crate::geom::quant::{IVec2, TrackPrim};
use crate::geom::{Grid, Vec2};
use crate::model::{BoardModel, Item, Options, TrackItem, ViaItem};
use crate::vias::{via_sizes, ViaSize};
use std::collections::BTreeMap;

/// Layer runs and via intervals of one wire, derived from the P3 events.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// Signal-layer index of each run, in wire order.
    pub layers: Vec<usize>,
    /// Between run `i` and `i + 1`: the traversals `(ka, kb)` bounding the via interval.
    pub vias: Vec<(usize, usize)>,
    /// The traversal P3/P4 planned for each via ([`super::refine::CapModel::via_k`]); the
    /// drawing tries that triangle first. Empty: no plan (the drawing's own order).
    pub at: Vec<usize>,
}

/// The layer runs of a wire from its events `(traversal, fraction, layer)`; a wire without
/// events lies on the bottom layer. O(events).
pub fn plan(ev: &[(usize, f64, i32)], n_layers: usize) -> Plan {
    let Some(first) = ev.first() else {
        return Plan { layers: vec![n_layers - 1], vias: Vec::new(), at: Vec::new() };
    };
    let mut layers = vec![first.2 as usize];
    let mut vias = Vec::new();
    for w in ev.windows(2) {
        if w[1].2 != w[0].2 {
            vias.push((w[0].0, w[1].0));
            layers.push(w[1].2 as usize);
        }
    }
    let at = vias.iter().map(|&(a, b)| super::refine::mid_k(a, b)).collect();
    Plan { layers, vias, at }
}

/// Funnel (string pulling) from `s` to `e` through `portals` `(left, right)`, seen along the
/// travel direction: [`super::funnel::funnel`] (exact predicates; a run that starts in front of
/// its first portal or ends behind its last comes back to touch it), corners only. The same
/// corners as the taut paths of [`super::pack`]: ends an ulp off a shared vertex are snapped in
/// the funnel itself (relative tolerance, so no absolute distance here, which dropped genuine
/// corners of nm-sized features); only repeated points go (a zero-length run is one point).
/// O(portals).
pub fn funnel(s: Vec2, e: Vec2, portals: &[(Vec2, Vec2)]) -> Vec<Vec2> {
    let mut ports = Vec::with_capacity(portals.len() + 2);
    ports.push((s, s));
    ports.extend_from_slice(portals);
    ports.push((e, e));
    let mut pts: Vec<Vec2> = super::funnel::funnel(&ports).into_iter().map(|c| c.0).collect();
    pts.dedup();
    pts
}

/// Crossing of the polyline `pl` with the segment `p-q`: the distance from `p` along it; of
/// several, the one nearest to `near`. O(segments).
pub(super) fn crossing(pl: &[Vec2], p: Vec2, q: Vec2, near: f64) -> Option<f64> {
    let d2 = q - p;
    let len = d2.len().max(1.0);
    let mut best: Option<f64> = None;
    for s in pl.windows(2) {
        let (a, c) = (s[0], s[1]);
        let d1 = c - a;
        let den = d1.cross(d2);
        if den.abs() < 1e-9 {
            continue;
        }
        let t = (p - a).cross(d2) / den;
        let u = (p - a).cross(d1) / den;
        if (-1e-9..=1.0 + 1e-9).contains(&t) && (-1e-9..=1.0 + 1e-9).contains(&u) {
            let x = u * len;
            if best.is_none_or(|b| (x - near).abs() < (b - near).abs()) {
                best = Some(x);
            }
        }
    }
    best
}

/// The segments `a-b` and `c-d` cross properly (not at an end point, not collinear).
pub(super) fn seg_proper(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let (d1, d2) = (b - a, d - c);
    let den = d1.cross(d2);
    if den.abs() < 1e-9 {
        return false;
    }
    let t = (c - a).cross(d2) / den;
    let u = (c - a).cross(d1) / den;
    t > 1e-9 && t < 1.0 - 1e-9 && u > 1e-9 && u < 1.0 - 1e-9
}

/// Exact sign of the orientation of `c` against the line `a -> b` (+1 left, -1 right, 0 on it):
/// Shewchuk's adaptive `orient2d` (robust crate), exact for every f64 input. O(1).
fn orient_sign(a: Vec2, b: Vec2, c: Vec2) -> i8 {
    let o = robust::orient2d(robust::Coord { x: a.x, y: a.y }, robust::Coord { x: b.x, y: b.y }, robust::Coord { x: c.x, y: c.y });
    if o > 0.0 {
        1
    } else if o < 0.0 {
        -1
    } else {
        0
    }
}

fn sgn(x: f64) -> i8 {
    if x > 0.0 {
        1
    } else if x < 0.0 {
        -1
    } else {
        0
    }
}

/// Round j: the segment `p0 p1`, translated by the infinitesimal vector `(e, e^2)`, crosses the
/// segment `q0 q1`. A symbolic perturbation (simulation of simplicity, Edelsbrunner & Muecke 1990,
/// ACM TOG 9(1) pp. 66-104, not in docs/literature): the orientation signs are exact
/// ([`orient_sign`], Shewchuk 1997 pp. 36-40, docs/literature/shewchuk1997-robust-predicates.pdf);
/// a zero one is decided by the translation's leading term, so afterwards no end point lies on the
/// other segment and no two segments are collinear. A whole polyline translated this way gives
/// every contact one answer: a crossing through a vertex of either polyline counts once, a
/// touch-and-return or a collinear stretch an even number of times. So the count of a closed loop
/// (two runs with the same ends) against a path, mod 2, is exact: the even-odd rule of the loop at
/// the path's two ends (Erickson, docs/literature/erickson-shortest-homotopic.html, "Crossing
/// Sequences", paths in general position). [`seg_proper`]'s tolerance window is not (verifier
/// round i: a crossing through a vertex counted 0). Zero-length segments never cross. O(1).
pub(super) fn seg_cross_sos(p0: Vec2, p1: Vec2, q0: Vec2, q1: Vec2) -> bool {
    // A zero-length segment gets one side for both ends (0 both before and after the translation
    // of a point segment's own line), so it never crosses: no special case.
    let (dp, dq) = (p1 - p0, q1 - q0);
    // orient(q0, q1, p + t) = orient(q0, q1, p) + dq.x e^2 - dq.y e
    let side_q = |p: Vec2| match orient_sign(q0, q1, p) {
        0 if dq.y != 0.0 => -sgn(dq.y),
        0 => sgn(dq.x),
        s => s,
    };
    // orient(p0 + t, p1 + t, q) = orient(p0, p1, q) + dp.y e - dp.x e^2
    let side_p = |q: Vec2| match orient_sign(p0, p1, q) {
        0 if dp.y != 0.0 => sgn(dp.y),
        0 => -sgn(dp.x),
        s => s,
    };
    side_q(p0) != side_q(p1) && side_p(q0) != side_p(q1)
}

/// Crossings of the polyline `poly` with the segment `a b` ([`seg_cross_sos`]: `poly` translated
/// by an infinitesimal vector, exact signs, no distance threshold). Mod 2 it is the side invariant
/// of an undrawn wire's path between two runs with the same end points: the round-i class test
/// ([`competitor_keeps_class`]; the reference paths are whole polylines whose ends are the other
/// wire's terminals, which no run touches), so the comparison is exact. Not (yet) the seq
/// drawing's parity readers, which keep [`seg_proper`] (round j, parked, design note needed):
/// the via side test ([`Drawer::via_side_ok`]) asks on which side of a chord a free point (a via
/// site) lies, and the taut pull's guards ([`Drawer::pull_taut`]) are single segments whose ends
/// (pass points, via centres) a run may touch; there the translation decides a contact at a
/// segment's end arbitrarily (esp and qmtech via sites, bornhack +3V3 at main 385e043). O(|poly|).
pub(super) fn proper_crossings(poly: &[Vec2], a: Vec2, b: Vec2) -> usize {
    poly.windows(2).filter(|x| seg_cross_sos(x[0], x[1], a, b)).count()
}

pub(super) fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.len2();
    let t = if l2 > 0.0 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + d * t)).len()
}

/// Distance from `p` to the convex polygon `poly` (0 inside).
pub(super) fn poly_dist(p: Vec2, poly: &[Vec2]) -> f64 {
    let n = poly.len();
    let mut inside = true;
    let mut d = f64::INFINITY;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (b - a).cross(p - a) < 0.0 {
            inside = false;
        }
        d = d.min(seg_dist(p, a, b));
    }
    if inside {
        0.0
    } else {
        d
    }
}


/// Closest points of the segments `a1-b1` and `a2-b2`: `(distance, point on 1, point on 2)`.
pub(super) fn seg_seg(a1: Vec2, b1: Vec2, a2: Vec2, b2: Vec2) -> (f64, Vec2, Vec2) {
    let (d1, d2) = (b1 - a1, b2 - a2);
    let den = d1.cross(d2);
    if den.abs() > 1e-9 {
        let t = (a2 - a1).cross(d2) / den;
        let u = (a2 - a1).cross(d1) / den;
        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
            let p = a1 + d1 * t;
            return (0.0, p, p);
        }
    }
    let near = |p: Vec2, a: Vec2, b: Vec2| {
        let d = b - a;
        let l2 = d.len2();
        let t = if l2 > 0.0 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
        a + d * t
    };
    let mut best = (f64::INFINITY, a1, a2);
    for (p, on_first) in [(a1, false), (b1, false), (a2, true), (b2, true)] {
        let (q, r) = if on_first { (p, near(p, a1, b1)) } else { (p, near(p, a2, b2)) };
        let d = (q - r).len();
        if d < best.0 {
            best = if on_first { (d, r, q) } else { (d, q, r) };
        }
    }
    best
}

/// A copper object the drawn centre lines must keep away from (static: pads, holes, board edge,
/// foreign shapes, keepouts; dynamic: copper accepted so far).
#[derive(Clone, Debug)]
pub(super) struct Ent {
    pub(super) core: Core,
    /// Radius around the core that is copper (half width of a track, radius of a via).
    pub(super) grow: f64,
    /// Bit per signal layer the object is on.
    pub(super) mask: u64,
    /// `None`: an obstacle for every net.
    pub(super) net: Option<i32>,
    /// Clearance the object itself demands (nm).
    pub(super) clear: f64,
    /// The demanded clearance replaces the wire's own (holes, board edge, keepouts).
    pub(super) fixed: bool,
    pub(super) live: bool,
    /// A copper shape (footprint polygon), not a pad: vias keep their hole clearance to it.
    pub(super) shape: bool,
}

#[derive(Clone, Debug)]
pub(super) enum Core {
    Poly(Vec<Vec2>),
    Seg(Vec2, Vec2),
    Pt(Vec2),
}

pub(super) fn bbox_of(pts: &[Vec2]) -> Aabb {
    let (mut lo, mut hi) = (pts[0], pts[0]);
    for p in pts {
        lo = Vec2 { x: lo.x.min(p.x), y: lo.y.min(p.y) };
        hi = Vec2 { x: hi.x.max(p.x), y: hi.y.max(p.y) };
    }
    Aabb { min: lo, max: hi }
}

/// Even-odd point in a polygon (any shape).
fn inside_poly(p: Vec2, poly: &[Vec2]) -> bool {
    super::state::point_in_poly(p, poly)
}

/// `(distance, closest point on the core, closest point on the segment)` from `a-b` to `core`.
pub(super) fn seg_core(a: Vec2, b: Vec2, core: &Core) -> (f64, Vec2, Vec2) {
    match core {
        Core::Pt(p) => {
            let d = b - a;
            let l2 = d.len2();
            let t = if l2 > 0.0 { ((*p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let c = a + d * t;
            ((c - *p).len(), *p, c)
        }
        Core::Seg(c, d) => seg_seg(a, b, *c, *d),
        Core::Poly(poly) => {
            let n = poly.len();
            if inside_poly(a, poly) || inside_poly(b, poly) {
                let c = if inside_poly(a, poly) { a } else { b };
                let mut best = (f64::INFINITY, c);
                for i in 0..n {
                    let (u, v) = (poly[i], poly[(i + 1) % n]);
                    let d = v - u;
                    let l2 = d.len2();
                    let t = if l2 > 0.0 { ((c - u).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
                    let q = u + d * t;
                    if (q - c).len() < best.0 {
                        best = ((q - c).len(), q);
                    }
                }
                return (0.0, best.1, c);
            }
            let mut best = (f64::INFINITY, a, a);
            for i in 0..n {
                let r = seg_seg(a, b, poly[i], poly[(i + 1) % n]);
                if r.0 < best.0 {
                    best = (r.0, r.2, r.1);
                }
            }
            best
        }
    }
}

/// Distance between the segments `a1-b1` and `a2-b2` (0 when they cross).
/// The point of the polylines `trunk` nearest to `p`, with the unit directions of the trunk
/// leaving it (two inside a segment, one or two at a vertex). `None` without a segment.
fn trunk_foot(trunk: &[&[Vec2]], p: Vec2) -> Option<(Vec2, Vec<Vec2>)> {
    let mut best: Option<(f64, Vec2, usize, usize, f64)> = None;
    for (ri, pts) in trunk.iter().enumerate() {
        for i in 0..pts.len().saturating_sub(1) {
            let (a, b) = (pts[i], pts[i + 1]);
            let ab = b - a;
            if ab.len2() < 1.0 {
                continue;
            }
            let t = ((p - a).dot(ab) / ab.len2()).clamp(0.0, 1.0);
            let q = a + ab * t;
            let d = (q - p).len();
            if best.is_none_or(|x| d < x.0) {
                best = Some((d, q, ri, i, t));
            }
        }
    }
    let (_, q, ri, i, t) = best?;
    let pts = trunk[ri];
    let mut arms = Vec::new();
    let mut push = |d: Vec2| {
        if let Some(u) = d.unit() {
            arms.push(u);
        }
    };
    // Away from q along the trunk on either side (a vertex counts as the nearer segment end).
    let back = if t <= 1e-9 { i.checked_sub(1).map(|j| pts[j]) } else { Some(pts[i]) };
    let fwd = if t >= 1.0 - 1e-9 { pts.get(i + 2).copied() } else { Some(pts[i + 1]) };
    if let Some(x) = back {
        push(x - q);
    }
    if let Some(x) = fwd {
        push(x - q);
    }
    Some((q, arms))
}

/// The failure reason of a run for which no attempt of the drawing ladder produced copper to
/// check (the exact path found none and no later attempt ran): a drawing failure, so the
/// never-drop pass draws it ([`Options::allow_violations`](crate::model::Options)). O(layers).
/// The reason a run that drew nothing fails with: the last attempt's own reason, or, when no
/// attempt produced copper to check (an empty reason), [`no_run_reason`] (M6 F-1: never blank, so
/// the never-drop pass, keyed on "drawing:", draws the wire and the report says why). O(layers).
pub fn failed_run_reason(last: String, model: &BoardModel, lid: u32) -> String {
    if last.is_empty() {
        no_run_reason(model, lid)
    } else {
        last
    }
}

pub fn no_run_reason(model: &BoardModel, lid: u32) -> String {
    format!(
        "drawing: no run found on layer {} (the exact taut path had no legal result and no other attempt gave one)",
        model.layers.iter().find(|l| l.id == lid).map_or(lid.to_string(), |l| l.name.clone())
    )
}

/// How many run vertices the fuse pass tries per wire end and trunk (main's `take(8)`, one link per vertex).
pub const FUSE_VERTEX_CAP: usize = 8;

/// The candidates of the `cap` best vertices (best = first in the gain order), in their order: every shape
/// of those vertices is kept, also one that comes after a later vertex's first candidate (fuse-octi r4: a
/// running filter dropped the later shapes of an early vertex once a 9th vertex had appeared). O(n cap).
pub fn fuse_cap(c: Vec<(f64, f64, usize, Vec<Vec2>)>, cap: usize) -> Vec<(f64, f64, usize, Vec<Vec2>)> {
    let mut seen: Vec<usize> = Vec::new();
    for x in &c {
        if !seen.contains(&x.2) && seen.len() < cap {
            seen.push(x.2);
        }
    }
    c.into_iter().filter(|x| seen.contains(&x.2)).collect()
}

/// One wire end in the fuse pass's queue (fuse-octi r4, M1): `key` = the price gain of its best
/// LEGAL fuse (`-inf`: none) as evaluated at the queue's version `stamp` (the version counts the
/// fuses taken; a key older than its net's last fuse is stale: that fuse moved trunks and anchors
/// or took a pad, so it may be too high or too low; [`lazy_taken`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FuseKey {
    pub key: f64,
    pub stamp: usize,
    pub w: WireId,
    pub end: usize,
    pub net: i32,
}

/// The fuse queue's next step ([`lazy_next`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LazyStep {
    /// Take the end at this index: its key is fresh and the best.
    Take(usize),
    /// Evaluate the end at this index again on the current copper (its key is stale).
    Reeval(usize),
    Done,
}

/// The best-first order of the fuse pass with lazy re-evaluation (fuse-octi r4, M1; after the
/// accelerated greedy of Minoux 1978, "Accelerated greedy algorithms for maximizing submodular set
/// functions", LNCIS 7, pp. 234-243, and CELF, Leskovec et al. 2007, KDD, sec. 4.1: keep the keys,
/// make again only those a step made stale): the end with the best key goes first (ties by wire id,
/// then end: deterministic). A stale key is made again before anything is taken: a fuse can raise
/// as well as lower the gains of its own net (its new link is copper the next foot can land on; its
/// foot is an anchor; it took a pad), so a stale key is no bound (the submodular case of Minoux's
/// lazy test does not hold) and only the stale ones are made again ([`lazy_taken`]: the net of the
/// last fuse). Two wires that meet at one pad can fuse only one of them, so the large legal gain must
/// go first whatever the wire ids (fuse-octi r3: an id order let a 2.7 mm fuse block a 43 mm one on
/// qmtech JS1_2). `Done` once every end left has a fresh `-inf` (no legal candidate). O(pool).
pub fn lazy_next(pool: &[FuseKey], version: usize) -> LazyStep {
    if let Some(i) = (0..pool.len()).filter(|&i| pool[i].stamp != version).min_by_key(|&i| (pool[i].w, pool[i].end)) {
        return LazyStep::Reeval(i);
    }
    (0..pool.len())
        .filter(|&i| pool[i].key.is_finite())
        .min_by(|&a, &b| {
            let (x, y) = (&pool[a], &pool[b]);
            y.key.total_cmp(&x.key).then(x.w.cmp(&y.w)).then(x.end.cmp(&y.end))
        })
        .map_or(LazyStep::Done, LazyStep::Take)
}

/// After a fuse of net `net` at queue version `version`: the new version; the keys of the other
/// nets that were fresh stay fresh (a fuse changes the candidates of its own net only: its trunks,
/// anchors and pads; its copper can block another net's candidate, which [`Drawer::refit_run`]
/// finds when that one is taken, and then it is evaluated again). Known limit (r5 verifier L3): a
/// fuse also FREES the room its old copper took, so another net's candidate that was illegal there
/// can become legal; its key stays the lower one it had (deterministic, at worst a smaller fuse than
/// possible for that end; re-pricing every net after each fuse costs a pass per fuse). O(pool).
pub fn lazy_taken(pool: &mut [FuseKey], version: usize, net: i32) -> usize {
    for k in pool.iter_mut() {
        if k.net != net && k.stamp == version {
            k.stamp = version + 1;
        }
    }
    version + 1
}

/// The style the fuse prices its links with: the octilinear style's config and the wire's pitch
/// (w + s, nm: the jog leg), `None` in the smooth style (the fuse is then length and junction
/// angle only). O(1).
pub fn fuse_style(octilinear: bool, cfg: &super::octi::OctiCfg, pitch: f64) -> Option<(&super::octi::OctiCfg, f64)> {
    octilinear.then_some((cfg, pitch))
}

/// Shortcuts of a branch run onto its trunk (D6b fuse; TopoR's FreeStyle auto move of T-points,
/// black-box E8c-E8e): `run` starts on the trunk's copper (`trunk`, the trunk's polylines on the
/// run's layer, math frame); leaving the trunk at the foot `q` of a later vertex `run[i]`
/// instead replaces `run[0..=i]` by a link `q -> run[i]`. Price per candidate (mm): its length
/// plus the `junction_angle` price `jw x ws_mm x psi(angle to the trunk)`, and with `style`
/// (`[drawing] style = "octilinear"`: the DP's prices and the wire's pitch w + s, nm) the
/// octilinear DP's style price of the whole run ([`super::octi::style_price`]: bends, jogs, acute
/// bends, `any_angle` per stretch), so a fuse prices on the first draw's scale (octi-bundle r2,
/// the user's A1c rule: every redraw path uses the first draw's rules). With `style` the links
/// are the DP's connectors ([`super::octi::link_shapes`]: straight, two elbows, two centred Zs)
/// and the straight any-angle link, which pays `any_angle` (allowed, graded, no cutoff); without,
/// the straight link only. Returns the candidates that are cheaper than the run as it is, the
/// cheapest first, as `(price gain, length gain mm, i, new run)`: the price gain is the length
/// gain plus the junction-angle and style price gains (a price, not a length: psi grows without
/// bound as the branch leaves the trunk tangentially). The caller checks each with the full
/// checker (every clearance against all drawn copper, [`Drawer::refit_run`]).
/// O(run vertices x (trunk segments + shapes x run vertices)).
pub fn fuse_candidates(run: &[Vec2], trunk: &[&[Vec2]], ws_mm: f64, jw: f64, style: Option<(&super::octi::OctiCfg, f64)>) -> Vec<(f64, f64, usize, Vec<Vec2>)> {
    let price = |dir: Vec2, arms: &[Vec2]| -> f64 {
        let Some(u) = dir.unit() else { return 0.0 };
        if arms.is_empty() || jw <= 0.0 {
            return 0.0;
        }
        let deg = arms.iter().map(|a| a.dot(u).clamp(-1.0, 1.0).acos().to_degrees()).fold(180.0, f64::min);
        jw * ws_mm * super::factors::junction_psi(deg)
    };
    let style_mm = |pts: &[Vec2]| -> f64 { style.map_or(0.0, |(cfg, pitch)| super::octi::style_price(cfg, pitch, pts) / 1e6) };
    let mut out = Vec::new();
    if run.len() < 2 {
        return out;
    }
    let Some((_, arms0)) = trunk_foot(trunk, run[0]) else { return out };
    let mut along = 0.0;
    let old0 = price(run[1] - run[0], &arms0);
    // The run as drawn may hold a piece the style forbids (kept where nothing else fitted): it is
    // priced with the bans lifted; a reversal in it (no finite price) leaves the style out of the
    // comparison, as without `style`.
    let old_style = style.map_or(0.0, |(cfg, pitch)| {
        let soft = super::octi::OctiCfg { any_forbidden: false, acute_forbidden: false, ..*cfg };
        super::octi::style_price(&soft, pitch, run) / 1e6
    });
    for i in 1..run.len() {
        along += (run[i] - run[i - 1]).len() / 1e6;
        let Some((q, arms)) = trunk_foot(trunk, run[i]) else { continue };
        // The links q -> run[i]: the straight one, and with the style the DP's connectors.
        let mut links: Vec<Vec<Vec2>> = vec![vec![run[i]]];
        if style.is_some() && super::octi::octi_dir(run[i] - q).is_none() {
            links.extend(super::octi::link_shapes(q, run[i]));
        }
        for link in links {
            let mut pts = vec![q];
            pts.extend_from_slice(&link[..link.len() - 1]);
            pts.extend_from_slice(&run[i..]);
            pts.dedup_by(|a, b| (*a - *b).len2() < 1.0);
            if pts.len() < 2 {
                continue;
            }
            let mut from = q;
            let mut d = 0.0;
            for &p in &link {
                d += (p - from).len() / 1e6;
                from = p;
            }
            let new_style = style_mm(&pts);
            if !new_style.is_finite() {
                continue;
            }
            let style_gain = if old_style.is_finite() { old_style - new_style } else { 0.0 };
            let gain = along + old0 - d - price(pts[1] - pts[0], &arms) + style_gain;
            if gain > 1e-3 {
                out.push((gain, along - d, i, pts));
            }
        }
    }
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    out
}

/// How far past its pad's outline a neck reaches out of the pad its run ends in (nm): above
/// KiCad's polygon error (at most ~5.6 um inside the round end at 5 um, `geom::kicad`; tidy
/// tests the same polygon), so the
/// narrow piece from the pad centre is never a track wholly inside the pad.
const NECK_PAD_OUT: f64 = 20_000.0;

/// draw-via-neck r6 (user 2026-11-26: every trace connects to the MIDDLE of its pad): the neck
/// zones of a run that ends in its own pad (`run` from that end outwards, `poly` the pad
/// outline) grown so no width step lies inside the pad. A zone that covers any part of the run
/// inside the pad (or the first `NECK_PAD_OUT` past its outline) is widened to cover all of it,
/// so the run is narrow from its end to `NECK_PAD_OUT` outside the pad. Otherwise the piece
/// between the pad centre and the step lies wholly inside the pad, KiCad's Cleanup deletes it
/// (`TRACKS_CLEANER::deleteTracksInPads`; the cleanup-safe tidy drops it first) and the trace
/// ends off the centre (qmtech one cycle: U9.9, U7.3 with neck_trim). Narrower copper on the
/// same centre line adds no violation. A run that never leaves the pad (a via in the pad) is
/// necked over its whole length when a zone covers any of it (qmtech U7.3); a run whose end is
/// not in the pad is left alone. O(zones x run points).
pub(super) fn neck_out_of_pad(neck: &mut super::exact::Neck, run: &[Vec2], poly: &[Vec2]) {
    use super::state::point_in_poly;
    if run.len() < 2 || poly.len() < 3 || !point_in_poly(run[0], poly) {
        return;
    }
    // The run inside the pad and `NECK_PAD_OUT` past its outline; all of it when the run never
    // leaves the pad (a via in the pad: one width from the centre to the via).
    let pre: Vec<Vec2> = match run.iter().position(|&p| !point_in_poly(p, poly)) {
        None => run.to_vec(),
        Some(j) => {
            let (a, b) = (run[j - 1], run[j]);
            let (mut lo, mut hi) = (0.0f64, 1.0f64);
            for _ in 0..30 {
                let m = 0.5 * (lo + hi);
                if point_in_poly(a + (b - a) * m, poly) {
                    lo = m;
                } else {
                    hi = m;
                }
            }
            let exit = a + (b - a) * hi;
            let l = (b - exit).len();
            let out = if l > 1.0 { exit + (b - exit) * (NECK_PAD_OUT.min(l) / l) } else { b };
            let mut pre: Vec<Vec2> = run[..j].to_vec();
            pre.push(exit);
            pre.push(out);
            pre
        }
    };
    for z in neck.ends.iter_mut() {
        let Some((c, r)) = *z else { continue };
        if !r.is_finite() {
            continue;
        }
        if !pre.windows(2).any(|s| seg_dist(c, s[0], s[1]) <= r) {
            continue;
        }
        let need = pre.iter().map(|&p| (p - c).len()).fold(r, f64::max);
        if need > r {
            *z = Some((c, need + 1.0));
        }
    }
}

pub fn seg_seg_dist(a1: Vec2, b1: Vec2, a2: Vec2, b2: Vec2) -> f64 {
    seg_seg(a1, b1, a2, b2).0
}

fn kicad_pt(p: Vec2) -> IVec2 {
    IVec2 { x: p.x.round() as i32, y: (-p.y).round() as i32 }
}

/// A track item for the segment `a -> b` (math frame in, KiCad frame out).
/// Clearance a via of `net` (copper diameter `d`, drill `drill`, nm) demands around its copper
/// disc: the net's copper clearance, or more where the hole clearance to its drill
/// ([`crate::model::Rules::via_hole_clear`]) reaches further (`drill / 2 + hole clearance >
/// d / 2 + clearance`, e.g. a 0.4 / 0.3 mm via with 0.2 mm clearance and 0.254 mm hole
/// clearance). Both are discs about the via centre.
/// The same rule as the plan's ([`crate::vias::via_need_copper`], 5.4 task 1: one function).
pub fn via_clear(model: &BoardModel, net: i32, d: i64, drill: i64) -> f64 {
    crate::vias::via_need_copper(&model.rules, net, ViaSize { d, drill }, 0.0, 0.0) - 0.5 * d as f64
}

pub fn track_item(net: i32, layer: u32, w: f64, a: Vec2, b: Vec2, wire: Option<usize>) -> Item {
    Item::Track(TrackItem {
        net,
        layer,
        width: w.round() as i64,
        prim: TrackPrim::Seg { a: kicad_pt(a), b: kicad_pt(b) },
        group: None,
        wire,
    })
}

/// A through-via item at `at` (math frame in).
pub fn via_item(net: i32, at: Vec2, size: ViaSize, wire: Option<usize>) -> Item {
    let p = kicad_pt(at);
    Item::Via(ViaItem {
        kind: "via".into(),
        net,
        at: crate::model::Pt { x: i64::from(p.x), y: i64::from(p.y) },
        d: size.d,
        drill: size.drill,
        layers: Vec::new(),
        group: None,
        wire,
    })
}

/// Fractions along each pass's edge `(lo, hi, nominal)`, by pass id (`NaN` for a pass that is
/// not packed).
#[derive(Clone, Debug, Default)]
pub struct Intervals {
    pub lo: Vec<f64>,
    pub hi: Vec<f64>,
    pub nom: Vec<f64>,
}

/// Why a run was rejected (drawing-yield diagnosis, plan S2c item 1). `class`:
/// `packing` (a pass lies on an edge whose demand exceeds its room), `repair` (the funnel
/// through the intervals was clean, or the violation lies at a vertex the repair inserted),
/// `chord` (a segment of the funnel itself cuts an obstacle's clearance and the repair did not
/// fix it).
#[derive(Clone, Debug, Default)]
pub struct Diag {
    pub wire: WireId,
    pub net: i32,
    pub layer: usize,
    pub width_nm: f64,
    pub passes: usize,
    /// Passes `k0..k1` of the rejected run.
    pub k0: usize,
    pub k1: usize,
    pub over_passes: usize,
    pub class: &'static str,
    pub against: String,
    pub gap_nm: i64,
    pub req_nm: i64,
    /// Midpoint of the violating segment of the last attempt.
    pub at: Vec2,
    pub from: Vec2,
    pub to: Vec2,
    /// Per attempt: the funnel before the repair violates.
    pub pre_bad: [bool; 3],
    pub bumps: usize,
    /// The exact search also found no path with the drawn copper ignored: the topology does not
    /// fit the static board (pads, holes, edge, keepouts, existing copper) at this width.
    pub static_infeasible: bool,
    /// The violated object is a via the router placed (diagnosis: does a wire lose to a via?).
    pub against_via: bool,
}

/// The plan's crossings of one wire with other nets: `(traversal, other net, other wire's layer
/// there or -1 = open)` ([`Drawer::class_crossings`], read by [`Drawer::via_reach`]).
pub type ClassCrossings = std::collections::BTreeSet<(usize, i32, i32)>;

/// Counters of the drawing: which method produced each accepted run.
#[derive(Clone, Copy, Debug, Default)]
pub struct DrawStats {
    /// Runs accepted from the exact taut path.
    pub exact_ok: usize,
    /// draw-via-neck C: runs re-anchored at their pad's tip when drawn (`[drawing] pad_tip_anchor`).
    pub tip_anchored: usize,
    /// Attempts where the exact search found no path (or hit its cap).
    pub exact_none: usize,
    /// Runs accepted only by S2a's funnel + repair fallback.
    pub fallback_ok: usize,
    /// Seconds spent in the exact search (successful or not).
    pub exact_s: f64,
    /// Never drop (M-B1): runs drawn by force, by the candidate that won: the exact path with the
    /// clearance relaxed, a funnel before the repair, one of the normal attempts.
    pub forced_relaxed: usize,
    pub forced_funnel: usize,
    pub forced_attempt: usize,
    /// forced-draw: forced runs drawn in their octilinear form (the cheaper by
    /// [`forced_form_key`]), and the forced runs that keep free-angle stretches (their number and
    /// length, mm): their octilinear form costs more (or there is none).
    pub forced_octi: usize,
    /// draw-absorb A1 (`[drawing] forced_shrink`): forced runs necked over their violating
    /// stretches (fewer or shallower violations).
    pub forced_shrunk: usize,
    pub forced_free_runs: usize,
    pub forced_free_stretches: usize,
    pub forced_free_mm: f64,
    /// M-45: runs redrawn octilinear; octilinear runs the checker rejected (the taut run kept);
    /// seconds in the octilinear search.
    pub octi_ok: usize,
    pub octi_rejected: usize,
    pub octi_s: f64,
    /// first-draw-45 P1: in the runs drawn octilinear, octilinear pieces of the taut path kept as
    /// drawn where the DP's leg test refused them, and of those the ones below the clearance.
    pub octi_asis: usize,
    pub octi_asis_below: usize,
    /// 5.4: vias placed at the plan's own site / planned sites that broke a rule there (searched).
    pub planned_sites: usize,
    pub planned_sites_missed: usize,
    /// 5.4 task 2 (plan vs drawn vias): vias of the plan placed by the drawing's own site search
    /// (no planned site, or the planned one not free), and by force (never drop, M-B1), per
    /// committed via (a wire drawn again counts again; the summary uses the final wires).
    pub via_searched: usize,
    pub via_forced: usize,
    /// Vias placed out of the plan's class ([`Drawer::via_reach`]): no site in the class was found
    /// at all, so the cheapest one outside it was taken (counted in the plan-vs-drawn summary).
    pub via_out_of_class: usize,
    /// P6 (c): vertices the taut pull removed from drawn runs ([`Drawer::pull_taut`]).
    pub pulled: usize,
    /// M3 `[drawing] realise = "erbs"`: runs drawn from their ERBS; runs whose ERBS the checker
    /// rejected (the sequential attempts drew them: the fallback); runs without an ERBS (a
    /// branch ending on its trunk, a moved junction, a stretch the measurement could not band).
    pub erbs_ok: usize,
    pub erbs_fallback: usize,
    pub erbs_none: usize,
    /// Of `erbs_ok`: drawn in the ERBS's octilinear form because the any-angle ERBS missed the
    /// clearance to octilinear copper drawn before it.
    pub erbs_octi: usize,
    /// M3 length factor ([`erbs_length_price`]): legal ERBS whose price passed `B (1 + g)`, so a
    /// sequential run was drawn to compare; of those, the ERBS a cheaper sequential run replaced.
    pub erbs_long: usize,
    pub erbs_long_replaced: usize,
    /// Round i: of `erbs_long`, competitors that would have replaced the ERBS but cross an undrawn
    /// wire's path with another parity than it ([`competitor_keeps_class`]): the ERBS is drawn.
    pub erbs_class_kept: usize,
    /// edge-clear ([`super::padend`]): pad ends moved off the pad centre (a track end there broke a
    /// static clearance), the largest move (nm), and the pad ends with no legal point in the pad
    /// at the wire's width (left at the centre, priced by never drop).
    pub pad_end_moved: usize,
    pub pad_end_max: f64,
    pub pad_end_none: usize,
}

/// M3 length factor (`[drawing] erbs_length_weight` k): the price of a legal ERBS of length `le`
/// over a band of length `lb`, `le (1 + k max(0, le/lb - 1)^2)`: a smooth price that grows with
/// the factor `le / lb` and is the length itself up to the band (no cutoff). The ERBS is the
/// shortest path of the band's class with the spoke tips as terminals (Dayan 1997, PDF pp. 25-26),
/// so it stays near its band where the class is right; a wound one (a wrong class, many times its
/// band) prices itself out against a sequential run. O(1).
pub fn erbs_length_price(le: f64, lb: f64, k: f64) -> f64 {
    if lb <= 0.0 {
        return le;
    }
    let x = (le / lb - 1.0).max(0.0);
    le * (1.0 + k.max(0.0) * x * x)
}

/// The sequential run that competes with a legal ERBS of price `pe` (band length `lb`,
/// `[drawing] erbs_margin` g): it is drawn only when `pe` passes `lb (1 + g)` (a sequential run of
/// the same class is not shorter than its band, so below that it cannot win), and it replaces the
/// ERBS when its own length `ls` priced at `1 + g` (the room the ERBS keeps for the others) is
/// below `pe`. `None` = not worth drawing; `Some(true)` = the sequential run replaces the ERBS.
pub fn erbs_vs_sequential(pe: f64, lb: f64, g: f64, ls: Option<f64>) -> Option<bool> {
    let g = g.max(0.0);
    if lb > 0.0 && pe <= lb * (1.0 + g) {
        return None;
    }
    Some(ls.is_some_and(|ls| ls * (1.0 + g) < pe))
}

/// Crossings ([`proper_crossings`]) of the run `run` with each path of `others` (key, path), per
/// key (the keys it crosses at least once; paths with the same key add up). The class test keys
/// by (wire, path): the parity of each path is its own invariant, two odd paths of one wire must
/// not cancel (verifier round i). O(|run| x sum |path|), boxes first.
pub fn crossings_per_wire<K: Ord + Copy>(run: &[Vec2], others: &[(K, &[Vec2])]) -> BTreeMap<K, usize> {
    let mut out = BTreeMap::new();
    if run.len() < 2 {
        return out;
    }
    let bb = |p: &[Vec2]| p.iter().fold((Vec2 { x: f64::INFINITY, y: f64::INFINITY }, Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY }), |(lo, hi), q| (Vec2 { x: lo.x.min(q.x), y: lo.y.min(q.y) }, Vec2 { x: hi.x.max(q.x), y: hi.y.max(q.y) }));
    let (rlo, rhi) = bb(run);
    for &(o, path) in others {
        if path.len() < 2 {
            continue;
        }
        let (lo, hi) = bb(path);
        if lo.x > rhi.x || hi.x < rlo.x || lo.y > rhi.y || hi.y < rlo.y {
            continue;
        }
        let n = path.windows(2).map(|b| proper_crossings(run, b[0], b[1])).sum::<usize>();
        if n > 0 {
            *out.entry(o).or_insert(0) += n;
        }
    }
    out
}

/// Round i: the sequential run competing with a legal ERBS (`erbs_vs_sequential`) is of the plan's
/// class only if, for every undrawn other-net wire, it crosses that wire's reference path as often
/// mod 2 as the ERBS does (`seq`, `erbs`: [`crossings_per_wire`] of the two runs against those
/// paths). The parity of the crossings of an arc with a cut between two fixed points (the other
/// wire's path between its terminals) is a homotopy invariant; the raw count is not (a bigon of two
/// extra crossings comes and goes with the representatives: qmtech HDMI5V crossed ADV_SCL's path
/// 4 times sequentially, 2 times as ERBS, one class). A run of the other parity takes a path the
/// plan gave that wire (a planned short once it is drawn: qmtech ADV_CLK, drawn before ADV_DE, cut
/// straight across ADV_DE's ERBS to its pad and left ADV_DE no legal geometry); it does not compete
/// and the ERBS is drawn (Dayan 1997 PDF pp. 25-26: the ERBS is a member of the class; Polubasov
/// 2010: the drawing realises the topology, it does not change it). The taut pull holds the same
/// invariant against undrawn chords ([`Drawer::pull_taut`]). Sound, not complete: a different
/// parity proves a different class, an equal one does not prove the same. O(wires crossed).
pub fn competitor_keeps_class<K: Ord>(seq: &BTreeMap<K, usize>, erbs: &BTreeMap<K, usize>) -> bool {
    seq.keys().chain(erbs.keys()).all(|o| seq.get(o).copied().unwrap_or(0) % 2 == erbs.get(o).copied().unwrap_or(0) % 2)
}

/// Round i: the reference paths of the class test ([`competitor_keeps_class`]): per wire and
/// layer its ERBS runs, or, where the wire has no ERBS on that layer (its sleeve failed, the time
/// limit), its planned path (the chords through its passes' nominal crossings and its turns; a
/// passless wire's terminal-to-terminal chord on its plan's layers, round j), so no undrawn wire
/// is skipped; segments in a grid of their boxes (a competitor is tested against the paths whose
/// boxes it meets only).
pub(super) struct ClassRef {
    paths: Vec<(WireId, usize, Vec<Vec2>)>,
    grid: Grid<(usize, usize)>,
}

/// Round i: the wires a run of wire `w` (net `net_w`) is class-tested against: every other wire
/// `o` that is alive, of another net (a same-net wire may cross it) and not drawn yet (a drawn
/// wire's reference path is not its copper; the checker sees the copper). O(1).
pub(super) fn class_test_wire(w: WireId, net_w: i32, o: WireId, o_alive: bool, o_net: i32, o_drawn: bool) -> bool {
    o != w && !o_drawn && o_alive && o_net != net_w
}

/// Round i/j: the layers of traversal `ok` (`0..=n`) of a wire with `n` passes on which its
/// planned chord is a reference path: the layers of the passes either side (`lay(j)`, `< 0` =
/// open or none), for a passless wire (`n == 0`) the layers of its plan (`plan`: terminal to
/// terminal), each once. O(plan layers).
pub(super) fn ref_trav_layers(n: usize, ok: usize, lay: &dyn Fn(usize) -> i32, plan: &[usize]) -> Vec<usize> {
    let mut ls: Vec<usize> = if n == 0 {
        plan.to_vec()
    } else {
        [if ok > 0 { lay(ok - 1) } else { -2 }, if ok < n { lay(ok) } else { -2 }].into_iter().filter(|&l| l >= 0).map(|l| l as usize).collect()
    };
    ls.sort_unstable();
    ls.dedup();
    ls
}

fn aabb_of(a: Vec2, b: Vec2) -> Aabb {
    Aabb { min: Vec2 { x: a.x.min(b.x), y: a.y.min(b.y) }, max: Vec2 { x: a.x.max(b.x), y: a.y.max(b.y) } }
}

impl ClassRef {
    /// The reference paths `(wire, layer, path)` in a grid of their segments' boxes. O(segments).
    pub(super) fn new(paths: Vec<(WireId, usize, Vec<Vec2>)>) -> Self {
        let mut grid = Grid::new(1.0e6);
        for (i, p) in paths.iter().enumerate() {
            for (k, s) in p.2.windows(2).enumerate() {
                let _ = grid.try_insert(aabb_of(s[0], s[1]), (i, k));
            }
        }
        ClassRef { paths, grid }
    }

    /// Crossings of `run` (on `layer`) with the reference paths on that layer of the wires `keep`
    /// admits, per (wire, path index). O(segments of the run x paths in their cells).
    fn crossings(&self, run: &[Vec2], layer: usize, keep: &dyn Fn(WireId) -> bool) -> BTreeMap<(WireId, usize), usize> {
        let mut ids: Vec<usize> = run.windows(2).flat_map(|s| self.grid.query(aabb_of(s[0], s[1]))).map(|(i, _)| i).collect();
        ids.sort_unstable();
        ids.dedup();
        let others: Vec<((WireId, usize), &[Vec2])> = ids.into_iter().filter_map(|i| self.paths.get(i).map(|p| (i, p))).filter(|(_, p)| p.1 == layer && keep(p.0)).map(|(i, p)| ((p.0, i), p.2.as_slice())).collect();
        crossings_per_wire(run, &others)
    }
}

/// Per-stretch widths that replace a wire's single width (the drawing witness, variant A: the
/// hand's own widths, necked where the hand necks). `pass_w` is the width of every pass for the
/// packing; `seg` the width of a drawn segment `(wire, layer, a, b)`; `min_w` the narrowest width
/// of each wire (the exact search adds clearance circles for it).
pub struct Widths<'a> {
    pub pass_w: Vec<f64>,
    pub min_w: Vec<f64>,
    #[allow(clippy::type_complexity)]
    pub seg: Box<dyn Fn(WireId, usize, Vec2, Vec2) -> f64 + Send + Sync + 'a>,
}

/// Where a pass sits in its packing: `(edge, layer, index in the lane, lmin, rmax)` (nm).
type Slot = (EdgeId, usize, usize, f64, f64);

/// The passes of one edge-layer that fits, in edge order, with the prefix sums of the required
/// spacing along the edge (`cum[i]`, pass 0 to pass `i`).
#[derive(Clone, Debug)]
struct Lane {
    pids: Vec<PassId>,
    cum: Vec<f64>,
}

/// One drawn wire.
#[derive(Clone, Debug, Default)]
pub struct WireOut {
    pub wire: WireId,
    pub ok: bool,
    pub reason: String,
    /// Tracks and vias to emit (KiCad frame).
    pub items: Vec<Item>,
    /// `(layer index, polyline)` per run (math frame).
    pub runs: Vec<(usize, Vec<Vec2>)>,
    /// Via centres and sizes, in path order.
    pub vias: Vec<(Vec2, ViaSize)>,
    pub length_nm: f64,
    /// Handles of the committed track items (for [`Drawer::uncommit`]).
    pub handles: Vec<usize>,
    /// Necked pad exits: `(pad end position, width nm, length nm)`; the wire's own width is
    /// above that of the neck.
    pub necks: Vec<(Vec2, f64, f64)>,
    /// Rules the written copper breaks (M-B1 never drop: a run or via drawn by [`Drawer::force`]
    /// on its best geometry); empty for a legal wire.
    pub viol: Vec<Viol>,
    /// TW2: how each run of `runs` came about (`realise = "erbs"`; [`super::joint::tally`]).
    pub run_src: Vec<super::joint::RunSrc>,
    /// exist-term: length (nm) of the leads drawn for continued dangling ends (in `length_nm`).
    pub lead_nm: f64,
}

/// One violation per object and layer (a via: per object, over all its layers): the stretches of a run's segments below
/// the rule against the same object are one violation, as deep as the deepest, as long as their
/// sum, at the deepest place (a run drawn as more segments is not more violations). Order kept.
pub fn merge_viols(vs: Vec<Viol>) -> Vec<Viol> {
    let rank = |k: &str| match k {
        "short" => 3,
        "hole_clearance" => 2,
        "connection_width" => 1,
        _ => 0,
    };
    let mut out: Vec<Viol> = Vec::new();
    for v in vs {
        // A via's violation is reported per layer by the checker: one per object it is too close to.
        match out.iter_mut().find(|o| o.via == v.via && o.against == v.against && if v.via { (o.at - v.at).len() < 2.0 } else { o.layer == v.layer }) {
            Some(o) => {
                if !v.via {
                    o.len_nm += v.len_nm;
                }
                if v.depth_nm > o.depth_nm {
                    o.depth_nm = v.depth_nm;
                    o.at = v.at;
                }
                if rank(v.kind) > rank(o.kind) {
                    o.kind = v.kind;
                }
            }
            None => out.push(v),
        }
    }
    out
}

/// draw-absorb A0 (docs/dev/design/draw-absorb.md): where a drawn violation comes from, recorded
/// when the violating copper is written. Measurement only: nothing in the drawing reads it (it is
/// not part of [`Viol`]'s equality), so the copper is the same with or without it. The report
/// ([`crate::route::ViolProv`]) adds the drawn compass class and the fault class F1-F7.
#[derive(Clone, Debug, Default)]
pub struct ViolSrc {
    /// The writer: `forced_run` (a never-drop run, [`Drawer::draw_wire`]'s force branch),
    /// `forced_via` (a via placed by [`Drawer::force_via`]), `gap_kept` (a band kept across a
    /// model gap, drawn second: TW2 `erbs_gap_keep_max_mm`), `junction_join` (a branch joined to its
    /// trunk by force, D5), `diff_pair` (D7 pair realisation); empty = not recorded.
    pub mech: &'static str,
    /// The forced run's candidate ([`ForcedCand`] source): `relaxed` (exact path with the clearance
    /// relaxed), `funnel`, `erbs` (its band), `erbs_kept`, `attempt0`..`attempt5` (the ladder
    /// attempt whose copper was forced); `via` for a forced via, `band` for a gap-kept band.
    pub cand: &'static str,
    /// The forced run's octilinear form: `ok` (a form won), `rejected`, `none`, `skip`.
    pub form: &'static str,
    /// The run's band status ([`super::joint::CutClass`] name: `static`, `predicted`, `gap`,
    /// `no_band`, `junction`, `redrawn`; `kept` = a reserved band drawn second); empty without ERBS.
    pub cut: &'static str,
    /// Run index of the wire (a via: the via's index).
    pub run: usize,
    /// The run's last legal attempt failed on the via-side topology test.
    pub wrong_side: bool,
    /// The plan necks the wire (M-C plan neck).
    pub plan_neck: bool,
    /// The wire's nominal width, its floor (net class / board minimum, `State::w_min`) and the
    /// narrowest drawn track of the violating run (nm; 0 for a via).
    pub w_nm: f64,
    pub w_min_nm: f64,
    pub drawn_w_nm: f64,
    /// The router wire whose copper it hits (`None`: a pad, kept copper, the board, a shape).
    pub hit_wire: Option<WireId>,
    /// That copper was a band the joint commit reserved (its wire not drawn yet).
    pub hit_reserved: bool,
    /// The hit wire's packed room (nm) on the side away from the violation, at its pass nearest to
    /// it ([`Drawer::iv`]): how far it could give way inside its own sleeve; `None` unpacked.
    pub hit_slack_nm: Option<f64>,
}

/// A rule the written copper of a wire breaks (M-B1 never drop), priced by the `violation`
/// factor ([`super::factors::violation_amount`]). Equality leaves out `src` (a measurement).
#[derive(Clone, Debug)]
pub struct Viol {
    /// `clearance`, `short` (copper overlap), `hole_clearance`, `edge_clearance`,
    /// `connection_width` (a sliver against a pad or via of its own net).
    pub kind: &'static str,
    /// Middle of the stretch below the rule (math frame).
    pub at: Vec2,
    /// KiCad layer id.
    pub layer: u32,
    /// How far below the rule (nm); an overlap counts its whole depth.
    pub depth_nm: f64,
    /// How long the copper stays below the rule (nm; a via: its diameter).
    pub len_nm: f64,
    /// The object it is too close to (checker label: `pad:REF:NUM`, `item:N`, `edge`, ...).
    pub against: String,
    /// The violating item is a via.
    pub via: bool,
    /// draw-absorb A0: its provenance (measurement only).
    pub src: ViolSrc,
}

impl PartialEq for Viol {
    fn eq(&self, o: &Self) -> bool {
        self.kind == o.kind && self.at == o.at && self.layer == o.layer && self.depth_nm == o.depth_nm && self.len_nm == o.len_nm && self.against == o.against && self.via == o.via
    }
}

impl Viol {
    /// Units of the `violation` factor.
    pub fn amount(&self) -> f64 {
        super::factors::violation_amount(self.depth_nm, self.len_nm)
    }
}

/// The severity key of a forced run's violations: `(short amount, total amount)`. A short
/// (copper overlap, KiCad's `shorting_items`; Cleanup removes the track) is no graded judgement
/// call: a run with less short amount wins whatever its clearance amount, and a change that adds
/// or deepens a short is never taken (KiCad tools are the spec).
pub fn viol_key(vs: &[Viol]) -> (f64, f64) {
    (vs.iter().filter(|v| v.kind == "short").map(Viol::amount).sum(), vs.iter().map(Viol::amount).sum())
}

/// The DRC class of checker violation `v` of a via (`via`) or track item, as KiCad names it:
/// `edge_clearance`; a via's drill check or a via against a hole (`hole:`, `viahole:`):
/// `hole_clearance`; copper overlap: `short`; else `clearance`. grid-snap r3: by the checker's
/// `drill` flag, not by the rule value (with the hole clearance equal to the copper clearance a
/// via's copper deficit read as `hole_clearance`). O(1).
pub fn viol_kind(v: &Violation, via: bool) -> &'static str {
    if v.against == "edge" {
        "edge_clearance"
    } else if via && (v.drill || v.against.starts_with("hole") || v.against.starts_with("viahole")) {
        "hole_clearance"
    } else if v.gap < 0 {
        "short"
    } else {
        "clearance"
    }
}

/// The choice key of a forced via site ([`Drawer::force_via`]) with violations `vs`, the further
/// price `extra` (wrong chord side, reach) and the run's safety margin `margin` (nm):
/// `[short amount, hole amount, violation amounts + extra]`, compared in that order
/// ([`force_via_better`]).
/// - Shorts first, as for a forced run ([`viol_key`]): a via on a foreign track (KiCad:
///   `shorting_items` and the drill's `hole_clearance`) never wins over a site with clearance
///   deficits only (grid-snap r2, esp IO8_SCL over +3V3).
/// - Then the drill (grid-snap r3): the `hole_clearance` amount as KiCad measures it, the drill's
///   edge against foreign copper below the board's hole clearance (the checker's depth less the
///   margin, which KiCad does not know; a site below the margin only is no KiCad violation). A
///   drill too close to copper is its own DRC class, above plain clearance: a site with copper
///   clearance deficits only wins over one whose drill is too close (esp r2: IO8_SCL 0.007 mm).
pub fn force_via_key(vs: &[Viol], extra: f64, margin: f64) -> [f64; 3] {
    let hole: f64 = vs
        .iter()
        .filter(|v| v.kind == "hole_clearance" && v.depth_nm > margin)
        .map(|v| super::factors::violation_amount(v.depth_nm - margin, v.len_nm))
        .sum();
    [viol_key(vs).0, hole, vs.iter().map(Viol::amount).sum::<f64>() + extra]
}

/// `a` is strictly better than `b` on [`force_via_key`] (lexicographic, 1e-9 ties).
pub fn force_via_better(a: [f64; 3], b: [f64; 3]) -> bool {
    for k in 0..3 {
        if a[k] < b[k] - 1e-9 {
            return true;
        }
        if a[k] > b[k] + 1e-9 {
            return false;
        }
    }
    false
}


/// forced-draw (docs/dev/design/forced-draw.md): the graded choice key of one form of a forced
/// run (never drop), its taut candidate or that candidate's octilinear form: `(short amount,
/// price nm)`, the price = `violation` x the violation amounts + `any_angle` x the amounts of its
/// free-angle stretches + `acute_bend` x the amounts of its acute bends + its length, the weights
/// of `cfg` (the factor registry, mm per unit). Shorts first (KiCad Cleanup removes a shorting
/// track: no graded judgement, [`viol_key`]); everything else on one scale, no gate.
pub fn forced_form_key(vs: &[Viol], pts: &[Vec2], cfg: &super::octi::OctiCfg) -> (f64, f64) {
    let (any, acute) = super::octi::breaks(pts);
    let short: f64 = vs.iter().filter(|v| v.kind == "short").map(Viol::amount).sum();
    let price = cfg.viol_nm * vs.iter().map(Viol::amount).sum::<f64>()
        + cfg.any_nm * any.iter().map(|a| super::octi::any_angle_amount(a.2)).sum::<f64>()
        + cfg.acute_nm * acute.iter().map(|a| super::octi::acute_amount(180.0 - a.1)).sum::<f64>()
        + poly_len(pts);
    (short, price)
}

/// `a` is strictly cheaper than `b` on [`forced_form_key`] (less short amount, else, at equal
/// shorts, a lower price; ties keep `b`).
pub fn forced_form_better(a: (f64, f64), b: (f64, f64)) -> bool {
    a.0 < b.0 - 1e-9 || (a.0 <= b.0 + 1e-9 && a.1 < b.1 - 1e-9)
}

/// forced-draw: the form of a forced run to draw, by [`forced_form_key`] (`keys[0]` the taut
/// run, then its octilinear forms in variant order): the strictly cheapest, the first on ties.
pub fn forced_form_pick(keys: &[(f64, f64)]) -> usize {
    let mut best = 0;
    for (j, &k) in keys.iter().enumerate().skip(1) {
        if forced_form_better(k, keys[best]) {
            best = j;
        }
    }
    best
}

/// `a` is no worse than `b` on [`viol_key`]: no more short amount, and no more total amount.
fn key_le(a: (f64, f64), b: (f64, f64)) -> bool {
    a.0 <= b.0 + 1e-9 && a.1 <= b.1 + 1e-9
}

pub struct Drawer<'a> {
    /// D7: per pair lead net, the distance from the wire's own end pads inside which a via pair
    /// is placed only when no site farther away is valid (the fan-out zone needs that room).
    pub pair_keep: BTreeMap<i32, f64>,
    pub model: &'a BoardModel,
    pub opts: &'a Options,
    pub b: &'a Board,
    pub st: &'a State<'a>,
    pub checker: &'a Checker,
    pub idx: ItemIndex,
    pub outline: Vec<(Vec2, Vec2)>,
    /// Extra distance on top of every clearance (nm).
    pub eps: f64,
    /// Copper-to-board-edge clearance the drawing keeps (nm): the KiCad value, 0 when the check
    /// `copper_edge_clearance` is set to ignore (a violation is then priced and reported).
    pub edge_clear: f64,
    /// The capacity test shared with P3/P4.
    pub packer: Packer<'a>,
    /// Failed runs for the debug dump: `(wire, layer, attempts' polylines, reason)`.
    pub fails: Vec<(WireId, usize, Vec<Vec<Vec2>>, String)>,
    /// Position of every pass in its wire's pass list.
    pass_k: Vec<usize>,
    /// Edge x layer (`e * n_layers + l`) whose passes do not fit in the room between the end
    /// margins (demand over room) when packed.
    pub over: Vec<bool>,
    /// Slack of every packed edge-layer: the least `rmax - lmin` (nm; infinite when empty).
    pub spare: Vec<f64>,
    /// One record per run the checker rejected (the drawing-yield diagnosis).
    pub diag: Vec<Diag>,
    pub stats: DrawStats,
    /// Nominal polyline (pad centres and pass points) of every wire.
    pub(super) polys: Vec<Vec<Vec2>>,
    /// Crossing direction of every pass from its wire's taut path (the oblique factor, shared
    /// with P3/P4 through [`Packer::taut_dirs`]).
    pub(super) dirs: Vec<Vec<Vec2>>,
    pub iv: Intervals,
    /// Layer index of every pass (`-1` unpacked).
    pub pass_layer: Vec<i32>,
    /// Via disc diameter at every pass next to a placed (or planned) via of its wire (0: none):
    /// the packing gives the via room on every layer ([`layer_items`]), as P3/P4 do.
    pub pass_disc: Vec<Disc>,
    pub(super) ents: Vec<Ent>,
    /// The box of every object's core, by ent id (F6: the clearance tests took it from the
    /// polygon's vertices on every call; a core never changes once added).
    pub(super) ent_box: Vec<Aabb>,
    pub(super) grid: Grid<usize>,
    /// `(ItemIndex id, ent id)` per committed item; the handle is the position.
    pub(super) handles: Vec<(usize, usize)>,
    /// Query distance of the repair (largest clearance + width).
    pub(super) qgrow: f64,
    /// Number of static objects (they come first in `ents`).
    pub(super) n_static: usize,
    /// The wire of each drawn (dynamic) object that has one, by ent id ([`Drawer::pair_disc_clear`]).
    ent_wire: BTreeMap<usize, usize>,
    /// Diagnosis: ignore the copper drawn so far.
    pub(super) only_static: std::sync::atomic::AtomicBool,
    /// Trial drawing (via reduction): a rejected run is not diagnosed or recorded.
    pub quiet: bool,
    /// Per-stretch widths (the drawing witness); `None`: every wire has its own single width.
    pub widths: Option<Widths<'a>>,
    /// D0 experiment, off by default (the drawing witness sets it with `--tight`): the funnel of
    /// a pass may use the whole span its packing allows (`lmin ..= rmax`), narrowed by the
    /// crossings of the neighbours drawn so far, instead of the S2 interval around an evenly
    /// spread nominal point. Off: the S2 intervals (a neighbour keeps half the spare room).
    pub tight: bool,
    /// Layer of every pass as far as the plans fix it before the vias are placed (`-1` where a
    /// via interval leaves it open or the wire is not drawn); exact for a wire once its vias are
    /// placed. Empty: no via side test (see [`Drawer::via_side_ok`]).
    pub prov_layer: Vec<i32>,
    /// Round j: the layers of every wire's plan (`Plan::layers`, set with `prov_layer`; empty = not
    /// known): a passless wire's reference path in the class test ([`ref_trav_layers`]).
    pub plan_layers: Vec<Vec<usize>>,
    /// Every via placed: `(wire, traversal, centre, layer before, layer after, handle)`; it
    /// counts while its copper is committed (see [`Drawer::via_walls`]).
    via_marks: Vec<(WireId, usize, Vec2, usize, usize, usize)>,
    /// Wires drawn at least once (their copper, not a via wall, stands for their chords).
    pub(super) drawn_once: Vec<bool>,
    /// Per pass of a fitting edge-layer: `(edge, layer, index in the lane, lmin, rmax)` (nm).
    slot: Vec<Option<Slot>>,
    /// Per edge-layer that fits: its passes in edge order (see [`Lane`]).
    lanes: BTreeMap<(EdgeId, usize), Lane>,
    /// Drawn crossing of every pass: distance along its edge (nm) and the handle of the first
    /// track of its run (the crossing counts while that copper is committed).
    drawn_x: Vec<Option<(f64, usize)>>,
    /// Where a branch's end (wire, end 0|1) lies on its trunk's drawn copper (D5 net trees): the
    /// drawing ends the branch there instead of at its junction vertex.
    pub end_at: BTreeMap<(WireId, usize), Vec2>,
    /// edge-clear: the pad end of a run that cannot end at its pad centre (the track end breaks a
    /// static clearance there) ends at this point inside the pad ([`super::padend`]); set once
    /// before ERBS and drawing ([`Drawer::set_pad_ends`]). `end_at` wins over it.
    pub pad_end: BTreeMap<(WireId, usize), Vec2>,
    /// 5.4 `[vias] relocate`: the site the plan decided for via `i` of wire `w`, `(traversal,
    /// point)` ([`super::viaopt::relocate_vias`]); [`Drawer::place_vias`] takes it when it is
    /// legal there, else searches as before. Empty: no planned sites.
    pub via_site: BTreeMap<(WireId, usize), (usize, Vec2)>,
    /// The planned sites whose relocation still predicts a violation ([`super::viaopt::RelocReport::tight`]):
    /// they pass only the side test on the drawing's nominal chords, not the one on the plan's taut geometry.
    pub via_site_tight: std::collections::BTreeSet<(WireId, usize)>,
    /// thick-layer M3 slack ([`super::viaopt::ViaCtx::site_slack`], 0 = off): the planned sites
    /// were judged with the crossers' slack, so the room a via pair keeps
    /// ([`Drawer::via_pair_price`]) counts the crossers whose planned (taut) track runs between
    /// the two vias, not every crosser of their traversals (one beyond the via in the same
    /// traversal is no crosser between them).
    pub site_slack: f64,
    /// exist-term r4 (`[search] dangling_lead_via`): wire `w`'s end (0 / 1) whose layer change the
    /// plan put at a continued dangling end's lead end: the via drawn at the lead's via end
    /// ([`super::board::KeptEndOut::via_end`]), the lead drawn up to it, no run on the lead's layer
    /// ([`Drawer::lead_via_site`]).
    pub lead_via: BTreeMap<(WireId, usize), Vec2>,
    /// 5.4 `[search] branch_relocate`: a junction point the plan moved, per wire and traversal:
    /// the wire's run is split there (drawn through it), like at a via without a layer change
    /// ([`super::viaopt::relocate_junctions`]). Empty: none.
    pub split_at: BTreeMap<WireId, Vec<(usize, Vec2)>>,
    /// M3 `[drawing] realise = "erbs"`: the ERBS of every wire's runs (`topo::erbs::realise`),
    /// tried before the attempt ladder.
    pub erbs: BTreeMap<WireId, Vec<super::erbs::RunBand>>,
    /// Test-only (hook "no sequential run"): every sequential attempt (0..5) of a run with an ERBS
    /// is illegal, so a parked ERBS reaches its last step (`ERBS_KEPT`). Always false in real runs.
    pub test_no_sequential: bool,
    /// Test-only (hook "erbs class keep off"): the round-i class test of the length factor's
    /// competitor ([`competitor_keeps_class`]) is skipped. Always false in real runs.
    pub test_no_class_keep: bool,
    /// Test-only (hook "straight sequential run"): a run with an ERBS gets the straight segment
    /// between its ends as its first sequential attempt (a competitor of another class, as the
    /// exact window search gave qmtech ADV_CLK). Always false in real runs.
    pub test_straight_seq: bool,
    /// Test-only (hook "fail via wire"): the drawing of a wire with more than one run fails
    /// before its first run, so its reserved bands are still in the index (TW2:
    /// [`Drawer::draw_wire`] releases them). Always false in real runs.
    pub test_fail_via_wire: bool,
    /// Test-only (hook "joint conflict"): the joint check sees a conflict between every two bands
    /// next in the draw order ([`Drawer::reserve_bands`]), so bands it rejects reach the octilinear
    /// rescue against the kept ones. Always false in real runs.
    pub test_joint_conflict: bool,
    /// Round i: the class test's reference paths, built at the first competitor.
    class_ref: Option<ClassRef>,
    /// M3 length factor: every legal ERBS a sequential run was drawn against (wire, layer, ERBS
    /// length, band length, sequential length or NaN when none was legal, replaced), for the
    /// `erbs_long` warning (core.rs).
    pub erbs_long: Vec<(WireId, usize, f64, f64, f64, bool)>,
    /// M-B1 never drop: a run that no attempt draws legally, or a via without a legal site, is
    /// placed anyway on the attempt / site with the least `violation` price, and its violations
    /// are recorded ([`WireOut::viol`], [`Drawer::via_viol`]). Off: such a wire fails.
    pub force: bool,
    /// Violations of the vias placed by force, per wire.
    pub via_viol: BTreeMap<WireId, Vec<Viol>>,
    /// TW2 joint band commit (`realise = "erbs"`, [`super::joint`]): reserved bands and their counts.
    pub joint: super::joint::JointState,
    /// Octilinear drawing (M-45, [`super::octi`]): style switch and prices.
    pub octi: super::octi::OctiCfg,
}

impl<'a> Drawer<'a> {
    pub fn new(model: &'a BoardModel, opts: &'a Options, b: &'a Board, st: &'a State<'a>, checker: &'a Checker) -> Self {
        let packer = Packer::for_model(model, opts, &b.proj);
        let (eps, edge_clear, outline) = (packer.eps(), packer.edge_clear(), super::board::outline_segments(model));
        let dirs = packer.taut_dirs(st);
        if let Ok(v) = std::env::var("TOPO_TAUTDBG") {
            // Debug: the taut path of the listed wires (crossing point, direction, edge, infl).
            for w in v.split(',').filter_map(|x| x.parse::<usize>().ok()).filter(|&w| w < st.wires.len() && st.wires[w].alive) {
                let t = packer.taut(st, w);
                for (k, &pid) in st.wires[w].passes.iter().enumerate() {
                    let e = st.passes[pid].edge;
                    eprintln!(
                        "TAUT wire {w} k {k} e{e} at ({:.3},{:.3}) dir ({:.3},{:.3}) infl {:.3}",
                        t.at[k].x / 1e6,
                        -t.at[k].y / 1e6,
                        t.dir[k].x / 1e6,
                        -t.dir[k].y / 1e6,
                        infl_dir(&b.proj, e, t.dir[k])
                    );
                }
            }
        }
        Drawer {
            pair_keep: BTreeMap::new(),
            model,
            opts,
            b,
            st,
            checker,
            idx: ItemIndex::new(checker.cell()),
            packer,
            outline,
            eps,
            edge_clear,
            fails: Vec::new(),
            pass_k: {
                let mut v = vec![0; st.passes.len()];
                for w in st.wires.iter().filter(|w| w.alive) {
                    for (k, &p) in w.passes.iter().enumerate() {
                        v[p] = k;
                    }
                }
                v
            },
            polys: (0..st.wires.len()).map(|w| if st.wires[w].alive { st.polyline(w) } else { Vec::new() }).collect(),
            dirs,
            iv: Intervals::default(),
            pass_layer: vec![-1; st.passes.len()],
            pass_disc: vec![[0.0; 3]; st.passes.len()],
            over: vec![false; st.proj.edges.len() * st.proj.n_layers],
            spare: vec![f64::INFINITY; st.proj.edges.len() * st.proj.n_layers],
            diag: Vec::new(),
            stats: DrawStats::default(),
            ents: Vec::new(),
            ent_box: Vec::new(),
            grid: Grid::new(2.0e6),
            handles: Vec::new(),
            qgrow: 0.0,
            n_static: 0,
            ent_wire: BTreeMap::new(),
            only_static: std::sync::atomic::AtomicBool::new(false),
            quiet: false,
            widths: None,
            tight: false,
            prov_layer: Vec::new(),
            plan_layers: Vec::new(),
            via_marks: Vec::new(),
            drawn_once: vec![false; st.wires.len()],
            slot: vec![None; st.passes.len()],
            lanes: BTreeMap::new(),
            drawn_x: vec![None; st.passes.len()],
            end_at: BTreeMap::new(),
            pad_end: BTreeMap::new(),
            via_site: BTreeMap::new(),
            via_site_tight: Default::default(),
            site_slack: 0.0,
            lead_via: BTreeMap::new(),
            split_at: BTreeMap::new(),
            erbs: BTreeMap::new(),
            test_no_sequential: false,
            test_no_class_keep: false,
            test_straight_seq: false,
            test_fail_via_wire: false,
            test_joint_conflict: false,
            class_ref: None,
            erbs_long: Vec::new(),
            force: false,
            via_viol: BTreeMap::new(),
            joint: super::joint::JointState::default(),
            octi: super::octi::OctiCfg::from_opts(opts),
        }
        .with_static()
    }

    /// The clearance of the last custom clearance region (`Rules::clearance_regions`, rule order)
    /// copper object `e` matches on one of its layers: it touches the polygon (or, `enclosed`, lies
    /// wholly inside), so every pair with it needs that clearance (KiCad: the matching custom rule
    /// wins over both nets' clearances). None without regions. O(regions x polygon edges).
    fn region_clear(&self, e: &Ent) -> Option<f64> {
        let regions = &self.model.rules.clearance_regions;
        if regions.is_empty() || e.fixed {
            return None;
        }
        let pts: Vec<Vec2> = match &e.core {
            Core::Poly(p) => p.clone(),
            Core::Seg(a, b) => vec![*a, *b],
            Core::Pt(p) => vec![*p],
        };
        let mut out = None;
        for r in regions {
            if r.poly.len() < 3 {
                continue;
            }
            if !r.layers.is_empty() {
                let m = self.b.signal_layers.iter().enumerate().filter(|(_, id)| r.layers.contains(id)).fold(0u64, |m, (i, _)| m | 1 << i);
                if m & e.mask == 0 {
                    continue;
                }
            }
            let poly: Vec<Vec2> = r.poly.iter().map(|p| p.to_vec2()).collect();
            let n = poly.len();
            let touches = (0..n).any(|i| seg_core(poly[i], poly[(i + 1) % n], &e.core).0 <= e.grow);
            let inside = pts.first().is_some_and(|&p| inside_poly(p, &poly));
            if if r.enclosed { !touches && inside } else { touches || inside } {
                out = Some(r.clearance as f64);
            }
        }
        out
    }

    fn add_ent(&mut self, mut e: Ent) -> usize {
        if let Some(c) = self.region_clear(&e) {
            // the region's clearance replaces both the object's and the wire's own
            e.clear = c;
            e.fixed = true;
        }
        let core_box = match &e.core {
            Core::Poly(p) => bbox_of(p),
            Core::Seg(a, b) => bbox_of(&[*a, *b]),
            Core::Pt(p) => bbox_of(&[*p]),
        };
        let id = self.ents.len();
        self.grid.insert(core_box.grow(e.grow + e.clear + 1.0), id);
        self.ents.push(e);
        self.ent_box.push(core_box);
        id
    }

    /// The static objects of the board for the repair: pads and holes of the projection, the
    /// board outline, foreign copper shapes and keepouts that forbid tracks.
    fn with_static(mut self) -> Self {
        let (b, model) = (self.b, self.model);
        let nl = b.signal_layers.len();
        let lmask = |ids: &[u32]| -> u64 {
            b.signal_layers.iter().enumerate().filter(|(_, id)| ids.contains(id)).fold(0, |m, (i, _)| m | 1 << i)
        };
        let all = if nl >= 64 { u64::MAX } else { (1u64 << nl) - 1 };
        // Entity `o` is obstacle `o` for every obstacle (`i < n_obs` tests, `is_kept_end`,
        // `is_exit_stub`): the obstacles are the first entities, one each.
        assert!(self.ents.is_empty(), "static entities come first");
        for (o, ob) in b.proj.obstacles.iter().enumerate() {
            let mask = if ob.drilled { all } else { ob.layers.0 };
            // exist-term: a kept track's polygon is a dead entity (ids stay aligned with the
            // obstacles); its exact segments are added below.
            if b.kept_bodies.contains(&o) {
                self.add_ent(Ent { core: Core::Poly(ob.poly.clone()), grow: 0.0, mask, net: ob.net, clear: ob.clear, fixed: true, live: false, shape: false });
                continue;
            }
            // The polygon of a rounded shape lies inside the copper by its sagitta (`sag`; 0 for a
            // rectangle). It was 2 % of the circumradius for every shape, 57 um phantom copper
            // around a 4 x 4 mm test pad (wiccon TP13: the 3.3v wire then found no path past
            // it, D1).
            self.add_ent(Ent { core: Core::Poly(ob.poly.clone()), grow: ob.sag, mask, net: ob.net, clear: ob.clear, fixed: ob.bare, live: true, shape: false });
        }
        assert_eq!(self.ents.len(), b.proj.obstacles.len(), "one entity per obstacle");
        for (o, &pi) in b.pad_of.iter().enumerate().filter(|(_, &pi)| pi != usize::MAX) {
            let pad = &model.pads[pi];
            if let Some(d) = pad.drill {
                let ob = &b.proj.obstacles[o];
                self.add_ent(Ent {
                    core: Core::Pt(ob.center),
                    grow: 0.5 * d.w.min(d.h) as f64,
                    mask: all,
                    net: ob.net,
                    clear: model.rules.hole_clearance as f64,
                    fixed: true,
                    live: true,
                    shape: false,
                });
            }
        }
        for &(a, c) in &self.outline.clone() {
            self.add_ent(Ent { core: Core::Seg(a, c), grow: 0.0, mask: all, net: None, clear: self.edge_clear, fixed: true, live: true, shape: false });
        }
        for sh in &model.copper_shapes {
            let m = lmask(&[sh.layer]);
            if m != 0 && sh.poly.len() >= 3 {
                let clear = model.rules.net_rule(sh.net).clearance as f64;
                self.add_ent(Ent { core: Core::Poly(sh.poly.iter().map(|p| p.to_vec2()).collect()), grow: 0.0, mask: m, net: (sh.net > 0).then_some(sh.net), clear, fixed: false, live: true, shape: true });
            }
        }
        for k in &model.keepouts {
            if k.no_tracks && k.poly.len() >= 3 {
                let m = if k.layers.is_empty() { all } else { lmask(&k.layers) };
                self.add_ent(Ent { core: Core::Poly(k.poly.iter().map(|p| p.to_vec2()).collect()), grow: 0.0, mask: m, net: None, clear: 0.0, fixed: true, live: true, shape: false });
            }
        }
        // Existing copper the board keeps: vias and tracks (arcs as chords).
        for v in &model.vias {
            let m = if v.layers.is_empty() { all } else { lmask(&v.layers) | if v.layers.contains(&0) && v.layers.contains(&2) { all } else { 0 } };
            let clear = via_clear(model, v.net, v.d, v.drill);
            self.add_ent(Ent { core: Core::Pt(v.at.to_vec2()), grow: 0.5 * v.d as f64, mask: m, net: (v.net > 0).then_some(v.net), clear, fixed: false, live: true, shape: false });
        }
        for k in &model.kept {
            let m = lmask(&[k.layer]);
            if m == 0 {
                continue;
            }
            let clear = model.rules.net_rule(k.net).clearance as f64;
            let segs: Vec<(Vec2, Vec2)> = match k.prim {
                TrackPrim::Seg { a, b } => vec![(a.to_f64(), b.to_f64())],
                TrackPrim::Arc { start, mid, end } => match crate::geom::Arc::from_three_points(start.to_f64(), mid.to_f64(), end.to_f64()) {
                    Some(arc) => arc.split_sagitta(0.02e6).iter().map(|sg| (sg.start(), sg.end())).collect(),
                    None => vec![(start.to_f64(), end.to_f64())],
                },
            };
            for (a, c) in segs {
                self.add_ent(Ent { core: Core::Seg(a, c), grow: 0.5 * k.width as f64, mask: m, net: (k.net > 0).then_some(k.net), clear, fixed: false, live: true, shape: false });
            }
        }
        let r = &model.rules;
        let maxclr = r.per_net.values().map(|n| n.clearance).fold(r.default.clearance.max(r.min_clearance).max(r.hole_clearance).max(r.edge_clear()), i64::max);
        let maxw = r.per_net.values().map(|n| n.width).fold(r.default.width, i64::max);
        self.qgrow = (maxclr + maxw) as f64 + 4.0 * self.eps;
        self.n_static = self.ents.len();
        self
    }

    /// Index copper accepted for `net` as an object the next wires keep away from.
    fn add_dynamic(&mut self, item: &Item) -> usize {
        let wire = match item {
            Item::Track(t) => t.wire,
            Item::Via(v) => v.wire,
        };
        let id = self.add_dynamic_ent(item);
        if let Some(w) = wire.filter(|_| id != usize::MAX) {
            self.ent_wire.insert(id, w);
        }
        id
    }

    fn add_dynamic_ent(&mut self, item: &Item) -> usize {
        let b = self.b;
        match item {
            Item::Track(t) => {
                let TrackPrim::Seg { a, b: e } = t.prim else { return usize::MAX };
                let li = b.signal_layers.iter().position(|&l| l == t.layer).unwrap_or(0);
                let clear = self.model.rules.net_rule(t.net).clearance as f64;
                self.add_ent(Ent {
                    core: Core::Seg(Vec2 { x: f64::from(a.x), y: -f64::from(a.y) }, Vec2 { x: f64::from(e.x), y: -f64::from(e.y) }),
                    grow: 0.5 * t.width as f64,
                    mask: 1 << li,
                    net: Some(t.net),
                    clear,
                    fixed: false,
                    live: true,
                    shape: false,
                })
            }
            Item::Via(v) => {
                let nl = b.signal_layers.len();
                let clear = via_clear(self.model, v.net, v.d, v.drill);
                self.add_ent(Ent {
                    core: Core::Pt(Vec2 { x: v.at.x as f64, y: -(v.at.y as f64) }),
                    grow: 0.5 * v.d as f64,
                    mask: if nl >= 64 { u64::MAX } else { (1u64 << nl) - 1 },
                    net: Some(v.net),
                    clear,
                    fixed: false,
                    live: true,
                    shape: false,
                })
            }
        }
    }

    pub(super) fn layer_id(&self, l: usize) -> u32 {
        self.b.signal_layers[l]
    }

    /// Pack the passes of every edge per layer (see the module doc) into [`Drawer::iv`]. O(passes).
    pub fn compute_intervals(&mut self) {
        let pr = &self.b.proj;
        let np = self.st.passes.len();
        self.iv = Intervals { lo: vec![f64::NAN; np], hi: vec![f64::NAN; np], nom: vec![f64::NAN; np] };
        self.lanes.clear();
        for e in 0..pr.edges.len() {
            if self.st.edge_order[e].is_empty() {
                continue;
            }
            for l in 0..pr.n_layers {
                self.pack(e, l);
            }
        }
    }

    /// Set the pass layers and via discs of wire `w` for its `plan` with vias at the traversals
    /// `ks` of diameters `ds`, at the points `pts` (one per via, `None`: not known). TW6 part A
    /// (lead 2026-11-19): with `[capacity] via_disc_height` the discs get the height of their via
    /// point off the edge line, as the plan packs them ([`super::refine::wire_heights_into`]), so
    /// the drawing reads the room the plan read (tw4-r2 (2b) packed the drawing at height 0: plan
    /// errors fell, drawn losses rose). O(passes of the wire).
    pub fn set_wire_layers(&mut self, w: WireId, plan: &Plan, ks: &[usize], ds: &[f64], pts: &[super::refine::ViaPt]) {
        let n = self.st.wires[w].passes.len();
        for r in 0..plan.layers.len() {
            let k0 = if r == 0 { 0 } else { ks[r - 1] };
            let k1 = if r == plan.layers.len() - 1 { n } else { ks[r] };
            for k in k0.min(n)..k1.min(n) {
                self.pass_layer[self.st.wires[w].passes[k]] = plan.layers[r] as i32;
            }
        }
        let mut disc = wire_discs(self.st, w, ks, ds);
        if self.packer.via_disc_h && pts.len() == ks.len() {
            let (st, ps) = (self.st, &self.st.wires[w].passes);
            super::refine::wire_heights_into(&mut disc, &self.b.proj, |j| st.passes[ps[j]].edge, ks, pts);
        }
        for (k, &p) in self.st.wires[w].passes.iter().enumerate() {
            self.pass_disc[p] = disc[k];
        }
    }

    /// Pack edge `e` on layer `l` (see [`Drawer::compute_intervals`]); no-op without items.
    fn pack(&mut self, e: EdgeId, l: usize) {
        let [p, q] = self.b.proj.edges[e];
        let len = (self.b.proj.pos[q] - self.b.proj.pos[p]).len().max(1.0);
        let (ids, pitems) = layer_items(
            &self.st.edge_order[e],
            l,
            |pid| self.pass_layer[pid],
            |pid| self.pass_disc[pid],
            |pid| {
                let wi = &self.st.wires[self.st.passes[pid].wire];
                let (w, k) = (self.st.passes[pid].wire, self.pass_k[pid]);
                // The plan's width of the pass (M-C: necked where P2 necked it).
                let pw = self.widths.as_ref().map_or(self.st.pass_w.get(pid).copied().unwrap_or(wi.w), |x| x.pass_w[pid]);
                PackItem { net: wi.net, w: pw, s: wi.s, infl: infl_dir(&self.b.proj, e, self.dirs[w][k]), disc: [0.0; 3], lean: 0.0 }
            },
        );
        if ids.is_empty() {
            self.lanes.remove(&(e, l));
            self.over[e * self.b.proj.n_layers + l] = false;
            self.spare[e * self.b.proj.n_layers + l] = f64::INFINITY;
            return;
        }
        let items: Vec<PassId> = ids.iter().map(|x| x.0).collect();
        let only_disc: Vec<bool> = ids.iter().map(|x| x.1).collect();
        let n = items.len();
        let bnd = self.packer.bounds(e, l, &pitems);
        let (lo, hi, lmin, rmax) = (&bnd.lo, &bnd.hi, &bnd.lmin, &bnd.rmax);
        let req = |i: usize| bnd.req[i];
        let fits = bnd.fits();
        self.over[e * self.b.proj.n_layers + l] = !fits;
        self.spare[e * self.b.proj.n_layers + l] = -bnd.worst();
        // A disc-only item (the via of a wire on another layer) holds room here; the pass's
        // slot and interval belong to its own layer.
        for (i, &pid) in items.iter().enumerate() {
            if !only_disc[i] {
                self.slot[pid] = None;
            }
        }
        if fits {
            let mut cum = vec![0.0; n];
            for i in 1..n {
                cum[i] = cum[i - 1] + req(i - 1);
            }
            for (i, &pid) in items.iter().enumerate() {
                if !only_disc[i] {
                    self.slot[pid] = Some((e, l, i, lmin[i], rmax[i]));
                }
            }
            self.lanes.insert((e, l), Lane { pids: items.to_vec(), cum });
        } else {
            self.lanes.remove(&(e, l));
        }
        let mut nom = vec![0.0; n];
        let (mut a, mut b) = (lmin.clone(), rmax.clone());
        if fits {
            for i in 0..n {
                let target = lmin[i] + (rmax[i] - lmin[i]) * (i + 1) as f64 / (n + 1) as f64;
                let floor = if i == 0 { lmin[0] } else { lmin[i].max(nom[i - 1] + req(i - 1)) };
                nom[i] = target.clamp(floor, rmax[i].max(floor));
            }
            for i in 0..n {
                a[i] = if i == 0 { lo[0].min(nom[0]) } else { nom[i] - 0.5 * (nom[i] - nom[i - 1] - req(i - 1)).max(0.0) };
                b[i] = if i == n - 1 { hi[i].max(nom[i]) } else { nom[i] + 0.5 * (nom[i + 1] - nom[i] - req(i)).max(0.0) };
                a[i] = a[i].min(nom[i]);
                b[i] = b[i].max(nom[i]);
            }
        } else {
            // Over capacity: spread evenly over the widest span; the checker will tell.
            let (l0, h0) = (lo.iter().cloned().fold(f64::INFINITY, f64::min), hi.iter().cloned().fold(f64::NEG_INFINITY, f64::max));
            let (l0, h0) = if h0 > l0 { (l0, h0) } else { (0.0, len) };
            for i in 0..n {
                nom[i] = l0 + (h0 - l0) * (i + 1) as f64 / (n + 1) as f64;
                a[i] = nom[i];
                b[i] = nom[i];
            }
        }
        for (i, &pid) in items.iter().enumerate() {
            if only_disc[i] {
                continue;
            }
            self.iv.lo[pid] = (a[i] / len).clamp(0.0, 1.0);
            self.iv.hi[pid] = (b[i] / len).clamp(0.0, 1.0);
            self.iv.nom[pid] = (nom[i] / len).clamp(0.0, 1.0);
        }
    }

    /// The drawn crossing of pass `pid` (nm along its edge) while its copper is committed.
    pub(super) fn live_x(&self, pid: PassId) -> Option<f64> {
        let (x, h) = self.drawn_x[pid]?;
        let (_, ent) = self.handles[h];
        self.ents.get(ent).is_some_and(|en| en.live).then_some(x)
    }

    /// Span (nm along the edge) pass `pid` may use under tight packing: `lmin ..= rmax` of its
    /// lane, narrowed by every drawn neighbour plus the required spacing of the passes between.
    /// `None` without a fitting lane. O(passes of the lane).
    pub fn tight_span(&self, pid: PassId) -> Option<(f64, f64)> {
        let (e, l, i, lmin, rmax) = self.slot[pid]?;
        let Lane { pids, cum, .. } = self.lanes.get(&(e, l))?;
        let (mut lo, mut hi) = (lmin, rmax);
        for (j, &q) in pids.iter().enumerate() {
            if j == i {
                continue;
            }
            if let Some(x) = self.live_x(q) {
                if j < i {
                    lo = lo.max(x + cum[i] - cum[j]);
                } else {
                    hi = hi.min(x - (cum[j] - cum[i]));
                }
            }
        }
        if lo > hi {
            // The drawn neighbours left less than the spacing: the middle; the checker decides.
            let m = 0.5 * (lo + hi);
            return Some((m, m));
        }
        Some((lo, hi))
    }

    /// Whether pass `pid` has another pass of its edge and layer before it / after it in its
    /// packed lane (both `true` when it is not packed in a fitting lane).
    pub(super) fn lane_sides(&self, pid: PassId) -> (bool, bool) {
        match self.slot[pid].and_then(|(e, l, i, _, _)| self.lanes.get(&(e, l)).map(|ln| (i, ln.pids.len()))) {
            Some((i, n)) => (i > 0, i + 1 < n),
            None => (true, true),
        }
    }

    /// Whether the pass next to `pid` in its packed lane before it / after it is drawn already
    /// (`false` without a fitting lane or without that neighbour). O(1).
    pub(super) fn lane_next_drawn(&self, pid: PassId) -> (bool, bool) {
        let Some((e, l, i, _, _)) = self.slot[pid] else { return (false, false) };
        let Some(ln) = self.lanes.get(&(e, l)) else { return (false, false) };
        let drawn = |j: usize| ln.pids.get(j).is_some_and(|&q| self.live_x(q).is_some());
        (i > 0 && drawn(i - 1), drawn(i + 1))
    }

    pub(super) fn lane_dbg(&self, pid: PassId) -> String {
        let Some((e, l, i, lmin, rmax)) = self.slot[pid] else { return "none".into() };
        let Some(ln) = self.lanes.get(&(e, l)) else { return "nolane".into() };
        let v: Vec<String> = ln.pids.iter().enumerate().map(|(j, &q)| format!("{}{}w{}n{}@{:.3}{}", if j == i { "*" } else { "" }, j, self.st.passes[q].wire, self.st.wires[self.st.passes[q].wire].net, ln.cum[j] / 1e6, self.live_x(q).map(|x| format!("D{:.3}", x / 1e6)).unwrap_or_default())).collect();
        format!("l{l} [{:.3},{:.3}] {}", lmin / 1e6, rmax / 1e6, v.join(" "))
    }

    /// The bound (nm along the edge from its first end) the plan keeps for the neighbour of
    /// `pid` in its packed lane, before it (`after` false) or after it: the neighbour's planned
    /// crossing (its taut path, [`Packer::taut`]) less the spacing the lane needs between the two.
    /// `None` without a fitting lane or without that neighbour. O(lane) plus the cached taut.
    ///
    /// With `[drawing] realise = "erbs"` the neighbour's planned crossing is where its ERBS on
    /// this layer crosses the edge (the taut point picks among several): the ERBS is the plan's
    /// realisation, spaced by the radii and spoke tips (Dayan 1997 PDF pp. 15-16, 32). The taut
    /// path has zero width, so round a vertex several wires wrap it lies on the vertex itself,
    /// and the bound, its crossing less the lane spacing, fell behind the vertex: the whole
    /// span of the inner wire was its neighbour's room (nested_wraps B, drawn after C and before
    /// A: 22 mm of `planned_room` price on its own ERBS, the octilinear form jogged out round
    /// A's pad to shorten the priced leg, 1.6 % longer).
    pub(super) fn neighbour_bound(&self, pid: PassId, after: bool) -> Option<f64> {
        let (e, l, i, _, _) = self.slot[pid]?;
        let ln = self.lanes.get(&(e, l))?;
        let j = if after { i + 1 } else { i.checked_sub(1)? };
        let x = self.neighbour_x(pid, after)?;
        let gap = (ln.cum[i.max(j)] - ln.cum[i.min(j)]).abs();
        Some(if after { x - gap } else { x + gap })
    }

    /// The planned crossing (nm along the edge) of the lane neighbour of [`Drawer::neighbour_bound`]
    /// itself, without the spacing: where that wire's ERBS (else its taut path) crosses the edge.
    pub(super) fn neighbour_x(&self, pid: PassId, after: bool) -> Option<f64> {
        let (e, l, i, _, _) = self.slot[pid]?;
        let ln = self.lanes.get(&(e, l))?;
        let j = if after { i + 1 } else { i.checked_sub(1)? };
        let q = *ln.pids.get(j)?;
        let wq = self.st.passes[q].wire;
        let at = *self.packer.taut(self.st, wq).at.get(self.pass_k[q])?;
        let [p, r] = self.b.proj.edges[e];
        let (pp, pr) = (self.b.proj.pos[p], self.b.proj.pos[r]);
        let len = (pr - pp).len();
        if len < 1.0 {
            return None;
        }
        let taut_x = (at - pp).dot((pr - pp) * (1.0 / len));
        let erbs_x = (self.opts.realise == "erbs").then(|| self.erbs.get(&wq)).flatten().and_then(|v| {
            v.iter().filter(|b| b.layer == l).filter_map(|b| crossing(&b.path, pp, pr, taut_x)).min_by(|a, b| (a - taut_x).abs().total_cmp(&(b - taut_x).abs()))
        });
        Some(erbs_x.unwrap_or(taut_x))
    }

    /// Remember where the accepted run `pts` (passes `k0..k1` of `w`, first track handle `h`)
    /// crosses its pass edges. O(passes x segments).
    fn record_crossings(&mut self, w: WireId, k0: usize, k1: usize, pts: &[Vec2], h: usize) {
        let pr = &self.b.proj;
        for k in k0..k1 {
            let pid = self.st.wires[w].passes[k];
            let [p, q] = pr.edges[self.st.passes[pid].edge];
            let (pp, pq) = (pr.pos[p], pr.pos[q]);
            let len = (pq - pp).len().max(1.0);
            let near = self.iv.nom[pid] * len;
            self.drawn_x[pid] = crossing(pts, pp, pq, if near.is_finite() { near } else { 0.5 * len }).map(|x| (x, h));
        }
    }

    /// Left and right end of the portal of pass index `k` of wire `w` for the fractions
    /// `lo..hi` along the edge, seen along the travel direction.
    fn portal(&self, w: WireId, k: usize, lo: f64, hi: f64) -> (Vec2, Vec2) {
        let (st, pr) = (self.st, &self.b.proj);
        let wi = &st.wires[w];
        let e = st.passes[wi.passes[k]].edge;
        let t: TriId = wi.tris[k];
        let [p, q] = pr.edges[e];
        let (pp, pq) = (pr.pos[p], pr.pos[q]);
        let pt = |f: f64| super::funnel::edge_point(pp, pq, f);
        let a = pr.tris[t][pr.side(t, e)];
        if a == p {
            (pt(hi), pt(lo))
        } else {
            (pt(lo), pt(hi))
        }
    }

    /// Candidate via sites of a via interval, best first: `(traversal, point)`. The planned
    /// traversal `at` (P3/P4) goes first, then the triangles no other wire traverses, wide
    /// enough, widest first.
    fn via_candidates(&self, w: WireId, ka: usize, kb: usize, need_r: f64, at: Option<usize>) -> Vec<(usize, Vec2)> {
        let (st, pr) = (self.st, &self.b.proj);
        let wi = &st.wires[w];
        let mut c: Vec<(bool, usize, bool, f64, usize, Vec2)> = Vec::new();
        for k in ka..=kb {
            let t = wi.tris[k];
            if pr.tri_out[t] || !pr.tri_obs[t].is_empty() {
                continue;
            }
            let others = st.tri_trav[t].iter().filter(|&&(o, _)| o != w).count();
            let [a, b, cc] = pr.tris[t].map(|v| pr.pos[v]);
            let inr = pr.inradius(t);
            for p in super::pack::tri_site_points(a, b, cc) {
                c.push((Some(k) != at, others, inr < need_r, -inr, k, p));
            }
        }
        c.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.cmp(&y.1)).then(x.2.cmp(&y.2)).then(x.3.total_cmp(&y.3)).then(x.4.cmp(&y.4)));
        c.into_iter().map(|x| (x.4, x.5)).collect()
    }

    /// Via sites on the nominal chord of wire `w` in traversal `k` (from its packed crossing of
    /// the edge it enters by to the one it leaves by, or its terminal; turn corners included),
    /// its two ends first: where the packing gave the via its room (the disc items of
    /// [`layer_items`]). Empty when the passes have no packed position yet. O(turns).
    fn chord_sites(&self, w: WireId, k: usize) -> Vec<(usize, Vec2)> {
        let (st, pr) = (self.st, &self.b.proj);
        let wi = &st.wires[w];
        let n = wi.passes.len();
        let nom = |j: usize| self.iv.nom.get(wi.passes[j]).copied().filter(|f| f.is_finite());
        let entry = if k == 0 {
            Some(pr.pos[wi.a])
        } else {
            nom(k - 1).map(|f| self.portal(w, k - 1, f, f).0)
        };
        let exit = if k >= n { Some(pr.pos[wi.b]) } else { nom(k).map(|f| self.portal(w, k, f, f).0) };
        let (Some(a), Some(b)) = (entry, exit) else { return Vec::new() };
        let mut pl = vec![a];
        pl.extend(wi.turns.iter().filter(|x| x.0 == k).map(|x| pr.pos[x.1]));
        pl.push(b);
        let total: f64 = pl.windows(2).map(|x| (x[1] - x[0]).len()).sum();
        if total < 1.0 {
            return vec![(k, a)];
        }
        let at = |f: f64| {
            let mut d = f * total;
            for x in pl.windows(2) {
                let l = (x[1] - x[0]).len();
                if d <= l {
                    return x[0] + (x[1] - x[0]) * (d / l.max(1e-9));
                }
                d -= l;
            }
            *pl.last().expect("chord")
        };
        // A junction of the wire in this traversal first (D6b): a via there is the tee itself
        // (its arms meet on both layers, Polubasov 2001 §5 p. 9: the via leaves the branch point
        // for an arm only when the point cannot hold it). Then the packed crossings themselves:
        // there the packing kept the disc off the edge's end obstacles and its neighbours on that
        // edge.
        let turns = wi.turns.iter().filter(|x| x.0 == k).map(|x| (k, pr.pos[x.1]));
        turns.chain([1.0, 0.0, 0.5, 0.75, 0.25, 0.9, 0.1].iter().map(|&f| (k, at(f)))).collect()
    }

    /// Least slack (nm) the packing leaves on the passage-layers next to a via of wire `w` in
    /// traversal `k` (the edges of its passes `k - 1` and `k`, every layer); infinite when none.
    pub fn via_slack(&self, w: WireId, k: usize) -> f64 {
        let ps = &self.st.wires[w].passes;
        let nl = self.b.proj.n_layers;
        [k.checked_sub(1), Some(k)]
            .into_iter()
            .flatten()
            .filter(|&j| j < ps.len())
            .flat_map(|j| {
                let e = self.st.passes[ps[j]].edge;
                (0..nl).map(move |l| e * nl + l)
            })
            .map(|i| self.spare[i])
            .fold(f64::INFINITY, f64::min)
    }

    /// The packed (nominal) chord of wire `o` in its traversal `k`: from its crossing of the
    /// edge it enters by (or its terminal) to the one it leaves by. `None` before packing.
    fn nom_chord(&self, o: WireId, k: usize) -> Option<(Vec2, Vec2)> {
        let (st, pr) = (self.st, &self.b.proj);
        let oi = &st.wires[o];
        let n = oi.passes.len();
        let nom = |j: usize| self.iv.nom.get(oi.passes[j]).copied().filter(|f| f.is_finite());
        let a = if k == 0 { Some(pr.pos[oi.a]) } else { nom(k - 1).map(|f| self.portal(o, k - 1, f, f).0) }?;
        let b = if k >= n { Some(pr.pos[oi.b]) } else { nom(k).map(|f| self.portal(o, k, f, f).0) }?;
        Some((a, b))
    }

    /// Free room (nm) around a via of diameter `d` at `p` in traversal `k` of wire `w`: the
    /// least gap beyond the clearance to the copper indexed so far of other nets (pads, vias,
    /// drawn tracks, any layer) and to the packed chords of the other nets' wires through that
    /// triangle. O(objects nearby + traversals of the triangle).
    pub fn via_room(&self, w: WireId, k: usize, p: Vec2, d: f64) -> f64 {
        let (st, pr) = (self.st, &self.b.proj);
        let wi = &st.wires[w];
        let own = self.model.rules.net_rule(wi.net).clearance as f64;
        let mut room = f64::INFINITY;
        let reach = 2.0e6 + d;
        let mut ids = self.grid.query(bbox_of(&[p]).grow(reach));
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            let e = &self.ents[id];
            if !e.live || e.net == Some(wi.net) {
                continue;
            }
            let dist = match &e.core {
                Core::Poly(q) => poly_dist(p, q),
                Core::Seg(a, b) => seg_dist(p, *a, *b),
                Core::Pt(q) => (p - *q).len(),
            };
            let need = if e.fixed { e.clear } else { e.clear.max(own) };
            room = room.min(dist - e.grow - 0.5 * d - need);
        }
        let t = wi.tris[k];
        for &(o, ok) in &st.tri_trav[t] {
            let oi = &st.wires[o];
            if o == w || !oi.alive || oi.net == wi.net {
                continue;
            }
            if let Some((a, b)) = self.nom_chord(o, ok) {
                room = room.min(seg_dist(p, a, b) - 0.5 * oi.w - 0.5 * d - oi.s.max(wi.s));
            }
        }
        let _ = pr;
        room
    }

    /// The plan's (taut) chord of wire `o` in its traversal `k`: its taut crossing of the edge it
    /// enters by (or its terminal), the turn corners there, its taut crossing of the edge it
    /// leaves by (or its terminal). O(turns) after the cached taut path.
    pub fn taut_chord(&self, o: WireId, k: usize) -> Vec<Vec2> {
        let (st, pr) = (self.st, &self.b.proj);
        let oi = &st.wires[o];
        let n = oi.passes.len();
        let t = self.packer.taut(st, o);
        let at = |j: usize| t.at.get(j).copied();
        let a = if k == 0 { Some(pr.pos[oi.a]) } else { at(k - 1) };
        let b = if k >= n { Some(pr.pos[oi.b]) } else { at(k) };
        let (Some(a), Some(b)) = (a, b) else { return Vec::new() };
        let mut pl = vec![a];
        pl.extend(oi.turns.iter().filter(|x| x.0 == k).map(|x| pr.pos[x.1]));
        pl.push(b);
        pl
    }

    /// Via sites on wire `w`'s own taut chord in traversal `k` (where the plan's path runs, so
    /// the crossings with the other wires keep the plan's order along it): its middle first,
    /// then towards both ends. O(turns).
    fn taut_sites(&self, w: WireId, k: usize) -> Vec<(usize, Vec2)> {
        let pl = self.taut_chord(w, k);
        let total: f64 = pl.windows(2).map(|x| (x[1] - x[0]).len()).sum();
        if pl.len() < 2 || total < 1.0 {
            return Vec::new();
        }
        let at = |f: f64| {
            let mut d = f * total;
            for x in pl.windows(2) {
                let l = (x[1] - x[0]).len();
                if d <= l {
                    return x[0] + (x[1] - x[0]) * (d / l.max(1e-9));
                }
                d -= l;
            }
            *pl.last().expect("chord")
        };
        [0.5, 0.375, 0.625, 0.25, 0.75, 0.125, 0.875].iter().map(|&f| (k, at(f))).collect()
    }

    /// P6: price of a via of wire `w` (reach diameter `d`, [`crate::vias::via_reach`] x 2: the
    /// disc or the drill's hole clearance, whichever reaches further) at `p` for the room the plan keeps for the
    /// other nets' wires not drawn yet through the triangles of its interval `k0..=k1`: per such
    /// wire, the depth by which the via disc plus the clearance reaches into the corridor of the
    /// wire's planned path (its taut chord and its packed nominal chord there; half its width
    /// plus the clearance either side), priced like the
    /// octilinear `planned_room` factor, `room x pitch x (x + x^2)` with `x = depth / pitch` and
    /// `pitch = width + clearance`: graded, no wall; 0 when the via keeps every planned corridor
    /// free. Earlier-drawn copper keeps the room the plan gives later wires (Dayan & Dai 1991,
    /// UCSC-CRL-91-40 §6 p. 6: for each net the other nets act as obstacles on its movable
    /// points, vias included §7; §8 p. 6: a non-linear force function bounds a quantity without
    /// a hard limit). O(traversals of the interval's triangles x turns).
    pub fn via_plan_price(&self, w: WireId, k0: usize, k1: usize, p: Vec2, d: f64) -> f64 {
        let wi = &self.st.wires[w];
        let tris: Vec<TriId> = (k0..=k1.min(wi.tris.len().saturating_sub(1))).map(|k| wi.tris[k]).collect();
        self.via_plan_price_in(w, &tris, p, d)
    }

    /// draw-via-neck A (r4, `[drawing] via_shift_mm` > 0): [`Drawer::via_plan_price`] over the
    /// interval's triangles and every triangle sharing a vertex with the site's own (traversal
    /// `k`): a site near its triangle's side reaches into the corridors of wires that pass the
    /// next triangles, outside the interval (qmtech: SDRAM_A2's via shifted 1 mm onto
    /// SDRAM_A1's corridor next to its interval; the price saw nothing). O((interval + ring
    /// triangles) x traversals).
    fn via_plan_price_near(&self, w: WireId, k0: usize, k1: usize, k: usize, p: Vec2, d: f64) -> f64 {
        let pr = &self.b.proj;
        let wi = &self.st.wires[w];
        let n = wi.tris.len();
        let mut tris: Vec<TriId> = (k0..=k1.min(n.saturating_sub(1))).map(|j| wi.tris[j]).collect();
        if let Some(&t) = wi.tris.get(k) {
            tris.extend(pr.tris[t].iter().flat_map(|&v| pr.vert_tris[v].iter().copied()));
        }
        tris.sort_unstable();
        tris.dedup();
        self.via_plan_price_in(w, &tris, p, d)
    }

    fn via_plan_price_in(&self, w: WireId, tris: &[TriId], p: Vec2, d: f64) -> f64 {
        let room = self.octi.room;
        if room <= 0.0 {
            return 0.0;
        }
        let st = self.st;
        let wi = &st.wires[w];
        let mut seen: Vec<(WireId, usize)> = Vec::new();
        // draw-via-neck r5 (esp RCA: +3V3's via moved onto IO8_SCL's pinned pad exit because
        // IO6's corridor, crossing 9 small triangles, was counted 9 times): with the shift on, a
        // neighbour's corridor counts once, at its deepest intrusion, so the price does not
        // depend on the triangulation's density ([`neighbour_room_price`]). Off: per traversal,
        // as before.
        let per_wire = self.opts.via_shift_mm > 0.0;
        let mut hits: Vec<RoomHit> = Vec::new();
        for &t in tris {
            for &(o, ok) in &st.tri_trav[t] {
                let oi = &st.wires[o];
                if o == w || !oi.alive || oi.net == wi.net || self.drawn_once[o] || seen.contains(&(o, ok)) {
                    continue;
                }
                seen.push((o, ok));
                // Both views of where the plan puts the wire: its taut chord (where the drawing
                // pulls it) and its packed nominal chord (the side test's and the via walls').
                let nom = self.plan_chord(o, ok);
                let taut = self.taut_chord(o, ok);
                let Some(dist) = taut.windows(2).chain(nom.windows(2)).map(|x| seg_dist(p, x[0], x[1])).reduce(f64::min) else { continue };
                let s = oi.s.max(wi.s);
                let pitch = (oi.w + s).max(1.0);
                let depth = 0.5 * d + s + 0.5 * oi.w - dist;
                if depth > 0.0 {
                    // How freely the neighbour can give way there (lead r5): its packed slack
                    // away from the via (only read with the shift on).
                    let slack = if per_wire { self.pass_slack_away(o, ok, p) } else { 0.0 };
                    if std::env::var_os("TOPO_VIAPLANDBG").is_some() {
                        eprintln!("VIAPLANP wire {w} at ({:.3},{:.3}) other {o} net {} ok {ok} dist {:.3} depth {:.3} slack {:.3} pitch {:.3}", p.x / 1e6, -p.y / 1e6, oi.net, dist / 1e6, depth / 1e6, slack / 1e6, pitch / 1e6);
                    }
                    hits.push(RoomHit { wire: o, depth, slack, pitch });
                }
            }
        }
        room * neighbour_room_price(&hits, per_wire)
    }

    /// draw-via-neck r5 (lead 2026-11-26, esp RCA): how far wire `o` can give way from a via at
    /// `p` in its traversal `ok`: on the pass of that traversal nearest to `p` (its entry or exit
    /// edge), the packed interval's room ([`Drawer::iv`], `lo..hi` along the edge) on the side
    /// away from the via, nm. 0 where the pass is not packed or the traversal has no pass (a
    /// wire between two terminals in one triangle: pinned). O(1).
    fn pass_slack_away(&self, o: WireId, ok: usize, p: Vec2) -> f64 {
        let (st, pr) = (self.st, &self.b.proj);
        let ps = &st.wires[o].passes;
        let mut best: Option<(f64, f64)> = None; // (distance to the lane point, slack)
        for j in [ok.checked_sub(1), Some(ok)].into_iter().flatten().filter(|&j| j < ps.len()) {
            let pid = ps[j];
            let (Some(&lo), Some(&hi), Some(&nom)) = (self.iv.lo.get(pid), self.iv.hi.get(pid), self.iv.nom.get(pid)) else { continue };
            if !(lo.is_finite() && hi.is_finite() && nom.is_finite()) {
                continue;
            }
            let [a, b] = pr.edges[st.passes[pid].edge];
            let (a, b) = (pr.pos[a], pr.pos[b]);
            let len = (b - a).len();
            if len < 1.0 {
                continue;
            }
            let lane = a + (b - a) * nom;
            let toward_b = (p - lane).dot(b - a) > 0.0;
            let slack = if toward_b { (nom - lo) * len } else { (hi - nom) * len };
            let d = (p - lane).len();
            if best.is_none_or(|x| d < x.0) {
                best = Some((d, slack));
            }
        }
        best.map_or(0.0, |x| x.1)
    }

    /// draw-via-neck r5: [`Drawer::via_room`] against copper only (the neighbours' planned
    /// chords are priced with their slack in [`Drawer::via_plan_price`] when the shift is on, so
    /// the squeeze does not count them a second time). O(nearby copper).
    fn via_room_copper(&self, w: WireId, p: Vec2, d: f64) -> f64 {
        let wi = &self.st.wires[w];
        let own = self.model.rules.net_rule(wi.net).clearance as f64;
        let mut room = f64::INFINITY;
        let mut ids = self.grid.query(bbox_of(&[p]).grow(2.0e6 + d));
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            let e = &self.ents[id];
            if !e.live || e.net == Some(wi.net) {
                continue;
            }
            let dist = match &e.core {
                Core::Poly(q) => poly_dist(p, q),
                Core::Seg(a, b) => seg_dist(p, *a, *b),
                Core::Pt(q) => (p - *q).len(),
            };
            let need = if e.fixed { e.clear } else { e.clear.max(own) };
            room = room.min(dist - e.grow - 0.5 * d - need);
        }
        room
    }

    /// draw-via-neck A (r4, `[drawing] via_shift_mm` > 0): price of a via of wire `w` (reach
    /// diameter `d`) at `p` for the planned via sites (`via_site`) of the other nets' wires not
    /// drawn yet: per site, the depth by which the two reach discs plus the clearance overlap,
    /// priced like [`Drawer::via_plan_price`] (`room x pitch x (x + x^2)`, `x = depth / pitch`,
    /// `pitch` = the other via's reach diameter plus the clearance): graded, no wall. Why: a via
    /// shifted 0.14 mm took the planned site of the next wire's via (qmtech ADV_SDA / ADV_SCL at
    /// U7: ADV_SCL's via then had no legal site), and `via_plan_price` sees only the corridors.
    /// O(planned via sites).
    fn via_site_price(&self, w: WireId, p: Vec2, d: f64) -> f64 {
        let room = self.octi.room;
        if room <= 0.0 {
            return 0.0;
        }
        let st = self.st;
        let wi = &st.wires[w];
        let mut tot = 0.0;
        for (&(o, _), &(_, q)) in &self.via_site {
            let oi = &st.wires[o];
            if o == w || !oi.alive || oi.net == wi.net || self.drawn_once[o] {
                continue;
            }
            let s = oi.s.max(wi.s);
            let dist = (p - q).len();
            if dist >= 0.5 * d + s + 2.0e6 {
                continue;
            }
            let Some(&sz) = via_sizes(&self.model.rules, oi.net, self.opts).first() else { continue };
            let dor = 2.0 * crate::vias::via_reach(&self.model.rules, oi.net, sz);
            let depth = 0.5 * d + s + 0.5 * dor - dist;
            if depth > 0.0 {
                let pitch = (dor + s).max(1.0);
                let x = depth / pitch;
                tot += pitch * (x + x * x);
            }
        }
        room * tot
    }

    /// P6: price of the room a via pair keeps for the wires the plan crosses between its two
    /// vias. With the wire's previous via at `prev` (traversal, centre, size), a via of size
    /// `sz` at `p` in traversal `k` leaves the wires of `xs` that cross the wire in traversals
    /// `prev.k..=k` (and are not drawn yet) the gap between the two discs; they need the reach of
    /// each via ([`crate::vias::via_reach`], 5.4 task 1), their widths and a clearance either side. Short of that by `depth`, the price is
    /// `room x pitch x (x + x^2)` with `x = depth / pitch` and `pitch` the widths plus clearances
    /// of those wires: graded, no wall, like [`Drawer::via_plan_price`]. 0 without a previous via
    /// or a wire crossing between. O(crossings).
    fn via_pair_price(&self, w: WireId, prev: Option<(usize, Vec2, ViaSize)>, xs: &[(usize, WireId)], k: usize, p: Vec2, sz: ViaSize) -> f64 {
        let Some((k0, q, sq)) = prev else { return 0.0 };
        let wi = &self.st.wires[w];
        let (lo, hi) = (k0.min(k), k0.max(k));
        let mut seen: Vec<WireId> = Vec::new();
        let mut pitch = 0.0;
        for &(xk, o) in xs {
            if xk < lo || xk > hi || self.drawn_once[o] || seen.contains(&o) {
                continue;
            }
            if self.site_slack > 0.0 && !self.taut_between(o, q, p) {
                continue;
            }
            seen.push(o);
            let oi = &self.st.wires[o];
            pitch += oi.w + oi.s.max(wi.s);
        }
        if pitch <= 0.0 {
            return 0.0;
        }
        // Each via as its reach (5.4 task 1: the hole clearance of a small via's drill reaches
        // past its disc's clearance), then the crossing wires and their clearances.
        let r = &self.model.rules;
        let need = crate::vias::via_reach(r, wi.net, sq) + crate::vias::via_reach(r, wi.net, sz) + wi.s + pitch;
        let depth = need - (p - q).len();
        if depth <= 0.0 {
            return 0.0;
        }
        let x = depth / pitch;
        self.octi.room * pitch * (x + x * x)
    }

    /// M3 slack: wire `o`'s planned (taut) track crosses the segment `q`-`p` (between two vias of
    /// one wire). O(passes).
    fn taut_between(&self, o: WireId, q: Vec2, p: Vec2) -> bool {
        let (st, pr) = (self.st, &self.b.proj);
        let oi = &st.wires[o];
        let t = self.packer.taut(st, o);
        let pts: Vec<Vec2> = std::iter::once(pr.pos[oi.a]).chain(t.at.iter().copied()).chain(std::iter::once(pr.pos[oi.b])).collect();
        pts.windows(2).any(|x| super::viaopt::seg_seg(x[0], x[1], q, p) <= 0.0)
    }

    /// Free room (nm) a via of diameter `d` at `p` of `net` leaves to the nearest foreign copper
    /// indexed so far (static board and committed items, any layer), past the clearance; the
    /// part of [`Drawer::via_room`] that needs no wire (D6a, plane stub vias). O(objects nearby).
    pub fn point_room(&self, p: Vec2, d: f64, net: i32) -> f64 {
        let own = self.model.rules.net_rule(net).clearance as f64;
        let mut room = f64::INFINITY;
        let mut ids = self.grid.query(bbox_of(&[p]).grow(2.0e6 + d));
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            let e = &self.ents[id];
            if !e.live || e.net == Some(net) {
                continue;
            }
            let dist = match &e.core {
                Core::Poly(q) => poly_dist(p, q),
                Core::Seg(a, b) => seg_dist(p, *a, *b),
                Core::Pt(q) => (p - *q).len(),
            };
            let need = if e.fixed { e.clear } else { e.clear.max(own) };
            room = room.min(dist - e.grow - 0.5 * d - need);
        }
        room
    }

    /// draw-via-neck A (`[drawing] via_shift_mm`; TopoR 6.3 manual p. 179: vias shifted into free
    /// room; Dayan & Dai 1991 UCSC-CRL-91-40 pp. 2-6): extra via sites for via `i` of wire `w`
    /// inside its interval `ka..=kb`: a ring of 12 directions at 0.5, 1, 1.5 and 2 via diameters
    /// (up to the shift distance) round the middle of the interval's taut chord, each in the
    /// interval's traversal whose triangle holds it. Never outside the interval (lead 2026-11-24):
    /// the interval runs between two layer events of the plan (`layers.rs`: a crossing, an SMD
    /// pad, a one-layer triangle, a gap that fits one layer), so a site past it puts the run on the
    /// wrong layer at the bounding event; widening the interval is the plan's job.
    /// O(48 x interval traversals).
    fn via_shift_sites(&self, w: WireId, ka: usize, kb: usize, dv: f64) -> Vec<(usize, Vec2)> {
        let (st, pr) = (self.st, &self.b.proj);
        let wi = &st.wires[w];
        let n = wi.tris.len();
        if n == 0 {
            return Vec::new();
        }
        let lim = self.opts.via_shift_mm * 1e6;
        let kb = kb.min(n - 1);
        let chord: Vec<Vec2> = (ka..=kb).flat_map(|k| self.taut_chord(w, k)).collect();
        if chord.is_empty() {
            return Vec::new();
        }
        let mid = chord[chord.len() / 2];
        let tri_of = |p: Vec2| (ka..=kb).find(|&k| super::state::point_in_poly(p, &pr.tris[wi.tris[k]].map(|v| pr.pos[v])));
        let mut out: Vec<(usize, Vec2)> = Vec::new();
        for r in [0.5, 1.0, 1.5, 2.0] {
            let rr = r * dv;
            if rr > lim {
                break;
            }
            for j in 0..12 {
                let a = j as f64 * std::f64::consts::PI / 6.0;
                let p = mid + Vec2 { x: rr * a.cos(), y: rr * a.sin() };
                if let Some(k) = tri_of(p) {
                    out.push((k, p));
                }
            }
        }
        out
    }

    /// Place the vias of wire `w` (ids of the indexed items in `ids`), or the reason it cannot be.
    pub fn place_vias(&mut self, w: WireId, plan: &Plan, ids: &mut Vec<usize>) -> Result<Vec<(usize, Vec2, ViaSize)>, String> {
        let net = self.st.wires[w].net;
        let sizes = via_sizes(&self.model.rules, net, self.opts);
        let mut out: Vec<(usize, Vec2, ViaSize)> = Vec::new();
        // P6: the other nets' wires the plan crosses this wire with (their traversal here), for
        // the room a via pair keeps between its two vias.
        let xs: Vec<(usize, WireId)> = if self.octi.room > 0.0 {
            self.st.crossings(w).iter().filter(|x| self.st.wires[x.other].net != net).map(|x| (x.k, x.other)).collect()
        } else {
            Vec::new()
        };
        // The plan's crossings of this wire, per traversal and layer: a via site's reach may go
        // over a drawn track only where the plan crosses it ([`Drawer::via_reach`]).
        let crossed = self.class_crossings(w);
        self.lead_via.remove(&(w, 0));
        self.lead_via.remove(&(w, 1));
        for (i, &(ka, kb)) in plan.vias.iter().enumerate() {
            let ka = out.last().map_or(ka, |l| ka.max(l.0));
            let mut done = false;
            // exist-term r4/r5: the plan changes layer at a continued dangling end's lead end: the
            // via at the lead's via end, the size that end was found for (net class first,
            // `vias::lead_via_sizes`; it fits there against the static copper, the checks below
            // judge the copper drawn so far). Refused: the plan's site / search / force below.
            if let Some((end, (ve, sz))) = self.lead_via_site(w, i, ka, plan) {
                let item = via_item(net, ve, sz, Some(w));
                let k = if end == 0 { 0 } else { self.st.wires[w].passes.len() };
                if self.via_side_ok(w, k, ve, plan.layers[i], plan.layers[i + 1]) && self.check_ok(&item) && self.via_hole_ok(ve, sz, net) && self.via_own_pad_ok(ve, sz, net) && self.pair_via_ok(w, ve, sz, net) {
                    let id = self.commit(&item);
                    self.via_marks.push((w, k, ve, plan.layers[i], plan.layers[i + 1], id));
                    ids.push(id);
                    out.push((k, ve, sz));
                    self.lead_via.insert((w, end), ve);
                    self.stats.planned_sites += 1;
                    if std::env::var_os("TOPO_KEPTDBG").is_some() {
                        eprintln!("KEPTVIA net {net} at ({:.4}, {:.4}) mm d {} drill {}", ve.x / 1e6, -ve.y / 1e6, sz.d, sz.drill);
                    }
                    continue;
                }
            }
            // The cheapest legal site out of the plan's class ([`Drawer::via_reach`]): taken only
            // when no site in the class exists at all.
            let mut oob: Option<(f64, f64, usize, Vec2, ViaSize)> = None;
            // P6 (d): the planned site when it is not free: the search then starts next to it.
            let mut missed: Option<Vec2> = None;
            // 5.4: the plan's site first, as it is, when every rule holds there.
            if let Some(&(k, p)) = self.via_site.get(&(w, i)).filter(|s| (ka..=kb.max(ka)).contains(&s.0)) {
                for &sz in &sizes {
                    let item = via_item(net, p, sz, Some(w));
                    // The side test on the drawing's nominal chords, or on the plan's taut
                    // geometry the relocation balanced the via on (the drawing's checker then
                    // judges the partners). M6 (user 2026-11-18) removed P6 a's partner move
                    // (`follow_vias`), which led a wire crossing a via wire's part back to the side
                    // the plan gave it (Polubasov 2001, printed p. 4). Its effect depends on the
                    // plan: on esp-rust-board (seed 0, new net numbering) the forced USB_D- then runs
                    // across IO21_TX on B.Cu next to its own via: 14 shorts, +43 DRC. In other plans
                    // it is -12 or 0. The real fix is ERBS sheets with the placed vias, or the plan
                    // itself (TopoR-way plan), not a drawing stand-in.
                    let (la, lb) = (plan.layers[i], plan.layers[i + 1]);
                    let taut_ok = || !self.via_site_tight.contains(&(w, i)) && self.via_side_ok_taut(w, k, p, la, lb);
                    // P6 (d): with the previous via as drawn (the plan's site of that one may not
                    // have been free), the pair still leaves the wires that cross between them
                    // their room ([`Drawer::via_pair_price`] 0); else the search prices the site.
                    let pair_ok = || self.via_pair_price(w, out.last().copied(), &xs, k, p, sz) <= 0.0;
                    if (self.via_side_ok(w, k, p, la, lb) || taut_ok()) && pair_ok() && self.check_ok(&item) && self.via_hole_ok(p, sz, net) && self.via_own_pad_ok(p, sz, net) && self.pair_via_ok(w, p, sz, net) {
                        let id = self.commit(&item);
                        self.via_marks.push((w, k, p, plan.layers[i], plan.layers[i + 1], id));
                        ids.push(id);
                        out.push((k, p, sz));
                        self.stats.planned_sites += 1;
                        self.pair_via_dbg(w, i, "planned", p, sz, 0);
                        done = true;
                        break;
                    }
                }
                if done {
                    continue;
                }
                self.stats.planned_sites_missed += 1;
                if std::env::var("TOPO_VIADBG").is_ok() {
                    // Diagnosis only: which test refused the planned site (first size).
                    let sz = sizes[0];
                    let item = via_item(net, p, sz, Some(w));
                    let (la, lb) = (plan.layers[i], plan.layers[i + 1]);
                    let rep = self.checker.check_against(&self.idx, std::slice::from_ref(&item), &[]);
                    eprintln!(
                        "VIAMISS wire {w} net {net} via {i} planned ({:.3},{:.3}) d {} side {} taut {} tight {} pair {} check {:?} hole {} own {} pairdisc {}",
                        p.x / 1e6, -p.y / 1e6, sz.d,
                        self.via_side_ok(w, k, p, la, lb), self.via_side_ok_taut(w, k, p, la, lb), self.via_site_tight.contains(&(w, i)),
                        self.via_pair_price(w, out.last().copied(), &xs, k, p, sz),
                        rep.violations.iter().take(3).map(|v| (v.against.clone(), v.gap, v.required)).collect::<Vec<_>>(),
                        self.via_hole_ok(p, sz, net), self.via_own_pad_ok(p, sz, net), self.pair_via_ok(w, p, sz, net)
                    );
                }
                // A site the relocation marked tight (no legal room on the plan's geometry,
                // planned_crossing_window: the taut crossing lies outside the via window) tells
                // nothing about where the next-best site is: the search runs as without a plan.
                if !self.via_site_tight.contains(&(w, i)) {
                    missed = Some(p);
                }
            }
            'sizes: for &sz in &sizes {
                let need_r = crate::vias::via_need_copper(&self.model.rules, net, sz, 0.0, self.opts.margin_nm.max(0) as f64);
                let at = plan.at.get(i).copied().filter(|&k| (ka..=kb.max(ka)).contains(&k));
                let mut sites = at.map_or(Vec::new(), |k| self.chord_sites(w, k));
                if let Some(k) = at {
                    sites.extend(self.taut_sites(w, k));
                }
                sites.extend(self.via_candidates(w, ka, kb.max(ka), need_r, at));
                if self.opts.via_offset_sites {
                    sites.extend(self.via_extra(w, ka, kb.max(ka), sz.d as f64, self.model.rules.net_rule(net).clearance as f64));
                }
                // P6 (d): a planned site that was not free: the next-best site is the nearest one
                // to it on the plan's taut path (the force balance put it there; the plan's
                // crossings keep their order along that path), so those come first, nearest
                // first, and the distance is priced (below). Polubasov 2001, printed p. 4.
                if let Some(p0) = missed {
                    let mut near: Vec<(usize, Vec2)> = Vec::new();
                    for k in ka..=kb.max(ka) {
                        let pl = self.taut_chord(w, k);
                        for x in pl.windows(2) {
                            near.extend((0..=16).map(|j| (k, x[0] + (x[1] - x[0]) * (j as f64 / 16.0))));
                        }
                    }
                    near.sort_by(|a, b| (a.1 - p0).len2().total_cmp(&(b.1 - p0).len2()));
                    near.extend(sites);
                    sites = near;
                }
                // D7: a via pair keeps out of its pads' fan-out zone where a site farther away exists
                // (sites near the pads go last, in their order).
                if let Some(&keep) = self.pair_keep.get(&net) {
                    let wi = &self.st.wires[w];
                    let (ea, eb) = (self.b.proj.pos[wi.a], self.b.proj.pos[wi.b]);
                    let near = |p: Vec2| (p - ea).len().min((p - eb).len()) < keep;
                    let (far, close): (Vec<_>, Vec<_>) = sites.into_iter().partition(|&(_, p)| !near(p));
                    sites = far.into_iter().chain(close).collect();
                }
                // draw-via-neck A (`[drawing] via_shift_mm`): a ring of sites round the interval's
                // taut chord, inside the interval (never past it: that breaks the plan's layer
                // event); each priced by its shift below. They come after the interval's own sites.
                if self.opts.via_shift_mm > 0.0 {
                    sites.extend(self.via_shift_sites(w, ka, kb.max(ka), sz.d as f64));
                }
                // Of the valid sites (side test, clearance) in the first triangle that has one
                // (the planned one first: the plan fixed which crossings lie on which side of
                // the via, and its packing kept the disc's room there), the one that takes the
                // least of the room planned for the wires not drawn yet (P6,
                // [`Drawer::via_plan_price`], [`Drawer::via_pair_price`]) plus the same price on
                // free room short of one via diameter around it (the `via_squeeze` measure); at
                // the same price the one with the most free room. Measured: letting a cheaper site
                // in a later triangle win (with a crossing-order check) moved vias out of the
                // packed bus on qmtech (round 1: 49 -> 55 connections with violations); kept in
                // the first triangle it is 47.
                let mut best: Option<(f64, usize, Vec2)> = None;
                let mut best_price = f64::INFINITY;
                let mut best_jam = 0.0f64;
                let dbg = std::env::var("TOPO_VIADBG").is_ok();
                let (mut n_side, mut n_check) = (0usize, 0usize);
                // In a traversal where the wire turns at a junction, only the junction itself:
                // elsewhere in it the via could stand on the wrong side of the tee (D6b).
                let turn_at = |k: usize| self.st.wires[w].turns.iter().find(|x| x.0 == k).map(|x| self.b.proj.pos[x.1]);
                // draw-via-neck A: the first traversal's first legal site (the shift price's
                // origin).
                let mut first_pick: Option<(usize, Vec2)> = None;
                // draw-via-neck A: the plan's site (free, missed or tight), where it lies in the
                // interval: the shift's origin.
                let plan_site = self.via_site.get(&(w, i)).filter(|s| (ka..=kb.max(ka)).contains(&s.0)).map(|s| s.1);
                for (k, p) in sites {
                    // The first traversal with a legal site decides, unless that site takes
                    // planned room (price > 0) and a shift may find a cheaper one (draw-via-neck A).
                    if best.is_some_and(|b| b.1 != k) && (self.opts.via_shift_mm <= 0.0 || best_jam <= 1.0) {
                        break;
                    }
                    if turn_at(k).is_some_and(|q| (q - p).len() > 1.0) {
                        continue;
                    }
                    if dbg {
                        // RCA aid: TOPO_VIAREJAT="x,y;x,y" (KiCad mm): every test of the sites within
                        // 0.06 mm of that point, with the blocking item's net.
                        let pts: Vec<(f64, f64)> = std::env::var("TOPO_VIAREJAT").unwrap_or_default().split(';').filter_map(|v| v.split_once(',').and_then(|(a, b)| Some((a.parse::<f64>().ok()?, b.parse::<f64>().ok()?)))).collect();
                        {
                            if pts.iter().any(|&(qx, qy)| (p.x / 1e6 - qx).hypot(-p.y / 1e6 - qy) < 0.06) {
                                let item = via_item(net, p, sz, Some(w));
                                let rep = self.checker.check_against(&self.idx, std::slice::from_ref(&item), &[]);
                                let who: Vec<_> = rep.violations.iter().take(3).map(|v| (v.against.clone(), v.against.strip_prefix("item:").and_then(|x| x.parse::<usize>().ok()).and_then(|id| self.idx.net_of(id)), v.gap, v.required)).collect();
                                eprintln!("VIAREJAT wire {w} via {i} k {k} at ({:.3},{:.3}) side {} check {who:?} hole {} own {} pair {}", p.x / 1e6, -p.y / 1e6, self.via_side_ok(w, k, p, plan.layers[i], plan.layers[i + 1]), self.via_hole_ok(p, sz, net), self.via_own_pad_ok(p, sz, net), self.pair_via_ok(w, p, sz, net));
                            }
                        }
                    }
                    if !self.via_side_ok(w, k, p, plan.layers[i], plan.layers[i + 1]) {
                        n_side += usize::from(Some(k) == at);
                        if dbg && Some(k) == at && std::env::var("TOPO_VIASIDE").is_ok() {
                            eprintln!("VIASIDE wire {w} via {i} k {k} at ({:.3},{:.3}) la {} lb {} entry ({:.3},{:.3}) exit ({:.3},{:.3})", p.x / 1e6, -p.y / 1e6, plan.layers[i], plan.layers[i + 1], self.chord_pt(w, k).x / 1e6, -self.chord_pt(w, k).y / 1e6, self.chord_pt(w, k + 1).x / 1e6, -self.chord_pt(w, k + 1).y / 1e6);
                        }
                        continue;
                    }
                    let item = via_item(net, p, sz, Some(w));
                    if dbg && Some(k) == at && !(self.check_ok(&item) && self.via_hole_ok(p, sz, net) && self.via_own_pad_ok(p, sz, net)) {
                        n_check += 1;
                        if n_check <= 2 {
                            let rep = self.checker.check_against(&self.idx, std::slice::from_ref(&item), &[]);
                            eprintln!("VIAREJ wire {w} via {i} k {k} at ({:.3},{:.3}): {:?} hole {} own {}", p.x / 1e6, -p.y / 1e6, rep.violations.first().map(|v| (v.against.clone(), v.gap, v.required)), self.via_hole_ok(p, sz, net), self.via_own_pad_ok(p, sz, net));
                        }
                    }
                    if self.check_ok(&item) && self.via_hole_ok(p, sz, net) && self.via_own_pad_ok(p, sz, net) && self.pair_via_ok(w, p, sz, net) {
                        let room = self.via_room(w, k, p, sz.d as f64);
                        // The room around the via is priced the same way (short of one via
                        // diameter of free room, the `via_squeeze` measure), so a site free of the
                        // planned corridors does not win when it squeezes the via against other
                        // copper; where nothing is planned this orders the sites by room as before.
                        let dv = (sz.d as f64).max(1.0);
                        let room_sq = if self.opts.via_shift_mm > 0.0 { self.via_room_copper(w, p, sz.d as f64) } else { room };
                        let xq = (dv - room_sq).max(0.0) / dv;
                        let squeeze = self.octi.room * dv * (xq + xq * xq);
                        // P6 (d): away from a planned site that was not free, graded.
                        let xm = missed.map_or(0.0, |p0| (p - p0).len() / dv);
                        let squeeze = squeeze + self.octi.room * dv * (xm + xm * xm);
                        // 5.4 task 1: the via as its reach disc (the hole rule included).
                        let dr = 2.0 * crate::vias::via_reach(&self.model.rules, net, sz);
                        let plan_p = if self.opts.via_shift_mm > 0.0 { self.via_plan_price_near(w, ka, kb.max(ka), k, p, dr) + self.via_site_price(w, p, dr) } else { self.via_plan_price(w, ka, kb.max(ka), p, dr) };
                        // draw-via-neck A: what the site takes from its neighbours (their planned
                        // room, its own squeeze): while the best site takes some, later traversals
                        // and shifted sites still compete.
                        let jam = plan_p + squeeze;
                        let price = plan_p + self.via_pair_price(w, out.last().copied(), &xs, k, p, sz) + squeeze;
                        // In the plan's class only; the reach's violations priced like the squeeze
                        // (one via diameter of squeeze per violation unit, graded).
                        let (in_class, reach) = self.via_reach_for_price(w, k, p, plan.layers[i], plan.layers[i + 1], &crossed);
                        // `[vias] reach_price` (default off): the reach's violations priced too.
                        let price = price + self.reach_cost(reach, self.octi.room * dv);
                        // draw-via-neck A: a site past the first traversal with a legal site is
                        // never more than `via_shift_mm` from the plan's site (free, missed or
                        // tight; without one, that first legal site, today's pick), and pays its
                        // distance from it, graded (half the squeeze price per via diameter; from
                        // a missed planned site the P6 (d) price above already does): it wins
                        // only by the room it gives back. r4: priced by the distance from the
                        // plan's taut path, a site far along the path was free (qmtech JS1_9 and
                        // SDRAM_A2 moved 1.4-1.5 mm into their neighbours' room).
                        let price = match first_pick {
                            Some((k1, p1)) if self.opts.via_shift_mm > 0.0 && k != k1 => {
                                let sh = (p - plan_site.unwrap_or(p1)).len();
                                if sh > self.opts.via_shift_mm * 1e6 {
                                    if dbg {
                                        eprintln!("VIAFAR wire {w} via {i} k {k} at ({:.3},{:.3}) shift {:.3} mm", p.x / 1e6, -p.y / 1e6, sh / 1e6);
                                    }
                                    continue;
                                }
                                let xs_ = if missed.is_some() { 0.0 } else { sh / dv };
                                price + 0.5 * self.octi.room * dv * (xs_ + xs_ * xs_)
                            }
                            _ => price,
                        };
                        if dbg {
                            eprintln!("VIACAND wire {w} via {i} k {k} at ({:.3},{:.3}) room {:.0} price {:.0} plan {plan_p:.0} squeeze {squeeze:.0} reach {reach:.2} in class {in_class}", p.x / 1e6, -p.y / 1e6, room, price);
                        }
                        if !in_class {
                            if oob.as_ref().is_none_or(|o| price < o.0 - 1.0 || (price <= o.0 + 1.0 && room > o.1 + 1.0)) {
                                oob = Some((price, room, k, p, sz));
                            }
                            continue;
                        }
                        if best.is_none_or(|b| price < best_price - 1.0 || (price <= best_price + 1.0 && room > b.0 + 1.0)) {
                            best = Some((room, k, p));
                            best_price = price;
                            best_jam = jam;
                            if first_pick.is_none_or(|f| f.0 == k) {
                                first_pick = Some((k, p));
                            }
                        }
                    }
                }
                if let Some((room, k, p)) = best {
                    if dbg {
                        for pad in self.model.pads.iter().filter(|pad| pad.net == net) {
                            for &l in &pad.layers {
                                let d = crate::geom::point_skeleton(p, &self.model.skeleton(pad, l));
                                if d < 0.5 * sz.d as f64 + 1e5 {
                                    eprintln!("VIAOWN wire {w} via at ({:.3},{:.3}) pad {}.{} layer {l} d {d:.0} r {}", p.x / 1e6, -p.y / 1e6, pad.ref_, pad.number, sz.d / 2);
                                }
                            }
                        }
                        eprintln!("VIAPICK wire {w} net {net} via {i} ka {ka} kb {kb} planned {at:?} chosen k {k} at ({:.3},{:.3}) room {:.0} size {} (planned triangle: {n_side} wrong side, {n_check} rejected)", p.x / 1e6, -p.y / 1e6, room, sz.d);
                    }
                    let id = self.commit(&via_item(net, p, sz, Some(w)));
                    self.via_marks.push((w, k, p, plan.layers[i], plan.layers[i + 1], id));
                    ids.push(id);
                    out.push((k, p, sz));
                    self.stats.via_searched += 1;
                    self.pair_via_dbg(w, i, if self.via_site.contains_key(&(w, i)) { "searched (planned site not free)" } else { "searched (no planned site)" }, p, sz, 0);
                    done = true;
                    break 'sizes;
                }
            }
            // No legal site in the plan's class: in force mode the cheapest site in the class with
            // its violations (costly, not failed); with no site in the class at all, the cheapest
            // legal one out of it (as before the class test), counted and reported.
            let forced_in = if !done && self.force { self.force_via(w, plan, i, ka, kb.max(ka), &sizes, Some(&crossed)) } else { None };
            if !done && forced_in.is_none() {
                if let Some((_, room, k, p, sz)) = oob {
                    if std::env::var("TOPO_VIADBG").is_ok() {
                        eprintln!("VIAPICK wire {w} net {net} via {i} out of class at ({:.3},{:.3}) room {room:.0} size {}", p.x / 1e6, -p.y / 1e6, sz.d);
                    }
                    let id = self.commit(&via_item(net, p, sz, Some(w)));
                    self.via_marks.push((w, k, p, plan.layers[i], plan.layers[i + 1], id));
                    ids.push(id);
                    out.push((k, p, sz));
                    self.stats.via_searched += 1;
                    self.stats.via_out_of_class += 1;
                    done = true;
                }
            }
            if !done && self.force {
                // Never drop (M-B1): the site with the least violation price.
                let any_site = forced_in.is_none();
                if let Some((k, p, sz, vs)) = forced_in.or_else(|| self.force_via(w, plan, i, ka, kb.max(ka), &sizes, None)) {
                    // A site forced over every site may be out of the class: counted like the legal one.
                    if any_site && !self.via_in_class(w, k, p, plan.layers[i], plan.layers[i + 1], &crossed) {
                        self.stats.via_out_of_class += 1;
                    }
                    let id = self.commit(&via_item(net, p, sz, Some(w)));
                    self.via_marks.push((w, k, p, plan.layers[i], plan.layers[i + 1], id));
                    ids.push(id);
                    out.push((k, p, sz));
                    self.pair_via_dbg(w, i, if self.via_site.contains_key(&(w, i)) { "forced (planned site not free)" } else { "forced (no planned site)" }, p, sz, vs.len());
                    let mut vs = vs;
                    let base = ViolSrc { mech: "forced_via", cand: "via", run: i, w_nm: self.st.wires[w].w, w_min_nm: self.st.w_min(net, self.st.wires[w].w), ..ViolSrc::default() };
                    self.tag_viols(&mut vs, &base);
                    self.via_viol.entry(w).or_default().extend(vs);
                    self.stats.via_forced += 1;
                    continue;
                }
            }
            if !done {
                if std::env::var("TOPO_VIADBG").is_ok() {
                    let wi = &self.st.wires[w];
                    let pr = &self.b.proj;
                    let need_r = 0.5 * sizes[0].d as f64 + self.model.rules.net_rule(net).clearance as f64;
                    let tr: Vec<(f64, bool, usize)> = (ka..=kb.max(ka)).map(|k| { let t = wi.tris[k]; (pr.inradius(t), pr.tri_out[t] || !pr.tri_obs[t].is_empty(), self.st.tri_trav[t].len()) }).collect();
                    let best = tr.iter().filter(|x| !x.1).map(|x| x.0).fold(0.0, f64::max);
                    // Report only (D1 via-stage diagnosis): would a via inside one of the wire's
                    // own-net pads in the interval (via-in-pad, at the pad centre) be clean?
                    let own: std::collections::BTreeSet<usize> = (ka..=kb.max(ka))
                        .flat_map(|k| pr.tri_obs[wi.tris[k]].iter().copied())
                        .filter(|&o| pr.obstacles[o].net == Some(net) && pr.obstacles[o].kind == super::state::ObsKind::Pad)
                        .collect();
                    let vip = own.iter().any(|&o| {
                        let c = pr.obstacles[o].center;
                        sizes.iter().any(|&sz| self.check_ok(&via_item(net, c, sz, Some(w))) && self.via_hole_ok(c, sz, net))
                    });
                    let blocked = tr.iter().filter(|x| !x.1 && x.0 >= need_r).count();
                    let mid = pr.pos[pr.tris[wi.tris[(ka + kb.max(ka)) / 2]][0]];
                    // Every candidate site of every size against the static board only (no
                    // other via placed yet), and what rejects it there.
                    let (mut n_sites, mut static_ok, mut hole_only, mut own_only) = (0usize, 0usize, 0usize, 0usize);
                    let r = &self.model.rules;
                    let hole_req = r.via_hole_clear() + self.opts.margin_nm;
                    let copper_req = r.net_rule(net).clearance.max(r.min_clearance) + self.opts.margin_nm;
                    for &sz in &sizes {
                        let need = 0.5 * sz.d as f64 + r.net_rule(net).clearance as f64;
                        let mut sites = self.via_candidates(w, ka, kb.max(ka), need, None);
                        if self.opts.via_offset_sites {
                            sites.extend(self.via_extra(w, ka, kb.max(ka), sz.d as f64, r.net_rule(net).clearance as f64));
                        }
                        for (_, p) in sites {
                            n_sites += 1;
                            let rep = self.checker.check_items(&[via_item(net, p, sz, Some(w))]);
                            let hole = self.via_hole_ok(p, sz, net);
                            let own_ok = self.via_own_pad_ok(p, sz, net);
                            if rep.violations.is_empty() && hole && own_ok {
                                static_ok += 1;
                            } else if rep.violations.is_empty() && hole {
                                own_only += 1;
                            } else if hole_req != copper_req && !rep.violations.is_empty() && rep.violations.iter().all(|v| v.required == hole_req) {
                                hole_only += 1;
                            }
                        }
                    }
                    // P3's own site test (layers.rs `site`): a triangle centroid of the interval
                    // with a free disc of the default via plus the default clearance.
                    let p3_r = 0.5 * r.default.via_d as f64 + r.default.clearance as f64;
                    let p3_site = (ka..=kb.max(ka)).any(|k| {
                        let t = wi.tris[k];
                        let c = pr.tris[t].iter().fold(Vec2 { x: 0.0, y: 0.0 }, |a, &v| a + pr.pos[v]) * (1.0 / 3.0);
                        !pr.tri_out[t] && pr.free_radius(c, net) >= p3_r
                    });
                    eprintln!(
                        "VIADBG wire {w} net {net} ka {ka} kb {kb} tris {} usable {} wide-enough {} best inradius {:.0} need_r {:.0} p3_r {:.0} p3-site {} sizes {} sites {} static-ok {} hole-only {} own-pad-only {} own-pads {} via-in-pad-ok {} at ({:.3},{:.3})",
                        tr.len(),
                        tr.iter().filter(|x| !x.1).count(),
                        blocked,
                        best,
                        need_r,
                        p3_r,
                        p3_site,
                        sizes.len(),
                        n_sites,
                        static_ok,
                        hole_only,
                        own_only,
                        own.len(),
                        vip,
                        mid.x / 1e6,
                        -mid.y / 1e6
                    );
                }
                return Err(format!("via {} of {}: no site in its interval (no free disc for a via)", i + 1, plan.vias.len()));
            }
        }
        Ok(out)
    }

    /// Via `i` of wire `w` by force (M-B1 never drop): of every candidate site in its interval
    /// `ka..=kb` (the legal search's sites, then every triangle's site points, obstacles or not)
    /// and every via size, the one with the least short amount (via copper over foreign copper,
    /// KiCad's `shorting_items`, plus its drill there, `hole_clearance`: no graded judgement call,
    /// [`viol_key`]; grid-snap r2: a site on a foreign track won over a clearance-only site), then
    /// the least `violation` price (a site on the wrong side of another wire's chord counts one
    /// violation more: its run would cross that wire), ties by the most room. Its violations go
    /// with it. O(sites x sizes x checker query).
    /// `class`: the plan's crossings of `w` ([`Drawer::class_crossings`]); `Some` = only sites in the
    /// plan's class ([`Drawer::via_reach`]), their reach violations added to the price with
    /// `[vias] reach_price`; `None` = every site.
    pub fn force_via(&self, w: WireId, plan: &Plan, i: usize, ka: usize, kb: usize, sizes: &[ViaSize], class: Option<&ClassCrossings>) -> Option<(usize, Vec2, ViaSize, Vec<Viol>)> {
        let net = self.st.wires[w].net;
        let (st, pr) = (self.st, &self.b.proj);
        let wi = &st.wires[w];
        let turn_at = |k: usize| wi.turns.iter().find(|x| x.0 == k).map(|x| pr.pos[x.1]);
        #[allow(clippy::type_complexity)]
        let mut best: Option<([f64; 3], f64, usize, Vec2, ViaSize, Vec<Viol>)> = None;
        // pad-snap r2b debug (`TOPO_VIADBG`): every forced candidate site with its price
        let fdbg = std::env::var_os("TOPO_VIADBG").is_some();
        let mut cands: Vec<(f64, usize, Vec2, i64, String)> = Vec::new();
        // TOPO_VIADBG (copper-neutral): every priced site as (price, short amount, k, site, size).
        let dbg = std::env::var_os("TOPO_VIADBG").is_some();
        let mut seen: Vec<(f64, f64, usize, Vec2, i64)> = Vec::new();
        for &sz in sizes {
            let need_r = crate::vias::via_need_copper(&self.model.rules, net, sz, 0.0, self.opts.margin_nm.max(0) as f64);
            let at = plan.at.get(i).copied().filter(|&k| (ka..=kb).contains(&k));
            let mut sites = at.map_or(Vec::new(), |k| self.chord_sites(w, k));
            sites.extend(self.via_candidates(w, ka, kb, need_r, at));
            for k in ka..=kb.min(wi.tris.len().saturating_sub(1)) {
                let t = wi.tris[k];
                if pr.tri_out[t] {
                    continue;
                }
                let [a, b, c] = pr.tris[t].map(|v| pr.pos[v]);
                sites.extend(super::pack::tri_site_points(a, b, c).into_iter().map(|p| (k, p)));
            }
            for (k, p) in sites {
                if turn_at(k).is_some_and(|q| (q - p).len() > 1.0) {
                    continue;
                }
                let item = via_item(net, p, sz, Some(w));
                let rep = self.checker.check_against(&self.idx, std::slice::from_ref(&item), &[]);
                let mut vs = self.viols_of(std::slice::from_ref(&item), &rep);
                if !self.via_hole_ok(p, sz, net) {
                    vs.push(Viol { kind: "hole_clearance", at: p, layer: u32::MAX, depth_nm: self.model.rules.via_hole_clear().max(1) as f64, len_nm: sz.d as f64, against: "copper shape".into(), via: true, src: ViolSrc::default() });
                }
                if !self.pair_via_ok(w, p, sz, net) {
                    vs.push(Viol { kind: "clearance", at: p, layer: u32::MAX, depth_nm: self.model.rules.net_rule(net).clearance.max(1) as f64, len_nm: sz.d as f64, against: "diff pair partner copper".into(), via: true, src: ViolSrc::default() });
                }
                if !self.via_own_pad_ok(p, sz, net) {
                    vs.push(Viol { kind: "connection_width", at: p, layer: u32::MAX, depth_nm: self.model.rules.min_connection.max(1) as f64, len_nm: sz.d as f64, against: "own-net pad".into(), via: true, src: ViolSrc::default() });
                }
                let side = if self.via_side_ok(w, k, p, plan.layers[i], plan.layers[i + 1]) { 0.0 } else { 1.0 };
                let reach = match class {
                    Some(c) => match self.via_reach_for_price(w, k, p, plan.layers[i], plan.layers[i + 1], c) {
                        (false, _) => continue,
                        (true, r) => self.reach_cost(r, 1.0),
                    },
                    None => 0.0,
                };
                let key = force_via_key(&vs, side + reach, self.opts.margin_nm.max(0) as f64);
                if dbg {
                    seen.push((key[2], viol_key(&vs).0, k, p, sz.d));
                }
                let room = self.via_room(w, k, p, sz.d as f64);
                if fdbg {
                    cands.push((key[2], k, p, sz.d, vs.iter().map(|v| format!("{} {} d{:.0} l{:.0}", v.kind, v.against, v.depth_nm / 1e3, v.len_nm / 1e3)).collect::<Vec<_>>().join("; ")));
                }
                if best.as_ref().is_none_or(|b| force_via_better(key, b.0) || !force_via_better(b.0, key) && room > b.1 + 1.0) {
                    best = Some((key, room, k, p, sz, vs));
                }
            }
        }
        if fdbg {
            cands.sort_by(|a, b| a.0.total_cmp(&b.0));
            eprintln!("FORCEVIA wire {w} net {net} via {i} class {} chosen {:?} of {} sites", class.is_some(), best.as_ref().map(|b| (b.0, b.3.x / 1e6, -b.3.y / 1e6)), cands.len());
            for c in cands.iter().take(8) {
                eprintln!("  FORCEVIA cand price {:.3} k {} ({:.4},{:.4}) d {} [{}]", c.0, c.1, c.2.x / 1e6, -c.2.y / 1e6, c.3, c.4);
            }
        }
        if dbg {
            let line = |x: &(f64, f64, usize, Vec2, i64)| format!("{:.2}/short {:.2} k{} ({:.3},{:.3}) d{}", x.0, x.1, x.2, x.3.x / 1e6, -x.3.y / 1e6, x.4);
            let mut by_price = seen.clone();
            by_price.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut by_short = seen.clone();
            by_short.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0)));
            eprintln!(
                "FORCEVIASITES wire {w} via {i} class {} sites {} short-free {} | cheapest [{}] | least short [{}]",
                class.is_some(),
                seen.len(),
                seen.iter().filter(|x| x.1 <= 0.0).count(),
                by_price.iter().take(6).map(line).collect::<Vec<_>>().join("; "),
                by_short.iter().take(4).map(line).collect::<Vec<_>>().join("; ")
            );
        }
        best.map(|b| (b.2, b.3, b.4, b.5))
    }

    /// `[vias] reach_price` (Options `via_reach_price`, default off): the price a via site pays for
    /// the violation amount `reach` of its reach ([`Drawer::via_reach`]), at `scale` per unit
    /// (`octi.room x via diameter` in the legal search, 1 in the forced search); 0 when off (the
    /// class verdict applies either way).
    pub fn reach_cost(&self, reach: f64, scale: f64) -> f64 {
        if self.opts.via_reach_price {
            scale * reach
        } else {
            0.0
        }
    }

    /// The plan's crossings of wire `w` with other nets, as `(traversal of w, other net, the other
    /// wire's layer there)` (layer -1: open there, a via interval of its own, or no pass layers
    /// known). The class test of a via site ([`Drawer::via_reach`]) reads it per traversal and
    /// layer: a crossing elsewhere on the wire, or on the other layer, does not make a drawn track
    /// crossable here. O(crossings of w).
    pub fn class_crossings(&self, w: WireId) -> ClassCrossings {
        let st = self.st;
        let net = st.wires[w].net;
        let layer = |o: WireId, ok: usize| -> i32 {
            if self.prov_layer.is_empty() {
                return -1;
            }
            let ps = &st.wires[o].passes;
            let a = if ok > 0 { self.prov_layer[ps[ok - 1]] } else { -2 };
            let b = if ok < ps.len() { self.prov_layer[ps[ok]] } else { -2 };
            match (a, b) {
                (-2, -2) => -1,
                (-2, x) | (x, -2) => x,
                (x, y) if x == y => x,
                _ => -1,
            }
        };
        st.crossings(w).iter().filter(|x| st.wires[x.other].net != net).map(|x| (x.k, st.wires[x.other].net, layer(x.other, x.other_k))).collect()
    }

    /// Whether via site `p` in traversal `k` of wire `w` keeps the plan's class, and the graded
    /// price of reaching it. The runs on the via's layers `la` and `lb` leave the plan's path in
    /// `k` (the point of [`Drawer::taut_chord`] nearest `p`) for `p`: a straight reach segment at
    /// the wire's width per layer, checked against the copper drawn so far. A reach on layer `l`
    /// that overlaps (a short) a drawn track of another net `n` goes over that wire: the site is on
    /// its far side, which changes the topological class (vias are placed homotopically, Dayan &
    /// Dai 1991, UCSC-CRL-91-40, PDF p. 1; a rubber band's ends are terminals, vias or junctions,
    /// Dayan 1997 PDF p. 29 = printed p. 18), so it is out of class unless the plan crosses `w`
    /// with net `n` in this traversal `k` on that layer `l` (or there with `n`'s layer open):
    /// `crossed` = [`Drawer::class_crossings`]. A crossing elsewhere on the wire or on the other
    /// layer does not count (qmtech JS_INTB_5V: crossed with VIN5V elsewhere, the site past
    /// VIN5V's F.Cu track in its via traversal was judged in class).
    /// Price: the violation amounts of both reach segments ([`Viol::amount`], graded; 0 for a free
    /// reach). O(chord + objects nearby).
    pub fn via_reach(&self, w: WireId, k: usize, p: Vec2, la: usize, lb: usize, crossed: &ClassCrossings) -> (bool, f64) {
        self.via_reach_(w, k, p, la, lb, crossed, true)
    }

    /// nd-perf F2: the class verdict of [`Drawer::via_reach`] without its price (the reach
    /// segments' violations are not measured). O(chord + objects nearby).
    pub fn via_in_class(&self, w: WireId, k: usize, p: Vec2, la: usize, lb: usize, crossed: &ClassCrossings) -> bool {
        self.via_reach_(w, k, p, la, lb, crossed, false).0
    }

    /// nd-perf F2: [`Drawer::via_reach`] where its price is read ([`reach_priced`]: `[vias]
    /// reach_price` on), else its verdict and 0 (the price `reach_cost` would give then).
    pub fn via_reach_for_price(&self, w: WireId, k: usize, p: Vec2, la: usize, lb: usize, crossed: &ClassCrossings) -> (bool, f64) {
        if reach_priced(self.opts) {
            self.via_reach(w, k, p, la, lb, crossed)
        } else {
            (self.via_in_class(w, k, p, la, lb, crossed), 0.0)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn via_reach_(&self, w: WireId, k: usize, p: Vec2, la: usize, lb: usize, crossed: &ClassCrossings, priced: bool) -> (bool, f64) {
        let net = self.st.wires[w].net;
        let chord = self.taut_chord(w, k);
        let q = match chord.len() {
            0 => return (true, 0.0),
            1 => chord[0],
            _ => chord
                .windows(2)
                .map(|x| {
                    let d = x[1] - x[0];
                    let l2 = d.len2();
                    let t = if l2 > 0.0 { ((p - x[0]).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
                    x[0] + d * t
                })
                .min_by(|a, b| (*a - p).len2().total_cmp(&(*b - p).len2()))
                .unwrap_or(chord[0]),
        };
        if (p - q).len2() <= 1.0 {
            return (true, 0.0);
        }
        let (mut ok, mut price) = (true, 0.0);
        for (j, &l) in [la, lb].iter().enumerate() {
            if j == 1 && lb == la {
                break;
            }
            let items = self.run_items(w, l, &[q, p]);
            let rep = self.checker.check_against(&self.idx, &items, &[]);
            for v in &rep.violations {
                let other = v.against.strip_prefix("item:").and_then(|x| x.parse::<usize>().ok()).and_then(|id| self.idx.net_of(id));
                if let Some((n2, false)) = other {
                    if n2 != net && !crossed.contains(&(k, n2, l as i32)) && !crossed.contains(&(k, n2, -1)) && v.gap <= 0 {
                        ok = false;
                    }
                }
            }
            // nd-perf F2: the class verdict above reads the report alone.
            if priced {
                price += self.viols_of(&items, &rep).iter().map(Viol::amount).sum::<f64>();
            }
        }
        (ok, price)
    }

    /// The via site `p` in traversal `k` of wire `w` (run layers `la` before the via, `lb` after)
    /// lies on the side the topology gives it of every other wire's chord through that triangle
    /// on `la` or `lb`: the via sits on `w`'s chord, so it is on the side of `w`'s exit point
    /// (for a chord on `lb`) or entry point (on `la`). Chords are the nominal ones (evenly spread
    /// pass points and turn corners, [`State::polyline`]); a chord whose layer is still open (a
    /// via interval of its own) or of the same net is no constraint. A via on the wrong side
    /// leaves its run on a layer no way to its first pass but across that wire (wiccon MOSI,
    /// D2). O(traversals of the triangle x chord corners).
    pub fn via_side_ok(&self, w: WireId, k: usize, p: Vec2, la: usize, lb: usize) -> bool {
        self.via_side_ok_(w, k, p, la, lb, false)
    }

    /// [`Drawer::via_side_ok`] on the plan's taut geometry (every wire's chord the taut path
    /// [`Packer::taut`] that the via relocation balanced the via on), not the drawing's packed
    /// nominal chords. A planned site that holds here keeps the plan's crossing order (the
    /// partners are judged by the drawing's checker).
    pub fn via_side_ok_taut(&self, w: WireId, k: usize, p: Vec2, la: usize, lb: usize) -> bool {
        self.via_side_ok_(w, k, p, la, lb, true)
    }

    /// Wire `o`'s packed nominal chord through traversal `ok`: [`Drawer::chord_pt`] `ok`, its turn
    /// points there, `ok + 1` (the plan's path; the side test, the via walls, the taut pull's
    /// guards and the round-i class test all read it). O(turns).
    fn plan_chord(&self, o: WireId, ok: usize) -> Vec<Vec2> {
        let mut c = vec![self.chord_pt(o, ok)];
        c.extend(self.st.wires[o].turns.iter().filter(|x| x.0 == ok).map(|x| self.b.proj.pos[x.1]));
        c.push(self.chord_pt(o, ok + 1));
        c
    }

    /// exist-term r4: whether via `i` of wire `w` (first traversal `ka` of its class) is the
    /// plan's layer change at a continued dangling end's lead end, and that end (0 / 1) with the
    /// lead's via end ([`super::board::KeptEndOut::via_end`]). At the lead end = the wire's first
    /// via with its pass interval starting at the terminal (`ka` 0: the layer may change before
    /// the first pass), or its last with the interval reaching the end (after the last pass):
    /// topologically the via may sit anywhere there, the via end included (the drawing keeps the
    /// plan's interval, like the planned site's test). O(terminals).
    fn lead_via_site(&self, w: WireId, i: usize, ka: usize, plan: &Plan) -> Option<(usize, (Vec2, ViaSize))> {
        let wi = &self.st.wires[w];
        let last = plan.vias.len().checked_sub(1)?;
        for (end, ob) in [(0usize, wi.pa), (1, wi.pb)] {
            // the layer change before the wire's first pass / after its last (no stretch of the
            // plan's topology on the lead's layer moves to the other)
            let n = wi.passes.len();
            if (end == 0 && (i != 0 || ka != 0)) || (end == 1 && (i != last || plan.vias[i].1 < n)) || ob == super::state::NO_OBS {
                continue;
            }
            let Some(k) = self.b.kept_ends.iter().find(|k| k.obs == ob) else { continue };
            if let (Some(_), Some(ve)) = (k.lead, k.via_end) {
                // tw4 T3: a wire that starts short of the lead's end has its via end only where
                // it lies on the part of the lead it draws
                let p = self.b.proj.pos[if end == 0 { wi.a } else { wi.b }];
                if !k.choices.is_empty() && k.lead_point_t(&self.b.proj, p).is_some_and(|t| (ve.0 - k.at).len() > t + 1.0) {
                    continue;
                }
                return Some((end, ve));
            }
        }
        None
    }

    /// Point `j` of wire `w`'s chord polyline (0: its start terminal, `n + 1`: its end, else
    /// the crossing of pass `j - 1`): the packed nominal crossing once the edges are packed
    /// (where the drawing and the via sites put it, D3), else the evenly spread pass point of
    /// [`State::polyline`].
    fn chord_pt(&self, w: WireId, j: usize) -> Vec2 {
        let wi = &self.st.wires[w];
        let n = wi.passes.len();
        if j == 0 {
            return self.b.proj.pos[wi.a];
        }
        if j > n {
            return self.b.proj.pos[wi.b];
        }
        match self.iv.nom.get(wi.passes[j - 1]).copied().filter(|f| f.is_finite()) {
            Some(f) => self.portal(w, j - 1, f, f).0,
            None => self.polys[w][j],
        }
    }

    fn via_side_ok_(&self, w: WireId, k: usize, p: Vec2, la: usize, lb: usize, taut: bool) -> bool {
        if self.prov_layer.is_empty() {
            return true;
        }
        let st = self.st;
        let wi = &st.wires[w];
        let t = wi.tris[k];
        // draw-via-neck A: a site outside its traversal's triangle (an offset or shifted site
        // labelled with the nearest traversal) leaves its runs through the neighbouring
        // triangles: the chords there are tested too (the via never lands past a crossing the
        // plan put after it, so the layer change keeps the class). A site inside is tested in
        // its own triangle alone, as before. Only with `via_shift_mm` > 0 (r5, verifier F5: the
        // test ran ungated and changed main's copper on topo_thick_layer's thick_dive_grouping).
        let pr = &self.b.proj;
        let tris: Vec<super::state::TriId> = if self.opts.via_shift_mm <= 0.0 || super::state::point_in_poly(p, &pr.tris[t].map(|v| pr.pos[v])) {
            vec![t]
        } else {
            let n = wi.tris.len();
            let mut v: Vec<super::state::TriId> = (k.saturating_sub(2)..=(k + 2).min(n - 1)).map(|j| wi.tris[j]).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let chord_layer = |o: WireId, ok: usize| -> i32 {
            let ps = &st.wires[o].passes;
            let a = if ok > 0 { self.prov_layer[ps[ok - 1]] } else { -2 };
            let b = if ok < ps.len() { self.prov_layer[ps[ok]] } else { -2 };
            match (a, b) {
                (-2, x) | (x, -2) => x,
                (x, y) if x == y => x,
                _ => -1,
            }
        };
        for &(o, ok) in tris.iter().flat_map(|&t| st.tri_trav[t].iter()) {
            let oi = &st.wires[o];
            if o == w || !oi.alive || oi.net == wi.net || self.polys[o].is_empty() {
                continue;
            }
            let lo = chord_layer(o, ok);
            let own = if taut { self.taut_chord(w, k) } else { Vec::new() };
            let r = if lo == lb as i32 {
                own.last().copied().unwrap_or_else(|| self.chord_pt(w, k + 1))
            } else if lo == la as i32 {
                own.first().copied().unwrap_or_else(|| self.chord_pt(w, k))
            } else {
                continue;
            };
            let chord = if taut && self.taut_chord(o, ok).len() >= 2 {
                self.taut_chord(o, ok)
            } else {
                self.plan_chord(o, ok)
            };
            let crossings = chord.windows(2).filter(|c| seg_proper(r, p, c[0], c[1])).count();
            if crossings % 2 == 1 {
                return false;
            }
        }
        true
    }

    /// Walls that keep a run of wire `w` (traversals `k0..=k1`, on `layer`) on the topology's
    /// side of the vias placed by other nets in its triangles: the chord of the via's wire on
    /// `layer`, from the via centre to its pass point (the nominal one of its interval) on the
    /// edge it leaves through (layer after the via) or enters through (layer before), or to its
    /// terminal. The run crosses no such chord in the topology; a via is a point, so without the
    /// wall the run may pass it on either side and take the room its wire needs (wiccon MOSI,
    /// D2: the 3.3v wire ran between the via and the pad corner the via's wire had to pass).
    /// Only until the via's wire is drawn. O(vias).
    pub fn via_walls(&self, w: WireId, k0: usize, k1: usize, layer: usize) -> Vec<(Vec2, Vec2)> {
        let (st, pr) = (self.st, &self.b.proj);
        let wi = &st.wires[w];
        let tris: Vec<TriId> = wi.tris[k0..=k1].to_vec();
        let mut out = Vec::new();
        for &(v, k, p, la, lb, h) in &self.via_marks {
            let vi = &st.wires[v];
            if v == w || vi.net == wi.net || !tris.contains(&vi.tris[k]) {
                continue;
            }
            let (_, ent) = self.handles[h];
            if !self.ents.get(ent).is_some_and(|e| e.live) {
                continue;
            }
            // Once the via's wire is drawn its copper is the obstacle (it may run off the
            // nominal chord); the wall stands for it only until then.
            if self.drawn_once[v] {
                continue;
            }
            let n = vi.passes.len();
            let end = if layer == lb {
                if k < n {
                    let f = self.iv.nom[vi.passes[k]];
                    if !f.is_finite() {
                        continue;
                    }
                    self.portal(v, k, f, f).0
                } else {
                    pr.pos[vi.b]
                }
            } else if layer == la {
                if k > 0 {
                    let f = self.iv.nom[vi.passes[k - 1]];
                    if !f.is_finite() {
                        continue;
                    }
                    self.portal(v, k - 1, f, f).0
                } else {
                    pr.pos[vi.a]
                }
            } else {
                continue;
            };
            // 10 um past the edge, so a run along the edge through the pass point crosses it.
            let past = (end - p).unit().map_or(end, |u| end + u * 10_000.0);
            out.push((p, past));
        }
        out
    }

    /// The violations of `items` (as the checker reported them in `rep`, against the indexed
    /// copper) as [`Viol`]s: depth = required minus gap, length = the part of the track whose
    /// copper is below the rule against that object (probed every 0.5 mm, then every 50 um around the hits; a via:
    /// its diameter). O(violations x samples x checker query).
    pub fn viols_of(&self, items: &[Item], rep: &CheckReport) -> Vec<Viol> {
        viols_with(self.model, self.opts, self.checker, &self.idx, items, rep)
    }

    /// draw-absorb A0: give each of `vs` (about to be written) the provenance `base` and what its
    /// copper hits: the router wire owning the indexed item it names (`item:N`, live now), whether
    /// that is a band the joint commit reserved, and that wire's packed room away from the
    /// violation ([`Drawer::slack_away_near`]). Measurement only. O(violations x handles).
    pub(super) fn tag_viols(&self, vs: &mut [Viol], base: &ViolSrc) {
        for v in vs.iter_mut() {
            let mut s = base.clone();
            if let Some((hw, reserved)) = against_item(&v.against).and_then(|id| self.wire_of_item(id)) {
                s.hit_wire = Some(hw);
                s.hit_reserved = reserved;
                s.hit_slack_nm = self.slack_away_near(hw, v.at);
            }
            v.src = s;
        }
    }

    /// The router wire of the live indexed item `id` and whether it is a reserved band (its wire
    /// not drawn yet). The index reuses freed ids: the live handle holding `id` is the one.
    /// O(handles).
    fn wire_of_item(&self, id: usize) -> Option<(WireId, bool)> {
        let h = (0..self.handles.len()).rev().find(|&h| self.handles[h].0 == id && self.ents.get(self.handles[h].1).is_some_and(|e| e.live))?;
        let w = self.ent_wire.get(&self.handles[h].1).copied().or_else(|| self.joint.band_ids.get(&id).map(|x| x.0))?;
        Some((w, self.joint.owner.contains_key(&h)))
    }

    /// draw-absorb A0: the packed room (nm) of wire `o` on the side away from `p`, at its pass
    /// whose packed lane point (`iv.nom`) lies nearest to `p` (the room a re-taut of `o` inside its
    /// sleeve could give, [`Drawer::pass_slack_away`] over all its passes); `None` when no pass of
    /// `o` is packed. O(passes of `o`).
    fn slack_away_near(&self, o: WireId, p: Vec2) -> Option<f64> {
        let (st, pr) = (self.st, &self.b.proj);
        let mut best: Option<(f64, f64)> = None;
        for &pid in st.wires.get(o)?.passes.iter() {
            let (Some(&lo), Some(&hi), Some(&nom)) = (self.iv.lo.get(pid), self.iv.hi.get(pid), self.iv.nom.get(pid)) else { continue };
            if !(lo.is_finite() && hi.is_finite() && nom.is_finite()) {
                continue;
            }
            let [a, b] = pr.edges[st.passes[pid].edge];
            let (a, b) = (pr.pos[a], pr.pos[b]);
            let len = (b - a).len();
            if len < 1.0 {
                continue;
            }
            let lane = a + (b - a) * nom;
            let slack = if (p - lane).dot(b - a) > 0.0 { (nom - lo) * len } else { (hi - nom) * len };
            let d = (p - lane).len();
            if best.is_none_or(|x| d < x.0) {
                best = Some((d, slack));
            }
        }
        best.map(|x| x.1)
    }

    /// draw-absorb A0: the provenance of a run's violations of wire `w` (run `r`, band status
    /// `cut`, the plan neck `pneck`) written by `mech` from candidate `cand` with octilinear
    /// outcome `form`; `items` = the run's copper (its narrowest track).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_src(&self, w: WireId, mech: &'static str, cand: &'static str, form: &'static str, cut: &'static str, r: usize, wrong_side: bool, plan_neck: bool, items: &[Item]) -> ViolSrc {
        let wi = &self.st.wires[w];
        let drawn = items.iter().filter_map(|i| match i {
            Item::Track(t) => Some(t.width as f64),
            _ => None,
        });
        ViolSrc {
            mech,
            cand,
            form,
            cut,
            run: r,
            wrong_side,
            plan_neck,
            w_nm: wi.w,
            w_min_nm: self.st.w_min(wi.net, wi.w),
            drawn_w_nm: drawn.fold(f64::NAN, f64::min).max(0.0),
            ..ViolSrc::default()
        }
    }
}

/// The index id an `against` label names (`item:N`, or `... (item:N)` as [`viols_measured`] names
/// routed copper).
pub fn against_item(against: &str) -> Option<usize> {
    let t = &against[against.find("item:")? + 5..];
    let n = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    t[..n].parse().ok()
}

/// draw-absorb A0: the label of forced candidate from ladder attempt `a` (-1 = the band, 6 = the
/// kept ERBS, 0..5 the ladder).
pub fn attempt_label(a: isize) -> &'static str {
    match a {
        ..0 => "erbs",
        0 => "attempt0",
        1 => "attempt1",
        2 => "attempt2",
        3 => "attempt3",
        4 => "attempt4",
        5 => "attempt5",
        _ => "erbs_kept",
    }
}

/// [`Drawer::viols_of`] for any checker and index of accepted copper (D7: the realised diff-pair
/// tracks are checked against the real model, not the drawing's alias model).
pub fn viols_with(model: &BoardModel, opts: &Options, checker: &Checker, idx: &ItemIndex, items: &[Item], rep: &CheckReport) -> Vec<Viol> {
    viols_measured(model, opts, checker, idx, items, rep, true)
}

/// nd-perf F1: [`viols_with`] without the probes (`probe = false`): every track violation's length at
/// the floor the measured one is raised to (`tw.min(len.max(tw))`), everything else (kind, depth,
/// grouping by [`merge_viols`], via lengths) as measured. Each length is at most the measured one
/// and [`super::factors::violation_amount`] does not fall with the length, so every component of
/// [`viol_key`] (and the price) of the floor violations is at most the measured one, also in
/// floating point (the same terms in the same order, each no larger). O(violations) without
/// probes; with them see [`Drawer::viols_of`].
pub fn viols_measured(model: &BoardModel, _opts: &Options, checker: &Checker, idx: &ItemIndex, items: &[Item], rep: &CheckReport, probe: bool) -> Vec<Viol> {
    {
        // Routed copper (`item:N`) is named by its net: "track of SIG (item:12)".
        let name = |against: &str| -> String {
            let Some(id) = against.strip_prefix("item:").and_then(|x| x.parse::<usize>().ok()) else { return against.to_string() };
            match idx.net_of(id) {
                Some((net, via)) => format!(
                    "{} of {} ({against})",
                    if via { "via" } else { "track" },
                    model.nets.iter().find(|n| n.code == net).map_or(net.to_string(), |n| n.name.clone())
                ),
                None => against.to_string(),
            }
        };
        let mut out = Vec::new();
        for v in &rep.violations {
            let Some(item) = items.get(v.item) else { continue };
            let depth = (v.required - v.gap).max(1) as f64;
            let kind = |via: bool| viol_kind(v, via);
            match item {
                Item::Via(x) => out.push(Viol {
                    kind: kind(true),
                    at: Vec2 { x: x.at.x as f64, y: -(x.at.y as f64) },
                    layer: v.layer,
                    depth_nm: depth,
                    len_nm: x.d as f64,
                    against: name(&v.against),
                    via: true,
                    src: ViolSrc::default(),
                }),
                Item::Track(t) => {
                    let TrackPrim::Seg { a, b } = t.prim else { continue };
                    let (a, b) = (Vec2 { x: f64::from(a.x), y: -f64::from(a.y) }, Vec2 { x: f64::from(b.x), y: -f64::from(b.y) });
                    let len = (b - a).len();
                    // Probes along the segment: which parameters `t` in [0, 1] are below the
                    // rule against this object. Coarse (every 0.5 mm, at most 48), then every
                    // 50 um around the coarse hits (or along the whole segment when none hit:
                    // a violation shorter than the coarse step).
                    let below = |ts: &[f64]| -> Vec<bool> {
                        let probes: Vec<Item> = ts.iter().map(|&u| track_item(t.net, t.layer, t.width as f64, a + (b - a) * u, a + (b - a) * u, None)).collect();
                        let pr = checker.check_against(idx, &probes, &[]);
                        (0..ts.len()).map(|i| pr.violations.iter().any(|x| x.item == i && x.against == v.against && x.layer == v.layer)).collect()
                    };
                    let grid = |lo: f64, hi: f64, step: f64, cap: usize| -> Vec<f64> {
                        let n = (((hi - lo) * len / step).ceil() as usize + 1).clamp(2, cap);
                        (0..n).map(|i| lo + (hi - lo) * i as f64 / (n - 1) as f64).collect()
                    };
                    if !probe {
                        let tw = t.width as f64;
                        out.push(Viol { kind: kind(false), at: (a + b) * 0.5, layer: v.layer, depth_nm: depth, len_nm: tw.min(len.max(tw)), against: name(&v.against), via: false, src: ViolSrc::default() });
                        continue;
                    }
                    let coarse = grid(0.0, 1.0, 500_000.0, 48);
                    let hc = below(&coarse);
                    let dt = 1.0 / (coarse.len() - 1) as f64;
                    let mut fine: Vec<f64> = Vec::new();
                    if hc.iter().any(|&h| h) {
                        for (i, &h) in hc.iter().enumerate() {
                            if h {
                                let (lo, hi) = ((coarse[i] - dt).max(0.0), (coarse[i] + dt).min(1.0));
                                fine.extend(grid(lo, hi, 50_000.0, 40));
                            }
                        }
                        fine.sort_by(f64::total_cmp);
                        fine.dedup_by(|x, y| (*x - *y).abs() * len < 1_000.0);
                    } else {
                        fine = grid(0.0, 1.0, 50_000.0, 400);
                    }
                    let hf = below(&fine);
                    // Length below the rule: the fine samples that hit, each standing for the
                    // stretch to its neighbours (half the gap on either side).
                    let mut blen = 0.0;
                    let (mut sum, mut nhit) = (Vec2::default(), 0usize);
                    for i in 0..fine.len() {
                        if hf[i] {
                            let l = if i > 0 { 0.5 * (fine[i] - fine[i - 1]) } else { 0.0 };
                            let r = if i + 1 < fine.len() { 0.5 * (fine[i + 1] - fine[i]) } else { 0.0 };
                            blen += (l + r) * len;
                            sum = sum + (a + (b - a) * fine[i]);
                            nhit += 1;
                        }
                    }
                    let at = if nhit == 0 { (a + b) * 0.5 } else { sum * (1.0 / nhit as f64) };
                    let tw = t.width as f64;
                    let blen = blen.max(tw.min(len.max(tw)));
                    out.push(Viol {
                        kind: kind(false),
                        at,
                        layer: v.layer,
                        depth_nm: depth,
                        len_nm: blen,
                        against: name(&v.against),
                        via: false,
                        src: ViolSrc::default(),
                    });
                }
            }
        }
        merge_viols(out)
    }
}

/// Debug (`TOPO_RUNPROV`, forced-draw): one stderr line per committed run of the drawing: the
/// writer (`ladder` = the first-draw ladder, `forced` = never drop), its source, the forced run's
/// octilinear outcome, its free-angle stretches and its points (mm, KiCad frame). Read only.
fn run_prov(kind: &str, w: WireId, net: i32, layer: usize, src: impl std::fmt::Display, tag: &str, pts: &[Vec2]) {
    if std::env::var_os("TOPO_RUNPROV").is_none() {
        return;
    }
    let any = super::octi::breaks(pts).0;
    let mm = pts.iter().map(|p| format!("({:.6}, {:.6})", p.x / 1e6, -p.y / 1e6)).collect::<Vec<_>>().join(", ");
    eprintln!("RUNPROV {kind} w{w} net {net} L{layer} src {src} octi {tag} any {} {:.3} pts [{mm}]", any.len(), any.iter().map(|a| a.2).sum::<f64>() / 1e6);
}

/// nd-perf F2: `via_reach` prices its reach segments only when the price is read: `[vias]
/// reach_price` on (`reach_cost` is 0 while it is off), or the `TOPO_VIADBG` line that prints it.
pub fn reach_priced(opts: &Options) -> bool {
    opts.via_reach_price || std::env::var_os("TOPO_VIADBG").is_some()
}

/// The key a forced candidate is chosen by: `(short amount, total amount, price)` ([`viol_key`]).
pub type ForcedKey = (f64, f64, f64);

/// The order of [`ForcedKey`]s: shorts first, then the total, then the price (`total_cmp`).
pub fn forced_key_cmp(a: &ForcedKey, b: &ForcedKey) -> std::cmp::Ordering {
    a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.total_cmp(&b.2))
}

/// nd-perf F1: the candidate of least exact key, the first index on ties (what `min_by` over the
/// exact keys returns), where `lbs[j]` is a lower bound of candidate `j`'s key and `exact(j)` its
/// exact key (computed at most once per `j`, only when the bound cannot rule `j` out). The
/// candidates are visited in ascending bound order (index on ties); the visit stops at the first
/// bound above the best exact key (every later one is at least as large, so it cannot win), and a
/// bound equal to it at a larger index is skipped (equal at best, it loses the tie). A branch and
/// bound over a finite set: the answer is the exhaustive one whenever `lbs[j] <= exact(j)`.
/// O(n log n) plus the exact keys it needs.
pub fn pick_by_bound(lbs: &[ForcedKey], mut exact: impl FnMut(usize) -> ForcedKey) -> Option<usize> {
    use std::cmp::Ordering;
    let mut order: Vec<usize> = (0..lbs.len()).collect();
    order.sort_by(|&a, &b| forced_key_cmp(&lbs[a], &lbs[b]).then(a.cmp(&b)));
    let mut best: Option<(ForcedKey, usize)> = None;
    for j in order {
        if let Some((bk, bi)) = best {
            match forced_key_cmp(&lbs[j], &bk) {
                Ordering::Greater => break,
                Ordering::Equal if j > bi => continue,
                _ => {}
            }
        }
        let k = exact(j);
        if best.is_none_or(|(bk, bi)| match forced_key_cmp(&k, &bk) {
            Ordering::Less => true,
            Ordering::Equal => j < bi,
            Ordering::Greater => false,
        }) {
            best = Some((k, j));
        }
    }
    best.map(|b| b.1)
}

/// One forced candidate of a run (never drop, M-B1): price, points, items, violations, pad neck,
/// source, and (nd-perf F1) the checker report while it is not priced yet: then the violations
/// hold only its graze violations and the price is NaN.
pub(super) type ForcedCand = (f64, Vec<Vec2>, Vec<Item>, Vec<Viol>, Option<(f64, [f64; 2])>, &'static str, Option<CheckReport>);

pub(super) fn forced_key(vs: &[Viol], price: f64) -> ForcedKey {
    let k = viol_key(vs);
    (k.0, k.1, price)
}

impl<'a> Drawer<'a> {
    /// nd-perf F1: a forced candidate not priced yet: its graze violations (exact and cheap) and its
    /// checker report, measured by [`Drawer::price_cand`] only when the selection needs it.
    pub(super) fn lazy_cand(&self, w: WireId, items: &[Item], rep: &CheckReport) -> (f64, Vec<Viol>, Option<CheckReport>) {
        (f64::NAN, self.graze_viols(w, items), Some(rep.clone()))
    }

    /// Price a lazy forced candidate: its measured violations, then its graze ones (the order the
    /// eager pricing had), and their price. A priced candidate stays as it is.
    pub(super) fn price_cand(&self, c: &mut ForcedCand) {
        if let Some(rep) = c.6.take() {
            let mut vs = self.viols_of(&c.2, &rep);
            vs.append(&mut c.3);
            c.0 = vs.iter().map(Viol::amount).sum();
            c.3 = vs;
        }
    }

    /// The lower bound of a forced candidate's key ([`viols_measured`] without probes, plus its
    /// graze violations); a priced candidate's exact key.
    pub(super) fn cand_bound(&self, c: &ForcedCand) -> ForcedKey {
        match &c.6 {
            None => forced_key(&c.3, c.0),
            Some(rep) => {
                let mut v = viols_measured(self.model, self.opts, self.checker, &self.idx, &c.2, rep, false);
                v.extend(c.3.iter().cloned());
                let p = v.iter().map(Viol::amount).sum();
                forced_key(&v, p)
            }
        }
    }

    /// nd-perf F1: the forced candidate to draw (least [`ForcedKey`], the first on ties), pricing
    /// only the candidates whose bound does not rule them out ([`pick_by_bound`]); the winner is
    /// priced. `TOPO_FORCED_CHECK=1` (debug) prices every candidate too and reports on stderr
    /// whether the exhaustive choice is the same and every bound held.
    fn pick_forced(&self, w: WireId, cands: &mut [ForcedCand]) -> Option<usize> {
        let lbs: Vec<ForcedKey> = cands.iter().map(|c| self.cand_bound(c)).collect();
        let pick = pick_by_bound(&lbs, |j| {
            self.price_cand(&mut cands[j]);
            forced_key(&cands[j].3, cands[j].0)
        });
        debug_assert!(pick.is_none_or(|j| forced_key_cmp(&lbs[j], &forced_key(&cands[j].3, cands[j].0)).is_le()), "nd-perf F1: a forced candidate's bound above its price");
        if std::env::var_os("TOPO_FORCED_CHECK").is_some() {
            let mut all: Vec<ForcedCand> = cands.to_vec();
            for c in all.iter_mut() {
                self.price_cand(c);
            }
            let keys: Vec<ForcedKey> = all.iter().map(|c| forced_key(&c.3, c.0)).collect();
            let old = (0..all.len()).min_by(|&a, &b| forced_key_cmp(&keys[a], &keys[b]));
            let same = old == pick;
            let held = (0..all.len()).all(|j| forced_key_cmp(&lbs[j], &keys[j]).is_le());
            eprintln!("FORCED CHECK wire {w}: {} candidates, {} priced, pick {pick:?} exhaustive {old:?}: {}, bounds {}", all.len(), cands.iter().filter(|c| c.6.is_none()).count(), if same { "same" } else { "MISMATCH" }, if held { "held" } else { "BROKEN" });
        }
        pick
    }

    /// The sliver connections of a run's `items` against pads and vias of its own net (what
    /// [`Drawer::track_own_pad_ok`] refuses), as `connection_width` [`Viol`]s: one per track that
    /// grazes, depth = KiCad's minimum connection width.
    pub(super) fn graze_viols(&self, w: WireId, items: &[Item]) -> Vec<Viol> {
        let vs = items
            .iter()
            .filter(|it| matches!(it, Item::Track(_)) && !self.track_own_pad_ok(w, std::slice::from_ref(*it)))
            .filter_map(|it| {
                let Item::Track(t) = it else { return None };
                let TrackPrim::Seg { a, b } = t.prim else { return None };
                let (a, b) = (Vec2 { x: f64::from(a.x), y: -f64::from(a.y) }, Vec2 { x: f64::from(b.x), y: -f64::from(b.y) });
                Some(Viol {
                    kind: "connection_width",
                    at: (a + b) * 0.5,
                    layer: t.layer,
                    depth_nm: self.model.rules.min_connection.max(1) as f64,
                    len_nm: t.width as f64,
                    against: "own-net pad or via".into(),
                    via: false,
                    src: ViolSrc::default(),
                })
            })
            .collect();
        merge_viols(vs)
    }

    /// `item` violates nothing on the board and against the indexed copper.
    pub fn check_ok(&self, item: &Item) -> bool {
        self.checker.check_against(&self.idx, std::slice::from_ref(item), &[]).violations.is_empty()
    }

    /// The drill of a via of `size` at `p` keeps the hole clearance to every foreign copper polygon
    /// (pads, copper shapes): the checker does not test a drill against a no-net shape.
    pub fn via_hole_ok(&self, p: Vec2, size: ViaSize, net: i32) -> bool {
        let hole = 0.5 * size.drill as f64 + self.model.rules.via_hole_clear() as f64 + self.eps;
        let mut ids = self.grid.query(bbox_of(&[p]).grow(hole + 0.02e6 * 100.0));
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter().all(|id| {
            let e = &self.ents[id];
            if !e.live || e.net == Some(net) {
                return true;
            }
            match &e.core {
                Core::Poly(_) if e.shape => seg_core(p, p, &e.core).0 >= e.grow + hole,
                _ => true,
            }
        })
    }

    /// A via of `size` at `p` does not overlap a pad of its own net only partly (D1): the overlap
    /// of the via disc with the pad copper is a lens whose chord is the narrowest copper between
    /// them, and KiCad's minimum connection width check flags it (seen on a 2-layer badge: a 0.6 mm
    /// via 4 um into a battery holder pad, "actual 0.1044 mm" < 0.127 mm). Clear of the pad (gap >= 0) or
    /// centred inside it is fine; with no minimum connection width set, anything goes.
    /// O(pads of the net x pad vertices).
    pub fn via_own_pad_ok(&self, p: Vec2, size: ViaSize, net: i32) -> bool {
        if self.model.rules.pair_disc(net) && !self.pair_disc_clear(p, size, net, None) {
            return false;
        }
        if self.model.rules.min_connection <= 0 || net == 0 {
            return true;
        }
        let r = 0.5 * size.d as f64;
        // KiCad's pad polygon reaches up to `max_error` past the arcs: a via that clears the
        // exact copper by less still touches it in a sliver (a capacitor pad, D6a: 6 um).
        let tol = self.model.rules.max_error.max(0) as f64;
        self.model.pads.iter().filter(|pad| pad.net == net).all(|pad| {
            pad.layers.iter().all(|&l| {
                let d = crate::geom::point_skeleton(p, &self.model.skeleton(pad, l));
                d >= r + tol || d <= 0.0
            })
        })
    }

    /// A pair's two-via disc of `size` at `p` (lead `net` of the planning model,
    /// [`crate::model::NetRule::pair_disc`]) keeps the net's clearance to the copper of its own
    /// net there: the static pads, copper shapes, kept copper and vias of both halves (the
    /// partner's carry the lead's net in the plan; the disc holds the N via as well as the P via,
    /// so none of it is own copper for it), and with `wire` (the wire the disc belongs to) the
    /// copper drawn so far of the pair's other wires (their tracks and discs: the other
    /// connections of the pair, own net in the plan, are partner copper in the drawing too).
    /// User 2026-11-13. O(objects nearby).
    pub fn pair_disc_clear(&self, p: Vec2, size: ViaSize, net: i32, wire: Option<WireId>) -> bool {
        let own = self.model.rules.net_rule(net).clearance as f64;
        let r = 0.5 * size.d as f64;
        let mut ids = self.grid.query(bbox_of(&[p]).grow(r + own + 2.0e6));
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter().all(|id| {
            let e = &self.ents[id];
            if !e.live || e.net != Some(net) {
                return true;
            }
            // drawn copper: only with the disc's wire known, and only the pair's other wires'
            if id >= self.n_static && (wire.is_none() || self.ent_wire.get(&id).copied() == wire) {
                return true;
            }
            let dist = match &e.core {
                Core::Poly(q) => {
                    if super::state::point_in_poly(p, q) {
                        return false;
                    }
                    poly_dist(p, q)
                }
                Core::Seg(a, b) => seg_dist(p, *a, *b),
                Core::Pt(q) => (p - *q).len(),
            };
            dist - e.grow - r >= e.clear.max(own) - self.eps
        })
    }

    /// Diagnosis (`TOPO_PAIRVIADBG`): how via `i` of a pair wire was placed. O(1).
    fn pair_via_dbg(&self, w: WireId, i: usize, how: &str, p: Vec2, sz: ViaSize, viols: usize) {
        let net = self.st.wires[w].net;
        if self.model.rules.pair_disc(net) && std::env::var("TOPO_PAIRVIADBG").is_ok() {
            let planned = self.via_site.get(&(w, i)).map(|s| format!("({:.3},{:.3})", s.1.x / 1e6, -s.1.y / 1e6)).unwrap_or_else(|| "none".into());
            eprintln!("PAIRVIA wire {w} net {net} via {i} {how} at ({:.3},{:.3}) d {} planned site {planned} violations {viols}", p.x / 1e6, -p.y / 1e6, sz.d);
        }
    }

    /// [`Drawer::pair_disc_clear`] of wire `w`'s disc at `p` against the static and the drawn copper;
    /// true for a via that is no pair disc. O(objects nearby).
    pub fn pair_via_ok(&self, w: WireId, p: Vec2, size: ViaSize, net: i32) -> bool {
        !self.model.rules.pair_disc(net) || self.pair_disc_clear(p, size, net, Some(w))
    }

    /// No track of `items` (a run of wire `w`) grazes a pad of its own net other than its end
    /// pads: either it keeps KiCad's `max_error` off the pad (KiCad's minimum connection width
    /// check builds the pad polygon up to that far outside the arcs) or its centre line runs
    /// into the pad. A graze is a sliver connection KiCad flags (a resistor pad, D2: a track
    /// 1.7 um off the corner arc, "actual 0.0023 mm" < 0.127 mm). Without a minimum connection
    /// width set, anything goes. O(tracks x pads of the net).
    pub fn track_own_pad_ok(&self, w: WireId, items: &[Item]) -> bool {
        let wi = &self.st.wires[w];
        if self.model.rules.min_connection <= 0 || wi.net == 0 {
            return true;
        }
        let pad_of = |o: usize| if o == super::state::NO_OBS { usize::MAX } else { self.b.pad_of[o] };
        let ends = [pad_of(wi.pa), pad_of(wi.pb)];
        let tol = self.model.rules.max_error as f64;
        let pads: Vec<usize> = (0..self.model.pads.len()).filter(|&i| self.model.pads[i].net == wi.net && !ends.contains(&i)).collect();
        // The vias other wires of the net placed (D5: a tree hub pad brings them close): a track
        // grazing one is the same sliver connection.
        let vias: Vec<(Vec2, f64)> = self
            .via_marks
            .iter()
            .filter(|m| m.0 != w && self.st.wires[m.0].net == wi.net)
            .filter_map(|m| self.handles.get(m.5).map(|h| &self.ents[h.1]).filter(|e| e.live).map(|e| (m.2, e.grow)))
            .collect();
        if pads.is_empty() && vias.is_empty() {
            return true;
        }
        items.iter().all(|it| {
            let Item::Track(t) = it else { return true };
            let TrackPrim::Seg { a, b } = t.prim else { return true };
            let (a, b) = (Vec2 { x: f64::from(a.x), y: -f64::from(a.y) }, Vec2 { x: f64::from(b.x), y: -f64::from(b.y) });
            let prim = crate::geom::Prim::Seg(crate::geom::Seg { a, b });
            pads.iter().all(|&i| {
                let pad = &self.model.pads[i];
                if !pad.layers.contains(&t.layer) {
                    return true;
                }
                let sk = self.model.skeleton(pad, t.layer);
                let d = crate::geom::dist::dist_prim_skeleton(&prim, &sk);
                d - 0.5 * t.width as f64 >= tol || d <= 0.0
            }) && vias.iter().all(|&(c, r)| {
                let ab = b - a;
                let u = ((c - a).dot(ab) / ab.len2().max(1.0)).clamp(0.0, 1.0);
                let d = (a + ab * u - c).len() - r;
                d - 0.5 * t.width as f64 >= tol || d <= 0.0
            })
        })
    }

    /// A plane stub's track does not graze a pad of its net other than its own (`own`, an index
    /// into `model.pads`) nor an own-net via of `vias` (centre, radius): the sliver connection
    /// [`Drawer::track_own_pad_ok`] keeps wires from (D6a, a 0.5 mm pitch FPC connector: a GND stub to a
    /// via down-left grazed the neighbouring GND pad, "actual 0.0904 mm" < 0.127 mm). O(pads of the net).
    pub fn stub_own_ok(&self, stub: &Item, own: usize, vias: &[(Vec2, f64)]) -> bool {
        let Item::Track(t) = stub else { return true };
        if self.model.rules.min_connection <= 0 || t.net == 0 {
            return true;
        }
        let TrackPrim::Seg { a, b } = t.prim else { return true };
        let (a, b) = (Vec2 { x: f64::from(a.x), y: -f64::from(a.y) }, Vec2 { x: f64::from(b.x), y: -f64::from(b.y) });
        let prim = crate::geom::Prim::Seg(crate::geom::Seg { a, b });
        let tol = self.model.rules.max_error as f64;
        let ok = |d: f64| d - 0.5 * t.width as f64 >= tol || d <= 0.0;
        // Another pad of the net: the stub keeps off it altogether. A centre line that only clips
        // a corner is still a sliver (the FPC pad again: 0.1025 mm), and a stub needs no other pad.
        // A pad the stub starts inside is the own pad's copper (D6b: a USB-C receptacle's A1 and
        // B12 are one GND pad drawn twice; every stub of A1 started inside B12 and was
        // refused, so J1.A1 became a 16 mm signal connection).
        let off = |d: f64| d - 0.5 * t.width as f64 >= tol;
        let root = crate::geom::Prim::Seg(crate::geom::Seg { a, b: a });
        self.model.pads.iter().enumerate().filter(|&(i, p)| i != own && p.net == t.net && p.layers.contains(&t.layer)).all(|(_, pad)| {
            let sk = self.model.skeleton(pad, t.layer);
            crate::geom::dist::dist_prim_skeleton(&root, &sk) <= 0.0 || off(crate::geom::dist::dist_prim_skeleton(&prim, &sk))
        })
            && vias.iter().all(|&(c, r)| {
                let ab = b - a;
                let u = ((c - a).dot(ab) / ab.len2().max(1.0)).clamp(0.0, 1.0);
                ok((a + ab * u - c).len() - r)
            })
    }

    /// Index `item` as accepted copper; its handle (for [`Drawer::uncommit`]).
    pub fn commit(&mut self, item: &Item) -> usize {
        let i = self.idx.insert(item);
        let e = self.add_dynamic(item);
        self.handles.push((i, e));
        self.handles.len() - 1
    }

    /// JSON of the drawing state for the debug picture (`TOPO_DUMP`): live dynamic segments,
    /// static pad polygons, failed runs.
    pub fn dump(&self) -> String {
        let pt = |p: Vec2| format!("[{:.0},{:.0}]", p.x, p.y);
        let mut segs = Vec::new();
        let mut polys = Vec::new();
        for e in self.ents.iter().filter(|e| e.live) {
            match &e.core {
                Core::Seg(a, b) if e.net.is_some() && !e.fixed => {
                    segs.push(format!("[{},{},{:.0},{},{}]", pt(*a), pt(*b), 2.0 * e.grow, e.mask, e.net.unwrap_or(0)))
                }
                Core::Poly(p) => polys.push(format!("[{},{}]", e.mask, p.iter().map(|&q| pt(q)).collect::<Vec<_>>().join(","))),
                Core::Pt(c) if e.grow > 0.0 => polys.push(format!(
                    "[{},{}]",
                    e.mask,
                    (0..16)
                        .map(|i| {
                            let a = i as f64 * std::f64::consts::PI / 8.0;
                            pt(*c + Vec2 { x: a.cos() * e.grow, y: a.sin() * e.grow })
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                )),
                _ => {}
            }
        }
        let fails: Vec<String> = self
            .fails
            .iter()
            .map(|(w, l, at, r)| {
                let wi = &self.st.wires[*w];
                format!(
                    "{{\"wire\":{w},\"net\":{},\"w\":{},\"layer\":{l},\"reason\":{:?},\"tris\":{:?},\"tries\":[{}]}}",
                    wi.net,
                    wi.w,
                    r,
                    wi.tris,
                    at.iter().map(|p| format!("[{}]", p.iter().map(|&q| pt(q)).collect::<Vec<_>>().join(","))).collect::<Vec<_>>().join(",")
                )
            })
            .collect();
        let pr = &self.b.proj;
        let tris: Vec<String> = pr.tris.iter().map(|t| format!("[{}]", t.iter().map(|&v| pt(pr.pos[v])).collect::<Vec<_>>().join(","))).collect();
        // Per triangle: the wires traversing it `[wire, net, k]`.
        let trav: Vec<String> = self.st.tri_trav.iter().map(|v| format!("[{}]", v.iter().map(|&(w, k)| format!("[{w},{},{k}]", self.st.wires[w].net)).collect::<Vec<_>>().join(","))).collect();
        format!("{{\"segs\":[{}],\"polys\":[{}],\"fails\":[{}],\"tris\":[{}],\"trav\":[{}]}}", segs.join(","), polys.join(","), fails.join(","), tris.join(","), trav.join(","))
    }

    /// Take the copper of `handle` back out.
    pub fn uncommit(&mut self, handle: usize) {
        let (i, e) = self.handles[handle];
        // Idempotent: the index reuses freed ids, so a second removal would drop another item.
        if self.ents.get(e).is_some_and(|en| !en.live) {
            return;
        }
        self.idx.remove(i);
        if let Some(en) = self.ents.get_mut(e) {
            en.live = false;
            // first-draw-45 r9 (esp runtime): the dead object leaves the spatial grid too. Every
            // grid query skips dead objects, so the answers are the same; but each refit that
            // the checker refuses (straightening, polish, fuse) commits the run's old tracks
            // again as new objects, and the dead ones piled up in the grid cells (esp: the
            // via reduction's repair queries 12 x slower). O(cells of the object).
            let bb = self.ent_box[e].grow(en.grow + en.clear + 1.0);
            self.grid.remove(bb, e);
        }
        self.joint.on_uncommit(handle, i);
    }

    /// Give the passes of wire `w` the layers of `layers` (runs separated at the traversals
    /// `cuts`, the vias kept, their discs too) and pack again the edge-layers whose passes
    /// changed (via reduction trial). Returns the previous layer and disc of every pass of the
    /// wire, for [`Drawer::restore_layers`].
    pub fn relayer(&mut self, w: WireId, layers: &[usize], cuts: &[usize]) -> (Vec<i32>, Vec<Disc>) {
        let st = self.st;
        let ps = &st.wires[w].passes;
        let n = ps.len();
        let old: (Vec<i32>, Vec<Disc>) = (ps.iter().map(|&p| self.pass_layer[p]).collect(), ps.iter().map(|&p| self.pass_disc[p]).collect());
        for r in 0..layers.len() {
            let k0 = if r == 0 { 0 } else { cuts[r - 1] };
            let k1 = if r == layers.len() - 1 { n } else { cuts[r] };
            for &p in &ps[k0.min(n)..k1.min(n)] {
                self.pass_layer[p] = layers[r] as i32;
            }
        }
        let dmax = old.1.iter().map(|d| d[0].max(d[1])).fold(0.0, f64::max);
        let mut disc = wire_discs(st, w, cuts, &vec![dmax; cuts.len()]);
        // TW6 part A: a kept via keeps the height its disc had (`[capacity] via_disc_height`).
        for (k, d) in disc.iter_mut().enumerate() {
            if d[0].max(d[1]) > 0.0 && old.1[k][0].max(old.1[k][1]) > 0.0 {
                d[2] = old.1[k][2];
            }
        }
        for (k, &p) in ps.iter().enumerate() {
            self.pass_disc[p] = disc[k];
        }
        self.repack(w, &old);
        old
    }

    /// Put back the pass layers and discs `old` of wire `w`.
    pub fn restore_layers(&mut self, w: WireId, old: &(Vec<i32>, Vec<Disc>)) {
        let ps = &self.st.wires[w].passes;
        let cur: (Vec<i32>, Vec<Disc>) = (ps.iter().map(|&p| self.pass_layer[p]).collect(), ps.iter().map(|&p| self.pass_disc[p]).collect());
        for (k, &p) in ps.iter().enumerate() {
            self.pass_layer[p] = old.0[k];
            self.pass_disc[p] = old.1[k];
        }
        self.repack(w, &cur);
    }

    fn repack(&mut self, w: WireId, was: &(Vec<i32>, Vec<Disc>)) {
        let st = self.st;
        let nl = self.b.proj.n_layers;
        let mut todo: std::collections::BTreeSet<(EdgeId, usize)> = std::collections::BTreeSet::new();
        for (k, &p) in st.wires[w].passes.iter().enumerate() {
            if self.pass_disc[p] != was.1[k] {
                for l in 0..nl {
                    todo.insert((st.passes[p].edge, l));
                }
            } else if self.pass_layer[p] != was.0[k] {
                for l in [was.0[k], self.pass_layer[p]] {
                    if l >= 0 && (l as usize) < nl {
                        todo.insert((st.passes[p].edge, l as usize));
                    }
                }
            }
        }
        for (e, l) in todo {
            self.pack(e, l);
        }
    }

    /// Draw wire `w` with its `plan` and placed `vias`; on success its copper is indexed.
    /// `ids` collects the indexed item ids for a rollback by the caller.
    pub fn draw_wire(&mut self, w: WireId, plan: &Plan, vias: &[(usize, Vec2, ViaSize)], ids: &mut Vec<usize>) -> WireOut {
        // TW2: copper this drawing takes out of the index belongs to `w` (`JointState::lost_to_other`).
        let cur = self.joint.cur.replace(w);
        let out = self.draw_wire_runs(w, plan, vias, ids);
        if !out.ok {
            // The reserved bands of its runs after the one that failed go with the wire.
            self.release_bands(w);
        }
        self.joint.cur = cur;
        out
    }

    fn draw_wire_runs(&mut self, w: WireId, plan: &Plan, vias: &[(usize, Vec2, ViaSize)], ids: &mut Vec<usize>) -> WireOut {
        if self.test_fail_via_wire && plan.layers.len() > 1 {
            return WireOut { reason: "drawing: injected by the test hook (fail via wire)".to_string(), ..WireOut::default() };
        }
        // 5.4 `[search] branch_relocate`: a junction point the plan moved splits the run it lies
        // in, like a via that keeps the layer (a zero-size via, never emitted).
        let split = self.split_at.get(&w).filter(|v| !v.is_empty()).cloned();
        let (plan_s, vias_s);
        let (plan, vias) = match split {
            None => (plan, vias),
            Some(sp) => {
                let mut ev: Vec<(usize, Vec2, ViaSize, Option<usize>)> = vias.iter().enumerate().map(|(i, v)| (v.0, v.1, v.2, Some(i))).collect();
                for &(k, p) in &sp {
                    if !vias.iter().any(|v| v.0 == k) {
                        ev.push((k, p, ViaSize { d: 0, drill: 0 }, None));
                    }
                }
                ev.sort_by_key(|e| (e.0, e.3.is_some()));
                let mut layers = vec![plan.layers[0]];
                for e in &ev {
                    let l = match e.3 {
                        Some(i) => plan.layers[i + 1],
                        None => *layers.last().expect("one layer"),
                    };
                    layers.push(l);
                }
                plan_s = Plan { layers, vias: ev.iter().map(|e| (e.0, e.0)).collect(), at: ev.iter().map(|e| e.0).collect() };
                vias_s = ev.iter().map(|e| (e.0, e.1, e.2)).collect::<Vec<_>>();
                (&plan_s, &vias_s[..])
            }
        };
        let st = self.st;
        let wi = &st.wires[w];
        let n = wi.passes.len();
        let mut out = WireOut { wire: w, ..Default::default() };
        let mut committed: Vec<usize> = Vec::new();
        for r in 0..plan.layers.len() {
            let k0 = if r == 0 { 0 } else { vias[r - 1].0 };
            let k1 = if r == plan.layers.len() - 1 { n } else { vias[r].0 };
            // A branch ends on its trunk's drawn copper (D5 net trees, `end_at`), else at its vertex.
            // edge-clear: a pad end whose centre breaks a static clearance ends at its anchor (`pad_end`).
            // exist-term r4: a run between a lead end and its via there is none (the lead is
            // drawn up to the via below).
            let at_lead = |end: usize, j: usize| vias.get(j).is_some_and(|v| self.lead_via.get(&(w, end)) == Some(&v.1));
            if (r == 0 && at_lead(0, 0)) || (r == plan.layers.len() - 1 && r > 0 && at_lead(1, r - 1)) {
                continue;
            }
            let s = if r == 0 { self.run_end(w, 0) } else { vias[r - 1].1 };
            let e = if r == plan.layers.len() - 1 { self.run_end(w, 1) } else { vias[r].1 };
            let layer = plan.layers[r];
            let lid = self.layer_id(layer);
            let mut ok = false;
            let mut last_reason = String::new();
            let mut last_viol: Option<Violation> = None;
            let mut tried: Vec<Vec<Vec2>> = Vec::new();
            // Force mode (M-B1): every attempt's copper with its violations, the cheapest wins.
            // nd-perf F1: a candidate with violations is priced only when the selection needs it.
            let mut cands: Vec<ForcedCand> = Vec::new();
            // M-C: the plan's necked stretches (width in P2); the drawing realises them.
            let pneck = self.plan_neck(w);
            // M3 `[drawing] realise = "erbs"`: the run's ERBS first (attempt -1), checked like any
            // attempt against the copper drawn so far; the ladder below is the counted fallback.
            let rb = self.erbs.get(&w).and_then(|v| v.iter().find(|b| b.layer == layer && (b.s - s).len() < 1000.0 && (b.e - e).len() < 1000.0));
            if rb.is_none() && !self.erbs.is_empty() {
                // TW2: why the run has no band (result.json route.erbs.no_band_list).
                let on_layer = self.erbs.get(&w).map(|v| v.iter().filter(|b| b.layer == layer).map(|b| (b.s - s).len().max((b.e - e).len())).fold(f64::INFINITY, f64::min));
                let why = match on_layer {
                    None => ("no stretch", 0),
                    Some(d) if d.is_infinite() => ("other layer", 0),
                    Some(d) => ("ends moved", d.round() as i64),
                };
                self.joint.no_band.insert((w, r), (layer as u32, why.0, why.1));
            }
            let band_len = rb.map_or(0.0, |b| b.band_len);
            let band = rb.map(|b| b.path.clone());
            // TW2: the run's band as the joint commit reserved it (`super::joint`): attempt -1
            // adopts its committed copper (`adopt`) instead of checking it against the copper
            // drawn so far. From here on it is this wire's copper.
            let mut res = self.joint.reserved.remove(&(w, r));
            for h in res.iter().flat_map(|x| &x.handles) {
                self.joint.owner.remove(h);
            }
            // draw-via-neck C (`[drawing] pad_tip_anchor`, r3): a band that reaches its own
            // elongated pad through the pad's side is re-anchored at the tip here, when the run
            // is drawn: the wires drawn before it are in the index as drawn (their octilinear
            // shapes, which leave their bands), the others as the joint commit reserved them, so
            // the anchor takes no room their copper holds (r2 decided it before the joint commit
            // from the bands alone: esp IO8_SCL lost its reserved band). The run's reservation
            // (the old tail) goes; the anchored band is then checked like any attempt -1.
            let band = match band {
                // r5: a reserved run's anchor was decided before any run was drawn
                // (`tip_anchor_reserved`, order-free); only runs without a reservation here.
                Some(b) if self.opts.pad_tip_anchor && res.is_none() && b.len() >= 2 && (r == 0 || r == plan.layers.len() - 1) => {
                    let mut skip: Vec<usize> = res.iter().flat_map(|x| &x.handles).map(|&h| self.handles[h].0).collect();
                    skip.sort_unstable();
                    let mut p = b.clone();
                    let mut moved = false;
                    for end in [0usize, 1] {
                        if (end == 0 && r != 0) || (end == 1 && r != plan.layers.len() - 1) {
                            continue;
                        }
                        if let Some(np) = self.tip_anchor_path(w, end, layer, &p, &skip) {
                            p = np;
                            moved = true;
                        }
                    }
                    if moved {
                        for h in res.take().map(|x| x.handles).into_iter().flatten() {
                            self.uncommit(h);
                        }
                        self.stats.tip_anchored += 1;
                        // The wires drawn after it read its band (corridors, class test) as drawn.
                        if let Some(rb) = self.erbs.get_mut(&w).and_then(|v| v.iter_mut().find(|b| b.layer == layer && (b.s - s).len() < 1000.0 && (b.e - e).len() < 1000.0)) {
                            rb.path = p.clone();
                        }
                        Some(p)
                    } else {
                        Some(b)
                    }
                }
                other => other,
            };
            let mut adopt: Option<Vec<usize>> = None;
            let (mut erbs_octi, mut octi_vs_band) = (false, false);
            let cut = self.cut_class(w, r, band.is_some());
            // M3 length factor: a legal ERBS priced past its band waits here while the ladder
            // draws a sequential run to compare (`erbs_vs_sequential`); the last step
            // (`ERBS_KEPT`) draws it when no sequential run was legal. Nothing is committed while
            // it waits, so its check stands. (price, length, points, items, plan neck, pad neck)
            #[allow(clippy::type_complexity)]
            let mut kept: Option<(f64, f64, Vec<Vec2>, Vec<Item>, Option<(f64, [f64; super::exact::NECK_ZONES])>, Option<(f64, [f64; 2])>)> = None;
            const ERBS_KEPT: isize = 6;
            let ladder: Vec<isize> = if self.opts.realise == "erbs" {
                if band.is_none() {
                    self.stats.erbs_none += 1;
                }
                std::iter::once(-1).chain(0..6).chain(std::iter::once(ERBS_KEPT)).collect()
            } else {
                (0..6).collect()
            };
            for attempt in ladder {
                // -1 (`realise = "erbs"`): the run's ERBS;
                // 0: the exact taut path at the nominal width; 1: the same with the plan's necked
                // stretches (M-C; TopoR 6.3 manual p. 56: the minimum width only where no other
                // path is available), only when the plan necks the wire; 2: none since M6 (the
                // drawing's own pad-end necks of `neck_in_drawing`, before M-C, removed 2026-11-18);
                // 3..5: S2a's funnel + repair attempts.
                let restored = attempt == ERBS_KEPT;
                if attempt >= 0 && kept.is_none() {
                    // A reserved band that did not end the run at attempt -1 leaves with it.
                    for h in adopt.take().into_iter().flatten().chain(res.take().map(|x| x.handles).into_iter().flatten()) {
                        self.uncommit(h);
                    }
                }
                if self.test_no_sequential && band.is_some() && (0..ERBS_KEPT).contains(&attempt) {
                    continue;
                }
                if restored && kept.is_none() {
                    continue;
                }
                let pts = if restored {
                    kept.as_ref().map(|k| k.2.clone()).unwrap_or_default()
                } else if attempt < 0 {
                    match &band {
                        Some(b) if b.len() >= 2 => {
                            let mut b = b.clone();
                            b[0] = s;
                            let n = b.len();
                            b[n - 1] = e;
                            b
                        }
                        _ => continue,
                    }
                } else if attempt == 0 && self.test_straight_seq && band.is_some() {
                    vec![s, e]
                } else if attempt <= 2 {
                    if attempt == 2 || (attempt == 1 && pneck.is_none()) {
                        continue;
                    }
                    let base = self.base_polyline(w, k0, k1, s, e, 0);
                    let t_ex = std::time::Instant::now();
                    let fixed = self.exact_fix(w, k0, k1, layer, base, if attempt == 1 { pneck } else { None });
                    self.stats.exact_s += t_ex.elapsed().as_secs_f64();
                    match fixed {
                        Some(p) => p,
                        None => {
                            self.stats.exact_none += 1;
                            continue;
                        }
                    }
                } else {
                    self.run_polyline(w, k0, k1, s, e, (attempt - 3) as usize, layer)
                };
                if !restored {
                    tried.push(pts.clone());
                }
                if attempt >= 3 && !restored {
                    let walls = self.via_walls(w, k0, k1, layer);
                    if pts.windows(2).any(|x| walls.iter().any(|&(c, d)| seg_proper(x[0], x[1], c, d))) {
                        last_reason = "drawing: passes a via of another net on the wrong side (topology)".to_string();
                        // Force mode: the checker judges the copper (the via's own trace is drawn).
                        if !self.force {
                            continue;
                        }
                    }
                }
                let (ps, pe) = (r == 0, r == plan.layers.len() - 1);
                let adopted = if attempt < 0 { res.take() } else { None };
                let mut items = if restored {
                    kept.as_ref().map(|k| k.3.clone()).unwrap_or_default()
                } else if let Some(x) = &adopted {
                    x.items.clone()
                } else {
                    self.run_items(w, layer, &pts)
                };
                // draw-absorb A1: the zones of an adopted band the joint commit necked against a
                // gap partner (its warnings name them; its octilinear form keeps them).
                // draw-absorb A1b (`[drawing] reserved_keep_neck`): an adopted band reserved with
                // the plan's neck takes its octilinear form at that neck too (never wider).
                let a1b = adopted.as_ref().is_some_and(|x| x.zones.is_none() && self.opts.reserved_keep_neck && x.pneck.is_some_and(|p| p.0 < wi.w) && pneck.is_some());
                let a1z: Option<super::exact::Neck> = adopted.as_ref().and_then(|x| {
                    x.zones.map(|ends| super::exact::Neck { min_w: x.pneck.map_or(0.0, |p| p.0), ends }).or(match (a1b, x.pneck, &pneck) {
                        (true, Some((wn, _)), Some(n)) => Some(super::exact::Neck { min_w: wn, ends: n.ends }),
                        _ => None,
                    })
                });
                // A1b meter: the adopted band's necked length (an octilinear form at full width drops it).
                let adopted_neck: f64 = adopted.as_ref().and_then(|x| x.pneck).map_or(0.0, |p| p.1.iter().sum());
                let adopted_wn: Option<f64> = adopted.as_ref().and_then(|x| x.pneck).map(|p| p.0);
                let mut pneck_out: Option<(f64, [f64; super::exact::NECK_ZONES])> = if restored {
                    kept.as_ref().and_then(|k| k.4)
                } else {
                    adopted.as_ref().and_then(|x| x.pneck)
                };
                // A reserved band passed the joint check against every band and static item.
                let full = if restored || adopted.is_some() { CheckReport::default() } else { self.checker.check_against(&self.idx, &items, &[]) };
                if let Some(x) = adopted {
                    erbs_octi = x.octi;
                    adopt = Some(x.handles);
                }
                let mut rep = match &pneck {
                    _ if full.violations.is_empty() => full,
                    Some(pn) => match self.plan_neck_run(w, lid, &pts, pn) {
                        Some((wn, its, ls)) => {
                            items = its;
                            pneck_out = Some((wn, ls));
                            CheckReport::default()
                        }
                        None => {
                            // draw-via-neck B: no width passes (a violation the width cannot
                            // remove): the widest width that adds none to the floor's, necked
                            // only where that changes something.
                            let wn = if self.opts.neck_trim { self.plan_neck_widest(w, lid, &pts, pn) } else { pn.min_w };
                            let nk = if self.opts.neck_trim { self.fit_neck(w, lid, &pts, pn, wn) } else { *pn };
                            items = self.neck_items(w, lid, &pts, &nk, wn).0;
                            self.checker.check_against(&self.idx, &items, &[])
                        }
                    },
                    None => full,
                };
                // The ERBS is any-angle; the wires drawn before it were made octilinear, which
                // moves their corners outwards (an octagon's vertex lies at rho / cos 22.5 deg),
                // into the room this ERBS keeps at the exact pitch. Its own octilinear form keeps
                // the same pitch to them (Dayan 1997 PDF p. 33, Fig. 11: octilinear spokes
                // approximate the arc), so it is checked before the ladder takes over.
                let mut pts = pts;
                if attempt < 0 && !rep.violations.is_empty() && self.octi.on && pts.len() >= 2 {
                    let exits = [if r == 0 { self.pad_exit(w, 0, pts[1]) } else { None }, if r == plan.layers.len() - 1 { self.pad_exit(w, 1, pts[pts.len() - 2]) } else { None }];
                    if let Some(o) = self.octilinear(w, k0, k1, layer, &pts, None, exits, None) {
                        if let Some((its, pn)) = self.realise(w, layer, lid, &o.pts, &pneck) {
                            self.stats.octi_asis += o.asis;
                            self.stats.octi_asis_below += o.asis_below;
                            pts = o.pts;
                            items = its;
                            pneck_out = pn;
                            rep = CheckReport::default();
                            self.stats.erbs_octi += 1;
                            erbs_octi = true;
                        }
                    }
                }
                let mut neck: Option<(f64, [f64; 2])> = if restored { kept.as_ref().and_then(|k| k.5) } else { None };
                if rep.violations.is_empty() && !self.track_own_pad_ok(w, &items) {
                    last_reason = "drawing: a track grazes a pad or via of its own net (KiCad minimum connection width)".to_string();
                    if self.force {
                        let vs = self.graze_viols(w, &items);
                        cands.push((vs.iter().map(Viol::amount).sum(), pts.clone(), items.clone(), vs, neck, attempt_label(attempt), None));
                    }
                    continue;
                }
                if rep.violations.is_empty() {
                    // M-45: the legal taut run redrawn octilinear where that keeps every rule
                    // (`super::octi`); else the taut run stays (its any-angle stretches priced).
                    let (mut pts, mut items, mut pneck_out) = (pts, items, pneck_out);
                    // Debug (`TOPO_ERBS_DEBUG`): the run before its octilinear form and pull.
                    let pre_octi = (band.is_some() && std::env::var_os("TOPO_ERBS_DEBUG").is_some()).then(|| pts.clone());
                    // first-draw-45 r9 forensics (`TOPO_RUNPROV`): the run's octilinear outcome.
                    let mut otag: &str = if restored { "erbs-kept" } else if neck.is_some() { "neck" } else { "skip" };
                    if self.octi.on && (neck.is_some() || restored) && std::env::var_os("TOPO_OCTIDBG").is_some() {
                        eprintln!("OCTISKIP wire {w} net {} L{layer} neck {} restored {restored} from ({:.4},{:.4})", wi.net, neck.is_some(), pts[0].x / 1e6, -pts[0].y / 1e6);
                    }
                    if self.octi.on && neck.is_none() && pts.len() >= 2 && !restored {
                        let t_o = std::time::Instant::now();
                        let wneck = if pneck_out.is_some() || attempt == 1 { pneck } else { None };
                        let exits = [if ps { self.pad_exit(w, 0, pts[1]) } else { None }, if pe { self.pad_exit(w, 1, pts[pts.len() - 2]) } else { None }];
                        let dbg = std::env::var("TOPO_OCTIDBG").is_ok();
                        // draw-absorb A1: an adopted band necked against a gap partner takes its
                        // octilinear form at its neck (zones and width), never wider.
                        let real = |o_pts: &[Vec2]| match &a1z {
                            // A1b: main's realise, unless it draws the band wider than reserved
                            // (full width, or a wider neck); then the reserved neck where legal.
                            Some(z) if a1b => {
                                let main = self.realise(w, layer, lid, o_pts, &pneck);
                                if main.as_ref().is_some_and(|(_, pn)| pn.is_some_and(|p| p.0 <= z.min_w + 0.5)) {
                                    return main;
                                }
                                let (its, ls) = self.kept_neck_items(w, layer, o_pts, z);
                                if ls.iter().sum::<f64>() > 0.0 && self.checker.check_against(&self.idx, &its, &[]).violations.is_empty() && self.track_own_pad_ok(w, &its) {
                                    Some((its, Some((z.min_w, ls))))
                                } else {
                                    main
                                }
                            }
                            Some(z) => {
                                // r2: one neck stretch per run, pad stubs kept (`a1_finish`).
                                let (its, _) = self.a1_finish(w, self.neck_items(w, lid, o_pts, z, z.min_w).0);
                                let mut ls = [0.0; super::exact::NECK_ZONES];
                                if let (Some((_, _, l)), Some(i)) = (super::joint::neck_of(&its, wi.w), z.ends.iter().position(Option::is_some)) {
                                    ls[i] = l;
                                }
                                if self.checker.check_against(&self.idx, &its, &[]).violations.is_empty() && self.track_own_pad_ok(w, &its) {
                                    return Some((its, Some((z.min_w, ls))));
                                }
                                // r2 (a1b_octi_crop): a necked form that breaks a rule falls back
                                // to main's realise of the same form (legal or None), so the
                                // band never loses its octilinear form to the neck. Debug arm
                                // (`TOPO_FD45_OFF=dab_ofb`): r1's reject.
                                if super::octi::fd45_off("dab_ofb") {
                                    return None;
                                }
                                let main = self.realise(w, layer, lid, o_pts, &pneck);
                                if std::env::var_os("TOPO_ABSORBDBG").is_some() {
                                    eprintln!("ABSORBDBG a1z octi fail wire {w} net {} run {r}: main realise {}", wi.net, if main.is_some() { "ok" } else { "none" });
                                }
                                main
                            }
                            None => self.realise(w, layer, lid, o_pts, &pneck),
                        };
                        // r2 (qm JS1_4 RCA): the joint commit's neck gives the adopted band a
                        // reserved neck, so main's call would search its form with the plan neck
                        // (`wneck`), a different staircase over the whole run that moves the
                        // bands nested beside it. An A1 band takes main's full-width form first
                        // (the form off would draw), necked by `real`; the plan-neck form only
                        // when that one breaks a rule. Debug arm (`TOPO_FD45_OFF=dab_form`): r1.
                        let a1_main_form = a1z.is_some() && !a1b && wneck.is_some() && !super::octi::fd45_off("dab_form");
                        let got = match a1_main_form.then(|| self.octilinear(w, k0, k1, layer, &pts, None, exits, None)).flatten() {
                            Some(o) if real(&o.pts).is_some() => Some(o),
                            _ => self.octilinear(w, k0, k1, layer, &pts, wneck, exits, None),
                        };
                        let tag = match got {
                            Some(o) => match real(&o.pts) {
                                Some((its, pn)) => {
                                    if attempt < 0 && adopted_neck > 0.0 {
                                        // A1b meter: an octilinear form necked wider than reserved.
                                        if let (Some((w2, _)), Some(wn)) = (pn, adopted_wn) {
                                            if w2 > wn + 0.5 && !a1b {
                                                if std::env::var_os("TOPO_A1BDBG").is_some() {
                                                    let (a, b) = (o.pts[0], o.pts[o.pts.len() - 1]);
                                                    eprintln!("A1B octi_wider wire {w} net {} run {r} reserved {:.3} drawn {:.3} mm from ({:.3},{:.3}) to ({:.3},{:.3})", self.st.wires[w].net, wn / 1e6, w2 / 1e6, a.x / 1e6, -a.y / 1e6, b.x / 1e6, -b.y / 1e6);
                                                }
                                                self.joint.unnecked.push((w, r, adopted_neck, 0.0, "octi_wider"));
                                            }
                                        }
                                        if pn.is_none() {
                                            let room = match (&pneck, adopted_wn) {
                                                (Some(n), Some(wn)) => self.room_taken(w, &o.pts, n, wn),
                                                _ => 0.0,
                                            };
                                            self.joint.unnecked.push((w, r, adopted_neck, room, "octi"));
                                        } else if a1b && pn.is_some_and(|p| adopted_wn.is_some_and(|wn| p.0 <= wn + 0.5)) {
                                            self.joint.kept_neck += 1;
                                        }
                                    }
                                    self.stats.octi_asis += o.asis;
                                    self.stats.octi_asis_below += o.asis_below;
                                    pts = o.pts;
                                    items = its;
                                    pneck_out = pn;
                                    self.stats.octi_ok += 1;
                                    if attempt < 0 {
                                        // The adopted band gives way to its own octilinear form.
                                        erbs_octi = true;
                                        for h in adopt.take().into_iter().flatten() {
                                            self.uncommit(h);
                                        }
                                    }
                                    "ok"
                                }
                                None => {
                                    // TW2 (lead Q1): the D11 transform's share, counted.
                                    if attempt < 0 && adopt.is_some() && self.hits_band(w, layer, &o.pts) {
                                        octi_vs_band = true;
                                    }
                                    if dbg {
                                        eprintln!("OCTIREJ wire {w} pts {:?}", o.pts.iter().map(|p| (p.x / 1e6, -p.y / 1e6)).collect::<Vec<_>>());
                                    }
                                    self.stats.octi_rejected += 1;
                                    "rejected"
                                }
                            },
                            None => "none",
                        };
                        otag = tag;
                        if dbg {
                            let (any, _) = super::octi::breaks(&pts);
                            eprintln!(
                                "OCTI wire {w} net {} L{layer} attempt {attempt} {tag}: {} pts, any-angle {} stretches {:.3} mm, from ({:.3},{:.3}) to ({:.3},{:.3})",
                                wi.net,
                                pts.len(),
                                any.len(),
                                any.iter().map(|a| a.2).sum::<f64>() / 1e6,
                                pts[0].x / 1e6,
                                -pts[0].y / 1e6,
                                pts[pts.len() - 1].x / 1e6,
                                -pts[pts.len() - 1].y / 1e6
                            );
                        }
                        self.stats.octi_s += t_o.elapsed().as_secs_f64();
                        if self.octi.any_forbidden && !super::octi::breaks(&pts).0.is_empty() {
                            last_reason = "drawing: no octilinear geometry for this run ([drawing] any_angle = \"forbidden\")".to_string();
                            if self.force {
                                cands.push((0.0, pts.clone(), items.clone(), Vec::new(), None, attempt_label(attempt), None));
                            }
                            continue;
                        }
                    }
                    // P6 (c): pulled taut in its corridor (no bumps, no spikes). An ERBS is taut
                    // by construction, and keeps the room of the wires not drawn yet.
                    if self.opts.draw_pull_taut && neck.is_none() && attempt >= 0 && !restored {
                        if let Some((p2, its2, pn2)) = self.pull_taut(w, k0, k1, layer, lid, &pts, &pneck) {
                            if std::env::var_os("TOPO_OCTIDBG").is_some() {
                                eprintln!("PULL wire {w} {:?} -> {:?}", pts.iter().map(|p| (p.x / 1e6, -p.y / 1e6)).collect::<Vec<_>>(), p2.iter().map(|p| (p.x / 1e6, -p.y / 1e6)).collect::<Vec<_>>());
                            }
                            self.stats.pulled += pts.len() - p2.len();
                            pts = p2;
                            items = its2;
                            pneck_out = pn2;
                        }
                    }
                    // M3 length factor: the ERBS against a sequential run (`erbs_vs_sequential`).
                    let mut src = if restored { -1 } else { attempt };
                    let mut replaced = false;
                    if attempt < 0 {
                        let le = poly_len(&pts);
                        let pe = erbs_length_price(le, band_len, self.opts.erbs_length_weight);
                        if erbs_vs_sequential(pe, band_len, self.opts.erbs_margin, None).is_some() {
                            self.stats.erbs_long += 1;
                            kept = Some((pe, le, pts, items, pneck_out, neck));
                            continue;
                        }
                    } else if restored {
                        if let Some(k) = kept.take() {
                            self.erbs_long.push((w, layer, k.1, band_len, f64::NAN, false));
                        }
                    } else if let Some(k) = kept.take() {
                        let ls = poly_len(&pts);
                        replaced = erbs_vs_sequential(k.0, band_len, self.opts.erbs_margin, Some(ls)) == Some(true);
                        // Round i: a competitor that crosses an undrawn wire's path with another
                        // parity than the ERBS left the plan's class (`competitor_keeps_class`).
                        if replaced && !self.test_no_class_keep {
                            if self.class_ref.is_none() {
                                self.class_ref = Some(self.build_class_ref());
                            }
                            let (st, drawn) = (self.st, &self.drawn_once);
                            let keep = |o: WireId| class_test_wire(w, wi.net, o, st.wires[o].alive, st.wires[o].net, drawn[o]);
                            let cr = self.class_ref.as_ref().expect("built");
                            let (xs, xe) = (cr.crossings(&pts, layer, &keep), cr.crossings(&k.2, layer, &keep));
                            if std::env::var_os("TOPO_ERBS_DEBUG").is_some() {
                                eprintln!("ERBS class check wire {w} L{layer}: competitor {xs:?}, the ERBS {xe:?}");
                            }
                            if !competitor_keeps_class(&xs, &xe) {
                                if std::env::var_os("TOPO_ERBS_DEBUG").is_some() {
                                    eprintln!("ERBS class kept wire {w} net {} L{layer}: competitor {:.3} mm crosses {xs:?}, the ERBS {xe:?}; pre {:?}", wi.net, ls / 1e6, pre_octi.as_ref().map(|v| v.len()));
                                }
                                replaced = false;
                                self.stats.erbs_class_kept += 1;
                            }
                        }
                        self.erbs_long.push((w, layer, k.1, band_len, ls, replaced));
                        if replaced {
                            self.stats.erbs_long_replaced += 1;
                            for h in adopt.take().into_iter().flatten() {
                                self.uncommit(h);
                            }
                        } else {
                            (pts, items, pneck_out, neck, src) = (k.2, k.3, k.4, k.5, -1);
                            otag = "erbs-long-kept";
                        }
                    }
                    let first = committed.len();
                    match adopt.take() {
                        // The reserved band's copper is already in the index.
                        Some(hs) if src < 0 => {
                            committed.extend(&hs);
                            ids.extend(&hs);
                        }
                        other => {
                            for h in other.into_iter().flatten() {
                                self.uncommit(h);
                            }
                            for it in &items {
                                let id = self.commit(it);
                                committed.push(id);
                                ids.push(id);
                            }
                        }
                    }
                    if let Some(&h) = committed.get(first) {
                        self.record_crossings(w, k0, k1, &pts, h);
                    }
                    // TW2 `[drawing] erbs_gap_keep_max_mm`: a band kept across a model gap
                    // carries the gap as its violation when it is drawn second (no hiding).
                    if src < 0 {
                        let mut gv = self.gap_viols(w, r, &items);
                        let base = self.run_src(w, "gap_kept", "band", "", "kept", r, false, pneck.is_some(), &items);
                        self.tag_viols(&mut gv, &base);
                        out.viol.extend(gv);
                    }
                    if let Some((wn, ls)) = neck {
                        if ps {
                            out.necks.push((pts[0], wn, ls[0]));
                        }
                        if pe {
                            out.necks.push((pts[pts.len() - 1], wn, ls[1]));
                        }
                    }
                    if let (Some((wn, ls)), Some(pn)) = (pneck_out, a1z.as_ref().or(pneck.as_ref())) {
                        if wn < wi.w {
                            for (z, l) in pn.ends.iter().zip(ls) {
                                if let (Some((c, _)), true) = (z, l > 0.0) {
                                    out.necks.push((*c, wn, l));
                                }
                            }
                        }
                    }
                    // Debug (`TOPO_ERBS_DEBUG`): how each run of the ERBS drawing was drawn.
                    if band.is_some() && std::env::var_os("TOPO_ERBS_DEBUG").is_some() {
                        eprintln!("ERBS draw wire {w} net {} L{layer} k {k0}..{k1}: src {src} len {:.3} band {:.3} replaced {replaced}", wi.net, poly_len(&pts) / 1e6, band_len / 1e6);
                        let mm = |v: &[Vec2]| v.iter().map(|p| ((p.x / 1e3).round() / 1e3, (-p.y / 1e3).round() / 1e3)).collect::<Vec<_>>();
                        eprintln!("  pre {:?}\n  drawn {:?}", pre_octi.as_deref().map(mm), mm(&pts));
                    }
                    if src < 0 {
                        self.stats.erbs_ok += 1;
                    } else if src <= 2 {
                        self.stats.exact_ok += 1;
                    } else {
                        self.stats.fallback_ok += 1;
                    }
                    // A run the length factor gave to a sequential run is no fallback (the check
                    // did not reject its ERBS): `erbs_long_replaced` counts it.
                    if src >= 0 && band.is_some() && !replaced {
                        self.stats.erbs_fallback += 1;
                    }
                    run_prov("ladder", w, wi.net, layer, src, otag, &pts);
                    out.items.extend(items);
                    out.runs.push((layer, pts));
                    out.run_src.push(if self.opts.realise != "erbs" {
                        super::joint::RunSrc::Seq
                    } else if src < 0 {
                        super::joint::RunSrc::Erbs { octi: erbs_octi, octi_vs_band }
                    } else if replaced {
                        super::joint::RunSrc::Replaced
                    } else {
                        super::joint::RunSrc::Cut(cut)
                    });
                    ok = true;
                    break;
                }
                if self.force {
                    let (price, vs, lazy) = self.lazy_cand(w, &items, &rep);
                    cands.push((price, pts.clone(), items.clone(), vs, neck, attempt_label(attempt), lazy));
                }
                let v = &rep.violations[0];
                last_viol = Some(v.clone());
                if std::env::var("TOPO_DEBUG").is_ok() {
                    eprintln!("wire {w} net {} layer {layer} attempt {attempt} k {k0}..{k1}: viol {} gap {} req {} pts {:?}", wi.net, v.against, v.gap, v.required, pts.iter().map(|p| (p.x.round() as i64 / 1000, -p.y.round() as i64 / 1000)).collect::<Vec<_>>());
                }
                last_reason = format!(
                    "drawing: {} gap {:.3} mm < {:.3} mm against {} on layer {}",
                    if wi.w > 0.0 { "clearance" } else { "gap" },
                    v.gap as f64 / 1e6,
                    v.required as f64 / 1e6,
                    v.against,
                    self.model.layers.iter().find(|l| l.id == v.layer).map_or(v.layer.to_string(), |l| l.name.clone())
                );
            }
            if !ok && self.force {
                // Never drop (M-B1): the attempt with the least violation price is written; the
                // funnels before the repair are candidates too (the repair's bumps can make a
                // violation worse where no legal path exists, e.g. a gap too narrow for the track).
                // The exact path with the clearance relaxed by the least amount that gives one
                // (fractions of clearance + half width, then two bisection steps): the geometry
                // with the shallowest violation the sleeve allows.
                let full = self.model.rules.net_rule(wi.net).clearance as f64 + 0.5 * wi.w + self.eps;
                let base0 = self.base_polyline(w, k0, k1, s, e, 0);
                let mut try_relax = |f: f64, cands: &mut Vec<_>| -> bool {
                    let t_ex = std::time::Instant::now();
                    let got = self.exact_fix_relaxed(w, k0, k1, layer, base0.clone(), pneck, f * full);
                    self.stats.exact_s += t_ex.elapsed().as_secs_f64();
                    let Some(pts) = got else { return false };
                    let items = match &pneck {
                        Some(pn) => self.neck_items(w, lid, &pts, pn, pn.min_w).0,
                        None => self.run_items(w, layer, &pts),
                    };
                    let rep = self.checker.check_against(&self.idx, &items, &[]);
                    let (price, vs, lazy) = self.lazy_cand(w, &items, &rep);
                    cands.push((price, pts, items, vs, None, "relaxed", lazy));
                    true
                };
                let (mut lo, mut hi) = (0.0, f64::NAN);
                for f in [0.125, 0.25, 0.5, 1.0] {
                    if try_relax(f, &mut cands) {
                        hi = f;
                        break;
                    }
                    lo = f;
                }
                if hi.is_finite() {
                    for _ in 0..2 {
                        let mid = 0.5 * (lo + hi);
                        if try_relax(mid, &mut cands) {
                            hi = mid;
                        } else {
                            lo = mid;
                        }
                    }
                }
                for a in 0..3 {
                    let pts = self.base_polyline(w, k0, k1, s, e, a);
                    if pts.len() < 2 || pts.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
                        continue;
                    }
                    let items = match &pneck {
                        Some(pn) => self.neck_items(w, lid, &pts, pn, pn.min_w).0,
                        None => self.run_items(w, layer, &pts),
                    };
                    let rep = self.checker.check_against(&self.idx, &items, &[]);
                    let (price, vs, lazy) = self.lazy_cand(w, &items, &rep);
                    cands.push((price, pts, items, vs, None, "funnel", lazy));
                }
                let pick = self.pick_forced(w, &mut cands);
                if std::env::var("TOPO_FORCEDBG").is_ok() {
                    for c in &cands {
                        // A candidate the bound ruled out is not priced: "unpriced", graze violations only.
                        eprintln!("FORCED wire {w} layer {layer} k {k0}..{k1}: {} price {}, {} pts, viols {:?}", c.5, if c.6.is_some() { "unpriced".to_string() } else { format!("{:.1}", c.0) }, c.1.len(), c.3.iter().map(|v| (v.kind, v.against.clone(), (v.depth_nm / 1e3).round(), (v.len_nm / 1e3).round())).collect::<Vec<_>>());
                    }
                }
                // Shorts first, then the total violation price (a funnel run through a via must
                // not beat a relaxed run that only grazes clearances).
                if let Some((_, pts, items, vs, neck, src, _)) = pick.and_then(|j| cands.into_iter().nth(j)) {
                    // M-45, forced-draw (docs/dev/design/forced-draw.md): the forced run's
                    // octilinear forms by the first draw's DP, its clearance priced at the full
                    // rule, a short priced like a clearance unit and shorts first (two variants:
                    // the DP prices per leg, the key per object), and the cheapest of the taut run
                    // and its forms by one graded key (shorts first, then violation + style +
                    // length), measured by the checker.
                    let (mut pts, mut items, mut vs) = (pts, items, vs);
                    let mut ftag = "skip";
                    if self.octi.on && neck.is_none() && pts.len() >= 2 {
                        ftag = "none";
                        let t_o = std::time::Instant::now();
                        let (ps, pe) = (r == 0, r == plan.layers.len() - 1);
                        let exits = [if ps { self.pad_exit(w, 0, pts[1]) } else { None }, if pe { self.pad_exit(w, 1, pts[pts.len() - 2]) } else { None }];
                        let dbg = std::env::var_os("TOPO_OCTIDBG").is_some();
                        let mut forms: Vec<(Vec<Vec2>, Vec<Item>, Vec<Viol>)> = Vec::new();
                        for short_nm in [self.octi.viol_nm, self.octi.short_nm] {
                            let Some(o) = self.octilinear(w, k0, k1, layer, &pts, pneck, exits, Some(short_nm)) else { continue };
                            if forms.iter().any(|f| f.0 == o.pts) {
                                continue;
                            }
                            let its = match &pneck {
                                Some(pn) => self.neck_items(w, lid, &o.pts, pn, pn.min_w).0,
                                None => self.run_items(w, layer, &o.pts),
                            };
                            let rep = self.checker.check_against(&self.idx, &its, &[]);
                            let mut v2 = self.viols_of(&its, &rep);
                            v2.extend(self.graze_viols(w, &its));
                            forms.push((o.pts, its, v2));
                        }
                        let keys: Vec<(f64, f64)> = std::iter::once(forced_form_key(&vs, &pts, &self.octi)).chain(forms.iter().map(|f| forced_form_key(&f.2, &f.0, &self.octi))).collect();
                        if dbg {
                            let vl = |x: &[Viol]| x.iter().map(|v| format!("{} {} d{:.0} l{:.0} a{:.2}", v.kind, v.against, v.depth_nm / 1e3, v.len_nm / 1e3, v.amount())).collect::<Vec<_>>().join("; ");
                            let pl = |x: &[Vec2]| x.iter().map(|p| format!("({:.4},{:.4})", p.x / 1e6, -p.y / 1e6)).collect::<Vec<_>>().join(" ");
                            let dl = |x: &[Vec2]| self.forced_leg_deficits(w, layer, x, pneck).iter().map(|d| format!("{:.2}/{:.2}", d[0], d[1])).collect::<Vec<_>>().join(" ");
                            eprintln!("OCTIFORCEDBG wire {w} L{layer} keys {keys:?}\n  taut viols [{}]\n  taut pts {}\n  taut dp-legs [{}]", vl(&vs), pl(&pts), dl(&pts));
                            for f in &forms {
                                eprintln!("  form viols [{}]\n  form pts {}\n  form dp-legs [{}]", vl(&f.2), pl(&f.0), dl(&f.0));
                            }
                        }
                        if !forms.is_empty() {
                            let j = forced_form_pick(&keys);
                            if j > 0 {
                                (pts, items, vs) = forms.swap_remove(j - 1);
                                self.stats.octi_ok += 1;
                                self.stats.forced_octi += 1;
                                ftag = "ok";
                            } else {
                                ftag = "rejected";
                                if dbg {
                                    eprintln!("OCTIFORCE wire {w} rejected: keys {keys:?}");
                                }
                                self.stats.octi_rejected += 1;
                            }
                        } else if dbg {
                            eprintln!("OCTIFORCE wire {w} none");
                        }
                        self.stats.octi_s += t_o.elapsed().as_secs_f64();
                    }
                    // P6 (c): the forced run pulled taut too, where its violation price does not rise.
                    if self.opts.draw_pull_taut && neck.is_none() && pneck.is_none() {
                        if let Some(p2) = self.pull_pts(w, k0, k1, layer, &pts, Some(viol_key(&vs))) {
                            let its = self.run_items(w, layer, &p2);
                            let rep = self.checker.check_against(&self.idx, &its, &[]);
                            let mut v2 = self.viols_of(&its, &rep);
                            v2.extend(self.graze_viols(w, &its));
                            if key_le(viol_key(&v2), viol_key(&vs)) {
                                self.stats.pulled += pts.len() - p2.len();
                                pts = p2;
                                items = its;
                                vs = v2;
                            }
                        }
                    }
                    // Counted, never silent: a forced run that keeps free-angle stretches.
                    if self.octi.on {
                        let any = super::octi::breaks(&pts).0;
                        if !any.is_empty() {
                            self.stats.forced_free_runs += 1;
                            self.stats.forced_free_stretches += any.len();
                            self.stats.forced_free_mm += any.iter().map(|a| a.2).sum::<f64>() / 1e6;
                        }
                    }
                    match src {
                        "relaxed" => self.stats.forced_relaxed += 1,
                        "funnel" => self.stats.forced_funnel += 1,
                        _ => self.stats.forced_attempt += 1,
                    }
                    // The plan's necks on the forced run (its candidates draw the necked zones at
                    // the floor): a run that came out legal takes the widest width the checker
                    // accepts, each zone trimmed to the stretch this geometry needs (the forced
                    // geometry is not the plan's, so the zones measured on the plan's edges do not
                    // describe it; TopoR 6.3 manual p. 56: the minimum width only where needed);
                    // the narrowed stretches are recorded either way (p. 72: shrinkages).
                    let mut fnecks: Vec<(Vec2, f64, f64)> = Vec::new();
                    if let (Some(pn), None) = (&pneck, &neck) {
                        let narrow = items.iter().filter_map(|i| match i {
                            Item::Track(t) if (t.width as f64) < wi.w - 0.5 => Some(t.width as f64),
                            _ => None,
                        }).fold(None, |a: Option<f64>, x| Some(a.map_or(x, |y| y.min(x))));
                        if let Some(wmin) = narrow {
                            let mut zones = (*pn, wmin, self.neck_items(w, lid, &pts, pn, pn.min_w).1);
                            if vs.is_empty() {
                                if let Some((wn, _, _)) = self.plan_neck_run(w, lid, &pts, pn) {
                                    let tz = self.neck_out_of_pads(w, lid, &pts, self.trim_neck(w, lid, &pts, pn, wn));
                                    let (its, ls) = self.neck_items(w, lid, &pts, &tz, wn);
                                    if self.track_own_pad_ok(w, &its) {
                                        items = its;
                                        zones = (tz, wn, ls);
                                    }
                                }
                            } else if self.opts.neck_trim {
                                // draw-via-neck B: a forced run keeps its violations, but at the
                                // widest width that adds none to the floor's
                                // (`plan_neck_widest`) and necked only where narrowing adds
                                // nothing to them (`fit_neck` against the plan zones' count).
                                let wn = if wmin <= pn.min_w + 0.5 { self.plan_neck_widest(w, lid, &pts, pn) } else { wmin };
                                let nk = self.fit_neck(w, lid, &pts, pn, wn);
                                let (its, ls) = self.neck_items(w, lid, &pts, &nk, wn);
                                let n_old = self.checker.check_against(&self.idx, &items, &[]).violations.len();
                                if self.track_own_pad_ok(w, &its) && self.checker.check_against(&self.idx, &its, &[]).violations.len() <= n_old {
                                    items = its;
                                    zones = (nk, wn, ls);
                                }
                            }
                            let (z, wn, ls) = zones;
                            if wn < wi.w - 0.5 {
                                for (e, l) in z.ends.iter().zip(ls) {
                                    if let (Some((c, _)), true) = (e, l > 0.0) {
                                        fnecks.push((*c, wn, l));
                                    }
                                }
                            }
                        }
                    }
                    // draw-absorb A1 (`[drawing] forced_shrink`): shrink before violate.
                    if self.opts.forced_shrink && !vs.is_empty() {
                        if let Some((its, neck, fix, v2)) = self.shrink_forced(w, lid, &pts, &items, &vs) {
                            items = its;
                            vs = v2;
                            // r2: one stretch, one neck entry (at the run end it touches).
                            fnecks.extend(neck);
                            self.stats.forced_shrunk += 1;
                            self.joint.a1_wires.insert(w);
                            self.joint.a1_mono += usize::from(fix.mono_nm > 0.0);
                            self.joint.a1_mono_nm += fix.mono_nm;
                            self.joint.a1_stubs += fix.stubs;
                        }
                    }
                    let (ps, pe) = (r == 0, r == plan.layers.len() - 1);
                    let first = committed.len();
                    for it in &items {
                        let id = self.commit(it);
                        committed.push(id);
                        ids.push(id);
                    }
                    if let Some(&h) = committed.get(first) {
                        self.record_crossings(w, k0, k1, &pts, h);
                    }
                    if let Some((wn, ls)) = neck {
                        if ps {
                            out.necks.push((pts[0], wn, ls[0]));
                        }
                        if pe {
                            out.necks.push((pts[pts.len() - 1], wn, ls[1]));
                        }
                    }
                    out.necks.extend(fnecks);
                    run_prov("forced", w, wi.net, layer, src, ftag, &pts);
                    let n_items = items.len();
                    out.items.extend(items);
                    out.runs.push((layer, pts));
                    out.run_src.push(if self.opts.realise != "erbs" { super::joint::RunSrc::Seq } else { super::joint::RunSrc::Forced { erbs: src == "erbs", class: cut } });
                    let base = self.run_src(w, "forced_run", src, ftag, if self.opts.realise != "erbs" { "" } else { cut.name() }, r, last_reason.contains("wrong side"), pneck.is_some(), &out.items[out.items.len() - n_items..]);
                    self.tag_viols(&mut vs, &base);
                    out.viol.extend(vs);
                    ok = true;
                }
            }
            // A reserved band no attempt drew from leaves with its run.
            for h in adopt.take().into_iter().flatten().chain(res.take().map(|x| x.handles).into_iter().flatten()) {
                self.uncommit(h);
            }
            if !ok {
                // Never a blank reason (M6 F-1): when no attempt produced copper to check (the
                // exact path had no result and no later attempt ran), the wire still fails as a
                // drawing failure, so the never-drop pass draws it (its key is "drawing:") and the
                // report names why.
                last_reason = failed_run_reason(std::mem::take(&mut last_reason), self.model, lid);
                if self.quiet {
                    for id in committed {
                        self.uncommit(id);
                    }
                    out.ok = false;
                    out.reason = last_reason;
                    out.items.clear();
                    out.runs.clear();
                    return out;
                }
                if let (Some(v), Some(pts)) = (last_viol.as_ref(), tried.last()) {
                    let pts = pts.clone();
                    self.diagnose(w, layer, k0, k1, s, e, v, &pts);
                }
                if std::env::var("TOPO_DUMP").is_ok() {
                    self.fails.push((w, layer, tried, last_reason.clone()));
                }
                for id in committed {
                    self.uncommit(id);
                }
                out.ok = false;
                out.reason = last_reason;
                out.items.clear();
                out.runs.clear();
                return out;
            }
        }
        // exist-term: the planned leads of continued dangling ends (the wire's terminal is the
        // lead's end; the lead joins it to the hand-started track along its direction).
        let bd = self.b;
        for (end, ob) in [(0usize, wi.pa), (1, wi.pb)] {
            let Some(k) = bd.kept_ends.iter().find(|k| k.obs == ob && ob != super::state::NO_OBS) else { continue };
            let Some((q, d)) = k.lead else { continue };
            // tw4 T3 (`dangling_lead_choice`): up to the start point the plan chose on the lead
            // (its end without the option).
            let q = if k.choices.is_empty() { q } else { k.lead_point_t(&bd.proj, bd.proj.pos[if end == 0 { wi.a } else { wi.b }]).map_or(q, |t| k.at + d * t) };
            // r4: up to the via the plan's layer change put on the lead (none: it is the hand
            // end itself). r5: when another wire starts at the same end, the full lead (the
            // other wire's lead goes on to `q`; two collinear leads from one end, one shorter,
            // are copper KiCad's Cleanup merges, badge I2C_SDA); the via sits on it. T3: the
            // farthest of the sharing wires' start points.
            let shared = self.st.wires.iter().enumerate().any(|(x, o)| x != w && o.alive && (o.pa == ob || o.pb == ob));
            let q = if shared && !k.choices.is_empty() {
                let far = self.st.wires.iter().filter(|o| o.alive).flat_map(|o| [(o.pa, o.a), (o.pb, o.b)]).filter(|&(x, _)| x == ob).filter_map(|(_, v)| k.lead_point_t(&bd.proj, bd.proj.pos[v])).fold(0.0, f64::max);
                k.at + d * far.max((q - k.at).len())
            } else if shared {
                q
            } else {
                self.lead_via.get(&(w, end)).copied().unwrap_or(q)
            };
            if (q - k.at).len() < super::board::LEAD_MIN_NM {
                continue;
            }
            let Some(layer) = bd.proj.obstacles[ob].layers.single() else { continue };
            let it = track_item(wi.net, self.layer_id(layer), k.w, k.at, q, Some(w));
            let id = self.commit(&it);
            committed.push(id);
            ids.push(id);
            out.items.push(it);
            out.lead_nm += (q - k.at).len();
        }
        for &(_, p, sz) in vias.iter().filter(|v| v.2.d > 0) {
            out.items.push(via_item(wi.net, p, sz, Some(w)));
            out.vias.push((p, sz));
        }
        // The violations of its vias placed by force (only those it still has).
        if let Some(vv) = self.via_viol.get(&w) {
            out.viol.extend(vv.iter().filter(|x| vias.iter().any(|v| (v.1 - x.at).len() < 2.0)).cloned());
        }
        out.handles = committed;
        self.drawn_once[w] = true;
        out.length_nm = out.runs.iter().map(|(_, p)| p.windows(2).map(|x| (x[1] - x[0]).len()).sum::<f64>()).sum::<f64>() + out.lead_nm;
        out.ok = true;
        out
    }

    /// Replace run `r` of the drawn wire `wo` by the polyline `pts` (same layer, math frame) when
    /// its tracks pass the checker against everything else drawn and graze no pad of the wire's
    /// own net; else the old tracks stay. Only for wires whose track items are exactly the plain
    /// [`Drawer::run_items`] of their runs (no necked or split widths) and no violations: returns
    /// `false` for any other. O(run segments x checker query).
    pub fn refit_run(&mut self, wo: &mut WireOut, r: usize, pts: Vec<Vec2>) -> bool {
        let w = wo.wire;
        let counts: Vec<usize> = wo.runs.iter().map(|(l, p)| self.run_items(w, *l, p).len()).collect();
        let n_tracks = wo.items.iter().filter(|i| matches!(i, Item::Track(_))).count();
        // A wire drawn with violations (never drop) keeps its copper: its violations belong to it.
        if r >= wo.runs.len() || !wo.viol.is_empty() || counts.iter().sum::<usize>() != n_tracks || n_tracks != wo.handles.len() || !wo.necks.is_empty() || !wo.items[..n_tracks].iter().all(|i| matches!(i, Item::Track(_))) {
            return false;
        }
        let off: usize = counts[..r].iter().sum();
        let (lo, hi) = (off, off + counts[r]);
        for &h in &wo.handles[lo..hi] {
            self.uncommit(h);
        }
        let layer = wo.runs[r].0;
        let new_items = self.run_items(w, layer, &pts);
        let ok = !new_items.is_empty() && self.checker.check_against(&self.idx, &new_items, &[]).violations.is_empty() && self.track_own_pad_ok(w, &new_items);
        let keep: Vec<Item> = if ok { new_items } else { wo.items[lo..hi].to_vec() };
        let hs: Vec<usize> = keep.iter().map(|it| self.commit(it)).collect();
        wo.handles.splice(lo..hi, hs);
        if ok {
            wo.items.splice(lo..hi, keep);
            wo.runs[r].1 = pts;
            wo.length_nm = wo.runs.iter().map(|(_, p)| p.windows(2).map(|x| (x[1] - x[0]).len()).sum::<f64>()).sum();
        }
        ok
    }

    /// Whether [`Drawer::refit_run`]`(wo, r, pts)` would be accepted, changing nothing (fuse-octi r4,
    /// M1: the fuse ranks its ends by their best LEGAL gain): the same preconditions and the same
    /// checker query, with the run's own tracks skipped instead of taken out. O(run segments x
    /// checker query).
    pub fn refit_legal(&self, wo: &WireOut, r: usize, pts: &[Vec2]) -> bool {
        let w = wo.wire;
        let counts: Vec<usize> = wo.runs.iter().map(|(l, p)| self.run_items(w, *l, p).len()).collect();
        let n_tracks = wo.items.iter().filter(|i| matches!(i, Item::Track(_))).count();
        if r >= wo.runs.len() || !wo.viol.is_empty() || counts.iter().sum::<usize>() != n_tracks || n_tracks != wo.handles.len() || !wo.necks.is_empty() || !wo.items[..n_tracks].iter().all(|i| matches!(i, Item::Track(_))) {
            return false;
        }
        let off: usize = counts[..r].iter().sum();
        let mut skip: Vec<usize> = wo.handles[off..off + counts[r]].iter().map(|&h| self.handles[h].0).collect();
        skip.sort_unstable();
        let new_items = self.run_items(w, wo.runs[r].0, pts);
        !new_items.is_empty() && self.checker.check_against(&self.idx, &new_items, &skip).violations.is_empty() && self.track_own_pad_ok(w, &new_items)
    }

    /// The best LEGAL fuse of run `r` of `wo` onto one trunk (fuse-octi r4, M1): the candidates of
    /// [`fuse_candidates`] of the [`FUSE_VERTEX_CAP`] best vertices ([`fuse_cap`]) in gain order, the
    /// first one that `skip` lets through (`skip(gain, i, points)`: the caller's rules, an anchor on
    /// the cut part, `junction_pad_mm`, a step refused before) and that [`Drawer::refit_legal`]
    /// accepts. `run` starts at the end that fuses (`rev`: the run is drawn the other way, end 1);
    /// `trunk` = the trunk's polylines on the run's layer; `ws_mm`, `jw`, `style` as for
    /// [`fuse_candidates`]. Returns `(price gain, length gain mm, i, new run as drawn)`.
    /// O(candidates x checker query).
    #[allow(clippy::too_many_arguments)]
    pub fn fuse_best_legal(&self, wo: &WireOut, r: usize, run: &[Vec2], rev: bool, trunk: &[&[Vec2]], ws_mm: f64, jw: f64, style: Option<(&super::octi::OctiCfg, f64)>, skip: &mut dyn FnMut(f64, usize, &[Vec2]) -> bool) -> Option<(f64, f64, usize, Vec<Vec2>)> {
        for (gain, len_gain, i, mut pts) in fuse_cap(fuse_candidates(run, trunk, ws_mm, jw, style), FUSE_VERTEX_CAP) {
            if skip(gain, i, &pts) {
                continue;
            }
            if rev {
                pts.reverse();
            }
            if self.refit_legal(wo, r, &pts) {
                return Some((gain, len_gain, i, pts));
            }
        }
        None
    }

    /// The rule violations of the tracks of `wo` (committed, plain tracks first in its items)
    /// against the board and the rest of the drawing (its own tracks skipped): the re-check of
    /// the moves kept after a partial undo of the fuse pour check compares it before and after
    /// the undo (verify V3). A wire's own forced violations (never drop) are in both counts, so they
    /// undo nothing while they stay; one that an undone move had cleared and the undo brings back
    /// raises the count, and that counts as a clash (the kept move is undone too: safe, it goes
    /// back toward the drawing's own copper).
    /// O(tracks x query).
    pub fn wire_viols(&self, wo: &WireOut) -> usize {
        let n = wo.handles.len();
        let mut skip: Vec<usize> = wo.handles.iter().map(|&h| self.handles[h].0).collect();
        skip.sort_unstable();
        self.checker.check_against(&self.idx, &wo.items[..n], &skip).violations.len()
    }

    /// Whether the tracks of `wo` keep every rule against the rest of the drawing
    /// ([`Self::wire_viols`] is 0). O(tracks x query).
    pub fn wire_clear(&self, wo: &WireOut) -> bool {
        self.wire_viols(wo) == 0
    }

    /// Whether every handle of `wo` names a live item of the drawing's index, one per track
    /// (the self-check after the fuse pour check's undo). O(handles).
    pub fn handles_hold(&self, wo: &WireOut) -> bool {
        wo.handles.iter().all(|&h| self.handles.get(h).is_some_and(|&(_, e)| self.ents.get(e).is_some_and(|en| en.live)))
    }

    /// Undo of [`Drawer::refit_run`]s (the fuse pour check, [`super::fusepour`]): the tracks of
    /// `cur` leave the drawn copper and those of `saved` (the same wire before the refits) come
    /// back; returns `saved` with its new handles. Both have plain tracks behind their handles,
    /// first in their items (what `refit_run` accepts and keeps). O(tracks x index update).
    pub fn restore_wire(&mut self, cur: &WireOut, saved: &WireOut) -> WireOut {
        for &h in &cur.handles {
            self.uncommit(h);
        }
        let mut wo = saved.clone();
        wo.handles = saved.items[..saved.handles.len()].iter().map(|it| self.commit(it)).collect();
        wo
    }

    /// The reference paths of the round-i class test ([`ClassRef`]). O(passes of all wires).
    fn build_class_ref(&self) -> ClassRef {
        let st = self.st;
        let mut paths: Vec<(WireId, usize, Vec<Vec2>)> = Vec::new();
        for (o, oi) in st.wires.iter().enumerate() {
            if !oi.alive {
                continue;
            }
            let runs = self.erbs.get(&o).map_or(&[][..], |v| v.as_slice());
            for rb in runs {
                paths.push((o, rb.layer, rb.path.clone()));
            }
            // Its planned path on the layers it has no ERBS on.
            let n = oi.passes.len();
            let lay = |j: usize| -> i32 { oi.passes.get(j).map_or(-2, |&q| self.prov_layer.get(q).copied().unwrap_or(-2)) };
            let plan = self.plan_layers.get(o).map_or(&[][..], |v| v.as_slice());
            let mut cur: BTreeMap<usize, Vec<Vec2>> = BTreeMap::new();
            for ok in 0..=n {
                let ls: Vec<usize> = ref_trav_layers(n, ok, &lay, plan).into_iter().filter(|l| !runs.iter().any(|rb| rb.layer == *l)).collect();
                for l in cur.keys().copied().collect::<Vec<_>>() {
                    if !ls.contains(&l) {
                        if let Some(p) = cur.remove(&l) {
                            paths.push((o, l, p));
                        }
                    }
                }
                for &l in &ls {
                    let e = cur.entry(l).or_default();
                    let c = self.plan_chord(o, ok);
                    let skip = usize::from(!e.is_empty());
                    e.extend_from_slice(&c[skip..]);
                }
            }
            paths.extend(cur.into_iter().map(|(l, p)| (o, l, p)));
        }
        ClassRef::new(paths)
    }

    /// P6 (c): the drawn run `pts` of wire `w` (traversals `k0..=k1`, on `layer`) pulled taut
    /// inside its own corridor: a vertex goes where the straight leg between its neighbours keeps
    /// every rule (the checker against all copper accepted so far), keeps the wire on the same
    /// side of every wire not drawn yet (the parity of its crossings with their nominal chords on
    /// this layer in the run's triangles, and with the walls of their vias, [`Drawer::via_walls`],
    /// is unchanged: the homotopy class stays) and, in the octilinear style, makes no 0/45/90
    /// degree run any-angle. Repeated until nothing changes. A rubber band has no bumps: the
    /// drawn wire is the shortest path of its homotopy class (Dayan 1997, pp. 15-16 and 22, the
    /// spokes keep the spacing; Popov & Popov 2012, TopoR's minimum-length wire path under the
    /// design rules), so the +-0.03 mm bumps and the spikes the lane walls left (P6 evidence,
    /// interleaved_boundary) cannot survive, and KiCad's Cleanup finds nothing to merge there.
    /// `None` when no vertex goes. O(passes x vertices x checker query).
    #[allow(clippy::type_complexity)]
    pub(super) fn pull_taut(&self, w: WireId, k0: usize, k1: usize, layer: usize, lid: u32, pts: &[Vec2], pneck: &Option<super::exact::Neck>) -> Option<(Vec<Vec2>, Vec<Item>, Option<(f64, [f64; super::exact::NECK_ZONES])>)> {
        let cur = self.pull_pts(w, k0, k1, layer, pts, None)?;
        self.realise(w, layer, lid, &cur, pneck).map(|(its, pn)| (cur, its, pn))
    }

    /// The vertex removal of [`Drawer::pull_taut`]; with `forced = Some(price)` (a run drawn by
    /// force, never drop) a leg is taken when the whole run's violation price stays at most
    /// `price` (then lowered to the new one) instead of when the leg is legal.
    /// Test access to the forced pull (P6 c): `pts` pulled at its own violation key, as the
    /// forced run's call does.
    #[cfg(test)]
    pub(super) fn pull_forced(&self, w: WireId, k0: usize, k1: usize, layer: usize, pts: &[Vec2]) -> Option<Vec<Vec2>> {
        let its = self.run_items(w, layer, pts);
        let rep = self.checker.check_against(&self.idx, &its, &[]);
        let mut vs = self.viols_of(&its, &rep);
        vs.extend(self.graze_viols(w, &its));
        self.pull_pts(w, k0, k1, layer, pts, Some(viol_key(&vs)))
    }

    fn pull_pts(&self, w: WireId, k0: usize, k1: usize, layer: usize, pts: &[Vec2], mut forced: Option<(f64, f64)>) -> Option<Vec<Vec2>> {
        if pts.len() < 3 {
            return None;
        }
        let st = self.st;
        let wi = &st.wires[w];
        // The guards: undrawn wires' chords on this layer and their via walls.
        let mut guards: Vec<(Vec2, Vec2)> = self.via_walls(w, k0, k1.min(wi.tris.len().saturating_sub(1)), layer);
        let lay = |o: WireId, j: usize| -> i32 { st.wires[o].passes.get(j).map_or(-2, |&q| self.prov_layer.get(q).copied().unwrap_or(-2)) };
        for k in k0..=k1.min(wi.tris.len().saturating_sub(1)) {
            for &(o, ok) in &st.tri_trav[wi.tris[k]] {
                let oi = &st.wires[o];
                if o == w || !oi.alive || oi.net == wi.net || self.drawn_once[o] || self.polys[o].is_empty() {
                    continue;
                }
                let (a, b) = (if ok > 0 { lay(o, ok - 1) } else { -2 }, lay(o, ok));
                let on = |x: i32| x == layer as i32;
                if !(on(a) || on(b)) {
                    continue;
                }
                let c = self.plan_chord(o, ok);
                guards.extend(c.windows(2).map(|x| (x[0], x[1])));
            }
        }
        // `seg_proper`, not the exact `proper_crossings` (round j, parked): the guards are single
        // segments whose ends (pass points, via centres) a run may touch, where the translation
        // of the run decides arbitrarily (bornhack +3V3 at main 385e043).
        let cross = |poly: &[Vec2], g: &(Vec2, Vec2)| poly.windows(2).filter(|x| seg_proper(x[0], x[1], g.0, g.1)).count() % 2;
        // The planned room of those wires (reserve planned room): a shortcut that reaches deeper
        // into the corridor of an undrawn wire's planned path (its taut and packed chords, half
        // its width plus the clearance either side) than the two legs it replaces is not taken.
        // Priced like `planned_room` (`pitch x (x + x^2)`, `x = depth / pitch`), compared, no wall.
        // Not in a forced run: there a shortcut is taken only when it lowers or keeps the run's
        // violations and adds or deepens no short ([`viol_key`]); measured on qmtech, the room
        // test there kept forced runs on their violations (new DRC 177 -> 658, shorts 62 -> 241),
        // without it 245 / 45 (main 434 / 104), planned_crossing_window cycles stays short-free.
        let mut corr: Vec<(Vec<Vec2>, f64, f64)> = Vec::new();
        let (ka, kb) = (k0.saturating_sub(1), (k1 + 1).min(wi.tris.len().saturating_sub(1)));
        for k in ka..=kb {
            for &(o, ok) in &st.tri_trav[wi.tris[k]] {
                let oi = &st.wires[o];
                if o == w || !oi.alive || oi.net == wi.net || self.drawn_once[o] || self.polys[o].is_empty() {
                    continue;
                }
                let (a, b) = (if ok > 0 { lay(o, ok - 1) } else { -2 }, lay(o, ok));
                if !(a == layer as i32 || b == layer as i32) {
                    continue;
                }
                let s = oi.s.max(wi.s);
                let reach = 0.5 * wi.w + s + 0.5 * oi.w;
                let pitch = (oi.w + s).max(1.0);
                corr.push((self.plan_chord(o, ok), reach, pitch));
                let taut = self.taut_chord(o, ok);
                if taut.len() >= 2 {
                    corr.push((taut, reach, pitch));
                }
            }
        }
        let room_price = |legs: &[Vec2]| -> f64 {
            let mut tot = 0.0;
            for (c, reach, pitch) in &corr {
                let d = legs.windows(2).flat_map(|l| c.windows(2).map(move |x| seg_seg_dist(l[0], l[1], x[0], x[1]))).fold(f64::INFINITY, f64::min);
                let depth = reach - d;
                if depth > 0.0 {
                    let x = depth / pitch;
                    tot += pitch * (x + x * x);
                }
            }
            tot
        };
        let octi = self.octi.on;
        // first-draw-45 P5: with the octilinear style a corner goes only where the shortcut is
        // cheaper on the octilinear DP's scale, length plus the `any_angle` price of the legs
        // ([`super::octi::any_price`]): an octilinear leg is not traded for free angle for a little
        // length (esp / qmtech: the DP's 45 degree legs next to a kept taut piece were pulled into
        // one free-angle leg). Graded, not a cutoff: a shortcut saving more than its any-angle
        // price is taken. Before (two octilinear legs kept, any other corner removed).
        let any_nm = self.octi.any_nm;
        let style = |q: &[Vec2]| -> f64 { poly_len(q) + any_nm * super::octi::any_price(q) };
        let mut cur = pts.to_vec();
        let mut changed = false;
        for _pass in 0..8 {
            let mut any = false;
            let mut i = 1;
            while i + 1 < cur.len() {
                let (a, b, c) = (cur[i - 1], cur[i], cur[i + 1]);
                let p5 = !super::octi::fd45_off("p5");
                // A corner next to an octilinear leg whose shortcut is free angle (P5); before P5
                // only one between two octilinear legs.
                let octi_corner = octi
                    && super::octi::octi_dir(c - a).is_none()
                    && if p5 {
                        super::octi::octi_dir(b - a).is_some() || super::octi::octi_dir(c - b).is_some()
                    } else {
                        super::octi::octi_dir(b - a).is_some() && super::octi::octi_dir(c - b).is_some()
                    };
                // r4 (qmtech PS2_DAT1 RCA): in a forced run the corner is judged below on the forced
                // run's own scale, its violations included (`forced_form_key`): the style price
                // alone kept four short legs across a VIN5V track the base pull had merged into one
                // crossing (KiCad: one crossing became a crossing and three shorting items).
                let keep_octi = octi_corner && (!p5 || (forced.is_none() && style(&[a, c]) >= style(&[a, b, c])));
                if keep_octi || (c - a).len2() < 1.0 || guards.iter().any(|g| cross(&[a, b, c], g) != cross(&[a, c], g)) || (forced.is_none() && room_price(&[a, c]) > room_price(&[a, b, c]) + 1.0) {
                    i += 1;
                    continue;
                }
                let ok = match forced {
                    None => self.checker.check_against(&self.idx, &self.run_items(w, layer, &[a, c]), &[]).violations.is_empty(),
                    Some(price) => {
                        let mut cand = cur.clone();
                        cand.remove(i);
                        let its = self.run_items(w, layer, &cand);
                        let rep = self.checker.check_against(&self.idx, &its, &[]);
                        let mut vs = self.viols_of(&its, &rep);
                        vs.extend(self.graze_viols(w, &its));
                        let k2 = viol_key(&vs);
                        // P5 in a forced run: a corner next to an octilinear leg goes when the
                        // shortcut is cheaper on the forced scale (shorts first, then `violation`
                        // x amount + length + `any_angle` price, `forced_form_key`'s weights).
                        let pays = !(octi_corner && p5) || k2.0 < price.0 - 1e-9 || self.octi.viol_nm * k2.1 + style(&cand) < self.octi.viol_nm * price.1 + style(&cur) - 1e-9;
                        if pays && key_le(k2, price) {
                            forced = Some(k2);
                            true
                        } else {
                            false
                        }
                    }
                };
                if ok {
                    cur.remove(i);
                    any = true;
                    changed = true;
                } else {
                    i += 1;
                }
            }
            if !any {
                break;
            }
        }
        changed.then_some(cur)
    }

    /// The tracks of the run `pts` of wire `w` when they pass the checker as the drawing's attempts
    /// do: at the nominal width, else with the plan's necks (`pneck`, M-C) at the widest width
    /// that passes; and no own-net pad grazed. `(items, plan neck width and lengths)`.
    #[allow(clippy::type_complexity)]
    pub(super) fn realise(&self, w: WireId, layer: usize, lid: u32, pts: &[Vec2], pneck: &Option<super::exact::Neck>) -> Option<(Vec<Item>, Option<(f64, [f64; super::exact::NECK_ZONES])>)> {
        let items = self.run_items(w, layer, pts);
        let (items, pn) = if self.checker.check_against(&self.idx, &items, &[]).violations.is_empty() {
            (items, None)
        } else {
            let (wn, its, ls) = self.plan_neck_run(w, lid, pts, pneck.as_ref()?)?;
            (its, Some((wn, ls)))
        };
        self.track_own_pad_ok(w, &items).then_some((items, pn))
    }

    /// Track items of the run polyline `pts` of wire `w` on `layer`: the wire's width, or the
    /// per-segment width of [`Drawer::widths`].
    pub(super) fn run_items(&self, w: WireId, layer: usize, pts: &[Vec2]) -> Vec<Item> {
        let wi = &self.st.wires[w];
        let lid = self.layer_id(layer);
        match &self.widths {
            None => Self::items_of(wi.net, lid, wi.w, pts, Some(w)),
            Some(x) => pts.windows(2).filter(|p| (p[1] - p[0]).len2() > 1.0).map(|p| track_item(wi.net, lid, (x.seg)(w, layer, p[0], p[1]), p[0], p[1], Some(w))).collect(),
        }
    }

    /// Width of the segment `a-b` of wire `w` on `layer` (see [`Drawer::widths`]); `dflt` without.
    pub(super) fn seg_width(&self, w: WireId, layer: usize, a: Vec2, b: Vec2, dflt: f64) -> f64 {
        match &self.widths {
            Some(x) if layer != usize::MAX => (x.seg)(w, layer, a, b),
            _ => dflt,
        }
    }

    /// Track items of the polyline `pts` at width `w`.
    fn items_of(net: i32, lid: u32, w: f64, pts: &[Vec2], wire: Option<usize>) -> Vec<Item> {
        pts.windows(2).filter(|p| (p[1] - p[0]).len2() > 1.0).map(|p| track_item(net, lid, w, p[0], p[1], wire)).collect()
    }

    /// Narrowest width a necked stretch may take (nm): the net's floor (KiCad minimum track width,
    /// custom-rule minimum) and twice KiCad's minimum connection width, which keeps the copper
    /// connection at a pad entered obliquely above that rule (KiCad measures the junction, not the
    /// track); never above the wire's own width.
    pub(super) fn neck_floor(rules: &crate::model::Rules, net: i32, w: f64) -> f64 {
        (rules.min_track_width.max(rules.net_rule(net).min_width).max(2 * rules.min_connection) as f64).min(w)
    }

    /// Neck zones of a run (item 3): where the wire may be narrowed to the net's floor, at the pad
    /// ends of the run (`ps` start pad, `pe` end pad). `None` without `neck_to_pad` or room to narrow.
    pub(super) fn neck_zones(&self, w: WireId, ps: bool, pe: bool) -> Option<super::exact::Neck> {
        let wi = &self.st.wires[w];
        // Only a pad end necks (a node terminal, `NO_OBS`, has no pad).
        let (ps, pe) = (ps && wi.pa != super::state::NO_OBS, pe && wi.pb != super::state::NO_OBS);
        let rules = &self.model.rules;
        let min_w = Self::neck_floor(rules, wi.net, wi.w);
        if !self.opts.neck_to_pad || min_w >= wi.w || !(ps || pe) {
            return None;
        }
        let pr = &self.b.proj;
        let ext = |ob: usize| pr.obstacles[ob].poly.iter().map(|&p| (p - pr.obstacles[ob].center).len()).fold(0.0, f64::max);
        let cap = |ob: usize| if self.opts.neck_max_nm > 0 { self.opts.neck_max_nm as f64 } else { 4.0 * ext(ob) };
        // Euclidean radius half the neck length: the neck is cut by path length, which is longer.
        let zone = |ob: usize| (pr.pos[if ob == wi.pa { wi.a } else { wi.b }], 0.5 * cap(ob));
        let mut ends = [None; super::exact::NECK_ZONES];
        ends[0] = ps.then(|| zone(wi.pa));
        ends[1] = pe.then(|| zone(wi.pb));
        Some(super::exact::Neck { min_w, ends })
    }

    /// The necked stretches of wire `w` as the plan decided them (M-C, [`State::refit_wire`]):
    /// a zone around a pad end reaching the far side of the last necked pass's edge (the drawn
    /// crossing lies somewhere on it), the whole board when the wire is necked end to end, and a
    /// zone around each run of necked passes in between (its edges' extent). `None`: the plan
    /// keeps the wire at its nominal width.
    pub(super) fn plan_neck(&self, w: WireId) -> Option<super::exact::Neck> {
        let st = self.st;
        let wi = &st.wires[w];
        let (na, nb, mid) = st.neck_runs(w);
        if std::env::var("TOPO_NECKDBG").is_ok() {
            eprintln!("PLANNECK wire {w} n {} na {na} nb {nb} mid {mid:?} widths {:?}", wi.passes.len(), wi.passes.iter().map(|&p| st.pass_w[p]).collect::<Vec<_>>());
        }
        if na == 0 && nb == 0 && mid.is_empty() {
            return None;
        }
        let pr = &self.b.proj;
        let n = wi.passes.len();
        let min_w = st.w_min(wi.net, wi.w);
        // The zone reaches one pitch (width + clearance) past the farthest point of the last
        // necked pass's edge: the full-width wire beyond it keeps its clearance to the obstacles
        // that make the bottleneck.
        let far = |k: usize, c: Vec2| -> f64 {
            let [p, q] = pr.edges[st.passes[wi.passes[k]].edge];
            (pr.pos[p] - c).len().max((pr.pos[q] - c).len()) + wi.w + wi.s
        };
        let mut ends = [None; super::exact::NECK_ZONES];
        if na == n {
            let c = pr.pos[wi.a];
            ends[0] = Some((c, f64::INFINITY));
            return Some(super::exact::Neck { min_w, ends });
        }
        if na > 0 {
            let c = pr.pos[wi.a];
            ends[0] = Some((c, (0..na).map(|k| far(k, c)).fold(0.0, f64::max)));
        }
        if nb > 0 {
            let c = pr.pos[wi.b];
            ends[1] = Some((c, (n - nb..n).map(|k| far(k, c)).fold(0.0, f64::max)));
        }
        // Runs of necked passes in between (without `neck_from_pad`): one zone each.
        let mut z = 2;
        let mut i = 0;
        while i < mid.len() && z < super::exact::NECK_ZONES {
            let mut j = i;
            while j + 1 < mid.len() && mid[j + 1] == mid[j] + 1 {
                j += 1;
            }
            let pts: Vec<Vec2> = (mid[i]..=mid[j]).flat_map(|k| pr.edges[st.passes[wi.passes[k]].edge].map(|v| pr.pos[v])).collect();
            let c = pts.iter().fold(Vec2::default(), |a, &p| a + p) * (1.0 / pts.len() as f64);
            ends[z] = Some((c, pts.iter().map(|&p| (p - c).len()).fold(0.0, f64::max) + wi.w + wi.s));
            z += 1;
            i = j + 1;
        }
        Some(super::exact::Neck { min_w, ends })
    }

    /// Track items of the polyline `pts` with the parts inside the zones of `neck` at width
    /// `wn` (each segment cut where it crosses a zone's circle) and the rest at the wire's width;
    /// with the necked length per zone. O(segments x zones).
    pub(super) fn neck_items(&self, w: WireId, lid: u32, pts: &[Vec2], neck: &super::exact::Neck, wn: f64) -> (Vec<Item>, [f64; super::exact::NECK_ZONES]) {
        let wi = &self.st.wires[w];
        let mut items = Vec::new();
        let mut lens = [0.0; super::exact::NECK_ZONES];
        for p in pts.windows(2) {
            let (a, b) = (p[0], p[1]);
            let d = b - a;
            let l2 = d.len2();
            if l2 <= 1.0 {
                continue;
            }
            let mut cuts = vec![0.0, 1.0];
            for (c, r) in neck.ends.iter().flatten() {
                if !r.is_finite() {
                    continue;
                }
                // |a + t d - c| = r
                let f = a - *c;
                let (qb, qc) = (f.dot(d), f.len2() - r * r);
                let disc = qb * qb - l2 * qc;
                if disc > 0.0 {
                    let sq = disc.sqrt();
                    for t in [(-qb - sq) / l2, (-qb + sq) / l2] {
                        if t > 0.0 && t < 1.0 {
                            cuts.push(t);
                        }
                    }
                }
            }
            cuts.sort_by(f64::total_cmp);
            for c in cuts.windows(2) {
                let (u, v) = (a + d * c[0], a + d * c[1]);
                if (v - u).len2() <= 1.0 {
                    continue;
                }
                let m = (u + v) * 0.5;
                let zone = neck.ends.iter().position(|z| z.is_some_and(|(cc, r)| (m - cc).len() <= r));
                if let Some(zi) = zone {
                    lens[zi] += (v - u).len();
                }
                items.push(track_item(wi.net, lid, if zone.is_some() { wn } else { wi.w }, u, v, Some(w)));
            }
        }
        (items, lens)
    }

    /// The run drawn with the plan's necks (M-C) at the widest width in `[floor, nominal]` the
    /// checker accepts (TopoR 6.3 manual p. 56: the minimum width only where needed); `None`
    /// when even the floor is rejected. Returns `(width, items, necked length per zone)`.
    #[allow(clippy::type_complexity)]
    fn plan_neck_run(&self, w: WireId, lid: u32, pts: &[Vec2], neck: &super::exact::Neck) -> Option<(f64, Vec<Item>, [f64; super::exact::NECK_ZONES])> {
        let wi = &self.st.wires[w];
        let ok = |wn: f64| {
            let its = self.neck_items(w, lid, pts, neck, wn).0;
            self.checker.check_against(&self.idx, &its, &[]).violations.is_empty()
        };
        if ok(wi.w) {
            let (items, lens) = self.neck_items(w, lid, pts, neck, wi.w);
            return Some((wi.w, items, lens));
        }
        if !ok(neck.min_w) {
            return None;
        }
        let (mut lo, mut hi) = (neck.min_w, wi.w);
        for _ in 0..12 {
            let mid = 0.5 * (lo + hi);
            if ok(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let wn = lo.floor().max(neck.min_w);
        let nk = if self.opts.neck_trim { self.fit_neck(w, lid, pts, neck, wn) } else { *neck };
        let (items, lens) = self.neck_items(w, lid, pts, &nk, wn);
        Some((wn, items, lens))
    }

    /// draw-absorb A1: neck zones over the tracks of `items` (wire `w`) that the violations of
    /// `rep` passing `keep` name: one per track round its middle, half its length plus one pitch
    /// (the trim shrinks it to what the checker needs), the deepest first; more tracks than `free`
    /// zones: one zone over all of them. O(violations).
    pub(super) fn hit_zones(&self, w: WireId, items: &[Item], rep: &CheckReport, keep: &dyn Fn(&Violation) -> bool, free: usize) -> Vec<(Vec2, f64)> {
        let wi = &self.st.wires[w];
        let mut segs: BTreeMap<usize, (i64, Vec2, Vec2)> = BTreeMap::new();
        for v in rep.violations.iter().filter(|v| keep(v)) {
            let Some(Item::Track(t)) = items.get(v.item) else { continue };
            let (a, b) = (t.prim.start(), t.prim.end());
            let (a, b) = (Vec2 { x: f64::from(a.x), y: -f64::from(a.y) }, Vec2 { x: f64::from(b.x), y: -f64::from(b.y) });
            let e = segs.entry(v.item).or_insert((0, a, b));
            e.0 = e.0.max(v.required - v.gap);
        }
        let mut z: Vec<(i64, Vec2, Vec2)> = segs.into_values().collect();
        z.sort_by_key(|p| std::cmp::Reverse(p.0));
        let pitch = wi.w + wi.s;
        if z.len() <= free {
            return z.iter().map(|&(_, a, b)| ((a + b) * 0.5, 0.5 * (b - a).len() + pitch)).collect();
        }
        if free == 0 {
            return Vec::new();
        }
        let n = z.len() as f64;
        let c = z.iter().fold(Vec2::default(), |s, &(_, a, b)| s + (a + b) * 0.5) * (1.0 / n);
        vec![(c, z.iter().map(|&(_, a, b)| (a - c).len().max((b - c).len())).fold(0.0, f64::max) + pitch)]
    }

    /// draw-absorb A1 (`[drawing] forced_shrink`, docs/dev/design/draw-absorb.md): the forced run
    /// `pts` (its `items` all at the wire's width; violations `vs`) necked over the stretches it
    /// violates ([`Drawer::hit_zones`]), at the widest width in `[floor, nominal]`
    /// whose severity key is the floor's, each zone trimmed to what the checker needs and never
    /// ending inside the run's own end pad (`neck_out_of_pads`). TopoR 6.3 manual p. 56 (minimum
    /// width in bottlenecks), p. 72 (shrinkages). `None` unless the key ([`viol_key`]: shorts,
    /// then amount) gets strictly better; a run the plan or a pad end already necks is left to
    /// those necks (never widened). `(items, zones, width, necked length per zone, violations)`.
    /// O(14 checks of the run + the trim).
    #[allow(clippy::type_complexity)]
    fn shrink_forced(&self, w: WireId, lid: u32, pts: &[Vec2], items: &[Item], vs: &[Viol]) -> Option<(Vec<Item>, Option<(Vec2, f64, f64)>, super::joint::A1Fix, Vec<Viol>)> {
        let wi = &self.st.wires[w];
        let min_w = self.st.w_min(wi.net, wi.w);
        if min_w >= wi.w - 0.5 || pts.len() < 2 || items.iter().any(|i| matches!(i, Item::Track(t) if (t.width as f64) < wi.w - 0.5)) {
            return None;
        }
        let rep = self.checker.check_against(&self.idx, items, &[]);
        let zs = self.hit_zones(w, items, &rep, &|_| true, super::exact::NECK_ZONES);
        if zs.is_empty() {
            return None;
        }
        let mut ends = [None; super::exact::NECK_ZONES];
        for (z, v) in ends.iter_mut().zip(zs) {
            *z = Some(v);
        }
        let nk0 = super::exact::Neck { min_w, ends };
        // r2: the copper as drawn: one neck stretch per run, pad stubs kept ([`Drawer::a1_finish`]).
        let eval = |nk: &super::exact::Neck, wn: f64| {
            let (its, fix) = self.a1_finish(w, self.neck_items(w, lid, pts, nk, wn).0);
            let rep = self.checker.check_against(&self.idx, &its, &[]);
            let mut v2 = self.viols_of(&its, &rep);
            v2.extend(self.graze_viols(w, &its));
            (its, fix, v2)
        };
        let better = |k: (f64, f64)| {
            let b = viol_key(vs);
            key_le(k, b) && (k.0 < b.0 - 1e-9 || k.1 < b.1 - 1e-9)
        };
        let kf = viol_key(&eval(&nk0, min_w).2);
        if !better(kf) {
            return None;
        }
        let (mut lo, mut hi) = (min_w, wi.w);
        for _ in 0..12 {
            let mid = 0.5 * (lo + hi);
            if key_le(viol_key(&eval(&nk0, mid).2), kf) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let wn = lo.floor().max(min_w);
        let mut pick: Option<(Vec<Item>, super::joint::A1Fix, Vec<Viol>)> = None;
        for nk in [self.neck_out_of_pads(w, lid, pts, self.trim_neck(w, lid, pts, &nk0, wn)), self.neck_out_of_pads(w, lid, pts, nk0)] {
            let (its, fix, v2) = eval(&nk, wn);
            // The trimmed zones first; the whole zones only where they are strictly better.
            let k = viol_key(&v2);
            if better(k) && pick.as_ref().is_none_or(|p| {
                let b = viol_key(&p.2);
                key_le(k, b) && (k.0 < b.0 - 1e-9 || k.1 < b.1 - 1e-9)
            }) {
                pick = Some((its, fix, v2));
            }
        }
        pick.map(|(its, fix, v2)| {
            let neck = super::joint::neck_of(&its, wi.w);
            (its, neck, fix, v2)
        })
    }

    /// draw-via-neck B (`[drawing] neck_trim`, lead 2026-11-24): the neck width of the run `pts`
    /// with the plan's zones `neck` when no width passes the checker ([`Drawer::plan_neck_run`]
    /// is `None`: a violation the width cannot remove, e.g. a terminal closer to the board edge
    /// than the edge clearance). The widest width in `[floor, nominal]` that adds no violation to
    /// the floor's: every object it violates (the same pad, track, via, hole or edge on the same
    /// layer) is one the floor violates too. Graded, not by count: the floor's violations only
    /// deepen by the width step itself (half the extra width; the run's centre line is fixed), so
    /// the unavoidable one does not grow past what the width costs, and nothing new is hit. The
    /// floor when even a little more width hits something new. TopoR 6.3 manual p. 56: the
    /// minimum width only where needed. O(14 checks of the run).
    fn plan_neck_widest(&self, w: WireId, lid: u32, pts: &[Vec2], neck: &super::exact::Neck) -> f64 {
        let wi = &self.st.wires[w];
        let hit = |wn: f64| -> std::collections::BTreeSet<(String, u32)> {
            let its = self.neck_items(w, lid, pts, neck, wn).0;
            self.checker.check_against(&self.idx, &its, &[]).violations.into_iter().map(|v| (v.against, v.layer)).collect()
        };
        let floor = hit(neck.min_w);
        let ok = |wn: f64| hit(wn).is_subset(&floor);
        if ok(wi.w) {
            return wi.w;
        }
        let (mut lo, mut hi) = (neck.min_w, wi.w);
        for _ in 0..12 {
            let mid = 0.5 * (lo + hi);
            if ok(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        if std::env::var("TOPO_NECKDBG").is_ok() {
            eprintln!("NECKWIDEST wire {w} floor {:.0} -> {:.0} (violated objects {})", neck.min_w, lo.floor(), floor.len());
        }
        lo.floor().max(neck.min_w)
    }

    /// draw-absorb A1b meter: length (nm) of the run `pts` of wire `w` inside the zones of `neck`
    /// where its full width takes planned room of wires not drawn yet that the width `wn` leaves
    /// free (the room price of [`Drawer::via_plan_price`], as [`Drawer::fit_neck`] grows a neck
    /// by it). 0 = the full width there costs no other trace room. O(samples x traversals).
    pub(super) fn room_taken(&self, w: WireId, pts: &[Vec2], neck: &super::exact::Neck, wn: f64) -> f64 {
        let wi = &self.st.wires[w];
        let last = wi.tris.len().saturating_sub(1);
        let mut l = 0.0;
        for s in pts.windows(2) {
            let sl = (s[1] - s[0]).len();
            let n = ((sl / 5.0e4).ceil() as usize).max(1);
            for j in 0..n {
                let q = s[0] + (s[1] - s[0]) * ((j as f64 + 0.5) / n as f64);
                let inside = neck.ends.iter().flatten().any(|&(c, r)| (q - c).len() <= r);
                if inside && self.via_plan_price(w, 0, last, q, wi.w) > self.via_plan_price(w, 0, last, q, wn) {
                    l += sl / n as f64;
                }
            }
        }
        l
    }

    /// draw-via-neck B (`[drawing] neck_trim`): the plan's neck zones of the run `pts` (necked to
    /// `wn`) cut to where the room ends. Each zone first shrinks to the smallest radius the checker
    /// accepts ([`Drawer::trim_neck`]; a zone over the whole wire is tried from either end, the
    /// shorter neck wins); then, between that radius and the plan's, the radius of least price
    /// `necked length + sum over the full-width part of (room price at full width - at wn)` is
    /// taken, the room price being the planned room of the wires not drawn yet
    /// ([`Drawer::via_plan_price`] per sample, over the wire's pitch): the neck reaches further
    /// only where another trace needs that room (TopoR 6.3 manual p. 56, the minimum width only
    /// where needed). Graded, no cutoff. O(zones x 14 checks of the run + zones x 65 x segments).
    pub(super) fn fit_neck(&self, w: WireId, lid: u32, pts: &[Vec2], neck: &super::exact::Neck, wn: f64) -> super::exact::Neck {
        self.fit_neck_with(w, lid, pts, neck, wn, false)
    }

    /// [`Drawer::fit_neck`]; `grow_trimmed` (draw-absorb A1b, `[drawing] reserved_keep_neck`): a
    /// zone the checker does not need (trimmed away) still grows from radius 0 where the full
    /// width would take planned room of traces not drawn yet (the room the reserved neck kept).
    pub(super) fn fit_neck_with(&self, w: WireId, lid: u32, pts: &[Vec2], neck: &super::exact::Neck, wn: f64, grow_trimmed: bool) -> super::exact::Neck {
        let wi = &self.st.wires[w];
        let ext = |c: Vec2| pts.iter().map(|&p| (p - c).len()).fold(0.0, f64::max) + 1.0;
        let len_of = |n: &super::exact::Neck| self.neck_items(w, lid, pts, n, wn).1.iter().sum::<f64>();
        // A zone over the whole wire: from the end where the shorter neck does.
        let mut base = *neck;
        if let Some(i) = neck.ends.iter().position(|z| z.is_some_and(|(_, r)| !r.is_finite())) {
            let pr = &self.b.proj;
            let mut best: Option<(f64, super::exact::Neck)> = None;
            for c in [pr.pos[wi.a], pr.pos[wi.b]] {
                let mut n = *neck;
                n.ends[i] = Some((c, ext(c)));
                let t = self.trim_neck(w, lid, pts, &n, wn);
                let l = len_of(&t);
                if best.as_ref().is_none_or(|b| l < b.0 - 1.0) {
                    best = Some((l, n));
                }
            }
            base = best.map_or(base, |b| b.1);
        }
        let mut nk = self.trim_neck(w, lid, pts, &base, wn);
        let trimmed_dbg = nk;
        let room = self.octi.room;
        let nv = |n: &super::exact::Neck| self.checker.check_against(&self.idx, &self.neck_items(w, lid, pts, n, wn).0, &[]).violations.len();
        let nv0 = nv(&nk);
        let ok = |n: &super::exact::Neck| nv(n) <= nv0;
        let last = wi.tris.len().saturating_sub(1);
        let pitch = (wi.w + wi.s).max(1.0);
        // Room price of the full width over the necked width at `q`, per nm of track.
        let extra = |q: Vec2| ((self.via_plan_price(w, 0, last, q, wi.w) - self.via_plan_price(w, 0, last, q, wn)) / pitch).max(0.0);
        for i in 0..super::exact::NECK_ZONES {
            let Some((c, r1)) = base.ends[i] else { continue };
            let r0 = match nk.ends[i] {
                Some((_, r0)) => r0,
                None if grow_trimmed => 0.0,
                None => continue,
            };
            let r1 = if r1.is_finite() { r1 } else { ext(c) };
            if room <= 0.0 || r1 <= r0 + 1.0 {
                continue;
            }
            // Debug arm `TOPO_FD45_OFF=dvn_growth` (proposed pattern neck_between_signals): trim only.
            if super::octi::fd45_off("dvn_growth") {
                continue;
            }
            // Samples of the run between the trimmed and the plan's circle (50 um apart).
            let mut samples: Vec<(f64, f64)> = Vec::new(); // (distance to c, price x length)
            for s in pts.windows(2) {
                let l = (s[1] - s[0]).len();
                let n = ((l / 5.0e4).ceil() as usize).max(1);
                for j in 0..n {
                    let q = s[0] + (s[1] - s[0]) * ((j as f64 + 0.5) / n as f64);
                    let d = (q - c).len();
                    if d > r0 && d <= r1 {
                        samples.push((d, extra(q) * l / n as f64));
                    }
                }
            }
            if samples.iter().all(|s| s.1 <= 0.0) {
                continue;
            }
            let mut pick = (f64::INFINITY, r0);
            // Radii 25 um apart (at most 64 steps): only lengths, no checker.
            let steps = (((r1 - r0) / 2.5e4).ceil() as usize).clamp(1, 64);
            for j in 0..=steps {
                let r = r0 + (r1 - r0) * j as f64 / steps as f64;
                let mut n = nk;
                n.ends[i] = Some((c, r));
                let necked = self.neck_items(w, lid, pts, &n, wn).1[i];
                let full: f64 = samples.iter().filter(|s| s.0 > r).map(|s| s.1).sum();
                let price = necked + room * full;
                if price < pick.0 - 1.0 {
                    pick = (price, r);
                }
            }
            let mut n = nk;
            n.ends[i] = Some((c, pick.1));
            if pick.1 > r0 && ok(&n) {
                nk = n;
            }
        }
        let nk = self.neck_out_of_pads(w, lid, pts, nk);
        if std::env::var("TOPO_NECKDBG").is_ok() {
            eprintln!("NECKFIT wire {w} wn {wn:.0} plan {:?} trim {:?} fit {:?}", neck.ends.iter().flatten().map(|z| z.1.round()).collect::<Vec<_>>(), trimmed_dbg.ends.iter().flatten().map(|z| z.1.round()).collect::<Vec<_>>(), nk.ends.iter().flatten().map(|z| z.1.round()).collect::<Vec<_>>());
        }
        nk
    }

    /// `neck` with each zone shrunk to the smallest radius at which the run `pts` necked to `wn`
    /// still passes the checker (zone by zone, bisection; a zone the run does not need is
    /// dropped; an unbounded zone starts at the run's extent from its centre). For runs whose
    /// geometry is not the plan's (the forced ones), whose plan zones can reach far past the
    /// bottleneck. O(zones x 13 checks of the run).
    fn trim_neck(&self, w: WireId, lid: u32, pts: &[Vec2], neck: &super::exact::Neck, wn: f64) -> super::exact::Neck {
        let mut nk = *neck;
        // No more violations than with the plan's zones (none for a legal run; a run that keeps
        // some elsewhere, e.g. a forced one, is still trimmed where the neck adds nothing).
        let nv = |n: &super::exact::Neck| self.checker.check_against(&self.idx, &self.neck_items(w, lid, pts, n, wn).0, &[]).violations.len();
        let base = nv(neck);
        let ok = |n: &super::exact::Neck| nv(n) <= base;
        for i in 0..super::exact::NECK_ZONES {
            let Some((c, r)) = nk.ends[i] else { continue };
            let r = if r.is_finite() { r } else { pts.iter().map(|&p| (p - c).len()).fold(0.0, f64::max) + 1.0 };
            nk.ends[i] = None;
            if ok(&nk) {
                continue;
            }
            let (mut lo, mut hi) = (0.0, r);
            for _ in 0..12 {
                let mid = 0.5 * (lo + hi);
                nk.ends[i] = Some((c, mid));
                if ok(&nk) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            nk.ends[i] = Some((c, hi));
        }
        nk
    }

    /// draw-via-neck r6 ([`neck_out_of_pad`]) at both pad ends of the run `pts` of wire `w` on
    /// the layer `lid` (`[drawing] neck_trim` only: main's whole-zone neck is kept as it was).
    /// r7 (verifier r6 F6): only at a pad on the run's layer (a middle run from a via inside an
    /// SMD end pad's outline on the other layer has no pad there). O(zones x run).
    fn neck_out_of_pads(&self, w: WireId, lid: u32, pts: &[Vec2], mut nk: super::exact::Neck) -> super::exact::Neck {
        if !self.opts.neck_trim || super::octi::fd45_off("dvn_pad_out") {
            return nk;
        }
        let wi = &self.st.wires[w];
        let pr = &self.b.proj;
        let li = self.b.signal_layers.iter().position(|&x| x == lid);
        for (end, ob) in [(0usize, wi.pa), (1, wi.pb)] {
            if ob == super::state::NO_OBS || ob >= pr.obstacles.len() || li.is_some_and(|l| !pr.obstacles[ob].layers.has(l)) {
                continue;
            }
            let run: Vec<Vec2> = if end == 0 { pts.to_vec() } else { pts.iter().rev().copied().collect() };
            neck_out_of_pad(&mut nk, &run, &pr.obstacles[ob].poly);
        }
        nk
    }

    /// The polyline of one run from `s` (in traversal `k0`) to `e` (in traversal `k1`); `attempt`
    /// 0 = full intervals, 1 = intervals halved towards nominal, 2 = nominal points.
    #[allow(clippy::too_many_arguments)]
    fn run_polyline(&self, w: WireId, k0: usize, k1: usize, s: Vec2, e: Vec2, attempt: usize, layer: usize) -> Vec<Vec2> {
        let wi = &self.st.wires[w];
        let pre = self.base_polyline(w, k0, k1, s, e, attempt);
        self.repair(wi.net, wi.w, wi.s, layer, pre)
    }

    /// Shopping list 5.10: obstacle `o` is an exit stub ([`Board::exit_stubs`]). O(log stubs).
    pub(super) fn is_exit_stub(&self, o: usize) -> bool {
        self.b.exit_stubs.binary_search_by_key(&o, |x| x.0).is_ok()
    }

    /// exist-term: obstacle `o` is a terminal on existing copper ([`Board::kept_ends`]): the
    /// wires of its own net pass it freely (it lies on their own track copper, or is the
    /// planned lead of one). `kept_ends` are in obstacle order. O(log terminals).
    pub(super) fn is_kept_end(&self, o: usize) -> bool {
        self.b.kept_ends.binary_search_by_key(&o, |k| k.obs).is_ok()
    }

    /// Shopping list 5.10: edge `e` lies inside the copper of `net` with an exit stub at one of its
    /// ends (both ends on obstacles of `net`, one of them an exit stub, [`Board::exit_stubs`]).
    /// O(log stubs).
    pub(super) fn in_own_stub(&self, net: i32, e: EdgeId) -> bool {
        let pr = &self.b.proj;
        let of = |v: VertId| match pr.vkind[v] {
            VertKind::Outline(o) | VertKind::Centre(o) if pr.obstacles[o].net == Some(net) => Some(o),
            _ => None,
        };
        let [p, q] = pr.edges[e];
        match (of(p), of(q)) {
            (Some(a), Some(b)) => self.is_exit_stub(a) || self.is_exit_stub(b),
            _ => false,
        }
    }

    /// The polyline before the repair.
    fn base_polyline(&self, w: WireId, k0: usize, k1: usize, s: Vec2, e: Vec2, attempt: usize) -> Vec<Vec2> {
        let st = self.st;
        let wi = &st.wires[w];
        let mut portals = Vec::with_capacity(k1 - k0);
        let mut nominal = vec![s];
        for k in k0..k1 {
            let pid = wi.passes[k];
            // 5.10: inside the wire's own pad and exit stub the plan's passes are no portals (the
            // copper there is the wire's own; the sliver edges between the two zigzagged the
            // funnel).
            if self.in_own_stub(wi.net, st.passes[pid].edge) {
                continue;
            }
            let (lo, hi, nom) = (self.iv.lo[pid], self.iv.hi[pid], self.iv.nom[pid]);
            let (lo, hi) = match attempt {
                0 => match self.tight_span(pid) {
                    Some((a, b)) => {
                        let [p, q] = self.b.proj.edges[st.passes[pid].edge];
                        let len = (self.b.proj.pos[q] - self.b.proj.pos[p]).len().max(1.0);
                        let (a, b) = ((a / len).clamp(0.0, 1.0), (b / len).clamp(0.0, 1.0));
                        if self.tight {
                            (a, b)
                        } else if lo.max(a) <= hi.min(b) {
                            // The S2 interval, narrowed to what the drawn neighbours left (the
                            // same interval while they keep to theirs).
                            (lo.max(a), hi.min(b))
                        } else {
                            // A neighbour drawn outside its own interval (its exact or
                            // octilinear run) took this one: the room it left, in the order of
                            // the lane, instead of a portal through its copper (wiccon 3.3v
                            // J1.1 -> U1.3: the S2 portals at the U1 end lay inside the bundle
                            // drawn hugging the pads, the run had no legal geometry).
                            (a, b)
                        }
                    }
                    None => (lo, hi),
                },
                1 => (nom - 0.5 * (nom - lo), nom + 0.5 * (hi - nom)),
                _ => (nom, nom),
            };
            let (l, r) = self.portal(w, k, lo, hi);
            portals.push((l, r));
            let (a, b) = self.portal(w, k, nom, nom);
            nominal.push((a + b) * 0.5);
        }
        nominal.push(e);
        if attempt >= 2 {
            nominal.dedup_by(|a, b| (*a - *b).len2() < 1.0);
            nominal
        } else {
            funnel(s, e, &portals)
        }
    }

    /// Fill a [`Diag`] for the run `k0..k1` of `w` that was rejected: re-draws the three attempts
    /// without the repair and with it, and classifies. Only on failure. O(attempts x segments).
    #[allow(clippy::too_many_arguments)]
    fn diagnose(&mut self, w: WireId, layer: usize, k0: usize, k1: usize, s: Vec2, e: Vec2, last: &Violation, last_pts: &[Vec2]) {
        let wi = &self.st.wires[w];
        let mut pre_bad = [false; 3];
        let mut pre0: Vec<Vec2> = Vec::new();
        for (a, bad) in pre_bad.iter_mut().enumerate() {
            let pre = self.base_polyline(w, k0, k1, s, e, a);
            let items = self.run_items(w, layer, &pre);
            *bad = !self.checker.check_against(&self.idx, &items, &[]).violations.is_empty();
            if a == 0 {
                pre0 = pre;
            }
        }
        let neck = self.neck_zones(w, k0 == 0, k1 == wi.passes.len());
        let pl = self.b.proj.n_layers;
        let over_passes = (k0..k1).filter(|&k| self.over[self.st.passes[wi.passes[k]].edge * pl + layer]).count();
        // Segment of the last attempt that violates; is it next to a vertex the repair inserted?
        let seg = last.item.min(last_pts.len().saturating_sub(2));
        let (pa, pb) = (last_pts[seg], last_pts[seg + 1]);
        let inserted = |p: Vec2| !pre0.iter().any(|q| (*q - p).len2() < 1e-6);
        let repair_vertex = inserted(pa) || inserted(pb);
        let class = if over_passes > 0 {
            "packing"
        } else if !pre_bad[0] || repair_vertex {
            "repair"
        } else {
            "chord"
        };
        let bumps = last_pts.len().saturating_sub(pre0.len());
        let deep = std::env::var("TOPO_DIAG_DEEP").is_ok();
        self.only_static.store(deep, std::sync::atomic::Ordering::Relaxed);
        let static_infeasible = deep && self.exact_fix(w, k0, k1, layer, pre0.clone(), neck).is_none();
        self.only_static.store(false, std::sync::atomic::Ordering::Relaxed);
        if std::env::var("TOPO_DEBUG").is_ok() {
            // Diagnosis (D1): per pass of the run, the room its packed lane promised (`lmin ..
            // rmax`) and what the neighbours drawn so far left of it (`tight_span`).
            let pr = &self.b.proj;
            for k in k0..k1 {
                let pid = wi.passes[k];
                let e = self.st.passes[pid].edge;
                let Some((_, sl, i, lmin, rmax)) = self.slot[pid] else {
                    eprintln!("LANE wire {w} k {k} e{e}: no fitting lane (pass layer {})", self.pass_layer[pid]);
                    continue;
                };
                let (lo, hi) = self.tight_span(pid).unwrap_or((f64::NAN, f64::NAN));
                // The lane of the pass's own layer (a feedback redraw may have relabelled it).
                let Some(lane) = self.lanes.get(&(e, sl)) else { continue };
                let nb: Vec<String> = lane
                    .pids
                    .iter()
                    .enumerate()
                    .filter(|&(j, _)| j != i)
                    .map(|(j, &q)| {
                        let (ql, qr) = self.slot[q].map_or((f64::NAN, f64::NAN), |s| (s.3, s.4));
                        let elen = (pr.pos[pr.edges[e][1]] - pr.pos[pr.edges[e][0]]).len();
                        format!(
                            "{}{}@{}[slot {:.3}..{:.3} iv {:.3}..{:.3}]",
                            if j < i { "<" } else { ">" },
                            self.st.passes[q].wire,
                            self.live_x(q).map_or("-".into(), |x| format!("{:.3}", x / 1e6)),
                            ql / 1e6,
                            qr / 1e6,
                            self.iv.lo[q] * elen / 1e6,
                            self.iv.hi[q] * elen / 1e6
                        )
                    })
                    .collect();
                let [p, q] = pr.edges[e];
                let mid = (pr.pos[p] + pr.pos[q]) * 0.5;
                eprintln!(
                    "LANE wire {w} k {k} e{e} p ({:.3},{:.3}) {:?} q ({:.3},{:.3}) {:?} at ({:.3},{:.3}) len {:.3}: slot {i} lmin {:.3} rmax {:.3} spare {:.3}; left by drawn neighbours {:.3}..{:.3}{} | {}",
                    pr.pos[p].x / 1e6,
                    -pr.pos[p].y / 1e6,
                    pr.vkind[p],
                    pr.pos[q].x / 1e6,
                    -pr.pos[q].y / 1e6,
                    pr.vkind[q],
                    mid.x / 1e6,
                    -mid.y / 1e6,
                    (pr.pos[q] - pr.pos[p]).len() / 1e6,
                    lmin / 1e6,
                    rmax / 1e6,
                    (rmax - lmin) / 1e6,
                    lo / 1e6,
                    hi / 1e6,
                    if lo > hi + 1.0 || (hi - lo).abs() < 1.0 { " SQUEEZED" } else { "" },
                    nb.join(" ")
                );
            }
        }
        self.diag.push(Diag {
            wire: w,
            net: wi.net,
            layer,
            width_nm: wi.w,
            passes: k1 - k0,
            k0,
            k1,
            over_passes,
            class,
            against: last.against.clone(),
            gap_nm: last.gap,
            req_nm: last.required,
            at: (pa + pb) * 0.5,
            from: s,
            to: e,
            pre_bad,
            bumps,
            static_infeasible,
            against_via: last.against.strip_prefix("item:").and_then(|x| x.parse::<usize>().ok()).is_some_and(|id| self.idx.is_via(id)),
        });
    }

    /// Bend `pts` around every object it comes closer to than its clearance: a vertex at the
    /// object's nearest point pushed out along the line from the object to the segment. Repeats
    /// (the push grows by 12 % per repeat at one object) until clean or the cap is reached; the
    /// result is judged by the [`Checker`] anyway. O(rounds x segments x neighbours).
    pub(super) fn repair(&self, net: i32, w: f64, s: f64, layer: usize, mut pts: Vec<Vec2>) -> Vec<Vec2> {
        let mut bumps: BTreeMap<usize, u32> = BTreeMap::new();
        // F6: the worst violation of a segment is a pure function of its two ends here (the objects
        // do not change during the repair), and a sweep changes only the segments it bumps: the
        // other segments' results are reused, not searched again. Keyed by the ends' bits.
        type Worst = Option<(f64, usize, Vec2, Vec2, f64)>;
        let mut memo: crate::fxhash::FxHashMap<[u64; 4], Worst> = crate::fxhash::map();
        for _ in 0..60 {
            // The worst violation of every segment: (segment, short, object, nearest point, push, need).
            let mut fixes: Vec<(usize, usize, Vec2, Vec2, f64)> = Vec::new();
            for i in 0..pts.len() - 1 {
                let (a, b) = (pts[i], pts[i + 1]);
                let key = [a.x.to_bits(), a.y.to_bits(), b.x.to_bits(), b.y.to_bits()];
                if let Some(&worst) = memo.get(&key) {
                    if let Some((_, id, q, n, need)) = worst {
                        fixes.push((i, id, q, n, need));
                    }
                    continue;
                }
                let mut ids = self.grid.query(bbox_of(&[a, b]).grow(self.qgrow));
                ids.sort_unstable();
                ids.dedup();
                let mut worst: Option<(f64, usize, Vec2, Vec2, f64)> = None;
                for id in ids {
                    let e = &self.ents[id];
                    if !e.live || (e.mask >> layer) & 1 == 0 || e.net == Some(net) {
                        continue;
                    }
                    let need = e.grow + if e.fixed { e.clear } else { e.clear.max(s) } + 0.5 * w + self.eps;
                    let (d, q, c) = seg_core(a, b, &e.core);
                    let short = need - d;
                    if short > 0.5 && worst.as_ref().is_none_or(|x| short > x.0) {
                        let centre = match &e.core {
                            Core::Poly(p) => p.iter().fold(Vec2 { x: 0.0, y: 0.0 }, |acc, &v| acc + v) * (1.0 / p.len() as f64),
                            Core::Seg(u, v) => (*u + *v) * 0.5,
                            Core::Pt(p) => *p,
                        };
                        let n = if d > 1e-6 { (c - q) * (1.0 / d) } else { (q - centre) * (1.0 / (q - centre).len().max(1e-9)) };
                        worst = Some((short, id, q, n, need));
                    }
                }
                memo.insert(key, worst);
                if let Some((_, id, q, n, need)) = worst {
                    fixes.push((i, id, q, n, need));
                }
            }
            if fixes.is_empty() {
                break;
            }
            let mut progressed = false;
            for &(i, id, q, n, need) in fixes.iter().rev() {
                let k = bumps.entry(id).or_insert(0);
                *k += 1;
                if *k > 8 {
                    continue;
                }
                progressed = true;
                let o = q + n * ((need + 3_000.0) * (1.0 + 0.12 * f64::from(*k - 1)));
                pts.insert(i + 1, o);
            }
            if !progressed {
                break;
            }
        }
        pts
    }
}

/// draw-via-neck r5: one traversal of a neighbour's planned corridor that a via site reaches into
/// ([`Drawer::via_plan_price_in`]): the wire, how deep (nm), its packed slack away from the via
/// (nm, [`Drawer::pass_slack_away`]) and its pitch (width + clearance, nm).
#[derive(Clone, Copy, Debug)]
pub(crate) struct RoomHit {
    pub wire: WireId,
    pub depth: f64,
    pub slack: f64,
    pub pitch: f64,
}

/// draw-via-neck r5 (lead 2026-11-26): how much more an intrusion into a pinned neighbour (no
/// packed slack away from the via: a pad exit, a full passage) costs than one into a free
/// neighbour, as a factor on the depth: `depth x (1 + PINNED_DEPTH x depth / (depth + slack))`.
/// A free neighbour (slack >> depth) pays its depth (it moves that far: its detour); a pinned
/// one pays 3x the depth (it cannot move: the room comes out of the next wire or the
/// clearance). Graded, no cutoff. Measured on the pocket pattern (via_interval_sliver: X's
/// pinned traversal, depth 0.39 mm, slack 0.014: the per-wire depth alone, factor 0, keeps the
/// via 1 mm along the wire; 1-3 put it into the pocket) and on esp's one-cycle via sites (re-priced
/// from the debug lines: 1 and 2 keep every pick, 3 moves IO8_SCL's via 0): 2.
pub(crate) const PINNED_DEPTH: f64 = 2.0;

/// The planned-room price (nm, before the `room` weight) of a via site's [`RoomHit`]s,
/// `pitch x (x + x^2)` with `x = depth / pitch`. `per_wire` (the shift on): each neighbour
/// once, at its deepest slack-weighted intrusion (`PINNED_DEPTH`): independent of how many
/// triangles its corridor crosses near the site. Off: every traversal at its depth (main).
pub(crate) fn neighbour_room_price(hits: &[RoomHit], per_wire: bool) -> f64 {
    let f = |e: f64, pitch: f64| {
        let x = e / pitch;
        pitch * (x + x * x)
    };
    if !per_wire {
        return hits.iter().filter(|h| h.depth > 0.0).map(|h| f(h.depth, h.pitch)).sum();
    }
    let mut deepest: Vec<(WireId, f64, f64)> = Vec::new();
    for h in hits.iter().filter(|h| h.depth > 0.0) {
        let e = h.depth * (1.0 + PINNED_DEPTH * h.depth / (h.depth + h.slack.max(0.0)));
        match deepest.iter_mut().find(|x| x.0 == h.wire) {
            Some(x) => x.1 = x.1.max(e),
            None => deepest.push((h.wire, e, h.pitch)),
        }
    }
    deepest.iter().map(|&(_, e, p)| f(e, p)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// draw-absorb A0: the index id of an `against` label, as the checker and `viols_measured`
    /// name routed copper; and each forced candidate's ladder label (a call-site mutant of the
    /// labels changes these).
    #[test]
    fn against_item_and_attempt_labels() {
        assert_eq!(against_item("item:12"), Some(12));
        assert_eq!(against_item("track of SIG (item:7)"), Some(7));
        assert_eq!(against_item("via of GND (item:345)"), Some(345));
        assert_eq!(against_item("pad:U1:3"), None);
        assert_eq!(against_item("kept:abc"), None);
        assert_eq!(against_item("item:"), None);
        let ls: Vec<&str> = (-1..=6).map(attempt_label).collect();
        assert_eq!(ls, ["erbs", "attempt0", "attempt1", "attempt2", "attempt3", "attempt4", "attempt5", "erbs_kept"]);
        // Equality leaves the provenance out (it is a measurement).
        let v = Viol { kind: "clearance", at: Vec2 { x: 1.0, y: 2.0 }, layer: 0, depth_nm: 3.0, len_nm: 4.0, against: "x".into(), via: false, src: ViolSrc::default() };
        let mut w = v.clone();
        w.src.mech = "forced_run";
        assert_eq!(v, w);
        w.depth_nm = 5.0;
        assert_ne!(v, w);
    }

    /// draw-via-neck r6: a neck at the run's pad end reaches past the pad's outline (no width
    /// step inside the pad, whose in-pad narrow piece KiCad's Cleanup would delete); a neck
    /// starting inside the pad covers the pad end too; a run that never leaves the pad is necked
    /// to its end; zones away from the pad, unbounded zones and runs whose end lies outside the
    /// pad are left alone.
    #[test]
    fn neck_out_of_pad_moves_the_width_step_outside_the_pad() {
        let v = |x: f64, y: f64| Vec2 { x, y };
        let pad = [v(-500_000.0, -150_000.0), v(500_000.0, -150_000.0), v(500_000.0, 150_000.0), v(-500_000.0, 150_000.0)];
        let run = [v(0.0, 0.0), v(300_000.0, 0.0), v(2_000_000.0, 0.0)];
        let nk = |z: [Option<(Vec2, f64)>; 3]| super::super::exact::Neck { min_w: 100_000.0, ends: [z[0], z[1], z[2], None, None, None] };
        let mut n = nk([Some((v(0.0, 0.0), 100_000.0)), Some((v(300_000.0, 0.0), 50_000.0)), Some((v(1_500_000.0, 0.0), 200_000.0))]);
        neck_out_of_pad(&mut n, &run, &pad);
        let r = |n: &super::super::exact::Neck, i: usize| n.ends[i].unwrap().1;
        assert!((r(&n, 0) - (500_000.0 + NECK_PAD_OUT + 1.0)).abs() < 2.0, "end zone past the outline: {}", r(&n, 0));
        assert!((r(&n, 1) - 300_001.0).abs() < 2.0, "inner zone covers the pad end: {}", r(&n, 1));
        assert_eq!(r(&n, 2), 200_000.0, "a zone away from the pad stays");
        let mut inf = nk([Some((v(0.0, 0.0), f64::INFINITY)), None, None]);
        neck_out_of_pad(&mut inf, &run, &pad);
        assert!(r(&inf, 0).is_infinite());
        let mut outside = nk([Some((v(600_000.0, 0.0), 100_000.0)), None, None]);
        neck_out_of_pad(&mut outside, &[v(600_000.0, 0.0), v(2_000_000.0, 0.0)], &pad);
        assert_eq!(r(&outside, 0), 100_000.0, "run end outside the pad");
        let mut inside = nk([Some((v(0.0, 0.0), 100_000.0)), None, None]);
        neck_out_of_pad(&mut inside, &[v(0.0, 0.0), v(300_000.0, 0.0)], &pad);
        assert!((r(&inside, 0) - 300_001.0).abs() < 2.0, "a run that never leaves the pad (via in pad) is necked to its end: {}", r(&inside, 0));
        let mut apart = nk([Some((v(1_500_000.0, 0.0), 100_000.0)), None, None]);
        neck_out_of_pad(&mut apart, &[v(0.0, 0.0), v(300_000.0, 0.0)], &pad);
        assert_eq!(r(&apart, 0), 100_000.0, "a zone off a run inside the pad stays");
    }

    /// draw-via-neck r5 (lead 2026-11-26): the per-wire planned-room price does not depend on
    /// the triangulation: a neighbour's corridor split over 1, 3 or 9 triangles near the site
    /// (the same depth and slack in each) prices the same; per traversal (shift off, main) it
    /// grows with the split. A pinned neighbour (slack 0) costs more than a free one (slack
    /// 10 mm) at the same depth, and a deeper intrusion more than a shallower one (graded).
    #[test]
    fn neighbour_room_price_is_split_invariant_and_graded_by_slack() {
        let mm = 1e6;
        let hit = |wire: WireId, depth: f64, slack: f64| RoomHit { wire, depth: depth * mm, slack: slack * mm, pitch: 0.45 * mm };
        let split = |n: usize| (0..n).map(|_| hit(3, 0.3, 0.5)).chain(std::iter::once(hit(4, 0.1, 0.0))).collect::<Vec<_>>();
        let one = neighbour_room_price(&split(1), true);
        for n in [3, 9] {
            assert!((neighbour_room_price(&split(n), true) - one).abs() < 1e-6, "split {n}");
            assert!(neighbour_room_price(&split(n), false) > neighbour_room_price(&split(1), false) * 1.5, "per traversal grows with the split {n}");
        }
        // the deepest traversal of a wire counts, not their sum
        let two = [hit(3, 0.3, 0.5), hit(3, 0.1, 0.5)];
        assert!((neighbour_room_price(&two, true) - neighbour_room_price(&two[..1], true)).abs() < 1e-6);
        // pinned costs much more than free at the same depth; free pays at least its depth's price
        let (pinned, free) = (neighbour_room_price(&[hit(3, 0.2, 0.0)], true), neighbour_room_price(&[hit(3, 0.2, 10.0)], true));
        assert!(pinned > 3.0 * free, "pinned {pinned} free {free}");
        assert!((free - neighbour_room_price(&[hit(3, 0.2, 0.0)], false)) / free < 0.1, "free ~ its depth: {free}");
        // graded: more slack, cheaper; deeper, dearer; nothing without depth
        let at = |d: f64, s: f64| neighbour_room_price(&[hit(3, d, s)], true);
        assert!(at(0.2, 0.0) > at(0.2, 0.1) && at(0.2, 0.1) > at(0.2, 1.0) && at(0.3, 0.5) > at(0.2, 0.5));
        assert_eq!(at(-0.1, 0.0), 0.0);
    }

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x, y }
    }

    /// grid-snap r3: the DRC class by the checker's drill flag: a via's copper deficit at a rule
    /// equal to the hole clearance is `clearance` (or `short`), its drill's is `hole_clearance`.
    #[test]
    fn viol_kind_reads_the_drill_flag() {
        let v = |against: &str, gap: i64, drill: bool| Violation { item: 0, against: against.into(), layer: 0, gap, required: 127_000, drill };
        assert_eq!(viol_kind(&v("item:3", 20_000, false), true), "clearance");
        assert_eq!(viol_kind(&v("item:3", -5_000, false), true), "short");
        assert_eq!(viol_kind(&v("item:3", 120_000, true), true), "hole_clearance");
        assert_eq!(viol_kind(&v("viahole:u", 120_000, false), true), "hole_clearance");
        assert_eq!(viol_kind(&v("item:3", 120_000, true), false), "clearance");
        assert_eq!(viol_kind(&v("edge", 0, false), true), "edge_clearance");
    }

    /// grid-snap r2: a forced via site on foreign copper (a short, here with its drill's hole
    /// clearance) loses to a site with deep clearance deficits only; between shorts the smaller
    /// short wins; the extra price adds to the second term only.
    #[test]
    fn force_via_key_puts_shorts_first() {
        const MM: f64 = 1e6;
        let viol = |kind: &'static str, depth_mm: f64| Viol { kind, at: v(0.0, 0.0), layer: 0, depth_nm: depth_mm * MM, len_nm: 0.6 * MM, against: "x".into(), via: true, src: ViolSrc::default() };
        let on_track = [viol("short", 0.05), viol("hole_clearance", 0.02)];
        let tight = [viol("clearance", 0.12), viol("clearance", 0.12), viol("hole_clearance", 0.1)];
        let (a, b) = (force_via_key(&on_track, 0.0, 0.0), force_via_key(&tight, 0.0, 0.0));
        assert!(a[2] < b[2], "the short site is cheaper on amounts alone: {a:?} {b:?}");
        assert!(force_via_better(b, a) && !force_via_better(a, b), "{a:?} {b:?}");
        let deeper = [viol("short", 0.3)];
        assert!(force_via_better(force_via_key(&on_track, 5.0, 0.0), force_via_key(&deeper, 0.0, 0.0)));
        assert_eq!(force_via_key(&tight, 1.0, 0.0)[2], b[2] + 1.0);
        assert_eq!(force_via_key(&tight, 1.0, 0.0)[0], 0.0);
        // r3: the drill tier: a shallow drill deficit (KiCad's 0.007 mm) loses to deep copper
        // clearance deficits only; with the margin subtracted a drill deficit inside the margin is
        // no KiCad violation (tier 0)
        let drill = [viol("hole_clearance", 0.007)];
        let copper = [viol("clearance", 0.1), viol("clearance", 0.1)];
        let (d, c) = (force_via_key(&drill, 0.0, 0.0), force_via_key(&copper, 0.0, 0.0));
        assert!(d[2] < c[2] && d[1] > 0.0 && c[1] == 0.0, "{d:?} {c:?}");
        assert!(force_via_better(c, d) && !force_via_better(d, c));
        assert_eq!(force_via_key(&[viol("hole_clearance", 0.05)], 0.0, 0.05e6)[1], 0.0);
        assert!(force_via_key(&[viol("hole_clearance", 0.15)], 0.0, 0.05e6)[1] > 0.0);
        assert!(!force_via_better(c, c));
    }

    /// forced-draw (T4): the graded choice of a forced run's form ([`forced_form_key`],
    /// [`forced_form_better`]): every term on one mm scale, shorts first, ties keep the taut form.
    #[test]
    fn forced_form_key_is_one_graded_scale_with_shorts_first() {
        const MM: f64 = 1e6;
        let cfg = super::super::octi::OctiCfg::default(); // violation 1000, any_angle 15, acute 50 (mm)
        let viol = |kind: &'static str, depth_mm: f64, len_mm: f64| Viol { kind, at: v(0.0, 0.0), layer: 0, depth_nm: depth_mm * MM, len_nm: len_mm * MM, against: "x".into(), via: false, src: ViolSrc::default() };
        // taut: one 10 mm free-angle stretch (8 x 6), then 10 mm straight; octi: 45 deg + straight, a bit longer
        let taut = [v(0.0, 0.0), v(8.0 * MM, 6.0 * MM), v(18.0 * MM, 6.0 * MM)];
        let octi = [v(0.0, 0.0), v(2.0 * MM, 0.0), v(8.0 * MM, 6.0 * MM), v(18.0 * MM, 6.0 * MM)];
        let lt = poly_len(&taut);
        let lo = poly_len(&octi);
        let base = vec![viol("clearance", 0.05, 0.2)];
        let (kt, ko) = (forced_form_key(&base, &taut, &cfg), forced_form_key(&base, &octi, &cfg));
        // the exact price: 1000 x (1 + 0.05 x 0.2 / 0.01) + 15 x (1 + 10 x 2/15) + length
        let want_t = 1000.0 * MM * 2.0 + 15.0 * MM * (1.0 + 10.0 * 2.0 / 15.0) + lt;
        assert!((kt.1 - want_t).abs() < 1.0 && kt.0 == 0.0, "{kt:?} vs {want_t}");
        assert!((ko.1 - (1000.0 * MM * 2.0 + lo)).abs() < 1.0, "{ko:?}");
        // the same violations, no free angle: the octilinear form wins although longer
        assert!(lo > lt && forced_form_better(ko, kt) && !forced_form_better(kt, ko));
        // one more violation unit (1000 mm) outweighs one 10 mm free-angle stretch (35 mm)
        let more = vec![viol("clearance", 0.05, 0.2), viol("clearance", 0.001, 0.1)];
        assert!(!forced_form_better(forced_form_key(&more, &octi, &cfg), kt));
        // shorts first: less short amount wins whatever the clearance price
        let short_t = vec![viol("short", 0.1, 0.3)];
        let many = (0..20).map(|_| viol("clearance", 0.1, 1.0)).collect::<Vec<_>>();
        assert!(forced_form_better(forced_form_key(&many, &octi, &cfg), forced_form_key(&short_t, &taut, &cfg)));
        assert!(!forced_form_better(forced_form_key(&short_t, &octi, &cfg), forced_form_key(&many, &taut, &cfg)));
        // a tie keeps the taut form (strictly cheaper only)
        assert!(!forced_form_better(kt, kt));
        // an acute bend (interior 45 deg: turn 135) pays acute_bend x 1.5
        let acute = [v(0.0, 0.0), v(10.0 * MM, 0.0), v(0.0, 10.0 * MM)];
        let ka = forced_form_key(&[], &acute, &cfg);
        assert!((ka.1 - (50.0 * MM * 1.5 + poly_len(&acute))).abs() < 1.0, "{ka:?}");
        // the weights are the registry's: a zero violation weight leaves style and length
        let free = super::super::octi::OctiCfg { viol_nm: 0.0, ..cfg };
        assert!((forced_form_key(&base, &taut, &free).1 - (want_t - 2000.0 * MM)).abs() < 1.0);
    }

    /// forced-draw (T7): [`forced_form_pick`] takes the strictly cheapest form by
    /// [`forced_form_better`] (shorts first, then the price), the first on ties: the taut run
    /// (index 0) stays unless a form is strictly cheaper, the earlier variant wins a tie.
    #[test]
    fn forced_form_pick_takes_the_strictly_cheapest_first_on_ties() {
        assert_eq!(forced_form_pick(&[(0.0, 5.0)]), 0);
        assert_eq!(forced_form_pick(&[(0.0, 5.0), (0.0, 5.0), (0.0, 5.0)]), 0, "ties keep the taut run");
        assert_eq!(forced_form_pick(&[(0.0, 5.0), (0.0, 4.0), (0.0, 4.0)]), 1, "ties keep the earlier variant");
        assert_eq!(forced_form_pick(&[(0.0, 5.0), (0.0, 4.0), (0.0, 3.0)]), 2);
        assert_eq!(forced_form_pick(&[(0.0, 5.0), (0.0, 3.0), (0.0, 4.0)]), 1);
        assert_eq!(forced_form_pick(&[(2.0, 5.0), (1.0, 9.0), (1.5, 1.0)]), 1, "shorts first");
        assert_eq!(forced_form_pick(&[(0.0, 5.0), (1.0, 1.0), (0.0, 6.0)]), 0, "a short never wins over none");
        assert_eq!(forced_form_pick(&[(3.0, 50.0), (2.0, 60.0), (1.0, 70.0)]), 2);
    }

    /// nd-perf F1: [`pick_by_bound`] returns what `min_by` over the exact keys returns (the least
    /// key, the first index on ties) for random candidate sets with many equal keys and bounds at
    /// or below the exact keys; it prices each candidate at most once, and never one whose bound
    /// is above the winner's exact key.
    #[test]
    fn pick_by_bound_is_the_exhaustive_choice() {
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |m: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % m
        };
        let (mut pruned, mut ties) = (0usize, 0usize);
        for _ in 0..20_000 {
            let n = next(12) as usize;
            let exact: Vec<ForcedKey> = (0..n)
                .map(|_| {
                    let (s, t) = (next(3) as f64, next(6) as f64);
                    (s, s + t, s + t + [0.0, 0.0, 0.5][next(3) as usize])
                })
                .collect();
            let lbs: Vec<ForcedKey> = exact
                .iter()
                .map(|k| match next(4) {
                    0 => *k,
                    1 => (k.0, k.1 - next(2) as f64, k.2 - next(3) as f64),
                    2 => (k.0 - next(2) as f64, k.1 - next(4) as f64, k.2 - next(5) as f64),
                    _ => (0.0, 0.0, 0.0),
                })
                .map(|k| (k.0.max(0.0), k.1.max(0.0), k.2.max(0.0)))
                .collect();
            for j in 0..n {
                assert!(lbs[j].0 <= exact[j].0 && lbs[j].1 <= exact[j].1 && lbs[j].2 <= exact[j].2);
            }
            let want = (0..n).min_by(|&a, &b| forced_key_cmp(&exact[a], &exact[b]));
            let mut priced = vec![0usize; n];
            let got = pick_by_bound(&lbs, |j| {
                priced[j] += 1;
                exact[j]
            });
            assert_eq!(got, want, "exact {exact:?} bounds {lbs:?}");
            assert!(priced.iter().all(|&c| c <= 1));
            if let Some(w) = want {
                for j in 0..n {
                    if forced_key_cmp(&lbs[j], &exact[w]).is_gt() {
                        assert_eq!(priced[j], 0, "candidate {j} priced although its bound is above the winner");
                        pruned += 1;
                    }
                }
                ties += (0..n).filter(|&j| j != w && forced_key_cmp(&exact[j], &exact[w]).is_eq()).count();
            }
        }
        assert!(pruned > 1000 && ties > 1000, "the sets exercise pruning ({pruned}) and ties ({ties})");
    }

    /// nd-perf F2: `via_reach` prices its reach exactly when `[vias] reach_price` reads the price.
    #[test]
    fn via_reach_is_priced_only_when_the_price_is_read() {
        if std::env::var_os("TOPO_VIADBG").is_some() {
            return;
        }
        let mut o = Options::default();
        assert!(!o.via_reach_price);
        assert!(!reach_priced(&o));
        o.via_reach_price = true;
        assert!(reach_priced(&o));
    }

    /// M3 length factor: the price is the length up to the band and grows smoothly (quadratic
    /// in the excess) past it; a sequential run is drawn only past `B (1 + g)` and replaces the
    /// ERBS only when its length priced at `1 + g` is below the ERBS's price.
    #[test]
    fn erbs_length_price_and_competition() {
        assert_eq!(erbs_length_price(9.0, 10.0, 1.0), 9.0);
        assert_eq!(erbs_length_price(10.0, 10.0, 1.0), 10.0);
        assert!((erbs_length_price(12.0, 10.0, 1.0) - 12.0 * 1.04).abs() < 1e-9);
        assert!((erbs_length_price(30.0, 10.0, 1.0) - 30.0 * 5.0).abs() < 1e-9);
        assert!((erbs_length_price(12.0, 10.0, 2.0) - 12.0 * 1.08).abs() < 1e-9);
        assert_eq!(erbs_length_price(12.0, 10.0, 0.0), 12.0);
        assert_eq!(erbs_length_price(12.0, 0.0, 1.0), 12.0);
        // smooth: no step anywhere past the band
        let mut last = erbs_length_price(10.0, 10.0, 1.0);
        for k in 1..400 {
            let p = erbs_length_price(10.0 + k as f64 * 0.05, 10.0, 1.0);
            assert!(p > last && p - last < 1.0, "{k}: {last} -> {p}");
            last = p;
        }
        // below B (1 + g) no sequential run is drawn
        assert_eq!(erbs_vs_sequential(10.4, 10.0, 0.05, None), None);
        assert_eq!(erbs_vs_sequential(10.5, 10.0, 0.05, Some(1.0)), None);
        // past it: drawn; replaced only when ls (1 + g) < pe
        assert_eq!(erbs_vs_sequential(10.6, 10.0, 0.05, None), Some(false));
        assert_eq!(erbs_vs_sequential(10.6, 10.0, 0.05, Some(10.0)), Some(true));
        assert_eq!(erbs_vs_sequential(10.6, 10.0, 0.05, Some(10.2)), Some(false));
        assert_eq!(erbs_vs_sequential(31.5, 10.0, 0.05, Some(30.0)), Some(false));
        assert_eq!(erbs_vs_sequential(31.6, 10.0, 0.05, Some(30.0)), Some(true));
        // no band: always compared
        assert_eq!(erbs_vs_sequential(1.0, 0.0, 0.05, Some(0.9)), Some(true));
    }

    /// Round i: crossings are counted by orientation signs (proper crossings), per wire; touching
    /// or collinear runs are no crossing, a path outside the run's box is not looked at; the
    /// competitor keeps the class only if it crosses every wire with the ERBS's parity.
    #[test]
    fn class_test_counts_proper_crossings_per_wire() {
        let run = [v(0.0, 0.0), v(10.0, 0.0)];
        let zig = [v(2.0, -1.0), v(3.0, 1.0), v(4.0, -1.0)];
        // Round j: a path that ends on the run counts by the side the run's translation (e, e^2)
        // puts it on: from above 1, from below 0 (one consistent answer for both compared runs).
        let touch = [v(5.0, 0.0), v(5.0, 3.0)];
        let touch_below = [v(5.5, 0.0), v(5.5, -3.0)];
        let along = [v(6.0, 0.0), v(8.0, 0.0)];
        let near = [v(1.0, 1e-6), v(9.0, 1e-6)];
        let far = [v(20.0, -1.0), v(20.0, 1.0)];
        let xs = crossings_per_wire(&run, &[(1, &zig[..]), (2, &touch), (3, &along), (4, &near), (5, &far), (6, &touch_below)]);
        assert_eq!(xs, BTreeMap::from([(1, 2), (2, 1)]));
        assert!(crossings_per_wire(&run[..1], &[(1, &zig)]).is_empty());
        let m = |v: &[(WireId, usize)]| v.iter().copied().collect::<BTreeMap<_, _>>();
        assert!(competitor_keeps_class(&m(&[]), &m(&[])));
        assert!(competitor_keeps_class(&m(&[(1, 2)]), &m(&[(1, 2)])));
        // qmtech HDMI5V / ADV_SCL: 4 against 2 is one class (a bigon), the competitor may win
        assert!(competitor_keeps_class(&m(&[(1, 4)]), &m(&[(1, 2)])));
        assert!(competitor_keeps_class(&m(&[(1, 2)]), &m(&[])));
        assert!(competitor_keeps_class(&m(&[(1, 1), (2, 2)]), &m(&[(1, 3)])));
        // the other parity on any wire, on either side: another class
        assert!(!competitor_keeps_class(&m(&[(1, 1)]), &m(&[])));
        assert!(!competitor_keeps_class(&m(&[]), &m(&[(3, 1)])));
        assert!(!competitor_keeps_class(&m(&[(1, 3)]), &m(&[(1, 2)])));
        assert!(!competitor_keeps_class(&m(&[(1, 1), (2, 1)]), &m(&[(1, 1)])));
    }

    /// Round i, the bigon (qmtech HDMI5V / ADV_SCL): A's ERBS dips across B's undrawn path and back
    /// (2 crossings), A's sequential competitor zigzags across it 4 times. Both bound a disc free
    /// of B's terminals with B's path: one class, the competitor may win. A competitor that ends on
    /// B's other side of A's terminals' line, crossing once (the qmtech ADV_CLK shortcut), may not.
    #[test]
    fn class_test_bigon_is_one_class() {
        let b = [v(0.0, 0.0), v(10.0, 0.0)];
        let erbs = [v(2.0, -3.0), v(3.0, 1.0), v(5.0, 1.0), v(6.0, -3.0)];
        let seq = [v(2.0, -3.0), v(3.0, 1.0), v(3.5, -1.0), v(4.5, 1.0), v(6.0, -3.0)];
        let (xe, xs) = (crossings_per_wire(&erbs, &[(7, &b)]), crossings_per_wire(&seq, &[(7, &b)]));
        assert_eq!((xe.get(&7), xs.get(&7)), (Some(&2), Some(&4)));
        assert!(competitor_keeps_class(&xs, &xe));
        let straight = [v(2.0, -3.0), v(6.0, -3.0)];
        assert!(competitor_keeps_class(&crossings_per_wire(&straight, &[(7, &b)]), &xe));
        // the ADV_CLK shortcut: B's path ends between A's ERBS and the shortcut (fixture shortcut_across)
        let b2 = [v(0.0, 0.0), v(4.0, 0.0)];
        let wrap = [v(5.0, -3.0), v(-1.0, -1.0), v(-1.0, 1.0), v(5.0, 3.0)];
        let short = [v(5.0, -3.0), v(2.0, 3.0)];
        let (xw, xc) = (crossings_per_wire(&wrap, &[(7, &b2)]), crossings_per_wire(&short, &[(7, &b2)]));
        assert_eq!((xw.get(&7), xc.get(&7)), (None, Some(&1)));
        assert!(!competitor_keeps_class(&xc, &xw));
    }

    /// Round j (verifier round i): the degenerate contacts of the crossing count. A crossing through
    /// a vertex of the reference path, at the run's own vertex, or vertex on vertex counts once
    /// (the tolerance window of `seg_proper` counted 0: the parity flipped); a touch-and-return and
    /// a collinear stretch that leaves on the side it came from count an even number, one that
    /// leaves on the other side an odd one. The same at nm coordinates (exact signs, no tolerance).
    #[test]
    fn crossing_count_degenerate_cases() {
        let par = |run: &[Vec2], path: &[Vec2]| path.windows(2).map(|b| proper_crossings(run, b[0], b[1])).sum::<usize>() % 2;
        let old = |run: &[Vec2], path: &[Vec2]| path.windows(2).map(|b| run.windows(2).filter(|x| seg_proper(x[0], x[1], b[0], b[1])).count()).sum::<usize>() % 2;
        let run = [v(0.0, 0.0), v(10.0, 0.0)];
        let bent = [v(0.0, -1.0), v(5.0, 0.0), v(10.0, 1.0)];
        let kinked = [v(0.0, 0.0), v(5.0, 0.0), v(10.0, 0.0)];
        let cases: [(&str, &[Vec2], Vec<Vec2>, usize); 9] = [
            ("through a path vertex", &run, vec![v(4.0, -1.0), v(5.0, 0.0), v(6.0, 1.0)], 1),
            ("through a path vertex, square", &run, vec![v(5.0, -1.0), v(5.0, 0.0), v(5.0, 1.0)], 1),
            ("at the run's own vertex", &bent, vec![v(5.0, -5.0), v(5.0, 5.0)], 1),
            ("vertex on vertex", &kinked, vec![v(5.0, -1.0), v(5.0, 0.0), v(5.0, 1.0)], 1),
            ("touch and return (path)", &run, vec![v(4.0, 1.0), v(5.0, 0.0), v(6.0, 1.0)], 0),
            ("touch and return (run)", &[v(4.0, 1.0), v(5.0, 0.0), v(6.0, 1.0)], vec![v(0.0, 0.0), v(10.0, 0.0)], 0),
            ("along and back", &run, vec![v(-1.0, 1.0), v(2.0, 0.0), v(6.0, 0.0), v(11.0, 1.0)], 0),
            ("along and across", &run, vec![v(-1.0, 1.0), v(2.0, 0.0), v(6.0, 0.0), v(11.0, -1.0)], 1),
            ("generic", &run, vec![v(3.0, -1.0), v(3.5, 1.0)], 1),
        ];
        let mut old_wrong = 0;
        for (name, r, path, want) in &cases {
            assert_eq!(par(r, path), *want, "{name}");
            // nm coordinates, off the origin: exact signs, the same answer
            let nm = |p: &[Vec2]| p.iter().map(|q| v(12_345_678.0 + q.x * 1e6, -98_765_432.0 + q.y * 1e6)).collect::<Vec<_>>();
            assert_eq!(par(&nm(r), &nm(path)), *want, "{name} at nm");
            // and with the roles swapped (the path translated instead): the parity of a crossing is symmetric
            if !name.starts_with("touch") && !name.starts_with("along") {
                assert_eq!(par(path, r), *want, "{name} swapped");
            }
            old_wrong += usize::from(old(r, path) != *want);
        }
        assert!(old_wrong >= 4, "the old predicate dropped the vertex crossings: {old_wrong}");
        // exact signs: three nm points the plain f64 cross product calls collinear (0) lie right
        // of the line (Shewchuk 1997, section 1: roundoff in the naive determinant)
        let (a, b, c) = (v(12345678.613484412, -9876542.838622024), v(12377094.539984412, -9849360.020622024), v(12401723.373432381, -9828049.775413485));
        assert_eq!((b - a).cross(c - a), 0.0, "the naive determinant rounds to 0");
        assert_eq!(orient_sign(a, b, c), -1);
        // zero-length segments never cross; a run of one point crosses nothing
        assert!(!seg_cross_sos(v(5.0, 0.0), v(5.0, 0.0), v(5.0, -1.0), v(5.0, 1.0)));
        assert!(!seg_cross_sos(v(0.0, 0.0), v(10.0, 0.0), v(5.0, 0.0), v(5.0, 0.0)));
    }

    /// Round j, DoD 2: the crossing parity against an exact integer reference. Two runs with the
    /// same ends make a closed loop L; a path Q whose ends are off L crosses L an odd number of
    /// times iff exactly one end is inside L (even-odd rule, computed in i128). Modes: a 5 x 5
    /// lattice (vertex hits, collinear stretches everywhere), octilinear walks on it (the drawing's
    /// 0/45/90 runs), the lattice at nm scale off the origin, generic integers up to 1e9. The new
    /// count has no mismatch in any mode; the old `seg_proper` count's mismatches are printed.
    #[test]
    fn crossing_parity_fuzz_against_exact_reference() {
        use rand::{Rng, SeedableRng};
        type P = (i64, i64);
        let on_seg = |a: P, b: P, q: P| {
            let c = (b.0 - a.0) as i128 * (q.1 - a.1) as i128 - (b.1 - a.1) as i128 * (q.0 - a.0) as i128;
            c == 0 && q.0 >= a.0.min(b.0) && q.0 <= a.0.max(b.0) && q.1 >= a.1.min(b.1) && q.1 <= a.1.max(b.1)
        };
        let inside = |l: &[P], q: P| {
            let mut c = false;
            for e in l.windows(2) {
                let (a, b) = (e[0], e[1]);
                if (a.1 > q.1) != (b.1 > q.1) {
                    // x of the edge at height q.1 > q.x, without division
                    let lhs = (b.0 - a.0) as i128 * (q.1 - a.1) as i128;
                    let rhs = (q.0 - a.0) as i128 * (b.1 - a.1) as i128;
                    if (b.1 > a.1 && lhs > rhs) || (b.1 < a.1 && lhs < rhs) {
                        c = !c;
                    }
                }
            }
            c
        };
        let walk = |rng: &mut rand_chacha::ChaCha8Rng, s: P, n: usize, octi: bool, m: i64| -> Vec<P> {
            let mut out = vec![s];
            for _ in 0..n {
                let q = if octi {
                    let d = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)][rng.gen_range(0..8)];
                    let k = rng.gen_range(1..3);
                    let p = *out.last().unwrap();
                    ((p.0 + d.0 * k).clamp(0, m), (p.1 + d.1 * k).clamp(0, m))
                } else {
                    (rng.gen_range(0..=m), rng.gen_range(0..=m))
                };
                out.push(q);
            }
            out
        };
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(20261117);
        for (mode, m, scale, off, octi) in [("lattice", 4i64, 1.0, (0.0, 0.0), false), ("octilinear", 6, 1.0, (0.0, 0.0), true), ("lattice-nm", 4, 1e6, (12_345_678.0, -98_765_432.0), false), ("generic", 1_000_000_000, 1.0, (0.0, 0.0), false)] {
            let (mut cases, mut new_bad, mut old_bad) = (0usize, 0usize, 0usize);
            while cases < 20_000 {
                let (s, n1, n2) = ((rng.gen_range(0..=m), rng.gen_range(0..=m)), rng.gen_range(1..4), rng.gen_range(0..3));
                let (q0, nq) = ((rng.gen_range(0..=m), rng.gen_range(0..=m)), rng.gen_range(1..4));
                let r1 = walk(&mut rng, s, n1, octi, m);
                let t = *r1.last().unwrap();
                let mut r2 = walk(&mut rng, s, n2, false, m);
                r2.push(t);
                let q = walk(&mut rng, q0, nq, octi, m);
                let lp: Vec<P> = r1.iter().copied().chain(r2.iter().rev().copied().skip(1)).collect();
                let (q0, q1) = (q[0], *q.last().unwrap());
                if lp.windows(2).any(|e| on_seg(e[0], e[1], q0) || on_seg(e[0], e[1], q1)) {
                    continue;
                }
                cases += 1;
                let want = usize::from(inside(&lp, q0) != inside(&lp, q1));
                let f = |p: &[P]| p.iter().map(|&(x, y)| v(x as f64 * scale + off.0, y as f64 * scale + off.1)).collect::<Vec<_>>();
                let (a, b, qf) = (f(&r1), f(&r2), f(&q));
                let k = |r: &[Vec2]| crossings_per_wire(r, &[(0usize, &qf[..])]).get(&0).copied().unwrap_or(0);
                new_bad += usize::from((k(&a) + k(&b)) % 2 != want);
                // the class decision: the two runs keep the class iff Q's ends are on one side of L
                let (xa, xb) = (crossings_per_wire(&a, &[(0usize, &qf[..])]), crossings_per_wire(&b, &[(0usize, &qf[..])]));
                assert_eq!(competitor_keeps_class(&xa, &xb), want == 0, "{mode}: {r1:?} {r2:?} {q:?}");
                let ko = |r: &[Vec2]| qf.windows(2).map(|c| r.windows(2).filter(|x| seg_proper(x[0], x[1], c[0], c[1])).count()).sum::<usize>();
                old_bad += usize::from((ko(&a) + ko(&b)) % 2 != want);
            }
            eprintln!("crossing parity fuzz {mode}: {cases} cases, new mismatches {new_bad}, old (seg_proper) mismatches {old_bad}");
            assert_eq!(new_bad, 0, "{mode}");
        }
    }

    /// Round j (verifier round i, mutants x_no_net_filter / x_drawn_too / x_any_layer): the class
    /// test looks at undrawn, alive, other-net wires only, and at their paths on the run's layer.
    /// Each path's parity counts on its own: two odd paths of one wire do not cancel.
    #[test]
    fn class_test_filters_and_per_path_parity() {
        // (w, net_w, o, alive, net_o, drawn)
        assert!(class_test_wire(0, 1, 1, true, 2, false));
        assert!(!class_test_wire(0, 1, 1, true, 1, false), "same net");
        assert!(!class_test_wire(0, 1, 1, true, 2, true), "drawn");
        assert!(!class_test_wire(0, 1, 1, false, 2, false), "dead");
        assert!(!class_test_wire(1, 1, 1, true, 2, false), "itself");
        let run = vec![v(0.0, 0.0), v(10.0e6, 0.0)];
        let across = |x: f64| vec![v(x * 1e6, -1.0e6), v(x * 1e6, 1.0e6)];
        let cr = ClassRef::new(vec![(1, 1, across(2.0)), (2, 0, across(3.0)), (3, 0, across(5.0)), (3, 0, across(7.0))]);
        let all = |_: WireId| true;
        assert_eq!(cr.crossings(&run, 0, &all), BTreeMap::from([((2, 1), 1), ((3, 2), 1), ((3, 3), 1)]), "layer 0 only");
        assert_eq!(cr.crossings(&run, 1, &all), BTreeMap::from([((1, 0), 1)]), "layer 1 only");
        assert!(cr.crossings(&run, 0, &|o| o == 1).is_empty(), "keep filters");
        // wire 3 crossed once on each of its two paths by the competitor, not at all by the ERBS:
        // another class (summed per wire it would be 2, even, and pass)
        let none = vec![v(0.0, 5.0e6), v(10.0e6, 5.0e6)];
        let xs = cr.crossings(&run, 0, &|o| o == 3);
        let xe = cr.crossings(&none, 0, &|o| o == 3);
        assert!(!competitor_keeps_class(&xs, &xe));
        let per_wire = |m: &BTreeMap<(WireId, usize), usize>| m.iter().fold(BTreeMap::<WireId, usize>::new(), |mut a, (k, n)| {
            *a.entry(k.0).or_insert(0) += n;
            a
        });
        assert!(competitor_keeps_class(&per_wire(&xs), &per_wire(&xe)), "summed per wire the two odd paths cancel");
    }

    /// Round j (verifier round i): the layers of a traversal's planned chord in the class test; a
    /// passless wire (its sleeve failed) gets its plan's layers, so it is never skipped.
    #[test]
    fn class_ref_layers_of_a_traversal() {
        let none = |_: usize| -2;
        assert_eq!(ref_trav_layers(0, 0, &none, &[1]), vec![1], "passless: its plan's layer");
        assert_eq!(ref_trav_layers(0, 0, &none, &[1, 0]), vec![0, 1]);
        assert!(ref_trav_layers(0, 0, &none, &[]).is_empty(), "no plan known");
        let lay = |j: usize| [0, 1, -1, 1][j];
        assert_eq!(ref_trav_layers(4, 0, &lay, &[0]), vec![0], "start: the first pass's layer");
        assert_eq!(ref_trav_layers(4, 1, &lay, &[0]), vec![0, 1], "a via between: both");
        assert_eq!(ref_trav_layers(4, 2, &lay, &[0]), vec![1], "an open pass is no layer");
        assert_eq!(ref_trav_layers(4, 4, &lay, &[0]), vec![1], "end: the last pass's layer");
        let same = |_: usize| 0;
        assert_eq!(ref_trav_layers(2, 1, &same, &[0]), vec![0], "one layer once");
    }

    #[test]
    fn funnel_is_straight_in_a_corridor() {
        // Travel along +x through two vertical portals: left = +y side, right = -y side.
        let p = [(v(10.0, 5.0), v(10.0, -5.0)), (v(20.0, 5.0), v(20.0, -5.0))];
        let pts = funnel(v(0.0, 0.0), v(30.0, 0.0), &p);
        assert_eq!(pts.len(), 2, "{pts:?}");
    }

    #[test]
    fn funnel_bends_at_the_gap_corner() {
        // Start low, go through a gap that is entirely high: the path touches the near corner.
        let p = [(v(10.0, 12.0), v(10.0, 10.0))];
        let pts = funnel(v(0.0, 0.0), v(20.0, 0.0), &p);
        assert_eq!(pts.len(), 3, "{pts:?}");
        assert!((pts[1] - v(10.0, 10.0)).len() < 1e-9, "{pts:?}");
    }

    /// The path wraps (5, 2) on its left, runs east to (6, 2) and turns up to the end: after
    /// the first corner the funnel opens past 180 degrees (right end (3, 0) south-west, the end
    /// north-east). The previous drawing funnel went (5, 2) -> end straight (`pack` test
    /// `funnel_past_180_degrees_both_sides`).
    #[test]
    fn funnel_past_180_degrees_keeps_the_corner() {
        let p = [(v(5.0, 2.0), v(3.0, 0.0)), (v(6.0, 2.0), v(3.0, 0.0))];
        let pts = funnel(v(3.2, 3.3), v(6.9, 4.4), &p);
        assert_eq!(pts, vec![v(3.2, 3.3), v(5.0, 2.0), v(6.0, 2.0), v(6.9, 4.4)]);
    }

    /// The drawing keeps the corners of the taut path however small the features: a gap 0.1 pm
    /// high, 0.5 pm from the start, keeps its corner (an absolute 1e-3 nm dedup dropped it, and
    /// the run then missed the gap). The same corners as [`super::super::funnel::funnel`].
    #[test]
    fn funnel_keeps_the_corners_of_tiny_features() {
        let p = [(v(5e-4, 3e-4), v(5e-4, 2e-4))];
        let pts = funnel(v(0.0, 0.0), v(1e-3, 0.0), &p);
        assert_eq!(pts, vec![v(0.0, 0.0), v(5e-4, 2e-4), v(1e-3, 0.0)]);
        let ports = [(v(0.0, 0.0), v(0.0, 0.0)), p[0], (v(1e-3, 0.0), v(1e-3, 0.0))];
        assert_eq!(pts, super::super::funnel::funnel(&ports).into_iter().map(|c| c.0).collect::<Vec<_>>());
    }

    #[test]
    fn fuse_leaves_the_trunk_where_the_branch_parts() {
        // A branch that leaves the trunk (y = 0) at x = 0 and runs beside it at 0.5 mm up to
        // x = 5 before it turns away: the cheapest shortcut leaves the trunk under the last
        // vertex, (6, 0) -> (6, 3), and the run beside the trunk is gone (D6b fuse).
        let mm = 1e6;
        let trunk = [v(-10.0 * mm, 0.0), v(10.0 * mm, 0.0)];
        let run = [v(0.0, 0.0), v(1.0 * mm, 0.5 * mm), v(5.0 * mm, 0.5 * mm), v(6.0 * mm, 3.0 * mm)];
        let c = fuse_candidates(&run, &[&trunk], 0.45, 10.0, None);
        assert!(!c.is_empty());
        let (gain, len_gain, i, pts) = &c[0];
        assert_eq!(*i, 3);
        // The length part alone: 1.118 + 4 + 2.693 - 3 = 4.81 mm.
        assert!((*len_gain - 4.81).abs() < 0.01, "{len_gain}");
        assert_eq!(pts.len(), 2, "{pts:?}");
        assert!((pts[0] - v(6.0 * mm, 0.0)).len() < 1.0 && (pts[1] - v(6.0 * mm, 3.0 * mm)).len() < 1.0, "{pts:?}");
        // Length 1.118 + 4 + 2.693 - 3 = 4.81 mm, plus the junction angle 10 x 0.45 x psi:
        // psi(26.6 deg) = 1.73 before, psi(90 deg) = 0.04 after: 12.39 mm.
        assert!((*gain - 12.39).abs() < 0.02, "{gain}");
        // A branch that leaves straight away has nothing to gain.
        assert!(fuse_candidates(&[v(0.0, 0.0), v(0.0, 3.0 * mm)], &[&trunk], 0.45, 10.0, None).is_empty());
    }

    /// octi-bundle r2 (A1c: the fuse prices on the first draw's scale): a 45/90 staircase branch
    /// that starts at the end of a vertical trunk (the wiccon GPIO14 shape: the chord leaves the
    /// trunk arm at 149 degrees, psi = 0, so the junction angle gains nothing). Without the style
    /// the cheapest fuse is the straight free-angle chord (length only); with it the chord pays
    /// `any_angle` (15 + 2 x 6.71 = 28.4 mm) and loses, and the cheapest link is the DP's elbow
    /// (vertical then 45 degrees, as long as the staircase, 5 bends fewer).
    #[test]
    fn fuse_prices_the_link_on_the_octilinear_dp_scale() {
        let mm = 1e6;
        let p = |x: f64, y: f64| v(x * mm, y * mm);
        let trunk = [p(0.0, 5.0), p(0.0, 0.0)];
        let run = [p(0.0, 0.0), p(0.0, -1.0), p(1.0, -2.0), p(1.0, -3.0), p(2.0, -4.0), p(2.0, -5.0), p(3.0, -6.0)];
        let plain = fuse_candidates(&run, &[&trunk], 0.45, 10.0, None);
        assert!(super::super::octi::any_price(&plain[0].3) > 0.0, "length only: the free-angle chord first {:?}", plain[0]);
        let cfg = super::super::octi::OctiCfg { on: true, ..Default::default() };
        let pitch = 0.45 * mm;
        let c = fuse_candidates(&run, &[&trunk], 0.45, 10.0, Some((&cfg, pitch)));
        assert!(!c.is_empty());
        let (gain, len_gain, _, pts) = &c[0];
        assert_eq!(super::super::octi::any_price(pts), 0.0, "{pts:?}");
        // One elbow: a single bend (at i = 5 the elbow's leg goes on through the last vertex).
        let bends = pts.windows(3).filter(|x| super::super::octi::turn_deg(x[1] - x[0], x[2] - x[1]) > 0.5).count();
        assert_eq!(bends, 1, "one elbow {pts:?}");
        // Same length as the staircase (both monotone in the same two directions).
        assert!(len_gain.abs() < 1e-6, "{len_gain}");
        // The price gain is the style price gain alone, on the DP's scale (bends and jogs).
        let st = |x: &[Vec2]| super::super::octi::style_price(&cfg, pitch, x) / 1e6;
        assert!((*gain - (st(&run) - st(pts))).abs() < 1e-9 && *gain > 0.9, "{gain}");
        // Every free-angle link that is offered pays `any_angle`: none gains here (graded, not
        // cut off: see the next case).
        assert!(c.iter().all(|x| super::super::octi::any_price(&x.3) == 0.0), "{c:?}");
        // A long octilinear detour (20 out, 3 across, 14 back) against its free-angle chord from
        // the trunk end (6.71 mm, any_angle 15 + 13.4): the chord still gains 2.9 and is offered,
        // far below the elbow (3 + 4.24 mm, gain 30.5).
        let run = [p(0.0, 0.0), p(0.0, -20.0), p(3.0, -20.0), p(3.0, -6.0)];
        let c = fuse_candidates(&run, &[&trunk], 0.45, 10.0, Some((&cfg, pitch)));
        let chord = c.iter().find(|x| x.2 == 3 && x.3.len() == 2).expect("the free-angle chord is offered");
        let elbow = c.iter().position(|x| x.2 == 3 && x.3.len() == 3 && super::super::octi::any_price(&x.3) == 0.0).expect("elbow");
        assert!(c[elbow].0 > chord.0, "{c:?}");
        let any_mm = 15.0 * super::super::octi::any_angle_amount((p(3.0, -6.0) - p(0.0, 0.0)).len());
        assert!((chord.0 - (chord.1 + st(&run) - any_mm)).abs() < 1e-6, "{chord:?}");
    }

    /// fuse-octi r2: the junction price of a link candidate is the one of its FIRST LEG (not of the
    /// chord), and a run that holds a piece the style forbids is priced with the bans lifted.
    #[test]
    fn fuse_prices_the_first_leg_and_lifts_the_bans_of_the_old_run() {
        let mm = 1e6;
        let p = |x: f64, y: f64| v(x * mm, y * mm);
        let trunk = [p(0.0, 5.0), p(0.0, 0.0)];
        let (ws, jw) = (0.45, 10.0);
        let cfg = super::super::octi::OctiCfg { on: true, ..Default::default() };
        let pitch = 0.45 * mm;
        let st = |c: &super::super::octi::OctiCfg, x: &[Vec2]| super::super::octi::style_price(c, pitch, x) / 1e6;
        let len = |x: &[Vec2]| x.windows(2).map(|w| (w[1] - w[0]).len()).sum::<f64>() / 1e6;
        let psi = |first: Vec2| {
            let deg = first.unit().unwrap().dot(p(0.0, 1.0)).clamp(-1.0, 1.0).acos().to_degrees();
            jw * ws * super::super::factors::junction_psi(deg)
        };
        // A staircase run leaving the trunk end straight down.
        let run = [p(0.0, 0.0), p(0.0, -1.0), p(1.0, -2.0), p(1.0, -3.0), p(2.0, -4.0), p(2.0, -5.0), p(3.0, -6.0)];
        let c = fuse_candidates(&run, &[&trunk], ws, jw, Some((&cfg, pitch)));
        let mut checked = 0;
        for (gain, _, i, pts) in &c {
            // pts = foot, link legs (ending at run[i]), run[i+1..]
            let tail = run.len() - 1 - i;
            let link = &pts[..pts.len() - tail];
            let want = len(&run[..=*i]) + psi(run[1] - run[0]) - len(link) - psi(pts[1] - pts[0]) + st(&cfg, &run) - st(&cfg, pts);
            assert!((gain - want).abs() < 1e-9, "i {i} {pts:?}: gain {gain} want {want}");
            checked += 1;
        }
        assert!(checked >= 4, "{c:?}");
        // The first legs differ (vertical or 45 degrees), so the junction prices differ: the test
        // fails if the chord's direction were used for all.
        let firsts: std::collections::HashSet<i64> = c.iter().map(|x| ((x.3[1] - x.3[0]).y / mm * 100.0).round() as i64).collect();
        assert!(firsts.len() >= 2, "{firsts:?}");
        // A run that leaves the trunk sideways: the first legs of the links are 90 and 135 degrees
        // from the trunk arm, the chord 112 degrees (psi 0.04 / 0 / 0.0003), so a junction price
        // taken from the chord instead of the first leg gives a different gain.
        let side = [p(0.0, 0.0), p(3.0, 0.0), p(3.0, -2.0), p(5.0, -2.0)];
        let c = fuse_candidates(&side, &[&trunk], ws, jw, Some((&cfg, pitch)));
        let mut legs: Vec<i64> = Vec::new();
        for (gain, _, i, pts) in c.iter().filter(|x| x.2 == 3) {
            let link = &pts[..pts.len()];
            let want = len(&side[..=*i]) + psi(side[1] - side[0]) - len(link) - psi(pts[1] - pts[0]) + st(&cfg, &side) - st(&cfg, pts);
            assert!((gain - want).abs() < 1e-9, "i {i} {pts:?}: gain {gain} want {want}");
            legs.push(((pts[1] - pts[0]).y / mm * 10.0).round() as i64);
        }
        legs.sort();
        legs.dedup();
        assert!(legs.len() >= 2 && psi(p(3.0, 0.0)) > psi(p(1.0, -1.0)), "first legs {legs:?}");
        // `any_angle = "forbidden"`: the old run holds a free-angle piece; it is priced with the ban
        // lifted (finite), so the octilinear links still gain by the style price.
        let free = [p(0.0, 0.0), p(1.0, -4.0), p(1.0, -5.0), p(3.0, -6.0)];
        let fcfg = super::super::octi::OctiCfg { any_forbidden: true, ..cfg };
        let soft = super::super::octi::OctiCfg { any_forbidden: false, ..cfg };
        let c = fuse_candidates(&free, &[&trunk], ws, jw, Some((&fcfg, pitch)));
        assert!(!c.is_empty());
        for (gain, _, i, pts) in &c {
            assert!(st(&fcfg, pts).is_finite(), "forbidden pieces are never offered");
            let tail = free.len() - 1 - i;
            let link = &pts[..pts.len() - tail];
            let want = len(&free[..=*i]) + psi(free[1] - free[0]) - len(link) - psi(pts[1] - pts[0]) + st(&soft, &free) - st(&fcfg, pts);
            assert!((gain - want).abs() < 1e-9, "i {i} {pts:?}: gain {gain} want {want}");
        }
    }

    #[test]
    fn fuse_style_is_off_in_the_smooth_style_and_carries_the_pitch() {
        let cfg = super::super::octi::OctiCfg { on: true, ..Default::default() };
        assert!(fuse_style(false, &cfg, 450_000.0).is_none());
        let (c, pitch) = fuse_style(true, &cfg, 450_000.0).expect("octilinear");
        assert!(c.on && (pitch - 450_000.0).abs() < 1e-9);
    }

    /// The fuse queue run to its end with `gain(i, taken)` as the evaluation (the key of item `i`
    /// on the copper after the fuses `taken`; `None`: no legal candidate): the order taken.
    fn lazy_run(items: &[(WireId, usize, i32)], gain: &dyn Fn(usize, &[usize]) -> Option<f64>) -> (Vec<usize>, usize) {
        let mut taken: Vec<usize> = Vec::new();
        let mut idx: Vec<usize> = (0..items.len()).collect();
        let mut pool: Vec<FuseKey> = items.iter().enumerate().map(|(i, &(w, end, net))| FuseKey { key: gain(i, &taken).unwrap_or(f64::NEG_INFINITY), stamp: 0, w, end, net }).collect();
        let (mut version, mut evals) = (0, 0);
        loop {
            match lazy_next(&pool, version) {
                LazyStep::Done => break,
                LazyStep::Reeval(k) => {
                    evals += 1;
                    pool[k].key = gain(idx[k], &taken).unwrap_or(f64::NEG_INFINITY);
                    pool[k].stamp = version;
                }
                LazyStep::Take(k) => {
                    taken.push(idx[k]);
                    let net = pool[k].net;
                    pool.swap_remove(k);
                    idx.swap_remove(k);
                    version = lazy_taken(&mut pool, version, net);
                }
            }
            assert!(evals < 100, "the queue does not end");
        }
        (taken, evals)
    }

    /// fuse-octi r4 (M1): best legal gain first, a stale key made again before its end is taken.
    #[test]
    fn the_fuse_queue_takes_the_best_fresh_gain_first() {
        // qmtech JS1_2: wire 383 (2.67) and wire 480 (43.3) share a pad; whichever fuses there takes
        // it from the other. The large one goes first whatever the ids, both ways round.
        for (a, b) in [(383, 480), (480, 383)] {
            let items = [(a, 0, 78), (b, 1, 78)];
            let g = |i: usize, t: &[usize]| if !t.is_empty() { None } else { Some(if items[i].0 == 480 { 43.3 } else { 2.67 }) };
            let (t, _) = lazy_run(&items, &g);
            assert_eq!(t.iter().map(|&i| items[i].0).collect::<Vec<_>>(), vec![480], "ids {a} {b}");
        }
        // D (net 1, 12) first; it drops C (net 1) from 10 to 1, so E (net 2, 5) goes before C; F (net 1)
        // had no candidate until D fused and then gets one (3): order D, E, F, C.
        let items = [(1, 0, 1), (2, 0, 1), (3, 0, 2), (4, 0, 1)]; // C, D, E, F
        let g = |i: usize, t: &[usize]| -> Option<f64> {
            let d = t.contains(&1);
            match i {
                0 => Some(if d { 1.0 } else { 10.0 }),
                1 => Some(12.0),
                2 => Some(5.0),
                _ => d.then_some(3.0),
            }
        };
        let (t, evals) = lazy_run(&items, &g);
        assert_eq!(t, vec![1, 2, 3, 0]);
        // Lazy: C and F made again after D (same net), C again after F; E never (other net), nothing after E.
        assert_eq!(evals, 3);
        // Ties: by wire id, then end; an end without a candidate is never taken.
        let items = [(9, 1, 1), (9, 0, 2), (5, 1, 3), (7, 0, 4)];
        let g = |i: usize, _: &[usize]| if i == 3 { None } else { Some(2.0) };
        assert_eq!(lazy_run(&items, &g).0, vec![2, 1, 0]);
        // A fresh best is taken at once; any stale key (also a low or `-inf` one) is made again first.
        let k = |key: f64, stamp: usize, w: WireId| FuseKey { key, stamp, w, end: 0, net: 1 };
        assert_eq!(lazy_next(&[k(1.0, 3, 1), k(5.0, 3, 2)], 3), LazyStep::Take(1));
        assert_eq!(lazy_next(&[k(1.0, 3, 1), k(5.0, 2, 2)], 3), LazyStep::Reeval(1));
        assert_eq!(lazy_next(&[k(1.0, 2, 1), k(5.0, 3, 2)], 3), LazyStep::Reeval(0));
        assert_eq!(lazy_next(&[k(f64::NEG_INFINITY, 3, 1)], 3), LazyStep::Done);
        assert_eq!(lazy_next(&[k(9.0, 3, 1), k(f64::NEG_INFINITY, 1, 2)], 3), LazyStep::Reeval(1));
        assert_eq!(lazy_next(&[], 0), LazyStep::Done);
        // A fuse keeps the other nets' fresh keys fresh, its own net's stale; an older stale one stays stale.
        let mut p = [FuseKey { net: 1, ..k(1.0, 4, 1) }, FuseKey { net: 2, ..k(1.0, 4, 2) }, FuseKey { net: 2, ..k(1.0, 3, 3) }];
        assert_eq!(lazy_taken(&mut p, 4, 1), 5);
        assert_eq!(p.map(|x| x.stamp), [4, 5, 3]);
    }

    #[test]
    fn the_fuse_cap_counts_vertices_and_keeps_every_shape_of_them() {
        let c = |g: f64, i: usize| (g, 0.0, i, vec![v(0.0, 0.0)]);
        // vertex 1 has shapes at gain 9 and 1 (the second after vertices 2, 3 appeared); cap 2 keeps vertices 1 and 2.
        let all = vec![c(9.0, 1), c(8.0, 2), c(7.0, 3), c(1.0, 1), c(0.5, 4), c(0.4, 2)];
        let got = fuse_cap(all.clone(), 2);
        assert_eq!(got.iter().map(|x| (x.2, x.0)).collect::<Vec<_>>(), vec![(1, 9.0), (2, 8.0), (1, 1.0), (2, 0.4)]);
        // a cap above the vertex count keeps everything; cap 0 nothing.
        assert_eq!(fuse_cap(all.clone(), 99).len(), 6);
        assert!(fuse_cap(all, 0).is_empty());
        assert_eq!(FUSE_VERTEX_CAP, 8);
    }

    #[test]
    fn a_run_that_drew_nothing_never_fails_blank() {
        // M6 F-1: no attempt produced copper (empty reason): the wire fails as a drawing failure
        // naming its layer, so the never-drop pass (key "drawing:") draws it; an attempt's own
        // reason stays as it was.
        let m = BoardModel::from_json(include_str!("../../tests/fixtures/fx-narrow-gap.json")).expect("fixture parses");
        let l = &m.layers[0];
        let r = failed_run_reason(String::new(), &m, l.id);
        assert!(r.starts_with("drawing: no run found on layer "), "{r}");
        assert!(r.contains(&l.name), "{r}");
        assert_eq!(failed_run_reason("drawing: blocked by GND".into(), &m, l.id), "drawing: blocked by GND");
        // A layer id the model does not have is named by its number.
        assert!(failed_run_reason(String::new(), &m, 9999).starts_with("drawing: no run found on layer 9999 "));
    }

    #[test]
    fn plan_splits_runs_at_label_changes() {
        let p = plan(&[(0, 0.0, 0), (2, 0.5, 0), (4, 0.2, 1), (6, 1.0, 1)], 2);
        assert_eq!(p.layers, vec![0, 1]);
        assert_eq!(p.vias, vec![(2, 4)]);
        assert_eq!(plan(&[], 2), Plan { layers: vec![1], vias: vec![], at: vec![] });
    }

    // ---- fuse-octi r4: Drawer-level tests of the polish's split guard and the fuse's best legal candidate.

    const FMM: f64 = 1e6;

    /// A 40 x 30 mm board, F.Cu only (B.Cu a keepout), 1 mm square SMD pads `(net, x, y)` in the
    /// KiCad frame; clearance 0.2 mm, width 0.25 mm.
    fn fx_model(pads: &[(i32, f64, f64)]) -> BoardModel {
        fx_model_with(pads, true)
    }

    /// [`fx_model`]; `keepout` false: B.Cu open for vias (the forced via fixture).
    fn fx_model_with(pads: &[(i32, f64, f64)], keepout: bool) -> BoardModel {
        BoardModel::from_json(&fx_json(pads, keepout).to_string()).unwrap()
    }

    /// The board JSON of [`fx_model_with`] (a fixture edits it before it is read).
    fn fx_json(pads: &[(i32, f64, f64)], keepout: bool) -> serde_json::Value {
        let n = |v: f64| (v * FMM).round() as i64;
        let (w, h) = (n(40.0), n(30.0));
        let pj: Vec<serde_json::Value> = pads
            .iter()
            .enumerate()
            .map(|(i, &(net, x, y))| {
                serde_json::json!({"uuid": format!("00000000-0000-4000-8000-{i:012}"), "ref": format!("P{i}"), "number": "1", "net": net, "attr": "smd",
                    "center": [n(x), n(y)], "rotation_deg": 0.0, "layers": [0], "shapes": {"0": {"kind": "rect", "w": n(1.0), "h": n(1.0)}},
                    "drill": null, "local_clearance": null, "net_tie_group": null, "spoke_angle_deg": 45.0})
            })
            .collect();
        let cls = serde_json::json!({"clearance": 200000, "width": 250000, "via_d": 600000, "via_drill": 300000, "netclass": "Default", "width_source": "default", "min_width": 0});
        let j = serde_json::json!({
            "units": "nm", "board": "fx",
            "layers": [{"id": 0, "name": "F.Cu", "kind": "signal", "routable": true}, {"id": 2, "name": "B.Cu", "kind": "signal", "routable": true}],
            "nets": [{"code": 1, "name": "N"}, {"code": 2, "name": "M"}], "pads": pj,
            "vias": [], "kept": [], "copper_shapes": [], "holes": [],
            "outline": [{"kind": "seg", "a": [0, 0], "b": [w, 0]}, {"kind": "seg", "a": [w, 0], "b": [w, h]}, {"kind": "seg", "a": [w, h], "b": [0, h]}, {"kind": "seg", "a": [0, h], "b": [0, 0]}],
            "keepouts": if keepout { serde_json::json!([{"name": "", "layers": [2], "poly": [[0, 0], [w, 0], [w, h], [0, h]], "no_tracks": true, "no_vias": true, "no_pour": false, "no_pads": false, "no_footprints": false}]) } else { serde_json::json!([]) },
            "net_ties": [], "pours": [], "teardrops": [], "dangling_ends": [],
            "rules": {"min_clearance": 200000, "min_track_width": 250000, "min_via_diameter": 400000, "min_via_drill": 300000, "min_annular": 50000, "hole_clearance": 250000, "via_hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000, "max_error": 5000, "min_connection": 0, "profile": "board", "predefined_vias": [], "default": cls, "per_net": {"1": cls, "2": cls}, "floors_applied": [], "fallbacks": [], "warnings": [], "ignored_checks": [], "clearance_regions": [], "outline_half_stroke": 25000}
        });
        j
    }

    /// grid-snap r4 (verifier F7, mutant V3): [`Drawer::force_via`] ranks its sites by
    /// [`force_via_key`] (shorts first, then the drill, then the rest), not by one sum of amounts.
    /// Fixture: wire A (5, 15) - B (35, 15) of net 1, no passes; its sites are the chord's
    /// (fractions 0, 0.1, ..., 1 of A-B, [`Drawer::chord_sites`]) and those of A's triangle. A thin
    /// net-3 track grazes the chord's middle (20, 15): a short and a drill deficit, about 22 units.
    /// Two net-2 tracks 1.5 mm off the chord cover every other site with net 2's clearance (set to
    /// 2 mm): one deep clearance deficit each, about 52 units, no short. The forced via takes a
    /// clearance-only site; by one sum it would take the short.
    #[test]
    fn force_via_takes_a_clearance_site_over_a_cheaper_short() {
        let mut j = fx_json(&[(1, 5.0, 15.0), (1, 35.0, 15.0)], false);
        j["nets"] = serde_json::json!([{"code": 1, "name": "N"}, {"code": 2, "name": "M"}, {"code": 3, "name": "S"}]);
        j["rules"]["per_net"]["2"]["clearance"] = serde_json::json!(2_000_000);
        let m = BoardModel::from_json(&j.to_string()).unwrap();
        let b = super::super::board::build(&m, &super::super::board::BuildOpts::default()).unwrap();
        let pr = &b.proj;
        let o = Options::default();
        let ck = Checker::new(&m, &o);
        let mut st = State::new(&b.proj);
        let obs = |x: f64, y: f64| (0..pr.obstacles.len()).find(|&i| (pr.obstacles[i].center - fp(x, y)).len() < 1e3).unwrap();
        let w = fx_wire(&mut st, &b, obs(5.0, 15.0), obs(35.0, 15.0));
        let sz = ViaSize { d: 600_000, drill: 300_000 };
        let mut dr = Drawer::new(&m, &o, &b, &st, &ck);
        let s0 = fp(20.0, 15.0);
        // The short: a 0.1 mm net-3 track 0.02 mm into the via's copper (0.3 + 0.05 - 0.02 off s0).
        dr.commit(&track_item(3, 0, 0.1 * FMM, fp(20.33, 14.8), fp(20.33, 15.2), None));
        // Net 2, 1.5 mm below the chord, clear of s0 by more than its 2 mm clearance.
        dr.commit(&track_item(2, 0, 0.1 * FMM, fp(2.0, 16.5), fp(16.0, 16.5), None));
        dr.commit(&track_item(2, 0, 0.1 * FMM, fp(24.0, 16.5), fp(38.0, 16.5), None));
        let sites: Vec<Vec2> = dr.chord_sites(w, 0).into_iter().chain(dr.via_candidates(w, 0, 0, 0.0, Some(0))).map(|x| x.1).collect();
        let t = st.wires[w].tris[0];
        let [ta, tb, tc] = pr.tris[t].map(|v| pr.pos[v]);
        let sites: Vec<Vec2> = sites.into_iter().chain(super::super::pack::tri_site_points(ta, tb, tc)).collect();
        assert!(sites.iter().any(|p| (*p - s0).len() < 1.0), "the chord's middle is a site: {sites:?}");
        let viols_at = |p: Vec2| {
            let item = via_item(1, p, sz, Some(w));
            let rep = dr.checker.check_against(&dr.idx, std::slice::from_ref(&item), &[]);
            dr.viols_of(std::slice::from_ref(&item), &rep)
        };
        // Precondition: the short site has a lower sum than every other site, none of which shorts.
        let sum = |vs: &[Viol]| vs.iter().map(Viol::amount).sum::<f64>();
        let v0 = viols_at(s0);
        assert!(v0.iter().any(|v| v.kind == "short"), "{v0:?}");
        for &p in sites.iter().filter(|p| (**p - s0).len() > 1.0) {
            let vs = viols_at(p);
            assert!(!vs.iter().any(|v| v.kind == "short") && sum(&vs) > sum(&v0) + 2.0, "site {p:?}: {vs:?} vs {v0:?}");
        }
        let plan = Plan { layers: vec![0, 1], vias: vec![(0, 0)], at: vec![0] };
        let (_, p, _, vs) = dr.force_via(w, &plan, 0, 0, 0, &[sz], None).expect("a forced site");
        assert!((p - s0).len() > 1.0 && !vs.is_empty() && !vs.iter().any(|v| v.kind == "short"), "took {p:?}: {vs:?}");
    }

    /// Math-frame point of the KiCad-frame mm coordinates.
    fn fp(x: f64, y: f64) -> Vec2 {
        Vec2 { x: x * FMM, y: -y * FMM }
    }

    /// A wire of net 1 between the pads `pa` and `pb` (obstacle ids), no passes.
    fn fx_wire(st: &mut State, b: &Board, pa: usize, pb: usize) -> WireId {
        let (a, z) = (b.centre[pa].unwrap(), b.centre[pb].unwrap());
        let t = b.proj.vert_tris[a][0];
        st.insert(super::super::state::Wire { net: 1, w: 0.25 * FMM, s: 0.2 * FMM, a, b: z, pa, pb, turns: vec![], passes: vec![], tris: vec![], alive: true }, &[], vec![t])
    }

    /// The drawn wire `w` with the one run `pts` on F.Cu, committed.
    fn fx_drawn(dr: &mut Drawer, w: WireId, pts: Vec<Vec2>) -> WireOut {
        let items = dr.run_items(w, 0, &pts);
        let handles = items.iter().map(|it| dr.commit(it)).collect();
        WireOut { wire: w, ok: true, items, runs: vec![(0, pts)], handles, ..WireOut::default() }
    }

    fn pieces_of(model: &BoardModel, b: &Board, wires: &[&WireOut]) -> super::super::netconn::Conn {
        let _ = model;
        let pcs: Vec<super::super::netconn::Piece> = wires.iter().map(|wo| super::super::netconn::Piece { runs: &wo.runs, vias: vec![], half_w: 0.125 * FMM }).collect();
        let ts: Vec<super::super::netconn::Term> = b.proj.obstacles.iter().filter(|o| o.net == Some(1)).map(|o| super::super::netconn::Term { poly: &o.poly, layers: o.layers, center: o.center }).collect();
        super::super::netconn::conn(&pcs, &ts)
    }

    fn octilinear(pts: &[Vec2]) -> bool {
        pts.windows(2).all(|p| (p[1] - p[0]).len2() <= 1.0 || super::super::octi::octi_dir(p[1] - p[0]).is_some())
    }

    /// fuse-octi r4, H1 (qmtech JS2_9, main's JS1_2): a trunk A-B drawn as one free-angle segment
    /// with a branch from C whose foot F lies on it (no elbow or Z of A-B passes F). Part 1: the
    /// polish keeps F as a vertex, so both pieces end on it and the net stays one piece. Part 2: the
    /// split guard: without F as a vertex every shape drops the foot, and the guard refuses them all.
    #[test]
    fn the_polish_keeps_a_branch_foot_on_the_trunk_or_refuses_the_step() {
        let m = fx_model(&[(1, 5.0, 5.0), (1, 25.0, 15.0), (1, 11.0, 25.0)]);
        let b = super::super::board::build(&m, &super::super::board::BuildOpts::default()).unwrap();
        let o = Options::default();
        let ck = Checker::new(&m, &o);
        let mut st = State::new(&b.proj);
        let obs = |x: f64, y: f64| (0..b.proj.obstacles.len()).find(|&i| (b.proj.obstacles[i].center - fp(x, y)).len() < 1e3).unwrap();
        let (oa, ob, oc) = (obs(5.0, 5.0), obs(25.0, 15.0), obs(11.0, 25.0));
        let wt = fx_wire(&mut st, &b, oa, ob);
        let wb = fx_wire(&mut st, &b, oc, oa);
        let (a, z, c, f) = (fp(5.0, 5.0), fp(25.0, 15.0), fp(11.0, 25.0), fp(11.0, 8.0));
        let mut dr = Drawer::new(&m, &o, &b, &st, &ck);
        let branch = fx_drawn(&mut dr, wb, vec![c, f]);
        let trunk0 = fx_drawn(&mut dr, wt, vec![a, z]);
        let c0 = pieces_of(&m, &b, &[&trunk0, &branch]);
        assert!(c0.n() == 1 && c0.dangling.is_empty(), "{c0:?}");
        // Part 1: the foot kept as a vertex.
        let mut t = trunk0.clone();
        let k = dr.octi_polish(&mut t, &[f], &mut |_, _| true);
        assert!(k >= 2 && octilinear(&t.runs[0].1), "{k} {:?}", t.runs[0].1);
        // The foot stays on the centre line (a vertex while the two halves are redrawn; r6: merged
        // into one straight segment where both halves leave it in one direction, as KiCad Cleanup would).
        assert!(t.runs[0].1.windows(2).any(|s| crate::geom::point_seg(f, &crate::geom::Seg { a: s[0], b: s[1] }) < 1.0), "the foot is on the trunk: {:?}", t.runs[0].1);
        assert_eq!(t.runs[0].1, crate::topo::octi::drop_collinear(&t.runs[0].1), "no collinear vertex left");
        assert!(!super::super::netconn::worse(&c0, &pieces_of(&m, &b, &[&t, &branch])));
        // Part 2: the guard (netconn: partition and dangling ends) refuses every shape that drops the
        // foot (each elbow / Z either leaves the branch apart or crossing it with its foot dangling).
        let back = dr.restore_wire(&t, &trunk0);
        let mut t = back.clone();
        let mut refused = 0;
        let mut guard = |_: &WireOut, now: &WireOut| {
            let ok = !super::super::netconn::worse(&c0, &pieces_of(&m, &b, &[now, &branch]));
            refused += usize::from(!ok);
            ok
        };
        let k = dr.octi_polish(&mut t, &[], &mut guard);
        assert_eq!(k, 0, "{:?}", t.runs[0].1);
        assert!(refused >= 4, "both elbows and both Zs tried: {refused}");
        assert_eq!(t.runs[0].1, vec![a, z]);
        assert!(dr.handles_hold(&t), "the refused shapes leave the drawing's index as it was");
        // Without the guard the same polish leaves the foot dangling (the r3 bug).
        let mut t2 = t.clone();
        assert!(dr.octi_polish(&mut t2, &[], &mut |_, _| true) >= 1);
        let c2 = pieces_of(&m, &b, &[&t2, &branch]);
        assert!(super::super::netconn::worse(&c0, &c2) && !c2.dangling.is_empty(), "{c2:?}");
    }

    /// fuse-octi r4 (M1, F1/L1): the fuse's best LEGAL candidate onto one trunk: a branch from the
    /// trunk's pad A that runs 2 mm beside the trunk past twelve vertices; the further along, the
    /// larger the gain. The best legal one is taken; a vertex whose link a foreign pad blocks is
    /// passed over; only the 8 best vertices are tried ([`FUSE_VERTEX_CAP`], every shape of them).
    #[test]
    fn the_fuse_takes_the_best_legal_candidate_of_the_capped_vertices() {
        for blocked in [false, true] {
            // The foreign pad (net 2) on the straight link of the best vertex (x = 31): (31, 19).
            let mut pads = vec![(1, 3.0, 20.0), (1, 37.0, 20.0), (1, 33.0, 18.0)];
            if blocked {
                pads.push((2, 31.0, 19.0));
            }
            let m = fx_model(&pads);
            let b = super::super::board::build(&m, &super::super::board::BuildOpts::default()).unwrap();
            let o = Options::default();
            let ck = Checker::new(&m, &o);
            let mut st = State::new(&b.proj);
            let obs = |x: f64, y: f64| (0..b.proj.obstacles.len()).find(|&i| (b.proj.obstacles[i].center - fp(x, y)).len() < 1e3).unwrap();
            let (oa, ob, oc) = (obs(3.0, 20.0), obs(37.0, 20.0), obs(33.0, 18.0));
            let wt = fx_wire(&mut st, &b, oa, ob);
            let wb = fx_wire(&mut st, &b, oa, oc);
            let mut dr = Drawer::new(&m, &o, &b, &st, &ck);
            let trunk = vec![fp(3.0, 20.0), fp(37.0, 20.0)];
            let _t = fx_drawn(&mut dr, wt, trunk.clone());
            // Branch: A, 45 degrees up to y = 18, east past x = 9, 11, ..., 31 (vertices 2..=13) to the
            // pad C at (33, 18) (i 14). The gain of vertex i (2 mm link) grows with i.
            let mut run = vec![fp(3.0, 20.0), fp(5.0, 18.0)];
            run.extend((0..12).map(|k| fp(9.0 + 2.0 * k as f64, 18.0)));
            run.push(fp(33.0, 18.0));
            let wo = fx_drawn(&mut dr, wb, run.clone());
            let tr: Vec<&[Vec2]> = vec![trunk.as_slice()];
            let best = |skip: &mut dyn FnMut(f64, usize, &[Vec2]) -> bool| dr.fuse_best_legal(&wo, 0, &run, false, &tr, 0.45, 0.0, None, skip);
            // Unblocked: the vertex furthest along, the pad C (i 14: its foot (33, 20), a 2 mm link).
            let (gain, _, i, pts) = best(&mut |_, _, _| false).expect("a legal fuse");
            if !blocked {
                assert_eq!(i, 14, "{gain} {pts:?}");
                assert!((pts[0] - fp(33.0, 20.0)).len() < 1.0);
                // The legal test is refit_run's: taken there too.
                let mut w2 = wo.clone();
                assert!(dr.refit_run(&mut w2, 0, pts.clone()));
                let back = dr.restore_wire(&w2, &wo);
                assert!(dr.handles_hold(&back));
                continue;
            }
            // Blocked: the foreign pad at (31, 19) sits on the straight link of x = 31 (i 13) only;
            // x = 33's stays legal and is still the best.
            assert_eq!(i, 14, "{gain} {pts:?}");
            // Skip the best (i 14): the next legal one is i 12 (x = 29), not the blocked i 13.
            let (_, _, i, _) = best(&mut |_, i, _| i == 14).unwrap();
            assert_eq!(i, 12);
            // The cap: with the 7 best vertices skipped the 8th best (i 7) is still tried; with the 8
            // best skipped nothing is (the 9th best, i 6, would be legal).
            let (_, _, i, _) = best(&mut |_, i, _| i >= 8).unwrap();
            assert_eq!(i, 7);
            assert!(best(&mut |_, i, _| i >= 7).is_none());
            let p13: Vec<Vec2> = std::iter::once(fp(31.0, 20.0)).chain(run[13..].iter().copied()).collect();
            let mut w2 = wo.clone();
            assert!(!dr.refit_legal(&wo, 0, &p13) && !dr.refit_run(&mut w2, 0, p13), "the blocked link is illegal for both");
        }
    }

    /// first-draw-45 r9 (esp runtime, 53 s -> 650 s at r8): a refit the checker refuses commits the
    /// run's old tracks again as new objects; the dead ones must leave the spatial grid, or every
    /// grid query near the run (the repair, the exact search, the octilinear DP) scans a pile that
    /// grows with each refused refit (esp: the straightening's refused forms, then 22 via
    /// reduction tries of 3-55 s each). A run from pad A (3, 20) to pad B (13, 20) and a foreign
    /// track across y = 17; 200 refused refits (a run through that track) leave the grid as large
    /// as after the first, its queries return no dead object, and the run's copper is the same.
    #[test]
    fn refused_refits_leave_no_dead_objects_in_the_grid() {
        let m = fx_model(&[(1, 3.0, 20.0), (1, 13.0, 20.0)]);
        let b = super::super::board::build(&m, &super::super::board::BuildOpts::default()).unwrap();
        let o = Options::default();
        let ck = Checker::new(&m, &o);
        let mut st = State::new(&b.proj);
        let obs = |x: f64, y: f64| (0..b.proj.obstacles.len()).find(|&i| (b.proj.obstacles[i].center - fp(x, y)).len() < 1e3).unwrap();
        let w = fx_wire(&mut st, &b, obs(3.0, 20.0), obs(13.0, 20.0));
        let mut dr = Drawer::new(&m, &o, &b, &st, &ck);
        dr.commit(&track_item(2, 0, 0.25 * FMM, fp(5.0, 17.0), fp(11.0, 17.0), None));
        let run = vec![fp(3.0, 20.0), fp(13.0, 20.0)];
        let mut wo = fx_drawn(&mut dr, w, run.clone());
        let bad = vec![fp(3.0, 20.0), fp(8.0, 15.0), fp(13.0, 20.0)];
        assert!(!dr.refit_run(&mut wo, 0, bad.clone()), "the run through the foreign track is refused");
        let n1 = dr.grid.len();
        for _ in 0..200 {
            assert!(!dr.refit_run(&mut wo, 0, bad.clone()));
        }
        assert_eq!(dr.grid.len(), n1, "refused refits piled dead objects into the grid");
        let ids = dr.grid.query(bbox_of(&run).grow(5.0 * FMM));
        assert!(ids.iter().all(|&i| dr.ents[i].live), "a grid query returned a dead object");
        assert_eq!(wo.runs[0].1, run, "the run keeps its copper");
        assert!(wo.handles.iter().all(|&h| dr.ents[dr.handles[h].1].live), "the run's tracks are live");
    }

    /// first-draw-45 r7 (kills the r6 survivor "no end-leg rule"): the straightening keeps the
    /// direction and length of a run's end legs (the pad exit or via leg the DP chose,
    /// [`Drawer::octi_straighten`]). A run from pad A (3, 20) to pad B (13, 16), both legs east
    /// (a 1 mm exit leg out of A, a 3 mm leg into B), a staircase of four bends between. The whole
    /// stretch A..B has a one-bend elbow (45 degrees out of A, then east into B: same length, one
    /// bend fewer than the Z), cheaper on the price, but it turns A's exit leg; the rule takes
    /// the Z east-45-east instead, both end legs east and at least their length or the jog leg.
    #[test]
    fn straightening_keeps_the_end_legs() {
        let m = fx_model(&[(1, 3.0, 20.0), (1, 13.0, 16.0)]);
        let b = super::super::board::build(&m, &super::super::board::BuildOpts::default()).unwrap();
        let o = Options::default();
        let ck = Checker::new(&m, &o);
        let mut st = State::new(&b.proj);
        let obs = |x: f64, y: f64| (0..b.proj.obstacles.len()).find(|&i| (b.proj.obstacles[i].center - fp(x, y)).len() < 1e3).unwrap();
        let w = fx_wire(&mut st, &b, obs(3.0, 20.0), obs(13.0, 16.0));
        let mut dr = Drawer::new(&m, &o, &b, &st, &ck);
        dr.octi = super::super::octi::OctiCfg { on: true, ..dr.octi };
        let run = vec![fp(3.0, 20.0), fp(4.0, 20.0), fp(6.0, 18.0), fp(8.0, 18.0), fp(10.0, 16.0), fp(13.0, 16.0)];
        let mut wo = fx_drawn(&mut dr, w, run.clone());
        let k = dr.octi_straighten(&mut wo, &[], &mut |_, _| true);
        let got = super::super::octi::drop_collinear(&wo.runs[0].1);
        assert!(k >= 1 && got.len() < run.len(), "straightened: {k} {got:?}");
        let east = |a: Vec2, b: Vec2| super::super::octi::octi_dir(b - a) == Some(0);
        let n = got.len();
        assert!(east(got[0], got[1]) && (got[1] - got[0]).len() + 1e3 >= 1.0 * FMM, "A's exit leg turned or cut: {got:?}");
        assert!(east(got[n - 2], got[n - 1]) && (got[n - 1] - got[n - 2]).len() + 1e3 >= super::super::octi::jog_leg(&dr.octi, 0.45 * FMM).min(3.0 * FMM), "B's leg turned or cut: {got:?}");
        assert!(octilinear(&got), "{got:?}");
        // Part 2, the length half of the rule: pad B at (7.4, 16), a 0.3 mm exit leg east out of A,
        // then 45 degrees, 0.05 mm east, 45 degrees, 0.05 mm east into B (four bends). The whole
        // stretch's centred Z keeps both end legs east but only 0.2 mm long: refused (below the
        // exit leg's 0.3 mm, under the jog leg); a form that keeps A's leg at 0.3 mm or more is
        // taken instead.
        let m = fx_model(&[(1, 3.0, 20.0), (1, 7.4, 16.0)]);
        let b = super::super::board::build(&m, &super::super::board::BuildOpts::default()).unwrap();
        let ck = Checker::new(&m, &o);
        let mut st = State::new(&b.proj);
        let obs = |x: f64, y: f64| (0..b.proj.obstacles.len()).find(|&i| (b.proj.obstacles[i].center - fp(x, y)).len() < 1e3).unwrap();
        let w = fx_wire(&mut st, &b, obs(3.0, 20.0), obs(7.4, 16.0));
        let mut dr = Drawer::new(&m, &o, &b, &st, &ck);
        dr.octi = super::super::octi::OctiCfg { on: true, ..dr.octi };
        let leff = super::super::octi::jog_leg(&dr.octi, 0.45 * FMM);
        assert!(leff > 0.3 * FMM, "the fixture needs the jog leg above the exit leg: {leff}");
        let run = vec![fp(3.0, 20.0), fp(3.3, 20.0), fp(5.3, 18.0), fp(5.35, 18.0), fp(7.35, 16.0), fp(7.4, 16.0)];
        let mut wo = fx_drawn(&mut dr, w, run.clone());
        let k = dr.octi_straighten(&mut wo, &[], &mut |_, _| true);
        let got = super::super::octi::drop_collinear(&wo.runs[0].1);
        assert!(k >= 1 && got.len() < run.len(), "part 2 straightened: {k} {got:?}");
        assert!(east(got[0], got[1]) && (got[1] - got[0]).len() + 1e3 >= 0.3 * FMM, "part 2: A's exit leg turned or cut: {got:?}");
        assert!(octilinear(&got), "{got:?}");
    }
}
