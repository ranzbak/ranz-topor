//! Stacks and Gauss–Seidel relaxation (21 §5, 25 §2; design note `m1-taut-design.md` §3).
//!
//! Wires are tautened one after another against the static obstacles and the *current* chains
//! of all other wires (a per-layer `Grid` of [`DynObstacle`]s). Anchor stacks give the radius
//! a wire needs at `(anchor, σ)` given the wires nested inside it. Every neighbour lookup goes
//! through a `Grid`; cost per iteration is O(Σ prims · passes · q). Iteration cap and deadline
//! bound the run; no randomness and no hash-order dependence (candidates are sorted).
use crate::geom::{Aabb, Grid, Orient, Prim, Vec2};
use crate::graph::RouteGraph;
use crate::model::Options;
use crate::obstacle::{LayerObstacles, ObsKind, ObstacleSet};
use crate::sketch::Sketch;
use crate::taut::{
    arc_members, corridor_obstacles, tauten, AnchorRef, Chain, DynObstacle, Env, Rail, TautError,
};
use crate::wires::Wire;
use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

/// Two chains are "the same" when all primitives agree within this, nm.
const SAME_TOL_NM: f64 = 1.0;

/// Required centreline-to-copper-edge clearance between wires `i` and `j` incl. ε, nm
/// (`0` for the same net).
fn pair_clearance(obs: &ObstacleSet, wires: &[Wire], i: usize, j: usize) -> f64 {
    let (wi, wj) = (&wires[i], &wires[j]);
    if wi.net != 0 && wi.net == wj.net {
        return 0.0;
    }
    let ci = obs.clearance(wi.net, ObsKind::Track, wi.net, 0.0);
    obs.clearance(wj.net, ObsKind::Track, wi.net, ci) + obs.margin()
}

/// Zero-slack retry (R6): wire `wire` keeps margin 0 (instead of ε) against the listed static
/// obstacles of its layer, `None` = against every anchor / wall. Clearances to other wires keep ε.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ZeroSlack {
    pub wire: usize,
    pub anchors: Option<Vec<usize>>,
    pub walls: Option<Vec<usize>>,
}

impl ZeroSlack {
    fn anchor(&self, wire: usize, a: usize) -> bool {
        wire == self.wire && self.anchors.as_ref().is_none_or(|v| v.binary_search(&a).is_ok())
    }

    fn wall(&self, wire: usize, w: usize) -> bool {
        wire == self.wire && self.walls.as_ref().is_none_or(|v| v.binary_search(&w).is_ok())
    }
}

/// `ρ_anchor` of `wire` at anchor `a`: margin 0 where the zero-slack corridor applies.
fn rho_anchor_zs(
    obs: &ObstacleSet,
    wires: &[Wire],
    wire: usize,
    a: usize,
    zs: Option<&ZeroSlack>,
) -> f64 {
    let w = &wires[wire];
    let r = obs.rho_anchor(w.layer, a, w.net, w.width as f64);
    if zs.is_some_and(|z| z.anchor(wire, a)) {
        r - obs.margin()
    } else {
        r
    }
}

/// Nesting read off the slot paths (Fix-B1 (a), design note `m2-metrize-design.md` §1.1): the
/// slot index of a crossing on an edge incident to an anchor is the exact number of wires between
/// the anchor and the crossing wire there.
#[derive(Clone, Debug, Default)]
pub struct SlotNesting {
    /// `(wire k, (anchor, σ))` -> for every fan edge of the anchor that `k` crosses with the
    /// anchor on side σ, the wires between the anchor and `k`, innermost first.
    pub inner: HashMap<(usize, AnchorRef), Vec<Vec<usize>>>,
    /// `(wire k, anchor)` -> side on which `k`'s slot path passes the anchor; absent when `k`
    /// crosses no fan edge of it, or passes it on both sides.
    pub sides: HashMap<(usize, usize), Orient>,
}

/// [`SlotNesting`] of the wires `conn_of[k]` (graph connection id of wire `k`) from their paths in
/// `g`. Entries of other connections (capacity-only inserts, wires not listed) are skipped.
/// O(Σ occupancy · n_max).
pub fn slot_nesting(g: &RouteGraph, conn_of: &[usize]) -> SlotNesting {
    let pairs: Vec<(usize, usize)> = conn_of.iter().copied().enumerate().collect();
    slot_nesting_of(g, &pairs)
}

/// [`slot_nesting`] of the listed `(wire k, connection id)` pairs (wire ids need not be dense).
pub fn slot_nesting_of(g: &RouteGraph, pairs: &[(usize, usize)]) -> SlotNesting {
    let wire_of: HashMap<usize, usize> = pairs.iter().map(|&(k, c)| (c, k)).collect();
    let mut out = SlotNesting::default();
    let mut both: BTreeSet<(usize, usize)> = BTreeSet::new();
    for &(k, conn) in pairs {
        let Some((cross, faces, _)) = g.path_of(conn) else {
            continue;
        };
        if faces.len() != cross.len() + 1 {
            continue;
        }
        for (&(e, slot), &prev) in cross.iter().zip(faces) {
            let ge = &g.edges[e];
            let (left, right) = g.crossing_lr(e, prev);
            let wires_of = |range: &mut dyn Iterator<Item = usize>| -> Vec<usize> {
                range
                    .filter_map(|i| wire_of.get(&ge.cross[i].conn).copied())
                    .collect()
            };
            // slots nearer `a` than `slot`, nearest first / nearer `b`, nearest first
            let near_a = wires_of(&mut (0..slot.min(ge.cross.len())));
            let near_b = wires_of(&mut (slot + 1..ge.cross.len()).rev());
            for (v, sigma) in [(left, Orient::Ccw), (right, Orient::Cw)] {
                let Some(anchor) = g.verts[v].anchor else {
                    continue;
                };
                let list = if v == ge.a { &near_a } else { &near_b };
                out.inner
                    .entry((k, AnchorRef { anchor, sigma }))
                    .or_default()
                    .push(list.clone());
                match out.sides.insert((k, anchor), sigma) {
                    Some(s) if s != sigma => {
                        both.insert((k, anchor));
                    }
                    _ => {}
                }
            }
        }
    }
    for key in both {
        out.sides.remove(&key);
    }
    out
}

/// 21 §5 fold: radius of `wire` at anchor `a` over the wires `inner` nested inside it, innermost
/// first: `R_j = max(ρ_j, max_{i<j}(R_i + w_i/2 + c(i,j) + w_j/2))`. O(|inner|²).
fn stacked_radius(
    inner: &[usize],
    wire: usize,
    a: usize,
    wires: &[Wire],
    obs: &ObstacleSet,
    zs: Option<&ZeroSlack>,
) -> f64 {
    let mut done: Vec<(usize, f64)> = Vec::with_capacity(inner.len() + 1);
    for &j in inner.iter().chain(std::iter::once(&wire)) {
        let mut r = rho_anchor_zs(obs, wires, j, a, zs);
        for &(i, ri) in &done {
            let need = ri
                + 0.5 * wires[i].width as f64
                + pair_clearance(obs, wires, i, j)
                + 0.5 * wires[j].width as f64;
            r = r.max(need);
        }
        done.push((j, r));
    }
    done.last().map(|x| x.1).unwrap_or(0.0)
}

/// Stack state of one layer. Two modes: hand sketches (`new`: nesting order per `(anchor, σ)`
/// from `stack_order`, membership = wires whose current chain bends there, initially every listed
/// wire) and slot paths (`from_slots`: the wires that can be inside `k` at `(anchor, σ)` are read
/// off the slot indices, per fan edge; of those, the ones whose current chain bends at the anchor
/// count, every one of them before its first tauten). One arc has one radius, so an inner wire
/// running straight past the anchor must not inflate the ring (a header pin gap would not fit).
#[derive(Clone)]
pub struct Stacks {
    pub order: HashMap<AnchorRef, Vec<usize>>,
    members: HashMap<AnchorRef, BTreeSet<usize>>,
    by_wire: HashMap<usize, Vec<AnchorRef>>,
    slots: Option<SlotNesting>,
    /// Slot mode: anchors (by index, either σ) at which each tautened wire's chain has an arc;
    /// a wire without an entry has not been tautened yet and counts everywhere (conservative).
    bends: HashMap<usize, BTreeSet<usize>>,
    /// Slot mode: `outers[j]` = wires with `j` in one of their inner lists, sorted.
    outers: HashMap<usize, Vec<usize>>,
}

fn outers_of(nesting: &SlotNesting) -> HashMap<usize, Vec<usize>> {
    let mut outers: HashMap<usize, Vec<usize>> = HashMap::new();
    for ((k, _), lists) in &nesting.inner {
        for j in lists.iter().flatten() {
            outers.entry(*j).or_default().push(*k);
        }
    }
    for v in outers.values_mut() {
        v.sort_unstable();
        v.dedup();
    }
    outers
}

impl Stacks {
    /// Initial membership is every listed wire (conservative, 21 §5).
    pub fn new(order: HashMap<AnchorRef, Vec<usize>>) -> Stacks {
        let mut members: HashMap<AnchorRef, BTreeSet<usize>> = HashMap::new();
        let mut by_wire: HashMap<usize, Vec<AnchorRef>> = HashMap::new();
        for (a, list) in &order {
            members.entry(*a).or_default().extend(list.iter().copied());
            for &w in list {
                by_wire.entry(w).or_default().push(*a);
            }
        }
        Stacks {
            order,
            members,
            by_wire,
            slots: None,
            bends: HashMap::new(),
            outers: HashMap::new(),
        }
    }

    /// Slot mode (Fix-B1 (a)).
    pub fn from_slots(nesting: &SlotNesting) -> Stacks {
        Stacks {
            order: HashMap::new(),
            members: HashMap::new(),
            by_wire: HashMap::new(),
            slots: Some(nesting.clone()),
            bends: HashMap::new(),
            outers: outers_of(nesting),
        }
    }

    /// Wires whose stack radius may depend on `wire`'s membership: the wires it is nested in
    /// (slot mode) / every wire listed at an anchor it is listed at (hand mode, superset).
    pub fn dependents(&self, wire: usize) -> Vec<usize> {
        if self.slots.is_some() {
            return self.outers.get(&wire).cloned().unwrap_or_default();
        }
        let mut out: Vec<usize> = self
            .order
            .values()
            .filter(|l| l.contains(&wire))
            .flatten()
            .copied()
            .filter(|&w| w != wire)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Slot mode: replace the nesting (the current bends are kept).
    fn set_slots(&mut self, nesting: &SlotNesting) {
        if self.slots.is_some() {
            self.slots = Some(nesting.clone());
            self.outers = outers_of(nesting);
        }
    }

    /// Forget `wire`'s tauten state: it counts everywhere again (slot mode) / at every anchor it
    /// is listed at (hand mode), as before its first tauten.
    fn reset(&mut self, wire: usize) {
        if self.slots.is_some() {
            self.bends.remove(&wire);
            return;
        }
        let listed: Vec<AnchorRef> = self
            .order
            .iter()
            .filter(|(_, l)| l.contains(&wire))
            .map(|(a, _)| *a)
            .collect();
        self.set_members(wire, &listed);
    }

    /// Side on which `wire`'s slot path passes anchor `a` (slot mode only).
    pub fn slot_side(&self, wire: usize, a: usize) -> Option<Orient> {
        self.slots.as_ref().and_then(|s| s.sides.get(&(wire, a)).copied())
    }

    /// Stack radius of `wire` at `a` (21 §5), counting only current members nested inside it.
    /// Unlisted wires are positioned after all listed ones, by id.
    pub fn radius(&self, a: AnchorRef, wire: usize, wires: &[Wire], obs: &ObstacleSet) -> f64 {
        self.radius_zs(a, wire, wires, obs, None)
    }

    /// [`Stacks::radius`] with the zero-slack corridor of `zs` applied to its wire.
    pub fn radius_zs(
        &self,
        a: AnchorRef,
        wire: usize,
        wires: &[Wire],
        obs: &ObstacleSet,
        zs: Option<&ZeroSlack>,
    ) -> f64 {
        if let Some(n) = &self.slots {
            // Max over the fan edges (nested wires in different sectors do not stack), counting
            // the inner wires whose current chain bends at the anchor (21 §5 membership).
            let bends = |j: usize| self.bends.get(&j).is_none_or(|b| b.contains(&a.anchor));
            return n
                .inner
                .get(&(wire, a))
                .map(|lists| {
                    lists
                        .iter()
                        .map(|l| {
                            let l: Vec<usize> = l.iter().copied().filter(|&j| bends(j)).collect();
                            stacked_radius(&l, wire, a.anchor, wires, obs, zs)
                        })
                        .fold(0.0, f64::max)
                })
                .filter(|&r| r > 0.0)
                .unwrap_or_else(|| rho_anchor_zs(obs, wires, wire, a.anchor, zs));
        }
        let listed = self.order.get(&a).map(Vec::as_slice).unwrap_or(&[]);
        let pos = |w: usize| -> (usize, usize) {
            match listed.iter().position(|&x| x == w) {
                Some(p) => (p, w),
                None => (listed.len(), w),
            }
        };
        let mine = pos(wire);
        let mut inner: Vec<usize> = self
            .members
            .get(&a)
            .map(|m| {
                m.iter()
                    .copied()
                    .filter(|&w| w != wire && pos(w) < mine)
                    .collect()
            })
            .unwrap_or_default();
        inner.sort_by_key(|&w| pos(w));
        stacked_radius(&inner, wire, a.anchor, wires, obs, zs)
    }

    /// Replace the memberships of `wire` with `arcs` (the anchors its chain bends at).
    pub fn set_members(&mut self, wire: usize, arcs: &[AnchorRef]) {
        if self.slots.is_some() {
            self.bends
                .insert(wire, arcs.iter().map(|a| a.anchor).collect());
            return;
        }
        if let Some(old) = self.by_wire.remove(&wire) {
            for a in old {
                if let Some(m) = self.members.get_mut(&a) {
                    m.remove(&wire);
                }
            }
        }
        let mut new: Vec<AnchorRef> = Vec::with_capacity(arcs.len());
        for &a in arcs {
            if !new.contains(&a) {
                new.push(a);
            }
        }
        for &a in &new {
            self.members.entry(a).or_default().insert(wire);
        }
        if !new.is_empty() {
            self.by_wire.insert(wire, new);
        }
    }
}

pub struct RelaxResult {
    pub chains: Vec<Result<Chain, TautError>>,
    pub iterations: u32,
    pub converged: bool,
    pub timed_out: bool,
}

/// Current chains of one layer as dynamic obstacles.
#[derive(Clone)]
struct DynLayer {
    grid: Grid<(usize, usize)>,
    store: HashMap<usize, Vec<DynObstacle>>,
}

impl DynLayer {
    fn new(cell: f64) -> Self {
        DynLayer {
            grid: Grid::new(cell),
            store: HashMap::new(),
        }
    }
    fn remove(&mut self, wire: usize) {
        if let Some(items) = self.store.remove(&wire) {
            for (i, d) in items.iter().enumerate() {
                self.grid.remove(d.prim.bbox(), (wire, i));
            }
        }
    }
    fn insert(&mut self, wire: usize, items: Vec<DynObstacle>) {
        for (i, d) in items.iter().enumerate() {
            self.grid.insert(d.prim.bbox(), (wire, i));
        }
        self.store.insert(wire, items);
    }
    /// Wires with a primitive whose box may overlap one of `boxes` grown by `grow` (grid
    /// superset), sorted and deduplicated.
    fn wires_near(&self, boxes: &[Aabb], grow: f64) -> Vec<usize> {
        let mut out: Vec<usize> = boxes
            .iter()
            .flat_map(|b| self.grid.query(b.grow(grow)))
            .map(|(w, _)| w)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

struct WireEnv<'a> {
    k: usize,
    wires: &'a [Wire],
    obs: &'a ObstacleSet,
    stacks: &'a Stacks,
    dynl: &'a DynLayer,
    zs: Option<&'a ZeroSlack>,
}

impl Env for WireEnv<'_> {
    fn layer(&self) -> &LayerObstacles {
        self.obs.layer(self.wires[self.k].layer)
    }
    fn rho_anchor(&self, a: usize) -> f64 {
        rho_anchor_zs(self.obs, self.wires, self.k, a, self.zs)
    }
    fn rho_wall(&self, i: usize) -> f64 {
        let w = &self.wires[self.k];
        let r = self.obs.rho_wall(w.layer, i, w.net, w.width as f64);
        if self.zs.is_some_and(|z| z.wall(self.k, i)) {
            r - self.obs.margin()
        } else {
            r
        }
    }
    fn exempt_anchor(&self, a: usize) -> bool {
        let o = &self.layer().anchors[a];
        ObstacleSet::exempt_anchor(o, self.wires[self.k].net)
    }
    fn exempt_wall(&self, i: usize) -> bool {
        let o = &self.layer().walls[i];
        ObstacleSet::exempt(o.net, &o.tie_nets, self.wires[self.k].net)
    }
    fn dyn_near(&self, bb: Aabb) -> Vec<&DynObstacle> {
        let net = self.wires[self.k].net;
        let mut ids = self.dynl.grid.query(bb);
        ids.sort_unstable();
        ids.into_iter()
            .filter(|&(w, _)| w != self.k)
            .filter_map(|(w, i)| self.dynl.store.get(&w).and_then(|v| v.get(i)))
            .filter(|d| !(d.net != 0 && d.net == net) && d.prim.bbox().overlaps(&bb))
            .collect()
    }
    fn dyn_clearance(&self, d: &DynObstacle) -> f64 {
        pair_clearance(self.obs, self.wires, self.k, d.wire)
            + 0.5 * self.wires[self.k].width as f64
            + 0.5 * d.width
    }
    fn stack_radius(&self, a: AnchorRef) -> f64 {
        self.stacks
            .radius_zs(a, self.k, self.wires, self.obs, self.zs)
    }
    fn query_grow(&self) -> f64 {
        self.obs.query_grow()
    }
    fn slot_side(&self, a: usize) -> Option<Orient> {
        self.stacks.slot_side(self.k, a)
    }
    fn start_rail(&self) -> Option<Rail> {
        self.rail(&self.wires[self.k].start, 1.0)
    }
    fn goal_rail(&self) -> Option<Rail> {
        self.rail(&self.wires[self.k].goal, -1.0)
    }
}

impl WireEnv<'_> {
    /// G1 rail (A14) of a continued dangling terminal: bend radius `w/2 + c` (the net's
    /// clearance) at the centreline. `sign` = +1 leaving (start), -1 arriving (goal: the chain
    /// travels against the hand direction). O(1).
    fn rail(&self, t: &crate::wires::Terminal, sign: f64) -> Option<Rail> {
        let crate::wires::Terminal::Dangling {
            direction: Some(d), ..
        } = t
        else {
            return None;
        };
        let w = &self.wires[self.k];
        let c = self.obs.clearance(w.net, ObsKind::Track, w.net, 0.0);
        Some(Rail {
            dir: d.unit()? * sign,
            r: 0.5 * w.width as f64 + c,
        })
    }
}

/// Dynamic obstacles of a finished chain (prims ↔ seq mapping from `arc_members`).
fn dyn_of(chain: &Chain, wire: &Wire, k: usize, env: &dyn Env) -> Vec<DynObstacle> {
    let arcs = arc_members(chain, env);
    let anchor_of = |pi: usize| {
        arcs.iter()
            .find(|(p, _)| *p == pi)
            .map(|&(_, s)| chain.seq[s])
    };
    chain
        .prims
        .iter()
        .enumerate()
        .map(|(pi, prim)| {
            let (anchor, adjacent) = match prim {
                Prim::Arc(_) => (anchor_of(pi), [None, None]),
                Prim::Seg(_) => (
                    None,
                    [pi.checked_sub(1).and_then(anchor_of), anchor_of(pi + 1)],
                ),
            };
            DynObstacle {
                prim: *prim,
                width: wire.width as f64,
                net: wire.net,
                wire: k,
                anchor,
                adjacent,
            }
        })
        .collect()
}

fn same_chain(a: &Chain, b: &Chain) -> bool {
    if a.prims.len() != b.prims.len() {
        return false;
    }
    let close = |p: Vec2, q: Vec2| (p - q).len() < SAME_TOL_NM;
    a.prims.iter().zip(&b.prims).all(|(p, q)| match (p, q) {
        (Prim::Seg(s), Prim::Seg(t)) => close(s.a, t.a) && close(s.b, t.b),
        (Prim::Arc(s), Prim::Arc(t)) => {
            s.sigma == t.sigma
                && close(s.c, t.c)
                && (s.r - t.r).abs() < SAME_TOL_NM
                && (s.sweep - t.sweep).abs() * s.r < SAME_TOL_NM
                && close(s.start(), t.start())
        }
        _ => false,
    })
}

fn not_relaxed() -> Result<Chain, TautError> {
    Err(TautError::Sleeve("not relaxed".into()))
}

/// Options of one [`Relaxer::run`].
pub struct RunOpts<'a> {
    pub opts: &'a Options,
    pub deadline: Instant,
    /// Zero-slack corridor of one wire (R6); a change against the previous run re-tautens that
    /// wire (old and new) and the wires it is nested in.
    pub zs: Option<&'a ZeroSlack>,
    /// Anytime (45 §5a): on a relaxer that has not run yet, the first sweep is completed
    /// regardless of the deadline, so every wire has a chain (or a definite error) before
    /// `timed_out` can be reported.
    pub anytime: bool,
}

/// Outcome of one [`Relaxer::run`].
#[derive(Clone, Debug, Default)]
pub struct RunStats {
    /// Gauss–Seidel sweeps over the work set (0 when it was empty).
    pub sweeps: u32,
    /// Wires tautened at least once, sorted.
    pub tautened: Vec<usize>,
    /// Wires whose chain changed (incl. `Err` <-> `Ok`), sorted.
    pub changed: Vec<usize>,
    pub converged: bool,
    pub timed_out: bool,
}

/// Gauss–Seidel state kept across metrization calls (Fix-B2, design note §2.2–2.3). Wire `k`
/// is `wires[k]` for its whole life; `set` places / re-sketches it, `remove` takes it out,
/// `set_nesting` replaces the slot nesting, `run` relaxes a work set: the wires queued by those
/// calls, the ones the caller names, and, as chains move, their dynamic neighbours and the
/// wires they are nested in. A wire whose inputs did not change is not tautened; its chain is
/// unchanged by construction (`tauten` is a pure function of seq and environment).
#[derive(Clone)]
pub struct Relaxer {
    wires: Vec<Wire>,
    present: Vec<bool>,
    chains: Vec<Result<Chain, TautError>>,
    /// Current anchor sequence (input of the next tauten).
    seqs: Vec<Vec<AnchorRef>>,
    /// Sketch sequence, restored when a tauten fails.
    sketch_seqs: Vec<Vec<AnchorRef>>,
    stacks: HashMap<u32, Stacks>,
    dyns: HashMap<u32, DynLayer>,
    /// Relax order rank per wire (wider first, then by net, then by id).
    rank: Vec<Rank>,
    /// Wires whose inputs changed since the last run (sorted by rank in `run`).
    queued: BTreeSet<usize>,
    last_zs: Option<ZeroSlack>,
    /// A run has completed its first sweep.
    ran: bool,
    cell: f64,
}

/// Relax order key: wider first, then by net, then by id.
type Rank = (i64, i32, usize);

fn rank_of(w: &Wire, k: usize) -> Rank {
    (-w.width, w.net, k)
}

impl Relaxer {
    /// Slot mode with no wires yet and stacks for `layers`: wires are installed with
    /// [`Relaxer::replace`] as the router allocates them (the multi-layer router, whose wire
    /// ids are leg ids that change layer and connection over a run).
    pub fn with_layers(layers: &[u32], obs: &ObstacleSet) -> Relaxer {
        let empty = SlotNesting::default();
        let mut rx = Self::with_stacks(Vec::new(), obs, &|_| Stacks::from_slots(&empty));
        for &l in layers {
            rx.stacks.entry(l).or_insert_with(|| Stacks::from_slots(&empty));
            rx.dyns
                .entry(l)
                .or_insert_with(|| DynLayer::new(obs.query_grow()));
        }
        rx
    }

    /// Install `w` as wire `k` (growing the tables), replacing the previous occupant of the
    /// slot. The previous wire must have been removed (`!is_present(k)`) so that no state of
    /// it is left in the stacks / dynamic obstacles. The wire's layer must be one of the
    /// layers given to [`Relaxer::with_layers`]. O(1) amortised.
    pub fn replace(&mut self, k: usize, w: Wire) {
        if k >= self.wires.len() {
            let pad = Wire {
                id: 0,
                net: 0,
                layer: w.layer,
                width: 0,
                start: w.start.clone(),
                goal: w.goal.clone(),
                hand: Vec::new(),
                kept_uuids: Vec::new(),
                hand_len: 0.0,
            };
            for j in self.wires.len()..=k {
                let mut p = pad.clone();
                p.id = j;
                self.rank.push(rank_of(&p, j));
                self.wires.push(p);
                self.present.push(false);
                self.chains.push(not_relaxed());
                self.seqs.push(Vec::new());
                self.sketch_seqs.push(Vec::new());
            }
        }
        debug_assert!(!self.present[k], "replace of a present wire");
        self.rank[k] = rank_of(&w, k);
        self.wires[k] = w;
    }

    /// Slot mode (stacks from [`SlotNesting`], `set_nesting`); every wire absent until `set`.
    pub fn new(wires: Vec<Wire>, obs: &ObstacleSet) -> Relaxer {
        let empty = SlotNesting::default();
        Self::with_stacks(wires, obs, &|_| Stacks::from_slots(&empty))
    }

    fn with_stacks(
        wires: Vec<Wire>,
        obs: &ObstacleSet,
        stacks_of: &dyn Fn(u32) -> Stacks,
    ) -> Relaxer {
        let n = wires.len();
        let mut stacks: HashMap<u32, Stacks> = HashMap::new();
        let mut dyns: HashMap<u32, DynLayer> = HashMap::new();
        for w in &wires {
            stacks.entry(w.layer).or_insert_with(|| stacks_of(w.layer));
            dyns.entry(w.layer)
                .or_insert_with(|| DynLayer::new(obs.query_grow()));
        }
        let rank: Vec<Rank> = wires.iter().enumerate().map(|(k, w)| rank_of(w, k)).collect();
        Relaxer {
            present: vec![false; n],
            chains: (0..n).map(|_| not_relaxed()).collect(),
            seqs: vec![Vec::new(); n],
            sketch_seqs: vec![Vec::new(); n],
            stacks,
            dyns,
            rank,
            queued: BTreeSet::new(),
            last_zs: None,
            ran: false,
            cell: obs.query_grow(),
            wires,
        }
    }

    pub fn wires(&self) -> &[Wire] {
        &self.wires
    }

    pub fn chains(&self) -> &[Result<Chain, TautError>] {
        &self.chains
    }

    pub fn is_present(&self, k: usize) -> bool {
        self.present[k]
    }

    /// Place wire `k` with the sketch sequence `seq` (first time, or its path changed): its
    /// chain starts from scratch, the wires around its old chain are queued.
    pub fn set(&mut self, k: usize, seq: &[AnchorRef]) {
        self.drop_chain(k);
        self.present[k] = true;
        self.sketch_seqs[k] = seq.to_vec();
        self.seqs[k] = seq.to_vec();
        self.stacks
            .get_mut(&self.wires[k].layer)
            .expect("layer")
            .reset(k);
        self.queued.insert(k);
    }

    /// Take wire `k` out (its connection left the graph); its neighbours are queued.
    pub fn remove(&mut self, k: usize) {
        self.drop_chain(k);
        self.present[k] = false;
        self.queued.remove(&k);
        self.stacks
            .get_mut(&self.wires[k].layer)
            .expect("layer")
            .set_members(k, &[]);
    }

    /// Forget `k`'s chain and dynamic obstacles; queue the wires that saw them.
    fn drop_chain(&mut self, k: usize) {
        let layer = self.wires[k].layer;
        if let Ok(c) = &self.chains[k] {
            let boxes: Vec<Aabb> = c.prims.iter().map(Prim::bbox).collect();
            let dynl = self.dyns.get_mut(&layer).expect("layer");
            dynl.remove(k);
            for j in dynl.wires_near(&boxes, self.cell) {
                if j != k {
                    self.queued.insert(j);
                }
            }
        }
        self.queued.extend(self.stacks[&layer].dependents(k));
        self.chains[k] = not_relaxed();
    }

    /// Replace the slot nesting (every layer). Wires whose inner lists or slot sides changed are
    /// queued; returned sorted.
    pub fn set_nesting(&mut self, nesting: &SlotNesting) -> Vec<usize> {
        let mut dirty: BTreeSet<usize> = BTreeSet::new();
        if let Some(old) = self.stacks.values().find_map(|s| s.slots.as_ref()) {
            for (key, lists) in &nesting.inner {
                if old.inner.get(key) != Some(lists) {
                    dirty.insert(key.0);
                }
            }
            for key in old.inner.keys() {
                if !nesting.inner.contains_key(key) {
                    dirty.insert(key.0);
                }
            }
            for (key, side) in &nesting.sides {
                if old.sides.get(key) != Some(side) {
                    dirty.insert(key.0);
                }
            }
            for key in old.sides.keys() {
                if !nesting.sides.contains_key(key) {
                    dirty.insert(key.0);
                }
            }
        }
        for s in self.stacks.values_mut() {
            s.set_slots(nesting);
        }
        let dirty: Vec<usize> = dirty.into_iter().filter(|&k| self.present[k]).collect();
        self.queued.extend(dirty.iter().copied());
        dirty
    }

    /// Tauten `k` against the current state. Returns whether its chain changed and the boxes
    /// of its old and new primitives.
    fn tauten_one(
        &mut self,
        k: usize,
        obs: &ObstacleSet,
        zs: Option<&ZeroSlack>,
    ) -> (bool, Vec<Aabb>) {
        let w = &self.wires[k];
        let dynl = self.dyns.get_mut(&w.layer).expect("layer");
        let stacks = self.stacks.get_mut(&w.layer).expect("layer");
        let mut boxes: Vec<Aabb> = match &self.chains[k] {
            Ok(c) => c.prims.iter().map(Prim::bbox).collect(),
            Err(_) => Vec::new(),
        };
        dynl.remove(k);
        let (out, new_dyn, arcs) = {
            let env = WireEnv {
                k,
                wires: &self.wires,
                obs,
                stacks: &*stacks,
                dynl: &*dynl,
                zs,
            };
            match tauten(w.start.at(), w.goal.at(), self.seqs[k].clone(), &env) {
                Ok(c) => {
                    let d = dyn_of(&c, w, k, &env);
                    let arcs: Vec<AnchorRef> = arc_members(&c, &env)
                        .iter()
                        .map(|&(_, s)| c.seq[s])
                        .collect();
                    (Ok(c), d, arcs)
                }
                Err(e) => (Err(e), Vec::new(), Vec::new()),
            }
        };
        let changed = match &out {
            Ok(c) => {
                boxes.extend(c.prims.iter().map(Prim::bbox));
                self.seqs[k] = c.seq.clone();
                !matches!(&self.chains[k], Ok(p) if same_chain(p, c))
            }
            Err(_) => {
                self.seqs[k] = self.sketch_seqs[k].clone();
                self.chains[k].is_ok()
            }
        };
        dynl.insert(k, new_dyn);
        stacks.set_members(k, &arcs);
        self.chains[k] = out;
        (changed, boxes)
    }

    /// Wires whose tauten inputs depend on `k`'s chain: dynamic neighbours of its old / new
    /// primitives and the wires it is nested in.
    fn affected(&self, k: usize, boxes: &[Aabb]) -> Vec<usize> {
        let layer = self.wires[k].layer;
        let mut out = self.dyns[&layer].wires_near(boxes, self.cell);
        out.extend(self.stacks[&layer].dependents(k));
        out.sort_unstable();
        out.dedup();
        out.retain(|&j| j != k && self.present[j]);
        out
    }

    /// Relax the queued wires plus `work` (design note §2.2 step 2): sweeps in relax order; a
    /// wire whose chain changes queues its affected wires — those later in the order into the
    /// current sweep, the others into the next. Stops when nothing is queued (`converged`), at
    /// `relax_iter_cap` sweeps, or at the deadline (checked before every tauten except in the
    /// anytime first sweep).
    pub fn run(&mut self, work: &[usize], obs: &ObstacleSet, ro: &RunOpts) -> RunStats {
        if self.last_zs.as_ref() != ro.zs {
            for z in [self.last_zs.as_ref(), ro.zs].into_iter().flatten() {
                if z.wire < self.wires.len() {
                    self.queued.insert(z.wire);
                    let layer = self.wires[z.wire].layer;
                    self.queued.extend(self.stacks[&layer].dependents(z.wire));
                }
            }
            self.last_zs = ro.zs.cloned();
        }
        self.queued.extend(work.iter().copied());
        let mut cur: BTreeSet<(Rank, usize)> = std::mem::take(&mut self.queued)
            .into_iter()
            .filter(|&k| self.present[k])
            .map(|k| (self.rank[k], k))
            .collect();
        let anytime_first = ro.anytime && !self.ran;
        let mut st = RunStats::default();
        let mut tautened: BTreeSet<usize> = BTreeSet::new();
        let mut changed: BTreeSet<usize> = BTreeSet::new();
        'outer: while !cur.is_empty() && st.sweeps < ro.opts.relax_cap() {
            let first = st.sweeps == 0 && anytime_first;
            let mut next: BTreeSet<(Rank, usize)> = BTreeSet::new();
            while let Some((r, k)) = cur.pop_first() {
                if !first && crate::control::expired(ro.deadline) {
                    st.timed_out = true;
                    // Anytime: the leftovers stay queued for the next call.
                    self.queued.extend(cur.iter().chain(&next).map(|&(_, k)| k));
                    break 'outer;
                }
                tautened.insert(k);
                let (moved, boxes) = self.tauten_one(k, obs, ro.zs);
                if moved {
                    changed.insert(k);
                    for j in self.affected(k, &boxes) {
                        if self.rank[j] > r {
                            cur.insert((self.rank[j], j));
                        } else {
                            next.insert((self.rank[j], j));
                        }
                    }
                }
            }
            st.sweeps += 1;
            cur = next;
        }
        if !st.timed_out {
            if cur.is_empty() {
                st.converged = true;
            } else {
                self.queued.extend(cur.iter().map(|&(_, k)| k));
            }
        }
        self.ran = true;
        st.tautened = tautened.into_iter().collect();
        st.changed = changed.into_iter().collect();
        st
    }
}

/// Gauss–Seidel relaxation. `sketches` is aligned with `wires` (wire id = index); `orders` holds
/// one `stack_order` result per layer. Stops when an iteration changes no chain (within 1 nm),
/// at `opts.relax_iter_cap`, or at `deadline` (checked before every wire; `timed_out`).
/// A wire whose tauten fails keeps `Err` and contributes no dynamic obstacles.
pub fn relax(
    wires: &[Wire],
    sketches: &[Sketch],
    obs: &ObstacleSet,
    orders: &HashMap<u32, HashMap<AnchorRef, Vec<usize>>>,
    opts: &Options,
    deadline: Instant,
) -> RelaxResult {
    relax_zs(wires, sketches, obs, orders, opts, deadline, None)
}

/// Static obstacles pinching `chain` (of wire `k`, tautened under `zs`) within `band`: see
/// [`corridor_obstacles`].
pub fn corridor_static(
    wires: &[Wire],
    k: usize,
    chain: &Chain,
    obs: &ObstacleSet,
    zs: Option<&ZeroSlack>,
    band: f64,
) -> (Vec<usize>, Vec<usize>) {
    let stacks = Stacks::new(HashMap::new());
    let dynl = DynLayer::new(obs.query_grow());
    let env = WireEnv {
        k,
        wires,
        obs,
        stacks: &stacks,
        dynl: &dynl,
        zs,
    };
    corridor_obstacles(chain, &env, band)
}

/// [`relax`] with one wire's zero-slack corridor (R6): that wire is tautened at margin 0 against
/// the obstacles `zs` names; everything else keeps ε.
pub fn relax_zs(
    wires: &[Wire],
    sketches: &[Sketch],
    obs: &ObstacleSet,
    orders: &HashMap<u32, HashMap<AnchorRef, Vec<usize>>>,
    opts: &Options,
    deadline: Instant,
    zs: Option<&ZeroSlack>,
) -> RelaxResult {
    let stacks = |l: u32| Stacks::new(orders.get(&l).cloned().unwrap_or_default());
    relax_impl(wires, sketches, obs, &stacks, opts, deadline, zs)
}

/// [`relax_zs`] for slot paths: stacks from [`SlotNesting`] (wire ids = indices into `wires`,
/// anchors per layer, so one nesting serves every layer). `Relaxer::new` + `run(all)`.
pub fn relax_slots(
    wires: &[Wire],
    sketches: &[Sketch],
    obs: &ObstacleSet,
    nesting: &SlotNesting,
    opts: &Options,
    deadline: Instant,
    zs: Option<&ZeroSlack>,
) -> RelaxResult {
    let stacks = |_: u32| Stacks::from_slots(nesting);
    relax_impl(wires, sketches, obs, &stacks, opts, deadline, zs)
}

fn relax_impl(
    wires: &[Wire],
    sketches: &[Sketch],
    obs: &ObstacleSet,
    stacks_of: &dyn Fn(u32) -> Stacks,
    opts: &Options,
    deadline: Instant,
    zs: Option<&ZeroSlack>,
) -> RelaxResult {
    let n = wires.len();
    let mut rx = Relaxer::with_stacks(wires.to_vec(), obs, stacks_of);
    for k in 0..n {
        let seq: &[AnchorRef] = sketches.get(k).map(|s| s.seq.as_slice()).unwrap_or(&[]);
        rx.set(k, seq);
    }
    let st = rx.run(
        &[],
        obs,
        &RunOpts {
            opts,
            deadline,
            zs,
            anytime: false,
        },
    );
    RelaxResult {
        chains: rx.chains,
        iterations: st.sweeps,
        converged: st.converged,
        timed_out: st.timed_out,
    }
}
