//! `RouteResult.stubs` rows (the result contract with the plugin).
//!
//! The pad exit stubs of the classic router (`plan_stubs`, A6 / NX, f45dc37) were removed with it
//! (green field 2f1430a, dead code removed in CL); the topological core plans no stubs, so the list
//! is always empty. The row type stays because the plugin reads the field.

use serde::Serialize;

/// `RouteResult.stubs` row: one planned stub of a pass.
#[derive(Clone, Debug, Serialize)]
pub struct StubOut {
    pub pad: String,
    pub net: i32,
    pub layer: u32,
    pub width: i64,
    pub net_width: i64,
    /// KiCad frame, nm.
    pub start: [i64; 2],
    pub end: [i64; 2],
    pub length_nm: i64,
    /// Part of `length_nm` outside the pad's copper.
    pub outside_nm: i64,
    /// Narrower than the net width (NX).
    pub necked: bool,
    /// Lateral offset from the pad centre line, nm (0 = centred).
    pub offset_nm: i64,
    /// A bridge: the other pad (`"U7.7"`) the stub ends in; the stub is the whole connection.
    pub bridge: Option<String>,
    /// Emitted with at least one routed (or partial) connection of the pass.
    pub used: bool,
}
