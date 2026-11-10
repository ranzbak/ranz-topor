//! Vias and branch points in the abstract (shopping list 5.4, roadmap P7 "via site check +
//! via-pair merge + force move on the topology"): three plan operators that run on the topology
//! and its layering, before anything is drawn. The drawing only realises what they decide.
//!
//! * **Via relocation** ([`relocate_vias`], `[vias] relocate`): every via of the plan gets a
//!   point of its free interval (the stretch of its wire between the two events whose layers
//!   differ, on the wire's taut path, so the length does not change) where the copper around it
//!   pushes equally: the largest clearance gap to the nearest copper (obstacles of other nets on
//!   every layer, other nets' wires, other vias, the board edge), ties broken by the least
//!   repulsion potential `Σ 1/gap` (Dayan & Dai 1991, UCSC-CRL-91-40 pp. 2-3: a movable point
//!   moves along the combined force of fixed-force springs until it balances; p. 5 obstacles
//!   push, p. 6 multi-layer vias feel the copper of every layer they touch, p. 6 the force law is
//!   a choice; Luzin, Popov & Popov 2013 p. 3: "afterwards the via positions are corrected by a
//!   force algorithm"; TopoR 6.3 manual p. 179 "Shift vias"). Between two neighbours that is the
//!   point of equal clearance (pattern `via_force_relocate`). A via is never moved to a point
//!   with a smaller gap than its old one, so the predicted violations never rise.
//! * **Branch-point relocation** ([`relocate_junctions`], `[search] branch_relocate`): the
//!   junction of three arms goes to their Fermat point when every angle of the arm ends is below
//!   120 degrees (Weber point by the same force iteration, Dayan & Dai 1991 pp. 2-4; Popov &
//!   Popov EDN 2013: "the angle should not be less than 120 degrees between the segments
//!   converging at one vertex ... a force-based algorithm ... iteratively moves them to a point of
//!   equilibrium"), into the pad at an angle >= 120 degrees, and out of a pad where two arms
//!   meet there at less than 120 degrees (EDN 2013 fig. 13: "if the angle between a pair of
//!   segments incident to a vertex (terminal or additional) is less than 120 degrees, you can add
//!   a branch point"). The move is homotopic: the new arms keep their clearances to all other
//!   copper, else the point is drawn back towards the old one (Dayan & Dai 1991 p. 5).
//! * **Local via-pair minimisation** ([`pair_candidates`], `[vias] pair_min`): see the function.
//!
//! Units nm, math frame. Deterministic (index order, B-tree maps).

use super::state::{State, WireId};
use crate::geom::Vec2;
use std::collections::BTreeMap;

/// Repulsion of copper farther than this from a via is ignored in the potential (nm).
pub const POT_RANGE: f64 = 5.0e6;
/// Samples per via interval of the maximin search (then refined).
const SAMPLES: usize = 240;

/// The plan's path of one wire: its taut corners ([`State::taut_points`]) with their arc
/// lengths, and where the path crosses each pass's edge (arc length, per pass).
#[derive(Clone, Debug, Default)]
pub struct Track {
    pub pts: Vec<Vec2>,
    pub cum: Vec<f64>,
    /// Arc length of the crossing of pass `k` (non-decreasing).
    pub sig: Vec<f64>,
}

fn seg_point(p: Vec2, a: Vec2, b: Vec2) -> (f64, Vec2) {
    let e = b - a;
    let t = ((p - a).dot(e) / e.len2().max(1e-12)).clamp(0.0, 1.0);
    let q = a + e * t;
    ((p - q).len(), q)
}

/// Distance from `p` to the polyline `pts`.
pub fn dist_polyline(p: Vec2, pts: &[Vec2]) -> f64 {
    match pts.len() {
        0 => f64::INFINITY,
        1 => (p - pts[0]).len(),
        _ => pts.windows(2).map(|x| seg_point(p, x[0], x[1]).0).fold(f64::INFINITY, f64::min),
    }
}

/// Distance from `p` to the outline of `poly` (0 inside).
pub fn dist_poly(p: Vec2, poly: &[Vec2]) -> f64 {
    if super::state::point_in_poly(p, poly) {
        return 0.0;
    }
    let n = poly.len();
    (0..n).map(|i| seg_point(p, poly[i], poly[(i + 1) % n]).0).fold(f64::INFINITY, f64::min)
}

/// Least distance between segments `ab` and `cd`.
pub fn seg_seg(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> f64 {
    let cr = |o: Vec2, p: Vec2, q: Vec2| (p - o).cross(q - o);
    let (d1, d2, d3, d4) = (cr(c, d, a), cr(c, d, b), cr(a, b, c), cr(a, b, d));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0)) {
        return 0.0;
    }
    seg_point(a, c, d).0.min(seg_point(b, c, d).0).min(seg_point(c, a, b).0).min(seg_point(d, a, b).0)
}

impl Track {
    /// The track of wire `w`. O(passes x corners) worst case, O(passes + corners) typically.
    pub fn of(st: &State, w: WireId) -> Track {
        let pts = st.taut_points(w);
        let mut cum = vec![0.0; pts.len()];
        for i in 1..pts.len() {
            cum[i] = cum[i - 1] + (pts[i] - pts[i - 1]).len();
        }
        let wi = &st.wires[w];
        let mut sig = Vec::with_capacity(wi.passes.len());
        let mut j0 = 0usize;
        let mut last = 0.0f64;
        for &p in &wi.passes {
            let [vp, vq] = st.proj.edges[st.passes[p].edge];
            let (a, b) = (st.proj.pos[vp], st.proj.pos[vq]);
            // The first corner segment from j0 on that comes closest to the edge (a crossing or a
            // touch at a span end).
            let mut best: Option<(f64, usize, f64)> = None;
            for j in j0..pts.len().saturating_sub(1) {
                let d = seg_seg(pts[j], pts[j + 1], a, b);
                if best.is_none_or(|x| d < x.0 - 1.0) {
                    // Arc length of the point of segment j nearest the edge.
                    let (pj, pk) = (pts[j], pts[j + 1]);
                    let mut tb = 0.0;
                    let mut db = f64::INFINITY;
                    for s in 0..=16 {
                        let t = s as f64 / 16.0;
                        let q = pj + (pk - pj) * t;
                        let dq = seg_point(q, a, b).0;
                        if dq < db {
                            db = dq;
                            tb = t;
                        }
                    }
                    best = Some((d, j, cum[j] + tb * (pk - pj).len()));
                }
                if best.is_some_and(|x| x.0 <= 1.0) {
                    break;
                }
            }
            let s = best.map_or(last, |b| {
                j0 = b.1;
                b.2
            });
            last = s.max(last);
            sig.push(last);
        }
        Track { pts, cum, sig }
    }

    pub fn len(&self) -> f64 {
        self.cum.last().copied().unwrap_or(0.0)
    }

    pub fn is_empty(&self) -> bool {
        self.pts.is_empty()
    }

    /// The point at arc length `s`.
    pub fn at(&self, s: f64) -> Vec2 {
        if self.pts.len() < 2 {
            return self.pts.first().copied().unwrap_or(Vec2 { x: 0.0, y: 0.0 });
        }
        let s = s.clamp(0.0, self.len());
        let j = self.cum.partition_point(|&c| c <= s).clamp(1, self.pts.len() - 1);
        let (a, b) = (self.pts[j - 1], self.pts[j]);
        let l = self.cum[j] - self.cum[j - 1];
        if l <= 0.0 {
            return a;
        }
        a + (b - a) * ((s - self.cum[j - 1]) / l)
    }

    /// The traversal holding arc length `s`: the number of passes crossed before it.
    pub fn trav(&self, s: f64) -> usize {
        self.sig.partition_point(|&x| x < s)
    }

    /// Arc length of the event `(k, f)` (traversal `k`, fraction `f` of it).
    pub fn sigma(&self, k: usize, f: f64) -> f64 {
        let a = if k == 0 { 0.0 } else { self.sig.get(k - 1).copied().unwrap_or(self.len()) };
        let b = self.sig.get(k).copied().unwrap_or(self.len());
        a + (b - a) * f.clamp(0.0, 1.0)
    }
}

/// Length (nm) of wire `w`'s plan polyline ([`State::polyline`]) from event `i` to event `j` of
/// `ev` (`(traversal, fraction, label)`). O(passes).
pub fn stretch_len(st: &State, w: WireId, ev: &[(usize, f64, i32)], i: usize, j: usize) -> f64 {
    let pts = st.polyline(w);
    if pts.len() < 2 || i >= ev.len() || j >= ev.len() {
        return 0.0;
    }
    let at = |k: usize, s: f64| {
        let k = k.min(pts.len() - 2);
        pts[k] + (pts[k + 1] - pts[k]) * s
    };
    let ((ka, sa), (kb, sb)) = ((ev[i].0, ev[i].1), (ev[j].0, ev[j].1));
    if ka == kb {
        return (at(kb, sb) - at(ka, sa)).len();
    }
    let mut l = (pts[(ka + 1).min(pts.len() - 1)] - at(ka, sa)).len();
    for k in ka + 1..kb.min(pts.len() - 1) {
        l += (pts[k + 1] - pts[k]).len();
    }
    l + (at(kb, sb) - pts[kb.min(pts.len() - 1)]).len()
}

/// What the via relocation needs of the board beyond the state.
pub struct ViaCtx<'a> {
    pub st: &'a State<'a>,
    /// Board outline segments and the copper-to-edge clearance (nm).
    pub outline: &'a [(Vec2, Vec2)],
    pub edge_clear: f64,
    /// Via radius (nm) of a net's via ([`crate::vias::via_sizes`] first).
    pub via_r: &'a dyn Fn(i32) -> f64,
    /// Clearance (nm) of a net.
    pub clear: &'a dyn Fn(i32) -> f64,
}

/// One via of the plan as the relocation sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Site {
    pub wire: WireId,
    /// Index of the via in its wire's plan.
    pub i: usize,
    /// Traversal and point.
    pub k: usize,
    pub p: Vec2,
}

/// What [`relocate_vias`] did.
#[derive(Clone, Debug, Default)]
pub struct RelocReport {
    pub vias: usize,
    pub moved: usize,
    /// Sum of the gain in the smallest clearance gap (nm).
    pub gap_gain: f64,
    /// Per moved via: wire, index, from, to, smallest gap before / after (nm).
    pub moves: Vec<(WireId, usize, Vec2, Vec2, f64, f64)>,
}

/// The via intervals `(ka, sa, kb, sb)` of `events` (consecutive events whose labels differ), in
/// wire order, as [`super::draw::plan`] reads them.
pub fn intervals(events: &[(usize, f64, i32)]) -> Vec<(usize, f64, usize, f64)> {
    events.windows(2).filter(|x| x[0].2 != x[1].2).map(|x| (x[0].0, x[0].1, x[1].0, x[1].1)).collect()
}

struct Near {
    obs: Vec<usize>,
    wires: Vec<WireId>,
}

/// Smallest clearance gap (nm) and repulsion potential of a via of `net` (radius `rv`, clearance
/// `c`) at `p`: against the obstacles of other nets (every layer: a through via), the board edge,
/// the other nets' wires (their tracks) and the other vias in `sites` (other nets: clearance;
/// the same wire: no overlap). O(near items).
#[allow(clippy::too_many_arguments)]
fn gap_at(cx: &ViaCtx, tracks: &[Track], near: &Near, sites: &[Site], me: (WireId, usize), net: i32, rv: f64, c: f64, p: Vec2) -> (f64, f64) {
    let st = cx.st;
    let mut gmin = f64::INFINITY;
    let mut pot = 0.0;
    let mut add = |g: f64| {
        gmin = gmin.min(g);
        if g < POT_RANGE {
            pot += 1.0 / g.max(1e3) - 1.0 / POT_RANGE;
        }
    };
    for &o in &near.obs {
        let ob = &st.proj.obstacles[o];
        if ob.net == Some(net) {
            continue;
        }
        let req = rv + if ob.bare { ob.clear } else { c.max(ob.clear) };
        add(dist_poly(p, &ob.poly) - req);
    }
    for &(a, b) in cx.outline {
        add(seg_point(p, a, b).0 - rv - cx.edge_clear);
    }
    for &u in &near.wires {
        let wu = &st.wires[u];
        add(dist_polyline(p, &tracks[u].pts) - rv - 0.5 * wu.w - c.max(wu.s));
    }
    for s in sites {
        if (s.wire, s.i) == me {
            continue;
        }
        let nu = st.wires[s.wire].net;
        let ru = (cx.via_r)(nu);
        if nu != net {
            add((p - s.p).len() - rv - ru - c.max((cx.clear)(nu)));
        } else if s.wire == me.0 {
            add((p - s.p).len() - rv - ru);
        }
    }
    (gmin, pot)
}

/// The planned via sites: `(wire, via index)` -> `(traversal, point)`.
pub type Sites = BTreeMap<(WireId, usize), (usize, Vec2)>;

/// Relocate the vias of `plans` (wire -> plan, its `at` updated) by the force balance (module
/// doc). `events` are the layering's per-wire events. Returns the planned site per
/// `(wire, via index)` as `(traversal, point)` and the report. Two Gauss-Seidel sweeps (each via
/// sees the others' current sites). O(vias x SAMPLES x near items).
pub fn relocate_vias(cx: &ViaCtx, plans: &mut BTreeMap<WireId, super::draw::Plan>, events: &[Vec<(usize, f64, i32)>]) -> (Sites, RelocReport) {
    let st = cx.st;
    let mut rep = RelocReport::default();
    let alive: Vec<WireId> = (0..st.wires.len()).filter(|&w| st.wires[w].alive).collect();
    let mut tracks: Vec<Track> = vec![Track::default(); st.wires.len()];
    for &w in &alive {
        tracks[w] = Track::of(st, w);
    }
    // Every via: its interval on its track and its default site.
    struct V {
        w: WireId,
        i: usize,
        lo: f64,
        hi: f64,
        ka: usize,
        kb: usize,
        near: Near,
    }
    let mut vs: Vec<V> = Vec::new();
    let mut sites: Vec<Site> = Vec::new();
    for (&w, pl) in plans.iter() {
        if pl.vias.is_empty() || tracks[w].is_empty() {
            continue;
        }
        let ivs = intervals(events.get(w).map_or(&[][..], |e| &e[..]));
        if ivs.len() != pl.vias.len() {
            continue;
        }
        let tr = &tracks[w];
        for (i, &(ka, sa, kb, sb)) in ivs.iter().enumerate() {
            let (lo, hi) = (tr.sigma(ka, sa), tr.sigma(kb, sb));
            let (lo, hi) = (lo.min(hi), lo.max(hi));
            // The drawing's own first choice: the middle of the planned traversal.
            let k = pl.at.get(i).copied().unwrap_or((ka + kb).div_ceil(2));
            let (a, b) = (tr.sigma(k, 0.0).max(lo), tr.sigma(k, 1.0).min(hi));
            let p = tr.at(if a <= b { 0.5 * (a + b) } else { 0.5 * (lo + hi) });
            // Copper near the interval (bbox + range).
            let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
            for s in 0..=8 {
                let q = tr.at(lo + (hi - lo) * s as f64 / 8.0);
                x0 = x0.min(q.x);
                y0 = y0.min(q.y);
                x1 = x1.max(q.x);
                y1 = y1.max(q.y);
            }
            let m = POT_RANGE;
            let inb = |q: Vec2| q.x >= x0 - m && q.x <= x1 + m && q.y >= y0 - m && q.y <= y1 + m;
            let obs: Vec<usize> = (0..st.proj.obstacles.len()).filter(|&o| st.proj.obstacles[o].poly.iter().any(|&q| inb(q)) || inb(st.proj.obstacles[o].center)).collect();
            let net = st.wires[w].net;
            let wires: Vec<WireId> = alive.iter().copied().filter(|&u| st.wires[u].net != net && (tracks[u].pts.iter().any(|&q| inb(q)) || tracks[u].pts.windows(2).any(|x| inb((x[0] + x[1]) * 0.5)))).collect();
            vs.push(V { w, i, lo, hi, ka, kb, near: Near { obs, wires } });
            sites.push(Site { wire: w, i, k, p });
        }
    }
    rep.vias = vs.len();
    let start: Vec<Site> = sites.clone();
    for _sweep in 0..2 {
        for (vi, v) in vs.iter().enumerate() {
            let net = st.wires[v.w].net;
            let (rv, c) = ((cx.via_r)(net), (cx.clear)(net));
            let tr = &tracks[v.w];
            let turn_k: Vec<usize> = st.wires[v.w].turns.iter().map(|t| t.0).collect();
            let ok_k = |k: usize| k >= v.ka && k <= v.kb && !turn_k.contains(&k) && !st.proj.tri_out[st.wires[v.w].tris[k.min(st.wires[v.w].tris.len() - 1)]];
            let eval = |s: f64, sites: &[Site]| -> Option<(f64, f64, usize, Vec2)> {
                let k = tr.trav(s);
                if !ok_k(k) {
                    return None;
                }
                let p = tr.at(s);
                let (g, pot) = gap_at(cx, &tracks, &v.near, sites, (v.w, v.i), net, rv, c, p);
                Some((g, pot, k, p))
            };
            let better = |a: &(f64, f64, usize, Vec2), b: &(f64, f64, usize, Vec2)| a.0 > b.0 + 1.0 || (a.0 > b.0 - 1.0 && a.1 < b.1 - 1e-12);
            let cur = sites[vi];
            let (gc, pc) = gap_at(cx, &tracks, &v.near, &sites, (v.w, v.i), net, rv, c, cur.p);
            let mut best = (gc, pc, cur.k, cur.p);
            let span = v.hi - v.lo;
            let mut best_s = None;
            for j in 0..=SAMPLES {
                let s = v.lo + span * j as f64 / SAMPLES as f64;
                if let Some(e) = eval(s, &sites) {
                    if better(&e, &best) {
                        best = e;
                        best_s = Some(s);
                    }
                }
            }
            // Refine around the best sample (golden-section style shrinking steps).
            if let Some(mut s0) = best_s {
                let mut h = span / SAMPLES as f64;
                for _ in 0..24 {
                    h *= 0.5;
                    for s in [s0 - h, s0 + h] {
                        if s < v.lo || s > v.hi {
                            continue;
                        }
                        if let Some(e) = eval(s, &sites) {
                            if better(&e, &best) {
                                best = e;
                                s0 = s;
                            }
                        }
                    }
                }
            }
            sites[vi] = Site { wire: v.w, i: v.i, k: best.2, p: best.3 };
        }
    }
    let mut out = BTreeMap::new();
    for (vi, v) in vs.iter().enumerate() {
        let s = sites[vi];
        let net = st.wires[v.w].net;
        let (rv, c) = ((cx.via_r)(net), (cx.clear)(net));
        let (g0, _) = gap_at(cx, &tracks, &v.near, &start, (v.w, v.i), net, rv, c, start[vi].p);
        let (g1, _) = gap_at(cx, &tracks, &v.near, &sites, (v.w, v.i), net, rv, c, s.p);
        if (s.p - start[vi].p).len() > 1.0 {
            rep.moved += 1;
            rep.gap_gain += g1 - g0;
            rep.moves.push((v.w, v.i, start[vi].p, s.p, g0, g1));
        }
        if let Some(pl) = plans.get_mut(&v.w) {
            if let Some(a) = pl.at.get_mut(v.i) {
                *a = s.k;
            }
        }
        out.insert((v.w, v.i), (s.k, s.p));
    }
    (out, rep)
}

/// A junction the plan moved ([`relocate_junctions`]).
#[derive(Clone, Debug, PartialEq)]
pub struct JMove {
    /// `"free"`: a T-point on a trunk moved towards the Fermat point of its arms; `"pad_out"`: a
    /// junction at a pad (two wires ending there) pulled out of the pad; `"into_pad"`: a T-point
    /// that goes into its trunk's end pad (an angle >= 120 degrees there).
    pub kind: &'static str,
    /// The wire drawn through the new point (split there) and its traversal; `None` for
    /// `into_pad` (the trunk is drawn as before).
    pub trunk: Option<(WireId, usize)>,
    /// The wire that ends at the new point, and which end.
    pub branch: (WireId, usize),
    /// The vertex of the old junction (a T-point's vertex or the pad centre).
    pub vertex: usize,
    pub from: Vec2,
    pub at: Vec2,
    /// Length saved by the arms (nm).
    pub gain: f64,
    /// Fraction of the way to the Fermat point taken (1 = all; less where the arms would come too
    /// near other copper).
    pub t: f64,
}

/// What the junction relocation needs of the board.
pub struct JCtx<'a> {
    pub st: &'a State<'a>,
    pub clear: &'a dyn Fn(i32) -> f64,
    /// Layer of each run of each wire's plan (wire -> layers in wire order) and its via
    /// traversals.
    pub plans: &'a BTreeMap<WireId, super::draw::Plan>,
    /// The drawing is octilinear: the junction goes to the point of least octilinear length
    /// ([`fermat_octi`]) instead of the Euclidean Fermat point (its legs are drawn at 0 / 45 / 90
    /// degrees, where the 120 degree point is not the shortest).
    pub octi: bool,
}

/// Clearance shortfall (nm, summed) of the arms `(from, to, wire)` against the obstacles of other
/// nets and the other nets' tracks; 0 = every arm keeps its clearance. O(arms x near copper).
fn arms_short(cx: &JCtx, tracks: &[Track], obs: &[usize], wires: &[WireId], arms: &[(Vec2, Vec2, WireId)]) -> f64 {
    let st = cx.st;
    let mut short = 0.0;
    for &(a, b, w) in arms {
        let wi = &st.wires[w];
        let c = (cx.clear)(wi.net);
        for &o in obs {
            let ob = &st.proj.obstacles[o];
            if ob.net == Some(wi.net) {
                continue;
            }
            let req = 0.5 * wi.w + if ob.bare { ob.clear } else { c.max(ob.clear) };
            let n = ob.poly.len();
            let inside = super::state::point_in_poly(a, &ob.poly) || super::state::point_in_poly(b, &ob.poly);
            let d = if inside { 0.0 } else { (0..n).map(|i| seg_seg(a, b, ob.poly[i], ob.poly[(i + 1) % n])).fold(f64::INFINITY, f64::min) };
            short += (req - d).max(0.0);
        }
        for &u in wires {
            let wu = &st.wires[u];
            if wu.net == wi.net {
                continue;
            }
            let req = 0.5 * wi.w + 0.5 * wu.w + c.max(wu.s);
            let d = tracks[u].pts.windows(2).map(|x| seg_seg(a, b, x[0], x[1])).fold(f64::INFINITY, f64::min);
            short += (req - d).max(0.0);
        }
    }
    short
}

/// The run layer of `plan` at traversal `k` (`None` inside a via interval).
fn layer_at(plan: &super::draw::Plan, k: usize) -> Option<usize> {
    let mut l = plan.layers[0];
    for (i, &(ka, kb)) in plan.vias.iter().enumerate() {
        if k > kb.max(ka) {
            l = plan.layers[i + 1];
        } else if k >= ka {
            return None;
        } else {
            break;
        }
    }
    Some(l)
}

/// Move the junctions of the net trees (module doc; `[search] branch_relocate`). Per T-point of
/// a trunk (`tree::junctions`): the Fermat point of the three arms' first corners (taut paths of
/// the plan); per pad where exactly two wires of its net end: the Fermat point of the pad centre
/// and the two first corners, when the angle at the pad is below 120 degrees. The point is drawn
/// back along the way from the old junction until the arms keep their clearances (homotopic
/// move, Dayan & Dai 1991 p. 5). Only where the arms at the junction share one layer (no via at
/// the junction). Returns the moves with a length gain above 1 um. O(junctions x near copper).
pub fn relocate_junctions(cx: &JCtx) -> Vec<JMove> {
    let st = cx.st;
    let pr = st.proj;
    let mut out = Vec::new();
    let alive: Vec<WireId> = (0..st.wires.len()).filter(|&w| st.wires[w].alive).collect();
    let mut tracks: Vec<Track> = vec![Track::default(); st.wires.len()];
    for &w in &alive {
        tracks[w] = Track::of(st, w);
    }
    let near = |pts: &[Vec2], net: i32| -> (Vec<usize>, Vec<WireId>) {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for q in pts {
            x0 = x0.min(q.x);
            y0 = y0.min(q.y);
            x1 = x1.max(q.x);
            y1 = y1.max(q.y);
        }
        let m = 2.0e6;
        let inb = |q: Vec2| q.x >= x0 - m && q.x <= x1 + m && q.y >= y0 - m && q.y <= y1 + m;
        let ovl = |ps: &[Vec2]| {
            let (mut a0, mut b0, mut a1, mut b1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
            for q in ps {
                a0 = a0.min(q.x);
                b0 = b0.min(q.y);
                a1 = a1.max(q.x);
                b1 = b1.max(q.y);
            }
            a0 <= x1 + m && a1 >= x0 - m && b0 <= y1 + m && b1 >= y0 - m
        };
        let obs = (0..pr.obstacles.len()).filter(|&o| ovl(&pr.obstacles[o].poly) || inb(pr.obstacles[o].center)).collect();
        let ws = alive.iter().copied().filter(|&u| st.wires[u].net != net && ovl(&tracks[u].pts)).collect();
        (obs, ws)
    };
    let tri_has = |t: usize, p: Vec2| {
        let [a, b, c] = pr.tris[t].map(|v| pr.pos[v]);
        let s1 = (b - a).cross(p - a);
        let s2 = (c - b).cross(p - b);
        let s3 = (a - c).cross(p - c);
        (s1 >= 0.0 && s2 >= 0.0 && s3 >= 0.0) || (s1 <= 0.0 && s2 <= 0.0 && s3 <= 0.0)
    };
    // The traversal of `w` near `k` whose triangle holds `p` (else `k`).
    let trav_of = |w: WireId, k: usize, p: Vec2| -> usize {
        let tris = &st.wires[w].tris;
        let lo = k.saturating_sub(6);
        let hi = (k + 6).min(tris.len() - 1);
        (lo..=hi).filter(|&j| tri_has(tris[j], p)).min_by_key(|&j| j.abs_diff(k)).unwrap_or(k)
    };
    let clamp = |j0: Vec2, f: Vec2, arms_of: &dyn Fn(Vec2) -> Vec<(Vec2, Vec2, WireId)>, obs: &[usize], ws: &[WireId]| -> (f64, Vec2) {
        let s0 = arms_short(cx, &tracks, obs, ws, &arms_of(j0));
        let ok = |t: f64| arms_short(cx, &tracks, obs, ws, &arms_of(j0 + (f - j0) * t)) <= s0 + 1.0;
        if ok(1.0) {
            return (1.0, f);
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..24 {
            let m = 0.5 * (lo + hi);
            if ok(m) {
                lo = m;
            } else {
                hi = m;
            }
        }
        (lo, j0 + (f - j0) * lo)
    };
    // T-points on trunks.
    for (bw, end, tw, k, v) in super::tree::junctions(st) {
        let (Some(pt), Some(pb)) = (cx.plans.get(&tw), cx.plans.get(&bw)) else { continue };
        let j0 = pr.pos[v];
        let tp = &tracks[tw].pts;
        let Some(ix) = (1..tp.len().saturating_sub(1)).min_by(|&a, &b| (tp[a] - j0).len().total_cmp(&(tp[b] - j0).len())) else { continue };
        if (tp[ix] - j0).len() > 1e3 {
            continue;
        }
        let bp = &tracks[bw].pts;
        if bp.len() < 2 {
            continue;
        }
        let arm = if end == 0 { bp[1] } else { bp[bp.len() - 2] };
        let (prev, next) = (tp[ix - 1], tp[ix + 1]);
        // The tee keeps one layer (no via at the junction).
        let lt = layer_at(pt, k);
        let lb = if end == 0 { pb.layers.first().copied() } else { pb.layers.last().copied() };
        if lt.is_none() || lt != lb {
            continue;
        }
        let f = if cx.octi { fermat_octi(&[prev, next, arm]) } else { fermat(&[prev, next, arm]) };
        let (twi, bwi) = (&st.wires[tw], &st.wires[bw]);
        let pad_end = |q: Vec2| -> bool {
            [(twi.a, twi.pa), (twi.b, twi.pb)].iter().any(|&(vv, o)| o != super::state::NO_OBS && (pr.pos[vv] - q).len() < 1.0)
        };
        let d = |a: Vec2| if cx.octi { len8(a) } else { a.len() };
        let len_of = |p: Vec2| d(p - prev) + d(p - next) + d(p - arm);
        if (f - prev).len() < 1.0 || (f - next).len() < 1.0 {
            // An angle >= 120 degrees at a trunk corner: when the corner is the trunk's pad, the
            // junction goes into that pad (the branch starts there).
            let q = if (f - prev).len() < 1.0 { prev } else { next };
            if pad_end(q) {
                let (obs, ws) = near(&[q, arm, j0], bwi.net);
                let arms_q = |p: Vec2| vec![(p, arm, bw)];
                if arms_short(cx, &tracks, &obs, &ws, &arms_q(q)) <= arms_short(cx, &tracks, &obs, &ws, &arms_q(j0)) + 1.0 {
                    let gain = len_of(j0) - len_of(q);
                    out.push(JMove { kind: "into_pad", trunk: None, branch: (bw, end), vertex: v, from: j0, at: q, gain: gain.max(0.0), t: 1.0 });
                }
            }
            continue;
        }
        if (f - arm).len() < 1.0 {
            // The branch's own corner takes the junction (its far pad or a bend): drawn as before.
            continue;
        }
        let (obs, ws) = near(&[prev, next, arm, j0, f], twi.net);
        let arms_of = |p: Vec2| vec![(prev, p, tw), (p, next, tw), (p, arm, bw)];
        let (t, at) = clamp(j0, f, &arms_of, &obs, &ws);
        let gain = len_of(j0) - len_of(at);
        if gain > 1e3 {
            out.push(JMove { kind: "free", trunk: Some((tw, trav_of(tw, k, at))), branch: (bw, end), vertex: v, from: j0, at, gain, t });
        }
    }
    // Junctions at pads: two wires of the net end at one pad centre.
    let mut at_pad: BTreeMap<usize, Vec<(WireId, usize)>> = BTreeMap::new();
    for &w in &alive {
        let wi = &st.wires[w];
        for (end, (vv, o)) in [(wi.a, wi.pa), (wi.b, wi.pb)].into_iter().enumerate() {
            if o != super::state::NO_OBS {
                at_pad.entry(vv).or_default().push((w, end));
            }
        }
    }
    let turned: std::collections::BTreeSet<usize> = alive.iter().flat_map(|&w| st.wires[w].turns.iter().map(|t| t.1)).collect();
    for (&v, ends) in &at_pad {
        if ends.len() != 2 || turned.contains(&v) {
            continue;
        }
        let ((w1, e1), (w2, e2)) = (ends[0], ends[1]);
        let (Some(p1), Some(p2)) = (cx.plans.get(&w1), cx.plans.get(&w2)) else { continue };
        let l1 = if e1 == 0 { p1.layers.first() } else { p1.layers.last() };
        let l2 = if e2 == 0 { p2.layers.first() } else { p2.layers.last() };
        if l1 != l2 {
            continue;
        }
        let q = pr.pos[v];
        let (t1, t2) = (&tracks[w1].pts, &tracks[w2].pts);
        if t1.len() < 2 || t2.len() < 2 {
            continue;
        }
        let c1 = if e1 == 0 { t1[1] } else { t1[t1.len() - 2] };
        let c2 = if e2 == 0 { t2[1] } else { t2[t2.len() - 2] };
        let f = if cx.octi { fermat_octi(&[q, c1, c2]) } else { fermat(&[q, c1, c2]) };
        if (f - q).len() < 1.0 || (f - c1).len() < 1.0 || (f - c2).len() < 1.0 {
            continue;
        }
        let net = st.wires[w1].net;
        let (obs, ws) = near(&[q, c1, c2, f], net);
        let arms_of = |p: Vec2| vec![(c1, p, w1), (p, q, w1), (p, c2, w2)];
        let (t, at) = clamp(q, f, &arms_of, &obs, &ws);
        let d = |a: Vec2| if cx.octi { len8(a) } else { a.len() };
        let len_of = |p: Vec2| d(p - q) + d(p - c1) + d(p - c2);
        let gain = len_of(q) - len_of(at);
        if gain <= 1e3 {
            continue;
        }
        // The first wire is drawn through the point (split in the traversal holding it, inside
        // its end run: no via between the point and the pad).
        let n1 = st.wires[w1].passes.len();
        let k_end = if e1 == 0 { 0 } else { n1 };
        let k1 = trav_of(w1, k_end, at);
        let vk: Vec<usize> = p1.vias.iter().flat_map(|&(a, b)| [a, b]).collect();
        let inside_end_run = if e1 == 0 { vk.iter().all(|&x| x > k1) } else { vk.iter().all(|&x| x < k1) };
        let k2 = trav_of(w2, if e2 == 0 { 0 } else { st.wires[w2].passes.len() }, at);
        let vk2: Vec<usize> = p2.vias.iter().flat_map(|&(a, b)| [a, b]).collect();
        let inside2 = if e2 == 0 { vk2.iter().all(|&x| x > k2) } else { vk2.iter().all(|&x| x < k2) };
        if !inside_end_run || !inside2 {
            continue;
        }
        out.push(JMove { kind: "pad_out", trunk: Some((w1, k1)), branch: (w2, e2), vertex: v, from: q, at, gain, t });
    }
    out
}

/// The Fermat (Weber) point of `pts` with unit weights by the force iteration of Dayan & Dai 1991
/// pp. 2-3 (each arm pulls with a unit force; the point moves along the combined force by a step
/// that shrinks), starting at the centroid. When an angle of the triangle is >= 120 degrees the
/// point is that vertex (EDN 2013). O(iterations x |pts|).
pub fn fermat(pts: &[Vec2]) -> Vec2 {
    if pts.len() == 3 {
        for i in 0..3 {
            let (a, o, b) = (pts[(i + 1) % 3], pts[i], pts[(i + 2) % 3]);
            if angle_deg(a, o, b) >= 120.0 {
                return o;
            }
        }
    }
    let n = pts.len().max(1) as f64;
    let mut s = pts.iter().fold(Vec2 { x: 0.0, y: 0.0 }, |a, &p| a + p) * (1.0 / n);
    let scale = pts.iter().map(|&p| (p - s).len()).fold(0.0, f64::max).max(1.0);
    let mut step = 0.25 * scale;
    for _ in 0..400 {
        let mut f = Vec2 { x: 0.0, y: 0.0 };
        for &p in pts {
            if let Some(u) = (p - s).unit() {
                f = f + u;
            }
        }
        let m = f.len();
        if m < 1e-9 || step < 1.0 {
            break;
        }
        let cand = s + f * (step / m);
        let len = |q: Vec2| pts.iter().map(|&p| (p - q).len()).sum::<f64>();
        if len(cand) < len(s) {
            s = cand;
        } else {
            step *= 0.5;
        }
    }
    s
}

/// Octilinear length of `d` (0 / 45 / 90 degree legs): `max(|x|, |y|) + (sqrt 2 - 1) min(|x|, |y|)`.
pub fn len8(d: Vec2) -> f64 {
    let (x, y) = (d.x.abs(), d.y.abs());
    x.max(y) + (std::f64::consts::SQRT_2 - 1.0) * x.min(y)
}

/// The point of least octilinear length to `pts` (the junction of an octilinear tree; pattern
/// `branch_point_relocate`, octilinear note: x = 8.0): the octilinear sum is convex, so a pattern
/// search over the eight directions with shrinking steps from the Euclidean Fermat point reaches
/// its minimum; ties (flat regions) go to the least Euclidean length. O(iterations x |pts|).
pub fn fermat_octi(pts: &[Vec2]) -> Vec2 {
    let f = |q: Vec2| pts.iter().map(|&p| len8(p - q)).sum::<f64>() + 1e-4 * pts.iter().map(|&p| (p - q).len()).sum::<f64>();
    let mut s = fermat(pts);
    let mut best = f(s);
    let mut step = pts.iter().map(|&p| (p - s).len()).fold(0.0, f64::max).max(1.0) * 0.5;
    let dirs: Vec<Vec2> = (0..8).map(|i| Vec2 { x: (i as f64 * std::f64::consts::FRAC_PI_4).cos(), y: (i as f64 * std::f64::consts::FRAC_PI_4).sin() }).collect();
    while step > 0.5 {
        let mut moved = false;
        for &d in &dirs {
            let c = s + d * step;
            let v = f(c);
            if v < best - 1e-9 {
                best = v;
                s = c;
                moved = true;
            }
        }
        if !moved {
            step *= 0.5;
        }
    }
    // Snap onto a terminal it reached (within the last step).
    pts.iter().copied().find(|&p| (p - s).len() < 2.0).unwrap_or(s)
}

/// The angle at `o` between `o -> a` and `o -> b`, degrees.
pub fn angle_deg(a: Vec2, o: Vec2, b: Vec2) -> f64 {
    let (u, v) = (a - o, b - o);
    let d = (u.len() * v.len()).max(1e-12);
    (u.dot(v) / d).clamp(-1.0, 1.0).acos().to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x: x * 1e6, y: y * 1e6 }
    }

    #[test]
    fn fermat_point_of_the_branch_pattern_and_the_wide_angle_vertex() {
        // scratch/via-patterns/branch_point_relocate (KiCad y down; the angles do not care).
        let f = fermat(&[v(3.0, 2.0), v(3.0, 12.0), v(17.0, 7.0)]);
        assert!((f - v(5.8868, 7.0)).len() < 0.002e6, "{:?}", f);
        let p = [v(3.0, 2.0), v(3.0, 12.0), v(17.0, 7.0)];
        for i in 0..3 {
            let a = angle_deg(p[(i + 1) % 3], f, p[(i + 2) % 3]);
            assert!((a - 120.0).abs() < 0.1, "{a}");
        }
        // branch_point_into_pad: the angle at M is 133.6 degrees, so M is the point.
        let m = v(10.0, 6.0);
        assert_eq!(fermat(&[v(3.0, 3.0), m, v(17.0, 3.0)]), m);
        // The octilinear junction of branch_point_relocate (expected.json octilinear note): x = 8.
        let o = fermat_octi(&[v(3.0, 2.0), v(3.0, 12.0), v(17.0, 7.0)]);
        assert!((o - v(8.0, 7.0)).len() < 0.01e6, "{:?}", o);
        let tot: f64 = [v(3.0, 2.0), v(3.0, 12.0), v(17.0, 7.0)].iter().map(|&p| len8(p - o)).sum();
        assert!((tot - 23.1421e6).abs() < 0.01e6, "{tot}");
    }

    #[test]
    fn polyline_and_polygon_distances() {
        let sq = [v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)];
        assert_eq!(dist_poly(v(0.5, 0.5), &sq), 0.0);
        assert!((dist_poly(v(2.0, 0.5), &sq) - 1e6).abs() < 1.0);
        assert!((dist_polyline(v(0.5, 1.0), &[v(0.0, 0.0), v(1.0, 0.0)]) - 1e6).abs() < 1.0);
        assert_eq!(seg_seg(v(0.0, 0.0), v(2.0, 2.0), v(0.0, 2.0), v(2.0, 0.0)), 0.0);
        assert!((seg_seg(v(0.0, 0.0), v(1.0, 0.0), v(0.0, 1.0), v(1.0, 1.0)) - 1e6).abs() < 1.0);
    }

    #[test]
    fn intervals_follow_the_label_changes() {
        let ev = [(1, 0.5, 0), (3, 0.2, 1), (5, 0.1, 1), (7, 0.9, 0)];
        assert_eq!(intervals(&ev), vec![(1, 0.5, 3, 0.2), (5, 0.1, 7, 0.9)]);
    }
}
