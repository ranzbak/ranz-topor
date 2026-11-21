//! first-draw-45 (docs/dev/design/first-draw-45.md): the first draw's octilinear form where a 45
//! degree form fits. P3 `[drawing] octi_order = "sweep"` (the default since r9; "bends" is the other order): the wires get their octilinear form in
//! Staepelaere's sweep order (1992, OrderSegments, PDF pp. 22, 29), so a bundle is drawn from one
//! side to the other and each wire's DP has its neighbours' room on one side only.
//!
//! Fixtures: the user-accepted drc patterns K5 jst_fanout_45_close and U6 pinrow_long_bundle
//! (model JSON of their problem boards in `tests/fixtures/drcpat`), one run, one round
//! (deterministic).

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::RouteResult;

fn route(p: &str, extra: &[(&str, serde_json::Value)]) -> RouteResult {
    let path = format!("{}/tests/fixtures/drcpat/model-{p}.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["topo_rounds"] = serde_json::json!(1);
    for (k, v) in extra {
        val[*k] = v.clone();
    }
    let o: Options = serde_json::from_value(val).unwrap();
    topor_engine::route::route_with(&m, &o, &RunControl::new())
}

/// Track segments off the 0/45/90 degree grid (the harness judge's rule: both spans over 1 um and
/// unequal by more than 1 % of the larger plus 1 um), as (net, length mm).
fn free_angle(r: &RouteResult) -> Vec<(i32, f64)> {
    r.items
        .iter()
        .filter_map(|i| if let Item::Track(t) = i { Some(t) } else { None })
        .filter_map(|t| {
            let (a, b) = (t.prim.start(), t.prim.end());
            let (dx, dy) = (((b.x - a.x) as f64).abs(), ((b.y - a.y) as f64).abs());
            (dx > 1e3 && dy > 1e3 && (dx - dy).abs() > 0.01 * dx.max(dy) + 1e3).then(|| (t.net, dx.hypot(dy) / 1e6))
        })
        .collect()
}

/// The tracks as (net, layer, start, end) in mm (failure messages).
fn tracks(r: &RouteResult) -> Vec<(i32, u32, [f64; 4])> {
    r.items
        .iter()
        .filter_map(|i| if let Item::Track(t) = i { Some(t) } else { None })
        .map(|t| (t.net, t.layer, [t.prim.start().x as f64 / 1e6, t.prim.start().y as f64 / 1e6, t.prim.end().x as f64 / 1e6, t.prim.end().y as f64 / 1e6]))
        .collect()
}

fn clean(r: &RouteResult) {
    let viol: usize = r.connections.iter().map(|c| c.violations).sum();
    assert_eq!(r.routed, r.connections_total, "{:?}", r.warnings);
    assert_eq!(viol, 0, "{:?}", r.warnings);
}

/// K5: six wires beside a kept 3V3 track. Base 5571d1bd drew D1 (6.5 mm) and D2 (5.6 mm) free-angle:
/// the draw order started in the middle of the bundle (the bend relation relates no straight
/// chord), and every 45 degree form of D1 crossed the reserved bands of its undrawn neighbours.
/// In the sweep order the bundle is drawn from one side: 0 free-angle, clean. The bend order
/// (`octi_order = "bends"`) kept free-angle legs before P4; with P4's chord nodes it is clean too.
#[test]
fn k5_sweep_order_draws_the_bundle_octilinear() {
    let r = route("jst_fanout_45_close", &[("octi_order", serde_json::json!("sweep"))]);
    clean(&r);
    assert!(free_angle(&r).is_empty(), "free-angle {:?}; warnings {:?}; items {:?}", free_angle(&r), r.warnings, tracks(&r));
    // The bends order: before P4 it kept free-angle legs here; with the chord nodes of P4 the
    // DP finds the 45 degree form in that order too.
    let b = route("jst_fanout_45_close", &[("octi_order", serde_json::json!("bends"))]);
    clean(&b);
    assert!(free_angle(&b).is_empty(), "bends order: free-angle {:?}", free_angle(&b));
}

/// The wires as chained polylines per net (mm, KiCad frame), each from its first to its last point.
fn polylines(r: &RouteResult) -> Vec<(i32, Vec<[f64; 2]>)> {
    let mut by: std::collections::BTreeMap<i32, Vec<[[f64; 2]; 2]>> = std::collections::BTreeMap::new();
    for (net, _, [x0, y0, x1, y1]) in tracks(r) {
        by.entry(net).or_default().push([[x0, y0], [x1, y1]]);
    }
    let same = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4;
    by.into_iter()
        .map(|(net, mut segs)| {
            // start at an end no other segment shares
            let deg = |p: [f64; 2], s: &[[[f64; 2]; 2]]| s.iter().flat_map(|x| x.iter()).filter(|&&q| same(p, q)).count();
            let i0 = (0..segs.len()).find(|&i| deg(segs[i][0], &segs) == 1).or_else(|| (0..segs.len()).find(|&i| deg(segs[i][1], &segs) == 1).inspect(|&i| segs[i].reverse())).unwrap_or(0);
            let first = segs.remove(i0);
            let mut pts = vec![first[0], first[1]];
            while let Some(i) = (0..segs.len()).find(|&i| same(segs[i][0], *pts.last().unwrap()) || same(segs[i][1], *pts.last().unwrap())) {
                let s = segs.remove(i);
                pts.push(if same(s[0], *pts.last().unwrap()) { s[1] } else { s[0] });
            }
            assert!(segs.is_empty(), "net {net}: the tracks are not one chain");
            (net, pts)
        })
        .collect()
}

/// Turns over half a degree along a polyline.
fn bends(pts: &[[f64; 2]]) -> usize {
    let d: Vec<[f64; 2]> = pts.windows(2).map(|p| [p[1][0] - p[0][0], p[1][1] - p[0][1]]).filter(|d| d[0].hypot(d[1]) > 1e-4).collect();
    d.windows(2)
        .filter(|x| {
            let (a, b) = (x[0], x[1]);
            let c = (a[0] * b[0] + a[1] * b[1]) / (a[0].hypot(a[1]) * b[0].hypot(b[1]));
            c.clamp(-1.0, 1.0).acos().to_degrees() > 0.5
        })
        .count()
}

/// U6: seven wires from a 0.65 mm pin row to a DB9 (base 5571d1bd: 134.9 mm free-angle copper).
/// first-draw-45 P4 (S-route chord nodes, Staepelaere 1992 pp. 18-19, then the straightening
/// pass, ch. 5 p. 15): the user-approved bundle (reports/first-draw-45-r2-u6-p4.png, panel 3),
/// judged as the lead set it (2026-11-21): 0 free-angle, clean, every wire at its octilinear
/// length (pad to pad, within 1 mm), the neighbours in the bundle parallel on their long stretch
/// at no more than the reference's 0.5 mm level spacing (no crossing: clean). Not asserted: where
/// the bends land (the depth of the long stretch, a tie on length; the straightening breaks it
/// towards the drawn neighbour, `bundle_pull`, approved by the user 2026-11-21). The pad exit
/// leg is octilinear (PAD-EXIT-AXIS closed: 45 degrees from the pad is fine, free-angle is not).
#[test]
fn u6_pinrow_long_bundle_is_one_octilinear_bundle() {
    // The bends order also draws U6 without free-angle copper, but S5 leaves the bundle there
    // (its level 2 mm below S7's): the one-bundle shape needs the sweep order, the default since
    // first-draw-45 r9 (so the run below sets nothing: a default back at "bends" fails it).
    let b = route("pinrow_long_bundle", &[("octi_order", serde_json::json!("bends"))]);
    clean(&b);
    assert!(free_angle(&b).is_empty(), "bends order: free-angle {:?}", free_angle(&b));
    let r = route("pinrow_long_bundle", &[]);
    // Debug: the tracks as JSON (net, layer, start, end in mm, KiCad frame) for the PNG.
    if let Ok(path) = std::env::var("FD45_DUMP") {
        let segs: Vec<serde_json::Value> = r
            .items
            .iter()
            .filter_map(|i| if let Item::Track(t) = i { Some(t) } else { None })
            .map(|t| serde_json::json!([t.net, t.layer, t.prim.start().x as f64 / 1e6, t.prim.start().y as f64 / 1e6, t.prim.end().x as f64 / 1e6, t.prim.end().y as f64 / 1e6]))
            .collect();
        std::fs::write(path, serde_json::Value::Array(segs).to_string()).unwrap();
    }
    let fa = free_angle(&r);
    let mm: f64 = fa.iter().map(|x| x.1).sum();
    eprintln!("U6 free-angle {} segments {mm:.3} mm: {fa:?}", fa.len());
    let lines: std::collections::BTreeMap<i32, Vec<[f64; 2]>> = polylines(&r).into_iter().collect();
    let plen = |p: &[[f64; 2]]| p.windows(2).map(|x| (x[1][0] - x[0][0]).hypot(x[1][1] - x[0][1])).sum::<f64>();
    for (net, pts) in &lines {
        eprintln!("U6 net {net}: {} bends, length {:.3} mm: {pts:?}", bends(pts), plen(pts));
    }
    clean(&r);
    assert!(fa.is_empty(), "U6 free-angle {mm:.3} mm: {fa:?}");
    // Bundle order at the pin row: S1 S6 S2 S3 S4 S7 S5 (nets 1 6 2 3 4 7 5).
    let order = [1, 6, 2, 3, 4, 7, 5];
    // The pin-row end first (the end with the larger x), and the longest horizontal segment (the
    // reference's level).
    let row_first = |p: &Vec<[f64; 2]>| if p[0][0] >= p[p.len() - 1][0] { p.clone() } else { p.iter().rev().copied().collect() };
    let level = |p: &[[f64; 2]]| p.windows(2).filter(|x| (x[1][1] - x[0][1]).abs() < 1e-4).map(|x| [x[0], x[1]]).max_by(|a, b| (a[1][0] - a[0][0]).abs().total_cmp(&(b[1][0] - b[0][0]).abs()));
    // The pattern's reference tracks (harness drc_patterns.pinrow_long_bundle, the user-approved
    // panel 3 of the P4 PNG): from the pin-row pad centre down, a horizontal level, 45 degrees to
    // the target pad (the second-row pads S6/S7 by a vertical leg between the first-row pads). The
    // level y is a free choice of the reference (a tie, not asserted); its length is the bound.
    let ref_len = |net: i32| -> f64 {
        let r4 = |x: f64| (x * 1e4).round() / 1e4;
        let (jx, jy) = (5.0, 15.0);
        let i = order.iter().position(|&n| n == net).unwrap();
        let col = [0, 2, 4, 6, 8, 1, 7][net as usize - 1] as f64;
        let (tx, ty) = (r4(jx + 1.385 * col), if net >= 6 { r4(jy + 2.84) } else { jy });
        let (uy, ux) = (r4(jy - 9.706), r4(r4(jx + 25.093) + 0.65 * i as f64 + if i > 0 { 0.01 } else { 0.0 }));
        let (yd, yh) = (r4(uy + 1.01 + 0.2), r4(jy - 6.0 + 0.5 * i as f64));
        let mut p = vec![[ux, uy], [ux, yd], [r4(ux + yd - yh), yh]];
        if ty > jy {
            p.extend([[r4(tx + jy - 0.6 - yh), yh], [tx, r4(jy - 0.6)], [tx, ty]]);
        } else {
            p.extend([[r4(tx + ty - yh), yh], [tx, ty]]);
        }
        plen(&p)
    };
    for (net, pts) in &lines {
        let p = row_first(pts);
        assert!(plen(&p) <= ref_len(*net) + 0.25, "net {net}: length {:.3} mm, reference {:.3} mm: {p:?}", plen(&p), ref_len(*net));
        // The pad exit (PAD-EXIT-AXIS, closed by the user 2026-11-21: "it should adhere to the
        // 45°"): no straight-first rule; the exit leg out of the pin-row pad is octilinear (0, 45
        // or 90 degrees from the pad's axis, here the board's y axis), never free-angle.
        let (e0, e1) = (p[0], p[1]);
        let (dx, dy) = ((e1[0] - e0[0]).abs(), (e1[1] - e0[1]).abs());
        assert!(dx.hypot(dy) > 1e-3, "net {net}: zero exit leg: {p:?}");
        assert!(dx < 1e-3 || dy < 1e-3 || (dx - dy).abs() < 1e-3, "net {net}: exit leg {e0:?} -> {e1:?} is not 0/45/90 degrees from the pad: {p:?}");
    }
    // One bundle: every wire has a level of at least 4 mm, neighbours in the pin-row order at most
    // 0.5 mm apart (the reference's level spacing), each one below the one before (no crossing).
    for x in order.windows(2) {
        let (la, lb) = (level(&lines[&x[0]]), level(&lines[&x[1]]));
        let (Some(la), Some(lb)) = (la, lb) else { panic!("nets {} and {}: no horizontal level: {la:?} {lb:?}", x[0], x[1]) };
        let dy = lb[0][1] - la[0][1];
        assert!(
            (la[1][0] - la[0][0]).abs() >= 4.0 && (lb[1][0] - lb[0][0]).abs() >= 4.0 && dy > 0.0 && dy <= 0.5 + 1e-3,
            "nets {} and {}: levels {la:?} and {lb:?}: {dy:.3} mm apart",
            x[0],
            x[1]
        );
    }
}

/// An unknown value falls back to the default with a warning.
#[test]
fn unknown_octi_order_warns() {
    let r = route("jst_fanout_45_close", &[("octi_order", serde_json::json!("zigzag"))]);
    assert!(r.warnings.iter().any(|w| w.contains("octi_order = \"zigzag\"")), "{:?}", r.warnings);
}

/// first-draw-45 r7 (kills the r6 survivor STRAIGHTEN_PASSES = 1): the straightening's passes
/// over the draw order run until one changes nothing. On the qmtech USB corner the 2nd pass still
/// straightens (a wire straightened before its neighbours moved gets another turn) and the 3rd
/// changes nothing: the line reports the passes and the fixpoint. A bound of 1 or 2 passes stops
/// with changes in the last pass. Also the line's two forms (AGENT_RULES 13).
#[test]
fn straightening_reaches_its_fixpoint() {
    use topor_engine::topo::core::straighten_line;
    let path = format!("{}/tests/fixtures/qmtech-usb-crop.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["topo_rounds"] = serde_json::json!(1);
    let o: Options = serde_json::from_value(val).unwrap();
    let r = topor_engine::route::route_with(&m, &o, &RunControl::new());
    let line = r.warnings.iter().find(|w| w.starts_with("octilinear straightening after drawing")).unwrap_or_else(|| panic!("no straightening line: {:?}", r.warnings));
    let passes: usize = line.split("draw order, ").nth(1).and_then(|x| x.split(' ').next()).and_then(|x| x.parse().ok()).expect("passes");
    assert!(passes >= 3 && line.contains(", the last without a change)"), "{line}");
    assert!(straighten_line(3, 0.54, 0, 3, true, 0.1).starts_with("octilinear straightening after drawing (draw order, 3 passes, the last without a change): 3 stretches"));
    let cut = straighten_line(9, -1.0, 2, 4, false, 0.1);
    assert!(cut.contains("(draw order, 4 passes, stopped at the bound of 4 passes with changes in the last): 9 stretches") && cut.contains("1.0 mm saved in length; 2 refused by the split guard"), "{cut}");
}

/// first-draw-45 r7 (`[drawing] bundle_pull_range_mm`, user 2026-11-21: the depth tie broken by
/// the bundle hug, with a configurable pull range): U6 in the sweep order. With no range (the
/// default, `u6_pinrow_long_bundle_is_one_octilinear_bundle`) S7 and S5 move about 2.5 mm up to
/// run beside S4 (one bundle). With a 0.5 mm range that move is out of reach: S7's level stays
/// where its own shape puts it, more than 1 mm below S4's (clean, octilinear). With a 3 mm range
/// the hug reaches again: one bundle, neighbours at most 0.5 mm apart.
#[test]
fn bundle_pull_range_limits_the_hug() {
    let order = [1, 6, 2, 3, 4, 7, 5];
    let level = |p: &[[f64; 2]]| p.windows(2).filter(|x| (x[1][1] - x[0][1]).abs() < 1e-4).map(|x| [x[0], x[1]]).max_by(|a, b| (a[1][0] - a[0][0]).abs().total_cmp(&(b[1][0] - b[0][0]).abs()));
    let gaps = |rg: f64| -> Vec<f64> {
        let r = route("pinrow_long_bundle", &[("octi_order", serde_json::json!("sweep")), ("bundle_pull_range_mm", serde_json::json!(rg))]);
        clean(&r);
        assert!(free_angle(&r).is_empty(), "range {rg}: free-angle {:?}", free_angle(&r));
        let lines: std::collections::BTreeMap<i32, Vec<[f64; 2]>> = polylines(&r).into_iter().collect();
        let ys: Vec<f64> = order.iter().map(|n| level(&lines[n]).map(|l| l[0][1]).unwrap_or(f64::NAN)).collect();
        eprintln!("range {rg}: levels {ys:?}");
        ys.windows(2).map(|y| y[1] - y[0]).collect()
    };
    let near = gaps(0.5);
    assert!(near[4] > 1.0, "range 0.5 mm: S7 pulled up to S4 anyway: gaps {near:?}");
    let far = gaps(3.0);
    assert!(far.iter().all(|&g| g > 0.0 && g <= 0.5 + 1e-3), "range 3 mm: not one bundle: gaps {far:?}");
}
