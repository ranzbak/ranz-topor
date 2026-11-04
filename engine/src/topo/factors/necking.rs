//! Necking: 30 mm per necked stretch (a wire narrowed below its netclass width, down to the
//! floor from `topor.toml`, never below the fab minimum read from KiCad). Reported as a warning.

use super::{Event, Factor, Report};

pub struct Necking;

impl Factor for Necking {
    fn name(&self) -> &'static str {
        "necking"
    }
    fn doc(&self) -> &'static str {
        "A stretch narrowed below the netclass width (not below the fab minimum)."
    }
    fn unit(&self) -> &'static str {
        "per necked stretch"
    }
    fn default_weight(&self) -> f64 {
        30.0
    }
    fn default_report(&self) -> Report {
        Report::Warning
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::Necking { .. } => 1.0,
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match *ev {
            Event::Necking { from_nm, to_nm, mm, .. } => {
                format!("necked from {:.3} mm to {:.3} mm over {:.2} mm", from_nm / 1e6, to_nm / 1e6, mm)
            }
            _ => String::new(),
        }
    }
}
