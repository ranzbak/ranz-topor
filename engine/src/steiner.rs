//! Same-net Steiner T-junctions and net ordering (M4a task A5, M4-D3; design note
//! `docs/progress/notes/m4-capacity-steiner-design.md` §2).
//!
//! A connection `B = (P, Q)` of a net may end on the copper of an already committed connection
//! `A` of the same net (a *sibling*) in a T-junction instead of at its pad, when `A`'s copper
//! lies in the cluster component on `B`'s far side: with the net's other connections as edges
//! (a connection attached to a sibling joins its real terminal to the sibling's), `A` must be
//! connected to `Q` and not to `P`, so the T keeps the net a spanning tree. A sibling connected
//! to `P` instead serves a *reversed* search (from `Q` toward junctions on `P`'s side). Kept
//! hand copper of the target cluster (`existing_copper = fixed`, the pour-completion passes)
//! is a goal the same way: any point of its tracks' constraint edges.
//!
//! The goal set of one search is every crossing of every committed leg of the allowed siblings
//! (`O(Σ sibling leg length)`), minus the crossings on edges that touch the siblings' own
//! terminals or `B`'s far terminal (a T right at a pad is the pad goal with extra steps), minus
//! the pairs the metrization rejected (`MultiGraph::steiner_reject`). The search tests a state
//! against it in `O(1)` (hash lookup by edge).
//!
//! Net ordering: `Decomposition::net_rank` (rank inside the net by `est_len`) equals the
//! shorter-first rule of `decompose::order` within a net, so the routing order is unchanged:
//! short spokes route first and the longer connections of the net may attach to them.
//!
//! Complexity: [`plan`] `O(p α(p) + Σ sibling leg length)` per search for a net with `p`
//! connections; [`lower_bound`] the same; no loop is unbounded (every loop runs over the net's
//! connections or a sibling's crossings).

use crate::decompose::Connection;
use crate::geom::{point_seg, Seg, Vec2};
use crate::multigraph::MultiGraph;
use crate::search::{Goal, OwnStrand};
use std::collections::BTreeMap;

/// Per live connection: what the junction rule needs to know about it.
#[derive(Clone, Debug)]
pub struct SConn {
    pub net: i32,
    /// Run-unique copper clusters of the two ends (`Decomposition::conn_clusters`).
    pub from_cl: usize,
    pub to_cl: usize,
    /// The target is a pour (no reversed search: a pour is reached by a via site).
    pub pour: bool,
    /// Graph vertex owner labels of the from / to terminals (terminal label and anchor label).
    pub owners: [Vec<String>; 2],
}

/// Static Steiner context of one routing pass (built by `route_pass`).
#[derive(Clone, Debug, Default)]
pub struct SteinerNets {
    /// Least junction angle, degrees (`Options.expert.steiner_min_angle_deg`).
    pub min_angle_deg: f64,
    pub conns: Vec<SConn>,
    /// Net -> its live connections (sorted).
    pub by_net: BTreeMap<i32, Vec<usize>>,
    /// Cluster id -> constraint edges `(layer_idx, edge)` of its kept tracks (sorted).
    pub copper: BTreeMap<usize, Vec<(usize, usize)>>,
}

/// Junction goals of one search.
#[derive(Clone, Debug)]
pub struct Plan {
    /// Search from the `to` terminal toward junctions on the `from` side.
    pub reversed: bool,
    /// Start vertices of the search (the `to` terminal's when reversed).
    pub start: Vec<(usize, usize)>,
    /// The goal with `strands` / `copper` filled.
    pub goal: Goal,
}

struct Dsu(Vec<usize>);

impl Dsu {
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a.max(b)] = a.min(b);
        }
    }
}

/// The cluster of `x`'s real terminal end: `from`, or `to` for a reversed junction path.
fn anchor_cl(mg: &MultiGraph, sn: &SteinerNets, x: usize) -> usize {
    let c = &sn.conns[x];
    match mg.junction_of(x) {
        Some(j) if j.reversed => c.to_cl,
        _ => c.from_cl,
    }
}

/// Committed siblings of `conn` on its goal side and on its start side (sorted live ids),
/// from the union-find over the net's clusters with every other connection as an edge (an
/// attached one joining its terminal end to its sibling's). Both empty when `conn`'s ends are
/// already joined by the others. O(p α(p)).
fn sides(mg: &MultiGraph, sn: &SteinerNets, conn: usize) -> (Vec<usize>, Vec<usize>) {
    let me = &sn.conns[conn];
    let Some(list) = sn.by_net.get(&me.net) else {
        return (Vec::new(), Vec::new());
    };
    let mut ids: Vec<usize> = list
        .iter()
        .flat_map(|&x| [sn.conns[x].from_cl, sn.conns[x].to_cl])
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let ix = |c: usize| ids.binary_search(&c).unwrap_or(0);
    let mut dsu = Dsu((0..ids.len()).collect());
    for &x in list.iter().filter(|&&x| x != conn) {
        let c = &sn.conns[x];
        match mg.junction_of(x).map(|j| j.sibling) {
            Some(Some((s, _))) if s < sn.conns.len() => {
                dsu.union(ix(anchor_cl(mg, sn, x)), ix(anchor_cl(mg, sn, s)));
            }
            _ => dsu.union(ix(c.from_cl), ix(c.to_cl)),
        }
    }
    let (rg, rs) = (dsu.find(ix(me.to_cl)), dsu.find(ix(me.from_cl)));
    if rg == rs {
        return (Vec::new(), Vec::new());
    }
    let (mut g, mut s) = (Vec::new(), Vec::new());
    for &a in list {
        if a == conn
            || mg.conn_wires.get(a).is_none_or(|w| w.is_empty())
            || mg.steiner_reject.contains(&(conn, a))
        {
            continue;
        }
        let r = dsu.find(ix(anchor_cl(mg, sn, a)));
        if r == rg {
            g.push(a);
        } else if r == rs {
            s.push(a);
        }
    }
    (g, s)
}

/// Point of crossing `(e, slot)` of `g` at its slot fraction (as the metrization's polyline).
fn slot_point(g: &crate::graph::RouteGraph, e: usize, slot: usize) -> Vec2 {
    let ge = &g.edges[e];
    let (pa, pb) = (g.verts[ge.a].at, g.verts[ge.b].at);
    let f = (slot as f64 + 1.0) / (ge.cross.len() as f64 + 1.0);
    pa + (pb - pa) * f
}

/// Junction goal crossings of sibling `a` (every crossing of its committed legs except its own
/// junction half crossing and the edges touching a vertex owned by one of `excl`). O(legs).
fn strands_of(mg: &MultiGraph, sn: &SteinerNets, a: usize, far: &[String]) -> Vec<OwnStrand> {
    let mut out = Vec::new();
    let ks = mg.conn_wires.get(a).map_or(&[][..], |v| v.as_slice());
    let jend = mg.junction_of(a).is_some();
    let excl = |o: &Option<String>| {
        o.as_ref().is_some_and(|o| {
            far.iter().any(|f| f == o) || sn.conns[a].owners.iter().flatten().any(|f| f == o)
        })
    };
    for (ord, &k) in ks.iter().enumerate() {
        let Some(li) = mg.leg_layer(k) else { continue };
        let g = &mg.layers[li].g;
        let Some((cross, _, (sv, gv))) = g.path_of(k) else {
            continue;
        };
        let n = cross.len() - usize::from(jend && ord + 1 == ks.len() && !cross.is_empty());
        // Where the sibling's metrized chain crosses the edges (known after its first
        // metrization), else the slot fraction.
        let known = mg.chain_cross(k);
        let point = |q: usize| -> Vec2 {
            known
                .get(q)
                .copied()
                .flatten()
                .unwrap_or_else(|| slot_point(g, cross[q].0, cross[q].1))
        };
        for q in 0..n {
            let (e, slot) = cross[q];
            let ge = &g.edges[e];
            if excl(&g.verts[ge.a].owner) || excl(&g.verts[ge.b].owner) {
                continue;
            }
            // A metrized sibling whose chain does not cross this edge's segment (it passes
            // beyond a dormant site at the edge's end) has no copper here to end on.
            if !known.is_empty() && known.get(q).copied().flatten().is_none() {
                continue;
            }
            let prev = if q == 0 { g.verts[sv].at } else { point(q - 1) };
            let next = if q + 1 < cross.len() {
                point(q + 1)
            } else {
                g.verts[gv].at
            };
            let d = next - prev;
            let l = d.len();
            if l <= 0.0 {
                continue;
            }
            out.push(OwnStrand {
                conn: a,
                leg: ord,
                layer_idx: li,
                edge: e,
                slot,
                dir: d * (1.0 / l),
                at: point(q),
            });
        }
    }
    out
}

/// The junction goals of a search of `conn` from `start` to `goal` (A5): `None` when Steiner
/// attachment is off, the connection has no committed sibling / hand copper on either side, or
/// every candidate was excluded. Goal-side junctions win over start-side (reversed) ones.
/// O(p α(p) + Σ sibling leg length).
pub fn plan(
    mg: &MultiGraph,
    conn: &Connection,
    start: &[(usize, usize)],
    goal: &Goal,
) -> Option<Plan> {
    let sn = mg.steiner.as_ref()?;
    let i = conn.id;
    let me = sn.conns.get(i)?;
    let (gs, ss) = sides(mg, sn, i);
    let copper_ok = !mg.steiner_reject.contains(&(i, usize::MAX));
    // A sibling that leaves a pad of the same name as the search's start (the sub-pads of one
    // exposed pad, numbered alike) is that start's own exit, not a target: no junction on it.
    let shares =
        |a: usize, own: &[String]| sn.conns[a].owners.iter().flatten().any(|o| own.contains(o));
    let collect = |sibs: &[usize], far: &[String], own: &[String], cl: usize| {
        let strands: Vec<OwnStrand> = sibs
            .iter()
            .filter(|&&a| !shares(a, own))
            .flat_map(|&a| strands_of(mg, sn, a, far))
            .collect();
        let copper: Vec<(usize, usize)> = if copper_ok {
            sn.copper.get(&cl).cloned().unwrap_or_default()
        } else {
            Vec::new()
        };
        (strands, copper)
    };
    let (strands, copper) = collect(&gs, &me.owners[1], &me.owners[0], me.to_cl);
    if !strands.is_empty() || !copper.is_empty() {
        return Some(Plan {
            reversed: false,
            start: start.to_vec(),
            goal: Goal {
                strands,
                copper,
                junction_min_angle_deg: sn.min_angle_deg,
                ..goal.clone()
            },
        });
    }
    if me.pour || goal.vertices.is_empty() {
        return None;
    }
    let (strands, copper) = collect(&ss, &me.owners[0], &me.owners[1], me.from_cl);
    if strands.is_empty() && copper.is_empty() {
        return None;
    }
    Some(Plan {
        reversed: true,
        start: goal.vertices.clone(),
        goal: Goal {
            vertices: start.to_vec(),
            pour_sites: Vec::new(),
            pour_layer: None,
            strands,
            copper,
            junction_min_angle_deg: sn.min_angle_deg,
            junction_cap: goal.junction_cap,
        },
    })
}

/// Lower bound of a junction path of `conn` onto the siblings `fresh[a]` (A5 commit-phase
/// re-search trigger): the distance from the search's start terminal to the nearest junction
/// edge of those siblings, on the side [`plan`] would use. `None` when there is none.
/// O(p α(p) + Σ sibling leg length).
pub fn lower_bound(mg: &MultiGraph, conn: &Connection, fresh: &[bool]) -> Option<f64> {
    let sn = mg.steiner.as_ref()?;
    let me = sn.conns.get(conn.id)?;
    let (gs, ss) = sides(mg, sn, conn.id);
    let (sibs, from, far) = if !gs.is_empty() {
        (gs, conn.from.at, &me.owners[1])
    } else if !me.pour {
        (ss, conn.to.at, &me.owners[0])
    } else {
        return None;
    };
    let mut best = f64::INFINITY;
    for a in sibs
        .into_iter()
        .filter(|&a| fresh.get(a).copied().unwrap_or(false))
    {
        for st in strands_of(mg, sn, a, far) {
            let g = &mg.layers[st.layer_idx].g;
            let ge = &g.edges[st.edge];
            let seg = Seg {
                a: g.verts[ge.a].at,
                b: g.verts[ge.b].at,
            };
            best = best.min(point_seg(from, &seg));
        }
    }
    best.is_finite().then_some(best)
}

/// Graph vertex owner labels of a terminal: its `TermRef` label (terminal vertex) and, for a
/// pad / via, the obstacle anchor label (`pad:REF:NUM`, `via:uuid`). O(1).
pub fn owner_labels(t: &crate::decompose::TermRef) -> Vec<String> {
    use crate::decompose::TermKind;
    let mut v = vec![t.label.clone()];
    match t.kind {
        TermKind::Pad(_) => {
            if let Some((r, n)) = t.label.rsplit_once('.') {
                v.push(format!("pad:{r}:{n}"));
            }
        }
        TermKind::Via(_) => v.push(format!("via:{}", t.label)),
        _ => {}
    }
    v
}
