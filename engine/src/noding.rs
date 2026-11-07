//! Noding of constraint segments by snap rounding: the input of a constrained triangulation
//! made free of crossings before it reaches spade (see [`crate::cdt::Cdt::build`]).
//!
//! Method: snap rounding with hot pixels (J. D. Hobby, "Practical segment intersection with
//! finite precision output", Computational Geometry 13(4), 1999, pp. 199-214; L. Guibas and
//! D. Marimont, "Rounding arrangements dynamically", IJCGA 8(2), 1998, pp. 157-176), as the
//! JTS / GEOS `SnapRoundingNoder` implements it:
//! 1. every point lies on an integer grid (the caller snaps it; [`crate::cdt::NODE_GRID_NM`]);
//! 2. the proper crossings of the segments are found (candidate pairs from a uniform grid of
//!    cells, the segment-intersection problem of de Berg et al., *Computational Geometry*, 3rd
//!    ed., ch. 2, here with buckets instead of the plane sweep) and rounded to the grid exactly
//!    (`i128` rationals);
//! 3. every grid point that is a vertex or a rounded crossing is a *hot pixel* (the unit square
//!    around it); every segment that meets a hot pixel is re-routed through its centre;
//! 4. the pieces between consecutive hot pixels of a segment are the output.
//!
//! The rounded arrangement has no proper crossings (the topological guarantee of snap rounding
//! proved by Guibas & Marimont and by Hobby): two pieces meet only at hot-pixel centres or overlap. All predicates are exact (`i128`
//! orientation tests on integer coordinates), so the result does not depend on rounding.
//! The pieces are checked once more for crossings; a crossing found there (none expected) adds
//! its pixel and the rounding runs again, up to [`MAX_ROUNDS`] times.
//!
//! Coordinates are integers with |x|, |y| ≤ [`MAX_COORD`] (2^31 grid units: 2.1 m at 1 nm), so
//! every product below fits an `i128`. Complexity: O((n + k + h) · c) for n segments, k
//! crossings, h hot pixels and c segments per grid cell (small for board geometry).

use std::collections::{HashMap, HashSet};

/// Largest coordinate magnitude (grid units).
pub const MAX_COORD: i64 = 1 << 31;

/// Rounds of "pixels → pieces → crossing check" before giving up on a crossing-free result.
pub const MAX_ROUNDS: usize = 4;

pub type IPt = [i64; 2];

/// The noded segments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Noded {
    /// Hot pixels that are no input point (rounded crossings), sorted; point id
    /// `pts.len() + i` is `new_pts[i]`.
    pub new_pts: Vec<IPt>,
    /// Per input segment, the point ids it passes through in order (first = its start, last =
    /// its end); consecutive ids differ.
    pub chains: Vec<Vec<usize>>,
    /// Proper crossings found in the input segments.
    pub crossings: usize,
    /// Rounds run (1 = the first rounding was crossing-free).
    pub rounds: usize,
    /// Proper crossings still left after the last round (0 expected).
    pub left_crossings: usize,
}

fn orient(a: IPt, b: IPt, c: IPt) -> i128 {
    (b[0] - a[0]) as i128 * (c[1] - a[1]) as i128 - (b[1] - a[1]) as i128 * (c[0] - a[0]) as i128
}

/// Nearest integer to `n / d` (`d > 0`), halves up.
fn round_div(n: i128, d: i128) -> i64 {
    (2 * n + d).div_euclid(2 * d) as i64
}

/// The rounded point where segments `a b` and `c d` cross properly, or `None`.
pub fn proper_crossing(a: IPt, b: IPt, c: IPt, d: IPt) -> Option<IPt> {
    let (d1, d2) = (orient(a, b, c), orient(a, b, d));
    let (d3, d4) = (orient(c, d, a), orient(c, d, b));
    if !((d1 > 0 && d2 < 0) || (d1 < 0 && d2 > 0)) || !((d3 > 0 && d4 < 0) || (d3 < 0 && d4 > 0)) {
        return None;
    }
    // a + r * num / den with r = b - a, s = d - c: den = r x s, num = (c - a) x s.
    let r = [(b[0] - a[0]) as i128, (b[1] - a[1]) as i128];
    let s = [(d[0] - c[0]) as i128, (d[1] - c[1]) as i128];
    let mut den = r[0] * s[1] - r[1] * s[0];
    let ca = [(c[0] - a[0]) as i128, (c[1] - a[1]) as i128];
    let mut num = ca[0] * s[1] - ca[1] * s[0];
    if den < 0 {
        den = -den;
        num = -num;
    }
    Some([a[0] + round_div(r[0] * num, den), a[1] + round_div(r[1] * num, den)])
}

/// Whether segment `a b` meets the closed unit square centred on grid point `h`. Exact: doubled
/// coordinates make the square corners integers.
pub fn meets_pixel(a: IPt, b: IPt, h: IPt) -> bool {
    let (a2, b2, h2) = ([2 * a[0], 2 * a[1]], [2 * b[0], 2 * b[1]], [2 * h[0], 2 * h[1]]);
    if a2[0].min(b2[0]) > h2[0] + 1 || a2[0].max(b2[0]) < h2[0] - 1 || a2[1].min(b2[1]) > h2[1] + 1 || a2[1].max(b2[1]) < h2[1] - 1 {
        return false;
    }
    let (mut pos, mut neg) = (false, false);
    for (dx, dy) in [(-1, -1), (1, -1), (1, 1), (-1, 1)] {
        let o = orient(a2, b2, [h2[0] + dx, h2[1] + dy]);
        pos |= o >= 0;
        neg |= o <= 0;
    }
    pos && neg
}

/// Uniform grid of cells over segments; a segment is listed in every cell it may pass through.
struct Grid {
    x0: i64,
    y0: i64,
    cs: i64,
    nx: i64,
    ny: i64,
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(pts: &[IPt], segs: &[(usize, usize)]) -> Grid {
        let (mut lo, mut hi) = ([i64::MAX; 2], [i64::MIN; 2]);
        for &(a, b) in segs {
            for p in [pts[a], pts[b]] {
                lo = [lo[0].min(p[0]), lo[1].min(p[1])];
                hi = [hi[0].max(p[0]), hi[1].max(p[1])];
            }
        }
        if segs.is_empty() {
            lo = [0, 0];
            hi = [0, 0];
        }
        let (w, h) = ((hi[0] - lo[0] + 1) as f64, (hi[1] - lo[1] + 1) as f64);
        // About two segments per cell, at most 4M cells, at least 16 grid units per cell.
        let n = segs.len().max(1) as f64;
        let cs = ((w * h / (2.0 * n)).sqrt().max((w * h / 4.0e6).sqrt()).max(16.0)).ceil() as i64;
        let (nx, ny) = ((hi[0] - lo[0]) / cs + 1, (hi[1] - lo[1]) / cs + 1);
        let mut g = Grid { x0: lo[0], y0: lo[1], cs, nx, ny, cells: vec![Vec::new(); (nx * ny) as usize] };
        for (i, &(a, b)) in segs.iter().enumerate() {
            g.add(i as u32, pts[a], pts[b]);
        }
        g
    }

    fn cx(&self, x: i64) -> i64 {
        ((x - self.x0).div_euclid(self.cs)).clamp(0, self.nx - 1)
    }

    fn cy(&self, y: i64) -> i64 {
        ((y - self.y0).div_euclid(self.cs)).clamp(0, self.ny - 1)
    }

    /// The cells of each column the segment spans, from its y range inside the column (padded by
    /// 2 grid units against the float evaluation).
    fn add(&mut self, i: u32, a: IPt, b: IPt) {
        let (c0, c1) = (self.cx(a[0].min(b[0])), self.cx(a[0].max(b[0])));
        for c in c0..=c1 {
            let (xl, xr) = ((self.x0 + c * self.cs).max(a[0].min(b[0])), (self.x0 + (c + 1) * self.cs).min(a[0].max(b[0])));
            let (ylo, yhi) = if a[0] == b[0] {
                (a[1].min(b[1]), a[1].max(b[1]))
            } else {
                let y = |x: i64| a[1] as f64 + (b[1] - a[1]) as f64 * ((x - a[0]) as f64 / (b[0] - a[0]) as f64);
                let (u, v) = (y(xl), y(xr));
                ((u.min(v) - 2.0).floor() as i64, (u.max(v) + 2.0).ceil() as i64)
            };
            for r in self.cy(ylo.max(a[1].min(b[1])))..=self.cy(yhi.min(a[1].max(b[1]))) {
                self.cells[(r * self.nx + c) as usize].push(i);
            }
        }
    }

    /// Segments listed in the cells meeting the box `lo..hi`, appended to `out` (may repeat).
    fn query(&self, lo: IPt, hi: IPt, out: &mut Vec<u32>) {
        for r in self.cy(lo[1])..=self.cy(hi[1]) {
            for c in self.cx(lo[0])..=self.cx(hi[0]) {
                out.extend_from_slice(&self.cells[(r * self.nx + c) as usize]);
            }
        }
    }
}

/// The rounded proper crossings of `segs` (each pair tested where they share a cell), sorted and
/// deduplicated, and the number of crossing pairs.
fn crossings(pts: &[IPt], segs: &[(usize, usize)]) -> (Vec<IPt>, usize) {
    let g = Grid::new(pts, segs);
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    let mut out: Vec<IPt> = Vec::new();
    for cell in &g.cells {
        for (x, &i) in cell.iter().enumerate() {
            for &j in &cell[x + 1..] {
                let (si, sj) = (segs[i as usize], segs[j as usize]);
                let Some(p) = proper_crossing(pts[si.0], pts[si.1], pts[sj.0], pts[sj.1]) else { continue };
                if seen.insert((i.min(j), i.max(j))) {
                    out.push(p);
                }
            }
        }
    }
    let n = seen.len();
    out.sort_unstable();
    out.dedup();
    (out, n)
}

/// Snap rounding of `segs` (index pairs into `pts`, integer points with |coordinate| ≤
/// [`MAX_COORD`]; zero-length segments allowed). Every point of `pts` is a hot pixel. See the
/// module doc.
pub fn snap_round(pts: &[IPt], segs: &[(usize, usize)]) -> Noded {
    let mut id: HashMap<IPt, usize> = HashMap::with_capacity(pts.len());
    for (i, &p) in pts.iter().enumerate() {
        id.entry(p).or_insert(i);
    }
    let grid = Grid::new(pts, segs);
    let (first, n_cross) = crossings(pts, segs);
    let mut hot_new: Vec<IPt> = first.into_iter().filter(|p| !id.contains_key(p)).collect();
    let mut out = Noded { crossings: n_cross, ..Noded::default() };
    let mut buf: Vec<u32> = Vec::new();
    for round in 1..=MAX_ROUNDS {
        hot_new.sort_unstable();
        hot_new.dedup();
        let mut all: Vec<IPt> = pts.to_vec();
        all.extend(hot_new.iter().copied());
        // Hot pixels: the distinct points (first id of each position).
        let mut on_seg: Vec<Vec<usize>> = vec![Vec::new(); segs.len()];
        let mut done: HashSet<IPt> = HashSet::with_capacity(all.len());
        for (pid, &h) in all.iter().enumerate() {
            if !done.insert(h) {
                continue;
            }
            buf.clear();
            grid.query([h[0] - 1, h[1] - 1], [h[0] + 1, h[1] + 1], &mut buf);
            buf.sort_unstable();
            buf.dedup();
            for &s in &buf {
                let (a, b) = (pts[segs[s as usize].0], pts[segs[s as usize].1]);
                if h != a && h != b && meets_pixel(a, b, h) {
                    on_seg[s as usize].push(pid);
                }
            }
        }
        let mut chains = Vec::with_capacity(segs.len());
        for (s, &(ia, ib)) in segs.iter().enumerate() {
            let (a, b) = (pts[ia], pts[ib]);
            let (ca, cb) = (id[&a], id[&b]);
            let d = [(b[0] - a[0]) as i128, (b[1] - a[1]) as i128];
            let mut mid: Vec<(i128, IPt, usize)> = on_seg[s]
                .iter()
                .map(|&p| {
                    let h = all[p];
                    ((h[0] - a[0]) as i128 * d[0] + (h[1] - a[1]) as i128 * d[1], h, p)
                })
                .collect();
            mid.sort_unstable();
            let mut chain = vec![ca];
            chain.extend(mid.iter().map(|m| m.2));
            chain.push(cb);
            chain.dedup();
            chains.push(chain);
        }
        // The pieces, checked for crossings among themselves.
        let mut pieces: Vec<(usize, usize)> = chains.iter().flat_map(|c| c.windows(2).map(|w| (w[0].min(w[1]), w[0].max(w[1])))).collect();
        pieces.sort_unstable();
        pieces.dedup();
        let (left, n_left) = crossings(&all, &pieces);
        out.rounds = round;
        out.left_crossings = n_left;
        out.chains = chains;
        out.new_pts = hot_new.clone();
        if n_left == 0 {
            break;
        }
        let before = hot_new.len();
        hot_new.extend(left.into_iter().filter(|p| !id.contains_key(p)));
        hot_new.sort_unstable();
        hot_new.dedup();
        if hot_new.len() == before {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_is_rounded_exactly() {
        assert_eq!(proper_crossing([0, 0], [10, 10], [0, 10], [10, 0]), Some([5, 5]));
        assert_eq!(proper_crossing([0, 0], [3, 1], [0, 1], [3, 0]), Some([2, 1]));
        // Touching at an end point is no proper crossing.
        assert_eq!(proper_crossing([0, 0], [10, 0], [5, 0], [5, 5]), None);
        // Huge coordinates: no overflow.
        let m = MAX_COORD;
        assert_eq!(proper_crossing([-m, -m], [m, m], [-m, m], [m, -m]), Some([0, 0]));
    }

    #[test]
    fn pixel_test_is_closed() {
        assert!(meets_pixel([0, 0], [10, 0], [5, 0]));
        assert!(!meets_pixel([0, 1], [10, 1], [5, 0]));
        // Through the corner (5.5, 0.5) only.
        assert!(meets_pixel([4, 2], [7, -1], [5, 0]));
        assert!(!meets_pixel([4, 3], [7, 0], [5, 0]));
    }

    #[test]
    fn two_crossing_segments_share_a_vertex() {
        let pts = [[0, 0], [10, 10], [0, 10], [10, 0]];
        let n = snap_round(&pts, &[(0, 1), (2, 3)]);
        assert_eq!(n.new_pts, vec![[5, 5]]);
        assert_eq!(n.chains, vec![vec![0, 4, 1], vec![2, 4, 3]]);
        assert_eq!(n.crossings, 1);
        assert_eq!(n.left_crossings, 0);
    }

    #[test]
    fn segment_passing_near_a_vertex_goes_through_it() {
        // (5, 0) lies on the first segment; the second one ends there.
        let pts = [[0, 0], [10, 0], [5, 0], [5, 7]];
        let n = snap_round(&pts, &[(0, 1), (2, 3)]);
        assert!(n.new_pts.is_empty());
        assert_eq!(n.chains[0], vec![0, 2, 1]);
    }
}
