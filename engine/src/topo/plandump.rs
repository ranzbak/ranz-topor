//! The plan handed to the drawing, as JSON: per wire its net, connection, width and clearance,
//! taut points, the layering's stretches and events, the layers of every triangle traversal, its
//! passes (edge ends, position in the edge's order, room per layer, need) and its crossings in
//! order along the wire with the layers of both wires there (the crossing order and homotopy).
//!
//! Two users: the debug dump `TOPO_PLANDUMP=<file>` ([`wire_json`] per wire, before the
//! unlayerable drops, every drawn round), and the plan report (`[output] plan_report`, thick-layer
//! F4 / alt-route-probe R4): [`plan_json`] of the plan the drawing got, carried in the round's
//! `RouteResult::plan` with the row it belongs to (label, roles, round, seed), so the result handed
//! back (and the plugin's `plan.json.gz` in the report folder) holds the plan of exactly the row
//! that was drawn. The report adds per wire its planned vias (interval, traversal, place, site,
//! static room deficit) and per connection the compass's planned vias, taut length, necks and
//! predicted items by class.
//!
//! Measurement only: reads the State, the layering, the capacity model and the compass the round
//! has built anyway; never changes the plan or the drawing. Coordinates in KiCad mm (y down),
//! rounded to 1 um. O(wires x (passes + crossings) + vias x one site test + compass items).

use super::compass::Compass;
use super::layers::LayerReport;
use super::refine::CapModel;
use super::state::{State, WireId};
use crate::geom::Vec2;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const MM: f64 = 1e6;

/// Format tag and version of [`plan_json`] (readers: `toporlib/plandump.py`).
pub const FORMAT: &str = "topor-plan";
pub const VERSION: u32 = 1;

fn mm(x: f64) -> f64 {
    (x / MM * 1e3).round() / 1e3
}

fn pt(p: Vec2) -> Value {
    json!([mm(p.x), mm(-p.y)])
}

/// Signal layer indices of a `layers_now` bit mask, ascending.
fn bits(m: u64) -> Vec<u32> {
    (0..64).filter(|&l| m >> l & 1 == 1).collect()
}

/// One wire of the plan (the fields of the `TOPO_PLANDUMP` debug dump; `lnow` =
/// [`super::optimise::layers_now`] of the same layering). The crossings carry the layers of both
/// wires at the crossing's triangle (`layers`, `other_layers`: signal layer indices), so their
/// order along the wire reads as the crossing order on each layer. O(passes + crossings).
pub fn wire_json(st: &State, lr: &LayerReport, lnow: &[Vec<u64>], w: WireId, conn: Option<usize>) -> Value {
    let proj = st.proj;
    let wi = &st.wires[w];
    let passes: Vec<Value> = wi
        .passes
        .iter()
        .enumerate()
        .map(|(k, &p)| {
            let e = st.passes[p].edge;
            let [a, c] = proj.edges[e];
            json!({"k": k, "edge": e, "a": pt(proj.pos[a]), "b": pt(proj.pos[c]), "len": mm((proj.pos[c] - proj.pos[a]).len()),
                "room": (0..proj.n_layers).map(|l| { let r = st.room(e, l); if r.is_finite() { mm(r) } else { -1.0 } }).collect::<Vec<_>>(),
                "need": mm(st.edge_need(e, None)), "n": st.edge_order[e].len(),
                "pos": st.edge_order[e].iter().position(|&q| q == p), "at": pt(st.pass_point(p))})
        })
        .collect();
    let lw = lnow.get(w).map_or(&[][..], |v| v.as_slice());
    let xs: Vec<Value> = st
        .crossings(w)
        .iter()
        .map(|x| {
            let lo = lnow.get(x.other).and_then(|v| v.get(x.other_k)).copied().unwrap_or(0);
            json!({"k": x.k, "other": x.other, "other_k": x.other_k, "other_net": st.wires[x.other].net,
                "layers": bits(lw.get(x.k).copied().unwrap_or(0)), "other_layers": bits(lo)})
        })
        .collect();
    json!({"wire": w, "net": wi.net, "conn": conn, "w": wi.w / MM, "s": wi.s / MM, "a": pt(proj.pos[wi.a]), "b": pt(proj.pos[wi.b]),
        "taut": st.taut_points(w).into_iter().map(pt).collect::<Vec<_>>(),
        "stretches": lr.stretches.get(w).map(|v| v.iter().map(|(l, ps)| json!([l, ps.iter().map(|&q| pt(q)).collect::<Vec<_>>()])).collect::<Vec<_>>()).unwrap_or_default(),
        "events": lr.events.get(w).cloned().unwrap_or_default(), "layers_now": lw, "passes": passes, "crossings": xs})
}

/// The row a plan belongs to: what drew it (the TopoR variant table's row label and roles, or
/// none), the round (cycle) and the run's seed.
pub struct Row<'a> {
    pub label: Option<&'a str>,
    pub roles: Vec<&'static str>,
    pub round: usize,
    pub seed: u64,
    pub opt_loop: &'a str,
}

/// The plan report of the plan handed to the drawing (see the module doc). `cm` = the plan's
/// per-edge capacity model (the drawing's via positions), `cp` = its compass, `conn` = wire ->
/// connection id, `net_name` = net code -> name, `layers` = signal layer names by index,
/// `foreign` = the plan's traversals through foreign copper (Q14b). O(as the module doc).
#[allow(clippy::too_many_arguments)]
pub fn plan_json(st: &State, lr: &LayerReport, bottom: i32, cm: &CapModel, cp: Option<&Compass>, conn: &dyn Fn(WireId) -> Option<usize>, net_name: &dyn Fn(i32) -> String, layers: &[String], foreign: usize, row: &Row) -> Value {
    let lnow = super::optimise::layers_now(st, &lr.events, bottom);
    // Predicted items of the compass per connection, by class (an item of several connections
    // counts for each).
    let mut pred: BTreeMap<usize, BTreeMap<&str, usize>> = BTreeMap::new();
    if let Some(cp) = cp {
        for it in &cp.items {
            for &c in &it.conns {
                *pred.entry(c).or_default().entry(it.class.as_str()).or_default() += 1;
            }
        }
    }
    let wires: Vec<Value> = (0..st.wires.len())
        .filter(|&w| st.wires[w].alive)
        .map(|w| {
            let mut v = wire_json(st, lr, &lnow, w, conn(w));
            let ev = lr.events.get(w).map_or(&[][..], |e| e.as_slice());
            v["net_name"] = json!(net_name(st.wires[w].net));
            v["vias"] = cm
                .via_report(w, ev)
                .into_iter()
                .map(|v| json!({"iv": [v.iv.0, v.iv.1], "k": v.k, "at": pt(v.at), "site": !v.nosite, "deficit": mm(v.deficit)}))
                .collect();
            v["nosite"] = json!(cm.nosite.get(w).copied().unwrap_or(0));
            v
        })
        .collect();
    let conns: BTreeMap<String, Value> = cp
        .map(|cp| {
            cp.conns
                .iter()
                .map(|c| {
                    (c.conn.to_string(), json!({"vias": c.vias, "taut_mm": mm(c.taut_nm as f64), "necks": c.necks, "predicted": pred.get(&c.conn).cloned().unwrap_or_default()}))
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "format": FORMAT, "version": VERSION,
        "row": {"label": row.label, "roles": row.roles, "round": row.round, "seed": row.seed, "loop": row.opt_loop},
        "units": "mm, KiCad frame (y down); layers = signal layer indices into \"layers\"",
        "layers": layers,
        "predicted": cp.map(|c| json!(c.predicted)).unwrap_or(Value::Null),
        "predicted_violations": cp.map(|c| c.predicted_violations),
        "foreign": foreign,
        "conns": conns,
        "wires": wires,
    })
}
