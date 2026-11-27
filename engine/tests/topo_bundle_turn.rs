//! bundle-turn r1 (docs/dev/design/bundle-turn.md): the per-edge capacity counts an edge-layer
//! over-full only when its wires do not fit crossing square (item 4, `[capacity] oblique_price`),
//! and the report lines of the re-fit after the straightening (item 3).

use topor_engine::control::RunControl;
use topor_engine::geom::quant::TrackPrim;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::core::{refit_line, straighten_line};
use topor_engine::topo::pack::{PackItem, Packer};

const MM: f64 = 1e6;

/// A 10 x 10 mm two-layer board with two through pads of one net: some CDT edges to pack on.
fn tiny() -> BoardModel {
    let p = |v: f64| (v * MM) as i64;
    let c = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pad = |i: usize, x: f64, y: f64| {
        format!(
            r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":1,"attr":"pth","center":[{},{}],"rotation_deg":0.0,"layers":[0, 2],
                "shapes":{{"0":{{"kind":"rect","w":{d},"h":{d}}},"2":{{"kind":"rect","w":{d},"h":{d}}}}},"drill":{{"w":{r},"h":{r}}}}}"#,
            p(x),
            p(y),
            d = p(1.0),
            r = p(0.5)
        )
    };
    let s = format!(
        r#"{{"units":"nm","board":"t","layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"A"}}],"pads":[{},{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
        pad(0, 2.0, 5.0),
        pad(1, 8.0, 5.0),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn track(net: i32, infl: f64) -> PackItem {
    PackItem { net, w: 0.25 * MM, s: 0.2 * MM, infl, disc: [0.0; 3], lean: 0.0 }
}

/// The over-fill of two tracks packed into the span `0..d` of edge `e` (both items alike).
fn worst(pk: &Packer, e: usize, items: &[PackItem], d: f64) -> f64 {
    let (lo, hi) = (vec![0.0; items.len()], vec![d; items.len()]);
    pk.worst_into(e, items, &lo, &hi, &mut Vec::new(), &mut Vec::new())
}

#[test]
fn oblique_overage_is_a_price_and_over_full_only_when_it_does_not_fit_square() {
    let m = tiny();
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let on = Packer::for_model(&m, &Options::default(), &b.proj);
    let off = Packer::for_model(&m, &Options { oblique_price: 0.0, ..Options::default() }, &b.proj);
    let w2 = Packer::for_model(&m, &Options { oblique_price: 2.0, ..Options::default() }, &b.proj);
    assert_eq!(Options::default().oblique_price, 1.0, "on by default");
    let e = (0..b.proj.edges.len()).max_by(|&a, &c| on.edge_len(a).total_cmp(&on.edge_len(c))).unwrap();
    let obl = [track(2, 1.5), track(3, 1.5)];
    let sq = [track(2, 1.0), track(3, 1.0)];
    let split = |pk: &Packer, d: f64, items: &[PackItem]| {
        let (lo, hi) = (vec![0.0; items.len()], vec![d; items.len()]);
        pk.oblique_split(e, items, &vec![1.0; items.len()], &lo, &hi, worst(pk, e, items, d), &mut Vec::new(), &mut Vec::new(), &mut Vec::new())
    };
    // The span fits the two tracks square but not at 1 / sin = 1.5.
    let req1 = worst(&on, e, &sq, 0.0);
    let d = req1 * 1.2;
    let (o_obl, o_sq) = (worst(&on, e, &obl, d), worst(&on, e, &sq, d));
    assert!(o_obl > 1_000.0 && o_sq < -1_000.0, "test setup: oblique over {o_obl}, square over {o_sq}");
    let (over, price) = split(&on, d, &obl);
    assert_eq!(over, o_sq, "fits square: not over-full");
    assert!((price - o_obl / MM).abs() < 1e-9, "price = the oblique overage in mm at weight 1: {price}");
    let (_, p2) = split(&w2, d, &obl);
    assert!((p2 - 2.0 * price).abs() < 1e-9, "weighted: {p2}");
    // Off: the oblique overage is the over-fill, no price.
    assert_eq!(split(&off, d, &obl), (o_obl, 0.0));
    // Not fitting even square: over-full by the square over-fill, the rest priced.
    let d2 = req1 * 0.8;
    let (o_obl2, o_sq2) = (worst(&on, e, &obl, d2), worst(&on, e, &sq, d2));
    assert!(o_sq2 > 1_000.0);
    let (over2, price2) = split(&on, d2, &obl);
    assert_eq!(over2, o_sq2);
    assert!((price2 - (o_obl2 - o_sq2) / MM).abs() < 1e-9, "{price2}");
    // Unchanged where it fits at the taut angles, and with square items.
    let d3 = req1 * 2.0;
    let o3 = worst(&on, e, &obl, d3);
    assert!(o3 < 0.0);
    assert_eq!(split(&on, d3, &obl), (o3, 0.0));
    assert_eq!(split(&on, d2, &sq), (o_sq2, 0.0));
    // Continuous: the price grows from 0 as the span shrinks past the oblique fit.
    let mut last = 0.0;
    for i in 0..20 {
        let di = req1 * 1.5 - i as f64 * req1 * 0.03;
        let (_, pi) = split(&on, di, &obl);
        assert!(pi >= last - 1e-12 && pi - last < 0.05 * req1 / MM + 1e-9, "step {i}: {last} -> {pi}");
        last = pi;
    }
}

#[test]
fn refit_and_straighten_lines_report_convergence() {
    assert_eq!(
        refit_line(3, 2, 1, false, 0),
        "never drop, re-fit after the straightening: 3 wires drawn with violations drawn again against the straightened neighbours; 2 kept (fewer violations, or shorter at equal violations), 1 of them now without violations"
    );
    assert!(refit_line(1, 0, 0, false, 4).ends_with("; 4 not drawn again (no violation partner and no wire on their planned edges moved)"));
    assert!(refit_line(3, 2, 1, true, 0).ends_with("stopped at Cancel or the time limit ([run] drawing_grace_s) before the rest (they keep their copper)"));
    assert!(straighten_line(2, 0.3, 0, 7, 7, false, 0.1).contains("stopped at the bound of 7 passes with changes in the last ([drawing] straighten_passes)"));
    assert!(straighten_line(2, 0.3, 0, 3, 7, true, 0.1).contains("3 passes, the last without a change"));
}

// ------------------------------------------------------------------------------ end to end
// The patterns of harness/bundle_turn_patterns.py (APPROVED, user 2026-11-27) (models:
// harness.radii_patterns.model_fixture, engine/tests/fixtures/bundle_turn), routed det s0 as the
// harness does (one run, one round).

/// Per net code: (net, length mm, bends, segments off 0/45/90 degrees).
type NetShape = (i32, f64, usize, usize);

/// Per net code its [`NetShape`], the violations per connection and the unrouted count.
fn route_pattern(p: &str) -> (Vec<NetShape>, Vec<(String, usize)>, usize) {
    let path = format!("{}/tests/fixtures/bundle_turn/model-{p}.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["rounds"] = serde_json::json!(1);
    let o: Options = serde_json::from_value(val).unwrap();
    let r = topor_engine::route::route_with(&m, &o, &RunControl::new());
    let mut out = Vec::new();
    for net in m.nets.iter().map(|n| n.code) {
        let mut segs: Vec<((f64, f64), (f64, f64))> = r
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Track(t) if t.net == net => match t.prim {
                    TrackPrim::Seg { a, b } => Some(((a.x as f64 / MM, a.y as f64 / MM), (b.x as f64 / MM, b.y as f64 / MM))),
                    _ => None,
                },
                _ => None,
            })
            .filter(|(a, b)| (a.0 - b.0).hypot(a.1 - b.1) > 1e-6)
            .collect();
        // Chain from the source pad S (the end nearest the board's top left).
        let d = |p: (f64, f64), q: (f64, f64)| (p.0 - q.0).hypot(p.1 - q.1);
        let ends: Vec<(f64, f64)> = segs.iter().flat_map(|s| [s.0, s.1]).collect();
        let mut cur = ends.iter().copied().filter(|e| ends.iter().filter(|f| d(*e, **f) < 1e-6).count() == 1).min_by(|a, b| (a.0 + a.1).total_cmp(&(b.0 + b.1))).unwrap_or((0.0, 0.0));
        let mut dirs: Vec<f64> = Vec::new();
        let (mut len, mut off) = (0.0, 0usize);
        while !segs.is_empty() {
            let k = (0..segs.len()).min_by(|&i, &j| d(cur, segs[i].0).min(d(cur, segs[i].1)).total_cmp(&d(cur, segs[j].0).min(d(cur, segs[j].1)))).unwrap();
            let (mut a, mut b) = segs.remove(k);
            if d(cur, b) < d(cur, a) {
                std::mem::swap(&mut a, &mut b);
            }
            let ang = (b.1 - a.1).atan2(b.0 - a.0).to_degrees();
            let r45 = (ang / 45.0).round() * 45.0;
            if (ang - r45).abs() > 0.5 {
                off += 1;
            }
            if dirs.last().is_none_or(|&x| (x - ang).abs() > 0.5) {
                dirs.push(ang);
            }
            len += d(a, b);
            cur = b;
        }
        out.push((net, len, dirs.len().saturating_sub(1), off));
    }
    let viols = r.connections.iter().map(|c| (c.name.clone(), c.violations)).collect();
    (out, viols, r.connections_total - r.routed)
}

/// turn_fork_corner, the parts items 4 and 1 fix: N3 keeps the corridor (P4 no longer rips it out
/// over the oblique factor; before: 7.99 mm round the wall), no violation, every wire within 1 % of
/// the reference length (5.2718 / 6.1115 / 6.7243 mm), and N2, drawn before N1 turns off, keeps
/// N1's fork room: its reference 3 bends (item 1 off, `TOPO_FD45_OFF=fork`: 4, N2 bends into the
/// room N1 needs after the fork and steps back).
#[test]
fn turn_fork_corner_no_violation_no_detour() {
    let (nets, viols, unrouted) = route_pattern("turn_fork_corner");
    assert_eq!(unrouted, 0);
    assert!(viols.iter().all(|v| v.1 == 0), "violations {viols:?}");
    for (net, l) in [(1, 5.2718), (2, 6.1115), (3, 6.7243)] {
        let &(_, g, _, _) = nets.iter().find(|x| x.0 == net).unwrap();
        assert!(g <= l * 1.01, "N{net}: {g:.4} mm vs the reference {l} ({nets:?})");
    }
    let &(_, _, b2, _) = nets.iter().find(|x| x.0 == 2).unwrap();
    assert!(b2 <= 3, "N2: {b2} bends, the reference 3 ({nets:?})");
}

/// turn_fork_corner in full: the bundle nests at pitch, every leg octilinear, no more bends than
/// the reference (N1 2, N2 3, N3 3).
#[test]
#[ignore = "N2 keeps a free-angle leg: open_strip's wall at undrawn N1's zero-width planned crossing (ERBS 37 um off N1's taut line) blocks the nested elbow; follow-up: ERBS bands spaced at pitch before drawing (lead 2026-11-27)"]
fn turn_fork_corner_matches_expected() {
    let (nets, viols, unrouted) = route_pattern("turn_fork_corner");
    assert_eq!(unrouted, 0);
    assert!(viols.iter().all(|v| v.1 == 0), "violations {viols:?}");
    let refs = [(1, 5.2718, 2), (2, 6.1115, 3), (3, 6.7243, 3)];
    for (net, l, bends) in refs {
        let &(_, g, b, off) = nets.iter().find(|x| x.0 == net).unwrap();
        assert!(g <= l * 1.01, "N{net}: {g:.4} mm vs the reference {l} ({nets:?})");
        assert_eq!(off, 0, "N{net}: legs off 0/45/90 degrees ({nets:?})");
        assert!(b <= bends, "N{net}: {b} bends, the reference {bends} ({nets:?})");
    }
}

/// hole_bundle_room: one bend per wire, nested at pitch, A at clearance to the hole; the only
/// violation B's in the pocket entrance (expected.json).
#[test]
#[ignore = "judged after via-site-truth lands: B is planned with two P3 vias onto the keepout-closed B.Cu (lead 2026-11-27)"]
fn hole_bundle_room_matches_expected() {
    let (nets, _viols, unrouted) = route_pattern("hole_bundle_room");
    assert_eq!(unrouted, 0);
    for &(net, _, b, off) in &nets {
        assert_eq!(off, 0, "net {net}: legs off 0/45/90 degrees ({nets:?})");
        assert!(b <= 1, "net {net}: {b} bends, the reference 1 ({nets:?})");
    }
}
