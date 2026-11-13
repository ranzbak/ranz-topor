//! Stage M1 = 6.3 "compass": what the plan handed to the drawing predicts, per violation class,
//! as structured fields of the result (`RouteResult::compass`), so that every run can show
//! predicted vs drawn vs KiCad DRC side by side (`toporlib/compass.py`, the run report,
//! `harness/predict_vs_drawn.py`).
//!
//! TopoR shows the predicted criteria of every variant in its variant table (TopoR 6.3 manual
//! p. 72: Length "of unsmoothed connections (usually exceeds the length of smoothed-out
//! connections by 10 to 15 percent)", Vias, Violation, Shrinkages; p. 73: these four are the
//! optimisation criteria) and judges the copper by DRC after routing (p. 183: "The autorouter in
//! TopoR routes all connections, even where this causes constraint violations"); the classes are the ones of
//! `docs/dev/inflated-sketch.md` §6.1 / §6.3. A drawn violation in a class the plan predicts is a
//! fault of that built step (or the drawing's realisation of it); a drawn count in a class the
//! plan cannot predict yet (`pad_exit` before 5.10) is a missing method step.
//!
//! Measurement only: reads the State, the layering and the capacity models the round has built
//! anyway; never changes the plan or the drawing. O(edges x layers + cuts + traversals x
//! obstacles per triangle).

use super::optimise::layers_now;
use super::refine::CapModel;
use super::state::{State, WireId};
use crate::geom::Vec2;
use crate::route::kicad;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// The compass classes in table order (`docs/dev/inflated-sketch.md` §6.1, M1 brief). The first
/// seven are predicted by the plan. `model_gap` (§6.1: the per-edge packer said fits while the
/// cut test says over-full) and `edge_overpredict` (the reverse: per-edge over-full, cut test
/// fits) compare the two capacity models on the plan; `drawing_loss` is drawn-only (the plan
/// never predicts its own drawing's failure), `open` is a connection without copper.
pub const CLASSES: [&str; 11] = ["planner_overfull", "shrinkage", "via_site", "unlayerable", "foreign_copper", "forced_path", "pad_exit", "model_gap", "edge_overpredict", "drawing_loss", "open"];

/// A located predicted item of the plan handed to the drawing (KiCad frame, nm).
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct CompassItem {
    /// One of [`CLASSES`], or `pad_exit_stub` (5.10: a planned exit stub, located, not counted).
    pub class: String,
    /// Copper layer names the item lies on; empty = any layer (a via, a crossing point).
    pub layers: Vec<String>,
    /// Connection ids ([`crate::route::ConnReport::id`]) of the wires in it.
    pub conns: Vec<usize>,
    /// Its place: segments (a cut, an edge, a traversal chord) or points (`[p, p]`).
    pub segs: Vec<[[i64; 2]; 2]>,
    /// How far over (nm, > 0 = over-full; a cut's `-slack`, a shrinkage's width reduction); 0
    /// when not measured. A bottleneck of several members: the largest.
    pub over_nm: i64,
    /// Cuts / edge-layers merged into this item: one physical bottleneck ([`bottlenecks`]).
    pub members: usize,
}

/// What the plan hands the drawing for one connection.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct CompassConn {
    pub conn: usize,
    /// Planned vias (the via intervals of its labels, as the drawing gets them).
    pub vias: usize,
    /// Planned length: the clearance-keeping shortest path through its sleeve (nm).
    pub taut_nm: i64,
    /// Planned necked stretches (the archive's shrinkages of this wire).
    pub necks: usize,
}

/// The compass of one drawn round (see the module doc).
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Compass {
    /// The plan's capacity authority: `cuts` (per-layer cut test) or `edges` (per-edge packer).
    pub model: String,
    /// `[optimize] loop` of the run.
    pub opt_loop: String,
    /// The cycle (round) whose plan this is.
    pub round: usize,
    /// Predicted items per class of the plan handed to the drawing, counted as the plan counts
    /// them (cuts, edge-layers, vias, crossings, traversals). A class missing from the map is not
    /// predicted by this build (`pad_exit` until 5.10 hooks in; `model_gap` without a cut test).
    pub predicted: BTreeMap<String, usize>,
    /// The same, counted as physical bottlenecks ([`bottlenecks`]): the twin cuts at both ends of
    /// one channel are one bottleneck.
    pub bottlenecks: BTreeMap<String, usize>,
    /// Sum of the violation classes of `predicted` (planner_overfull + via_site + unlayerable +
    /// foreign_copper + forced_path + pad_exit when predicted): the plan's own predicted violations
    /// ([`super::optimise::Predicted::count`] of this plan).
    pub predicted_violations: usize,
    /// The TopoR loop's predicted violations of its pick (archive criteria), when the loop ran:
    /// equal to `predicted_violations` unless something between the pick and the drawing
    /// changed the plan.
    pub loop_predicted: Option<usize>,
    /// Drawn shrinkages (necked stretches of the written copper; the variant criterion).
    pub drawn_shrinkages: usize,
    pub items: Vec<CompassItem>,
    pub conns: Vec<CompassConn>,
    /// Classes this build cannot measure, and why (plain text for the report).
    pub notes: Vec<String>,
}

/// The inputs: the plan as handed to the drawing.
pub struct PlanView<'a, 'p, 'c> {
    pub st: &'a State<'p>,
    pub lr: &'a super::layers::LayerReport,
    /// The per-edge capacity model of the plan (the drawing's packer and via positions).
    pub cm_edge: &'a CapModel<'c>,
    /// The cut model: the plan's (`[capacity] model = "cuts"`, `cut_is_plan`) or the read-only cut
    /// test of the compass (`[capacity] compass_cut_check`); `None` = no cut test.
    pub cm_cut: Option<&'a CapModel<'c>>,
    /// `cm_cut` is the plan's capacity authority (else a read-only check).
    pub cut_is_plan: bool,
    pub bottom: i32,
    /// Wire -> connection id (`None`: a wire of no connection).
    pub conn: &'a dyn Fn(WireId) -> Option<usize>,
    /// Signal layer index -> copper layer name.
    pub layer: &'a dyn Fn(usize) -> String,
    /// Connections placed without a legal topological path (`initial = "once"`), by id.
    pub forced: &'a BTreeSet<usize>,
    /// Shopping list 5.10 slot: the plan's pad-exit objects as items, built by the caller from
    /// the board (`Board::exit_stubs` on `padexit-510`). `None` = this build has no pad-exit plan
    /// object (main): the class is "not predicted". Item classes: `pad_exit_stub` = a planned
    /// exit stub of the connection (located, not counted: a drawn pad_exit violation next to it
    /// is a fault of the drawing's realisation of the stub); `pad_exit` = a planned exit the plan
    /// itself predicts as violating (counted in `predicted["pad_exit"]`).
    pub pad_exits: Option<&'a [CompassItem]>,
}

fn seg(a: Vec2, b: Vec2) -> [[i64; 2]; 2] {
    [kicad(a), kicad(b)]
}

fn conns_of(v: &PlanView, ws: impl IntoIterator<Item = WireId>) -> Vec<usize> {
    let mut c: Vec<usize> = ws.into_iter().filter_map(|w| (v.conn)(w)).collect();
    c.sort_unstable();
    c.dedup();
    c
}

/// Per-edge over-full edge-layers of `cm` as items of `class`. O(over-full x passes).
fn edge_items(v: &PlanView, cm: &CapModel, class: &str) -> Vec<CompassItem> {
    let pr = v.st.proj;
    cm.overfull_list()
        .into_iter()
        .map(|(e, l)| {
            let ws = v.st.edge_order[e].iter().filter(|&&p| cm.pass_layer.get(p).is_some_and(|&pl| pl == l as i32)).map(|&p| v.st.passes[p].wire);
            let [a, b] = pr.edges[e];
            CompassItem { class: class.into(), layers: vec![(v.layer)(l)], conns: conns_of(v, ws), segs: vec![seg(pr.pos[a], pr.pos[b])], over_nm: 0, members: 1 }
        })
        .collect()
}

/// Over-full cuts (`over` = true) or cuts that fit only necked (false) of the cut model. O(cuts).
fn cut_items(v: &PlanView, cm: &CapModel, over: bool, class: &str) -> Vec<CompassItem> {
    let Some(c) = cm.cut.as_ref() else { return Vec::new() };
    let tol = super::cutmodel::OVER_TOL;
    c.report()
        .into_iter()
        .filter(|r| if over { r.slack < -tol } else { r.slack >= -tol && r.shrink > tol })
        .map(|r| CompassItem {
            class: class.into(),
            layers: vec![(v.layer)(r.layer)],
            conns: conns_of(v, r.wires.iter().copied()),
            segs: vec![seg(r.p, r.q)],
            over_nm: (if over { -r.slack } else { r.shrink }).round() as i64,
            members: 1,
        })
        .collect()
}

/// The plan's traversals through foreign copper, located: one item per (wire, object) with the
/// chords of the wire inside it, in the order and with the count of
/// [`super::optimise::foreign_copper`] (same test, same loops). O(traversals x obstacles per
/// triangle).
pub fn foreign_items(v: &PlanView) -> Vec<CompassItem> {
    let st = v.st;
    let pr = st.proj;
    let layers = layers_now(st, &v.lr.events, v.bottom);
    let all = if pr.n_layers >= 64 { u64::MAX } else { (1u64 << pr.n_layers) - 1 };
    let mut out = Vec::new();
    for (w, wi) in st.wires.iter().enumerate() {
        if !wi.alive || wi.tris.is_empty() {
            continue;
        }
        let Some(lw) = layers.get(w).filter(|l| l.len() == wi.tris.len()) else { continue };
        let n = wi.passes.len();
        // Per object: the layer bits it blocks for this wire and the chords inside it.
        let mut inside: Vec<(usize, u64, Vec<(Vec2, Vec2)>)> = Vec::new();
        for (k, &t) in wi.tris.iter().enumerate() {
            let on = lw[k] & all;
            if on == 0 {
                continue;
            }
            let block = |o: usize| {
                let ob = &pr.obstacles[o];
                if ob.drilled { all } else { ob.layers.0 & all }
            };
            let foreign: Vec<usize> = pr.tri_obs[t].iter().copied().filter(|&o| pr.obstacles[o].net != Some(wi.net)).collect();
            let blocked = foreign.iter().fold(0u64, |m, &o| m | block(o));
            if on & !blocked != 0 {
                continue;
            }
            let from = if k == 0 { pr.pos[wi.a] } else { st.pass_point(wi.passes[k - 1]) };
            let to = if k == n { pr.pos[wi.b] } else { st.pass_point(wi.passes[k]) };
            for o in foreign.into_iter().filter(|&o| block(o) & on != 0) {
                match inside.iter_mut().find(|x| x.0 == o) {
                    Some(x) => {
                        x.1 |= block(o) & on;
                        x.2.push((from, to));
                    }
                    None => inside.push((o, block(o) & on, vec![(from, to)])),
                }
            }
        }
        for (_, bits, chords) in inside {
            let ls = (0..pr.n_layers.min(64)).filter(|&l| bits & (1u64 << l) != 0).map(|l| (v.layer)(l)).collect();
            out.push(CompassItem { class: "foreign_copper".into(), layers: ls, conns: conns_of(v, [w]), segs: chords.into_iter().map(|(a, b)| seg(a, b)).collect(), over_nm: 0, members: 1 });
        }
    }
    out
}

/// Shopping list 5.10 hook: the plan's pad-exit items ([`PlanView::pad_exits`]); `None` on a
/// build without a pad-exit plan object (main). What 5.10 (`padexit-510`) must add when it
/// merges: in `core.rs` at the compass hand-off, with `[search] pad_exit_stubs` on, one
/// `pad_exit_stub` item per `Board::exit_stubs` entry (layers = the pad's layer, conns = the
/// connections ending at that pad, segs = the stub rectangle's sides) and one `pad_exit` item
/// per exit request that needed a stub (another net's copper closer than w + 2 x clearance) but
/// got none because the rectangle would break a clearance (today `exit_stub` returns `None`
/// for both "not needed" and "not legal": it must tell them apart).
pub fn pad_exit_items(v: &PlanView) -> Option<Vec<CompassItem>> {
    v.pad_exits.map(|x| x.to_vec())
}

/// Distance between segments `a` and `b` (nm; 0 when they cross or touch). O(1).
fn seg_dist(a: &[[i64; 2]; 2], b: &[[i64; 2]; 2]) -> f64 {
    let f = |p: [i64; 2]| Vec2 { x: p[0] as f64, y: p[1] as f64 };
    let (p, q, r, s) = (f(a[0]), f(a[1]), f(b[0]), f(b[1]));
    let cr = |o: Vec2, u: Vec2, w: Vec2| (u.x - o.x) * (w.y - o.y) - (u.y - o.y) * (w.x - o.x);
    let (d1, d2, d3, d4) = (cr(p, q, r), cr(p, q, s), cr(r, s, p), cr(r, s, q));
    if ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0)) && d1 != 0.0 && d2 != 0.0 {
        return 0.0;
    }
    let pd = |x: Vec2, u: Vec2, w: Vec2| {
        let d = w - u;
        let l2 = d.x * d.x + d.y * d.y;
        let t = if l2 <= 0.0 { 0.0 } else { (((x.x - u.x) * d.x + (x.y - u.y) * d.y) / l2).clamp(0.0, 1.0) };
        (x - (u + d * t)).len()
    };
    pd(p, r, s).min(pd(q, r, s)).min(pd(r, p, q)).min(pd(s, p, q))
}

/// Touch tolerance of [`explains`]: the cut test's sheet and the triangulation place the same
/// pad corner within a few um of each other (polygonised arcs, sagitta).
const TOUCH_NM: f64 = 5_000.0;

/// The over-full item `a` (one model) accounts for the over-full item `b` (the other model): on
/// the same layer, touching or crossing it, and carrying every connection of `b`. O(segments).
fn explains(a: &CompassItem, b: &CompassItem) -> bool {
    a.layers == b.layers && b.conns.iter().all(|c| a.conns.contains(c)) && a.segs.iter().any(|x| b.segs.iter().any(|y| seg_dist(x, y) <= TOUCH_NM))
}

/// Merges the items of one class into physical bottlenecks: items on the same layers with the
/// same connections whose segments come within the shorter one's length of each other are one
/// bottleneck (the top and bottom cut of one channel between two pads, the cuts of one pad gap).
/// The merged item keeps every segment, the largest `over_nm` and `members` = how many. Union
/// find, O(items^2) per class (items are the over-full ones, few).
pub fn bottlenecks(items: Vec<CompassItem>) -> Vec<CompassItem> {
    let n = items.len();
    let mut up: Vec<usize> = (0..n).collect();
    fn root(up: &mut [usize], mut i: usize) -> usize {
        while up[i] != i {
            up[i] = up[up[i]];
            i = up[i];
        }
        i
    }
    let len = |s: &[[i64; 2]; 2]| (((s[1][0] - s[0][0]) as f64).powi(2) + ((s[1][1] - s[0][1]) as f64).powi(2)).sqrt();
    for i in 0..n {
        for j in i + 1..n {
            let (a, b) = (&items[i], &items[j]);
            if a.layers != b.layers || a.conns != b.conns || a.conns.is_empty() {
                continue;
            }
            let near = a.segs.iter().any(|x| b.segs.iter().any(|y| seg_dist(x, y) <= len(x).min(len(y)).max(TOUCH_NM)));
            if near {
                let (ri, rj) = (root(&mut up, i), root(&mut up, j));
                up[rj] = ri;
            }
        }
    }
    let mut out: Vec<CompassItem> = Vec::new();
    let mut at: BTreeMap<usize, usize> = BTreeMap::new();
    for (i, it) in items.into_iter().enumerate() {
        let r = root(&mut up, i);
        match at.get(&r) {
            Some(&k) => {
                let o = &mut out[k];
                o.segs.extend(it.segs);
                o.over_nm = o.over_nm.max(it.over_nm);
                o.members += it.members;
            }
            None => {
                at.insert(r, out.len());
                out.push(it);
            }
        }
    }
    out
}

/// The note of a plan without a cut test (`[capacity] model = "edges"`, `compass_cut_check` off).
pub const NOT_MEASURED: &str = "model_gap: not measured (compass_cut_check off: [capacity] model = \"edges\" and no cut test on this plan; harness.score_runs turns it on)";

/// The two capacity models compared on the same plan (inflated-sketch §6.1): a cut over-full by
/// the cut test that no per-edge over-full edge-layer carrying all its wires touches is a model
/// gap; a per-edge over-full edge-layer that no over-full cut carrying all its wires touches is a
/// per-edge over-prediction. O(over-full edges x over-full cuts).
fn compare(edge_over: &[CompassItem], cut_over: &[CompassItem], items: &mut Vec<CompassItem>, predicted: &mut BTreeMap<String, usize>, bn: &mut BTreeMap<String, usize>) {
    let gap: Vec<CompassItem> = cut_over.iter().filter(|c| !edge_over.iter().any(|e| explains(e, c))).map(|c| CompassItem { class: "model_gap".into(), ..c.clone() }).collect();
    let opd: Vec<CompassItem> = edge_over.iter().filter(|e| !cut_over.iter().any(|c| explains(c, e))).map(|e| CompassItem { class: "edge_overpredict".into(), ..e.clone() }).collect();
    predicted.insert("model_gap".to_string(), gap.len());
    predicted.insert("edge_overpredict".to_string(), opd.len());
    let (g1, g2) = (bottlenecks(gap), bottlenecks(opd));
    bn.insert("model_gap".to_string(), g1.len());
    bn.insert("edge_overpredict".to_string(), g2.len());
    items.extend(g1);
    items.extend(g2);
}

/// `[capacity] compass_cut_check` (model = "edges"): the model comparison of the plan `v`, whose
/// `cm_cut` is the read-only cut test, added to its compass `cp` after the drawing (the caller
/// gives its time back to the run's limit). O(see [`compare`]).
pub fn model_check(v: &PlanView, cp: &mut Compass) {
    let Some(cc) = v.cm_cut else { return };
    let edge_over = edge_items(v, v.cm_edge, "planner_overfull");
    let cut_over = cut_items(v, cc, true, "planner_overfull");
    compare(&edge_over, &cut_over, &mut cp.items, &mut cp.predicted, &mut cp.bottlenecks);
    cp.notes.retain(|n| n != NOT_MEASURED);
    cp.notes.push("model_gap: measured after the drawing by a read-only cut test ([capacity] compass_cut_check, outside the time limit); the plan and the drawing used the per-edge packer".into());
}

/// The compass of the plan `v` (predicted side; the drawn side is filled by the caller and the
/// Python classifier). O(see module doc).
pub fn plan_compass(v: &PlanView, model: &str, opt_loop: &str, round: usize) -> Compass {
    let st = v.st;
    let auth = if v.cut_is_plan { v.cm_cut.unwrap_or(v.cm_edge) } else { v.cm_edge };
    let mut items: Vec<CompassItem> = Vec::new();
    let mut notes = Vec::new();
    let mut predicted = BTreeMap::new();
    let mut bn = BTreeMap::new();
    let n_nosite = auth.nosite();
    let n_over = auth.overfull().saturating_sub(n_nosite);
    // Over-full: the authority's items, merged into bottlenecks. The two models compared on the
    // same plan (§6.1): a cut over-full by the cut test that no per-edge over-full edge-layer
    // carrying all its wires touches is a model gap; a per-edge over-full edge-layer that no
    // over-full cut carrying all its wires touches is a per-edge over-prediction.
    let edge_over = edge_items(v, v.cm_edge, "planner_overfull");
    let cut_over = v.cm_cut.map(|c| cut_items(v, c, true, "planner_overfull")).unwrap_or_default();
    let over_items = if v.cut_is_plan && v.cm_cut.is_some() { cut_over.clone() } else { edge_over.clone() };
    let g = bottlenecks(over_items);
    bn.insert("planner_overfull".to_string(), g.len());
    items.extend(g);
    if v.cut_is_plan {
        if let Some(cc) = v.cm_cut {
            let g = bottlenecks(cut_items(v, cc, false, "shrinkage"));
            items.extend(g);
        }
    }
    if v.cm_cut.is_some() {
        compare(&edge_over, &cut_over, &mut items, &mut predicted, &mut bn);
    } else {
        notes.push(NOT_MEASURED.into());
    }
    // Vias without a site: per wire, the vias of its intervals (points: the planned via
    // traversals' pass points).
    for w in auth.nosite_wires() {
        let pts: Vec<[[i64; 2]; 2]> = v.cm_edge.via_k.get(w).into_iter().flatten().filter_map(|&k| st.wires[w].passes.get(k.min(st.wires[w].passes.len().saturating_sub(1)))).map(|&p| {
            let q = st.pass_point(p);
            seg(q, q)
        }).collect();
        items.push(CompassItem { class: "via_site".into(), layers: Vec::new(), conns: conns_of(v, [w]), segs: pts, over_nm: 0, members: 1 });
    }
    // Crossings no layer separates.
    for (i, &(a, b)) in v.lr.unlayerable_pairs.iter().enumerate() {
        let p = v.lr.unlayerable_at.get(i).copied();
        items.push(CompassItem { class: "unlayerable".into(), layers: Vec::new(), conns: conns_of(v, [a, b]), segs: p.map(|p| seg(p, p)).into_iter().collect(), over_nm: 0, members: 1 });
    }
    let fc = foreign_items(v);
    let n_fc = fc.len();
    items.extend(fc);
    for &c in v.forced {
        items.push(CompassItem { class: "forced_path".into(), layers: Vec::new(), conns: vec![c], segs: Vec::new(), over_nm: 0, members: 1 });
    }
    predicted.insert("planner_overfull".to_string(), n_over);
    predicted.insert("via_site".to_string(), n_nosite);
    predicted.insert("unlayerable".to_string(), v.lr.unlayerable.len());
    predicted.insert("foreign_copper".to_string(), n_fc);
    predicted.insert("forced_path".to_string(), v.forced.len());
    // Shrinkages: necked stretches of the plan (the archive's measure; with the cut model the
    // cut necks are in the pass widths by now).
    let empty = BTreeMap::new();
    let necks: Vec<(WireId, usize)> = (0..st.wires.len()).filter(|&w| st.wires[w].alive).map(|w| (w, st.neck_stretches_with(w, &empty))).collect();
    predicted.insert("shrinkage".to_string(), necks.iter().map(|x| x.1).sum());
    let mut n_px = 0;
    match pad_exit_items(v) {
        Some(px) => {
            n_px = px.iter().filter(|i| i.class == "pad_exit").count();
            predicted.insert("pad_exit".to_string(), n_px);
            items.extend(px);
        }
        None => notes.push("pad_exit: not predicted by this build (no pad-exit plan object; shopping list 5.10 adds it): every drawn pad_exit violation is a missing method step".into()),
    }
    let predicted_violations = n_over + n_nosite + v.lr.unlayerable.len() + n_fc + v.forced.len() + n_px;
    // Per connection: planned vias, taut length, necked stretches.
    let mut per: BTreeMap<usize, CompassConn> = BTreeMap::new();
    for (w, nk) in necks {
        let Some(c) = (v.conn)(w) else { continue };
        let e = per.entry(c).or_insert_with(|| CompassConn { conn: c, ..Default::default() });
        e.vias += v.cm_edge.via_k.get(w).map_or(0, |x| x.len());
        e.taut_nm += st.taut_points(w).windows(2).map(|x| (x[1] - x[0]).len()).sum::<f64>().round() as i64;
        e.necks += nk;
    }
    Compass {
        model: model.into(),
        opt_loop: opt_loop.into(),
        round,
        predicted,
        bottlenecks: bn,
        predicted_violations,
        loop_predicted: None,
        drawn_shrinkages: 0,
        items,
        conns: per.into_values().collect(),
        notes,
    }
}
