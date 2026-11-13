//! A coupled pair's via in the plan is the two-via disc, and that disc keeps clear of the copper of
//! both halves (user 2026-11-13: "partner copper is not own copper for clearance purposes").
//! In the planning model (`topo::diffpair::alias`) the N half's pads carry the P net, so before
//! this a pair via site could lie on a partner pad (dcsgonebadge USB-: vias on USB+ pads of U7 /
//! J1; qmtech U1-Out_1 +/-); and with the pair via resolved by the plugin (f6880cd) the plan
//! reserved only one small via instead of the disc of both.

use topor_engine::model::{BoardModel, Options};
use topor_engine::topo::{board, cutflow, diffpair, pack::Packer};
use topor_engine::vias::via_sizes;

const MM: f64 = 1e6;

/// A pad field: J1 on the left, J2 on the right, each with a USB+ / USB- pad 0.65 mm apart
/// (0.4 x 0.4 mm SMD on F.Cu), a 0.2 / 0.2 mm pair with a 0.45 / 0.25 mm pair via, gap 0.2 mm.
fn pair_board() -> BoardModel {
    let p = |v: f64| (v * MM).round() as i64;
    let c = [(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    // (ref, number, net, x, y)
    let pads = [("J1", "1", 1, 2.0, 5.0), ("J1", "2", 2, 2.0, 5.65), ("J2", "1", 1, 18.0, 5.0), ("J2", "2", 2, 18.0, 5.65)];
    let pj: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(r, num, net, x, y))| {
            format!(
                r#"{{"uuid":"p{i}","ref":"{r}","number":"{num}","net":{net},"attr":"smd","center":[{},{}],
                    "rotation_deg":0.0,"layers":[0],"shapes":{{"0":{{"kind":"rect","w":{},"h":{}}}}},"drill":null}}"#,
                p(x),
                p(y),
                p(0.4),
                p(0.4)
            )
        })
        .collect();
    let nr = r#"{"clearance":200000,"width":200000,"via_d":600000,"via_drill":300000,"netclass":"Default","width_source":"netclass"}"#;
    let s = format!(
        r#"{{"units":"nm","board":"pairvia",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"USB+"}},{{"code":2,"name":"USB-"}}],
            "pads":[{}],"outline":[{}],
            "rules":{{"min_clearance":100000,"min_track_width":100000,"min_via_diameter":250000,"min_via_drill":150000,
                     "min_annular":50000,"hole_clearance":100000,"hole_to_hole":200000,"edge_clearance":200000,
                     "default":{{"clearance":200000,"width":200000,"via_d":600000,"via_drill":300000}},
                     "per_net":{{"1":{nr},"2":{nr}}},
                     "diff_pairs":[{{"p":1,"n":2,"p_name":"USB+","n_name":"USB-","netclass":"Default","width":200000,"gap":200000,
                                    "via_gap":200000,"via_d":450000,"via_drill":250000}}]}}}}"#,
        pj.join(","),
        segs.join(",")
    );
    serde_json::from_str(&s).expect("model json")
}

/// The planning model of the pair board and its board build.
fn planned() -> (BoardModel, Options) {
    let model = pair_board();
    let opts = Options::default();
    let mut dec = topor_engine::decompose::decompose(&model, &opts, &[0, 2]);
    let pairs = diffpair::plan(&model, &mut dec);
    assert_eq!(pairs.coupled.len(), 1, "the pair is coupled: {:?}", pairs.notes);
    (diffpair::alias(&model, &pairs, &opts), opts)
}

#[test]
fn pair_via_in_the_plan_is_the_disc_of_both_vias() {
    let (pm, opts) = planned();
    // disc = pitch of the two vias (0.45 + 0.2) + one via 0.45 = 1.1 mm; before: 0.45 (one via)
    let first = via_sizes(&pm.rules, 1, &opts)[0];
    assert_eq!(first.d, 1_100_000, "{first:?}");
    assert_eq!(via_sizes(&pm.rules, 1, &opts).len(), 1, "no smaller disc");
    // a net routed alone (outside the plan) still uses the pair's own via
    let single = pair_board();
    assert_eq!(via_sizes(&single.rules, 1, &opts)[0].d, 450_000);
    assert_eq!(via_sizes(&single.rules, 2, &opts)[0].d, 450_000);
}

#[test]
fn pair_via_disc_keeps_clear_of_the_partner_pads() {
    let (pm, opts) = planned();
    let bo = board::BuildOpts { existing_vias: true, keepouts: true, copper_shapes: true, ..board::BuildOpts::default() };
    let b = board::build_full(&pm, &bo, &[], &[]).expect("board");
    let packer = Packer::for_model(&pm, &opts, &b.proj);
    let pads: Vec<_> = b.proj.obstacles.iter().filter(|o| o.net == Some(1)).collect();
    assert_eq!(pads.len(), 4, "every pad of both halves carries the lead net in the plan");
    for o in &pads {
        // on a pad of either half (the N pads are the partner's copper): no room for the disc
        let f = cutflow::point_free(&b.proj, &packer, o.center, 1, 200_000.0);
        assert!(f < 0.0, "pair via on pair pad at {:?}: free {f}", o.center);
        // 0.5 mm off the pad edge the 1.1 mm disc (radius 0.55 + clearance 0.2) does not fit
        let q = o.center + topor_engine::geom::Vec2 { x: 0.0, y: 0.7e6 };
        let f = cutflow::point_free(&b.proj, &packer, q, 1, 200_000.0);
        assert!(f < 0.55e6, "disc next to the pad field at {q:?}: free {f}");
    }
    // in the open the disc fits
    let mid = topor_engine::geom::Vec2 { x: 10.0e6, y: 5.0e6 };
    assert!(cutflow::point_free(&b.proj, &packer, mid, 1, 200_000.0) > 0.55e6);
}

#[test]
fn pair_disc_free_radius_counts_both_halves_pads() {
    // Item 1 (pair-via site proposal 2026-11-13): the projection's free radius of a pair net counts
    // the pair's own pads (in the plan the partner's pads carry the lead net): on a pad it is 0.
    let (pm, _) = planned();
    let bo = board::BuildOpts { existing_vias: true, keepouts: true, copper_shapes: true, ..board::BuildOpts::default() };
    let b = board::build_full(&pm, &bo, &[], &[]).expect("board");
    assert!(b.proj.pair_nets.contains(&1));
    for o in b.proj.obstacles.iter().filter(|o| o.net == Some(1)) {
        assert_eq!(b.proj.free_radius(o.center, 1), 0.0, "pair net on its own pad at {:?}", o.center);
    }
    // the layer report's site radius of the pair: the disc (radius 0.55) + clearance 0.2
    let need = topor_engine::vias::pair_via_need(&pm.rules, &Options::default());
    assert!((need - 0.75e6).abs() < 0.02e6, "pair via need {need}");
    assert_eq!(topor_engine::vias::pair_via_need(&pair_board().rules, &Options::default()), 0.0, "no pair disc outside the plan");
}

// ---- Item 2 / 4 end to end in the engine: a pair from J1 (F.Cu) to J2 (B.Cu) must change layer in a
// GND-walled channel (copper on both layers). The pair band (0.2 + 0.2 + 0.2 mm, 0.2 mm clearance:
// 1.0 mm) fits the 1.4 mm channel everywhere; the two-via disc (1.1 mm + 2 x 0.2 mm = 1.5 mm) fits
// only where the channel widens to 3.2 mm (x 5 .. 9 mm), when it does.

fn channel_board(wide: bool) -> BoardModel {
    let p = |v: f64| (v * MM).round() as i64;
    let c = [(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    // (ref, number, net, x, y, layer)
    let pads = [("J1", "1", 1, 1.0, 5.0, 0), ("J1", "2", 2, 1.0, 5.65, 0), ("J2", "1", 1, 19.0, 5.0, 2), ("J2", "2", 2, 19.0, 5.65, 2)];
    let pj: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(r, num, net, x, y, l))| {
            format!(
                r#"{{"uuid":"p{i}","ref":"{r}","number":"{num}","net":{net},"attr":"smd","center":[{},{}],
                    "rotation_deg":0.0,"layers":[{l}],"shapes":{{"{l}":{{"kind":"rect","w":{},"h":{}}}}},"drill":null}}"#,
                p(x),
                p(y),
                p(0.3),
                p(0.3)
            )
        })
        .collect();
    // channel centre y 5.325, half width 0.7 (walls at 4.625 / 6.025); wide part half width 1.6
    let (yc, h, hw) = (5.325, 0.7, 1.6);
    let mut walls: Vec<(f64, f64, f64, f64)> = Vec::new();
    if wide {
        for &(x0, x1, hh) in &[(0.3, 5.0, h), (5.0, 9.0, hw), (9.0, 19.7, h)] {
            walls.push((x0, 0.3, x1, yc - hh));
            walls.push((x0, yc + hh, x1, 9.7));
        }
    } else {
        walls.push((0.3, 0.3, 19.7, yc - h));
        walls.push((0.3, yc + h, 19.7, 9.7));
    }
    let mut shapes: Vec<String> = Vec::new();
    for (i, &(x0, y0, x1, y1)) in walls.iter().enumerate() {
        for l in [0, 2] {
            shapes.push(format!(
                r#"{{"uuid":"w{i}-{l}","net":3,"layer":{l},"poly":[[{},{}],[{},{}],[{},{}],[{},{}]]}}"#,
                p(x0),
                p(y0),
                p(x1),
                p(y0),
                p(x1),
                p(y1),
                p(x0),
                p(y1)
            ));
        }
    }
    let nr = r#"{"clearance":200000,"width":200000,"via_d":600000,"via_drill":300000,"netclass":"Default","width_source":"netclass"}"#;
    let s = format!(
        r#"{{"units":"nm","board":"pairchannel",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"USB+"}},{{"code":2,"name":"USB-"}},{{"code":3,"name":"GND"}}],
            "pads":[{}],"outline":[{}],"copper_shapes":[{}],
            "rules":{{"min_clearance":100000,"min_track_width":100000,"min_via_diameter":250000,"min_via_drill":150000,
                     "min_annular":50000,"hole_clearance":100000,"hole_to_hole":200000,"edge_clearance":200000,
                     "default":{{"clearance":200000,"width":200000,"via_d":600000,"via_drill":300000}},
                     "per_net":{{"1":{nr},"2":{nr}}},
                     "diff_pairs":[{{"p":1,"n":2,"p_name":"USB+","n_name":"USB-","netclass":"Default","width":200000,"gap":200000,
                                    "via_gap":200000,"via_d":450000,"via_drill":250000}}]}}}}"#,
        pj.join(","),
        segs.join(","),
        shapes.join(",")
    );
    serde_json::from_str(&s).expect("model json")
}

fn route_channel(wide: bool) -> (BoardModel, topor_engine::route::RouteResult) {
    let m = channel_board(wide);
    let opts = Options { runs: 1, ..Options::default() };
    let r = topor_engine::route::route_with(&m, &opts, &topor_engine::control::RunControl::new());
    for w in r.warnings.iter().filter(|w| w.starts_with("pair via") || w.starts_with("via relocation") || w.starts_with("vias, plan vs drawn") || w.starts_with("plan metrics")) {
        eprintln!("{w}");
    }
    (m, r)
}

fn pair_vias(r: &topor_engine::route::RouteResult) -> Vec<(i32, f64, f64)> {
    r.items
        .iter()
        .filter_map(|it| match it {
            topor_engine::model::Item::Via(v) if v.net == 1 || v.net == 2 => Some((v.net, v.at.x as f64 / MM, v.at.y as f64 / MM)),
            _ => None,
        })
        .collect()
}

#[test]
fn pair_via_takes_the_one_side_with_room_and_draws_clean() {
    let (m, r) = route_channel(true);
    assert_eq!(r.failed, 0, "{:?}", r.warnings);
    let vs = pair_vias(&r);
    assert_eq!(vs.len(), 2, "one via pair: {vs:?}");
    for &(_, x, _) in &vs {
        assert!((5.0..=9.0).contains(&x), "the via pair sits where the channel holds its disc: {vs:?}");
    }
    assert!(!r.warnings.iter().any(|w| w.starts_with("pair via forced:")), "{:?}", r.warnings);
    let rep = topor_engine::check::Checker::new(&m, &Options::default()).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

#[test]
fn pair_via_without_room_is_still_drawn_and_warned() {
    // Costly, not failed: no site holds the disc; the pair is drawn with its via pair at the least
    // missing room and the forced via pair is a warning of its own.
    let (_, r) = route_channel(false);
    assert_eq!(r.failed, 0, "{:?}", r.warnings);
    assert_eq!(pair_vias(&r).len(), 2, "{:?}", pair_vias(&r));
    let forced: Vec<&String> = r.warnings.iter().filter(|w| w.starts_with("pair via forced: diff pair USB+ / USB-")).collect();
    assert_eq!(forced.len(), 1, "{:?}", r.warnings);
    let metrics = r.warnings.iter().find(|w| w.starts_with("plan metrics:")).expect("plan metrics");
    assert!(metrics.contains("\"vias_without_site\":1"), "the plan predicts it: {metrics}");
}
