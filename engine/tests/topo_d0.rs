//! D0 drawing witness: known-answer tests on tiny synthetic boards (hand traces on a 10 x 10 mm
//! board, the R0 builder). The hand copper is DRC-clean, so variant A must draw every wire.

use topor_engine::model::BoardModel;
use topor_engine::topo::dwitness::{run, DwOpts, DwOut, Variant};

const MM: f64 = 1e6;

fn p(v: f64) -> i64 {
    (v * MM) as i64
}

/// Through-hole square pad: net, x, y, size.
struct Th(i32, f64, f64, f64);

/// Hand track: net, layer id (0 = F, 2 = B), width, a, b (mm, KiCad frame).
type Track = (i32, u32, f64, (f64, f64), (f64, f64));

/// Two-layer board with through-hole pads, hand tracks and vias `(net, x, y)` (0.45 mm, drill 0.3).
fn board(pads: &[Th], tracks: &[Track], vias: &[(i32, f64, f64)]) -> BoardModel {
    let vias: Vec<String> = vias
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y))| format!(r#"{{"uuid":"v{i}","net":{net},"at":[{},{}],"d":450000,"drill":300000,"layers":[0,2]}}"#, p(x), p(y)))
        .collect();
    let c = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let outline: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, Th(net, x, y, d))| {
            let sh = format!(r#"{{"kind":"rect","w":{0},"h":{0}}}"#, p(*d));
            format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"pth","center":[{},{}],"layers":[0,2],
                   "shapes":{{"0":{sh},"2":{sh}}},"drill":{{"w":{dr},"h":{dr}}}}}"#,
                p(*x),
                p(*y),
                dr = p(*d / 2.0)
            )
        })
        .collect();
    let kept: Vec<String> = tracks
        .iter()
        .enumerate()
        .map(|(i, &(net, layer, w, a, b))| {
            format!(
                r#"{{"uuid":"k{i}","net":{net},"layer":{layer},"width":{},"prim":{{"kind":"seg","a":[{},{}],"b":[{},{}]}}}}"#,
                p(w),
                p(a.0),
                p(a.1),
                p(b.0),
                p(b.1)
            )
        })
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}},{{"code":3,"name":"C"}}],
            "pads":[{}],"vias":[{}],"kept":[{}],"outline":[{}],"rules":{{}}}}"#,
        pads.join(","),
        vias.join(","),
        kept.join(","),
        outline.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn witness(m: &BoardModel, v: Variant) -> DwOut {
    run(m, &DwOpts { variant: v, ..DwOpts::default() }).unwrap()
}

/// Two 0.2 mm tracks of different nets, 0.6 mm apart (0.4 mm copper gap, clearance 0.2), between
/// two unconnected pads of net 3 at x = 5 (the CDT edge joining them is crossed by both).
fn parallel() -> BoardModel {
    board(
        &[Th(1, 1.0, 4.7, 0.5), Th(1, 9.0, 4.7, 0.5), Th(2, 1.0, 5.3, 0.5), Th(2, 9.0, 5.3, 0.5), Th(3, 5.0, 3.6, 0.6), Th(3, 5.0, 6.4, 0.6)],
        &[(1, 0, 0.2, (1.0, 4.7), (9.0, 4.7)), (2, 0, 0.2, (1.0, 5.3), (9.0, 5.3))],
        &[],
    )
}

#[test]
fn parallel_hand_tracks_are_all_drawn() {
    for v in [Variant::A, Variant::B, Variant::C] {
        let r = witness(&parallel(), v);
        assert_eq!((r.wires_in, r.drawn, r.rejected), (2, 2, 0), "{v:?}: {:?}", r.reasons);
    }
}

#[test]
fn the_audit_sees_the_two_neighbours_at_their_hand_gap() {
    let r = witness(&parallel(), Variant::A);
    // Every audited pair is the two tracks; their copper gap is the hand's 0.4 mm, so the slack
    // over the 0.2 mm clearance is 0.2 mm (the taut wires run straight between their pads).
    assert!(!r.audit.is_empty());
    for a in &r.audit {
        assert!((a.slack_nm - 0.2e6).abs() < 5_000.0, "{a:?}");
    }
}

/// Net 1 on F from the left pad to a via at x = 5, then on B to the right pad.
fn layer_change() -> BoardModel {
    board(
        &[Th(1, 1.0, 5.0, 0.6), Th(1, 9.0, 5.0, 0.6)],
        &[(1, 0, 0.2, (1.0, 5.0), (5.0, 5.0)), (1, 2, 0.2, (5.0, 5.0), (9.0, 5.0))],
        &[(1, 5.0, 5.0)],
    )
}

#[test]
fn a_hand_via_is_placed_and_the_wire_drawn() {
    let r = witness(&layer_change(), Variant::A);
    assert_eq!((r.wires_in, r.drawn, r.vias_ok, r.vias_failed), (1, 1, 1, 0), "{:?}", r.reasons);
    // Variant C places the via itself in the window around the hand's via.
    let r = witness(&layer_change(), Variant::C);
    assert_eq!((r.drawn, r.vias_ok), (1, 1), "{:?}", r.reasons);
}

#[test]
fn a_tee_with_a_node_terminal_draws_without_panicking() {
    // Three chains meet at the junction (5, 5): node terminals, no pad to neck into.
    let m = board(
        &[Th(1, 1.0, 5.0, 0.6), Th(1, 9.0, 5.0, 0.6), Th(1, 5.0, 9.0, 0.6)],
        &[(1, 0, 0.2, (1.0, 5.0), (5.0, 5.0)), (1, 0, 0.2, (5.0, 5.0), (9.0, 5.0)), (1, 0, 0.2, (5.0, 5.0), (5.0, 9.0))],
        &[],
    );
    let r = witness(&m, Variant::A);
    assert_eq!((r.wires_in, r.drawn), (3, 3), "{:?}", r.reasons);
}
