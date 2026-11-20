//! TW6 parts A + C (`[vias] site_price`, docs/dev/design/tw6-via-sites.md A, C): one site room
//! (`Packer::site_room`) read by the plan, the drawing and the pad exits' via term; a via without a
//! site pays its count plus the graded deficit `(r - room) / (d + s)`; a layer change at a
//! continued dangling end's lead end takes the lead's via end as its site.

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route_with;
use topor_engine::topo::pack::Packer;

const MM: f64 = 1e6;

#[test]
fn site_price_is_opt_in_and_parses() {
    assert_eq!(Options::default().via_site_price, "off");
    let o = Options::from_json(r#"{"via_site_price": "graded"}"#).expect("options parse");
    assert_eq!(o.via_site_price, "graded");
}

/// The deficit is 0 where the via fits, grows with the shortfall (no step above 0), and is capped
/// at the via size inside an obstacle (room -inf).
#[test]
fn the_deficit_is_graded_and_capped() {
    let d = 0.6 * MM;
    assert_eq!(Packer::site_deficit(0.3 * MM, d), 0.0);
    assert_eq!(Packer::site_deficit(0.5 * MM, d), 0.0);
    let a = Packer::site_deficit(0.25 * MM, d);
    let b = Packer::site_deficit(0.1 * MM, d);
    assert!((a - 0.05 * MM).abs() < 1.0 && (b - 0.2 * MM).abs() < 1.0, "{a} {b}");
    assert_eq!(Packer::site_deficit(f64::NEG_INFINITY, d), d);
    assert_eq!(Packer::site_deficit(-5.0 * MM, d), d);
}

fn qmtech_crop() -> BoardModel {
    let path = format!("{}/tests/fixtures/qmtech-usb-crop.json", env!("CARGO_MANIFEST_DIR"));
    BoardModel::from_json(&std::fs::read_to_string(path).expect("fixture")).expect("fixture parses")
}

/// The graded site price sits in the wire's CM1 entry (`Frame::wire_over`), so every refinement
/// still ends with the stored via prices equal to fresh ones and the local judge's via focus stays
/// exact; with the via discs at their heights and the pad exits (the combination the A/B measures).
#[test]
fn site_prices_stay_fresh_and_focus_exact() {
    use std::sync::atomic::Ordering;
    use topor_engine::topo::refine::viacheck;
    let m = qmtech_crop();
    let o = Options::from_json(
        r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 2, "via_cuts": true, "via_site_price": "graded", "via_disc_height": true, "pad_exits": true}"#,
    )
    .expect("options parse");
    viacheck::ON.store(true, Ordering::Relaxed);
    viacheck::FOCUS_ON.store(true, Ordering::Relaxed);
    let (b0, f0) = (viacheck::BAD.load(Ordering::Relaxed), viacheck::FOCUS_BAD.load(Ordering::Relaxed));
    let (n0, g0) = (viacheck::CHECKED.load(Ordering::Relaxed), viacheck::FOCUS_CHECKED.load(Ordering::Relaxed));
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let _ = route_with(&m, &o, &ctl);
    let (bad, n) = (viacheck::BAD.load(Ordering::Relaxed) - b0, viacheck::CHECKED.load(Ordering::Relaxed) - n0);
    let (fbad, fn_) = (viacheck::FOCUS_BAD.load(Ordering::Relaxed) - f0, viacheck::FOCUS_CHECKED.load(Ordering::Relaxed) - g0);
    assert!(n > 0 && fn_ > 0, "the case does not show: {n} via entries, {fn_} judgements checked");
    assert_eq!(bad, 0, "{bad} stale via prices of {n}");
    assert_eq!(fbad, 0, "{fbad} of {fn_} local judgements differ from the judge with every wire in focus");
}

/// Off = the model before TW6: the same routed items with the option off as with it absent
/// (the default), and a run with it on completes (determinism of the default copper is checked on
/// the boards by the A/B).
#[test]
fn off_is_the_default_copper() {
    let m = qmtech_crop();
    let base = r#""wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 1, "via_cuts": true"#;
    let run = |extra: &str| {
        let o = Options::from_json(&format!("{{{base}{extra}}}")).expect("options parse");
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        let r = route_with(&m, &o, &ctl);
        format!("{:?}", r.items)
    };
    assert_eq!(run(""), run(r#", "via_site_price": "off""#));
    let _ = run(r#", "via_site_price": "graded""#);
}

/// TW6 part D: `[vias] pair_room` is opt-in and parses.
#[test]
fn pair_room_is_opt_in_and_parses() {
    assert!(!Options::default().via_pair_room);
    assert!(Options::from_json(r#"{"via_pair_room": true}"#).expect("options parse").via_pair_room);
}

/// TW6 part D on the qmtech USB crop: with pair_room the run completes and the off run equals the
/// default copper.
#[test]
fn pair_room_off_is_the_default_copper() {
    let m = qmtech_crop();
    let base = r#""wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 1, "via_cuts": true"#;
    let run = |extra: &str| {
        let o = Options::from_json(&format!("{{{base}{extra}}}")).expect("options parse");
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        format!("{:?}", route_with(&m, &o, &ctl).items)
    };
    assert_eq!(run(""), run(r#", "via_pair_room": false"#));
    let _ = run(r#", "via_pair_room": true"#);
}

/// site-price-cost: `Packer::point_free_cached` returns `cutflow::point_free`'s bits for every
/// (point, net, clearance), keyed by all three (a cached value of another net or clearance is never
/// handed out), and stores one entry per key (a repeat is a hit). Points next to a pad of net 1
/// on the two-pad board of `pads_board`, so net and clearance change the value. Then
/// [`memo_keeps_the_graded_copper`] (one test: it sets the process-wide `point_free_memo::OFF`,
/// which would empty this test's memo if they ran in parallel).
#[test]
fn the_point_free_memo_returns_the_computed_bits_and_keeps_the_copper() {
    use topor_engine::topo::board::{self, BuildOpts};
    use topor_engine::topo::cutflow::point_free;
    let m = pads_board();
    let b = board::build(&m, &BuildOpts { existing_vias: true, keepouts: true, copper_shapes: true, ..BuildOpts::default() }).unwrap();
    let pk = Packer::for_model(&m, &Options::default(), &b.proj);
    let pts = [(10.0, 9.0), (10.0, 8.2), (12.5, 10.0), (20.0, 20.0)].map(|(x, y)| topor_engine::geom::Vec2 { x: x * MM, y: -y * MM });
    let keys: Vec<(topor_engine::geom::Vec2, i32, f64)> = pts.iter().flat_map(|&q| [(q, 1, 0.2 * MM), (q, 2, 0.2 * MM), (q, 2, 0.5 * MM)]).collect();
    assert_eq!(pk.point_free_memo_len(), 0);
    for round in 0..2 {
        for &(q, net, s) in &keys {
            let want = point_free(&b.proj, &pk, q, net, s);
            assert_eq!(pk.point_free_cached(q, net, s).to_bits(), want.to_bits(), "round {round}: {q:?} net {net} s {s}");
        }
        assert_eq!(pk.point_free_memo_len(), keys.len(), "round {round}: one entry per key, a repeat is a hit");
    }
    // Not vacuous: next to the pad of net 1 the other net sees less room than its own net, and a
    // larger clearance less still.
    let q = pts[0];
    let (own, other, wide) = (pk.point_free_cached(q, 1, 0.2 * MM), pk.point_free_cached(q, 2, 0.2 * MM), pk.point_free_cached(q, 2, 0.5 * MM));
    assert!(own > other && other > wide, "own {own} other {other} wide {wide}");
    memo_keeps_the_graded_copper();
}

/// site-price-cost: with `[vias] site_price = "graded"` the memo leaves the copper as it was: the
/// qmtech USB crop routes to the same items with the memo as with every point priced afresh
/// (`pack::point_free_memo::OFF`), and the graded price changes this crop's copper (the case shows).
fn memo_keeps_the_graded_copper() {
    use std::sync::atomic::Ordering;
    use topor_engine::topo::pack::point_free_memo;
    let m = qmtech_crop();
    let base = r#""wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 1, "via_cuts": true, "via_disc_height": true, "pad_exits": true"#;
    let run = |extra: &str| {
        let o = Options::from_json(&format!("{{{base}{extra}}}")).expect("options parse");
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        format!("{:?}", route_with(&m, &o, &ctl).items)
    };
    let memo = run(r#", "via_site_price": "graded""#);
    point_free_memo::OFF.store(true, Ordering::Relaxed);
    let fresh = run(r#", "via_site_price": "graded""#);
    point_free_memo::OFF.store(false, Ordering::Relaxed);
    assert_eq!(memo, fresh, "the memo changed the graded copper");
    assert_ne!(memo, run(""), "the graded price does not change this crop's copper: the case does not show");
}

/// Two pads of net 1 at (10, 10) and (30, 10) and one of net 2 at (20, 30) on a two-layer 40 x 40 mm
/// board (KiCad frame).
fn pads_board() -> BoardModel {
    let p = |v: f64| (v * MM) as i64;
    let c = [(0.0, 0.0), (40.0, 0.0), (40.0, 40.0), (0.0, 40.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads: Vec<String> = [("A1", 1, 10.0, 10.0), ("A2", 1, 30.0, 10.0), ("B1", 2, 20.0, 30.0)]
        .iter()
        .map(|&(r, net, x, y)| {
            format!(
                r#"{{"uuid":"{r}","ref":"{r}","number":"1","net":{net},"attr":"smd","center":[{},{}],"rotation_deg":0.0,
                    "layers":[0],"shapes":{{"0":{{"kind":"rect","w":{},"h":{}}}}}}}"#,
                p(x),
                p(y),
                p(1.0),
                p(1.0)
            )
        })
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}}],
            "pads":[{}],"outline":[{}],
            "rules":{{"default":{{"clearance":200000,"width":250000}},"min_connection":127000,"max_error":5000,"per_net":{{}}}}}}"#,
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}
