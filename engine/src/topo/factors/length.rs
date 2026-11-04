//! Length: 1 mm of copper costs 1 mm, scaled by the wire's (w+s)/(w0+s0) (a wide wire uses more of
//! every channel, so its length is dearer).

use super::{Event, Factor};

pub struct Length;

impl Factor for Length {
    fn name(&self) -> &'static str {
        "length"
    }
    fn doc(&self) -> &'static str {
        "Copper length; a wide wire's mm count more by (width + clearance) / default (width + clearance)."
    }
    fn unit(&self) -> &'static str {
        "per mm"
    }
    fn default_weight(&self) -> f64 {
        1.0
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::Length { mm, scale, .. } => mm * scale,
            _ => 0.0,
        }
    }
}
