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

/// Junction angle (D5): a new trace that leaves a junction close to an arm already there runs
/// side by side with it (two traces of one net through one passage) and leaves an acid trap.
/// The Steiner optimum has 120 degrees between the arms at a branch point (Popov & Popov, EDN
/// 2013); the price is 0 from 120 degrees up and rises smoothly and ever steeper below:
/// `(1 - angle / 120)^exp` per junction, 1 at 0 degrees. No cutoff: a sharp junction is only
/// expensive, so a separate path or another junction (a pad) wins on price.
pub struct JunctionAngle;

/// The curve of [`JunctionAngle`]: 0 at or above 120 degrees, 1 at 0.
pub fn junction_angle_amount(deg: f64, exp: f64) -> f64 {
    if deg >= 120.0 {
        0.0
    } else {
        (1.0 - deg.max(0.0) / 120.0).powf(exp.max(0.0))
    }
}

impl Factor for JunctionAngle {
    fn name(&self) -> &'static str {
        "junction_angle"
    }
    fn doc(&self) -> &'static str {
        "A trace that leaves a junction (a branch point, or a pad that already carries a trace of its net) at a small angle to an arm already there: (1 - angle/120°)^exp, 0 from 120° (the Steiner optimum) up, the full weight at 0°."
    }
    fn unit(&self) -> &'static str {
        "per junction at 0°, (1 - angle/120°)^exp below 120°"
    }
    fn default_weight(&self) -> f64 {
        10.0
    }
    fn default_report(&self) -> Report {
        Report::Off
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::JunctionAngle { deg, exp, .. } => junction_angle_amount(deg, exp),
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
