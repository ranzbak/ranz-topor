//! tw4-r2 (3) (`[capacity] pour_cuts`, docs/dev/design/tw4-r2.md): the plan's graded pour-cut
//! term. Planes are terminals (user rule 2026-11-10): a pad that reaches its net only through a
//! pour (`decompose` gives it no connection) is cut off when the plan's copper of other nets
//! rings it, and KiCad then reports a missing connection the stitching after the drawing may not
//! repair (no stitch site). So the plan prices it like a predicted violation: costly, not
//! forbidden (the wire stays placed, the loop's moves can take the ring away).
//!
//! **Measure.** The plan's copper (every live wire's layer stretches at their pass widths along
//! its pass points, [`super::cutflow::wire_stretches`], and its planned vias at their discs)
//! goes through the pour-reach check of the drawn copper ([`crate::pourcheck`], KiCad's fill
//! approximation, validated against KiCad's refill: IoU 0.99) at a coarser raster
//! (`[capacity] pour_cut_cells` cells per pour layer). Every component it reports cut off from
//! its net's main copper that holds a pad which is not cut off before routing ([`PourBase`]) is
//! one predicted violation, graded `1 + 0.25` per further such pad (a ring round three pads
//! costs more than one round a single pad, smoothly). TopoR 6.3 manual p. 121: pouring "may
//! split the copper area into multiple unconnected parts" and the user deletes the islands
//! afterwards; here the plan sees the split before the drawing.
//!
//! Cost: one [`crate::pourcheck::check_pour_reach_cells`] per call, O(pour nets x pour layers x
//! cells + stamped cells), plus O(passes) for the plan copper.

use super::board::Board;
use super::cutflow::wire_stretches;
use super::pack::Packer;
use super::state::{State, WireId};
use crate::model::{BoardModel, Item};
use crate::vias::ViaSize;
use std::collections::BTreeSet;

/// Graded price of each further cut-off pad of one component (the first costs 1).
pub const EXTRA_PAD: f64 = 0.25;

/// Cost meter (process-wide sums, the plan metrics line `move_cost.pour_cut`): calls of
/// [`pour_cuts`] from the whole-board measure and from the local judge, and the time in them.
pub mod meter {
    use std::sync::atomic::{AtomicU64, Ordering};
    pub static GLOBAL: AtomicU64 = AtomicU64::new(0);
    pub static LOCAL: AtomicU64 = AtomicU64::new(0);
    pub static LOCAL_SKIP: AtomicU64 = AtomicU64::new(0);
    pub static NS: AtomicU64 = AtomicU64::new(0);
    /// `(whole-board calls, local calls, local judgements that kept the old value, ms in calls)`.
    pub fn read() -> (u64, u64, u64, f64) {
        (GLOBAL.load(Ordering::Relaxed), LOCAL.load(Ordering::Relaxed), LOCAL_SKIP.load(Ordering::Relaxed), NS.load(Ordering::Relaxed) as f64 / 1e6)
    }
}

/// The pour connectivity before routing: the pads already cut off from their net's main copper
/// (a hand board's own islands are no cost of the plan). Built once per run.
#[derive(Clone, Debug, Default)]
pub struct PourBase {
    pub cut_pads: BTreeSet<usize>,
    /// Whether the board has a pour with a fill (no pour: the term is always 0, no check runs).
    pub any: bool,
    pub cells: f64,
}

impl PourBase {
    /// The check of the board without router copper at `cells` cells per pour layer. O(one
    /// pour check).
    pub fn new(model: &BoardModel, cells: f64) -> PourBase {
        let any = model.pours.iter().any(|z| z.net != 0 && z.fills.iter().any(|f| f.outer.len() >= 3));
        if !any {
            return PourBase { cut_pads: BTreeSet::new(), any, cells };
        }
        let r = crate::pourcheck::check_pour_reach_cells(model, &[], &[], None, cells);
        PourBase { cut_pads: r.cuts.iter().flat_map(|c| c.pads.iter().copied()).chain(r.lost_pads.iter().copied()).collect(), any, cells }
    }
}

/// The plan's copper as router items (KiCad frame): per live wire its layer stretches (one
/// track per stretch segment at the smaller width of its two ends) and its planned vias (at the
/// static site of their triangle, [`Packer::via_point_cached`]; the drawing moves them along the
/// path, which a pour ring does not change). `events[w]` = the wire's labelled events. O(passes).
pub fn plan_items(st: &State, packer: &Packer, b: &Board, events: &[Vec<(usize, f64, i32)>], bottom: i32) -> Vec<Item> {
    let mut out = Vec::new();
    let site = |t: usize, net: i32, r: f64, s: f64| (packer.via_point_cached(t, net, 2.0 * r, s), true);
    for (w, wi) in st.wires.iter().enumerate() {
        if !wi.alive || w >= events.len() {
            continue;
        }
        wire_items(st, packer, b, w, &events[w], bottom, &site, &mut out);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn wire_items(st: &State, packer: &Packer, b: &Board, w: WireId, ev: &[(usize, f64, i32)], bottom: i32, site: &dyn Fn(usize, i32, f64, f64) -> (crate::geom::Vec2, bool), out: &mut Vec<Item>) {
    let r = wire_stretches(st, packer, w, ev, &[], bottom, None, site, None, [None, None]);
    for s in &r.runs {
        let Some(&lid) = b.signal_layers.get(s.layer) else { continue };
        let n = s.curve.len();
        for i in 0..n.saturating_sub(1) {
            let (a, c) = (s.curve[i], s.curve[i + 1]);
            if (c - a).len() < 1.0 {
                continue;
            }
            // Width of curve point j: an inner point its pass width, an end the nominal width.
            let wid = |j: usize| if j == 0 || j + 1 == n { s.w } else { s.pass_w.get(j - 1).copied().unwrap_or(s.w) };
            out.push(super::draw::track_item(s.net, lid, wid(i).min(wid(i + 1)).max(1.0), a, c, Some(w)));
        }
    }
    for (v, _) in &r.vias {
        let d = (2.0 * v.r).round() as i64;
        out.push(super::draw::via_item(v.net, v.at, ViaSize { d, drill: d / 2 }, Some(w)));
    }
}

/// The pour-cut term of `items` (the plan's copper): `(count, graded)`, one per component cut off
/// from its net's main copper holding a pad not in `base`, graded `1 + EXTRA_PAD` per further
/// such pad. O(one pour check).
pub fn pour_cuts(model: &BoardModel, items: &[Item], base: &PourBase) -> (usize, f64) {
    if !base.any {
        return (0, 0.0);
    }
    let t0 = std::time::Instant::now();
    let r = crate::pourcheck::check_pour_reach_cells(model, items, &[], None, base.cells);
    meter::NS.fetch_add(t0.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    let mut n = 0;
    let mut g = 0.0;
    for c in &r.cuts {
        let new = c.pads.iter().filter(|p| !base.cut_pads.contains(p)).count();
        if new > 0 {
            n += 1;
            g += 1.0 + EXTRA_PAD * (new - 1) as f64;
        }
    }
    (n, g)
}
