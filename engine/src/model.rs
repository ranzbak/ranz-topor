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
    /// Least track width the net may use (nm): the largest `track_width` minimum of the custom
    /// rules matching the net (KiCad `(constraint track_width (min …))`); 0 = none. The engine's
    /// effective minimum is `max(Rules.min_track_width, min_width)` (necked exit stubs, A6/NX).
    pub min_width: i64,
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
            min_width: 0,
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
    /// KiCad's pre-defined via list (`board.design_settings.via_dimensions`, entries with a
    /// diameter > 0), `[diameter, drill]` nm. The smaller ones are alternatives for tight spots
    /// (`vias::via_sizes`).
    pub predefined_vias: Vec<[i64; 2]>,
    /// `[diameter, drill]` nm used instead of KiCad's factory 0.8 / 0.4 net class via when the
    /// board sets no via up (`predefined_vias` empty and the net's class at the factory size).
    /// The single place of truth for the fallback (`vias::via_sizes`); the plugin only fills it.
    pub via_fallback: [i64; 2],
    /// Run-time copy of `Options.allowed_drills_nm` (set by `route`, never serialized).
    #[serde(skip)]
    pub allowed_drills: Vec<i64>,
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
            predefined_vias: Vec::new(),
            via_fallback: [800_000, 400_000],
            allowed_drills: Vec::new(),
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

    /// `max(item net clearance, pad net clearance, pad local clearance, board minimum)`, nm;
    /// an NPTH pad also charges `hole_clearance` ([`BoardModel::pad_own_clearance`]).
    pub fn required_clearance(&self, net: i32, pad: &Pad) -> f64 {
        let r = &self.rules;
        r.net_rule(net)
            .clearance
            .max(r.net_rule(pad.net).clearance)
            .max(pad.local_clearance.unwrap_or(0))
            .max(r.min_clearance)
            .max(self.npth_hole_clearance(pad)) as f64
    }

    /// `hole_clearance` for an NPTH pad, else 0, nm. An NPTH pad is a net-0 copper shape *and*
    /// a drilled hole at the same centre, so the CDT vertex (one per position) can carry only
    /// one of the two anchors; the pad anchor must therefore charge the larger clearance
    /// itself, in every stage (graph capacity, search, sketch / taut, relax, Checker).
    /// O(1).
    pub fn npth_hole_clearance(&self, pad: &Pad) -> i64 {
        if pad.attr == PadAttr::Npth {
            self.rules.hole_clearance
        } else {
            0
        }
    }

    /// Own clearance of a pad's copper as an obstacle: `max(pad local clearance, `net_clr`, NPTH
    /// hole clearance)`, nm. The one helper behind `ObstacleSet::build` and `Checker::new`.
    /// O(1).
    pub fn pad_own_clearance(&self, pad: &Pad, net_clr: f64) -> f64 {
        (pad.local_clearance.unwrap_or(0) as f64)
            .max(net_clr)
            .max(self.npth_hole_clearance(pad) as f64)
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

    /// The other pads of pad `pi`'s footprint that belong to a net-tie group, when that
    /// footprint is a net tie (KiCad `FOOTPRINT::MapPadNumbersToNetTieGroups` ≥ 0), as indices
    /// into `pads`. Empty for every pad of an ordinary footprint. O(T) for an ordinary
    /// footprint, O(T + P) for a net-tie footprint (T net ties, P pads).
    pub fn tie_group_pads_of(&self, pi: usize) -> Vec<usize> {
        let p = &self.pads[pi];
        if !self.net_ties.iter().any(|t| t.ref_ == p.ref_) {
            return Vec::new();
        }
        (0..self.pads.len())
            .filter(|&j| {
                j != pi && self.pads[j].ref_ == p.ref_ && self.pads[j].net_tie_group.is_some()
            })
            .collect()
    }

    /// Nets (other than its own) for which pad `pi` is no obstacle in the routing model: the
    /// nets of tie-group pads of the same net-tie footprint whose copper touches or overlaps
    /// `pi`'s on a shared layer ("fused" tie pads).
    ///
    /// KiCad 10 (`DRC_ENGINE::IsNetTieExclusion`, verified with kicad-cli 10.0.8 in task NT,
    /// `engine/tests/net_tie.rs`) allows a track of net N to collide with a pad of a net-tie
    /// footprint only where the collision lies inside a tie-group pad of net N of that
    /// footprint. That needs the two pads to overlap; a gapped tie pad (dcsgonebadge JP1, 0.3 mm)
    /// is an ordinary obstacle for the other tied net. For fused pads the routing model exempts
    /// the whole pad (a track ending in its own pad cannot keep clearance from the fused
    /// neighbour); the exact rule is enforced by `Checker` (self-check), which rejects a
    /// collision outside the own pad. O(T + P + k · v²) for k tie-group pads of v vertices.
    pub fn pad_fused_tie_nets(&self, pi: usize) -> Vec<i32> {
        let p = &self.pads[pi];
        let mut nets: Vec<i32> = Vec::new();
        for j in self.tie_group_pads_of(pi) {
            let q = &self.pads[j];
            if q.net == 0 || q.net == p.net || nets.contains(&q.net) {
                continue;
            }
            let fused = p.layers.iter().filter(|l| q.layers.contains(l)).any(|&l| {
                crate::geom::skeleton_gap(&self.skeleton(p, l), &self.skeleton(q, l)) <= 0.0
            });
            if fused {
                nets.push(q.net);
            }
        }
        nets.sort_unstable();
        nets
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
    /// Relaxation sweep cap (single source; see [`Options::relax_cap`]).
    pub relax_iter_cap: u32,
    /// Capacity rule (M4-D1, `topor.toml [expert] own_net_capacity`): a pad's own edges charge
    /// no clearance gap for its own net and give the corner radius back, same-net strands
    /// charge no gap between each other. `false` = the net-blind M3 rule (identity
    /// experiments only).
    pub own_net_capacity: bool,
    /// Capacity rule (M4-D1, `topor.toml [expert] wall_aware_capacity`): with an own end,
    /// foreign walls along the edge that are not its ends (outline, keepouts, long pad sides)
    /// remove the blocked part of the edge. `false` = F6 behaviour (outline-wall corridors
    /// accepted that the metrization then rejects).
    pub wall_aware_capacity: bool,
    /// Search (A4, `topor.toml [expert] transit_pinch`): the slot search refuses a face
    /// transit around a vertex that a foreign wall pinches below the strand's need (a via next
    /// to the board edge: the vertex-only edge capacity sees only the outline corner), where
    /// the metrization would report the wall grazed mid-span. `false` = M3.
    pub transit_pinch: bool,
    /// Steiner T-junctions (A5, `topor.toml [expert] steiner_min_angle_deg`): a same-net
    /// T-junction is taken only when the joining track meets the sibling at no less than this
    /// angle (degrees, 0..=90; all three pairwise angles at the junction are then >= it). Below
    /// 60 the branch forms an acute sliver (acid trap); 90 allows perpendicular joins only.
    pub steiner_min_angle_deg: f64,
    /// Steiner T-junctions (A5, `topor.toml [expert] steiner_len_slack`): a junction whose
    /// metrized connection is longer than this factor times the search's estimate (+ 1 mm) is
    /// rejected and searched again without it (the sibling's copper lay elsewhere than the
    /// estimate on a long graph edge).
    pub steiner_len_slack: f64,
    /// Steiner T-junctions (A5, `topor.toml [expert] steiner_ghost_factor`): a junction path is
    /// a goal only while its length plus via cost stays at most this factor times the
    /// connection's own pad-to-pad ghost on the empty board (1 = only junctions that are
    /// cheaper than the free pad route; larger also lets junctions dodge congestion).
    pub steiner_ghost_factor: f64,
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
            own_net_capacity: true,
            wall_aware_capacity: true,
            transit_pinch: true,
            steiner_min_angle_deg: 60.0,
            steiner_len_slack: 1.1,
            steiner_ghost_factor: 1.0,
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
    /// Router (A4, `topor.toml [search] zero_slack_failed`): the zero-slack retry also tries
    /// the connections that failed (not permanently), ahead of the length gains of routed
    /// ones: a connection whose only corridor is pinched below ε gets its margin-0 attempt even
    /// when the negotiation never routed it at ε. Off = M3 (routed connections only).
    pub zero_slack_failed: bool,
    /// Router (A14, `topor.toml [search] dangling_ends`): what a connection that starts at the free
    /// end of a hand-started track does. `"continue"` (default): the new copper leaves the end
    /// along the hand direction (G1, `g1_angle_deg <= 1`); `"fixed"`: M3 behaviour, the end is
    /// a fixed point and the new copper may leave at any angle; `"drop"`: the plugin rips the
    /// dangling stubs of the nets in scope before the run (the engine then sees none and routes
    /// from the pads; for the engine it behaves like `"fixed"`).
    pub dangling_ends: String,
    /// Router (D-1, `topor.toml [search] strict_unfit_edges`): the layered search rejects a
    /// graph edge that is too narrow for the connection's trace even when empty
    /// (`!fits(e)`) instead of pricing it as an overuse crossing. Off by default (the default
    /// keeps boards such as `eprom-to-kernal` routable through one pinched edge).
    pub strict_unfit_edges: bool,
    /// Router (D-6, `topor.toml [search] pour_listed_via_goals`): a connection to a pour may end
    /// on a same-net via the pour already lists (no new via) instead of always placing one.
    /// Off by default: on wiccon-2023 it saves one via but lengthens the GND connection (outside
    /// pads 1381.4 mm, over the 1380 mm M3 bound), so it is the user's trade-off.
    pub pour_listed_via_goals: bool,
    /// Router (M3): cost of one via in nm of wire length.
    pub via_cost_nm: i64,
    /// Router (A10): per-net via cost in nm by net name; overrides `via_cost_nm` (the plugin
    /// resolves `[vias] cost_by_netclass` > `cost_by_priority` > `via_cost_mm` into it).
    pub via_cost_by_net: BTreeMap<String, i64>,
    /// Run-time form of `via_cost_by_net` by net code: filled by `route` from the model's net
    /// names (anything passed in is replaced when `via_cost_by_net` is not empty). Internal.
    pub via_cost_by_code: BTreeMap<i32, i64>,
    /// Router (A10): when non-empty only these drills (nm) are used for the *alternative* via
    /// sizes (predefined and profile vias); the net's own class via is never dropped.
    pub allowed_drills_nm: Vec<i64>,
    /// Router (A10): allow via sites inside the net's own SMD pads (fab must fill and cap).
    pub via_in_pad: bool,
    /// Router (A10): a via is removed by the reduction pass when the via-free route is at most
    /// this many percent longer (outside pads).
    pub reduction_len_slack_pct: f64,
    /// Router (A11): extra cost in nm of cost per mm of track inside a foreign-net copper pour
    /// (the fill of a zone of another net on that layer), charged per step; 0 disables. Default 0
    /// (measured: 1 000 000 = 1 mm per mm makes wiccon 58/61 with 83 vias and eprom 65.9 mm
    /// outside pads instead of 62/62, 37 vias and 64.6 mm, so it is opt-in).
    pub pour_cut_cost_nm: i64,
    /// Router (A11): preferred routing direction per layer name, in degrees (0 = along x,
    /// 90 = along y); see `direction_cost`.
    pub layer_direction: BTreeMap<String, f64>,
    /// Router (A11): a step of length `L` at angle `a` to its layer's preferred direction costs
    /// an extra `L * direction_cost * |sin a|`; 0 disables (layers without an entry are free).
    pub direction_cost: f64,
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
    /// Net name -> priority tier `1..=4` (1 routed first and cheapest per length, 4 last; see
    /// `search::tier_weight` / `via_weight`). Overrides the inferred tier (wide net 2, else 3).
    /// The plugin builds it from `@N` marker > `topor_pN` netclass > `[nets] priority` (1..3) and
    /// power-by-name (2); its numbers are the engine tiers 1:1. Values outside `1..=4` are
    /// ignored with a warning. Empty = inference.
    pub net_priority: BTreeMap<String, u8>,
    /// Do-not-touch nets (names, already expanded): never decomposed, never routed; their copper
    /// stays a fixed obstacle; `NetRouteReport.status == "excluded"`.
    pub exclude_nets: Vec<String>,
    /// Ignored pads (`REF.NUMBER`): removed from the model before routing (no terminal, no
    /// obstacle); each is reported as a `ConnReport` with `status == "ignored"`,
    /// `reason == "pad outside board"`.
    pub ignore_pads: Vec<PadRef>,
    /// Per-net pour override: `true` = treat the net as pour-served (only pour completion edges
    /// to its pour; pour-less / fill-less nets get no connections), `false` = route it as a
    /// signal net even if it has a pour (the pour is not used to join pads). Absent = automatic.
    pub pour_served: BTreeMap<String, bool>,
    /// Offline / report only (M3-R15): also search every connection once on the fixed copper
    /// alone with full clearance and width (a DRC-feasible shortest path) and report it as
    /// `ConnReport::ref_len_nm`. Never changes the routing. Off by default (costs one search per
    /// connection).
    pub detour_reference: bool,
    /// Share (percent, 0..=100) of the remaining wall-clock budget the reference searches may
    /// use; connections not finished in time fall back to the ghost (`ref_kind = "ghost"`).
    pub detour_reference_budget_pct: f64,
    /// Router (A6, `topor.toml [search] exit_stubs`): pad exit stubs (`crate::stubs`). `"auto"`:
    /// a straight stub out of a pad whose centre exit at the net width violates the clearance
    /// (fine-pitch rows; off-centre or necked when needed); `"all"`: also every SMD terminal pad
    /// with a long axis; `"off"`: M3 (attach at the pad centre).
    pub exit_stubs: String,
    /// Router (NX, `topor.toml [search] neck_to_pad`): an exit stub may be narrower than the net
    /// width (down to the effective minimum track width) when the full width does not fit out of
    /// the pad ("necking"); the rest of the connection keeps the net width.
    pub neck_to_pad: bool,
    /// Router (NX, `topor.toml [search] neck_max_mm`): longest necked stub, nm; 0 = twice the
    /// pad's long side.
    pub neck_max_nm: i64,
    /// Router (A6, `topor.toml [search] pad_bridge_max_mm`): a connection between two pads of
    /// one footprint at most this far apart (centre to centre, nm) whose straight line is clean,
    /// possibly only off the centre line, is routed by that one track (`exit_stubs` auto: only
    /// where a pad needs a stub); 0 = off.
    pub pad_bridge_max_nm: i64,
    /// Router (A5, M4-D3, `topor.toml [search] steiner`): a connection may end on the copper of
    /// an already routed connection of its own net (or on kept hand copper of its target
    /// cluster) in a T-junction instead of only at its pad, when that is cheaper; sibling
    /// connections of one net then share a pad exit instead of each needing its own. Off = M3
    /// (pad to pad only).
    pub steiner: bool,
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
            drc_loop_cap: 5,
            seed: 1,
            existing_copper: ExistingCopper::Fixed,
            scope: Scope::default(),
            skip_self_check: false,
            pinned: BTreeMap::new(),
            drc_spots: Vec::new(),
            zero_slack_retry: true,
            zero_slack_failed: true,
            dangling_ends: "continue".into(),
            strict_unfit_edges: false,
            pour_listed_via_goals: false,
            via_cost_nm: 2_000_000,
            via_cost_by_net: BTreeMap::new(),
            via_cost_by_code: BTreeMap::new(),
            allowed_drills_nm: Vec::new(),
            via_in_pad: false,
            reduction_len_slack_pct: 10.0,
            pour_cut_cost_nm: 0,
            layer_direction: BTreeMap::new(),
            direction_cost: 0.0,
            max_vias_per_connection: 4,
            via_reduction: true,
            via_relaxation: true,
            profile: false,
            net_priority: BTreeMap::new(),
            exclude_nets: Vec::new(),
            ignore_pads: Vec::new(),
            pour_served: BTreeMap::new(),
            detour_reference: false,
            detour_reference_budget_pct: 10.0,
            exit_stubs: "auto".into(),
            neck_to_pad: true,
            neck_max_nm: 0,
            pad_bridge_max_nm: 2_000_000,
            steiner: true,
        }
    }
}

impl Options {
    pub fn from_json(s: &str) -> Result<Self, String> {
        if s.trim().is_empty() {
            return Ok(Options::default());
        }
        let mut v: serde_json::Value =
            serde_json::from_str(s).map_err(|e| format!("options json: {e}"))?;
        // Legacy input key: a top-level `relax_iter_cap` (before 0.5.0) is the single source
        // `expert.relax_iter_cap`; an explicit `expert.relax_iter_cap` wins.
        if let Some(obj) = v.as_object_mut() {
            if let Some(cap) = obj.remove("relax_iter_cap") {
                let ex = obj
                    .entry("expert")
                    .or_insert_with(|| serde_json::Value::Object(Default::default()));
                if let Some(ex) = ex.as_object_mut() {
                    ex.entry("relax_iter_cap").or_insert(cap);
                }
            }
        }
        serde_json::from_value(v).map_err(|e| format!("options json: {e}"))
    }

    /// Effective relaxation sweep cap: `expert.relax_iter_cap` (the one source of truth; the
    /// pre-0.5.0 top-level JSON key is folded into it by [`Options::from_json`]). O(1).
    pub fn relax_cap(&self) -> u32 {
        self.expert.relax_iter_cap
    }

    /// Via cost in nm of a via on net `code`: the per-net override, else `via_cost_nm`. O(log n).
    pub fn via_cost_for(&self, code: i32) -> i64 {
        self.via_cost_by_code
            .get(&code)
            .copied()
            .unwrap_or(self.via_cost_nm)
    }

    /// Length factor of the via-reduction acceptance: `1 + reduction_len_slack_pct / 100`.
    pub fn reduction_len_slack(&self) -> f64 {
        1.0 + self.reduction_len_slack_pct.max(0.0) / 100.0
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
