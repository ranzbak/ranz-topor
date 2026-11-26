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
    let pr = v.st.proj;
    foreign_hits(v.st, &v.lr.events, v.bottom)
        .into_iter()
        .map(|(w, bits, chords)| {
            let ls = (0..pr.n_layers.min(64)).filter(|&l| bits & (1u64 << l) != 0).map(|l| (v.layer)(l)).collect();
            CompassItem { class: "foreign_copper".into(), layers: ls, conns: conns_of(v, [w]), segs: chords.into_iter().map(|(a, b)| seg(a, b)).collect(), over_nm: 0, members: 1 }
        })
        .collect()
}

/// One traversal through foreign copper: the wire, the layer bits the object blocks it on, its
/// chords inside the object.
type ForeignHit = (WireId, u64, Vec<(Vec2, Vec2)>);

/// The (wire, object) traversals [`super::optimise::foreign_copper`] counts, in its order (same
/// test, same loops), under the layering `events`. O(traversals x obstacles per triangle).
fn foreign_hits(st: &State, events: &[Vec<(usize, f64, i32)>], bottom: i32) -> Vec<ForeignHit> {
    let pr = st.proj;
    let layers = layers_now(st, events, bottom);
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
        out.extend(inside.into_iter().map(|(_, bits, chords)| (w, bits, chords)));
    }
    out
}

/// plan-dots: where a plan's predicted violations are, for the progress dialog's dots on a plan
/// sketch (`control::Sketch::errors_at`, KiCad frame, nm), and how many have no place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanPlaces {
    pub at: Vec<[i64; 2]>,
    /// Counted but not located: the connections placed without a legal path (`forced`; the
    /// plan knows the connection, not the spot where its path breaks a rule).
    pub unplaced: usize,
}

/// plan-dots: one place per predicted violation the TopoR loop's measure counts for the plan
/// `st` + `lr` ([`super::optimise::Predicted::count`]), part by part in its order: the capacity
/// model `cm`'s count ([`CapModel::error_places`]: over-full edge-layers or cuts, vias without a
/// site, over-full via cuts), the crossings no layer separates (`lr.unlayerable`, at the
/// crossing), the traversals through foreign copper (at the middle of the first chord inside
/// the object, [`foreign_hits`]), the pour pieces the plan cuts off (`pour`, at the pieces'
/// centroid, [`super::pourplan::cut_components`]); `forced` connections without a place
/// ([`PlanPlaces::unplaced`]). So `at.len() + unplaced` is the measure's count of the same plan
/// built with the same capacity model. Places may coincide (two layers of one edge). Measurement
/// only. O(as the measure).
pub fn plan_places(st: &State, lr: &super::layers::LayerReport, cm: &CapModel, bottom: i32, forced: usize, pour: &[super::pourplan::CutComp]) -> PlanPlaces {
    let mut at: Vec<[i64; 2]> = cm.error_places(&lr.events).into_iter().map(kicad).collect();
    at.extend(lr.unlayerable.iter().map(|&p| kicad(p)));
    at.extend(foreign_hits(st, &lr.events, bottom).into_iter().filter_map(|(_, _, ch)| ch.first().map(|&(a, b)| kicad((a + b) * 0.5))));
    at.extend(pour.iter().map(|c| kicad(c.at)));
    PlanPlaces { at, unplaced: forced }
}

/// The plan's necked stretches, located (TW4 meter: before, `shrinkage` items came only from the
/// cut model's "fits only necked" cuts with the cuts as the plan's authority, so a drawn violation
/// on a planned neck of the default per-edge plan could never be classified `neck_loss`). One item
/// per run of necked passes (an end run, `neck_from_pad`, starts at the pad's centre): segs = the
/// chords of the wire from the point before the run's first pass to the point after its last,
/// layers = the layers its triangles are on in the plan's layering (empty: not layered),
/// `over_nm` = how far the narrowest pass of the run is below the wire's width. Counted as before
/// (`predicted["shrinkage"]`); the items only locate them. O(passes).
pub fn neck_items(v: &PlanView) -> Vec<CompassItem> {
    let st = v.st;
    let pr = st.proj;
    let layers = layers_now(st, &v.lr.events, v.bottom);
    let all = if pr.n_layers >= 64 { u64::MAX } else { (1u64 << pr.n_layers) - 1 };
    let mut out = Vec::new();
    for (w, wi) in st.wires.iter().enumerate() {
        if !wi.alive || wi.passes.is_empty() {
            continue;
        }
        let n = wi.passes.len();
        let lw = layers.get(w).filter(|l| l.len() == wi.tris.len() && !l.is_empty());
        // Point i of the wire: 0 = end a, i in 1..=n = pass i - 1, n + 1 = end b.
        let pt = |i: usize| if i == 0 { pr.pos[wi.a] } else if i > n { pr.pos[wi.b] } else { st.pass_point(wi.passes[i - 1]) };
        let mut k = 0;
        while k < n {
            if !st.necked(wi.passes[k]) {
                k += 1;
                continue;
            }
            let k0 = k;
            while k < n && st.necked(wi.passes[k]) {
                k += 1;
            }
            // Necked passes k0..k = points k0 + 1..=k; the stretch runs from point k0 to point
            // k + 1, through triangles k0..=k (triangle t lies between pass t - 1 and pass t).
            let segs = (k0..=k).map(|i| seg(pt(i), pt(i + 1))).collect();
            let bits = lw.map_or(0, |l| (k0..=k.min(l.len() - 1)).fold(0u64, |m, t| m | l[t]) & all);
            let ls = (0..pr.n_layers.min(64)).filter(|&l| bits & (1u64 << l) != 0).map(|l| (v.layer)(l)).collect();
            let wmin = wi.passes[k0..k].iter().map(|&p| st.pass_w[p]).fold(f64::INFINITY, f64::min);
            out.push(CompassItem { class: "shrinkage".into(), layers: ls, conns: conns_of(v, [w]), segs, over_nm: (wi.w - wmin).max(0.0).round() as i64, members: 1 });
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

/// tw4-r2 (1) (`[capacity] pad_exits`): one `pad_exit` item (predicted) per planned pad exit with
/// an over-full term (`refine::CapModel::exit_items`): segs = the stub's centre line, layers =
/// the pad's, conns = the wire's connection, over_nm = the graded price x 1000 (no length). O(exits).
pub fn plan_exit_items(list: &[(WireId, u8, usize, Vec2, Vec2, f64)], conn: &dyn Fn(WireId) -> Option<usize>, layer: &dyn Fn(usize) -> String) -> Vec<CompassItem> {
    list.iter()
        .map(|&(w, _, l, a, b, g)| CompassItem { class: "pad_exit".into(), layers: vec![layer(l)], conns: conn(w).into_iter().collect(), segs: vec![seg(a, b)], over_nm: (g * 1000.0).round() as i64, members: 1 })
        .collect()
}

/// The items of [`PlanView::pad_exits`] for a build with `[search] pad_exit_stubs` on (the caller
/// passes `None` with it off): one `pad_exit_stub` per `Board::exit_stubs` entry (segs = the stub
/// rectangle's sides) and one `pad_exit` per `Board::exit_blocked` entry (segs = the pad's sides:
/// the needed exit that no legal stub realises, a violation the plan predicts). Layers = the
/// pad's, conns = the connections of the wires ending at that pad (`conn`). O(stubs x wires).
pub fn exit_stub_items(b: &super::board::Board, st: &State, conn: &dyn Fn(WireId) -> Option<usize>, layer: &dyn Fn(usize) -> String) -> Vec<CompassItem> {
    let pr = &b.proj;
    let at_pad = |pad: usize| -> Vec<usize> {
        let mut c: Vec<usize> = (0..st.wires.len())
            .filter(|&w| st.wires[w].alive && [st.wires[w].pa, st.wires[w].pb].iter().any(|&o| b.pad_of.get(o) == Some(&pad)))
            .filter_map(conn)
            .collect();
        c.sort_unstable();
        c.dedup();
        c
    };
    let sides = |o: usize| -> Vec<[[i64; 2]; 2]> {
        let p = &pr.obstacles[o].poly;
        (0..p.len()).map(|i| seg(p[i], p[(i + 1) % p.len()])).collect()
    };
    let lay = |o: usize| -> Vec<String> {
        let m = pr.obstacles[o].layers.0;
        (0..pr.n_layers.min(64)).filter(|&l| m & (1u64 << l) != 0).map(layer).collect()
    };
    let one = |class: &str, o: usize, sides_of: usize, pad: usize| CompassItem { class: class.into(), layers: lay(o), conns: at_pad(pad), segs: sides(sides_of), over_nm: 0, members: 1 };
    let mut out: Vec<CompassItem> = b.exit_stubs.iter().map(|&(so, pad)| one("pad_exit_stub", so, so, pad)).collect();
    out.extend(b.exit_blocked.iter().map(|&(po, pad)| one("pad_exit", po, po, pad)));
    out
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
    let mut g = bottlenecks(over_items);
    // CM1 (tw4 T2, `[capacity] via_cuts`): the planned vias with an over-full gap or via-to-via
    // cut, located at the via (any layer), with the via's connection and its cuts' crossers.
    let xs: BTreeMap<WireId, Vec<WireId>> = auth.via_cut_list().into_iter().collect();
    for (w, p) in auth.via_cut_points() {
        let ws = std::iter::once(w).chain(xs.get(&w).into_iter().flatten().copied());
        g.push(CompassItem { class: "planner_overfull".into(), layers: Vec::new(), conns: conns_of(v, ws), segs: vec![seg(p, p)], over_nm: 0, members: 1 });
    }
    bn.insert("planner_overfull".to_string(), g.len());
    items.extend(g);
    if v.cut_is_plan {
        if let Some(cc) = v.cm_cut {
            let g = bottlenecks(cut_items(v, cc, false, "shrinkage"));
            items.extend(g);
        }
    }
    // The plan's necked stretches, located (every model; the cut items above are their cuts).
    items.extend(neck_items(v));
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

// ---- draw-absorb A0: the drawn side, per violation (docs/dev/design/draw-absorb.md §1, §4 A0) ----

/// `toporlib/compass.py` `REACH_MM`: the least reach of a drawn violation to a plan item (mm).
pub const DRAWN_REACH_MM: f64 = 0.5;

/// Distance from `p` to segment `a`-`b` (nm, KiCad frame); the `_seg_dist` of compass.py.
fn pt_seg_dist(p: [i64; 2], a: [i64; 2], b: [i64; 2]) -> f64 {
    let f = |q: [i64; 2]| (q[0] as f64, q[1] as f64);
    let ((px, py), (ax, ay), (bx, by)) = (f(p), f(a), f(b));
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = dx * dx + dy * dy;
    let t = if l2 <= 0.0 { 0.0 } else { (((px - ax) * dx + (py - ay) * dy) / l2).clamp(0.0, 1.0) };
    (px - (ax + t * dx)).hypot(py - (ay + t * dy))
}

/// `it` lies on `layer` (or any layer) with a segment within `r` nm of `at` (no segments:
/// anywhere); the `_near` of compass.py.
fn near(it: &CompassItem, at: [i64; 2], layer: &str, r: f64) -> bool {
    if !it.layers.is_empty() && !it.layers.iter().any(|l| l == layer) && layer != "all layers" {
        return false;
    }
    it.segs.is_empty() || it.segs.iter().any(|s| pt_seg_dist(at, s[0], s[1]) <= r)
}

/// The drawn compass class of a violation of engine class `vc` at `at` on `layer` (reach `r`,
/// nm): the rule of `toporlib/compass.py` `classify_violation`, the first that matches, in its
/// order (a port, so the engine's result carries it; `harness` checks both agree, the Python
/// stays the reference). `mine`: the compass items of its connection, in item order; `others`:
/// the over-full and model-gap items of other connections. O(items).
pub fn drawn_class(vc: &str, at: [i64; 2], layer: &str, mine: &[&CompassItem], others: &[&CompassItem], r: f64) -> &'static str {
    let first = |cls: &str, need_near: bool| mine.iter().any(|it| it.class == cls && (!need_near || near(it, at, layer, r)));
    if vc == "via_site" && first("via_site", false) {
        return "via_site";
    }
    for cls in ["unlayerable", "foreign_copper", "planner_overfull", "model_gap"] {
        if first(cls, true) {
            return match cls {
                "unlayerable" => "unlayerable",
                "foreign_copper" => "foreign_copper",
                "planner_overfull" => "planner_overfull",
                _ => "model_gap",
            };
        }
    }
    if others.iter().any(|it| (it.class == "planner_overfull" || it.class == "model_gap") && near(it, at, layer, r)) {
        return "overfull_nearby";
    }
    if first("forced_path", false) {
        return "forced_path";
    }
    if first("pad_exit", true) || (vc == "pad_exit" && first("pad_exit_stub", true)) {
        return "pad_exit";
    }
    if first("shrinkage", true) {
        return if vc == "pad_exit" { "pad_exit" } else { "neck_loss" };
    }
    if first("edge_overpredict", true) {
        return "edge_overpredict";
    }
    if vc == "pad_exit" {
        return "pad_exit";
    }
    "drawing_loss"
}

/// The fault class of the note (docs/dev/design/draw-absorb.md §1) of a drawn violation written by
/// `mech`, of drawn compass class `cls`, whose copper hits a `movable` (another router wire) or
/// static item: F1 forced via, static hit; F2 forced via, movable hit; F3 at a planned neck
/// (`neck_loss`); F4 the plan's cuts fit (`drawing_loss`, `overfull_nearby`, `edge_overpredict`),
/// movable hit; F5 the same, static hit; F6 the plan has no room or no method there (over-full,
/// model gap, unlayerable, foreign copper, forced path, via site of a run); F7 pad exit.
pub fn fault_class(mech: &str, cls: &str, movable: bool) -> &'static str {
    if mech == "forced_via" {
        return if movable { "F2" } else { "F1" };
    }
    match cls {
        "neck_loss" => "F3",
        "drawing_loss" | "overfull_nearby" | "edge_overpredict" => {
            if movable {
                "F4"
            } else {
                "F5"
            }
        }
        "pad_exit" => "F7",
        _ => "F6",
    }
}

/// draw-absorb A0: the drawn compass class and fault class of every violation of `r`
/// (`violations[].prov`) and their counts (`r.draw_absorb`), from the result alone (its compass,
/// its connections' widths), so it holds for whichever variant is handed back. A result without a
/// compass classes every violation `drawing_loss` (as compass.py does). Measurement only.
/// O(violations x compass items).
pub fn absorb_meter(r: &mut crate::route::RouteResult) {
    let width: BTreeMap<usize, i64> = r.connections.iter().map(|c| (c.id, c.width)).collect();
    if let Some(m) = absorb_classes(&mut r.violations, r.compass.as_ref(), &width) {
        r.draw_absorb = Some(m);
    }
}

/// [`absorb_meter`] on the parts: the violations `vs` (their `prov` filled), the round's compass
/// `cp`, the connections' widths (nm by id). `None` when no violation has a provenance (another
/// engine, or nothing drawn by force). O(violations x compass items).
pub fn absorb_classes(vs: &mut [crate::route::ViolationReport], cp: Option<&Compass>, width: &BTreeMap<usize, i64>) -> Option<crate::route::DrawAbsorb> {
    if vs.iter().all(|v| v.prov.is_none()) {
        return None;
    }
    let items: &[CompassItem] = cp.map_or(&[], |c| &c.items[..]);
    let mut by_conn: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (k, it) in items.iter().enumerate() {
        for &c in &it.conns {
            by_conn.entry(c).or_default().push(k);
        }
    }
    let has = cp.is_some();
    let mut m = crate::route::DrawAbsorb::default();
    let mut conns: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let mut runs: BTreeMap<(usize, usize), String> = BTreeMap::new();
    let add = |map: &mut BTreeMap<String, usize>, k: &str| *map.entry(k.to_string()).or_insert(0) += 1;
    for v in vs.iter_mut() {
        let Some(p) = v.prov.as_mut() else { continue };
        let reach = (DRAWN_REACH_MM * 1e6).max(4.0 * width.get(&v.conn).copied().unwrap_or(0) as f64) + 0.5 * v.length_nm as f64;
        let mine_k = by_conn.get(&v.conn).map_or(&[][..], |x| &x[..]);
        let mine: Vec<&CompassItem> = mine_k.iter().map(|&k| &items[k]).collect();
        let others: Vec<&CompassItem> = items.iter().enumerate().filter(|(k, it)| (it.class == "planner_overfull" || it.class == "model_gap") && !mine_k.contains(k)).map(|x| x.1).collect();
        p.class = if has { drawn_class(&v.class, v.at, &v.layer, &mine, &others, reach) } else { "drawing_loss" }.into();
        p.fclass = fault_class(&p.mech, &p.class, p.hit == "movable").into();
        m.violations += 1;
        add(&mut m.by_fclass, &p.fclass);
        conns.entry(p.fclass.clone()).or_default().insert(v.conn);
        add(&mut m.by_mech, if p.mech.is_empty() { "unrecorded" } else { &p.mech });
        add(&mut m.by_cand, if p.cand.is_empty() { "-" } else { &p.cand });
        add(&mut m.by_cut, if p.cut.is_empty() { "-" } else { &p.cut });
        add(&mut m.by_class, &p.class);
        add(&mut m.by_hit, &p.hit);
        if p.mech == "forced_run" {
            runs.insert((v.conn, p.run), if p.cut.is_empty() { "-".into() } else { p.cut.clone() });
        }
        if p.hit == "movable" && p.hit_slack_nm.is_some_and(|s| s >= v.depth_nm) {
            add(&mut m.retaut_room, &p.fclass);
        }
        if p.mech != "forced_via" && p.drawn_w_nm > 0 && (p.drawn_w_nm - p.w_min_nm) >= 2 * v.depth_nm {
            add(&mut m.shrink_room, &p.fclass);
        }
    }
    m.conns_by_fclass = conns.into_iter().map(|(k, s)| (k, s.len())).collect();
    for c in runs.into_values() {
        *m.forced_runs_by_cut.entry(c).or_insert(0) += 1;
    }
    Some(m)
}

#[cfg(test)]
mod absorb_tests {
    use super::*;
    use crate::route::{ViolProv, ViolationReport};

    fn item(class: &str, conns: &[usize], at: [i64; 2], layers: &[&str]) -> CompassItem {
        CompassItem { class: class.into(), layers: layers.iter().map(|s| s.to_string()).collect(), conns: conns.to_vec(), segs: vec![[at, at]], over_nm: 0, members: 1 }
    }

    fn viol(conn: usize, class: &str, at: [i64; 2], mech: &str, hit: &str) -> ViolationReport {
        ViolationReport { conn, class: class.into(), at, layer: "F.Cu".into(), depth_nm: 50_000, length_nm: 200_000, prov: Some(ViolProv { mech: mech.into(), hit: hit.into(), ..ViolProv::default() }), ..ViolationReport::default() }
    }

    /// The drawn class follows compass.py's order: each branch, and a near item of an earlier
    /// class wins over a later one.
    #[test]
    fn drawn_class_takes_the_first_match_in_compass_order() {
        let at = [1_000_000, 1_000_000];
        let far = [9_000_000, 9_000_000];
        let r = 600_000.0;
        let c = |cls: &str, vc: &str, at_it: [i64; 2]| {
            let it = item(cls, &[0], at_it, &["F.Cu"]);
            drawn_class(vc, at, "F.Cu", &[&it], &[], r)
        };
        assert_eq!(c("via_site", "via_site", far), "via_site", "a via site needs no nearness");
        assert_eq!(c("via_site", "clearance", far), "drawing_loss", "only for a via_site violation");
        assert_eq!(c("unlayerable", "clearance", at), "unlayerable");
        assert_eq!(c("foreign_copper", "clearance", at), "foreign_copper");
        assert_eq!(c("planner_overfull", "clearance", at), "planner_overfull");
        assert_eq!(c("planner_overfull", "clearance", far), "drawing_loss", "an over-full cut far away");
        assert_eq!(c("model_gap", "clearance", at), "model_gap");
        assert_eq!(c("forced_path", "clearance", far), "forced_path");
        assert_eq!(c("pad_exit", "clearance", at), "pad_exit");
        assert_eq!(c("pad_exit_stub", "pad_exit", at), "pad_exit");
        assert_eq!(c("pad_exit_stub", "clearance", at), "drawing_loss");
        assert_eq!(c("shrinkage", "clearance", at), "neck_loss");
        assert_eq!(c("shrinkage", "pad_exit", at), "pad_exit");
        assert_eq!(c("edge_overpredict", "clearance", at), "edge_overpredict");
        assert_eq!(c("nothing", "pad_exit", at), "pad_exit");
        assert_eq!(c("nothing", "clearance", at), "drawing_loss");
        // Another layer does not match; an item on no layer does.
        let it = item("planner_overfull", &[0], at, &["B.Cu"]);
        assert_eq!(drawn_class("clearance", at, "F.Cu", &[&it], &[], r), "drawing_loss");
        let it = item("planner_overfull", &[0], at, &[]);
        assert_eq!(drawn_class("clearance", at, "F.Cu", &[&it], &[], r), "planner_overfull");
        // Another wire's over-full bottleneck nearby; the own over-full one first.
        let o = item("planner_overfull", &[1], at, &["F.Cu"]);
        assert_eq!(drawn_class("clearance", at, "F.Cu", &[], &[&o], r), "overfull_nearby");
        let sh = item("shrinkage", &[0], at, &["F.Cu"]);
        assert_eq!(drawn_class("clearance", at, "F.Cu", &[&sh], &[&o], r), "overfull_nearby", "over-full nearby before the neck");
        let fp = item("forced_path", &[0], far, &[]);
        assert_eq!(drawn_class("clearance", at, "F.Cu", &[&sh, &fp], &[], r), "forced_path", "forced path before the neck");
        assert_eq!(drawn_class("clearance", at, "F.Cu", &[&fp], &[&o], r), "overfull_nearby", "over-full nearby before the forced path");
    }

    #[test]
    fn fault_class_follows_the_note() {
        assert_eq!(fault_class("forced_via", "pad_exit", false), "F1");
        assert_eq!(fault_class("forced_via", "via_site", true), "F2");
        assert_eq!(fault_class("forced_run", "neck_loss", true), "F3");
        assert_eq!(fault_class("forced_run", "neck_loss", false), "F3");
        for c in ["drawing_loss", "overfull_nearby", "edge_overpredict"] {
            assert_eq!(fault_class("forced_run", c, true), "F4");
            assert_eq!(fault_class("gap_kept", c, false), "F5");
        }
        for c in ["planner_overfull", "model_gap", "unlayerable", "foreign_copper", "forced_path", "via_site"] {
            assert_eq!(fault_class("forced_run", c, true), "F6");
        }
        assert_eq!(fault_class("forced_run", "pad_exit", true), "F7");
    }

    /// The counts: per fault class and connection, per writer, forced runs per band status once
    /// per (connection, run), the re-taut room (hit slack >= depth, movable only) and the shrink
    /// room (narrowing to the floor clears twice the depth, never for a via).
    #[test]
    fn the_meter_counts_every_class_once() {
        let at = [1_000_000, 1_000_000];
        let cp = Compass { items: vec![item("shrinkage", &[0], at, &["F.Cu"])], ..Compass::default() };
        let mut vs = vec![viol(0, "clearance", at, "forced_run", "movable"), viol(0, "clearance", at, "forced_run", "movable"), viol(1, "clearance", at, "forced_via", "static"), viol(2, "clearance", at, "forced_run", "static")];
        for (v, (cut, run, slack, w, wmin)) in vs.iter_mut().zip([("gap", 0, Some(60_000), 500_000, 380_000), ("gap", 0, Some(40_000), 500_000, 450_000), ("", 0, None, 0, 0), ("no_band", 1, None, 250_000, 250_000)]) {
            let p = v.prov.as_mut().unwrap();
            (p.cut, p.run, p.hit_slack_nm, p.drawn_w_nm, p.w_min_nm) = (cut.into(), run, slack, w, wmin);
        }
        vs.push(ViolationReport { conn: 3, prov: None, ..ViolationReport::default() });
        let width: BTreeMap<usize, i64> = [(0, 500_000), (1, 250_000), (2, 250_000)].into_iter().collect();
        let m = absorb_classes(&mut vs, Some(&cp), &width).expect("provenance present");
        let cls: Vec<&str> = vs.iter().filter_map(|v| v.prov.as_ref()).map(|p| p.fclass.as_str()).collect();
        assert_eq!(cls, ["F3", "F3", "F1", "F5"]);
        assert_eq!(m.violations, 4);
        let get = |x: &BTreeMap<String, usize>, k: &str| x.get(k).copied().unwrap_or(0);
        assert_eq!((get(&m.by_fclass, "F3"), get(&m.by_fclass, "F1"), get(&m.by_fclass, "F5")), (2, 1, 1));
        assert_eq!((get(&m.conns_by_fclass, "F3"), get(&m.conns_by_fclass, "F1")), (1, 1));
        assert_eq!((get(&m.by_mech, "forced_run"), get(&m.by_mech, "forced_via")), (3, 1));
        assert_eq!((get(&m.forced_runs_by_cut, "gap"), get(&m.forced_runs_by_cut, "no_band")), (1, 1), "one forced run per (connection, run)");
        assert_eq!(get(&m.retaut_room, "F3"), 1, "slack 60 um >= depth 50 um; 40 um is not");
        assert_eq!(get(&m.shrink_room, "F3"), 1, "0.5 -> 0.38 mm frees 60 um a side >= 50 um; 0.5 -> 0.45 does not");
        assert_eq!(get(&m.shrink_room, "F5"), 0, "at its floor");
        // No compass: every violation a drawing loss (as compass.py), no provenance at all: no meter.
        let m2 = absorb_classes(&mut vs[..1], None, &width).unwrap();
        assert_eq!(vs[0].prov.as_ref().unwrap().class, "drawing_loss");
        assert_eq!(get(&m2.by_fclass, "F4"), 1);
        assert!(absorb_classes(&mut vs[4..], Some(&cp), &width).is_none());
    }
}
