//! Clearance checker: grid-indexed copper gap tests of candidate items against the board.
//!
//! Gaps are copper-to-copper (`copper_gap` / `copper_gap_skeleton`), compared with
//! `required + margin`. Copper-shape polygons are checked by their edges (grid obstacles) plus
//! containment of the item's start point (gap 0). Keepouts (rule areas) and shape polygons
//! live in a per-layer grid of regions (`region_grids`, bbox query, then exact edge tests).

use crate::geom::prim::Aabb;
use crate::geom::{
    copper_gap, copper_gap_skeleton, dequantise, point_skeleton, skeleton_closest_point, Grid,
    Prim, Seg, Skeleton, Track, Vec2,
};
use crate::model::{flip_track, via_layers, BoardModel, Item, Options};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Violation {
    /// Index into the checked item list.
    pub item: usize,
    /// `pad:REF:NUM`, `item:N`, `kept:UUID`, `via:UUID`, `shape:UUID`, `hole:X,Y`, `edge`.
    pub against: String,
    pub layer: u32,
    /// Copper gap, nm (rounded).
    pub gap: i64,
    /// Requirement including the margin, nm (rounded).
    pub required: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CheckReport {
    pub checked: usize,
    pub violations: Vec<Violation>,
    /// Candidate entries returned by grid queries (scaling diagnostics).
    pub candidates_visited: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Pad,
    Track,
    Via,
    Shape,
    Hole,
    Edge,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RegionKind {
    Shape,
    Keepout { tracks: bool, vias: bool },
}

/// A polygon area: foreign copper shape or keepout.
struct Region {
    poly: Vec<Vec2>,
    bbox: Aabb,
    net: i32,
    tie_nets: Vec<i32>,
    label: String,
    kind: RegionKind,
}

pub(crate) fn point_in_poly(p: Vec2, poly: &[Vec2]) -> bool {
    let n = poly.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

#[derive(Clone)]
enum Geo {
    Track(Track),
    Skel(Skeleton),
}

#[derive(Clone)]
struct Obstacle {
    /// Drill of an indexed via (hole-to-hole target); board holes are `Kind::Hole` obstacles.
    hole: Option<Track>,
    geo: Geo,
    net: i32,
    kind: Kind,
    label: String,
    /// Own clearance contribution (max of the obstacle net's clearance and, for pads, the local override), nm.
    own_clr: f64,
    /// Nets bridged by the net tie this obstacle belongs to: not an obstacle for them (the
    /// net-less copper graphics of a net-tie footprint, KiCad case 25 in `tests/net_tie.rs`).
    tie_nets: Vec<i32>,
    /// Pads of a net-tie footprint: the footprint's other tie-group pads `(net, copper on this
    /// layer)`. A collision with an item of one of these nets is allowed only inside that pad
    /// ([`tie_exclusion`], KiCad `DRC_ENGINE::IsNetTieExclusion`).
    tie_pads: Vec<(i32, Skeleton)>,
}

impl Obstacle {
    fn gap(&self, item: &Track) -> f64 {
        match &self.geo {
            Geo::Track(t) => copper_gap(item, t),
            Geo::Skel(s) => copper_gap_skeleton(item, s),
        }
    }

    fn bbox(&self) -> Aabb {
        match &self.geo {
            Geo::Track(t) => t.prim.bbox().grow(0.5 * t.w),
            Geo::Skel(s) => skel_bbox(s),
        }
    }
}

fn pts_bbox(pts: impl Iterator<Item = Vec2>, r: f64) -> Aabb {
    let (mut lo, mut hi) = (
        Vec2 {
            x: f64::INFINITY,
            y: f64::INFINITY,
        },
        Vec2 {
            x: f64::NEG_INFINITY,
            y: f64::NEG_INFINITY,
        },
    );
    for p in pts {
        lo = Vec2 {
            x: lo.x.min(p.x),
            y: lo.y.min(p.y),
        };
        hi = Vec2 {
            x: hi.x.max(p.x),
            y: hi.y.max(p.y),
        };
    }
    Aabb { min: lo, max: hi }.grow(r)
}

fn skel_bbox(s: &Skeleton) -> Aabb {
    match s {
        Skeleton::Point { p, r } => pts_bbox([*p].into_iter(), *r),
        Skeleton::Seg { s, r } => pts_bbox([s.a, s.b].into_iter(), *r),
        Skeleton::Poly { pts, r } => pts_bbox(pts.iter().copied(), *r),
    }
}

fn zero_seg(c: Vec2, w: f64) -> Track {
    Track {
        prim: Prim::Seg(Seg { a: c, b: c }),
        w,
    }
}

/// Copper-free hole as a track: a circle of diameter `d`, or for a slot the capsule of its
/// short side along its long axis (axis-aligned, as `ObstacleSet::build` models it: the
/// router and the Checker must see the same hole). O(1).
fn hole_track(h: &crate::model::Hole) -> Track {
    let c = h.at.to_vec2();
    match h.slot {
        Some([w, hh]) if w != hh => {
            let half = 0.5 * (w - hh).abs() as f64;
            let axis = if w >= hh {
                Vec2 { x: half, y: 0.0 }
            } else {
                Vec2 { x: 0.0, y: half }
            };
            Track {
                prim: Prim::Seg(Seg {
                    a: c - axis,
                    b: c + axis,
                }),
                w: w.min(hh) as f64,
            }
        }
        _ => zero_seg(c, h.d as f64),
    }
}

pub struct Checker {
    rules_min_clearance: f64,
    hole_clearance: f64,
    /// Clearance of a placed via's drill to foreign copper (`Rules::via_hole_clear`).
    via_hole_clearance: f64,
    edge_clearance: f64,
    margin: f64,
    clearances: HashMap<i32, f64>,
    default_clearance: f64,
    cell: f64,
    hole_to_hole: f64,
    obstacles: Vec<Obstacle>,
    grids: HashMap<u32, Grid<usize>>,
    regions: Vec<Region>,
    region_grids: HashMap<u32, Grid<usize>>,
}

impl Checker {
    /// Build one grid per copper layer from the model (math frame, see `BoardModel::from_json`).
    pub fn new(model: &BoardModel, opts: &Options) -> Checker {
        let r = &model.rules;
        let all: Vec<u32> = model.all_layer_ids();

        let margin = opts.margin_nm as f64;
        let mut max_clr = r
            .default
            .clearance
            .max(r.min_clearance)
            .max(r.hole_clearance)
            .max(r.edge_clearance);
        max_clr = r
            .per_net
            .values()
            .map(|n| n.clearance)
            .fold(max_clr, i64::max);
        max_clr = model
            .pads
            .iter()
            .filter_map(|p| p.local_clearance)
            .fold(max_clr, i64::max);
        let max_w = model
            .kept
            .iter()
            .map(|k| k.width)
            .chain(model.vias.iter().map(|v| v.d))
            .chain(r.per_net.values().map(|n| n.width.max(n.via_d)))
            .chain([r.default.width, r.default.via_d])
            .max()
            .unwrap_or(0);
        let cell = ((max_w + max_clr) as f64 + margin).max(100_000.0);

        let mut c = Checker {
            rules_min_clearance: r.min_clearance as f64,
            hole_clearance: r.hole_clearance as f64,
            via_hole_clearance: r.via_hole_clear() as f64,
            edge_clearance: r.edge_clearance as f64,
            margin,
            clearances: r
                .per_net
                .iter()
                .map(|(k, v)| (*k, v.clearance as f64))
                .collect(),
            default_clearance: r.default.clearance as f64,
            cell,
            hole_to_hole: r.hole_to_hole as f64,
            obstacles: Vec::new(),
            grids: HashMap::new(),
            regions: Vec::new(),
            region_grids: HashMap::new(),
        };
        for l in &all {
            c.grids.insert(*l, Grid::new(cell));
        }

        let tie_nets_of = |ref_: &str| model.tie_nets_of(ref_);
        let tie_pads = |pi: usize, l: u32| -> Vec<(i32, Skeleton)> {
            model
                .tie_group_pads_of(pi)
                .into_iter()
                .map(|j| &model.pads[j])
                .filter(|q| q.net != 0 && q.layers.contains(&l))
                .map(|q| (q.net, model.skeleton(q, l)))
                .collect()
        };

        let pad_by_hole = model.pad_by_hole();
        for (pi, p) in model.pads.iter().enumerate() {
            for &l in &p.layers {
                // KiCad: max(item class, pad class, pad local override).
                let own = model.pad_own_clearance(p, c.clr(p.net));
                c.add(
                    &[l],
                    Obstacle {
                        hole: None,
                        geo: Geo::Skel(model.skeleton(p, l)),
                        net: p.net,
                        kind: Kind::Pad,
                        label: format!("pad:{}:{}", p.ref_, p.number),
                        own_clr: own,
                        tie_nets: Vec::new(),
                        tie_pads: tie_pads(pi, l),
                    },
                );
            }
        }
        for v in &model.vias {
            let ls = via_layers(&v.layers, &all);
            c.add(
                &ls,
                Obstacle {
                    hole: None,
                    geo: Geo::Track(zero_seg(v.at.to_vec2(), v.d as f64)),
                    net: v.net,
                    kind: Kind::Via,
                    label: format!("via:{}", v.uuid),
                    own_clr: c.clr(v.net),
                    tie_nets: Vec::new(),
                    tie_pads: Vec::new(),
                },
            );
            if v.drill > 0 {
                // The drill pierces every copper layer, whatever layers carry via copper.
                c.add(
                    &all,
                    Obstacle {
                        hole: None,
                        geo: Geo::Track(zero_seg(v.at.to_vec2(), v.drill as f64)),
                        net: v.net,
                        kind: Kind::Hole,
                        label: format!("viahole:{}", v.uuid),
                        own_clr: 0.0,
                        tie_nets: Vec::new(),
                        tie_pads: Vec::new(),
                    },
                );
            }
        }
        for k in &model.kept {
            c.add(
                &[k.layer],
                Obstacle {
                    hole: None,
                    geo: Geo::Track(Track {
                        prim: dequantise(&k.prim),
                        w: k.width as f64,
                    }),
                    net: k.net,
                    kind: Kind::Track,
                    label: format!("kept:{}", k.uuid),
                    own_clr: c.clr(k.net),
                    tie_nets: Vec::new(),
                    tie_pads: Vec::new(),
                },
            );
        }
        for s in &model.copper_shapes {
            let tie = s.owner_ref.as_deref().map(tie_nets_of).unwrap_or_default();
            let n = s.poly.len();
            if n >= 3 {
                c.add_region(
                    &[s.layer],
                    Region {
                        poly: s.poly.iter().map(|p| p.to_vec2()).collect(),
                        bbox: Aabb {
                            min: Vec2 { x: 0.0, y: 0.0 },
                            max: Vec2 { x: 0.0, y: 0.0 },
                        },
                        net: s.net,
                        tie_nets: tie.clone(),
                        label: format!("shape:{}", s.uuid),
                        kind: RegionKind::Shape,
                    },
                );
            }
            for i in 0..n {
                let seg = Seg {
                    a: s.poly[i].to_vec2(),
                    b: s.poly[(i + 1) % n].to_vec2(),
                };
                c.add(
                    &[s.layer],
                    Obstacle {
                        hole: None,
                        geo: Geo::Track(Track {
                            prim: Prim::Seg(seg),
                            w: 0.0,
                        }),
                        net: s.net,
                        kind: Kind::Shape,
                        label: format!("shape:{}", s.uuid),
                        own_clr: c.clr(s.net),
                        tie_nets: tie.clone(),
                        tie_pads: Vec::new(),
                    },
                );
            }
        }
        for k in &model.keepouts {
            if k.poly.len() >= 3 && (k.no_tracks || k.no_vias) {
                c.add_region(
                    &k.layers,
                    Region {
                        poly: k.poly.iter().map(|p| p.to_vec2()).collect(),
                        bbox: Aabb {
                            min: Vec2 { x: 0.0, y: 0.0 },
                            max: Vec2 { x: 0.0, y: 0.0 },
                        },
                        net: 0,
                        tie_nets: Vec::new(),
                        label: format!("keepout:{}", k.name),
                        kind: RegionKind::Keepout {
                            tracks: k.no_tracks,
                            vias: k.no_vias,
                        },
                    },
                );
            }
        }
        for h in &model.holes {
            // A PTH hole belongs to its pad: same-net copper may touch the pad around it.
            let owner = pad_by_hole.get(&h.at).map(|&i| &model.pads[i]);
            c.add(
                &all,
                Obstacle {
                    hole: None,
                    geo: Geo::Track(hole_track(h)),
                    net: owner.map_or(0, |p| p.net),
                    kind: Kind::Hole,
                    label: format!("hole:{},{}", h.at.x, h.at.y),
                    own_clr: 0.0,
                    // the drill of a net-tie pad is an obstacle for the other tied nets (task NT)
                    tie_nets: Vec::new(),
                    tie_pads: Vec::new(),
                },
            );
        }
        for o in &model.outline {
            c.add(
                &all,
                Obstacle {
                    hole: None,
                    geo: Geo::Track(Track {
                        prim: dequantise(o),
                        w: 0.0,
                    }),
                    net: 0,
                    kind: Kind::Edge,
                    label: "edge".into(),
                    own_clr: 0.0,
                    tie_nets: Vec::new(),
                    tie_pads: Vec::new(),
                },
            );
        }
        c
    }

    fn clr(&self, net: i32) -> f64 {
        *self.clearances.get(&net).unwrap_or(&self.default_clearance)
    }

    fn add(&mut self, layers: &[u32], o: Obstacle) {
        let idx = self.obstacles.len();
        let bb = o.bbox();
        for l in layers {
            let cell = self.cell;
            self.grids
                .entry(*l)
                .or_insert_with(|| Grid::new(cell))
                .insert(bb, idx);
        }
        self.obstacles.push(o);
    }

    fn add_region(&mut self, layers: &[u32], mut r: Region) {
        r.bbox = pts_bbox(r.poly.iter().copied(), 0.0);
        let idx = self.regions.len();
        for l in layers {
            let cell = self.cell;
            self.region_grids
                .entry(*l)
                .or_insert_with(|| Grid::new(cell))
                .insert(r.bbox, idx);
        }
        self.regions.push(r);
    }

    /// Gap of `item` to a region: `Some(gap <= 0)` when it intersects or lies inside.
    fn region_hit(item: &Track, r: &Region) -> Option<f64> {
        let n = r.poly.len();
        let mut best = f64::INFINITY;
        for i in 0..n {
            let edge = Track {
                prim: Prim::Seg(Seg {
                    a: r.poly[i],
                    b: r.poly[(i + 1) % n],
                }),
                w: 0.0,
            };
            best = best.min(copper_gap(item, &edge));
        }
        if best <= 0.0 {
            return Some(best);
        }
        let start = match &item.prim {
            Prim::Seg(s) => s.a,
            Prim::Arc(a) => a.start(),
        };
        point_in_poly(start, &r.poly).then_some(0.0)
    }

    fn required(&self, item_net: i32, o: &Obstacle, own_clr: f64) -> f64 {
        let base = match o.kind {
            Kind::Hole => self.hole_clearance,
            Kind::Edge => self.edge_clearance,
            _ => self
                .clr(item_net)
                .max(own_clr)
                .max(self.rules_min_clearance),
        };
        base + self.margin
    }

    /// KiCad's net-tie exclusion for a pad `o` of a net-tie footprint against an item of `net`
    /// (copper `track`): allowed iff the collision lies inside a tie-group pad of `net` of the
    /// same footprint ([`tie_exclusion`]). O(1) for every other obstacle.
    fn tie_excluded(o: &Obstacle, net: i32, track: &Track) -> bool {
        let Geo::Skel(own) = &o.geo else {
            return false;
        };
        o.tie_pads
            .iter()
            .any(|(n, q)| *n == net && tie_exclusion(track, own, q))
    }

    fn is_exempt(item_net: i32, o: &Obstacle) -> bool {
        (o.net != 0 && o.net == item_net) || o.tie_nets.contains(&item_net)
    }

    /// A through via of `size` at `at` (math frame, nm) violates nothing: net 0 is foreign to
    /// every pad / track / shape, so copper clearance, edge, NPTH / hole clearance,
    /// hole-to-hole and `no_vias` keepouts all apply. Build the checker with margin `eps` for
    /// the site fit (P3). O(q) grid candidates.
    pub fn via_fits(&self, at: Vec2, size: crate::vias::ViaSize) -> bool {
        let item = Item::Via(crate::model::ViaItem {
            kind: "via".into(),
            net: 0,
            at: crate::model::Pt {
                x: at.x.round() as i64,
                y: (-at.y).round() as i64,
            },
            d: size.d,
            drill: size.drill,
            layers: Vec::new(),
            group: None,
            wire: None,
        });
        self.check_items(&[item]).violations.is_empty()
    }

    /// Grid cell size of the checker (an [`ItemIndex`] must use the same).
    pub fn cell(&self) -> f64 {
        self.cell
    }

    /// Check `items` (wire frame: KiCad, Y down) against the board and against each other.
    /// Each item is tested against grid candidates and the earlier items only.
    /// O(n * q): `q` grid candidates per item.
    pub fn check_items(&self, items: &[Item]) -> CheckReport {
        let mut rep = CheckReport::default();
        let mut idx = ItemIndex::new(self.cell);
        let all_layers = self.all_layers();
        for (i, item) in items.iter().enumerate() {
            self.check_one(&idx, i, item, &[], &all_layers, &mut rep);
            idx.insert(item);
        }
        rep
    }

    /// `items` against the board obstacles and against every indexed item except the ids in
    /// `skip` (sorted) and except each other (callers index what they keep). `Violation.against`
    /// for an indexed item is `item:<id>`. Items are checked in parallel on the current rayon
    /// pool (one report per item, concatenated in item order, so the result is independent of
    /// the thread count). O(|items| * q), independent of the index size; wall time / threads.
    pub fn check_against(&self, idx: &ItemIndex, items: &[Item], skip: &[usize]) -> CheckReport {
        use rayon::prelude::*;
        let all_layers = self.all_layers();
        let one = |(i, item): (usize, &Item)| {
            let mut r = CheckReport::default();
            self.check_one(idx, i, item, skip, &all_layers, &mut r);
            r
        };
        let parts: Vec<CheckReport> = if items.len() < PAR_MIN_ITEMS {
            items.iter().enumerate().map(one).collect()
        } else {
            items.par_iter().enumerate().map(one).collect()
        };
        let mut rep = CheckReport::default();
        for r in parts {
            rep.checked += r.checked;
            rep.candidates_visited += r.candidates_visited;
            rep.violations.extend(r.violations);
        }
        rep
    }

    fn all_layers(&self) -> Vec<u32> {
        let mut all_layers: Vec<u32> = self.grids.keys().copied().collect();
        all_layers.sort_unstable();
        all_layers
    }

    fn check_one(
        &self,
        idx: &ItemIndex,
        i: usize,
        item: &Item,
        skip: &[usize],
        all_layers: &[u32],
        rep: &mut CheckReport,
    ) {
        let (net, track) = item_track(item);
        let layers = match item {
            Item::Track(t) => vec![t.layer],
            // a via without listed layers occupies every layer (as in the index)
            Item::Via(v) if v.layers.is_empty() => all_layers.to_vec(),
            Item::Via(v) => via_layers(&v.layers, all_layers),
        };
        let bb = track.prim.bbox().grow(0.5 * track.w);
        let mut found: Vec<Violation> = Vec::new();
        let drill_track = match item {
            Item::Via(v) if v.drill > 0 => Some(Track {
                prim: track.prim,
                w: v.drill as f64,
            }),
            _ => None,
        };
        let is_via = matches!(item, Item::Via(_));
        for &layer in &layers {
            let mut test = |o: &Obstacle, own_clr: f64| {
                if let Some(d) = &drill_track {
                    let hole_gap = match (&o.hole, o.kind) {
                        (_, Kind::Hole) => Some(o.gap(d)),
                        (Some(h), _) => Some(copper_gap(d, h)),
                        _ => None,
                    };
                    if let Some(gap) = hole_gap {
                        let req = self.hole_to_hole + self.margin;
                        if gap < req {
                            let (g, r) = (gap.round() as i64, req.round() as i64);
                            match found.iter_mut().find(|v| {
                                v.against == o.label && v.layer == layer && v.required == r
                            }) {
                                Some(v) => v.gap = v.gap.min(g),
                                None => found.push(Violation {
                                    item: i,
                                    against: o.label.clone(),
                                    layer,
                                    gap: g,
                                    required: r,
                                }),
                            }
                        }
                    }
                }
                if Self::is_exempt(net, o) {
                    return;
                }
                // Hole clearance between foreign copper and a placed via's drill
                // (`Rules::via_hole_clear`): the drill of an indexed via against this item's
                // copper, and this via's drill against the obstacle's copper (model vias and
                // holes carry their own `Kind::Hole` obstacle with `hole_clearance`).
                let holes = [
                    o.hole.as_ref().map(|h| copper_gap(&track, h)),
                    drill_track.as_ref().filter(|_| matches!(o.kind, Kind::Pad | Kind::Track | Kind::Via | Kind::Shape)).map(|d| o.gap(d)),
                ];
                for gap in holes.into_iter().flatten() {
                    let req = self.via_hole_clearance + self.margin;
                    if gap < req {
                        let (g, r) = (gap.round() as i64, req.round() as i64);
                        match found.iter_mut().find(|v| v.against == o.label && v.layer == layer && v.required == r) {
                            Some(v) => v.gap = v.gap.min(g),
                            None => found.push(Violation { item: i, against: o.label.clone(), layer, gap: g, required: r }),
                        }
                    }
                }
                let req = self.required(net, o, own_clr);
                let gap = o.gap(&track);
                if gap < req && !Self::tie_excluded(o, net, &track) {
                    let (g, r) = (gap.round() as i64, req.round() as i64);
                    match found
                        .iter_mut()
                        .find(|v| v.against == o.label && v.layer == layer && v.required == r)
                    {
                        Some(v) => v.gap = v.gap.min(g),
                        None => found.push(Violation {
                            item: i,
                            against: o.label.clone(),
                            layer,
                            gap: g,
                            required: r,
                        }),
                    }
                }
            };
            if let Some(g) = self.grids.get(&layer) {
                let (cands, v) = g.query_counted(bb);
                rep.candidates_visited += v;
                cands
                    .iter()
                    .for_each(|&k| test(&self.obstacles[k], self.obstacles[k].own_clr));
            }
            for key in [layer, WILD] {
                if let Some(g) = idx.grids.get(&key) {
                    let (cands, v) = g.query_counted(bb);
                    rep.candidates_visited += v;
                    for &k in &cands {
                        if skip.binary_search(&k).is_ok() {
                            continue;
                        }
                        if let Some(slot) = &idx.obstacles[k] {
                            test(&slot.0, self.clr(slot.0.net));
                        }
                    }
                }
            }
            if let Some(g) = self.region_grids.get(&layer) {
                let (cands, v) = g.query_counted(bb);
                rep.candidates_visited += v;
                for &k in &cands {
                    let r = &self.regions[k];
                    let (applies, req) = match r.kind {
                        RegionKind::Shape => (
                            !((r.net != 0 && r.net == net) || r.tie_nets.contains(&net)),
                            self.clr(net)
                                .max(self.clr(r.net))
                                .max(self.rules_min_clearance)
                                + self.margin,
                        ),
                        RegionKind::Keepout { tracks, vias } => {
                            (if is_via { vias } else { tracks }, 0.0)
                        }
                    };
                    if !applies || !r.bbox.overlaps(&bb) {
                        continue;
                    }
                    if let Some(gap) = Self::region_hit(&track, r) {
                        let g = gap.round() as i64;
                        match found
                            .iter_mut()
                            .find(|v| v.against == r.label && v.layer == layer)
                        {
                            Some(v) => v.gap = v.gap.min(g),
                            None => found.push(Violation {
                                item: i,
                                against: r.label.clone(),
                                layer,
                                gap: g,
                                required: req.round() as i64,
                            }),
                        }
                    }
                }
            }
        }
        rep.violations.extend(found);
        rep.checked += 1;
    }
}

/// Samples along an item's centreline for [`tie_exclusion`] (cap) and their spacing, nm.
const TIE_SAMPLES_MAX: usize = 1024;
const TIE_SAMPLE_STEP_NM: f64 = 2_000.0;

/// KiCad 10 `DRC_ENGINE::IsNetTieExclusion` (verified with kicad-cli 10.0.8, task NT,
/// `tests/net_tie.rs`): a collision of `item` with the net-tie pad `own` is allowed iff the
/// collision position lies inside the item net's tie pad `q`. The position is taken as the
/// point of `own`'s copper nearest to the item centreline; conservatively the nearest
/// centreline point must lie in `q` as well. Both lie in `q` only when `own` and `q` overlap
/// (fused tie pads); a gapped tie pad is never excluded. The nearest centreline point is found
/// on ≤ [`TIE_SAMPLES_MAX`] samples (first minimum, deterministic).
/// O(s · v), s samples, v skeleton vertices; only run for a violation against a tie pad.
fn tie_exclusion(item: &Track, own: &Skeleton, q: &Skeleton) -> bool {
    let len = item.prim.length();
    let n = ((len / TIE_SAMPLE_STEP_NM).ceil() as usize).clamp(1, TIE_SAMPLES_MAX - 1);
    let at = |i: usize| {
        let f = i as f64 / n as f64;
        match &item.prim {
            Prim::Seg(s) => s.a + (s.b - s.a) * f,
            Prim::Arc(a) => a.point_at(f),
        }
    };
    let (mut best, mut best_d) = (at(0), f64::INFINITY);
    for i in 0..=n {
        let c = at(i);
        let d = point_skeleton(c, own);
        if d < best_d {
            (best, best_d) = (c, d);
        }
    }
    let pos = skeleton_closest_point(own, best);
    point_skeleton(pos, q) <= 0.0 && point_skeleton(best, q) <= 0.0
}

/// Fewest items for which `check_against` fans out over the pool.
const PAR_MIN_ITEMS: usize = 16;

/// Grid key of items occupying every layer (vias without a listed layer).
const WILD: u32 = u32::MAX;

/// Net and checker-frame track (flipped, dequantised; a via is a zero-length segment of
/// width `d`) of an item.
fn item_track(item: &Item) -> (i32, Track) {
    match item {
        Item::Track(t) => (
            t.net,
            Track {
                prim: dequantise(&flip_track(t.prim)),
                w: t.width as f64,
            },
        ),
        Item::Via(v) => (
            v.net,
            zero_seg(
                Vec2 {
                    x: v.at.x as f64,
                    y: -(v.at.y as f64),
                },
                v.d as f64,
            ),
        ),
    }
}

/// Persistent index of emitted items, usable as obstacles by [`Checker::check_against`].
/// Slots are reused (smallest free id first) so ids are deterministic. A via lists its layers;
/// one without listed layers blocks every layer. Insert and remove are
/// O(cells covered by the item); the clearance of an indexed item is resolved by the checker.
#[derive(Clone)]
pub struct ItemIndex {
    cell: f64,
    /// Obstacle and the grid keys it was placed under.
    obstacles: Vec<Option<(Obstacle, Vec<u32>)>>,
    grids: HashMap<u32, Grid<usize>>,
    free: BinaryHeap<Reverse<usize>>,
    live: usize,
}

impl ItemIndex {
    /// `cell` = [`Checker::cell`].
    pub fn new(cell: f64) -> ItemIndex {
        ItemIndex {
            cell,
            obstacles: Vec::new(),
            grids: HashMap::new(),
            free: BinaryHeap::new(),
            live: 0,
        }
    }

    /// Index `item`; returns its id (label `item:<id>`). O(cells covered).
    pub fn insert(&mut self, item: &Item) -> usize {
        let (net, track) = item_track(item);
        let id = match self.free.pop() {
            Some(Reverse(i)) => i,
            None => {
                self.obstacles.push(None);
                self.obstacles.len() - 1
            }
        };
        let (keys, kind, hole) = match item {
            Item::Track(t) => (vec![t.layer], Kind::Track, None),
            Item::Via(v) => {
                let keys =
                    if v.layers.is_empty() || (v.layers.contains(&0) && v.layers.contains(&2)) {
                        vec![WILD]
                    } else {
                        v.layers.clone()
                    };
                let hole = (v.drill > 0).then_some(Track {
                    prim: track.prim,
                    w: v.drill as f64,
                });
                (keys, Kind::Via, hole)
            }
        };
        let o = Obstacle {
            hole,
            geo: Geo::Track(track),
            net,
            kind,
            label: format!("item:{id}"),
            own_clr: 0.0,
            tie_nets: Vec::new(),
            tie_pads: Vec::new(),
        };
        let bb = o.bbox();
        for &k in &keys {
            let cell = self.cell;
            self.grids
                .entry(k)
                .or_insert_with(|| Grid::new(cell))
                .insert(bb, id);
        }
        self.obstacles[id] = Some((o, keys));
        self.live += 1;
        id
    }

    /// Whether item `id` is a live via. O(1).
    pub fn is_via(&self, id: usize) -> bool {
        matches!(self.obstacles.get(id), Some(Some((o, _))) if o.kind == Kind::Via)
    }

    /// Drop item `id` (no-op if absent) and free its slot. O(cells covered).
    pub fn remove(&mut self, id: usize) {
        let Some((o, keys)) = self.obstacles.get_mut(id).and_then(Option::take) else {
            return;
        };
        let bb = o.bbox();
        for k in keys {
            if let Some(g) = self.grids.get_mut(&k) {
                g.remove(bb, id);
                if g.is_empty() {
                    self.grids.remove(&k);
                }
            }
        }
        self.free.push(Reverse(id));
        self.live -= 1;
    }

    pub fn len(&self) -> usize {
        self.live
    }

    pub fn is_empty(&self) -> bool {
        self.live == 0
    }
}
