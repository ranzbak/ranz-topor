//! Clearance checker: grid-indexed copper gap tests of candidate items against the board.
//!
//! Gaps are copper-to-copper (`copper_gap` / `copper_gap_skeleton`), compared with
//! `required + margin`. Copper-shape polygons are checked by their edges (grid obstacles) plus
//! containment of the item's start point (gap 0). Keepouts (rule areas) and shape polygons
//! live in a per-layer grid of regions (`region_grids`, bbox query, then exact edge tests).

use crate::geom::prim::Aabb;
use crate::geom::{
    copper_gap, copper_gap_skeleton, dequantise, Grid, Prim, Seg, Skeleton, Track, Vec2,
};
use crate::model::{flip_track, via_layers, BoardModel, Item, Options, Pad};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

fn point_in_poly(p: Vec2, poly: &[Vec2]) -> bool {
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

enum Geo {
    Track(Track),
    Skel(Skeleton),
}

struct Obstacle {
    geo: Geo,
    net: i32,
    kind: Kind,
    label: String,
    /// Own clearance contribution (max of the obstacle net's clearance and, for pads, the local override), nm.
    own_clr: f64,
    /// Nets bridged by the net tie this obstacle belongs to: not an obstacle for them.
    tie_nets: Vec<i32>,
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

pub struct Checker {
    rules_min_clearance: f64,
    hole_clearance: f64,
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
        let group_nets = |p: &Pad| model.pad_group_nets(p);

        let pad_by_hole = model.pad_by_hole();
        for p in &model.pads {
            for &l in &p.layers {
                // KiCad: max(item class, pad class, pad local override).
                let own = (p.local_clearance.unwrap_or(0) as f64).max(c.clr(p.net));
                c.add(
                    &[l],
                    Obstacle {
                        geo: Geo::Skel(model.skeleton(p, l)),
                        net: p.net,
                        kind: Kind::Pad,
                        label: format!("pad:{}:{}", p.ref_, p.number),
                        own_clr: own,
                        tie_nets: group_nets(p),
                    },
                );
            }
        }
        for v in &model.vias {
            let ls = via_layers(&v.layers, &all);
            c.add(
                &ls,
                Obstacle {
                    geo: Geo::Track(zero_seg(v.at.to_vec2(), v.d as f64)),
                    net: v.net,
                    kind: Kind::Via,
                    label: format!("via:{}", v.uuid),
                    own_clr: c.clr(v.net),
                    tie_nets: Vec::new(),
                },
            );
            if v.drill > 0 {
                // The drill pierces every copper layer, whatever layers carry via copper.
                c.add(
                    &all,
                    Obstacle {
                        geo: Geo::Track(zero_seg(v.at.to_vec2(), v.drill as f64)),
                        net: v.net,
                        kind: Kind::Hole,
                        label: format!("viahole:{}", v.uuid),
                        own_clr: 0.0,
                        tie_nets: Vec::new(),
                    },
                );
            }
        }
        for k in &model.kept {
            c.add(
                &[k.layer],
                Obstacle {
                    geo: Geo::Track(Track {
                        prim: dequantise(&k.prim),
                        w: k.width as f64,
                    }),
                    net: k.net,
                    kind: Kind::Track,
                    label: format!("kept:{}", k.uuid),
                    own_clr: c.clr(k.net),
                    tie_nets: Vec::new(),
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
                        geo: Geo::Track(Track {
                            prim: Prim::Seg(seg),
                            w: 0.0,
                        }),
                        net: s.net,
                        kind: Kind::Shape,
                        label: format!("shape:{}", s.uuid),
                        own_clr: c.clr(s.net),
                        tie_nets: tie.clone(),
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
                    geo: Geo::Track(zero_seg(h.at.to_vec2(), h.d as f64)),
                    net: owner.map_or(0, |p| p.net),
                    kind: Kind::Hole,
                    label: format!("hole:{},{}", h.at.x, h.at.y),
                    own_clr: 0.0,
                    tie_nets: owner.map(group_nets).unwrap_or_default(),
                },
            );
        }
        for o in &model.outline {
            c.add(
                &all,
                Obstacle {
                    geo: Geo::Track(Track {
                        prim: dequantise(o),
                        w: 0.0,
                    }),
                    net: 0,
                    kind: Kind::Edge,
                    label: "edge".into(),
                    own_clr: 0.0,
                    tie_nets: Vec::new(),
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

    fn required(&self, item_net: i32, o: &Obstacle) -> f64 {
        let base = match o.kind {
            Kind::Hole => self.hole_clearance,
            Kind::Edge => self.edge_clearance,
            _ => self
                .clr(item_net)
                .max(o.own_clr)
                .max(self.rules_min_clearance),
        };
        base + self.margin
    }

    fn is_exempt(item_net: i32, o: &Obstacle) -> bool {
        (o.net != 0 && o.net == item_net) || o.tie_nets.contains(&item_net)
    }

    /// Check `items` (wire frame: KiCad, Y down) against the board and against each other.
    /// Each item is tested against grid candidates and the earlier items only.
    pub fn check_items(&self, items: &[Item]) -> CheckReport {
        let mut rep = CheckReport::default();
        let mut all_layers: Vec<u32> = self.grids.keys().copied().collect();
        all_layers.sort_unstable();
        // Earlier items, inserted as the walk proceeds.
        let mut seen: Vec<Obstacle> = Vec::new();
        let mut seen_grids: HashMap<u32, Grid<usize>> = HashMap::new();

        for (i, item) in items.iter().enumerate() {
            let (net, layers, track) = match item {
                Item::Track(t) => (
                    t.net,
                    vec![t.layer],
                    Track {
                        prim: dequantise(&flip_track(t.prim)),
                        w: t.width as f64,
                    },
                ),
                Item::Via(v) => (
                    v.net,
                    via_layers(&v.layers, &all_layers),
                    zero_seg(
                        Vec2 {
                            x: v.at.x as f64,
                            y: -(v.at.y as f64),
                        },
                        v.d as f64,
                    ),
                ),
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
                let mut test = |o: &Obstacle| {
                    if let (Some(d), Kind::Hole) = (&drill_track, o.kind) {
                        let gap = o.gap(d);
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
                    if Self::is_exempt(net, o) {
                        return;
                    }
                    let req = self.required(net, o);
                    let gap = o.gap(&track);
                    if gap < req {
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
                    cands.iter().for_each(|&k| test(&self.obstacles[k]));
                }
                if let Some(g) = seen_grids.get(&layer) {
                    let (cands, v) = g.query_counted(bb);
                    rep.candidates_visited += v;
                    cands.iter().for_each(|&k| test(&seen[k]));
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

            // Insert as an obstacle for later items.
            let idx = seen.len();
            let o = Obstacle {
                geo: Geo::Track(track),
                net,
                kind: if matches!(item, Item::Via(_)) {
                    Kind::Via
                } else {
                    Kind::Track
                },
                label: format!("item:{i}"),
                own_clr: self.clr(net),
                tie_nets: Vec::new(),
            };
            let obb = o.bbox();
            for &layer in &layers {
                seen_grids
                    .entry(layer)
                    .or_insert_with(|| Grid::new(self.cell))
                    .insert(obb, idx);
            }
            seen.push(o);
        }
        rep
    }
}
