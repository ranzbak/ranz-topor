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
use super::state::{centre_vertices, point_in_poly, point_in_segments, LayerMask, ObsId, ObsKind, Obstacle, Projection, VertId, VertKind};
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
    /// exist-term: the tracks the board keeps (`BoardModel::kept`, arcs as chords) are obstacles
    /// of their net on their layer (TopoR 6.3 manual p. 110: the fixed state of wires "is taken
    /// into account in all modes"): a wire of another net keeps its clearance to them in the
    /// topology, not only in the drawing; a wire of their own net passes them freely (own copper).
    /// No centre vertex: their ends become terminals through [`Extra::KeptEnd`].
    pub kept_tracks: bool,
    /// exist-term: the extra distance (nm) the lead of a continued dangling end keeps on top of
    /// every clearance it is cut at ([`lead_len`]): the ERBS placement margin of the route
    /// (`margin_nm` + [`super::pack::DRAW_TOL_NM`], 10 nm by default; erbs-margin), so the lead's
    /// end is no zero-slack vertex the drawing's packer cannot leave (r1 verifier F4).
    pub lead_margin_nm: f64,
    /// tw4 T3 (`[search] dangling_lead_choice`): the lead of a continued dangling end offers the
    /// points at 0, 1/3 and 2/3 of its (cut) length as start points too, besides its end
    /// ([`KeptEndOut::choices`]): extra centre vertices of the lead's terminal obstacle, so the
    /// search takes the cheapest (the length from it and the `dangling_turn` price there) and the
    /// drawing draws the lead only up to the chosen one. Off: the lead's end only.
    pub lead_choice: bool,
}

impl Default for BuildOpts {
    fn default() -> Self {
        BuildOpts { steiner_mm: 2.0, steiner_clear_mm: 0.6, lct_mm: 3.0, lct_eps_mm: 0.01, round_steps: 16, existing_vias: false, keepouts: false, plane_layers: false, copper_shapes: false, kept_tracks: false, lead_margin_nm: super::pack::DRAW_TOL_NM, lead_choice: false }
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
    /// exist-term: a terminal on existing copper of `net` (a kept track) at `at` on model layer
    /// `layer`, for a wire of width `w` and clearance `s` (nm): a terminal like a pad (centre
    /// vertex), on the track's copper. Without `lead` it is the disc of radius 0.4 `w` at `at`
    /// (inside the track's copper, centre vertex `at`). With `lead = Some((d, len))` (a dangling
    /// end continued, `[search] dangling_ends = "continue"`) it is the new copper that continues the
    /// end along the unit direction `d`: the stadium `at -> at + d·L` of radius `w / 2`, centre
    /// vertex at `at + d·L`, where `L` is the longest length up to `len` whose stadium keeps the
    /// clearance to every foreign obstacle and the board edge ([`lead_len`]); `L` below
    /// [`LEAD_MIN_NM`] = the disc (a note says so).
    /// `via`: the via sizes the lead's via end tries, in order ([`crate::vias::lead_via_sizes`];
    /// empty = none, `dangling_lead_via` off): see [`KeptEndOut::via_end`] (exist-term r3-r5: a
    /// via the plan puts at the lead end; badge conn 116 forced one 23 um into a pad's clearance).
    KeptEnd { net: i32, layer: u32, at: Vec2, w: f64, s: f64, lead: Option<(Vec2, f64)>, via: Vec<crate::vias::ViaSize> },
}

/// exist-term: a lead shorter than this (nm) is degenerate; the end is a plain disc terminal.
pub const LEAD_MIN_NM: f64 = 10_000.0;

/// tw4 T3 ([`BuildOpts::lead_choice`]): the start points a built lead offers besides its end,
/// `(point, t)` on its axis at `t` = 0 (the hand end itself), 1/3 and 2/3 of its length; an inner
/// point nearer than [`LEAD_MIN_NM`] to the hand end or the lead's end is left out. Empty without
/// a lead. O(1).
pub fn lead_choice_points(k: &KeptEndOut) -> Vec<(Vec2, f64)> {
    let Some((q, d)) = k.lead else { return Vec::new() };
    let l = (q - k.at).len();
    [0.0, l / 3.0, 2.0 * l / 3.0].into_iter().filter(|&t| t == 0.0 || (t >= LEAD_MIN_NM && l - t >= LEAD_MIN_NM)).map(|t| (k.at + d * t, t)).collect()
}

/// exist-term: one terminal on existing copper ([`Extra::KeptEnd`]) as built: its obstacle, the
/// end point `at` on the kept track and, for a lead, the lead's end (the centre vertex) and its
/// direction. `note`: why a requested lead was not built.
#[derive(Clone, Debug)]
pub struct KeptEndOut {
    pub obs: ObsId,
    pub net: i32,
    pub at: Vec2,
    /// The width (nm) of the kept track (the lead's width).
    pub w: f64,
    pub lead: Option<(Vec2, Vec2)>,
    /// r4/r5 (`[search] dangling_lead_via`): for a lead, the point on it where a via fits
    /// ([`lead_len`]) and that via's size: the first of [`crate::vias::lead_via_sizes`] (net class
    /// via first, the tightest Board Setup size last) with room on the lead. The drawing puts that
    /// via there (and ends the lead there) when the plan changes layer at the lead end. None: no
    /// via room on the lead for any size, or the option is off.
    pub via_end: Option<(Vec2, crate::vias::ViaSize)>,
    pub note: Option<String>,
    /// tw4 T3 ([`BuildOpts::lead_choice`]): the start points the lead offers, `(vertex, t)` with
    /// `t` the distance (nm) from `at` along the lead, ascending, the lead's end (the obstacle's
    /// main centre vertex) last. Every one is a centre vertex of `obs`. Empty: the option is off
    /// or the end has no lead (the end vertex is the only start point).
    pub choices: Vec<(VertId, f64)>,
}

impl KeptEndOut {
    /// tw4 T3: the distance (nm) from `at` along the lead of the start point at `p` (one of
    /// [`KeptEndOut::choices`] or the lead's end), within 1 nm; `None` for any other point or
    /// without a lead. Off ([`BuildOpts::lead_choice`]) only the lead's end matches.
    pub fn lead_point_t(&self, proj: &Projection, p: Vec2) -> Option<f64> {
        let (q, _) = self.lead?;
        if (p - q).len() < 1.0 {
            return Some((q - self.at).len());
        }
        self.choices.iter().find(|&&(v, _)| (proj.pos[v] - p).len() < 1.0).map(|&(_, t)| t)
    }
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
    /// 5.10 / TW4 meter: the exits that needed a stub (fine pitch towards the target) but got
    /// none, because the rectangle would break a clearance: `(pad obstacle, pad index)`, once per
    /// pad. The compass predicts each as a `pad_exit`.
    pub exit_blocked: Vec<(ObsId, usize)>,
    /// exist-term: the obstacle of each existing via (index into `BoardModel::vias`).
    pub via_obs: BTreeMap<usize, ObsId>,
    /// exist-term: the terminals on existing copper (`Extra::KeptEnd`), in `extra` order.
    pub kept_ends: Vec<KeptEndOut>,
    /// exist-term: the obstacles of the kept tracks ([`BuildOpts::kept_tracks`]), the last ones
    /// of the projection (the drawing has their exact copper and skips these polygons).
    pub kept_bodies: std::ops::Range<ObsId>,
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
    /// exist-term: a kept track (index into `BoardModel::kept`).
    Kept(usize),
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
        Some(&Src::Kept(i)) => {
            let k = &m.kept[i];
            format!("existing track on {} [{}] (uuid {})", layer_name(m, k.layer), net_name(m, k.net), k.uuid)
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
pub(crate) fn seg_poly_dist(a: Vec2, b: Vec2, poly: &[Vec2]) -> f64 {
    if point_in_poly(a, poly) || point_in_poly(b, poly) {
        return 0.0;
    }
    let k = poly.len();
    (0..k).map(|i| super::draw::seg_seg_dist(a, b, poly[i], poly[(i + 1) % k])).fold(f64::INFINITY, f64::min)
}

/// Distance between two polygons (0 when a vertex of one lies inside the other). O(|p| |q|).
pub(crate) fn poly_poly_dist(p: &[Vec2], q: &[Vec2]) -> f64 {
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
/// `more`: copper that is no obstacle of the build yet but must keep the stub's clearance too (the
/// kept tracks, exist-term r2: they are added after the stubs; none on a board without them).
/// Why [`exit_stub`] gave no stub (TW4 meter: the compass predicts the second as a `pad_exit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StubNo {
    /// No planned exit needed (no elongated SMD pad, or no other net's copper beside it).
    NotNeeded,
    /// Needed (fine pitch towards the target) but the rectangle would break a clearance.
    Illegal,
}

#[allow(clippy::too_many_arguments)]
fn exit_stub(obstacles: &[Obstacle], more: &[Obstacle], src: &[Src], oi: usize, toward: Vec2, w: f64, s: f64, outline: &[(Vec2, Vec2)], edge_clear: f64) -> Result<(Vec<Vec2>, bool), StubNo> {
    let ob = &obstacles[oi];
    let Some(net) = ob.net else { return Err(StubNo::NotNeeded) };
    if ob.drilled || ob.layers.0.count_ones() != 1 || ob.poly.len() < 3 || w <= 0.0 {
        return Err(StubNo::NotNeeded);
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
        return Err(StubNo::NotNeeded);
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
    // Every copper to keep clear of: the build's obstacles, then `more` (index usize::MAX: no
    // source, so never a keepout).
    let each = || obstacles.iter().enumerate().chain(more.iter().map(|o| (usize::MAX, o)));
    let fine = each().any(|(j, o)| {
        if j == oi || o.net == Some(net) || !on(o) || keepout(j) || (!tip_zone && side * (o.center - c).dot(v) <= 0.5 * lv) {
            return false;
        }
        let gap = ws + 2.0 * need(j, o);
        near(plo, phi, &o.poly, gap) && poly_poly_dist(&ob.poly, &o.poly) - o.sag < gap
    });
    if !fine {
        return Err(StubNo::NotNeeded);
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
        return Err(StubNo::Illegal);
    }
    let (a, b) = (c + u * t, c + u * t1);
    // Legal: the clearance to every other net's copper, rule area and the board edge.
    let (slo, shi) = bb(&[a, b]);
    for (j, o) in each() {
        if j == oi || o.net == Some(net) || !on(o) {
            continue;
        }
        let req = need(j, o) + 0.5 * ws + o.sag;
        if near(slo, shi, &o.poly, req) && seg_poly_dist(a, b, &o.poly) < req - 1.0 {
            return Err(StubNo::Illegal);
        }
    }
    if outline.iter().any(|&(p, q)| super::draw::seg_seg_dist(a, b, p, q) < edge_clear + 0.5 * ws - 1.0) {
        return Err(StubNo::Illegal);
    }
    Ok((vec![a - nrm, b - nrm, b + nrm, a + nrm], plus))
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

/// exist-term: the polygon of one chord of a kept track ([`kept_polygons`]).
struct KeptPoly {
    kept: usize,
    poly: Vec<Vec2>,
    center: Vec2,
    sag: f64,
    li: usize,
    net: i32,
    clear: f64,
}

/// exist-term: the kept tracks on projection layers as stadium polygons of radius `width / 2`,
/// one per straight track and per chord of an arc (sagitta 0.02 mm, as the drawing's entities).
/// O(kept tracks x chords).
fn kept_polygons(m: &BoardModel, layers: &[u32], steps: usize) -> Vec<KeptPoly> {
    let mut out = Vec::new();
    for (ki, k) in m.kept.iter().enumerate() {
        let Some(li) = layers.iter().position(|&id| id == k.layer) else { continue };
        let segs: Vec<(Vec2, Vec2)> = match k.prim {
            TrackPrim::Seg { a, b } => vec![(a.to_f64(), b.to_f64())],
            TrackPrim::Arc { start, mid, end } => match Arc::from_three_points(start.to_f64(), mid.to_f64(), end.to_f64()) {
                Some(arc) => arc.split_sagitta(0.02e6).iter().map(|sg| (sg.start(), sg.end())).collect(),
                None => vec![(start.to_f64(), end.to_f64())],
            },
        };
        let clear = (m.rules.net_rule(k.net).clearance).max(m.rules.min_clearance) as f64;
        for (a, b) in segs {
            let (poly, sag) = skel_poly_sag(&Skeleton::Seg { s: crate::geom::Seg { a, b }, r: 0.5 * k.width as f64 }, steps);
            if poly.len() < 3 {
                continue;
            }
            out.push(KeptPoly { kept: ki, poly, center: (a + b) * 0.5, sag, li, net: k.net, clear });
        }
    }
    out
}

/// exist-term: the length (nm, at most `len`) of the lead that continues a dangling end at `at`
/// along the unit direction `d` on projection layer `li`: the longest stadium `at -> at + d·t` of
/// radius `w / 2` that keeps `max(clearance, s)` (+ the polygon's sagitta) to every obstacle of
/// another net on that layer (drilled ones on every layer), to the kept tracks of other nets, and
/// the copper-to-edge clearance `edge` to the outline `osegs`, each plus `margin` (nm, the ERBS
/// placement margin, [`BuildOpts::lead_margin_nm`]); 0 when even the end itself does not. The
/// clearance shrinks monotonically with `t`, so a bisection finds it.
///
/// Second value, with `via` (diameter, drill; 0 = none): the longest `t` at most that length
/// where the via, centred on `at + d·t`, fits: its disc keeps the clearance to every foreign
/// obstacle and kept track on every layer (a through via) and `edge` to the outline, its drill
/// the hole clearance `hole_clear` more; None when it fits nowhere on the lead. The drawing puts
/// the via there when the plan changes layer at the lead end (r4, lead decision: a lead with no
/// via there keeps the track-only length). Exact: per obstacle edge the line meets the capsule of
/// the disc's need in one interval, the polygon's inside adds its own; the answer is the largest
/// of the lead's end, 0 and the bad intervals' starts that no interval covers ([`via_end_on`]). O(obstacle vertices x log).
#[allow(clippy::too_many_arguments)]
fn lead_len(obs: &[Obstacle], kept: &[KeptPoly], osegs: &[(Vec2, Vec2)], edge: f64, at: Vec2, d: Vec2, len: f64, w: f64, s: f64, li: usize, net: i32, margin: f64, via: (f64, f64), hole_clear: f64) -> (f64, Option<f64>) {
    let margin = margin.max(0.0);
    let r = 0.5 * w + margin;
    // the via at the end: its copper disc keeps the clearance, its drill the hole clearance
    let (rv, rd) = if via.0 > 0.0 { (0.5 * via.0 + margin, 0.5 * via.1 + margin + hole_clear) } else { (0.0, 0.0) };
    let reach = len + r.max(rv).max(rd) + s + 1e6;
    let near = |poly: &[Vec2]| poly.iter().any(|p| (*p - at).len() <= reach + poly_span(poly));
    // per obstacle of another net: on the lead's layer?, the stadium's need, and the via disc's
    // need (the via spans every layer: every such obstacle; its drill keeps the hole clearance)
    let foreign: Vec<(&[Vec2], bool, f64, f64)> = obs
        .iter()
        .filter(|ob| ob.net != Some(net) && (rv > 0.0 || ob.drilled || ob.layers.has(li)) && near(&ob.poly))
        .map(|ob| (&ob.poly[..], ob.drilled || ob.layers.has(li), ob.clear.max(s), ob.sag))
        .chain(kept.iter().filter(|k| k.net != net && (rv > 0.0 || k.li == li) && near(&k.poly)).map(|k| (&k.poly[..], k.li == li, k.clear.max(s), k.sag)))
        .map(|(poly, on, c, sag)| (poly, on, r + c + sag, (rv + c).max(rd) + sag))
        .collect();
    // the stadium at -> at + d t: monotone in t (a bisection finds its longest)
    let ok = |t: f64| {
        let q = at + d * t;
        foreign.iter().all(|&(poly, on, need, _)| !on || seg_poly_dist(at, q, poly) >= need) && osegs.iter().all(|&(a, b)| super::draw::seg_seg_dist(at, q, a, b) >= r + edge)
    };
    if std::env::var_os("TOPO_KEPTDBG").is_some() {
        // Debug: the lead's room, the tightest constraints at the full length (slack nm).
        let q = at + d * len;
        let mut v: Vec<(f64, String)> = foreign.iter().map(|&(poly, on, need, end)| ((if on { seg_poly_dist(at, q, poly) - need } else { f64::INFINITY }).min(if rv > 0.0 { seg_poly_dist(q, q, poly) - end } else { f64::INFINITY }), format!("poly {} on {on} need {need:.0} end {end:.0}", kicad_mm(poly[0])))).collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        eprintln!("KEPTLEAD at {} dir ({:.3},{:.3}) len {len:.0} w {w:.0} s {s:.0} margin {margin:.1} via {via:?} tightest {:?}", kicad_mm(at), d.x, d.y, &v[..v.len().min(3)]);
    }
    if !ok(0.0) {
        return (0.0, None);
    }
    let ts = if ok(len) {
        len
    } else {
        let (mut lo, mut hi) = (0.0, len);
        for _ in 0..30 {
            let mid = 0.5 * (lo + hi);
            if ok(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    };
    if rv <= 0.0 {
        return (ts, None);
    }
    let disc: Vec<(&[Vec2], f64)> = foreign.iter().map(|&(poly, _, _, end)| (poly, end)).collect();
    (ts, via_end_on(&disc, osegs, rv + edge, at, d, ts))
}

/// exist-term r4: the largest `t` in `[0, hi]` where a disc centred on `at + d·t` (`d` a unit
/// vector) keeps distance `need` to every polygon of `polys` (`(poly, need)`; 0 inside one) and
/// `need_edge` to every segment of `osegs`; None when no such `t` exists. Exact up to 1e-3 nm:
/// the line meets each edge's capsule of radius `need` in one open interval ([`line_capsule`]);
/// a polygon's inside is bad too, but its boundary lies in the edges' capsules (`need` > 0), so
/// no bad stretch starts inside one. The answer is `hi`, 0 or the start of a capsule interval
/// that no other covers (the covered ones are skipped as a fast path: `holds` would refuse them),
/// checked against the distances themselves (rounding on the boundary is stepped back by at most
/// 0.04 nm). O((polygon vertices + segments) x candidates).
fn via_end_on(polys: &[(&[Vec2], f64)], osegs: &[(Vec2, Vec2)], need_edge: f64, at: Vec2, d: Vec2, hi: f64) -> Option<f64> {
    let mut bad: Vec<(f64, f64)> = Vec::new();
    for &(poly, need) in polys {
        let k = poly.len();
        for i in 0..k {
            bad.extend(line_capsule(at, d, poly[i], poly[(i + 1) % k], need));
        }
    }
    for &(a, b) in osegs {
        bad.extend(line_capsule(at, d, a, b, need_edge));
    }
    let holds = |t: f64| {
        let q = at + d * t;
        polys.iter().all(|&(poly, need)| seg_poly_dist(q, q, poly) >= need) && osegs.iter().all(|&(a, b)| super::draw::seg_seg_dist(q, q, a, b) >= need_edge)
    };
    let mut cands: Vec<f64> = std::iter::once(hi).chain(bad.iter().map(|b| b.0).filter(|&t| (0.0..=hi).contains(&t))).chain(std::iter::once(0.0)).collect();
    cands.sort_by(|a, b| b.total_cmp(a));
    cands.dedup();
    for c in cands {
        if bad.iter().any(|&(lo, up)| lo + 1e-3 < c && c < up - 1e-3) {
            continue;
        }
        if let Some(t) = [0.0, 1e-3, 1e-2, 4e-2].iter().map(|e| c - e).find(|&t| t >= 0.0 && holds(t)) {
            return Some(t);
        }
    }
    None
}

/// The open interval of `t` where `at + d·t` (`d` a unit vector) lies closer than `r` to the
/// segment `a b` (the line through its capsule: convex, so one interval), or None. O(1).
fn line_capsule(at: Vec2, d: Vec2, a: Vec2, b: Vec2, r: f64) -> Option<(f64, f64)> {
    let disc = |c: Vec2| {
        let p = at - c;
        let bq = d.dot(p);
        let h = bq * bq - (p.dot(p) - r * r);
        (h > 0.0).then(|| (-bq - h.sqrt(), -bq + h.sqrt()))
    };
    let mut iv: Vec<(f64, f64)> = [disc(a), disc(b)].into_iter().flatten().collect();
    let u = b - a;
    let l = u.len();
    if l > 0.0 {
        let e = u * (1.0 / l);
        let n = e.perp();
        // |n·(p - a)| < r and 0 < e·(p - a) < l, both linear in t
        let lin = |c0: f64, c1: f64, lo: f64, up: f64| -> Option<(f64, f64)> {
            if c1.abs() < 1e-12 {
                return (lo < c0 && c0 < up).then_some((f64::NEG_INFINITY, f64::INFINITY));
            }
            let (t0, t1) = ((lo - c0) / c1, (up - c0) / c1);
            Some((t0.min(t1), t0.max(t1)))
        };
        if let (Some(x), Some(y)) = (lin(n.dot(at - a), n.dot(d), -r, r), lin(e.dot(at - a), e.dot(d), 0.0, l)) {
            let (lo, up) = (x.0.max(y.0), x.1.min(y.1));
            if lo < up {
                iv.push((lo, up));
            }
        }
    }
    let lo = iv.iter().map(|x| x.0).fold(f64::INFINITY, f64::min);
    let up = iv.iter().map(|x| x.1).fold(f64::NEG_INFINITY, f64::max);
    (lo < up).then_some((lo, up))
}

/// Largest distance between two vertices of `poly` bound (its bounding-box diagonal). O(|poly|).
fn poly_span(poly: &[Vec2]) -> f64 {
    let (lo, hi) = bbox(poly);
    (hi - lo).len()
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
    let mut via_obs: BTreeMap<usize, ObsId> = BTreeMap::new();
    if o.existing_vias {
        for (vi, v) in m.vias.iter().enumerate() {
            via_obs.insert(vi, obstacles.len());
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
    // exist-term: the kept tracks' polygons (added as obstacles after everything else below; the
    // leads of continued dangling ends keep their clearance to the other nets' ones).
    let kept_polys: Vec<KeptPoly> = if o.kept_tracks { kept_polygons(m, &signal_layers, o.round_steps.max(4)) } else { Vec::new() };
    let osegs_lead = if extra.iter().any(|x| matches!(x, Extra::KeptEnd { lead: Some(_), .. })) { outline_segments(m) } else { Vec::new() };
    let mut kept_ends: Vec<KeptEndOut> = Vec::new();
    for (xi, x) in extra.iter().enumerate() {
        if let Extra::KeptEnd { net, layer, at, w, s, lead, ref via } = *x {
            let Some(li) = signal_layers.iter().position(|&id| id == layer) else { continue };
            let own = (m.rules.net_rule(net).clearance).max(m.rules.min_clearance) as f64;
            let mut note = None;
            let mut via_end = None;
            let led = lead.and_then(|(d, len)| {
                // r5: the via end of the first size (net class first, `vias::lead_via_sizes`) that
                // fits somewhere on the lead; the track's length does not depend on the size
                let hc = m.rules.via_hole_clear() as f64;
                let lead_of = |z: Option<&crate::vias::ViaSize>| lead_len(&obstacles, &kept_polys, &osegs_lead, m.rules.edge_clear() as f64, at, d, len, w, s.max(own), li, net, o.lead_margin_nm, z.map_or((0.0, 0.0), |z| (z.d as f64, z.drill as f64)), hc);
                let (l, mut lv) = lead_of(via.first());
                let mut size = via.first().copied().filter(|_| lv.is_some());
                for z in via.iter().skip(1) {
                    if lv.is_some() {
                        break;
                    }
                    lv = lead_of(Some(z)).1;
                    size = lv.map(|_| *z);
                }
                if std::env::var_os("TOPO_KEPTDBG").is_some() {
                    let e = at + d * l;
                    eprintln!("KEPTLEAD net {} at ({:.1}, {:.1}) nm lead {l:.1} of {len:.0} -> end ({:.1}, {:.1}) nm margin {:.1}", net_name(m, net), at.x, -at.y, e.x, -e.y, o.lead_margin_nm);
                }
                if l < LEAD_MIN_NM {
                    note = Some(format!(
                        "dangling end of net {} at {} on {}: no room to continue it along its direction (foreign copper or the board edge within the clearance ahead); the new copper leaves the end at the angle the plan takes (dangling_turn price)",
                        net_name(m, net),
                        kicad_mm(at),
                        layer_name(m, layer)
                    ));
                    None
                } else {
                    // r4: where the via sits when the plan changes layer at the lead end (a via end
                    // below LEAD_MIN_NM: on the hand track's end itself, no lead track)
                    via_end = lv.zip(size).map(|(t, z)| (if t < LEAD_MIN_NM { at } else { at + d * t }, z));
                    Some((at + d * l, d))
                }
            });
            let (poly, sag, centre) = match led {
                Some((q, _)) => {
                    let (p, sg) = skel_poly_sag(&Skeleton::Seg { s: crate::geom::Seg { a: at, b: q }, r: 0.5 * w }, o.round_steps.max(4));
                    (p, sg, q)
                }
                None => (skel_poly_sag(&Skeleton::Point { p: at, r: 0.4 * w }, o.round_steps.max(4)).0, 0.0, at),
            };
            if poly.len() < 3 {
                continue;
            }
            kept_ends.push(KeptEndOut { obs: obstacles.len(), net, at, w, lead: led, via_end, note, choices: Vec::new() });
            term_obs.push(obstacles.len());
            pad_of.push(usize::MAX);
            src.push(Src::Extra(xi));
            obstacles.push(Obstacle { kind: ObsKind::Pad, poly, center: centre, layers: LayerMask(1 << li), drilled: false, net: (net > 0).then_some(net), clear: own, bare: false, sag });
            continue;
        }
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
            Extra::Exit { .. } | Extra::KeptEnd { .. } => continue,
        };
        if poly.len() < 3 {
            continue;
        }
        let c = poly.iter().fold(Vec2 { x: 0.0, y: 0.0 }, |acc, &p| acc + p) * (1.0 / poly.len() as f64);
        pad_of.push(usize::MAX);
        src.push(Src::Extra(xi));
        obstacles.push(Obstacle { kind: ObsKind::Pad, poly, center: c, layers, drilled, net: (net > 0).then_some(net), clear, bare: false, sag });
    }
    // exist-term: the kept tracks as obstacles (pushed last, below); the exit stubs keep their
    // clearance to them too (r2: before, a stub could lie on another net's kept track).
    let kept_obs: Vec<Obstacle> = kept_polys
        .iter()
        .map(|kp| Obstacle { kind: ObsKind::Pad, poly: kp.poly.clone(), center: kp.center, layers: LayerMask(1 << kp.li), drilled: false, net: (kp.net > 0).then_some(kp.net), clear: kp.clear, bare: false, sag: kp.sag })
        .collect();
    // 5.10: the exit stubs, after every other obstacle (each keeps its clearance to all of them
    // and to the stubs placed before it); one per pad and tip.
    let mut exit_stubs: Vec<(ObsId, usize)> = Vec::new();
    let mut exit_blocked: Vec<(ObsId, usize)> = Vec::new();
    if extra.iter().any(|x| matches!(x, Extra::Exit { .. })) {
        let osegs = outline_segments(m);
        let obs_of: BTreeMap<usize, usize> = pad_of.iter().enumerate().filter(|(_, &p)| p != usize::MAX).map(|(o, &p)| (p, o)).collect();
        let mut done: BTreeSet<(usize, bool)> = BTreeSet::new();
        for x in extra {
            let Extra::Exit { pad, toward, w, s } = *x else { continue };
            let Some(&oi) = obs_of.get(&pad) else { continue };
            let (poly, plus) = match exit_stub(&obstacles, &kept_obs, &src, oi, toward, w, s, &osegs, m.rules.edge_clearance as f64) {
                Ok(x) => x,
                Err(StubNo::NotNeeded) => continue,
                Err(StubNo::Illegal) => {
                    if !exit_blocked.contains(&(oi, pad)) {
                        exit_blocked.push((oi, pad));
                    }
                    continue;
                }
            };
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
    // exist-term: the kept tracks, the last obstacles (every id before them stays as it was).
    let kb0 = obstacles.len();
    for (kp, ob) in kept_polys.iter().zip(kept_obs) {
        pad_of.push(usize::MAX);
        src.push(Src::Kept(kp.kept));
        obstacles.push(ob);
    }
    let kept_bodies = kb0..obstacles.len();
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
    // tw4 T3 (`lead_choice`): the lead's other start points (0, 1/3, 2/3 of its length) on its
    // axis, extra centre vertices of its terminal obstacle (after every main centre, so a
    // coincident terminal keeps its own kind and the point is dropped below).
    let lead_pts: Vec<(usize, Vec2, f64)> = if o.lead_choice { kept_ends.iter().enumerate().flat_map(|(ki, k)| lead_choice_points(k).into_iter().map(move |(p, t)| (ki, p, t))).collect() } else { Vec::new() };
    for &(ki, p, _) in &lead_pts {
        points.push((p, VertKind::Centre(kept_ends[ki].obs)));
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
    let mut notes = cdt_notes(m, &src, &points, &cons, &proj.cdt);
    proj.edge_clear = m.rules.edge_clear() as f64;
    // A copper-to-edge check set to ignore is no hard bound in the topology: the cuts at the
    // outline bound with 0 and P2 prices a pass that fits only without the rule (ignored_rule).
    if m.rules.ignored_checks.iter().any(|c| c == "copper_edge_clearance") {
        proj.edge_clear_ignored = proj.edge_clear;
        proj.edge_clear = 0.0;
    }
    // Keepouts forbidding vias, on whatever copper layer (a through via spans them all): no
    // via site may touch one (the P3 site test, the drawing's checker).
    proj.pair_nets = m.rules.per_net.iter().filter(|(_, r)| r.pair_disc).map(|(&n, _)| n).collect();
    proj.no_via = m.keepouts.iter().filter(|k| k.no_vias && k.poly.len() >= 3).map(|k| k.poly.iter().map(|p| p.to_vec2()).collect()).collect();
    // The projection is one CDT over every layer: a region bound to some layers only is left to
    // the drawing and its checker (which see layers).
    proj.set_regions(m.rules.clearance_regions.iter().filter(|r| r.layers.is_empty()).map(|r| (r.poly.iter().map(|p| p.to_vec2()).collect(), r.clearance as f64)).collect());
    let centre = centre_vertices(&proj);
    // tw4 T3: the lead's start points as built (a point the CDT merged into another terminal is
    // dropped), the lead's end (the main centre) last.
    if o.lead_choice {
        for (ki, k) in kept_ends.iter_mut().enumerate() {
            let (Some(vq), Some((q, _))) = (centre[k.obs], k.lead) else { continue };
            let mut ch: Vec<(VertId, f64)> = lead_pts
                .iter()
                .filter(|x| x.0 == ki)
                .filter_map(|&(_, p, t)| proj.vertex_at(p, 1.0).filter(|&v| v != vq && proj.vkind[v] == VertKind::Centre(k.obs)).map(|v| (v, t)))
                .collect();
            if std::env::var_os("TOPO_T3DBG").is_some() {
                for &(_, p, t) in lead_pts.iter().filter(|x| x.0 == ki) {
                    let hit = proj.vertex_at(p, 1.0);
                    let near = (0..proj.pos.len()).filter(|&v| proj.vkind[v] == VertKind::Centre(k.obs)).map(|v| ((proj.pos[v] - p).len(), v)).min_by(|a, b| a.0.total_cmp(&b.0));
                    eprintln!("T3DBG build lead pt t {:.0} nm at ({:.1}, {:.1}) vertex_at {:?} kind {:?} nearest own centre {:?} main {vq} path {:?}", t, p.x, -p.y, hit, hit.map(|v| proj.vkind[v]), near, proj.cdt.path);
                }
            }
            if ch.is_empty() {
                continue;
            }
            ch.push((vq, (q - k.at).len()));
            k.choices = ch;
        }
    }
    let poured = m.pours.iter().map(|p| p.net).collect();
    notes.extend(kept_ends.iter().filter_map(|k| k.note.clone()));
    Ok(Board { proj, signal_layers, n_signal, centre, pad_of, poured, notes, exit_stubs, exit_blocked, via_obs, kept_ends, kept_bodies })
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

#[cfg(test)]
mod tests {
    //! exist-term r2 (verifier F4, F6; DoD 2): the lead of a continued dangling end and the kept
    //! tracks' polygons against exact (slow) references written apart from the code under test.
    use super::*;
    use crate::geom::quant::IVec2;
    use crate::model::KeptItem;

    /// xorshift64: a deterministic fuzz without a crate.
    struct Rng(u64);
    impl Rng {
        fn f(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        fn r(&mut self, a: f64, b: f64) -> f64 {
            a + (b - a) * self.f()
        }
    }

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x, y }
    }

    /// Point to segment distance, the reference's own formula.
    fn pt_seg(p: Vec2, a: Vec2, b: Vec2) -> f64 {
        let d = b - a;
        let l2 = d.dot(d);
        let t = if l2 <= 0.0 { 0.0 } else { ((p - a).dot(d) / l2).clamp(0.0, 1.0) };
        (p - (a + d * t)).len()
    }

    /// Point to convex polygon distance (0 inside; either orientation).
    fn pt_poly(p: Vec2, poly: &[Vec2]) -> f64 {
        let k = poly.len();
        let cr: Vec<f64> = (0..k).map(|i| (poly[(i + 1) % k] - poly[i]).cross(p - poly[i])).collect();
        if cr.iter().all(|&c| c >= 0.0) || cr.iter().all(|&c| c <= 0.0) {
            return 0.0;
        }
        (0..k).map(|i| pt_seg(p, poly[i], poly[(i + 1) % k])).fold(f64::INFINITY, f64::min)
    }

    /// The exact first `t` in `[0, len]` at which the point `at + d t` comes strictly closer than
    /// `need` to the convex set whose distance is `f`: `t -> f(at + d t)` is convex (the distance to
    /// a convex set along a line), so a ternary search finds its minimum and a bisection the
    /// falling side's crossing. `None`: never closer than `need`. 400 halvings: exact in f64.
    fn first_entry(f: &dyn Fn(Vec2) -> f64, at: Vec2, d: Vec2, len: f64, need: f64) -> Option<f64> {
        let g = |t: f64| f(at + d * t);
        if g(0.0) < need {
            return Some(0.0);
        }
        let (mut lo, mut hi) = (0.0, len);
        for _ in 0..200 {
            let (m1, m2) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
            if g(m1) < g(m2) {
                hi = m2;
            } else {
                lo = m1;
            }
        }
        let tm = 0.5 * (lo + hi);
        if g(tm) >= need {
            return None;
        }
        let (mut a, mut b) = (0.0, tm);
        for _ in 0..200 {
            let m = 0.5 * (a + b);
            if g(m) < need {
                b = m;
            } else {
                a = m;
            }
        }
        Some(a)
    }

    fn obs(poly: Vec<Vec2>, net: Option<i32>, li: usize, drilled: bool, clear: f64, sag: f64) -> Obstacle {
        let c = poly.iter().fold(v(0.0, 0.0), |s, &p| s + p) * (1.0 / poly.len() as f64);
        Obstacle { kind: ObsKind::Pad, poly, center: c, layers: LayerMask(1 << li), drilled, net, clear, bare: false, sag }
    }

    /// The lead length the reference expects: the first entry into any foreign obstacle's or
    /// kept track's clearance (+ margin) or the edge band, capped at `len`.
    #[allow(clippy::too_many_arguments)]
    fn lead_ref(ob: &[Obstacle], kept: &[KeptPoly], os: &[(Vec2, Vec2)], edge: f64, at: Vec2, d: Vec2, len: f64, w: f64, s: f64, li: usize, net: i32, margin: f64) -> f64 {
        let r = 0.5 * w + margin;
        let mut t = len;
        for o in ob.iter().filter(|o| o.net != Some(net) && (o.drilled || o.layers.has(li))) {
            if let Some(e) = first_entry(&|p| pt_poly(p, &o.poly), at, d, len, r + o.clear.max(s) + o.sag) {
                t = t.min(e);
            }
        }
        for k in kept.iter().filter(|k| k.net != net && k.li == li) {
            if let Some(e) = first_entry(&|p| pt_poly(p, &k.poly), at, d, len, r + k.clear.max(s) + k.sag) {
                t = t.min(e);
            }
        }
        for &(a, b) in os {
            if let Some(e) = first_entry(&|p| pt_seg(p, a, b), at, d, len, r + edge) {
                t = t.min(e);
            }
        }
        t
    }

    #[test]
    fn lead_len_matches_an_exact_reference_on_random_boards() {
        // 3000 random cases: convex obstacles of the lead's net and of others, on its layer, the
        // other layer, drilled; foreign and own kept tracks; an outline box; margin 0 or 10 nm.
        let mut g = Rng(0x9e37_79b9_7f4a_7c15);
        let (mut n, mut cut, mut full, mut zero, mut worst) = (0, 0, 0, 0, 0.0f64);
        for _ in 0..3000 {
            let mm = 1e6;
            let at = v(g.r(-0.5, 0.5) * mm, g.r(-0.5, 0.5) * mm);
            let a = g.r(0.0, std::f64::consts::TAU);
            let d = v(a.cos(), a.sin());
            let len = g.r(0.0, 2.0) * mm;
            let (w, s) = (g.r(0.1, 0.5) * mm, g.r(0.1, 0.3) * mm);
            let margin = if g.f() < 0.5 { 0.0 } else { 10.0 };
            let mut ob = Vec::new();
            for _ in 0..(1 + (g.f() * 6.0) as usize) {
                let c = v(g.r(-3.0, 3.0) * mm, g.r(-3.0, 3.0) * mm);
                let rad = g.r(0.05, 1.0) * mm;
                let pts: Vec<Vec2> = (0..(3 + (g.f() * 10.0) as usize)).map(|_| c + v(g.r(-rad, rad), g.r(-rad, rad))).collect();
                let poly = hull(pts);
                if poly.len() < 3 {
                    continue;
                }
                let net = match (g.f() * 4.0) as usize {
                    0 => Some(1),
                    1 => None,
                    _ => Some(2),
                };
                let li = if g.f() < 0.7 { 0 } else { 1 };
                let sag = if g.f() < 0.5 { 0.0 } else { g.r(0.0, 5000.0) };
                ob.push(obs(poly, net, li, g.f() < 0.15, g.r(0.1, 0.3) * mm, sag));
            }
            let mut kept = Vec::new();
            for ki in 0..(g.f() * 3.0) as usize {
                let (p, q) = (v(g.r(-3.0, 3.0) * mm, g.r(-3.0, 3.0) * mm), v(g.r(-3.0, 3.0) * mm, g.r(-3.0, 3.0) * mm));
                let (poly, sag) = skel_poly_sag(&Skeleton::Seg { s: crate::geom::Seg { a: p, b: q }, r: g.r(0.05, 0.25) * mm }, 16);
                kept.push(KeptPoly { kept: ki, poly, center: (p + q) * 0.5, sag, li: if g.f() < 0.8 { 0 } else { 1 }, net: if g.f() < 0.3 { 1 } else { 2 }, clear: g.r(0.1, 0.3) * mm });
            }
            let os: Vec<(Vec2, Vec2)> = if g.f() < 0.5 {
                let h = g.r(0.5, 3.0) * mm;
                let c = [v(-h, -h), v(h, -h), v(h, h), v(-h, h)];
                (0..4).map(|i| (c[i], c[(i + 1) % 4])).collect()
            } else {
                Vec::new()
            };
            let edge = g.r(0.0, 0.5) * mm;
            let (got, gv) = lead_len(&ob, &kept, &os, edge, at, d, len, w, s, 0, 1, margin, (0.0, 0.0), 0.0);
            assert_eq!(gv, None, "no via asked: no via end");
            let want = lead_ref(&ob, &kept, &os, edge, at, d, len, w, s, 0, 1, margin);
            let err = (got - want).abs();
            worst = worst.max(err);
            assert!(err <= 0.05, "lead_len {got} vs exact {want} (len {len}, at {at:?}, d {d:?}, w {w}, s {s}, margin {margin})");
            n += 1;
            if want <= 0.0 {
                zero += 1;
            } else if want >= len {
                full += 1;
            } else {
                cut += 1;
            }
        }
        eprintln!("lead_len fuzz: {n} cases, {cut} cut, {full} full length, {zero} no room; worst |error| {worst:.4} nm");
        assert!(cut > 300 && full > 300 && zero > 100, "the fuzz reaches every outcome: cut {cut}, full {full}, zero {zero}");
    }

    /// The convex bad interval `(entry, exit)` of `t` in `[0, hi]` where `f(at + d t) < need`
    /// (entry -1: bad at 0; exit hi + 1: bad at hi), or None: ternary search for the minimum,
    /// bisection for both crossings (the r3 verifier's reference, `d5be9d5`).
    fn bad_interval(f: &dyn Fn(Vec2) -> f64, at: Vec2, d: Vec2, hi: f64, need: f64) -> Option<(f64, f64)> {
        let g = |t: f64| f(at + d * t);
        let (mut lo, mut h) = (0.0, hi);
        for _ in 0..200 {
            let (m1, m2) = (lo + (h - lo) / 3.0, h - (h - lo) / 3.0);
            if g(m1) < g(m2) {
                h = m2
            } else {
                lo = m1
            }
        }
        let tm = [0.0, 0.5 * (lo + h), hi].into_iter().min_by(|a, b| g(*a).total_cmp(&g(*b))).expect("three");
        if g(tm) >= need {
            return None;
        }
        let entry = if g(0.0) < need {
            -1.0
        } else {
            let (mut a, mut b) = (0.0, tm);
            for _ in 0..200 {
                let m = 0.5 * (a + b);
                if g(m) < need {
                    b = m
                } else {
                    a = m
                }
            }
            a
        };
        let exit = if g(hi) < need {
            hi + 1.0
        } else {
            let (mut a, mut b) = (tm, hi);
            for _ in 0..200 {
                let m = 0.5 * (a + b);
                if g(m) < need {
                    a = m
                } else {
                    b = m
                }
            }
            b
        };
        Some((entry, exit))
    }

    #[test]
    fn lead_len_via_end_matches_an_exact_reference_on_random_boards() {
        // r4 rule 9b: 20000 random boards as above plus a via (0.25-0.8 mm, drill 10-60 %) and a
        // hole clearance: the via end is the largest t <= the track-only length where the via
        // disc keeps every foreign item's clearance on every layer (drill: the hole clearance)
        // and the outline's edge clearance, None when there is none.
        let mut g = Rng(0x51ed_270b_27a3_c4d1);
        let (mut n, mut active, mut none, mut worst) = (0, 0, 0, 0.0f64);
        for case in 0..20000 {
            let mm = 1e6;
            let at = v(g.r(-0.5, 0.5) * mm, g.r(-0.5, 0.5) * mm);
            let a = g.r(0.0, std::f64::consts::TAU);
            let d = v(a.cos(), a.sin());
            let len = g.r(0.0, 2.0) * mm;
            let (w, s) = (g.r(0.1, 0.5) * mm, g.r(0.1, 0.3) * mm);
            let margin = if g.f() < 0.5 { 0.0 } else { 10.0 };
            let vd = g.r(0.25, 0.8) * mm;
            let via = (vd, g.r(0.1, 0.6) * vd);
            let hc = g.r(0.0, 0.35) * mm;
            let mut ob = Vec::new();
            for _ in 0..(1 + (g.f() * 6.0) as usize) {
                let c = v(g.r(-3.0, 3.0) * mm, g.r(-3.0, 3.0) * mm);
                let rad = g.r(0.05, 1.0) * mm;
                let pts: Vec<Vec2> = (0..(3 + (g.f() * 10.0) as usize)).map(|_| c + v(g.r(-rad, rad), g.r(-rad, rad))).collect();
                let poly = hull(pts);
                if poly.len() < 3 {
                    continue;
                }
                let net = match (g.f() * 4.0) as usize {
                    0 => Some(1),
                    1 => None,
                    _ => Some(2),
                };
                let li = if g.f() < 0.7 { 0 } else { 1 };
                let sag = if g.f() < 0.5 { 0.0 } else { g.r(0.0, 5000.0) };
                ob.push(obs(poly, net, li, g.f() < 0.15, g.r(0.1, 0.3) * mm, sag));
            }
            let mut kept = Vec::new();
            for ki in 0..(g.f() * 3.0) as usize {
                let (p, q) = (v(g.r(-3.0, 3.0) * mm, g.r(-3.0, 3.0) * mm), v(g.r(-3.0, 3.0) * mm, g.r(-3.0, 3.0) * mm));
                let (poly, sag) = skel_poly_sag(&Skeleton::Seg { s: crate::geom::Seg { a: p, b: q }, r: g.r(0.05, 0.25) * mm }, 16);
                kept.push(KeptPoly { kept: ki, poly, center: (p + q) * 0.5, sag, li: if g.f() < 0.8 { 0 } else { 1 }, net: if g.f() < 0.3 { 1 } else { 2 }, clear: g.r(0.1, 0.3) * mm });
            }
            let os: Vec<(Vec2, Vec2)> = if g.f() < 0.5 {
                let h = g.r(0.5, 3.0) * mm;
                let c = [v(-h, -h), v(h, -h), v(h, h), v(-h, h)];
                (0..4).map(|i| (c[i], c[(i + 1) % 4])).collect()
            } else {
                Vec::new()
            };
            let edge = g.r(0.0, 0.5) * mm;
            let (got, gv) = lead_len(&ob, &kept, &os, edge, at, d, len, w, s, 0, 1, margin, via, hc);
            let ts = lead_ref(&ob, &kept, &os, edge, at, d, len, w, s, 0, 1, margin);
            assert!((got - ts).abs() <= 0.05, "case {case}: the track-only length {got} vs {ts}");
            if ts <= 0.0 {
                continue;
            }
            let rv = 0.5 * via.0 + margin;
            let rd = 0.5 * via.1 + margin + hc;
            #[allow(clippy::type_complexity)]
            let mut cons: Vec<(Box<dyn Fn(Vec2) -> f64>, f64)> = Vec::new();
            for o in ob.iter().filter(|o| o.net != Some(1)) {
                let p = o.poly.clone();
                cons.push((Box::new(move |x| pt_poly(x, &p)), (rv + o.clear.max(s)).max(rd) + o.sag));
            }
            for k in kept.iter().filter(|k| k.net != 1) {
                let p = k.poly.clone();
                cons.push((Box::new(move |x| pt_poly(x, &p)), (rv + k.clear.max(s)).max(rd) + k.sag));
            }
            for &(a0, b0) in &os {
                cons.push((Box::new(move |x| pt_seg(x, a0, b0)), rv + edge));
            }
            let ivs: Vec<(f64, f64)> = cons.iter().filter_map(|(f, need)| bad_interval(f.as_ref(), at, d, ts, *need)).collect();
            let mut t = ts;
            let want = loop {
                match ivs.iter().find(|&&(e, x)| e < t && t < x) {
                    None => break Some(t),
                    Some(&(e, _)) if e >= 0.0 => t = e,
                    Some(_) => break None,
                }
            };
            n += 1;
            if want != Some(ts) {
                active += 1;
            }
            match (gv, want) {
                (None, None) => none += 1,
                (Some(x), Some(y)) => {
                    worst = worst.max((x - y).abs());
                    assert!((x - y).abs() <= 0.05, "case {case}: via end {x} vs exact {y} (ts {ts})");
                }
                _ => panic!("case {case}: via end {gv:?} vs exact {want:?} (ts {ts})"),
            }
        }
        eprintln!("via-end fuzz: {n} cases with track room, {active} with the via constraint active, {none} with no via end; worst |error| {worst:.4} nm");
        assert!(active > 2000 && none > 200, "the fuzz reaches every outcome: active {active}, none {none}");
    }

    #[test]
    fn lead_len_degenerate_cases() {
        // integer geometry: a 0.25 mm lead (r 125 um) on layer 0, clearance 0.2 mm (need 325 um)
        let sq = |x0: f64, y0: f64, x1: f64, y1: f64| vec![v(x0, y0), v(x1, y0), v(x1, y1), v(x0, y1)];
        let at = v(0.0, 0.0);
        let d = v(1.0, 0.0);
        let one = |o: Obstacle, margin: f64| lead_len(&[o], &[], &[], 0.0, at, d, 450_000.0, 250_000.0, 200_000.0, 0, 1, margin, (0.0, 0.0), 0.0).0;
        // a face exactly at the need from the full lead's end: the full lead (the rule is >=)
        assert_eq!(one(obs(sq(775_000.0, -1e6, 2e6, 1e6), Some(2), 0, false, 200_000.0, 0.0), 0.0), 450_000.0);
        // 1 nm closer: 1 nm shorter
        let l = one(obs(sq(774_999.0, -1e6, 2e6, 1e6), Some(2), 0, false, 200_000.0, 0.0), 0.0);
        assert!((l - 449_999.0).abs() < 0.01, "{l}");
        // the ERBS margin (r1 verifier F4): 10 nm further back than the zero-slack end
        let l = one(obs(sq(774_999.0, -1e6, 2e6, 1e6), Some(2), 0, false, 200_000.0, 0.0), 10.0);
        assert!((l - 449_989.0).abs() < 0.01, "margin 10 nm: {l}");
        // grazing: a face parallel to the lead at exactly the need is no cut
        assert_eq!(one(obs(sq(100_000.0, 325_000.0, 2e6, 1e6), Some(2), 0, false, 200_000.0, 0.0), 0.0), 450_000.0);
        // ... but it is with the margin (the lead's copper plus 10 nm reaches into it at once)
        assert_eq!(one(obs(sq(-100_000.0, 325_000.0, 2e6, 1e6), Some(2), 0, false, 200_000.0, 0.0), 10.0), 0.0);
        // the end itself inside the clearance: no lead
        assert_eq!(one(obs(sq(-1e6, 300_000.0, 1e6, 1e6), Some(2), 0, false, 200_000.0, 0.0), 0.0), 0.0);
        // own net, another layer, behind the end: no cut
        assert_eq!(one(obs(sq(500_000.0, -1e6, 2e6, 1e6), Some(1), 0, false, 200_000.0, 0.0), 0.0), 450_000.0);
        assert_eq!(one(obs(sq(500_000.0, -1e6, 2e6, 1e6), Some(2), 1, false, 200_000.0, 0.0), 0.0), 450_000.0);
        assert_eq!(one(obs(sq(-2e6, -1e6, -325_001.0, 1e6), Some(2), 0, false, 200_000.0, 0.0), 0.0), 450_000.0);
        // ... but a drilled one on the other layer cuts
        assert!(one(obs(sq(500_000.0, -1e6, 2e6, 1e6), Some(2), 1, true, 200_000.0, 0.0), 0.0) < 175_001.0);
        // the polygon's sagitta counts: 5 um of sag = 5 um shorter
        let l = one(obs(sq(774_999.0, -1e6, 2e6, 1e6), Some(2), 0, false, 200_000.0, 5_000.0), 0.0);
        assert!((l - 444_999.0).abs() < 0.01, "{l}");
        // zero length asked: zero
        assert_eq!(lead_len(&[], &[], &[], 0.0, at, d, 0.0, 250_000.0, 200_000.0, 0, 1, 10.0, (0.0, 0.0), 0.0), (0.0, None));
        // r4 via end (0.6/0.3 mm via, hole clearance 0.25 mm; `.0` the track-only length, `.1`
        // where the via fits): a face ahead at 775 um: the via disc (300 + 200 um) fits up to
        // 275 um, the track runs its full 450 um
        let via = |o: Obstacle| lead_len(&[o], &[], &[], 0.0, at, d, 450_000.0, 250_000.0, 200_000.0, 0, 1, 0.0, (600_000.0, 300_000.0), 250_000.0);
        let near = |got: Option<f64>, want: f64| got.is_some_and(|t| (t - want).abs() < 1.0);
        let (l, lv) = via(obs(sq(775_000.0, -1e6, 2e6, 1e6), Some(2), 0, false, 200_000.0, 0.0));
        assert!(l == 450_000.0 && near(lv, 275_000.0), "via room ahead: {l} {lv:?}");
        // ... the hole clearance (150 + 250 um) wins over a clearance of 50 um (the lead's own 50 um)
        let (_, lv) = lead_len(&[obs(sq(775_000.0, -1e6, 2e6, 1e6), Some(2), 0, false, 50_000.0, 0.0)], &[], &[], 0.0, at, d, 450_000.0, 250_000.0, 50_000.0, 0, 1, 0.0, (600_000.0, 300_000.0), 250_000.0);
        assert!(near(lv, 375_000.0), "hole clearance: {lv:?}");
        // ... a pad on another layer (not drilled) does not cut the track but does the via
        let (l, lv) = via(obs(sq(500_000.0, -1e6, 2e6, 1e6), Some(2), 1, false, 200_000.0, 0.0));
        assert!(l == 450_000.0 && near(lv, 0.0), "via spans all layers: {l} {lv:?}");
        // ... not monotone: beside the start only, the via fits at the full length
        let (_, lv) = via(obs(sq(-1e6, 400_000.0, 100_000.0, 1e6), Some(2), 0, false, 200_000.0, 0.0));
        assert!(near(lv, 450_000.0), "via clear beside the start: {lv:?}");
        // ... beside the lead's far half only (clearance 0.1 mm): the via ends before it, the track would not stop
        let (l, lv) = lead_len(&[obs(sq(300_000.0, 350_000.0, 2e6, 1e6), Some(2), 0, false, 100_000.0, 0.0)], &[], &[], 0.0, at, d, 450_000.0, 250_000.0, 100_000.0, 0, 1, 0.0, (600_000.0, 300_000.0), 250_000.0);
        assert!(l == 450_000.0 && near(lv, 300_000.0 - 37_500f64.sqrt() * 1000.0), "via before the far obstacle: {l} {lv:?}");
        // ... no via end anywhere on the lead: None, the track keeps its length
        let (l, lv) = via(obs(sq(-1e6, 300_000.0, 2e6, 1e6), Some(2), 1, false, 10_000.0, 0.0));
        assert!(l == 450_000.0 && lv.is_none(), "no via end: {l} {lv:?}");
        // r4 rule 9b (verifier mutant kept_layers_off): a foreign kept track on the other layer
        // 0.8 mm ahead (r 0.1 mm, clearance 0.2 mm) does not cut the track but keeps the via
        // (r 0.3 mm + 0.2 mm) at 0.8 - 0.1 - 0.5 = 0.2 mm
        let kp = |li: usize| {
            let (p, q) = (v(800_000.0, -1e6), v(800_000.0, 1e6));
            let (poly, sag) = skel_poly_sag(&Skeleton::Seg { s: crate::geom::Seg { a: p, b: q }, r: 100_000.0 }, 16);
            KeptPoly { kept: 0, poly, center: (p + q) * 0.5, sag, li, net: 2, clear: 200_000.0 }
        };
        let k1 = kp(1);
        let want = 200_000.0 - k1.sag; // the polygon's flat face at 0.7 mm, its sagitta counts
        let (l, lv) = lead_len(&[], &[k1], &[], 0.0, at, d, 450_000.0, 250_000.0, 200_000.0, 0, 1, 0.0, (600_000.0, 300_000.0), 0.0);
        assert!(l == 450_000.0 && near(lv, want), "other-layer kept track: {l} {lv:?} want {want}");
        // r4 rule 9b (via_edge_off): the outline 0.9 mm ahead with an edge clearance of 0.3 mm:
        // the via (r 0.3 mm) fits up to 0.9 - 0.3 - 0.3 = 0.3 mm, the track (r 0.125 mm) to 0.45
        let os = [(v(900_000.0, -1e6), v(900_000.0, 1e6))];
        let (l, lv) = lead_len(&[], &[], &os, 300_000.0, at, d, 450_000.0, 250_000.0, 200_000.0, 0, 1, 0.0, (600_000.0, 300_000.0), 0.0);
        assert!(l == 450_000.0 && near(lv, 300_000.0), "outline ahead: {l} {lv:?}");
        // r4 rule 9b (via_grid2, the r3 grid's coarse windows): pads on the other layer (no
        // track cut; via need 300 + 200 um) at x 0.6-0.7 mm and from 1.702 mm block the via over
        // (0.1, 1.2) and from 1.202 mm: the far window [1.2, 1.202] mm, 2 um wide, holds the end
        let o2 = [obs(sq(600_000.0, -1e6, 700_000.0, 1e6), Some(2), 1, false, 200_000.0, 0.0), obs(sq(1_702_000.0, -1e6, 3e6, 1e6), Some(2), 1, false, 200_000.0, 0.0)];
        let (l, lv) = lead_len(&o2, &[], &[], 0.0, at, d, 2_000_000.0, 250_000.0, 200_000.0, 0, 1, 0.0, (600_000.0, 300_000.0), 0.0);
        assert!(l == 2_000_000.0 && near(lv, 1_202_000.0), "narrow far window: {l} {lv:?}");
    }

    #[test]
    fn kept_polygons_lie_inside_the_copper_and_cover_it_to_the_sagitta() {
        // straight kept tracks: every polygon vertex within w/2 of the track (inside its copper),
        // every point of the copper's outline within the polygon's sagitta of it; arcs: every
        // point of the arc's centre line within 0.02 mm (the chord sagitta) of a chord.
        let mut g = Rng(0x2545_f491_4f6c_dd1d);
        let mut m = BoardModel::default();
        let iv = |p: Vec2| IVec2 { x: p.x.round() as _, y: p.y.round() as _ };
        let mut arcs = 0;
        for i in 0..400 {
            let mm = 1e6;
            let a = v(g.r(-5.0, 5.0) * mm, g.r(-5.0, 5.0) * mm);
            let b = if i % 7 == 0 { a } else { v(g.r(-5.0, 5.0) * mm, g.r(-5.0, 5.0) * mm) };
            let w = (g.r(0.1, 1.0) * mm).round();
            let prim = if i % 3 == 0 {
                let mid = (a + b) * 0.5 + (b - a).perp() * g.r(-0.5, 0.5);
                TrackPrim::Arc { start: iv(a), mid: iv(mid), end: iv(b) }
            } else {
                TrackPrim::Seg { a: iv(a), b: iv(b) }
            };
            m.kept = vec![KeptItem { uuid: format!("k{i}"), net: 1, layer: 0, width: w as i64, prim, locked: false }];
            let polys = kept_polygons(&m, &[0], 16);
            let r = 0.5 * w;
            match prim {
                TrackPrim::Seg { a, b } => {
                    let (a, b) = (a.to_f64(), b.to_f64());
                    assert_eq!(polys.len(), 1);
                    let kp = &polys[0];
                    for &p in &kp.poly {
                        assert!(pt_seg(p, a, b) <= r + 1e-6, "vertex outside the copper: {} > {r}", pt_seg(p, a, b));
                    }
                    let u = if (b - a).len() > 0.0 { (b - a) * (1.0 / (b - a).len()) } else { v(1.0, 0.0) };
                    // the copper's outline: the end circles and the two sides
                    let mut outline: Vec<Vec2> = Vec::new();
                    for k in 0..64 {
                        let t = k as f64 / 64.0 * std::f64::consts::TAU;
                        outline.extend([a + v(t.cos(), t.sin()) * r, b + v(t.cos(), t.sin()) * r]);
                        let f = k as f64 / 63.0;
                        outline.extend([a + (b - a) * f + u.perp() * r, a + (b - a) * f - u.perp() * r]);
                    }
                    for q in outline {
                        assert!(pt_poly(q, &kp.poly) <= kp.sag + 1e-6, "copper point {:.1} nm outside the polygon, sag {:.1}", pt_poly(q, &kp.poly), kp.sag);
                    }
                }
                TrackPrim::Arc { start, mid, end } => {
                    let Some(arc) = Arc::from_three_points(start.to_f64(), mid.to_f64(), end.to_f64()) else { continue };
                    arcs += 1;
                    let chords: Vec<(Vec2, Vec2)> = arc.split_sagitta(0.02e6).iter().map(|s| (s.start(), s.end())).collect();
                    assert_eq!(polys.len(), chords.len());
                    for k in 0..=200 {
                        let p = arc.point_at(k as f64 / 200.0);
                        let dmin = chords.iter().map(|&(x, y)| pt_seg(p, x, y)).fold(f64::INFINITY, f64::min);
                        assert!(dmin <= 0.02e6 + 1.0, "arc point {dmin:.1} nm off its chords");
                    }
                    for (kp, &(x, y)) in polys.iter().zip(&chords) {
                        assert!(kp.poly.iter().all(|&p| pt_seg(p, x, y) <= r + 1e-6), "a chord polygon outside its chord's copper");
                    }
                }
            }
        }
        assert!(arcs > 100, "arcs reached: {arcs}");
    }

    #[test]
    fn an_exit_stub_keeps_its_clearance_to_another_nets_kept_track() {
        // r2 (verifier F6 note): the kept tracks are obstacles after the exit stubs; the stub of a
        // fine-pitch pad (R1 1.6 x 0.4 mm at (5, 5), its neighbour of net 2 0.2 mm below) towards
        // +x must not lie on net 2's kept track across its tip (x = 6.1). Without the track the
        // stub is built (the control), with it there is none.
        let pads = r#"[{"uuid":"a","ref":"R1","number":"1","net":1,"attr":"smd","center":[5000000,5000000],"rotation_deg":0.0,"layers":[0],"shapes":{"0":{"kind":"rect","w":1600000,"h":400000}}},
                       {"uuid":"b","ref":"R2","number":"1","net":2,"attr":"smd","center":[5000000,5600000],"rotation_deg":0.0,"layers":[0],"shapes":{"0":{"kind":"rect","w":1600000,"h":400000}}}]"#;
        let mk = |kept: &str| {
            let s = format!(
                r#"{{"units":"nm","board":"t","layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
                "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}}],"pads":{pads},"kept":[{kept}],
                "outline":[],"rules":{{"default":{{"clearance":200000,"width":250000}},"min_connection":127000,"max_error":5000,"per_net":{{}}}}}}"#
            );
            BoardModel::from_json(&s).expect("model")
        };
        let x = [Extra::Exit { pad: 0, toward: v(10e6, -5e6), w: 250_000.0, s: 200_000.0 }];
        let o = BuildOpts { kept_tracks: true, ..BuildOpts::default() };
        let b = build_full(&mk(""), &o, &[], &x).expect("build");
        assert_eq!(b.exit_stubs.len(), 1, "control: the fine-pitch pad gets its stub");
        let k = r#"{"uuid":"o","net":2,"layer":0,"width":200000,"prim":{"kind":"seg","a":[6100000,3000000],"b":[6100000,7000000]}}"#;
        let b = build_full(&mk(k), &o, &[], &x).expect("build");
        for &(so, _) in &b.exit_stubs {
            let kb = b.kept_bodies.clone().map(|i| &b.proj.obstacles[i]).collect::<Vec<_>>();
            for kob in kb {
                let gap = poly_poly_dist(&b.proj.obstacles[so].poly, &kob.poly);
                assert!(gap >= 200_000.0 - 1.0, "the stub lies {gap:.0} nm from net 2's kept track");
            }
        }
        assert!(b.exit_stubs.is_empty(), "no legal stub across the kept track: {:?}", b.exit_stubs);
    }

    /// tw4 T3 (`lead_choice`): a dangling end's lead (net A's kept track (2, 5) -> (5, 5) mm, its
    /// end continued 1.5 mm along +x) offers the hand end and 1/3, 2/3 of its length as start
    /// points besides its end: centre vertices of the lead's obstacle on its axis, the end (the
    /// main centre) last, every `lead_point_t` exact; a point between them is none. Off: no
    /// choices and the same projection as before (one centre per obstacle).
    #[test]
    fn a_lead_offers_its_start_points_with_lead_choice() {
        let s = r#"{"units":"nm","board":"t","layers":[{"id":0,"name":"F.Cu","kind":"signal"},{"id":2,"name":"B.Cu","kind":"signal"}],
            "nets":[{"code":1,"name":"A"}],"pads":[],"kept":[{"uuid":"t","net":1,"layer":0,"width":250000,"prim":{"kind":"seg","a":[2000000,5000000],"b":[5000000,5000000]}}],
            "outline":[],"rules":{"default":{"clearance":200000,"width":250000},"min_connection":127000,"max_error":5000,"per_net":{}}}"#;
        let m = BoardModel::from_json(s).expect("model");
        let at = v(5e6, -5e6);
        let x = [Extra::KeptEnd { net: 1, layer: 0, at, w: 250_000.0, s: 200_000.0, lead: Some((v(1.0, 0.0), 1.5e6)), via: Vec::new() }];
        let off = build_full(&m, &BuildOpts { kept_tracks: true, ..BuildOpts::default() }, &[], &x).expect("build");
        let on = build_full(&m, &BuildOpts { kept_tracks: true, lead_choice: true, ..BuildOpts::default() }, &[], &x).expect("build");
        assert!(off.kept_ends[0].choices.is_empty(), "off: no choices");
        assert_eq!(off.proj.vkind.iter().filter(|k| matches!(k, VertKind::Centre(_))).count(), 1, "off: one centre vertex");
        let k = &on.kept_ends[0];
        let (q, _) = k.lead.expect("a lead");
        assert!((q - v(6.5e6, -5e6)).len() < 1.0, "the full 1.5 mm lead: {q:?}");
        let ts: Vec<f64> = k.choices.iter().map(|c| c.1).collect();
        assert_eq!(ts.len(), 4, "0, 1/3, 2/3 and the end: {ts:?}");
        for (t, want) in ts.iter().zip([0.0, 0.5e6, 1.0e6, 1.5e6]) {
            assert!((t - want).abs() < 1.0, "{ts:?}");
        }
        assert_eq!(k.choices.last().map(|c| c.0), on.centre[k.obs], "the end is the main centre, last");
        for &(vx, t) in &k.choices {
            assert_eq!(on.proj.vkind[vx], VertKind::Centre(k.obs));
            assert!((on.proj.pos[vx] - (at + v(t, 0.0))).len() < 1.0, "on the lead's axis");
            assert_eq!(k.lead_point_t(&on.proj, on.proj.pos[vx]), Some(t));
        }
        assert_eq!(k.lead_point_t(&on.proj, v(5.25e6, -5e6)), None, "no start point between two");
        assert_eq!(super::super::state::centre_vertices(&on.proj), on.centre, "the state's centres are the board's");
        // degenerate: a lead too short for inner points offers the hand end only
        let x = [Extra::KeptEnd { net: 1, layer: 0, at, w: 250_000.0, s: 200_000.0, lead: Some((v(1.0, 0.0), 20_000.0)), via: Vec::new() }];
        let short = build_full(&m, &BuildOpts { kept_tracks: true, lead_choice: true, ..BuildOpts::default() }, &[], &x).expect("build");
        let ts: Vec<f64> = short.kept_ends[0].choices.iter().map(|c| c.1).collect();
        assert_eq!(ts.len(), 2, "the hand end and the end: {ts:?}");
    }
}
