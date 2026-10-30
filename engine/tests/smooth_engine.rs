use topor_engine::check::Checker;
use topor_engine::model::{BoardModel, Item, Options, TrackItem};
use topor_engine::smooth::{smooth, SmoothResult};

const MM: f64 = 1_000_000.0;

fn nm(x: f64) -> i64 {
    (x * MM).round() as i64
}

/// Pad (math-frame mm in, KiCad-frame JSON out).
fn pad(id: &str, net: i32, x: f64, y: f64, d: i64) -> String {
    format!(
        r#"{{ "uuid": "{id}", "ref": "U{id}", "number": "1", "net": {net}, "attr": "smd",
            "center": [{}, {}], "rotation_deg": 0.0, "layers": [0],
            "shapes": {{ "0": {{ "kind": "circle", "d": {d} }} }} }}"#,
        nm(x),
        nm(-y)
    )
}

/// Straight kept segment (math-frame mm in).
fn seg(u: &str, net: i32, a: (f64, f64), b: (f64, f64)) -> String {
    format!(
        r#"{{ "uuid": "{u}", "net": {net}, "layer": 0, "width": 250000,
            "prim": {{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}, "locked": false }}"#,
        nm(a.0),
        nm(-a.1),
        nm(b.0),
        nm(-b.1)
    )
}

fn model(pads: &[String], kept: &[String], extra: &str) -> BoardModel {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu", "kind": "signal", "routable": true }} ],
            "nets": [ {{ "code": 2, "name": "FOREIGN" }}, {{ "code": 5, "name": "N5" }},
                      {{ "code": 6, "name": "N6" }}, {{ "code": 7, "name": "GND" }} ],
            "pads": [ {} ], "kept": [ {} ], "outline": [ ] {extra},
            "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }},
                       "min_clearance": 0, "hole_clearance": 250000, "edge_clearance": 200000 }} }}"#,
        pads.join(","),
        kept.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn opts(extra: &str) -> Options {
    Options::from_json(&format!(
        r#"{{ "existing_copper": "smooth", "margin_nm": 5000 {extra} }}"#
    ))
    .unwrap()
}

/// P1 (0,0), P2 (10,0) net 5, foreign pad net 2 (d 0.8) at (5, 0.5); hand V below the pad.
fn l_route() -> BoardModel {
    model(
        &[
            pad("1", 5, 0.0, 0.0, 1_000_000),
            pad("2", 5, 10.0, 0.0, 1_000_000),
            pad("f", 2, 5.0, 0.5, 800_000),
        ],
        &[
            seg("h1", 5, (0.0, 0.0), (5.0, -1.0)),
            seg("h2", 5, (5.0, -1.0), (10.0, 0.0)),
        ],
        "",
    )
}

fn items_of(r: &SmoothResult) -> Vec<&TrackItem> {
    r.items
        .iter()
        .map(|i| match i {
            Item::Track(t) => t,
            _ => panic!("via"),
        })
        .collect()
}

fn ends(t: &TrackItem) -> [(i32, i32); 2] {
    let (s, e) = (t.prim.start(), t.prim.end());
    [(s.x, s.y), (e.x, e.y)]
}

#[test]
fn smooth_l_route_around_pad_becomes_arc_chain() {
    let m = l_route();
    let r = smooth(&m, &opts(""));
    assert_eq!(r.wires_total, 1);
    assert_eq!(r.wires_taut, 1, "{:?}", r.wires);
    assert_eq!(r.wires[0].pattern, "SAS", "{:?}", r.wires);
    assert_eq!(r.wires[0].status, "taut");
    let mut rep = r.replaced.clone();
    rep.sort();
    assert_eq!(rep, vec!["h1".to_string(), "h2".to_string()]);
    assert!(r.total_length_nm < r.hand_length_nm);
    let n5 = r.nets.iter().find(|n| n.net == 5).unwrap();
    assert_eq!(n5.status, "taut");
    assert_eq!((r.nets_total, r.nets_taut), (1, 1));
    assert_eq!(r.threads_used, 1);
}

#[test]
fn smooth_output_items_pass_checker() {
    let m = l_route();
    let o = opts("");
    let r = smooth(&m, &o);
    assert!(!r.items.is_empty());
    let mut m2 = m.clone();
    m2.kept.retain(|k| !r.replaced.contains(&k.uuid));
    let rep = Checker::new(&m2, &o).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

#[test]
fn smooth_dirty_hand_wire_falls_back() {
    // Foreign pad right next to P1: the start point lies inside its inflated circle.
    let m = model(
        &[
            pad("1", 5, 0.0, 0.0, 400_000),
            pad("2", 5, 10.0, 0.0, 400_000),
            pad("f", 2, 0.0, 0.7, 800_000),
        ],
        &[seg("h", 5, (0.0, 0.0), (10.0, 0.0))],
        "",
    );
    let r = smooth(&m, &opts(""));
    assert_eq!(r.wires.len(), 1);
    let w = &r.wires[0];
    assert_eq!(w.status, "fallback", "{w:?}");
    assert!(
        w.reason.starts_with("infeasible") || w.reason.starts_with("inside"),
        "{}",
        w.reason
    );
    assert!(r.items.is_empty() && r.replaced.is_empty());
    assert_eq!(
        r.nets.iter().find(|n| n.net == 5).unwrap().status,
        "fallback"
    );
}

#[test]
fn smooth_dangling_end_stays_fixed() {
    let m = model(
        &[
            pad("1", 5, 0.0, 0.0, 1_000_000),
            pad("f", 2, 2.5, 1.0, 800_000),
        ],
        &[seg("h", 5, (0.0, 0.0), (5.0, 5.0))],
        "",
    );
    let r = smooth(&m, &opts(""));
    assert_eq!(r.wires_taut, 1, "{:?}", r.wires);
    let items = items_of(&r);
    // KiCad frame: y flipped.
    assert_eq!(ends(items[items.len() - 1])[1], (5_000_000, -5_000_000));
    assert_eq!(ends(items[0])[0], (0, 0));
}

#[test]
fn smooth_junction_endpoints_bit_identical() {
    // T on a vertex: P1 (0,0) - J (10,0) - P3 (20,0), branch J -> P4 (10,5).
    let m = model(
        &[
            pad("1", 5, 0.0, 0.0, 500_000),
            pad("3", 5, 20.0, 0.0, 500_000),
            pad("4", 5, 10.0, 5.0, 500_000),
            pad("f", 2, 5.0, 0.4, 800_000),
        ],
        &[
            seg("a", 5, (0.0, 0.0), (10.0, 0.0)),
            seg("b", 5, (10.0, 0.0), (20.0, 0.0)),
            seg("c", 5, (10.0, 0.0), (10.0, 5.0)),
        ],
        "",
    );
    let r = smooth(&m, &opts(""));
    assert_eq!(r.wires_total, 3, "{:?}", r.wires);
    assert_eq!(r.wires_taut, 3, "{:?}", r.wires);
    let j = (10_000_000i32, 0i32);
    let mut touching = 0;
    for t in items_of(&r) {
        for p in ends(t) {
            if (p.0 - j.0).abs() < 100 && (p.1 - j.1).abs() < 100 {
                assert_eq!(p, j, "junction end not bit-identical");
                touching += 1;
            }
        }
    }
    assert_eq!(touching, 3);
}

#[test]
fn smooth_fixed_copper_is_skipped() {
    let m = l_route();
    let o = Options::from_json("{}").unwrap();
    let r = smooth(&m, &o);
    assert!(r.items.is_empty() && r.replaced.is_empty());
    assert_eq!(r.wires_total, 0);
    assert!(r.nets.iter().all(|n| n.status == "skipped"), "{:?}", r.nets);
    assert_eq!(r.nets_total, 0);
}

#[test]
fn smooth_zero_wall_clock_times_out() {
    let m = l_route();
    let r = smooth(&m, &opts(r#", "wall_clock_s": 0.0"#));
    assert!(r.timed_out);
    assert!(r.items.is_empty(), "{:?}", r.items);
    assert!(r.runtime_s < 0.5);
    assert_eq!(r.wires[0].status, "fallback");
}

#[test]
fn smooth_is_deterministic() {
    let m = l_route();
    let o = opts("");
    let run = || {
        let mut r = smooth(&m, &o);
        r.runtime_s = 0.0;
        serde_json::to_string(&r).unwrap()
    };
    assert_eq!(run(), run());
}

#[test]
fn smooth_self_check_fallback_restores_obstacle_for_others() {
    // A (net 5) wraps below the foreign pad through a no_tracks keepout only the checker knows.
    // B (net 6) runs below at y=-1.5 and passes with A's chain and again with A's hand copper.
    let keepout = format!(
        r#", "keepouts": [ {{ "name": "k", "layers": [0], "no_tracks": true, "no_vias": false,
             "poly": [[{}, {}], [{}, {}], [{}, {}], [{}, {}]] }} ]"#,
        nm(4.0),
        nm(0.1),
        nm(6.0),
        nm(0.1),
        nm(6.0),
        nm(0.6),
        nm(4.0),
        nm(0.6)
    );
    let m = model(
        &[
            pad("1", 5, 0.0, 0.0, 500_000),
            pad("2", 5, 10.0, 0.0, 500_000),
            pad("f", 2, 5.0, 0.5, 800_000),
            pad("3", 6, 0.0, -1.5, 500_000),
            pad("4", 6, 10.0, -1.5, 500_000),
        ],
        &[
            seg("a1", 5, (0.0, 0.0), (5.0, -0.6)),
            seg("a2", 5, (5.0, -0.6), (10.0, 0.0)),
            seg("b", 6, (0.0, -1.5), (10.0, -1.5)),
        ],
        &keepout,
    );
    let o = opts("");
    let r = smooth(&m, &o);
    assert_eq!(r.self_check_rounds, 2, "{:?}", r.wires);
    let a = r.wires.iter().find(|w| w.net == 5).unwrap();
    let b = r.wires.iter().find(|w| w.net == 6).unwrap();
    assert_eq!(a.status, "fallback", "{a:?}");
    assert!(a.reason.starts_with("self-check:"), "{}", a.reason);
    assert_eq!(b.status, "taut", "{b:?}");
    assert_eq!(r.replaced, vec!["b".to_string()]);
    let mut m2 = m.clone();
    m2.kept.retain(|k| !r.replaced.contains(&k.uuid));
    assert!(Checker::new(&m2, &o)
        .check_items(&r.items)
        .violations
        .is_empty());
}

#[test]
fn smooth_pour_served_net_is_skipped_with_reason() {
    let poly = r#", "copper_shapes": [ { "uuid": "z", "net": 7, "layer": 0,
        "poly": [[20000000, 20000000], [21000000, 20000000], [21000000, 21000000], [20000000, 21000000]] } ]"#;
    let mut m = l_route();
    m.copper_shapes =
        BoardModel::from_json(&format!(r#"{{ "layers": [ {{ "id": 0 }} ] {poly} }}"#))
            .unwrap()
            .copper_shapes;
    let r = smooth(&m, &opts(""));
    let gnd = r.nets.iter().find(|n| n.net == 7).unwrap();
    assert_eq!(gnd.status, "skipped");
    assert_eq!(gnd.reason, "no tracks (pour-served)");
    let n6 = r.nets.iter().find(|n| n.net == 6).unwrap();
    assert_eq!(
        (n6.status.as_str(), n6.reason.as_str()),
        ("skipped", "no tracks")
    );
    assert_eq!((r.nets_total, r.nets_taut), (1, 1));
    assert!(r
        .nets
        .iter()
        .find(|n| n.net == 5)
        .unwrap()
        .reason
        .is_empty());
}

#[test]
fn tangent_chain_passes_self_check() {
    // Foreign pad tangent to the straight hop: centre distance r + clr + w/2 + eps.
    let y = 0.5 + 0.2 + 0.125 + 0.005;
    let m = model(
        &[
            pad("1", 5, 0.0, 0.0, 1_000_000),
            pad("2", 5, 10.0, 0.0, 1_000_000),
            pad("f", 2, 5.0, y, 1_000_000),
        ],
        &[seg("h1", 5, (0.0, 0.0), (10.0, 0.0))],
        "",
    );
    let o = opts("");
    let r = smooth(&m, &o);
    assert_eq!(r.wires_taut, 1, "{:?}", r.wires);
    assert_eq!(r.wires[0].status, "taut");
    // At eps the output may fall short only by rounding (< 2 nm); at design clearance it is clean.
    let mut m2 = m.clone();
    m2.kept.retain(|k| !r.replaced.contains(&k.uuid));
    let strict = Checker::new(&m2, &o).check_items(&r.items);
    for v in &strict.violations {
        assert!(v.required - v.gap < 2, "{v:?}");
    }
    let o0 = Options {
        margin_nm: 0,
        ..o.clone()
    };
    assert!(Checker::new(&m2, &o0)
        .check_items(&r.items)
        .violations
        .is_empty());
}

#[test]
fn outside_pad_length_excludes_pad_radius() {
    let m = model(
        &[
            pad("1", 5, 0.0, 0.0, 1_600_000),
            pad("2", 5, 2.0047, 0.0, 1_600_000),
        ],
        &[seg("h1", 5, (0.0, 0.0), (2.0047, 0.0))],
        "",
    );
    let r = smooth(&m, &opts(""));
    assert_eq!(r.wires_taut, 1, "{:?}", r.wires);
    assert_eq!(r.wires[0].length_nm, 2_004_700);
    assert_eq!(r.wires[0].length_outside_pads_nm, 404_700);
    assert_eq!(r.wires[0].hand_outside_pads_nm, 404_700);
    assert_eq!(r.length_outside_pads_nm, 404_700);
    assert_eq!(r.hand_outside_pads_nm, 404_700);
    let n5 = r.nets.iter().find(|n| n.net == 5).unwrap();
    assert_eq!(n5.length_outside_pads_nm, 404_700);
}

#[test]
fn zero_slack_hand_wire_is_classified() {
    // Hand wire (half width 0.125) between two foreign pads (r 0.4) at exactly the design
    // clearance 0.2: the channel has zero width, so no room for the build margin.
    let y = 0.4 + 0.125 + 0.2;
    let m = model(
        &[
            pad("1", 5, 0.0, 0.0, 400_000),
            pad("2", 5, 10.0, 0.0, 400_000),
            pad("a", 2, 5.0, y, 800_000),
            pad("b", 2, 5.0, -y, 800_000),
        ],
        &[seg("h", 5, (0.0, 0.0), (10.0, 0.0))],
        "",
    );
    let r = smooth(&m, &opts(""));
    assert_eq!(r.wires.len(), 1);
    let w = &r.wires[0];
    assert_eq!(w.status, "fallback", "{w:?}");
    assert!(w.zero_slack, "{w:?}");
    assert!(w.reason.starts_with("zero slack"), "{}", w.reason);
    assert!(w.hand_slack_nm.unwrap().abs() <= 2, "{w:?}");
    assert_eq!(r.wires_zero_slack, 1);
    assert_eq!(r.nets.iter().find(|n| n.net == 5).unwrap().zero_slack, 1);
    assert!(r.items.is_empty() && r.replaced.is_empty());
}

#[test]
fn zero_slack_false_for_dirty_and_taut_wires() {
    let r = smooth(&l_route(), &opts(""));
    assert!(!r.wires[0].zero_slack && r.wires[0].hand_slack_nm.is_none());
    assert_eq!(r.wires_zero_slack, 0);
    let m = model(
        &[
            pad("1", 5, 0.0, 0.0, 400_000),
            pad("2", 5, 10.0, 0.0, 400_000),
            pad("f", 2, 0.0, 0.7, 800_000),
        ],
        &[seg("h", 5, (0.0, 0.0), (10.0, 0.0))],
        "",
    );
    let r = smooth(&m, &opts(""));
    assert_eq!(r.wires[0].status, "fallback");
    assert!(!r.wires[0].zero_slack && r.wires_zero_slack == 0);
    assert!(!r.wires[0].reason.starts_with("zero slack"));
}

fn squeezed(extra_pads: &[String], extra_kept: &[String]) -> BoardModel {
    let y = 0.4 + 0.125 + 0.2;
    let mut pads = vec![
        pad("1", 5, 0.0, 0.0, 400_000),
        pad("2", 5, 10.0, 0.0, 400_000),
        pad("a", 2, 5.0, y, 800_000),
        pad("b", 2, 5.0, -y, 800_000),
    ];
    pads.extend(extra_pads.iter().cloned());
    let mut kept = vec![seg("h", 5, (0.0, 0.0), (10.0, 0.0))];
    kept.extend(extra_kept.iter().cloned());
    model(&pads, &kept, "")
}

#[test]
fn timeout_fallback_is_not_zero_slack() {
    let r = smooth(&squeezed(&[], &[]), &opts(r#", "wall_clock_s": 0.0"#));
    let w = &r.wires[0];
    assert_eq!(w.status, "fallback");
    assert_eq!(w.reason, "timeout", "{w:?}");
    assert!(!w.zero_slack && w.hand_slack_nm.is_none());
    assert_eq!(r.wires_zero_slack, 0);
}

#[test]
fn shared_item_fallback_is_not_zero_slack() {
    // Branch T-joins the squeezed hand segment in its interior: wires share kept item "h".
    let m = squeezed(
        &[pad("3", 5, 3.0, 3.0, 400_000)],
        &[seg("br", 5, (3.0, 3.0), (3.0, 0.0))],
    );
    let r = smooth(&m, &opts(""));
    let shared: Vec<_> = r
        .wires
        .iter()
        .filter(|w| w.reason.starts_with("shares item"))
        .collect();
    assert!(!shared.is_empty(), "{:?}", r.wires);
    for w in shared {
        assert!(!w.zero_slack && w.hand_slack_nm.is_none(), "{w:?}");
    }
}
