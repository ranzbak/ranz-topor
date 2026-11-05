//! S3 (P4) tests: the social factors `squeeze`, `blocks_pad` and `ignored_rule` change P2's choice,
//! the `congestion` history moves a path, and the capacity-aware layers and negotiated reroute
//! never leave a crossing on one layer.

use topor_engine::model::BoardModel;
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::factors::{standard_factors, FactorConfig, Registry, WireCtx};
use topor_engine::topo::init::route_all_reg;
use topor_engine::topo::state::State;

const MM: f64 = 1e6;

fn cfg(weights: &[(&str, f64)]) -> FactorConfig {
    FactorConfig { weight: weights.iter().map(|&(n, w)| (n.to_string(), w)).collect(), ..FactorConfig::default() }
}

/// P2 on `m` with the factor weights; the state and the board are handed to `f`.
fn with_p2<R>(m: &BoardModel, weights: &[(&str, f64)], f: impl FnOnce(&State, &board::Board) -> R) -> R {
    let b = board::build(m, &BuildOpts::default()).unwrap();
    let conns = b.connections(m);
    let mut st = State::new(&b.proj);
    let (reg, problems) = Registry::from_config(standard_factors(), &cfg(weights));
    assert!(problems.is_empty(), "{problems:?}");
    let rep = route_all_reg(&mut st, &conns, &reg, 2_000_000, 400_000.0);
    assert!(rep.failed.is_empty(), "every connection routes");
    f(&st, &b)
}

/// KiCad-frame y (mm) range of the polyline of the wire of `net`.
fn y_range(st: &State, net: i32) -> (f64, f64) {
    let w = st.wires.iter().position(|w| w.alive && w.net == net).expect("wire of the net");
    let ys: Vec<f64> = st.polyline(w).iter().map(|p| -p.y / MM).collect();
    (ys.iter().cloned().fold(f64::INFINITY, f64::min), ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max))
}

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


// ---------------------------------------------------------------- ignored_rule in P2

/// Net 3 runs along the bottom edge past a pad of net 1; the passage between the pad and the
/// outline is 1.4 mm, the wire (0.25 mm, 0.2 clearance to the pad) needs 0.4 mm plus the
/// copper-to-edge rule, which is 1.1 mm here.
fn edge_board(ignored: bool) -> BoardModel {
    let mut m = board((0.0, 0.0, 10.0, 10.0), &[(3, 1.0, 1.0, true, 0.6), (3, 9.0, 1.0, true, 0.6), (1, 5.0, 1.9, true, 1.0)], "");
    m.rules.edge_clearance = 1_100_000;
    if ignored {
        m.rules.ignored_checks.push("copper_edge_clearance".into());
    }
    m
}

#[test]
fn an_ignored_edge_rule_is_priced_in_p2_not_hard() {
    // Hard rule: the passage does not fit (1.45 > 1.4), the wire goes around the pad.
    let hard = with_p2(&edge_board(false), &[], |st, _| y_range(st, 3).1);
    assert!(hard > 2.0, "around the pad: {hard}");
    // Ignored and free: the passage is allowed, the wire goes straight past the pad.
    let free = with_p2(&edge_board(true), &[("ignored_rule", 0.0)], |st, _| y_range(st, 3).1);
    assert!(free < 1.6, "through the passage: {free}");
    // Ignored at the default price (100 mm) the violation is dearer than the 2 mm detour.
    let priced = with_p2(&edge_board(true), &[], |st, _| y_range(st, 3).1);
    assert!(priced > 2.0, "priced out: {priced}");
}

// ---------------------------------------------------------------- squeeze

/// A wall of three pads with two doors. Nets 1 and 2 run short wires through the narrow door
/// (1.5 mm); net 3 starts and ends between their pads, so straight through that door it passes
/// between the two stretches with 0.1 mm of slack. The other door is 2.1 mm wide, 3.7 mm of
/// detour away.
fn door_board() -> BoardModel {
    board(
        (0.0, 0.0, 13.0, 10.0),
        &[
            (7, 1.9, 5.0, true, 2.8),
            (8, 6.2, 5.0, true, 2.8),
            (9, 11.0, 5.0, true, 2.6),
            (1, 3.7, 3.0, true, 0.4),
            (1, 3.7, 7.0, true, 0.4),
            (2, 4.7, 3.0, true, 0.4),
            (2, 4.7, 7.0, true, 0.4),
            (3, 4.2, 0.4, true, 0.4),
            (3, 4.2, 9.6, true, 0.4),
        ],
        "",
    )
}

/// Whether the wire of net 3 passes the wall between the pads at x = 1.9 and x = 6.2 (the narrow door).
fn through_narrow_door(weights: &[(&str, f64)]) -> bool {
    with_p2(&door_board(), weights, |st, b| {
        let obs_at = |x: f64| b.proj.obstacles.iter().position(|o| (o.center.x - x * MM).abs() < 1.0).unwrap();
        let door = st.passes_between(obs_at(1.9), obs_at(6.2));
        let w = st.wires.iter().position(|w| w.alive && w.net == 3).unwrap();
        door.iter().any(|&p| st.passes[p].wire == w)
    })
}

#[test]
fn squeeze_sends_a_wire_through_the_wide_door() {
    assert!(through_narrow_door(&[("squeeze", 0.0)]), "free squeeze: the short way between the two stretches");
    assert!(!through_narrow_door(&[("squeeze", 100.0)]), "a dear squeeze: the other door");
}

// ---------------------------------------------------------------- congestion history

#[test]
fn an_over_full_history_moves_a_wire_to_the_other_door() {
    let m = board(
        (0.0, 0.0, 13.0, 10.0),
        &[(7, 1.9, 5.0, true, 2.8), (8, 6.2, 5.0, true, 2.8), (9, 11.0, 5.0, true, 2.6), (3, 4.2, 0.4, true, 0.4), (3, 4.2, 9.6, true, 0.4)],
        "",
    );
    let run = |history: f64| {
        let b = board::build(&m, &BuildOpts::default()).unwrap();
        let conns = b.connections(&m);
        let mut st = State::new(&b.proj);
        let obs_at = |x: f64| b.proj.obstacles.iter().position(|o| (o.center.x - x * MM).abs() < 1.0).unwrap();
        for e in b.proj.edges_between(obs_at(1.9), obs_at(6.2)) {
            st.history[e] = history;
        }
        let rep = route_all_reg(&mut st, &conns, &Registry::standard(), 2_000_000, 400_000.0);
        assert!(rep.failed.is_empty());
        st.passes_between(obs_at(1.9), obs_at(6.2)).iter().any(|&p| st.wires[st.passes[p].wire].net == 3)
    };
    assert!(run(0.0), "no history: straight through the narrow door");
    assert!(!run(5.0), "five rounds of history (10 mm each per pass): the other door");
}

// ---------------------------------------------------------------- blocks_pad

/// The last free entry sector of a pad: all but one triangle of its ring are taken by another
/// net's stretch. Only a wire of a third net taking that last one blocks the pad; the pad's own
/// net does not, nor does anybody once the pad has a wire.
#[test]
fn blocks_pad_fires_only_for_the_last_free_sector_of_an_unrouted_pad() {
    let m = board((0.0, 0.0, 10.0, 10.0), &[(5, 5.0, 5.0, true, 0.8), (5, 9.0, 9.0, true, 0.8), (3, 1.0, 1.0, true, 0.4), (3, 1.0, 9.0, true, 0.4)], "");
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let conns = b.connections(&m);
    let mut st = State::new(&b.proj);
    let z = conns.iter().find(|c| c.net == 3).unwrap();
    st.route_conn(z, &topor_engine::topo::init::InitWeights::default(), 1.0).unwrap();
    let x = b.proj.obstacles.iter().position(|o| o.net == Some(5) && (o.center.x - 5.0 * MM).abs() < 1.0).unwrap();
    let ring = st.ring[x].clone();
    assert!(ring.len() > 3, "a ring of {}", ring.len());
    // The wire of net 3 "occupies" every sector but the last (a stand-in traversal entry).
    let last = *ring.last().unwrap();
    for &t in &ring[..ring.len() - 1] {
        st.tri_trav[t].push((0, 0));
    }
    assert_eq!(st.blocked_pads(last, 4), vec![x], "a third net taking the last free sector");
    assert!(st.blocked_pads(last, 5).is_empty(), "the pad's own net may take it");
    assert!(st.blocked_pads(ring[0], 4).is_empty(), "an occupied sector blocks nothing new");
    let price = |t: usize| Registry::standard().price(&topor_engine::topo::factors::Event::BlocksPad { wire: 9, tri: t, pad: x }, &WireCtx::default());
    assert_eq!(price(last), 30.0);
    // Once the pad has a wire there is nothing left to block.
    let c5 = conns.iter().find(|c| c.net == 5).unwrap();
    let mut st2 = State::new(&b.proj);
    st2.route_conn(c5, &topor_engine::topo::init::InitWeights::default(), 1.0).unwrap();
    assert!(st2.blocked_pads(last, 4).is_empty());
}

#[test]
fn a_path_through_the_last_free_sector_costs_the_blocks_pad_price_more() {
    use topor_engine::topo::factors::{events, Event, Move};
    use topor_engine::topo::state::Wire;
    let m = board(
        (0.0, 0.0, 10.0, 10.0),
        &[(5, 5.0, 5.0, true, 0.8), (5, 9.0, 9.0, true, 0.8), (3, 1.0, 1.0, true, 0.4), (3, 1.0, 9.0, true, 0.4), (4, 2.0, 5.0, true, 0.4), (4, 8.0, 5.0, true, 0.4)],
        "",
    );
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let conns = b.connections(&m);
    let mut st = State::new(&b.proj);
    let w0 = topor_engine::topo::init::InitWeights::default();
    st.route_conn(conns.iter().find(|c| c.net == 3).unwrap(), &w0, 1.0).unwrap();
    let x = b.proj.obstacles.iter().position(|o| o.net == Some(5) && (o.center.x - 5.0 * MM).abs() < 1.0).unwrap();
    let c4 = conns.iter().find(|c| c.net == 4).unwrap();
    let (path, tris) = st.search(c4, &w0, 1.0).unwrap();
    let t_star = *tris.iter().find(|t| st.ring[x].contains(t)).expect("the path passes the pad's ring");
    for &t in st.ring[x].clone().iter().filter(|&&t| t != t_star) {
        st.tri_trav[t].push((0, 0));
    }
    let wire = Wire { net: 4, w: c4.w, s: c4.s, a: c4.a, b: c4.b, pa: c4.pa, pb: c4.pb, turns: Vec::new(), passes: Vec::new(), tris: Vec::new(), alive: true };
    let mv = Move::Insert { wire: &wire, path: &path, tris: &tris, scale: 1.0 };
    let evs = events(&st, &mv);
    let blocks: Vec<&Event> = evs.iter().filter(|e| matches!(e, Event::BlocksPad { .. })).collect();
    assert_eq!(blocks.len(), 1, "{evs:?}");
    let price = |w: f64| Registry::from_config(standard_factors(), &cfg(&[("blocks_pad", w)])).0.cost(&st, &mv, &WireCtx { net: 4, tier: 3 });
    assert!((price(30.0) - price(0.0) - 30.0).abs() < 1e-9, "the pad adds exactly its price");
}
