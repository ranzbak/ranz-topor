//! Via: 25 mm per via (the pick rule measured on TopoR). A pass that fixes the layer (over
//! another net's SMD pad, or through a gap too narrow on some layer) is priced as one via: it
//! forces the wire to that layer, so at most one via is needed to get there.

use super::{Event, Factor};

pub struct Via;

impl Factor for Via {
    fn name(&self) -> &'static str {
        "via"
    }
    fn doc(&self) -> &'static str {
        "A through via; also a stretch that is forced onto one layer (counts as up to one via)."
    }
    fn unit(&self) -> &'static str {
        "per via"
    }
    fn default_weight(&self) -> f64 {
        25.0
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::Via { count, .. } => f64::from(count),
            Event::LayerForced { .. } => 1.0,
            _ => 0.0,
        }
    }
}
