//! S4: the via reduction after drawing never loses a connection, never breaks a rule (the checker
//! sees the result) and never adds a via.

use topor_engine::check::Checker;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::route_with;

const MM: f64 = 1e6;

/// Board with `layers` signal layers (ids 0, 1, .., n-2 inner, last = 2 as the bottom layer id of
/// KiCad's frame), through-hole pads `(net, x, y)` mm of 1.6 mm, outline 10 x 10 mm.
fn board(layers: usize, pads: &[(i32, f64, f64)]) -> BoardModel {
    let p = |v: f64| (v * MM) as i64;
    let ids: Vec<u32> = (0..layers as u32).map(|i| if i + 1 == layers as u32 { 2 } else if i == 0 { 0 } else { 2 + i * 2 }).collect();
    let c = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let lj: Vec<String> = ids.iter().map(|id| format!(r#"{{"id":{id},"name":"L{id}","kind":"signal"}}"#)).collect();
    let idl = format!("[{}]", ids.iter().map(u32::to_string).collect::<Vec<_>>().join(","));
    let shapes = format!(
        "{{{}}}",
        ids.iter().map(|id| format!(r#""{id}":{{"kind":"circle","d":{}}}"#, p(1.4))).collect::<Vec<_>>().join(",")
    );
    let pj: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y))| {
            format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"pth","center":[{cx},{cy}],
                    "rotation_deg":0.0,"layers":{idl},"shapes":{shapes},"drill":{{"w":{dr},"h":{dr}}}}}"#,
                cx = p(x),
                cy = p(y),
                dr = p(0.7)
            )
        })
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"t","layers":[{}],
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}},{{"code":3,"name":"C"}}],
            "pads":[{}],"outline":[{}],"rules":{{}}}}"#,
        lj.join(","),
        pj.join(","),
        segs.join(",")
    );
    let mut m = BoardModel::from_json(&s).unwrap();
    m.rules.edge_clearance = 0;
    m
}

fn triple() -> Vec<(i32, f64, f64)> {
    vec![(1, 1.0, 4.5), (1, 9.0, 4.5), (2, 2.0, 1.0), (2, 8.0, 9.0), (3, 8.0, 1.0), (3, 2.0, 9.0)]
}

#[test]
fn via_reduction_keeps_the_routes_clean_and_never_adds_a_via() {
    let m = board(2, &triple());
    let run = |reduce: bool| route_with(&m, &Options { via_reduce: reduce, ..Options::default() }, &RunControl::new());
    let (off, on) = (run(false), run(true));
    assert_eq!((on.routed, on.failed), (off.routed, off.failed));
    assert!(on.vias <= off.vias, "{} vias with the reduction, {} without", on.vias, off.vias);
    let n_via = on.items.iter().filter(|i| matches!(i, Item::Via(_))).count();
    assert_eq!(n_via, on.vias, "the report counts the emitted vias");
    let rep = Checker::new(&m, &Options::default()).check_items(&on.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}
