#![allow(clippy::type_complexity, clippy::too_many_arguments)]
//! Dev tool (M3 gate diagnosis): replicate the `route()` setup, then
//! (A) search + ghost every connection on the EMPTY graph (static routability, per-connection
//!     cost), (B) replay the negotiator to its stalled state, (C) for each still-blocked
//!     connection probe which dynamic occupancy (vias, direct paths, everything) stands in the
//!     ghost's way.
//!
//! `cargo run --release --example gate_diag -- <model.json> [--options JSON] [--iters N] [--skip-a] [--probes]
//!     [--detour 34,208]`
//!
//! `--detour`: for the given connection ids (M3-R11 detour diagnosis) dump, on the EMPTY graph,
//! the slot-search path and the topological ghost crossing by crossing (edge ends, length, cap,
//! need, fits / passable), plus the path the negotiator finally committed.
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Instant;
use topor_engine::check::Checker;
use topor_engine::decompose::{decompose, Connection, TermKind, TermRef};
use topor_engine::graph::{GraphRules, TerminalInfo};
use topor_engine::model::{BoardModel, Options, PadAttr};
use topor_engine::multigraph::MultiGraph;
use topor_engine::negotiate::{Negotiator, PRESENT_OVERUSE};
use topor_engine::obstacle::{ObstacleSet, TerminalObstacle};
use topor_engine::search::{
    default_expansion_cap, ghost, search, tier_weight, via_weight, Goal, SearchOutcome,
    SearchParams,
};
use topor_engine::vias::{via_sites, via_sizes, ViaSize};

fn bits(p: topor_engine::geom::Vec2) -> (u64, u64) {
    (p.x.to_bits(), p.y.to_bits())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut path, mut options_json, mut iters, mut skip_a, mut probes) =
        (None, None, 15u32, false, false);
    let mut detour: Vec<usize> = Vec::new();
    // `--chord ID,L,x0,y0,x1,y1[,x2,y2...]` (mm, math frame): clearance bends of the chords of
    // connection ID on layer index L, as the slot search's `los` would see them.
    let mut chords: Vec<(usize, usize, Vec<topor_engine::geom::Vec2>)> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--options" => {
                options_json = Some(args[i + 1].clone());
                i += 1;
            }
            "--iters" => {
                iters = args[i + 1].parse().unwrap();
                i += 1;
            }
            "--skip-a" => skip_a = true,
            "--probes" => probes = true,
            "--chord" => {
                let v: Vec<f64> = args[i + 1].split(',').map(|x| x.parse().unwrap()).collect();
                let pts = v[2..]
                    .chunks(2)
                    .map(|c| topor_engine::geom::Vec2 {
                        x: c[0] * 1e6,
                        y: c[1] * 1e6,
                    })
                    .collect();
                chords.push((v[0] as usize, v[1] as usize, pts));
                i += 1;
            }
            "--detour" => {
                detour = args[i + 1].split(',').map(|x| x.parse().unwrap()).collect();
                i += 1;
            }
            a => path = Some(a.to_string()),
        }
        i += 1;
    }
    let json = std::fs::read_to_string(path.expect("model")).expect("read");
    let model = BoardModel::from_json(&json).expect("parse");
    let opts = match options_json {
        Some(o) => Options::from_json(&o).expect("options"),
        None => Options::default(),
    };
    let mut routable: Vec<u32> = model
        .layers
        .iter()
        .filter(|l| l.routable)
        .map(|l| l.id)
        .collect();
    routable.sort_unstable();
    routable.dedup();
    let dec = decompose(&model, &opts, &routable);
    let mut live: Vec<Connection> = Vec::new();
    for c in &dec.connections {
        if !(c.from.layers.is_empty() || c.to.layers.is_empty()) {
            let mut l = c.clone();
            l.id = live.len();
            live.push(l);
        }
    }
    let m = live.len();
    let mut tmap: BTreeMap<(u32, (u64, u64), i32), TerminalObstacle> = BTreeMap::new();
    for c in &live {
        for t in [&c.from, &c.to] {
            if let (TermKind::Copper(_) | TermKind::Dangling(_), Some(&layer)) =
                (&t.kind, t.layers.first())
            {
                let e =
                    tmap.entry((layer, bits(t.at), c.net))
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
    let mut obs = ObstacleSet::build(&model, &opts, &HashSet::new(), &tobs);
    let escape: Vec<usize> = live
        .iter()
        .flat_map(|c| [&c.from, &c.to])
        .filter_map(|t| match t.kind {
            TermKind::Pad(pi) if model.pads[pi].attr == PadAttr::Smd => Some(pi),
            _ => None,
        })
        .collect::<BTreeSet<usize>>()
        .into_iter()
        .collect();
    let mut sizes: Vec<ViaSize> = live
        .iter()
        .map(|c| c.net)
        .collect::<BTreeSet<i32>>()
        .into_iter()
        .flat_map(|n| via_sizes(&model.rules, n))
        .collect();
    sizes.sort_by_key(|z| std::cmp::Reverse((z.d, z.drill)));
    sizes.dedup();
    let fit_checker = Checker::new(&model, &opts);
    let t = Instant::now();
    let sites = via_sites(
        &model,
        &obs,
        &fit_checker,
        &routable,
        &escape,
        &sizes,
        opts.margin_nm as f64,
    );
    println!(
        "sites {} ({:.2}s), sizes {:?}",
        sites.len(),
        t.elapsed().as_secs_f64(),
        sizes
    );
    let mut term_at: BTreeMap<(u64, u64), usize> = BTreeMap::new();
    let mut terms: Vec<(TerminalInfo, Vec<u32>)> = Vec::new();
    for c in &live {
        for t in [&c.from, &c.to] {
            if matches!(t.kind, TermKind::Pour(_)) {
                continue;
            }
            match term_at.get(&bits(t.at)) {
                Some(&i) => {
                    for l in &t.layers {
                        if !terms[i].1.contains(l) {
                            terms[i].1.push(*l);
                        }
                    }
                }
                None => {
                    term_at.insert(bits(t.at), terms.len());
                    terms.push((
                        TerminalInfo {
                            at: t.at,
                            net: c.net,
                            owner: t.label.clone(),
                            clr: (c.clearance as f64).max(model.rules.min_clearance as f64),
                            r: match t.kind {
                                TermKind::Copper(_) | TermKind::Dangling(_) => 0.5 * c.width as f64,
                                _ => 0.0,
                            },
                        },
                        t.layers.clone(),
                    ));
                }
            }
        }
    }
    let rules = GraphRules {
        eps: opts.margin_nm as f64,
        min_clearance: model.rules.min_clearance as f64,
        edge_clearance: model.rules.edge_clearance as f64,
        hole_clearance: model.rules.hole_clearance as f64,
    };
    let t = Instant::now();
    let mut mg =
        MultiGraph::build_on(&model, &mut obs, sites, &routable, &terms, &rules).expect("mg");
    println!("multigraph {:.2}s", t.elapsed().as_secs_f64());
    for (li, c) in mg.layers.iter().enumerate() {
        let x: usize = (0..c.g.edges.len()).map(|e| c.g.slot_count(e)).sum();
        println!(
            "  layer {li}: V {} E {} X {}",
            c.g.verts.len(),
            c.g.edges.len(),
            x
        );
    }
    let cap = default_expansion_cap(&mg);
    println!("expansion cap {cap}");
    let vertices = |mg: &MultiGraph, t: &TermRef| -> Vec<(usize, usize)> {
        routable
            .iter()
            .enumerate()
            .filter(|(_, l)| t.layers.contains(l))
            .filter_map(|(li, _)| mg.vertex_of(li, t.at).map(|v| (li, v)))
            .collect()
    };
    let mut starts: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut goals: Vec<Goal> = Vec::new();
    let mut perm = vec![false; m];
    for (i, c) in live.iter().enumerate() {
        let st = vertices(&mg, &c.from);
        let goal = match c.to.kind {
            TermKind::Pour(zi) => {
                let pour_sites: Vec<usize> = (0..mg.sites.len())
                    .filter(|&s| mg.in_pour[s].contains(&zi))
                    .collect();
                if pour_sites.is_empty() {
                    perm[i] = true;
                }
                Goal {
                    vertices: Vec::new(),
                    pour_layer: None,
                    pour_sites,
                }
            }
            _ => Goal {
                vertices: vertices(&mg, &c.to),
                pour_sites: Vec::new(),
                pour_layer: None,
            },
        };
        if st.is_empty() || (goal.vertices.is_empty() && goal.pour_sites.is_empty()) {
            perm[i] = true;
        }
        starts.push(st);
        goals.push(goal);
    }
    let hist: Vec<Vec<f64>> = mg
        .layers
        .iter()
        .map(|c| vec![0.0; c.g.edges.len()])
        .collect();
    let shist = vec![0.0; mg.sites.len()];
    let far = Instant::now() + std::time::Duration::from_secs(3600);
    let params = |c: &Connection| SearchParams {
        history: &hist,
        site_history: &shist,
        weight: tier_weight(c.tier),
        present_overuse: PRESENT_OVERUSE,
        expansion_cap: cap,
        deadline: far,
        eps: None,
        via_cost: opts.via_cost_nm as f64 * via_weight(c.tier),
        max_vias: opts.max_vias_per_connection,
    };
    let label = |c: &Connection| {
        format!(
            "#{} net{} {}->{} tier {} w {}",
            c.id, c.net, c.from.label, c.to.label, c.tier, c.width
        )
    };
    let outcome_str = |r: &SearchOutcome| match r {
        SearchOutcome::Found(p) => format!(
            "FOUND vias {} legs {} est {:.2}mm exp {}",
            p.sites.len(),
            p.legs.len(),
            p.est_len * 1e-6,
            p.expansions
        ),
        SearchOutcome::Blocked {
            ghost,
            blocked_edge,
            blocked_site,
            ..
        } => format!(
            "BLOCKED ghost {} edge {:?} site {:?}",
            match ghost {
                Some(g) => format!(
                    "Some(vias {} est {:.2}mm cost {:.1e})",
                    g.sites.len(),
                    g.est_len * 1e-6,
                    g.cost
                ),
                None => "NONE".into(),
            },
            blocked_edge,
            blocked_site
        ),
        SearchOutcome::Timeout => "TIMEOUT".into(),
    };
    // ---- (A) empty graph
    if !skip_a {
        println!("== (A) empty graph: search / ghost per connection");
        let mut rows: Vec<(f64, String)> = Vec::new();
        for (i, c) in live.iter().enumerate() {
            if perm[i] {
                println!("  {} PERM", label(c));
                continue;
            }
            let p = params(c);
            let t = Instant::now();
            let r = search(&mg, &obs, c, &starts[i], &goals[i], &p);
            let ts = t.elapsed().as_secs_f64();
            let t = Instant::now();
            let g = ghost(&mg, &obs, c, &starts[i], &goals[i], &p);
            let tg = t.elapsed().as_secs_f64();
            let gs = match &g {
                Some((gp, be, bs, _)) => format!(
                    "ghost Some(vias {} est {:.2}mm exp {}) edge {:?} site {:?}",
                    gp.sites.len(),
                    gp.est_len * 1e-6,
                    gp.expansions,
                    be,
                    bs
                ),
                None => "ghost NONE".into(),
            };
            let line = format!(
                "  {} | {} ({:.3}s) | {} ({:.3}s) starts {:?} goalv {} pour_sites {}",
                label(c),
                outcome_str(&r),
                ts,
                gs,
                tg,
                starts[i],
                goals[i].vertices.len(),
                goals[i].pour_sites.len()
            );
            println!("{line}");
            rows.push((ts + tg, line));
        }
        rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        println!("== (A) slowest 8:");
        for (t, l) in rows.iter().take(8) {
            println!("  {t:.3}s {l}");
        }
    }
    // ---- (B) negotiator replay
    println!("== (B) negotiator replay (threads as in options)");
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(opts.effective_threads())
        .build()
        .unwrap();
    let mut neg = Negotiator::new(&mg, m);
    let mut active: Vec<usize> = (0..m).filter(|&i| !perm[i]).collect();
    let mut last_blocked: BTreeMap<usize, (Option<(usize, usize)>, Option<usize>, bool)> =
        BTreeMap::new();
    let mut paths: Vec<Option<topor_engine::search::SlotPath>> = vec![None; m];
    for it in 1..=iters {
        let t = Instant::now();
        let out = pool.install(|| {
            neg.iteration(
                &mut mg, &mut obs, &live, &active, &starts, &goals, &opts, far,
            )
        });
        let routed = out.paths.iter().filter(|p| p.is_some()).count();
        let blk: Vec<usize> = out.blocked.keys().copied().collect();
        println!(
            "  iter {it}: {:.2}s active {} routed {} blocked {:?} ripped {:?} resolved_seq {} overuse {} vias {}",
            t.elapsed().as_secs_f64(),
            active.len(),
            routed,
            blk,
            out.ripped,
            out.resolved_sequentially,
            out.overuse_edges,
            mg.owner.iter().filter(|o| o.is_some()).count()
        );
        last_blocked = out
            .blocked
            .iter()
            .map(|(&k, b)| (k, (b.edge, b.site, b.ghost.is_some())))
            .collect();
        paths = out.paths.clone();
        let mut next: BTreeSet<usize> = blk.iter().copied().collect();
        next.extend(out.ripped.iter().copied());
        if next.is_empty() {
            println!("  converged");
            break;
        }
        active = next.into_iter().collect();
    }
    // ---- (C) probes on the stalled state
    println!("== (C) blocked connections in the final state");
    let owners: Vec<usize> = {
        let mut v: Vec<usize> = mg.owner.iter().flatten().map(|(c, _)| *c).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let direct_owners: Vec<usize> = {
        let mut v: Vec<usize> = Vec::new();
        for c in &mg.layers {
            for e in &c.g.edges {
                if let Some(k) = e.along {
                    if let Some(Some((cc, _))) = mg.wire_of.get(k) {
                        v.push(*cc);
                    }
                }
            }
        }
        v.sort_unstable();
        v.dedup();
        v
    };
    println!("  via owners: {owners:?}");
    println!("  direct-path owners: {direct_owners:?}");
    let blocked_ids: Vec<usize> = last_blocked.keys().copied().collect();
    for &i in &blocked_ids {
        let c = &live[i];
        let (be, bs, had_ghost) = last_blocked[&i];
        let p = params(c);
        let g0 = ghost(&mg, &obs, c, &starts[i], &goals[i], &p);
        println!(
            "  {} | last BlockInfo edge {:?} site {:?} ghost {} | ghost now: {}",
            label(c),
            be,
            bs,
            had_ghost,
            g0.as_ref()
                .map(|(gp, be, bs, _)| format!(
                    "Some(vias {} est {:.2}mm) edge {:?} site {:?}",
                    gp.sites.len(),
                    gp.est_len * 1e-6,
                    be,
                    bs
                ))
                .unwrap_or("NONE".into())
        );
        // probe: remove a set of other connections, ghost + search, re-insert
        let probe = |mg: &mut MultiGraph, obs: &mut ObstacleSet, set: &[usize], name: &str| {
            let removed: Vec<usize> = set
                .iter()
                .copied()
                .filter(|&j| j != i && paths[j].is_some())
                .collect();
            for &j in &removed {
                mg.remove_conn(live[j].id, obs);
            }
            let g = ghost(mg, obs, c, &starts[i], &goals[i], &p);
            let r = search(mg, obs, c, &starts[i], &goals[i], &p);
            println!(
                "      without {name} ({} conns): ghost {} | search {}",
                removed.len(),
                g.as_ref()
                    .map(|(gp, be, bs, _)| format!(
                        "Some(vias {} est {:.2}mm) edge {:?} site {:?}",
                        gp.sites.len(),
                        gp.est_len * 1e-6,
                        be,
                        bs
                    ))
                    .unwrap_or("NONE".into()),
                outcome_str(&r)
            );
            for &j in removed.iter() {
                mg.insert_path(&live[j], paths[j].as_ref().unwrap(), obs);
            }
            g.is_some()
        };
        if !probes {
            continue;
        }
        // `--probes`: each probe runs a ghost + search (seconds each on dcsgonebadge).
        let all: Vec<usize> = (0..m).collect();
        probe(&mut mg, &mut obs, &owners, "via owners");
        probe(&mut mg, &mut obs, &direct_owners, "direct-path owners");
        let both: Vec<usize> = owners.iter().chain(direct_owners.iter()).copied().collect();
        probe(&mut mg, &mut obs, &both, "via + direct owners");
        probe(&mut mg, &mut obs, &all, "everything");
    }
    // ---- terminal context for blocked connections: sites near the terminals
    println!("== terminal context");
    for &i in &blocked_ids {
        let c = &live[i];
        for t in [&c.from, &c.to] {
            let near: Vec<(usize, f64, bool, u32)> = mg
                .sites
                .iter()
                .enumerate()
                .filter(|(_, s)| (s.at - t.at).len() < 3.0e6)
                .map(|(s, st)| {
                    (
                        s,
                        (st.at - t.at).len() * 1e-6,
                        mg.owner[s].is_some(),
                        mg.blocked[s],
                    )
                })
                .collect();
            println!(
                "  #{} {} layers {:?} kind {:?}: sites within 3mm: {}  owned/blocked: {}",
                c.id,
                t.label,
                t.layers,
                std::mem::discriminant(&t.kind),
                near.len(),
                near.iter().filter(|x| x.2 || x.3 > 0).count()
            );
        }
    }
    // ---- start / goal vertex escape: faces and edges around each terminal vertex
    println!("== vertex escape (blocked connections, empty graph)");
    for &j in &(0..m).collect::<Vec<usize>>() {
        if paths[j].is_some() {
            mg.remove_conn(live[j].id, &mut obs);
        }
    }
    for &i in &blocked_ids {
        let c = &live[i];
        let need = c.width as f64 + opts.margin_nm as f64;
        let cc = (c.clearance as f64).max(model.rules.min_clearance as f64);
        let ends: Vec<(&str, Vec<(usize, usize)>)> = vec![
            ("start", starts[i].clone()),
            ("goal", goals[i].vertices.clone()),
        ];
        for (what, vs) in ends {
            for &(li, v) in &vs {
                let g = &mg.layers[li].g;
                let vi = &g.verts[v];
                println!(
                    "  #{} {} {} L{} v{} at ({:.3},{:.3}) r {:.0} clr {:.0} nets {:?} owner {:?} anchor {:?} site {:?} faces {}",
                    c.id, what, if what == "start" { &c.from.label } else { &c.to.label }, li, v, vi.at.x * 1e-6, vi.at.y * 1e-6, vi.r, vi.clr, vi.nets, vi.owner, vi.anchor, vi.site, g.faces_of_vertex(v).len()
                );
                for &f in g.faces_of_vertex(v) {
                    let fc = &g.faces[f];
                    let mut desc = Vec::new();
                    for &e in &fc.e {
                        let ge = &g.edges[e];
                        let far = if ge.a == v {
                            ge.b
                        } else if ge.b == v {
                            ge.a
                        } else {
                            usize::MAX
                        };
                        let needs = need + ge.end_gaps() as f64 * cc.max(ge.c_e);
                        let fv = if far != usize::MAX {
                            &g.verts[far]
                        } else {
                            &g.verts[ge.a]
                        };
                        desc.push(format!(
                            "e{}{}[{}cap {:.0}{} c_e {:.0} gaps {} pass {} far v{} r {:.0} nets {:?} {}]",
                            e,
                            if far == usize::MAX { "(opp)" } else { "" },
                            if ge.constraint { format!("CONSTR {:?} ", ge.owner_nets) } else { String::new() },
                            ge.cap,
                            if ge.cap < needs { "<NEED" } else { "" },
                            ge.c_e,
                            ge.end_gaps(),
                            g.passable(e, c.net),
                            if far == usize::MAX { ge.a } else { far },
                            fv.r,
                            fv.nets,
                            fv.owner.clone().unwrap_or_default()
                        ));
                    }
                    println!("      face {} inside {}: {}", f, fc.inside, desc.join(" "));
                }
            }
        }
        // the ghost's first soft-blocked edge on the empty graph: what is it?
        {
            let p = params(c);
            if let Some((_, Some((li, e)), _, _)) = ghost(&mg, &obs, c, &starts[i], &goals[i], &p) {
                let g = &mg.layers[li].g;
                let ge = &g.edges[e];
                let (va, vb) = (&g.verts[ge.a], &g.verts[ge.b]);
                let needs = need + ge.end_gaps() as f64 * cc.max(ge.c_e);
                println!(
                    "  #{} blocked edge L{} e{}: a v{} ({:.3},{:.3}) r {:.0} nets {:?} {} | b v{} ({:.3},{:.3}) r {:.0} nets {:?} {} | len {:.0} cap {:.0} need {:.0} c_e {:.0} gaps {} constraint {} owner_nets {:?} faces inside {:?} along {:?} passable {} fits {}",
                    c.id, li, e,
                    ge.a, va.at.x * 1e-6, va.at.y * 1e-6, va.r, va.nets, va.owner.clone().unwrap_or_default(),
                    ge.b, vb.at.x * 1e-6, vb.at.y * 1e-6, vb.r, vb.nets, vb.owner.clone().unwrap_or_default(),
                    (va.at - vb.at).len(), ge.cap, needs, ge.c_e, ge.end_gaps(), ge.constraint, ge.owner_nets,
                    ge.faces.map(|f| f.map(|f| g.faces[f].inside)), ge.along,
                    g.passable(e, c.net), g.fits(e, c.width as f64, cc)
                );
            }
        }
        // reverse search (goal -> start) to tell which end fails
        if !goals[i].vertices.is_empty() {
            let rg = Goal {
                vertices: starts[i].clone(),
                pour_sites: Vec::new(),
                pour_layer: None,
            };
            let p = params(c);
            let r = search(&mg, &obs, c, &goals[i].vertices, &rg, &p);
            println!("  #{} reverse search: {}", c.id, outcome_str(&r));
        }
    }

    // ---- (D) detour diagnosis: paths crossing by crossing on the empty graph
    if !detour.is_empty() {
        println!("== (D) detour diagnosis (empty graph)");
        let dump = |mg: &MultiGraph,
                    c: &Connection,
                    p: &topor_engine::search::SlotPath,
                    tag: &str| {
            let need_w = c.width as f64 + opts.margin_nm as f64;
            let cc = (c.clearance as f64).max(model.rules.min_clearance as f64);
            println!(
                "  #{} {tag}: vias {} legs {} est {:.3}mm cost {:.3}mm exp {}",
                c.id,
                p.sites.len(),
                p.legs.len(),
                p.est_len * 1e-6,
                p.cost * 1e-6,
                p.expansions
            );
            for (j, leg) in p.legs.iter().enumerate() {
                let g = &mg.layers[leg.layer_idx].g;
                let (sv, gv) = (&g.verts[leg.start], &g.verts[leg.goal]);
                println!(
                    "    leg {j} L{} start v{} ({:.3},{:.3}) goal v{} ({:.3},{:.3}) est {:.3}mm crossings {}",
                    leg.layer_idx,
                    leg.start,
                    sv.at.x * 1e-6,
                    sv.at.y * 1e-6,
                    leg.goal,
                    gv.at.x * 1e-6,
                    gv.at.y * 1e-6,
                    leg.est_len * 1e-6,
                    leg.crossings.len()
                );
                for &(e, slot) in &leg.crossings {
                    let ge = &g.edges[e];
                    let (va, vb) = (&g.verts[ge.a], &g.verts[ge.b]);
                    let need = need_w + ge.end_gaps() as f64 * cc.max(ge.c_e);
                    let cap = ge.cap;
                    println!(
                        "      e{e} slot {slot}/{}: ({:.3},{:.3}){}{} -> ({:.3},{:.3}){}{} len {:.3} cap {:.3} need {:.3}{} c_e {:.0} gaps {} {}{}fits {} pass {}",
                        g.slot_count(e),
                        va.at.x * 1e-6,
                        va.at.y * 1e-6,
                        va.owner.as_deref().map(|o| format!("[{o}]")).unwrap_or_default(),
                        if va.nets.is_empty() { String::new() } else { format!("{:?}", va.nets) },
                        vb.at.x * 1e-6,
                        vb.at.y * 1e-6,
                        vb.owner.as_deref().map(|o| format!("[{o}]")).unwrap_or_default(),
                        if vb.nets.is_empty() { String::new() } else { format!("{:?}", vb.nets) },
                        (va.at - vb.at).len() * 1e-6,
                        cap * 1e-6,
                        need * 1e-6,
                        if cap < need { " UNFIT" } else { "" },
                        ge.c_e,
                        ge.end_gaps(),
                        if ge.constraint { format!("CONSTR{:?} ", ge.owner_nets) } else { String::new() },
                        if ge.along.is_some() { "ALONG " } else { "" },
                        g.fits(e, c.width as f64, cc),
                        g.passable(e, c.net)
                    );
                }
                if j < p.sites.len() {
                    let (s, z) = p.sites[j];
                    let st = &mg.sites[s];
                    println!(
                        "    via site {s} at ({:.3},{:.3}) size {:?} origin {:?} owner {:?} blocked {}",
                        st.at.x * 1e-6,
                        st.at.y * 1e-6,
                        z,
                        st.origin,
                        mg.owner[s],
                        mg.blocked[s]
                    );
                }
            }
        };
        for &id in &detour {
            let Some(i) = live.iter().position(|c| c.id == id) else {
                println!("  #{id}: not a live connection");
                continue;
            };
            let c = &live[i];
            println!("  {}", label(c));
            if let Some(p) = &paths[i] {
                dump(&mg, c, p, "committed (negotiator, final iteration)");
            }
            let p = params(c);
            match search(&mg, &obs, c, &starts[i], &goals[i], &p) {
                SearchOutcome::Found(sp) => dump(&mg, c, &sp, "search alone"),
                o => println!("  #{id} search alone: {}", outcome_str(&o)),
            }
            match ghost(&mg, &obs, c, &starts[i], &goals[i], &p) {
                Some((gp, be, bs, _)) => {
                    dump(&mg, c, &gp, "ghost");
                    println!("    ghost first soft block: edge {be:?} site {bs:?}");
                }
                None => println!("  #{id} ghost: NONE"),
            }
            // with every other committed path present: which ghost edges are soft and who
            // occupies them (negotiation order vs static geometry)
            let alone = match search(&mg, &obs, c, &starts[i], &goals[i], &p) {
                SearchOutcome::Found(sp) => Some(sp),
                _ => None,
            };
            let others: Vec<usize> = (0..m).filter(|&j| j != i && paths[j].is_some()).collect();
            for &j in &others {
                mg.insert_path(&live[j], paths[j].as_ref().unwrap(), &mut obs);
            }
            if let Some(sp) = &alone {
                let cc = (c.clearance as f64).max(model.rules.min_clearance as f64);
                println!("  #{id} the empty-graph search path with all others present:");
                for leg in &sp.legs {
                    let g = &mg.layers[leg.layer_idx].g;
                    for &(e, _) in &leg.crossings {
                        let ge = &g.edges[e];
                        let pass = g.passable(e, c.net);
                        let fits = g.fits(e, c.width as f64, cc);
                        if pass && fits && ge.cross.is_empty() {
                            continue;
                        }
                        let occ: Vec<String> = ge
                            .cross
                            .iter()
                            .map(|x| match mg.wire_of.get(x.conn).cloned().flatten() {
                                Some((cc, _)) => format!(
                                    "#{} net{} {}->{}",
                                    live[cc].id,
                                    live[cc].net,
                                    live[cc].from.label,
                                    live[cc].to.label
                                ),
                                None => format!("leg{}", x.conn),
                            })
                            .collect();
                        println!(
                            "      L{} e{e} ({:.3},{:.3})->({:.3},{:.3}) cap {:.3} pass {pass} fits {fits} along {:?} occupied by {:?}",
                            leg.layer_idx,
                            g.verts[ge.a].at.x * 1e-6,
                            g.verts[ge.a].at.y * 1e-6,
                            g.verts[ge.b].at.x * 1e-6,
                            g.verts[ge.b].at.y * 1e-6,
                            ge.cap * 1e-6,
                            ge.along.map(|k| mg.wire_of.get(k).cloned().flatten().map(|(cc, _)| live[cc].id)),
                            occ
                        );
                    }
                }
            }
            match search(&mg, &obs, c, &starts[i], &goals[i], &p) {
                SearchOutcome::Found(sp) => dump(&mg, c, &sp, "search with all others present"),
                o => println!(
                    "  #{id} search with all others present: {}",
                    outcome_str(&o)
                ),
            }
            if let Some((gp, be, bs, _)) = ghost(&mg, &obs, c, &starts[i], &goals[i], &p) {
                println!(
                    "  #{id} ghost with all others present: vias {} est {:.3}mm first soft edge {be:?} site {bs:?}",
                    gp.sites.len(),
                    gp.est_len * 1e-6
                );
                let cc = (c.clearance as f64).max(model.rules.min_clearance as f64);
                for leg in &gp.legs {
                    let g = &mg.layers[leg.layer_idx].g;
                    for &(e, _) in &leg.crossings {
                        let ge = &g.edges[e];
                        let pass = g.passable(e, c.net);
                        let fits = g.fits(e, c.width as f64, cc);
                        if pass && fits {
                            continue;
                        }
                        let occ: Vec<String> = ge
                            .cross
                            .iter()
                            .map(|x| match mg.wire_of.get(x.conn).cloned().flatten() {
                                Some((cc, _)) => format!(
                                    "#{} net{} {}->{}",
                                    live[cc].id,
                                    live[cc].net,
                                    live[cc].from.label,
                                    live[cc].to.label
                                ),
                                None => format!("leg{}", x.conn),
                            })
                            .collect();
                        println!(
                            "      soft L{} e{e} ({:.3},{:.3})->({:.3},{:.3}) cap {:.3} pass {pass} fits {fits} along {:?} occupied by {:?}",
                            leg.layer_idx,
                            g.verts[ge.a].at.x * 1e-6,
                            g.verts[ge.a].at.y * 1e-6,
                            g.verts[ge.b].at.x * 1e-6,
                            g.verts[ge.b].at.y * 1e-6,
                            ge.cap * 1e-6,
                            ge.along.map(|k| mg.wire_of.get(k).cloned().flatten().map(|(cc, _)| live[cc].id)),
                            occ
                        );
                    }
                }
            }
            for &j in &others {
                mg.remove_conn(live[j].id, &mut obs);
            }
        }
    }

    // ---- (E) clearance-bend probes along given chords (empty graph)
    for (id, li, pts) in &chords {
        let Some(c) = live.iter().find(|c| c.id == *id) else {
            println!("  chord #{id}: not live");
            continue;
        };
        let g = &mg.layers[*li].g;
        let lobs = obs.layer(mg.layers[*li].layer);
        println!("== (E) chords of #{id} on L{li}");
        for w in pts.windows(2) {
            let (a, t) = (w[0], w[1]);
            let full = topor_engine::search::probe_clearance_bend(g, lobs, c, a, t, &[], true);
            let desc = |b: &Option<(topor_engine::geom::Vec2, Option<usize>)>| match b {
                None => "none".to_string(),
                Some((p, v)) => format!(
                    "bend ({:.3},{:.3}) at vertex {:?}",
                    p.x * 1e-6,
                    p.y * 1e-6,
                    v.map(|v| {
                        let vi = &g.verts[v];
                        format!(
                            "v{v} ({:.3},{:.3}) r {:.0} clr {:.0} nets {:?} {}",
                            vi.at.x * 1e-6,
                            vi.at.y * 1e-6,
                            vi.r,
                            vi.clr,
                            vi.nets,
                            vi.owner.clone().unwrap_or_default()
                        )
                    })
                ),
            };
            println!(
                "  chord ({:.3},{:.3}) -> ({:.3},{:.3}) len {:.3}: full-box {}",
                a.x * 1e-6,
                a.y * 1e-6,
                t.x * 1e-6,
                t.y * 1e-6,
                (t - a).len() * 1e-6,
                desc(&full)
            );
            // vertices near the chord
            let bb = topor_engine::geom::Aabb {
                min: topor_engine::geom::Vec2 {
                    x: a.x.min(t.x) - 600_000.0,
                    y: a.y.min(t.y) - 600_000.0,
                },
                max: topor_engine::geom::Vec2 {
                    x: a.x.max(t.x) + 600_000.0,
                    y: a.y.max(t.y) + 600_000.0,
                },
            };
            for v in g.verts_near(bb) {
                let vi = &g.verts[v];
                if vi.owner.is_none() {
                    continue;
                }
                println!(
                    "      near v{v} ({:.3},{:.3}) r {:.0} clr {:.0} nets {:?} {}",
                    vi.at.x * 1e-6,
                    vi.at.y * 1e-6,
                    vi.r,
                    vi.clr,
                    vi.nets,
                    vi.owner.clone().unwrap_or_default()
                );
            }
        }
    }
}
