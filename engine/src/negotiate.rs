//! PathFinder negotiation over layers and via sites (M2 / T2.3b, M3 / T3.1; design notes
//! `m2-search-design.md` §4-5, `m3-layers-vias-design.md` §4): history costs, failure bumps,
//! site history, best-iteration bookkeeping and clinch detection.
//!
//! One `iteration` re-routes the active connections in `decompose::order` (bumps are a bounded
//! priority adjustment, never a permutation). `route` metrizes / self-checks the result and feeds
//! back through `penalise_failed`, `penalise_blocked` and `penalise_overuse`.
//! Complexity per iteration: O(m · V' log V') searches (V' = states of all layers plus sites)
//! plus O(Σ_L E_L + S) for the overuse sweep.
//! No `HashMap` iteration reaches any output; the wall clock is checked before every search.

use crate::decompose::{order, Connection};
use crate::model::Options;
use crate::multigraph::MultiGraph;
use crate::obstacle::ObstacleSet;
use crate::search::{
    default_expansion_cap, ghost, search, strand_blockers, tier_weight, via_weight, Goal,
    SearchOutcome, SearchParams, SlotPath,
};
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
    cur: Vec<Option<SlotPath>>,
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
            cur: vec![None; conns],
        }
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
        for i in order(conns, &self.bumps) {
            if !is_active[i] {
                continue;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                break;
            }
            let c = &conns[i];
            mg.remove_conn(c.id, obs);
            self.cur[i] = None;
            let params = SearchParams {
                history: &self.history,
                site_history: &self.site_history,
                weight: tier_weight(c.tier),
                present_overuse: PRESENT_OVERUSE,
                expansion_cap: cap,
                deadline,
                eps: None,
                via_cost: opts.via_cost_nm as f64 * via_weight(c.tier),
                max_vias: opts.max_vias_per_connection,
            };
            match search(mg, obs, c, &starts[i], &goals[i], &params) {
                SearchOutcome::Found(p) => {
                    // Absurd detour: rip the strands that block the ghost path.
                    let straight = (c.from.at - c.to.at).len();
                    let crosses = p.legs.iter().any(|l| !l.crossings.is_empty());
                    if p.est_len > DETOUR_FACTOR * straight && crosses {
                        if let Some((gp, None, None, _)) =
                            ghost(mg, obs, c, &starts[i], &goals[i], &params)
                        {
                            if p.est_len > DETOUR_FACTOR * gp.est_len {
                                let b = mg.conns_of_legs(&strand_blockers(mg, &gp));
                                if self.rip(mg, i, &b, true) {
                                    ripped.insert(i);
                                    ripped.extend(b.iter().copied().filter(|&x| x != i));
                                }
                            }
                        }
                    }
                    mg.insert_path(c, &p, obs);
                    self.cur[i] = Some(p);
                }
                SearchOutcome::Blocked {
                    ghost,
                    blocked_edge,
                    blocked_site,
                    last_reached,
                } => {
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
                        },
                    );
                }
                SearchOutcome::Timeout => {
                    timed_out = true;
                    break;
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
        IterOutcome {
            paths: self.cur.clone(),
            blocked,
            overuse_edges,
            timed_out,
            ripped: ripped.into_iter().collect(),
        }
    }

    /// Topological rip-up bookkeeping for connection `i` whose ghost path is blocked by the
    /// strands / vias of the connections `blockers`: history on the blockers' leg edges and
    /// sites, a bump for `i` when `bump` (a blocked connection is bumped by `penalise_blocked`).
    /// `false` (nothing done) when there is no blocker or more than [`MAX_RIP`].
    fn rip(&mut self, mg: &MultiGraph, i: usize, blockers: &[usize], bump: bool) -> bool {
        let b: Vec<usize> = blockers.iter().copied().filter(|&x| x != i).collect();
        if b.is_empty() || b.len() > MAX_RIP {
            return false;
        }
        for &x in &b {
            self.add_conn_history(mg, x, HIST_BLOCK, HIST_SITE);
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
        self.add_conn_history(mg, conn, HIST_FAIL, 0.0);
        self.bumps[conn] += 1;
    }

    /// Blocked connection: history on the ghost path's edges and sites (none without a
    /// ghost), bump.
    pub fn penalise_blocked(&mut self, conn: usize, b: &BlockInfo) {
        if let Some(gp) = &b.ghost {
            for leg in &gp.legs {
                for e in dedup_edges(leg.crossings.iter().map(|&(e, _)| e).collect()) {
                    self.history[leg.layer_idx][e] += HIST_BLOCK;
                }
            }
            for &(s, _) in &gp.sites {
                self.site_history[s] += HIST_SITE;
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
                    self.history[li][e] += HIST_OVERUSE;
                    n += 1;
                }
            }
        }
        n
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
