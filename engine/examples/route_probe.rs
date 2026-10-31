//! Dev tool (M2 diagnosis, F): replay `route()` iteration by iteration and print what the
//! negotiation, the sketch, the relax stage and the self-check do to every connection.
//!
//! `cargo run --release --example route_probe -- <model.json> [--iters N] [--margin NM]
//!     [--watch NAME,...] [--quiet]`
//!
//! Mirrors `route::route` on F.Cu (same decompose, obstacle set, CDT, graph, negotiator,
//! metrization and feedback order; `Options::default()`), but prints per iteration: the search
//! order, every search outcome (path crossings as `edge(a-b)@slot` with vertex positions in mm,
//! `est_len`, expansions; ghost + blocked edge for a blocked connection), the relax result of
//! every wire (anchor sequence with σ, chain pattern / length, or the `TautError`), the
//! self-check violations, the overused edges with their capacity arithmetic, the history
//! increments and the best-iteration bookkeeping. `--watch` names (net names) get their sketch
//! crossings, anchors, chain primitives and wall penetrations printed in full. Not part of the
//! PyO3 surface; `length_outside_pads` is not reproduced (pub(crate) in smooth.rs).
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Instant;
use topor_engine::cdt::{Cdt, Crossing};
use topor_engine::check::Checker;
use topor_engine::decompose::{decompose, order, terminal_of, Connection, TermKind};
use topor_engine::geom::quant::quantise_chain;
use topor_engine::geom::{dequantise, point_seg, Orient, Prim, Seg, Vec2};
use topor_engine::graph::{GraphRules, RouteGraph, TerminalInfo};
use topor_engine::model::{flip_track, BoardModel, Item, Options, TrackItem};
use topor_engine::negotiate::{overused_conns, Negotiator};
use topor_engine::obstacle::{Core, ObstacleSet, TerminalObstacle};
use topor_engine::relax::relax;
use topor_engine::search::SlotPath;
use topor_engine::sketch::{
    build_layer_cdt, sketch_from_crossings, stack_order, unanchored_bends, AnchorRef,
    ConstraintSet, LayerSketch, Sketch,
};
use topor_engine::taut::TautError;
use topor_engine::wires::Wire;

fn mm(p: Vec2) -> String {
    format!("({:.3},{:.3})", p.x / 1e6, -p.y / 1e6)
}

fn bits(p: Vec2) -> (u64, u64) {
    ((p.x + 0.0).to_bits(), (p.y + 0.0).to_bits())
}

fn edge_str(g: &RouteGraph, e: usize) -> String {
    let ge = &g.edges[e];
    let lab = |v: usize| g.verts[v].owner.clone().unwrap_or_else(|| format!("v{v}"));
    format!(
        "e{}[{}{}-{}{}{}]",
        e,
        lab(ge.a),
        mm(g.verts[ge.a].at),
        lab(ge.b),
        mm(g.verts[ge.b].at),
        if ge.constraint { " C" } else { "" }
    )
}

fn path_str(g: &RouteGraph, p: &SlotPath) -> String {
    let mut s = String::new();
    for &(e, slot) in &p.crossings {
        s.push_str(&format!(" {}@{}", edge_str(g, e), slot));
    }
    s
}

/// Crossings of the live path of `c` (copy of route.rs `path_crossings`).
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

fn sigma(s: Orient) -> &'static str {
    match s {
        Orient::Ccw => "ccw",
        Orient::Cw => "cw",
    }
}

fn prim_str(p: &Prim) -> String {
    match p {
        Prim::Seg(s) => format!(
            "S {}->{} {:.3}mm",
            mm(s.a),
            mm(s.b),
            (s.b - s.a).len() / 1e6
        ),
        Prim::Arc(a) => format!(
            "A c{} r{:.3} {} sweep{:.1}deg {:.3}mm",
            mm(a.c),
            a.r / 1e6,
            sigma(a.sigma),
            a.sweep.to_degrees(),
            a.length() / 1e6
        ),
    }
}

/// Walls of `obs` whose inflated band a primitive enters (same formula as taut `penetrations`).
fn wall_hits(
    obs: &ObstacleSet,
    layer: u32,
    net: i32,
    width: f64,
    prim: &Prim,
) -> Vec<(String, f64, f64)> {
    let l = obs.layer(layer);
    let bb = prim.bbox().grow(obs.query_grow());
    let mut out = Vec::new();
    for w in l.query_walls(bb) {
        let wall = &l.walls[w];
        if ObstacleSet::exempt(wall.net, &wall.tie_nets, net) {
            continue;
        }
        let d = match &wall.core {
            Core::Point(q) => match prim {
                Prim::Seg(s) => point_seg(*q, s),
                Prim::Arc(a) => topor_engine::geom::point_arc(*q, a),
            },
            Core::Seg(s) => topor_engine::geom::dist_prim(&Prim::Seg(*s), prim),
            Core::Arc(a) => topor_engine::geom::dist_prim(&Prim::Arc(*a), prim),
        };
        let rho = obs.rho_wall(layer, w, net, width);
        if d < rho - 1.0 {
            let ends: Vec<String> = wall
                .ends
                .iter()
                .map(|&e| format!("{}{}", l.anchors[e].label, mm(l.anchors[e].c)))
                .collect();
            out.push((format!("{} ends[{}]", wall.label, ends.join(", ")), d, rho));
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut path: Option<String> = None;
    let mut iters: Option<u32> = None;
    let mut watch: BTreeSet<String> = BTreeSet::new();
    let mut quiet = false;
    let mut margin: Option<i64> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--iters" => {
                iters = Some(args[i + 1].parse().expect("--iters N"));
                i += 1;
            }
            "--margin" => {
                margin = Some(args[i + 1].parse().expect("--margin NM"));
                i += 1;
            }
            "--watch" => {
                watch.extend(args[i + 1].split(',').map(str::to_string));
                i += 1;
            }
            "--quiet" => quiet = true,
            a => path = Some(a.to_string()),
        }
        i += 1;
    }
    let path = path.expect(
        "usage: route_probe <model.json> [--iters N] [--margin NM] [--watch A,B] [--quiet]",
    );
    let json = std::fs::read_to_string(&path).expect("read model");
    let model = BoardModel::from_json(&json).expect("parse model");
    let mut opts = Options::default();
    if let Some(n) = iters {
        opts.pathfinder_iter_cap = n;
    }
    if let Some(e) = margin {
        opts.margin_nm = e;
    }
    let t0 = Instant::now();
    let deadline = t0 + std::time::Duration::from_secs_f64(opts.wall_clock_s);
    let layer = 0u32;

    let dec = decompose(&model, &opts, layer);
    let name_of = |net: i32| -> String {
        dec.nets
            .iter()
            .find(|n| n.net == net)
            .map(|n| n.name.clone())
            .unwrap_or_else(|| format!("net{net}"))
    };
    let live: Vec<Connection> = dec
        .connections
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut l = c.clone();
            l.id = i;
            l
        })
        .collect();
    let m = live.len();
    let cname = |i: usize| -> String {
        let c = &live[i];
        format!(
            "#{} {}({}->{})",
            i,
            name_of(c.net),
            c.from.label,
            c.to.label
        )
    };
    println!(
        "model {}: {} connections, {} nets, rules clr {} edge {} eps {}",
        model.board,
        m,
        dec.nets.len(),
        model.rules.min_clearance,
        model.rules.edge_clearance,
        opts.margin_nm
    );
    for c in &live {
        println!(
            "  {} tier {} w {} clr {} est {:.3}mm {} -> {}",
            cname(c.id),
            c.tier,
            c.width,
            c.clearance,
            c.est_len / 1e6,
            mm(c.from.at),
            mm(c.to.at)
        );
    }

    // Obstacles / CDT / graph as route() builds them.
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
    let obs = ObstacleSet::build(&model, &opts, &HashSet::new(), &tobs);
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
                    clr: (c.clearance as f64).max(model.rules.min_clearance as f64),
                    r: match t.kind {
                        TermKind::Copper(_) | TermKind::Dangling(_) => 0.5 * c.width as f64,
                        _ => 0.0,
                    },
                });
            }
        }
    }
    let ls: LayerSketch =
        build_layer_cdt(obs.layer(layer), &pts, ConstraintSet::Routing).expect("cdt");
    let outline: Vec<Prim> = model.outline.iter().map(dequantise).collect();
    let rules = GraphRules {
        eps: opts.margin_nm as f64,
        min_clearance: model.rules.min_clearance as f64,
        edge_clearance: model.rules.edge_clearance as f64,
        hole_clearance: model.rules.hole_clearance as f64,
    };
    let mut g = RouteGraph::build(&ls, obs.layer(layer), &outline, &tinfo, &rules);
    let verts: Vec<(usize, usize)> = live
        .iter()
        .map(|c| {
            (
                ls.vertex_at(c.from.at).expect("from vertex"),
                ls.vertex_at(c.to.at).expect("to vertex"),
            )
        })
        .collect();
    println!(
        "graph: {} verts, {} edges, {} faces ({} inside), {} constraint edges; walls {} anchors {}",
        g.verts.len(),
        g.edges.len(),
        g.faces.len(),
        g.faces.iter().filter(|f| f.inside).count(),
        g.edges.iter().filter(|e| e.constraint).count(),
        obs.layer(layer).walls.len(),
        obs.layer(layer).anchors.len()
    );
    // Edges whose capacity cannot take even one 0.25 mm wire: the search sees them as x4.
    let neg_cap: Vec<usize> = (0..g.edges.len())
        .filter(|&e| g.passable(e, i32::MAX) && !g.fits(e, 250_000.0, 150_000.0))
        .collect();
    println!(
        "passable edges that do not fit one 0.25/0.15 wire: {} of {}",
        neg_cap.len(),
        g.edges.len()
    );

    let check_opts = Options {
        margin_nm: 0,
        ..opts.clone()
    };
    let checker = Checker::new(&model, &check_opts);
    let layer_obs = obs.layer(layer);
    let mut neg = Negotiator::new(g.edges.len(), m);
    let mut perm: Vec<Option<String>> = vec![None; m];
    let mut active: Vec<usize> = (0..m).collect();
    let mut best: Option<(usize, f64, u32, Vec<String>)> = None;
    let mut last_hist = neg.history.clone();

    for _ in 1..=opts.pathfinder_iter_cap {
        let ord: Vec<usize> = order(&live, &neg.bumps)
            .into_iter()
            .filter(|i| active.contains(i))
            .collect();
        println!(
            "\n=== iteration {} active {:?}",
            neg.iteration + 1,
            ord.iter().map(|&i| cname(i)).collect::<Vec<_>>()
        );
        let out = neg.iteration(&mut g, layer_obs, &live, &active, &verts, &opts, deadline);
        let it = neg.iteration;
        for &i in &ord {
            match (&out.paths[i], out.blocked.get(&i)) {
                (Some(p), _) => {
                    if !quiet || watch.contains(&name_of(live[i].net)) {
                        println!(
                            "  search {}: FOUND {} crossings est {:.3}mm exp {}:{}",
                            cname(i),
                            p.crossings.len(),
                            p.est_len / 1e6,
                            p.expansions,
                            path_str(&g, p)
                        );
                    } else {
                        println!(
                            "  search {}: FOUND {} crossings est {:.3}mm exp {}",
                            cname(i),
                            p.crossings.len(),
                            p.est_len / 1e6,
                            p.expansions
                        );
                    }
                }
                (None, Some(b)) => {
                    println!(
                        "  search {}: BLOCKED edge {:?} last {:?} ghost {}",
                        cname(i),
                        b.edge.map(|e| edge_str(&g, e)),
                        b.last_reached,
                        b.ghost
                            .as_ref()
                            .map(|p| format!("{} crossings{}", p.crossings.len(), path_str(&g, p)))
                            .unwrap_or_else(|| "none".into())
                    );
                }
                (None, None) => println!("  search {}: no path (timeout?)", cname(i)),
            }
        }

        // --- metrize (copy of route.rs) -------------------------------------------------
        let routed: Vec<usize> = (0..m)
            .filter(|&i| out.paths[i].is_some() && perm[i].is_none())
            .collect();
        let mut wires: Vec<Wire> = Vec::new();
        let mut sketches: Vec<Sketch> = Vec::new();
        let mut live_of: Vec<usize> = Vec::new();
        let mut res: Vec<Option<String>> = vec![None; m]; // failure reason
        for &i in &routed {
            let c = &live[i];
            let Some(path) = path_crossings(&g, c.id) else {
                res[i] = Some("sketch: inconsistent slot path".into());
                continue;
            };
            let (start, goal) = (c.from.at, c.to.at);
            if let Some(p) = unanchored_bends(start, goal, &path, &ls).first() {
                res[i] = Some(format!(
                    "sketch: path bends at a wall split vertex {}",
                    mm(*p)
                ));
                continue;
            }
            let k = wires.len();
            let s = sketch_from_crossings(k, start, goal, path.clone(), &ls);
            if watch.contains(&name_of(c.net)) {
                println!(
                    "  sketch {}: {} raw crossings, {} reduced",
                    cname(i),
                    path.len(),
                    s.crossings.len()
                );
                for cr in &s.crossings {
                    let lv = &g.verts[cr.left];
                    let rv = &g.verts[cr.right];
                    println!(
                        "      cross L {}{} R {}{} t {:.3} at {}",
                        lv.owner.clone().unwrap_or_else(|| format!("v{}", cr.left)),
                        mm(lv.at),
                        rv.owner.clone().unwrap_or_else(|| format!("v{}", cr.right)),
                        mm(rv.at),
                        cr.t,
                        mm(lv.at + (rv.at - lv.at) * cr.t)
                    );
                }
                for a in &s.seq {
                    let an = &layer_obs.anchors[a.anchor];
                    println!(
                        "      anchor {} {} r_core {:.3} {} rho {:.3}",
                        an.label,
                        mm(an.c),
                        an.r_core / 1e6,
                        sigma(a.sigma),
                        obs.rho_anchor(layer, a.anchor, c.net, c.width as f64) / 1e6
                    );
                }
            }
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
        let mut orders: HashMap<u32, HashMap<AnchorRef, Vec<usize>>> = HashMap::new();
        orders.insert(layer, stack_order(&sketches, &ls));
        let r = relax(&wires, &sketches, &obs, &orders, &opts, deadline);
        println!(
            "  relax: {} wires, {} iterations, converged {}",
            n, r.iterations, r.converged
        );
        let mut emit: Vec<Vec<TrackItem>> = vec![Vec::new(); n];
        let mut chain_len: Vec<f64> = vec![0.0; n];
        for (k, ch) in r.chains.iter().enumerate() {
            let i = live_of[k];
            let c = &live[i];
            match ch {
                Ok(ch) => {
                    let q = quantise_chain(&ch.prims);
                    chain_len[k] = ch.length;
                    if q.is_empty() {
                        res[i] = Some("empty chain after quantisation".into());
                    } else {
                        emit[k] = q
                            .into_iter()
                            .map(|t| TrackItem {
                                net: c.net,
                                layer: c.layer,
                                width: c.width,
                                prim: flip_track(t),
                                group: None,
                                wire: Some(i),
                            })
                            .collect();
                    }
                    let seqs: Vec<String> = ch
                        .seq
                        .iter()
                        .map(|a| {
                            format!("{}/{}", layer_obs.anchors[a.anchor].label, sigma(a.sigma))
                        })
                        .collect();
                    if !quiet || watch.contains(&name_of(c.net)) {
                        println!(
                            "  chain {}: {} {:.3}mm anchors [{}]",
                            cname(i),
                            ch.pattern(),
                            ch.length / 1e6,
                            seqs.join(" ")
                        );
                    }
                    if watch.contains(&name_of(c.net)) {
                        for p in &ch.prims {
                            println!("      {}", prim_str(p));
                        }
                    }
                }
                Err(e) => {
                    let reason = match e {
                        TautError::Sleeve(s) => format!("sleeve: {s}"),
                        TautError::InsideObstacle(s) => format!("inside obstacle: {s}"),
                        TautError::Infeasible(s) => format!("infeasible: {s}"),
                        TautError::Cap => "infeasible: pass cap".into(),
                    };
                    println!("  chain {}: FAIL {}", cname(i), reason);
                    if matches!(e, TautError::InsideObstacle(_)) {
                        res[i] = Some(format!("PERM {reason}"));
                    } else {
                        res[i] = Some(reason);
                    }
                    // Replay the straight sketch geometry to show the penetrations.
                    if watch.contains(&name_of(c.net)) {
                        let sk = &sketches[k];
                        let mut pts: Vec<Vec2> = vec![c.from.at];
                        pts.extend(sk.seq.iter().map(|a| layer_obs.anchors[a.anchor].c));
                        pts.push(c.to.at);
                        for w in pts.windows(2) {
                            let prim = Prim::Seg(Seg { a: w[0], b: w[1] });
                            for (lab, d, rho) in
                                wall_hits(&obs, layer, c.net, c.width as f64, &prim)
                            {
                                println!(
                                    "      polyline {} penetrates wall {} dist {:.3} rho {:.3}",
                                    prim_str(&prim),
                                    lab,
                                    d / 1e6,
                                    rho / 1e6
                                );
                            }
                        }
                    }
                }
            }
        }
        // self-check rounds
        let mut rounds = 0u32;
        loop {
            if rounds as usize > n {
                break;
            }
            let mut items: Vec<Item> = Vec::new();
            let mut owner: Vec<usize> = Vec::new();
            for k in 0..n {
                if res[live_of[k]].is_none() {
                    for t in &emit[k] {
                        items.push(Item::Track(t.clone()));
                        owner.push(k);
                    }
                }
            }
            if items.is_empty() {
                break;
            }
            rounds += 1;
            let rep = checker.check_items(&items);
            let mut any = false;
            for v in &rep.violations {
                let i = live_of[owner[v.item]];
                if res[i].is_none() {
                    res[i] = Some(format!("self-check: {}", v.against));
                    println!(
                        "  self-check {}: against {} gap {} required {}",
                        cname(i),
                        v.against,
                        v.gap,
                        v.required
                    );
                    any = true;
                }
            }
            if !any {
                break;
            }
        }

        // --- feedback (copy of route.rs) ------------------------------------------------
        let mut failed: Vec<usize> = Vec::new();
        for i in 0..m {
            if out.blocked.contains_key(&i) || perm[i].is_some() {
                continue;
            }
            if let Some(r) = &res[i] {
                if let Some(p) = r.strip_prefix("PERM ") {
                    perm[i] = Some(p.to_string());
                    g.remove_conn(i);
                } else {
                    failed.push(i);
                }
            }
        }
        for &i in &failed {
            let occ: Vec<String> = g
                .occupancy(i)
                .iter()
                .map(|&(e, _)| edge_str(&g, e))
                .collect();
            println!(
                "  penalise_failed {} ({} occupancy edges: {})",
                cname(i),
                occ.len(),
                occ.join(" ")
            );
            neg.penalise_failed(&g, i);
        }
        for (&i, b) in &out.blocked {
            println!(
                "  penalise_blocked {} ghost edges {}",
                cname(i),
                b.ghost.as_ref().map_or(0, |p| p.crossings.len())
            );
            neg.penalise_blocked(i, b);
        }
        let overuse = neg.penalise_overuse(&g);
        for e in 0..g.edges.len() {
            if g.overuse(e) > 0.0 {
                let ge = &g.edges[e];
                let ents: Vec<String> = ge
                    .cross
                    .iter()
                    .map(|x| format!("{}(w{} c{})", cname(x.conn), x.width, x.clr))
                    .collect();
                println!(
                    "  OVERUSED {} cap {:.3}mm c_e {:.3} overuse {:.3}mm entries [{}]",
                    edge_str(&g, e),
                    ge.cap / 1e6,
                    ge.c_e / 1e6,
                    g.overuse(e) / 1e6,
                    ents.join(", ")
                );
            }
        }
        let nok = (0..m)
            .filter(|&i| out.paths[i].is_some() && perm[i].is_none() && res[i].is_none())
            .count();
        let len: f64 = (0..n)
            .filter(|&k| res[live_of[k]].is_none())
            .map(|k| chain_len[k])
            .sum();
        let bumped: Vec<String> = (0..g.edges.len())
            .filter(|&e| neg.history[e] != last_hist[e])
            .map(|e| {
                format!(
                    "{}:{:+.1}->{:.1}",
                    edge_str(&g, e),
                    neg.history[e] - last_hist[e],
                    neg.history[e]
                )
            })
            .collect();
        if !quiet {
            println!("  history changes ({}): {}", bumped.len(), bumped.join(" "));
        } else {
            println!("  history changes: {}", bumped.len());
        }
        last_hist = neg.history.clone();
        let statuses: Vec<String> = (0..m)
            .map(
                |i| match (&perm[i], out.blocked.get(&i), &res[i], &out.paths[i]) {
                    (Some(r), ..) => format!("{}: PERM {}", cname(i), r),
                    (_, Some(_), ..) => format!("{}: blocked", cname(i)),
                    (_, _, Some(r), _) => format!("{}: {}", cname(i), r),
                    (_, _, None, Some(_)) => format!("{}: ok", cname(i)),
                    _ => format!("{}: not routed", cname(i)),
                },
            )
            .collect();
        let better = best
            .as_ref()
            .is_none_or(|b| nok > b.0 || (nok == b.0 && len < b.1));
        println!(
            "  summary: routed {} / {} metric len {:.3}mm failed {:?} blocked {} overused {}{}",
            nok,
            m,
            len / 1e6,
            failed.iter().map(|&i| cname(i)).collect::<Vec<_>>(),
            out.blocked.len(),
            overuse,
            if better { "  <- BEST so far" } else { "" }
        );
        if better {
            best = Some((nok, len, it, statuses));
        }
        if failed.is_empty() && out.blocked.is_empty() && out.ripped.is_empty() && overuse == 0 {
            println!("  CONVERGED");
            break;
        }
        let mut next: BTreeSet<usize> = failed.into_iter().collect();
        next.extend(out.blocked.keys().copied());
        next.extend(out.ripped.iter().copied());
        next.extend(overused_conns(&g));
        next.retain(|&i| perm[i].is_none());
        active = next.into_iter().collect();
        if active.is_empty() {
            println!("  CONVERGED (empty active set)");
            break;
        }
    }
    if let Some((nok, len, it, statuses)) = best {
        println!(
            "\n=== best iteration {}: routed {} / {} metric len {:.3}mm, {:.3}s",
            it,
            nok,
            m,
            len / 1e6,
            t0.elapsed().as_secs_f64()
        );
        for s in statuses {
            println!("  {s}");
        }
    }
    let _ = Cdt::reduce;
}
