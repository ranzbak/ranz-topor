//! Board-rules finding 5 (verifier 2026-11-13): the via relocation ([`topor_engine::topo::viaopt`],
//! `[vias] relocate`) keeps a via out of a keepout that forbids vias (one room count with the
//! plan's site test, `Packer::via_free`), so the drawing takes the relocated site as planned.

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::route_with;

const MM: f64 = 1e6;

/// 20 x 10 mm two-layer board: net 1 from an F.Cu pad at (2, 5) to a B.Cu pad at (18, 5) (one via
/// forced by the pad layers), a keepout that forbids vias (tracks allowed) over x 6 .. 14.
fn board() -> BoardModel {
    let p = |v: f64| (v * MM).round() as i64;
    let c = [(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads = [(2.0, 5.0, 0), (18.0, 5.0, 2)];
    let pj: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(x, y, l))| {
            format!(
                r#"{{"uuid":"p{i}","ref":"J{}","number":"1","net":1,"attr":"smd","center":[{},{}],
                    "rotation_deg":0.0,"layers":[{l}],"shapes":{{"{l}":{{"kind":"rect","w":{},"h":{}}}}},"drill":null}}"#,
                i + 1,
                p(x),
                p(y),
                p(1.0),
                p(1.0)
            )
        })
        .collect();
    let ko = format!(r#"{{"name":"novia","layers":[0,2],"poly":[[{},{}],[{},{}],[{},{}],[{},{}]],"no_tracks":false,"no_vias":true}}"#, p(6.0), p(0.0), p(14.0), p(0.0), p(14.0), p(10.0), p(6.0), p(10.0));
    let s = format!(
        r#"{{"units":"nm","board":"novia",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"SIG"}}],
            "pads":[{}],"keepouts":[{}],"outline":[{}],
            "rules":{{"min_clearance":200000,"min_track_width":250000,"min_via_diameter":400000,"min_via_drill":300000,
                     "min_annular":50000,"hole_clearance":250000,"hole_to_hole":250000,"edge_clearance":500000,
                     "default":{{"clearance":200000,"width":250000,"via_d":600000,"via_drill":300000}},
                     "per_net":{{"1":{{"clearance":200000,"width":250000,"via_d":600000,"via_drill":300000,"netclass":"Default","width_source":"netclass"}}}}}}}}"#,
        pj.join(","),
        ko,
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

#[test]
fn relocation_keeps_a_via_out_of_a_no_via_keepout() {
    let m = board();
    let r = route_with(&m, &Options { runs: 1, ..Options::default() }, &RunControl::new());
    assert_eq!(r.failed, 0, "{:?}", r.warnings);
    for w in r.warnings.iter().filter(|w| w.starts_with("via relocation")) {
        eprintln!("{w}");
    }
    let vias: Vec<(f64, f64)> = r.items.iter().filter_map(|it| if let Item::Via(v) = it { Some((v.at.x as f64 / MM, v.at.y as f64 / MM)) } else { None }).collect();
    assert_eq!(vias.len(), 1, "{vias:?}");
    // the via's disc (0.3 mm) stays out of x 6 .. 14
    assert!(vias[0].0 <= 6.0 - 0.3 + 1e-3 || vias[0].0 >= 14.0 + 0.3 - 1e-3, "via outside the keepout: {vias:?}");
    // decided in the plan, realised by the drawing: the relocated site is free, the drawing takes it
    let rel = r.warnings.iter().find(|w| w.starts_with("via relocation in the drawing")).expect("relocation line");
    assert!(rel.contains("1 vias at their planned site, 0 planned sites not free"), "{rel}");
    let rep = topor_engine::check::Checker::new(&m, &Options::default()).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}
