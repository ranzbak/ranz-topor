//! Single-wire taut chain (21 §2–4, 11 §4, 25 §2): a tangent-arc chain over an anchor sequence,
//! plus the add/drop fix-up loop. See `docs/progress/notes/m1-taut-design.md` §1–2.
//!
//! Every pass is O(k · q) (k anchors, q = one `Grid` query); `TAUT_PASS_CAP` bounds the loop.
use crate::geom::prim::{Aabb, Arc, Circle, Prim, Seg};
use crate::geom::{bitangent, dist_prim, point_arc, point_seg, GeomError, Orient, Vec2};
use crate::obstacle::{Core, LayerObstacles};
use std::collections::HashSet;
use std::f64::consts::TAU;

pub const TAUT_PASS_CAP: u32 = 64;
/// 25 §2 ε-band: drop an anchor only when its slack exceeds this, nm.
pub const SLACK_BAND_NM: f64 = 10_000.0;
/// Sleeve length cap (45 §5a).
pub use crate::cdt::MAX_SLEEVE;
/// Penetration tolerance, nm.
const PEN_TOL: f64 = 1.0;

pub use crate::sketch::AnchorRef;

#[derive(Clone, Debug, PartialEq)]
pub enum TautError {
    InsideObstacle(String),
    Infeasible(String),
    Cap,
    Sleeve(String),
}

#[derive(Clone, Debug)]
pub struct Chain {
    /// Alternating `Seg` / `Arc`, G1.
    pub prims: Vec<Prim>,
    pub seq: Vec<AnchorRef>,
    pub length: f64,
}

impl Chain {
    /// `"SAS…"`.
    pub fn pattern(&self) -> String {
        self.prims
            .iter()
            .map(|p| match p {
                Prim::Seg(_) => 'S',
                Prim::Arc(_) => 'A',
            })
            .collect()
    }
}

/// Dynamic obstacle = a primitive of another wire's current chain.
#[derive(Clone, Debug)]
pub struct DynObstacle {
    pub prim: Prim,
    pub width: f64,
    pub net: i32,
    pub wire: usize,
    /// Arcs: the anchor they wrap.
    pub anchor: Option<AnchorRef>,
    /// Segments: the anchors at both ends.
    pub adjacent: [Option<AnchorRef>; 2],
}

pub trait Env {
    fn layer(&self) -> &LayerObstacles;
    /// This wire's inflation of anchor `a` (static obstacle), incl. stack-independent parts.
    fn rho_anchor(&self, a: usize) -> f64;
    fn rho_wall(&self, w: usize) -> f64;
    fn exempt_anchor(&self, a: usize) -> bool;
    fn exempt_wall(&self, w: usize) -> bool;
    /// Other wires' prims (same layer, same net excluded) whose bbox overlaps `bb`.
    fn dyn_near(&self, bb: Aabb) -> Vec<&DynObstacle>;
    /// Required centreline distance to `d`: `c(j,k) + ε + w_j/2 + w_k/2`.
    fn dyn_clearance(&self, d: &DynObstacle) -> f64;
    /// Radius for this wire at `(anchor, σ)`, ≥ `rho_anchor`.
    fn stack_radius(&self, a: AnchorRef) -> f64;
    /// Bound on `rho − r_core` and on every `dyn_clearance` (query pre-grow).
    fn query_grow(&self) -> f64;
}

// ---------------------------------------------------------------------------------------------
// geometry

fn key(p: Vec2) -> (u64, u64) {
    (p.x.to_bits(), p.y.to_bits())
}

#[derive(Clone, Copy)]
struct Node {
    c: Circle,
    s: Option<Orient>,
}

fn node(a: AnchorRef, c: Vec2, r: f64) -> Node {
    Node {
        c: Circle { c, r },
        s: Some(a.sigma),
    }
}

fn term(p: Vec2) -> Node {
    Node {
        c: Circle { c: p, r: 0.0 },
        s: None,
    }
}

fn bt(a: &Node, b: &Node) -> Result<Seg, GeomError> {
    let sa = a.s.or(b.s).unwrap_or(Orient::Ccw);
    let sb = b.s.or(a.s).unwrap_or(Orient::Ccw);
    bitangent(a.c, sa, b.c, sb)
}

fn label(l: &LayerObstacles, a: usize) -> String {
    l.anchors[a].label.clone()
}

fn push_seg(prims: &mut Vec<Prim>, s: Seg) {
    if let Some(Prim::Seg(last)) = prims.last_mut() {
        last.b = s.b;
    } else {
        prims.push(Prim::Seg(s));
    }
}

fn is_zero_sweep(a: &Arc) -> bool {
    a.r * a.sweep < 1.0 || a.r * (TAU - a.sweep) < 1.0
}

/// Geometry of a fixed anchor sequence (21 §2–3). Exempt and concentric anchors are removed.
pub fn chain_geometry(
    start: Vec2,
    goal: Vec2,
    seq: &[AnchorRef],
    env: &dyn Env,
) -> Result<Chain, TautError> {
    let l = env.layer();
    let g = env.query_grow();
    for p in [start, goal] {
        let bb = Aabb { min: p, max: p }.grow(g);
        for a in query_anchors(l, bb) {
            if !env.exempt_anchor(a) && (p - l.anchors[a].c).len() < env.rho_anchor(a) - PEN_TOL {
                return Err(TautError::InsideObstacle(label(l, a)));
            }
        }
        for w in query_walls(l, bb) {
            if !env.exempt_wall(w)
                && core_point_dist(&l.walls[w].core, p) < env.rho_wall(w) - PEN_TOL
            {
                return Err(TautError::InsideObstacle(l.walls[w].label.clone()));
            }
        }
    }
    let mut seen = HashSet::new();
    let mut kept: Vec<AnchorRef> = Vec::with_capacity(seq.len());
    for &a in seq {
        if env.exempt_anchor(a.anchor) || !seen.insert(key(l.anchors[a.anchor].c)) {
            continue;
        }
        kept.push(a);
    }
    if kept.len() > MAX_SLEEVE {
        return Err(TautError::Sleeve(format!("{} anchors", kept.len())));
    }
    let mut nodes = Vec::with_capacity(kept.len() + 2);
    nodes.push(term(start));
    for &a in &kept {
        nodes.push(node(a, l.anchors[a.anchor].c, env.stack_radius(a)));
    }
    nodes.push(term(goal));
    let mut segs = Vec::with_capacity(nodes.len() - 1);
    for i in 0..nodes.len() - 1 {
        match bt(&nodes[i], &nodes[i + 1]) {
            Ok(s) => segs.push(s),
            Err(GeomError::InsideObstacle) => {
                let who = if i + 1 < nodes.len() - 1 { i } else { i - 1 };
                let who = who.min(kept.len().saturating_sub(1));
                return Err(TautError::InsideObstacle(
                    kept.get(who)
                        .map(|a| label(l, a.anchor))
                        .unwrap_or_default(),
                ));
            }
            Err(_) => {
                let n = |j: usize| -> String {
                    if j == 0 {
                        "start".into()
                    } else if j == nodes.len() - 1 {
                        "goal".into()
                    } else {
                        label(l, kept[j - 1].anchor)
                    }
                };
                return Err(TautError::Infeasible(format!("{} <-> {}", n(i), n(i + 1))));
            }
        }
    }
    let mut prims: Vec<Prim> = Vec::new();
    push_seg(&mut prims, segs[0]);
    for (i, a) in kept.iter().enumerate() {
        let arc = Arc::from_tangent_points(
            nodes[i + 1].c.c,
            nodes[i + 1].c.r,
            a.sigma,
            segs[i].b,
            segs[i + 1].a,
        );
        if is_zero_sweep(&arc) {
            // seg ends are already continuous through the next push (merged)
            push_seg(&mut prims, segs[i + 1]);
        } else {
            prims.push(Prim::Arc(arc));
            prims.push(Prim::Seg(segs[i + 1]));
        }
    }
    let length = prims.iter().map(|p| p.length()).sum();
    Ok(Chain {
        prims,
        seq: kept,
        length,
    })
}

/// `(prim index, seq index)` for every arc of `chain` (matched by centre and σ in path order).
pub fn arc_members(chain: &Chain, env: &dyn Env) -> Vec<(usize, usize)> {
    let l = env.layer();
    let mut out = Vec::new();
    let mut cur = 0usize;
    for (pi, p) in chain.prims.iter().enumerate() {
        if let Prim::Arc(a) = p {
            let hit = (cur..chain.seq.len()).find(|&k| {
                let s = chain.seq[k];
                s.sigma == a.sigma && key(l.anchors[s.anchor].c) == key(a.c)
            });
            if let Some(k) = hit {
                out.push((pi, k));
                cur = k + 1;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// obstacle queries

fn query_anchors(l: &LayerObstacles, bb: Aabb) -> Vec<usize> {
    l.query_anchors(bb)
}

fn query_walls(l: &LayerObstacles, bb: Aabb) -> Vec<usize> {
    l.query_walls(bb)
}

fn core_point_dist(c: &Core, p: Vec2) -> f64 {
    match c {
        Core::Point(q) => (p - *q).len(),
        Core::Seg(s) => point_seg(p, s),
        Core::Arc(a) => point_arc(p, a),
    }
}

fn point_prim(p: Vec2, q: &Prim) -> f64 {
    match q {
        Prim::Seg(s) => point_seg(p, s),
        Prim::Arc(a) => point_arc(p, a),
    }
}

fn core_prim_dist(c: &Core, q: &Prim) -> f64 {
    match c {
        Core::Point(p) => point_prim(*p, q),
        Core::Seg(s) => dist_prim(&Prim::Seg(*s), q),
        Core::Arc(a) => dist_prim(&Prim::Arc(*a), q),
    }
}

/// Anchors already in the sequence (by index or bit-equal centre).
#[derive(Clone)]
struct Members {
    idx: HashSet<usize>,
    centres: HashSet<(u64, u64)>,
}

impl Members {
    fn new(seq: &[AnchorRef], l: &LayerObstacles) -> Self {
        Members {
            idx: seq.iter().map(|a| a.anchor).collect(),
            centres: seq.iter().map(|a| key(l.anchors[a.anchor].c)).collect(),
        }
    }
    fn has(&self, a: usize, l: &LayerObstacles) -> bool {
        self.idx.contains(&a) || self.centres.contains(&key(l.anchors[a].c))
    }
}

/// Anchors to add for the non-member obstacles that `prim` penetrates (sorted, deduplicated).
fn penetrations(prim: &Prim, mem: &Members, env: &dyn Env) -> Result<Vec<usize>, TautError> {
    let l = env.layer();
    let bb = prim.bbox().grow(env.query_grow());
    let mut out: Vec<usize> = Vec::new();
    for a in query_anchors(l, bb) {
        if mem.has(a, l) || env.exempt_anchor(a) {
            continue;
        }
        if point_prim(l.anchors[a].c, prim) < env.rho_anchor(a) - PEN_TOL {
            out.push(a);
        }
    }
    for w in query_walls(l, bb) {
        if env.exempt_wall(w) {
            continue;
        }
        let wall = &l.walls[w];
        if core_prim_dist(&wall.core, prim) >= env.rho_wall(w) - PEN_TOL {
            continue;
        }
        let d = |e: usize| point_prim(l.anchors[e].c, prim);
        let [e0, e1] = wall.ends;
        let (near, far) = if d(e0) <= d(e1) { (e0, e1) } else { (e1, e0) };
        for e in [near, far] {
            if !mem.has(e, l) && !env.exempt_anchor(e) {
                // The end anchor must cover the penetration point; otherwise the wall is grazed
                // mid-span and no end anchor can express the detour.
                if d(e) >= env.rho_anchor(e) + SLACK_BAND_NM {
                    let depth = env.rho_wall(w) - core_prim_dist(&wall.core, prim);
                    return Err(TautError::Infeasible(format!(
                        "wall {} grazed mid-span by {:.0} nm",
                        wall.label, depth
                    )));
                }
                out.push(e);
                break;
            }
        }
    }
    for d in env.dyn_near(bb) {
        if dist_prim(prim, &d.prim) >= env.dyn_clearance(d) - PEN_TOL {
            continue;
        }
        let cands: Vec<AnchorRef> = match d.prim {
            Prim::Arc(_) => d.anchor.into_iter().collect(),
            Prim::Seg(_) => d.adjacent.iter().flatten().copied().collect(),
        };
        for a in cands {
            if !mem.has(a.anchor, l) && !env.exempt_anchor(a.anchor) {
                out.push(a.anchor);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// fix-up loop

fn tri_contains(a: Vec2, b: Vec2, c: Vec2, p: Vec2) -> bool {
    let o = (b - a).cross(c - a);
    if o == 0.0 {
        return false;
    }
    let s = o.signum();
    s * (b - a).cross(p - a) > 0.0
        && s * (c - b).cross(p - b) > 0.0
        && s * (a - c).cross(p - c) > 0.0
}

/// One left-to-right sweep dropping slack anchors; returns whether anything was dropped.
fn drop_pass(start: Vec2, goal: Vec2, seq: &mut Vec<AnchorRef>, env: &dyn Env) -> bool {
    let l = env.layer();
    let n = seq.len();
    let r: Vec<f64> = seq.iter().map(|a| env.stack_radius(*a)).collect();
    let ctr = |i: usize| l.anchors[seq[i].anchor].c;
    let mut mem = Members::new(seq, l);
    let mut kept: Vec<usize> = Vec::with_capacity(n);
    let mut dropped = false;
    for i in 0..n {
        let p = match kept.last() {
            Some(&k) => node(seq[k], ctr(k), r[k]),
            None => term(start),
        };
        let q = if i + 1 < n {
            node(seq[i + 1], ctr(i + 1), r[i + 1])
        } else {
            term(goal)
        };
        let me = node(seq[i], ctr(i), r[i]);
        if is_slack(&p, &me, &q, r[i], seq[i].sigma, &mem, env) {
            mem.centres.remove(&key(ctr(i)));
            mem.idx.remove(&seq[i].anchor);
            dropped = true;
        } else {
            kept.push(i);
        }
    }
    if dropped {
        *seq = kept.iter().map(|&i| seq[i]).collect();
    }
    dropped
}

fn is_slack(
    p: &Node,
    me: &Node,
    q: &Node,
    ri: f64,
    sigma: Orient,
    mem: &Members,
    env: &dyn Env,
) -> bool {
    let Ok(s) = bt(p, q) else { return false };
    let Some(t) = s.dir() else { return false };
    let ci = me.c.c;
    // (b) side: the shortcut would need a turn of sign σ to wrap the anchor
    if sigma.sign() * t.cross(ci - s.a) <= 0.0 {
        return false;
    }
    // (c) slack beyond the ε-band
    if point_seg(ci, &s) < ri + SLACK_BAND_NM {
        return false;
    }
    // After (b) and (c) a wrap of the current arc is < π (a sweep > π would be a negative arc,
    // 2π minus a small backward wrap, which is exactly what must go), so the swept region lies
    // inside the centre triangle.
    let l = env.layer();
    // (e) no non-exempt core centre strictly inside the centre triangle
    let (a, b, c) = (p.c.c, ci, q.c.c);
    let bb = Aabb {
        min: Vec2 {
            x: a.x.min(b.x).min(c.x),
            y: a.y.min(b.y).min(c.y),
        },
        max: Vec2 {
            x: a.x.max(b.x).max(c.x),
            y: a.y.max(b.y).max(c.y),
        },
    };
    for o in query_anchors(l, bb) {
        let oc = l.anchors[o].c;
        if env.exempt_anchor(o) || key(oc) == key(ci) {
            continue;
        }
        if tri_contains(a, b, c, oc) {
            return false;
        }
    }
    // (f) the shortcut violates nothing (the dropped anchor itself is a non-member now)
    let mut mem2 = mem.clone();
    mem2.centres.remove(&key(ci));
    mem2.idx.retain(|&x| key(l.anchors[x].c) != key(ci));
    matches!(penetrations(&Prim::Seg(s), &mem2, env), Ok(v) if v.is_empty())
}

/// Closest point of `prim` to `c`, the travel tangent there, and the fraction along `prim`.
fn closest(prim: &Prim, c: Vec2) -> (Vec2, Vec2, f64) {
    match prim {
        Prim::Seg(s) => {
            let d = s.b - s.a;
            let l2 = d.len2();
            let t = if l2 == 0.0 {
                0.0
            } else {
                ((c - s.a).dot(d) / l2).clamp(0.0, 1.0)
            };
            (s.a + d * t, s.dir().unwrap_or(Vec2 { x: 1.0, y: 0.0 }), t)
        }
        Prim::Arc(a) => {
            let (pt, frac) = match (c - a.c).unit() {
                Some(u) if a.contains_dir(c) => {
                    let p = a.c + u * a.r;
                    let off = (a.sigma.sign() * ((p - a.c).angle() - a.a_start)).rem_euclid(TAU);
                    let f = if a.sweep > 0.0 {
                        (off / a.sweep).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    (p, f)
                }
                _ => {
                    if (c - a.start()).len() <= (c - a.end()).len() {
                        (a.start(), 0.0)
                    } else {
                        (a.end(), 1.0)
                    }
                }
            };
            let rad = (pt - a.c).unit().unwrap_or(Vec2 { x: 1.0, y: 0.0 }).perp();
            let t = if a.sigma == Orient::Ccw { rad } else { -rad };
            (pt, t, frac)
        }
    }
}

fn sigma_for(prim: &Prim, c: Vec2, p: Vec2, t: Vec2) -> Orient {
    let mut x = t.cross(c - p);
    if x == 0.0 {
        let a = match prim {
            Prim::Seg(s) => s.a,
            Prim::Arc(a) => a.start(),
        };
        x = t.cross(c - a);
    }
    if x < 0.0 {
        Orient::Cw
    } else {
        Orient::Ccw
    }
}

/// `(seq position, path key, anchor, σ)` for every penetration of the current chain.
fn add_pass(chain: &Chain, env: &dyn Env) -> Result<Vec<(usize, f64, usize, Orient)>, TautError> {
    let l = env.layer();
    let mem = Members::new(&chain.seq, l);
    let arcs = arc_members(chain, env);
    let mut out = Vec::new();
    let mut after = 0usize; // seq entries before the current prim
    for (pi, prim) in chain.prims.iter().enumerate() {
        let arc_seq = arcs.iter().find(|(p, _)| *p == pi).map(|&(_, k)| k);
        for a in penetrations(prim, &mem, env)? {
            let c = l.anchors[a].c;
            let (pt, t, frac) = closest(prim, c);
            let sg = sigma_for(prim, c, pt, t);
            let pos = match arc_seq {
                Some(k) => {
                    if frac < 0.5 {
                        k
                    } else {
                        k + 1
                    }
                }
                None => after,
            };
            out.push((pos, pi as f64 + frac, a, sg));
        }
        if let Some(k) = arc_seq {
            after = k + 1;
        }
    }
    out.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)).then(x.2.cmp(&y.2)));
    let mut seen = HashSet::new();
    out.retain(|x| seen.insert(x.2));
    Ok(out)
}

/// Fix-up loop (21 §4, 11 §4, 25 §2 step 3), ≤ `TAUT_PASS_CAP` passes.
pub fn tauten(
    start: Vec2,
    goal: Vec2,
    seq: Vec<AnchorRef>,
    env: &dyn Env,
) -> Result<Chain, TautError> {
    let mut seq = seq;
    for _ in 0..TAUT_PASS_CAP {
        if seq.len() > MAX_SLEEVE {
            return Err(TautError::Sleeve(format!("{} anchors", seq.len())));
        }
        let chain = chain_geometry(start, goal, &seq, env)?;
        let mut cur = chain.seq.clone();
        if drop_pass(start, goal, &mut cur, env) {
            seq = cur;
            continue;
        }
        let adds = add_pass(&chain, env)?;
        if adds.is_empty() {
            return Ok(chain);
        }
        let mut next = Vec::with_capacity(cur.len() + adds.len());
        for i in 0..=cur.len() {
            for &(pos, _, a, sigma) in &adds {
                if pos == i {
                    next.push(AnchorRef { anchor: a, sigma });
                }
            }
            if i < cur.len() {
                next.push(cur[i]);
            }
        }
        seq = next;
    }
    Err(TautError::Cap)
}
