//! DRC-feasible detour reference (M4a / A7, ruling M3-R15): `Options::detour_reference` adds one
//! slot search per connection on the fixed copper alone; the ghost (topological, foreign pads
//! soft) stays as it is.
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route, RouteResult};

const MM: f64 = 1_000_000.0;

fn pad(name: &str, net: i32, x: f64, y: f64, w: f64, h: f64, both: bool) -> String {
    let (w, h) = ((w * MM) as i64, (h * MM) as i64);
    let (layers, shapes) = if both {
        (
            "[0, 31]",
            format!(
                r#""0": {{ "kind": "roundrect", "w": {w}, "h": {h}, "rc": 150000 }}, "31": {{ "kind": "roundrect", "w": {w}, "h": {h}, "rc": 150000 }}"#
            ),
        )
    } else {
        ("[0]", format!(r#""0": {{ "kind": "rect", "w": {w}, "h": {h} }}"#))
    };
    format!(
        r#"{{ "uuid": "{name}", "ref": "{name}", "number": "1", "net": {net},
          "center": [{}, {}], "layers": {layers}, "shapes": {{ {shapes} }} }}"#,
        (x * MM) as i64,
        (y * MM) as i64,
    )
}

/// USB-C like row (dcsgone #208): A6 and B6 (net 1) 1.0 mm apart with the foreign pad A7
/// (net 2) between them, 0.3 x 1.0 mm pads at 0.5 mm pitch (0.2 mm gap = nothing fits between).
fn usb_row() -> BoardModel {
    let pads = [
        pad("a6", 1, 20.0, 20.0, 0.3, 1.0, true),
        pad("a7", 2, 20.5, 20.0, 0.3, 1.0, true),
        pad("b6", 1, 21.0, 20.0, 0.3, 1.0, true),
    ]
    .join(",");
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "routable": true }}, {{ "id": 31, "routable": true }} ],
          "nets": [ {{ "code": 1, "name": "N1" }}, {{ "code": 2, "name": "N2" }}, {{ "code": 3, "name": "N3" }} ],
          "outline": [
            {{ "kind": "seg", "a": [0, 0], "b": [50000000, 0] }},
            {{ "kind": "seg", "a": [50000000, 0], "b": [50000000, 40000000] }},
            {{ "kind": "seg", "a": [50000000, 40000000], "b": [0, 40000000] }},
            {{ "kind": "seg", "a": [0, 40000000], "b": [0, 0] }} ],
          "pads": [ {pads} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000 }} }}"#
    );
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"))
}

fn opts(reference: bool) -> Options {
    Options {
        wall_clock_s: 60.0,
        threads: 1,
        detour_reference: reference,
        ..Options::default()
    }
}

fn json_no_runtime(r: &RouteResult, drop_ref: bool) -> String {
    let mut v = serde_json::to_value(r).unwrap();
    let o = v.as_object_mut().unwrap();
    for k in ["runtime_s", "threads_used"] {
        o.remove(k);
    }
    if drop_ref {
        for c in o.get_mut("connections").unwrap().as_array_mut().unwrap() {
            let c = c.as_object_mut().unwrap();
            c.remove("ref_len_nm");
            c.remove("ref_kind");
        }
    }
    serde_json::to_string(&v).unwrap()
}

#[test]
fn reference_is_feasible_not_topological() {
    let r = route(&usb_row(), &opts(true));
    let c = r.connections.iter().find(|c| c.net == 1).unwrap();
    assert_eq!(c.status, "routed");
    let g = c.ghost_len_nm.expect("ghost");
    let rf = c.ref_len_nm.expect("reference");
    // the ghost is the topological estimate (foreign constraint edges soft), the reference an
    // independent slot search: a different number, never shorter than the pad-to-pad distance
    assert_eq!(c.ref_kind, "feasible");
    assert_ne!(rf, g, "reference must come from the slot search, not the ghost");
    assert!(rf as f64 >= 0.99 * MM, "ref {rf}");
    // the reference is a lower-bound-ish estimate of the committed path (no other connections)
    assert!(c.length_nm as f64 >= 0.99 * MM, "{}", c.length_nm);
}

#[test]
fn reference_off_leaves_fields_none() {
    let r = route(&usb_row(), &opts(false));
    for c in &r.connections {
        assert_eq!(c.ref_len_nm, None);
        assert_eq!(c.ref_kind, "none");
    }
    let v: serde_json::Value = serde_json::to_value(&r).unwrap();
    assert!(v["connections"][0]["ref_len_nm"].is_null());
    assert_eq!(v["connections"][0]["ref_kind"], "none");
}

#[test]
fn reference_budget_exhausted_falls_back_to_ghost() {
    let r = route(
        &usb_row(),
        &Options {
            detour_reference_budget_pct: 0.0,
            ..opts(true)
        },
    );
    let c = r.connections.iter().find(|c| c.net == 1).unwrap();
    assert_eq!(c.ref_kind, "ghost");
    assert_eq!(c.ref_len_nm, c.ghost_len_nm);
    assert!(c.ref_len_nm.is_some());
}

#[test]
fn reference_threads_byte_identical() {
    let m = usb_row();
    let off = route(&m, &opts(false));
    let on1 = route(&m, &opts(true));
    // the reference never changes the routing
    assert_eq!(json_no_runtime(&off, true), json_no_runtime(&on1, true));
    let full = |threads: usize| {
        json_no_runtime(
            &route(
                &m,
                &Options {
                    threads,
                    ..opts(true)
                },
            ),
            false,
        )
    };
    let j1 = full(1);
    assert_eq!(j1, full(2));
    assert_eq!(j1, full(4));
    assert_eq!(j1, full(1));
}
