//! PathFinder negotiation over layers and via sites (M2 / T2.3b, M3 / T3.1; design notes
//! `m2-search-design.md` §4-5, `m3-layers-vias-design.md` §4): history costs, failure bumps,
//! site history, best-iteration bookkeeping and clinch detection.
//!
//! One `iteration` re-routes the active connections in `decompose::order` (bumps are a bounded
//! priority adjustment, never a permutation). `route` metrizes / self-checks the result and feeds
//! back through `penalise_failed`, `penalise_blocked` and `penalise_overuse`.
//! Complexity per iteration: O(m · V' log V') searches (V' = states of all layers plus sites)
//! plus O(Σ_L E_L + S) for the overuse sweep.
//! Batch mode (T3.11, note §7): all active connections leave the graph, are searched in
//! parallel against that frozen graph and committed in order; a path whose faces changed since
//! the freeze (edge versions, sites) is first re-validated in O(k) against the current graph
//! (`search::revalidate_path`, F4) and searched again, in order, only when that fails.
//! At the deadline the paths found before it are still committed in order (no search runs after
//! it: an invalidated path is dropped), so a timed-out iteration leaves its committed subset in
//! the graph for the anytime result (ruling Q2). The absurd-detour ghost is
//! computed in the parallel phase and reused while its own faces are unchanged. A connection
//! blocked before any search by a static fact of the board (a terminal that cannot be left /
//! entered, `search::terminal_open`, F3) is never searched again (`dead`). Rip-up is damped (F2, scaling diagnosis RC-A2): a connection rips a given
//! blocker at most [`RIP_PAIR_CAP`] times per run, so two connections that keep ripping each
//! other out of a shared class stop after a bounded exchange and the history they left decides.
//! Every decision is a function of the (deterministic) search results, so the outcome does not
//! depend on the thread count.
//! No `HashMap` iteration reaches any output; the wall clock is checked before every search.

use crate::decompose::{order, Connection};
use crate::geom::{Aabb, Vec2};
use crate::model::Options;
use crate::multigraph::MultiGraph;
use crate::obstacle::ObstacleSet;
use crate::search::{
    default_expansion_cap, ghost, revalidate_path, search, strand_blockers, tier_weight,
    via_weight, Goal, SearchOutcome, SearchParams, SlotPath,
};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

pub const HIST_FAIL: f64 = 1.0;
pub const HIST_BLOCK: f64 = 0.5;
/// History added to a via site on a block / rip-up (as an edge, `HIST_BLOCK`).
pub const HIST_SITE: f64 = HIST_BLOCK;
pub const HIST_OVERUSE: f64 = 0.5;
pub const PRESENT_OVERUSE: f64 = crate::search::PRESENT_OVERUSE;
/// A found path longer than this factor times its ghost path counts as an absurd detour (fix B).
pub const DETOUR_FACTOR: f64 = 2.0;
/// Most blockers one connection rips per iteration (keeps the churn bounded).
pub const MAX_RIP: usize = 3;
/// Damping (F2, scaling diagnosis RC-A2): a connection rips a given blocker at most this many
/// times per run. One exchange leaves only `HIST_BLOCK` on the blocker's class, too little to
/// settle a three-way contention (the F.Cu-only eprom board loses +5V with a cap of 1); three
/// leave `3 * HIST_BLOCK = 1.5`, the scale of one failure (`HIST_FAIL`). Total rips per run are
/// then bounded by `RIP_PAIR_CAP * m^2` whatever the length comparisons do.
pub const RIP_PAIR_CAP: u32 = 3;

/// Clinches found in a routing: `(conn, edge)` pairs (edges of the layer graphs, all layers).
#[derive(Clone, Debug, Default, Serialize)]
pub struct ClinchReport {
    pub regular: Vec<(usize, usize)>,
    pub u: Vec<(usize, usize)>,
}

/// Why a connection found no path (ghost search result).
#[derive(Clone, Debug)]
pub struct BlockInfo {
    /// First soft-blocked edge of the ghost, `(layer_idx, edge)` (P5).
    pub edge: Option<(usize, usize)>,
    /// First soft-blocked element is an owned / blocked via site (P5).
    pub site: Option<usize>,
    pub ghost: Option<SlotPath>,
    /// Crossing before the blocked element, `(leg, index in leg.crossings)` (P5).
    pub last_reached: Option<(usize, usize)>,
    /// Blocked by a static fact of the board (F3): the reason, reported as is; such a
    /// connection is `dead`.
    pub static_reason: Option<String>,
}

#[derive(Clone, Debug)]
pub struct IterOutcome {
    /// Indexed like `conns`; `None` for blocked / never routed connections.
    pub paths: Vec<Option<SlotPath>>,
    pub blocked: BTreeMap<usize, BlockInfo>,
    /// Overused edges over all layers.
    pub overuse_edges: usize,
    pub timed_out: bool,
    /// Connections ripped up this iteration because their strands (or vias) block another
    /// connection's ghost path (fix B); they belong to the next active set.
    pub ripped: Vec<usize>,
    /// Paths of the batch invalidated by an earlier commit and searched again (note §7).
    pub resolved_sequentially: usize,
    /// Paths of the batch invalidated by an earlier commit and accepted by the O(k)
    /// re-validation against the current graph (F4), slots recomputed.
    pub revalidated: usize,
    /// Profile: seconds of the parallel search phase (2) and of the commit phase (3, including
    /// the sequential re-searches), and the number of connections searched in phase 2.
    pub search_s: f64,
    pub commit_s: f64,
    pub searched: usize,
}

/// PathFinder state over the layers and via sites of a [`MultiGraph`] (note §4).
pub struct Negotiator {
    /// Per-edge history cost, `[layer_idx][edge]`.
    pub history: Vec<Vec<f64>>,
    /// Per-site history cost.
    pub site_history: Vec<f64>,
    /// Per-connection failure bumps (index like `conns`).
    pub bumps: Vec<u32>,
    pub iteration: u32,
    pending_rip: BTreeSet<usize>,
    /// Connections blocked by a static fact of the board (F3: a terminal without an inside
    /// face that has a statically passable edge or a via site; every dynamic quantity -
    /// capacity, strands, owned sites, `along`, history - is excluded), with the reason: blocked
    /// in every later iteration and not searched again.
    dead: BTreeMap<usize, String>,
    /// Rips per `(ripper, blocker)` pair so far: at most [`RIP_PAIR_CAP`] per run (damping,
    /// design note §7 amendment).
    rip_pairs: BTreeMap<(usize, usize), u32>,
    cur: Vec<Option<SlotPath>>,
    /// Tunables (history increments, rip cap); the M3 constants until [`Negotiator::configure`].
    cfg: NegCfg,
}

/// Negotiation tunables taken from `Options` (`expert.*`, `ripup_max_per_pair`).
#[derive(Clone, Copy, Debug)]
struct NegCfg {
    hist_fail: f64,
    hist_block: f64,
    hist_overuse: f64,
    rip_pair_cap: u32,
}

impl Default for NegCfg {
    fn default() -> Self {
        NegCfg {
            hist_fail: HIST_FAIL,
            hist_block: HIST_BLOCK,
            hist_overuse: HIST_OVERUSE,
            rip_pair_cap: RIP_PAIR_CAP,
        }
    }
}

fn dedup_edges(mut v: Vec<usize>) -> Vec<usize> {
    v.sort_unstable();
    v.dedup();
    v
}

impl Negotiator {
    /// Empty history for the layers and sites of `mg`, `conns` connections. O(Σ_L E_L + S).
    pub fn new(mg: &MultiGraph, conns: usize) -> Negotiator {
        Negotiator {
            history: mg
                .layers
                .iter()
                .map(|c| vec![0.0; c.g.edges.len()])
                .collect(),
            site_history: vec![0.0; mg.sites.len()],
            bumps: vec![0; conns],
            iteration: 0,
            pending_rip: BTreeSet::new(),
            dead: BTreeMap::new(),
            rip_pairs: BTreeMap::new(),
            cur: vec![None; conns],
            cfg: NegCfg::default(),
        }
    }

    /// Take the tunables from `opts` (history increments, rip-up cap per pair). O(1).
    pub fn configure(&mut self, opts: &Options) {
        self.cfg = NegCfg {
            hist_fail: opts.expert.hist_fail,
            hist_block: opts.expert.hist_block,
            hist_overuse: opts.expert.hist_overuse,
            rip_pair_cap: opts.ripup_max_per_pair,
        };
    }

    /// The connection is blocked by static facts alone and is not searched again.
    pub fn is_dead(&self, conn: usize) -> bool {
        self.dead.contains_key(&conn)
    }

    /// One PathFinder iteration over the `active` connections (indices into `conns`; the others
    /// keep their paths in `mg`). `starts[i]` are the `(layer_idx, vertex)` start vertices and
    /// `goals[i]` the [`Goal`] of `conns[i]`; `conns[i].id` must equal `i`.
    ///
    /// Complexity: O(|active| · V' log V') plus O(Σ_L E_L + S) for the overuse sweep.
    #[allow(clippy::too_many_arguments)]
    pub fn iteration(
        &mut self,
        mg: &mut MultiGraph,
        obs: &mut ObstacleSet,
        conns: &[Connection],
        active: &[usize],
        starts: &[Vec<(usize, usize)>],
        goals: &[Goal],
        opts: &Options,
        deadline: Instant,
    ) -> IterOutcome {
        self.iteration += 1;
        let mut is_active = vec![false; conns.len()];
        for &a in active {
            is_active[a] = true;
        }
        let cap = default_expansion_cap(mg);
        let mut blocked = BTreeMap::new();
        let mut timed_out = false;
        // Rip-up of the previous iteration's blockers: they leave the graph (their sites are
        // released) before anything is searched, so the connection they blocked (bumped, hence
        // first) can pass.
        for r in std::mem::take(&mut self.pending_rip) {
            if is_active[r] {
                mg.remove_conn(conns[r].id, obs);
                self.cur[r] = None;
            }
        }
        let mut ripped: BTreeSet<usize> = BTreeSet::new();
        let mut resolved_sequentially = 0usize;
        let mut revalidated = 0usize;
        let seq: Vec<usize> = order(conns, &self.bumps)
            .into_iter()
            .filter(|&i| is_active[i])
            .collect();
        // (1) Snapshot: every active connection leaves the graph (its sites are released).
        if crate::control::expired(deadline) {
            timed_out = true;
        } else {
            for &i in &seq {
                mg.remove_conn(conns[i].id, obs);
                self.cur[i] = None;
            }
        }
        let dead = self.dead.clone();
        // (2) Every search reads only the frozen graph: results are independent of the pool size.
        let todo: Vec<usize> = if timed_out { Vec::new() } else { seq.clone() };
        let t_search = Instant::now();
        let results = self.search_all(mg, obs, conns, &todo, starts, goals, opts, deadline, &dead);
        let search_s = t_search.elapsed().as_secs_f64();
        let t_commit = Instant::now();
        // (3) Commit in order; a path invalidated by an earlier commit is searched again,
        // sequentially, against the graph as it then stands (the M2 order is kept).
        let t0_layers: Vec<u64> = mg.layers.iter().map(|c| c.g.clock).collect();
        let t0_sites = mg.clock();
        let mut activated: Vec<(Vec2, f64)> = Vec::new();
        for (&i, (mut res, mut pre)) in todo.iter().zip(results) {
            let c = &conns[i];
            if let SearchOutcome::Found(p) = &res {
                let v = Self::path_valid(mg, c, p, &t0_layers, t0_sites, &activated);
                if v != Validity::Ok {
                    // Faces changed: the O(k) re-validation (F4); a via near the path changed
                    // the geometry: search again.
                    let fast = if v == Validity::Faces {
                        revalidate_path(mg, c, p)
                    } else {
                        None
                    };
                    match fast {
                        Some(q) => {
                            revalidated += 1;
                            res = SearchOutcome::Found(q);
                        }
                        None => {
                            // Past the deadline no search runs: the path is dropped, the
                            // remaining found paths are still committed (anytime).
                            if crate::control::expired(deadline) {
                                timed_out = true;
                                continue;
                            }
                            resolved_sequentially += 1;
                            let params = params_for(
                                c,
                                &self.history,
                                &self.site_history,
                                cap,
                                deadline,
                                opts,
                            );
                            res = search(mg, obs, c, &starts[i], &goals[i], &params);
                            pre = None;
                        }
                    }
                }
            }
            match res {
                SearchOutcome::Found(p) => {
                    // Absurd detour: rip the strands that block the ghost path.
                    if is_detour(c, &p, opts.detour_factor) {
                        // The snapshot ghost stands while its own faces are unchanged
                        // (commits only add occupancy, so no cheaper ghost can appear).
                        let usable = match &pre {
                            Some(Some((gp, ..))) => {
                                Self::path_valid(mg, c, gp, &t0_layers, t0_sites, &activated)
                                    == Validity::Ok
                            }
                            Some(None) => true,
                            None => false,
                        };
                        let gh = match pre {
                            Some(g) if usable => g,
                            _ => {
                                let params = params_for(
                                    c,
                                    &self.history,
                                    &self.site_history,
                                    cap,
                                    deadline,
                                    opts,
                                );
                                ghost(mg, obs, c, &starts[i], &goals[i], &params)
                            }
                        };
                        if let Some((gp, None, None, _)) = gh {
                            if p.est_len > opts.detour_factor * gp.est_len {
                                let b = mg.conns_of_legs(&strand_blockers(mg, &gp));
                                if self.rip(mg, i, &b, true) {
                                    ripped.insert(i);
                                    ripped.extend(b.iter().copied().filter(|&x| x != i));
                                }
                            }
                        }
                    }
                    mg.insert_path(c, &p, obs);
                    activated.extend(
                        p.sites
                            .iter()
                            .map(|&(s, z)| (mg.sites[s].at, 0.5 * z.d as f64 + c.clearance as f64)),
                    );
                    self.cur[i] = Some(p);
                }
                SearchOutcome::Blocked {
                    ghost,
                    blocked_edge,
                    blocked_site,
                    last_reached,
                    static_reason,
                } => {
                    if let Some(r) = &static_reason {
                        self.dead.insert(i, r.clone());
                    }
                    if let (Some(gp), None) = (&ghost, blocked_edge) {
                        let b = site_aware_blockers(mg, gp);
                        if self.rip(mg, i, &b, false) {
                            ripped.extend(b.iter().copied().filter(|&x| x != i));
                        }
                    }
                    blocked.insert(
                        i,
                        BlockInfo {
                            edge: blocked_edge,
                            site: blocked_site,
                            ghost,
                            last_reached,
                            static_reason,
                        },
                    );
                }
                SearchOutcome::Timeout => {
                    // Anytime (ruling Q2): the searches that finished before the deadline are
                    // committed in order all the same; this one stays unrouted.
                    timed_out = true;
                }
            }
        }
        let overuse_edges = mg
            .layers
            .iter()
            .map(|c| {
                (0..c.g.edges.len())
                    .filter(|&e| c.g.overuse(e) > 0.0)
                    .count()
            })
            .sum();
        self.pending_rip = ripped.clone();
        let commit_s = t_commit.elapsed().as_secs_f64();
        IterOutcome {
            search_s,
            commit_s,
            searched: todo.len(),
            paths: self.cur.clone(),
            blocked,
            overuse_edges,
            timed_out,
            ripped: ripped.into_iter().collect(),
            resolved_sequentially,
            revalidated,
        }
    }

    /// Search `idx` (indices into `conns`) in parallel against the frozen `mg`; results in `idx`
    /// order. Dead connections are not searched (see `dead`). O(|idx| · V' log V' / threads).
    #[allow(clippy::too_many_arguments)]
    fn search_all(
        &self,
        mg: &MultiGraph,
        obs: &ObstacleSet,
        conns: &[Connection],
        idx: &[usize],
        starts: &[Vec<(usize, usize)>],
        goals: &[Goal],
        opts: &Options,
        deadline: Instant,
        dead: &BTreeMap<usize, String>,
    ) -> Vec<(SearchOutcome, Option<GhostOut>)> {
        let cap = default_expansion_cap(mg);
        idx.par_iter()
            .map(|&i| {
                if let Some(r) = dead.get(&i) {
                    let none = SearchOutcome::Blocked {
                        ghost: None,
                        blocked_edge: None,
                        blocked_site: None,
                        last_reached: None,
                        static_reason: Some(r.clone()),
                    };
                    return (none, None);
                }
                let c = &conns[i];
                let p = params_for(c, &self.history, &self.site_history, cap, deadline, opts);
                let r = search(mg, obs, c, &starts[i], &goals[i], &p);
                // The absurd-detour test of the commit needs the ghost: compute it here, in
                // parallel, and let the commit reuse it while its faces are unchanged.
                let g = match &r {
                    SearchOutcome::Found(path) if is_detour(c, path, opts.detour_factor) => {
                        Some(ghost(mg, obs, c, &starts[i], &goals[i], &p))
                    }
                    _ => None,
                };
                (r, g)
            })
            .collect()
    }

    /// Batch validity of a snapshot path `p` of `c` (note §7, P4): `Sites` when a site of the
    /// path changed since the freeze (`t0_sites`) or a via activated since the freeze
    /// (`activated`, `(centre, d/2 + clr)`) lies within that via's own `d/2 + clr + w/2 + eps`
    /// of the bounding box of some leg's faces (F4: per leg and per via, not `rho_max` of the
    /// layer); otherwise `Faces` when an edge of a face a leg touches changed (`t0_layers`);
    /// otherwise `Ok`. O(Σ|faces| · (1 + |activated|) + |p.sites|).
    fn path_valid(
        mg: &MultiGraph,
        c: &Connection,
        p: &SlotPath,
        t0_layers: &[u64],
        t0_sites: u64,
        activated: &[(Vec2, f64)],
    ) -> Validity {
        if p.sites.iter().any(|&(s, _)| mg.site_version[s] > t0_sites) {
            return Validity::Sites;
        }
        let mut faces_changed = false;
        let eps = mg.layers.first().map_or(0.0, |l| l.g.rules.eps);
        let grow = 0.5 * c.width as f64 + eps;
        for leg in &p.legs {
            let g = &mg.layers[leg.layer_idx].g;
            let faces = g.leg_faces(leg.start, leg.goal, &leg.faces);
            // A clock that has not moved cannot have changed any edge.
            if g.clock > t0_layers[leg.layer_idx]
                && !g.faces_unchanged_since(&faces, t0_layers[leg.layer_idx])
            {
                faces_changed = true;
            }
            if activated.is_empty() {
                continue;
            }
            let mut bb: Option<Aabb> = None;
            for &f in &faces {
                for &v in &g.faces[f].v {
                    let at = g.verts[v].at;
                    bb = Some(match bb {
                        None => Aabb { min: at, max: at },
                        Some(b) => Aabb {
                            min: Vec2 {
                                x: b.min.x.min(at.x),
                                y: b.min.y.min(at.y),
                            },
                            max: Vec2 {
                                x: b.max.x.max(at.x),
                                y: b.max.y.max(at.y),
                            },
                        },
                    });
                }
            }
            let Some(bb) = bb else { continue };
            if activated.iter().any(|&(a, r)| {
                let b = bb.grow(r + grow);
                a.x >= b.min.x && a.x <= b.max.x && a.y >= b.min.y && a.y <= b.max.y
            }) {
                return Validity::Sites;
            }
        }
        if faces_changed {
            Validity::Faces
        } else {
            Validity::Ok
        }
    }

    /// Topological rip-up bookkeeping for connection `i` whose ghost path is blocked by the
    /// strands / vias of the connections `blockers`: history on the blockers' leg edges and
    /// sites, a bump for `i` when `bump` (a blocked connection is bumped by `penalise_blocked`).
    /// `false` (nothing done) when there is no blocker, more than [`MAX_RIP`], or one that `i`
    /// has already ripped [`RIP_PAIR_CAP`] times in this run (damping: without a bound two
    /// connections sharing one class rip each other every iteration until the stall stop,
    /// scaling diagnosis RC-A2; the history they leave behind decides the class for whoever is
    /// searched next).
    fn rip(&mut self, mg: &MultiGraph, i: usize, blockers: &[usize], bump: bool) -> bool {
        let b: Vec<usize> = blockers.iter().copied().filter(|&x| x != i).collect();
        if b.is_empty()
            || b.len() > MAX_RIP
            || b
                .iter()
                .any(|&x| self.rip_pairs.get(&(i, x)).is_some_and(|&n| n >= self.cfg.rip_pair_cap))
        {
            return false;
        }
        for &x in &b {
            *self.rip_pairs.entry((i, x)).or_insert(0) += 1;
            self.add_conn_history(mg, x, self.cfg.hist_block, self.cfg.hist_block);
        }
        if bump {
            self.bumps[i] += 1;
        }
        true
    }

    /// `edge` history on every edge of `conn`'s legs (per layer) and `site` history on its
    /// sites. O(path length · layers).
    fn add_conn_history(&mut self, mg: &MultiGraph, conn: usize, edge: f64, site: f64) {
        for &k in mg.conn_wires.get(conn).map_or(&[][..], |v| v.as_slice()) {
            for (li, c) in mg.layers.iter().enumerate() {
                for e in dedup_edges(c.g.occupancy(k).iter().map(|&(e, _)| e).collect()) {
                    self.history[li][e] += edge;
                }
            }
        }
        for &s in mg.sites_of(conn) {
            self.site_history[s] += site;
        }
    }

    /// Relax / self-check failure: history on the connection's edges, bump. The path stays in `mg`.
    pub fn penalise_failed(&mut self, mg: &MultiGraph, conn: usize) {
        self.add_conn_history(mg, conn, self.cfg.hist_fail, 0.0);
        self.bumps[conn] += 1;
    }

    /// Blocked connection: history on the ghost path's edges and sites (none without a
    /// ghost), bump.
    pub fn penalise_blocked(&mut self, conn: usize, b: &BlockInfo) {
        if let Some(gp) = &b.ghost {
            for leg in &gp.legs {
                for e in dedup_edges(leg.crossings.iter().map(|&(e, _)| e).collect()) {
                    self.history[leg.layer_idx][e] += self.cfg.hist_block;
                }
            }
            for &(s, _) in &gp.sites {
                self.site_history[s] += self.cfg.hist_block;
            }
        }
        self.bumps[conn] += 1;
    }

    /// History on every overused edge (every layer); returns their count.
    pub fn penalise_overuse(&mut self, mg: &MultiGraph) -> usize {
        let mut n = 0;
        for (li, c) in mg.layers.iter().enumerate() {
            for e in 0..c.g.edges.len() {
                if c.g.overuse(e) > 0.0 {
                    self.history[li][e] += self.cfg.hist_overuse;
                    n += 1;
                }
            }
        }
        n
    }
}

/// Outcome of [`Negotiator::path_valid`] (note §7, F4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Validity {
    Ok,
    /// An edge of a touched face changed: topology only, the O(k) re-validation applies.
    Faces,
    /// A site of the path or a via near it changed: the geometry moved, search again.
    Sites,
}

/// Ghost path with its first blocked edge / site and the crossing before it (see `search::ghost`).
type GhostOut = Option<(
    SlotPath,
    Option<(usize, usize)>,
    Option<usize>,
    Option<(usize, usize)>,
)>;

/// Absurd-detour precondition (fix B): `p` is longer than [`DETOUR_FACTOR`] times the straight
/// distance and crosses at least one strand. O(legs).
fn is_detour(c: &Connection, p: &SlotPath, factor: f64) -> bool {
    let straight = (c.from.at - c.to.at).len();
    p.est_len > factor * straight && p.legs.iter().any(|l| !l.crossings.is_empty())
}

/// Search parameters of connection `c` over the given histories.
fn params_for<'a>(
    c: &Connection,
    history: &'a [Vec<f64>],
    site_history: &'a [f64],
    cap: usize,
    deadline: Instant,
    opts: &Options,
) -> SearchParams<'a> {
    SearchParams {
        history,
        site_history,
        weight: tier_weight(c.tier),
        present_overuse: opts.expert.present_overuse,
        expansion_cap: cap,
        deadline,
        eps: None,
        via_cost: opts.via_cost_for(c.net) as f64 * via_weight(c.tier),
        max_vias: opts.max_vias_per_connection,
    }
}

/// Connections (sorted, deduplicated) that block the slot-free ghost `gp`: the owners of the
/// strands it cuts, of every owned site it uses, and of the active neighbours that block a
/// site it uses. O(strands + ghost sites · neighbours).
fn site_aware_blockers(mg: &MultiGraph, gp: &SlotPath) -> Vec<usize> {
    let mut out = mg.conns_of_legs(&strand_blockers(mg, gp));
    for &(s, _) in &gp.sites {
        if let Some((c, _)) = mg.owner[s] {
            out.push(c);
        }
        for &u in mg.blocked_by(s) {
            if let Some((c, _)) = mg.owner[u] {
                out.push(c);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Connections (sorted) with a crossing on an overused edge: part of the next active set.
/// O(Σ_L E_L).
pub fn overused_conns(mg: &MultiGraph) -> Vec<usize> {
    let mut legs = BTreeSet::new();
    for c in &mg.layers {
        for (e, ge) in c.g.edges.iter().enumerate() {
            if c.g.overuse(e) > 0.0 {
                legs.extend(ge.cross.iter().map(|x| x.conn));
            }
        }
    }
    let legs: Vec<usize> = legs.into_iter().collect();
    mg.conns_of_legs(&legs)
}

/// Diagnostic clinch detection (note §5), per layer, summed. `conns[i].id` must equal `i`.
/// O(Σ_L E_L · n_max + legs).
pub fn detect_clinches(
    mg: &MultiGraph,
    conns: &[Connection],
    paths: &[Option<SlotPath>],
    _starts: &[Vec<(usize, usize)>],
) -> ClinchReport {
    let mut rep = ClinchReport::default();
    let owner_net =
        |leg: usize| -> Option<i32> { mg.conns_of_legs(&[leg]).first().map(|&o| conns[o].net) };
    // Regular: foreign entry between a terminal and the connection's own crossing on an
    // edge incident to that terminal (the first leg's start, the last leg's goal when it is a
    // terminal rather than a via).
    for (i, c) in conns.iter().enumerate() {
        let Some(Some(path)) = paths.get(i) else {
            continue;
        };
        let ks = mg.conn_wires.get(c.id).map_or(&[][..], |v| v.as_slice());
        for (j, leg) in path.legs.iter().enumerate() {
            let Some(&k) = ks.get(j) else { continue };
            let g = &mg.layers[leg.layer_idx].g;
            let Some((cross, _, _)) = g.path_of(k) else {
                continue;
            };
            let mut ends: Vec<usize> = Vec::new();
            if j == 0 {
                ends.push(leg.start);
            }
            if j + 1 == path.legs.len() && path.sites.len() < path.legs.len() {
                ends.push(leg.goal);
            }
            for &(e, slot) in cross {
                let ge = &g.edges[e];
                for &p in &ends {
                    let range = if ge.a == p {
                        0..slot
                    } else if ge.b == p {
                        slot + 1..ge.cross.len()
                    } else {
                        continue;
                    };
                    if ge.cross[range]
                        .iter()
                        .any(|x| owner_net(x.conn).is_some_and(|n| n != c.net))
                    {
                        rep.regular.push((c.id, e));
                        break;
                    }
                }
            }
        }
    }
    // U-clinch: a net's own terminal pair is crossed by a foreign wire while that net's
    // single-layer connection had to leave the edge.
    for (li, lc) in mg.layers.iter().enumerate() {
        let g = &lc.g;
        let mut by_pair: BTreeMap<(usize, usize), Vec<i32>> = BTreeMap::new();
        for (i, c) in conns.iter().enumerate() {
            let Some(Some(p)) = paths.get(i) else {
                continue;
            };
            if let [leg] = p.legs.as_slice() {
                if leg.layer_idx == li && !leg.crossings.is_empty() && p.sites.is_empty() {
                    let (a, b) = (leg.start, leg.goal);
                    by_pair.entry((a.min(b), a.max(b))).or_default().push(c.net);
                }
            }
        }
        for (e, ge) in g.edges.iter().enumerate() {
            if ge.constraint || ge.cross.is_empty() {
                continue;
            }
            let Some(nets) = by_pair.get(&(ge.a, ge.b)) else {
                continue;
            };
            let (na, nb) = (&g.verts[ge.a].nets, &g.verts[ge.b].nets);
            for &n in nets {
                if n == 0 || !na.contains(&n) || !nb.contains(&n) {
                    continue;
                }
                for x in ge.cross.iter() {
                    if let Some(&o) = mg.conns_of_legs(&[x.conn]).first() {
                        if conns[o].net != n {
                            rep.u.push((o, e));
                        }
                    }
                }
            }
        }
    }
    rep.regular.sort_unstable();
    rep.u.sort_unstable();
    rep.u.dedup();
    rep
}
