//! PathFinder negotiation (M2 / T2.3b; design note `m2-search-design.md` §4-5): history costs,
//! failure bumps, best-iteration bookkeeping and clinch detection.
//!
//! One `iteration` re-routes the active connections in `decompose::order` (bumps are a bounded
//! priority adjustment, never a permutation). Task 8 metrizes / self-checks the result and feeds
//! back through `penalise_failed`, `penalise_blocked` and `penalise_overuse`.
//! Complexity per iteration: O(m · V log V) searches plus O(E) for the overuse sweep.
//! No `HashMap` iteration reaches any output; the wall clock is checked before every search.

use crate::decompose::{order, Connection};
use crate::graph::RouteGraph;
use crate::model::Options;
use crate::obstacle::LayerObstacles;
use crate::search::{
    default_expansion_cap, ghost, search, strand_blockers, tier_weight, SearchOutcome,
    SearchParams, SlotPath,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

pub const HIST_FAIL: f64 = 1.0;
pub const HIST_BLOCK: f64 = 0.5;
pub const HIST_OVERUSE: f64 = 0.5;
pub const PRESENT_OVERUSE: f64 = crate::search::PRESENT_OVERUSE;
/// A found path longer than this factor times its ghost path counts as an absurd detour (fix B).
pub const DETOUR_FACTOR: f64 = 2.0;
/// Most blockers one connection rips per iteration (keeps the churn bounded).
pub const MAX_RIP: usize = 3;

/// Clinches found in a routing: `(conn, edge)` pairs.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ClinchReport {
    pub regular: Vec<(usize, usize)>,
    pub u: Vec<(usize, usize)>,
}

/// Why a connection found no path (ghost search result).
#[derive(Clone, Debug)]
pub struct BlockInfo {
    pub edge: Option<usize>,
    pub ghost: Option<SlotPath>,
    pub last_reached: Option<(usize, usize)>,
}

#[derive(Clone, Debug)]
pub struct IterOutcome {
    /// Indexed like `conns`; `None` for blocked / never routed connections.
    pub paths: Vec<Option<SlotPath>>,
    pub blocked: BTreeMap<usize, BlockInfo>,
    pub overuse_edges: usize,
    pub timed_out: bool,
    /// Connections ripped up this iteration because their strands block another connection's
    /// ghost path (fix B); they belong to the next active set.
    pub ripped: Vec<usize>,
}

type Best = (usize, f64, Vec<Option<SlotPath>>);

pub struct Negotiator {
    /// Per-edge history cost.
    pub history: Vec<f64>,
    /// Per-connection failure bumps (index like `conns`).
    pub bumps: Vec<u32>,
    pub iteration: u32,
    pending_rip: BTreeSet<usize>,
    cur: Vec<Option<SlotPath>>,
    best: Option<Best>,
}

fn dedup_edges(mut v: Vec<usize>) -> Vec<usize> {
    v.sort_unstable();
    v.dedup();
    v
}

impl Negotiator {
    pub fn new(edges: usize, conns: usize) -> Negotiator {
        Negotiator {
            history: vec![0.0; edges],
            bumps: vec![0; conns],
            iteration: 0,
            pending_rip: BTreeSet::new(),
            cur: vec![None; conns],
            best: None,
        }
    }

    /// One PathFinder iteration over the `active` connections (indices into `conns`; the others
    /// keep their paths in `g`). `verts[i]` is `(start, goal)` graph vertex of `conns[i]`;
    /// `conns[i].id` must equal `i`.
    #[allow(clippy::too_many_arguments)]
    pub fn iteration(
        &mut self,
        g: &mut RouteGraph,
        obs: &LayerObstacles,
        conns: &[Connection],
        active: &[usize],
        verts: &[(usize, usize)],
        _opts: &Options,
        deadline: Instant,
    ) -> IterOutcome {
        self.iteration += 1;
        let mut is_active = vec![false; conns.len()];
        for &a in active {
            is_active[a] = true;
        }
        let cap = default_expansion_cap(g);
        let mut blocked = BTreeMap::new();
        let mut timed_out = false;
        // Rip-up of the previous iteration's blockers: they leave the graph before anything is
        // searched, so the connection they blocked (bumped, hence first) can pass.
        for r in std::mem::take(&mut self.pending_rip) {
            if is_active[r] {
                g.remove_conn(conns[r].id);
                self.cur[r] = None;
            }
        }
        let mut ripped: BTreeSet<usize> = BTreeSet::new();
        for i in order(conns, &self.bumps) {
            if !is_active[i] {
                continue;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                break;
            }
            let c = &conns[i];
            g.remove_conn(c.id);
            self.cur[i] = None;
            let (s, t) = verts[i];
            let params = SearchParams {
                history: &self.history,
                weight: tier_weight(c.tier),
                present_overuse: PRESENT_OVERUSE,
                expansion_cap: cap,
                deadline,
                eps: None,
            };
            match search(g, obs, c, s, t, &params) {
                SearchOutcome::Found(p) => {
                    // Absurd detour: rip the strands that block the ghost path.
                    let straight = (conns[i].from.at - conns[i].to.at).len();
                    if p.est_len > DETOUR_FACTOR * straight && !p.crossings.is_empty() {
                        if let Some((gp, None, _)) = ghost(g, obs, c, s, t, &params) {
                            if p.est_len > DETOUR_FACTOR * gp.est_len {
                                let b = strand_blockers(g, &gp, s, t);
                                if self.rip(g, i, &b, true) {
                                    ripped.insert(i);
                                    ripped.extend(b.iter().copied().filter(|&x| x != i));
                                }
                            }
                        }
                    }
                    g.insert_path(
                        c.id,
                        s,
                        t,
                        &p.crossings,
                        &p.faces,
                        c.width as f64,
                        c.clearance as f64,
                    );
                    self.cur[i] = Some(p);
                }
                SearchOutcome::Blocked {
                    ghost,
                    blocked_edge,
                    last_reached,
                } => {
                    if let (Some(gp), None) = (&ghost, blocked_edge) {
                        let b = strand_blockers(g, gp, s, t);
                        if self.rip(g, i, &b, false) {
                            ripped.extend(b.iter().copied().filter(|&x| x != i));
                        }
                    }
                    blocked.insert(
                        i,
                        BlockInfo {
                            edge: blocked_edge,
                            ghost,
                            last_reached,
                        },
                    );
                }
                SearchOutcome::Timeout => {
                    timed_out = true;
                    break;
                }
            }
        }
        let overuse_edges = (0..g.edges.len()).filter(|&e| g.overuse(e) > 0.0).count();
        self.pending_rip = ripped.clone();
        IterOutcome {
            paths: self.cur.clone(),
            blocked,
            overuse_edges,
            timed_out,
            ripped: ripped.into_iter().collect(),
        }
    }

    /// Topological rip-up bookkeeping for connection `i` whose ghost path is blocked by the
    /// strands of `blockers`: history on the blockers' edges, a bump for `i` when `bump` (a blocked connection is bumped by `penalise_blocked`). `false` (nothing
    /// done) when there is no blocker or more than [`MAX_RIP`].
    fn rip(&mut self, g: &RouteGraph, i: usize, blockers: &[usize], bump: bool) -> bool {
        let b: Vec<usize> = blockers.iter().copied().filter(|&x| x != i).collect();
        if b.is_empty() || b.len() > MAX_RIP {
            return false;
        }
        for &x in &b {
            for e in dedup_edges(g.occupancy(x).iter().map(|&(e, _)| e).collect()) {
                self.history[e] += HIST_BLOCK;
            }
        }
        if bump {
            self.bumps[i] += 1;
        }
        true
    }

    /// Relax / self-check failure: history on the connection's edges, bump. The path stays in `g`.
    pub fn penalise_failed(&mut self, g: &RouteGraph, conn: usize) {
        let edges = dedup_edges(g.occupancy(conn).iter().map(|&(e, _)| e).collect());
        for e in edges {
            self.history[e] += HIST_FAIL;
        }
        self.bumps[conn] += 1;
    }

    /// Blocked connection: history on the ghost path's edges (none without a ghost), bump.
    pub fn penalise_blocked(&mut self, conn: usize, b: &BlockInfo) {
        if let Some(gp) = &b.ghost {
            for e in dedup_edges(gp.crossings.iter().map(|&(e, _)| e).collect()) {
                self.history[e] += HIST_BLOCK;
            }
        }
        self.bumps[conn] += 1;
    }

    /// History on every overused edge; returns their count.
    pub fn penalise_overuse(&mut self, g: &RouteGraph) -> usize {
        let mut n = 0;
        for e in 0..g.edges.len() {
            if g.overuse(e) > 0.0 {
                self.history[e] += HIST_OVERUSE;
                n += 1;
            }
        }
        n
    }

    /// Keep the best iteration: more routed connections, then shorter total length.
    pub fn record_best(&mut self, paths: &[Option<SlotPath>], routed: usize, len: f64) {
        let better = match &self.best {
            None => true,
            Some((r, l, _)) => routed > *r || (routed == *r && len < *l),
        };
        if better {
            self.best = Some((routed, len, paths.to_vec()));
        }
    }

    pub fn best(&self) -> Option<&Vec<Option<SlotPath>>> {
        self.best.as_ref().map(|b| &b.2)
    }
}

/// Connections (sorted) with a crossing on an overused edge: part of the next active set.
pub fn overused_conns(g: &RouteGraph) -> Vec<usize> {
    let mut s = BTreeSet::new();
    for (e, ge) in g.edges.iter().enumerate() {
        if g.overuse(e) > 0.0 {
            s.extend(ge.cross.iter().map(|x| x.conn));
        }
    }
    s.into_iter().collect()
}

/// Diagnostic clinch detection (note §5). `verts[i]` is `(start, goal)` of `conns[i]`.
pub fn detect_clinches(
    g: &RouteGraph,
    conns: &[Connection],
    paths: &[Option<SlotPath>],
    verts: &[(usize, usize)],
) -> ClinchReport {
    let mut rep = ClinchReport::default();
    // Regular: foreign entry between a terminal and the connection's own crossing on an
    // edge incident to that terminal.
    for (i, c) in conns.iter().enumerate() {
        if paths.get(i).is_none_or(|p| p.is_none()) {
            continue;
        }
        let Some((cross, _, _)) = g.path_of(c.id) else {
            continue;
        };
        for &(e, slot) in cross {
            let ge = &g.edges[e];
            for p in [verts[i].0, verts[i].1] {
                let range = if ge.a == p {
                    0..slot
                } else if ge.b == p {
                    slot + 1..ge.cross.len()
                } else {
                    continue;
                };
                if ge.cross[range].iter().any(|x| conns[x.conn].net != c.net) {
                    rep.regular.push((c.id, e));
                    break;
                }
            }
        }
    }
    // U-clinch: a net's own terminal pair is crossed by a foreign wire while that net's
    // connection had to leave the edge.
    let mut by_pair: BTreeMap<(usize, usize), Vec<i32>> = BTreeMap::new();
    for (i, c) in conns.iter().enumerate() {
        let (a, b) = verts[i];
        let detoured = paths
            .get(i)
            .and_then(|p| p.as_ref())
            .is_some_and(|p| !p.crossings.is_empty());
        if detoured {
            by_pair.entry((a.min(b), a.max(b))).or_default().push(c.net);
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
            for x in ge.cross.iter().filter(|x| conns[x.conn].net != n) {
                rep.u.push((x.conn, e));
            }
        }
    }
    rep.regular.sort_unstable();
    rep.u.sort_unstable();
    rep.u.dedup();
    rep
}
