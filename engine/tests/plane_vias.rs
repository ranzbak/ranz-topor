//! Plane vias (0.4.2, `Options.plane_vias`): an SMD pad whose net's pour lies only on a
//! non-routed inner layer is joined by a through via within `plane_via_max_nm` of the pad.
use topor_engine::check::Checker;
use topor_engine::model::{BoardModel, Item, Options, ViaItem};
use topor_engine::route::{route, RouteResult};

const MM: f64 = 1_000_000.0;

/// 30 x 30 mm, F.Cu / B.Cu routed, In1.Cu / In2.Cu planes; GND (net 1) pour fills all of In1.
/// U1.1 (10, 15) and U2.1 (20, 15): GND SMD pads 1 x 2 mm on F.Cu; A / B: a net-2 pair.
fn model() -> BoardModel {
    let pad = |r: &str, net: i32, x: f64, y: f64| {
        format!(
            r#"{{ "uuid": "{r}1", "ref": "{r}", "number": "1", "net": {net}, "attr": "smd",
              "center": [{}, {}], "layers": [0],
              "shapes": {{ "0": {{ "kind": "rect", "w": 1000000, "h": 2000000 }} }} }}"#,
            (x * MM) as i64,
            (y * MM) as i64
        )
    };
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 4, "name": "In1.Cu", "routable": false }},
            {{ "id": 6, "name": "In2.Cu", "routable": false }}, {{ "id": 2, "name": "B.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "GND" }}, {{ "code": 2, "name": "SIG" }} ],
          "outline": [
            {{ "kind": "seg", "a": [0, 0], "b": [30000000, 0] }},
            {{ "kind": "seg", "a": [30000000, 0], "b": [30000000, 30000000] }},
            {{ "kind": "seg", "a": [30000000, 30000000], "b": [0, 30000000] }},
            {{ "kind": "seg", "a": [0, 30000000], "b": [0, 0] }} ],
          "pads": [ {}, {}, {}, {} ],
          "pours": [ {{ "uuid": "gp", "net": 1, "layers": [4], "pads": [], "vias": [],
            "fills": [ {{ "layer": 4, "outer": [[200000, 200000], [29800000, 200000],
              [29800000, 29800000], [200000, 29800000]] }} ] }} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000,
            "min_via_drill": 250000, "min_annular": 100000, "min_via_diameter": 400000 }} }}"#,
        pad("U1", 1, 10.0, 15.0),
        pad("U2", 1, 20.0, 15.0),
        pad("A", 2, 5.0, 25.0),
        pad("B", 2, 25.0, 25.0),
    );
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"))
}

fn opts() -> Options {
    Options {
        wall_clock_s: 30.0,
        threads: 1,
        ..Options::default()
    }
}

fn vias(r: &RouteResult) -> Vec<&ViaItem> {
    r.items
        .iter()
        .filter_map(|i| match i {
            Item::Via(v) => Some(v),
            _ => None,
        })
        .collect()
}

/// Gap (nm, KiCad frame) from via centre `v` to the 1 x 2 mm pad at (`cx`, 15) mm.
fn gap_to_pad(v: &ViaItem, cx: f64) -> f64 {
    let dx = ((v.at.x as f64 - cx * MM).abs() - 0.5 * MM).max(0.0);
    let dy = ((v.at.y as f64 - 15.0 * MM).abs() - 1.0 * MM).max(0.0);
    dx.hypot(dy)
}

fn clean(m: &BoardModel, r: &RouteResult) {
    let o = Options {
        margin_nm: 0,
        ..Options::default()
    };
    let rep = Checker::new(m, &o).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

#[test]
fn smd_pads_get_a_via_onto_the_inner_plane() {
    let m = model();
    let r = route(&m, &opts());
    let gnd: Vec<_> = r.connections.iter().filter(|c| c.net == 1).collect();
    assert_eq!(gnd.len(), 2, "{:?}", r.connections);
    for c in &gnd {
        assert_eq!(c.to.kind, "pour");
        assert_eq!((c.status.as_str(), c.vias), ("routed", 1), "{c:?}");
    }
    let v: Vec<_> = vias(&r).into_iter().filter(|v| v.net == 1).collect();
    assert_eq!(v.len(), 2);
    for cx in [10.0, 20.0] {
        let g = v.iter().map(|v| gap_to_pad(v, cx)).fold(f64::INFINITY, f64::min);
        assert!(g <= 1.0 * MM + 1.0, "nearest via {g} nm from pad at x = {cx}");
    }
    let sig = r.connections.iter().find(|c| c.net == 2).unwrap();
    assert_eq!(sig.status, "routed", "{sig:?}");
    clean(&m, &r);
}

#[test]
fn no_site_within_max_distance_fails_with_reason() {
    let m = model();
    let r = route(
        &m,
        &Options {
            plane_via_max_nm: 0,
            plane_via_share: false,
            ..opts()
        },
    );
    for c in r.connections.iter().filter(|c| c.net == 1) {
        assert_eq!(c.status, "failed", "{c:?}");
        assert_eq!(
            c.reason, "no via site within 0.00 mm of the terminal (plane pour)",
            "{c:?}"
        );
    }
    // With sharing, no pad has a via of its own to share: the reason names both distances.
    let r2 = route(&m, &Options { plane_via_max_nm: 0, ..opts() });
    for c in r2.connections.iter().filter(|c| c.net == 1) {
        assert_eq!(c.status, "failed", "{c:?}");
        assert_eq!(
            c.reason,
            "no via site within 0.00 mm of the terminal and no shared via within 2.00 mm (plane pour)",
            "{c:?}"
        );
    }
    assert!(vias(&r).iter().all(|v| v.net != 1));
    clean(&m, &r);
}

#[test]
fn via_in_pad_puts_the_plane_via_inside_the_pad() {
    let m = model();
    let r = route(
        &m,
        &Options {
            via_in_pad: true,
            plane_via_max_nm: 0,
            ..opts()
        },
    );
    for c in r.connections.iter().filter(|c| c.net == 1) {
        assert_eq!((c.status.as_str(), c.vias), ("routed", 1), "{c:?}");
    }
    let v: Vec<_> = vias(&r).into_iter().filter(|v| v.net == 1).collect();
    assert_eq!(v.len(), 2);
    for cx in [10.0, 20.0] {
        assert!(
            v.iter().any(|v| gap_to_pad(v, cx) == 0.0),
            "no via inside the pad at x = {cx}: {v:?}"
        );
    }
    assert_eq!(r.vias_in_pads.len(), 2, "{:?}", r.vias_in_pads);
}

#[test]
fn plane_vias_off_keeps_the_old_failure() {
    let m = model();
    let r = route(
        &m,
        &Options {
            plane_vias: false,
            ..opts()
        },
    );
    for c in r.connections.iter().filter(|c| c.net == 1) {
        assert_eq!(c.status, "failed", "{c:?}");
        assert_eq!(c.reason, "terminal not on any routable layer");
    }
}

/// Shared plane vias are on by default (2 mm); with sharing off the plane fixtures route the
/// same and stay clean (on this board every pad has a free site of its own).
#[test]
fn plane_via_share_default_and_same_without_contention() {
    let d = Options::default();
    assert!(d.plane_via_share && d.plane_via_share_max_nm == 2_000_000);
    assert!(Options::from_json("{}").unwrap().plane_via_share);
    let m = model();
    let on = route(&m, &opts());
    let off = route(&m, &Options { plane_via_share: false, ..opts() });
    assert_eq!((on.routed, on.vias), (off.routed, off.vias));
    clean(&m, &on);
}
