use std::collections::HashSet;
use topor_engine::cdt::Crossing;
use topor_engine::check::Checker;
use topor_engine::geom::quant::TrackPrim;
use topor_engine::geom::Vec2;
use topor_engine::model::{BoardModel, ExistingCopper, Item, Options};
use topor_engine::obstacle::ObstacleSet;
use topor_engine::route::{route, wire_stats, RouteResult};
use topor_engine::sketch::{
    build_layer_cdt, sketch_from_crossings, unanchored_bends, ConstraintSet,
};

const MM: f64 = 1_000_000.0;

fn outline() -> String {
    let seg = |a: (i64, i64), b: (i64, i64)| {
        format!(
            r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
            a.0 * 1_000_000,
            a.1 * 1_000_000,
            b.0 * 1_000_000,
            b.1 * 1_000_000
        )
    };
    [
        ((0, 0), (50, 0)),
        ((50, 0), (50, -40)),
        ((50, -40), (0, -40)),
        ((0, -40), (0, 0)),
    ]
    .iter()
    .map(|&(a, b)| seg(a, b))
    .collect::<Vec<_>>()
    .join(",")
}

/// `(ref, number, net, x_mm, y_mm [KiCad frame, y up positive -> board y = -y], d_mm, layers)`.
struct P(&'static str, &'static str, i32, f64, f64, f64, &'static str);

fn pad(p: &P) -> String {
    format!(
        r#"{{ "uuid": "{r}{n}", "ref": "{r}", "number": "{n}", "net": {net},
          "center": [{x}, {y}], "layers": {l},
          "shapes": {{ "0": {{ "kind": "circle", "d": {d} }}, "31": {{ "kind": "circle", "d": {d} }} }} }}"#,
        r = p.0,
        n = p.1,
        net = p.2,
        x = (p.3 * MM) as i64,
        y = (-p.4 * MM) as i64,
        d = (p.5 * MM) as i64,
        l = p.6
    )
}

fn model(pads: &[P], extra: &str) -> BoardModel {
    let nets: Vec<String> = (1..=6)
        .map(|n| format!(r#"{{ "code": {n}, "name": "N{n}" }}"#))
        .collect();
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {} ], "pads": [ {} ], {extra} "outline": [ {} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#,
        nets.join(","),
        pads.iter().map(pad).collect::<Vec<_>>().join(","),
        outline()
    );
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"))
}

fn opts() -> Options {
    Options {
        wall_clock_s: 10.0,
        ..Options::default()
    }
}

fn pat(r: &RouteResult) -> Vec<String> {
    r.connections.iter().map(|c| c.pattern.clone()).collect()
}

fn clean(m: &BoardModel, r: &RouteResult) {
    let o = Options {
        margin_nm: 0,
        ..Options::default()
    };
    let rep = Checker::new(m, &o).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

fn json_no_runtime(r: &RouteResult) -> String {
    let mut v = serde_json::to_value(r).unwrap();
    v.as_object_mut().unwrap().remove("runtime_s");
    serde_json::to_string(&v).unwrap()
}

#[test]
fn route_two_pads_straight() {
    let m = model(
        &[
            P("A", "1", 1, 10.0, 20.0, 1.0, "[0]"),
            P("B", "1", 1, 20.0, 20.0, 1.0, "[0]"),
        ],
        "",
    );
    let r = route(&m, &opts());
    assert_eq!(r.routed, 1, "{:?}", r.connections);
    assert_eq!(r.items.len(), 1);
    assert_eq!(pat(&r), vec!["S"]);
    assert!((r.connections[0].length_nm - 10_000_000).abs() <= 2);
    assert!(!pat(&r)[0].contains("SS"));
    assert_eq!(r.threads_used, 1);
    assert!(!r.timed_out);
    clean(&m, &r);
}

#[test]
fn route_around_foreign_pad_has_arc() {
    let m = model(
        &[
            P("A", "1", 1, 5.0, 20.0, 1.0, "[0]"),
            P("B", "1", 1, 35.0, 20.0, 1.0, "[0]"),
            P("C", "1", 2, 20.0, 20.0, 1.0, "[0]"),
            P("D", "1", 2, 20.0, 30.0, 1.0, "[0]"),
        ],
        "",
    );
    let r = route(&m, &opts());
    assert_eq!(r.routed, 2, "{:?}", r.connections);
    let a = r.connections.iter().find(|c| c.net == 1).unwrap();
    assert!(a.pattern.contains('A'), "{}", a.pattern);
    clean(&m, &r);
}

fn four_nets() -> BoardModel {
    model(
        &[
            P("A", "1", 1, 5.0, 10.0, 1.0, "[0]"),
            P("A", "2", 1, 40.0, 30.0, 1.0, "[0]"),
            P("B", "1", 2, 5.0, 30.0, 1.0, "[0]"),
            P("B", "2", 2, 40.0, 10.0, 1.0, "[0]"),
            P("C", "1", 3, 20.0, 5.0, 1.0, "[0]"),
            P("C", "2", 3, 25.0, 35.0, 1.0, "[0]"),
            P("D", "1", 4, 10.0, 20.0, 1.0, "[0]"),
            P("D", "2", 4, 45.0, 20.0, 1.0, "[0]"),
        ],
        "",
    )
}

#[test]
fn route_four_nets_grid_deterministic() {
    let m = four_nets();
    let (a, b) = (route(&m, &opts()), route(&m, &opts()));
    assert_eq!(a.routed, 4, "{:?}", a.connections);
    assert_eq!(json_no_runtime(&a), json_no_runtime(&b));
    clean(&m, &a);
}

#[test]
fn route_pad_off_layer_fails_cleanly() {
    let m = model(
        &[
            P("A", "1", 1, 5.0, 10.0, 1.0, "[0]"),
            P("A", "2", 1, 40.0, 10.0, 1.0, "[0]"),
            P("B", "1", 2, 5.0, 25.0, 1.0, "[31]"),
            P("B", "2", 2, 40.0, 25.0, 1.0, "[0]"),
            P("C", "1", 3, 5.0, 35.0, 1.0, "[0]"),
            P("C", "2", 3, 40.0, 35.0, 1.0, "[0]"),
        ],
        "",
    );
    let r = route(&m, &opts());
    let b = r.connections.iter().find(|c| c.net == 2).unwrap();
    assert_eq!(b.status, "failed");
    assert_eq!(b.reason, "terminal not on layer F.Cu");
    assert_eq!(r.routed, 2, "{:?}", r.connections);
    assert!(r.warnings.iter().any(|w| w.contains("B.Cu")));
    clean(&m, &r);
}

#[test]
fn route_terminal_inside_foreign_obstacle_fails_once() {
    // Net 2 pad centre lies inside net 1's pad clearance zone (0.5 mm pitch, d 1.0 pads).
    let m = model(
        &[
            P("A", "1", 1, 10.0, 20.0, 1.0, "[0]"),
            P("A", "2", 1, 40.0, 20.0, 1.0, "[0]"),
            P("B", "1", 2, 10.0, 20.5, 1.0, "[0]"),
            P("B", "2", 2, 40.0, 25.0, 1.0, "[0]"),
            P("C", "1", 3, 5.0, 35.0, 1.0, "[0]"),
            P("C", "2", 3, 40.0, 35.0, 1.0, "[0]"),
        ],
        "",
    );
    let r = route(&m, &opts());
    let b = r.connections.iter().find(|c| c.net == 2).unwrap();
    assert_eq!(b.status, "failed");
    assert!(b.reason.starts_with("inside obstacle:"), "{}", b.reason);
    assert!(r.iterations <= 2, "{}", r.iterations);
    assert!(r
        .connections
        .iter()
        .filter(|c| c.net == 3)
        .all(|c| c.status == "routed"));
}

#[test]
fn route_zero_wall_clock_times_out() {
    let m = four_nets();
    let r = route(
        &m,
        &Options {
            wall_clock_s: 0.0,
            ..Options::default()
        },
    );
    assert!(r.timed_out);
    assert!(r.items.is_empty());
    assert_eq!(r.failed, 4);
    assert!(r.connections.iter().all(|c| c.reason == "timeout"));
}

#[test]
fn route_pour_served_net_not_counted() {
    let m = model(
        &[
            P("G", "1", 1, 10.0, 10.0, 1.0, "[0]"),
            P("G", "2", 1, 40.0, 10.0, 1.0, "[0]"),
            P("A", "1", 2, 10.0, 30.0, 1.0, "[0]"),
            P("A", "2", 2, 40.0, 30.0, 1.0, "[0]"),
        ],
        r#""pours": [ { "uuid": "z", "net": 1, "layers": [0], "pads": ["G.1", "G.2"], "vias": [] } ],"#,
    );
    let r = route(&m, &opts());
    let g = r.nets.iter().find(|n| n.net == 1).unwrap();
    assert_eq!(g.status, "pour_served");
    assert_eq!(r.nets_total, 1);
    assert_eq!(r.nets_routed, 1);
    assert_eq!(r.connections_total, 1);
}

#[test]
fn route_scope_pads_only() {
    let m = model(
        &[
            P("U1", "1", 1, 10.0, 10.0, 1.0, "[0]"),
            P("U1", "2", 1, 40.0, 10.0, 1.0, "[0]"),
            P("U2", "1", 2, 10.0, 20.0, 1.0, "[0]"),
            P("U2", "2", 2, 40.0, 20.0, 1.0, "[0]"),
            P("U3", "1", 3, 10.0, 30.0, 1.0, "[0]"),
            P("U3", "2", 3, 40.0, 30.0, 1.0, "[0]"),
        ],
        "",
    );
    let o = Options::from_json(r#"{ "scope": { "pads": ["U1.1", "U1.2"] } }"#).unwrap();
    let r = route(&m, &o);
    assert_eq!(r.connections_total, 1);
    assert_eq!(r.routed, 1);
}

#[test]
fn route_smooth_option_warns() {
    let m = four_nets();
    let o = Options {
        existing_copper: ExistingCopper::Smooth,
        ..opts()
    };
    let r = route(&m, &o);
    assert!(r.warnings[0].starts_with("existing_copper=smooth ignored"));
}

#[test]
fn wire_stats_on_hand_board() {
    let m = model(
        &[
            P("A", "1", 1, 10.0, 20.0, 1.0, "[0]"),
            P("A", "2", 1, 20.0, 20.0, 1.0, "[0]"),
        ],
        r#""kept": [ { "uuid": "k1", "net": 1, "layer": 0, "width": 250000,
            "prim": { "kind": "seg", "a": [10000000, -20000000], "b": [20000000, -20000000] } } ],"#,
    );
    let s = wire_stats(&m);
    assert_eq!(s.wires, 1);
    assert!((s.hand_outside_pads_nm - 9_000_000).abs() <= 2, "{s:?}");
}

/// Ledger A4: with `ConstraintSet::Routing` a split vertex (two same-net tracks crossing) has no
/// anchor; `sketch_from_crossings` drops a funnel bend there, `unanchored_bends` reports it so
/// `route()` fails the connection instead of straightening the path through the wall.
#[test]
fn split_vertex_bend_is_reported_not_silently_dropped() {
    let m = model(
        &[],
        r#""kept": [
          { "uuid": "k1", "net": 5, "layer": 0, "width": 250000,
            "prim": { "kind": "seg", "a": [10000000, -30000000], "b": [30000000, -10000000] } },
          { "uuid": "k2", "net": 5, "layer": 0, "width": 250000,
            "prim": { "kind": "seg", "a": [10000000, -10000000], "b": [30000000, -30000000] } } ],"#,
    );
    let obs = ObstacleSet::build(&m, &Options::default(), &HashSet::new(), &[]);
    let v = |x: f64, y: f64| Vec2 {
        x: x * MM,
        y: y * MM,
    };
    let (start, goal, top) = (v(15.0, 0.0), v(25.0, 0.0), v(20.0, 50.0));
    let ls = build_layer_cdt(obs.layer(0), &[start, goal, top], ConstraintSet::Routing).unwrap();
    let s = (0..ls.cdt.num_vertices())
        .find(|&i| ls.cdt.is_split_vertex(i))
        .expect("the crossing tracks split");
    assert!(ls.point_to_anchor[s].is_none());
    let cr = vec![Crossing {
        left: ls.vertex_at(top).unwrap(),
        right: s,
        t: 0.5,
    }];
    let bends = unanchored_bends(start, goal, &cr, &ls);
    assert_eq!(bends.len(), 1);
    assert!((bends[0] - ls.cdt.vertex_pos(s)).len() < 1.0);
    // The sketch itself keeps no anchor for it (the dropped bend).
    assert!(sketch_from_crossings(0, start, goal, cr, &ls)
        .seq
        .is_empty());
}

/// VCC pads 1 (20,20) / 2 (35,28) walled off by a locked GND track y = 24 mm between two GND pads
/// at the board sides (0.7 mm gap to the outline: a 0.25 mm wire needs 0.95 mm).
fn wall_model() -> BoardModel {
    model(
        &[
            P("A", "1", 1, 20.0, 20.0, 1.0, "[0]"),
            P("A", "2", 1, 35.0, 28.0, 1.0, "[0]"),
            P("W", "1", 3, 1.2, 24.0, 1.0, "[0]"),
            P("W", "2", 3, 48.8, 24.0, 1.0, "[0]"),
        ],
        r#""kept": [ { "uuid": "wall", "net": 3, "layer": 0, "width": 250000, "locked": true,
            "prim": { "kind": "seg", "a": [1200000, -24000000], "b": [48800000, -24000000] } } ],"#,
    )
}

#[test]
fn partial_off_emits_nothing_for_blocked_connection() {
    let m = wall_model();
    let r = route(&m, &opts());
    assert_eq!(r.failed, 1, "{:?}", r.connections);
    assert_eq!(r.partial, 0);
    assert!(r.items.is_empty());
    let p = r.connections[0].blocking.expect("blocking point");
    assert!((p[1].abs() - 24_000_000).abs() <= 1_000_000, "{p:?}");
}

#[test]
fn partial_on_emits_two_checked_stubs() {
    let m = wall_model();
    let o = Options {
        partial_routes: true,
        ..opts()
    };
    let r = route(&m, &o);
    assert_eq!(r.partial, 1, "{:?}", r.connections);
    assert_eq!(r.connections[0].status, "partial");
    assert!(r.items.len() >= 2);
    for it in &r.items {
        let Item::Track(t) = it else {
            panic!("non-track item")
        };
        assert_eq!(t.group.as_deref(), Some("topor-partial"));
        assert_eq!(t.wire, Some(0));
    }
    // Far ends: distance to the wall centreline (y = 24 mm) is the 0.455 mm clearance.
    let ends: Vec<f64> = r
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Track(t) => Some(t),
            _ => None,
        })
        .flat_map(|t| match t.prim {
            TrackPrim::Seg { a, b } => [a, b],
            TrackPrim::Arc { start, end, .. } => [start, end],
        })
        .map(|p| (p.y as f64 + 24.0 * MM).abs())
        .collect();
    let near = ends.iter().cloned().fold(f64::MAX, f64::min);
    assert!((455_000.0..=460_000.0).contains(&near), "{ends:?}");
    assert_eq!(
        ends.iter().filter(|d| **d < 470_000.0).count(),
        2,
        "{ends:?}"
    );
    let c = Options {
        margin_nm: 0,
        ..Options::default()
    };
    let rep = Checker::new(&m, &c).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
    let p = r.connections[0].blocking.expect("blocking point");
    assert!((p[1].abs() - 24_000_000).abs() <= 1_000_000, "{p:?}");
}

/// Fix C: a connection that stays unroutable stops the negotiation after a few unchanged
/// iterations instead of running to the iteration cap.
#[test]
fn route_stuck_connection_stops_early() {
    let m = wall_model();
    let r = route(&m, &opts());
    assert_eq!(r.failed, 1, "{:?}", r.connections);
    assert!(r.iterations <= 5, "iterations {}", r.iterations);
    assert!(!r.converged);
    assert!(
        r.warnings.iter().any(|w| w.starts_with("stuck after")),
        "{:?}",
        r.warnings
    );
}
