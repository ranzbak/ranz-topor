//! Crossing in P2: 20 mm per crossing of two different nets (TopoR took a 13-20 mm detour rather
//! than two vias, so a crossing costs at least that and at most one via pair). A crossing of a
//! net with itself is electrically harmless (a loop event) and is its own factor at 0.

use super::{Event, Factor};

pub struct Crossing;

impl Factor for Crossing {
    fn name(&self) -> &'static str {
        "crossing"
    }
    fn doc(&self) -> &'static str {
        "Two wires of different nets cross in the projection (P2); P3 turns it into a layer change."
    }
    fn unit(&self) -> &'static str {
        "per crossing"
    }
    fn default_weight(&self) -> f64 {
        20.0
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::Crossing { same_net: false, .. } => 1.0,
            _ => 0.0,
        }
    }
}

pub struct SameNetCrossing;

impl Factor for SameNetCrossing {
    fn name(&self) -> &'static str {
        "same_net_crossing"
    }
    fn doc(&self) -> &'static str {
        "A wire crosses a wire of its own net (a loop); no constraint on the layers."
    }
    fn unit(&self) -> &'static str {
        "per crossing"
    }
    fn default_weight(&self) -> f64 {
        0.0
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::Crossing { same_net: true, .. } => 1.0,
            _ => 0.0,
        }
    }
}
