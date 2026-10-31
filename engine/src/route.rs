//! `route()` orchestration (M2 / T2.4a): decompose -> graph -> negotiation (slot search) ->
//! metrization hand-off to the M1 `relax` -> quantisation -> `Checker` self-check with
//! per-connection feedback -> `RouteResult`.
//!
//! M2 routes on one layer (F.Cu). Every PathFinder iteration searches the active connections,
//! sketches every routed slot path (`sketch_from_crossings`), relaxes all of them together,
//! quantises and self-checks; a connection failing relax or the self-check is penalised
//! (`penalise_failed`) and re-searched. A connection whose terminal lies inside a foreign
//! obstacle (`InsideObstacle`) is class-independent: it is failed once, removed from the graph
//! and never searched again (design note §4).
//! Caps: iterations `pathfinder_iter_cap`, self-check rounds <= routed connections + 1, the wall
//! clock is checked before every search (negotiate), relax call and self-check round. Output is
//! ordered by connection id; no `HashMap` iteration reaches it.

use crate::cdt::Crossing;
use crate::check::Checker;
use crate::decompose::{decompose, terminal_of, Connection, TermKind, TermRef};
use crate::geom::quant::{quantise_chain, TrackPrim};
use crate::geom::{dequantise, Prim, Skeleton, Vec2};
use crate::graph::{GraphRules, RouteGraph, TerminalInfo};
use crate::model::{flip_track, via_layers, BoardModel, ExistingCopper, Item, Options, TrackItem};
use crate::negotiate::{detect_clinches, overused_conns, ClinchReport, Negotiator};
use crate::obstacle::{ObstacleSet, TerminalObstacle};
use crate::relax::relax;
use crate::sketch::{
    build_layer_cdt, sketch_from_crossings, stack_order, unanchored_bends, AnchorRef,
    ConstraintSet, LayerSketch, Sketch,
};
use crate::smooth::{deadline_after, hand_outside, outside_len, terminal_skeletons};
use crate::taut::TautError;
use crate::wires::{extract_wires, Wire};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Instant;

#[derive(Clone, Debug, Serialize)]
pub struct TermOut {
    /// `pad`, `via`, `copper` or `dangling`.
    pub kind: String,
    pub label: String,
    /// KiCad frame, nm.
    pub at: [i64; 2],
}

#[derive(Clone, Debug, Serialize)]
pub struct ConnReport {
    pub id: usize,
    pub net: i32,
    pub name: String,
    pub layer: u32,
    pub width: i64,
    /// `routed`, `partial` or `failed`.
    pub status: String,
    pub reason: String,
    pub from: TermOut,
    pub to: TermOut,
    pub length_nm: i64,
    pub length_outside_pads_nm: i64,
    pub pattern: String,
    pub anchors: usize,
    pub crossings: usize,
    /// Where the connection was blocked (KiCad frame nm), when known.
    pub blocking: Option<[i64; 2]>,
    pub g1_angle_deg: Option<f64>,
    /// PathFinder iteration the report comes from.
    pub iteration: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct NetRouteReport {
    pub net: i32,
    pub name: String,
    /// `routed`, `partial`, `failed`, `pour_served`, `complete` or `out_of_scope`.
    pub status: String,
    pub connections: usize,
    pub routed: usize,
    pub partial: usize,
    pub failed: usize,
    pub length_nm: i64,
    pub length_outside_pads_nm: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RouteResult {
    pub items: Vec<Item>,
    pub connections: Vec<ConnReport>,
    pub nets: Vec<NetRouteReport>,
    pub connections_total: usize,
    pub routed: usize,
    pub partial: usize,
    pub failed: usize,
    pub nets_total: usize,
    pub nets_routed: usize,
    pub total_length_nm: i64,
    pub length_outside_pads_nm: i64,
    pub iterations: u32,
    pub converged: bool,
    pub overuse_edges: usize,
    pub clinches: ClinchReport,
    pub relax_iterations: u32,
    pub self_check_rounds: u32,
    pub timed_out: bool,
    pub runtime_s: f64,
    pub threads_used: usize,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireStats {
    pub wires: usize,
    pub skipped: usize,
    pub hand_length_nm: i64,
    pub hand_outside_pads_nm: i64,
}

/// Hand-copper statistics of the unripped board (the length gate's reference).
pub fn wire_stats(model: &BoardModel) -> WireStats {
    let smooth = Options {
        existing_copper: ExistingCopper::Smooth,
        ..Options::default()
    };
    let ex = extract_wires(model, &smooth);
    let (mut len, mut out) = (0.0, 0.0);
    for w in &ex.wires {
        len += w.hand_len;
        out += hand_outside(w, &terminal_skeletons(model, w));
    }
    WireStats {
        wires: ex.wires.len(),
        skipped: ex.skipped.len(),
        hand_length_nm: len.round() as i64,
        hand_outside_pads_nm: out.round() as i64,
    }
}

fn kicad(p: Vec2) -> [i64; 2] {
    [p.x.round() as i64, (-p.y).round() as i64]
}

fn term_out(t: &TermRef) -> TermOut {
    TermOut {
        kind: match t.kind {
            TermKind::Pad(_) => "pad",
            TermKind::Via(_) => "via",
            TermKind::Copper(_) => "copper",
            TermKind::Dangling(_) => "dangling",
        }
        .into(),
        label: t.label.clone(),
        at: kicad(t.at),
    }
}

fn bits(p: Vec2) -> (u64, u64) {
    ((p.x + 0.0).to_bits(), (p.y + 0.0).to_bits())
}

/// One routed connection after metrization and self-check.
#[derive(Clone, Debug)]
struct ConnOk {
    items: Vec<TrackItem>,
    len: f64,
    out: f64,
    pattern: String,
    anchors: usize,
    crossings: usize,
}

#[derive(Clone, Debug)]
enum Res {
    Ok(ConnOk),
    Fail {
        reason: String,
        /// Class-independent failure: never searched again.
        perm: bool,
        blocking: Option<[i64; 2]>,
    },
}

fn fail(reason: impl Into<String>) -> Res {
    Res::Fail {
        reason: reason.into(),
        perm: false,
        blocking: None,
    }
}

fn taut_reason(e: &TautError) -> Res {
    match e {
        TautError::Sleeve(s) => fail(format!("sleeve: {s}")),
        TautError::InsideObstacle(s) => Res::Fail {
            reason: format!("inside obstacle: {s}"),
            perm: true,
            blocking: None,
        },
        TautError::Infeasible(s) => fail(format!("infeasible: {s}")),
        TautError::Cap => fail("infeasible: pass cap"),
    }
}

struct Ctx<'a> {
    model: &'a BoardModel,
    opts: &'a Options,
    obs: &'a ObstacleSet,
    ls: &'a LayerSketch,
    conns: &'a [Connection],
    checker: &'a Checker,
    deadline: Instant,
}

struct Metrized {
    /// `(live connection index, result)` for every connection in `routed`.
    res: Vec<(usize, Res)>,
    relax_iterations: u32,
    rounds: u32,
    timed_out: bool,
}

/// Crossings of the live path of connection `c` in the direction of travel.
fn path_crossings(g: &RouteGraph, c: usize) -> Option<Vec<Crossing>> {
    let (cross, faces, _) = g.path_of(c)?;
    if cross.is_empty() {
        return Some(Vec::new());
    }
    if faces.len() != cross.len() + 1 {
        return None;
    }
    Some(
        cross
            .iter()
            .zip(faces)
            .map(|(&(e, slot), &prev)| {
                let ge = &g.edges[e];
                // Entering from the face left of a->b means travelling clockwise of a->b:
                // `b` is then on the left of the direction of travel.
                let (left, right) = if ge.faces[0] == Some(prev) {
                    (ge.b, ge.a)
                } else {
                    (ge.a, ge.b)
                };
                let from_a = (slot as f64 + 1.0) / (ge.cross.len() as f64 + 1.0);
                let t = if left == ge.a { from_a } else { 1.0 - from_a };
                Crossing { left, right, t }
            })
            .collect(),
    )
}

/// Sketch, relax, quantise and self-check the connections `routed` (live indices) of `g`.
fn metrize(cx: &Ctx, g: &RouteGraph, routed: &[usize]) -> Metrized {
    let mut out = Metrized {
        res: Vec::new(),
        relax_iterations: 0,
        rounds: 0,
        timed_out: false,
    };
    let mut wires: Vec<Wire> = Vec::new();
    let mut sketches: Vec<Sketch> = Vec::new();
    let mut live_of: Vec<usize> = Vec::new();
    let mut ncross: Vec<usize> = Vec::new();
    for &i in routed {
        let c = &cx.conns[i];
        let Some(path) = path_crossings(g, c.id) else {
            out.res.push((i, fail("sketch: inconsistent slot path")));
            continue;
        };
        let (start, goal) = (c.from.at, c.to.at);
        if let Some(p) = unanchored_bends(start, goal, &path, cx.ls).first() {
            out.res.push((
                i,
                fail(format!(
                    "sketch: path bends at a wall split vertex ({}, {})",
                    kicad(*p)[0],
                    kicad(*p)[1]
                )),
            ));
            continue;
        }
        let k = wires.len();
        let s = sketch_from_crossings(k, start, goal, path, cx.ls);
        ncross.push(s.crossings.len());
        sketches.push(s);
        wires.push(Wire {
            id: k,
            net: c.net,
            layer: c.layer,
            width: c.width,
            start: terminal_of(&c.from),
            goal: terminal_of(&c.to),
            hand: vec![start, goal],
            kept_uuids: Vec::new(),
            hand_len: c.est_len,
        });
        live_of.push(i);
    }
    let n = wires.len();
    if n == 0 {
        return out;
    }
    let mut orders: HashMap<u32, HashMap<AnchorRef, Vec<usize>>> = HashMap::new();
    for l in wires.iter().map(|w| w.layer).collect::<BTreeSet<_>>() {
        orders.insert(l, stack_order(&sketches, cx.ls));
    }
    if Instant::now() >= cx.deadline {
        out.timed_out = true;
        return out;
    }
    let r = relax(&wires, &sketches, cx.obs, &orders, cx.opts, cx.deadline);
    out.relax_iterations = r.iterations;
    if r.timed_out {
        out.timed_out = true;
        return out;
    }
    let mut results: Vec<Option<Res>> = vec![None; n];
    let mut emit: Vec<Vec<TrackItem>> = vec![Vec::new(); n];
    let mut chains = Vec::with_capacity(n);
    for (k, c) in r.chains.into_iter().enumerate() {
        match c {
            Ok(ch) => {
                let q = quantise_chain(&ch.prims);
                if q.is_empty() {
                    results[k] = Some(fail("empty chain after quantisation"));
                } else {
                    let w = &wires[k];
                    emit[k] = q
                        .into_iter()
                        .map(|t| TrackItem {
                            net: w.net,
                            layer: w.layer,
                            width: w.width,
                            prim: flip_track(t),
                            group: None,
                            wire: Some(live_of[k]),
                        })
                        .collect();
                }
                chains.push(Some(ch));
            }
            Err(e) => {
                results[k] = Some(taut_reason(&e));
                chains.push(None);
            }
        }
    }

    // Self-check rounds (fallback granularity = connection).
    loop {
        if cx.opts.skip_self_check || out.rounds as usize > n {
            break;
        }
        let mut items: Vec<Item> = Vec::new();
        let mut owner: Vec<usize> = Vec::new();
        for k in 0..n {
            if results[k].is_none() {
                for t in &emit[k] {
                    items.push(Item::Track(t.clone()));
                    owner.push(k);
                }
            }
        }
        if items.is_empty() {
            break;
        }
        if Instant::now() >= cx.deadline {
            out.timed_out = true;
            return out;
        }
        out.rounds += 1;
        let rep = cx.checker.check_items(&items);
        let mut any = false;
        for v in &rep.violations {
            let k = owner[v.item];
            if results[k].is_none() {
                results[k] = Some(fail(format!("self-check: {}", v.against)));
                any = true;
            }
        }
        if !any {
            break;
        }
    }

    for k in 0..n {
        let res = match results[k].take() {
            Some(r) => r,
            None => {
                let ch = chains[k].as_ref().expect("chain of a checked wire");
                let sks: Vec<Skeleton> = terminal_skeletons(cx.model, &wires[k]);
                let pattern: String = emit[k]
                    .iter()
                    .map(|t| match t.prim {
                        TrackPrim::Seg { .. } => 'S',
                        TrackPrim::Arc { .. } => 'A',
                    })
                    .collect();
                let len: f64 = emit[k].iter().map(|t| dequantise(&t.prim).length()).sum();
                Res::Ok(ConnOk {
                    items: std::mem::take(&mut emit[k]),
                    len,
                    out: outside_len(&ch.prims, &sks),
                    pattern,
                    anchors: ch.seq.len(),
                    crossings: ncross[k],
                })
            }
        };
        out.res.push((live_of[k], res));
    }
    out
}

#[derive(Clone)]
struct Snap {
    routed: usize,
    len: f64,
    iteration: u32,
    res: Vec<Res>,
    overuse_edges: usize,
    clinches: ClinchReport,
    relax_iterations: u32,
    rounds: u32,
}

fn layer_name(model: &BoardModel, id: u32) -> String {
    match model.layers.iter().find(|l| l.id == id) {
        Some(l) if !l.name.is_empty() => l.name.clone(),
        _ if id == 0 => "F.Cu".into(),
        _ => format!("layer {id}"),
    }
}

/// Terminal lies on copper of `layer`.
fn on_layer(model: &BoardModel, t: &TermRef, layer: u32) -> bool {
    match &t.kind {
        TermKind::Pad(i) => model
            .pads
            .get(*i)
            .is_some_and(|p| p.layers.contains(&layer)),
        TermKind::Via(i) => model
            .vias
            .get(*i)
            .is_some_and(|v| via_layers(&v.layers, &model.all_layer_ids()).contains(&layer)),
        TermKind::Copper(u) | TermKind::Dangling(u) => model
            .kept
            .iter()
            .find(|k| &k.uuid == u)
            .is_none_or(|k| k.layer == layer),
    }
}

/// Route the unconnected nets of `model` on F.Cu.
pub fn route(model: &BoardModel, opts: &Options) -> RouteResult {
    let t0 = Instant::now();
    let deadline = deadline_after(opts.wall_clock_s, t0);
    let mut warnings: Vec<String> = Vec::new();
    if opts.existing_copper == ExistingCopper::Smooth {
        warnings.push("existing_copper=smooth ignored by route(); treated as fixed".into());
    }
    let routable: Vec<u32> = model
        .layers
        .iter()
        .filter(|l| l.routable)
        .map(|l| l.id)
        .collect();
    let layer = if routable.contains(&0) {
        0
    } else {
        routable.iter().copied().min().unwrap_or(0)
    };
    let unused: Vec<String> = routable
        .iter()
        .filter(|&&l| l != layer)
        .map(|&l| layer_name(model, l))
        .collect();
    if !unused.is_empty() {
        warnings.push(format!(
            "routable layers {} unused (M2 routes on {} only)",
            unused.join(", "),
            layer_name(model, layer)
        ));
    }
    if routable.is_empty() {
        warnings.push("no routable layer".into());
    }
    let dec = decompose(model, opts, layer);
    for (net, r) in &dec.skipped {
        warnings.push(format!("net {net} skipped: {r}"));
    }
    let all = &dec.connections;
    let lname = layer_name(model, layer);

    // Connections that can be searched, renumbered 0..m (the negotiator indexes by id).
    let mut early: Vec<Option<String>> = vec![None; all.len()];
    let mut live: Vec<Connection> = Vec::new();
    let mut orig: Vec<usize> = Vec::new();
    for c in all {
        if routable.is_empty() {
            early[c.id] = Some("no routable layer".into());
        } else if !on_layer(model, &c.from, layer) || !on_layer(model, &c.to, layer) {
            early[c.id] = Some(format!("terminal not on layer {lname}"));
        } else {
            let mut l = c.clone();
            l.id = live.len();
            orig.push(c.id);
            live.push(l);
        }
    }
    let m = live.len();

    let mut best: Option<Snap> = None;
    let (mut iterations, mut converged, mut timed_out) = (0u32, false, false);
    let mut setup_fail: Option<String> = None;

    if m > 0 {
        // Obstacles: junction / dangling ends of copper attach points are terminals.
        let mut tmap: BTreeMap<((u64, u64), i32), TerminalObstacle> = BTreeMap::new();
        for c in &live {
            for t in [&c.from, &c.to] {
                if matches!(t.kind, TermKind::Copper(_) | TermKind::Dangling(_)) {
                    let e = tmap
                        .entry((bits(t.at), c.net))
                        .or_insert_with(|| TerminalObstacle {
                            layer,
                            at: t.at,
                            net: c.net,
                            width: c.width,
                            label: format!("terminal:net{}", c.net),
                        });
                    e.width = e.width.max(c.width);
                }
            }
        }
        let tobs: Vec<TerminalObstacle> = tmap.into_values().collect();
        let obs = ObstacleSet::build(model, opts, &HashSet::new(), &tobs);

        let mut seen = HashSet::new();
        let mut pts: Vec<Vec2> = Vec::new();
        let mut tinfo: Vec<TerminalInfo> = Vec::new();
        for c in &live {
            for t in [&c.from, &c.to] {
                if seen.insert(bits(t.at)) {
                    pts.push(t.at);
                    tinfo.push(TerminalInfo {
                        at: t.at,
                        net: c.net,
                        owner: t.label.clone(),
                    });
                }
            }
        }
        match build_layer_cdt(obs.layer(layer), &pts, ConstraintSet::Routing) {
            Err(e) => setup_fail = Some(format!("sketch: {e}")),
            Ok(ls) => {
                let outline: Vec<Prim> = model.outline.iter().map(dequantise).collect();
                let rules = GraphRules {
                    eps: opts.margin_nm as f64,
                    min_clearance: model.rules.min_clearance as f64,
                    edge_clearance: model.rules.edge_clearance as f64,
                    hole_clearance: model.rules.hole_clearance as f64,
                };
                let mut g = RouteGraph::build(&ls, obs.layer(layer), &outline, &tinfo, &rules);
                let verts: Vec<Option<(usize, usize)>> = live
                    .iter()
                    .map(|c| Some((ls.vertex_at(c.from.at)?, ls.vertex_at(c.to.at)?)))
                    .collect();
                if let Some(i) = verts.iter().position(Option::is_none) {
                    setup_fail = Some(format!(
                        "sketch: terminal of {} is not a CDT vertex",
                        live[i].from.label
                    ));
                } else {
                    let verts: Vec<(usize, usize)> = verts.into_iter().flatten().collect();
                    let check_opts = Options {
                        margin_nm: 0,
                        ..opts.clone()
                    };
                    let checker = Checker::new(model, &check_opts);
                    let cx = Ctx {
                        model,
                        opts,
                        obs: &obs,
                        ls: &ls,
                        conns: &live,
                        checker: &checker,
                        deadline,
                    };
                    let layer_obs = obs.layer(layer);
                    let mut neg = Negotiator::new(g.edges.len(), m);
                    let mut perm: Vec<Option<String>> = vec![None; m];
                    let mut active: Vec<usize> = (0..m).collect();
                    for _ in 1..=opts.pathfinder_iter_cap {
                        if Instant::now() >= deadline {
                            timed_out = true;
                            break;
                        }
                        let out = neg
                            .iteration(&mut g, layer_obs, &live, &active, &verts, opts, deadline);
                        if out.timed_out {
                            timed_out = true;
                            break;
                        }
                        let it = neg.iteration;
                        let routed: Vec<usize> = (0..m)
                            .filter(|&i| out.paths[i].is_some() && perm[i].is_none())
                            .collect();
                        let mt = metrize(&cx, &g, &routed);
                        if mt.timed_out {
                            timed_out = true;
                            break;
                        }
                        iterations = it;
                        let mut res: Vec<Res> = (0..m)
                            .map(|i| match (&perm[i], out.blocked.get(&i)) {
                                (Some(r), _) => Res::Fail {
                                    reason: r.clone(),
                                    perm: true,
                                    blocking: None,
                                },
                                (None, Some(b)) => Res::Fail {
                                    reason: if b.edge.is_some() {
                                        "blocked: no path (wall or capacity)".into()
                                    } else {
                                        "blocked: no path".into()
                                    },
                                    perm: false,
                                    blocking: b.edge.map(|e| {
                                        let ge = &g.edges[e];
                                        kicad((g.verts[ge.a].at + g.verts[ge.b].at) * 0.5)
                                    }),
                                },
                                (None, None) => fail("not routed"),
                            })
                            .collect();
                        for (i, r) in mt.res {
                            res[i] = r;
                        }
                        // Feedback: failures first (class-independent ones leave the graph).
                        let mut failed: Vec<usize> = Vec::new();
                        for (i, r) in res.iter().enumerate() {
                            if let Res::Fail {
                                reason, perm: p, ..
                            } = r
                            {
                                if out.blocked.contains_key(&i) {
                                    continue;
                                }
                                if *p {
                                    if perm[i].is_none() {
                                        perm[i] = Some(reason.clone());
                                        g.remove_conn(i);
                                    }
                                } else {
                                    failed.push(i);
                                }
                            }
                        }
                        for &i in &failed {
                            neg.penalise_failed(&g, i);
                        }
                        for (&i, b) in &out.blocked {
                            neg.penalise_blocked(i, b);
                        }
                        let overuse = neg.penalise_overuse(&g);
                        let (nok, len) = res.iter().fold((0usize, 0.0f64), |(n, l), r| match r {
                            Res::Ok(o) => (n + 1, l + o.len),
                            _ => (n, l),
                        });
                        neg.record_best(&out.paths, nok, len);
                        let better = best
                            .as_ref()
                            .is_none_or(|b| nok > b.routed || (nok == b.routed && len < b.len));
                        if better {
                            best = Some(Snap {
                                routed: nok,
                                len,
                                iteration: it,
                                res,
                                overuse_edges: overuse,
                                clinches: detect_clinches(&g, &live, &out.paths, &verts),
                                relax_iterations: mt.relax_iterations,
                                rounds: mt.rounds,
                            });
                        }
                        if failed.is_empty() && out.blocked.is_empty() && overuse == 0 {
                            converged = true;
                            break;
                        }
                        let mut next: BTreeSet<usize> = failed.into_iter().collect();
                        next.extend(out.blocked.keys().copied());
                        next.extend(overused_conns(&g));
                        next.retain(|&i| perm[i].is_none());
                        active = next.into_iter().collect();
                        if active.is_empty() {
                            converged = true;
                            break;
                        }
                    }
                }
            }
        }
    }

    // Assemble.
    let mut by_orig: BTreeMap<usize, usize> = BTreeMap::new();
    for (i, &o) in orig.iter().enumerate() {
        by_orig.insert(o, i);
    }
    let no_result = if timed_out {
        "timeout"
    } else {
        "not routed: no completed iteration"
    };
    let mut items: Vec<Item> = Vec::new();
    let mut reports: Vec<ConnReport> = Vec::with_capacity(all.len());
    let (mut total_len, mut out_total) = (0.0, 0.0);
    for c in all {
        let name = dec
            .nets
            .iter()
            .find(|n| n.net == c.net)
            .map(|n| n.name.clone())
            .unwrap_or_default();
        let mut rep = ConnReport {
            id: c.id,
            net: c.net,
            name,
            layer: c.layer,
            width: c.width,
            status: "failed".into(),
            reason: String::new(),
            from: term_out(&c.from),
            to: term_out(&c.to),
            length_nm: 0,
            length_outside_pads_nm: 0,
            pattern: String::new(),
            anchors: 0,
            crossings: 0,
            blocking: None,
            g1_angle_deg: None,
            iteration: 0,
        };
        if let Some(r) = &early[c.id] {
            rep.reason = r.clone();
        } else if let Some(s) = setup_fail.as_ref() {
            rep.reason = s.clone();
        } else {
            let i = by_orig[&c.id];
            match &best {
                None => rep.reason = no_result.into(),
                Some(b) => {
                    rep.iteration = b.iteration;
                    match &b.res[i] {
                        Res::Ok(o) => {
                            rep.status = "routed".into();
                            rep.length_nm = o.len.round() as i64;
                            rep.length_outside_pads_nm = o.out.round() as i64;
                            rep.pattern = o.pattern.clone();
                            rep.anchors = o.anchors;
                            rep.crossings = o.crossings;
                            total_len += o.len;
                            out_total += o.out;
                            items.extend(o.items.iter().cloned().map(Item::Track));
                        }
                        Res::Fail {
                            reason, blocking, ..
                        } => {
                            rep.reason = reason.clone();
                            rep.blocking = *blocking;
                        }
                    }
                }
            }
        }
        reports.push(rep);
    }

    let mut nets: Vec<NetRouteReport> = Vec::with_capacity(dec.nets.len());
    for n in &dec.nets {
        let cs: Vec<&ConnReport> = n.connections.iter().map(|&i| &reports[i]).collect();
        let count = |s: &str| cs.iter().filter(|r| r.status == s).count();
        let (routed, partial, failed) = (count("routed"), count("partial"), count("failed"));
        let status = if n.pour_served {
            "pour_served"
        } else if !n.in_scope {
            "out_of_scope"
        } else if cs.is_empty() {
            "complete"
        } else if routed == cs.len() {
            "routed"
        } else if routed == 0 && partial == 0 {
            "failed"
        } else {
            "partial"
        };
        nets.push(NetRouteReport {
            net: n.net,
            name: n.name.clone(),
            status: status.into(),
            connections: cs.len(),
            routed,
            partial,
            failed,
            length_nm: cs.iter().map(|r| r.length_nm).sum(),
            length_outside_pads_nm: cs.iter().map(|r| r.length_outside_pads_nm).sum(),
        });
    }
    let nets_total = nets.iter().filter(|n| n.connections > 0).count();
    let nets_routed = nets
        .iter()
        .filter(|n| n.connections > 0 && n.status == "routed")
        .count();
    let routed = reports.iter().filter(|r| r.status == "routed").count();
    let partial = reports.iter().filter(|r| r.status == "partial").count();
    let failed = reports.iter().filter(|r| r.status == "failed").count();
    RouteResult {
        items,
        connections_total: reports.len(),
        connections: reports,
        nets,
        routed,
        partial,
        failed,
        nets_total,
        nets_routed,
        total_length_nm: total_len.round() as i64,
        length_outside_pads_nm: out_total.round() as i64,
        iterations,
        converged,
        overuse_edges: best.as_ref().map_or(0, |b| b.overuse_edges),
        clinches: best
            .as_ref()
            .map(|b| b.clinches.clone())
            .unwrap_or_default(),
        relax_iterations: best.as_ref().map_or(0, |b| b.relax_iterations),
        self_check_rounds: best.as_ref().map_or(0, |b| b.rounds),
        timed_out,
        runtime_s: t0.elapsed().as_secs_f64(),
        threads_used: 1,
        warnings,
    }
}
