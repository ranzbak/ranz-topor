//! tw4-r2 (1), T1 part 2 (`[capacity] pad_exits`, docs/dev/design/tw4-r2.md (1)): the pad exit
//! as a plan object.
//!
//! **Why.** Between fine-pitch neighbours (another net's copper beside the pad closer than the
//! wire could pass, the 5.10 test of `board::exit_stub`) the only legal way out of a pad is a
//! short straight stub through one of its sides. Before this, the plan never held that stub: the
//! drawing found it (octi's `Exit`) and broke a clearance when the plan had routed other wires
//! through its room, and a via planned in the exit zone could stand on a neighbour pin (qmtech,
//! the via-at-pin half). TopoR decides pad exits in the topological phase (6.3 manual p. 71,
//! "Don't stretch wire to polygonal pad origin"; p. 56, the minimum width): the drawing only
//! realises them.
//!
//! **Model.** Per wire end at such a pad, the exit is read from the plan, never from the straight
//! line to the target (D1): the *leaving pass* is the first pass whose next triangle is outside
//! the pad (mirrored at end b), `at` its taut crossing point, the *side* the pad side `at` lies
//! beyond against the pad's principal axis (tip +/-, long side +/-: second moments, as
//! `exit_stub`), the stub runs from that side `s + w_e / 2` outwards (octi's reach), and `w_e` is
//! the widest width in `[floor, min(w, pad width)]` whose stub keeps `w_e / 2 + s` from the
//! neighbour copper (static, cached per (pad, side, width)). Three graded terms, each over-full
//! one priced `1 + over / pitch` (Maley 1987 pp. 32-33, 40: a sketch is routable iff no critical
//! cut is over-full; the stub is a feature, its cuts end at the neighbour pins):
//! - (a) static: the stub at `w_e` (at worst the floor) nearer than its clearance to a neighbour;
//! - (b) crossers: CM1's gap cuts at the stub's middle and tip (`viacut`) with the stub as the
//!   feature item (width `w_e`) on the pad's layer: other nets' chords between the stub and a
//!   neighbour pin;
//! - (c) a planned via of the wire in the exit zone nearer than `r + s` to a neighbour pin.
//!
//! The terms are added to the wire's CM1 entry (`refine::Frame::wire_over`), so the undo log, the
//! lane overlays, the local judge's focus and the stored-vs-fresh check cover them as the via
//! terms. Complexity: building one exit is O(obstacles x polygon sizes) once per (pad, side,
//! width); pricing it is CM1's (cuts x crossers log).

use super::pack::Packer;
use super::state::{ObsId, State, TriId, WireId, NO_OBS};
use crate::geom::Vec2;

/// The side of a pad an exit leaves by: its tip along `+axis` / `-axis`, or its long side on
/// `+perp` / `-perp` (the axis: the pad's principal axis with its larger component positive).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    TipPlus,
    TipMinus,
    LongPlus,
    LongMinus,
}

/// The static part of an exit for one (pad, side, width): its stub width, the static deficit,
/// the neighbour copper. Cached in the packer.
#[derive(Clone, Debug)]
pub struct ExitStatic {
    /// The stub's width (nm): the widest that keeps its clearance, at worst the floor.
    pub w_e: f64,
    /// (a): how far the stub at `w_e` comes nearer than its clearance to a neighbour (nm; <= 0: fits).
    pub over: f64,
    /// Other nets' copper on the pad's layer that is fine pitch to it (the neighbour pins).
    pub neigh: Vec<ObsId>,
}

/// One planned pad exit (see the module doc).
#[derive(Clone, Debug)]
pub struct ExitGeo {
    pub wire: WireId,
    /// 0: the wire's start, 1: its end.
    pub end: u8,
    pub pad: ObsId,
    /// The pad's layer (projection layer index).
    pub layer: usize,
    pub side: Side,
    /// The leaving pass (index in the wire) and its taut crossing point.
    pub pass: usize,
    pub at: Vec2,
    /// Outward unit direction of the stub.
    pub dir: Vec2,
    pub w_e: f64,
    /// The stub's centre line, from the pad side to `s + w_e / 2` beyond it.
    pub a: Vec2,
    pub b: Vec2,
    /// (a) static over-fill (nm, > 0 = over).
    pub stat: f64,
    pub neigh: Vec<ObsId>,
    /// The triangle whose vertex star holds the gap cuts' neighbourhood (the leaving pass's
    /// outside triangle).
    pub tri: TriId,
}

/// The pad axes of obstacle `o`: centre, unit axis (larger component positive), half lengths
/// along the axis and across. O(|poly|).
fn axes(poly: &[Vec2], c: Vec2) -> (Vec2, f64, f64, f64, f64) {
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for &p in poly {
        let d = p - c;
        sxx += d.x * d.x;
        syy += d.y * d.y;
        sxy += d.x * d.y;
    }
    let th = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let u0 = Vec2 { x: th.cos(), y: th.sin() };
    let u0 = if u0.x.abs() >= u0.y.abs() {
        if u0.x < 0.0 {
            -u0
        } else {
            u0
        }
    } else if u0.y < 0.0 {
        -u0
    } else {
        u0
    };
    let ext = |v: Vec2| poly.iter().map(|&p| (p - c).dot(v)).fold(f64::MIN, f64::max);
    (u0, ext(u0), ext(-u0), ext(u0.perp()), ext(-u0.perp()))
}

/// The triangles of wire `w` at its pad ends that an exit can read: per end with a pad, the
/// traversals inside the pad and the first one outside (the leaving pass's outside triangle).
/// O(traversals inside the pads).
pub fn end_tris(st: &State, w: WireId) -> Vec<TriId> {
    let wi = &st.wires[w];
    let pr = st.proj;
    let n = wi.passes.len();
    let mut out = Vec::new();
    if !wi.alive || n == 0 || wi.tris.len() != n + 1 {
        return out;
    }
    let inside = |t: TriId, ob: ObsId| pr.tri_obs[t].contains(&ob);
    // The same scan as [`leaving`]: every triangle up to its outside one.
    if wi.pa != NO_OBS {
        out.push(wi.tris[0]);
        for k in 0..n {
            out.push(wi.tris[k + 1]);
            if !inside(wi.tris[k + 1], wi.pa) {
                break;
            }
        }
    }
    if wi.pb != NO_OBS {
        out.push(wi.tris[n]);
        for k in (0..n).rev() {
            out.push(wi.tris[k]);
            if !inside(wi.tris[k], wi.pb) {
                break;
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The leaving pass of end `end` of `w` from pad `ob`: the first pass whose next triangle is
/// outside the pad (end 1: the last pass whose previous triangle is outside), with that outside
/// triangle. `None` for a wire without passes. O(traversals inside the pad).
fn leaving(st: &State, w: WireId, end: u8, ob: ObsId) -> Option<(usize, TriId)> {
    let wi = &st.wires[w];
    let n = wi.passes.len();
    if n == 0 || wi.tris.len() != n + 1 {
        return None;
    }
    let inside = |t: TriId| st.proj.tri_obs[t].contains(&ob);
    if end == 0 {
        let k = (0..n).find(|&k| !inside(wi.tris[k + 1])).unwrap_or(n - 1);
        Some((k, wi.tris[k + 1]))
    } else {
        let k = (0..n).rev().find(|&k| !inside(wi.tris[k])).unwrap_or(0);
        Some((k, wi.tris[k]))
    }
}

impl Packer<'_> {
    /// The static part of the exit of pad `ob` by `side` for a wire of `net`, width `w`,
    /// clearance `s` and neck floor `floor` ([`ExitStatic`]); `None` when the pad needs no
    /// planned exit there (no fine-pitch neighbour on that side). Cached. O(obstacles x polygon
    /// sizes x width steps) per new key.
    pub fn exit_static(&self, ob: ObsId, side: Side, net: i32, w: f64, s: f64, floor: f64) -> Option<std::sync::Arc<ExitStatic>> {
        let key = (ob, side, net, w.to_bits(), s.to_bits(), floor.to_bits());
        if let Some(x) = self.exit_cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key) {
            return x.clone();
        }
        let x = self.exit_static_raw(ob, side, net, w, s, floor).map(std::sync::Arc::new);
        self.exit_cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(key, x.clone());
        x
    }

    fn exit_static_raw(&self, ob: ObsId, side: Side, net: i32, w: f64, s: f64, floor: f64) -> Option<ExitStatic> {
        let pr = self.proj();
        let o = &pr.obstacles[ob];
        let c = o.center;
        let (u0, ep, em, eq, er) = axes(&o.poly, c);
        let v = u0.perp();
        let (lu, lv) = (ep + em, eq + er);
        let (dir, from) = stub_frame(side, c, u0, v, ep, em, eq, er, None);
        let ws = w.min(lv).max(floor.min(w));
        let need = |x: &super::state::Obstacle| x.clear.max(s);
        // Fine pitch on this side: another net's copper on the layer beside the pad closer than a
        // wire of width `ws` could pass (`exit_stub`'s test); at a long side only copper on that side.
        let on = |x: &super::state::Obstacle| x.drilled || x.layers.0 & o.layers.0 != 0;
        let long = matches!(side, Side::LongPlus | Side::LongMinus);
        let mut neigh: Vec<ObsId> = Vec::new();
        for (j, x) in pr.obstacles.iter().enumerate() {
            if j == ob || x.net == Some(net) || !on(x) || x.bare {
                continue;
            }
            if long && (x.center - c).dot(dir) <= 0.5 * lv {
                continue;
            }
            let gap = ws + 2.0 * need(x);
            if (x.center - c).len() > lu + gap + 4.0 * lv.max(gap) {
                continue;
            }
            if super::board::poly_poly_dist(&o.poly, &x.poly) - x.sag < gap {
                neigh.push(j);
            }
        }
        if neigh.is_empty() {
            return None;
        }
        // The widest stub width that keeps its clearance, from min(w, pad width) down to the floor.
        let def = |x: f64| {
            let (a, b) = (from, from + dir * (s + 0.5 * x));
            neigh.iter().map(|&j| need(&pr.obstacles[j]) + 0.5 * x + pr.obstacles[j].sag - super::board::seg_poly_dist(a, b, &pr.obstacles[j].poly)).fold(f64::NEG_INFINITY, f64::max)
        };
        let lo = floor.min(ws);
        let steps = 16;
        let mut best = None;
        for i in 0..=steps {
            let x = ws - (ws - lo) * i as f64 / steps as f64;
            if def(x) <= 0.0 {
                best = Some(x);
                break;
            }
        }
        let w_e = best.unwrap_or(lo);
        Some(ExitStatic { w_e, over: def(w_e).max(0.0), neigh })
    }

    /// The planned exits of wire `w` (0..2, see the module doc), from its taut path. O(traversals
    /// inside its pads), cached statics.
    pub fn exits_of(&self, st: &State, w: WireId, floor: f64) -> Vec<ExitGeo> {
        let wi = &st.wires[w];
        let pr = st.proj;
        if !wi.alive || wi.passes.is_empty() {
            return Vec::new();
        }
        let taut = self.taut(st, w);
        let mut out = Vec::new();
        for (end, ob) in [(0u8, wi.pa), (1u8, wi.pb)] {
            if ob == NO_OBS {
                continue;
            }
            let o = &pr.obstacles[ob];
            if o.net != Some(wi.net) || o.drilled || o.layers.0.count_ones() != 1 || o.poly.len() < 3 {
                continue;
            }
            let c = o.center;
            let (u0, ep, em, eq, er) = axes(&o.poly, c);
            let (lu, lv) = (ep + em, eq + er);
            if lu < 1.2 * lv {
                continue;
            }
            let Some((k, tri)) = leaving(st, w, end, ob) else { continue };
            let at = taut.at[k];
            let v = u0.perp();
            let (du, dv) = ((at - c).dot(u0), (at - c).dot(v));
            // Where the line from the centre to `at` leaves the pad: through a tip zone (the last
            // lv / 2 of its length) or a long side.
            let tx = (0.5 * lu / du.abs().max(1e-9)).min(0.5 * lv / dv.abs().max(1e-9));
            let tip_zone = (du * tx).abs() >= 0.5 * (lu - lv) - 1.0;
            let side = match (tip_zone, du >= 0.0, dv >= 0.0) {
                (true, true, _) => Side::TipPlus,
                (true, false, _) => Side::TipMinus,
                (false, _, true) => Side::LongPlus,
                (false, _, false) => Side::LongMinus,
            };
            let Some(x) = self.exit_static(ob, side, wi.net, wi.w, wi.s, floor) else { continue };
            let (dir, from) = stub_frame(side, c, u0, v, ep, em, eq, er, Some(at));
            let layer = o.layers.0.trailing_zeros() as usize;
            out.push(ExitGeo { wire: w, end, pad: ob, layer, side, pass: k, at, dir, w_e: x.w_e, a: from, b: from + dir * (wi.s + 0.5 * x.w_e), stat: x.over, neigh: x.neigh.clone(), tri });
        }
        out
    }
}

/// The stub's outward direction and start point (on the pad's side) for `side`: a tip exit
/// starts at the tip's centre, a long-side exit at the point of that side across from `at`
/// (`None`: the side's middle), kept off the tip zones. O(1).
#[allow(clippy::too_many_arguments)]
fn stub_frame(side: Side, c: Vec2, u0: Vec2, v: Vec2, ep: f64, em: f64, eq: f64, er: f64, at: Option<Vec2>) -> (Vec2, Vec2) {
    match side {
        Side::TipPlus => (u0, c + u0 * ep),
        Side::TipMinus => (-u0, c - u0 * em),
        Side::LongPlus | Side::LongMinus => {
            let (dir, e) = if side == Side::LongPlus { (v, eq) } else { (-v, er) };
            let half = 0.5 * (ep + em) - 0.5 * (eq + er);
            let mid = 0.5 * (ep - em);
            let t = at.map_or(mid, |p| (p - c).dot(u0).clamp(mid - half.max(0.0), mid + half.max(0.0)));
            (dir, c + u0 * t + dir * e)
        }
    }
}

/// (c): how far a via of radius `r` and clearance `s` at `p` comes nearer than its clearance to
/// the neighbour copper `neigh` (nm, <= 0: clear). O(neighbours x polygon sizes).
pub fn via_deficit(st: &State, neigh: &[ObsId], p: Vec2, r: f64, s: f64) -> f64 {
    let pr = st.proj;
    neigh
        .iter()
        .map(|&j| {
            let x = &pr.obstacles[j];
            r + x.clear.max(s) + x.sag - super::draw::poly_dist(p, &x.poly)
        })
        .fold(f64::NEG_INFINITY, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_side_stub_starts_across_from_the_pass_point_and_a_tip_stub_at_the_tip() {
        let c = Vec2 { x: 0.0, y: 0.0 };
        let (u0, v) = (Vec2 { x: 1.0, y: 0.0 }, Vec2 { x: 0.0, y: 1.0 });
        // A 2 x 0.6 pad: tips at x = +/-1, long sides at y = +/-0.3.
        let (dir, a) = stub_frame(Side::TipPlus, c, u0, v, 1.0, 1.0, 0.3, 0.3, None);
        assert_eq!((dir, a), (u0, Vec2 { x: 1.0, y: 0.0 }));
        let (dir, a) = stub_frame(Side::LongMinus, c, u0, v, 1.0, 1.0, 0.3, 0.3, Some(Vec2 { x: 0.2, y: -2.0 }));
        assert_eq!(dir, -v);
        assert!((a.x - 0.2).abs() < 1e-12 && (a.y + 0.3).abs() < 1e-12, "{a:?}");
        // Kept off the tip zones (the last 0.3 at each end).
        let (_, a) = stub_frame(Side::LongPlus, c, u0, v, 1.0, 1.0, 0.3, 0.3, Some(Vec2 { x: 5.0, y: 2.0 }));
        assert!((a.x - 0.7).abs() < 1e-12, "{a:?}");
    }

    #[test]
    fn the_axes_are_the_long_axis_with_a_positive_larger_component() {
        let poly = [Vec2 { x: -1.0, y: -0.3 }, Vec2 { x: 1.0, y: -0.3 }, Vec2 { x: 1.0, y: 0.3 }, Vec2 { x: -1.0, y: 0.3 }];
        let (u0, ep, em, eq, er) = axes(&poly, Vec2::default());
        assert!((u0.x - 1.0).abs() < 1e-9 && u0.y.abs() < 1e-9);
        assert!((ep - 1.0).abs() < 1e-9 && (em - 1.0).abs() < 1e-9 && (eq - 0.3).abs() < 1e-9 && (er - 0.3).abs() < 1e-9);
    }
}
