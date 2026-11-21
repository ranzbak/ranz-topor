//! M6 (6b "remove firefighting", user approval 2026-11-18): what stays of the removed drawing-side
//! stand-ins, and the two report fixes of the phase 1 findings.
//!
//! * `drawing_feedback` acts with `[optimize] loop = "cycles"` only; with the TopoR loop a true
//!   value is ignored with the warning `FEEDBACK_IGNORED` (the picked plan is drawn once, M2).
//! * The fuse pass reports the length it saved in mm and the junction-angle and style price apart (F-3: the
//!   sum of both was printed as "mm cheaper", 1281560.3 mm on esp-rust-board).
//!
//! Fixture: `harness/mechfix.py fx_narrow_gap_forced` (one connection, a gap too narrow for it),
//! the smallest board that reaches the drawing.

use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route;
use topor_engine::topo::core::{fuse_line, polish_line, split_line, CYCLES_MEMORY, FEEDBACK_IGNORED};

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
    assert!(l.contains(": 4.8 mm saved in length, junction-angle and style price ([factors] junction_angle, any_angle; [drawing] bend_mm) lower by 7.6"), "{l}");
    assert!(!l.contains("mm cheaper"), "{l}");
    // A fuse that trades length for a better angle says "added"; a worse angle "higher".
    let l = fuse_line(&[(1281555.7, -0.3)]);
    assert!(l.contains(": 0.3 mm added in length, junction-angle and style price ([factors] junction_angle, any_angle; [drawing] bend_mm) lower by 1281556.0"), "{l}");
    assert!(fuse_line(&[(0.5 - 2.0, 0.5)]).contains("0.5 mm saved in length, junction-angle and style price ([factors] junction_angle, any_angle; [drawing] bend_mm) higher by 2.0"));
    assert!(fuse_line(&[]).starts_with("fuse after drawing: 0 trace ends moved onto their trunk's copper where the two part: 0.0 mm saved in length, junction-angle and style price ([factors] junction_angle, any_angle; [drawing] bend_mm) lower by 0.0"));
    // The polish line says the length its elbows and Zs changed, "added" (never shorter) or
    // "saved" by its sign.
    assert!(polish_line(34, 3.013).ends_with(": 34 segments not at 0/45/90 degrees (fused branch ends) redrawn as an elbow or Z: 3.0 mm added in length"));
    assert!(polish_line(0, 0.0).ends_with(": 0.0 mm added in length"));
    assert!(polish_line(1, -0.26).ends_with(": 0.3 mm saved in length"));
}

/// fuse-octi r4 (H1 part 2): the split guard's line, every form: nothing to say, steps refused by
/// the fuse and the polish, and the self-check's nets (one, several).
#[test]
fn split_line_names_the_refused_steps_and_the_split_nets() {
    assert_eq!(split_line(0, 0, &[]), None);
    let l = split_line(2, 0, &[]).unwrap();
    assert_eq!(l, "split guard after drawing: 2 fuse and 0 polish steps not taken (each would have left a net's copper in more pieces: a dangling branch)");
    assert!(split_line(0, 4, &[]).unwrap().starts_with("split guard after drawing: 0 fuse and 4 polish steps not taken ("));
    let l = split_line(0, 0, &["JS2_9".into()]).unwrap();
    assert!(l.starts_with("split guard after drawing: 0 fuse and 0 polish steps") && l.ends_with("; engine bug: 1 net in more pieces after the steps after the drawing than before them: JS2_9"), "{l}");
    let l = split_line(1, 1, &["A".into(), "B".into()]).unwrap();
    assert!(l.ends_with("; engine bug: nets in more pieces after the steps after the drawing than before them: A, B"), "{l}");
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
    // The esp USB crop fuses 14 trace ends: the mm the line reports saved are the copper the fuse
    // took away, and the octilinear polish after it gives part of it back (a fused end at a free
    // angle becomes a longer elbow or Z: 7.1 mm saved, 1.0 mm added; with 48 fuses 8.5 and 3.0,
    // hotfix-main 2026-11-18). So the fuse's saving minus the polish's addition matches the routed
    // length with `fuse_branches` off minus on (rounding of the 0.1 mm lines only), and the
    // junction-angle price part is the rest of the price gain, not 0.
    let m = BoardModel::from_json(include_str!("fixtures/esp-rust-board-usb-crop.json")).expect("fixture parses");
    let wall = if cfg!(debug_assertions) { 900.0 } else { 120.0 };
    let on = route(&m, &opts(&format!(r#"{{"runs": 1, "wall_clock_s": {wall}}}"#)));
    let off = route(&m, &opts(&format!(r#"{{"runs": 1, "wall_clock_s": {wall}, "fuse_branches": false}}"#)));
    let line = |r: &topor_engine::route::RouteResult, start: &str| -> String { r.warnings.iter().find(|w| w.starts_with(start)).unwrap_or_else(|| panic!("{start}: {:?}", r.warnings)).clone() };
    let num = |l: &str, after: &str| -> f64 {
        let t = l.split(after).nth(1).unwrap_or_else(|| panic!("{after}: {l}"));
        t.trim_start().split(|c: char| c != '.' && !c.is_ascii_digit()).next().and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("{after}: {l}"))
    };
    let l = line(&on, "fuse after drawing: ");
    let n = num(&l, "fuse after drawing: ");
    let saved = num(&l, "where the two part: ");
    let angle = num(&l, ") lower by ");
    assert!(n >= 1.0 && l.contains(" mm saved in length"), "{l}");
    // the polish only lengthens: "added" on both routes
    let polished = |r| {
        let p = line(r, "octilinear polish after drawing: ");
        assert!(p.ends_with(" mm added in length"), "{p}");
        num(&p, "elbow or Z: ")
    };
    let (pol_on, pol_off) = (polished(&on), polished(&off));
    let measured = (off.total_length_nm - on.total_length_nm) as f64 / 1e6;
    assert!(measured > 0.0, "fuse on {} nm, off {} nm", on.total_length_nm, off.total_length_nm);
    let net = saved - pol_on + pol_off;
    assert!((net - measured).abs() <= 0.2 + 0.02 * measured, "reported {saved} mm saved by the fuse, {pol_on} mm added by the polish (off: {pol_off}), measured {measured:.2} mm: {l}");
    assert!(angle > 0.0, "{l}");
}
