//! Net decomposition (12 §1): copper clusters, MST connections, scope and routing order.
//!
//! Per net the pads, vias, kept copper items and pours are joined into clusters (union-find
//! over contact). The connections to route are the minimum spanning tree over the clusters
//! the scope selects, attached at the closest ports (pad / via centres, kept item ends).
//! Everything is built from sorted vectors and `BTreeMap`s: the output is deterministic.

use crate::geom::{
    copper_gap, copper_gap_skeleton, dequantise, dist_prim_skeleton, point_arc, point_seg, Prim,
    Seg, Track, Vec2,
};
use crate::check::point_in_poly;
use crate::model::{via_layers, BoardModel, Options, Pad, PadRef, Pour, Pt};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum TermKind {
    /// Index into `model.pads`.
    Pad(usize),
    /// Index into `model.vias`.
    Via(usize),
    /// End of hand copper (a junction), kept item uuid.
    Copper(String),
    /// Free end of a half-drawn track, kept item uuid.
    Dangling(String),
    /// Pour-completion target: index into `model.pours`.
    Pour(usize),
}

fn ser_vec2<S: serde::Serializer>(v: &Vec2, s: S) -> Result<S::Ok, S::Error> {
    [v.x, v.y].serialize(s)
}

#[derive(Clone, Debug, Serialize)]
pub struct TermRef {
    pub kind: TermKind,
    /// `"U1.3"` for a pad, the uuid for a via / kept item.
    pub label: String,
    /// Math frame, nm. For a pour: the nearest point of its fill outline.
    #[serde(serialize_with = "ser_vec2")]
    pub at: Vec2,
    /// Routable copper layers (sorted, a subset of the `layers` given to `decompose`) the
    /// terminal has copper on; for a pour its fill layers.
    pub layers: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct Connection {
    pub id: usize,
    pub net: i32,
    pub width: i64,
    pub clearance: i64,
    /// Priority tier (12 §3.1): 2 when wider than the default width, else 3.
    pub tier: u8,
    pub from: TermRef,
    pub to: TermRef,
    pub est_len: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct NetInfo {
    pub net: i32,
    pub name: String,
    pub pads: usize,
    pub clusters: usize,
    pub pour_served: bool,
    pub in_scope: bool,
    /// Ids into `Decomposition.connections`.
    pub connections: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Decomposition {
    pub connections: Vec<Connection>,
    pub nets: Vec<NetInfo>,
    /// In-scope nets that yield no connection although a terminal was selected: `(net, reason)`.
    pub skipped: Vec<(i32, String)>,
    /// Model problems found while decomposing (e.g. a pour without fill outline).
    pub warnings: Vec<String>,
}

struct Dsu(Vec<usize>);

impl Dsu {
    fn new(n: usize) -> Dsu {
        Dsu((0..n).collect())
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        // Keep the lower index as the root so cluster order follows node order.
        if a < b {
            self.0[b] = a;
        } else if b < a {
            self.0[a] = b;
        }
    }
}

/// Port of a cluster: a place a connection may attach.
#[derive(Clone, Debug)]
struct Port {
    kind: PortKind,
    at: Vec2,
}

#[derive(Clone, Debug)]
enum PortKind {
    Pad(usize),
    Via(usize),
    Kept(usize),
}

struct Cluster {
    /// Usable pours (indices into `model.pours`) joined into this cluster.
    pours: Vec<usize>,
    ports: Vec<Port>,
    has_terminal: bool,
    selected: bool,
}

fn point_prim(p: Vec2, prim: &Prim) -> f64 {
    match prim {
        Prim::Seg(s) => point_seg(p, s),
        Prim::Arc(a) => point_arc(p, a),
    }
}

/// Distance from `p` to the closed ring `poly` and the nearest ring point; `(0, p)` inside.
/// O(P) in the ring size.
fn point_to_poly(p: Vec2, poly: &[Vec2]) -> (f64, Vec2) {
    if poly.len() < 3 {
        return (f64::INFINITY, p);
    }
    if point_in_poly(p, poly) {
        return (0.0, p);
    }
    let mut best = (f64::INFINITY, p);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let ab = b - a;
        let l2 = ab.x * ab.x + ab.y * ab.y;
        let t = if l2 > 0.0 {
            (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = Vec2 {
            x: a.x + t * ab.x,
            y: a.y + t * ab.y,
        };
        let d = (p - q).len();
        if d < best.0 {
            best = (d, q);
        }
    }
    best
}

/// Scope resolution: which pads and vias are selected terminals.
struct Selector<'a> {
    model: &'a BoardModel,
    opts: &'a Options,
    /// Scope area converted to the math frame (the options are in the KiCad frame).
    area: Option<Vec<Vec2>>,
}

impl<'a> Selector<'a> {
    fn new(model: &'a BoardModel, opts: &'a Options) -> Self {
        let area = opts.scope.area.as_ref().map(|a| {
            a.iter()
                .map(|p: &Pt| Vec2 {
                    x: p.x as f64,
                    y: -(p.y as f64),
                })
                .collect()
        });
        Selector { model, opts, area }
    }

    fn net_selected(&self, net: i32) -> bool {
        let sc = &self.opts.scope;
        if sc.is_whole_board() {
            return true;
        }
        if !sc.nets.is_empty() {
            let name = self
                .model
                .nets
                .iter()
                .find(|n| n.code == net)
                .map(|n| n.name.as_str());
            if name.is_some_and(|n| sc.nets.iter().any(|s| s == n)) {
                return true;
            }
        }
        if !sc.netclasses.is_empty() {
            let class = &self.model.rules.net_rule(net).netclass;
            if class
                .split(',')
                .any(|c| sc.netclasses.iter().any(|s| s == c.trim()))
            {
                return true;
            }
        }
        false
    }

    fn in_area(&self, p: Vec2) -> bool {
        self.area.as_ref().is_some_and(|a| point_in_poly(p, a))
    }

    fn pad(&self, p: &Pad) -> bool {
        let sc = &self.opts.scope;
        sc.is_whole_board()
            || sc.selects_pad(p)
            || self.net_selected(p.net)
            || self.in_area(p.center.to_vec2())
    }

    fn via(&self, net: i32, at: Vec2, uuid: &str) -> bool {
        self.opts.scope.is_whole_board()
            || self.net_selected(net)
            || self.in_area(at)
            || self.opts.scope.vias.iter().any(|u| u == uuid)
    }
}

pub fn parse_pour_pad(s: &str) -> Option<PadRef> {
    PadRef::try_from(s.to_string()).ok()
}

fn bbox_overlap(a: &crate::geom::Aabb, b: &crate::geom::Aabb, pad: f64) -> bool {
    a.min.x - pad <= b.max.x
        && b.min.x - pad <= a.max.x
        && a.min.y - pad <= b.max.y
        && b.min.y - pad <= a.max.y
}

/// Pad, via and kept-item indices of one net.
type NetItems = (Vec<usize>, Vec<usize>, Vec<usize>);

/// Decompose every net into the connections still to route; `layers` = routable copper layers
/// (sorted). In a net with a pour every cluster without the pour gets one connection to it
/// (pour completion, 12 §1), otherwise the clusters are joined by an MST.
/// O(pads^2) per net for the MST ports plus O(P) per cluster port for the pour distance.
pub fn decompose(model: &BoardModel, opts: &Options, layers: &[u32]) -> Decomposition {
    let sel = Selector::new(model, opts);
    let all_layers = model.all_layer_ids();

    let mut by_net: BTreeMap<i32, NetItems> = BTreeMap::new();
    for (i, p) in model.pads.iter().enumerate() {
        if p.net != 0 {
            by_net.entry(p.net).or_default().0.push(i);
        }
    }
    for (i, v) in model.vias.iter().enumerate() {
        if v.net != 0 {
            by_net.entry(v.net).or_default().1.push(i);
        }
    }
    for (i, k) in model.kept.iter().enumerate() {
        if k.net != 0 {
            by_net.entry(k.net).or_default().2.push(i);
        }
    }
    let kept_prims: Vec<Prim> = model.kept.iter().map(|k| dequantise(&k.prim)).collect();
    let kept_boxes: Vec<_> = kept_prims.iter().map(|p| p.bbox()).collect();

    let mut out = Decomposition::default();
    for (&net, (pads, vias, kept)) in &by_net {
        if pads.is_empty() {
            continue; // no terminal: nothing to connect
        }
        let (np, nv, nk) = (pads.len(), vias.len(), kept.len());
        let pours: Vec<(usize, &Pour)> = model
            .pours
            .iter()
            .enumerate()
            .filter(|(_, z)| z.net == net)
            .collect();
        let npz = pours.len();
        // Local node ids: pads, vias, kept items.
        let mut dsu = Dsu::new(np + nv + nk + npz);
        let track = |ki: usize| Track {
            prim: kept_prims[ki],
            w: model.kept[ki].width as f64,
        };

        for (a, &pi) in pads.iter().enumerate() {
            let pad = &model.pads[pi];
            for (c, &ki) in kept.iter().enumerate() {
                let k = &model.kept[ki];
                if !pad.layers.contains(&k.layer) {
                    continue;
                }
                let sk = model.skeleton(pad, k.layer);
                if copper_gap_skeleton(&track(ki), &sk) <= 0.0 {
                    dsu.union(a, np + nv + c);
                }
            }
            for (b, &vi) in vias.iter().enumerate() {
                let v = &model.vias[vi];
                let vl = via_layers(&v.layers, &all_layers);
                let Some(&l) = vl.iter().find(|l| pad.layers.contains(l)) else {
                    continue;
                };
                let at = v.at.to_vec2();
                let dot = Prim::Seg(Seg { a: at, b: at });
                if dist_prim_skeleton(&dot, &model.skeleton(pad, l)) - 0.5 * v.d as f64 <= 0.0 {
                    dsu.union(a, np + b);
                }
            }
        }
        for (b, &vi) in vias.iter().enumerate() {
            let v = &model.vias[vi];
            let vl = via_layers(&v.layers, &all_layers);
            let at = v.at.to_vec2();
            for (c, &ki) in kept.iter().enumerate() {
                if vl.contains(&model.kept[ki].layer)
                    && point_prim(at, &kept_prims[ki]) <= 0.5 * v.d as f64
                {
                    dsu.union(np + b, np + nv + c);
                }
            }
        }
        for (c, &ki) in kept.iter().enumerate() {
            for (d, &kj) in kept.iter().enumerate().skip(c + 1) {
                if model.kept[ki].layer != model.kept[kj].layer {
                    continue;
                }
                let pad_w = 0.5 * (model.kept[ki].width + model.kept[kj].width) as f64;
                if !bbox_overlap(&kept_boxes[ki], &kept_boxes[kj], pad_w) {
                    continue;
                }
                if copper_gap(&track(ki), &track(kj)) <= 0.0 {
                    dsu.union(np + nv + c, np + nv + d);
                }
            }
        }
        // Pours join the pads and vias they list that have copper on one of the pour's layers
        // (a pour is a node of its own).
        for (zn, (_, z)) in pours.iter().enumerate() {
            let pn = np + nv + nk + zn;
            for s in &z.pads {
                let Some(r) = parse_pour_pad(s) else { continue };
                if let Some(a) = pads.iter().position(|&pi| {
                    model.pads[pi].ref_ == r.ref_
                        && model.pads[pi].number == r.number
                        && model.pads[pi].layers.iter().any(|l| z.layers.contains(l))
                }) {
                    dsu.union(a, pn);
                }
            }
            for u in &z.vias {
                if let Some(b) = vias.iter().position(|&vi| {
                    let v = &model.vias[vi];
                    &v.uuid == u
                        && via_layers(&v.layers, &all_layers)
                            .iter()
                            .any(|l| z.layers.contains(l))
                }) {
                    dsu.union(np + b, pn);
                }
            }
        }

        // Clusters in order of their lowest node.
        let mut cl_of_root: BTreeMap<usize, usize> = BTreeMap::new();
        let mut clusters: Vec<Cluster> = Vec::new();
        let mut cl_of_node = vec![0usize; np + nv + nk + npz];
        for (n, slot) in cl_of_node.iter_mut().enumerate() {
            let r = dsu.find(n);
            let ci = *cl_of_root.entry(r).or_insert_with(|| {
                clusters.push(Cluster {
                    pours: Vec::new(),
                    ports: Vec::new(),
                    has_terminal: false,
                    selected: false,
                });
                clusters.len() - 1
            });
            *slot = ci;
        }
        for (zn, (zi, z)) in pours.iter().enumerate() {
            if z.fills.iter().any(|f| f.outer.len() >= 3) {
                clusters[cl_of_node[np + nv + nk + zn]].pours.push(*zi);
            } else {
                out.warnings
                    .push(format!("pour {} has no fill outline", z.uuid));
            }
        }
        let net_has_pour = clusters.iter().any(|c| !c.pours.is_empty());
        let mut selected_terms = 0usize;
        for (a, &pi) in pads.iter().enumerate() {
            let c = &mut clusters[cl_of_node[a]];
            c.has_terminal = true;
            c.ports.push(Port {
                kind: PortKind::Pad(pi),
                at: model.pads[pi].center.to_vec2(),
            });
            if sel.pad(&model.pads[pi]) {
                c.selected = true;
                selected_terms += 1;
            }
        }
        for (b, &vi) in vias.iter().enumerate() {
            let v = &model.vias[vi];
            let c = &mut clusters[cl_of_node[np + b]];
            c.has_terminal = true;
            c.ports.push(Port {
                kind: PortKind::Via(vi),
                at: v.at.to_vec2(),
            });
            if sel.via(net, v.at.to_vec2(), &v.uuid) {
                c.selected = true;
                selected_terms += 1;
            }
        }
        for (k, &ki) in kept.iter().enumerate() {
            let c = &mut clusters[cl_of_node[np + nv + k]];
            let prim = &model.kept[ki].prim;
            for e in [prim.start(), prim.end()] {
                let at = e.to_f64();
                if c.ports.iter().any(|p| (p.at - at).len() <= 2.0) {
                    continue; // already a port (pad / via centre or a shared end)
                }
                c.ports.push(Port {
                    kind: PortKind::Kept(ki),
                    at,
                });
            }
        }

        // A pour without listed items is a cluster without ports; it is no copper cluster to join.
        let nclusters = clusters.iter().filter(|c| !c.ports.is_empty()).count();
        let in_scope = selected_terms > 0;
        let mut conn_ids = Vec::new();
        let mut skipped = None;
        if in_scope {
            let terminal: Vec<usize> = (0..clusters.len())
                .filter(|&i| clusters[i].has_terminal)
                .collect();
            let chosen: Vec<usize> = terminal
                .iter()
                .copied()
                .filter(|&i| clusters[i].selected)
                .collect();
            // Pour completion: every chosen cluster without a pour attaches to the nearest pour.
            let mut pour_edges: Vec<(usize, usize, usize, Vec2, f64)> = Vec::new();
            let edges: Vec<(usize, usize, usize, usize, f64)> = if net_has_pour {
                for &a in chosen.iter().filter(|&&a| clusters[a].pours.is_empty()) {
                    let mut best: Option<(usize, usize, Vec2, f64)> = None;
                    for (pi, port) in clusters[a].ports.iter().enumerate() {
                        for zi in clusters.iter().flat_map(|c| c.pours.iter().copied()) {
                            for f in model.pours[zi].fills.iter().filter(|f| f.outer.len() >= 3) {
                                let ring: Vec<Vec2> =
                                    f.outer.iter().map(|q| q.to_vec2()).collect();
                                let (d, q) = point_to_poly(port.at, &ring);
                                if best.is_none_or(|b| d < b.3) {
                                    best = Some((pi, zi, q, d));
                                }
                            }
                        }
                    }
                    if let Some((pi, zi, q, d)) = best {
                        pour_edges.push((a, pi, zi, q, d));
                    }
                }
                let with_pour: Vec<usize> = chosen
                    .iter()
                    .copied()
                    .filter(|&i| !clusters[i].pours.is_empty() && !clusters[i].ports.is_empty())
                    .collect();
                mst(&clusters, &with_pour)
            } else if selected_terms == 1 {
                // Lone selected terminal: nearest other cluster of the net.
                let a = chosen[0];
                let mut best: Option<(usize, usize, usize, f64)> = None;
                for &b in terminal.iter().filter(|&&b| b != a) {
                    let (pa, pb, d) = closest(&clusters[a], &clusters[b]);
                    if best.is_none_or(|x| d < x.3) {
                        best = Some((b, pa, pb, d));
                    }
                }
                match best {
                    Some((b, pa, pb, d)) => vec![(a, b, pa, pb, d)],
                    None => {
                        skipped = Some("no other terminal in the net".to_string());
                        Vec::new()
                    }
                }
            } else {
                mst(&clusters, &chosen)
            };
            let rule = model.rules.net_rule(net);
            let tier = if rule.width > model.rules.default.width {
                2
            } else {
                3
            };
            let mut edges = edges;
            edges.sort_by_key(|&(a, b, ..)| (a.min(b), a.max(b)));
            for (a, b, pa, pb, d) in edges {
                // Orient from the lower cluster.
                let (lo, hi, plo, phi) = if a < b {
                    (a, b, pa, pb)
                } else {
                    (b, a, pb, pa)
                };
                let id = out.connections.len();
                out.connections.push(Connection {
                    id,
                    net,
                    width: rule.width,
                    clearance: rule.clearance,
                    tier,
                    from: term_ref(model, &clusters[lo].ports[plo], net, layers),
                    to: term_ref(model, &clusters[hi].ports[phi], net, layers),
                    est_len: d,
                });
                conn_ids.push(id);
            }
            for (a, pi, zi, q, d) in pour_edges {
                let id = out.connections.len();
                let mut pl: Vec<u32> = model.pours[zi].fills.iter().map(|f| f.layer).collect();
                pl.sort_unstable();
                pl.dedup();
                pl.retain(|l| layers.contains(l));
                out.connections.push(Connection {
                    id,
                    net,
                    width: rule.width,
                    clearance: rule.clearance,
                    tier,
                    from: term_ref(model, &clusters[a].ports[pi], net, layers),
                    to: TermRef {
                        kind: TermKind::Pour(zi),
                        label: format!("pour:{}", model.pours[zi].uuid),
                        at: q,
                        layers: pl,
                    },
                    est_len: d,
                });
                conn_ids.push(id);
            }
        }
        if let Some(r) = skipped {
            out.skipped.push((net, r));
        }
        let pour_served = if net_has_pour {
            conn_ids.is_empty()
        } else {
            !pours.is_empty() && np + nv >= 2 && conn_ids.is_empty()
        };
        out.nets.push(NetInfo {
            net,
            name: model
                .nets
                .iter()
                .find(|n| n.code == net)
                .map(|n| n.name.clone())
                .unwrap_or_default(),
            pads: np,
            clusters: nclusters,
            pour_served,
            in_scope,
            connections: conn_ids,
        });
    }
    out
}

/// Closest port pair of two clusters: `(port in a, port in b, distance)`; first minimum wins.
fn closest(a: &Cluster, b: &Cluster) -> (usize, usize, f64) {
    let mut best = (0, 0, f64::INFINITY);
    for (i, pa) in a.ports.iter().enumerate() {
        for (j, pb) in b.ports.iter().enumerate() {
            let d = (pa.at - pb.at).len();
            if d < best.2 {
                best = (i, j, d);
            }
        }
    }
    best
}

/// Prim's MST over `nodes` (cluster indices): `(a, b, port in a, port in b, weight)`.
fn mst(clusters: &[Cluster], nodes: &[usize]) -> Vec<(usize, usize, usize, usize, f64)> {
    let n = nodes.len();
    let mut edges = Vec::new();
    if n < 2 {
        return edges;
    }
    let mut in_tree = vec![false; n];
    // Best known connection of every outside node to the tree: (tree slot, pa, pb, w).
    let mut best: Vec<Option<(usize, usize, usize, f64)>> = vec![None; n];
    let add =
        |t: usize, in_tree: &mut Vec<bool>, best: &mut Vec<Option<(usize, usize, usize, f64)>>| {
            in_tree[t] = true;
            for j in (0..n).filter(|&j| !in_tree[j]) {
                let (pa, pb, d) = closest(&clusters[nodes[t]], &clusters[nodes[j]]);
                if best[j].is_none_or(|b| d < b.3) {
                    best[j] = Some((t, pa, pb, d));
                }
            }
        };
    add(0, &mut in_tree, &mut best);
    for _ in 1..n {
        let mut pick: Option<(usize, f64)> = None;
        for j in (0..n).filter(|&j| !in_tree[j]) {
            if let Some(b) = best[j] {
                if pick.is_none_or(|p| b.3 < p.1) {
                    pick = Some((j, b.3));
                }
            }
        }
        let Some((j, _)) = pick else { break };
        let (t, pa, pb, d) = best[j].expect("picked");
        edges.push((nodes[t], nodes[j], pa, pb, d));
        add(j, &mut in_tree, &mut best);
    }
    edges
}

fn term_ref(model: &BoardModel, port: &Port, net: i32, layers: &[u32]) -> TermRef {
    let keep = |ls: &[u32]| -> Vec<u32> {
        let mut v: Vec<u32> = ls.iter().copied().filter(|l| layers.contains(l)).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    match port.kind {
        PortKind::Pad(i) => {
            let p = &model.pads[i];
            TermRef {
                kind: TermKind::Pad(i),
                label: format!("{}.{}", p.ref_, p.number),
                at: port.at,
                layers: keep(&p.layers),
            }
        }
        PortKind::Via(i) => TermRef {
            kind: TermKind::Via(i),
            label: model.vias[i].uuid.clone(),
            at: port.at,
            layers: keep(&via_layers(&model.vias[i].layers, &model.all_layer_ids())),
        },
        PortKind::Kept(i) => {
            let k = &model.kept[i];
            let dangling = model.dangling_ends.iter().any(|d| {
                d.item == k.uuid && d.net == net && (d.end.to_vec2() - port.at).len() <= 2.0
            });
            TermRef {
                kind: if dangling {
                    TermKind::Dangling(k.uuid.clone())
                } else {
                    TermKind::Copper(k.uuid.clone())
                },
                label: k.uuid.clone(),
                at: port.at,
                layers: keep(&[k.layer]),
            }
        }
    }
}

/// Routing order (indices into `conns`): failure bumps first, then tier, wider first, shorter
/// first, net, id. `bumps` is indexed like `conns`; missing entries count as 0.
pub fn order(conns: &[Connection], bumps: &[u32]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..conns.len()).collect();
    idx.sort_by(|&a, &b| {
        let (ca, cb) = (&conns[a], &conns[b]);
        let ba = bumps.get(a).copied().unwrap_or(0);
        let bb = bumps.get(b).copied().unwrap_or(0);
        bb.cmp(&ba)
            .then(ca.tier.cmp(&cb.tier))
            .then(cb.width.cmp(&ca.width))
            .then(ca.est_len.total_cmp(&cb.est_len))
            .then(ca.net.cmp(&cb.net))
            .then(ca.id.cmp(&cb.id))
    });
    idx
}

/// Wire terminal of a connection end.
pub fn terminal_of(t: &TermRef) -> crate::wires::Terminal {
    use crate::wires::Terminal;
    match &t.kind {
        TermKind::Pad(i) => Terminal::Pad { pad: *i, at: t.at },
        TermKind::Via(i) => Terminal::Via { via: *i, at: t.at },
        TermKind::Copper(_) => Terminal::Junction { at: t.at },
        TermKind::Dangling(_) => Terminal::Dangling { at: t.at },
        // The via lands in the pour: a router via site, assigned by the search (P6).
        TermKind::Pour(_) => Terminal::Site {
            site: usize::MAX,
            at: t.at,
        },
    }
}
