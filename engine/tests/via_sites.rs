use std::collections::HashSet;
use topor_engine::check::Checker;
use topor_engine::geom::Vec2;
use topor_engine::model::{BoardModel, NetRule, Options};
use topor_engine::obstacle::ObstacleSet;
use topor_engine::vias::{via_sites, via_sizes, SiteOrigin, ViaSite, ViaSize, MIN_SITE_PITCH_NM};

const MM: f64 = 1_000_000.0;
const EPS: f64 = 5_000.0;

fn outline(w: i64, h: i64) -> String {
    let s = |a: (i64, i64), b: (i64, i64)| {
        format!(
            r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
            a.0 * 1_000_000,
            a.1 * 1_000_000,
            b.0 * 1_000_000,
            b.1 * 1_000_000
        )
    };
    [
        s((0, 0), (w, 0)),
        s((w, 0), (w, h)),
        s((w, h), (0, h)),
        s((0, h), (0, 0)),
    ]
    .join(",")
}

fn round_pad(i: usize, net: i32, x: f64, y: f64) -> String {
    format!(
        r#"{{ "uuid": "p{i}", "ref": "R{i}", "number": "1", "net": {net},
          "center": [{}, {}], "layers": [0, 31],
          "shapes": {{ "0": {{ "kind": "circle", "d": 1000000 }}, "31": {{ "kind": "circle", "d": 1000000 }} }} }}"#,
        (x * MM) as i64,
        (y * MM) as i64
    )
}

fn smd_rect(i: usize, net: i32, x: f64, y: f64) -> String {
    format!(
        r#"{{ "uuid": "p{i}", "ref": "U{i}", "number": "1", "net": {net}, "attr": "smd",
          "center": [{}, {}], "layers": [0],
          "shapes": {{ "0": {{ "kind": "rect", "w": 1000000, "h": 2000000 }} }} }}"#,
        (x * MM) as i64,
        (y * MM) as i64
    )
}

fn model(extra: &str) -> BoardModel {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "routable": true }}, {{ "id": 31, "routable": true }} ],
          "nets": [ {{ "code": 1, "name": "A" }}, {{ "code": 2, "name": "B" }} ],
          "outline": [ {} ], {extra},
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000,
            "min_via_drill": 250000, "min_annular": 100000, "min_via_diameter": 400000 }} }}"#,
        outline(50, 40)
    );
    BoardModel::from_json(&s).unwrap()
}

fn sites_of(m: &BoardModel, escape: &[usize]) -> Vec<ViaSite> {
    let obs = ObstacleSet::build(m, &Options::default(), &HashSet::new(), &[]);
    let ck = Checker::new(m, &Options::default());
    let sizes = via_sizes(&m.rules, 0);
    via_sites(m, &obs, &ck, &[0, 31], escape, &sizes, EPS)
}

#[test]
fn via_sizes_default_then_smaller_routing_vias() {
    let m = model(r#""pads": []"#);
    let sz = |d, drill| ViaSize { d, drill };
    assert_eq!(
        via_sizes(&m.rules, 0),
        vec![sz(600_000, 300_000), sz(450_000, 250_000)]
    );
    let mut r = m.rules.clone();
    r.per_net.insert(
        7,
        NetRule {
            via_d: 800_000,
            via_drill: 400_000,
            ..Default::default()
        },
    );
    assert_eq!(
        via_sizes(&r, 7),
        vec![
            sz(800_000, 400_000),
            // note §1: d = max(drill + 2 * min_annular, min_via_diameter) = 0.3 + 0.2
            sz(500_000, 300_000),
            sz(450_000, 250_000)
        ]
    );
    for s in via_sizes(&r, 7).iter().chain(via_sizes(&r, 0).iter()) {
        assert!(r.routing_vias.contains(&s.drill) && s.drill != 200_000 && s.drill != 150_000);
    }
}

#[test]
fn sites_fill_open_board_and_avoid_pads() {
    let pads = [(10.0, 10.0), (40.0, 10.0), (10.0, 30.0), (40.0, 30.0)];
    let body = pads
        .iter()
        .enumerate()
        .map(|(i, p)| round_pad(i, 1, p.0, p.1))
        .collect::<Vec<_>>()
        .join(",");
    let m = model(&format!(r#""pads": [ {body} ]"#));
    let sites = sites_of(&m, &[]);
    assert!(sites.len() >= 4, "{}", sites.len());
    let ck = Checker::new(&m, &Options::default());
    for s in &sites {
        assert!(s.max_d > 0);
        assert!(ck.via_fits(
            s.at,
            ViaSize {
                d: s.max_d,
                drill: s.max_drill
            }
        ));
        for p in &m.pads {
            let d = (s.at - p.center.to_vec2()).len();
            assert!(d >= 0.5 * MM + 0.3 * MM + 0.2 * MM + EPS - 1.0, "d {d}");
        }
    }
    for w in sites.windows(2) {
        assert!(
            (w[0].at.x, w[0].at.y) <= (w[1].at.x, w[1].at.y),
            "sorted by (x, y)"
        );
    }
}

#[test]
fn escape_sites_next_to_smd_terminal() {
    let m = model(&format!(r#""pads": [ {} ]"#, smd_rect(0, 1, 20.0, 20.0)));
    let c = m.pads[0].center.to_vec2();
    let hull = (0.5f64 * 0.5 + 1.0 * 1.0).sqrt() * MM;
    let r = hull + 0.2 * MM + 0.3 * MM + EPS;
    let all = sites_of(&m, &[0]);
    let esc: Vec<&ViaSite> = all
        .iter()
        .filter(|s| s.origin == SiteOrigin::Escape(0))
        .collect();
    assert_eq!(esc.len(), 4);
    for want in [(r, 0.0), (-r, 0.0), (0.0, r), (0.0, -r)] {
        let p = c + Vec2 {
            x: want.0,
            y: want.1,
        };
        assert!(
            esc.iter().any(|s| (s.at - p).len() < 1.0),
            "missing {want:?}"
        );
    }
    // a foreign pad sitting on the +x escape site removes that site
    let blocker = round_pad(1, 2, 20.0 + r / MM, 20.0);
    let m2 = model(&format!(
        r#""pads": [ {}, {} ]"#,
        smd_rect(0, 1, 20.0, 20.0),
        blocker
    ));
    let n = sites_of(&m2, &[0])
        .iter()
        .filter(|s| s.origin == SiteOrigin::Escape(0))
        .count();
    assert_eq!(n, 3);
}

#[test]
fn via_sites_respect_keepout_npth_and_via_holes() {
    let extra = format!(
        r#""pads": [ {} ],
          "keepouts": [ {{ "name": "k", "layers": [0, 31], "no_vias": true,
              "poly": [[8000000, 8000000], [22000000, 8000000], [22000000, 22000000], [8000000, 22000000]] }} ],
          "holes": [ {{ "at": [35000000, 30000000], "d": 1000000, "kind": "npth" }} ],
          "vias": [ {{ "uuid": "v1", "net": 0, "at": [40000000, 10000000], "d": 600000,
                       "drill": 300000, "layers": [0, 31] }} ]"#,
        round_pad(0, 1, 30.0, 20.0)
    );
    let m = model(&extra);
    let sites = sites_of(&m, &[]);
    assert!(!sites.is_empty());
    let hole = m.holes[0].at.to_vec2();
    let via = m.vias[0].at.to_vec2();
    for s in &sites {
        let (x, y) = (s.at.x / MM, -s.at.y / MM);
        assert!(
            !(x > 8.0 && x < 22.0 && y > 8.0 && y < 22.0),
            "inside keepout {x},{y}"
        );
        assert!((s.at - hole).len() >= 0.5 * MM + 0.25 * MM + 0.3 * MM);
        assert!((s.at - via).len() >= 0.3 * MM + 0.3 * MM + 0.2 * MM);
    }
    // candidates at those places are rejected by the exact fit
    let ck = Checker::new(&m, &Options::default());
    let big = ViaSize {
        d: 600_000,
        drill: 300_000,
    };
    assert!(!ck.via_fits(
        Vec2 {
            x: 15.0 * MM,
            y: -15.0 * MM
        },
        big
    ));
    assert!(!ck.via_fits(
        hole + Vec2 {
            x: 0.9 * MM,
            y: 0.0
        },
        big
    ));
    assert!(!ck.via_fits(
        via + Vec2 {
            x: 0.7 * MM,
            y: 0.0
        },
        big
    ));
}

#[test]
fn site_pitch_dedup_and_determinism() {
    // two pads whose facing escape sites are 0.05 mm apart
    let hull = (0.5f64 * 0.5 + 1.0 * 1.0).sqrt() * MM;
    let r = hull + 0.2 * MM + 0.3 * MM + EPS;
    let bx = 20.0 + (2.0 * r + 50_000.0) / MM;
    let extra = format!(
        r#""pads": [ {}, {} ]"#,
        smd_rect(0, 1, 20.0, 20.0),
        smd_rect(1, 2, bx, 20.0)
    );
    let m = model(&extra);
    let a = sites_of(&m, &[0, 1]);
    let b = sites_of(&m, &[0, 1]);
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(
            (x.at.x, x.at.y, x.max_d, x.max_drill),
            (y.at.x, y.at.y, y.max_d, y.max_drill)
        );
    }
    let c = m.pads[0].center.to_vec2();
    let near = a
        .iter()
        .filter(|s| (s.at.x - (c.x + r)).abs() < 0.1 * MM && (s.at.y - c.y).abs() < 0.1 * MM)
        .count();
    assert_eq!(near, 1);
    for (i, s) in a.iter().enumerate() {
        for t in &a[i + 1..] {
            assert!((s.at - t.at).len() >= MIN_SITE_PITCH_NM);
        }
    }
}
