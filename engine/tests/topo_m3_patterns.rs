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

/// (name, length mm) of every connection of pattern `p` with `realise = "erbs"`, every ERBS wound
/// three times by the test hook ("wind erbs"), and the engine's warnings.
fn route_wound(p: &str, extra: &[(&str, serde_json::Value)]) -> (Vec<(String, f64)>, Vec<String>) {
    route_hooked(p, extra, &["wind erbs"])
}

/// As `route_wound` with the test hook answering true at the named points `hooks`.
fn route_hooked(p: &str, extra: &[(&str, serde_json::Value)], hooks: &'static [&'static str]) -> (Vec<(String, f64)>, Vec<String>) {
    let path = format!("{}/tests/fixtures/m3/model-{p}.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["realise"] = serde_json::json!("erbs");
    for (k, v) in extra {
        val[*k] = v.clone();
    }
    let o: Options = serde_json::from_value(val).unwrap();
    let mut ctl = RunControl::new();
    ctl.set_test_hook(move |p| hooks.contains(&p));
    let r = topor_engine::route::route_with(&m, &o, &ctl);
    assert_eq!(r.routed, r.connections_total, "{p}: every connection routes");
    (r.connections.iter().map(|c| (c.name.clone(), c.length_nm as f64 / 1e6)).collect(), r.warnings.clone())
}

/// The length factor (`draw::erbs_length_price`): a legal ERBS three times its band is priced
/// past a sequential run, which replaces it (the warning erbs_long says so); with a margin that
/// keeps it (erbs_margin 10: the sequential run priced at 11 x its length) the wound ERBS is drawn.
#[test]
fn wound_erbs_gives_way_to_the_sequential_run() {
    let (got, warn) = route_wound("channel_fit", &[]);
    let seq = route("channel_fit", "sequential");
    for (n, l) in &got {
        let s = seq.iter().find(|c| &c.0 == n).unwrap().1;
        assert!(*l <= s * 1.01 + 0.01, "{n}: {l} mm drawn, the sequential run is {s} mm ({got:?})");
    }
    let el = warn.iter().find(|w| w.starts_with("erbs_long: ")).unwrap_or_else(|| panic!("no erbs_long line in {warn:?}"));
    assert!(el.contains("replaced by a cheaper sequential run") && el.contains("drawn sequential"), "{el}");
    assert!(!el.starts_with("erbs_long: 0 "), "{el}");
    // a run the length factor gave to a sequential run is no check fallback (the ERBS was legal)
    let line = warn.iter().find(|w| w.contains("ERBS priced past their band")).unwrap_or_else(|| panic!("no ERBS line in {warn:?}"));
    assert!(line.contains(" 0 by the sequential attempts after the check rejected the ERBS"), "{line}");
    // weight 0, margin 10: the wound ERBS (3 x its band) is not priced past B (1 + g), no
    // sequential run is drawn against it (its retraced copper is one track, so the length does
    // not show it)
    let (_, warn) = route_wound("channel_fit", &[("erbs_length_weight", serde_json::json!(0.0)), ("erbs_margin", serde_json::json!(10.0))]);
    assert!(!warn.iter().any(|w| w.starts_with("erbs_long: ")), "margin 10: no ERBS priced past its band ({warn:?})");
    let line = warn.iter().find(|w| w.contains("ERBS priced past their band")).unwrap_or_else(|| panic!("no ERBS line in {warn:?}"));
    assert!(line.contains(" 0 ERBS priced past their band"), "{line}");
    // weight 100, margin 1: priced 3 x (1 + 100 x 4) past 2 x the band, the sequential run (priced
    // at 2 x its length) is cheaper: replaced
    let (_, warn) = route_wound("channel_fit", &[("erbs_margin", serde_json::json!(1.0)), ("erbs_length_weight", serde_json::json!(100.0))]);
    let el = warn.iter().find(|w| w.starts_with("erbs_long: ")).unwrap_or_else(|| panic!("no erbs_long line in {warn:?}"));
    assert!(el.contains(", 0 kept"), "{el}");
}

/// The last step of the length factor: a wound (long) ERBS whose sequential attempts are all
/// illegal (test point "no sequential run") is drawn after all, and ERBS_LONG says so
/// ("ERBS kept (no legal sequential run)"). Nothing is dropped.
#[test]
fn kept_erbs_is_drawn_when_no_sequential_run_is_legal() {
    let (got, warn) = route_hooked("channel_fit", &[], &["wind erbs", "no sequential run"]);
    assert_eq!(got.len(), 3, "{got:?}");
    let el = warn.iter().find(|w| w.starts_with("erbs_long: ")).unwrap_or_else(|| panic!("no erbs_long line in {warn:?}"));
    assert!(el.contains("ERBS kept (no legal sequential run)"), "{el}");
    assert!(el.contains(" 0 replaced by a cheaper sequential run"), "{el}");
    assert!(!el.contains("drawn sequential"), "{el}");
    let line = warn.iter().find(|w| w.contains("ERBS priced past their band")).unwrap_or_else(|| panic!("no ERBS line in {warn:?}"));
    assert!(line.contains(" 0 by the sequential attempts after the check rejected the ERBS"), "{line}");
}

/// A stretch whose sleeve fails (test point "fail erbs sleeve") is drawn the sequential way and
/// its reason reaches the erbs_fallback warning (plugin finding ERBS_FALLBACK).
#[test]
fn failed_sleeve_reason_reaches_the_erbs_fallback_warning() {
    let (got, warn) = route_hooked("channel_fit", &[], &["fail erbs sleeve"]);
    assert_eq!(got.len(), 3, "every connection still routes: {got:?}");
    let w = warn.iter().find(|w| w.starts_with("erbs_fallback: ")).unwrap_or_else(|| panic!("no erbs_fallback line in {warn:?}"));
    assert!(w.contains("1 stretch(es) without a sleeve to the end") && w.contains("injected sleeve failure"), "{w}");
}
