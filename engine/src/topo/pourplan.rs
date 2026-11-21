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

/// TW7 E (detection; docs/dev/design/tw7-moves.md E, tw4-r2.md (3)): one component of a pour
/// net cut off from its main copper by the plan, the pads in it that were not cut off before
/// routing, its islands `(pour, layer, island)`, and the plan wires of other nets whose copper
/// bounds it (the movers of the pour repair move).
#[derive(Clone, Debug, PartialEq)]
pub struct CutComp {
    pub net: i32,
    pub pads: Vec<usize>,
    pub islands: Vec<(usize, u32, u32)>,
    pub bounding: Vec<WireId>,
    /// The islands' area-weighted centroid (math frame, the projection's) and fill area (nm²).
    pub at: crate::geom::Vec2,
    pub area: f64,
}

/// TW7 E: the components [`pour_cuts`] counts (cut off, holding a pad not in `base`), each with
/// the wires that bound it: a plan track (or via) of another net on an island's layer whose
/// copper has the island on one side, probed at its clearance plus 1.5 cells off its edge (both
/// sides of a track, along it every two cells, at most 32 probes; eight directions round a via).
/// Read-only. O(one pour check + islands x items x probes).
pub fn cut_components(model: &BoardModel, items: &[Item], base: &PourBase) -> Vec<CutComp> {
    if !base.any {
        return Vec::new();
    }
    let r = crate::pourcheck::check_pour_reach_cells(model, items, &[], None, base.cells);
    let math = |x: i64, y: i64| crate::geom::Vec2 { x: x as f64, y: -(y as f64) };
    let mut out = Vec::new();
    for c in &r.cuts {
        let pads: Vec<usize> = c.pads.iter().copied().filter(|p| !base.cut_pads.contains(p)).collect();
        if pads.is_empty() {
            continue;
        }
        let clr = model.rules.net_rule(c.net).clearance.max(model.rules.min_clearance) as f64;
        let mut bounding: BTreeSet<WireId> = BTreeSet::new();
        for &(zi, layer, l) in &c.islands {
            let Some(ras) = r.raster(zi, layer) else { continue };
            let res = ras.res();
            for it in items {
                match it {
                    Item::Track(t) if t.layer == layer && t.net != c.net => {
                        let (Some(w), crate::geom::TrackPrim::Seg { a, b }) = (t.wire, t.prim) else { continue };
                        if bounding.contains(&w) {
                            continue;
                        }
                        let (pa, pb) = (math(i64::from(a.x), i64::from(a.y)), math(i64::from(b.x), i64::from(b.y)));
                        let d = pb - pa;
                        let len = d.len();
                        if len <= 0.0 {
                            continue;
                        }
                        let nrm = crate::geom::Vec2 { x: -d.y / len, y: d.x / len };
                        let off = t.width as f64 / 2.0 + clr + 1.5 * res;
                        let n = ((len / (2.0 * res)).ceil() as usize).clamp(1, 32);
                        let hit = (0..n).any(|i| {
                            let p = pa + d * ((i as f64 + 0.5) / n as f64);
                            ras.island_at(p + nrm * off) == l || ras.island_at(p - nrm * off) == l
                        });
                        if hit {
                            bounding.insert(w);
                        }
                    }
                    Item::Via(v) if v.net != c.net && (v.layers.is_empty() || v.layers.contains(&layer)) => {
                        let Some(w) = v.wire else { continue };
                        let at = math(v.at.x, v.at.y);
                        let rr = v.d as f64 / 2.0 + clr + 1.5 * res;
                        let hit = (0..8).any(|k| {
                            let a = std::f64::consts::FRAC_PI_4 * k as f64;
                            ras.island_at(at + crate::geom::Vec2 { x: rr * a.cos(), y: rr * a.sin() }) == l
                        });
                        if hit {
                            bounding.insert(w);
                        }
                    }
                    _ => {}
                }
            }
        }
        out.push(CutComp { net: c.net, pads, islands: c.islands.clone(), bounding: bounding.into_iter().collect(), at: c.at, area: c.area });
    }
    out
}

/// grid-snap r2 (`[drawing] grid_snap = "prefer"`): a via moved onto the grid must not change
/// the plan's pour cut-offs. `tracks`: the plan's tracks; `sites`: every via site after the grid
/// moves `moves` `(wire, via index, site before)`; `via`: the via item of a wire at a point.
/// When the copper with the moves cuts off more ([`pour_cuts`], graded) than without them, the
/// moves of the vias on the wires that bound a cut-off piece ([`cut_components`]) go back, and
/// if it still cuts off more, every move goes back. Returns how many went back. O(up to three
/// pour checks; none without a pour or a move).
pub fn undo_grid_cuts(model: &BoardModel, tracks: &[Item], sites: &mut super::viaopt::Sites, moves: &[(WireId, usize, super::viaopt::Site)], via: &dyn Fn(WireId, crate::geom::Vec2) -> Option<Item>, pb: &PourBase) -> usize {
    if moves.is_empty() || !pb.any {
        return 0;
    }
    let with = |sites: &super::viaopt::Sites| -> Vec<Item> { tracks.iter().cloned().chain(sites.iter().filter_map(|(&(w, _), &(_, p))| via(w, p))).collect() };
    let mut before = sites.clone();
    for &(w, i, s0) in moves {
        before.insert((w, i), (s0.k, s0.p));
    }
    let base = pour_cuts(model, &with(&before), pb).1;
    let now = with(sites);
    if pour_cuts(model, &now, pb).1 <= base + 1e-9 {
        return 0;
    }
    let bound: BTreeSet<WireId> = cut_components(model, &now, pb).into_iter().flat_map(|c| c.bounding).collect();
    let mut back = 0;
    for &(w, i, s0) in moves.iter().filter(|m| bound.contains(&m.0)) {
        sites.insert((w, i), (s0.k, s0.p));
        back += 1;
    }
    if pour_cuts(model, &with(sites), pb).1 > base + 1e-9 {
        *sites = before;
        back = moves.len();
    }
    back
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

    /// grid-snap r2: a SIG track stops short of pad B (the pour between them stays open);
    /// a via moved into that gap closes it (U1.1 and U2.1 cut off): the move goes back. A move
    /// that cuts nothing stays; no move, nothing to do.
    #[test]
    fn grid_moves_that_cut_off_pour_go_back() {
        use crate::geom::Vec2;
        let m = model();
        let pb = PourBase::new(&m, 1.0e5);
        let mut t = seg(2, (7 * MM, 2_300_000), (7 * MM, 30_500_000));
        if let Item::Track(x) = &mut t {
            x.wire = Some(0);
        }
        let math = |x: i64, y: i64| Vec2 { x: x as f64, y: -(y as f64) };
        let via = |w: WireId, p: Vec2| Some(super::super::draw::via_item(2, p, ViaSize { d: 1_200_000, drill: 600_000 }, Some(w)));
        let site = |p: Vec2| super::super::viaopt::Site { wire: 1, i: 0, k: 0, p };
        let (far, gap, far2) = (math(12 * MM, 31 * MM), math(7 * MM, 31 * MM), math(12 * MM, 30_800_000));
        let tracks = [t];
        assert_eq!(pour_cuts(&m, &[tracks[0].clone(), via(1, far).unwrap()], &pb).0, 0);
        assert_eq!(pour_cuts(&m, &[tracks[0].clone(), via(1, gap).unwrap()], &pb).0, 1);
        // wire 1's move closes the gap and goes back; wire 3's move (far from it) stays
        let other = math(20 * MM, 10 * MM);
        let mut sites: super::super::viaopt::Sites = [((1, 0), (0, gap)), ((3, 0), (0, other))].into_iter().collect();
        let moves = [(1, 0, site(far)), (3, 0, super::super::viaopt::Site { wire: 3, i: 0, k: 0, p: math(20 * MM, 10_200_000) })];
        assert_eq!(undo_grid_cuts(&m, &tracks, &mut sites, &moves, &via, &pb), 1);
        assert_eq!((sites[&(1, 0)].1, sites[&(3, 0)].1), (far, other));
        let mut sites: super::super::viaopt::Sites = [((1, 0), (0, far2))].into_iter().collect();
        assert_eq!(undo_grid_cuts(&m, &tracks, &mut sites, &[(1, 0, site(far))], &via, &pb), 0);
        assert_eq!(sites[&(1, 0)].1, far2);
        assert_eq!(undo_grid_cuts(&m, &tracks, &mut sites, &[], &via, &pb), 0);
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

    /// TW7 E: the cut-off component of `a_track_across_the_pour_...` (U1.1, U2.1) is bounded by
    /// the track's wire; a second track of another wire far inside the main side is not; a pour
    /// left open gives no component; pads cut off before routing do not count.
    #[test]
    fn cut_components_name_the_bounding_wires() {
        let m = model();
        let base = PourBase::new(&m, 1.0e5);
        let mut cut = seg(2, (7 * MM, 2_300_000), (7 * MM, 31_700_000));
        let mut far = seg(2, (26 * MM, 10 * MM), (26 * MM, 12 * MM));
        if let (Item::Track(t), Item::Track(f)) = (&mut cut, &mut far) {
            t.wire = Some(4);
            f.wire = Some(9);
        }
        let comps = cut_components(&m, &[cut.clone(), far.clone()], &base);
        assert_eq!(comps.len(), 1);
        assert_eq!((comps[0].net, comps[0].pads.clone(), comps[0].bounding.clone()), (1, vec![0, 1], vec![4]));
        // The west piece [2, 7] x [2, 32] mm: its centroid near (4.5, -17) mm (math frame), its area near 5 x 30 mm².
        assert!((comps[0].at.x / MM as f64 - 4.4).abs() < 0.6 && (comps[0].at.y / MM as f64 + 17.0).abs() < 0.6, "{:?}", comps[0].at);
        assert!((comps[0].area / (MM * MM) as f64 - 145.0).abs() < 20.0, "{}", comps[0].area / (MM * MM) as f64);
        let mut open = seg(2, (7 * MM, 2_300_000), (7 * MM, 20 * MM));
        if let Item::Track(t) = &mut open {
            t.wire = Some(4);
        }
        assert!(cut_components(&m, &[open], &base).is_empty());
        let mut b2 = base.clone();
        b2.cut_pads.extend([0, 1]);
        assert!(cut_components(&m, &[cut], &b2).is_empty());
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
