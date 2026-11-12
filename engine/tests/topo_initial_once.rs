//! Shopping list 4.3 (initial routing once): the search's length is the shortest path through the
//! sleeve that keeps the clearances (`[search] length = "taut"`, Polubasov 2010, «Начальная
//! трассировка»), with the projection with and without the Steiner grid; double crossings are
//! removed after every wire; a wire is always placed (violations priced, never "no path").

use topor_engine::geom::Vec2;
use topor_engine::model::BoardModel;
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::demand::Item;
use topor_engine::topo::factors::Registry;
use topor_engine::topo::init::{funnel_step, route_all_once, route_all_reg, via_seg, Step};
use topor_engine::topo::state::{State, VertKind};

const MM: f64 = 1e6;

fn v(x: f64, y: f64) -> Vec2 {
    Vec2 { x: x * MM, y: y * MM }
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// Two-layer board, `outline` (x0, y0, x1, y1) mm, pads (net, x, y, th, size) mm, KiCad frame.
fn board(outline: (f64, f64, f64, f64), pads: &[(i32, f64, f64, bool, f64)]) -> BoardModel {
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
            let (attr, layers, drill) = if th { ("pth", "[0, 2]", format!(r#"{{"w":{0},"h":{0}}}"#, p(d / 2.0))) } else { ("smd", "[0]", "null".to_string()) };
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
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}},{{"code":3,"name":"C"}}],
            "pads":[{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn opts(steiner: bool) -> BuildOpts {
    let d = BuildOpts::default();
    BuildOpts { steiner_mm: if steiner { d.steiner_mm } else { 0.0 }, ..d }
}

#[test]
fn via_seg_and_funnel_step_hand_values() {
    // Straight through the span.
    let (l, x) = via_seg(v(0.0, 0.0), v(5.0, -1.0), v(5.0, 1.0), v(10.0, 0.0));
    assert!(close(l, 10.0 * MM, 1.0) && (x - v(5.0, 0.0)).len() < 1.0, "{l} {x:?}");
    // The span lies above the line: the path bends at its lower end, 2 x sqrt(5² + 1²).
    let (l, x) = via_seg(v(0.0, 0.0), v(5.0, 1.0), v(5.0, 3.0), v(10.0, 0.0));
    assert!(close(l, 2.0 * 26f64.sqrt() * MM, 1.0) && (x - v(5.0, 1.0)).len() < 1.0, "{l}");
    // Both ends on one side of the span's line: the lower bound through the mirror, sqrt(10² + 2²).
    let (l, _) = via_seg(v(0.0, 1.0), v(5.0, 0.0), v(15.0, 0.0), v(10.0, 1.0));
    assert!(close(l, 104f64.sqrt() * MM, 1.0), "{l}");
    // The cone from the origin through x = 5, y -1..1 meets x = 10 in y -2..2.
    match funnel_step(v(0.0, 0.0), v(5.0, -1.0), v(5.0, 1.0), v(10.0, -5.0), v(10.0, 5.0)) {
        Step::Vis(a, b) => assert!((a - v(10.0, -2.0)).len() < 1.0 && (b - v(10.0, 2.0)).len() < 1.0, "{a:?} {b:?}"),
        s => panic!("{s:?}"),
    }
    // A span wholly above the cone: bend at the upper end of the span before.
    assert_eq!(funnel_step(v(0.0, 0.0), v(5.0, -1.0), v(5.0, 1.0), v(10.0, 3.0), v(10.0, 8.0)), Step::Bend(v(5.0, 1.0)));
    // From a point (a terminal), everything is visible.
    assert_eq!(funnel_step(v(0.0, 0.0), v(0.0, 0.0), v(0.0, 0.0), v(3.0, -1.0), v(3.0, 1.0)), Step::Vis(v(3.0, -1.0), v(3.0, 1.0)));
}

/// A free straight run: the taut length is the pad distance (hand value 16 mm), with and without
/// the Steiner grid; the evenly spaced estimate is never shorter.
#[test]
fn taut_length_of_a_free_straight_wire_is_the_pad_distance() {
    let m = board((0.0, 0.0, 20.0, 10.0), &[(1, 2.0, 5.0, true, 1.0), (1, 18.0, 5.0, true, 1.0), (2, 6.0, 8.5, true, 1.0), (2, 14.0, 8.5, true, 1.0), (3, 9.0, 1.5, true, 1.0), (3, 12.0, 1.0, true, 1.0)]);
    for steiner in [false, true] {
        let b = board::build(&m, &opts(steiner)).unwrap();
        let conns = b.connections(&m);
        let mut st = State::new(&b.proj);
        st.taut_len = true;
        let rep = route_all_reg(&mut st, &conns, &Registry::standard(), 2_000_000, 450_000.0);
        assert!(rep.failed.is_empty());
        let w = st.wires.iter().position(|w| w.alive && w.net == 1).unwrap();
        let taut = st.wire_len(w) / MM;
        assert!(close(taut, 16.0, 1e-3), "steiner {steiner}: taut {taut}");
        st.taut_len = false;
        let even = st.wire_len(w) / MM;
        assert!(even >= taut - 1e-6, "steiner {steiner}: even {even} < taut {taut}");
    }
}

/// The span of an empty edge between another net's pad and the board edge: from the pad its
/// clearance plus half the width, from the board edge the edge clearance plus half the width
/// (hand values from the rules: 0.2 + 0.125 and edge clearance + 0.125 mm).
#[test]
fn the_span_of_an_empty_edge_keeps_the_end_clearances() {
    let m = board((0.0, 0.0, 20.0, 10.0), &[(1, 2.0, 5.0, true, 1.0), (1, 18.0, 5.0, true, 1.0), (2, 10.0, 8.0, true, 1.0), (2, 10.0, 2.0, true, 1.0)]);
    let b = board::build(&m, &opts(false)).unwrap();
    let pr = &b.proj;
    let st = State::new(pr);
    let rule = m.rules.net_rule(1);
    let me = Item { net: 1, w: rule.width as f64, s: rule.clearance as f64 };
    let ec = pr.edge_clear;
    let mut found = 0;
    for e in 0..pr.edges.len() {
        let [p, q] = pr.edges[e];
        let (VertKind::Outline(o), VertKind::Edge) = (pr.vkind[p], pr.vkind[q]) else { continue };
        if pr.obstacles[o].net != Some(2) || !st.edge_order[e].is_empty() {
            continue;
        }
        let mut sp = Vec::new();
        st.gap_spans(e, me, 0b11, None, &mut sp);
        let len = (pr.pos[q] - pr.pos[p]).len();
        let lo = pr.obstacles[o].clear.max(me.s) + 0.5 * me.w;
        let hi = len - (ec + 0.5 * me.w);
        assert_eq!(sp.len(), 1);
        assert!(close(sp[0].0, lo, 1.0) && close(sp[0].1, hi, 1.0), "edge {e}: {:?} vs ({lo}, {hi})", sp[0]);
        found += 1;
    }
    assert!(found > 0, "an edge from a pad of net 2 to the board edge");
}

/// A wire past another net's pad that blocks the straight line: the taut path bends within the
/// clearance band of the pad's corners: at least the path around the bare corners, at most that
/// plus the band on the four bends.
#[test]
fn taut_length_bends_around_a_pad_at_its_clearance() {
    // Net 2 pad 2 x 2 mm on the line y = 5; net 1 runs from x 2 to x 18.
    let m = board((0.0, 0.0, 20.0, 10.0), &[(1, 2.0, 5.0, true, 1.0), (1, 18.0, 5.0, true, 1.0), (2, 10.0, 5.0, true, 2.0), (2, 10.0, 9.0, true, 0.6)]);
    let rule = m.rules.net_rule(1);
    let band = (rule.clearance as f64 + 0.5 * rule.width as f64) / MM;
    for steiner in [false, true] {
        let b = board::build(&m, &opts(steiner)).unwrap();
        let conns: Vec<_> = b.connections(&m).into_iter().filter(|c| c.net == 1).collect();
        let mut st = State::new(&b.proj);
        st.taut_len = true;
        let rep = route_all_reg(&mut st, &conns, &Registry::standard(), 2_000_000, 450_000.0);
        assert!(rep.failed.is_empty());
        let w = st.wires.iter().position(|w| w.alive && w.net == 1).unwrap();
        let taut = st.wire_len(w) / MM;
        // Around the bare corners (9, 4) and (11, 4) (or their mirror): 2 x sqrt(7² + 1²) + 2.
        let bare = 2.0 * 50f64.sqrt() + 2.0;
        assert!(taut >= bare - 1e-6 && taut <= bare + 4.0 * band, "steiner {steiner}: taut {taut} bare {bare} band {band}");
    }
}

/// Two wires side by side through the same edges; swapping their passes on a run of shared edges
/// makes a double crossing (two crossings of one pair), which `uncross_from` removes again.
#[test]
fn uncross_from_removes_the_double_crossing_of_a_wire() {
    let m = board((0.0, 0.0, 20.0, 10.0), &[(1, 2.0, 5.6, true, 0.6), (1, 18.0, 5.6, true, 0.6), (2, 2.0, 6.4, true, 0.6), (2, 18.0, 6.4, true, 0.6), (3, 7.0, 9.0, true, 0.6), (3, 13.0, 1.0, true, 0.6)]);
    for steiner in [false, true] {
        let b = board::build(&m, &opts(steiner)).unwrap();
        let conns: Vec<_> = b.connections(&m).into_iter().filter(|c| c.net != 3).collect();
        let mut st = State::new(&b.proj);
        st.taut_len = true;
        let rep = route_all_once(&mut st, &conns, &Registry::standard(), 2_000_000, 450_000.0);
        assert!(rep.failed.is_empty() && rep.forced.is_empty(), "{rep:?}");
        assert_eq!(st.total_crossings(), 0, "steiner {steiner}: the two nets need not cross");
        let (a, bw) = (st.wires.iter().position(|w| w.alive && w.net == 1).unwrap(), st.wires.iter().position(|w| w.alive && w.net == 2).unwrap());
        // A run of shared edges of the two wires (same order).
        let ea: Vec<usize> = st.wires[a].passes.iter().map(|&p| st.passes[p].edge).collect();
        let eb: Vec<usize> = st.wires[bw].passes.iter().map(|&p| st.passes[p].edge).collect();
        let mut run = None;
        'f: for i in 1..ea.len() {
            for j in 1..eb.len() {
                if ea[i] == eb[j] && i + 1 < ea.len() && j + 1 < eb.len() && ea[i + 1] == eb[j + 1] {
                    run = Some((i, j));
                    break 'f;
                }
            }
        }
        let Some((i, j)) = run else { panic!("steiner {steiner}: the wires share no two edges in a row: {ea:?} {eb:?}") };
        let pairs = st.double_stretch(a, (i, i + 2), bw, (j, j + 2)).expect("same edges");
        st.uncross_double(&pairs);
        let twice = st.crossings(a).iter().filter(|x| x.other == bw).count();
        assert_eq!(twice, 2, "steiner {steiner}: the swap makes a double crossing");
        let swaps = st.uncross_from(a, 1000);
        assert!(swaps >= 1 && st.total_crossings() == 0, "steiner {steiner}: swaps {swaps}, crossings {}", st.total_crossings());
        assert!(st.check().is_ok(), "{:?}", st.check());
    }
}

/// A search that cannot finish (cap of one expansion) fails the connection in today's initial
/// routing; the 4.3 initial routing places every wire anyway and lists them as forced.
#[test]
fn a_wire_without_a_found_path_is_placed_and_priced() {
    let m = board((0.0, 0.0, 20.0, 10.0), &[(1, 2.0, 5.0, true, 1.0), (1, 18.0, 5.0, true, 1.0), (2, 10.0, 8.0, true, 1.0), (2, 10.0, 2.0, true, 1.0)]);
    for steiner in [false, true] {
        let b = board::build(&m, &opts(steiner)).unwrap();
        let conns = b.connections(&m);
        let mut st = State::new(&b.proj);
        let old = route_all_reg(&mut st, &conns, &Registry::standard(), 1, 450_000.0);
        assert!(!old.failed.is_empty(), "steiner {steiner}: the capped search fails today");
        let mut st = State::new(&b.proj);
        st.taut_len = true;
        let rep = route_all_once(&mut st, &conns, &Registry::standard(), 1, 450_000.0);
        assert!(rep.failed.is_empty(), "steiner {steiner}: never fails: {rep:?}");
        assert_eq!(rep.routed, conns.len());
        assert!(!rep.forced.is_empty(), "steiner {steiner}: reported as placed without a found path");
        assert_eq!(st.wires.iter().filter(|w| w.alive).count(), conns.len(), "every wire present");
        assert!(st.check().is_ok(), "{:?}", st.check());
    }
}

/// 4.3b: the initial routing order is (tier, width desc, length asc, input index); tier 1 first.
/// Wire ids follow the order the connections were routed in.
#[test]
fn initial_order_is_tier_first() {
    use topor_engine::topo::init::{route_all_once_tiered, route_all_reg_tiered, route_order};
    // Three far-apart nets, input order 1, 2, 3 (net 3 the shortest, net 1 the longest).
    let m = board((0.0, 0.0, 40.0, 20.0), &[(1, 2.0, 3.0, true, 1.0), (1, 38.0, 3.0, true, 1.0), (2, 2.0, 10.0, true, 1.0), (2, 30.0, 10.0, true, 1.0), (3, 2.0, 17.0, true, 1.0), (3, 12.0, 17.0, true, 1.0)]);
    let b = board::build(&m, &opts(false)).unwrap();
    let conns = b.connections(&m);
    assert_eq!(conns.len(), 3);
    let net_of = |o: &[usize]| o.iter().map(|&i| conns[i].net).collect::<Vec<_>>();
    let st = State::new(&b.proj);
    // Equal tiers (or none): length ascending, as before.
    assert_eq!(net_of(&route_order(&st, &conns, &[])), vec![3, 2, 1]);
    assert_eq!(net_of(&route_order(&st, &conns, &[3, 3, 3])), vec![3, 2, 1]);
    // Three tiers: tier beats length.
    let tiers: Vec<u8> = conns.iter().map(|c| match c.net { 1 => 1, 2 => 3, _ => 2 }).collect();
    assert_eq!(net_of(&route_order(&st, &conns, &tiers)), vec![1, 3, 2]);
    for once in [false, true] {
        let mut st = State::new(&b.proj);
        let reg = Registry::standard();
        let rep = if once { route_all_once_tiered(&mut st, &conns, &tiers, &reg, 2_000_000, 450_000.0) } else { route_all_reg_tiered(&mut st, &conns, &tiers, &reg, 2_000_000, 450_000.0) };
        assert!(rep.failed.is_empty());
        let nets: Vec<i32> = st.wires.iter().filter(|w| w.alive).map(|w| w.net).collect();
        assert_eq!(nets, vec![1, 3, 2], "once {once}");
    }
}
