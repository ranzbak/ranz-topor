//! Obstacle model for the taut smoother (20 §1-3): per layer, anchor circles (`Anchor`, a core
//! point with radius `r_core`) and walls (`Wall`, a core segment or arc with `r_core`).
//!
//! All coordinates are nm in the math frame (`BoardModel::from_json` already flipped Y). The
//! inflated radius of an obstacle for a wire of width `w` and net `n` is
//! `rho = r_core + clearance + w/2 + margin` ([`ObstacleSet::rho_anchor`], [`ObstacleSet::rho_wall`]);
//! clearance and exemption rules mirror `Checker` (`required`, `is_exempt`).
//!
//! Build is O(items); queries go through one `Grid` per layer (cell = the Checker's formula),
//! so a query is O(1) on average, followed by an exact bounding-box filter.

use crate::geom::prim::Aabb;
use crate::geom::{dequantise, Grid, Prim, Seg, Skeleton, Vec2};
use crate::model::{via_layers, BoardModel, Options};
use std::collections::{BTreeMap, HashMap, HashSet};

pub use crate::geom::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObsKind {
    Pad,
    Via,
    Hole,
    Edge,
    Shape,
    Track,
    Terminal,
    /// Rule area that forbids tracks: no clearance, every net (KiCad `items_not_allowed`).
    Keepout,
}

#[derive(Clone, Debug)]
pub enum Core {
    Point(Vec2),
    Seg(Seg),
    Arc(Arc),
}

impl Core {
    fn bbox(&self) -> Aabb {
        match self {
            Core::Point(p) => Aabb { min: *p, max: *p },
            Core::Seg(s) => Prim::Seg(*s).bbox(),
            Core::Arc(a) => a.bbox(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Anchor {
    pub c: Vec2,
    pub r_core: f64,
    pub net: i32,
    pub kind: ObsKind,
    pub own_clr: f64,
    pub tie_nets: Vec<i32>,
    pub label: String,
    /// Non-empty: the anchor is an obstacle only for these nets (DRC spots).
    pub only_nets: Vec<i32>,
}

#[derive(Clone, Debug)]
pub struct Wall {
    pub core: Core,
    pub r_core: f64,
    pub net: i32,
    pub kind: ObsKind,
    pub own_clr: f64,
    pub tie_nets: Vec<i32>,
    pub label: String,
    /// Anchor indices (same layer) at the two ends of the core.
    pub ends: [usize; 2],
}

pub struct LayerObstacles {
    pub anchors: Vec<Anchor>,
    pub walls: Vec<Wall>,
    anchor_grid: Grid<usize>,
    wall_grid: Grid<usize>,
}

impl LayerObstacles {
    fn new(anchors: Vec<Anchor>, walls: Vec<Wall>, cell: f64) -> Self {
        let mut anchor_grid = Grid::new(cell);
        for (i, a) in anchors.iter().enumerate() {
            anchor_grid.insert(Core::Point(a.c).bbox().grow(a.r_core), i);
        }
        let mut wall_grid = Grid::new(cell);
        for (i, w) in walls.iter().enumerate() {
            wall_grid.insert(w.core.bbox().grow(w.r_core), i);
        }
        LayerObstacles {
            anchors,
            walls,
            anchor_grid,
            wall_grid,
        }
    }
}

impl LayerObstacles {
    /// Anchors whose core circle overlaps `bb`, sorted.
    pub fn query_anchors(&self, bb: Aabb) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .anchor_grid
            .query(bb)
            .into_iter()
            .filter(|&i| {
                let a = &self.anchors[i];
                Core::Point(a.c).bbox().grow(a.r_core).overlaps(&bb)
            })
            .collect();
        v.sort_unstable();
        v
    }

    /// Walls whose core (grown by `r_core`) overlaps `bb`, sorted.
    pub fn query_walls(&self, bb: Aabb) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .wall_grid
            .query(bb)
            .into_iter()
            .filter(|&i| {
                let w = &self.walls[i];
                w.core.bbox().grow(w.r_core).overlaps(&bb)
            })
            .collect();
        v.sort_unstable();
        v
    }
}

/// Junction / dangling point of a smoothed wire: an obstacle for other nets only.
#[derive(Clone, Debug)]
pub struct TerminalObstacle {
    pub layer: u32,
    pub at: Vec2,
    pub net: i32,
    pub width: i64,
    pub label: String,
}

pub struct ObstacleSet {
    layers: BTreeMap<u32, LayerObstacles>,
    empty: LayerObstacles,
    margin: f64,
    cell: f64,
    min_clearance: f64,
    hole_clearance: f64,
    edge_clearance: f64,
    default_clearance: f64,
    clearances: HashMap<i32, f64>,
}

/// Obstacle attributes shared by every anchor / wall of one source item.
#[derive(Clone)]
struct Meta {
    net: i32,
    kind: ObsKind,
    own_clr: f64,
    tie_nets: Vec<i32>,
    label: String,
}

/// Anchors and walls of one source item (indices local to the fragment).
#[derive(Default)]
struct Frag {
    anchors: Vec<Anchor>,
    walls: Vec<Wall>,
    seen: HashMap<(u64, u64), usize>,
}

fn key(p: Vec2) -> (u64, u64) {
    (p.x.to_bits(), p.y.to_bits())
}

impl Frag {
    fn anchor(&mut self, c: Vec2, r_core: f64, m: &Meta, share: bool) -> usize {
        if share {
            if let Some(&i) = self.seen.get(&key(c)) {
                return i;
            }
        }
        self.anchors.push(Anchor {
            c,
            r_core,
            net: m.net,
            kind: m.kind,
            own_clr: m.own_clr,
            tie_nets: m.tie_nets.clone(),
            label: m.label.clone(),
            only_nets: Vec::new(),
        });
        let i = self.anchors.len() - 1;
        self.seen.insert(key(c), i);
        i
    }

    fn wall(&mut self, core: Core, r_core: f64, m: &Meta, ends: [usize; 2]) {
        self.walls.push(Wall {
            core,
            r_core,
            net: m.net,
            kind: m.kind,
            own_clr: m.own_clr,
            tie_nets: m.tie_nets.clone(),
            label: m.label.clone(),
            ends,
        });
    }

    fn point(c: Vec2, r: f64, m: &Meta) -> Frag {
        let mut f = Frag::default();
        f.anchor(c, r, m, false);
        f
    }

    fn segment(s: Seg, r: f64, m: &Meta) -> Frag {
        let mut f = Frag::default();
        let a = f.anchor(s.a, r, m, false);
        let b = f.anchor(s.b, r, m, false);
        f.wall(Core::Seg(s), r, m, [a, b]);
        f
    }

    /// Closed polygon: one anchor per vertex, one wall per edge.
    fn polygon(pts: &[Vec2], r: f64, m: &Meta) -> Frag {
        let mut f = Frag::default();
        let ids: Vec<usize> = pts.iter().map(|&p| f.anchor(p, r, m, false)).collect();
        let n = pts.len();
        let edges = match n {
            0 | 1 => 0,
            2 => 1,
            _ => n,
        };
        for i in 0..edges {
            let j = (i + 1) % n;
            f.wall(
                Core::Seg(Seg {
                    a: pts[i],
                    b: pts[j],
                }),
                r,
                m,
                [ids[i], ids[j]],
            );
        }
        f
    }

    fn skeleton(sk: &Skeleton, m: &Meta) -> Frag {
        match sk {
            Skeleton::Point { p, r } => Frag::point(*p, *r, m),
            Skeleton::Seg { s, r } => Frag::segment(*s, *r, m),
            Skeleton::Poly { pts, r } => Frag::polygon(pts, *r, m),
        }
    }

    /// A track primitive with endpoint anchors; `share` merges coincident endpoints.
    fn prim(&mut self, p: &Prim, r: f64, m: &Meta, share: bool) {
        match p {
            Prim::Seg(s) => {
                let a = self.anchor(s.a, r, m, share);
                let b = self.anchor(s.b, r, m, share);
                self.wall(Core::Seg(*s), r, m, [a, b]);
            }
            Prim::Arc(arc) => {
                // Model coordinates are integer nm: snap the computed arc ends so joints with
                // neighbouring primitives share one anchor.
                let snap = |p: Vec2| Vec2 {
                    x: p.x.round(),
                    y: p.y.round(),
                };
                let a = self.anchor(snap(arc.start()), r, m, share);
                let b = self.anchor(snap(arc.end()), r, m, share);
                self.wall(Core::Arc(*arc), r, m, [a, b]);
            }
        }
    }
}

#[derive(Default)]
struct Builder {
    layers: BTreeMap<u32, (Vec<Anchor>, Vec<Wall>)>,
}

impl Builder {
    fn add(&mut self, layers: &[u32], f: &Frag) {
        for &l in layers {
            let (anchors, walls) = self.layers.entry(l).or_default();
            let off = anchors.len();
            anchors.extend(f.anchors.iter().cloned());
            walls.extend(f.walls.iter().map(|w| {
                let mut w = w.clone();
                w.ends = [w.ends[0] + off, w.ends[1] + off];
                w
            }));
        }
    }
}

impl ObstacleSet {
    /// `exclude_kept`: uuids of kept items being smoothed (not obstacles). `terminals`: junction /
    /// dangling points of smoothed wires (kind `Terminal`, `r_core = width/2`).
    pub fn build(
        model: &BoardModel,
        opts: &Options,
        exclude_kept: &HashSet<String>,
        terminals: &[TerminalObstacle],
    ) -> ObstacleSet {
        let r = &model.rules;
        let margin = opts.margin_nm as f64;
        let clearances: HashMap<i32, f64> = r
            .per_net
            .iter()
            .map(|(k, v)| (*k, v.clearance as f64))
            .collect();
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
            .chain(terminals.iter().map(|t| t.width))
            .chain(r.per_net.values().map(|n| n.width.max(n.via_d)))
            .chain([r.default.width, r.default.via_d])
            .max()
            .unwrap_or(0);
        let cell = ((max_w + max_clr) as f64 + margin).max(100_000.0);

        let mut set = ObstacleSet {
            layers: BTreeMap::new(),
            empty: LayerObstacles::new(Vec::new(), Vec::new(), cell),
            margin,
            cell,
            min_clearance: r.min_clearance as f64,
            hole_clearance: r.hole_clearance as f64,
            edge_clearance: r.edge_clearance as f64,
            default_clearance: r.default.clearance as f64,
            clearances,
        };

        let all = model.all_layer_ids();
        let mut b = Builder::default();
        for &l in &all {
            b.layers.entry(l).or_default();
        }

        // Pads: per-layer padstack (44 §3.1).
        for p in &model.pads {
            let tie = model.pad_group_nets(p);
            let own = (p.local_clearance.unwrap_or(0) as f64).max(set.clr(p.net));
            for &l in &p.layers {
                let m = Meta {
                    net: p.net,
                    kind: ObsKind::Pad,
                    own_clr: own,
                    tie_nets: tie.clone(),
                    label: format!("pad:{}:{}", p.ref_, p.number),
                };
                b.add(&[l], &Frag::skeleton(&model.skeleton(p, l), &m));
            }
        }

        // Vias: copper anchor on the via layers, drill (Hole) anchor on every layer.
        for v in &model.vias {
            let c = v.at.to_vec2();
            let m = Meta {
                net: v.net,
                kind: ObsKind::Via,
                own_clr: set.clr(v.net),
                tie_nets: Vec::new(),
                label: format!("via:{}", v.uuid),
            };
            b.add(
                &via_layers(&v.layers, &all),
                &Frag::point(c, 0.5 * v.d as f64, &m),
            );
            if v.drill > 0 {
                let hm = Meta {
                    kind: ObsKind::Hole,
                    own_clr: 0.0,
                    label: format!("viahole:{}", v.uuid),
                    ..m
                };
                b.add(&all, &Frag::point(c, 0.5 * v.drill as f64, &hm));
            }
        }

        // Holes: a PTH hole belongs to its pad (same lookup as `Checker::new`).
        let pad_by_hole = model.pad_by_hole();
        for h in &model.holes {
            let owner = pad_by_hole.get(&h.at).map(|&i| &model.pads[i]);
            let m = Meta {
                net: owner.map_or(0, |p| p.net),
                kind: ObsKind::Hole,
                own_clr: 0.0,
                tie_nets: owner.map(|p| model.pad_group_nets(p)).unwrap_or_default(),
                label: format!("hole:{},{}", h.at.x, h.at.y),
            };
            let c = h.at.to_vec2();
            let f = match h.slot {
                Some([w, hh]) => {
                    let (w, hh) = (w as f64, hh as f64);
                    let half = 0.5 * (w - hh).abs();
                    let axis = if w >= hh {
                        Vec2 { x: half, y: 0.0 }
                    } else {
                        Vec2 { x: 0.0, y: half }
                    };
                    let rr = 0.5 * w.min(hh);
                    if half == 0.0 {
                        Frag::point(c, rr, &m)
                    } else {
                        Frag::segment(
                            Seg {
                                a: c - axis,
                                b: c + axis,
                            },
                            rr,
                            &m,
                        )
                    }
                }
                None => Frag::point(c, 0.5 * h.d as f64, &m),
            };
            b.add(&all, &f);
        }

        // Board outline: Edge walls with shared endpoint anchors, on every layer.
        if !model.outline.is_empty() {
            let m = Meta {
                net: 0,
                kind: ObsKind::Edge,
                own_clr: 0.0,
                tie_nets: Vec::new(),
                label: "edge".into(),
            };
            let mut f = Frag::default();
            for o in &model.outline {
                f.prim(&dequantise(o), 0.0, &m, true);
            }
            b.add(&all, &f);
        }

        // Foreign copper shapes: polygon edges (convex or not), tie nets from the owner footprint.
        for s in &model.copper_shapes {
            let m = Meta {
                net: s.net,
                kind: ObsKind::Shape,
                own_clr: set.clr(s.net),
                tie_nets: s
                    .owner_ref
                    .as_deref()
                    .map(|r| model.tie_nets_of(r))
                    .unwrap_or_default(),
                label: format!("shape:{}", s.uuid),
            };
            let pts: Vec<Vec2> = s.poly.iter().map(|p| p.to_vec2()).collect();
            b.add(&[s.layer], &Frag::polygon(&pts, 0.0, &m));
        }

        // Rule areas forbidding tracks (the smoother emits tracks only): polygon edges, all nets.
        for k in model
            .keepouts
            .iter()
            .filter(|k| k.no_tracks && k.poly.len() >= 3)
        {
            let m = Meta {
                net: 0,
                kind: ObsKind::Keepout,
                own_clr: 0.0,
                tie_nets: Vec::new(),
                label: format!("keepout:{}", k.name),
            };
            let pts: Vec<Vec2> = k.poly.iter().map(|p| p.to_vec2()).collect();
            b.add(&k.layers, &Frag::polygon(&pts, 0.0, &m));
        }

        // Kept hand copper that is not being smoothed.
        for k in &model.kept {
            if exclude_kept.contains(&k.uuid) {
                continue;
            }
            let m = Meta {
                net: k.net,
                kind: ObsKind::Track,
                own_clr: set.clr(k.net),
                tie_nets: Vec::new(),
                label: format!("kept:{}", k.uuid),
            };
            let mut f = Frag::default();
            f.prim(&dequantise(&k.prim), 0.5 * k.width as f64, &m, false);
            b.add(&[k.layer], &f);
        }

        // DRC spots: a core-less circle that only its own net avoids (exempt for every other).
        for (i, sp) in opts.drc_spots.iter().enumerate() {
            let m = Meta {
                net: 0,
                kind: ObsKind::Keepout,
                own_clr: 0.0,
                tie_nets: Vec::new(),
                label: format!("spot:{i}"),
            };
            let at = Vec2 {
                x: sp.at.x as f64,
                y: -(sp.at.y as f64),
            };
            let mut f = Frag::point(at, sp.r as f64, &m);
            f.anchors[0].only_nets = vec![sp.net];
            b.add(&[sp.layer], &f);
        }

        for t in terminals {
            let m = Meta {
                net: t.net,
                kind: ObsKind::Terminal,
                own_clr: set.clr(t.net),
                tie_nets: Vec::new(),
                label: t.label.clone(),
            };
            b.add(&[t.layer], &Frag::point(t.at, 0.5 * t.width as f64, &m));
        }

        for (l, (anchors, walls)) in b.layers {
            set.layers
                .insert(l, LayerObstacles::new(anchors, walls, cell));
        }
        set
    }

    fn clr(&self, net: i32) -> f64 {
        *self.clearances.get(&net).unwrap_or(&self.default_clearance)
    }

    /// Build margin ε, nm.
    pub fn margin(&self) -> f64 {
        self.margin
    }

    /// Bound on `rho - r_core` of any obstacle and on any pairwise clearance (max width +
    /// max clearance + margin): grow a query box by this before the exact distance test.
    pub fn query_grow(&self) -> f64 {
        self.cell
    }

    /// Layer ids present, ascending.
    pub fn layers(&self) -> Vec<u32> {
        self.layers.keys().copied().collect()
    }

    /// Obstacles of `layer` (empty set for an unknown layer).
    pub fn layer(&self, layer: u32) -> &LayerObstacles {
        self.layers.get(&layer).unwrap_or(&self.empty)
    }

    /// Same-net or net-tie obstacles are not obstacles (mirrors `Checker::is_exempt`).
    pub fn exempt(a_net: i32, a_tie: &[i32], net: i32) -> bool {
        (a_net != 0 && a_net == net) || a_tie.contains(&net)
    }

    /// [`ObstacleSet::exempt`] for an anchor, plus the `only_nets` rule of DRC spots.
    pub fn exempt_anchor(a: &Anchor, net: i32) -> bool {
        Self::exempt(a.net, &a.tie_nets, net)
            || (!a.only_nets.is_empty() && !a.only_nets.contains(&net))
    }

    /// Required copper clearance of an obstacle for a wire of `net`, nm (without margin).
    pub fn clearance(&self, net: i32, kind: ObsKind, _obs_net: i32, own_clr: f64) -> f64 {
        match kind {
            ObsKind::Hole => self.hole_clearance,
            ObsKind::Edge => self.edge_clearance,
            ObsKind::Keepout => 0.0,
            _ => self.clr(net).max(own_clr).max(self.min_clearance),
        }
    }

    /// Inflated radius of anchor `idx` for a wire of `net` and `width`:
    /// `r_core + clearance + width/2 + margin`.
    pub fn rho_anchor(&self, layer: u32, idx: usize, net: i32, width: f64) -> f64 {
        let a = &self.layer(layer).anchors[idx];
        a.r_core + self.clearance(net, a.kind, a.net, a.own_clr) + 0.5 * width + self.margin
    }

    /// Inflated half-width of wall `idx`, same formula as [`ObstacleSet::rho_anchor`].
    pub fn rho_wall(&self, layer: u32, idx: usize, net: i32, width: f64) -> f64 {
        let w = &self.layer(layer).walls[idx];
        w.r_core + self.clearance(net, w.kind, w.net, w.own_clr) + 0.5 * width + self.margin
    }

    /// Anchors whose core circle (`r_core`) overlaps `bb`; grow `bb` by the largest `rho`
    /// of interest. Sorted, deduplicated.
    pub fn query_anchors(&self, layer: u32, bb: Aabb) -> Vec<usize> {
        self.layer(layer).query_anchors(bb)
    }

    /// Walls whose core (grown by `r_core`) overlaps `bb`. Sorted, deduplicated.
    pub fn query_walls(&self, layer: u32, bb: Aabb) -> Vec<usize> {
        self.layer(layer).query_walls(bb)
    }
}
