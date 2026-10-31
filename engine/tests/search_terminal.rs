//! F3 terminal pre-check (`search::terminal_open`): a goal that cannot be entered statically is
//! blocked before any expansion. This file holds one test on purpose: it reads the per-process
//! search counters (`search_totals`), which the tests of one binary would share.
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route;
use topor_engine::search::{search_totals, search_totals_reset};

const MM: f64 = 1_000_000.0;

fn pad(r: &str, n: &str, net: i32, x: f64, y: f64) -> String {
    format!(
        r#"{{ "uuid": "{r}{n}", "ref": "{r}", "number": "{n}", "net": {net},
          "center": [{}, {}], "layers": [0],
          "shapes": {{ "0": {{ "kind": "circle", "d": 1000000 }} }} }}"#,
        (x * MM) as i64,
        (-y * MM) as i64
    )
}

/// 50 x 40 mm board, F.Cu only; `A.2` lies 10 mm outside the right edge: every face around its
/// vertex is outside the outline, so no edge around it is passable.
fn model() -> BoardModel {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "N1" }} ],
          "pads": [ {}, {} ],
          "outline": [
            {{ "kind": "seg", "a": [0, 0], "b": [50000000, 0] }},
            {{ "kind": "seg", "a": [50000000, 0], "b": [50000000, -40000000] }},
            {{ "kind": "seg", "a": [50000000, -40000000], "b": [0, -40000000] }},
            {{ "kind": "seg", "a": [0, -40000000], "b": [0, 0] }} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#,
        pad("A", "1", 1, 10.0, 20.0),
        pad("A", "2", 1, 60.0, 20.0)
    );
    BoardModel::from_json(&s).unwrap()
}

/// The start is open, the goal cannot be entered: `Blocked` at once with the static reason,
/// zero expansions of the slot search and of the ghost (the geometric search explored the whole
/// reachable component before conceding: dcsgone #201, 24 s), `ghost_len_nm` null, the
/// connection dead (the loop ends by the stuck rule).
#[test]
fn search_blocks_unenterable_goal_instantly() {
    let m = model();
    search_totals_reset();
    let r = route(
        &m,
        &Options {
            wall_clock_s: 600.0,
            threads: 1,
            ..Options::default()
        },
    );
    let [(_, runs, exps, cands, _), (_, g_runs, g_exps, _, _)] = search_totals();
    assert_eq!((runs, exps, cands), (0, 0, 0), "no slot search ran");
    assert_eq!((g_runs, g_exps), (0, 0), "no ghost ran");
    assert_eq!(r.routed, 0);
    let c = &r.connections[0];
    assert_eq!(c.status, "failed");
    assert_eq!(
        c.reason,
        "blocked: terminal cannot be entered (width 0.250 mm, clearance 0.200 mm)"
    );
    assert_eq!(c.ghost_len_nm, None);
    assert!(c.blocking.is_none());
    assert!(r.iterations <= 4, "{}", r.iterations);
    assert!(
        r.warnings.iter().any(|w| w.starts_with("stuck after")),
        "{:?}",
        r.warnings
    );
}
