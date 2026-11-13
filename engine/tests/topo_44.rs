//! 4.4 (planes as terminals, user rule 2026-11-10): an own-net pour is a terminal of its net like
//! a pad, one that may carry vias. A pad no stub serves is wired to the pour's filled copper, not
//! to another pad of the net. Plane pads may share a via onto the plane, sited where their stubs
//! are shortest together, except under a ball-grid footprint, where every ball gets its own via
//! (`[planes] share_vias` = "outside_bga", user 2026-11-13); "never" = one via per pad (the rule
//! of 2026-11-12).

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::route_with;

const MM: f64 = 1e6;

/// `(net, layer id, x0, y0, x1, y1, listed pads)`.
type PourSpec<'a> = (i32, u32, f64, f64, f64, f64, &'a [&'a str]);

/// Two-layer board `w` x `h` mm (KiCad frame) with square SMD pads `(ref, net, x, y, size)` on
/// F.Cu, nets 1 = GND, 2 = VCC, and pours `(net, layer id, x0, y0, x1, y1, listed pads)`, each
/// filled over its rectangle.
fn board(w: f64, h: f64, pads: &[(&str, i32, f64, f64, f64)], pours: &[PourSpec]) -> BoardModel {
    let p = |v: f64| (v * MM) as i64;
    let c = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pj: Vec<String> = pads
        .iter()
        .map(|&(r, net, x, y, s)| {
            format!(
                r#"{{"uuid":"{r}","ref":"{r}","number":"1","net":{net},"attr":"smd","center":[{},{}],"rotation_deg":0.0,
                    "layers":[0],"shapes":{{"0":{{"kind":"rect","w":{},"h":{}}}}}}}"#,
                p(x),
                p(y),
                p(s),
                p(s)
            )
        })
        .collect();
    let zj: Vec<String> = pours
        .iter()
        .enumerate()
        .map(|(i, &(net, l, x0, y0, x1, y1, listed))| {
            let ring = format!("[[{},{}],[{},{}],[{},{}],[{},{}]]", p(x0), p(y0), p(x1), p(y0), p(x1), p(y1), p(x0), p(y1));
            let listed: Vec<String> = listed.iter().map(|s| format!(r#""{s}""#)).collect();
            format!(
                r#"{{"uuid":"z{i}","net":{net},"layers":[{l}],"pads":[{}],"vias":[],"clearance":200000,"min_thickness":250000,
                    "thermal_gap":500000,"spoke_width":500000,"pad_connection":2,"fills":[{{"layer":{l},"outer":{ring}}}]}}"#,
                listed.join(",")
            )
        })
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"GND"}},{{"code":2,"name":"VCC"}}],
            "pads":[{}],"pours":[{}],"outline":[{}],
            "rules":{{"default":{{"clearance":200000,"width":250000}},"min_connection":127000,"max_error":5000,"per_net":{{}}}}}}"#,
        pj.join(","),
        zj.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn vias(items: &[Item]) -> Vec<(f64, f64)> {
    items.iter().filter_map(|i| if let Item::Via(v) = i { Some((v.at.x as f64 / MM, v.at.y as f64 / MM)) } else { None }).collect()
}

/// plane_via_3r in small: three GND pads round (12, 12), 1.5 mm out at 120 degrees, a GND pour on
/// B.Cu over the board. No BGA, so by default (share_vias = "outside_bga") one via shared by the
/// three stubs, at the centre; "never" (the 2026-11-12 rule): one via per pad.
#[test]
fn three_plane_pads_share_one_via_unless_never() {
    let a = |deg: f64| {
        let r = deg.to_radians();
        (12.0 + 1.5 * r.cos(), 12.0 + 1.5 * r.sin())
    };
    let (p1, p2, p3) = (a(-90.0), a(30.0), a(150.0));
    let m = board(24.0, 24.0, &[("R1", 1, p1.0, p1.1, 1.0), ("R2", 1, p2.0, p2.1, 1.0), ("R3", 1, p3.0, p3.1, 1.0)], &[(1, 2, 0.5, 0.5, 23.5, 23.5, &[])]);
    let def = route_with(&m, &Options::default(), &RunControl::new());
    let v = vias(&def.items);
    assert_eq!(def.routed, 3, "{:?}", def.warnings);
    assert_eq!(v.len(), 1, "one shared plane via: {v:?}");
    assert!((v[0].0 - 12.0).hypot(v[0].1 - 12.0) < 0.15, "the via stands at the pads' median: {v:?}");
    let never = route_with(&m, &Options { plane_share_vias: "never".into(), ..Options::default() }, &RunControl::new());
    assert_eq!((never.routed, vias(&never.items).len()), (3, 3), "one via per plane pad: {:?}", never.warnings);
    // planes.terminals off (stubs into any fill): the sharing is the same.
    let off = route_with(&m, &Options { plane_terminals: false, ..Options::default() }, &RunControl::new());
    assert_eq!((off.routed, vias(&off.items).len()), (3, 1));
}

/// A 4 x 4 ball grid U1 (pitch 1.27 mm, all GND) over a B.Cu GND pour: by default every ball gets
/// a via of its own (user 2026-11-13: "no shared vias under bga chips"); "always" shares.
#[test]
fn bga_balls_get_a_via_each_by_default() {
    let mut pads = Vec::new();
    for i in 0..4 {
        for j in 0..4 {
            pads.push(("U1", 1, 8.0 + 1.27 * i as f64, 8.0 + 1.27 * j as f64, 0.5));
        }
    }
    let m = board(24.0, 24.0, &pads, &[(1, 2, 0.5, 0.5, 23.5, 23.5, &[])]);
    assert_eq!(topor_engine::topo::planes::bga_areas(&m).len(), 1);
    let def = route_with(&m, &Options::default(), &RunControl::new());
    assert_eq!(vias(&def.items).len(), def.routed, "one via per ball: {:?}", def.warnings);
    let always = route_with(&m, &Options { plane_share_vias: "always".into(), ..Options::default() }, &RunControl::new());
    assert!(vias(&always.items).len() < always.routed, "\"always\" shares under the BGA: {:?}", always.warnings);
}

/// The detector: a ring of 16 pads (QFN-like, 2 neighbours each) and a 2 x 10 header are no
/// ball grid; a 4 x 4 grid with its centre 2 x 2 missing (12 pads) is too small.
#[test]
fn rings_and_headers_are_no_bga() {
    let mut ring = Vec::new();
    for k in 0..5 {
        let t = 5.0 + 0.5 * k as f64;
        ring.extend([("Q1", 1, t, 4.0, 0.3), ("Q1", 1, t, 7.0, 0.3), ("Q1", 1, 4.0, t + 0.25, 0.3), ("Q1", 1, 8.5, t + 0.25, 0.3)]);
    }
    let mut hdr = Vec::new();
    for k in 0..10 {
        hdr.extend([("J1", 1, 12.0, 4.0 + 2.54 * k as f64, 1.0), ("J1", 1, 14.54, 4.0 + 2.54 * k as f64, 1.0)]);
    }
    let mut hollow = Vec::new();
    for i in 0..4 {
        for j in 0..4 {
            if !((1..3).contains(&i) && (1..3).contains(&j)) {
                hollow.push(("U2", 1, 20.0 + 1.0 * i as f64, 4.0 + 1.0 * j as f64, 0.5));
            }
        }
    }
    let all: Vec<_> = ring.iter().chain(&hdr).chain(&hollow).copied().collect();
    let m = board(40.0, 40.0, &all, &[]);
    assert!(topor_engine::topo::planes::bga_areas(&m).is_empty());
}

/// A VCC pad 15 mm left of a VCC pour (F.Cu, x >= 18) that holds the other VCC pad: the pad is
/// wired to the pour's near edge (the trace ends inside the fill, ~15.5 mm), not through the
/// pour to the other pad (20 mm).
#[test]
fn a_pad_out_of_stub_reach_is_wired_to_its_pour_not_to_a_pad() {
    let m = board(30.0, 20.0, &[("J1", 2, 3.0, 10.0, 1.0), ("C1", 2, 23.0, 10.0, 1.0)], &[(2, 0, 18.0, 0.5, 29.5, 19.5, &["C1.1"])]);
    let on = route_with(&m, &Options::default(), &RunControl::new());
    assert_eq!(on.routed, 1, "{:?}", on.warnings);
    let xmax = on
        .items
        .iter()
        .filter_map(|i| if let Item::Track(t) = i { Some(t) } else { None })
        .flat_map(|t| {
            let (a, b) = (t.prim.start(), t.prim.end());
            [a.x as f64 / MM, b.x as f64 / MM]
        })
        .fold(f64::MIN, f64::max);
    assert!((18.0..20.0).contains(&xmax), "the trace ends just inside the pour, not at C1 (x 23): {xmax}");
    assert!(vias(&on.items).is_empty());
    let off = route_with(&m, &Options { plane_terminals: false, ..Options::default() }, &RunControl::new());
    let xmax_off = off.items.iter().filter_map(|i| if let Item::Track(t) = i { Some(t) } else { None }).flat_map(|t| {
        let (a, b) = (t.prim.start(), t.prim.end());
        [a.x as f64 / MM, b.x as f64 / MM]
    }).fold(f64::MIN, f64::max);
    assert!(xmax_off > 22.0, "without terminals the pad is wired to C1: {xmax_off}");
}
