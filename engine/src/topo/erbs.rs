//! M3 step c (shopping list 6.2; design note `docs/progress/notes/m3-radii-spokes-design.md`
//! §4.3, §15): the extended rubber band (ERBS) of every stretch, from the spoke tips and stack
//! radii of [`super::radii`], and its check.
//!
//! **The ERBS is the shortest path of the band's homotopy class among the spoke tips** (Dayan
//! 1997, PDF pp. 25-26, printed pp. 14-15: the ERBS is "a minimal length member of the topological
//! class", its terminals a superset of the RBS's, the spoke ends added; the spokes PDF p. 32).
//! There is no order of gates to solve: the class says on which side of the path every obstacle
//! lies, and the shortest path of a class among point obstacles is the funnel through the sleeve
//! of a triangulation of the points (Hershberger & Snoeyink 1994; Efrat, Kobourov & Lubiw 2006,
//! PDF p. 9: the sleeve, then the funnel, O(n + k) per path; Popov & Popov 2012, PDF p. 3, Fig. 3:
//! the funnel over circles classified left or right of the path, p. 2, Fig. 1: the taut thread
//! runs along the arcs). Per stretch:
//!
//! 1. **Obstacle points, each with its side** (left or right of the path):
//!    - every cut item: its two spoke tips, `a` (end of the spoke `p a`) on `p`'s side and `b`
//!      (end of `b q`) on `q`'s side (the tips hold half the width, the clearance and the items
//!      between, [`super::radii::tip_point`]). A stretch that crosses one cut more than once: the
//!      sides alternate along the cut (the band is a simple curve: each crossing changes side).
//!    - every wrap: its arc at the stack radius, sampled inside the wrap's wedge (the octagon's
//!      vertices in the octagonal norm, a circumscribed polygon of `arc_step_deg` otherwise), on the
//!      wrapped vertex's side.
//!    - every feature of another net near the band (pads, via discs, holes, keepouts, junctions):
//!      its outline grown by half the width and its clearance (the Minkowski sum with the norm's
//!      circumscribed unit polygon; the convex hull for a convex feature), on the feature's side.
//!      A feature at an end of an over-full cut the stretch is on is left out (the necked tips
//!      hold that clearance; it is a predicted violation).
//!    - a frame beyond the corridor on both sides, so that both sides exist everywhere.
//! 2. **Sleeve.** A Delaunay triangulation of the points and the two terminals ([`Cdt`]). A
//!    triangle with a left and a right point has two edges between the classes; such triangles
//!    chain from the start terminal to the end terminal (the contour between the two classes):
//!    that chain is the sleeve, its edges between the classes the portals in path order. Points
//!    that coincide with opposite sides are counted (`conflicts`); a chain that does not reach the
//!    end is a failure: no ERBS, the stretch is drawn the sequential way and counted.
//! 3. **Funnel** through the portals ([`super::funnel::funnel`]): every bend of the path lies on an
//!    obstacle point and turns toward it.
//!
//! **Check.** All ERBS of a layer against each other (different nets: half widths plus KiCad's
//! pairwise clearance) and against the sheet's features (pads, vias, holes, the outline). Dayan
//! PDF p. 26 claims SURF's spacing holds when the sketch is feasible; the proofs were not
//! obtained (inflated-sketch §2.5), so it is measured: a violation where an over-full cut carries
//! both parties (or the plan crosses them) is **predicted**; any other is an **`erbs_gap`**.
//!
//! Debug: `TOPO_RADII` puts the ERBS counts per layer in the cut report's summary;
//! `TOPO_ERBS_DUMP=<dir>` writes every layer's ERBS (cut report: `erbs-<layer>.json`, drawing:
//! `realise-L<n>.json`; read by `harness/erbs_accept.py`); `TOPO_ERBS_TIME` one line per stretch.
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
}

impl Default for ErbsOpts {
    fn default() -> Self {
        ErbsOpts { arc_step_deg: 22.5, deadline: None, stop: None }
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
    super::pack::funnel(ports)
}

/// One stretch's obstacle points and their sides.
struct Points {
    pts: Vec<Vec2>,
    /// 0 start, 1 end, 2 left, 3 right.
    lab: Vec<u8>,
    gate: Vec<Option<usize>>,
    w: Vec<f64>,
    seen: HashMap<(u64, u64), usize>,
    conflicts: usize,
}

impl Points {
    fn new(s: Vec2, e: Vec2, w: f64) -> Points {
        let mut seen = HashMap::new();
        seen.insert(key(s), 0);
        seen.entry(key(e)).or_insert(1);
        Points { pts: vec![s, e], lab: vec![0, 1], gate: vec![None, None], w: vec![w, w], seen, conflicts: 0 }
    }

    /// Add a point (the first of coincident points stays; one of the other side is a conflict).
    fn add(&mut self, p: Vec2, left: bool, gate: Option<usize>, w: f64) {
        if !(p.x.is_finite() && p.y.is_finite()) {
            return;
        }
        let lab = if left { 2 } else { 3 };
        match self.seen.get(&key(p)) {
            Some(&i) => {
                if self.lab[i] >= 2 && self.lab[i] != lab {
                    self.conflicts += 1;
                }
            }
            None => {
                self.seen.insert(key(p), self.pts.len());
                self.pts.push(p);
                self.lab.push(lab);
                self.gate.push(gate);
                self.w.push(w);
            }
        }
    }
}

/// The sleeve's portals from `pts[0]` to `pts[1]` in the triangulation of the points: the edges
/// between a left and a right point of the chain of mixed triangles (the contour between the two
/// classes), as point indices `(left, right)`. `Err` when the chain does not reach the end.
/// O(n log n).
fn sleeve(pp: &Points, dir0: Vec2) -> Result<Vec<(usize, usize)>, String> {
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
    // Labels by canonical vertex: 0 start, 1 end, 2 left, 3 right.
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
    let is_l = |c: usize| lab(c) == 2 || c == ce;
    let is_r = |c: usize| lab(c) == 3 || c == ce;
    // Start: a triangle (s, x, y) counter-clockwise with x right and y left of the path; where
    // several qualify (inconsistent sides round the start), the one whose opening faces the band.
    let mut start: Option<(f64, usize, usize)> = None;
    for f in &topo.faces {
        let Some(k) = f.iter().position(|&c| c == cs) else { continue };
        let (x, y) = (f[(k + 1) % 3], f[(k + 2) % 3]);
        if !(is_r(x) && is_l(y)) || (x == ce && y == ce) {
            continue;
        }
        let px = cdt.vertex_pos(x);
        let py = cdt.vertex_pos(y);
        let s = cdt.vertex_pos(cs);
        let mid = (px + py) * 0.5 - s;
        let score = mid.unit().map_or(-2.0, |u| u.dot(dir0));
        if start.is_none_or(|b| score > b.0) {
            start = Some((score, x, y));
        }
    }
    let Some((_, x, y)) = start else { return Err("no sleeve leaves the start (both sides not present round it)".into()) };
    if x == ce || y == ce {
        return Ok(Vec::new());
    }
    let mut out = vec![(inp[y], inp[x])];
    let (mut l, mut r) = (y, x);
    for _ in 0..=topo.faces.len() {
        let Some(&fi) = face_of.get(&(l, r)) else { return Err("the sleeve left the triangulation (the hull)".into()) };
        let f = topo.faces[fi];
        let k = f.iter().position(|&c| c == l).unwrap_or(0);
        let z = f[(k + 2) % 3];
        if z == ce {
            return Ok(out);
        }
        // The new portal: (z, r) for a left z, (l, z) for a right z.
        match lab(z) {
            2 => l = z,
            3 => r = z,
            _ => return Err("the sleeve came back to the start".into()),
        }
        out.push((inp[l], inp[r]));
    }
    Err("the sleeve does not end (a loop)".into())
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

/// The ERBS of stretch `si` (module documentation, steps 1-3). `items`: its cut items `(cut,
/// item)`; `radius`: stack radius per (vertex, stretch, corner); `skip`: features at the ends of
/// over-full cuts it is on. O(n log n) in the obstacle points.
#[allow(clippy::too_many_arguments)]
fn erbs_stretch(sh: &LayerSheet, fg: &Grid, fclear: f64, s: &Stretch, si: usize, o: &LayerOrder, items: &[(usize, usize)], radius: &BTreeMap<(usize, usize, usize), f64>, skip: &BTreeSet<usize>, opts: &ErbsOpts) -> Result<Erbs, String> {
    let band = &s.band;
    let (c0, c1) = (band[0], band[band.len() - 1]);
    let (dirs, grow) = norm_dirs(sh.norm, opts.arc_step_deg);
    let mut gates: Vec<Gate> = Vec::new();
    // Obstacle candidates before the corridor filter: (point, left, gate, width).
    let mut cand: Vec<(Vec2, bool, usize, f64)> = Vec::new();

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
            let a_left = region0 ^ (k % 2 == 1);
            let gi = gates.len();
            gates.push(Gate { a, b, w: x.w_tip, src: GateSrc::Cut(ci), p: c.p, q: c.q, p_left: a_left });
            cand.push((a, a_left, gi, x.w_tip));
            cand.push((b, !a_left, gi, x.w_tip));
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
            cand.push((a, left, gi, w));
        }
    }

    // The corridor: as far as any obstacle on the wrong side of the band pushes the path, plus
    // twice the widest grown feature and 0.5 mm.
    let rmax = 0.5 * s.w + fclear.max(s.s);
    let push = cand.iter().map(|&(p, left, _, _)| {
        let (l, d) = band_side(band, p);
        if l != left {
            d
        } else {
            0.0
        }
    });
    let push = push.fold(0.0, f64::max).min(5.0e6);
    let reach = push + 2.0 * rmax + 0.5e6;

    // 1c. Features of other nets near the band, grown by the half width and their clearance.
    let it = Item { net: s.net, w: s.w, s: s.s };
    let (lo, hi) = band.iter().fold((Vec2 { x: f64::INFINITY, y: f64::INFINITY }, Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY }), |(lo, hi), p| (Vec2 { x: lo.x.min(p.x), y: lo.y.min(p.y) }, Vec2 { x: hi.x.max(p.x), y: hi.y.max(p.y) }));
    let mut near = BTreeSet::new();
    fg.query(lo - Vec2 { x: reach, y: reach }, hi + Vec2 { x: reach, y: reach }, &mut near);
    let wrapped: BTreeMap<usize, bool> = s.wraps.iter().flatten().filter_map(|&(v, l)| sh.vfeat[v].map(|f| (f, l))).collect();
    for &f in &near {
        let ft = &sh.feats[f];
        let cl = ft.bound.clearance(&it);
        if cl <= 0.0 || skip.contains(&f) || ft.poly.is_empty() {
            continue;
        }
        let d = band.windows(2).map(|w| seg_poly_dist(w[0], w[1], &ft.poly)).fold(f64::INFINITY, f64::min);
        if d > reach {
            continue;
        }
        let left = match wrapped.get(&f) {
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
        let r = 0.5 * s.w + cl;
        for x in grown(&ft.poly, r, &dirs, grow) {
            let root = ft.poly.iter().copied().min_by(|a, b| (*a - x).len2().total_cmp(&(*b - x).len2())).unwrap_or(x);
            let gi = gates.len();
            gates.push(Gate { a: x, b: x, w: s.w, src: GateSrc::Feature(f), p: root, q: root, p_left: left });
            cand.push((x, left, gi, s.w));
        }
    }

    // The points: tips and arcs first (a coincident feature point is the same obstacle), the
    // corridor only, then the frame.
    let mut pp = Points::new(c0, c1, s.w);
    for &(p, left, gi, w) in &cand {
        if band_dist(band, p) <= reach {
            pp.add(p, left, Some(gi), w);
        }
    }
    let n_obs = pp.pts.len() - 2;
    let fr = reach + 1.0e6;
    let nb = band.len();
    for k in 0..nb {
        let din = if k > 0 { (band[k] - band[k - 1]).unit() } else { None };
        let dout = if k + 1 < nb { (band[k + 1] - band[k]).unit() } else { None };
        let Some(d) = (match (din, dout) {
            (Some(a), Some(b)) => (a + b).unit().or(Some(b)),
            (a, b) => a.or(b),
        }) else {
            continue;
        };
        let nl = d.perp();
        let mut frame = vec![(band[k] + nl * fr, true), (band[k] - nl * fr, false)];
        if k == 0 || k + 1 == nb {
            let back = if k == 0 { -d } else { d };
            frame.push((band[k] + back * fr + nl * (0.5 * fr), true));
            frame.push((band[k] + back * fr - nl * (0.5 * fr), false));
        }
        for (x, left) in frame {
            let (l, dd) = band_side(band, x);
            if l == left && dd >= 0.5 * fr {
                pp.add(x, left, None, s.w);
            }
        }
    }

    // 2. The sleeve, 3. the funnel.
    let dir0 = (band[1] - band[0]).unit().unwrap_or(Vec2 { x: 1.0, y: 0.0 });
    let portals = sleeve(&pp, dir0)?;
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
    Ok(Erbs { stretch: si, len: poly_len(&path), band_len: poly_len(band), pts, w, at, path, gate_list: gates, points: n_obs, portals: portals.len(), conflicts: pp.conflicts, converged: true })
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
        let r = erbs_stretch(sh, &fg, fclear, s, si, o, per.get(&si).unwrap_or(&none), &radius, squeezed.get(&si).unwrap_or(&no_skip), opts);
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

fn poly_len(p: &[Vec2]) -> f64 {
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
    let mut worst: BTreeMap<(usize, Party), Violation> = BTreeMap::new();
    let mut put = |v: Violation| {
        let e = worst.entry((v.stretch, v.other)).or_insert(v);
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
                put(Violation { stretch: si, other: Party::Feature(f), at: (a + b) * 0.5, amount: req - dist, predicted: pred_feat.contains(&(si, f)) });
            }
        }
        if let Some(ef) = sh.edge_feat {
            if let Bound::Edge { clear } = sh.feats[ef].bound {
                let req = 0.5 * it.w + clear;
                for &(p, q) in &sh.outline {
                    let dist = seg_seg_dist(a, b, p, q);
                    if dist < req - 1.0 {
                        put(Violation { stretch: si, other: Party::Edge, at: (a + b) * 0.5, amount: req - dist, predicted: pred_feat.contains(&(si, ef)) });
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
pub fn realise(st: &super::state::State, events: &[Vec<super::refine::LEv>], packer: &super::pack::Packer, bottom: i32, conn: &dyn Fn(usize) -> Option<usize>, placed: &BTreeMap<usize, Vec<(usize, Vec2)>>, outline: &[(Vec2, Vec2)], mo: &super::cutflow::MeasureOpts, layers: &[usize], given: &[usize], deadline: Option<std::time::Instant>, stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>) -> Result<Realised, String> {
    let t0 = std::time::Instant::now();
    let none: Vec<Vec<usize>> = Vec::new();
    let plan = super::cutflow::Plan { st, events, via_k: &none, bottom, packer, conn, placed: Some(placed) };
    let m = super::cutflow::measure(&plan, outline, mo, layers)?;
    let ords = super::radii::from_measure(&m, &|p| st.proj.regime_at(p));
    let mut out = Realised::default();
    let mut before: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (lf, o) in m.layers.iter().zip(&ords) {
        out.unbanded += lf.walk_failed;
        let eo = ErbsOpts { deadline, stop: stop.clone(), ..ErbsOpts::default() };
        let le = erbs_layer_with(&lf.sheet, &m.stretches, &lf.stretches, o, &eo);
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
            out.runs.entry(s.wire).or_default().push(RunBand { layer: s.layer, s: e.pts[0], e: e.pts[e.pts.len() - 1], path: e.path.clone(), gap: gap.contains(&e.stretch) });
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

    /// The sleeve's path from start to end through `ls` (left) and `rs` (right) points.
    fn path_of(s: Vec2, e: Vec2, ls: &[Vec2], rs: &[Vec2]) -> Result<Vec<Vec2>, String> {
        let mut pp = Points::new(s, e, 0.2e6);
        for &p in ls {
            pp.add(p, true, None, 0.2e6);
        }
        for &p in rs {
            pp.add(p, false, None, 0.2e6);
        }
        let por = sleeve(&pp, (e - s).unit().unwrap())?;
        let mut ports = vec![(s, s)];
        ports.extend(por.iter().map(|&(l, r)| (pp.pts[l], pp.pts[r])));
        ports.push((e, e));
        Ok(simplify(&pull(&ports).into_iter().map(|c| c.0).collect::<Vec<_>>(), 0.5))
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

    /// A tip the band only touches (the with_dropped counter-example class: touch vs crossing):
    /// a left point on the straight band adds no length and no bend.
    #[test]
    fn touched_tip_is_no_detour() {
        let ls = vec![v(3.0, 0.0), v(6.0, 1.0)];
        let rs = vec![v(4.0, -1.0), v(7.0, -1.0)];
        let p = path_of(v(0.0, 0.0), v(10.0, 0.0), &ls, &rs).unwrap();
        assert!((poly_len(&p) - 10e6).abs() < 1.0, "{p:?}");
    }

    /// A start with no right point round it has no sleeve: an error, never a guessed path.
    #[test]
    fn no_sleeve_is_an_error() {
        let ls: Vec<Vec2> = (0..8).map(|k| v((k as f64 * 0.785).cos(), (k as f64 * 0.785).sin())).collect();
        assert!(path_of(v(0.0, 0.0), v(10.0, 0.0), &ls, &[]).is_err());
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

    /// A convex feature grown by r in the octagonal norm: the hull of the vertex octagons, every
    /// point at least r from the feature, none farther than r / cos 22.5.
    #[test]
    fn grown_rectangle() {
        let (dirs, grow) = norm_dirs(Norm::Octagonal, 22.5);
        let poly = vec![v(0.0, 0.0), v(2.0, 0.0), v(2.0, 1.0), v(0.0, 1.0)];
        let g = grown(&poly, 0.3e6, &dirs, grow);
        assert_eq!(g.len(), 16, "{g:?}");
        for x in &g {
            let d = poly_dist(*x, &poly);
            assert!(d >= 0.3e6 - 1.0 && d <= 0.3e6 / FRAC_PI_8.cos() + 1.0, "{d}");
        }
    }

    #[test]
    fn simplify_collinear() {
        let p = simplify(&[v(0.0, 0.0), v(1.0, 0.0), v(1.0, 0.0), v(2.0, 0.0), v(2.0, 1.0)], 0.5);
        assert_eq!(p, vec![v(0.0, 0.0), v(2.0, 0.0), v(2.0, 1.0)]);
    }
}
