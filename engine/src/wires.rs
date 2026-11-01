//! Wire extraction from hand copper (M1 / T1.2a; 20 §4, 23 §4.4).
//!
//! A *wire* is a maximal chain of same-net, same-layer kept items between two *terminals*:
//! a pad (attached at its centre), a via, a junction (vertex of degree >= 3, fixed) or a
//! dangling end (degree 1, fixed). An item endpoint lying on the interior of another item
//! splits that item there, so hand T-junctions without a shared vertex become junctions.
//! Likewise a same-net via or pad whose copper an item crosses mid-span (no endpoint of that
//! item inside it) splits the item at the point nearest the via / pad centre, and the via / pad
//! becomes a terminal there: the taut wire must not leave copper the hand wire connected.
//! Neighbour lookups go through [`Grid`]; every loop is bounded by the item / vertex count.

use crate::geom::quant::round_nm;
use crate::geom::{
    copper_gap_skeleton, dequantise, point_arc, point_seg, Aabb, Arc, Grid, IVec2, Prim, Seg,
    Skeleton, Track, Vec2,
};
use crate::model::{BoardModel, ExistingCopper, Options};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::f64::consts::TAU;

/// Max centreline distance (nm) for an endpoint to count as lying on another item. Integer
/// endpoints of a diagonal can be up to ~0.71 nm off the exact line, hence a tolerance
/// instead of a strict `<= 0`.
const SPLIT_TOL_NM: f64 = 1.0;
/// Arc sampling step of the hand polyline.
const ARC_STEP_DEG: f64 = 10.0;
/// Grid cell size (nm) for the per-net item and pad lookups.
const CELL_NM: f64 = 2_000_000.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Terminal {
    Pad { pad: usize, at: Vec2 },
    Via { via: usize, at: Vec2 },
    Junction { at: Vec2 },
    Dangling { at: Vec2 },
    /// A via site of the router (M3, P6): the end of a leg that changes layer, or the via of a
    /// pour connection. `site` indexes `MultiGraph::sites` (`usize::MAX`: not yet assigned).
    Site { site: usize, at: Vec2 },
    /// Pad exit stub (A6, [`crate::stubs`]): the wire attaches at `end`, the outer end of the
    /// stub that starts at `pad` (inside the pad) on `layer`; a fixed point like a junction.
    Stub { pad: Vec2, end: Vec2, layer: u32 },
}

impl Terminal {
    pub fn at(&self) -> Vec2 {
        match self {
            Terminal::Pad { at, .. }
            | Terminal::Via { at, .. }
            | Terminal::Junction { at }
            | Terminal::Dangling { at }
            | Terminal::Site { at, .. } => *at,
            Terminal::Stub { end, .. } => *end,
        }
    }

    /// Junction and dangling terminals stay where the hand copper put them.
    pub fn is_fixed_point(&self) -> bool {
        matches!(
            self,
            Terminal::Junction { .. } | Terminal::Dangling { .. } | Terminal::Stub { .. }
        )
    }
}

#[derive(Clone, Debug)]
pub struct Wire {
    pub id: usize,
    pub net: i32,
    pub layer: u32,
    pub width: i64,
    pub start: Terminal,
    pub goal: Terminal,
    /// Starts at `start.at()`, ends at `goal.at()`.
    pub hand: Vec<Vec2>,
    pub kept_uuids: Vec<String>,
    pub hand_len: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Extraction {
    pub wires: Vec<Wire>,
    /// `(uuid, reason)`; skipped items stay as hand copper.
    pub skipped: Vec<(String, String)>,
}

/// Piece of a kept item between two vertices (an item is split at T-junction vertices).
struct Piece {
    a: IVec2,
    b: IVec2,
    /// `a`, arc samples strictly between, `b`.
    pts: Vec<Vec2>,
    len: f64,
    item: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum NodeKey {
    Pad(usize),
    Via(usize),
    Vertex(i32, i32),
}

type VKey = (i32, i32);
/// Chain between two terminal vertices: `(piece, traversed a→b)` in order.
type Chain = (VKey, VKey, Vec<(usize, bool)>);

fn key_of(v: IVec2) -> VKey {
    (v.x, v.y)
}

fn skeleton_bbox(sk: &Skeleton) -> Aabb {
    let (lo, hi, r) = match sk {
        Skeleton::Point { p, r } => (*p, *p, *r),
        Skeleton::Seg { s, r } => (
            Vec2 {
                x: s.a.x.min(s.b.x),
                y: s.a.y.min(s.b.y),
            },
            Vec2 {
                x: s.a.x.max(s.b.x),
                y: s.a.y.max(s.b.y),
            },
            *r,
        ),
        Skeleton::Poly { pts, r } => {
            let mut lo = pts.first().copied().unwrap_or(Vec2 { x: 0.0, y: 0.0 });
            let mut hi = lo;
            for p in pts {
                lo = Vec2 {
                    x: lo.x.min(p.x),
                    y: lo.y.min(p.y),
                };
                hi = Vec2 {
                    x: hi.x.max(p.x),
                    y: hi.y.max(p.y),
                };
            }
            (lo, hi, *r)
        }
    };
    Aabb { min: lo, max: hi }.grow(r)
}

fn point_box(p: Vec2, d: f64) -> Aabb {
    Aabb { min: p, max: p }.grow(d)
}

fn prim_of(model: &BoardModel, item: usize) -> Prim {
    dequantise(&model.kept[item].prim)
}

/// Whether `v` lies on the interior of `item`; returns its parameter in `(0, 1)`.
fn interior_param(model: &BoardModel, item: usize, v: IVec2) -> Option<f64> {
    let p = v.to_f64();
    match prim_of(model, item) {
        Prim::Seg(s) => {
            let d = s.b - s.a;
            let l2 = d.len2();
            if l2 == 0.0 || point_seg(p, &s) > SPLIT_TOL_NM {
                return None;
            }
            let t = (p - s.a).dot(d) / l2;
            (t > 0.0 && t < 1.0).then_some(t)
        }
        Prim::Arc(a) => {
            if a.sweep <= 0.0 || point_arc(p, &a) > SPLIT_TOL_NM {
                return None;
            }
            let ang = (a.sigma.sign() * ((p - a.c).angle() - a.a_start)).rem_euclid(TAU);
            let t = ang / a.sweep;
            (t > 0.0 && t < 1.0).then_some(t)
        }
    }
}

/// Point of `item`'s centreline nearest `c`, with its parameter, when strictly interior.
fn nearest_interior(model: &BoardModel, item: usize, c: Vec2) -> Option<(f64, Vec2)> {
    let (t, q) = match prim_of(model, item) {
        Prim::Seg(s) => {
            let d = s.b - s.a;
            let l2 = d.len2();
            if l2 == 0.0 {
                return None;
            }
            let t = (c - s.a).dot(d) / l2;
            (t, s.a + d * t)
        }
        Prim::Arc(a) => {
            let v = c - a.c;
            if a.sweep <= 0.0 || v.len2() == 0.0 {
                return None;
            }
            let t = (a.sigma.sign() * (v.angle() - a.a_start)).rem_euclid(TAU) / a.sweep;
            (t, a.point_at(t))
        }
    };
    (t > 0.0 && t < 1.0).then_some((t, q))
}

fn arc_samples(a: &Arc) -> usize {
    ((a.sweep.to_degrees() / ARC_STEP_DEG).ceil() as usize).max(1)
}

/// Split `item` at the given `(t, vertex)` list (sorted by `t`) into pieces.
fn split_item(model: &BoardModel, item: usize, splits: &[(f64, IVec2)], out: &mut Vec<Piece>) {
    let k = &model.kept[item].prim;
    let prim = prim_of(model, item);
    let mut bounds: Vec<(f64, IVec2)> = vec![(0.0, k.start())];
    bounds.extend_from_slice(splits);
    bounds.push((1.0, k.end()));
    let n = match &prim {
        Prim::Arc(a) => arc_samples(a),
        Prim::Seg(_) => 1,
    };
    for w in bounds.windows(2) {
        let ((t0, a), (t1, b)) = (w[0], w[1]);
        if a == b {
            continue;
        }
        let mut pts = vec![a.to_f64()];
        let len = match &prim {
            Prim::Seg(_) => (b.to_f64() - a.to_f64()).len(),
            Prim::Arc(arc) => {
                for i in 1..n {
                    let f = i as f64 / n as f64;
                    if f > t0 && f < t1 {
                        pts.push(arc.point_at(f));
                    }
                }
                arc.length() * (t1 - t0)
            }
        };
        pts.push(b.to_f64());
        out.push(Piece {
            a,
            b,
            pts,
            len,
            item,
        });
    }
}

/// Everything the per-group extraction needs.
struct Ctx<'a> {
    model: &'a BoardModel,
    pad_grid: Grid<usize>,
    /// Via / pad indices per net.
    vias_of: HashMap<i32, Vec<usize>>,
    pads_of: HashMap<i32, Vec<usize>>,
}

impl Ctx<'_> {
    fn pad_at(&self, net: i32, layer: u32, v: IVec2) -> Option<usize> {
        let p = v.to_f64();
        let mut cands = self.pad_grid.query(point_box(p, 1.0));
        cands.sort_unstable();
        cands.dedup();
        let probe = Track {
            prim: Prim::Seg(Seg { a: p, b: p }),
            w: 0.0,
        };
        cands.into_iter().find(|&i| {
            let pad = &self.model.pads[i];
            pad.net == net
                && (pad.layers.contains(&layer) || pad.shapes.contains_key(&layer))
                && copper_gap_skeleton(&probe, &self.model.skeleton(pad, layer)) <= 0.0
        })
    }
}

fn terminal_of(ctx: &Ctx, key: NodeKey, deg: usize) -> Terminal {
    match key {
        NodeKey::Pad(i) => Terminal::Pad {
            pad: i,
            at: ctx.model.pads[i].center.to_vec2(),
        },
        NodeKey::Via(i) => Terminal::Via {
            via: i,
            at: ctx.model.vias[i].at.to_vec2(),
        },
        NodeKey::Vertex(x, y) => {
            let at = IVec2 { x, y }.to_f64();
            if deg >= 3 {
                Terminal::Junction { at }
            } else {
                Terminal::Dangling { at }
            }
        }
    }
}

fn lex_gt(a: Vec2, b: Vec2) -> bool {
    a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)).is_gt()
}

/// `(item, split vertex, t)` for every item of the group crossing the copper `inside` mid-span
/// with no endpoint inside it; the vertex is the item point nearest `c`, rounded to nm.
fn crossings_of(
    model: &BoardModel,
    grid: &Grid<usize>,
    bb: Aabb,
    c: Vec2,
    inside: impl Fn(&Track) -> bool,
) -> Vec<(usize, IVec2, f64)> {
    let mut cands = grid.query(bb);
    cands.sort_unstable();
    cands.dedup();
    let mut out = Vec::new();
    for j in cands {
        let k = &model.kept[j];
        let dot = |p: IVec2| Track {
            prim: Prim::Seg(Seg {
                a: p.to_f64(),
                b: p.to_f64(),
            }),
            w: 0.0,
        };
        let body = Track {
            prim: prim_of(model, j),
            w: k.width as f64,
        };
        if !inside(&body) || inside(&dot(k.prim.start())) || inside(&dot(k.prim.end())) {
            continue;
        }
        if let Some((t, q)) = nearest_interior(model, j, c) {
            let v = round_nm(q);
            if v != k.prim.start() && v != k.prim.end() {
                out.push((j, v, t));
            }
        }
    }
    out
}

/// Same-net vias on `layer` crossed mid-span by group items (copper overlap).
fn via_contacts(
    ctx: &Ctx,
    net: i32,
    layer: u32,
    grid: &Grid<usize>,
    reach: f64,
) -> Vec<(usize, (usize, IVec2, f64))> {
    let model = ctx.model;
    let mut out = Vec::new();
    for &i in ctx.vias_of.get(&net).map(Vec::as_slice).unwrap_or(&[]) {
        let v = &model.vias[i];
        if !(v.layers.is_empty() || v.layers.contains(&layer)) {
            continue;
        }
        let (c, r) = (v.at.to_vec2(), 0.5 * v.d as f64);
        let disc = Skeleton::Point { p: c, r };
        let hit = |t: &Track| copper_gap_skeleton(t, &disc) < 0.0;
        for x in crossings_of(model, grid, point_box(c, r + reach), c, hit) {
            out.push((i, x));
        }
    }
    out
}

/// Same-net pads on `layer` crossed mid-span by group items whose nearest centreline point
/// to the pad centre lies in the pad (the vertex then resolves to the pad, see `pad_at`).
fn pad_contacts(
    ctx: &Ctx,
    net: i32,
    layer: u32,
    grid: &Grid<usize>,
    reach: f64,
) -> Vec<(usize, (usize, IVec2, f64))> {
    let model = ctx.model;
    let mut out = Vec::new();
    for &i in ctx.pads_of.get(&net).map(Vec::as_slice).unwrap_or(&[]) {
        let p = &model.pads[i];
        if !(p.layers.contains(&layer) || p.shapes.contains_key(&layer)) {
            continue;
        }
        let sk = model.skeleton(p, layer);
        let hit = |t: &Track| copper_gap_skeleton(t, &sk) < 0.0;
        let c = p.center.to_vec2();
        for x in crossings_of(model, grid, skeleton_bbox(&sk).grow(reach), c, hit) {
            if ctx.pad_at(net, layer, x.1) == Some(i) {
                out.push((i, x));
            }
        }
    }
    out
}

/// Extract the wires of one `(net, layer)` group of candidate item indices.
fn extract_group(
    ctx: &Ctx,
    net: i32,
    layer: u32,
    items: &[usize],
    wires: &mut Vec<Wire>,
    skip: &mut Vec<(String, String)>,
) {
    let model = ctx.model;
    // 1. split items at foreign endpoints lying on their interior
    let mut grid: Grid<usize> = Grid::new(CELL_NM);
    let mut verts: BTreeSet<(i32, i32)> = BTreeSet::new();
    for &i in items {
        let k = &model.kept[i].prim;
        grid.insert(prim_of(model, i).bbox().grow(SPLIT_TOL_NM), i);
        verts.insert(key_of(k.start()));
        verts.insert(key_of(k.end()));
    }
    let mut splits: BTreeMap<usize, Vec<(f64, IVec2)>> = BTreeMap::new();
    // 1b. vias / pads crossed mid-span: split there (vertex -> via / pad terminal)
    let mut contact: HashMap<(i32, i32), NodeKey> = HashMap::new();
    let reach = items
        .iter()
        .map(|&i| 0.5 * model.kept[i].width as f64)
        .fold(0.0, f64::max);
    for (i, v) in via_contacts(ctx, net, layer, &grid, reach) {
        contact.insert(key_of(v.1), NodeKey::Via(i));
        verts.insert(key_of(v.1));
        splits.entry(v.0).or_default().push((v.2, v.1));
    }
    for (_, v) in pad_contacts(ctx, net, layer, &grid, reach) {
        verts.insert(key_of(v.1));
        splits.entry(v.0).or_default().push((v.2, v.1));
    }
    for &(x, y) in &verts {
        let v = IVec2 { x, y };
        let mut cands = grid.query(point_box(v.to_f64(), SPLIT_TOL_NM));
        cands.sort_unstable();
        cands.dedup();
        for j in cands {
            let k = &model.kept[j].prim;
            if k.start() == v || k.end() == v {
                continue;
            }
            if let Some(t) = interior_param(model, j, v) {
                splits.entry(j).or_default().push((t, v));
            }
        }
    }
    let mut pieces: Vec<Piece> = Vec::new();
    for &i in items {
        let mut s = splits.remove(&i).unwrap_or_default();
        s.sort_by(|a, b| a.0.total_cmp(&b.0).then(key_of(a.1).cmp(&key_of(b.1))));
        s.dedup_by(|a, b| a.1 == b.1);
        split_item(model, i, &s, &mut pieces);
    }
    // 2. nodes: pad / via / plain vertex
    let via_at: HashMap<(i64, i64), usize> = model
        .vias
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, v)| v.net == net && (v.layers.is_empty() || v.layers.contains(&layer)))
        .map(|(i, v)| ((v.at.x, v.at.y), i))
        .collect();
    let mut key_of_vertex: HashMap<(i32, i32), NodeKey> = HashMap::new();
    for &(x, y) in &verts {
        let v = IVec2 { x, y };
        let key = if let Some(p) = ctx.pad_at(net, layer, v) {
            NodeKey::Pad(p)
        } else if let Some(&k) = contact.get(&(x, y)) {
            k
        } else if let Some(&i) = via_at.get(&(x as i64, y as i64)) {
            NodeKey::Via(i)
        } else {
            NodeKey::Vertex(x, y)
        };
        key_of_vertex.insert((x, y), key);
    }
    let ident = |v: IVec2| key_of_vertex[&key_of(v)];
    // adjacency per vertex (piece index, whether the vertex is the piece's `a` end)
    let mut adj: BTreeMap<(i32, i32), Vec<(usize, bool)>> = BTreeMap::new();
    for (pi, p) in pieces.iter().enumerate() {
        adj.entry(key_of(p.a)).or_default().push((pi, true));
        adj.entry(key_of(p.b)).or_default().push((pi, false));
    }
    let other = |pi: usize, a_end: bool| if a_end { pieces[pi].b } else { pieces[pi].a };
    // From pad vertex `v` (degree 2) leave through `(pi, a_end)`: does the run stay inside the
    // same pad over degree-2 vertices and end on an in-pad dead end (degree 1)?
    let tail_in_pad = |v: (i32, i32), pi: usize, a_end: bool| -> bool {
        let NodeKey::Pad(pad) = key_of_vertex[&v] else {
            return false;
        };
        let (mut piece, mut ae) = (pi, a_end);
        for _ in 0..=verts.len() {
            let w = other(piece, ae);
            if ident(w) != NodeKey::Pad(pad) {
                return false;
            }
            let l = &adj[&key_of(w)];
            match l.len() {
                1 => return true,
                2 => {
                    let Some(&(pn, f)) = l.iter().find(|c| c.0 != piece) else {
                        return false;
                    };
                    (piece, ae) = (pn, f);
                }
                _ => return false,
            }
        }
        false
    };
    // Pad / via vertices are terminals, except degree-2 pad vertices from which a run of
    // in-pad vertices leads to an in-pad dead end (that tail belongs to the wire).
    let is_term = |v: (i32, i32)| {
        let l = &adj[&v];
        match key_of_vertex[&v] {
            NodeKey::Vertex(..) => l.len() != 2,
            NodeKey::Via(_) => true,
            NodeKey::Pad(_) => {
                l.len() != 2 || !(tail_in_pad(v, l[0].0, l[0].1) || tail_in_pad(v, l[1].0, l[1].1))
            }
        }
    };
    // 3. walk chains between terminals
    let mut used = vec![false; pieces.len()];
    let mut chains: Vec<Chain> = Vec::new();
    for (&start, list) in &adj {
        if !is_term(start) {
            continue;
        }
        for &(p0, a_end) in list {
            if used[p0] {
                continue;
            }
            // leave `start` through piece `p0`; `a_end` says `start` is its `a` end
            let mut chain = vec![(p0, a_end)];
            used[p0] = true;
            let mut cur = key_of(other(p0, a_end));
            let end = loop {
                if is_term(cur) {
                    break cur;
                }
                let last = chain.last().map(|c| c.0);
                let Some(&(pn, ae)) = adj[&cur]
                    .iter()
                    .find(|(pi, _)| Some(*pi) != last && !used[*pi])
                else {
                    break cur; // closed ring without terminal: ends where it started
                };
                used[pn] = true;
                chain.push((pn, ae));
                cur = key_of(other(pn, ae));
            };
            chains.push((start, end, chain));
        }
    }
    for (pi, p) in pieces.iter().enumerate() {
        if !used[pi] {
            skip.push((model.kept[p.item].uuid.clone(), "loop".into())); // terminal-free ring
        }
    }
    // 4. build wires
    for (ks, ke, mut chain) in chains {
        let mut uuids: Vec<String> = Vec::new();
        for &(p, _) in &chain {
            let u = &model.kept[pieces[p].item].uuid;
            if uuids.last() != Some(u) {
                uuids.push(u.clone());
            }
        }
        if ident(IVec2 { x: ks.0, y: ks.1 }) == ident(IVec2 { x: ke.0, y: ke.1 }) {
            for u in uuids {
                skip.push((u, "loop".into()));
            }
            continue;
        }
        let term = |v: (i32, i32)| terminal_of(ctx, ident(IVec2 { x: v.0, y: v.1 }), adj[&v].len());
        let (mut ts, mut tg) = (term(ks), term(ke));
        if lex_gt(ts.at(), tg.at()) {
            std::mem::swap(&mut ts, &mut tg);
            chain.reverse();
            for c in &mut chain {
                c.1 = !c.1;
            }
            uuids.reverse();
        }
        let mut hand = vec![ts.at()];
        let mut len = 0.0;
        let mut first_v = None;
        let mut last_v = ts.at();
        for &(p, fwd) in &chain {
            let pc = &pieces[p];
            len += pc.len;
            let seq: Box<dyn Iterator<Item = &Vec2>> = if fwd {
                Box::new(pc.pts.iter())
            } else {
                Box::new(pc.pts.iter().rev())
            };
            for q in seq {
                first_v.get_or_insert(*q);
                hand.push(*q);
                last_v = *q;
            }
        }
        hand.push(tg.at());
        len += (first_v.unwrap_or(ts.at()) - ts.at()).len() + (tg.at() - last_v).len();
        hand.dedup();
        let width = chain
            .iter()
            .map(|c| model.kept[pieces[c.0].item].width)
            .max()
            .unwrap_or(0);
        wires.push(Wire {
            id: 0,
            net,
            layer,
            width,
            start: ts,
            goal: tg,
            hand,
            kept_uuids: uuids,
            hand_len: len,
        });
    }
}

/// Extract the wires of all candidate hand items of `model` (see module docs).
pub fn extract_wires(model: &BoardModel, opts: &Options) -> Extraction {
    let mut out = Extraction::default();
    let smooth = opts.existing_copper == ExistingCopper::Smooth;
    let scope_codes: Option<HashSet<i32>> = (!opts.scope.nets.is_empty()).then(|| {
        model
            .nets
            .iter()
            .filter(|n| opts.scope.nets.contains(&n.name))
            .map(|n| n.code)
            .collect()
    });
    let mut groups: BTreeMap<(i32, u32), Vec<usize>> = BTreeMap::new();
    for (i, k) in model.kept.iter().enumerate() {
        let reason = if !smooth {
            Some("existing_copper fixed")
        } else if k.net == 0 {
            Some("net 0")
        } else if k.locked {
            Some("locked")
        } else {
            None
        };
        if let Some(r) = reason {
            out.skipped.push((k.uuid.clone(), r.into()));
        } else if scope_codes.as_ref().is_none_or(|s| s.contains(&k.net)) {
            groups.entry((k.net, k.layer)).or_default().push(i);
        }
    }
    let mut pad_grid: Grid<usize> = Grid::new(CELL_NM);
    for (i, p) in model.pads.iter().enumerate() {
        if p.net == 0 || groups.is_empty() {
            continue;
        }
        let mut layers: Vec<u32> = p.layers.iter().chain(p.shapes.keys()).copied().collect();
        layers.sort_unstable();
        layers.dedup();
        let bb = layers
            .iter()
            .map(|&l| skeleton_bbox(&model.skeleton(p, l)))
            .reduce(|a, b| a.union(&b));
        if let Some(bb) = bb {
            pad_grid.insert(bb.grow(1.0), i);
        }
    }
    let mut vias_of: HashMap<i32, Vec<usize>> = HashMap::new();
    for (i, v) in model.vias.iter().enumerate() {
        vias_of.entry(v.net).or_default().push(i);
    }
    let mut pads_of: HashMap<i32, Vec<usize>> = HashMap::new();
    for (i, p) in model.pads.iter().enumerate().filter(|(_, p)| p.net != 0) {
        pads_of.entry(p.net).or_default().push(i);
    }
    let ctx = Ctx {
        model,
        pad_grid,
        vias_of,
        pads_of,
    };
    for ((net, layer), items) in &groups {
        extract_group(&ctx, *net, *layer, items, &mut out.wires, &mut out.skipped);
    }
    out.wires.sort_by(|a, b| {
        a.net
            .cmp(&b.net)
            .then(a.layer.cmp(&b.layer))
            .then(a.start.at().x.total_cmp(&b.start.at().x))
            .then(a.start.at().y.total_cmp(&b.start.at().y))
            .then(a.goal.at().x.total_cmp(&b.goal.at().x))
            .then(a.goal.at().y.total_cmp(&b.goal.at().y))
            .then(a.kept_uuids.cmp(&b.kept_uuids))
    });
    for (i, w) in out.wires.iter_mut().enumerate() {
        w.id = i;
    }
    let mut seen = HashSet::new();
    out.skipped.retain(|s| seen.insert(s.clone()));
    out
}
