//! S2b: the topological core on boards with three signal layers: three mutually crossing wires
//! need no via on three layers (they do on two), and the layers are all used legally.

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

/// Three nets whose straight connections cross each other pairwise (a triangle of crossings).
fn triple() -> Vec<(i32, f64, f64)> {
    vec![(1, 1.0, 4.5), (1, 9.0, 4.5), (2, 2.0, 1.0), (2, 8.0, 9.0), (3, 8.0, 1.0), (3, 2.0, 9.0)]
}

fn run(m: &BoardModel) -> topor_engine::route::RouteResult {
    route_with(m, &Options::default(), &RunControl::new())
}

#[test]
fn three_mutually_crossing_wires_need_no_via_on_three_layers() {
    let m = board(3, &triple());
    let r = run(&m);
    assert_eq!((r.connections_total, r.routed, r.failed), (3, 3, 0), "{:?}", r.connections.iter().map(|c| &c.reason).collect::<Vec<_>>());
    assert_eq!(r.vias, 0);
    let layers: std::collections::BTreeSet<u32> = r
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Track(t) => Some(t.layer),
            Item::Via(_) => None,
        })
        .collect();
    assert_eq!(layers.len(), 3, "each wire on its own layer: {layers:?}");
    let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

#[test]
fn the_same_wires_on_two_layers_need_a_via() {
    let m = board(2, &triple());
    let r = run(&m);
    assert_eq!((r.connections_total, r.routed, r.failed), (3, 3, 0), "{:?}", r.connections.iter().map(|c| &c.reason).collect::<Vec<_>>());
    assert!(r.vias >= 1, "an odd cycle of crossings cannot be two-coloured without a via");
    let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

#[test]
fn a_rule_area_forbidding_tracks_closes_its_layers_in_the_topology() {
    use topor_engine::model::{Keepout, Pt};
    let mut m = board(2, &[(1, 1.0, 5.0), (1, 9.0, 5.0)]);
    let p = |v: f64| (v * MM) as i64;
    // A wall across the straight line with a gap above y = 8.5 mm (KiCad frame, as the model JSON).
    m.keepouts.push(Keepout {
        name: "wall".into(),
        layers: vec![0, 2],
        poly: vec![Pt { x: p(4.5), y: p(-9.0) }, Pt { x: p(5.5), y: p(-9.0) }, Pt { x: p(5.5), y: p(-1.5) }, Pt { x: p(4.5), y: p(-1.5) }],
        no_tracks: true,
        no_vias: true,
    });
    let r = run(&m);
    assert_eq!((r.routed, r.failed), (1, 0), "{:?}", r.connections[0].reason);
    let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

#[test]
fn a_plane_layer_is_a_projection_layer_that_signals_avoid_while_a_via_is_cheaper() {
    let mut m = board(3, &triple());
    m.layers[1].routable = false; // the middle layer is a plane
    let r = run(&m);
    assert_eq!((r.connections_total, r.routed, r.failed), (3, 3, 0), "{:?}", r.connections.iter().map(|c| &c.reason).collect::<Vec<_>>());
    let on_plane = r.items.iter().filter(|i| matches!(i, Item::Track(t) if t.layer == 4)).count();
    assert_eq!(on_plane, 0, "a via costs less than ten times the length of a stretch on a plane");
    assert!(r.warnings.iter().any(|w| w.contains("planes ([planes] signals = costly)")), "{:?}", r.warnings);
    let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

#[test]
fn forbidden_plane_signals_leave_the_plane_out_of_the_projection() {
    let mut m = board(3, &triple());
    m.layers[1].routable = false;
    let o = Options { plane_signals: "forbidden".into(), ..Options::default() };
    let r = route_with(&m, &o, &RunControl::new());
    assert_eq!((r.routed, r.failed), (3, 0));
    assert!(!r.warnings.iter().any(|w| w.starts_with("planes (")));
}
