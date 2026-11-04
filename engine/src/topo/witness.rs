//! R0 witness test: does the P1 projection model accept the hand routings? Every hand track is
//! mapped into the projection (which CDT edges it crosses, where, on which layer), the result is
//! checked against the room model (per edge and layer, per layer combination, allowed layers,
//! obstacle-pair cuts), loaded into a [`State`] (topology, crossings) and handed to P3.
//! The hand copper is DRC-clean, so a violation is a model error except at hand DRC spots.
//!
//! Coordinates are nm in the math frame (Y up); everything written out is in the KiCad frame.
//!
//! Complexity: the walk of a chain is O(triangles it crosses) after an O(1) grid lookup; the
//! edge checks are O(passes · 2^layers); the pair cuts are O(obstacles · neighbours · v²) to build
//! and O(pieces · cuts per cell) to check.

use super::board::{self, Board, BuildOpts};
use super::demand::{self, Bound, Item};
use super::layers::{self, LayerParams};
use super::state::{interleave, point_in_poly, EdgeId, ObsId, State, TriId, VertId, Wire, NO_OBS};
use crate::geom::quant::TrackPrim;
use crate::geom::{point_skeleton, Arc, Vec2};
use crate::model::BoardModel;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Instant;

pub const MM: f64 = 1e6;
/// Tolerance of the point-on-line tests (nm).
const TOL: f64 = 1.0;

#[derive(Clone, Copy, Debug)]
pub struct WitnessOpts {
    pub build: BuildOpts,
    /// Arc sagitta (mm).
    pub sagitta_mm: f64,
    /// Obstacle pairs closer than this make a cut (mm).
    pub pair_mm: f64,
    /// A violation within this distance of a hand DRC spot is a "hand DRC" one (mm).
    pub drc_mm: f64,
    /// A pass with less slack than this is "tight" (mm).
    pub tight_mm: f64,
}

impl Default for WitnessOpts {
    fn default() -> Self {
        WitnessOpts { build: BuildOpts::default(), sagitta_mm: 0.05, pair_mm: 3.0, drc_mm: 0.5, tight_mm: 0.2 }
    }
}

/// One straight piece of hand copper.
#[derive(Clone, Copy, Debug)]
pub struct Piece {
    pub a: Vec2,
    pub b: Vec2,
    pub net: i32,
    /// Index into the signal layers.
    pub layer: usize,
    pub width: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum End {
    Pad(ObsId),
    Free,
}

#[derive(Clone, Copy, Debug)]
pub struct Meta {
    pub layer: usize,
    pub width: f64,
    /// A connecting piece (pad centre to track end, via hop): not hand copper.
    pub virt: bool,
}

/// A run of hand copper from one pad / junction / dead end to the next. Vias stay inside.
#[derive(Clone, Debug)]
pub struct Chain {
    pub net: i32,
    pub pts: Vec<Vec2>,
    /// Piece `i` runs from `pts[i]` to `pts[i + 1]`.
    pub meta: Vec<Meta>,
    pub start: End,
    pub end: End,
    /// Vias inside the chain (layer changes at a via node).
    pub vias: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct HandPass {
    pub edge: EdgeId,
    /// Position along the edge from its vertex `p`, 0..1.
    pub u: f64,
    pub layer: usize,
    pub width: f64,
    /// Arc length along the chain polyline (nm).
    pub s: f64,
    pub at: Vec2,
    pub virt: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Walked {
    pub passes: Vec<HandPass>,
    pub tris: Vec<TriId>,
    /// Layer of the copper in traversal `k` (before its first turn).
    pub lay: Vec<usize>,
    /// Corners where the copper changes layer inside a traversal: `(k, vertex, layer after)`.
    pub turns: Vec<(usize, VertId, usize)>,
    pub a: VertId,
    pub b: VertId,
    pub mapped: f64,
    pub unmapped: f64,
    pub degenerate: usize,
    pub err: Option<String>,
}

// ---------------------------------------------------------------- hand copper -> pieces

fn arc_points(start: Vec2, mid: Vec2, end: Vec2, sag: f64) -> Vec<Vec2> {
    match Arc::from_three_points(start, mid, end) {
        Some(a) => {
            let subs = a.split_sagitta(sag);
            let mut v = vec![start];
            for (i, s) in subs.iter().enumerate() {
                v.push(if i + 1 == subs.len() { end } else { s.end() });
            }
            v
        }
        None => vec![start, end],
    }
}

/// Hand tracks as pieces on signal layers; the total length per non-signal layer id.
pub fn pieces(m: &BoardModel, signal: &[u32], sag_mm: f64) -> (Vec<Piece>, BTreeMap<u32, f64>) {
    let mut out = Vec::new();
    let mut other: BTreeMap<u32, f64> = BTreeMap::new();
    for k in &m.kept {
        let pts: Vec<Vec2> = match k.prim {
            TrackPrim::Seg { a, b } => vec![a.to_f64(), b.to_f64()],
            TrackPrim::Arc { start, mid, end } => {
                arc_points(start.to_f64(), mid.to_f64(), end.to_f64(), sag_mm * MM)
            }
        };
        let len: f64 = pts.windows(2).map(|w| (w[1] - w[0]).len()).sum();
        let Some(li) = signal.iter().position(|&l| l == k.layer) else {
            *other.entry(k.layer).or_default() += len;
            continue;
        };
        for w in pts.windows(2) {
            if (w[1] - w[0]).len() > 0.0 {
                out.push(Piece { a: w[0], b: w[1], net: k.net, layer: li, width: k.width as f64 });
            }
        }
    }
    (out, other)
}

// ---------------------------------------------------------------- chains

struct Dsu(Vec<usize>);
impl Dsu {
    fn find(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.0[r] != r {
            r = self.0[r];
        }
        let mut c = x;
        while self.0[c] != r {
            let n = self.0[c];
            self.0[c] = r;
            c = n;
        }
        r
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb)] = ra.min(rb);
        }
    }
}

fn bbox_of(poly: &[Vec2]) -> (Vec2, Vec2) {
    let (mut lo, mut hi) = (poly[0], poly[0]);
    for p in poly {
        lo = Vec2 { x: lo.x.min(p.x), y: lo.y.min(p.y) };
        hi = Vec2 { x: hi.x.max(p.x), y: hi.y.max(p.y) };
    }
    (lo, hi)
}

/// Chains of the pieces of one net. Nodes: piece ends (by nm coordinate), vias (a piece end
/// within the via radius joins it) and pad nodes (a piece end inside a same-net pad polygon on
/// its layer, or a via centre inside one, joins it). Pad nodes, nodes of degree != 2 are chain
/// ends; vias of degree 2 are inside a chain.
fn net_chains(b: &Board, m: &BoardModel, net: i32, ps: &[Piece], dropped: &mut Dropped) -> Vec<Chain> {
    let pr = &b.proj;
    let pads: Vec<ObsId> = (0..pr.obstacles.len())
        .filter(|&o| pr.obstacles[o].net == Some(net) && b.centre[o].is_some())
        .collect();
    let vias: Vec<usize> = (0..m.vias.len()).filter(|&i| m.vias[i].net == net).collect();
    // Node ids: pads, vias, then piece ends by coordinate.
    let mut key: HashMap<(i64, i64), usize> = HashMap::new();
    let mut pos: Vec<Vec2> = Vec::new();
    let mut lay: Vec<u64> = Vec::new(); // layers of incident pieces
    let mut is_via: Vec<bool> = Vec::new();
    let mut pad_of: Vec<Option<ObsId>> = Vec::new();
    for &o in &pads {
        pos.push(pr.obstacles[o].center);
        lay.push(0);
        is_via.push(false);
        pad_of.push(Some(o));
    }
    let via_base = pos.len();
    for &v in &vias {
        pos.push(m.vias[v].at.to_vec2());
        lay.push(u64::MAX);
        is_via.push(true);
        pad_of.push(None);
    }
    let mut ends: Vec<(usize, usize)> = Vec::new();
    let mut node = |p: Vec2, l: usize, pos: &mut Vec<Vec2>, lay: &mut Vec<u64>, is_via: &mut Vec<bool>, pad_of: &mut Vec<Option<ObsId>>| {
        let k = (p.x.round() as i64, p.y.round() as i64);
        let id = *key.entry(k).or_insert_with(|| {
            pos.push(p);
            lay.push(0);
            is_via.push(false);
            pad_of.push(None);
            pos.len() - 1
        });
        lay[id] |= 1 << l;
        id
    };
    for p in ps {
        let na = node(p.a, p.layer, &mut pos, &mut lay, &mut is_via, &mut pad_of);
        let nb = node(p.b, p.layer, &mut pos, &mut lay, &mut is_via, &mut pad_of);
        ends.push((na, nb));
    }
    let mut dsu = Dsu((0..pos.len()).collect());
    let pad_hit = |p: Vec2, lmask: u64| -> Option<usize> {
        pads.iter().position(|&o| {
            let ob = &pr.obstacles[o];
            (ob.drilled || ob.layers.0 & lmask != 0)
                && (ob.center - p).len() < 1e9
                && point_in_poly(p, &ob.poly)
        })
    };
    for id in 0..pos.len() {
        if id < pads.len() {
            continue;
        }
        if id >= via_base && id < via_base + vias.len() {
            if let Some(i) = pad_hit(pos[id], u64::MAX) {
                dsu.union(i, id);
            }
            continue;
        }
        // Piece end: a via within its radius, else a pad.
        let via = (0..vias.len()).find(|&i| (m.vias[vias[i]].at.to_vec2() - pos[id]).len() <= 0.5 * m.vias[vias[i]].d as f64);
        if let Some(i) = via {
            dsu.union(via_base + i, id);
        }
        if let Some(i) = pad_hit(pos[id], lay[id]) {
            dsu.union(i, id);
        }
    }
    // Representative attributes and graph.
    let n = pos.len();
    let rep: Vec<usize> = (0..n).map(|i| dsu.find(i)).collect();
    let mut rep_pad: Vec<Option<ObsId>> = vec![None; n];
    let mut rep_via = vec![false; n];
    for i in 0..n {
        if pad_of[i].is_some() {
            rep_pad[rep[i]] = pad_of[i];
        }
        if is_via[i] {
            rep_via[rep[i]] = true;
        }
    }
    // Pieces as edges between representatives; drop loops and exact duplicates.
    let mut seen: BTreeSet<(usize, usize, usize)> = BTreeSet::new();
    let mut edges: Vec<(usize, usize, usize)> = Vec::new(); // (ra, rb, piece)
    for (i, &(na, nb)) in ends.iter().enumerate() {
        let (ra, rb) = (rep[na], rep[nb]);
        if ra == rb {
            dropped.in_node += (ps[i].b - ps[i].a).len();
            continue;
        }
        if seen.insert((ra.min(rb), ra.max(rb), ps[i].layer)) {
            edges.push((ra, rb, i));
        } else {
            dropped.duplicate += (ps[i].b - ps[i].a).len();
        }
    }
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n]; // edge ids
    for (ei, &(ra, rb, _)) in edges.iter().enumerate() {
        adj[ra].push(ei);
        adj[rb].push(ei);
    }
    let boundary = |v: usize, adj: &Vec<Vec<usize>>| rep_pad[v].is_some() || adj[v].len() != 2;
    let mut used = vec![false; edges.len()];
    let mut out = Vec::new();
    let mut starts: Vec<usize> = (0..n).filter(|&v| rep[v] == v && !adj[v].is_empty() && boundary(v, &adj)).collect();
    // Leftover cycles: start anywhere on an unused edge (handled after the boundary pass).
    let mut pass = 0;
    loop {
        for &s0 in &starts.clone() {
            for &e0 in &adj[s0].clone() {
                if used[e0] {
                    continue;
                }
                let mut cur = s0;
                let mut ei = e0;
                let mut seq: Vec<(usize, usize, usize)> = Vec::new(); // (edge, from, to)
                loop {
                    used[ei] = true;
                    let (ra, rb, _) = edges[ei];
                    let nxt = if ra == cur { rb } else { ra };
                    seq.push((ei, cur, nxt));
                    if (boundary(nxt, &adj) && pass == 0) || nxt == s0 || adj[nxt].len() != 2 || rep_pad[nxt].is_some() {
                        break;
                    }
                    let other = adj[nxt].iter().copied().find(|&x| x != ei).expect("degree 2");
                    if used[other] {
                        break;
                    }
                    cur = nxt;
                    ei = other;
                }
                out.push(make_chain(b, ps, &edges, &seq, &pos, &rep_pad, &rep_via, net));
            }
        }
        if used.iter().all(|&u| u) {
            break;
        }
        // Remaining edges form cycles of degree-2 nodes.
        starts = (0..edges.len()).filter(|&e| !used[e]).map(|e| edges[e].0).take(1).collect();
        pass += 1;
        if pass > edges.len() + 2 {
            break;
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn make_chain(
    b: &Board,
    ps: &[Piece],
    edges: &[(usize, usize, usize)],
    seq: &[(usize, usize, usize)],
    pos: &[Vec2],
    rep_pad: &[Option<ObsId>],
    rep_via: &[bool],
    net: i32,
) -> Chain {
    let mut pts: Vec<Vec2> = Vec::new();
    let mut meta: Vec<Meta> = Vec::new();
    let mut vias = 0;
    let _ = pos;
    for (i, &(ei, from, to)) in seq.iter().enumerate() {
        let p = &ps[edges[ei].2];
        // Orient the piece: its end nearer to the `from` node's side. Pieces keep their own a/b;
        // the shared node may differ by a snap, so join with a virtual hop.
        let (pa, pb) = {
            let (ra, _rb, _) = edges[ei];
            if ra == from { (p.a, p.b) } else { (p.b, p.a) }
        };
        if pts.is_empty() {
            pts.push(pa);
        } else if (pts[pts.len() - 1] - pa).len() > 0.5 {
            let last = meta.last().map(|m| m.layer).unwrap_or(p.layer);
            meta.push(Meta { layer: last, width: 0.0, virt: true });
            pts.push(pa);
        }
        if i > 0 && rep_via[from] {
            vias += 1;
        }
        meta.push(Meta { layer: p.layer, width: p.width, virt: false });
        pts.push(pb);
        let _ = to;
    }
    let first = seq[0].1;
    let last = seq[seq.len() - 1].2;
    let mut start = End::Free;
    let mut end = End::Free;
    if let Some(o) = rep_pad[first] {
        let c = b.proj.obstacles[o].center;
        if (pts[0] - c).len() > 0.5 {
            pts.insert(0, c);
            meta.insert(0, Meta { layer: meta[0].layer, width: 0.0, virt: true });
        }
        start = End::Pad(o);
    }
    if let Some(o) = rep_pad[last] {
        let c = b.proj.obstacles[o].center;
        if (pts[pts.len() - 1] - c).len() > 0.5 {
            let l = meta[meta.len() - 1].layer;
            pts.push(c);
            meta.push(Meta { layer: l, width: 0.0, virt: true });
        }
        end = End::Pad(o);
    }
    Chain { net, pts, meta, start, end, vias }
}

/// Copper not in any chain (nm): pieces with both ends in one node (inside a pad or a via) and
/// exact duplicates of another piece.
#[derive(Clone, Copy, Debug, Default)]
pub struct Dropped {
    pub in_node: f64,
    pub duplicate: f64,
}

/// All chains of the hand copper, by net.
pub fn chains(b: &Board, m: &BoardModel, ps: &[Piece], dropped: &mut Dropped) -> Vec<Chain> {
    let mut by_net: BTreeMap<i32, Vec<Piece>> = BTreeMap::new();
    for p in ps {
        by_net.entry(p.net).or_default().push(*p);
    }
    let mut out = Vec::new();
    for (net, v) in &by_net {
        out.extend(net_chains(b, m, *net, v, dropped));
    }
    out
}

// ---------------------------------------------------------------- the walk

/// Grid over the triangles for point location.
pub struct Locator {
    lo: Vec2,
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

fn tri_contains(b: &Board, t: TriId, p: Vec2) -> bool {
    let pr = &b.proj;
    (0..3).all(|i| {
        let (a, c) = (pr.pos[pr.tris[t][i]], pr.pos[pr.tris[t][(i + 1) % 3]]);
        let e = c - a;
        e.cross(p - a) / e.len().max(1.0) >= -TOL
    })
}

impl Locator {
    pub fn new(b: &Board) -> Locator {
        let pr = &b.proj;
        let (lo, hi) = bbox_of(&pr.pos);
        let cell = 2.0 * MM;
        let nx = ((hi.x - lo.x) / cell).floor() as usize + 1;
        let ny = ((hi.y - lo.y) / cell).floor() as usize + 1;
        let mut cells = vec![Vec::new(); nx * ny];
        for (t, f) in pr.tris.iter().enumerate() {
            let (tl, th) = bbox_of(&[pr.pos[f[0]], pr.pos[f[1]], pr.pos[f[2]]]);
            let (x0, x1) = (((tl.x - lo.x) / cell) as usize, ((th.x - lo.x) / cell) as usize);
            let (y0, y1) = (((tl.y - lo.y) / cell) as usize, ((th.y - lo.y) / cell) as usize);
            for y in y0..=y1.min(ny - 1) {
                for x in x0..=x1.min(nx - 1) {
                    cells[y * nx + x].push(t as u32);
                }
            }
        }
        Locator { lo, cell, nx, ny, cells }
    }

    pub fn locate(&self, b: &Board, p: Vec2) -> Option<TriId> {
        let (x, y) = (((p.x - self.lo.x) / self.cell).floor(), ((p.y - self.lo.y) / self.cell).floor());
        if x < 0.0 || y < 0.0 || x as usize >= self.nx || y as usize >= self.ny {
            return None;
        }
        self.cells[y as usize * self.nx + x as usize].iter().map(|&t| t as usize).find(|&t| tri_contains(b, t, p))
    }
}

/// Walk the polyline of `ch` through the projection. A vertex within 1 nm of the line counts as
/// on its left side (the nudge), once per vertex as `degenerate`.
pub fn walk(b: &Board, loc: &Locator, ch: &Chain) -> Walked {
    let pr = &b.proj;
    let mut w = Walked::default();
    let fail = |mut w: Walked, e: String| {
        w.err = Some(e);
        w
    };
    // Start triangle.
    let (mut t, mut start_vert) = match ch.start {
        End::Pad(o) => {
            let v = b.centre[o].expect("pad centre");
            let d = ch.pts[1] - ch.pts[0];
            let a = pr.pos[v];
            let pick = wedge(b, v, a, d);
            (pick.unwrap_or(pr.vert_tris[v][0]), Some(v))
        }
        End::Free => match pr.vertex_at(ch.pts[0], TOL) {
            // A node terminal: a corner like a pad centre.
            Some(v) => (wedge(b, v, pr.pos[v], ch.pts[1] - ch.pts[0]).unwrap_or(pr.vert_tris[v][0]), Some(v)),
            None => match loc.locate(b, ch.pts[0]) {
                Some(t) => (t, None),
                None => return fail(w, "start outside the projection".into()),
            },
        },
    };
    w.tris.push(t);
    w.lay.push(ch.meta[0].layer);
    let mut hits: BTreeSet<VertId> = BTreeSet::new();
    let mut s_base = 0.0;
    for i in 0..ch.pts.len() - 1 {
        let (a, bb, mt) = (ch.pts[i], ch.pts[i + 1], ch.meta[i]);
        let d = bb - a;
        let dl = d.len();
        // A layer change sits at a node vertex: when the next piece leaves into the triangle we
        // are in, the traversal turns at that corner; otherwise the walk rotates around the
        // vertex to the triangle it leaves into (zero-length passes on the fan edges).
        if i > 0 && mt.layer != ch.meta[i - 1].layer {
            if let Some(v) = pr.tris[t].iter().copied().find(|&v| (pr.pos[v] - a).len() <= TOL) {
                if let Some(nt) = wedge(b, v, a, d) {
                    if nt == t {
                        w.turns.push((w.tris.len() - 1, v, mt.layer));
                    } else {
                        let Some(hops) = fan_path(b, v, t, nt) else {
                            return fail(w, format!("no fan path at node {v} on piece {i}"));
                        };
                        for (e, tt) in hops {
                            let end = if pr.edges[e][0] == v { 0.0 } else { 1.0 };
                            w.passes.push(HandPass { edge: e, u: end, layer: mt.layer, width: mt.width, s: s_base, at: a, virt: mt.virt });
                            w.tris.push(tt);
                            w.lay.push(mt.layer);
                        }
                        t = nt;
                    }
                    hits.insert(v);
                    start_vert = Some(v);
                }
            }
        }
        let mut entry: Option<EdgeId> = None;
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > 200_000 {
                return fail(w, "walk does not end".into());
            }
            if tri_contains(b, t, bb) {
                break;
            }
            let mut best: Option<(f64, EdgeId, f64, VertId, VertId)> = None;
            for k in 0..3 {
                let e = pr.tri_edges[t][k];
                let (v0, v1) = (pr.tris[t][k], pr.tris[t][(k + 1) % 3]);
                if Some(e) == entry || Some(v0) == start_vert || Some(v1) == start_vert {
                    continue;
                }
                let (o0, o1) = (d.cross(pr.pos[v0] - a) / dl, d.cross(pr.pos[v1] - a) / dl);
                for (v, o) in [(v0, o0), (v1, o1)] {
                    if o.abs() <= TOL && (pr.pos[v] - a).len() > TOL && (pr.pos[v] - bb).len() > TOL {
                        let l = (pr.pos[v] - a).dot(d) / (dl * dl);
                        if l > 0.0 && l < 1.0 {
                            hits.insert(v);
                        }
                    }
                }
                if (o0 > -TOL) == (o1 > -TOL) {
                    continue;
                }
                let ev = pr.pos[v1] - pr.pos[v0];
                let den = d.cross(ev);
                if den.abs() < 1e-9 {
                    continue;
                }
                let lam = (pr.pos[v0] - a).cross(ev) / den;
                let mu = (pr.pos[v0] - a).cross(d) / den;
                if best.is_none_or(|x| lam > x.0) {
                    best = Some((lam, e, mu, v0, v1));
                }
            }
            let Some((lam, e, mu, v0, _)) = best else {
                // `a` sits on the boundary of `t` and the ray leaves at once: reseat at the vertex
                // (wedge test) or across the edge it lies on.
                if guard < 8 {
                    if let Some(&v) = pr.tris[t].iter().find(|&&v| (pr.pos[v] - a).len() <= TOL) {
                        if let Some(nt) = wedge(b, v, a, d) {
                            if nt != t || start_vert != Some(v) {
                                // Rotate around `v` from `t` to `nt`, crossing the fan edges.
                                let Some(hops) = fan_path(b, v, t, nt) else {
                                    return fail(w, format!("no fan path at vertex {v} on piece {i}"));
                                };
                                for (e, tt) in hops {
                                    let end = if pr.edges[e][0] == v { 0.0 } else { 1.0 };
                                    w.passes.push(HandPass { edge: e, u: end, layer: mt.layer, width: mt.width, s: s_base, at: a, virt: mt.virt });
                                    w.tris.push(tt);
                                    w.lay.push(mt.layer);
                                    hits.insert(v);
                                }
                                t = nt;
                                start_vert = Some(v);
                                entry = None;
                                continue;
                            }
                        }
                    } else if let Some(&e) = pr.tri_edges[t].iter().find(|&&e| {
                        let [p, q] = pr.edges[e];
                        let (pp, qq) = (pr.pos[p], pr.pos[q]);
                        (qq - pp).cross(a - pp).abs() / (qq - pp).len().max(1.0) <= TOL
                    }) {
                        if let Some(nt) = pr.across(e, t) {
                            t = nt;
                            entry = Some(e);
                            w.tris.push(t);
                            w.lay.push(mt.layer);
                            w.passes.push(HandPass { edge: e, u: edge_u(b, e, a), layer: mt.layer, width: mt.width, s: s_base, at: a, virt: mt.virt });
                            continue;
                        }
                    }
                }
                return fail(w, format!("no exit from triangle {t} on piece {i} (start vertex {start_vert:?})"));
            };
            if lam >= 1.0 {
                break;
            }
            let lam = lam.max(0.0);
            let u = if pr.edges[e][0] == v0 { mu } else { 1.0 - mu };
            w.passes.push(HandPass {
                edge: e,
                u: u.clamp(0.0, 1.0),
                layer: mt.layer,
                width: mt.width,
                s: s_base + lam * dl,
                at: a + d * lam,
                virt: mt.virt,
            });
            let Some(nt) = pr.across(e, t) else {
                w.unmapped += if mt.virt { 0.0 } else { (1.0 - lam) * dl };
                let msg = format!("left the projection on piece {i}/{} at ({:.3},{:.3}) mm, layer {} (pieces' layers {:?})", ch.meta.len(), a.x / MM, -a.y / MM, mt.layer, ch.meta.iter().map(|m| m.layer).collect::<Vec<_>>());
                return fail(w, msg);
            };
            entry = Some(e);
            start_vert = None;
            t = nt;
            w.tris.push(t);
            w.lay.push(mt.layer);
        }
        start_vert = None;
        s_base += dl;
        if !mt.virt {
            w.mapped += dl;
        }
    }
    w.degenerate = hits.len();
    w.a = match ch.start {
        End::Pad(o) => b.centre[o].expect("pad centre"),
        End::Free => pr.vertex_at(ch.pts[0], TOL).unwrap_or_else(|| nearest_corner(b, w.tris[0], ch.pts[0])),
    };
    w.b = match ch.end {
        End::Pad(o) => b.centre[o].expect("pad centre"),
        End::Free => pr.vertex_at(ch.pts[ch.pts.len() - 1], TOL).unwrap_or_else(|| nearest_corner(b, t, ch.pts[ch.pts.len() - 1])),
    };
    if !pr.tris[t].contains(&w.b) || !pr.tris[w.tris[0]].contains(&w.a) {
        let which = if !pr.tris[t].contains(&w.b) { "end" } else { "start" };
        let msg = format!("{which} triangle lacks the terminal ({:?} -> {:?}, {} pts, {} passes)", ch.start, ch.end, ch.pts.len(), w.passes.len());
        return fail(w, msg);
    }
    w
}

/// The triangle around vertex `v` (at `a`) that the ray along `d` enters. A vertex on the line
/// counts as left of it (the nudge).
fn wedge(b: &Board, v: VertId, a: Vec2, d: Vec2) -> Option<TriId> {
    let pr = &b.proj;
    let dl = d.len();
    let left = |q: Vec2| d.cross(q - a) / dl > -TOL;
    pr.vert_tris[v].iter().copied().find(|&t| {
        let i = pr.tris[t].iter().position(|&x| x == v).expect("corner");
        let (u, x) = (pr.pos[pr.tris[t][(i + 1) % 3]], pr.pos[pr.tris[t][(i + 2) % 3]]);
        !left(u) && left(x)
    })
}

/// Hops `(edge, triangle entered)` around vertex `v` from triangle `from` to `to` (BFS over
/// the triangles of the fan).
fn fan_path(b: &Board, v: VertId, from: TriId, to: TriId) -> Option<Vec<(EdgeId, TriId)>> {
    let pr = &b.proj;
    let mut prev: BTreeMap<TriId, (TriId, EdgeId)> = BTreeMap::new();
    let mut queue = std::collections::VecDeque::from([from]);
    let mut seen: BTreeSet<TriId> = BTreeSet::from([from]);
    while let Some(t) = queue.pop_front() {
        if t == to {
            break;
        }
        for &e in &pr.tri_edges[t] {
            if !pr.edges[e].contains(&v) {
                continue;
            }
            if let Some(n) = pr.across(e, t) {
                if seen.insert(n) {
                    prev.insert(n, (t, e));
                    queue.push_back(n);
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut cur = to;
    while cur != from {
        let &(p, e) = prev.get(&cur)?;
        out.push((e, cur));
        cur = p;
    }
    out.reverse();
    Some(out)
}

/// Position of `p` along edge `e` from its vertex `p`, 0..1.
fn edge_u(b: &Board, e: EdgeId, p: Vec2) -> f64 {
    let [x, y] = b.proj.edges[e];
    let d = b.proj.pos[y] - b.proj.pos[x];
    ((p - b.proj.pos[x]).dot(d) / d.len2().max(1.0)).clamp(0.0, 1.0)
}

fn nearest_corner(b: &Board, t: TriId, p: Vec2) -> VertId {
    let pr = &b.proj;
    *pr.tris[t].iter().min_by(|&&x, &&y| (pr.pos[x] - p).len().total_cmp(&(pr.pos[y] - p).len())).expect("corner")
}

// ---------------------------------------------------------------- results

/// One pass of a violated cut, for the report.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct PassRec {
    pub net: i32,
    pub layer: usize,
    pub width_mm: f64,
    pub clear_mm: f64,
    /// Position across the cut from its first end (mm).
    pub pos_mm: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Violation {
    pub id: usize,
    /// edge_fit, layer_combo, allowed, pair_fit or same_layer.
    pub check: &'static str,
    pub edge: Option<usize>,
    pub cut: Option<usize>,
    /// Signal layer index, or the layer mask for a combination.
    pub layer: u64,
    pub nets: Vec<i32>,
    pub need_mm: f64,
    pub have_mm: f64,
    /// KiCad frame, nm.
    pub at: [f64; 2],
    pub cause: String,
    pub hand_drc: bool,
    /// The violated cut (CDT edge or pair cut) in the KiCad frame, nm.
    pub line: Option<[[f64; 2]; 2]>,
    /// The passes that make up the demand, ordered across the cut.
    pub passes: Vec<PassRec>,
    /// The two ends of the cut (pad `REF.number nNET`, `outline`, `free`).
    pub ends: [String; 2],
    /// Cut length minus the distance of both ends' vertices from the true copper (mm): the
    /// copper-to-copper gap. Equal to `have_mm` unless a vertex lies outside its copper.
    pub true_gap_mm: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct CheckCounts {
    pub edge_fit: usize,
    pub layer_combo: usize,
    pub allowed: usize,
    pub pair_fit: usize,
    pub same_layer: usize,
    /// The same counts, but only the ones inside a hand DRC spot.
    pub hand_drc: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Summary {
    pub board: String,
    pub signal_layers: usize,
    pub chains: usize,
    pub chains_failed: usize,
    pub signal_len_mm: f64,
    pub mapped_len_mm: f64,
    pub mapped_pct: f64,
    /// Copper left out of the chains (mm): inside a pad / via node, and exact duplicates.
    pub in_node_len_mm: f64,
    pub duplicate_len_mm: f64,
    pub nonsignal_len_mm: BTreeMap<u32, f64>,
    pub degenerate: usize,
    pub passes: usize,
    pub counts: CheckCounts,
    pub edge_cuts_checked: usize,
    pub pair_cuts: usize,
    pub pair_cuts_with_traffic: usize,
    /// Pair cuts that are a CDT edge (both ends at vertices joined by an edge): the LCT cuts.
    pub pair_cuts_as_edges: usize,
    /// Of the other pair cuts: those whose segment runs through a third obstacle (shadowed).
    pub pair_cuts_shadowed: usize,
    pub tight_passes: usize,
    pub hand_drc_spots: usize,
    pub topo_wires: usize,
    pub topo_excluded: usize,
    pub state_check: String,
    pub crossings: usize,
    pub same_layer_crossings: usize,
    /// Pairs of traversals the topology could not decide and left to the geometric fallback: 0 by
    /// construction (every pair is derived from the State; see [`crossings_report`]).
    pub fallback_pairs: usize,
    /// Pairs where the geometric cross-check disagreed with the topology (must be 0).
    pub crosscheck_mismatch: usize,
    pub crosscheck_mismatch_at_drc: usize,
    pub crosscheck_mismatch_contact: usize,
    pub missed_same_layer: usize,
    /// Node vertices inserted for vias / free ends, and traversals that turn at a node corner.
    pub nodes: usize,
    pub turns: usize,
    pub p3_ran: bool,
    pub p3_vias: usize,
    pub p3_unlayerable: usize,
    pub p3_same_layer: usize,
    pub hand_vias: usize,
    pub hand_signal_vias: usize,
    pub g0_pair_violations: Option<usize>,
    /// Of those, the violated cuts that are CDT edges (P2 sees them) and those with need above
    /// twice the cut (no distribution over two layers fits).
    pub g0_pair_violations_on_edges: Option<usize>,
    pub g0_pair_violations_over_2x: Option<usize>,
    pub g0_pair_cuts_with_traffic: Option<usize>,
    pub build_s: f64,
    pub walk_s: f64,
    pub fit_s: f64,
    pub topo_s: f64,
    pub runtime_s: f64,
}

pub struct Witness {
    pub summary: Summary,
    pub violations: Vec<Violation>,
    pub chains: Vec<Chain>,
    pub walked: Vec<Walked>,
    /// Per chain: index into the State's wires, or None.
    pub wire_of: Vec<Option<usize>>,
    /// Different-net crossings per State wire (index = wire id).
    pub wire_crossings: Vec<usize>,
    /// Pair-cut violations of the G0 routing (`g0` given), with their passes.
    pub g0_violations: Vec<Violation>,
    pub board: Board,
}

pub fn kicad(p: Vec2) -> [f64; 2] {
    [p.x, -p.y]
}

// ---------------------------------------------------------------- fit

#[derive(Clone, Copy, Debug)]
struct FPass {
    u: f64,
    layer: usize,
    width: f64,
    clear: f64,
    net: i32,
    mask: u64,
    at: Vec2,
}

/// Where the board's own kicad-cli DRC reports a copper fault (math frame, nm): item positions
/// (pads, vias, zones) and whole hand tracks (the position of a track item is its start point, the
/// fault can be anywhere along it).
#[derive(Clone, Debug, Default)]
pub struct Spots {
    pub points: Vec<[f64; 2]>,
    /// Flagged hand tracks: `(a, b, width)`.
    pub tracks: Vec<(Vec2, Vec2, f64)>,
}

impl Spots {
    pub fn len(&self) -> usize {
        self.points.len()
    }
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
    /// A point within `r` of a reported position, or on a flagged track.
    pub fn near(&self, p: Vec2, r: f64) -> bool {
        self.points.iter().any(|s| (Vec2 { x: s[0], y: s[1] } - p).len() <= r)
            || self.tracks.iter().any(|&(a, b, w)| (p - seg_point(p, a, b)).len() <= 0.5 * w + 1.0e3)
    }
}

fn near_drc(spots: &Spots, p: Vec2, r: f64) -> bool {
    spots.near(p, r)
}

/// Name of the object at CDT vertex `v` for the report.
fn vert_name(b: &Board, m: &BoardModel, v: VertId) -> String {
    match b.proj.vkind[v] {
        super::state::VertKind::Outline(o) | super::state::VertKind::Centre(o) => {
            let pd = &m.pads[b.pad_of[o]];
            format!("{}.{} n{}", pd.ref_, pd.number, pd.net)
        }
        super::state::VertKind::Edge => "outline".into(),
        super::state::VertKind::Free | super::state::VertKind::Node => "free".into(),
    }
}

/// How far (nm) vertex `v` lies outside the copper of its pad (0 for the outline); the true
/// copper-to-copper gap of a cut is its length plus this at both ends.
fn vert_outside(b: &Board, m: &BoardModel, v: VertId, l: usize) -> f64 {
    match b.proj.vkind[v] {
        super::state::VertKind::Outline(o) => {
            let pad = &m.pads[b.pad_of[o]];
            point_skeleton(b.proj.pos[v], &m.skeleton(pad, b.signal_layers.get(l).copied().unwrap_or(0)))
        }
        _ => 0.0,
    }
}

fn allowed_cause(b: &Board, m: &BoardModel, ts: [TriId; 2], f: &FPass) -> String {
    let pr = &b.proj;
    let mut best: Option<(f64, f64)> = None; // (copper distance, polygon overshoot)
    for t in ts {
        if pr.tri_out[t] {
            return "outside the board outline (hand track beyond the outline)".into();
        }
        for &o in &pr.tri_obs[t] {
            let ob = &pr.obstacles[o];
            if ob.net == Some(f.net) || !(ob.drilled || ob.layers.has(f.layer)) {
                continue;
            }
            let pad = &m.pads[b.pad_of[o]];
            let lid = b.signal_layers[f.layer];
            let d = point_skeleton(f.at, &m.skeleton(pad, lid));
            if best.is_none_or(|x| d > x.0) {
                best = Some((d, 0.0));
            }
        }
    }
    match best {
        Some((d, _)) if d >= 0.5 * f.width => {
            format!("pad polygon size: track centre {:.3} mm clear of the pad copper, inside its polygon", d / MM)
        }
        Some((d, _)) => format!("mapping or hand DRC: track centre {:.3} mm from the pad copper", d / MM),
        None => "wall (drilled obstacle)".into(),
    }
}

pub struct FitOut {
    pub violations: Vec<Violation>,
    pub tight: usize,
    pub edges_checked: usize,
}

/// Fit of the walked hand against the CDT-edge cuts: per layer, per layer combination and the
/// allowed layers. Same-net duplicates (overlap within half a track width) count once.
pub fn fit_edges(
    b: &Board,
    m: &BoardModel,
    chains: &[Chain],
    walked: &[Walked],
    spots: &Spots,
    o: &WitnessOpts,
) -> FitOut {
    let pr = &b.proj;
    let nl = pr.n_layers;
    let mut by_edge: BTreeMap<EdgeId, Vec<FPass>> = BTreeMap::new();
    let mut out = FitOut { violations: Vec::new(), tight: 0, edges_checked: 0 };
    let mut seen_allowed: BTreeSet<(EdgeId, usize, i32)> = BTreeSet::new();
    let ends_of = |e: EdgeId| [vert_name(b, m, pr.edges[e][0]), vert_name(b, m, pr.edges[e][1])];
    for (ch, wk) in chains.iter().zip(walked) {
        if wk.err.is_some() && wk.passes.is_empty() {
            continue;
        }
        let clear = m.rules.net_rule(ch.net).clearance as f64;
        for (k, p) in wk.passes.iter().enumerate().take(wk.tris.len().saturating_sub(1)) {
            if p.virt {
                continue;
            }
            let (t1, t2) = (wk.tris[k], wk.tris[k + 1]);
            let (m1, m2) = (pr.allowed(t1, ch.net), pr.allowed(t2, ch.net));
            let mask = match (m1, m2) {
                (Some(a), Some(c)) => a.0 & c.0,
                _ => 0,
            };
            let f = FPass { u: p.u, layer: p.layer, width: p.width, clear, net: ch.net, mask, at: p.at };
            if mask >> p.layer & 1 == 0 && seen_allowed.insert((p.edge, p.layer, ch.net)) {
                let cause = allowed_cause(b, m, [t1, t2], &f);
                let e = pr.edges[p.edge];
                let id = out.violations.len();
                out.violations.push(Violation {
                    id,
                    check: "allowed",
                    edge: Some(p.edge),
                    cut: None,
                    layer: p.layer as u64,
                    nets: vec![ch.net],
                    need_mm: p.width / MM,
                    have_mm: 0.0,
                    at: kicad(p.at),
                    cause,
                    hand_drc: near_drc(spots, p.at, o.drc_mm * MM),
                    line: Some([kicad(pr.pos[e[0]]), kicad(pr.pos[e[1]])]),
                    passes: vec![PassRec { net: ch.net, layer: p.layer, width_mm: p.width / MM, clear_mm: clear / MM, pos_mm: p.u * (pr.pos[e[1]] - pr.pos[e[0]]).len() / MM }],
                    ends: ends_of(p.edge),
                    true_gap_mm: 0.0,
                });
            }
            by_edge.entry(p.edge).or_default().push(f);
        }
    }
    let combos: Vec<u64> = if nl <= 4 { (1u64..(1 << nl)).filter(|s| s.count_ones() >= 2).collect() } else { Vec::new() };
    for (&e, ps) in &mut by_edge {
        out.edges_checked += 1;
        let [vp, vq] = pr.edges[e];
        let len = (pr.pos[vq] - pr.pos[vp]).len();
        // Dedupe: same net, same layer, within half a width along the edge.
        ps.sort_by(|a, c| a.layer.cmp(&c.layer).then(a.net.cmp(&c.net)).then(a.u.total_cmp(&c.u)));
        let mut d: Vec<FPass> = Vec::new();
        for f in ps.iter() {
            if let Some(l) = d.last_mut() {
                if l.layer == f.layer && l.net == f.net && (f.u - l.u).abs() * len < 0.5 * l.width.max(f.width) {
                    l.width = l.width.max(f.width);
                    continue;
                }
            }
            d.push(*f);
        }
        let line = [pr.pos[vp], pr.pos[vq]];
                let item = |f: &FPass| Item { net: f.net, w: f.width, s: f.clear };
        for l in 0..nl {
            let mut on: Vec<&FPass> = d.iter().filter(|f| f.layer == l).collect();
            if on.is_empty() {
                continue;
            }
            on.sort_by(|a, c| a.u.total_cmp(&c.u));
            let items: Vec<Item> = on.iter().map(|f| item(f)).collect();
            let need = pr.edge_demand(e, &items);
            let have = pr.room[e][l];
            if !have.is_finite() {
                continue;
            }
            if have - need < o.tight_mm * MM {
                out.tight += on.len();
            }
            if need > have + TOL {
                let id = out.violations.len();
                let at = on[0].at;
                out.violations.push(Violation {
                    id,
                    check: "edge_fit",
                    edge: Some(e),
                    cut: None,
                    layer: l as u64,
                    nets: on.iter().map(|f| f.net).collect(),
                    need_mm: need / MM,
                    have_mm: have / MM,
                    at: kicad(at),
                    cause: String::new(),
                    hand_drc: near_drc(spots, at, o.drc_mm * MM) || on.iter().any(|f| near_drc(spots, f.at, o.drc_mm * MM)),
                    line: Some([kicad(line[0]), kicad(line[1])]),
                    passes: on.iter().map(|f| PassRec { net: f.net, layer: l, width_mm: f.width / MM, clear_mm: f.clear / MM, pos_mm: f.u * len / MM }).collect(),
                    ends: ends_of(e),
                    true_gap_mm: (have + vert_outside(b, m, vp, l) + vert_outside(b, m, vq, l)) / MM,
                });
            }
        }
        for &s in &combos {
            let inn: Vec<&FPass> = d.iter().filter(|f| (f.mask | 1 << f.layer) & !s == 0).collect();
            if inn.is_empty() {
                continue;
            }
            let have: f64 = (0..nl).filter(|&l| s >> l & 1 == 1).map(|l| pr.room[e][l]).sum();
            let items: Vec<Item> = inn.iter().map(|f| item(f)).collect();
            let need = demand::need_lower(&items, s.count_ones() as usize);
            // A combination that holds the passes of one layer only is the per-layer check.
            let layers_used: BTreeSet<usize> = inn.iter().map(|f| f.layer).collect();
            let multi = layers_used.len() >= 2 || inn.iter().any(|f| f.mask.count_ones() >= 2);
            if multi && have.is_finite() && need > have + TOL {
                let id = out.violations.len();
                out.violations.push(Violation {
                    id,
                    check: "layer_combo",
                    edge: Some(e),
                    cut: None,
                    layer: s,
                    nets: inn.iter().map(|f| f.net).collect(),
                    need_mm: need / MM,
                    have_mm: have / MM,
                    at: kicad(d[0].at),
                    cause: String::new(),
                    hand_drc: near_drc(spots, d[0].at, o.drc_mm * MM) || inn.iter().any(|f| near_drc(spots, f.at, o.drc_mm * MM)),
                    line: Some([kicad(line[0]), kicad(line[1])]),
                    passes: inn.iter().map(|f| PassRec { net: f.net, layer: f.layer, width_mm: f.width / MM, clear_mm: f.clear / MM, pos_mm: f.u * len / MM }).collect(),
                    ends: ends_of(e),
                    true_gap_mm: have / MM,
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------- pair cuts

#[derive(Clone, Debug)]
pub struct Cut {
    pub a: Vec2,
    pub b: Vec2,
    /// Obstacle ids of the two ends; `None` = board outline.
    pub ends: [Option<ObsId>; 2],
    /// Layers where both ends block.
    pub layers: u64,
}

impl Cut {
    pub fn len(&self) -> f64 {
        (self.b - self.a).len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0.0
    }
}

fn seg_point_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    (p - seg_point(p, a, b)).len()
}

fn seg_point(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let d = b - a;
    let l2 = d.len2();
    let t = if l2 > 0.0 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    a + d * t
}

/// Closest points between a convex polygon and a polygon / segment given as vertex ring.
fn closest(pa: &[Vec2], pb: &[Vec2], ring_b: bool) -> (Vec2, Vec2, f64) {
    let mut best = (pa[0], pb[0], f64::INFINITY);
    let eds = |p: &[Vec2], ring: bool| -> Vec<(Vec2, Vec2)> {
        let n = p.len();
        if ring {
            (0..n).map(|i| (p[i], p[(i + 1) % n])).collect()
        } else {
            vec![(p[0], p[1])]
        }
    };
    let ea = eds(pa, true);
    let eb = eds(pb, ring_b);
    for &v in pa {
        for &(c, d) in &eb {
            let q = seg_point(v, c, d);
            let l = (q - v).len();
            if l < best.2 {
                best = (v, q, l);
            }
        }
    }
    for &v in pb {
        for &(c, d) in &ea {
            let q = seg_point(v, c, d);
            let l = (q - v).len();
            if l < best.2 {
                best = (q, v, l);
            }
        }
    }
    best
}

/// Pair cuts: for every pair of obstacles (and obstacle and outline segment) closer than
/// `pair_mm`, the segment between their nearest points.
pub fn pair_cuts(b: &Board, m: &BoardModel, pair_mm: f64) -> Vec<Cut> {
    let pr = &b.proj;
    let r = pair_mm * MM;
    let boxes: Vec<(Vec2, Vec2)> = pr.obstacles.iter().map(|o| bbox_of(&o.poly)).collect();
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    order.sort_by(|&i, &j| boxes[i].0.x.total_cmp(&boxes[j].0.x).then(i.cmp(&j)));
    let mut cuts = Vec::new();
    for (ia, &i) in order.iter().enumerate() {
        for &j in &order[ia + 1..] {
            if boxes[j].0.x > boxes[i].1.x + r {
                break;
            }
            if boxes[j].0.y > boxes[i].1.y + r || boxes[j].1.y < boxes[i].0.y - r {
                continue;
            }
            let (oi, oj) = (&pr.obstacles[i], &pr.obstacles[j]);
            // Same pad (a multi-obstacle pad) is not a pair.
            if b.pad_of[i] == b.pad_of[j] {
                continue;
            }
            let (pa, pb, d) = closest(&oi.poly, &oj.poly, true);
            if d < 1.0 || d >= r || oi.poly.iter().any(|&v| point_in_poly(v, &oj.poly)) || oj.poly.iter().any(|&v| point_in_poly(v, &oi.poly)) {
                continue;
            }
            let layers = (0..pr.n_layers)
                .filter(|&l| (oi.drilled || oi.layers.has(l)) && (oj.drilled || oj.layers.has(l)))
                .fold(0u64, |a, l| a | 1 << l);
            if layers != 0 {
                cuts.push(Cut { a: pa, b: pb, ends: [Some(i), Some(j)], layers });
            }
        }
    }
    for (a, c) in board::outline_segments(m) {
        let (lo, hi) = bbox_of(&[a, c]);
        for (i, o) in pr.obstacles.iter().enumerate() {
            let (ol, oh) = boxes[i];
            if ol.x > hi.x + r || oh.x < lo.x - r || ol.y > hi.y + r || oh.y < lo.y - r {
                continue;
            }
            let (pa, pb, d) = closest(&o.poly, &[a, c], false);
            if d < 1.0 || d >= r {
                continue;
            }
            let layers = (0..pr.n_layers).filter(|&l| o.drilled || o.layers.has(l)).fold(0u64, |a, l| a | 1 << l);
            if layers != 0 {
                cuts.push(Cut { a: pa, b: pb, ends: [Some(i), None], layers });
            }
        }
    }
    cuts
}

/// Whether the pair cut `c` is a CDT edge of the projection: both ends at vertices (1 nm) that an
/// edge joins.
pub fn cut_is_edge(b: &Board, c: &Cut) -> bool {
    let pr = &b.proj;
    match (pr.vertex_at(c.a, 1.0), pr.vertex_at(c.b, 1.0)) {
        (Some(x), Some(y)) => pr.edges.binary_search(&[x.min(y), x.max(y)]).is_ok(),
        _ => false,
    }
}

/// Whether the segment of `c` passes through an obstacle other than its two ends.
pub fn cut_shadowed(b: &Board, c: &Cut) -> bool {
    let pr = &b.proj;
    let mid = (c.a + c.b) * 0.5;
    (0..pr.obstacles.len()).any(|o| {
        !c.ends.contains(&Some(o)) && {
            let (lo, hi) = bbox_of(&pr.obstacles[o].poly);
            let (sl, sh) = bbox_of(&[c.a, c.b]);
            lo.x <= sh.x && hi.x >= sl.x && lo.y <= sh.y && hi.y >= sl.y && {
                let poly = &pr.obstacles[o].poly;
                let n = poly.len();
                (0..n).any(|i| seg_cross(c.a, c.b - c.a, poly[i], poly[(i + 1) % n] - poly[i]).is_some()) || point_in_poly(mid, poly)
            }
        }
    })
}

/// One piece of copper for the cut check.
#[derive(Clone, Copy, Debug)]
pub struct Trace {
    pub a: Vec2,
    pub b: Vec2,
    pub net: i32,
    pub layer: usize,
    pub width: f64,
    pub clear: f64,
}

/// Parameter of the intersection of segments `p + t r` and `q + u s`, if they cross.
fn seg_cross(p: Vec2, r: Vec2, q: Vec2, s: Vec2) -> Option<(f64, f64)> {
    let den = r.cross(s);
    if den.abs() < 1e-9 {
        return None;
    }
    let t = (q - p).cross(s) / den;
    let u = (q - p).cross(r) / den;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some((t, u))
}

/// A trace crossing a cut: (layer, net, width, clearance, position along the cut, point).
type Hit = (usize, i32, f64, f64, f64, Vec2);

pub struct PairOut {
    pub violations: Vec<Violation>,
    /// Cuts crossed by at least one trace.
    pub with_traffic: usize,
}

/// Check `traces` against `cuts`: per cut and layer, Σ(w + s) + s <= length, where a pass of a
/// net that owns one end of the cut has no limit.
pub fn check_cuts(
    b: &Board,
    m: &BoardModel,
    cuts: &[Cut],
    traces: &[Trace],
    spots: &Spots,
    drc_r: f64,
    id0: usize,
) -> PairOut {
    let pr = &b.proj;
    // Grid of the cuts.
    let all: Vec<Vec2> = cuts.iter().flat_map(|c| [c.a, c.b]).collect();
    let mut out = PairOut { violations: Vec::new(), with_traffic: 0 };
    if all.is_empty() {
        return out;
    }
    let (lo, hi) = bbox_of(&all);
    let cell = 2.0 * MM;
    let nx = ((hi.x - lo.x) / cell) as usize + 1;
    let ny = ((hi.y - lo.y) / cell) as usize + 1;
    let mut cells: Vec<Vec<u32>> = vec![Vec::new(); nx * ny];
    for (i, c) in cuts.iter().enumerate() {
        let (cl, ch) = bbox_of(&[c.a, c.b]);
        for y in ((cl.y - lo.y) / cell) as usize..=(((ch.y - lo.y) / cell) as usize).min(ny - 1) {
            for x in ((cl.x - lo.x) / cell) as usize..=(((ch.x - lo.x) / cell) as usize).min(nx - 1) {
                cells[y * nx + x].push(i as u32);
            }
        }
    }
    // (cut) -> passes (layer, net, width, clear, pos along the cut, point)
    let mut hits: BTreeMap<usize, Vec<Hit>> = BTreeMap::new();
    for tr in traces {
        let (tl, th) = bbox_of(&[tr.a, tr.b]);
        let (x0, x1) = (((tl.x - lo.x) / cell).floor().max(0.0) as usize, (((th.x - lo.x) / cell).floor().max(0.0) as usize).min(nx - 1));
        let (y0, y1) = (((tl.y - lo.y) / cell).floor().max(0.0) as usize, (((th.y - lo.y) / cell).floor().max(0.0) as usize).min(ny - 1));
        if tl.x > hi.x + cell || tl.y > hi.y + cell || th.x < lo.x - cell || th.y < lo.y - cell {
            continue;
        }
        let mut seen: BTreeSet<u32> = BTreeSet::new();
        for y in y0.min(ny - 1)..=y1 {
            for x in x0.min(nx - 1)..=x1 {
                for &ci in &cells[y * nx + x] {
                    if !seen.insert(ci) {
                        continue;
                    }
                    let c = &cuts[ci as usize];
                    if c.layers >> tr.layer & 1 == 0 {
                        continue;
                    }
                    if let Some((_t, u)) = seg_cross(tr.a, tr.b - tr.a, c.a, c.b - c.a) {
                        hits.entry(ci as usize).or_default().push((tr.layer, tr.net, tr.width, tr.clear, u * c.len(), c.a + (c.b - c.a) * u));
                    }
                }
            }
        }
    }
    out.with_traffic = hits.len();
    for (ci, mut hs) in hits {
        let c = &cuts[ci];
        let len = c.len();
        hs.sort_by(|a, c| a.0.cmp(&c.0).then(a.1.cmp(&c.1)).then(a.4.total_cmp(&c.4)));
        let mut d: Vec<Hit> = Vec::new();
        for h in hs {
            if let Some(l) = d.last_mut() {
                if l.0 == h.0 && l.1 == h.1 && (h.4 - l.4).abs() < 0.5 * l.2.max(h.2) {
                    l.2 = l.2.max(h.2);
                    continue;
                }
            }
            d.push(h);
        }
        let bound = |k: usize| match c.ends[k] {
            Some(o) if pr.obstacles[o].bare => Bound::Hole { clear: pr.obstacles[o].clear },
            Some(o) => Bound::Pad { net: pr.obstacles[o].net, clear: pr.obstacles[o].clear },
            None => Bound::Edge { clear: pr.edge_clear },
        };
        let name = |k: usize| match c.ends[k] {
            Some(o) => {
                let pd = &m.pads[b.pad_of[o]];
                format!("{}.{} n{}", pd.ref_, pd.number, pd.net)
            }
            None => "outline".into(),
        };
        for l in 0..pr.n_layers {
            let mut on: Vec<&Hit> = d.iter().filter(|h| h.0 == l).collect();
            if on.is_empty() {
                continue;
            }
            on.sort_by(|a, c| a.4.total_cmp(&c.4));
            let items: Vec<Item> = on.iter().map(|h| Item { net: h.1, w: h.2, s: h.3 }).collect();
            let need = demand::need_in(pr.regime_at((c.a + c.b) * 0.5), &items, bound(0), bound(1));
            if need > len + TOL {
                let at = on[0].5;
                let id = id0 + out.violations.len();
                let outside: f64 = c
                    .ends
                    .iter()
                    .zip([c.a, c.b])
                    .map(|(e, p)| match e {
                        Some(o) => point_skeleton(p, &m.skeleton(&m.pads[b.pad_of[*o]], b.signal_layers[l])),
                        None => 0.0,
                    })
                    .sum();
                out.violations.push(Violation {
                    id,
                    check: "pair_fit",
                    edge: None,
                    cut: Some(ci),
                    layer: l as u64,
                    nets: on.iter().map(|h| h.1).collect(),
                    need_mm: need / MM,
                    have_mm: len / MM,
                    at: kicad(at),
                    cause: String::new(),
                    hand_drc: near_drc(spots, at, drc_r) || on.iter().any(|h| near_drc(spots, h.5, drc_r)),
                    line: Some([kicad(c.a), kicad(c.b)]),
                    passes: on.iter().map(|h| PassRec { net: h.1, layer: l, width_mm: h.2 / MM, clear_mm: h.3 / MM, pos_mm: h.4 / MM }).collect(),
                    ends: [name(0), name(1)],
                    true_gap_mm: (len + outside) / MM,
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------- topology

pub fn sub_state<'p>(
    b: &'p Board,
    m: &BoardModel,
    chains: &[Chain],
    walked: &[Walked],
) -> (State<'p>, Vec<Option<usize>>, usize) {
    let pr = &b.proj;
    let mut st = State::new(pr);
    let mut items = Vec::new();
    let mut wire_of = vec![None; chains.len()];
    let mut excluded = 0;
    for (ci, (ch, wk)) in chains.iter().zip(walked).enumerate() {
        if b.poured.contains(&ch.net) {
            continue;
        }
        let wall = wk.tris.iter().any(|&t| pr.allowed(t, ch.net).is_none());
        if wk.err.is_some() || wall {
            excluded += 1;
            continue;
        }
        let r = m.rules.net_rule(ch.net);
        let w = ch.meta.iter().filter(|x| !x.virt).map(|x| x.width).fold(0.0, f64::max);
        let obs = |e: End| match e {
            End::Pad(o) => o,
            End::Free => NO_OBS,
        };
        let wire = Wire {
            net: ch.net,
            w: if w > 0.0 { w } else { r.width as f64 },
            s: r.clearance as f64,
            a: wk.a,
            b: wk.b,
            pa: obs(ch.start),
            pb: obs(ch.end),
            turns: wk.turns.iter().map(|&(k, v, _)| (k, v)).collect(),
            passes: Vec::new(),
            tris: Vec::new(),
            alive: true,
        };
        wire_of[ci] = Some(items.len());
        items.push((wire, wk.passes.iter().map(|p| (p.edge, p.u)).collect::<Vec<_>>(), wk.tris.clone()));
    }
    st.load_all(items);
    (st, wire_of, excluded)
}

/// The hand polyline of traversal `k` of a chain inside its triangle as straight pieces
/// `(from, to, layer)` (clipped to the arc-length range between pass `k-1` and pass `k`; the
/// connecting hops are part of it, as they are of the walk), and whether the traversal is *anchored*: a via changes the
/// layer inside the triangle or a free terminal (dead end, junction) sits in it. `cum[i]` is the
/// arc length of `pts[i]`.
/// Copper pieces of a traversal: `(from, to, layer)`.
type Pieces = Vec<(Vec2, Vec2, usize)>;

fn sub_chords(pr: &super::state::Projection, ch: &Chain, wk: &Walked, k: usize, cum: &[f64]) -> (Pieces, bool) {
    let n = wk.passes.len();
    let total = cum[cum.len() - 1];
    let (s_lo, s_hi) = (if k > 0 { wk.passes[k - 1].s } else { 0.0 }, if k < n { wk.passes[k].s } else { total });
    let entry = if k > 0 { wk.passes[k - 1].at } else if ch.start == End::Free { ch.pts[0] } else { pr.pos[wk.a] };
    let exit = if k < n { wk.passes[k].at } else if ch.end == End::Free { ch.pts[ch.pts.len() - 1] } else { pr.pos[wk.b] };
    let mut out = Vec::new();
    let mut layers = BTreeSet::new();
    for j in 0..ch.meta.len() {
        if cum[j + 1] <= s_lo + 1.0 || cum[j] >= s_hi - 1.0 {
            continue;
        }
        let from = if cum[j] < s_lo { entry } else { ch.pts[j] };
        let to = if cum[j + 1] > s_hi { exit } else { ch.pts[j + 1] };
        layers.insert(ch.meta[j].layer);
        if (to - from).len() > 0.0 {
            out.push((from, to, ch.meta[j].layer));
        }
    }
    let free = (k == 0 && ch.start == End::Free) || (k == n && ch.end == End::Free);
    (out, layers.len() > 1 || free)
}

/// Result of [`crossings_report`].
pub struct CrossOut {
    /// Different-net crossings.
    pub total: usize,
    pub same_layer: Vec<Violation>,
    /// Crossings per State wire.
    pub per_wire: Vec<usize>,
    /// Pairs of traversals whose geometric cross-check disagreed with the topology: the parity of
    /// the clipped hand polylines' intersections differs from the interleaving, or two pieces of
    /// one layer meet that the topology does not cross on that layer. Must be 0 away from the
    /// hand's own DRC spots (where the hand copper touches or overlaps and a geometric answer is
    /// meaningless); the count there is `mismatch_at_drc`.
    pub mismatch: usize,
    pub mismatch_at_drc: usize,
    /// Of `mismatch`: pairs where the geometric answer is undefined: the hand polylines touch or
    /// overlap within 1 nm (two tracks stacked exactly on top of each other on different layers,
    /// a track ending on another's centre line) or one wire only grazes the triangle at a vertex
    /// (zero-length piece). The topology breaks such ties by the edge order and the vertex nudge.
    /// `mismatch - mismatch_contact` is the number of real disagreements.
    pub mismatch_contact: usize,
    /// Pairs where the geometry finds two pieces of one layer intersecting that the topology does
    /// not cross on that layer (a missed same-layer crossing). Must be 0.
    pub missed_same_layer: usize,
}

/// Layers of the chords of traversal `k` of a walked chain: the layer in the traversal, then the
/// layer after each turn.
fn chord_layers(wk: &Walked, k: usize) -> Vec<usize> {
    let mut v = vec![wk.lay[k]];
    v.extend(wk.turns.iter().filter(|x| x.0 == k).map(|x| x.2));
    v
}

/// Crossings of different nets in `st`, derived from the State alone: two chords of a triangle
/// cross iff their ends interleave on its boundary (a traversal that turns at a node corner has
/// one chord per leg), and they cross on one layer iff the legs have the same layer. No pair is
/// decided by geometry. As a cross-check every pair is also compared by the hand polylines
/// clipped to the triangle (`sub_chords`): see [`CrossOut::mismatch`].
/// `chain_of` maps a wire back to its chain.
pub fn crossings_report(
    st: &State,
    chains: &[Chain],
    walked: &[Walked],
    chain_of: &[usize],
    spots: &Spots,
    drc_r: f64,
    vid0: usize,
) -> CrossOut {
    let pr = st.proj;
    let cums: Vec<Vec<f64>> = chains
        .iter()
        .map(|c| {
            let mut v = vec![0.0];
            for i in 0..c.pts.len() - 1 {
                v.push(v[i] + (c.pts[i + 1] - c.pts[i]).len());
            }
            v
        })
        .collect();
    let mut out = CrossOut { total: 0, same_layer: Vec::new(), per_wire: vec![0usize; st.wires.len()], mismatch: 0, mismatch_at_drc: 0, mismatch_contact: 0, missed_same_layer: 0 };
    for t in 0..pr.tris.len() {
        let tr = &st.tri_trav[t];
        if tr.len() < 2 {
            continue;
        }
        let geo: Vec<Pieces> = tr
            .iter()
            .map(|&(w, k)| {
                let c = chain_of[w];
                sub_chords(pr, &chains[c], &walked[c], k, &cums[c]).0
            })
            .collect();
        for i in 0..tr.len() {
            for j in i + 1..tr.len() {
                let (wa, ka) = tr[i];
                let (wb, kb) = tr[j];
                if st.wires[wa].net == st.wires[wb].net {
                    continue;
                }
                let (ca, cb) = (chain_of[wa], chain_of[wb]);
                let (cha, chb) = (st.chords(wa, ka), st.chords(wb, kb));
                let (la, lb) = (chord_layers(&walked[ca], ka), chord_layers(&walked[cb], kb));
                let (mut n_topo, mut same) = (0usize, None);
                for (x, &(a1, a2)) in cha.iter().enumerate() {
                    for (y, &(b1, b2)) in chb.iter().enumerate() {
                        if interleave(a1, a2, b1, b2) {
                            n_topo += 1;
                            if la[x.min(la.len() - 1)] == lb[y.min(lb.len() - 1)] {
                                same = Some(la[x.min(la.len() - 1)]);
                            }
                        }
                    }
                }
                // Cross-check by geometry (never decides).
                let (mut n_geo, mut geo_same) = (0usize, false);
                for &(p, q, lp) in &geo[i] {
                    for &(r, u, lr) in &geo[j] {
                        if let Some((x, y)) = seg_cross(p, q - p, r, u - r) {
                            if x > 1e-6 && x < 1.0 - 1e-6 && y > 1e-6 && y < 1.0 - 1e-6 {
                                n_geo += 1;
                                geo_same |= lp == lr;
                            }
                        }
                    }
                }
                if n_topo % 2 != n_geo % 2 || (geo_same && same.is_none()) {
                    let ctr = (pr.pos[pr.tris[t][0]] + pr.pos[pr.tris[t][1]] + pr.pos[pr.tris[t][2]]) * (1.0 / 3.0);
                    let near = near_drc(spots, ctr, drc_r) || geo[i].iter().chain(&geo[j]).any(|x| near_drc(spots, x.0, drc_r));
                    if geo_same && same.is_none() {
                        out.missed_same_layer += 1;
                    }
                    if near {
                        out.mismatch_at_drc += 1;
                    } else {
                        out.mismatch += 1;
                        let contact = geo[i].is_empty() || geo[j].is_empty() || geo[i].iter().any(|&(p, q, _)| {
                            geo[j].iter().any(|&(r, u, _)| {
                                seg_point_dist(r, p, q).min(seg_point_dist(u, p, q)).min(seg_point_dist(p, r, u)).min(seg_point_dist(q, r, u)) < 1.0
                            })
                        });
                        if contact {
                            out.mismatch_contact += 1;
                        }
                    }
                    let touching = geo[i].is_empty() || geo[j].is_empty() || geo[i].iter().any(|&(p, q, _)| {
                        geo[j].iter().any(|&(r, u, _)| seg_point_dist(r, p, q).min(seg_point_dist(u, p, q)).min(seg_point_dist(p, r, u)).min(seg_point_dist(q, r, u)) < 1.0)
                    });
                    if !near && !touching && std::env::var("R0_DEBUG").is_ok() {
                        let f = |g: &Pieces| g.iter().map(|s| format!("({:.4},{:.4})->({:.4},{:.4})L{}", s.0.x / MM, -s.0.y / MM, s.1.x / MM, -s.1.y / MM, s.2)).collect::<Vec<_>>().join(" ");
                        let cs = pr.tris[t].map(|v| format!("({:.4},{:.4})", pr.pos[v].x / MM, -pr.pos[v].y / MM)).join(" ");
                        eprintln!("  tri corners {cs}; chords A {cha:?} B {chb:?}; turns A {:?} B {:?}; wire a/b {:?}/{:?} k {ka}/{kb} of {}/{}", st.wires[wa].turns, st.wires[wb].turns, (st.wires[wa].a, st.wires[wa].b), (st.wires[wb].a, st.wires[wb].b), st.wires[wa].passes.len(), st.wires[wb].passes.len());
                        for (nm, c) in [("A", ca), ("B", cb)] {
                            eprintln!("  chain {nm}: {}", chains[c].pts.iter().zip(&chains[c].meta).map(|(p, m)| format!("({:.3},{:.3})L{}{}", p.x / MM, -p.y / MM, m.layer, if m.virt { "v" } else { "" })).collect::<Vec<_>>().join(" "));
                        }
                        eprintln!("mismatch tri {t} nets {}/{} topo {n_topo} geo {n_geo} geo_same {geo_same}: A {} | B {}", st.wires[wa].net, st.wires[wb].net, f(&geo[i]), f(&geo[j]));
                    }
                }
                if n_topo == 0 {
                    continue;
                }
                out.total += n_topo;
                out.per_wire[wa] += n_topo;
                out.per_wire[wb] += n_topo;
                if let Some(x) = same {
                    let real = chains_intersect(&chains[ca], &chains[cb], x);
                    let c = (pr.pos[pr.tris[t][0]] + pr.pos[pr.tris[t][1]] + pr.pos[pr.tris[t][2]]) * (1.0 / 3.0);
                    out.same_layer.push(Violation {
                        id: vid0 + out.same_layer.len(),
                        check: "same_layer",
                        edge: None,
                        cut: None,
                        layer: x as u64,
                        nets: vec![st.wires[wa].net, st.wires[wb].net],
                        need_mm: 0.0,
                        have_mm: 0.0,
                        at: kicad(c),
                        cause: if real {
                            "hand copper really intersects (different nets on one layer: hand short / net tie)".into()
                        } else {
                            "mapping or interleaving: the projection says crossing, the hand polylines do not intersect".into()
                        },
                        hand_drc: near_drc(spots, c, drc_r),
                        line: None,
                        passes: Vec::new(),
                        ends: [String::new(), String::new()],
                        true_gap_mm: 0.0,
                    });
                }
            }
        }
    }
    out
}

/// Whether any real piece of `a` crosses (or touches within the track half-widths) a real piece of
/// `b` on `layer`.
fn chains_intersect(a: &Chain, b: &Chain, layer: usize) -> bool {
    let real = |c: &Chain| -> Vec<(Vec2, Vec2, f64)> {
        (0..c.meta.len())
            .filter(|&i| !c.meta[i].virt && c.meta[i].layer == layer)
            .map(|i| (c.pts[i], c.pts[i + 1], c.meta[i].width))
            .collect()
    };
    let (ra, rb) = (real(a), real(b));
    for &(p, q, wp) in &ra {
        for &(r, s, wr) in &rb {
            if seg_cross(p, q - p, r, s - r).is_some() {
                return true;
            }
            // Overlapping copper: the centre lines closer than the half widths.
            let d = [(p, r, s), (q, r, s), (r, p, q), (s, p, q)]
                .iter()
                .map(|&(x, u, v)| (x - seg_point(x, u, v)).len())
                .fold(f64::INFINITY, f64::min);
            if d < 0.5 * (wp + wr) {
                return true;
            }
        }
    }
    false
}

/// Run P3 on the hand topology (2 signal layers).
pub fn p3_witness(b: &Board, st: &State, via_d: f64, clear: f64) -> layers::LayerReport {
    let n = b.proj.obstacles.len();
    let lp = LayerParams { via_site_r: 0.5 * via_d + clear, ..LayerParams::default() };
    layers::assign(st, &|o| if o < n { b.smd_layer(o) } else { None }, &lp)
}

// ---------------------------------------------------------------- driver

#[derive(Debug, Deserialize)]
pub struct G0Stretch {
    pub layer: i32,
    pub pts: Vec<[f64; 2]>,
}
#[derive(Debug, Deserialize)]
pub struct G0Wire {
    pub net: i32,
    pub stretches: Vec<G0Stretch>,
}
#[derive(Debug, Deserialize)]
pub struct G0In {
    pub wires: Vec<G0Wire>,
    #[serde(default)]
    pub vias: Vec<[f64; 2]>,
    #[serde(default)]
    pub summary: serde_json::Value,
}

fn chain_traces(m: &BoardModel, chains: &[Chain]) -> Vec<Trace> {
    let mut v = Vec::new();
    for ch in chains {
        let clear = m.rules.net_rule(ch.net).clearance as f64;
        for (i, mt) in ch.meta.iter().enumerate() {
            if !mt.virt {
                v.push(Trace { a: ch.pts[i], b: ch.pts[i + 1], net: ch.net, layer: mt.layer, width: mt.width, clear });
            }
        }
    }
    v
}

fn g0_traces(m: &BoardModel, g0: &G0In) -> (Vec<Trace>, f64) {
    let mut v = Vec::new();
    let mut skipped = 0.0;
    for w in &g0.wires {
        let r = m.rules.net_rule(w.net);
        for s in &w.stretches {
            for p in s.pts.windows(2) {
                let (a, c) = (Vec2 { x: p[0][0], y: -p[0][1] }, Vec2 { x: p[1][0], y: -p[1][1] });
                if s.layer < 0 {
                    skipped += (c - a).len();
                    continue;
                }
                v.push(Trace { a, b: c, net: w.net, layer: s.layer as usize, width: r.width as f64, clear: r.clearance as f64 });
            }
        }
    }
    (v, skipped)
}

/// The points of the chains that become vertices: free ends (dead ends, junctions) and the points
/// where the copper changes layer (vias), deduplicated.
pub fn chain_nodes(chains: &[Chain]) -> Vec<Vec2> {
    let mut seen: BTreeSet<(i64, i64)> = BTreeSet::new();
    let mut out = Vec::new();
    let mut add = |p: Vec2| {
        if seen.insert((p.x.round() as i64, p.y.round() as i64)) {
            out.push(p);
        }
    };
    for c in chains {
        if c.start == End::Free {
            add(c.pts[0]);
        }
        if c.end == End::Free {
            add(c.pts[c.pts.len() - 1]);
        }
        for i in 1..c.meta.len() {
            if c.meta[i].layer != c.meta[i - 1].layer {
                add(c.pts[i]);
            }
        }
    }
    out
}

/// The full R0 witness on `hand`. `spots`: hand DRC violations (math frame, nm).
pub fn run(hand: &BoardModel, g0: Option<&G0In>, spots: &Spots, o: &WitnessOpts) -> Result<Witness, String> {
    let t0 = Instant::now();
    // The chains first (they need the pad obstacles only), then the projection again with the
    // chains' nodes (free ends and layer changes) inserted as vertices.
    let b0 = board::build(hand, &WitnessOpts { ..*o }.build)?;
    let (ps, other) = pieces(hand, &b0.signal_layers, o.sagitta_mm);
    let mut dropped = Dropped::default();
    let chs = chains(&b0, hand, &ps, &mut dropped);
    drop(b0);
    let nodes = chain_nodes(&chs);
    let b = board::build_with_nodes(hand, &o.build, &nodes)?;
    let loc = Locator::new(&b);
    let build_s = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    let walked: Vec<Walked> = chs.iter().map(|c| walk(&b, &loc, c)).collect();
    let walk_s = t1.elapsed().as_secs_f64();
    let total: f64 = ps.iter().map(|p| (p.b - p.a).len()).sum::<f64>() - dropped.in_node - dropped.duplicate;
    let mapped: f64 = walked.iter().map(|w| w.mapped).sum();
    let mut s = Summary {
        board: hand.board.clone(),
        signal_layers: b.signal_layers.len(),
        chains: chs.len(),
        chains_failed: walked.iter().filter(|w| w.err.is_some()).count(),
        signal_len_mm: total / MM,
        mapped_len_mm: mapped / MM,
        in_node_len_mm: dropped.in_node / MM,
        duplicate_len_mm: dropped.duplicate / MM,
        mapped_pct: if total > 0.0 { 100.0 * mapped / total } else { 100.0 },
        nonsignal_len_mm: other.into_iter().map(|(k, v)| (k, v / MM)).collect(),
        degenerate: walked.iter().map(|w| w.degenerate).sum(),
        passes: walked.iter().map(|w| w.passes.iter().filter(|p| !p.virt).count()).sum(),
        hand_drc_spots: spots.points.len(),
        hand_vias: hand.vias.len(),
        hand_signal_vias: hand.vias.iter().filter(|v| !b.poured.contains(&v.net)).count(),
        build_s,
        walk_s,
        ..Summary::default()
    };
    let t2 = Instant::now();
    let fit = fit_edges(&b, hand, &chs, &walked, spots, o);
    s.edge_cuts_checked = fit.edges_checked;
    s.tight_passes = fit.tight;
    let mut violations = fit.violations;
    let cuts = pair_cuts(&b, hand, o.pair_mm);
    s.pair_cuts = cuts.len();
    s.pair_cuts_as_edges = cuts.iter().filter(|c| cut_is_edge(&b, c)).count();
    s.pair_cuts_shadowed = cuts.iter().filter(|c| !cut_is_edge(&b, c) && cut_shadowed(&b, c)).count();
    let hand_tr = chain_traces(hand, &chs);
    let po = check_cuts(&b, hand, &cuts, &hand_tr, spots, o.drc_mm * MM, violations.len());
    s.pair_cuts_with_traffic = po.with_traffic;
    violations.extend(po.violations);
    let mut g0_violations = Vec::new();
    if let Some(g) = g0 {
        let (tr, _skipped) = g0_traces(hand, g);
        let go = check_cuts(&b, hand, &cuts, &tr, spots, o.drc_mm * MM, 0);
        s.g0_pair_violations = Some(go.violations.len());
        s.g0_pair_cuts_with_traffic = Some(go.with_traffic);
        s.g0_pair_violations_on_edges = Some(go.violations.iter().filter(|v| v.cut.is_some_and(|c| cut_is_edge(&b, &cuts[c]))).count());
        s.g0_pair_violations_over_2x = Some(go.violations.iter().filter(|v| v.need_mm > 2.0 * v.have_mm).count());
        g0_violations = go.violations;
    }
    s.fit_s = t2.elapsed().as_secs_f64();
    let t3 = Instant::now();
    let (st, wire_of, excluded) = sub_state(&b, hand, &chs, &walked);
    s.topo_wires = st.wires.len();
    s.topo_excluded = excluded;
    s.state_check = st.check().map_or_else(|e| e, |_| "ok".into());
    let mut chain_of = vec![0; st.wires.len()];
    for (c, w) in wire_of.iter().enumerate() {
        if let Some(w) = w {
            chain_of[*w] = c;
        }
    }
    let cx = crossings_report(&st, &chs, &walked, &chain_of, spots, o.drc_mm * MM, violations.len());
    let pw = cx.per_wire;
    s.crossings = cx.total;
    s.same_layer_crossings = cx.same_layer.len();
    s.fallback_pairs = 0;
    s.crosscheck_mismatch = cx.mismatch;
    s.crosscheck_mismatch_at_drc = cx.mismatch_at_drc;
    s.crosscheck_mismatch_contact = cx.mismatch_contact;
    s.missed_same_layer = cx.missed_same_layer;
    s.nodes = nodes.len();
    s.turns = walked.iter().map(|w| w.turns.len()).sum();
    violations.extend(cx.same_layer);
    if b.proj.n_layers == 2 && s.state_check == "ok" {
        let r = &hand.rules.default;
        let lr = p3_witness(&b, &st, r.via_d as f64, r.clearance as f64);
        s.p3_ran = true;
        s.p3_vias = lr.vias;
        s.p3_unlayerable = lr.unlayerable.len();
        s.p3_same_layer = lr.same_layer;
    }
    s.topo_s = t3.elapsed().as_secs_f64();
    for (i, v) in violations.iter_mut().enumerate() {
        v.id = i;
        let c = &mut s.counts;
        if v.hand_drc {
            c.hand_drc += 1;
            continue;
        }
        match v.check {
            "edge_fit" => c.edge_fit += 1,
            "layer_combo" => c.layer_combo += 1,
            "allowed" => c.allowed += 1,
            "pair_fit" => c.pair_fit += 1,
            _ => c.same_layer += 1,
        }
    }
    s.runtime_s = t0.elapsed().as_secs_f64();
    Ok(Witness { summary: s, violations, chains: chs, walked, wire_of, wire_crossings: pw, g0_violations, board: b })
}
