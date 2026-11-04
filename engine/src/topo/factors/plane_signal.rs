//! Signal on a plane layer: 10 x its length (this factor adds 10 mm per mm on top of the length
//! factor's 1) and, as `plane_cut`, 25 mm per cut of the plane. A plane is a reference for its own
//! net; routing a signal there is allowed but dear.

use super::{Event, Factor};

pub struct PlaneSignal;

impl Factor for PlaneSignal {
    fn name(&self) -> &'static str {
        "plane_signal"
    }
    fn doc(&self) -> &'static str {
        "Signal copper on a plane layer, on top of its length."
    }
    fn unit(&self) -> &'static str {
        "per mm on a plane"
    }
    fn default_weight(&self) -> f64 {
        10.0
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::PlaneLength { mm, .. } => mm,
            _ => 0.0,
        }
    }
}

pub struct PlaneCut;

impl Factor for PlaneCut {
    fn name(&self) -> &'static str {
        "plane_cut"
    }
    fn doc(&self) -> &'static str {
        "A signal wire cuts the copper of a plane layer (a slot in the return path)."
    }
    fn unit(&self) -> &'static str {
        "per cut"
    }
    fn default_weight(&self) -> f64 {
        25.0
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::PlaneCut { .. } => 1.0,
            _ => 0.0,
        }
    }
}
