//! Uniform grid spatial index (23 §3).
//!
//! Cell size rule for callers: `cell = max_width + max_clearance` of the model, so that
//! any two items closer than the clearance live in the same or adjacent cells.

use super::prim::Aabb;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// Uniform grid keyed by `floor(coord / cell)`. An item is stored in every cell its box covers.
pub struct Grid<T: Copy + Eq + Hash> {
    cell: f64,
    cells: HashMap<(i32, i32), Vec<T>>,
    len: usize,
}

impl<T: Copy + Eq + Hash> Grid<T> {
    pub fn new(cell: f64) -> Self {
        assert!(cell > 0.0 && cell.is_finite(), "cell size must be positive");
        Grid {
            cell,
            cells: HashMap::new(),
            len: 0,
        }
    }

    fn idx(&self, v: f64) -> i32 {
        (v / self.cell).floor() as i32
    }

    fn range(&self, b: Aabb, pad: i32) -> (i32, i32, i32, i32) {
        (
            self.idx(b.min.x) - pad,
            self.idx(b.max.x) + pad,
            self.idx(b.min.y) - pad,
            self.idx(b.max.y) + pad,
        )
    }

    pub fn insert(&mut self, bbox: Aabb, item: T) {
        let (x0, x1, y0, y1) = self.range(bbox, 0);
        for x in x0..=x1 {
            for y in y0..=y1 {
                self.cells.entry((x, y)).or_default().push(item);
            }
        }
        self.len += 1;
    }

    /// Remove `item` previously inserted with the same `bbox`.
    pub fn remove(&mut self, bbox: Aabb, item: T) {
        let (x0, x1, y0, y1) = self.range(bbox, 0);
        let mut found = false;
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(v) = self.cells.get_mut(&(x, y)) {
                    if let Some(p) = v.iter().position(|t| *t == item) {
                        v.swap_remove(p);
                        found = true;
                    }
                    if v.is_empty() {
                        self.cells.remove(&(x, y));
                    }
                }
            }
        }
        if found {
            self.len -= 1;
        }
    }

    /// Deduplicated items in the cells overlapped by `bbox` grown by one cell per side
    /// (the 3x3 neighbourhood of 23 §3). Cost is O(items in those cells).
    pub fn query(&self, bbox: Aabb) -> Vec<T> {
        self.query_counted(bbox).0
    }

    /// Like [`Grid::query`], also returning the number of candidate entries visited
    /// (before deduplication); used by the scaling test.
    pub fn query_counted(&self, bbox: Aabb) -> (Vec<T>, usize) {
        let (x0, x1, y0, y1) = self.range(bbox, 1);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let mut visited = 0;
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(v) = self.cells.get(&(x, y)) {
                    visited += v.len();
                    for &t in v {
                        if seen.insert(t) {
                            out.push(t);
                        }
                    }
                }
            }
        }
        (out, visited)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}
