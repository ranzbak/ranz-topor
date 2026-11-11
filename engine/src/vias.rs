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
/// is the drill filter. O(|predefined| + |routing_vias|) plus the sort.
pub fn via_sizes(rules: &Rules, net: i32, opts: &Options) -> Vec<ViaSize> {
    if opts.via_size == "board_min" && !rules.diff_pairs.iter().any(|p| p.p == net || p.n == net) {
        return vec![rule_size(rules, carried_width(rules, net), opts.via_drill_grid_nm, &opts.allowed_drills_nm)];
    }
    sizes_for(rules, net, &opts.allowed_drills_nm)
}

/// The diameter (nm) of the default net's via in the plan's via-site test and via area: the net
/// class via (`net_class`, as before 5.4) or the `board_min` rule size. O(pairs).
pub fn plan_via_d(rules: &Rules, opts: &Options) -> f64 {
    if opts.via_size == "board_min" {
        rule_size(rules, rules.default.width, opts.via_drill_grid_nm, &opts.allowed_drills_nm).d as f64
    } else {
        rules.default.via_d as f64
    }
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
    let need = (width.max(0) as f64 / std::f64::consts::PI).ceil() as i64;
    let mut drill = if grid > 0 { (need + grid - 1) / grid * grid } else { need };
    drill = drill.max(rules.min_via_drill);
    if let Some(&a) = allowed.iter().filter(|&&a| a >= drill).min() {
        drill = a;
    }
    ViaSize { d: rules.min_via_diameter.max(drill + 2 * rules.min_annular), drill }
}

/// Via sizes for `net`, largest first, never empty, single place of truth (46 row 2):
///
/// 1. the net's own `(via_d, via_drill)` (netclass or default) -- or, only when the board sets
///    no via up (`predefined_vias` empty and the class at the factory 0.8 / 0.4), `via_fallback`
///    clamped to the board minimums ([`via_fallback_used`]);
/// 2. then the smaller alternatives: every `predefined_vias` entry and every `routing_vias`
///    (fab profile) drill with `d = max(drill + 2 * min_annular, min_via_diameter)`, kept when
///    `drill >= min_via_drill`, `d >= min_via_diameter`, `(d - drill) / 2 >= min_annular`,
///    `d < first.d`, `drill < first.drill` and (when `allowed` is not empty) `drill` in `allowed`.
///    The list is sorted by `(d, drill)` descending and deduplicated.
///
/// Never a drill outside the predefined / profile lists. O(P log P) for P candidate sizes.
fn sizes_for(rules: &Rules, net: i32, allowed: &[i64]) -> Vec<ViaSize> {
    let nr = rules.net_rule(net);
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
    let profile = rules.routing_vias.iter().map(|&dr| ViaSize {
        d: (dr + 2 * rules.min_annular).max(rules.min_via_diameter),
        drill: dr,
    });
    let predefined = rules
        .predefined_vias
        .iter()
        .map(|&[d, drill]| ViaSize { d, drill });
    let mut rest: Vec<ViaSize> = predefined
        .chain(profile)
        .filter(|s| {
            s.drill >= rules.min_via_drill
                && s.d >= rules.min_via_diameter
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
