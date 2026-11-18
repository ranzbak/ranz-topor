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
            e.live && (wp.layer == usize::MAX || (e.mask >> wp.layer) & 1 == 1) && (e.net != Some(wp.net) || own_pad) && (i < self.n_static || !self.only_static.get())
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

    /// nd-perf F2: `via_reach` gives the same class verdict with `[vias] reach_price` off and on; with
    /// it on the price is the measured price of the reach segments (positive where they break a
    /// rule), with it off 0 (its only reader, `reach_cost`, returns 0 then). Random via sites on
    /// the routed wire's traversals of the pad field with drawn copper.
    #[test]
    fn via_reach_verdict_is_the_same_and_priced_only_when_read() {
        if std::env::var_os("TOPO_VIADBG").is_some() {
            return;
        }
        let crossed = crate::topo::draw::ClassCrossings::new();
        let sites = |opts: &Options| -> Vec<(bool, f64)> {
            let mut out = Vec::new();
            with_fields_opts(opts, &mut |dr, g| {
                let n = dr.st.wires[0].passes.len();
                for _ in 0..80 {
                    let k = g.int(0, n as i64) as usize;
                    out.push(dr.via_reach(0, k, pt(g), g.int(0, 1) as usize, g.int(0, 1) as usize, &crossed));
                }
            });
            out
        };
        let off = sites(&Options::default());
        let mut on_opts = Options::default();
        on_opts.via_reach_price = true;
        let on = sites(&on_opts);
        assert_eq!(off.len(), on.len());
        assert!(off.iter().zip(&on).all(|(a, b)| a.0 == b.0), "the class verdict depends on reach_price");
        assert!(off.iter().all(|a| a.1 == 0.0));
        let priced = on.iter().filter(|b| b.1 > 0.0).count();
        assert!(priced > 20, "only {priced} priced reaches");
        assert!(off.iter().any(|a| !a.0), "no out-of-class site exercised");
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
                let (p0, vs0, lazy) = dr.lazy_cand(wire, &items, &rep);
                let mut c: crate::topo::draw::ForcedCand = (p0, Vec::new(), items.clone(), vs0, None, "test", lazy);
                let bound = dr.cand_bound(&c);
                dr.price_cand(&mut c);
                let mut eager = dr.viols_of(&items, &rep);
                eager.extend(dr.graze_viols(wire, &items));
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
    fn with_octi_fields(f: &mut dyn FnMut(&Drawer, &mut Rng)) {
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
            f(&dr, &mut g);
        }
    }

    /// `octilinear` with its per-pair shape cache returns the same run as the DP loop that builds
    /// and prices every connector shape for every arrival state (`OCTI_REF`), on random any-angle
    /// runs of the routed wire with and without pad exits.
    #[test]
    fn octilinear_shape_cache_matches_the_reference_loop() {
        use super::super::octi::{OCTI_HITS, OCTI_REF};
        let (mut some, mut runs) = (0, 0);
        with_octi_fields(&mut |dr, g| {
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
                let relax = [0.0, 20_000.0][g.int(0, 1) as usize];
                OCTI_REF.with(|f| f.set(false));
                let got = dr.octilinear(0, 0, k1, layer, &pts, None, exits, relax);
                OCTI_REF.with(|f| f.set(true));
                let want = dr.octilinear(0, 0, k1, layer, &pts, None, exits, relax);
                OCTI_REF.with(|f| f.set(false));
                runs += 1;
                some += usize::from(want.is_some());
                let bits = |v: &Option<super::super::octi::OctiOut>| v.as_ref().map(|o| o.pts.iter().map(|p| (p.x.to_bits(), p.y.to_bits())).collect::<Vec<_>>());
                assert_eq!(bits(&got), bits(&want));
            }
        });
        let hits = OCTI_HITS.with(|h| h.get());
        assert!(some > 10 && hits[0] > 20, "{some} of {runs}, hits {hits:?}");
    }
}
