//! A10: via sizes from the board setup (netclass, predefined list, fallback), `allowed_drills_nm`,
//! via-in-pad sites, per-net via cost and the reduction slack option.
use topor_engine::model::{NetRule, Options, Rules};
use topor_engine::vias::{via_fallback_used, via_sizes, ViaSize};


fn sz(d: i64, drill: i64) -> ViaSize {
    ViaSize { d, drill }
}

fn rules() -> Rules {
    Rules {
        min_via_drill: 250_000,
        min_annular: 100_000,
        min_via_diameter: 400_000,
        default: NetRule {
            via_d: 800_000,
            via_drill: 400_000,
            ..Default::default()
        },
        ..Rules::default()
    }
}

#[test]
fn via_sizes_prefer_netclass_then_predefined_smaller() {
    let mut r = rules();
    r.predefined_vias = vec![
        [450_000, 250_000],
        [900_000, 500_000], // larger than the netclass via: not an alternative
        [600_000, 300_000],
        [350_000, 200_000], // below the board's minimum drill / diameter
        [800_000, 400_000], // the netclass via itself, listed once only
    ];
    let o = Options::default();
    assert_eq!(
        via_sizes(&r, 0, &o),
        vec![sz(800_000, 400_000), sz(600_000, 300_000), sz(450_000, 250_000)]
    );
    // a net class with its own via first, the predefined ones below it
    r.per_net.insert(
        5,
        NetRule {
            via_d: 600_000,
            via_drill: 300_000,
            ..Default::default()
        },
    );
    assert_eq!(
        via_sizes(&r, 5, &o),
        vec![sz(600_000, 300_000), sz(450_000, 250_000)]
    );
}

#[test]
fn no_fab_profile_vias_only_the_boards_own() {
    // User decision 2026-11-12: the engine holds no fab profile. An older model still carrying
    // `profile` / `drill_list` / `routing_vias` reads fine, and those keys add no via size: a
    // board without pre-defined vias routes with its net class via only.
    let old = r#"{"min_via_drill": 250000, "min_annular": 100000, "min_via_diameter": 400000,
        "profile": "jlcpcb_1oz", "drill_list": [200000, 250000, 300000],
        "routing_vias": [250000, 300000, 400000, 500000],
        "default": {"via_d": 800000, "via_drill": 400000}}"#;
    let r: Rules = serde_json::from_str(old).expect("an old model's rules still deserialize");
    let o = Options::default();
    assert_eq!(via_sizes(&r, 0, &o), vec![sz(800_000, 400_000)]);
    // the board's pre-defined vias are the alternatives
    let mut p = r.clone();
    p.predefined_vias = vec![[500_000, 300_000], [450_000, 250_000]];
    assert_eq!(
        via_sizes(&p, 0, &o),
        vec![sz(800_000, 400_000), sz(500_000, 300_000), sz(450_000, 250_000)]
    );
    // and the rules serialize without any profile key
    let js = serde_json::to_string(&r).unwrap();
    assert!(!js.contains("routing_vias") && !js.contains("drill_list") && !js.contains("\"profile\""), "{js}");
}

#[test]
fn allowed_drills_filters_predefined() {
    let mut r = rules();
    r.predefined_vias = vec![[450_000, 250_000], [600_000, 300_000]];
    let o = Options {
        allowed_drills_nm: vec![250_000, 400_000],
        ..Options::default()
    };
    // 0.3 mm is not allowed; the net's own 0.4 mm drill is never dropped
    assert_eq!(via_sizes(&r, 0, &o), vec![sz(800_000, 400_000), sz(450_000, 250_000)]);
    let none = Options {
        allowed_drills_nm: vec![500_000],
        ..Options::default()
    };
    assert_eq!(via_sizes(&r, 0, &none), vec![sz(800_000, 400_000)]);
}

#[test]
fn fallback_only_when_nothing_set_up() {
    let o = Options::default();
    let mut r = rules(); // default class at the factory 0.8 / 0.4, no predefined vias
    r.via_fallback = [600_000, 300_000];
    assert!(via_fallback_used(&r, 0));
    assert_eq!(via_sizes(&r, 0, &o), vec![sz(600_000, 300_000)]);
    // clamped by the board's minimums
    r.via_fallback = [300_000, 200_000];
    assert_eq!(via_sizes(&r, 0, &o), vec![sz(400_000, 250_000)]);
    r.via_fallback = [600_000, 300_000];
    // a predefined list means the board is set up
    r.predefined_vias = vec![[450_000, 250_000]];
    assert!(!via_fallback_used(&r, 0));
    assert_eq!(via_sizes(&r, 0, &o)[0], sz(800_000, 400_000));
    // so does a net class with its own size
    r.predefined_vias.clear();
    r.default.via_d = 700_000;
    r.default.via_drill = 350_000;
    assert!(!via_fallback_used(&r, 0));
    assert_eq!(via_sizes(&r, 0, &o), vec![sz(700_000, 350_000)]);
    // the default fallback is the factory size itself: nothing changes, nothing is reported
    let d = rules();
    assert_eq!(d.via_fallback, [800_000, 400_000]);
    assert!(!via_fallback_used(&d, 0));
}
