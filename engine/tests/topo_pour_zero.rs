//! Routing over a connected own-net pour has no length (user 2026-11-12: "Routing over pours
//! should be zero length, because the trace ends there, given that the pour is connected to the
//! rest of the net"). A plane pad no stub serves is wired to its net's pour, and the trace ends
//! where it first reaches the pour's connected fill; an island of the pour that is cut off from
//! the rest of the net is no terminal (the pad would float).

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::route_with;

const MM: f64 = 1e6;

/// `(net, layer id, x0, y0, x1, y1, island_removal)`: a pour filled over its rectangle.
type PourSpec = (i32, u32, f64, f64, f64, f64, i32);

/// Two-layer board `w` x `h` mm (KiCad frame), square SMD pads `(ref, net, x, y, size)` on F.Cu
/// (net 0 = no net), nets 1 = GND, 2 = VCC, and pours that list no pads.
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
        .enumerate()
        .map(|(i, &(r, net, x, y, s))| {
            format!(
                r#"{{"uuid":"p{i}","ref":"{r}","number":"1","net":{net},"attr":"smd","center":[{},{}],"rotation_deg":0.0,
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
        .map(|(i, &(net, l, x0, y0, x1, y1, irm))| {
            let ring = format!("[[{},{}],[{},{}],[{},{}],[{},{}]]", p(x0), p(y0), p(x1), p(y0), p(x1), p(y1), p(x0), p(y1));
            format!(
                r#"{{"uuid":"z{i}","net":{net},"layers":[{l}],"pads":[],"vias":[],"clearance":200000,"min_thickness":250000,
                    "thermal_gap":500000,"spoke_width":500000,"pad_connection":2,"island_removal":{irm},"fills":[{{"layer":{l},"outer":{ring}}}]}}"#
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

/// Track ends `(x, y, layer)` in mm (KiCad frame) of net `net`.
fn track_ends(items: &[Item], net: i32) -> Vec<(f64, f64, u32)> {
    items
        .iter()
        .filter_map(|i| if let Item::Track(t) = i { Some(t) } else { None })
        .filter(|t| t.net == net)
        .flat_map(|t| {
            let (a, b) = (t.prim.start(), t.prim.end());
            [(a.x as f64 / MM, a.y as f64 / MM, t.layer), (b.x as f64 / MM, b.y as f64 / MM, t.layer)]
        })
        .collect()
}

fn track_len(items: &[Item], net: i32) -> f64 {
    items
        .iter()
        .filter_map(|i| if let Item::Track(t) = i { Some(t) } else { None })
        .filter(|t| t.net == net)
        .map(|t| {
            let (a, b) = (t.prim.start(), t.prim.end());
            ((b.x - a.x) as f64).hypot((b.y - a.y) as f64) / MM
        })
        .sum()
}

/// A GND pad 1.5 mm below a small GND island on F.Cu (no pad in it, "remove islands: never") and
/// 6 mm from the GND pour on B.Cu (the net's main copper). The island is no terminal: the pad is
/// wired to the B.Cu pour (a via and a trace ending on its fill), not only stubbed into the
/// island. After routing the
/// island is joined to the pad by a short stub (`[planes] join_islands`).
#[test]
fn a_pad_is_not_stubbed_into_a_cut_off_island() {
    let m = board(30.0, 20.0, &[("R1", 1, 10.0, 10.0, 1.0)], &[(1, 0, 9.0, 5.5, 11.0, 8.0, 1), (1, 2, 16.0, 0.5, 29.5, 19.5, 0)]);
    let on = route_with(&m, &Options::default(), &RunControl::new());
    assert_eq!(on.routed, 1, "{:?}", on.warnings);
    let ends = track_ends(&on.items, 1);
    let in_island = |&(x, y, l): &(f64, f64, u32)| l == 0 && (9.0..=11.0).contains(&x) && (5.5..=8.0).contains(&y);
    let on_plane = |&(x, _, l): &(f64, f64, u32)| l == 2 && x >= 16.0;
    assert!(ends.iter().any(on_plane), "the pad is wired to the B.Cu pour: {ends:?}");
    // planes.join_islands (default, user 2026-11-12): the island, no longer isolated copper, is
    // joined to the pad (now connected through its via) by a short stub after routing.
    assert!(ends.iter().any(in_island), "the island is joined to R1: {ends:?}");
    assert!(on.warnings.iter().any(|w| w.starts_with("island join R1.1")), "{:?}", on.warnings);
    let nojoin = route_with(&m, &Options { plane_join_islands: false, ..Options::default() }, &RunControl::new());
    let nj = track_ends(&nojoin.items, 1);
    assert!(nj.iter().any(on_plane) && !nj.iter().any(in_island), "join_islands off: the island is only reported: {nj:?}");
    // Before (planes.terminals off): the island counts as the net's copper, a stub ends in it
    // and nothing reaches the B.Cu pour (the pad floats).
    let off = route_with(&m, &Options { plane_terminals: false, ..Options::default() }, &RunControl::new());
    let off_ends = track_ends(&off.items, 1);
    assert!(off_ends.iter().any(in_island) && !off_ends.iter().any(on_plane), "{off_ends:?}");
}

/// A VCC pad below a VCC pour (F.Cu, x 5..25, y 2..10), with a row of unconnected pads between
/// them (y 14, x 3.5..23.6, gaps too narrow for a trace). The trace ends where its shortest path
/// first reaches the pour (round the row's right end, straight up to the pour's lower edge, about
/// 14.7 mm), not at the pour point nearest the pad (behind the row, about 19.3 mm).
#[test]
fn a_far_pad_ends_where_its_path_first_reaches_the_pour() {
    let mut pads = vec![("J1", 2, 15.0, 18.0, 1.0)];
    let row: Vec<String> = (0..16).map(|k| format!("X{k}")).collect();
    for (k, r) in row.iter().enumerate() {
        pads.push((r.as_str(), 0, 4.0 + 1.27 * k as f64, 14.0, 1.0));
    }
    let m = board(30.0, 22.0, &pads, &[(2, 0, 5.0, 2.0, 25.0, 10.0, 0)]);
    let on = route_with(&m, &Options::default(), &RunControl::new());
    assert_eq!(on.routed, 1, "{:?}", on.warnings);
    let len = track_len(&on.items, 2);
    let ends = track_ends(&on.items, 2);
    let far_end = ends.iter().map(|e| e.0).fold(f64::MIN, f64::max);
    assert!(len < 17.0 && far_end > 20.0, "the trace ends near the pour's lower right corner: {len:.2} mm, ends {ends:?}");
}
