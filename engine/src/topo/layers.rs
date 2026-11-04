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
use super::state::{State, TriId, ViaSlot, WireId};
use crate::geom::Vec2;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::{BTreeMap, BTreeSet};

const EPS: f64 = 1e-9;

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
}

impl Default for LayerParams {
    fn default() -> Self {
        LayerParams { via_mm: 25.0, via_site_r: 0.5e6, exact_max: 16, restarts: 64, seed: 1 }
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

/// Assign layers 0/1 to the wires of `st` (two signal layers). `smd_layer(pad)` is the layer of
/// a single-layer pad.
pub fn assign(st: &State, smd_layer: &dyn Fn(usize) -> Option<usize>, p: &LayerParams) -> LayerReport {
    let pr = st.proj;
    let mut rep = LayerReport::default();
    let mut dom: BTreeMap<(WireId, usize, WireId, usize), usize> = BTreeMap::new();
    let mut unl: BTreeSet<(WireId, usize, WireId, usize)> = BTreeSet::new();
    let polys: Vec<Vec<Vec2>> = (0..st.wires.len()).map(|w| st.polyline(w)).collect();
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
    // Labels per (domain, wire) for the same-layer check.
    let mut dom_labels: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    for (w, ev) in events.iter().enumerate() {
        let wi = &st.wires[w];
        if !wi.alive {
            rep.stretches.push(Vec::new());
            continue;
        }
        let pts = &polys[w];
        let lab = |e: &Event| (o[e.node] ^ e.pi) as i32;
        for e in ev.iter().filter(|e| e.node > 0) {
            dom_labels.entry(e.node).or_default().push(lab(e) as u8);
        }
        let n = wi.passes.len();
        let mut pieces: Vec<(i32, Vec<Vec2>)> = Vec::new();
        let Some(first) = ev.first() else {
            // Crosses nothing: the bottom layer (TopoR's default).
            pieces.push((pr.n_layers as i32 - 1, pts.clone()));
            rep.stretches.push(pieces);
            continue;
        };
        pieces.push((lab(first), piece(pts, 0, 0.0, first.k, first.s)));
        for x in ev.windows(2) {
            let (a, b) = (x[0], x[1]);
            let (la, lb) = (lab(&a), lab(&b));
            if la == lb {
                pieces.push((la, piece(pts, a.k, a.s, b.k, b.s)));
                continue;
            }
            // The via in the middle of its interval: a pass point when the stretch spans one.
            let (km, sm) = if b.k > a.k { ((a.k + b.k).div_ceil(2), 0.0) } else { (a.k, 0.5 * (a.s + b.s)) };
            pieces.push((la, piece(pts, a.k, a.s, km, sm)));
            pieces.push((lb, piece(pts, km, sm, b.k, b.s)));
            rep.vias += 1;
            rep.via_pts.push(at(pts, km, sm));
            rep.slots.push(ViaSlot { wire: w, from: (a.k, a.s), to: (b.k, b.s), at: (km, sm) });
            if !site(a.k, b.k, w) {
                rep.vias_no_site += 1;
            }
        }
        let last = ev.last().expect("non-empty");
        pieces.push((lab(last), piece(pts, last.k, last.s, n, 1.0)));
        rep.stretches.push(pieces);
    }
    rep.same_layer = dom_labels.values().filter(|l| l.len() == 2 && l[0] == l[1]).count();
    for &(w, k, ..) in &unl {
        rep.unlayerable.push(at(&polys[w], k, 0.5));
    }
    rep
}
