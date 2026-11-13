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
    /// KiCad island removal mode: 0 always (an island that holds no pad, via or track of the
    /// net is dropped from the fill), 1 never (every island stays), 2 below `min_island_area`.
    pub island_removal: i32,
    /// Smallest kept island with `island_removal` 2, nm².
    pub min_island_area: f64,
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
    /// The rule area forbids copper pours (zone fills) on its layers (D6a: the pour estimate
    /// leaves it out).
    #[serde(default)]
    pub no_pour: bool,
    /// Carried from KiCad, not used by the router (pads / footprints are never placed).
    #[serde(default)]
    pub no_pads: bool,
    #[serde(default)]
    pub no_footprints: bool,
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
    /// Clearance from the drill of a via the router places to foreign copper (nm): KiCad's hole
    /// clearance floored by the fab's via-hole-to-track value (`hole_clearance` carries the PTH
    /// floor); 0 (unset, older models) = `hole_clearance`. See [`Rules::via_hole_clear`].
    #[serde(default)]
    pub via_hole_clearance: i64,
    pub hole_to_hole: i64,
    pub edge_clearance: i64,
    /// Narrowest copper connection KiCad's DRC accepts (`min_connection`, nm; 0 = unset): a
    /// necked pad exit stays at least this wide.
    #[serde(default)]
    pub min_connection: i64,
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
    /// Custom `.kicad_dru` clearance rules tied to a footprint courtyard
    /// (`A.intersectsCourtyard('U6')`): inside the polygon the rule's clearance replaces the net
    /// classes' (KiCad: the matching custom rule wins), e.g. a BGA neck-down rule.
    pub clearance_regions: Vec<ClearanceRegion>,
    /// KiCad checks whose severity in Board Setup -> Violation Severity is `ignore`
    /// (`rule_severities` of the `.kicad_pro`), by KiCad name (e.g. `copper_edge_clearance`).
    /// Such a check is not a hard rule: the topological core prices a violation (`ignored_rule`
    /// factor) and reports it as a warning.
    #[serde(default)]
    pub ignored_checks: Vec<String>,
    /// D7: the board's differential pairs with their resolved dimensions (the plugin's
    /// `toporlib.diffpair`, KiCad's `MatchDpSuffix` on the full net name; one definition). Empty:
    /// no pairs (an older model, or `[diffpair] detect = "off"`).
    #[serde(default)]
    pub diff_pairs: Vec<DiffPairRule>,
}

/// One differential pair (D7) as the plugin resolved it (nm; 0 = not set). KiCad sources:
/// width = netclass `diff_pair_width`, gap = netclass `diff_pair_gap` or a custom rule
/// `diff_pair_gap (opt)`, both floored by the board minimums; via gap =
/// `EffectiveDiffPairViaGap`; `gap_min` / `gap_max`, `max_uncoupled`, `max_skew` only from custom
/// rules (KiCad raises no pair violation without them).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DiffPairRule {
    /// Net code of the positive (`+` / `P`) and negative half.
    pub p: i32,
    pub n: i32,
    pub p_name: String,
    pub n_name: String,
    pub netclass: String,
    /// Track width of each half and the edge-to-edge gap between them.
    pub width: i64,
    pub gap: i64,
    /// Edge-to-edge gap of a via pair; the via size of the pair (the P net's class via).
    pub via_gap: i64,
    pub via_d: i64,
    pub via_drill: i64,
    pub gap_min: i64,
    pub gap_max: i64,
    /// "Coupled" band around the gap (KiCad `RANGED_NUM(gap, 10000, 10000)`).
    pub tolerance: i64,
    pub max_uncoupled: i64,
    pub max_skew: i64,
    /// Names of the custom rules behind `gap_min`/`gap_max`, `max_uncoupled`, `max_skew`
    /// (KiCad's violation texts quote them).
    pub rule_gap: String,
    pub rule_uncoupled: String,
    pub rule_skew: String,
}

/// A position-bound custom clearance rule (`.kicad_dru` `A.intersectsCourtyard('U6')`,
/// `A.intersectsArea('x')`, `A.enclosedByArea('x')`; `toporlib.custom_rules`): for a pair of
/// copper items of which either one touches the polygon (`enclosed`: lies wholly inside it) the
/// rule's clearance replaces the nets' (KiCad: the matching custom rule wins, also when it is
/// smaller). A later region of the list wins over an earlier one (KiCad's last matching rule).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ClearanceRegion {
    #[serde(default)]
    pub name: String,
    /// Footprint reference whose courtyard it is.
    #[serde(default, rename = "ref")]
    pub ref_: String,
    /// Courtyard outline (a rectangle for most footprints).
    #[serde(default)]
    pub poly: Vec<Pt>,
    /// Clearance (nm) that applies to items touching the polygon.
    #[serde(default)]
    pub clearance: i64,
    /// Only items wholly inside the polygon match (`enclosedByArea`).
    #[serde(default)]
    pub enclosed: bool,
    /// Copper layer ids the rule applies on (`(layer ...)`); empty = every layer.
    #[serde(default)]
    pub layers: Vec<u32>,
}

impl Rules {
    /// Clearance from a placed via's drill to foreign copper (nm): `via_hole_clearance`, or
    /// `hole_clearance` when unset. O(1).
    pub fn via_hole_clear(&self) -> i64 {
        if self.via_hole_clearance > 0 {
            self.via_hole_clearance
        } else {
            self.hole_clearance
        }
    }
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
            via_hole_clearance: 0,
            hole_to_hole: 250_000,
            edge_clearance: 500_000,
            min_connection: 0,
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
            clearance_regions: Vec::new(),
            ignored_checks: Vec::new(),
            diff_pairs: Vec::new(),
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
        for r in &mut self.rules.clearance_regions {
            r.poly.iter_mut().for_each(Pt::flip);
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// Extra distance on top of every clearance during search and drawing (nm); 0 (default) =
    /// exactly the design rule, which KiCad's DRC accepts.
    pub margin_nm: i64,
    /// Wall-clock budget of the run in seconds; `0.0` disables the timeout (the stop rule and
    /// the iteration cap still end the run).
    pub wall_clock_s: f64,
    /// An accepted path longer than this factor times its slot-free ghost counts as an absurd
    /// detour and triggers rip-up of the blockers.
    pub detour_factor: f64,
    /// Social factors (`topor.toml [factors]`): weight, scope and report level per factor name,
    /// used by the topological core (`topo::factors`).
    pub factors: crate::topo::factors::FactorConfig,
    pub drc_loop_cap: u32,
    pub seed: u64,
    pub existing_copper: ExistingCopper,
    pub scope: Scope,
    pub skip_self_check: bool,
    /// Router: per-net circular obstacles from the harness DRC loop.
    pub drc_spots: Vec<DrcSpot>,
    /// Router (A14, `topor.toml [search] dangling_ends`): what a connection that starts at the free
    /// end of a hand-started track does. `"continue"` (default): the new copper leaves the end
    /// along the hand direction (G1, `g1_angle_deg <= 1`); `"fixed"`: M3 behaviour, the end is
    /// a fixed point and the new copper may leave at any angle; `"drop"`: the plugin rips the
    /// dangling stubs of the nets in scope before the run (the engine then sees none and routes
    /// from the pads; for the engine it behaves like `"fixed"`).
    pub dangling_ends: String,
    /// Topological core (`topor.toml [planes] signals`): `"costly"` (default) lets signal copper
    /// use the plane layers at the `plane_signal` / `plane_cut` prices; `"forbidden"` keeps
    /// signals off them (only the routable layers are signal layers).
    pub plane_signals: String,
    /// Router (A10): when non-empty only these drills (nm) are used for the *alternative* via
    /// sizes (predefined and profile vias); the net's own class via is never dropped.
    pub allowed_drills_nm: Vec<i64>,
    /// Router (A10): allow via sites inside the net's own SMD pads (fab must fill and cap).
    pub via_in_pad: bool,
    /// Router (0.4.2): a terminal whose pour has fills only on plane (non-routed) layers is
    /// joined by a through via onto the plane at a via site within `plane_via_max_nm` of its
    /// copper (with `via_in_pad` the pad's own sites are the nearest). False: such a
    /// connection fails with "terminal not on any routable layer".
    pub plane_vias: bool,
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
    /// Router (NX, `topor.toml [search] neck_to_pad`): an exit stub may be narrower than the net
    /// width (down to the effective minimum track width) when the full width does not fit out of
    /// the pad ("necking"); the rest of the connection keeps the net width.
    pub neck_to_pad: bool,
    /// Topological core (S2c, `topor.toml [search] drawing_feedback`): a wire the drawing cannot
    /// fit is drawn before the wires that block its sleeve, which are redrawn around it (at most
    /// one round per wire) instead of being reported failed at once; with `opt_loop = "cycles"`
    /// the rooms the drawing found too narrow also feed into the next cycle's plan. Default false
    /// (M0: the TopoR method computes the copper once from the topology, Polubasov 2010 intro;
    /// the plugin's `"auto"` is on with `opt_loop = "cycles"`, off with `"topor"`).
    pub drawing_feedback: bool,
    /// Topological core (S2c, `topor.toml [vias] offset_sites`): besides the sample points of the
    /// via interval's triangles, also try the via's offset corners of nearby pads and copper.
    pub via_offset_sites: bool,
    /// Topological core (S2c, `topor.toml [search] feedback_max_rip`): work cap, most drawn
    /// wires one feedback round may rip up and redraw. Whether a rejected wire's blockers are
    /// ripped at all is a graded price (M-B2: their reroute price against the violation the
    /// rejected wire would otherwise be drawn with), not this count.
    pub feedback_max_rip: usize,
    /// M-B2 (`[optimize] history_fade`): what a cycle carries over to the next one (the edge
    /// history of over-full passages, the history on the violating stretches, soft cuts) is
    /// multiplied by this before the cycle's own findings are added (Dr. CU §IV-C: history fades).
    /// 1 = never fades (before M-B2), 0 = no memory between cycles.
    pub history_fade: f64,
    /// M-B2 (`[optimize] soft_cuts`): the lowered rooms the drawing asked for (cuts, Haas 2026
    /// §VI-E) turn into a soft price on their passage when a cycle comes out worse than the best
    /// so far; false = they stay hard for the whole run (before M-B2).
    pub soft_cuts: bool,
    /// M-B2 (`[optimize] crossing_rules`): the planner looks for the crossing patterns that force
    /// vias and over-full passages (a trace wrapped around another net's terminal = ordinary
    /// clinch / half-loop, a crossing that cannot be layered, a pair crossing twice) and re-plans
    /// the two traces locally in the other order (Luzin & Popov 2012; Polubasov 2010), kept when
    /// the plan gets better.
    pub crossing_rules: bool,
    /// Topological core (S3, `topor.toml [search] reroute_rounds`): most rounds of negotiated
    /// rip-up-and-reroute after the first layer assignment (0 = off).
    pub reroute_rounds: usize,
    /// Topological core (S4, `topor.toml [vias] reduce_after_drawing`): after drawing, try every
    /// via-carrying wire with a layer run moved onto the layer of its neighbour run (dropping the
    /// vias between), keep it where the checker accepts the redrawn wire.
    pub via_reduce: bool,
    /// Topological core (D4, `topor.toml [planes] stitch_islands`): after routing, a piece of a
    /// pour net's copper that the routed copper cut off from the rest of its pour (an island of
    /// the estimated refill holding a pad or via of the net, or a pad whose thermal spokes are all
    /// cut) gets a via into the pour on another layer, or a short stub, near its pad.
    pub stitch_islands: bool,
    /// Topological core (`topor.toml [planes] join_islands`, user 2026-11-12, default true): with
    /// `stitch_islands`, a cut-off piece of a pour net's copper that holds no pad (an island the
    /// zone keeps, e.g. remove islands = never) and that no stitch via can reach gets a short
    /// track of the net width from the nearest pad of the net that is connected to the rest of its
    /// net, into the island, so the island is no longer isolated copper; its end stands at least
    /// w/2 + the zone's minimum width deep in the island; priced length x (1 + (d / 3 mm)^2), d
    /// = distance from the pad's edge ([`crate::topo::stitch::join_price`]), no cutoff, a warning
    /// beyond 3 mm. False: such an island is only reported.
    pub plane_join_islands: bool,
    /// Topological core (4.4, `topor.toml [planes] terminals`, user rule 2026-11-10): an own-net
    /// pour is a terminal of its net like a pad, which may carry vias. Only fill connected to the
    /// rest of its net before the run (the main component of [`crate::pourcheck`] on the board as
    /// given) counts: a stub never ends in a cut-off island. A plane pad that no stub serves is
    /// wired to its net's connected fill (a far terminal), ending where the path search first
    /// reaches it (user 2026-11-12: routing over a connected pour has no length), instead of to
    /// another pad of its net. False: stubs into any fill, other pads joined pad to pad (before 4.4).
    pub plane_terminals: bool,
    /// Topological core (4.4, `topor.toml [planes] share_vias`, default `"outside_bga"`): whether
    /// a plane pad's stub may end on own-net copper already on the plane (another pad's stub via,
    /// an existing through via in the fill) instead of a via of its own; a stub via that other
    /// stubs share moves to where their stubs are shortest together. `"outside_bga"` (user
    /// 2026-11-13: "no shared vias under bga chips, shared vias are okay outside of BGA
    /// footprints"): pads and vias in a ball-grid footprint's area ([`crate::topo::planes::bga_areas`])
    /// never share, every plane ball gets its own via; `"never"` (the 2026-11-12 rule: a shared
    /// via conducts heat worse, makes single-via current paths and adds inductance towards the
    /// power pins); `"always"`.
    pub plane_share_vias: String,
    /// Topological core (`topor.toml [planes] bga_refs`, default empty): overrides of the BGA
    /// detection behind `plane_share_vias = "outside_bga"` ([`crate::topo::planes::bga_footprints`]):
    /// "REF" counts footprint REF as a BGA (its whole pad box), "-REF" never.
    pub plane_bga_refs: Vec<String>,
    /// `topor.toml [output] cleanup_safe`: the written copper is made stable under KiCad's Edit ->
    /// Cleanup Tracks & Vias ([`crate::topo::tidy`]): no null, duplicate or in-pad segments, T
    /// junctions as exact vertices, collinear segments merged.
    pub cleanup_safe: bool,
    /// Topological core (D5, `topor.toml [search] net_tree`): `"free"` = a connection joins the
    /// two parts of its net's routed copper it connects, at any pad of them or at a branch point
    /// (T-junction) on one of their routed traces, so the net's tree follows the routing;
    /// `"fixed"` = pad to pad as the decomposition planned (no branch points).
    pub net_tree: String,
    /// Topological core (shopping list 4.2, `topor.toml [search] steiner`): free points on a 2 mm
    /// grid in the open areas of the projection (Steiner points: junction candidates for a branch
    /// point on a trace, and extra triangulation vertices). `false` (default) = the projection's
    /// vertices are the fixed items only (pads, vias, keepouts, copper graphics, outline, cuts), as
    /// TopoR builds it (Polubasov 2010); a net's tree then joins pad to pad or through a pad
    /// already on a trace of its net (TopoR 5.0 black-box E3b: pad-to-pad trees, no free Steiner
    /// points).
    pub steiner: bool,
    /// Topological core (shopping list 4.3, `topor.toml [search] length`): how the search prices a
    /// wire's length. `"taut"` = the shortest path through the wire's sleeve that keeps the
    /// clearances (to the pads at the ends of each crossed edge and to the passes already there),
    /// as TopoR's initial routing takes "the shortest path on the triangulation with the clearances
    /// needed" (Polubasov 2010, «Начальная трассировка»; default since M0); `"even"` (the default
    /// before M0) = between evenly spaced points on each crossed edge, a poor estimate on long
    /// edges (the projection without the Steiner grid).
    pub search_length: String,
    /// Topological core (shopping list 4.3, `topor.toml [optimize] initial`): `"once"` = the
    /// initial routing runs once per run (first cycle): every wire placed (never "no topological
    /// path": a wire without a legal path goes through at the `violation` price), double crossings
    /// removed after every wire, a crossing that no layer assignment separates priced as a
    /// violation; later cycles start from the previous cycle's plan and improve it.
    /// Default since M0 (Polubasov 2010: the initial routing once, then the optimisation of that
    /// topology). `"per_cycle"` (the default before M0) = every cycle routes from scratch.
    pub initial: String,
    /// Topological core (shopping list 5.1, `topor.toml [optimize] loop`): how a run iterates.
    /// `"cycles"` (the default before M0) = whole-route cycles, each planned and drawn, the drawing
    /// feeding back into the next plan (before 5.1). `"topor"` (default since M0) = TopoR's loop (Polubasov 2010 «Оптимизация
    /// разводки»): the initial routing once (as `initial = "once"`), then cycles on the topology
    /// only: single-wire rip-up and reroute with per-wire oscillating coefficients, the layer
    /// re-optimisation with a random choice among equal layerings, the wires whose vias rose laid
    /// again, floating up (the label moves) and the crossing rules; every cycle's plan is judged by
    /// its predicted criteria (violations, vias, length, shrinkages) and kept in an archive of
    /// topological variants, with restarts from the archive; the picked variant is drawn once.
    pub opt_loop: String,
    /// `[optimize] loop_restart` (5.5, `loop = "topor"`): cycles without a gain of the picked
    /// variant after which the loop goes on from a variant of the archive drawn at random (seeded;
    /// Polubasov 2010 «Многовариантная оптимизация»); 0 = never.
    pub loop_restart: usize,
    /// `[optimize] loop_stable_window` (`loop = "topor"`): the loop's plan is stable when its pick
    /// gained less than `stable_gain_pct` % over this many loop cycles (a loop cycle costs no
    /// drawing, so the window is longer than the drawn cycles' `stable_window`).
    pub loop_stable_window: usize,
    /// `[optimize] loop_via_start` / `loop_via_end` (5.3, `loop = "topor"`): the weight of the
    /// via-standing factors (via, crossing) at the start and the end of every cycle's reroute sweep,
    /// graded in between (Luzin & Polubasov 2006 p. 75: length first, vias later in each cycle).
    pub loop_via_start: f64,
    pub loop_via_end: f64,
    /// `[optimize] loop_time_share` (`loop = "topor"`): the share of the time left that the loop may
    /// use at most (an emergency stop, like the time limit; the rest is the drawing's).
    pub loop_time_share: f64,
    /// `[optimize] loop_crossing` (5.2, `loop = "topor"`): the share of the `crossing` factor's
    /// weight the loop's layer-aware reroute keeps for a crossing itself. The loop prices the via
    /// a crossing implies against the crossed stretch's present layer as a via (Polubasov 2010
    /// «Оптимизация разводки»: before the optimisation every stretch gets a layer; a crossing of a
    /// stretch with a via is penalised, fig. 4), so a crossing the wire can take on another layer
    /// costs this share only, one that forces a via pair costs two vias on top.
    pub loop_crossing: f64,
    /// Topological core (D6b, `topor.toml [search] crossing_vias`): `"crossing"` (default) = a
    /// crossing of two nets costs the `crossing` factor, which stands for the layer change P3
    /// makes; only a crossing no layer assignment separates (both wires held to one layer in its
    /// triangle) is priced as a violation (`overflow`); `"via"` = also price the via a crossing of
    /// two stretches held to the same layer implies, and keep the wire on the other layer after it.
    pub crossing_vias: String,
    /// Topological core (D5, `topor.toml [search] junction_pad_mm`): a branch point on a trace
    /// closer than this (mm) to a pad of the copper it joins is not used; the pad is the junction.
    /// 0 (default) = no such distance: the `junction_angle` factor prices a T next to a pad.
    pub junction_pad_mm: f64,
    /// Topological core (D6b, `topor.toml [search] fuse_branches`): after the drawing, a trace
    /// that leaves a junction or a pad beside another trace of its net leaves that trace's copper
    /// where the two part instead (one copper, a T there), when that is cheaper by length and the
    /// `junction_angle` factor and the new track keeps every clearance. Default on.
    pub fuse_branches: bool,
    /// Topological core (S3, `topor.toml [optimize] rounds`): most whole-route cycles per run (the
    /// first included). Since M-B0 a safety cap: a run ends at "Routes stable" (`stable_*`) or at
    /// the user's OK; the cap ends a run that never settles. A work cap, not a time cap: the same
    /// board and settings give the same result on any machine and with any thread count.
    pub topo_rounds: usize,
    /// Topological core (S3, `topor.toml [optimize] max_ripped`): most wires one whole route may
    /// take out and route again (negotiated rip-up-and-reroute, drawing feedback and the P4 moves
    /// together). A work cap.
    pub max_ripped: usize,
    /// Topological core (S3, `topor.toml [optimize] max_expansions`): search cap (expanded search
    /// nodes) per connection; the one retry of a failed connection uses four times this. A work cap.
    pub max_expansions: usize,
    /// Topological core (S3, `topor.toml [optimize] objective`): `"length_vias"` (Pareto archive
    /// over errors, vias, length and shrinkages; the final pick is set by `pick`) or `"area"` (fewest errors, then the least
    /// occupied area).
    pub objective: String,
    /// Topological core (S3, `topor.toml [optimize] mm_per_via`): the via price of the
    /// `errors_weighted` pick (TopoR: good results at 25 mm/via), also the P4 round pick and the
    /// archive's cap pruning.
    pub mm_per_via: f64,
    /// Topological core (`topor.toml [optimize] pick`): the final choice between complete results
    /// (archive variants, seeded runs) for `objective = "length_vias"`: `"errors_vias_length"`
    /// (default; strictly: fewest errors, then fewest open connections, then fewest vias, then the shortest length, then fewest
    /// shrinkages) or `"errors_weighted"` (errors, then the least `length + mm_per_via x vias`,
    /// then shrinkages; the order before 2026-11-10). Only the final pick: the archive's Pareto
    /// membership and the internal search prices do not change. `objective = "area"` ignores it.
    pub pick: String,
    /// Topological core (`topor.toml [optimize] pick_shorts`, user 2026-11-12 "shorts should be
    /// in the routing criteria"): connections with a short (copper over another net's copper:
    /// KiCad `shorting_items` / `tracks_crossing`) are a criterion right after the errors, before
    /// the open connections, in every pick (the archive pick, the seeded runs' pick, the TopoR
    /// loop's plan pick with the predicted shorts) and in the P4 round score (per-edge capacity
    /// model), and in the stable meter (so in the reheat schedule and the stable / finish stop:
    /// it changes a run's trajectory, not only ties). Default true; false = shorts counted only as
    /// errors, as before 2026-11-12 (the archive's cap pruning keeps the pick either way).
    pub pick_shorts: bool,
    /// Topological core (`topor.toml [optimize] short_weight`, PS1, user 2026-11-13): k, how many
    /// errors a connection with a short counts in every ranking (`pick_shorts` on): the ranked
    /// count is `errors + (k - 1) x shorts`, the shorts themselves only a tie-break after it.
    /// Default 1 (a short is one error, TopoR 6.3 manual p. 72; the order of 2026-11-12); values
    /// below 1 count as 1. A k above 1 is not chosen yet: it needs a run that checks the
    /// alternative variants in KiCad.
    pub short_weight: f64,
    /// Topological core (S3, `topor.toml [optimize] variants`): most result variants the archive
    /// keeps besides the champions (fewest errors, shortest, fewest vias, fewest shrinkages,
    /// fewest warnings).
    pub variants: usize,
    /// Topological core (S3, `topor.toml [optimize] moves`): the P4 move descent (reroute,
    /// attachment, merge, channel swap, joint move, double-crossing swap; crossing flip, via
    /// shift, segment lifting) after the negotiated rip-up-and-reroute. Off by default: on the
    /// gate boards it routed more on tanmatsu but fewer on hackerhotel2027 and bornhack2026.
    pub moves: bool,
    /// Topological core (S3, `topor.toml [optimize] max_moves`): most move evaluations of the
    /// descent per whole route (each label sweep has the same cap). A work cap.
    pub max_moves: usize,
    /// Topological core (S3 / M-B0, `topor.toml [optimize] fluctuation`): start width of the
    /// fluctuation schedule (`topo::schedule`): the factor weights of a whole-route cycle are
    /// multiplied by seeded factors in `[1 - width, 1 + width]` (search, rip-up-and-reroute, descent);
    /// the first cycle of the first run uses the plain weights (0 = fixed weights throughout).
    pub fluctuation: f64,
    /// M-B0 (`[optimize] fluctuation_narrowing`): the width is multiplied by this after every cycle.
    pub fluctuation_narrowing: f64,
    /// M-B0 (`[optimize] fluctuation_stuck`): cycles without a gain of the best result, while it is
    /// still incomplete, after which the width is set back to `fluctuation_reheat` (0 = never).
    pub fluctuation_stuck: usize,
    /// M-B0 (`[optimize] fluctuation_reheat`): width after a reheat (then it narrows again).
    pub fluctuation_reheat: f64,
    /// M-B0 (`[optimize] stable_window`): cycles over which the gain of the best result is measured.
    pub stable_window: usize,
    /// M-B0 (`[optimize] stable_gain_pct`): gain (%) over `stable_window` cycles below which the
    /// routes count as stable ("Routes stable" in the progress dialog).
    pub stable_gain_pct: f64,
    /// M-B0: a run ends by itself when its routes are stable (+ `stable_patience` cycles); the
    /// KiCad plugin's dialog sets it false (the user's OK ends the run). Not in topor.toml.
    pub stop_when_stable: bool,
    /// M-B0 (`[optimize] stable_patience`): cycles a run that stops by itself keeps going after
    /// "stable" before it ends.
    pub stable_patience: usize,
    /// M-B0 (`[optimize] finish_when_stable`): a run that does not stop at "stable" by itself (the
    /// plugin's dialog) still finishes on its own once its best result has not improved for
    /// `finish_patience` more cycles after "stable" (a normal finish, no time warning); false =
    /// keep iterating until OK / Cancel / the safety cap / `wall_clock_s` (the TopoR way).
    pub finish_when_stable: bool,
    /// M-B0 (`[optimize] finish_patience`): cycles without a better result after "stable" before
    /// such a run finishes by itself; 0 (default, user 2026-11-07: "When route stable appears it
    /// should stop indeed") = it stops at the cycle that becomes stable.
    pub finish_patience: usize,
    /// M-B0 (`[optimize] runs`): independent seeded runs; the best by the objective is the result.
    /// They run in parallel on the engine's thread pool (threads are shared, not multiplied).
    pub runs: usize,
    /// M-B1 (`[optimize] allow_violations`): never drop. A connection the drawing cannot make
    /// legal is drawn anyway on its best geometry and written to the board; every rule it breaks
    /// is priced by the `violation` factor and reported with its place, type and cause. false =
    /// legal copper only (such a connection is reported unrouted, as before M-B1).
    pub allow_violations: bool,
    /// Router (NX, `topor.toml [search] neck_max_mm`): longest necked stub, nm; 0 = twice the
    /// pad's long side.
    pub neck_max_nm: i64,
    /// M-C (`topor.toml [search] neck_from_pad`): a necked stretch the plan decides runs from the
    /// wire's nearest pad end to the bottleneck (user rule 2026-11-05: a neck starts at the
    /// footprint and may reach further back to free room for others; never a neck in the middle
    /// of a full-width run). False: only the bottleneck passes themselves are necked.
    pub neck_from_pad: bool,
    /// Shopping list 5.10 (`topor.toml [search] pad_exit_stubs`): a connection's exit from a
    /// fine-pitch pad (another net's copper beside it closer than the wire could pass) is fixed
    /// before the topology as a stub along the pad's long axis out of the tip that faces the
    /// connection's other end, `clearance + width` long past the tip; the planner routes the other
    /// nets round it and prices its clearance cuts, the drawing keeps other copper off it.
    /// False (default until the plan chooses the exit tip; today the straight line to the other end
    /// does, shopping list 5.10): the exit is
    /// left to the plan and the drawing (the behaviour before 5.10). Not the classic router's
    /// removed `[search] exit_stubs`.
    pub pad_exit_stubs: bool,
    /// M-C (`topor.toml [search] neck_in_drawing`): the drawing may also neck a pad end the plan
    /// kept at full width when the full-width copper is rejected (the behaviour before M-C).
    /// False (default): the drawing necks only where the plan necked.
    pub neck_in_drawing: bool,
    /// M-C (`topor.toml [vias] reserve_sites`): a via the plan puts in a triangle takes its site
    /// there; another net's via planned in the same small triangle counts as a via without a site,
    /// so P3/P4 move it or reroute its wire before the drawing (true, default).
    pub reserve_via_sites: bool,
    /// M-45 (`topor.toml [drawing] style`): `"octilinear"` (default; straight and 45 degree
    /// segments, the hand boards' style; a stretch where 45 degrees does not fit stays any-angle
    /// and pays the `any_angle` factor) or `"smooth"` (the taut any-angle drawing).
    pub draw_style: String,
    /// M-45 (`[drawing] any_angle`): `"costly"` (default: a stretch that is not at 0/45/90
    /// degrees pays the `any_angle` factor and is reported) or `"forbidden"` (such a run counts
    /// as a drawing failure; never drop still writes it on the last resort).
    pub any_angle: String,
    /// M-45 (`[drawing] acute`): `"costly"` (default: a bend sharper than 90 degrees pays the
    /// `acute_bend` factor and is reported) or `"forbidden"` (the octilinear drawing never makes one).
    pub acute: String,
    /// M-45 (`[drawing] bend_mm`): price of one 45 degree bend of the octilinear drawing, in mm of
    /// length (90 degrees: twice); fewer bends where the length allows.
    pub bend_mm: f64,
    /// M-45 (`[drawing] bend_spacing_mm`): preferred least length between two bends (`L_pref`); a
    /// shorter leg pays more the shorter it is (jog penalty), down to `min(L_pref, 2 (w + s))`.
    pub bend_spacing_mm: f64,
    /// M-45 (`[drawing] octilinear_room`): with the octilinear style, the planner (P3 / P4) and
    /// the drawing's packing count the room an octilinear wire takes in a passage (the octagon
    /// around a wrapped corner, the 0/45 degree legs of a slanted stretch; `demand::octi_stretch`)
    /// instead of the any-angle room.
    pub octi_room: bool,
    /// P6 (c) (`[drawing] pull_taut`): every drawn run is pulled taut inside its corridor (a
    /// vertex goes where the straight leg keeps every rule and the side of every wire not drawn
    /// yet; `draw::Drawer::pull_taut`).
    pub draw_pull_taut: bool,
    /// P6 (b) (`[drawing] lane_walls`): "price" = the lane slot outside a run's span is a graded
    /// price in the octilinear drawing; "hard" = a wall (before P6).
    pub lane_walls: String,
    /// M-45 (`[drawing] bundle_pull`): with the octilinear style, the share of a segment's length
    /// taken off its price where it runs beside a drawn trace of another net at the clearance, so
    /// a trace follows its neighbour and a bundle keeps its pitch through its bends (0 = off).
    pub bundle_pull: f64,
    /// Inflated sketch U0 (`topor.toml [capacity] mixed_clearance`): how the per-layer cut test
    /// (`topo::sheet`, `topo::cutflow`) spaces neighbours of different nets on a cut:
    /// `"conservative"` (default; every gap at the largest clearance of the cut's items, the
    /// additive territory model of Maley 1987 p. 29) or `"pairwise"` (KiCad's max of the two).
    /// Read only by the measurement (U1) so far.
    pub mixed_clearance: String,
    /// Inflated sketch U0 (`[capacity] cut_radius_mm`): the per-layer cut set keeps only cuts at
    /// most this long (mm); 0 = off, the exact critical-cut set (with it on, the report says
    /// "routability not proven", Maley p. 53). Read only by the measurement so far.
    pub cut_radius_mm: f64,
    /// Inflated sketch U1 (`[capacity] cut_report`, debug): a directory; when set, the topological
    /// core builds the per-layer sheets and cut flows after P3, after every P4 round and for the
    /// plan handed to the drawing, and writes them there as JSON (`run<k>/cycle<c>.json`). Empty
    /// (default): off. Never changes the routing.
    pub cut_report: String,
    /// P1a fit authority (`[capacity] model`, shopping list 2.2): which capacity model prices the
    /// layer assignment (P3), the P4 label moves and the P4 pick of the best round. `"edges"`
    /// (default; user 2026-11-13, interim until the cut prices are fast enough for a large board
    /// within the time limit and an early limit draws the initial plan): the per-edge packer of the shared projection
    /// (`topo::refine::CapModel`), its over-full edge-layers priced as [`Options::edges_test`]
    /// says (default `"price"`). `"cuts"` (the method's capacity price, default in M0 before 2026-11-13): the per-layer cut test (`topo::cutmodel`, the critical cuts of each layer's sheet
    /// in the planning norm) with the graded over-full price `overflow x phi` per cut; P2's
    /// per-edge room of an edge with an end that is no copper on a layer then comes from that
    /// layer's sheet (`topo::cutmodel::sheet_room`, shopping list 2.3), and P4 is priced by the
    /// cuts (shopping list 3.2): victims of the negotiated rounds are the wires of over-full cuts,
    /// the topology and label moves keep a change by its graded total (no hard capacity test) and
    /// the pick of the best round is objective + over-full price.
    pub capacity_model: String,
    /// M1 compass (`[capacity] compass_cut_check`): with `capacity_model = "edges"` the compass
    /// also runs the per-layer cut test on the plan handed to the drawing, read-only and after the
    /// drawing, so the model gap of `docs/dev/inflated-sketch.md` §6.1 (per-edge packer: fits,
    /// cut test: over-full) is measured. Its time goes back to the time limit. Off by default
    /// (user 2026-11-13); `harness.score_runs` turns it on. Never changes the routing.
    pub compass_cut_check: bool,
    /// M0 (`[capacity] edges_test`, `capacity_model = "edges"` only): how the per-edge model's
    /// over-full edge-layers enter P4. `"price"` (default) = as the method prices everything
    /// (Polubasov 2010 «Оптимизация разводки»: one price S = sum k_i x_i): a topology move pays
    /// the `overflow` weight per over-room or unsafe pass, a label move is kept on its price alone
    /// (the over-full price of the edge-layers + vias) and the pick of the best P4 round is the
    /// objective plus the graded over-full price, as with `"cuts"`. `"refuse"` = the hard test
    /// before M0 (a move with more over-room passes, or a label move that makes an edge-layer more
    /// over-full or leaves a via without a site, is refused whatever its price; the rounds are
    /// picked by the over-full count first, the objective second). The cut model is always priced.
    pub edges_test: String,
    /// `[capacity] overflow_softness` (model = "cuts"): the softness `tau` of the graded over-full
    /// price, in track pitches: the price of a cut rises smoothly from almost 0 at a slack of a few
    /// `tau` pitches through about 0.57 x `overflow` at zero slack to `overflow x (1 + overflow in
    /// pitches)` (`topo::cutmodel::phi`).
    pub overflow_softness: f64,
    /// `[capacity] cut_vias`: the layer refinement (model = "cuts") and the P4 label moves (model =
    /// "cuts", and since M0 model = "edges" with edges_test = "price") may add vias where they
    /// remove more over-full price than the vias cost (false: only moves that keep the via price).
    pub cut_vias: bool,
    /// C0 (`[run] stop_after`, measurement): `"plan"` ends every run of the topological core
    /// after the plan of its first cycle (later cycles need the drawing's feedback), before the
    /// drawing; the result has no copper and its warnings carry the plan metrics line. `""`
    /// (default): the full run.
    pub stop_after: String,
    /// D7 (`topor.toml [diffpair] enabled`): route the pairs of `rules.diff_pairs` as coupled
    /// pairs (true, default); false leaves their connections unrouted with the reason
    /// "diff pair: not in the rough version" (the behaviour before D7).
    pub diffpair_route: bool,
    /// D7 (`[diffpair] zip_max_mm`): how far from a pad (or via) along the pair's centre line the
    /// two halves may stay apart before they zip at the pair gap (the fan-out zone, TopoR's movable
    /// split; the drawing tries the nearest legal zip point first). nm.
    pub diffpair_zip_max_nm: i64,
    /// D7 (`[diffpair] via_pair_axes`): allowed directions (degrees) of the line through the two
    /// vias of a via pair.
    pub diffpair_via_axes: Vec<f64>,
    /// 5.4 (`topor.toml [vias] size`): which via the router puts at a layer change, in the plan
    /// (room, via sites) and in the drawing. `"net_class"` (default, the behaviour before 5.4) =
    /// the net class via first, then the smaller pre-defined / fab-profile vias for tight spots
    /// ([`crate::vias::via_sizes`]). `"board_min"` = the smallest via the Board Setup minimums
    /// allow (min via diameter, min through-hole drill, min annular ring: the supplier's
    /// capability) that still carries the trace's current: drill = the smallest with
    /// pi x drill >= the track width (a diff pair: its `diff_pair_width`), rounded up to
    /// `via_drill_grid_nm`; diameter = max(min via diameter, drill + 2 x min annular ring)
    /// (user rule 2026-11-10, memory power-via-rules rule 0; [`crate::vias::rule_size`]).
    pub via_size: String,
    /// 5.4 (`[vias] drill_grid_mm`): with `via_size = "board_min"` the drill is rounded up to a
    /// multiple of this (nm; default 0.01 mm; a fab usually offers 0.05 mm steps). 0 = no rounding.
    pub via_drill_grid_nm: i64,
    /// 5.4 (`[vias] pair_min`): local via-pair minimisation in the topology (Luzin, Popov & Popov
    /// 2013 p. 2 steps 1-10, [`crate::topo::viaopt`]): a via pair whose stretch could stay on the
    /// layer of its neighbours (by re-laying it round what it dives under, or by moving the
    /// stretch between two adjacent vias to the other layer) is removed when that is cheaper by
    /// the plan's price (mm + via price); a priced move, never a gate. Runs in the TopoR loop
    /// (`loop = "topor"`, every cycle) and once before the drawing in the `cycles` loop.
    pub via_pair_min: bool,
    /// 5.4 (`[vias] pair_max_mm`): the via-pair minimisation looks at pairs of vias joined by a
    /// stretch at most this long (mm; Luzin-Popov 2013 p. 2: "shorter than a given length, e.g.
    /// 10 mm").
    pub via_pair_max_mm: f64,
    /// 5.4 (`[vias] relocate`): the plan puts every via at the point of its free interval where
    /// the forces of the copper around it balance (Dayan & Dai 1991; TopoR FreeStyle "Shift
    /// vias", 6.3 manual p. 179; Luzin-Popov 2013 p. 3), equal clearance between two neighbours,
    /// and the drawing takes that site first. A move never raises the predicted violations.
    /// Own pads push too (force only, no rule). Needs `search_length == "taut"` (skipped with a
    /// warning otherwise: the "even" corridor is no wire path).
    pub via_relocate: bool,
    /// 5.4 (`[search] branch_relocate`): the plan moves each free junction (T-point) of a net
    /// tree to the point of least length of its three arms (the Fermat / Steiner point, all
    /// angles < 120 degrees), and pulls a junction out of a pad when the two arms at the pad meet
    /// at less than 120 degrees; a junction whose arms meet at >= 120 degrees at a pad goes into
    /// that pad (Popov & Popov EDN 2013; Dayan & Dai 1991 pp. 2-5). Any-angle point; with the
    /// octilinear drawing style the point of least octilinear length (`viaopt::fermat_octi`).
    pub branch_relocate: bool,
    /// 5.4 (`[vias] between_crossings`): a via sits strictly between the two events of its
    /// interval (crossings, tees, one-layer triangles, narrow passes), never in a crossing's own
    /// triangle (Polubasov 2001 p. 4: a layer change lies on the wire piece between two vertices
    /// of the domain graph, the crossings). The plan (layer refinement, via sites, the TopoR
    /// loop's price) counts an interval without a triangle there as a via without a site; the
    /// drawing searches only those triangles. false = the interval includes the triangles of its
    /// events (before 5.4; Q15: wiccon, a via planned in the crossing's triangle cannot be drawn
    /// and the wire is forced across other nets).
    pub via_between: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            margin_nm: 0,
            wall_clock_s: 60.0,
            detour_factor: 2.0,
            factors: Default::default(),
            drc_loop_cap: 5,
            seed: 1,
            existing_copper: ExistingCopper::Fixed,
            scope: Scope::default(),
            skip_self_check: false,
            drc_spots: Vec::new(),
            dangling_ends: "continue".into(),
            plane_signals: "costly".into(),
            allowed_drills_nm: Vec::new(),
            via_in_pad: false,
            plane_vias: true,
            net_priority: BTreeMap::new(),
            exclude_nets: Vec::new(),
            ignore_pads: Vec::new(),
            pour_served: BTreeMap::new(),
            neck_to_pad: true,
            drawing_feedback: false,
            via_offset_sites: true,
            feedback_max_rip: 8,
            history_fade: 0.5,
            soft_cuts: true,
            crossing_rules: true,
            reroute_rounds: 6,
            via_reduce: true,
            stitch_islands: true,
            plane_join_islands: true,
            plane_terminals: true,
            plane_share_vias: "outside_bga".into(),
            plane_bga_refs: Vec::new(),
            cleanup_safe: true,
            net_tree: "free".into(),
            steiner: false,
            search_length: "taut".into(),
            initial: "once".into(),
            opt_loop: "topor".into(),
            loop_restart: 2,
            loop_stable_window: 8,
            loop_via_start: 0.5,
            loop_via_end: 1.5,
            loop_time_share: 0.5,
            loop_crossing: 0.25,
            crossing_vias: "crossing".into(),
            junction_pad_mm: 0.0,
            fuse_branches: true,
            topo_rounds: 30,
            max_ripped: 5000,
            max_expansions: 2_000_000,
            objective: "length_vias".into(),
            mm_per_via: 25.0,
            pick: "errors_vias_length".into(),
            pick_shorts: true,
            short_weight: 1.0,
            variants: 8,
            moves: false,
            max_moves: 400,
            fluctuation: 0.2,
            fluctuation_narrowing: 0.7,
            fluctuation_stuck: 2,
            fluctuation_reheat: 0.2,
            stable_window: 4,
            stable_gain_pct: 0.5,
            stop_when_stable: true,
            stable_patience: 0,
            finish_when_stable: true,
            finish_patience: 0,
            runs: 5,
            allow_violations: true,
            neck_max_nm: 0,
            neck_from_pad: true,
            pad_exit_stubs: false,
            neck_in_drawing: false,
            draw_style: "octilinear".into(),
            any_angle: "costly".into(),
            acute: "costly".into(),
            bend_mm: 0.25,
            bend_spacing_mm: 0.5,
            octi_room: true,
            draw_pull_taut: true,
            lane_walls: "price".into(),
            bundle_pull: 0.3,
            reserve_via_sites: true,
            mixed_clearance: "conservative".into(),
            cut_radius_mm: 0.0,
            cut_report: String::new(),
            capacity_model: "edges".into(),
            compass_cut_check: false,
            edges_test: "price".into(),
            overflow_softness: 0.1,
            cut_vias: true,
            stop_after: String::new(),
            diffpair_route: true,
            diffpair_zip_max_nm: 3_000_000,
            diffpair_via_axes: vec![0.0, 45.0, 90.0, 135.0],
            via_size: "net_class".into(),
            via_drill_grid_nm: 10_000,
            via_pair_min: false,
            via_pair_max_mm: 10.0,
            via_relocate: true,
            branch_relocate: false,
            via_between: true,
        }
    }
}

impl Options {
    pub fn from_json(s: &str) -> Result<Self, String> {
        if s.trim().is_empty() {
            return Ok(Options::default());
        }
        // Keys this engine does not know (options of the classic router removed in the CL cleanup,
        // e.g. `expert`, `threads`, `rip_pass`) are ignored, so old callers keep working.
        serde_json::from_str(s).map_err(|e| format!("options json: {e}"))
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

#[cfg(test)]
mod tests {
    use super::Options;

    /// M0 (user 2026-11-12, "change the options to match TopoR"): the defaults are the TopoR method
    /// (Polubasov 2010): initial routing once, taut lengths, TopoR's loop on the topology, no drawing feedback, the vias relocated between their
    /// crossings (the plugin's `auto` with taut), and every capacity judgement priced. The capacity
    /// model is the per-edge one, priced (user 2026-11-13, interim: the cut model, the method's
    /// price, is too slow for qmtech within the time limit). The plugin's
    /// schema (`plugin/topor/schema.py`) has the same defaults (tests/plugin/test_config.py).
    #[test]
    fn m0_defaults_are_the_topor_method() {
        let o = Options::default();
        assert_eq!(
            (o.opt_loop.as_str(), o.initial.as_str(), o.search_length.as_str(), o.capacity_model.as_str(), o.edges_test.as_str()),
            ("topor", "once", "taut", "edges", "price")
        );
        assert!(!o.drawing_feedback);
        assert!(o.via_relocate && o.via_between);
    }
}
