//! U1 of the inflated sketch (docs/dev/inflated-sketch.md §4.3 batch mode, §6.1): the cut flows
//! of a labelled plan, per layer, measured on the layer sheets of [`super::sheet`]. Measurement
//! only: nothing here feeds back into the planner or the drawing.
//!
//! 1. **Stretches** (§4.3 step 1): every alive wire is cut into its per-layer runs at its planned
//!    vias and at the junction corners it turns at. A via gets a concrete point: the first static
//!    site of the planned triangle in the drawing's order ([`super::pack::tri_site_points`], the
//!    same test as `Packer::via_site`), else the freest of those points (flagged "no site").
//! 2. **Homotopy in the sheet** (§4.3 step 2): the stretch's projection curve (terminal, pass points,
//!    terminal) is walked through the layer's sheet ([`crate::cdt::Cdt::crossings`]), the sequence
//!    of crossed sheet edges is reduced (adjacent back-and-forth crossings cancel), and the funnel
//!    over those portals ([`super::pack::funnel`], zero-width portals from vertex to vertex) gives
//!    the rubber band and the vertices it wraps, with the side it passes them on. Every sheet
//!    vertex lies on or inside a feature, so the reduced sequence is the class of the curve in the
//!    plane minus the layer's features (inferred; Maley 1987 p. 40 computes congestion from the
//!    rubber-band equivalent for this reason).
//! 3. **Flow of a cut** `p q` (§4.3 step 4): the stretches whose band crosses the open segment,
//!    plus those that wrap `p` (or `q`) with the cut's direction inside their wrapped wedge (the
//!    reflex side of the bend: a band that bends around `v` must cross every cut from `v` on its
//!    outer side, Maley's entanglement p. 32). Items are ordered by their position along the cut;
//!    several wraps at one vertex are ordered by stretch id (the design orders them by the
//!    projection's edge order; that only matters for same-net neighbours, which need no gap).
//! 4. **Need and slack**: [`super::demand::need_in`] over the items with the widths of the passes
//!    nearest to each crossing (necks), no oblique factor (Maley's congestion is a plain width
//!    sum, p. 32); `slack = cap - need` with `cap` the cut's length in the planning norm.
//!    `mixed_clearance = "conservative"` charges every different-net gap at the largest clearance
//!    of the cut's items (the additive territory model); `"pairwise"` is KiCad's max of the two
//!    neighbours. Both are reported; the selected one is `need`.
//!
//! Complexity: per layer the sheet (see [`super::sheet`]) plus O(sum of stretch walk lengths) for
//! the bands plus a cut-grid query per band segment and O(cuts at a vertex) per wrap.

use super::demand::{need_in, Bound, Item};
use super::draw::poly_dist;
use super::pack::{funnel, tri_site_points, Packer};
use super::refine::{mid_k, pass_layers_at, via_intervals, LEv};
use super::sheet::{JunctionFeat, LayerSheet, Norm, SheetInput, SheetOpts, SheetStats, ViaFeat};
use super::state::{point_in_poly, Projection, State, VertId, VertKind, WireId, NO_OBS};
use crate::geom::Vec2;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const MM: f64 = 1e6;

/// The plan to measure: the State with its labels and planned via traversals.
pub struct Plan<'a> {
    pub st: &'a State<'a>,
    /// Labelled events per wire (P3 / P4).
    pub events: &'a [Vec<LEv>],
    /// Traversal of every planned via per wire (`CapModel::via_k`).
    pub via_k: &'a [Vec<usize>],
    pub bottom: i32,
    pub packer: &'a Packer<'a>,
    /// Connection id (index into the decomposition's connections) of a wire, if any.
    pub conn: &'a dyn Fn(WireId) -> Option<usize>,
}

/// Measurement options (from `[capacity]`).
#[derive(Clone, Copy, Debug)]
pub struct MeasureOpts {
    pub sheet: SheetOpts,
    /// `mixed_clearance = "pairwise"`.
    pub pairwise: bool,
}

impl MeasureOpts {
    pub fn from_options(o: &crate::model::Options) -> MeasureOpts {
        MeasureOpts {
            sheet: SheetOpts { norm: Norm::for_style(&o.draw_style), radius_nm: o.cut_radius_mm.max(0.0) * MM, via_steps: 16, prune_walk: true },
            pairwise: o.mixed_clearance == "pairwise",
        }
    }
}

/// One per-layer run of a wire.
#[derive(Clone, Debug)]
pub struct Stretch {
    pub wire: WireId,
    pub conn: Option<usize>,
    pub net: i32,
    pub layer: usize,
    /// Wire clearance (nm).
    pub s: f64,
    /// Nominal width and neck floor (nm).
    pub w: f64,
    pub w_floor: f64,
    /// The projection curve: terminal, pass points, terminal.
    pub curve: Vec<Vec2>,
    /// Width of each pass point of `curve[1..len-1]`.
    pub pass_w: Vec<f64>,
    /// Ends at a via without a static site.
    pub nosite: bool,
    /// The rubber band in the layer's sheet (corners), empty when the walk failed.
    pub band: Vec<Vec2>,
    /// Per band corner: the sheet vertex it wraps and whether the vertex lies left of the band.
    pub wraps: Vec<Option<(usize, bool)>>,
}

impl Stretch {
    /// Width at a point of the band: the width of the nearest pass point (nominal at the ends).
    fn width_at(&self, x: Vec2) -> f64 {
        let mut best = (f64::INFINITY, self.w);
        for (i, &w) in self.pass_w.iter().enumerate() {
            let d = (self.curve[i + 1] - x).len2();
            if d < best.0 {
                best = (d, w);
            }
        }
        best.1
    }
}

/// One stretch on one cut.
#[derive(Clone, Copy, Debug)]
pub struct FlowItem {
    pub stretch: usize,
    /// Position along the cut from `p` (0..1); wraps at `p` are 0, at `q` 1.
    pub t: f64,
    /// 0 = crosses the open segment, 1 = wraps `p`, 2 = wraps `q`.
    pub kind: u8,
    pub w: f64,
}

/// The flow of one nonempty cut.
#[derive(Clone, Debug)]
pub struct CutFlow {
    pub cut: usize,
    pub items: Vec<FlowItem>,
    /// Capacity after the half widths of junction ends (nm).
    pub cap: f64,
    /// Need with the selected mixed-clearance rule, the other rule, and at the neck floors.
    pub need: f64,
    pub need_other: f64,
    pub need_floor: f64,
    pub slack: f64,
    /// Slack of each item alone (the static room for that stretch).
    pub alone: Vec<f64>,
}

/// One layer: its sheet, its stretches and its nonempty cuts.
pub struct LayerFlows {
    pub sheet: LayerSheet,
    pub stretches: Vec<usize>,
    pub flows: Vec<CutFlow>,
    /// Stretches whose walk through the sheet failed (degenerate curve), and the first reasons.
    pub walk_failed: usize,
    pub why: Vec<String>,
}

/// A measured plan.
pub struct Measure {
    pub layers: Vec<LayerFlows>,
    pub stretches: Vec<Stretch>,
    pub vias: Vec<(WireId, ViaFeat, bool)>,
    pub opts: MeasureOpts,
    pub seconds: f64,
}

/// The static site point of a via of `net` (radius `r`, clearance `s`) in triangle `t`: the first
/// candidate point with room, else the freest one (`false`). Same test as `Packer::via_site`.
fn via_point(pr: &Projection, packer: &Packer, t: usize, net: i32, r: f64, s: f64) -> (Vec2, bool) {
    let [a, b, c] = pr.tris[t].map(|v| pr.pos[v]);
    let pts = tri_site_points(a, b, c);
    let mut best = (f64::NEG_INFINITY, pts[0]);
    for q in pts {
        let mut f = f64::INFINITY;
        for o in pr.obstacles.iter().filter(|o| o.net != Some(net)) {
            let d = if point_in_poly(q, &o.poly) { f64::NEG_INFINITY } else { poly_dist(q, &o.poly) - o.clear.max(s) - packer.eps() };
            f = f.min(d);
        }
        for k in &pr.no_via {
            let d = if point_in_poly(q, k) { f64::NEG_INFINITY } else { poly_dist(q, k) - packer.eps() };
            f = f.min(d);
        }
        if f >= r {
            return (q, true);
        }
        if f > best.0 {
            best = (f, q);
        }
    }
    (best.1, false)
}

/// What [`stretches`] returns: the stretches, the planned vias (wire, disc, has a site), the
/// junction points and the terminal points.
type Planned = (Vec<Stretch>, Vec<(WireId, ViaFeat, bool)>, Vec<JunctionFeat>, Vec<Vec2>);

/// The stretches of the plan, and the planned vias and junction points. O(passes).
#[allow(clippy::needless_range_loop)]
fn stretches(plan: &Plan) -> Planned {
    let st = plan.st;
    let pr = st.proj;
    let mut out = Vec::new();
    let mut vias = Vec::new();
    let mut junc: BTreeMap<VertId, JunctionFeat> = BTreeMap::new();
    let mut terms: Vec<Vec2> = Vec::new();
    for (w, wi) in st.wires.iter().enumerate() {
        if !wi.alive || w >= plan.events.len() {
            continue;
        }
        let ev = &plan.events[w];
        let n = wi.passes.len();
        let mut ks: Vec<usize> = plan.via_k.get(w).cloned().unwrap_or_default();
        // Every layer change has its via traversal (the drawing's default where the plan has none).
        let iv = via_intervals(ev);
        ks.truncate(iv.len());
        for &(ka, kb) in &iv[ks.len()..] {
            ks.push(mid_k(ka, kb));
        }
        let lay = pass_layers_at(n, ev, plan.bottom, &ks);
        // Layer before / after every layer change.
        let changes: Vec<(i32, i32)> = ev.windows(2).filter(|x| x[0].2 != x[1].2).map(|x| (x[0].2, x[1].2)).collect();
        let first = ev.first().map_or(plan.bottom, |e| e.2);
        let w_floor = st.w_min(wi.net, wi.w);
        let r = 0.5 * plan.packer.via_d(wi.net);
        for (end, v, o) in [(0, wi.a, wi.pa), (1, wi.b, wi.pb)] {
            let _ = end;
            if o == NO_OBS || matches!(pr.vkind[v], VertKind::Node) {
                junc.entry(v).or_insert(JunctionFeat { at: pr.pos[v], net: wi.net, clear: wi.s });
            } else {
                terms.push(pr.pos[v]);
            }
        }
        for &(_, v) in &wi.turns {
            junc.entry(v).or_insert(JunctionFeat { at: pr.pos[v], net: wi.net, clear: wi.s });
        }
        // Events in traversal order: (traversal, 0 = turn / 1 = via, point, via index).
        let mut evs: Vec<(usize, u8, Vec2, usize)> = wi.turns.iter().map(|&(k, v)| (k, 0u8, pr.pos[v], 0)).collect();
        for (i, &k) in ks.iter().enumerate() {
            let k = k.min(n);
            let (pt, ok) = via_point(pr, plan.packer, wi.tris[k], wi.net, r, wi.s);
            vias.push((w, ViaFeat { at: pt, r, net: wi.net, clear: wi.s }, ok));
            terms.push(pt);
            evs.push((k, 1, pt, i));
        }
        evs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut cur_layer = first;
        let mut curve = vec![pr.pos[wi.a]];
        let mut pw: Vec<f64> = Vec::new();
        let mut layer_of_cur: Option<i32> = None;
        let mut nosite_cur = false;
        let mut ei = 0;
        let mut flush = |curve: &mut Vec<Vec2>, pw: &mut Vec<f64>, layer: i32, end: Vec2, nosite: bool| {
            curve.push(end);
            if layer >= 0 {
                out.push(Stretch {
                    wire: w,
                    conn: (plan.conn)(w),
                    net: wi.net,
                    layer: layer as usize,
                    s: wi.s,
                    w: wi.w,
                    w_floor,
                    curve: std::mem::take(curve),
                    pass_w: std::mem::take(pw),
                    nosite,
                    band: Vec::new(),
                    wraps: Vec::new(),
                });
            }
        };
        for k in 0..=n {
            while ei < evs.len() && evs[ei].0 == k {
                let (_, kind, pt, vi) = evs[ei];
                let layer = layer_of_cur.unwrap_or(cur_layer);
                let ns = kind == 1 && !vias[vias.len() - ks.len() + vi].2;
                flush(&mut curve, &mut pw, layer, pt, ns || nosite_cur);
                nosite_cur = ns;
                curve.push(pt);
                if kind == 1 {
                    cur_layer = changes.get(vi).map_or(cur_layer, |c| c.1);
                }
                layer_of_cur = None;
                ei += 1;
            }
            if k < n {
                let p = wi.passes[k];
                curve.push(st.pass_point(p));
                pw.push(st.pass_w.get(p).copied().unwrap_or(wi.w));
                layer_of_cur = Some(lay[k]);
            }
        }
        let layer = layer_of_cur.unwrap_or(cur_layer);
        flush(&mut curve, &mut pw, layer, pr.pos[wi.b], nosite_cur);
    }
    (out, vias, junc.into_values().collect(), terms)
}

/// The band of a curve in a sheet: corners and wrapped vertices. `Err` when the walk fails.
#[allow(clippy::type_complexity)]
fn band(sh: &LayerSheet, curve: &[Vec2]) -> Result<(Vec<Vec2>, Vec<Option<(usize, bool)>>), String> {
    let mut c = curve.to_vec();
    c.dedup();
    if c.len() < 2 {
        return Ok((c.clone(), vec![None; c.len()]));
    }
    let mut cr = None;
    let mut why = String::new();
    for attempt in 0..4 {
        match sh.cdt.crossings(&c) {
            Ok(x) => {
                cr = Some(x);
                break;
            }
            Err(e) => {
                why = e;
                // A curve along a sheet edge (two terminals joined by an edge, no pass between):
                // a middle point 2 nm off the line, so it crosses instead of overlapping.
                if c.len() == 2 {
                    let (a, b) = (c[0], c[1]);
                    let n = (b - a).perp().unit().unwrap_or_default();
                    c.insert(1, (a + b) * 0.5 + n * 2.0);
                    continue;
                }
                // A pass point on a sheet edge or vertex: nudge the interior points (nm).
                let n = c.len();
                for (i, p) in c.iter_mut().enumerate().take(n - 1).skip(1) {
                    let k = (attempt + 1) as f64;
                    p.x += 1.7 * k * (1.0 + (i % 3) as f64);
                    p.y += 2.3 * k * (1.0 + (i % 5) as f64);
                }
            }
        }
    }
    let Some(cr) = cr else { return Err(why) };
    let red = crate::cdt::Cdt::reduce(&cr);
    let (a, b) = (c[0], c[c.len() - 1]);
    let mut ports: Vec<(Vec2, Vec2)> = Vec::with_capacity(red.len() + 2);
    ports.push((a, a));
    ports.extend(red.iter().map(|x| (sh.pos[x.left], sh.pos[x.right])));
    ports.push((b, b));
    let corners = funnel(&ports);
    let mut pts = Vec::with_capacity(corners.len());
    let mut wr = Vec::with_capacity(corners.len());
    for (i, &(q, pi)) in corners.iter().enumerate() {
        // A portal end the band passes straight through (it runs along a feature side, e.g. past
        // a row of aligned pads): a corner with no turn, on the side that portal end lies on, so
        // the cuts from that vertex on the band's far side count it (the funnel emits only bends).
        if i > 0 {
            let (a, pa) = (corners[i - 1].0, corners[i - 1].1);
            for (j, x) in red.iter().enumerate().take(pi.saturating_sub(1)).skip(pa) {
                let _ = j;
                for (v, left) in [(x.left, true), (x.right, false)] {
                    let z = sh.pos[v];
                    if z != a && z != q && seg_dist(z, a, q) <= 1.0 && pts.last() != Some(&z) {
                        pts.push(z);
                        wr.push(Some((v, left)));
                    }
                }
            }
        }
        pts.push(q);
        if i == 0 || i + 1 == corners.len() || pi == 0 || pi > red.len() {
            wr.push(None);
            continue;
        }
        let x = red[pi - 1];
        wr.push(Some(if q == sh.pos[x.left] { (x.left, true) } else { (x.right, false) }));
    }
    Ok((pts, wr))
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.len2();
    let t = if l2 > 0.0 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + d * t)).len()
}

/// Clockwise angle from `a` to `b` in `[0, 2 pi)`.
fn cw(a: Vec2, b: Vec2) -> f64 {
    let t = (-a.cross(b)).atan2(a.dot(b));
    if t < 0.0 {
        t + std::f64::consts::TAU
    } else {
        t
    }
}

/// Whether a band that comes in along `din`, leaves along `dout` and has the wrapped vertex on
/// its `left`, crosses the ray from the vertex in direction `u` (the reflex side of the bend).
fn in_wedge(din: Vec2, dout: Vec2, left: bool, u: Vec2) -> bool {
    const TOL: f64 = 1e-9;
    let back = -din;
    if left {
        // The band passes right of the vertex: from `dout` clockwise to `-din`.
        let (a, lim) = (cw(dout, u), cw(dout, back));
        a > TOL && a < lim - TOL
    } else {
        let (a, lim) = (cw(u, dout), cw(back, dout));
        a > TOL && a < lim - TOL
    }
}

/// Measure the plan: sheets per layer (with the planned vias and junctions), bands, cut flows.
pub fn measure(plan: &Plan, outline: &[(Vec2, Vec2)], o: &MeasureOpts, layers: &[usize]) -> Result<Measure, String> {
    let t0 = std::time::Instant::now();
    let pr = plan.st.proj;
    let (mut sts, vias, junc, terms) = stretches(plan);
    let vf: Vec<ViaFeat> = vias.iter().map(|v| v.1).collect();
    let mut out = Vec::new();
    for &l in layers {
        // A junction is copper on the layers of the stretches that end there.
        let jl: Vec<JunctionFeat> = junc
            .iter()
            .filter(|j| sts.iter().any(|s| s.layer == l && (s.curve.first() == Some(&j.at) || s.curve.last() == Some(&j.at))))
            .copied()
            .collect();
        let inp = SheetInput { proj: pr, outline, vias: &vf, junctions: &jl, terminals: &terms };
        let sheet = LayerSheet::build(&inp, l, &o.sheet)?;
        let mine: Vec<usize> = (0..sts.len()).filter(|&i| sts[i].layer == l).collect();
        let mut walk_failed = 0;
        let mut why: Vec<String> = Vec::new();
        for &i in &mine {
            match band(&sheet, &sts[i].curve) {
                Ok((b, w)) => {
                    sts[i].band = b;
                    sts[i].wraps = w;
                }
                Err(e) => {
                    walk_failed += 1;
                    if why.len() < 3 {
                        let p = sts[i].curve[0];
                        why.push(format!("wire {} at ({:.3}, {:.3}): {e}", sts[i].wire, p.x / MM, -p.y / MM));
                    }
                }
            }
        }
        let flows = flows(&sheet, &sts, &mine, pr, plan.st, o);
        out.push(LayerFlows { sheet, stretches: mine, flows, walk_failed, why });
    }
    Ok(Measure { layers: out, stretches: sts, vias, opts: *o, seconds: t0.elapsed().as_secs_f64() })
}

/// The half width of a junction end (a point feature carries the copper of its wires).
fn half_at(sh: &LayerSheet, f: usize, sts: &[Stretch], mine: &[usize]) -> f64 {
    if sh.feats[f].poly.len() != 1 {
        return 0.0;
    }
    let at = sh.feats[f].poly[0];
    mine.iter().map(|&i| &sts[i]).filter(|s| s.curve.first() == Some(&at) || s.curve.last() == Some(&at)).map(|s| 0.5 * s.w).fold(0.0, f64::max)
}

fn flows(sh: &LayerSheet, sts: &[Stretch], mine: &[usize], pr: &Projection, st: &State, o: &MeasureOpts) -> Vec<CutFlow> {
    let _ = st;
    let mut per: BTreeMap<usize, Vec<FlowItem>> = BTreeMap::new();
    let mut buf: Vec<u32> = Vec::new();
    for &si in mine {
        let s = &sts[si];
        if s.band.len() < 2 {
            continue;
        }
        for k in 0..s.band.len() - 1 {
            let (a, b) = (s.band[k], s.band[k + 1]);
            let lo = Vec2 { x: a.x.min(b.x), y: a.y.min(b.y) };
            let hi = Vec2 { x: a.x.max(b.x), y: a.y.max(b.y) };
            sh.cut_grid.query(lo, hi, &mut buf);
            let d = b - a;
            for &ci in buf.iter() {
                let c = &sh.cuts[ci as usize];
                let e = c.q - c.p;
                let den = d.cross(e);
                if den.abs() < 1e-9 * d.len() * e.len() {
                    continue;
                }
                // a + s d = p + t e
                let w0 = c.p - a;
                let sp = w0.cross(e) / den;
                let tc = w0.cross(d) / den;
                let tol = 1.0 / e.len().max(1.0);
                if !(0.0..=1.0).contains(&sp) || tc <= tol || tc >= 1.0 - tol {
                    continue;
                }
                let x = a + d * sp;
                per.entry(ci as usize).or_default().push(FlowItem { stretch: si, t: tc, kind: 0, w: s.width_at(x) });
            }
        }
        // Wraps at the band's interior corners.
        for k in 1..s.band.len().saturating_sub(1) {
            let Some((v, left)) = s.wraps[k] else { continue };
            let (din, dout) = (s.band[k] - s.band[k - 1], s.band[k + 1] - s.band[k]);
            for &ci in &sh.vcuts[v] {
                let c = &sh.cuts[ci];
                let at_p = c.vp == v;
                let u = if at_p { c.q - c.p } else { c.p - c.q };
                if in_wedge(din, dout, left, u) {
                    per.entry(ci).or_default().push(FlowItem { stretch: si, t: if at_p { 0.0 } else { 1.0 }, kind: if at_p { 1 } else { 2 }, w: s.width_at(s.band[k]) });
                }
            }
        }
    }
    let mut out = Vec::with_capacity(per.len());
    for (ci, mut items) in per {
        items.sort_by(|a, b| a.t.total_cmp(&b.t).then(a.stretch.cmp(&b.stretch)));
        let c = &sh.cuts[ci];
        let (bp, bq) = (sh.bound(c.fp), sh.bound(c.fq));
        let regime = pr.regime_at((c.p + c.q) * 0.5);
        let cap = c.cap - half_at(sh, c.fp, sts, mine) - half_at(sh, c.fq, sts, mine);
        let its: Vec<Item> = items.iter().map(|x| Item { net: sts[x.stretch].net, w: x.w, s: sts[x.stretch].s }).collect();
        let smax = its.iter().map(|x| x.s).fold(0.0, f64::max);
        let cons: Vec<Item> = its.iter().map(|x| Item { s: smax, ..*x }).collect();
        let pw = need_in(regime, &its, bp, bq);
        let cv = need_in(regime, &cons, bp, bq);
        let floor: Vec<Item> = if o.pairwise { its.clone() } else { cons.clone() }
            .into_iter()
            .zip(items.iter())
            .map(|(it, x)| Item { w: it.w.min(sts[x.stretch].w_floor), ..it })
            .collect();
        let nf = need_in(regime, &floor, bp, bq);
        let (need, need_other) = if o.pairwise { (pw, cv) } else { (cv, pw) };
        let alone: Vec<f64> = its.iter().map(|it| cap - need_in(regime, std::slice::from_ref(it), bp, bq)).collect();
        out.push(CutFlow { cut: ci, items, cap, need, need_other, need_floor: nf, slack: cap - need, alone });
    }
    out
}

fn bound_txt(b: Bound) -> Value {
    match b {
        Bound::Pad { net, clear } => json!({"pad": net, "clear": r3(clear / MM)}),
        Bound::Hole { clear } => json!({"hole": r3(clear / MM)}),
        Bound::Edge { clear } => json!({"edge": r3(clear / MM)}),
        Bound::Open => json!("open"),
    }
}

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// A point in mm, KiCad frame (y down).
fn kpt(p: Vec2) -> Value {
    json!([r3(p.x / MM), r3(-p.y / MM)])
}

fn stats_json(s: &SheetStats) -> Value {
    json!({
        "features": s.features, "vertices": s.vertices, "triangles": s.triangles,
        "raw": s.raw, "breakpoint": s.breakpoint, "shadowed": s.shadowed, "beyond_radius": s.beyond_radius,
        "kept": s.kept, "capped_walks": s.capped_walks,
        "cdt_s": r3(s.cdt_s), "walk_s": r3(s.walk_s), "shadow_s": r3(s.shadow_s),
    })
}

impl Measure {
    /// Counts per layer (the stage summary of the report).
    pub fn summary(&self, layer_names: &dyn Fn(usize) -> String) -> Value {
        let layers: Vec<Value> = self
            .layers
            .iter()
            .map(|lf| {
                let over = lf.flows.iter().filter(|f| f.slack < -1.0).count();
                let over_other = lf.flows.iter().filter(|f| f.cap - f.need_other < -1.0).count();
                let over_floor = lf.flows.iter().filter(|f| f.cap - f.need_floor < -1.0).count();
                let deficit: f64 = lf.flows.iter().filter(|f| f.slack < 0.0).map(|f| -f.slack).sum();
                json!({
                    "layer": layer_names(lf.sheet.l),
                    "sheet": stats_json(&lf.sheet.stats),
                    "stretches": lf.stretches.len(),
                    "walk_failed": lf.walk_failed,
                    "walk_failed_why": lf.why,
                    "nonempty": lf.flows.len(),
                    "overfull": over,
                    "overfull_other_rule": over_other,
                    "overfull_at_floor": over_floor,
                    "deficit_mm": r3(deficit / MM),
                })
            })
            .collect();
        json!({
            "layers": layers,
            "vias": self.vias.len(),
            "vias_without_site": self.vias.iter().filter(|v| !v.2).count(),
            "seconds": r3(self.seconds),
        })
    }

    /// The full report of the measured plan: per layer every nonempty cut with cap, need, slack
    /// and items; the stretches with their bands; the vias. Points in mm, KiCad frame.
    pub fn full(&self, layer_names: &dyn Fn(usize) -> String) -> Value {
        let layers: Vec<Value> = self
            .layers
            .iter()
            .map(|lf| {
                let sh = &lf.sheet;
                let cuts: Vec<Value> = lf
                    .flows
                    .iter()
                    .map(|f| {
                        let c = &sh.cuts[f.cut];
                        let items: Vec<Value> = f
                            .items
                            .iter()
                            .zip(&f.alone)
                            .map(|(x, al)| {
                                let s = &self.stretches[x.stretch];
                                json!([s.conn.map_or(-1, |c| c as i64), s.net, s.wire, r3(x.w / MM), r3(x.t), x.kind, r3(al / MM), x.stretch])
                            })
                            .collect();
                        json!({
                            "p": kpt(c.p), "q": kpt(c.q),
                            "kind": match c.kind { super::sheet::CutKind::Vertex => "vv", super::sheet::CutKind::Foot => "foot", super::sheet::CutKind::Breakpoint => "bp" },
                            "bp": bound_txt(sh.bound(c.fp)), "bq": bound_txt(sh.bound(c.fq)),
                            "cap": r3(f.cap / MM), "need": r3(f.need / MM), "need_other": r3(f.need_other / MM),
                            "need_floor": r3(f.need_floor / MM), "slack": r3(f.slack / MM),
                            "items": items,
                        })
                    })
                    .collect();
                json!({ "layer": layer_names(sh.l), "cuts": cuts })
            })
            .collect();
        let stretches: Vec<Value> = self
            .stretches
            .iter()
            .enumerate()
            .map(|(i, s)| {
                json!({
                    "id": i, "wire": s.wire, "conn": s.conn.map_or(-1, |c| c as i64), "net": s.net,
                    "layer": layer_names(s.layer), "w": r3(s.w / MM), "s": r3(s.s / MM), "w_floor": r3(s.w_floor / MM), "nosite": s.nosite,
                    "band": s.band.iter().map(|&p| kpt(p)).collect::<Vec<_>>(),
                })
            })
            .collect();
        let vias: Vec<Value> = self.vias.iter().map(|(w, v, ok)| json!({"wire": w, "at": kpt(v.at), "d": r3(2.0 * v.r / MM), "site": ok})).collect();
        json!({ "layers": layers, "stretches": stretches, "vias": vias })
    }
}

/// The `[capacity] cut_report` recorder of one cycle (`route_round`): the U0 static sheets, a
/// summary per stage (after P3, after every P4 round) and the full report of the plan handed to
/// the drawing, written as `<dir>/cycle<c>.json`. Never changes the routing: it reads the State,
/// the labels and the packer's caches only. To keep the debug cost (and its share of a CPU-time
/// budget) low, the static sheets are measured in cycle 1 of run 1 only (they are the same in
/// every cycle) and the stage summaries in run 1 only; every run and cycle gets its final plan.
pub struct Recorder {
    dir: std::path::PathBuf,
    cycle: usize,
    opts: MeasureOpts,
    radius_mm: f64,
    mixed: String,
    outline: Vec<(Vec2, Vec2)>,
    names: Vec<String>,
    n_signal: usize,
    u0: Value,
    stages: Vec<Value>,
    full: Value,
    problems: Vec<String>,
    /// Run 1 (`<cut_report>/run1`): the stage summaries; with cycle 1 also the static sheets.
    first_run: bool,
}

impl Recorder {
    /// `None` when `cut_report` is off.
    pub fn new(model: &crate::model::BoardModel, o: &crate::model::Options, layer_ids: &[u32], n_signal: usize, cycle: usize) -> Option<Recorder> {
        if o.cut_report.is_empty() {
            return None;
        }
        let names = layer_ids.iter().map(|id| model.layers.iter().find(|l| l.id == *id).map_or(format!("layer {id}"), |l| l.name.clone())).collect();
        let dir = std::path::PathBuf::from(&o.cut_report);
        let first_run = dir.file_name().is_none_or(|n| n == "run1");
        Some(Recorder {
            first_run,
            dir,
            cycle,
            opts: MeasureOpts::from_options(o),
            radius_mm: o.cut_radius_mm,
            mixed: o.mixed_clearance.clone(),
            outline: super::board::outline_segments(model),
            names,
            n_signal,
            u0: Value::Null,
            stages: Vec::new(),
            full: Value::Null,
            problems: Vec::new(),
        })
    }

    fn name(&self, l: usize) -> String {
        self.names.get(l).cloned().unwrap_or_else(|| format!("layer {l}"))
    }

    /// U0: the static sheets of the signal layers (no vias, pad centres as terminals), timed
    /// against P2 (`p2_s`, seconds).
    pub fn static_sheets(&mut self, pr: &Projection, p2_s: f64) {
        if !(self.first_run && self.cycle == 1) {
            self.u0 = json!({"p2_s": r3(p2_s)});
            return;
        }
        let t0 = std::time::Instant::now();
        let mut per = Vec::new();
        for l in 0..self.n_signal {
            let terms: Vec<Vec2> = pr.obstacles.iter().filter(|o| o.net.is_some() && (o.drilled || o.layers.has(l))).map(|o| o.center).collect();
            let inp = SheetInput { proj: pr, outline: &self.outline, vias: &[], junctions: &[], terminals: &terms };
            let t = std::time::Instant::now();
            match LayerSheet::build(&inp, l, &self.opts.sheet) {
                Ok(sh) => {
                    let build_s = t.elapsed().as_secs_f64();
                    // The raw critical-cut count needs the walk without shadow pruning (slow).
                    let full = SheetOpts { prune_walk: false, ..self.opts.sheet };
                    let raw = if self.opts.sheet.norm == Norm::Octagonal { LayerSheet::build(&inp, l, &full).ok().map(|s| stats_json(&s.stats)) } else { None };
                    per.push(json!({"layer": self.name(l), "build_s": r3(build_s), "sheet": stats_json(&sh.stats), "unpruned_walk": raw}));
                }
                Err(e) => self.problems.push(format!("static sheet {}: {e}", self.name(l))),
            }
        }
        let _ = t0;
        let total: f64 = per.iter().filter_map(|v| v["build_s"].as_f64()).sum();
        self.u0 = json!({"p2_s": r3(p2_s), "sheets_s": r3(total), "share_of_p2": r3(if p2_s > 0.0 { total / p2_s } else { 0.0 }), "layers": per});
    }

    /// The layers to measure: the signal layers, and a plane layer when a label uses it.
    fn layers(&self, events: &[Vec<LEv>]) -> Vec<usize> {
        let top = events.iter().flatten().map(|e| e.2).max().unwrap_or(0).max(0) as usize;
        (0..self.n_signal.max(top + 1)).collect()
    }

    fn run(&mut self, plan: &Plan) -> Option<Measure> {
        let layers = self.layers(plan.events);
        match measure(plan, &self.outline, &self.opts, &layers) {
            Ok(m) => Some(m),
            Err(e) => {
                self.problems.push(format!("{}: {e}", self.stages.len()));
                None
            }
        }
    }

    /// A stage summary (the via traversals from a fresh capacity model of the labels).
    pub fn stage(&mut self, label: &str, st: &State, events: &[Vec<LEv>], packer: &Packer, bottom: i32, conn: &dyn Fn(WireId) -> Option<usize>) {
        if !self.first_run {
            return;
        }
        let dirs = packer.taut_dirs(st);
        let cm = super::refine::CapModel::new(st, packer, &dirs, events, bottom, 1.0);
        let plan = Plan { st, events, via_k: &cm.via_k, bottom, packer, conn };
        let edge_over = cm.overfull();
        if let Some(m) = self.run(&plan) {
            let names = self.names.clone();
            let mut s = m.summary(&|l| names.get(l).cloned().unwrap_or_default());
            s["stage"] = json!(label);
            s["edge_layers_overfull"] = json!(edge_over);
            self.stages.push(s);
        }
    }

    /// The plan handed to the drawing: summary plus the full report, and the projection's
    /// over-full edge-layers of the per-edge model (for the "model gap" class).
    #[allow(clippy::too_many_arguments)]
    pub fn final_plan(&mut self, st: &State, events: &[Vec<LEv>], via_k: &[Vec<usize>], packer: &Packer, bottom: i32, conn: &dyn Fn(WireId) -> Option<usize>, edge_over: &[(usize, usize)]) {
        let plan = Plan { st, events, via_k, bottom, packer, conn };
        if let Some(m) = self.run(&plan) {
            let names = self.names.clone();
            let nm = |l: usize| names.get(l).cloned().unwrap_or_default();
            let mut s = m.summary(&nm);
            s["stage"] = json!("plan handed to the drawing");
            s["edge_layers_overfull"] = json!(edge_over.len());
            self.stages.push(s);
            let mut f = m.full(&nm);
            let pr = st.proj;
            f["edge_over"] = Value::Array(edge_over.iter().map(|&(e, l)| {
                let [a, b] = pr.edges[e];
                json!({"layer": nm(l), "a": kpt(pr.pos[a]), "b": kpt(pr.pos[b])})
            }).collect());
            self.full = f;
        }
    }

    /// Write `<dir>/cycle<c>.json`; problems go to the returned warnings (never fatal).
    pub fn write(&self) -> Option<String> {
        let v = json!({
            "cycle": self.cycle,
            "norm": format!("{:?}", self.opts.sheet.norm),
            "mixed_clearance": self.mixed,
            "cut_radius_mm": self.radius_mm,
            "routability_proven": self.radius_mm <= 0.0,
            "u0": self.u0,
            "stages": self.stages,
            "final": self.full,
            "problems": self.problems,
        });
        let path = self.dir.join(format!("cycle{}.json", self.cycle));
        let r = std::fs::create_dir_all(&self.dir).and_then(|_| std::fs::write(&path, serde_json::to_vec(&v).unwrap_or_default()));
        r.err().map(|e| format!("cut report {}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wedge_left_turn() {
        // East, then north-east: the vertex is on the left, the band passes south of it.
        let (din, dout) = (Vec2 { x: 1.0, y: 0.0 }, Vec2 { x: 1.0, y: 1.0 });
        assert!(in_wedge(din, dout, true, Vec2 { x: 0.0, y: -1.0 }));
        assert!(in_wedge(din, dout, true, Vec2 { x: -1.0, y: -0.1 }));
        assert!(!in_wedge(din, dout, true, Vec2 { x: 0.0, y: 1.0 }));
        assert!(!in_wedge(din, dout, true, Vec2 { x: 1.0, y: 1.0 }));
        // Mirrored: east then south-east, vertex on the right, the band passes north of it.
        let dout2 = Vec2 { x: 1.0, y: -1.0 };
        assert!(in_wedge(din, dout2, false, Vec2 { x: 0.0, y: 1.0 }));
        assert!(!in_wedge(din, dout2, false, Vec2 { x: 0.0, y: -1.0 }));
    }
}
