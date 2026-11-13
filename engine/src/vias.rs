//! Via sizes (through vias; design note `m3-layers-vias-design.md` §1).

use crate::model::{Options, Rules};
use serde::Serialize;

/// A through-via size: copper diameter and drill, nm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ViaSize {
    pub d: i64,
    pub drill: i64,
}

/// KiCad's factory net class via (diameter, drill), nm: "the board sets no via up".
pub const FACTORY_VIA: [i64; 2] = [800_000, 400_000];

/// The fallback applies to `net`: the board lists no pre-defined via and the net's class via is
/// KiCad's factory 0.8 / 0.4 mm, and the configured fallback is a different size. O(1).
pub fn via_fallback_used(rules: &Rules, net: i32) -> bool {
    let nr = rules.net_rule(net);
    rules.predefined_vias.is_empty()
        && [nr.via_d, nr.via_drill] == FACTORY_VIA
        && rules.via_fallback != FACTORY_VIA
}

/// Via sizes for `net`, largest first, never empty (see `sizes_for`); `opts.allowed_drills_nm`
/// is the drill filter. O(|predefined|) plus the sort.
pub fn via_sizes(rules: &Rules, net: i32, opts: &Options) -> Vec<ViaSize> {
    // A diff-pair net: the pair's via (the smallest the board allows by default, resolved by the
    // plugin; `topo::diffpair::via_of`), also where the net is routed alone.
    // Never below the custom-rule minimum of either net of the pair (KiCad DRC checks each via
    // against its own net's rules).
    if let Some(p) = rules.diff_pairs.iter().find(|p| (p.p == net || p.n == net) && p.via_d > 0 && p.via_drill > 0) {
        return vec![pair_floor(rules, p, ViaSize { d: p.via_d, drill: p.via_drill })];
    }
    if opts.via_size == "board_min" && !rules.diff_pairs.iter().any(|p| p.p == net || p.n == net) {
        return vec![net_rule_size(rules, net, carried_width(rules, net), opts.via_drill_grid_nm, &opts.allowed_drills_nm)];
    }
    sizes_for(rules, net, &opts.allowed_drills_nm)
}

/// A net id no net has: [`Rules::net_rule`] gives the board's default rule for it (net 0 is a
/// real net of the model with its own class and clearance, not the board default).
pub const DEFAULT_RULE_NET: i32 = i32::MIN;

/// The reach diameter (nm, [`via_reach`] x 2) of the default net's via in the plan's via-site
/// test and via area: the net class via (`net_class`, as before 5.4) or the `board_min` rule
/// size. O(pairs).
pub fn plan_via_d(rules: &Rules, opts: &Options) -> f64 {
    let sz = if opts.via_size == "board_min" {
        rule_size(rules, rules.default.width, opts.via_drill_grid_nm, &opts.allowed_drills_nm)
    } else {
        ViaSize { d: rules.default.via_d, drill: rules.default.via_drill }
    };
    2.0 * via_reach(rules, DEFAULT_RULE_NET, sz)
}

/// Centre distance (nm) the plan's board-level via ([`plan_via_d`]) needs to foreign copper:
/// [`via_need_copper`] of the default net, `[search] margin_um` included (the projection's via
/// site radius, the via area of the cut price). O(1).
pub fn plan_via_need(rules: &Rules, opts: &Options) -> f64 {
    0.5 * plan_via_d(rules, opts) + net_clear(rules, DEFAULT_RULE_NET) + opts.margin_nm.max(0) as f64
}

// ---- The room a via needs (shopping list 5.4, task 1): one rule for the plan and the drawing ----
//
// KiCad checks a through via against foreign copper twice (Board Setup -> Constraints): its
// copper disc keeps the clearance (`d / 2 + clearance`), and its drill keeps the hole clearance
// (`drill / 2 + hole clearance`, the router's via drill: [`Rules::via_hole_clear`]); between two
// holes (any nets) the hole-to-hole distance holds (`(drill_a + drill_b) / 2 + hole_to_hole`).
// The drawing's checker tests all three ([`crate::topo::draw::via_clear`] is its disc form); the
// plan reasons with the same numbers through these functions. With small vias (`[vias] size =
// "board_min"`, e.g. 0.4 / 0.3 mm with 0.2 mm clearance and 0.25 mm hole clearance) the hole term
// reaches as far as or further than the copper term, which the plan's disc-only room missed.
// `margin` is `[search] margin_um` (nm), added to every requirement as the drawing does.

/// The clearance (nm) of `net` the via rules start from: the net's rule, floored by the board
/// minimum. O(1).
fn net_clear(rules: &Rules, net: i32) -> f64 {
    rules.net_rule(net).clearance.max(rules.min_clearance) as f64
}

/// Centre distance (nm) a via `sz` of `net` needs to the edge of foreign copper whose own
/// clearance is `other_clear` (0: none beyond the via net's): `max(d / 2 + clearance, drill / 2 +
/// hole clearance) + margin`, the clearance the larger of the two nets'. O(1).
pub fn via_need_copper(rules: &Rules, net: i32, sz: ViaSize, other_clear: f64, margin: f64) -> f64 {
    let copper = 0.5 * sz.d as f64 + net_clear(rules, net).max(other_clear);
    let hole = if sz.drill > 0 { 0.5 * sz.drill as f64 + rules.via_hole_clear() as f64 } else { 0.0 };
    copper.max(hole) + margin
}

/// The reach (nm) of a via `sz` of `net`: the radius of the disc that, with the net's clearance
/// round it, needs exactly [`via_need_copper`] (`max(d / 2, drill / 2 + hole clearance -
/// clearance)`). The plan packs and prices a via as this disc, so every place that adds "radius +
/// clearance" keeps the hole rule too. Never below `d / 2`; equal to it when the copper term is the
/// larger (every net class via of the ladder boards). O(1).
pub fn via_reach(rules: &Rules, net: i32, sz: ViaSize) -> f64 {
    via_need_copper(rules, net, sz, 0.0, 0.0) - net_clear(rules, net)
}

/// Centre distance (nm) two vias need: `a` of net `na` and `b` of net `nb`. Foreign nets: each
/// disc keeps the clearance to the other's disc and each drill the hole clearance to the other's
/// disc; any nets: the two drills keep `hole_to_hole`; the same net: the discs may touch. Plus
/// `margin`. O(1).
pub fn via_via_need(rules: &Rules, na: i32, a: ViaSize, nb: i32, b: ViaSize, margin: f64) -> f64 {
    let (ra, rb) = (0.5 * a.d as f64, 0.5 * b.d as f64);
    let (ha, hb) = (0.5 * a.drill.max(0) as f64, 0.5 * b.drill.max(0) as f64);
    let mut need = if na != nb || na == 0 {
        let c = net_clear(rules, na).max(net_clear(rules, nb));
        let hc = rules.via_hole_clear() as f64;
        (ra + rb + c).max(if a.drill > 0 { ha + hc + rb } else { 0.0 }).max(if b.drill > 0 { hb + hc + ra } else { 0.0 })
    } else {
        0.0
    };
    if a.drill > 0 && b.drill > 0 {
        need = need.max(ha + hb + rules.hole_to_hole as f64);
    }
    need + margin
}

/// The width (nm) of the trace a via of `net` carries: the pair's `diff_pair_width` for a net of
/// a differential pair (when set), else the net's track width. O(pairs).
pub fn carried_width(rules: &Rules, net: i32) -> i64 {
    rules
        .diff_pairs
        .iter()
        .find(|p| (p.p == net || p.n == net) && p.width > 0)
        .map_or(rules.net_rule(net).width, |p| p.width)
}

/// The via of `[vias] size = "board_min"` (shopping list 5.4; user rule 2026-11-10, memory
/// power-via-rules rule 0): the smallest via the Board Setup minimums allow (the supplier's
/// capability) that carries a trace of `width` nm. A plated hole carries the trace's current when
/// its circumference is at least the trace width, `pi x drill >= width`, so
///
///   drill = max(min_via_drill, ceil(width / pi / grid) x grid)
///   d     = max(min_via_diameter, drill + 2 x min_annular)
///
/// with `grid` the drill grid (nm; 0 = no rounding). With an `allowed` drill list (the fab
/// filter) the smallest allowed drill at least that large is taken (none: the computed drill).
/// Not the net class via (patterns `scratch/via-patterns/via_size_rule`: 0.2 mm -> 0.45 / 0.25,
/// 1.0 mm -> 0.52 / 0.32, 2.0 mm -> 0.84 / 0.64 with minimums 0.45 / 0.25 / 0.10). O(|allowed|).
pub fn rule_size(rules: &Rules, width: i64, grid: i64, allowed: &[i64]) -> ViaSize {
    size_with_min(rules, (rules.min_via_diameter, rules.min_via_drill), width, grid, allowed)
}

/// [`rule_size`] with the minimums of `net` ([`Rules::via_min_of`]: a matching custom rule's
/// `via_diameter` / `hole_size` minimum replaces the Board Setup one, as in KiCad's DRC). O(|allowed|).
pub fn net_rule_size(rules: &Rules, net: i32, width: i64, grid: i64, allowed: &[i64]) -> ViaSize {
    size_with_min(rules, rules.via_min_of(net), width, grid, allowed)
}

fn size_with_min(rules: &Rules, (min_d, min_drill): (i64, i64), width: i64, grid: i64, allowed: &[i64]) -> ViaSize {
    let need = (width.max(0) as f64 / std::f64::consts::PI).ceil() as i64;
    let mut drill = if grid > 0 { (need + grid - 1) / grid * grid } else { need };
    drill = drill.max(min_drill);
    if let Some(&a) = allowed.iter().filter(|&&a| a >= drill).min() {
        drill = a;
    }
    ViaSize { d: min_d.max(drill + 2 * rules.min_annular), drill }
}

/// A pair via `s` raised to the custom-rule minimums of both nets of pair `p` (diameter and drill,
/// the ring kept at the annular minimum). The plugin already resolves the pair via above them
/// (`toporlib.diffpair.smallest_via`); this keeps an older or hand-written model legal. O(1).
pub fn pair_floor(rules: &Rules, p: &crate::model::DiffPairRule, s: ViaSize) -> ViaSize {
    let (dp, rp) = rules.via_min_of(p.p);
    let (dn, rn) = rules.via_min_of(p.n);
    let drill = s.drill.max(rp).max(rn);
    ViaSize { d: s.d.max(dp).max(dn).max(drill + 2 * rules.min_annular), drill }
}

/// Via sizes for `net`, largest first, never empty, single place of truth (46 row 2):
///
/// 1. the net's own `(via_d, via_drill)` (netclass or default) -- or, only when the board sets
///    no via up (`predefined_vias` empty and the class at the factory 0.8 / 0.4), `via_fallback`
///    clamped to the board minimums ([`via_fallback_used`]);
/// 2. then the smaller alternatives: every `predefined_vias` entry (KiCad's pre-defined vias,
///    Board Setup -> Pre-defined Sizes; no fab-profile list, user decision 2026-11-12), kept when
///    `drill >= min_via_drill`, `d >= min_via_diameter`, `(d - drill) / 2 >= min_annular`,
///    `d < first.d`, `drill < first.drill` and (when `allowed` is not empty) `drill` in `allowed`.
///    The list is sorted by `(d, drill)` descending and deduplicated.
///
/// Never a size the board does not define. O(P log P) for P pre-defined vias.
fn sizes_for(rules: &Rules, net: i32, allowed: &[i64]) -> Vec<ViaSize> {
    let nr = rules.net_rule(net);
    // a pre-defined via below the net's custom-rule minimum is no alternative (KiCad's DRC would flag it)
    let (lo_d, lo_drill) = rules.via_min_of(net);
    let first = if via_fallback_used(rules, net) {
        ViaSize {
            d: rules.via_fallback[0].max(rules.min_via_diameter),
            drill: rules.via_fallback[1].max(rules.min_via_drill),
        }
    } else {
        ViaSize {
            d: nr.via_d,
            drill: nr.via_drill,
        }
    };
    let mut rest: Vec<ViaSize> = rules
        .predefined_vias
        .iter()
        .map(|&[d, drill]| ViaSize { d, drill })
        .filter(|s| {
            s.drill >= lo_drill
                && s.d >= lo_d
                && 2 * rules.min_annular <= s.d - s.drill
                && s.d < first.d
                && s.drill < first.drill
                && (allowed.is_empty() || allowed.contains(&s.drill))
        })
        .collect();
    rest.sort_by_key(|b| std::cmp::Reverse((b.d, b.drill)));
    rest.dedup();
    let mut out = vec![first];
    out.extend(rest);
    out
}


#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules { min_via_diameter: 450_000, min_via_drill: 250_000, min_annular: 100_000, ..Rules::default() }
    }

    #[test]
    fn the_board_min_via_carries_the_trace_width_on_the_drill_grid() {
        // scratch/via-patterns/via_size_rule: SIG 0.2, PWR 1.0, HEAVY 2.0 mm.
        let r = rules();
        assert_eq!(rule_size(&r, 200_000, 10_000, &[]), ViaSize { d: 450_000, drill: 250_000 });
        assert_eq!(rule_size(&r, 250_000, 10_000, &[]), ViaSize { d: 450_000, drill: 250_000 });
        assert_eq!(rule_size(&r, 1_000_000, 10_000, &[]), ViaSize { d: 520_000, drill: 320_000 });
        assert_eq!(rule_size(&r, 2_000_000, 10_000, &[]), ViaSize { d: 840_000, drill: 640_000 });
        // A 0.05 mm grid rounds 0.3183 up to 0.35; no grid keeps the exact drill.
        assert_eq!(rule_size(&r, 1_000_000, 50_000, &[]).drill, 350_000);
        assert_eq!(rule_size(&r, 1_000_000, 0, &[]).drill, 318_310);
        // The fab filter takes the smallest allowed drill at least that large.
        assert_eq!(rule_size(&r, 1_000_000, 10_000, &[300_000, 400_000, 500_000]).drill, 400_000);
        // pi x drill >= width always holds.
        for w in (100_000..3_000_000).step_by(37_000) {
            let s = rule_size(&r, w, 10_000, &[]);
            assert!(std::f64::consts::PI * s.drill as f64 >= w as f64 - 1e-6, "{w} {s:?}");
            assert!(s.d - s.drill >= 2 * r.min_annular && s.d >= r.min_via_diameter);
        }
    }

    #[test]
    fn the_via_need_is_the_larger_of_the_copper_and_the_hole_rule() {
        // wiccon's rules: clearance 0.2, hole clearance 0.25, hole to hole 0.25.
        let mut r = Rules { hole_clearance: 250_000, hole_to_hole: 250_000, ..Rules::default() };
        r.default.clearance = 200_000;
        let class = ViaSize { d: 800_000, drill: 400_000 };
        let small = ViaSize { d: 400_000, drill: 300_000 };
        // The class via: the copper term binds (0.4 + 0.2 > 0.2 + 0.25); reach = radius.
        assert_eq!(via_need_copper(&r, 1, class, 0.0, 0.0), 600_000.0);
        assert_eq!(via_reach(&r, 1, class), 400_000.0);
        // 0.4 / 0.3: copper 0.2 + 0.2 = 0.4 = hole 0.15 + 0.25; with a 0.27 mm via-hole clearance
        // (a fab floor) the hole binds: 0.42, reach 0.22 > the 0.2 radius.
        assert_eq!(via_need_copper(&r, 1, small, 0.0, 0.0), 400_000.0);
        r.via_hole_clearance = 270_000;
        assert_eq!(via_need_copper(&r, 1, small, 0.0, 0.0), 420_000.0);
        assert_eq!(via_reach(&r, 1, small), 220_000.0);
        // A neighbour with a larger clearance raises the copper term only; the margin adds.
        assert_eq!(via_need_copper(&r, 1, small, 300_000.0, 10_000.0), 510_000.0);
        // Via to via, foreign nets: discs 0.2 + 0.2 + 0.2 = 0.6, drill to disc 0.15 + 0.27 + 0.2
        // = 0.62, drill to drill 0.15 + 0.15 + 0.25 = 0.55 -> 0.62.
        assert_eq!(via_via_need(&r, 1, small, 2, small, 0.0), 620_000.0);
        // The same net: only hole to hole (the discs may touch).
        assert_eq!(via_via_need(&r, 1, small, 1, small, 0.0), 550_000.0);
        // Class vias: copper 1.0 binds, as the old disc sum.
        assert_eq!(via_via_need(&r, 1, class, 2, class, 0.0), 1_000_000.0);
        // The reach form never asks less than the rule: reach + clearance >= the need.
        for (a, b) in [(small, small), (small, class), (class, small)] {
            let c = r.default.clearance as f64;
            assert!(via_reach(&r, 1, a) + c >= via_need_copper(&r, 1, a, 0.0, 0.0) - 1e-9);
            assert!(via_reach(&r, 1, a) + via_reach(&r, 2, b) + c >= via_via_need(&r, 1, a, 2, b, 0.0) - 1e-9);
        }
    }

    #[test]
    fn board_min_replaces_the_net_class_ladder_and_net_class_stays_the_default() {
        let mut r = rules();
        r.default.via_d = 800_000;
        r.default.via_drill = 400_000;
        r.default.width = 250_000;
        let old = via_sizes(&r, 1, &Options::default());
        assert_eq!(old[0], ViaSize { d: 800_000, drill: 400_000 });
        let o = Options { via_size: "board_min".into(), ..Options::default() };
        assert_eq!(via_sizes(&r, 1, &o), vec![ViaSize { d: 450_000, drill: 250_000 }]);
        // A diff pair net carries the pair width (its vias come from `diffpair::via_of`; the
        // planning model's envelope via of the P net stays in its rule).
        r.diff_pairs.push(crate::model::DiffPairRule { p: 1, n: 2, width: 1_000_000, ..Default::default() });
        assert_eq!(carried_width(&r, 2), 1_000_000);
        assert_eq!(via_sizes(&r, 2, &o)[0], ViaSize { d: 800_000, drill: 400_000 });
    }
}
