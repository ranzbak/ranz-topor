//! P1a / P1b standalone tests (shopping list 2.2 / 2.3): the cut model (`topo::cutmodel::flow`) on
//! the seven user-approved fit patterns (`scratch/fit-patterns`, exported by
//! `harness/fit_fixtures.py` to `tests/fixtures/fit/`), with no router run. Each pattern's wires
//! are fed as their hand-made witness polylines on their layer; the predicted capacity, demand and
//! slack of every named cut must equal the hand numbers of `expected.json`.
//!
//! Tolerance: the need is exact (1 nm: it is a sum of widths and clearances). The capacity is
//! the length of the sheet's cut between the end features as the engine polygonises them: exact
//! (1 nm) between polygon pads; a round pad is a circumscribed polygon (grown by its sagitta,
//! `sheet.rs::grow`) whose vertices stand up to about one sagitta outside the circle, so a cut
//! ending on round copper may be up to 1.1 x (sum of the two ends' sagittas) + 1 nm shorter
//! (16 segments per turn: 16 um on a 1.7 mm THT pad).
//!
//! Unsolvable patterns: exactly the named cut (its two end features) is over-full by the stated
//! amount, nothing else is.

use serde_json::Value;
use topor_engine::geom::Vec2;
use topor_engine::model::{BoardModel, Options};
use topor_engine::topo::board::{build_full, outline_segments, Board, BuildOpts};
use topor_engine::topo::cutflow::Stretch;
use topor_engine::topo::cutmodel::{flow, CutOpts, Flow, ViaItem};
use topor_engine::topo::sheet::{FeatKind, LayerSheet, Norm, SheetInput};

const MM: f64 = 1e6;

struct Fx {
    doc: Value,
    m: BoardModel,
    b: Board,
}

fn load(name: &str) -> Fx {
    let path = format!("{}/tests/fixtures/fit/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("json");
    let m = BoardModel::from_json(&doc["model"].to_string()).expect("model");
    // The router's build (topo::core::build_opts with plane signals allowed).
    let o = BuildOpts { existing_vias: true, keepouts: true, plane_layers: true, copper_shapes: true, ..BuildOpts::default() };
    let b = build_full(&m, &o, &[], &[]).expect("board");
    Fx { doc, m, b }
}

/// KiCad mm (y down) to engine nm (y up).
fn pt(v: &Value) -> Vec2 {
    Vec2 { x: v[0].as_f64().unwrap() * MM, y: -v[1].as_f64().unwrap() * MM }
}

fn layer_idx(fx: &Fx, name: &str) -> usize {
    let id = fx.m.layers.iter().find(|l| l.name == name).expect("layer").id;
    fx.b.signal_layers.iter().position(|&x| x == id).expect("projection layer")
}

fn net_code(fx: &Fx, name: &str) -> i32 {
    fx.m.nets.iter().find(|n| n.name == name).expect("net").code
}

fn opts(norm: Norm) -> CutOpts {
    let mut o = CutOpts::from_options(&Options::default());
    o.sheet.norm = norm;
    o
}

fn sheet(fx: &Fx, l: usize, o: &CutOpts) -> LayerSheet {
    let pr = &fx.b.proj;
    let terms: Vec<Vec2> = pr.obstacles.iter().filter(|ob| ob.net.is_some()).map(|ob| ob.center).collect();
    let outline = outline_segments(&fx.m);
    LayerSheet::build(&SheetInput { proj: pr, outline: &outline, vias: &[], junctions: &[], terminals: &terms }, l, &o.sheet).expect("sheet")
}

/// The pattern's wires as runs on layer `l` (their witness polylines), and a via at each end of
/// a witness that lies off its terminal pads (the inner-layer wires of `inner_layer_room`).
fn runs(fx: &Fx, l: usize, floors: &dyn Fn(&str) -> f64) -> (Vec<Stretch>, Vec<ViaItem>) {
    let mut out = Vec::new();
    let mut vias = Vec::new();
    let pr = &fx.b.proj;
    for (i, w) in fx.doc["wires"].as_array().unwrap().iter().enumerate() {
        if layer_idx(fx, w["witness_layer"].as_str().unwrap()) != l {
            continue;
        }
        let net = net_code(fx, w["net"].as_str().unwrap());
        let (wd, s) = (w["width"].as_f64().unwrap() * MM, w["clearance"].as_f64().unwrap() * MM);
        let curve: Vec<Vec2> = w["witness"].as_array().unwrap().iter().map(pt).collect();
        let on_pad = |p: Vec2| pr.obstacles.iter().any(|o| o.net == Some(net) && topor_engine::topo::state::point_in_poly(p, &o.poly));
        let ends = [!on_pad(curve[0]), !on_pad(*curve.last().unwrap())];
        let via_r = 0.5 * fx.m.rules.default.via_d as f64;
        for (e, p) in [(ends[0], curve[0]), (ends[1], *curve.last().unwrap())] {
            if e {
                vias.push(ViaItem { wire: i, at: p, r: via_r, s, net, k: 0 });
            }
        }
        let n = curve.len();
        out.push(Stretch {
            wire: i,
            conn: None,
            net,
            layer: l,
            s,
            w: wd,
            w_floor: floors(w["net"].as_str().unwrap()) * MM,
            curve,
            curve_k: vec![0; n],
            pass_w: vec![wd; n.saturating_sub(2)],
            nosite: false,
            via_ends: ends,
            band: Vec::new(),
            wraps: Vec::new(),
        });
    }
    (out, vias)
}

fn poly_dist(p: Vec2, poly: &[Vec2]) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            let d = b - a;
            let t = ((p - a).dot(d) / d.len2().max(1e-9)).clamp(0.0, 1.0);
            (p - (a + d * t)).len()
        })
        .fold(f64::INFINITY, f64::min)
}

/// The feature of sheet `sh` whose copper `p` lies on (within 50 um of its polygon, which is
/// grown by its sagitta).
fn feature_at(sh: &LayerSheet, p: Vec2) -> usize {
    let mut best = (f64::INFINITY, usize::MAX);
    for (i, f) in sh.feats.iter().enumerate() {
        if f.poly.len() < 3 {
            continue;
        }
        let d = if topor_engine::topo::state::point_in_poly(p, &f.poly) { 0.0 } else { poly_dist(p, &f.poly) };
        if d < best.0 {
            best = (d, i);
        }
    }
    assert!(best.0 < 0.05 * MM, "no feature at ({:.3},{:.3})", p.x / MM, -p.y / MM);
    best.1
}

/// The critical cut between the two features the expected cut `p q` joins: the shortest sheet
/// cut with those end features (Maley p. 40: endpoint to closest point; in the octagonal norm
/// the closest point of a side is often a whole stretch, so its end points need not be `p q`).
fn find_cut(sh: &LayerSheet, p: Vec2, q: Vec2) -> usize {
    let (fp, fq) = (feature_at(sh, p), feature_at(sh, q));
    let mut best = (f64::INFINITY, usize::MAX);
    for (i, c) in sh.cuts.iter().enumerate() {
        if ((c.fp == fp && c.fq == fq) || (c.fp == fq && c.fq == fp)) && c.cap < best.0 - 1.0 {
            best = (c.cap, i);
        }
    }
    assert!(best.1 != usize::MAX, "no sheet cut between the features at ({:.3},{:.3}) and ({:.3},{:.3})", p.x / MM, -p.y / MM, q.x / MM, -q.y / MM);
    best.1
}

fn sag_of(fx: &Fx, sh: &LayerSheet, f: usize) -> f64 {
    match sh.feats[f].kind {
        FeatKind::Obstacle(o) => fx.b.proj.obstacles[o].sag,
        _ => 0.0,
    }
}

/// One checked cut: (id, predicted cap, need, slack, expected length, demand, slack) in mm.
type Row = (String, f64, f64, f64, f64, f64, f64);

/// The checked layers: (layer, its sheet, its flows).
type Layers = Vec<(usize, LayerSheet, Vec<Flow>)>;

/// Check every expected cut on its layer; returns the flows per layer index and the rows.
fn check(name: &str, norm: Norm, floors: &dyn Fn(&str) -> f64) -> (Fx, Layers, Vec<Row>) {
    let fx = load(name);
    let o = opts(norm);
    let pr = &fx.b.proj;
    let mut layers: Vec<usize> = fx.doc["cuts"].as_array().unwrap().iter().map(|c| layer_idx(&fx, c["layer"].as_str().unwrap())).collect();
    layers.sort_unstable();
    layers.dedup();
    let mut per = Vec::new();
    let mut rows = Vec::new();
    for &l in &layers {
        let sh = sheet(&fx, l, &o);
        let (mut rs, vias) = runs(&fx, l, floors);
        let fl = flow(&sh, &mut rs, &vias, &|p| pr.regime_at(p), &o).expect("flow");
        for c in fx.doc["cuts"].as_array().unwrap().iter().filter(|c| layer_idx(&fx, c["layer"].as_str().unwrap()) == l) {
            let id = c["id"].as_str().unwrap().to_string();
            let ci = find_cut(&sh, pt(&c["p"]), pt(&c["q"]));
            let cut = &sh.cuts[ci];
            let len = c[if norm == Norm::Octagonal { "length_octagonal" } else { "length_euclid" }].as_f64().unwrap();
            let demand = c["demand"].as_f64().unwrap();
            // 60 nm: expected.json is rounded to 4 decimals (mm).
            let tol = 60.0 + 1.1 * (sag_of(&fx, &sh, cut.fp) + sag_of(&fx, &sh, cut.fq));
            let crossing: Vec<i32> = c["crossing"].as_array().unwrap().iter().map(|x| net_code(&fx, x["net"].as_str().unwrap())).collect();
            match fl.iter().find(|f| f.cut == ci) {
                None => {
                    assert!(crossing.is_empty(), "{name} {id}: no flow, expected {crossing:?}");
                    assert!(cut.cap <= len * MM + 60.0 && cut.cap >= len * MM - tol, "{name} {id}: capacity {} vs {len}", cut.cap / MM);
                }
                Some(f) => {
                    let mut nets: Vec<i32> = f.items.iter().map(|x| x.it.net).collect();
                    nets.sort_unstable();
                    let mut want = crossing.clone();
                    want.sort_unstable();
                    assert_eq!(nets, want, "{name} {id}: items");
                    assert!(f.cap <= len * MM + 60.0 && f.cap >= len * MM - tol, "{name} {id}: capacity {:.6} vs {len} (tol {:.6})", f.cap / MM, tol / MM);
                    assert!((f.need - demand * MM).abs() <= 60.0, "{name} {id}: need {:.6} vs {demand}", f.need / MM);
                    let slack = len - demand;
                    assert!(f.slack <= slack * MM + 60.0 && f.slack >= slack * MM - tol, "{name} {id}: slack {:.6} vs {slack}", f.slack / MM);
                    rows.push((id, f.cap / MM, f.need / MM, f.slack / MM, len, demand, slack));
                }
            }
        }
        per.push((l, sh, fl));
    }
    for r in &rows {
        eprintln!("{name} {:?}: {} predicted cap {:.4} need {:.4} slack {:+.4} | expected {:.4} {:.4} {:+.4}", norm, r.0, r.1, r.2, r.3, r.4, r.5, r.6);
    }
    (fx, per, rows)
}

/// The over-full flows (slack below -1 nm) of every checked layer: (layer, end features, slack).
fn overfull(per: &[(usize, LayerSheet, Vec<Flow>)]) -> Vec<(usize, (usize, usize), f64)> {
    let mut v = Vec::new();
    for (l, sh, fl) in per {
        for f in fl.iter().filter(|f| f.slack < -1.0) {
            let c = &sh.cuts[f.cut];
            eprintln!(
                "  over-full layer {l}: ({:.4},{:.4})-({:.4},{:.4}) feats {}-{} cap {:.4} need {:.4} slack {:.4} floor-need {:.4} items {:?}",
                c.p.x / MM, -c.p.y / MM, c.q.x / MM, -c.q.y / MM, c.fp, c.fq, f.cap / MM, f.need / MM, f.slack / MM, f.need_floor / MM,
                f.items.iter().map(|x| (x.wire, x.kind, (x.t * 1000.0).round() / 1000.0, x.it.w / MM)).collect::<Vec<_>>()
            );
            v.push((*l, (c.fp.min(c.fq), c.fp.max(c.fq)), f.slack));
        }
    }
    v
}

/// Unsolvable: every over-full cut joins the named cut's two features, by its stated overflow.
fn only_named_overfull(name: &str, norm: Norm, cut_id: &str, deficit_mm: f64, tol_mm: f64) {
    let (fx, per, _) = check(name, norm, &|_| 0.0);
    let c = fx.doc["cuts"].as_array().unwrap().iter().find(|c| c["id"] == cut_id).unwrap();
    let l = layer_idx(&fx, c["layer"].as_str().unwrap());
    let (_, sh, _) = per.iter().find(|x| x.0 == l).unwrap();
    let ci = find_cut(sh, pt(&c["p"]), pt(&c["q"]));
    let pair = (sh.cuts[ci].fp.min(sh.cuts[ci].fq), sh.cuts[ci].fp.max(sh.cuts[ci].fq));
    let over = overfull(&per);
    assert!(!over.is_empty(), "{name}: nothing over-full");
    for (ol, op, s) in &over {
        assert_eq!((*ol, *op), (l, pair), "{name}: another cut over-full (layer {ol}, slack {:.4})", s / MM);
        assert!((s / MM + deficit_mm).abs() <= tol_mm, "{name}: over-full by {:.4}, expected {deficit_mm}", -s / MM);
    }
}

fn nothing_overfull(name: &str, norm: Norm, floors: &dyn Fn(&str) -> f64) {
    let (_, per, _) = check(name, norm, floors);
    let over = overfull(&per);
    assert!(over.is_empty(), "{name}: over-full {:?}", over.iter().map(|x| (x.0, x.2 / MM)).collect::<Vec<_>>());
}

#[test]
fn channel_fit_holds_three_wires_with_slack() {
    nothing_overfull("channel_fit", Norm::Octagonal, &|_| 0.25);
    nothing_overfull("channel_fit", Norm::Euclid, &|_| 0.25);
}

#[test]
fn channel_overfull_is_over_full_at_c1_by_040() {
    only_named_overfull("channel_overfull", Norm::Octagonal, "C1", 0.40, 1e-6);
    only_named_overfull("channel_overfull", Norm::Euclid, "C1", 0.40, 1e-6);
}

/// Neck patterns: N1 (the critical cut between the two header pins) is over-full by 0.36 at the
/// nominal 0.8 mm, every over-full cut joins the two pins, and N1's slack at the neck floor is
/// `floor_slack`.
fn neck(name: &str, floor: f64, floor_slack: f64) {
    let (fx, per, rows) = check(name, Norm::Octagonal, &|_| floor);
    let n1 = rows.iter().find(|r| r.0 == "N1").expect("N1 checked");
    assert!(n1.3 < -0.36 + 1e-6 && n1.3 > -0.40, "{n1:?}");
    let over = overfull(&per);
    assert!(over.iter().all(|x| x.1 == over[0].1), "a cut between other features is over-full: {over:?}");
    let c = fx.doc["cuts"].as_array().unwrap().iter().find(|c| c["id"] == "N1").unwrap();
    let (_, sh, fl) = &per[0];
    let ci = find_cut(sh, pt(&c["p"]), pt(&c["q"]));
    let f = fl.iter().find(|f| f.cut == ci).expect("N1 flow");
    let tol = 60.0 + 1.1 * (sag_of(&fx, sh, sh.cuts[ci].fp) + sag_of(&fx, sh, sh.cuts[ci].fq));
    let s_floor = f.cap - f.need_floor;
    eprintln!("{name}: N1 at the {floor} floor: need {:.4} slack {:+.4} (expected {floor_slack:+.4})", f.need_floor / MM, s_floor / MM);
    assert!(s_floor <= floor_slack * MM + 60.0 && s_floor >= floor_slack * MM - tol, "floor slack {:.4}", s_floor / MM);
}

#[test]
fn neck_fit_fits_at_the_floor_only() {
    neck("neck_fit", 0.40, 0.04);
}

#[test]
fn neck_too_narrow_stays_over_full_at_the_floor() {
    neck("neck_too_narrow", 0.50, -0.06);
}

#[test]
fn model_gap_cut_is_over_full_at_ac_only() {
    // AC is over-full by 0.10 in both norms; AB, AD, BC, DC hold four wires each with room.
    only_named_overfull("model_gap_cut", Norm::Octagonal, "AC", 0.10, 1e-6);
    only_named_overfull("model_gap_cut", Norm::Euclid, "AC", 0.10, 1e-6);
}

#[test]
fn diagonal_cut_fits_in_euclid_and_overflows_in_the_octagonal_norm() {
    nothing_overfull("diagonal_cut_norm", Norm::Euclid, &|_| 0.25);
    only_named_overfull("diagonal_cut_norm", Norm::Octagonal, "C6", 0.05, 1e-4);
}

#[test]
fn inner_layer_holds_three_wires_drill_to_drill() {
    let (fx, per, rows) = check("inner_layer_room", Norm::Octagonal, &|_| 0.25);
    let i1 = rows.iter().find(|r| r.0 == "I1").expect("I1 checked");
    // Three wires on In1 with slack (the shared-edge reading, TP1's 0.70 sub-gaps, holds two).
    assert!(i1.3 > 0.2 && i1.2 > 1.5, "{i1:?}");
    // ... and not four (need 4 x 0.25 + 5 x 0.20 = 2.00 > the capacity).
    assert!(i1.1 >= 1.55 && i1.1 < 2.0, "{i1:?}");
    // TP1 (F.Cu only) is no feature of the In1 sheet: no In1 cut ends on it.
    let l_in1 = layer_idx(&fx, "In1.Cu");
    let (_, sh, _) = per.iter().find(|x| x.0 == l_in1).unwrap();
    let tp1 = fx.m.pads.iter().position(|p| p.ref_ == "TP1").expect("TP1");
    let tp1_obs = fx.b.pad_of.iter().position(|&p| p == tp1).expect("TP1 obstacle");
    assert!(sh.feats.iter().all(|f| f.kind != FeatKind::Obstacle(tp1_obs)), "TP1 is a feature of In1");
    assert!(overfull(&per).is_empty(), "{:?}", overfull(&per));
}
