//! The G0 projection of a [`BoardModel`]: pads (polygons from their skeletons, drilled pads walls
//! for other nets), the board outline, Steiner points in open areas (so gap midpoints are a fair
//! length estimate), and the pad-to-pad connections (MST per net on pad centres).
//!
//! Complexity: O(pads · hull) for the polygons, O(grid · obstacles) for the Steiner points,
//! O(k²) per net for the MST.

use super::init::Conn;
use super::state::{point_in_segments, LayerMask, ObsId, ObsKind, Obstacle, Projection, VertId, VertKind};
use crate::geom::quant::TrackPrim;
use crate::geom::{Arc, Skeleton, Vec2};
use crate::model::{BoardModel, PadAttr};
use std::collections::{BTreeMap, BTreeSet};

const MM: f64 = 1e6;

#[derive(Clone, Copy, Debug)]
pub struct BuildOpts {
    /// Steiner grid pitch (mm); 0 = none.
    pub steiner_mm: f64,
    /// Minimum distance of a Steiner point from any obstacle's bounding box (mm).
    pub steiner_clear_mm: f64,
    /// Obstacle vertices closer than this to the outline (mm) split it at their foot points.
    pub edge_feet_mm: f64,
}

impl Default for BuildOpts {
    fn default() -> Self {
        BuildOpts { steiner_mm: 2.0, steiner_clear_mm: 0.6, edge_feet_mm: 3.0 }
    }
}

#[derive(Clone, Debug)]
pub struct Board {
    pub proj: Projection,
    /// Model layer ids of the signal layers, by layer index.
    pub signal_layers: Vec<u32>,
    /// Centre vertex of each pad obstacle with a net.
    pub centre: Vec<Option<VertId>>,
    /// Index into `BoardModel::pads` of each obstacle.
    pub pad_of: Vec<usize>,
    /// Nets with a copper pour (connected by the pour, not wired in G0).
    pub poured: BTreeSet<i32>,
}

/// Convex hull, counter-clockwise (monotone chain).
fn hull(mut p: Vec<Vec2>) -> Vec<Vec2> {
    p.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let mut h: Vec<Vec2> = Vec::new();
    for pass in 0..2 {
        let start = h.len();
        let pts: Vec<Vec2> = if pass == 0 { p.clone() } else { p.iter().rev().copied().collect() };
        for q in pts {
            while h.len() >= start + 2 && (h[h.len() - 1] - h[h.len() - 2]).cross(q - h[h.len() - 2]) <= 0.0 {
                h.pop();
            }
            h.push(q);
        }
        h.pop();
    }
    h
}

/// Polygon containing the skeleton (radius enlarged so the octagon holds the circle).
fn skel_poly(sk: &Skeleton) -> Vec<Vec2> {
    let (base, r) = match sk {
        Skeleton::Point { p, r } => (vec![*p], *r),
        Skeleton::Seg { s, r } => (vec![s.a, s.b], *r),
        Skeleton::Poly { pts, r } => (pts.clone(), *r),
    };
    if r <= 0.0 {
        return hull(base);
    }
    let eighth = std::f64::consts::PI / 8.0;
    let rr = r / eighth.cos();
    let oct: Vec<Vec2> = (0..8)
        .map(|i| {
            let a = eighth + i as f64 * 2.0 * eighth;
            Vec2 { x: rr * a.cos(), y: rr * a.sin() }
        })
        .collect();
    hull(base.iter().flat_map(|&b| oct.iter().map(move |&o| b + o)).collect())
}

/// The board outline as segments (arcs split to 0.05 mm sagitta).
pub fn outline_segments(m: &BoardModel) -> Vec<(Vec2, Vec2)> {
    let mut segs = Vec::new();
    for p in &m.outline {
        match *p {
            TrackPrim::Seg { a, b } => segs.push((a.to_f64(), b.to_f64())),
            TrackPrim::Arc { start, mid, end } => {
                match Arc::from_three_points(start.to_f64(), mid.to_f64(), end.to_f64()) {
                    Some(a) => segs.extend(a.split_sagitta(0.05 * MM).iter().map(|s| (s.start(), s.end()))),
                    None => segs.push((start.to_f64(), end.to_f64())),
                }
            }
        }
    }
    segs
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.len2();
    let t = if l2 > 0.0 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + d * t)).len()
}

fn bbox(pts: &[Vec2]) -> (Vec2, Vec2) {
    let (mut lo, mut hi) = (pts[0], pts[0]);
    for p in pts {
        lo = Vec2 { x: lo.x.min(p.x), y: lo.y.min(p.y) };
        hi = Vec2 { x: hi.x.max(p.x), y: hi.y.max(p.y) };
    }
    (lo, hi)
}

pub fn build(m: &BoardModel, o: &BuildOpts) -> Result<Board, String> {
    let signal_layers: Vec<u32> = m.layers.iter().filter(|l| l.routable).map(|l| l.id).collect();
    let n = signal_layers.len();
    let mut obstacles = Vec::new();
    let mut pad_of = Vec::new();
    for (pi, pad) in m.pads.iter().enumerate() {
        let mut mask = LayerMask(0);
        for (li, id) in signal_layers.iter().enumerate() {
            if pad.layers.contains(id) {
                mask.0 |= 1 << li;
            }
        }
        let drilled = pad.drill.is_some() || matches!(pad.attr, PadAttr::Pth | PadAttr::Npth);
        if mask.0 == 0 && !drilled {
            continue;
        }
        let c = pad.center.to_vec2();
        let poly = match signal_layers.iter().find(|id| pad.layers.contains(id)) {
            Some(&l) => skel_poly(&m.skeleton(pad, l)),
            None => {
                let d = pad.drill.map(|d| d.w.max(d.h)).unwrap_or(0) as f64;
                skel_poly(&Skeleton::Point { p: c, r: 0.5 * d })
            }
        };
        if poly.len() < 3 {
            continue;
        }
        pad_of.push(pi);
        obstacles.push(Obstacle {
            kind: if pad.attr == PadAttr::Npth { ObsKind::Drill } else { ObsKind::Pad },
            poly,
            center: c,
            layers: if drilled { LayerMask::all(n) } else { mask },
            drilled,
            net: (pad.net > 0).then_some(pad.net),
        });
    }
    // Centres first, so a coincident vertex keeps the terminal kind.
    let mut points: Vec<(Vec2, VertKind)> = Vec::new();
    let mut cons = Vec::new();
    for (i, ob) in obstacles.iter().enumerate() {
        if ob.net.is_some() {
            points.push((ob.center, VertKind::Centre(i)));
        }
    }
    for (i, ob) in obstacles.iter().enumerate() {
        let s = points.len();
        let k = ob.poly.len();
        points.extend(ob.poly.iter().map(|&p| (p, VertKind::Outline(i))));
        cons.extend((0..k).map(|j| (s + j, s + (j + 1) % k)));
    }
    // Split the outline at the feet of obstacle vertices near it, so every pad-to-edge gap is a
    // CDT edge (a cut whose room is checked).
    let mut segs = Vec::new();
    for (a, b) in outline_segments(m) {
        let d = b - a;
        let l2 = d.len2();
        let mut ts: Vec<f64> = obstacles
            .iter()
            .flat_map(|ob| ob.poly.iter())
            .filter(|&&q| seg_dist(q, a, b) < o.edge_feet_mm * MM)
            .map(|&q| if l2 > 0.0 { (q - a).dot(d) / l2 } else { 0.0 })
            .filter(|&t| t > 0.0 && t < 1.0)
            .collect();
        ts.sort_by(f64::total_cmp);
        let min_dt = 0.05 * MM / l2.sqrt().max(1.0);
        let mut prev = a;
        let mut last_t = 0.0;
        for t in ts {
            if t - last_t >= min_dt && 1.0 - t >= min_dt {
                let q = a + d * t;
                segs.push((prev, q));
                prev = q;
                last_t = t;
            }
        }
        segs.push((prev, b));
    }
    for &(a, b) in &segs {
        let s = points.len();
        points.push((a, VertKind::Edge));
        points.push((b, VertKind::Edge));
        cons.push((s, s + 1));
    }
    let inside = |p: Vec2| segs.is_empty() || point_in_segments(p, &segs);
    if o.steiner_mm > 0.0 && !points.is_empty() {
        let all: Vec<Vec2> = points.iter().map(|p| p.0).collect();
        let (lo, hi) = bbox(&all);
        let clear = o.steiner_clear_mm * MM;
        let boxes: Vec<(Vec2, Vec2)> = obstacles.iter().map(|ob| bbox(&ob.poly)).collect();
        let step = o.steiner_mm * MM;
        let mut y = lo.y + 0.5 * step;
        while y < hi.y {
            let mut x = lo.x + 0.5 * step;
            while x < hi.x {
                let p = Vec2 { x, y };
                let near = boxes.iter().any(|&(a, b)| {
                    p.x > a.x - clear && p.x < b.x + clear && p.y > a.y - clear && p.y < b.y + clear
                });
                if !near && inside(p) && !segs.iter().any(|&(a, b)| seg_dist(p, a, b) < clear) {
                    points.push((p, VertKind::Free));
                }
                x += step;
            }
            y += step;
        }
    }
    let proj = Projection::build(n, obstacles, &points, &cons, inside)?;
    let mut centre = vec![None; proj.obstacles.len()];
    for (v, k) in proj.vkind.iter().enumerate() {
        if let VertKind::Centre(o) = *k {
            centre[o] = Some(v);
        }
    }
    let poured = m.pours.iter().map(|p| p.net).collect();
    Ok(Board { proj, signal_layers, centre, pad_of, poured })
}

impl Board {
    /// The layer of a single-layer (SMD) pad obstacle.
    pub fn smd_layer(&self, o: ObsId) -> Option<usize> {
        let ob = &self.proj.obstacles[o];
        if ob.drilled {
            None
        } else {
            ob.layers.single()
        }
    }

    /// Pad-to-pad connections: per net (not poured, ≥ 2 pads) the MST on pad centres.
    pub fn connections(&self, m: &BoardModel) -> Vec<Conn> {
        let mut by_net: BTreeMap<i32, Vec<ObsId>> = BTreeMap::new();
        for (o, ob) in self.proj.obstacles.iter().enumerate() {
            if let (Some(net), Some(_)) = (ob.net, self.centre[o]) {
                if !self.poured.contains(&net) {
                    by_net.entry(net).or_default().push(o);
                }
            }
        }
        let mut out = Vec::new();
        for (&net, obs) in &by_net {
            let k = obs.len();
            if k < 2 {
                continue;
            }
            let r = m.rules.net_rule(net);
            let c = |o: ObsId| self.proj.obstacles[o].center;
            // Prim's MST.
            let mut inn = vec![false; k];
            let mut dist = vec![f64::INFINITY; k];
            let mut from = vec![0usize; k];
            dist[0] = 0.0;
            for _ in 0..k {
                let u = (0..k)
                    .filter(|&i| !inn[i])
                    .min_by(|&i, &j| dist[i].total_cmp(&dist[j]).then(i.cmp(&j)))
                    .expect("node left");
                inn[u] = true;
                if u != 0 {
                    let (pa, pb) = (obs[from[u]], obs[u]);
                    out.push(Conn {
                        net,
                        pa,
                        pb,
                        a: self.centre[pa].expect("centre"),
                        b: self.centre[pb].expect("centre"),
                        w: r.width as f64,
                        s: r.clearance as f64,
                    });
                }
                for v in 0..k {
                    let d = (c(obs[v]) - c(obs[u])).len();
                    if !inn[v] && d < dist[v] {
                        dist[v] = d;
                        from[v] = u;
                    }
                }
            }
        }
        out
    }
}
