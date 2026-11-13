//! Shopping list 5.8: the obstructing-wire half of the local via-pair minimisation (Luzin, Popov
//! & Popov 2013 p. 134 step 8.1, fig. 1; Luzin 2013 p. 104 fig. 21c) on the engine model of the
//! `pair_move` pattern (`harness/pair_patterns.py`, scratch/pair-patterns/pair_move/; the
//! 5.8 suite pattern, approved by the user 2026-11-13): two 0.5 mm rails B1, B2 (laid first, straight) cross the short 0.25 mm wire A,
//! whose pads are SMD on F.Cu; A then dives under both (2 vias), a local minimum of single-wire
//! reroutes. The pair move lays B1 and B2 again together round A's lower pad: 0 vias.

use topor_engine::check::Checker;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::route_with;
use topor_engine::geom::quant::TrackPrim;

const MM: f64 = 1e6;

/// One pad: net code, centre (mm, KiCad frame), size (w, h) mm, copper layer id (0 F.Cu, 2 B.Cu).
type P = (i32, f64, f64, f64, f64, u32);

/// 2-layer board `w` x `h` mm with SMD pads `pads`; net `k` has track width `widths[k-1]` (mm).
fn board(w: f64, h: f64, pads: &[P], widths: &[f64]) -> BoardModel {
    let p = |v: f64| (v * MM).round() as i64;
    let c = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pj: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y, sw, sh, l))| {
            format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"smd","center":[{},{}],
                    "rotation_deg":0.0,"layers":[{l}],"shapes":{{"{l}":{{"kind":"rect","w":{},"h":{}}}}},"drill":null}}"#,
                p(x),
                p(y),
                p(sw),
                p(sh)
            )
        })
        .collect();
    let nets: Vec<String> = (1..=8).map(|k| format!(r#"{{"code":{k},"name":"N{k}"}}"#)).collect();
    let per: Vec<String> = widths
        .iter()
        .enumerate()
        .map(|(i, &wd)| format!(r#""{}":{{"clearance":200000,"width":{},"via_d":800000,"via_drill":400000,"netclass":"C{i}","width_source":"netclass"}}"#, i + 1, p(wd)))
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{}],"pads":[{}],"outline":[{}],
            "rules":{{"min_clearance":200000,"min_track_width":200000,"min_via_diameter":450000,"min_via_drill":250000,
                     "min_annular":100000,"hole_to_hole":250000,"edge_clearance":500000,
                     "default":{{"clearance":200000,"width":250000,"via_d":800000,"via_drill":400000}},
                     "per_net":{{{}}}}}}}"#,
        nets.join(","),
        pj.join(","),
        segs.join(","),
        per.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn vias(items: &[Item]) -> Vec<(f64, f64, f64, f64)> {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Via(v) => Some((v.at.x as f64 / MM, v.at.y as f64 / MM, v.d as f64 / MM, v.drill as f64 / MM)),
            _ => None,
        })
        .collect()
}

fn clean(m: &BoardModel, items: &[Item], tag: &str) {
    let rep = Checker::new(m, &Options::default()).check_items(items);
    assert!(rep.violations.is_empty(), "{tag}: {} violations, first {:?}", rep.violations.len(), rep.violations.first());
}

/// The pair_move pattern (generator constants): A (net 1) pads (68, 3), (68, 70.125); B1 (net 2)
/// (3, 43) - (133, 43); B2 (net 3) (3, 44.2) - (133, 44.2); all 0.8 mm SMD pads on F.Cu; 136 x
/// 74.5 mm. Rails 0.5 mm, A 0.25 mm.
fn pair_move_board() -> BoardModel {
    let pads: Vec<P> = vec![
        (1, 68.0, 3.0, 0.8, 0.8, 0),
        (1, 68.0, 70.125, 0.8, 0.8, 0),
        (2, 3.0, 43.0, 0.8, 0.8, 0),
        (2, 133.0, 43.0, 0.8, 0.8, 0),
        (3, 3.0, 44.2, 0.8, 0.8, 0),
        (3, 133.0, 44.2, 0.8, 0.8, 0),
    ];
    board(136.0, 74.5, &pads, &[0.25, 0.5, 0.5])
}

fn opts58(steiner: bool, capacity: &str, pair_move: bool, seed: u64) -> Options {
    Options { opt_loop: "topor".into(), search_length: "taut".into(), steiner, capacity_model: capacity.into(), via_pair_move: pair_move, seed, ..Options::default() }
}

/// Drawn length (mm) of net `net`'s straight segments.
fn net_len(items: &[Item], net: i32) -> f64 {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Track(t) if t.net == net => match t.prim {
                TrackPrim::Seg { a, b } => Some(((b.x - a.x) as f64).hypot((b.y - a.y) as f64) / MM),
                _ => None,
            },
            _ => None,
        })
        .sum()
}

/// The TopoR loop with the pair move, steiner off (per-edge capacity and the cut model): no via,
/// A straight (67.125 mm), the copper clean. Without it the loop stays in the local minimum: A
/// keeps its two vias (the rails stay straight).
#[test]
fn pair_move_frees_the_short_wire() {
    let m = pair_move_board();
    for cap in ["edges", "cuts"] {
        for seed in [0u64, 1] {
            let tag = format!("steiner off {cap} seed {seed}");
            let r = route_with(&m, &opts58(false, cap, true, seed), &RunControl::new());
            assert_eq!(r.failed, 0, "{tag}: {:?}", r.warnings);
            assert!(vias(&r.items).is_empty(), "{tag}: {:?} {:?}", vias(&r.items), r.warnings);
            assert!((net_len(&r.items, 1) - 67.125).abs() < 0.05, "{tag}: A {}", net_len(&r.items, 1));
            assert!(r.warnings.iter().any(|w| w.contains("[vias] pair_move") && !w.contains("): 0 of")), "{tag}: {:?}", r.warnings);
            clean(&m, &r.items, &tag);
            let r0 = route_with(&m, &opts58(false, cap, false, seed), &RunControl::new());
            assert_eq!(r0.failed, 0, "{tag}: {:?}", r0.warnings);
            assert_eq!(vias(&r0.items).len(), 2, "{tag}, pair_move off: {:?}", vias(&r0.items));
        }
    }
}

/// The same board with steiner on: the correct result is the same (no via, A straight, clean
/// copper). PARKED, not 5.8 (Steiner-vertex edge room): an edge from A2's pad corner to a free
/// Steiner vertex 0.85 mm away is unlimited room for the search but over-full for the per-edge
/// capacity model and the drawing's packer; with the per-edge model the move is refused (3
/// predicted over-full edge-layers), with the cut model it is kept but the drawing crosses and
/// shorts the rails at A2. Ignored until that is fixed; it asserts the correct result, not today's.
#[test]
#[ignore = "parked: Steiner-vertex edge room (search unlimited, per-edge capacity / drawing packer over-full)"]
fn pair_move_frees_the_short_wire_steiner_on() {
    let m = pair_move_board();
    for cap in ["edges", "cuts"] {
        let tag = format!("steiner on {cap}");
        let r = route_with(&m, &opts58(true, cap, true, 0), &RunControl::new());
        assert_eq!(r.failed, 0, "{tag}: {:?}", r.warnings);
        assert!(vias(&r.items).is_empty(), "{tag}: {:?}", vias(&r.items));
        assert!((net_len(&r.items, 1) - 67.125).abs() < 0.05, "{tag}: A {}", net_len(&r.items, 1));
        clean(&m, &r.items, &tag);
    }
}

/// The graded stretch price (`[vias] pair_max_mm` as a scale, never a cutoff): next to nothing for
/// a short stretch, one via at the scale, growing smoothly (with the square) beyond it.
#[test]
fn pair_stretch_price_is_graded() {
    use topor_engine::topo::viaopt::pair_len_price;
    let (scale, via) = (10.0 * MM, 25.0);
    assert!(pair_len_price(1.2 * MM, scale, via) < 0.5);
    assert!((pair_len_price(scale, scale, via) - via).abs() < 1e-9);
    let mut last = -1.0;
    for k in 0..=60 {
        let p = pair_len_price(k as f64 * MM, scale, via);
        assert!(p > last && p.is_finite(), "not growing at {k} mm");
        last = p;
    }
    assert!((pair_len_price(20.0 * MM, scale, via) - 4.0 * via).abs() < 1e-9);
    assert!(pair_len_price(5.0 * MM, 0.0, via).is_finite());
}

/// `loop = "cycles"` (the default before M0; M0 made `"topor"` the default, so it is set here): the
/// move does not run, the setting changes nothing.
#[test]
fn pair_move_is_topor_loop_only() {
    let m = pair_move_board();
    let a = route_with(&m, &Options { search_length: "taut".into(), opt_loop: "cycles".into(), via_pair_move: true, ..Options::default() }, &RunControl::new());
    let b = route_with(&m, &Options { search_length: "taut".into(), opt_loop: "cycles".into(), via_pair_move: false, ..Options::default() }, &RunControl::new());
    assert_eq!(format!("{:?}", a.items), format!("{:?}", b.items));
}
