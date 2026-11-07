//! U0 of the inflated sketch (docs/dev/inflated-sketch.md §4.1-4.2): the *layer sheet* of one
//! copper layer and its critical cuts, the per-layer model of the Leiserson-Maley cut test.
//!
//! The projection ([`super::state::Projection`]) is one CDT over the obstacles of all layers, so
//! an edge whose end is another layer's pad is no cut on this layer and the true cut behind it is
//! never formed (design §3.2 item 1). A sheet holds only what is copper (or a wall) on its layer:
//!
//! - the pads on the layer and every drilled obstacle (other nets' TH pads are walls on every
//!   layer), each polygon grown outward by its `sag`, so the polygon encloses the round copper
//!   (Maley 1987 p. 29: features are points and segments, and must enclose the element);
//! - keepouts and copper shapes on the layer, the board outline;
//! - planned via discs (a through via is a feature on every layer; U1 passes them in);
//! - junction points (point features, the net's fixed branch terminals);
//! - terminal points (pad and via centres) as plain vertices inside their feature, so a band can
//!   start there. No free Steiner vertices (design §8 item 9): every vertex is on or inside a
//!   feature, so a curve's reduced sequence of crossed edges is its homotopy class.
//!
//! **Critical cuts** (Maley p. 40: "a cut that begins at a feature endpoint and travels to the
//! closest point on another feature"): from every feature vertex `p`, a visibility walk through the
//! sheet's triangles reaches every feature side that `p` sees (portal by portal, the window of
//! directions narrowing at each vertex in front). For every side reached, the side's closest point
//! to `p` in the planning norm is a cut when that point is visible: an end of the side (a
//! vertex-vertex cut) or an interior point (a foot; in a polygonal norm only at a breakpoint
//! direction, since the distance along the side is convex and piecewise linear with its kinks at
//! the breakpoint directions). Kept per side, not per owner, with no distance cap; crossing cuts
//! are kept as records (they are not inserted into the triangulation, unlike `cuts::lct_feet`).
//!
//! **Planning norm**: Euclidean for `[drawing] style = "smooth"`; for "octilinear" the octagonal
//! norm `max(|x|, |y|, (|x| + |y|) / sqrt 2)` (Staepelaere 1992 eq. 1.2, PDF p. 12: the capacity of
//! a cut for octilinear wires). Its unit ball is the octagon with flat sides facing the octilinear
//! directions; it is linear inside each 45 degree cone centred on an octilinear direction, and its
//! breakpoints are the directions 22.5 + k 45 degrees.
//!
//! **Shadow pruning** (Maley eq. 1-3, p. 55): a cut `pq` need not be checked when some point `r`
//! of a feature satisfies `|p-q| = |p-r| + |r-q|` ("shadows include their frontiers"). In the
//! octagonal norm the set of such `r` is the parallelogram `(p + C) ∩ (q - C)` of the linearity
//! cone `C` holding `q - p`, with sides along the two breakpoint directions that bound `C`. (The
//! design's §4.2 calls it Staepelaere's octilinear routing parallelogram, PDF p. 20; that one has
//! sides along the octilinear directions and belongs to the wire-length norm. For this capacity
//! norm the sides are the breakpoint directions: inferred from the norm being linear exactly
//! inside those cones.) A cut at a breakpoint direction has a degenerate parallelogram (the
//! segment itself, which touches no feature) and is never shadowed: Maley's diagonal cuts, kept
//! always (p. 55: "points on these lines are shadowed only by other such points"; design §8 item
//! 12). From every feature vertex the 8 breakpoint rays to the first side they hit are added as
//! such cuts. In the Euclidean norm the shadow is a ray and prunes nothing (p. 55).
//!
//! Complexity: the CDT is O(V log V); the walk from one vertex is O(triangles it sees), small in
//! pin fields and bounded by [`WALK_CAP`]; the shadow test is a grid query over the cut's
//! parallelogram. Deterministic: vertex order, then walk order (a stack), then sorting.

use super::demand::Bound;
use super::state::{point_in_poly, point_in_segments, ObsId, Projection};
use crate::cdt::Cdt;
use crate::geom::Vec2;
use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_4, FRAC_PI_8};

/// Most portals the visibility walk from one vertex may open (a guard; hits are counted).
pub const WALK_CAP: usize = 200_000;

/// The planning norm of the cut test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Norm {
    Euclid,
    /// `max(|x|, |y|, (|x| + |y|) / sqrt 2)`.
    Octagonal,
}

impl Norm {
    /// The norm of the planning metric for a `[drawing] style`.
    pub fn for_style(style: &str) -> Norm {
        if style == "octilinear" {
            Norm::Octagonal
        } else {
            Norm::Euclid
        }
    }

    pub fn len(self, v: Vec2) -> f64 {
        match self {
            Norm::Euclid => v.len(),
            Norm::Octagonal => {
                let (x, y) = (v.x.abs(), v.y.abs());
                x.max(y).max((x + y) * FRAC_1_SQRT_2)
            }
        }
    }
}

/// The 8 breakpoint directions of the octagonal norm, 22.5 + k 45 degrees.
fn breakpoints() -> [Vec2; 8] {
    static BP: std::sync::OnceLock<[Vec2; 8]> = std::sync::OnceLock::new();
    *BP.get_or_init(|| {
        std::array::from_fn(|k| {
            let a = FRAC_PI_8 + k as f64 * FRAC_PI_4;
            Vec2 { x: a.cos(), y: a.sin() }
        })
    })
}

/// What a feature is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatKind {
    /// A projection obstacle present on the layer (pad, drilled obstacle, keepout, copper shape,
    /// existing via).
    Obstacle(ObsId),
    /// The board outline (one island).
    Edge,
    /// A planned via disc (index into the caller's via list).
    Via(usize),
    /// A junction point (index into the caller's junction list).
    Junction(usize),
}

/// One feature (an island of Maley's model): a closed polygon, the outline segments, or a point.
#[derive(Clone, Debug)]
pub struct Feature {
    pub kind: FeatKind,
    /// Counter-clockwise polygon (closed), or a single point (a junction). Empty for the outline,
    /// whose segments are in [`LayerSheet::outline`].
    pub poly: Vec<Vec2>,
    pub bound: Bound,
}

/// A planned via disc: a feature on every layer.
#[derive(Clone, Copy, Debug)]
pub struct ViaFeat {
    pub at: Vec2,
    /// Copper radius (nm).
    pub r: f64,
    pub net: i32,
    /// Clearance the disc demands from other nets (nm).
    pub clear: f64,
}

/// A junction (branch) point: a point feature of its net.
#[derive(Clone, Copy, Debug)]
pub struct JunctionFeat {
    pub at: Vec2,
    pub net: i32,
    pub clear: f64,
}

/// Options of the sheet and its cut set.
#[derive(Clone, Copy, Debug)]
pub struct SheetOpts {
    pub norm: Norm,
    /// `[capacity] cut_radius_mm` in nm: cuts longer (Euclidean) than this are dropped; 0 = off
    /// (exact). With it on, the routability claim is not proven (Maley p. 53).
    pub radius_nm: f64,
    /// Vertices per full turn of a via disc polygon (circumscribed, so it encloses the copper).
    pub via_steps: usize,
    /// Octagonal norm: the visibility walk skips windows that are shadowed as a whole (fast);
    /// false walks everything (the raw critical-cut count of the U0 report).
    pub prune_walk: bool,
}

impl Default for SheetOpts {
    fn default() -> Self {
        SheetOpts { norm: Norm::Octagonal, radius_nm: 0.0, via_steps: 16, prune_walk: true }
    }
}

/// How a cut was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutKind {
    /// Feature vertex to feature vertex (the closest point of a side is its end, or a point
    /// feature).
    Vertex,
    /// Feature vertex to the interior of a side (Euclidean foot).
    Foot,
    /// A breakpoint-direction (diagonal) cut of the octagonal norm: never shadowed.
    Breakpoint,
}

/// One critical cut of a layer: from feature vertex `vp` (at `p`) to `q`, a sheet vertex `vq` or
/// an interior point of a side of feature `fq`.
#[derive(Clone, Copy, Debug)]
pub struct Cut {
    pub p: Vec2,
    pub q: Vec2,
    pub vp: usize,
    pub vq: Option<usize>,
    pub fp: usize,
    pub fq: usize,
    /// Length in the planning norm (nm): Maley's capacity, the features having zero width.
    pub cap: f64,
    pub kind: CutKind,
}

/// Sizes and times of one sheet (U0 gate numbers).
#[derive(Clone, Copy, Debug, Default)]
pub struct SheetStats {
    pub features: usize,
    pub vertices: usize,
    pub triangles: usize,
    /// Critical cuts found before shadow pruning (breakpoint cuts included).
    pub raw: usize,
    /// Of those, breakpoint-direction cuts (never pruned).
    pub breakpoint: usize,
    /// Dropped by shadowing.
    pub shadowed: usize,
    /// Dropped by `cut_radius_mm`.
    pub beyond_radius: usize,
    /// Kept (raw - shadowed - beyond radius).
    pub kept: usize,
    /// Walks stopped by [`WALK_CAP`].
    pub capped_walks: usize,
    /// Seconds: triangulation, cut walk, shadow pruning.
    pub cdt_s: f64,
    pub walk_s: f64,
    pub shadow_s: f64,
}

/// A uniform grid over segments (by bounding box). Deterministic: ids in insertion order per cell.
#[derive(Clone, Debug)]
pub struct SegGrid {
    lo: Vec2,
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

impl SegGrid {
    /// A grid over the segments `segs`, each registered in the cells it passes through (cell
    /// about 1.5 x the mean spacing, at least 50 um).
    pub fn new(segs: &[(Vec2, Vec2)], lo: Vec2, hi: Vec2) -> SegGrid {
        let mut g = SegGrid::empty(segs.len(), lo, hi);
        for (i, &(a, b)) in segs.iter().enumerate() {
            g.walk(a, b, |c: &mut Vec<u32>| c.push(i as u32));
        }
        g
    }

    /// A grid over boxes `(lo, hi)`, each registered in every cell it overlaps (areas).
    pub fn boxes(boxes: &[(Vec2, Vec2)], lo: Vec2, hi: Vec2) -> SegGrid {
        let mut g = SegGrid::empty(boxes.len(), lo, hi);
        for (i, &(a, b)) in boxes.iter().enumerate() {
            let (x0, y0) = g.at(a);
            let (x1, y1) = g.at(b);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    g.cells[y * g.nx + x].push(i as u32);
                }
            }
        }
        g
    }

    fn empty(n: usize, lo: Vec2, hi: Vec2) -> SegGrid {
        let area = ((hi.x - lo.x) * (hi.y - lo.y)).max(1.0);
        let cell = (area / (n.max(1) as f64)).sqrt().max(50_000.0) * 1.5;
        let nx = (((hi.x - lo.x) / cell) as usize + 1).min(4096);
        let ny = (((hi.y - lo.y) / cell) as usize + 1).min(4096);
        SegGrid { lo, cell, nx, ny, cells: vec![Vec::new(); nx * ny] }
    }

    /// Visit the cells segment `a b` passes through (Amanatides & Woo), each once.
    fn walk(&mut self, a: Vec2, b: Vec2, mut f: impl FnMut(&mut Vec<u32>)) {
        let (mut cx, mut cy) = self.at(a);
        let (ex, ey) = self.at(b);
        let d = b - a;
        let (sx, sy): (i64, i64) = (if d.x > 0.0 { 1 } else { -1 }, if d.y > 0.0 { 1 } else { -1 });
        let rel = |v: f64, lo: f64| (v - lo) / self.cell;
        let (fx, fy) = (rel(a.x, self.lo.x), rel(a.y, self.lo.y));
        let next_x = if sx > 0 { cx as f64 + 1.0 } else { cx as f64 };
        let next_y = if sy > 0 { cy as f64 + 1.0 } else { cy as f64 };
        let (dxc, dyc) = (d.x / self.cell, d.y / self.cell);
        let mut tmx = if dxc.abs() > 1e-15 { (next_x - fx) / dxc } else { f64::INFINITY };
        let mut tmy = if dyc.abs() > 1e-15 { (next_y - fy) / dyc } else { f64::INFINITY };
        let tdx = if dxc.abs() > 1e-15 { (1.0 / dxc).abs() } else { f64::INFINITY };
        let tdy = if dyc.abs() > 1e-15 { (1.0 / dyc).abs() } else { f64::INFINITY };
        let nx = self.nx;
        let mut guard = self.nx + self.ny + 4;
        loop {
            f(&mut self.cells[cy * nx + cx]);
            if (cx, cy) == (ex, ey) || guard == 0 {
                break;
            }
            guard -= 1;
            if tmx < tmy {
                let n = cx as i64 + sx;
                if n < 0 || n >= self.nx as i64 {
                    break;
                }
                cx = n as usize;
                tmx += tdx;
            } else {
                let n = cy as i64 + sy;
                if n < 0 || n >= self.ny as i64 {
                    break;
                }
                cy = n as usize;
                tmy += tdy;
            }
        }
    }

    fn at(&self, p: Vec2) -> (usize, usize) {
        let x = (((p.x - self.lo.x) / self.cell).floor().max(0.0) as usize).min(self.nx - 1);
        let y = (((p.y - self.lo.y) / self.cell).floor().max(0.0) as usize).min(self.ny - 1);
        (x, y)
    }

    /// Ids of the segments whose box may meet the box `lo..hi`, ascending, deduplicated.
    pub fn query(&self, lo: Vec2, hi: Vec2, out: &mut Vec<u32>) {
        out.clear();
        let (x0, y0) = self.at(lo);
        let (x1, y1) = self.at(hi);
        for y in y0..=y1 {
            for x in x0..=x1 {
                out.extend_from_slice(&self.cells[y * self.nx + x]);
            }
        }
        out.sort_unstable();
        out.dedup();
    }
}

/// The sheet of one layer: its features, their triangulation and the critical cuts.
pub struct LayerSheet {
    pub l: usize,
    pub norm: Norm,
    pub feats: Vec<Feature>,
    /// Outline segments (feature [`LayerSheet::edge_feat`]).
    pub outline: Vec<(Vec2, Vec2)>,
    pub edge_feat: Option<usize>,
    pub cdt: Cdt,
    pub pos: Vec<Vec2>,
    /// Feature of each vertex; `None` for a terminal point inside a feature.
    pub vfeat: Vec<Option<usize>>,
    pub tris: Vec<[usize; 3]>,
    /// Inside a feature or outside the board.
    pub tri_wall: Vec<bool>,
    pub vert_tris: Vec<Vec<usize>>,
    /// Edge of side `i` (`v_i`, `v_i+1`) of each triangle.
    tri_e: Vec<[usize; 3]>,
    edges: Vec<[usize; 2]>,
    edge_con: Vec<bool>,
    edge_tris: Vec<[Option<usize>; 2]>,
    /// Feature of each constraint edge (`None` for other edges).
    edge_of: Vec<Option<usize>>,
    /// Feature sides `(a, b, feature)`, with points as zero-length sides, and their grid.
    sides: Vec<(Vec2, Vec2, usize)>,
    side_grid: SegGrid,
    pub cuts: Vec<Cut>,
    /// Per vertex: the cuts with an end there.
    pub vcuts: Vec<Vec<usize>>,
    pub cut_grid: SegGrid,
    pub stats: SheetStats,
}

/// `poly` (counter-clockwise, convex or not) with every side moved outward by `d` (miter join;
/// a polygon of a round shape becomes the circumscribed one).
pub fn grow(poly: &[Vec2], d: f64) -> Vec<Vec2> {
    let n = poly.len();
    if d <= 0.0 || n < 3 {
        return poly.to_vec();
    }
    let normal = |a: Vec2, b: Vec2| -> Vec2 {
        let e = b - a;
        let l = e.len().max(1e-9);
        Vec2 { x: e.y / l, y: -e.x / l }
    };
    (0..n)
        .map(|i| {
            let (a, v, b) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
            let (n1, n2) = (normal(a, v), normal(v, b));
            let k = 1.0 + n1.dot(n2);
            if k < 0.1 {
                // A spike (turn of nearly 180 degrees): bevel to the side normal.
                v + n1 * d
            } else {
                v + (n1 + n2) * (d / k)
            }
        })
        .collect()
}

/// A circumscribed `steps`-gon of the disc `c`, `r` (counter-clockwise).
pub fn disc_poly(c: Vec2, r: f64, steps: usize) -> Vec<Vec2> {
    let steps = steps.max(4);
    let rc = r / (std::f64::consts::PI / steps as f64).cos();
    (0..steps)
        .map(|i| {
            let a = i as f64 * 2.0 * std::f64::consts::PI / steps as f64;
            c + Vec2 { x: rc * a.cos(), y: rc * a.sin() }
        })
        .collect()
}

/// The closest point of segment `x y` to `p` in `norm`: `(t, point)`, ties by Euclid.
pub fn closest_on_seg(norm: Norm, p: Vec2, x: Vec2, y: Vec2) -> (f64, Vec2) {
    let e = y - x;
    let l2 = e.len2();
    if l2 <= 0.0 {
        return (0.0, x);
    }
    match norm {
        Norm::Euclid => {
            let t = ((p - x).dot(e) / l2).clamp(0.0, 1.0);
            (t, x + e * t)
        }
        Norm::Octagonal => {
            let mut cand = [0.0f64; 10];
            cand[1] = 1.0;
            let mut nc = 2;
            for b in breakpoints() {
                let den = b.cross(e);
                if den.abs() > 1e-12 * e.len() {
                    let s = -b.cross(x - p) / den;
                    if s > 0.0 && s < 1.0 {
                        cand[nc] = s;
                        nc += 1;
                    }
                }
            }
            let mut best = (f64::INFINITY, f64::INFINITY, 0.0);
            for &s in &cand[..nc] {
                let q = x + e * s;
                let key = (norm.len(q - p), (q - p).len());
                if key.0 < best.0 - 1e-6 || (key.0 <= best.0 + 1e-6 && key.1 < best.1) {
                    best = (key.0, key.1, s);
                }
            }
            (best.2, x + e * best.2)
        }
    }
}

/// Whether direction `d` lies strictly inside the window from `r` counter-clockwise to `l`
/// (a window narrower than 180 degrees).
fn inside(r: Vec2, l: Vec2, d: Vec2) -> bool {
    let tol = 1e-12 * d.len();
    r.cross(d) > tol * r.len() && d.cross(l) > tol * l.len()
}

/// The linearity cone of the octagonal norm strictly holding direction `d` (cone `k` is centred
/// on `k x 45` degrees); `None` on a breakpoint direction (within 1e-9 rad).
fn cone_strict(d: Vec2) -> Option<usize> {
    let a = d.angle();
    let k = (a / FRAC_PI_4).round();
    let off = (a - k * FRAC_PI_4).abs();
    if off >= FRAC_PI_8 - 1e-9 {
        return None;
    }
    Some(((k as i64).rem_euclid(8)) as usize)
}

/// The cones whose closed set holds `d` (one, or two on a breakpoint direction).
fn cones_closed(d: Vec2) -> ConeSet {
    if let Some(k) = cone_strict(d) {
        return ConeSet([k, k], 1);
    }
    let a = d.angle();
    let lo = ((a - FRAC_PI_8) / FRAC_PI_4).round() as i64;
    ConeSet([lo.rem_euclid(8) as usize, (lo + 1).rem_euclid(8) as usize], 2)
}

/// One or two cone indices.
#[derive(Clone, Copy)]
struct ConeSet([usize; 2], usize);

impl ConeSet {
    fn iter(self) -> impl Iterator<Item = usize> {
        self.0.into_iter().take(self.1)
    }
    fn contains(self, k: usize) -> bool {
        self.iter().any(|x| x == k)
    }
}

/// [`seg_in_para`] with the segment already in cone coordinates `(a, b)`.
fn seg_in_para_ab(ua: f64, ub: f64, va: f64, vb: f64, qa: f64, qb: f64) -> bool {
    const TOL: f64 = 0.5;
    if qa <= 1.0 || qb <= 1.0 {
        return false;
    }
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (s0, ds, lo, hi) in [(ua, va - ua, -TOL, qa + TOL), (ub, vb - ub, -TOL, qb + TOL)] {
        if ds.abs() < 1e-12 {
            if s0 < lo || s0 > hi {
                return false;
            }
            continue;
        }
        let (mut ta, mut tb) = ((lo - s0) / ds, (hi - s0) / ds);
        if ta > tb {
            std::mem::swap(&mut ta, &mut tb);
        }
        t0 = t0.max(ta);
        t1 = t1.min(tb);
        if t0 > t1 {
            return false;
        }
    }
    let pt = |t: f64| (ua + (va - ua) * t, ub + (vb - ub) * t);
    let ((a0, b0), (a1, b1)) = (pt(t0), pt(t1));
    // Only the cut's own end points (a side through `p` or `q` leaving the region): no shadow.
    // (Cone coordinates are about lengths: 1 nm of slack.)
    let at = |a: f64, b: f64| (a0 - a).abs() <= 1.0 && (b0 - b).abs() <= 1.0 && (a1 - a).abs() <= 1.0 && (b1 - b).abs() <= 1.0;
    !(at(0.0, 0.0) || at(qa, qb))
}

/// Parameter on segment `x y` where the ray from `p` in direction `d` meets its line.
fn ray_param(p: Vec2, d: Vec2, x: Vec2, y: Vec2) -> f64 {
    let den = d.cross(y - x);
    if den.abs() < 1e-30 {
        return f64::NAN;
    }
    -d.cross(x - p) / den
}

/// What the caller hands the sheet builder.
pub struct SheetInput<'a> {
    pub proj: &'a Projection,
    pub outline: &'a [(Vec2, Vec2)],
    pub vias: &'a [ViaFeat],
    pub junctions: &'a [JunctionFeat],
    /// Extra vertices (pad and via centres where bands start), each inside a feature.
    pub terminals: &'a [Vec2],
}

impl LayerSheet {
    /// The sheet of layer `l` (a projection layer index). O(V log V + walk + pruning).
    pub fn build(inp: &SheetInput, l: usize, o: &SheetOpts) -> Result<LayerSheet, String> {
        let t0 = std::time::Instant::now();
        let pr = inp.proj;
        let mut feats: Vec<Feature> = Vec::new();
        for (oi, ob) in pr.obstacles.iter().enumerate() {
            if !(ob.drilled || ob.layers.has(l)) || ob.poly.len() < 3 {
                continue;
            }
            let bound = if ob.bare { Bound::Hole { clear: ob.clear } } else { Bound::Pad { net: ob.net, clear: ob.clear } };
            feats.push(Feature { kind: FeatKind::Obstacle(oi), poly: grow(&ob.poly, ob.sag), bound });
        }
        for (i, v) in inp.vias.iter().enumerate() {
            feats.push(Feature { kind: FeatKind::Via(i), poly: disc_poly(v.at, v.r, o.via_steps), bound: Bound::Pad { net: Some(v.net), clear: v.clear } });
        }
        for (i, j) in inp.junctions.iter().enumerate() {
            feats.push(Feature { kind: FeatKind::Junction(i), poly: vec![j.at], bound: Bound::Pad { net: Some(j.net), clear: j.clear } });
        }
        let edge_feat = (!inp.outline.is_empty()).then(|| {
            feats.push(Feature { kind: FeatKind::Edge, poly: Vec::new(), bound: Bound::Edge { clear: pr.edge_clear } });
            feats.len() - 1
        });
        // Points, constraints and the feature of each input point.
        let mut pts: Vec<Vec2> = Vec::new();
        let mut pfeat: Vec<Option<usize>> = Vec::new();
        let mut cons: Vec<(usize, usize)> = Vec::new();
        let mut sides: Vec<(Vec2, Vec2, usize)> = Vec::new();
        for (fi, f) in feats.iter().enumerate() {
            let s = pts.len();
            let k = f.poly.len();
            pts.extend(f.poly.iter().copied());
            pfeat.extend(std::iter::repeat_n(Some(fi), k));
            if k >= 3 {
                for j in 0..k {
                    cons.push((s + j, s + (j + 1) % k));
                    sides.push((f.poly[j], f.poly[(j + 1) % k], fi));
                }
            } else if k == 1 {
                sides.push((f.poly[0], f.poly[0], fi));
            }
        }
        if let Some(ef) = edge_feat {
            for &(a, b) in inp.outline {
                let s = pts.len();
                pts.extend([a, b]);
                pfeat.extend([Some(ef), Some(ef)]);
                cons.push((s, s + 1));
                sides.push((a, b, ef));
            }
        }
        for &t in inp.terminals {
            pts.push(t);
            pfeat.push(None);
        }
        let cdt = Cdt::build(&pts, &cons)?;
        let nv = cdt.num_vertices();
        let pos: Vec<Vec2> = (0..nv).map(|v| cdt.vertex_pos(v)).collect();
        let (mut lo, mut hi) = (Vec2 { x: f64::INFINITY, y: f64::INFINITY }, Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY });
        for p in &pos {
            lo = Vec2 { x: lo.x.min(p.x), y: lo.y.min(p.y) };
            hi = Vec2 { x: hi.x.max(p.x), y: hi.y.max(p.y) };
        }
        if nv == 0 {
            lo = Vec2::default();
            hi = lo;
        }
        let side_segs: Vec<(Vec2, Vec2)> = sides.iter().map(|s| (s.0, s.1)).collect();
        let side_grid = SegGrid::new(&side_segs, lo, hi);
        // The feature of a point on a side (split vertices, edges of overlapping features).
        let mut buf: Vec<u32> = Vec::new();
        let mut feat_at = |q: Vec2| -> Option<usize> {
            let pad = Vec2 { x: 2.0, y: 2.0 };
            side_grid.query(q - pad, q + pad, &mut buf);
            buf.iter().map(|&i| &sides[i as usize]).find(|s| seg_dist(q, s.0, s.1) < 2.0).map(|s| s.2)
        };
        let mut vfeat: Vec<Option<usize>> = vec![None; nv];
        let mut seen = vec![false; nv];
        for (i, f) in pfeat.iter().enumerate() {
            let v = cdt.vertex_index(i);
            if !seen[v] {
                seen[v] = true;
                vfeat[v] = *f;
            }
        }
        for v in 0..nv {
            if !seen[v] {
                vfeat[v] = feat_at(pos[v]);
            }
        }
        let topo = cdt.topology();
        let edges: Vec<[usize; 2]> = topo.edges.iter().map(|&(a, b, _)| [a, b]).collect();
        let edge_con: Vec<bool> = topo.edges.iter().map(|&(_, _, c)| c).collect();
        let tris = topo.faces.clone();
        let mut edge_tris = vec![[None, None]; edges.len()];
        let mut vert_tris = vec![Vec::new(); nv];
        let mut tri_e = vec![[0usize; 3]; tris.len()];
        let edge_id = |a: usize, b: usize| edges.binary_search(&[a.min(b), a.max(b)]).expect("triangle side");
        for (t, f) in tris.iter().enumerate() {
            for i in 0..3 {
                let (a, b) = (f[i], f[(i + 1) % 3]);
                let e = edge_id(a, b);
                tri_e[t][i] = e;
                edge_tris[e][if a < b { 0 } else { 1 }] = Some(t);
                vert_tris[a].push(t);
            }
        }
        let mut edge_of: Vec<Option<usize>> = vec![None; edges.len()];
        for (e, &[a, b]) in edges.iter().enumerate() {
            if edge_con[e] {
                edge_of[e] = match (vfeat[a], vfeat[b]) {
                    (Some(x), Some(y)) if x == y => Some(x),
                    _ => feat_at((pos[a] + pos[b]) * 0.5),
                };
            }
        }
        // Walls: outside the board, or inside a feature polygon (bbox prefilter by grid).
        let fboxes: Vec<(Vec2, Vec2)> = feats
            .iter()
            .map(|f| {
                if f.poly.len() < 3 {
                    return (Vec2 { x: 1.0, y: 1.0 }, Vec2 { x: 0.0, y: 0.0 });
                }
                let mut a = f.poly[0];
                let mut b = a;
                for p in &f.poly {
                    a = Vec2 { x: a.x.min(p.x), y: a.y.min(p.y) };
                    b = Vec2 { x: b.x.max(p.x), y: b.y.max(p.y) };
                }
                (a, b)
            })
            .collect();
        let fgrid = SegGrid::boxes(&fboxes.iter().map(|&(a, b)| if a.x <= b.x { (a, b) } else { (lo, lo) }).collect::<Vec<_>>(), lo, hi);
        let tri_wall: Vec<bool> = tris
            .iter()
            .map(|f| {
                let c = (pos[f[0]] + pos[f[1]] + pos[f[2]]) * (1.0 / 3.0);
                if !inp.outline.is_empty() && !point_in_segments(c, inp.outline) {
                    return true;
                }
                let mut ids = Vec::new();
                fgrid.query(c, c, &mut ids);
                ids.iter().any(|&i| {
                    let (a, b) = fboxes[i as usize];
                    let f = &feats[i as usize];
                    f.poly.len() >= 3 && c.x >= a.x && c.x <= b.x && c.y >= a.y && c.y <= b.y && point_in_poly(c, &f.poly)
                })
            })
            .collect();
        let cdt_s = t0.elapsed().as_secs_f64();
        let cut_grid = SegGrid::new(&[], lo, hi);
        let mut sh = LayerSheet {
            l,
            norm: o.norm,
            feats,
            outline: inp.outline.to_vec(),
            edge_feat,
            cdt,
            pos,
            vfeat,
            tris,
            tri_wall,
            vert_tris,
            tri_e,
            edges,
            edge_con,
            edge_tris,
            edge_of,
            sides,
            side_grid,
            cuts: Vec::new(),
            vcuts: vec![Vec::new(); nv],
            cut_grid,
            stats: SheetStats::default(),
        };
        sh.stats.features = sh.feats.len();
        sh.stats.vertices = nv;
        sh.stats.triangles = sh.tris.len();
        sh.stats.cdt_s = cdt_s;
        sh.find_cuts(o);
        Ok(sh)
    }

    fn across(&self, e: usize, t: usize) -> Option<usize> {
        match self.edge_tris[e] {
            [Some(a), b] if a == t => b,
            [a, Some(b)] if b == t => a,
            _ => None,
        }
    }

    /// Whether vertex `v` borders free space (some incident triangle is no wall).
    pub fn free_vertex(&self, v: usize) -> bool {
        self.vert_tris[v].iter().any(|&t| !self.tri_wall[t])
    }

    /// The critical cuts of every feature vertex, then shadow pruning and the radius option.
    ///
    /// Shadow pruning (octagonal norm) uses the feature points the walk from `p` saw (vertices in
    /// its windows, the visible ends of the sides it hit, the breakpoint hit points), in cone
    /// coordinates per linearity cone: `q` is shadowed by `r` in cone `C` when `r - p` and
    /// `q - r` both lie in `C`, i.e. `r` is at most `q` in both coordinates. Two side shadows are
    /// added: a side of `p`'s own feature that runs into `C` shadows every cut from `p` strictly
    /// inside `C` (its points arbitrarily close to `p`), and a side of `q`'s feature that runs from
    /// `q` back into `-C` shadows `p q`. With `prune_walk` the walk does not enter a window that
    /// lies inside one cone and is shadowed as a whole (by one such point, or a killed cone).
    /// A feature point hidden from `p` behind a long side is not used: the pruning is
    /// conservative (it keeps a cut that the exact test, [`LayerSheet::shadowed`], may drop).
    fn find_cuts(&mut self, o: &SheetOpts) {
        let t0 = std::time::Instant::now();
        let bps = breakpoints();
        let oct = self.norm == Norm::Octagonal;
        let prune = oct && o.prune_walk;
        let nv = self.pos.len();
        let (e1, e2): ([Vec2; 8], [Vec2; 8]) = (
            std::array::from_fn(|k| Vec2 { x: (k as f64 * FRAC_PI_4 - FRAC_PI_8).cos(), y: (k as f64 * FRAC_PI_4 - FRAC_PI_8).sin() }),
            std::array::from_fn(|k| Vec2 { x: (k as f64 * FRAC_PI_4 + FRAC_PI_8).cos(), y: (k as f64 * FRAC_PI_4 + FRAC_PI_8).sin() }),
        );
        let det = FRAC_PI_4.sin();
        let ab = |k: usize, d: Vec2| -> (f64, f64) { (d.cross(e2[k]) / det, e1[k].cross(d) / det) };
        // Directions of each vertex's own feature sides.
        let mut vside: Vec<Vec<Vec2>> = vec![Vec::new(); nv];
        for (e, &[a, b]) in self.edges.iter().enumerate() {
            if self.edge_con[e] && self.edge_of[e].is_some() && self.edge_of[e] == self.vfeat[a] && self.edge_of[e] == self.vfeat[b] {
                vside[a].push(self.pos[b] - self.pos[a]);
                vside[b].push(self.pos[a] - self.pos[b]);
            }
        }
        const TOL: f64 = 0.5; // nm: the frontier belongs to the shadow
        let mut raw_vv: Vec<Cut> = Vec::new();
        let mut raw_other: Vec<Cut> = Vec::new();
        let mut shadowed_vv: Vec<(usize, usize)> = Vec::new();
        let mut kept: Vec<Cut> = Vec::new();
        let mut stack: Vec<(usize, usize, usize, usize, Vec2, Vec2)> = Vec::new();
        let mut front: [Vec<(f64, f64)>; 8] = Default::default();
        let mut mine: Vec<Cut> = Vec::new();
        let mut hit: Vec<(Vec2, Vec2)> = Vec::new();
        // Per cone: the hit sides' end points in cone coordinates, and their least a and b.
        let mut hit_ab: [Vec<(f64, f64, f64, f64)>; 8] = Default::default();
        let mut shadowed_n = 0usize;
        for p in 0..nv {
            let Some(fp) = self.vfeat[p] else { continue };
            if !self.free_vertex(p) {
                continue;
            }
            let pp = self.pos[p];
            for f in front.iter_mut() {
                f.clear();
            }
            let mut dead = [false; 8];
            if oct {
                for &d in &vside[p] {
                    for k in cones_closed(d).iter() {
                        dead[k] = true;
                    }
                }
            }
            mine.clear();
            hit.clear();
            let seen = |front: &mut [Vec<(f64, f64)>; 8], q: Vec2| {
                if oct {
                    let d = q - pp;
                    if d.len2() > 1.0 {
                        for k in cones_closed(d).iter() {
                            // Only the minimal points (the staircase) are kept: a point
                            // dominated by another shadows nothing that one does not.
                            let (a, b) = ab(k, d);
                            let f = &mut front[k];
                            if f.iter().any(|&(x, y)| x <= a + TOL && y <= b + TOL) {
                                continue;
                            }
                            f.retain(|&(x, y)| !(a <= x + TOL && b <= y + TOL));
                            f.push((a, b));
                        }
                    }
                }
            };
            let norm = self.norm;
            let add = |mine: &mut Vec<Cut>, vq: Option<usize>, fq: usize, q: Vec2, kind: CutKind| {
                if fq != fp && (q - pp).len2() >= 1.0 {
                    mine.push(Cut { p: pp, q, vp: p, vq, fp, fq, cap: norm.len(q - pp), kind });
                }
            };
            // Whether the region beyond portal `x y` inside window `r l` is shadowed as a whole.
            let shadow_window = |front: &[Vec<(f64, f64)>; 8], dead: &[bool; 8], x: Vec2, y: Vec2, r: Vec2, l: Vec2| -> bool {
                let (Some(k), Some(k2)) = (cone_strict(r), cone_strict(l)) else { return false };
                if k != k2 {
                    return false;
                }
                if dead[k] {
                    return true;
                }
                let (sr, sl) = (ray_param(pp, r, x, y), ray_param(pp, l, x, y));
                if !sr.is_finite() || !sl.is_finite() {
                    return false;
                }
                let (xa, xb) = ab(k, x + (y - x) * sr.clamp(0.0, 1.0) - pp);
                let (ya, yb) = ab(k, x + (y - x) * sl.clamp(0.0, 1.0) - pp);
                let (ma, mb) = (xa.min(ya), xb.min(yb));
                front[k].iter().any(|&(a, b)| a + b > TOL && a <= ma + TOL && b <= mb + TOL)
            };
            stack.clear();
            for &t in &self.vert_tris[p] {
                if self.tri_wall[t] {
                    continue;
                }
                let f = self.tris[t];
                let i = f.iter().position(|&x| x == p).expect("corner");
                let (a, b) = (f[(i + 1) % 3], f[(i + 2) % 3]);
                for c in [a, b] {
                    if let Some(fc) = self.vfeat[c] {
                        seen(&mut front, self.pos[c]);
                        if self.feats[fc].poly.len() == 1 {
                            add(&mut mine, Some(c), fc, self.pos[c], CutKind::Vertex);
                        }
                    }
                }
                let (ra, lb) = (self.pos[a] - pp, self.pos[b] - pp);
                let e = self.tri_e[t][(i + 1) % 3];
                if prune {
                    // One window per cone, so a shadowed window can be skipped as a whole.
                    let mut from = ra;
                    for &bd in bps.iter() {
                        if inside(from, lb, bd) {
                            stack.push((t, e, a, b, from, bd));
                            from = bd;
                        }
                    }
                    stack.push((t, e, a, b, from, lb));
                } else {
                    stack.push((t, e, a, b, ra, lb));
                }
            }
            let mut bp_done = [false; 8];
            let mut steps = 0usize;
            while let Some((t, e, x, y, r, l)) = stack.pop() {
                steps += 1;
                if steps > WALK_CAP {
                    self.stats.capped_walks += 1;
                    break;
                }
                let (px, py) = (self.pos[x], self.pos[y]);
                if self.edge_con[e] {
                    // A feature side: its visible part is feature points; its closest point to `p`
                    // when visible; the breakpoint rays.
                    let Some(fe) = self.edge_of[e] else { continue };
                    if oct {
                        hit.push((px, py));
                    }
                    let (sr, sl) = (ray_param(pp, r, px, py), ray_param(pp, l, px, py));
                    if sr.is_finite() && sl.is_finite() {
                        seen(&mut front, px + (py - px) * sr.clamp(0.0, 1.0));
                        seen(&mut front, px + (py - px) * sl.clamp(0.0, 1.0));
                    }
                    if fe == fp {
                        continue;
                    }
                    let (s, q) = closest_on_seg(norm, pp, px, py);
                    if s <= 0.0 {
                        if r.cross(px - pp).abs() <= 1e-9 * r.len() * (px - pp).len() || inside(r, l, px - pp) {
                            add(&mut mine, Some(x), fe, px, CutKind::Vertex);
                        }
                    } else if s >= 1.0 {
                        if l.cross(py - pp).abs() <= 1e-9 * l.len() * (py - pp).len() || inside(r, l, py - pp) {
                            add(&mut mine, Some(y), fe, py, CutKind::Vertex);
                        }
                    } else if !oct && inside(r, l, q - pp) {
                        add(&mut mine, None, fe, q, CutKind::Foot);
                    }
                    if oct {
                        for (k, &bd) in bps.iter().enumerate() {
                            if bp_done[k] || !(inside(r, l, bd) || r == bd || l == bd) {
                                continue;
                            }
                            let s = ray_param(pp, bd, px, py);
                            if !(s > 0.0 && s < 1.0) {
                                continue;
                            }
                            bp_done[k] = true;
                            add(&mut mine, None, fe, px + (py - px) * s, CutKind::Breakpoint);
                        }
                    }
                    continue;
                }
                let Some(t2) = self.across(e, t) else { continue };
                if self.tri_wall[t2] {
                    continue;
                }
                let f2 = self.tris[t2];
                let ci = f2.iter().position(|&v| v != x && v != y).expect("third vertex");
                let c = f2[ci];
                let pc = self.pos[c];
                let dc = pc - pp;
                if inside(r, l, dc) {
                    if let Some(fc) = self.vfeat[c] {
                        seen(&mut front, pc);
                        if self.feats[fc].poly.len() == 1 {
                            add(&mut mine, Some(c), fc, pc, CutKind::Vertex);
                        }
                    }
                }
                // Sides of t2: (c, next) is side ci, (prev, c) is side ci + 2. The portal (x, c)
                // shares x; (c, y) shares y.
                let e_c_next = self.tri_e[t2][ci];
                let e_prev_c = self.tri_e[t2][(ci + 2) % 3];
                let next = f2[(ci + 1) % 3];
                let (exc, ecy) = if next == x { (e_c_next, e_prev_c) } else { (e_prev_c, e_c_next) };
                let l1 = if dc.cross(l) > 0.0 { dc } else { l };
                // `cut_radius_mm`: no portal beyond the radius.
                let far = |u: Vec2, v: Vec2| o.radius_nm > 0.0 && seg_dist(pp, u, v) > o.radius_nm;
                if r.cross(l1) > 0.0 && !far(px, pc) && !(prune && shadow_window(&front, &dead, px, pc, r, l1)) {
                    stack.push((t2, exc, x, c, r, l1));
                }
                let r2 = if r.cross(dc) > 0.0 { dc } else { r };
                if r2.cross(l) > 0.0 && !far(pc, py) && !(prune && shadow_window(&front, &dead, pc, py, r2, l)) {
                    stack.push((t2, ecy, c, y, r2, l));
                }
            }
            // Shadow test of the cuts from `p`.
            if oct {
                for (k, h) in hit_ab.iter_mut().enumerate() {
                    h.clear();
                    h.extend(hit.iter().map(|&(u, v)| {
                        let ((ua, ub), (va, vb)) = (ab(k, u - pp), ab(k, v - pp));
                        (ua, ub, va, vb)
                    }));
                }
            }
            for c in mine.drain(..) {
                let mut sh = false;
                if oct && c.kind != CutKind::Breakpoint {
                    let d = c.q - pp;
                    if let Some(k) = cone_strict(d) {
                        let (qa, qb) = ab(k, d);
                        sh = dead[k]
                            || front[k].iter().any(|&(a, b)| a + b > TOL && a <= qa + TOL && b <= qb + TOL && ((a - qa).abs() > TOL || (b - qb).abs() > TOL))
                            || c.vq.is_some_and(|v| vside[v].iter().any(|&s| cones_closed(-s).contains(k)))
                            || hit_ab[k].iter().any(|&(ua, ub, va, vb)| ua.min(va) <= qa + TOL && ub.min(vb) <= qb + TOL && ua.max(va) >= -TOL && ub.max(vb) >= -TOL && seg_in_para_ab(ua, ub, va, vb, qa, qb));
                    }
                }
                if sh {
                    shadowed_n += 1;
                    if let Some(v) = c.vq {
                        shadowed_vv.push((p.min(v), p.max(v)));
                    }
                }
                if c.vq.is_some() {
                    raw_vv.push(c);
                } else {
                    raw_other.push(c);
                }
                if !sh {
                    kept.push(c);
                }
            }
        }
        self.stats.walk_s = t0.elapsed().as_secs_f64();
        let t1 = std::time::Instant::now();
        // One record per vertex pair (found from both ends); shadowed from either end: dropped.
        let key = |c: &Cut| { let v = c.vq.unwrap_or(usize::MAX); (c.vp.min(v), c.vp.max(v)) };
        raw_vv.sort_by_key(key);
        raw_vv.dedup_by_key(|c| key(c));
        shadowed_vv.sort_unstable();
        shadowed_vv.dedup();
        self.stats.raw = raw_vv.len() + raw_other.len();
        self.stats.breakpoint = raw_other.iter().filter(|c| c.kind == CutKind::Breakpoint).count();
        let _ = shadowed_n;
        let mut out: Vec<Cut> = Vec::with_capacity(kept.len());
        kept.sort_by(|a, b| {
            let (ka, kb) = (if a.vq.is_some() { key(a) } else { (usize::MAX, usize::MAX) }, if b.vq.is_some() { key(b) } else { (usize::MAX, usize::MAX) });
            ka.cmp(&kb).then(a.vp.cmp(&b.vp)).then(a.q.x.total_cmp(&b.q.x)).then(a.q.y.total_cmp(&b.q.y))
        });
        let mut last: Option<(usize, usize)> = None;
        for c in kept {
            if c.vq.is_some() {
                let k = key(&c);
                if last == Some(k) || shadowed_vv.binary_search(&k).is_ok() {
                    continue;
                }
                last = Some(k);
            }
            if o.radius_nm > 0.0 && (c.q - c.p).len() > o.radius_nm {
                self.stats.beyond_radius += 1;
                continue;
            }
            out.push(c);
        }
        self.stats.shadowed = self.stats.raw - out.len() - self.stats.beyond_radius;
        // Canonical order: by first vertex, then the far end.
        out.sort_by(|a, b| a.vp.cmp(&b.vp).then(a.q.x.total_cmp(&b.q.x)).then(a.q.y.total_cmp(&b.q.y)));
        self.stats.shadow_s = t1.elapsed().as_secs_f64();
        self.stats.kept = out.len();
        for (i, c) in out.iter().enumerate() {
            self.vcuts[c.vp].push(i);
            if let Some(v) = c.vq {
                self.vcuts[v].push(i);
            }
        }
        let segs: Vec<(Vec2, Vec2)> = out.iter().map(|c| (c.p, c.q)).collect();
        let (mut lo, mut hi) = (Vec2 { x: f64::INFINITY, y: f64::INFINITY }, Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY });
        for p in &self.pos {
            lo = Vec2 { x: lo.x.min(p.x), y: lo.y.min(p.y) };
            hi = Vec2 { x: hi.x.max(p.x), y: hi.y.max(p.y) };
        }
        if !self.pos.is_empty() {
            self.cut_grid = SegGrid::new(&segs, lo, hi);
        }
        self.cuts = out;
    }

    /// Whether the cut `p q` is shadowed in the octagonal norm: some feature point other than
    /// `p` and `q` lies in the closed parallelogram `(p + C) ∩ (q - C)`, `C` the linearity cone of
    /// `q - p` (see the module doc). A cut at a breakpoint direction is never shadowed.
    pub fn shadowed(&self, p: Vec2, q: Vec2, buf: &mut Vec<u32>) -> bool {
        let d = q - p;
        let k = (d.angle() / FRAC_PI_4).round();
        let c = k * FRAC_PI_4;
        let e1 = Vec2 { x: (c - FRAC_PI_8).cos(), y: (c - FRAC_PI_8).sin() };
        let e2 = Vec2 { x: (c + FRAC_PI_8).cos(), y: (c + FRAC_PI_8).sin() };
        let det = e1.cross(e2);
        let ab = |v: Vec2| -> (f64, f64) { (v.cross(e2) / det, e1.cross(v) / det) };
        let (a_max, b_max) = ab(d);
        // Degenerate (a breakpoint cut): the parallelogram is the segment itself.
        if a_max <= 1.0 || b_max <= 1.0 {
            return false;
        }
        let corners = [p, p + e1 * a_max, p + e2 * b_max, q];
        let lo = Vec2 { x: corners.iter().map(|v| v.x).fold(f64::INFINITY, f64::min), y: corners.iter().map(|v| v.y).fold(f64::INFINITY, f64::min) };
        let hi = Vec2 { x: corners.iter().map(|v| v.x).fold(f64::NEG_INFINITY, f64::max), y: corners.iter().map(|v| v.y).fold(f64::NEG_INFINITY, f64::max) };
        self.side_grid.query(lo, hi, buf);
        const TOL: f64 = 0.5; // nm, the frontier is part of the shadow
        for &i in buf.iter() {
            let (u, v, _) = self.sides[i as usize];
            let (ua, ub) = ab(u - p);
            let (va, vb) = ab(v - p);
            // Liang-Barsky clip of u + t (v - u) against -TOL <= a <= a_max + TOL, same for b.
            let (mut t0, mut t1) = (0.0f64, 1.0f64);
            let mut ok = true;
            for (s0, ds, lo_, hi_) in [(ua, va - ua, -TOL, a_max + TOL), (ub, vb - ub, -TOL, b_max + TOL)] {
                if ds.abs() < 1e-12 {
                    if s0 < lo_ || s0 > hi_ {
                        ok = false;
                        break;
                    }
                    continue;
                }
                let (mut ta, mut tb) = ((lo_ - s0) / ds, (hi_ - s0) / ds);
                if ta > tb {
                    std::mem::swap(&mut ta, &mut tb);
                }
                t0 = t0.max(ta);
                t1 = t1.min(tb);
                if t0 > t1 {
                    ok = false;
                    break;
                }
            }
            if !ok {
                continue;
            }
            let (c0, c1) = (u + (v - u) * t0, u + (v - u) * t1);
            // Only the cut's own end points (a side through `p` or `q` leaving the region): no
            // shadow. Anything else is a feature point in the shadow region.
            let at = |x: Vec2| (c0 - x).len() <= 1.0 && (c1 - x).len() <= 1.0;
            if !(at(p) || at(q)) {
                return true;
            }
        }
        false
    }

    /// The bound of the feature at a cut end.
    pub fn bound(&self, f: usize) -> Bound {
        self.feats[f].bound
    }
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.len2();
    let t = if l2 > 0.0 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + d * t)).len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topo::state::{LayerMask, ObsKind, Obstacle, Projection, VertKind};

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x: x * 1e6, y: y * 1e6 }
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Vec2> {
        vec![v(x0, y0), v(x1, y0), v(x1, y1), v(x0, y1)]
    }

    /// A projection holding only the given obstacles (layer masks as given, 2 layers).
    fn proj(obs: Vec<(Vec<Vec2>, u64, bool)>) -> Projection {
        let obstacles: Vec<Obstacle> = obs
            .into_iter()
            .enumerate()
            .map(|(i, (poly, mask, drilled))| {
                let c = poly.iter().fold(Vec2::default(), |a, &p| a + p) * (1.0 / poly.len() as f64);
                Obstacle { kind: ObsKind::Pad, poly, center: c, layers: LayerMask(mask), drilled, net: Some(i as i32 + 1), clear: 0.2e6, bare: false, sag: 0.0 }
            })
            .collect();
        let mut pts = Vec::new();
        let mut cons = Vec::new();
        for (i, ob) in obstacles.iter().enumerate() {
            let s = pts.len();
            for &p in &ob.poly {
                pts.push((p, VertKind::Outline(i)));
            }
            for j in 0..ob.poly.len() {
                cons.push((s + j, s + (j + 1) % ob.poly.len()));
            }
        }
        Projection::build(2, obstacles, &pts, &cons, |_| true).expect("projection")
    }

    fn sheet(pr: &Projection, l: usize, norm: Norm) -> LayerSheet {
        let inp = SheetInput { proj: pr, outline: &[], vias: &[], junctions: &[], terminals: &[] };
        LayerSheet::build(&inp, l, &SheetOpts { norm, ..SheetOpts::default() }).expect("sheet")
    }

    fn has_cut(sh: &LayerSheet, a: Vec2, b: Vec2) -> bool {
        sh.cuts.iter().any(|c| ((c.p - a).len() < 10.0 && (c.q - b).len() < 10.0) || ((c.p - b).len() < 10.0 && (c.q - a).len() < 10.0))
    }

    #[test]
    fn octagonal_norm_values() {
        let n = Norm::Octagonal;
        assert!((n.len(v(1.0, 0.0)) - 1e6).abs() < 1e-6);
        assert!((n.len(v(FRAC_1_SQRT_2, FRAC_1_SQRT_2)) - 1e6).abs() < 1e-3);
        let a = FRAC_PI_8;
        // At a breakpoint the norm is cos 22.5 of the length: the octilinear capacity.
        assert!((n.len(v(a.cos(), a.sin())) - a.cos() * 1e6).abs() < 1e-3);
    }

    /// Two squares side by side: the facing sides give vertex-vertex cuts between the facing
    /// corners, and both are kept (nothing lies between them).
    #[test]
    fn square_pair_facing_corners() {
        let pr = proj(vec![(rect(0.0, 0.0, 1.0, 1.0), 1, false), (rect(2.0, 0.0, 3.0, 1.0), 1, false)]);
        for norm in [Norm::Euclid, Norm::Octagonal] {
            let sh = sheet(&pr, 0, norm);
            assert!(has_cut(&sh, v(1.0, 0.0), v(2.0, 0.0)), "{norm:?} {:?}", sh.cuts);
            assert!(has_cut(&sh, v(1.0, 1.0), v(2.0, 1.0)), "{norm:?}");
            let c = sh.cuts.iter().find(|c| ((c.p - v(1.0, 0.0)).len() < 10.0 || (c.q - v(1.0, 0.0)).len() < 10.0) && c.kind == CutKind::Vertex && ((c.q - c.p).len() - 1e6).abs() < 10.0).expect("cut");
            assert!((c.cap - 1e6).abs() < 1.0);
        }
    }

    /// Facing pads offset by half a side: the corner of one faces the side of the other. In
    /// Euclid that is a foot (perpendicular); in the octagonal norm the closest point of the side
    /// is the vertex or a breakpoint-direction point, and the foot is not a critical cut.
    #[test]
    fn facing_pads_feet() {
        let pr = proj(vec![(rect(0.0, 0.0, 1.0, 1.0), 1, false), (rect(0.5, 1.5, 1.5, 2.5), 1, false)]);
        let e = sheet(&pr, 0, Norm::Euclid);
        assert!(e.cuts.iter().any(|c| c.kind == CutKind::Foot && ((c.q - c.p).len() - 0.5e6).abs() < 10.0), "{:?}", e.cuts);
        let o = sheet(&pr, 0, Norm::Octagonal);
        assert!(o.cuts.iter().all(|c| c.kind != CutKind::Foot));
        assert!(o.stats.breakpoint > 0);
    }

    /// The other layer's pad does not exist in this layer's sheet: the cut between the two
    /// layer-0 pads runs across where the layer-1 pad is (boardlet B5's per-layer cut).
    #[test]
    fn other_layer_pad_is_no_feature() {
        let pr = proj(vec![(rect(0.0, 0.0, 1.0, 1.0), 1, false), (rect(1.4, 0.2, 1.6, 0.8), 2, false), (rect(2.0, 0.0, 3.0, 1.0), 1, false)]);
        let s0 = sheet(&pr, 0, Norm::Octagonal);
        assert!(has_cut(&s0, v(1.0, 0.0), v(2.0, 0.0)), "{:?}", s0.cuts);
        let s1 = sheet(&pr, 1, Norm::Octagonal);
        assert!(s1.cuts.is_empty(), "one feature on layer 1: no cut");
        // A drilled obstacle is a wall on every layer.
        let pr2 = proj(vec![(rect(0.0, 0.0, 1.0, 1.0), 1, false), (rect(1.4, 0.2, 1.6, 0.8), 2, true), (rect(2.0, 0.0, 3.0, 1.0), 1, false)]);
        let s = sheet(&pr2, 0, Norm::Octagonal);
        assert!(!has_cut(&s, v(1.0, 0.2), v(2.0, 0.2)));
    }

    /// Shadowing (Maley eq. 1-3): A at the origin, a small pad B straight between A and C on the
    /// x axis. In the octagonal norm the cut A-C is shadowed by B (B's points lie in its
    /// parallelogram); in Euclid it is not even a cut (it runs through B) when they line up. With
    /// B moved 0.3 mm off the line, the octagonal shadow still covers it, the Euclidean ray does not.
    #[test]
    fn shadowed_cut() {
        let a = rect(-0.1, -0.1, 0.0, 0.1);
        let b = rect(1.0, 0.25, 1.1, 0.35);
        let c = rect(3.0, -0.1, 3.1, 0.1);
        let pr = proj(vec![(a, 1, false), (b, 1, false), (c, 1, false)]);
        let o = sheet(&pr, 0, Norm::Octagonal);
        assert!(!has_cut(&o, v(0.0, 0.1), v(3.0, 0.1)), "{:?}", o.cuts);
        // Without the pruned walk the cut is found and then dropped as shadowed; the kept sets agree.
        let inp = SheetInput { proj: &pr, outline: &[], vias: &[], junctions: &[], terminals: &[] };
        let full = LayerSheet::build(&inp, 0, &SheetOpts { prune_walk: false, ..SheetOpts::default() }).expect("sheet");
        assert!(!has_cut(&full, v(0.0, 0.1), v(3.0, 0.1)));
        assert!(full.stats.shadowed > 0 && full.stats.raw > o.stats.raw, "{:?} {:?}", full.stats, o.stats);
        assert_eq!(full.stats.kept, o.stats.kept);
        let e = sheet(&pr, 0, Norm::Euclid);
        assert!(has_cut(&e, v(0.0, 0.1), v(3.0, 0.1)), "{:?}", e.cuts);
        let mut buf = Vec::new();
        assert!(o.shadowed(v(0.0, 0.1), v(3.0, 0.1), &mut buf));
    }

    /// A breakpoint-direction cut is never shadowed, whatever lies beside it: the parallelogram
    /// degenerates to the segment.
    #[test]
    fn breakpoint_cut_kept() {
        let ang = FRAC_PI_8;
        let p = Vec2::default();
        let q = Vec2 { x: 3e6 * ang.cos(), y: 3e6 * ang.sin() };
        // Pads around the segment, none touching it.
        let pr = proj(vec![
            (vec![p + v(-0.2, -0.2), p + v(0.0, -0.2), p, p + v(-0.2, 0.0)], 1, false),
            (rect(1.0, 0.6, 1.2, 0.8), 1, false),
            (rect(1.5, 0.3, 1.7, 0.45), 1, false),
            (vec![q, q + v(0.2, 0.0), q + v(0.2, 0.2), q + v(0.0, 0.2)], 1, false),
        ]);
        let o = sheet(&pr, 0, Norm::Octagonal);
        let mut buf = Vec::new();
        assert!(!o.shadowed(p, q, &mut buf));
        assert!(o.cuts.iter().any(|c| c.kind == CutKind::Breakpoint), "{:?}", o.cuts);
        // A cut 10 degrees off the breakpoint with a pad corner inside its parallelogram is.
        let a2 = FRAC_PI_8 - 0.17;
        let q2 = Vec2 { x: 3e6 * a2.cos(), y: 3e6 * a2.sin() };
        let r = Vec2 { x: 1.5e6 * a2.cos(), y: 1.5e6 * a2.sin() } + Vec2 { x: 0.0, y: -0.05e6 };
        let pr2 = proj(vec![(rect(-0.2, -0.2, 0.0, 0.0), 1, false), (vec![r, r + v(0.1, 0.0), r + v(0.1, -0.1), r + v(0.0, -0.1)].into_iter().rev().collect(), 1, false)]);
        let o2 = sheet(&pr2, 0, Norm::Octagonal);
        assert!(o2.shadowed(p, q2, &mut buf));
    }

    #[test]
    fn grow_circumscribes() {
        let sq = rect(0.0, 0.0, 1.0, 1.0);
        let g = grow(&sq, 0.1e6);
        assert!((g[0] - v(-0.1, -0.1)).len() < 1.0, "{g:?}");
        assert!((g[2] - v(1.1, 1.1)).len() < 1.0);
    }
}
