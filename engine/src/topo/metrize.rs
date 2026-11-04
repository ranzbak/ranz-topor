//! Metrization gate (plan S1): does a wire's sleeve (the strip of projection triangles it runs
//! through) hold a real drawing? For one *leg* (a wire on one layer, pad to pad) P1 says "fits"
//! when every cut along the sleeve has room for the wire on that layer, alone (the demand model
//! of [`super::demand`], the room of the LCT cuts), and the layer is allowed in every triangle.
//! Metrization draws the leg with `taut` (the provisional reference, restored from `main`): the
//! funnel over the sleeve gives the bends, the CDT vertices mapped to obstacle anchors, `tauten`
//! pulls the chain taut against the inflated true copper (circle metric, arcs), and the engine's
//! `Checker` judges the quantised segments and arcs against pads, holes and the board edge.
//! The gate compares the two answers per leg (2 x 2 matrix).
//!
//! Legs are drawn alone: other wires are not obstacles, so P1 is the "alone" form too
//! (the passage-order drawing of many wires through one passage is the S2 job, plan "Still to
//! specify" A).
//!
//! Complexity: O(sleeve) for the funnel and the fit; `tauten` is O(passes x anchors queried).

use super::demand::Item;
use super::state::{EdgeId, Projection, TriId};
use crate::check::Checker;
use crate::geom::prim::Aabb;
use crate::geom::quant::quantise_chain;
use crate::geom::{Prim, Vec2};
use crate::model::{flip_track, BoardModel, Item as OutItem, Options, TrackItem};
use crate::obstacle::{LayerObstacles, ObstacleSet};
use crate::sketch::{funnel, AnchorRef};
use crate::taut::{tauten, DynObstacle, Env};
use serde::Serialize;
use std::collections::HashSet;

/// Shortfall (nm) of a drawing against a rule that still counts as clean: the quantisation to 1 nm
/// of a chain that is tangent to the inflated copper.
pub const TOL_NM: f64 = 10.0;

/// One wire on one layer, pad to pad (or pad to free end).
#[derive(Clone, Debug)]
pub struct Leg {
    pub net: i32,
    /// Track width and clearance (nm).
    pub w: f64,
    pub s: f64,
    /// Signal layer index.
    pub layer: usize,
    pub start: Vec2,
    pub goal: Vec2,
    /// Triangles of the sleeve from the start to the goal triangle.
    pub tris: Vec<TriId>,
    /// The cut edge between consecutive triangles (`tris.len() - 1` of them).
    pub edges: Vec<EdgeId>,
}

/// The two answers for a leg.
#[derive(Clone, Debug, Serialize)]
pub struct LegOutcome {
    pub fits: bool,
    /// Why P1 says "does not fit" (first cut), empty when it fits.
    pub fit_why: String,
    pub taut_ok: bool,
    /// Why the drawing failed: taut error, Checker violation or "left the sleeve".
    pub taut_why: String,
    pub net: i32,
    pub layer: usize,
    /// Start of the leg (KiCad frame, nm).
    pub at: [f64; 2],
    pub len_mm: f64,
    /// The sleeve's triangles and the drawn path, KiCad frame nm (for the renderer).
    pub sleeve: Vec<[[f64; 2]; 3]>,
    pub path: Vec<[f64; 2]>,
}

/// 2 x 2 agreement of "P1 fits" and "metrization succeeds".
#[derive(Clone, Debug, Default, Serialize)]
pub struct Matrix {
    pub fit_and_ok: usize,
    pub fit_not_ok: usize,
    pub nofit_ok: usize,
    pub nofit_not_ok: usize,
}

impl Matrix {
    pub fn add(&mut self, o: &LegOutcome) {
        match (o.fits, o.taut_ok) {
            (true, true) => self.fit_and_ok += 1,
            (true, false) => self.fit_not_ok += 1,
            (false, true) => self.nofit_ok += 1,
            (false, false) => self.nofit_not_ok += 1,
        }
    }
    pub fn total(&self) -> usize {
        self.fit_and_ok + self.fit_not_ok + self.nofit_ok + self.nofit_not_ok
    }
    /// Share of legs on which the two answers agree, in percent.
    pub fn agreement_pct(&self) -> f64 {
        if self.total() == 0 {
            100.0
        } else {
            100.0 * (self.fit_and_ok + self.nofit_not_ok) as f64 / self.total() as f64
        }
    }
}

/// P1 "fits" for a leg: every sleeve triangle allows the layer and every cut has room for the
/// wire alone on that layer. Returns the reason when not.
pub fn p1_fits(pr: &Projection, leg: &Leg) -> Result<(), String> {
    let me = Item { net: leg.net, w: leg.w, s: leg.s };
    for &t in &leg.tris {
        match pr.allowed(t, leg.net) {
            Some(m) if m.has(leg.layer) => {}
            _ => return Err(format!("layer {} not allowed in triangle {t}", leg.layer)),
        }
    }
    for &e in &leg.edges {
        let need = pr.edge_demand(e, &[me]);
        let room = pr.room_for(e, leg.layer, leg.net);
        if need > room {
            let [p, q] = pr.edges[e];
            return Err(format!("cut {:.3} mm needs {:.3} mm at ({:.2}, {:.2})", room / 1e6, need / 1e6, (pr.pos[p].x + pr.pos[q].x) / 2e6, -(pr.pos[p].y + pr.pos[q].y) / 2e6));
        }
    }
    Ok(())
}

/// Portals `(left, right)` seen in the direction of travel of the strip `tris` / `edges`.
pub fn portals(pr: &Projection, leg: &Leg) -> Vec<(Vec2, Vec2)> {
    leg.edges
        .iter()
        .enumerate()
        .map(|(k, &e)| {
            // Leaving triangle k through side i (CCW v_i -> v_i+1): v_i+1 is on the left.
            let t = leg.tris[k];
            let i = pr.side(t, e);
            (pr.pos[pr.tris[t][(i + 1) % 3]], pr.pos[pr.tris[t][i]])
        })
        .collect()
}

struct LegEnv<'a> {
    set: &'a ObstacleSet,
    layer: u32,
    net: i32,
    w: f64,
}

impl Env for LegEnv<'_> {
    fn layer(&self) -> &LayerObstacles {
        self.set.layer(self.layer)
    }
    fn rho_anchor(&self, a: usize) -> f64 {
        self.set.rho_anchor(self.layer, a, self.net, self.w)
    }
    fn rho_wall(&self, w: usize) -> f64 {
        self.set.rho_wall(self.layer, w, self.net, self.w)
    }
    fn exempt_anchor(&self, a: usize) -> bool {
        let o = &self.set.layer(self.layer).anchors[a];
        ObstacleSet::exempt(o.net, &o.tie_nets, self.net)
    }
    fn exempt_wall(&self, w: usize) -> bool {
        let o = &self.set.layer(self.layer).walls[w];
        ObstacleSet::exempt(o.net, &o.tie_nets, self.net)
    }
    fn dyn_near(&self, _bb: Aabb) -> Vec<&DynObstacle> {
        Vec::new()
    }
    fn dyn_clearance(&self, _d: &DynObstacle) -> f64 {
        0.0
    }
    fn stack_radius(&self, a: AnchorRef) -> f64 {
        self.rho_anchor(a.anchor)
    }
    fn query_grow(&self) -> f64 {
        self.set.query_grow()
    }
}

/// The obstacles and the checker of one board (tracks and vias stripped). The engine's default
/// margin (5 um) is used by `taut` and the checker alike, as everywhere in the engine: a drawing
/// that touches the rule exactly would fail the checker by rounding.
pub struct Metrizer {
    set: ObstacleSet,
    checker: Checker,
    /// Model layer id of each signal layer index.
    layer_ids: Vec<u32>,
}

impl Metrizer {
    pub fn new(stripped: &BoardModel, layer_ids: Vec<u32>) -> Metrizer {
        let opts = Options::default();
        Metrizer { set: ObstacleSet::build(stripped, &opts, &HashSet::new(), &[]), checker: Checker::new(stripped, &opts), layer_ids }
    }

    /// The anchor at CDT vertex `v` on the leg's layer: the one whose circle the vertex lies on
    /// (round pads are one anchor, polygon vertices are corner anchors of radius 0), skipping the
    /// leg's own-net copper.
    fn anchor_at(&self, layer: u32, v: Vec2, net: i32) -> Option<usize> {
        let lo = self.set.layer(layer);
        let tol = 2_000.0;
        let bb = Aabb { min: Vec2 { x: v.x - 1e6, y: v.y - 1e6 }, max: Vec2 { x: v.x + 1e6, y: v.y + 1e6 } };
        let mut best: Option<(f64, usize)> = None;
        for a in lo.query_anchors(bb) {
            let an = &lo.anchors[a];
            if !an.enabled || ObstacleSet::exempt_anchor(an, net) {
                continue;
            }
            let off = ((v - an.c).len() - an.r_core).abs();
            if off <= tol && best.is_none_or(|b| off < b.0) {
                best = Some((off, a));
            }
        }
        best.map(|b| b.1)
    }

    /// Draw `leg` taut and judge it with the checker: `Ok(length in nm)` or why not, and the
    /// points of the chain when there is one.
    pub fn draw(&self, pr: &Projection, leg: &Leg) -> (Result<f64, String>, Vec<Vec2>) {
        let layer = self.layer_ids[leg.layer];
        let ps = portals(pr, leg);
        let mut seq: Vec<AnchorRef> = Vec::new();
        for (k, is_left, sigma) in funnel(leg.start, leg.goal, &ps) {
            let v = if is_left { ps[k].0 } else { ps[k].1 };
            if let Some(a) = self.anchor_at(layer, v, leg.net) {
                let r = AnchorRef { anchor: a, sigma };
                if seq.last() != Some(&r) {
                    seq.push(r);
                }
            }
        }
        let env = LegEnv { set: &self.set, layer, net: leg.net, w: leg.w };
        let chain = match tauten(leg.start, leg.goal, seq, &env) {
            Ok(c) => c,
            Err(e) => return (Err(format!("taut: {e:?}")), Vec::new()),
        };
        let mut path = vec![leg.start];
        for p in &chain.prims {
            match p {
                Prim::Seg(sg) => path.push(sg.b),
                Prim::Arc(a) => path.extend(a.split_sagitta(20_000.0).iter().map(|x| x.end())),
            }
        }
        let items: Vec<OutItem> = quantise_chain(&chain.prims)
            .into_iter()
            .map(|prim| OutItem::Track(TrackItem { net: leg.net, layer, width: leg.w as i64, prim: flip_track(prim), group: None, wire: None }))
            .collect();
        let rep = self.checker.check_items(&items);
        // taut stops exactly at the rule (+ margin), so the quantised drawing may miss it by
        // rounding: only a shortfall above `TOL_NM` is a violation.
        if let Some(v) = rep.violations.iter().find(|v| (v.required - v.gap) as f64 > TOL_NM) {
            return (Err(format!("checker: {} gap {} nm < {} nm on layer {}", v.against, v.gap, v.required, v.layer)), path);
        }
        (Ok(chain.length), path)
    }

    /// Both answers for `leg`.
    pub fn outcome(&self, pr: &Projection, leg: &Leg) -> LegOutcome {
        let fit = p1_fits(pr, leg);
        let (drawn, path) = self.draw(pr, leg);
        let k = |p: Vec2| [p.x, -p.y];
        LegOutcome {
            fits: fit.is_ok(),
            fit_why: fit.err().unwrap_or_default(),
            taut_ok: drawn.is_ok(),
            taut_why: drawn.as_ref().err().cloned().unwrap_or_default(),
            net: leg.net,
            layer: leg.layer,
            at: [leg.start.x, -leg.start.y],
            len_mm: drawn.unwrap_or(0.0) / 1e6,
            sleeve: leg.tris.iter().map(|&t| pr.tris[t].map(|v| k(pr.pos[v]))).collect(),
            path: path.into_iter().map(k).collect(),
        }
    }
}
