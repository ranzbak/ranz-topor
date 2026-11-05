//! The G0 projection of a [`BoardModel`]: pads (polygons from their skeletons, drilled pads walls
//! for other nets), the board outline, Steiner points in open areas (so gap midpoints are a fair
//! length estimate), and the pad-to-pad connections (MST per net on pad centres).
//!
//! Complexity: O(pads · hull) for the polygons, O(grid · obstacles) for the Steiner points,
//! O(k²) per net for the MST.

use super::cuts;
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
    /// LCT cuts (see [`super::cuts`]): a vertex facing an obstacle or outline side closer than
    /// this (mm) gets a cut edge to its foot on that side; 0 = none.
    pub lct_mm: f64,
    /// Feet closer than this (mm) to an end of their side or to another foot are skipped.
    pub lct_eps_mm: f64,
    /// Vertices per full turn of a round pad / rounded corner (see [`skel_poly_sag`]); 16 keeps every
    /// polygon within 1.9 % of the radius of the copper, 8 halves the triangles (7.6 %).
    pub round_steps: usize,
    /// The vias of the model are drilled obstacles (copper the board keeps; S2a). The gate and
    /// witness inputs have none.
    pub existing_vias: bool,
    /// Rule areas that forbid tracks are obstacles without a net on their layers (S2b): a wire
    /// may not enter them, so P1 closes those layers there. Off for the gate and witness inputs.
    pub keepouts: bool,
    /// The non-routable copper layers (planes) follow the routable ones as extra projection
    /// layers, so signal copper can be priced onto them (S2b, `[planes] signals = "costly"`).
    pub plane_layers: bool,
    /// Copper graphics (`BoardModel::copper_shapes`: gr_* shapes and footprint graphics on a
    /// copper layer) are obstacles on their layer, with their net (D6a): a wire of another net
    /// keeps the clearance to them in the topology, not only in the drawing.
    pub copper_shapes: bool,
}

impl Default for BuildOpts {
    fn default() -> Self {
        BuildOpts { steiner_mm: 2.0, steiner_clear_mm: 0.6, lct_mm: 3.0, lct_eps_mm: 0.01, round_steps: 16, existing_vias: false, keepouts: false, plane_layers: false, copper_shapes: false }
    }
}

/// Copper placed before the topology that the projection must see (D6a: the plane stubs and
/// their vias). A via is drilled (a wall on every layer for other nets); a track is a
/// rectangle `a`-`b` of `width` on model layer `layer` (the caller trims it to the part outside
/// the copper it joins, so the polygons do not overlap). Neither gets a centre vertex: it is no
/// terminal and no pad left to fan out.
#[derive(Clone, Debug)]
pub enum Extra {
    /// `ring`: extra clearance (nm) other nets keep, so the pour reaches the via (a plane via:
    /// the zone's minimum width; KiCad's connection width check otherwise flags the neck).
    Via { net: i32, at: Vec2, d: f64, ring: f64 },
    Track { net: i32, layer: u32, a: Vec2, b: Vec2, width: f64 },
}

#[derive(Clone, Debug)]
pub struct Board {
    pub proj: Projection,
    /// Model layer ids of the projection layers by layer index: the routable (signal) layers
    /// first, then the plane layers when [`BuildOpts::plane_layers`] is set.
    pub signal_layers: Vec<u32>,
    /// How many of `signal_layers` are routable signal layers (the rest are planes).
    pub n_signal: usize,
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

/// Vertices on the circle of a round copper shape (`BuildOpts::round_steps` per full turn, default
/// 16), so the polygon lies inside the true copper (sagitta `r (1 - cos(pi/steps))`, 1.9 % of the
/// radius) and every CDT cut ends on real copper, not on an inflated hull.
/// Polygon of the pad copper: the hull of the skeleton's base points, each replaced by a 16-gon
/// whose vertices lie on the circle of the skeleton radius (a rectangle stays exact, a rounded
/// corner or a circle is the 16-gon).
/// Returns the polygon and its sagitta (how far the true copper bulges past it, `r (1 -
/// cos(pi / steps))` for a skeleton of radius `r`, 0 for a rectangle).
fn skel_poly_sag(sk: &Skeleton, steps: usize) -> (Vec<Vec2>, f64) {
    let (base, r) = match sk {
        Skeleton::Point { p, r } => (vec![*p], *r),
        Skeleton::Seg { s, r } => (vec![s.a, s.b], *r),
        Skeleton::Poly { pts, r } => (pts.clone(), *r),
    };
    if r <= 0.0 {
        return (hull(base), 0.0);
    }
    let ring: Vec<Vec2> = (0..steps)
        .map(|i| {
            let a = i as f64 * 2.0 * std::f64::consts::PI / steps as f64;
            Vec2 { x: r * a.cos(), y: r * a.sin() }
        })
        .collect();
    (hull(base.iter().flat_map(|&b| ring.iter().map(move |&o| b + o)).collect()), r * (1.0 - (std::f64::consts::PI / steps as f64).cos()))
}

/// An NPTH pad whose copper shape lies within its drill: a bare hole without copper.
fn bare_hole(pad: &crate::model::Pad) -> bool {
    use crate::model::PadShape;
    let Some(d) = pad.drill.filter(|_| pad.attr == PadAttr::Npth) else { return false };
    let (dw, dh) = (d.w, d.h);
    pad.shapes.values().all(|sh| match *sh {
        PadShape::Circle { d: c } => c <= dw.max(dh) + 1000,
        PadShape::Rect { w, h } | PadShape::RoundRect { w, h, .. } | PadShape::Oval { w, h } => w <= dw + 1000 && h <= dh + 1000,
        PadShape::Poly { .. } => false,
    })
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
    build_with_nodes(m, o, &[])
}

/// [`build`] with extra vertices `nodes` (nm) inserted into the triangulation (kind
/// `VertKind::Node`): the points inside triangles of the State, see the `state` module doc.
pub fn build_with_nodes(m: &BoardModel, o: &BuildOpts, nodes: &[Vec2]) -> Result<Board, String> {
    build_full(m, o, nodes, &[])
}

/// [`build_with_nodes`] with the extra copper `extra` as obstacles (after the pads, existing
/// vias, keepouts and copper shapes, so their obstacle ids do not move).
pub fn build_full(m: &BoardModel, o: &BuildOpts, nodes: &[Vec2], extra: &[Extra]) -> Result<Board, String> {
    let mut signal_layers: Vec<u32> = m.layers.iter().filter(|l| l.routable).map(|l| l.id).collect();
    let n_signal = signal_layers.len();
    if o.plane_layers {
        signal_layers.extend(m.layers.iter().filter(|l| !l.routable).map(|l| l.id));
    }
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
        let (poly, sag) = match signal_layers.iter().find(|id| pad.layers.contains(id)) {
            Some(&l) => skel_poly_sag(&m.skeleton(pad, l), o.round_steps.max(4)),
            None => {
                let d = pad.drill.map(|d| d.w.max(d.h)).unwrap_or(0) as f64;
                skel_poly_sag(&Skeleton::Point { p: c, r: 0.5 * d }, o.round_steps.max(4))
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
            // `required_clearance` of a pad with the item's own net clearance left out (the
            // demand model takes the larger of the two).
            bare: bare_hole(pad),
            clear: if bare_hole(pad) {
                // No copper, only a hole: KiCad charges the hole clearance, not the net's.
                (pad.local_clearance.unwrap_or(0)).max(m.rules.hole_clearance) as f64
            } else {
                (m.rules.net_rule(pad.net).clearance)
                    .max(pad.local_clearance.unwrap_or(0))
                    .max(m.rules.min_clearance)
                    .max(m.npth_hole_clearance(pad)) as f64
            },
            sag,
        });
    }
    if o.existing_vias {
        for v in &m.vias {
            let c = v.at.to_vec2();
            let (poly, sag) = skel_poly_sag(&Skeleton::Point { p: c, r: 0.5 * v.d as f64 }, o.round_steps.max(4));
            // `pad_of` stays index-aligned with the obstacles; `usize::MAX` marks a via.
            pad_of.push(usize::MAX);
            obstacles.push(Obstacle {
                kind: ObsKind::Pad,
                poly,
                center: c,
                layers: LayerMask::all(n),
                drilled: true,
                net: (v.net > 0).then_some(v.net),
                clear: (m.rules.net_rule(v.net).clearance).max(m.rules.min_clearance).max(m.rules.hole_clearance) as f64,
                bare: false,
                sag,
            });
        }
    }
    if o.keepouts {
        for k in m.keepouts.iter().filter(|k| k.no_tracks && k.poly.len() >= 3) {
            let mut mask = LayerMask(0);
            for (li, id) in signal_layers.iter().enumerate() {
                if k.layers.contains(id) {
                    mask.0 |= 1 << li;
                }
            }
            if mask.0 == 0 {
                continue;
            }
            let mut poly: Vec<Vec2> = k.poly.iter().map(|p| p.to_vec2()).collect();
            poly.dedup();
            let area: f64 = (0..poly.len()).map(|i| poly[i].cross(poly[(i + 1) % poly.len()])).sum();
            if area < 0.0 {
                poly.reverse();
            }
            let c = poly.iter().fold(Vec2 { x: 0.0, y: 0.0 }, |a, &p| a + p) * (1.0 / poly.len() as f64);
            pad_of.push(usize::MAX);
            obstacles.push(Obstacle {
                kind: ObsKind::Pad,
                poly,
                center: c,
                layers: mask,
                drilled: false,
                net: None,
                clear: 0.0,
                bare: false,
                sag: 0.0,
            });
        }
    }
    // Pads, existing vias and keepouts may carry a centre vertex; copper shapes and the extra
    // copper are no terminals.
    let n_centred = obstacles.len();
    if o.copper_shapes {
        for sh in m.copper_shapes.iter().filter(|c| c.poly.len() >= 3) {
            let Some(li) = signal_layers.iter().position(|&id| id == sh.layer) else { continue };
            let mut poly: Vec<Vec2> = sh.poly.iter().map(|p| p.to_vec2()).collect();
            poly.dedup();
            if poly.len() >= 2 && poly[0] == poly[poly.len() - 1] {
                poly.pop();
            }
            if poly.len() < 3 {
                continue;
            }
            let area: f64 = (0..poly.len()).map(|i| poly[i].cross(poly[(i + 1) % poly.len()])).sum();
            if area < 0.0 {
                poly.reverse();
            }
            let c = poly.iter().fold(Vec2 { x: 0.0, y: 0.0 }, |a, &p| a + p) * (1.0 / poly.len() as f64);
            pad_of.push(usize::MAX);
            obstacles.push(Obstacle {
                kind: ObsKind::Pad,
                poly,
                center: c,
                layers: LayerMask(1 << li),
                drilled: false,
                net: (sh.net > 0).then_some(sh.net),
                // The checker charges a shape the larger of its net's clearance and the other
                // item's; the demand model takes the larger of `clear` and the wire's.
                clear: (m.rules.net_rule(sh.net).clearance).max(m.rules.min_clearance) as f64,
                bare: false,
                sag: 0.0,
            });
        }
    }
    for x in extra {
        let (poly, sag, layers, drilled, net, clear) = match *x {
            Extra::Via { net, at, d, ring } => {
                let (poly, sag) = skel_poly_sag(&Skeleton::Point { p: at, r: 0.5 * d }, o.round_steps.max(4));
                (poly, sag, LayerMask::all(n), true, net, (m.rules.net_rule(net).clearance).max(m.rules.min_clearance).max(m.rules.hole_clearance) as f64 + ring)
            }
            Extra::Track { net, layer, a, b, width } => {
                let Some(li) = signal_layers.iter().position(|&id| id == layer) else { continue };
                let u = b - a;
                let l = u.len();
                if l <= 0.0 {
                    continue;
                }
                let nrm = Vec2 { x: -u.y / l, y: u.x / l } * (0.5 * width);
                (vec![a - nrm, b - nrm, b + nrm, a + nrm], 0.0, LayerMask(1 << li), false, net, (m.rules.net_rule(net).clearance).max(m.rules.min_clearance) as f64)
            }
        };
        if poly.len() < 3 {
            continue;
        }
        let c = poly.iter().fold(Vec2 { x: 0.0, y: 0.0 }, |acc, &p| acc + p) * (1.0 / poly.len() as f64);
        pad_of.push(usize::MAX);
        obstacles.push(Obstacle { kind: ObsKind::Pad, poly, center: c, layers, drilled, net: (net > 0).then_some(net), clear, bare: false, sag });
    }
    // Centres first, so a coincident vertex keeps the terminal kind.
    let mut points: Vec<(Vec2, VertKind)> = Vec::new();
    let mut cons = Vec::new();
    for (i, ob) in obstacles.iter().enumerate().take(n_centred) {
        if ob.net.is_some() {
            points.push((ob.center, VertKind::Centre(i)));
        }
    }
    // LCT cuts (see `cuts`): the feet of obstacle / outline vertices on facing sides, so that the
    // nearest copper of every sector is a vertex. Sides: obstacle polygon sides, then the outline.
    let osegs = outline_segments(m);
    let mut verts: Vec<cuts::Vertex> = Vec::new();
    let mut sides: Vec<cuts::Side> = Vec::new();
    let mut side0 = Vec::with_capacity(obstacles.len());
    for (i, ob) in obstacles.iter().enumerate() {
        side0.push(sides.len());
        let k = ob.poly.len();
        for j in 0..k {
            verts.push(cuts::Vertex { p: ob.poly[j], owner: Some(i) });
            sides.push(cuts::Side { a: ob.poly[j], b: ob.poly[(j + 1) % k], owner: Some(i) });
        }
    }
    let out0 = sides.len();
    for &(a, b) in &osegs {
        verts.push(cuts::Vertex { p: a, owner: None });
        verts.push(cuts::Vertex { p: b, owner: None });
        sides.push(cuts::Side { a, b, owner: None });
    }
    let polys: Vec<&[Vec2]> = obstacles.iter().map(|ob| &ob.poly[..]).collect();
    let mask_of = |o: Option<usize>| o.map_or(u64::MAX, |i| if obstacles[i].drilled { u64::MAX } else { obstacles[i].layers.0 });
    let feet = cuts::lct_feet(&verts, &sides, &polys, &|a, b| mask_of(a) & mask_of(b) != 0, o.lct_mm * MM, o.lct_eps_mm * MM);
    // Feet per side, in order along it.
    let mut on_side: Vec<Vec<(f64, Vec2)>> = vec![Vec::new(); sides.len()];
    for f in feet.iter().filter(|f| f.t > 0.0 && f.t < 1.0) {
        on_side[f.side].push((f.t, f.at));
    }
    for v in &mut on_side {
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    for (i, ob) in obstacles.iter().enumerate() {
        let k = ob.poly.len();
        for j in 0..k {
            let mut chain = vec![ob.poly[j]];
            chain.extend(on_side[side0[i] + j].iter().map(|x| x.1));
            chain.push(ob.poly[(j + 1) % k]);
            let s = points.len();
            points.extend(chain.iter().map(|&p| (p, VertKind::Outline(i))));
            cons.extend((0..chain.len() - 1).map(|q| (s + q, s + q + 1)));
        }
    }
    let mut segs = Vec::new();
    for (si, &(a, b)) in osegs.iter().enumerate() {
        let mut prev = a;
        for &(_, q) in &on_side[out0 + si] {
            segs.push((prev, q));
            prev = q;
        }
        segs.push((prev, b));
    }
    for &(a, b) in &segs {
        let s = points.len();
        points.push((a, VertKind::Edge));
        points.push((b, VertKind::Edge));
        cons.push((s, s + 1));
    }
    for f in &feet {
        let (v, sd) = (&verts[f.v], &sides[f.side]);
        let kind = |o: Option<usize>| o.map_or(VertKind::Edge, VertKind::Outline);
        let s = points.len();
        points.push((v.p, kind(v.owner)));
        points.push((f.at, kind(sd.owner)));
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
    points.extend(nodes.iter().map(|&p| (p, VertKind::Node)));
    let mut proj = Projection::build(n, obstacles, &points, &cons, inside)?;
    proj.edge_clear = m.rules.edge_clearance as f64;
    // A copper-to-edge check set to ignore is no hard bound in the topology: the cuts at the
    // outline bound with 0 and P2 prices a pass that fits only without the rule (ignored_rule).
    if m.rules.ignored_checks.iter().any(|c| c == "copper_edge_clearance") {
        proj.edge_clear_ignored = proj.edge_clear;
        proj.edge_clear = 0.0;
    }
    // Keepouts forbidding vias, on whatever copper layer (a through via spans them all): no
    // via site may touch one (the P3 site test, the drawing's checker).
    proj.no_via = m.keepouts.iter().filter(|k| k.no_vias && k.poly.len() >= 3).map(|k| k.poly.iter().map(|p| p.to_vec2()).collect()).collect();
    proj.set_regions(m.rules.clearance_regions.iter().map(|r| (r.poly.iter().map(|p| p.to_vec2()).collect(), r.clearance as f64)).collect());
    let mut centre = vec![None; proj.obstacles.len()];
    for (v, k) in proj.vkind.iter().enumerate() {
        if let VertKind::Centre(o) = *k {
            centre[o] = Some(v);
        }
    }
    let poured = m.pours.iter().map(|p| p.net).collect();
    Ok(Board { proj, signal_layers, n_signal, centre, pad_of, poured })
}

impl Board {
    /// The layer of a single-layer (SMD) pad obstacle.
    pub fn smd_layer(&self, o: ObsId) -> Option<usize> {
        // A node terminal (`NO_OBS`, a junction) has no pad.
        let ob = self.proj.obstacles.get(o)?;
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
