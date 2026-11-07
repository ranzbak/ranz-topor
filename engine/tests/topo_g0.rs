//! G0 flat-plane gate: the domain-graph solver against brute force, and known-answer boards.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use topor_engine::model::BoardModel;
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::init::{route_all, InitWeights};
use topor_engine::topo::layers::{assign, brute_force, edge_cost, DomainGraph, LayerParams, LayerReport};
use topor_engine::topo::state::State;

const MM: f64 = 1e6;

#[test]
fn reductions_keep_the_optimum() {
    let mut rng = ChaCha8Rng::seed_from_u64(7);
    let mut full = 0;
    for case in 0..600 {
        let n = rng.gen_range(2..=12);
        let m = rng.gen_range(1..=3 * n);
        let edges: Vec<(usize, usize, f64)> = (0..m)
            .map(|_| {
                let w = 25.0 * rng.gen_range(1..=3) as f64 * if rng.gen_bool(0.5) { 1.0 } else { -1.0 };
                (rng.gen_range(0..n), rng.gen_range(0..n), w)
            })
            .collect();
        let mut g = DomainGraph::new(n);
        for &(u, v, w) in &edges {
            g.add_edge(u, v, w);
        }
        let mut reduced = g.clone();
        reduced.reduce();
        let o = g.solve(0, 1, 1); // exact_max 0: whatever is left goes to local search
        assert_eq!(o[0], 0, "node 0 is fixed");
        let got: f64 = edges.iter().map(|&(u, v, w)| edge_cost(w, o[u] != o[v])).sum();
        let want = brute_force(n, &edges);
        if reduced.size().edges == 0 {
            full += 1;
            assert!((got - want).abs() < 1e-6, "case {case}: {got} != optimum {want}");
            assert!((reduced.k - want).abs() < 1e-6, "case {case}: K {} != {want}", reduced.k);
        } else {
            assert!(got >= want - 1e-6, "case {case}: below the optimum");
        }
    }
    assert!(full > 400, "only {full} of 600 graphs reduce completely");
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

/// P2 + P3 on `m`: (crossings after uncrossing, layer report).
fn g0(m: &BoardModel) -> (usize, LayerReport) {
    let b = board::build(m, &BuildOpts::default()).unwrap();
    let conns = b.connections(m);
    let mut st = State::new(&b.proj);
    let rep = route_all(&mut st, &conns, &InitWeights::default(), 400_000.0);
    assert!(rep.failed.is_empty(), "every connection routes");
    st.check().unwrap();
    let lr = assign(&st, &|o| b.smd_layer(o), &LayerParams::default());
    (rep.crossings_after, lr)
}

/// Net 1 left-right, net 2 top-bottom; every pad 0.1 mm from the edge, so neither wire can
/// pass around the other's pad.
fn cross(th: bool) -> BoardModel {
    board(
        (0.0, 0.0, 10.0, 10.0),
        &[(1, 0.5, 5.0, th, 0.8), (1, 9.5, 5.0, th, 0.8), (2, 5.0, 0.5, th, 0.8), (2, 5.0, 9.5, th, 0.8)],
        "",
    )
}

#[test]
fn crossing_with_through_hole_ends_needs_no_via() {
    let (crossings, lr) = g0(&cross(true));
    assert_eq!(crossings, 1);
    assert_eq!(lr.vias, 0, "a TH end changes layer for free");
    assert!(lr.unlayerable.is_empty());
    assert_eq!(lr.same_layer, 0);
}

#[test]
fn crossing_with_smd_ends_needs_exactly_two_vias() {
    let (_, lr) = g0(&cross(false));
    // All four pads on F: one wire leaves F and comes back.
    assert_eq!(lr.vias, 2);
    assert!(lr.unlayerable.is_empty());
    assert_eq!(lr.same_layer, 0);
}

#[test]
fn via_slot_lies_between_its_events() {
    let (_, lr) = g0(&cross(false));
    assert_eq!(lr.slots.len(), 2);
    let le = |a: (usize, f64), b: (usize, f64)| a.0 < b.0 || (a.0 == b.0 && a.1 <= b.1);
    for s in &lr.slots {
        assert!(le(s.from, s.at) && le(s.at, s.to) && s.from != s.to, "slot {s:?}");
    }
}

/// TopoR black-box case E7b: net 1 runs left-right at y = 2; net 2 (wider, routed first) hangs
/// from a pad on the top edge down to a pad at y = 2 + `d`. Going around that pad costs about
/// 2·√(25 + d²) − 10 mm; a crossing costs 20 mm.
fn e7b(d: f64) -> usize {
    let rule = r#""2":{"clearance":200000,"width":500000,"via_d":600000,"via_drill":300000,
                 "netclass":"W","width_source":"netclass","min_width":0}"#;
    let m = board(
        (-3.0, 0.0, 13.0, 7.0 + d),
        &[(1, 0.0, 2.0, true, 1.0), (1, 10.0, 2.0, true, 1.0), (2, 5.0, 0.0, true, 2.0), (2, 5.0, 2.0 + d, true, 1.0)],
        rule,
    );
    g0(&m).0
}

#[test]
fn short_detour_beats_a_crossing() {
    assert_eq!(e7b(4.0), 0, "≈ 2.8 mm detour");
}

#[test]
fn long_detour_loses_to_a_crossing() {
    assert_eq!(e7b(25.0), 1, "≈ 41 mm detour");
}

/// LCT cuts: two SMD squares (1 mm) with a 0.6 mm gap, offset by half a side, face each other
/// with a corner against the other's side. A plain CDT only has the corner-to-corner diagonals
/// (0.78 mm); the refinement adds the cut edge corner-to-foot of exactly the gap (0.6 mm).
#[test]
fn lct_cut_is_an_edge_of_the_gap() {
    let m = board((0.0, 0.0, 12.0, 12.0), &[(1, 5.0, 5.0, false, 1.0), (2, 5.5, 6.6, false, 1.0)], "");
    let gap = 0.6 * MM;
    let has_gap_edge = |o: &BuildOpts| {
        let b = board::build(&m, o).unwrap();
        let pr = &b.proj;
        pr.edges.iter().enumerate().any(|(e, &[p, q])| {
            let (a, c) = (pr.pos[p], pr.pos[q]);
            matches!((pr.vkind[p], pr.vkind[q]), (topor_engine::topo::state::VertKind::Outline(x), topor_engine::topo::state::VertKind::Outline(y)) if x != y)
                && ((a - c).len() - gap).abs() < 100.0
                && pr.room[e][0].is_finite()
        })
    };
    assert!(has_gap_edge(&BuildOpts::default()), "the corner-to-foot cut is an edge");
    assert!(!has_gap_edge(&BuildOpts { lct_mm: 0.0, ..BuildOpts::default() }), "without the refinement it is not");
}

// ---------------------------------------------------------------- factors

use topor_engine::topo::factors::{events, standard_factors, Event, FactorConfig, Move, Registry, WireCtx};
use topor_engine::topo::init::route_all_reg;
use topor_engine::topo::state::{LayerMask, Wire};

fn cfg(weights: &[(&str, f64)]) -> FactorConfig {
    FactorConfig { weight: weights.iter().map(|&(n, w)| (n.to_string(), w)).collect(), ..FactorConfig::default() }
}

/// Crossings after P2 on the E7b board with the given factor weights.
fn e7b_with(d: f64, weights: &[(&str, f64)]) -> usize {
    let rule = r#""2":{"clearance":200000,"width":500000,"via_d":600000,"via_drill":300000,
                 "netclass":"W","width_source":"netclass","min_width":0}"#;
    let m = board(
        (-3.0, 0.0, 13.0, 7.0 + d),
        &[(1, 0.0, 2.0, true, 1.0), (1, 10.0, 2.0, true, 1.0), (2, 5.0, 0.0, true, 2.0), (2, 5.0, 2.0 + d, true, 1.0)],
        rule,
    );
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let conns = b.connections(&m);
    let mut st = State::new(&b.proj);
    let (reg, problems) = Registry::from_config(standard_factors(), &cfg(weights));
    assert!(problems.is_empty(), "{problems:?}");
    route_all_reg(&mut st, &conns, &reg, 2_000_000, 400_000.0);
    st.total_crossings()
}

#[test]
fn p2_takes_its_prices_from_the_factors() {
    // The short detour wins at the default crossing price (20 mm) ...
    assert_eq!(e7b_with(4.0, &[]), 0);
    // ... and a free crossing wins over any detour.
    assert_eq!(e7b_with(4.0, &[("crossing", 0.0)]), 1);
    // A dear crossing flips the long case: the 41 mm detour beats 100 mm.
    assert_eq!(e7b_with(25.0, &[("crossing", 100.0)]), 0);
    // The resolved P2 costs are the factor weights.
    let (reg, _) = Registry::from_config(standard_factors(), &cfg(&[("crossing", 7.0), ("via", 11.0), ("length", 2.0)]));
    let w = InitWeights::from_registry(&reg, &WireCtx::default());
    assert_eq!((w.cross_mm, w.forced_layer_mm, w.smd_over_mm, w.length_mm, w.overflow_mm), (7.0, 11.0, 11.0, 2.0, 50.0));
}

#[test]
fn an_insert_move_is_priced_from_its_events() {
    let m = cross(true);
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let conns = b.connections(&m);
    let mut st = State::new(&b.proj);
    let wt = InitWeights::default();
    let (first, second) = (&conns[0], &conns[1]);
    st.route_conn(first, &wt, 1.0).unwrap();
    let (path, tris) = st.search(second, &wt, 1.0).unwrap();
    let wire = Wire {
        net: second.net,
        w: second.w,
        s: second.s,
        a: second.a,
        b: second.b,
        pa: second.pa,
        pb: second.pb,
        turns: Vec::new(),
        passes: Vec::new(),
        tris: Vec::new(),
        alive: true,
    };
    let mv = Move::Insert { wire: &wire, path: &path, tris: &tris, scale: 1.0 };
    let before = st.edge_order.iter().map(Vec::len).sum::<usize>();
    let evs = events(&st, &mv);
    let crossings = evs.iter().filter(|e| matches!(e, Event::Crossing { same_net: false, .. })).count();
    let len_mm: f64 = evs.iter().filter_map(|e| if let Event::Length { mm, .. } = e { Some(*mm) } else { None }).sum();
    assert_eq!(crossings, 1, "{evs:?}");
    assert!((len_mm - 9.0).abs() < 1.0, "pad to pad is 9 mm (gap points zigzag a little): {len_mm}");
    let reg = Registry::standard();
    let cost = reg.cost(&st, &mv, &WireCtx::default());
    assert!((cost - (len_mm + 20.0)).abs() < 1e-6, "length + one crossing: {cost}");
    assert_eq!(before, st.edge_order.iter().map(Vec::len).sum::<usize>(), "evaluation does not touch the state");
}

#[test]
fn state_event_queries() {
    // Net 3 runs top to bottom between a pad of net 1 and a pad of net 2.
    let m = board((0.0, 0.0, 10.0, 10.0), &[(1, 3.0, 5.0, true, 0.8), (2, 7.0, 5.0, true, 0.8), (3, 5.0, 1.0, true, 0.8), (3, 5.0, 9.0, true, 0.8)], "");
    let b = board::build(&m, &BuildOpts { steiner_mm: 0.0, lct_mm: 4.0, ..BuildOpts::default() }).unwrap();
    let conns = b.connections(&m);
    let mut st = State::new(&b.proj);
    let wt = InitWeights::default();
    for c in &conns {
        st.route_conn(c, &wt, 1.0).unwrap();
    }
    let obs_at = |x: f64| b.proj.obstacles.iter().position(|o| (o.center.x - x * MM).abs() < 1.0).unwrap();
    let (o1, o2) = (obs_at(3.0), obs_at(7.0));
    let between = st.passes_between(o1, o2);
    assert!(!between.is_empty() && between.iter().all(|&p| st.wires[st.passes[p].wire].net == 3), "the wire of net 3 runs between the pads: {between:?}");
    let e = st.passes[between[0]].edge;
    let all = LayerMask::all(2);
    assert!(b.proj.room_of(e, 0).is_finite() && st.room(e, 0) == b.proj.room[e][0], "the pair edge is a cut");
    let d = st.demand(e, all);
    assert!(d > 0.0 && d >= m.rules.default.width as f64, "{d}");
    assert_eq!(st.demand(e, LayerMask(0)), 0.0, "no pass is restricted to no layer");
    // Through-hole ends allow both layers: no pass is restricted to one layer alone.
    assert_eq!(st.demand(e, LayerMask(1)), 0.0);
}

#[test]
fn the_factor_reference_matches_the_schema_text() {
    // plugin/topor/schema.py [factors] weight lists the factors and defaults; keep both in step.
    let r = Registry::standard();
    let got: Vec<(&str, f64)> = r.reference().iter().map(|x| (x.0, x.2)).collect();
    let want = [
        ("length", 1.0),
        ("via", 25.0),
        ("crossing", 20.0),
        ("same_net_crossing", 0.0),
        ("plane_signal", 10.0),
        ("plane_cut", 25.0),
        ("necking", 30.0),
        ("overflow", 50.0),
        ("congestion", 10.0),
        ("squeeze", 5.0),
        ("via_squeeze", 5.0),
        ("blocks_pad", 30.0),
        ("ignored_rule", 100.0),
        ("reroute", 5.0),
        ("branch", 1.0),
        ("smd_through", 0.0),
        ("junction_angle", 10.0),
        ("any_angle", 15.0),
        ("acute_bend", 50.0),
        ("violation", 1000.0),
        ("dp_uncoupled", 1.0),
        ("dp_skew", 10.0),
        ("dp_gap_deviation", 5.0),
        ("dp_via_unpaired", 25.0),
        ("dp_polarity_cross", 1000.0),
    ];
    assert_eq!(got, want);
}

// ---------------------------------------------------------------- metrization

use topor_engine::topo::metrize::{Leg, Metrizer};

/// A wire between two pads with room to spare fits and is drawn; squeezed between two pads that
/// leave less than its width plus clearances, P1 says it does not fit and the drawing fails.
#[test]
fn metrization_agrees_with_p1_on_a_free_and_a_pinched_passage() {
    let run = |gap: f64| {
        // Net 3 runs top to bottom between a pad of net 1 and a pad of net 2 (gap mm apart).
        let m = board((0.0, 0.0, 10.0, 10.0), &[(1, 5.0 - gap / 2.0 - 0.4, 5.0, false, 0.8), (2, 5.0 + gap / 2.0 + 0.4, 5.0, false, 0.8), (3, 5.0, 1.0, false, 0.8), (3, 5.0, 9.0, false, 0.8)], "");
        let b = board::build(&m, &BuildOpts { steiner_mm: 0.0, lct_mm: 4.0, ..BuildOpts::default() }).unwrap();
        let conns = b.connections(&m);
        let mut st = State::new(&b.proj);
        // Force the straight path: a sleeve through the passage, found with crossings and detours free.
        let wt = topor_engine::topo::init::InitWeights { overflow_mm: 0.0, forced_layer_mm: 0.0, smd_over_mm: 0.0, ..InitWeights::default() };
        st.route_conn(&conns[0], &wt, 1.0).unwrap();
        let w = &st.wires[0];
        let leg = Leg {
            net: w.net,
            w: w.w,
            s: w.s,
            layer: 0,
            start: b.proj.pos[w.a],
            goal: b.proj.pos[w.b],
            tris: w.tris.clone(),
            edges: w.passes.iter().map(|&p| st.passes[p].edge).collect(),
        };
        Metrizer::new(&m, b.signal_layers.clone()).outcome(&b.proj, &leg)
    };
    let free = run(3.0);
    assert!(free.fits && free.taut_ok, "{free:?}");
    let pinched = run(0.3);
    assert!(!pinched.fits, "P1: {}", pinched.fit_why);
    assert!(!pinched.taut_ok, "the passage is too narrow to draw: {pinched:?}");
}
