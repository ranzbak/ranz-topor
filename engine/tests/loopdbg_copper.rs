//! A debug env var never changes copper (shopping list A1): `TOPO_LOOPDBG` once did (pinrow sD:
//! copper hash 7229e49b with it, 3451556e without). The same TopoR loop runs with and without the
//! flag and the routed items must be identical. One test, so no other test in the binary reads the
//! env var meanwhile.

use std::hash::{Hash, Hasher};
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route_with;
use topor_engine::topo::board::{self, Board, BuildOpts};
use topor_engine::topo::init::{Conn, InitWeights};
use topor_engine::topo::state::State;

const MM: f64 = 1e6;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A board `w` x `h` mm with `n_layers` copper layers (2, or 4 with In1.Cu and In2.Cu), SMD rect
/// pads `(ref, net, x, y, w, h, layer)` (layer 0 = F.Cu, 2 = B.Cu, 99 = both) and nets
/// 1..=`nets` (as tests/topo_d6b.rs).
fn board_layers(w: f64, h: f64, nets: usize, pads: &[(&str, i32, f64, f64, f64, f64, u32)], n_layers: usize) -> BoardModel {
    let layers = if n_layers == 4 {
        r#"{"id":0,"name":"F.Cu","kind":"signal"},{"id":4,"name":"In1.Cu","kind":"signal"},{"id":6,"name":"In2.Cu","kind":"signal"},{"id":2,"name":"B.Cu","kind":"signal"}"#
    } else {
        r#"{"id":0,"name":"F.Cu","kind":"signal"},{"id":2,"name":"B.Cu","kind":"signal"}"#
    };
    let p = |v: f64| (v * MM) as i64;
    let c = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads: Vec<String> = pads
        .iter()
        .map(|&(r, net, x, y, w, h, l)| {
            let ls: Vec<u32> = if l == 99 { vec![0, 2] } else { vec![l] };
            let shape = format!(r#"{{"kind":"rect","w":{},"h":{}}}"#, p(w), p(h));
            let shapes: Vec<String> = ls.iter().map(|l| format!(r#""{l}":{shape}"#)).collect();
            let ls: Vec<String> = ls.iter().map(|l| l.to_string()).collect();
            format!(
                r#"{{"uuid":"{r}","ref":"{r}","number":"1","net":{net},"attr":"smd","center":[{},{}],"rotation_deg":0.0,
                    "layers":[{}],"shapes":{{{}}}}}"#,
                p(x),
                p(y),
                ls.join(","),
                shapes.join(",")
            )
        })
        .collect();
    let nets: Vec<String> = (1..=nets).map(|i| format!(r#"{{"code":{i},"name":"N{i}"}}"#)).collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{layers}],
            "nets":[{}],
            "pads":[{}],"outline":[{}],
            "rules":{{"default":{{"clearance":200000,"width":250000}},"min_connection":127000,"max_error":5000,"per_net":{{}}}}}}"#,
        nets.join(","),
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn fan_pads() -> Vec<(&'static str, i32, f64, f64, f64, f64, u32)> {
    let mut pads = vec![
        ("A1", 1, 2.0, 6.0, 1.0, 1.0, 0),
        ("A2", 1, 28.0, 6.0, 1.0, 1.0, 0),
        ("C1", 3, 2.0, 14.0, 1.0, 1.0, 2),
        ("C2", 3, 28.0, 14.0, 1.0, 1.0, 0),
        ("E1", 5, 2.0, 10.0, 1.0, 1.0, 0),
        ("E2", 5, 28.0, 11.0, 1.0, 1.0, 2),
        ("B1", 2, 9.0, 1.5, 1.0, 1.0, 2),
        ("B2", 2, 10.0, 18.5, 1.0, 1.0, 2),
        ("F1", 6, 16.0, 1.5, 1.0, 1.0, 0),
        ("F2", 6, 15.0, 18.5, 1.0, 1.0, 2),
        ("G1", 7, 22.0, 1.5, 1.0, 1.0, 0),
        ("G2", 7, 23.0, 18.5, 1.0, 1.0, 0),
        ("D1", 4, 2.0, 7.5, 1.0, 1.0, 0),
        ("D2", 4, 28.0, 7.5, 1.0, 1.0, 0),
    ];
    // A foreign pad on each layer in the middle (no via site there).
    pads.push(("K0", 8, 12.5, 10.0, 2.0, 2.0, 0));
    pads.push(("K1", 8, 19.5, 8.0, 2.0, 2.0, 2));
    pads
}

/// The options of a short TopoR loop on the fan (`extra`: JSON fields).
fn loop_opts(extra: &str) -> Options {
    let mut o: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 3, "runs": 1, "seed": 3}"#).unwrap();
    let more: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&format!("{{{extra}}}")).unwrap();
    o.extend(more);
    Options::from_json(&serde_json::Value::Object(o).to_string()).unwrap()
}

fn copper_hash(m: &BoardModel, o: &Options) -> (u64, usize) {
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let r = route_with(m, o, &ctl);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", r.items).hash(&mut h);
    (h.finish(), r.items.len())
}

#[test]
fn loopdbg_flag_does_not_change_copper() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut pads = fan_pads();
    pads.push(("F3", 6, 26.0, 4.0, 1.0, 1.0, 0));
    let mut bad = Vec::new();
    for n_layers in [2usize] {
        let m = board_layers(30.0, 20.0, 8, &pads, n_layers);
        for seed in 1..=1 {
            let o = loop_opts(&format!(r#""seed": {seed}"#));
            unsafe { std::env::remove_var("TOPO_LOOPDBG") };
            let off = copper_hash(&m, &o);
            let off2 = copper_hash(&m, &o);
            assert_eq!(off, off2, "{n_layers} layers seed {seed}: the run is not deterministic without the flag");
            // SAFETY: ENV_LOCK is held.
            unsafe { std::env::set_var("TOPO_LOOPDBG", "1") };
            let on = copper_hash(&m, &o);
            unsafe { std::env::remove_var("TOPO_LOOPDBG") };
            if on != off {
                bad.push(format!("{n_layers} layers seed {seed}: off {off:?} on {on:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "TOPO_LOOPDBG changed copper: {bad:?}");
}

fn conn(m: &BoardModel, b: &Board, net: i32, ra: &str, rb: &str) -> Conn {
    let ob = |r: &str| {
        let i = m.pads.iter().position(|p| p.ref_ == r).unwrap();
        b.pad_of.iter().position(|&x| x == i).unwrap()
    };
    let (pa, pb) = (ob(ra), ob(rb));
    Conn { net, pa, pb, a: b.centre[pa].unwrap(), b: b.centre[pb].unwrap(), w: 0.25 * MM, s: 0.2 * MM }
}

fn shape(st: &State) -> String {
    format!("{:?}|{:?}|{:?}|{:?}", st.edge_order, st.tri_trav, st.pass_w, st.pass_d)
}

/// The probe `Topo::wire_price_by` (the per-factor prices of `TOPO_LOOPDBG`) takes a wire out and
/// puts it back: `State::probe_detached` leaves the state bit for bit as it was, where the
/// bare detach + reattach does not (the order of the traversals in the wire's triangles changes,
/// and the neck refits of its neighbours take the layering of the moment).
#[test]
fn detach_reattach_probe_restores_the_state() {
    let mut pads = fan_pads();
    pads.push(("F3", 6, 26.0, 4.0, 1.0, 1.0, 0));
    let m = board_layers(30.0, 20.0, 8, &pads, 2);
    let b: &'static Board = Box::leak(Box::new(board::build(&m, &BuildOpts { existing_vias: true, keepouts: true, copper_shapes: true, ..BuildOpts::default() }).unwrap()));
    let mut st = State::new(&b.proj);
    for net in 1..=8 {
        st.floors.insert(net, 0.1 * MM);
    }
    let wt = InitWeights::default();
    for (net, ra, rb) in [(1, "A1", "A2"), (2, "B1", "B2"), (3, "C1", "C2"), (6, "F1", "F2"), (5, "E1", "E2"), (7, "G1", "G2")] {
        st.route_conn(&conn(&m, b, net, ra, rb), &wt, 1.0).expect("routed");
    }
    // The pass widths as a refit under a later layering would leave them: stale until a refit.
    for p in 0..st.pass_w.len() {
        st.pass_w[p] += 1.0;
        st.pass_d[p] += 0.5;
    }
    let (mut bare_changes, mut tt_changed, mut pw_changed, mut pd_changed) = (0, false, false, false);
    for w in (0..st.wires.len()).filter(|&w| st.wires[w].alive).collect::<Vec<_>>() {
        let before = shape(&st);
        let mut bare = st.clone();
        let path = bare.detach(w);
        bare.reattach(w, &path);
        if shape(&bare) != before {
            bare_changes += 1;
        }
        // Each restored field is one the bare detach + reattach really changes (a mutant that
        // drops that restore must die); edge_order is not among them: reattach puts the passes
        // back at their gaps, so it is not snapshotted, only asserted equal below.
        tt_changed |= bare.tri_trav != st.tri_trav;
        pw_changed |= bare.pass_w != st.pass_w;
        pd_changed |= bare.pass_d != st.pass_d;
        let mut probed = st.clone();
        let alive_inside = probed.probe_detached(w, |s, _, _| s.wires[w].alive);
        assert!(!alive_inside, "the probe sees the state without the wire");
        assert_eq!(shape(&probed), before, "wire {w}: the probe changed the state");
    }
    assert!(tt_changed && pw_changed && pd_changed, "tri_trav {tt_changed} pass_w {pw_changed} pass_d {pd_changed}: each must change under a bare detach + reattach");
    assert!(bare_changes > 0, "the fixture does not show the side effect the probe must undo");
}
