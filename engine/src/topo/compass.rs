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
/// six are predicted by the plan; `drawing_loss` and `model_gap` are drawn-only by definition
/// (the plan never predicts its own drawing's failure), `open` is a connection without copper.
pub const CLASSES: [&str; 10] = ["planner_overfull", "shrinkage", "via_site", "unlayerable", "foreign_copper", "forced_path", "pad_exit", "drawing_loss", "model_gap", "open"];

/// A located predicted item of the plan handed to the drawing (KiCad frame, nm).
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct CompassItem {
    /// One of [`CLASSES`], or `model_ref` (the other capacity model's over-full item: the
    /// classifier's evidence for `model_gap`, not part of the predicted counts).
    pub class: String,
    /// Copper layer names the item lies on; empty = any layer (a via, a crossing point).
    pub layers: Vec<String>,
    /// Connection ids ([`crate::route::ConnReport::id`]) of the wires in it.
    pub conns: Vec<usize>,
    /// Its place: segments (a cut, an edge, a traversal chord) or points (`[p, p]`).
    pub segs: Vec<[[i64; 2]; 2]>,
    /// How far over (nm, > 0 = over-full; a cut's `-slack`, a shrinkage's width reduction); 0
    /// when not measured.
    pub over_nm: i64,
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
    /// Predicted items per class of the plan handed to the drawing. A class missing from the map
    /// is not predicted by this build (`pad_exit` until 5.10 hooks in).
    pub predicted: BTreeMap<String, usize>,
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
    /// The cut model of the plan (`[capacity] model = "cuts"`), else `None`.
    pub cm_cut: Option<&'a CapModel<'c>>,
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
            CompassItem { class: class.into(), layers: vec![(v.layer)(l)], conns: conns_of(v, ws), segs: vec![seg(pr.pos[a], pr.pos[b])], over_nm: 0 }
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
            out.push(CompassItem { class: "foreign_copper".into(), layers: ls, conns: conns_of(v, [w]), segs: chords.into_iter().map(|(a, b)| seg(a, b)).collect(), over_nm: 0 });
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

/// The compass of the plan `v` (predicted side; the drawn side is filled by the caller and the
/// Python classifier). O(see module doc).
pub fn plan_compass(v: &PlanView, model: &str, opt_loop: &str, round: usize) -> Compass {
    let st = v.st;
    let auth = v.cm_cut.unwrap_or(v.cm_edge);
    let mut items: Vec<CompassItem> = Vec::new();
    let mut notes = Vec::new();
    // Over-full: the authority's items; the other model's over-full items as the model-gap
    // evidence (with the cut model in charge the per-edge packer is the drawing's model).
    let n_nosite = auth.nosite();
    let n_over = auth.overfull().saturating_sub(n_nosite);
    if let Some(cc) = v.cm_cut {
        items.extend(cut_items(v, cc, true, "planner_overfull"));
        items.extend(cut_items(v, cc, false, "shrinkage"));
        items.extend(edge_items(v, v.cm_edge, "model_ref"));
    } else {
        items.extend(edge_items(v, v.cm_edge, "planner_overfull"));
        notes.push("model_gap: not measured ([capacity] model = \"edges\": the plan and the drawing share the per-edge packer; the per-layer cut test runs only with model = \"cuts\" or [capacity] cut_report, see harness/cutclass.py)".into());
    }
    // Vias without a site: per wire, the vias of its intervals (points: the planned via
    // traversals' pass points).
    for w in auth.nosite_wires() {
        let pts: Vec<[[i64; 2]; 2]> = v.cm_edge.via_k.get(w).into_iter().flatten().filter_map(|&k| st.wires[w].passes.get(k.min(st.wires[w].passes.len().saturating_sub(1)))).map(|&p| {
            let q = st.pass_point(p);
            seg(q, q)
        }).collect();
        items.push(CompassItem { class: "via_site".into(), layers: Vec::new(), conns: conns_of(v, [w]), segs: pts, over_nm: 0 });
    }
    // Crossings no layer separates.
    for (i, &(a, b)) in v.lr.unlayerable_pairs.iter().enumerate() {
        let p = v.lr.unlayerable_at.get(i).copied();
        items.push(CompassItem { class: "unlayerable".into(), layers: Vec::new(), conns: conns_of(v, [a, b]), segs: p.map(|p| seg(p, p)).into_iter().collect(), over_nm: 0 });
    }
    let fc = foreign_items(v);
    let n_fc = fc.len();
    items.extend(fc);
    for &c in v.forced {
        items.push(CompassItem { class: "forced_path".into(), layers: Vec::new(), conns: vec![c], segs: Vec::new(), over_nm: 0 });
    }
    let mut predicted = BTreeMap::new();
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
        predicted_violations,
        loop_predicted: None,
        drawn_shrinkages: 0,
        items,
        conns: per.into_values().collect(),
        notes,
    }
}
