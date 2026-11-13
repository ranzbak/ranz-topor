//! 4.4 (planes as terminals, user rule 2026-11-10): an own-net pour is a terminal of its net like
//! a pad, one that may carry vias. Plane pads share a via onto the plane, sited where their stubs
//! are shortest together; a pad no stub serves is wired to the pour's filled copper, not to
//! another pad of the net.

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
/// B.Cu over the board. One via shared by the three stubs, at the centre; one via per pad with
/// `[planes] terminals` off.
#[test]
fn three_plane_pads_share_one_via_at_their_median() {
    let a = |deg: f64| {
        let r = deg.to_radians();
        (12.0 + 1.5 * r.cos(), 12.0 + 1.5 * r.sin())
    };
    let (p1, p2, p3) = (a(-90.0), a(30.0), a(150.0));
    let m = board(24.0, 24.0, &[("R1", 1, p1.0, p1.1, 1.0), ("R2", 1, p2.0, p2.1, 1.0), ("R3", 1, p3.0, p3.1, 1.0)], &[(1, 2, 0.5, 0.5, 23.5, 23.5, &[])]);
    let on = route_with(&m, &Options::default(), &RunControl::new());
    let v = vias(&on.items);
    assert_eq!(on.routed, 3, "{:?}", on.warnings);
    assert_eq!(v.len(), 1, "one shared plane via: {v:?}");
    assert!((v[0].0 - 12.0).hypot(v[0].1 - 12.0) < 0.15, "the via stands at the pads' median: {v:?}");
    let off = route_with(&m, &Options { plane_terminals: false, ..Options::default() }, &RunControl::new());
    assert_eq!((off.routed, vias(&off.items).len()), (3, 3));
}

/// verify-m2 F2: the soft time limit (M2.TL, default) cuts no step, the plane stubs included
/// (placed first, before the topology): a limit already passed when the run begins places every
/// stub, as without a limit; the initial routing needs them for a complete plan.
#[test]
fn a_soft_limit_passed_before_the_run_places_every_plane_stub() {
    let a = |deg: f64| {
        let r = deg.to_radians();
        (12.0 + 1.5 * r.cos(), 12.0 + 1.5 * r.sin())
    };
    let (p1, p2, p3) = (a(-90.0), a(30.0), a(150.0));
    let m = board(24.0, 24.0, &[("R1", 1, p1.0, p1.1, 1.0), ("R2", 1, p2.0, p2.1, 1.0), ("R3", 1, p3.0, p3.1, 1.0)], &[(1, 2, 0.5, 0.5, 23.5, 23.5, &[])]);
    let planes = |r: &topor_engine::route::RouteResult| r.warnings.iter().find(|w| w.starts_with("planes as terminals")).cloned();
    let free = route_with(&m, &Options::default(), &RunControl::new());
    assert!(planes(&free).is_some_and(|w| w.ends_with("1 stub via(s) for 3 plane pad(s)")), "{:?}", free.warnings);
    let ctl = RunControl::new();
    ctl.limit.init(1e-6);
    let late = route_with(&m, &Options::default(), &ctl);
    assert!(late.limit.as_ref().is_some_and(|l| l.reached && l.mode == "soft" && l.cut == "none"), "{:?}", late.limit);
    assert_eq!(planes(&late), planes(&free), "the soft limit cut the plane stubs");
    assert_eq!((late.routed, vias(&late.items)), (free.routed, vias(&free.items)));
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
