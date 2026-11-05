//! S2a: `route()` end to end on small two-layer boards: copper that the checker accepts, no
//! same-layer crossing of different nets, vias where the layers force them, reasons for what is
//! not routed.

use topor_engine::check::Checker;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::route_with;

const MM: f64 = 1e6;

/// Two-layer board, `outline` (x0, y0, x1, y1) mm, pads (net, x, y, th, size) mm, KiCad frame.
fn board(outline: (f64, f64, f64, f64), pads: &[(i32, f64, f64, bool, f64)], per_net: &str) -> BoardModel {
    let p = |v: f64| (v * MM) as i64;
    let (x0, y0, x1, y1) = outline;
    let c = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y, th, d))| {
            let (attr, layers, drill) = if th {
                ("pth", "[0, 2]", format!(r#"{{"w":{0},"h":{0}}}"#, p(d / 2.0)))
            } else {
                ("smd", "[0]", "null".to_string())
            };
            let sh = format!(r#"{{"kind":"rect","w":{0},"h":{0}}}"#, p(d));
            format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"{attr}",
                    "center":[{},{}],"rotation_deg":0.0,"layers":{layers},
                    "shapes":{{"0":{sh},"2":{sh}}},"drill":{drill}}}"#,
                p(x),
                p(y)
            )
        })
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}}],
            "pads":[{}],"outline":[{}],"rules":{{"per_net":{{{per_net}}}}}}}"#,
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}


/// The test boards put pads against the outline, so the copper-to-edge rule is off.
fn run(m: &BoardModel) -> topor_engine::route::RouteResult {
    route_with(m, &Options::default(), &RunControl::new())
}

fn no_edge_rule(mut m: BoardModel) -> BoardModel {
    m.rules.edge_clearance = 0;
    m
}

fn tracks(r: &topor_engine::route::RouteResult) -> Vec<&topor_engine::model::TrackItem> {
    r.items
        .iter()
        .filter_map(|i| match i {
            Item::Track(t) => Some(t),
            Item::Via(_) => None,
        })
        .collect()
}

#[test]
fn a_single_connection_is_routed_and_checker_clean() {
    let m = board((0.0, 0.0, 10.0, 10.0), &[(1, 1.0, 5.0, false, 0.8), (1, 9.0, 5.0, false, 0.8)], "");
    let r = run(&m);
    assert_eq!((r.connections_total, r.routed, r.failed), (1, 1, 0), "{:?}", r.connections[0].reason);
    assert_eq!(r.vias, 0);
    let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
    assert!(r.connections[0].length_nm as f64 >= 8.0 * MM * 0.99);
}

#[test]
fn a_crossing_of_smd_pads_is_layered_with_vias_and_nothing_crosses_on_one_layer() {
    let m = no_edge_rule(board(
        (0.0, 0.0, 10.0, 10.0),
        &[(1, 0.5, 5.0, false, 0.8), (1, 9.5, 5.0, false, 0.8), (2, 5.0, 0.5, false, 0.8), (2, 5.0, 9.5, false, 0.8)],
        "",
    ));
    let r = run(&m);
    assert_eq!(r.routed, 2, "{:?}", r.connections.iter().map(|c| &c.reason).collect::<Vec<_>>());
    assert_eq!(r.vias, 2, "all pads on F: one wire dives under the other and returns");
    let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
    // No two tracks of different nets intersect on the same layer.
    let t = tracks(&r);
    for (i, a) in t.iter().enumerate() {
        for b in &t[i + 1..] {
            if a.net == b.net || a.layer != b.layer {
                continue;
            }
            let (topor_engine::geom::quant::TrackPrim::Seg { a: a1, b: a2 }, topor_engine::geom::quant::TrackPrim::Seg { a: b1, b: b2 }) = (a.prim, b.prim) else { continue };
            let (p, q, u, v) = (a1.to_f64(), a2.to_f64(), b1.to_f64(), b2.to_f64());
            let (d1, d2) = (q - p, v - u);
            let den = d1.cross(d2);
            if den.abs() > 1e-9 {
                let s = (u - p).cross(d2) / den;
                let w = (u - p).cross(d1) / den;
                assert!(!((0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&w)), "same-layer crossing of nets {} and {}", a.net, b.net);
            }
        }
    }
}

#[test]
fn through_hole_ends_cross_without_vias() {
    let m = no_edge_rule(board(
        (0.0, 0.0, 10.0, 10.0),
        &[(1, 0.5, 5.0, true, 0.8), (1, 9.5, 5.0, true, 0.8), (2, 5.0, 0.5, true, 0.8), (2, 5.0, 9.5, true, 0.8)],
        "",
    ));
    let r = run(&m);
    assert_eq!((r.routed, r.vias), (2, 0), "{:?}", r.connections.iter().map(|c| &c.reason).collect::<Vec<_>>());
}

#[test]
fn a_board_with_one_signal_layer_reports_every_connection_failed_with_a_reason() {
    let mut m = board((0.0, 0.0, 10.0, 10.0), &[(1, 1.0, 5.0, false, 0.8), (1, 9.0, 5.0, false, 0.8)], "");
    m.layers.truncate(1);
    let r = run(&m);
    assert_eq!((r.routed, r.failed), (0, 1));
    assert!(r.connections[0].reason.contains("two or more signal layers"), "{}", r.connections[0].reason);
}
