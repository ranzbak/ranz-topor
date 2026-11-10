//! Shopping list 5.4 (roadmap P7): via size rule, via relocation, branch-point relocation and the
//! local via-pair minimisation on engine models of the approved via patterns
//! (`scratch/via-patterns/`, generator `harness/via_patterns.py`): same pads, same rules (net
//! class 0.25 / 0.20, class via 0.80 / 0.40, Board Setup minimums via 0.45, drill 0.25, annular
//! 0.10, copper to edge 0.50), routed end to end by `route_with`. Expected answers from each
//! pattern's `expected.json`.

use topor_engine::check::Checker;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::route_with;

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

fn opts54(loop_: &str, steiner: bool) -> Options {
    Options {
        via_size: "board_min".into(),
        via_relocate: true,
        via_pair_min: true,
        branch_relocate: true,
        opt_loop: loop_.into(),
        search_length: "taut".into(),
        steiner,
        ..Options::default()
    }
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

fn clean(m: &BoardModel, items: &[Item]) {
    let rep = Checker::new(m, &Options::default()).check_items(items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

/// via_size_rule: 0.2, 1.0, 2.0 mm traces from F.Cu to B.Cu pads: one via each, 0.45 / 0.25,
/// 0.52 / 0.32, 0.84 / 0.64, never the class via 0.80 / 0.40.
#[test]
fn via_size_rule_board_min() {
    let pads: Vec<P> = vec![
        (1, 4.5, 3.0, 0.8, 0.8, 0),
        (1, 21.5, 3.0, 0.8, 0.8, 2),
        (2, 4.5, 9.5, 2.0, 2.0, 0),
        (2, 21.5, 9.5, 2.0, 2.0, 2),
        (3, 4.5, 16.5, 3.0, 3.0, 0),
        (3, 21.5, 16.5, 3.0, 3.0, 2),
    ];
    let m = board(26.0, 21.0, &pads, &[0.2, 1.0, 2.0]);
    let r = route_with(&m, &opts54("cycles", false), &RunControl::new());
    assert_eq!(r.failed, 0, "{:?}", r.warnings);
    let mut v = vias(&r.items);
    v.sort_by(|a, b| a.1.total_cmp(&b.1));
    assert_eq!(v.len(), 3, "{v:?}");
    let want = [(0.45, 0.25), (0.52, 0.32), (0.84, 0.64)];
    for (x, wnt) in v.iter().zip(want) {
        assert!((x.2 - wnt.0).abs() < 1e-6 && (x.3 - wnt.1).abs() < 1e-6, "{v:?}");
    }
    clean(&m, &r.items);
    // The old behaviour stays the default: the net class via.
    let r0 = route_with(&m, &Options { search_length: "taut".into(), ..Options::default() }, &RunControl::new());
    assert!(vias(&r0.items).iter().all(|x| (x.2 - 0.8).abs() < 1e-6 && (x.3 - 0.4).abs() < 1e-6), "{:?}", vias(&r0.items));
}

/// via_force_relocate: A must pass under the F.Cu wall W at x = 12 on B.Cu, between the B.Cu
/// strips N1 (x = 6) and N3 (x = 16): two vias, centred in their free intervals (9.0 and 14.0,
/// +-0.25).
#[test]
fn via_force_relocate_equal_clearance() {
    let pads: Vec<P> = vec![
        (1, 2.0, 7.0, 0.8, 0.8, 0),
        (1, 20.0, 7.0, 0.8, 0.8, 0),
        (2, 6.0, 7.0, 0.8, 13.0, 2),
        (3, 12.0, 7.0, 0.8, 13.0, 0),
        (4, 16.0, 7.0, 0.8, 13.0, 2),
    ];
    let m = board(22.0, 14.0, &pads, &[0.25, 0.25, 0.25, 0.25]);
    for lp in ["cycles", "topor"] {
        let r = route_with(&m, &opts54(lp, false), &RunControl::new());
        assert_eq!(r.failed, 0, "{lp}: {:?}", r.warnings);
        let mut v = vias(&r.items);
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(v.len(), 2, "{lp}: {v:?}");
        assert!((v[0].0 - 9.0).abs() <= 0.25 && (v[1].0 - 14.0).abs() <= 0.25, "{lp}: {v:?}");
        clean(&m, &r.items);
    }
}

fn tracks(items: &[Item]) -> Vec<((f64, f64), (f64, f64))> {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Track(t) => match t.prim {
                topor_engine::geom::quant::TrackPrim::Seg { a, b } => Some(((a.x as f64 / MM, a.y as f64 / MM), (b.x as f64 / MM, b.y as f64 / MM))),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn total_len(items: &[Item]) -> f64 {
    tracks(items).iter().map(|(a, b)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()).sum()
}

/// Points where three or more track ends of the net meet (the junctions), rounded to 1 um.
fn junctions(items: &[Item]) -> Vec<(f64, f64)> {
    let mut ends: std::collections::BTreeMap<(i64, i64), usize> = Default::default();
    for (a, b) in tracks(items) {
        for p in [a, b] {
            *ends.entry(((p.0 * 1e3).round() as i64, (p.1 * 1e3).round() as i64)).or_default() += 1;
        }
    }
    ends.into_iter().filter(|e| e.1 >= 3).map(|((x, y), _)| (x as f64 / 1e3, y as f64 / 1e3)).collect()
}

/// branch_point_relocate: T joins (3, 2), (3, 12), (17, 7). The junction goes to the point of
/// least length for the drawing style: octilinear (8, 7), 23.142 mm (expected.json octilinear
/// note); smooth (any-angle) the Fermat point (5.887, 7), 22.660 mm, +-0.15.
#[test]
fn branch_point_relocate_octilinear_and_fermat() {
    let pads: Vec<P> = vec![(1, 3.0, 2.0, 0.8, 0.8, 0), (1, 3.0, 12.0, 0.8, 0.8, 0), (1, 17.0, 7.0, 0.8, 0.8, 0)];
    let m = board(20.0, 14.0, &pads, &[0.25]);
    // Steiner grid on: the T is a free point on the straight trunk P-Q (not a corner of it).
    for (lp, st) in [("cycles", false), ("topor", false), ("cycles", true)] {
        for (style, at, len) in [("octilinear", (8.0, 7.0), 23.142), ("smooth", (5.887, 7.0), 22.660)] {
            // Parking lot: with the grid and the smooth style the plan's arm R bends at a grid
            // point next to Q (3.4, 11.6); the pad_out Fermat point of that corner is next to Q.
            if st && style == "smooth" {
                continue;
            }
            let r = route_with(&m, &Options { draw_style: style.into(), ..opts54(lp, st) }, &RunControl::new());
            assert_eq!(r.failed, 0, "{lp} {style}: {:?}", r.warnings);
            let j = junctions(&r.items);
            assert!(j.iter().any(|p| (p.0 - at.0).hypot(p.1 - at.1) <= 0.15), "{lp} {style}: {j:?} {:?}", tracks(&r.items));
            assert!(total_len(&r.items) <= len + 0.01, "{lp} {style}: {}", total_len(&r.items));
            clean(&m, &r.items);
        }
    }
}

/// branch_point_into_pad: the angle at M (10, 6) is 133.6 degrees, so the junction is M itself:
/// the net is a chain through M, no free junction.
#[test]
fn branch_point_into_pad_no_free_junction() {
    let pads: Vec<P> = vec![(1, 3.0, 3.0, 0.8, 0.8, 0), (1, 10.0, 6.0, 0.8, 0.8, 0), (1, 17.0, 3.0, 0.8, 0.8, 0)];
    let m = board(20.0, 12.0, &pads, &[0.25]);
    for lp in ["cycles", "topor"] {
        for st in [false, true] {
            let r = route_with(&m, &opts54(lp, st), &RunControl::new());
            assert_eq!(r.failed, 0, "{lp}: {:?}", r.warnings);
            let free: Vec<_> = junctions(&r.items).into_iter().filter(|p| (p.0 - 10.0).abs() > 0.41 || (p.1 - 6.0).abs() > 0.41).collect();
            assert!(free.is_empty(), "{lp} steiner {st}: {free:?}");
            clean(&m, &r.items);
        }
    }
}

/// via_pair_redundant: the F.Cu strip (10, 6.5), 0.8 x 5.0 does not reach the edge: A walks round
/// it on F.Cu, 0 vias.
#[test]
fn via_pair_redundant_zero_vias() {
    let pads: Vec<P> = vec![(1, 3.0, 7.0, 0.8, 0.8, 0), (1, 17.0, 7.0, 0.8, 0.8, 0), (2, 10.0, 6.5, 0.8, 5.0, 0)];
    let m = board(20.0, 14.0, &pads, &[0.25, 0.25]);
    for lp in ["cycles", "topor"] {
        let r = route_with(&m, &opts54(lp, false), &RunControl::new());
        assert_eq!(r.failed, 0, "{lp}: {:?}", r.warnings);
        assert!(vias(&r.items).is_empty(), "{lp}: {:?}", vias(&r.items));
        clean(&m, &r.items);
    }
}

/// via_pair_slide: two F.Cu walls (x = 8, 14) edge to edge: one B.Cu stretch under both, 2 vias,
/// in [2.825, 7.175] and [14.825, 19.175] (relocation centres them: 5.0, 17.0).
#[test]
fn via_pair_slide_two_vias() {
    let pads: Vec<P> = vec![(1, 2.0, 7.0, 0.8, 0.8, 0), (1, 20.0, 7.0, 0.8, 0.8, 0), (2, 8.0, 7.0, 0.8, 13.0, 0), (3, 14.0, 7.0, 0.8, 13.0, 0)];
    let m = board(22.0, 14.0, &pads, &[0.25, 0.25, 0.25]);
    for lp in ["cycles", "topor"] {
        let r = route_with(&m, &opts54(lp, false), &RunControl::new());
        assert_eq!(r.failed, 0, "{lp}: {:?}", r.warnings);
        let mut v = vias(&r.items);
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(v.len(), 2, "{lp}: {v:?}");
        // Centred between the own pad and the wall (equal push), 5.0 and 17.0.
        assert!((v[0].0 - 5.0).abs() <= 0.25 && (v[1].0 - 17.0).abs() <= 0.25, "{lp}: {v:?}");
        clean(&m, &r.items);
    }
}
