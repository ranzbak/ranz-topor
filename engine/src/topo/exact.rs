//! Exact local repair of a drawn run (plan S2c item 2, the TopoR "taut string through circle
//! primitives"): where the funnel polyline of [`super::draw`] cuts an obstacle's clearance, the
//! violating stretch is replaced by the shortest path *inside the wire's sleeve* (the triangle
//! strip of its topology, whose non-portal sides are walls) that keeps the true clearance to
//! every pad polygon, hole, board edge, keepout and every copper item drawn so far.
//!
//! The shortest path in a polygonal domain with offset obstacles is a chain of tangent segments
//! and arcs around the obstacles' offset corners. The arcs are flattened onto *circumscribed*
//! sample points (radius `need / cos(step / 2)`, 22.5 degree steps), so a chord between two
//! samples stays outside the circle of radius `need`; the chain is found by A* over a lazily
//! built visibility graph whose nodes are those samples (convex corners: the arc between the
//! two adjacent edge normals; reflex corners: the offset-line intersection; segment ends and
//! vias: a full circle). Nested radii are not a separate construct here: a wire drawn earlier is
//! an obstacle whose corners get their own circles, so the next wire wraps it at that wire's
//! radius plus its own demand, which is the cumulative-demand radius of the plan.
//!
//! Only the stretch around a violation is searched (window grown 2 / 6 / 20 mm), so the cost is
//! bounded by the local node count (capped) and does not grow with the wire's length.
//!
//! Complexity: per window O(nodes x expanded x (obstacles + walls near the segment)); the node
//! count is capped at [`MAX_NODES`]. Deterministic (ids and `total_cmp` orders only).

use super::draw::{bbox_of, seg_core, Core, Drawer};
use super::state::{TriId, WireId};
use crate::geom::prim::Aabb;
use crate::geom::Vec2;
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// Most graph nodes of one window; above it the search gives up (the caller falls back).
const MAX_NODES: usize = 1200;
/// Angular step of the arc flattening (rad).
const STEP: f64 = std::f64::consts::PI / 8.0;
/// Tolerance on a clearance test (nm).
const TOL: f64 = 0.5;

/// forced-draw: the length of the segment `a-b` that lies closer than `need` to `core` (the
/// stretch a violation of that object runs along, [`Drawer::seg_deficit_near`]). The distance
/// along a segment to a convex core is convex, so that stretch is one interval: it holds the
/// segment's point closest to the core ([`seg_core`]), and each of its ends is found by bisection
/// towards the segment end (30 steps, below 1e-9 of the length). A non-convex core (kept copper)
/// gives the interval around the closest point only. O(bisection steps x core size).
pub(super) fn band_len(a: Vec2, b: Vec2, core: &Core, need: f64) -> f64 {
    let d = b - a;
    let l = d.len();
    if l < 1.0 {
        return 0.0;
    }
    let at = |t: f64| a + d * t;
    let dist = |t: f64| seg_core(at(t), at(t), core).0;
    let proj = |p: Vec2| ((p - a).dot(d) / (l * l)).clamp(0.0, 1.0);
    // `seg_core` returns the closest pair (the order of the two points depends on the core kind):
    // the one on `a-b` is the one nearer to the segment.
    let (_, p, q) = seg_core(a, b, core);
    let off = |x: Vec2| (at(proj(x)) - x).len();
    let tc = proj(if off(p) <= off(q) { p } else { q });
    if dist(tc) >= need {
        return 0.0;
    }
    let edge = |mut inside: f64, mut outside: f64| {
        if dist(outside) < need {
            return outside;
        }
        for _ in 0..30 {
            let m = 0.5 * (inside + outside);
            if dist(m) < need {
                inside = m;
            } else {
                outside = m;
            }
        }
        0.5 * (inside + outside)
    };
    (edge(tc, 1.0) - edge(tc, 0.0)) * l
}
/// forced-draw: a segment `d` from object `e` at width `sw` overlaps its copper and `e` is
/// foreign copper (not a hole, the board edge or a keepout, `fixed`; not the own net `net`): a
/// short (KiCad `shorting_items`; the checker's `gap < 0`).
pub(super) fn ent_short(e: &super::draw::Ent, net: i32, d: f64, sw: f64) -> bool {
    !e.fixed && e.net != Some(net) && d < e.grow + 0.5 * sw
}
/// Rings of pocket triangles around the strip.
const POCKET_DEPTH: usize = 2;

/// The sleeve of a run: its triangles and the walls (sides never crossed by the wire).
pub(super) struct Strip {
    pub(super) tris: Vec<[Vec2; 3]>,
    pub(super) boxes: Vec<Aabb>,
    pub(super) walls: Vec<(Vec2, Vec2)>,
    /// Triangle ids per cell (empty: few triangles, `contains` scans them all).
    grid: TriGrid,
}

/// A uniform grid over the strip's triangles: a triangle sits in every cell its box grown by
/// [`TRI_PAD`] overlaps. `contains` accepts `p` only inside a box grown by 1 nm, and the cell of
/// a coordinate is monotone in it, so every triangle that can hold `p` is in `p`'s cell: the
/// grid skips only triangles the box test of the scan rejects (the result is the same; `any`
/// does not depend on the order).
#[derive(Default)]
struct TriGrid {
    lo: Vec2,
    hi: Vec2,
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

/// Below this many triangles `contains` scans them all.
const TRI_GRID_MIN: usize = 16;
/// Grow of a triangle box in the grid (nm), more than the 1 nm of the box test.
const TRI_PAD: f64 = 2.0;

impl TriGrid {
    fn new(boxes: &[Aabb]) -> TriGrid {
        if boxes.len() < TRI_GRID_MIN {
            return TriGrid::default();
        }
        let mut lo = Vec2 { x: f64::INFINITY, y: f64::INFINITY };
        let mut hi = Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY };
        for b in boxes {
            lo = Vec2 { x: lo.x.min(b.min.x - TRI_PAD), y: lo.y.min(b.min.y - TRI_PAD) };
            hi = Vec2 { x: hi.x.max(b.max.x + TRI_PAD), y: hi.y.max(b.max.y + TRI_PAD) };
        }
        if !(lo.x.is_finite() && lo.y.is_finite() && hi.x.is_finite() && hi.y.is_finite()) {
            return TriGrid::default();
        }
        let side = (hi.x - lo.x).max(hi.y - lo.y).max(1.0);
        let cell = (side / (boxes.len() as f64).sqrt()).max(1.0);
        let nx = (((hi.x - lo.x) / cell).ceil() as usize).clamp(1, 256);
        let ny = (((hi.y - lo.y) / cell).ceil() as usize).clamp(1, 256);
        let mut g = TriGrid { lo, hi, cell, nx, ny, cells: vec![Vec::new(); nx * ny] };
        for (i, b) in boxes.iter().enumerate() {
            for y in g.slot(b.min.y - TRI_PAD, lo.y, ny)..=g.slot(b.max.y + TRI_PAD, lo.y, ny) {
                for x in g.slot(b.min.x - TRI_PAD, lo.x, nx)..=g.slot(b.max.x + TRI_PAD, lo.x, nx) {
                    g.cells[y * nx + x].push(i as u32);
                }
            }
        }
        g
    }

    /// The cell index of coordinate `v` (monotone in `v`, clamped to the grid).
    fn slot(&self, v: f64, lo: f64, n: usize) -> usize {
        (((v - lo) / self.cell).floor().max(0.0) as usize).min(n - 1)
    }
}

impl Strip {
    pub(super) fn new(tris: Vec<[Vec2; 3]>, walls: Vec<(Vec2, Vec2)>) -> Strip {
        let boxes: Vec<Aabb> = tris.iter().map(|t| bbox_of(t)).collect();
        let grid = TriGrid::new(&boxes);
        Strip { tris, boxes, walls, grid }
    }

    /// `p` lies in (or on the boundary of) triangle `i` of the sleeve.
    #[inline]
    fn in_tri(&self, i: usize, p: Vec2) -> bool {
        let (t, bb) = (&self.tris[i], &self.boxes[i]);
        if p.x < bb.min.x - 1.0 || p.x > bb.max.x + 1.0 || p.y < bb.min.y - 1.0 || p.y > bb.max.y + 1.0 {
            return false;
        }
        let d = |a: Vec2, b: Vec2| (b - a).cross(p - a);
        let (d1, d2, d3) = (d(t[0], t[1]), d(t[1], t[2]), d(t[2], t[0]));
        let tol = -2.0 * (t[1] - t[0]).len().max((t[2] - t[1]).len()).max(1.0);
        d1 >= tol && d2 >= tol && d3 >= tol
    }

    /// `p` lies in (or on the boundary of) a triangle of the sleeve.
    pub(super) fn contains(&self, p: Vec2) -> bool {
        let g = &self.grid;
        if g.cells.is_empty() {
            return (0..self.tris.len()).any(|i| self.in_tri(i, p));
        }
        if p.x < g.lo.x || p.x > g.hi.x || p.y < g.lo.y || p.y > g.hi.y {
            return false;
        }
        let c = g.slot(p.y, g.lo.y, g.ny) * g.nx + g.slot(p.x, g.lo.x, g.nx);
        g.cells[c].iter().any(|&i| self.in_tri(i as usize, p))
    }

    /// The segment `a-b` lies at least partly in the sleeve.
    pub(super) fn touches(&self, a: Vec2, b: Vec2) -> bool {
        self.contains(a) || self.contains(b) || self.contains((a + b) * 0.5) || self.crosses_wall(a, b)
    }

    /// The segment `a-b` properly crosses a wall.
    pub(super) fn crosses_wall(&self, a: Vec2, b: Vec2) -> bool {
        let bb = bbox_of(&[a, b]);
        self.walls.iter().any(|&(c, d)| {
            let wb = bbox_of(&[c, d]);
            if !bb.overlaps(&wb) {
                return false;
            }
            let (d1, d2) = (b - a, d - c);
            let den = d1.cross(d2);
            if den.abs() < 1e-9 {
                return false;
            }
            let t = (c - a).cross(d2) / den;
            let u = (c - a).cross(d1) / den;
            t > 1e-9 && t < 1.0 - 1e-9 && u > 1e-9 && u < 1.0 - 1e-9
        })
    }
}

/// F6 (profile 2026-11-18: `seg_clear_w` was 22-26 % of the drawing, every segment test of a
/// search looping over all its objects): the objects of one search in a loose uniform grid, by the
/// cell of their box's low corner (boxes wider than [`NEAR_BIG`] cells are tested always). A
/// segment test visits only the cells whose objects can reach the segment: an object is skipped
/// only when its box misses the segment's box grown by `c + sw / 2 + 1 nm`, where `c` is the
/// largest clearance need of the set at width 0 (a need grows by exactly `sw / 2` with the
/// width), so the exact per-object test ([`Drawer::ent_clear`]) would have passed it by its own
/// box test. The answer is that of [`Drawer::seg_clear`] (an `all` over the objects, order-free).
pub(super) struct Near {
    wp: Wp,
    ids: Vec<usize>,
    c: f64,
    big: Vec<usize>,
    /// Largest box side of the objects in `cells`.
    ext: f64,
    lo: Vec2,
    cell: f64,
    nx: usize,
    ny: usize,
    /// Empty: fewer than [`NEAR_MIN`] objects, every test scans them.
    cells: Vec<Vec<usize>>,
}

/// Sets with fewer objects are scanned without a grid.
const NEAR_MIN: usize = 24;
/// Smallest cell of a [`Near`] grid (nm).
const NEAR_CELL_MIN: f64 = 0.05e6;
/// Objects whose box is wider than this many cells are tested by every query.
const NEAR_BIG: f64 = 4.0;

impl Near {
    /// Cell index of `v` (clamped to `0..n`; monotone in `v`).
    fn slot(&self, v: f64, lo: f64, n: usize) -> usize {
        (((v - lo) / self.cell).floor().max(0.0) as usize).min(n - 1)
    }
}

/// Heap entry: ordered by `f` ascending, then node id.
struct Open {
    f: f64,
    n: usize,
}
impl PartialEq for Open {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for Open {}
impl PartialOrd for Open {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Open {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.total_cmp(&self.f).then(o.n.cmp(&self.n))
    }
}

/// Parameters of the wire being repaired.
#[derive(Clone, Copy)]
pub(super) struct Wp {
    pub wire: WireId,
    pub net: i32,
    pub w: f64,
    pub s: f64,
    pub layer: usize,
    /// The wire's own end pads: the only own-net pads it may overlap.
    pub own: [usize; 2],
    /// Necking (item 3): within `radius` of an end point the wire is `min_w` wide.
    pub neck: Option<Neck>,
    /// Never drop (M-B1): the clearance to foreign objects is kept only up to this much less (nm;
    /// 0 = the rule): the least violating geometry where no legal one exists.
    pub relax: f64,
}

/// Necking of a run: `(centre, radius nm)` zones inside which the wire is `min_w` wide: the
/// pad-end zones of the drawing's own necks, or (M-C) the necked stretches of the plan (from a
/// pad end, or in between; up to [`NECK_ZONES`]).
#[derive(Clone, Copy)]
pub(super) struct Neck {
    pub min_w: f64,
    pub ends: [Option<(Vec2, f64)>; NECK_ZONES],
}

/// Most neck zones of one wire: its two pad ends and up to four necked stretches in between.
pub(super) const NECK_ZONES: usize = 6;


impl Wp {
    /// The segment `a-b` cut where it crosses a neck zone's circle, each piece with the width the
    /// wire has there: narrow inside a zone, full outside (the same cut as the drawing's
    /// `Drawer::neck_items`, M-C: a necked stretch ends inside a segment, not at a node).
    fn pieces(&self, a: Vec2, b: Vec2) -> Vec<(Vec2, Vec2, f64)> {
        let Some(n) = &self.neck else { return vec![(a, b, self.w)] };
        let d = b - a;
        let l2 = d.len2();
        let mut cuts = vec![0.0, 1.0];
        if l2 > 1.0 {
            for (c, r) in n.ends.iter().flatten() {
                if !r.is_finite() {
                    continue;
                }
                let f = a - *c;
                let (qb, qc) = (f.dot(d), f.len2() - r * r);
                let disc = qb * qb - l2 * qc;
                if disc > 0.0 {
                    let sq = disc.sqrt();
                    for t in [(-qb - sq) / l2, (-qb + sq) / l2] {
                        if t > 0.0 && t < 1.0 {
                            cuts.push(t);
                        }
                    }
                }
            }
        }
        cuts.sort_by(f64::total_cmp);
        cuts.windows(2)
            .map(|c| {
                let (u, v) = (a + d * c[0], a + d * c[1]);
                let m = (u + v) * 0.5;
                let inside = n.ends.iter().flatten().any(|(cc, r)| (m - *cc).len() <= *r);
                (u, v, if inside { n.min_w } else { self.w })
            })
            .collect()
    }
}

impl Drawer<'_> {
    /// The sleeve of passes `k0..k1` of wire `w`: the triangles of its strip, plus one ring of
    /// *pockets* (a free triangle across a side the wire never crosses, entered and left through
    /// that side only: going into it and back changes no crossing, and it is where a path hugging a
    /// corner at its clearance radius has to go), plus the triangles inside the wire's own pads.
    /// Walls are the sides left over: strip sides with nothing free behind them, and the other
    /// two sides of a pocket. A side shared by two strip triangles is open.
    pub(super) fn strip(&self, w: WireId, k0: usize, k1: usize) -> Strip {
        let (st, pr) = (self.st, &self.b.proj);
        let wi = &st.wires[w];
        let portal: Vec<usize> = (k0..k1).map(|k| st.passes[wi.passes[k]].edge).collect();
        let mut tids: Vec<TriId> = wi.tris[k0..=k1].to_vec();
        tids.sort_unstable();
        tids.dedup();
        let own = |t: TriId| pr.tri_obs[t].iter().all(|&o| pr.obstacles[o].net == Some(wi.net) || o == wi.pa || o == wi.pb);
        // Pockets, `POCKET_DEPTH` rings: a free triangle across a side of the region so far, with
        // exactly one entry side.
        let mut region: Vec<TriId> = tids.clone();
        let mut pockets: Vec<(TriId, usize)> = Vec::new();
        let mut frontier: Vec<TriId> = tids.clone();
        for _ in 0..POCKET_DEPTH {
            let mut cand: Vec<(TriId, usize)> = Vec::new();
            for &t in &frontier {
                for i in 0..3 {
                    let e = pr.tri_edges[t][i];
                    if portal.contains(&e) {
                        continue;
                    }
                    if let Some(p) = pr.across(e, t) {
                        if !region.contains(&p) && !pr.tri_out[p] && own(p) {
                            cand.push((p, e));
                        }
                    }
                }
            }
            frontier = Vec::new();
            for &(p, e) in &cand {
                if cand.iter().filter(|&&(q, _)| q == p).count() == 1 {
                    pockets.push((p, e));
                    frontier.push(p);
                }
            }
            region.extend(frontier.iter().copied());
        }
        let mut tris = Vec::new();
        let mut walls = Vec::new();
        for &t in &tids {
            let p = pr.tris[t].map(|v| pr.pos[v]);
            tris.push(p);
            for i in 0..3 {
                let e = pr.tri_edges[t][i];
                if portal.contains(&e) {
                    continue;
                }
                let open = pr.across(e, t).is_some_and(|x| region.contains(&x));
                if !open {
                    walls.push((p[i], p[(i + 1) % 3]));
                }
            }
        }
        for &(t, entry) in &pockets {
            let p = pr.tris[t].map(|v| pr.pos[v]);
            tris.push(p);
            for i in 0..3 {
                let e = pr.tri_edges[t][i];
                if e == entry {
                    continue;
                }
                // The entry of a deeper pocket is open from this side.
                let open = pockets.iter().any(|&(q, qe)| qe == e && q != t);
                if !open {
                    walls.push((p[i], p[(i + 1) % 3]));
                }
            }
        }
        Strip::new(tris, walls)
    }

    /// Objects the wire must clear that may reach into `bb`.
    pub(super) fn local_ents(&self, wp: &Wp, bb: Aabb) -> Vec<usize> {
        let mut ids = self.grid.query(bb);
        ids.sort_unstable();
        ids.dedup();
        let n_obs = self.b.proj.obstacles.len();
        ids.retain(|&i| {
            let e = &self.ents[i];
            // Foreign objects, and the pads of the wire's own net other than its end pads: a
            // track grazing one overlaps it by a sliver, which KiCad reports as a connection
            // narrower than the minimum (they are kept apart without a clearance).
            // An exit stub (5.10) is no copper: the wires of its own net pass through it freely.
            let own_pad = e.net == Some(wp.net) && i < n_obs && !wp.own.contains(&i) && !self.is_exit_stub(i) && !self.is_kept_end(i);
            e.live && (wp.layer == usize::MAX || (e.mask >> wp.layer) & 1 == 1) && (e.net != Some(wp.net) || own_pad) && (i < self.n_static || !self.only_static.load(std::sync::atomic::Ordering::Relaxed))
        });
        ids
    }

    pub(super) fn need_of(&self, id: usize, wp: &Wp, w: f64) -> f64 {
        let e = &self.ents[id];
        if e.net == Some(wp.net) {
            // A pad of the own net that is not an end: KiCad's minimum connection width check
            // builds the pad's polygon up to `max_error` outside its arcs, so a track closer than
            // that reads as a sliver connection (a resistor pad, D2: 1.7 um off the arc,
            // "actual 0.0023 mm").
            return e.grow + 0.5 * w + self.eps + self.model.rules.max_error as f64;
        }
        (e.grow + if e.fixed { e.clear } else { e.clear.max(wp.s) } + 0.5 * w + self.eps - wp.relax).max(e.grow)
    }

    /// The segment `a-b` keeps the clearance to every object of `ents`.
    pub(super) fn seg_clear(&self, ents: &[usize], wp: &Wp, a: Vec2, b: Vec2) -> bool {
        if wp.neck.is_some() {
            return wp.pieces(a, b).into_iter().all(|(u, v, w)| self.seg_clear_w(ents, wp, u, v, self.seg_width(wp.wire, wp.layer, u, v, w)));
        }
        self.seg_clear_w(ents, wp, a, b, self.seg_width(wp.wire, wp.layer, a, b, wp.w))
    }

    /// [`Drawer::seg_clear`] of one piece at width `sw`.
    fn seg_clear_w(&self, ents: &[usize], wp: &Wp, a: Vec2, b: Vec2, sw: f64) -> bool {
        let bb = bbox_of(&[a, b]);
        ents.iter().all(|&i| self.ent_clear(i, wp, a, b, &bb, sw))
    }

    /// The segment `a-b` (box `bb`) at width `sw` keeps the clearance to object `i`.
    #[inline]
    fn ent_clear(&self, i: usize, wp: &Wp, a: Vec2, b: Vec2, bb: &Aabb, sw: f64) -> bool {
        let e = &self.ents[i];
        let need = self.need_of(i, wp, sw);
        // The core's box (cached at `add_ent`; the same min/max of its points).
        let (lo, hi) = (self.ent_box[i].min, self.ent_box[i].max);
        if bb.min.x > hi.x + need || bb.max.x < lo.x - need || bb.min.y > hi.y + need || bb.max.y < lo.y - need {
            return true;
        }
        seg_core(a, b, &e.core).0 >= need - TOL
    }

    /// The objects `ents` of one search (the window of [`Self::local_path`], the region of
    /// [`Drawer::octilinear`]) indexed for the many segment tests it makes ([`Near`]).
    pub(super) fn near(&self, ents: Vec<usize>, wp: &Wp) -> Near {
        let n = ents.len();
        let c = ents.iter().map(|&i| self.need_of(i, wp, 0.0)).fold(0.0, f64::max);
        let mut nr = Near { wp: *wp, ids: ents, c, big: Vec::new(), ext: 0.0, lo: Vec2::default(), cell: 1.0, nx: 0, ny: 0, cells: Vec::new() };
        if n < NEAR_MIN {
            return nr;
        }
        let mut u = self.ent_box[nr.ids[0]];
        for &i in &nr.ids {
            u = u.union(&self.ent_box[i]);
        }
        let (w, h) = (u.max.x - u.min.x, u.max.y - u.min.y);
        nr.cell = (w.max(h) / (n as f64).sqrt()).max(NEAR_CELL_MIN);
        nr.lo = u.min;
        (nr.nx, nr.ny) = ((w / nr.cell).floor() as usize + 1, (h / nr.cell).floor() as usize + 1);
        nr.cells = vec![Vec::new(); nr.nx * nr.ny];
        for k in 0..n {
            let i = nr.ids[k];
            let bx = self.ent_box[i];
            let side = (bx.max.x - bx.min.x).max(bx.max.y - bx.min.y);
            if side > NEAR_BIG * nr.cell {
                nr.big.push(i);
                continue;
            }
            nr.ext = nr.ext.max(side);
            let (x, y) = (nr.slot(bx.min.x, nr.lo.x, nr.nx), nr.slot(bx.min.y, nr.lo.y, nr.ny));
            nr.cells[y * nr.nx + x].push(i);
        }
        nr
    }

    /// [`Drawer::seg_clear`] against the objects of `nr` (the same answer, testing only the objects
    /// near the segment; see [`Near`]).
    pub(super) fn seg_clear_near(&self, nr: &Near, a: Vec2, b: Vec2) -> bool {
        let wp = &nr.wp;
        if wp.neck.is_some() {
            return wp.pieces(a, b).into_iter().all(|(u, v, w)| self.seg_clear_w_near(nr, u, v, self.seg_width(wp.wire, wp.layer, u, v, w)));
        }
        self.seg_clear_w_near(nr, a, b, self.seg_width(wp.wire, wp.layer, a, b, wp.w))
    }

    fn seg_clear_w_near(&self, nr: &Near, a: Vec2, b: Vec2, sw: f64) -> bool {
        if nr.cells.is_empty() {
            return self.seg_clear_w(&nr.ids, &nr.wp, a, b, sw);
        }
        let bb = bbox_of(&[a, b]);
        let wp = &nr.wp;
        if !nr.big.iter().all(|&i| self.ent_clear(i, wp, a, b, &bb, sw)) {
            return false;
        }
        // Every object whose box meets the segment's box grown by the largest need (plus 1 nm)
        // has its box's low corner in this range (boxes up to `ext` wide, plus 1 nm).
        let g = nr.c + 0.5 * sw + 1.0;
        let (x0, x1) = (nr.slot(bb.min.x - g - nr.ext - 1.0, nr.lo.x, nr.nx), nr.slot(bb.max.x + g, nr.lo.x, nr.nx));
        let (y0, y1) = (nr.slot(bb.min.y - g - nr.ext - 1.0, nr.lo.y, nr.ny), nr.slot(bb.max.y + g, nr.lo.y, nr.ny));
        for y in y0..=y1 {
            for x in x0..=x1 {
                if !nr.cells[y * nr.nx + x].iter().all(|&i| self.ent_clear(i, wp, a, b, &bb, sw)) {
                    return false;
                }
            }
        }
        true
    }

    /// forced-draw (never drop, docs/dev/design/forced-draw.md): the violation amount of the
    /// segment `a-b` against the objects of `nr`, the graded counterpart of
    /// [`Drawer::seg_clear_near`] (0 exactly when that is true, up to its tolerance): per object
    /// it comes too close to, [`violation_amount`](super::factors::violation_amount) of
    /// the depth (the clearance missing at the closest point, [`Drawer::need_of`] at `nr`'s
    /// relax) and the length of the segment inside the object's clearance band ([`band_len`]).
    /// The sum of [`Drawer::seg_deficit_parts_near`]. O(objects near the segment x bisection
    /// steps). Tests only: the drawing reads the parts.
    #[cfg(test)]
    pub(super) fn seg_deficit_near(&self, nr: &Near, a: Vec2, b: Vec2) -> f64 {
        let [c, s] = self.seg_deficit_parts_near(nr, a, b);
        c + s
    }

    /// [`Drawer::seg_deficit_near`] split as `[clearance, short]`: the amounts against the
    /// objects whose copper the segment's copper overlaps (a foreign pad, via, track or copper
    /// shape closer than its copper radius plus half the width: KiCad's `shorting_items`, which
    /// Cleanup removes, the checker's `short`), and the rest (clearance; holes, board edge and
    /// keepouts, `fixed`, and own-net slivers stay here).
    pub(super) fn seg_deficit_parts_near(&self, nr: &Near, a: Vec2, b: Vec2) -> [f64; 2] {
        let wp = &nr.wp;
        if wp.neck.is_some() {
            return wp.pieces(a, b).into_iter().fold([0.0; 2], |t, (u, v, w)| {
                let x = self.seg_deficit_w_near(nr, u, v, self.seg_width(wp.wire, wp.layer, u, v, w));
                [t[0] + x[0], t[1] + x[1]]
            });
        }
        self.seg_deficit_w_near(nr, a, b, self.seg_width(wp.wire, wp.layer, a, b, wp.w))
    }

    fn seg_deficit_w_near(&self, nr: &Near, a: Vec2, b: Vec2, sw: f64) -> [f64; 2] {
        let bb = bbox_of(&[a, b]);
        let wp = &nr.wp;
        let mut tot = [0.0; 2];
        let mut add = |i: usize| {
            let x = self.ent_deficit(i, wp, a, b, &bb, sw);
            tot[0] += x[0];
            tot[1] += x[1];
        };
        if nr.cells.is_empty() {
            nr.ids.iter().for_each(|&i| add(i));
            return tot;
        }
        nr.big.iter().for_each(|&i| add(i));
        // The cells of `seg_clear_w_near`: every object in reach has its low corner there.
        let g = nr.c + 0.5 * sw + 1.0;
        let (x0, x1) = (nr.slot(bb.min.x - g - nr.ext - 1.0, nr.lo.x, nr.nx), nr.slot(bb.max.x + g, nr.lo.x, nr.nx));
        let (y0, y1) = (nr.slot(bb.min.y - g - nr.ext - 1.0, nr.lo.y, nr.ny), nr.slot(bb.max.y + g, nr.lo.y, nr.ny));
        for y in y0..=y1 {
            for x in x0..=x1 {
                nr.cells[y * nr.nx + x].iter().for_each(|&i| add(i));
            }
        }
        tot
    }

    /// The violation amount of `a-b` (box `bb`, width `sw`) against object `i`, `[clearance,
    /// short]` (one of them 0): 0 where [`Drawer::ent_clear`] holds.
    #[inline]
    fn ent_deficit(&self, i: usize, wp: &Wp, a: Vec2, b: Vec2, bb: &Aabb, sw: f64) -> [f64; 2] {
        let e = &self.ents[i];
        let need = self.need_of(i, wp, sw);
        let (lo, hi) = (self.ent_box[i].min, self.ent_box[i].max);
        if bb.min.x > hi.x + need || bb.max.x < lo.x - need || bb.min.y > hi.y + need || bb.max.y < lo.y - need {
            return [0.0; 2];
        }
        let d = seg_core(a, b, &e.core).0;
        if d >= need - TOL {
            return [0.0; 2];
        }
        let amt = super::factors::violation_amount(need - d, band_len(a, b, &e.core, need));
        if ent_short(e, wp.net, d, sw) {
            [0.0, amt]
        } else {
            [amt, 0.0]
        }
    }

    /// Candidate nodes around the corners of object `id` that fall in `bb`.
    fn corner_nodes(&self, id: usize, wp: &Wp, bb: &Aabb, out: &mut Vec<Vec2>) {
        // Nodes carry the full width, plus the narrow one where a neck zone is near.
        let need = self.need_of(id, wp, wp.w);
        let mut needs = vec![need];
        if let Some(x) = self.widths.as_ref().filter(|_| wp.layer != usize::MAX) {
            if x.min_w[wp.wire] < wp.w {
                needs.push(self.need_of(id, wp, x.min_w[wp.wire]));
            }
        }
        if let Some(n) = &wp.neck {
            if n.ends.iter().flatten().any(|(c, r)| bb.min.x - r <= c.x && c.x <= bb.max.x + r && bb.min.y - r <= c.y && c.y <= bb.max.y + r) {
                needs.push(self.need_of(id, wp, n.min_w));
            }
        }
        for need in needs {
            self.corner_nodes_at(id, need, bb, out);
        }
    }

    fn corner_nodes_at(&self, id: usize, need: f64, bb: &Aabb, out: &mut Vec<Vec2>) {
        let circle = |c: Vec2, out: &mut Vec<Vec2>| {
            let r = need / (0.5 * STEP).cos();
            let n = (2.0 * std::f64::consts::PI / STEP).round() as usize;
            for i in 0..n {
                let a = (i as f64 + 0.5) * STEP;
                out.push(c + Vec2 { x: a.cos() * r, y: a.sin() * r });
            }
        };
        let inside = |p: Vec2| p.x >= bb.min.x - need && p.x <= bb.max.x + need && p.y >= bb.min.y - need && p.y <= bb.max.y + need;
        match &self.ents[id].core {
            Core::Pt(c) => {
                if inside(*c) {
                    circle(*c, out);
                }
            }
            Core::Seg(a, b) => {
                for c in [*a, *b] {
                    if inside(c) {
                        circle(c, out);
                    }
                }
            }
            Core::Poly(poly) => {
                let n = poly.len();
                let area: f64 = (0..n).map(|i| poly[i].cross(poly[(i + 1) % n])).sum();
                let ccw = area >= 0.0;
                for i in 0..n {
                    let v = poly[i];
                    if !inside(v) {
                        continue;
                    }
                    let (p, q) = (poly[(i + n - 1) % n], poly[(i + 1) % n]);
                    let (d1, d2) = (v - p, q - v);
                    let (Some(u1), Some(u2)) = (d1.unit(), d2.unit()) else { continue };
                    // Outward normals (right of the travel direction for a CCW polygon).
                    let sgn = if ccw { 1.0 } else { -1.0 };
                    let n1 = Vec2 { x: u1.y, y: -u1.x } * sgn;
                    let n2 = Vec2 { x: u2.y, y: -u2.x } * sgn;
                    let turn = (u1.cross(u2)).atan2(u1.dot(u2)) * sgn;
                    if turn > 1e-3 {
                        let k = (turn / STEP).ceil().max(1.0) as usize;
                        let a0 = n1.angle();
                        let rr = need / (0.5 * turn / k as f64).cos();
                        for j in 0..=k {
                            let a = a0 + turn * sgn * j as f64 / k as f64;
                            let _ = n2;
                            out.push(v + Vec2 { x: a.cos() * rr, y: a.sin() * rr });
                        }
                    } else {
                        // Reflex or straight: the offset lines meet on the bisector.
                        let Some(bis) = (n1 + n2).unit() else { continue };
                        let c = bis.dot(n1).max(0.2);
                        out.push(v + bis * (need / c * 1.0005));
                    }
                }
            }
        }
    }

    /// The shortest path from `a` to `b` in the sleeve that clears `ents`, searched over nodes in
    /// the window `bb`; `None` when there is none or the node cap is hit.
    fn local_path(&self, strip: &Strip, wp: &Wp, a: Vec2, b: Vec2, bb: Aabb) -> Option<Vec<Vec2>> {
        let reach = self.qgrow;
        let ents = self.local_ents(wp, bb.grow(reach));
        let nr = self.near(ents.clone(), wp);
        let mut cand: Vec<Vec2> = Vec::new();
        for &id in &ents {
            self.corner_nodes(id, wp, &bb, &mut cand);
        }
        let mut nodes: Vec<Vec2> = vec![a, b];
        for p in cand {
            if nodes.len() > MAX_NODES {
                if std::env::var("TOPO_DEBUG").is_ok() {
                    eprintln!("EXACTWIN wire {} cap: more than {MAX_NODES} nodes in a {:.1} x {:.1} mm window", wp.wire, (bb.max.x - bb.min.x) / 1e6, (bb.max.y - bb.min.y) / 1e6);
                }
                return None;
            }
            if p.x < bb.min.x || p.x > bb.max.x || p.y < bb.min.y || p.y > bb.max.y {
                continue;
            }
            if strip.contains(p) && self.seg_clear_near(&nr, p, p) {
                nodes.push(p);
            }
        }
        let n = nodes.len();
        let mut g = vec![f64::INFINITY; n];
        let mut from = vec![usize::MAX; n];
        let mut done = vec![false; n];
        let mut heap = BinaryHeap::new();
        g[0] = 0.0;
        heap.push(Open { f: (b - a).len(), n: 0 });
        while let Some(Open { n: u, .. }) = heap.pop() {
            if done[u] {
                continue;
            }
            done[u] = true;
            if u == 1 {
                break;
            }
            for v in 0..n {
                if done[v] || v == u {
                    continue;
                }
                let d = (nodes[v] - nodes[u]).len();
                if g[u] + d >= g[v] {
                    continue;
                }
                let (p, q) = (nodes[u], nodes[v]);
                let mid = (p + q) * 0.5;
                if strip.crosses_wall(p, q) || !strip.contains(mid) || !self.seg_clear_near(&nr, p, q) {
                    continue;
                }
                g[v] = g[u] + d;
                from[v] = u;
                heap.push(Open { f: g[v] + (b - nodes[v]).len(), n: v });
            }
        }
        if !done[1] {
            if std::env::var("TOPO_DEBUG").is_ok() {
                let reached: Vec<String> = (0..n).filter(|&i| done[i]).map(|i| format!("({:.3},{:.3})", nodes[i].x / 1e6, -nodes[i].y / 1e6)).collect();
                eprintln!(
                    "EXACTWIN wire {} no path: {n} nodes in a {:.1} x {:.1} mm window; from ({:.3},{:.3}) to ({:.3},{:.3}); reached {}: {}",
                    wp.wire,
                    (bb.max.x - bb.min.x) / 1e6,
                    (bb.max.y - bb.min.y) / 1e6,
                    a.x / 1e6,
                    -a.y / 1e6,
                    b.x / 1e6,
                    -b.y / 1e6,
                    reached.len(),
                    reached.join(" ")
                );
            }
            return None;
        }
        let mut path = vec![b];
        let mut c = 1;
        while from[c] != usize::MAX {
            c = from[c];
            path.push(nodes[c]);
        }
        path.reverse();
        Some(path)
    }

    /// `pts` without the vertices where it turns back (by more than 90 degrees) and the
    /// shortcut past the vertex is as legal as a [`Self::local_path`] edge (inside the sleeve,
    /// no wall crossed, every clearance kept). `exact_fix` repairs the funnel stretch by stretch
    /// and keeps the funnel vertex between two repairs; when both wrap the same corner, that
    /// vertex is left behind as an out-and-back spike (a segment and its exact reverse). The
    /// ends stay. O(vertices x local entities) per sweep, sweeps until nothing drops.
    fn drop_backtracks(&self, strip: &Strip, wp: &Wp, mut pts: Vec<Vec2>) -> Vec<Vec2> {
        loop {
            let mut dropped = false;
            let mut i = 1;
            while i + 1 < pts.len() {
                let (p, q, r) = (pts[i - 1], pts[i], pts[i + 1]);
                if (q - p).dot(r - q) < 0.0 {
                    let ents = self.local_ents(wp, bbox_of(&[p, r]).grow(self.qgrow));
                    if !strip.crosses_wall(p, r) && strip.contains((p + r) * 0.5) && self.seg_clear(&ents, wp, p, r) {
                        pts.remove(i);
                        dropped = true;
                        continue;
                    }
                }
                i += 1;
            }
            pts.dedup_by(|a, b| (*a - *b).len2() < 1.0);
            if !dropped {
                return pts;
            }
        }
    }

    /// `p` keeps every clearance and lies in the sleeve.
    fn vertex_ok(&self, strip: &Strip, wp: &Wp, p: Vec2) -> bool {
        let ents = self.local_ents(wp, bbox_of(&[p]).grow(self.qgrow));
        strip.contains(p) && self.seg_clear(&ents, wp, p, p)
    }

    /// Repair the polyline `pts` of one run (`passes k0..k1` of `w` on `layer`): every stretch
    /// that violates a clearance is replaced by the exact shortest path around it. `None` when
    /// a stretch cannot be repaired (no path in the sleeve, or the node cap).
    pub(super) fn exact_fix(&self, w: WireId, k0: usize, k1: usize, layer: usize, pts: Vec<Vec2>, neck: Option<Neck>) -> Option<Vec<Vec2>> {
        self.exact_fix_relaxed(w, k0, k1, layer, pts, neck, 0.0)
    }

    /// [`Drawer::exact_fix`] keeping the clearances to foreign objects only up to `relax` nm less
    /// (never drop, M-B1: the least violating geometry where no legal one exists).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn exact_fix_relaxed(&self, w: WireId, k0: usize, k1: usize, layer: usize, mut pts: Vec<Vec2>, neck: Option<Neck>, relax: f64) -> Option<Vec<Vec2>> {
        let wi = &self.st.wires[w];
        let wp = Wp { wire: w, net: wi.net, w: wi.w, s: wi.s, layer, own: [wi.pa, wi.pb], neck, relax };
        let mut strip = self.strip(w, k0, k1);
        // The chords of other nets' vias in the sleeve are walls too (see `via_walls`); a stretch
        // that crosses one is repaired like a clearance violation.
        let n_walls = strip.walls.len();
        strip.walls.extend(self.via_walls(w, k0, k1, layer));
        let via_wall = |a: Vec2, b: Vec2| strip.walls[n_walls..].iter().any(|&(c, d)| super::draw::seg_proper(a, b, c, d));
        // F6: whether a segment breaks a clearance or a via wall depends only on its two ends in
        // this call (the sleeve and the objects stay); a repair changes one stretch, so the
        // segments before and after it are not tested again. Keyed by the ends' bits.
        let mut seg_ok: crate::fxhash::FxHashMap<[u64; 4], bool> = crate::fxhash::map();
        for _ in 0..24 {
            let mut bad = None;
            for i in 0..pts.len() - 1 {
                let (a, b) = (pts[i], pts[i + 1]);
                let key = [a.x.to_bits(), a.y.to_bits(), b.x.to_bits(), b.y.to_bits()];
                let ok = *seg_ok.entry(key).or_insert_with(|| {
                    let ents = self.local_ents(&wp, bbox_of(&[a, b]).grow(self.qgrow));
                    self.seg_clear(&ents, &wp, a, b) && !via_wall(a, b)
                });
                if !ok {
                    bad = Some(i);
                    break;
                }
            }
            let Some(i) = bad else { return Some(self.drop_backtracks(&strip, &wp, pts)) };
            // A via wall crossed: the run reached the wrong side of the via before this
            // segment, so the search starts one vertex earlier.
            let (mut l, mut r) = (if via_wall(pts[i], pts[i + 1]) { i.saturating_sub(1) } else { i }, i + 1);
            while l > 0 && !self.vertex_ok(&strip, &wp, pts[l]) {
                l -= 1;
            }
            while r < pts.len() - 1 && !self.vertex_ok(&strip, &wp, pts[r]) {
                r += 1;
            }
            let base = bbox_of(&pts[l..=r]);
            let mut sub = None;
            for m in [2.0e6, 8.0e6] {
                sub = self.local_path(&strip, &wp, pts[l], pts[r], base.grow(m));
                if sub.is_some() {
                    break;
                }
            }
            let Some(sub) = sub else {
                if std::env::var("TOPO_DEBUG").is_ok() {
                    // Diagnosis: what the violating segment hits, and whether the static board
                    // alone already blocks the stretch.
                    let ents = self.local_ents(&wp, bbox_of(&[pts[i], pts[i + 1]]).grow(self.qgrow));
                    let hit: Vec<String> = ents
                        .iter()
                        .filter(|&&id| !self.seg_clear(&[id], &wp, pts[i], pts[i + 1]))
                        .map(|&id| format!("{}{}", if id < self.n_static { "static" } else { "drawn" }, if self.ents[id].grow > 0.0 && matches!(self.ents[id].core, Core::Pt(_)) { "-disc" } else { "" }))
                        .collect();
                    eprintln!(
                        "EXACTNONE wire {w} net {} L{layer} k {k0}..{k1} stretch {l}..{r} of {} at ({:.3},{:.3}) mm hits {:?}",
                        wi.net,
                        pts.len(),
                        0.5 * (pts[i].x + pts[i + 1].x) / 1e6,
                        -0.5 * (pts[i].y + pts[i + 1].y) / 1e6,
                        hit
                    );
                }
                return None;
            };
            let mut np: Vec<Vec2> = pts[..l].to_vec();
            np.extend(sub);
            np.extend_from_slice(&pts[r + 1..]);
            np.dedup_by(|a, b| (*a - *b).len2() < 1.0);
            pts = np;
        }
        None
    }
}

impl Drawer<'_> {
    /// Drawn wires (other than the rejected one) whose copper on the layer of `d` lies in the
    /// sleeve of its rejected run: the candidates to rip up and redraw after it (feedback,
    /// plan S2c item 4). O(sleeve walls x drawn segments).
    pub fn blockers<'w>(&self, d: &super::draw::Diag, drawn: impl Iterator<Item = (WireId, &'w super::draw::WireOut)>) -> Vec<WireId> {
        let strip = self.strip(d.wire, d.k0, d.k1);
        let mut out = Vec::new();
        for (w, wo) in drawn {
            if w == d.wire {
                continue;
            }
            let hit = wo.runs.iter().filter(|(l, _)| *l == d.layer).any(|(_, pl)| pl.windows(2).any(|p| strip.touches(p[0], p[1])));
            if hit {
                out.push(w);
            }
        }
        out
    }
}

impl Drawer<'_> {
    /// Extra via sites for the via interval `ka..=kb` of wire `w` (diameter `d`, clearance `s`): the
    /// offset corners (convex corners of pads and shapes, ends of copper drawn so far) at the
    /// via's own radius that lie in the sleeve of the interval or its pockets, nearest to the
    /// wire's nominal polyline first, at most `MAX_VIA_SITES`. Each is `(traversal, point)`. The
    /// caller still runs the checker on the via. O(ents near x samples + sleeve triangles).
    pub(super) fn via_extra(&self, w: WireId, ka: usize, kb: usize, d: f64, s: f64) -> Vec<(usize, Vec2)> {
        let wi = &self.st.wires[w];
        let wp = Wp { wire: w, net: wi.net, w: d, s, layer: usize::MAX, own: [wi.pa, wi.pb], neck: None, relax: 0.0 };
        let strip = self.strip(w, ka, kb.max(ka));
        let mut bb = strip.boxes[0];
        for b in &strip.boxes {
            bb = bb.union(b);
        }
        let ents = self.local_ents(&wp, bb.grow(self.qgrow + d));
        let mut cand: Vec<Vec2> = Vec::new();
        for &id in &ents {
            self.corner_nodes(id, &wp, &bb, &mut cand);
        }
        let pr = &self.b.proj;
        let np = self.polys[w].len();
        let nominal: Vec<Vec2> = self.polys[w][ka.min(np - 1)..=(kb + 2).min(np - 1)].to_vec();
        let near = |p: Vec2| nominal.windows(2).map(|x| super::draw::seg_dist(p, x[0], x[1])).fold(f64::INFINITY, f64::min);
        let mut out: Vec<(f64, usize, Vec2)> = Vec::new();
        for p in cand {
            if !strip.contains(p) || !self.seg_clear(&ents, &wp, p, p) {
                continue;
            }
            // Traversal: the triangle of the interval nearest to the point.
            let k = (ka..=kb.max(ka))
                .min_by(|&x, &y| {
                    let c = |k: usize| {
                        let t = wi.tris[k];
                        let c = pr.tris[t].map(|v| pr.pos[v]);
                        (p - (c[0] + c[1] + c[2]) * (1.0 / 3.0)).len()
                    };
                    c(x).total_cmp(&c(y))
                })
                .unwrap_or(ka);
            out.push((near(p), k, p));
        }
        out.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        out.drain(MAX_VIA_SITES.min(out.len())..);
        out.into_iter().map(|(_, k, p)| (k, p)).collect()
    }
}

/// Most extra via sites tried per via.
const MAX_VIA_SITES: usize = 60;

#[cfg(test)]
mod tests {
    //! F6: the indexed / memoised clearance searches against their brute-force originals on a
    //! random pad field (rect, rounded, round and oval pads at any angle, three nets, drawn copper).
    use super::*;
    use crate::check::Checker;
    use crate::model::{BoardModel, Options};
    use crate::topo::board::{self, BuildOpts};
    use crate::topo::draw::{track_item, via_item};
    use crate::topo::init::InitWeights;
    use crate::topo::state::State;
    use crate::vias::ViaSize;
    use std::collections::BTreeMap;

    const MM: f64 = 1e6;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }
        fn f(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
        fn int(&mut self, lo: i64, hi: i64) -> i64 {
            lo + (self.next() % (hi - lo + 1) as u64) as i64
        }
    }

    /// A 24 x 24 mm two-layer board: net 1's end pads at (2, 12) and (22, 12), a 6 x 6 field of
    /// jittered, rotated pads of nets 1-3 between them.
    fn field(g: &mut Rng) -> BoardModel {
        let p = |v: f64| (v * MM) as i64;
        let mut pads: Vec<String> = Vec::new();
        let mut pad = |net: i32, x: f64, y: f64, rot: f64, sh: String| {
            let i = pads.len();
            pads.push(format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"smd","center":[{},{}],"rotation_deg":{rot},"layers":[0],"shapes":{{"0":{sh}}},"drill":null}}"#,
                p(x),
                p(y)
            ));
        };
        pad(1, 2.0, 12.0, 0.0, format!(r#"{{"kind":"rect","w":{0},"h":{0}}}"#, p(0.6)));
        pad(1, 22.0, 12.0, 0.0, format!(r#"{{"kind":"rect","w":{0},"h":{0}}}"#, p(0.6)));
        for i in 0..6 {
            for j in 0..6 {
                let (x, y) = (5.0 + 2.8 * i as f64 + 0.6 * g.f(), 4.0 + 2.8 * j as f64 + 0.6 * g.f());
                let (w, h) = (0.3 + 1.1 * g.f(), 0.3 + 1.1 * g.f());
                let sh = match g.int(0, 3) {
                    0 => format!(r#"{{"kind":"rect","w":{},"h":{}}}"#, p(w), p(h)),
                    1 => format!(r#"{{"kind":"roundrect","w":{},"h":{},"rc":{}}}"#, p(w), p(h), p(0.25 * w.min(h))),
                    2 => format!(r#"{{"kind":"circle","d":{}}}"#, p(w)),
                    _ => format!(r#"{{"kind":"oval","w":{},"h":{}}}"#, p(w), p(h)),
                };
                let rot = [0.0, 45.0, 30.0, 90.0, 17.0][g.int(0, 4) as usize];
                pad(g.int(1, 3) as i32, x, y, rot, sh);
            }
        }
        let c = [(0.0, 0.0), (24.0, 0.0), (24.0, 24.0), (0.0, 24.0)];
        let segs: Vec<String> = (0..4).map(|i| format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(c[i].0), p(c[i].1), p(c[(i + 1) % 4].0), p(c[(i + 1) % 4].1))).collect();
        let s = format!(
            r#"{{"units":"nm","board":"t","layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],"nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}},{{"code":3,"name":"C"}}],"pads":[{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
            pads.join(","),
            segs.join(",")
        );
        BoardModel::from_json(&s).unwrap()
    }

    /// The brute-force clearance test of before F6: every object, its box from its points.
    fn seg_clear_ref(dr: &Drawer, ents: &[usize], wp: &Wp, a: Vec2, b: Vec2) -> bool {
        let one = |u: Vec2, v: Vec2, sw: f64| {
            let bb = bbox_of(&[u, v]);
            ents.iter().all(|&i| {
                let e = &dr.ents[i];
                let need = dr.need_of(i, wp, sw);
                let (lo, hi) = match &e.core {
                    Core::Poly(p) => {
                        let eb = bbox_of(p);
                        (eb.min, eb.max)
                    }
                    Core::Seg(c, d) => (Vec2 { x: c.x.min(d.x), y: c.y.min(d.y) }, Vec2 { x: c.x.max(d.x), y: c.y.max(d.y) }),
                    Core::Pt(c) => (*c, *c),
                };
                if bb.min.x > hi.x + need || bb.max.x < lo.x - need || bb.min.y > hi.y + need || bb.max.y < lo.y - need {
                    return true;
                }
                seg_core(u, v, &e.core).0 >= need - TOL
            })
        };
        if wp.neck.is_some() {
            return wp.pieces(a, b).into_iter().all(|(u, v, w)| one(u, v, dr.seg_width(wp.wire, wp.layer, u, v, w)));
        }
        one(a, b, dr.seg_width(wp.wire, wp.layer, a, b, wp.w))
    }

    fn pt(g: &mut Rng) -> Vec2 {
        Vec2 { x: (1.0 + 22.0 * g.f()) * MM, y: -(1.0 + 22.0 * g.f()) * MM }
    }

    /// Run `f` on a few random fields, each with one routed net-1 wire (wire 0) and random
    /// drawn copper of nets 2 and 3 committed.
    fn with_fields(f: &mut dyn FnMut(&Drawer, &mut Rng)) {
        with_fields_opts(&Options::default(), f)
    }

    /// [`with_fields`] with the given options.
    fn with_fields_opts(opts0: &Options, f: &mut dyn FnMut(&Drawer, &mut Rng)) {
        for seed in 1..=4u64 {
            let mut g = Rng(0x51_7c_c1_b7_27_22_0a_95 ^ seed);
            let m = field(&mut g);
            let b = board::build(&m, &BuildOpts::default()).unwrap();
            let conns = b.connections(&m);
            let mut st = State::new(&b.proj);
            let c = conns.iter().find(|c| c.w > 0.0 && c.net == 1).expect("a net-1 connection");
            st.route_conn(c, &InitWeights::default(), (c.w + c.s) / 400_000.0).expect("routes");
            let opts = opts0.clone();
            let checker = Checker::new(&m, &opts);
            let mut dr = Drawer::new(&m, &opts, &b, &st, &checker);
            for _ in 0..40 {
                let (a, e) = (pt(&mut g), pt(&mut g));
                let e = if g.int(0, 1) == 0 { a + (e - a) * 0.15 } else { a };
                let net = g.int(2, 3) as i32;
                if g.int(0, 4) == 0 {
                    dr.commit(&via_item(net, a, ViaSize { d: 600_000, drill: 300_000 }, None));
                } else {
                    dr.commit(&track_item(net, [0u32, 2][g.int(0, 1) as usize], 150_000.0 + 200_000.0 * g.f(), a, e, None));
                }
            }
            f(&dr, &mut g);
        }
    }

    fn rand_wp(dr: &Drawer, g: &mut Rng) -> Wp {
        let wi = &dr.st.wires[0];
        let neck = (g.int(0, 2) == 0).then(|| {
            let mut ends = [None; NECK_ZONES];
            ends[0] = Some((pt(g), 3.0 * MM * g.f()));
            ends[1] = Some((pt(g), 3.0 * MM * g.f()));
            Neck { min_w: 100_000.0, ends }
        });
        let relax = [0.0, 0.0, 20_000.0, 1e9][g.int(0, 3) as usize];
        Wp { wire: 0, net: g.int(1, 3) as i32, w: 100_000.0 + 400_000.0 * g.f(), s: 100_000.0 + 200_000.0 * g.f(), layer: g.int(0, 1) as usize, own: [wi.pa, wi.pb], neck, relax }
    }

    /// nd-perf F2: the verdict without the price (`via_in_class`) is `via_reach`'s; the call sites'
    /// `via_reach_for_price` is `via_reach` itself with `[vias] reach_price` on and (the verdict, 0)
    /// with it off (0 = what `reach_cost` gives then). Random via sites on the routed wire's
    /// traversals of the pad field with drawn copper.
    #[test]
    fn via_reach_verdict_is_the_same_and_priced_only_when_read() {
        if std::env::var_os("TOPO_VIADBG").is_some() {
            return;
        }
        let crossed = crate::topo::draw::ClassCrossings::new();
        let (mut priced, mut out_of_class) = (0usize, 0usize);
        for on in [false, true] {
            let opts = Options { via_reach_price: on, ..Options::default() };
            with_fields_opts(&opts, &mut |dr, g| {
                let n = dr.st.wires[0].passes.len();
                for _ in 0..80 {
                    let (k, p, la, lb) = (g.int(0, n as i64) as usize, pt(g), g.int(0, 1) as usize, g.int(0, 1) as usize);
                    let full = dr.via_reach(0, k, p, la, lb, &crossed);
                    assert_eq!(dr.via_in_class(0, k, p, la, lb, &crossed), full.0);
                    let used = dr.via_reach_for_price(0, k, p, la, lb, &crossed);
                    if on {
                        assert_eq!((used.0, used.1.to_bits()), (full.0, full.1.to_bits()));
                        priced += usize::from(full.1 > 0.0);
                    } else {
                        assert_eq!(used, (full.0, 0.0));
                    }
                    out_of_class += usize::from(!full.0);
                }
            });
        }
        assert!(priced > 20, "only {priced} priced reaches");
        assert!(out_of_class > 0, "no out-of-class site exercised");
    }

    /// nd-perf F1: the violations without probes ([`viols_measured`] with `probe = false`) bound the
    /// measured ones from below: the same groups (kind, depth, place for vias, against, layer), each
    /// length at most the measured one, so the short amount, the total and the price are at most the
    /// measured ones. Random runs of net 1 (one to four segments, any width, either layer, a via
    /// sometimes) across the pad field and the random drawn copper of nets 2 and 3.
    #[test]
    fn unprobed_violations_bound_the_measured_ones() {
        use crate::topo::draw::{viol_key, viols_measured, viols_with, Viol};
        let mut with_viols = 0usize;
        with_fields(&mut |dr, g| {
            for _ in 0..40 {
                let layer = [0u32, 2][g.int(0, 1) as usize];
                let w = 100_000.0 + 400_000.0 * g.f();
                let mut items = Vec::new();
                let mut p = pt(g);
                for _ in 0..g.int(1, 4) {
                    let q = pt(g);
                    items.push(track_item(1, layer, w, p, q, None));
                    p = q;
                }
                if g.int(0, 3) == 0 {
                    items.push(via_item(1, p, ViaSize { d: 600_000, drill: 300_000 }, None));
                }
                let rep = dr.checker.check_against(&dr.idx, &items, &[]);
                let exact = viols_with(dr.model, dr.opts, dr.checker, &dr.idx, &items, &rep);
                let lb = viols_measured(dr.model, dr.opts, dr.checker, &dr.idx, &items, &rep, false);
                with_viols += usize::from(!exact.is_empty());
                assert_eq!(lb.len(), exact.len());
                for (a, b) in lb.iter().zip(&exact) {
                    assert_eq!((a.kind, a.layer, a.via, &a.against, a.depth_nm), (b.kind, b.layer, b.via, &b.against, b.depth_nm));
                    assert!(a.len_nm <= b.len_nm, "{a:?} vs {b:?}");
                    assert!(a.amount() <= b.amount());
                }
                let (kl, ke) = (viol_key(&lb), viol_key(&exact));
                assert!(kl.0 <= ke.0 && kl.1 <= ke.1, "{kl:?} vs {ke:?}");
                let (pl, pe): (f64, f64) = (lb.iter().map(Viol::amount).sum(), exact.iter().map(Viol::amount).sum());
                assert!(pl <= pe);
                // The lazy candidate priced later is the eager one (violations in the same order,
                // the same price bits), and its bound is at or below that key.
                let wire = 0;
                let (p0, mut vs0, lazy) = dr.lazy_cand(wire, &items, &rep);
                // A graze violation (none arise on this field: no minimum connection width) goes
                // after the measured ones, as the eager pricing appended it.
                let mut graze = dr.graze_viols(wire, &items);
                assert_eq!(vs0, graze);
                if g.int(0, 1) == 0 {
                    let fake = Viol { kind: "connection_width", at: p, layer, depth_nm: 1000.0, len_nm: w, against: "own-net pad or via".into(), via: false };
                    vs0.push(fake.clone());
                    graze.push(fake);
                }
                let mut c: crate::topo::draw::ForcedCand = (p0, Vec::new(), items.clone(), vs0, None, "test", lazy);
                let bound = dr.cand_bound(&c);
                dr.price_cand(&mut c);
                let mut eager = dr.viols_of(&items, &rep);
                eager.extend(graze);
                let pe: f64 = eager.iter().map(Viol::amount).sum();
                assert!(c.6.is_none());
                assert_eq!(c.3, eager);
                assert_eq!(c.0.to_bits(), pe.to_bits());
                let ke = crate::topo::draw::forced_key(&eager, pe);
                assert!(bound.0 <= ke.0 && bound.1 <= ke.1 && bound.2 <= ke.2, "{bound:?} vs {ke:?}");
            }
        });
        assert!(with_viols > 40, "only {with_viols} runs with violations");
    }

    /// [`Drawer::seg_clear_near`] (the [`Near`] grid) and [`Drawer::seg_clear`] (the cached
    /// boxes) give the brute-force answer for random and adversarial segments: zero length, along
    /// a pad's box side, and exactly at / just inside / just outside an object's need.
    #[test]
    fn near_and_cached_boxes_match_the_brute_force_clearance_test() {
        let mut n = [0usize; 3];
        with_fields(&mut |dr, g| {
            for _ in 0..60 {
                let wp = rand_wp(dr, g);
                let c = pt(g);
                let half = [0.3, 1.5, 4.0, 12.0, 12.0, 24.0][g.int(0, 5) as usize] * MM;
                let ents = dr.local_ents(&wp, Aabb { min: c - Vec2 { x: half, y: half }, max: c + Vec2 { x: half, y: half } }.grow(dr.qgrow));
                // also a random subset (sets on both sides of NEAR_MIN)
                let ents: Vec<usize> = if g.int(0, 3) == 0 { ents.into_iter().filter(|_| g.int(0, 2) == 0).collect() } else { ents };
                let nr = dr.near(ents.clone(), &wp);
                n[0] += usize::from(!nr.cells.is_empty());
                for k in 0..80 {
                    let a = c + Vec2 { x: (2.0 * g.f() - 1.0) * half, y: (2.0 * g.f() - 1.0) * half };
                    let b = match k % 4 {
                        0 => a,
                        1 => a + Vec2 { x: (g.f() - 0.5) * 2.0 * half, y: 0.0 },
                        _ => c + Vec2 { x: (2.0 * g.f() - 1.0) * half, y: (2.0 * g.f() - 1.0) * half },
                    };
                    let want = seg_clear_ref(dr, &ents, &wp, a, b);
                    assert_eq!(dr.seg_clear(&ents, &wp, a, b), want, "cached boxes: {a:?}-{b:?}");
                    assert_eq!(dr.seg_clear_near(&nr, a, b), want, "near: {a:?}-{b:?} ({} objects)", ents.len());
                    n[1 + usize::from(want)] += 1;
                }
                // At an object's need: a segment parallel to the side of its box at need - 100 nm,
                // need, need + 100 nm (zero length too).
                for &i in ents.iter().take(12) {
                    let bx = dr.ent_box[i];
                    let need = dr.need_of(i, &wp, wp.w);
                    for off in [need - 100.0, need, need + 100.0] {
                        let y = bx.max.y + off;
                        for (a, b) in [(Vec2 { x: bx.min.x, y }, Vec2 { x: bx.max.x, y }), (Vec2 { x: bx.min.x - off, y: bx.min.y }, Vec2 { x: bx.min.x - off, y: bx.min.y })] {
                            let want = seg_clear_ref(dr, &ents, &wp, a, b);
                            assert_eq!(dr.seg_clear(&ents, &wp, a, b), want);
                            assert_eq!(dr.seg_clear_near(&nr, a, b), want, "near at need offset {off}: {a:?}-{b:?}");
                            n[1 + usize::from(want)] += 1;
                        }
                    }
                }
            }
        });
        assert!(n[0] > 60 && n[1] > 2_000 && n[2] > 2_000, "{n:?}");
    }

    /// The repair of before F6 (every segment searched again in every sweep).
    fn repair_ref(dr: &Drawer, net: i32, w: f64, s: f64, layer: usize, mut pts: Vec<Vec2>) -> Vec<Vec2> {
        let mut bumps: BTreeMap<usize, u32> = BTreeMap::new();
        for _ in 0..60 {
            let mut fixes: Vec<(usize, usize, Vec2, Vec2, f64)> = Vec::new();
            for i in 0..pts.len() - 1 {
                let (a, b) = (pts[i], pts[i + 1]);
                let mut ids = dr.grid.query(bbox_of(&[a, b]).grow(dr.qgrow));
                ids.sort_unstable();
                ids.dedup();
                let mut worst: Option<(f64, usize, Vec2, Vec2, f64)> = None;
                for id in ids {
                    let e = &dr.ents[id];
                    if !e.live || (e.mask >> layer) & 1 == 0 || e.net == Some(net) {
                        continue;
                    }
                    let need = e.grow + if e.fixed { e.clear } else { e.clear.max(s) } + 0.5 * w + dr.eps;
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

    /// The memoised repair returns the reference's polyline bit for bit (random polylines across
    /// the pad field, repeated points included).
    #[test]
    fn memoised_repair_matches_the_reference() {
        let mut bumped = 0;
        with_fields(&mut |dr, g| {
            for _ in 0..40 {
                let mut pts: Vec<Vec2> = (0..g.int(2, 7)).map(|_| pt(g)).collect();
                if g.int(0, 3) == 0 {
                    let k = g.int(0, pts.len() as i64 - 1) as usize;
                    pts.insert(k, pts[k]);
                }
                let (net, w, s, layer) = (g.int(1, 3) as i32, 100_000.0 + 300_000.0 * g.f(), 100_000.0 + 200_000.0 * g.f(), g.int(0, 1) as usize);
                let want = repair_ref(dr, net, w, s, layer, pts.clone());
                let got = dr.repair(net, w, s, layer, pts.clone());
                bumped += usize::from(want.len() > pts.len());
                let bits = |v: &[Vec2]| v.iter().map(|p| (p.x.to_bits(), p.y.to_bits())).collect::<Vec<_>>();
                assert_eq!(bits(&got), bits(&want));
            }
        });
        assert!(bumped > 40, "{bumped}");
    }

    /// The A* window search of before F6 (every object tested per edge).
    fn local_path_ref(dr: &Drawer, strip: &Strip, wp: &Wp, a: Vec2, b: Vec2, bb: Aabb) -> Option<Vec<Vec2>> {
        let ents = dr.local_ents(wp, bb.grow(dr.qgrow));
        let mut cand: Vec<Vec2> = Vec::new();
        for &id in &ents {
            dr.corner_nodes(id, wp, &bb, &mut cand);
        }
        let mut nodes: Vec<Vec2> = vec![a, b];
        for p in cand {
            if nodes.len() > MAX_NODES {
                return None;
            }
            if p.x < bb.min.x || p.x > bb.max.x || p.y < bb.min.y || p.y > bb.max.y {
                continue;
            }
            if strip.contains(p) && seg_clear_ref(dr, &ents, wp, p, p) {
                nodes.push(p);
            }
        }
        let n = nodes.len();
        let (mut gs, mut from, mut done) = (vec![f64::INFINITY; n], vec![usize::MAX; n], vec![false; n]);
        let mut heap = BinaryHeap::new();
        gs[0] = 0.0;
        heap.push(Open { f: (b - a).len(), n: 0 });
        while let Some(Open { n: u, .. }) = heap.pop() {
            if done[u] {
                continue;
            }
            done[u] = true;
            if u == 1 {
                break;
            }
            for v in 0..n {
                if done[v] || v == u {
                    continue;
                }
                let d = (nodes[v] - nodes[u]).len();
                if gs[u] + d >= gs[v] {
                    continue;
                }
                let (p, q) = (nodes[u], nodes[v]);
                if strip.crosses_wall(p, q) || !strip.contains((p + q) * 0.5) || !seg_clear_ref(dr, &ents, wp, p, q) {
                    continue;
                }
                gs[v] = gs[u] + d;
                from[v] = u;
                heap.push(Open { f: gs[v] + (b - nodes[v]).len(), n: v });
            }
        }
        if !done[1] {
            return None;
        }
        let mut path = vec![b];
        let mut c = 1;
        while from[c] != usize::MAX {
            c = from[c];
            path.push(nodes[c]);
        }
        path.reverse();
        Some(path)
    }

    /// `local_path` (with [`Near`]) returns the reference's path bit for bit, for windows round
    /// random stretches of the routed wire's sleeve.
    #[test]
    fn local_path_with_near_matches_the_reference() {
        let mut found = 0;
        with_fields(&mut |dr, g| {
            let wi = &dr.st.wires[0];
            let k1 = wi.passes.len();
            let strip = dr.strip(0, 0, k1);
            let poly = dr.st.polyline(0);
            for _ in 0..12 {
                let mut wp = rand_wp(dr, g);
                wp.net = wi.net;
                let i = g.int(0, poly.len() as i64 - 2) as usize;
                let j = g.int(i as i64 + 1, poly.len() as i64 - 1) as usize;
                let (a, b) = (poly[i], poly[j]);
                let bb = bbox_of(&poly[i..=j]).grow([0.5, 2.0][g.int(0, 1) as usize] * MM);
                let want = dr.local_path(&strip, &wp, a, b, bb);
                let rf = local_path_ref(dr, &strip, &wp, a, b, bb);
                found += usize::from(rf.is_some());
                let bits = |v: &Option<Vec<Vec2>>| v.as_ref().map(|v| v.iter().map(|p| (p.x.to_bits(), p.y.to_bits())).collect::<Vec<_>>());
                assert_eq!(bits(&want), bits(&rf));
            }
        });
        assert!(found > 5, "{found}");
    }

    /// `exact_fix_relaxed` with its segment memo returns, for random funnel-like polylines of the
    /// routed wire, the same polyline as the brute-force loop over its parts (the reference below
    /// re-tests every segment every round).
    #[test]
    fn exact_fix_memo_matches_the_reference_loop() {
        let mut fixed = 0;
        with_fields(&mut |dr, g| {
            let wi = &dr.st.wires[0];
            let k1 = wi.passes.len();
            let poly = dr.st.polyline(0);
            for _ in 0..10 {
                let mut pts = poly.clone();
                for p in pts.iter_mut().skip(1).take(poly.len().saturating_sub(2)) {
                    *p = *p + Vec2 { x: (g.f() - 0.5) * 1.0 * MM, y: (g.f() - 0.5) * 1.0 * MM };
                }
                let layer = g.int(0, 1) as usize;
                let relax = [0.0, 30_000.0][g.int(0, 1) as usize];
                let got = dr.exact_fix_relaxed(0, 0, k1, layer, pts.clone(), None, relax);
                let want = exact_fix_ref(dr, 0, 0, k1, layer, pts.clone(), relax);
                fixed += usize::from(want.is_some());
                let bits = |v: &Option<Vec<Vec2>>| v.as_ref().map(|v| v.iter().map(|p| (p.x.to_bits(), p.y.to_bits())).collect::<Vec<_>>());
                assert_eq!(bits(&got), bits(&want));
            }
        });
        assert!(fixed > 4, "{fixed}");
    }

    fn exact_fix_ref(dr: &Drawer, w: WireId, k0: usize, k1: usize, layer: usize, mut pts: Vec<Vec2>, relax: f64) -> Option<Vec<Vec2>> {
        let wi = &dr.st.wires[w];
        let wp = Wp { wire: w, net: wi.net, w: wi.w, s: wi.s, layer, own: [wi.pa, wi.pb], neck: None, relax };
        let mut strip = dr.strip(w, k0, k1);
        let n_walls = strip.walls.len();
        strip.walls.extend(dr.via_walls(w, k0, k1, layer));
        let via_wall = |a: Vec2, b: Vec2| strip.walls[n_walls..].iter().any(|&(c, d)| super::super::draw::seg_proper(a, b, c, d));
        for _ in 0..24 {
            let mut bad = None;
            for i in 0..pts.len() - 1 {
                let ents = dr.local_ents(&wp, bbox_of(&[pts[i], pts[i + 1]]).grow(dr.qgrow));
                if !seg_clear_ref(dr, &ents, &wp, pts[i], pts[i + 1]) || via_wall(pts[i], pts[i + 1]) {
                    bad = Some(i);
                    break;
                }
            }
            let Some(i) = bad else { return Some(dr.drop_backtracks(&strip, &wp, pts)) };
            let (mut l, mut r) = (if via_wall(pts[i], pts[i + 1]) { i.saturating_sub(1) } else { i }, i + 1);
            while l > 0 && !dr.vertex_ok(&strip, &wp, pts[l]) {
                l -= 1;
            }
            while r < pts.len() - 1 && !dr.vertex_ok(&strip, &wp, pts[r]) {
                r += 1;
            }
            let base = bbox_of(&pts[l..=r]);
            let mut sub = None;
            for m in [2.0e6, 8.0e6] {
                sub = local_path_ref(dr, &strip, &wp, pts[l], pts[r], base.grow(m));
                if sub.is_some() {
                    break;
                }
            }
            let sub = sub?;
            let mut np: Vec<Vec2> = pts[..l].to_vec();
            np.extend(sub);
            np.extend_from_slice(&pts[r + 1..]);
            np.dedup_by(|a, b| (*a - *b).len2() < 1.0);
            pts = np;
        }
        None
    }

    /// `Strip::contains` before the triangle grid: every triangle, box test first.
    fn contains_ref(s: &Strip, p: Vec2) -> bool {
        s.tris.iter().zip(&s.boxes).any(|(t, bb)| {
            if p.x < bb.min.x - 1.0 || p.x > bb.max.x + 1.0 || p.y < bb.min.y - 1.0 || p.y > bb.max.y + 1.0 {
                return false;
            }
            let d = |a: Vec2, b: Vec2| (b - a).cross(p - a);
            let (d1, d2, d3) = (d(t[0], t[1]), d(t[1], t[2]), d(t[2], t[0]));
            let tol = -2.0 * (t[1] - t[0]).len().max((t[2] - t[1]).len()).max(1.0);
            d1 >= tol && d2 >= tol && d3 >= tol
        })
    }

    /// The triangle grid of [`Strip`] answers `contains` like the scan of every triangle, on
    /// random fans and strips (shared vertices, zero-area and needle triangles, triangles of a
    /// few nm and of many mm) and query points on vertices, on edges, at +-1 / +-2 nm of the
    /// boxes and far away.
    #[test]
    fn strip_grid_matches_the_scan_of_every_triangle() {
        let mut g = Rng(0x9e37_79b9_7f4a_7c15);
        let (mut hits, mut grids) = (0usize, 0usize);
        for case in 0..300 {
            let n = [3usize, 15, 16, 17, 40, 120, 400][case % 7];
            let scale = [5.0, 1e4, 1e6, 2e7][g.int(0, 3) as usize];
            let mut pts: Vec<Vec2> = Vec::new();
            let mut tris: Vec<[Vec2; 3]> = Vec::new();
            let o = Vec2 { x: 1e8 * g.f(), y: -1e8 * g.f() };
            for k in 0..n {
                let rnd = |g: &mut Rng| o + Vec2 { x: (g.f() * 30.0).round() * scale, y: (g.f() * 30.0).round() * scale };
                let t = match g.int(0, 4) {
                    // A strip: shares an edge with the last triangle.
                    0 | 1 if k > 0 => {
                        let l = tris[k - 1];
                        [l[1], l[2], rnd(&mut g)]
                    }
                    // Zero area: three points on a line.
                    2 => {
                        let (a, b) = (rnd(&mut g), rnd(&mut g));
                        [a, b, a + (b - a) * 0.5]
                    }
                    // A needle / a point triangle.
                    3 => {
                        let a = rnd(&mut g);
                        [a, a, a + Vec2 { x: scale * g.f(), y: 0.0 }]
                    }
                    _ => [rnd(&mut g), rnd(&mut g), rnd(&mut g)],
                };
                pts.extend(t);
                tris.push(t);
            }
            let s = Strip::new(tris, Vec::new());
            grids += usize::from(!s.grid.cells.is_empty());
            for _ in 0..400 {
                let p = match g.int(0, 5) {
                    0 => pts[g.int(0, pts.len() as i64 - 1) as usize],
                    1 => {
                        let t = s.tris[g.int(0, s.tris.len() as i64 - 1) as usize];
                        let (a, b) = (t[g.int(0, 2) as usize], t[g.int(0, 2) as usize]);
                        a + (b - a) * g.f()
                    }
                    2 => {
                        let bb = s.boxes[g.int(0, s.boxes.len() as i64 - 1) as usize];
                        let d = [-2.0, -1.0, -0.5, 0.0, 0.5, 1.0, 1.5, 2.0, 2.5][g.int(0, 8) as usize];
                        let (x, y) = if g.int(0, 1) == 0 { (bb.min.x - d, bb.min.y + (bb.max.y - bb.min.y) * g.f()) } else { (bb.max.x + d, bb.max.y + d) };
                        Vec2 { x, y }
                    }
                    3 => o + Vec2 { x: (g.f() * 34.0 - 2.0) * scale, y: (g.f() * 34.0 - 2.0) * scale },
                    4 => o + Vec2 { x: (g.f() - 0.5) * 1e9, y: (g.f() - 0.5) * 1e9 },
                    _ => {
                        let t = s.tris[g.int(0, s.tris.len() as i64 - 1) as usize];
                        (t[0] + t[1] + t[2]) * (1.0 / 3.0) + Vec2 { x: g.f() * 4.0 - 2.0, y: g.f() * 4.0 - 2.0 }
                    }
                };
                let want = contains_ref(&s, p);
                hits += usize::from(want);
                assert_eq!(s.contains(p), want, "case {case} p {p:?}");
            }
        }
        assert!(grids > 150 && hits > 20_000, "{grids} {hits}");
    }

    /// Fields like [`with_fields`], the drawn copper of nets 2 and 3 octilinear (0 / 45 / 90
    /// degree tracks, the bundle neighbours `follow` prices) and any angle.
    fn with_octi_fields(f: &mut dyn FnMut(&mut Drawer, &mut Rng)) {
        for seed in 1..=4u64 {
            let mut g = Rng(0x2f_1c_93_aa_51_08_6d_e3 ^ seed);
            let m = field(&mut g);
            let b = board::build(&m, &BuildOpts::default()).unwrap();
            let conns = b.connections(&m);
            let mut st = State::new(&b.proj);
            let c = conns.iter().find(|c| c.w > 0.0 && c.net == 1).expect("a net-1 connection");
            st.route_conn(c, &InitWeights::default(), (c.w + c.s) / 400_000.0).expect("routes");
            // Further wires, not drawn: lane neighbours of wire 0 (the planned-room zones and
            // the lane-slot gates that `room` prices).
            for c in conns.iter().filter(|c| c.w > 0.0 && c.net != 1) {
                let _ = st.route_conn(c, &InitWeights::default(), (c.w + c.s) / 400_000.0);
            }
            let opts = Options::default();
            let checker = Checker::new(&m, &opts);
            let mut dr = Drawer::new(&m, &opts, &b, &st, &checker);
            dr.compute_intervals();
            for _ in 0..40 {
                let a = pt(&mut g);
                let e = match g.int(0, 2) {
                    0 => pt(&mut g),
                    _ => a + super::super::octi::dir(g.int(0, 7) as usize) * (0.5 + 4.0 * g.f()) * MM,
                };
                let net = g.int(2, 3) as i32;
                dr.commit(&track_item(net, [0u32, 2][g.int(0, 1) as usize], 150_000.0 + 200_000.0 * g.f(), a, e, None));
            }
            f(&mut dr, &mut g);
        }
    }

    /// `octilinear` with its per-pair shape cache returns the same run as the DP loop that builds
    /// and prices every connector shape for every arrival state (`OCTI_REF`), on random any-angle
    /// runs of the routed wire with and without pad exits.
    #[test]
    fn octilinear_shape_cache_matches_the_reference_loop() {
        use super::super::octi::{OCTI_HITS, OCTI_LANE_MIN, OCTI_NODE_SPLIT, OCTI_REF};
        // per lane count: nodes done alone and on the crew (lanes 2, 4, 8 must do every node on the crew)
        let mut split = [[0usize; 2]; 4];
        let (mut some, mut runs) = (0, 0);
        with_octi_fields(&mut |dr, g| {
            let dr: &Drawer = dr;
            let wi = &dr.st.wires[0];
            let k1 = wi.passes.len();
            let poly = dr.st.polyline(0);
            for _ in 0..10 {
                let mut pts = poly.clone();
                for p in pts.iter_mut().skip(1).take(poly.len().saturating_sub(2)) {
                    *p = *p + Vec2 { x: (g.f() - 0.5) * 0.4 * MM, y: (g.f() - 0.5) * 0.4 * MM };
                }
                let exits = match g.int(0, 2) {
                    0 => [None, None],
                    _ => [dr.pad_exit(0, 0, pts[1]), dr.pad_exit(0, 1, pts[pts.len() - 2])],
                };
                let layer = g.int(0, 1) as usize;
                let forced = [None, Some(dr.octi.viol_nm), Some(dr.octi.short_nm)][g.int(0, 2) as usize];
                OCTI_REF.with(|f| f.set(false));
                let got = dr.octilinear(0, 0, k1, layer, &pts, None, exits, forced);
                // par-draw: every node's successors on 1/2/4/8 lanes (pair rule 1, the reverse
                // hook: the first successors finish last). The test switch and the node counts are
                // thread-locals of the thread that calls `octilinear`: the pool's worker.
                let laned: Vec<_> = [1usize, 2, 4, 8]
                    .iter()
                    .enumerate()
                    .map(|(ki, &k)| {
                        let pool = rayon::ThreadPoolBuilder::new().num_threads(k).build().expect("pool");
                        pool.install(|| {
                            OCTI_LANE_MIN.with(|f| f.set(1));
                            OCTI_NODE_SPLIT.with(|c| c.set([0, 0]));
                            let r = super::super::par::with_mode(Some(super::super::refine_par::lanes::Mode::Fixed(k)), true, || dr.octilinear(0, 0, k1, layer, &pts, None, exits, forced));
                            let c = OCTI_NODE_SPLIT.with(|c| c.get());
                            OCTI_LANE_MIN.with(|f| f.set(0));
                            split[ki] = [split[ki][0] + c[0], split[ki][1] + c[1]];
                            r
                        })
                    })
                    .collect();
                OCTI_REF.with(|f| f.set(true));
                let want = dr.octilinear(0, 0, k1, layer, &pts, None, exits, forced);
                OCTI_REF.with(|f| f.set(false));
                runs += 1;
                some += usize::from(want.is_some());
                let bits = |v: &Option<super::super::octi::OctiOut>| v.as_ref().map(|o| o.pts.iter().map(|p| (p.x.to_bits(), p.y.to_bits())).collect::<Vec<_>>());
                assert_eq!(bits(&got), bits(&want));
                for (k, l) in [1usize, 2, 4, 8].iter().zip(&laned) {
                    assert_eq!(bits(l), bits(&want), "{k} lanes");
                }
            }
        });
        let hits = OCTI_HITS.with(|h| h.get());
        assert!(some > 10 && hits[0] > 20, "{some} of {runs}, hits {hits:?}");
        // one lane: every node alone; 2, 4, 8 lanes: every node on the crew
        assert!(split[0][0] > 0 && split[0][1] == 0, "1 lane: {:?}", split[0]);
        for (ki, k) in [(1usize, 2usize), (2, 4), (3, 8)] {
            assert!(split[ki][1] > 0 && split[ki][0] == 0, "{k} lanes: nodes alone / on the crew {:?}", split[ki]);
        }
    }

    /// forced-draw (T1): [`band_len`] against a slow sampling reference (20 000 points along the
    /// segment): point, segment and convex polygon cores, zero-length and far segments, segments
    /// through the core and along it.
    #[test]
    fn band_len_matches_a_sampling_reference() {
        let mut g = Rng(0x7a_31_5c_02_e9_44_b1_0f);
        let (mut hit, mut zero) = (0usize, 0usize);
        for case in 0..3000 {
            let c = Vec2 { x: (g.f() - 0.5) * 4.0 * MM, y: (g.f() - 0.5) * 4.0 * MM };
            let core = match case % 3 {
                0 => Core::Pt(c),
                1 => Core::Seg(c, c + Vec2 { x: (g.f() - 0.5) * 3.0 * MM, y: (g.f() - 0.5) * 3.0 * MM }),
                _ => {
                    // a rotated rectangle (a pad)
                    let (hx, hy, t) = (0.1 * MM + g.f() * MM, 0.1 * MM + g.f() * MM, g.f() * 3.2);
                    let (u, v) = (Vec2 { x: t.cos(), y: t.sin() }, Vec2 { x: -t.sin(), y: t.cos() });
                    Core::Poly(vec![c + u * hx + v * hy, c - u * hx + v * hy, c - u * hx - v * hy, c + u * hx - v * hy])
                }
            };
            let a = Vec2 { x: (g.f() - 0.5) * 6.0 * MM, y: (g.f() - 0.5) * 6.0 * MM };
            let b = match g.int(0, 4) {
                0 => a,
                1 => c + Vec2 { x: (g.f() - 0.5) * 0.2 * MM, y: (g.f() - 0.5) * 0.2 * MM },
                2 => Vec2 { x: a.x + (g.f() - 0.5) * 8.0 * MM, y: a.y },
                _ => Vec2 { x: (g.f() - 0.5) * 6.0 * MM, y: (g.f() - 0.5) * 6.0 * MM },
            };
            let need = 0.05 * MM + g.f() * 1.2 * MM;
            let got = band_len(a, b, &core, need);
            let (l, n) = ((b - a).len(), 20_000usize);
            let inside = (0..n).filter(|&k| {
                let p = a + (b - a) * ((k as f64 + 0.5) / n as f64);
                seg_core(p, p, &core).0 < need
            }).count();
            let want = if l < 1.0 { 0.0 } else { l * inside as f64 / n as f64 };
            assert!((got - want).abs() <= 2.0 * l / n as f64 + 1.0, "case {case}: {got} vs {want} (len {l}, need {need})");
            if want > 0.0 { hit += 1 } else { zero += 1 }
        }
        assert!(hit > 500 && zero > 500, "{hit} {zero}");
    }

    /// forced-draw (T1b): [`Drawer::seg_deficit_near`] is 0 exactly where
    /// [`Drawer::seg_clear_near`] holds, and otherwise the brute-force sum over the objects of
    /// `violation_amount(need - distance, band_len)` (no box filter, no grid; neck pieces too);
    /// [`Drawer::seg_deficit_parts_near`] puts an object's amount in the short part exactly when
    /// the copper overlaps foreign copper that is not a hole, edge or keepout.
    #[test]
    fn seg_deficit_is_the_graded_clearance_test() {
        let mut n = [0usize; 3];
        with_fields(&mut |dr, g| {
            for _ in 0..60 {
                let wp = rand_wp(dr, g);
                let c = pt(g);
                let half = [0.3, 1.5, 4.0, 12.0][g.int(0, 3) as usize] * MM;
                let ents = dr.local_ents(&wp, Aabb { min: c - Vec2 { x: half, y: half }, max: c + Vec2 { x: half, y: half } }.grow(dr.qgrow));
                let nr = dr.near(ents.clone(), &wp);
                let one = |u: Vec2, v: Vec2, sw: f64| -> [f64; 2] {
                    ents.iter().fold([0.0; 2], |t, &i| {
                        let e = &dr.ents[i];
                        let need = dr.need_of(i, &wp, sw);
                        let d = seg_core(u, v, &e.core).0;
                        if d >= need - TOL {
                            return t;
                        }
                        let x = crate::topo::factors::violation_amount(need - d, band_len(u, v, &e.core, need));
                        // copper over foreign copper (not a hole, edge or keepout): a short
                        if !e.fixed && e.net != Some(wp.net) && d < e.grow + 0.5 * sw {
                            [t[0], t[1] + x]
                        } else {
                            [t[0] + x, t[1]]
                        }
                    })
                };
                for k in 0..80 {
                    let a = c + Vec2 { x: (2.0 * g.f() - 1.0) * half, y: (2.0 * g.f() - 1.0) * half };
                    let b = if k % 5 == 0 { a } else { c + Vec2 { x: (2.0 * g.f() - 1.0) * half, y: (2.0 * g.f() - 1.0) * half } };
                    let want: [f64; 2] = if wp.neck.is_some() {
                        wp.pieces(a, b).into_iter().fold([0.0; 2], |t, (u, v, w)| {
                            let x = one(u, v, dr.seg_width(wp.wire, wp.layer, u, v, w));
                            [t[0] + x[0], t[1] + x[1]]
                        })
                    } else {
                        one(a, b, dr.seg_width(wp.wire, wp.layer, a, b, wp.w))
                    };
                    let got = dr.seg_deficit_near(&nr, a, b);
                    let parts = dr.seg_deficit_parts_near(&nr, a, b);
                    assert_eq!(got == 0.0, dr.seg_clear_near(&nr, a, b), "{a:?}-{b:?}: deficit {got}");
                    assert!((got - want[0] - want[1]).abs() <= 1e-9 * got.max(1.0), "{a:?}-{b:?}: {got} vs {want:?}");
                    for q in 0..2 {
                        assert!((parts[q] - want[q]).abs() <= 1e-9 * want[q].max(1.0), "{a:?}-{b:?}: parts {parts:?} vs {want:?}");
                    }
                    n[usize::from(got > 0.0)] += 1;
                    n[2] += usize::from(parts[1] > 0.0);
                }
            }
        });
        assert!(n[0] > 2_000 && n[1] > 2_000 && n[2] > 100, "{n:?}");
    }

    /// forced-draw (T3): the forced octilinear DP prices the clearance. Pricing a violation can
    /// only lower the violation the DP chooses (exchange argument over the same chains), so over
    /// random forced runs the chosen forms' summed violation amount with the `violation` weight
    /// is at most the one with weight 0, and below it in some runs; the forced DP takes legs
    /// below the rule where that is cheapest (the clearance is a price, not a test).
    #[test]
    fn forced_octilinear_prices_the_clearance() {
        let (mut tot, mut tot0, mut lower, mut below, mut runs) = (0.0, 0.0, 0usize, 0usize, 0usize);
        with_octi_fields(&mut |dr, g| {
            let k1 = dr.st.wires[0].passes.len();
            let poly = dr.st.polyline(0);
            for _ in 0..10 {
                let mut pts = poly.clone();
                for p in pts.iter_mut().skip(1).take(poly.len().saturating_sub(2)) {
                    *p = *p + Vec2 { x: (g.f() - 0.5) * 0.6 * MM, y: (g.f() - 0.5) * 0.6 * MM };
                }
                let layer = g.int(0, 1) as usize;
                let wi = &dr.st.wires[0];
                let wp = Wp { wire: 0, net: wi.net, w: wi.w, s: wi.s, layer, own: [wi.pa, wi.pb], neck: None, relax: 0.0 };
                let region = bbox_of(&pts).grow(2.0 * MM + 4.0 * (wi.w + wi.s));
                let form = |dr: &Drawer| dr.octilinear(0, 0, k1, layer, &pts, None, [None, None], Some(dr.octi.viol_nm)).map_or(pts.clone(), |o| o.pts);
                let f1 = form(dr);
                let keep = dr.octi;
                dr.octi.viol_nm = 0.0;
                let f0 = form(dr);
                dr.octi = keep;
                let dr: &Drawer = dr;
                let nr = dr.near(dr.local_ents(&wp, region.grow(dr.qgrow)), &wp);
                let deficit = |q: &[Vec2]| -> f64 { q.windows(2).map(|x| dr.seg_deficit_near(&nr, x[0], x[1])).sum() };
                let (d1, d0) = (deficit(&f1), deficit(&f0));
                tot += d1;
                tot0 += d0;
                lower += usize::from(d1 < d0 - 1e-9);
                below += usize::from(f1.windows(2).any(|x| super::super::octi::octi_dir(x[1] - x[0]).is_some() && dr.seg_deficit_near(&nr, x[0], x[1]) > 0.0));
                runs += 1;
            }
        });
        assert!(tot <= tot0 + 1e-9 && lower > 3 && below > 0, "priced {tot} vs free {tot0}, lower in {lower} and octilinear legs below the rule in {below} of {runs} runs");
    }

    /// A two-pad board (net 1, A and B, KiCad frame mm) with wire 0 routed between them, the
    /// drawer built with the octilinear style on; `f` gets it (math frame: y up).
    fn two_pads(a: (f64, f64), b: (f64, f64), f: &mut dyn FnMut(&mut Drawer)) {
        let p = |v: f64| (v * MM) as i64;
        let pad = |i: usize, x: f64, y: f64| format!(r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":1,"attr":"smd","center":[{x},{y}],"rotation_deg":0,"layers":[0],"shapes":{{"0":{{"kind":"rect","w":{s},"h":{s}}}}},"drill":null}}"#, x = p(x), y = p(y), s = p(0.4));
        let c = [(0.0, 0.0), (24.0, 0.0), (24.0, 24.0), (0.0, 24.0)];
        let segs: Vec<String> = (0..4).map(|i| format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(c[i].0), p(c[i].1), p(c[(i + 1) % 4].0), p(c[(i + 1) % 4].1))).collect();
        let js = format!(
            r#"{{"units":"nm","board":"t","layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],"nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}},{{"code":3,"name":"C"}}],"pads":[{},{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
            pad(0, a.0, a.1),
            pad(1, b.0, b.1),
            segs.join(",")
        );
        let m = BoardModel::from_json(&js).unwrap();
        let bd = board::build(&m, &BuildOpts::default()).unwrap();
        let conns = bd.connections(&m);
        let mut st = State::new(&bd.proj);
        let cn = conns.iter().find(|c| c.w > 0.0 && c.net == 1).expect("a net-1 connection");
        st.route_conn(cn, &InitWeights::default(), (cn.w + cn.s) / 400_000.0).expect("routes");
        let opts = Options::default();
        let checker = Checker::new(&m, &opts);
        let mut dr = Drawer::new(&m, &opts, &bd, &st, &checker);
        dr.octi = crate::topo::octi::OctiCfg { on: true, ..dr.octi };
        f(&mut dr);
    }

    /// first-draw-45 r7 (`[drawing] short_leg_mm`, the esp +3V3 staircase): a net-1 run from A
    /// (2, 12) to B (8, 10) mm, the taut chord at 18.4 degrees, in a corridor of two foreign net-2
    /// tracks parallel to it, each `dw` beyond the chord's clearance. Every octilinear form in the
    /// corridor is a staircase whose steps stay within `dw` of the chord: legs far below the jog
    /// leg. Without the short-leg price (weight 0) the first-draw DP takes such a staircase (its
    /// bends and jogs cost less than one free-angle stretch); with it (the default weight) no bend
    /// follows two consecutive legs below the jog leg: the chord stays as drawn (free-angle,
    /// one item) or the steps are long. Returns (such staircase bends at weight 0, at the default).
    fn short_legs_in_corridor(dw: f64) -> (usize, usize) {
        let v = |x: f64, y: f64| Vec2 { x: x * MM, y: -y * MM };
        let (a, b) = (v(2.0, 12.0), v(8.0, 10.0));
        let mut got = (0, 0);
        two_pads((2.0, 12.0), (8.0, 10.0), &mut |dr| {
            let u = (b - a).unit().expect("A != B");
            let left = Vec2 { x: -u.y, y: u.x };
            let wi = dr.st.wires[0].clone();
            let wp = Wp { wire: 0, net: 1, w: wi.w, s: wi.s, layer: 0, own: [wi.pa, wi.pb], neck: None, relax: 0.0 };
            let tw = 150_000.0;
            dr.commit(&track_item(2, 0, tw, a + left * 20.0 * MM, b + left * 20.0 * MM, None));
            let need = dr.need_of(dr.ents.len() - 1, &wp, wi.w);
            for sg in [-1.0, 1.0] {
                dr.commit(&track_item(2, 0, tw, a - u * 1.5 * MM + left * sg * (need + dw), b + u * 1.5 * MM + left * sg * (need + dw), None));
            }
            let k1 = dr.st.wires[0].passes.len();
            let leff = super::super::octi::jog_leg(&dr.octi, wi.w + wi.s);
            let short = |pts: &[Vec2]| -> usize {
                let segs: Vec<Vec2> = pts.windows(2).map(|x| x[1] - x[0]).filter(|d| d.len2() > 1.0).collect();
                // a bend after two consecutive legs below the jog leg (a staircase step)
                (1..segs.len().saturating_sub(1)).filter(|&k| super::super::octi::turn_deg(segs[k], segs[k + 1]) > 0.5 && segs[k].len() < leff - 1_000.0 && segs[k - 1].len() < leff - 1_000.0).count()
            };
            let pts = vec![a, b];
            let form = |dr: &Drawer| dr.octilinear(0, 0, k1, 0, &pts, None, [None, None], None).map_or(pts.clone(), |o| o.pts);
            let keep = dr.octi;
            assert!(keep.short_leg_nm > 0.0, "the default weight is on");
            dr.octi.short_leg_nm = 0.0;
            let f0 = form(dr);
            dr.octi = keep;
            let f1 = form(dr);
            let mm = |q: &[Vec2]| q.iter().map(|p| format!("({:.3},{:.3})", p.x / MM, -p.y / MM)).collect::<Vec<_>>().join(" ");
            eprintln!("dw {dw}: weight 0 {} legs {} short [{}]; default {} legs {} short [{}]; leff {leff}", f0.len() - 1, short(&f0), mm(&f0), f1.len() - 1, short(&f1), mm(&f1));
            // r7 chord nodes at least the jog leg apart: the unpriced staircase's steps (from the
            // start of one flat leg to the next, along the chord) are no shorter than the jog leg.
            let starts: Vec<f64> = (1..f0.len().saturating_sub(1)).filter(|&k| super::super::octi::octi_dir(f0[k + 1] - f0[k]).is_some_and(|d| d % 4 == 0) && super::super::octi::octi_dir(f0[k] - f0[k - 1]).is_some_and(|d| d % 2 == 1)).map(|k| (f0[k] - a).dot(u)).collect();
            if let Some(m) = starts.windows(2).map(|x| x[1] - x[0]).reduce(f64::min) {
                assert!(m >= leff - 2_000.0, "dw {dw}: staircase step {m} nm below the jog leg {leff}");
            }
            got = (short(&f0), short(&f1));
        });
        got
    }

    #[test]
    fn short_leg_price_keeps_the_steps_long() {
        let mut seen = Vec::new();
        for dw in [0.03 * MM, 0.06 * MM, 0.1 * MM] {
            let (s0, s1) = short_legs_in_corridor(dw);
            seen.push((dw / MM, s0, s1));
            assert_eq!(s1, 0, "dw {} mm: a staircase step (two legs below the jog leg) with the price: {seen:?}", dw / MM);
        }
        // dw 0.06 mm: the unpriced DP draws a 24-leg staircase of 0.32 / 0.22 mm legs (all below
        // the 0.5 mm jog leg); with the price the chord stays as drawn (one item).
        assert!(seen.iter().any(|x| x.1 > 5), "the fixture must make the unpriced DP staircase: {seen:?}");
        // The shortness: 0 at and above the jog leg, graded below it, bounded.
        let p = |l: f64| super::super::octi::shortness(0.5 * MM, l * MM);
        assert!(p(0.5) == 0.0 && p(0.7) == 0.0 && p(0.499) > 0.0 && p(0.499) < 1e-4);
        assert!((p(0.25) - 0.64).abs() < 1e-9 && (p(0.0) - 64.0).abs() < 1e-9);
        assert!(p(0.1) > p(0.2) && p(0.2) > p(0.3) && p(0.3) > p(0.4));
        // The staircase price reads two consecutive legs: a single short leg between two long
        // ones (eprom A0: 45 degrees, 0.17 mm, 45 degrees) pays no more than without the weight;
        // two short legs in a row do.
        let on = super::super::octi::OctiCfg { on: true, short_leg_nm: 2.0 * MM, ..Default::default() };
        let off = super::super::octi::OctiCfg { short_leg_nm: 0.0, ..on };
        let pitch = 0.375 * MM;
        let v = |x: f64, y: f64| Vec2 { x: x * MM, y: y * MM };
        let jog = [v(0.0, 0.0), v(0.665, 0.665), v(0.835, 0.665), v(1.5, 1.33)];
        let st = |c: &super::super::octi::OctiCfg, q: &[Vec2]| super::super::octi::style_price(c, pitch, q);
        assert_eq!(st(&on, &jog), st(&off, &jog), "a single short leg pays only the jog penalty");
        let stair = [v(0.0, 0.0), v(0.158, 0.0), v(0.316, 0.158), v(0.474, 0.158), v(0.632, 0.316), v(2.0, 0.316)];
        assert!(st(&on, &stair) > st(&off, &stair) + 2.0 * MM, "a staircase pays: {} vs {}", st(&on, &stair), st(&off, &stair));
    }

    /// first-draw-45 r9 (`[drawing] bundle_pull_range_mm` in the DP, user: a configurable bundle
    /// pull range): a net-1 run from A (2, 12) to B (12, 9) mm (KiCad frame) and a drawn net-2
    /// track along y = 12 + need + 0.3 mm from x = 3 to 8 (need = the centre distance at the
    /// clearance), below the run's own octilinear forms (the elbows along y = 12 or y = 9). Only
    /// the DP's own bundle reach can give the hug: the track's octagon corners are nodes up to 2
    /// pitches past its clearance from the taut chord (here about 0.9 mm), and from the corner
    /// node an elbow runs along the track at its clearance (0.3 mm below y = 12, 5 mm beside it:
    /// the `bundle_pull` share of that length outweighs its two bends and 0.25 mm of length).
    /// Range off (0, the default): the form has that hug leg. Range 0.5 mm (below the corners'
    /// distance from the chord): no corner of the track is a node, no leg beside it; the form is
    /// an elbow of the run's own (on y = 12 or y = 9). The straightening is not involved (the DP
    /// output itself is checked).
    #[test]
    fn bundle_pull_range_limits_the_dp_reach() {
        let v = |x: f64, y: f64| Vec2 { x: x * MM, y: -y * MM };
        let (a, b) = (v(2.0, 12.0), v(12.0, 9.0));
        let tw = 150_000.0;
        let mut got: Vec<(f64, bool, f64, Vec<Vec2>)> = Vec::new();
        for range in [f64::INFINITY, 0.5 * MM] {
            two_pads((2.0, 12.0), (12.0, 9.0), &mut |dr| {
                let wi = dr.st.wires[0].clone();
                let wp = Wp { wire: 0, net: 1, w: wi.w, s: wi.s, layer: 0, own: [wi.pa, wi.pb], neck: None, relax: 0.0 };
                // the need of a net-2 track of width tw: its half width + the clearance + the run's half width
                let h = dr.commit(&track_item(2, 0, tw, v(3.0, 30.0), v(8.0, 30.0), None));
                let id = dr.handles[h].1;
                let need = dr.need_of(id, &wp, wi.w);
                dr.uncommit(h);
                let ny = 12.0 + need / MM + 0.3;
                dr.commit(&track_item(2, 0, tw, v(3.0, ny), v(8.0, ny), None));
                dr.octi.pull_range = range;
                let k1 = dr.st.wires[0].passes.len();
                let pts = vec![a, b];
                let dr: &Drawer = dr;
                let o = dr.octilinear(0, 0, k1, 0, &pts, None, [None, None], None).expect("a form");
                assert!(o.pts.windows(2).all(|x| super::super::octi::octi_dir(x[1] - x[0]).is_some()), "range {range}: free-angle in {:?}", o.pts);
                // the longest horizontal leg at the hug level (the track's clearance line, KiCad y = ny - need)
                let hug_y = -(ny * MM - need);
                let hug: f64 = o.pts.windows(2).filter(|x| (x[0].y - hug_y).abs() < 2_000.0 && (x[1].y - hug_y).abs() < 2_000.0).map(|x| (x[1] - x[0]).len()).fold(0.0, f64::max);
                let own = o.pts.windows(2).any(|x| (x[1] - x[0]).len() > 4.0 * MM && ((x[0].y - a.y).abs() < 2_000.0 && (x[1].y - a.y).abs() < 2_000.0 || (x[0].y - b.y).abs() < 2_000.0 && (x[1].y - b.y).abs() < 2_000.0));
                got.push((range, own, hug, o.pts.clone()));
            });
        }
        let mm = |q: &[Vec2]| q.iter().map(|p| format!("({:.3},{:.3})", p.x / MM, -p.y / MM)).collect::<Vec<_>>().join(" ");
        for g in &got {
            eprintln!("range {} mm: own elbow leg {}, hug leg {:.3} mm: {}", g.0 / MM, g.1, g.2 / MM, mm(&g.3));
        }
        assert!(got[0].2 >= 4.0 * MM, "no limit: the DP's form runs beside the drawn track (hug leg >= 4 mm): {}", mm(&got[0].3));
        assert!(got[1].2 < 1_000.0 && got[1].1, "range 0.5 mm: the run keeps an elbow of its own, no leg beside the track: {}", mm(&got[1].3));
    }

    /// first-draw-45 P1: an octilinear piece of the taut run is a chain of the DP as drawn, without
    /// the leg test, priced by its clearance below the rule and counted. A run from A (2, 12)
    /// along the line y = 12 to M (8, 12), then any-angle to B (12, 9); foreign net-2 tracks
    /// 0.3 mm off that line (below the rule, not a short).
    /// - Case 1, a track on both sides from x = 1 to 7.5 (past A): no leg out of A passes the leg
    ///   test, so before P1 the DP had no chain at all (the whole run stayed any-angle, `None`).
    ///   With P1 the piece A-M stays as drawn (counted, below the clearance) and M-B is drawn
    ///   octilinear.
    /// - Case 2, one track from x = 3 to 7: a legal 45 degree form runs below it; the piece A-M as
    ///   drawn pays its clearance and loses (costly, not failed: a priced piece is not taken where
    ///   a legal connector exists).
    #[test]
    fn octilinear_taut_piece_is_a_chain_as_drawn_priced_and_counted() {
        let v = |x: f64, y: f64| Vec2 { x: x * MM, y: -y * MM };
        let (a, m, b) = (v(2.0, 12.0), v(8.0, 12.0), v(12.0, 9.0));
        let tw = 150_000.0;
        for case in 0..2 {
            two_pads((2.0, 12.0), (12.0, 9.0), &mut |dr| {
                let tracks: &[(f64, f64, f64)] = if case == 0 { &[(1.0, 7.5, 0.3), (1.0, 7.5, -0.3)] } else { &[(3.0, 7.0, 0.3)] };
                for &(x0, x1, dy) in tracks {
                    dr.commit(&track_item(2, 0, tw, v(x0, 12.0 - dy), v(x1, 12.0 - dy), None));
                }
                let k1 = dr.st.wires[0].passes.len();
                let pts = vec![a, m, b];
                let dr: &Drawer = dr;
                let wi = &dr.st.wires[0];
                let wp = Wp { wire: 0, net: 1, w: wi.w, s: wi.s, layer: 0, own: [wi.pa, wi.pb], neck: None, relax: 0.0 };
                let nr = dr.near(dr.local_ents(&wp, bbox_of(&pts).grow(4.0 * MM)), &wp);
                assert!(!dr.seg_clear_near(&nr, a, m) && dr.seg_deficit_near(&nr, a, m) > 0.0, "case {case}: A-M must be below the rule");
                let o = dr.octilinear(0, 0, k1, 0, &pts, None, [None, None], None);
                let o = o.unwrap_or_else(|| panic!("case {case}: no octilinear form"));
                let octi_legs = o.pts.windows(2).all(|x| super::super::octi::octi_dir(x[1] - x[0]).is_some());
                assert!(octi_legs, "case {case}: free-angle left in {:?}", o.pts);
                let below = o.pts.windows(2).any(|x| dr.seg_deficit_near(&nr, x[0], x[1]) > 0.0);
                if case == 0 {
                    // A-M kept as drawn (merged with the collinear start of the octilinear rest).
                    assert!(o.pts[0] == a && o.pts[1].y == a.y && o.pts[1].x >= m.x, "case 0: A-M kept as drawn: {:?}", o.pts);
                    assert!(below && (o.asis, o.asis_below) == (1, 1), "case 0: counted {} / {}: {:?}", o.asis, o.asis_below, o.pts);
                } else {
                    assert!(!below && (o.asis, o.asis_below) == (0, 0), "case 1: the legal form must win: {:?} (asis {})", o.pts, o.asis);
                }
            });
        }
        // Case 3 (r4, the price changes the form; kills mutant P1-price): case 2 without the
        // bundle-follow reward (`pull` 0) and a second foreign track at y = 10.5 (x 3 to 7) that
        // closes the one-bend legal forms (45 degrees out of A, then along y = 9). Every
        // monotone octilinear form is equally long; the as-drawn A-(9, 12)-B has one bend, the
        // legal form between the tracks (45 degrees to y = 11.1, along it, 45 degrees to B) two:
        // only the clearance price of the as-drawn piece makes the legal form win (unpriced,
        // A-M would stay below the rule, counted).
        two_pads((2.0, 12.0), (12.0, 9.0), &mut |dr| {
            dr.octi.pull = 0.0;
            dr.commit(&track_item(2, 0, tw, v(3.0, 11.7), v(7.0, 11.7), None));
            dr.commit(&track_item(2, 0, tw, v(3.0, 10.5), v(7.0, 10.5), None));
            let k1 = dr.st.wires[0].passes.len();
            let pts = vec![a, m, b];
            let dr: &Drawer = dr;
            let wi = &dr.st.wires[0];
            let wp = Wp { wire: 0, net: 1, w: wi.w, s: wi.s, layer: 0, own: [wi.pa, wi.pb], neck: None, relax: 0.0 };
            let nr = dr.near(dr.local_ents(&wp, bbox_of(&pts).grow(4.0 * MM)), &wp);
            let o = dr.octilinear(0, 0, k1, 0, &pts, None, [None, None], None).expect("case 3: a form");
            let below = o.pts.windows(2).any(|x| dr.seg_deficit_near(&nr, x[0], x[1]) > 0.0);
            assert!(!below && o.asis == 0, "case 3: the priced piece must lose to the legal form: {:?} (asis {})", o.pts, o.asis);
        });
    }

    /// first-draw-45 P2: an edge the run's own taut path crosses (no portal of its sleeve) that an
    /// undrawn wire crosses too is a portal of the run in the room the plan left free there: open
    /// at the run's own crossing and up to the undrawn wire's planned crossing less the spacing,
    /// a wall from there on (r3: the wire's room and its packed slack stay its own; esp RCA). On
    /// the pad field: wire 0's run gets a detour vertex just past such an edge, crossing it 1.3
    /// spacings short of the undrawn wire's planned crossing. Before P2 the whole edge was a
    /// wall. Checked: no wall at the run's crossing nor in the free room next to the spacing
    /// limit, a wall just past the limit and at the far end; the DP finds a form in most runs.
    #[test]
    fn own_taut_crossings_are_portals_in_free_room() {
        let (mut cases, mut limit_checked, mut some) = (0usize, 0usize, 0usize);
        // candidate edges dropped per filter: not one undrawn pass, its planned crossing,
        // distance, position, own gate
        let mut why = [0usize; 5];
        with_octi_fields(&mut |dr, _g| {
            // The undrawn wires on layer 0 (the fixture sets no plan layers, and the packer leaves
            // a pass without a layer unpacked: no planned crossing), packed again.
            for l in dr.pass_layer.iter_mut() {
                *l = 0;
            }
            dr.compute_intervals();
            let dr: &Drawer = dr;
            let (st, pr) = (dr.st, &dr.b.proj);
            let wi = &st.wires[0];
            let k1 = wi.passes.len();
            let poly = st.polyline(0);
            let portal: Vec<usize> = wi.passes.iter().map(|&q| st.passes[q].edge).collect();
            let mut tried = 0;
            for e in 0..pr.edges.len() {
                if tried >= 6 || portal.contains(&e) {
                    continue;
                }
                if pr.edge_tris[e].iter().flatten().filter(|&&t| !pr.tri_out[t]).count() < 2 {
                    continue;
                }
                let [ea, eb] = pr.edges[e];
                let (pp, pq) = (pr.pos[ea], pr.pos[eb]);
                let len = (pq - pp).len();
                let qs: Vec<usize> = st.edge_order[e].iter().copied().filter(|&q| st.passes[q].wire != 0).collect();
                let [q] = qs[..] else {
                    why[0] += 1;
                    continue;
                };
                let Some(tq) = dr.iv.nom.get(q).copied().filter(|x| x.is_finite()).map(|x| x * len) else {
                    why[1] += 1;
                    continue;
                };
                let oi = &st.wires[st.passes[q].wire];
                let sp = 0.5 * wi.w + wi.s.max(oi.s) + 0.5 * oi.w;
                let u = (pq - pp) * (1.0 / len.max(1.0));
                let Some((i, d)) = poly.windows(2).enumerate().filter(|(_, x)| !super::super::draw::seg_proper(x[0], x[1], pp, pq)).map(|(i, x)| (i, super::super::draw::seg_seg_dist(x[0], x[1], pp, pq))).min_by(|a, b| a.1.total_cmp(&b.1)) else { continue };
                if !(0.3 * MM..=2.0 * MM).contains(&d) {
                    why[2] += 1;
                    continue;
                }
                let (a, b) = (poly[i], poly[i + 1]);
                let mid = (a + b) * 0.5;
                let near = (mid - pp).dot(u).clamp(0.0, len);
                let below = near < tq;
                let tx = if below { tq - 1.3 * sp } else { tq + 1.3 * sp };
                if !(0.2 * MM..len - 0.2 * MM).contains(&tx) {
                    why[3] += 1;
                    continue;
                }
                let nrm = Vec2 { x: -u.y, y: u.x };
                let side = if (mid - pp).dot(nrm) > 0.0 { 1.0 } else { -1.0 };
                let x = pp + u * tx - nrm * (side * 50_000.0);
                let mut pts = poly.clone();
                pts.insert(i + 1, x);
                tried += 1;
                let (strip, gates) = dr.open_strip(0, 0, k1, 0, &pts);
                let Some(&(gp, gu, glen, _, _)) = gates.iter().find(|g| (g.0 - pp).len() < 1.0 && (g.2 - len).abs() < 1.0) else {
                    why[4] += 1;
                    continue;
                };
                cases += 1;
                let n = Vec2 { x: -gu.y, y: gu.x } * 1_000.0;
                let wall_at = |t: f64| strip.crosses_wall(gp + gu * t - n, gp + gu * t + n);
                let far = if below { glen - 3_000.0 } else { 3_000.0 };
                assert!(wall_at(far), "edge {e}: no wall beyond the undrawn crossing");
                let hit: Vec<_> = strip.walls.iter().filter(|w| Strip::new(Vec::new(), vec![**w]).crosses_wall(gp + gu * tx - n, gp + gu * tx + n)).collect();
                assert!(hit.is_empty(), "edge {e} ({pp:?}-{pq:?}, tq {tq}, tx {tx}): the run's own crossing is a wall: {hit:?}");
                // the spacing limit: free room short of it, a wall past it
                let lim = if below { tq - sp } else { tq + sp };
                let (inside, past) = if below { (lim - 5_000.0, lim + 5_000.0) } else { (lim + 5_000.0, lim - 5_000.0) };
                assert!(wall_at(past), "edge {e}: no wall just past the spacing limit {lim} (planned crossing {tq})");
                if (inside - tx).abs() > 5_000.0 {
                    assert!(!wall_at(inside), "edge {e}: a wall in the free room short of the spacing limit {lim}");
                    limit_checked += 1;
                }
                some += usize::from(dr.octilinear(0, 0, k1, 0, &pts, None, [None, None], None).is_some());
            }
        });
        eprintln!("P2: dropped {why:?}; {cases} runs with an own portal, spacing limit checked in {limit_checked}, DP form in {some}");
        assert!(cases > 5 && limit_checked > 0 && some > 0, "{cases} runs, limit checked {limit_checked}, DP forms {some}");
    }

    /// Three nets, each a pad pair from the row A (x = 4) to the row B (x = 20), all wires routed
    /// and on layer 0, packed (`compute_intervals`), none drawn: the middle wire has an undrawn
    /// lane neighbour on both sides of its passes.
    fn pad_rows(f: &mut dyn FnMut(&mut Drawer)) {
        let p = |v: f64| (v * MM) as i64;
        let pad = |i: usize, net: i32, x: f64, y: f64| format!(r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"smd","center":[{x},{y}],"rotation_deg":0,"layers":[0],"shapes":{{"0":{{"kind":"rect","w":{s},"h":{s}}}}},"drill":null}}"#, x = p(x), y = p(y), s = p(0.4));
        let c = [(0.0, 0.0), (24.0, 0.0), (24.0, 24.0), (0.0, 24.0)];
        let segs: Vec<String> = (0..4).map(|i| format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(c[i].0), p(c[i].1), p(c[(i + 1) % 4].0), p(c[(i + 1) % 4].1))).collect();
        let mut pads = Vec::new();
        for n in 0..3 {
            pads.push(pad(2 * n, n as i32 + 1, 4.0, 14.0 + 0.8 * n as f64));
            pads.push(pad(2 * n + 1, n as i32 + 1, 20.0, 8.0 + 0.8 * n as f64));
        }
        let js = format!(
            r#"{{"units":"nm","board":"t","layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],"nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}},{{"code":3,"name":"C"}}],"pads":[{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
            pads.join(","),
            segs.join(",")
        );
        let m = BoardModel::from_json(&js).unwrap();
        let bd = board::build(&m, &BuildOpts::default()).unwrap();
        let conns = bd.connections(&m);
        let mut st = State::new(&bd.proj);
        for net in 1..=3 {
            let cn = conns.iter().find(|c| c.w > 0.0 && c.net == net).expect("a connection per net");
            st.route_conn(cn, &InitWeights::default(), (cn.w + cn.s) / 400_000.0).expect("routes");
        }
        let opts = Options::default();
        let checker = Checker::new(&m, &opts);
        let mut dr = Drawer::new(&m, &opts, &bd, &st, &checker);
        dr.octi = crate::topo::octi::OctiCfg { on: true, ..dr.octi };
        for l in dr.pass_layer.iter_mut() {
            *l = 0;
        }
        dr.compute_intervals();
        f(&mut dr);
    }

    /// first-draw-45 r4 (qmtech MAX_SCLK RCA): the octilinear search keeps the plan's lane order.
    /// Three parallel wires (`pad_rows`), none drawn: for each pass of each wire with an undrawn
    /// lane neighbour on a side, the edge is a wall from just past that neighbour's planned
    /// crossing on (a leg there passes the neighbour on its other side) and open at the run's own
    /// crossing and in the slot between (that slot is the graded lane-slot price, M6; a wall at
    /// the neighbour's bound, its crossing less the lane spacing, would close it).
    #[test]
    fn octilinear_keeps_the_lane_order_of_undrawn_neighbours() {
        let (mut sides, mut slots) = (0usize, 0usize);
        pad_rows(&mut |dr| {
            let dr: &Drawer = dr;
            let (st, pr) = (dr.st, &dr.b.proj);
            for w in 0..st.wires.len() {
                let wi = &st.wires[w];
                let k1 = wi.passes.len();
                let taut = st.polyline(w);
                let walls = dr.lane_order_walls(w, 0, k1, &taut);
                let strip = Strip::new(Vec::new(), walls.clone());
                for k in 0..k1 {
                    let pid = wi.passes[k];
                    let [p, q] = pr.edges[st.passes[pid].edge];
                    let (pp, pq) = (pr.pos[p], pr.pos[q]);
                    let len = (pq - pp).len();
                    let u = (pq - pp) * (1.0 / len);
                    let n = Vec2 { x: -u.y, y: u.x } * 1_000.0;
                    let at = |t: f64| strip.crosses_wall(pp + u * t - n, pp + u * t + n);
                    let nom = dr.iv.nom.get(pid).copied().filter(|x| x.is_finite()).unwrap_or(0.5) * len;
                    let Some(xt) = super::super::draw::crossing(&taut, pp, pq, nom) else { continue };
                    assert!(!at(xt), "wire {w} pass {k}: the run's own crossing is a wall");
                    let (before, after) = dr.lane_sides(pid);
                    let (pd, nd) = dr.lane_next_drawn(pid);
                    for (side, on, drawn) in [(false, before, pd), (true, after, nd)] {
                        let Some(xn) = dr.neighbour_x(pid, side).filter(|_| on && !drawn) else { continue };
                        let past = if side { xn + 5_000.0 } else { xn - 5_000.0 };
                        if (side && xn <= xt + 1.0) || (!side && xn >= xt - 1.0) || !(1.0..len - 1.0).contains(&past) {
                            continue;
                        }
                        assert!(at(past), "wire {w} pass {k}: no wall past the undrawn neighbour's crossing {xn} (own {xt}, edge {len})");
                        sides += 1;
                        let mid = 0.5 * (xt + xn);
                        if (mid - xt).abs() > 10_000.0 && (mid - xn).abs() > 10_000.0 {
                            assert!(!at(mid), "wire {w} pass {k}: a wall in the slot between the own crossing {xt} and the neighbour's {xn}");
                            slots += 1;
                        }
                    }
                }
            }
        });
        eprintln!("lane order: {sides} neighbour sides walled, {slots} slots open");
        assert!(sides > 3 && slots > 0, "{sides} sides, {slots} slots");
    }

    /// first-draw-45 P5: the taut pull trades an octilinear leg for free angle only where the
    /// shortcut is cheaper on the DP's scale (length plus the `any_angle` price). A 2 mm leg along
    /// y = 12 then any-angle to B: the shortcut saves 0.1 mm and makes the whole run free-angle,
    /// so the corner stays (before: removed). A 6 mm leg up and a sharp turn back: the shortcut
    /// saves 10.7 mm, more than its any-angle price, and is taken (graded, not a cutoff).
    #[test]
    fn pull_keeps_an_octilinear_leg_unless_the_shortcut_pays() {
        let v = |x: f64, y: f64| Vec2 { x: x * MM, y: -y * MM };
        for (b, c, kept) in [((4.0, 12.0), (12.0, 9.0), true), ((2.0, 6.0), (4.0, 12.5), false)] {
            two_pads((2.0, 12.0), c, &mut |dr| {
                let k1 = dr.st.wires[0].passes.len();
                let pts = vec![v(2.0, 12.0), v(b.0, b.1), v(c.0, c.1)];
                let got = dr.pull_taut(0, 0, k1, 0, 0, &pts, &None).map(|x| x.0);
                if kept {
                    assert_eq!(got, None, "the octilinear leg must stay: {got:?}");
                } else {
                    assert_eq!(got, Some(vec![pts[0], pts[2]]), "the paying shortcut must be taken");
                }
            });
        }
    }

    /// forced-draw (T6, the qmtech `Net-(U9-~{RST})` mechanism: the forced form cut through a
    /// foreign via, a short, where its taut run only missed the clearance): the forced DP puts
    /// shorts first in its shorts-first variant (`short_nm`). A net-1 run straight from A to B
    /// (not octilinear) along a foreign track
    /// on its left, closer than the rule but clear of its copper, and a second foreign track on
    /// its right, clear of the run: every octilinear form between A and B leaves the line by
    /// more than the room to one of the tracks, so it crosses it (a short). The forced DP keeps
    /// the taut run's short amount (0: it returns the taut run or a form without a short); the
    /// flat variant (a short priced like a clearance unit) takes a form that shorts, its
    /// clearance amount lower than the taut run's: the two variants differ, the key picks.
    #[test]
    fn forced_octilinear_puts_shorts_first() {
        let p = |v: f64| (v * MM) as i64;
        let (mut runs, mut traded) = (0usize, 0usize);
        let mut seen = Vec::new();
        for dy in [2.0, 3.0, 5.0] {
            for f2 in [0.05, 0.2, 0.4] {
                let (ax, ay, bx, by) = (2.0, 12.0, 12.0, 12.0 - dy);
                let pad = |i: usize, x: f64, y: f64| format!(r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":1,"attr":"smd","center":[{x},{y}],"rotation_deg":0,"layers":[0],"shapes":{{"0":{{"kind":"rect","w":{s},"h":{s}}}}},"drill":null}}"#, x = p(x), y = p(y), s = p(0.4));
                let c = [(0.0, 0.0), (24.0, 0.0), (24.0, 24.0), (0.0, 24.0)];
                let segs: Vec<String> = (0..4).map(|i| format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(c[i].0), p(c[i].1), p(c[(i + 1) % 4].0), p(c[(i + 1) % 4].1))).collect();
                let js = format!(
                    r#"{{"units":"nm","board":"t","layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],"nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}},{{"code":3,"name":"C"}}],"pads":[{},{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
                    pad(0, ax, ay),
                    pad(1, bx, by),
                    segs.join(",")
                );
                let m = BoardModel::from_json(&js).unwrap();
                let bd = board::build(&m, &BuildOpts::default()).unwrap();
                let conns = bd.connections(&m);
                let mut st = State::new(&bd.proj);
                let cn = conns.iter().find(|c| c.w > 0.0 && c.net == 1).expect("a net-1 connection");
                st.route_conn(cn, &InitWeights::default(), (cn.w + cn.s) / 400_000.0).expect("routes");
                let opts = Options::default();
                let checker = Checker::new(&m, &opts);
                let mut dr = Drawer::new(&m, &opts, &bd, &st, &checker);
                dr.octi = crate::topo::octi::OctiCfg { on: true, ..dr.octi };
                // math frame (y up)
                let (a, b) = (Vec2 { x: ax * MM, y: -ay * MM }, Vec2 { x: bx * MM, y: -by * MM });
                let u = (b - a).unit().expect("A != B");
                let left = Vec2 { x: -u.y, y: u.x };
                let wi = dr.st.wires[0].clone();
                let wp = Wp { wire: 0, net: 1, w: wi.w, s: wi.s, layer: 0, own: [wi.pa, wi.pb], neck: None, relax: 0.0 };
                let tw = 150_000.0;
                let r0 = 0.5 * tw + 0.5 * wi.w;
                // the room to a track's copper and its clearance band, measured on the first one
                let track = |dr: &mut Drawer, net: i32, off: f64| {
                    let (s0, s1) = (a - u * 1.5 * MM + left * off, b + u * 1.5 * MM + left * off);
                    dr.commit(&track_item(net, 0, tw, s0, s1, None));
                    dr.ents.len() - 1
                };
                let probe = track(&mut dr, 3, 20.0 * MM);
                let need = dr.need_of(probe, &wp, wi.w);
                track(&mut dr, 2, r0 + 0.1 * (need - r0));
                track(&mut dr, 3, -(need + f2 * MM));
                let pts = vec![a, b];
                let k1 = dr.st.wires[0].passes.len();
                let short = |dr: &Drawer, q: &[Vec2]| -> f64 { dr.forced_leg_deficits(0, 0, q, None).iter().map(|x| x[1]).sum() };
                let form = |sh: f64| dr.octilinear(0, 0, k1, 0, &pts, None, [None, None], Some(sh)).map_or(pts.clone(), |o| o.pts);
                let st0 = short(&dr, &pts);
                let s1 = short(&dr, &form(dr.octi.short_nm));
                let s0 = short(&dr, &form(dr.octi.viol_nm));
                assert!(st0 == 0.0 && s1 == 0.0, "dy {dy} f2 {f2}: the forced form shorts ({s1}, taut {st0})");
                traded += usize::from(s0 > 0.0);
                seen.push((dy, f2, s0));
                runs += 1;
            }
        }
        assert!(traded > 0, "short priced as clearance: no run traded clearance for a short ({runs} runs: {seen:?})");
    }

    /// pad-snap: the board JSON of the stub fixtures (pad A 1 mm square at (2, 12) mm, pad B at (8, 11.8)).
    fn pad_stub_json() -> String {
        let p = |v: f64| (v * MM) as i64;
        let pad = |i: usize, x: f64, y: f64| format!(r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":1,"attr":"smd","center":[{x},{y}],"rotation_deg":0,"layers":[0],"shapes":{{"0":{{"kind":"rect","w":{s},"h":{s}}}}},"drill":null}}"#, x = p(x), y = p(y), s = p(1.0));
        let c = [(0.0, 0.0), (24.0, 0.0), (24.0, 24.0), (0.0, 24.0)];
        let segs: Vec<String> = (0..4).map(|i| format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(c[i].0), p(c[i].1), p(c[(i + 1) % 4].0), p(c[(i + 1) % 4].1))).collect();
        format!(
            r#"{{"units":"nm","board":"t","layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],"nets":[{{"code":1,"name":"A"}}],"pads":[{},{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
            pad(0, 2.0, 12.0),
            pad(1, 8.0, 11.8),
            segs.join(",")
        )
    }

    /// pad-snap r2b (D3b, mutant M13): an all-octilinear run whose first leg is a stub wholly
    /// inside its own pad (tidy and KiCad's Cleanup delete it) is not kept as it is: the DP
    /// reprices it and returns a form whose first leg leaves the pad.
    #[test]
    fn pad_stub_run_is_repriced_not_kept() {
        let js = pad_stub_json();
        let m = BoardModel::from_json(&js).unwrap();
        let bd = board::build(&m, &BuildOpts::default()).unwrap();
        let conns = bd.connections(&m);
        let mut st = State::new(&bd.proj);
        let cn = conns.iter().find(|c| c.w > 0.0 && c.net == 1).expect("a net-1 connection");
        st.route_conn(cn, &InitWeights::default(), (cn.w + cn.s) / 400_000.0).expect("routes");
        let opts = Options::default();
        let checker = Checker::new(&m, &opts);
        let mut dr = Drawer::new(&m, &opts, &bd, &st, &checker);
        dr.octi = crate::topo::octi::OctiCfg { on: true, ..dr.octi };
        let k1 = dr.st.wires[0].passes.len();
        // math frame (y up): A's centre, 0.2 mm up inside A (1 mm square), then straight to B
        let (a, j, b) = (Vec2 { x: 2.0 * MM, y: -12.0 * MM }, Vec2 { x: 2.0 * MM, y: -11.8 * MM }, Vec2 { x: 8.0 * MM, y: -11.8 * MM });
        let pts = vec![a, j, b];
        assert_eq!(dr.run_pad_stubs(0, 0, k1, &pts), [true, false], "the fixture's first leg is an in-pad stub");
        let o = dr.octilinear(0, 0, k1, 0, &pts, None, [None, None], None).expect("an all-octilinear run with an in-pad stub is repriced, not kept as it is");
        assert!(o.pts.windows(2).all(|q| crate::topo::octi::octi_dir(q[1] - q[0]).is_some()), "octilinear: {:?}", o.pts);
        assert_eq!(dr.run_pad_stubs(0, 0, k1, &o.pts), [false, false], "the form's first leg leaves the pad: {:?}", o.pts);
        assert!((o.pts[0] - a).len() < 1.0 && (o.pts[o.pts.len() - 1] - b).len() < 1.0, "same ends: {:?}", o.pts);
    }

    /// pad-snap r3 (M1, mutants V2 and V3): ONE predicate, [`Drawer::own_pad_end`], decides whether
    /// a run end is an own-pad end, for the DP's stub price and for `run_pad_stubs`. An end anchored
    /// elsewhere (`end_at`) is none (V2); neither is a pad carrying a kept lead (V3), a continued
    /// lead, an end short of the run's end, or an end outside the pad.
    #[test]
    fn own_pad_end_predicate_is_one() {
        let m = BoardModel::from_json(&pad_stub_json()).unwrap();
        let opts = Options::default();
        let checker = Checker::new(&m, &opts);
        let (a, j, b) = (Vec2 { x: 2.0 * MM, y: -12.0 * MM }, Vec2 { x: 2.0 * MM, y: -11.8 * MM }, Vec2 { x: 8.0 * MM, y: -11.8 * MM });
        let pts = vec![a, j, b];
        for arm in ["base", "end_at", "kept_lead"] {
            let mut bd = board::build(&m, &BuildOpts::default()).unwrap();
            let conns = bd.connections(&m);
            let mut st = State::new(&bd.proj);
            let cn = conns.iter().find(|c| c.w > 0.0 && c.net == 1).expect("a net-1 connection");
            st.route_conn(cn, &InitWeights::default(), (cn.w + cn.s) / 400_000.0).expect("routes");
            if arm == "kept_lead" {
                let obs = st.wires[0].pa;
                bd.kept_ends.push(crate::topo::board::KeptEndOut { obs, net: 1, at: a, w: 150_000.0, lead: Some((a, j)), via_end: None, note: None, choices: Vec::new() });
            }
            let mut dr = Drawer::new(&m, &opts, &bd, &st, &checker);
            dr.octi = crate::topo::octi::OctiCfg { on: true, ..dr.octi };
            if arm == "end_at" {
                dr.end_at.insert((0, 0), a);
            }
            let k1 = dr.st.wires[0].passes.len();
            let priced = arm == "base";
            assert_eq!(dr.own_pad_end(0, 0, true, false, a).is_some(), priced, "{arm}");
            assert_eq!(dr.run_pad_stubs(0, 0, k1, &pts), [priced, false], "{arm}");
            // the DP: priced -> repriced form; else the all-octilinear run stays as it is
            assert_eq!(dr.octilinear(0, 0, k1, 0, &pts, None, [None, None], None).is_some(), priced, "{arm}");
            if arm == "base" {
                assert!(dr.own_pad_end(0, 0, false, false, a).is_none(), "not at the run's end");
                assert!(dr.own_pad_end(0, 0, true, true, a).is_none(), "continued lead");
                assert!(dr.own_pad_end(0, 0, true, false, Vec2 { x: 20.0 * MM, y: -12.0 * MM }).is_none(), "end outside the pad");
            }
        }
    }
}
