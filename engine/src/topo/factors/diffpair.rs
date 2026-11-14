//! Differential pairs (D7, docs/dev/kicad-diff-pairs.md section 6.4): graded prices of a pair's
//! shape, never cut-offs. Each grows smoothly with its amount and steeper once past the limit of a
//! KiCad custom rule (`diff_pair_uncoupled`, `skew`) when the board has one ([`knee`]).
//!
//! * `dp_uncoupled`: per mm of a pair connection that is not coupled at the gap (the fan-out
//!   stubs at pads and vias and every stretch where the tracks part), plus 4 per mm past the rule's
//!   maximum uncoupled length.
//! * `dp_skew`: per mm of length difference P - N (the octilinear bends: 2 x pitch x tan(theta/2)
//!   each, opposite bends cancel), plus 4 per mm past the rule's maximum skew.
//! * `dp_gap_deviation`: per mm of coupled-looking stretch (parallel, within twice the gap) at a gap
//!   off the target by more than the tolerance, weighted by `dev / (dev + tolerance)`.
//! * `dp_via_unpaired`: per via on one half without its partner via on the other.
//! * `dp_polarity_cross`: per pair connection whose P and N swap sides between the ends (they
//!   would have to cross; KiCad never routes that), effectively prohibitive.

use super::{Event, Factor, Report};

/// Smooth knee: about 0 below `max`, about `x - max` above, softened over a tenth of `max`
/// (at least 0.1). `max <= 0`: no rule, 0.
pub fn knee(x: f64, max: f64) -> f64 {
    if max <= 0.0 {
        return 0.0;
    }
    let tau = (0.1 * max).max(0.1);
    let z = (x - max) / tau;
    if z > 30.0 {
        x - max
    } else {
        tau * z.exp().ln_1p()
    }
}

pub struct Uncoupled;
impl Factor for Uncoupled {
    fn name(&self) -> &'static str {
        "dp_uncoupled"
    }
    fn doc(&self) -> &'static str {
        "Differential pair: 1 per mm of a pair connection not coupled at the gap (fan-out at pads and vias, parted stretches), plus 4 per mm past the custom rule's maximum uncoupled length (diff_pair_uncoupled) when there is one."
    }
    fn unit(&self) -> &'static str {
        "per mm uncoupled"
    }
    fn default_weight(&self) -> f64 {
        1.0
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::DpUncoupled { mm, max_mm, .. } => mm + 4.0 * knee(mm, max_mm),
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match ev {
            Event::DpUncoupled { mm, max_mm, .. } if *max_mm > 0.0 => format!("uncoupled {mm:.3} mm (rule maximum {max_mm:.3} mm)"),
            Event::DpUncoupled { mm, .. } => format!("uncoupled {mm:.3} mm"),
            _ => String::new(),
        }
    }
}

pub struct Skew;
impl Factor for Skew {
    fn name(&self) -> &'static str {
        "dp_skew"
    }
    fn doc(&self) -> &'static str {
        "Differential pair: 1 per mm of length difference between the two tracks of a pair connection (each octilinear bend adds 2 x pitch x tan(angle/2) to the outer track; opposite bends cancel), plus 4 per mm past the custom rule's maximum skew when there is one."
    }
    fn unit(&self) -> &'static str {
        "per mm of skew"
    }
    fn default_weight(&self) -> f64 {
        10.0
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::DpSkew { mm, max_mm, .. } => mm.abs() + 4.0 * knee(mm.abs(), max_mm),
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match ev {
            Event::DpSkew { mm, max_mm, .. } if *max_mm > 0.0 => format!("skew {mm:+.3} mm (rule maximum {max_mm:.3} mm)"),
            Event::DpSkew { mm, .. } => format!("skew {mm:+.3} mm"),
            _ => String::new(),
        }
    }
}

pub struct GapDeviation;
impl Factor for GapDeviation {
    fn name(&self) -> &'static str {
        "dp_gap_deviation"
    }
    fn doc(&self) -> &'static str {
        "Differential pair: per mm of parallel stretch whose gap is off the pair gap by more than the tolerance, weighted by deviation / (deviation + tolerance)."
    }
    fn unit(&self) -> &'static str {
        "per mm at a deviating gap"
    }
    fn default_weight(&self) -> f64 {
        5.0
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::DpGapDeviation { mm, dev_mm, tol_mm } => mm * dev_mm / (dev_mm + tol_mm.max(1e-6)),
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match ev {
            Event::DpGapDeviation { mm, dev_mm, .. } => format!("{mm:.3} mm at a gap up to {dev_mm:.3} mm off the pair gap"),
            _ => String::new(),
        }
    }
}

pub struct ViaUnpaired;
impl Factor for ViaUnpaired {
    fn name(&self) -> &'static str {
        "dp_via_unpaired"
    }
    fn doc(&self) -> &'static str {
        "Differential pair: per via on one track of a pair connection without a partner via on the other (a pair routed as two single nets)."
    }
    fn unit(&self) -> &'static str {
        "per unpaired via"
    }
    fn default_weight(&self) -> f64 {
        25.0
    }
    fn default_report(&self) -> Report {
        Report::Warning
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::DpViaUnpaired { count } => f64::from(count),
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match ev {
            Event::DpViaUnpaired { count } => format!("{count} vias without a partner via on the other track"),
            _ => String::new(),
        }
    }
}

pub struct PolarityCross;
impl Factor for PolarityCross {
    fn name(&self) -> &'static str {
        "dp_polarity_cross"
    }
    fn doc(&self) -> &'static str {
        "Differential pair: per pair connection whose two tracks swap sides between its ends (P and N would have to cross; KiCad never routes that, effectively prohibitive)."
    }
    fn unit(&self) -> &'static str {
        "per polarity cross"
    }
    fn default_weight(&self) -> f64 {
        1000.0
    }
    fn default_report(&self) -> Report {
        Report::Warning
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::DpPolarityCross { .. } => 1.0,
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match ev {
            Event::DpPolarityCross { at } => format!("P and N swap sides between the ends: the tracks cross near ({:.3}, {:.3}) mm", at.x / 1e6, -at.y / 1e6),
            _ => String::new(),
        }
    }
}

/// M4: the fan-out skew of a via pair from the turn of the pair's path across it: keep costs
/// `2 pv sin(theta/2)`, flip `2 pv cos(theta/2)` (`pv` the via pitch); priced in the plan's
/// polarity choice ([`crate::topo::dpplan::solve_parity`]), so a flip lands where the path turns
/// back and a keep where it runs on (graded, no cut-off at 90 degrees).
pub struct ViaTurn;
impl Factor for ViaTurn {
    fn name(&self) -> &'static str {
        "dp_via_turn"
    }
    fn doc(&self) -> &'static str {
        "Differential pair: per mm of fan-out skew at a via pair from the turn of the pair's path across it: 2 x via pitch x sin(turn/2) to keep P and N on their sides, 2 x via pitch x cos(turn/2) to swap them (a turn-back); the plan picks the cheapest choice whose number of swaps gives the polarity the pair ends need."
    }
    fn unit(&self) -> &'static str {
        "per mm of via-pair skew"
    }
    fn default_weight(&self) -> f64 {
        10.0
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::DpViaTurn { mm, .. } => mm.abs(),
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match ev {
            Event::DpViaTurn { mm, deg, flip } => format!("via pair at a {deg:.0} degree turn drawn as a {}: fan-out skew {mm:.3} mm", if *flip { "flip (turn-back)" } else { "keep" }),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knee_is_smooth_and_steeper_past_the_rule() {
        assert_eq!(knee(5.0, 0.0), 0.0);
        assert!(knee(0.5, 2.0) < 1e-3);
        assert!((knee(4.0, 2.0) - 2.0).abs() < 1e-3);
        let f = Uncoupled;
        let a = |mm| f.amount(&Event::DpUncoupled { mm, max_mm: 2.0 });
        assert!(a(1.0) < a(2.0) && a(2.0) < a(3.0) && (a(4.0) - a(3.0)) > 4.0);
    }
}
