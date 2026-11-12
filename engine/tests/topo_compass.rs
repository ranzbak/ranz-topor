//! M1 compass (`RouteResult::compass`, `topo/compass.rs`): the plan's predicted items per class on
//! three of the user-approved fit patterns (`harness/fit_patterns.py`, exported to
//! `tests/fixtures/fit/`) whose answers are known by hand:
//!
//! - `channel_fit`: three wires, slack +0.05 mm: the cut model predicts nothing. The per-edge
//!   model predicts two over-full edge-layers here (its oblique factor on the edges from the row
//!   corners, fit_patterns README "Capacity definitions"), which the drawing draws clean: the
//!   compass shows that over-prediction ("predicted, drawn clean").
//! - `channel_overfull`: four wires through the same channel, over-full by 0.40 mm: the cut model
//!   predicts the over-full cut C1 between the rows' top corners and its twin between the bottom
//!   corners (same 1.60 mm, pattern note "the bottom corners give the same cut"): 2 cuts, each
//!   with the four connections; the per-edge model sees it too.
//! - `model_gap_cut`: the opening's non-adjacent cut AC is over-full by 0.10 mm while every
//!   triangulation edge fits (Maley 1987 p. 32): the cut model predicts AC and its bottom twin (2
//!   cuts, 8 connections each), the per-edge model none (its `model_ref` items are empty; with
//!   `model = "edges"` the plan predicts 0 over-full and says model gap is not measured).
//!
//! And the compass never changes the routing: the copper is the same with and without it (it is
//! always built, so this is checked by determinism of two runs and by the C0 plan line).

use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route, RouteResult};

fn model(name: &str) -> BoardModel {
    let path = format!("{}/tests/fixtures/fit/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("json");
    BoardModel::from_json(&doc["model"].to_string()).expect("model")
}

fn run(name: &str, extra: &str) -> RouteResult {
    let o = Options::from_json(&format!(r#"{{"runs": 1, "wall_clock_s": 120.0, "topo_rounds": 1{extra}}}"#)).expect("options");
    route(&model(name), &o)
}

fn items(r: &RouteResult, class: &str) -> usize {
    r.compass.as_ref().expect("compass").items.iter().filter(|i| i.class == class).count()
}

fn pred(r: &RouteResult, class: &str) -> Option<usize> {
    r.compass.as_ref().expect("compass").predicted.get(class).copied()
}

#[test]
fn channel_fit_predicts_nothing() {
    for model in ["cuts", "edges"] {
        let r = run("channel_fit", &format!(r#", "capacity_model": "{model}""#));
        let c = r.compass.as_ref().expect("compass");
        assert_eq!(c.model, model);
        let want = if model == "cuts" { 0 } else { 2 };
        assert_eq!(c.predicted_violations, want, "{model}: {:?}", c.predicted);
        assert_eq!(items(&r, "planner_overfull"), want);
        assert!(r.violations.is_empty(), "{model}: drawn clean: {:?}", r.violations);
        // Every connection of the plan carries its planned vias and taut length.
        assert_eq!(c.conns.len(), 3, "{:?}", c.conns);
        assert!(c.conns.iter().all(|x| x.taut_nm > 0 && x.vias == 0), "{:?}", c.conns);
        // pad_exit is not predicted by this build (5.10 hooks in): absent, with a note.
        assert_eq!(pred(&r, "pad_exit"), None);
        assert!(c.notes.iter().any(|n| n.starts_with("pad_exit")), "{:?}", c.notes);
    }
}

#[test]
fn channel_overfull_predicts_one_cut_with_its_four_connections() {
    let r = run("channel_overfull", r#", "capacity_model": "cuts""#);
    let c = r.compass.as_ref().expect("compass");
    assert_eq!(pred(&r, "planner_overfull"), Some(2), "{:?}", c.predicted);
    let it: Vec<_> = c.items.iter().filter(|i| i.class == "planner_overfull").collect();
    assert_eq!(it.len(), 2);
    for x in &it {
        assert_eq!(x.conns.len(), 4, "{x:?}");
        assert_eq!(x.layers, vec!["F.Cu".to_string()]);
        // Over-full by 0.40 mm (hand number; at most the sheet's polygonisation off).
        assert!((x.over_nm - 400_000).abs() < 2_000, "{x:?}");
    }
    // The per-edge model (the drawing's packer) sees it too: no model gap.
    assert!(items(&r, "model_ref") >= 1, "{:?}", c.items);
    assert_eq!(c.predicted_violations, c.predicted.iter().filter(|(k, _)| ["planner_overfull", "via_site", "unlayerable", "foreign_copper", "forced_path"].contains(&k.as_str())).map(|(_, v)| v).sum::<usize>());
}

#[test]
fn model_gap_cut_is_seen_by_the_cut_model_only() {
    let r = run("model_gap_cut", r#", "capacity_model": "cuts""#);
    let c = r.compass.as_ref().expect("compass");
    assert_eq!(pred(&r, "planner_overfull"), Some(2), "{:?}", c.items.iter().filter(|i| i.class == "planner_overfull").collect::<Vec<_>>());
    for it in c.items.iter().filter(|i| i.class == "planner_overfull") {
        assert_eq!(it.conns.len(), 8, "{it:?}");
        assert!((it.over_nm - 100_000).abs() < 2_000, "{it:?}");
    }
    assert_eq!(items(&r, "model_ref"), 0, "the per-edge model fits every edge: {:?}", c.items);
    // The per-edge model in charge: nothing over-full predicted, the cut test not run (note).
    let e = run("model_gap_cut", r#", "capacity_model": "edges""#);
    let ce = e.compass.as_ref().expect("compass");
    assert_eq!(ce.predicted.get("planner_overfull").copied(), Some(0));
    assert!(ce.notes.iter().any(|n| n.starts_with("model_gap: not measured")), "{:?}", ce.notes);
}

#[test]
fn the_compass_is_the_same_plan_on_two_runs_and_with_stop_after_plan() {
    let a = run("channel_overfull", r#", "capacity_model": "cuts""#);
    let b = run("channel_overfull", r#", "capacity_model": "cuts""#);
    assert_eq!(a.items, b.items, "deterministic copper");
    assert_eq!(a.compass, b.compass);
    let p = run("channel_overfull", r#", "capacity_model": "cuts", "stop_after": "plan""#);
    let (ca, cp) = (a.compass.unwrap(), p.compass.unwrap());
    assert_eq!(ca.predicted, cp.predicted, "the plan before the drawing is the plan the full run drew");
    assert_eq!(ca.items, cp.items);
    assert_eq!(cp.drawn_shrinkages, 0);
}
