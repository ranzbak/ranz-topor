//! Shopping list 5.2: the TopoR loop's measure on the approved untangle pattern
//! `planned_crossing_window` (`scratch/untangle-patterns/planned_crossing_window`, the same pads,
//! rule areas and net classes): VCC (POWER 0.5 mm, via 0.8 / 0.4) left to right, SDA (0.25 mm, via
//! 0.6 / 0.3) top to bottom, SMD pads on F.Cu, B.Cu closed by rule areas (no tracks, no vias)
//! except a 2.4 x 3.0 mm window right of the straight crossing point. Expected: 1 crossing in the
//! window, 2 vias, no predicted violation.

use topor_engine::check::Checker;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route_with;

const MM: f64 = 1e6;

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> String {
    let p = |v: f64| (v * MM).round() as i64;
    format!("[[{},{}],[{},{}],[{},{}],[{},{}]]", p(x0), p(y0), p(x1), p(y0), p(x1), p(y1), p(x0), p(y1))
}

fn pcw() -> BoardModel {
    let p = |v: f64| (v * MM).round() as i64;
    let c = [(0.0, 0.0), (20.0, 0.0), (20.0, 14.0), (0.0, 14.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    // (net, x, y): net 1 SDA, net 2 VCC.
    let pads = [(1, 10.0, 1.0), (2, 1.0, 7.0), (1, 10.0, 13.0), (2, 19.0, 7.0)];
    let pj: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y))| {
            format!(
                r#"{{"uuid":"p{i}","ref":"J{}","number":"1","net":{net},"attr":"smd","center":[{},{}],
                    "rotation_deg":0.0,"layers":[0],"shapes":{{"0":{{"kind":"rect","w":{},"h":{}}}}},"drill":null}}"#,
                i + 1,
                p(x),
                p(y),
                p(1.0),
                p(1.0)
            )
        })
        .collect();
    let keep: Vec<String> = [rect(0.0, 0.0, 11.8, 14.0), rect(14.2, 0.0, 20.0, 14.0), rect(11.8, 0.0, 14.2, 5.5), rect(11.8, 8.5, 14.2, 14.0)]
        .iter()
        .enumerate()
        .map(|(i, poly)| format!(r#"{{"name":"ko{i}","layers":[2],"poly":{poly},"no_tracks":true,"no_vias":true}}"#))
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"pcw",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"SDA"}},{{"code":2,"name":"VCC"}}],
            "pads":[{}],"keepouts":[{}],"outline":[{}],
            "rules":{{"min_clearance":200000,"min_track_width":250000,"min_via_diameter":400000,"min_via_drill":300000,
                     "min_annular":50000,"hole_clearance":250000,"hole_to_hole":250000,"edge_clearance":500000,
                     "default":{{"clearance":200000,"width":250000,"via_d":600000,"via_drill":300000}},
                     "per_net":{{"1":{{"clearance":200000,"width":250000,"via_d":600000,"via_drill":300000,"netclass":"Default","width_source":"netclass"}},
                                 "2":{{"clearance":200000,"width":500000,"via_d":800000,"via_drill":400000,"netclass":"POWER","width_source":"netclass"}}}}}}}}"#,
        pj.join(","),
        keep.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn loop_opts() -> Options {
    Options { opt_loop: "topor".into(), search_length: "taut".into(), via_between: true, via_relocate: true, ..Options::default() }
}

/// 5.2: the via of VCC between the closed B.Cu area and its crossing with SDA has no traversal
/// strictly between the two events (`[vias] between_crossings`), but its site lies in the
/// crossing's own triangle on VCC's side of the crossing (Polubasov 2001 p. 4: the layer change
/// lies on the wire piece between the two events). The plan predicts no violation for it, so
/// the loop's archive holds a variant without errors.
///
/// M0: the plan's prediction holds under both capacity models. The drawn copper is checked by
/// the two ignored tests below: per-edge model (M3, taut geometry outside the window) and cut
/// model (the same picked plan drawn with the SDA vias in passes without slack and an F.Cu
/// overlap of SDA with VCC: planned != drawn, M2/M3).
#[test]
fn planned_crossing_window_predicts_no_via_without_a_site() {
    let m = pcw();
    for cap in ["cuts", "edges"] {
        let r = route_with(&m, &Options { capacity_model: cap.into(), ..loop_opts() }, &RunControl::new());
        assert_eq!(r.failed, 0, "{cap}: {:?}", r.warnings);
        let metrics = r.warnings.iter().find(|w| w.starts_with("plan metrics:")).expect("plan metrics");
        assert!(metrics.contains("\"vias_without_site\":0"), "{cap}: {metrics}");
        let lp = r.warnings.iter().find(|w| w.starts_with("TopoR loop (")).expect("loop line");
        assert!(lp.contains("predicted 0 violations"), "{cap}: {lp}");
    }
}

/// The drawn half under the per-edge model (the default since 2026-11-13). Expected to fail until
/// M3 (radii / spokes / octilinear drawing geometry); shopping list PK-TW.
///
/// cm1-room (2026-11-17, main 385e043): not a capacity-model miss. The plan's VCC via sites lie
/// 1.67 mm apart in the window (SDA between needs 1.45: CM1's via-to-via cut fits, so the plan
/// rightly predicts 0); the drawing's via relocation (`[vias] relocate`, "2 moved in their free
/// interval to the most clearance") moves them to (12.28, 7.90) and (13.30, 7.00), 1.36 mm apart,
/// and SDA's drawn F.Cu track runs between them at 0.149-0.186 mm (4 clearance violations).
///
/// Board-rules diagnosis (2026-11-13): the clean drawing before the board-rules merge relied on a
/// 0.4/0.3 via drawn for VCC where the plan used the POWER class 0.8/0.4 disc (a plan/draw
/// disagreement, and outside the power-via rule: net class via by default). With the net class
/// via, SDA's TAUT track runs through the B.Cu keepout left of the window the topology put the
/// crossing in, so the VCC via pair has no legal sites on that geometry and the drawing puts SDA
/// between vias closer than the 1.45 mm it needs. The taut pull not respecting the layer window
/// is M3's drawing geometry.
#[test]
#[ignore = "drawing, not CM1: the plan's VCC via sites are 1.67 mm apart (fits); the drawing's via relocation moves them to 1.36 mm with SDA between (needs 1.45); PK-TW"]
fn planned_crossing_window_draws_clean_with_the_edges_model() {
    let m = pcw();
    let r = route_with(&m, &Options { capacity_model: "edges".into(), ..loop_opts() }, &RunControl::new());
    let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

/// The drawn half of the test above under the cut model (`[capacity] model = "cuts"`, the M0 default until 2026-11-13). Fails at M0: a
/// drawn-side miss parked until the draw-once stages (M2/M3); un-ignore when they land.
///
/// cm1-room (2026-11-17): until then this test ran the default model (edges), not the cut model
/// its name says; it now sets `capacity_model = "cuts"`. Under the cut model the plan predicts 0
/// violations with the CM1 via cuts on (the via sites fit); the drawing finds all 4 planned via
/// sites not free, places both VCC vias by its own site search, and SDA then shorts VCC's F.Cu
/// track at (11.34, 6.38) mm. A drawing miss (planned room not reserved), not the capacity model.
///
/// Board-rules diagnosis (2026-11-13), shopping list item CM1 "Capacity measure completeness: a
/// foreign band through a planned via disc must count": SDA's planned F.Cu band runs through the
/// centre of a planned VCC via; the cut measure has no cut inside a feature, so the plan predicts 0
/// violations and the drawing then finds no room for both vias (1.36 mm apart, 1.45 mm needed).
#[test]
#[ignore = "drawing, not CM1: plan predicts 0 with via cuts; the drawing finds the 4 planned via sites not free, site-searches the VCC vias, and SDA shorts VCC's F.Cu track at (11.34, 6.38); PK-TW"]
fn planned_crossing_window_draws_clean_with_the_cut_model() {
    let m = pcw();
    let r = route_with(&m, &Options { capacity_model: "cuts".into(), ..loop_opts() }, &RunControl::new());
    let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

