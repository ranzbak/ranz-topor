//! Pour stitching after routing (D4): join the pieces of a pour net that the routed copper cut
//! off from the rest of its pour back to it, the way a designer does by hand: look at each copper
//! layer for areas of the pour that are cut off and, where there is room, put a via in that area
//! near a pad of the net, joining it to the pour on another layer.
//!
//! The refilled pour is estimated per (pour, layer) by [`crate::pourcheck`] (the pre-route fill of
//! the ripped board, minus the routed foreign copper grown by the clearance, opened by half the
//! zone's minimum width as KiCad's filler does, islands kept by the zone's island removal mode,
//! thermal / solid pads, vias and tracks of the net joined by union-find). Every component it
//! reports cut off ([`crate::pourcheck::Cut`]) gets one try per pass:
//! - **island via**: a via inside one of the component's islands, at a cell whose fill depth on
//!   that layer and on another pour layer of the net is at least the via radius, where that
//!   other layer's fill belongs to the main component; candidates are ranked by the distance to
//!   the component's nearest pad (or via) plus the `via_squeeze` price of the slack the fill
//!   leaves around the via (the fill depth is the room to the nearest foreign copper less its
//!   clearance) and the first that the checker accepts (clearances, holes, edge, keepouts,
//!   against the routed copper and the stitches placed before) wins;
//! - **pad stub**: a pad of the component with no island to put a via in (its thermal spokes all
//!   cut) gets a short track of the net width on its own layer, ending in the main fill of that
//!   layer, or else in a via standing in the main fill of another pour layer (nearest first,
//!   rings 0.1 mm apart up to [`STUB_MAX_NM`], 32 directions), as the plane stage does;
//! - **island join** (`[planes] join_islands`): a component with no pad and no via site gets a
//!   track from a pad of the net in the main component into the island, priced by
//!   [`join_price`] (length, growing with the square of the distance from the pad over the
//!   [`STUB_MAX_NM`] scale; a warning beyond it), its end clear of the net's other copper.
//!
//! The pass repeats (the fill is estimated again with the stitches in) up to [`PASSES`] times or
//! until a pass places nothing. Literature: TopoR leaves islands of a poured area to the user
//! ("this may split the copper area into multiple unconnected parts. You can select and delete
//! the resulting islands", TopoR 6.3 manual p. 121; pour last, after routing, p. 184; TopoR 5.0
//! help: plane net integrity is not verified, "add wires to connect islands"), so this stage is
//! the user's own hand technique, not TopoR's.
//!
//! Cost: per pass one pour check (O(raster cells + items), see `pourcheck`) plus, per cut-off
//! component, O(island cells / STEP²) candidates and at most [`MAX_TRIES`] checker queries.

use super::draw::{track_item, via_item};
use super::factors::{Event, Registry, WireCtx};
use crate::check::{Checker, ItemIndex};
use crate::geom::Vec2;
use crate::model::{BoardModel, Item, Options};
use crate::pourcheck::{check_pour_reach_until, Cut, PourReach};
use crate::vias::via_sizes;

/// Most stitching passes.
pub const PASSES: usize = 3;
/// Most checker queries per cut-off component and pass.
pub const MAX_TRIES: usize = 64;
/// Candidate lattice: every STEP-th raster cell.
pub const STEP: usize = 4;
/// Longest pad stub, nm; also the length scale of an island join (no cutoff there, see
/// [`join_price`]).
pub const STUB_MAX_NM: f64 = 3.0e6;

/// What the stitching added and found.
#[derive(Clone, Debug, Default)]
pub struct StitchOut {
    /// New items (KiCad frame): vias and pad stubs.
    pub items: Vec<Item>,
    /// Cut-off components before and after.
    pub before: usize,
    pub after: usize,
    /// Island vias and pad stubs placed.
    pub island_vias: usize,
    pub pad_stubs: usize,
    /// Pad-less islands joined to a connected pad of their net by a track (`[planes] join_islands`).
    pub island_joins: usize,
    /// Report lines.
    pub notes: Vec<String>,
}

fn kicad_mm(p: Vec2) -> String {
    format!("({:.3}, {:.3})", p.x / 1e6, -p.y / 1e6)
}

fn cut_text(model: &BoardModel, c: &Cut) -> String {
    let name = model.nets.iter().find(|n| n.code == c.net).map_or("?", |n| n.name.as_str());
    let pads: Vec<String> = c.pads.iter().take(4).map(|&p| format!("{}.{}", model.pads[p].ref_, model.pads[p].number)).collect();
    let at = c
        .pads
        .first()
        .map(|&p| model.pads[p].center.to_vec2())
        .or_else(|| c.vias.first().copied())
        .unwrap_or(c.at);
    format!(
        "{name}: {} island(s) {:.2} mm², pads [{}{}], {} via(s), near {}",
        c.islands.len(),
        c.area / 1e12,
        pads.join(", "),
        if c.pads.len() > 4 { ", ..." } else { "" },
        c.vias.len(),
        kicad_mm(at)
    )
}

/// The nets' rasters on layers other than `layer`, where `p` is in the main fill with at least
/// `depth` of room: `(pour, layer, fill depth)`.
fn main_elsewhere(reach: &PourReach, model: &BoardModel, net: i32, layer: Option<u32>, p: Vec2, depth: f64) -> Option<(usize, u32, f64)> {
    let mut best: Option<(usize, u32, f64)> = None;
    for (zi, z) in model.pours.iter().enumerate().filter(|(_, z)| z.net == net) {
        for &l in &z.layers {
            if Some(l) == layer {
                continue;
            }
            let Some(r) = reach.raster(zi, l) else { continue };
            let d = r.depth_at(p);
            if d >= depth && reach.island_in_main(zi, l, r.island_at(p)) && best.is_none_or(|b| d > b.2) {
                best = Some((zi, l, d));
            }
        }
    }
    best
}

/// Stitch the cut-off pour components of `items` (the routed copper, KiCad frame).
/// `routed_pads` = model pad indices at the ends of routed connections of pour nets.
pub fn stitch(model: &BoardModel, opts: &Options, reg: &Registry, items: &[Item], routed_pads: &[usize], deadline: Option<std::time::Instant>) -> StitchOut {
    stitch_until(model, opts, reg, items, routed_pads, deadline, &|| false)
}

/// [`stitch`] that also ends after the pass in progress once `stop()` holds (cancel-fix: the run's
/// Cancel). Every pass starts with a complete pour-reach check, so the pieces still cut off
/// (`after`) are counted correctly also when it stops early.
pub fn stitch_until(model: &BoardModel, opts: &Options, reg: &Registry, items: &[Item], routed_pads: &[usize], deadline: Option<std::time::Instant>, stop: &dyn Fn() -> bool) -> StitchOut {
    let mut out = StitchOut::default();
    if model.pours.iter().all(|z| z.fills.is_empty()) {
        return out;
    }
    let checker = Checker::new(model, opts);
    let mut idx = ItemIndex::new(checker.cell());
    for it in items {
        idx.insert(it);
    }
    let mut all: Vec<Item> = items.to_vec();
    let expired = || deadline.is_some_and(|d| std::time::Instant::now() >= d) || crate::control::cancelled() || stop();
    let ok = |idx: &ItemIndex, its: &[Item]| checker.check_against(idx, its, &[]).violations.is_empty();
    // A via stays off the pads of its own net: no via in a pad unless `[vias] via_in_pad` (then
    // centred inside it), and never dipping partly into one (KiCad connection width, D1).
    // KiCad builds a pad's and a via's polygon up to `max_error` outside their arcs: clear by
    // that much, or the two touch in a sliver (D6a).
    let tol = model.rules.max_error.max(0) as f64;
    let own_pad_ok = |p: Vec2, r: f64, net: i32| {
        model.pads.iter().filter(|pad| pad.net == net).all(|pad| {
            pad.layers.iter().all(|&l| {
                let d = crate::geom::point_skeleton(p, &model.skeleton(pad, l));
                d >= r + tol || (opts.via_in_pad && d <= 0.0)
            })
        })
    };
    // Nor touch other copper of its net without overlapping it well (a via grazing an own-net via
    // or track is a sliver KiCad flags: dcsgonebadge D4, two GND vias 0.596 mm apart, "actual
    // 0.074 mm"): it keeps a gap to every own-net via and track.
    let own_copper_ok = |all: &[Item], p: Vec2, r: f64, net: i32| {
        let gap = 1_000.0 + 2.0 * tol;
        let q = Vec2 { x: p.x, y: -p.y }; // KiCad frame of the items
        model.vias.iter().filter(|v| v.net == net).all(|v| (v.at.to_vec2() - p).len() >= r + 0.5 * v.d as f64 + gap)
            && all.iter().all(|it| match it {
                Item::Via(v) if v.net == net => (Vec2 { x: v.at.x as f64, y: v.at.y as f64 } - q).len() >= r + 0.5 * v.d as f64 + gap,
                Item::Track(tr) if tr.net == net => match crate::geom::quant::dequantise(&tr.prim) {
                    crate::geom::Prim::Seg(s) => crate::geom::point_seg(q, &s) >= r + 0.5 * tr.width as f64 + gap,
                    _ => true,
                },
                _ => true,
            })
    };
    let squeeze = |slack: f64, d: f64| reg.price(&Event::ViaSqueeze { wire: 0, at: Vec2::default(), slack_nm: slack, via_nm: d }, &WireCtx::default()) * 1e6;
    let mut unplaced: Vec<String> = Vec::new();
    for pass in 0..PASSES {
        unplaced.clear();
        let reach = check_pour_reach_until(model, &all, routed_pads, deadline);
        if let Ok(path) = std::env::var("TOPO_POURDUMP") {
            reach.dump(&format!("{path}.p{pass}"));
        }
        if pass == 0 {
            out.before = reach.cuts.len();
            for c in &reach.cuts {
                out.notes.push(format!("pour piece cut off: {}", cut_text(model, c)));
            }
        }
        out.after = reach.cuts.len();
        if reach.cuts.is_empty() || expired() {
            break;
        }
        let mut placed_any = false;
        for c in &reach.cuts {
            let Some(&sz) = via_sizes(&model.rules, c.net, opts).first() else { continue };
            let r = 0.5 * sz.d as f64;
            // The fill has to reach the via with a ring of at least the zone's minimum width
            // (thinner fill is removed by KiCad's filler, and what is left meets the via in a
            // neck KiCad's connection width check flags: dcsgonebadge D5, a GND via at
            // (126.79, 121.1) mm, 0.1038 mm < 0.127 mm; D6a).
            let ring = model.pours.iter().filter(|z| z.net == c.net).map(|z| z.min_thickness.max(0) as f64).fold(0.0, f64::max).max(model.rules.min_connection.max(0) as f64);
            let need = r + ring;
            let targets: Vec<Vec2> = if c.pads.is_empty() { c.vias.clone() } else { c.pads.iter().map(|&p| model.pads[p].center.to_vec2()).collect() };
            // Island via candidates.
            let mut cand: Vec<(f64, Vec2)> = Vec::new();
            for &(zi, l, isl) in &c.islands {
                let Some(ras) = reach.raster(zi, l) else { continue };
                for p in ras.island_sites(isl, need, STEP) {
                    let Some((_, _, d2)) = main_elsewhere(&reach, model, c.net, Some(l), p, need) else { continue };
                    if !own_pad_ok(p, r, c.net) || !own_copper_ok(&all, p, r, c.net) {
                        continue;
                    }
                    let d1 = ras.depth_at(p);
                    let near = targets.iter().map(|&t| (t - p).len()).fold(f64::INFINITY, f64::min);
                    let near = if near.is_finite() { near } else { 0.0 };
                    cand.push((near + squeeze(d1 - r, sz.d as f64) + squeeze(d2 - r, sz.d as f64), p));
                }
            }
            cand.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.x.total_cmp(&b.1.x)).then(a.1.y.total_cmp(&b.1.y)));
            let mut done = false;
            for &(_, p) in cand.iter().take(MAX_TRIES) {
                let v = via_item(c.net, p, sz, None);
                if ok(&idx, std::slice::from_ref(&v)) {
                    out.notes.push(format!("stitch via {} mm at {} for {}", sz.d as f64 / 1e6, kicad_mm(p), cut_text(model, c)));
                    idx.insert(&v);
                    all.push(v.clone());
                    out.items.push(v);
                    out.island_vias += 1;
                    done = true;
                    break;
                }
            }
            if done {
                placed_any = true;
                continue;
            }
            // Pad stub: a pad with no usable island.
            'pads: for &pi in &c.pads {
                let pad = &model.pads[pi];
                let cpos = pad.center.to_vec2();
                let w = model.rules.net_rule(c.net).width.max(1) as f64;
                let layers: Vec<u32> = pad.layers.iter().copied().filter(|l| model.layers.iter().any(|x| x.id == *l && x.routable)).collect();
                let mut tries = 0usize;
                // Out from the pad centre; the own-pad test keeps a via off the pad.
                let mut d: f64 = 0.2e6;
                while d <= STUB_MAX_NM && tries < 4 * MAX_TRIES {
                    for k in 0..32 {
                        let a = k as f64 * std::f64::consts::PI / 16.0;
                        let p = cpos + Vec2 { x: a.cos(), y: a.sin() } * d;
                        for &sl in &layers {
                            // Track only, into the main fill of its own layer.
                            let same = model.pours.iter().enumerate().any(|(zi, z)| {
                                z.net == c.net && reach.raster(zi, sl).is_some_and(|ras| ras.depth_at(p) >= 0.5 * w && reach.island_in_main(zi, sl, ras.island_at(p)))
                            });
                            let via_ok = !same && main_elsewhere(&reach, model, c.net, None, p, need).is_some() && own_pad_ok(p, r, c.net) && own_copper_ok(&all, p, r, c.net);
                            if !same && !via_ok {
                                continue;
                            }
                            tries += 1;
                            let mut its = vec![track_item(c.net, sl, w, cpos, p, None)];
                            if via_ok {
                                its.push(via_item(c.net, p, sz, None));
                            }
                            if ok(&idx, &its) {
                                out.notes.push(format!(
                                    "pad stub {}.{} -> {} ({:.3} mm{}) for {}",
                                    pad.ref_,
                                    pad.number,
                                    kicad_mm(p),
                                    (p - cpos).len() / 1e6,
                                    if via_ok { ", via" } else { "" },
                                    cut_text(model, c)
                                ));
                                for it in its {
                                    idx.insert(&it);
                                    all.push(it.clone());
                                    out.items.push(it);
                                }
                                out.pad_stubs += 1;
                                done = true;
                                break 'pads;
                            }
                        }
                    }
                    d += 0.1e6;
                }
            }
            // Island join (`[planes] join_islands`, user 2026-11-12): a piece holding no pad that no
            // via reaches is joined to the nearest pad of its net that is connected to the rest of
            // the net, by a track from the pad into the island (shortest first).
            if !done && c.pads.is_empty() && opts.plane_join_islands {
                let site_ok = |p: Vec2, r: f64| own_copper_ok(&all, p, r, c.net);
                if let Some(it) = join_island(model, &reach, c, &|it: &Item| ok(&idx, std::slice::from_ref(it)), &site_ok) {
                    let (pi, len) = (it.1, it.2);
                    out.notes.push(format!(
                        "island join {}.{} -> {} ({:.3} mm) for {}",
                        model.pads[pi].ref_,
                        model.pads[pi].number,
                        kicad_mm(it.3),
                        len / 1e6,
                        cut_text(model, c)
                    ));
                    if it.4 > STUB_MAX_NM {
                        out.notes.push(format!(
                            "long island join {}.{} -> {}: {:.3} mm from the pad edge, over the {:.1} mm scale (check it by hand)",
                            model.pads[pi].ref_,
                            model.pads[pi].number,
                            kicad_mm(it.3),
                            it.4 / 1e6,
                            STUB_MAX_NM / 1e6
                        ));
                    }
                    idx.insert(&it.0);
                    all.push(it.0.clone());
                    out.items.push(it.0);
                    out.island_joins += 1;
                    done = true;
                }
            }
            if done {
                placed_any = true;
            } else {
                unplaced.push(cut_text(model, c));
            }
        }
        if !placed_any {
            break;
        }
        if pass + 1 == PASSES {
            let reach = check_pour_reach_until(model, &all, routed_pads, deadline);
            if let Ok(path) = std::env::var("TOPO_POURDUMP") {
                reach.dump(&format!("{path}.p{PASSES}"));
            }
            out.after = reach.cuts.len();
        }
    }
    out.notes.extend(unplaced.into_iter().map(|s| format!("no stitch site for {s}")));
    out
}

/// Price of an island join of length `len` that runs `edge` from the pad's edge (nm): the length,
/// growing smoothly with the square of `edge` over the [`STUB_MAX_NM`] scale (graded, no cutoff:
/// a join twice the scale costs five times its length). O(1).
pub fn join_price(len: f64, edge: f64) -> f64 {
    let e = edge.max(0.0) / STUB_MAX_NM;
    len * (1.0 + e * e)
}

/// The island join of a cut-off component `c` that holds no pad: candidates are pairs (pad of the
/// net in the main component with copper on a routable layer of one of `c`'s islands, a site of
/// that island at least w/2 deep whose track end keeps a gap to the net's other vias and tracks,
/// `site_ok(end, w/2)`, the D4 sliver rule), priced by [`join_price`]; tried cheapest first, the
/// first track `ok` accepts wins. Returns (track, pad index, length nm, end point math frame,
/// distance from the pad edge nm). O(sites x pads log + MAX_TRIES checker queries).
fn join_island(model: &BoardModel, reach: &PourReach, c: &Cut, ok: &dyn Fn(&Item) -> bool, site_ok: &dyn Fn(Vec2, f64) -> bool) -> Option<(Item, usize, f64, Vec2, f64)> {
    let w = model.rules.net_rule(c.net).width.max(1) as f64;
    let mut main: Vec<usize> = model
        .pours
        .iter()
        .enumerate()
        .filter(|(_, z)| z.net == c.net)
        .flat_map(|(zi, _)| reach.main_pads.get(&zi).cloned().unwrap_or_default())
        .collect();
    main.sort_unstable();
    main.dedup();
    let mut cand: Vec<(f64, usize, u32, Vec2, f64, f64)> = Vec::new();
    for &(zi, l, isl) in &c.islands {
        if !model.layers.iter().any(|x| x.id == l && x.routable) {
            continue;
        }
        let Some(ras) = reach.raster(zi, l) else { continue };
        let pads: Vec<usize> = main.iter().copied().filter(|&pi| model.pads[pi].layers.contains(&l)).collect();
        if pads.is_empty() {
            continue;
        }
        for p in ras.island_sites(isl, 0.5 * w, STEP) {
            if !site_ok(p, 0.5 * w) {
                continue;
            }
            for &pi in &pads {
                let pad = &model.pads[pi];
                let len = (p - pad.center.to_vec2()).len();
                let edge = crate::geom::point_skeleton(p, &model.skeleton(pad, l)).max(0.0);
                cand.push((join_price(len, edge), pi, l, p, len, edge));
            }
        }
    }
    cand.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.3.x.total_cmp(&b.3.x)).then(a.3.y.total_cmp(&b.3.y)));
    for &(_, pi, l, p, len, edge) in cand.iter().take(MAX_TRIES) {
        let t = track_item(c.net, l, w, model.pads[pi].center.to_vec2(), p, None);
        if ok(&t) {
            return Some((t, pi, len, p, edge));
        }
    }
    None
}
