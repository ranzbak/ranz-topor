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
}

impl Strip {
    /// `p` lies in (or on the boundary of) a triangle of the sleeve.
    pub(super) fn contains(&self, p: Vec2) -> bool {
        self.tris.iter().zip(&self.boxes).any(|(t, bb)| {
            if p.x < bb.min.x - 1.0 || p.x > bb.max.x + 1.0 || p.y < bb.min.y - 1.0 || p.y > bb.max.y + 1.0 {
                return false;
            }
            let d = |a: Vec2, b: Vec2| (b - a).cross(p - a);
            let (d1, d2, d3) = (d(t[0], t[1]), d(t[1], t[2]), d(t[2], t[0]));
            let tol = -2.0 * (t[1] - t[0]).len().max((t[2] - t[1]).len()).max(1.0);
            d1 >= tol && d2 >= tol && d3 >= tol
        })
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
        let boxes = tris.iter().map(|t| bbox_of(t)).collect();
        Strip { tris, boxes, walls }
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
            let own_pad = e.net == Some(wp.net) && i < n_obs && !wp.own.contains(&i);
            e.live && (wp.layer == usize::MAX || (e.mask >> wp.layer) & 1 == 1) && (e.net != Some(wp.net) || own_pad) && (i < self.n_static || !self.only_static.get())
        });
        ids
    }

    pub(super) fn need_of(&self, id: usize, wp: &Wp, w: f64) -> f64 {
        let e = &self.ents[id];
        if e.net == Some(wp.net) {
            // A pad of the own net that is not an end: KiCad's minimum connection width check
            // builds the pad's polygon up to `max_error` outside its arcs, so a track closer than
            // that reads as a sliver connection (dcsgonebadge R10.1, D2: 1.7 um off the arc,
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
        ents.iter().all(|&i| {
            let e = &self.ents[i];
            let need = self.need_of(i, wp, sw);
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
            seg_core(a, b, &e.core).0 >= need - TOL
        })
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
            if strip.contains(p) && self.seg_clear(&ents, wp, p, p) {
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
                if strip.crosses_wall(p, q) || !strip.contains(mid) || !self.seg_clear(&ents, wp, p, q) {
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
        for _ in 0..24 {
            let mut bad = None;
            for i in 0..pts.len() - 1 {
                let ents = self.local_ents(&wp, bbox_of(&[pts[i], pts[i + 1]]).grow(self.qgrow));
                if !self.seg_clear(&ents, &wp, pts[i], pts[i + 1]) || via_wall(pts[i], pts[i + 1]) {
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
