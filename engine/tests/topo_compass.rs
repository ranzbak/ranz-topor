//! M1 compass (`RouteResult::compass`, `topo/compass.rs`): the plan's predicted items per class on
//! three of the user-approved fit patterns (`harness/fit_patterns.py`, exported to
//! `tests/fixtures/fit/`). The expected counts are derived here from the fixture geometry and the
//! board rules (Default class w = 0.25 mm, s = 0.20 mm; QFN-like pads 0.30 x 0.80 mm at 0.5 mm
//! pitch), not read from the tool:
//!
//! - Critical cuts (Maley 1987 p. 40): from every corner to the closest point of every other
//!   feature. Across a channel between two facing pad sides (the pads' long sides, 0.80 mm) the
//!   top corner's closest point on the other pad is the facing top corner, the bottom corner's
//!   the facing bottom corner: two horizontal cuts of the same length, crossed by the same wires.
//!   They bound one channel, so they are one physical bottleneck reported as 2 cuts in 1
//!   bottleneck (`Compass::bottlenecks`).
//! - Need of a cut (inflated-sketch §1.2; Maley p. 32): n x w + (n + 1) x s with foreign pads at
//!   both ends; slack = length - need.
//! - Model gap (inflated-sketch §6.1): a cut over-full by the cut test that no per-edge over-full
//!   edge-layer carrying all its wires touches.
//!
//! `channel_fit`: walls at x 4.30 and 5.90 (the 3 centre lines at pitch 0.4625 about x 5.10 plus
//! 0.125 + 0.2125): cut 1.60, need 3 x 0.25 + 4 x 0.20 = 1.55, slack +0.05: nothing over-full.
//! `channel_overfull`: the same channel, 4 wires: need 1.00 + 1.00 = 2.00, slack -0.40: 2 cuts
//! (y 5.60 and 6.40) in 1 bottleneck, 4 connections each. The slot's corners are cocircular and no
//! other pad corner lies in that circle (radius 0.894 about (5.10, 6.00); the nearest, U1.7's
//! (4.00, 5.60), is 1.17 away), so both cuts are triangulation edges: the per-edge model holds the
//! 4 wires on the same segment and sees it (model gap 0).
//! `model_gap_cut`: the rows' facing sides at x 5.30 and 9.00 (7.15 -+ 1.85): AC 3.70, need 8 x
//! 0.25 + 9 x 0.20 = 3.80, slack -0.10, its bottom twin the same: 2 cuts in 1 bottleneck, 8
//! connections. TP1 / TP2 split the opening (the Delaunay check of the pattern: 1.8927 > 1.8173),
//! so no triangulation edge joins U1.10 to U2.11 and each edge holds at most the 4 wires of one
//! side: the per-edge model cannot carry all 8: model gap = 2 cuts in 1 bottleneck.
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

fn pred(r: &RouteResult, class: &str) -> Option<usize> {
    r.compass.as_ref().expect("compass").predicted.get(class).copied()
}

#[test]
fn channel_fit_predicts_nothing() {
    for model in ["cuts", "edges"] {
        let r = run("channel_fit", &format!(r#", "capacity_model": "{model}""#));
        let c = r.compass.as_ref().expect("compass");
        assert_eq!(c.model, model);
        // Hand: slack +0.05, the cut test finds nothing over-full: no model gap in either model.
        assert_eq!(pred(&r, "model_gap"), Some(0), "{model}: {:?}", c.predicted);
        if model == "cuts" {
            assert_eq!(c.predicted_violations, 0, "{:?}", c.predicted);
        }
        // Whatever the per-edge packer flags here (its oblique factor, fit_patterns README "Capacity
        // definitions") is an over-prediction: the read-only cut test says it fits.
        let edge_over = c.items.iter().filter(|i| i.class == if model == "edges" { "planner_overfull" } else { "edge_overpredict" }).map(|i| i.members).sum::<usize>();
        assert_eq!(pred(&r, "edge_overpredict"), Some(edge_over), "{model}: {:?}", c.items);
        assert!(r.violations.is_empty(), "{model}: drawn clean (solvable by construction): {:?}", r.violations);
        // Every connection of the plan carries its planned vias and taut length.
        assert_eq!(c.conns.len(), 3, "{:?}", c.conns);
        assert!(c.conns.iter().all(|x| x.taut_nm > 0 && x.vias == 0), "{:?}", c.conns);
        // pad_exit is not predicted by this build (5.10 hooks in): absent, with a note.
        assert_eq!(pred(&r, "pad_exit"), None);
        assert!(c.notes.iter().any(|n| n.starts_with("pad_exit")), "{:?}", c.notes);
    }
}

#[test]
fn channel_overfull_predicts_one_bottleneck_of_two_cuts_with_its_four_connections() {
    let r = run("channel_overfull", r#", "capacity_model": "cuts""#);
    let c = r.compass.as_ref().expect("compass");
    // Hand: 2 cuts (top and bottom of the slot), 1 bottleneck.
    assert_eq!(pred(&r, "planner_overfull"), Some(2), "{:?}", c.predicted);
    assert_eq!(c.bottlenecks.get("planner_overfull").copied(), Some(1), "{:?}", c.items);
    let it: Vec<_> = c.items.iter().filter(|i| i.class == "planner_overfull").collect();
    assert_eq!((it.len(), it[0].members, it[0].segs.len()), (1, 2, 2), "{:?}", it);
    assert_eq!(it[0].conns.len(), 4, "{:?}", it[0]);
    assert_eq!(it[0].layers, vec!["F.Cu".to_string()]);
    let ys: Vec<i64> = it[0].segs.iter().map(|s| s[0][1]).collect();
    assert!(ys.contains(&5_600_000) && ys.contains(&6_400_000), "{ys:?}");
    // Over-full by 0.40 mm (hand number; at most the sheet's polygonisation off).
    assert!((it[0].over_nm - 400_000).abs() < 2_000, "{:?}", it[0]);
    // Hand: the per-edge model holds the same 4 wires on the same segments: no model gap.
    assert_eq!(pred(&r, "model_gap"), Some(0), "{:?}", c.items);
    assert_eq!(c.predicted_violations, c.predicted.iter().filter(|(k, _)| ["planner_overfull", "via_site", "unlayerable", "foreign_copper", "forced_path", "pad_exit"].contains(&k.as_str())).map(|(_, v)| v).sum::<usize>());
}

#[test]
fn model_gap_cut_is_a_model_gap_in_both_models() {
    let across = |s: &[[i64; 2]; 2]| s[0][1] == s[1][1] && s[0][0].min(s[1][0]) == 5_300_000 && s[0][0].max(s[1][0]) == 9_000_000;
    for model in ["cuts", "edges"] {
        let r = run("model_gap_cut", &format!(r#", "capacity_model": "{model}""#));
        let c = r.compass.as_ref().expect("compass");
        // Hand: AC and its twin, over-full by 0.10, 8 connections, 1 bottleneck; the per-edge
        // model has no edge holding all 8: both are a model gap (with model = "edges" measured by
        // the compass's read-only cut test).
        let gap: Vec<_> = c.items.iter().filter(|i| i.class == "model_gap").collect();
        assert_eq!(pred(&r, "model_gap"), Some(2), "{model}: {gap:?}");
        assert_eq!((gap.len(), gap[0].members, gap[0].conns.len()), (1, 2, 8), "{model}: {gap:?}");
        assert!(gap[0].segs.iter().all(across), "{model}: {gap:?}");
        assert!((gap[0].over_nm - 100_000).abs() < 2_000, "{model}: {gap:?}");
        if model == "cuts" {
            assert_eq!(pred(&r, "planner_overfull"), Some(2), "{:?}", c.predicted);
            assert_eq!(c.bottlenecks.get("planner_overfull").copied(), Some(1));
        } else {
            // The per-edge plan never puts AC among its over-full items.
            assert!(c.items.iter().filter(|i| i.class == "planner_overfull").all(|i| !i.segs.iter().any(across)), "{:?}", c.items);
            assert!(c.notes.iter().any(|n| n.starts_with("model_gap: measured")), "{:?}", c.notes);
        }
    }
    // Off: no cut test with the per-edge model, said so.
    let e = run("model_gap_cut", r#", "capacity_model": "edges", "compass_cut_check": false"#);
    let ce = e.compass.as_ref().expect("compass");
    assert_eq!(ce.predicted.get("model_gap"), None);
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
