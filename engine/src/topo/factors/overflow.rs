//! Overflow: a pass through an edge whose room is exceeded. Not a social factor of the plan's
//! table but the price P2 puts on a violation it may still repair (50 mm); P3 and P4 count it.

use super::{Event, Factor};

pub struct Overflow;

impl Factor for Overflow {
    fn name(&self) -> &'static str {
        "overflow"
    }
    fn doc(&self) -> &'static str {
        "A pass through a cut whose room is exceeded, or a crossing of two stretches that no layer assignment can separate (both held to the same single layer in its triangle) (P2 search penalty; counted as a violation)."
    }
    fn unit(&self) -> &'static str {
        "per overflowing pass"
    }
    fn default_weight(&self) -> f64 {
        50.0
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::Overflow { .. } => 1.0,
            _ => 0.0,
        }
    }
}
