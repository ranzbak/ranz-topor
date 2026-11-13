//! The G0 projection of a [`BoardModel`]: pads (polygons from their skeletons, drilled pads walls
//! for other nets), the board outline, Steiner points in open areas (so gap midpoints are a fair
//! length estimate), and the pad-to-pad connections (MST per net on pad centres).
//!
//! The router's projection (`core`, shopping list 4.2) is built once per call from the fixed
//! items and has no Steiner grid unless `[search] steiner` is on (`BuildOpts::steiner_mm` = 0);
//! the R0 gate and the witnesses keep the default grid.
//!
//! Complexity: O(pads · hull) for the polygons, O(grid · obstacles) for the Steiner points,
//! O(k²) per net for the MST.

use super::cuts;
use super::init::Conn;
use super::state::{point_in_poly, point_in_segments, LayerMask, ObsId, ObsKind, Obstacle, Projection, VertId, VertKind};
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
    /// 4.4 (planes as terminals): the place where a wire of `net` ends on its own pour, a square
    /// of side `size` at `at` on the model layers `layers` where the pour's fill covers it. It
    /// gets a centre vertex (a terminal like a pad) but is no copper: the wire's track end in the
    /// fill joins the pour when KiCad refills it.
    Terminal { net: i32, at: Vec2, layers: Vec<u32>, size: f64 },
    /// Shopping list 5.10 (`[search] pad_exit_stubs`): the planned exit of a connection from the pad
    /// `pad` (index into `BoardModel::pads`) towards the point `toward` (its other terminal), for
    /// a wire of width `w` and clearance `s` (nm). The build turns it into an exit stub
    /// ([`exit_stub`]): the rectangle the wire's copper takes from the pad's tip along the pad's
    /// long axis to where it may turn without coming nearer than the clearance to the pads beside
    /// it, an obstacle of the pad's net. Only for an elongated SMD pad with another net's copper
    /// beside it, on the long side facing `toward`, closer than the wire could pass, and only where the rectangle keeps every
    /// clearance; else nothing (the wire leaves the pad as before).
    Exit { pad: usize, toward: Vec2, w: f64, s: f64 },
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
    /// Report lines of the build: the triangulation needed its noded path, and the constraints
    /// (obstacle sides, cuts) it left out, with their owners and places ([`cdt_notes`]).
    pub notes: Vec<String>,
    /// 5.10: the exit stubs of the build (`Extra::Exit`): `(obstacle, pad index)`.
    pub exit_stubs: Vec<(ObsId, usize)>,
}

/// Where an obstacle of the projection comes from (for the report).
#[derive(Clone, Copy, Debug)]
enum Src {
    Pad(usize),
    Via(usize),
    Keepout(usize),
    Shape(usize),
    Extra(usize),
    /// 5.10: the exit stub of a pad (index into `BoardModel::pads`).
    Exit(usize),
}

/// A point in the KiCad frame (y down), mm.
fn kicad_mm(p: Vec2) -> String {
    format!("({:.4}, {:.4}) mm", p.x / MM, -p.y / MM)
}

fn net_name(m: &BoardModel, net: i32) -> String {
    m.nets.iter().find(|n| n.code == net).map_or_else(|| if net > 0 { format!("net {net}") } else { "no net".into() }, |n| n.name.clone())
}

fn layer_name(m: &BoardModel, id: u32) -> String {
    m.layers.iter().find(|l| l.id == id).map_or_else(|| format!("layer {id}"), |l| l.name.clone())
}

/// The copper (or outline) a projection vertex belongs to, in words.
fn owner_text(m: &BoardModel, src: &[Src], k: VertKind) -> String {
    let o = match k {
        VertKind::Outline(o) | VertKind::Centre(o) => o,
        VertKind::Edge => return "the board outline (Edge.Cuts)".into(),
        VertKind::Free | VertKind::Node => return "a free point".into(),
    };
    match src.get(o) {
        Some(&Src::Pad(i)) => {
            let p = &m.pads[i];
            format!("pad {}.{} [{}] at {}", p.ref_, p.number, net_name(m, p.net), kicad_mm(p.center.to_vec2()))
        }
        Some(&Src::Via(i)) => {
            let v = &m.vias[i];
            format!("via [{}] at {}{}", net_name(m, v.net), kicad_mm(v.at.to_vec2()), if v.locked { " (locked)" } else { "" })
        }
        Some(&Src::Keepout(i)) => format!("rule area '{}'", m.keepouts[i].name),
        Some(&Src::Shape(i)) => {
            let c = &m.copper_shapes[i];
            let fp = c.owner_ref.as_deref().map_or(String::new(), |r| format!(" of footprint {r}"));
            format!("copper graphic{fp} on {} [{}] (uuid {})", layer_name(m, c.layer), net_name(m, c.net), c.uuid)
        }
        Some(&Src::Extra(i)) => format!("plane stub {i} (a via or track placed before routing)"),
        Some(&Src::Exit(i)) => {
            let p = &m.pads[i];
            format!("planned exit of pad {}.{} [{}]", p.ref_, p.number, net_name(m, p.net))
        }
        None => format!("obstacle {o}"),
    }
}

/// Input constraint `k` of the projection in words: an obstacle side or the board edge between
/// two points, or an LCT cut joining two owners.
fn constraint_text(m: &BoardModel, src: &[Src], points: &[(Vec2, VertKind)], cons: &[(usize, usize)], k: usize) -> String {
    let (a, b) = cons[k];
    let ((pa, ka), (pb, kb)) = (points[a], points[b]);
    let (oa, ob) = (owner_text(m, src, ka), owner_text(m, src, kb));
    if oa == ob {
        format!("a side of {oa} from {} to {}", kicad_mm(pa), kicad_mm(pb))
    } else {
        format!("a cut from {oa} to {ob}, {} to {}", kicad_mm(pa), kicad_mm(pb))
    }
}

/// Distance between segments `a b` and `c d` (0 when they cross). O(1).
fn seg_seg_dist(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> f64 {
    let o = |p: Vec2, q: Vec2, r: Vec2| (q - p).cross(r - p);
    let (d1, d2, d3, d4) = (o(a, b, c), o(a, b, d), o(c, d, a), o(c, d, b));
    if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
        return 0.0;
    }
    seg_dist(a, c, d).min(seg_dist(b, c, d)).min(seg_dist(c, a, b)).min(seg_dist(d, a, b))
}

/// The constraints closest to constraint `k` (other owners, within 1 µm), nearest first: the
/// partners of a degenerate crossing. O(constraints).
fn near_constraints(points: &[(Vec2, VertKind)], cons: &[(usize, usize)], k: usize) -> Vec<usize> {
    let (a, b) = (points[cons[k].0].0, points[cons[k].1].0);
    let mut near: Vec<(f64, usize)> = cons
        .iter()
        .enumerate()
        .filter(|&(j, _)| j != k)
        .map(|(j, &(c, d))| (seg_seg_dist(a, b, points[c].0, points[d].0), j))
        .filter(|&(dist, j)| {
            let (c, d) = (points[cons[j].0].0, points[cons[j].1].0);
            dist <= 1000.0 && ![c, d].iter().any(|p| *p == a || *p == b)
        })
        .collect();
    near.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
    near.into_iter().take(4).map(|x| x.1).collect()
}

/// The report lines of a triangulation that needed more than spade's own splitting.
fn cdt_notes(m: &BoardModel, src: &[Src], points: &[(Vec2, VertKind)], cons: &[(usize, usize)], r: &crate::cdt::CdtReport) -> Vec<String> {
    use crate::cdt::CdtPath;
    if r.path == CdtPath::Direct {
        return Vec::new();
    }
    let mut out = Vec::new();
    let at = r.panic_at.map(|k| {
        let near: Vec<String> = near_constraints(points, cons, k).iter().map(|&j| constraint_text(m, src, points, cons, j)).collect();
        let with = if near.is_empty() { String::new() } else { format!("; nearest copper: {}", near.join("; ")) };
        format!(" at {}{with}", constraint_text(m, src, points, cons, k))
    });
    let counts = if r.path == CdtPath::Noded {
        format!(" ({} crossings noded, {} points merged on the {} nm grid)", r.crossings, r.merged, crate::cdt::NODE_GRID_NM)
    } else {
        format!(" (the noded path failed: {})", r.noded_error.as_deref().unwrap_or("?"))
    };
    out.push(format!(
        "projection: the triangulation's own splitting of crossing constraints failed ({}){}; built by the {} path instead{counts}",
        r.panic.as_deref().unwrap_or("skipped"),
        at.unwrap_or_default(),
        r.path.name(),
    ));
    const MAX: usize = 10;
    for &k in r.dropped.iter().take(MAX) {
        out.push(format!("projection: left out {} (it still crossed other copper after the noding); the drawing checks the exact copper there", constraint_text(m, src, points, cons, k)));
    }
    if r.dropped.len() > MAX {
        out.push(format!("projection: {} more constraint(s) left out", r.dropped.len() - MAX));
    }
    out
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

/// Distance between segment `a`-`b` and the polygon `poly` (0 when an end lies inside). O(|poly|).
fn seg_poly_dist(a: Vec2, b: Vec2, poly: &[Vec2]) -> f64 {
    if point_in_poly(a, poly) || point_in_poly(b, poly) {
        return 0.0;
    }
    let k = poly.len();
    (0..k).map(|i| super::draw::seg_seg_dist(a, b, poly[i], poly[(i + 1) % k])).fold(f64::INFINITY, f64::min)
}

/// Distance between two polygons (0 when a vertex of one lies inside the other). O(|p| |q|).
fn poly_poly_dist(p: &[Vec2], q: &[Vec2]) -> f64 {
    if p.iter().any(|&x| point_in_poly(x, q)) || q.iter().any(|&x| point_in_poly(x, p)) {
        return 0.0;
    }
    let k = p.len();
    (0..k).map(|i| seg_poly_dist(p[i], p[(i + 1) % k], q)).fold(f64::INFINITY, f64::min)
}

/// Shopping list 5.10: the exit stub of the pad obstacle `oi` for a wire of width `w` and
/// clearance `s` (nm) leaving towards `toward`, as `(rectangle, tip)`, `tip` = whether it leaves
/// along `+axis` (the pad's principal axis, a deterministic sign).
///
/// TopoR (6.3 manual p. 71, "Don't stretch wire to polygonal pad origin"; p. 56, the minimum width
/// "near pads that are narrower than the wire") and Maley (1987 p. 29: terminals are points)
/// both put a wire's end at a point of its pad; where the pads stand closer than a wire can pass
/// between them (a fine-pitch row), the only legal way out is through the pad's tip along its
/// long axis, until the wire's copper may turn without nearing the neighbour pads' tips closer
/// than the clearance: `s + width` past the tip (the octilinear drawing's pad exit leg,
/// `octi::Exit::reach`, plus the copper's half width). That stretch is fixed before any topology
/// is planned (as the plane stubs are, D6a): the planner routes the other nets round it, the
/// clearance cuts between its end and the neighbour pads are cuts of the projection (and of the
/// cut model's layer sheets: `[capacity] model = "cuts"` prices them), and the drawing keeps
/// other copper off it. Its width is the wire's, at most the pad's own width (the neck rule: the
/// pad's width first). The rectangle starts where its whole width has left the pad's polygon, so
/// the two do not overlap.
///
/// `None` (no stub, the exit is left to the plan as before) when the pad is no elongated SMD pad
/// (long side under 1.2 x the short side, drilled, or on several layers), when no copper of
/// another net on its layer stands closer to it than `w + 2 x clearance` (the wire could leave
/// through a side), or when the rectangle would come nearer than the clearance to any other
/// net's copper, a no-tracks rule area or the board edge (`edge_clear`).
/// O(obstacles x polygon sizes) per call.
#[allow(clippy::too_many_arguments)]
fn exit_stub(obstacles: &[Obstacle], src: &[Src], oi: usize, toward: Vec2, w: f64, s: f64, outline: &[(Vec2, Vec2)], edge_clear: f64) -> Option<(Vec<Vec2>, bool)> {
    let ob = &obstacles[oi];
    let net = ob.net?;
    if ob.drilled || ob.layers.0.count_ones() != 1 || ob.poly.len() < 3 || w <= 0.0 {
        return None;
    }
    let c = ob.center;
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for &p in &ob.poly {
        let d = p - c;
        sxx += d.x * d.x;
        syy += d.y * d.y;
        sxy += d.x * d.y;
    }
    let th = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let u0 = Vec2 { x: th.cos(), y: th.sin() };
    // A deterministic sign of the axis (its larger component positive).
    let u0 = if u0.x.abs() >= u0.y.abs() { if u0.x < 0.0 { -u0 } else { u0 } } else if u0.y < 0.0 { -u0 } else { u0 };
    let ext = |v: Vec2| ob.poly.iter().map(|&p| (p - c).dot(v)).fold(f64::MIN, f64::max);
    let (lu, lv) = (ext(u0) + ext(-u0), ext(u0.perp()) + ext(-u0.perp()));
    if lu < 1.2 * lv {
        return None;
    }
    let plus = (toward - c).dot(u0) >= 0.0;
    let u = if plus { u0 } else { -u0 };
    let ws = w.min(lv);
    let on = |o: &Obstacle| o.drilled || o.layers.0 & ob.layers.0 != 0;
    let keepout = |j: usize| matches!(src.get(j), Some(Src::Keepout(_)));
    let need = |j: usize, o: &Obstacle| if keepout(j) { o.clear } else { o.clear.max(s) };
    let bb = |poly: &[Vec2]| {
        poly.iter().fold((Vec2 { x: f64::INFINITY, y: f64::INFINITY }, Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY }), |(lo, hi), p| {
            (Vec2 { x: lo.x.min(p.x), y: lo.y.min(p.y) }, Vec2 { x: hi.x.max(p.x), y: hi.y.max(p.y) })
        })
    };
    let near = |lo: Vec2, hi: Vec2, poly: &[Vec2], d: f64| {
        let (a, b) = bb(poly);
        !(a.x > hi.x + d || b.x < lo.x - d || a.y > hi.y + d || b.y < lo.y - d)
    };
    // Fine pitch: another net's copper beside the pad closer than the wire could pass. Where the
    // straight line to the target leaves the pad decides which: through a tip zone (the last
    // `lv / 2` of the pad's length, its rounded ends) a neighbour on either side, through a long
    // side only one on that side. A pad whose side towards the target is free (the end pin of a
    // row) needs no planned exit: its wire may leave through that side (fanout_inner_beyond U1.5:
    // the stub on its tip took the room of U1.6's wire, which the wire of U1.5, leaving through the
    // free side at 45 degrees, never needed).
    let v = u0.perp();
    let (du, dv) = ((toward - c).dot(u0), (toward - c).dot(v));
    let tx = (0.5 * lu / du.abs().max(1e-9)).min(0.5 * lv / dv.abs().max(1e-9));
    let tip_zone = (du * tx).abs() >= 0.5 * (lu - lv) - 1.0;
    let side = if dv >= 0.0 { 1.0 } else { -1.0 };
    let (plo, phi) = bb(&ob.poly);
    let fine = obstacles.iter().enumerate().any(|(j, o)| {
        if j == oi || o.net == Some(net) || !on(o) || keepout(j) || (!tip_zone && side * (o.center - c).dot(v) <= 0.5 * lv) {
            return false;
        }
        let gap = ws + 2.0 * need(j, o);
        near(plo, phi, &o.poly, gap) && poly_poly_dist(&ob.poly, &o.poly) - o.sag < gap
    });
    if !fine {
        return None;
    }
    // From where the whole width has left the pad to `s + width` past its tip.
    let nrm = u.perp() * (0.5 * ws);
    let inside = |q: Vec2| point_in_poly(q, &ob.poly);
    let tip = ext(u);
    let step = 0.005 * MM;
    let mut t = 0.0;
    while t < tip + ws && (inside(c + u * t) || inside(c + u * t + nrm) || inside(c + u * t - nrm)) {
        t += step;
    }
    let t1 = tip + s + ws;
    if t1 - t <= step {
        return None;
    }
    let (a, b) = (c + u * t, c + u * t1);
    // Legal: the clearance to every other net's copper, rule area and the board edge.
    let (slo, shi) = bb(&[a, b]);
    for (j, o) in obstacles.iter().enumerate() {
        if j == oi || o.net == Some(net) || !on(o) {
            continue;
        }
        let req = need(j, o) + 0.5 * ws + o.sag;
        if near(slo, shi, &o.poly, req) && seg_poly_dist(a, b, &o.poly) < req - 1.0 {
            return None;
        }
    }
    if outline.iter().any(|&(p, q)| super::draw::seg_seg_dist(a, b, p, q) < edge_clear + 0.5 * ws - 1.0) {
        return None;
    }
    Some((vec![a - nrm, b - nrm, b + nrm, a + nrm], plus))
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

/// A failed board build: the error and the copper where it happened (owners and places in
/// words, KiCad frame, mm), for the plugin's fail-fast probe.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildFailure {
    pub error: String,
    /// The constraint the triangulation failed on, then the nearest other copper.
    pub places: Vec<String>,
}

impl BuildFailure {
    /// One line: the error, then the places.
    pub fn text(&self) -> String {
        match self.places.split_first() {
            None => self.error.clone(),
            Some((at, [])) => format!("{} at {at}", self.error),
            Some((at, near)) => format!("{} at {at}; nearest copper: {}", self.error, near.join("; ")),
        }
    }
}

/// [`build`] with the failure's places ([`BuildFailure`]).
pub fn build_checked(m: &BoardModel, o: &BuildOpts) -> Result<Board, BuildFailure> {
    build_full_checked(m, o, &[], &[])
}

/// [`build_with_nodes`] with the extra copper `extra` as obstacles (after the pads, existing
/// vias, keepouts and copper shapes, so their obstacle ids do not move).
pub fn build_full(m: &BoardModel, o: &BuildOpts, nodes: &[Vec2], extra: &[Extra]) -> Result<Board, String> {
    build_full_checked(m, o, nodes, extra).map_err(|f| f.text())
}

/// [`build_full`] with the failure's places.
pub fn build_full_checked(m: &BoardModel, o: &BuildOpts, nodes: &[Vec2], extra: &[Extra]) -> Result<Board, BuildFailure> {
    let mut signal_layers: Vec<u32> = m.layers.iter().filter(|l| l.routable).map(|l| l.id).collect();
    let n_signal = signal_layers.len();
    if o.plane_layers {
        signal_layers.extend(m.layers.iter().filter(|l| !l.routable).map(|l| l.id));
    }
    let n = signal_layers.len();
    let mut obstacles = Vec::new();
    let mut pad_of = Vec::new();
    let mut src: Vec<Src> = Vec::new();
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
        src.push(Src::Pad(pi));
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
        for (vi, v) in m.vias.iter().enumerate() {
            src.push(Src::Via(vi));
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
        for (ki, k) in m.keepouts.iter().enumerate().filter(|(_, k)| k.no_tracks && k.poly.len() >= 3) {
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
            src.push(Src::Keepout(ki));
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
        for (si, sh) in m.copper_shapes.iter().enumerate().filter(|(_, c)| c.poly.len() >= 3) {
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
            src.push(Src::Shape(si));
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
    let mut term_obs: Vec<usize> = Vec::new();
    for (xi, x) in extra.iter().enumerate() {
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
            Extra::Terminal { net, at, ref layers, size } => {
                let mut mask = LayerMask(0);
                for (li, id) in signal_layers.iter().enumerate() {
                    if layers.contains(id) {
                        mask.0 |= 1 << li;
                    }
                }
                if mask.0 == 0 {
                    continue;
                }
                let h = 0.5 * size;
                let poly = vec![at + Vec2 { x: -h, y: -h }, at + Vec2 { x: h, y: -h }, at + Vec2 { x: h, y: h }, at + Vec2 { x: -h, y: h }];
                term_obs.push(obstacles.len());
                (poly, 0.0, mask, false, net, (m.rules.net_rule(net).clearance).max(m.rules.min_clearance) as f64)
            }
            Extra::Exit { .. } => continue,
        };
        if poly.len() < 3 {
            continue;
        }
        let c = poly.iter().fold(Vec2 { x: 0.0, y: 0.0 }, |acc, &p| acc + p) * (1.0 / poly.len() as f64);
        pad_of.push(usize::MAX);
        src.push(Src::Extra(xi));
        obstacles.push(Obstacle { kind: ObsKind::Pad, poly, center: c, layers, drilled, net: (net > 0).then_some(net), clear, bare: false, sag });
    }
    // 5.10: the exit stubs, after every other obstacle (each keeps its clearance to all of them
    // and to the stubs placed before it); one per pad and tip.
    let mut exit_stubs: Vec<(ObsId, usize)> = Vec::new();
    if extra.iter().any(|x| matches!(x, Extra::Exit { .. })) {
        let osegs = outline_segments(m);
        let obs_of: BTreeMap<usize, usize> = pad_of.iter().enumerate().filter(|(_, &p)| p != usize::MAX).map(|(o, &p)| (p, o)).collect();
        let mut done: BTreeSet<(usize, bool)> = BTreeSet::new();
        for x in extra {
            let Extra::Exit { pad, toward, w, s } = *x else { continue };
            let Some(&oi) = obs_of.get(&pad) else { continue };
            let Some((poly, plus)) = exit_stub(&obstacles, &src, oi, toward, w, s, &osegs, m.rules.edge_clearance as f64) else { continue };
            if !done.insert((pad, plus)) {
                continue;
            }
            let ob = &obstacles[oi];
            let net = ob.net.unwrap_or(0);
            let c = poly.iter().fold(Vec2 { x: 0.0, y: 0.0 }, |acc, &p| acc + p) * (1.0 / poly.len() as f64);
            let clear = (m.rules.net_rule(net).clearance).max(m.rules.min_clearance) as f64;
            let layers = ob.layers;
            if std::env::var_os("TOPO_EXITDBG").is_some() {
                let (lo, hi) = poly.iter().fold(((f64::MAX, f64::MAX), (f64::MIN, f64::MIN)), |(l, h), p| ((l.0.min(p.x), l.1.min(-p.y)), (h.0.max(p.x), h.1.max(-p.y))));
                eprintln!("EXITSTUB pad {}.{} net {net} x {:.3}..{:.3} y {:.3}..{:.3}", m.pads[pad].ref_, m.pads[pad].number, lo.0 / 1e6, hi.0 / 1e6, lo.1 / 1e6, hi.1 / 1e6);
            }
            exit_stubs.push((obstacles.len(), pad));
            pad_of.push(usize::MAX);
            src.push(Src::Exit(pad));
            obstacles.push(Obstacle { kind: ObsKind::Pad, poly, center: c, layers, drilled: false, net: (net > 0).then_some(net), clear, bare: false, sag: 0.0 });
        }
    }
    // Centres first, so a coincident vertex keeps the terminal kind.
    let mut points: Vec<(Vec2, VertKind)> = Vec::new();
    let mut cons = Vec::new();
    for (i, ob) in obstacles.iter().enumerate().take(n_centred) {
        if ob.net.is_some() {
            points.push((ob.center, VertKind::Centre(i)));
        }
    }
    // 4.4: a pour terminal is a terminal like a pad.
    for &i in &term_obs {
        points.push((obstacles[i].center, VertKind::Centre(i)));
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
    let feet = cuts::lct_feet(&verts, &sides, &polys, &mask_of, o.lct_mm * MM, o.lct_eps_mm * MM);
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
    // Q14a: a rule area's corner (or foot) on the board outline is the board edge for copper: it
    // bounds every layer with the copper-to-edge clearance. As the rule area's own vertex (the
    // first kind of a coincident point wins) a cut from a pad to it had no room on the layers
    // the rule area leaves open, so the planner sent wires between a pad and the edge where the
    // edge clearance leaves no room (planned_crossing_window, double_cross_bait: B.Cu closed by a
    // rule area along the outline). TopoR 6.3 manual (`docs/literature/eremex2017-topor63-user-
    // manual.pdf` p. 57, «Board Edge Clearance»): one clearance "between wires and the board
    // edge", whatever else covers the edge. Copper obstacles on the outline (castellated pads)
    // keep their own kind (pad exits).
    if !segs.is_empty() {
        let keepout: Vec<bool> = src.iter().map(|s| matches!(s, Src::Keepout(_))).collect();
        for p in points.iter_mut() {
            if let VertKind::Outline(ob) = p.1 {
                if keepout.get(ob).copied().unwrap_or(false) && segs.iter().any(|&(a, b)| seg_dist(p.0, a, b) < 1.0) {
                    p.1 = VertKind::Edge;
                }
            }
        }
    }
    let mut proj = match Projection::build_reported(n, obstacles, &points, &cons, inside) {
        Ok(p) => p,
        Err(f) => {
            let (error, r) = *f;
            // Name the copper where the triangulation failed (the plugin's probe shows it).
            let mut places = Vec::new();
            if let Some(k) = r.panic_at {
                places.push(constraint_text(m, &src, &points, &cons, k));
                places.extend(near_constraints(&points, &cons, k).iter().map(|&j| constraint_text(m, &src, &points, &cons, j)));
            }
            return Err(BuildFailure { error, places });
        }
    };
    let notes = cdt_notes(m, &src, &points, &cons, &proj.cdt);
    proj.edge_clear = m.rules.edge_clear() as f64;
    // A copper-to-edge check set to ignore is no hard bound in the topology: the cuts at the
    // outline bound with 0 and P2 prices a pass that fits only without the rule (ignored_rule).
    if m.rules.ignored_checks.iter().any(|c| c == "copper_edge_clearance") {
        proj.edge_clear_ignored = proj.edge_clear;
        proj.edge_clear = 0.0;
    }
    // Keepouts forbidding vias, on whatever copper layer (a through via spans them all): no
    // via site may touch one (the P3 site test, the drawing's checker).
    proj.no_via = m.keepouts.iter().filter(|k| k.no_vias && k.poly.len() >= 3).map(|k| k.poly.iter().map(|p| p.to_vec2()).collect()).collect();
    // The projection is one CDT over every layer: a region bound to some layers only is left to
    // the drawing and its checker (which see layers).
    proj.set_regions(m.rules.clearance_regions.iter().filter(|r| r.layers.is_empty()).map(|r| (r.poly.iter().map(|p| p.to_vec2()).collect(), r.clearance as f64)).collect());
    let mut centre = vec![None; proj.obstacles.len()];
    for (v, k) in proj.vkind.iter().enumerate() {
        if let VertKind::Centre(o) = *k {
            centre[o] = Some(v);
        }
    }
    let poured = m.pours.iter().map(|p| p.net).collect();
    Ok(Board { proj, signal_layers, n_signal, centre, pad_of, poured, notes, exit_stubs })
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
