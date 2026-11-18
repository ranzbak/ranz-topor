//! CM1 (cm1-room, docs/dev/design/cm1-room.md): a planned via is a feature of the capacity
//! test. The wiccon GUI case in miniature (reports/wiccon-gui27-20261117-2215.md, cluster A):
//! two 2.5 x 1.0 mm SMD pads on F.Cu 1.0 mm apart (U22 TXD above, U21 another net below), TXD's
//! other end a small B.Cu pad in the middle of that gap (so TXD's via has to sit in the gap), and
//! RST from left to right at the gap's height on F.Cu. The gap holds the via (0.6) or RST
//! (0.25 + 2 x 0.2), never both (1.25 > 1.0): the plan must route RST round a pad, and must
//! predict the conflict when it does not.

use topor_engine::check::Checker;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route_with;

const MM: f64 = 1e6;

fn gap() -> BoardModel {
    let p = |v: f64| (v * MM).round() as i64;
    let c = [(0.0, 0.0), (12.0, 0.0), (12.0, 10.0), (0.0, 10.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    // (ref, net, x, y, w, h, layer): net 1 TXD, 2 RST, 3 RXD (U21, one pad: no connection).
    let pads = [
        ("U22", 1, 6.0, 4.0, 2.5, 1.0, 0),
        ("T1", 1, 6.0, 5.0, 0.3, 0.3, 2),
        ("U21", 3, 6.0, 6.0, 2.5, 1.0, 0),
        ("R1", 2, 1.0, 5.0, 1.0, 1.0, 0),
        ("R2", 2, 11.0, 5.0, 1.0, 1.0, 0),
    ];
    let pj: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(r, net, x, y, w, h, l))| {
            format!(
                r#"{{"uuid":"p{i}","ref":"{r}","number":"1","net":{net},"attr":"smd","center":[{},{}],
                    "rotation_deg":0.0,"layers":[{l}],"shapes":{{"{l}":{{"kind":"rect","w":{},"h":{}}}}},"drill":null}}"#,
                p(x),
                p(y),
                p(w),
                p(h)
            )
        })
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"cm1gap",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"TXD"}},{{"code":2,"name":"RST"}},{{"code":3,"name":"RXD"}}],
            "pads":[{}],"keepouts":[],"outline":[{}],
            "rules":{{"min_clearance":200000,"min_track_width":250000,"min_via_diameter":600000,"min_via_drill":300000,
                     "min_annular":50000,"hole_clearance":250000,"hole_to_hole":250000,"edge_clearance":500000,
                     "default":{{"clearance":200000,"width":250000,"via_d":600000,"via_drill":300000}}}}}}"#,
        pj.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn opts(via_cuts: bool) -> Options {
    Options { via_cuts, ..Options::default() }
}

/// The picked plan predicts what the drawing hits: with via cuts the drawing of the pick is
/// clean (RST goes round a pad or the via leaves the gap), and the plan's prediction equals 0.
#[test]
fn a_via_in_a_pad_gap_takes_its_room_from_the_wire_through_the_gap() {
    let m = gap();
    let r = route_with(&m, &opts(true), &RunControl::new());
    assert_eq!(r.failed, 0, "{:?}", r.warnings);
    let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
    let lp = r.warnings.iter().find(|w| w.starts_with("TopoR loop (")).expect("loop line");
    assert!(rep.violations.is_empty(), "{:?}\n{lp}", rep.violations);
    assert!(lp.contains("predicted 0 violations"), "{lp}");
}

/// The same board before CM1 (`[capacity] via_cuts = false`): the plan predicts 0 and the drawing
/// hits the gap, or the board does not show the case (then the test above proves nothing).
#[test]
fn without_via_cuts_the_plan_misses_the_gap() {
    let m = gap();
    let r = route_with(&m, &opts(false), &RunControl::new());
    let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
    let lp = r.warnings.iter().find(|w| w.starts_with("TopoR loop (")).expect("loop line");
    assert!(lp.contains("predicted 0 violations"), "{lp}");
    assert!(!rep.violations.is_empty(), "the old model must draw into the gap: {lp}");
}

/// tw4 T2: with via cuts every refinement ends with stored via prices equal to a fresh pricing
/// (`CapModel::via_check`). The qmtech USB corner (4 layers, planned vias, many rejected moves
/// in the loop-local judge's serial refinement) is the case: a rejected move must restore its
/// wires' via points with their prices (tw4t2-ae57e35: 383+ stale entries per run before).
#[test]
fn via_prices_stay_fresh_through_rejected_moves() {
    use std::sync::atomic::Ordering;
    use topor_engine::topo::refine::viacheck;
    let path = format!("{}/tests/fixtures/qmtech-usb-crop.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(path).expect("fixture")).expect("fixture parses");
    let o = Options::from_json(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 2, "via_cuts": true}"#).expect("options parse");
    viacheck::ON.store(true, Ordering::Relaxed);
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let _ = route_with(&m, &o, &ctl);
    let (bad, n) = (viacheck::BAD.load(Ordering::Relaxed), viacheck::CHECKED.load(Ordering::Relaxed));
    assert!(n > 0, "no priced via entry was checked: the case does not show");
    assert_eq!(bad, 0, "{bad} stale via prices of {n}");
}
