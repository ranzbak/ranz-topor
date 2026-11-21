//! grid-snap (docs/dev/design/grid-snap.md): the user's KiCad grid as a graded price on the drawn
//! copper's bends and vias. The pad snap wins over the grid, as in KiCad (`pcb_grid_helper.cpp`
//! `computeAnchors`: a pad centre is an ORIGIN | SNAPPABLE anchor of the object tier): a run's
//! ends (pad centres, vias, T points) are never priced. A wire goes off the grid wherever
//! clearance, the planned room of others, the 45 degree form or the bundle packing need it, since
//! every one of those is priced far above `off_grid` (0.15 mm per bend fully off the grid, below
//! one 45 degree bend's 0.25 mm). This is graded, with no cutoff.
//!
//! The grid comes from the engine options (`grid_track_nm`, `grid_via_nm`, `grid_origin_nm`;
//! the plugin reads them from topor.toml `[drawing] grid_mm` or KiCad's own pcbnew.json, with the
//! board's grid origin), in the KiCad frame. [`Lattice::from_opts`] turns them into the math frame
//! (`y` negated, as [`crate::model::BoardModel::from_json`] does).
//!
//! Complexity: every query is O(1).

use crate::geom::Vec2;

/// `[drawing] grid_snap`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// No grid price and no grid nodes: the copper is exactly the copper without the feature.
    Off,
    /// Grid nodes only in a second octilinear pass, when the first one leaves an any-angle stretch
    /// or a forced violation (the "last spacing fallback", user 2026-11-17).
    Fallback,
    /// Grid nodes and the grid price in every octilinear pass.
    Prefer,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s {
            "prefer" => Mode::Prefer,
            "fallback" => Mode::Fallback,
            _ => Mode::Off,
        }
    }
}

/// A rectangular grid in the math frame: lattice points `o + (i gx, j gy)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lattice {
    pub g: [f64; 2],
    pub o: [f64; 2],
}

impl Lattice {
    /// The lattice of `step` (KiCad frame, nm, both > 0) and `origin` (KiCad frame, nm), in the
    /// math frame. None without a step.
    pub fn new(step: [i64; 2], origin: [i64; 2]) -> Option<Lattice> {
        (step[0] > 0 && step[1] > 0).then(|| Lattice { g: [step[0] as f64, step[1] as f64], o: [origin[0] as f64, -(origin[1] as f64)] })
    }

    /// The track and via lattices of the options, when `grid_snap` is not off and a step is set
    /// (the via grid falls back to the track grid).
    pub fn from_opts(o: &crate::model::Options) -> (Option<Lattice>, Option<Lattice>) {
        if Mode::parse(&o.grid_snap) == Mode::Off {
            return (None, None);
        }
        let t = Lattice::new(o.grid_track_nm, o.grid_origin_nm);
        let v = Lattice::new(o.grid_via_nm, o.grid_origin_nm).or(t);
        (t, v)
    }

    /// The lattice point nearest to `p`.
    pub fn nearest(&self, p: Vec2) -> Vec2 {
        let f = |x: f64, g: f64, o: f64| o + ((x - o) / g).round() * g;
        Vec2 { x: f(p.x, self.g[0], self.o[0]), y: f(p.y, self.g[1], self.o[1]) }
    }

    /// The four lattice points of the cell around `p` (lower-left first, counter-clockwise).
    pub fn cell(&self, p: Vec2) -> [Vec2; 4] {
        let x0 = self.o[0] + ((p.x - self.o[0]) / self.g[0]).floor() * self.g[0];
        let y0 = self.o[1] + ((p.y - self.o[1]) / self.g[1]).floor() * self.g[1];
        let (x1, y1) = (x0 + self.g[0], y0 + self.g[1]);
        [Vec2 { x: x0, y: y0 }, Vec2 { x: x1, y: y0 }, Vec2 { x: x1, y: y1 }, Vec2 { x: x0, y: y1 }]
    }

    /// Distance from `p` to the nearest lattice point (nm).
    pub fn off(&self, p: Vec2) -> f64 {
        (self.nearest(p) - p).len()
    }

    /// The graded off-grid amount of `p`: its distance to the nearest lattice point over the
    /// largest such distance (half the cell diagonal); 0 on the grid, 1 at a cell centre. Below
    /// [`ON_TOL_NM`] it counts as on the grid (KiCad stores integer nm).
    pub fn amount(&self, p: Vec2) -> f64 {
        let d = self.off(p);
        if d <= ON_TOL_NM {
            return 0.0;
        }
        (d / (0.5 * self.g[0].hypot(self.g[1]))).min(1.0)
    }

    /// Whether `p` is on the grid (within [`ON_TOL_NM`]).
    pub fn on(&self, p: Vec2) -> bool {
        self.off(p) <= ON_TOL_NM
    }

    /// Square grid (the only one on which every elbow between two lattice points bends on the
    /// lattice: a 45 degree leg moves as far in x as in y).
    pub fn square(&self) -> bool {
        (self.g[0] - self.g[1]).abs() < 0.5
    }

    /// The length step along octilinear direction `d` (unit) that moves a lattice point to another
    /// one: `gx` along an axis, `gx sqrt 2` along a diagonal of a square grid. None on a diagonal
    /// of a rectangular grid.
    pub fn step_along(&self, d: Vec2) -> Option<f64> {
        if d.y.abs() < 1e-9 {
            Some(self.g[0])
        } else if d.x.abs() < 1e-9 {
            Some(self.g[1])
        } else if self.square() {
            Some(self.g[0] * std::f64::consts::SQRT_2)
        } else {
            None
        }
    }
}

/// The `off_grid` price (nm per vertex fully off the grid): the factor's unfluctuated weight (the
/// drawing's prices stay put across cycles, as [`super::octi::OctiCfg::from_opts`]).
pub fn price_nm(o: &crate::model::Options) -> f64 {
    let reg = super::factors::Registry::from_config(super::factors::standard_factors(), &o.factors).0;
    reg.weight("off_grid").unwrap_or(OFF_GRID_MM).max(0.0) * 1e6
}

/// The via lattice and price the via relocation uses: only with `grid_snap = "prefer"` (the
/// fallback mode is a second octilinear pass only).
pub fn via_grid(o: &crate::model::Options) -> Option<(Lattice, f64)> {
    (Mode::parse(&o.grid_snap) == Mode::Prefer).then(|| Lattice::from_opts(o).1).flatten().map(|l| (l, price_nm(o)))
}

/// On-grid tolerance (nm): the drawn coordinates are rounded to integer nm on output, so a point
/// within this of a lattice point lands on it.
pub const ON_TOL_NM: f64 = 1.0;

/// Default `off_grid` weight (mm per bend fully off the grid): below one 45 degree bend
/// (`[drawing] bend_mm` 0.25), so the grid never buys a bend or a jog.
pub const OFF_GRID_MM: f64 = 0.15;

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x, y }
    }

    /// The lattice in the math frame: y of the origin negated; nearest, cell, amount and steps.
    #[test]
    fn lattice_points_distances_and_steps() {
        let l = Lattice::new([254_000, 254_000], [75_927_000, 45_000_000]).unwrap();
        assert_eq!(l.o, [75_927_000.0, -45_000_000.0]);
        let p = v(75_927_000.0 + 254_000.0 * 3.0 + 10.0, -45_000_000.0 - 254_000.0 * 2.0 - 0.4);
        assert!(l.on(v(p.x - 10.0, p.y + 0.4)));
        assert!(!l.on(p));
        assert!((l.off(p) - 10.0f64.hypot(0.4)).abs() < 1e-6);
        let c = l.cell(v(75_927_000.0 + 100_000.0, -45_000_000.0 + 50_000.0));
        assert_eq!(c[0], v(75_927_000.0, -45_000_000.0));
        assert_eq!(c[2], v(75_927_000.0 + 254_000.0, -45_000_000.0 + 254_000.0));
        // cell centre = 1, on the grid = 0, graded between
        assert!((l.amount(v(75_927_000.0 + 127_000.0, -45_000_000.0 + 127_000.0)) - 1.0).abs() < 1e-9);
        assert_eq!(l.amount(c[1]), 0.0);
        let a = l.amount(v(75_927_000.0 + 50_000.0, -45_000_000.0));
        assert!(a > 0.2 && a < 0.3, "{a}");
        assert_eq!(l.step_along(v(1.0, 0.0)), Some(254_000.0));
        assert!((l.step_along(v(0.7071, 0.7071)).unwrap() - 254_000.0 * 2f64.sqrt()).abs() < 1e-3);
        let r = Lattice::new([200_000, 100_000], [0, 0]).unwrap();
        assert!(!r.square());
        assert_eq!(r.step_along(v(0.0, 1.0)), Some(100_000.0));
        assert_eq!(r.step_along(v(0.7071, -0.7071)), None);
        assert!(Lattice::new([0, 254_000], [0, 0]).is_none());
    }

    /// Off without the option, the via grid falls back to the track grid.
    #[test]
    fn lattices_from_the_options() {
        let mut o = crate::model::Options::default();
        o.grid_track_nm = [254_000, 254_000];
        assert_eq!(Lattice::from_opts(&o), (None, None));
        o.grid_snap = "prefer".into();
        let (t, w) = Lattice::from_opts(&o);
        assert_eq!(t, w);
        assert!(t.is_some());
        o.grid_via_nm = [500_000, 500_000];
        assert_eq!(Lattice::from_opts(&o).1.unwrap().g, [500_000.0, 500_000.0]);
        o.grid_track_nm = [0, 0];
        assert_eq!(Lattice::from_opts(&o).0, None);
        assert_eq!(Mode::parse("fallback"), Mode::Fallback);
        assert_eq!(Mode::parse("x"), Mode::Off);
    }
}
