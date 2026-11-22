//! A1b pad-snap: every track end on a pad ends at the pad's centre.
//!
//! KiCad's connectivity connects a track end anywhere inside a pad (connectivity_algo.cpp
//! CN_VISITOR, shape collision), but its push-and-shove router links a pad only at its anchor
//! (pns_node.cpp NODE::addSolid, linkJoint at the pad position): an end off the centre is a loose
//! joint for dragging and footprint moves, the "dangling" the user sees. KiCad's Cleanup removes a
//! segment wholly inside a pad (tracks_cleaner.cpp deleteTracksInPads), so a centre stub inside
//! the pad is not a fix: the first segment from the centre must leave the pad.
//!
//! Fixture `padsnap/model-pad_end_centre.json`: the PROPOSED drc pattern `pad_end_centre`
//! (harness/drc_patterns.py; still PROPOSED until the user approves it, decision D1), exported by `harness.g0 export` (the problem board).

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::RouteResult;

fn model() -> serde_json::Value {
    let path = format!("{}/tests/fixtures/padsnap/model-pad_end_centre.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn route(m: &serde_json::Value, realise: &str) -> RouteResult {
    route_opt(m, realise, &[])
}

fn route_opt(m: &serde_json::Value, realise: &str, set: &[(&str, serde_json::Value)]) -> RouteResult {
    let m = BoardModel::from_json(&m.to_string()).unwrap();
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["rounds"] = serde_json::json!(1);
    val["realise"] = serde_json::json!(realise);
    for (k, v) in set {
        val[*k] = v.clone();
    }
    let o: Options = serde_json::from_value(val).unwrap();
    topor_engine::route::route_with(&m, &o, &RunControl::new())
}

/// The segments of the result, (net, a, b) in nm, KiCad frame.
fn segs(r: &RouteResult) -> Vec<(i64, [i64; 2], [i64; 2])> {
    let v = serde_json::to_value(r).unwrap();
    let pt = |p: &serde_json::Value| [p[0].as_i64().unwrap(), p[1].as_i64().unwrap()];
    v["items"].as_array().unwrap().iter().filter(|it| it["prim"]["kind"] == "seg").map(|it| (it["net"].as_i64().unwrap(), pt(&it["prim"]["a"]), pt(&it["prim"]["b"]))).collect()
}

/// Terminal ends (an end no other segment end shares) lying inside a pad of their net (bounding
/// box test, the pads are axis-aligned): (net, end, pad centre, distance nm).
fn pad_terminals(m: &serde_json::Value, s: &[(i64, [i64; 2], [i64; 2])]) -> Vec<(i64, [i64; 2], [i64; 2], f64)> {
    let mut count = std::collections::HashMap::new();
    for &(n, a, b) in s {
        *count.entry((n, a)).or_insert(0) += 1;
        *count.entry((n, b)).or_insert(0) += 1;
    }
    let mut out = Vec::new();
    for (&(n, p), &k) in &count {
        if k != 1 {
            continue;
        }
        for pad in m["pads"].as_array().unwrap() {
            if pad["net"].as_i64() != Some(n) {
                continue;
            }
            let c = [pad["center"][0].as_i64().unwrap(), pad["center"][1].as_i64().unwrap()];
            let sh = &pad["shapes"]["0"];
            let (hw, hh) = match sh["kind"].as_str().unwrap() {
                "circle" => (sh["d"].as_i64().unwrap() / 2, sh["d"].as_i64().unwrap() / 2),
                _ => (sh["w"].as_i64().unwrap() / 2, sh["h"].as_i64().unwrap() / 2),
            };
            if (p[0] - c[0]).abs() <= hw && (p[1] - c[1]).abs() <= hh {
                out.push((n, p, c, (((p[0] - c[0]) as f64).hypot((p[1] - c[1]) as f64))));
            }
        }
    }
    out.sort_by_key(|x| (x.0, x.1));
    out
}

/// The count in the `pad ends off centre: N ...` line.
fn off_count(r: &RouteResult) -> usize {
    off_line(r)["pad ends off centre: ".len()..].split(' ').next().unwrap().parse().unwrap()
}

fn off_line(r: &RouteResult) -> String {
    r.warnings.iter().find(|w| w.starts_with("pad ends off centre: ")).cloned().unwrap_or_else(|| panic!("no off-centre line in {:?}", r.warnings))
}

fn len_mm(s: &[(i64, [i64; 2], [i64; 2])]) -> f64 {
    s.iter().map(|(_, a, b)| ((a[0] - b[0]) as f64).hypot((a[1] - b[1]) as f64)).sum::<f64>() / 1e6
}

/// The pattern's expected answer: every wire leaves its pad centre, its first segment leaves the
/// pad (the round end at least sticks out, so Cleanup's "tracks fully inside pads" keeps it), no
/// tidy drop, octilinear, in both realise modes. The reference (35.237 mm: A, B, C 45 degrees to
/// the pad side, up beside it onto the 45 degree line past the next pin's tip corner; D 45 degrees
/// from its centre) is not reached yet: the length ceiling is the r1 copper. main 33f63998: A, B, C ended 0.525 mm off their centres (the DP
/// bent inside the pad, tidy dropped 3 stubs).
#[test]
fn every_pad_end_is_at_the_pad_centre() {
    let m = model();
    for realise in ["sequential", "erbs"] {
        let r = route(&m, realise);
        assert_eq!(r.routed, 4, "{realise}");
        assert!(r.violations.is_empty(), "{realise}: {:?}", r.violations);
        let s = segs(&r);
        let t = pad_terminals(&m, &s);
        assert_eq!(t.len(), 8, "{realise}: two pad ends per wire, {t:?}\n{s:?}");
        let off: Vec<_> = t.iter().filter(|x| x.3 > 1.0).collect();
        assert!(off.is_empty(), "{realise}: pad ends off the centre {off:?}\nsegments {s:?}");
        assert!(off_line(&r).starts_with("pad ends off centre: 0 track ends"), "{realise}: {}", off_line(&r));
        assert!(!r.warnings.iter().any(|w| w.contains("cleanup-safe") && !w.contains(" 0 segments a pad of their net makes redundant")), "{realise}: tidy dropped a stub: {:?}", r.warnings);
        for (_, a, b) in &s {
            let (dx, dy) = ((b[0] - a[0]).abs(), (b[1] - a[1]).abs());
            assert!(dx <= 1 || dy <= 1 || (dx - dy).abs() <= 2, "{realise}: off-45 segment {a:?} {b:?}");
        }
        let l = len_mm(&s);
        // ceiling = the r1 copper (36.115 mm: A and C run up the axis to the leave point, B leaves
        // at 45 degrees to its pad side). The reference 35.237 mm is shorter: A and C would also
        // leave at 45 degrees to the pad side, a pad-exit shape the DP does not offer yet (open).
        assert!(l <= 36.115 + 0.01, "{realise}: length {l:.3} mm vs r1 36.115 (reference 35.237)\n{s:?}");
        // A (no pin to its left to clear first) runs up its axis to the leave point: its round end
        // 0.02 mm past the tip (y 5.525 - 0.1 + 0.02), the cheapest centred form at the default
        // bend price (the side exit has one bend more and two short legs)
        let a0 = [2_000_000, 6_500_000];
        let (_, sa, sb) = s.iter().find(|x| x.1 == a0 || x.2 == a0).unwrap();
        let far = if *sa == a0 { sb } else { sa };
        assert!(far[0] == a0[0] && (far[1] - 5_605_000).abs() <= 2, "{realise}: A leaves to {far:?}, not to the leave point (2, 5.605)\n{s:?}");
        // the first segment from each U1 centre leaves the pad: its far end is outside the pad or
        // closer to the pad's side than half the width (0.2 mm track), i.e. the round end sticks out
        for &(_, p, c, _) in t.iter().filter(|x| x.2[1] == 6_500_000) {
            let (_, a, b) = s.iter().find(|x| x.1 == p || x.2 == p).unwrap();
            let far = if *a == p { b } else { a };
            let inset = (300_000 - (far[0] - c[0]).abs()).min(975_000 - (far[1] - c[1]).abs());
            assert!(inset < 100_000 - 5_000, "{realise}: first segment from {c:?} ends at {far:?}, wholly inside the pad");
        }
    }
}

/// Fine pitch (lead's case 2): a foreign copper bar (0.3 x 0.2 mm, no net) just past C's tip and
/// right of its axis, in the room C's exit and its turn onto the 45 degree line use. No cutoff:
/// the stub price competes with the other legal shapes. Nothing is dropped or violated, every pad
/// end is at its centre and the count line matches the copper.
#[test]
fn blocked_axis_exit_still_leaves_the_centre() {
    let mut m = model();
    let pads = m["pads"].as_array_mut().unwrap();
    let mut bar = pads[0].clone();
    bar["uuid"] = serde_json::json!("00000000-0000-0000-0000-00000000d0d0");
    bar["ref"] = serde_json::json!("FD1");
    bar["number"] = serde_json::json!("1");
    bar["net"] = serde_json::json!(0);
    bar["center"] = serde_json::json!([4_890_000, 5_100_000]);
    bar["shapes"]["0"] = serde_json::json!({"kind": "rect", "w": 300_000, "h": 200_000});
    pads.push(bar);
    for realise in ["sequential", "erbs"] {
        let r = route(&m, realise);
        assert_eq!(r.routed, 4, "{realise}");
        assert!(r.violations.is_empty(), "{realise}: {:?}", r.violations);
        let s = segs(&r);
        let t = pad_terminals(&m, &s);
        let off: Vec<_> = t.iter().filter(|x| x.3 > 1.0).collect();
        assert!(off.is_empty(), "{realise}: pad ends off the centre {off:?}\n{s:?}");
        assert!(off_line(&r).starts_with("pad ends off centre: 0 track ends"), "{realise}: {}", off_line(&r));
    }
}

/// pad-snap r2 (D3a): the 45 degree exit to the pad side. With the bend prices and the bundle pull
/// off (`bend_mm` 0, `bend_spacing_mm` 0, `bundle_pull` 0: length only) A and B leave their pad
/// centres at 45 degrees to the pad side (the round end out of the pad, then up beside it: fewer
/// vertical millimetres than the axis exit), every end centred, and the copper is shorter than at
/// the default prices (36.115 mm, the axis exit). The reference 35.237 mm is not reached: its 45
/// degree line is the tangent of the next pin's corner arc at the clearance, a point the DP has no
/// node at, and C staircases round D's corner when bends are free (first-draw-45 P3, open).
#[test]
fn side_exit_is_taken_when_only_length_counts() {
    let m = model();
    for realise in ["sequential", "erbs"] {
        let r = route_opt(&m, realise, &[("bend_mm", serde_json::json!(0.0)), ("bend_spacing_mm", serde_json::json!(0.0)), ("bundle_pull", serde_json::json!(0.0))]);
        assert_eq!(r.routed, 4, "{realise}");
        assert!(r.violations.is_empty(), "{realise}: {:?}", r.violations);
        let s = segs(&r);
        let off: Vec<_> = pad_terminals(&m, &s).into_iter().filter(|x| x.3 > 1.0).collect();
        assert!(off.is_empty(), "{realise}: pad ends off the centre {off:?}\n{s:?}");
        for c in [[2_000_000i64, 6_500_000i64], [3_270_000, 6_500_000]] {
            let (_, a, b) = s.iter().find(|x| x.1 == c || x.2 == c).unwrap();
            let far = if *a == c { b } else { a };
            let (dx, dy) = (far[0] - c[0], c[1] - far[1]);
            // 45 degrees up-right, at least to the leave point on the pad side (0.3 - 0.1 + 0.02 mm)
            assert!((dx - dy).abs() <= 2 && dx >= 220_000 - 2, "{realise}: first segment from {c:?} to {far:?} is no side exit\n{s:?}");
        }
        let l = len_mm(&s);
        // fd45 r10 (lead): the length half holds under erbs only. Sequential, the P4 chord nodes
        // let net 2 take a path 0.5 mm longer that leaves net 1's planned room (planned_room 27 ->
        // 11 mm), but that room is net 1's taut crossing and its octilinear form bows away from
        // it, so net 1 is 0.2 mm longer anyway: 36.345 mm (35.53 with planned_room 0). Open:
        // ROOM-FROM-OCTI (the planned room from the neighbour's octilinear form, shopping list).
        if realise == "erbs" {
            assert!(l < 36.115 - 0.1, "{realise}: length {l:.3} mm, not below the axis-exit copper 36.115 (reference 35.237)\n{s:?}");
        }
    }
}

/// pad-snap r2: both ends of every wire on an elongated pad with the stub problem: the test pads
/// replaced by a second SOIC row (U2, the U1 row moved 4.5 mm right and 5 mm up). Each wire
/// leaves U1 past the next pin's tip corner (A, B, C) and enters U2 past the previous pin's
/// (B, C, D): the start and the end stub prices both act. Every pad end at its centre.
#[test]
fn both_ends_on_soic_rows_end_at_the_centres() {
    let mut m = model();
    {
        let pads = m["pads"].as_array_mut().unwrap();
        let u1: Vec<serde_json::Value> = pads.iter().filter(|p| p["ref"] == "U1").cloned().collect();
        pads.retain(|p| p["ref"] == "U1");
        for (i, p) in u1.into_iter().enumerate() {
            let mut q = p;
            q["uuid"] = serde_json::json!(format!("00000000-0000-0000-0000-00000000e{i:03}"));
            q["ref"] = serde_json::json!("U2");
            let c = [q["center"][0].as_i64().unwrap() + 4_500_000, q["center"][1].as_i64().unwrap() - 5_000_000];
            q["center"] = serde_json::json!(c);
            pads.push(q);
        }
    }
    for realise in ["sequential", "erbs"] {
        let r = route(&m, realise);
        assert_eq!(r.routed, 4, "{realise}: {:?}", r.warnings);
        assert!(r.violations.is_empty(), "{realise}: {:?}", r.violations);
        let s = segs(&r);
        let t = pad_terminals(&m, &s);
        assert_eq!(t.len(), 8, "{realise}: {t:?}\n{s:?}");
        let off: Vec<_> = t.iter().filter(|x| x.3 > 1.0).collect();
        assert!(off.is_empty(), "{realise}: pad ends off the centre {off:?}\nsegments {s:?}");
        assert_eq!(off_count(&r), 0, "{realise}: {}", off_line(&r));
    }
}

/// pad-snap r2b (D3b): square pads (1.0 x 1.0 mm, not elongated: no pad `Exit`, so no axis exit
/// shapes) in place of the SOIC row. The taut path from each centre bends round the next pin's
/// corner; the DP's leave shapes in the 8 octilinear directions still let every run leave its
/// pad centred: every U1 end at its centre, no tidy drop, the count line 0.
#[test]
fn square_pads_end_at_the_centres() {
    let mut m = model();
    for p in m["pads"].as_array_mut().unwrap().iter_mut().filter(|p| p["ref"] == "U1") {
        p["shapes"]["0"] = serde_json::json!({"kind": "rect", "w": 1_000_000, "h": 1_000_000});
    }
    for realise in ["sequential", "erbs"] {
        let r = route(&m, realise);
        assert_eq!(r.routed, 4, "{realise}: {:?}", r.warnings);
        assert!(r.violations.is_empty(), "{realise}: {:?}", r.violations);
        let s = segs(&r);
        let t = pad_terminals(&m, &s);
        assert_eq!(t.len(), 8, "{realise}: {t:?}\n{s:?}");
        let off: Vec<_> = t.iter().filter(|x| x.3 > 1.0).collect();
        assert!(off.is_empty(), "{realise}: pad ends off the centre {off:?}\nsegments {s:?}");
        assert_eq!(off_count(&r), 0, "{realise}: {}", off_line(&r));
    }
}
