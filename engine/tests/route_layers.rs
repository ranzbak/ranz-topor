//! Two-layer routing (M3 / T3.1, T3.2): B.Cu pads without a via, one via for a mixed pair,
//! tie-break to F.Cu, walls on F.Cu, site contention, pour completion, determinism.
use topor_engine::check::Checker;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::{route, RouteResult};

const MM: f64 = 1_000_000.0;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Through-hole: copper on both layers.
    Tht,
    /// SMD on F.Cu (layer 0).
    F,
    /// SMD on B.Cu (layer 31).
    B,
    /// No copper on any layer.
    None,
}

struct P {
    ref_: String,
    net: i32,
    x: f64,
    y: f64,
    kind: Kind,
}

fn p(ref_: &str, net: i32, x: f64, y: f64, kind: Kind) -> P {
    P {
        ref_: ref_.into(),
        net,
        x,
        y,
        kind,
    }
}

fn pad_json(q: &P) -> String {
    let shape = |l: u32| format!(r#""{l}": {{ "kind": "circle", "d": 1000000 }}"#);
    let (layers, shapes) = match q.kind {
        Kind::Tht => ("[0, 31]", format!("{}, {}", shape(0), shape(31))),
        Kind::F => ("[0]", shape(0)),
        Kind::B => ("[31]", shape(31)),
        Kind::None => ("[]", String::new()),
    };
    format!(
        r#"{{ "uuid": "{r}1", "ref": "{r}", "number": "1", "net": {net},
          "center": [{x}, {y}], "layers": {layers}, "shapes": {{ {shapes} }} }}"#,
        r = q.ref_,
        net = q.net,
        x = (q.x * MM) as i64,
        y = (q.y * MM) as i64
    )
}

/// 50 x 40 mm board, two routable layers, JLCPCB-like rules (0.6 / 0.3 mm default via).
fn model(pads: &[P], extra: &str) -> BoardModel {
    model_outline(
        pads,
        extra,
        r#"{ "kind": "seg", "a": [0, 0], "b": [50000000, 0] },
            { "kind": "seg", "a": [50000000, 0], "b": [50000000, 40000000] },
            { "kind": "seg", "a": [50000000, 40000000], "b": [0, 40000000] },
            { "kind": "seg", "a": [0, 40000000], "b": [0, 0] }"#,
    )
}

fn model_outline(pads: &[P], extra: &str, outline: &str) -> BoardModel {
    let nets: Vec<String> = (1..=12)
        .map(|n| format!(r#"{{ "code": {n}, "name": "N{n}" }}"#))
        .collect();
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {} ],
          "outline": [ {outline} ],
          "pads": [ {} ], {extra}
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000 }} }}"#,
        nets.join(","),
        pads.iter().map(pad_json).collect::<Vec<_>>().join(",")
    );
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"))
}

fn opts() -> Options {
    Options {
        wall_clock_s: 10.0,
        ..Options::default()
    }
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

fn tracks_on(r: &RouteResult, layer: u32) -> usize {
    r.items
        .iter()
        .filter(|i| matches!(i, Item::Track(t) if t.layer == layer))
        .count()
}

fn vias(r: &RouteResult) -> Vec<&topor_engine::model::ViaItem> {
    r.items
        .iter()
        .filter_map(|i| match i {
            Item::Via(v) => Some(v),
            _ => None,
        })
        .collect()
}

#[test]
fn route_b_cu_pads_route_without_via() {
    let m = model(
        &[
            p("A", 1, 10.0, 10.0, Kind::B),
            p("B", 1, 30.0, 10.0, Kind::B),
        ],
        "",
    );
    let r = route(&m, &opts());
    assert_eq!(r.routed, 1, "{:?}", r.connections);
    assert_eq!(r.vias, 0);
    assert!(vias(&r).is_empty());
    assert!(!r.items.is_empty());
    assert!(
        r.items
            .iter()
            .all(|i| matches!(i, Item::Track(t) if t.layer == 31)),
        "{:?}",
        r.items
    );
    assert_eq!(r.connections[0].layers, vec!["B.Cu".to_string()]);
    assert_eq!(r.connections[0].vias, 0);
    assert!(r.length_by_layer_nm.get("B.Cu").copied().unwrap_or(0) > 0);
    assert!(!r.length_by_layer_nm.contains_key("F.Cu"));
    clean(&m, &r);
}

#[test]
fn route_mixed_layers_uses_one_via() {
    let m = model(
        &[
            p("A", 1, 10.0, 20.0, Kind::F),
            p("B", 1, 30.0, 20.0, Kind::B),
        ],
        "",
    );
    let r = route(&m, &opts());
    assert_eq!(r.routed, 1, "{:?}", r.connections);
    assert_eq!(r.vias, 1);
    let v = vias(&r);
    assert_eq!(v.len(), 1);
    assert_eq!((v[0].d, v[0].drill), (600_000, 300_000));
    assert!(v[0].layers.is_empty(), "through via");
    assert_eq!(v[0].net, 1);
    assert!(
        tracks_on(&r, 0) > 0 && tracks_on(&r, 31) > 0,
        "{:?}",
        r.items
    );
    let c = &r.connections[0];
    assert_eq!(c.vias, 1);
    assert_eq!(c.via_sites.len(), 1);
    assert_eq!(c.via_sites[0], [v[0].at.x, v[0].at.y]);
    assert_eq!(c.layers, vec!["F.Cu".to_string(), "B.Cu".to_string()]);
    assert!(r.sites >= 1);
    assert!(r.length_by_layer_nm["F.Cu"] > 0 && r.length_by_layer_nm["B.Cu"] > 0);
    clean(&m, &r);
}

#[test]
fn route_tht_board_prefers_f_cu_on_ties() {
    let m = model(
        &[
            p("A", 1, 10.0, 20.0, Kind::Tht),
            p("B", 1, 30.0, 20.0, Kind::Tht),
        ],
        "",
    );
    let r = route(&m, &opts());
    assert_eq!(r.routed, 1, "{:?}", r.connections);
    assert_eq!(r.connections[0].layers, vec!["F.Cu".to_string()]);
    assert_eq!(r.vias, 0);
    assert_eq!(tracks_on(&r, 31), 0);
    clean(&m, &r);
}

/// The M2 wall fixture: VCC 20,20 -> 35,28 behind a locked GND track y = 24 mm that closes the
/// F.Cu gap to the board sides. On two layers VCC goes under the wall: two vias.
#[test]
fn route_wall_on_f_cu_goes_under_on_b_cu() {
    let m = model(
        &[
            p("A", 1, 20.0, 20.0, Kind::F),
            p("B", 1, 35.0, 28.0, Kind::F),
            p("W", 3, 1.2, 24.0, Kind::F),
            p("X", 3, 48.8, 24.0, Kind::F),
        ],
        r#""kept": [ { "uuid": "wall", "net": 3, "layer": 0, "width": 250000, "locked": true,
            "prim": { "kind": "seg", "a": [1200000, 24000000], "b": [48800000, 24000000] } } ],"#,
    );
    let r = route(&m, &opts());
    let vcc = r.connections.iter().find(|c| c.net == 1).unwrap();
    assert_eq!(vcc.status, "routed", "{vcc:?}");
    assert_eq!((r.partial, r.failed), (0, 0), "{:?}", r.connections);
    assert_eq!(vcc.vias, 2, "{vcc:?}");
    assert_eq!(vcc.layers, vec!["F.Cu".to_string(), "B.Cu".to_string()]);
    assert_eq!(vias(&r).len(), 2);
    clean(&m, &r);
}

/// Two F.Cu -> B.Cu connections on a board whose via sites are few: dummy pads form two
/// triangles whose incentres are the only sites that survive a `no_vias` keepout closing
/// everything but a window around each. Both connections route with one via each, on different
/// sites (a site is exclusive), deterministically.
#[test]
fn negotiate_site_contention_two_nets() {
    let ko = |x0: i64, y0: i64, x1: i64, y1: i64| {
        let (x0, y0, x1, y1) = (
            x0 * 1_000_000,
            y0 * 1_000_000,
            x1 * 1_000_000,
            y1 * 1_000_000,
        );
        format!(
            r#"{{ "name": "k", "layers": [0, 31], "no_vias": true,
              "poly": [[{x0}, {y0}], [{x1}, {y0}], [{x1}, {y1}], [{x0}, {y1}]] }}"#
        )
    };
    // Open windows: x in 14..16 and 24..26, y in 17..23.
    let kos = [
        ko(-1, -1, 14, 41),
        ko(16, -1, 24, 41),
        ko(26, -1, 51, 41),
        ko(14, -1, 16, 17),
        ko(14, 23, 16, 41),
        ko(24, -1, 26, 17),
        ko(24, 23, 26, 41),
    ]
    .join(",");
    let m = model(
        &[
            p("A", 1, 8.0, 15.0, Kind::F),
            p("B", 1, 40.0, 15.0, Kind::B),
            p("C", 2, 8.0, 25.0, Kind::F),
            p("D", 2, 40.0, 25.0, Kind::B),
            p("T1", 3, 13.0, 18.0, Kind::F),
            p("T2", 3, 17.0, 18.0, Kind::F),
            p("T3", 3, 15.0, 22.4, Kind::F),
            p("T4", 3, 23.0, 18.0, Kind::F),
            p("T5", 3, 27.0, 18.0, Kind::F),
            p("T6", 3, 25.0, 22.4, Kind::F),
        ],
        &format!(r#""keepouts": [ {kos} ],"#),
    );
    let r = route(&m, &opts());
    assert!(r.sites >= 2, "sites {}", r.sites);
    let a = r.connections.iter().find(|c| c.net == 1).unwrap();
    let c = r.connections.iter().find(|c| c.net == 2).unwrap();
    assert_eq!(
        (a.status.as_str(), c.status.as_str()),
        ("routed", "routed"),
        "{a:?} {c:?}"
    );
    assert!(r.iterations <= 50, "{}", r.iterations);
    assert_eq!((a.vias, c.vias), (1, 1));
    assert_ne!(a.via_sites[0], c.via_sites[0], "one site per via");
    clean(&m, &r);
    let again = route(&m, &opts());
    assert_eq!(json_no_runtime(&r), json_no_runtime(&again));
}

fn pour_json(width_mm: f64) -> String {
    let w = (width_mm * MM) as i64;
    format!(
        r#""pours": [ {{ "uuid": "zz", "net": 1, "layers": [31], "pads": ["U1.1"], "vias": [],
            "fills": [ {{ "layer": 31, "outer": [[0, 0], [{w}, 0], [{w}, 30000000], [0, 30000000]] }} ] }} ],"#
    )
}

/// GND: THT pad U1.1 inside a B.Cu pour (listed), SMD F.Cu pad U2.1 outside it. The pour
/// completion connection runs a stub from U2.1 to a via inside the fill.
#[test]
fn route_pour_completion_lands_via_in_pour() {
    let m = model(
        &[
            p("U1", 1, 10.0, 15.0, Kind::Tht),
            p("U2", 1, 40.0, 15.0, Kind::F),
        ],
        &pour_json(30.0),
    );
    let r = route(&m, &opts());
    assert_eq!(r.connections.len(), 1);
    let c = &r.connections[0];
    assert_eq!(c.to.kind, "pour");
    assert_eq!(c.status, "routed", "{c:?}");
    assert_eq!(c.vias, 1, "{c:?}");
    let v = vias(&r);
    assert_eq!(v.len(), 1);
    let (x, y) = (v[0].at.x as f64, v[0].at.y as f64);
    assert!(
        x > 0.3 * MM && x < 29.7 * MM && y > 0.3 * MM && y < 29.7 * MM,
        "via ({x}, {y}) is inside the fill"
    );
    assert_eq!(r.nets[0].status, "routed", "{:?}", r.nets);
    assert_eq!(r.nets_routed, 1);
    clean(&m, &r);
}

#[test]
fn route_pour_completion_without_site_fails_cleanly() {
    let m = model(
        &[
            p("U1", 1, 0.25, 15.0, Kind::Tht),
            p("U2", 1, 40.0, 15.0, Kind::F),
            p("A", 2, 10.0, 35.0, Kind::F),
            p("B", 2, 30.0, 35.0, Kind::F),
        ],
        &pour_json(0.5),
    );
    let r = route(&m, &opts());
    let c = r.connections.iter().find(|c| c.net == 1).unwrap();
    assert_eq!(c.status, "failed", "{c:?}");
    assert_eq!(c.reason, "no via site inside pour");
    assert!(r.iterations <= 2, "{}", r.iterations);
    let n2 = r.connections.iter().find(|c| c.net == 2).unwrap();
    assert_eq!(n2.status, "routed", "{n2:?}");
    clean(&m, &r);
}

#[test]
fn route_terminal_on_no_routable_layer_fails() {
    let m = model(
        &[
            p("A", 1, 10.0, 10.0, Kind::F),
            p("B", 1, 30.0, 10.0, Kind::None),
            p("C", 2, 10.0, 30.0, Kind::F),
            p("D", 2, 30.0, 30.0, Kind::F),
        ],
        "",
    );
    let r = route(&m, &opts());
    let c = r.connections.iter().find(|c| c.net == 1).unwrap();
    assert_eq!(c.status, "failed");
    assert_eq!(c.reason, "terminal not on any routable layer");
    assert_eq!(r.routed, 1, "{:?}", r.connections);
    clean(&m, &r);
}

#[test]
fn route_two_layers_deterministic() {
    let m = model(
        &[
            p("A", 1, 5.0, 8.0, Kind::F),
            p("B", 1, 44.0, 30.0, Kind::B),
            p("C", 2, 6.0, 30.0, Kind::B),
            p("D", 2, 45.0, 10.0, Kind::F),
            p("E", 3, 20.0, 5.0, Kind::Tht),
            p("F", 3, 25.0, 35.0, Kind::Tht),
            p("G", 4, 10.0, 20.0, Kind::F),
            p("H", 4, 40.0, 20.0, Kind::F),
            p("I", 5, 15.0, 33.0, Kind::B),
            p("J", 5, 35.0, 8.0, Kind::B),
            p("K", 6, 12.0, 12.0, Kind::F),
            p("L", 6, 38.0, 28.0, Kind::B),
        ],
        "",
    );
    let (a, b) = (route(&m, &opts()), route(&m, &opts()));
    assert_eq!(a.routed, 6, "{:?}", a.connections);
    assert_eq!(json_no_runtime(&a), json_no_runtime(&b));
    clean(&m, &a);
}

/// F -> F connection A(10,20) -> B(30,20) with a short locked F.Cu wall between (net 3,
/// y from `y0` to `y1` mm at x = 20). A cheap via makes the first iterations hop under it.
fn wall_model(y0: f64, y1: f64) -> BoardModel {
    model(
        &[
            p("A", 1, 10.0, 20.0, Kind::F),
            p("B", 1, 30.0, 20.0, Kind::F),
            p("W", 3, 20.0, y0, Kind::F),
            p("X", 3, 20.0, y1, Kind::F),
        ],
        &format!(
            r#""kept": [ {{ "uuid": "wall", "net": 3, "layer": 0, "width": 250000, "locked": true,
            "prim": {{ "kind": "seg", "a": [20000000, {}], "b": [20000000, {}] }} }} ],"#,
            (y0 * MM) as i64,
            (y1 * MM) as i64
        ),
    )
}

fn cheap_via(reduce: bool) -> Options {
    Options {
        via_cost_nm: 400_000,
        via_reduction: reduce,
        ..opts()
    }
}

#[test]
fn via_reduction_removes_detourable_via_pair() {
    let m = wall_model(17.0, 23.0);
    let off = route(&m, &cheap_via(false));
    assert_eq!(off.via_reduction, "off");
    assert_eq!(off.routed, 1, "{:?}", off.connections);
    assert_eq!(
        off.vias, 2,
        "cheap vias hop under the wall: {:?}",
        off.connections
    );
    let r = route(&m, &cheap_via(true));
    assert_eq!(r.routed, 1, "{:?}", r.connections);
    assert_eq!(r.vias, 0, "{} {:?}", r.via_reduction, r.connections);
    assert!(
        r.via_reduction
            .starts_with("removed 2 of 2 vias in 1 candidates"),
        "{}",
        r.via_reduction
    );
    assert!(vias(&r).is_empty());
    clean(&m, &r);
}

#[test]
fn via_reduction_keeps_necessary_vias_and_length_bound() {
    // F -> B pad pair: the via is necessary.
    let m = model(
        &[
            p("A", 1, 10.0, 20.0, Kind::F),
            p("B", 1, 30.0, 20.0, Kind::B),
        ],
        "",
    );
    let r = route(&m, &cheap_via(true));
    assert_eq!(r.vias, 1);
    assert!(
        r.via_reduction
            .starts_with("removed 0 of 1 vias in 1 candidates"),
        "{}",
        r.via_reduction
    );
    clean(&m, &r);
    // A 10 mm wall: the search at 4x via cost prefers the F.Cu detour (+14 %), but it is longer
    // than REDUCTION_LEN_SLACK allows: the vias stay.
    let m = wall_model(15.0, 25.0);
    let r = route(
        &m,
        &Options {
            via_cost_nm: 800_000,
            ..cheap_via(true)
        },
    );
    assert_eq!(r.routed, 1, "{:?}", r.connections);
    assert_eq!(r.vias, 2, "{} {:?}", r.via_reduction, r.connections);
    assert!(
        r.via_reduction
            .starts_with("removed 0 of 2 vias in 1 candidates"),
        "{}",
        r.via_reduction
    );
    clean(&m, &r);
}

#[test]
fn via_reduction_off_option() {
    let m = wall_model(17.0, 23.0);
    let r = route(&m, &cheap_via(false));
    assert_eq!(r.via_reduction, "off");
    assert_eq!(r.vias, 2);
    let d = route(&m, &opts());
    assert!(
        d.via_reduction.starts_with("removed"),
        "{}",
        d.via_reduction
    );
}

/// 40 x 40 mm body with a 180 degree arc (r 20 mm, centre (40, 20)) bulging to x = 60 mm in place of
/// the right edge: with the arc collapsed to its chord the pocket x > 40 mm lies "outside".
const POCKET_OUTLINE: &str = r#"{ "kind": "seg", "a": [0, 0], "b": [40000000, 0] },
    { "kind": "arc", "start": [40000000, 0], "mid": [60000000, 20000000], "end": [40000000, 40000000] },
    { "kind": "seg", "a": [40000000, 40000000], "b": [0, 40000000] },
    { "kind": "seg", "a": [0, 40000000], "b": [0, 0] }"#;

#[test]
fn route_pad_in_arc_pocket_routes() {
    for (ka, kb) in [(Kind::F, Kind::F), (Kind::B, Kind::B)] {
        let m = model_outline(
            &[p("A", 1, 50.0, 20.0, ka), p("B", 1, 10.0, 20.0, kb)],
            "",
            POCKET_OUTLINE,
        );
        let r = route(&m, &opts());
        assert_eq!(r.routed, 1, "{:?}", r.connections);
        assert_eq!(r.vias, 0);
        clean(&m, &r);
    }
}
