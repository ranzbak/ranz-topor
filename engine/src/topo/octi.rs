//! Octilinear drawing (plan P6 `topo/octi`, M-45): a run the drawing has made legal as a taut
//! any-angle polyline is redrawn with straight and 45 degree segments only, where that fits.
//!
//! Method (Bast, Brosi & Storandt 2020, "Metro Maps on Octilinear Grid Graphs", EuroVis, sections
//! 3-4: a path in a graph whose nodes have 8 ports, bends priced per angle, obstacles excluded,
//! solved greedily per line in line order): the wire's own corridor is the sleeve of its
//! topology (the triangle strip with its pockets, [`super::exact`]); the nodes are the taut path's
//! own vertices (the TopoR minimum-length wire, Popov & Popov 2012 pp. 117-122) plus the vertices
//! of the *octagons* around every obstacle corner near it (the octagon clearance metric: the
//! circumscribed octagon of the clearance circle, edges at 0/45/90 degrees, so a wire that hugs
//! an obstacle or a wire drawn before it runs parallel to it at exactly the clearance, which keeps
//! a bundle's pitch through its bends). Between two nodes the connectors are octilinear: straight
//! when the nodes are aligned, else the two elbows (one bend) and the two Z shapes (two bends,
//! middle leg centred; the coordinates of the bends follow from the nodes, the "bend LP" of the
//! plan reduced to the elbow / Z choice). A dynamic programme over the nodes in path order with
//! the arrival direction as state (8 ports) finds the cheapest chain: length, plus a price per
//! bend growing with its angle (90 degrees costs twice 45), plus the jog penalty (a bend after a
//! leg shorter than `L_min = min(L_pref, 2 (w + s))` pays more the shorter the leg), plus the
//! `acute_bend` factor for a turn sharper than 90 degrees, plus a pad exit / entry that does not
//! follow the pad's long axis (graded by its angle; a leg along the axis to just past the pad's
//! tip is a connector of its own). Every connector leg must stay inside the sleeve (no wall
//! crossed: the topology is kept) and keep every clearance to the board and to the copper drawn
//! so far. The original taut segments are always available as drawn, without the leg test
//! (first-draw-45 P1: an octilinear one too, so one piece the leg test refuses no longer leaves
//! the run without any chain), an any-angle one priced by the `any_angle` factor (15 mm per
//! stretch plus 2 per mm by default), and in the first draw every one by its clearance below the
//! rule (the forced DP's `violation` price, costly not failed, counted in [`OctiOut::asis`]): where
//! 45 degrees does not fit, the stretch stays any-angle and is reported (never an open wire or a
//! new violation because of the style).
//!
//! The caller checks the result with the full checker and keeps the taut run when it is rejected.
//!
//! Complexity: nodes N (taut vertices plus octagon vertices within two pitches of the path,
//! capped), connector window J: O(N x J x 8 x shapes) prices; each leg's clearance test is
//! O(local objects) and cached. Deterministic (index order, `total_cmp`).

use super::draw::{bbox_of, Core, Drawer};
use super::exact::{Neck, Strip, Wp};
use super::state::{point_in_poly, WireId, NO_OBS};
use crate::geom::prim::Aabb;
use crate::geom::Vec2;
use crate::fxhash::FxHashMap;
use std::f64::consts::FRAC_PI_4;

const MM: f64 = 1e6;
/// Most nodes of one run (taut vertices always kept); above it the farthest octagon nodes go.
const MAX_NODES: usize = 260;
/// Connector window: a node connects to the next `WINDOW` nodes in path order, and to every node
/// up to `REACH_MM` further along the taut path (at most `WINDOW_MAX` nodes).
/// par-draw (docs/dev/design/par-draw.md section 13): a node of the octilinear DP whose
/// (arrival state, successor) pairs reach this count runs its successors on the drawing's lanes
/// ([`super::par::map`]); a smaller one stays on the calling thread. A work-size rule: the copper
/// is the same either way.
const OCTI_LANE_MIN_PAIRS: usize = 64;

/// par-draw: an `octilinear` call with at least this many nodes takes its lanes (a crew,
/// [`super::par::crew`]) for the whole call; a smaller one stays on the calling thread. A
/// work-size rule, as [`OCTI_LANE_MIN_PAIRS`].
const OCTI_CREW_MIN_NODES: usize = 24;

/// Nodes whose successors went to [`super::par::map`] (tests: the lanes were used).
pub static OCTI_NODES_LANED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The memo of the leg prices and the legality of one `octilinear` call (pure functions of the
/// leg, keyed by its end points' bits).
#[derive(Default)]
struct Memo {
    room: FxHashMap<[u64; 4], f64>,
    follow: FxHashMap<[u64; 4], f64>,
    legal: FxHashMap<[u64; 4], bool>,
    /// forced-draw: the violation price of a leg of a forced run.
    viol: FxHashMap<[u64; 4], f64>,
}

impl Memo {
    /// A lane's overlay into the call's memo (the values are the same whoever computed them).
    fn merge(&mut self, o: Memo) {
        self.room.extend(o.room);
        self.follow.extend(o.follow);
        self.legal.extend(o.legal);
        self.viol.extend(o.viol);
    }
}

fn leg_key(a: Vec2, b: Vec2) -> [u64; 4] {
    [a.x.to_bits(), a.y.to_bits(), b.x.to_bits(), b.y.to_bits()]
}

/// `f()` memoised: the shared memo (read only), else the own one, else computed into the own one.
fn memo_get<V: Copy>(shared: Option<&FxHashMap<[u64; 4], V>>, own: &mut FxHashMap<[u64; 4], V>, key: [u64; 4], f: impl FnOnce() -> V) -> V {
    if let Some(&v) = shared.and_then(|m| m.get(&key)) {
        return v;
    }
    *own.entry(key).or_insert_with(f)
}

const WINDOW: usize = 36;
const WINDOW_MAX: usize = 160;
const REACH_MM: f64 = 12.0;
/// first-draw-45 P4: most chord nodes on one taut segment (S-route staircase nodes; a work-size
/// bound: a longer segment gets its nodes further apart than a pitch).
const CHORD_NODES: usize = 48;

/// The pitch (width plus spacing, nm) of wire `wi`.
fn pitch_of(wi: &super::state::Wire) -> f64 {
    wi.w + wi.s
}

/// Octagon vertex radius over the clearance radius: 1 / cos(22.5 degrees), plus a hair.
const OCT_R: f64 = 1.082_392_200_292_394 * (1.0 + 2e-6);

/// Prices and switches of the octilinear drawing (from `topor.toml [drawing]` and `[factors]`).
#[derive(Clone, Copy, Debug)]
pub struct OctiCfg {
    /// `[drawing] style = "octilinear"`.
    pub on: bool,
    /// Price of a 45 degree bend (nm of length; a turn of `a` degrees pays `a / 45` of it).
    pub bend_nm: f64,
    /// `L_pref` of the jog penalty (nm).
    pub spacing_nm: f64,
    /// first-draw-45 r7 (`[drawing] short_leg_mm`): weight (nm) of the staircase price
    /// ([`bend_price_seq`], [`shortness`]).
    pub short_leg_nm: f64,
    /// `any_angle` factor weight (nm per stretch; plus 2/15 of it per mm).
    pub any_nm: f64,
    pub any_forbidden: bool,
    /// `acute_bend` factor weight (nm per bend, graded by the angle).
    pub acute_nm: f64,
    pub acute_forbidden: bool,
    /// Bundles (`[drawing] bundle_pull`): share of a leg's length taken off its price where it
    /// runs parallel to a drawn wire of another net at that wire's clearance (it follows its
    /// neighbour, so a bundle keeps its pitch through its bends).
    pub pull: f64,
    /// `[drawing] bundle_pull_range_mm` (nm): how far the bundle pull may move a run to run beside
    /// a drawn neighbour: the straightening's hug forms ([`hug_within_range`]) and, r9, the DP's
    /// bundle reach (a drawn wire's corner is a node up to 2 pitches past its clearance and no
    /// further from the taut path than the range); infinite = no limit beyond those (the default).
    pub pull_range: f64,
    /// `planned_room` factor weight: price per nm of drawn shape inside the room the plan keeps
    /// for a lane neighbour not drawn yet, times `x + x^2` (`x` = depth over the pitch).
    pub room: f64,
    /// forced-draw: `violation` factor weight (nm per violation unit): the price of a forced
    /// run's leg that comes closer than the rule ([`Drawer::octilinear`] with `forced`).
    pub viol_nm: f64,
    /// forced-draw: the price per violation unit of a leg whose copper overlaps foreign copper (a
    /// short) in the forced DP's shorts-first variant, `viol_nm` x [`SHORT_FIRST`]: the DP's one
    /// scale carries the choice key's "shorts first" ([`super::draw::forced_form_key`]).
    pub short_nm: f64,
}

/// forced-draw: a short's violation unit costs this many clearance units in the shorts-first
/// variant of the forced octilinear DP. The choice key ranks the short amount before everything
/// else (KiCad Cleanup removes a shorting track: [`super::draw::viol_key`]); the DP has one scalar
/// price, so a short unit is priced far above what a run's clearance, style and length add up to
/// (about 1e2 units on qmtech's deepest forced runs), graded by the short's own depth and length.
/// Not a judgement of its own: the order is the key's. The DP prices per leg, the key per object
/// (a violation of one object is a short as a whole once any piece of it overlaps,
/// [`super::draw::merge_viols`]), so the forced run also gets the variant with a short priced
/// like a clearance unit, and the key picks ([`super::draw::forced_form_pick`]).
pub const SHORT_FIRST: f64 = 1e4;

/// pad-snap (A1b, docs/dev/design/pad-snap.md): how far the round end of a pad exit's axis leg
/// sticks out of its pad at the *leave point* (nm). More than tidy's `PAD_TOL_NM` (5 um) and
/// KiCad's default maximum error (5 um, `ERROR_INSIDE` polygons), so neither topor's tidy nor
/// KiCad's Cleanup (`deleteTracksInPads`: the track polygon minus the pad polygon empty) sees the
/// centre stub as lying wholly inside the pad.
pub const LEAVE_NM: f64 = 20_000.0;

/// pad-snap: tidy's tolerance for a segment wholly inside a pad (`tidy.rs` `PAD_TOL_NM`).
const STUB_TOL_NM: f64 = 5_000.0;

/// pad-snap: price of a first (last) leg of a run whose far (near) end lies so deep inside the
/// run's own pad that the stub from the pad centre is wholly inside it (nm of length: 4 bends at
/// the default 0.25 mm; [`OctiCfg::stub_nm`] derives it from the config, this is its default). tidy and KiCad's Cleanup delete that stub, so the track ends off the pad
/// centre, where KiCad's push-and-shove router does not link it to the pad (`pns_node.cpp`
/// `NODE::addSolid`: the pad's joint is its anchor). Graded by how far off the centre the end
/// lands: times `1 + off / extent` (extent = the pad's farthest outline point from the end).
/// Costly, not failed: a legal shape is never dropped for it.
pub const STUB_NM: f64 = 1.0e6;

/// Share of one violation unit's price ([`OctiCfg::viol_nm`]) the dearest stub (2 x base) may cost:
/// 0.8 < 1, so short > clearance > stub holds for every `violation` weight.
const STUB_VIOL_SHARE: f64 = 0.4;

/// pad-snap: the [`STUB_NM`] price of a stub from `c` (a run end at its own pad `poly`, `ext` =
/// the farthest outline point from `c`) to `j`, for a track of width `w`: 0 unless `j` lies in the
/// pad at least `w / 2` (less tidy's tolerance) from its outline. O(outline).
pub fn stub_price(poly: &[Vec2], c: Vec2, ext: f64, w: f64, j: Vec2, base: f64) -> f64 {
    if (j - c).len2() < 1.0 || !point_in_poly(j, poly) || super::padend::boundary_dist(j, poly) < 0.5 * w - STUB_TOL_NM {
        return 0.0;
    }
    base * (1.0 + (j - c).len() / ext.max(1.0))
}

/// pad-snap r2b (D3b): the leave point of a run end `c` inside its own pad `poly` in the unit
/// direction `d` for a track of width `w`: the nearest point `c + t d` from which a stub from `c`
/// is not wholly inside the pad (its round end sticks out by [`LEAVE_NM`]: the point's distance to
/// the outline is at most `w / 2 - LEAVE_NM`, or it lies outside). `None` when `c` already is that
/// near the outline (no stub can lie wholly inside) or the ray does not leave the pad.
/// Bisection on `t` between `c` and the ray's exit from the pad. O(outline x 40).
pub fn leave_point(poly: &[Vec2], c: Vec2, d: Vec2, w: f64) -> Option<Vec2> {
    let lim = 0.5 * w - LEAVE_NM;
    let out = |t: f64| {
        let q = c + d * t;
        !point_in_poly(q, poly) || super::padend::boundary_dist(q, poly) <= lim
    };
    if poly.len() < 3 || out(0.0) {
        return None;
    }
    // the ray's exit: the nearest crossing with an outline edge
    let n = poly.len();
    let mut hi = f64::INFINITY;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let e = b - a;
        let den = d.cross(e);
        if den.abs() < 1e-12 {
            continue;
        }
        let t = (a - c).cross(e) / den;
        let u = (a - c).cross(d) / den;
        if t > 0.0 && (-1e-9..=1.0 + 1e-9).contains(&u) {
            hi = hi.min(t);
        }
    }
    if !hi.is_finite() {
        return None;
    }
    let mut lo = 0.0;
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if out(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(c + d * hi)
}

impl OctiCfg {
    /// pad-snap: base price of a pad stub (see [`STUB_NM`]): `4 x bend_nm` (design note), but at
    /// most [`STUB_VIOL_SHARE`] of one violation unit's price, so the order short > clearance >
    /// stub (dearest stub = 2 x base < one violation unit) holds for every `violation` weight the
    /// config accepts (a weight of 0 makes violations free and the stub free with it).
    ///
    /// fd45 r10: at least the wire's `pitch` (track width + clearance), so a stub never goes
    /// free with the bend price: at `bend_mm = 0` the DP took an elbow whose bend lay inside the
    /// pad (tidy deletes the stub, the end sits off the centre; topo_pad_snap side_exit, exposed by
    /// the P4 chord nodes). The default (`4 x 0.25 mm` = 1 mm) is above every usual pitch.
    pub fn stub_nm(&self, pitch: f64) -> f64 {
        (4.0 * self.bend_nm).max(pitch).min(STUB_VIOL_SHARE * self.viol_nm)
    }

    /// From the engine options: `[drawing]` and the `any_angle` / `acute_bend` weights of
    /// `[factors]` (unfluctuated: the drawing's prices stay put across cycles).
    pub fn from_opts(o: &crate::model::Options) -> OctiCfg {
        let reg = super::factors::Registry::from_config(super::factors::standard_factors(), &o.factors).0;
        OctiCfg {
            on: o.draw_style == "octilinear",
            bend_nm: o.bend_mm.max(0.0) * MM,
            spacing_nm: o.bend_spacing_mm.max(0.0) * MM,
            short_leg_nm: o.short_leg_mm.max(0.0) * MM,
            any_nm: reg.weight("any_angle").unwrap_or(15.0) * MM,
            any_forbidden: o.any_angle == "forbidden",
            acute_nm: reg.weight("acute_bend").unwrap_or(50.0) * MM,
            acute_forbidden: o.acute == "forbidden",
            pull: o.bundle_pull.clamp(0.0, 0.9),
            pull_range: if o.bundle_pull_range_mm > 0.0 { o.bundle_pull_range_mm * MM } else { f64::INFINITY },
            room: reg.weight("planned_room").unwrap_or(5.0).max(0.0),
            viol_nm: reg.weight("violation").unwrap_or(1000.0).max(0.0) * MM,
            short_nm: reg.weight("violation").unwrap_or(1000.0).max(0.0) * MM * SHORT_FIRST,
        }
    }
}

impl Default for OctiCfg {
    fn default() -> Self {
        OctiCfg { on: false, bend_nm: 0.25 * MM, spacing_nm: 0.5 * MM, short_leg_nm: 0.0, any_nm: 15.0 * MM, any_forbidden: false, acute_nm: 50.0 * MM, acute_forbidden: false, pull: 0.3, pull_range: f64::INFINITY, room: 5.0, viol_nm: 1000.0 * MM, short_nm: 1000.0 * MM * SHORT_FIRST }
    }
}

/// An octilinear run: its points, and what is left of the style breaks.
#[derive(Clone, Debug, Default)]
pub struct OctiOut {
    pub pts: Vec<Vec2>,
    /// first-draw-45 P1: stretches of the taut path (consecutive octilinear legs) kept as drawn
    /// where the leg test refuses them (no legal connector there), and of those the ones below
    /// the clearance.
    pub asis: usize,
    pub asis_below: usize,
}

/// Debug (`TOPO_FD45_OFF=p1,p2,p5`, first-draw-45 ladder arms): the named part of the change
/// off, the behaviour of base 5571d1bd for it. Read once per process.
pub fn fd45_off(part: &str) -> bool {
    static OFF: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    OFF.get_or_init(|| std::env::var("TOPO_FD45_OFF").unwrap_or_default()).split(',').any(|x| x == part)
}

/// Debug (`TOPO_FD45_ON=slack`): the r3 measurement arm of P2 that lets an own portal take the
/// undrawn wire's packed slack (its wall at the far end of its interval less the spacing, the
/// room between priced by the graded gate). Off by default: esp r3 (USB_D+ forced, 0 -> 14
/// violations, drc_new 49 -> 83) against 49 -> 46 with the walls at the planned crossing.
pub fn fd45_on(part: &str) -> bool {
    static ON: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ON.get_or_init(|| std::env::var("TOPO_FD45_ON").unwrap_or_default()).split(',').any(|x| x == part)
}

/// Debug (`TOPO_OCTINODES=<wire>[,<wire>...]`): dump the DP's nodes, chain and gates of these wires.
fn octi_nodes_dbg(w: WireId) -> bool {
    static W: std::sync::OnceLock<Vec<usize>> = std::sync::OnceLock::new();
    W.get_or_init(|| std::env::var("TOPO_OCTINODES").unwrap_or_default().split(',').filter_map(|x| x.trim().parse().ok()).collect()).contains(&w)
}

/// Unit vector of octilinear direction `k` (k x 45 degrees, math frame).
pub fn dir(k: usize) -> Vec2 {
    let a = (k % 8) as f64 * FRAC_PI_4;
    Vec2 { x: a.cos(), y: a.sin() }
}

/// The octilinear direction of `d`, if it is one (perpendicular deviation under 1 nm, or 1e-6 of
/// its length): `None` for any other direction or a vector shorter than 1 nm.
pub fn octi_dir(d: Vec2) -> Option<usize> {
    let l = d.len();
    if l < 1.0 {
        return None;
    }
    let k = ((d.y.atan2(d.x) / FRAC_PI_4).round() as i64).rem_euclid(8) as usize;
    (d.cross(dir(k)).abs() <= 1.0 + 1e-6 * l).then_some(k)
}

/// Turn angle (degrees, 0 .. 180) from direction `a` to direction `b` (unit or not).
pub fn turn_deg(a: Vec2, b: Vec2) -> f64 {
    let (la, lb) = (a.len(), b.len());
    if la < 1e-12 || lb < 1e-12 {
        return 0.0;
    }
    (a.dot(b) / (la * lb)).clamp(-1.0, 1.0).acos().to_degrees()
}

/// `any_angle` amount of a stretch of `len_nm`: 1 per stretch plus 2/15 per mm (the plan's
/// 15 mm per stretch plus 2x its length at the default weight 15).
pub fn any_angle_amount(len_nm: f64) -> f64 {
    1.0 + len_nm / MM * (2.0 / 15.0)
}

/// octi-bundle: the `any_angle` amount of the polyline `pts` (the sum of [`any_angle_amount`] over
/// its any-angle stretches, [`breaks`]); 0 for an octilinear polyline.
pub fn any_price(pts: &[Vec2]) -> f64 {
    breaks(pts).0.iter().map(|a| any_angle_amount(a.2)).sum()
}

/// fd45 r10: the least price of a 45 degree bend, nm ([`bend_price`]): a tie-break, not a price.
pub const BEND_TIE_NM: f64 = 1_000.0;

/// The leg length under which a bend pays the jog penalty: `L_min = min(L_pref, 2 (w + s))`
/// (`pitch` = w + s, nm; at least 1 nm). The octilinear DP's and [`style_price`]'s.
pub fn jog_leg(cfg: &OctiCfg, pitch: f64) -> f64 {
    cfg.spacing_nm.min(2.0 * pitch).max(1.0)
}

/// The octilinear DP's price of a turn from direction `u` (after a leg of `l_in` nm; `u` zero:
/// no leg before, no price) to direction `v`, nm: `bend_nm` per 45 degrees, plus `acute_bend`
/// beyond 90 degrees, plus the jog penalty `2 bend_nm (1 - l_in / leff)^2` after a leg shorter
/// than `leff` ([`jog_leg`]); infinite for a reversal (over 179 degrees) or a forbidden acute
/// turn. The one bend price of the first draw and of every redraw that prices on its scale
/// ([`style_price`]). O(1).
pub fn bend_price(cfg: &OctiCfg, leff: f64, u: Vec2, l_in: f64, v: Vec2) -> f64 {
    if u.len2() < 1e-18 {
        return 0.0;
    }
    let t = turn_deg(u, v);
    if t < 0.5 {
        return 0.0;
    }
    if t > 179.0 {
        return f64::INFINITY;
    }
    // fd45 r10: a bend is never free. At `bend_mm = 0` ("only length counts") a staircase and its
    // elbow are equally long in the octilinear metric; the floor (1 um per 45 degrees, below any
    // length the copper can tell) breaks that tie towards fewer bends instead of the node order
    // (the P4 chord nodes drew length-equal staircases that cost later wires their room:
    // topo_pad_snap side_exit).
    let mut p = cfg.bend_nm.max(BEND_TIE_NM) * t / 45.0;
    if t > 90.5 {
        if cfg.acute_forbidden {
            return f64::INFINITY;
        }
        p += cfg.acute_nm * acute_amount(t);
    }
    // The jog penalty with the same floor (fd45 r10, verifier F8): at `bend_mm = 0` a staircase
    // of long and tiny legs and one of long legs only cost the same; the floor ranks the tiny
    // legs lower, per leg and graded by its shortness (the short-leg price of
    // [`bend_price_seq`] adds only after two short legs, so a lone short leg (eprom A0: 45
    // degrees, 0.17 mm, 45 degrees, the shortest form) pays no more than this tie-break there).
    if l_in < leff {
        p += 2.0 * cfg.bend_nm.max(BEND_TIE_NM) * (1.0 - l_in / leff).powi(2);
    }
    p
}

/// first-draw-45 r7: [`bend_price`] of the turn after the leg `l_in`, whose previous leg was
/// `l_prev` (infinite: none), plus the staircase price of the two legs ([`shortness`]). The
/// octilinear DP carries `l_prev` with its label, [`style_price`] reads it off the polyline. O(1).
pub fn bend_price_seq(cfg: &OctiCfg, leff: f64, u: Vec2, l_in: f64, l_prev: f64, v: Vec2) -> f64 {
    let p = bend_price(cfg, leff, u, l_in, v);
    if p.is_finite() && p > 0.0 && cfg.short_leg_nm > 0.0 {
        p + cfg.short_leg_nm * (shortness(leff, l_in) * shortness(leff, l_prev)).sqrt()
    } else {
        p
    }
}

/// first-draw-45 r7 (`[drawing] bundle_pull_range_mm`, lead 2026-11-21): a hug form whose
/// middle leg `p1-p2` lies beside a drawn neighbour pulls the run only where that leg stays within
/// the range of the run's stretch it replaces (`old`): the farthest of 9 points along the leg
/// from `old` is at most the range (always with no range set). O(|old|).
pub fn hug_within_range(cfg: &OctiCfg, old: &[Vec2], p1: Vec2, p2: Vec2) -> bool {
    if !cfg.pull_range.is_finite() {
        return true;
    }
    (0..=8).all(|t| {
        let q = p1 + (p2 - p1) * (t as f64 / 8.0);
        old.windows(2).any(|s| super::draw::seg_dist(q, s[0], s[1]) <= cfg.pull_range)
    })
}

/// first-draw-45 r7 (`[drawing] short_leg_mm`, lead 2026-11-21: graded, no cutoff): the graded
/// shortness of a leg of `l` nm below the jog leg `leff` ([`jog_leg`]): `((leff - l) / (l + leff /
/// 8))^2`. 0 at and above `leff` (and its slope there), rising smoothly as the leg shrinks: 0.64 at
/// `leff / 2`, 64 for a vanishing leg. [`bend_price_seq`] prices a bend after two legs by the
/// weight times the geometric mean of their shortness: a staircase (a run of sub-jog legs: esp
/// +3V3 r6, 11 KiCad items where the DRC then counted one clearance item per step against the
/// IO8_SCL crossing) pays per step, while a single short leg between two long ones (eprom A0:
/// 45 degrees, 0.17 mm, 45 degrees, the shortest form) pays nothing extra: the jog penalty of
/// [`bend_price`] (bounded by `2 bend_nm`) stays its only price. O(1).
pub fn shortness(leff: f64, l: f64) -> f64 {
    if l >= leff || l.is_nan() {
        return 0.0;
    }
    let x = (leff - l.max(0.0)) / (l.max(0.0) + leff / 8.0);
    x * x
}

/// octi-bundle r2 (fuse octilinear, the user's A1c rule: every redraw path prices on the first
/// draw's scale): the style price of the polyline `pts` (nm, without its length) as the
/// octilinear DP prices its legs: every bend at [`bend_price`] with the segment before it, plus
/// `any_nm` per [`any_price`] amount (each any-angle stretch once, 1 plus 2/15 per mm at the
/// default weight 15). Infinite where the style forbids a piece (`any_angle = "forbidden"`, a
/// forbidden acute bend, a reversal). `pitch` = w + s of the wire (the jog leg). O(points).
pub fn style_price(cfg: &OctiCfg, pitch: f64, pts: &[Vec2]) -> f64 {
    let leff = jog_leg(cfg, pitch);
    let segs: Vec<Vec2> = pts.windows(2).map(|p| p[1] - p[0]).filter(|d| d.len2() > 1.0).collect();
    let mut tot = 0.0;
    for (k, s) in segs.windows(2).enumerate() {
        let l_prev = if k > 0 { segs[k - 1].len() } else { f64::INFINITY };
        tot += bend_price_seq(cfg, leff, s[0], s[0].len(), l_prev, s[1]);
    }
    let any = any_price(pts);
    if any > 0.0 {
        if cfg.any_forbidden {
            return f64::INFINITY;
        }
        tot += cfg.any_nm * any;
    }
    tot
}

/// The octilinear links from `p` to `q` (fuse octilinear): the straight segment when `p -> q` is
/// at 0/45/90 degrees, else the two elbows and the two centred Z shapes of the DP's connectors,
/// each as its points after `p` (ending at `q`). Empty for `p == q`. O(1).
pub fn link_shapes(p: Vec2, q: Vec2) -> Vec<Vec<Vec2>> {
    connectors(p, q).into_iter().map(|s| s.into_iter().map(|l| l.0).collect()).collect()
}

/// `acute_bend` amount of a turn of `turn` degrees (> 90): 1, rising to 2 as the interior angle
/// closes from 90 to 0 degrees.
pub fn acute_amount(turn: f64) -> f64 {
    1.0 + ((turn - 90.0) / 90.0).clamp(0.0, 1.0)
}

/// The style breaks of a drawn polyline: `(stretches not at 0/45/90 degrees as (middle, angle
/// deg, length nm), acute bends as (vertex, interior angle deg))`. Consecutive any-angle segments
/// form one stretch. O(points).
#[allow(clippy::type_complexity)]
pub fn breaks(pts: &[Vec2]) -> (Vec<(Vec2, f64, f64)>, Vec<(Vec2, f64)>) {
    let mut any: Vec<(Vec2, f64, f64)> = Vec::new();
    let mut acute = Vec::new();
    let mut open: Option<(Vec2, f64, f64, f64)> = None; // (start, len, angle of the longest piece, its len)
    let segs: Vec<(Vec2, Vec2)> = pts.windows(2).filter(|p| (p[1] - p[0]).len2() > 1.0).map(|p| (p[0], p[1])).collect();
    for (i, &(a, b)) in segs.iter().enumerate() {
        let d = b - a;
        if octi_dir(d).is_none() {
            let l = d.len();
            let ang = d.y.atan2(d.x).to_degrees().rem_euclid(180.0);
            open = Some(match open {
                Some((s, tl, an, al)) => (s, tl + l, if l > al { ang } else { an }, al.max(l)),
                None => (a, l, ang, l),
            });
        } else if let Some((s, tl, an, _)) = open.take() {
            any.push((s + (a - s) * 0.5, an, tl));
        }
        if i + 1 < segs.len() {
            let t = turn_deg(d, segs[i + 1].1 - segs[i + 1].0);
            if t > 90.5 {
                acute.push((b, 180.0 - t));
            }
        }
    }
    if let (Some((s, tl, an, _)), Some(&(_, b))) = (open, segs.last()) {
        any.push((s + (b - s) * 0.5, an, tl));
    }
    (any, acute)
}

/// A connector leg: its end point and its octilinear direction (`None`: the taut segment, any
/// angle).
type Leg = (Vec2, Option<usize>);

#[cfg(test)]
thread_local! {
    /// Test switch: run the DP loop without the shape cache (the reference).
    pub(super) static OCTI_REF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Test coverage: legs with a bundle length > 0, legs with a room price > 0 (cached loop).
    pub(super) static OCTI_HITS: std::cell::Cell<[usize; 2]> = const { std::cell::Cell::new([0, 0]) };
    /// Tests: the lane rule's pair count instead of [`OCTI_LANE_MIN_PAIRS`] (0 = the constant).
    pub(super) static OCTI_LANE_MIN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Tests: nodes with successors done on this thread alone and on the crew (both read on the
    /// thread that called `octilinear`, the pool worker inside a `pool.install`).
    pub(super) static OCTI_NODE_SPLIT: std::cell::Cell<[usize; 2]> = const { std::cell::Cell::new([0, 0]) };
    /// Tests (first-draw-45 P2): the own-crossing portals without their gates (no price).
    pub(super) static OCTI_NO_OWN_GATES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Where the run ends at a pad: the pad's long axis (unit, pointing out of the pad towards the
/// run) and the distance from the pad centre to just past its tip (half its length along the
/// axis plus the clearance and half the width).
#[derive(Clone, Copy, Debug)]
pub struct Exit {
    pub axis: Vec2,
    pub reach: f64,
}

impl Drawer<'_> {
    /// The pad end of run `r` of wire `w` (`end` 0: its start, 1: its end) as an [`Exit`] when the
    /// pad is elongated (long side over 1.2 x the short side), `toward` = the next taut point.
    pub(super) fn pad_exit(&self, w: WireId, end: usize, toward: Vec2) -> Option<Exit> {
        let wi = &self.st.wires[w];
        let ob = if end == 0 { wi.pa } else { wi.pb };
        if ob == NO_OBS || self.end_at.contains_key(&(w, end)) {
            return None;
        }
        let pr = &self.b.proj;
        let o = &pr.obstacles[ob];
        // The run's end (the pad anchor when the centre is illegal, edge-clear): the leg reaches past
        // the tip from there.
        let c = self.run_end(w, end);
        if o.poly.len() < 3 {
            return None;
        }
        // Principal axis of the outline (second moments of its vertices about the centre).
        let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
        for &p in &o.poly {
            let d = p - o.center;
            sxx += d.x * d.x;
            syy += d.y * d.y;
            sxy += d.x * d.y;
        }
        let th = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        let u = Vec2 { x: th.cos(), y: th.sin() };
        let ext = |v: Vec2| o.poly.iter().map(|&p| (p - o.center).dot(v)).fold(f64::MIN, f64::max);
        let (lu, lv) = (ext(u) + ext(-u), ext(u.perp()) + ext(-u.perp()));
        if lu < 1.2 * lv {
            return None;
        }
        let u = if (toward - c).dot(u) >= 0.0 { u } else { -u };
        let reach = ext(u) + (c - o.center).dot(-u) + wi.s + 0.5 * wi.w;
        Some(Exit { axis: u, reach })
    }

    /// pad-snap r2: the first (last) leg of the run `pts` (passes `k0..k1` of `w`) is a stub wholly
    /// inside the wire's own pad at that end, which tidy and KiCad's Cleanup delete (a positive
    /// [`stub_price`]; not at a branch end or a continued lead). The DP reprices an all-octilinear
    /// run with one instead of keeping it as it is.
    pub(super) fn run_pad_stubs(&self, w: WireId, k0: usize, k1: usize, pts: &[Vec2]) -> [bool; 2] {
        let wi = &self.st.wires[w];
        let n = pts.len();
        if n < 3 {
            return [false; 2];
        }
        [0usize, 1].map(|end| {
            let at_end = if end == 0 { k0 == 0 } else { k1 == wi.passes.len() };
            let (c, j) = if end == 0 { (pts[0], pts[1]) } else { (pts[n - 1], pts[n - 2]) };
            self.own_pad_end(w, end, at_end, false, c).is_some_and(|(poly, c, ext)| stub_price(poly, c, ext, wi.w, j, 1.0) > 0.0)
        })
    }

    /// pad-snap: the ONE predicate for "the run end `c` (`end` 0: start, 1: end of wire `w`) is an
    /// own-pad end whose stub is priced": the run really ends there (`at_end`, no continued lead:
    /// neither a kept lead on that pad nor `lead_cont`), the pad is an obstacle and not anchored
    /// elsewhere (`end_at`), and `c` lies in the pad outline. Gives (outline, `c`, the farthest
    /// outline point from `c`). Used by the DP's price and by [`Drawer::run_pad_stubs`].
    pub(super) fn own_pad_end(&self, w: WireId, end: usize, at_end: bool, lead_cont: bool, c: Vec2) -> Option<(&[Vec2], Vec2, f64)> {
        let wi = &self.st.wires[w];
        let ob = if end == 0 { wi.pa } else { wi.pb };
        if !at_end || lead_cont || ob == NO_OBS || ob >= self.b.proj.obstacles.len() || self.end_at.contains_key(&(w, end)) || self.b.kept_ends.iter().any(|k| k.obs == ob && k.lead.is_some()) {
            return None;
        }
        let poly: &[Vec2] = &self.b.proj.obstacles[ob].poly;
        if poly.len() < 3 || !point_in_poly(c, poly) {
            return None;
        }
        Some((poly, c, poly.iter().map(|&q| (q - c).len()).fold(0.0, f64::max)))
    }

    /// The run `pts` (passes `k0..k1` of `w` on `layer`, legal as drawn) redrawn octilinear in its
    /// sleeve (see the module doc). `None` when nothing changes or the run is too short.
    #[allow(clippy::too_many_arguments)]
    /// `forced` (a run drawn by force, never drop; docs/dev/design/forced-draw.md): the clearance
    /// is a price, not a test. Every leg, the taut pieces included, pays the `violation` weight
    /// times the violation amount it has at the full rule ([`Drawer::seg_deficit_parts_near`]),
    /// the amount of a short (copper over foreign copper) at the given price per unit instead
    /// (`Some(short_nm)`), on the DP's one scale (length, bends, `any_angle`, `planned_room`); the
    /// topology tests (region, strip, walls) stay. `None`: a leg must keep every clearance (the
    /// first draw).
    pub(super) fn octilinear(&self, w: WireId, k0: usize, k1: usize, layer: usize, pts: &[Vec2], neck: Option<Neck>, exits: [Option<Exit>; 2], forced: Option<f64>) -> Option<OctiOut> {
        let t = std::time::Instant::now();
        let r = self.octilinear_dp(w, k0, k1, layer, pts, neck, exits, forced);
        // first-draw-45 r9 debug (`TOPO_REDUCEDBG`, copper-neutral): an octilinear call over 0.5 s.
        if std::env::var_os("TOPO_REDUCEDBG").is_some() && t.elapsed().as_secs_f64() > 0.5 {
            eprintln!("OCTISLOW w{w} L{layer} passes {k0}..{k1} taut pts {} forced {} -> {}: {:.2} s", pts.len(), forced.is_some(), r.as_ref().map_or(0, |o| o.pts.len()), t.elapsed().as_secs_f64());
        }
        super::par::meter::time(super::par::meter::OCTI, t);
        super::par::meter::count(super::par::meter::OCTI_CALLS, 1);
        r
    }

    /// Debug (`TOPO_OCTIDBG`, forced-draw): the forced DP's violation amounts `[clearance,
    /// short]` per leg of `pts` (the near set the DP builds for this run). Read only.
    pub(super) fn forced_leg_deficits(&self, w: WireId, layer: usize, pts: &[Vec2], neck: Option<Neck>) -> Vec<[f64; 2]> {
        let wi = &self.st.wires[w];
        let wp = Wp { wire: w, net: wi.net, w: wi.w, s: wi.s, layer, own: [wi.pa, wi.pb], neck, relax: 0.0 };
        let region: Aabb = bbox_of(pts).grow(2.0 * MM + 4.0 * (wi.w + wi.s));
        let nr = self.near(self.local_ents(&wp, region.grow(self.qgrow)), &wp);
        pts.windows(2).map(|x| self.seg_deficit_parts_near(&nr, x[0], x[1])).collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn octilinear_dp(&self, w: WireId, k0: usize, k1: usize, layer: usize, pts: &[Vec2], neck: Option<Neck>, exits: [Option<Exit>; 2], forced: Option<f64>) -> Option<OctiOut> {
        let cfg = self.octi;
        let (short_nm, forced) = (forced.unwrap_or(0.0), forced.is_some());
        let mut taut: Vec<Vec2> = pts.to_vec();
        taut.dedup_by(|a, b| (*a - *b).len2() < 1.0);
        if taut.len() < 2 {
            return None;
        }
        // pad-snap r2b (D3b): an all-octilinear taut run is kept as it is, unless its first (last)
        // leg is a stub wholly inside its own pad (tidy and Cleanup delete it): the DP reprices it.
        if taut.windows(2).all(|p| octi_dir(p[1] - p[0]).is_some()) && exits.iter().all(|e| e.is_none()) && !self.run_pad_stubs(w, k0, k1, &taut).iter().any(|&x| x) {
            return None;
        }
        let wi = &self.st.wires[w];
        let wp = Wp { wire: w, net: wi.net, w: wi.w, s: wi.s, layer, own: [wi.pa, wi.pb], neck, relax: 0.0 };
        let (mut strip, own_gates) = self.open_strip(w, k0, k1, layer, &taut);
        #[cfg(test)]
        let own_gates = if OCTI_NO_OWN_GATES.with(|f| f.get()) { Vec::new() } else { own_gates };
        strip.walls.extend(self.via_walls(w, k0, k1, layer));
        // The wire's own lane on every edge it crosses (Bast et al.: the grid restricted to the
        // line's corridor): the parts of the edge outside the span its packing leaves it are
        // walls, so the octilinear shape takes no room a wire not drawn yet needs to fit. That
        // span is the tight span (the drawn neighbours plus the spacing of every undrawn pass
        // between, or the lane end less it), widened to where the taut run crosses; the S2
        // interval only where there is no lane. The planned room of an undrawn neighbour beyond
        // that is not a wall but a price (`planned_room`, below).
        //
        // Per pass, towards a lane neighbour not drawn yet: the bound the plan keeps for it, its
        // planned (taut) crossing less the lane spacing between the two. Two consecutive bounds
        // on one side are joined across their triangle: the strip beyond that line is the
        // neighbour's planned room inside the triangle (wiccon 3.3v J1.1 -> U1.3: the ADC trace
        // drawn first bulged into it, GPIO16 then followed ADC at the clearance, and 3.3v, drawn
        // last, had no legal geometry left). A leg pays for its length in that strip, graded by
        // depth (Bast, Brosi & Storandt 2020 section 4.3, p. 7: keep room for the lines routed
        // later; Dayan 1997 pp. 15-16: spacing for every wire).
        // (a, b, sign): the room lies where `sign * (b - a) x (p - a) > 0`.
        let mut zones: Vec<(Vec2, Vec2, f64)> = Vec::new();
        // P6 (b): per crossed edge (first end, unit direction, length, span lo, span hi): the
        // lane slot outside `lo..hi` is priced (see below).
        let mut gates: Vec<(Vec2, Vec2, f64, f64, f64)> = own_gates;
        let lane_walls = if fd45_off("lane") { Vec::new() } else { self.lane_order_walls(w, k0, k1, &taut) };
        {
            let (st, pr) = (self.st, &self.b.proj);
            let mut guard: Vec<(usize, [Option<Vec2>; 2])> = Vec::new();
            let dbg = std::env::var("TOPO_LANEDBG").is_ok();
            for k in k0..k1 {
                let pid = wi.passes[k];
                let [p, q] = pr.edges[st.passes[pid].edge];
                let (pp, pq) = (pr.pos[p], pr.pos[q]);
                let len = (pq - pp).len();
                if len < 1.0 {
                    continue;
                }
                let u = (pq - pp) * (1.0 / len);
                let (mut lo, mut hi) = (self.iv.lo.get(pid).copied().unwrap_or(f64::NAN) * len, self.iv.hi.get(pid).copied().unwrap_or(f64::NAN) * len);
                if !lo.is_finite() || !hi.is_finite() {
                    continue;
                }
                if let Some((a, b)) = self.tight_span(pid) {
                    (lo, hi) = (a, b);
                }
                let nom = self.iv.nom.get(pid).copied().filter(|x| x.is_finite()).unwrap_or(0.5) * len;
                if let Some(x) = super::draw::crossing(&taut, pp, pq, nom) {
                    lo = lo.min(x - 1.0);
                    hi = hi.max(x + 1.0);
                }
                let (lo, hi) = (lo.max(0.0), hi.min(len));
                // Only towards a neighbour pass of the same edge and layer: on a side without
                // one the room is the obstacle's, which the clearance check guards.
                let (before, after) = self.lane_sides(pid);
                // P6 (b): the lane slot outside the span is a graded price, not a wall (walls
                // stay for drawn copper, fixed obstacles and the undrawn wires' via chords): a
                // leg crossing the edge `depth` beyond the span pays `room x pitch x (x + x^2)`,
                // `x = depth / pitch` (Bast, Brosi & Storandt 2020 section 4.3, p. 7: room for
                // the lines routed later as a cost). M6 (2026-11-18) removed the walls of before
                // (`[drawing] lane_walls = "hard"`).
                gates.push((pp, u, len, if before && lo > 1.0 { lo } else { f64::NEG_INFINITY }, if after && hi < len - 1.0 { hi } else { f64::INFINITY }));
                // Which end of the edge lies on the wire's left, coming from the triangle before
                // the pass.
                let t = pr.tris[wi.tris[k]];
                let c = (pr.pos[t[0]] + pr.pos[t[1]] + pr.pos[t[2]]) * (1.0 / 3.0);
                let mid = (pp + pq) * 0.5;
                let p_left = (mid - c).cross(pp - mid) > 0.0;
                let (prev_drawn, next_drawn) = self.lane_next_drawn(pid);
                let g_lo = (before && !prev_drawn).then(|| self.neighbour_bound(pid, false)).flatten().map(|x| pp + u * x.clamp(0.0, len));
                let g_hi = (after && !next_drawn).then(|| self.neighbour_bound(pid, true)).flatten().map(|x| pp + u * x.clamp(0.0, len));
                if dbg {
                    eprintln!(
                        "LANE wire {w} k {k} edge {} ({:.3},{:.3})-({:.3},{:.3}) len {:.3} iv {:.3}..{:.3} tight {:?} lohi {:.3}..{:.3} room {:?} {:?} lane {}",
                        st.passes[pid].edge,
                        pp.x / 1e6,
                        -pp.y / 1e6,
                        pq.x / 1e6,
                        -pq.y / 1e6,
                        len / 1e6,
                        self.iv.lo[pid] * len / 1e6,
                        self.iv.hi[pid] * len / 1e6,
                        self.tight_span(pid).map(|(a, b)| (a / 1e6, b / 1e6)),
                        lo / 1e6,
                        hi / 1e6,
                        self.neighbour_bound(pid, false).map(|x| x / 1e6),
                        self.neighbour_bound(pid, true).map(|x| x / 1e6),
                        self.lane_dbg(pid)
                    );
                }
                guard.push((k, if p_left { [g_lo, g_hi] } else { [g_hi, g_lo] }));
            }
            for g in guard.windows(2) {
                let ((ka, sa), (kb, sb)) = (g[0], g[1]);
                if kb != ka + 1 {
                    continue;
                }
                for (side, sign) in [(0, 1.0), (1, -1.0)] {
                    if let (Some(a), Some(b)) = (sa[side], sb[side]) {
                        if (b - a).len2() >= 1.0 {
                            zones.push((a, b, sign));
                        }
                    }
                }
            }
        }
        strip.walls.extend(lane_walls);
        let pitch = wi.w + wi.s;
        let region: Aabb = bbox_of(&taut).grow(2.0 * MM + 4.0 * pitch);
        let cors = if fd45_off("loser") { Vec::new() } else { self.loser_corridors(w, layer, &taut, &region) };
        let ents = self.local_ents(&wp, region.grow(self.qgrow));
        let nr = self.near(ents.clone(), &wp);
        // Cumulative length of the taut path, for the node order.
        let mut cum = vec![0.0];
        for p in taut.windows(2) {
            cum.push(cum.last().unwrap() + (p[1] - p[0]).len());
        }
        // Distance to the taut path, order along it, and the nearest point on it.
        let proj = |p: Vec2| -> (f64, f64, Vec2) {
            let mut best = (f64::INFINITY, 0.0, p);
            for (i, s) in taut.windows(2).enumerate() {
                let d = s[1] - s[0];
                let l2 = d.len2();
                let u = if l2 > 0.0 { ((p - s[0]).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
                let f = s[0] + d * u;
                let dist = (f - p).len();
                if dist < best.0 {
                    best = (dist, cum[i] + u * l2.sqrt(), f);
                }
            }
            best
        };
        // exist-term r4 (the r3 I2C_SDA fold-back): a run that starts at a continued dangling
        // end's lead end goes on from the hand track, so it arrives there along the lead: a first
        // leg back along it is a full reversal (never), a sharp turn pays the bend and
        // `acute_bend` prices like any other. Likewise a run that ends at a lead end goes on into
        // the lead. (Both ends of the hand copper are one wire: Popov & Popov 2012 pp. 117-122,
        // the wire's length and bends count from its terminal, here the hand track's end.)
        let lead_dir = |end: usize, p: Vec2| -> Option<Vec2> {
            let ob = if end == 0 { wi.pa } else { wi.pb };
            let k = self.b.kept_ends.iter().find(|k| k.obs == ob && ob != NO_OBS)?;
            let (q, d) = k.lead?;
            if (p - q).len() < 1.0 {
                (q - k.at).unit()
            } else if !k.choices.is_empty() && k.lead_point_t(&self.b.proj, p).is_some() {
                // tw4 T3: a run from a start point short of the lead's end goes on from the lead
                Some(d)
            } else {
                None
            }
        };
        let n_pass = wi.passes.len();
        let lead_in = if k0 == 0 { lead_dir(0, taut[0]) } else { None };
        let lead_out = if k1 == n_pass { lead_dir(1, taut[taut.len() - 1]) } else { None };
        // pad-snap (A1b): the run's own pad at each end it starts / ends on a pad (not on existing
        // copper, not from a continued lead): (outline, the run's end point, farthest outline
        // point from it). A first (last) leg that leaves a stub wholly inside it pays
        // [`stub_price`].
        let own_pad: [Option<(&[Vec2], Vec2, f64)>; 2] = [0usize, 1].map(|end| {
            let (at_end, lead_cont) = if end == 0 { (k0 == 0, lead_in.is_some()) } else { (k1 == n_pass, lead_out.is_some()) };
            let c = if end == 0 { taut[0] } else { taut[taut.len() - 1] };
            self.own_pad_end(w, end, at_end, lead_cont, c)
        });
        // Nodes: (point, order, taut vertex index, on the taut path). A point on the taut path
        // (its vertices, and the foot of every octagon node) may continue along it at any angle
        // to the next such point: where 45 degrees does not fit, only that piece stays any-angle.
        let mut nodes: Vec<(Vec2, f64, Option<usize>, bool)> = taut.iter().enumerate().map(|(i, &p)| (p, cum[i], Some(i), true)).collect();
        // first-draw-45 P4, S-routes (Staepelaere 1992, PDF pp. 18-19): a long taut segment that is
        // not octilinear gets nodes along it, at least a pitch apart (at most CHORD_NODES per
        // segment), so the DP can build its octilinear staircase close to the chord: steps between
        // chord nodes, one-sided where the neighbours leave room on one side only, as long as the
        // bend and jog prices allow (a longer step is cheaper wherever it is legal). Nodes on the
        // taut path: a piece between two of them may stay as drawn (P1).
        if !fd45_off("p4") {
            for (i, sg) in taut.windows(2).enumerate() {
                let d = sg[1] - sg[0];
                let l = d.len();
                if octi_dir(d).is_some() || l < 2.0 * pitch_of(wi) {
                    continue;
                }
                // r7: at least the jog leg apart (a step between neighbouring chord nodes then
                // need not be a sub-jog leg; a thick wire keeps its pitch).
                let step = pitch_of(wi).max(jog_leg(&cfg, pitch_of(wi))).max(l / CHORD_NODES as f64);
                let m = (l / step).floor() as usize;
                for j in 1..m {
                    let f = j as f64 * step / l;
                    if f >= 1.0 - 1e-9 {
                        break;
                    }
                    // fd45 r10 (pad-snap): no chord node where a stub from the own pad's centre
                    // would lie wholly inside the pad (pad-snap's one predicate, `own_pad_end` +
                    // `stub_price`, at base 1: whatever the bend price). The run leaves its pad
                    // by the leave shapes (`leave8`); a chord node there gave the DP an in-pad
                    // joint that tidy and KiCad's Cleanup delete, an end off its centre (bend_mm
                    // 0 prices the stub at 0; topo_pad_snap side_exit).
                    let q = sg[0] + d * f;
                    if own_pad.iter().flatten().any(|&(poly, c, ext)| stub_price(poly, c, ext, wi.w, q, 1.0) > 0.0) {
                        continue;
                    }
                    nodes.push((q, cum[i] + f * l, None, true));
                }
            }
        }
        // An octagon node matters where the taut path wraps its corner (within a fraction of the
        // corner's clearance radius of it) or runs next to it.
        let near = 0.5 * pitch;
        let mut extra: Vec<(f64, Vec2, f64, Vec2)> = Vec::new();
        let mut cand = Vec::new();
        for &id in &ents {
            cand.clear();
            self.octagon_nodes(id, &wp, &region, &mut cand);
            // A drawn wire's corners further out too: the run may follow it (bundles), up to 2
            // pitches past the clearance, and (r9, `[drawing] bundle_pull_range_mm`) no further
            // from the taut path than the range (never below the reach of any other corner).
            let need = self.need_of(id, &wp, wp.w);
            let reach = if id >= self.n_static && cfg.pull > 0.0 { (need + 2.0 * pitch).min(cfg.pull_range.max(near + 0.15 * need)) } else { near + 0.15 * need };
            for &p in &cand {
                let (d, t, f) = proj(p);
                if d <= reach && t > 0.0 && t < *cum.last().unwrap() {
                    extra.push((d, p, t, f));
                }
            }
        }
        extra.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.total_cmp(&b.2)));
        let foot_gap = 0.25 * pitch;
        for (_, p, t, f) in extra {
            if nodes.len() >= MAX_NODES {
                break;
            }
            if nodes.iter().any(|n| (n.0 - p).len2() < 100.0) {
                continue;
            }
            if strip.contains(p) && self.seg_clear_near(&nr, p, p) {
                nodes.push((p, t, None, false));
                if !nodes.iter().any(|n| n.3 && (n.1 - t).abs() < foot_gap) {
                    nodes.push((f, t, None, true));
                }
            }
        }
        nodes.sort_by(|a, b| a.1.total_cmp(&b.1).then(b.2.is_some().cmp(&a.2.is_some())).then(b.3.cmp(&a.3)));
        // The two ends stay first and last.
        let n = nodes.len();
        let last_t = nodes.iter().position(|x| x.2 == Some(taut.len() - 1)).unwrap_or(n - 1);
        if last_t != n - 1 {
            let e = nodes.remove(last_t);
            nodes.push(e);
        }
        let first_t = nodes.iter().position(|x| x.2 == Some(0)).unwrap_or(0);
        if first_t != 0 {
            let s = nodes.remove(first_t);
            nodes.insert(0, s);
        }
        // The next node on the taut path after each node on it.
        let next_on: Vec<Option<usize>> = (0..n).map(|i| if nodes[i].3 { (i + 1..n).find(|&j| nodes[j].3) } else { None }).collect();
        let leff = jog_leg(&cfg, pitch);
        // Drawn octilinear segments of other nets near the run, with the centre distance at which
        // this wire would run beside them at the clearance (bundle following).
        let side: Vec<(Vec2, Vec2, usize, f64)> = if cfg.pull > 0.0 {
            ents.iter()
                .filter(|&&id| id >= self.n_static && self.ents[id].net != Some(wp.net))
                .filter_map(|&id| match self.ents[id].core {
                    Core::Seg(a, b) => octi_dir(b - a).map(|k| (a, b, k % 4, self.need_of(id, &wp, wp.w))),
                    _ => None,
                })
                .collect()
        } else {
            Vec::new()
        };
        let tol = 0.05 * pitch + 1_000.0;
        // The leg prices and the legality below are pure functions of the leg (the board, the
        // strip and the neighbours are fixed during the call); [`Memo`] memoises them per leg
        // (par-draw: a lane reads the call's memo and writes its own overlay, merged after the
        // node; the values do not depend on who computed them).
        // Length of the leg `a-b` (direction `k`) that runs beside a drawn neighbour at its
        // clearance (the overlap of their projections). The scan of every neighbour: the
        // reference of `follow_x` below (tests, `OCTI_REF`).
        let follow_f = |a: Vec2, b: Vec2, k: usize| -> f64 {
            let u = dir(k);
            let (t0, t1) = (0.0f64, (b - a).dot(u));
            let mut best = 0.0f64;
            for &(c, e, kk, need) in &side {
                if kk != k % 4 {
                    continue;
                }
                let off = u.cross(c - a).abs();
                if (off - need).abs() > tol {
                    continue;
                }
                let (s0, s1) = ((c - a).dot(u), (e - a).dot(u));
                let ov = (t1.min(s0.max(s1)) - t0.max(s0.min(s1))).max(0.0);
                best = best.max(ov);
            }
            best
        };
        // `planned_room` price of the leg `a-b`: its length inside the planned room of an undrawn
        // lane neighbour (`zones`), each piece weighted by `x + x^2`, `x` = depth over the pitch
        // (midpoint rule, pieces of at most a quarter pitch or 1/24 of the leg).
        let room_none = (zones.is_empty() && gates.is_empty() && cors.is_empty()) || cfg.room <= 0.0;
        let room_f = |a: Vec2, b: Vec2| -> f64 {
            let l = (b - a).len();
            let mut tot = 0.0;
            if l >= 1.0 {
                for &(za, zb, sign) in &zones {
                    let zl = (zb - za).len();
                    let zu = (zb - za) * (1.0 / zl);
                    let d0 = sign * zu.cross(a - za);
                    let d1 = sign * zu.cross(b - za);
                    if d0 <= 0.0 && d1 <= 0.0 {
                        continue;
                    }
                    let (t0, t1) = ((a - za).dot(zu), (b - za).dot(zu));
                    if (t0 < 0.0 && t1 < 0.0) || (t0 > zl && t1 > zl) {
                        continue;
                    }
                    let n = ((l / (0.25 * pitch)).ceil() as usize).clamp(2, 24);
                    let ds = l / n as f64;
                    for i in 0..n {
                        let f = (i as f64 + 0.5) / n as f64;
                        let (d, t) = (d0 + (d1 - d0) * f, t0 + (t1 - t0) * f);
                        if d > 0.0 && (0.0..=zl).contains(&t) {
                            let x = d / pitch;
                            tot += ds * (x + x * x);
                        }
                    }
                }
            }
            tot += corridor_room(&cors, pitch, a, b);
            // The lane slots beyond the span (P6 b): the depth of the crossing past it.
            for &(gp, gu, glen, glo, ghi) in &gates {
                let gq = gp + gu * glen;
                if !super::draw::seg_proper(a, b, gp, gq) {
                    continue;
                }
                let (d1, d2) = (b - a, gq - gp);
                let den = d1.cross(d2);
                if den.abs() < 1e-9 {
                    continue;
                }
                let t = (gp - a).cross(d1) / den * glen;
                let depth = (glo - t).max(t - ghi).max(0.0);
                if depth > 0.0 {
                    let x = depth / pitch;
                    tot += pitch * (x + x * x);
                }
            }
            cfg.room * tot
        };
        // The same two prices with the per-leg work cut, exactly (see `RoomPre`, `FollowIdx`);
        // the cached loop's (the scans above are the reference loop's, `OCTI_REF`).
        let rpre = RoomPre::new(&zones, &gates, &cors);
        let fidx = FollowIdx::new(&side);
        let follow_x = |a: Vec2, b: Vec2, k: usize| -> f64 {
            let best = fidx.follow(&side, tol, a, b, k);
            #[cfg(test)]
            OCTI_HITS.with(|h| h.set([h.get()[0] + usize::from(best > 0.0), h.get()[1]]));
            best
        };
        let room_x = |a: Vec2, b: Vec2| -> f64 {
            let v = cfg.room * rpre.room(pitch, a, b);
            #[cfg(test)]
            OCTI_HITS.with(|h| h.set([h.get()[0], h.get()[1] + usize::from(v > 0.0)]));
            v
        };
        let legal_f = |a: Vec2, b: Vec2| -> bool {
            let bb = bbox_of(&[a, b]);
            bb.min.x >= region.min.x
                && bb.min.y >= region.min.y
                && bb.max.x <= region.max.x
                && bb.max.y <= region.max.y
                && strip.contains(b)
                && strip.contains((a + b) * 0.5)
                && !strip.crosses_wall(a, b)
                && (forced || self.seg_clear_near(&nr, a, b))
        };
        let stub_nm = cfg.stub_nm(pitch);
        let stub = |end: usize, j: Vec2| own_pad[end].map_or(0.0, |(poly, c, ext)| stub_price(poly, c, ext, wi.w, j, stub_nm));
        // pad-snap r2b (D3b): per own-pad end and octilinear direction `k`, the leave point: the
        // nearest point on the ray from the run end whose round end sticks out of the pad by
        // [`LEAVE_NM`] ([`leave_point`]). Every own pad, also one too short for an [`Exit`].
        let leave8: [[Option<Vec2>; 8]; 2] = [0usize, 1].map(|end| match own_pad[end] {
            Some((poly, c, _)) => std::array::from_fn(|k| leave_point(poly, c, dir(k), wi.w)),
            None => [None; 8],
        });
        // forced-draw: the violation price of the leg `a-b` (0 in the first draw, where a leg keeps
        // every clearance or is not taken): a short (copper over foreign copper) at `short_nm`,
        // the rest at `viol_nm`.
        let viol_x = |a: Vec2, b: Vec2| -> f64 {
            if !forced {
                return 0.0;
            }
            let [c, sh] = self.seg_deficit_parts_near(&nr, a, b);
            cfg.viol_nm * c + short_nm * sh
        };
        // first-draw-45 P1: the price of a piece of the taut path taken as drawn (no leg test,
        // see the shapes below) in the first draw: its clearance below the rule at the full rule,
        // a short at `short_nm`, the rest at `viol_nm` ([`Drawer::seg_deficit_parts_near`]), the
        // forced DP's scale. Costly, not failed: a legal connector wins wherever there is one,
        // the piece stays where there is none, counted (`OctiOut::asis`). In a forced run every
        // leg already pays `viol_x`.
        let p1 = !fd45_off("p1");
        let asis_x = |a: Vec2, b: Vec2| -> f64 {
            if forced || !p1 {
                return 0.0;
            }
            let [c, sh] = self.seg_deficit_parts_near(&nr, a, b);
            cfg.viol_nm * c + cfg.short_nm * sh
        };
        let mut memo = Memo::default();
        // DP state per node: arrival direction 0..7, or 8 (the start / an any-angle leg).
        const ND: usize = 9;
        let mut dist = vec![[f64::INFINITY; ND]; n];
        // (previous node, previous state, shape id, legs)
        #[allow(clippy::type_complexity)]
        let mut back: Vec<[Option<(usize, usize, Vec<Leg>)>; ND]> = vec![std::array::from_fn(|_| None); n];
        // Last leg of the state: unit direction and length.
        let mut last: Vec<[(Vec2, f64, f64); ND]> = vec![[(Vec2::default(), f64::INFINITY, f64::INFINITY); ND]; n];
        dist[0][8] = 0.0;
        if let Some(u) = lead_in {
            last[0][8] = (u, f64::INFINITY, f64::INFINITY);
        }
        let bend = |u: Vec2, l_in: f64, l_prev: f64, v: Vec2| -> f64 { bend_price_seq(&cfg, leff, u, l_in, l_prev, v) };
        // Price of leaving / entering a pad off its axis (up to two bends' worth at 90 degrees),
        // or along it but turning before the pad's tip is cleared (graded like the jog penalty):
        // pad exit along the pad axis until clear of the neighbour pads.
        let axis_price = |e: &Option<Exit>, d: Vec2, len: f64| -> f64 {
            match e {
                Some(x) => {
                    let t = turn_deg(x.axis, d);
                    let off = t.min(180.0 - t);
                    if off < 0.5 {
                        2.0 * cfg.bend_nm * (1.0 - len / x.reach.max(1.0)).max(0.0).powi(2)
                    } else {
                        cfg.bend_nm * 2.0 * (off / 45.0)
                    }
                }
                None => 0.0,
            }
        };
        // The pad-exit shapes of the node pair `i`-`j` (`p`-`q`). Mode 0: an axis leg out of the
        // start pad to past its tip (`Exit::reach`), then a connector (`i` = 0), and a connector,
        // then an axis leg into the end pad (`j` = the last node). Mode 3 (pad-snap, A1b/D3b): from
        // every own-pad end, a leg in each of the 8 octilinear directions to its leave point
        // ([`leave_point`]: the round end sticks out of the pad by [`LEAVE_NM`]), then a connector,
        // where the run goes on forward from there: the axis leg to the leave point (r1) and the 45
        // degree exit to the pad side (r2) are two of them, so a run whose next node is short of
        // the full reach, or a pad too short for an [`Exit`], still leaves its pad centred.
        let exit_shapes = |shapes: &mut Vec<Vec<Leg>>, i: usize, j: usize, p: Vec2, q: Vec2, mode: u8| {
            // Mode 3 (pad-snap r2b, D3b): from an own-pad end, a leg in each octilinear direction to
            // its leave point, then a connector, where the run goes on forward from there.
            if mode == 3 {
                for (end, ls) in leave8.iter().enumerate() {
                    if (end == 0 && i != 0) || (end == 1 && j != n - 1) {
                        continue;
                    }
                    let (from, to) = if end == 0 { (p, q) } else { (q, p) };
                    for (k, m) in ls.iter().enumerate() {
                        let Some(m) = *m else { continue };
                        let d = m - from;
                        if (to - m).dot(d) <= 0.0 || (to - m).len2() < 1.0 {
                            continue;
                        }
                        for mut s in if end == 0 { connectors(m, to) } else { connectors(to, m) } {
                            if end == 0 {
                                s.insert(0, (m, Some(k)));
                            } else {
                                s.push((from, Some((k + 4) % 8)));
                            }
                            shapes.push(s);
                        }
                    }
                }
                return;
            }
            if i == 0 {
                if let Some(t) = exits[0].as_ref().map(|x| (x.axis, x.reach)).filter(|&(u, t)| (q - p).dot(u) > t) {
                    let p2 = p + t.0 * t.1;
                    for mut s in connectors(p2, q) {
                        s.insert(0, (p2, octi_dir(t.0)));
                        shapes.push(s);
                    }
                }
            }
            if j == n - 1 {
                if let Some(t) = exits[1].as_ref().map(|x| (x.axis, x.reach)).filter(|&(u, t)| (p - q).dot(u) > t) {
                    let q2 = q + t.0 * t.1;
                    for mut s in connectors(p, q2) {
                        s.push((q, octi_dir(-t.0)));
                        shapes.push(s);
                    }
                }
            }
        };
        // The connector shapes of a node pair, their legs' lengths, directions, `planned_room`
        // and bundle prices and their legality do not depend on the arrival state: built once
        // per pair (on its first use) and shared by the states of node `i`, in the same order.
        struct Shape {
            s: Vec<Leg>,
            // (leg index, length, unit direction, room price, follow length, direction)
            legs: Vec<(usize, f64, Vec2, f64, f64, Option<usize>)>,
            legal: Option<bool>,
        }
        let pull_on = cfg.pull > 0.0 && !side.is_empty();
        // Tests: the loop before the shape cache (the reference the cached loop must equal).
        #[cfg(test)]
        let reference = OCTI_REF.with(|f| f.get());
        #[cfg(not(test))]
        let reference = false;
        if reference {
            let mut room = |a: Vec2, b: Vec2| if room_none { 0.0 } else { memo_get(None, &mut memo.room, leg_key(a, b), || room_f(a, b)) };
            let mut fmemo: FxHashMap<[u64; 4], f64> = crate::fxhash::map();
            let mut follow = |a: Vec2, b: Vec2, k: usize| memo_get(None, &mut fmemo, leg_key(a, b), || follow_f(a, b, k));
            let mut lmemo: FxHashMap<[u64; 4], bool> = crate::fxhash::map();
            let mut vmemo: FxHashMap<[u64; 4], f64> = crate::fxhash::map();
            let mut viol = |a: Vec2, b: Vec2| memo_get(None, &mut vmemo, leg_key(a, b), || viol_x(a, b));
            let mut legal = |a: Vec2, b: Vec2| memo_get(None, &mut lmemo, leg_key(a, b), || legal_f(a, b));
            for i in 0..n {
                for din in 0..ND {
                    let d0 = dist[i][din];
                    if !d0.is_finite() {
                        continue;
                    }
                    let (u_in, l_in, lp_in) = last[i][din];
                    let mut js: Vec<usize> = (i + 1..n.min(i + 1 + WINDOW_MAX)).take_while(|&j| j < i + 1 + WINDOW || nodes[j].1 - nodes[i].1 <= REACH_MM * MM).collect();
                    if let Some(j) = next_on[i] {
                        if !js.contains(&j) {
                            js.push(j);
                        }
                    }
                    if !js.contains(&(n - 1)) && n - 1 > i {
                        js.push(n - 1);
                    }
                    for &j in &js {
                        let (p, q) = (nodes[i].0, nodes[j].0);
                        let mut shapes: Vec<Vec<Leg>> = connectors(p, q);
                        exit_shapes(&mut shapes, i, j, p, q, 0);
                        // A piece of the taut path itself (any angle), between consecutive nodes on it.
                        let taut_from = shapes.len();
                        if next_on[i] == Some(j) && octi_dir(q - p).is_none() && !cfg.any_forbidden {
                            shapes.push(vec![(q, None)]);
                        }
                        let taut_to = shapes.len();
                        exit_shapes(&mut shapes, i, j, p, q, 3);
                        // first-draw-45 P1: an octilinear piece of the taut path as drawn too.
                        let oct_from = shapes.len();
                        if p1 && next_on[i] == Some(j) && octi_dir(q - p).is_some() {
                            shapes.push(vec![(q, octi_dir(q - p))]);
                        }
                        for (si, s) in shapes.into_iter().enumerate() {
                            let asis = (taut_from..taut_to).contains(&si) || si >= oct_from;
                            // Price.
                            let mut c = 0.0;
                            let (mut u, mut l, mut lp, mut from) = (u_in, l_in, lp_in, p);
                            // One any-angle stretch pays its per-stretch price once, however many
                            // pieces of the taut path it is made of.
                            let mut prev_any = din == 8 && i != 0;
                            let mut ok = true;
                            for (k, &(e, dk)) in s.iter().enumerate() {
                                let d = e - from;
                                let len = d.len();
                                if len < 1.0 {
                                    continue;
                                }
                                let v = d * (1.0 / len);
                                let b = bend(u, l, lp, v);
                                if !b.is_finite() {
                                    ok = false;
                                    break;
                                }
                                c += len + b + room(from, e) + viol(from, e);
                                if asis {
                                    c += asis_x(from, e);
                                }
                                if let (Some(kk), true) = (dk, cfg.pull > 0.0 && !side.is_empty()) {
                                    c -= cfg.pull * follow(from, e, kk);
                                }
                                if dk.is_none() {
                                    c += cfg.any_nm * (any_angle_amount(len) - if prev_any { 1.0 } else { 0.0 });
                                }
                                prev_any = dk.is_none();
                                if i == 0 && k == 0 {
                                    c += axis_price(&exits[0], v, len) + stub(0, e);
                                }
                                if j == n - 1 && k == s.len() - 1 {
                                    c += axis_price(&exits[1], v, len) + stub(1, if k == 0 { p } else { s[k - 1].0 });
                                    if let Some(ul) = lead_out {
                                        // on into the lead (from its end back to the hand track)
                                        c += bend(v, len, l, -ul);
                                    }
                                }
                                lp = l;
                                u = v;
                                l = len;
                                from = e;
                            }
                            if !ok {
                                continue;
                            }
                            let dout = s.last().and_then(|x| x.1).unwrap_or(8);
                            if d0 + c >= dist[j][dout] {
                                continue;
                            }
                            // Legality (a piece of the taut path is legal as drawn).
                            let mut from = p;
                            let mut leg_ok = true;
                            for &(e, dk) in &s {
                                if !asis && (e - from).len2() >= 1.0 && dk.is_some() && !legal(from, e) {
                                    leg_ok = false;
                                    break;
                                }
                                from = e;
                            }
                            if !leg_ok {
                                continue;
                            }
                            dist[j][dout] = d0 + c;
                            last[j][dout] = (u, l, lp);
                            back[j][dout] = Some((i, din, s));
                        }
                    }
                }
            }
        } else {
            // One successor `j` of node `i`: its shapes (built once, shared by the arrival
            // states), then for every finite arrival state of `i` in order the relaxation of
            // `dist[j]` as the serial loop did it (the prune against `j`'s row as updated so far,
            // the lazy legality). It reads only `i`'s row (final: every edge goes from `i` to a
            // later node) and its own copy of `j`'s row, so the successors of one node are
            // independent units (par-draw, docs/dev/design/par-draw.md section 13); it returns
            // the states of `j` it improved, (state, price, last leg, back pointer).
            #[allow(clippy::type_complexity)]
            let relax = |i: usize, j: usize, fin: &[(usize, f64, Vec2, f64, f64)], dj: [f64; ND], shared: Option<&Memo>, own: &mut Memo| -> Vec<(usize, f64, (Vec2, f64, f64), (usize, usize, Vec<Leg>))> {
                let (p, q) = (nodes[i].0, nodes[j].0);
                let (sh_room, sh_follow, sh_legal, sh_viol) = (shared.map(|m| &m.room), shared.map(|m| &m.follow), shared.map(|m| &m.legal), shared.map(|m| &m.viol));
                let (own_room, own_follow, own_legal, own_viol) = (&mut own.room, &mut own.follow, &mut own.legal, &mut own.viol);
                let mut shapes: Vec<Vec<Leg>> = connectors(p, q);
                exit_shapes(&mut shapes, i, j, p, q, 0);
                // A piece of the taut path itself (any angle), between consecutive nodes on it.
                let taut_from = shapes.len();
                if next_on[i] == Some(j) && octi_dir(q - p).is_none() && !cfg.any_forbidden {
                    shapes.push(vec![(q, None)]);
                }
                let taut_to = shapes.len();
                exit_shapes(&mut shapes, i, j, p, q, 3);
                // first-draw-45 P1: an octilinear piece of the taut path as drawn too (legal as
                // drawn like the any-angle one, priced by `asis_x`).
                let oct_from = shapes.len();
                if p1 && next_on[i] == Some(j) && octi_dir(q - p).is_some() {
                    shapes.push(vec![(q, octi_dir(q - p))]);
                }
                let mut shapes: Vec<Shape> = shapes
                    .into_iter()
                    .enumerate()
                    .map(|(si, s)| {
                        let asis = (taut_from..taut_to).contains(&si) || si >= oct_from;
                        let mut legs = Vec::with_capacity(s.len());
                        let mut from = p;
                        for (k, &(e, dk)) in s.iter().enumerate() {
                            let d = e - from;
                            let len = d.len();
                            if len < 1.0 {
                                continue;
                            }
                            let v = d * (1.0 / len);
                            let rv = if room_none { 0.0 } else { memo_get(sh_room, own_room, leg_key(from, e), || room_x(from, e)) };
                            let rv = if forced { rv + memo_get(sh_viol, own_viol, leg_key(from, e), || viol_x(from, e)) } else { rv };
                            let rv = if asis { rv + asis_x(from, e) } else { rv };
                            let fv = match (dk, pull_on) {
                                (Some(kk), true) => memo_get(sh_follow, own_follow, leg_key(from, e), || follow_x(from, e, kk)),
                                _ => 0.0,
                            };
                            legs.push((k, len, v, rv, fv, dk));
                            from = e;
                        }
                        Shape { s, legs, legal: if asis { Some(true) } else { None } }
                    })
                    .collect();
                let mut dj = dj;
                let mut best: [Option<((Vec2, f64, f64), usize, usize)>; ND] = [None; ND];
                for &(din, d0, u_in, l_in, lp_in) in fin {
                    for (si, sh) in shapes.iter_mut().enumerate() {
                        // Price.
                        let mut c = 0.0;
                        let (mut u, mut l, mut lp) = (u_in, l_in, lp_in);
                        // One any-angle stretch pays its per-stretch price once, however many
                        // pieces of the taut path it is made of.
                        let mut prev_any = din == 8 && i != 0;
                        let mut ok = true;
                        let sn = sh.s.len();
                        for &(k, len, v, rv, fv, dk) in &sh.legs {
                            let b = bend(u, l, lp, v);
                            if !b.is_finite() {
                                ok = false;
                                break;
                            }
                            c += len + b + rv;
                            if let (Some(_), true) = (dk, pull_on) {
                                c -= cfg.pull * fv;
                            }
                            if dk.is_none() {
                                c += cfg.any_nm * (any_angle_amount(len) - if prev_any { 1.0 } else { 0.0 });
                            }
                            prev_any = dk.is_none();
                            if i == 0 && k == 0 {
                                c += axis_price(&exits[0], v, len) + stub(0, sh.s[0].0);
                            }
                            if j == n - 1 && k == sn - 1 {
                                c += axis_price(&exits[1], v, len) + stub(1, if k == 0 { p } else { sh.s[k - 1].0 });
                                if let Some(ul) = lead_out {
                                    // on into the lead (from its end back to the hand track)
                                    c += bend(v, len, l, -ul);
                                }
                            }
                            lp = l;
                            u = v;
                            l = len;
                        }
                        if !ok {
                            continue;
                        }
                        let dout = sh.s.last().and_then(|x| x.1).unwrap_or(8);
                        if d0 + c >= dj[dout] {
                            continue;
                        }
                        // Legality (a piece of the taut path is legal as drawn: set when built).
                        let leg_ok = *sh.legal.get_or_insert_with(|| {
                            let mut from = p;
                            for &(e, dk) in &sh.s {
                                if (e - from).len2() >= 1.0 && dk.is_some() && !memo_get(sh_legal, own_legal, leg_key(from, e), || legal_f(from, e)) {
                                    return false;
                                }
                                from = e;
                            }
                            true
                        });
                        if !leg_ok {
                            continue;
                        }
                        dj[dout] = d0 + c;
                        best[dout] = Some(((u, l, lp), din, si));
                    }
                }
                (0..ND).filter_map(|dout| best[dout].map(|(ul, din, si)| (dout, dj[dout], ul, (i, din, shapes[si].s.clone())))).collect()
            };
            #[cfg(test)]
            let test_min = OCTI_LANE_MIN.with(|f| f.get());
            #[cfg(not(test))]
            let test_min = 0usize;
            let (lane_min, crew_min) = if test_min > 0 { (test_min, 2) } else { (OCTI_LANE_MIN_PAIRS, OCTI_CREW_MIN_NODES) };
            // One laned node's job: its successors in runs (about three per lane, one memo
            // overlay each), their rows as the node found them, the call's memo (read only).
            struct OJob {
                i: usize,
                fin: Vec<(usize, f64, Vec2, f64, f64)>,
                js: Vec<usize>,
                dj: Vec<[f64; ND]>,
                runs: Vec<(usize, usize)>,
                memo: Memo,
            }
            let unit = |jb: &OJob, r: usize| {
                let (a, b) = jb.runs[r];
                let mut own = Memo::default();
                let ups: Vec<_> = (a..b).map(|x| relax(jb.i, jb.js[x], &jb.fin, jb.dj[x], Some(&jb.memo), &mut own)).collect();
                (ups, own)
            };
            #[allow(clippy::type_complexity)]
            let mut nodes_loop = |mut crew: Option<&mut super::par::Crew<'_, OJob, (Vec<Vec<(usize, f64, (Vec2, f64, f64), (usize, usize, Vec<Leg>))>>, Memo)>>| {
                for i in 0..n {
                    let fin: Vec<(usize, f64, Vec2, f64, f64)> = (0..ND).filter(|&d| dist[i][d].is_finite()).map(|d| (d, dist[i][d], last[i][d].0, last[i][d].1, last[i][d].2)).collect();
                    if fin.is_empty() {
                        continue;
                    }
                    let mut js: Vec<usize> = (i + 1..n.min(i + 1 + WINDOW_MAX)).take_while(|&j| j < i + 1 + WINDOW || nodes[j].1 - nodes[i].1 <= REACH_MM * MM).collect();
                    if let Some(j) = next_on[i] {
                        if !js.contains(&j) {
                            js.push(j);
                        }
                    }
                    if !js.contains(&(n - 1)) && n - 1 > i {
                        js.push(n - 1);
                    }
                    // Work-size rule (never a timing rule): a node with few (state, successor)
                    // pairs stays on this thread; the copper is the same either way.
                    match crew.as_deref_mut() {
                        Some(c) if fin.len() * js.len() >= lane_min => {
                            let t_node = std::time::Instant::now();
                            let nc = (3 * c.lanes()).min(js.len()).max(1);
                            let runs: Vec<(usize, usize)> = (0..nc).map(|k| (k * js.len() / nc, (k + 1) * js.len() / nc)).collect();
                            let dj: Vec<[f64; ND]> = js.iter().map(|&j| dist[j]).collect();
                            let nr = runs.len();
                            let (jb, res) = c.round(OJob { i, fin, js, dj, runs, memo: std::mem::take(&mut memo) }, nr);
                            memo = jb.memo;
                            for (&(a, b), (ups, own)) in jb.runs.iter().zip(res) {
                                for (&j, up) in jb.js[a..b].iter().zip(ups) {
                                    for (dout, d, ul, bk) in up {
                                        dist[j][dout] = d;
                                        last[j][dout] = ul;
                                        back[j][dout] = Some(bk);
                                    }
                                }
                                memo.merge(own);
                            }
                            OCTI_NODES_LANED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            #[cfg(test)]
                            OCTI_NODE_SPLIT.with(|c| c.set([c.get()[0], c.get()[1] + 1]));
                            super::par::meter::time(super::par::meter::OCTI_LANED, t_node);
                        }
                        _ => {
                            #[cfg(test)]
                            OCTI_NODE_SPLIT.with(|c| c.set([c.get()[0] + usize::from(!js.is_empty()), c.get()[1]]));
                            for &j in &js {
                                let mut own = std::mem::take(&mut memo);
                                let up = relax(i, j, &fin, dist[j], None, &mut own);
                                memo = own;
                                for (dout, d, ul, bk) in up {
                                    dist[j][dout] = d;
                                    last[j][dout] = ul;
                                    back[j][dout] = Some(bk);
                                }
                            }
                        }
                    }
                }
            };
            // The crew (persistent lanes for the call) only for a call with enough nodes.
            match if n >= crew_min { super::par::lease(n) } else { None } {
                Some(lease) => super::par::crew(lease, unit, |c| nodes_loop(Some(c))),
                None => nodes_loop(None),
            }
        }
        // Debug (`TOPO_OCTIWHYF`, forced-draw): a forced run's (relax > 0) octilinear legs the
        // legality refused, by the first test that failed (read only).
        if forced && std::env::var_os("TOPO_OCTIWHYF").is_some() {
            let nr0 = self.near(ents.clone(), &wp);
            let mut c = [0usize; 6];
            for (k, &ok) in &memo.legal {
                let (a, b) = (Vec2 { x: f64::from_bits(k[0]), y: f64::from_bits(k[1]) }, Vec2 { x: f64::from_bits(k[2]), y: f64::from_bits(k[3]) });
                let bb = bbox_of(&[a, b]);
                let i = if ok {
                    if self.seg_clear_near(&nr0, a, b) { 0 } else { 1 }
                } else if !(bb.min.x >= region.min.x && bb.min.y >= region.min.y && bb.max.x <= region.max.x && bb.max.y <= region.max.y) {
                    2
                } else if !(strip.contains(b) && strip.contains((a + b) * 0.5)) {
                    3
                } else if strip.crosses_wall(a, b) {
                    4
                } else {
                    5
                };
                c[i] += 1;
            }
            eprintln!("OCTIWHYF w{w} L{layer} legs {}: legal {} (of them below the full rule {}), refused: region {} strip {} wall {} clearance {}", memo.legal.len(), c[0] + c[1], c[1], c[2], c[3], c[4], c[5]);
        }
        // TOPO_OCTINODES=<wire>: the nodes, their best price and the arrival state.
        if octi_nodes_dbg(w) {
            for (i, nd) in nodes.iter().enumerate() {
                let (bd, bs) = (0..ND).map(|s| (dist[i][s], s)).fold((f64::INFINITY, 9), |m, x| if x.0 < m.0 { x } else { m });
                eprintln!("OCTINODE w{w} {i} ({:.3},{:.3}) t {:.3} taut {:?} on {} best {:.3} st {bs} from {:?}", nd.0.x / MM, -nd.0.y / MM, nd.1 / MM, nd.2, nd.3, bd / MM, back[i].get(bs).and_then(|b| b.as_ref()).map(|b| b.0));
            }
        }
        let (mut j, mut d) = (n - 1, (0..ND).min_by(|&a, &b| dist[n - 1][a].total_cmp(&dist[n - 1][b]))?);
        if !dist[j][d].is_finite() {
            return None;
        }
        let mut chain: Vec<Vec<Leg>> = Vec::new();
        while let Some((i, din, s)) = back[j][d].clone() {
            chain.push(s);
            j = i;
            d = din;
        }
        chain.reverse();
        if octi_nodes_dbg(w) {
            let mut from = taut[0];
            for s in &chain {
                for &(e, dk) in s {
                    eprintln!("OCTILEG w{w} ({:.3},{:.3})->({:.3},{:.3}) dir {:?} len {:.3} room {:.3} zones {} gates {}", from.x / MM, -from.y / MM, e.x / MM, -e.y / MM, dk, (e - from).len() / MM, (if room_none { 0.0 } else { room_f(from, e) }) / MM, zones.len(), gates.len());
                    from = e;
                }
            }
            for z in &zones {
                eprintln!("OCTIZONE w{w} ({:.3},{:.3})->({:.3},{:.3}) sign {}", z.0.x / MM, -z.0.y / MM, z.1.x / MM, -z.1.y / MM, z.2);
            }
            for g in &gates {
                let q = g.0 + g.1 * g.2;
                eprintln!("OCTIGATE w{w} ({:.3},{:.3})->({:.3},{:.3}) span {:.3}..{:.3}", g.0.x / MM, -g.0.y / MM, q.x / MM, -q.y / MM, g.3 / MM, g.4 / MM);
            }
        }
        // first-draw-45 P1: the stretches of the taut path (consecutive octilinear legs the leg
        // test refuses, one count each) the chain takes as drawn, and of those the ones below
        // the clearance (priced by `asis_x`).
        let (mut asis, mut asis_below) = (0usize, 0usize);
        {
            let mut from = taut[0];
            let mut open: Option<bool> = None;
            for s in &chain {
                for &(e, dk) in s {
                    if (e - from).len2() < 1.0 {
                        continue;
                    }
                    if dk.is_some() && !legal_f(from, e) {
                        let [c, sh] = self.seg_deficit_parts_near(&nr, from, e);
                        let below = c + sh > 0.0;
                        match open {
                            None => {
                                asis += 1;
                                asis_below += usize::from(below);
                                open = Some(below);
                            }
                            Some(false) if below => {
                                asis_below += 1;
                                open = Some(true);
                            }
                            Some(_) => {}
                        }
                    } else {
                        open = None;
                    }
                    from = e;
                }
            }
        }
        // first-draw-45 r9 forensics (`TOPO_FREEDBG`, copper-neutral): every free-angle leg the chain
        // takes, with the octilinear connectors between its ends whose legs all pass the leg test.
        if std::env::var_os("TOPO_FREEDBG").is_some() {
            let mut from = taut[0];
            for s in &chain {
                for &(e, dk) in s {
                    if dk.is_none() && (e - from).len2() >= 1.0 {
                        let forms = connectors(from, e);
                        let legal = forms
                            .iter()
                            .filter(|sh| {
                                let mut a = from;
                                sh.iter().all(|&(q, _)| {
                                    let ok = (q - a).len2() < 1.0 || legal_f(a, q);
                                    a = q;
                                    ok
                                })
                            })
                            .count();
                        eprintln!("FREEDP w{w} L{layer} forced {forced} ({:.6}, {:.6}) ({:.6}, {:.6}) len {:.4} legal_octi_forms {legal}/{}", from.x / MM, -from.y / MM, e.x / MM, -e.y / MM, (e - from).len() / MM, forms.len());
                    }
                    from = e;
                }
            }
        }
        let mut out = vec![taut[0]];
        for s in chain {
            for (e, _) in s {
                if (e - *out.last().unwrap()).len2() >= 1.0 {
                    out.push(e);
                }
            }
        }
        // Merge collinear legs.
        let mut i = 1;
        while i + 1 < out.len() {
            let (a, b, c) = (out[i - 1], out[i], out[i + 1]);
            if (b - a).cross(c - b).abs() / (c - a).len().max(1.0) <= 0.5 && (b - a).dot(c - b) > 0.0 {
                out.remove(i);
            } else {
                i += 1;
            }
        }
        if out == taut {
            return None;
        }
        Some(OctiOut { pts: out, asis, asis_below })
    }

    /// first-draw-45 r4 (qmtech MAX_SCLK RCA): the lane order is the plan's topology. A leg crossing
    /// one of the run's pass edges `k0..k1` beyond an undrawn same-layer lane neighbour's planned
    /// crossing ([`Drawer::neighbour_x`]) passes that wire on its other side: the two cross
    /// (MAX_SCLK drawn between MAX_I2S0's vias, MAX_I2S0 then forced across it). The slot up to
    /// that crossing stays the graded lane-slot price (P6 b, M6); from the crossing outwards the
    /// edge is a wall, as for P2's own edges (`open_strip`). The taut run's own crossing stays open:
    /// a neighbour planned on the other side of it adds no wall. Debug arm `TOPO_FD45_OFF=lane`.
    /// O(passes x lane).
    pub(super) fn lane_order_walls(&self, w: WireId, k0: usize, k1: usize, taut: &[Vec2]) -> Vec<(Vec2, Vec2)> {
        let (st, pr) = (self.st, &self.b.proj);
        let wi = &st.wires[w];
        let mut out = Vec::new();
        for k in k0..k1 {
            let pid = wi.passes[k];
            let [p, q] = pr.edges[st.passes[pid].edge];
            let (pp, pq) = (pr.pos[p], pr.pos[q]);
            let len = (pq - pp).len();
            if len < 1.0 {
                continue;
            }
            let u = (pq - pp) * (1.0 / len);
            let nom = self.iv.nom.get(pid).copied().filter(|x| x.is_finite()).unwrap_or(0.5) * len;
            let xt = super::draw::crossing(taut, pp, pq, nom);
            let (before, after) = self.lane_sides(pid);
            let (prev_drawn, next_drawn) = self.lane_next_drawn(pid);
            if before && !prev_drawn {
                if let Some(xn) = self.neighbour_x(pid, false).filter(|&xn| xt.is_none_or(|t| xn < t - 1.0)) {
                    if xn > 1.0 {
                        out.push((pp, pp + u * xn.min(len)));
                    }
                }
            }
            if after && !next_drawn {
                if let Some(xn) = self.neighbour_x(pid, true).filter(|&xn| xt.is_none_or(|t| xn > t + 1.0)) {
                    if xn < len - 1.0 {
                        out.push((pp + u * xn.max(0.0), pq));
                    }
                }
            }
        }
        out
    }

    /// The corridor of the octilinear search for passes `k0..k1` of `w` on `layer`: the sleeve's
    /// triangles and `OPEN_RINGS` rings of triangles around them (inside the board). A side is a
    /// wall only where a pass of another wire not drawn yet on this layer (or not yet layered)
    /// crosses it, or where the region ends: crossing an edge that no such pass uses moves the
    /// wire past nobody (a pad or a drawn wire is guarded by the clearance check), so an elbow may
    /// leave the sleeve where the room is free (an empty board's few triangles, a pad end entered
    /// from another side). O(region triangles x passes per edge).
    ///
    /// first-draw-45 P2: an edge the run's own taut path `taut` crosses is a portal of this run
    /// too (the drawn run itself goes through it; the sleeve's pass list misses it where the run
    /// keeps its distance round a corner of a thin triangle). The undrawn wires on it keep their
    /// planned crossing: the edge beyond it stays a wall (the topology), and between the taut
    /// crossing and it the room is a graded price, a lane-slot gate (P6 b) whose span ends at the
    /// planned crossing less the spacing of the two wires: a leg pays for the room it takes from
    /// that wire, `room x pitch x (x + x^2)`, `x = depth / pitch` (Bast, Brosi & Storandt 2020
    /// section 4.3, p. 7). The gates are returned with the strip, `(first end, unit direction,
    /// length, span lo, span hi)`. An edge the taut path crosses on both sides of an undrawn
    /// wire's planned crossing stays a wall.
    #[allow(clippy::type_complexity)]
    pub(super) fn open_strip(&self, w: WireId, k0: usize, k1: usize, layer: usize, taut: &[Vec2]) -> (Strip, Vec<(Vec2, Vec2, f64, f64, f64)>) {
        const OPEN_RINGS: usize = 6;
        let (st, pr) = (self.st, &self.b.proj);
        let wi = &st.wires[w];
        let mut region: Vec<super::state::TriId> = wi.tris[k0..=k1].to_vec();
        region.sort_unstable();
        region.dedup();
        let mut frontier = region.clone();
        for _ in 0..OPEN_RINGS {
            let mut next = Vec::new();
            for &t in &frontier {
                for &e in &pr.tri_edges[t] {
                    if let Some(x) = pr.across(e, t) {
                        if !pr.tri_out[x] && region.binary_search(&x).is_err() && !next.contains(&x) {
                            next.push(x);
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            region.extend(next.iter().copied());
            region.sort_unstable();
            frontier = next;
        }
        let portal: Vec<usize> = (k0..k1).map(|k| st.passes[wi.passes[k]].edge).collect();
        let blocker = |q: usize| {
            let qw = st.passes[q].wire;
            qw != w && (self.pass_layer.get(q).is_none_or(|&l| l < 0 || l as usize == layer)) && self.live_x(q).is_none()
        };
        // P2: the region's edges the taut path crosses that are no portal and carry an undrawn
        // wire: partial walls beyond the undrawn crossings, and a gate each.
        let mut own: Vec<usize> = Vec::new();
        let mut part_walls: Vec<(Vec2, Vec2)> = Vec::new();
        let mut gates: Vec<(Vec2, Vec2, f64, f64, f64)> = Vec::new();
        // fd45 r10: the undrawn wires on the opened edges (their planned paths become walls below).
        let mut opened_for: std::collections::BTreeSet<WireId> = std::collections::BTreeSet::new();
        if self.opts.octi_own_portals && !fd45_off("p2") {
            let mut seen: crate::fxhash::FxHashSet<usize> = crate::fxhash::FxHashSet::default();
            for &t in &region {
                for &e in &pr.tri_edges[t] {
                    if portal.contains(&e) || !seen.insert(e) {
                        continue;
                    }
                    // Only an inner edge of the region can open (a boundary edge stays a wall).
                    if pr.across(e, t).is_none_or(|x| region.binary_search(&x).is_err()) {
                        continue;
                    }
                    if !st.edge_order[e].iter().any(|&q| blocker(q)) {
                        continue;
                    }
                    let [ea, eb] = pr.edges[e];
                    let (pp, pq) = (pr.pos[ea], pr.pos[eb]);
                    let len = (pq - pp).len();
                    if len < 1.0 {
                        continue;
                    }
                    let u = (pq - pp) * (1.0 / len);
                    let xs: Vec<f64> = taut
                        .windows(2)
                        .filter(|x| super::draw::seg_proper(x[0], x[1], pp, pq))
                        .filter_map(|x| {
                            let (d1, d2) = (x[1] - x[0], pq - pp);
                            let den = d1.cross(d2);
                            (den.abs() >= 1e-9).then(|| (pp - x[0]).cross(d1) / den * len)
                        })
                        .collect();
                    if xs.is_empty() {
                        continue;
                    }
                    let (xlo, xhi) = xs.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |m, &x| (m.0.min(x), m.1.max(x)));
                    // (planned crossing along the edge, spacing of the two wires' centres, the far
                    // end of the undrawn wire's packed interval: its slack on the edge)
                    let mut below: Option<(f64, f64, f64)> = None;
                    let mut above: Option<(f64, f64, f64)> = None;
                    let mut mixed = false;
                    for &q in st.edge_order[e].iter().filter(|&&q| blocker(q)) {
                        let oi = &st.wires[st.passes[q].wire];
                        let Some(nom) = self.iv.nom.get(q).copied().filter(|x| x.is_finite()).map(|x| x * len) else {
                            mixed = true;
                            break;
                        };
                        // fd45 r10 (tw2_metric swap_vias): the undrawn wire's planned crossing is
                        // its ERBS's where it has one on this layer (as [`Drawer::neighbour_x`]),
                        // its packed nominal one only without. The nominal one let the octilinear
                        // rescue of Y4's band (joint check) cross Y1's band 4 mm past it: the
                        // selector then cut Y1 back (one form crossing the other's reserved copper).
                        let tq = (self.opts.realise == "erbs")
                            .then(|| self.erbs.get(&st.passes[q].wire))
                            .flatten()
                            .and_then(|v| v.iter().filter(|b| b.layer == layer).filter_map(|b| super::draw::crossing(&b.path, pp, pq, nom)).min_by(|a, b| (a - nom).abs().total_cmp(&(b - nom).abs())))
                            .unwrap_or(nom);
                        let sp = 0.5 * wi.w + wi.s.max(oi.s) + 0.5 * oi.w;
                        // the undrawn wire keeps its slack (debug arm `TOPO_FD45_ON=slack` lends it)
                        let slack = fd45_on("slack");
                        let lo_q = self.iv.lo.get(q).copied().filter(|x| x.is_finite() && slack).map_or(tq, |x| (x * len).min(tq));
                        let hi_q = self.iv.hi.get(q).copied().filter(|x| x.is_finite() && slack).map_or(tq, |x| (x * len).max(tq));
                        if tq < xlo {
                            if below.is_none_or(|b| tq > b.0) {
                                below = Some((tq, sp, lo_q));
                            }
                        } else if tq > xhi {
                            if above.is_none_or(|b| tq < b.0) {
                                above = Some((tq, sp, hi_q));
                            }
                        } else {
                            mixed = true;
                            break;
                        }
                    }
                    if mixed {
                        continue;
                    }
                    own.push(e);
                    for &q in st.edge_order[e].iter().filter(|&&q| blocker(q)) {
                        opened_for.insert(st.passes[q].wire);
                    }
                    // r3 (esp RCA): the wall reaches to the undrawn wire's planned crossing less the
                    // spacing: the run takes only the room the plan left free on the edge. Before
                    // (r2) the wall stopped at the planned crossing and the graded gate (at most
                    // about 2 pitches of length deep) let the run take the room the wire had no
                    // other place for: esp USB_D+ forced (0 -> 14 violations). Its packed slack is
                    // no free room either: it holds the wire's lane over its other edges (the
                    // `slack` arm, measured: esp drc_new 83 vs 46). The taut path's own crossings
                    // stay open.
                    if let Some((_, sp, lo_q)) = below {
                        part_walls.push((pp, pp + u * (lo_q + sp).min(xlo - 1.0).max(0.0)));
                    }
                    if let Some((_, sp, hi_q)) = above {
                        part_walls.push((pp + u * (hi_q - sp).max(xhi + 1.0).min(len), pq));
                    }
                    let lo = below.map_or(f64::NEG_INFINITY, |(tq, sp, _)| (tq + sp).min(xlo - 1.0));
                    let hi = above.map_or(f64::INFINITY, |(tq, sp, _)| (tq - sp).max(xhi + 1.0));
                    gates.push((pp, u, len, lo, hi));
                }
            }
        }
        let blocked = |e: usize| !portal.contains(&e) && !own.contains(&e) && st.edge_order[e].iter().any(|&q| blocker(q));
        let mut tris = Vec::with_capacity(region.len());
        let mut walls = Vec::new();
        for &t in &region {
            let p = pr.tris[t].map(|v| pr.pos[v]);
            tris.push(p);
            for i in 0..3 {
                let e = pr.tri_edges[t][i];
                let inside = pr.across(e, t).is_some_and(|x| region.binary_search(&x).is_ok());
                // An inner edge is shared by two region triangles: add it once.
                if !inside || blocked(e) && pr.across(e, t).is_some_and(|x| x > t) {
                    walls.push((p[i], p[(i + 1) % 3]));
                }
            }
        }
        walls.extend(part_walls);
        // fd45 r10 (tw2_metric swap_vias, P2): an opened edge keeps the order of the run and an
        // undrawn wire on that edge only; through the region's other edges (no blocker on them,
        // so no wall) the run's form can still reach the far side of that wire's path inside a
        // triangle and cross it there (Y4's octilinear rescue in the joint check entered the
        // triangle above Y1's crossing of the opened edge through an edge Y1 does not cross,
        // and crossed Y1's band in it: a short, the selector cut Y1 back). The planned path of
        // every undrawn same-layer wire on an opened edge is a wall: its ERBS on this layer (the
        // reserved copper the joint check commits), unless the run's own taut path crosses it
        // (the plan's own crossing, left to the clearance check). Without an ERBS the per-edge
        // walls above stay its only guard. O(opened wires x band points x taut points).
        if self.opts.realise == "erbs" {
            for &ow in &opened_for {
                for b in self.erbs.get(&ow).map_or(&[][..], |v| v.as_slice()).iter().filter(|b| b.layer == layer) {
                    let crosses = b.path.windows(2).any(|x| taut.windows(2).any(|y| super::draw::seg_proper(x[0], x[1], y[0], y[1])));
                    if !crosses {
                        walls.extend(b.path.windows(2).filter(|x| (x[1] - x[0]).len2() >= 1.0).map(|x| (x[0], x[1])));
                    }
                }
            }
        }
        (Strip::new(tris, walls), gates)
    }

    /// first-draw-45 r10 (mechanism C, lead-approved; "reserve planned room" for joint losers):
    /// the corridors of the wires the joint check rejected ([`super::joint`]: no reserved copper)
    /// and not drawn yet, other nets, on `layer`, near `region`: their ERBS path there is the
    /// plan's realisation and the room they will be drawn in. The octilinear DP of run `w`
    /// prices its legs inside them like an undrawn lane neighbour's zone (`planned_room`). tw2
    /// swap_vias under the sweep order: X3 drawn first staircased across X2's band and X1 ended
    /// diagonally into its pad beside X2's, X2 (rejected, drawn after) found no legal form.
    /// A band the run's own taut path crosses is left out (the plan's own crossing, left to the
    /// clearance check), as the P2 walls do. Without ERBS or the joint check: none.
    /// O(rejected bands x band points x taut points).
    pub(super) fn loser_corridors(&self, w: WireId, layer: usize, taut: &[Vec2], region: &Aabb) -> Vec<Corridor> {
        if self.opts.realise != "erbs" || !self.joint.on {
            return Vec::new();
        }
        let wi = &self.st.wires[w];
        let losers: std::collections::BTreeSet<WireId> = self.joint.class.keys().map(|&(o, _)| o).collect();
        let mut out = Vec::new();
        for o in losers {
            let oi = &self.st.wires[o];
            if o == w || !oi.alive || oi.net == wi.net || self.drawn_once[o] {
                continue;
            }
            let reach = 0.5 * oi.w + oi.s.max(wi.s) + 0.5 * wi.w;
            for b in self.erbs.get(&o).map_or(&[][..], |v| v.as_slice()).iter().filter(|b| b.layer == layer && b.path.len() >= 2) {
                // A run of the same wire whose band is reserved keeps its copper in the index.
                if self.joint.reserved.iter().any(|(&(ro, _), r)| ro == o && r.pts.first().is_some_and(|p| (*p - b.path[0]).len() < 1_000.0)) {
                    continue;
                }
                let bb = bbox_of(&b.path).grow(reach);
                if bb.max.x < region.min.x || bb.min.x > region.max.x || bb.max.y < region.min.y || bb.min.y > region.max.y {
                    continue;
                }
                if b.path.windows(2).any(|x| taut.windows(2).any(|y| super::draw::seg_proper(x[0], x[1], y[0], y[1]))) {
                    continue;
                }
                out.push(Corridor { path: b.path.clone(), reach, pitch: (oi.w + oi.s.max(wi.s)).max(1.0), bb });
            }
        }
        out
    }

    /// Octagon vertices (edges at 0/45/90 degrees) around the corners of object `id` inside `bb`:
    /// a round end (a via, a track end, a point) gets the whole octagon; a convex polygon corner
    /// the vertices within its turn; a reflex corner the offset-line intersection.
    fn octagon_nodes(&self, id: usize, wp: &Wp, bb: &Aabb, out: &mut Vec<Vec2>) {
        let need = self.need_of(id, wp, wp.w);
        let r = need * OCT_R;
        let inside = |p: Vec2| p.x >= bb.min.x - need && p.x <= bb.max.x + need && p.y >= bb.min.y - need && p.y <= bb.max.y + need;
        let oct = |c: Vec2, out: &mut Vec<Vec2>| {
            for k in 0..8 {
                let a = (k as f64 + 0.5) * FRAC_PI_4;
                out.push(c + Vec2 { x: a.cos() * r, y: a.sin() * r });
            }
        };
        match &self.ents[id].core {
            Core::Pt(c) => {
                if inside(*c) {
                    oct(*c, out);
                }
            }
            Core::Seg(a, b) => {
                for c in [*a, *b] {
                    if inside(c) {
                        oct(c, out);
                    }
                }
            }
            Core::Poly(poly) => {
                let n = poly.len();
                let area: f64 = (0..n).map(|i| poly[i].cross(poly[(i + 1) % n])).sum();
                let sgn = if area >= 0.0 { 1.0 } else { -1.0 };
                for i in 0..n {
                    let v = poly[i];
                    if !inside(v) {
                        continue;
                    }
                    let (p, q) = (poly[(i + n - 1) % n], poly[(i + 1) % n]);
                    let (Some(u1), Some(u2)) = ((v - p).unit(), (q - v).unit()) else { continue };
                    let n1 = Vec2 { x: u1.y, y: -u1.x } * sgn;
                    let n2 = Vec2 { x: u2.y, y: -u2.x } * sgn;
                    let turn = u1.cross(u2).atan2(u1.dot(u2)) * sgn;
                    if turn > 1e-3 {
                        // Octagon vertex angles strictly inside the wedge of outward normals.
                        let a0 = n1.angle();
                        for k in 0..8 {
                            let a = (k as f64 + 0.5) * FRAC_PI_4;
                            let rel = if sgn > 0.0 { (a - a0).rem_euclid(std::f64::consts::TAU) } else { (a0 - a).rem_euclid(std::f64::consts::TAU) };
                            if rel > 1e-6 && rel < turn - 1e-6 {
                                out.push(v + Vec2 { x: a.cos() * r, y: a.sin() * r });
                            }
                        }
                        // The offset edges' ends at the corner (on an axis-aligned pad these
                        // are octilinear lines already).
                        out.push(v + n1 * (need * (1.0 + 2e-6) + 0.5));
                        out.push(v + n2 * (need * (1.0 + 2e-6) + 0.5));
                    } else if let Some(bis) = (n1 + n2).unit() {
                        let c = bis.dot(n1).max(0.2);
                        out.push(v + bis * (need / c * 1.0005));
                    }
                }
            }
        }
    }
}

/// The `planned_room` zones (a, b, sign) and lane-slot gates (first end, unit direction,
/// length, span lo, span hi) of one octilinear run with what every leg needs of them computed
/// once: the zone's length and unit direction, the gate's far end (the same expressions as the
/// scan in `octilinear`, so the price is bit-identical).
pub(super) struct RoomPre {
    zones: Vec<(Vec2, f64, f64, Vec2)>,
    gates: Vec<(Vec2, Vec2, f64, f64, f64)>,
    cors: Vec<Corridor>,
}

/// first-draw-45 r10 (mechanism C, lead-approved): the planned room of a joint-rejected wire not
/// drawn yet, its ERBS path on the layer (`path`), the room `reach` either side of it (its half
/// width, the larger clearance and the drawn run's half width), its `pitch` (width plus
/// clearance) and the box `path` grown by `reach`.
#[derive(Clone, Debug)]
pub(super) struct Corridor {
    pub path: Vec<Vec2>,
    pub reach: f64,
    pub pitch: f64,
    pub bb: Aabb,
}

/// The `planned_room` price (before the `room` factor) of the leg `a-b` inside the corridors
/// `cors`: per corridor, the leg's length within `reach` of the path, each piece weighted by
/// `x + x^2`, `x` = the depth into the corridor over the corridor's pitch (the zones' midpoint
/// rule: pieces of at most a quarter of the run's `pitch` or 1/24 of the leg). Graded, no wall
/// (Dayan & Dai 1991, UCSC-CRL-91-40 section 8 p. 6: a non-linear force bounds a quantity
/// without a hard limit). O(corridors x samples x path points).
pub(super) fn corridor_room(cors: &[Corridor], pitch: f64, a: Vec2, b: Vec2) -> f64 {
    let l = (b - a).len();
    if l < 1.0 || cors.is_empty() {
        return 0.0;
    }
    let lb = bbox_of(&[a, b]);
    let n = ((l / (0.25 * pitch)).ceil() as usize).clamp(2, 24);
    let ds = l / n as f64;
    let mut tot = 0.0;
    for c in cors {
        if lb.max.x < c.bb.min.x || lb.min.x > c.bb.max.x || lb.max.y < c.bb.min.y || lb.min.y > c.bb.max.y {
            continue;
        }
        for i in 0..n {
            let p = a + (b - a) * ((i as f64 + 0.5) / n as f64);
            let Some(dist) = c.path.windows(2).map(|x| super::draw::seg_dist(p, x[0], x[1])).reduce(f64::min) else { continue };
            let d = c.reach - dist;
            if d > 0.0 {
                let x = d / c.pitch;
                tot += ds * (x + x * x);
            }
        }
    }
    tot
}

impl RoomPre {
    pub(super) fn new(zones: &[(Vec2, Vec2, f64)], gates: &[(Vec2, Vec2, f64, f64, f64)], cors: &[Corridor]) -> RoomPre {
        let zones = zones
            .iter()
            .map(|&(za, zb, sign)| {
                let zl = (zb - za).len();
                (za, sign, zl, (zb - za) * (1.0 / zl))
            })
            .collect();
        let gates = gates.iter().map(|&(gp, gu, glen, glo, ghi)| (gp, gp + gu * glen, glen, glo, ghi)).collect();
        RoomPre { zones, gates, cors: cors.to_vec() }
    }

    /// The room price of the leg `a-b` before the `room` factor (the scan's `tot`). O(zones x
    /// samples + gates).
    pub(super) fn room(&self, pitch: f64, a: Vec2, b: Vec2) -> f64 {
        let l = (b - a).len();
        let mut tot = 0.0;
        if l >= 1.0 {
            let n = ((l / (0.25 * pitch)).ceil() as usize).clamp(2, 24);
            let ds = l / n as f64;
            for &(za, sign, zl, zu) in &self.zones {
                let d0 = sign * zu.cross(a - za);
                let d1 = sign * zu.cross(b - za);
                if d0 <= 0.0 && d1 <= 0.0 {
                    continue;
                }
                let (t0, t1) = ((a - za).dot(zu), (b - za).dot(zu));
                if (t0 < 0.0 && t1 < 0.0) || (t0 > zl && t1 > zl) {
                    continue;
                }
                for i in 0..n {
                    let f = (i as f64 + 0.5) / n as f64;
                    let (d, t) = (d0 + (d1 - d0) * f, t0 + (t1 - t0) * f);
                    if d > 0.0 && (0.0..=zl).contains(&t) {
                        let x = d / pitch;
                        tot += ds * (x + x * x);
                    }
                }
            }
        }
        tot += corridor_room(&self.cors, pitch, a, b);
        // The lane slots beyond the span (P6 b): the depth of the crossing past it.
        for &(gp, gq, glen, glo, ghi) in &self.gates {
            if !super::draw::seg_proper(a, b, gp, gq) {
                continue;
            }
            let (d1, d2) = (b - a, gq - gp);
            let den = d1.cross(d2);
            if den.abs() < 1e-9 {
                continue;
            }
            let t = (gp - a).cross(d1) / den * glen;
            let depth = (glo - t).max(t - ghi).max(0.0);
            if depth > 0.0 {
                let x = depth / pitch;
                tot += pitch * (x + x * x);
            }
        }
        tot
    }
}

/// The drawn bundle neighbours (a, b, direction class 0..4, clearance) of one octilinear run per
/// direction `k` (0..8): (`dir(k) x a`, index) of those of class `k % 4`, sorted, and the widest
/// clearance. `follow` looks only at the neighbours whose offset across the leg can lie within
/// `tol` of their clearance (window: widest clearance + tol + 1 nm against rounding) and takes the
/// same `max` as the scan of every neighbour (order-free).
pub(super) struct FollowIdx {
    by: Vec<(Vec<(f64, usize)>, f64)>,
}

impl FollowIdx {
    pub(super) fn new(side: &[(Vec2, Vec2, usize, f64)]) -> FollowIdx {
        let by = (0..8)
            .map(|k| {
                let u = dir(k);
                let mut v: Vec<(f64, usize)> = side.iter().enumerate().filter(|(_, x)| x.2 == k % 4).map(|(i, x)| (u.cross(x.0), i)).collect();
                v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                let nmax = v.iter().map(|&(_, i)| side[i].3).fold(0.0, f64::max);
                (v, nmax)
            })
            .collect();
        FollowIdx { by }
    }

    /// Length of the leg `a-b` (direction `k`) that runs beside a neighbour of `side` at its
    /// clearance (the overlap of their projections). O(log n + window).
    pub(super) fn follow(&self, side: &[(Vec2, Vec2, usize, f64)], tol: f64, a: Vec2, b: Vec2, k: usize) -> f64 {
        let u = dir(k);
        let (t0, t1) = (0.0f64, (b - a).dot(u));
        let mut best = 0.0f64;
        let (by, nmax) = &self.by[k % 8];
        let (sa, r) = (u.cross(a), nmax + tol + 1.0);
        let from = by.partition_point(|x| x.0 < sa - r);
        for &(sc, i) in &by[from..] {
            if sc > sa + r {
                break;
            }
            let (c, e, _, need) = side[i];
            let off = u.cross(c - a).abs();
            if (off - need).abs() > tol {
                continue;
            }
            let (s0, s1) = ((c - a).dot(u), (e - a).dot(u));
            let ov = (t1.min(s0.max(s1)) - t0.max(s0.min(s1))).max(0.0);
            best = best.max(ov);
        }
        best
    }
}

/// Octilinear connectors from `p` to `q`: the straight leg when they are aligned, else the two
/// elbows and the two Z shapes over the two octilinear directions that bracket `q - p`.
fn connectors(p: Vec2, q: Vec2) -> Vec<Vec<Leg>> {
    let d = q - p;
    if d.len2() < 1.0 {
        return Vec::new();
    }
    if let Some(k) = octi_dir(d) {
        return vec![vec![(q, Some(k))]];
    }
    let a = d.y.atan2(d.x).rem_euclid(std::f64::consts::TAU);
    let k0 = ((a / FRAC_PI_4).floor() as usize) % 8;
    let k1 = (k0 + 1) % 8;
    let (d0, d1) = (dir(k0), dir(k1));
    let den = d0.cross(d1);
    let al = d.cross(d1) / den;
    let be = d0.cross(d) / den;
    if al <= 0.0 || be <= 0.0 {
        return Vec::new();
    }
    vec![
        vec![(p + d0 * al, Some(k0)), (q, Some(k1))],
        vec![(p + d1 * be, Some(k1)), (q, Some(k0))],
        vec![(p + d0 * (0.5 * al), Some(k0)), (p + d0 * (0.5 * al) + d1 * be, Some(k1)), (q, Some(k0))],
        vec![(p + d1 * (0.5 * be), Some(k1)), (p + d1 * (0.5 * be) + d0 * al, Some(k0)), (q, Some(k1))],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fuse-octi r2: the jog penalty and its leg (`jog_leg` = min(L_pref, 2 pitch)) in `style_price`.
    #[test]
    fn style_price_pays_the_jog_penalty_on_the_pitch_scale() {
        let mm = 1e6;
        let cfg = OctiCfg { on: true, bend_nm: 1.0 * mm, spacing_nm: 2.0 * mm, any_nm: 15.0 * mm, ..Default::default() };
        let v = |x: f64, y: f64| Vec2 { x: x * mm, y: y * mm };
        // 0.5 mm leg, then a 45 degree turn.
        let pts = [v(0.0, 0.0), v(0.5, 0.0), v(1.5, 1.0)];
        // pitch 0.5 mm: leff = min(2, 1) = 1 mm: bend 1 + jog 2 (1 - 0.5)^2 = 1.5 mm.
        assert!((style_price(&cfg, 0.5 * mm, &pts) / mm - 1.5).abs() < 1e-9);
        // pitch 2 mm: leff = min(2, 4) = 2 mm: 1 + 2 (1 - 0.25)^2 = 2.125 mm.
        assert!((style_price(&cfg, 2.0 * mm, &pts) / mm - 2.125).abs() < 1e-9);
        // pitch 0.2 mm: leff 0.4 mm < the leg: no jog penalty.
        assert!((style_price(&cfg, 0.2 * mm, &pts) / mm - 1.0).abs() < 1e-9);
        assert!((jog_leg(&cfg, 0.5 * mm) / mm - 1.0).abs() < 1e-9 && (jog_leg(&cfg, 2.0 * mm) / mm - 2.0).abs() < 1e-9);
    }


    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x, y }
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }
        fn f(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
        fn int(&mut self, lo: i64, hi: i64) -> i64 {
            lo + (self.next() % (hi - lo + 1) as u64) as i64
        }
    }

    /// The `room` scan of `octilinear` before `RoomPre` (its `tot`), verbatim.
    fn room_ref(zones: &[(Vec2, Vec2, f64)], gates: &[(Vec2, Vec2, f64, f64, f64)], pitch: f64, a: Vec2, b: Vec2) -> f64 {
        let l = (b - a).len();
        let mut tot = 0.0;
        if l >= 1.0 {
            for &(za, zb, sign) in zones {
                let zl = (zb - za).len();
                let zu = (zb - za) * (1.0 / zl);
                let d0 = sign * zu.cross(a - za);
                let d1 = sign * zu.cross(b - za);
                if d0 <= 0.0 && d1 <= 0.0 {
                    continue;
                }
                let (t0, t1) = ((a - za).dot(zu), (b - za).dot(zu));
                if (t0 < 0.0 && t1 < 0.0) || (t0 > zl && t1 > zl) {
                    continue;
                }
                let n = ((l / (0.25 * pitch)).ceil() as usize).clamp(2, 24);
                let ds = l / n as f64;
                for i in 0..n {
                    let f = (i as f64 + 0.5) / n as f64;
                    let (d, t) = (d0 + (d1 - d0) * f, t0 + (t1 - t0) * f);
                    if d > 0.0 && (0.0..=zl).contains(&t) {
                        let x = d / pitch;
                        tot += ds * (x + x * x);
                    }
                }
            }
        }
        for &(gp, gu, glen, glo, ghi) in gates {
            let gq = gp + gu * glen;
            if !super::super::draw::seg_proper(a, b, gp, gq) {
                continue;
            }
            let (d1, d2) = (b - a, gq - gp);
            let den = d1.cross(d2);
            if den.abs() < 1e-9 {
                continue;
            }
            let t = (gp - a).cross(d1) / den * glen;
            let depth = (glo - t).max(t - ghi).max(0.0);
            if depth > 0.0 {
                let x = depth / pitch;
                tot += pitch * (x + x * x);
            }
        }
        tot
    }

    /// The `follow` scan of `octilinear` before `FollowIdx`, verbatim.
    fn follow_ref(side: &[(Vec2, Vec2, usize, f64)], tol: f64, a: Vec2, b: Vec2, k: usize) -> f64 {
        let u = dir(k);
        let (t0, t1) = (0.0f64, (b - a).dot(u));
        let mut best = 0.0f64;
        for &(c, e, kk, need) in side {
            if kk != k % 4 {
                continue;
            }
            let off = u.cross(c - a).abs();
            if (off - need).abs() > tol {
                continue;
            }
            let (s0, s1) = ((c - a).dot(u), (e - a).dot(u));
            let ov = (t1.min(s0.max(s1)) - t0.max(s0.min(s1))).max(0.0);
            best = best.max(ov);
        }
        best
    }

    /// `RoomPre::room` and `FollowIdx::follow` equal the scans bit for bit on random runs:
    /// zones and gates crossed, touched and missed, zero-length and sub-nm legs, gates parallel
    /// to the leg, gate spans open on one side; neighbours exactly at clearance +- tol, at the
    /// window edge (widest clearance + tol), of every direction class, collinear with the leg.
    #[test]
    fn room_and_follow_indexes_match_the_scans() {
        let mut g = Rng(0x6a09_e667_f3bc_c909);
        let (mut room_hits, mut follow_hits) = (0usize, 0usize);
        const MM: f64 = 1e6;
        for _ in 0..300 {
            let o = v(1e8 * g.f(), -1e8 * g.f());
            let sc = [0.2, 2.0, 10.0][g.int(0, 2) as usize] * MM;
            let rp = |g: &mut Rng| o + v(g.f() * sc, g.f() * sc);
            let pitch = (0.1 + 0.5 * g.f()) * MM;
            let zones: Vec<(Vec2, Vec2, f64)> = (0..g.int(0, 12)).map(|_| (rp(&mut g), rp(&mut g), if g.int(0, 1) == 0 { 1.0 } else { -1.0 })).collect();
            let gates: Vec<(Vec2, Vec2, f64, f64, f64)> = (0..g.int(0, 12))
                .map(|_| {
                    let (p, q) = (rp(&mut g), rp(&mut g));
                    let len = (q - p).len().max(1.0);
                    let lo = if g.int(0, 2) == 0 { f64::NEG_INFINITY } else { len * 0.5 * g.f() };
                    let hi = if g.int(0, 2) == 0 { f64::INFINITY } else { len * (0.5 + 0.5 * g.f()) };
                    (p, (q - p) * (1.0 / len), len, lo, hi)
                })
                .collect();
            let tol = 0.05 * pitch + 1_000.0;
            let mut side: Vec<(Vec2, Vec2, usize, f64)> = Vec::new();
            for _ in 0..g.int(0, 30) {
                let kk = g.int(0, 7) as usize;
                let c = rp(&mut g);
                side.push((c, c + dir(kk) * sc * g.f(), kk % 4, pitch * (0.5 + g.f())));
            }
            let rpre = RoomPre::new(&zones, &gates, &[]);
            let fidx = FollowIdx::new(&side);
            for _ in 0..60 {
                let a = match g.int(0, 3) {
                    0 if !side.is_empty() => side[g.int(0, side.len() as i64 - 1) as usize].0,
                    1 if !gates.is_empty() => gates[g.int(0, gates.len() as i64 - 1) as usize].0,
                    _ => rp(&mut g),
                };
                let k = g.int(0, 7) as usize;
                let b = match g.int(0, 4) {
                    0 => a,
                    1 => a + dir(k) * 0.5,
                    2 if !gates.is_empty() => {
                        let gt = gates[g.int(0, gates.len() as i64 - 1) as usize];
                        a + gt.1 * sc * g.f()
                    }
                    _ => a + dir(k) * sc * g.f(),
                };
                let r0 = room_ref(&zones, &gates, pitch, a, b);
                assert_eq!(rpre.room(pitch, a, b).to_bits(), r0.to_bits(), "room a {a:?} b {b:?}");
                room_hits += usize::from(r0 > 0.0);
                // A leg next to a neighbour: offset exactly at its clearance, +- tol, the window edge.
                let (a2, k2) = if !side.is_empty() && g.int(0, 1) == 0 {
                    let (c, _, kk, need) = side[g.int(0, side.len() as i64 - 1) as usize];
                    let k2 = kk + 4 * g.int(0, 1) as usize;
                    let nmax = side.iter().map(|x| x.3).fold(0.0, f64::max);
                    let off = [need, need + tol, need - tol, need + tol + 1e-3, nmax + tol, nmax + tol + 1.0, 0.0][g.int(0, 6) as usize];
                    let u = dir(k2);
                    (c + v(-u.y, u.x) * off * if g.int(0, 1) == 0 { 1.0 } else { -1.0 } - u * sc * g.f(), k2)
                } else {
                    (a, k)
                };
                let b2 = a2 + dir(k2) * sc * (0.2 + g.f());
                let f0 = follow_ref(&side, tol, a2, b2, k2);
                assert_eq!(fidx.follow(&side, tol, a2, b2, k2).to_bits(), f0.to_bits(), "follow a {a2:?} k {k2}");
                follow_hits += usize::from(f0 > 0.0);
            }
        }
        assert!(room_hits > 500 && follow_hits > 500, "room {room_hits} follow {follow_hits}");
    }

    #[test]
    fn directions_are_recognised() {
        assert_eq!(octi_dir(v(5.0, 0.0)), Some(0));
        assert_eq!(octi_dir(v(3.0, 3.0)), Some(1));
        assert_eq!(octi_dir(v(0.0, -2e6)), Some(6));
        assert_eq!(octi_dir(v(-1e6, 1e6 + 0.5)), Some(3));
        assert_eq!(octi_dir(v(2e6, 1e6)), None);
    }

    fn stub_price_d(poly: &[Vec2], c: Vec2, ext: f64, w: f64, j: Vec2) -> f64 {
        stub_price(poly, c, ext, w, j, STUB_NM)
    }

    /// pad-snap: the stub price is 0 for a joint outside the pad, on its outline or within half the
    /// width of it (the stub's round end sticks out: kept by tidy and Cleanup), and at the centre;
    /// deeper inside it is STUB_NM graded by the distance from the centre.
    /// fd45 r10 (verifier F8): at `bend_mm = 0` ("only length counts") a staircase still costs
    /// more than its elbow (the bend tie floor), and a bend after a tiny leg more than one after
    /// a long leg (the jog floor): the side-exit staircase of long legs and 0.03-0.07 mm legs
    /// (P4 chord nodes) is no longer a tie the node order decides. Both floors stay far below a
    /// length the copper can tell (1 um per 45 degrees, 2 um per jog).
    #[test]
    fn staircase_costs_more_than_its_elbow_when_only_length_counts() {
        let cfg = OctiCfg { on: true, bend_nm: 0.0, ..Default::default() };
        let pitch = 0.45 * MM;
        let v = |x: f64, y: f64| Vec2 { x: x * MM, y: y * MM };
        // A 45 degree stair of long 0 degree legs and tiny 45 degree legs against its elbow.
        let mut stair = vec![v(0.0, 0.0)];
        for i in 0..5 {
            let (x, y) = (i as f64 * 1.05, i as f64 * 0.05);
            stair.extend([v(x + 1.0, y), v(x + 1.05, y + 0.05)]);
        }
        let end = *stair.last().unwrap();
        let elbow = vec![v(0.0, 0.0), Vec2 { x: end.x - (end.y - 0.0), y: 0.0 }, end];
        let (ps, pe) = (style_price(&cfg, pitch, &stair), style_price(&cfg, pitch, &elbow));
        assert!(ps > pe && pe > 0.0, "stair {ps} elbow {pe}");
        let leff = jog_leg(&cfg, pitch);
        let (e, d) = (v(1.0, 0.0), v(1.0, 1.0) * std::f64::consts::FRAC_1_SQRT_2);
        let (tiny, long) = (bend_price(&cfg, leff, d, 0.05 * MM, e), bend_price(&cfg, leff, d, 2.0 * MM, e));
        assert!(tiny > long && long > 0.0 && tiny < 4.0 * BEND_TIE_NM, "tiny {tiny} long {long}");
    }

    #[test]
    fn stub_price_ranks_below_one_violation() {
        // pad-snap r2 (rule order short > clearance > off-centre stub): the dearest stub (an end
        // at the pad's far side, 2 x STUB_NM) is cheaper than the least clearance violation the
        // forced DP prices (one violation unit at the default weight)
        let pitch = 0.4 * MM;
        assert_eq!(OctiCfg::default().stub_nm(pitch), STUB_NM, "4 x bend_nm at the defaults");
        // fd45 r10: never free with the bend price: at least the pitch (bend_mm 0), unless the
        // violation weight caps it
        let free_bends = OctiCfg { bend_nm: 0.0, ..OctiCfg::default() };
        assert_eq!(free_bends.stub_nm(pitch), pitch, "bend_mm 0: the pitch");
        assert_eq!(free_bends.stub_nm(2.0 * pitch), 2.0 * pitch, "the pitch, whatever it is");
        // every violation weight the config accepts (>= 0) and a range of bend prices and pitches
        for wt in [0.0, 0.01, 0.5, 1.0, 1.9, 2.0, 5.0, 1000.0] {
            for bend in [0.0, 0.1, 0.25, 1.0, 5.0] {
                for pitch in [0.0, 0.25 * MM, 0.4 * MM, 1.0 * MM] {
                    let cfg = OctiCfg { viol_nm: wt * MM, short_nm: wt * MM * SHORT_FIRST, bend_nm: bend * MM, ..OctiCfg::default() };
                    let dearest = 2.0 * cfg.stub_nm(pitch);
                    assert!(dearest <= 0.8 * cfg.viol_nm + 1e-9 && (dearest < cfg.viol_nm || wt == 0.0), "w {wt} bend {bend}: {dearest} vs {}", cfg.viol_nm);
                    assert!(wt == 0.0 || cfg.stub_nm(pitch) >= (4.0 * cfg.bend_nm).max(pitch).min(0.4 * cfg.viol_nm) - 1e-9);
                    assert!(cfg.viol_nm <= cfg.short_nm);
                }
            }
        }
    }

    #[test]
    fn stub_price_only_for_a_stub_wholly_inside_the_pad() {
        // 0.6 x 1.95 mm pad centred at the origin (math frame), a 0.2 mm track
        let poly = vec![v(-0.3e6, -0.975e6), v(0.3e6, -0.975e6), v(0.3e6, 0.975e6), v(-0.3e6, 0.975e6)];
        let c = v(0.0, 0.0);
        let ext = (0.3e6f64).hypot(0.975e6);
        let w = 0.2e6;
        assert_eq!(stub_price_d(&poly, c, ext, w, c), 0.0);
        assert_eq!(stub_price_d(&poly, c, ext, w, v(0.0, 1.2e6)), 0.0, "outside");
        assert_eq!(stub_price_d(&poly, c, ext, w, v(0.0, 0.975e6 - 0.1e6 + LEAVE_NM)), 0.0, "the leave point");
        assert_eq!(stub_price_d(&poly, c, ext, w, v(0.3e6 - 0.05e6, 0.0)), 0.0, "near the long side");
        let deep = stub_price_d(&poly, c, ext, w, v(0.0, 0.5e6));
        assert!((deep - STUB_NM * (1.0 + 0.5e6 / ext)).abs() < 1e-6, "{deep}");
        let deeper = stub_price_d(&poly, c, ext, w, v(0.0, 0.8e6));
        assert!(deeper > deep, "graded by the distance from the centre: {deeper} vs {deep}");
        // within tidy's tolerance of half the width: inside (tidy drops it), so priced
        assert!(stub_price_d(&poly, c, ext, w, v(0.0, 0.975e6 - 0.1e6 + 0.5 * STUB_TOL_NM)) > 0.0);
    }

    /// pad-snap r2b: the leave point lies on the ray where the round end of a stub from the centre
    /// first sticks out of the pad by LEAVE_NM (axis, side and diagonal directions), so its stub
    /// price is 0 there and > 0 just before it; none for a pad narrower than the track's half
    /// width plus LEAVE_NM about the end.
    #[test]
    fn leave_point_is_where_the_stub_first_sticks_out() {
        let poly = vec![v(-0.3e6, -0.975e6), v(0.3e6, -0.975e6), v(0.3e6, 0.975e6), v(-0.3e6, 0.975e6)];
        let (c, w) = (v(0.0, 0.0), 0.2e6);
        let ext = (0.3e6f64).hypot(0.975e6);
        let s2 = std::f64::consts::FRAC_1_SQRT_2;
        for (d, want) in [(v(0.0, 1.0), v(0.0, 0.895e6)), (v(-1.0, 0.0), v(-0.22e6, 0.0)), (v(s2, s2), v(0.22e6, 0.22e6)), (v(s2, -s2), v(0.22e6, -0.22e6))] {
            let m = leave_point(&poly, c, d, w).expect("a leave point");
            assert!((m - want).len() < 10.0, "{m:?} vs {want:?}");
            assert_eq!(stub_price_d(&poly, c, ext, w, m), 0.0);
            assert!(stub_price_d(&poly, c, ext, w, m - d * 30_000.0) > 0.0, "30 um before the leave point the stub is wholly inside");
        }
        let thin = vec![v(-0.075e6, -0.975e6), v(0.075e6, -0.975e6), v(0.075e6, 0.975e6), v(-0.075e6, 0.975e6)];
        assert!(leave_point(&thin, c, v(0.0, 1.0), w).is_none());
    }

    #[test]
    fn connectors_reach_the_target_octilinear() {
        let (p, q) = (v(0.0, 0.0), v(3e6, 1e6));
        let cs = connectors(p, q);
        assert_eq!(cs.len(), 4);
        for c in &cs {
            let mut from = p;
            for &(e, k) in c {
                assert_eq!(octi_dir(e - from), k, "{c:?}");
                from = e;
            }
            assert!((from - q).len() < 1e-6);
        }
        // The elbows have the same length (0/45 split of 3 x 1 mm: 2 + 1.414 mm).
        let len = |c: &Vec<Leg>| {
            let mut f = p;
            c.iter().map(|&(e, _)| { let l = (e - f).len(); f = e; l }).sum::<f64>()
        };
        assert!((len(&cs[0]) - (2e6 + 2f64.sqrt() * 1e6)).abs() < 1.0);
        assert!((len(&cs[2]) - len(&cs[0])).abs() < 1.0);
    }

    #[test]
    fn breaks_count_stretches_and_acute_bends() {
        let pts = [v(0.0, 0.0), v(1e6, 0.0), v(2e6, 0.5e6), v(3e6, 0.7e6), v(4e6, 0.7e6), v(3e6, 1.7e6)];
        let (any, acute) = breaks(&pts);
        assert_eq!(any.len(), 1, "{any:?}");
        assert!((any[0].2 - (v(1e6, 0.5e6).len() + v(1e6, 0.2e6).len())).abs() < 1.0);
        assert_eq!(acute.len(), 1);
        assert!((acute[0].1 - 45.0).abs() < 1e-6);
    }
}

impl Drawer<'_> {
    /// Post-pass of the octilinear style over a drawn wire (`wo`, plain legal copper only, as
    /// [`Drawer::refit_run`]): every segment not at 0/45/90 degrees that a later step drew (a
    /// branch end fused onto its trunk) is replaced by the elbow or Z shape with the least turning
    /// that the checker accepts against everything drawn and `joined` lets through (fuse-octi r4,
    /// H1 part 2: the split guard, `joined(wire before, wire as redrawn)` is false when the step
    /// would split the net; the segment then goes back as it was); else it stays. `keep`: the points other copper
    /// ends on (planned branch points, fused feet, r6: other wires' ends and vias within KiCad's reach,
    /// projected), fixed vertices. Returns the number of segments
    /// replaced. O(segments x shapes x checker query).
    pub fn octi_polish(&mut self, wo: &mut super::draw::WireOut, keep: &[Vec2], joined: &mut dyn FnMut(&super::draw::WireOut, &super::draw::WireOut) -> bool) -> usize {
        let mut n = 0;
        for r in 0..wo.runs.len() {
            let n_run = n;
            let mut cur = wo.runs[r].1.clone();
            // Every point other copper ends on (`keep`: a planned branch point or a fused foot) lying inside a
            // segment becomes a vertex of it, so the segment is redrawn in two pieces that both end on it
            // (fuse-octi r4: the elbow of the whole segment dropped the foot and orphaned the branch). Only
            // segments the polish redraws (not at 0/45/90 degrees) take them: the others stay as they are.
            for &p in keep {
                if let Some(i) = (0..cur.len().saturating_sub(1)).find(|&i| {
                    let ab = cur[i + 1] - cur[i];
                    let t = (p - cur[i]).dot(ab) / ab.len2().max(1.0);
                    octi_dir(ab).is_none() && t > 0.0 && t < 1.0 && (cur[i] + ab * t - p).len() < 1_000.0 && (p - cur[i]).len() > 1_000.0 && (p - cur[i + 1]).len() > 1_000.0
                }) {
                    cur.insert(i + 1, p);
                }
            }
            if breaks(&cur).0.is_empty() {
                continue;
            }
            let mut i = 0;
            while i + 1 < cur.len() {
                let (a, b) = (cur[i], cur[i + 1]);
                if (b - a).len2() <= 1.0 || octi_dir(b - a).is_some() {
                    i += 1;
                    continue;
                }
                let u_in = if i > 0 { a - cur[i - 1] } else { Vec2::default() };
                let u_out = if i + 2 < cur.len() { cur[i + 2] - b } else { Vec2::default() };
                let mut shapes: Vec<(f64, Vec<Vec2>)> = connectors(a, b)
                    .into_iter()
                    .map(|s| {
                        let pts: Vec<Vec2> = s.iter().map(|x| x.0).collect();
                        let mut turn = 0.0;
                        let (mut u, mut from) = (u_in, a);
                        for &p in &pts {
                            turn += turn_deg(u, p - from);
                            u = p - from;
                            from = p;
                        }
                        turn += turn_deg(u, u_out);
                        (turn, pts)
                    })
                    .collect();
                shapes.sort_by(|x, y| x.0.total_cmp(&y.0));
                let mut done = false;
                for (_, s) in shapes {
                    let mut trial = cur[..=i].to_vec();
                    trial.extend(s.iter().copied());
                    trial.extend_from_slice(&cur[i + 2..]);
                    let snap = wo.clone();
                    if self.refit_run(wo, r, trial.clone()) {
                        if !joined(&snap, wo) {
                            let back = self.restore_wire(wo, &snap);
                            *wo = back;
                            continue;
                        }
                        i += s.len();
                        cur = trial;
                        n += 1;
                        done = true;
                        break;
                    }
                }
                if !done {
                    i += 1;
                }
            }
            // r6: in a run the polish redrew, a kept foot between two legs that leave it in one
            // direction is a collinear vertex; KiCad Cleanup would merge the two segments ("merge co-linear tracks"), so the
            // run goes out with them merged (the same copper, the foot still on it).
            let simple = drop_collinear(&wo.runs[r].1);
            if n > n_run && simple.len() < wo.runs[r].1.len() {
                let snap = wo.clone();
                if self.refit_run(wo, r, simple) && !joined(&snap, wo) {
                    let back = self.restore_wire(wo, &snap);
                    *wo = back;
                }
            }
        }
        n
    }
}

impl Drawer<'_> {
    /// first-draw-45 P4 step 2, the straightening pass (Staepelaere 1992: StraightenLayer, ch. 5,
    /// PDF p. 15, removes the jogs of the transformed layer; each segment is pushed onto the
    /// already transformed ones, PDF pp. 16, 33). Over the drawn wire `wo` (plain legal copper,
    /// as [`Drawer::refit_run`]), per run, from its first vertex on: the longest stretch
    /// `cur[i..=j]` with two bends or more that one octilinear form of its ends replaces at a
    /// lower price, checked against everything drawn ([`Drawer::refit_run`]) and the split guard
    /// `joined`. The forms: straight, the two elbows, and the Z shapes of the two direction orders
    /// with their middle leg centred ([`connectors`]) or at the clearance beside a parallel drawn
    /// segment of another net (the depth of the middle leg is a tie on length; the bundle price
    /// below breaks it towards the drawn neighbour).
    /// Price, the octilinear DP's scale: length + [`style_price`] with the legs before and after
    /// (bends by angle, the jog penalty: a staircase of short steps pays most) - `bundle_pull` x
    /// the length running beside a drawn neighbour at its clearance (the DP's bundle following).
    /// Graded, no bend count. A stretch from the run's first vertex (or to its last) keeps the
    /// direction of that end leg (the pad exit or via leg the DP chose) and at least its length or
    /// the jog leg. The points in `keep` stay vertices. Returns the stretches replaced.
    /// Deterministic (vertex order; forms by price, then in order). O(vertices^2 x forms x checker
    /// query) per run.
    pub fn octi_straighten(&mut self, wo: &mut super::draw::WireOut, keep: &[Vec2], joined: &mut dyn FnMut(&super::draw::WireOut, &super::draw::WireOut) -> bool) -> usize {
        let cfg = self.octi;
        let w = wo.wire;
        let wi = &self.st.wires[w];
        let (net, ww, pitch, own) = (wi.net, wi.w, wi.w + wi.s, [wi.pa, wi.pb]);
        let leff = jog_leg(&cfg, pitch);
        let tol = 0.05 * pitch + 1_000.0;
        let plen = |p: &[Vec2]| p.windows(2).map(|x| (x[1] - x[0]).len()).sum::<f64>();
        let dbg = std::env::var_os("TOPO_STRDBG").is_some();
        let mut n = 0;
        for r in 0..wo.runs.len() {
            let mut cur = drop_collinear(&wo.runs[r].1);
            if cur.len() < 4 {
                continue;
            }
            let layer = wo.runs[r].0;
            let wp = Wp { wire: w, net, w: ww, s: wi.s, layer, own, neck: None, relax: 0.0 };
            // The drawn octilinear segments of other nets near the run (the DP's `side`).
            let region: Aabb = bbox_of(&cur).grow(2.0 * MM + 4.0 * pitch);
            let side: Vec<(Vec2, Vec2, usize, f64)> = self
                .local_ents(&wp, region.grow(self.qgrow))
                .into_iter()
                .filter(|&id| id >= self.n_static && self.ents[id].net != Some(net))
                .filter_map(|id| match self.ents[id].core {
                    Core::Seg(a, b) => octi_dir(b - a).map(|k| (a, b, k % 4, self.need_of(id, &wp, ww))),
                    _ => None,
                })
                .collect();
            let fidx = FollowIdx::new(&side);
            let follow = |p: &[Vec2]| -> f64 {
                if cfg.pull <= 0.0 {
                    return 0.0;
                }
                p.windows(2).filter_map(|x| octi_dir(x[1] - x[0]).map(|k| fidx.follow(&side, tol, x[0], x[1], k))).sum()
            };
            if dbg {
                eprintln!("STRAIGHTEN wire {w} run {r}: {} pts, side {}, refit preconditions {}", cur.len(), side.len(), self.refit_legal(wo, r, &wo.runs[r].1));
            }
            // The whole run's price: a step is taken only where it lowers this (by more than 1 nm),
            // so the pass ends. The span price alone is not enough: the style price is not a sum
            // over the spans (a jog's price reads both legs beside it), two overlapping spans can
            // each look cheaper than the other (seen on K5: an endless swap).
            let full = |p: &[Vec2]| plen(p) + style_price(&cfg, pitch, p) - cfg.pull * follow(p);
            // fd45 r10 (pad-snap): at an end of the wire on its own pad (pad-snap's one predicate,
            // [`Drawer::own_pad_end`]), the end leg keeps leaving the pad: a shorter end leg whose
            // far joint `new` leaves a stub wholly inside the pad ([`stub_price`] at base 1, any
            // bend price) where the drawn joint `old` did not is refused. Tidy and KiCad's Cleanup
            // delete such a stub, the end then sits off the pad centre (topo_pad_snap
            // both_ends_on_soic_rows). The anchor itself never moves (a form starts at `cur[i]`).
            let n_runs = wo.runs.len();
            let ends = [cur[0], cur[cur.len() - 1]];
            let pads: [Option<(Vec<Vec2>, Vec2, f64)>; 2] = [0usize, 1].map(|end| self.own_pad_end(w, end, if end == 0 { r == 0 } else { r + 1 == n_runs }, false, ends[end]).map(|(poly, c, ext)| (poly.to_vec(), c, ext)));
            let pad_leave_lost = |end: usize, old: Vec2, new: Vec2| -> bool { pads[end].as_ref().is_some_and(|(poly, c, ext)| stub_price(poly, *c, *ext, ww, new, 1.0) > 0.0 && stub_price(poly, *c, *ext, ww, old, 1.0) <= 0.0) };
            let mut i = 0;
            while i + 2 < cur.len() {
                let mut done = false;
                let full_old = full(&cur);
                for j in (i + 2..cur.len()).rev() {
                    if (i + 1..j).any(|m| keep.iter().any(|&p| (cur[m] - p).len() < 1_000.0)) {
                        continue;
                    }
                    let last = cur.len() - 1;
                    let price = |mid: &[Vec2]| -> f64 {
                        let mut ext: Vec<Vec2> = if i > 0 { vec![cur[i - 1]] } else { Vec::new() };
                        ext.extend_from_slice(mid);
                        if j < last {
                            ext.push(cur[j + 1]);
                        }
                        plen(mid) + style_price(&cfg, pitch, &ext) - cfg.pull * follow(mid)
                    };
                    let old = price(&cur[i..=j]);
                    let (a, b) = (cur[i], cur[j]);
                    let mut forms: Vec<Vec<Vec2>> = connectors(a, b).into_iter().map(|sh| sh.iter().map(|x| x.0).collect()).collect();
                    // Z shapes with the middle leg at the clearance beside a parallel drawn segment.
                    let d = b - a;
                    if octi_dir(d).is_none() && d.len2() >= 1.0 {
                        let ang = d.y.atan2(d.x).rem_euclid(std::f64::consts::TAU);
                        let k0 = ((ang / FRAC_PI_4).floor() as usize) % 8;
                        let k1 = (k0 + 1) % 8;
                        let (d0, d1) = (dir(k0), dir(k1));
                        let den = d0.cross(d1);
                        let (al, be) = (d.cross(d1) / den, d0.cross(d) / den);
                        for &(c, _, kk, need) in &side {
                            for (u, um, lu, mk) in [(d0, d1, al, k1), (d1, d0, be, k0)] {
                                if kk != mk % 4 {
                                    continue;
                                }
                                let x = u.cross(um);
                                for sg in [-1.0, 1.0] {
                                    let s = ((c - a).cross(um) + sg * need) / x;
                                    if s > 1_000.0 && s < lu - 1_000.0 {
                                        let p1 = a + u * s;
                                        let p2 = p1 + um * if mk == k1 { be } else { al };
                                        if hug_within_range(&cfg, &cur[i..=j], p1, p2) {
                                            forms.push(vec![p1, p2, b]);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let mut cands: Vec<(f64, usize, Vec<Vec2>)> = forms
                        .into_iter()
                        .enumerate()
                        .filter_map(|(fi, sh)| {
                            let mut mid = vec![a];
                            mid.extend(sh);
                            // The run's end legs keep their direction and length (pad exit, via leg).
                            if i == 0 {
                                let (o, q) = (cur[1] - cur[0], mid[1] - mid[0]);
                                if octi_dir(q) != octi_dir(o) || q.len() + 1_000.0 < o.len().min(leff) || pad_leave_lost(0, cur[1], mid[1]) {
                                    return None;
                                }
                            }
                            if j == last {
                                let m = mid.len();
                                let (o, q) = (cur[last] - cur[last - 1], mid[m - 1] - mid[m - 2]);
                                if octi_dir(q) != octi_dir(o) || q.len() + 1_000.0 < o.len().min(leff) || pad_leave_lost(1, cur[last - 1], mid[m - 2]) {
                                    return None;
                                }
                            }
                            let p = price(&mid);
                            if p >= old - 1.0 {
                                return None;
                            }
                            let mut trial = cur[..i].to_vec();
                            trial.extend_from_slice(&mid);
                            trial.extend_from_slice(&cur[j + 1..]);
                            let trial = drop_collinear(&trial);
                            (full(&trial) < full_old - 1.0).then_some((p, fi, trial))
                        })
                        .collect();
                    cands.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
                    for (p, _, trial) in cands {
                        let snap = wo.clone();
                        let ok = self.refit_run(wo, r, trial.clone());
                        if dbg {
                            eprintln!("STRAIGHTEN wire {w} run {r} i {i} j {j}: old {old:.0} form {p:.0} legal {ok}");
                        }
                        if ok {
                            if !joined(&snap, wo) {
                                if dbg {
                                    eprintln!("STRAIGHTEN wire {w} run {r} i {i} j {j}: refused by the split guard");
                                }
                                let back = self.restore_wire(wo, &snap);
                                *wo = back;
                                continue;
                            }
                            cur = trial;
                            n += 1;
                            done = true;
                            break;
                        }
                    }
                    if done {
                        break;
                    }
                }
                if !done {
                    i += 1;
                }
            }
        }
        n
    }
}

/// The polyline `pts` without its interior vertices that lie on the straight line between their
/// kept neighbours (within 1 um, between them) or on the previous kept vertex: the same copper.
/// O(points).
pub fn drop_collinear(pts: &[Vec2]) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::with_capacity(pts.len());
    for (k, &p) in pts.iter().enumerate() {
        if let Some(&last) = out.last() {
            if (p - last).len() < 1_000.0 && k + 1 < pts.len() {
                continue;
            }
        }
        if out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let ab = p - a;
            let t = (b - a).dot(ab) / ab.len2().max(1.0);
            if t > 0.0 && t < 1.0 && (a + ab * t - b).len() < 1_000.0 {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

/// One bend that a bundle takes together (M-45 item 4): at `at` the inner wire turns by `deg`,
/// and a wire of another net runs parallel outside it at centre distance `d_nm` on both legs.
/// The outer wire is `skew_nm` = `2 d tan(deg / 2)` longer there (0.83 x pitch per 45 degree
/// bend, the plan's diff-pair note); bends of the opposite sense cancel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BundleBend {
    pub at: Vec2,
    pub deg: f64,
    pub d_nm: f64,
    pub skew_nm: f64,
    /// +1 for a left turn, -1 for a right turn (for cancelling pairs).
    pub sense: f64,
}

/// The bundle bends of the drawn runs `runs` = `(wire, net, layer, polyline)`: every bend of a
/// run with a wire of another net parallel outside it on both legs within `1.5 x pitch`
/// (`pitch(wire)`, centre to centre). O(bends x segments of the layer).
pub fn bundle_bends(runs: &[(usize, i32, usize, &[Vec2])], pitch: &dyn Fn(usize) -> f64) -> Vec<BundleBend> {
    let mut segs: Vec<(usize, i32, usize, Vec2, Vec2)> = Vec::new();
    for &(w, net, l, pts) in runs {
        for p in pts.windows(2) {
            if (p[1] - p[0]).len2() > 1.0 {
                segs.push((w, net, l, p[0], p[1]));
            }
        }
    }
    // Perpendicular offset of segment (c, e) from the line through a with unit direction u, if
    // parallel and overlapping the stretch [s0, s1] of that line near the bend.
    let par = |a: Vec2, u: Vec2, s0: f64, s1: f64, c: Vec2, e: Vec2| -> Option<f64> {
        let d = e - c;
        let l = d.len();
        if l < 1.0 || (d.cross(u) / l).abs() > 1e-4 {
            return None;
        }
        let (t0, t1) = ((c - a).dot(u), (e - a).dot(u));
        let (lo, hi) = (t0.min(t1), t0.max(t1));
        (hi > s0 && lo < s1).then(|| u.cross(c - a))
    };
    let mut out = Vec::new();
    for &(w, net, l, pts) in runs {
        let pts: Vec<Vec2> = {
            let mut v = pts.to_vec();
            v.dedup_by(|a, b| (*a - *b).len2() < 1.0);
            v
        };
        let p0 = pitch(w);
        for i in 1..pts.len().saturating_sub(1) {
            let (a, v, b) = (pts[i - 1], pts[i], pts[i + 1]);
            let (Some(u1), Some(u2)) = ((v - a).unit(), (b - v).unit()) else { continue };
            let deg = turn_deg(u1, u2);
            if !(0.5..=135.5).contains(&deg) {
                continue;
            }
            let sense = if u1.cross(u2) > 0.0 { 1.0 } else { -1.0 };
            // Outside of the turn: right of the travel for a left turn.
            let side = -sense;
            let (l1, l2) = ((v - a).len(), (b - v).len());
            let reach = 1.5 * p0;
            let mut best: Option<f64> = None;
            for &(w2, n2, l2_, c, e) in &segs {
                if w2 == w || n2 == net || l2_ != l {
                    continue;
                }
                // Parallel to the incoming leg near its end, on the outside.
                let Some(o1) = par(v, u1, -l1.min(reach * 4.0), 0.0, c, e) else { continue };
                if o1 * side <= 0.0 || o1.abs() > reach {
                    continue;
                }
                // The same wire parallel to the outgoing leg at the same distance.
                let d = o1.abs();
                let found = segs.iter().any(|&(w3, _, l3, c2, e2)| {
                    w3 == w2 && l3 == l && par(v, u2, 0.0, l2.min(reach * 4.0), c2, e2).is_some_and(|o2| o2 * side > 0.0 && (o2.abs() - d).abs() <= 0.05 * d + 1_000.0)
                });
                if found && best.is_none_or(|x| d < x) {
                    best = Some(d);
                }
            }
            if let Some(d) = best {
                let half = (deg * 0.5).to_radians();
                out.push(BundleBend { at: v, deg, d_nm: d, skew_nm: 2.0 * d * half.tan(), sense });
            }
        }
    }
    out
}

#[cfg(test)]
mod collinear_tests {
    use super::*;

    #[test]
    fn collinear_and_repeated_vertices_are_dropped_the_ends_stay() {
        let v = |x: f64, y: f64| Vec2 { x: x * 1e6, y: y * 1e6 };
        assert_eq!(drop_collinear(&[v(0.0, 0.0), v(1.0, 0.0), v(2.0, 0.0), v(2.0, 1.0)]), vec![v(0.0, 0.0), v(2.0, 0.0), v(2.0, 1.0)]);
        assert_eq!(drop_collinear(&[v(0.0, 0.0), v(1.0, 1.0), v(1.0, 1.0), v(2.0, 2.0)]), vec![v(0.0, 0.0), v(2.0, 2.0)]);
        assert_eq!(drop_collinear(&[v(0.0, 0.0), v(1.0, 1.0), v(2.0, 0.0)]), vec![v(0.0, 0.0), v(1.0, 1.0), v(2.0, 0.0)], "a bend stays");
        assert_eq!(drop_collinear(&[v(0.0, 0.0), v(2.0, 0.0), v(1.0, 0.0)]), vec![v(0.0, 0.0), v(2.0, 0.0), v(1.0, 0.0)], "a reversal stays");
    }
}
