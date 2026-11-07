//! Necking: 30 mm per necked stretch (a wire narrowed below its netclass width, down to the
//! floor from `topor.toml`, never below the fab minimum read from KiCad), graded by the depth of
//! the neck (M-C): amount `1 + (nominal - necked) / nominal`, so a deeper neck costs more and a
//! neck never costs less than one stretch (TopoR 6.3 manual pp. 72-73: shrinkages are a
//! criterion of their own). P2 prices it where a stretch starts to neck. Reported as a warning.

/// Amount of one necked stretch from width `from` to `to` (nm): `1 + (from - to) / from`.
pub fn necking_amount(from: f64, to: f64) -> f64 {
    if from <= 0.0 {
        return 1.0;
    }
    1.0 + ((from - to) / from).clamp(0.0, 1.0)
}

use super::{Event, Factor, Report};

pub struct Necking;

impl Factor for Necking {
    fn name(&self) -> &'static str {
        "necking"
    }
    fn doc(&self) -> &'static str {
        "A stretch narrowed below the netclass width (not below the fab minimum); more the deeper the neck (1 + depth / width)."
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
            Event::Necking { from_nm, to_nm, .. } => necking_amount(from_nm, to_nm),
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
