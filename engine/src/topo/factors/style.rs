//! Drawing style (M-45, plan "45 degrees is a cost, not a law", decision 2026-11-04): the
//! octilinear drawing (`[drawing] style = "octilinear"`) breaks 0/45/90 degrees only where they do
//! not fit, and makes a bend sharper than 90 degrees only where nothing else fits. Both are prices
//! the drawing weighs against length and bends, and both are reported as warnings with place and
//! angle, so every break can be reviewed by hand.
//!
//! * `any_angle`: 15 mm per stretch that is not at 0/45/90 degrees plus 2 per mm of it (amount
//!   `1 + mm x 2/15`). No social side of its own: when an octilinear shape would take a passage
//!   from another wire, the clearance check and that wire's prices already see it.
//! * `acute_bend`: 50 mm per bend with an interior angle under 90 degrees, rising to twice that
//!   as the angle closes (amount `1 + (90 - interior) / 90`): acid traps and impedance.
//! * `planned_room`: the drawing is greedy in wire order, so a wire drawn early can take the
//!   corridor the plan keeps for a neighbour drawn later (its lane on the edges and the strip
//!   between its planned crossings inside the triangles). Entering it costs, per mm of the drawn
//!   shape, `depth / pitch + (depth / pitch)^2` (depth past the neighbour's planned path less the
//!   lane spacing, pitch = the wire's width + clearance): smooth and growing, no cutoff
//!   (Bast, Brosi & Storandt 2020 section 4.3: room is kept for the lines routed later).
//!   The same weight prices the vias the drawing places itself (no planned site, or the planned
//!   one fails): a via disc reaching into the planned corridor of another net's wire not drawn
//!   yet (its taut and its packed chord), and a via pair whose two discs leave less than the
//!   wires the plan crosses between them need, cost `pitch x (x + x^2)`, `x = depth / pitch`;
//!   free room short of one via diameter pays the same; in the via's first triangle with a
//!   valid site the drawing takes the least price, then the most room (Dayan & Dai 1991 §6-§8
//!   p. 6).

use super::{Event, Factor, Report};
use crate::topo::octi::{acute_amount, any_angle_amount};

pub struct AnyAngle;

impl Factor for AnyAngle {
    fn name(&self) -> &'static str {
        "any_angle"
    }
    fn doc(&self) -> &'static str {
        "Octilinear drawing: a stretch that is not at 0, 45 or 90 degrees (where 45 degrees does not fit); 1 per stretch plus 2/15 per mm of it (15 mm + 2 x its length at the default weight)."
    }
    fn unit(&self) -> &'static str {
        "per any-angle stretch (plus 2/15 per mm)"
    }
    fn default_weight(&self) -> f64 {
        15.0
    }
    fn default_report(&self) -> Report {
        Report::Warning
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::AnyAngle { mm, .. } => any_angle_amount(mm * 1e6),
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match *ev {
            Event::AnyAngle { at, deg, mm, .. } => format!("not at 0/45/90 degrees ({deg:.1} deg) over {mm:.2} mm at ({:.3}, {:.3}) mm", at.x / 1e6, -at.y / 1e6),
            _ => String::new(),
        }
    }
}

pub struct AcuteBend;

impl Factor for AcuteBend {
    fn name(&self) -> &'static str {
        "acute_bend"
    }
    fn doc(&self) -> &'static str {
        "A bend sharper than 90 degrees (interior angle below 90: acid trap, impedance); 1 per bend, rising to 2 as the interior angle closes to 0."
    }
    fn unit(&self) -> &'static str {
        "per acute bend"
    }
    fn default_weight(&self) -> f64 {
        50.0
    }
    fn default_report(&self) -> Report {
        Report::Warning
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::AcuteBend { interior_deg, .. } => acute_amount(180.0 - interior_deg),
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match *ev {
            Event::AcuteBend { at, interior_deg, .. } => format!("acute bend ({interior_deg:.1} deg inside) at ({:.3}, {:.3}) mm", at.x / 1e6, -at.y / 1e6),
            _ => String::new(),
        }
    }
}

pub struct PlannedRoom;

impl Factor for PlannedRoom {
    fn name(&self) -> &'static str {
        "planned_room"
    }
    fn doc(&self) -> &'static str {
        "Drawing: a drawn shape in the corridor the plan keeps for a neighbour not drawn yet; per mm, depth / pitch + (depth / pitch)^2 (depth past the neighbour's planned path less the lane spacing)."
    }
    fn unit(&self) -> &'static str {
        "per mm inside a later wire's planned room, times (d/p + (d/p)^2)"
    }
    fn default_weight(&self) -> f64 {
        5.0
    }
    fn default_report(&self) -> Report {
        Report::Off
    }
    fn amount(&self, _ev: &Event) -> f64 {
        0.0
    }
}
