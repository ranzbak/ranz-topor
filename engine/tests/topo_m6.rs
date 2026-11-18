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
    // its violation becomes a room hint and its own history for the next cycle; the TopoR loop
    // routes once per run and keeps nothing (M6 R2).
    let mem = |json: &str| -> Option<String> { route(&gap(), &opts(json)).warnings.into_iter().find(|w| w.starts_with(CYCLES_MEMORY)) };
    let l = mem(r#"{"runs": 1, "wall_clock_s": 60.0, "opt_loop": "cycles", "topo_rounds": 1}"#).expect("cycles memory line");
    let n = |key: &str| -> usize { l.split(key).next().and_then(|t| t.rsplit(' ').find(|x| !x.is_empty())).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("{key}: {l}")) };
    assert!(n(" room hints") >= 1 && n(" connections with their own history") >= 1, "{l}");
    assert!(mem(r#"{"runs": 1, "wall_clock_s": 60.0, "opt_loop": "topor"}"#).is_none());
}
