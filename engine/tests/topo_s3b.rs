//! S3b learning loop: factor totals, loss and the subgradient loop on tiny known boards.

use topor_engine::model::BoardModel;
use topor_engine::topo::factors::{standard_factors, FactorConfig, Registry};
use topor_engine::topo::learn::{
    engine_layer_masks, hand_stretches, learn, learn_loop, loss, prepare, reduce, route_lite, state_totals, toml_snippet, LearnOpts, Words,
};
use topor_engine::topo::witness::{run, sub_state, Spots, WitnessOpts};

const MM: f64 = 1e6;

fn p(v: f64) -> i64 {
    (v * MM) as i64
}

/// 10 x 10 mm two-layer board, through-hole pads `(net, x, y)` of 1 mm, tracks `(net, layer id
/// 0 = F / 2 = B, a, b)` 0.25 mm wide, vias `(net, x, y)`. KiCad frame, mm.
/// A track: net, layer id, from, to (mm).
type Track = (i32, u32, (f64, f64), (f64, f64));

fn board(pads: &[(i32, f64, f64)], tracks: &[Track], vias: &[(i32, f64, f64)]) -> BoardModel {
    let c = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let outline: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y))| {
            let sh = format!(r#"{{"kind":"rect","w":{0},"h":{0}}}"#, p(1.0));
            format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"pth","center":[{},{}],"layers":[0,2],"shapes":{{"0":{sh},"2":{sh}}},"drill":{{"w":{d},"h":{d}}}}}"#,
                p(x),
                p(y),
                d = p(0.5)
            )
        })
        .collect();
    let kept: Vec<String> = tracks
        .iter()
        .enumerate()
        .map(|(i, &(net, layer, a, b))| {
            format!(
                r#"{{"uuid":"k{i}","net":{net},"layer":{layer},"width":{},"prim":{{"kind":"seg","a":[{},{}],"b":[{},{}]}}}}"#,
                p(0.25),
                p(a.0),
                p(a.1),
                p(b.0),
                p(b.1)
            )
        })
        .collect();
    let vias: Vec<String> = vias
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y))| format!(r#"{{"uuid":"v{i}","net":{net},"at":[{},{}],"d":450000,"drill":300000,"layers":[0,2]}}"#, p(x), p(y)))
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}}],
            "pads":[{}],"vias":[{}],"kept":[{}],"outline":[{}],"rules":{{}}}}"#,
        pads.join(","),
        vias.join(","),
        kept.join(","),
        outline.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

/// A crosses the board left to right between B's two pads (a gap of 1.4 mm, no Steiner point in
/// it), B joins its pads; the two must cross once.
const PADS: [(i32, f64, f64); 4] = [(1, 2.0, 5.0), (1, 8.0, 5.0), (2, 5.0, 3.8), (2, 5.0, 6.2)];

fn hand_plain() -> BoardModel {
    board(&PADS, &[(1, 0, (2.0, 5.0), (8.0, 5.0)), (2, 2, (5.0, 3.8), (5.0, 6.2))], &[])
}

/// B starts on F, drops to B.Cu through a via at (5, 4.5) and crosses A there.
fn hand_via() -> BoardModel {
    board(&PADS, &[(1, 0, (2.0, 5.0), (8.0, 5.0)), (2, 0, (5.0, 3.8), (5.0, 4.5)), (2, 2, (5.0, 4.5), (5.0, 6.2))], &[(2, 5.0, 4.5)])
}

fn input() -> BoardModel {
    board(&PADS, &[], &[])
}

fn idx(name: &str) -> usize {
    standard_factors().iter().position(|f| f.name() == name).unwrap()
}

#[test]
fn hand_totals_are_known() {
    for (m, vias) in [(hand_plain(), 0.0), (hand_via(), 1.0)] {
        let s = prepare("t", &m, &input(), None).unwrap();
        let t = &s.hand.totals;
        assert_eq!(t.wires, 2, "{t:?}");
        assert_eq!(t.amount[idx("crossing")], 1.0, "{t:?}");
        assert_eq!(t.amount[idx("same_net_crossing")], 0.0);
        assert_eq!(t.events["vias"], vias, "{t:?}");
        assert_eq!(t.amount[idx("via")], vias + t.events["layer_forced"]);
        // The State polyline runs through the gap points: at least the straight 6 + 2.4 mm, and
        // less than 1.5 times that on this open board.
        let len = t.amount[idx("length")];
        assert!((8.4..12.6).contains(&len), "length {len}");
        assert_eq!(t.amount[idx("overflow")], 0.0);
        assert_eq!(t.amount[idx("blocks_pad")], 0.0);
    }
}

#[test]
fn hand_loss_against_itself() {
    let m = hand_plain();
    let w = run(&m, None, &Spots::default(), &WitnessOpts::default()).unwrap();
    let (st, _, _) = sub_state(&w.board, &m, &w.chains, &w.walked);
    let words = Words::new(&w.board.proj);
    let hs = hand_stretches(&words, &w, &st);
    // A passes through the cut between B's pads: its word is that cut. B's way from pad to pad
    // passes no cut of another net: its word is empty and it does not count.
    let counted: Vec<_> = hs.iter().filter(|h| !h.edges.is_empty()).collect();
    assert_eq!(counted.len(), 1, "{hs:?}");
    assert_eq!(counted[0].net, 1);
    // Each pass on its own hand layer: everything reproduced.
    let own: Vec<Vec<u64>> = (0..st.wires.len())
        .map(|wi| {
            let l = hs.iter().find(|h| h.net == st.wires[wi].net).unwrap().layer;
            vec![1u64 << l; st.wires[wi].passes.len()]
        })
        .collect();
    assert_eq!(loss(&words, &hs, &st, &own, &|_| true), (0.0, 1, 1));
    // Every pass on the other layer: nothing reproduced.
    let other: Vec<Vec<u64>> = own.iter().map(|v| v.iter().map(|b| b ^ 3).collect()).collect();
    assert_eq!(loss(&words, &hs, &st, &other, &|_| true), (1.0, 0, 1));
    // A net that does not count.
    assert_eq!(loss(&words, &hs, &st, &own, &|n| n == 2), (0.0, 0, 0));
}

#[test]
fn words_tell_the_sides_of_an_island() {
    // A goes around the island pad of net 2 at (5, 5) once above and once below it.
    let pads = [(1, 2.0, 5.0), (1, 8.0, 5.0), (2, 5.0, 5.0), (2, 5.0, 9.0)];
    let above = board(&pads, &[(1, 0, (2.0, 5.0), (5.0, 3.0)), (1, 0, (5.0, 3.0), (8.0, 5.0))], &[]);
    let below = board(&pads, &[(1, 0, (2.0, 5.0), (5.0, 7.0)), (1, 0, (5.0, 7.0), (8.0, 5.0))], &[]);
    let word = |m: &BoardModel| {
        let w = run(m, None, &Spots::default(), &WitnessOpts::default()).unwrap();
        let (st, _, _) = sub_state(&w.board, m, &w.chains, &w.walked);
        let words = Words::new(&w.board.proj);
        assert!(words.islands(1) >= 1);
        let wid = (0..st.wires.len()).find(|&i| st.wires[i].net == 1).unwrap();
        reduce(words.wire(&st, wid)).into_iter().map(|x| x.0).collect::<Vec<i64>>()
    };
    let (a, b) = (word(&above), word(&below));
    assert_ne!(a, b, "above {a:?} below {b:?}");
}

#[test]
fn engine_totals_and_loss_on_the_witness_board() {
    let s = prepare("t", &hand_plain(), &input(), None).unwrap();
    let base = Registry::standard();
    let r = route_lite(&s, &base, &base, None, 0.0);
    assert_eq!(r.routed, 2);
    assert_eq!(r.failed, 0);
    // At the default crossing price (20 mm) A goes around one of B's pads (a few mm longer)
    // instead of crossing B: no crossing, no via, and A's hand stretch (through the cut between
    // B's pads) is not reproduced.
    assert_eq!(r.totals.amount[idx("crossing")], 0.0, "{r:?}");
    assert_eq!(r.vias, 0);
    assert_eq!(r.stretches, 1);
    assert_eq!(r.loss, 1.0);
    assert!(r.totals.events["excess_length_mm"] > 1.0, "{r:?}");
    // At a crossing price of 1 mm A goes straight through, like the hand: one crossing, layered
    // without a via; whether A sits on its hand layer is P3's free choice of orientation.
    let cheap = Registry::from_config(
        standard_factors(),
        &FactorConfig { weight: [("crossing".to_string(), 1.0)].into_iter().collect(), ..FactorConfig::default() },
    )
    .0;
    let c = route_lite(&s, &cheap, &base, None, 0.0);
    assert_eq!(c.totals.amount[idx("crossing")], 1.0, "{c:?}");
    assert_eq!(c.vias, 0);
    assert_eq!(c.same_layer, 0);
    assert!(c.loss == 0.0 || c.loss == 1.0, "{}", c.loss);
    // Loss augmentation: the hand's edges cost 50 mm more per pass, so even the cheap crossing
    // routing leaves the hand's way.
    let a = route_lite(&s, &cheap, &base, None, 50.0);
    assert_eq!(a.totals.amount[idx("crossing")], 0.0, "{a:?}");
    assert_eq!(a.loss, 1.0);
}

#[test]
fn engine_layer_masks_follow_the_events() {
    let s = prepare("t", &hand_plain(), &input(), None).unwrap();
    let w = &s.w;
    let (st, _, _) = sub_state(&w.board, &hand_plain(), &w.chains, &w.walked);
    let lr = topor_engine::topo::layers::LayerReport {
        events: vec![vec![(1, 0.5, 0)], vec![(0, 0.5, 1), (2, 0.5, 0)]],
        ..Default::default()
    };
    let m = engine_layer_masks(&st, &lr);
    // Wire 0: one event, every pass on its layer.
    assert!(m[0].iter().all(|&x| x == 1), "{m:?}");
    // Wire 1: passes 0 and 1 lie between the events (labels 1 and 0): either layer; from pass
    // 2 on, after the last event, layer 0.
    assert!(m[1].iter().take(2).all(|&x| x == 3), "{m:?}");
    assert!(m[1].iter().skip(2).all(|&x| x == 1), "{m:?}");
    let t = state_totals(&st, &[0, 0], s.ws0, &|_| true);
    assert_eq!(t.wires, 2);
}

#[test]
fn subgradient_step_signs_projection_and_stop() {
    let o = LearnOpts { learn: vec!["via".into(), "crossing".into()], lambda: 0.0, eta: 4.0, iters: 5, eps: 1e-9, ..LearnOpts::default() };
    let n = standard_factors().len();
    let mut hand = vec![0.0; n];
    hand[idx("via")] = 10.0;
    hand[idx("crossing")] = 0.0;
    // Engine: fewer vias than the hand (the via price is too high), many more crossings (too low).
    let route = |_: &Registry| {
        let mut e = vec![0.0; n];
        e[idx("via")] = 5.0;
        e[idx("crossing")] = 30.0;
        vec![(e, 0.5)]
    };
    let r = learn_loop(&[(hand.clone(), 10)], &route, &o).unwrap();
    // Per wire: via (10 - 5) / 10 = 0.5 -> down by eta_t * 0.5; crossing (0 - 30) / 10 = -3 -> up.
    let it0 = &r.iterations[0];
    assert_eq!(it0.grad["via"], 0.5);
    assert_eq!(it0.grad["crossing"], -3.0);
    assert_eq!(r.iterations[1].weights["via"], 25.0 - 4.0 * 0.5);
    assert_eq!(r.iterations[1].weights["crossing"], 20.0 + 4.0 * 3.0);
    assert!(r.last["crossing"] > 20.0 && r.last["via"] < 25.0);
    // Projection: a large via surplus in the hand drives the via weight to 0, never below.
    hand[idx("via")] = 1e6;
    let r2 = learn_loop(&[(hand, 10)], &route, &o).unwrap();
    assert_eq!(r2.last["via"], 0.0);
    // No difference and no regularizer: the first step is 0, the defaults come back.
    let same = |_: &Registry| vec![(vec![0.0; n], 0.0)];
    let r3 = learn_loop(&[(vec![0.0; n], 10)], &same, &o).unwrap();
    assert!(r3.converged);
    assert_eq!(r3.iterations.len(), 1);
    assert_eq!(r3.weights["via"], 25.0);
    // Unknown factor names are refused.
    assert!(learn_loop(&[(vec![0.0; n], 1)], &same, &LearnOpts { learn: vec!["nope".into()], ..o }).is_err());
}

#[test]
fn learning_is_deterministic() {
    let o = LearnOpts { iters: 3, ..LearnOpts::default() };
    let go = || {
        let s = vec![prepare("a", &hand_plain(), &input(), None).unwrap(), prepare("b", &hand_via(), &input(), None).unwrap()];
        learn(&s, &o).unwrap()
    };
    let (a, b) = (go(), go());
    assert_eq!(a.weights, b.weights);
    assert_eq!(a.last, b.last);
    assert_eq!(a.iterations.len(), b.iterations.len());
}

#[test]
fn snippet_is_the_factors_weight_format() {
    let w = [("crossing".to_string(), 12.5), ("via".to_string(), 20.0)].into_iter().collect();
    let s = toml_snippet(&w, None, "test");
    assert!(s.contains("[factors]\nweight = { crossing = 12.500, via = 20.000 }\n"), "{s}");
    let t = toml_snippet(&w, Some(2), "test");
    assert!(!t.lines().any(|l| l.starts_with("weight") || l.starts_with("[factors]")), "{t}");
}
