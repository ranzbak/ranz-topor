//! A9: priority tiers, do-not-touch nets, ignored pads and the per-net pour override
//! (`Options.net_priority`, `exclude_nets`, `ignore_pads`, `pour_served`).
use topor_engine::check::Checker;
use topor_engine::decompose::{decompose, order};
use topor_engine::model::{BoardModel, Options, PadRef};
use topor_engine::route::{route, RouteResult};
use topor_engine::search::{tier_weight, via_weight};

const MM: f64 = 1_000_000.0;

/// `(ref, number, net, x_mm, y_mm [KiCad frame y up], layers)`.
struct P(&'static str, &'static str, i32, f64, f64, &'static str);

fn pad(p: &P) -> String {
    format!(
        r#"{{ "uuid": "{r}{n}", "ref": "{r}", "number": "{n}", "net": {net}, "center": [{x}, {y}], "layers": {l},
          "shapes": {{ "0": {{ "kind": "circle", "d": 1000000 }}, "31": {{ "kind": "circle", "d": 1000000 }} }} }}"#,
        r = p.0,
        n = p.1,
        net = p.2,
        x = (p.3 * MM) as i64,
        y = (-p.4 * MM) as i64,
        l = p.5
    )
}

fn model(pads: &[P], extra: &str) -> BoardModel {
    let seg = |a: (i64, i64), b: (i64, i64)| {
        format!(
            r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
            a.0 * 1_000_000,
            a.1 * 1_000_000,
            b.0 * 1_000_000,
            b.1 * 1_000_000
        )
    };
    let outline = [
        ((0, 0), (50, 0)),
        ((50, 0), (50, -40)),
        ((50, -40), (0, -40)),
        ((0, -40), (0, 0)),
    ]
    .iter()
    .map(|&(a, b)| seg(a, b))
    .collect::<Vec<_>>()
    .join(",");
    let nets: Vec<String> = (1..=4)
        .map(|n| format!(r#"{{ "code": {n}, "name": "N{n}" }}"#))
        .collect();
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {} ], "pads": [ {} ], {extra} "outline": [ {outline} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#,
        nets.join(","),
        pads.iter().map(pad).collect::<Vec<_>>().join(",")
    );
    let mut m = BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"));
    for l in &mut m.layers {
        l.routable = l.id == 0;
    }
    m
}

fn opts() -> Options {
    Options {
        wall_clock_s: 600.0,
        threads: 1,
        ..Options::default()
    }
}

fn clean(m: &BoardModel, r: &RouteResult) {
    let o = Options {
        margin_nm: 0,
        ..Options::default()
    };
    let rep = Checker::new(m, &o).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

fn two_nets() -> BoardModel {
    // N2 is the shorter connection: by default it is routed first.
    model(
        &[
            P("A", "1", 1, 10.0, 10.0, "[0]"),
            P("A", "2", 1, 40.0, 10.0, "[0]"),
            P("B", "1", 2, 10.0, 30.0, "[0]"),
            P("B", "2", 2, 20.0, 30.0, "[0]"),
        ],
        "",
    )
}

#[test]
fn priority_table_orders_and_weights() {
    let m = two_nets();
    let d = decompose(&m, &Options::default(), &[0]);
    assert_eq!(d.connections.iter().map(|c| c.tier).collect::<Vec<_>>(), [3, 3]);
    let first = |d: &topor_engine::decompose::Decomposition| d.connections[order(&d.connections, &[])[0]].net;
    assert_eq!(first(&d), 2, "shorter first without a table");

    let mut o = Options::default();
    o.net_priority.insert("N1".into(), 1);
    o.net_priority.insert("N2".into(), 4);
    let d = decompose(&m, &o, &[0]);
    let tier = |n: i32| d.connections.iter().find(|c| c.net == n).unwrap().tier;
    assert_eq!((tier(1), tier(2)), (1, 4));
    assert_eq!(first(&d), 1, "tier 1 routed before tier 4");
    assert!(tier_weight(1) < tier_weight(3) && tier_weight(3) < tier_weight(4));

    // Out-of-range values are ignored with a warning (inference stays).
    let mut o = Options::default();
    o.net_priority.insert("N1".into(), 9);
    let d = decompose(&m, &o, &[0]);
    assert_eq!(d.connections[0].tier, 3);
    assert!(d.warnings.iter().any(|w| w.contains("N1") && w.contains("priority")));
}

#[test]
fn tier_4_pays_lower_via_cost() {
    assert!(via_weight(4) < via_weight(3));
    assert!(via_weight(3) < via_weight(1));
    let m = two_nets();
    let mut o = opts();
    o.net_priority.insert("N1".into(), 4);
    let d = decompose(&m, &o, &[0]);
    let c = d.connections.iter().find(|c| c.net == 1).unwrap();
    assert_eq!(c.tier, 4);
    let base = o.via_cost_nm as f64;
    assert!(base * via_weight(c.tier) < base * via_weight(3));
    // The run still completes with the table in force.
    let r = route(&m, &o);
    assert_eq!(r.routed, 2, "{:?}", r.connections);
    clean(&m, &r);
}

fn half_wall() -> BoardModel {
    // Net 3 is half joined by hand copper (to x = 30 mm) and not routed; net 1 must pass it.
    model(
        &[
            P("A", "1", 1, 20.0, 20.0, "[0]"),
            P("A", "2", 1, 35.0, 28.0, "[0]"),
            P("W", "1", 3, 1.2, 24.0, "[0]"),
            P("W", "2", 3, 48.8, 24.0, "[0]"),
        ],
        r#""kept": [ { "uuid": "wall", "net": 3, "layer": 0, "width": 250000, "locked": true,
            "prim": { "kind": "seg", "a": [1200000, -24000000], "b": [30000000, -24000000] } } ],"#,
    )
}

#[test]
fn excluded_net_copper_is_fixed_obstacle() {
    let m = half_wall();
    let free = route(&m, &opts());
    assert_eq!(free.connections.iter().filter(|c| c.net == 3).count(), 1);

    let mut o = opts();
    o.exclude_nets = vec!["N3".into()];
    let r = route(&m, &o);
    let w = r.nets.iter().find(|n| n.net == 3).unwrap();
    assert_eq!(w.status, "excluded");
    assert_eq!(w.connections, 0);
    assert!(r.connections.iter().all(|c| c.net != 3));
    assert!(r.items.iter().all(|i| !matches!(i, topor_engine::model::Item::Track(t) if t.net == 3)));
    assert_eq!(r.connections_total, 1);
    assert_eq!(r.routed, 1, "{:?}", r.connections);
    // The kept wall is an obstacle: the routed copper keeps clear of it.
    clean(&m, &r);
    let c = Checker::new(&m, &Options { margin_nm: 0, ..Options::default() });
    assert!(c.check_items(&r.items).violations.is_empty());
}

#[test]
fn ignored_pad_skipped_and_reported() {
    let m = model(
        &[
            P("A", "1", 1, 10.0, 10.0, "[0]"),
            P("A", "2", 1, 40.0, 10.0, "[0]"),
            P("A", "3", 1, 60.0, 10.0, "[0]"), // outside the board
            P("B", "1", 2, 10.0, 30.0, "[0]"),
            P("B", "2", 2, 20.0, 30.0, "[0]"),
        ],
        "",
    );
    let mut o = opts();
    o.ignore_pads = vec![PadRef::try_from("A.3".to_string()).unwrap()];
    let r = route(&m, &o);
    let ig: Vec<_> = r.connections.iter().filter(|c| c.status == "ignored").collect();
    assert_eq!(ig.len(), 1);
    assert_eq!(ig[0].reason, "pad outside board");
    assert_eq!(ig[0].from.label, "A.3");
    assert_eq!(ig[0].net, 1);
    assert_eq!(r.routed, 2, "{:?}", r.connections);
    assert_eq!(r.connections_total, 2, "ignored pads are not counted");
    assert_eq!(r.failed, 0);
    // No connection involves the ignored pad.
    assert!(r
        .connections
        .iter()
        .filter(|c| c.status != "ignored")
        .all(|c| c.from.label != "A.3" && c.to.label != "A.3"));
    // Without the option the pad is a terminal (and unreachable).
    let r0 = route(&m, &opts());
    assert!(r0.connections.iter().all(|c| c.status != "ignored"));
    assert!(r0.connections.iter().any(|c| c.to.label == "A.3" || c.from.label == "A.3"));
}

fn pour_net() -> BoardModel {
    model(
        &[
            P("G", "1", 1, 10.0, 10.0, "[0]"),
            P("G", "2", 1, 40.0, 10.0, "[0]"),
            P("A", "1", 2, 10.0, 30.0, "[0]"),
            P("A", "2", 2, 40.0, 30.0, "[0]"),
        ],
        r#""pours": [ { "uuid": "z", "net": 1, "layers": [0], "pads": ["G.1", "G.2"], "vias": [] } ],"#,
    )
}

#[test]
fn pour_served_override_both_directions() {
    let m = pour_net();
    // auto: the pour serves net 1
    let d = decompose(&m, &Options::default(), &[0]);
    assert!(d.nets.iter().find(|n| n.net == 1).unwrap().pour_served);
    // false: net 1 is routed as a signal net
    let mut o = Options::default();
    o.pour_served.insert("N1".into(), false);
    let d = decompose(&m, &o, &[0]);
    let n1 = d.nets.iter().find(|n| n.net == 1).unwrap();
    assert!(!n1.pour_served);
    assert_eq!(n1.connections.len(), 1);
    // true: net 2 has no pour but is trusted to be served: no connections
    let mut o = Options::default();
    o.pour_served.insert("N2".into(), true);
    let d = decompose(&m, &o, &[0]);
    let n2 = d.nets.iter().find(|n| n.net == 2).unwrap();
    assert!(n2.pour_served && n2.connections.is_empty());
    // end to end
    let mut o = opts();
    o.pour_served.insert("N1".into(), false);
    let r = route(&m, &o);
    assert_eq!(r.routed, 2, "{:?}", r.connections);
    assert_eq!(r.nets.iter().find(|n| n.net == 1).unwrap().status, "routed");
    let mut o = opts();
    o.pour_served.insert("N2".into(), true);
    let r = route(&m, &o);
    assert_eq!(r.nets.iter().find(|n| n.net == 2).unwrap().status, "pour_served");
    assert_eq!(r.connections_total, 0);
}
