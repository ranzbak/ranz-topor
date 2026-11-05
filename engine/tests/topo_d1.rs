//! D1 tests: the oblique factor of a pass comes from its wire's taut path (pack.rs), not from
//! the evenly spread pass points (the exact-clearance Checker test is in `model_json.rs`).

use topor_engine::model::{BoardModel, Options};
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::init::InitWeights;
use topor_engine::topo::pack::{infl_dir, infl_of, Packer};
use topor_engine::topo::state::State;

const MM: f64 = 1e6;
const WS0: f64 = 400_000.0;

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
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}}],
            "pads":[{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}


/// A straight wire up a corridor between staggered pads of two sizes (each pad its own net):
/// the CDT edges it crosses run at changing angles, so the evenly spread pass points zig-zag
/// around x = 5 mm while the wire itself can run straight from (5, 1) to (5, 15).
fn corridor() -> BoardModel {
    let mut pads = vec![(1, 5.0, 1.0, true, 0.4), (1, 5.0, 15.0, true, 0.4)];
    for (i, y) in [3.0, 5.0, 7.0, 9.0, 11.0, 13.0].into_iter().enumerate() {
        pads.push((10 + i as i32, 3.6, y, true, if i % 2 == 0 { 1.0 } else { 1.4 }));
    }
    for (i, y) in [4.0, 6.0, 8.0, 10.0, 12.0].into_iter().enumerate() {
        pads.push((20 + i as i32, 6.4, y, true, if i % 2 == 0 { 1.4 } else { 1.0 }));
    }
    board((0.0, 0.0, 10.0, 16.0), &pads)
}

#[test]
fn oblique_factor_follows_the_taut_path_not_the_spread_pass_points() {
    let m = corridor();
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let conns = b.connections(&m);
    assert_eq!(conns.len(), 1);
    let mut st = State::new(&b.proj);
    st.route_conn(&conns[0], &InitWeights::default(), (conns[0].w + conns[0].s) / WS0).expect("routes");
    let packer = Packer::for_model(&m, &Options::default(), &b.proj);
    let pl = st.polyline(0);
    let t = packer.taut(&st, 0);
    let n = st.wires[0].passes.len();
    assert!(n >= 10, "{n} passes");
    let up = topor_engine::geom::Vec2 { x: 0.0, y: 1.0 };
    let (mut zig, mut worst_even) = (0usize, 0.0f64);
    for k in 0..n {
        let e = st.passes[st.wires[0].passes[k]].edge;
        // The taut path is the straight line x = 5 mm.
        assert!((t.at[k].x - 5.0 * MM).abs() < 1_000.0, "pass {k} at {:?}", t.at[k]);
        let (even, taut, truth) = (infl_of(&b.proj, e, &pl, k), infl_dir(&b.proj, e, t.dir[k]), infl_dir(&b.proj, e, up));
        assert!((taut - truth).abs() < 1e-3, "pass {k}: taut {taut} vs straight {truth}");
        worst_even = worst_even.max(even - truth);
        if (pl[k + 1].x - 5.0 * MM).abs() > 50_000.0 {
            zig += 1;
        }
        // Where the edge is square to the wire the factor is 1, whatever the spread points say.
        if truth < 1.01 {
            assert!(taut < 1.01, "pass {k}: {taut}");
        }
    }
    assert!(zig >= 4, "the spread pass points zig-zag ({zig} off the line)");
    assert!(worst_even > 0.5, "the spread pass points overstate 1/sin by {worst_even}");
    // Cached per wire content: the same Rc comes back.
    assert!(std::rc::Rc::ptr_eq(&t, &packer.taut(&st, 0)));
}

#[test]
fn funnel_bends_only_where_a_portal_forces_it() {
    use topor_engine::geom::Vec2;
    use topor_engine::topo::pack::funnel;
    let v = |x: f64, y: f64| Vec2 { x, y };
    // Northward from (0, 0) to (0, 10); portals at y = 1..9 are (left, right) = (-10, 10) wide
    // except the one at y = 5, which starts at x = 3: the path bends there and nowhere else.
    let mut ports = vec![(v(0.0, 0.0), v(0.0, 0.0))];
    for y in 1..10 {
        let l = if y == 5 { 3.0 } else { -10.0 };
        ports.push((v(l, y as f64), v(10.0, y as f64)));
    }
    ports.push((v(0.0, 10.0), v(0.0, 10.0)));
    let path = funnel(&ports);
    assert_eq!(path, vec![(v(0.0, 0.0), 0), (v(3.0, 5.0), 5), (v(0.0, 10.0), 10)]);
    // A straight corridor: no corner between the terminals.
    let straight: Vec<_> = (0..=10).map(|y| if y == 0 || y == 10 { (v(0.0, y as f64), v(0.0, y as f64)) } else { (v(-1.0, y as f64), v(1.0, y as f64)) }).collect();
    assert_eq!(funnel(&straight).len(), 2);
}

/// A via that dips a few um into a pad of its own net leaves a lens of copper narrower than
/// KiCad's minimum connection width (dcsgonebadge, BT1 pad 3: 4 um deep, "actual 0.1044 mm" <
/// 0.127 mm): such a site is refused; clear of the pad or centred in it is fine, and so is any
/// site when no minimum connection width is set.
#[test]
fn a_via_does_not_dip_into_its_own_pad() {
    use topor_engine::check::Checker;
    use topor_engine::geom::Vec2;
    use topor_engine::topo::draw::Drawer;
    use topor_engine::vias::ViaSize;
    let mut m = board((0.0, 0.0, 10.0, 10.0), &[(1, 5.0, 5.0, false, 0.8), (1, 2.0, 2.0, false, 0.4)]);
    m.rules.min_connection = 127_000;
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let st = State::new(&b.proj);
    let opts = Options::default();
    let checker = Checker::new(&m, &opts);
    let dr = Drawer::new(&m, &opts, &b, &st, &checker);
    let sz = ViaSize { d: 600_000, drill: 300_000 };
    // Pad edge at x = 5.4 mm, via radius 0.3 mm (math frame: y = -5 mm).
    let at = |x: f64| Vec2 { x: x * MM, y: -5.0 * MM };
    assert!(!dr.via_own_pad_ok(at(5.7 - 0.004), sz, 1), "4 um into the pad");
    assert!(dr.via_own_pad_ok(at(5.7 + 0.001), sz, 1), "clear of the pad");
    assert!(dr.via_own_pad_ok(at(5.0), sz, 1), "centred in the pad");
    assert!(dr.via_own_pad_ok(at(5.7 - 0.004), sz, 2), "another net: the Checker's business");
    let mut m0 = m.clone();
    m0.rules.min_connection = 0;
    let checker0 = Checker::new(&m0, &opts);
    let dr0 = Drawer::new(&m0, &opts, &b, &st, &checker0);
    assert!(dr0.via_own_pad_ok(at(5.7 - 0.004), sz, 1), "no minimum connection width set");
}

/// The drawing keeps a via's hole clearance too: a 0.4 / 0.3 mm via with 0.2 mm copper
/// clearance and 0.254 mm hole clearance needs 0.204 mm around its copper (the drill reaches
/// further), a 0.6 / 0.3 mm via keeps the copper clearance.
#[test]
fn via_clearance_covers_the_hole_clearance() {
    use topor_engine::topo::draw::via_clear;
    let mut m = board((0.0, 0.0, 10.0, 10.0), &[(1, 5.0, 5.0, false, 0.8)]);
    m.rules.default.clearance = 200_000;
    m.rules.hole_clearance = 254_000;
    assert!((via_clear(&m, 1, 400_000, 300_000) - 204_000.0).abs() < 1e-6);
    assert!((via_clear(&m, 1, 600_000, 300_000) - 200_000.0).abs() < 1e-6);
}
