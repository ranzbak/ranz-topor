//! G0 flat-plane gate: build the projection, route every connection in one plane (P2-lite),
//! remove double crossings, assign two layers (P3) and measure against the hand routing.
//! Output points are in the KiCad frame (Y down, nm), ready for `harness.g0 render`.

use super::board::{self, BuildOpts};
use super::factors::{Event, FactorConfig, Registry, WireCtx, standard_factors};
use super::layers::{self, LayerParams};
use super::state::State;
use crate::geom::Vec2;
use crate::model::BoardModel;
use serde::Serialize;
use std::time::Instant;

#[derive(Clone, Debug, Default, Serialize)]
pub struct G0Summary {
    pub board: String,
    pub nets: usize,
    pub poured_nets: usize,
    pub connections: usize,
    pub routed: usize,
    pub failed: usize,
    pub overflows: usize,
    pub crossings_before: usize,
    pub crossings_after: usize,
    pub swaps: usize,
    pub domains: usize,
    pub graph_nodes: usize,
    pub graph_edges: usize,
    pub reduced_nodes: usize,
    pub reduced_edges: usize,
    /// 1 − (reduced nodes + edges) / (nodes + edges), in %.
    pub reduction_pct: f64,
    pub vias: usize,
    pub vias_no_site: usize,
    pub unlayerable: usize,
    pub same_layer: usize,
    /// Wire length over pad-to-pad distance: median, p90, max, and wires above 1.5.
    pub detour_median: f64,
    pub detour_p90: f64,
    pub detour_max: f64,
    pub detours_over_1_5: usize,
    /// SMD pads of nets poured only on non-signal layers: one stub via each (not wired).
    pub plane_stub_vias: usize,
    pub hand_vias: usize,
    /// Hand vias not on a poured net (stitching and plane vias excluded).
    pub hand_signal_vias: usize,
    pub tris: usize,
    pub build_s: f64,
    pub p2_s: f64,
    pub p3_s: f64,
    pub runtime_s: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct G0Stretch {
    pub layer: i32,
    pub pts: Vec<[f64; 2]>,
}

#[derive(Clone, Debug, Serialize)]
pub struct G0Wire {
    pub net: i32,
    pub stretches: Vec<G0Stretch>,
}

#[derive(Clone, Debug, Serialize)]
pub struct G0Result {
    pub summary: G0Summary,
    pub wires: Vec<G0Wire>,
    pub vias: Vec<[f64; 2]>,
    pub errors: Vec<[f64; 2]>,
}

fn kicad(p: Vec2) -> [f64; 2] {
    [p.x, -p.y]
}

#[derive(Clone, Debug)]
pub struct G0Opts {
    pub build: BuildOpts,
    /// Weights, scopes and report levels of the factors (`topor.toml [factors]`).
    pub factors: FactorConfig,
    /// Search cap per connection.
    pub max_expansions: usize,
    pub layers: LayerParams,
}

impl Default for G0Opts {
    fn default() -> Self {
        G0Opts { build: BuildOpts::default(), factors: FactorConfig::default(), max_expansions: 2_000_000, layers: LayerParams::default() }
    }
}

/// Run the gate on `input` (tracks and vias removed); `hand` is the untouched board.
pub fn run(input: &BoardModel, hand: Option<&BoardModel>, o: &G0Opts) -> Result<G0Result, String> {
    let t0 = Instant::now();
    let b = board::build(input, &o.build)?;
    if b.proj.n_layers != 2 {
        return Err(format!("G0 assigns 2 signal layers, the board has {}", b.proj.n_layers));
    }
    let conns = b.connections(input);
    let build_s = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    let mut st = State::new(&b.proj);
    let ws0 = (input.rules.default.width + input.rules.default.clearance) as f64;
    let (reg, problems) = Registry::from_config(standard_factors(), &o.factors);
    if let Some(p) = problems.first() {
        return Err(p.clone());
    }
    let rep = super::init::route_all_reg(&mut st, &conns, &reg, o.max_expansions, ws0);
    st.check()?;
    let p2_s = t1.elapsed().as_secs_f64();
    let t2 = Instant::now();
    let mut lp = o.layers;
    lp.via_mm = reg.price(&Event::Via { wire: 0, count: 1, at: None }, &WireCtx::default());
    lp.via_site_r = 0.5 * input.rules.default.via_d as f64 + input.rules.default.clearance as f64;
    let lr = layers::assign(&st, &|ob| b.smd_layer(ob), &lp);
    let p3_s = t2.elapsed().as_secs_f64();
    let nets: std::collections::BTreeSet<i32> = conns.iter().map(|c| c.net).collect();
    let (gn, gr) = (lr.before.nodes + lr.before.edges, lr.after.nodes + lr.after.edges);
    let mut ratios: Vec<f64> = (0..st.wires.len())
        .filter(|&w| st.wires[w].alive)
        .map(|w| {
            let pts = st.polyline(w);
            let len: f64 = pts.windows(2).map(|x| (x[1] - x[0]).len()).sum();
            len / (pts[pts.len() - 1] - pts[0]).len().max(1.0)
        })
        .collect();
    ratios.sort_by(f64::total_cmp);
    let q = |f: f64| ratios.get(((ratios.len() as f64 - 1.0) * f).round() as usize).copied().unwrap_or(1.0);
    let signal: Vec<u32> = b.signal_layers.clone();
    let plane_nets: std::collections::BTreeSet<i32> = input
        .pours
        .iter()
        .filter(|p| !p.layers.iter().any(|l| signal.contains(l)))
        .map(|p| p.net)
        .collect();
    let plane_stub_vias = b
        .proj
        .obstacles
        .iter()
        .filter(|ob| !ob.drilled && ob.net.is_some_and(|n| plane_nets.contains(&n)))
        .count();
    let summary = G0Summary {
        detour_median: q(0.5),
        detour_p90: q(0.9),
        detour_max: q(1.0),
        detours_over_1_5: ratios.iter().filter(|&&r| r > 1.5).count(),
        plane_stub_vias,
        board: input.board.clone(),
        nets: nets.len(),
        poured_nets: b.poured.len(),
        connections: conns.len(),
        routed: rep.routed,
        failed: rep.failed.len(),
        overflows: rep.overflows,
        crossings_before: rep.crossings_before,
        crossings_after: rep.crossings_after,
        swaps: rep.swaps,
        domains: lr.domains,
        graph_nodes: lr.before.nodes,
        graph_edges: lr.before.edges,
        reduced_nodes: lr.after.nodes,
        reduced_edges: lr.after.edges,
        reduction_pct: if gn > 0 { 100.0 * (1.0 - gr as f64 / gn as f64) } else { 100.0 },
        vias: lr.vias,
        vias_no_site: lr.vias_no_site,
        unlayerable: lr.unlayerable.len(),
        same_layer: lr.same_layer,
        hand_vias: hand.map_or(0, |h| h.vias.len()),
        hand_signal_vias: hand.map_or(0, |h| h.vias.iter().filter(|v| !b.poured.contains(&v.net)).count()),
        tris: b.proj.tris.len(),
        build_s,
        p2_s,
        p3_s,
        runtime_s: t0.elapsed().as_secs_f64(),
    };
    let wires = st
        .wires
        .iter()
        .zip(&lr.stretches)
        .filter(|(w, _)| w.alive)
        .map(|(w, s)| G0Wire {
            net: w.net,
            stretches: s
                .iter()
                .map(|(l, pts)| G0Stretch { layer: *l, pts: pts.iter().map(|&p| kicad(p)).collect() })
                .collect(),
        })
        .collect();
    let mut errors: Vec<[f64; 2]> = lr.unlayerable.iter().map(|&p| kicad(p)).collect();
    errors.extend(rep.failed.iter().map(|&i| kicad(b.proj.pos[conns[i].a])));
    Ok(G0Result { summary, wires, vias: lr.via_pts.iter().map(|&p| kicad(p)).collect(), errors })
}
