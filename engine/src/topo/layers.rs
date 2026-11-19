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
        self.solve_ties(exact_max, restarts, seed, 0)
    }

    /// [`DomainGraph::solve`] with the ties of the backward pass broken at random when `ties` != 0
    /// (a ChaCha8 stream seeded by `ties`): an eliminated domain whose two orientations cost the
    /// same (an isolated part of the crossing graph, or a balanced neighbourhood) takes either.
    /// The result is an optimum all the same; which of the equal optima differs. Polubasov 2010
    /// («Оптимизация расслоения»): "layerings with the same number of vias are unexpectedly many
    /// ... the layering should be chosen at random, to get varied topological situations" (shopping
    /// list 5.2, the TopoR loop's layer re-optimisation). `ties` = 0: the first optimum
    /// (orientation 0 on a tie), as [`DomainGraph::solve`] always did.
    pub fn solve_ties(&mut self, exact_max: usize, restarts: usize, seed: u64, ties: u64) -> Vec<u8> {
        self.reduce();
        let mut o = vec![0u8; self.adj.len()];
        self.solve_rest(&mut o, exact_max, restarts, seed);
        let mut tie_rng = (ties != 0).then(|| ChaCha8Rng::seed_from_u64(ties));
        for step in self.stack.iter().rev() {
            match step {
                &Step::Merge { a, into, flip } => o[a] = o[into] ^ flip as u8,
                Step::Elim { d, nbrs } => {
                    let cost = |t: u8| -> f64 { nbrs.iter().filter(|&&(x, _)| o[x] != t).map(|p| p.1).sum() };
                    let (c0, c1) = (cost(0), cost(1));
                    o[*d] = match tie_rng.as_mut() {
                        Some(r) if (c1 - c0).abs() <= EPS => r.gen_range(0..2u8),
                        _ => (c1 < c0 - EPS) as u8,
                    };
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

impl DomainGraph {
    /// TW5 (b), the layering escape (docs/dev/design/tw5-escape.md): a seeded walk over the
    /// layerings with the same number of vias as `o` (Polubasov 2010 «Оптимизация расслоения»,
    /// figs. 8-10: "layerings with the same number of vias are unexpectedly many"; of 12 optimal
    /// ones only 2 lead out of the local minimum, so the layering "should be chosen at random";
    /// the gain comes "from a radical change of the topological situation", also with 0 vias).
    /// Call it on the graph as built (before [`DomainGraph::solve_ties`] reduces it), with `o` an
    /// orientation of it (at least `adj.len()` long, `o[0] = 0`). First every connected part of
    /// the graph that does not reach node 0 (no SMD pad, no one-layer triangle fixes it) is
    /// flipped as a whole with probability 1/2: every stretch of its wires on the other layer, the
    /// vias the same (Polubasov 2001 p. 4: only the parity along a wire counts). Then `tries`
    /// cluster moves: a random domain, grown to a random size m (log-uniform in 1..=n) across the edges that carry no via under `o` with
    /// probability 1/2 each and across the via edges with 1/4 (node 0 never; any other optimum
    /// differs from `o` by connected parts with change 0 each, so every optimum stays reachable),
    /// is flipped when that adds no via (the cut value
    /// of its boundary does not rise: the vias move to other stretches of the same wires, or fall
    /// when `o` was no optimum). The cut value never rises, so an optimum stays an optimum.
    /// Returns the domains flipped (a domain may count more than once). Deterministic per `seed`.
    /// O(nodes + edges) for the parts plus O(tries x cluster boundary).
    pub fn plateau_walk(&self, o: &mut [u8], tries: usize, seed: u64) -> usize {
        let n = self.adj.len();
        if n <= 1 || o.len() < n {
            return 0;
        }
        let mut rng = ChaCha8Rng::seed_from_u64(seed ^ 0x7E5C_A9E5_0000_0001);
        let mut flipped = 0usize;
        // Whole parts without node 0.
        let mut part = vec![usize::MAX; n];
        let mut free_nodes: Vec<usize> = Vec::new();
        for s in 1..n {
            if part[s] != usize::MAX {
                continue;
            }
            let mut stack = vec![s];
            let mut members = Vec::new();
            let mut reaches0 = false;
            part[s] = s;
            while let Some(v) = stack.pop() {
                members.push(v);
                for &u in self.adj[v].keys() {
                    if u == 0 {
                        reaches0 = true;
                    } else if part[u] == usize::MAX {
                        part[u] = s;
                        stack.push(u);
                    }
                }
            }
            if !reaches0 && rng.gen_range(0..2u8) == 1 {
                for &v in &members {
                    o[v] ^= 1;
                }
                flipped += members.len();
            }
            free_nodes.extend(members);
        }
        free_nodes.sort_unstable();
        if free_nodes.is_empty() {
            return flipped;
        }
        let cost = |o: &[u8], u: usize, v: usize, w: f64, flip_u: bool| edge_cost(w, (o[u] ^ flip_u as u8) != o[v]);
        let mut in_s = vec![false; n];
        for _ in 0..tries {
            let v = free_nodes[rng.gen_range(0..free_nodes.len())];
            // The cluster's size m, log-uniform in 1..=n (every scale equally likely: single domains as
            // often as whole groups; any size possible). A fixed growth probability per edge alone is
            // supercritical on a board's graph (degree about 4): every cluster swells to a whole part,
            // whose flip almost always adds vias; small sizes only (2^-m, tw5r3) miss the large flips
            // of equal vias (wiccon: 0.0 % of the positions changed).
            let m = ((n as f64).powf(rng.gen::<f64>()).floor() as usize).clamp(1, n);
            let mut s = vec![v];
            in_s[v] = true;
            let mut i = 0;
            while i < s.len() && s.len() < m {
                let x = s[i];
                i += 1;
                for (&u, &w) in &self.adj[x] {
                    // A satisfied edge joins with probability 1/2, a via edge with 1/4: a via inside the
                    // cluster stays a via, so it does not help the flip, but without it the other optima
                    // whose difference holds a via inside are out of reach (any optimum differs from `o` by
                    // connected parts with change 0 each).
                    let p4 = if cost(o, x, u, w, false) == 0.0 { 2 } else { 1 };
                    if s.len() < m && u != 0 && !in_s[u] && rng.gen_range(0..4u8) < p4 {
                        in_s[u] = true;
                        s.push(u);
                    }
                }
            }
            // The change of the cut value on the boundary of `s` when it is flipped.
            let mut delta = 0.0;
            for &x in &s {
                for (&u, &w) in &self.adj[x] {
                    if !in_s[u] {
                        delta += cost(o, x, u, w, true) - cost(o, x, u, w, false);
                    }
                }
            }
            if delta <= EPS {
                for &x in &s {
                    o[x] ^= 1;
                }
                flipped += s.len();
            }
            for &x in &s {
                in_s[x] = false;
            }
        }
        flipped
    }

    /// The cut value of orientation `o` on the graph as built (tests, the escape's check). O(edges).
    pub fn value(&self, o: &[u8]) -> f64 {
        let mut f = 0.0;
        for (u, m) in self.adj.iter().enumerate() {
            for (&v, &w) in m {
                if u < v {
                    f += edge_cost(w, o[u] != o[v]);
                }
            }
        }
        f
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
    /// TW5 (b): domains the layering escape's plateau walk flipped (0 without a walk).
    pub walked: usize,
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
    /// loop-local: the wire pair `(a, b)`, `a <= b`, of each entry of `unlayerable` (same order),
    /// so a local layering can drop the entries of the wires it lays out again.
    pub unlayerable_w: Vec<(WireId, WireId)>,
    /// Over-full edge-layers by the P1 demand model before and after the capacity refinement
    /// (measured with the same model when the refinement is off), and the moves it kept.
    pub overfull_before: usize,
    pub overfull_after: usize,
    pub refine_moves: usize,
    /// par-refine: the refinement's (evaluations, stream hash, end-site memo hash), see
    /// [`super::refine::RefineStats::stream`].
    pub refine_stream: (usize, u64, u64),
    /// Per wire, per event: the layers it may take (bit per layer) and the event of the other
    /// wire of its crossing (the two must differ in layer); for the P4 label moves.
    pub mask: Vec<Vec<u64>>,
    pub partner: Vec<Vec<Option<(WireId, usize)>>>,
    /// The refinement's final capacity model measure ([`super::refine::FinalCap`]; par-refine
    /// item 3), valid while the state and `events` are the ones it was taken for.
    pub fin_cap: Option<super::refine::FinalCap>,
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
    /// Seed of the random choice among layerings with the same price ([`DomainGraph::solve_ties`];
    /// the TopoR loop's layer re-optimisation, shopping list 5.2); 0 = the first optimum, as
    /// before.
    pub ties: u64,
    /// TW5 (b) `[optimize] escape_walk`: the layering escape's plateau walk, tries per domain
    /// ([`DomainGraph::plateau_walk`], with `ties` as its seed); 0 = off (every layering but the
    /// loop's escapes).
    pub walk: usize,
    /// 5.4 `[vias] between_crossings`: a via sits strictly between its two events
    /// ([`super::refine::via_ranges`]); an interval without a traversal there has no site.
    pub via_between: bool,
    /// par-refine: the refinement runs the serial in-place loop (`RefineParams::reference`), the
    /// reference the lane executor is tested against. Off by default.
    pub cap_reference: bool,
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
            ties: 0,
            walk: 0,
            via_between: false,
            cap_reference: false,
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

/// Where crossing `x` lies along traversal `x.k` of wire `w` (fraction `s`, D6b): a traversal
/// with a turn (a junction, D5) runs entry -> turn vertex -> exit, so a crossing on its first
/// sub-chord lies in [0, 0.5), on its second in (0.5, 1], on the right side of the tee event at
/// 0.5 (measured on the straight entry-exit line it could sort to the wrong side and put the via
/// of the tee beyond the crossing: wiccon-2023 3.3v trunk against CS0, D6b). O(turns).
fn cross_s(st: &State, polys: &[Vec<Vec2>], w: WireId, x: &super::state::Crossing) -> f64 {
    let seg = |w: WireId, k: usize, sub: usize| -> (Vec2, Vec2, Option<usize>) {
        let pts = &polys[w];
        match st.wires[w].turns.iter().find(|t| t.0 == k) {
            Some(&(_, v)) if sub == 0 => (pts[k], st.proj.pos[v], Some(0)),
            Some(&(_, v)) => (st.proj.pos[v], pts[k + 1], Some(1)),
            None => (pts[k], pts[k + 1], None),
        }
    };
    let (p0, p1, half) = seg(w, x.k, x.sub);
    let (q0, q1, _) = seg(x.other, x.other_k, x.other_sub);
    let s = seg_param(p0, p1, q0, q1);
    match half {
        Some(0) => 0.5 * s.min(0.998),
        Some(_) => 0.5 + 0.5 * s.max(0.002),
        None => s,
    }
}

/// The stretches, vias and vias without a site of one wire ([`stretches_and_vias`]): the pieces
/// `(layer, points)`, per via its point and slot, and how many vias have no plausible site.
#[derive(Clone, Debug)]
struct WireStretches {
    pieces: Vec<(i32, Vec<Vec2>)>,
    vias: Vec<(Vec2, ViaSlot)>,
    no_site: usize,
}

/// The stretches of live wire `w` from its labelled events `ev` (the body of
/// [`stretches_and_vias`]). Reads the wire's net, triangles, turns, pass count and polyline
/// `polys[w]`, and the static projection ([`StretchKey`]: whether an end site exists does not
/// depend on the crossed chords [`super::refine::end_site`] ranks its candidates by).
/// O(events + passes x cost of `free_radius`).
fn wire_stretches(st: &State, polys: &[Vec<Vec2>], w: WireId, ev: &[(usize, f64, i32)], p: &LayerParams) -> WireStretches {
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
    let wi = &st.wires[w];
    let pts = &polys[w];
    let n = wi.passes.len();
    let mut out = WireStretches { pieces: Vec::new(), vias: Vec::new(), no_site: 0 };
    let Some(&(fk, fs, fl)) = ev.first() else {
        // Crosses nothing: the bottom layer (TopoR's default).
        let bottom = if p.n_signal == 0 { pr.n_layers } else { p.n_signal };
        out.pieces.push((bottom as i32 - 1, pts.clone()));
        return out;
    };
    out.pieces.push((fl, piece(pts, 0, 0.0, fk, fs)));
    let (ranges, between) = super::refine::via_ranges(ev, p.via_between);
    let mut vi = 0usize;
    for x in ev.windows(2) {
        let ((ka, sa, la), (kb, sb, lb)) = (x[0], x[1]);
        if la == lb {
            out.pieces.push((la, piece(pts, ka, sa, kb, sb)));
            continue;
        }
        // The via in the middle of its interval: a pass point when the stretch spans one.
        // With [vias] between_crossings the middle of the traversals strictly between the
        // two events (`ranges`), and no site when there are none.
        let (r, has) = (ranges[vi], between[vi]);
        vi += 1;
        let (km, sm) = if p.via_between && has {
            ((r.0 + r.1).div_ceil(2), 0.0)
        } else if kb > ka {
            ((ka + kb).div_ceil(2), 0.0)
        } else {
            (ka, 0.5 * (sa + sb))
        };
        out.pieces.push((la, piece(pts, ka, sa, km, sm)));
        out.pieces.push((lb, piece(pts, km, sm, kb, sb)));
        out.vias.push((at(pts, km, sm), ViaSlot { wire: w, from: (ka, sa), to: (kb, sb), at: (km, sm) }));
        // 5.2: without a traversal strictly between its events the via may still stand on
        // its own side of an end event ([`super::refine::end_site`]).
        let free = |t: TriId, q: Vec2| pr.inradius(t) >= p.via_site_r && pr.free_radius(q, wi.net) >= p.via_site_r;
        let site_ok = if has { site(r.0, r.1, w) } else { super::refine::end_site(st, w, x[0], x[1], &free).is_some() };
        if !site_ok {
            out.no_site += 1;
        }
    }
    let &(lk, ls, ll) = ev.last().expect("non-empty");
    out.pieces.push((ll, piece(pts, lk, ls, n, 1.0)));
    out
}

/// loop-local Step A (profile F9): a per-wire cache of [`wire_stretches`], so a layering that
/// leaves most wires as they were (every TopoR-loop move is judged by one) does not search
/// their via sites again (`Projection::free_radius` over every obstacle). Exact: an entry is
/// used only when everything [`wire_stretches`] reads for that wire is bit-equal
/// ([`StretchKey`]) under the same parameters and projection, so the reports are bit-identical
/// with and without it. Shared by reference (a mutex, uncontended: one run's layerings are
/// serial).
#[derive(Default)]
pub struct StretchCache {
    inner: std::sync::Mutex<CacheInner>,
}

/// One cached wire: the inputs it was made from and its result.
type CacheEntry = Option<(StretchKey, WireStretches)>;

#[derive(Default)]
struct CacheInner {
    /// (projection address, via_site_r bits, n_signal, via_between) the entries were made under.
    params: Option<(usize, u64, usize, bool)>,
    wires: Vec<CacheEntry>,
    hits: u64,
    misses: u64,
}

/// Everything [`wire_stretches`] reads for one wire (f64 as bits) besides the static projection
/// and the parameters: its net (the free radius against other nets), triangles (the site
/// centroids, the end-site triangle), turns (an end site skips a turning traversal), polyline
/// (the pieces, the pass-point sites; its length gives the pass count) and events. The end site
/// ([`super::refine::end_site`]) also reads the crossed wires' chords, but only to rank the
/// candidate centres: whether a site exists (all the stretch pass asks) does not depend on them.
#[derive(Clone, Debug, PartialEq)]
struct StretchKey {
    net: i32,
    tris: Vec<TriId>,
    turns: Vec<(usize, usize)>,
    poly: Vec<(u64, u64)>,
    ev: Vec<(usize, u64, i32)>,
}

impl StretchKey {
    /// The key of live wire `w` with events `ev`. O(passes + events).
    fn of(st: &State, polys: &[Vec<Vec2>], w: WireId, ev: &[(usize, f64, i32)]) -> StretchKey {
        let wi = &st.wires[w];
        StretchKey {
            net: wi.net,
            tris: wi.tris.clone(),
            turns: wi.turns.clone(),
            poly: polys[w].iter().map(|q| (q.x.to_bits(), q.y.to_bits())).collect(),
            ev: ev.iter().map(|&(k, s, l)| (k, s.to_bits(), l)).collect(),
        }
    }
}

impl StretchCache {
    /// `(hits, misses)` so far.
    pub fn stats(&self) -> (u64, u64) {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (g.hits, g.misses)
    }
}

/// Stretches, via slots and counts of `rep` from the labelled events `evl[w] = (k, s, layer)` of
/// every wire (shared by the two-layer and the N-layer assignment); with `cache`, a wire whose
/// inputs are unchanged takes its last result ([`StretchCache`], bit-identical). O(events).
fn stretches_and_vias(st: &State, polys: &[Vec<Vec2>], evl: &[Vec<(usize, f64, i32)>], p: &LayerParams, rep: &mut LayerReport, cache: Option<&StretchCache>) {
    let mut guard = cache.map(|c| c.inner.lock().unwrap_or_else(|e| e.into_inner()));
    if let Some(g) = guard.as_mut() {
        let params = (st.proj as *const _ as usize, p.via_site_r.to_bits(), p.n_signal, p.via_between);
        if g.params != Some(params) {
            g.params = Some(params);
            g.wires.clear();
        }
        if g.wires.len() < evl.len() {
            g.wires.resize(evl.len(), None);
        }
    }
    for (w, ev) in evl.iter().enumerate() {
        let wi = &st.wires[w];
        if !wi.alive {
            rep.stretches.push(Vec::new());
            rep.events.push(Vec::new());
            continue;
        }
        rep.events.push(ev.clone());
        let ws = match guard.as_mut() {
            None => wire_stretches(st, polys, w, ev, p),
            Some(g) => {
                let key = StretchKey::of(st, polys, w, ev);
                let hit = g.wires[w].as_ref().filter(|(k, _)| *k == key).map(|(_, ws)| ws.clone());
                match hit {
                    Some(ws) => {
                        g.hits += 1;
                        ws
                    }
                    None => {
                        g.misses += 1;
                        let ws = wire_stretches(st, polys, w, ev, p);
                        g.wires[w] = Some((key, ws.clone()));
                        ws
                    }
                }
            }
        };
        for (q, slot) in &ws.vias {
            rep.vias += 1;
            rep.via_pts.push(*q);
            rep.slots.push(*slot);
        }
        rep.vias_no_site += ws.no_site;
        rep.stretches.push(ws.pieces);
    }
}

/// Recompute the stretches, via slots and via counts of `rep` from changed labels `evl` (the P4
/// label moves; the wires and their events are unchanged). O(events + passes).
pub fn relabel(st: &State, evl: &[Vec<(usize, f64, i32)>], p: &LayerParams, rep: &mut LayerReport) {
    relabel_cached(st, evl, p, rep, None)
}

/// [`relabel`] with the per-wire stretch cache ([`StretchCache`]; bit-identical).
pub fn relabel_cached(st: &State, evl: &[Vec<(usize, f64, i32)>], p: &LayerParams, rep: &mut LayerReport, cache: Option<&StretchCache>) {
    let _pg = super::prof::g(super::prof::K::Relabel);
    let polys: Vec<Vec<Vec2>> = (0..st.wires.len()).map(|w| if st.wires[w].alive { st.polyline(w) } else { Vec::new() }).collect();
    rep.stretches.clear();
    rep.events.clear();
    rep.vias = 0;
    rep.via_pts.clear();
    rep.slots.clear();
    rep.vias_no_site = 0;
    stretches_and_vias(st, &polys, evl, p, rep, cache);
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
    assign_cached(st, smd_layer, p, packer, None)
}

/// [`assign_with`] with the per-wire stretch cache ([`StretchCache`]; the same report bit for
/// bit, the untouched wires' via sites not searched again).
pub fn assign_cached(st: &State, smd_layer: &dyn Fn(usize) -> Option<usize>, p: &LayerParams, packer: Option<&Packer>, cache: Option<&StretchCache>) -> LayerReport {
    let _pg = super::prof::g(super::prof::K::AssignWith);
    if super::prof::on() {
        super::prof::units(super::prof::K::AssignWith, st.wires.iter().filter(|w| w.alive).count());
    }
    if st.proj.n_layers == 2 {
        assign_two(st, smd_layer, p, packer, cache, None)
    } else {
        assign_n(st, smd_layer, p, packer, cache, None)
    }
}

/// loop-local (docs/dev/design/loop-local.md): the layering of the wires outside `free` is held
/// as `prev` has it (their events, labels, masks and partners), and only the free wires are
/// laid out again: their events are built afresh, a crossing with a held wire becomes a
/// fixed-orientation event (it must take another layer than the held event), and the same
/// solver runs on the free events alone (two layers: the exact domain-graph solve; more: the
/// greedy colouring and the alpha-beta swaps), then the capacity refinement on the model of the
/// free wires and the wires sharing an edge with them ([`neighbourhood`]), the held events
/// masked to their layers. Dayan 1997 (thesis printed pp. 55-56, PDF pp. 66-67, Algorithm 1 and
/// fig. 34): "when a 2-Net is assigned or reassigned, only the optimal candidate assignments
/// for the 2-Nets affected by this assignment are recomputed". `prev` is the layering of the
/// state before the move (wire ids shared; the new wires are free). O(events of the free wires
/// x their crossings + the local model) plus O(wires) to copy the held lists and the stretch
/// cache's checks.
pub fn assign_local(
    st: &State,
    smd_layer: &dyn Fn(usize) -> Option<usize>,
    p: &LayerParams,
    packer: Option<&Packer>,
    cache: Option<&StretchCache>,
    held: &Held,
) -> LayerReport {
    let _pg = super::prof::g(super::prof::K::AssignWith);
    if super::prof::on() {
        super::prof::units(super::prof::K::AssignWith, st.wires.iter().filter(|w| w.alive).count());
    }
    if st.proj.n_layers == 2 {
        assign_two(st, smd_layer, p, packer, cache, Some(held))
    } else {
        assign_n(st, smd_layer, p, packer, cache, Some(held))
    }
}

/// loop-local: the layering held outside the free wires ([`assign_local`]).
pub struct Held<'r> {
    /// The layering of the state before the move.
    pub prev: &'r LayerReport,
    /// Per wire of the state now: laid out again (a wire past `free`'s end is free).
    pub free: &'r [bool],
}

impl Held<'_> {
    /// Whether wire `w` is laid out again.
    pub fn is_free(&self, w: WireId) -> bool {
        self.free.get(w).copied().unwrap_or(true)
    }

    /// The held event of wire `o`'s traversal `ok` at its crossing with free wire `w`'s traversal
    /// `k` (`w` kept its path, so `prev` has the pair: o's event whose partner is w's event at
    /// `k`) and its label; without it (the pair was not an event, e.g. it could not be layered)
    /// the label `o` has at `ok` by its events. O(events of `o`).
    fn at(&self, o: WireId, ok: usize, w: WireId, k: usize, bottom: i32) -> (Option<usize>, i32) {
        let ev = self.prev.events.get(o).map_or(&[][..], |e| &e[..]);
        let pt = self.prev.partner.get(o).map_or(&[][..], |e| &e[..]);
        for (i, e) in ev.iter().enumerate().filter(|(_, e)| e.0 == ok) {
            if let Some(Some((pw, pj))) = pt.get(i) {
                if *pw == w && self.prev.events.get(w).and_then(|x| x.get(*pj)).is_some_and(|x| x.0 == k) {
                    return (Some(i), e.2);
                }
            }
        }
        let mut l = ev.first().map_or(bottom, |e| e.2);
        for e in ev.iter().filter(|e| e.0 <= ok) {
            l = e.2;
        }
        (None, l)
    }
}

/// loop-local (debug, tests): the partner links of `rep` that do not point at an existing event of
/// a live wire of `st` pointing back (`(wire, event, partner)`). O(events).
pub fn partner_faults(st: &State, rep: &LayerReport) -> Vec<(WireId, usize, (WireId, usize))> {
    let mut out = Vec::new();
    for (w, list) in rep.partner.iter().enumerate() {
        for (i, x) in list.iter().enumerate() {
            if let Some((w2, i2)) = *x {
                let ok = st.wires.get(w2).is_some_and(|x| x.alive)
                    && rep.events.get(w2).is_some_and(|e| i2 < e.len())
                    && rep.partner.get(w2).and_then(|p| p.get(i2)).copied().flatten() == Some((w, i));
                if !ok {
                    out.push((w, i, (w2, i2)));
                }
            }
        }
    }
    out
}

/// loop-local (debug, tests): the different-net crossings of `st` that `rep` neither layers as an
/// event pair nor counts unlayerable while both traversals sit on one equal layer (an unpriced
/// short), as (all of them, those between two wires `free` holds). After an exact local layering
/// both are 0: a crossing a move changed is priced as a pair or as unlayerable (after the
/// loop-local-verify `TOPO_LLV` count, 2026-11-18). O(crossings x log).
pub fn unpriced_crossings(st: &State, rep: &LayerReport, bottom: i32, free: Option<&[bool]>) -> (usize, usize) {
    let mut pairs: std::collections::BTreeSet<(usize, usize, usize, usize)> = std::collections::BTreeSet::new();
    for (w, pl) in rep.partner.iter().enumerate() {
        for (i, p) in pl.iter().enumerate() {
            if let Some((w2, i2)) = *p {
                if let (Some(a), Some(b)) = (rep.events.get(w).and_then(|e| e.get(i)), rep.events.get(w2).and_then(|e| e.get(i2))) {
                    pairs.insert((w, a.0, w2, b.0));
                }
            }
        }
    }
    let unl: std::collections::BTreeSet<(usize, usize)> = rep.unlayerable_w.iter().map(|&(a, b)| (a.min(b), a.max(b))).collect();
    let ln = super::optimise::layers_now(st, &rep.events, bottom);
    let held = |w: usize| free.is_some_and(|f| !f.get(w).copied().unwrap_or(true));
    let (mut all, mut hh) = (0, 0);
    for w in 0..st.wires.len() {
        if !st.wires[w].alive {
            continue;
        }
        for x in st.crossings(w) {
            let o = x.other;
            if o <= w || st.wires[o].net == st.wires[w].net || pairs.contains(&(w, x.k, o, x.other_k)) || unl.contains(&(w, o)) {
                continue;
            }
            let (a, b) = (ln[w].get(x.k).copied().unwrap_or(0), ln[o].get(x.other_k).copied().unwrap_or(0));
            if a.count_ones() == 1 && a == b {
                all += 1;
                hh += usize::from(held(w) && held(o));
            }
        }
    }
    (all, hh)
}

/// loop-local (debug, tests): the events a local layering would keep stale: an event of a wire
/// `free` holds whose crossing partner in `prev` is dead in `st` (or gone past its end). The
/// crossing no longer exists, so the held wire would keep a layer change (a via) for nothing;
/// a judge that lays out again every wire a move killed frees those partners, so this is 0.
/// O(events of the held wires).
pub fn stale_held_events(st: &State, prev: &LayerReport, free: &[bool]) -> usize {
    let dead = |t: usize| st.wires.get(t).is_none_or(|x| !x.alive);
    (0..st.wires.len().min(prev.partner.len()))
        .filter(|&w| st.wires[w].alive && !free.get(w).copied().unwrap_or(true))
        .map(|w| prev.partner[w].iter().filter(|x| x.is_some_and(|(t, _)| dead(t))).count())
        .sum()
}

/// loop-local: the free wires and every wire with a pass on an edge one of them passes (the
/// wires whose packing a free wire's layers can change; [`assign_local`]'s capacity model).
/// O(passes of the free wires x passes per edge).
pub fn neighbourhood(st: &State, free: &[bool]) -> Vec<bool> {
    let mut out = vec![false; st.wires.len()];
    for (w, wi) in st.wires.iter().enumerate().filter(|(w, wi)| wi.alive && free.get(*w).copied().unwrap_or(true)) {
        out[w] = true;
        for &pid in &wi.passes {
            for &q in &st.edge_order[st.passes[pid].edge] {
                out[st.passes[q].wire] = true;
            }
        }
    }
    out
}

/// The free wires' part of a local layering ([`assign_local`]), before it is merged with the
/// held one: per wire (empty for held ones) the labelled events, the refinement's masks and
/// partners (inside the free set), the reported masks (a crossing with a held wire may take
/// either layer in the report), the (free event, held event) crossing pairs, and the crossings
/// left on one layer `((a, b), point)`.
/// The capacity refinement of a local layering: (labels, masks, partners, model wires).
type LocalRefine<'f> = dyn Fn(&mut [Vec<(usize, f64, i32)>], &[Vec<u64>], &[Vec<Option<(WireId, usize)>>], &[bool]) -> super::refine::RefineStats + 'f;
/// One event of a triangle in `assign_n`: (s, mask, crossing key, held partner and its mask).
type HereEv = (f64, u64, Option<CrossKey>, Option<((WireId, usize), u64)>);

struct LocalParts {
    evl: Vec<Vec<(usize, f64, i32)>>,
    rmask: Vec<Vec<u64>>,
    rpartner: Vec<Vec<Option<(WireId, usize)>>>,
    omask: Vec<Vec<u64>>,
    links: Vec<((WireId, usize), (WireId, usize))>,
    unl: Vec<((WireId, WireId), Vec2)>,
}

/// Merge the free wires' layering `lp` with the held one, refine it on the local model and fill
/// the report (stretches through `cache`). `same_unl`: a crossing left on one layer after the
/// refinement is also an unlayerable entry (the N-layer report's rule). O(wires + local model).
#[allow(clippy::too_many_arguments)]
fn finish_local(
    st: &State,
    polys: &[Vec<Vec2>],
    h: &Held,
    mut lp: LocalParts,
    p: &LayerParams,
    cache: Option<&StretchCache>,
    same_unl: bool,
    rep: &mut LayerReport,
    refine_fn: &LocalRefine,
) {
    let nw = st.wires.len();
    let held = |w: WireId| st.wires[w].alive && !h.is_free(w);
    for w in 0..nw {
        if held(w) {
            let ev = h.prev.events.get(w).cloned().unwrap_or_default();
            lp.rmask[w] = ev.iter().map(|e| 1u64 << e.2.clamp(0, 63)).collect();
            lp.rpartner[w] = vec![None; ev.len()];
            lp.evl[w] = ev;
        }
    }
    let only = neighbourhood(st, h.free);
    let rs = refine_fn(&mut lp.evl, &lp.rmask, &lp.rpartner, &only);
    rep.overfull_before = rs.before;
    rep.overfull_after = rs.after;
    rep.refine_moves = rs.moves;
    rep.refine_stream = (rs.evals, rs.stream, rs.memo);
    rep.fin_cap = None;
    // The reported masks and partners: the free wires' own, the held wires' as before with the
    // partners on free wires found again through the crossing pairs.
    let mut mask = lp.omask;
    let mut partner = lp.rpartner;
    for w in 0..nw {
        if held(w) {
            mask[w] = h.prev.mask.get(w).cloned().unwrap_or_default();
            // Only the partners on held wires stay (a free or dead one's events are new).
            partner[w] = h.prev.partner.get(w).cloned().unwrap_or_default().into_iter().map(|x| x.filter(|&(t, _)| t < nw && held(t))).collect();
            mask[w].resize(lp.evl[w].len(), 0);
            partner[w].resize(lp.evl[w].len(), None);
        }
    }
    for &((fw, fi), (hw, hi)) in &lp.links {
        partner[fw][fi] = Some((hw, hi));
        if let Some(x) = partner.get_mut(hw).and_then(|v| v.get_mut(hi)) {
            *x = Some((fw, fi));
        }
    }
    // Crossings on one layer: the held wires' own entries, the free ones' and (N layers) the
    // partner pairs the refinement left on one layer.
    let mut unl: Vec<((WireId, WireId), Vec2)> = Vec::new();
    for (i, &(a, b)) in h.prev.unlayerable_w.iter().enumerate() {
        if a < nw && b < nw && held(a) && held(b) {
            unl.push(((a, b), h.prev.unlayerable[i]));
        }
    }
    unl.append(&mut lp.unl);
    let mut same = 0usize;
    for w in 0..nw {
        for (i, x) in partner[w].iter().enumerate() {
            if let Some((w2, i2)) = *x {
                if (w, i) < (w2, i2) && lp.evl[w2].get(i2).is_some_and(|e| e.2 == lp.evl[w][i].2) {
                    same += 1;
                    if same_unl && (h.is_free(w) || h.is_free(w2)) {
                        unl.push(((w.min(w2), w.max(w2)), at(&polys[w], lp.evl[w][i].0, lp.evl[w][i].1)));
                    }
                }
            }
        }
    }
    rep.same_layer = same;
    stretches_and_vias(st, polys, &lp.evl, p, rep, cache);
    rep.mask = mask;
    rep.partner = partner;
    let mut pairs: BTreeMap<(WireId, WireId), Vec2> = BTreeMap::new();
    for &(ab, q) in &unl {
        rep.unlayerable.push(q);
        rep.unlayerable_w.push(ab);
        pairs.entry(ab).or_insert(q);
    }
    (rep.unlayerable_pairs, rep.unlayerable_at) = pairs.into_iter().unzip();
}

fn assign_two(st: &State, smd_layer: &dyn Fn(usize) -> Option<usize>, p: &LayerParams, packer: Option<&Packer>, cache: Option<&StretchCache>, held: Option<&Held>) -> LayerReport {
    let pr = st.proj;
    let mut rep = LayerReport::default();
    let mut dom: BTreeMap<(WireId, usize, WireId, usize), usize> = BTreeMap::new();
    let mut unl: BTreeSet<(WireId, usize, WireId, usize)> = BTreeSet::new();
    let polys: Vec<Vec<Vec2>> = (0..st.wires.len()).map(|w| if st.wires[w].alive { st.polyline(w) } else { Vec::new() }).collect();
    // The tee rule (Polubasov 2001 §5 p. 9): a degree-3 junction has at least two arms on one
    // layer and the via on the remaining arm, and the tee is one domain (flipping it moves the
    // tee to the other layer). Each junction (D5 net trees: a branch ends where its trunk turns)
    // is a node of its own, nodes 1..=nt: an event on the trunk at its turn and one at the
    // branch's end, both with parity 0, so the solver puts the trunk's two arms and the branch on
    // one layer there and pays a via on whichever arm must leave it.
    let mut tees = super::tree::junctions(st);
    // loop-local: only the free wires' tees (a tree is free or held as a whole).
    let is_held = |w: WireId| held.is_some_and(|h| st.wires[w].alive && !h.is_free(w));
    if held.is_some() {
        tees.retain(|t| !is_held(t.0) && !is_held(t.2));
    }
    let bottom = (if p.n_signal == 0 { pr.n_layers } else { p.n_signal }) as i32 - 1;
    // loop-local: (free event, held event) per crossing of a free wire with a held one.
    let mut links: Vec<((WireId, usize), (WireId, usize))> = Vec::new();
    let nt = tees.len();
    let mut tee_trunk: BTreeMap<(WireId, usize), Vec<usize>> = BTreeMap::new();
    let mut tee_branch: BTreeMap<(WireId, usize), usize> = BTreeMap::new();
    for (i, &(bw, end, tw, k, _)) in tees.iter().enumerate() {
        tee_trunk.entry((tw, k)).or_default().push(i + 1);
        tee_branch.insert((bw, end), i + 1);
    }
    let mut events: Vec<Vec<Event>> = Vec::with_capacity(st.wires.len());
    for (w, wi) in st.wires.iter().enumerate() {
        let mut ev = Vec::new();
        if !wi.alive || is_held(w) {
            events.push(ev);
            continue;
        }
        let n = wi.passes.len();
        if let Some(l) = smd_layer(wi.pa) {
            ev.push(Event { node: 0, pi: l as u8, k: 0, s: 0.0 });
        }
        if let Some(&d) = tee_branch.get(&(w, 0)) {
            ev.push(Event { node: d, pi: 0, k: 0, s: 0.0 });
        }
        let mut xs = st.crossings(w);
        xs.retain(|x| st.wires[x.other].net != wi.net);
        for (k, &t) in wi.tris.iter().enumerate() {
            // A wire placed through another net's wall (4.3 `initial = "once"`, `State::any_path`:
            // placed anyway, its foreign copper priced, Q14b) is restricted to no layer there.
            let mask = pr.allowed(t, wi.net).unwrap_or(super::state::LayerMask::all(pr.n_layers));
            // Events with, for a crossing with a held wire (loop-local), the held event.
            let mut here: Vec<(Event, Option<(WireId, usize)>)> = Vec::new();
            if let Some(l) = mask.single() {
                here.push((Event { node: 0, pi: l as u8, k, s: 0.0 }, None));
            }
            for x in xs.iter().filter(|x| x.k == k) {
                let o = x.other;
                let s = cross_s(st, &polys, w, x);
                let mo = pr.allowed(x.tri, st.wires[o].net).unwrap_or(super::state::LayerMask::all(pr.n_layers));
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
                    (Some(a), None) => here.push((Event { node: 0, pi: a as u8, k, s }, None)),
                    (None, Some(b)) => here.push((Event { node: 0, pi: 1 - b as u8, k, s }, None)),
                    (None, None) if is_held(o) => {
                        // loop-local: the held wire keeps its layer here, this one takes the other.
                        let (i_o, lo) = held.map_or((None, 0), |h| h.at(o, x.other_k, w, k, bottom));
                        here.push((Event { node: 0, pi: (1 - lo.clamp(0, 1)) as u8, k, s }, i_o.map(|i| (o, i))));
                    }
                    (None, None) => {
                        let next = dom.len() + 1 + nt;
                        let d = *dom.entry(key).or_insert(next);
                        here.push((Event { node: d, pi: (w > o) as u8, k, s }, None));
                    }
                }
            }
            for &d in tee_trunk.get(&(w, k)).into_iter().flatten() {
                here.push((Event { node: d, pi: 0, k, s: 0.5 }, None));
            }
            // A pass through a gap too narrow on some layer for this wire alone excludes it.
            if k < n {
                let e = st.passes[wi.passes[k]].edge;
                let pw = st.pass_w.get(wi.passes[k]).copied().unwrap_or(wi.w);
                let fits: Vec<usize> =
                    (0..pr.n_layers).filter(|&l| pr.fits_alone(e, l, Item { net: wi.net, w: pw, s: wi.s })).collect();
                if let [l] = fits[..] {
                    here.push((Event { node: 0, pi: l as u8, k, s: 1.0 }, None));
                }
            }
            here.sort_by(|a, b| a.0.s.total_cmp(&b.0.s));
            for (e, link) in here {
                if let Some(hl) = link {
                    links.push(((w, ev.len()), hl));
                }
                ev.push(e);
            }
        }
        if let Some(&d) = tee_branch.get(&(w, 1)) {
            ev.push(Event { node: d, pi: 0, k: n, s: 1.0 });
        }
        if let Some(l) = smd_layer(wi.pb) {
            ev.push(Event { node: 0, pi: l as u8, k: n, s: 1.0 });
        }
        events.push(ev);
    }
    rep.domains = dom.len() + nt;
    let mut g = DomainGraph::new(dom.len() + 1 + nt);
    for ev in &events {
        for x in ev.windows(2) {
            g.add_edge(x[0].node, x[1].node, if x[0].pi == x[1].pi { p.via_mm } else { -p.via_mm });
        }
    }
    rep.before = g.size();
    let mut reduced = g.clone();
    reduced.reduce();
    rep.after = reduced.size();
    let g0 = (p.walk > 0 && p.ties != 0 && held.is_none()).then(|| g.clone());
    let mut o = g.solve_ties(p.exact_max, p.restarts, p.seed, p.ties);
    if let Some(g0) = g0 {
        rep.walked = g0.plateau_walk(&mut o, p.walk.saturating_mul(rep.domains.max(1)), p.ties);
    }
    if std::env::var("TOPO_TEEDBG").is_ok() {
        for &(bw, end, tw, k, v) in &tees {
            eprintln!("TEEDBG tee branch {bw} end {end} trunk {tw} k {k} v ({:.3},{:.3}) node {}", pr.pos[v].x / 1e6, -pr.pos[v].y / 1e6, tee_branch[&(bw, end)]);
            for w in [tw, bw] {
                let ev: Vec<String> = events[w].iter().map(|e| format!("(k{} s{:.2} n{} pi{} -> {})", e.k, e.s, e.node, e.pi, o[e.node] ^ e.pi)).collect();
                let pts: Vec<String> = polys[w].iter().map(|q| format!("({:.2},{:.2})", q.x / 1e6, -q.y / 1e6)).collect();
                eprintln!("TEEDBG   wire {w} net {} turns {:?} events {}\n          pts {}", st.wires[w].net, st.wires[w].turns, ev.join(" "), pts.join(" "));
            }
        }
    }
    let mut evl: Vec<Vec<(usize, f64, i32)>> = Vec::with_capacity(events.len());
    for ev in events.iter() {
        evl.push(ev.iter().map(|e| (e.k, e.s, (o[e.node] ^ e.pi) as i32)).collect());
    }
    if let Some(h) = held {
        // loop-local: the free wires' masks and partners, then the merge with the held layering.
        let mut rmask: Vec<Vec<u64>> = Vec::with_capacity(events.len());
        let mut rpartner: Vec<Vec<Option<(WireId, usize)>>> = events.iter().map(|ev| vec![None; ev.len()]).collect();
        let mut by_node: BTreeMap<usize, Vec<(WireId, usize)>> = BTreeMap::new();
        for (w, ev) in events.iter().enumerate() {
            rmask.push(ev.iter().enumerate().map(|(i, e)| if e.node <= nt { 1u64 << evl[w][i].2 } else { 0b11 }).collect());
            for (i, e) in ev.iter().enumerate().filter(|(_, e)| e.node > nt) {
                by_node.entry(e.node).or_default().push((w, i));
            }
        }
        for v in by_node.values().filter(|v| v.len() == 2) {
            rpartner[v[0].0][v[0].1] = Some(v[1]);
            rpartner[v[1].0][v[1].1] = Some(v[0]);
        }
        let mut omask = rmask.clone();
        for &((fw, fi), _) in &links {
            omask[fw][fi] = 0b11;
        }
        let unl: Vec<((WireId, WireId), Vec2)> = unl.iter().map(|&(w, k, o, _)| ((w, o), at(&polys[w], k, 0.5))).collect();
        let lp = LocalParts { evl, rmask, rpartner, omask, links, unl };
        let via = p.via_mm;
        let refine_fn = |evl: &mut [Vec<(usize, f64, i32)>], mask: &[Vec<u64>], partner: &[Vec<Option<(WireId, usize)>>], only: &[bool]| {
            packer
                .map(|pk| {
                    super::refine::refine_only(
                        st,
                        pk,
                        &pk.taut_dirs_only(st, only),
                        evl,
                        mask,
                        partner,
                        bottom,
                        &|a, b| if a == b { 0.0 } else { via },
                        &|_, _, _| 0.0,
                        &RefineParams { overflow_mm: p.cap_mm, max_evals: p.cap_evals, add_vias: p.cap_vias, reference: p.cap_reference },
                        Some(only),
                    )
                })
                .unwrap_or_default()
        };
        finish_local(st, &polys, h, lp, p, cache, false, &mut rep, &refine_fn);
        return rep;
    }
    // Capacity-aware refinement: node 0 events are fixed, domain events flip with their partner.
    let mut mask: Vec<Vec<u64>> = Vec::with_capacity(events.len());
    let mut partner: Vec<Vec<Option<(WireId, usize)>>> = events.iter().map(|ev| vec![None; ev.len()]).collect();
    let mut by_node: BTreeMap<usize, Vec<(WireId, usize)>> = BTreeMap::new();
    // A tee's events keep their labels in the refinement (its arms move together or not at all).
    for (w, ev) in events.iter().enumerate() {
        mask.push(ev.iter().enumerate().map(|(i, e)| if e.node <= nt { 1u64 << evl[w][i].2 } else { 0b11 }).collect());
        for (i, e) in ev.iter().enumerate().filter(|(_, e)| e.node > nt) {
            by_node.entry(e.node).or_default().push((w, i));
        }
    }
    for v in by_node.values().filter(|v| v.len() == 2) {
        partner[v[0].0][v[0].1] = Some(v[1]);
        partner[v[1].0][v[1].1] = Some(v[0]);
    }
    let via = p.via_mm;
    let h_in = super::rtrace::on().then(|| super::rtrace::hash(&evl));
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
            &RefineParams { overflow_mm: p.cap_mm, max_evals: p.cap_evals, add_vias: p.cap_vias, reference: p.cap_reference },
        )
    });
    let rs = rs.unwrap_or_default();
    if let Some(h) = h_in {
        super::rtrace::note("a", h, &evl, &rs);
    }
    rep.overfull_before = rs.before;
    rep.overfull_after = rs.after;
    rep.refine_moves = rs.moves;
    rep.refine_stream = (rs.evals, rs.stream, rs.memo);
    rep.fin_cap = rs.fin;
    rep.mask = mask.clone();
    rep.partner = partner.clone();
    // Labels per (domain, wire) for the same-layer check.
    let mut dom_labels: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    for (w, ev) in events.iter().enumerate() {
        if st.wires[w].alive {
            for (i, e) in ev.iter().enumerate().filter(|(_, e)| e.node > nt) {
                dom_labels.entry(e.node).or_default().push(evl[w][i].2 as u8);
            }
        }
    }
    stretches_and_vias(st, &polys, &evl, p, &mut rep, cache);
    rep.same_layer = dom_labels.values().filter(|l| l.len() == 2 && l[0] == l[1]).count();
    let mut pairs: BTreeMap<(WireId, WireId), Vec2> = BTreeMap::new();
    for &(w, k, o, _) in &unl {
        let p = at(&polys[w], k, 0.5);
        rep.unlayerable.push(p);
        rep.unlayerable_w.push((w, o));
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
pub fn assign_n(st: &State, smd_layer: &dyn Fn(usize) -> Option<usize>, p: &LayerParams, packer: Option<&Packer>, cache: Option<&StretchCache>, held: Option<&Held>) -> LayerReport {
    let pr = st.proj;
    let nl = pr.n_layers;
    let full: u64 = if nl >= 64 { u64::MAX } else { (1u64 << nl) - 1 };
    let mut rep = LayerReport::default();
    let polys: Vec<Vec<Vec2>> = (0..st.wires.len()).map(|w| if st.wires[w].alive { st.polyline(w) } else { Vec::new() }).collect();
    let mut ev: Vec<NEv> = Vec::new();
    let mut wev: Vec<Vec<usize>> = vec![Vec::new(); st.wires.len()];
    let mut open: BTreeMap<CrossKey, usize> = BTreeMap::new();
    let mut unl: BTreeSet<CrossKey> = BTreeSet::new();
    // loop-local: held wires, the bottom label, (free event id, held event) per crossing with a
    // held wire, and each free event's mask in the report (before a held partner narrowed it).
    let is_held = |w: WireId| held.is_some_and(|h| st.wires[w].alive && !h.is_free(w));
    let bottom_l = (if p.n_signal == 0 { nl } else { p.n_signal }) as i32 - 1;
    let mut links_id: Vec<(usize, (WireId, usize))> = Vec::new();
    let mut allowed: Vec<u64> = Vec::new();
    for (w, wi) in st.wires.iter().enumerate() {
        if !wi.alive || is_held(w) {
            continue;
        }
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
            // A wire placed through another net's wall (4.3 `initial = "once"`, `State::any_path`:
            // placed anyway, its foreign copper priced, Q14b) is restricted to no layer there.
            let tm = pr.allowed(t, wi.net).map_or(full, |m| m.0);
            // (s, mask, crossing key) of the events of this triangle, in order along the wire;
            // loop-local: a crossing with a held wire carries its held event and the mask
            // without the held layer instead of a key.
            let mut here: Vec<HereEv> = Vec::new();
            if tm != full {
                here.push((0.0, tm, None, None));
            }
            for x in xs.iter().filter(|x| x.k == k) {
                let o = x.other;
                let s = cross_s(st, &polys, w, x);
                let mo = pr.allowed(x.tri, st.wires[o].net).map_or(full, |m| m.0);
                let key = if w < o { (w, k, o, x.other_k) } else { (o, x.other_k, w, k) };
                if tm == mo && tm.count_ones() == 1 {
                    unl.insert(key);
                } else if is_held(o) {
                    let (i_o, lo) = held.map_or((None, 0), |h| h.at(o, x.other_k, w, k, bottom_l));
                    let m = tm & !(1u64 << lo.clamp(0, 63));
                    if m == 0 {
                        unl.insert(key);
                    } else {
                        here.push((s, m, None, i_o.map(|i| ((o, i), tm))));
                    }
                } else {
                    here.push((s, tm, Some(key), None));
                }
            }
            if k < n {
                let e = st.passes[wi.passes[k]].edge;
                let pw = st.pass_w.get(wi.passes[k]).copied().unwrap_or(wi.w);
                let fits = (0..nl)
                    .filter(|&l| pr.fits_alone(e, l, Item { net: wi.net, w: pw, s: wi.s }))
                    .fold(0u64, |m, l| m | 1 << l);
                if fits != full && fits != 0 {
                    here.push((1.0, fits, None, None));
                }
            }
            here.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (s, mask, key, link) in here {
                let id = push(&mut ev, NEv { k, s, mask, partner: None });
                if held.is_some() {
                    allowed.resize(id + 1, 0);
                    allowed[id] = link.map_or(mask, |l| l.1);
                    if let Some((hl, _)) = link {
                        links_id.push((id, hl));
                    }
                }
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
    // 2./3. Alpha-beta swaps. `solve_pair` re-solves the events labelled a or b exactly (the
    // others fixed); with `walk` > 0 the result is moved by the plateau walk (TW5 b) to a random
    // labelling of the same cost.
    let solve_pair = |lab: &[u8], a: usize, b: usize, walk: usize, wseed: u64| -> (Vec<u8>, f64) {
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
        let g0 = (walk > 0).then(|| g.clone());
        let mut o = g.solve_ties(p.exact_max, p.restarts, p.seed, if p.ties == 0 { 0 } else { p.ties ^ ((a as u64) << 8 | b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) });
        if let Some(g0) = g0 {
            g0.plateau_walk(&mut o, walk.saturating_mul(g_nodes), wseed);
        }
        let mut cand = lab.to_vec();
        for e in 0..ev.len() {
            if let Some((nd, pi)) = np[e] {
                cand[e] = if (o[nd] ^ pi) == 0 { a as u8 } else { b as u8 };
            }
        }
        let c = n_cost(&wev, &ev, &cand, &costs);
        (cand, c)
    };
    let escape = p.walk > 0 && p.ties != 0 && held.is_none();
    let mut best = n_cost(&wev, &ev, &lab, &costs);
    for _round in 0..20 {
        let mut improved = false;
        for a in 0..nl {
            for b in a + 1..nl {
                let (cand, c) = solve_pair(&lab, a, b, 0, 0);
                if c < best - EPS {
                    best = c;
                    lab = cand;
                    improved = true;
                } else if escape && c <= best + EPS && cand != lab {
                    // TW5 (Appendix F 7): an equal-cost swap is taken too (random among equals).
                    lab = cand;
                }
            }
        }
        if !improved {
            break;
        }
    }
    // TW5 (b) the layering escape on N layers: one random pair of layers (any two, not only
    // neighbours) re-solved and moved by the plateau walk to a random labelling of the same cost.
    if escape && nl >= 2 {
        let mut rng = ChaCha8Rng::seed_from_u64(p.ties ^ 0x9A12_5EED);
        let a = rng.gen_range(0..nl);
        let b = (a + 1 + rng.gen_range(0..nl - 1)) % nl;
        let (a, b) = (a.min(b), a.max(b));
        let (cand, c) = solve_pair(&lab, a, b, p.walk, p.ties);
        if c <= best + EPS {
            rep.walked = cand.iter().zip(&lab).filter(|(x, y)| x != y).count();
            lab = cand;
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
    if let Some(h) = held {
        // loop-local: the merge with the held layering (the masks in the report without the
        // held partners' narrowing).
        allowed.resize(ev.len(), 0);
        let omask: Vec<Vec<u64>> = wev.iter().map(|l| l.iter().map(|&e| if allowed[e] != 0 { allowed[e] } else { ev[e].mask }).collect()).collect();
        let links: Vec<((WireId, usize), (WireId, usize))> = links_id.iter().map(|&(id, hl)| (at_ev[id], hl)).collect();
        let unl: Vec<((WireId, WireId), Vec2)> = unl.iter().map(|&(w, k, o, _)| ((w, o), at(&polys[w], k, 0.5))).collect();
        rep.domains = ev.iter().enumerate().filter(|(i, e)| e.partner.is_some_and(|q| q > *i)).count() + links.len();
        let lp = LocalParts { evl, rmask: mask, rpartner: partner, omask, links, unl };
        let bottom = costs.plane_from.min(nl) as i32 - 1;
        let refine_fn = |evl: &mut [Vec<(usize, f64, i32)>], mask: &[Vec<u64>], partner: &[Vec<Option<(WireId, usize)>>], only: &[bool]| {
            packer
                .map(|pk| {
                    super::refine::refine_only(
                        st,
                        pk,
                        &pk.taut_dirs_only(st, only),
                        evl,
                        mask,
                        partner,
                        bottom,
                        &|a, b| costs.trans(a, b),
                        &|w, i, l| wev.get(w).and_then(|x| x.get(i)).map_or(0.0, |&e| costs.unary(e, l)),
                        &RefineParams { overflow_mm: p.cap_mm, max_evals: p.cap_evals, add_vias: p.cap_vias, reference: p.cap_reference },
                        Some(only),
                    )
                })
                .unwrap_or_default()
        };
        finish_local(st, &polys, h, lp, p, cache, true, &mut rep, &refine_fn);
        return rep;
    }
    let h_in = super::rtrace::on().then(|| super::rtrace::hash(&evl));
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
                &RefineParams { overflow_mm: p.cap_mm, max_evals: p.cap_evals, add_vias: p.cap_vias, reference: p.cap_reference },
            )
        })
        .unwrap_or_default();
    if let Some(h) = h_in {
        super::rtrace::note("b", h, &evl, &rs);
    }
    rep.overfull_before = rs.before;
    rep.overfull_after = rs.after;
    rep.refine_moves = rs.moves;
    rep.refine_stream = (rs.evals, rs.stream, rs.memo);
    rep.fin_cap = rs.fin;
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
            rep.unlayerable_w.push((wa.min(wb), wa.max(wb)));
        }
    }
    for &(w, k, o, _) in &unl {
        let p = at(&polys[w], k, 0.5);
        rep.unlayerable.push(p);
        rep.unlayerable_w.push((w, o));
        pairs.entry((w, o)).or_insert(p);
    }
    (rep.unlayerable_pairs, rep.unlayerable_at) = pairs.into_iter().unzip();
    stretches_and_vias(st, &polys, &evl, p, &mut rep, cache);
    rep
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every orientation (o[0] = 0) of `n` nodes with its cut value.
    fn all_values(g: &DomainGraph, n: usize) -> Vec<(u64, f64)> {
        (0..(1u64 << (n - 1)))
            .map(|m| {
                let o: Vec<u8> = (0..n).map(|i| if i == 0 { 0 } else { ((m >> (i - 1)) & 1) as u8 }).collect();
                (m, g.value(&o))
            })
            .collect()
    }

    #[test]
    fn the_plateau_walk_keeps_the_optimum_and_reaches_other_optima() {
        let mut rng = ChaCha8Rng::seed_from_u64(11);
        let (mut multi, mut reached_other) = (0, 0);
        for case in 0..300 {
            let n = rng.gen_range(3..=10);
            let m = rng.gen_range(1..=2 * n);
            let mut g = DomainGraph::new(n);
            for _ in 0..m {
                let w = if rng.gen_bool(0.5) { 25.0 } else { -25.0 };
                g.add_edge(rng.gen_range(0..n), rng.gen_range(0..n), w);
            }
            let vals = all_values(&g, n);
            let best = vals.iter().map(|v| v.1).fold(f64::INFINITY, f64::min);
            let optima = vals.iter().filter(|v| (v.1 - best).abs() < 1e-6).count();
            let g0 = g.clone();
            let mut o = g.clone().solve_ties(16, 4, 1, 0);
            o.truncate(n);
            assert!((g0.value(&o) - best).abs() < 1e-6, "case {case}: the solver's optimum");
            let start = o.clone();
            let mut seen_other = false;
            for seed in 1..=20u64 {
                let mut w = start.clone();
                g0.plateau_walk(&mut w, 4 * n, seed);
                assert_eq!(w[0], 0, "node 0 is fixed");
                assert!((g0.value(&w) - best).abs() < 1e-6, "case {case} seed {seed}: the walk left the optimum ({} vs {best})", g0.value(&w));
                seen_other |= w != start;
                let mut again = start.clone();
                g0.plateau_walk(&mut again, 4 * n, seed);
                assert_eq!(again, w, "deterministic per seed");
            }
            if optima > 1 {
                multi += 1;
                reached_other += usize::from(seen_other);
            }
        }
        assert!(multi > 50, "{multi} cases with several optima");
        assert!(reached_other * 10 >= multi * 9, "the walk reached another optimum in {reached_other} of {multi}");
    }

    #[test]
    fn the_plateau_walk_never_raises_a_non_optimum() {
        let mut rng = ChaCha8Rng::seed_from_u64(5);
        for _ in 0..200 {
            let n = rng.gen_range(3..=9);
            let mut g = DomainGraph::new(n);
            for _ in 0..rng.gen_range(1..=3 * n) {
                g.add_edge(rng.gen_range(0..n), rng.gen_range(0..n), 25.0 * rng.gen_range(1..=2) as f64 * if rng.gen_bool(0.5) { 1.0 } else { -1.0 });
            }
            let mut o: Vec<u8> = (0..n).map(|i| if i == 0 { 0 } else { rng.gen_range(0..2) }).collect();
            let v0 = g.value(&o);
            g.plateau_walk(&mut o, 5 * n, rng.gen());
            assert!(g.value(&o) <= v0 + 1e-6);
        }
        // A part without node 0 flips as a whole at random: same vias, every domain moved.
        let mut g = DomainGraph::new(4);
        g.add_edge(1, 2, 25.0);
        g.add_edge(2, 3, -25.0);
        let mut flips = 0;
        for seed in 0..40 {
            let mut o = vec![0u8, 0, 0, 1];
            g.plateau_walk(&mut o, 0, seed);
            assert_eq!(g.value(&o), 0.0);
            flips += usize::from(o == vec![0, 1, 1, 0]);
        }
        assert!((8..=32).contains(&flips), "about half the seeds flip the part: {flips}");
    }
}
