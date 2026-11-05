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
//! Complexity: a margin is a 30-step bisection over the end obstacle's polygon, cached per
//! (edge, end, net, width, clearance, layer); a fit test is O(passes).

use super::draw::{poly_dist, seg_dist};
use super::state::{EdgeId, Obstacle, Projection, VertKind};
use crate::geom::Vec2;
use std::cell::RefCell;
use std::collections::HashMap;

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

/// Margin cache key: edge, end, net, width bits, clearance bits, layer.
type MarginKey = (usize, u8, i32, u64, u64, u8);

pub struct Packer<'a> {
    pr: &'a Projection,
    outline: Vec<(Vec2, Vec2)>,
    /// Extra distance on top of every clearance (nm).
    eps: f64,
    /// Copper-to-edge clearance kept (nm; 0 when that check is ignored).
    edge_clear: f64,
    cache: RefCell<HashMap<MarginKey, f64>>,
}

/// The oblique-chord factor of the pass of a wire whose polyline is `pl` at traversal `k`
/// (the pass point is `pl[k + 1]`) through edge `e`: `1 / sin` of the crossing angle, 1 .. 2.5.
pub fn infl_of(pr: &Projection, e: EdgeId, pl: &[Vec2], k: usize) -> f64 {
    let [p, q] = pr.edges[e];
    let (pp, pq) = (pr.pos[p], pr.pos[q]);
    let ed = (pq - pp) * (1.0 / (pq - pp).len().max(1.0));
    let d = pl[k + 2] - pl[k];
    let dl = d.len();
    if dl < 1.0 {
        1.0
    } else {
        1.0 / (d.cross(ed).abs() / dl).clamp(0.4, 1.0)
    }
}

impl<'a> Packer<'a> {
    pub fn new(pr: &'a Projection, outline: Vec<(Vec2, Vec2)>, eps: f64, edge_clear: f64) -> Self {
        Packer { pr, outline, eps, edge_clear, cache: RefCell::new(HashMap::new()) }
    }

    /// The packer of `model` over the projection `pr`, with the drawing's tolerance and edge rule.
    pub fn for_model(model: &crate::model::BoardModel, opts: &crate::model::Options, pr: &'a Projection) -> Self {
        let edge_clear = if model.rules.ignored_checks.iter().any(|c| c == "copper_edge_clearance") { 0.0 } else { pr.edge_clear.max(pr.edge_clear_ignored) };
        Packer::new(pr, super::board::outline_segments(model), (opts.margin_nm + 2_000) as f64, edge_clear)
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
