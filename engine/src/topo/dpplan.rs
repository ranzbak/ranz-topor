//! M4 (shopping list 5.7): differential pairs decided in the topology, the planner part. Design
//! note: `docs/progress/notes/m4-pairs-planner.md`.
//!
//! Three decisions, all made in the cheap topological phase (the drawing only realises them):
//!
//! * **Gateways and fan-out zones** ([`plan_fan`], [`plan_ends`]): every pair end gets the KiCad
//!   fan-out (docs/dev/kicad-diff-pairs.md 3.2, user rule 2026-11-08): the P and N tracks leave
//!   their pads straight in the pads' exit direction, run out until the two symmetric 45-degree
//!   convergence legs and the first coupled stretch keep every clearance (graded: the least
//!   violating run-out when none is clean, never dropped), and meet at the pair pitch; that point
//!   is the gateway (TopoR's slider, 6.3 manual p. 171). The planning model keeps the fan-out free
//!   of other nets with the D7 bridge between partner pads (`[diffpair] fanout = "bridge"`,
//!   default) or, opt-in, with the fan-out tracks themselves as copper of the lead net
//!   (`"gateway"`, [`zone_shapes`]; in patterns dp_gateway and dp_bus_crossing those polygons bend
//!   the lead's own way into its pads, so it waits for the M4 drawing stage).
//! * **Polarity parity**: the handedness of the pair at the two gateways (N left or right of the
//!   travel direction leaving end A, and arriving at end B) is a planar invariant: on one layer a
//!   pair keeps it whatever its path (a ribbon in the plane cannot twist). A flip comes only from a
//!   via pair: at a layer change with turn angle theta, the two vias' axis can be set to keep the
//!   sides (axis square to the bisector) or to flip them (axis along the bisector, the turn-back of
//!   user rule 3 / pattern dp_uturn_parity). Each costs fan-out skew at the vias, graded with theta
//!   ([`turn_prices`]: keep `2 pv sin(theta/2)`, flip `2 pv cos(theta/2)`, `pv` the via pitch), so
//!   a flip is cheap at a turn-back and dear on a straight run; the cheapest assignment with the
//!   right parity is [`solve_parity`] (exact, O(vias)).
//! * **Swap sites priced against two singles** ([`plan_ends`]): a connection whose parity needs a
//!   flip and whose ends share a layer (no layer change is forced) gets either a swap (user rule
//!   3b: two via pairs, one to turn back and one to return to the surface, 4 vias, a sideways U
//!   excursion) at its cheapest site along the gateway-to-gateway line, priced by vias, excursion
//!   length, crowding (`via_squeeze`) and clearance breaks against the static board (graded, never
//!   a cut-off), or two single tracks (uncoupled over the whole connection, the N track hops the P
//!   track: two vias, unpaired, one crossing). `[diffpair] swap`: `via_pair` = the cheaper (user
//!   2026-11-09), `via_pair_always` = the swap at any price, `single` = always two single tracks.
//!
//! Units nm (model frame), prices in the planner's mm. Deterministic (B-tree maps, index order).

use super::diffpair::{pad_corners, Coupled, Pairs};
use super::draw::{track_item, via_item};
use super::factors::{Event, Registry, WireCtx};
use crate::check::{Checker, ItemIndex};
use crate::decompose::{Decomposition, TermKind, TermRef};
use crate::geom::Vec2;
use crate::model::{BoardModel, Item, Options};
use crate::vias::ViaSize;
use std::collections::BTreeMap;

const MM: f64 = 1e6;

/// One planned pair end: the fan-out from the pad pair to the gateway.
#[derive(Clone, Debug)]
pub struct Fan {
    pub pad_p: usize,
    pub pad_n: usize,
    /// Exit direction (unit) and whether it comes from the pads (their long axis away from the
    /// footprint body) rather than the fallback (square to the pad pair, toward the far end).
    pub x: Vec2,
    pub derived: bool,
    /// Gateway centre and the P / N track points there.
    pub g: Vec2,
    pub gp: Vec2,
    pub gn: Vec2,
    /// The planned tracks, pad centre to gateway point.
    pub p_line: Vec<Vec2>,
    pub n_line: Vec<Vec2>,
    /// Model layers the fan-out may lie on (the layers both pads have).
    pub layers: Vec<u32>,
    /// Straight run-out (nm) from the pad-pair middle before the convergence starts.
    pub run_out: f64,
    /// Clearance breaks of the planned fan-out against the static board `(count, depth nm)`.
    pub viol: (usize, f64),
}

impl Fan {
    /// Which side N lies on when the pair leaves this end along `x` (+1 = left of the travel).
    pub fn n_side_out(&self) -> f64 {
        if self.x.cross(self.gn - self.gp) >= 0.0 {
            1.0
        } else {
            -1.0
        }
    }
}

/// How a pair connection gets its polarity right (decided by [`plan_ends`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// P arrives on its side without a flip.
    Keep,
    /// A flip is needed and the ends lie on different layers: one of the plan's own via pairs
    /// turns back ([`solve_parity`] after the via sites are known).
    FlipAtLayerChange,
    /// A flip is needed and no layer change is forced: a swap (two via pairs) at [`ConnPlan::swap`].
    Swap,
    /// Routed as two single tracks (the swap cost more, or `[diffpair] swap = "single"`).
    Single,
    /// A flip is needed and nothing can provide one (one signal layer): P and N cross (priced).
    Cross,
}

/// A planned swap: the pair leaves its line sideways on the leg layer up to via pair A, turns back
/// on the other layer, comes down shifted by `s` to via pair B and rejoins its line on the leg
/// layer (pattern dp_swap_needed).
#[derive(Clone, Debug)]
pub struct SwapSite {
    /// Foot of the up leg on the gateway line, the line direction and the excursion side (units).
    pub at: Vec2,
    pub t: Vec2,
    pub side: Vec2,
    /// Excursion height of via pair A (centre), of via pair B, and the shift between the legs.
    pub h: f64,
    pub hb: f64,
    pub s: f64,
    pub leg: u32,
    pub to: u32,
    /// The four vias (net, centre): A P, A N, B P, B N; the leg tracks and vias as P and N items.
    pub vias: [(i32, Vec2); 4],
    pub p_items: Vec<Item>,
    pub n_items: Vec<Item>,
    /// Clearance breaks against the static board `(count, depth nm)`, crowding (fully squeezed via
    /// neighbours), excursion length of the centre line (mm) and the site's price.
    pub viol: (usize, f64),
    pub crowd: f64,
    pub mm: f64,
    pub price: f64,
}

/// The polarity plan of one pair connection.
#[derive(Clone, Debug)]
pub struct ConnPlan {
    /// Index into `Pairs::coupled`, P / N connection ids, the P pads of the ends.
    pub k: usize,
    pub conn_p: usize,
    pub conn_n: usize,
    pub pad_a: usize,
    pub pad_b: usize,
    pub fa: Option<Fan>,
    pub fb: Option<Fan>,
    /// The handedness differs between the ends: an odd number of flips is needed.
    pub need: bool,
    /// The ends share no signal layer: the plan changes layer at least once.
    pub forced: bool,
    pub choice: Choice,
    pub swap: Option<SwapSite>,
    /// Prices of the two options (planner mm; `INFINITY` = not priced).
    pub swap_price: f64,
    pub single_price: f64,
    /// A planned swap the drawing cannot realise yet ([`SWAPS_DRAWN`]): routed as two single
    /// tracks, the swap kept as the plan's decision.
    pub drawn_single: bool,
}

/// Whether the drawing realises a planned swap (the U excursion with two via pairs). Not before
/// the rebuilt ERBS (M4 drawing stage): until then a planned swap is routed as two single tracks,
/// which draw legal copper, instead of a coupled pair whose tracks cross at an end.
pub const SWAPS_DRAWN: bool = false;

/// Whether the drawing draws a pair end from its planned gateway (a reverse exit with its turn-back
/// included). Not before the M4 drawing stage: the drawing takes the pair's handedness from its
/// own zip (`pick_zip`), which follows the preferred exits, so a reverse exit that would change
/// the pair's parity is drawn with P and N crossed (a short). Until then the plan keeps the
/// preferred exits' parity and only reports the cheaper reverse exit.
pub const REVERSE_EXITS_DRAWN: bool = false;

/// Fan-out skew of a via pair at a layer change whose path turns by `theta` (radians, 0 =
/// straight on, pi = turn-back), via pitch `pv` (nm): `(keep, flip)` in nm. The vias' axis is
/// square to the incoming leg rotated by `phi`; each leg's two tracks then differ by `pv sin phi`.
/// Keep: axis along the left normal of `din + dout`, `phi = theta / 2` on both sides. Flip: along
/// the left normal of `din - dout`, `phi = 90 deg - theta / 2`. Smooth in theta, equal at 90 deg.
pub fn turn_prices(theta: f64, pv: f64) -> (f64, f64) {
    let h = 0.5 * theta.clamp(0.0, std::f64::consts::PI);
    (2.0 * pv * h.sin(), 2.0 * pv * h.cos())
}

/// Turn angle (radians, 0..pi) from `din` to `dout`.
pub fn turn_angle(din: Vec2, dout: Vec2) -> f64 {
    match (din.unit(), dout.unit()) {
        (Some(a), Some(b)) => a.dot(b).clamp(-1.0, 1.0).acos(),
        _ => 0.0,
    }
}

/// The cheapest flip / keep per via pair (`prices[i] = (keep, flip)`) such that the number of
/// flips is odd exactly when `need`: every via pair takes its cheaper option, and when the parity
/// is then wrong the one with the smallest difference changes. Returns the flips, the total and
/// whether the parity could be met (not without via pairs). O(n).
pub fn solve_parity(need: bool, prices: &[(f64, f64)]) -> (Vec<bool>, f64, bool) {
    let mut flips: Vec<bool> = prices.iter().map(|&(k, f)| f < k).collect();
    let mut total: f64 = prices.iter().map(|&(k, f)| k.min(f)).sum();
    let odd = flips.iter().filter(|&&f| f).count() % 2 == 1;
    if odd == need {
        return (flips, total, true);
    }
    let best = prices.iter().enumerate().map(|(i, &(k, f))| (i, (k - f).abs())).min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    match best {
        Some((i, d)) => {
            flips[i] = !flips[i];
            total += d;
            (flips, total, true)
        }
        None => (flips, total, false),
    }
}

fn unit(v: Vec2) -> Vec2 {
    v.unit().unwrap_or(Vec2 { x: 1.0, y: 0.0 })
}

fn clean(pts: &[Vec2]) -> Vec<Vec2> {
    let mut o: Vec<Vec2> = Vec::with_capacity(pts.len());
    for &p in pts {
        if o.last().is_none_or(|q: &Vec2| (p - *q).len() > 1.0) {
            o.push(p);
        }
    }
    o
}

fn tracks(net: i32, layer: u32, w: f64, pts: &[Vec2]) -> Vec<Item> {
    pts.windows(2).filter(|s| (s[1] - s[0]).len() > 1.0).map(|s| track_item(net, layer, w, s[0], s[1], None)).collect()
}

/// Signal layers (model ids, routable) of `pad`.
fn pad_signal_layers(model: &BoardModel, pad: &crate::model::Pad) -> Vec<u32> {
    model.layers.iter().filter(|l| l.routable && pad.layers.contains(&l.id)).map(|l| l.id).collect()
}

/// The long axis of `pad` (unit) when it is elongated (long side over 1.2 x the short one).
fn long_axis(pad: &crate::model::Pad, layer: u32) -> Option<Vec2> {
    let c = pad_corners(pad, layer);
    if c.len() != 4 {
        return None;
    }
    let (a, b) = (c[1] - c[0], c[2] - c[1]);
    let (la, lb) = (a.len(), b.len());
    if la > 1.2 * lb {
        a.unit()
    } else if lb > 1.2 * la {
        b.unit()
    } else {
        None
    }
}

/// `v` snapped to the nearest multiple of 45 degrees when within 3 degrees of it.
fn snap45(v: Vec2) -> Vec2 {
    let a = v.y.atan2(v.x);
    let q = (a / std::f64::consts::FRAC_PI_4).round() * std::f64::consts::FRAC_PI_4;
    if (a - q).abs() < 3f64.to_radians() {
        Vec2 { x: q.cos(), y: q.sin() }
    } else {
        v
    }
}

/// The exit direction of the pad pair `(pp, pn)`: the pads' common long axis pointing away from
/// the footprint body (the mean of its pad centres, footprints of three or more pads); round or
/// square pads, or pads one behind the other along their axis: square to the pad pair (KiCad's
/// `DP_GATEWAYS::BuildFromPrimitivePair`, doc 3.2). Without a body to point away from, toward
/// `toward` (the far end; `derived` false). O(1) with the body map. Ported from D7b (c24c67f).
pub fn exit_dir(model: &BoardModel, body: &BTreeMap<&str, (Vec2, usize)>, pp: usize, pn: usize, toward: Vec2) -> (Vec2, bool) {
    let (a, b) = (&model.pads[pp], &model.pads[pn]);
    let (ca, cb) = (a.center.to_vec2(), b.center.to_vec2());
    let m = (ca + cb) * 0.5;
    let pair = unit(cb - ca);
    let la = pad_signal_layers(model, a).first().copied().unwrap_or(0);
    let lb = pad_signal_layers(model, b).first().copied().unwrap_or(0);
    let ax = match (long_axis(a, la), long_axis(b, lb)) {
        (Some(u), Some(v)) if u.dot(v).abs() > 0.9 && u.dot(pair).abs() < 0.7 => u,
        _ => pair.perp(),
    };
    let mut away = Vec2::default();
    for r in [&a.ref_, &b.ref_] {
        if let Some(&(c, k)) = body.get(r.as_str()) {
            if k >= 3 {
                away = away + (m - c);
            }
        }
    }
    if let Some(u) = away.unit().filter(|u| u.dot(ax).abs() > 0.3) {
        let x = if u.dot(ax) >= 0.0 { ax } else { -ax };
        return (snap45(x), true);
    }
    let x = if (toward - m).dot(ax) >= 0.0 { ax } else { -ax };
    (snap45(x), false)
}

/// Rounding of quantised probe geometry (nm) below which a clearance shortfall is no break.
const ROUNDING_NM: i64 = 1_000;

/// Clearance breaks `(count, depth nm)` of P items `pi` and N items `ni` against the static board
/// (the checker's model) and the copper in `base`, and of N against P at the pair gap.
pub fn static_viol(checker: &Checker, base: &ItemIndex, pi: &[Item], ni: &[Item]) -> (usize, f64) {
    let mut px = ItemIndex::new(checker.cell());
    for it in pi {
        px.insert(it);
    }
    let (mut n, mut d) = (0, 0.0);
    for (items, ix, only_items) in [(pi, base, false), (ni, base, false), (ni, &px, true)] {
        for v in checker.check_against(ix, items, &[]).violations {
            if only_items && !v.against.starts_with("item:") {
                continue;
            }
            // The nm rounding of quantised geometry built at an exact fit (a mitred offset at the
            // pair gap) is no break.
            if v.required - v.gap <= ROUNDING_NM {
                continue;
            }
            n += 1;
            d += (v.required - v.gap).max(1) as f64;
        }
    }
    (n, d)
}

/// Crowding around the vias `vias` (graded, the `via_squeeze` amount): every foreign object that
/// leaves a via less than one via diameter of slack past its clearance adds `(d - slack) / d`, once
/// per object and layer. O(vias x checker query).
pub fn crowding(checker: &Checker, base: &ItemIndex, vias: &[(i32, Vec2)], size: ViaSize) -> f64 {
    let d = size.d as f64;
    let grown = ViaSize { d: size.d * 3, drill: size.drill };
    let mut sum = 0.0;
    for &(net, at) in vias {
        let it = via_item(net, at, grown, None);
        let mut worst: BTreeMap<(String, u32), f64> = BTreeMap::new();
        for v in checker.check_against(base, &[it], &[]).violations {
            let slack = (v.gap as f64 + d - v.required as f64).max(0.0);
            let e = worst.entry((v.against.clone(), v.layer)).or_insert(f64::INFINITY);
            *e = e.min(slack);
        }
        sum += worst.values().map(|&s| super::factors::squeeze_amount(s, d)).sum::<f64>();
    }
    sum
}

/// The fan-out of the pad pair `(pp, pn)` leaving along `x`: the shortest straight run-out (25 um
/// steps, up to 3 mm or 3 pad lengths past the pads) after which the two 45-degree convergence
/// legs and the first coupled stretch keep every clearance on each of `layers`; graded: the least
/// violating when none is clean. O(candidates x checker queries). Ported from D7b (c24c67f).
#[allow(clippy::too_many_arguments)]
pub fn plan_fan(model: &BoardModel, checker: &Checker, fixed_ix: &ItemIndex, cp: &Coupled, pp: usize, pn: usize, x: Vec2, derived: bool, layers: Vec<u32>) -> Fan {
    let (w, p) = (cp.rule.width as f64, cp.pitch());
    let hp = 0.5 * p + 2.0;
    let (cpad, npad) = (model.pads[pp].center.to_vec2(), model.pads[pn].center.to_vec2());
    let m = (cpad + npad) * 0.5;
    let n = x.perp();
    let (ap, bp) = ((cpad - m).dot(x), (cpad - m).dot(n));
    let sp = if bp >= 0.0 { 1.0 } else { -1.0 };
    let conv = (bp.abs() - hp).abs();
    let long = super::diffpair::pad_long(&model.pads[pp]).max(super::diffpair::pad_long(&model.pads[pn]));
    // The run-out starts at the pads' ends along the exit (half the pad's extent along `x`).
    let base = ap.abs().max(0.5 * long * x_extent(&model.pads[pp], x));
    let cap = base + (3.0 * MM).max(3.0 * long);
    let at = |a: f64, b: f64| m + x * a + n * b;
    let build = |a0: f64| {
        let ag = a0 + conv;
        (clean(&[cpad, at(a0, bp), at(ag, sp * hp)]), clean(&[npad, at(a0, -bp), at(ag, -sp * hp)]), ag)
    };
    let ahead = p + 1.5 * w + 20_000.0;
    let mut best: Option<((usize, f64), f64)> = None;
    let mut a0 = base;
    while a0 <= cap + 1.0 {
        let (mut pe, mut ne, ag) = build(a0);
        pe.push(at(ag + ahead, sp * hp));
        ne.push(at(ag + ahead, -sp * hp));
        let mut v = (0usize, 0.0f64);
        for &l in &layers {
            let (a, b) = static_viol(checker, fixed_ix, &tracks(cp.rule.p, l, w, &pe), &tracks(cp.rule.n, l, w, &ne));
            v = (v.0 + a, v.1 + b);
        }
        if best.as_ref().is_none_or(|b| v.0 < b.0 .0 || (v.0 == b.0 .0 && v.1 < b.0 .1 - 1.0)) {
            best = Some((v, a0));
        }
        if v.0 == 0 {
            break;
        }
        a0 += 25_000.0;
    }
    let (viol, a0) = best.unwrap_or(((0, 0.0), base));
    let (p_line, n_line, ag) = build(a0);
    Fan { pad_p: pp, pad_n: pn, x, derived, g: at(ag, 0.0), gp: at(ag, sp * hp), gn: at(ag, -sp * hp), p_line, n_line, layers, run_out: a0, viol }
}

/// Share (0..1) of a pad's long side that lies along `x` (1 for a pad elongated along `x`).
fn x_extent(pad: &crate::model::Pad, x: Vec2) -> f64 {
    let c = pad_corners(pad, pad.layers.first().copied().unwrap_or(0));
    if c.len() != 4 {
        return 1.0;
    }
    let (a, b) = (c[1] - c[0], c[2] - c[1]);
    let l = a.len().max(b.len()).max(1.0);
    (a.dot(x).abs().max(b.dot(x).abs()) / l).clamp(0.0, 1.0)
}

/// The turn-back of the reverse exit `r` of a pad pair (KiCad builds gateways on every side, doc
/// 3.2): the pair leaves its pads away from the far end, turns 180 degrees and passes the pad pair
/// on the cheaper side to the point abreast of the preferred gateway `g_pref`. Returns the graded
/// clearance breaks of its P and N tracks on the fan's layers and its length (nm): a turn-back past
/// a pad row's neighbours is costly, not forbidden. O(sides x layers x checker queries).
fn turn_back(model: &BoardModel, checker: &Checker, base: &ItemIndex, cp: &Coupled, r: &Fan, g_pref: Vec2, clear: f64) -> ((usize, f64), f64) {
    let (w, p) = (cp.rule.width as f64, cp.pitch());
    let n = r.x.perp();
    let (ca, cb) = (model.pads[r.pad_p].center.to_vec2(), model.pads[r.pad_n].center.to_vec2());
    let m = (ca + cb) * 0.5;
    let mut ext = 0.0f64;
    for q in [r.pad_p, r.pad_n] {
        let pad = &model.pads[q];
        for c in pad_corners(pad, pad.layers.first().copied().unwrap_or(0)) {
            ext = ext.max((c - m).dot(n).abs());
        }
    }
    // Lateral offset of the pair's centre line: past the pads, a clearance and half the ribbon.
    let lat = ext + clear + 0.5 * p + 0.5 * w + 50_000.0;
    let len = lat + (r.g - g_pref).len();
    let mut best: Option<(usize, f64)> = None;
    for s in [1.0, -1.0] {
        let centre = [r.g, r.g + n * (s * lat), g_pref + n * (s * lat)];
        let (a, b) = (super::diffpair::offset(&centre, 0.5 * p), super::diffpair::offset(&centre, -0.5 * p));
        if a.len() < 2 || b.len() < 2 {
            continue;
        }
        let (pl, nl) = if (a[0] - r.gp).len() <= (a[0] - r.gn).len() { (a, b) } else { (b, a) };
        let mut v = (0usize, 0.0f64);
        for &l in &r.layers {
            let (x, y) = static_viol(checker, base, &tracks(cp.rule.p, l, w, &pl), &tracks(cp.rule.n, l, w, &nl));
            v = (v.0 + x, v.1 + y);
        }
        if best.is_none_or(|b| v.0 < b.0 || (v.0 == b.0 && v.1 < b.1)) {
            best = Some(v);
        }
    }
    (best.unwrap_or((0, 0.0)), len)
}

/// The fan-out tracks of `f` as copper polygons, one per track (the track's outline at the pair
/// width), the zone reserved for the pair in the planning model. Each starts where its run-out
/// leaves its pad, so no polygon overlaps a pad or another one (the projection's CDT nodes
/// overlapping obstacles into slivers that bend the lead's own way to its pad). O(segments).
pub fn zone_shapes(model: &BoardModel, f: &Fan, w: f64) -> Vec<Vec<Vec2>> {
    let mut out = Vec::new();
    for (line, pad) in [(&f.p_line, f.pad_p), (&f.n_line, f.pad_n)] {
        if line.len() < 2 {
            continue;
        }
        let pad = &model.pads[pad];
        let c = pad.center.to_vec2();
        // Half the pad's extent along the run-out (its corners projected on `x`).
        let corners = pad_corners(pad, pad.layers.first().copied().unwrap_or(0));
        let half = corners.iter().map(|&q| (q - c).dot(f.x)).fold(0.0f64, f64::max);
        let mut pts = line.clone();
        let Some(u) = (pts[1] - pts[0]).unit() else { continue };
        let along = (pts[1] - pts[0]).len();
        if half >= along - 1.0 {
            pts.remove(0);
            if pts.len() < 2 {
                continue;
            }
        } else {
            pts[0] = pts[0] + u * half;
        }
        let left = super::diffpair::offset(&pts, 0.5 * w);
        let mut right = super::diffpair::offset(&pts, -0.5 * w);
        if left.len() < 2 || right.len() < 2 {
            continue;
        }
        right.reverse();
        out.push(left.into_iter().chain(right).collect());
    }
    out
}

/// The swap footprint at line point `c` (direction `t`, excursion side `side`), the geometry of
/// pattern dp_swap_needed (harness/diffpair_patterns.py `s1u_swap`, `swap_footprint_min`): the
/// pair leaves the line on `leg` up to via pair A (axis along `t`, height `h`), turns back on `to`
/// (the via fan, a straight, a 45-degree shift by `s`, a short tail, the via fan) down to via pair B (axis along `t`, height
/// `hb`) and returns to the line on `leg`. Physically P keeps one side along `t` on both legs; the
/// turn-back is the swap. `s` keeps via pair B clear of the up leg's near track, `hb` keeps it
/// clear of the line, the fan length `f = (pv - p) / 2 + w` is the via fan of the pattern (0.375 mm
/// at the pattern's values). O(1).
#[allow(clippy::too_many_arguments)]
fn swap_at(cp: &Coupled, size: ViaSize, clear: f64, c: Vec2, t: Vec2, side: Vec2, leg: u32, to: u32, n_left: bool) -> SwapSite {
    let (w, p) = (cp.rule.width as f64, cp.pitch());
    let vd = size.d as f64;
    let pv = vd + cp.rule.via_gap as f64;
    let f = 0.5 * (pv - p) + w;
    // 50 um slack past each clearance, as the pattern builds it (s = 1.1 mm for s_min 1.05 mm), so
    // the probe does not count a break that is only the nm rounding of an exact fit.
    let slack = 50_000.0;
    let s = 0.5 * pv + 0.5 * vd + clear + slack + 0.5 * w + 0.5 * p;
    let hb = 0.5 * p + clear + slack + 0.5 * w + 0.5 * vd + 0.5 * pv;
    let h = hb + 3.0 * f + s + 0.5 * w;
    let a = c + side * h;
    let b = c + t * s + side * hb;
    // P right of the travel along `t` when N is left; the up leg keeps that side relative to its
    // travel, the down leg (reversed travel) the other one: physically the same side along `t`.
    let sn = if n_left { 1.0 } else { -1.0 };
    let pside = side.perp() * (-sn);
    let (pa, na) = (a + pside * (0.5 * pv), a - pside * (0.5 * pv));
    let (pb, nb) = (b + pside * (0.5 * pv), b - pside * (0.5 * pv));
    let up = [c, a - side * f];
    let down = [a - side * f, a - side * (2.0 * f), a + t * s - side * (2.0 * f + s), b + side * f];
    let ret = [b - side * f, c + t * s];
    let mut p_items = tracks(cp.rule.p, leg, w, &[super::diffpair::offset(&up, -0.5 * p * sn), vec![pa]].concat());
    p_items.extend(tracks(cp.rule.p, to, w, &[vec![pa], super::diffpair::offset(&down, 0.5 * p * sn), vec![pb]].concat()));
    p_items.extend(tracks(cp.rule.p, leg, w, &[vec![pb], super::diffpair::offset(&ret, 0.5 * p * sn)].concat()));
    p_items.push(via_item(cp.rule.p, pa, size, None));
    p_items.push(via_item(cp.rule.p, pb, size, None));
    let mut n_items = tracks(cp.rule.n, leg, w, &[super::diffpair::offset(&up, 0.5 * p * sn), vec![na]].concat());
    n_items.extend(tracks(cp.rule.n, to, w, &[vec![na], super::diffpair::offset(&down, -0.5 * p * sn), vec![nb]].concat()));
    n_items.extend(tracks(cp.rule.n, leg, w, &[vec![nb], super::diffpair::offset(&ret, -0.5 * p * sn)].concat()));
    n_items.push(via_item(cp.rule.n, na, size, None));
    n_items.push(via_item(cp.rule.n, nb, size, None));
    let vias = [(cp.rule.p, pa), (cp.rule.n, na), (cp.rule.p, pb), (cp.rule.n, nb)];
    let mm = (2.0 * h + (std::f64::consts::SQRT_2 - 2.0) * s) / MM;
    SwapSite { at: c, t, side, h, hb, s, leg, to, vias, p_items, n_items, viol: (0, 0.0), crowd: 0.0, mm, price: 0.0 }
}

/// The CRITICAL report line of a pair connection whose drawn P and N tracks cross between its ends
/// (a short where they cross; a survivor of the polarity price, never dropped), with what the plan
/// decided (`pl`) so the cause is visible: a plan that wanted no flip and a drawing that crosses is
/// the plan-vs-drawn gap of the drawing stage. O(1).
pub fn critical_polarity_line(p: &str, n: &str, a: &str, b: &str, pl: Option<&ConnPlan>) -> String {
    let why = match pl {
        None => "the connection had no polarity plan".to_string(),
        Some(pl) if !pl.need && pl.fa.is_none() => "the plan's direct run from pad pair to pad pair (ends too close for two gateways) needs no flip: the drawn ends leave their pads on other sides (plan-vs-drawn gap, M4 drawing stage)".to_string(),
        Some(pl) if !pl.need => "the plan's gateways need no flip: the drawn ends leave their pads on other sides than the planned gateways (plan-vs-drawn gap, M4 drawing stage)".to_string(),
        Some(pl) => match pl.choice {
            Choice::Swap => "the plan wanted a swap (two via pairs) that the drawing does not realise yet (M4 drawing stage)".to_string(),
            Choice::Cross => "the board has one signal layer: the plan prices the crossing, there is no other way".to_string(),
            Choice::FlipAtLayerChange => "the plan wanted one of the pair's via pairs to turn back, the drawing did not fit it".to_string(),
            _ => "the plan wanted a flip that the drawing did not draw".to_string(),
        },
    };
    format!("CRITICAL: diff pair {p} / {n} {a} -> {b}: P and N cross between the ends (a short where the tracks cross): {why}; fix by hand, or route it as two single tracks ([diffpair] swap = \"single\")")
}

/// The P pad of a terminal.
fn pad_of(t: &TermRef) -> Option<usize> {
    if let TermKind::Pad(p) = t.kind {
        Some(p)
    } else {
        None
    }
}

/// Plan every pair connection's ends and polarity (module doc): fan-outs at both ends, the
/// parity, the swap or two-singles decision. Connections that become two single tracks move from
/// `pairs.lead` to `pairs.single`. `fixed` = copper placed before the topology (plane stubs).
/// O(pair ends x fan candidates x checker queries + swap candidates x checker queries).
pub fn plan_ends(model: &BoardModel, opts: &Options, checker: &Checker, fixed: &[Item], dec: &Decomposition, pairs: &mut Pairs, reg: &Registry) {
    let mut body: BTreeMap<&str, (Vec2, usize)> = BTreeMap::new();
    for p in &model.pads {
        let e = body.entry(p.ref_.as_str()).or_insert((Vec2::default(), 0));
        e.0 = e.0 + p.center.to_vec2();
        e.1 += 1;
    }
    for v in body.values_mut() {
        v.0 = v.0 * (1.0 / v.1.max(1) as f64);
    }
    let mut base = ItemIndex::new(checker.cell());
    for it in fixed {
        base.insert(it);
    }
    let mut uses: BTreeMap<usize, usize> = BTreeMap::new();
    for &ci in pairs.lead.keys() {
        let c = &dec.connections[ci];
        for t in [&c.from, &c.to] {
            if let Some(p) = pad_of(t) {
                *uses.entry(p).or_default() += 1;
            }
        }
    }
    let ctx = WireCtx::default();
    let price_mm = reg.price(&Event::Length { wire: 0, mm: 1.0, scale: 1.0 }, &ctx);
    let price_via = reg.price(&Event::Via { wire: 0, count: 1, at: None }, &ctx);
    let price_cross = reg.price(&Event::Crossing { a: 0, b: 1, tri: 0, same_net: false }, &ctx);
    let price_unpaired = reg.price(&Event::DpViaUnpaired { count: 1 }, &ctx);
    let brk = 10.0 * price_via.max(1.0);
    let routable: Vec<u32> = model.layers.iter().filter(|l| l.routable).map(|l| l.id).collect();
    let leads: Vec<(usize, usize, usize)> = pairs.lead.iter().map(|(&ci, &(k, cn))| (ci, k, cn)).collect();
    let mut fans: BTreeMap<(usize, i64, i64), Fan> = BTreeMap::new();
    let mut plans: BTreeMap<usize, ConnPlan> = BTreeMap::new();
    let mut exit_notes: Vec<String> = Vec::new();
    // Pass 1: the fan-outs of every pair end (shared ends: one fan-out per pad pair and direction).
    for &(ci, k, cn) in &leads {
        let cp = &pairs.coupled[k];
        let c = &dec.connections[ci];
        let (Some(a), Some(b)) = (pad_of(&c.from), pad_of(&c.to)) else { continue };
        let mid = |q: usize| (model.pads[q].center.to_vec2() + model.pads[cp.partner[&q]].center.to_vec2()) * 0.5;
        // Both exits of every end (KiCad builds gateways on every side of a pad pair, doc 3.2): the
        // preferred one (away from the body, or toward the far end) and the reverse, each priced by
        // its fan-out's clearance breaks (graded), its run-out and, for the reverse, the extra way
        // to the far end. The pair takes the cheapest combination, the parity included (a needed
        // flip on one layer costs at least two single tracks), so a pad pair on a module's rim
        // whose "away" side is the board edge leaves inward when that avoids a swap.
        let clear = model.rules.net_rule(cp.rule.p).clearance.max(model.rules.net_rule(cp.rule.n).clearance).max(model.rules.min_clearance) as f64;
        let la: Vec<u32> = pad_signal_layers(model, &model.pads[a]);
        let lb: Vec<u32> = pad_signal_layers(model, &model.pads[b]);
        let forced_ends = !la.iter().any(|l| lb.contains(l));
        let mut cands: [Vec<(Fan, f64)>; 2] = [Vec::new(), Vec::new()];
        for (slot, (q, other)) in [(a, b), (b, a)].into_iter().enumerate() {
            let qn = cp.partner[&q];
            // Both ends on one part (a flow-through part): the pads leave toward each other.
            let (x0, derived) = if model.pads[q].ref_ == model.pads[other].ref_ {
                let mut nb = body.clone();
                nb.remove(model.pads[q].ref_.as_str());
                exit_dir(model, &nb, q, qn, mid(other))
            } else {
                exit_dir(model, &body, q, qn, mid(other))
            };
            let mut layers = pad_signal_layers(model, &model.pads[q]);
            layers.retain(|l| model.pads[qn].layers.contains(l));
            if layers.is_empty() {
                continue;
            }
            let mut g0: Option<Vec2> = None;
            for (x, rev) in [(x0, false), (-x0, true)] {
                let key = (q, (x.x * 1e6) as i64, (x.y * 1e6) as i64);
                let f = match fans.get(&key) {
                    Some(f) => f.clone(),
                    None => {
                        let f = plan_fan(model, checker, &base, cp, q, qn, x, derived && !rev, layers.clone());
                        fans.insert(key, f.clone());
                        f
                    }
                };
                // The reverse exit adds its turn-back (graded breaks and length) to the fan-out.
                let g_pref = *g0.get_or_insert(f.g);
                let (tb, extra) = if rev { turn_back(model, checker, &base, cp, &f, g_pref, clear) } else { ((0, 0.0), 0.0) };
                let price = brk * ((f.viol.0 + tb.0) as f64 + (f.viol.1 + tb.1) / MM) + price_mm * (f.run_out + extra) / MM;
                cands[slot].push((f, price));
            }
        }
        let ends: [Option<Fan>; 2] = if cands[0].is_empty() || cands[1].is_empty() {
            [cands[0].first().map(|c| c.0.clone()), cands[1].first().map(|c| c.0.clone())]
        } else {
            let single_lb = 2.0 * price_via + 2.0 * price_unpaired + price_cross;
            let need_of = |i: usize, j: usize| cands[0][i].0.n_side_out() != -cands[1][j].0.n_side_out();
            let need0 = need_of(0, 0);
            let mut best: Option<(f64, usize, usize)> = None;
            let mut best_any: Option<(f64, usize, usize)> = None;
            for (i, (_, px)) in cands[0].iter().enumerate() {
                for (j, (_, py)) in cands[1].iter().enumerate() {
                    let need = need_of(i, j);
                    let p = px + py + if need && !forced_ends { single_lb } else { 0.0 };
                    // Ties keep the preferred exits (index 0 first).
                    if best_any.is_none_or(|b| p < b.0 - 1e-9) {
                        best_any = Some((p, i, j));
                    }
                    // Not drawn yet: the drawn handedness follows the preferred exits.
                    if (REVERSE_EXITS_DRAWN || need == need0) && best.is_none_or(|b| p < b.0 - 1e-9) {
                        best = Some((p, i, j));
                    }
                }
            }
            let (pb, i, j) = best.expect("candidates");
            if let Some((pa, ia, ja)) = best_any.filter(|b| (b.1, b.2) != (i, j)) {
                exit_notes.push(format!(
                    "diff pair {} / {}: leaving {}.{} {} and {}.{} {} would avoid the polarity flip for less ({pa:.1} against {pb:.1}), but the drawing does not draw from the planned gateways yet (M4 drawing stage): the plan keeps the drawn sides",
                    cp.rule.p_name,
                    cp.rule.n_name,
                    model.pads[a].ref_,
                    model.pads[a].number,
                    if ia == 0 { "as preferred" } else { "on the reverse side" },
                    model.pads[b].ref_,
                    model.pads[b].number,
                    if ja == 0 { "as preferred" } else { "on the reverse side" }
                ));
            }
            if i != 0 || j != 0 {
                exit_notes.push(format!(
                    "diff pair {} / {}: the pair leaves {}.{} {} and {}.{} {} (the reverse exit is cheaper: fewer clearance breaks, its turn-back included)",
                    cp.rule.p_name,
                    cp.rule.n_name,
                    model.pads[a].ref_,
                    model.pads[a].number,
                    if i == 0 { "as preferred" } else { "on the reverse side" },
                    model.pads[b].ref_,
                    model.pads[b].number,
                    if j == 0 { "as preferred" } else { "on the reverse side" }
                ));
            }
            [Some(cands[0][i].0.clone()), Some(cands[1][j].0.clone())]
        };
        let [mut fa, mut fb] = ends;
        if std::env::var_os("TOPO_DPDBG").is_some() {
            for f in [&fa, &fb].into_iter().flatten() {
                eprintln!(
                    "dpdbg fan pad {} x ({:.2},{:.2}) derived {} g ({:.3},{:.3}) run_out {:.3} viol {:?} p_line {:?} n_line {:?}",
                    f.pad_p,
                    f.x.x,
                    f.x.y,
                    f.derived,
                    f.g.x / MM,
                    f.g.y / MM,
                    f.run_out / MM,
                    f.viol,
                    f.p_line.iter().map(|q| ((q.x / 1e3).round() / 1e3, (q.y / 1e3).round() / 1e3)).collect::<Vec<_>>(),
                    f.n_line.iter().map(|q| ((q.x / 1e3).round() / 1e3, (q.y / 1e3).round() / 1e3)).collect::<Vec<_>>()
                );
            }
        }
        // Ends too close for two gateways (facing pads about a pad length apart): no fan-outs, the
        // pair runs pad pair to pad pair.
        if let (Some(x), Some(y)) = (&fa, &fb) {
            let room = 2.0 * (cp.pitch() + cp.rule.width as f64);
            let passed = (y.g - x.g).dot(x.x) < 0.0 && (x.g - y.g).dot(y.x) < 0.0;
            if passed || (y.g - x.g).len() < room {
                fa = None;
                fb = None;
            }
        }
        // Parity: N side leaving A, N side arriving at B (relative to the travel).
        let (need, forced) = {
            let (sa, sb) = match (&fa, &fb) {
                (Some(x), Some(y)) => (x.n_side_out(), -y.n_side_out()),
                _ => {
                    let d = unit(mid(b) - mid(a));
                    let side = |p: usize| if d.cross(model.pads[cp.partner[&p]].center.to_vec2() - model.pads[p].center.to_vec2()) >= 0.0 { 1.0 } else { -1.0 };
                    (side(a), side(b))
                }
            };
            let la: Vec<u32> = pad_signal_layers(model, &model.pads[a]);
            let lb: Vec<u32> = pad_signal_layers(model, &model.pads[b]);
            (sa != sb, !la.iter().any(|l| lb.contains(l)))
        };
        plans.insert(ci, ConnPlan { k, conn_p: ci, conn_n: cn, pad_a: a, pad_b: b, fa, fb, need, forced, choice: Choice::Keep, swap: None, swap_price: f64::INFINITY, single_price: f64::INFINITY, drawn_single: false });
    }
    pairs.notes.append(&mut exit_notes);
    // Pass 2: polarity. The fan-outs of every end and the swaps planned so far are what a swap
    // site must keep clear of besides the static board.
    let mut taken = ItemIndex::new(checker.cell());
    for it in fixed {
        taken.insert(it);
    }
    for pl in plans.values() {
        let cp = &pairs.coupled[pl.k];
        for f in [&pl.fa, &pl.fb].into_iter().flatten() {
            for &l in &f.layers {
                for it in tracks(cp.rule.p, l, cp.rule.width as f64, &f.p_line).into_iter().chain(tracks(cp.rule.n, l, cp.rule.width as f64, &f.n_line)) {
                    taken.insert(&it);
                }
            }
        }
    }
    let mode = opts.diffpair_swap.as_str();
    let cis: Vec<usize> = plans.keys().copied().collect();
    for ci in cis {
        let pl = plans[&ci].clone();
        let cp = pairs.coupled[pl.k].clone();
        let r = &cp.rule;
        let lbl = |q: usize| format!("{}.{}", model.pads[q].ref_, model.pads[q].number);
        let head = format!("diff pair {} / {} {} -> {}", r.p_name, r.n_name, lbl(pl.pad_a), lbl(pl.pad_b));
        if !pl.need {
            continue;
        }
        if pl.forced {
            plans.get_mut(&ci).expect("plan").choice = Choice::FlipAtLayerChange;
            pairs.notes.push(format!("{head}: P and N arrive on the other sides (polarity); the ends lie on different layers, so one of the pair's own via pairs turns back (the flip is priced at the planned via sites)"));
            continue;
        }
        if routable.len() < 2 {
            plans.get_mut(&ci).expect("plan").choice = Choice::Cross;
            pairs.notes.push(format!("{head}: P and N arrive on the other sides (polarity) and the board has one signal layer: the tracks cross (priced)"));
            continue;
        }
        let mid = |q: usize| (model.pads[q].center.to_vec2() + model.pads[cp.partner[&q]].center.to_vec2()) * 0.5;
        let ga = pl.fa.as_ref().map_or(mid(pl.pad_a), |f| f.g);
        let gb = pl.fb.as_ref().map_or(mid(pl.pad_b), |f| f.g);
        let len = (gb - ga).len();
        // Two single tracks: uncoupled over the whole connection, N hops P (2 unpaired vias, one
        // crossing of P and N).
        let single = reg.price(&Event::DpUncoupled { mm: len / MM, max_mm: r.max_uncoupled as f64 / MM }, &ctx) + 2.0 * price_via + 2.0 * price_unpaired + price_cross;
        // The swap: every site along the gateway line on both sides, 0.25 mm apart.
        let (vd, vdr) = super::diffpair::via_of(model, r, opts);
        let size = ViaSize { d: vd, drill: vdr };
        let clear = model.rules.net_rule(r.p).clearance.max(model.rules.net_rule(r.n).clearance).max(model.rules.min_clearance) as f64;
        let leg = pl.fa.as_ref().and_then(|f| f.layers.first().copied()).or_else(|| pad_signal_layers(model, &model.pads[pl.pad_a]).first().copied()).unwrap_or(routable[0]);
        let to = routable.iter().rev().copied().find(|&l| l != leg).unwrap_or(leg);
        let t = unit(gb - ga);
        let n_left = pl.fa.as_ref().map_or(true, |f| f.n_side_out() > 0.0);
        let squeeze = reg.price(&Event::ViaSqueeze { wire: 0, at: Vec2::default(), slack_nm: 0.0, via_nm: vd as f64 }, &ctx);
        let mut best: Option<SwapSite> = None;
        let step = 250_000.0;
        let n_steps = ((len / step).floor() as usize).max(1);
        for side in [t.perp(), -t.perp()] {
            for i in 0..=n_steps {
                let u = (i as f64 * step).min(len);
                let mut sw = swap_at(&cp, size, clear, ga + t * u, t, side, leg, to, n_left);
                sw.viol = static_viol(checker, &taken, &sw.p_items, &sw.n_items);
                sw.crowd = crowding(checker, &taken, &sw.vias, size);
                sw.price = 4.0 * price_via + sw.mm * price_mm + brk * sw.viol.0 as f64 + brk * sw.viol.1 / MM + squeeze * sw.crowd;
                if best.as_ref().is_none_or(|b| sw.price < b.price - 1e-9) {
                    best = Some(sw);
                }
            }
        }
        let lone = |q: usize| uses.get(&q).copied().unwrap_or(0) == 1;
        let sw = best.expect("at least one site");
        if std::env::var_os("TOPO_DPDBG").is_some() {
            let mut px = ItemIndex::new(checker.cell());
            for it in &sw.p_items {
                px.insert(it);
            }
            for (items, ix) in [(&sw.p_items, &taken), (&sw.n_items, &taken), (&sw.n_items, &px)] {
                for v in checker.check_against(ix, items, &[]).violations {
                    eprintln!("dpdbg swap site ({:.3},{:.3}) side ({:.2},{:.2}) break vs {} layer {} gap {} required {}", sw.at.x / MM, sw.at.y / MM, sw.side.x, sw.side.y, v.against, v.layer, v.gap, v.required);
                }
            }
        }
        let swap_price = sw.price;
        let take_single = mode == "single" || (mode == "via_pair" && swap_price > single && lone(pl.pad_a) && lone(pl.pad_b));
        {
            let e = plans.get_mut(&ci).expect("plan");
            e.swap_price = swap_price;
            e.single_price = single;
        }
        if take_single && lone(pl.pad_a) && lone(pl.pad_b) {
            if !to_single(model, pairs, ci, &pl) {
                continue;
            }
            plans.get_mut(&ci).expect("plan").choice = Choice::Single;
            pairs.notes.push(format!(
                "{head}: P and N arrive on the other sides (polarity) and no layer change is forced; the swap (two via pairs) at its cheapest site costs {swap_price:.1} ({} clearance breaks, crowding {:.2}), two single tracks {single:.1}: routed as two single tracks at the pair width ([diffpair] swap = \"{mode}\")",
                sw.viol.0, sw.crowd
            ));
            continue;
        }
        for it in sw.p_items.iter().chain(&sw.n_items) {
            taken.insert(it);
        }
        pairs.notes.push(format!(
            "{head}: P and N arrive on the other sides (polarity) and no layer change is forced: swap planned at ({:.3}, {:.3}) mm (two via pairs, excursion {:.2} mm, {} clearance breaks, crowding {:.2}; price {swap_price:.1} against two single tracks {single:.1}, [diffpair] swap = \"{mode}\"); the drawing realises swaps with the rebuilt ERBS (M4 drawing stage); until then a pair whose end pads serve this connection only is drawn as two single tracks, otherwise the drawn pair keeps its sides and the polarity cross is reported",
            sw.at.x / MM,
            -sw.at.y / MM,
            sw.mm,
            sw.viol.0,
            sw.crowd
        ));
        let drawn_single = !SWAPS_DRAWN && lone(pl.pad_a) && lone(pl.pad_b) && to_single(model, pairs, ci, &pl);
        if drawn_single {
            pairs.notes.push(format!("CRITICAL: {head}: the planned swap is drawn as two single tracks (uncoupled over the whole connection, one hop) until the drawing realises swaps (M4 drawing stage, rebuilt ERBS); route it by hand as a coupled pair with the swap if the coupling matters"));
        }
        let e = plans.get_mut(&ci).expect("plan");
        e.choice = Choice::Swap;
        e.swap = Some(sw);
        e.drawn_single = drawn_single;
    }
    pairs.conn_plan = plans;
}

/// Moves pair connection `ci` (plan `pl`) from the coupled lead to two single tracks: the N pads
/// of both ends keep their net. False when `ci` is no lead. O(pads of the pair).
fn to_single(model: &BoardModel, pairs: &mut Pairs, ci: usize, pl: &ConnPlan) -> bool {
    let Some((_, cn)) = pairs.lead.remove(&ci) else { return false };
    pairs.carried.remove(&cn);
    let r = pairs.coupled[pl.k].rule.clone();
    let (na, nb) = (pairs.coupled[pl.k].partner[&pl.pad_a], pairs.coupled[pl.k].partner[&pl.pad_b]);
    let ri = model.rules.diff_pairs.iter().position(|x| *x == r).unwrap_or(0);
    let kp = &mut pairs.coupled[pl.k];
    kp.conns.retain(|&(ip, _)| ip != ci);
    kp.keep.extend([na, nb]);
    if kp.conns.is_empty() {
        pairs.lead_nets.remove(&r.p);
    }
    pairs.single.push((ci, cn, ri));
    true
}

/// The planning model's reservations of the pairs' ends and swaps ([`zone_shapes`] and the swap
/// footprints) as copper shapes of the lead net on their layers. O(shapes).
pub fn reservations(model: &BoardModel, pairs: &Pairs) -> Vec<crate::model::CopperShape> {
    let mut out = Vec::new();
    let routable: Vec<u32> = model.layers.iter().filter(|l| l.routable).map(|l| l.id).collect();
    let pt = |q: Vec2| crate::model::Pt { x: q.x.round() as i64, y: q.y.round() as i64 };
    for (ci, pl) in &pairs.conn_plan {
        if !matches!(pl.choice, Choice::Keep | Choice::FlipAtLayerChange | Choice::Swap | Choice::Cross) || !pairs.lead.contains_key(ci) {
            continue;
        }
        let cp = &pairs.coupled[pl.k];
        let w = cp.rule.width as f64;
        for (e, f) in [&pl.fa, &pl.fb].into_iter().enumerate().filter_map(|(e, f)| f.as_ref().map(|f| (e, f))) {
            for &l in &f.layers {
                for (i, poly) in zone_shapes(model, f, w).into_iter().enumerate() {
                    out.push(crate::model::CopperShape { uuid: format!("dp-zone-{ci}-{e}-{l}-{i}"), net: cp.rule.p, layer: l, poly: poly.into_iter().map(pt).collect(), owner_ref: None });
                }
            }
        }
        // A planned swap's footprint (its legs and both via pairs) is the pair's room too.
        let Some(sw) = pl.swap.as_ref().filter(|_| pl.choice == Choice::Swap) else { continue };
        let mut i = 0;
        for it in sw.p_items.iter().chain(&sw.n_items) {
            match it {
                Item::Track(t) => {
                    if let crate::geom::quant::TrackPrim::Seg { a, b } = t.prim {
                        let (a, b) = (Vec2 { x: f64::from(a.x), y: -f64::from(a.y) }, Vec2 { x: f64::from(b.x), y: -f64::from(b.y) });
                        let Some(u) = (b - a).unit() else { continue };
                        let n = u.perp() * (0.5 * t.width as f64);
                        out.push(crate::model::CopperShape { uuid: format!("dp-swap-{ci}-{i}"), net: cp.rule.p, layer: t.layer, poly: [a - n, b - n, b + n, a + n].into_iter().map(pt).collect(), owner_ref: None });
                        i += 1;
                    }
                }
                Item::Via(v) => {
                    let c = Vec2 { x: v.at.x as f64, y: -(v.at.y as f64) };
                    let r = 0.5 * v.d as f64 / (std::f64::consts::PI / 8.0).cos();
                    let oct: Vec<Vec2> = (0..8).map(|j| c + Vec2 { x: r, y: 0.0 }.rot(std::f64::consts::FRAC_PI_4 * (j as f64 + 0.5))).collect();
                    for &l in &routable {
                        out.push(crate::model::CopperShape { uuid: format!("dp-swap-{ci}-{i}"), net: cp.rule.p, layer: l, poly: oct.iter().copied().map(pt).collect(), owner_ref: None });
                        i += 1;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DiffPairRule;

    #[test]
    fn turn_prices_are_graded_and_meet_at_ninety_degrees() {
        let pv = 700_000.0;
        let (k0, f0) = turn_prices(0.0, pv);
        assert!(k0.abs() < 1e-6 && (f0 - 2.0 * pv).abs() < 1e-6);
        let (k9, f9) = turn_prices(std::f64::consts::FRAC_PI_2, pv);
        assert!((k9 - f9).abs() < 1e-6);
        let (kp, fp) = turn_prices(std::f64::consts::PI, pv);
        assert!(fp.abs() < 1e-6 && (kp - 2.0 * pv).abs() < 1e-6);
        // Smooth: no jump anywhere.
        let mut last = turn_prices(0.0, pv);
        for i in 1..=180 {
            let x = turn_prices((i as f64).to_radians(), pv);
            assert!((x.0 - last.0).abs() < 0.02 * pv && (x.1 - last.1).abs() < 0.02 * pv);
            last = x;
        }
    }

    #[test]
    fn parity_takes_the_cheapest_flip() {
        // No via pairs: a needed flip cannot be met.
        assert!(!solve_parity(true, &[]).2);
        assert!(solve_parity(false, &[]).2);
        // One straight via (keep 0, flip 1.4) and one turn-back (keep 1.4, flip 0).
        let (f, t, ok) = solve_parity(true, &[(0.0, 1.4), (1.4, 0.0)]);
        assert!(ok && f == vec![false, true] && t.abs() < 1e-9);
        // No flip wanted: the turn-back keeps at its price.
        let (f, t, _) = solve_parity(false, &[(0.0, 1.4), (1.4, 0.0)]);
        assert_eq!(f.iter().filter(|&&x| x).count() % 2, 0);
        assert!((t - 1.4).abs() < 1e-9);
        // Two straight vias, one flip needed: one of them flips (the first on a tie).
        let (f, t, _) = solve_parity(true, &[(0.0, 1.4), (0.0, 1.4)]);
        assert_eq!(f, vec![true, false]);
        assert!((t - 1.4).abs() < 1e-9);
    }

    /// Edge-to-edge distance of two items on a shared layer (vias: every layer), segments only.
    fn gap(a: &Item, b: &Item) -> Option<f64> {
        fn geo(i: &Item) -> (Option<u32>, Vec2, Vec2, f64) {
            match i {
                Item::Track(t) => match t.prim {
                    crate::geom::quant::TrackPrim::Seg { a, b } => (Some(t.layer), Vec2 { x: a.x as f64, y: a.y as f64 }, Vec2 { x: b.x as f64, y: b.y as f64 }, 0.5 * t.width as f64),
                    _ => unreachable!(),
                },
                Item::Via(v) => {
                    let c = Vec2 { x: v.at.x as f64, y: v.at.y as f64 };
                    (None, c, c, 0.5 * v.d as f64)
                }
            }
        }
        fn pt_seg(p: Vec2, a: Vec2, b: Vec2) -> f64 {
            let e = b - a;
            let t = if e.len2() > 0.0 { ((p - a).dot(e) / e.len2()).clamp(0.0, 1.0) } else { 0.0 };
            (p - (a + e * t)).len()
        }
        let (la, a0, a1, ra) = geo(a);
        let (lb, b0, b1, rb) = geo(b);
        if let (Some(x), Some(y)) = (la, lb) {
            if x != y {
                return None;
            }
        }
        let d = pt_seg(a0, b0, b1).min(pt_seg(a1, b0, b1)).min(pt_seg(b0, a0, a1)).min(pt_seg(b1, a0, a1));
        Some(d - ra - rb)
    }

    #[test]
    fn swap_footprint_keeps_p_and_n_apart_on_every_side() {
        // The pattern's pair: w 0.2, gap 0.15, via 0.45 at via gap 0.25, clearance 0.2.
        let rule = DiffPairRule { p: 1, n: 2, width: 200_000, gap: 150_000, via_gap: 250_000, ..Default::default() };
        let cp = Coupled { rule, conns: Vec::new(), partner: BTreeMap::new(), keep: Default::default() };
        let size = ViaSize { d: 450_000, drill: 250_000 };
        let clear = 200_000.0;
        for t in [Vec2 { x: 1.0, y: 0.0 }, Vec2 { x: 0.0, y: 1.0 }, Vec2 { x: -1.0, y: 0.0 }] {
            for side in [t.perp(), -t.perp()] {
                for n_left in [true, false] {
                    let sw = swap_at(&cp, size, clear, Vec2::default(), t, side, 0, 2, n_left);
                    let mut worst = f64::INFINITY;
                    for a in &sw.p_items {
                        for b in &sw.n_items {
                            if let Some(g) = gap(a, b) {
                                worst = worst.min(g);
                            }
                        }
                    }
                    // P and N at least the pair gap apart everywhere (vias at the via gap).
                    assert!(worst >= 150_000.0 - 1_000.0, "t {t:?} side {side:?} n_left {n_left}: P-N gap {worst}");
                    let foot = |items: &[Item]| match &items[0] {
                        Item::Track(tr) => match tr.prim {
                            crate::geom::quant::TrackPrim::Seg { a, .. } => Vec2 { x: a.x as f64, y: -(a.y as f64) },
                            _ => unreachable!(),
                        },
                        _ => unreachable!(),
                    };
                    // N left of the up leg's travel (`side`) exactly when N is left of `t`: the
                    // pair turns as a ribbon, the sides relative to the travel stay.
                    let (fp, fnn) = (foot(&sw.p_items), foot(&sw.n_items));
                    assert_eq!(side.cross(fnn - fp) > 0.0, n_left);
                }
            }
        }
    }

    #[test]
    fn turn_angle_of_a_u_turn_is_pi() {
        let a = Vec2 { x: 1.0, y: 0.0 };
        assert!((turn_angle(a, -a) - std::f64::consts::PI).abs() < 1e-9);
        assert!(turn_angle(a, a).abs() < 1e-9);
    }
}
