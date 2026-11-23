//! pour-pad-join (fail fast): a board whose every connection is already made has nothing to route.
//! The route ends before the plane stubs, the projection and any TopoR loop, also in the dialog's
//! mode (no stop at "Routes stable", no cycle cap): before, an empty plan spun through empty loop
//! cycles until the time limit (a complete small board: 40 075 cycles per run in 20 s, a 5.8 MB
//! report line; the private board P: about 150 s of plane stubs and projection first).
//!
//! Fixture (synthetic, KiCad frame, mm): net N with pads P (5, 5) and Q (20, 5) on F.Cu, joined by a
//! kept 0.25 mm track; net M with pads A (5, 15) and B (20, 15), not joined (the control case).

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::RouteResult;
use topor_engine::model::Item;
use topor_engine::topo::core::{NOTHING_TO_ROUTE, STITCH_ONLY};

const MM: i64 = 1_000_000;

fn pad(r: &str, net: i32, x: i64, y: i64) -> serde_json::Value {
    serde_json::json!({
        "uuid": format!("00000000-0000-4000-8000-{:012}", x * 1000 + y),
        "ref": r, "number": "1", "net": net, "attr": "smd",
        "center": [x * MM, y * MM], "rotation_deg": 0.0, "layers": [0],
        "shapes": {"0": {"kind": "rect", "w": MM, "h": MM}},
        "drill": null, "local_clearance": null, "net_tie_group": null, "spoke_angle_deg": 45.0
    })
}

/// `open_m`: whether net M (A, B) is still to route.
fn model(open_m: bool) -> BoardModel {
    let (w, h) = (30 * MM, 20 * MM);
    let cls = serde_json::json!({"clearance": 200000, "width": 250000, "via_d": 600000, "via_drill": 300000, "netclass": "Default", "width_source": "default", "min_width": 0});
    let mut pads = vec![pad("P", 1, 5, 5), pad("Q", 1, 20, 5)];
    if open_m {
        pads.extend([pad("A", 2, 5, 15), pad("B", 2, 20, 15)]);
    }
    let j = serde_json::json!({
        "units": "nm", "board": "nothing_to_route",
        "layers": [{"id": 0, "name": "F.Cu", "kind": "signal", "routable": true}, {"id": 2, "name": "B.Cu", "kind": "signal", "routable": true}],
        "nets": [{"code": 1, "name": "N"}, {"code": 2, "name": "M"}],
        "pads": pads,
        "vias": [],
        "kept": [{"uuid": "k1", "net": 1, "layer": 0, "width": 250000, "prim": {"kind": "seg", "a": [5 * MM, 5 * MM], "b": [20 * MM, 5 * MM]}}],
        "copper_shapes": [], "holes": [],
        "outline": [
            {"kind": "seg", "a": [0, 0], "b": [w, 0]}, {"kind": "seg", "a": [w, 0], "b": [w, h]},
            {"kind": "seg", "a": [w, h], "b": [0, h]}, {"kind": "seg", "a": [0, h], "b": [0, 0]}
        ],
        "keepouts": [], "net_ties": [], "pours": [], "teardrops": [], "dangling_ends": [],
        "rules": {"min_clearance": 200000, "min_track_width": 250000, "min_via_diameter": 400000, "min_via_drill": 300000, "min_annular": 50000, "hole_clearance": 250000, "via_hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000, "max_error": 5000, "min_connection": 0, "profile": "board", "predefined_vias": [], "default": cls, "per_net": {"1": cls, "2": cls}, "floors_applied": [], "fallbacks": [], "warnings": [], "ignored_checks": [], "clearance_regions": [], "outline_half_stroke": 25000}
    });
    BoardModel::from_json(&j.to_string()).unwrap()
}

/// The dialog's options (no stop at stable, no finish at stable: the loop has no cycle cap), 5 runs,
/// a 5 s limit: without the early end the empty loop runs until that limit. `set`: further options.
fn dialog_route(m: &BoardModel) -> (RouteResult, f64) {
    dialog_route_with(m, &[])
}

fn dialog_route_with(m: &BoardModel, set: &[(&str, serde_json::Value)]) -> (RouteResult, f64) {
    let mut val = serde_json::to_value(Options::default()).unwrap();
    for (k, v) in set {
        val[*k] = v.clone();
    }
    for (k, v) in [("stop_when_stable", serde_json::json!(false)), ("finish_when_stable", serde_json::json!(false)), ("finish_patience", serde_json::json!(0)), ("wall_clock_s", serde_json::json!(5.0))] {
        val[k] = v;
    }
    let o: Options = serde_json::from_value(val).unwrap();
    let t = std::time::Instant::now();
    let r = topor_engine::route::route_with(m, &o, &RunControl::new());
    (r, t.elapsed().as_secs_f64())
}

#[test]
fn a_complete_board_ends_at_once_with_nothing_to_route() {
    let (r, s) = dialog_route(&model(false));
    assert_eq!(r.stop_reason, "nothing_to_route");
    assert!(r.items.is_empty() && r.connections.is_empty());
    assert_eq!(r.warnings.first().map(String::as_str), Some(NOTHING_TO_ROUTE));
    assert!(!r.warnings.iter().any(|w| w.starts_with("TopoR loop") || w.starts_with("ERROR")), "{:?}", r.warnings);
    // no loop ran: far below the 5 s limit the empty loop used to run into
    assert!(s < if cfg!(debug_assertions) { 3.0 } else { 1.0 }, "{s:.2} s");
    assert!(r.limit.as_ref().is_some_and(|l| !l.reached), "{:?}", r.limit);
}

#[test]
fn a_board_with_one_open_connection_still_routes_it() {
    // control: one open net keeps the full route (the early end needs an empty decomposition)
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["topo_rounds"] = serde_json::json!(1);
    let o: Options = serde_json::from_value(val).unwrap();
    let r = topor_engine::route::route_with(&model(true), &o, &RunControl::new());
    assert_ne!(r.stop_reason, "nothing_to_route");
    assert_eq!((r.connections.len(), r.connections.iter().filter(|c| c.status == "routed").count()), (1, 1));
    assert!(!r.warnings.iter().any(|w| w == NOTHING_TO_ROUTE));
}

/// A complete board (net GND: one THT pad U3.1 in its pour, so no connection) whose F.Cu GND fill
/// (2..32 x 2..32 mm) has its lower part behind a 0.7 mm neck at (16.2..16.9, 19.6..20.4) with no via
/// or through-hole pad of the net in it; the B.Cu fill is whole. The thin-neck stitching
/// (`[planes] stitch_thin_mm`, default 1 mm) puts vias into that part; a full route does so on its
/// picked result even with 0 connections (11 vias, 2026-11-23, early end off).
fn neck_model() -> BoardModel {
    let fill_f = r#"{ "layer": 0, "outer": [[2000000,2000000],[32000000,2000000],[32000000,19600000],[16900000,19600000],[16900000,20400000],[32000000,20400000],[32000000,32000000],[2000000,32000000],[2000000,20400000],[16200000,20400000],[16200000,19600000],[2000000,19600000]] }"#;
    let fill_b = r#"{ "layer": 31, "outer": [[2000000,2000000],[32000000,2000000],[32000000,32000000],[2000000,32000000]] }"#;
    let s = format!(
        r#"{{ "units": "nm", "board": "neck",
      "layers": [ {{ "id": 0, "name": "F.Cu", "kind": "signal", "routable": true }}, {{ "id": 31, "name": "B.Cu", "kind": "signal", "routable": true }} ],
      "nets": [ {{ "code": 1, "name": "GND" }}, {{ "code": 2, "name": "SIG" }} ],
      "outline": [ {{ "kind": "seg", "a": [0, 0], "b": [40000000, 0] }}, {{ "kind": "seg", "a": [40000000, 0], "b": [40000000, 34000000] }},
        {{ "kind": "seg", "a": [40000000, 34000000], "b": [0, 34000000] }}, {{ "kind": "seg", "a": [0, 34000000], "b": [0, 0] }} ],
      "pads": [ {{ "uuid": "U3", "ref": "U3", "number": "1", "net": 1, "attr": "pth", "center": [20000000, 8000000], "layers": [0, 31],
        "shapes": {{ "0": {{ "kind": "circle", "d": 1500000 }}, "31": {{ "kind": "circle", "d": 1500000 }} }} }} ],
      "pours": [ {{ "uuid": "zz", "net": 1, "layers": [0, 31], "pads": ["U3.1", "U3.1"],
        "vias": [], "clearance": 200000, "min_thickness": 254000, "thermal_gap": 500000,
        "spoke_width": 500000, "pad_connection": 2, "island_removal": 0, "fills": [ {fill_f}, {fill_b} ] }} ],
      "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0 }} }}"#
    );
    BoardModel::from_json(&s).unwrap()
}

fn vias(r: &RouteResult) -> usize {
    r.items.iter().filter(|i| matches!(i, Item::Via(_))).count()
}

#[test]
fn a_complete_board_still_gets_its_pour_stitching() {
    // Nothing to route, but the pours need stitching: the call hands back exactly the vias a full route
    // stitches on its 0-connection result (11, thin-neck class), at once, without a loop.
    let (r, s) = dialog_route(&neck_model());
    assert_eq!(r.stop_reason, "stitch_only", "{:?}", r.warnings);
    assert!(r.connections.is_empty());
    assert_eq!((vias(&r), r.items.len()), (11, 11), "{:?}", r.warnings);
    assert_eq!(r.warnings.first().map(String::as_str), Some(STITCH_ONLY));
    assert!(r.warnings.iter().any(|w| w.contains("thin-neck cores 1 of 1 (11 vias)")), "{:?}", r.warnings);
    assert!(!r.warnings.iter().any(|w| w.starts_with("TopoR loop") || w.starts_with("ERROR")), "{:?}", r.warnings);
    assert!(s < if cfg!(debug_assertions) { 4.0 } else { 2.0 }, "{s:.2} s");
}

#[test]
fn a_complete_board_ends_with_nothing_to_route_only_when_no_stitching_is_due() {
    // The same board with the thin-neck class off (the dropped-island class finds nothing there):
    // nothing to add, the NOTHING_TO_ROUTE end; with the pour stitching off altogether, the same.
    for set in [vec![("stitch_thin_nm", serde_json::json!(0))], vec![("stitch_islands", serde_json::json!(false))]] {
        let (r, _) = dialog_route_with(&neck_model(), &set);
        assert_eq!(r.stop_reason, "nothing_to_route", "{set:?}: {:?}", r.warnings);
        assert!(r.items.is_empty(), "{set:?}");
        assert_eq!(r.warnings.first().map(String::as_str), Some(NOTHING_TO_ROUTE));
    }
}

#[test]
fn a_complete_board_with_area_stitching_gets_its_lattice() {
    // `[planes] stitch_area` (class 4) on the same board without the thin-neck class: the area lattice is
    // work of its own, handed back as the call's vias, as a full route's 0-connection result has it.
    let (r, _) = dialog_route_with(&neck_model(), &[("stitch_thin_nm", serde_json::json!(0)), ("stitch_area", serde_json::json!(true))]);
    assert_eq!(r.stop_reason, "stitch_only", "{:?}", r.warnings);
    assert!(vias(&r) > 0 && vias(&r) == r.items.len(), "{:?}", r.warnings);
    assert_eq!(vias(&r), AREA_VIAS, "{:?}", r.warnings);
}

/// The class-4 lattice of [`neck_model`] with the thin-neck class off, as a full route (early end off)
/// stitches it on its 0-connection result (2026-11-23: 24 vias; thin-neck class on, area off: 11;
/// both classes off, or `stitch_islands` off: 0).
const AREA_VIAS: usize = 24;
