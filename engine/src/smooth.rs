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
use crate::geom::{dequantise, Vec2};
use crate::model::{flip_track, BoardModel, Item, Options, TrackItem};
use crate::obstacle::{ObstacleSet, TerminalObstacle};
use crate::relax::relax;
use crate::sketch::{build_layer_cdt, sketch_wire, stack_order, AnchorRef, LayerSketch, Sketch};
use crate::taut::{Chain, TautError};
use crate::wires::{extract_wires, Terminal, Wire};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
    pub length_nm: i64,
    pub hand_length_nm: i64,
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
    pub nets_total: usize,
    pub nets_taut: usize,
    pub total_length_nm: i64,
    pub hand_length_nm: i64,
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

    // Obstacles: every extracted wire's kept items are being replaced.
    let exclude: HashSet<String> = wires
        .iter()
        .flat_map(|w| w.kept_uuids.iter().cloned())
        .collect();
    let mut tmap: BTreeMap<(u32, (u64, u64), i32), TerminalObstacle> = BTreeMap::new();
    for w in &wires {
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
        for w in wires.iter().filter(|w| w.layer == l) {
            for p in [w.start.at(), w.goal.at()] {
                if seen.insert(bits(p)) {
                    pts.push(p);
                }
            }
        }
        match build_layer_cdt(obs.layer(l), &pts) {
            Ok(ls) => {
                for (i, w) in wires.iter().enumerate().filter(|(_, w)| w.layer == l) {
                    match sketch_wire(w, &ls) {
                        Ok(s) => sketches[i] = Some(s),
                        Err(e) => fail[i] = Some(format!("sketch: {e}")),
                    }
                }
                lss.insert(l, ls);
            }
            Err(e) => {
                for (i, _) in wires.iter().enumerate().filter(|(_, w)| w.layer == l) {
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
        let rep = Checker::new(&m, opts).check_items(&items);
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

    // Assemble.
    let mut items: Vec<Item> = Vec::new();
    let mut replaced: BTreeSet<String> = BTreeSet::new();
    let mut reports: Vec<WireReport> = Vec::with_capacity(n);
    let mut total_len = 0.0;
    let mut hand_total = 0.0;
    for (i, w) in wires.iter().enumerate() {
        hand_total += w.hand_len;
        let taut = fail[i].is_none();
        let (pattern, anchors, len) = if taut {
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
            (pat, chains[i].as_ref().map_or(0, |c| c.seq.len()), len)
        } else {
            (String::new(), 0, w.hand_len)
        };
        reports.push(WireReport {
            id: w.id,
            net: w.net,
            layer: w.layer,
            width: w.width,
            status: if taut { "taut" } else { "fallback" }.into(),
            reason: fail[i].clone().unwrap_or_default(),
            pattern,
            anchors,
            length_nm: len.round() as i64,
            hand_length_nm: w.hand_len.round() as i64,
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
            length_nm: ws.iter().map(|r| r.length_nm).sum(),
            hand_length_nm: ws.iter().map(|r| r.hand_length_nm).sum(),
        });
    }
    let nets_total = nets.iter().filter(|x| x.wires > 0).count();
    let nets_taut = nets.iter().filter(|x| x.status == "taut").count();
    let wires_taut = reports.iter().filter(|r| r.status == "taut").count();

    SmoothResult {
        items,
        replaced: replaced.into_iter().collect(),
        nets,
        wires: reports,
        wires_total: n,
        wires_taut,
        nets_total,
        nets_taut,
        total_length_nm: total_len.round() as i64,
        hand_length_nm: hand_total.round() as i64,
        iterations,
        converged,
        timed_out,
        self_check_rounds: rounds,
        runtime_s: t0.elapsed().as_secs_f64(),
        threads_used: 1,
    }
}
