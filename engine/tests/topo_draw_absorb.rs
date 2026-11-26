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

// ---- draw-absorb A1 (`[drawing] forced_shrink`) ----

fn with(m: &str, f: impl Fn(&mut serde_json::Value)) -> BoardModel {
    let mut j: serde_json::Value = serde_json::from_str(m).unwrap();
    f(&mut j);
    BoardModel::from_json(&j.to_string()).expect("fixture parses")
}

fn opts_shrink(base: &Options, on: bool) -> Options {
    let mut val = serde_json::to_value(base).unwrap();
    val["forced_shrink"] = serde_json::json!(on);
    serde_json::from_value(val).unwrap()
}

/// Track widths (nm) and lengths (nm) of the routed copper.
fn tracks(r: &RouteResult) -> Vec<(i64, f64)> {
    r.items
        .iter()
        .filter_map(|it| match it {
            topor_engine::model::Item::Track(t) => {
                let (a, b) = (t.prim.start(), t.prim.end());
                Some((t.width, ((a.x as f64 - b.x as f64).powi(2) + (a.y as f64 - b.y as f64).powi(2)).sqrt()))
            }
            _ => None,
        })
        .collect()
}

/// A1 pattern `wide_bottleneck_neck` on fx_narrow_gap: the 0.25 mm track's only way is a gap
/// that fits it only at 0.1 mm over the gap (the net's floor: min track width 0.1 mm). Main
/// (forced_shrink off) draws it forced at full width, 2 clearance violations of 75 um. On: the
/// forced run is necked over the gap only, no violation, one neck warning; the rest of the track
/// (and both pad ends) keeps 0.25 mm.
#[test]
fn a_forced_run_shrinks_over_the_gap_instead_of_violating() {
    let m = with(include_str!("fixtures/fx-narrow-gap.json"), |j| j["rules"]["min_track_width"] = serde_json::json!(100_000));
    let off = route(&m, &opts_shrink(&opts(), false));
    assert_eq!(off.violations.len(), 2, "main: the forced run at full width: {:?}", off.violations);
    assert!(tracks(&off).iter().all(|t| t.0 == 250_000));
    let on = route(&m, &opts_shrink(&opts(), true));
    assert_eq!((on.routed, on.violations.len()), (1, 0), "{:?}", on.violations);
    let necks: Vec<&String> = on.warnings.iter().filter(|w| w.contains("necked from 0.250 mm to 0.100 mm")).collect();
    assert_eq!(necks.len(), 1, "one shrinkage warning: {:?}", on.warnings);
    let ts = tracks(&on);
    assert!(ts.iter().all(|t| t.0 >= 100_000 && t.0 <= 250_000), "{ts:?}");
    let thin: f64 = ts.iter().filter(|t| t.0 < 250_000).map(|t| t.1).sum();
    let all: f64 = ts.iter().map(|t| t.1).sum();
    // The gap's walls are 1 mm long (x 14.5 .. 15.5): the neck covers them and about one pitch more.
    assert!(thin > 1.0 * MM && thin < 3.0 * MM && thin < 0.2 * all, "necked {thin} of {all}");
    let ends = |r: &RouteResult| r.items.iter().filter_map(|it| match it {
        topor_engine::model::Item::Track(t) => Some((t.prim.start(), t.prim.end(), t.width)),
        _ => None,
    }).filter(|(a, b, _)| [a, b].iter().any(|p| (p.x as f64 - 3.0 * MM).abs() < 1e3 || (p.x as f64 - 27.0 * MM).abs() < 1e3)).map(|x| x.2).collect::<Vec<_>>();
    assert!(ends(&on).iter().all(|&w| w == 250_000), "pad ends at full width: {:?}", ends(&on));
}

/// The floor is the nominal width (min track width 0.25 mm): nothing to shrink, the copper is
/// main's whether forced_shrink is on or off.
#[test]
fn no_floor_below_the_width_no_shrink() {
    let m = with(include_str!("fixtures/fx-narrow-gap.json"), |j| j["rules"]["min_track_width"] = serde_json::json!(250_000));
    let (off, on) = (route(&m, &opts_shrink(&opts(), false)), route(&m, &opts_shrink(&opts(), true)));
    assert_eq!(off.items, on.items);
    assert_eq!(off.violations.len(), on.violations.len());
}

/// A gap too narrow even for the floor (the fixture's 0.2 mm: 50 um short per side after necking): the
/// shrink is graded, it is taken where it makes the violations shallower, never deeper.
#[test]
fn a_shrink_that_cannot_clear_still_shallows() {
    let m = with(include_str!("fixtures/fx-narrow-gap.json"), |j| j["rules"]["min_track_width"] = serde_json::json!(200_000));
    let (off, on) = (route(&m, &opts_shrink(&opts(), false)), route(&m, &opts_shrink(&opts(), true)));
    let d = |r: &RouteResult| r.violations.iter().map(|v| v.depth_nm).max().unwrap_or(0);
    assert!(!on.violations.is_empty() && d(&on) < d(&off), "off {:?} on {:?}", off.violations, on.violations);
    assert!(tracks(&on).iter().all(|t| t.0 >= 200_000));
}

/// A1 pattern `neck_not_at_pad`: as above with the gap 0.6 mm from pad A1's edge (walls at x
/// 4.1 .. 5.1, the pad's edge at 3.5). The neck may reach back to the pad (neck from pad), but no
/// width step lies inside the pad (KiCad's Cleanup would delete the narrow piece as a track in the
/// pad), and the track still ends at the pad's centre.
#[test]
fn a_shrink_next_to_a_pad_has_no_width_step_in_the_pad() {
    let m = with(include_str!("fixtures/fx-narrow-gap.json"), |j| {
        j["rules"]["min_track_width"] = serde_json::json!(100_000);
        for p in j["pads"].as_array_mut().unwrap() {
            if p["ref"].as_str().unwrap().starts_with('W') {
                p["center"][0] = serde_json::json!(4_600_000);
            }
        }
    });
    assert!(!route(&m, &opts_shrink(&opts(), false)).violations.is_empty(), "main: forced at full width");
    let on = route(&m, &opts_shrink(&opts(), true));
    assert_eq!((on.routed, on.violations.len()), (1, 0), "{:?}", on.violations);
    let segs: Vec<(f64, f64, i64)> = on.items.iter().filter_map(|it| match it {
        topor_engine::model::Item::Track(t) => Some((t.prim.start().x as f64 / MM, t.prim.end().x as f64 / MM, t.width)),
        _ => None,
    }).collect();
    assert!(segs.iter().any(|s| s.2 < 250_000), "necked: {segs:?}");
    assert!(segs.iter().any(|s| (s.0 - 3.0).abs() < 1e-3 || (s.1 - 3.0).abs() < 1e-3), "ends at the pad centre: {segs:?}");
    // A width step = the joint of two tracks of different width; none inside the pad (x < 3.5).
    for a in &segs {
        for b in &segs {
            if a.2 != b.2 && (a.1 - b.0).abs() < 1e-6 {
                assert!(a.1 > 3.5, "width step inside pad A1 at x {}: {segs:?}", a.1);
            }
        }
    }
}
