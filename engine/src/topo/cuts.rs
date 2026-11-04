//! P1 clearance cuts: the locally-clearance-correct refinement of the projection CDT (plan P1,
//! Kallmann 2010, "Navigation meshes and clearance" / LCT).
//!
//! A wire that traverses triangle `t` from edge `a` to edge `b` around their shared corner `c`
//! has the room "distance from `c` to the nearest obstacle copper in that sector". In a plain
//! CDT that distance is the length of a triangle side only when the nearest copper is a vertex;
//! when it is the interior of an obstacle side (a corner facing a pad side, two parallel pads)
//! the sides of the triangle are longer than the true gap and the room is overestimated. This
//! module finds those facing pairs: for every obstacle vertex `v` and every other obstacle (or
//! outline) side `s` within `max_d`, the foot `f` of `v` on `s` when it lies inside `s` and the
//! segment `v f` is free. The builder inserts `f` as a vertex on `s` and `v f` as a constraint,
//! so the sector's nearest copper is always a vertex and the cut is a CDT edge: its room is
//! its length, its demand comes from [`super::demand`], and the State, P2, P3 and the witness use
//! it with no further machinery.
//!
//! Per vertex and obstacle only the nearest foot is kept (the others are not bottlenecks), cuts
//! that would cross an already kept cut are dropped (the CDT would split both at the crossing and
//! lose the room), and feet closer than `eps` to a vertex of the side or to another foot are
//! skipped (a vertex-to-vertex edge is as short already).
//!
//! Complexity: O(V · S_near) with a grid of cell size `max_d`, plus O(cuts · cuts_near) for the
//! crossing test.

use crate::geom::Vec2;
use super::state::point_in_poly;

/// One obstacle or outline side.
#[derive(Clone, Copy, Debug)]
pub struct Side {
    pub a: Vec2,
    pub b: Vec2,
    /// Obstacle id; `None` = board outline.
    pub owner: Option<usize>,
}

/// One vertex (obstacle polygon corner or outline end point).
#[derive(Clone, Copy, Debug)]
pub struct Vertex {
    pub p: Vec2,
    pub owner: Option<usize>,
}

/// A cut: vertex `v` (index into the vertex list) faces side `side` at parameter `t` (0..1);
/// `t` of exactly 0 or 1 is a vertex-to-vertex cut (no new vertex).
#[derive(Clone, Copy, Debug)]
pub struct Foot {
    pub v: usize,
    pub side: usize,
    pub t: f64,
    pub at: Vec2,
    pub d: f64,
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.len2();
    let t = if l2 > 0.0 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + d * t)).len()
}

/// Whether segments `pq` and `rs` touch or cross.
fn touch(p: Vec2, q: Vec2, r: Vec2, s: Vec2) -> bool {
    let (d1, d2) = (q - p, s - r);
    let den = d1.cross(d2);
    if den.abs() > 1e-9 {
        let t = (r - p).cross(d2) / den;
        let u = (r - p).cross(d1) / den;
        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
            return true;
        }
    }
    seg_dist(r, p, q).min(seg_dist(s, p, q)).min(seg_dist(p, r, s)).min(seg_dist(q, r, s)) < 1.0
}

/// Uniform grid of cell size `cell` over a set of items given by their bounding boxes.
struct Grid {
    lo: Vec2,
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(boxes: &[(Vec2, Vec2)], cell: f64) -> Grid {
        let mut lo = Vec2 { x: f64::INFINITY, y: f64::INFINITY };
        let mut hi = Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY };
        for &(a, b) in boxes {
            lo = Vec2 { x: lo.x.min(a.x), y: lo.y.min(a.y) };
            hi = Vec2 { x: hi.x.max(b.x), y: hi.y.max(b.y) };
        }
        if boxes.is_empty() {
            lo = Vec2 { x: 0.0, y: 0.0 };
            hi = lo;
        }
        let nx = ((hi.x - lo.x) / cell) as usize + 1;
        let ny = ((hi.y - lo.y) / cell) as usize + 1;
        let mut g = Grid { lo, cell, nx, ny, cells: vec![Vec::new(); nx * ny] };
        for (i, &(a, b)) in boxes.iter().enumerate() {
            let (x0, y0) = g.at(a);
            let (x1, y1) = g.at(b);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    g.cells[y * nx + x].push(i as u32);
                }
            }
        }
        g
    }

    fn at(&self, p: Vec2) -> (usize, usize) {
        let x = (((p.x - self.lo.x) / self.cell).floor().max(0.0) as usize).min(self.nx - 1);
        let y = (((p.y - self.lo.y) / self.cell).floor().max(0.0) as usize).min(self.ny - 1);
        (x, y)
    }

    /// Items registered in the cell of `p` (boxes are padded by the query radius, so the cell
    /// holds every item within that radius of `p`).
    fn near(&self, p: Vec2) -> &[u32] {
        let (cx, cy) = self.at(p);
        &self.cells[cy * self.nx + cx]
    }
}

/// The cuts of `verts` against `sides`: feet within `max_d` (nm), at least `eps` from a side's
/// ends and from each other. `polys[o]` is the polygon of obstacle `o`; `shares(a, b)` says
/// whether the two owners (`None` = outline) have a copper layer in common (otherwise no wire is
/// squeezed between them). Deterministic: candidates in vertex order, nearest first.
pub fn lct_feet(
    verts: &[Vertex],
    sides: &[Side],
    polys: &[&[Vec2]],
    shares: &dyn Fn(Option<usize>, Option<usize>) -> bool,
    max_d: f64,
    eps: f64,
) -> Vec<Foot> {
    if max_d <= 0.0 || sides.is_empty() {
        return Vec::new();
    }
    let boxes: Vec<(Vec2, Vec2)> = sides
        .iter()
        .map(|s| {
            (
                Vec2 { x: s.a.x.min(s.b.x) - max_d, y: s.a.y.min(s.b.y) - max_d },
                Vec2 { x: s.a.x.max(s.b.x) + max_d, y: s.a.y.max(s.b.y) + max_d },
            )
        })
        .collect();
    // Long sides span many cells; the grid is over the padded boxes, a lookup is the cell of `v`.
    let grid = Grid::new(&boxes, max_d.max(1.0));
    // Candidates per vertex: nearest foot per (vertex, other owner); outline: nearest overall.
    let mut cand: Vec<Foot> = Vec::new();
    for (vi, v) in verts.iter().enumerate() {
        let mut best: Vec<(Option<usize>, Foot)> = Vec::new();
        for &si in grid.near(v.p) {
            let s = &sides[si as usize];
            if s.owner == v.owner || !shares(v.owner, s.owner) {
                continue;
            }
            let e = s.b - s.a;
            let l = e.len();
            if l <= 2.0 * eps {
                continue;
            }
            // The nearest point of the side: its interior (a foot) or an end (vertex to vertex).
            let t = ((v.p - s.a).dot(e) / (l * l)).clamp(0.0, 1.0);
            let t = if t * l <= eps { 0.0 } else if t * l >= l - eps { 1.0 } else { t };
            let at = s.a + e * t;
            let d = (v.p - at).len();
            if d >= max_d || d < 1.0 {
                continue;
            }
            let f = Foot { v: vi, side: si as usize, t, at, d };
            match best.iter_mut().find(|b| b.0 == s.owner) {
                Some(b) => {
                    if d < b.1.d {
                        b.1 = f;
                    }
                }
                None => best.push((s.owner, f)),
            }
        }
        cand.extend(best.into_iter().map(|b| b.1));
    }
    // Nearest first so a crossing keeps the tighter cut.
    cand.sort_by(|a, b| a.d.total_cmp(&b.d).then(a.v.cmp(&b.v)).then(a.side.cmp(&b.side)));
    let mut kept: Vec<Foot> = Vec::new();
    let mut feet_on: Vec<Vec<f64>> = vec![Vec::new(); sides.len()];
    // The crossing test against kept cuts uses a grid over the candidates' boxes (all kept cuts
    // are candidates); rebuild lazily by testing the kept list through a map cell -> kept ids.
    // Kept cuts by cell (a cut is registered in every cell its box overlaps).
    let kgrid = Grid::new(&[(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 0.0, y: 0.0 })], max_d.max(1.0));
    let mut kcells: std::collections::BTreeMap<(i64, i64), Vec<u32>> = std::collections::BTreeMap::new();
    let cell_of = |p: Vec2| ((p.x / max_d).floor() as i64, (p.y / max_d).floor() as i64);
    let _ = kgrid;
    let obox: Vec<(Vec2, Vec2)> = polys
        .iter()
        .map(|p| {
            let mut lo = Vec2 { x: f64::INFINITY, y: f64::INFINITY };
            let mut hi = Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY };
            for q in p.iter() {
                lo = Vec2 { x: lo.x.min(q.x), y: lo.y.min(q.y) };
                hi = Vec2 { x: hi.x.max(q.x), y: hi.y.max(q.y) };
            }
            (lo, hi)
        })
        .collect();

    for c in cand.iter() {
        let (vp, f) = (verts[c.v].p, c.at);
        let s = &sides[c.side];
        let ovn = verts[c.v].owner;
        // The side's own feet keep their distance (vertex targets are not feet).
        let along = c.t * (s.b - s.a).len();
        let vertex = c.t == 0.0 || c.t == 1.0;
        if !vertex && feet_on[c.side].iter().any(|&a| (a - along).abs() < eps) {
            continue;
        }
        let (c0, c1) = (cell_of(Vec2 { x: vp.x.min(f.x), y: vp.y.min(f.y) }), cell_of(Vec2 { x: vp.x.max(f.x), y: vp.y.max(f.y) }));
        let mut near_kept: Vec<u32> = Vec::new();
        for cy in c0.1..=c1.1 {
            for cx in c0.0..=c1.0 {
                if let Some(v) = kcells.get(&(cx, cy)) {
                    near_kept.extend_from_slice(v);
                }
            }
        }
        near_kept.sort_unstable();
        near_kept.dedup();
        if near_kept.iter().any(|&k| {
            let (kp, kf) = (verts[kept[k as usize].v].p, kept[k as usize].at);
            ((kp - vp).len() < 1.0 && (kf - f).len() < 1.0) || ((kp - f).len() < 1.0 && (kf - vp).len() < 1.0)
        }) {
            continue;
        }
        // Free: no other side touches the segment (sides incident to `v` meet it only there).
        let mid = (vp + f) * 0.5;
        let (sl, sh) = (Vec2 { x: vp.x.min(f.x), y: vp.y.min(f.y) }, Vec2 { x: vp.x.max(f.x), y: vp.y.max(f.y) });
        let mut free = true;
        for &si in grid.near(mid) {
            let o = &sides[si as usize];
            if si as usize == c.side {
                continue;
            }
            if o.a.x.min(o.b.x) > sh.x + 1.0 || o.a.x.max(o.b.x) < sl.x - 1.0 || o.a.y.min(o.b.y) > sh.y + 1.0 || o.a.y.max(o.b.y) < sl.y - 1.0 {
                continue;
            }
            if (o.a - vp).len() < 1.0 || (o.b - vp).len() < 1.0 || (o.a - f).len() < 1.0 || (o.b - f).len() < 1.0 {
                continue;
            }
            if touch(vp, f, o.a, o.b) {
                free = false;
                break;
            }
        }
        if !free {
            continue;
        }
        // Not through copper: the midpoint lies in no polygon of the nearby owners.
        let mut inside = false;
        let mut last_owner = usize::MAX;
        for &si in grid.near(mid) {
            if let Some(o) = sides[si as usize].owner {
                if o == last_owner {
                    continue;
                }
                last_owner = o;
                let (lo, hi) = obox[o];
                let pin = |q: Vec2| q.x >= lo.x && q.x <= hi.x && q.y >= lo.y && q.y <= hi.y && point_in_poly(q, polys[o]);
                if pin(mid) || pin(f) && Some(o) != s.owner {
                    inside = true;
                    break;
                }
            }
        }
        if inside || ovn.is_some_and(|o| point_in_poly(mid, polys[o])) {
            continue;
        }
        // A kept cut crossing this one would split both.
        let mut crosses = false;
        for &k in &near_kept {
            let k = k as usize;
            let (kp, kf) = (verts[kept[k].v].p, kept[k].at);
            if (kp - vp).len() < 1.0 || (kf - f).len() < 1.0 {
                continue;
            }
            if touch(vp, f, kp, kf) {
                crosses = true;
                break;
            }
        }
        if crosses {
            continue;
        }
        for cy in c0.1..=c1.1 {
            for cx in c0.0..=c1.0 {
                kcells.entry((cx, cy)).or_default().push(kept.len() as u32);
            }
        }
        if !vertex {
            feet_on[c.side].push(along);
        }
        kept.push(*c);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x: x * 1e6, y: y * 1e6 }
    }

    /// Two squares 0.5 mm apart, offset by half a side: the facing corners project into the
    /// other square's side.
    #[test]
    fn offset_squares_get_two_feet() {
        let a = [v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)];
        let b = [v(0.5, 1.5), v(1.5, 1.5), v(1.5, 2.5), v(0.5, 2.5)];
        let polys: Vec<&[Vec2]> = vec![&a, &b];
        let mut verts = Vec::new();
        let mut sides = Vec::new();
        for (o, p) in [&a, &b].iter().enumerate() {
            for i in 0..4 {
                verts.push(Vertex { p: p[i], owner: Some(o) });
                sides.push(Side { a: p[i], b: p[(i + 1) % 4], owner: Some(o) });
            }
        }
        let f = lct_feet(&verts, &sides, &polys, &|_, _| true, 3e6, 1e4);
        // a's (1,1) corner onto b's bottom side is not interior (it is at x=1 inside 0.5..1.5):
        // yes interior; b's (0.5,1.5) onto a's top side (x=0.5 inside 0..1): interior.
        assert_eq!(f.len(), 2, "{f:?}");
        assert!(f.iter().all(|x| (x.d - 0.5e6).abs() < 1.0));
    }

    #[test]
    fn blocked_feet_are_dropped() {
        let a = [v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)];
        let b = [v(0.0, 3.0), v(1.0, 3.0), v(1.0, 4.0), v(0.0, 4.0)];
        // A pad between them across the facing corners.
        let m = [v(0.2, 1.5), v(0.8, 1.5), v(0.8, 2.0), v(0.2, 2.0)];
        let polys: Vec<&[Vec2]> = vec![&a, &b, &m];
        let mut verts = Vec::new();
        let mut sides = Vec::new();
        for (o, p) in [&a, &b, &m].iter().enumerate() {
            for i in 0..4 {
                verts.push(Vertex { p: p[i], owner: Some(o) });
                sides.push(Side { a: p[i], b: p[(i + 1) % 4], owner: Some(o) });
            }
        }
        let f = lct_feet(&verts, &sides, &polys, &|_, _| true, 3e6, 1e4);
        // a and b are aligned: their feet are the ends, i.e. vertex-to-vertex cuts along the free
        // sides. The interior feet are those of the middle pad m with a and b.
        assert!(f.iter().filter(|x| x.t > 0.0 && x.t < 1.0).all(|x| verts[x.v].owner == Some(2) || sides[x.side].owner == Some(2)), "{f:?}");
        assert_eq!(f.iter().filter(|x| x.t > 0.0 && x.t < 1.0).count(), 4, "{f:?}");
        // The cut through m itself (a's top-left corner to b's bottom-left corner at x = 0) is
        // free, the one at the middle of the pad is not there.
        assert!(f.iter().all(|x| !(verts[x.v].owner == Some(0) && sides[x.side].owner == Some(1) && x.t > 0.0 && x.t < 1.0)));
    }
}
