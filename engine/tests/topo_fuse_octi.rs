//! octi-bundle r2 (the user's A1c rule: every redraw path uses the first draw's rules): the fuse
//! after drawing (D6b, `[search] fuse_branches`) builds its link from the octilinear DP's
//! connectors and prices it on the DP's scale (length, bends, `any_angle`, `junction_angle`), so
//! it never trades a legal octilinear branch for one straight free-angle leg for a little length
//! (wiccon GPIO14: the 45/90 staircase from (139.38, 100.68) to (157, 70.06) became one 35.3 mm
//! free-angle segment; trace reports/octi-bundle-r1-20261120-2317.md).
//!
//! Fixture `fuse_blocked_chord` (synthetic, built here; PROPOSED pattern, the PNG and text are in
//! the round-2 report): net N has three pads, P (10, 10), T (10, 2) above it and B (16, 20) below
//! right; the tree is P-T (the trunk, vertical) and P-B (the branch, 149 degrees from the trunk
//! arm, so the junction-angle price is 0 for any form). Three unconnected 0.5 mm pads stand
//! beside the straight chord P-B, clear of it, on the legs of both its elbows and both its centred
//! Zs, so no single octilinear link from P to B is legal: the first draw finds an octilinear
//! branch around them, and a fuse onto the trunk's copper at P could only offer the straight
//! chord (shorter, legal, free-angle). Expected (the optimal path under the first draw's rules):
//! the drawn octilinear branch stays, 0 free-angle copper. Before (straight link, length and
//! junction angle only): the fuse took the chord, and the octilinear polish could not redraw it.
//!
//! One layer (B.Cu closed), one run, one round: deterministic.

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::RouteResult;

const MM: i64 = 1_000_000;

fn pad(r: &str, net: i32, x: f64, y: f64, s: f64) -> serde_json::Value {
    let n = |v: f64| (v * MM as f64).round() as i64;
    serde_json::json!({
        "uuid": format!("00000000-0000-4000-8000-{:012}", (x * 1000.0) as i64 * 100_000 + (y * 1000.0) as i64),
        "ref": r, "number": "1", "net": net, "attr": "smd",
        "center": [n(x), n(y)], "rotation_deg": 0.0, "layers": [0],
        "shapes": {"0": {"kind": "rect", "w": n(s), "h": n(s)}},
        "drill": null, "local_clearance": null, "net_tie_group": null, "spoke_angle_deg": 45.0
    })
}

/// The fixture board (KiCad frame, mm): see the module doc.
fn model() -> BoardModel {
    let (w, h) = (30 * MM, 30 * MM);
    let cls = serde_json::json!({"clearance": 200000, "width": 250000, "via_d": 600000, "via_drill": 300000, "netclass": "Default", "width_source": "default", "min_width": 0});
    let j = serde_json::json!({
        "units": "nm", "board": "fuse_blocked_chord",
        "layers": [{"id": 0, "name": "F.Cu", "kind": "signal", "routable": true}, {"id": 2, "name": "B.Cu", "kind": "signal", "routable": true}],
        "nets": [{"code": 1, "name": "N"}],
        "pads": [
            pad("P", 1, 10.0, 10.0, 1.0), pad("T", 1, 10.0, 2.0, 1.0), pad("B", 1, 16.0, 20.0, 1.0),
            // On the second leg of the elbow down-then-45 (corner (10, 14)).
            pad("X1", 0, 11.0, 14.6, 0.5),
            // On the 45 degree leg of the Z down-45-down (corners (10, 12), (16, 18)).
            pad("X2", 0, 11.0, 13.0, 0.5),
            // On the first 45 degree leg of the elbow 45-then-down and of the Z 45-down-45.
            pad("X3", 0, 12.0, 11.6, 0.5),
        ],
        "vias": [], "kept": [], "copper_shapes": [], "holes": [],
        "outline": [
            {"kind": "seg", "a": [0, 0], "b": [w, 0]}, {"kind": "seg", "a": [w, 0], "b": [w, h]},
            {"kind": "seg", "a": [w, h], "b": [0, h]}, {"kind": "seg", "a": [0, h], "b": [0, 0]}
        ],
        "keepouts": [{"name": "", "layers": [2], "poly": [[0, 0], [w, 0], [w, h], [0, h]], "no_tracks": true, "no_vias": true, "no_pour": false, "no_pads": false, "no_footprints": false}],
        "net_ties": [], "pours": [], "teardrops": [], "dangling_ends": [],
        "rules": {"min_clearance": 200000, "min_track_width": 250000, "min_via_diameter": 400000, "min_via_drill": 300000, "min_annular": 50000, "hole_clearance": 250000, "via_hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000, "max_error": 5000, "min_connection": 0, "profile": "board", "predefined_vias": [], "default": cls, "per_net": {"1": cls}, "floors_applied": [], "fallbacks": [], "warnings": [], "ignored_checks": [], "clearance_regions": [], "outline_half_stroke": 25000}
    });
    BoardModel::from_json(&j.to_string()).unwrap()
}

fn route(extra: &[(&str, serde_json::Value)]) -> RouteResult {
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["topo_rounds"] = serde_json::json!(1);
    for (k, v) in extra {
        val[*k] = v.clone();
    }
    let o: Options = serde_json::from_value(val).unwrap();
    topor_engine::route::route_with(&model(), &o, &RunControl::new())
}

/// Track segments off the 0/45/90 degree grid (the harness judge's rule), as length mm.
fn free_angle(r: &RouteResult) -> Vec<f64> {
    r.items
        .iter()
        .filter_map(|i| if let Item::Track(t) = i { Some(t) } else { None })
        .filter_map(|t| {
            let (a, b) = (t.prim.start(), t.prim.end());
            let (dx, dy) = (((b.x - a.x) as f64).abs(), ((b.y - a.y) as f64).abs());
            (dx > 1e3 && dy > 1e3 && (dx - dy).abs() > 0.01 * dx.max(dy) + 1e3).then(|| dx.hypot(dy) / 1e6)
        })
        .collect()
}

fn tracks(r: &RouteResult) -> Vec<String> {
    r.items
        .iter()
        .filter_map(|i| if let Item::Track(t) = i { Some(t) } else { None })
        .map(|t| {
            let (a, b) = (t.prim.start(), t.prim.end());
            format!("({:.3},{:.3})-({:.3},{:.3})", a.x as f64 / 1e6, a.y as f64 / 1e6, b.x as f64 / 1e6, b.y as f64 / 1e6)
        })
        .collect()
}

#[test]
fn fuse_keeps_an_octilinear_branch_whose_chord_is_free_angle() {
    let r = route(&[]);
    let viol: usize = r.connections.iter().map(|c| c.violations).sum();
    assert_eq!(r.routed, r.connections_total, "{:?}", r.warnings);
    assert_eq!(viol, 0, "{:?}", r.warnings);
    let fa = free_angle(&r);
    assert!(fa.is_empty(), "free-angle {fa:?}: {:?} {:?}", tracks(&r), r.warnings);
    // The fixture reaches the fuse: without it the copper is the same octilinear branch.
    let off = route(&[("fuse_branches", serde_json::json!(false))]);
    assert!(free_angle(&off).is_empty(), "{:?}", tracks(&off));
}
