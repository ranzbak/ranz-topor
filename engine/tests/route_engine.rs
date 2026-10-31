use std::collections::HashSet;
use topor_engine::cdt::Crossing;
use topor_engine::check::Checker;
use topor_engine::geom::quant::TrackPrim;
use topor_engine::geom::{dequantise, Vec2};
use topor_engine::model::{BoardModel, ExistingCopper, Item, Options};
use topor_engine::obstacle::ObstacleSet;
use topor_engine::route::{route, wire_stats, RouteResult, STALL_ITERS};
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

/// R6 zero-slack retry board. Net 1 (A1 -> A2) along the top edge; the foreign pad C (net 3)
/// sits exactly `r + clr + w + edge_clr` = 1.45 mm below the edge: the corridor above it has
/// zero slack (feasible at margin 0, 10 µm short at ε = 5 µm). A locked net-3 track from C
/// down to C2 walls the board, leaving the long way round below C2 (0.34 mm slack). Pad E
/// (net 4) bends the corridor path on the A2 side with room to spare (a bend that keeps ε).
fn zero_slack_model() -> BoardModel {
    model(
        &[
            P("A", "1", 1, 5.0, 1.45, 1.0, "[0]"),
            P("A", "2", 1, 35.0, 1.45, 1.0, "[0]"),
            P("C", "1", 3, 20.0, 1.45, 1.0, "[0]"),
            P("C", "2", 3, 20.0, 14.0, 1.0, "[0]"),
            P("E", "1", 4, 30.0, 1.9, 1.0, "[0]"),
        ],
        r#""kept": [ { "uuid": "wall", "net": 3, "layer": 0, "width": 250000, "locked": true,
            "prim": { "kind": "seg", "a": [20000000, -1450000], "b": [20000000, -14000000] } } ],"#,
    )
}

/// Largest KiCad-frame y (nm) reached by the items (arcs by their rebuilt geometry; the board
/// spans y in [-40, 0] mm, so this is the point nearest the y = 0 edge).
fn max_y(r: &RouteResult) -> f64 {
    r.items
        .iter()
        .filter_map(|it| match it {
            Item::Track(t) => Some(dequantise(&t.prim).bbox().max.y),
            _ => None,
        })
        .fold(f64::MIN, f64::max)
}

/// R6: the retry routes net 1 through the zero-slack corridor above C with margin 0 against
/// that corridor only (C and the board edge); the bend around E keeps ε. Retry off: the long
/// way round, no zero-slack wire.
#[test]
fn zero_slack_retry_uses_margin_zero_on_corridor_only() {
    let m = zero_slack_model();
    let r = route(&m, &opts());
    assert_eq!(r.routed, 1, "{:?}", r.connections);
    assert_eq!(r.zero_slack, "kept", "{:?}", r.warnings);
    assert_eq!(r.zero_slack_wires.len(), 1);
    assert_eq!(r.zero_slack_wires[0].net, 1);
    assert!(r.connections[0].zero_slack);
    // Through the corridor: the centreline reaches the edge band, 0.5 + 0.125 = 0.625 mm
    // from the y = 0 edge.
    assert!((max_y(&r) + 625_000.0).abs() <= 2.0, "max y {}", max_y(&r));
    assert!(r.connections[0].length_nm < 40_000_000, "{}", r.connections[0].length_nm);
    // Design clearance holds to the board file's resolution: an exact tangency rebuilt from
    // integer-nm points may read a nanometre short (KiCad's own DRCEpsilon is 500 nm).
    let c0 = Options {
        margin_nm: 0,
        ..Options::default()
    };
    for v in &Checker::new(&m, &c0).check_items(&r.items).violations {
        assert!(v.gap + 1 >= v.required, "{v:?}");
        assert!(v.against == "pad:C:1" || v.against.starts_with("edge"), "{v:?}");
    }
    // Margin 0 on the corridor only: at a margin of 4 µm only C and the edge are too close,
    // the bend around E (and everything else) still has its ε slack.
    let c4 = Options {
        margin_nm: 4_000,
        ..Options::default()
    };
    let rep = Checker::new(&m, &c4).check_items(&r.items);
    let mut against: Vec<&str> = rep.violations.iter().map(|v| v.against.as_str()).collect();
    against.sort_unstable();
    against.dedup();
    assert!(against.contains(&"pad:C:1"), "{against:?}");
    assert!(against.iter().any(|a| a.starts_with("edge")), "{against:?}");
    assert!(!against.contains(&"pad:E:1"), "{against:?}");
    assert_eq!(against.len(), 2, "{against:?}");

    let off = route(
        &m,
        &Options {
            zero_slack_retry: false,
            ..opts()
        },
    );
    assert_eq!(off.routed, 1, "{:?}", off.connections);
    assert_eq!(off.zero_slack, "off");
    assert!(off.zero_slack_wires.is_empty());
    assert!(!off.connections[0].zero_slack);
    assert!(max_y(&off) < -1_000_000.0, "max y {}", max_y(&off));
    assert!(off.connections[0].length_nm > r.connections[0].length_nm);
    clean(&m, &off);
    // The retry pass only adds to a finished negotiation: same iterations either way.
    assert_eq!(off.iterations, r.iterations);
    let (a, b) = (route(&m, &opts()), route(&m, &opts()));
    assert_eq!(json_no_runtime(&a), json_no_runtime(&b));
}

/// Retry on, no sub-ε corridor to gain from: the result is the plain negotiation's.
#[test]
fn zero_slack_retry_without_candidate_is_identity() {
    let m = four_nets();
    let on = route(&m, &opts());
    let off = route(
        &m,
        &Options {
            zero_slack_retry: false,
            ..opts()
        },
    );
    assert_eq!(on.zero_slack, "no candidate", "{}", on.zero_slack);
    assert_eq!(off.zero_slack, "off");
    assert_eq!(on.items, off.items);
    assert_eq!(on.length_outside_pads_nm, off.length_outside_pads_nm);
    assert_eq!(on.iterations, off.iterations);
}

/// The eprom-to-kernal benchmark board after `harness.rip --rip all` (model JSON of the M2
/// bench, F.Cu, 27 connections / 25 nets; hand outside-pad length 76.598 mm).
fn eprom_ripped() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/eprom-to-kernal-ripped.json")).unwrap()
}

/// R6 on the real board. Stall stop: the negotiation oscillates between 26/27 and 27/27 after
/// its best (iteration 8) and used to run to the cap of 50; now it stops after `STALL_ITERS`
/// iterations without a better best. Zero-slack retry: +5V U2.28 -> J1.2 takes the hand's
/// corridor along the top edge (margin 0 against J1.3 / J1.4 and the edge only), A13 is
/// re-routed into its short class, and the outside-pad length drops below the hand's.
/// Deterministic: two runs give byte-identical reports.
#[test]
fn eprom_ripped_stalls_and_zero_slack_retry_passes_length_gate() {
    let m = eprom_ripped();
    let r = route(&m, &opts());
    assert_eq!(r.routed, 27, "{:?}", r.failed);
    assert_eq!(r.nets_routed, 25);
    assert!(!r.converged);
    assert!(
        r.warnings.iter().any(|w| w.starts_with("stalled:")),
        "{:?}",
        r.warnings
    );
    assert!(r.iterations < 50 && r.iterations >= STALL_ITERS, "{}", r.iterations);
    assert_eq!(r.zero_slack, "kept");
    let names: Vec<&str> = r.zero_slack_wires.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, vec!["+5V"]);
    assert!(r.length_outside_pads_nm < 76_598_000, "{}", r.length_outside_pads_nm);
    // M2 gate bound (B1: 67.095 mm); incremental metrization (B2) must not regress it.
    assert!(r.length_outside_pads_nm <= 67_100_000, "{}", r.length_outside_pads_nm);
    assert_eq!(r.failed, 0);
    let again = route(&m, &opts());
    assert_eq!(json_no_runtime(&r), json_no_runtime(&again));
    // Retry off: 27/27 as well, but the long +5V / A13 pair and no zero-slack wire.
    let off = route(
        &m,
        &Options {
            zero_slack_retry: false,
            ..opts()
        },
    );
    assert_eq!(off.routed, 27);
    assert!(off.zero_slack_wires.is_empty());
    assert_eq!(off.iterations, r.iterations);
    assert!(off.length_outside_pads_nm > 76_598_000, "{}", off.length_outside_pads_nm);
}
