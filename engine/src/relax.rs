//! Stacks and Gauss–Seidel relaxation (21 §5, 25 §2; design note `m1-taut-design.md` §3).
//!
//! Wires are tautened one after another against the static obstacles and the *current* chains
//! of all other wires (a per-layer `Grid` of [`DynObstacle`]s). Anchor stacks give the radius
//! a wire needs at `(anchor, σ)` given the wires nested inside it. Every neighbour lookup goes
//! through a `Grid`; cost per iteration is O(Σ prims · passes · q). Iteration cap and deadline
//! bound the run; no randomness and no hash-order dependence (candidates are sorted).
use crate::geom::{Aabb, Grid, Prim, Vec2};
use crate::model::Options;
use crate::obstacle::{LayerObstacles, ObsKind, ObstacleSet};
use crate::sketch::Sketch;
use crate::taut::{
    arc_members, corridor_obstacles, tauten, AnchorRef, Chain, DynObstacle, Env, TautError,
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
#[derive(Clone, Debug, Default)]
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

/// Stack state of one layer: nesting order (from the sketch) and current membership.
pub struct Stacks {
    pub order: HashMap<AnchorRef, Vec<usize>>,
    members: HashMap<AnchorRef, BTreeSet<usize>>,
    by_wire: HashMap<usize, Vec<AnchorRef>>,
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
        }
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
        let listed = self.order.get(&a).map(Vec::as_slice).unwrap_or(&[]);
        let pos = |w: usize| -> (usize, usize) {
            match listed.iter().position(|&x| x == w) {
                Some(p) => (p, w),
                None => (listed.len(), w),
            }
        };
        let rho = |w: usize| rho_anchor_zs(obs, wires, w, a.anchor, zs);
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
        let mut done: Vec<(usize, f64)> = Vec::with_capacity(inner.len() + 1);
        for &j in inner.iter().chain(std::iter::once(&wire)) {
            let mut r = rho(j);
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

    /// Replace the memberships of `wire` with `arcs`.
    pub fn set_members(&mut self, wire: usize, arcs: &[AnchorRef]) {
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
    let n = wires.len();
    let mut chains: Vec<Result<Chain, TautError>> = (0..n)
        .map(|_| Err(TautError::Sleeve("not relaxed".into())))
        .collect();
    let mut seqs: Vec<Vec<AnchorRef>> = sketches.iter().map(|s| s.seq.clone()).collect();
    seqs.resize(n, Vec::new());
    let mut stacks: HashMap<u32, Stacks> = HashMap::new();
    let mut dyns: HashMap<u32, DynLayer> = HashMap::new();
    for w in wires {
        stacks
            .entry(w.layer)
            .or_insert_with(|| Stacks::new(orders.get(&w.layer).cloned().unwrap_or_default()));
        dyns.entry(w.layer)
            .or_insert_with(|| DynLayer::new(obs.query_grow()));
    }
    let mut proc_order: Vec<usize> = (0..n).collect();
    proc_order.sort_by(|&a, &b| {
        wires[b]
            .width
            .cmp(&wires[a].width)
            .then(wires[a].net.cmp(&wires[b].net))
            .then(a.cmp(&b))
    });
    let mut iterations = 0;
    let mut converged = false;
    let mut timed_out = false;
    'outer: for it in 1..=opts.relax_iter_cap {
        let mut changed = false;
        for &k in &proc_order {
            if Instant::now() >= deadline {
                timed_out = true;
                break 'outer;
            }
            let w = &wires[k];
            let dynl = dyns.get_mut(&w.layer).expect("layer");
            dynl.remove(k);
            let (out, new_dyn, arcs) = {
                let env = WireEnv {
                    k,
                    wires,
                    obs,
                    stacks: &stacks[&w.layer],
                    dynl: &*dynl,
                    zs,
                };
                match tauten(w.start.at(), w.goal.at(), seqs[k].clone(), &env) {
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
            match &out {
                Ok(c) => {
                    changed |= !matches!(&chains[k], Ok(p) if same_chain(p, c));
                    seqs[k] = c.seq.clone();
                }
                Err(_) => {
                    changed |= chains[k].is_ok();
                    seqs[k] = sketches.get(k).map(|s| s.seq.clone()).unwrap_or_default();
                }
            }
            dynl.insert(k, new_dyn);
            stacks
                .get_mut(&w.layer)
                .expect("layer")
                .set_members(k, &arcs);
            chains[k] = out;
        }
        iterations = it;
        if !changed {
            converged = true;
            break;
        }
    }
    RelaxResult {
        chains,
        iterations,
        converged,
        timed_out,
    }
}
