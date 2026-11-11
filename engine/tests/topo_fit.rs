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
use topor_engine::topo::cutmodel::{bare_sheets, flow, sheet_room, CutOpts, CutPrice, Flow, ViaItem};
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

// ---------------------------------------------------------------- 2.3: the P2 room per layer

/// The projection's room per (edge, layer) after [`sheet_room`] (the `[capacity] model = "cuts"`
/// build), and the changed edge-layers.
type Rooms = (Vec<Vec<f64>>, Vec<(usize, usize, f64)>);

fn sheet_rooms(fx: &Fx) -> Rooms {
    let pr = &fx.b.proj;
    let sh = bare_sheets(pr, &outline_segments(&fx.m), &opts(Norm::Octagonal)).expect("sheets");
    let ch = sheet_room(pr, &sh);
    let mut room = pr.room.clone();
    for &(e, l, r) in &ch {
        assert!(pr.room[e][l].is_infinite(), "edge {e} layer {l}: a bounded room was changed");
        room[e][l] = r;
    }
    (room, ch)
}

fn pad_obs(fx: &Fx, r: &str, n: &str) -> usize {
    let pi = fx.m.pads.iter().position(|p| p.ref_ == r && p.number == n).unwrap_or_else(|| panic!("pad {r}.{n}"));
    fx.b.pad_of.iter().position(|&p| p == pi).expect("pad obstacle")
}

/// Every edge-layer that is no edge along one obstacle and has no pad centre end is bounded
/// (the board has an outline).
fn all_bounded(fx: &Fx, room: &[Vec<f64>]) {
    use topor_engine::topo::state::VertKind;
    let pr = &fx.b.proj;
    for (e, &[p, q]) in pr.edges.iter().enumerate() {
        let same = matches!((pr.vkind[p], pr.vkind[q]), (VertKind::Outline(a), VertKind::Outline(b)) if a == b);
        let centre = matches!(pr.vkind[p], VertKind::Centre(_)) || matches!(pr.vkind[q], VertKind::Centre(_));
        if same || centre {
            continue;
        }
        for (l, r) in room[e].iter().enumerate() {
            assert!(r.is_finite(), "edge {e} layer {l} unbounded");
        }
    }
}

#[test]
fn p2_room_row_end_is_the_pad_to_edge_gap_not_unbounded() {
    // channel_fit: the projection edges from the last U2 pad to the B.Cu keepout's corners (on the
    // board edge) were unbounded on F.Cu (the keepout is not on F.Cu); the F.Cu gap there is the
    // pad-to-edge 0.50 mm, which P2 must see (the S3 plan went round the row end). Since Q14a the
    // projection itself has that cut (the keepout's corners on the outline are the board edge, the
    // pad's foot on the edge is kept), so the gap is in the per-edge room before the sheet fill
    // (`[capacity] model = "cuts"`) as well as after it.
    for name in ["channel_fit", "model_gap_cut"] {
        let fx = load(name);
        let pr = &fx.b.proj;
        let (room, _) = sheet_rooms(&fx);
        all_bounded(&fx, &room);
        let f_cu = layer_idx(&fx, "F.Cu");
        let ch: Vec<(usize, usize, f64)> = (0..pr.edges.len()).map(|e| (e, f_cu, room[e][f_cu])).collect();
        // The rightmost pad of the right row.
        let last = (0..pr.obstacles.len()).filter(|&o| pr.obstacles[o].layers.has(f_cu) && !pr.obstacles[o].drilled && pr.obstacles[o].poly.len() == 4 && (pr.obstacles[o].center.y.abs() - 6.0 * MM).abs() < 3.0 * MM).max_by(|&a, &b| pr.obstacles[a].center.x.total_cmp(&pr.obstacles[b].center.x)).expect("row pad");
        let right = pr.obstacles[last].poly.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
        let mut gap = f64::INFINITY;
        for &(e, l, r) in ch.iter().filter(|x| x.1 == f_cu) {
            let [p, q] = pr.edges[e];
            let on = |v: usize| matches!(pr.vkind[v], topor_engine::topo::state::VertKind::Outline(o) if o == last);
            if (on(p) || on(q)) && pr.pos[p].x.max(pr.pos[q].x) > right + 1.0 {
                eprintln!("{name}: row-end edge {e} layer {l} room {:.4}", r / MM);
                gap = gap.min(r);
            }
        }
        assert!((gap / MM - 0.50).abs() < 1e-3, "{name}: row-end gap {:.4}", gap / MM);
    }
}

#[test]
fn p2_room_is_zero_inside_a_keepout_of_the_layer() {
    // channel_fit: B.Cu is closed over the rows by a rule area: an edge whose gap runs inside it
    // has no room on B.Cu (it had none on F.Cu's terms: unbounded).
    let fx = load("channel_fit");
    let pr = &fx.b.proj;
    let (room, ch) = sheet_rooms(&fx);
    let b_cu = layer_idx(&fx, "B.Cu");
    let keep: Vec<usize> = (0..pr.obstacles.len()).filter(|&o| pr.obstacles[o].net.is_none() && pr.obstacles[o].layers.has(b_cu) && !pr.obstacles[o].drilled).collect();
    assert!(!keep.is_empty());
    let mut n = 0;
    for &(e, l, _) in ch.iter().filter(|x| x.1 == b_cu) {
        let [p, q] = pr.edges[e];
        let mid = (pr.pos[p] + pr.pos[q]) * 0.5;
        if keep.iter().any(|&k| topor_engine::topo::state::point_in_poly(mid, &pr.obstacles[k].poly)) {
            assert_eq!(room[e][l], 0.0, "edge {e} inside the keepout");
            n += 1;
        }
    }
    assert!(n > 20, "{n} edges inside the keepout");
}

#[test]
fn p2_room_on_in1_is_pin_to_pin_not_the_tp1_sub_gaps() {
    // inner_layer_room: TP1 (F.Cu only) splits the pin gap into two 0.70 mm sub-gaps on F.Cu; on
    // In1 the edges that end at TP1 stand for the 1.80 mm pin-to-pin gap (I1).
    let fx = load("inner_layer_room");
    let pr = &fx.b.proj;
    let (room, _) = sheet_rooms(&fx);
    all_bounded(&fx, &room);
    let in1 = layer_idx(&fx, "In1.Cu");
    let f_cu = layer_idx(&fx, "F.Cu");
    let tp1 = pad_obs(&fx, "TP1", "1");
    let (j1, j2) = (pad_obs(&fx, "J1", "1"), pad_obs(&fx, "J1", "2"));
    let sag = pr.obstacles[j1].sag + pr.obstacles[j2].sag;
    let (mut n, mut f_min) = (0, f64::INFINITY);
    for (e, &[p, q]) in pr.edges.iter().enumerate() {
        let of = |v: usize| match pr.vkind[v] {
            topor_engine::topo::state::VertKind::Outline(o) => Some(o),
            _ => None,
        };
        let ends = [of(p), of(q)];
        if ends.contains(&Some(tp1)) && (ends.contains(&Some(j1)) || ends.contains(&Some(j2))) {
            n += 1;
            eprintln!("edge {e}: F.Cu {:.4} In1 {:.4}", room[e][f_cu] / MM, room[e][in1] / MM);
            f_min = f_min.min(room[e][f_cu]);
            assert!(room[e][in1] >= 1.80 * MM - 1.1 * sag - 60.0 && room[e][in1].is_finite(), "In1 {:.4}", room[e][in1] / MM);
        }
    }
    assert!(n >= 2, "{n} pin-TP1 edges");
    // F.Cu keeps the shared-edge reading of TP1's sub-gaps (both ends block there: unchanged;
    // the 0.70 critical cut itself is no projection edge, so the shortest edge is a little longer).
    assert!(f_min < 0.80 * MM, "F.Cu sub-gap {:.4}", f_min / MM);
}

// ---------------------------------------------------------------- 3.1: incremental edits

/// A small deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// One wire of the edit test: its witness polyline, the via points on it (as `(segment,
/// fraction)`, in order) and the layer of every piece between them.
#[derive(Clone)]
struct EditWire {
    poly: Vec<Vec2>,
    net: i32,
    w: f64,
    s: f64,
    splits: Vec<(usize, f64)>,
    layers: Vec<usize>,
    alive: bool,
}

impl EditWire {
    fn at(&self, (i, f): (usize, f64)) -> Vec2 {
        self.poly[i] + (self.poly[i + 1] - self.poly[i]) * f
    }
    /// Its runs (one per piece) and vias.
    fn runs(&self, wire: usize, via_r: f64) -> (Vec<Stretch>, Vec<ViaItem>) {
        let mut cuts: Vec<(usize, f64)> = vec![(0, 0.0)];
        cuts.extend(self.splits.iter().copied());
        cuts.push((self.poly.len() - 2, 1.0));
        let mut runs = Vec::new();
        for j in 0..cuts.len() - 1 {
            let (a, b) = (cuts[j], cuts[j + 1]);
            let mut curve = vec![self.at(a)];
            for k in a.0 + 1..=b.0 {
                curve.push(self.poly[k]);
            }
            curve.push(self.at(b));
            curve.dedup();
            let n = curve.len();
            runs.push(Stretch {
                wire,
                conn: None,
                net: self.net,
                layer: self.layers[j],
                s: self.s,
                w: self.w,
                w_floor: self.w,
                curve_k: (0..n).map(|i| a.0 + i).collect(),
                pass_w: vec![self.w; n.saturating_sub(2)],
                curve,
                nosite: false,
                via_ends: [j > 0, j + 1 < cuts.len() - 1],
                band: Vec::new(),
                wraps: Vec::new(),
            });
        }
        let vias = self.splits.iter().enumerate().map(|(i, &x)| ViaItem { wire, at: self.at(x), r: via_r, s: self.s, net: self.net, k: x.0 + i }).collect();
        (runs, vias)
    }
}

/// A cut with items: (layer, ends in nm, capacity, need, slack).
type CutKey = (usize, i64, i64, i64, i64, f64, f64, f64);

/// One item of a cut, bit for bit: (wire, traversal, kind, net, position, width, clearance,
/// floor).
type ItemBits = (usize, usize, u8, i32, u64, u64, u64, u64);

/// Every cut's items as (layer, cut, items in their order along the cut).
fn all_items(cp: &CutPrice, sh: &[LayerSheet]) -> Vec<(usize, usize, Vec<ItemBits>)> {
    let mut out = Vec::new();
    for (l, s) in sh.iter().enumerate() {
        for ci in 0..s.cuts.len() {
            let v: Vec<ItemBits> = cp.cut_items(l, ci).iter().map(|x| (x.wire, x.k, x.kind, x.it.net, x.t.to_bits(), x.it.w.to_bits(), x.it.s.to_bits(), x.floor.to_bits())).collect();
            if !v.is_empty() {
                out.push((l, ci, v));
            }
        }
    }
    out
}

/// The totals of a fresh model holding `ws` (the full recompute): total, over-full count and
/// every cut's (capacity, need, slack) by its ends.
#[allow(clippy::type_complexity)]
fn full_price(pr: &topor_engine::topo::state::Projection, sh: &std::rc::Rc<Vec<LayerSheet>>, o: CutOpts, ws: &[EditWire], via_r: f64) -> (f64, usize, Vec<CutKey>, Vec<(usize, usize, Vec<ItemBits>)>) {
    let mut cp = CutPrice::new(pr, sh.clone(), o, 1.0, ws.len());
    for (w, x) in ws.iter().enumerate().filter(|(_, x)| x.alive) {
        let (mut r, v) = x.runs(w, via_r);
        cp.set_wire(w, &mut r, &v);
    }
    let mut rows: Vec<_> = cp.report().into_iter().map(|r| (r.layer, r.p.x as i64, r.p.y as i64, r.q.x as i64, r.q.y as i64, r.cap, r.need, r.slack)).collect();
    rows.sort_by_key(|a| (a.0, a.1, a.2, a.3, a.4));
    (cp.total(), cp.overfull(), rows, all_items(&cp, sh))
}

#[test]
fn incremental_edits_price_as_a_full_recompute() {
    // Random edit sequences (insert, remove, relabel a piece, move / add / remove a via) on the
    // wires of inner_layer_room (four layers, a pin gap that over-fills easily): after every
    // edit the incremental model's total, over-full count and every cut's need and slack equal
    // a model built from scratch, and the returned change equals the change of the full totals.
    let fx = load("inner_layer_room");
    let pr = &fx.b.proj;
    let o = opts(Norm::Octagonal);
    let sh = std::rc::Rc::new(bare_sheets(pr, &outline_segments(&fx.m), &o).expect("sheets"));
    let layers = [layer_idx(&fx, "F.Cu"), layer_idx(&fx, "In1.Cu"), layer_idx(&fx, "B.Cu")];
    let via_r = 0.5 * fx.m.rules.default.via_d as f64;
    let base: Vec<EditWire> = fx.doc["wires"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| EditWire {
            poly: w["witness"].as_array().unwrap().iter().map(pt).collect(),
            net: net_code(&fx, w["net"].as_str().unwrap()),
            w: w["width"].as_f64().unwrap() * MM,
            s: w["clearance"].as_f64().unwrap() * MM,
            splits: Vec::new(),
            layers: vec![layer_idx(&fx, w["witness_layer"].as_str().unwrap())],
            alive: false,
        })
        .collect();
    let (mut checked, mut reused, mut computed, mut skipped) = (0, 0, 0, 0);
    for seed in 1..=6u64 {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ seed);
        let mut ws = base.clone();
        let mut cp = CutPrice::new(pr, sh.clone(), o, 1.0, ws.len());
        let mut prev = 0.0;
        let mut prev_rows: Vec<CutKey> = Vec::new();
        for step in 0..60 {
            let w = rng.below(ws.len());
            let mut x = ws[w].clone();
            let undo = rng.below(5) == 0;
            match if x.alive { rng.below(6) } else { 6 } {
                0 => x.alive = false,
                1 | 2 => {
                    let j = rng.below(x.layers.len());
                    x.layers[j] = layers[rng.below(layers.len())];
                }
                3 if !x.splits.is_empty() => {
                    // Via move: along its segment, or to the next one.
                    let j = rng.below(x.splits.len());
                    let (i, _) = x.splits[j];
                    let lo = if j > 0 { x.splits[j - 1] } else { (0, 0.0) };
                    let hi = if j + 1 < x.splits.len() { x.splits[j + 1] } else { (x.poly.len() - 2, 1.0) };
                    let ni = (i + rng.below(3)).saturating_sub(1).clamp(lo.0, hi.0);
                    let nf = 0.15 + 0.7 * rng.unit();
                    if (ni, nf) > lo && (ni, nf) < hi {
                        x.splits[j] = (ni, nf);
                    }
                }
                4 => {
                    // A new via (and piece) on a free segment.
                    let i = rng.below(x.poly.len() - 1);
                    let f = 0.15 + 0.7 * rng.unit();
                    let at = x.splits.partition_point(|&s| s < (i, f));
                    x.splits.insert(at, (i, f));
                    x.layers.insert(at + 1, layers[rng.below(layers.len())]);
                }
                5 if !x.splits.is_empty() => {
                    let j = rng.below(x.splits.len());
                    x.splits.remove(j);
                    x.layers.remove(j + 1);
                }
                6 => x.alive = true,
                _ => {}
            }
            if undo {
                cp.begin();
            }
            let d = if x.alive {
                let (mut r, v) = x.runs(w, via_r);
                cp.set_wire(w, &mut r, &v)
            } else {
                cp.clear_wire(w)
            };
            let mut after = ws.clone();
            after[w] = x;
            let (t_full, over_full, rows, items_full) = full_price(pr, &sh, o, &after, via_r);
            let tol = 1e-6 * t_full.abs().max(1.0);
            assert!((cp.total() - t_full).abs() <= tol, "seed {seed} step {step}: total {} vs full {}", cp.total(), t_full);
            assert!((d - (t_full - prev)).abs() <= tol, "seed {seed} step {step}: change {d} vs {}", t_full - prev);
            assert_eq!(cp.overfull(), over_full, "seed {seed} step {step}");
            let mut mine: Vec<_> = cp.report().into_iter().map(|r| (r.layer, r.p.x as i64, r.p.y as i64, r.q.x as i64, r.q.y as i64, r.cap, r.need, r.slack)).collect();
                    mine.sort_by_key(|a| (a.0, a.1, a.2, a.3, a.4));
            assert_eq!(mine.len(), rows.len(), "seed {seed} step {step}: cuts with items");
            for (a, b) in mine.iter().zip(&rows) {
                assert_eq!((a.0, a.1, a.2, a.3, a.4), (b.0, b.1, b.2, b.3, b.4));
                assert!((a.6 - b.6).abs() < 1e-6 && (a.7 - b.7).abs() < 1e-6, "seed {seed} step {step}: cut need/slack {a:?} vs {b:?}");
            }
            // The items themselves (wire, traversal) per cut, and the strict worse test of the
            // move against the slacks before it.
            assert!(all_items(&cp, &sh) == items_full, "seed {seed} step {step}: cut items differ");
            if undo {
                let before: std::collections::HashMap<(usize, i64, i64, i64, i64), f64> = prev_rows.iter().map(|r| ((r.0, r.1, r.2, r.3, r.4), r.7)).collect();
                let worse = rows.iter().any(|r| r.7 < -1.0 && r.7 < before.get(&(r.0, r.1, r.2, r.3, r.4)).copied().unwrap_or(f64::INFINITY) - 1.0);
                assert_eq!(cp.worse_somewhere(), worse, "seed {seed} step {step}: worse_somewhere");
            }
            checked += 1;
            if undo {
                cp.rollback();
                let (t0, o0, _, it0) = full_price(pr, &sh, o, &ws, via_r);
                assert!((cp.total() - t0).abs() <= 1e-6 * t0.abs().max(1.0) && cp.overfull() == o0, "seed {seed} step {step}: rollback");
                assert!(all_items(&cp, &sh) == it0, "seed {seed} step {step}: rollback items");
                prev = t0;
            } else {
                ws = after;
                prev = t_full;
                prev_rows = rows;
            }
        }
        reused += cp.parts.1;
        computed += cp.parts.0;
        skipped += cp.skipped;
    }
    eprintln!("incremental edits: {checked} edits checked; parts computed {computed}, reused {reused}; cut pricings skipped {skipped}");
    // Via moves re-pull bands whose crossings mostly keep their place: the shortcut is taken.
    assert!(skipped > 0, "no cut pricing was skipped");
    // Inserts compute every piece; relabels and via moves keep the pieces they do not touch.
    assert!(reused * 4 > computed, "the edits reused almost nothing: {reused} of {}", reused + computed);
}
