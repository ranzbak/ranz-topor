//! pick-necks r3 (verifier r2 survivors): the necking of the DRAWN copper in the real criteria (bests JSON),
//! measured against the real rules (pair via / pair width), on tiny generated boards.

use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route;

const MM: f64 = 1e6;

/// J1 (F.Cu) left, J2 (B.Cu) right, a pair USB+/USB- (pads 0.65 mm apart); net-rule width `w` mm, pair rule width
/// `pw` mm, net-rule via 0.6 mm, pair via 0.45 mm. `pair` false: two plain nets without a pair rule.
fn board(w: f64, pw: f64, pair: bool, wall: bool) -> BoardModel {
    let p = |v: f64| (v * MM).round() as i64;
    let c = [(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let (l2, n_pads) = if wall { (0, 2) } else { (2, 4) };
    let pads = [("J1", "1", 1, 2.0, 5.0, 0), ("J2", "1", 1, 18.0, 5.0, l2), ("J1", "2", 2, 2.0, 5.65, 0), ("J2", "2", 2, 18.0, 5.65, l2)];
    // wall: two tall through-hole pads of net 3 (both layers) leave a 0.6 mm gap, narrower than w + 2 x clearance.
    let wall_pads = [(2.35, 0.0, 4.7), (7.65, 5.3, 10.0)];
    let mut pj: Vec<String> = pads[..n_pads]
        .iter()
        .enumerate()
        .map(|(i, &(r, num, net, x, y, l))| {
            let sh = format!(r#"{{"kind":"rect","w":{0},"h":{0}}}"#, p(0.4));
            format!(
                r#"{{"uuid":"p{i}","ref":"{r}","number":"{num}","net":{net},"attr":"smd","center":[{},{}],
                    "rotation_deg":0.0,"layers":[{l}],"shapes":{{"{l}":{sh}}},"drill":null}}"#,
                p(x),
                p(y)
            )
        })
        .collect();
    if wall {
        for (k, &(cy, y0, y1)) in wall_pads.iter().enumerate() {
            let _ = cy;
            let sh = format!(r#"{{"kind":"rect","w":{},"h":{}}}"#, p(0.4), p(y1 - y0));
            pj.push(format!(
                r#"{{"uuid":"w{k}","ref":"W{k}","number":"1","net":3,"attr":"pth","center":[{},{}],"rotation_deg":0.0,"layers":[0,2],"shapes":{{"0":{sh},"2":{sh}}},"drill":{{"w":{0},"h":{0}}}}}"#,
                p(10.0),
                p(0.5 * (y0 + y1))
            ));
        }
    }
    let nr = format!(r#"{{"clearance":200000,"width":{},"via_d":600000,"via_drill":300000,"netclass":"Default","width_source":"netclass"}}"#, p(w));
    let dp = if pair {
        format!(r#"[{{"p":1,"n":2,"p_name":"USB+","n_name":"USB-","netclass":"Default","width":{},"gap":200000,"via_gap":200000,"via_d":450000,"via_drill":250000}}]"#, p(pw))
    } else {
        "[]".into()
    };
    let s = format!(
        r#"{{"units":"nm","board":"pickneck",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"USB+"}},{{"code":2,"name":"USB-"}},{{"code":3,"name":"W"}}],
            "pads":[{}],"outline":[{}],
            "rules":{{"min_clearance":100000,"min_track_width":100000,"min_via_diameter":250000,"min_via_drill":150000,
                     "min_annular":50000,"hole_clearance":100000,"hole_to_hole":200000,"edge_clearance":200000,
                     "default":{{"clearance":200000,"width":{},"via_d":600000,"via_drill":300000}},
                     "per_net":{{"1":{nr},"2":{nr}}},"diff_pairs":{dp}}}}}"#,
        pj.join(","),
        segs.join(","),
        p(w)
    );
    serde_json::from_str(&s).expect("model json")
}

/// The drawn criteria of the applied best row.
fn drawn(m: &BoardModel) -> serde_json::Value {
    drawn_with(m, "")
}

fn drawn_with(m: &BoardModel, extra: &str) -> serde_json::Value {
    let o = Options::from_json(&format!(r#"{{"runs": 1, "wall_clock_s": 60.0, "opt_loop": "topor", "topo_rounds": 2{extra}}}"#)).unwrap();
    let r = route(m, &o);
    eprintln!("{} vias {} bests {}", r.routed, r.vias, serde_json::to_string(&r.bests).unwrap());
    let b = r.bests.iter().find(|b| b["applied"] == true).expect("an applied best");
    b["drawn"].clone()
}

/// m6 + m9: drawn copper through a 0.6 mm gap necks (0.3 mm -> ~0.2 mm): the real criteria report it, in the
/// bests JSON (drawn necking in mm2 and the one mm-eq value).
#[test]
fn drawn_necked_copper_is_in_the_criteria_and_the_bests_json() {
    let d = drawn(&board(0.3, 0.3, false, true));
    assert!(d["neck_mm2"].as_f64().unwrap() > 0.1, "{d}");
    assert!(d["necking"].as_f64().unwrap() > 1.0, "necking mm-eq: {d}");
    assert!(d["necking"].as_f64().unwrap() >= 10.0 * d["neck_mm2"].as_f64().unwrap() - 0.05, "neck_mm_per_mm2 default 10: {d}");
}

/// m4: the pair's real via (0.45) is full size for its net: not necked against the planning disc (0.6, the net class).
#[test]
fn a_pair_via_is_not_a_necked_via() {
    let d = drawn(&board(0.2, 0.2, true, false));
    assert_eq!(d["vias"], 2, "the fixture needs the pair's vias: {d}");
    assert_eq!((d["neck_vias"].as_u64(), d["necking"].as_f64()), (Some(0), Some(0.0)), "{d}");
}

/// A pair drawn at its rule width (0.15, under the net width 0.2) is full width, not necked. (Mutant m5, the
/// pair width ignored in `criteria`, is equivalent: `diffpair::plan` sets every pair connection's width to the
/// rule width, diffpair.rs:116, so the fallback `dec.connections[ci].width` is the same value; with
/// `diffpair_route` off the pair nets are not routed at all, so no drawn item reaches it.)
#[test]
fn a_pair_drawn_at_its_rule_width_is_not_necked() {
    let d = drawn(&board(0.2, 0.15, true, false));
    assert_eq!((d["vias"].as_u64(), d["neck_mm2"].as_f64()), (Some(2), Some(0.0)), "{d}");
}
