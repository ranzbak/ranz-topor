//! D2 tests (board ladder, simple boards first).

use topor_engine::geom::Vec2;
use topor_engine::topo::pack::funnel;

const MM: f64 = 1e6;

/// A wire that wraps a vertex with no clearance margin (a Steiner point, or a pad corner of
/// another layer: `margin_any` is 0 there) meets several radial edges of that vertex at the
/// vertex itself. The funnel gives that vertex once, so the taut path keeps a direction on both
/// sides of it (wiccon SCLK: three repeated corners at a B.Cu pad corner gave a zero direction,
/// i.e. an oblique factor of 1, on an edge the wire really crosses at 34 degrees).
#[test]
fn funnel_gives_a_corner_on_shared_portal_ends_once() {
    let v = |x: f64, y: f64| Vec2 { x: x * MM, y: y * MM };
    let c = v(0.0, 0.0);
    // From the north, round the vertex c (on the left, seen going south), then east: three
    // radial portals from c (pointing west, south-west and south) have c as their left end.
    let ports = vec![
        (v(-0.5, 3.0), v(-0.5, 3.0)),
        (c, v(-2.0, 0.0)),
        (c, v(-2.0, -2.0)),
        (c, v(0.0, -2.0)),
        (c, v(2.0, -1.5)),
        (v(4.0, -0.5), v(2.0, -1.5)),
        (v(6.0, -0.5), v(6.0, -0.5)),
    ];
    let path = funnel(&ports);
    let pts: Vec<Vec2> = path.iter().map(|p| p.0).collect();
    for w in pts.windows(2) {
        assert!((w[1] - w[0]).len() > 1.0, "repeated corner in {path:?}");
    }
    assert_eq!(pts.len(), 4, "{path:?}");
    assert!((pts[1] - c).len() < 1.0, "{path:?}");
}

/// The taut path's crossing directions where it wraps a corner are real directions for
/// `infl_dir`, never shorter than 1 nm (which reads as undefined, factor 1): they were unit
/// vectors, so a pass of a wire wrapping a pad corner got the factor 1 whenever the vector's
/// length rounded below 1.0 (wiccon SCLK past TP-pad corner (125.74, 131.76): 1.0 instead of
/// 2.5 on the edge the drawn neighbour crosses at 34 degrees).
#[test]
fn taut_directions_at_a_wrapped_corner_are_defined() {
    use topor_engine::model::Options;
    use topor_engine::topo::board::{self, BuildOpts};
    use topor_engine::topo::init::InitWeights;
    use topor_engine::topo::pack::{infl_dir, Packer};
    use topor_engine::topo::state::State;
    // A wire from (2, 10) to (18, 10) round a 6 x 6 mm pad of another net at (10, 10).
    let p = |v: f64| (v * MM) as i64;
    let pads = [(1, 2.0, 10.0, 0.4), (1, 18.0, 10.0, 0.4), (2, 10.0, 10.0, 6.0)];
    let pads: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y, d))| {
            let sh = format!(r#"{{"kind":"rect","w":{0},"h":{0}}}"#, p(d));
            format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"pth","center":[{},{}],"rotation_deg":0.0,"layers":[0, 2],"shapes":{{"0":{sh},"2":{sh}}},"drill":{{"w":{dr},"h":{dr}}}}}"#,
                p(x),
                p(y),
                dr = p(0.2)
            )
        })
        .collect();
    let c = [(0.0, 0.0), (20.0, 0.0), (20.0, 20.0), (0.0, 20.0)];
    let segs: Vec<String> = (0..4).map(|i| format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(c[i].0), p(c[i].1), p(c[(i + 1) % 4].0), p(c[(i + 1) % 4].1))).collect();
    let s = format!(
        r#"{{"units":"nm","board":"t","layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],"nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}}],"pads":[{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
        pads.join(","),
        segs.join(",")
    );
    let m = topor_engine::model::BoardModel::from_json(&s).unwrap();
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let conns = b.connections(&m);
    assert_eq!(conns.len(), 1);
    let mut st = State::new(&b.proj);
    st.route_conn(&conns[0], &InitWeights::default(), (conns[0].w + conns[0].s) / 400_000.0).expect("routes");
    let packer = Packer::for_model(&m, &Options::default(), &b.proj);
    let t = packer.taut(&st, 0);
    let pad = b.proj.obstacles.iter().find(|o| (o.center.x - 10.0 * MM).abs() < 1.0).unwrap();
    let mut wrapped_oblique = 0;
    for (k, &pid) in st.wires[0].passes.iter().enumerate() {
        let d = t.dir[k];
        assert!(d.len() == 0.0 || d.len() >= 1.0, "pass {k}: direction {d:?} reads as undefined");
        // Passes at a corner of the big pad (the taut path wraps it at its clearance margin).
        let near = pad.poly.iter().any(|&v| (v - t.at[k]).len() < 1.0 * MM);
        let e = st.passes[pid].edge;
        if near && infl_dir(&b.proj, e, d) > 1.01 {
            wrapped_oblique += 1;
        }
    }
    assert!(wrapped_oblique > 0, "no oblique pass at the wrapped corners: {:?}", t.dir);
}
