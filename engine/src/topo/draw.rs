//! Minimal drawing (plan P6, item A): the abstract topology of [`State`] plus the layer labels
//! of P3 become copper. Per layer, the passes of every CDT edge are packed along the edge in
//! their `edge_order` (the order is the topology), each pass gets an *interval* of the edge it
//! may use (separated from its neighbours by the demand model's spacing and from the obstacle
//! at each end by its clearance), and a wire is the shortest path (funnel / string pulling)
//! through the intervals of its sleeve. Every drawn wire is checked by the [`Checker`] against
//! the board and all copper accepted before it, so each wire is an obstacle for the next; a wire
//! the checker rejects is reported unrouted with the reason, never emitted.
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
use super::pack::{infl_dir, layer_items, wire_discs, Disc, PackItem, Packer};
use super::state::{EdgeId, PassId, State, TriId, WireId};
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
/// travel direction. O(portals).
pub fn funnel(s: Vec2, e: Vec2, portals: &[(Vec2, Vec2)]) -> Vec<Vec2> {
    let cross = |a: Vec2, b: Vec2| a.x * b.y - a.y * b.x;
    let mut all: Vec<(Vec2, Vec2)> = portals.to_vec();
    all.push((e, e));
    let mut pts = vec![s];
    let (mut apex, mut left, mut right) = (s, s, s);
    let (mut left_i, mut right_i) = (0usize, 0usize);
    let mut i = 0;
    let mut guard = 0usize;
    while i < all.len() {
        guard += 1;
        if guard > 20 * all.len() + 100 {
            break;
        }
        let (l, r) = all[i];
        let same = |a: Vec2, b: Vec2| (a - b).len2() < 1e-6;
        if cross(right - apex, r - apex) >= 0.0 {
            if same(apex, right) || cross(left - apex, r - apex) < 0.0 {
                right = r;
                right_i = i;
            } else {
                pts.push(left);
                apex = left;
                right_i = left_i;
                right = apex;
                left = apex;
                i = left_i + 1;
                continue;
            }
        }
        if cross(left - apex, l - apex) <= 0.0 {
            if same(apex, left) || cross(right - apex, l - apex) > 0.0 {
                left = l;
                left_i = i;
            } else {
                pts.push(right);
                apex = right;
                left_i = right_i;
                left = apex;
                right = apex;
                i = right_i + 1;
                continue;
            }
        }
        i += 1;
    }
    if (*pts.last().expect("start") - e).len2() > 1e-6 {
        pts.push(e);
    }
    pts.dedup_by(|a, b| (*a - *b).len2() < 1e-6);
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
fn seg_seg(a1: Vec2, b1: Vec2, a2: Vec2, b2: Vec2) -> (f64, Vec2, Vec2) {
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
pub fn via_clear(model: &BoardModel, net: i32, d: i64, drill: i64) -> f64 {
    let r = &model.rules;
    let copper = r.net_rule(net).clearance as f64;
    if drill <= 0 {
        return copper;
    }
    copper.max(0.5 * drill as f64 + r.via_hole_clear() as f64 - 0.5 * d as f64)
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

/// Counters of the drawing: which method produced each accepted run.
#[derive(Clone, Copy, Debug, Default)]
pub struct DrawStats {
    /// Runs accepted from the exact taut path.
    pub exact_ok: usize,
    /// Attempts where the exact search found no path (or hit its cap).
    pub exact_none: usize,
    /// Runs accepted only by S2a's funnel + repair fallback.
    pub fallback_ok: usize,
    /// Seconds spent in the exact search (successful or not).
    pub exact_s: f64,
}

/// Per-stretch widths that replace a wire's single width (the drawing witness, variant A: the
/// hand's own widths, necked where the hand necks). `pass_w` is the width of every pass for the
/// packing; `seg` the width of a drawn segment `(wire, layer, a, b)`; `min_w` the narrowest width
/// of each wire (the exact search adds clearance circles for it).
pub struct Widths<'a> {
    pub pass_w: Vec<f64>,
    pub min_w: Vec<f64>,
    #[allow(clippy::type_complexity)]
    pub seg: Box<dyn Fn(WireId, usize, Vec2, Vec2) -> f64 + 'a>,
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
}

pub struct Drawer<'a> {
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
    pub(super) grid: Grid<usize>,
    /// `(ItemIndex id, ent id)` per committed item; the handle is the position.
    handles: Vec<(usize, usize)>,
    /// Query distance of the repair (largest clearance + width).
    pub(super) qgrow: f64,
    /// Number of static objects (they come first in `ents`).
    pub(super) n_static: usize,
    /// Diagnosis: ignore the copper drawn so far.
    pub(super) only_static: std::cell::Cell<bool>,
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
    /// Every via placed: `(wire, traversal, centre, layer before, layer after, handle)`; it
    /// counts while its copper is committed (see [`Drawer::via_walls`]).
    via_marks: Vec<(WireId, usize, Vec2, usize, usize, usize)>,
    /// Wires drawn at least once (their copper, not a via wall, stands for their chords).
    drawn_once: Vec<bool>,
    /// Per pass of a fitting edge-layer: `(edge, layer, index in the lane, lmin, rmax)` (nm).
    slot: Vec<Option<Slot>>,
    /// Per edge-layer that fits: its passes in edge order (see [`Lane`]).
    lanes: BTreeMap<(EdgeId, usize), Lane>,
    /// Drawn crossing of every pass: distance along its edge (nm) and the handle of the first
    /// track of its run (the crossing counts while that copper is committed).
    drawn_x: Vec<Option<(f64, usize)>>,
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
            pass_disc: vec![[0.0; 2]; st.passes.len()],
            over: vec![false; st.proj.edges.len() * st.proj.n_layers],
            spare: vec![f64::INFINITY; st.proj.edges.len() * st.proj.n_layers],
            diag: Vec::new(),
            stats: DrawStats::default(),
            ents: Vec::new(),
            grid: Grid::new(2.0e6),
            handles: Vec::new(),
            qgrow: 0.0,
            n_static: 0,
            only_static: std::cell::Cell::new(false),
            quiet: false,
            widths: None,
            tight: false,
            prov_layer: Vec::new(),
            via_marks: Vec::new(),
            drawn_once: vec![false; st.wires.len()],
            slot: vec![None; st.passes.len()],
            lanes: BTreeMap::new(),
            drawn_x: vec![None; st.passes.len()],
        }
        .with_static()
    }

    fn add_ent(&mut self, e: Ent) -> usize {
        let core_box = match &e.core {
            Core::Poly(p) => bbox_of(p),
            Core::Seg(a, b) => bbox_of(&[*a, *b]),
            Core::Pt(p) => bbox_of(&[*p]),
        };
        let id = self.ents.len();
        self.grid.insert(core_box.grow(e.grow + e.clear + 1.0), id);
        self.ents.push(e);
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
        for ob in &b.proj.obstacles {
            let mask = if ob.drilled { all } else { ob.layers.0 };
            // The polygon of a rounded shape lies inside the copper by its sagitta (`sag`; 0 for a
            // rectangle). It was 2 % of the circumradius for every shape, 57 um phantom copper
            // around a 4 x 4 mm test pad (wiccon TP13: the 3.3v wire then found no path past
            // it, D1).
            self.add_ent(Ent { core: Core::Poly(ob.poly.clone()), grow: ob.sag, mask, net: ob.net, clear: ob.clear, fixed: ob.bare, live: true, shape: false });
        }
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
        let maxclr = r.per_net.values().map(|n| n.clearance).fold(r.default.clearance.max(r.min_clearance).max(r.hole_clearance).max(r.edge_clearance), i64::max);
        let maxw = r.per_net.values().map(|n| n.width).fold(r.default.width, i64::max);
        self.qgrow = (maxclr + maxw) as f64 + 4.0 * self.eps;
        self.n_static = self.ents.len();
        self
    }

    /// Index copper accepted for `net` as an object the next wires keep away from.
    fn add_dynamic(&mut self, item: &Item) -> usize {
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

    fn layer_id(&self, l: usize) -> u32 {
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
    /// `ks` of diameters `ds`. O(passes of the wire).
    pub fn set_wire_layers(&mut self, w: WireId, plan: &Plan, ks: &[usize], ds: &[f64]) {
        let n = self.st.wires[w].passes.len();
        for r in 0..plan.layers.len() {
            let k0 = if r == 0 { 0 } else { ks[r - 1] };
            let k1 = if r == plan.layers.len() - 1 { n } else { ks[r] };
            for k in k0.min(n)..k1.min(n) {
                self.pass_layer[self.st.wires[w].passes[k]] = plan.layers[r] as i32;
            }
        }
        let disc = wire_discs(self.st, w, ks, ds);
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
                let pw = self.widths.as_ref().map_or(wi.w, |x| x.pass_w[pid]);
                PackItem { net: wi.net, w: pw, s: wi.s, infl: infl_dir(&self.b.proj, e, self.dirs[w][k]), disc: [0.0; 2] }
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
    fn live_x(&self, pid: PassId) -> Option<f64> {
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
        let pt = |f: f64| pp + (pq - pp) * f;
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
            let (la, lb, lc) = ((b - cc).len(), (a - cc).len(), (a - b).len());
            let sum = (la + lb + lc).max(1.0);
            let inc = (a * la + b * lb + cc * lc) * (1.0 / sum);
            let cen = (a + b + cc) * (1.0 / 3.0);
            let bary = |u: f64, v: f64| a * u + b * v + cc * (1.0 - u - v);
            for p in [inc, cen, bary(0.6, 0.2), bary(0.2, 0.6), bary(0.2, 0.2), bary(0.4, 0.4), bary(0.4, 0.1), bary(0.1, 0.4), bary(0.1, 0.1)] {
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
        // The packed crossings themselves first: there the packing kept the disc off the edge's
        // end obstacles and its neighbours on that edge.
        [1.0, 0.0, 0.5, 0.75, 0.25, 0.9, 0.1].iter().map(|&f| (k, at(f))).collect()
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

    /// Place the vias of wire `w` (ids of the indexed items in `ids`), or the reason it cannot be.
    pub fn place_vias(&mut self, w: WireId, plan: &Plan, ids: &mut Vec<usize>) -> Result<Vec<(usize, Vec2, ViaSize)>, String> {
        let net = self.st.wires[w].net;
        let sizes = via_sizes(&self.model.rules, net, self.opts);
        let mut out: Vec<(usize, Vec2, ViaSize)> = Vec::new();
        for (i, &(ka, kb)) in plan.vias.iter().enumerate() {
            let ka = out.last().map_or(ka, |l| ka.max(l.0));
            let mut done = false;
            'sizes: for &sz in &sizes {
                let need_r = 0.5 * sz.d as f64 + self.model.rules.net_rule(net).clearance as f64;
                let at = plan.at.get(i).copied().filter(|&k| (ka..=kb.max(ka)).contains(&k));
                let mut sites = at.map_or(Vec::new(), |k| self.chord_sites(w, k));
                sites.extend(self.via_candidates(w, ka, kb.max(ka), need_r, at));
                if self.opts.via_offset_sites {
                    sites.extend(self.via_extra(w, ka, kb.max(ka), sz.d as f64, self.model.rules.net_rule(net).clearance as f64));
                }
                // Of the valid sites (side test, clearance) in the first triangle that has one,
                // the one with the most free room around it (the `via_squeeze` factor's aim).
                let mut best: Option<(f64, usize, Vec2)> = None;
                let dbg = std::env::var("TOPO_VIADBG").is_ok();
                let (mut n_side, mut n_check) = (0usize, 0usize);
                for (k, p) in sites {
                    if best.is_some_and(|b| b.1 != k) {
                        break;
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
                    if self.check_ok(&item) && self.via_hole_ok(p, sz, net) && self.via_own_pad_ok(p, sz, net) {
                        let room = self.via_room(w, k, p, sz.d as f64);
                        if best.is_none_or(|b| room > b.0 + 1.0) {
                            best = Some((room, k, p));
                        }
                    }
                }
                if let Some((room, k, p)) = best {
                    if dbg {
                        eprintln!("VIAPICK wire {w} net {net} via {i} ka {ka} kb {kb} planned {at:?} chosen k {k} at ({:.3},{:.3}) room {:.0} size {} (planned triangle: {n_side} wrong side, {n_check} rejected)", p.x / 1e6, -p.y / 1e6, room, sz.d);
                    }
                    let id = self.commit(&via_item(net, p, sz, Some(w)));
                    self.via_marks.push((w, k, p, plan.layers[i], plan.layers[i + 1], id));
                    ids.push(id);
                    out.push((k, p, sz));
                    done = true;
                    break 'sizes;
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

    /// The via site `p` in traversal `k` of wire `w` (run layers `la` before the via, `lb` after)
    /// lies on the side the topology gives it of every other wire's chord through that triangle
    /// on `la` or `lb`: the via sits on `w`'s chord, so it is on the side of `w`'s exit point
    /// (for a chord on `lb`) or entry point (on `la`). Chords are the nominal ones (evenly spread
    /// pass points and turn corners, [`State::polyline`]); a chord whose layer is still open (a
    /// via interval of its own) or of the same net is no constraint. A via on the wrong side
    /// leaves its run on a layer no way to its first pass but across that wire (wiccon MOSI,
    /// D2). O(traversals of the triangle x chord corners).
    pub fn via_side_ok(&self, w: WireId, k: usize, p: Vec2, la: usize, lb: usize) -> bool {
        self.via_side_ok_(w, k, p, la, lb)
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

    fn via_side_ok_(&self, w: WireId, k: usize, p: Vec2, la: usize, lb: usize) -> bool {
        if self.prov_layer.is_empty() {
            return true;
        }
        let (st, pr) = (self.st, &self.b.proj);
        let wi = &st.wires[w];
        let t = wi.tris[k];
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
        for &(o, ok) in &st.tri_trav[t] {
            let oi = &st.wires[o];
            if o == w || !oi.alive || oi.net == wi.net || self.polys[o].is_empty() {
                continue;
            }
            let lo = chord_layer(o, ok);
            let r = if lo == lb as i32 {
                self.chord_pt(w, k + 1)
            } else if lo == la as i32 {
                self.chord_pt(w, k)
            } else {
                continue;
            };
            let mut chord = vec![self.chord_pt(o, ok)];
            chord.extend(oi.turns.iter().filter(|x| x.0 == ok).map(|x| pr.pos[x.1]));
            chord.push(self.chord_pt(o, ok + 1));
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
    /// them, and KiCad's minimum connection width check flags it (seen on dcsgonebadge: a 0.6 mm
    /// via 4 um into BT1 pad 3, "actual 0.1044 mm" < 0.127 mm). Clear of the pad (gap >= 0) or
    /// centred inside it is fine; with no minimum connection width set, anything goes.
    /// O(pads of the net x pad vertices).
    pub fn via_own_pad_ok(&self, p: Vec2, size: ViaSize, net: i32) -> bool {
        if self.model.rules.min_connection <= 0 || net == 0 {
            return true;
        }
        let r = 0.5 * size.d as f64;
        self.model.pads.iter().filter(|pad| pad.net == net).all(|pad| {
            pad.layers.iter().all(|&l| {
                let d = crate::geom::point_skeleton(p, &self.model.skeleton(pad, l));
                d >= r || d <= 0.0
            })
        })
    }

    /// No track of `items` (a run of wire `w`) grazes a pad of its own net other than its end
    /// pads: either it keeps KiCad's `max_error` off the pad (KiCad's minimum connection width
    /// check builds the pad polygon up to that far outside the arcs) or its centre line runs
    /// into the pad. A graze is a sliver connection KiCad flags (dcsgonebadge R10.1, D2: a track
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
        if pads.is_empty() {
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
            })
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
        }
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
        let disc = wire_discs(st, w, cuts, &vec![dmax; cuts.len()]);
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
        let (st, pr) = (self.st, &self.b.proj);
        let wi = &st.wires[w];
        let n = wi.passes.len();
        let mut out = WireOut { wire: w, ..Default::default() };
        let mut committed: Vec<usize> = Vec::new();
        for r in 0..plan.layers.len() {
            let k0 = if r == 0 { 0 } else { vias[r - 1].0 };
            let k1 = if r == plan.layers.len() - 1 { n } else { vias[r].0 };
            let s = if r == 0 { pr.pos[wi.a] } else { vias[r - 1].1 };
            let e = if r == plan.layers.len() - 1 { pr.pos[wi.b] } else { vias[r].1 };
            let layer = plan.layers[r];
            let lid = self.layer_id(layer);
            let mut ok = false;
            let mut last_reason = String::new();
            let mut last_viol: Option<Violation> = None;
            let mut tried: Vec<Vec<Vec2>> = Vec::new();
            let neck = self.neck_zones(w, r == 0, r == plan.layers.len() - 1);
            for attempt in 0..5 {
                // 0: the exact taut path at full width; 1: the same with pad-end neck zones (only
                // where the first failed: a necked end enters the pad with less copper);
                // 2..4: S2a's funnel + repair attempts.
                let pts = if attempt <= 1 {
                    if attempt == 1 && neck.is_none() {
                        continue;
                    }
                    let base = self.base_polyline(w, k0, k1, s, e, 0);
                    let t_ex = std::time::Instant::now();
                    let fixed = self.exact_fix(w, k0, k1, layer, base, if attempt == 1 { neck } else { None });
                    self.stats.exact_s += t_ex.elapsed().as_secs_f64();
                    match fixed {
                        Some(p) => p,
                        None => {
                            self.stats.exact_none += 1;
                            continue;
                        }
                    }
                } else {
                    self.run_polyline(w, k0, k1, s, e, attempt - 2, layer)
                };
                tried.push(pts.clone());
                if attempt >= 2 {
                    let walls = self.via_walls(w, k0, k1, layer);
                    if pts.windows(2).any(|x| walls.iter().any(|&(c, d)| seg_proper(x[0], x[1], c, d))) {
                        last_reason = "drawing: passes a via of another net on the wrong side (topology)".to_string();
                        continue;
                    }
                }
                let (ps, pe) = (r == 0, r == plan.layers.len() - 1);
                let mut items = self.run_items(w, layer, &pts);
                let mut rep = self.checker.check_against(&self.idx, &items, &[]);
                let mut neck: Option<(f64, [f64; 2])> = None;
                if !rep.violations.is_empty() && self.opts.neck_to_pad && (ps || pe) {
                    if let Some((wn, its, ls)) = self.neck_run(w, lid, &pts, ps, pe) {
                        items = its;
                        rep = CheckReport::default();
                        neck = Some((wn, ls));
                    }
                }
                if rep.violations.is_empty() && !self.track_own_pad_ok(w, &items) {
                    last_reason = "drawing: a track grazes a pad of its own net (KiCad minimum connection width)".to_string();
                    continue;
                }
                if rep.violations.is_empty() {
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
                    if attempt <= 1 {
                        self.stats.exact_ok += 1;
                    } else {
                        self.stats.fallback_ok += 1;
                    }
                    out.items.extend(items);
                    out.runs.push((layer, pts));
                    ok = true;
                    break;
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
            if !ok {
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
        for &(_, p, sz) in vias {
            out.items.push(via_item(wi.net, p, sz, Some(w)));
            out.vias.push((p, sz));
        }
        out.handles = committed;
        self.drawn_once[w] = true;
        out.length_nm = out.runs.iter().map(|(_, p)| p.windows(2).map(|x| (x[1] - x[0]).len()).sum::<f64>()).sum();
        out.ok = true;
        out
    }

    /// Track items of the run polyline `pts` of wire `w` on `layer`: the wire's width, or the
    /// per-segment width of [`Drawer::widths`].
    fn run_items(&self, w: WireId, layer: usize, pts: &[Vec2]) -> Vec<Item> {
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
        Some(super::exact::Neck { min_w, ends: [ps.then(|| zone(wi.pa)), pe.then(|| zone(wi.pb))] })
    }

    /// The run with its pad ends narrowed to the widest width the checker accepts (plan item
    /// "necking", `Options.neck_to_pad`): the first / last `neck_max` of the polyline (twice the
    /// pad's long side unless `Options.neck_max_nm` is set) take width `wn`, `min_w <= wn < w`;
    /// the rest keeps `w`. `None` when even `min_w` is rejected. Returns `(wn, items, lengths)`.
    fn neck_run(&self, w: WireId, lid: u32, pts: &[Vec2], ps: bool, pe: bool) -> Option<(f64, Vec<Item>, [f64; 2])> {
        let wi = &self.st.wires[w];
        let (ps, pe) = (ps && wi.pa != super::state::NO_OBS, pe && wi.pb != super::state::NO_OBS);
        let rules = &self.model.rules;
        let min_w = Self::neck_floor(rules, wi.net, wi.w);
        if min_w >= wi.w || !(ps || pe) {
            return None;
        }
        let ext = |ob: usize| self.b.proj.obstacles[ob].poly.iter().map(|&p| (p - self.b.proj.obstacles[ob].center).len()).fold(0.0, f64::max);
        let cap = |ob: usize| if self.opts.neck_max_nm > 0 { self.opts.neck_max_nm as f64 } else { 4.0 * ext(ob) };
        let lens = [if ps { cap(wi.pa) } else { 0.0 }, if pe { cap(wi.pb) } else { 0.0 }];
        let build = |wn: f64| -> (Vec<Item>, [f64; 2]) {
            let n = pts.len();
            // Cut points: along the polyline from each pad end.
            let total: f64 = pts.windows(2).map(|p| (p[1] - p[0]).len()).sum();
            let (mut l0, mut l1) = (lens[0].min(total), lens[1].min(total));
            if l0 + l1 > total {
                let f = total / (l0 + l1).max(1.0);
                l0 *= f;
                l1 *= f;
            }
            let mut items = Vec::new();
            let mut acc = 0.0;
            for i in 0..n - 1 {
                let (a, b) = (pts[i], pts[i + 1]);
                let sl = (b - a).len();
                if sl < 1.0 {
                    continue;
                }
                // Cuts of this segment at the neck ends, as fractions.
                let mut cuts = vec![0.0, 1.0];
                if l0 > acc && l0 < acc + sl {
                    cuts.push((l0 - acc) / sl);
                }
                if total - l1 > acc && total - l1 < acc + sl {
                    cuts.push((total - l1 - acc) / sl);
                }
                cuts.sort_by(f64::total_cmp);
                for c in cuts.windows(2) {
                    let (u, v) = (a + (b - a) * c[0], a + (b - a) * c[1]);
                    let mid = acc + sl * 0.5 * (c[0] + c[1]);
                    let neck = mid < l0 || mid > total - l1;
                    if (v - u).len2() > 1.0 {
                        items.push(track_item(wi.net, lid, if neck { wn } else { wi.w }, u, v, Some(w)));
                    }
                }
                acc += sl;
            }
            (items, [l0, l1])
        };
        let ok = |wn: f64| self.checker.check_against(&self.idx, &build(wn).0, &[]).violations.is_empty();
        if !ok(min_w) {
            return None;
        }
        let (mut lo, mut hi) = (min_w, wi.w);
        for _ in 0..12 {
            let mid = 0.5 * (lo + hi);
            if ok(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let wn = lo.floor();
        let (items, ls) = build(wn);
        Some((wn, items, ls))
    }

    /// The polyline of one run from `s` (in traversal `k0`) to `e` (in traversal `k1`); `attempt`
    /// 0 = full intervals, 1 = intervals halved towards nominal, 2 = nominal points.
    #[allow(clippy::too_many_arguments)]
    fn run_polyline(&self, w: WireId, k0: usize, k1: usize, s: Vec2, e: Vec2, attempt: usize, layer: usize) -> Vec<Vec2> {
        let wi = &self.st.wires[w];
        let pre = self.base_polyline(w, k0, k1, s, e, attempt);
        self.repair(wi.net, wi.w, wi.s, layer, pre)
    }

    /// The polyline before the repair.
    fn base_polyline(&self, w: WireId, k0: usize, k1: usize, s: Vec2, e: Vec2, attempt: usize) -> Vec<Vec2> {
        let st = self.st;
        let wi = &st.wires[w];
        let mut portals = Vec::with_capacity(k1 - k0);
        let mut nominal = vec![s];
        for k in k0..k1 {
            let pid = wi.passes[k];
            let (lo, hi, nom) = (self.iv.lo[pid], self.iv.hi[pid], self.iv.nom[pid]);
            let (lo, hi) = match attempt {
                0 => match self.tight_span(pid).filter(|_| self.tight) {
                    Some((a, b)) => {
                        let [p, q] = self.b.proj.edges[st.passes[pid].edge];
                        let len = (self.b.proj.pos[q] - self.b.proj.pos[p]).len().max(1.0);
                        ((a / len).clamp(0.0, 1.0), (b / len).clamp(0.0, 1.0))
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
        self.only_static.set(deep);
        let static_infeasible = deep && self.exact_fix(w, k0, k1, layer, pre0.clone(), neck).is_none();
        self.only_static.set(false);
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
    fn repair(&self, net: i32, w: f64, s: f64, layer: usize, mut pts: Vec<Vec2>) -> Vec<Vec2> {
        let mut bumps: BTreeMap<usize, u32> = BTreeMap::new();
        for _ in 0..60 {
            // The worst violation of every segment: (segment, short, object, nearest point, push, need).
            let mut fixes: Vec<(usize, usize, Vec2, Vec2, f64)> = Vec::new();
            for i in 0..pts.len() - 1 {
                let (a, b) = (pts[i], pts[i + 1]);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x, y }
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

    #[test]
    fn plan_splits_runs_at_label_changes() {
        let p = plan(&[(0, 0.0, 0), (2, 0.5, 0), (4, 0.2, 1), (6, 1.0, 1)], 2);
        assert_eq!(p.layers, vec![0, 1]);
        assert_eq!(p.vias, vec![(2, 4)]);
        assert_eq!(plan(&[], 2), Plan { layers: vec![1], vias: vec![], at: vec![] });
    }
}
