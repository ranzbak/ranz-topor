//! draw-absorb A0 (docs/dev/design/draw-absorb.md §4 A0): the provenance meter. Every drawn
//! violation carries its writer, its forced candidate, its band status, what its copper hits
//! (`violations[].prov`), its drawn compass class and fault class F1-F7, and the result counts them
//! (`draw_absorb`). Measurement only: the copper is the same with or without it (checked on the
//! boards by copper hash, not here).

use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route, RouteResult};

const MM: f64 = 1e6;

fn opts() -> Options {
    Options::from_json(r#"{"runs": 1, "wall_clock_s": 60.0, "topo_rounds": 3}"#).expect("options parse")
}

/// The counts add up to the violations, each violation has a writer, a candidate and a fault class.
fn check_counts(r: &RouteResult) {
    let m = r.draw_absorb.as_ref().expect("the meter ran");
    assert_eq!(m.violations, r.violations.len());
    assert_eq!(m.by_fclass.values().sum::<usize>(), r.violations.len(), "{m:?}");
    assert_eq!(m.by_mech.values().sum::<usize>(), r.violations.len(), "{m:?}");
    for v in &r.violations {
        let p = v.prov.as_ref().expect("every topo violation has a provenance");
        assert!(["F1", "F2", "F3", "F4", "F5", "F6", "F7"].contains(&p.fclass.as_str()), "{p:?}");
        assert!(!p.mech.is_empty() && !p.cand.is_empty() && !p.class.is_empty(), "{p:?}");
    }
}

/// fx_narrow_gap_forced (tests/topo_mb1.rs): the only way is a gap too narrow for the track; the
/// forced run hits the two wall pads: static hits, written by the never-drop run.
#[test]
fn a_forced_run_against_pads_is_a_static_hit() {
    let m = BoardModel::from_json(include_str!("fixtures/fx-narrow-gap.json")).expect("fixture parses");
    let r = route(&m, &opts());
    assert!(!r.violations.is_empty());
    check_counts(&r);
    for v in &r.violations {
        let p = v.prov.as_ref().unwrap();
        assert_eq!((p.mech.as_str(), p.hit.as_str(), p.hit_wire), ("forced_run", "static", None), "{p:?}");
        assert!(["relaxed", "funnel", "erbs", "erbs_kept", "attempt0", "attempt1", "attempt3", "attempt4", "attempt5"].contains(&p.cand.as_str()), "{p:?}");
        assert_eq!(p.w_nm, 250_000, "{p:?}");
        assert!(p.w_min_nm > 0 && p.w_min_nm <= p.w_nm && p.drawn_w_nm > 0, "{p:?}");
        assert_eq!(p.fclass, topor_engine::topo::compass::fault_class("forced_run", &p.class, false), "{p:?}");
    }
}

/// The narrow gap widened to fit one track (0.9 mm: 0.25 mm + 2 x 0.2 mm fits, two tracks need
/// 1.1 mm), with a second net through it: one is drawn legally, the other is forced through the
/// gap against it. That violation hits the other connection's track: a movable hit, with the
/// router wire and its connection named.
#[test]
fn a_forced_run_against_a_neighbour_names_the_neighbour() {
    let mut j: serde_json::Value = serde_json::from_str(include_str!("fixtures/fx-narrow-gap.json")).unwrap();
    let mm = |v: f64| serde_json::json!((v * MM) as i64);
    j["nets"].as_array_mut().unwrap().push(serde_json::json!({"code": 2, "name": "SIG2"}));
    let pads = j["pads"].as_array_mut().unwrap();
    for p in pads.iter_mut() {
        let r = p["ref"].as_str().unwrap().to_string();
        let lid = p["layers"][0].to_string();
        if r.starts_with("WN") {
            p["center"][1] = mm(4.775);
            p["shapes"][&lid]["h"] = mm(9.55);
        } else if r.starts_with("WS") {
            p["center"][1] = mm(15.225);
            p["shapes"][&lid]["h"] = mm(9.55);
        }
    }
    let a = pads.iter().find(|p| p["ref"] == "A1").unwrap().clone();
    for (r, x) in [("A2", 3.0), ("B2", 27.0)] {
        let mut q = a.clone();
        q["uuid"] = serde_json::json!(format!("absorb-{r}"));
        q["ref"] = serde_json::json!(r);
        q["net"] = serde_json::json!(2);
        q["center"] = serde_json::json!([(x * MM) as i64, (12.0 * MM) as i64]);
        pads.push(q);
    }
    let m = BoardModel::from_json(&j.to_string()).expect("fixture parses");
    let r = route(&m, &opts());
    assert_eq!(r.routed, 2, "never drop: both drawn: {:?}", r.warnings);
    assert!(!r.violations.is_empty(), "two tracks do not fit the gap");
    check_counts(&r);
    let mv: Vec<_> = r.violations.iter().filter(|v| v.prov.as_ref().unwrap().hit == "movable").collect();
    assert!(!mv.is_empty(), "the forced run hits the other net's track: {:?}", r.violations);
    for v in &mv {
        let p = v.prov.as_ref().unwrap();
        assert_eq!(p.mech, "forced_run", "{p:?}");
        assert!(p.hit_wire.is_some(), "the hit router wire: {p:?}");
        let other = r.connections.iter().find(|c| c.id != v.conn).unwrap().id;
        assert_eq!(p.hit_conn, Some(other), "{p:?}");
        assert!(v.against.contains("SIG"), "{v:?}");
    }
    let m = r.draw_absorb.as_ref().unwrap();
    assert_eq!(m.by_hit.get("movable").copied().unwrap_or(0), mv.len());
}

/// A via forced over sites blocked by another router wire's vias (tests/topo_d2.rs
/// `a_forced_via_stays_in_the_plans_class`, the blockers given a wire): its violations are tagged
/// `forced_via` / `via` with the via's index, and each hit names the blocking wire.
#[test]
fn a_forced_via_names_its_writer_and_the_wire_it_hits() {
    use topor_engine::check::Checker;
    use topor_engine::geom::Vec2;
    use topor_engine::topo::board::{self, BuildOpts};
    use topor_engine::topo::draw::{via_item, Drawer, Plan};
    use topor_engine::topo::init::InitWeights;
    use topor_engine::topo::state::State;
    use topor_engine::vias::ViaSize;
    let p = |v: f64| (v * MM) as i64;
    let pad = |i: usize, net: i32, x: f64, y: f64| {
        format!(
            r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"pth","center":[{},{}],"rotation_deg":0.0,"layers":[0, 2],"shapes":{{"0":{{"kind":"rect","w":{d},"h":{d}}},"2":{{"kind":"rect","w":{d},"h":{d}}}}},"drill":{{"w":{h},"h":{h}}}}}"#,
            p(x),
            p(y),
            d = p(0.6),
            h = p(0.3)
        )
    };
    let c = [(0.0, 0.0), (20.0, 0.0), (20.0, 20.0), (0.0, 20.0)];
    let segs: Vec<String> = (0..4).map(|i| format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(c[i].0), p(c[i].1), p(c[(i + 1) % 4].0), p(c[(i + 1) % 4].1))).collect();
    let s = format!(
        r#"{{"units":"nm","board":"t","layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],"nets":[{{"code":1,"name":"A"}},{{"code":3,"name":"C"}}],"pads":[{},{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
        pad(0, 1, 2.0, 10.0),
        pad(1, 1, 18.0, 10.0),
        segs.join(",")
    );
    let m = BoardModel::from_json(&s).unwrap();
    let b = board::build(&m, &BuildOpts { steiner_mm: 0.0, ..BuildOpts::default() }).unwrap();
    let conns = b.connections(&m);
    let mut st = State::new(&b.proj);
    let cn = conns.iter().find(|c| c.net == 1).unwrap();
    st.route_conn(cn, &InitWeights::default(), (cn.w + cn.s) / 400_000.0).expect("routes");
    let o = Options::default();
    let checker = Checker::new(&m, &o);
    let v = |x: f64, y: f64| Vec2 { x: x * MM, y: y * MM };
    let mut dr = Drawer::new(&m, &o, &b, &st, &checker);
    dr.force = true;
    // Vias of net 3, owned by router wire 7, on a 0.7 mm grid over the whole board: no legal site.
    let blocker = ViaSize { d: 1_000_000, drill: 500_000 };
    let mut y = 0.4;
    while y <= 19.6 {
        let mut x = 0.4;
        while x <= 19.6 {
            dr.commit(&via_item(3, v(x, -y), blocker, Some(7)));
            x += 0.7;
        }
        y += 0.7;
    }
    let k = st.wires[0].passes.len() / 2;
    let plan = Plan { layers: vec![0, 1], vias: vec![(k, k)], at: vec![k] };
    let mut ids = Vec::new();
    let vias = dr.place_vias(0, &plan, &mut ids).expect("never drop: a via");
    assert_eq!((vias.len(), dr.stats.via_forced), (1, 1), "{:?}", dr.stats);
    let vs = dr.via_viol.get(&0).expect("the forced via's violations");
    assert!(!vs.is_empty());
    for x in vs {
        assert_eq!((x.src.mech, x.src.cand, x.src.run), ("forced_via", "via", 0), "{x:?}");
        if x.against.contains("item:") {
            assert_eq!(x.src.hit_wire, Some(7), "the blocking via's wire: {x:?}");
        }
    }
    assert!(vs.iter().any(|x| x.src.hit_wire == Some(7)), "{vs:?}");
}

/// tests/tw2_metric.rs `model_gaps_keep_both_bands_and_report_the_violation` (erbs/gap_squeeze):
/// two bands kept across a model gap; the band drawn second carries the gap as its violation,
/// tagged `gap_kept` / `band` / `kept`, against the other band's copper (a movable hit).
#[test]
fn a_band_kept_across_a_model_gap_is_tagged_gap_kept() {
    let path = format!("{}/tests/fixtures/erbs/model-gap_squeeze.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["topo_rounds"] = serde_json::json!(1);
    val["draw_style"] = serde_json::json!("smooth");
    let r = route(&m, &serde_json::from_value(val).unwrap());
    assert!(r.erbs.as_ref().is_some_and(|e| e.gap_kept > 0), "{:?}", r.erbs);
    check_counts(&r);
    let g: Vec<_> = r.violations.iter().filter_map(|v| v.prov.as_ref()).filter(|p| p.mech == "gap_kept").collect();
    assert!(!g.is_empty(), "{:?}", r.violations);
    for p in g {
        assert_eq!((p.cand.as_str(), p.cut.as_str(), p.hit.as_str()), ("band", "kept", "movable"), "{p:?}");
        assert!(p.hit_wire.is_some(), "{p:?}");
    }
}
