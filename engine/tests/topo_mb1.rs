//! M-B1 tests (never drop): a connection the drawing cannot make legal is drawn anyway on its
//! least violating geometry, priced and reported; `allow_violations = false` keeps the old
//! legal-only output. Fixture: `harness/mechfix.py fx_narrow_gap_forced` as the engine sees it
//! (SIG from (3, 10) to (27, 10) mm; walls of unconnected pads on both layers at x = 15 leave a
//! 0.5 mm gap for a 0.25 mm track with 0.2 mm clearance).

use topor_engine::check::Checker;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route;

fn gap() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/fx-narrow-gap.json")).expect("fixture parses")
}

fn opts(json: &str) -> Options {
    Options::from_json(json).expect("options parse")
}

#[test]
fn a_connection_without_legal_geometry_is_drawn_and_reported() {
    let m = gap();
    let r = route(&m, &opts(r#"{"runs": 1, "wall_clock_s": 60.0, "topo_rounds": 3}"#));
    assert_eq!((r.routed, r.connections_total), (1, 1), "{:?}", r.warnings);
    assert!(!r.violations.is_empty());
    for v in &r.violations {
        assert_eq!(v.kind, "clearance", "no copper overlap: {v:?}");
        assert!((v.at[0] as f64 / 1e6 - 15.0).abs() < 1.0 && (v.at[1] as f64 / 1e6 - 10.0).abs() < 1.0, "{v:?}");
        assert!(v.depth_nm > 0 && v.depth_nm < 200_000 && v.length_nm > 0, "{v:?}");
        assert!(v.cause.starts_with("drawing:") && v.price_mm >= 1000.0, "{v:?}");
    }
    assert_eq!(r.connections[0].violations, r.violations.len());
    // The copper written breaks exactly the rules reported.
    let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
    assert!(!rep.violations.is_empty() && rep.violations.iter().all(|v| v.against.starts_with("pad:W")), "{rep:?}");
    assert!(r.warnings.iter().any(|w| w.starts_with("never drop ([optimize] allow_violations): 1 connections written with")), "{:?}", r.warnings);
    assert!(r.warnings.iter().any(|w| w.starts_with("SIG (A1.1 -> B1.1): violation (clearance) at (")), "{:?}", r.warnings);
    // The variant table counts the connection with violations as one error, the violations apart.
    let picked = r.variants.iter().find(|v| v.picked).expect("picked variant");
    assert_eq!(picked.violations, r.violations.len());
    assert_eq!(picked.errors, 1);
    assert!(picked.penalty_mm >= 1000.0 * r.violations.len() as f64);
}

#[test]
fn legal_only_output_drops_it_as_before() {
    let r = route(&gap(), &opts(r#"{"runs": 1, "wall_clock_s": 60.0, "topo_rounds": 3, "allow_violations": false}"#));
    assert_eq!(r.routed, 0);
    assert!(r.items.is_empty() && r.violations.is_empty());
    assert!(r.connections[0].reason.starts_with("drawing:"), "{}", r.connections[0].reason);
    assert_eq!(r.variants.iter().find(|v| v.picked).expect("picked").errors, 1);
}
