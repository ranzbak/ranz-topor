//! A11: layer cost terms of the search -- cost of running inside a foreign-net copper pour, and a
//! per-layer preferred direction. Both are additive step costs (admissible); zero = M3 behaviour.
use topor_engine::check::Checker;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::{route, RouteResult};

const MM: f64 = 1_000_000.0;

/// 50 x 40 mm, two routable layers (F.Cu = 0, B.Cu = 31), through-hole pads A (10, 20) and B (30, 20)
/// on net 1 (both layers: no via is ever needed), optionally a zone of net 3 filling the middle
/// strip x 15..25 mm, y 5..35 mm on F.Cu.
fn model(pour_on_front: bool) -> BoardModel {
    let pad = |r: &str, x: f64, y: f64| {
        format!(
            r#"{{ "uuid": "{r}1", "ref": "{r}", "number": "1", "net": 1, "center": [{}, {}],
              "layers": [0, 31], "shapes": {{ "0": {{ "kind": "circle", "d": 1000000 }},
              "31": {{ "kind": "circle", "d": 1000000 }} }} }}"#,
            (x * MM) as i64,
            (y * MM) as i64
        )
    };
    let pours = if pour_on_front {
        r#""pours": [ { "uuid": "z1", "net": 3, "layers": [0], "pads": [], "vias": [],
            "fills": [ { "layer": 0, "holes": [],
              "outer": [ [15000000, 5000000], [25000000, 5000000], [25000000, 35000000], [15000000, 35000000] ] } ] } ],"#
    } else {
        ""
    };
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "N1" }}, {{ "code": 3, "name": "GND" }} ],
          "outline": [ {{ "kind": "seg", "a": [0, 0], "b": [50000000, 0] }},
            {{ "kind": "seg", "a": [50000000, 0], "b": [50000000, 40000000] }},
            {{ "kind": "seg", "a": [50000000, 40000000], "b": [0, 40000000] }},
            {{ "kind": "seg", "a": [0, 40000000], "b": [0, 0] }} ],
          "pads": [ {}, {} ], {pours}
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000 }} }}"#,
        pad("A", 10.0, 20.0),
        pad("B", 30.0, 20.0)
    );
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"))
}

fn opts() -> Options {
    Options {
        wall_clock_s: 20.0,
        ..Options::default()
    }
}

fn tracks_on(r: &RouteResult, layer: u32) -> usize {
    r.items
        .iter()
        .filter(|i| matches!(i, Item::Track(t) if t.layer == layer))
        .count()
}

fn clean(m: &BoardModel, r: &RouteResult) {
    let o = Options {
        margin_nm: 0,
        ..Options::default()
    };
    let rep = Checker::new(m, &o).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

fn json_no_runtime(r: &RouteResult) -> String {
    let mut v = serde_json::to_value(r).unwrap();
    v.as_object_mut().unwrap().remove("runtime_s");
    serde_json::to_string(&v).unwrap()
}

#[test]
fn leg_through_foreign_pour_prefers_other_layer_when_cost_set() {
    let m = model(true);
    // no cost: both layers are equally short, F.Cu wins the tie (M3)
    let free = route(
        &m,
        &Options {
            pour_cut_cost_nm: 0,
            ..opts()
        },
    );
    assert_eq!(free.routed, 1, "{:?}", free.connections);
    assert!(tracks_on(&free, 0) > 0 && tracks_on(&free, 31) == 0, "free: F {} B {}", tracks_on(&free, 0), tracks_on(&free, 31));
    // a cost of 1 mm per mm makes the pour-free B.Cu cheaper
    let r = route(
        &m,
        &Options {
            pour_cut_cost_nm: 1_000_000,
            ..opts()
        },
    );
    assert_eq!(r.routed, 1, "{:?}", r.connections);
    assert_eq!(r.vias, 0);
    assert!(tracks_on(&r, 31) > 0 && tracks_on(&r, 0) == 0, "cost: F {} B {}", tracks_on(&r, 0), tracks_on(&r, 31));
    clean(&m, &r);
}

#[test]
fn preferred_direction_cost_tilts_choice() {
    let m = model(false);
    let dirs = |f: f64, b: f64| -> std::collections::BTreeMap<String, f64> {
        [("F.Cu".to_string(), f), ("B.Cu".to_string(), b)].into_iter().collect()
    };
    // the connection is horizontal; F.Cu prefers vertical, B.Cu horizontal: B.Cu
    let r = route(
        &m,
        &Options {
            layer_direction: dirs(90.0, 0.0),
            direction_cost: 1.0,
            ..opts()
        },
    );
    assert_eq!(r.routed, 1, "{:?}", r.connections);
    assert!(tracks_on(&r, 31) > 0 && tracks_on(&r, 0) == 0, "F {} B {}", tracks_on(&r, 0), tracks_on(&r, 31));
    clean(&m, &r);
    // the other way round: F.Cu
    let r = route(
        &m,
        &Options {
            layer_direction: dirs(0.0, 90.0),
            direction_cost: 1.0,
            ..opts()
        },
    );
    assert!(tracks_on(&r, 0) > 0 && tracks_on(&r, 31) == 0);
    // a cost of zero ignores the table (tie-break F.Cu)
    let r = route(
        &m,
        &Options {
            layer_direction: dirs(90.0, 0.0),
            direction_cost: 0.0,
            ..opts()
        },
    );
    assert!(tracks_on(&r, 0) > 0 && tracks_on(&r, 31) == 0);
}

#[test]
fn costs_zero_match_m3() {
    for pour in [false, true] {
        let m = model(pour);
        let plain = route(
            &m,
            &Options {
                pour_cut_cost_nm: 0,
                ..opts()
            },
        );
        // direction table present but direction_cost 0, pour cost 0: identical result
        let with_table = route(
            &m,
            &Options {
                pour_cut_cost_nm: 0,
                direction_cost: 0.0,
                layer_direction: [("F.Cu".to_string(), 90.0)].into_iter().collect(),
                ..opts()
            },
        );
        assert_eq!(json_no_runtime(&plain), json_no_runtime(&with_table), "pour {pour}");
        assert_eq!(plain.routed, 1);
    }
    // the default is off, and a pour cost on a board without pour changes nothing
    assert_eq!(Options::default().pour_cut_cost_nm, 0);
    let m = model(false);
    let a = route(&m, &opts());
    let b = route(
        &m,
        &Options {
            pour_cut_cost_nm: 1_000_000,
            ..opts()
        },
    );
    assert_eq!(json_no_runtime(&a), json_no_runtime(&b));
}
