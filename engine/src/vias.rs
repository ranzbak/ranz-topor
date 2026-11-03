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

/// Via sizes for `net`, largest first, never empty (see [`via_sizes_baked`]); `opts.allowed_drills_nm`
/// is the drill filter. O(|predefined| + |routing_vias|) plus the sort.
pub fn via_sizes(rules: &Rules, net: i32, opts: &Options) -> Vec<ViaSize> {
    sizes_for(rules, net, &opts.allowed_drills_nm)
}

/// [`via_sizes`] with the drill filter baked into `rules.allowed_drills` by `route` (the search
/// has the rules but not the options).
pub fn via_sizes_baked(rules: &Rules, net: i32) -> Vec<ViaSize> {
    sizes_for(rules, net, &rules.allowed_drills)
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

