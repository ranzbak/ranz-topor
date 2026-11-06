//! P3 layer assignment for two signal layers, Polubasov 2001 ("Global minimization of the
//! number of vias"): every different-net crossing is a domain whose orientation says which of
//! its two wires is on layer 0. Consecutive events along a wire are joined by an edge of weight
//! +c (no via under the initial orientation) or −c (via), c = via cost; the optimum is the most
//! negative cut. SMD pads, and stretches over another net's SMD pad, are fixed-orientation
//! domains merged into one super-domain (node 0), which makes the graph non-planar.
//!
//! Solver: the reductions of §2/§4 keep a stack for the backward pass — T1 parallel edges
//! (merged on insertion), T2 dominant-edge contraction, T3 isolated domain, the exact
//! elimination of a domain of degree ≤ 3 (degree 3 = T5, applied last as the paper advises;
//! degree ≤ 2 generalises T2) and T6 (with its partial form) for a triangle with one negative
//! edge, T4 (positive triangle to star) when a corner then contracts; flips relabel domains so
//! any sign pattern of a triangle fits T4 or T6. The remainder (no planar cut once the super-domain exists) is solved exhaustively when
//! small, else by local search with seeded restarts.
//!
//! Cost model of a reduced graph: f(o) = K + Σ_e (w_e·[o_u ≠ o_v] + max(0, −w_e)); every
//! transform keeps min f unchanged. Complexity: the reductions are O((N + M) · rounds) with
//! BTreeMap adjacency (deterministic); the remainder search is O(2^R · M) or O(restarts · R · M).

use super::demand::Item;
use super::pack::Packer;
use super::refine::{refine, RefineParams};
use super::state::{State, TriId, ViaSlot, WireId};
use crate::geom::Vec2;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::{BTreeMap, BTreeSet};

const EPS: f64 = 1e-9;
const MM: f64 = 1e6;

#[derive(Clone, Debug)]
enum Step {
    /// `o[a] = o[into] ^ flip`.
    Merge { a: usize, into: usize, flip: bool },
    /// `o[d] = argmin_t Σ w·[t ≠ o[x]]` over its neighbours at elimination.
    Elim { d: usize, nbrs: Vec<(usize, f64)> },
    /// T6: flip `c` when it differs from both `a` and `b`.
    Promise { c: usize, a: usize, b: usize },
    /// `o[v] ^= 1` (a relabeling).
    Flip { v: usize },
}

/// Domain graph with node 0 fixed (orientation 0).
#[derive(Clone, Debug)]
pub struct DomainGraph {
    adj: Vec<BTreeMap<usize, f64>>,
    alive: Vec<bool>,
    /// Accumulated constant (the paper's accumulated vias, in cost units).
    pub k: f64,
    stack: Vec<Step>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GraphSize {
    pub nodes: usize,
    pub edges: usize,
}

impl DomainGraph {
    pub fn new(n: usize) -> Self {
        DomainGraph { adj: vec![BTreeMap::new(); n], alive: vec![true; n], k: 0.0, stack: Vec::new() }
    }

    /// Add edge weight `w` between `u` and `v` (T1 merge; a loop only adds its forced cost).
    pub fn add_edge(&mut self, u: usize, v: usize, w: f64) {
        if u == v {
            self.k += (-w).max(0.0);
            return;
        }
        let old = self.adj[u].get(&v).copied().unwrap_or(0.0);
        let new = old + w;
        self.k += (-old).max(0.0) + (-w).max(0.0) - (-new).max(0.0);
        if new.abs() < EPS {
            self.adj[u].remove(&v);
            self.adj[v].remove(&u);
        } else {
            self.adj[u].insert(v, new);
            self.adj[v].insert(u, new);
        }
    }

    /// Add the pure cut term `w·[o_u ≠ o_v]`.
    fn add_term(&mut self, u: usize, v: usize, w: f64) {
        if w.abs() < EPS {
            return;
        }
        self.add_edge(u, v, w);
        self.k -= (-w).max(0.0);
    }

    /// Live domains with at least one edge, and edges.
    pub fn size(&self) -> GraphSize {
        let nodes = (1..self.adj.len()).filter(|&v| self.alive[v] && !self.adj[v].is_empty()).count();
        let edges = self.adj.iter().map(|a| a.len()).sum::<usize>() / 2;
        GraphSize { nodes, edges }
    }

    fn detach(&mut self, v: usize) -> Vec<(usize, f64)> {
        let nb: Vec<(usize, f64)> = std::mem::take(&mut self.adj[v]).into_iter().collect();
        for &(x, _) in &nb {
            self.adj[x].remove(&v);
        }
        self.alive[v] = false;
        nb
    }

    /// Exact elimination of `d` (degree ≤ 3).
    fn eliminate(&mut self, d: usize) {
        let nb = self.detach(d);
        self.k += nb.iter().map(|&(_, w)| (-w).max(0.0)).sum::<f64>();
        match nb.as_slice() {
            [] => {}
            &[(_, a)] => self.k += a.min(0.0),
            &[(x, a), (y, b)] => {
                let same = (a + b).min(0.0);
                self.k += same;
                self.add_term(x, y, a.min(b) - same);
            }
            &[(x, a), (y, b), (z, c)] => {
                let h0 = (a + b + c).min(0.0);
                let hx = a.min(b + c) - h0;
                let hy = b.min(a + c) - h0;
                let hz = c.min(a + b) - h0;
                self.k += h0;
                self.add_term(x, y, (hx + hy - hz) / 2.0);
                self.add_term(x, z, (hx + hz - hy) / 2.0);
                self.add_term(y, z, (hy + hz - hx) / 2.0);
            }
            _ => unreachable!("eliminate degree > 3"),
        }
        self.stack.push(Step::Elim { d, nbrs: nb });
    }

    /// T2: merge `a` into `b` (`o_a = o_b ^ flip`), dropping edge ab.
    fn contract(&mut self, a: usize, b: usize, flip: bool) {
        let nb = self.detach(a);
        for (x, w) in nb {
            if x != b {
                self.add_edge(b, x, if flip { -w } else { w });
            }
        }
        self.stack.push(Step::Merge { a, into: b, flip });
    }

    /// T2 test at `v`: a neighbour whose edge outweighs all other edges of `v` together.
    fn dominant(&self, v: usize) -> Option<(usize, f64)> {
        let total: f64 = self.adj[v].values().map(|w| w.abs()).sum();
        self.adj[v].iter().find(|(_, w)| 2.0 * w.abs() >= total - EPS).map(|(&u, &w)| (u, w))
    }

    /// Relabel `v` (`o_v` ↦ `o_v ^ 1`): negates its edges, cost unchanged.
    fn flip(&mut self, v: usize) {
        debug_assert!(v != 0, "node 0 is fixed");
        let nb: Vec<usize> = self.adj[v].keys().copied().collect();
        for x in nb {
            let w = -self.adj[v][&x];
            self.adj[v].insert(x, w);
            self.adj[x].insert(v, w);
        }
        self.stack.push(Step::Flip { v });
    }

    /// Flips (never of node 0) that make `ac`, `bc` positive; `ab` then has the triangle's
    /// parity sign.
    fn normalise(&mut self, a: usize, b: usize, c: usize) {
        let sg = |w: f64| if w < 0.0 { -1.0 } else { 1.0 };
        let (ac, bc) = (self.adj[a][&c], self.adj[b][&c]);
        let fc = if a == 0 { sg(ac) } else if b == 0 { sg(bc) } else { 1.0 };
        if fc < 0.0 {
            self.flip(c);
        }
        if sg(ac) * fc < 0.0 {
            self.flip(a);
        }
        if sg(bc) * fc < 0.0 {
            self.flip(b);
        }
    }

    /// Triangles `(a, b)` at apex `c ≥ 1`, with `a < b`.
    fn triangles(&self, c: usize) -> Vec<(usize, usize)> {
        let nb: Vec<usize> = self.adj[c].keys().copied().collect();
        let mut out = Vec::new();
        for (i, &a) in nb.iter().enumerate() {
            for &b in &nb[i + 1..] {
                if self.adj[a].contains_key(&b) {
                    out.push((a, b));
                }
            }
        }
        out
    }

    /// T6 (and its partial form) on a triangle with an odd number of negative edges, made
    /// `ab = −z`, `ac = x > 0`, `bc = y > 0` by flips; `s` = Σ|w| of `c`'s other edges.
    fn t6(&mut self) -> bool {
        for c in 1..self.adj.len() {
            if !self.alive[c] {
                continue;
            }
            let tot: f64 = self.adj[c].values().map(|w| w.abs()).sum();
            for (a, b) in self.triangles(c) {
                let (x, y, ab) = (self.adj[a][&c], self.adj[b][&c], self.adj[a][&b]);
                if x * y * ab >= 0.0 {
                    continue;
                }
                let (x, y, z) = (x.abs(), y.abs(), ab.abs());
                let s = tot - x - y;
                // Full T6 removes ab (d = z); the partial form lowers all three by d.
                let d = if x + y - 2.0 * z >= s - EPS {
                    z
                } else if x + y > s + EPS {
                    (x + y - s) / 2.0
                } else {
                    continue;
                };
                self.normalise(a, b, c);
                // Cancelling d of ab's negative weight moves d forced vias into K (T1).
                self.add_term(a, b, d);
                self.add_term(a, c, -d);
                self.add_term(b, c, -d);
                self.stack.push(Step::Promise { c, a, b });
                return true;
            }
        }
        false
    }

    /// T4: an all-positive triangle (after flips) becomes a star around a new node, applied
    /// only when a corner `v ≥ 1` then contracts by T2 (its two triangle edges outweigh its
    /// other edges), so the graph never grows.
    fn t4(&mut self) -> bool {
        for c in 1..self.adj.len() {
            if !self.alive[c] {
                continue;
            }
            for (a, b) in self.triangles(c) {
                let (ac, bc, ab) = (self.adj[a][&c], self.adj[b][&c], self.adj[a][&b]);
                if ac * bc * ab <= 0.0 {
                    continue;
                }
                let (x, y, z) = (ab.abs(), bc.abs(), ac.abs());
                let gain = |v: usize, e1: f64, e2: f64| -> bool {
                    let tot: f64 = self.adj[v].values().map(|w| w.abs()).sum();
                    v != 0 && e1 + e2 >= tot - e1 - e2 - EPS
                };
                if !(gain(a, x, z) || gain(b, x, y) || gain(c, y, z)) {
                    continue;
                }
                self.normalise(a, b, c);
                for (u, v) in [(a, b), (a, c), (b, c)] {
                    self.adj[u].remove(&v);
                    self.adj[v].remove(&u);
                }
                let d = self.adj.len();
                self.adj.push(BTreeMap::new());
                self.alive.push(true);
                self.add_edge(d, a, x + z);
                self.add_edge(d, b, x + y);
                self.add_edge(d, c, y + z);
                return true;
            }
        }
        false
    }

    /// Apply the reductions until none applies.
    pub fn reduce(&mut self) {
        loop {
            let mut changed = false;
            for v in 1..self.adj.len() {
                if !self.alive[v] {
                    continue;
                }
                if self.adj[v].len() <= 2 {
                    self.eliminate(v);
                    changed = true;
                } else if let Some((u, w)) = self.dominant(v) {
                    self.contract(v, u, w < 0.0);
                    changed = true;
                }
            }
            if changed || self.t6() || self.t4() {
                continue;
            }
            match (1..self.adj.len()).find(|&v| self.alive[v] && self.adj[v].len() == 3) {
                Some(v) => self.eliminate(v),
                None => break,
            }
        }
    }

    fn cut_value(&self, o: &[u8], nodes: &[usize]) -> f64 {
        let mut f = 0.0;
        for &u in nodes.iter().chain(std::iter::once(&0)) {
            for (&v, &w) in &self.adj[u] {
                if u < v && o[u] != o[v] {
                    f += w;
                }
            }
        }
        f
    }

    /// Orientation of the remaining nodes: exhaustive up to `exact_max` nodes, else local search.
    fn solve_rest(&self, o: &mut [u8], exact_max: usize, restarts: usize, seed: u64) {
        let rest: Vec<usize> = (1..self.adj.len()).filter(|&v| self.alive[v]).collect();
        if rest.is_empty() {
            return;
        }
        if rest.len() <= exact_max {
            let mut best = (f64::INFINITY, 0u64);
            for m in 0..(1u64 << rest.len()) {
                for (i, &v) in rest.iter().enumerate() {
                    o[v] = ((m >> i) & 1) as u8;
                }
                let f = self.cut_value(o, &rest);
                if f < best.0 - EPS {
                    best = (f, m);
                }
            }
            for (i, &v) in rest.iter().enumerate() {
                o[v] = ((best.1 >> i) & 1) as u8;
            }
            return;
        }
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut best: (f64, Vec<u8>) = (f64::INFINITY, Vec::new());
        for r in 0..restarts.max(1) {
            for &v in &rest {
                o[v] = if r == 0 { 0 } else { rng.gen_range(0..2) };
            }
            loop {
                let mut improved = false;
                for &v in &rest {
                    let gain: f64 = self.adj[v].iter().map(|(&u, &w)| if o[u] == o[v] { w } else { -w }).sum();
                    if gain < -EPS {
                        o[v] ^= 1;
                        improved = true;
                    }
                }
                if !improved {
                    break;
                }
            }
            let f = self.cut_value(o, &rest);
            if f < best.0 - EPS {
                best = (f, rest.iter().map(|&v| o[v]).collect());
            }
        }
        for (i, &v) in rest.iter().enumerate() {
            o[v] = best.1[i];
        }
    }

    /// Reduce, solve the remainder, run the backward pass: the orientation of every node.
    pub fn solve(&mut self, exact_max: usize, restarts: usize, seed: u64) -> Vec<u8> {
        self.reduce();
        let mut o = vec![0u8; self.adj.len()];
        self.solve_rest(&mut o, exact_max, restarts, seed);
        for step in self.stack.iter().rev() {
            match step {
                &Step::Merge { a, into, flip } => o[a] = o[into] ^ flip as u8,
                Step::Elim { d, nbrs } => {
                    let cost = |t: u8| -> f64 { nbrs.iter().filter(|&&(x, _)| o[x] != t).map(|p| p.1).sum() };
                    o[*d] = (cost(1) < cost(0) - EPS) as u8;
                }
                &Step::Flip { v } => o[v] ^= 1,
                &Step::Promise { c, a, b } => {
                    if o[a] != o[c] && o[b] != o[c] {
                        o[c] ^= 1;
                    }
                }
            }
        }
        o
    }
}

/// Cost of an edge of weight `w`, cut or not.
pub fn edge_cost(w: f64, cut: bool) -> f64 {
    if (w > 0.0) == cut {
        w.abs()
    } else {
        0.0
    }
}

/// Brute-force minimum of Σ_e cost_e(o) over orientations with `o[0] = 0` (tests).
pub fn brute_force(n: usize, edges: &[(usize, usize, f64)]) -> f64 {
    let mut best = f64::INFINITY;
    for m in 0..(1u64 << (n - 1)) {
        let o = |v: usize| if v == 0 { 0 } else { (m >> (v - 1)) & 1 };
        let f: f64 = edges.iter().map(|&(u, v, w)| edge_cost(w, o(u) != o(v))).sum();
        best = best.min(f);
    }
    best
}

/// One event along a wire: a domain (node ≥ 1) or the super-domain (node 0) with parity `pi`
/// (label = o[node] ^ pi), at traversal `k`, fraction `s` along it.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub node: usize,
    pub pi: u8,
    pub k: usize,
    pub s: f64,
}

#[derive(Clone, Debug, Default)]
pub struct LayerReport {
    pub domains: usize,
    pub before: GraphSize,
    pub after: GraphSize,
    pub vias: usize,
    /// Vias whose stretch has no triangle with a free disc for a via.
    pub vias_no_site: usize,
    pub via_pts: Vec<Vec2>,
    pub slots: Vec<ViaSlot>,
    /// Different-net crossings that both wires must take on one layer.
    pub unlayerable: Vec<Vec2>,
    /// Different-net crossings left on one layer after the assignment (must be 0).
    pub same_layer: usize,
    /// Per wire: (layer, points) pieces.
    pub stretches: Vec<Vec<(i32, Vec<Vec2>)>>,
    /// Per wire: its events as `(traversal k, fraction s, layer label)` in wire order (empty for
    /// a wire that crosses nothing). Consecutive events with different labels bound a via interval.
    pub events: Vec<Vec<(usize, f64, i32)>>,
    /// The wire pairs `(a, b)`, `a < b`, of the different-net crossings left on one layer.
    pub unlayerable_pairs: Vec<(WireId, WireId)>,
    /// Where each pair of `unlayerable_pairs` crosses (its first crossing; same order).
    pub unlayerable_at: Vec<Vec2>,
    /// Over-full edge-layers by the P1 demand model before and after the capacity refinement
    /// (measured with the same model when the refinement is off), and the moves it kept.
    pub overfull_before: usize,
    pub overfull_after: usize,
    pub refine_moves: usize,
    /// Per wire, per event: the layers it may take (bit per layer) and the event of the other
    /// wire of its crossing (the two must differ in layer); for the P4 label moves.
    pub mask: Vec<Vec<u64>>,
    pub partner: Vec<Vec<Option<(WireId, usize)>>>,
}

/// Parameters of P3.
#[derive(Clone, Copy, Debug)]
pub struct LayerParams {
    pub via_mm: f64,
    /// Via pad radius + clearance (nm), for the plausible-site count.
    pub via_site_r: f64,
    pub exact_max: usize,
    pub restarts: usize,
    pub seed: u64,
    /// Layers `0..n_signal` are signal layers, the rest planes; 0 = all are signal layers.
    pub n_signal: usize,
    /// Extra price per mm of signal copper on a plane layer (the `plane_signal` factor).
    pub plane_mm: f64,
    /// Price per cut of a plane (the `plane_cut` factor); a stretch on a plane has two layer
    /// changes, each pays half of it.
    pub plane_cut: f64,
    /// Price (mm) of an over-full edge-layer in the capacity-aware refinement ([`refine`]; the
    /// `overflow` factor); 0 = layers are assigned for vias only.
    pub cap_mm: f64,
    /// Most move evaluations of the refinement.
    pub cap_evals: usize,
    /// The refinement may add vias to remove overflow ([`RefineParams::add_vias`]).
    pub cap_vias: bool,
}

impl Default for LayerParams {
    fn default() -> Self {
        LayerParams {
            via_mm: 25.0,
            via_site_r: 0.5e6,
            exact_max: 16,
            restarts: 64,
            seed: 1,
            n_signal: 0,
            plane_mm: 0.0,
            plane_cut: 0.0,
            cap_mm: 0.0,
            cap_evals: 200_000,
            cap_vias: false,
        }
    }
}

/// Point at fraction `s` of traversal `k` of polyline `pts`.
fn at(pts: &[Vec2], k: usize, s: f64) -> Vec2 {
    let j = (k + 1).min(pts.len() - 1);
    pts[k] + (pts[j] - pts[k]) * s
}

/// Piece of `pts` from `(k1, s1)` to `(k2, s2)`.
fn piece(pts: &[Vec2], k1: usize, s1: f64, k2: usize, s2: f64) -> Vec<Vec2> {
    let mut v = vec![at(pts, k1, s1)];
    v.extend_from_slice(&pts[k1 + 1..=k2]);
    v.push(at(pts, k2, s2));
    v
}

fn seg_param(p0: Vec2, p1: Vec2, q0: Vec2, q1: Vec2) -> f64 {
    let (d, e) = (p1 - p0, q1 - q0);
    let den = d.cross(e);
    if den.abs() < 1e-12 {
        return 0.5;
    }
    ((q0 - p0).cross(e) / den).clamp(0.0, 1.0)
}

/// Stretches, via slots and counts of `rep` from the labelled events `evl[w] = (k, s, layer)` of
/// every wire (shared by the two-layer and the N-layer assignment). O(events).
fn stretches_and_vias(st: &State, polys: &[Vec<Vec2>], evl: &[Vec<(usize, f64, i32)>], p: &LayerParams, rep: &mut LayerReport) {
    let pr = st.proj;
    // A plausible site: a point of the stretch (pass point or triangle centroid) whose free disc
    // against other nets' obstacles holds a via pad plus clearance (wire demand ignored).
    let site = |k1: usize, k2: usize, w: WireId| -> bool {
        let net = st.wires[w].net;
        let free = |q: Vec2| pr.free_radius(q, net) >= p.via_site_r;
        st.wires[w].tris[k1..=k2].iter().any(|&t: &TriId| {
            let c = pr.tris[t].iter().fold(Vec2 { x: 0.0, y: 0.0 }, |a, &v| a + pr.pos[v]) * (1.0 / 3.0);
            !pr.tri_out[t] && free(c)
        }) || (k1 + 1..=k2).any(|k| free(polys[w][k]))
    };
    for (w, ev) in evl.iter().enumerate() {
        let wi = &st.wires[w];
        if !wi.alive {
            rep.stretches.push(Vec::new());
            rep.events.push(Vec::new());
            continue;
        }
        let pts = &polys[w];
        let n = wi.passes.len();
        let mut pieces: Vec<(i32, Vec<Vec2>)> = Vec::new();
        rep.events.push(ev.clone());
        let Some(&(fk, fs, fl)) = ev.first() else {
            // Crosses nothing: the bottom layer (TopoR's default).
            let bottom = if p.n_signal == 0 { pr.n_layers } else { p.n_signal };
            pieces.push((bottom as i32 - 1, pts.clone()));
            rep.stretches.push(pieces);
            continue;
        };
        pieces.push((fl, piece(pts, 0, 0.0, fk, fs)));
        for x in ev.windows(2) {
            let ((ka, sa, la), (kb, sb, lb)) = (x[0], x[1]);
            if la == lb {
                pieces.push((la, piece(pts, ka, sa, kb, sb)));
                continue;
            }
            // The via in the middle of its interval: a pass point when the stretch spans one.
            let (km, sm) = if kb > ka { ((ka + kb).div_ceil(2), 0.0) } else { (ka, 0.5 * (sa + sb)) };
            pieces.push((la, piece(pts, ka, sa, km, sm)));
            pieces.push((lb, piece(pts, km, sm, kb, sb)));
            rep.vias += 1;
            rep.via_pts.push(at(pts, km, sm));
            rep.slots.push(ViaSlot { wire: w, from: (ka, sa), to: (kb, sb), at: (km, sm) });
            if !site(ka, kb, w) {
                rep.vias_no_site += 1;
            }
        }
        let &(lk, ls, ll) = ev.last().expect("non-empty");
        pieces.push((ll, piece(pts, lk, ls, n, 1.0)));
        rep.stretches.push(pieces);
    }
}

/// Recompute the stretches, via slots and via counts of `rep` from changed labels `evl` (the P4
/// label moves; the wires and their events are unchanged). O(events + passes).
pub fn relabel(st: &State, evl: &[Vec<(usize, f64, i32)>], p: &LayerParams, rep: &mut LayerReport) {
    let polys: Vec<Vec<Vec2>> = (0..st.wires.len()).map(|w| if st.wires[w].alive { st.polyline(w) } else { Vec::new() }).collect();
    rep.stretches.clear();
    rep.events.clear();
    rep.vias = 0;
    rep.via_pts.clear();
    rep.slots.clear();
    rep.vias_no_site = 0;
    stretches_and_vias(st, &polys, evl, p, rep);
}

/// Assign layers to the wires of `st`: the exact two-layer Polubasov solve for two signal layers,
/// greedy colouring plus alpha-beta swaps ([`assign_n`]) otherwise. `smd_layer(pad)` is the layer of
/// a single-layer pad.
pub fn assign(st: &State, smd_layer: &dyn Fn(usize) -> Option<usize>, p: &LayerParams) -> LayerReport {
    assign_with(st, smd_layer, p, None)
}

/// [`assign`] with the drawing's packer: the labelling is then refined against capacity
/// ([`refine`]) and the over-full edge-layers are counted.
pub fn assign_with(st: &State, smd_layer: &dyn Fn(usize) -> Option<usize>, p: &LayerParams, packer: Option<&Packer>) -> LayerReport {
    if st.proj.n_layers == 2 {
        assign_two(st, smd_layer, p, packer)
    } else {
        assign_n(st, smd_layer, p, packer)
    }
}

fn assign_two(st: &State, smd_layer: &dyn Fn(usize) -> Option<usize>, p: &LayerParams, packer: Option<&Packer>) -> LayerReport {
    let pr = st.proj;
    let mut rep = LayerReport::default();
    let mut dom: BTreeMap<(WireId, usize, WireId, usize), usize> = BTreeMap::new();
    let mut unl: BTreeSet<(WireId, usize, WireId, usize)> = BTreeSet::new();
    let polys: Vec<Vec<Vec2>> = (0..st.wires.len()).map(|w| if st.wires[w].alive { st.polyline(w) } else { Vec::new() }).collect();
    let mut events: Vec<Vec<Event>> = Vec::with_capacity(st.wires.len());
    for (w, wi) in st.wires.iter().enumerate() {
        let mut ev = Vec::new();
        if !wi.alive {
            events.push(ev);
            continue;
        }
        let pts = &polys[w];
        let n = wi.passes.len();
        if let Some(l) = smd_layer(wi.pa) {
            ev.push(Event { node: 0, pi: l as u8, k: 0, s: 0.0 });
        }
        let mut xs = st.crossings(w);
        xs.retain(|x| st.wires[x.other].net != wi.net);
        for (k, &t) in wi.tris.iter().enumerate() {
            let mask = pr.allowed(t, wi.net).expect("wire inside a wall");
            let mut here: Vec<Event> = Vec::new();
            if let Some(l) = mask.single() {
                here.push(Event { node: 0, pi: l as u8, k, s: 0.0 });
            }
            for x in xs.iter().filter(|x| x.k == k) {
                let o = x.other;
                let (q0, q1) = (polys[o][x.other_k], polys[o][x.other_k + 1]);
                let s = seg_param(pts[k], pts[k + 1], q0, q1);
                let mo = pr.allowed(x.tri, st.wires[o].net).expect("wire inside a wall");
                let key = if w < o { (w, k, o, x.other_k) } else { (o, x.other_k, w, k) };
                match (mask.single(), mo.single()) {
                    (Some(a), Some(b)) => {
                        if a == b {
                            if std::env::var("TOPO_UNLDBG").is_ok() {
                                let vs: Vec<String> = pr.tris[x.tri].iter().map(|&v| format!("{:?}@({:.3},{:.3})", pr.vkind[v], pr.pos[v].x / 1e6, -pr.pos[v].y / 1e6)).collect();
                                let obs: Vec<String> = pr.tri_obs[x.tri].iter().map(|&o| format!("{o}:{:?}:{:?}:({:.2},{:.2})", pr.obstacles[o].net, pr.obstacles[o].layers, pr.obstacles[o].center.x / 1e6, -pr.obstacles[o].center.y / 1e6)).collect();
                                eprintln!("UNLDBG w {w} net {} k {k}/{} o {o} net {} ok {}/{} tri {} verts {vs:?} obs {obs:?}", wi.net, wi.tris.len(), st.wires[o].net, x.other_k, st.wires[o].tris.len(), x.tri);
                            }
                            unl.insert(key);
                        }
                    }
                    (Some(a), None) => here.push(Event { node: 0, pi: a as u8, k, s }),
                    (None, Some(b)) => here.push(Event { node: 0, pi: 1 - b as u8, k, s }),
                    (None, None) => {
                        let next = dom.len() + 1;
                        let d = *dom.entry(key).or_insert(next);
                        here.push(Event { node: d, pi: (w > o) as u8, k, s });
                    }
                }
            }
            // A pass through a gap too narrow on some layer for this wire alone excludes it.
            if k < n {
                let e = st.passes[wi.passes[k]].edge;
                let fits: Vec<usize> =
                    (0..pr.n_layers).filter(|&l| pr.fits_alone(e, l, Item { net: wi.net, w: wi.w, s: wi.s })).collect();
                if let [l] = fits[..] {
                    here.push(Event { node: 0, pi: l as u8, k, s: 1.0 });
                }
            }
            here.sort_by(|a, b| a.s.total_cmp(&b.s));
            ev.extend(here);
        }
        if let Some(l) = smd_layer(wi.pb) {
            ev.push(Event { node: 0, pi: l as u8, k: n, s: 1.0 });
        }
        events.push(ev);
    }
    rep.domains = dom.len();
    let mut g = DomainGraph::new(dom.len() + 1);
    for ev in &events {
        for x in ev.windows(2) {
            g.add_edge(x[0].node, x[1].node, if x[0].pi == x[1].pi { p.via_mm } else { -p.via_mm });
        }
    }
    rep.before = g.size();
    let mut reduced = g.clone();
    reduced.reduce();
    rep.after = reduced.size();
    let o = g.solve(p.exact_max, p.restarts, p.seed);
    let mut evl: Vec<Vec<(usize, f64, i32)>> = Vec::with_capacity(events.len());
    for ev in events.iter() {
        evl.push(ev.iter().map(|e| (e.k, e.s, (o[e.node] ^ e.pi) as i32)).collect());
    }
    // Capacity-aware refinement: node 0 events are fixed, domain events flip with their partner.
    let bottom = (if p.n_signal == 0 { pr.n_layers } else { p.n_signal }) as i32 - 1;
    let mut mask: Vec<Vec<u64>> = Vec::with_capacity(events.len());
    let mut partner: Vec<Vec<Option<(WireId, usize)>>> = events.iter().map(|ev| vec![None; ev.len()]).collect();
    let mut by_node: BTreeMap<usize, Vec<(WireId, usize)>> = BTreeMap::new();
    for (w, ev) in events.iter().enumerate() {
        mask.push(ev.iter().enumerate().map(|(i, e)| if e.node == 0 { 1u64 << evl[w][i].2 } else { 0b11 }).collect());
        for (i, e) in ev.iter().enumerate().filter(|(_, e)| e.node > 0) {
            by_node.entry(e.node).or_default().push((w, i));
        }
    }
    for v in by_node.values().filter(|v| v.len() == 2) {
        partner[v[0].0][v[0].1] = Some(v[1]);
        partner[v[1].0][v[1].1] = Some(v[0]);
    }
    let via = p.via_mm;
    let rs = packer.map(|pk| {
        refine(
            st,
            pk,
            &pk.taut_dirs(st),
            &mut evl,
            &mask,
            &partner,
            bottom,
            &|a, b| if a == b { 0.0 } else { via },
            &|_, _, _| 0.0,
            &RefineParams { overflow_mm: p.cap_mm, max_evals: p.cap_evals, add_vias: p.cap_vias },
        )
    });
    let rs = rs.unwrap_or_default();
    rep.overfull_before = rs.before;
    rep.overfull_after = rs.after;
    rep.refine_moves = rs.moves;
    rep.mask = mask.clone();
    rep.partner = partner.clone();
    // Labels per (domain, wire) for the same-layer check.
    let mut dom_labels: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    for (w, ev) in events.iter().enumerate() {
        if st.wires[w].alive {
            for (i, e) in ev.iter().enumerate().filter(|(_, e)| e.node > 0) {
                dom_labels.entry(e.node).or_default().push(evl[w][i].2 as u8);
            }
        }
    }
    stretches_and_vias(st, &polys, &evl, p, &mut rep);
    rep.same_layer = dom_labels.values().filter(|l| l.len() == 2 && l[0] == l[1]).count();
    let mut pairs: BTreeMap<(WireId, WireId), Vec2> = BTreeMap::new();
    for &(w, k, o, _) in &unl {
        let p = at(&polys[w], k, 0.5);
        rep.unlayerable.push(p);
        pairs.entry((w, o)).or_insert(p);
    }
    (rep.unlayerable_pairs, rep.unlayerable_at) = pairs.into_iter().unzip();
    rep
}

/// A crossing: `(wire, traversal, other wire, other traversal)` with `wire < other`.
type CrossKey = (WireId, usize, WireId, usize);

/// One event of the N-layer model: a point along a wire whose layer is a variable with the
/// allowed set `mask` (bit per layer); `partner` is the event of the other wire of a
/// different-net crossing (the two must take different layers).
#[derive(Clone, Copy, Debug)]
struct NEv {
    k: usize,
    s: f64,
    mask: u64,
    partner: Option<usize>,
}

/// Prices of the N-layer labelling: `via` per layer change, the plane surcharge per mm (`share`
/// is the length of copper each event stands for) and half a `cut` per change into or out of a
/// plane layer.
struct Costs {
    via: f64,
    plane_from: usize,
    plane_mm: f64,
    cut: f64,
    share: Vec<f64>,
}

impl Costs {
    fn plane(&self, l: usize) -> bool {
        l >= self.plane_from
    }

    fn trans(&self, a: usize, b: usize) -> f64 {
        if a == b {
            0.0
        } else {
            self.via + 0.5 * self.cut * (self.plane(a) as u8 + self.plane(b) as u8) as f64
        }
    }

    fn unary(&self, e: usize, l: usize) -> f64 {
        if self.plane(l) {
            self.plane_mm * self.share[e]
        } else {
            0.0
        }
    }
}

/// Cost of a labelling: layer changes and plane copper per [`Costs`] plus `CONFLICT` per crossing
/// on one layer (counted per event pair). O(events).
fn n_cost(wev: &[Vec<usize>], ev: &[NEv], lab: &[u8], c: &Costs) -> f64 {
    const CONFLICT: f64 = 1e6;
    let mut t = 0.0;
    for list in wev {
        t += list.windows(2).map(|x| c.trans(lab[x[0]] as usize, lab[x[1]] as usize)).sum::<f64>();
    }
    for (i, e) in ev.iter().enumerate() {
        t += c.unary(i, lab[i] as usize);
        if e.partner.is_some_and(|p| p > i && lab[p] == lab[i]) {
            t += CONFLICT;
        }
    }
    t
}

/// P3 for any number of signal layers (plan "Layer assignment for N signal layers"):
/// 1. greedy colouring in wire order (a Viterbi pass per wire over its events: layer changes cost
///    `via`, a layer taken by the crossing partner of an earlier wire costs a large penalty, ties
///    go to the layer with the fewest events so far);
/// 2. for every pair (α, β) of layers, the events labelled α or β are re-solved exactly with the
///    Polubasov reductions ([`DomainGraph`]) while all other labels stay fixed: events whose
///    allowed set holds only one of α, β, or whose partner is fixed, are fixed-orientation (node
///    0), the others are domains (a crossing with both ends free) or lone free nodes;
/// 3. the swap is kept when the total cost drops; repeated until a full sweep improves nothing.
///
/// Complexity: O(events · L²) per greedy wire pass; a sweep is L² / 2 domain-graph solves.
pub fn assign_n(st: &State, smd_layer: &dyn Fn(usize) -> Option<usize>, p: &LayerParams, packer: Option<&Packer>) -> LayerReport {
    let pr = st.proj;
    let nl = pr.n_layers;
    let full: u64 = if nl >= 64 { u64::MAX } else { (1u64 << nl) - 1 };
    let mut rep = LayerReport::default();
    let polys: Vec<Vec<Vec2>> = (0..st.wires.len()).map(|w| if st.wires[w].alive { st.polyline(w) } else { Vec::new() }).collect();
    let mut ev: Vec<NEv> = Vec::new();
    let mut wev: Vec<Vec<usize>> = vec![Vec::new(); st.wires.len()];
    let mut open: BTreeMap<CrossKey, usize> = BTreeMap::new();
    let mut unl: BTreeSet<CrossKey> = BTreeSet::new();
    for (w, wi) in st.wires.iter().enumerate() {
        if !wi.alive {
            continue;
        }
        let pts = &polys[w];
        let n = wi.passes.len();
        let mut push = |ev: &mut Vec<NEv>, e: NEv| {
            ev.push(e);
            wev[w].push(ev.len() - 1);
            ev.len() - 1
        };
        if let Some(l) = smd_layer(wi.pa) {
            push(&mut ev, NEv { k: 0, s: 0.0, mask: 1 << l, partner: None });
        }
        let mut xs = st.crossings(w);
        xs.retain(|x| st.wires[x.other].net != wi.net);
        for (k, &t) in wi.tris.iter().enumerate() {
            let tm = pr.allowed(t, wi.net).expect("wire inside a wall").0;
            // (s, mask, crossing key) of the events of this triangle, in order along the wire.
            let mut here: Vec<(f64, u64, Option<CrossKey>)> = Vec::new();
            if tm != full {
                here.push((0.0, tm, None));
            }
            for x in xs.iter().filter(|x| x.k == k) {
                let o = x.other;
                let (q0, q1) = (polys[o][x.other_k], polys[o][x.other_k + 1]);
                let s = seg_param(pts[k], pts[k + 1], q0, q1);
                let mo = pr.allowed(x.tri, st.wires[o].net).expect("wire inside a wall").0;
                let key = if w < o { (w, k, o, x.other_k) } else { (o, x.other_k, w, k) };
                if tm == mo && tm.count_ones() == 1 {
                    unl.insert(key);
                } else {
                    here.push((s, tm, Some(key)));
                }
            }
            if k < n {
                let e = st.passes[wi.passes[k]].edge;
                let fits = (0..nl)
                    .filter(|&l| pr.fits_alone(e, l, Item { net: wi.net, w: wi.w, s: wi.s }))
                    .fold(0u64, |m, l| m | 1 << l);
                if fits != full && fits != 0 {
                    here.push((1.0, fits, None));
                }
            }
            here.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (s, mask, key) in here {
                let id = push(&mut ev, NEv { k, s, mask, partner: None });
                if let Some(key) = key {
                    match open.remove(&key) {
                        Some(q) => {
                            ev[id].partner = Some(q);
                            ev[q].partner = Some(id);
                        }
                        None => {
                            open.insert(key, id);
                        }
                    }
                }
            }
        }
        if let Some(l) = smd_layer(wi.pb) {
            push(&mut ev, NEv { k: n, s: 1.0, mask: 1 << l, partner: None });
        }
    }
    // Length of copper each event stands for: half of the way to its neighbours along the wire.
    let mut share = vec![0.0; ev.len()];
    for (w, list) in wev.iter().enumerate().filter(|(_, l)| !l.is_empty()) {
        let pts = &polys[w];
        let mut cum = vec![0.0; pts.len()];
        for k in 1..pts.len() {
            cum[k] = cum[k - 1] + (pts[k] - pts[k - 1]).len();
        }
        let total = cum[pts.len() - 1];
        let pos: Vec<f64> = list.iter().map(|&e| cum[ev[e].k] + ev[e].s * (cum[(ev[e].k + 1).min(pts.len() - 1)] - cum[ev[e].k])).collect();
        for (i, &e) in list.iter().enumerate() {
            let lo = if i == 0 { 0.0 } else { 0.5 * (pos[i - 1] + pos[i]) };
            let hi = if i + 1 == list.len() { total } else { 0.5 * (pos[i] + pos[i + 1]) };
            share[e] = (hi - lo) / MM;
        }
    }
    let costs = Costs { via: p.via_mm, plane_from: if p.n_signal == 0 { nl } else { p.n_signal }, plane_mm: p.plane_mm, cut: p.plane_cut, share };
    // 1. Greedy colouring in wire order.
    const NONE: u8 = u8::MAX;
    let mut lab = vec![NONE; ev.len()];
    let mut load = vec![0usize; nl];
    for list in &wev {
        let m = list.len();
        if m == 0 {
            continue;
        }
        // cost[i][l], back[i][l]
        let mut cost = vec![vec![f64::INFINITY; nl]; m];
        let mut back = vec![vec![0usize; nl]; m];
        for (i, &e) in list.iter().enumerate() {
            for l in 0..nl {
                if (ev[e].mask >> l) & 1 == 0 {
                    continue;
                }
                let clash = ev[e].partner.is_some_and(|q| lab[q] == l as u8);
                let here = if clash { 1e6 } else { 0.0 } + load[l] as f64 * 1e-6 + costs.unary(e, l);
                if i == 0 {
                    cost[0][l] = here;
                    continue;
                }
                for l0 in 0..nl {
                    let c = cost[i - 1][l0] + here + costs.trans(l0, l);
                    if c < cost[i][l] - EPS {
                        cost[i][l] = c;
                        back[i][l] = l0;
                    }
                }
            }
        }
        let mut l = (0..nl).min_by(|&a, &b| cost[m - 1][a].total_cmp(&cost[m - 1][b])).expect("layer");
        if !cost[m - 1][l].is_finite() {
            l = (0..nl).find(|&l| (ev[list[m - 1]].mask >> l) & 1 == 1).unwrap_or(0);
        }
        for i in (0..m).rev() {
            lab[list[i]] = l as u8;
            load[l] += 1;
            if i > 0 {
                l = back[i][l];
            }
        }
    }
    // 2./3. Alpha-beta swaps.
    let mut best = n_cost(&wev, &ev, &lab, &costs);
    for _round in 0..20 {
        let mut improved = false;
        for a in 0..nl {
            for b in a + 1..nl {
                let ab = (1u64 << a) | (1u64 << b);
                let active = |e: usize| lab[e] as usize == a || lab[e] as usize == b;
                let mut g_nodes = 1usize;
                // node, pi per active event
                let mut np: Vec<Option<(usize, u8)>> = vec![None; ev.len()];
                let mut dom: BTreeMap<usize, usize> = BTreeMap::new();
                for e in 0..ev.len() {
                    if !active(e) {
                        continue;
                    }
                    let ra = ev[e].mask & ab;
                    let to_pi = |l: usize| (l == b) as u8;
                    np[e] = Some(if ra.count_ones() == 1 {
                        (0, to_pi(ra.trailing_zeros() as usize))
                    } else {
                        match ev[e].partner.filter(|&q| active(q)) {
                            Some(q) => {
                                let rp = ev[q].mask & ab;
                                if rp.count_ones() == 1 {
                                    let f = rp.trailing_zeros() as usize;
                                    (0, to_pi(if f == a { b } else { a }))
                                } else {
                                    let key = e.min(q);
                                    let next = g_nodes;
                                    let d = *dom.entry(key).or_insert(next);
                                    if d == next {
                                        g_nodes += 1;
                                    }
                                    (d, (e > q) as u8)
                                }
                            }
                            None => {
                                g_nodes += 1;
                                (g_nodes - 1, 0)
                            }
                        }
                    });
                }
                let mut g = DomainGraph::new(g_nodes);
                let tv = costs.trans(a, b);
                // Plane surcharge: a free event costs `u(b) - u(a)` more on b than on a.
                for (e, slot) in np.iter().enumerate() {
                    if let Some((nd, pi)) = *slot {
                        let d = costs.unary(e, b) - costs.unary(e, a);
                        if nd != 0 && d != 0.0 {
                            // label b iff o[nd] ^ pi = 1.
                            g.add_term(nd, 0, if pi == 0 { d } else { -d });
                            if pi == 1 {
                                g.k += d;
                            }
                        }
                    }
                }
                for list in &wev {
                    for x in list.windows(2) {
                        if let (Some((n0, p0)), Some((n1, p1))) = (np[x[0]], np[x[1]]) {
                            g.add_edge(n0, n1, if p0 == p1 { tv } else { -tv });
                        }
                    }
                }
                let o = g.solve(p.exact_max, p.restarts, p.seed);
                let mut cand = lab.clone();
                for e in 0..ev.len() {
                    if let Some((nd, pi)) = np[e] {
                        cand[e] = if (o[nd] ^ pi) == 0 { a as u8 } else { b as u8 };
                    }
                }
                let c = n_cost(&wev, &ev, &cand, &costs);
                if c < best - EPS {
                    best = c;
                    lab = cand;
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
    // Capacity-aware refinement of the labelling.
    let mut evl: Vec<Vec<(usize, f64, i32)>> = Vec::with_capacity(wev.len());
    for list in &wev {
        evl.push(list.iter().map(|&e| (ev[e].k, ev[e].s, lab[e] as i32)).collect());
    }
    let mut at_ev: Vec<(WireId, usize)> = vec![(0, 0); ev.len()];
    for (w, list) in wev.iter().enumerate() {
        for (i, &e) in list.iter().enumerate() {
            at_ev[e] = (w, i);
        }
    }
    let mask: Vec<Vec<u64>> = wev.iter().map(|l| l.iter().map(|&e| ev[e].mask).collect()).collect();
    let partner: Vec<Vec<Option<(WireId, usize)>>> =
        wev.iter().map(|l| l.iter().map(|&e| ev[e].partner.map(|q| at_ev[q])).collect()).collect();
    let rs = packer
        .map(|pk| {
            refine(
                st,
                pk,
                &pk.taut_dirs(st),
                &mut evl,
                &mask,
                &partner,
                costs.plane_from.min(nl) as i32 - 1,
                &|a, b| costs.trans(a, b),
                &|w, i, l| costs.unary(wev[w][i], l),
                &RefineParams { overflow_mm: p.cap_mm, max_evals: p.cap_evals, add_vias: p.cap_vias },
            )
        })
        .unwrap_or_default();
    rep.overfull_before = rs.before;
    rep.overfull_after = rs.after;
    rep.refine_moves = rs.moves;
    rep.mask = mask.clone();
    rep.partner = partner.clone();
    for (w, list) in wev.iter().enumerate() {
        for (i, &e) in list.iter().enumerate() {
            lab[e] = evl[w][i].2 as u8;
        }
    }
    rep.domains = ev.iter().enumerate().filter(|(i, e)| e.partner.is_some_and(|q| q > *i)).count();
    // Crossings left on one layer: forced ones (both fixed to the same layer) and any the
    // colouring could not separate; the lower-tier wire of each stays unrouted.
    let mut pairs: BTreeMap<(WireId, WireId), Vec2> = BTreeMap::new();
    let wire_of = |e: usize| wev.iter().position(|l| l.binary_search(&e).is_ok()).expect("event of a wire");
    for (i, e) in ev.iter().enumerate() {
        if let Some(q) = e.partner.filter(|&q| q > i && lab[q] == lab[i]) {
            let (wa, wb) = (wire_of(i), wire_of(q));
            let p = at(&polys[wa], ev[i].k, ev[i].s);
            pairs.entry((wa.min(wb), wa.max(wb))).or_insert(p);
            rep.same_layer += 1;
            rep.unlayerable.push(p);
        }
    }
    for &(w, k, o, _) in &unl {
        let p = at(&polys[w], k, 0.5);
        rep.unlayerable.push(p);
        pairs.entry((w, o)).or_insert(p);
    }
    (rep.unlayerable_pairs, rep.unlayerable_at) = pairs.into_iter().unzip();
    stretches_and_vias(st, &polys, &evl, p, &mut rep);
    rep
}
