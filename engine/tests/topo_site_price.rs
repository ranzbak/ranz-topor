//! TW6 parts A + C (`[vias] site_price`, docs/dev/design/tw6-via-sites.md A, C): one site room
//! (`Packer::site_room`) read by the plan, the drawing and the pad exits' via term; a via without a
//! site pays its count plus the graded deficit `(r - room) / (d + s)`; a layer change at a
//! continued dangling end's lead end takes the lead's via end as its site.

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route_with;
use topor_engine::topo::pack::Packer;

const MM: f64 = 1e6;

#[test]
fn site_price_is_opt_in_and_parses() {
    assert_eq!(Options::default().via_site_price, "off");
    let o = Options::from_json(r#"{"via_site_price": "graded"}"#).expect("options parse");
    assert_eq!(o.via_site_price, "graded");
}

/// The deficit is 0 where the via fits, grows with the shortfall (no step above 0), and is capped
/// at the via size inside an obstacle (room -inf).
#[test]
fn the_deficit_is_graded_and_capped() {
    let d = 0.6 * MM;
    assert_eq!(Packer::site_deficit(0.3 * MM, d), 0.0);
    assert_eq!(Packer::site_deficit(0.5 * MM, d), 0.0);
    let a = Packer::site_deficit(0.25 * MM, d);
    let b = Packer::site_deficit(0.1 * MM, d);
    assert!((a - 0.05 * MM).abs() < 1.0 && (b - 0.2 * MM).abs() < 1.0, "{a} {b}");
    assert_eq!(Packer::site_deficit(f64::NEG_INFINITY, d), d);
    assert_eq!(Packer::site_deficit(-5.0 * MM, d), d);
}

fn qmtech_crop() -> BoardModel {
    let path = format!("{}/tests/fixtures/qmtech-usb-crop.json", env!("CARGO_MANIFEST_DIR"));
    BoardModel::from_json(&std::fs::read_to_string(path).expect("fixture")).expect("fixture parses")
}

/// The graded site price sits in the wire's CM1 entry (`Frame::wire_over`), so every refinement
/// still ends with the stored via prices equal to fresh ones and the local judge's via focus stays
/// exact; with the via discs at their heights and the pad exits (the combination the A/B measures).
#[test]
fn site_prices_stay_fresh_and_focus_exact() {
    use std::sync::atomic::Ordering;
    use topor_engine::topo::refine::viacheck;
    let m = qmtech_crop();
    let o = Options::from_json(
        r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 2, "via_cuts": true, "via_site_price": "graded", "via_disc_height": true, "pad_exits": true}"#,
    )
    .expect("options parse");
    viacheck::ON.store(true, Ordering::Relaxed);
    viacheck::FOCUS_ON.store(true, Ordering::Relaxed);
    let (b0, f0) = (viacheck::BAD.load(Ordering::Relaxed), viacheck::FOCUS_BAD.load(Ordering::Relaxed));
    let (n0, g0) = (viacheck::CHECKED.load(Ordering::Relaxed), viacheck::FOCUS_CHECKED.load(Ordering::Relaxed));
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let _ = route_with(&m, &o, &ctl);
    let (bad, n) = (viacheck::BAD.load(Ordering::Relaxed) - b0, viacheck::CHECKED.load(Ordering::Relaxed) - n0);
    let (fbad, fn_) = (viacheck::FOCUS_BAD.load(Ordering::Relaxed) - f0, viacheck::FOCUS_CHECKED.load(Ordering::Relaxed) - g0);
    assert!(n > 0 && fn_ > 0, "the case does not show: {n} via entries, {fn_} judgements checked");
    assert_eq!(bad, 0, "{bad} stale via prices of {n}");
    assert_eq!(fbad, 0, "{fbad} of {fn_} local judgements differ from the judge with every wire in focus");
}

/// Off = the model before TW6: the same routed items with the option off as with it absent
/// (the default), and a run with it on completes (determinism of the default copper is checked on
/// the boards by the A/B).
#[test]
fn off_is_the_default_copper() {
    let m = qmtech_crop();
    let base = r#""wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 1, "via_cuts": true"#;
    let run = |extra: &str| {
        let o = Options::from_json(&format!("{{{base}{extra}}}")).expect("options parse");
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        let r = route_with(&m, &o, &ctl);
        format!("{:?}", r.items)
    };
    assert_eq!(run(""), run(r#", "via_site_price": "off""#));
    let _ = run(r#", "via_site_price": "graded""#);
}
