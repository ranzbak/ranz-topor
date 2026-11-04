//! R0 witness: known-answer tests on tiny synthetic boards (hand traces on a 10 x 10 mm board).

use topor_engine::model::BoardModel;
use topor_engine::topo::witness::{run, Witness, WitnessOpts};

const MM: f64 = 1e6;

fn p(v: f64) -> i64 {
    (v * MM) as i64
}

enum Pad {
    /// Through-hole square, net, x, y, size.
    Th(i32, f64, f64, f64),
    /// SMD rectangle on F, net, x, y, w, h.
    Smd(i32, f64, f64, f64, f64),
    /// SMD convex polygon on F, net, centre, corners.
    Poly(i32, (f64, f64), Vec<(f64, f64)>),
}

type Track = (i32, u32, f64, (f64, f64), (f64, f64));

/// Two-layer board; tracks are (net, layer id 0 = F / 2 = B, width, a, b) in mm, KiCad frame.
fn board(pads: &[Pad], tracks: &[Track]) -> BoardModel {
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
        .map(|(i, pd)| match pd {
            Pad::Th(net, x, y, d) => {
                let sh = format!(r#"{{"kind":"rect","w":{0},"h":{0}}}"#, p(*d));
                format!(
                    r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"pth","center":[{},{}],"layers":[0,2],
                       "shapes":{{"0":{sh},"2":{sh}}},"drill":{{"w":{1},"h":{1}}}}}"#,
                    p(*x),
                    p(*d / 2.0),
                    net = net
                )
                .replace(&format!("[{},{}]", p(*x), p(*d / 2.0)), &format!("[{},{}]", p(*x), p(*y)))
            }
            Pad::Smd(net, x, y, w, h) => format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"smd","center":[{},{}],"layers":[0],
                   "shapes":{{"0":{{"kind":"rect","w":{},"h":{}}}}}}}"#,
                p(*x),
                p(*y),
                p(*w),
                p(*h)
            ),
            Pad::Poly(net, ctr, pts) => {
                let pts: Vec<String> = pts.iter().map(|q| format!("[{},{}]", p(q.0), p(q.1))).collect();
                format!(
                    r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"smd","center":[{},{}],"layers":[0],
                       "shapes":{{"0":{{"kind":"poly","pts":[{}]}}}}}}"#,
                    p(ctr.0),
                    p(ctr.1),
                    pts.join(",")
                )
            }
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
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}},{{"code":3,"name":"C"}},{{"code":4,"name":"D"}}],
            "pads":[{}],"kept":[{}],"outline":[{}],"rules":{{}}}}"#,
        pads.join(","),
        kept.join(","),
        outline.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn witness(m: &BoardModel) -> Witness {
    let w = run(m, None, &[], &WitnessOpts::default()).unwrap();
    assert_eq!(w.summary.chains_failed, 0, "every chain maps");
    assert_eq!(w.summary.state_check, "ok");
    w
}

/// P1 below and P2 above a gap that tapers from 1.4 mm (x = 4) to `g` mm (x = 6); two 0.2 mm
/// tracks (clearance 0.2: they need 1.0 mm) run through it from left to right.
fn gap(g: f64) -> BoardModel {
    let y1 = 4.4;
    board(
        &[
            Pad::Smd(3, 5.0, 3.4, 2.0, 2.0),
            Pad::Poly(4, (5.0, 6.6), vec![(4.0, y1 + 1.4), (6.0, y1 + g), (6.0, 7.5), (4.0, 7.5)]),
            Pad::Th(1, 1.0, 4.775, 0.3),
            Pad::Th(1, 9.0, 4.775, 0.3),
            Pad::Th(2, 1.0, 5.225, 0.3),
            Pad::Th(2, 9.0, 5.225, 0.3),
        ],
        &[(1, 0, 0.2, (1.0, 4.775), (9.0, 4.775)), (2, 0, 0.2, (1.0, 5.225), (9.0, 5.225))],
    )
}

#[test]
fn parallel_traces_through_a_gap_that_fits() {
    let w = witness(&gap(1.05));
    assert!(w.violations.is_empty(), "{:?}", w.violations);
    assert!(w.summary.mapped_pct > 99.9);
}

#[test]
fn narrowing_the_gap_gives_exactly_one_violation() {
    let w = witness(&gap(0.9));
    let n = |check: &str| w.violations.iter().filter(|v| v.check == check).count();
    // The narrow cut is one CDT edge and, equally, one obstacle-pair cut.
    assert_eq!((n("edge_fit"), n("pair_fit")), (1, 1), "{:?}", w.violations);
    assert_eq!((n("layer_combo"), n("allowed")), (0, 0));
}

fn under_pad(layer: u32) -> BoardModel {
    board(
        &[Pad::Smd(3, 5.0, 5.0, 2.0, 2.0), Pad::Th(1, 1.0, 5.0, 0.6), Pad::Th(1, 9.0, 5.0, 0.6)],
        &[(1, layer, 0.2, (1.0, 5.0), (9.0, 5.0))],
    )
}

#[test]
fn trace_on_b_under_an_f_pad_is_allowed_and_on_f_is_not() {
    let ok = witness(&under_pad(2));
    assert_eq!(ok.summary.counts.allowed, 0, "{:?}", ok.violations);
    let bad = witness(&under_pad(0));
    assert!(bad.summary.counts.allowed >= 1, "{:?}", bad.violations);
}

fn crossing(l1: u32, l2: u32) -> BoardModel {
    board(
        &[
            Pad::Th(1, 1.0, 5.0, 0.6),
            Pad::Th(1, 9.0, 5.0, 0.6),
            Pad::Th(2, 5.0, 1.0, 0.6),
            Pad::Th(2, 5.0, 9.0, 0.6),
        ],
        &[(1, l1, 0.2, (1.0, 5.0), (9.0, 5.0)), (2, l2, 0.2, (5.0, 1.0), (5.0, 9.0))],
    )
}

#[test]
fn crossing_on_two_layers_is_no_same_layer_crossing() {
    let w = witness(&crossing(0, 2));
    assert_eq!(w.summary.crossings, 1);
    assert_eq!(w.summary.same_layer_crossings, 0, "{:?}", w.violations);
}

#[test]
fn crossing_on_one_layer_is_reported() {
    let w = witness(&crossing(0, 0));
    assert_eq!(w.summary.crossings, 1);
    assert_eq!(w.summary.same_layer_crossings, 1, "{:?}", w.violations);
}

#[test]
fn a_tee_splits_the_net_into_three_chains() {
    let w = witness(&board(
        &[Pad::Th(1, 1.0, 5.0, 0.6), Pad::Th(1, 9.0, 5.0, 0.6), Pad::Th(1, 5.0, 9.0, 0.6)],
        &[
            (1, 0, 0.2, (1.0, 5.0), (5.0, 5.0)),
            (1, 0, 0.2, (5.0, 5.0), (9.0, 5.0)),
            (1, 0, 0.2, (5.0, 5.0), (5.0, 9.0)),
        ],
    ));
    assert_eq!(w.chains.len(), 3);
}
