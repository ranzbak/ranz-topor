//! Pour-reach check after routing (M3 pour completion, second half): approximates the zone fill
//! KiCad computes with the router's copper in place and reports the pour-connected pads and vias
//! that lose their copper island, so that `route` can add pour-completion connections for them
//! in the same run (the engine result must be correct without KiCad in the loop).
//!
//! **Fill approximation** per (pour, layer): raster of the pre-route fill polygons (outer rings
//! minus holes — KiCad's fill of the ripped board, which already carries every knockout of the
//! pre-existing copper, the thermal holes and spokes of the pour's own pads) minus the router's
//! foreign-net items dilated by `clearance + cell/4` (KiCad's clearance knockout), opened by
//! `min_thickness/2` (KiCad deflates the fill by half the minimum width before it drops islands:
//! an erosion of the raster by an exact Euclidean distance transform, D4), then 4-connected
//! islands. Checked against KiCad's own refill of wiccon-2023 (D4): the kept islands match one
//! to one, area IoU 0.995 (F.Cu) / 0.994 (B.Cu) after re-inflating by `min_thickness/2`.
//! Resolution: `max(25 µm, √(bbox area / 4·10⁶))` so a layer is at most ~4 M cells.
//!
//! **Connectivity** of a pour net = union-find over islands, pads, vias and tracks of the net: a
//! thermal pad joins the island its spoke test points land in (KiCad `buildThermalSpokes`: pad
//! bbox edge + thermal gap, four directions in the pad frame; a point is in the opened fill when
//! a fill cell lies within `min_thickness/2` of it), a solid pad the islands under its copper, a
//! via the island under its disc, a track the islands it runs over; tracks join the pads, vias
//! and tracks they touch (copper gap ≤ 0, as `decompose`). The **main component** of a net holds
//! the most pour-listed items (ties: largest fill area); every listed item and every pad routed in
//! this run that is not in it has **lost the pour**. Every other component that KiCad keeps
//! (it holds a listed or routed pad, a via, or an island the zone's island removal mode keeps)
//! is reported in [`PourReach::cuts`] (D4 pour stitching).
//!
//! Cost: O(cells + Σ item stamps + T·(P + V + T)) per net with `T` tracks of the net; rasters
//! are built in parallel per (pour, layer) and the result is independent of the thread count.
use crate::check::point_in_poly;
use crate::decompose::parse_pour_pad;
use crate::geom::{
    copper_gap, copper_gap_skeleton, dequantise, point_seg, Aabb, Prim, Seg, Skeleton, Track, Vec2,
};
use crate::model::{flip_track, via_layers, BoardModel, Item, KeptItem, PadAttr, Pt, Via};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

/// Smallest raster cell, nm.
pub const MIN_CELL_NM: f64 = 25_000.0;
/// Cell budget per (pour, layer) raster.
pub const CELL_BUDGET: f64 = 4.0e6;
/// Stamped-cell budget of one `check_pour_reach` run (all pours): once spent, further capsule
/// knockouts are skipped (the check turns optimistic and says so in `notes`).
pub const STAMP_CELL_BUDGET: u64 = 16_000_000;
/// KiCad `ZONE_CONNECTION` values.
pub const CONN_NONE: i32 = 0;
pub const CONN_THERMAL: i32 = 1;
pub const CONN_FULL: i32 = 2;
pub const CONN_THT_THERMAL: i32 = 3;

/// Model uuid of a router item (track or via) when it is carried over as fixed copper.
pub fn item_uuid(i: usize) -> String {
    format!("topor:{i}")
}

/// Raster of one pour's fill on one layer: `label[j·nx + i]` = island id (0 = no fill).
#[derive(Clone, Debug)]
pub struct Raster {
    x0: f64,
    y0: f64,
    res: f64,
    nx: usize,
    ny: usize,
    label: Vec<u32>,
    /// Fill area per island id (index 0 unused), nm².
    area: Vec<f64>,
    /// Island id → in the net's main component (index 0 unused).
    in_main: Vec<bool>,
    /// Per cell: distance from the cell centre to the edge of the fill after the knockouts and
    /// before the min-width opening, in 1/8 cells (0 outside the fill; capped at `u16::MAX`).
    depth: Vec<u16>,
}

impl Raster {
    fn cell(&self, p: Vec2) -> Option<usize> {
        let i = ((p.x - self.x0) / self.res).floor();
        let j = ((p.y - self.y0) / self.res).floor();
        if i < 0.0 || j < 0.0 || i >= self.nx as f64 || j >= self.ny as f64 {
            return None;
        }
        Some(j as usize * self.nx + i as usize)
    }

    fn centre(&self, i: usize, j: usize) -> Vec2 {
        Vec2 {
            x: self.x0 + (i as f64 + 0.5) * self.res,
            y: self.y0 + (j as f64 + 0.5) * self.res,
        }
    }

    /// Cell index ranges covering `bb` (clamped).
    fn range(&self, bb: &Aabb) -> (usize, usize, usize, usize) {
        let lo = |v: f64, o: f64, n: usize| (((v - o) / self.res).floor().max(0.0) as usize).min(n);
        let hi = |v: f64, o: f64, n: usize| (((v - o) / self.res).ceil().max(0.0) as usize).min(n);
        (
            lo(bb.min.x, self.x0, self.nx),
            hi(bb.max.x, self.x0, self.nx),
            lo(bb.min.y, self.y0, self.ny),
            hi(bb.max.y, self.y0, self.ny),
        )
    }

    /// Island ids (sorted, dedup) of the fill cells within `r` of `p`.
    fn labels_within(&self, p: Vec2, r: f64) -> Vec<u32> {
        let bb = Aabb { min: p, max: p }.grow(r);
        let (i0, i1, j0, j1) = self.range(&bb);
        let mut out = Vec::new();
        for j in j0..j1 {
            for i in i0..i1 {
                let l = self.label[j * self.nx + i];
                if l != 0 && (self.centre(i, j) - p).len() <= r {
                    out.push(l);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Island id of the cell containing `p` (0 outside the fill or the raster).
    fn label_at(&self, p: Vec2) -> u32 {
        self.cell(p).map_or(0, |c| self.label[c])
    }

    fn n_islands(&self) -> usize {
        self.area.len() - 1
    }

    /// Distance from `p` to the edge of the fill (knockouts in, before the min-width opening),
    /// nm; 0 outside the fill or the raster.
    pub fn depth_at(&self, p: Vec2) -> f64 {
        self.cell(p).map_or(0.0, |c| self.depth[c] as f64 * self.res / 8.0)
    }

    /// Island id of the cell containing `p` (0 outside the fill or the raster).
    pub fn island_at(&self, p: Vec2) -> u32 {
        self.label_at(p)
    }

    /// Cell size, nm.
    pub fn res(&self) -> f64 {
        self.res
    }

    /// Centres of the cells of island `l` whose fill depth is at least `min_depth`, on a lattice
    /// of every `step`-th cell (deterministic, row-major). O(cells).
    pub fn island_sites(&self, l: u32, min_depth: f64, step: usize) -> Vec<Vec2> {
        let step = step.max(1);
        let mut out = Vec::new();
        for j in (0..self.ny).step_by(step) {
            for i in (0..self.nx).step_by(step) {
                let c = j * self.nx + i;
                if self.label[c] == l && self.depth[c] as f64 * self.res / 8.0 >= min_depth {
                    out.push(self.centre(i, j));
                }
            }
        }
        out
    }
}

/// Stand-in for an infinite distance in [`edt_1d`] (F&H use a large finite value).
const EDT_BIG: f64 = 1e20;

/// 1-D squared Euclidean distance transform (Felzenszwalb & Huttenlocher 2012, "Distance
/// Transforms of Sampled Functions", Theory of Computing 8, p. 419, algorithm DT(f)):
/// `d[q] = min_p (q-p)^2 + f[p]`, lower envelope of parabolas. O(n).
fn edt_1d(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    let sect = |q: usize, p: usize| ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * q as f64 - 2.0 * p as f64);
    for q in 1..n {
        let mut s = sect(q, v[k]);
        while s <= z[k] {
            if k == 0 {
                break;
            }
            k -= 1;
            s = sect(q, v[k]);
        }
        if s <= z[k] {
            // k == 0 and the new parabola wins everywhere.
            v[0] = q;
            z[1] = f64::INFINITY;
            continue;
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    let mut k = 0usize;
    for (q, dq) in d.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        *dq = (q as f64 - p as f64).powi(2) + f[p];
    }
}

/// Squared distance (cells²) from every cell to the nearest cell where `fill` is false. O(n).
fn edt_2d(fill: &[bool], nx: usize, ny: usize) -> Vec<f64> {
    let mut g: Vec<f64> = fill.iter().map(|&f| if f { EDT_BIG } else { 0.0 }).collect();
    let n = nx.max(ny);
    let (mut f, mut d, mut v, mut z) = (vec![0.0; n], vec![0.0; n], vec![0usize; n], vec![0.0; n + 1]);
    for i in 0..nx {
        for j in 0..ny {
            f[j] = g[j * nx + i];
        }
        edt_1d(&f[..ny], &mut d[..ny], &mut v[..ny], &mut z[..ny + 1]);
        for j in 0..ny {
            g[j * nx + i] = d[j];
        }
    }
    for j in 0..ny {
        f[..nx].copy_from_slice(&g[j * nx..(j + 1) * nx]);
        edt_1d(&f[..nx], &mut d[..nx], &mut v[..nx], &mut z[..nx + 1]);
        g[j * nx..(j + 1) * nx].copy_from_slice(&d[..nx]);
    }
    g
}

/// Build the fill mask of `rings` (even-odd over outer rings and holes), knock out `stamps`
/// (capsules of radius `r`), open it by `erode` (KiCad deflates the fill by half the zone's
/// minimum width before it removes islands and inflates it again: features thinner than the
/// minimum width vanish, so the islands are those of the deflated fill; ZONE_FILLER::fillCopperZone),
/// label the 4-connected islands. The fill depth of every cell (distance to the knocked-out
/// fill's edge, from an exact Euclidean distance transform) is kept for via sites.
///
/// Knockouts run one scanline per raster row: a capsule meets a row in one interval, so a stamp
/// visits `O(rows spanned + cells covered)` cells (`O(L/res * w/res)` for a track of length `L` and
/// width `w`), not its whole bounding box. At most `allowance` cells are stamped and `stop` is
/// polled every 64 stamps; either cut leaves later stamps out. Returns the raster, the number of
/// cells stamped and whether the stamping was cut short. Total `O(nx*ny + stamped cells)`.
fn build_raster(
    rings: &[&[Vec2]],
    stamps: &[(Seg, f64)],
    allowance: u64,
    stop: &dyn Fn() -> bool,
    erode: f64,
) -> (Raster, u64, bool) {
    let mut bb: Option<Aabb> = None;
    for ring in rings {
        for p in ring.iter() {
            let b = Aabb { min: *p, max: *p };
            bb = Some(bb.map_or(b, |x| x.union(&b)));
        }
    }
    let bb = bb.unwrap_or(Aabb {
        min: Vec2 { x: 0.0, y: 0.0 },
        max: Vec2 { x: 0.0, y: 0.0 },
    });
    let (w, h) = (bb.max.x - bb.min.x, bb.max.y - bb.min.y);
    let res = (w * h / CELL_BUDGET).sqrt().max(MIN_CELL_NM);
    let nx = ((w / res).ceil() as usize + 3).max(1);
    let ny = ((h / res).ceil() as usize + 3).max(1);
    let x0 = bb.min.x - 1.5 * res;
    let y0 = bb.min.y - 1.5 * res;
    let mut r = Raster {
        x0,
        y0,
        res,
        nx,
        ny,
        label: vec![0; nx * ny],
        area: vec![0.0],
        in_main: vec![false],
        depth: Vec::new(),
    };
    // Even-odd scanline fill at the cell centres; edges are bucketed by the rows they span.
    let mut rows: Vec<Vec<f64>> = vec![Vec::new(); ny];
    for ring in rings {
        let n = ring.len();
        for k in 0..n {
            let (a, b) = (ring[k], ring[(k + 1) % n]);
            if a.y == b.y {
                continue;
            }
            let (ylo, yhi) = (a.y.min(b.y), b.y.max(a.y));
            let j0 = (((ylo - y0) / res - 0.5).ceil().max(0.0)) as usize;
            let j1 = ((((yhi - y0) / res - 0.5).floor()).max(-1.0) as isize).min(ny as isize - 1);
            let mut j = j0 as isize;
            while j <= j1 {
                let yc = y0 + (j as f64 + 0.5) * res;
                if (a.y > yc) != (b.y > yc) {
                    rows[j as usize].push(a.x + (yc - a.y) / (b.y - a.y) * (b.x - a.x));
                }
                j += 1;
            }
        }
    }
    let mut fill = vec![false; nx * ny];
    for (j, xs) in rows.iter_mut().enumerate() {
        xs.sort_by(f64::total_cmp);
        for pair in xs.chunks_exact(2) {
            let i0 = ((pair[0] - x0) / res - 0.5).ceil().max(0.0) as usize;
            let i1 = (((pair[1] - x0) / res - 0.5).floor().max(-1.0) as isize).min(nx as isize - 1);
            let mut i = i0 as isize;
            while i <= i1 {
                fill[j * nx + i as usize] = true;
                i += 1;
            }
        }
    }
    // Knockouts: one interval per capsule and row.
    let mut stamped = 0u64;
    let mut cut = false;
    'stamps: for (k, (s, rad)) in stamps.iter().enumerate() {
        if k % 64 == 0 && stop() {
            cut = true;
            break;
        }
        let sb = Prim::Seg(*s).bbox().grow(*rad);
        let (_, _, j0, j1) = r.range(&sb);
        for j in j0..j1 {
            let yc = y0 + (j as f64 + 0.5) * res;
            let Some((lo, hi)) = capsule_row_span(s, *rad, yc) else {
                continue;
            };
            let i0 = ((lo - x0) / res - 0.5).ceil().max(0.0) as usize;
            let i1 = (((hi - x0) / res - 0.5).floor().max(-1.0) as isize).min(nx as isize - 1);
            let mut i = i0 as isize;
            if i1 - i + 1 > 0 {
                let n = (i1 - i + 1) as u64;
                if stamped + n > allowance {
                    cut = true;
                    break 'stamps;
                }
                stamped += n;
            }
            while i <= i1 {
                fill[j * nx + i as usize] = false;
                i += 1;
            }
        }
    }
    // Fill depth and the min-width opening. A cell next to a knocked-out cell has its centre
    // half a cell inside the edge.
    let d2 = edt_2d(&fill, nx, ny);
    r.depth = d2
        .iter()
        .map(|&q| ((q.sqrt() - 0.5).max(0.0) * 8.0).min(u16::MAX as f64) as u16)
        .collect();
    if erode > 0.0 {
        for (c, f) in fill.iter_mut().enumerate() {
            if *f && (r.depth[c] as f64 / 8.0 + 0.25) * res < erode {
                *f = false;
            }
        }
    }
    // Islands.
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..nx * ny {
        if !fill[start] || r.label[start] != 0 {
            continue;
        }
        let id = r.area.len() as u32;
        r.area.push(0.0);
        r.in_main.push(false);
        r.label[start] = id;
        stack.push(start);
        let mut cells = 0usize;
        while let Some(c) = stack.pop() {
            cells += 1;
            let (i, j) = (c % nx, c / nx);
            let mut visit = |n: usize| {
                if fill[n] && r.label[n] == 0 {
                    r.label[n] = id;
                    stack.push(n);
                }
            };
            if i > 0 {
                visit(c - 1);
            }
            if i + 1 < nx {
                visit(c + 1);
            }
            if j > 0 {
                visit(c - nx);
            }
            if j + 1 < ny {
                visit(c + nx);
            }
        }
        r.area[id as usize] = cells as f64 * res * res;
    }
    (r, stamped, cut)
}

/// x-interval where the horizontal line `y = yc` meets the capsule (segment `s` grown by `rad`);
/// `None` when it misses. The capsule is convex, so the union of the two end discs and the
/// centre rectangle is one interval. O(1).
fn capsule_row_span(s: &Seg, rad: f64, yc: f64) -> Option<(f64, f64)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut add = |x: f64| {
        lo = lo.min(x);
        hi = hi.max(x);
    };
    for p in [s.a, s.b] {
        let dy = (yc - p.y).abs();
        if dy <= rad {
            let dx = (rad * rad - dy * dy).sqrt();
            add(p.x - dx);
            add(p.x + dx);
        }
    }
    let d = s.b - s.a;
    let len = d.len();
    if len > 0.0 {
        let n = Vec2 {
            x: -d.y / len * rad,
            y: d.x / len * rad,
        };
        let q = [s.a + n, s.b + n, s.b - n, s.a - n];
        for k in 0..4 {
            let (a, b) = (q[k], q[(k + 1) % 4]);
            if a.y == b.y {
                if a.y == yc {
                    add(a.x);
                    add(b.x);
                }
            } else if (a.y <= yc && yc <= b.y) || (b.y <= yc && yc <= a.y) {
                add(a.x + (yc - a.y) / (b.y - a.y) * (b.x - a.x));
            }
        }
    }
    (lo <= hi).then_some((lo, hi))
}

/// Capsule stamps (segment, radius) of a track primitive in the math frame, radius `r`.
fn prim_stamps(p: &Prim, r: f64, sag: f64) -> Vec<(Seg, f64)> {
    match p {
        Prim::Seg(s) => vec![(*s, r)],
        Prim::Arc(a) => a
            .split_sagitta(sag)
            .iter()
            .map(|sub| {
                (
                    Seg {
                        a: sub.start(),
                        b: sub.end(),
                    },
                    r + sub.sagitta(),
                )
            })
            .collect(),
    }
}

/// Points along a primitive at most `step` apart (both ends included).
fn prim_samples(p: &Prim, step: f64) -> Vec<Vec2> {
    let n = ((p.length() / step).ceil() as usize).max(1);
    (0..=n)
        .map(|k| {
            let f = k as f64 / n as f64;
            match p {
                Prim::Seg(s) => s.a + (s.b - s.a) * f,
                Prim::Arc(a) => a.point_at(f),
            }
        })
        .collect()
}

/// Half extent of a pad skeleton from the pad centre in direction `u` (support function).
fn support(sk: &Skeleton, c: Vec2, u: Vec2) -> f64 {
    match sk {
        Skeleton::Point { p, r } => (*p - c).dot(u) + r,
        Skeleton::Seg { s, r } => (s.a - c).dot(u).max((s.b - c).dot(u)) + r,
        Skeleton::Poly { pts, r } => {
            pts.iter()
                .map(|p| (*p - c).dot(u))
                .fold(f64::NEG_INFINITY, f64::max)
                + r
        }
    }
}

fn skeleton_contains(sk: &Skeleton, p: Vec2) -> bool {
    match sk {
        Skeleton::Point { p: c, r } => (p - *c).len() <= *r,
        Skeleton::Seg { s, r } => point_seg(p, s) <= *r,
        Skeleton::Poly { pts, r } => {
            point_in_poly(p, pts)
                || (0..pts.len()).any(|i| {
                    point_seg(
                        p,
                        &Seg {
                            a: pts[i],
                            b: pts[(i + 1) % pts.len()],
                        },
                    ) <= *r
                })
        }
    }
}

struct Dsu(Vec<usize>);

impl Dsu {
    fn new(n: usize) -> Self {
        Dsu((0..n).collect())
    }
    fn find(&mut self, mut a: usize) -> usize {
        while self.0[a] != a {
            self.0[a] = self.0[self.0[a]];
            a = self.0[a];
        }
        a
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb)] = ra.min(rb);
        }
    }
}

/// A track of the pour net (kept or routed), math frame.
struct NetTrack {
    layer: u32,
    t: Track,
}

/// A via of the pour net (model or routed), math frame.
struct NetVia {
    at: Vec2,
    r: f64,
    layers: Vec<u32>,
    /// Model uuid (model vias) or `item_uuid(i)` (routed).
    uuid: String,
}

/// Result of the pour-reach check.
#[derive(Clone, Debug, Default)]
pub struct PourReach {
    rasters: BTreeMap<(usize, u32), Raster>,
    /// Pad indices (sorted) that were pour-connected or routed and are not in their net's main
    /// component.
    pub lost_pads: Vec<usize>,
    /// Model via uuids (sorted) listed by a pour and not in the main component.
    pub lost_vias: Vec<String>,
    /// Per pour: the net's pads (indices) in the main component with copper on a pour layer.
    pub main_pads: BTreeMap<usize, Vec<usize>>,
    /// Per pour: via uuids (model and `item_uuid`) in the main component on a pour layer.
    pub main_vias: BTreeMap<usize, Vec<String>>,
    /// One line per checked net: islands, main component size, lost items.
    pub notes: Vec<String>,
    /// Raster cells knocked out by capsule stamps, all pours (diagnostic; bounded by
    /// [`STAMP_CELL_BUDGET`]).
    pub stamped_cells: u64,
    /// Copper of a pour net that KiCad keeps but that is not joined to the net's main
    /// component (each one is a DRC "missing connection"), by net, then smallest pad index.
    pub cuts: Vec<Cut>,
}

/// One component of a pour net's copper (islands, pads, vias, tracks) that is cut off from the
/// net's main component and that KiCad keeps: it holds a pour-listed or routed pad or a via,
/// or an island that the zone's island removal keeps (an island with a track or pad of the net,
/// or any island with removal "never", or one of at least the minimum area).
#[derive(Clone, Debug, Default)]
pub struct Cut {
    pub net: i32,
    /// Model pad indices in the component (sorted).
    pub pads: Vec<usize>,
    /// Centres (math frame) of the net's vias in the component.
    pub vias: Vec<Vec2>,
    /// Islands `(pour, layer, island id)` in the component.
    pub islands: Vec<(usize, u32, u32)>,
    /// Fill area of those islands, nm².
    pub area: f64,
}

impl PourReach {
    pub fn is_clean(&self) -> bool {
        self.lost_pads.is_empty() && self.lost_vias.is_empty()
    }

    /// Debug dump (TOPO_POURDUMP): one text header line per raster `pour layer x0 y0 res nx ny`
    /// (math frame, nm), then `nx*ny` little-endian u32 island labels, then one byte per island
    /// (1 = in the main component), per raster in key order.
    pub fn dump(&self, path: &str) {
        let mut buf: Vec<u8> = Vec::new();
        for ((zi, l), r) in &self.rasters {
            buf.extend(format!("{zi} {l} {} {} {} {} {} {}\n", r.x0, r.y0, r.res, r.nx, r.ny, r.n_islands()).as_bytes());
            for v in &r.label {
                buf.extend(v.to_le_bytes());
            }
            for &m in &r.in_main[1..] {
                buf.push(u8::from(m));
            }
        }
        let _ = std::fs::write(path, buf);
    }

    /// The raster of pour `pour` on `layer`, when there is one.
    pub fn raster(&self, pour: usize, layer: u32) -> Option<&Raster> {
        self.rasters.get(&(pour, layer))
    }

    /// Whether island `l` of the raster of `pour` on `layer` is in its net's main component.
    pub fn island_in_main(&self, pour: usize, layer: u32, l: u32) -> bool {
        self.rasters.get(&(pour, layer)).is_some_and(|r| l != 0 && (l as usize) < r.in_main.len() && r.in_main[l as usize])
    }

    /// Whether `p` (math frame) lies in the main component of pour `pour` on `layer`; pours and
    /// layers without a raster count as in (no information: the pre-route fill applies).
    pub fn in_main(&self, pour: usize, layer: u32, p: Vec2) -> bool {
        match self.rasters.get(&(pour, layer)) {
            None => true,
            Some(r) => {
                let l = r.label_at(p);
                l != 0 && r.in_main[l as usize]
            }
        }
    }

    /// Whether a via site at `at` keeps pour `pour` as a goal: inside the main component on at
    /// least one of the pour's rastered routable layers (true when the pour has no raster).
    pub fn site_in_main(&self, pour: usize, routable: &[u32], at: Vec2) -> bool {
        let keys: Vec<&(usize, u32)> = self
            .rasters
            .keys()
            .filter(|(p, l)| *p == pour && routable.contains(l))
            .collect();
        keys.is_empty() || keys.iter().any(|(p, l)| self.in_main(*p, *l, at))
    }
}

/// Every pad a pour label `REF.NUM` names (a footprint can have several pads of one number, e.g.
/// a push button's two pads "2"; the pour list cannot tell them apart, so all of them count).
fn pad_indices(model: &BoardModel, label: &str) -> Vec<usize> {
    let Some(r) = parse_pour_pad(label) else { return Vec::new() };
    (0..model.pads.len()).filter(|&i| model.pads[i].ref_ == r.ref_ && model.pads[i].number == r.number).collect()
}

/// Check every pour net of `model` against the router `items` (KiCad frame, as emitted by
/// `route`). `routed_pads` = pad indices at the ends of connections routed in this run (their
/// copper must reach the main component too).
pub fn check_pour_reach(model: &BoardModel, items: &[Item], routed_pads: &[usize]) -> PourReach {
    check_pour_reach_until(model, items, routed_pads, None)
}

/// [`check_pour_reach`] with a deadline (and the cancel flag): the capsule stamping of every pour
/// polls it, and once it fires (or [`STAMP_CELL_BUDGET`] is spent, shared deterministically: each
/// net's rasters split the budget left at the net's start equally) the remaining knockouts are
/// skipped and a note says so. O(nets * pours * (cells + stamped cells)).
pub fn check_pour_reach_until(
    model: &BoardModel,
    items: &[Item],
    routed_pads: &[usize],
    deadline: Option<std::time::Instant>,
) -> PourReach {
    let stop = move || deadline.map_or_else(crate::control::cancelled, crate::control::expired);
    let mut budget_left = STAMP_CELL_BUDGET;
    let mut out = PourReach::default();
    let all = model.all_layer_ids();
    let rules = &model.rules;
    let clr = |net: i32| rules.net_rule(net).clearance.max(rules.min_clearance) as f64;

    // Router items in the math frame.
    let tracks: Vec<(usize, u32, i32, Track)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| match it {
            Item::Track(t) => Some((
                i,
                t.layer,
                t.net,
                Track {
                    prim: dequantise(&flip_track(t.prim)),
                    w: t.width as f64,
                },
            )),
            _ => None,
        })
        .collect();
    let vias: Vec<(usize, i32, Vec2, f64, Vec<u32>)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| match it {
            Item::Via(v) => {
                let mut at = v.at;
                at.flip();
                let layers = if v.layers.is_empty() {
                    all.clone()
                } else {
                    via_layers(&v.layers, &all)
                };
                Some((i, v.net, at.to_vec2(), 0.5 * v.d as f64, layers))
            }
            _ => None,
        })
        .collect();

    let mut by_net: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for (zi, z) in model.pours.iter().enumerate() {
        if z.net != 0 && z.fills.iter().any(|f| f.outer.len() >= 3) {
            by_net.entry(z.net).or_default().push(zi);
        }
    }

    for (&net, pours) in &by_net {
        // Rasters per (pour, layer), in parallel.
        let keys: Vec<(usize, u32)> = pours
            .iter()
            .flat_map(|&zi| {
                model.pours[zi]
                    .fills
                    .iter()
                    .filter(|f| f.outer.len() >= 3)
                    .map(move |f| (zi, f.layer))
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let allowance = budget_left / keys.len().max(1) as u64;
        let built: Vec<((usize, u32), Raster, u64, bool)> = keys
            .par_iter()
            .map(|&(zi, layer)| {
                let z = &model.pours[zi];
                let rings_owned: Vec<Vec<Vec2>> = z
                    .fills
                    .iter()
                    .filter(|f| f.layer == layer && f.outer.len() >= 3)
                    .flat_map(|f| {
                        std::iter::once(&f.outer)
                            .chain(f.holes.iter())
                            .filter(|r| r.len() >= 3)
                            .map(|r| r.iter().map(|p| p.to_vec2()).collect::<Vec<Vec2>>())
                    })
                    .collect();
                let rings: Vec<&[Vec2]> = rings_owned.iter().map(|r| r.as_slice()).collect();
                // Cell size as `build_raster` derives it, for the conservative half-diagonal.
                let probe = build_cell(&rings);
                let half_min = 0.5 * z.min_thickness as f64;
                // The knockout by the clearance (a quarter cell conservative); the min-width
                // opening is the erosion in `build_raster`.
                let dil = |inet: i32| (z.clearance as f64).max(clr(net)).max(clr(inet)) + probe * 0.25;
                let mut stamps: Vec<(Seg, f64)> = Vec::new();
                for (_, _, inet, t) in tracks
                    .iter()
                    .filter(|(_, l, n, _)| *l == layer && *n != net)
                {
                    stamps.extend(prim_stamps(&t.prim, 0.5 * t.w + dil(*inet), probe * 0.25));
                }
                for (_, inet, at, r, _) in vias
                    .iter()
                    .filter(|(_, n, _, _, ls)| *n != net && ls.contains(&layer))
                {
                    stamps.push((Seg { a: *at, b: *at }, r + dil(*inet)));
                }
                let (raster, stamped, cut) = build_raster(&rings, &stamps, allowance, &stop, half_min);
                ((zi, layer), raster, stamped, cut)
            })
            .collect();
        for (k, _, stamped, cut) in &built {
            out.stamped_cells += stamped;
            budget_left = budget_left.saturating_sub(*stamped);
            if *cut {
                out.notes.push(format!(
                    "pour {} layer {}: knockout stamping cut short (cell budget or deadline); \
                     reach check optimistic",
                    k.0, k.1
                ));
            }
        }
        let mut rasters: BTreeMap<(usize, u32), Raster> =
            built.into_iter().map(|(k, r, _, _)| (k, r)).collect();

        // Nodes: islands, pads, vias, tracks of the net.
        let mut island_base: BTreeMap<(usize, u32), usize> = BTreeMap::new();
        let mut n = 0usize;
        for (k, r) in &rasters {
            island_base.insert(*k, n);
            n += r.n_islands() + 1;
        }
        let pads: Vec<usize> = (0..model.pads.len())
            .filter(|&i| model.pads[i].net == net)
            .collect();
        let pad_base = n;
        n += pads.len();
        let mut nvias: Vec<NetVia> = model
            .vias
            .iter()
            .filter(|v| v.net == net)
            .map(|v| NetVia {
                at: v.at.to_vec2(),
                r: 0.5 * v.d as f64,
                layers: via_layers(&v.layers, &all),
                uuid: v.uuid.clone(),
            })
            .collect();
        nvias.extend(vias.iter().filter(|v| v.1 == net).map(|v| NetVia {
            at: v.2,
            r: v.3,
            layers: v.4.clone(),
            uuid: item_uuid(v.0),
        }));
        let via_base = n;
        n += nvias.len();
        let mut ntracks: Vec<NetTrack> = model
            .kept
            .iter()
            .filter(|k| k.net == net)
            .map(|k| NetTrack {
                layer: k.layer,
                t: Track {
                    prim: dequantise(&k.prim),
                    w: k.width as f64,
                },
            })
            .collect();
        ntracks.extend(tracks.iter().filter(|t| t.2 == net).map(|t| NetTrack {
            layer: t.1,
            t: Track {
                prim: t.3.prim,
                w: t.3.w,
            },
        }));
        let track_base = n;
        n += ntracks.len();
        let mut dsu = Dsu::new(n);
        let island_node = |k: (usize, u32), l: u32| island_base[&k] + l as usize;

        // Pads ↔ islands.
        for (a, &pi) in pads.iter().enumerate() {
            let pad = &model.pads[pi];
            let c = pad.center.to_vec2();
            for (&(zi, layer), r) in &rasters {
                if !pad.layers.contains(&layer) {
                    continue;
                }
                let z = &model.pours[zi];
                let mode = match pad.zone_connection.unwrap_or(z.pad_connection) {
                    CONN_THT_THERMAL if pad.attr == PadAttr::Pth => CONN_THERMAL,
                    CONN_THT_THERMAL => CONN_FULL,
                    m => m,
                };
                let sk = model.skeleton(pad, layer);
                let half_min = 0.5 * z.min_thickness as f64;
                let tol = half_min + r.res * 0.71;
                let labels: Vec<u32> = match mode {
                    CONN_FULL => {
                        let bb = skeleton_bbox(&sk).grow(r.res);
                        let (i0, i1, j0, j1) = r.range(&bb);
                        let mut v = Vec::new();
                        for j in j0..j1 {
                            for i in i0..i1 {
                                let l = r.label[j * r.nx + i];
                                if l != 0 && skeleton_contains(&sk, r.centre(i, j)) {
                                    v.push(l);
                                }
                            }
                        }
                        v.sort_unstable();
                        v.dedup();
                        v
                    }
                    CONN_THERMAL => {
                        let gap = pad.thermal_gap.unwrap_or(z.thermal_gap) as f64;
                        let base = pad.rotation_deg.to_radians()
                            + (pad.spoke_angle_deg.unwrap_or(90.0) - 90.0).to_radians();
                        let mut v = Vec::new();
                        for k in 0..4 {
                            let th = base + std::f64::consts::FRAC_PI_2 * k as f64;
                            let u = Vec2 {
                                x: th.cos(),
                                y: th.sin(),
                            };
                            let d = support(&sk, c, u) + gap + 1_000.0;
                            v.extend(r.labels_within(c + u * d, tol));
                        }
                        v.sort_unstable();
                        v.dedup();
                        v
                    }
                    _ => Vec::new(),
                };
                for l in labels {
                    dsu.union(pad_base + a, island_node((zi, layer), l));
                }
            }
        }
        // Vias ↔ islands.
        for (b, v) in nvias.iter().enumerate() {
            for (&(zi, layer), r) in &rasters {
                if !v.layers.contains(&layer) {
                    continue;
                }
                for l in r.labels_within(v.at, v.r) {
                    dsu.union(via_base + b, island_node((zi, layer), l));
                }
            }
        }
        // Tracks ↔ islands, pads, vias, tracks.
        let pad_sk: Vec<Vec<(u32, Skeleton)>> = pads
            .iter()
            .map(|&pi| {
                let pad = &model.pads[pi];
                pad.layers
                    .iter()
                    .map(|&l| (l, model.skeleton(pad, l)))
                    .collect()
            })
            .collect();
        for (k, t) in ntracks.iter().enumerate() {
            let node = track_base + k;
            let tb = t.t.prim.bbox().grow(0.5 * t.t.w);
            for (&(zi, layer), r) in &rasters {
                if layer != t.layer {
                    continue;
                }
                let mut ls: Vec<u32> = prim_samples(&t.t.prim, 0.5 * r.res)
                    .into_iter()
                    .map(|p| r.label_at(p))
                    .filter(|&l| l != 0)
                    .collect();
                ls.sort_unstable();
                ls.dedup();
                for l in ls {
                    dsu.union(node, island_node((zi, layer), l));
                }
            }
            for (a, sks) in pad_sk.iter().enumerate() {
                for (l, sk) in sks {
                    if *l == t.layer
                        && skeleton_bbox(sk).grow(1.0).overlaps(&tb)
                        && copper_gap_skeleton(&t.t, sk) <= 0.0
                    {
                        dsu.union(node, pad_base + a);
                    }
                }
            }
            for (b, v) in nvias.iter().enumerate() {
                if v.layers.contains(&t.layer)
                    && copper_gap_skeleton(&t.t, &Skeleton::Point { p: v.at, r: v.r }) <= 0.0
                {
                    dsu.union(node, via_base + b);
                }
            }
            for (k2, t2) in ntracks.iter().enumerate().skip(k + 1) {
                if t2.layer == t.layer
                    && t2.t.prim.bbox().grow(0.5 * t2.t.w).overlaps(&tb)
                    && copper_gap(&t.t, &t2.t) <= 0.0
                {
                    dsu.union(node, track_base + k2);
                }
            }
        }

        // Listed items and the main component.
        let mut listed_pads: BTreeSet<usize> = BTreeSet::new(); // local pad index
        let mut listed_vias: BTreeSet<usize> = BTreeSet::new(); // nvias index
        for &zi in pours {
            let z = &model.pours[zi];
            for s in &z.pads {
                for pi in pad_indices(model, s) {
                    if let Some(a) = pads.iter().position(|&x| x == pi) {
                        if model.pads[pi].layers.iter().any(|l| z.layers.contains(l)) {
                            listed_pads.insert(a);
                        }
                    }
                }
            }
            for u in &z.vias {
                if let Some(b) = nvias.iter().position(|v| &v.uuid == u) {
                    if nvias[b].layers.iter().any(|l| z.layers.contains(l)) {
                        listed_vias.insert(b);
                    }
                }
            }
        }
        let mut score: BTreeMap<usize, (usize, f64)> = BTreeMap::new();
        for &a in &listed_pads {
            score.entry(dsu.find(pad_base + a)).or_default().0 += 1;
        }
        for &b in &listed_vias {
            score.entry(dsu.find(via_base + b)).or_default().0 += 1;
        }
        for (k, r) in &rasters {
            for l in 1..=r.n_islands() {
                score
                    .entry(dsu.find(island_node(*k, l as u32)))
                    .or_default()
                    .1 += r.area[l];
            }
        }
        let main = score
            .iter()
            .max_by(|(ra, (ca, aa)), (rb, (cb, ab))| {
                ca.cmp(cb).then(aa.total_cmp(ab)).then(rb.cmp(ra))
            })
            .map(|(r, _)| *r);
        let Some(main) = main else { continue };
        let in_main = |dsu: &mut Dsu, node: usize| dsu.find(node) == main;
        for (k, r) in rasters.iter_mut() {
            for l in 1..=r.n_islands() {
                r.in_main[l] = in_main(&mut dsu, island_base[k] + l);
            }
        }
        let mut lost_here: Vec<usize> = Vec::new();
        for &a in &listed_pads {
            if !in_main(&mut dsu, pad_base + a) {
                lost_here.push(pads[a]);
            }
        }
        for &pi in routed_pads.iter().filter(|&&pi| model.pads[pi].net == net) {
            if let Some(a) = pads.iter().position(|&x| x == pi) {
                if !in_main(&mut dsu, pad_base + a) {
                    lost_here.push(pi);
                }
            }
        }
        lost_here.sort_unstable();
        lost_here.dedup();
        let mut lost_vias_here: Vec<String> = listed_vias
            .iter()
            .filter(|&&b| !in_main(&mut dsu, via_base + b))
            .map(|&b| nvias[b].uuid.clone())
            .collect();
        lost_vias_here.sort();
        for &zi in pours {
            let z = &model.pours[zi];
            let mp: Vec<usize> = pads
                .iter()
                .enumerate()
                .filter(|(a, &pi)| {
                    model.pads[pi].layers.iter().any(|l| z.layers.contains(l))
                        && in_main(&mut dsu, pad_base + a)
                })
                .map(|(_, &pi)| pi)
                .collect();
            let mut mv: Vec<String> = nvias
                .iter()
                .enumerate()
                .filter(|(b, v)| {
                    v.layers.iter().any(|l| z.layers.contains(l)) && in_main(&mut dsu, via_base + b)
                })
                .map(|(_, v)| v.uuid.clone())
                .collect();
            mv.sort();
            out.main_pads.insert(zi, mp);
            out.main_vias.insert(zi, mv);
        }
        // Cut-off components KiCad keeps.
        {
            let mut comp: BTreeMap<usize, Cut> = BTreeMap::new();
            let mut must: BTreeSet<usize> = BTreeSet::new();
            let mut items: BTreeSet<usize> = BTreeSet::new();
            for (a, &pi) in pads.iter().enumerate() {
                let root = dsu.find(pad_base + a);
                items.insert(root);
                comp.entry(root).or_default().pads.push(pi);
                if listed_pads.contains(&a) || routed_pads.contains(&pi) {
                    must.insert(root);
                }
            }
            for (b, v) in nvias.iter().enumerate() {
                let root = dsu.find(via_base + b);
                items.insert(root);
                must.insert(root);
                comp.entry(root).or_default().vias.push(v.at);
            }
            for k in 0..ntracks.len() {
                items.insert(dsu.find(track_base + k));
            }
            for (k, r) in &rasters {
                let z = &model.pours[k.0];
                for l in 1..=r.n_islands() {
                    let root = dsu.find(island_base[k] + l);
                    let c = comp.entry(root).or_default();
                    c.islands.push((k.0, k.1, l as u32));
                    c.area += r.area[l];
                    let kept = match z.island_removal {
                        1 => true,
                        2 => r.area[l] >= z.min_island_area,
                        _ => false,
                    };
                    if kept {
                        must.insert(root);
                    }
                }
            }
            for (root, mut c) in comp {
                let keep = must.contains(&root) || (items.contains(&root) && !c.islands.is_empty());
                if root == main || !keep {
                    continue;
                }
                c.net = net;
                c.pads.sort_unstable();
                out.cuts.push(c);
            }
        }
        let islands: usize = rasters.values().map(|r| r.n_islands()).sum();
        let name = model
            .nets
            .iter()
            .find(|n| n.code == net)
            .map(|n| n.name.clone())
            .unwrap_or_default();
        let lost_names: Vec<String> = lost_here
            .iter()
            .map(|&pi| format!("{}.{}", model.pads[pi].ref_, model.pads[pi].number))
            .chain(lost_vias_here.iter().map(|u| format!("via {u}")))
            .collect();
        out.notes.push(format!(
            "pour reach net {name}: {islands} island(s) on {} raster(s), {} listed item(s), \
             main component {} of {}, lost: [{}]",
            rasters.len(),
            listed_pads.len() + listed_vias.len(),
            score[&main].0,
            listed_pads.len() + listed_vias.len(),
            lost_names.join(", ")
        ));
        out.lost_pads.extend(lost_here);
        out.lost_vias.extend(lost_vias_here);
        out.rasters.extend(rasters);
    }
    out.lost_pads.sort_unstable();
    out.lost_pads.dedup();
    out.lost_vias.sort();
    out.lost_vias.dedup();
    out
}

/// Cell size `build_raster` picks for these rings.
fn build_cell(rings: &[&[Vec2]]) -> f64 {
    let mut bb: Option<Aabb> = None;
    for ring in rings {
        for p in ring.iter() {
            let b = Aabb { min: *p, max: *p };
            bb = Some(bb.map_or(b, |x| x.union(&b)));
        }
    }
    bb.map_or(MIN_CELL_NM, |b| {
        ((b.max.x - b.min.x) * (b.max.y - b.min.y) / CELL_BUDGET)
            .sqrt()
            .max(MIN_CELL_NM)
    })
}

fn skeleton_bbox(sk: &Skeleton) -> Aabb {
    match sk {
        Skeleton::Point { p, r } => Aabb { min: *p, max: *p }.grow(*r),
        Skeleton::Seg { s, r } => Prim::Seg(*s).bbox().grow(*r),
        Skeleton::Poly { pts, r } => {
            let mut bb = Aabb {
                min: pts[0],
                max: pts[0],
            };
            for p in pts {
                bb = bb.union(&Aabb { min: *p, max: *p });
            }
            bb.grow(*r)
        }
    }
}

/// The model for a pour-completion pass: `items` (KiCad frame) become fixed copper (kept tracks
/// and vias with uuids `item_uuid(i)`), and every pour lists exactly the pads and vias of its
/// net that are in the main component — so `decompose` yields one pour-completion connection
/// per copper cluster that lost the pour.
pub fn with_routed_copper(model: &BoardModel, items: &[Item], reach: &PourReach) -> BoardModel {
    let mut m = model.clone();
    let all = model.all_layer_ids();
    for (i, it) in items.iter().enumerate() {
        match it {
            Item::Track(t) => m.kept.push(KeptItem {
                uuid: item_uuid(i),
                net: t.net,
                layer: t.layer,
                width: t.width,
                prim: flip_track(t.prim),
                locked: false,
            }),
            Item::Via(v) => {
                let mut at: Pt = v.at;
                at.flip();
                m.vias.push(Via {
                    uuid: item_uuid(i),
                    net: v.net,
                    at,
                    d: v.d,
                    drill: v.drill,
                    layers: if v.layers.is_empty() {
                        all.clone()
                    } else {
                        v.layers.clone()
                    },
                    free: false,
                    locked: false,
                });
            }
        }
    }
    for (zi, z) in m.pours.iter_mut().enumerate() {
        if let Some(mp) = reach.main_pads.get(&zi) {
            z.pads = mp
                .iter()
                .map(|&pi| format!("{}.{}", model.pads[pi].ref_, model.pads[pi].number))
                .collect();
            z.pads.sort();
        }
        if let Some(mv) = reach.main_vias.get(&zi) {
            z.vias = mv.clone();
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::quant::{IVec2, TrackPrim};
    use crate::model::{Options, TrackItem, ViaItem};

    const MM: i64 = 1_000_000;

    fn pad(r: &str, net: i32, x: i64, y: i64, layers: &str) -> String {
        let shapes: Vec<String> = layers
            .split(',')
            .map(|l| format!(r#""{}": {{ "kind": "circle", "d": 1000000 }}"#, l.trim()))
            .collect();
        format!(
            r#"{{ "uuid": "{r}", "ref": "{r}", "number": "1", "net": {net}, "attr": "pth",
              "center": [{x}, {y}], "layers": [{layers}], "shapes": {{ {} }} }}"#,
            shapes.join(",")
        )
    }

    /// 50 x 40 mm board; net 1 pour on B.Cu (31): [2, 32] x [2, 32] mm, listed U1.1 (4, 15)
    /// and U3.1 (20, 15), both THT; net 2 pads A (7, 2.3) and B (7, 31.7) on B.Cu.
    fn model() -> BoardModel {
        let pads = [
            pad("U1", 1, 4 * MM, 15 * MM, "0, 31"),
            pad("U3", 1, 20 * MM, 15 * MM, "0, 31"),
            pad("A", 2, 7 * MM, 2_300_000, "31"),
            pad("B", 2, 7 * MM, 31_700_000, "31"),
        ];
        let s = format!(
            r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
              "nets": [ {{ "code": 1, "name": "GND" }}, {{ "code": 2, "name": "SIG" }} ],
              "outline": [ {{ "kind": "seg", "a": [0, 0], "b": [50000000, 0] }},
                {{ "kind": "seg", "a": [50000000, 0], "b": [50000000, 40000000] }},
                {{ "kind": "seg", "a": [50000000, 40000000], "b": [0, 40000000] }},
                {{ "kind": "seg", "a": [0, 40000000], "b": [0, 0] }} ],
              "pads": [ {} ],
              "pours": [ {{ "uuid": "zz", "net": 1, "layers": [31], "pads": ["U1.1", "U3.1"],
                "vias": [], "clearance": 200000, "min_thickness": 254000, "thermal_gap": 500000,
                "spoke_width": 500000, "pad_connection": 1,
                "fills": [ {{ "layer": 31, "outer": [[2000000, 2000000], [32000000, 2000000],
                  [32000000, 32000000], [2000000, 32000000]] }} ] }} ],
              "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0 }} }}"#,
            pads.join(",")
        );
        BoardModel::from_json(&s).unwrap()
    }

    fn seg(net: i32, layer: u32, a: (i64, i64), b: (i64, i64)) -> Item {
        Item::Track(TrackItem {
            net,
            layer,
            width: 250_000,
            prim: TrackPrim::Seg {
                a: IVec2 {
                    x: a.0 as i32,
                    y: a.1 as i32,
                },
                b: IVec2 {
                    x: b.0 as i32,
                    y: b.1 as i32,
                },
            },
            group: None,
            wire: None,
        })
    }

    fn via(net: i32, at: (i64, i64)) -> Item {
        Item::Via(ViaItem {
            kind: "via".into(),
            net,
            at: Pt { x: at.0, y: at.1 },
            d: 600_000,
            drill: 300_000,
            layers: vec![],
            group: None,
            wire: None,
        })
    }

    #[test]
    fn untouched_fill_keeps_every_listed_pad() {
        let m = model();
        let r = check_pour_reach(&m, &[], &[]);
        assert!(r.is_clean(), "{:?}", r.notes);
        assert_eq!(r.main_pads[&0], vec![0, 1]);
        assert!(r.in_main(
            0,
            31,
            Vec2 {
                x: 10.0e6,
                y: -10.0e6
            }
        ));
        assert!(!r.in_main(
            0,
            31,
            Vec2 {
                x: 40.0e6,
                y: -10.0e6
            }
        ));
    }

    #[test]
    fn pourcheck_long_diagonal_track_stamps_linear() {
        let m = model();
        // 41.6 mm diagonal of the 30 x 30 mm fill: the bounding box is the whole fill.
        let items = [seg(2, 31, (2_300_000, 2_300_000), (31_700_000, 31_700_000))];
        let r = check_pour_reach(&m, &items, &[]);
        let rings: Vec<Vec<Vec2>> = vec![m.pours[0].fills[0]
            .outer
            .iter()
            .map(|p| p.to_vec2())
            .collect()];
        let refs: Vec<&[Vec2]> = rings.iter().map(|v| v.as_slice()).collect();
        let res = build_cell(&refs);
        let l = 29.4e6f64 * std::f64::consts::SQRT_2;
        let w = 2.0 * (125_000.0 + 200_000.0 + 127_000.0 + 0.71 * res);
        let bound = 4.0 * (l / res) * (w / res) + 64.0;
        assert!(r.stamped_cells > 0);
        assert!(
            (r.stamped_cells as f64) <= bound,
            "stamped {} > bound {bound}",
            r.stamped_cells
        );
        // the knockout still separates U1 (above the diagonal) from U3 (below)
        assert!(!r.is_clean(), "{:?}", r.notes);
    }

    #[test]
    fn stamp_budget_exhaustion_is_noted_not_fatal() {
        let m = model();
        let items = [seg(2, 31, (2_300_000, 2_300_000), (31_700_000, 31_700_000))];
        let r = check_pour_reach_until(&m, &items, &[], Some(std::time::Instant::now()));
        assert!(
            r.notes.iter().any(|n| n.contains("cut short")),
            "{:?}",
            r.notes
        );
        assert_eq!(r.stamped_cells, 0);
    }

    #[test]
    fn foreign_track_cutting_the_fill_loses_the_pad_on_the_small_side() {
        let m = model();
        let items = [seg(2, 31, (7 * MM, 2_300_000), (7 * MM, 31_700_000))];
        let r = check_pour_reach(&m, &items, &[]);
        assert_eq!(r.lost_pads, vec![0], "{:?}", r.notes);
        assert!(r.lost_vias.is_empty());
        assert_eq!(r.main_pads[&0], vec![1]);
        // The site filter keeps the main side only.
        assert!(r.site_in_main(
            0,
            &[0, 31],
            Vec2 {
                x: 20.0e6,
                y: -15.0e6
            }
        ));
        assert!(!r.site_in_main(
            0,
            &[0, 31],
            Vec2 {
                x: 4.0e6,
                y: -10.0e6
            }
        ));
        // The pass model lists only the main pads and carries the track as kept copper.
        let m2 = with_routed_copper(&m, &items, &r);
        assert_eq!(m2.pours[0].pads, vec!["U3.1".to_string()]);
        assert_eq!(m2.kept.len(), 1);
        assert_eq!(m2.kept[0].uuid, item_uuid(0));
        assert_eq!(m2.kept[0].prim.start().y, -2_300_000);
        let d = crate::decompose::decompose(
            &m2,
            &Options {
                scope: crate::model::Scope {
                    pads: vec![crate::model::PadRef::try_from("U1.1".to_string()).unwrap()],
                    ..Default::default()
                },
                ..Options::default()
            },
            &[0, 31],
        );
        assert_eq!(d.connections.len(), 1, "{:?}", d.connections);
        assert!(matches!(
            d.connections[0].to.kind,
            crate::decompose::TermKind::Pour(0)
        ));
    }

    #[test]
    fn same_net_track_and_via_bridge_the_cut_pad() {
        let m = model();
        let items = [
            seg(2, 31, (7 * MM, 2_300_000), (7 * MM, 31_700_000)),
            seg(1, 0, (4 * MM, 15 * MM), (12 * MM, 15 * MM)),
            via(1, (12 * MM, 15 * MM)),
        ];
        let r = check_pour_reach(&m, &items, &[0]);
        assert!(r.is_clean(), "{:?}", r.notes);
        assert_eq!(r.main_vias[&0], vec![item_uuid(2)]);
        let m2 = with_routed_copper(&m, &items, &r);
        assert_eq!(m2.pours[0].vias, vec![item_uuid(2)]);
        assert_eq!(m2.vias.len(), 1);
        assert_eq!(m2.vias[0].layers, vec![0, 31]);
    }

    #[test]
    fn routed_pad_whose_via_lands_in_a_cut_island_is_lost() {
        let m = model();
        // U1.1 "completed" by a via inside the small island: still cut from the main body.
        let items = [
            seg(2, 31, (7 * MM, 2_300_000), (7 * MM, 31_700_000)),
            seg(1, 0, (4 * MM, 15 * MM), (4 * MM, 20 * MM)),
            via(1, (4 * MM, 20 * MM)),
        ];
        let r = check_pour_reach(&m, &items, &[0]);
        assert_eq!(r.lost_pads, vec![0], "{:?}", r.notes);
        assert!(r.main_vias[&0].is_empty());
    }

    #[test]
    fn spokes_blocked_on_all_four_sides_lose_the_pad() {
        // Foreign tracks box U3.1 in on B.Cu at 0.9 mm from its centre: every spoke test point
        // (0.5 + 0.5 gap + eps from the centre) lies inside a knockout.
        let m = model();
        let c = (20 * MM, 15 * MM);
        let d = 900_000;
        let items = [
            seg(2, 31, (c.0 - 3 * MM, c.1 - d), (c.0 + 3 * MM, c.1 - d)),
            seg(2, 31, (c.0 - 3 * MM, c.1 + d), (c.0 + 3 * MM, c.1 + d)),
            seg(2, 31, (c.0 - d, c.1 - 3 * MM), (c.0 - d, c.1 + 3 * MM)),
            seg(2, 31, (c.0 + d, c.1 - 3 * MM), (c.0 + d, c.1 + 3 * MM)),
        ];
        let r = check_pour_reach(&m, &items, &[]);
        assert_eq!(r.lost_pads, vec![1], "{:?}", r.notes);
    }
}
