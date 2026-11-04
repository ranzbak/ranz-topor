//! Ignored rule: a KiCad check whose severity is `ignore` (Board Setup -> Violation Severity) is
//! not a hard rule for the router. Violating it is priced here (high by default) and reported as a
//! warning at the place. The rule's value is read from KiCad; nothing about it is stored here.
//! Wiring the events into P2 is S2's job; this file fixes the vocabulary.

use super::{Event, Factor, Report};

pub struct IgnoredRule;

impl Factor for IgnoredRule {
    fn name(&self) -> &'static str {
        "ignored_rule"
    }
    fn doc(&self) -> &'static str {
        "Violating a KiCad check that the project set to ignore; reported as a warning where it happens."
    }
    fn unit(&self) -> &'static str {
        "per violation"
    }
    fn default_weight(&self) -> f64 {
        100.0
    }
    fn default_report(&self) -> Report {
        Report::Warning
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::IgnoredRuleViolated { .. } => 1.0,
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match ev {
            Event::IgnoredRuleViolated { rule, at, amount_nm } => format!(
                "ignored KiCad check {rule} violated at ({:.3}, {:.3}) mm by {:.3} mm",
                at.x / 1e6,
                -at.y / 1e6,
                amount_nm / 1e6
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
    fn an_ignored_rule_is_priced_and_reported() {
        let r = Registry::standard();
        let ev = Event::IgnoredRuleViolated { rule: "copper_edge_clearance".into(), at: Vec2 { x: 1e6, y: -2e6 }, amount_nm: 0.1e6 };
        assert_eq!(r.price(&ev, &WireCtx::default()), 100.0);
        let f = r.findings(std::slice::from_ref(&ev), &WireCtx::default());
        assert_eq!(f.len(), 1);
        assert!(f[0].text.contains("copper_edge_clearance") && f[0].text.contains("(1.000, 2.000)"), "{}", f[0].text);
    }
}
