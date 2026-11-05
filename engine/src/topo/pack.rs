//! The packer's capacity test, shared by the drawing and by P3/P4. The passes of one edge on one
//! layer, in edge order, are packed between the two end margins (the distance along the edge
//! from each end at which a centre line keeps its clearance to the obstacle there) with the
//! demand model's spacing between neighbours (half the widths plus, for different nets, the
//! larger clearance and a tolerance), each spacing stretched by the oblique-chord factor of the
//! wire crossing the edge at an angle (`1 / sin`, 0.4 .. 1). Leftmost positions `lmin` and
//! rightmost positions `rmax` follow from the two ends; the passes fit when `lmin <= rmax` for
//! every one. `Drawer::pack` takes its intervals from the same numbers, and
//! [`crate::topo::refine`] prices a labelling by how far `lmin` exceeds `rmax`, so P3 and the
//! drawing agree on what fits.
//!
//! The crossing direction of a pass (for the oblique factor) comes from the wire's *taut path*
//! ([`Packer::taut`]): the shortest path from terminal to terminal through its sleeve that
//! crosses every edge of its pass list inside the edge's end margins (string pulling, the
//! simple stupid funnel over the per-edge intervals; the margins are the least over the layers,
//! since a pass may run on any layer). Where the taut path runs straight the direction is that
//! of the segment; where it bends at a pass it wraps the end vertex of that edge, and wires
//! packed there follow arcs about it, which cross the (radial) edge square when the edge lies in
//! the wrapped wedge, else the straight part before or after the bend. The evenly
//! spread pass points of [`State::polyline`] are not used for it: on short edges near pins they
//! zig-zag and drove `1 / sin` into its 2.5 cap where the wire runs straight (plan D1).
//!
//! Complexity: a margin is a 30-step bisection over the end obstacle's polygon, cached per
//! (edge, end, net, width, clearance, layer); a fit test is O(passes); a taut path is O(passes
//! of the wire) (the funnel restarts at most once per corner it adds), cached per wire content.

use super::draw::{poly_dist, seg_dist};
use super::state::{EdgeId, Obstacle, Projection, State, VertKind, WireId};
use crate::geom::Vec2;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// One pass to pack: its net, width, clearance and the oblique-chord factor.
#[derive(Clone, Copy, Debug)]
pub struct PackItem {
    pub net: i32,
    pub w: f64,
    pub s: f64,
    pub infl: f64,
}

/// The packing numbers of the passes of one edge on one layer.
#[derive(Clone, Debug, Default)]
pub struct Bounds {
    /// Allowed span along the edge for each pass (distance from end 0).
    pub lo: Vec<f64>,
    pub hi: Vec<f64>,
    /// Leftmost and rightmost position of each pass.
    pub lmin: Vec<f64>,
    pub rmax: Vec<f64>,
    /// Required centre distance between neighbour `i` and `i + 1`.
    pub req: Vec<f64>,
}

impl Bounds {
    /// How far the packing is over: the largest `lmin - rmax` (nm; <= 0 when everything fits).
    pub fn worst(&self) -> f64 {
        self.lmin.iter().zip(&self.rmax).map(|(a, b)| a - b).fold(f64::NEG_INFINITY, f64::max)
    }

    pub fn fits(&self) -> bool {
        self.lmin.iter().zip(&self.rmax).all(|(a, b)| *a <= *b + 1e-6)
    }
}

/// The drawing's numeric tolerance on top of `margin_nm` (nm). The Checker judges the drawn
/// items after rounding to whole nm with the exact rule (`gap < required` fails), and the exact
/// path accepts 0.5 nm short ([`super::exact`]), so geometry computed at exactly the clearance
/// can fail by a fraction of a nm. Measured on the drawing witness A (hand topology, margin 0):
/// tolerance 0 rejects 116 / 103 runs on tanmatsu / hh2027, 2 nm and 10 nm reject 107 / 58,
/// 2 um (the S2a value) 115 / 60. 10 nm is far below any fab or DRC resolution.
pub const DRAW_TOL_NM: f64 = 10.0;

/// Margin cache key: edge, end, net, width bits, clearance bits, layer (`u8::MAX`: any layer).
type MarginKey = (usize, u8, i32, u64, u64, u8);

/// Taut path cache key: the wire's net, width and clearance bits, terminal vertices, turn
/// corners and the edges of its passes (everything the taut path depends on).
type TautKey = Vec<u64>;

/// The taut path of one wire (see the module doc).
#[derive(Clone, Debug, Default)]
pub struct Taut {
    /// Crossing point of every pass on its edge (math frame, nm).
    pub at: Vec<Vec2>,
    /// Crossing direction of every pass (not normalised; zero when undefined).
    pub dir: Vec<Vec2>,
}

pub struct Packer<'a> {
    pr: &'a Projection,
    outline: Vec<(Vec2, Vec2)>,
    /// Extra distance on top of every clearance (nm).
    eps: f64,
    /// Copper-to-edge clearance kept (nm; 0 when that check is ignored).
    edge_clear: f64,
    cache: RefCell<HashMap<MarginKey, f64>>,
    taut_cache: RefCell<HashMap<TautKey, Rc<Taut>>>,
}

/// The oblique-chord factor of a pass through edge `e` crossed in direction `d`: `1 / sin` of
/// the crossing angle, 1 .. 2.5 (1 when `d` is shorter than 1 nm, i.e. undefined).
pub fn infl_dir(pr: &Projection, e: EdgeId, d: Vec2) -> f64 {
    let [p, q] = pr.edges[e];
    let (pp, pq) = (pr.pos[p], pr.pos[q]);
    let ed = (pq - pp) * (1.0 / (pq - pp).len().max(1.0));
    let dl = d.len();
    if dl < 1.0 {
        1.0
    } else {
        1.0 / (d.cross(ed).abs() / dl).clamp(0.4, 1.0)
    }
}

/// The oblique-chord factor of the pass of a polyline `pl` at traversal `k` (the pass point is
/// `pl[k + 1]`) through edge `e`, the direction taken from its two neighbours. Used for the
/// hand's own crossings in the drawing witness; the router uses [`Packer::taut`] directions.
pub fn infl_of(pr: &Projection, e: EdgeId, pl: &[Vec2], k: usize) -> f64 {
    infl_dir(pr, e, pl[k + 2] - pl[k])
}

/// Shortest path through a sequence of portals (simple stupid funnel). `ports[i]` is
/// `(left, right)` seen in the direction of travel; the first and the last are the two
/// terminals (`left == right`). Returns the path corners with the index of the portal each lies
/// on, first and last included. O(portals) when the path is straight, O(portals x corners) at
/// worst (the scan restarts after each new corner from that corner's portal; the corner index
/// strictly grows, so it ends; capped at `n^2 + 16` steps as a guard).
pub fn funnel(ports: &[(Vec2, Vec2)]) -> Vec<(Vec2, usize)> {
    let n = ports.len();
    if n == 0 {
        return Vec::new();
    }
    let mut out = vec![(ports[0].0, 0usize)];
    let (mut apex, mut ai) = (ports[0].0, 0usize);
    let (mut lp, mut li, mut rp, mut ri) = (apex, ai, apex, ai);
    let mut i = 1;
    let mut steps = 0usize;
    while i < n && steps < n * n + 16 {
        steps += 1;
        let (l, r) = ports[i];
        // Right side: `r` tightens when it lies left of (or on) apex -> right.
        if (rp - apex).cross(r - apex) >= 0.0 {
            if apex == rp || (lp - apex).cross(r - apex) < 0.0 {
                rp = r;
                ri = i;
            } else {
                // Crosses over the left side: the left point is a corner.
                out.push((lp, li));
                (apex, ai) = (lp, li);
                (lp, li, rp, ri) = (apex, ai, apex, ai);
                i = ai + 1;
                continue;
            }
        }
        // Left side: `l` tightens when it lies right of (or on) apex -> left.
        if (lp - apex).cross(l - apex) <= 0.0 {
            if apex == lp || (rp - apex).cross(l - apex) > 0.0 {
                lp = l;
                li = i;
            } else {
                out.push((rp, ri));
                (apex, ai) = (rp, ri);
                (lp, li, rp, ri) = (apex, ai, apex, ai);
                i = ai + 1;
                continue;
            }
        }
        i += 1;
    }
    let last = ports[n - 1].0;
    if out.last().is_none_or(|c| c.1 != n - 1) {
        out.push((last, n - 1));
    }
    out
}

impl<'a> Packer<'a> {
    pub fn new(pr: &'a Projection, outline: Vec<(Vec2, Vec2)>, eps: f64, edge_clear: f64) -> Self {
        Packer { pr, outline, eps, edge_clear, cache: RefCell::new(HashMap::new()), taut_cache: RefCell::new(HashMap::new()) }
    }

    /// The packer of `model` over the projection `pr`, with the drawing's tolerance and edge rule.
    pub fn for_model(model: &crate::model::BoardModel, opts: &crate::model::Options, pr: &'a Projection) -> Self {
        let edge_clear = if model.rules.ignored_checks.iter().any(|c| c == "copper_edge_clearance") { 0.0 } else { pr.edge_clear.max(pr.edge_clear_ignored) };
        Packer::new(pr, super::board::outline_segments(model), opts.margin_nm as f64 + DRAW_TOL_NM, edge_clear)
    }

    pub fn eps(&self) -> f64 {
        self.eps
    }

    pub fn edge_clear(&self) -> f64 {
        self.edge_clear
    }

    /// Clearance a wire with clearance `s` needs to the obstacle `ob` on edge `e` (nm).
    fn clear_to(&self, ob: &Obstacle, e: EdgeId, s: f64) -> f64 {
        match self.pr.regime(e) {
            Some(r) if !ob.bare => r,
            _ => {
                if ob.bare {
                    ob.clear
                } else {
                    ob.clear.max(s)
                }
            }
        }
    }

    /// Distance from end `end` of edge `e` along the edge at which a centre line of width `w` on
    /// `layer` keeps its clearance to the obstacle at that vertex (0 when none). Cached.
    pub fn margin(&self, e: EdgeId, end: usize, net: i32, w: f64, s: f64, layer: usize) -> f64 {
        let key = (e, end as u8, net, w.to_bits(), s.to_bits(), layer as u8);
        if let Some(&m) = self.cache.borrow().get(&key) {
            return m;
        }
        let m = self.margin_raw(e, end, net, w, s, layer);
        self.cache.borrow_mut().insert(key, m);
        m
    }

    /// [`Packer::margin`] on whichever layer needs the least: the taut path is one per wire, not
    /// per layer, and a pass may run on any layer, under an SMD pad of another layer too (a
    /// drilled obstacle is a wall on every layer and keeps its margin). Cached.
    pub fn margin_any(&self, e: EdgeId, end: usize, net: i32, w: f64, s: f64) -> f64 {
        let key = (e, end as u8, net, w.to_bits(), s.to_bits(), u8::MAX);
        if let Some(&m) = self.cache.borrow().get(&key) {
            return m;
        }
        let m = (0..self.pr.n_layers.max(1)).map(|l| self.margin(e, end, net, w, s, l)).fold(f64::INFINITY, f64::min);
        self.cache.borrow_mut().insert(key, m);
        m
    }

    /// The taut path of wire `w` of `st` (see the module doc). Cached per wire content (net,
    /// width, clearance, terminals, turns and pass edges), so an unchanged wire is never
    /// recomputed, whatever else of the State changes. O(passes of the wire).
    pub fn taut(&self, st: &State, w: WireId) -> Rc<Taut> {
        let wi = &st.wires[w];
        let mut key: TautKey = Vec::with_capacity(6 + 2 * wi.turns.len() + wi.passes.len());
        key.extend([wi.net as i64 as u64, wi.w.to_bits(), wi.s.to_bits(), wi.a as u64, wi.b as u64, wi.turns.len() as u64]);
        for &(k, v) in &wi.turns {
            key.extend([k as u64, v as u64]);
        }
        key.extend(wi.passes.iter().map(|&p| st.passes[p].edge as u64));
        if let Some(t) = self.taut_cache.borrow().get(&key) {
            return t.clone();
        }
        let t = Rc::new(self.taut_raw(st, w));
        self.taut_cache.borrow_mut().insert(key, t.clone());
        t
    }

    /// The crossing directions of every pass of every wire (empty for dead wires), indexed like
    /// [`State::wires`] and the pass lists. O(passes) when cached.
    pub fn taut_dirs(&self, st: &State) -> Vec<Vec<Vec2>> {
        (0..st.wires.len()).map(|w| if st.wires[w].alive { self.taut(st, w).dir.clone() } else { Vec::new() }).collect()
    }

    fn taut_raw(&self, st: &State, w: WireId) -> Taut {
        let pr = self.pr;
        let wi = &st.wires[w];
        let n = wi.passes.len();
        let (pa, pb) = (pr.pos[wi.a], pr.pos[wi.b]);
        let mut ports: Vec<(Vec2, Vec2)> = Vec::with_capacity(n + wi.turns.len() + 2);
        ports.push((pa, pa));
        let mut pass_port = Vec::with_capacity(n);
        let mut ti = 0;
        for k in 0..=n {
            while ti < wi.turns.len() && wi.turns[ti].0 == k {
                let v = pr.pos[wi.turns[ti].1];
                ports.push((v, v));
                ti += 1;
            }
            if k == n {
                break;
            }
            let e = st.passes[wi.passes[k]].edge;
            let [p, q] = pr.edges[e];
            let (pp, pq) = (pr.pos[p], pr.pos[q]);
            let len = (pq - pp).len().max(1.0);
            let m0 = self.margin_any(e, 0, wi.net, wi.w, wi.s);
            let m1 = self.margin_any(e, 1, wi.net, wi.w, wi.s);
            let (lo, hi) = (m0, len - m1);
            let (lo, hi) = if lo <= hi { (lo, hi) } else { (0.5 * (lo + hi), 0.5 * (lo + hi)) };
            let (lo, hi) = (lo.clamp(0.0, len), hi.clamp(0.0, len));
            let (a, b) = (pp + (pq - pp) * (lo / len), pp + (pq - pp) * (hi / len));
            // Left and right as seen coming from the triangle of traversal `k`.
            let t = pr.tris[wi.tris[k]];
            let c = (pr.pos[t[0]] + pr.pos[t[1]] + pr.pos[t[2]]) * (1.0 / 3.0);
            let left_is_q = (pq - pp).cross(c - pp) > 0.0;
            pass_port.push(ports.len());
            ports.push(if left_is_q { (b, a) } else { (a, b) });
        }
        ports.push((pb, pb));
        let path = funnel(&ports);
        let unit = |d: Vec2| d.unit().unwrap_or_default();
        let mut at = Vec::with_capacity(n);
        let mut dir = Vec::with_capacity(n);
        let mut j = 0usize;
        for (k, &i) in pass_port.iter().enumerate() {
            while j + 1 < path.len() && path[j + 1].1 <= i {
                j += 1;
            }
            // `path[j]` is the last corner at or before portal `i`.
            let (l, r) = ports[i];
            let on = |c: Vec2| seg_dist(c, l, r) < 1.0;
            let corner = if path[j].1 == i || on(path[j].0) {
                Some(j)
            } else if j + 1 < path.len() && on(path[j + 1].0) {
                Some(j + 1)
            } else {
                None
            };
            match corner {
                Some(c) if c > 0 && c + 1 < path.len() => {
                    // The path wraps the end vertex `v` of the edge at this pass: wires packed
                    // around it follow arcs about `v`, which cross the edge (radial from `v`)
                    // square when the edge lies in the wrapped wedge (`d_in . u >= 0 >= d_out .
                    // u`, `u` the edge direction away from `v`); outside the wedge the edge cuts
                    // the straight part before (or after) the bend.
                    let (d_in, d_out) = (unit(path[c].0 - path[c - 1].0), unit(path[c + 1].0 - path[c].0));
                    let [p, q] = pr.edges[st.passes[wi.passes[k]].edge];
                    let (pp, pq) = (pr.pos[p], pr.pos[q]);
                    let x = path[c].0;
                    let u = if (x - pp).len2() <= (x - pq).len2() { unit(pq - pp) } else { unit(pp - pq) };
                    at.push(x);
                    dir.push(if d_in.dot(u) < 0.0 {
                        d_in
                    } else if d_out.dot(u) > 0.0 {
                        d_out
                    } else {
                        u.perp()
                    });
                }
                Some(c) => {
                    // A pass at a terminal: the direction of the path's first / last segment.
                    let (a, b) = if c == 0 { (path[0].0, path[1.min(path.len() - 1)].0) } else { (path[c - 1].0, path[c].0) };
                    at.push(path[c].0);
                    dir.push(b - a);
                }
                None => {
                    let (c0, c1) = (path[j].0, path[(j + 1).min(path.len() - 1)].0);
                    // Where the segment c0 -> c1 meets the portal line.
                    let (d, pd) = (c1 - c0, r - l);
                    let den = d.cross(pd);
                    let x = if den.abs() < 1e-9 { l + pd * 0.5 } else { l + pd * ((c0 - l).cross(d) / -den).clamp(0.0, 1.0) };
                    at.push(x);
                    dir.push(d);
                }
            }
        }
        Taut { at, dir }
    }

    fn margin_raw(&self, e: EdgeId, end: usize, net: i32, w: f64, s: f64, layer: usize) -> f64 {
        let pr = self.pr;
        let v = pr.edges[e][end];
        let u = pr.edges[e][1 - end];
        let (pv, pu) = (pr.pos[v], pr.pos[u]);
        let len = (pu - pv).len();
        match pr.vkind[v] {
            VertKind::Outline(o) => {
                let ob = &pr.obstacles[o];
                if ob.net == Some(net) || !(ob.drilled || ob.layers.has(layer)) {
                    return 0.0;
                }
                let req = self.clear_to(ob, e, s) + 0.5 * w + self.eps;
                let at = |d: f64| pv + (pu - pv) * (d / len.max(1.0));
                // Whether or not the far end is closer than required as well (the edge is then
                // too narrow), the bisection finds the first distance that keeps the clearance.
                let (mut lo, mut hi) = (0.0, len);
                for _ in 0..30 {
                    let mid = 0.5 * (lo + hi);
                    if poly_dist(at(mid), &ob.poly) >= req {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                hi
            }
            VertKind::Edge => {
                let req = self.edge_clear + 0.5 * w + self.eps;
                let at = |d: f64| pv + (pu - pv) * (d / len.max(1.0));
                let dist = |p: Vec2| self.outline.iter().map(|&(a, b)| seg_dist(p, a, b)).fold(f64::INFINITY, f64::min);
                // The outline is not convex: take the last sample that is too close.
                let steps = 24;
                let mut m = 0.0;
                for i in 0..=steps {
                    let d = len * i as f64 / steps as f64;
                    if dist(at(d)) < req {
                        m = len * (i + 1).min(steps) as f64 / steps as f64;
                    }
                }
                m
            }
            _ => 0.0,
        }
    }

    /// The packing numbers of `items` (in edge order) on `layer` of edge `e`.
    pub fn bounds(&self, e: EdgeId, layer: usize, items: &[PackItem]) -> Bounds {
        let pr = self.pr;
        let [p, q] = pr.edges[e];
        let len = (pr.pos[q] - pr.pos[p]).len().max(1.0);
        let regime = pr.regime(e);
        let s_of = |it: &PackItem| regime.unwrap_or(it.s);
        let n = items.len();
        let lo: Vec<f64> = items.iter().map(|i| self.margin(e, 0, i.net, i.w, i.s, layer)).collect();
        let hi: Vec<f64> = items.iter().map(|i| len - self.margin(e, 1, i.net, i.w, i.s, layer)).collect();
        let req: Vec<f64> = items
            .windows(2)
            .map(|x| {
                let (a, b) = (&x[0], &x[1]);
                (0.5 * (a.w + b.w) + if a.net != b.net { s_of(a).max(s_of(b)) + self.eps } else { 0.0 }) * a.infl.max(b.infl)
            })
            .collect();
        let mut lmin = vec![0.0; n];
        let mut rmax = vec![0.0; n];
        for i in 0..n {
            lmin[i] = if i == 0 { lo[0] } else { lo[i].max(lmin[i - 1] + req[i - 1]) };
        }
        for i in (0..n).rev() {
            rmax[i] = if i == n - 1 { hi[i] } else { hi[i].min(rmax[i + 1] - req[i]) };
        }
        Bounds { lo, hi, lmin, rmax, req }
    }
}
