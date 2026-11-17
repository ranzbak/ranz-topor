//! Approved survivor patterns (`harness/fx_survivors.py`, user 2026-11-17: collinear_row, diag_exit, via_fold, qfn_two_ends)
//! routed with `[drawing] realise = "erbs"`. Each pattern's optimum is laid out in the harness and checked by KiCad DRC; here:
//! copper (length at the reference's, no violation beyond the unavoidable ones) and rules read from the ERBS dump
//! (`TOPO_ERBS_DUMP`: the realised ERBS per layer), not exact numbers. Mutants these kill: tie_label (collinear_row),
//! own_tip_keep (diag_exit), vertex_leave_off (via_fold), h_exit_no_b (qfn_two_ends).
//! Own test binary: the dump directory is a process-wide environment variable, so the tests take turns.

use std::sync::Mutex;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};

static TURN: Mutex<()> = Mutex::new(());

struct Run {
    /// Sum of the connection lengths (mm) and of their violations.
    len: f64,
    viol: usize,
    /// Per layer dump (`realise-L<n>.json`).
    dumps: Vec<serde_json::Value>,
}

fn route(p: &str) -> Run {
    let _g = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let path = format!("{}/tests/fixtures/fx/model-{p}.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["realise"] = serde_json::json!("erbs");
    let o: Options = serde_json::from_value(val).unwrap();
    let dir = std::env::temp_dir().join(format!("topor-fx-{}-{p}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("TOPO_ERBS_DUMP", &dir);
    let r = topor_engine::route::route_with(&m, &o, &RunControl::new());
    std::env::remove_var("TOPO_ERBS_DUMP");
    assert_eq!(r.routed, r.connections_total, "{p}: every connection routes");
    let mut dumps = Vec::new();
    for l in 0..8 {
        if let Ok(s) = std::fs::read_to_string(dir.join(format!("realise-L{l}.json"))) {
            dumps.push(serde_json::from_str(&s).unwrap());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    Run { len: r.connections.iter().map(|c| c.length_nm as f64 / 1e6).sum(), viol: r.connections.iter().map(|c| c.violations as usize).sum(), dumps }
}

fn n_failed(r: &Run) -> usize {
    r.dumps.iter().map(|d| d["failed"].as_array().unwrap().len()).sum()
}
fn n_erbs(r: &Run) -> usize {
    r.dumps.iter().map(|d| d["erbs"].as_array().unwrap().len()).sum()
}
fn violations(r: &Run) -> Vec<(bool, f64)> {
    r.dumps.iter().flat_map(|d| d["violations"].as_array().unwrap().iter().map(|v| (v["predicted"].as_bool().unwrap(), v["mm"].as_f64().unwrap()))).collect()
}

/// Three pads whose top edges lie on the band's line: every collinear vertex and cut is walked, the ERBS exists (a failed walk
/// "no exit edge: the path runs along a vertex of both sides" is the mutant tie_label), and the drawn wire is at the optimum
/// (reference 15.171 mm, topor's drawn run 15.269 mm).
#[test]
fn collinear_row_is_walked_and_drawn_over_the_row() {
    let r = route("collinear_row");
    assert_eq!(n_failed(&r), 0, "the ERBS walk fails on collinear vertices");
    assert_eq!(n_erbs(&r), 1, "one ERBS");
    assert!(r.len <= 15.1715 * 1.01, "{} mm", r.len);
    assert_eq!(r.viol, 0);
}

/// The wire leaves its own pad exactly through a corner between two neighbours: the ERBS is the straight 45 degree line, two
/// path points, no kink at the pad's corner (mutant own_tip_keep: a spoke tip on the own pad bends it, 4 points).
#[test]
fn diag_exit_is_one_straight_line() {
    let r = route("diag_exit");
    assert_eq!(n_failed(&r), 0);
    let es: Vec<&serde_json::Value> = r.dumps.iter().flat_map(|d| d["erbs"].as_array().unwrap().iter()).collect();
    assert_eq!(es.len(), 1);
    let pts: Vec<(f64, f64)> = es[0]["pts"].as_array().unwrap().iter().map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap())).collect();
    assert_eq!(pts.len(), 2, "the straight band needs no extra path point: {pts:?}");
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    for p in &pts {
        let off = ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs() / dx.hypot(dy);
        assert!(off < 1e-3, "path point {p:?} is {off} mm off the straight line: {pts:?}");
    }
    assert!(r.len <= 8.4853 * 1.002, "{} mm", r.len);
    assert_eq!(r.viol, 0);
}

/// A net folded back across a no-via area has stretches on both layers; every stretch gets its ERBS (mutant vertex_leave_off:
/// the walks along a vertex fail on both layers) and the wires stay at the optimum (reference 18.451 mm).
#[test]
fn via_fold_has_an_erbs_on_both_layers() {
    let r = route("via_fold");
    assert_eq!(r.dumps.len(), 2, "two layers with ERBS");
    assert_eq!(n_failed(&r), 0, "an ERBS walk failed");
    assert_eq!(n_erbs(&r), 3, "Q: one stretch per layer, R: one");
    assert!(r.dumps.iter().all(|d| !d["erbs"].as_array().unwrap().is_empty()), "an ERBS per layer");
    assert!(r.len <= 18.451 * 1.01, "{} mm", r.len);
    assert_eq!(r.viol, 0);
}

/// Pad exits of a 0.4 mm row with a 0.25 mm wire violate by 0.025 mm at both ends, unavoidably (no neck below the board
/// minimum): the ERBS predicts exactly those four (wire against the two neighbours of each pad) and none is an erbs_gap
/// (mutant h_exit_no_b: the far end's exit run is not predicted, two become erbs_gap).
#[test]
fn qfn_two_ends_predicts_its_four_exit_violations() {
    let r = route("qfn_two_ends");
    assert_eq!(n_failed(&r), 0);
    let v = violations(&r);
    assert_eq!(v.iter().filter(|x| x.0).count(), 4, "{v:?}");
    assert_eq!(v.iter().filter(|x| !x.0).count(), 0, "an unpredicted violation: {v:?}");
    assert!(r.len <= 8.2426 * 1.01, "{} mm", r.len);
}
