//! M3 step c (shopping list 6.2; design note `docs/progress/notes/m3-radii-spokes-design.md`
//! §4.3, §15): the extended rubber band (ERBS) of every stretch, from the spoke tips and stack
//! radii of [`super::radii`], and its check.
//!
//! **The ERBS is the shortest path of the band's homotopy class among the spoke tips** (Dayan
//! 1997, PDF pp. 25-26, printed pp. 14-15: the ERBS is "a minimal length member of the topological
//! class", its terminals a superset of the RBS's, the spoke ends added; the spokes PDF p. 32).
//! There is no order of gates to solve: the class says on which side of the path every obstacle
//! lies, and the shortest path of a class among point obstacles is the funnel through the sleeve
//! of a triangulation of the points (Erickson, lecture notes "Shortest (Homotopic) Paths",
//! sections "Crossing Sequences" and "Sleeves", docs/literature/erickson-shortest-homotopic.html;
//! Efrat, Kobourov & Lubiw 2006, PDF p. 9: the sleeve, then the funnel, O(n + k) per path;
//! Popov & Popov 2012, PDF p. 3, Fig. 3:
//! the funnel over circles classified left or right of the path, p. 2, Fig. 1: the taut thread
//! runs along the arcs). Per stretch:
//!
//! 1. **Obstacle points, each with its side** (left or right of the path):
//!    - every cut item: its two spoke tips, `a` (end of the spoke `p a`) on `p`'s side and `b`
//!      (end of `b q`) on `q`'s side of the band's crossing (the tips hold half the width, the
//!      clearance and the items between, [`super::radii::tip_point`]).
//!    - every wrap: its arc at the stack radius, sampled inside the wrap's wedge (the octagon's
//!      vertices in the octagonal norm, a circumscribed polygon of `arc_step_deg` otherwise), on the
//!      wrapped vertex's side.
//!    - every feature of another net near the band (pads, via discs, holes, keepouts, junctions):
//!      its outline grown by half the width, its clearance and the placement margin
//!      ([`ErbsOpts::margin`]: `margin_nm + DRAW_TOL_NM`, 10 nm by default, which the tips and
//!      stack radii carry on every clearance too, so the copper keeps the rule after KiCad's
//!      rounding to whole nm; the sequential packer keeps the same) (the Minkowski sum with the norm's
//!      circumscribed unit polygon; the convex hull for a convex feature), on the feature's side.
//!      A feature at an end of an over-full cut the stretch is on is left out (the necked tips
//!      hold that clearance; it is a predicted violation).
//!    - a box round all of them (the triangulation covers the rough path).
//! 2. **Rough path** ([`rough_path`]): the band with a thin detour round every point the band
//!    passes on the wrong side: a spoke tip whose spoke the band's crossing lies on (the crossing
//!    before `a` or after `b` along the cut; Dayan PDF p. 32: the band goes round the spoke's
//!    end), every arc sample, a grown feature point on the other side of its nearest band piece.
//!    Detours leave at the nearest band point, so they never cross each other.
//! 3. **Sleeve** ([`sleeve`]): the triangulation edges the rough path crosses in a Delaunay
//!    triangulation of the points ([`Cdt`]), a crossing straight back over the same edge cancelled
//!    (the reduced sleeve of the class). A path that leaves the triangulation is a failure: no
//!    ERBS, the stretch is drawn the sequential way and counted (`erbs_fallback`).
//! 4. **Funnel** through the portals: every bend of the path lies on an obstacle point and turns
//!    toward it.
//!
//! **Check.** All ERBS of a layer against each other (different nets: half widths plus KiCad's
//! pairwise clearance) and against the sheet's features (pads, vias, holes, the outline). Dayan
//! PDF p. 26 claims SURF's spacing holds when the sketch is feasible; the proofs were not
//! obtained (inflated-sketch §2.5), so it is measured: a violation where an over-full cut carries
//! both parties (or the plan crosses them) is **predicted**; any other is an **`erbs_gap`**.
//!
//! Debug: `TOPO_RADII` puts the ERBS counts per layer in the cut report's summary;
//! `TOPO_ERBS_DUMP=<dir>` writes every layer's ERBS (cut report: `erbs-<layer>.json`, drawing:
//! `realise-L<n>.json`; read by `harness/erbs_accept.py`); `TOPO_ERBS_TIME` one line per stretch; `TOPO_ERBS_SLEEVE` the band, points, rough path,
//! portals and path of every stretch (mm; `TOPO_ERBS_SLEEVE=bad`: only failed stretches and those over 1.3 x their band).
//!
//! Complexity per stretch: O(n log n) for n obstacle points (the triangulation; the walk and the
//! funnel are O(n)); per layer the check O(segments x neighbours in the grid cells).

use super::cutflow::Stretch;
use super::demand::{Bound, Item};
use super::draw::{poly_dist, seg_seg_dist};
use super::radii::{tip_point, LayerOrder};
use super::sheet::{Feature, LayerSheet, Norm};
use super::state::point_in_poly;
use crate::cdt::Cdt;
use crate::geom::Vec2;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::f64::consts::{FRAC_PI_4, FRAC_PI_8, TAU};

thread_local! {
    /// Debug (`TOPO_ERBS_SLEEVE=bad`): the stretch is run again with its sleeve printed.
    static SLEEVE_AGAIN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Print the sleeve of this stretch: `TOPO_ERBS_SLEEVE` set (any value but `bad`), or the
/// second run of a bad stretch under `TOPO_ERBS_SLEEVE=bad`.
fn sleeve_dbg() -> bool {
    match std::env::var_os("TOPO_ERBS_SLEEVE") {
        Some(v) if v == "bad" => SLEEVE_AGAIN.with(|c| c.get()),
        Some(_) => true,
        None => false,
    }
}

/// ERBS options.
#[derive(Clone, Debug)]
pub struct ErbsOpts {
    /// Arc sample step of the Euclidean norm (degrees); the octagonal norm uses its 8 vertices.
    pub arc_step_deg: f64,
    /// The run's time limit: a stretch not started by then has no ERBS (its runs are drawn by the
    /// sequential ladder, counted in `timed_out`, with a warning). `None`: no limit.
    pub deadline: Option<std::time::Instant>,
    /// The run's cancel flag (the user's Cancel, the CPU-clock limit), as the deadline. The
    /// drawing passes it only while a Cancel may still stop the drawing (not in the TopoR loop's
    /// one drawing, which runs through, core.rs `draw_through`).
    pub stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Test-only: the first stretch of the layer fails ("injected sleeve failure"), to see its
    /// reason reach `failed` and the ERBS_FALLBACK line. [`realise`] sets it on the first layer
    /// that has a stretch only, so a run has exactly one injected failure whatever its layering.
    /// Always false in real runs.
    pub fail_first: bool,
    /// Placement margin on every feature clearance (nm; [`super::cutflow::MeasureOpts::margin`],
    /// the same value the tips and stack radii of [`super::radii::radii_layer_m`] carry): the
    /// grown features keep `half width + clearance + margin`, so the ERBS copper keeps the rule
    /// after rounding to whole nm. 0 = exact (the model tests).
    pub margin: f64,
}

impl Default for ErbsOpts {
    fn default() -> Self {
        ErbsOpts { arc_step_deg: 22.5, deadline: None, stop: None, fail_first: false, margin: 0.0 }
    }
}

impl ErbsOpts {
    /// The time limit has passed or the run was cancelled. O(1).
    fn expired(&self) -> bool {
        self.deadline.is_some_and(|d| std::time::Instant::now() >= d) || self.stop.as_ref().is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed))
    }
}

/// Where an obstacle point comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GateSrc {
    /// A spoke tip of a cut item (index into [`LayerOrder::cuts`]).
    Cut(usize),
    /// An arc sample round sheet vertex `v` (a wrap at its stack radius).
    Arc(usize),
    /// A vertex of feature `f`'s outline grown by the half width and its clearance.
    Feature(usize),
}

/// The obstacle behind a point: for a cut item its portal `a b` on the cut `p q` (the spokes `p a`
/// and `b q`); for an arc sample or a grown feature `a == b` (the point) and `p == q` (its root:
/// the wrapped vertex, the feature's nearest vertex).
#[derive(Clone, Copy, Debug)]
pub struct Gate {
    pub a: Vec2,
    pub b: Vec2,
    /// Width of the wire there.
    pub w: f64,
    pub src: GateSrc,
    pub p: Vec2,
    pub q: Vec2,
    /// `a` lies left of the path (for an arc or feature point: the point does).
    pub p_left: bool,
}

/// The ERBS of one stretch.
#[derive(Clone, Debug)]
pub struct Erbs {
    pub stretch: usize,
    /// Start, the bends (obstacle points, in path order), end.
    pub pts: Vec<Vec2>,
    /// Width at every point of `pts`.
    pub w: Vec<f64>,
    /// The obstacle (index into `gate_list`) of every point of `pts` (`None` at the terminals).
    pub at: Vec<Option<usize>>,
    /// `pts` without repeated and collinear points (the drawn shape).
    pub path: Vec<Vec2>,
    /// Every obstacle of the stretch (debug dump, precedes order).
    pub gate_list: Vec<Gate>,
    /// Obstacle points in the triangulation (frame included) and portals of the sleeve.
    pub points: usize,
    pub portals: usize,
    /// Coincident points of opposite sides (the later one left out): a passage of zero width.
    pub conflicts: usize,
    /// Features of other nets the band runs through (the plan's violation): no obstacle, the
    /// check counts them as predicted.
    pub through: Vec<usize>,
    /// Features of other nets within the half width plus their clearance of an end of the band
    /// (the wire's own end violates their clearance: the pad exit's neck): no obstacle and no
    /// spoke detour at them; the check counts them as predicted.
    pub exit: Vec<usize>,
    /// Obstacles whose push was settled away (opposite pushes with crossing spokes, rule 4 of
    /// round h): the band stays at their over-full room; the check counts its violations of their
    /// parties (the cut's ends and the stretches on the cut, the wrapped vertex's or the grown
    /// feature) as predicted, as an over-full cut's.
    pub settled: Vec<GateSrc>,
    /// Labelled points left out as obstacles (rule 5 of round h: no push, on the band's other
    /// side than their label). Their features stay in the check (`check_layer`).
    pub unenforced: usize,
    /// The sleeve reached the end (always true for a returned ERBS; kept for the reports).
    pub converged: bool,
    pub len: f64,
    pub band_len: f64,
}

/// The other party of a violation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Party {
    Stretch(usize),
    Feature(usize),
    Edge,
}

/// One clearance violation of an ERBS (the worst place per pair).
#[derive(Clone, Copy, Debug)]
pub struct Violation {
    pub stretch: usize,
    pub other: Party,
    pub at: Vec2,
    /// Required minus actual distance (nm).
    pub amount: f64,
    /// An over-full cut (or a planned crossing) carries both parties.
    pub predicted: bool,
}

/// Counts of one layer.
#[derive(Clone, Debug, Default)]
pub struct ErbsReport {
    pub stretches: usize,
    pub points: usize,
    pub portals: usize,
    pub conflicts: usize,
    /// Stretches whose sleeve did not reach the end (no ERBS: drawn the sequential way).
    pub failed: usize,
    pub len: f64,
    pub band_len: f64,
    pub predicted: usize,
    pub erbs_gap: usize,
    pub worst_gap: f64,
}

/// The ERBS of a layer: the stretches that have one, those whose sleeve failed (with the reason)
/// and the number not started before the time limit.
#[derive(Clone, Debug, Default)]
pub struct LayerErbs {
    pub es: Vec<Erbs>,
    pub failed: Vec<(usize, String)>,
    pub timed_out: usize,
}

/// Clockwise angle from `a` to `b` in `[0, 2 pi)`.
fn cw(a: Vec2, b: Vec2) -> f64 {
    let t = (-a.cross(b)).atan2(a.dot(b));
    if t < 0.0 {
        t + TAU
    } else {
        t
    }
}

/// Share of the way round a wrap (0 at the incoming side, 1 at the outgoing side) of the
/// direction `u` from the vertex, or `None` outside the wedge (as `cutflow::in_wedge`).
fn wedge_share(din: Vec2, dout: Vec2, left: bool, u: Vec2) -> Option<f64> {
    const TOL: f64 = 1e-9;
    let back = -din;
    let (a, lim) = if left { (cw(dout, u), cw(dout, back)) } else { (cw(u, dout), cw(back, dout)) };
    (a > TOL && a < lim - TOL).then(|| 1.0 - a / lim)
}

/// The unit directions of the norm's circumscribed polygon and its vertex radius factor: the
/// octagon with its flat sides facing the octilinear directions (vertices at 22.5 + k 45 degrees,
/// radius `1 / cos 22.5`) in the octagonal norm; a polygon of `arc_step_deg` in the Euclidean.
fn norm_dirs(norm: Norm, arc_step_deg: f64) -> (Vec<Vec2>, f64) {
    match norm {
        Norm::Octagonal => ((0..8).map(|k| Vec2 { x: (FRAC_PI_8 + k as f64 * FRAC_PI_4).cos(), y: (FRAC_PI_8 + k as f64 * FRAC_PI_4).sin() }).collect(), 1.0 / FRAC_PI_8.cos()),
        Norm::Euclid => {
            let st = arc_step_deg.clamp(1.0, 45.0).to_radians();
            let n = (TAU / st).round().max(8.0) as usize;
            let st = TAU / n as f64;
            ((0..n).map(|k| Vec2 { x: (k as f64 * st).cos(), y: (k as f64 * st).sin() }).collect(), 1.0 / (0.5 * st).cos())
        }
    }
}

/// Side of point `x` against the polyline `band` (`true`: left of its direction of travel) and
/// the distance to it. The nearest piece decides; at a corner both pieces' sides, and where they
/// differ (the cone outside the corner) the outer side of the turn. O(band).
fn band_side(band: &[Vec2], x: Vec2) -> (bool, f64) {
    let n = band.len();
    if n < 2 {
        return (false, band.first().map_or(f64::INFINITY, |&b| (x - b).len()));
    }
    let mut best = (f64::INFINITY, 0usize, 0.0f64);
    for j in 0..n - 1 {
        let (a, b) = (band[j], band[j + 1]);
        let d = b - a;
        let l2 = d.len2();
        let t = if l2 > 0.0 { ((x - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
        let d2 = (x - (a + d * t)).len2();
        if d2 < best.0 {
            best = (d2, j, t);
        }
    }
    let (d2, j, t) = best;
    let side = |j: usize| (band[j + 1] - band[j]).cross(x - band[j]) > 0.0;
    let corner = |k: usize| {
        let (sa, sb) = (side(k - 1), side(k));
        if sa == sb {
            sa
        } else {
            (band[k] - band[k - 1]).cross(band[k + 1] - band[k]) < 0.0
        }
    };
    let left = if t <= 0.0 && j > 0 {
        corner(j)
    } else if t >= 1.0 && j + 2 < n {
        corner(j + 1)
    } else {
        side(j)
    };
    (left, d2.sqrt())
}

/// Distance of `x` to the polyline `band`. O(band).
fn band_dist(band: &[Vec2], x: Vec2) -> f64 {
    band.windows(2).map(|w| seg_seg_dist(x, x, w[0], w[1])).fold(f64::INFINITY, f64::min)
}

/// Convex hull (counter-clockwise, Andrew's monotone chain). O(n log n).
fn hull(mut p: Vec<Vec2>) -> Vec<Vec2> {
    p.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    p.dedup_by(|a, b| a.x == b.x && a.y == b.y);
    if p.len() < 3 {
        return p;
    }
    let mut h: Vec<Vec2> = Vec::with_capacity(2 * p.len());
    for pass in 0..2 {
        let start = h.len();
        let it: Box<dyn Iterator<Item = &Vec2>> = if pass == 0 { Box::new(p.iter()) } else { Box::new(p.iter().rev()) };
        for &q in it {
            while h.len() >= start + 2 && (h[h.len() - 1] - h[h.len() - 2]).cross(q - h[h.len() - 2]) <= 0.0 {
                h.pop();
            }
            h.push(q);
        }
        h.pop();
    }
    h
}

/// The outline of `poly` grown by `r` in the planning norm, as points: every vertex's
/// circumscribed polygon (`dirs` x `r x grow`); for a convex feature their hull (the Minkowski
/// sum), else the points not nearer the feature than `r`. O(vertices x dirs).
fn grown(poly: &[Vec2], r: f64, dirs: &[Vec2], grow: f64) -> Vec<Vec2> {
    let cand: Vec<Vec2> = poly.iter().flat_map(|&v| dirs.iter().map(move |&u| v + u * (r * grow))).collect();
    if poly.len() < 3 {
        return cand;
    }
    let n = poly.len();
    let convex = (0..n).all(|i| (poly[(i + 1) % n] - poly[i]).cross(poly[(i + 2) % n] - poly[(i + 1) % n]) >= 0.0);
    if convex {
        hull(cand)
    } else {
        cand.into_iter().filter(|&x| !point_in_poly(x, poly) && poly_dist(x, poly) >= r - 1.0).collect()
    }
}

/// Bits of a point (exact coincidence).
fn key(p: Vec2) -> (u64, u64) {
    (p.x.to_bits(), p.y.to_bits())
}

/// The shortest path through the portals (`(left, right)` in the direction of travel, the
/// terminals as points). Corners with the portal index.
fn pull(ports: &[(Vec2, Vec2)]) -> Vec<(Vec2, usize)> {
    super::funnel::funnel(ports)
}

/// One stretch's obstacle points and their sides.
struct Points {
    pts: Vec<Vec2>,
    /// 0 start, 1 end, 2 left, 3 right, 4 frame (no side).
    lab: Vec<u8>,
    gate: Vec<Option<usize>>,
    w: Vec<f64>,
    /// The rough path's detour round the point ([`Detour`]).
    det: Vec<Detour>,
    seen: HashMap<(u64, u64), usize>,
    /// Points by [`MERGE`] cell.
    grid: HashMap<(i64, i64), Vec<usize>>,
    conflicts: usize,
}

impl Points {
    fn new(s: Vec2, e: Vec2, w: f64) -> Points {
        let mut seen = HashMap::new();
        seen.insert(key(s), 0);
        seen.entry(key(e)).or_insert(1);
        Points { pts: vec![s, e], lab: vec![0, 1], gate: vec![None, None], w: vec![w, w], det: vec![Detour::Never, Detour::Never], seen, grid: HashMap::new(), conflicts: 0 }
    }

    /// Add a point (the first of coincident points stays; one of the other side is a conflict).
    #[cfg(test)]
    fn add(&mut self, p: Vec2, left: bool, gate: Option<usize>, w: f64) {
        self.add_at(p, if left { 2 } else { 3 }, gate, w, Detour::Auto);
    }

    fn add_at(&mut self, p: Vec2, lab: u8, gate: Option<usize>, w: f64, det: Detour) {
        if !(p.x.is_finite() && p.y.is_finite()) {
            return;
        }
        // Points closer than [`MERGE`] are one obstacle (a detour's loop is [`EPS`] wide).
        let cell = |v: f64| (v / MERGE).floor() as i64;
        let (cx, cy) = (cell(p.x), cell(p.y));
        let mut found = self.seen.get(&key(p)).copied();
        if found.is_none() {
            'o: for dx in -1..=1 {
                for dy in -1..=1 {
                    for &i in self.grid.get(&(cx + dx, cy + dy)).map_or(&[][..], |v| v.as_slice()) {
                        if (self.pts[i] - p).len() <= MERGE {
                            found = Some(i);
                            break 'o;
                        }
                    }
                }
            }
        }
        if found.is_none() {
            self.grid.entry((cx, cy)).or_default().push(self.pts.len());
        }
        match found {
            Some(i) => {
                if self.lab[i] >= 2 && self.lab[i] <= 3 && lab <= 3 && self.lab[i] != lab {
                    self.conflicts += 1;
                }
            }
            None => {
                self.seen.insert(key(p), self.pts.len());
                self.pts.push(p);
                self.lab.push(lab);
                self.gate.push(gate);
                self.w.push(w);
                self.det.push(det);
            }
        }
    }

    /// A box round every point and the band, 1 mm out: the triangulation covers the rough path.
    fn frame(&mut self, band: &[Vec2]) {
        let (mut lo, mut hi) = (Vec2 { x: f64::INFINITY, y: f64::INFINITY }, Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY });
        for &p in self.pts.iter().chain(band) {
            lo = Vec2 { x: lo.x.min(p.x), y: lo.y.min(p.y) };
            hi = Vec2 { x: hi.x.max(p.x), y: hi.y.max(p.y) };
        }
        let m = 1.0e6;
        let (lo, hi) = (lo - Vec2 { x: m, y: m }, hi + Vec2 { x: m, y: m });
        let c = (lo + hi) * 0.5;
        for p in [lo, Vec2 { x: hi.x, y: lo.y }, hi, Vec2 { x: lo.x, y: hi.y }, Vec2 { x: c.x, y: lo.y }, Vec2 { x: hi.x, y: c.y }, Vec2 { x: c.x, y: hi.y }, Vec2 { x: lo.x, y: c.y }] {
            self.add_at(p, 4, None, 0.0, Detour::Never);
        }
    }
}

/// Whether the rough path goes round a point ([`rough_path`]).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Detour {
    /// When the point lies on the other side of the band than its label (nearest piece), from
    /// the nearest band point (a grown feature's point).
    Auto,
    /// Always, from this band point: a spoke tip the band's crossing of its cut lies beyond (the
    /// crossing, Dayan's spoke: the spoke `p a` or `b q` is crossed), an arc sample (its wrapped
    /// vertex).
    From(Vec2),
    /// Never (a spoke tip whose spoke the band does not cross, the terminals, the frame).
    Never,
}

/// The band crosses (or touches) the spoke from feature vertex `root` to its grown point `x`:
/// the band runs between the point and its feature (Dayan 1997 PDF p. 32: a band that crosses a
/// spoke goes round its end). O(band).
fn spoke_crossed(band: &[Vec2], x: Vec2, root: Vec2) -> bool {
    band.windows(2).any(|w| seg_touch(w[0], w[1], x, root))
}

/// The segments `a b` and `c d` cross or touch (an end on the other segment). O(1).
fn seg_touch(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let o = |p: Vec2, q: Vec2, r: Vec2| (q - p).cross(r - p);
    let (o1, o2, o3, o4) = (o(a, b, c), o(a, b, d), o(c, d, a), o(c, d, b));
    if o1 * o2 < 0.0 && o3 * o4 < 0.0 {
        return true;
    }
    let on = |p: Vec2, q: Vec2, r: Vec2| seg_seg_dist(r, r, p, q) <= ON;
    on(a, b, c) || on(a, b, d) || on(c, d, a) || on(c, d, b)
}

/// Distance under which a point counts as on a line (nm): its side is its label.
const ON: f64 = 0.5;
/// Half size of a detour round a point (nm).
const EPS: f64 = 5.0;
/// Points closer than this are one obstacle (nm).
const MERGE: f64 = 4.0 * EPS;

/// Detours whose bases coincide (within [`ON`]: a band vertex is the end of one piece and the
/// start of the next, its arclength one rounding apart) leave from one base in angle order; split
/// by the rounding, two of them would cross (qmtech st 359). `det` sorted by arclength, entries
/// (arclength, angle, point, base). O(d log d).
fn same_base(det: &mut [(f64, f64, usize, Vec2)]) {
    let mut a = 0;
    while a < det.len() {
        let mut b = a + 1;
        while b < det.len() && (det[b].3 - det[a].3).len() <= ON {
            b += 1;
        }
        if b - a > 1 {
            let (s0, x0) = (det[a].0, det[a].3);
            for e in &mut det[a..b] {
                e.0 = s0;
                e.3 = x0;
            }
            det[a..b].sort_by(|p, q| p.1.total_cmp(&q.1));
        }
        a = b;
    }
}

/// The rough path: the band with a detour round every point on the wrong side of it, so that
/// every labelled point lies on its own side (Dayan 1997 PDF p. 32: a band that crosses a spoke
/// goes round the spoke's tip). A detour ([`Step::Loop`]) leaves the band at its base (the band's
/// crossing of the spoke, or the nearest band point); the sleeve walk takes it out to the point,
/// once round it and back over the same edges, so it adds one turn round that point only.
/// Detours at one place leave in the order the path meets them (the angle from the backward
/// direction; one base for those within `ON`, [`same_base`]). O(points x band + d log d).
fn rough_path(band: &[Vec2], pp: &Points) -> Vec<Step> {
    let n = band.len();
    let mut cum = vec![0.0; n];
    for k in 1..n {
        cum[k] = cum[k - 1] + (band[k] - band[k - 1]).len();
    }
    // Nearest place on the band pieces that overlap arclengths `win`: (arclength, point,
    // incoming direction).
    let place = |x: Vec2, win: (f64, f64)| -> (f64, Vec2, Vec2) {
        let mut best = (f64::INFINITY, 0usize, 0.0);
        for j in 0..n - 1 {
            if cum[j + 1] < win.0 || cum[j] > win.1 {
                continue;
            }
            let d = band[j + 1] - band[j];
            let l2 = d.len2();
            let t = if l2 > 0.0 { ((x - band[j]).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let d2 = (x - (band[j] + d * t)).len2();
            if d2 < best.0 {
                best = (d2, j, t);
            }
        }
        let (_, j, t) = best;
        let seg = (band[j + 1] - band[j]).unit().unwrap_or(Vec2 { x: 1.0, y: 0.0 });
        // At a vertex the incoming direction is the piece before it.
        let din = if t <= 0.0 && j > 0 { (band[j] - band[j - 1]).unit().unwrap_or(seg) } else { seg };
        (cum[j] + t * (cum[j + 1] - cum[j]), band[j] + (band[j + 1] - band[j]) * t, din)
    };
    let mut det: Vec<(f64, f64, usize, Vec2)> = Vec::new();
    for i in 2..pp.pts.len() {
        if pp.lab[i] != 2 && pp.lab[i] != 3 {
            continue;
        }
        let p = pp.pts[i];
        let (l, d) = band_side(band, p);
        // The detour leaves at the nearest band point (such detours never cross each other: a
        // point of one lies nearer its own base); a spoke tip's or an arc sample's from the
        // nearest point of the band near its crossing or wrapped vertex.
        let all = (f64::NEG_INFINITY, f64::INFINITY);
        let (s, x, din) = match pp.det[i] {
            Detour::Never => continue,
            Detour::Auto if d <= ON || l == (pp.lab[i] == 2) => continue,
            // Nearest to an end of the band (behind the start or past the end): the band does
            // not pass it, it has no side to change.
            Detour::Auto => match place(p, all) {
                (s, _, _) if s <= ON || s >= cum[n - 1] - ON => continue,
                pl => pl,
            },
            Detour::From(x) if (p - x).len() <= ON => continue,
            Detour::From(x) => {
                let sx = place(x, all).0;
                let r = 2.0 * (p - x).len() + 1.0e3;
                place(p, (sx - r, sx + r))
            }
        };
        let u = (p - x).unit().unwrap_or(din);
        // The angle from the backward direction, turning toward the point's side of the band
        // (counter-clockwise for a point right of it): the order the path meets them round a
        // corner, past 180 degrees too.
        let l = din.cross(p - x) > 0.0 || (din.cross(p - x) == 0.0 && l);
        let ang = if l { cw(-din, u) } else { cw(u, -din) };
        det.push((s, ang, i, x));
    }
    det.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    same_base(&mut det);
    // Detours from one base along one ray (a spoke tip on an arc sample's ray): the farther
    // first. The nearer point lies beside the farther one's loop, and a loop round the pair is the
    // farther one's then the nearer one's; in the other order the path winds round the pair.
    let along = |a: &(f64, f64, usize, Vec2), b: &(f64, f64, usize, Vec2)| {
        let (pa, pb) = (pp.pts[a.2] - a.3, pp.pts[b.2] - b.3);
        (a.3 - b.3).len() <= ON && pa.dot(pb) > 0.0 && pa.cross(pb).abs() <= 2.0 * MERGE * pa.len().max(pb.len())
    };
    let mut swapped = true;
    while swapped {
        swapped = false;
        for k in 1..det.len() {
            if along(&det[k - 1], &det[k]) && (pp.pts[det[k - 1].2] - det[k - 1].3).len() < (pp.pts[det[k].2] - det[k].3).len() {
                det.swap(k - 1, k);
                swapped = true;
            }
        }
    }
    let mut out: Vec<Step> = Vec::with_capacity(n + 2 * det.len());
    let mut at = band[0];
    let to = |q: Vec2, out: &mut Vec<Step>, at: &mut Vec2| {
        if q != *at {
            out.push(Step::To(q));
            *at = q;
        }
    };
    let mut k = 1;
    for &(s, _, i, x) in &det {
        while k < n && cum[k] <= s {
            to(band[k], &mut out, &mut at);
            k += 1;
        }
        if (pp.pts[i] - x).len() <= ON {
            continue;
        }
        to(x, &mut out, &mut at);
        out.push(Step::Loop(i));
    }
    for &q in &band[k..] {
        to(q, &mut out, &mut at);
    }
    out
}

/// One step of the rough path ([`rough_path`], walked by [`sleeve`]).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// The straight piece from where the path is to this point.
    To(Vec2),
    /// One turn round obstacle point `i` from where the path is (its base on the band): out along
    /// the straight segment to the point, once round it (counter-clockwise for a left label, so it
    /// ends on the path's left), back the same way. The way back crosses exactly the edges of the
    /// way out in reverse, so the loop never winds round another point (a point on the segment
    /// counts as on `i`'s label side of it for both legs, as a thin loop on the other side would
    /// leave it).
    Loop(usize),
}

/// The rough path as points (debug print, plots): every loop drawn thin, out on one side of the
/// ray and back on the other round the point.
fn rough_points(start: Vec2, steps: &[Step], pp: &Points) -> Vec<Vec2> {
    let mut out = vec![start];
    let mut at = start;
    for st in steps {
        match *st {
            Step::To(q) => {
                out.push(q);
                at = q;
            }
            Step::Loop(i) => {
                let p = pp.pts[i];
                let Some(d) = (p - at).unit() else { continue };
                let nn = if pp.lab[i] == 2 { -d.perp() } else { d.perp() };
                out.extend([p + d * EPS + nn * EPS, p + d * EPS - nn * EPS, p - d * EPS - nn * EPS, p - d * EPS + nn * (0.5 * EPS), at]);
            }
        }
    }
    out
}

/// The sleeve's portals: the triangulation edges the rough path crosses from `pts[0]` to `pts[1]`
/// in a Delaunay triangulation of the points (Efrat, Kobourov & Lubiw 2006, PDF p. 9: the funnel
/// "follows the path through the triangulation"), a crossing straight back over the edge before
/// cancelled (the reduced sleeve of the homotopy class: Erickson's notes, "Crossing Sequences":
/// two paths are homotopic iff their reduced crossing sequences agree; adjacent equal crossings
/// cancel), as point
/// indices `(left, right)` in the direction of travel. A band piece closer than [`ON`] to a
/// vertex's line takes the vertex's label; a loop's leg gives such a vertex the loop point's
/// label side, the same on the way out and back ([`Step::Loop`]). `Err` when the path leaves the
/// triangulation or a straight piece crosses more faces than there are (the side test cycles).
/// O(n log n + crossings of the unreduced path).
fn sleeve(pp: &Points, rough: &[Step]) -> Result<Vec<(usize, usize)>, String> {
    let cdt = Cdt::build(&pp.pts, &[])?;
    let topo = cdt.topology();
    let nc = cdt.num_vertices();
    let mut inp = vec![usize::MAX; nc];
    for i in 0..pp.pts.len() {
        let c = cdt.vertex_index(i);
        if c < nc && inp[c] == usize::MAX {
            inp[c] = i;
        }
    }
    let lab = |c: usize| inp.get(c).filter(|&&i| i != usize::MAX).map_or(4, |&i| pp.lab[i]);
    let (cs, ce) = (cdt.vertex_index(0), cdt.vertex_index(1));
    if cs == ce {
        return Ok(Vec::new());
    }
    let mut face_of: HashMap<(usize, usize), usize> = HashMap::with_capacity(3 * topo.faces.len());
    for (fi, f) in topo.faces.iter().enumerate() {
        for k in 0..3 {
            face_of.insert((f[k], f[(k + 1) % 3]), fi);
        }
    }
    let pos = |c: usize| cdt.vertex_pos(c);
    // Side of vertex c against the line a -> b: true = left. Within ON of the line: `tie`, or
    // without one the vertex's own label.
    let left_of = |c: usize, a: Vec2, b: Vec2, tie: Option<bool>| {
        let d = b - a;
        let l = d.len();
        let o = d.cross(pos(c) - a);
        if l > 0.0 && o.abs() <= ON * l {
            tie.unwrap_or(lab(c) != 3)
        } else {
            o > 0.0
        }
    };
    // Inside (or on) face f: no edge has the point strictly outside.
    let inside = |f: [usize; 3], x: Vec2| (0..3).all(|k| (pos(f[(k + 1) % 3]) - pos(f[k])).cross(x - pos(f[k])) >= -ON * (pos(f[(k + 1) % 3]) - pos(f[k])).len());
    let mut star: Vec<Vec<usize>> = vec![Vec::new(); nc];
    for (fi, f) in topo.faces.iter().enumerate() {
        for &c in f {
            star[c].push(fi);
        }
    }
    // A piece that leaves a vertex: the face round it whose wedge holds the piece.
    let leave = |c: usize, a: Vec2, b: Vec2, tie: Option<bool>| {
        star[c].iter().copied().find(|&fi| {
            let f = topo.faces[fi];
            let k = f.iter().position(|&v| v == c).unwrap_or(0);
            !left_of(f[(k + 1) % 3], a, b, tie) && left_of(f[(k + 2) % 3], a, b, tie)
        })
    };
    // Round vertex c from face `from` to face `to` (clockwise: c stays on the right), the edges
    // crossed into `rec`; `full`: once all round (`to` = `from`).
    let round = |c: usize, from: usize, to: usize, cw: bool, full: bool, rec: &mut Vec<(usize, usize)>| -> Result<(), String> {
        let mut fi = from;
        let mut n = 0;
        while fi != to || (full && n == 0) {
            n += 1;
            if n > star[c].len() + 1 {
                return Err("the walk round a point on the path does not close".into());
            }
            let f = topo.faces[fi];
            let k = f.iter().position(|&v| v == c).unwrap_or(0);
            let (x1, x2) = (f[(k + 1) % 3], f[(k + 2) % 3]);
            let (l, r, key) = if cw { (x1, c, (x1, c)) } else { (c, x2, (c, x2)) };
            rec.push((l, r));
            let Some(&nf) = face_of.get(&key) else { return Err("the path left the triangulation (the hull)".into()) };
            fi = nf;
        }
        Ok(())
    };
    // A straight piece a -> b from face `fi` (which holds a): the edges it crosses into `rec`;
    // ends in the face that holds b, or with `to_v` the first face with that vertex. A piece
    // from a vertex leaves through the face that holds it (round an obstacle point there on its
    // label side). A straight piece crosses a face at most once: more steps than faces = a cycle.
    let nf = topo.faces.len();
    let walk = |mut fi: usize, a: Vec2, b: Vec2, to_v: Option<usize>, tie: Option<bool>, wi: usize, rec: &mut Vec<(usize, usize)>| -> Result<usize, String> {
        let f = topo.faces[fi];
        if let Some(&c) = f.iter().find(|&&c| (pos(c) - a).len() <= ON && Some(c) != to_v) {
            let Some(fl) = leave(c, a, b, tie) else { return Err("no face round a vertex on the path holds its next piece".into()) };
            // The path runs through an obstacle point: go round it on the side opposite its
            // label, crossing its spokes from the face it arrived in to the one it leaves by
            // (right label: clockwise, the point stays right; left: counter-clockwise).
            if matches!(lab(c), 2 | 3) {
                round(c, fi, fl, lab(c) == 3, false, rec)?;
            }
            fi = fl;
        }
        let mut steps = 0;
        loop {
            let f = topo.faces[fi];
            match to_v {
                Some(v) if f.contains(&v) => return Ok(fi),
                None if inside(f, b) => return Ok(fi),
                _ => {}
            }
            steps += 1;
            if steps > nf + 8 {
                return Err("the walk through the triangulation does not end".into());
            }
            if steps + 12 > nf + 8 && sleeve_dbg() {
                eprintln!("  walk piece {wi} {:?} -> {:?}: face {:?} labels {:?} sides {:?}", a, b, f.map(&pos), f.map(&lab), f.map(|c| left_of(c, a, b, tie)));
            }
            // The exit edge: from a right vertex to a left one (counter-clockwise), b beyond it.
            let mut next = None;
            for k in 0..3 {
                let (vi, vj) = (f[k], f[(k + 1) % 3]);
                if !left_of(vi, a, b, tie) && left_of(vj, a, b, tie) {
                    next = Some((vj, vi));
                    break;
                }
            }
            let Some((l, r)) = next else {
                if sleeve_dbg() {
                    eprintln!("  no exit: face {:?} labels {:?} seg {:?} -> {:?}", f.map(&pos), f.map(&lab), a, b);
                }
                return Err("no exit edge (the path runs along a vertex of both sides)".into());
            };
            rec.push((l, r));
            let Some(&nf2) = face_of.get(&(l, r)) else { return Err("the path left the triangulation (the hull)".into()) };
            fi = nf2;
        }
    };
    let mut out: Vec<(usize, usize)> = Vec::new();
    // A crossing straight back over the edge before cancels it (the reduced sleeve).
    let cross = |l: usize, r: usize, out: &mut Vec<(usize, usize)>| {
        if out.last() == Some(&(r, l)) {
            out.pop();
        } else {
            out.push((l, r));
        }
    };
    let Some(&f0) = star[cs].first() else { return Err("no face at the start".into()) };
    let (mut fi, mut at) = (f0, pp.pts[0]);
    // The last straight piece ends at the end vertex (the walk stops in a face of it).
    let Some(last) = rough.iter().rposition(|st| matches!(st, Step::To(_))) else { return Err("rough path of one point".into()) };
    let mut rec: Vec<(usize, usize)> = Vec::new();
    for (wi, st) in rough.iter().enumerate() {
        rec.clear();
        match *st {
            Step::To(b) => {
                fi = walk(fi, at, b, (wi == last).then_some(ce), None, wi, &mut rec)?;
                at = b;
            }
            Step::Loop(i) => {
                let c = cdt.vertex_index(i);
                if c >= nc || (pp.pts[i] - at).len() <= ON {
                    continue;
                }
                // Out to the point (points on the segment on its label side), once round it on
                // its label side, back over the same edges; the path is then where it left.
                let fp = walk(fi, at, pp.pts[i], Some(c), Some(pp.lab[i] == 2), wi, &mut rec)?;
                let legs = rec.len();
                round(c, fp, fp, pp.lab[i] == 3, true, &mut rec)?;
                for k in (0..legs).rev() {
                    let (l, r) = rec[k];
                    rec.push((r, l));
                }
            }
        }
        for &(l, r) in &rec {
            cross(l, r, &mut out);
        }
    }
    Ok(out.into_iter().map(|(l, r)| (inp[l], inp[r])).collect())
}

/// The side of `p` at the band's crossing of cut `c` (item `x`): left of the band. For a crossing
/// (kind 0) the band piece crossing the cut nearest the item's band position; for a wrap of `p`
/// (kind 1) or `q` (kind 2) the wrap's side. `None` when no piece crosses.
fn p_left_of(s: &Stretch, c: &super::sheet::Cut, x: &super::radii::CutItem) -> Option<bool> {
    let band = &s.band;
    match (x.kind, x.corner) {
        (0, _) => {
            let e = c.q - c.p;
            let mut best: Option<(f64, bool)> = None;
            for j in 0..band.len().saturating_sub(1) {
                let d = band[j + 1] - band[j];
                let den = d.cross(e);
                if den == 0.0 {
                    continue;
                }
                let w0 = c.p - band[j];
                let (sp, tc) = (w0.cross(e) / den, w0.cross(d) / den);
                if (-1e-9..=1.0 + 1e-9).contains(&sp) {
                    let score = (tc - x.t_band).abs();
                    if best.is_none_or(|bb| score < bb.0) {
                        best = Some((score, d.cross(c.p - band[j]) > 0.0));
                    }
                }
            }
            best.map(|b| b.1)
        }
        (k, Some(cr)) => {
            let left = s.wraps.get(cr).copied().flatten().is_some_and(|w| w.1);
            Some(if k == 1 { left } else { !left })
        }
        _ => None,
    }
}

/// The band's crossing point of the item (its band position `t_band` along the cut).
fn band_cross(c: &super::sheet::Cut, x: &super::radii::CutItem) -> Vec2 {
    c.p + (c.q - c.p) * x.t_band.clamp(0.0, 1.0)
}

/// Round h: make the pushes of one stretch agree before the rough path is built (Dayan 1997,
/// PDF p. 32: spoke creation "checks the routability of a sketch"; PDF pp. 25-26: the ERBS keeps
/// the spacing iff the class is routable, so where the spokes contradict each other there is no
/// legal ERBS to find, only a predicted violation).
/// 1. Opposite pushes: a pushed point (its spoke crossed by the band: a `From` detour, or an
///    `Auto` point on the other side of the band than its label) whose spoke (root -> point)
///    crosses the spoke of a pushed point of the other label asks the path to pass beyond it
///    on one side while the other asks for the opposite side at the same place (the room the two
///    spokes claim overlaps: over-full across two cuts). Neither pushes (esp st 79: the path
///    zigzagged between the two fans, 1.8 x its band).
/// 2. A point that does not push and lies on the other side of the band than its label is no
///    obstacle: its label is not enforced (a squeezed or pad-exit feature's tip, rules 4 / 4b, or
///    1 above), and kept on that side it hooks the path round it wherever another point's loop
///    sweeps over it (qmtech st 108: 805 such tips among 748 pushed ones, the ERBS 21 x its band).
///
/// O(c log c) for c candidates (a grid of the pushed spokes).
fn settle_pushes(band: &[Vec2], cand: &mut Vec<(Vec2, bool, usize, f64, Detour, Vec2)>) -> (Vec<usize>, usize) {
    let pushed = |&(p, left, _, _, det, _): &(Vec2, bool, usize, f64, Detour, Vec2)| match det {
        Detour::From(x) => (p - x).len() > ON,
        Detour::Auto => {
            let (l, d) = band_side(band, p);
            d > ON && l != left
        }
        Detour::Never => false,
    };
    let idx: Vec<usize> = (0..cand.len()).filter(|&i| pushed(&cand[i])).collect();
    let mut settled = Vec::new();
    let n0 = cand.len();
    if idx.iter().any(|&i| cand[i].1) && idx.iter().any(|&i| !cand[i].1) {
        // A grid of the pushed spokes' boxes; pairs of the other label that cross.
        let cell = 0.1e6;
        // Each spoke goes into the cells it passes (a crossing lies in a cell both pass).
        let mut grid: HashMap<(i64, i64), (Vec<usize>, Vec<usize>)> = HashMap::new();
        for &i in &idx {
            for c in seg_cells(cand[i].5, cand[i].0, cell) {
                let e = grid.entry(c).or_default();
                if cand[i].1 { e.0.push(i) } else { e.1.push(i) }
            }
        }
        let mut off: BTreeSet<usize> = BTreeSet::new();
        let mut cells: Vec<&(Vec<usize>, Vec<usize>)> = grid.values().filter(|e| !e.0.is_empty() && !e.1.is_empty()).collect();
        cells.sort();
        for (ls, rs) in cells {
            for &i in ls {
                for &j in rs {
                    if seg_cross(cand[i].5, cand[i].0, cand[j].5, cand[j].0) {
                        off.insert(i);
                        off.insert(j);
                    }
                }
            }
        }
        for &i in &off {
            cand[i].4 = Detour::Never;
            settled.push(cand[i].2);
        }
    }
    cand.retain(|&(p, left, _, _, det, _)| {
        if det != Detour::Never {
            return true;
        }
        let (l, d) = band_side(band, p);
        d <= ON || l == left
    });
    (settled, n0 - cand.len())
}

/// The grid cells (side `cell`) the segment `a b` passes, a superset at cell corners (Amanatides &
/// Woo traversal; a step that leaves through a corner also adds both side cells, a segment along
/// a cell line both rows). O(cells passed).
fn seg_cells(a: Vec2, b: Vec2, cell: f64) -> Vec<(i64, i64)> {
    let key = |v: f64| (v / cell).floor() as i64;
    let on_line = |v: f64| (v / cell).floor() == v / cell;
    let d = b - a;
    let (sx, sy) = (if d.x > 0.0 { 1 } else { -1 }, if d.y > 0.0 { 1 } else { -1 });
    // A start on a cell line, going down that axis, starts in the cell below the line; an end on
    // one, coming up, ends in the cell below it.
    let (mut x, mut y) = (key(a.x) - (d.x < 0.0 && on_line(a.x)) as i64, key(a.y) - (d.y < 0.0 && on_line(a.y)) as i64);
    let (x1, y1) = (key(b.x) - (d.x > 0.0 && on_line(b.x)) as i64, key(b.y) - (d.y > 0.0 && on_line(b.y)) as i64);
    let mut out = vec![(x, y)];
    let next = |k: i64, s: i64, p: f64, dv: f64| -> f64 {
        if dv == 0.0 {
            f64::INFINITY
        } else {
            let edge = (if s > 0 { k + 1 } else { k }) as f64 * cell;
            ((edge - p) / dv).max(0.0)
        }
    };
    let (mut tx, mut ty) = (next(x, sx, a.x, d.x), next(y, sy, a.y, d.y));
    let (dtx, dty) = (if d.x == 0.0 { f64::INFINITY } else { cell / d.x.abs() }, if d.y == 0.0 { f64::INFINITY } else { cell / d.y.abs() });
    let n = (x1 - x).abs() + (y1 - y).abs();
    for _ in 0..n {
        if tx.is_finite() && ty.is_finite() && (tx - ty).abs() <= 1e-12 * tx.max(ty).max(1.0) {
            out.push((x + sx, y));
            out.push((x, y + sy));
            x += sx;
            y += sy;
            tx += dtx;
            ty += dty;
        } else if tx < ty {
            x += sx;
            tx += dtx;
        } else {
            y += sy;
            ty += dty;
        }
        out.push((x, y));
        if (x, y) == (x1, y1) {
            break;
        }
    }
    if out.last() != Some(&(x1, y1)) {
        out.push((x1, y1));
    }
    // A segment along a cell line lies in the cells on both sides of it.
    if d.y == 0.0 && on_line(a.y) {
        let below: Vec<_> = out.iter().map(|&(x, y)| (x, y - 1)).collect();
        out.extend(below);
    }
    if d.x == 0.0 && on_line(a.x) {
        let left: Vec<_> = out.iter().map(|&(x, y)| (x - 1, y)).collect();
        out.extend(left);
    }
    out
}

/// The segments `a b` and `c d` cross properly (each has the other's ends strictly on both sides).
/// O(1).
fn seg_cross(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let o = |p: Vec2, q: Vec2, r: Vec2| (q - p).cross(r - p);
    o(a, b, c) * o(a, b, d) < 0.0 && o(c, d, a) * o(c, d, b) < 0.0
}

/// The ERBS of stretch `si` (module documentation, steps 1-4). `items`: its cut items `(cut,
/// item)`; `radius`: stack radius per (vertex, stretch, corner); `skip`: features at the ends of
/// over-full cuts it is on. O(n log n) in the obstacle points.
#[allow(clippy::too_many_arguments)]
fn erbs_stretch(sh: &LayerSheet, fg: &Grid, fclear: f64, s: &Stretch, si: usize, o: &LayerOrder, items: &[(usize, usize)], radius: &BTreeMap<(usize, usize, usize), f64>, skip: &BTreeSet<usize>, opts: &ErbsOpts) -> Result<Erbs, String> {
    let band = &s.band;
    let (c0, c1) = (band[0], band[band.len() - 1]);
    let (dirs, grow) = norm_dirs(sh.norm, opts.arc_step_deg);
    let mut gates: Vec<Gate> = Vec::new();
    // Obstacle candidates before the corridor filter: (point, left, gate, width).
    // (point, left, gate, width, detour, the spoke root: cut end, wrapped vertex, feature vertex).
    let mut cand: Vec<(Vec2, bool, usize, f64, Detour, Vec2)> = Vec::new();

    // The stretch's own terminal pads (not a via end): a cut from such a pad that the band crosses
    // at the pad (at the cut's empty-spoke root; the band starts inside the pad) is no constraint
    // on the path, which ends in the pad anyway (three_diagonals: its tips on the pad's edge
    // pulled the path along the edge next to the neighbour pad).
    let tpads: Vec<&[Vec2]> = [c0, c1]
        .iter()
        .enumerate()
        .filter(|(end, _)| !s.via_ends[*end])
        .filter_map(|(_, &c)| sh.feats.iter().find(|ft| ft.poly.len() >= 3 && matches!(ft.bound, Bound::Pad { net: Some(m), .. } if m == s.net) && point_in_poly(c, &ft.poly)))
        .map(|ft| ft.poly.as_slice())
        .collect();
    let in_pad = |x: Vec2| tpads.iter().any(|poly| point_in_poly(x, poly) || poly_dist(x, poly) < 0.5 * s.w);

    // Features of other nets the wire's own ends lie in the clearance of (the end is within half
    // the width plus the feature's clearance of it): no room the band could keep there, the pad
    // exit's neck (a predicted violation). As the features of an over-full cut (`skip`) they are
    // no obstacle and their spokes push nothing (qmtech: a 0.63 mm wire leaving a 0.3 mm pad at
    // 0.5 mm pitch, the neighbours' long cuts' tips 0.505 mm out on both sides, the ERBS wound
    // round them, 4.6 x its band).
    let it = Item { net: s.net, w: s.w, s: s.s };
    // The placement margin on every feature clearance (the tips and arcs carry it from radii).
    let em = opts.margin.max(0.0);
    let mut exit: Vec<usize> = Vec::new();
    {
        let rmax = 0.5 * s.w + fclear.max(s.s);
        let (mut near, mut one) = (BTreeSet::new(), BTreeSet::new());
        for c in [c0, c1] {
            fg.query(c - Vec2 { x: rmax, y: rmax }, c + Vec2 { x: rmax, y: rmax }, &mut one);
            near.append(&mut one);
        }
        for &f in &near {
            let ft = &sh.feats[f];
            let cl = ft.bound.clearance(&it);
            if cl > 0.0 && ft.poly.len() >= 3 && [c0, c1].iter().any(|&c| point_in_poly(c, &ft.poly) || poly_dist(c, &ft.poly) < 0.5 * s.w + cl + em) {
                exit.push(f);
            }
        }
    }
    let exit_set: BTreeSet<usize> = exit.iter().copied().collect();
    let no_push = |f: usize| skip.contains(&f) || exit_set.contains(&f);

    // 1a. Cut items, grouped per cut in their order from p: the sides alternate along the cut.
    let mut per_cut: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &(ci, ii) in items {
        per_cut.entry(ci).or_default().push(ii);
    }
    for (&ci, iis) in per_cut.iter_mut() {
        iis.sort_unstable();
        let cu = &o.cuts[ci];
        let c = &sh.cuts[cu.cut];
        let first = iis.iter().find_map(|&ii| p_left_of(s, c, &cu.items[ii]));
        let Some(region0) = first else { continue };
        for (k, &ii) in iis.iter().enumerate() {
            let x = &cu.items[ii];
            let lo = x.tip_p.min(c.cap - x.tip_q);
            let hi = (c.cap - x.tip_q).max(lo);
            let (a, b) = (tip_point(c, lo), tip_point(c, hi));
            let open = (a - c.p).len() < 1.0 || (c.q - b).len() < 1.0;
            if open && x.kind == 0 && !tpads.is_empty() {
                let root = if (a - c.p).len() < 1.0 { c.p } else { c.q };
                if in_pad(root) && in_pad(band_cross(c, x)) {
                    continue;
                }
            }
            // The side of p at this crossing; where no band piece crosses, the sides alternate
            // along the cut from the first crossing (each crossing of a simple band changes side).
            let a_left = p_left_of(s, c, x).unwrap_or(region0 ^ (k % 2 == 1));
            let gi = gates.len();
            gates.push(Gate { a, b, w: x.w_tip, src: GateSrc::Cut(ci), p: c.p, q: c.q, p_left: a_left });
            // The band crosses the spoke `p a` (its crossing before `a` along the cut) or `b q`
            // (after `b`): the rough path goes round that tip from the crossing.
            // A tip at a feature of an over-full cut this stretch is on (`skip`: the cut's squeeze
            // is its predicted violation) asks for room the over-full cut does not have: the
            // band keeps its side of it, no detour (qmtech: a 0.63 mm wire between two pads 0.7
            // mm apart, the tips of their long cuts 0.505 mm from each pad on both sides, the
            // ERBS wound round them, 4.6 x its band).
            let xb = band_cross(c, x);
            let at = x.t_band.clamp(0.0, 1.0) * c.cap;
            let da = if at < lo - ON && !no_push(c.fp) { Detour::From(xb) } else { Detour::Never };
            let db = if at > hi + ON && !no_push(c.fq) { Detour::From(xb) } else { Detour::Never };
            // A tip at its cut's end on one of the stretch's own terminal pads holds no room (no
            // item before it, no clearance to its own copper): the wire may pass over that pad.
            if !((a - c.p).len() < 1.0 && in_pad(c.p)) {
                cand.push((a, a_left, gi, x.w_tip, da, c.p));
            }
            if !((c.q - b).len() < 1.0 && in_pad(c.q)) {
                cand.push((b, !a_left, gi, x.w_tip, db, c.q));
            }
        }
    }

    // 1b. The arcs of the wraps at their stack radii, inside each wrap's wedge.
    let pidx = s.pass_index();
    for k in 1..band.len().saturating_sub(1) {
        let Some((v, left)) = s.wraps[k] else { continue };
        let Some(&rho) = radius.get(&(v, si, k)) else { continue };
        let at = sh.pos[v];
        let w = s.width_at_idx(&pidx, band[k]);
        let poly = sh.vfeat[v].map(|f| &sh.feats[f].poly).filter(|p| p.len() >= 3);
        // A feature the band runs through, or whose vertex it wraps from the inside (its body in
        // the wrap's wedge, `wraps_inside`), has no side (1c leaves it out, a predicted
        // violation): its vertex holds no arc either. The rays of the wedge run over its body, so
        // the arc would put the path round the body's far side (esp L0 st 6: samples at 1.09 mm
        // round a 0.31 mm pad, the ERBS 3.7 x its band) or into it at the stack radius.
        if poly.is_some_and(|p| band.windows(2).any(|w| seg_through(w[0], w[1], p)) || wraps_inside(band, p, left)) {
            continue;
        }
        for &u in &dirs {
            if wedge_share(band[k] - band[k - 1], band[k + 1] - band[k], left, u).is_none() {
                continue;
            }
            // Past the arc, and past the feature's own rho-offset along the ray (a ray over a
            // side of the feature leaves the offset later than the corner's circle: Popov p. 119).
            let t = poly.map_or(0.0, |p| exit_offset(at, u, rho, p));
            let a = at + u * (rho * grow).max(t);
            let gi = gates.len();
            gates.push(Gate { a, b: a, w, src: GateSrc::Arc(v), p: at, q: at, p_left: left });
            cand.push((a, left, gi, w, Detour::From(band[k]), at));
        }
    }

    // The corridor: as far as any obstacle on the wrong side of the band pushes the path, plus
    // twice the widest grown feature and 0.5 mm.
    let rmax = 0.5 * s.w + fclear.max(s.s);
    let push = cand.iter().map(|&(p, left, _, _, det, _)| {
        let (l, d) = band_side(band, p);
        match det {
            Detour::From(_) => d,
            Detour::Auto if l != left => d,
            _ => 0.0,
        }
    });
    let push = push.fold(0.0, f64::max).min(5.0e6);
    let reach = push + 2.0 * rmax + 0.5e6;

    // 1c. Features of other nets near the band, grown by the half width and their clearance.
    let (lo, hi) = band.iter().fold((Vec2 { x: f64::INFINITY, y: f64::INFINITY }, Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY }), |(lo, hi), p| (Vec2 { x: lo.x.min(p.x), y: lo.y.min(p.y) }, Vec2 { x: hi.x.max(p.x), y: hi.y.max(p.y) }));
    let mut near = BTreeSet::new();
    fg.query(lo - Vec2 { x: reach, y: reach }, hi + Vec2 { x: reach, y: reach }, &mut near);
    let mut through = Vec::new();
    let wrapped: BTreeMap<usize, bool> = s.wraps.iter().flatten().filter_map(|&(v, l)| sh.vfeat[v].map(|f| (f, l))).collect();
    for &f in &near {
        let ft = &sh.feats[f];
        let cl = ft.bound.clearance(&it);
        if cl <= 0.0 || no_push(f) || ft.poly.is_empty() {
            continue;
        }
        let d = band.windows(2).map(|w| seg_poly_dist(w[0], w[1], &ft.poly)).fold(f64::INFINITY, f64::min);
        if d > reach {
            continue;
        }
        // The band runs through the feature (a violation the plan accepted, costly not failed):
        // it has no side, so it is no obstacle; points of both sides would wind the class
        // round each one (Dayan PDF p. 26: the ERBS keeps the spacing of a feasible sketch only).
        if band.windows(2).any(|w| seg_through(w[0], w[1], &ft.poly)) {
            through.push(f);
            continue;
        }
        let left = match wrapped.get(&f) {
            // The band wraps a vertex of the feature with the feature's body on the other side
            // (it passes the vertex on the inside): through it as well.
            Some(&l) if wraps_inside(band, &ft.poly, l) => {
                through.push(f);
                continue;
            }
            Some(&l) => l,
            None => {
                let nv = ft.poly.iter().copied().min_by(|a, b| band_dist(band, *a).total_cmp(&band_dist(band, *b))).unwrap_or(ft.poly[0]);
                let (l, dn) = band_side(band, nv);
                if dn > 1.0 {
                    l
                } else {
                    let cen = ft.poly.iter().fold(Vec2::default(), |m, &p| m + p) * (1.0 / ft.poly.len() as f64);
                    band_side(band, cen).0
                }
            }
        };
        let r = 0.5 * s.w + cl + em;
        for x in grown(&ft.poly, r, &dirs, grow) {
            let root = ft.poly.iter().copied().min_by(|a, b| (*a - x).len2().total_cmp(&(*b - x).len2())).unwrap_or(x);
            let gi = gates.len();
            gates.push(Gate { a: x, b: x, w: s.w, src: GateSrc::Feature(f), p: root, q: root, p_left: left });
            // A grown point lies on the other side of the band than its feature only where the
            // band runs between the two (it dips into the feature's clearance): then the rough
            // path goes round it. Elsewhere its side is the band's own (a point past an end of
            // the band, or off a short band's corner, sided by the extended pieces; qmtech: big
            // pads 1-1.4 mm off a 0.5 mm band at a terminal, the ERBS 16 x its band).
            let dips = spoke_crossed(band, x, root);
            cand.push((x, left, gi, s.w, if dips { Detour::Auto } else { Detour::Never }, root));
        }
    }

    // The points: tips and arcs first (a coincident feature point is the same obstacle), the
    // corridor only, then the frame.
    let mut pp = Points::new(c0, c1, s.w);
    let (settled, unenforced) = settle_pushes(band, &mut cand);
    let mut settled: Vec<GateSrc> = settled.into_iter().map(|g| gates[g].src).collect();
    settled.sort_by_key(|g| match *g {
        GateSrc::Cut(c) => (0, c),
        GateSrc::Arc(v) => (1, v),
        GateSrc::Feature(f) => (2, f),
    });
    settled.dedup();
    for &(p, left, gi, w, det, _) in &cand {
        if band_dist(band, p) <= reach {
            pp.add_at(p, if left { 2 } else { 3 }, Some(gi), w, det);
        }
    }
    let n_obs = pp.pts.len() - 2;
    pp.frame(band);

    // 2. The rough path and its sleeve, 3. the funnel.
    let rough = rough_path(band, &pp);
    let dbg = sleeve_dbg();
    let mm = |p: Vec2| (p.x / 1e6, p.y / 1e6);
    if dbg {
        eprintln!("ERBS sleeve st {si}: band {:?}", band.iter().map(|&p| mm(p)).collect::<Vec<_>>());
        eprintln!("  points {:?}", pp.pts.iter().zip(&pp.lab).map(|(&p, &l)| (mm(p), l)).collect::<Vec<_>>());
        // Per point: source (C cut, A arc vertex, F feature, - none) and detour (N never, A auto, F from).
        let src = |i: usize| -> String {
            let g = pp.gate[i].map(|g| match gates[g].src {
                GateSrc::Cut(c) => format!("C{c}"),
                GateSrc::Arc(v) => format!("A{v}"),
                GateSrc::Feature(f) => format!("F{f}"),
            });
            let d = match pp.det[i] {
                Detour::Never => "N",
                Detour::Auto => "A",
                Detour::From(_) => "F",
            };
            format!("{}{}", g.unwrap_or_else(|| "-".into()), d)
        };
        eprintln!("  srcs {:?}", (0..pp.pts.len()).map(src).collect::<Vec<_>>());
        eprintln!("  rough {:?}", rough_points(c0, &rough, &pp).iter().map(|&p| mm(p)).collect::<Vec<_>>());
    }
    let portals = sleeve(&pp, &rough)?;
    if dbg {
        eprintln!("  portals {:?}", portals.iter().map(|&(l, r)| (mm(pp.pts[l]), mm(pp.pts[r]))).collect::<Vec<_>>());
    }
    let mut ports: Vec<(Vec2, Vec2)> = Vec::with_capacity(portals.len() + 2);
    ports.push((c0, c0));
    ports.extend(portals.iter().map(|&(l, r)| (pp.pts[l], pp.pts[r])));
    ports.push((c1, c1));
    let corners = pull(&ports);
    let mut pts = Vec::with_capacity(corners.len());
    let mut w = Vec::with_capacity(corners.len());
    let mut at = Vec::with_capacity(corners.len());
    for (k, &(p, _)) in corners.iter().enumerate() {
        let i = if k == 0 || k + 1 == corners.len() { None } else { pp.seen.get(&key(p)).copied() };
        pts.push(p);
        w.push(i.map_or(s.w, |i| pp.w[i]));
        at.push(i.and_then(|i| pp.gate[i]));
    }
    let path = simplify(&pts, 0.5);
    if dbg {
        eprintln!("  path {:?}", path.iter().map(|&p| (p.x / 1e6, p.y / 1e6)).collect::<Vec<_>>());
    }
    Ok(Erbs { stretch: si, len: poly_len(&path), band_len: poly_len(band), pts, w, at, path, gate_list: gates, points: n_obs, portals: portals.len(), conflicts: pp.conflicts, through, exit, settled, unenforced, converged: true })
}

/// The ERBS of every stretch of `mine` on this layer (module documentation). Each stretch is
/// independent of the others (its obstacles come from the plan's tips, radii and the sheet, never
/// from another ERBS), so the result does not depend on the order of `mine`.
pub fn erbs_layer(sh: &LayerSheet, sts: &[Stretch], mine: &[usize], o: &LayerOrder, opts: &ErbsOpts) -> Vec<Erbs> {
    erbs_layer_with(sh, sts, mine, o, opts).es
}

/// [`erbs_layer`] with the stretches whose sleeve failed and the count the time limit cut.
pub fn erbs_layer_with(sh: &LayerSheet, sts: &[Stretch], mine: &[usize], o: &LayerOrder, opts: &ErbsOpts) -> LayerErbs {
    let mut per: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    for (ci, cu) in o.cuts.iter().enumerate() {
        for (ii, x) in cu.items.iter().enumerate() {
            per.entry(x.stretch).or_default().push((ci, ii));
        }
    }
    let mut radius: BTreeMap<(usize, usize, usize), f64> = BTreeMap::new();
    for st in &o.stacks {
        for (i, (&s, &k)) in st.order.iter().zip(&st.corner).enumerate() {
            if let Some(&r) = st.radius.get(i) {
                radius.insert((st.v, s, k), r);
            }
        }
    }
    let mut squeezed: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for cu in o.cuts.iter().filter(|cu| cu.squeeze > 0.0) {
        let c = &sh.cuts[cu.cut];
        for x in &cu.items {
            squeezed.entry(x.stretch).or_default().extend([c.fp, c.fq]);
        }
    }
    let (fg, fclear) = feat_grid(&sh.feats, 0.5e6);
    let mut own: Vec<usize> = mine.to_vec();
    own.sort_unstable();
    own.dedup();
    let none = Vec::new();
    let no_skip = BTreeSet::new();
    let time = std::env::var_os("TOPO_ERBS_TIME").is_some();
    let mut out = LayerErbs::default();
    for si in own.into_iter().filter(|&si| sts[si].band.len() >= 2) {
        if opts.expired() {
            out.timed_out += 1;
            continue;
        }
        let t = std::time::Instant::now();
        let s = &sts[si];
        let r = if opts.fail_first && out.es.is_empty() && out.failed.is_empty() {
            Err("injected sleeve failure (test hook)".to_string())
        } else {
            erbs_stretch(sh, &fg, fclear, s, si, o, per.get(&si).unwrap_or(&none), &radius, squeezed.get(&si).unwrap_or(&no_skip), opts)
        };
        // Debug: `TOPO_ERBS_SLEEVE=bad` prints the sleeve of a failed stretch or one longer
        // than 1.3 x its band (run again with the print on).
        if std::env::var_os("TOPO_ERBS_SLEEVE").is_some_and(|v| v == "bad") && r.as_ref().map_or(true, |e| e.len > 1.3 * e.band_len) {
            eprintln!("ERBS bad st {si} wire {} net {} layer {}: {}", s.wire, s.net, sh.l, r.as_ref().map_or_else(|m| m.clone(), |e| format!("len {:.3} mm band {:.3} mm", e.len / 1e6, e.band_len / 1e6)));
            SLEEVE_AGAIN.with(|c| c.set(true));
            let _ = erbs_stretch(sh, &fg, fclear, s, si, o, per.get(&si).unwrap_or(&none), &radius, squeezed.get(&si).unwrap_or(&no_skip), opts);
            SLEEVE_AGAIN.with(|c| c.set(false));
        }
        if time {
            match &r {
                Ok(e) => eprintln!("ERBS time st {si} wire {} points {} portals {} conflicts {} len/band {:.4} {:.2} ms", s.wire, e.points, e.portals, e.conflicts, e.len / e.band_len.max(1.0), t.elapsed().as_secs_f64() * 1e3),
                Err(m) => eprintln!("ERBS time st {si} wire {} FAILED {m} {:.2} ms", s.wire, t.elapsed().as_secs_f64() * 1e3),
            }
        }
        match r {
            Ok(e) => out.es.push(e),
            Err(m) => out.failed.push((si, m)),
        }
    }
    out
}

/// `pts` without repeated points and without points within `tol` of the line of their
/// neighbours.
fn simplify(pts: &[Vec2], tol: f64) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::with_capacity(pts.len());
    for &p in pts {
        if out.last().is_some_and(|&q| (q - p).len() <= tol) {
            continue;
        }
        while out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let d = p - a;
            let off = if d.len() > 0.0 { d.cross(b - a).abs() / d.len() } else { (b - a).len() };
            let inside = (b - a).dot(b - p) <= 0.0;
            if off <= tol && inside {
                out.pop();
            } else {
                break;
            }
        }
        out.push(p);
    }
    out
}

pub(super) fn poly_len(p: &[Vec2]) -> f64 {
    p.windows(2).map(|w| (w[1] - w[0]).len()).sum()
}

/// Distance along the ray `at + t u` (`at` on the convex polygon's boundary, `u` outward) where
/// the distance to `poly` reaches `rho` (bisection, 1 nm). O(sides x 40).
fn exit_offset(at: Vec2, u: Vec2, rho: f64, poly: &[Vec2]) -> f64 {
    let (mut lo, mut hi) = (0.0, rho);
    let mut n = 0;
    while poly_dist(at + u * hi, poly) < rho && n < 40 {
        lo = hi;
        hi *= 2.0;
        n += 1;
    }
    for _ in 0..40 {
        if hi - lo <= 1.0 {
            break;
        }
        let m = 0.5 * (lo + hi);
        if poly_dist(at + u * m, poly) < rho {
            lo = m;
        } else {
            hi = m;
        }
    }
    hi
}

/// A uniform grid of feature boxes and the largest feature clearance (for the bare test).
fn feat_grid(feats: &[Feature], cell: f64) -> (Grid, f64) {
    let mut g = Grid { cell, cells: BTreeMap::new() };
    let mut fclear: f64 = 0.0;
    for (f, ft) in feats.iter().enumerate() {
        if ft.poly.is_empty() {
            continue;
        }
        let lo = ft.poly.iter().fold(Vec2 { x: f64::INFINITY, y: f64::INFINITY }, |m, p| Vec2 { x: m.x.min(p.x), y: m.y.min(p.y) });
        let hi = ft.poly.iter().fold(Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY }, |m, p| Vec2 { x: m.x.max(p.x), y: m.y.max(p.y) });
        g.insert(lo, hi, f);
        fclear = fclear.max(match ft.bound {
            Bound::Pad { clear, .. } | Bound::Hole { clear } | Bound::Edge { clear } => clear,
            Bound::Open => 0.0,
        });
    }
    (g, fclear)
}

/// A uniform grid of boxes (cell `cell` nm).
struct Grid {
    cell: f64,
    cells: BTreeMap<(i64, i64), Vec<usize>>,
}

impl Grid {
    fn key(&self, p: Vec2) -> (i64, i64) {
        ((p.x / self.cell).floor() as i64, (p.y / self.cell).floor() as i64)
    }

    fn insert(&mut self, lo: Vec2, hi: Vec2, id: usize) {
        let (a, b) = (self.key(lo), self.key(hi));
        for x in a.0..=b.0 {
            for y in a.1..=b.1 {
                self.cells.entry((x, y)).or_default().push(id);
            }
        }
    }

    fn query(&self, lo: Vec2, hi: Vec2, out: &mut BTreeSet<usize>) {
        out.clear();
        let (a, b) = (self.key(lo), self.key(hi));
        for x in a.0..=b.0 {
            for y in a.1..=b.1 {
                if let Some(v) = self.cells.get(&(x, y)) {
                    out.extend(v.iter().copied());
                }
            }
        }
    }
}

fn bbox(a: Vec2, b: Vec2, r: f64) -> (Vec2, Vec2) {
    (Vec2 { x: a.x.min(b.x) - r, y: a.y.min(b.y) - r }, Vec2 { x: a.x.max(b.x) + r, y: a.y.max(b.y) + r })
}

/// Whether segment `a b` runs through the inside of polygon `poly` deeper than [`DEEP`]: a
/// piece between two of its crossings with the outline has its middle inside. O(sides log sides).
fn seg_through(a: Vec2, b: Vec2, poly: &[Vec2]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let d = b - a;
    let mut ts = vec![0.0, 1.0];
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        let e = q - p;
        let den = d.cross(e);
        if den == 0.0 {
            continue;
        }
        let w0 = p - a;
        let (t, u) = (w0.cross(e) / den, w0.cross(d) / den);
        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
            ts.push(t);
        }
    }
    ts.sort_by(f64::total_cmp);
    ts.windows(2).any(|w| {
        let m = a + d * (0.5 * (w[0] + w[1]));
        point_in_poly(m, poly) && (0..poly.len()).all(|i| seg_seg_dist(m, m, poly[i], poly[(i + 1) % poly.len()]) > DEEP)
    })
}

/// Whether the feature `poly` lies on the other side of the band than `left`, the side of its
/// wrapped vertex: the band passes that vertex on the inside of the feature. The side of the
/// centroid, more than [`DEEP`] off the band.
fn wraps_inside(band: &[Vec2], poly: &[Vec2], left: bool) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let cen = poly.iter().fold(Vec2::default(), |m, &p| m + p) * (1.0 / poly.len() as f64);
    let (l, d) = band_side(band, cen);
    d > DEEP && l != left
}

/// Depth inside a feature that counts as running through it (1 µm).
const DEEP: f64 = 1_000.0;

/// Distance of segment `a b` to polygon `poly` (0 when it touches or lies inside; a single point
/// is a point feature).
fn seg_poly_dist(a: Vec2, b: Vec2, poly: &[Vec2]) -> f64 {
    if poly.len() == 1 {
        return seg_seg_dist(a, b, poly[0], poly[0]);
    }
    if poly.len() >= 3 && (point_in_poly(a, poly) || point_in_poly(b, poly)) {
        return 0.0;
    }
    (0..poly.len()).map(|i| seg_seg_dist(a, b, poly[i], poly[(i + 1) % poly.len()])).fold(f64::INFINITY, f64::min)
}

/// Check the ERBS of a layer against each other and the sheet's features; violations by party,
/// the worst place each, classed predicted (an over-full cut carries both, or the order reports
/// the pair crossing) or `erbs_gap`. Deterministic.
pub fn check_layer(sh: &LayerSheet, sts: &[Stretch], o: &LayerOrder, es: &[Erbs]) -> Vec<Violation> {
    // Predicted pairs: over-full cuts and the order's crossing pairs.
    let mut pred_pair: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut pred_feat: BTreeSet<(usize, usize)> = BTreeSet::new();
    for cu in &o.cuts {
        if cu.squeeze <= 0.0 {
            continue;
        }
        let c = &sh.cuts[cu.cut];
        let ss: BTreeSet<usize> = cu.items.iter().map(|x| x.stretch).collect();
        for &a in &ss {
            pred_feat.insert((a, c.fp));
            pred_feat.insert((a, c.fq));
            for &b in &ss {
                pred_pair.insert((a.min(b), a.max(b)));
            }
        }
    }
    for d in &o.report.examples {
        pred_pair.insert((d.a.min(d.b), d.a.max(d.b)));
    }
    // Through features: predicted along the whole stretch. Exit features (rule 4b): only at the
    // pad exit (`exit_run`), not along the stretch.
    let mut pred_exit: BTreeSet<(usize, usize)> = BTreeSet::new();
    for e in es {
        for &f in &e.through {
            pred_feat.insert((e.stretch, f));
        }
        for &f in &e.exit {
            pred_exit.insert((e.stretch, f));
        }
        // Settled pushes (round h rule 4): the over-full room of two cuts, as an over-full cut.
        for g in &e.settled {
            match *g {
                GateSrc::Cut(ci) => {
                    let cu = &o.cuts[ci];
                    let c = &sh.cuts[cu.cut];
                    pred_feat.insert((e.stretch, c.fp));
                    pred_feat.insert((e.stretch, c.fq));
                    for x in &cu.items {
                        pred_pair.insert((e.stretch.min(x.stretch), e.stretch.max(x.stretch)));
                    }
                }
                GateSrc::Arc(v) => {
                    if let Some(f) = sh.vfeat[v] {
                        pred_feat.insert((e.stretch, f));
                    }
                }
                GateSrc::Feature(f) => {
                    pred_feat.insert((e.stretch, f));
                }
            }
        }
    }
    // Segments: (erbs index, point index), width, half reach.
    let segs: Vec<(usize, usize)> = es.iter().enumerate().flat_map(|(i, e)| (0..e.pts.len().saturating_sub(1)).map(move |k| (i, k))).collect();
    let smax = es.iter().map(|e| sts[e.stretch].s).fold(0.0, f64::max);
    let wmax = es.iter().flat_map(|e| e.w.iter().copied()).fold(0.0, f64::max);
    let reach = wmax + smax;
    let cell = (4.0 * reach).max(0.5e6);
    let mut grid = Grid { cell, cells: BTreeMap::new() };
    for (id, &(i, k)) in segs.iter().enumerate() {
        let (lo, hi) = bbox(es[i].pts[k], es[i].pts[k + 1], 0.0);
        grid.insert(lo, hi, id);
    }
    let seg_w = |i: usize, k: usize| es[i].w[k].min(es[i].w[k + 1]);
    // Rule 4b: the pad exit of ERBS `i` at exit feature `f` = the segments from either end that
    // violate `f` without a break (the neck's own violation, as long as the wire runs along the
    // neighbour); the rest of the stretch is not predicted. Returns (a, b): segments < a or >= b.
    let exit_run = |i: usize, f: usize| -> (usize, usize) {
        let sa = &sts[es[i].stretch];
        let n = es[i].pts.len() - 1;
        let viol = |k: usize| -> bool {
            let (p, q) = (es[i].pts[k], es[i].pts[k + 1]);
            let it = Item { net: sa.net, w: seg_w(i, k), s: sa.s };
            if sh.edge_feat == Some(f) {
                match sh.feats[f].bound {
                    Bound::Edge { clear } => sh.outline.iter().any(|&(a, b)| seg_seg_dist(p, q, a, b) < 0.5 * it.w + clear - 1.0),
                    _ => false,
                }
            } else {
                let cl = sh.feats[f].bound.clearance(&it);
                cl > 0.0 && seg_poly_dist(p, q, &sh.feats[f].poly) < 0.5 * it.w + cl - 1.0
            }
        };
        let mut a = 0;
        while a < n && viol(a) {
            a += 1;
        }
        let mut b = n;
        while b > a && viol(b - 1) {
            b -= 1;
        }
        (a, b)
    };
    let mut worst: BTreeMap<(usize, Party, bool), Violation> = BTreeMap::new();
    let mut put = |v: Violation| {
        let e = worst.entry((v.stretch, v.other, v.predicted)).or_insert(v);
        if v.amount > e.amount {
            *e = v;
        }
    };
    let mut near = BTreeSet::new();
    for (id, &(i, k)) in segs.iter().enumerate() {
        let (a, b) = (es[i].pts[k], es[i].pts[k + 1]);
        let sa = &sts[es[i].stretch];
        let (lo, hi) = bbox(a, b, reach);
        grid.query(lo, hi, &mut near);
        for &jd in near.iter().filter(|&&jd| jd > id) {
            let (j, l) = segs[jd];
            let sb = &sts[es[j].stretch];
            if sa.net == sb.net {
                continue;
            }
            let req = 0.5 * (seg_w(i, k) + seg_w(j, l)) + sa.s.max(sb.s);
            let (c, d) = (es[j].pts[l], es[j].pts[l + 1]);
            let dist = seg_seg_dist(a, b, c, d);
            if dist < req - 1.0 {
                let (x, y) = (es[i].stretch.min(es[j].stretch), es[i].stretch.max(es[j].stretch));
                put(Violation { stretch: x, other: Party::Stretch(y), at: (a + b + c + d) * 0.25, amount: req - dist, predicted: pred_pair.contains(&(x, y)) });
            }
        }
    }
    // Features: pads, vias, junctions, holes (in a grid of their boxes); the outline.
    let (fgrid, fclear) = feat_grid(&sh.feats, cell);
    for &(i, k) in &segs {
        let (a, b) = (es[i].pts[k], es[i].pts[k + 1]);
        let si = es[i].stretch;
        let s = &sts[si];
        let it = Item { net: s.net, w: seg_w(i, k), s: s.s };
        let (lo, hi) = bbox(a, b, 0.5 * it.w + fclear.max(s.s));
        fgrid.query(lo, hi, &mut near);
        for &f in &near {
            let ft = &sh.feats[f];
            let cl = ft.bound.clearance(&it);
            if cl <= 0.0 {
                continue;
            }
            let req = 0.5 * it.w + cl;
            let dist = seg_poly_dist(a, b, &ft.poly);
            if dist < req - 1.0 {
                put(Violation { stretch: si, other: Party::Feature(f), at: (a + b) * 0.5, amount: req - dist, predicted: pred_feat.contains(&(si, f)) || (pred_exit.contains(&(si, f)) && { let (x, y) = exit_run(i, f); k < x || k >= y }) });
            }
        }
        if let Some(ef) = sh.edge_feat {
            if let Bound::Edge { clear } = sh.feats[ef].bound {
                let req = 0.5 * it.w + clear;
                for &(p, q) in &sh.outline {
                    let dist = seg_seg_dist(a, b, p, q);
                    if dist < req - 1.0 {
                        put(Violation { stretch: si, other: Party::Edge, at: (a + b) * 0.5, amount: req - dist, predicted: pred_feat.contains(&(si, ef)) || (pred_exit.contains(&(si, ef)) && { let (x, y) = exit_run(i, ef); k < x || k >= y }) });
                    }
                }
            }
        }
    }
    worst.into_values().collect()
}


/// Counts of a layer's ERBS and their violations.
pub fn report(le: &LayerErbs, vs: &[Violation]) -> ErbsReport {
    let mut r = ErbsReport { stretches: le.es.len(), failed: le.failed.len(), ..ErbsReport::default() };
    for e in &le.es {
        r.points += e.points;
        r.portals += e.portals;
        r.conflicts += e.conflicts;
        r.len += e.len;
        r.band_len += e.band_len;
    }
    for v in vs {
        if v.predicted {
            r.predicted += 1;
        } else {
            r.erbs_gap += 1;
            r.worst_gap = r.worst_gap.max(v.amount);
        }
    }
    r
}

/// Debug dump of one layer (`TOPO_ERBS_DUMP`, mm, engine frame): features, bands, every
/// obstacle (`gates`: a cut item's portal `a b` on `p q`; an arc or feature point as `a`, its root
/// as `p`), the path and the violations. Read by `harness/erbs_accept.py`.
pub fn dump(sh: &LayerSheet, sts: &[Stretch], le: &LayerErbs, vs: &[Violation]) -> Value {
    let mm = |p: Vec2| json!([p.x / 1e6, p.y / 1e6]);
    let feats: Vec<Value> = sh.feats.iter().map(|f| json!(f.poly.iter().map(|&p| mm(p)).collect::<Vec<_>>())).collect();
    let ers: Vec<Value> = le
        .es
        .iter()
        .map(|e| {
            let s = &sts[e.stretch];
            let gates: Vec<Value> = e
                .gate_list
                .iter()
                .map(|g| {
                    let (k, i) = match g.src {
                        GateSrc::Cut(c) => ("cut", c),
                        GateSrc::Arc(v) => ("arc", v),
                        GateSrc::Feature(f) => ("feat", f),
                    };
                    json!({"a": mm(g.a), "b": mm(g.b), "src": k, "id": i, "w": g.w / 1e6, "p": mm(g.p), "q": mm(g.q), "p_left": g.p_left})
                })
                .collect();
            json!({"stretch": e.stretch, "wire": s.wire, "net": s.net, "w": s.w / 1e6, "s": s.s / 1e6, "band": s.band.iter().map(|&p| mm(p)).collect::<Vec<_>>(),
                   "wraps": s.wraps.iter().map(|w| w.map(|(v, l)| json!([v, l]))).collect::<Vec<_>>(),
                   "pts": e.pts.iter().map(|&p| mm(p)).collect::<Vec<_>>(), "path": e.path.iter().map(|&p| mm(p)).collect::<Vec<_>>(),
                   "at": e.at, "gates": gates, "points": e.points, "portals": e.portals, "conflicts": e.conflicts, "converged": e.converged, "len": e.len / 1e6, "band_len": e.band_len / 1e6})
        })
        .collect();
    let failed: Vec<Value> = le.failed.iter().map(|(si, m)| json!({"stretch": si, "wire": sts[*si].wire, "why": m, "band": sts[*si].band.iter().map(|&p| mm(p)).collect::<Vec<_>>()})).collect();
    let viol: Vec<Value> = vs.iter().map(|v| json!({"stretch": v.stretch, "other": format!("{:?}", v.other), "at": mm(v.at), "mm": v.amount / 1e6, "predicted": v.predicted})).collect();
    json!({"layer": sh.l, "feats": feats, "outline": sh.outline.iter().map(|&(a, b)| json!([mm(a), mm(b)])).collect::<Vec<_>>(), "erbs": ers, "failed": failed, "violations": viol})
}

/// Debug summary of one layer (mm, KiCad frame), the first `erbs_gap` places included.
pub fn summary(r: &ErbsReport, vs: &[Violation], sts: &[Stretch]) -> Value {
    let gaps: Vec<Value> = vs
        .iter()
        .filter(|v| !v.predicted)
        .take(20)
        .map(|v| {
            let other = match v.other {
                Party::Stretch(b) => json!({"wire": sts[b].wire}),
                Party::Feature(f) => json!({"feature": f}),
                Party::Edge => json!("edge"),
            };
            json!({"wire": sts[v.stretch].wire, "other": other, "at": [v.at.x / 1e6, -v.at.y / 1e6], "mm": v.amount / 1e6})
        })
        .collect();
    json!({
        "stretches": r.stretches, "points": r.points, "portals": r.portals, "conflicts": r.conflicts, "failed": r.failed,
        "length_over_band": if r.band_len > 0.0 { r.len / r.band_len } else { 1.0 },
        "predicted": r.predicted, "erbs_gap": r.erbs_gap, "worst_gap_mm": r.worst_gap / 1e6, "gaps": gaps,
    })
}

/// One run of a wire as its ERBS (`[drawing] realise = "erbs"`): layer, end points, the path.
#[derive(Clone, Debug)]
pub struct RunBand {
    pub layer: usize,
    pub s: Vec2,
    pub e: Vec2,
    pub path: Vec<Vec2>,
    /// Length of the stretch's band (nm): the drawing's length factor compares the ERBS with it
    /// (`draw::erbs_length_price`).
    pub band_len: f64,
    /// The check found a violation no over-full cut predicts on this band (an `erbs_gap`).
    pub gap: bool,
}

/// The ERBS of every wire for the drawing, its precedes order and the counts.
#[derive(Clone, Debug, Default)]
pub struct Realised {
    pub runs: BTreeMap<usize, Vec<RunBand>>,
    /// The wires in the precedes order ([`precedes`]), ties in the order given.
    pub order: Vec<usize>,
    pub report: ErbsReport,
    /// Stretches the measurement could not band (walk failed): drawn the sequential way.
    pub unbanded: usize,
    /// Stretches without an ERBS because the run's time limit passed ([`ErbsOpts::deadline`]):
    /// drawn the sequential way.
    pub timed_out: usize,
    /// Stretches whose sleeve did not reach the end (the first few reasons with their wire):
    /// drawn the sequential way.
    pub failed: Vec<(usize, String)>,
    pub seconds: f64,
}

/// The precedes order (Staepelaere 1992, OrderSegments, PDF p. 29: a wire is made octilinear
/// after every wire between it and what it bends round, so each one has its "previous boundary"
/// drawn, PDF pp. 18, 33-34): a wire that bends at a spoke tip of a cut comes after the items
/// of that cut between it and that end, one that bends on a stack arc after the stack's inner
/// members. Kahn's sort over the wires with the given order as the tie-break, a cycle broken at
/// the earliest. O(bends x items of the cut + wires^2 worst).
pub fn precedes(es: &[Erbs], sts: &[Stretch], o: &LayerOrder, before: &mut BTreeSet<(usize, usize)>) {
    for e in es {
        let wire = sts[e.stretch].wire;
        for k in 1..e.pts.len().saturating_sub(1) {
            let Some(gi) = e.at[k] else { continue };
            let g = &e.gate_list[gi];
            let x = e.pts[k];
            let (u, v) = (e.pts[k - 1], e.pts[k + 1]);
            if (v - x).cross(x - u).abs() <= 1e-6 * (v - x).len() * (x - u).len() + 1.0 {
                continue;
            }
            match g.src {
                GateSrc::Cut(ci) => {
                    let at_a = (x - g.a).len() < 2.0;
                    let items = &o.cuts[ci].items;
                    let Some(ii) = items.iter().position(|it| it.stretch == e.stretch) else { continue };
                    let range = if at_a { 0..ii } else { ii + 1..items.len() };
                    for it in &items[range] {
                        let w2 = sts[it.stretch].wire;
                        if w2 != wire {
                            before.insert((w2, wire));
                        }
                    }
                }
                GateSrc::Arc(v) => {
                    for st in o.stacks.iter().filter(|st| st.v == v) {
                        if let Some(i) = st.order.iter().position(|&m| m == e.stretch) {
                            for &m in &st.order[..i] {
                                let w2 = sts[m].wire;
                                if w2 != wire {
                                    before.insert((w2, wire));
                                }
                            }
                        }
                    }
                }
                GateSrc::Feature(_) => {}
            }
        }
    }
}

/// Topological order of `given` under `before` (pairs of wires), the given order as tie-break.
pub fn precedes_order(given: &[usize], before: &BTreeSet<(usize, usize)>) -> Vec<usize> {
    let pos: BTreeMap<usize, usize> = given.iter().enumerate().map(|(i, &w)| (w, i)).collect();
    let n = given.len();
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    for &(a, b) in before {
        if let (Some(&i), Some(&j)) = (pos.get(&a), pos.get(&b)) {
            if i != j {
                succ[i].push(j);
                indeg[j] += 1;
            }
        }
    }
    let mut free: BTreeSet<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut done = vec![false; n];
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let i = free.pop_first().unwrap_or_else(|| (0..n).find(|&i| !done[i]).expect("left"));
        if done[i] {
            continue;
        }
        done[i] = true;
        out.push(given[i]);
        for &j in &succ[i] {
            indeg[j] = indeg[j].saturating_sub(1);
            if indeg[j] == 0 && !done[j] {
                free.insert(j);
            }
        }
    }
    out
}

/// The ERBS of every wire for the drawing (`[drawing] realise = "erbs"`): the plan measured with
/// the drawing's placed vias (sheets with the via discs, bands, cut items), the order, tips and
/// radii (steps a, b), the ERBS per layer and their check, and the precedes order of `given`.
#[allow(clippy::too_many_arguments)]
pub fn realise(st: &super::state::State, events: &[Vec<super::refine::LEv>], packer: &super::pack::Packer, bottom: i32, conn: &dyn Fn(usize) -> Option<usize>, placed: &BTreeMap<usize, Vec<(usize, Vec2)>>, outline: &[(Vec2, Vec2)], mo: &super::cutflow::MeasureOpts, layers: &[usize], given: &[usize], deadline: Option<std::time::Instant>, stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>, fail_first: bool) -> Result<Realised, String> {
    let t0 = std::time::Instant::now();
    let none: Vec<Vec<usize>> = Vec::new();
    let plan = super::cutflow::Plan { st, events, via_k: &none, bottom, packer, conn, placed: Some(placed) };
    let m = super::cutflow::measure(&plan, outline, mo, layers)?;
    let ords = super::radii::from_measure(&m, &|p| st.proj.regime_at(p));
    let mut out = Realised::default();
    let mut before: BTreeSet<(usize, usize)> = BTreeSet::new();
    // The test hook fails one stretch of the run (the first of the first layer with one), not
    // one per layer: the count it leaves does not depend on how the plan spreads the wires over
    // the layers.
    let mut inject = fail_first;
    for (lf, o) in m.layers.iter().zip(&ords) {
        out.unbanded += lf.walk_failed;
        let eo = ErbsOpts { deadline, stop: stop.clone(), fail_first: inject, margin: m.opts.margin, ..ErbsOpts::default() };
        let le = erbs_layer_with(&lf.sheet, &m.stretches, &lf.stretches, o, &eo);
        if !le.es.is_empty() || !le.failed.is_empty() {
            inject = false;
        }
        out.timed_out += le.timed_out;
        let vs = check_layer(&lf.sheet, &m.stretches, o, &le.es);
        // Debug: the drawing's ERBS per layer (harness/erbs_accept.py reads `realise-L<n>.json`).
        if let Some(d) = std::env::var_os("TOPO_ERBS_DUMP") {
            let path = std::path::Path::new(&d).join(format!("realise-L{}.json", lf.sheet.l));
            let _ = std::fs::write(path, dump(&lf.sheet, &m.stretches, &le, &vs).to_string());
        }
        let r = report(&le, &vs);
        let gap: BTreeSet<usize> = vs.iter().filter(|v| !v.predicted).flat_map(|v| match v.other {
            Party::Stretch(b) => vec![v.stretch, b],
            _ => vec![v.stretch],
        }).collect();
        precedes(&le.es, &m.stretches, o, &mut before);
        for e in &le.es {
            let s = &m.stretches[e.stretch];
            out.runs.entry(s.wire).or_default().push(RunBand { layer: s.layer, s: e.pts[0], e: e.pts[e.pts.len() - 1], path: e.path.clone(), band_len: e.band_len, gap: gap.contains(&e.stretch) });
        }
        out.failed.extend(le.failed.iter().map(|(si, why)| (m.stretches[*si].wire, why.clone())));
        let t = &mut out.report;
        t.stretches += r.stretches;
        t.points += r.points;
        t.portals += r.portals;
        t.conflicts += r.conflicts;
        t.failed += r.failed;
        t.len += r.len;
        t.band_len += r.band_len;
        t.predicted += r.predicted;
        t.erbs_gap += r.erbs_gap;
        t.worst_gap = t.worst_gap.max(r.worst_gap);
    }
    out.order = precedes_order(given, &before);
    if std::env::var_os("TOPO_ERBS_DEBUG").is_some() {
        eprintln!("ERBS precedes {:?} given {:?} order {:?}", before, given, out.order);
    }
    out.seconds = t0.elapsed().as_secs_f64();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x: x * 1e6, y: y * 1e6 }
    }

    /// The ERBS from start to end with `ls` left and `rs` right of it (the band straight).
    fn path_of(s: Vec2, e: Vec2, ls: &[Vec2], rs: &[Vec2]) -> Result<Vec<Vec2>, String> {
        let mut pp = Points::new(s, e, 0.2e6);
        for &p in ls {
            pp.add(p, true, None, 0.2e6);
        }
        for &p in rs {
            pp.add(p, false, None, 0.2e6);
        }
        pp.frame(&[s, e]);
        let rough = rough_path(&[s, e], &pp);
        let por = sleeve(&pp, &rough)?;
        let mut ports = vec![(s, s)];
        ports.extend(por.iter().map(|&(l, r)| (pp.pts[l], pp.pts[r])));
        ports.push((e, e));
        let p = simplify(&pull(&ports).into_iter().map(|c| c.0).collect::<Vec<_>>(), 0.5);
        // Every point on its own side of the path (or on it).
        for (&x, left) in ls.iter().map(|x| (x, true)).chain(rs.iter().map(|x| (x, false))) {
            let (l, d) = band_side(&p, x);
            assert!(l == left || d < 2.0, "{x:?} on the wrong side of {p:?}");
        }
        Ok(p)
    }

    /// The path of `band` among labelled points (`2` left, `3` right) with their detours.
    fn path_band(band: &[Vec2], pts: &[(Vec2, u8, Detour)]) -> Result<Vec<Vec2>, String> {
        let (s, e) = (band[0], band[band.len() - 1]);
        let mut pp = Points::new(s, e, 0.2e6);
        for &(p, lab, det) in pts {
            pp.add_at(p, lab, None, 0.2e6, det);
        }
        pp.frame(band);
        let rough = rough_path(band, &pp);
        let por = sleeve(&pp, &rough)?;
        let mut ports = vec![(s, s)];
        ports.extend(por.iter().map(|&(l, r)| (pp.pts[l], pp.pts[r])));
        ports.push((e, e));
        Ok(simplify(&pull(&ports).into_iter().map(|c| c.0).collect::<Vec<_>>(), 0.5))
    }

    /// A detour from the band's start (a tip whose spoke the band crosses at its first point):
    /// the rough path's loop leaves from a vertex of the triangulation, the walk picks the face
    /// of that vertex's star the loop enters, and the path goes round the point.
    #[test]
    fn detour_from_the_start_vertex() {
        let (s, e) = (v(0.0, 0.0), v(10.0, 0.0));
        let p = path_band(&[s, e], &[(v(0.5, -1.0), 2, Detour::From(s)), (v(5.0, 1.0), 2, Detour::Never)]).unwrap();
        assert_eq!(p, vec![s, v(0.5, -1.0), e]);
    }

    /// The band runs through an obstacle point (a tie of the walk's side test): the point's own
    /// label decides its side.
    #[test]
    fn band_through_a_point_takes_its_label() {
        let band = [v(0.0, -0.5), v(1.0, 0.0), v(2.0, -0.5)];
        let x = v(1.0, 0.0);
        let right = path_band(&band, &[(x, 3, Detour::Never)]).unwrap();
        assert_eq!(right, band.to_vec(), "kept on the right: the path bends over it");
        let left = path_band(&band, &[(x, 2, Detour::Never)]).unwrap();
        assert_eq!(left, vec![band[0], band[2]], "kept on the left: straight under it");
    }

    /// Obstacles on their own sides of a straight band: the path is the band.
    #[test]
    fn sleeve_straight() {
        let ls: Vec<Vec2> = (1..10).map(|k| v(k as f64, 1.0)).collect();
        let rs: Vec<Vec2> = (1..10).map(|k| v(k as f64, -1.0)).collect();
        let p = path_of(v(0.0, 0.0), v(10.0, 0.0), &ls, &rs).unwrap();
        assert_eq!(p, vec![v(0.0, 0.0), v(10.0, 0.0)]);
    }

    /// A left point on the band's right (a spoke tip the band crosses): the path goes round it,
    /// bending at the point and turning toward it, and nowhere else.
    #[test]
    fn sleeve_pushes_round_a_tip() {
        let mut ls: Vec<Vec2> = (1..10).map(|k| v(k as f64, 2.0)).collect();
        ls.push(v(5.0, -0.5));
        let rs: Vec<Vec2> = (1..10).map(|k| v(k as f64, -2.0)).collect();
        let p = path_of(v(0.0, 0.0), v(10.0, 0.0), &ls, &rs).unwrap();
        assert_eq!(p, vec![v(0.0, 0.0), v(5.0, -0.5), v(10.0, 0.0)]);
    }

    /// Crossing cuts whose portals all hold the straight line (eprom stretch 2's fan: cuts that
    /// meet inside both portals): no order to solve, the path is straight. The old gate order
    /// zigzagged here (11.06 vs 9.15 mm).
    #[test]
    fn crossing_portals_give_the_straight_path() {
        let mut ls = Vec::new();
        let mut rs = Vec::new();
        for k in 0..12 {
            let x = 1.0 + 0.7 * k as f64;
            // Two cuts crossing at (x, 0): tips at +-0.3 .. 0.6 off the line, alternately slanted.
            ls.push(v(x - 0.4, 0.3 + 0.03 * k as f64));
            rs.push(v(x + 0.4, -0.3 - 0.02 * k as f64));
            ls.push(v(x + 0.4, 0.35));
            rs.push(v(x - 0.4, -0.35 - 0.01 * k as f64));
        }
        let p = path_of(v(0.0, 0.0), v(10.0, 0.0), &ls, &rs).unwrap();
        assert_eq!(p, vec![v(0.0, 0.0), v(10.0, 0.0)]);
    }

    /// Round h: a loop's two legs see a point on its segment on the same side (the way back is
    /// the way out reversed). The old thin loop's legs passed a point at 15 % of the leg 0.75 nm
    /// and 0.375 nm off: the first beyond the walk's 0.5 nm tie, the second inside it (taking the
    /// label), so the loop enclosed the point.
    #[test]
    fn loop_legs_agree_on_a_point_near_the_base() {
        let (s, e) = (v(0.0, 0.0), v(10.0, 0.0));
        let x = v(5.0, 0.0);
        let p = path_band(&[s, e], &[(v(5.0, -1.0), 2, Detour::From(x)), (v(5.0, -0.15), 2, Detour::From(x))]).unwrap();
        assert_eq!(p, vec![s, v(5.0, -1.0), e]);
    }

    /// Round h rule 1's tie with a point of the other label on the leg: a right-labelled point
    /// q on the segment from the base to a pushed left-labelled point c counts as on c's label
    /// side of the leg (the out leg passes it on c's side), not by its own label. The lasso
    /// w (turn round c) w^-1 then depends on that side: the path goes round c first and then
    /// keeps q on its right ([s, c, q, e]); by q's own label the leg would pass q on the other
    /// side and the path would touch q first ([s, q, c, e]), a different class.
    #[test]
    fn loop_leg_point_of_the_other_label_takes_the_loop_side() {
        let (s, e) = (v(0.0, 0.0), v(10.0, 0.0));
        let x = v(5.0, 0.0);
        let (c, q) = (v(5.0, -1.0), v(5.0, -0.5));
        let p = path_band(&[s, e], &[(c, 2, Detour::From(x)), (q, 3, Detour::Never)]).unwrap();
        assert_eq!(p, vec![s, c, q, e]);
    }

    /// Round h: many loops deep into a field of pushed points (esp st 38: 1029 tips round a pad,
    /// the unreduced walk longer than the old cap of 8 (faces + rough) + 64 steps). Each
    /// straight piece is capped by the face count; the path goes round the outermost points and
    /// leaves every point on its left.
    #[test]
    fn many_deep_loops_walk_to_the_end() {
        let (s, e) = (v(0.0, 0.0), v(10.0, 0.0));
        let x = v(5.0, 0.0);
        let mut pts = Vec::new();
        for r in 0..60 {
            let a = std::f64::consts::PI * (0.1 + 0.8 * r as f64 / 59.0);
            for k in 1..=60 {
                let d = 0.05 * k as f64;
                pts.push((v(5.0 + d * a.cos(), -d * a.sin()), 2u8, Detour::From(x)));
            }
        }
        let p = path_band(&[s, e], &pts).unwrap();
        for &(q, _, _) in &pts {
            let (l, d) = band_side(&p, q);
            assert!(l || d < 2.0, "{q:?} right of the path {p:?}");
        }
        assert!(poly_len(&p) < 20e6, "{p:?}");
    }

    /// Round h: loops at one band vertex, their arclengths one rounding apart, go in angle order
    /// from one base.
    #[test]
    fn same_base_loops_in_angle_order() {
        let x = v(1.0, 1.0);
        let s1: f64 = 1.0e6;
        let s0 = f64::from_bits(s1.to_bits() - 1);
        let mut det = vec![(s0, 1.0, 7, x + Vec2 { x: 0.1, y: 0.0 }), (s1, 0.5, 8, x), (s1, 2.0, 9, x)];
        same_base(&mut det);
        assert_eq!(det.iter().map(|d| d.2).collect::<Vec<_>>(), vec![8, 7, 9]);
        assert!(det.iter().all(|d| d.0 == s0 && d.3 == x + Vec2 { x: 0.1, y: 0.0 }), "{det:?}");
    }

    /// The path of `band` among candidates `(point, left, detour, spoke root)` after
    /// [`settle_pushes`].
    fn path_settled(band: &[Vec2], cs: &[(Vec2, bool, Detour, Vec2)]) -> (Vec<Vec2>, usize) {
        path_settled_by(band, cs).0
    }

    /// [`path_settled`] with the gates settle 1 settled (candidate `i` has gate `i`) and the
    /// number left out.
    fn path_settled_by(band: &[Vec2], cs: &[(Vec2, bool, Detour, Vec2)]) -> ((Vec<Vec2>, usize), Vec<usize>, usize) {
        let mut cand: Vec<(Vec2, bool, usize, f64, Detour, Vec2)> = cs.iter().enumerate().map(|(i, &(p, l, d, r))| (p, l, i, 0.2e6, d, r)).collect();
        let (mut settled, out) = settle_pushes(band, &mut cand);
        settled.sort();
        let pts: Vec<(Vec2, u8, Detour)> = cand.iter().map(|c| (c.0, if c.1 { 2 } else { 3 }, c.4)).collect();
        ((path_band(band, &pts).unwrap(), cand.len()), settled, out)
    }

    /// Round h, settle 1: two pushes of opposite labels whose spokes cross (each asks for the
    /// path beyond its tip on the other's side, the room over-full): neither pushes, both are
    /// left out, the band stays. Pushed, the path zigzags between the two tips.
    #[test]
    fn opposite_pushes_with_crossing_spokes_settle() {
        let (s, e) = (v(0.0, 0.0), v(10.0, 0.0));
        let x = v(5.0, 0.0);
        let a = (v(5.0, -0.5), true, Detour::From(x), v(5.2, 0.5));
        let b = (v(5.0, 0.5), false, Detour::From(x), v(5.2, -0.5));
        let ((p, n), settled, out) = path_settled_by(&[s, e], &[a, b]);
        assert_eq!((p, n), (vec![s, e], 0));
        // both gates reported settled (their violations predicted, check_layer), both left out
        assert_eq!((settled, out), (vec![0, 1], 2));
        // spokes apart (b's root beyond a's tip): both push, the path goes round both
        let b2 = (v(7.0, 0.5), false, Detour::From(v(7.0, 0.0)), v(7.0, -0.5));
        let (p, n) = path_settled(&[s, e], &[a, b2]);
        assert_eq!((p, n), (vec![s, v(5.0, -0.5), v(7.0, 0.5), e], 2));
        // The same with Auto pushes (grown points on the other side of the band than their
        // feature): pushed means on the wrong side, so the two settle as well.
        let aa = (v(5.0, -0.5), true, Detour::Auto, v(5.2, 0.5));
        let ba = (v(5.0, 0.5), false, Detour::Auto, v(5.2, -0.5));
        let ((p, n), settled, out) = path_settled_by(&[s, e], &[aa, ba]);
        assert_eq!((p, n, settled, out), (vec![s, e], 0, vec![0, 1], 2));
        // An Auto point on its own side pushes nothing, though its spoke crosses a pushed one of
        // the other label: nothing settles, the pushed tip keeps its loop and the Auto point stays.
        let c = (v(5.3, -0.6), false, Detour::Auto, v(4.9, -0.2));
        let ((p, n), settled, out) = path_settled_by(&[s, e], &[a, c]);
        assert_eq!((p, n, settled, out), (vec![s, v(5.0, -0.5), e], 2, vec![], 0));
    }

    #[test]
    fn crossing_spokes_share_a_passed_cell() {
        // Every properly crossing pair of segments (random, incl. axis-parallel and through cell
        // corners) has a cell both pass: the grid in settle_pushes misses no crossing.
        let mut r: u64 = 0x9e3779b97f4a7c15;
        let mut f = || {
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            ((r >> 11) as f64 / (1u64 << 53) as f64) * 1.0e6 - 0.5e6
        };
        let cell = 0.1e6;
        let q = |v: f64| (v / cell).round() * cell;
        let mut n = 0;
        for k in 0..4000 {
            let mut s = [Vec2 { x: f(), y: f() }, Vec2 { x: f(), y: f() }, Vec2 { x: f(), y: f() }, Vec2 { x: f(), y: f() }];
            if k % 4 == 1 {
                s[1].y = s[0].y;
            }
            if k % 4 >= 2 {
                for p in s.iter_mut() {
                    *p = Vec2 { x: q(p.x), y: q(p.y) };
                }
            }
            if k % 4 == 3 {
                s[3].y = s[2].y;
            }
            if !seg_cross(s[0], s[1], s[2], s[3]) {
                continue;
            }
            n += 1;
            let a: BTreeSet<_> = seg_cells(s[0], s[1], cell).into_iter().collect();
            assert!(seg_cells(s[2], s[3], cell).iter().any(|c| a.contains(c)), "{s:?}");
        }
        assert!(n > 500, "{n}");
    }

    /// Round h, settle 2: a point that does not push and lies on the other side of the band than
    /// its label (a squeezed feature's tip) is no obstacle; kept, the path hooks round it to keep
    /// it on the band's side while it goes round the pushed tip beyond it.
    #[test]
    fn unpushed_wrong_side_point_is_no_obstacle() {
        let (s, e) = (v(0.0, 0.0), v(10.0, 0.0));
        let x = v(5.0, 0.0);
        let p1 = (v(5.0, -1.0), true, Detour::From(x), v(5.0, 1.0));
        let q = (v(5.3, -0.3), true, Detour::Never, v(5.3, 1.0));
        let (p, n) = path_settled(&[s, e], &[p1, q]);
        assert_eq!((p, n), (vec![s, v(5.0, -1.0), e], 1));
        // on its own side it stays an obstacle
        let q2 = (v(5.3, 0.3), true, Detour::Never, v(5.3, 1.0));
        let (_, n) = path_settled(&[s, e], &[p1, q2]);
        assert_eq!(n, 2);
    }

    /// Two points to go round on one ray from the band (a spoke tip on an arc sample's ray), the
    /// nearer added first: the path goes round the farther one only, once.
    #[test]
    fn collinear_detours_far_first() {
        let ls = vec![v(5.0, -0.5), v(5.0, -1.0)];
        let p = path_of(v(0.0, 0.0), v(10.0, 0.0), &ls, &[]).unwrap();
        assert_eq!(p, vec![v(0.0, 0.0), v(5.0, -1.0), v(10.0, 0.0)]);
    }

    /// Points closer than MERGE are one obstacle.
    #[test]
    fn near_points_merge() {
        let mut pp = Points::new(v(0.0, 0.0), v(1.0, 0.0), 1.0);
        pp.add(v(0.5, 0.1), true, None, 1.0);
        pp.add(Vec2 { x: 0.5e6 + 5.0, y: 0.1e6 - 3.0 }, true, None, 1.0);
        assert_eq!(pp.pts.len(), 3);
    }

    /// A tip the band only touches (the with_dropped counter-example class: touch vs crossing):
    /// a left point on the straight band adds no length and no bend.
    #[test]
    fn touched_tip_is_no_detour() {
        let ls = vec![v(3.0, 0.0), v(6.0, 1.0)];
        let rs = vec![v(4.0, -1.0), v(7.0, -1.0)];
        let p = path_of(v(0.0, 0.0), v(10.0, 0.0), &ls, &rs).unwrap();
        assert!((poly_len(&p) - 10e6).abs() < 1.0, "{p:?}");
    }

    /// Points on the wrong side far from the band, on both sides (spoke tips that push): the
    /// path goes round each, in band order, and bends only at them.
    #[test]
    fn pushed_both_ways() {
        let ls = vec![v(3.0, -1.0)];
        let rs = vec![v(7.0, 1.0)];
        let p = path_of(v(0.0, 0.0), v(10.0, 0.0), &ls, &rs).unwrap();
        assert_eq!(p, vec![v(0.0, 0.0), v(3.0, -1.0), v(7.0, 1.0), v(10.0, 0.0)]);
    }

    /// Coincident points of opposite sides are counted, the later one left out.
    #[test]
    fn conflicts_counted() {
        let mut pp = Points::new(v(0.0, 0.0), v(1.0, 0.0), 1.0);
        pp.add(v(0.5, 0.1), true, None, 1.0);
        pp.add(v(0.5, 0.1), false, None, 1.0);
        pp.add(v(0.5, 0.1), true, None, 1.0);
        assert_eq!((pp.conflicts, pp.pts.len()), (1, 3));
    }

    /// Sides against a bent band: the pieces, the inner and the outer side of a corner.
    #[test]
    fn band_sides() {
        let band = [v(0.0, 0.0), v(10.0, 0.0), v(10.0, 10.0)];
        assert!(band_side(&band, v(5.0, 1.0)).0);
        assert!(!band_side(&band, v(5.0, -1.0)).0);
        assert!(band_side(&band, v(9.0, 1.0)).0, "inside the left turn");
        assert!(!band_side(&band, v(11.0, -1.0)).0, "outside the corner");
        assert!(!band_side(&band, v(11.0, 5.0)).0);
    }

    /// A band piece through a feature (the plan's accepted violation) runs through it; one that
    /// touches it, runs along a side or passes by does not.
    #[test]
    fn seg_through_feature() {
        let sq = [v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)];
        assert!(seg_through(v(-1.0, 0.5), v(2.0, 0.5), &sq));
        assert!(seg_through(v(0.5, 0.5), v(0.6, 0.6), &sq), "a piece inside");
        assert!(seg_through(v(-1.0, -0.5), v(1.0, 1.5), &sq), "a diagonal through a corner region");
        assert!(!seg_through(v(-1.0, 0.0), v(2.0, 0.0), &sq), "along a side");
        assert!(!seg_through(v(-1.0, 1.0), v(1.0, 3.0), &sq), "touching a corner");
        assert!(!seg_through(v(-1.0, 2.0), v(2.0, 2.0), &sq), "past it");
        assert!(!seg_through(v(-1.0, 0.5), v(2.0, 0.5), &sq[..2]), "no area");
    }

    /// A band along a side of a square, its vertices wrapped: the square on the wrap's side is
    /// outside the band; on the other side the band passes its vertices on the inside.
    #[test]
    fn wrap_from_inside() {
        let sq = [v(0.0, -1.0), v(1.0, -1.0), v(1.0, 0.0), v(0.0, 0.0)];
        let band = [v(2.0, 0.0), v(1.0, 0.0), v(0.0, 0.0), v(-1.0, 0.0)];
        // Going west along y = 0 the square (below) is on the band's left.
        assert!(!wraps_inside(&band, &sq, true));
        assert!(wraps_inside(&band, &sq, false));
    }

    /// A convex feature grown by r in the octagonal norm: the hull of the vertex octagons, every
    /// point at least r from the feature, none farther than r / cos 22.5.
    #[test]
    fn grown_rectangle() {
        let (dirs, grow) = norm_dirs(Norm::Octagonal, 22.5);
        let poly = vec![v(0.0, 0.0), v(2.0, 0.0), v(2.0, 1.0), v(0.0, 1.0)];
        let g = grown(&poly, 0.3e6, &dirs, grow);
        assert!((8..=12).contains(&g.len()), "{g:?}");
        for x in &g {
            let d = poly_dist(*x, &poly);
            assert!(d >= 0.3e6 - 1.0 && d <= 0.3e6 / FRAC_PI_8.cos() + 1.0, "{d}");
        }
    }

    /// Round g rule 3: a grown point pushes only when the band crosses its spoke (from the
    /// feature vertex to the point), or touches it (a band through the vertex).
    #[test]
    fn spoke_crossed_only_between_point_and_feature() {
        let band = [v(0.0, 0.0), v(0.0, 6.0)];
        // a pad right of the band, its grown point left of it: crossed
        assert!(spoke_crossed(&band, v(-0.2, 3.0), v(0.2, 3.0)));
        // the band through the vertex: touched
        assert!(spoke_crossed(&band, v(-0.3, 3.0), v(0.0, 3.0)));
        // a point left of the band beside the start, its pad behind the start: not crossed
        assert!(!spoke_crossed(&band, v(-1.0, 0.1), v(-0.5, -0.3)));
        // both left of the band: not crossed
        assert!(!spoke_crossed(&band, v(-1.0, 3.0), v(-0.5, 3.0)));
        // past the band's end: not crossed
        assert!(!spoke_crossed(&band, v(0.3, 6.5), v(-0.3, 6.5)));
        // a polyline: the second piece crosses
        assert!(spoke_crossed(&[v(0.0, 0.0), v(2.0, 0.0), v(2.0, 2.0)], v(1.5, 1.0), v(2.5, 1.0)));
    }

    #[test]
    fn simplify_collinear() {
        let p = simplify(&[v(0.0, 0.0), v(1.0, 0.0), v(1.0, 0.0), v(2.0, 0.0), v(2.0, 1.0)], 0.5);
        assert_eq!(p, vec![v(0.0, 0.0), v(2.0, 0.0), v(2.0, 1.0)]);
    }
}
