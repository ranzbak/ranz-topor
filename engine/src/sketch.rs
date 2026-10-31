//! Funnel and sketch (M1 / T1.2c; 11 §2, 21 §5; design note `m1-sketch-design.md`).
//!
//! A *sketch* is the homotopy class of a hand wire: the CDT edges its polyline crosses,
//! reduced, then pulled taut with the funnel algorithm over the CDT vertices. The interior
//! funnel vertices (anchors) with their side `σ` are what the taut smoother keeps.
//! Everything is O(k) per wire (k = sleeve length <= `MAX_SLEEVE`); no randomness and no
//! order-dependent `HashMap` iteration.

use crate::cdt::{Cdt, Crossing};
use crate::geom::{Aabb, Orient, Vec2};
use crate::obstacle::{LayerObstacles, ObsKind};
use crate::wires::Wire;
use std::collections::{HashMap, HashSet, VecDeque};

/// Frame margin lower bound (nm): 10 mm.
const FRAME_MARGIN_MIN: f64 = 10_000_000.0;
/// Retry translations (nm) for a hand lying exactly on a triangulation edge.
const RETRY_SHIFTS: [(f64, f64); 2] = [(1.0, 2.0), (-2.0, 1.0)];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AnchorRef {
    /// Index into `LayerObstacles.anchors`.
    pub anchor: usize,
    /// `Ccw`: obstacle on the left of the wire; `Cw`: on the right.
    pub sigma: Orient,
}

#[derive(Clone, Debug)]
pub struct Sketch {
    pub wire: usize,
    /// Interior anchors in path order.
    pub seq: Vec<AnchorRef>,
    /// Reduced crossings.
    pub crossings: Vec<Crossing>,
}

/// Which walls become CDT constraints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintSet {
    /// Board outline only (the smoother).
    EdgeOnly,
    /// Outline, pad hulls, kept tracks, copper shapes and keepouts (the router).
    Routing,
}

impl ConstraintSet {
    fn includes(self, k: ObsKind) -> bool {
        match self {
            ConstraintSet::EdgeOnly => k == ObsKind::Edge,
            ConstraintSet::Routing => matches!(
                k,
                ObsKind::Edge | ObsKind::Pad | ObsKind::Track | ObsKind::Shape | ObsKind::Keepout
            ),
        }
    }
}

/// `(kind, net, tie_nets, label)` of the wall that owns a constraint edge.
pub type ConstraintOwner = (ObsKind, i32, Vec<i32>, String);

pub struct LayerSketch {
    pub cdt: Cdt,
    /// Constraint edge (canonical `(min, max)`) -> owning wall; every constraint piece has one
    /// (split pieces inherit the wall they lie on; the first wall in wall order wins).
    pub constraint_owner: HashMap<(usize, usize), ConstraintOwner>,
    /// Indexed by canonical CDT vertex; `None` for terminals and frame corners.
    pub point_to_anchor: Vec<Option<usize>>,
    /// Exact position -> first input index at that position.
    vertex_of: HashMap<(u64, u64), usize>,
    frame: [usize; 4],
}

fn key(p: Vec2) -> (u64, u64) {
    // `+ 0.0` folds -0.0 into 0.0
    ((p.x + 0.0).to_bits(), (p.y + 0.0).to_bits())
}

/// Per layer: CDT points = anchor centres, then `terminals`, then 4 frame corners;
/// constraints = the walls selected by `cons` (design note §1); crossing constraints are split.
pub fn build_layer_cdt(
    obs: &LayerObstacles,
    terminals: &[Vec2],
    cons: ConstraintSet,
) -> Result<LayerSketch, String> {
    build_layer_cdt_masked(obs, terminals, cons, &|_| true)
}

/// [`build_layer_cdt`] over the anchors `keep` selects only; `point_to_anchor` still holds
/// indices into `obs.anchors`. The router builds the sketch triangulation of the real obstacles
/// this way, without the dormant via-site anchors (free vertices bend no wire, M3 §5).
/// Wall end points must be kept. O(N log N).
pub fn build_layer_cdt_masked(
    obs: &LayerObstacles,
    terminals: &[Vec2],
    cons: ConstraintSet,
    keep: &dyn Fn(usize) -> bool,
) -> Result<LayerSketch, String> {
    let na = obs.anchors.len();
    let mut pt_of_anchor: Vec<Option<usize>> = vec![None; na];
    let mut anchor_of_pt: Vec<usize> = Vec::new();
    let mut pts: Vec<Vec2> = Vec::new();
    for (i, a) in obs.anchors.iter().enumerate() {
        if keep(i) {
            pt_of_anchor[i] = Some(pts.len());
            anchor_of_pt.push(i);
            pts.push(a.c);
        }
    }
    pts.extend_from_slice(terminals);
    if pts.is_empty() {
        return Err("cdt: no points".into());
    }
    let (mut lo, mut hi) = (pts[0], pts[0]);
    for p in &pts {
        lo = Vec2 {
            x: lo.x.min(p.x),
            y: lo.y.min(p.y),
        };
        hi = Vec2 {
            x: hi.x.max(p.x),
            y: hi.y.max(p.y),
        };
    }
    let m = FRAME_MARGIN_MIN.max(hi.x - lo.x).max(hi.y - lo.y);
    let nf = pts.len();
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        pts.push(Vec2 {
            x: if sx < 0.0 { lo.x - m } else { hi.x + m },
            y: if sy < 0.0 { lo.y - m } else { hi.y + m },
        });
    }
    let cons_pairs: Vec<(usize, usize)> = obs
        .walls
        .iter()
        .filter(|w| cons.includes(w.kind))
        .filter_map(|w| Some((pt_of_anchor[w.ends[0]]?, pt_of_anchor[w.ends[1]]?)))
        .collect();
    let cdt = Cdt::build(&pts, &cons_pairs).map_err(|e| format!("cdt: {e}"))?;
    let mut point_to_anchor = vec![None; cdt.num_vertices()];
    for (pi, &ai) in anchor_of_pt.iter().enumerate() {
        let v = cdt.vertex_index(pi);
        if point_to_anchor[v].is_none() {
            point_to_anchor[v] = Some(ai);
        }
    }
    let constraint_owner = constraint_owners(obs, &cdt, cons);
    let mut vertex_of = HashMap::with_capacity(pts.len());
    for (i, p) in pts.iter().enumerate() {
        vertex_of.entry(key(*p)).or_insert(i);
    }
    let frame = [0, 1, 2, 3].map(|j| cdt.vertex_index(nf + j));
    Ok(LayerSketch {
        cdt,
        constraint_owner,
        point_to_anchor,
        vertex_of,
        frame,
    })
}

impl LayerSketch {
    /// Canonical CDT vertex at exactly position `p` (an input point).
    pub fn vertex_at(&self, p: Vec2) -> Option<usize> {
        self.vertex_of
            .get(&key(p))
            .map(|&i| self.cdt.vertex_index(i))
    }
}

/// Tolerance (nm) for "lies on a wall chord".
const OWNER_TOL: f64 = 1.0;

/// Owner of every constraint edge: the first wall (wall order) whose chord contains both
/// endpoints within `OWNER_TOL` (geometric, because split vertices are not exactly on the chord).
fn constraint_owners(
    obs: &LayerObstacles,
    cdt: &Cdt,
    cons: ConstraintSet,
) -> HashMap<(usize, usize), ConstraintOwner> {
    let mut out = HashMap::new();
    for (u, v) in cdt.constraint_edges() {
        let (pu, pv) = (cdt.vertex_pos(u), cdt.vertex_pos(v));
        let bb = Aabb {
            min: Vec2 {
                x: pu.x.min(pv.x),
                y: pu.y.min(pv.y),
            },
            max: Vec2 {
                x: pu.x.max(pv.x),
                y: pu.y.max(pv.y),
            },
        }
        .grow(OWNER_TOL);
        for wi in obs.query_walls(bb) {
            let w = &obs.walls[wi];
            if !cons.includes(w.kind) {
                continue;
            }
            let (a, b) = (obs.anchors[w.ends[0]].c, obs.anchors[w.ends[1]].c);
            let d = b - a;
            let len2 = d.x * d.x + d.y * d.y;
            if len2 == 0.0 {
                continue;
            }
            let len = len2.sqrt();
            let on = |p: Vec2| {
                let q = p - a;
                let t = (q.x * d.x + q.y * d.y) / len2;
                (-1e-9..=1.0 + 1e-9).contains(&t) && d.cross(q).abs() / len <= OWNER_TOL
            };
            if on(pu) && on(pv) {
                out.insert((u, v), (w.kind, w.net, w.tie_nets.clone(), w.label.clone()));
                break;
            }
        }
    }
    out
}

fn cross3(o: Vec2, a: Vec2, b: Vec2) -> f64 {
    (a - o).cross(b - o)
}

/// Chain entry: position, portal index, on the left chain.
type Ent = (Vec2, usize, bool);

/// Prediction for `prev` of the last chain element: the one before it, or the apex.
fn prev_of(ch: &VecDeque<Ent>, apex: Vec2) -> Vec2 {
    let n = ch.len();
    if n >= 2 {
        ch[n - 2].0
    } else {
        apex
    }
}

/// Funnel (11 §2 step 4, Lee-Preparata) over portals `(left, right)` seen in the direction of
/// travel; returns interior vertices `(portal index, is_left, σ)`. A left bend has the
/// obstacle on the left (`Ccw`), a right bend on the right (`Cw`). Each portal vertex is pushed
/// once and popped at most once: O(k). A portal vertex exactly on the string (a collinear tie,
/// Fix-B1 (c)) is kept on its chain and becomes a bend on its portal side: the slot path passes
/// it on that side, and an inflated anchor on the string is never slack.
pub fn funnel(start: Vec2, goal: Vec2, portals: &[(Vec2, Vec2)]) -> Vec<(usize, bool, Orient)> {
    let k = portals.len();
    let mut out = Vec::new();
    let mut apex = start;
    let mut left: VecDeque<Ent> = VecDeque::new();
    let mut right: VecDeque<Ent> = VecDeque::new();
    let mut steps = 0usize;
    let cap = 8 * (k + 1) + 8;
    for (i, &(l, r)) in portals
        .iter()
        .chain(std::iter::once(&(goal, goal)))
        .enumerate()
    {
        // right vertex
        if r != apex && right.back().is_none_or(|e| e.0 != r) {
            while !right.is_empty()
                && cross3(prev_of(&right, apex), right.back().unwrap().0, r) > 0.0
            {
                right.pop_back();
                steps += 1;
            }
            if right.is_empty() {
                while let Some(&(lp, li, _)) = left.front() {
                    if li == k || cross3(apex, lp, r) < 0.0 || steps > cap {
                        break;
                    }
                    apex = lp;
                    out.push((li, true, Orient::Ccw));
                    left.pop_front();
                    steps += 1;
                }
            }
            right.push_back((r, i, false));
        }
        // left vertex
        if l != apex && left.back().is_none_or(|e| e.0 != l) {
            while !left.is_empty() && cross3(prev_of(&left, apex), left.back().unwrap().0, l) < 0.0
            {
                left.pop_back();
                steps += 1;
            }
            if left.is_empty() {
                while let Some(&(rp, ri, _)) = right.front() {
                    if ri == k || cross3(apex, rp, l) > 0.0 || steps > cap {
                        break;
                    }
                    apex = rp;
                    out.push((ri, false, Orient::Cw));
                    right.pop_front();
                    steps += 1;
                }
            }
            left.push_back((l, i, true));
        }
    }
    debug_assert!(steps <= cap);
    out
}

fn is_frame_edge(ls: &LayerSketch, c: &Crossing) -> bool {
    ls.frame.contains(&c.left) && ls.frame.contains(&c.right)
}

/// Hand polyline -> crossings -> reduce -> funnel -> `AnchorRef`s (design note §2).
pub fn sketch_wire(wire: &Wire, ls: &LayerSketch) -> Result<Sketch, String> {
    let mut hand: Vec<Vec2> = Vec::with_capacity(wire.hand.len());
    for &p in &wire.hand {
        if hand.last() != Some(&p) {
            hand.push(p);
        }
    }
    if hand.len() < 2 {
        return Err("hand has fewer than two distinct points".into());
    }
    let n = hand.len();
    let is_ = *ls
        .vertex_of
        .get(&key(hand[0]))
        .ok_or("hand start is not a CDT vertex")?;
    let ig = *ls
        .vertex_of
        .get(&key(hand[n - 1]))
        .ok_or("hand goal is not a CDT vertex")?;
    if ls.cdt.vertex_index(is_) == ls.cdt.vertex_index(ig) {
        return Err("hand start and goal coincide".into());
    }
    if hand[1..n - 1]
        .iter()
        .any(|p| ls.vertex_of.contains_key(&key(*p)))
    {
        return Err("hand vertex on CDT vertex".into());
    }

    let raw = if n == 2 && ls.cdt.is_edge(is_, ig) {
        Vec::new()
    } else {
        let mut res = ls.cdt.crossings(&hand);
        for (dx, dy) in RETRY_SHIFTS {
            let retry = matches!(&res, Err(e) if e.contains("lies on a triangulation edge") || e.contains("overlaps"));
            if !retry || n < 3 {
                break;
            }
            let shifted: Vec<Vec2> = hand
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    if i == 0 || i == n - 1 {
                        *p
                    } else {
                        Vec2 {
                            x: p.x + dx,
                            y: p.y + dy,
                        }
                    }
                })
                .collect();
            res = ls.cdt.crossings(&shifted);
        }
        res?
    };
    let crossings = Cdt::reduce(&raw);
    if crossings.iter().any(|c| is_frame_edge(ls, c)) {
        return Err("hand leaves frame".into());
    }
    Ok(finish_sketch(wire.id, hand[0], hand[n - 1], crossings, ls))
}

/// Funnel bends `(portal index, is_left, σ)` of reduced `crossings` between `start` and `goal`.
fn bends(
    start: Vec2,
    goal: Vec2,
    crossings: &[Crossing],
    ls: &LayerSketch,
) -> Vec<(usize, bool, Orient)> {
    let portals: Vec<(Vec2, Vec2)> = crossings
        .iter()
        .map(|c| (ls.cdt.vertex_pos(c.left), ls.cdt.vertex_pos(c.right)))
        .collect();
    funnel(start, goal, &portals)
}

fn finish_sketch(
    wire: usize,
    start: Vec2,
    goal: Vec2,
    crossings: Vec<Crossing>,
    ls: &LayerSketch,
) -> Sketch {
    let seq = bends(start, goal, &crossings, ls)
        .into_iter()
        .filter_map(|(i, is_left, sigma)| {
            let v = if is_left {
                crossings[i].left
            } else {
                crossings[i].right
            };
            ls.point_to_anchor[v].map(|anchor| AnchorRef { anchor, sigma })
        })
        .collect();
    Sketch {
        wire,
        seq,
        crossings,
    }
}

/// Router entry: crossings of a slot path (steps 5-6 of `sketch_wire`: reduce, funnel, anchors).
pub fn sketch_from_crossings(
    wire: usize,
    start: Vec2,
    goal: Vec2,
    crossings: Vec<Crossing>,
    ls: &LayerSketch,
) -> Sketch {
    finish_sketch(wire, start, goal, Cdt::reduce(&crossings), ls)
}

/// Positions of funnel bends at vertices without an anchor (split vertices of crossing
/// constraints; ledger A4). `sketch_from_crossings` drops such bends, which would straighten the
/// path through the wall; the router fails the connection instead (`ConstraintSet::Routing`).
pub fn unanchored_bends(
    start: Vec2,
    goal: Vec2,
    crossings: &[Crossing],
    ls: &LayerSketch,
) -> Vec<Vec2> {
    let red = Cdt::reduce(crossings);
    bends(start, goal, &red, ls)
        .into_iter()
        .filter_map(|(i, is_left, _)| {
            let v = if is_left { red[i].left } else { red[i].right };
            ls.point_to_anchor[v]
                .is_none()
                .then(|| ls.cdt.vertex_pos(v))
        })
        .collect()
}

/// Stack order per `(anchor, σ)` from crossings on shared edges (21 §5): candidates are wires
/// whose reduced crossings contain an edge incident to the anchor with the anchor on side σ;
/// key = fraction from the anchor along the first such edge, ties by wire id.
pub fn stack_order(sketches: &[Sketch], ls: &LayerSketch) -> HashMap<AnchorRef, Vec<usize>> {
    let mut keyed: HashMap<AnchorRef, Vec<(f64, usize)>> = HashMap::new();
    let mut seen: HashSet<AnchorRef> = HashSet::new();
    for s in sketches {
        seen.clear();
        for c in &s.crossings {
            for (v, sigma, d) in [(c.left, Orient::Ccw, c.t), (c.right, Orient::Cw, 1.0 - c.t)] {
                if let Some(anchor) = ls.point_to_anchor[v] {
                    let r = AnchorRef { anchor, sigma };
                    if seen.insert(r) {
                        keyed.entry(r).or_default().push((d, s.wire));
                    }
                }
            }
        }
    }
    keyed
        .into_iter()
        .map(|(r, mut v)| {
            v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            (r, v.into_iter().map(|e| e.1).collect())
        })
        .collect()
}
