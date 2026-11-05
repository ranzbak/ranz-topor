//! The P4 move set (plan "Pipeline" P4): local changes of the topology state and of the layer
//! labelling, each priced as a cost delta through the factor registry ([`Registry::cost`]: the
//! events of the wire as it would be inserted, own and social factors) and applied only when it
//! lowers the cost.
//!
//! Topological moves (on the [`State`]; a wire's cost is the price of its insert events with the
//! wire itself taken out, so the crossings, squeezes and blocked pads it causes others count):
//! * [`Topo::try_reroute`] routes a wire again on the current state;
//! * [`Topo::try_attach`] connects it to a different pad of its net tree (the tree stays a tree:
//!   the new pad pair spans the same cut of the tree);
//! * [`Topo::try_merge`] puts a stretch next to a parallel stretch of its own net (same-net
//!   neighbours need no clearance between them);
//! * [`Topo::try_channel_swap`] slides a crossing along the run of edges the two wires share,
//!   so it happens where there is room;
//! * [`Topo::try_joint`] takes two crossing wires out and routes them again in the other order;
//! * the double-crossing swap is [`State::uncross_all`] (reused).
//!
//! Capacity test of a topological move: a pass is unsafe when its cut is over-full now (by the
//! drawing's packer, [`Topo::tight`]) or when the passes of the cut, itself included, do not fit on
//! the narrowest layer (the demand model: only then can no layer assignment over-fill it); a move
//! whose new path has more unsafe passes or more over-room passes (the `overflow` events) than the
//! old one is refused whatever its price.
//!
//! Label moves (on the P3 labelling, against the drawing's packer [`CapModel`]): the crossing
//! flip of the capacity refinement ([`refine::try_move`], reused), shifting a via interval over
//! one event along its wire, and segment lifting (a stretch between two layer changes onto
//! another layer). They are kept only when the price (`via` per layer change plus `overflow` per
//! over-full edge-layer) drops, the via count does not rise and no edge-layer they touch gets
//! more over-full (the drawing's capacity test, [`pack`](super::pack)).
//!
//! Deterministic: candidates in wire, event and edge order; Vec + index ids.
//!
//! Complexity: a wire's price is O(path x traversals per triangle); a reroute adds one search
//! (capped by `max_expansions`); a label move is O(passes of the edges it touches).

use super::factors::{Event, Move, Registry, WireCtx};
use super::init::{Conn, InitWeights};
use super::refine::{try_move, CapModel, LEv};
use super::state::{EdgeId, ObsId, State, TriId, Wire, WireId};

const EPS: f64 = 1e-6;

/// The move types, in report order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Reroute,
    Attach,
    Merge,
    Flip,
    ChannelSwap,
    ViaShift,
    Lift,
    DoubleSwap,
    Joint,
}

pub const KINDS: [Kind; 9] = [Kind::Reroute, Kind::Attach, Kind::Merge, Kind::Flip, Kind::ChannelSwap, Kind::ViaShift, Kind::Lift, Kind::DoubleSwap, Kind::Joint];

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Reroute => "reroute",
            Kind::Attach => "attachment",
            Kind::Merge => "merge",
            Kind::Flip => "crossing flip",
            Kind::ChannelSwap => "channel swap",
            Kind::ViaShift => "via shift",
            Kind::Lift => "segment lifting",
            Kind::DoubleSwap => "double-crossing swap",
            Kind::Joint => "joint move",
        }
    }
}

/// Tried / applied counts per [`Kind`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tally {
    pub tried: [usize; 9],
    pub applied: [usize; 9],
}

impl Tally {
    pub fn add(&mut self, o: &Tally) {
        for i in 0..9 {
            self.tried[i] += o.tried[i];
            self.applied[i] += o.applied[i];
        }
    }
    pub fn applied_of(&self, k: Kind) -> usize {
        self.applied[k as usize]
    }
    /// "reroute 3/40, ..." (applied / tried) for the report.
    pub fn text(&self) -> String {
        KINDS.iter().map(|&k| format!("{} {}/{}", k.name(), self.applied[k as usize], self.tried[k as usize])).collect::<Vec<_>>().join(", ")
    }
}

/// What the topological moves work with besides the state.
pub struct Topo {
    pub reg: Registry,
    /// The pad-to-pad connections; an attachment move replaces an entry.
    pub conns: Vec<Conn>,
    /// Tier per connection index.
    pub tiers: Vec<u8>,
    /// Wire id -> connection index; a move that inserts a wire appends.
    pub wire_conn: Vec<usize>,
    /// Reroutes per connection index (the `reroute` factor's counter).
    pub reroutes: Vec<u32>,
    /// w0 + s0 of the default net class: the length unit of the scale.
    pub ws0: f64,
    /// Search cap per connection.
    pub cap: usize,
    /// Move evaluations so far and their cap.
    pub evals: usize,
    pub max_evals: usize,
    /// Wires taken out and routed again so far and their cap.
    pub ripped: usize,
    pub max_ripped: usize,
    /// Edges that are over-full on some layer under the current labelling (the drawing's packer):
    /// a move may not add passes through them (empty = no such test).
    pub tight: Vec<bool>,
    pub tally: Tally,
}

/// A path as the state takes it: `(edge, gap)` per pass, and the triangles.
pub type Path = (Vec<(EdgeId, usize)>, Vec<TriId>);

impl Topo {
    fn ctx(&self, ci: usize) -> WireCtx {
        WireCtx { net: self.conns[ci].net, tier: self.tiers[ci] }
    }
    fn scale(&self, ci: usize) -> f64 {
        let c = &self.conns[ci];
        (c.w + c.s) / self.ws0
    }
    fn weights(&self, ci: usize) -> InitWeights {
        InitWeights { max_expansions: self.cap, ..InitWeights::from_registry(&self.reg, &self.ctx(ci)) }
    }
    /// Whether another evaluation is allowed (and count it).
    pub fn budget(&mut self) -> bool {
        if self.evals >= self.max_evals {
            return false;
        }
        self.evals += 1;
        true
    }
    fn wire_of(c: &Conn) -> Wire {
        Wire { net: c.net, w: c.w, s: c.s, a: c.a, b: c.b, pa: c.pa, pb: c.pb, turns: Vec::new(), passes: Vec::new(), tris: Vec::new(), alive: true }
    }
    /// Price (mm) and capacity count of `wire` inserted along `path` into `st` (which must not
    /// hold it): passes over the room of their cut plus passes through a [`Topo::tight`] edge.
    /// O(path x traversals).
    pub fn path_cost(&self, st: &State, wire: &Wire, path: &[(EdgeId, usize)], tris: &[TriId], ci: usize) -> (f64, usize) {
        let mv = Move::Insert { wire, path, tris, scale: self.scale(ci) };
        let evs = super::factors::events(st, &mv);
        let me = super::demand::Item { net: wire.net, w: wire.w, s: wire.s };
        let pr = st.proj;
        // A pass is safe when all passes of its cut, itself included, fit on the narrowest layer
        // (then no layer split can over-fill it); an unsafe pass counts, and so does a pass
        // through an edge that is over-full now.
        let unsafe_pass = |&(e, g): &(EdgeId, usize)| {
            let room = (0..pr.n_layers).map(|l| pr.room_for(e, l, wire.net)).fold(f64::INFINITY, f64::min);
            self.tight.get(e).copied().unwrap_or(false) || (room.is_finite() && st.edge_need(e, Some((g, me))) > room)
        };
        let over = evs.iter().filter(|e| matches!(e, Event::Overflow { .. })).count() + path.iter().filter(|x| unsafe_pass(x)).count();
        let ctx = self.ctx(ci);
        (evs.iter().map(|e| self.reg.price(e, &ctx)).sum(), over)
    }
    /// What `w` could gain at most by moving: its price minus the price of its straight pad-to-pad
    /// length (the length factor alone). O(path x traversals).
    pub fn potential(&self, st: &mut State, w: WireId) -> f64 {
        let ci = self.wire_conn[w];
        let c = &self.conns[ci];
        let straight = (st.proj.pos[c.b] - st.proj.pos[c.a]).len() / 1e6;
        let floor = self.reg.price(&Event::Length { wire: w, mm: straight, scale: self.scale(ci) }, &self.ctx(ci));
        self.wire_cost(st, w) - floor
    }

    /// Price of the alive wire `w` where it lies. O(path x traversals).
    pub fn wire_cost(&self, st: &mut State, w: WireId) -> f64 {
        let path = st.detach(w);
        let wire = st.wires[w].clone();
        let c = self.path_cost(st, &wire, &path, &wire.tris, self.wire_conn[w]).0;
        st.reattach(w, &path);
        c
    }
    /// The `reroute` factor's price for connection `ci` moving once more.
    fn reroute_price(&self, ci: usize) -> f64 {
        self.reg.price(&Event::Rerouted { wire: 0, count: self.reroutes[ci] }, &self.ctx(ci))
    }
    fn insert_new(&mut self, st: &mut State, ci: usize, (path, tris): Path) -> WireId {
        let id = st.insert(Self::wire_of(&self.conns[ci]), &path, tris);
        if self.wire_conn.len() <= id {
            self.wire_conn.resize(id + 1, usize::MAX);
        }
        self.wire_conn[id] = ci;
        self.reroutes[ci] += 1;
        self.ripped += 1;
        id
    }

    /// Route `w` again on the current state; kept when its price drops by more than the
    /// `reroute` price of its connection. Returns the new wire.
    pub fn try_reroute(&mut self, st: &mut State, w: WireId) -> Option<WireId> {
        if self.ripped >= self.max_ripped || !self.budget() {
            return None;
        }
        self.tally.tried[Kind::Reroute as usize] += 1;
        let ci = self.wire_conn[w];
        let old_path = st.detach(w);
        let old = st.wires[w].clone();
        let (oc, oo) = self.path_cost(st, &old, &old_path, &old.tris, ci);
        let found = st.search(&self.conns[ci], &self.weights(ci), self.scale(ci));
        if let Some((path, tris)) = found {
            if path != old_path || tris != old.tris {
                let (nc, no) = self.path_cost(st, &Self::wire_of(&self.conns[ci]), &path, &tris, ci);
                if no <= oo && nc + self.reroute_price(ci) < oc - EPS {
                    self.tally.applied[Kind::Reroute as usize] += 1;
                    return Some(self.insert_new(st, ci, (path, tris)));
                }
            }
        }
        st.reattach(w, &old_path);
        None
    }

    /// Connect `w`'s connection to a different pad pair across the same cut of its net's tree
    /// (the up to `k` nearest pairs); kept when the best of them is cheaper than the wire now by
    /// more than its `reroute` price. `centre(obs)` is a pad's centre vertex.
    pub fn try_attach(&mut self, st: &mut State, w: WireId, k: usize) -> Option<WireId> {
        let ci = self.wire_conn[w];
        let c = self.conns[ci];
        // Pads of the other connections of the net, joined into components.
        let mut pads: Vec<ObsId> = Vec::new();
        let mut links: Vec<(ObsId, ObsId)> = Vec::new();
        for (j, o) in self.conns.iter().enumerate() {
            if j != ci && o.net == c.net {
                links.push((o.pa, o.pb));
                pads.extend([o.pa, o.pb]);
            }
        }
        pads.extend([c.pa, c.pb]);
        pads.sort_unstable();
        pads.dedup();
        if pads.len() < 3 {
            return None;
        }
        let mut comp: Vec<usize> = (0..pads.len()).collect();
        let find = |comp: &mut Vec<usize>, mut x: usize| {
            while comp[x] != x {
                comp[x] = comp[comp[x]];
                x = comp[x];
            }
            x
        };
        let at = |p: ObsId| pads.binary_search(&p).expect("pad of the net");
        for &(p, q) in &links {
            let (a, b) = (find(&mut comp, at(p)), find(&mut comp, at(q)));
            comp[a.max(b)] = a.min(b);
        }
        let (ra, rb) = (find(&mut comp, at(c.pa)), find(&mut comp, at(c.pb)));
        if ra == rb {
            return None; // a cycle: the connection is redundant here, leave it
        }
        let pos = |p: ObsId| st.centre[p].map(|v| st.proj.pos[v]);
        let mut cands: Vec<(f64, ObsId, ObsId)> = Vec::new();
        for (i, &x) in pads.iter().enumerate() {
            for (j, &y) in pads.iter().enumerate() {
                if find(&mut comp, i) == ra && find(&mut comp, j) == rb && (x, y) != (c.pa, c.pb) {
                    if let (Some(px), Some(py)) = (pos(x), pos(y)) {
                        cands.push(((px - py).len(), x, y));
                    }
                }
            }
        }
        cands.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        cands.truncate(k);
        if cands.is_empty() || self.ripped >= self.max_ripped {
            return None;
        }
        let old_path = st.detach(w);
        let old = st.wires[w].clone();
        let (oc, oo) = self.path_cost(st, &old, &old_path, &old.tris, ci);
        let mut best: Option<(f64, Conn, Path)> = None;
        for &(_, x, y) in &cands {
            if !self.budget() {
                break;
            }
            self.tally.tried[Kind::Attach as usize] += 1;
            let (Some(va), Some(vb)) = (st.centre[x], st.centre[y]) else { continue };
            let nc = Conn { pa: x, pb: y, a: va, b: vb, ..c };
            self.conns[ci] = nc;
            let found = st.search(&nc, &self.weights(ci), self.scale(ci));
            self.conns[ci] = c;
            let Some((path, tris)) = found else { continue };
            let (cost, over) = self.path_cost(st, &Self::wire_of(&nc), &path, &tris, ci);
            if over <= oo && best.as_ref().is_none_or(|b| cost < b.0 - EPS) {
                best = Some((cost, nc, (path, tris)));
            }
        }
        if let Some((cost, nc, p)) = best {
            if cost + self.reroute_price(ci) < oc - EPS {
                self.conns[ci] = nc;
                self.tally.applied[Kind::Attach as usize] += 1;
                return Some(self.insert_new(st, ci, p));
            }
        }
        st.reattach(w, &old_path);
        None
    }

    /// Re-attach `w` (taken out, `old` its old path) with the gaps `new`; kept when cheaper and
    /// not over more cuts. Returns whether the new gaps were kept.
    fn try_gaps(&mut self, st: &mut State, w: WireId, old: &[(EdgeId, usize)], new: &[(EdgeId, usize)], kind: Kind) -> bool {
        self.tally.tried[kind as usize] += 1;
        let wire = st.wires[w].clone();
        let ci = self.wire_conn[w];
        let (oc, oo) = self.path_cost(st, &wire, old, &wire.tris, ci);
        let (nc, no) = self.path_cost(st, &wire, new, &wire.tris, ci);
        if no <= oo && nc < oc - EPS {
            st.reattach(w, new);
            self.tally.applied[kind as usize] += 1;
            true
        } else {
            st.reattach(w, old);
            false
        }
    }

    /// Put the stretches of `v` that run parallel to a stretch of another wire of its net (two or
    /// more shared edges in a row) right next to it. Returns whether a merge was kept.
    pub fn try_merge(&mut self, st: &mut State, v: WireId) -> bool {
        let net = st.wires[v].net;
        let others: Vec<WireId> = (0..st.wires.len()).filter(|&u| u != v && st.wires[u].alive && st.wires[u].net == net).collect();
        for u in others {
            // Runs of v's passes on u's edges with u's pass index stepping by one.
            let ve: Vec<EdgeId> = st.wires[v].passes.iter().map(|&p| st.passes[p].edge).collect();
            let ue: Vec<EdgeId> = st.wires[u].passes.iter().map(|&p| st.passes[p].edge).collect();
            let mut runs: Vec<Vec<(usize, usize)>> = Vec::new(); // (v pass index, u pass index)
            let mut cur: Vec<(usize, usize)> = Vec::new();
            for (j, e) in ve.iter().enumerate() {
                let hit = ue.iter().position(|x| x == e);
                let cont = match (hit, cur.last()) {
                    (Some(k), Some(&(_, k0))) => k + 1 == k0 || k == k0 + 1,
                    _ => false,
                };
                if !cont {
                    if cur.len() >= 2 {
                        runs.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                }
                if let Some(k) = hit {
                    cur.push((j, k));
                }
            }
            if cur.len() >= 2 {
                runs.push(cur);
            }
            for run in runs {
                if !self.budget() {
                    return false;
                }
                let old = st.detach(v);
                let mut new = old.clone();
                for &(j, k) in &run {
                    let e = ve[j];
                    let up = st.wires[u].passes[k];
                    let iu = st.edge_order[e].iter().position(|&x| x == up).expect("pass of u on its edge");
                    new[j].1 = if old[j].1 <= iu { iu } else { iu + 1 };
                }
                if new == old {
                    st.reattach(v, &old);
                    continue;
                }
                if self.try_gaps(st, v, &old, &new, Kind::Merge) {
                    return true;
                }
            }
        }
        false
    }

    /// Slide each crossing of `a` with a different-net wire to the far end of the run of edges
    /// the two share next to it (forward or backward). Returns whether a slide was kept.
    pub fn try_channel_swap(&mut self, st: &mut State, a: WireId) -> bool {
        let xs = st.crossings(a);
        for x in xs {
            let b = x.other;
            if st.wires[b].net == st.wires[a].net {
                continue;
            }
            let ae: Vec<EdgeId> = st.wires[a].passes.iter().map(|&p| st.passes[p].edge).collect();
            let be: Vec<EdgeId> = st.wires[b].passes.iter().map(|&p| st.passes[p].edge).collect();
            // Shared runs leaving the crossing triangle: forward (a's passes k, k+1, ...) and
            // backward (k-1, k-2, ...); b's matching passes in either direction.
            for fwd in [true, false] {
                let mut run: Vec<(usize, usize)> = Vec::new(); // (a pass index, b pass index)
                let mut j = if fwd { x.k } else { x.k.wrapping_sub(1) };
                let mut bj: Option<usize> = None;
                while j < ae.len() {
                    let next_b = match bj {
                        None => [x.other_k, x.other_k.wrapping_sub(1)].into_iter().find(|&q| q < be.len() && be[q] == ae[j]),
                        Some(q) => [q + 1, q.wrapping_sub(1)].into_iter().find(|&r| r < be.len() && be[r] == ae[j] && run.iter().all(|&(_, s)| s != r)),
                    };
                    let Some(q) = next_b else { break };
                    run.push((j, q));
                    bj = Some(q);
                    j = if fwd { j + 1 } else { j.wrapping_sub(1) };
                }
                if run.is_empty() {
                    continue;
                }
                if !self.budget() {
                    return false;
                }
                let old = st.detach(a);
                let mut new = old.clone();
                for &(j, q) in &run {
                    let e = ae[j];
                    let bp = st.wires[b].passes[q];
                    let ib = st.edge_order[e].iter().position(|&p| p == bp).expect("pass of b on its edge");
                    new[j].1 = if old[j].1 <= ib { ib + 1 } else { ib };
                }
                if self.try_gaps(st, a, &old, &new, Kind::ChannelSwap) {
                    return true;
                }
            }
        }
        false
    }

    /// Take the crossing wires `a` and `b` out and route them again, `b` first; kept when the
    /// pair's price (each with the other in place) drops by more than their `reroute` prices.
    /// Returns the new wires.
    pub fn try_joint(&mut self, st: &mut State, a: WireId, b: WireId) -> Option<(WireId, WireId)> {
        if self.ripped + 2 > self.max_ripped || !self.budget() {
            return None;
        }
        self.tally.tried[Kind::Joint as usize] += 1;
        let (ca, cb) = (self.wire_conn[a], self.wire_conn[b]);
        let old = self.wire_cost(st, a) + self.wire_cost(st, b);
        let pa = st.detach(a);
        let pb = st.detach(b);
        let restore = |st: &mut State| {
            st.reattach(b, &pb);
            st.reattach(a, &pa);
        };
        let Some(nb) = st.search(&self.conns[cb], &self.weights(cb), self.scale(cb)) else {
            restore(st);
            return None;
        };
        let wb = st.insert(Self::wire_of(&self.conns[cb]), &nb.0, nb.1.clone());
        let Some(na) = st.search(&self.conns[ca], &self.weights(ca), self.scale(ca)) else {
            st.remove(wb);
            restore(st);
            return None;
        };
        let wa = st.insert(Self::wire_of(&self.conns[ca]), &na.0, na.1.clone());
        let grow = |v: &mut Vec<usize>, id: usize, ci: usize| {
            if v.len() <= id {
                v.resize(id + 1, usize::MAX);
            }
            v[id] = ci;
        };
        grow(&mut self.wire_conn, wb, cb);
        grow(&mut self.wire_conn, wa, ca);
        let new = self.wire_cost(st, wa) + self.wire_cost(st, wb);
        let over = |t: &Topo, st: &mut State, w: WireId| {
            let p = st.detach(w);
            let wi = st.wires[w].clone();
            let o = t.path_cost(st, &wi, &p, &wi.tris, t.wire_conn[w]).1;
            st.reattach(w, &p);
            o
        };
        let new_over = over(self, st, wa) + over(self, st, wb);
        // The old pair's over-room passes, measured the same way.
        st.remove(wa);
        st.remove(wb);
        restore(st);
        let old_over = over(self, st, a) + over(self, st, b);
        if new_over <= old_over && new + self.reroute_price(ca) + self.reroute_price(cb) < old - EPS {
            st.remove(a);
            st.remove(b);
            let wb2 = self.insert_new(st, cb, nb);
            let wa2 = self.insert_new(st, ca, na);
            self.tally.applied[Kind::Joint as usize] += 1;
            return Some((wa2, wb2));
        }
        None
    }
}

/// The labelling the label moves work on.
pub struct Labels<'a> {
    pub st: &'a State<'a>,
    pub cm: CapModel<'a>,
    pub evl: Vec<Vec<LEv>>,
    pub mask: &'a [Vec<u64>],
    pub partner: &'a [Vec<Option<(WireId, usize)>>],
    pub bottom: i32,
    /// Price of one layer change (the `via` factor).
    pub via_mm: f64,
    /// Layers `0..n_signal` are signal layers; wires with a label on a plane are left alone.
    pub n_signal: usize,
    pub evals: usize,
    pub max_evals: usize,
    pub tally: Tally,
}

impl<'a> Labels<'a> {
    fn signal_only(&self, w: WireId) -> bool {
        self.evl[w].iter().all(|e| (e.2 as usize) < self.n_signal)
    }
    /// Whether event `i` of `w` may take layer `l` (allowed there, not its partner's layer).
    fn may(&self, w: WireId, i: usize, l: i32) -> bool {
        l >= 0
            && (l as usize) < self.n_signal
            && (self.mask[w][i] >> l) & 1 == 1
            && self.partner[w][i].is_none_or(|(w2, i2)| self.evl[w2][i2].2 != l)
    }
    fn apply(&mut self, mv: &[((WireId, usize), i32)], kind: Kind) -> bool {
        if self.evals >= self.max_evals {
            return false;
        }
        self.evals += 1;
        self.tally.tried[kind as usize] += 1;
        let via = self.via_mm;
        let ok = try_move(self.st, &mut self.cm, &mut self.evl, mv, self.bottom, &|a, b| if a == b { 0.0 } else { via }, &|_, _, _| 0.0, false, true);
        if ok {
            self.tally.applied[kind as usize] += 1;
        }
        ok
    }

    /// Crossing flips next to a layer change of `w`: the two events of the crossing swap layers.
    pub fn flips(&mut self, w: WireId) -> usize {
        let mut n = 0;
        for i in 0..self.evl[w].len() {
            let Some((w2, i2)) = self.partner[w][i] else { continue };
            let next_to_via = (i > 0 && self.evl[w][i - 1].2 != self.evl[w][i].2) || (i + 1 < self.evl[w].len() && self.evl[w][i + 1].2 != self.evl[w][i].2);
            if !next_to_via || !self.signal_only(w2) {
                continue;
            }
            let (l1, l2) = (self.evl[w][i].2, self.evl[w2][i2].2);
            if l1 == l2 || (self.mask[w][i] >> l2) & 1 == 0 || (self.mask[w2][i2] >> l1) & 1 == 0 || l1 as usize >= self.n_signal || l2 as usize >= self.n_signal {
                continue;
            }
            if self.apply(&[((w, i), l2), ((w2, i2), l1)], Kind::Flip) {
                n += 1;
            }
        }
        n
    }

    /// Shift every layer change of `w` over one event (forward or backward).
    pub fn via_shifts(&mut self, w: WireId) -> usize {
        let mut n = 0;
        for i in 0..self.evl[w].len().saturating_sub(1) {
            let (a, b) = (self.evl[w][i].2, self.evl[w][i + 1].2);
            if a == b {
                continue;
            }
            if self.may(w, i, b) && self.apply(&[((w, i), b)], Kind::ViaShift) {
                n += 1;
                continue;
            }
            if self.may(w, i + 1, a) && self.apply(&[((w, i + 1), a)], Kind::ViaShift) {
                n += 1;
            }
        }
        n
    }

    /// Lift each stretch of `w` between two layer changes (or a change and an end) onto another
    /// signal layer.
    pub fn lifts(&mut self, w: WireId) -> usize {
        let mut n = 0;
        let mut i = 0;
        while i < self.evl[w].len() {
            let l = self.evl[w][i].2;
            let mut j = i;
            while j + 1 < self.evl[w].len() && self.evl[w][j + 1].2 == l {
                j += 1;
            }
            let has_change = i > 0 || j + 1 < self.evl[w].len();
            if has_change {
                for t in 0..self.n_signal as i32 {
                    if t != l && (i..=j).all(|k| self.may(w, k, t)) {
                        let mv: Vec<((WireId, usize), i32)> = (i..=j).map(|k| ((w, k), t)).collect();
                        if self.apply(&mv, Kind::Lift) {
                            n += 1;
                            break;
                        }
                    }
                }
            }
            i = j + 1;
        }
        n
    }

    /// Wires with a layer change on signal layers only, in id order.
    pub fn candidates(&self) -> Vec<WireId> {
        (0..self.evl.len()).filter(|&w| self.st.wires[w].alive && self.signal_only(w) && self.evl[w].windows(2).any(|x| x[0].2 != x[1].2)).collect()
    }
}
