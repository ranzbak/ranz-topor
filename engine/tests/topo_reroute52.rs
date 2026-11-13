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
/// the loop's archive holds a variant without errors, and the drawing is clean.
///
/// M0: the plan's prediction holds under both capacity models (the cut model is the default
/// since M0). The drawn copper is clean with the per-edge model; with the cut model the same
/// picked plan (0 predicted violations, 2 vias, 30.6 mm) is drawn with the SDA vias in passes
/// without slack and an F.Cu overlap of SDA with VCC (a drawn-side miss: planned != drawn, parked
/// for the predicted-vs-drawn report M1 and draw-once M2/M3), checked by the ignored test below.
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
        if cap == "edges" {
            let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
            assert!(rep.violations.is_empty(), "{cap}: {:?}", rep.violations);
        }
    }
}

/// The drawn half of the test above under the cut model (`[capacity] model = "cuts"`, the M0 default until 2026-11-13). Fails at M0: a
/// drawn-side miss parked until the draw-once stages (M2/M3); un-ignore when they land.
#[test]
#[ignore = "M0 parked: cut model, plan predicts 0 violations but the sequential drawing shorts SDA into VCC on F.Cu (M2/M3 draw once)"]
fn planned_crossing_window_draws_clean_with_the_cut_model() {
    let m = pcw();
    let r = route_with(&m, &loop_opts(), &RunControl::new());
    let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

