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
use topor_engine::topo::core::{fuse_line, FEEDBACK_IGNORED};

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
    let l = fuse_line(3, 4.81, 7.58);
    assert!(l.starts_with("fuse after drawing: 3 trace ends moved onto their trunk's copper"), "{l}");
    assert!(l.contains(": 4.8 mm saved in length, junction-angle price ([factors] junction_angle) lower by 7.6"), "{l}");
    assert!(!l.contains("mm cheaper"), "{l}");
    // A fuse that trades length for a better angle says "added"; a worse angle "higher".
    let l = fuse_line(1, -0.25, 1281556.0);
    assert!(l.contains(": 0.2 mm added in length") || l.contains(": 0.3 mm added in length"), "{l}");
    assert!(l.contains("lower by 1281556.0"), "{l}");
    assert!(fuse_line(1, 0.5, -2.0).contains("higher by 2.0"));
    assert!(fuse_line(0, 0.0, 0.0).contains(": 0.0 mm saved in length, junction-angle price ([factors] junction_angle) lower by 0.0"));
}
