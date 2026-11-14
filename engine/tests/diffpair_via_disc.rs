//! A coupled pair's via in the plan is the two-via disc, and that disc keeps clear of the copper of
//! both halves (user 2026-11-13: "partner copper is not own copper for clearance purposes").
//! In the planning model (`topo::diffpair::alias`) the N half's pads carry the P net, so before
//! this a pair via site could lie on a partner pad (a 2-layer badge, USB-: vias on USB+ pads of U7 /
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
