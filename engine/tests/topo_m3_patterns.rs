//! M3: the four approved judge patterns (`harness/radii_patterns.py`; design note
//! `docs/progress/notes/m3-radii-spokes-design.md` §10) drawn with `[drawing] realise = "erbs"`:
//! every connection at its ERBS length (the optimum the judge expects, `tools/judge.py`), no
//! violation. A drawing that ignores the ERBS (or takes another layer's) is longer
//! (channel_fit S1/S3 10.37 mm, nested_wraps A/B 18.14/17.32 mm sequentially).

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};

/// (name, from, to, length mm) of every connection of pattern `p` routed with `realise`.
fn route(p: &str, realise: &str) -> Vec<(String, f64, usize)> {
    let path = format!("{}/tests/fixtures/m3/model-{p}.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut o = Options::default();
    let mut val = serde_json::to_value(&o).unwrap();
    val["runs"] = serde_json::json!(1);
    val["realise"] = serde_json::json!(realise);
    o = serde_json::from_value(val).unwrap();
    let r = topor_engine::route::route_with(&m, &o, &RunControl::new());
    assert_eq!(r.routed, r.connections_total, "{p}: every connection routes");
    r.connections.iter().map(|c| (c.name.clone(), c.length_nm as f64 / 1e6, c.violations as usize)).collect()
}

/// The ERBS lengths (mm) of the judge run m3f4 (all four patterns at the optimum, 1.000).
const EXPECT: [(&str, &[(&str, f64)]); 4] = [
    ("channel_fit", &[("S1", 8.6586), ("S2", 8.0), ("S3", 8.6586)]),
    ("three_diagonals", &[("P1", 12.0711), ("S", 12.0296), ("P2", 11.9882)]),
    ("nested_wraps", &[("A", 16.8552), ("B", 15.5297), ("C", 14.2663)]),
    ("distant_radius", &[("W1", 14.2343), ("W2", 14.1929), ("W3", 14.1515)]),
];

#[test]
fn judge_patterns_drawn_from_their_erbs() {
    for (p, want) in EXPECT {
        let got = route(p, "erbs");
        for &(n, l) in want {
            let (_, g, v) = got.iter().find(|c| c.0 == n).unwrap_or_else(|| panic!("{p}: no {n} in {got:?}"));
            assert!(*g <= l * 1.002, "{p} {n}: {g} mm, the ERBS gives {l} mm ({got:?})");
            assert_eq!(*v, 0, "{p} {n}: violations ({got:?})");
        }
    }
}
