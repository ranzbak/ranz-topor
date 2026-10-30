//! `smooth()` orchestration (M1 / T1.4a): extract wires, build obstacles, sketch, relax,
//! quantise, self-check, per-wire fallback.
//!
//! Fallback granularity is the wire. A wire that fails to sketch, tauten or pass the self-check
//! keeps its hand copper (its kept uuids are not in `replaced`). A kept item split into several
//! wires (T-junction) is replaced only if every wire containing it is taut; otherwise all of them
//! fall back. Self-check rounds are bounded by the wire count (each round fails >= 1 wire);
//! neighbour lookups go through `Grid`s inside `Checker` / `ObstacleSet`; output is sorted by
//! wire id and no `HashMap` iteration order reaches it.

use crate::check::Checker;
use crate::geom::quant::{quantise_chain, TrackPrim};
use crate::geom::{dequantise, dist_prim_skeleton, Prim, Seg, Skeleton, Vec2};
use crate::model::{flip_track, BoardModel, Item, Options, TrackItem};
use crate::obstacle::{ObstacleSet, TerminalObstacle};
use crate::relax::relax;
use crate::sketch::{build_layer_cdt, sketch_wire, stack_order, AnchorRef, LayerSketch, Sketch};
use crate::taut::{Chain, TautError};
use crate::wires::{extract_wires, Terminal, Wire};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::f64::consts::PI;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize)]
pub struct WireReport {
    pub id: usize,
    pub net: i32,
    pub layer: u32,
    pub width: i64,
    /// `taut` or `fallback`.
    pub status: String,
    pub reason: String,
    pub pattern: String,
    pub anchors: usize,
    pub length_nm: i64,
    pub hand_length_nm: i64,
    /// Length outside the terminal pads' copper (emitted / hand).
    pub length_outside_pads_nm: i64,
    pub hand_outside_pads_nm: i64,
    /// Fallback wire whose hand copper is clean at the design clearance but has no room for the
    /// build margin ε (user ruling: counts as OK, reported as "fallback: zero slack").
    pub zero_slack: bool,
    /// The hand copper's real slack (0 <= s < ε), only when `zero_slack`.
    pub hand_slack_nm: Option<i64>,
    /// Hand items (kept uuids) of the wire, in path order.
    pub kept_uuids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct NetReport {
    pub net: i32,
    pub name: String,
    /// `taut`, `partial`, `fallback` or `skipped`.
    pub status: String,
    /// Why the net is `skipped` (empty otherwise).
    pub reason: String,
    pub wires: usize,
    pub taut: usize,
    /// Fallback wires classified as zero slack.
    pub zero_slack: usize,
    pub length_nm: i64,
    pub hand_length_nm: i64,
    /// Length outside the terminal pads' copper (emitted / hand).
    pub length_outside_pads_nm: i64,
    pub hand_outside_pads_nm: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SmoothResult {
    /// Replacement items in the KiCad frame.
    pub items: Vec<Item>,
    /// Kept uuids the items replace.
    pub replaced: Vec<String>,
    pub nets: Vec<NetReport>,
    pub wires: Vec<WireReport>,
    pub wires_total: usize,
    pub wires_taut: usize,
    pub wires_zero_slack: usize,
    pub nets_total: usize,
    pub nets_taut: usize,
    pub total_length_nm: i64,
    pub hand_length_nm: i64,
    pub length_outside_pads_nm: i64,
    pub hand_outside_pads_nm: i64,
    pub iterations: u32,
    pub converged: bool,
    pub timed_out: bool,
    pub self_check_rounds: u32,
    pub runtime_s: f64,
    pub threads_used: usize,
}

fn taut_reason(e: &TautError, timed_out: bool) -> String {
    match e {
        TautError::Sleeve(s) if timed_out && s == "not relaxed" => "timeout".into(),
        TautError::Sleeve(s) => format!("sleeve: {s}"),
        TautError::InsideObstacle(s) => format!("inside obstacle: {s}"),
        TautError::Infeasible(s) => format!("infeasible: {s}"),
        TautError::Cap => "infeasible: pass cap".into(),
    }
}

fn bits(p: Vec2) -> (u64, u64) {
    ((p.x + 0.0).to_bits(), (p.y + 0.0).to_bits())
}

fn prim_len(t: &TrackPrim) -> f64 {
    dequantise(t).length()
}

/// Inside interval `[t0, t1]` of the segment `a→b` within pad skeleton `sk` (distance to a convex
/// skeleton is convex along the segment: ternary search for the minimum, bisection for the ends).
/// Every loop runs a fixed 48 iterations.
fn inside_interval(a: Vec2, b: Vec2, sk: &Skeleton) -> Option<(f64, f64)> {
    let g = |t: f64| {
        let p = a + (b - a) * t;
        dist_prim_skeleton(&Prim::Seg(Seg { a: p, b: p }), sk)
    };
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..48 {
        let (m1, m2) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
        if g(m1) <= g(m2) {
            hi = m2;
        } else {
            lo = m1;
        }
    }
    let tm = 0.5 * (lo + hi);
    if g(tm) > 0.0 {
        return None;
    }
    let edge = |mut out: f64, mut inn: f64| {
        for _ in 0..48 {
            let m = 0.5 * (out + inn);
            if g(m) <= 0.0 {
                inn = m;
            } else {
                out = m;
            }
        }
        inn
    };
    let t0 = if g(0.0) <= 0.0 { 0.0 } else { edge(0.0, tm) };
    let t1 = if g(1.0) <= 0.0 { 1.0 } else { edge(1.0, tm) };
    Some((t0, t1))
}

/// Length of the chord `a→b` (representing `len` of copper) outside every skeleton.
fn chord_outside(a: Vec2, b: Vec2, len: f64, sks: &[Skeleton]) -> f64 {
    let mut iv: Vec<(f64, f64)> = sks
        .iter()
        .filter_map(|s| inside_interval(a, b, s))
        .collect();
    iv.sort_by(|x, y| x.0.total_cmp(&y.0));
    let (mut covered, mut end) = (0.0, 0.0f64);
    for (t0, t1) in iv {
        let t0 = t0.max(end);
        if t1 > t0 {
            covered += t1 - t0;
            end = t1;
        }
    }
    len * (1.0 - covered).max(0.0)
}

/// Length of `prims` outside the terminal pads' copper (arcs chorded into ≤ 64 pieces).
fn outside_len(prims: &[Prim], sks: &[Skeleton]) -> f64 {
    if sks.is_empty() {
        return prims.iter().map(Prim::length).sum();
    }
    let mut total = 0.0;
    for p in prims {
        match p {
            Prim::Seg(s) => total += chord_outside(s.a, s.b, (s.b - s.a).len(), sks),
            Prim::Arc(a) => {
                let n = ((a.sweep / (PI / 32.0)).ceil() as usize).clamp(1, 64);
                let pt = |k: usize| {
                    let ang = a.a_start + a.sigma.sign() * a.sweep * (k as f64 / n as f64);
                    a.c + Vec2 {
                        x: ang.cos(),
                        y: ang.sin(),
                    } * a.r
                };
                let piece = a.length() / n as f64;
                for k in 0..n {
                    total += chord_outside(pt(k), pt(k + 1), piece, sks);
                }
            }
        }
    }
    total
}

fn terminal_skeletons(model: &BoardModel, w: &Wire) -> Vec<Skeleton> {
    [&w.start, &w.goal]
        .into_iter()
        .filter_map(|t| match t {
            Terminal::Pad { pad, .. } => model.pads.get(*pad).map(|p| model.skeleton(p, w.layer)),
            _ => None,
        })
        .collect()
}

fn hand_outside(w: &Wire, sks: &[Skeleton]) -> f64 {
    let segs: Vec<Prim> = w
        .hand
        .windows(2)
        .map(|v| Prim::Seg(Seg { a: v[0], b: v[1] }))
        .collect();
    outside_len(&segs, sks)
}

fn deadline_after(s: f64, now: Instant) -> Instant {
    let d = Duration::try_from_secs_f64(s.max(0.0)).unwrap_or(Duration::from_secs(86_400 * 365));
    now.checked_add(d)
        .unwrap_or_else(|| now + Duration::from_secs(86_400 * 365))
}

/// Fail every wire that shares a kept item with a failed wire (to a fixpoint).
fn propagate(wires: &[Wire], fail: &mut [Option<String>]) {
    let mut by_uuid: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, w) in wires.iter().enumerate() {
        for u in &w.kept_uuids {
            by_uuid.entry(u.as_str()).or_default().push(i);
        }
    }
    loop {
        let mut changed = false;
        for (u, ws) in &by_uuid {
            if ws.iter().any(|&i| fail[i].is_some()) {
                for &i in ws {
                    if fail[i].is_none() {
                        fail[i] = Some(format!("shares item {u} with a fallback wire"));
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
}

pub fn smooth(model: &BoardModel, opts: &Options) -> SmoothResult {
    let t0 = Instant::now();
    let deadline = deadline_after(opts.wall_clock_s, t0);
    let ex = extract_wires(model, opts);
    let wires = ex.wires;
    let n = wires.len();
    let mut fail: Vec<Option<String>> = vec![None; n];
    // Wires holding an item the caller pinned keep their hand copper (and every wire sharing
    // an item with them, see `propagate`).
    for (i, w) in wires.iter().enumerate() {
        if let Some(r) = w.kept_uuids.iter().find_map(|u| opts.pinned.get(u)) {
            fail[i] = Some(format!("pinned: {r}"));
        }
    }
    propagate(&wires, &mut fail);

    // Obstacles: the kept items of every wire still being smoothed are being replaced;
    // pinned wires' hand copper stays an obstacle.
    let exclude: HashSet<String> = wires
        .iter()
        .zip(&fail)
        .filter(|(_, f)| f.is_none())
        .flat_map(|(w, _)| w.kept_uuids.iter().cloned())
        .collect();
    let mut tmap: BTreeMap<(u32, (u64, u64), i32), TerminalObstacle> = BTreeMap::new();
    for w in wires
        .iter()
        .zip(&fail)
        .filter(|(_, f)| f.is_none())
        .map(|(w, _)| w)
    {
        for t in [&w.start, &w.goal] {
            if matches!(t, Terminal::Junction { .. } | Terminal::Dangling { .. }) {
                let e = tmap
                    .entry((w.layer, bits(t.at()), w.net))
                    .or_insert_with(|| TerminalObstacle {
                        layer: w.layer,
                        at: t.at(),
                        net: w.net,
                        width: w.width,
                        label: format!("terminal:net{}", w.net),
                    });
                e.width = e.width.max(w.width);
            }
        }
    }
    let terms: Vec<TerminalObstacle> = tmap.into_values().collect();
    let obs = ObstacleSet::build(model, opts, &exclude, &terms);

    // Per layer: CDT + sketches.
    let layers: BTreeSet<u32> = wires.iter().map(|w| w.layer).collect();
    let mut lss: BTreeMap<u32, LayerSketch> = BTreeMap::new();
    let mut sketches: Vec<Option<Sketch>> = vec![None; n];
    for &l in &layers {
        let mut seen = HashSet::new();
        let mut pts: Vec<Vec2> = Vec::new();
        let on_layer = |(i, w): &(usize, &Wire)| w.layer == l && fail[*i].is_none();
        for (_, w) in wires.iter().enumerate().filter(on_layer) {
            for p in [w.start.at(), w.goal.at()] {
                if seen.insert(bits(p)) {
                    pts.push(p);
                }
            }
        }
        match build_layer_cdt(obs.layer(l), &pts) {
            Ok(ls) => {
                let idx: Vec<usize> = wires
                    .iter()
                    .enumerate()
                    .filter(on_layer)
                    .map(|(i, _)| i)
                    .collect();
                for i in idx {
                    let w = &wires[i];
                    match sketch_wire(w, &ls) {
                        Ok(s) => sketches[i] = Some(s),
                        Err(e) => fail[i] = Some(format!("sketch: {e}")),
                    }
                }
                lss.insert(l, ls);
            }
            Err(e) => {
                let idx: Vec<usize> = wires
                    .iter()
                    .enumerate()
                    .filter(on_layer)
                    .map(|(i, _)| i)
                    .collect();
                for i in idx {
                    fail[i] = Some(format!("sketch: {e}"));
                }
            }
        }
    }

    // Compact to the wires that sketched; ids = index into the compacted slice.
    let active: Vec<usize> = (0..n).filter(|&i| fail[i].is_none()).collect();
    let mut sub_wires: Vec<Wire> = Vec::with_capacity(active.len());
    let mut sub_sketches: Vec<Sketch> = Vec::with_capacity(active.len());
    for (k, &i) in active.iter().enumerate() {
        let mut w = wires[i].clone();
        w.id = k;
        sub_wires.push(w);
        let mut s = sketches[i].take().expect("active wire has a sketch");
        s.wire = k;
        sub_sketches.push(s);
    }
    let mut orders: HashMap<u32, HashMap<AnchorRef, Vec<usize>>> = HashMap::new();
    for (&l, ls) in &lss {
        let ss: Vec<Sketch> = sub_sketches
            .iter()
            .zip(&sub_wires)
            .filter(|(_, w)| w.layer == l)
            .map(|(s, _)| s.clone())
            .collect();
        orders.insert(l, stack_order(&ss, ls));
    }
    let (mut iterations, mut converged, mut timed_out) = (0, true, false);
    let mut chains: Vec<Option<Chain>> = vec![None; n];
    if !sub_wires.is_empty() {
        let r = relax(&sub_wires, &sub_sketches, &obs, &orders, opts, deadline);
        iterations = r.iterations;
        converged = r.converged;
        timed_out = r.timed_out;
        for (k, c) in r.chains.into_iter().enumerate() {
            let i = active[k];
            match c {
                Ok(ch) => chains[i] = Some(ch),
                Err(e) => fail[i] = Some(taut_reason(&e, timed_out)),
            }
        }
    }

    // Quantise.
    let mut emit: Vec<Vec<TrackItem>> = vec![Vec::new(); n];
    for i in 0..n {
        let Some(ch) = &chains[i] else { continue };
        let w = &wires[i];
        let q = quantise_chain(&ch.prims);
        if q.is_empty() {
            fail[i] = Some("empty chain after quantisation".into());
            continue;
        }
        emit[i] = q
            .into_iter()
            .map(|t| TrackItem {
                net: w.net,
                layer: w.layer,
                width: w.width,
                prim: flip_track(t),
                group: None,
                wire: Some(w.id),
            })
            .collect();
    }

    // Self-check rounds.
    let mut rounds = 0u32;
    loop {
        propagate(&wires, &mut fail);
        if opts.skip_self_check || rounds as usize > n {
            break;
        }
        let mut items: Vec<Item> = Vec::new();
        let mut owner: Vec<usize> = Vec::new();
        let mut replaced: HashSet<&str> = HashSet::new();
        for i in 0..n {
            if fail[i].is_none() {
                for t in &emit[i] {
                    items.push(Item::Track(t.clone()));
                    owner.push(i);
                }
                replaced.extend(wires[i].kept_uuids.iter().map(String::as_str));
            }
        }
        if items.is_empty() {
            break;
        }
        rounds += 1;
        let mut m = model.clone();
        m.kept.retain(|k| !replaced.contains(k.uuid.as_str()));
        // ε is build headroom, not a rule: check at the design clearance.
        let check_opts = Options {
            margin_nm: 0,
            ..opts.clone()
        };
        let rep = Checker::new(&m, &check_opts).check_items(&items);
        let mut any = false;
        for v in &rep.violations {
            let i = owner[v.item];
            if fail[i].is_none() {
                fail[i] = Some(format!("self-check: {}", v.against));
                any = true;
            }
        }
        if !any {
            break;
        }
    }

    // Zero-slack classification of the fallback wires. Only the two eps-binding failure classes
    // qualify; the hand copper is judged against the post-smooth board (kept minus replaced, plus
    // the emitted items), so a neighbour the smoother moved away no longer excuses it. The hand
    // items are same-net to themselves, hence exempt from each other. Two Checkers built once.
    let mut zero_slack: Vec<Option<i64>> = vec![None; n];
    let eligible = |r: &str| r.starts_with("infeasible:") || r.starts_with("inside obstacle:");
    if (0..n).any(|i| fail[i].as_deref().is_some_and(eligible)) {
        let eps = opts.margin_nm;
        let mut post = model.clone();
        let mut emitted: Vec<Item> = Vec::new();
        let mut gone: HashSet<&str> = HashSet::new();
        for i in 0..n {
            if fail[i].is_none() {
                emitted.extend(emit[i].iter().cloned().map(Item::Track));
                gone.extend(wires[i].kept_uuids.iter().map(String::as_str));
            }
        }
        post.kept.retain(|k| !gone.contains(k.uuid.as_str()));
        let c0 = Checker::new(
            &post,
            &Options {
                margin_nm: 0,
                ..opts.clone()
            },
        );
        let ce = Checker::new(&post, opts);
        let by_uuid: HashMap<&str, &crate::model::KeptItem> =
            model.kept.iter().map(|k| (k.uuid.as_str(), k)).collect();
        let base = emitted.len();
        for (i, w) in wires.iter().enumerate() {
            if !fail[i].as_deref().is_some_and(eligible) {
                continue;
            }
            let mut probe = emitted.clone();
            probe.extend(
                w.kept_uuids
                    .iter()
                    .filter_map(|u| by_uuid.get(u.as_str()))
                    .map(|k| {
                        Item::Track(TrackItem {
                            net: k.net,
                            layer: k.layer,
                            width: k.width,
                            prim: flip_track(k.prim),
                            group: None,
                            wire: None,
                        })
                    }),
            );
            if probe.len() == base {
                continue;
            }
            let hand_viol = |c: &Checker| -> Vec<crate::check::Violation> {
                c.check_items(&probe)
                    .violations
                    .into_iter()
                    .filter(|v| v.item >= base)
                    .collect()
            };
            if !hand_viol(&c0).is_empty() {
                continue;
            }
            let slack = hand_viol(&ce)
                .iter()
                .map(|v| v.gap - (v.required - eps))
                .min();
            if let Some(s) = slack {
                zero_slack[i] = Some(s.max(0));
            }
        }
    }

    // Assemble.
    let mut items: Vec<Item> = Vec::new();
    let mut replaced: BTreeSet<String> = BTreeSet::new();
    let mut reports: Vec<WireReport> = Vec::with_capacity(n);
    let mut total_len = 0.0;
    let mut hand_total = 0.0;
    let (mut out_total, mut hand_out_total) = (0.0, 0.0);
    for (i, w) in wires.iter().enumerate() {
        hand_total += w.hand_len;
        let taut = fail[i].is_none();
        let sks = terminal_skeletons(model, w);
        let hand_out = hand_outside(w, &sks);
        hand_out_total += hand_out;
        let (pattern, anchors, len, out) = if taut {
            let pat: String = emit[i]
                .iter()
                .map(|t| match t.prim {
                    TrackPrim::Seg { .. } => 'S',
                    TrackPrim::Arc { .. } => 'A',
                })
                .collect();
            let len: f64 = emit[i].iter().map(|t| prim_len(&t.prim)).sum();
            total_len += len;
            items.extend(emit[i].iter().cloned().map(Item::Track));
            replaced.extend(w.kept_uuids.iter().cloned());
            let out = chains[i]
                .as_ref()
                .map_or(len, |c| outside_len(&c.prims, &sks));
            (pat, chains[i].as_ref().map_or(0, |c| c.seq.len()), len, out)
        } else {
            (String::new(), 0, w.hand_len, hand_out)
        };
        out_total += out;
        reports.push(WireReport {
            id: w.id,
            net: w.net,
            layer: w.layer,
            width: w.width,
            status: if taut { "taut" } else { "fallback" }.into(),
            reason: match (zero_slack[i], &fail[i]) {
                (Some(s), Some(r)) => format!(
                    "zero slack: hand slack {s} nm < ε {} nm; {r}",
                    opts.margin_nm
                ),
                _ => fail[i].clone().unwrap_or_default(),
            },
            pattern,
            anchors,
            length_nm: len.round() as i64,
            hand_length_nm: w.hand_len.round() as i64,
            length_outside_pads_nm: out.round() as i64,
            hand_outside_pads_nm: hand_out.round() as i64,
            zero_slack: zero_slack[i].is_some(),
            hand_slack_nm: zero_slack[i],
            kept_uuids: w.kept_uuids.clone(),
        });
    }

    // Nets.
    let mut codes: BTreeSet<i32> = model
        .nets
        .iter()
        .map(|x| x.code)
        .filter(|&c| c != 0)
        .collect();
    codes.extend(wires.iter().map(|w| w.net));
    let mut nets: Vec<NetReport> = Vec::with_capacity(codes.len());
    for &c in &codes {
        let name = model
            .nets
            .iter()
            .find(|x| x.code == c)
            .map(|x| x.name.clone())
            .unwrap_or_default();
        let ws: Vec<&WireReport> = reports.iter().filter(|r| r.net == c).collect();
        let taut = ws.iter().filter(|r| r.status == "taut").count();
        let (status, reason) = if ws.is_empty() {
            let kept: Vec<&str> = model
                .kept
                .iter()
                .filter(|k| k.net == c)
                .map(|k| k.uuid.as_str())
                .collect();
            let reason = if kept.is_empty() {
                if model.copper_shapes.iter().any(|s| s.net == c) {
                    "no tracks (pour-served)".to_string()
                } else {
                    "no tracks".to_string()
                }
            } else {
                ex.skipped
                    .iter()
                    .find(|(u, _)| kept.contains(&u.as_str()))
                    .map(|(_, r)| r.clone())
                    .unwrap_or_else(|| "no candidate wires".into())
            };
            ("skipped", reason)
        } else if taut == ws.len() {
            ("taut", String::new())
        } else if taut == 0 {
            ("fallback", String::new())
        } else {
            ("partial", String::new())
        };
        nets.push(NetReport {
            net: c,
            name,
            status: status.into(),
            reason,
            wires: ws.len(),
            taut,
            zero_slack: ws.iter().filter(|r| r.zero_slack).count(),
            length_nm: ws.iter().map(|r| r.length_nm).sum(),
            hand_length_nm: ws.iter().map(|r| r.hand_length_nm).sum(),
            length_outside_pads_nm: ws.iter().map(|r| r.length_outside_pads_nm).sum(),
            hand_outside_pads_nm: ws.iter().map(|r| r.hand_outside_pads_nm).sum(),
        });
    }
    let nets_total = nets.iter().filter(|x| x.wires > 0).count();
    let nets_taut = nets.iter().filter(|x| x.status == "taut").count();
    let wires_taut = reports.iter().filter(|r| r.status == "taut").count();

    let wires_zero_slack = reports.iter().filter(|r| r.zero_slack).count();
    SmoothResult {
        items,
        replaced: replaced.into_iter().collect(),
        nets,
        wires: reports,
        wires_total: n,
        wires_taut,
        wires_zero_slack,
        nets_total,
        nets_taut,
        total_length_nm: total_len.round() as i64,
        hand_length_nm: hand_total.round() as i64,
        length_outside_pads_nm: out_total.round() as i64,
        hand_outside_pads_nm: hand_out_total.round() as i64,
        iterations,
        converged,
        timed_out,
        self_check_rounds: rounds,
        runtime_s: t0.elapsed().as_secs_f64(),
        threads_used: 1,
    }
}
