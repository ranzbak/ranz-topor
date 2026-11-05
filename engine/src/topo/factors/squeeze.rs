//! Squeeze: a wire passes between two stretches of other copper through a cut that is left with
//! less slack than one track pitch. Nobody else gets through there afterwards, and the metric
//! drawing has to pull both neighbours tight. A social factor (plan "Open points" 2): the price
//! grows linearly as the slack falls from one pitch to zero.

use super::{Event, Factor};

pub struct Squeeze;

/// Units of the squeeze of a pass that leaves `slack_nm` in a cut where one more wire needs
/// `pitch_nm` (its width plus clearance): 0 with a pitch of slack or more, 1 with none.
pub fn squeeze_amount(slack_nm: f64, pitch_nm: f64) -> f64 {
    if pitch_nm <= 0.0 {
        return 0.0;
    }
    ((pitch_nm - slack_nm) / pitch_nm).clamp(0.0, 1.0)
}

impl Factor for Squeeze {
    fn name(&self) -> &'static str {
        "squeeze"
    }
    fn doc(&self) -> &'static str {
        "A pass between two other stretches through a cut left with less slack than one track pitch (width plus clearance); linear from one pitch of slack (free) to none."
    }
    fn unit(&self) -> &'static str {
        "per fully squeezed pass"
    }
    fn default_weight(&self) -> f64 {
        5.0
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::Squeeze { slack_nm, pitch_nm, .. } => squeeze_amount(slack_nm, pitch_nm),
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Event, Registry, WireCtx};

    #[test]
    fn the_price_falls_linearly_with_the_slack() {
        let r = Registry::standard();
        let p = |slack: f64| r.price(&Event::Squeeze { wire: 0, edge: 0, slack_nm: slack, pitch_nm: 400e3 }, &WireCtx::default());
        assert_eq!(p(400e3), 0.0);
        assert_eq!(p(1e6), 0.0);
        assert!((p(200e3) - 2.5).abs() < 1e-9);
        assert_eq!(p(0.0), 5.0);
    }
}
