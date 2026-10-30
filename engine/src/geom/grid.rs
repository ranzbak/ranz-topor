//! Uniform grid spatial index (23 §3).
//!
//! Cell size rule for callers: `cell = max_width + max_clearance` of the model, so that
//! any two items closer than the clearance live in the same or adjacent cells.

use super::prim::Aabb;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::Hash;

/// KiCad world range: coordinates are `i32` nm, so |coord| <= 2^31.
const WORLD: f64 = 2_147_483_648.0;
/// Items covering more cells than this go to a side list returned by every query.
const MAX_ITEM_CELLS: i64 = 1024;

/// Error returned by [`Grid::try_insert`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridError {
    /// The bounding box has a NaN or infinite coordinate.
    NonFinite,
}

impl fmt::Display for GridError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GridError::NonFinite => write!(f, "bounding box has a non-finite coordinate"),
        }
    }
}

impl std::error::Error for GridError {}

type Range = (i64, i64, i64, i64);

/// Uniform grid keyed by `floor(coord / cell)`. An item is stored in every cell its box covers;
/// items covering more than `MAX_ITEM_CELLS` cells live in a side list instead.
pub struct Grid<T: Copy + Eq + Hash> {
    cell: f64,
    cells: HashMap<(i64, i64), Vec<T>>,
    big: Vec<T>,
    len: usize,
}

fn finite(b: &Aabb) -> bool {
    b.min.x.is_finite() && b.min.y.is_finite() && b.max.x.is_finite() && b.max.y.is_finite()
}

impl<T: Copy + Eq + Hash> Grid<T> {
    pub fn new(cell: f64) -> Self {
        assert!(cell > 0.0 && cell.is_finite(), "cell size must be positive");
        Grid {
            cell,
            cells: HashMap::new(),
            big: Vec::new(),
            len: 0,
        }
    }

    /// Cell index of a finite coordinate, clamped to the world range (i64, no overflow).
    fn idx(&self, v: f64) -> i64 {
        (v.clamp(-WORLD, WORLD) / self.cell).floor() as i64
    }

    /// Inclusive cell range of a finite box (normalised), grown by `pad` cells per side.
    fn range(&self, b: &Aabb, pad: i64) -> Range {
        let (ax, bx) = (b.min.x.min(b.max.x), b.min.x.max(b.max.x));
        let (ay, by) = (b.min.y.min(b.max.y), b.min.y.max(b.max.y));
        (
            self.idx(ax) - pad,
            self.idx(bx) + pad,
            self.idx(ay) - pad,
            self.idx(by) + pad,
        )
    }

    fn count((x0, x1, y0, y1): Range) -> i64 {
        // each side <= ~4.3e9 / cell + 3; saturate to stay overflow-free for tiny cells
        (x1 - x0 + 1).saturating_mul(y1 - y0 + 1)
    }

    /// Insert `item`, panicking if `bbox` is non-finite (an obstacle silently dropped would
    /// hide DRC violations). Use [`Grid::try_insert`] to handle the error.
    pub fn insert(&mut self, bbox: Aabb, item: T) {
        if let Err(e) = self.try_insert(bbox, item) {
            panic!("Grid::insert: {e}");
        }
    }

    pub fn try_insert(&mut self, bbox: Aabb, item: T) -> Result<(), GridError> {
        if !finite(&bbox) {
            return Err(GridError::NonFinite);
        }
        let r = self.range(&bbox, 0);
        if Self::count(r) > MAX_ITEM_CELLS {
            self.big.push(item);
        } else {
            for x in r.0..=r.1 {
                for y in r.2..=r.3 {
                    self.cells.entry((x, y)).or_default().push(item);
                }
            }
        }
        self.len += 1;
        Ok(())
    }

    /// Remove `item` previously inserted with the same `bbox`. Returns `false` (and changes
    /// nothing) if the item is not found in every cell `bbox` covers, e.g. a mismatched box.
    pub fn remove(&mut self, bbox: Aabb, item: T) -> bool {
        if !finite(&bbox) {
            return false;
        }
        let r = self.range(&bbox, 0);
        if Self::count(r) > MAX_ITEM_CELLS {
            return match self.big.iter().position(|t| *t == item) {
                Some(p) => {
                    self.big.swap_remove(p);
                    self.len -= 1;
                    true
                }
                None => false,
            };
        }
        let has =
            |g: &Self, x: i64, y: i64| g.cells.get(&(x, y)).is_some_and(|v| v.contains(&item));
        for x in r.0..=r.1 {
            for y in r.2..=r.3 {
                if !has(self, x, y) {
                    debug_assert!(false, "Grid::remove: bbox does not match insert");
                    return false;
                }
            }
        }
        for x in r.0..=r.1 {
            for y in r.2..=r.3 {
                if let Some(v) = self.cells.get_mut(&(x, y)) {
                    if let Some(p) = v.iter().position(|t| *t == item) {
                        v.swap_remove(p);
                    }
                    if v.is_empty() {
                        self.cells.remove(&(x, y));
                    }
                }
            }
        }
        self.len -= 1;
        true
    }

    /// Deduplicated items in the cells overlapped by `bbox` grown by one cell per side
    /// (the 3x3 neighbourhood of 23 §3), plus all oversized items. A non-finite `bbox`
    /// yields an empty result. Cost is O(min(cells in box, occupied cells)).
    pub fn query(&self, bbox: Aabb) -> Vec<T> {
        self.query_counted(bbox).0
    }

    /// Like [`Grid::query`], also returning the number of candidate entries visited
    /// (before deduplication); used by the scaling test.
    pub fn query_counted(&self, bbox: Aabb) -> (Vec<T>, usize) {
        let mut out = Vec::new();
        if !finite(&bbox) {
            return (out, 0);
        }
        let r = self.range(&bbox, 1);
        let mut seen = HashSet::new();
        let mut visited = 0;
        let mut take = |v: &Vec<T>, out: &mut Vec<T>, visited: &mut usize| {
            *visited += v.len();
            for &t in v {
                if seen.insert(t) {
                    out.push(t);
                }
            }
        };
        if Self::count(r) <= self.cells.len() as i64 {
            for x in r.0..=r.1 {
                for y in r.2..=r.3 {
                    if let Some(v) = self.cells.get(&(x, y)) {
                        take(v, &mut out, &mut visited);
                    }
                }
            }
        } else {
            // box covers more cells than are occupied: scan occupied cells (sorted: deterministic)
            let mut keys: Vec<&(i64, i64)> = self
                .cells
                .keys()
                .filter(|(x, y)| (r.0..=r.1).contains(x) && (r.2..=r.3).contains(y))
                .collect();
            keys.sort_unstable();
            for k in keys {
                take(&self.cells[k], &mut out, &mut visited);
            }
        }
        take(&self.big, &mut out, &mut visited);
        (out, visited)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}
