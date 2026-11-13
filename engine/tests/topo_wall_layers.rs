//! M0 (2026-11-13): a wire placed through another net's wall (4.3 `initial = "once"`, the last
//! resort `State::any_path`: "placed anyway through other copper, priced as violations",
//! costly-not-failed) is layered like any other wire: the layer assignment does not panic on it
//! ("wire inside a wall", seen on qmtech in the TopoR loop's reroute with `[capacity] model =
//! "edges"`); the wall triangle restricts no layer, its crossing is priced as foreign copper
//! (Q14b) elsewhere.

use topor_engine::model::BoardModel;
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::layers::{self, LayerParams};
use topor_engine::topo::state::{State, Wire};

const MM: f64 = 1e6;

/// Board 20 x 10 mm with `layers` copper layers (KiCad ids), net 1 pads at x 2 and 18 (y 5) and
/// a column of overlapping net 2 PTH pads across the board at x 10: no legal path for net 1.
fn walled(layers: &[(i32, &str)]) -> BoardModel {
    let p = |v: f64| (v * MM) as i64;
    let c = [(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let ids: Vec<String> = layers.iter().map(|l| l.0.to_string()).collect();
    let mut pads = vec![(1, 2.0, 5.0, 1.0), (1, 18.0, 5.0, 1.0)];
    for k in 0..6 {
        pads.push((2, 10.0, 0.0 + 2.0 * k as f64, 2.4));
    }
    let pads: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y, d))| {
            let sh = format!(r#"{{"kind":"rect","w":{0},"h":{0}}}"#, p(d));
            let shapes: Vec<String> = layers.iter().map(|l| format!(r#""{}":{sh}"#, l.0)).collect();
            format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"pth",
                    "center":[{},{}],"rotation_deg":0.0,"layers":[{}],
                    "shapes":{{{}}},"drill":{{"w":{4},"h":{4}}}}}"#,
                p(x),
                p(y),
                ids.join(","),
                shapes.join(","),
                p(d / 2.0),
            )
        })
        .collect();
    let ls: Vec<String> = layers.iter().map(|l| format!(r#"{{"id":{},"name":"{}","kind":"signal"}}"#, l.0, l.1)).collect();
    let s = format!(
        r#"{{"units":"nm","board":"t","layers":[{}],
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}}],
            "pads":[{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
        ls.join(","),
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

#[test]
fn a_wire_placed_through_a_wall_is_layered_without_a_panic() {
    for layers in [&[(0, "F.Cu"), (2, "B.Cu")][..], &[(0, "F.Cu"), (4, "In1.Cu"), (6, "In2.Cu"), (2, "B.Cu")][..]] {
        let m = walled(layers);
        let b = board::build(&m, &BuildOpts::default()).unwrap();
        let c = b.connections(&m).into_iter().find(|c| c.net == 1).expect("net 1 connection");
        let mut st = State::new(&b.proj);
        let (path, tris) = st.any_path(&c).expect("any_path crosses the wall");
        let walls = tris.iter().filter(|&&t| b.proj.allowed(t, c.net).is_none()).count();
        assert!(walls > 0, "{} layers: the test needs a path through net 2's wall", layers.len());
        let wire = Wire { net: c.net, w: c.w, s: c.s, a: c.a, b: c.b, pa: c.pa, pb: c.pb, turns: Vec::new(), passes: Vec::new(), tris: Vec::new(), alive: true };
        st.insert(wire, &path, tris);
        let smd = |o: usize| b.smd_layer(o);
        let lr = layers::assign_with(&st, &smd, &LayerParams::default(), None);
        assert_eq!(lr.events.len(), st.wires.len(), "{} layers: the wire is layered", layers.len());
    }
}
