//! M6 (6b "remove firefighting", user approval 2026-11-18): what stays of the removed drawing-side
//! stand-ins, and the two report fixes of the phase 1 findings.
//!
//! * `drawing_feedback` acts with `[optimize] loop = "cycles"` only; with the TopoR loop a true
//!   value is ignored with the warning `FEEDBACK_IGNORED` (the picked plan is drawn once, M2).
//! * The fuse pass reports the length it saved in mm and the junction-angle price apart (F-3: the
//!   sum of both was printed as "mm cheaper", 1281560.3 mm on esp-rust-board).
//!
//! Fixture: `harness/mechfix.py fx_narrow_gap_forced` (one connection, a gap too narrow for it),
//! the smallest board that reaches the drawing.

use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route;
use topor_engine::topo::core::{fuse_line, CYCLES_MEMORY, FEEDBACK_IGNORED};

fn gap() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/fx-narrow-gap.json")).expect("fixture parses")
}

fn opts(json: &str) -> Options {
    Options::from_json(json).expect("options parse")
}

fn ignored(json: &str) -> bool {
    route(&gap(), &opts(json)).warnings.iter().any(|w| w == FEEDBACK_IGNORED)
}

#[test]
fn drawing_feedback_is_ignored_with_a_warning_on_the_topor_loop_only() {
    // TopoR loop (the default): true is ignored and says so; false says nothing.
    assert!(ignored(r#"{"runs": 1, "wall_clock_s": 60.0, "opt_loop": "topor", "drawing_feedback": true}"#));
    assert!(!ignored(r#"{"runs": 1, "wall_clock_s": 60.0, "opt_loop": "topor", "drawing_feedback": false}"#));
    // Whole-route cycles: the option acts, no warning either way.
    assert!(!ignored(r#"{"runs": 1, "wall_clock_s": 60.0, "opt_loop": "cycles", "topo_rounds": 1, "drawing_feedback": true}"#));
    assert!(!ignored(r#"{"runs": 1, "wall_clock_s": 60.0, "opt_loop": "cycles", "topo_rounds": 1, "drawing_feedback": false}"#));
}

#[test]
fn drawing_feedback_redraws_only_with_the_cycles_loop() {
    // The narrow gap's connection cannot be drawn legally: with feedback it is a rejected wire
    // the feedback looks at ("feedback tried N" in the layers line). Only the cycles loop tries.
    let tried = |json: &str| -> String {
        let r = route(&gap(), &opts(json));
        let l = r.warnings.iter().find(|w| w.contains("feedback tried ")).unwrap_or_else(|| panic!("layers line: {:?}", r.warnings));
        l.split("feedback tried ").nth(1).and_then(|t| t.split(',').next()).expect("count").to_string()
    };
    assert_eq!(tried(r#"{"runs": 1, "wall_clock_s": 60.0, "opt_loop": "topor", "drawing_feedback": true}"#), "0");
    assert_ne!(tried(r#"{"runs": 1, "wall_clock_s": 60.0, "opt_loop": "cycles", "topo_rounds": 1, "drawing_feedback": true}"#), "0");
}

#[test]
fn fuse_line_keeps_length_and_angle_price_apart() {
    // Two fuses: (price gain, length gain) = (7.0, 3.0) and (5.39, 1.81): 4.81 mm saved, the
    // junction-angle price 4.0 + 3.58 = 7.58 lower.
    let l = fuse_line(&[(7.0, 3.0), (5.39, 1.81)]);
    assert!(l.starts_with("fuse after drawing: 2 trace ends moved onto their trunk's copper"), "{l}");
    assert!(l.contains(": 4.8 mm saved in length, junction-angle price ([factors] junction_angle) lower by 7.6"), "{l}");
    assert!(!l.contains("mm cheaper"), "{l}");
    // A fuse that trades length for a better angle says "added"; a worse angle "higher".
    let l = fuse_line(&[(1281555.7, -0.3)]);
    assert!(l.contains(": 0.3 mm added in length, junction-angle price ([factors] junction_angle) lower by 1281556.0"), "{l}");
    assert!(fuse_line(&[(0.5 - 2.0, 0.5)]).contains("0.5 mm saved in length, junction-angle price ([factors] junction_angle) higher by 2.0"));
    assert!(fuse_line(&[]).starts_with("fuse after drawing: 0 trace ends moved onto their trunk's copper where the two part: 0.0 mm saved in length, junction-angle price ([factors] junction_angle) lower by 0.0"));
}

#[test]
fn the_cycles_memory_is_kept_with_the_cycles_loop_only() {
    // The narrow gap's connection is drawn with violations (never drop): with whole-route cycles
    // its violation becomes its own history for the next cycle (no over-full edge here, so 0 room
    // hints and 0 edges with history); the TopoR loop routes once per run and keeps nothing (M6 R2).
    let mem = |json: &str| -> Option<String> { route(&gap(), &opts(json)).warnings.into_iter().find(|w| w.starts_with(CYCLES_MEMORY)) };
    let l = mem(r#"{"runs": 1, "wall_clock_s": 60.0, "opt_loop": "cycles", "topo_rounds": 1}"#).expect("cycles memory line");
    let n = |key: &str| -> usize { l.split(key).next().and_then(|t| t.rsplit(' ').find(|x| !x.is_empty())).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("{key}: {l}")) };
    assert!(n(" room hints") == 0 && n(" connections with their own history") >= 1, "{l}");
    assert!(mem(r#"{"runs": 1, "wall_clock_s": 60.0, "opt_loop": "topor"}"#).is_none());
}

#[test]
fn the_fuse_line_of_a_routed_board_reports_the_length_the_fuse_saved() {
    // V-2 (verify 2026-11-18): the fuse line as the route writes it, not only `fuse_line` alone.
    // The esp USB crop fuses 15 trace ends: the mm the line reports saved are the copper the fuse
    // took away, so they match the routed length with `fuse_branches` off minus on (the steps
    // after the fuse move a little copper too: 7.7 mm reported, 6.6 mm measured, 2026-11-18), and
    // the junction-angle price part is the rest of the price gain, not 0.
    let m = BoardModel::from_json(include_str!("fixtures/esp-rust-board-usb-crop.json")).expect("fixture parses");
    let wall = if cfg!(debug_assertions) { 900.0 } else { 120.0 };
    let on = route(&m, &opts(&format!(r#"{{"runs": 1, "wall_clock_s": {wall}}}"#)));
    let off = route(&m, &opts(&format!(r#"{{"runs": 1, "wall_clock_s": {wall}, "fuse_branches": false}}"#)));
    let l = on.warnings.iter().find(|w| w.starts_with("fuse after drawing: ")).expect("fuse line").clone();
    let num = |after: &str| -> f64 {
        let t = l.split(after).nth(1).unwrap_or_else(|| panic!("{after}: {l}"));
        t.trim_start().split(|c: char| c != '.' && !c.is_ascii_digit()).next().and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("{after}: {l}"))
    };
    let n = num("fuse after drawing: ");
    let saved = num("where the two part: ");
    let angle = num(") lower by ");
    assert!(n >= 1.0 && l.contains(" mm saved in length"), "{l}");
    let measured = (off.total_length_nm - on.total_length_nm) as f64 / 1e6;
    assert!(measured > 0.0, "fuse on {} nm, off {} nm", on.total_length_nm, off.total_length_nm);
    assert!((saved - measured).abs() <= 0.25 * measured + 0.5, "reported {saved} mm saved, measured {measured:.2} mm: {l}");
    assert!(angle > 0.0, "{l}");
}
