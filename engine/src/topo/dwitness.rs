//! Drawing witness (plan D0): the hand topology of a board (the R0 walk of its copper, loaded
//! into a [`State`] with the hand's layers and via points) is drawn by our drawing (P6,
//! [`Drawer`]). The hand copper is DRC-clean, so with the hand's widths and via positions every
//! rejection is a drawing fault. Three variants:
//!
//! - **A**: hand topology, hand layers, hand widths (per stretch, necked where the hand necks),
//!   hand via positions;
//! - **B**: as A with netclass widths (the difference is the room only necking gives);
//! - **C**: as A with our via placement (`Drawer::place_vias` in a window of traversals around
//!   the hand's via; the difference is via-site loss).
//!
//! The drawing runs as in the core (vias first, widest wires first, then shortest), without the
//! feedback round. [`audit`] compares, per edge-layer, the packer's positions with the crossing
//! points and gaps of the drawn copper (the loose-packing diagnosis); the oblique factor the
//! packer estimates is compared with the one of the hand's own crossing directions.
//!
//! Complexity: the R0 walk plus one drawing (O(wires x runs x checker queries)); the audit is
//! O(passes x run segments). Deterministic (Vec + index ids, BTreeMap counts).

use super::board::{self, Board};
use super::draw::{crossing, seg_dist, via_item, Drawer, Plan, WireOut, Widths};
use super::pack::{infl_dir, infl_of, PackItem};
use super::state::{EdgeId, State, WireId};
use super::witness::{chain_nodes, chains, pieces, sub_state, walk, Chain, Dropped, Locator, Walked, WitnessOpts};
use crate::check::Checker;
use crate::geom::Vec2;
use crate::model::{BoardModel, Options};
use crate::vias::{via_sizes, ViaSize};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Variant {
    A,
    B,
    C,
}

/// Options of the drawing witness.
#[derive(Clone, Debug)]
pub struct DwOpts {
    pub variant: Variant,
    /// Variant C: the via interval reaches this many traversals on each side of the hand's via
    /// traversal (never past the neighbouring vias of the wire).
    pub via_slack: usize,
    /// The drawing options (the router's defaults unless a test sets otherwise).
    pub draw: Options,
    /// `Drawer::tight` (the D0 tight-packing experiment); off (default) is the router's drawing.
    pub tight: bool,
}

impl Default for DwOpts {
    fn default() -> Self {
        DwOpts { variant: Variant::A, via_slack: 2, draw: Options::default(), tight: false }
    }
}

/// One pair of neighbouring drawn passes of different nets on an edge-layer.
#[derive(Clone, Debug, Default, Serialize)]
pub struct PairAudit {
    pub edge: EdgeId,
    pub layer: usize,
    /// The two wires, in edge order.
    pub wires: [WireId; 2],
    /// Packer: required centre spacing along the edge (with the oblique factor) and without it.
    pub req_nm: f64,
    pub req_flat_nm: f64,
    /// Packer: nominal spacing (the evenly spread positions) and the leftmost spacing.
    pub nom_nm: f64,
    pub lmin_nm: f64,
    /// Interval of the passes `[lo, hi]` (nm along the edge) for both.
    pub iv: [[f64; 2]; 2],
    /// Drawn: spacing of the crossing points along the edge.
    pub drawn_nm: f64,
    /// Drawn: copper gap minus clearance between the two runs near the edge (nm).
    pub slack_nm: f64,
    /// Edge length and the room between the end margins (nm).
    pub len_nm: f64,
    /// Number of passes on the edge-layer.
    pub n: usize,
    /// Midpoint of the two crossing points (math frame, nm).
    pub at: [f64; 2],
    /// Absolute positions along the edge (nm from its first vertex) of both passes: drawn
    /// crossing, nominal point, leftmost and rightmost packed position.
    pub x: [f64; 2],
    pub nom: [f64; 2],
    pub lmin: [f64; 2],
    pub rmax: [f64; 2],
}

/// Result of one witness run.
#[derive(Clone, Debug, Default, Serialize)]
pub struct DwOut {
    pub board: String,
    pub variant: Option<Variant>,
    /// Hand chains walked into the State (wires in) and the chains left out by R0 (failed walk,
    /// wall, poured net).
    pub wires_in: usize,
    pub excluded: usize,
    pub drawn: usize,
    pub rejected: usize,
    /// Rejections by reason: `via: ...` (no via) or `<class>/<against kind>` (drawing).
    pub reasons: BTreeMap<String, usize>,
    /// Hand vias placed (A, B) or vias our placement found (C), and those that failed.
    pub vias_ok: usize,
    pub vias_failed: usize,
    /// Accepted runs: from the exact path, from the funnel + repair fallback.
    pub exact_ok: usize,
    pub fallback_ok: usize,
    pub runtime_s: f64,
    /// Drawn and rejected wires, per wire: `(wire, ok, reason)`.
    pub per_wire: Vec<(WireId, bool, String)>,
    pub audit: Vec<PairAudit>,
    /// The oblique factor (`pack::infl_of`, 1 / sin of the crossing angle) of every pass of a
    /// drawn edge-layer with two or more passes: as the packer estimates it (from the State
    /// polyline through the evenly spread pass points) and from the hand's own direction there
    /// (the chord between the hand's neighbouring crossings), `(planned, hand)`.
    pub infl: Vec<(f64, f64)>,
    /// Edge-layers the drawing's packer finds over capacity, and how many of them still are with
    /// the hand's own crossing directions in the oblique factor.
    pub over_lanes: usize,
    pub over_lanes_hand_dir: usize,
    /// Of those still over-full: lanes with two neighbouring passes of one net (the packer spaces
    /// them by half their widths each, though same-net copper may touch).
    pub over_lanes_hand_dir_same_net: usize,
}

/// The kind of a checker `against` label (`pad:U1:3` -> `pad`).
fn kind_of(against: &str) -> &str {
    against.split(':').next().unwrap_or(against)
}

/// Hand width lookup per wire: the non-virtual pieces of its chain `(a, b, layer, width)`.
struct HandWidths {
    pieces: Vec<Vec<(Vec2, Vec2, usize, f64)>>,
}

impl HandWidths {
    fn new(chs: &[Chain], wire_of: &[Option<usize>], n_wires: usize) -> Self {
        let mut pieces = vec![Vec::new(); n_wires];
        for (c, w) in wire_of.iter().enumerate() {
            if let Some(w) = *w {
                let ch = &chs[c];
                for (i, m) in ch.meta.iter().enumerate() {
                    if !m.virt && m.width > 0.0 {
                        pieces[w].push((ch.pts[i], ch.pts[i + 1], m.layer, m.width));
                    }
                }
            }
        }
        HandWidths { pieces }
    }

    /// Width of the hand at `p` on `layer`: that of the nearest piece of the wire on the layer.
    fn at(&self, w: WireId, layer: usize, p: Vec2) -> Option<f64> {
        self.pieces[w].iter().filter(|x| x.2 == layer).map(|x| (seg_dist(p, x.0, x.1), x.3)).min_by(|a, b| a.0.total_cmp(&b.0)).map(|x| x.1)
    }

    /// Width of a drawn segment: the narrowest of the hand widths at its ends and middle.
    fn seg(&self, w: WireId, layer: usize, a: Vec2, b: Vec2, dflt: f64) -> f64 {
        [a, (a + b) * 0.5, b].iter().filter_map(|&p| self.at(w, layer, p)).fold(None, |m: Option<f64>, x| Some(m.map_or(x, |y| y.min(x)))).unwrap_or(dflt)
    }
}

/// Hand layer changes of a walked chain: `(traversal, layer after)` in wire order.
fn layer_changes(wk: &Walked) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut cur = wk.lay[0];
    for k in 0..wk.tris.len() {
        if k > 0 && wk.lay[k] != cur {
            out.push((k - 1, wk.lay[k]));
            cur = wk.lay[k];
        }
        for t in wk.turns.iter().filter(|t| t.0 == k) {
            if t.2 != cur {
                out.push((k, t.2));
                cur = t.2;
            }
        }
    }
    out
}

/// Points where the chain changes layer (the hand's via points), in order.
fn chain_via_points(ch: &Chain) -> Vec<Vec2> {
    (1..ch.meta.len()).filter(|&i| ch.meta[i].layer != ch.meta[i - 1].layer).map(|i| ch.pts[i]).collect()
}

/// Run the drawing witness on the hand board `hand`.
pub fn run(hand: &BoardModel, o: &DwOpts) -> Result<DwOut, String> {
    let t0 = std::time::Instant::now();
    let wo = WitnessOpts::default();
    let b0 = board::build(hand, &wo.build)?;
    let (ps, _) = pieces(hand, &b0.signal_layers, wo.sagitta_mm);
    let mut dropped = Dropped::default();
    let chs = chains(&b0, hand, &ps, &mut dropped);
    drop(b0);
    let nodes = chain_nodes(&chs);
    let b = board::build_with_nodes(hand, &wo.build, &nodes)?;
    let loc = Locator::new(&b);
    let walked: Vec<Walked> = chs.iter().map(|c| walk(&b, &loc, c)).collect();
    let (mut st, wire_of, excluded) = sub_state(&b, hand, &chs, &walked);
    let mut chain_of = vec![0usize; st.wires.len()];
    for (c, w) in wire_of.iter().enumerate() {
        if let Some(w) = w {
            chain_of[*w] = c;
        }
    }
    if o.variant == Variant::B {
        for w in 0..st.wires.len() {
            let net = st.wires[w].net;
            st.wires[w].w = hand.rules.net_rule(net).width as f64;
        }
    }
    // The router sees the board without its tracks and vias.
    let mut stripped = hand.clone();
    stripped.kept.clear();
    stripped.vias.clear();
    let mut out = DwOut { board: hand.board.clone(), variant: Some(o.variant), wires_in: st.wires.len(), excluded, ..DwOut::default() };
    draw_all(&stripped, hand, &o.draw, &b, &st, &chs, &walked, &chain_of, o, &mut out);
    out.runtime_s = t0.elapsed().as_secs_f64();
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn draw_all(m: &BoardModel, hand: &BoardModel, opts: &Options, b: &Board, st: &State, chs: &[Chain], walked: &[Walked], chain_of: &[usize], o: &DwOpts, out: &mut DwOut) {
    let edge_ignored = m.rules.ignored_checks.iter().any(|c| c == "copper_edge_clearance");
    let mut cm = m.clone();
    if edge_ignored {
        cm.rules.edge_clearance = 0;
    }
    let checker = Checker::new(&cm, opts);
    let mut dr = Drawer::new(m, opts, b, st, &checker);
    dr.tight = o.tight;
    let nw = st.wires.len();
    let wire_of: Vec<Option<usize>> = {
        let mut v = vec![None; chs.len()];
        for (w, &c) in chain_of.iter().enumerate() {
            v[c] = Some(w);
        }
        v
    };
    if o.variant != Variant::B {
        let hw = HandWidths::new(chs, &wire_of, nw);
        let pass_w: Vec<f64> = {
            let mut v = vec![0.0; st.passes.len()];
            for (w, wi) in st.wires.iter().enumerate() {
                let wk = &walked[chain_of[w]];
                for (k, &p) in wi.passes.iter().enumerate() {
                    let hp = &wk.passes[k];
                    v[p] = if hp.width > 0.0 && !hp.virt { hp.width } else { wi.w };
                }
            }
            v
        };
        let min_w: Vec<f64> = (0..nw).map(|w| hw.pieces[w].iter().map(|x| x.3).fold(st.wires[w].w, f64::min)).collect();
        let wdef: Vec<f64> = st.wires.iter().map(|w| w.w).collect();
        dr.widths = Some(Widths { pass_w, min_w, seg: Box::new(move |w, l, a, c| hw.seg(w, l, a, c, wdef[w])) });
    }
    // Plans from the hand layers.
    let mut plans: BTreeMap<WireId, Plan> = BTreeMap::new();
    let mut hand_vias: BTreeMap<WireId, Vec<(usize, Vec2)>> = BTreeMap::new();
    for w in 0..nw {
        let wk = &walked[chain_of[w]];
        let ch = &chs[chain_of[w]];
        let changes = layer_changes(wk);
        let pts = chain_via_points(ch);
        let mut layers = vec![wk.lay[0]];
        layers.extend(changes.iter().map(|c| c.1));
        let n = st.wires[w].passes.len();
        let vias: Vec<(usize, usize)> = changes
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if o.variant == Variant::C {
                    let lo = if i == 0 { 0 } else { changes[i - 1].0 + 1 };
                    let hi = if i + 1 == changes.len() { n } else { changes[i + 1].0.saturating_sub(1) };
                    (c.0.saturating_sub(o.via_slack).max(lo).min(c.0), (c.0 + o.via_slack).min(hi).max(c.0))
                } else {
                    (c.0, c.0)
                }
            })
            .collect();
        // The via point: the chain's layer-change point (it is a node vertex of the projection).
        let vp: Vec<(usize, Vec2)> = changes.iter().enumerate().map(|(i, c)| (c.0, pts.get(i).copied().unwrap_or(b.proj.pos[st.wires[w].a]))).collect();
        hand_vias.insert(w, vp);
        plans.insert(w, Plan { layers, vias, at: Vec::new() });
    }
    let wlen = |w: WireId| st.polyline(w).windows(2).map(|p| (p[1] - p[0]).len()).sum::<f64>();
    let mut order: Vec<WireId> = (0..nw).collect();
    let lens: Vec<f64> = (0..nw).map(wlen).collect();
    order.sort_by(|&x, &y| st.wires[y].w.total_cmp(&st.wires[x].w).then(lens[x].total_cmp(&lens[y])).then(x.cmp(&y)));
    // Phase 1: vias.
    let mut placed: BTreeMap<WireId, Vec<(usize, Vec2, ViaSize)>> = BTreeMap::new();
    let mut via_ids: BTreeMap<WireId, Vec<usize>> = BTreeMap::new();
    let mut fail: BTreeMap<WireId, String> = BTreeMap::new();
    for &w in &order {
        let net = st.wires[w].net;
        let mut ids = Vec::new();
        let res: Result<Vec<(usize, Vec2, ViaSize)>, String> = if o.variant == Variant::C {
            dr.place_vias(w, &plans[&w], &mut ids)
        } else {
            let mut v = Vec::new();
            let mut err = None;
            for (i, &(k, p)) in hand_vias[&w].iter().enumerate() {
                // The hand via whose copper holds the layer change (the chain's change point is
                // a track end inside the via, not its centre): its centre and size.
                let (p, sz) = hand
                    .vias
                    .iter()
                    .filter(|x| x.net == net)
                    .min_by(|a, c| (a.at.to_vec2() - p).len().total_cmp(&(c.at.to_vec2() - p).len()))
                    .filter(|x| (x.at.to_vec2() - p).len() < 0.5 * x.d as f64)
                    .map_or_else(|| (p, via_sizes(&m.rules, net, opts)[0]), |x| (x.at.to_vec2(), ViaSize { d: x.d, drill: x.drill }));
                let item = via_item(net, p, sz, Some(w));
                if dr.check_ok(&item) && dr.via_hole_ok(p, sz, net) {
                    ids.push(dr.commit(&item));
                    v.push((k, p, sz));
                } else {
                    let rep = dr.checker.check_against(&dr.idx, std::slice::from_ref(&item), &[]);
                    let ag = rep.violations.first().map_or("hole".to_string(), |x| kind_of(&x.against).to_string());
                    if std::env::var("TOPO_D0DBG").is_ok() {
                        eprintln!("D0 via wire {w} net {net} at ({:.3},{:.3}) d {} drill {}: {:?}", p.x / 1e6, -p.y / 1e6, sz.d, sz.drill, rep.violations.first());
                    }
                    err = Some(format!("via {} of {}: hand via rejected against {ag}", i + 1, hand_vias[&w].len()));
                    break;
                }
            }
            match err {
                Some(e) => Err(e),
                None => Ok(v),
            }
        };
        match res {
            Ok(v) => {
                out.vias_ok += v.len();
                placed.insert(w, v);
                via_ids.insert(w, ids);
            }
            Err(e) => {
                out.vias_failed += 1;
                for id in ids {
                    dr.uncommit(id);
                }
                fail.insert(w, e);
            }
        }
    }
    for (&w, vs) in &placed {
        let pl = &plans[&w];
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
    if std::env::var("TOPO_D0DBG").is_ok() {
        over_debug(&dr, walked, chain_of);
    }
    let mut drawn: BTreeMap<WireId, WireOut> = BTreeMap::new();
    for &w in &order {
        if fail.contains_key(&w) {
            continue;
        }
        let mut ids = via_ids.remove(&w).unwrap_or_default();
        let wo = dr.draw_wire(w, &plans[&w], &placed[&w], &mut ids);
        if wo.ok {
            drawn.insert(w, wo);
        } else {
            for id in ids {
                dr.uncommit(id);
            }
            fail.insert(w, wo.reason.clone());
        }
    }
    out.drawn = drawn.len();
    out.rejected = fail.len();
    out.exact_ok = dr.stats.exact_ok;
    out.fallback_ok = dr.stats.fallback_ok;
    for (&w, r) in &fail {
        let key = if r.starts_with("via") {
            if let Some(p) = r.find("against ") {
                format!("via/{}", &r[p + 8..])
            } else {
                "via/no site".to_string()
            }
        } else if let Some(d) = dr.diag.iter().rev().find(|d| d.wire == w) {
            if std::env::var("TOPO_D0DBG").is_ok() {
                eprintln!(
                    "D0 reject wire {w} net {} L{} k {}..{} of {} {} against {} gap {:.3} req {:.3} short {} nm at ({:.3},{:.3}) static_infeasible {} pre_bad {:?} bumps {}",
                    d.net, d.layer, d.k0, d.k1, st.wires[w].passes.len(), d.class, d.against, d.gap_nm as f64 / 1e6, d.req_nm as f64 / 1e6, d.req_nm - d.gap_nm, d.at.x / 1e6, -d.at.y / 1e6, d.static_infeasible, d.pre_bad, d.bumps
                );
            }
            format!("{}/{}", d.class, kind_of(&d.against))
        } else {
            "other".to_string()
        };
        *out.reasons.entry(key).or_insert(0) += 1;
    }
    out.per_wire = (0..nw).map(|w| (w, drawn.contains_key(&w), fail.get(&w).cloned().unwrap_or_default())).collect();
    out.audit = audit(&dr, &drawn);
    let (infl, over, over_hand, same) = infl_stats(&dr, walked, chain_of);
    out.infl = infl;
    out.over_lanes = over;
    out.over_lanes_hand_dir = over_hand;
    out.over_lanes_hand_dir_same_net = same;
}

/// Neighbour audit of the drawing: per edge-layer, consecutive drawn passes of different nets,
/// the packer's numbers against the drawn crossing points and copper gap.
pub fn audit(dr: &Drawer, drawn: &BTreeMap<WireId, WireOut>) -> Vec<PairAudit> {
    let (st, pr) = (dr.st, &dr.b.proj);
    let mut kpos = vec![0usize; st.passes.len()];
    for wi in st.wires.iter().filter(|w| w.alive) {
        for (k, &p) in wi.passes.iter().enumerate() {
            kpos[p] = k;
        }
    }
    let mut out = Vec::new();
    for e in 0..pr.edges.len() {
        let [p, q] = pr.edges[e];
        let (pp, pq) = (pr.pos[p], pr.pos[q]);
        let len = (pq - pp).len().max(1.0);
        for l in 0..pr.n_layers {
            let items: Vec<usize> = st.edge_order[e].iter().copied().filter(|&pid| dr.pass_layer[pid] == l as i32).collect();
            if items.len() < 2 {
                continue;
            }
            let pitems: Vec<PackItem> = items
                .iter()
                .map(|&pid| {
                    let w = st.passes[pid].wire;
                    let wi = &st.wires[w];
                    let pw = dr.widths.as_ref().map_or(wi.w, |x| x.pass_w[pid]);
                    PackItem { net: wi.net, w: pw, s: wi.s, infl: infl_dir(pr, e, dr.dirs[w][kpos[pid]]), disc: [0.0; 2] }
                })
                .collect();
            let bnd = dr.packer.bounds(e, l, &pitems);
            // Drawn crossing of every pass, from its wire's run on the layer.
            let cross: Vec<Option<(f64, Vec<Vec2>)>> = items
                .iter()
                .map(|&pid| {
                    let w = st.passes[pid].wire;
                    let wo = drawn.get(&w)?;
                    let nom = dr.iv.nom[pid] * len;
                    wo.runs.iter().filter(|(rl, _)| *rl == l).find_map(|(_, pl)| crossing(pl, pp, pq, nom).map(|x| (x, pl.clone())))
                })
                .collect();
            for i in 0..items.len() - 1 {
                let (a, c) = (items[i], items[i + 1]);
                let (wa, wc) = (st.passes[a].wire, st.passes[c].wire);
                if st.wires[wa].net == st.wires[wc].net {
                    continue;
                }
                let (Some((xa, pla)), Some((xc, plc))) = (&cross[i], &cross[i + 1]) else { continue };
                let (ca, cc) = (pp + (pq - pp) * (xa / len), pp + (pq - pp) * (xc / len));
                // Copper gap of the two runs near the edge (segments within 1 mm of the crossings).
                let near = |pl: &[Vec2], c: Vec2| -> Vec<(Vec2, Vec2)> { pl.windows(2).filter(|s| seg_dist(c, s[0], s[1]) < 1.0e6).map(|s| (s[0], s[1])).collect() };
                let (sa, sc) = (near(pla, ca), near(plc, cc));
                let mut g = f64::INFINITY;
                for &(a1, b1) in &sa {
                    for &(a2, b2) in &sc {
                        g = g.min(super::draw::seg_seg_dist(a1, b1, a2, b2));
                    }
                }
                let (it_a, it_c) = (&pitems[i], &pitems[i + 1]);
                let clear = st.wires[wa].s.max(st.wires[wc].s);
                let slack = g - 0.5 * (it_a.w + it_c.w) - clear;
                let infl = it_a.infl.max(it_c.infl);
                out.push(PairAudit {
                    edge: e,
                    layer: l,
                    wires: [wa, wc],
                    req_nm: bnd.req[i],
                    req_flat_nm: bnd.req[i] / infl,
                    nom_nm: (dr.iv.nom[c] - dr.iv.nom[a]) * len,
                    lmin_nm: bnd.lmin[i + 1] - bnd.lmin[i],
                    iv: [[dr.iv.lo[a] * len, dr.iv.hi[a] * len], [dr.iv.lo[c] * len, dr.iv.hi[c] * len]],
                    drawn_nm: xc - xa,
                    slack_nm: slack,
                    len_nm: len,
                    n: items.len(),
                    at: [0.5 * (ca.x + cc.x), 0.5 * (ca.y + cc.y)],
                    x: [*xa, *xc],
                    nom: [dr.iv.nom[a] * len, dr.iv.nom[c] * len],
                    lmin: [bnd.lmin[i], bnd.lmin[i + 1]],
                    rmax: [bnd.rmax[i], bnd.rmax[i + 1]],
                });
            }
        }
    }
    out
}

/// Debug (`TOPO_D0DBG`): every edge-layer the packer finds over capacity, with the hand's own
/// positions of its passes.
fn over_debug(dr: &Drawer, walked: &[Walked], chain_of: &[usize]) {
    let (st, pr) = (dr.st, &dr.b.proj);
    let nl = pr.n_layers;
    let mut kpos = vec![0usize; st.passes.len()];
    for wi in st.wires.iter() {
        for (k, &p) in wi.passes.iter().enumerate() {
            kpos[p] = k;
        }
    }
    for e in 0..pr.edges.len() {
        for l in 0..nl {
            if !dr.over[e * nl + l] {
                continue;
            }
            let items: Vec<usize> = st.edge_order[e].iter().copied().filter(|&pid| dr.pass_layer[pid] == l as i32).collect();
            let [p, q] = pr.edges[e];
            let len = (pr.pos[q] - pr.pos[p]).len();
            let pitems: Vec<PackItem> = items
                .iter()
                .map(|&pid| {
                    let w = st.passes[pid].wire;
                    let wi = &st.wires[w];
                    let pw = dr.widths.as_ref().map_or(wi.w, |x| x.pass_w[pid]);
                    PackItem { net: wi.net, w: pw, s: wi.s, infl: infl_dir(pr, e, dr.dirs[w][kpos[pid]]), disc: [0.0; 2] }
                })
                .collect();
            let bnd = dr.packer.bounds(e, l, &pitems);
            let hand: Vec<f64> = items.iter().map(|&pid| walked[chain_of[st.passes[pid].wire]].passes[kpos[pid]].u * len / 1e6).collect();
            let mid = (pr.pos[p] + pr.pos[q]) * 0.5;
            eprintln!(
                "D0 over e{e} L{l} at ({:.3},{:.3}) len {:.3} ends {:?}/{:?} n {} worst {:.3}: lo {:?} hi {:?} req {:?} infl {:?} w {:?} nets {:?} hand {:?}",
                mid.x / 1e6,
                -mid.y / 1e6,
                len / 1e6,
                pr.vkind[p],
                pr.vkind[q],
                items.len(),
                bnd.worst() / 1e6,
                bnd.lo.iter().map(|x| (x / 1e3).round() / 1e3).collect::<Vec<_>>(),
                bnd.hi.iter().map(|x| (x / 1e3).round() / 1e3).collect::<Vec<_>>(),
                bnd.req.iter().map(|x| (x / 1e3).round() / 1e3).collect::<Vec<_>>(),
                pitems.iter().map(|x| (x.infl * 100.0).round() / 100.0).collect::<Vec<_>>(),
                pitems.iter().map(|x| x.w / 1e6).collect::<Vec<_>>(),
                pitems.iter().map(|x| x.net).collect::<Vec<_>>(),
                hand.iter().map(|x| (x * 1e3).round() / 1e3).collect::<Vec<_>>(),
            );
        }
    }
}

/// The oblique factor as planned against the hand's own crossing direction (see
/// [`DwOut::infl`]), and the over-full edge-layers with either. O(passes).
fn infl_stats(dr: &Drawer, walked: &[Walked], chain_of: &[usize]) -> (Vec<(f64, f64)>, usize, usize, usize) {
    let (st, pr) = (dr.st, &dr.b.proj);
    let nl = pr.n_layers;
    let mut kpos = vec![0usize; st.passes.len()];
    for wi in st.wires.iter() {
        for (k, &p) in wi.passes.iter().enumerate() {
            kpos[p] = k;
        }
    }
    // The hand's direction at pass `pid`: its neighbouring hand crossings (terminals at the ends).
    let hand_infl = |pid: usize| -> f64 {
        let w = st.passes[pid].wire;
        let wk = &walked[chain_of[w]];
        let k = kpos[pid];
        let at = |j: isize| -> Vec2 {
            if j < 0 {
                pr.pos[st.wires[w].a]
            } else if j as usize >= wk.passes.len() {
                pr.pos[st.wires[w].b]
            } else {
                wk.passes[j as usize].at
            }
        };
        let pl = [at(k as isize - 1), wk.passes[k].at, at(k as isize + 1)];
        infl_of(pr, st.passes[pid].edge, &pl, 0)
    };
    let (mut out, mut over, mut over_hand, mut same) = (Vec::new(), 0usize, 0usize, 0usize);
    for e in 0..pr.edges.len() {
        for l in 0..nl {
            let items: Vec<usize> = st.edge_order[e].iter().copied().filter(|&pid| dr.pass_layer[pid] == l as i32).collect();
            if items.is_empty() {
                continue;
            }
            let item = |pid: usize, infl: f64| {
                let wi = &st.wires[st.passes[pid].wire];
                PackItem { net: wi.net, w: dr.widths.as_ref().map_or(wi.w, |x| x.pass_w[pid]), s: wi.s, infl, disc: [0.0; 2] }
            };
            let planned: Vec<f64> = items.iter().map(|&pid| infl_dir(pr, e, dr.dirs[st.passes[pid].wire][kpos[pid]])).collect();
            let hand: Vec<f64> = items.iter().map(|&pid| hand_infl(pid)).collect();
            let a: Vec<PackItem> = items.iter().zip(&planned).map(|(&p, &f)| item(p, f)).collect();
            let b: Vec<PackItem> = items.iter().zip(&hand).map(|(&p, &f)| item(p, f)).collect();
            if !dr.packer.bounds(e, l, &a).fits() {
                over += 1;
                if std::env::var("TOPO_D1DBG").is_ok() && dr.packer.bounds(e, l, &b).fits() {
                    let [p, q] = pr.edges[e];
                    let len = (pr.pos[q] - pr.pos[p]).len();
                    let u = |x: Vec2| ((x - pr.pos[p]).dot(pr.pos[q] - pr.pos[p]) / len / 1e6 * 1e3).round() / 1e3;
                    let mid = (pr.pos[p] + pr.pos[q]) * 0.5;
                    let det: Vec<String> = items
                        .iter()
                        .enumerate()
                        .map(|(i, &pid)| {
                            let w = st.passes[pid].wire;
                            let t = dr.packer.taut(st, w);
                            let hp = &walked[chain_of[w]].passes[kpos[pid]];
                            format!("w{w}/k{}/{}: taut {:.2} at {} hand {:.2} at {}", kpos[pid], st.wires[w].passes.len(), planned[i], u(t.at[kpos[pid]]), hand[i], u(hp.at))
                        })
                        .collect();
                    eprintln!(
                        "D1 taut-over e{e} L{l} at ({:.3},{:.3}) len {:.3} ends {:?}/{:?} worst {:.3} / hand {:.3}: {}",
                        mid.x / 1e6,
                        -mid.y / 1e6,
                        len / 1e6,
                        pr.vkind[p],
                        pr.vkind[q],
                        dr.packer.bounds(e, l, &a).worst() / 1e6,
                        dr.packer.bounds(e, l, &b).worst() / 1e6,
                        det.join("; ")
                    );
                }
                if !dr.packer.bounds(e, l, &b).fits() {
                    over_hand += 1;
                    if b.windows(2).any(|x| x[0].net == x[1].net) {
                        same += 1;
                    }
                }
            }
            if items.len() >= 2 {
                out.extend(planned.into_iter().zip(hand));
            }
        }
    }
    (out, over, over_hand, same)
}
