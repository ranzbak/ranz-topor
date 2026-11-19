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
    pub static NS: AtomicU64 = AtomicU64::new(0);
    /// `(whole-board calls, local calls, ms in calls)`.
    pub fn read() -> (u64, u64, f64) {
        (GLOBAL.load(Ordering::Relaxed), LOCAL.load(Ordering::Relaxed), NS.load(Ordering::Relaxed) as f64 / 1e6)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{IVec2, TrackPrim};
    use crate::model::TrackItem;

    const MM: i64 = 1_000_000;

    fn pad(r: &str, net: i32, x: i64, y: i64) -> String {
        format!(
            r#"{{ "uuid": "{r}", "ref": "{r}", "number": "1", "net": {net}, "attr": "pth",
              "center": [{x}, {y}], "layers": [0, 31], "shapes": {{ "0": {{ "kind": "circle", "d": 1000000 }}, "31": {{ "kind": "circle", "d": 1000000 }} }} }}"#
        )
    }

    /// 50 x 40 mm board, GND pour on B.Cu [2, 32] x [2, 32] mm listing U1.1 (4, 15), U2.1 (4, 25)
    /// (west of x = 7) and U3.1 (20, 15), U4.1 (20, 25), U5.1 (25, 20) (east: the main component
    /// holds the most listed pads, `pourcheck`); SIG pads A (7, 2.3) and B (7, 31.7).
    fn model() -> BoardModel {
        let pads = [pad("U1", 1, 4 * MM, 15 * MM), pad("U2", 1, 4 * MM, 25 * MM), pad("U3", 1, 20 * MM, 15 * MM), pad("U4", 1, 20 * MM, 25 * MM), pad("U5", 1, 25 * MM, 20 * MM), pad("A", 2, 7 * MM, 2_300_000), pad("B", 2, 7 * MM, 31_700_000)];
        let s = format!(
            r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
              "nets": [ {{ "code": 1, "name": "GND" }}, {{ "code": 2, "name": "SIG" }} ],
              "outline": [ {{ "kind": "seg", "a": [0, 0], "b": [50000000, 0] }},
                {{ "kind": "seg", "a": [50000000, 0], "b": [50000000, 40000000] }},
                {{ "kind": "seg", "a": [50000000, 40000000], "b": [0, 40000000] }},
                {{ "kind": "seg", "a": [0, 40000000], "b": [0, 0] }} ],
              "pads": [ {} ],
              "pours": [ {{ "uuid": "zz", "net": 1, "layers": [31], "pads": ["U1.1", "U2.1", "U3.1", "U4.1", "U5.1"],
                "vias": [], "clearance": 200000, "min_thickness": 254000, "thermal_gap": 500000,
                "spoke_width": 500000, "pad_connection": 1,
                "fills": [ {{ "layer": 31, "outer": [[2000000, 2000000], [32000000, 2000000],
                  [32000000, 32000000], [2000000, 32000000]] }} ] }} ],
              "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0 }} }}"#,
            pads.join(",")
        );
        BoardModel::from_json(&s).unwrap()
    }

    fn seg(net: i32, a: (i64, i64), b: (i64, i64)) -> Item {
        let p = |q: (i64, i64)| IVec2 { x: q.0 as i32, y: q.1 as i32 };
        Item::Track(TrackItem { net, layer: 31, width: 250_000, prim: TrackPrim::Seg { a: p(a), b: p(b) }, group: None, wire: None })
    }

    #[test]
    fn no_copper_cuts_nothing() {
        let m = model();
        let base = PourBase::new(&m, 1.0e5);
        assert!(base.any && base.cut_pads.is_empty());
        assert_eq!(pour_cuts(&m, &[], &base), (0, 0.0));
    }

    /// A SIG track across the whole fill at x = 7 mm cuts U1.1 and U2.1 (west) off the main side
    /// (U3.1, U4.1, U5.1, east): one cut-off component of two pads, graded 1 + 0.25.
    #[test]
    fn a_track_across_the_pour_cuts_off_the_small_side_graded_by_its_pads() {
        let m = model();
        let base = PourBase::new(&m, 1.0e5);
        let cut = [seg(2, (7 * MM, 2_300_000), (7 * MM, 31_700_000))];
        assert_eq!(pour_cuts(&m, &cut, &base), (1, 1.0 + EXTRA_PAD));
        // Stopping short of the fill's edge leaves the pour open: nothing.
        let open = [seg(2, (7 * MM, 2_300_000), (7 * MM, 20 * MM))];
        assert_eq!(pour_cuts(&m, &open, &base), (0, 0.0));
        // The same at the check's own raster (4 M cells): the coarse plan raster agrees here.
        let fine = PourBase::new(&m, crate::pourcheck::CELL_BUDGET);
        assert_eq!(pour_cuts(&m, &cut, &fine), (1, 1.0 + EXTRA_PAD));
    }

    /// Pads already cut off before routing are the board's own: the plan pays only for new ones.
    #[test]
    fn pads_cut_off_before_routing_do_not_count() {
        let m = model();
        let mut base = PourBase::new(&m, 1.0e5);
        let cut = [seg(2, (7 * MM, 2_300_000), (7 * MM, 31_700_000))];
        // U1.1 (pad 0) counted as cut before: the component still holds U2.1, priced 1.
        base.cut_pads.insert(0);
        assert_eq!(pour_cuts(&m, &cut, &base), (1, 1.0));
        base.cut_pads.insert(1);
        assert_eq!(pour_cuts(&m, &cut, &base), (0, 0.0));
    }

    /// A board without a filled pour never runs the check.
    #[test]
    fn no_pour_no_term() {
        let mut m = model();
        m.pours.clear();
        let base = PourBase::new(&m, 1.0e5);
        assert!(!base.any);
        assert_eq!(pour_cuts(&m, &[seg(2, (7 * MM, 0), (7 * MM, 40 * MM))], &base), (0, 0.0));
    }
}
