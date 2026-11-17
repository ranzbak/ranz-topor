//! edge-clear: track ends off the pad centre where the centre is illegal (`topo::padend`), and the
//! copper-to-edge clearance measured from the Edge.Cuts centre line (KiCad 10: the stroke width is
//! set to 0, drc_test_provider_edge_clearance.cpp tag 10.0.8 l. 295-298).
//!
//! Fixture `edge/model-edge_pads.json`: the approved fx pattern `edge_pads` (harness/fx_survivors.py)
//! exported by `harness.g0 export`: two 0.2 x 0.5 mm pads 0.6 mm from the left board edge (stroke
//! 0.05 mm, `outline_half_stroke` 25 um), copper-to-edge 0.5 mm, a 0.25 mm track. The track end at a
//! pad centre is 0.025 mm into the edge clearance; the optimum is the straight wire at x = 0.625 mm
//! (plus the drawing's tolerance), inside both pads (main 0a53849: x = 0.600, 1 edge violation).

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::RouteResult;

fn model() -> serde_json::Value {
    let path = format!("{}/tests/fixtures/edge/model-edge_pads.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn route(m: &serde_json::Value, realise: &str) -> RouteResult {
    let m = BoardModel::from_json(&m.to_string()).unwrap();
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["rounds"] = serde_json::json!(1);
    val["realise"] = serde_json::json!(realise);
    let o: Options = serde_json::from_value(val).unwrap();
    topor_engine::route::route_with(&m, &o, &RunControl::new())
}

/// The segments of the result, (a, b) in nm, KiCad frame (as the result JSON writes them).
fn segs(r: &RouteResult) -> Vec<([i64; 2], [i64; 2])> {
    let v = serde_json::to_value(r).unwrap();
    let pt = |p: &serde_json::Value| [p[0].as_i64().unwrap(), p[1].as_i64().unwrap()];
    v["items"].as_array().unwrap().iter().filter(|it| it["prim"]["kind"] == "seg").map(|it| (pt(&it["prim"]["a"]), pt(&it["prim"]["b"]))).collect()
}

fn pad_end_line(r: &RouteResult) -> String {
    r.warnings.iter().find(|w| w.starts_with("pad ends: ")).cloned().unwrap_or_else(|| panic!("no pad ends line in {:?}", r.warnings))
}

#[test]
fn edge_pads_wire_at_the_optimal_x_in_both_modes() {
    let m = model();
    for realise in ["sequential", "erbs"] {
        let r = route(&m, realise);
        assert_eq!(r.routed, 1, "{realise}");
        assert!(r.violations.is_empty(), "{realise}: {:?}", r.violations);
        let s = segs(&r);
        assert_eq!(s.len(), 1, "{realise}: one straight segment, {s:?}");
        let (a, b) = s[0];
        for x in [a[0], b[0]] {
            // 0.5 edge + 0.125 half width = 0.625 mm, plus the drawing's tolerance; never the half
            // stroke on top (0.650) nor the pad centre (0.600)
            assert!((625_000..=625_100).contains(&x), "{realise}: x = {x} nm ({s:?})");
        }
        assert_eq!([a[1].min(b[1]), a[1].max(b[1])], [2_000_000, 9_000_000], "{realise}: from pad to pad, {s:?}");
        if realise == "erbs" {
            // the ERBS itself ends at the anchors (its band matches the run's ends): drawn from it
            let e = r.warnings.iter().find(|w| w.contains("drawn: ") && w.contains("runs from their ERBS")).unwrap_or_else(|| panic!("no ERBS line in {:?}", r.warnings));
            assert!(e.contains("drawn: 1 runs from their ERBS"), "{e}");
        }
        let l = pad_end_line(&r);
        assert!(l.contains("pad ends: 2 track ends moved") && l.contains("largest move 0.025 mm") && l.contains("; 0 pad ends with no legal point"), "{realise}: {l}");
    }
}

/// Pads 0.1 mm further out (x = 0.5: the pad reaches 0.6, the track end needs 0.625): no legal
/// point in the pad, the end stays at the centre (never drop draws it with the violation, priced),
/// and the line counts it.
#[test]
fn no_legal_point_in_the_pad_keeps_the_centre() {
    let mut m = model();
    for p in m["pads"].as_array_mut().unwrap() {
        p["center"][0] = serde_json::json!(500_000);
    }
    let r = route(&m, "sequential");
    assert_eq!(r.routed, 1);
    let s = segs(&r);
    assert!(s.iter().any(|(a, _)| a[0] == 500_000) || s.iter().any(|(_, b)| b[0] == 500_000), "{s:?}");
    let l = pad_end_line(&r);
    assert!(l.contains("pad ends: 0 track ends moved") && l.contains("; 2 pad ends with no legal point"), "{l}");
}

/// A pad in the board corner (0.2 x 0.2 mm at (0.6, 0.6): 0.5 mm from both edges) to a pad inside:
/// the track end moves off both edges, to (0.625, 0.625) mm, still inside the pad; the wire is legal.
#[test]
fn corner_pad_end_moves_off_both_edges() {
    let mut m = model();
    let pads = m["pads"].as_array_mut().unwrap();
    pads[0]["center"] = serde_json::json!([600_000, 600_000]);
    pads[0]["shapes"]["0"] = serde_json::json!({"kind": "rect", "w": 200_000, "h": 200_000});
    pads[1]["center"] = serde_json::json!([4_000_000, 6_000_000]);
    pads[1]["shapes"]["0"] = serde_json::json!({"kind": "rect", "w": 500_000, "h": 500_000});
    for realise in ["sequential", "erbs"] {
        let r = route(&m, realise);
        assert!(r.violations.is_empty(), "{realise}: {:?}", r.violations);
        let s = segs(&r);
        let ends: Vec<[i64; 2]> = s.iter().flat_map(|&(a, b)| [a, b]).filter(|p| p[0] < 800_000 && p[1] < 800_000).collect();
        assert!(!ends.is_empty(), "{realise}: {s:?}");
        for p in &ends {
            assert!((625_000..=626_000).contains(&p[0]) && (625_000..=626_000).contains(&p[1]), "{realise}: end {p:?} ({s:?})");
        }
        assert!(pad_end_line(&r).contains("pad ends: 1 track ends moved"), "{realise}: {}", pad_end_line(&r));
    }
}

/// Every pad centre legal (pads 1 mm in from the edge): nothing moves, the line says 0.
#[test]
fn legal_centres_stay() {
    let mut m = model();
    for p in m["pads"].as_array_mut().unwrap() {
        p["center"][0] = serde_json::json!(1_000_000);
    }
    let r = route(&m, "sequential");
    assert!(r.violations.is_empty(), "{:?}", r.violations);
    let s = segs(&r);
    assert!(s.iter().all(|(a, b)| a[0] == 1_000_000 && b[0] == 1_000_000), "{s:?}");
    assert!(pad_end_line(&r).contains("pad ends: 0 track ends moved"), "{}", pad_end_line(&r));
}

/// The end must lie inside the pad by the drawing's tolerance (1 um), not on its boundary (KiCad's
/// connectivity: the end in the pad): pads whose inner side is 0.5 um past the point the track end
/// needs (x = 0.625 mm + tolerance) give no legal point.
#[test]
fn the_anchor_keeps_inside_the_pad() {
    let mut m = model();
    for p in m["pads"].as_array_mut().unwrap() {
        p["center"][0] = serde_json::json!(525_510);
    }
    let r = route(&m, "sequential");
    let l = pad_end_line(&r);
    assert!(l.contains("pad ends: 0 track ends moved") && l.contains("; 2 pad ends with no legal point"), "{l}");
    // 2 um more room: both move
    for p in m["pads"].as_array_mut().unwrap() {
        p["center"][0] = serde_json::json!(527_510);
    }
    let r = route(&m, "sequential");
    assert!(pad_end_line(&r).contains("pad ends: 2 track ends moved"), "{}", pad_end_line(&r));
    assert!(r.violations.is_empty(), "{:?}", r.violations);
}

/// The push step (kills mutant `no_push`): the left board edge slanted so its normal lies 11.25
/// degrees off the nearest of the 16 fallback rays. A 0.38 mm track on a 0.2 mm pad whose centre
/// is 0.62 mm from the edge (the pad keeps 0.5024): the end needs 0.5 + 0.19 = 0.69 mm, so the
/// nearest legal point is 0.070 mm along the edge's normal (the push, exact for one straight
/// feature); the best ray point is 0.070 / cos 11.25 deg = 0.071 mm away.
#[test]
fn slanted_edge_push_takes_the_nearest_point() {
    let mut m = model();
    // left side from (0, 12) to (12 tan 11.25 deg, 0) mm: inward normal (cos, sin) 11.25 deg
    let x0 = 2_386_946;
    m["outline"] = serde_json::json!([
        {"kind": "seg", "a": [x0, 0], "b": [8_000_000, 0]},
        {"kind": "seg", "a": [0, 12_000_000], "b": [x0, 0]},
        {"kind": "seg", "a": [8_000_000, 0], "b": [8_000_000, 12_000_000]},
        {"kind": "seg", "a": [8_000_000, 12_000_000], "b": [0, 12_000_000]}
    ]);
    let (nx, ny) = (11.25f64.to_radians().cos(), 11.25f64.to_radians().sin());
    let (qx, qy) = (x0 as f64 * 0.5, 6_000_000.0); // the edge at y = 6 mm
    let c = [(qx + 620_000.0 * nx).round() as i64, (qy + 620_000.0 * ny).round() as i64];
    let pads = m["pads"].as_array_mut().unwrap();
    pads[0]["center"] = serde_json::json!(c);
    pads[0]["shapes"]["0"] = serde_json::json!({"kind": "rect", "w": 200_000, "h": 200_000});
    pads[1]["center"] = serde_json::json!([5_000_000, 6_000_000]);
    pads[1]["shapes"]["0"] = serde_json::json!({"kind": "rect", "w": 500_000, "h": 500_000});
    m["rules"]["min_track_width"] = serde_json::json!(380_000);
    m["rules"]["default"]["width"] = serde_json::json!(380_000);
    m["rules"]["per_net"]["1"]["width"] = serde_json::json!(380_000);
    let r = route(&m, "sequential");
    assert_eq!(r.routed, 1);
    assert!(r.violations.is_empty(), "{:?}", r.violations);
    let l = pad_end_line(&r);
    assert!(l.contains("pad ends: 1 track ends moved") && l.contains("largest move 0.070 mm") && l.contains("; 0 pad ends with no legal point"), "{l}");
    // the drawn end: 0.69 mm (+ the drawing's tolerance) from the slanted edge, inside the pad
    let dist = |p: [i64; 2]| (p[0] as f64 - x0 as f64 * (1.0 - p[1] as f64 / 12_000_000.0)) * nx;
    let near: Vec<[i64; 2]> = segs(&r).iter().flat_map(|&(a, b)| [a, b]).filter(|p| (p[0] - c[0]).abs() < 100_000 && (p[1] - c[1]).abs() < 100_000).collect();
    assert!(!near.is_empty(), "{:?}", segs(&r));
    for p in near {
        assert!((690_000.0..=690_200.0).contains(&dist(p)), "end {p:?} at {} nm from the edge", dist(p));
    }
}

/// The ray fallback (kills mutant `no_rays`): a foreign 0.05 mm pad 0.075 mm short of the clearance
/// beside a 0.2 x 1.0 mm pad, the edge far away. The push moves the end 0.125 mm straight away from
/// it, out of the pad (half width 0.1): no push candidate; a ray along the pad (67.5 deg) finds a
/// legal point inside, so the end still moves and nothing is left at the centre.
#[test]
fn rays_find_a_point_where_the_push_leaves_the_pad() {
    let mut m = model();
    let mut foreign = m["pads"][0].clone();
    foreign["uuid"] = serde_json::json!("00000000-0000-0000-0000-00000000f0f0");
    foreign["ref"] = serde_json::json!("J3");
    foreign["net"] = serde_json::json!(2);
    foreign["center"] = serde_json::json!([2_775_000, 2_000_000]);
    foreign["shapes"]["0"] = serde_json::json!({"kind": "rect", "w": 50_000, "h": 50_000});
    let pads = m["pads"].as_array_mut().unwrap();
    pads[0]["center"] = serde_json::json!([3_000_000, 2_000_000]);
    pads[0]["shapes"]["0"] = serde_json::json!({"kind": "rect", "w": 200_000, "h": 1_000_000});
    pads[1]["center"] = serde_json::json!([3_000_000, 9_000_000]);
    pads.push(foreign);
    m["nets"].as_array_mut().unwrap().push(serde_json::json!({"code": 2, "name": "F"}));
    let n1 = m["rules"]["per_net"]["1"].clone();
    m["rules"]["per_net"]["2"] = n1;
    let r = route(&m, "sequential");
    assert_eq!(r.routed, 1);
    let l = pad_end_line(&r);
    assert!(l.contains("pad ends: 1 track ends moved") && l.contains("; 0 pad ends with no legal point"), "{l}");
}
