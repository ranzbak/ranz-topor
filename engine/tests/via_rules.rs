//! A10: via sizes from the board setup (netclass, predefined list, fallback), `allowed_drills_nm`,
//! via-in-pad sites, per-net via cost and the reduction slack option.
use std::collections::HashSet;
use topor_engine::check::Checker;
use topor_engine::geom::Vec2;
use topor_engine::model::{BoardModel, NetRule, Options, Rules};
use topor_engine::obstacle::ObstacleSet;
use topor_engine::route::route;
use topor_engine::vias::{via_fallback_used, via_sites_opts, via_sizes, SiteOrigin, ViaSite, ViaSize};

const MM: f64 = 1_000_000.0;
const EPS: f64 = 5_000.0;

fn sz(d: i64, drill: i64) -> ViaSize {
    ViaSize { d, drill }
}

fn rules() -> Rules {
    Rules {
        min_via_drill: 250_000,
        min_annular: 100_000,
        min_via_diameter: 400_000,
        routing_vias: Vec::new(),
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
    // the fab profile list still contributes when the board sets nothing smaller up
    let mut p = rules();
    p.routing_vias = vec![250_000, 300_000];
    assert_eq!(
        via_sizes(&p, 0, &o),
        vec![sz(800_000, 400_000), sz(500_000, 300_000), sz(450_000, 250_000)]
    );
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
    // the profile list is filtered too
    let mut p = rules();
    p.routing_vias = vec![250_000, 300_000];
    assert_eq!(
        via_sizes(&p, 0, &o),
        vec![sz(800_000, 400_000), sz(450_000, 250_000)]
    );
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

fn smd_model() -> BoardModel {
    let s = r#"{ "layers": [ { "id": 0, "routable": true }, { "id": 31, "routable": true } ],
      "nets": [ { "code": 1, "name": "A" }, { "code": 2, "name": "B" } ],
      "outline": [
        { "kind": "seg", "a": [0, 0], "b": [30000000, 0] },
        { "kind": "seg", "a": [30000000, 0], "b": [30000000, 30000000] },
        { "kind": "seg", "a": [30000000, 30000000], "b": [0, 30000000] },
        { "kind": "seg", "a": [0, 30000000], "b": [0, 0] } ],
      "pads": [
        { "uuid": "p1", "ref": "U1", "number": "1", "net": 1, "attr": "smd",
          "center": [15000000, 15000000], "layers": [0],
          "shapes": { "0": { "kind": "rect", "w": 1000000, "h": 2000000 } } },
        { "uuid": "p2", "ref": "U2", "number": "1", "net": 2, "attr": "smd",
          "center": [25000000, 15000000], "layers": [0],
          "shapes": { "0": { "kind": "rect", "w": 1000000, "h": 2000000 } } } ],
      "rules": { "default": { "clearance": 200000, "width": 250000 }, "min_clearance": 0,
        "hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000,
        "min_via_drill": 250000, "min_annular": 100000, "min_via_diameter": 400000 } }"#;
    BoardModel::from_json(s).unwrap()
}

fn sites_in_pad(in_pad: bool) -> Vec<ViaSite> {
    let m = smd_model();
    let o = Options::default();
    let obs = ObstacleSet::build(&m, &o, &HashSet::new(), &[]);
    let ck = Checker::new(&m, &o);
    let sizes = via_sizes(&m.rules, 0, &o);
    via_sites_opts(&m, &obs, &ck, &[0, 31], &[0, 1], &sizes, EPS, in_pad)
}

#[test]
fn via_in_pad_site_inside_own_smd_pad_only_when_enabled() {
    // sites are in the math frame (y up): the pads sit at y = -15 mm
    let inside = |s: &ViaSite, cx: f64| {
        (s.at.x - cx * MM).abs() <= 0.5 * MM && (s.at.y + 15.0 * MM).abs() <= 1.0 * MM
    };
    let off = sites_in_pad(false);
    assert!(
        !off.iter().any(|s| inside(s, 15.0) || inside(s, 25.0)),
        "{:?}",
        off.iter().map(|s| s.at).collect::<Vec<_>>()
    );
    assert!(off.iter().all(|s| s.net.is_none()));
    let on = sites_in_pad(true);
    let own: Vec<_> = on
        .iter()
        .filter(|s| matches!(s.origin, SiteOrigin::InPad(_)))
        .collect();
    assert!(!own.is_empty(), "no in-pad site");
    for s in &own {
        // owned by the pad's net, inside that pad, and never at the terminal itself
        let (net, cx) = match s.origin {
            SiteOrigin::InPad(0) => (1, 15.0),
            SiteOrigin::InPad(1) => (2, 25.0),
            ref o => panic!("{o:?}"),
        };
        assert_eq!(s.net, Some(net));
        assert!(inside(s, cx), "{:?}", s.at);
        assert!(
            (s.at
                - Vec2 {
                    x: cx * MM,
                    y: -15.0 * MM
                })
            .len()
                > 1_000.0
        );
    }
    // the other sites are unchanged
    let rest: Vec<_> = on.iter().filter(|s| s.net.is_none()).map(|s| s.at).collect();
    assert!(off.iter().all(|s| rest.contains(&s.at)));
}

/// 50 x 40 mm, F -> F connection A(10, 20) -> B(30, 20) with a locked F.Cu wall (net 3) between.
fn wall_model(y0: f64, y1: f64) -> BoardModel {
    let pad = |r: &str, net: i32, x: f64, y: f64| {
        format!(
            r#"{{ "uuid": "{r}1", "ref": "{r}", "number": "1", "net": {net}, "center": [{}, {}],
              "layers": [0], "shapes": {{ "0": {{ "kind": "circle", "d": 1000000 }} }} }}"#,
            (x * MM) as i64,
            (y * MM) as i64
        )
    };
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "N1" }}, {{ "code": 3, "name": "N3" }} ],
          "outline": [ {{ "kind": "seg", "a": [0, 0], "b": [50000000, 0] }},
            {{ "kind": "seg", "a": [50000000, 0], "b": [50000000, 40000000] }},
            {{ "kind": "seg", "a": [50000000, 40000000], "b": [0, 40000000] }},
            {{ "kind": "seg", "a": [0, 40000000], "b": [0, 0] }} ],
          "pads": [ {}, {}, {}, {} ],
          "kept": [ {{ "uuid": "wall", "net": 3, "layer": 0, "width": 250000, "locked": true,
            "prim": {{ "kind": "seg", "a": [20000000, {}], "b": [20000000, {}] }} }} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000 }} }}"#,
        pad("A", 1, 10.0, 20.0),
        pad("B", 1, 30.0, 20.0),
        pad("W", 3, 20.0, y0),
        pad("X", 3, 20.0, y1),
        (y0 * MM) as i64,
        (y1 * MM) as i64
    );
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"))
}

fn base() -> Options {
    Options {
        wall_clock_s: 20.0,
        via_reduction: false,
        ..Options::default()
    }
}

#[test]
fn via_cost_by_net_overrides_default() {
    let m = wall_model(17.0, 23.0);
    let with = |g: i64, net: i64| Options {
        via_cost_nm: g,
        via_cost_by_net: [("N1".to_string(), net)].into_iter().collect(),
        ..base()
    };
    // global cost low, N1 expensive: the detour on F.Cu wins
    let r = route(&m, &with(400_000, 20_000_000));
    assert_eq!((r.routed, r.vias), (1, 0), "{:?}", r.connections);
    // global cost high, N1 cheap: hop under the wall
    let r = route(&m, &with(20_000_000, 400_000));
    assert_eq!((r.routed, r.vias), (1, 2), "{:?}", r.connections);
    // an override for another net changes nothing
    let o = Options {
        via_cost_nm: 400_000,
        via_cost_by_net: [("N3".to_string(), 20_000_000)].into_iter().collect(),
        ..base()
    };
    assert_eq!(route(&m, &o).vias, 2);
}

#[test]
fn reduction_slack_from_options() {
    assert_eq!(Options::default().reduction_len_slack_pct, 10.0);
    assert!((Options::default().reduction_len_slack() - 1.10).abs() < 1e-12);
    let o = Options {
        reduction_len_slack_pct: 50.0,
        ..Options::default()
    };
    assert!((o.reduction_len_slack() - 1.5).abs() < 1e-12);
    // 10 mm wall at 4x via cost: the F.Cu detour is longer than +10 %: the vias stay ...
    let m = wall_model(15.0, 25.0);
    let o = Options {
        via_cost_nm: 800_000,
        via_reduction: true,
        ..base()
    };
    let r = route(&m, &o);
    assert_eq!(r.vias, 2, "{} {:?}", r.via_reduction, r.connections);
    // ... and are removed when the user accepts a 100 % longer route
    let r = route(
        &m,
        &Options {
            reduction_len_slack_pct: 100.0,
            ..o
        },
    );
    assert_eq!(r.vias, 0, "{} {:?}", r.via_reduction, r.connections);
}

/// Plane-via ring (`via_sites_ring`): a ring pad gets candidates on every side, not only its
/// four axis escape sites; every ring candidate keeps the via clear of the pad (it fits) and
/// lies on one of the two ring radii; an empty ring list is the old site set.
#[test]
fn plane_via_ring_offers_sites_all_around_the_pad() {
    use topor_engine::vias::{via_sites_ring, RING_COUNT, RING_DIRS};
    let m = smd_model();
    let o = Options::default();
    let obs = ObstacleSet::build(&m, &o, &HashSet::new(), &[]);
    let ck = Checker::new(&m, &o);
    let sizes = via_sizes(&m.rules, 0, &o);
    let plain = via_sites_ring(&m, &obs, &ck, &[0, 31], &[0, 1], &sizes, EPS, false, &[], &[]);
    let old = via_sites_opts(&m, &obs, &ck, &[0, 31], &[0, 1], &sizes, EPS, false);
    assert_eq!(plain.iter().map(|s| s.at).collect::<Vec<_>>(), old.iter().map(|s| s.at).collect::<Vec<_>>());
    let ring = via_sites_ring(&m, &obs, &ck, &[0, 31], &[0, 1], &sizes, EPS, false, &[], &[0]);
    let c = Vec2 { x: 15.0 * MM, y: -15.0 * MM };
    let near = |v: &[ViaSite]| v.iter().filter(|s| (s.at - c).len() < 3.0 * MM).count();
    assert!(near(&ring) >= near(&plain) + RING_DIRS, "{} vs {}", near(&ring), near(&plain));
    // a site in every 45-degree sector around the pad
    for k in 0..8 {
        let lo = k as f64 * std::f64::consts::FRAC_PI_4 - std::f64::consts::PI;
        assert!(
            ring.iter().any(|s| {
                let d = s.at - c;
                let a = d.y.atan2(d.x);
                d.len() < 3.0 * MM && a >= lo && a < lo + std::f64::consts::FRAC_PI_4
            }),
            "no ring site in sector {k}"
        );
    }
    for s in &ring {
        assert!(ck.via_fits(s.at, ViaSize { d: s.max_d, drill: s.max_drill }));
    }
    assert_eq!(RING_COUNT, 2);
}

/// `plane_via_ring` is on and `blame_vias` off by default (older option JSON too); both can be
/// switched.
#[test]
fn ring_on_blame_off_by_default() {
    for o in [Options::default(), Options::from_json("{}").unwrap()] {
        assert!(o.plane_via_ring && !o.blame_vias);
    }
    let o = Options::from_json(r#"{ "plane_via_ring": false, "blame_vias": true }"#).unwrap();
    assert!(!o.plane_via_ring && o.blame_vias);
}
