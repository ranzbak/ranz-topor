//! TW2 METRIC (docs/dev/design/tw2-metric.md): ERBS is the default metric stage, every band is
//! committed and checked at once (`topo::joint`), a rejected band causes no cascade, and every
//! drawn run is counted once in `route.erbs` (forced runs included).
//!
//! Fixtures: the approved m3, fx and ERBS showcase sets (`tests/fixtures/{m3,fx,erbs}`), one run,
//! one round (deterministic).

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{ErbsRunReport, RouteResult};

fn route(dir: &str, p: &str, style: &str, extra: &[(&str, serde_json::Value)], hooks: &'static [&'static str]) -> RouteResult {
    let path = format!("{}/tests/fixtures/{dir}/model-{p}.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["topo_rounds"] = serde_json::json!(1);
    val["draw_style"] = serde_json::json!(style);
    for (k, v) in extra {
        val[*k] = v.clone();
    }
    let o: Options = serde_json::from_value(val).unwrap();
    let mut ctl = RunControl::new();
    ctl.set_test_hook(move |pt| hooks.contains(&pt));
    topor_engine::route::route_with(&m, &o, &ctl)
}

fn erbs(r: &RouteResult) -> ErbsRunReport {
    r.erbs.clone().unwrap_or_else(|| panic!("no route.erbs; warnings {:?}", r.warnings))
}

/// The partition of the counted runs (route.rs doc of `ErbsRunReport`).
fn partition_holds(e: &ErbsRunReport) {
    assert_eq!(e.runs, e.from_erbs + e.replaced + e.cut_back + e.forced_erbs, "{e:?}");
    assert_eq!(e.cut_back, e.cut_static + e.cut_predicted + e.cut_gap + e.cut_no_band + e.cut_junction + e.cut_redrawn, "{e:?}");
    assert_eq!(e.cut_back, e.sequential + e.forced - e.forced_erbs, "{e:?}");
    assert_eq!(e.bands, e.kept + e.rejected_static + e.rejected_predicted + e.rejected_gap, "{e:?}");
    assert_eq!(e.lost_to_other, 0, "I1: {e:?}");
    assert_eq!(e.left_reserved, 0, "{e:?}");
}

/// A copper fingerprint of the result: every item as text, sorted.
fn copper(r: &RouteResult) -> Vec<String> {
    let mut v: Vec<String> = r.items.iter().map(|i| serde_json::to_string(i).unwrap()).collect();
    v.sort();
    v
}

#[test]
fn erbs_is_the_default_and_tier_keeps() {
    let o = Options::default();
    assert_eq!(o.realise, "erbs");
    assert_eq!(o.erbs_conflict_keep, "tier");
    let r = route("m3", "nested_wraps", "smooth", &[], &[]);
    let e = erbs(&r);
    assert_eq!(e.keep, "tier");
    partition_holds(&e);
    assert!(e.runs > 0 && e.from_erbs == e.runs, "{e:?}");
    let line = r.warnings.iter().find(|w| w.contains("runs from their ERBS")).expect("ERBS line");
    assert!(line.contains(&format!("drawn: {} runs from their ERBS", e.from_erbs)), "{line}");
    assert!(line.contains("joint check ([drawing] erbs_conflict_keep = \"tier\")"), "{line}");
}

#[test]
fn sequential_has_no_erbs_report_and_count_mode_is_reported() {
    let r = route("m3", "nested_wraps", "smooth", &[("realise", serde_json::json!("sequential"))], &[]);
    assert!(r.erbs.is_none());
    assert!(!r.warnings.iter().any(|w| w.contains("runs from their ERBS")));
    let r = route("m3", "nested_wraps", "smooth", &[("erbs_conflict_keep", serde_json::json!("count"))], &[]);
    assert_eq!(erbs(&r).keep, "count");
}

/// The cascade of the qfn_fan pattern (smooth): one band has a model gap to its neighbour; before
/// the joint commit its sequential copper then rejected the three bands drawn after it. Now at most
/// that band leaves its ERBS (none while the gap is kept, `erbs_gap_keep_max_mm`).
#[test]
fn qfn_fan_smooth_rejects_only_the_band_with_the_gap() {
    let r = route("erbs", "qfn_fan", "smooth", &[], &[]);
    let e = erbs(&r);
    partition_holds(&e);
    assert!(e.cut_back <= 1, "cascade: {e:?}");
    assert_eq!(e.cut_back, e.cut_gap + e.cut_predicted, "{e:?}");
    assert_eq!(e.from_erbs + e.cut_back, e.runs, "{e:?}");
}

/// Hook "fail erbs sleeve" removes one band (no sleeve). Every other run is still drawn from its
/// ERBS: on main this cascaded on nested_wraps smooth and serpentine_bundle octilinear.
#[test]
fn a_failed_sleeve_costs_only_its_own_run() {
    for (dir, p, style) in [("m3", "nested_wraps", "smooth"), ("erbs", "serpentine_bundle", "octilinear"), ("erbs", "river_wrap", "smooth")] {
        let base = erbs(&route(dir, p, style, &[], &[]));
        let hook = erbs(&route(dir, p, style, &[], &["fail erbs sleeve"]));
        partition_holds(&base);
        partition_holds(&hook);
        assert!(hook.cut_no_band >= 1, "{dir}/{p} {style}: {hook:?}");
        assert!(hook.cut_back <= base.cut_back + 1, "{dir}/{p} {style}: cascade, base {base:?}, hook {hook:?}");
        assert!(hook.from_erbs + 1 >= base.from_erbs, "{dir}/{p} {style}: base {base:?}, hook {hook:?}");
    }
}

/// I2 order-free: the precedes order backwards keeps the joint check's result; with every band kept
/// the copper is the same.
#[test]
fn reverse_draw_order_keeps_the_kept_bands() {
    for (dir, p) in [("m3", "nested_wraps"), ("m3", "three_diagonals"), ("erbs", "river_wrap"), ("erbs", "qfn_fan"), ("erbs", "nested_u"), ("erbs", "swap_vias")] {
        let a = route(dir, p, "smooth", &[], &[]);
        let b = route(dir, p, "smooth", &[], &["reverse draw order"]);
        let (ea, eb) = (erbs(&a), erbs(&b));
        assert_eq!((ea.bands, ea.kept, ea.rejected_static, ea.rejected_predicted, ea.rejected_gap), (eb.bands, eb.kept, eb.rejected_static, eb.rejected_predicted, eb.rejected_gap), "{dir}/{p}");
        if ea.cut_back == 0 && ea.forced == 0 && ea.replaced == 0 {
            assert_eq!(copper(&a), copper(&b), "{dir}/{p}: all bands kept, the copper must not depend on the order");
        }
    }
}

/// Counter gap of main: runs drawn by never drop are counted (qfn_two_ends counted 0 runs before).
#[test]
fn forced_runs_are_counted() {
    let r = route("fx", "qfn_two_ends", "octilinear", &[], &[]);
    let e = erbs(&r);
    partition_holds(&e);
    let drawn_runs: usize = r.connections.iter().filter(|c| c.status == "routed" || c.violations > 0).count();
    assert!(e.runs >= drawn_runs, "every drawn connection has a counted run: {e:?}, {drawn_runs} drawn");
    let viol: usize = r.connections.iter().map(|c| c.violations).sum();
    if viol > 0 {
        assert!(e.forced > 0, "violations {viol} but no forced run counted: {e:?}");
    }
}

/// Every approved fixture in both styles: the partition holds and nothing is lost to another wire.
#[test]
#[ignore]
fn joint_counts_hold_on_every_approved_fixture() {
    let sets: [(&str, &[&str]); 3] = [
        ("m3", &["channel_fit", "three_diagonals", "nested_wraps", "distant_radius", "shortcut_across"]),
        ("fx", &["collinear_row", "diag_exit", "qfn_two_ends", "via_fold"]),
        ("erbs", &["river_wrap", "nested_u", "corner_turn", "gap_squeeze", "qfn_fan", "swap_vias", "serpentine_maze", "serpentine_bundle"]),
    ];
    for (dir, ps) in sets {
        for p in ps {
            for style in ["octilinear", "smooth"] {
                let r = route(dir, p, style, &[], &[]);
                let e = erbs(&r);
                let v: usize = r.connections.iter().map(|c| c.violations).sum();
                println!("TW2 {dir}/{p} {style}: viol {v} routed {}/{} {e:?}", r.routed, r.connections_total);
                partition_holds(&e);
            }
        }
    }
}

/// Force off (`allow_violations = false`): a wire that fails takes its reserved bands with it, so no
/// band copper is left that no wire owns.
#[test]
fn force_off_leaves_no_reserved_copper() {
    for (dir, p, style) in [("fx", "qfn_two_ends", "octilinear"), ("erbs", "gap_squeeze", "smooth"), ("erbs", "serpentine_bundle", "smooth")] {
        let r = route(dir, p, style, &[("allow_violations", serde_json::json!(false))], &[]);
        let e = erbs(&r);
        partition_holds(&e);
        assert_eq!(e.forced, 0, "{dir}/{p}: {e:?}");
    }
}

/// Lead decision (b) 2026-11-18: two bands too close only by a model gap up to
/// `[drawing] erbs_gap_keep_max_mm` are both kept; the gap is a clearance violation of the band drawn
/// second (reported, not hidden) instead of a forced run. 0 = the selector decides (one yields).
#[test]
fn model_gaps_keep_both_bands_and_report_the_violation() {
    for (dir, p) in [("m3", "distant_radius"), ("erbs", "gap_squeeze"), ("erbs", "river_wrap"), ("erbs", "serpentine_bundle")] {
        let r = route(dir, p, "smooth", &[], &[]);
        let e = erbs(&r);
        partition_holds(&e);
        let viol: usize = r.connections.iter().map(|c| c.violations).sum();
        println!("GAPKEPT {dir}/{p}: {} pairs, worst {:.4} mm, viol {viol}, forced {}, cut back {}: {:?}", e.gap_kept, e.gap_kept_max_mm, e.forced, e.cut_back, e.gap_kept_list);
        assert!(e.gap_kept > 0, "{dir}/{p}: {e:?}");
        assert_eq!(e.gap_kept, e.gap_kept_list.len());
        assert!(e.gap_kept_max_mm > 0.0 && e.gap_kept_max_mm <= 0.15 + 1e-9, "{dir}/{p}: {e:?}");
        assert!(e.gap_kept_list.iter().all(|g| g.overlap_mm > 0.0 && g.overlap_mm <= e.gap_kept_max_mm + 1e-12 && g.wire != g.other_wire));
        assert!(viol >= 1, "{dir}/{p}: the kept gap must show as a violation: {e:?}");
        let line = r.warnings.iter().find(|w| w.contains("runs from their ERBS")).expect("ERBS line");
        assert!(line.contains(&format!("{} pairs kept across a model gap", e.gap_kept)), "{line}");
        let off = erbs(&route(dir, p, "smooth", &[("erbs_gap_keep_max_mm", serde_json::json!(0.0))], &[]));
        partition_holds(&off);
        assert_eq!((off.gap_kept, off.gap_kept_list.len()), (0, 0), "{dir}/{p}: {off:?}");
        assert!(off.rejected_gap >= 1, "{dir}/{p}: with 0 the selector rejects a band: {off:?}");
    }
}
