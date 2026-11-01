//! Board model, options and rules (serde wire types).
//!
//! The wire format is the KiCad frame (Y down, integer nm). [`BoardModel::from_json`] negates
//! every `y` so that the model held in memory uses the math frame (Y up) of `geom`. Rotations
//! stay unchanged: a positive KiCad rotation is counter-clockwise on screen, which is
//! counter-clockwise in the flipped (Y up) frame as well. [`Options`] are not flipped.

use crate::geom::quant::{IVec2, TrackPrim};
use crate::geom::{Skeleton, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Integer-nm point, serialised as `[x, y]`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(from = "[i64; 2]", into = "[i64; 2]")]
pub struct Pt {
    pub x: i64,
    pub y: i64,
}

impl From<[i64; 2]> for Pt {
    fn from(a: [i64; 2]) -> Self {
        Pt { x: a[0], y: a[1] }
    }
}

impl From<Pt> for [i64; 2] {
    fn from(p: Pt) -> Self {
        [p.x, p.y]
    }
}

impl Pt {
    pub fn to_vec2(self) -> Vec2 {
        Vec2 {
            x: self.x as f64,
            y: self.y as f64,
        }
    }
    pub fn flip(&mut self) {
        self.y = self.y.saturating_neg();
    }
}

/// Mirror a track primitive about the X axis (KiCad frame <-> math frame).
pub fn flip_track(t: TrackPrim) -> TrackPrim {
    let f = |p: IVec2| IVec2 {
        x: p.x,
        y: p.y.saturating_neg(),
    };
    match t {
        TrackPrim::Seg { a, b } => TrackPrim::Seg { a: f(a), b: f(b) },
        TrackPrim::Arc { start, mid, end } => TrackPrim::Arc {
            start: f(start),
            mid: f(mid),
            end: f(end),
        },
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default = "yes")]
    pub routable: bool,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Net {
    pub code: i32,
    #[serde(default)]
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PadAttr {
    Pth,
    #[default]
    Smd,
    Npth,
    Conn,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PadShape {
    Circle {
        d: i64,
    },
    Rect {
        w: i64,
        h: i64,
    },
    RoundRect {
        w: i64,
        h: i64,
        rc: i64,
    },
    Oval {
        w: i64,
        h: i64,
    },
    /// Absolute convex hull (trapezoid / chamfered / custom / offset shapes).
    Poly {
        pts: Vec<Pt>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Drill {
    pub w: i64,
    pub h: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pad {
    #[serde(default)]
    pub uuid: String,
    #[serde(rename = "ref", default)]
    pub ref_: String,
    #[serde(default)]
    pub number: String,
    #[serde(default)]
    pub net: i32,
    #[serde(default)]
    pub attr: PadAttr,
    pub center: Pt,
    #[serde(default)]
    pub rotation_deg: f64,
    #[serde(default)]
    pub layers: Vec<u32>,
    #[serde(default)]
    pub shapes: HashMap<u32, PadShape>,
    #[serde(default)]
    pub drill: Option<Drill>,
    #[serde(default)]
    pub local_clearance: Option<i64>,
    #[serde(default)]
    pub net_tie_group: Option<usize>,
    /// Zone connection override (KiCad `ZONE_CONNECTION`: 0 none, 1 thermal, 2 full,
    /// 3 THT thermal); `None` = the zone's setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone_connection: Option<i32>,
    /// Thermal relief gap override, nm; `None` = the zone's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thermal_gap: Option<i64>,
    /// Thermal spoke angle in degrees (KiCad: 90 = along the pad axes); `None` = 90.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spoke_angle_deg: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Via {
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub net: i32,
    pub at: Pt,
    pub d: i64,
    #[serde(default)]
    pub drill: i64,
    #[serde(default)]
    pub layers: Vec<u32>,
    #[serde(default)]
    pub free: bool,
    #[serde(default)]
    pub locked: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeptItem {
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub net: i32,
    pub layer: u32,
    pub width: i64,
    pub prim: TrackPrim,
    #[serde(default)]
    pub locked: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CopperShape {
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub net: i32,
    pub layer: u32,
    pub poly: Vec<Pt>,
    #[serde(default)]
    pub owner_ref: Option<String>,
}

/// Copper zone (not a rule area) with the pads (`"REF.NUM"`) and via uuids it connects to
/// after a refill; sorted by the harness.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pour {
    pub uuid: String,
    pub net: i32,
    pub layers: Vec<u32>,
    pub pads: Vec<String>,
    pub vias: Vec<String>,
    /// Filled-copper outlines per zone layer (KiCad frame on the wire; `from_json` flips Y).
    pub fills: Vec<PourFill>,
    /// Zone clearance setting, nm (0: the net rule's clearance applies).
    pub clearance: i64,
    /// Zone minimum fill thickness, nm (KiCad prunes fill features thinner than this).
    pub min_thickness: i64,
    /// Thermal relief gap, nm.
    pub thermal_gap: i64,
    /// Thermal spoke width, nm.
    pub spoke_width: i64,
    /// Pad connection (KiCad `ZONE_CONNECTION`: 0 none, 1 thermal, 2 full, 3 THT thermal).
    #[serde(default = "default_pad_connection")]
    pub pad_connection: i32,
}

fn default_pad_connection() -> i32 {
    1
}

/// One outline of a zone's filled copper on `layer`: outer ring plus its holes (the holes are
/// empty in models exported before the pour-reach check; the router then over-approximates
/// the fill).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PourFill {
    pub layer: u32,
    pub outer: Vec<Pt>,
    pub holes: Vec<Vec<Pt>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hole {
    pub at: Pt,
    pub d: i64,
    #[serde(default)]
    pub kind: String,
    /// Slotted hole `[w, h]` (emitted by the harness; checked as a circle of diameter `d`).
    #[serde(default)]
    pub slot: Option<[i64; 2]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keepout {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub layers: Vec<u32>,
    #[serde(default)]
    pub poly: Vec<Pt>,
    #[serde(default)]
    pub no_tracks: bool,
    #[serde(default)]
    pub no_vias: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetTie {
    #[serde(rename = "ref", default)]
    pub ref_: String,
    #[serde(default)]
    pub groups: Vec<Vec<String>>,
    #[serde(default)]
    pub nets: Vec<Vec<i32>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DanglingEnd {
    #[serde(default)]
    pub item: String,
    #[serde(default)]
    pub net: i32,
    pub layer: u32,
    #[serde(default)]
    pub width: i64,
    pub end: Pt,
    /// Unit vector of travel at the free end (KiCad frame in JSON; `dy` negated in memory).
    pub direction: [f64; 2],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetRule {
    pub clearance: i64,
    pub width: i64,
    pub via_d: i64,
    pub via_drill: i64,
    pub netclass: String,
    pub width_source: String,
}

impl Default for NetRule {
    fn default() -> Self {
        NetRule {
            clearance: 200_000,
            width: 200_000,
            via_d: 600_000,
            via_drill: 300_000,
            netclass: "Default".into(),
            width_source: "default".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    pub min_clearance: i64,
    pub min_track_width: i64,
    pub min_via_diameter: i64,
    pub min_via_drill: i64,
    pub min_annular: i64,
    pub hole_clearance: i64,
    pub hole_to_hole: i64,
    pub edge_clearance: i64,
    pub max_error: i64,
    pub profile: String,
    pub drill_list: Vec<i64>,
    pub routing_vias: Vec<i64>,
    pub default: NetRule,
    pub per_net: HashMap<i32, NetRule>,
    pub warnings: Vec<String>,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            min_clearance: 0,
            min_track_width: 200_000,
            min_via_diameter: 400_000,
            min_via_drill: 300_000,
            min_annular: 50_000,
            hole_clearance: 250_000,
            hole_to_hole: 250_000,
            edge_clearance: 500_000,
            max_error: 5_000,
            profile: "jlcpcb_1oz".into(),
            drill_list: vec![
                200_000, 250_000, 300_000, 400_000, 500_000, 800_000, 1_000_000,
            ],
            routing_vias: vec![250_000, 300_000, 400_000, 500_000],
            default: NetRule::default(),
            per_net: HashMap::new(),
            warnings: Vec::new(),
        }
    }
}

impl Rules {
    pub fn net_rule(&self, net: i32) -> &NetRule {
        self.per_net.get(&net).unwrap_or(&self.default)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoardModel {
    pub units: String,
    pub board: String,
    pub layers: Vec<Layer>,
    pub nets: Vec<Net>,
    pub pads: Vec<Pad>,
    pub vias: Vec<Via>,
    pub kept: Vec<KeptItem>,
    pub copper_shapes: Vec<CopperShape>,
    pub holes: Vec<Hole>,
    pub outline: Vec<TrackPrim>,
    pub keepouts: Vec<Keepout>,
    pub net_ties: Vec<NetTie>,
    pub dangling_ends: Vec<DanglingEnd>,
    pub pours: Vec<Pour>,
    pub rules: Rules,
}

impl BoardModel {
    /// Parse the wire JSON (KiCad frame) and convert to the math frame (`y` negated).
    pub fn from_json(s: &str) -> Result<Self, String> {
        let mut m: BoardModel = serde_json::from_str(s).map_err(|e| format!("model json: {e}"))?;
        m.flip_y();
        Ok(m)
    }

    fn flip_y(&mut self) {
        for p in &mut self.pads {
            p.center.flip();
            for sh in p.shapes.values_mut() {
                if let PadShape::Poly { pts } = sh {
                    pts.iter_mut().for_each(Pt::flip);
                }
            }
        }
        for v in &mut self.vias {
            v.at.flip();
        }
        for k in &mut self.kept {
            k.prim = flip_track(k.prim);
        }
        for c in &mut self.copper_shapes {
            c.poly.iter_mut().for_each(Pt::flip);
        }
        for p in &mut self.pours {
            for f in &mut p.fills {
                f.outer.iter_mut().for_each(Pt::flip);
                for h in &mut f.holes {
                    h.iter_mut().for_each(Pt::flip);
                }
            }
        }
        for h in &mut self.holes {
            h.at.flip();
        }
        for o in &mut self.outline {
            *o = flip_track(*o);
        }
        for k in &mut self.keepouts {
            k.poly.iter_mut().for_each(Pt::flip);
        }
        for d in &mut self.dangling_ends {
            d.end.flip();
            d.direction[1] = -d.direction[1];
        }
    }

    /// Pad skeleton for `layer` (math frame); falls back to any layer's shape.
    pub fn skeleton(&self, pad: &Pad, layer: u32) -> Skeleton {
        let c = pad.center.to_vec2();
        let rot = pad.rotation_deg.to_radians();
        let shape = pad.shapes.get(&layer).or_else(|| {
            pad.layers
                .iter()
                .find_map(|l| pad.shapes.get(l))
                .or_else(|| pad.shapes.values().next())
        });
        let Some(shape) = shape else {
            return Skeleton::Point { p: c, r: 0.0 };
        };
        match shape {
            PadShape::Circle { d } => Skeleton::circle(c, *d as f64),
            PadShape::Rect { w, h } => Skeleton::rect(c, *w as f64, *h as f64, rot),
            PadShape::RoundRect { w, h, rc } => {
                Skeleton::round_rect(c, *w as f64, *h as f64, *rc as f64, rot)
            }
            PadShape::Oval { w, h } => Skeleton::oval(c, *w as f64, *h as f64, rot),
            PadShape::Poly { pts } => poly_skeleton(pts, c),
        }
    }

    /// `max(item net clearance, pad net clearance, pad local clearance, board minimum)`, nm.
    pub fn required_clearance(&self, net: i32, pad: &Pad) -> f64 {
        let r = &self.rules;
        r.net_rule(net)
            .clearance
            .max(r.net_rule(pad.net).clearance)
            .max(pad.local_clearance.unwrap_or(0))
            .max(r.min_clearance) as f64
    }
}

/// Layers a via occupies: both outer layers listed → every board layer; else the listed ones.
pub fn via_layers(listed: &[u32], all: &[u32]) -> Vec<u32> {
    if listed.contains(&0) && listed.contains(&2) {
        all.to_vec()
    } else {
        listed.to_vec()
    }
}

impl BoardModel {
    /// Every layer id the model mentions, sorted.
    pub fn all_layer_ids(&self) -> Vec<u32> {
        let mut set: std::collections::BTreeSet<u32> = self.layers.iter().map(|l| l.id).collect();
        set.extend(self.pads.iter().flat_map(|p| p.layers.iter().copied()));
        set.extend(self.kept.iter().map(|k| k.layer));
        set.extend(self.vias.iter().flat_map(|v| v.layers.iter().copied()));
        set.extend(self.copper_shapes.iter().map(|c| c.layer));
        set.extend(self.keepouts.iter().flat_map(|k| k.layers.iter().copied()));
        set.into_iter().collect()
    }

    /// Nets bridged by the net tie footprint `ref_`.
    pub fn tie_nets_of(&self, ref_: &str) -> Vec<i32> {
        self.net_ties
            .iter()
            .filter(|t| t.ref_ == ref_)
            .flat_map(|t| t.nets.iter().flatten().copied())
            .collect()
    }

    /// Nets of the net-tie group the pad belongs to (empty if none).
    pub fn pad_group_nets(&self, p: &Pad) -> Vec<i32> {
        let (Some(g), Some(t)) = (
            p.net_tie_group,
            self.net_ties.iter().find(|t| t.ref_ == p.ref_),
        ) else {
            return Vec::new();
        };
        t.nets.get(g).cloned().unwrap_or_default()
    }

    /// Drilled pad index by hole position (a PTH hole belongs to its pad).
    pub fn pad_by_hole(&self) -> HashMap<Pt, usize> {
        let mut m = HashMap::new();
        for (i, p) in self.pads.iter().enumerate() {
            if p.drill.is_some() {
                m.insert(p.center, i);
            }
        }
        m
    }
}

fn poly_skeleton(pts: &[Pt], c: Vec2) -> Skeleton {
    let mut v: Vec<Vec2> = pts.iter().map(|p| p.to_vec2()).collect();
    match v.len() {
        0 => Skeleton::Point { p: c, r: 0.0 },
        1 => Skeleton::Point { p: v[0], r: 0.0 },
        2 => Skeleton::Seg {
            s: crate::geom::Seg { a: v[0], b: v[1] },
            r: 0.0,
        },
        n => {
            // The Y flip reverses the winding; `Skeleton::Poly` needs counter-clockwise.
            let area2: f64 = (0..n).map(|i| v[i].cross(v[(i + 1) % n])).sum();
            if area2 < 0.0 {
                v.reverse();
            }
            Skeleton::Poly { pts: v, r: 0.0 }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExistingCopper {
    #[default]
    Fixed,
    Smooth,
    Rip,
}

/// `"U1.3"` pad reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PadRef {
    pub ref_: String,
    pub number: String,
}

impl TryFrom<String> for PadRef {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        match s.rsplit_once('.') {
            Some((r, n)) if !r.is_empty() && !n.is_empty() => Ok(PadRef {
                ref_: r.to_string(),
                number: n.to_string(),
            }),
            _ => Err(format!("bad pad reference {s:?} (expected REF.NUMBER)")),
        }
    }
}

impl From<PadRef> for String {
    fn from(p: PadRef) -> String {
        format!("{}.{}", p.ref_, p.number)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scope {
    pub nets: Vec<String>,
    pub netclasses: Vec<String>,
    pub pads: Vec<PadRef>,
    /// Via uuids (model vias) selected as terminals.
    pub vias: Vec<String>,
    pub footprints: Vec<String>,
    /// Polygon in the KiCad frame; `None` = no area limit.
    pub area: Option<Vec<Pt>>,
    pub unrouted_only: bool,
}

impl Scope {
    /// No selector is set (`unrouted_only` is a filter, not a selector).
    pub fn is_whole_board(&self) -> bool {
        self.nets.is_empty()
            && self.netclasses.is_empty()
            && self.pads.is_empty()
            && self.vias.is_empty()
            && self.footprints.is_empty()
            && self.area.is_none()
    }

    /// Whether the pad is selected by `pads` / `footprints`. Net, netclass and area
    /// selectors need data a `Pad` does not carry and are resolved by the caller.
    pub fn selects_pad(&self, pad: &Pad) -> bool {
        if self.is_whole_board() {
            return true;
        }
        self.footprints.contains(&pad.ref_)
            || self
                .pads
                .iter()
                .any(|p| p.ref_ == pad.ref_ && p.number == pad.number)
    }
}

/// A circular obstacle that only `net` must avoid (harness DRC loop: KiCad flagged copper of
/// `net` at `at` on `layer`; the router keeps `r` nm from `at`). `at` is in the KiCad frame.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DrcSpot {
    pub layer: u32,
    pub at: Pt,
    pub r: i64,
    pub net: i32,
}

/// Expert knobs of the negotiation (`topor.toml [expert]`); defaults are the M3 constants.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Expert {
    /// History added to the edges of a connection that failed relax / self-check.
    pub hist_fail: f64,
    /// History added to the edges / sites that block a connection.
    pub hist_block: f64,
    /// History added to every overused edge per iteration.
    pub hist_overuse: f64,
    /// Present-congestion factor of the search cost.
    pub present_overuse: f64,
    /// Candidates the zero-slack retry tries.
    pub zero_slack_candidates: usize,
    /// Relaxation sweep cap (see [`Options::relax_cap`]).
    pub relax_iter_cap: u32,
}

impl Default for Expert {
    fn default() -> Self {
        Expert {
            hist_fail: 1.0,
            hist_block: 0.5,
            hist_overuse: 0.5,
            present_overuse: 4.0,
            zero_slack_candidates: 4,
            relax_iter_cap: 200,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub margin_nm: i64,
    pub threads: usize,
    pub partial_routes: bool,
    /// Wall-clock budget of the run in seconds; `0.0` disables the timeout (the stop rule and
    /// the iteration cap still end the run).
    pub wall_clock_s: f64,
    /// Most PathFinder iterations of one pass (`topor.toml [routing] pathfinder_iter_cap`).
    pub pathfinder_iter_cap: u32,
    /// Stop rule: the pass ends when over the last `patience` iterations no connection was
    /// newly routed and the best cost improved by less than `min_gain_pct` in total.
    pub patience: u32,
    /// Stop rule: total best-cost gain (percent) over `patience` iterations below which the
    /// run counts as converged-by-no-gain.
    pub min_gain_pct: f64,
    /// An accepted path longer than this factor times its slot-free ghost counts as an absurd
    /// detour and triggers rip-up of the blockers.
    pub detour_factor: f64,
    /// Rip-up cost growth per repeated rip of one `(ripper, blocker)` pair (consumed by A16).
    pub ripup_growth: f64,
    /// Rips per `(ripper, blocker)` pair and run (damping cap).
    pub ripup_max_per_pair: u32,
    /// Expert knobs (`topor.toml [expert]`).
    pub expert: Expert,
    /// Relaxation sweep cap; the effective value is [`Options::relax_cap`].
    pub relax_iter_cap: u32,
    pub drc_loop_cap: u32,
    pub seed: u64,
    pub existing_copper: ExistingCopper,
    pub scope: Scope,
    pub skip_self_check: bool,
    /// Smoother: kept uuid -> reason. A wire containing a pinned item keeps its hand copper
    /// (fallback "pinned: <reason>"); the harness DRC loop pins items KiCad flagged.
    pub pinned: BTreeMap<String, String>,
    /// Router: per-net circular obstacles from the harness DRC loop.
    pub drc_spots: Vec<DrcSpot>,
    /// Router (R6): after negotiation, one retry pass in which a connection may take a corridor
    /// that is pinched below the margin ε (but not below the rules) with margin 0 against that
    /// corridor only; kept when it shortens the routing. The harness DRC is the final judge.
    pub zero_slack_retry: bool,
    /// Router (M3): cost of one via in nm of wire length.
    pub via_cost_nm: i64,
    /// Router (M3): maximum vias on one connection.
    pub max_vias_per_connection: u32,
    /// Router (M3-R2): after convergence, one via-reduction pass (TopoR lifting).
    pub via_reduction: bool,
    /// Router (A13): after via reduction, move each via a bounded distance inside the star of
    /// its site towards the straight line between its two legs' last anchors.
    pub via_relaxation: bool,
    /// Diagnostics: emit `RouteResult.profile` (per-iteration timers). Off by default: timings
    /// are run-dependent and must not enter the (thread-count independent) result JSON.
    pub profile: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            margin_nm: 5_000,
            threads: 0,
            partial_routes: false,
            wall_clock_s: 60.0,
            pathfinder_iter_cap: 30,
            patience: 5,
            min_gain_pct: 1.0,
            detour_factor: 2.0,
            ripup_growth: 2.0,
            ripup_max_per_pair: 8,
            expert: Expert::default(),
            relax_iter_cap: 200,
            drc_loop_cap: 5,
            seed: 1,
            existing_copper: ExistingCopper::Fixed,
            scope: Scope::default(),
            skip_self_check: false,
            pinned: BTreeMap::new(),
            drc_spots: Vec::new(),
            zero_slack_retry: true,
            via_cost_nm: 2_000_000,
            max_vias_per_connection: 4,
            via_reduction: true,
            via_relaxation: true,
            profile: false,
        }
    }
}

impl Options {
    pub fn from_json(s: &str) -> Result<Self, String> {
        if s.trim().is_empty() {
            return Ok(Options::default());
        }
        serde_json::from_str(s).map_err(|e| format!("options json: {e}"))
    }

    /// Effective relaxation sweep cap: the top-level `relax_iter_cap` when it differs from the
    /// default 200 (older option JSON, tests), else `expert.relax_iter_cap`. O(1).
    pub fn relax_cap(&self) -> u32 {
        if self.relax_iter_cap != 200 {
            self.relax_iter_cap
        } else {
            self.expert.relax_iter_cap
        }
    }

    /// `threads`, or `available_parallelism() - 1` (min 1) when 0.
    pub fn effective_threads(&self) -> usize {
        if self.threads > 0 {
            return self.threads;
        }
        std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(1))
            .unwrap_or(1)
            .max(1)
    }
}

/// Engine output / writer input: a track item or a via.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Item {
    Via(ViaItem),
    Track(TrackItem),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackItem {
    pub net: i32,
    pub layer: u32,
    pub width: i64,
    pub prim: TrackPrim,
    #[serde(default)]
    pub group: Option<String>,
    /// Smoother output: id of the wire this item belongs to (`WireReport.id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViaItem {
    /// Always `"via"`.
    pub kind: String,
    pub net: i32,
    pub at: Pt,
    pub d: i64,
    #[serde(default)]
    pub drill: i64,
    #[serde(default)]
    pub layers: Vec<u32>,
    #[serde(default)]
    pub group: Option<String>,
    /// Router output: id of the wire this via belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire: Option<usize>,
}

impl Item {
    /// Parse a JSON array of items (KiCad frame, as on the wire).
    pub fn list_from_json(s: &str) -> Result<Vec<Item>, String> {
        if s.trim().is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_str(s).map_err(|e| format!("items json: {e}"))
    }
}
