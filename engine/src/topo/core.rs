//! The `route()` entry of the topological core, stage S2a ("first copper", two signal layers):
//! BoardModel -> projection (`board`) -> P2 initial routing (`init`, pad to pad, widest first)
//! -> P3 layer assignment (`layers`, Polubasov) -> minimal drawing (`draw`) -> `RouteResult`.
//!
//! Every connection ends in one of three states, none of them silent:
//! * routed: copper emitted, checked by the [`Checker`] against the board and all earlier copper;
//! * failed with a reason (P7 wording): no topological path, a crossing that cannot be layered,
//!   no via site, a drawing the checker rejects, a plane pad without a via site, an unsupported
//!   terminal, a differential pair ("not in the rough version"), time limit;
//! * never emitted: a different-net crossing on one layer (the lower-tier wire stays unrouted).
//!
//! Boards with other than two signal layers (S2b) report every connection failed with a reason.
//! Deterministic: no randomness besides the seeded P3 local search, Vec + index ids throughout.
//!
//! Complexity: dominated by P2 (one A* per connection over the gap graph, capped by
//! `max_expansions`) and P3 (reductions linear in the domain graph, local search on the rest).

use super::board::{self, BuildOpts};
use super::draw::{plan, Drawer, Plan};
use super::factors::{standard_factors, Event, Finding, Registry, Report, WireCtx};
use super::init::{Conn, InitWeights};
use super::layers::{self, LayerParams};
use super::planes;
use super::state::{ObsId, State, WireId};
use crate::check::Checker;
use crate::control::{cancelled, RunControl};
use crate::decompose::{decompose, Connection, TermKind};
use crate::geom::Vec2;
use crate::model::{BoardModel, Item, Options};
use crate::route::{kicad, term_out, unrouted, ConnReport, NetRouteReport, RipReport, RouteResult};
use crate::vias::via_sizes;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

const MM: f64 = 1e6;

/// Failure reason of a differential pair (counted separately in the gate).
pub const DIFF_PAIR: &str = "diff pair: not in the rough version";

/// Nets that are halves of a differential pair by name (`_P`/`_N`, `+`/`-`, `P`/`N` suffix; both
/// halves present): the plugin's own rule (`priority._pair_key`). O(N log N).
pub fn diff_pair_nets(model: &BoardModel) -> BTreeSet<i32> {
    let key = |name: &str| -> Option<(String, u8)> {
        let seg = name.rsplit('/').next().unwrap_or(name);
        let mut s = seg.to_string();
        if let Some(at) = s.find('@') {
            let digits = s[at + 1..].chars().take_while(char::is_ascii_digit).count();
            s.replace_range(at..at + 1 + digits, "");
        }
        for (pos, neg) in [("_P", "_N"), ("+", "-"), ("P", "N")] {
            if let Some(k) = s.strip_suffix(pos) {
                return Some((k.to_string(), 0));
            }
            if let Some(k) = s.strip_suffix(neg) {
                return Some((k.to_string(), 1));
            }
        }
        None
    };
    let mut by: BTreeMap<String, [Option<i32>; 2]> = BTreeMap::new();
    for n in &model.nets {
        if let Some((k, side)) = key(&n.name) {
            by.entry(k).or_default()[side as usize] = Some(n.code);
        }
    }
    by.values().filter_map(|h| h[0].zip(h[1])).flat_map(|(a, b)| [a, b]).collect()
}

/// What happened to one connection.
#[derive(Clone, Debug, Default)]
struct Outcome {
    routed: bool,
    reason: String,
    items: Vec<Item>,
    vias: Vec<(Vec2, crate::vias::ViaSize)>,
    layers: Vec<u32>,
    length_nm: f64,
    stub_nm: f64,
    /// Necked pad exits: total length (nm) and the narrowest width (nm).
    neck_nm: f64,
    neck_w: Option<f64>,
    /// Straight pad-to-pad distance (nm), the detour reference.
    ghost_nm: f64,
}

fn fail(o: &mut [Outcome], i: usize, reason: impl Into<String>) {
    if !o[i].routed && !o[i].reason.is_empty() {
        return;
    }
    o[i].routed = false;
    o[i].reason = reason.into();
}

/// Route `model` with the topological core (see the module doc).
pub fn route_topo(model: &BoardModel, opts: &Options, ctl: &RunControl) -> RouteResult {
    let t0 = Instant::now();
    let deadline = (opts.wall_clock_s > 0.0).then(|| t0 + std::time::Duration::from_secs_f64(opts.wall_clock_s));
    let expired = || cancelled() || ctl.is_cancelled() || deadline.is_some_and(|d| Instant::now() >= d);
    let signal: Vec<u32> = model.layers.iter().filter(|l| l.routable).map(|l| l.id).collect();
    if signal.len() != 2 {
        let why = format!(
            "the topological core (S2a) routes boards with two signal layers; this board has {} (S2b: three or more)",
            signal.len()
        );
        return unrouted(model, opts, &why);
    }
    ctl.update("setup", 0, 0, 0, 0, 0.0);
    let dec = decompose(model, opts, &signal);
    let mut warnings: Vec<String> = dec.warnings.clone();
    for (net, r) in &dec.skipped {
        warnings.push(format!("net {net} skipped: {r}"));
    }
    let (reg, problems) = Registry::from_config(standard_factors(), &opts.factors);
    warnings.extend(problems);
    let b = match board::build(model, &BuildOpts::default()) {
        Ok(b) => b,
        Err(e) => return unrouted(model, opts, &format!("projection failed: {e}")),
    };
    let nconn = dec.connections.len();
    let mut out: Vec<Outcome> = vec![Outcome::default(); nconn];
    let diff = diff_pair_nets(model);
    let mut obs_of_pad: BTreeMap<usize, ObsId> = BTreeMap::new();
    for (o, &p) in b.pad_of.iter().enumerate() {
        obs_of_pad.insert(p, o);
    }
    // Sort the connections: pad-to-pad ones into P2, pour ones into the plane stage.
    let mut conns: Vec<Conn> = Vec::new();
    let mut conn_of: Vec<usize> = Vec::new();
    let mut plane_reqs: Vec<planes::Req> = Vec::new();
    for c in &dec.connections {
        let i = c.id;
        let (ta, tb) = (&c.from.kind, &c.to.kind);
        out[i].ghost_nm = (c.from.at - c.to.at).len();
        if diff.contains(&c.net) {
            fail(&mut out, i, DIFF_PAIR);
            continue;
        }
        match (ta, tb) {
            (TermKind::Pad(pa), TermKind::Pad(pb)) => {
                let (Some(&oa), Some(&ob)) = (obs_of_pad.get(pa), obs_of_pad.get(pb)) else {
                    fail(&mut out, i, "a pad has no copper on a signal layer");
                    continue;
                };
                let (Some(va), Some(vb)) = (b.centre[oa], b.centre[ob]) else {
                    fail(&mut out, i, "a pad has no net centre vertex");
                    continue;
                };
                conn_of.push(i);
                conns.push(Conn { net: c.net, pa: oa, pb: ob, a: va, b: vb, w: c.width as f64, s: c.clearance as f64 });
            }
            (TermKind::Pad(pa), TermKind::Pour(zi)) => match obs_of_pad.get(pa) {
                Some(&oa) => plane_reqs.push(planes::Req {
                    conn: i,
                    pad: *pa,
                    obs: oa,
                    net: c.net,
                    w: c.width as f64,
                    pour: *zi,
                }),
                None => fail(&mut out, i, "a pad has no copper on a signal layer"),
            },
            _ => fail(
                &mut out,
                i,
                "terminal is an existing via, track end or stub: not supported by the rough topological core (S2a)",
            ),
        }
    }
    ctl.update("route", 0, 0, nconn, 0, t0.elapsed().as_secs_f64());
    // ---- P2 ----
    let proj = &b.proj;
    let mut st = State::new(proj);
    let ws0 = (model.rules.default.width + model.rules.default.clearance) as f64;
    let tier_of = |i: usize| dec.connections[i].tier;
    let weights = |c: &Conn, tier: u8, cap: usize| InitWeights { max_expansions: cap, ..InitWeights::from_registry(&reg, &WireCtx { net: c.net, tier }) };
    let len_of = |c: &Conn| (proj.pos[c.b] - proj.pos[c.a]).len();
    let mut order: Vec<usize> = (0..conns.len()).collect();
    order.sort_by(|&i, &j| {
        conns[j].w.total_cmp(&conns[i].w).then(len_of(&conns[i]).total_cmp(&len_of(&conns[j]))).then(i.cmp(&j))
    });
    let mut wire_conn: Vec<usize> = Vec::new(); // wire id -> index into conns
    let mut p2_failed: Vec<usize> = Vec::new();
    let mut timed_out = false;
    let route_one = |st: &mut State, ci: usize, cap: usize, wire_conn: &mut Vec<usize>| -> bool {
        let c = &conns[ci];
        let w = weights(c, tier_of(conn_of[ci]), cap);
        match st.route_conn(c, &w, (c.w + c.s) / ws0) {
            Some((wid, _)) => {
                debug_assert_eq!(wid, wire_conn.len());
                wire_conn.push(ci);
                true
            }
            None => false,
        }
    };
    let cap = 2_000_000usize;
    for &ci in &order {
        if expired() {
            timed_out = true;
            fail(&mut out, conn_of[ci], "time limit reached before this connection was routed");
            continue;
        }
        if !route_one(&mut st, ci, cap, &mut wire_conn) {
            p2_failed.push(ci);
        }
    }
    let swaps = st.uncross_all(50);
    // One retry of the failed connections with a larger search cap.
    let mut still: Vec<usize> = Vec::new();
    for &ci in &p2_failed {
        if expired() || !route_one(&mut st, ci, cap * 4, &mut wire_conn) {
            still.push(ci);
        }
    }
    if still.len() < p2_failed.len() {
        st.uncross_all(50);
    }
    for &ci in &still {
        fail(&mut out, conn_of[ci], "no topological path between the pads (search exhausted); the pad is enclosed or the retry cap was reached");
    }
    debug_assert!(st.check().is_ok(), "{:?}", st.check());
    ctl.update("route", 1, 0, nconn, 0, t0.elapsed().as_secs_f64());
    // ---- P3 ----
    let lp = LayerParams {
        via_mm: reg.price(&Event::Via { wire: 0, count: 1, at: None }, &WireCtx::default()),
        via_site_r: 0.5 * model.rules.default.via_d as f64 + model.rules.default.clearance as f64,
        ..LayerParams::default()
    };
    let lr = layers::assign(&st, &|ob| b.smd_layer(ob), &lp);
    // A different-net crossing still on one layer is an error: the lower-tier wire stays unrouted.
    let wlen: Vec<f64> = (0..st.wires.len()).map(|w| st.polyline(w).windows(2).map(|x| (x[1] - x[0]).len()).sum::<f64>()).collect();
    let wire_len = |w: WireId| wlen[w];
    let mut dropped: BTreeSet<WireId> = BTreeSet::new();
    let mut unlayerable_errors = 0usize;
    for &(wa, wb) in &lr.unlayerable_pairs {
        if dropped.contains(&wa) || dropped.contains(&wb) {
            continue;
        }
        unlayerable_errors += 1;
        let (ca, cb) = (conn_of[wire_conn[wa]], conn_of[wire_conn[wb]]);
        let key = |w: WireId, c: usize| (tier_of(c), std::cmp::Reverse((wire_len(w) * 1e3) as i64), w);
        let victim = if key(wa, ca) >= key(wb, cb) { wa } else { wb };
        let other = if victim == wa { wb } else { wa };
        let cv = conn_of[wire_conn[victim]];
        let name = |w: WireId| model.nets.iter().find(|n| n.code == st.wires[w].net).map_or(String::new(), |n| n.name.clone());
        let at = lr
            .unlayerable
            .first()
            .map_or(String::new(), |p| format!(" near ({:.3}, {:.3}) mm", p.x / MM, -p.y / MM));
        let msg = format!(
            "crossing of {} and {}{at} cannot be layered: both stretches are fixed to the same layer (error, not emitted)",
            name(victim),
            name(other)
        );
        warnings.push(msg.clone());
        fail(&mut out, cv, msg);
        dropped.insert(victim);
    }
    for &w in &dropped {
        st.remove(w);
    }
    ctl.update("route", 2, 0, nconn, lr.vias, t0.elapsed().as_secs_f64());
    // ---- Drawing ----
    let checker = Checker::new(model, opts);
    let mut dr = Drawer::new(model, opts, &b, &st, &checker);
    let mut plans: BTreeMap<WireId, Plan> = BTreeMap::new();
    for w in 0..st.wires.len() {
        if st.wires[w].alive {
            plans.insert(w, plan(&lr.events[w], proj.n_layers));
        }
    }
    // Draw order: widest first, then shortest.
    let mut draw_order: Vec<WireId> = plans.keys().copied().collect();
    draw_order.sort_by(|&x, &y| {
        st.wires[y].w.total_cmp(&st.wires[x].w).then(wire_len(x).total_cmp(&wire_len(y))).then(x.cmp(&y))
    });
    // Plane stubs and vias go right after the wide wires: before the first default-width wire.
    let wide = draw_order.iter().take_while(|&&w| st.wires[w].w > model.rules.default.width as f64).count();
    // Phase 1: the vias of every wire (they are obstacles for the wires drawn next).
    let mut placed: BTreeMap<WireId, Vec<(usize, Vec2, crate::vias::ViaSize)>> = BTreeMap::new();
    let mut via_ids: BTreeMap<WireId, Vec<usize>> = BTreeMap::new();
    let mut live: Vec<WireId> = Vec::new();
    for &w in &draw_order {
        if expired() {
            timed_out = true;
            fail(&mut out, conn_of[wire_conn[w]], "time limit reached before this connection was drawn");
            continue;
        }
        let mut ids = Vec::new();
        match dr.place_vias(w, &plans[&w], &mut ids) {
            Ok(v) => {
                placed.insert(w, v);
                via_ids.insert(w, ids);
                live.push(w);
            }
            Err(r) => {
                for id in ids {
                    dr.uncommit(id);
                }
                fail(&mut out, conn_of[wire_conn[w]], r);
            }
        }
    }
    // Pass layers and intervals from the placed vias.
    for &w in &live {
        let (pl, vs) = (&plans[&w], &placed[&w]);
        let n = st.wires[w].passes.len();
        for r in 0..pl.layers.len() {
            let k0 = if r == 0 { 0 } else { vs[r - 1].0 };
            let k1 = if r == pl.layers.len() - 1 { n } else { vs[r].0 };
            for k in k0..k1 {
                dr.pass_layer[st.wires[w].passes[k]] = pl.layers[r] as i32;
            }
        }
    }
    dr.compute_intervals();
    // Phase 2: plane stubs, then the wires. Plane pads go before the first default-width wire.
    let mut plane_res: Vec<(usize, Result<planes::Out, String>)> = Vec::new();
    let mut plane_done = false;
    let run_planes = |dr: &mut Drawer, out: &mut Vec<Outcome>, plane_res: &mut Vec<(usize, Result<planes::Out, String>)>| {
        for rq in &plane_reqs {
            let r = planes::place(dr, rq);
            if let Ok(o) = &r {
                out[rq.conn].routed = true;
                out[rq.conn].items = o.items.clone();
                out[rq.conn].vias = vec![(o.via, o.size)];
                out[rq.conn].layers = vec![o.layer];
                out[rq.conn].length_nm = o.stub_nm;
                out[rq.conn].stub_nm = o.stub_nm;
            } else if let Err(e) = &r {
                fail(out, rq.conn, e.clone());
            }
            plane_res.push((rq.conn, r));
        }
    };
    let mut drawn_ok = 0usize;
    let mut necking: Vec<(usize, Event)> = Vec::new();
    for (pos, &w) in live.iter().enumerate() {
        if pos == wide.min(live.len()) && !plane_done {
            run_planes(&mut dr, &mut out, &mut plane_res);
            plane_done = true;
        }
        if expired() {
            timed_out = true;
            fail(&mut out, conn_of[wire_conn[w]], "time limit reached before this connection was drawn");
            continue;
        }
        let ci = conn_of[wire_conn[w]];
        let mut ids = via_ids.remove(&w).unwrap_or_default();
        let wo = dr.draw_wire(w, &plans[&w], &placed[&w], &mut ids);
        if wo.ok {
            drawn_ok += 1;
            let o = &mut out[ci];
            o.routed = true;
            o.length_nm = wo.length_nm;
            o.layers = wo.runs.iter().map(|(l, _)| b.signal_layers[*l]).collect();
            o.layers.dedup();
            o.vias = wo.vias.clone();
            o.items = wo.items;
            o.neck_nm = wo.necks.iter().map(|n| n.2).sum();
            o.neck_w = wo.necks.iter().map(|n| n.1).fold(None, |a: Option<f64>, x| Some(a.map_or(x, |y| y.min(x))));
            for n in &wo.necks {
                necking.push((ci, Event::Necking { wire: w, from_nm: st.wires[w].w, to_nm: n.1, mm: n.2 / MM }));
            }
        } else {
            for id in ids {
                dr.uncommit(id);
            }
            fail(&mut out, ci, wo.reason);
        }
        if drawn_ok.is_multiple_of(50) {
            ctl.update("route", 3, drawn_ok, nconn, 0, t0.elapsed().as_secs_f64());
        }
    }
    if !plane_done {
        run_planes(&mut dr, &mut out, &mut plane_res);
    }
    drop(dr);
    let isl = planes::islands(model, &b, &out_vias(&out, &plane_reqs, &dec.connections), &plane_reqs);
    warnings.extend(isl);
    // ---- Assemble the result ----
    let first_layer = signal[0];
    let name_of = |net: i32| model.nets.iter().find(|n| n.code == net).map_or(String::new(), |n| n.name.clone());
    let layer_name = |id: u32| model.layers.iter().find(|l| l.id == id).map_or(id.to_string(), |l| l.name.clone());
    let mut items: Vec<Item> = Vec::new();
    let mut item_conn: Vec<usize> = Vec::new();
    let mut reports: Vec<ConnReport> = Vec::with_capacity(nconn);
    let mut length_by_layer: BTreeMap<String, i64> = BTreeMap::new();
    let (mut tot_len, mut tot_vias) = (0i64, 0usize);
    for c in &dec.connections {
        let o = &out[c.id];
        let mut r = ConnReport::blank(
            c.id,
            c.net,
            name_of(c.net),
            first_layer,
            c.width,
            if o.routed { "routed" } else { "failed" },
            if o.routed { "" } else { &o.reason },
            term_out(&c.from),
            term_out(&c.to),
        );
        if o.routed {
            r.vias = o.vias.len();
            r.layers = o.layers.iter().map(|&l| layer_name(l)).collect();
            r.layer = o.layers.first().copied().unwrap_or(first_layer);
            r.via_sites = o.vias.iter().map(|v| kicad(v.0)).collect();
            r.length_nm = o.length_nm.round() as i64;
            r.length_outside_pads_nm = r.length_nm;
            r.stub_nm = o.stub_nm.round() as i64;
            r.neck_nm = o.neck_nm.round() as i64;
            r.neck_width_nm = o.neck_w.map(|x| x.round() as i64);
            r.ghost_len_nm = Some(o.ghost_nm.round() as i64);
            r.pattern = if o.vias.is_empty() { "topo".into() } else { "topo+via".into() };
            tot_len += r.length_nm;
            tot_vias += r.vias;
            for it in &o.items {
                if let Item::Track(t) = it {
                    let len = match t.prim {
                        crate::geom::quant::TrackPrim::Seg { a, b } => ((b.x - a.x) as f64).hypot((b.y - a.y) as f64),
                        _ => 0.0,
                    };
                    *length_by_layer.entry(layer_name(t.layer)).or_insert(0) += len.round() as i64;
                }
            }
            for it in &o.items {
                items.push(it.clone());
                item_conn.push(c.id);
            }
        }
        reports.push(r);
    }
    let nets: Vec<NetRouteReport> = dec
        .nets
        .iter()
        .map(|n| {
            let count = n.connections.len();
            let routed = n.connections.iter().filter(|&&i| out[i].routed).count();
            let vias: usize = n.connections.iter().map(|&i| out[i].vias.len()).sum();
            let length: i64 = n.connections.iter().map(|&i| reports[i].length_nm).sum();
            let status = if n.excluded {
                "excluded"
            } else if n.pour_served {
                "pour_served"
            } else if !n.in_scope {
                "out_of_scope"
            } else if count == 0 {
                "complete"
            } else if routed == count {
                "routed"
            } else if routed == 0 {
                "failed"
            } else {
                "partial"
            };
            NetRouteReport {
                net: n.net,
                name: n.name.clone(),
                status: status.into(),
                connections: count,
                routed,
                partial: 0,
                failed: count - routed,
                vias,
                length_nm: length,
                length_outside_pads_nm: length,
            }
        })
        .collect();
    let routed_n = reports.iter().filter(|r| r.status == "routed").count();
    let nets_total = nets.iter().filter(|n| n.connections > 0).count();
    let nets_routed = nets.iter().filter(|n| n.connections > 0 && n.status == "routed").count();
    let diff_failed = reports.iter().filter(|r| r.reason == DIFF_PAIR).count();
    if diff_failed > 0 {
        warnings.push(format!("diff pairs skipped: {diff_failed} connections ({DIFF_PAIR})"));
    }
    if unlayerable_errors > 0 {
        warnings.push(format!("layering errors: {unlayerable_errors} different-net crossings cannot be layered (not emitted)"));
    }
    warnings.push(format!(
        "topological core S2a: {} wires in P2 ({} double-crossing swaps), P3 vias {}, {} triangles",
        st.wires.iter().filter(|w| w.alive).count() + dropped.len(),
        swaps,
        lr.vias,
        proj.tris.len()
    ));
    let mut findings: Vec<Finding> = Vec::new();
    for (ci, ev) in &necking {
        let ctx = WireCtx { net: dec.connections[*ci].net, tier: dec.connections[*ci].tier };
        for f in reg.findings(std::slice::from_ref(ev), &ctx) {
            findings.push(Finding { text: format!("{} ({}): {}", name_of(ctx.net), dec.connections[*ci].from.label, f.text), ..f });
        }
    }
    for f in findings.iter().filter(|f| f.level >= Report::Warning) {
        warnings.push(f.text.clone());
    }
    let stop = if cancelled() || ctl.is_cancelled() {
        "cancel"
    } else if timed_out {
        "timeout"
    } else {
        "converged"
    };
    ctl.update("done", 4, routed_n, nconn, tot_vias, t0.elapsed().as_secs_f64());
    RouteResult {
        items,
        connections_total: reports.len(),
        routed: routed_n,
        partial: 0,
        failed: reports.len() - routed_n,
        connections: reports,
        nets,
        nets_total,
        nets_routed,
        total_length_nm: tot_len,
        length_outside_pads_nm: tot_len,
        vias: tot_vias,
        length_by_layer_nm: length_by_layer,
        sites: 0,
        vias_in_pads: Vec::new(),
        via_reduction: "not run".into(),
        via_relaxation: "not run".into(),
        via_shove: "not run".into(),
        rip_pass: "not run".into(),
        ripup: RipReport::default(),
        iterations: 1,
        converged: !timed_out,
        overuse_edges: 0,
        clinches: Default::default(),
        relax_iterations: 0,
        self_check_rounds: 0,
        check_candidates_visited: 0,
        timed_out,
        stop_reason: stop.into(),
        runtime_s: t0.elapsed().as_secs_f64(),
        threads_used: 1,
        warnings,
        zero_slack_wires: Vec::new(),
        zero_slack: "not run".into(),
        profile: None,
        item_conn,
        stubs: Vec::new(),
    }
}

/// `(connection id, via centre)` of every routed plane connection (for the island check).
fn out_vias(out: &[Outcome], reqs: &[planes::Req], _conns: &[Connection]) -> Vec<(usize, Vec2)> {
    reqs.iter().filter(|r| out[r.conn].routed).filter_map(|r| out[r.conn].vias.first().map(|v| (r.conn, v.0))).collect()
}

/// Unused import guard for the via size lookup used by the plane stage.
#[allow(dead_code)]
fn _sizes(model: &BoardModel, opts: &Options, net: i32) -> Vec<crate::vias::ViaSize> {
    via_sizes(&model.rules, net, opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_pair_halves_by_name() {
        let mut m = BoardModel::default();
        for (i, n) in ["USB_D+", "USB_D-", "CLK_P", "CLK_N", "VCC", "ONLY_P"].iter().enumerate() {
            m.nets.push(serde_json::from_value(serde_json::json!({"code": i as i32 + 1, "name": n})).expect("net"));
        }
        let d = diff_pair_nets(&m);
        assert_eq!(d.into_iter().collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    }
}
