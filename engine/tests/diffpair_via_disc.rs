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
    pair_board_pitch(0.65)
}

/// [`pair_board`] with the two pads of a pair `dy` mm apart.
fn pair_board_pitch(dy: f64) -> BoardModel {
    let p = |v: f64| (v * MM).round() as i64;
    let c = [(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    // (ref, number, net, x, y)
    let pads = [("J1", "1", 1, 2.0, 5.0), ("J1", "2", 2, 2.0, 5.0 + dy), ("J2", "1", 1, 18.0, 5.0), ("J2", "2", 2, 18.0, 5.0 + dy)];
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

/// A pair whose plan changes layer at ONE point at each pad (a run of one point, e.g. the plan's
/// forced via at a pad centre when the terminal triangle has no site): `realise` kept no via pair
/// for it (`pts.len() >= 2` filter) and both tracks ended on the far layer at their F.Cu pads.
#[test]
fn one_point_layer_change_keeps_its_via_pair() {
    use topor_engine::check::Checker;
    use topor_engine::geom::Vec2;
    use topor_engine::model::Item;
    let model = pair_board_pitch(1.2);
    let opts = Options::default();
    let mut dec = topor_engine::decompose::decompose(&model, &opts, &[0, 2]);
    let pairs = diffpair::plan(&model, &mut dec);
    let cp = &pairs.coupled[0];
    let checker = Checker::new(&model, &opts);
    let ctx = diffpair::Ctx { model: &model, opts: &opts, checker: &checker, octi: true };
    let at = |x: f64, y: f64| Vec2 { x: x * MM, y: y * MM };
    let pad = |i: usize| model.pads[i].center.to_vec2();
    let long = diffpair::pad_long(&model.pads[0]);
    let (ea, eb) = (diffpair::Ends { p: pad(0), n: pad(1), long }, diffpair::Ends { p: pad(2), n: pad(3), long });
    // F.Cu at the J1 pad (one point), B.Cu across, F.Cu at the J2 pad (one point).
    let runs = vec![(0u32, vec![at(2.0, 5.0)]), (2u32, vec![at(2.0, 5.0), at(18.0, 5.0)]), (0u32, vec![at(18.0, 5.0)])];
    let idx = topor_engine::check::ItemIndex::new(checker.cell());
    let rz = diffpair::realise(&ctx, cp, &runs, ea, eb, &idx, true);
    assert_eq!((rz.p_vias.len(), rz.n_vias.len()), (2, 2), "a via pair at each one-point layer change");
    for (items, net, pads) in [(&rz.p_items, 1, [0usize, 2]), (&rz.n_items, 2, [1usize, 3])] {
        for &pi in &pads {
            let c = pad(pi);
            // F.Cu copper (stub) touches the pad centre, and B.Cu copper exists: the wire reaches the pad on its layer.
            let on = |layer: u32| {
                items.iter().any(|it| match it {
                    Item::Track(t) if t.layer == layer => {
                        let pts = match t.prim {
                            topor_engine::geom::TrackPrim::Seg { a, b } => [a, b],
                            _ => return false,
                        };
                        pts.iter().any(|q| (Vec2 { x: f64::from(q.x), y: -f64::from(q.y) } - c).len() < 1.0 || (Vec2 { x: f64::from(q.x), y: f64::from(q.y) } - c).len() < 1.0)
                    }
                    _ => false,
                })
            };
            assert!(on(0), "net {net}: no F.Cu stub at pad {pi}");
        }
        assert!(items.iter().any(|it| matches!(it, Item::Track(t) if t.layer == 2)), "net {net}: B.Cu run");
    }
    let rep = checker.check_items(&[rz.p_items.clone(), rz.n_items.clone()].concat());
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

/// A pair connection of only two runs (a one-point run and one real run): the real run's end legs
/// are centred on the pad pairs exactly once. Centring it twice (in the run expansion and again in
/// `realise`) moved a single leg by the mean of both ends each time and left it off the pair middle.
#[test]
fn one_point_run_with_one_neighbour_is_centred_once() {
    use topor_engine::check::Checker;
    use topor_engine::geom::{TrackPrim, Vec2};
    use topor_engine::model::Item;
    let model = pair_board_pitch(1.2);
    let opts = Options::default();
    let mut dec = topor_engine::decompose::decompose(&model, &opts, &[0, 2]);
    let pairs = diffpair::plan(&model, &mut dec);
    let cp = &pairs.coupled[0];
    let checker = Checker::new(&model, &opts);
    let ctx = diffpair::Ctx { model: &model, opts: &opts, checker: &checker, octi: true };
    let pad = |i: usize| model.pads[i].center.to_vec2();
    let long = diffpair::pad_long(&model.pads[0]);
    let (ea, eb) = (diffpair::Ends { p: pad(0), n: pad(1), long }, diffpair::Ends { p: pad(2), n: pad(3), long });
    let at = |x: f64, y: f64| Vec2 { x: x * MM, y: y * MM };
    let runs = vec![(0u32, vec![at(2.0, 5.0)]), (2u32, vec![at(2.0, 5.0), at(18.0, 5.0)])];
    let idx = topor_engine::check::ItemIndex::new(checker.cell());
    let rz = diffpair::realise(&ctx, cp, &runs, ea, eb, &idx, true);
    // The long B.Cu leg of P and N lies at mid -+ pitch / 2 (pads at y 5.0 and 6.2, mid 5.6; items have y flipped).
    let leg = |items: &[Item]| {
        items.iter().filter_map(|it| match it {
            Item::Track(t) if t.layer == 2 => match t.prim {
                TrackPrim::Seg { a, b } if a.y == b.y && (b.x - a.x).abs() > 5_000_000 => Some(-f64::from(a.y)),
                _ => None,
            },
            _ => None,
        }).next().expect("a long B.Cu leg")
    };
    let (yp, yn) = (leg(&rz.p_items), leg(&rz.n_items));
    assert!(((yp + yn) * 0.5 - 5.6 * MM).abs() < 5_000.0, "legs at {yp} / {yn}, pair middle 5.6 mm");
}
