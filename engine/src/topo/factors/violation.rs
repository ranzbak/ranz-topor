//! Violation (M-B1 "never drop"): copper the drawing could not make legal is written anyway on
//! its best geometry, and every rule it breaks there is priced here, very high and graded, and
//! reported as a warning with its place and cause. Routing everything with conflicts first and
//! then optimising the conflicts away moved routers "from the sequential to the optimising class",
//! with an "enormous" effect on quality (Luzin & Polubasov 2002, Chip News 5/2002, section
//! «Оптимизационный характер», on Specctra). TopoR's path cost has the number of design-rule
//! violations as one of its factors, and its board evaluation counts them next to length and
//! vias: "a design-rule violation cannot be compensated by any reduction of wire length"
//! (Polubasov 2010, Электроника НТБ 6/2010, section «Многовариантная оптимизация»), so the archive
//! ranks the connections with violations (and those without copper) before the objective
//! ([`super::super::archive`]). A connection that
//! stays in the router can still be improved by the later cycles, a dropped one never connects
//! anything (user decision 2026-11-06: "accept DRC errors at a very high social factor cost").
//!
//! One unit is one violation plus its depth x length in units of [`AREA_REF_MM2`]: `amount = 1 +
//! depth_mm x length_mm / 0.01`. Depth is how far the copper is below the rule (the clearance
//! missing; an overlap counts its whole depth), length how long the copper stays below it. A
//! connection without any copper (no topological path, time limit) counts as the worst case of
//! the same scale: its pitch (width + clearance) as the depth over its straight pad-to-pad length.
//! No threshold: a deeper or longer violation always costs more, a shallow one never nothing.

use super::{Event, Factor, Report};

/// Area (mm²) of depth x length that one more unit of the factor stands for.
pub const AREA_REF_MM2: f64 = 0.01;

/// Units of a violation `depth_nm` deep over `len_nm` (see the module doc).
pub fn violation_amount(depth_nm: f64, len_nm: f64) -> f64 {
    1.0 + depth_nm.max(0.0) / 1e6 * len_nm.max(0.0) / 1e6 / AREA_REF_MM2
}

pub struct Violation;

impl Factor for Violation {
    fn name(&self) -> &'static str {
        "violation"
    }
    fn doc(&self) -> &'static str {
        "A KiCad rule the written copper breaks (never drop: a connection the drawing cannot make legal is drawn anyway on its best geometry); 1 per violation plus its depth x length per 0.01 mm², and a connection left without copper as its pitch over its straight length. Reported as a warning with place, type and cause."
    }
    fn unit(&self) -> &'static str {
        "per violation + per 0.01 mm² of depth x length"
    }
    fn default_weight(&self) -> f64 {
        1000.0
    }
    fn default_report(&self) -> Report {
        Report::Warning
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::Violation { depth_nm, len_nm, .. } => violation_amount(depth_nm, len_nm),
            Event::Open { pitch_nm, len_nm, .. } => violation_amount(pitch_nm, len_nm),
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match ev {
            Event::Violation { kind, at, layer, depth_nm, len_nm, against, cause, .. } => format!(
                "violation ({kind}) at ({:.3}, {:.3}) mm on {layer}: {:.3} mm below the rule over {:.3} mm against {against}; first failed check of the connection: {cause}",
                at.x / 1e6,
                -at.y / 1e6,
                depth_nm / 1e6,
                len_nm / 1e6
            ),
            Event::Open { len_nm, cause, .. } => format!("not connected ({:.3} mm straight): {cause}", len_nm / 1e6),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Event, Registry, WireCtx};
    use crate::geom::Vec2;

    fn ev(depth_mm: f64, len_mm: f64) -> Event {
        Event::Violation {
            kind: "clearance".into(),
            at: Vec2 { x: 1e6, y: -2e6 },
            layer: "F.Cu".into(),
            depth_nm: depth_mm * 1e6,
            len_nm: len_mm * 1e6,
            against: "pad:R1:2".into(),
            cause: "no legal geometry".into(),
        }
    }

    #[test]
    fn a_violation_is_very_high_and_graded() {
        let r = Registry::standard();
        let w = WireCtx::default();
        let via = r.price(&Event::Via { wire: 0, count: 1, at: None }, &w);
        let tiny = r.price(&ev(0.001, 0.01), &w);
        assert!(tiny > 30.0 * via, "even a tiny violation costs more than many vias: {tiny}");
        assert!(r.price(&ev(0.1, 1.0), &w) > r.price(&ev(0.05, 1.0), &w), "deeper costs more");
        assert!(r.price(&ev(0.1, 2.0), &w) > r.price(&ev(0.1, 1.0), &w), "longer costs more");
        assert!((r.price(&ev(0.1, 1.0), &w) - 1000.0 * 11.0).abs() < 1e-6);
        let open = Event::Open { pitch_nm: 0.4e6, len_nm: 5e6, cause: "no path".into() };
        assert!(r.price(&open, &w) > r.price(&ev(0.4, 1.0), &w), "an open connection is the worst case");
        let f = r.findings(std::slice::from_ref(&ev(0.1, 1.0)), &w);
        assert_eq!(f.len(), 1);
        assert!(f[0].text.contains("(1.000, 2.000)") && f[0].text.contains("clearance") && f[0].text.contains("pad:R1:2"), "{}", f[0].text);
    }
}
