//! Octilinear drawing (plan P6 `topo/octi`, M-45): a run the drawing has made legal as a taut
//! any-angle polyline is redrawn with straight and 45 degree segments only, where that fits.
//!
//! Method (Bast, Brosi & Storandt 2020, "Metro Maps on Octilinear Grid Graphs", EuroVis, sections
//! 3-4: a path in a graph whose nodes have 8 ports, bends priced per angle, obstacles excluded,
//! solved greedily per line in line order): the wire's own corridor is the sleeve of its
//! topology (the triangle strip with its pockets, [`super::exact`]); the nodes are the taut path's
//! own vertices (the TopoR minimum-length wire, Popov & Popov 2012 pp. 117-122) plus the vertices
//! of the *octagons* around every obstacle corner near it (the octagon clearance metric: the
//! circumscribed octagon of the clearance circle, edges at 0/45/90 degrees, so a wire that hugs
//! an obstacle or a wire drawn before it runs parallel to it at exactly the clearance, which keeps
//! a bundle's pitch through its bends). Between two nodes the connectors are octilinear: straight
//! when the nodes are aligned, else the two elbows (one bend) and the two Z shapes (two bends,
//! middle leg centred; the coordinates of the bends follow from the nodes, the "bend LP" of the
//! plan reduced to the elbow / Z choice). A dynamic programme over the nodes in path order with
//! the arrival direction as state (8 ports) finds the cheapest chain: length, plus a price per
//! bend growing with its angle (90 degrees costs twice 45), plus the jog penalty (a bend after a
//! leg shorter than `L_min = min(L_pref, 2 (w + s))` pays more the shorter the leg), plus the
//! `acute_bend` factor for a turn sharper than 90 degrees, plus a pad exit / entry that does not
//! follow the pad's long axis (graded by its angle; a leg along the axis to just past the pad's
//! tip is a connector of its own). Every connector leg must stay inside the sleeve (no wall
//! crossed: the topology is kept) and keep every clearance to the board and to the copper drawn
//! so far. The original taut segments are always available, priced by the `any_angle` factor
//! (15 mm per stretch plus 2 per mm by default): where 45 degrees does not fit, the stretch stays
//! any-angle and is reported (never an open wire or a new violation because of the style).
//!
//! The caller checks the result with the full checker and keeps the taut run when it is rejected.
//!
//! Complexity: nodes N (taut vertices plus octagon vertices within two pitches of the path,
//! capped), connector window J: O(N x J x 8 x shapes) prices; each leg's clearance test is
//! O(local objects) and cached. Deterministic (index order, `total_cmp`).

use super::draw::{bbox_of, Core, Drawer};
use super::exact::{Neck, Strip, Wp};
use super::state::{WireId, NO_OBS};
use crate::geom::prim::Aabb;
use crate::geom::Vec2;
use std::collections::HashMap;
use std::f64::consts::FRAC_PI_4;

const MM: f64 = 1e6;
/// Most nodes of one run (taut vertices always kept); above it the farthest octagon nodes go.
const MAX_NODES: usize = 260;
/// Connector window: a node connects to the next `WINDOW` nodes in path order, and to every node
/// up to `REACH_MM` further along the taut path (at most `WINDOW_MAX` nodes).
const WINDOW: usize = 36;
const WINDOW_MAX: usize = 160;
const REACH_MM: f64 = 12.0;
/// Octagon vertex radius over the clearance radius: 1 / cos(22.5 degrees), plus a hair.
const OCT_R: f64 = 1.082_392_200_292_394 * (1.0 + 2e-6);

/// Prices and switches of the octilinear drawing (from `topor.toml [drawing]` and `[factors]`).
#[derive(Clone, Copy, Debug)]
pub struct OctiCfg {
    /// `[drawing] style = "octilinear"`.
    pub on: bool,
    /// Price of a 45 degree bend (nm of length; a turn of `a` degrees pays `a / 45` of it).
    pub bend_nm: f64,
    /// `L_pref` of the jog penalty (nm).
    pub spacing_nm: f64,
    /// `any_angle` factor weight (nm per stretch; plus 2/15 of it per mm).
    pub any_nm: f64,
    pub any_forbidden: bool,
    /// `acute_bend` factor weight (nm per bend, graded by the angle).
    pub acute_nm: f64,
    pub acute_forbidden: bool,
    /// Bundles (`[drawing] bundle_pull`): share of a leg's length taken off its price where it
    /// runs parallel to a drawn wire of another net at that wire's clearance (it follows its
    /// neighbour, so a bundle keeps its pitch through its bends).
    pub pull: f64,
}

impl OctiCfg {
    /// From the engine options: `[drawing]` and the `any_angle` / `acute_bend` weights of
    /// `[factors]` (unfluctuated: the drawing's prices stay put across cycles).
    pub fn from_opts(o: &crate::model::Options) -> OctiCfg {
        let reg = super::factors::Registry::from_config(super::factors::standard_factors(), &o.factors).0;
        OctiCfg {
            on: o.draw_style == "octilinear",
            bend_nm: o.bend_mm.max(0.0) * MM,
            spacing_nm: o.bend_spacing_mm.max(0.0) * MM,
            any_nm: reg.weight("any_angle").unwrap_or(15.0) * MM,
            any_forbidden: o.any_angle == "forbidden",
            acute_nm: reg.weight("acute_bend").unwrap_or(50.0) * MM,
            acute_forbidden: o.acute == "forbidden",
            pull: o.bundle_pull.clamp(0.0, 0.9),
        }
    }
}

impl Default for OctiCfg {
    fn default() -> Self {
        OctiCfg { on: false, bend_nm: 0.25 * MM, spacing_nm: 0.5 * MM, any_nm: 15.0 * MM, any_forbidden: false, acute_nm: 50.0 * MM, acute_forbidden: false, pull: 0.3 }
    }
}

/// An octilinear run: its points, and what is left of the style breaks.
#[derive(Clone, Debug, Default)]
pub struct OctiOut {
    pub pts: Vec<Vec2>,
}

/// Unit vector of octilinear direction `k` (k x 45 degrees, math frame).
pub fn dir(k: usize) -> Vec2 {
    let a = (k % 8) as f64 * FRAC_PI_4;
    Vec2 { x: a.cos(), y: a.sin() }
}

/// The octilinear direction of `d`, if it is one (perpendicular deviation under 1 nm, or 1e-6 of
/// its length): `None` for any other direction or a vector shorter than 1 nm.
pub fn octi_dir(d: Vec2) -> Option<usize> {
    let l = d.len();
    if l < 1.0 {
        return None;
    }
    let k = ((d.y.atan2(d.x) / FRAC_PI_4).round() as i64).rem_euclid(8) as usize;
    (d.cross(dir(k)).abs() <= 1.0 + 1e-6 * l).then_some(k)
}

/// Turn angle (degrees, 0 .. 180) from direction `a` to direction `b` (unit or not).
pub fn turn_deg(a: Vec2, b: Vec2) -> f64 {
    let (la, lb) = (a.len(), b.len());
    if la < 1e-12 || lb < 1e-12 {
        return 0.0;
    }
    (a.dot(b) / (la * lb)).clamp(-1.0, 1.0).acos().to_degrees()
}

/// `any_angle` amount of a stretch of `len_nm`: 1 per stretch plus 2/15 per mm (the plan's
/// 15 mm per stretch plus 2x its length at the default weight 15).
pub fn any_angle_amount(len_nm: f64) -> f64 {
    1.0 + len_nm / MM * (2.0 / 15.0)
}

/// `acute_bend` amount of a turn of `turn` degrees (> 90): 1, rising to 2 as the interior angle
/// closes from 90 to 0 degrees.
pub fn acute_amount(turn: f64) -> f64 {
    1.0 + ((turn - 90.0) / 90.0).clamp(0.0, 1.0)
}

/// The style breaks of a drawn polyline: `(stretches not at 0/45/90 degrees as (middle, angle
/// deg, length nm), acute bends as (vertex, interior angle deg))`. Consecutive any-angle segments
/// form one stretch. O(points).
#[allow(clippy::type_complexity)]
pub fn breaks(pts: &[Vec2]) -> (Vec<(Vec2, f64, f64)>, Vec<(Vec2, f64)>) {
    let mut any: Vec<(Vec2, f64, f64)> = Vec::new();
    let mut acute = Vec::new();
    let mut open: Option<(Vec2, f64, f64, f64)> = None; // (start, len, angle of the longest piece, its len)
    let segs: Vec<(Vec2, Vec2)> = pts.windows(2).filter(|p| (p[1] - p[0]).len2() > 1.0).map(|p| (p[0], p[1])).collect();
    for (i, &(a, b)) in segs.iter().enumerate() {
        let d = b - a;
        if octi_dir(d).is_none() {
            let l = d.len();
            let ang = d.y.atan2(d.x).to_degrees().rem_euclid(180.0);
            open = Some(match open {
                Some((s, tl, an, al)) => (s, tl + l, if l > al { ang } else { an }, al.max(l)),
                None => (a, l, ang, l),
            });
        } else if let Some((s, tl, an, _)) = open.take() {
            any.push((s + (a - s) * 0.5, an, tl));
        }
        if i + 1 < segs.len() {
            let t = turn_deg(d, segs[i + 1].1 - segs[i + 1].0);
            if t > 90.5 {
                acute.push((b, 180.0 - t));
            }
        }
    }
    if let (Some((s, tl, an, _)), Some(&(_, b))) = (open, segs.last()) {
        any.push((s + (b - s) * 0.5, an, tl));
    }
    (any, acute)
}

/// A connector leg: its end point and its octilinear direction (`None`: the taut segment, any
/// angle).
type Leg = (Vec2, Option<usize>);

/// Where the run ends at a pad: the pad's long axis (unit, pointing out of the pad towards the
/// run) and the distance from the pad centre to just past its tip (half its length along the
/// axis plus the clearance and half the width).
#[derive(Clone, Copy, Debug)]
pub struct Exit {
    pub axis: Vec2,
    pub reach: f64,
}

impl Drawer<'_> {
    /// The pad end of run `r` of wire `w` (`end` 0: its start, 1: its end) as an [`Exit`] when the
    /// pad is elongated (long side over 1.2 x the short side), `toward` = the next taut point.
    pub(super) fn pad_exit(&self, w: WireId, end: usize, toward: Vec2) -> Option<Exit> {
        let wi = &self.st.wires[w];
        let ob = if end == 0 { wi.pa } else { wi.pb };
        if ob == NO_OBS || self.end_at.contains_key(&(w, end)) {
            return None;
        }
        let pr = &self.b.proj;
        let o = &pr.obstacles[ob];
        let c = pr.pos[if end == 0 { wi.a } else { wi.b }];
        if o.poly.len() < 3 {
            return None;
        }
        // Principal axis of the outline (second moments of its vertices about the centre).
        let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
        for &p in &o.poly {
            let d = p - o.center;
            sxx += d.x * d.x;
            syy += d.y * d.y;
            sxy += d.x * d.y;
        }
        let th = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        let u = Vec2 { x: th.cos(), y: th.sin() };
        let ext = |v: Vec2| o.poly.iter().map(|&p| (p - o.center).dot(v)).fold(f64::MIN, f64::max);
        let (lu, lv) = (ext(u) + ext(-u), ext(u.perp()) + ext(-u.perp()));
        if lu < 1.2 * lv {
            return None;
        }
        let u = if (toward - c).dot(u) >= 0.0 { u } else { -u };
        let reach = ext(u) + (c - o.center).dot(-u) + wi.s + 0.5 * wi.w;
        Some(Exit { axis: u, reach })
    }

    /// The run `pts` (passes `k0..k1` of `w` on `layer`, legal as drawn) redrawn octilinear in its
    /// sleeve (see the module doc). `None` when nothing changes or the run is too short.
    #[allow(clippy::too_many_arguments)]
    /// `relax` (nm, 0 = the rule): the clearance to foreign objects is kept only up to this much
    /// less (a run drawn by force, never drop: no deeper than its own violations).
    pub(super) fn octilinear(&self, w: WireId, k0: usize, k1: usize, layer: usize, pts: &[Vec2], neck: Option<Neck>, exits: [Option<Exit>; 2], relax: f64) -> Option<OctiOut> {
        let cfg = self.octi;
        let mut taut: Vec<Vec2> = pts.to_vec();
        taut.dedup_by(|a, b| (*a - *b).len2() < 1.0);
        if taut.len() < 2 {
            return None;
        }
        if taut.windows(2).all(|p| octi_dir(p[1] - p[0]).is_some()) && exits.iter().all(|e| e.is_none()) {
            return None;
        }
        let wi = &self.st.wires[w];
        let wp = Wp { wire: w, net: wi.net, w: wi.w, s: wi.s, layer, own: [wi.pa, wi.pb], neck, relax };
        let mut strip: Strip = self.open_strip(w, k0, k1, layer);
        strip.walls.extend(self.via_walls(w, k0, k1, layer));
        // The wire's own lane on every edge it crosses (Bast et al.: the grid restricted to the
        // line's corridor): the parts of the edge outside the span its packing leaves it (the
        // interval, widened to the tight span the drawn neighbours leave less the room kept for
        // the undrawn ones, `reserved_span`, and to where the taut run crosses) are walls, so the octilinear shape takes no room planned for a wire not
        // drawn yet.
        {
            let (st, pr) = (self.st, &self.b.proj);
            // Per pass of the run: the lane bound on its left / right (seen along the wire) where
            // the neighbour on that side is not drawn yet; consecutive bounds on one side are
            // joined across their triangle (below).
            let mut guard: Vec<(usize, [Option<Vec2>; 2])> = Vec::new();
            for k in k0..k1 {
                let pid = wi.passes[k];
                let [p, q] = pr.edges[st.passes[pid].edge];
                let (pp, pq) = (pr.pos[p], pr.pos[q]);
                let len = (pq - pp).len();
                if len < 1.0 {
                    continue;
                }
                let u = (pq - pp) * (1.0 / len);
                let (mut lo, mut hi) = (self.iv.lo.get(pid).copied().unwrap_or(f64::NAN) * len, self.iv.hi.get(pid).copied().unwrap_or(f64::NAN) * len);
                if !lo.is_finite() || !hi.is_finite() {
                    continue;
                }
                // Never past the tight span: a neighbour drawn outside its own interval may
                // have taken part of this one, and the room for the undrawn passes between the
                // two stays (wiccon: GPIO16's interval reached to 0.2 mm of the drawn ADC trace,
                // where 3.3v, drawn later, is planned between them).
                if let (Some((a, b)), Some((ra, rb))) = (self.tight_span(pid), self.reserved_span(pid)) {
                    lo = lo.min(ra).max(a);
                    hi = hi.max(rb).min(b);
                }
                let nom = self.iv.nom.get(pid).copied().filter(|x| x.is_finite()).unwrap_or(0.5) * len;
                let own = super::draw::crossing(&taut, pp, pq, nom);
                if let Some(x) = own {
                    lo = lo.min(x - 1.0);
                    hi = hi.max(x + 1.0);
                }
                let (lo, hi) = (lo.max(0.0), hi.min(len));
                if std::env::var("TOPO_LANEDBG").is_ok() {
                    let lane = self.lane_dbg(pid);
                    eprintln!("LANE wire {w} k {k} edge {} ({:.3},{:.3})-({:.3},{:.3}) len {:.3} iv {:.3}..{:.3} tight {:?} res {:?} lohi {:.3}..{:.3} lane {lane}", st.passes[pid].edge, pp.x/1e6, -pp.y/1e6, pq.x/1e6, -pq.y/1e6, len/1e6, self.iv.lo[pid]*len/1e6, self.iv.hi[pid]*len/1e6, self.tight_span(pid).map(|(a,b)| (a/1e6,b/1e6)), self.reserved_span(pid).map(|(a,b)| (a/1e6,b/1e6)), lo/1e6, hi/1e6);
                }
                // Only towards a neighbour pass of the same edge and layer: on a side without
                // one the room is the obstacle's, which the clearance check guards.
                let (before, after) = self.lane_sides(pid);
                if before && lo > 1.0 {
                    strip.walls.push((pp, pp + u * lo));
                }
                if after && hi < len - 1.0 {
                    strip.walls.push((pp + u * hi, pq));
                }
                // Which end of the edge lies on the wire's left, coming from the triangle before
                // the pass.
                let t = pr.tris[wi.tris[k]];
                let c = (pr.pos[t[0]] + pr.pos[t[1]] + pr.pos[t[2]]) * (1.0 / 3.0);
                let mid = (pp + pq) * 0.5;
                let p_left = (mid - c).cross(pp - mid) > 0.0;
                let (prev_drawn, next_drawn) = self.lane_next_drawn(pid);
                // Inside the triangles the bound follows the undrawn neighbour's planned path: its
                // taut crossing less the spacing between the two (never past this wire's own run).
                let g_lo = if before && !prev_drawn {
                    let x = self.neighbour_bound(pid, false).map_or(lo, |nb| nb.max(lo));
                    let x = own.map_or(x, |o| x.min(o - 1.0));
                    (x > 1.0).then(|| pp + u * x)
                } else {
                    None
                };
                let g_hi = if after && !next_drawn {
                    let x = self.neighbour_bound(pid, true).map_or(hi, |nb| nb.min(hi));
                    let x = own.map_or(x, |o| x.max(o + 1.0));
                    (x < len - 1.0).then(|| pp + u * x)
                } else {
                    None
                };
                guard.push((k, if p_left { [g_lo, g_hi] } else { [g_hi, g_lo] }));
            }
            // The lane walls of two consecutive passes on one side, joined across the triangle
            // between them, where the neighbour on that side is not drawn yet. The edge walls
            // keep the room the lane packing reserves for the passes not drawn yet (their
            // spacing, from the next drawn pass or the lane end) only on the edges; inside a
            // long triangle a leg could run into that room and out again (wiccon 3.3v J1.1 ->
            // U1.3: GPIO16 drawn along the ADC trace at the clearance, no room left between them
            // for 3.3v, drawn later, which the plan routes there). The straight wall between two
            // points on two sides of the triangle cuts it into two convex parts, the room of the
            // undrawn neighbour in the other one: the capacity the plan counted is kept for the
            // wires drawn later, not a position (Dayan 1997 pp. 15-16: spacing for every wire;
            // Bast et al. 2020 section 4.3: room kept for the lines routed later).
            for g in guard.windows(2) {
                let ((ka, sa), (kb, sb)) = (g[0], g[1]);
                if kb != ka + 1 {
                    continue;
                }
                for side in 0..2 {
                    if let (Some(a), Some(b)) = (sa[side], sb[side]) {
                        strip.walls.push((a, b));
                    }
                }
            }
        }
        let pitch = wi.w + wi.s;
        let region: Aabb = bbox_of(&taut).grow(2.0 * MM + 4.0 * pitch);
        let ents = self.local_ents(&wp, region.grow(self.qgrow));
        // Cumulative length of the taut path, for the node order.
        let mut cum = vec![0.0];
        for p in taut.windows(2) {
            cum.push(cum.last().unwrap() + (p[1] - p[0]).len());
        }
        // Distance to the taut path, order along it, and the nearest point on it.
        let proj = |p: Vec2| -> (f64, f64, Vec2) {
            let mut best = (f64::INFINITY, 0.0, p);
            for (i, s) in taut.windows(2).enumerate() {
                let d = s[1] - s[0];
                let l2 = d.len2();
                let u = if l2 > 0.0 { ((p - s[0]).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
                let f = s[0] + d * u;
                let dist = (f - p).len();
                if dist < best.0 {
                    best = (dist, cum[i] + u * l2.sqrt(), f);
                }
            }
            best
        };
        // Nodes: (point, order, taut vertex index, on the taut path). A point on the taut path
        // (its vertices, and the foot of every octagon node) may continue along it at any angle
        // to the next such point: where 45 degrees does not fit, only that piece stays any-angle.
        let mut nodes: Vec<(Vec2, f64, Option<usize>, bool)> = taut.iter().enumerate().map(|(i, &p)| (p, cum[i], Some(i), true)).collect();
        // An octagon node matters where the taut path wraps its corner (within a fraction of the
        // corner's clearance radius of it) or runs next to it.
        let near = 0.5 * pitch;
        let mut extra: Vec<(f64, Vec2, f64, Vec2)> = Vec::new();
        let mut cand = Vec::new();
        for &id in &ents {
            cand.clear();
            self.octagon_nodes(id, &wp, &region, &mut cand);
            // A drawn wire's corners further out too: the run may follow it (bundles).
            let need = self.need_of(id, &wp, wp.w);
            let reach = if id >= self.n_static && cfg.pull > 0.0 { need + 2.0 * pitch } else { near + 0.15 * need };
            for &p in &cand {
                let (d, t, f) = proj(p);
                if d <= reach && t > 0.0 && t < *cum.last().unwrap() {
                    extra.push((d, p, t, f));
                }
            }
        }
        extra.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.total_cmp(&b.2)));
        let foot_gap = 0.25 * pitch;
        for (_, p, t, f) in extra {
            if nodes.len() >= MAX_NODES {
                break;
            }
            if nodes.iter().any(|n| (n.0 - p).len2() < 100.0) {
                continue;
            }
            if strip.contains(p) && self.seg_clear(&ents, &wp, p, p) {
                nodes.push((p, t, None, false));
                if !nodes.iter().any(|n| n.3 && (n.1 - t).abs() < foot_gap) {
                    nodes.push((f, t, None, true));
                }
            }
        }
        nodes.sort_by(|a, b| a.1.total_cmp(&b.1).then(b.2.is_some().cmp(&a.2.is_some())).then(b.3.cmp(&a.3)));
        // The two ends stay first and last.
        let n = nodes.len();
        let last_t = nodes.iter().position(|x| x.2 == Some(taut.len() - 1)).unwrap_or(n - 1);
        if last_t != n - 1 {
            let e = nodes.remove(last_t);
            nodes.push(e);
        }
        let first_t = nodes.iter().position(|x| x.2 == Some(0)).unwrap_or(0);
        if first_t != 0 {
            let s = nodes.remove(first_t);
            nodes.insert(0, s);
        }
        // The next node on the taut path after each node on it.
        let next_on: Vec<Option<usize>> = (0..n).map(|i| if nodes[i].3 { (i + 1..n).find(|&j| nodes[j].3) } else { None }).collect();
        let leff = cfg.spacing_nm.min(2.0 * pitch).max(1.0);
        // Drawn octilinear segments of other nets near the run, with the centre distance at which
        // this wire would run beside them at the clearance (bundle following).
        let side: Vec<(Vec2, Vec2, usize, f64)> = if cfg.pull > 0.0 {
            ents.iter()
                .filter(|&&id| id >= self.n_static && self.ents[id].net != Some(wp.net))
                .filter_map(|&id| match self.ents[id].core {
                    Core::Seg(a, b) => octi_dir(b - a).map(|k| (a, b, k % 4, self.need_of(id, &wp, wp.w))),
                    _ => None,
                })
                .collect()
        } else {
            Vec::new()
        };
        let tol = 0.05 * pitch + 1_000.0;
        let mut follow_cache: HashMap<[u64; 4], f64> = HashMap::new();
        // Length of the leg `a-b` (direction `k`) that runs beside a drawn neighbour at its
        // clearance (the overlap of their projections).
        let mut follow = |a: Vec2, b: Vec2, k: usize| -> f64 {
            let key = [a.x.to_bits(), a.y.to_bits(), b.x.to_bits(), b.y.to_bits()];
            if let Some(&v) = follow_cache.get(&key) {
                return v;
            }
            let u = dir(k);
            let (t0, t1) = (0.0f64, (b - a).dot(u));
            let mut best = 0.0f64;
            for &(c, e, kk, need) in &side {
                if kk != k % 4 {
                    continue;
                }
                let off = u.cross(c - a).abs();
                if (off - need).abs() > tol {
                    continue;
                }
                let (s0, s1) = ((c - a).dot(u), (e - a).dot(u));
                let ov = (t1.min(s0.max(s1)) - t0.max(s0.min(s1))).max(0.0);
                best = best.max(ov);
            }
            follow_cache.insert(key, best);
            best
        };
        let mut legal_cache: HashMap<[u64; 4], bool> = HashMap::new();
        let mut legal = |a: Vec2, b: Vec2| -> bool {
            let key = [a.x.to_bits(), a.y.to_bits(), b.x.to_bits(), b.y.to_bits()];
            if let Some(&v) = legal_cache.get(&key) {
                return v;
            }
            let bb = bbox_of(&[a, b]);
            let v = bb.min.x >= region.min.x
                && bb.min.y >= region.min.y
                && bb.max.x <= region.max.x
                && bb.max.y <= region.max.y
                && strip.contains(b)
                && strip.contains((a + b) * 0.5)
                && !strip.crosses_wall(a, b)
                && self.seg_clear(&ents, &wp, a, b);
            legal_cache.insert(key, v);
            v
        };
        // DP state per node: arrival direction 0..7, or 8 (the start / an any-angle leg).
        const ND: usize = 9;
        let mut dist = vec![[f64::INFINITY; ND]; n];
        // (previous node, previous state, shape id, legs)
        #[allow(clippy::type_complexity)]
        let mut back: Vec<[Option<(usize, usize, Vec<Leg>)>; ND]> = vec![std::array::from_fn(|_| None); n];
        // Last leg of the state: unit direction and length.
        let mut last: Vec<[(Vec2, f64); ND]> = vec![[(Vec2::default(), f64::INFINITY); ND]; n];
        dist[0][8] = 0.0;
        let bend = |u: Vec2, l_in: f64, v: Vec2| -> f64 {
            if u.len2() < 1e-18 {
                return 0.0;
            }
            let t = turn_deg(u, v);
            if t < 0.5 {
                return 0.0;
            }
            if t > 179.0 {
                return f64::INFINITY;
            }
            let mut p = cfg.bend_nm * t / 45.0;
            if t > 90.5 {
                if cfg.acute_forbidden {
                    return f64::INFINITY;
                }
                p += cfg.acute_nm * acute_amount(t);
            }
            if l_in < leff {
                p += 2.0 * cfg.bend_nm * (1.0 - l_in / leff).powi(2);
            }
            p
        };
        // Price of leaving / entering a pad off its axis (up to two bends' worth at 90 degrees),
        // or along it but turning before the pad's tip is cleared (graded like the jog penalty):
        // pad exit along the pad axis until clear of the neighbour pads.
        let axis_price = |e: &Option<Exit>, d: Vec2, len: f64| -> f64 {
            match e {
                Some(x) => {
                    let t = turn_deg(x.axis, d);
                    let off = t.min(180.0 - t);
                    if off < 0.5 {
                        2.0 * cfg.bend_nm * (1.0 - len / x.reach.max(1.0)).max(0.0).powi(2)
                    } else {
                        cfg.bend_nm * 2.0 * (off / 45.0)
                    }
                }
                None => 0.0,
            }
        };
        for i in 0..n {
            for din in 0..ND {
                let d0 = dist[i][din];
                if !d0.is_finite() {
                    continue;
                }
                let (u_in, l_in) = last[i][din];
                let mut js: Vec<usize> = (i + 1..n.min(i + 1 + WINDOW_MAX)).take_while(|&j| j < i + 1 + WINDOW || nodes[j].1 - nodes[i].1 <= REACH_MM * MM).collect();
                if let Some(j) = next_on[i] {
                    if !js.contains(&j) {
                        js.push(j);
                    }
                }
                if !js.contains(&(n - 1)) && n - 1 > i {
                    js.push(n - 1);
                }
                for &j in &js {
                    let (p, q) = (nodes[i].0, nodes[j].0);
                    let mut shapes: Vec<Vec<Leg>> = connectors(p, q);
                    if i == 0 {
                        if let Some(x) = exits[0] {
                            let p2 = p + x.axis * x.reach;
                            if (q - p).dot(x.axis) > x.reach {
                                for mut s in connectors(p2, q) {
                                    s.insert(0, (p2, octi_dir(x.axis)));
                                    shapes.push(s);
                                }
                            }
                        }
                    }
                    if j == n - 1 {
                        if let Some(x) = exits[1] {
                            let q2 = q + x.axis * x.reach;
                            if (p - q).dot(x.axis) > x.reach {
                                for mut s in connectors(p, q2) {
                                    s.push((q, octi_dir(-x.axis)));
                                    shapes.push(s);
                                }
                            }
                        }
                    }
                    // A piece of the taut path itself (any angle), between consecutive nodes on it.
                    if next_on[i] == Some(j) && octi_dir(q - p).is_none() && !cfg.any_forbidden {
                        shapes.push(vec![(q, None)]);
                    }
                    for s in shapes {
                        // Price.
                        let mut c = 0.0;
                        let (mut u, mut l, mut from) = (u_in, l_in, p);
                        // One any-angle stretch pays its per-stretch price once, however many
                        // pieces of the taut path it is made of.
                        let mut prev_any = din == 8 && i != 0;
                        let mut ok = true;
                        for (k, &(e, dk)) in s.iter().enumerate() {
                            let d = e - from;
                            let len = d.len();
                            if len < 1.0 {
                                continue;
                            }
                            let v = d * (1.0 / len);
                            let b = bend(u, l, v);
                            if !b.is_finite() {
                                ok = false;
                                break;
                            }
                            c += len + b;
                            if let (Some(kk), true) = (dk, cfg.pull > 0.0 && !side.is_empty()) {
                                c -= cfg.pull * follow(from, e, kk);
                            }
                            if dk.is_none() {
                                c += cfg.any_nm * (any_angle_amount(len) - if prev_any { 1.0 } else { 0.0 });
                            }
                            prev_any = dk.is_none();
                            if i == 0 && k == 0 {
                                c += axis_price(&exits[0], v, len);
                            }
                            if j == n - 1 && k == s.len() - 1 {
                                c += axis_price(&exits[1], v, len);
                            }
                            u = v;
                            l = len;
                            from = e;
                        }
                        if !ok {
                            continue;
                        }
                        let dout = s.last().and_then(|x| x.1).unwrap_or(8);
                        if d0 + c >= dist[j][dout] {
                            continue;
                        }
                        // Legality (the taut segment is legal as drawn).
                        let mut from = p;
                        let mut leg_ok = true;
                        for &(e, dk) in &s {
                            if (e - from).len2() >= 1.0 && dk.is_some() && !legal(from, e) {
                                leg_ok = false;
                                break;
                            }
                            from = e;
                        }
                        if !leg_ok {
                            continue;
                        }
                        dist[j][dout] = d0 + c;
                        last[j][dout] = (u, l);
                        back[j][dout] = Some((i, din, s));
                    }
                }
            }
        }
        let (mut j, mut d) = (n - 1, (0..ND).min_by(|&a, &b| dist[n - 1][a].total_cmp(&dist[n - 1][b]))?);
        if !dist[j][d].is_finite() {
            return None;
        }
        let mut chain: Vec<Vec<Leg>> = Vec::new();
        while let Some((i, din, s)) = back[j][d].clone() {
            chain.push(s);
            j = i;
            d = din;
        }
        chain.reverse();
        let mut out = vec![taut[0]];
        for s in chain {
            for (e, _) in s {
                if (e - *out.last().unwrap()).len2() >= 1.0 {
                    out.push(e);
                }
            }
        }
        // Merge collinear legs.
        let mut i = 1;
        while i + 1 < out.len() {
            let (a, b, c) = (out[i - 1], out[i], out[i + 1]);
            if (b - a).cross(c - b).abs() / (c - a).len().max(1.0) <= 0.5 && (b - a).dot(c - b) > 0.0 {
                out.remove(i);
            } else {
                i += 1;
            }
        }
        if out == taut {
            return None;
        }
        Some(OctiOut { pts: out })
    }

    /// The corridor of the octilinear search for passes `k0..k1` of `w` on `layer`: the sleeve's
    /// triangles and `OPEN_RINGS` rings of triangles around them (inside the board). A side is a
    /// wall only where a pass of another wire not drawn yet on this layer (or not yet layered)
    /// crosses it, or where the region ends: crossing an edge that no such pass uses moves the
    /// wire past nobody (a pad or a drawn wire is guarded by the clearance check), so an elbow may
    /// leave the sleeve where the room is free (an empty board's few triangles, a pad end entered
    /// from another side). O(region triangles x passes per edge).
    pub(super) fn open_strip(&self, w: WireId, k0: usize, k1: usize, layer: usize) -> Strip {
        const OPEN_RINGS: usize = 6;
        let (st, pr) = (self.st, &self.b.proj);
        let wi = &st.wires[w];
        let mut region: Vec<super::state::TriId> = wi.tris[k0..=k1].to_vec();
        region.sort_unstable();
        region.dedup();
        let mut frontier = region.clone();
        for _ in 0..OPEN_RINGS {
            let mut next = Vec::new();
            for &t in &frontier {
                for &e in &pr.tri_edges[t] {
                    if let Some(x) = pr.across(e, t) {
                        if !pr.tri_out[x] && region.binary_search(&x).is_err() && !next.contains(&x) {
                            next.push(x);
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            region.extend(next.iter().copied());
            region.sort_unstable();
            frontier = next;
        }
        let portal: Vec<usize> = (k0..k1).map(|k| st.passes[wi.passes[k]].edge).collect();
        let blocked = |e: usize| {
            !portal.contains(&e)
                && st.edge_order[e].iter().any(|&q| {
                let qw = st.passes[q].wire;
                qw != w && (self.pass_layer.get(q).is_none_or(|&l| l < 0 || l as usize == layer)) && self.live_x(q).is_none()
            })
        };
        let mut tris = Vec::with_capacity(region.len());
        let mut walls = Vec::new();
        for &t in &region {
            let p = pr.tris[t].map(|v| pr.pos[v]);
            tris.push(p);
            for i in 0..3 {
                let e = pr.tri_edges[t][i];
                let inside = pr.across(e, t).is_some_and(|x| region.binary_search(&x).is_ok());
                // An inner edge is shared by two region triangles: add it once.
                if !inside || blocked(e) && pr.across(e, t).is_some_and(|x| x > t) {
                    walls.push((p[i], p[(i + 1) % 3]));
                }
            }
        }
        let boxes = tris.iter().map(|t| bbox_of(t)).collect();
        Strip { tris, boxes, walls }
    }

    /// Octagon vertices (edges at 0/45/90 degrees) around the corners of object `id` inside `bb`:
    /// a round end (a via, a track end, a point) gets the whole octagon; a convex polygon corner
    /// the vertices within its turn; a reflex corner the offset-line intersection.
    fn octagon_nodes(&self, id: usize, wp: &Wp, bb: &Aabb, out: &mut Vec<Vec2>) {
        let need = self.need_of(id, wp, wp.w);
        let r = need * OCT_R;
        let inside = |p: Vec2| p.x >= bb.min.x - need && p.x <= bb.max.x + need && p.y >= bb.min.y - need && p.y <= bb.max.y + need;
        let oct = |c: Vec2, out: &mut Vec<Vec2>| {
            for k in 0..8 {
                let a = (k as f64 + 0.5) * FRAC_PI_4;
                out.push(c + Vec2 { x: a.cos() * r, y: a.sin() * r });
            }
        };
        match &self.ents[id].core {
            Core::Pt(c) => {
                if inside(*c) {
                    oct(*c, out);
                }
            }
            Core::Seg(a, b) => {
                for c in [*a, *b] {
                    if inside(c) {
                        oct(c, out);
                    }
                }
            }
            Core::Poly(poly) => {
                let n = poly.len();
                let area: f64 = (0..n).map(|i| poly[i].cross(poly[(i + 1) % n])).sum();
                let sgn = if area >= 0.0 { 1.0 } else { -1.0 };
                for i in 0..n {
                    let v = poly[i];
                    if !inside(v) {
                        continue;
                    }
                    let (p, q) = (poly[(i + n - 1) % n], poly[(i + 1) % n]);
                    let (Some(u1), Some(u2)) = ((v - p).unit(), (q - v).unit()) else { continue };
                    let n1 = Vec2 { x: u1.y, y: -u1.x } * sgn;
                    let n2 = Vec2 { x: u2.y, y: -u2.x } * sgn;
                    let turn = u1.cross(u2).atan2(u1.dot(u2)) * sgn;
                    if turn > 1e-3 {
                        // Octagon vertex angles strictly inside the wedge of outward normals.
                        let a0 = n1.angle();
                        for k in 0..8 {
                            let a = (k as f64 + 0.5) * FRAC_PI_4;
                            let rel = if sgn > 0.0 { (a - a0).rem_euclid(std::f64::consts::TAU) } else { (a0 - a).rem_euclid(std::f64::consts::TAU) };
                            if rel > 1e-6 && rel < turn - 1e-6 {
                                out.push(v + Vec2 { x: a.cos() * r, y: a.sin() * r });
                            }
                        }
                        // The offset edges' ends at the corner (on an axis-aligned pad these
                        // are octilinear lines already).
                        out.push(v + n1 * (need * (1.0 + 2e-6) + 0.5));
                        out.push(v + n2 * (need * (1.0 + 2e-6) + 0.5));
                    } else if let Some(bis) = (n1 + n2).unit() {
                        let c = bis.dot(n1).max(0.2);
                        out.push(v + bis * (need / c * 1.0005));
                    }
                }
            }
        }
    }
}

/// Octilinear connectors from `p` to `q`: the straight leg when they are aligned, else the two
/// elbows and the two Z shapes over the two octilinear directions that bracket `q - p`.
fn connectors(p: Vec2, q: Vec2) -> Vec<Vec<Leg>> {
    let d = q - p;
    if d.len2() < 1.0 {
        return Vec::new();
    }
    if let Some(k) = octi_dir(d) {
        return vec![vec![(q, Some(k))]];
    }
    let a = d.y.atan2(d.x).rem_euclid(std::f64::consts::TAU);
    let k0 = ((a / FRAC_PI_4).floor() as usize) % 8;
    let k1 = (k0 + 1) % 8;
    let (d0, d1) = (dir(k0), dir(k1));
    let den = d0.cross(d1);
    let al = d.cross(d1) / den;
    let be = d0.cross(d) / den;
    if al <= 0.0 || be <= 0.0 {
        return Vec::new();
    }
    vec![
        vec![(p + d0 * al, Some(k0)), (q, Some(k1))],
        vec![(p + d1 * be, Some(k1)), (q, Some(k0))],
        vec![(p + d0 * (0.5 * al), Some(k0)), (p + d0 * (0.5 * al) + d1 * be, Some(k1)), (q, Some(k0))],
        vec![(p + d1 * (0.5 * be), Some(k1)), (p + d1 * (0.5 * be) + d0 * al, Some(k0)), (q, Some(k1))],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x, y }
    }

    #[test]
    fn directions_are_recognised() {
        assert_eq!(octi_dir(v(5.0, 0.0)), Some(0));
        assert_eq!(octi_dir(v(3.0, 3.0)), Some(1));
        assert_eq!(octi_dir(v(0.0, -2e6)), Some(6));
        assert_eq!(octi_dir(v(-1e6, 1e6 + 0.5)), Some(3));
        assert_eq!(octi_dir(v(2e6, 1e6)), None);
    }

    #[test]
    fn connectors_reach_the_target_octilinear() {
        let (p, q) = (v(0.0, 0.0), v(3e6, 1e6));
        let cs = connectors(p, q);
        assert_eq!(cs.len(), 4);
        for c in &cs {
            let mut from = p;
            for &(e, k) in c {
                assert_eq!(octi_dir(e - from), k, "{c:?}");
                from = e;
            }
            assert!((from - q).len() < 1e-6);
        }
        // The elbows have the same length (0/45 split of 3 x 1 mm: 2 + 1.414 mm).
        let len = |c: &Vec<Leg>| {
            let mut f = p;
            c.iter().map(|&(e, _)| { let l = (e - f).len(); f = e; l }).sum::<f64>()
        };
        assert!((len(&cs[0]) - (2e6 + 2f64.sqrt() * 1e6)).abs() < 1.0);
        assert!((len(&cs[2]) - len(&cs[0])).abs() < 1.0);
    }

    #[test]
    fn breaks_count_stretches_and_acute_bends() {
        let pts = [v(0.0, 0.0), v(1e6, 0.0), v(2e6, 0.5e6), v(3e6, 0.7e6), v(4e6, 0.7e6), v(3e6, 1.7e6)];
        let (any, acute) = breaks(&pts);
        assert_eq!(any.len(), 1, "{any:?}");
        assert!((any[0].2 - (v(1e6, 0.5e6).len() + v(1e6, 0.2e6).len())).abs() < 1.0);
        assert_eq!(acute.len(), 1);
        assert!((acute[0].1 - 45.0).abs() < 1e-6);
    }
}

impl Drawer<'_> {
    /// Post-pass of the octilinear style over a drawn wire (`wo`, plain legal copper only, as
    /// [`Drawer::refit_run`]): every segment not at 0/45/90 degrees that a later step drew (a
    /// branch end fused onto its trunk) is replaced by the elbow or Z shape with the least turning
    /// that the checker accepts against everything drawn; else it stays. Returns the number of
    /// segments replaced. O(segments x shapes x checker query).
    pub fn octi_polish(&mut self, wo: &mut super::draw::WireOut) -> usize {
        let mut n = 0;
        for r in 0..wo.runs.len() {
            let mut cur = wo.runs[r].1.clone();
            if breaks(&cur).0.is_empty() {
                continue;
            }
            let mut i = 0;
            while i + 1 < cur.len() {
                let (a, b) = (cur[i], cur[i + 1]);
                if (b - a).len2() <= 1.0 || octi_dir(b - a).is_some() {
                    i += 1;
                    continue;
                }
                let u_in = if i > 0 { a - cur[i - 1] } else { Vec2::default() };
                let u_out = if i + 2 < cur.len() { cur[i + 2] - b } else { Vec2::default() };
                let mut shapes: Vec<(f64, Vec<Vec2>)> = connectors(a, b)
                    .into_iter()
                    .map(|s| {
                        let pts: Vec<Vec2> = s.iter().map(|x| x.0).collect();
                        let mut turn = 0.0;
                        let (mut u, mut from) = (u_in, a);
                        for &p in &pts {
                            turn += turn_deg(u, p - from);
                            u = p - from;
                            from = p;
                        }
                        turn += turn_deg(u, u_out);
                        (turn, pts)
                    })
                    .collect();
                shapes.sort_by(|x, y| x.0.total_cmp(&y.0));
                let mut done = false;
                for (_, s) in shapes {
                    let mut trial = cur[..=i].to_vec();
                    trial.extend(s.iter().copied());
                    trial.extend_from_slice(&cur[i + 2..]);
                    if self.refit_run(wo, r, trial.clone()) {
                        i += s.len();
                        cur = trial;
                        n += 1;
                        done = true;
                        break;
                    }
                }
                if !done {
                    i += 1;
                }
            }
        }
        n
    }
}

/// One bend that a bundle takes together (M-45 item 4): at `at` the inner wire turns by `deg`,
/// and a wire of another net runs parallel outside it at centre distance `d_nm` on both legs.
/// The outer wire is `skew_nm` = `2 d tan(deg / 2)` longer there (0.83 x pitch per 45 degree
/// bend, the plan's diff-pair note); bends of the opposite sense cancel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BundleBend {
    pub at: Vec2,
    pub deg: f64,
    pub d_nm: f64,
    pub skew_nm: f64,
    /// +1 for a left turn, -1 for a right turn (for cancelling pairs).
    pub sense: f64,
}

/// The bundle bends of the drawn runs `runs` = `(wire, net, layer, polyline)`: every bend of a
/// run with a wire of another net parallel outside it on both legs within `1.5 x pitch`
/// (`pitch(wire)`, centre to centre). O(bends x segments of the layer).
pub fn bundle_bends(runs: &[(usize, i32, usize, &[Vec2])], pitch: &dyn Fn(usize) -> f64) -> Vec<BundleBend> {
    let mut segs: Vec<(usize, i32, usize, Vec2, Vec2)> = Vec::new();
    for &(w, net, l, pts) in runs {
        for p in pts.windows(2) {
            if (p[1] - p[0]).len2() > 1.0 {
                segs.push((w, net, l, p[0], p[1]));
            }
        }
    }
    // Perpendicular offset of segment (c, e) from the line through a with unit direction u, if
    // parallel and overlapping the stretch [s0, s1] of that line near the bend.
    let par = |a: Vec2, u: Vec2, s0: f64, s1: f64, c: Vec2, e: Vec2| -> Option<f64> {
        let d = e - c;
        let l = d.len();
        if l < 1.0 || (d.cross(u) / l).abs() > 1e-4 {
            return None;
        }
        let (t0, t1) = ((c - a).dot(u), (e - a).dot(u));
        let (lo, hi) = (t0.min(t1), t0.max(t1));
        (hi > s0 && lo < s1).then(|| u.cross(c - a))
    };
    let mut out = Vec::new();
    for &(w, net, l, pts) in runs {
        let pts: Vec<Vec2> = {
            let mut v = pts.to_vec();
            v.dedup_by(|a, b| (*a - *b).len2() < 1.0);
            v
        };
        let p0 = pitch(w);
        for i in 1..pts.len().saturating_sub(1) {
            let (a, v, b) = (pts[i - 1], pts[i], pts[i + 1]);
            let (Some(u1), Some(u2)) = ((v - a).unit(), (b - v).unit()) else { continue };
            let deg = turn_deg(u1, u2);
            if !(0.5..=135.5).contains(&deg) {
                continue;
            }
            let sense = if u1.cross(u2) > 0.0 { 1.0 } else { -1.0 };
            // Outside of the turn: right of the travel for a left turn.
            let side = -sense;
            let (l1, l2) = ((v - a).len(), (b - v).len());
            let reach = 1.5 * p0;
            let mut best: Option<f64> = None;
            for &(w2, n2, l2_, c, e) in &segs {
                if w2 == w || n2 == net || l2_ != l {
                    continue;
                }
                // Parallel to the incoming leg near its end, on the outside.
                let Some(o1) = par(v, u1, -l1.min(reach * 4.0), 0.0, c, e) else { continue };
                if o1 * side <= 0.0 || o1.abs() > reach {
                    continue;
                }
                // The same wire parallel to the outgoing leg at the same distance.
                let d = o1.abs();
                let found = segs.iter().any(|&(w3, _, l3, c2, e2)| {
                    w3 == w2 && l3 == l && par(v, u2, 0.0, l2.min(reach * 4.0), c2, e2).is_some_and(|o2| o2 * side > 0.0 && (o2.abs() - d).abs() <= 0.05 * d + 1_000.0)
                });
                if found && best.is_none_or(|x| d < x) {
                    best = Some(d);
                }
            }
            if let Some(d) = best {
                let half = (deg * 0.5).to_radians();
                out.push(BundleBend { at: v, deg, d_nm: d, skew_nm: 2.0 * d * half.tan(), sense });
            }
        }
    }
    out
}
