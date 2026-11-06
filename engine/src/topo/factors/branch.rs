//! Branch reward (D5 net trees): a connection that joins its net's routed copper at a branch
//! point on a trace, or passes through a pad of its net that already has a trace (the pad
//! becomes a junction), saves a second trace and a second pad exit. A reward, not a rule: the
//! multi-source search subtracts it at such an end (a goal-cost adjustment, so A* stays exact),
//! and a separate pad-to-pad path still wins when the branch costs more (crossings, a squeeze,
//! a via at the tee, a detour). 0 = branch points only where they are cheaper on length alone.

use super::{Event, Factor, Report};

pub struct Branch;

impl Factor for Branch {
    fn name(&self) -> &'static str {
        "branch"
    }
    fn doc(&self) -> &'static str {
        "Reward (subtracted) for each connection end that joins the net's routed copper at a branch point on a trace or at a pad that already carries a trace of the net (the tree follows the routing instead of a separate trace to the planned pad)."
    }
    fn unit(&self) -> &'static str {
        "per branch point or through-pad, subtracted"
    }
    fn default_weight(&self) -> f64 {
        1.0
    }
    fn default_report(&self) -> Report {
        Report::Off
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::Branch { .. } => 1.0,
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match *ev {
            Event::Branch { at, on_pad, .. } => format!("branch {} at ({:.3}, {:.3}) mm", if on_pad { "through a pad" } else { "point on a trace" }, at.x / 1e6, -at.y / 1e6),
            _ => String::new(),
        }
    }
}

/// Assembly cost of a trace end on an SMD pad that already carries a trace (a through-pad on a
/// soldered pad: the paste and the joint see copper on two sides). Off by default so it does
/// not cancel the branch reward; a positive weight makes SMD pads less attractive junctions.
pub struct SmdThrough;

impl Factor for SmdThrough {
    fn name(&self) -> &'static str {
        "smd_through"
    }
    fn doc(&self) -> &'static str {
        "Assembly cost of a second trace ending on an SMD pad (the pad becomes a junction of its net's tree)."
    }
    fn unit(&self) -> &'static str {
        "per extra trace end on an SMD pad"
    }
    fn default_weight(&self) -> f64 {
        0.0
    }
    fn default_report(&self) -> Report {
        Report::Off
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::Branch { on_pad: true, smd: true, .. } => 1.0,
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match *ev {
            Event::Branch { at, .. } => format!("second trace on an SMD pad at ({:.3}, {:.3}) mm", at.x / 1e6, -at.y / 1e6),
            _ => String::new(),
        }
    }
}

/// Junction angle (D5, curve D6b): a new trace that leaves a junction close to an arm already
/// there runs side by side with it (two traces of one net through one passage) and leaves an
/// acid trap. A Steiner tree has every angle >= 120 degrees, exactly 120 at a Steiner point (Ras,
/// Swanepoel & Thomas, arXiv 1605.01172 p. 1); with smallest angle `a` the worst relative length
/// excess over the optimum is `sec(60 - a/2) - 1` (Prop. 7.3 p. 12, sharp), and the wedge between
/// the two arms keeps an edge gap below one clearance out to `(w + s) / (2 sin(a/2))` from the
/// junction. The price is `weight x (w + s) x psi(a)` with
/// `psi(a) = (sec(60 - a/2) - 1) x sin 60 / sin(a/2)` below 120 degrees, 0 from 120 up: 0 and
/// smooth at 120, monotone, growing like 1 / a towards 0 (psi 110 deg 0.004, 90 deg 0.043,
/// 60 deg 0.27, 48 deg 0.50, 35 deg 1.03, 30 deg 1.39, 15 deg 4.26). No cutoff: a sharp junction is only
/// expensive, so a separate path or another junction (a pad) wins on price. The same price at a
/// pad that already carries traces of the net (user decision, D6b).
pub struct JunctionAngle;

/// The shape `psi` of [`JunctionAngle`] at `deg` degrees (see there): 0 at or above 120.
pub fn junction_psi(deg: f64) -> f64 {
    if deg >= 120.0 {
        return 0.0;
    }
    let h = (0.5 * deg.max(1e-3)).to_radians();
    let sec = 1.0 / (std::f64::consts::FRAC_PI_3 - h).cos();
    (sec - 1.0) * std::f64::consts::FRAC_PI_3.sin() / h.sin()
}

impl Factor for JunctionAngle {
    fn name(&self) -> &'static str {
        "junction_angle"
    }
    fn doc(&self) -> &'static str {
        "A trace that leaves a junction (a branch point, or a pad that already carries a trace of its net) at a small angle a to an arm already there: weight x (width + clearance) x psi(a), psi(a) = (sec(60° - a/2) - 1) x sin 60° / sin(a/2) below 120° (the Steiner optimum; Ras, Swanepoel & Thomas 2016 Prop. 7.3), 0 from 120° up, growing like 1/a towards 0 (psi: 90° 0.04, 60° 0.27, 48° 0.5, 35° 1.0, 30° 1.4, 15° 4.3)."
    }
    fn unit(&self) -> &'static str {
        "per mm of width + clearance at psi = 1 (about 35°)"
    }
    fn default_weight(&self) -> f64 {
        10.0
    }
    fn default_report(&self) -> Report {
        Report::Off
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::JunctionAngle { deg, ws_mm, .. } => ws_mm * junction_psi(deg),
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match *ev {
            Event::JunctionAngle { at, deg, .. } => format!("trace leaves a junction at {deg:.0}° to another arm at ({:.3}, {:.3}) mm", at.x / 1e6, -at.y / 1e6),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::junction_psi;

    /// psi (D6b item 2d): 0 and flat at 120 degrees, monotone below, the values of REPORT2 §4
    /// (Ras, Swanepoel & Thomas Prop. 7.3 times the wedge factor).
    #[test]
    fn psi_curve() {
        assert_eq!(junction_psi(120.0), 0.0);
        assert_eq!(junction_psi(150.0), 0.0);
        assert!(junction_psi(119.0) < 1e-4);
        for (d, v) in [(110.0, 0.0040), (90.0, 0.0432), (60.0, 0.2679), (48.0, 0.5026), (30.0, 1.386), (15.0, 4.2641)] {
            assert!((junction_psi(d) - v).abs() < 1e-3, "psi({d}) = {}", junction_psi(d));
        }
        let mut last = 0.0;
        for d in (1..120).rev() {
            let p = junction_psi(d as f64);
            assert!(p > last, "monotone at {d}");
            last = p;
        }
    }
}
