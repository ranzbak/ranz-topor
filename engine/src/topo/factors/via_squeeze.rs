//! Via squeeze: a via left with less room than one via diameter in a passage next to it (D3).
//! A through via is a round pad on every layer (Popov & Popov 2012, p. 118); in a narrow
//! passage it takes the room the wires need there and leaves the drawing no slack. A social
//! factor, not a rule: P3/P4 put a via where the passages around it keep the most slack, and the
//! price grows linearly as the slack left on a passage-layer next to the via falls from one via
//! diameter (free) to none. A drawn via with less slack than half its diameter is reported.

use super::{squeeze_amount, Event, Factor, Report};

pub struct ViaSqueeze;

impl Factor for ViaSqueeze {
    fn name(&self) -> &'static str {
        "via_squeeze"
    }
    fn doc(&self) -> &'static str {
        "A via in a narrow pass: a passage next to it (on any layer: a through via blocks all of them) is left with less slack than one via diameter; linear from one via diameter of slack (free) to none. A drawn via with less than half a diameter of slack is reported."
    }
    fn unit(&self) -> &'static str {
        "per fully squeezed passage-layer next to a via"
    }
    fn default_weight(&self) -> f64 {
        5.0
    }
    fn default_report(&self) -> Report {
        Report::Warning
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::ViaSqueeze { slack_nm, via_nm, .. } => squeeze_amount(slack_nm, via_nm),
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match *ev {
            Event::ViaSqueeze { at, slack_nm, via_nm, .. } => format!(
                "via in a narrow pass at ({:.3}, {:.3}) mm: {:.3} mm of slack next to it (via {:.3} mm)",
                at.x / 1e6,
                -at.y / 1e6,
                slack_nm.max(0.0) / 1e6,
                via_nm / 1e6
            ),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Event, Registry, WireCtx};
    use crate::geom::Vec2;

    #[test]
    fn the_price_falls_linearly_with_the_slack_up_to_one_via_diameter() {
        let r = Registry::standard();
        let p = |slack: f64| r.price(&Event::ViaSqueeze { wire: 0, at: Vec2::default(), slack_nm: slack, via_nm: 800e3 }, &WireCtx::default());
        assert_eq!(p(800e3), 0.0);
        assert!((p(400e3) - 2.5).abs() < 1e-9);
        assert_eq!(p(0.0), 5.0);
        assert_eq!(p(-1e5), 5.0);
    }
}
