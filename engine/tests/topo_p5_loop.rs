//! Shopping list 5.2 (the TopoR loop's single-wire rip-up and reroute, Polubasov 2010
//! «Оптимизация разводки»: "removing the wires one at a time and laying them again along cheaper
//! paths"), on a fixed topology: a kept reroute lowers the wire's price by more than the
//! `reroute` price; a reroute that does not pay leaves the state exactly as it was; a connection
//! that cannot be placed again never leaves its wire out.

use topor_engine::geom::Vec2;
use topor_engine::model::BoardModel;
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::factors::{events, Event, Move, Registry, WireCtx};
use topor_engine::topo::init::{route_all_once, Conn, InitWeights};
use topor_engine::topo::layers::{self, LayerParams};
use topor_engine::topo::optimise::{reroute_set, Rerouter};
use topor_engine::topo::state::{State, Wire, WireId};

const MM: f64 = 1e6;
const WS0: f64 = 450_000.0;

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

/// The price of wire `w` where it lies: its insert events with itself taken out (own and social
/// factors), as the P4 moves read it.
fn price(st: &mut State, reg: &Registry, w: WireId) -> f64 {
    let path = st.detach(w);
    let wire = st.wires[w].clone();
    let scale = (wire.w + wire.s) / WS0;
    let p: f64 = events(st, &Move::Insert { wire: &wire, path: &path, tris: &wire.tris, scale }).iter().map(|e| reg.price(e, &WireCtx { net: wire.net, tier: 3 })).sum();
    st.reattach(w, &path);
    p
}

/// Places the connection of the removed wire with the full search (or not at all, `fail`).
struct Full<'c> {
    conns: &'c [Conn],
    reg: Registry,
    fail: bool,
    undone: usize,
}

impl<'p> Rerouter<'p> for Full<'_> {
    fn price(&mut self, st: &mut State<'p>, w: WireId) -> f64 {
        price(st, &self.reg, w)
    }
    fn place(&mut self, st: &mut State<'p>, gone: &[WireId]) -> Option<Vec<WireId>> {
        if self.fail {
            return None;
        }
        let mut out = Vec::new();
        for &g in gone {
            let net = st.wires[g].net;
            let c = self.conns.iter().find(|c| c.net == net)?;
            let wt = InitWeights { max_expansions: 2_000_000, ..InitWeights::from_registry(&self.reg, &WireCtx { net, tier: 3 }) };
            out.push(st.route_conn(c, &wt, (c.w + c.s) / WS0)?.0);
        }
        Some(out)
    }
    fn undo(&mut self) {
        self.undone += 1;
    }
}

fn alive_of(st: &State, net: i32) -> WireId {
    st.wires.iter().position(|w| w.alive && w.net == net).unwrap()
}

/// Net 1 runs 16 mm from (2, 5) to (18, 5); net 2 stands in the way (pads at (10, 8) and (10, 2)).
/// With a search capped at one expansion both wires are forced through any sequence of triangles
/// (4.3 never fail); the reroute of net 1 with the full search keeps a path that is cheaper by
/// more than the `reroute` price, and the price it reports is the price of the wire now in the
/// state. A second reroute of the same wire finds nothing cheaper than its own path plus the
/// reroute price: the state is back bit for bit.
#[test]
fn a_kept_reroute_pays_and_a_refused_one_leaves_the_state_as_it_was() {
    let m = board((0.0, 0.0, 20.0, 10.0), &[(1, 2.0, 5.0, true, 1.0), (1, 18.0, 5.0, true, 1.0), (2, 10.0, 8.0, true, 1.0), (2, 10.0, 2.0, true, 1.0)]);
    let reg = Registry::standard();
    let reroute_mm = reg.price(&Event::Rerouted { wire: 0, count: 1 }, &WireCtx::default());
    assert_eq!(reroute_mm, 5.0, "the reroute factor's default: 5 mm per earlier reroute");
    for steiner in [false, true] {
        let b = board::build(&m, &opts(steiner)).unwrap();
        let conns = b.connections(&m);
        let mut st = State::new(&b.proj);
        st.taut_len = true;
        let rep = route_all_once(&mut st, &conns, &reg, 1, WS0);
        assert!(rep.failed.is_empty() && !rep.forced.is_empty(), "steiner {steiner}: {rep:?}");
        let w1 = alive_of(&st, 1);
        let before = price(&mut st, &reg, w1);
        let mut r = Full { conns: &conns, reg: reg.clone(), fail: false, undone: 0 };
        let out = reroute_set(&mut st, &[w1], &mut r, reroute_mm);
        assert!(out.kept, "steiner {steiner}: {out:?}");
        assert!((out.old - before).abs() < 1e-9);
        assert!(out.new + reroute_mm < out.old, "steiner {steiner}: the kept move pays: {out:?}");
        let w1b = alive_of(&st, 1);
        assert!(w1b != w1 && !st.wires[w1].alive, "the old wire is gone, a new one placed");
        assert!((price(&mut st, &reg, w1b) - out.new).abs() < 1e-9, "the reported price is the state's");
        // The straight line crosses net 2's wire; going round costs at most the 16 mm line plus one
        // crossing (20 mm), and never less than the line (length priced per mm of (w + s) / (w0 + s0)).
        let len = st.wire_len(w1b) / MM;
        let scale = (st.wires[w1b].w + st.wires[w1b].s) / WS0;
        assert!((16.0 * scale - 1e-6..=16.0 * scale + 20.0).contains(&out.new) && len >= 16.0 - 1e-6, "steiner {steiner}: price {} length {len} scale {scale}", out.new);
        assert!(st.check().is_ok(), "{:?}", st.check());
        // Again: the same search finds the same path; with the reroute price on top it does not pay.
        let snap = format!("{:?}", st.topology());
        let out2 = reroute_set(&mut st, &[w1b], &mut r, reroute_mm);
        assert!(!out2.kept, "steiner {steiner}: {out2:?}");
        assert!((out2.new - out2.old).abs() < 1e-6, "the same path again: {out2:?}");
        assert_eq!(format!("{:?}", st.topology()), snap, "steiner {steiner}: a refused reroute leaves the state as it was");
        assert_eq!(r.undone, 1);
        // A connection that cannot be placed again: refused, the old wire stays.
        r.fail = true;
        let out3 = reroute_set(&mut st, &[w1b], &mut r, 0.0);
        assert!(!out3.kept && out3.new.is_infinite());
        assert_eq!(format!("{:?}", st.topology()), snap, "never leaves a wire out");
        assert!(st.wires[w1b].alive);
    }
}

/// The layer re-optimisation's random choice among equal layerings (Polubasov 2010, «Оптимизация
/// расслоения»): two wires that cross once need one via in either orientation; every tie seed
/// gives a layering with the same via count as the deterministic one, and the seeds do not all
/// give the same one.
#[test]
fn random_ties_keep_the_via_count_and_vary_the_layering() {
    let m = board((0.0, 0.0, 20.0, 20.0), &[(1, 2.0, 10.0, true, 1.0), (1, 18.0, 10.0, true, 1.0), (2, 10.0, 2.0, true, 1.0), (2, 10.0, 18.0, true, 1.0), (3, 4.0, 4.0, true, 1.0), (3, 16.0, 16.0, true, 1.0)]);
    let reg = Registry::standard();
    let b = board::build(&m, &opts(false)).unwrap();
    let conns = b.connections(&m);
    let mut st = State::new(&b.proj);
    // Force the crossings: route without uncrossing preferences (plain search, straight lines).
    for c in &conns {
        let wt = InitWeights { max_expansions: 2_000_000, cross_mm: 0.0, ..InitWeights::from_registry(&reg, &WireCtx { net: c.net, tier: 3 }) };
        let (path, tris) = st.search(c, &wt, (c.w + c.s) / WS0).expect("a path");
        let wire = Wire { net: c.net, w: c.w, s: c.s, a: c.a, b: c.b, pa: c.pa, pb: c.pb, turns: Vec::new(), passes: Vec::new(), tris: Vec::new(), alive: true };
        st.insert(wire, &path, tris);
    }
    assert!(st.total_crossings() >= 2, "the three straight lines cross: {}", st.total_crossings());
    let smd = |_: usize| None;
    let lp = LayerParams::default();
    let base = layers::assign(&st, &smd, &lp);
    let mut labels = std::collections::BTreeSet::new();
    for ties in 1..=16u64 {
        let r = layers::assign(&st, &smd, &LayerParams { ties, ..lp });
        assert_eq!(r.vias, base.vias, "ties {ties}: an equal optimum");
        assert_eq!(r.same_layer, 0);
        labels.insert(format!("{:?}", r.events));
    }
    assert!(labels.len() > 1, "the seeds choose among the equal layerings");
    // ties = 0 is the deterministic layering of before.
    assert_eq!(format!("{:?}", layers::assign(&st, &smd, &LayerParams { ties: 0, ..lp }).events), format!("{:?}", base.events));
    let _ = Vec2::default();
}
