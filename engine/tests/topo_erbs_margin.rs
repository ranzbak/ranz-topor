//! erbs-margin (2026-11-17): the ERBS is placed at every clearance plus the drawing's numeric
//! margin (`margin_nm + topo::pack::DRAW_TOL_NM`, the sequential packer's too) and the checker
//! passes a gap short by at most `check::CHECK_TOL_NM`. Before, the innermost ERBS of every
//! showcase pattern sat at exactly the clearance, the checker rejected it by < 0.5 nm
//! (diag-bundle 2026-11-17: "gap 200000 req 200000"), and every outer ERBS then collided with the
//! inner wire's sequential copper (erbs_fallback on every run; river_wrap +29 DRC).
//!
//! Fixtures: `tests/fixtures/erbs/model-<name>.json`, the approved ERBS showcase patterns
//! (harness/erbs_showcase.py, branch erbs-showcase), exported with `harness.radii_patterns.model_fixture`.

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};

/// The route of fixture `dir/model-<p>.json` with `[drawing] realise` and `style`, one run, one
/// round (deterministic): (connections (name, length mm, violations), warnings, routed, total).
fn route(dir: &str, p: &str, style: &str, realise: &str) -> (Vec<(String, f64, usize)>, Vec<String>, usize, usize) {
    let path = format!("{}/tests/fixtures/{dir}/model-{p}.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["topo_rounds"] = serde_json::json!(1);
    val["realise"] = serde_json::json!(realise);
    val["draw_style"] = serde_json::json!(style);
    let o: Options = serde_json::from_value(val).unwrap();
    let r = topor_engine::route::route_with(&m, &o, &RunControl::new());
    let c = r.connections.iter().map(|c| (c.name.clone(), c.length_nm as f64 / 1e6, c.violations as usize)).collect();
    (c, r.warnings.clone(), r.routed, r.connections_total)
}

/// The number before `key` in the ERBS drawing line ("drawn: N runs from their ERBS ..., N by the
/// sequential attempts after the check rejected the ERBS, ...").
fn count(warn: &[String], key: &str) -> usize {
    let line = warn.iter().find(|w| w.contains(key)).unwrap_or_else(|| panic!("no ERBS line with {key:?} in {warn:?}"));
    let head = &line[..line.find(key).unwrap()];
    head.rsplit(|c: char| !c.is_ascii_digit()).find(|t| !t.is_empty()).and_then(|t| t.parse().ok()).unwrap_or_else(|| panic!("no count before {key:?}: {line}"))
}

const REJECTED: &str = " by the sequential attempts after the check rejected the ERBS";
const FROM_ERBS: &str = " runs from their ERBS";

/// Probe (slow set, VM): every approved pattern fixture (m3 judge, fx, ERBS showcase) in both
/// styles with `realise = "erbs"`: runs from their ERBS, check fallbacks, violations, length.
#[test]
#[ignore]
fn erbs_fallback_probe() {
    let sets: [(&str, &[&str]); 3] = [
        ("m3", &["channel_fit", "three_diagonals", "nested_wraps", "distant_radius", "shortcut_across"]),
        ("fx", &["collinear_row", "diag_exit", "qfn_two_ends", "via_fold"]),
        ("erbs", &["river_wrap", "nested_u", "corner_turn", "gap_squeeze", "qfn_fan", "swap_vias", "serpentine_maze", "serpentine_bundle"]),
    ];
    for (dir, ps) in sets {
        for p in ps {
            for style in ["octilinear", "smooth"] {
                let t = std::time::Instant::now();
                let (c, w, routed, total) = route(dir, p, style, "erbs");
                let v: usize = c.iter().map(|x| x.2).sum();
                let len: f64 = c.iter().map(|x| x.1).sum();
                println!(
                    "PROBE {dir}/{p} {style}: erbs_ok {} fallback {} violations {v} routed {routed}/{total} length {len:.3} mm ({:.1} s)",
                    count(&w, FROM_ERBS),
                    count(&w, REJECTED),
                    t.elapsed().as_secs_f64()
                );
            }
        }
    }
}
