use std::collections::HashSet;
use topor_engine::cdt::Crossing;
use topor_engine::check::Checker;
use topor_engine::geom::quant::TrackPrim;
use topor_engine::geom::{dequantise, Vec2};
use topor_engine::model::{BoardModel, ExistingCopper, Item, Options};
use topor_engine::negotiate::RIP_PAIR_CAP;
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
    let nets: Vec<String> = (1..=40)
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
    front_only(BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}")))
}

/// The M2 router routed F.Cu only: these tests pin that single-layer behaviour (walls, stubs,
/// zero-slack corridors), so every other copper layer is made non-routable. The two-layer
/// behaviour is pinned in `route_layers.rs`.
fn front_only(mut m: BoardModel) -> BoardModel {
    for l in &mut m.layers {
        l.routable = l.id == 0;
    }
    m
}

fn opts() -> Options {
    Options {
        // These tests pin geometry and iteration counts, not the clock: a budget that cannot
        // fire. The unoptimised build spends ~6 s in the block probe of the wall fixture alone
        // and the suite runs its tests concurrently; a 10 s budget made the partial pass skip
        // its stubs under load (debug-only failures). Tests of the deadline set their own.
        wall_clock_s: 600.0,
        // 22 tests run concurrently in a debug build: one thread each avoids oversubscribing the cores
        threads: 1,
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
    // M3: a terminal with copper on no routable layer fails before any search.
    assert_eq!(b.reason, "terminal not on any routable layer");
    assert_eq!(r.routed, 2, "{:?}", r.connections);
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
    assert!(
        r.connections[0].length_nm < 40_000_000,
        "{}",
        r.connections[0].length_nm
    );
    // Design clearance holds to the board file's resolution: an exact tangency rebuilt from
    // integer-nm points may read a nanometre short (KiCad's own DRCEpsilon is 500 nm).
    let c0 = Options {
        margin_nm: 0,
        ..Options::default()
    };
    for v in &Checker::new(&m, &c0).check_items(&r.items).violations {
        assert!(v.gap + 1 >= v.required, "{v:?}");
        assert!(
            v.against == "pad:C:1" || v.against.starts_with("edge"),
            "{v:?}"
        );
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
/// bench: 27 connections / 25 nets; hand outside-pad length 76.598 mm), both copper layers
/// routable.
fn eprom_ripped() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/eprom-to-kernal-ripped.json")).unwrap()
}

/// M3 on the real board (main ruling M3-R6): with B.Cu available the negotiation converges at
/// once, the zero-slack corridor of M2 (+5V along the top edge) is no longer needed, and the
/// outside-pad length stays below the M2 gate bound (B1: 67.095 mm) and the hand's 76.598 mm.
/// Deterministic: two runs give byte-identical reports.
#[test]
fn eprom_ripped_two_layers_routes_all_without_zero_slack() {
    let m = eprom_ripped();
    let r = route(&m, &opts());
    assert_eq!(r.routed, 27, "{:?}", r.failed);
    assert_eq!(r.nets_routed, 25);
    assert_eq!(r.failed, 0);
    assert!(r.converged, "{:?}", r.warnings);
    assert!(r.iterations < 50, "{}", r.iterations);
    assert!(r.zero_slack_wires.is_empty(), "{:?}", r.zero_slack_wires);
    assert!(
        r.length_outside_pads_nm <= 67_100_000,
        "{}",
        r.length_outside_pads_nm
    );
    assert!(r.length_outside_pads_nm < 76_598_000);
    let again = route(&m, &opts());
    assert_eq!(json_no_runtime(&r), json_no_runtime(&again));
}

/// R6 on the real board, F.Cu only: the M2 situation and the M2 gate result. A single routable
/// layer gets no via sites (M3 scaling diagnosis), so the graph is the M2 graph again: 27/27,
/// stall stop, the +5V corridor as the one zero-slack wire (R6, DRC-judged: its tangencies are
/// exact to the 1 nm quantisation, `ZS_QUANT_TOL_NM`), outside-pad length ratcheted at 67.8 mm
/// (M2 result 67.095 mm; ruling M3-R12, see the comment at the assertion). With free via-site vertices in the graph (T9–T12) the negotiation took
/// another way, 26/27 without the corridor.
#[test]
fn eprom_ripped_front_only_is_the_m2_result() {
    let m = front_only(eprom_ripped());
    let r = route(&m, &opts());
    assert_eq!(r.sites, 0);
    assert_eq!(r.routed, 27, "{} routed {:?}", r.routed, r.failed);
    assert!(r.iterations < 50, "{}", r.iterations);
    assert!(
        r.warnings
            .iter()
            .any(|w| w.starts_with("stalled:") || w.starts_with("stuck after")),
        "{:?}",
        r.warnings
    );
    assert_eq!(r.zero_slack, "kept");
    let zs: Vec<&str> = r.zero_slack_wires.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(zs, vec!["+5V"]);
    // Ratchet (ruling M3-R12): the M2 result was 67.095 mm. With the Rule-T rip fix of commit
    // 4cc9dad (ghost slots as `cross_slot` chooses them, so strands ending on the ghost's exit
    // edge are reported as blockers) the rips become effective and the negotiation takes
    // another trajectory: A12 takes a route 0.73 mm longer (67.767 mm). Bound = that + 33 µm,
    // and always below the hand's 76.598 mm.
    assert!(r.length_outside_pads_nm <= 67_800_000, "{}", r.length_outside_pads_nm);
    assert!(r.length_outside_pads_nm <= 76_598_000, "{}", r.length_outside_pads_nm);
    // Clean at margin 0 except the zero-slack wire, within the 1 nm quantisation tolerance.
    let zs_conn = r.zero_slack_wires[0].conn;
    let o = Options {
        margin_nm: 0,
        ..Options::default()
    };
    let rep = Checker::new(&m, &o).check_items(&r.items);
    for v in &rep.violations {
        let on_zs = matches!(&r.items[v.item], Item::Track(t) if t.wire == Some(zs_conn));
        assert!(on_zs && v.gap + 1 >= v.required, "{v:?}");
    }
    let again = route(&m, &opts());
    assert_eq!(json_no_runtime(&r), json_no_runtime(&again));
}

/// `n` two-pad nets (round pads d 1 mm on a 3 mm grid, each pair within 9 mm; the layout of
/// `tests/scaling/synth.py`, with a fixed LCG instead of Python's RNG), front layer only.
fn synth_model(n: usize) -> BoardModel {
    let cols = 10usize;
    let rows = 10usize;
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut rnd = |m: usize| {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((state >> 33) as usize) % m
    };
    let cells = cols * rows;
    let mut order: Vec<usize> = (0..cells).collect();
    for i in (1..cells).rev() {
        order.swap(i, rnd(i + 1));
    }
    let mut free: Vec<bool> = vec![true; cells];
    let xy = |c: usize| ((c % cols) as f64, (c / cols) as f64);
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for &i in &order {
        if pairs.len() == n {
            break;
        }
        if !free[i] {
            continue;
        }
        free[i] = false;
        let near: Vec<usize> = (0..cells)
            .filter(|&j| {
                free[j] && {
                    let (a, b) = (xy(i), xy(j));
                    ((a.0 - b.0).hypot(a.1 - b.1)) * 3.0 <= 9.0
                }
            })
            .collect();
        if near.is_empty() {
            continue;
        }
        let j = near[rnd(near.len())];
        free[j] = false;
        pairs.push((i, j));
    }
    assert_eq!(pairs.len(), n);
    let nets: Vec<String> = (1..=n)
        .map(|k| format!(r#"{{ "code": {k}, "name": "N{k}" }}"#))
        .collect();
    let mut pads = Vec::new();
    for (k, &(i, j)) in pairs.iter().enumerate() {
        for (num, c) in [(1, i), (2, j)] {
            let (x, y) = xy(c);
            pads.push(format!(
                r#"{{ "uuid": "p{k}-{num}", "ref": "P{k}", "number": "{num}", "net": {net}, "attr": "smd",
                  "center": [{px}, {py}], "layers": [0],
                  "shapes": {{ "0": {{ "kind": "circle", "d": 1000000 }} }} }}"#,
                net = k + 1,
                px = (x * 3.0 * MM) as i64,
                py = (y * 3.0 * MM) as i64
            ));
        }
    }
    let (lo, hi) = (-3_000_000i64, 30_000_000i64);
    let o = [
        (lo, lo, hi, lo),
        (hi, lo, hi, hi),
        (hi, hi, lo, hi),
        (lo, hi, lo, lo),
    ]
    .iter()
    .map(|&(a, b, c, d)| format!(r#"{{ "kind": "seg", "a": [{a}, {b}], "b": [{c}, {d}] }}"#))
    .collect::<Vec<_>>()
    .join(",");
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }} ], "nets": [ {} ], "pads": [ {} ],
          "outline": [ {o} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#,
        nets.join(","),
        pads.join(",")
    );
    front_only(BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}")))
}

#[test]
fn route_self_check_rounds_scale_with_changed_wires() {
    let m = synth_model(40);
    let r = route(&m, &opts());
    assert!(r.routed > 0, "nothing routed");
    assert!(r.self_check_rounds <= 3, "rounds {}", r.self_check_rounds);
    assert!(
        r.check_candidates_visited < 40 * 40 * 10,
        "visited {}",
        r.check_candidates_visited
    );
    eprintln!(
        "routed {}/{} rounds {} visited {}",
        r.routed, r.connections_total, r.self_check_rounds, r.check_candidates_visited
    );
}

/// D5: a Routing-constraint split vertex (two crossing same-net kept tracks, here a locked X of
/// net 5 corner to corner) that the net-5 connection's slot path has to bend around: the
/// connection fails once with a clear reason (`unanchored_bends`), the other connection routes,
/// and the negotiation stops early instead of looping. The pad positions were found by a seeded
/// random search over this fixture; the two foreign pads (nets 4 and 2) fix the homotopy.
#[test]
fn route_split_vertex_bend_fails_connection_once() {
    let x = |a: (i64, i64), b: (i64, i64)| -> String {
        format!(
            r#"{{ "uuid": "k{}", "net": 5, "layer": 0, "width": 250000, "locked": true,
            "prim": {{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }} }}"#,
            a.1,
            a.0 * 1_000_000,
            a.1 * 1_000_000,
            b.0 * 1_000_000,
            b.1 * 1_000_000
        )
    };
    let m = model(
        &[
            P("A", "1", 5, 22.345, 4.015, 1.0, "[0]"),
            P("A", "2", 5, 25.575, 34.878, 1.0, "[0]"),
            P("F", "1", 4, 26.48, 23.629, 1.0, "[0]"),
            P("G", "1", 2, 24.073, 14.05, 1.0, "[0]"),
            P("H", "1", 6, 4.0, 20.0, 1.0, "[0]"),
            P("H", "2", 6, 10.0, 20.0, 1.0, "[0]"),
        ],
        &format!(
            r#""kept": [ {}, {} ],"#,
            x((0, 0), (50, -40)),
            x((0, -40), (50, 0))
        ),
    );
    let r = route(&m, &opts());
    assert!(r.iterations <= 5, "iterations {}", r.iterations);
    let by = |n: i32| r.connections.iter().find(|c| c.net == n).unwrap();
    assert_eq!(by(6).status, "routed", "{:?}", r.connections);
    let a = by(5);
    // M2 failed this connection (its slot path had to bend at the split vertex of the crossing
    // net-5 tracks). The M3 sketch is built on the triangulation of the real obstacles (free via
    // sites deleted), the search finds another class, and the connection routes; either outcome
    // must be clean and, when it fails, give the clear reason.
    if a.status == "failed" {
        assert!(
            a.reason.contains("split vertex") || a.reason.starts_with("infeasible"),
            "{}",
            a.reason
        );
    } else {
        assert_eq!(a.status, "routed", "{a:?}");
    }
    clean(&m, &r);
}

/// D6: a foreign pad sits on the straight line between the start and the blocking wall (a locked
/// track spanning the board). The forward stub grows from the `last_reached` crossing along the
/// ghost crossings, so it passes the pad and reaches the wall clearance; the old stub from the
/// terminal straight to the blocking point was stopped by the pad and dropped.
#[test]
fn partial_stub_starts_at_last_reached() {
    let m = model(
        &[
            P("A", "1", 1, 20.0, 20.0, 1.0, "[0]"),
            P("A", "2", 1, 35.0, 28.0, 1.0, "[0]"),
            P("F", "1", 4, 22.5, 22.0, 1.0, "[0]"),
        ],
        r#""kept": [ { "uuid": "wall", "net": 3, "layer": 0, "width": 250000, "locked": true,
            "prim": { "kind": "seg", "a": [0, -24000000], "b": [50000000, -24000000] } } ],"#,
    );
    let o = Options {
        partial_routes: true,
        ..opts()
    };
    let r = route(&m, &o);
    let c = r.connections.iter().find(|c| c.net == 1).unwrap();
    assert_eq!(c.status, "partial", "{c:?}");
    let ends: Vec<f64> = r
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Track(t) if t.net == 1 => Some(t),
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
    // Both the forward stub (past the foreign pad) and the reverse stub reach the wall clearance.
    assert_eq!(
        ends.iter().filter(|d| **d < 470_000.0).count(),
        2,
        "{ends:?}"
    );
    let chk = Options {
        margin_nm: 0,
        ..Options::default()
    };
    let rep = Checker::new(&m, &chk).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

/// 10 walled-off connections (nets 10..19, one pad pair each across the wall) plus 20 free ones
/// (nets 20..39).
fn probe_model() -> BoardModel {
    let n = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let mut pads = vec![
        P("W", "1", 3, 1.2, 24.0, 1.0, "[0]"),
        P("W", "2", 3, 48.8, 24.0, 1.0, "[0]"),
    ];
    for k in 0..10 {
        let x = 6.0 + 4.0 * k as f64;
        pads.push(P(n(format!("U{k}")), "1", 10 + k, x, 30.0, 0.8, "[0]"));
        pads.push(P(n(format!("U{k}")), "2", 10 + k, x, 18.0, 0.8, "[0]"));
    }
    for k in 0..20 {
        let x = 4.0 + 2.0 * k as f64;
        pads.push(P(n(format!("F{k}")), "1", 20 + k, x, 36.0, 0.6, "[0]"));
        pads.push(P(n(format!("F{k}")), "2", 20 + k, x, 38.0, 0.6, "[0]"));
    }
    model(
        &pads,
        r#""kept": [ { "uuid": "wall", "net": 3, "layer": 0, "width": 250000, "locked": true,
            "prim": { "kind": "seg", "a": [1200000, -24000000], "b": [48800000, -24000000] } } ],"#,
    )
}

/// D9: the block probe stops at the deadline and at `PROBE_STEP_CAP` steps per connection.
#[test]
fn block_probe_respects_deadline_and_step_cap() {
    let m = probe_model();
    let t0 = std::time::Instant::now();
    let r = route(
        &m,
        &Options {
            wall_clock_s: 0.05,
            partial_routes: true,
            ..Options::default()
        },
    );
    assert!(t0.elapsed().as_secs_f64() < 1.0, "{:?}", t0.elapsed());
    assert!(r.timed_out, "{} routed", r.routed);
    // Generous budget: the unoptimised build needs > 10 s here, and a probe that starts after
    // the deadline is (correctly) skipped, so the assertions below must not race the clock.
    let r = route(
        &m,
        &Options {
            wall_clock_s: 600.0,
            partial_routes: true,
            ..opts()
        },
    );
    assert!(!r.timed_out);
    assert_eq!(r.connections.len(), 30);
    let blocked: Vec<_> = r
        .connections
        .iter()
        .filter(|c| c.status != "routed")
        .collect();
    assert!(blocked.len() >= 10, "{} not routed", blocked.len());
    for c in blocked {
        assert!(c.blocking.is_some(), "{c:?}");
        let n = c.probe_steps.expect("walled-off connections are probed");
        assert!(
            (1..=topor_engine::route::PROBE_STEP_CAP).contains(&n),
            "{n}"
        );
    }
}

#[test]
fn profile_absent_by_default_and_per_iteration_when_enabled() {
    let m = four_nets();
    let off = route(&m, &opts());
    let v = serde_json::to_value(&off).unwrap();
    assert!(
        v.get("profile").is_none(),
        "profile must not be emitted by default"
    );
    assert_eq!(json_no_runtime(&off), json_no_runtime(&route(&m, &opts())));

    let on = route(
        &m,
        &Options {
            profile: true,
            ..opts()
        },
    );
    let p = on.profile.as_ref().expect("profile enabled");
    assert_eq!(p.len(), on.iterations as usize, "{p:?}");
    for (i, it) in p.iter().enumerate() {
        assert_eq!(it.iteration as usize, i + 1);
        assert!(
            it.search_s >= 0.0 && it.commit_s >= 0.0 && it.metrize_s >= 0.0 && it.check_s >= 0.0
        );
        assert!(it.check_s <= it.metrize_s + 1e-9);
    }
    assert_eq!(p[0].searched, 4);
    // the enabled run routes identically
    let mut a = serde_json::to_value(&on).unwrap();
    a.as_object_mut().unwrap().remove("profile");
    let mut b = serde_json::to_value(&off).unwrap();
    for k in ["runtime_s", "threads_used"] {
        a.as_object_mut().unwrap().remove(k);
        b.as_object_mut().unwrap().remove(k);
    }
    assert_eq!(a, b);
}

/// Regression (M3 scaling diagnosis, layers=1): a board with one routable layer has no layer to
/// change to, so no via sites are generated. Dormant site vertices multiplied the faces of the
/// single graph (synth n=1000: 11 837 sites, 12x expansions / 70x clearance candidates per
/// search, overuse that never cleared, 25 stalled iterations and 999/1000 instead of 7
/// converged iterations and 1000/1000).
#[test]
fn single_layer_board_generates_no_via_sites() {
    // 4 x 3 grid of 1 mm SMD pads on a 3 mm pitch, six horizontal pairs (synth-like).
    let refs: &'static [&'static str] = &["P1", "P2", "P3", "P4", "P5", "P6"];
    let mut pads = Vec::new();
    for net in 1..=6i32 {
        let (col, row) = (((net - 1) % 2) * 2, (net - 1) / 2);
        let (x, y) = (10.0 + col as f64 * 3.0, 10.0 + row as f64 * 3.0);
        let r = refs[(net - 1) as usize];
        pads.push(P(r, "1", net, x, y, 1.0, "[0]"));
        pads.push(P(r, "2", net, x + 3.0, y, 1.0, "[0]"));
    }
    let m = model(&pads, "");
    let r = route(&m, &opts());
    assert_eq!(r.sites, 0, "sites on a single-layer board");
    assert_eq!(r.routed, 6, "{:?}", r.connections);
    assert_eq!(r.vias, 0);
    assert_eq!(r.iterations, 1);
    assert!(r.converged);
    clean(&m, &r);
}

/// A pour target on a single-layer board still needs its via sites (the pour goal is a site).
#[test]
fn single_layer_pour_target_keeps_via_sites() {
    let m = model(
        &[
            P("U1", "1", 1, 10.0, 15.0, 1.0, "[0]"),
            P("U2", "1", 1, 40.0, 15.0, 1.0, "[0]"),
        ],
        r#""pours": [ { "uuid": "zz", "net": 1, "layers": [0], "pads": ["U1.1"], "vias": [],
            "fills": [ { "layer": 0, "outer": [[0, 0], [30000000, 0], [30000000, -30000000], [0, -30000000]] } ] } ],"#,
    );
    let r = route(&m, &opts());
    assert_eq!(r.connections.len(), 1, "{:?}", r.connections);
    assert_eq!(r.connections[0].to.kind, "pour");
    assert!(r.sites >= 1, "pour completion needs sites, got {}", r.sites);
}

/// A plus-shaped board (two 3 mm bars crossing at (22, 22)): A (net 1) runs along the horizontal
/// bar, B (net 2) along the vertical one. Each terminal is a rect pad taller than its bar, so the
/// pad's own polygon (a constraint chain from outline to outline, hard for the other net) seals
/// the arm end: neither connection can go round the other's pad, and the faces outside the
/// outline are hard too. The one routed second is blocked, its ghost crosses the first, the
/// first is ripped, and next iteration the roles swap.
fn sealed_cross_model() -> BoardModel {
    let pts: [(f64, f64); 12] = [
        (20.5, 2.0),
        (23.5, 2.0),
        (23.5, 20.5),
        (42.0, 20.5),
        (42.0, 23.5),
        (23.5, 23.5),
        (23.5, 42.0),
        (20.5, 42.0),
        (20.5, 23.5),
        (2.0, 23.5),
        (2.0, 20.5),
        (20.5, 20.5),
    ];
    let segs: Vec<String> = (0..12)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % 12]);
            format!(
                r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
                (a.0 * MM) as i64,
                (-a.1 * MM) as i64,
                (b.0 * MM) as i64,
                (-b.1 * MM) as i64
            )
        })
        .collect();
    let rect = |r: &str, n: &str, net: i32, x: f64, y: f64, w: f64, h: f64| {
        format!(
            r#"{{ "uuid": "{r}{n}", "ref": "{r}", "number": "{n}", "net": {net},
              "center": [{}, {}], "layers": [0],
              "shapes": {{ "0": {{ "kind": "rect", "w": {}, "h": {} }} }} }}"#,
            (x * MM) as i64,
            (-y * MM) as i64,
            (w * MM) as i64,
            (h * MM) as i64
        )
    };
    let pads = [
        rect("A", "1", 1, 4.0, 22.0, 1.0, 4.0),
        rect("A", "2", 1, 40.0, 22.0, 1.0, 4.0),
        rect("B", "1", 2, 22.0, 4.0, 4.0, 1.0),
        rect("B", "2", 2, 22.0, 40.0, 4.0, 1.0),
    ];
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "N1" }}, {{ "code": 2, "name": "N2" }} ],
          "pads": [ {} ], "outline": [ {} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#,
        pads.join(","),
        segs.join(",")
    );
    front_only(BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}")))
}

/// F2 damping (RC-A2): a connection rips a given blocker at most `RIP_PAIR_CAP` times per run.
/// On the sealed cross A and B rip each other out of the only class every iteration (A routes,
/// B is blocked and rips A; B routes, A is blocked and rips B; ...): the cap ends the exchange
/// after exactly `2 * RIP_PAIR_CAP` iterations, every later iteration rips nothing, and the loop
/// ends by the stall rule. Without the cap every iteration up to the stall stop rips. One routed,
/// the other blocked, iteration count pinned, deterministic.
#[test]
fn rip_up_ping_pong_is_damped() {
    let m = sealed_cross_model();
    let o = Options {
        profile: true,
        ..opts()
    };
    let r = route(&m, &o);
    assert_eq!((r.routed, r.failed), (1, 1), "{:?}", pat(&r));
    let p = r.profile.as_ref().unwrap();
    let rips: Vec<usize> = p.iter().map(|it| it.ripped).collect();
    let n = 2 * RIP_PAIR_CAP as usize;
    assert!(rips.len() > n, "{rips:?}");
    assert!(rips[..n].iter().all(|&k| k > 0), "{rips:?}");
    assert!(rips[n..].iter().all(|&k| k == 0), "ping-pong not damped: {rips:?}");
    assert_eq!(r.iterations, 9, "{:?}", r.warnings);
    assert!(
        r.warnings.iter().any(|w| w.starts_with("stalled:")),
        "{:?}",
        r.warnings
    );
    let blocked = r.connections.iter().find(|c| c.status != "routed").unwrap();
    assert!(blocked.reason.starts_with("blocked: no path"), "{blocked:?}");
    clean(&m, &r);
    // deterministic (the profile carries wall-clock seconds: compared without it)
    let r2 = route(&m, &o);
    let strip = |r: &RouteResult| {
        let mut v = serde_json::to_value(r).unwrap();
        let o = v.as_object_mut().unwrap();
        o.remove("runtime_s");
        o.remove("profile");
        serde_json::to_string(&v).unwrap()
    };
    assert_eq!(strip(&r), strip(&r2));
}

/// F2 (ruling M3-R11): every connection reports `ghost_len_nm`, the funnel length of its
/// topological ghost on the empty graph (no other connection present): the detour reference
/// `length_nm / ghost_len_nm` of the harness. A straight pair reports its straight distance; a
/// walled-off connection still has a ghost (through the wall, soft); a connection that cannot
/// be searched reports `null`.
#[test]
fn ghost_len_reported_for_every_connection() {
    let m = model(
        &[
            P("A", "1", 1, 10.0, 10.0, 1.0, "[0]"),
            P("A", "2", 1, 30.0, 10.0, 1.0, "[0]"),
        ],
        "",
    );
    let r = route(&m, &opts());
    let c = &r.connections[0];
    assert_eq!(c.status, "routed");
    assert_eq!(c.ghost_len_nm, Some(20_000_000));
    assert_eq!(c.length_nm, 20_000_000);
    let v: serde_json::Value = serde_json::from_str(&json_no_runtime(&r)).unwrap();
    assert_eq!(v["connections"][0]["ghost_len_nm"], serde_json::json!(20_000_000));
    // the wall board: the blocked connection still has a ghost length. The ghost minimises soft
    // elements first: the wall track is a foreign constraint (soft), the 0.7 mm gap at the
    // board edge fits the capacity model (the edge clearance is geometry, not capacity), so
    // the reference is the way round the wall end, not the straight line through the wall.
    let w = wall_model();
    let rw = route(&w, &opts());
    let b = rw.connections.iter().find(|c| c.status != "routed").unwrap();
    let gl = b.ghost_len_nm.expect("ghost round the wall");
    let straight = ((35.0 - 20.0f64).powi(2) + (28.0 - 20.0f64).powi(2)).sqrt() * MM;
    assert!(gl as f64 > 2.0 * straight && (gl as f64) < 60.0 * MM, "{gl}");
    // a pad on no routable layer: never searched, null
    let n = model(
        &[
            P("A", "1", 1, 10.0, 10.0, 1.0, "[0]"),
            P("A", "2", 1, 30.0, 10.0, 1.0, "[31]"),
        ],
        "",
    );
    let rn = route(&n, &opts());
    assert_eq!(rn.connections[0].ghost_len_nm, None);
    let v: serde_json::Value = serde_json::from_str(&json_no_runtime(&rn)).unwrap();
    assert!(v["connections"][0]["ghost_len_nm"].is_null());
}
