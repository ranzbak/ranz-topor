//! Per-layer routing graph on the CDT (M2 / T2.1; docs 10 §3-4, design note
//! `m2-search-design.md`): vertices with radius/clearance, edges with cut capacity and an
//! ordered list of crossing slots, faces with an inside flag.
//!
//! Capacity model (conservative, 10 §4): `cap(e) = |pq| - r_p - r_q`, `c_e = max(c_p, c_q)`.
//! Every connection crossing `e` occupies a *slot* in `GEdge::cross`, ordered `a -> b`.
//! Build is O(V) after the CDT; `insert` / `remove_conn` cost O(n_max) per touched edge
//! (`n_max` = entries on one edge). No `HashMap` iteration reaches any output.

use crate::cdt::Cdt;
use crate::geom::{Aabb, Grid, Prim, Vec2};
use crate::obstacle::{LayerObstacles, ObsKind};
use crate::sketch::LayerSketch;
use std::collections::{BTreeMap, HashMap};

/// Cell size (nm) of the vertex grid.
const VERT_CELL: f64 = 5_000_000.0;

/// Largest chord sagitta (nm) of the pieces an arc wall is split into by the wall term (the
/// sagitta is added to the blocking radius, so the split is conservative).
const ARC_SAG_NM: f64 = 10_000.0;

/// A wall whose core passes this close (nm) to a vertex's centre belongs to that vertex's
/// item (or is the wall a split vertex lies on): the transit pinch skips it.
const TOUCH_NM: f64 = 1_000.0;

#[derive(Clone, Debug)]
pub struct VertexInfo {
    pub at: Vec2,
    /// Index into `LayerObstacles.anchors`; `None` for terminals, split vertices, frame corners.
    pub anchor: Option<usize>,
    pub owner: Option<String>,
    /// Anchor net plus tie nets (net 0 excluded), sorted.
    pub nets: Vec<i32>,
    /// Core radius (0 for non-anchors).
    pub r: f64,
    /// Vertex clearance.
    pub clr: f64,
    /// Via-site index when the vertex is a via site (`r 0`, `clr 0`, `nets []`, `owner None`
    /// while the site is unowned; see [`RouteGraph::set_site`]).
    pub site: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CrossEntry {
    pub conn: usize,
    pub width: f64,
    pub clr: f64,
    /// Index of this crossing in its connection's path (`insert_path`); `None` for plain
    /// `insert` (capacity-only, `Port::Detached`).
    pub k: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct GEdge {
    /// `a < b`.
    pub a: usize,
    pub b: usize,
    /// `faces[0]` is the face on the left of `a -> b`, `faces[1]` on the right; `None` outside the
    /// triangulation hull.
    pub faces: [Option<usize>; 2],
    pub constraint: bool,
    pub owner_nets: Vec<i32>,
    pub cap: f64,
    pub c_e: f64,
    /// `|ab|` (centre to centre), the `L` of the own-net capacity rule ([`RouteGraph::fits_for`]).
    pub len: f64,
    /// `[a, b]` is a free (dormant via-site) vertex: nothing is there to keep clear of, so the
    /// end gap of that side is 0 in [`RouteGraph::fits_at`], [`RouteGraph::overuse`] and the
    /// static fit of the search (M3).
    pub free: [bool; 2],
    /// Crossing entries ordered `a -> b`.
    pub cross: Vec<CrossEntry>,
    /// Connection whose wire runs along this edge (empty crossing path between its terminals).
    pub along: Option<usize>,
}

impl GEdge {
    /// Ends of the edge that need a clearance gap: 2 minus the free (dormant site) ends. O(1).
    pub fn end_gaps(&self) -> usize {
        2 - self.free.iter().filter(|&&f| f).count()
    }
}

/// A dormant via-site vertex: no copper, no clearance (`set_site(None)`).
fn is_free(v: &VertexInfo) -> bool {
    v.site.is_some() && v.owner.is_none()
}

/// Net of a strand whose connection never told the graph its net (plain `insert`,
/// `insert_path` without a net): never equal to a connection's net, so it is always charged
/// as a foreign strand (M3 behaviour).
pub const UNKNOWN_NET: i32 = i32::MIN;

/// Which capacity rule [`RouteGraph::fits_for`] / [`RouteGraph::overuse`] apply (M4-D1,
/// `topor.toml [expert] own_net_capacity` / `wall_aware_capacity`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapacityMode {
    /// Own-net ends charge no gap and give the core radius back; same-net strands charge no
    /// gap between each other. `false` = the net-blind M3 rule ([`RouteGraph::fits_at`]).
    pub own_net: bool,
    /// With an own end, foreign walls that are not edge ends (outline, keepouts, long pad
    /// sides) remove the part of the edge they block (`W_n(e)`). `false` = the withdrawn F6
    /// behaviour (outline-wall corridors accepted, taut fails).
    pub wall_aware: bool,
    /// Transit pinch ([`RouteGraph::transit_fits`]): a face transit around a vertex that a
    /// foreign wall pinches inside the face's wedge (a via next to the board edge, whose
    /// edges reach only the outline corners) is refused by the slot search, where taut would
    /// report the wall grazed mid-span.
    pub transit: bool,
}

impl Default for CapacityMode {
    fn default() -> Self {
        CapacityMode {
            own_net: true,
            wall_aware: true,
            transit: true,
        }
    }
}

/// The parts of a wall the wall term of the capacity rule needs (copied from
/// `LayerObstacles.walls` at build: walls are static for the life of a graph).
#[derive(Clone, Debug)]
struct WallLite {
    core: crate::obstacle::Core,
    r_core: f64,
    kind: ObsKind,
    own_clr: f64,
    net: i32,
    tie_nets: Vec<i32>,
    /// `only_nets` of the wall's first end anchor (DRC spots; empty = every net).
    only_nets: Vec<i32>,
    /// Anchor indices of the wall's two ends.
    ends: [usize; 2],
}

/// Per edge, the wall candidates of the wall term (design note §1 `walls_near`): walls whose
/// core box meets the edge's box grown by the vertex radius bound and the obstacle query
/// reach (the wall term skips the walls ending at `a` or `b`; the transit pinch uses them).
/// CSR layout, sorted per edge, static; shared between graph clones.
#[derive(Debug, Default)]
struct WallsNear {
    walls: Vec<WallLite>,
    /// `idx[start[e]..start[e + 1]]` are the wall indices of edge `e`.
    start: Vec<u32>,
    idx: Vec<u32>,
}

/// End terms and demand of the own-net capacity rule for one new strand on one edge (see
/// [`RouteGraph::fit_parts`]). Parameters are along `a -> b`, `0` at `a`, `len` at `b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitParts {
    /// Usable sub-segment `[E_a - K_a, L - E_b + K_b]`.
    pub lo: f64,
    pub hi: f64,
    /// `Σ_x (w_x + ε + g(x)) + w + ε`.
    pub need: f64,
    /// An own end gave its gap (and radius) back: the wall term applies.
    pub wall: bool,
    /// Connection clearance the wall term charges: `max(c, c_x)` over the new and the
    /// existing strands (each strand of the stack keeps its clearance to the walls).
    pub c_wall: f64,
}

/// Blocked parameter interval of the line `p + u t` against the stadium of radius `rr` around
/// the segment `s0 s1` (a disc when `s0 == s1`): the stadium is convex, so its intersection
/// with the line is the hull of the end-disc and band intersections. `None` when disjoint. O(1).
fn stadium_interval(p: Vec2, u: Vec2, s0: Vec2, s1: Vec2, rr: f64) -> Option<(f64, f64)> {
    let disc = |q: Vec2| -> Option<(f64, f64)> {
        let d = p - q;
        let b = d.dot(u);
        let disc = b * b - d.len2() + rr * rr;
        (disc > 0.0).then(|| {
            let s = disc.sqrt();
            (-b - s, -b + s)
        })
    };
    let mut out: Option<(f64, f64)> = None;
    let mut add = |iv: Option<(f64, f64)>| {
        if let Some((a, b)) = iv {
            out = Some(match out {
                None => (a, b),
                Some((x, y)) => (x.min(a), y.max(b)),
            });
        }
    };
    add(disc(s0));
    let sv = s1 - s0;
    let ls = sv.len();
    if ls > 0.0 {
        add(disc(s1));
        let v = sv * (1.0 / ls);
        let n = v.perp();
        // f(t) = f0 + f1 t inside (lo, hi): the t-interval (all / nothing when f1 ~ 0)
        let clip = |f0: f64, f1: f64, lo: f64, hi: f64| -> Option<(f64, f64)> {
            if f1.abs() < 1e-12 {
                return (f0 > lo && f0 < hi).then_some((f64::NEG_INFINITY, f64::INFINITY));
            }
            let (t0, t1) = ((lo - f0) / f1, (hi - f0) / f1);
            Some((t0.min(t1), t0.max(t1)))
        };
        let d = p - s0;
        if let (Some(x), Some(y)) = (
            clip(d.dot(n), u.dot(n), -rr, rr),
            clip(d.dot(v), u.dot(v), 0.0, ls),
        ) {
            let (a, b) = (x.0.max(y.0), x.1.min(y.1));
            if a < b {
                add(Some((a, b)));
            }
        }
    }
    out
}

#[derive(Clone, Debug)]
pub struct Face {
    /// Counter-clockwise.
    pub v: [usize; 3],
    /// `e[i]` is the edge opposite `v[i]`.
    pub e: [usize; 3],
    pub inside: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct GraphRules {
    pub eps: f64,
    pub min_clearance: f64,
    pub edge_clearance: f64,
    pub hole_clearance: f64,
}

/// Terminal vertices that are not anchors: (position, net, owner label) — pad centres of
/// non-round pads, junctions.
#[derive(Clone, Debug)]
pub struct TerminalInfo {
    pub at: Vec2,
    pub net: i32,
    pub owner: String,
    /// Clearance of the terminal's net rule, `max(net clearance, min_clearance)`.
    pub clr: f64,
    /// Core radius: `width / 2` for a `Copper` / `Dangling` junction, 0 for pad / via centres.
    pub r: f64,
}

/// One end of a strand inside a face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Port {
    Vertex(usize),
    /// Crossing at `(edge, index in cross)`.
    Cross(usize, usize),
    /// Entry from a plain `insert`: capacity only, no topology.
    Detached,
}

#[derive(Clone, Debug)]
struct PathRec {
    start: usize,
    goal: usize,
    faces: Vec<usize>,
    along: Option<usize>,
}

/// `(crossings (edge, slot), faces, (start, goal))` of an inserted path.
pub type PathView<'a> = (&'a [(usize, usize)], &'a [usize], (usize, usize));

#[derive(Clone)]
pub struct RouteGraph {
    pub verts: Vec<VertexInfo>,
    pub edges: Vec<GEdge>,
    pub faces: Vec<Face>,
    pub rules: GraphRules,
    edge_of: HashMap<(usize, usize), usize>,
    faces_of_vertex: Vec<Vec<usize>>,
    /// Per connection: `(edge, slot)`; in travel order for `insert_path`.
    occupancy: Vec<Vec<(usize, usize)>>,
    paths: Vec<Option<PathRec>>,
    vgrid: Grid<usize>,
    rho_max: f64,
    /// Per face: incentre (centre of the largest inscribed disk; degenerate faces: centroid).
    pub incentre: Vec<Vec2>,
    /// Per face `f`, the vertex part of the clear radius: distance from `incentre[f]` to the
    /// nearest vertex that is not a corner of `f`, capped at `2 * longest edge(f)` (the cap
    /// only lowers it, so it stays a lower bound); `-1` for a degenerate face. Static: vertex
    /// positions never move; a vertex's *extent* (`r + clr`, which `set_site` can raise) is
    /// charged at query time through [`RouteGraph::rho_max`].
    pub clear_r: Vec<f64>,
    /// Per face `f`, the wall part of the clear radius: the minimum over every wall `w` that is
    /// not one of the face's own walls (see `face_walls`) of
    /// `dist(incentre[f], core(w)) - r_core(w) - static_clearance(w)`, capped like `clear_r`
    /// (`static_clearance` = the wall's clearance with a zero connection clearance). A wire of
    /// half-width `w/2` and clearance `c` needs at most `r_core + static_clearance + c + w/2 +
    /// eps` from the core, so the disk of radius `clear_w - (c + w/2 + eps)` around the incentre
    /// is free of every non-face wall. `-1` for a degenerate face.
    pub clear_w: Vec<f64>,
    /// Per face: the walls both of whose end anchors are corners of the face (a wall along one
    /// of its edges, a pad outline on its corner); excluded from `clear_w`, to be tested
    /// exactly whenever a scan of the face is skipped. Sorted.
    pub face_walls: Vec<Vec<usize>>,
    /// Per edge: value of `clock` at the last change that can affect a path through the edge
    /// (P4): `insert_path` / `remove_conn` bump every edge of every face the path touches
    /// (plus the `along` edge and its two faces); `set_site` bumps every edge of every face
    /// around the vertex; a plain `insert` bumps its edge.
    pub edge_version: Vec<u64>,
    /// Version counter of this layer graph (`edge_version` values are `<= clock`).
    pub clock: u64,
    /// Capacity rule (M4-D1); default own-net + wall-aware + transit pinch.
    pub cap_mode: CapacityMode,
    /// Per strand id (the `conn` of `insert_path` / `insert`): its net, [`UNKNOWN_NET`] when the
    /// caller did not give one.
    strand_net: Vec<i32>,
    /// Wall candidates of the wall term and the transit pinch, per edge (static, shared
    /// between clones).
    walls_near: std::sync::Arc<WallsNear>,
}

fn key(p: Vec2) -> (u64, u64) {
    ((p.x + 0.0).to_bits(), (p.y + 0.0).to_bits())
}

pub(crate) fn chord(p: &Prim) -> (Vec2, Vec2) {
    match p {
        Prim::Seg(s) => (s.a, s.b),
        Prim::Arc(a) => (a.point_at(0.0), a.point_at(1.0)),
    }
}

/// Ray-casting parity of `p` against the outline chords; empty outline => inside.
pub(crate) fn inside_outline(p: Vec2, chords: &[(Vec2, Vec2)]) -> bool {
    if chords.is_empty() {
        return true;
    }
    let mut inside = false;
    for &(a, b) in chords {
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if x > p.x {
                inside = !inside;
            }
        }
    }
    inside
}

fn vertex_clr(kind: ObsKind, own_clr: f64, r: &GraphRules) -> f64 {
    match kind {
        ObsKind::Hole => r.hole_clearance,
        ObsKind::Edge => r.edge_clearance,
        ObsKind::Keepout => 0.0,
        ObsKind::Pad | ObsKind::Via | ObsKind::Track | ObsKind::Shape | ObsKind::Terminal => {
            own_clr.max(r.min_clearance)
        }
    }
}

fn sorted_nets(net: i32, tie: &[i32]) -> Vec<i32> {
    let mut v: Vec<i32> = std::iter::once(net)
        .chain(tie.iter().copied())
        .filter(|&n| n != 0)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Wall clearance with a zero connection clearance (the static part of the search's
/// `wall_clearance`).
fn static_wall_clr(kind: ObsKind, own_clr: f64, r: &GraphRules) -> f64 {
    match kind {
        ObsKind::Hole => r.hole_clearance,
        ObsKind::Edge => r.edge_clearance,
        ObsKind::Keepout => 0.0,
        _ => own_clr.max(r.min_clearance),
    }
}

/// Per-face incentre, vertex / wall clear radii and own walls (see the `RouteGraph` fields).
/// One bounded vertex-grid query and one bounded wall-grid query per face, so `O(F * q)` with
/// `q` the hits inside a box of side `~4 * longest edge` (+ the walls' reach).
type FaceClear = (Vec<Vec2>, Vec<f64>, Vec<f64>, Vec<Vec<usize>>);

#[allow(clippy::too_many_arguments)]
fn face_clearance(
    ls: &LayerSketch,
    obs: &LayerObstacles,
    rules: &GraphRules,
    verts: &[VertexInfo],
    faces: &[Face],
    vgrid: &Grid<usize>,
    faces_of_vertex: &[Vec<usize>],
    rho_max: f64,
) -> FaceClear {
    use crate::geom::{point_arc, point_seg, Seg};
    use crate::obstacle::Core;
    // anchor -> vertex, anchor -> walls ending at it
    let mut anchor_vertex: Vec<Option<usize>> = vec![None; obs.anchors.len()];
    for (v, a) in ls.point_to_anchor.iter().enumerate() {
        if let Some(a) = *a {
            if a < anchor_vertex.len() {
                anchor_vertex[a] = Some(v);
            }
        }
    }
    let mut walls_at: Vec<Vec<usize>> = vec![Vec::new(); obs.anchors.len()];
    let mut reach = 0.0f64; // largest r_core + static clearance of any wall
    let sclr: Vec<f64> = obs
        .walls
        .iter()
        .map(|w| static_wall_clr(w.kind, w.own_clr, rules))
        .collect();
    for (wi, w) in obs.walls.iter().enumerate() {
        reach = reach.max(w.r_core + sclr[wi]);
        for &e in &w.ends {
            if e < walls_at.len() && walls_at[e].last() != Some(&wi) {
                walls_at[e].push(wi);
            }
        }
    }
    let n = faces.len();
    let (mut inc, mut cv, mut cw, mut fw) = (
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
    );
    for f in faces {
        let p = [verts[f.v[0]].at, verts[f.v[1]].at, verts[f.v[2]].at];
        // side opposite corner i
        let side = [
            (p[1] - p[2]).len(),
            (p[2] - p[0]).len(),
            (p[0] - p[1]).len(),
        ];
        let per = side[0] + side[1] + side[2];
        let area2 = (p[1] - p[0]).cross(p[2] - p[0]).abs();
        if per <= 0.0 || area2 <= 1e-9 * per * per {
            inc.push((p[0] + p[1] + p[2]) * (1.0 / 3.0));
            cv.push(-1.0);
            cw.push(-1.0);
            fw.push(Vec::new());
            continue;
        }
        let c = (p[0] * side[0] + p[1] * side[1] + p[2] * side[2]) * (1.0 / per);
        let longest = side[0].max(side[1]).max(side[2]);
        // A skip needs a disk around `c` that holds the sub-chord end points (on the face's
        // edges, so `>= inradius` away and `<= longest` away) with the search's reach `rho >=
        // rho_max` to spare. Cheap reject: the nearest vertex of the faces around the corners
        // is an upper bound of the clear distance; below `inradius + rho_max` the face can
        // never skip, and `-1` (never skip) is a valid lower bound.
        let inradius = area2 / per;
        let mut ub = f64::INFINITY;
        for &v in &f.v {
            for &f2 in &faces_of_vertex[v] {
                for &u in &faces[f2].v {
                    if !f.v.contains(&u) {
                        ub = ub.min((verts[u].at - c).len());
                    }
                }
            }
        }
        if ub < inradius + rho_max {
            inc.push(c);
            cv.push(-1.0);
            cw.push(-1.0);
            fw.push(Vec::new());
            continue;
        }
        // a larger clear radius never helps: clipping it only lowers the lower bound
        let cap = longest + 2.0 * rho_max + 2_000_000.0;
        // own walls
        let mut own: Vec<usize> = Vec::new();
        for &v in &f.v {
            if let Some(a) = verts[v].anchor {
                for &wi in walls_at.get(a).map(Vec::as_slice).unwrap_or(&[]) {
                    let ok = obs.walls[wi].ends.iter().all(|&e| {
                        anchor_vertex
                            .get(e)
                            .copied()
                            .flatten()
                            .is_some_and(|ev| f.v.contains(&ev))
                    });
                    if ok {
                        own.push(wi);
                    }
                }
            }
        }
        own.sort_unstable();
        own.dedup();
        // vertex part
        let bb = Aabb { min: c, max: c }.grow(cap);
        let mut rv = cap;
        for i in vgrid.query(bb) {
            if !f.v.contains(&i) {
                rv = rv.min((verts[i].at - c).len());
            }
        }
        // wall part: a wall whose core is farther than cap + reach cannot lower the minimum
        // below `cap`
        let wb = Aabb { min: c, max: c }.grow(cap + reach);
        let mut rw = cap;
        for wi in obs.query_walls(wb) {
            if own.binary_search(&wi).is_ok() {
                continue;
            }
            let w = &obs.walls[wi];
            let d = match &w.core {
                Core::Point(q) => (*q - c).len(),
                Core::Seg(s) => point_seg(c, &Seg { a: s.a, b: s.b }),
                Core::Arc(ar) => point_arc(c, ar),
            };
            rw = rw.min(d - w.r_core - sclr[wi]);
        }
        inc.push(c);
        cv.push(rv);
        cw.push(rw);
        fw.push(own);
    }
    (inc, cv, cw, fw)
}

/// `walls_near` of every edge: one wall-grid query per edge with the edge's box grown by the larger end radius (the usable
/// sub-segment reaches past an own end) plus twice `obs.query_grow()` (a bound on every wall's
/// `rho - r_core` for any connection, twice for the transit pinch: vertex gap + width + wall
/// clearance). `O(E · q)`; sorted per edge.
fn walls_near(obs: &LayerObstacles, verts: &[VertexInfo], edges: &[GEdge]) -> WallsNear {
    let walls: Vec<WallLite> = obs
        .walls
        .iter()
        .map(|w| WallLite {
            core: w.core.clone(),
            r_core: w.r_core,
            kind: w.kind,
            own_clr: w.own_clr,
            net: w.net,
            tie_nets: w.tie_nets.clone(),
            only_nets: obs
                .anchors
                .get(w.ends[0])
                .map(|a| a.only_nets.clone())
                .unwrap_or_default(),
            ends: w.ends,
        })
        .collect();
    let grow = obs.query_grow();
    let ext = |v: &VertexInfo| {
        v.anchor
            .and_then(|ai| obs.anchors.get(ai))
            .map_or(v.r, |a| a.r_core.max(v.r))
    };
    // one independent query per edge, in parallel; collected in edge order (deterministic)
    let per_edge: Vec<Vec<usize>> = if obs.walls.is_empty() {
        vec![Vec::new(); edges.len()]
    } else {
        use rayon::prelude::*;
        edges
            .par_iter()
            .map(|ge| {
                let (va, vb) = (&verts[ge.a], &verts[ge.b]);
                let bb = Aabb {
                    min: va.at,
                    max: va.at,
                }
                .union(&Aabb {
                    min: vb.at,
                    max: vb.at,
                })
                .grow(ext(va).max(ext(vb)) + 2.0 * grow);
                obs.query_walls(bb)
            })
            .collect()
    };
    let mut start = Vec::with_capacity(edges.len() + 1);
    let mut idx: Vec<u32> = Vec::with_capacity(per_edge.iter().map(Vec::len).sum());
    for ws in per_edge {
        start.push(idx.len() as u32);
        idx.extend(ws.into_iter().map(|wi| wi as u32));
    }
    start.push(idx.len() as u32);
    WallsNear { walls, start, idx }
}

impl RouteGraph {
    pub fn build(
        ls: &LayerSketch,
        obs: &LayerObstacles,
        outline: &[Prim],
        terminals: &[TerminalInfo],
        rules: &GraphRules,
    ) -> RouteGraph {
        let cdt: &Cdt = &ls.cdt;
        let nv = cdt.num_vertices();
        let mut term_at: HashMap<(u64, u64), usize> = HashMap::new();
        for (i, t) in terminals.iter().enumerate() {
            term_at.entry(key(t.at)).or_insert(i);
        }
        let mut verts = Vec::with_capacity(nv);
        let mut vgrid = Grid::new(VERT_CELL);
        let mut rho_max = 0.0f64;
        for v in 0..nv {
            let at = cdt.vertex_pos(v);
            let anchor = ls.point_to_anchor.get(v).copied().flatten();
            let info = if let Some(ai) = anchor {
                let a = &obs.anchors[ai];
                if a.enabled {
                    VertexInfo {
                        at,
                        anchor,
                        owner: Some(a.label.clone()),
                        nets: sorted_nets(a.net, &a.tie_nets),
                        r: a.r_core,
                        clr: vertex_clr(a.kind, a.own_clr, rules),
                        site: a.site,
                    }
                } else {
                    // dormant via site: a free vertex until `set_site` activates it
                    VertexInfo {
                        at,
                        anchor,
                        owner: None,
                        nets: Vec::new(),
                        r: 0.0,
                        clr: 0.0,
                        site: a.site,
                    }
                }
            } else if let Some(&ti) = (!cdt.is_split_vertex(v))
                .then(|| term_at.get(&key(at)))
                .flatten()
            {
                let t = &terminals[ti];
                VertexInfo {
                    at,
                    anchor: None,
                    owner: Some(t.owner.clone()),
                    nets: sorted_nets(t.net, &[]),
                    r: t.r,
                    clr: t.clr.max(rules.min_clearance),
                    site: None,
                }
            } else {
                VertexInfo {
                    at,
                    anchor: None,
                    owner: None,
                    nets: Vec::new(),
                    r: 0.0,
                    clr: 0.0,
                    site: None,
                }
            };
            rho_max = rho_max.max(info.r + info.clr);
            vgrid.insert(Aabb { min: at, max: at }, v);
            verts.push(info);
        }

        let topo = cdt.topology();
        let nedges = topo.edges.len();
        let mut edges: Vec<GEdge> = Vec::with_capacity(topo.edges.len());
        let mut edge_of = HashMap::with_capacity(topo.edges.len());
        for (i, &(a, b, constraint)) in topo.edges.iter().enumerate() {
            let owner_nets = if constraint {
                match ls.constraint_owner.get(&(a, b)) {
                    Some((ObsKind::Edge | ObsKind::Keepout, ..)) | None => Vec::new(),
                    Some((_, net, tie, _)) => sorted_nets(*net, tie),
                }
            } else {
                Vec::new()
            };
            let (pa, pb) = (&verts[a], &verts[b]);
            edges.push(GEdge {
                a,
                b,
                faces: [None, None],
                constraint,
                owner_nets,
                cap: (pb.at - pa.at).len() - pa.r - pb.r,
                c_e: pa.clr.max(pb.clr),
                len: (pb.at - pa.at).len(),
                free: [is_free(pa), is_free(pb)],
                cross: Vec::new(),
                along: None,
            });
            edge_of.insert((a, b), i);
        }

        let chords: Vec<(Vec2, Vec2)> = outline.iter().map(chord).collect();
        let mut faces = Vec::with_capacity(topo.faces.len());
        let mut faces_of_vertex = vec![Vec::new(); nv];
        for (fi, &v) in topo.faces.iter().enumerate() {
            let find = |x: usize, y: usize| edge_of[&(x.min(y), x.max(y))];
            let e = [find(v[1], v[2]), find(v[2], v[0]), find(v[0], v[1])];
            for j in 0..3 {
                let (x, y) = (v[j], v[(j + 1) % 3]);
                // the face lies on the left of the directed edge x -> y
                let slot = usize::from(x > y);
                edges[find(x, y)].faces[slot] = Some(fi);
                faces_of_vertex[x].push(fi);
            }
            let c = (verts[v[0]].at + verts[v[1]].at + verts[v[2]].at) * (1.0 / 3.0);
            faces.push(Face {
                v,
                e,
                inside: inside_outline(c, &chords),
            });
        }

        let (incentre, clear_r, clear_w, face_walls) = face_clearance(
            ls,
            obs,
            rules,
            &verts,
            &faces,
            &vgrid,
            &faces_of_vertex,
            rho_max,
        );
        let walls_near = std::sync::Arc::new(walls_near(obs, &verts, &edges));
        RouteGraph {
            verts,
            edges,
            faces,
            incentre,
            clear_r,
            clear_w,
            face_walls,
            rules: *rules,
            edge_of,
            faces_of_vertex,
            occupancy: Vec::new(),
            paths: Vec::new(),
            vgrid,
            rho_max,
            edge_version: vec![0; nedges],
            clock: 0,
            cap_mode: CapacityMode::default(),
            strand_net: Vec::new(),
            walls_near,
        }
    }

    /// Stamp `edges` with a fresh version.
    fn bump(&mut self, edges: impl IntoIterator<Item = usize>) {
        self.clock += 1;
        let c = self.clock;
        for e in edges {
            self.edge_version[e] = c;
        }
    }

    fn face_edges(&self, faces: &[usize]) -> Vec<usize> {
        faces
            .iter()
            .flat_map(|&f| self.faces[f].e)
            .collect::<Vec<_>>()
    }

    /// Activate (`Some((net, r, clr, owner))`) or release (`None`) the via-site vertex `v`:
    /// set `r`, `clr`, `nets = [net]`, `owner`, recompute `cap = |ab| - r_a - r_b` and `c_e =
    /// max(clr_a, clr_b)` of the incident edges, raise `rho_max` (never lowered: it is an
    /// upper bound), bump `edge_version` of every edge of every face around `v` (P4).
    /// Returns the incident edges recomputed, sorted. O(deg).
    pub fn set_site(&mut self, v: usize, active: Option<(i32, f64, f64, String)>) -> Vec<usize> {
        debug_assert!(
            self.verts[v].site.is_some(),
            "set_site on a non-site vertex"
        );
        match active {
            Some((net, r, clr, owner)) => {
                let vi = &mut self.verts[v];
                vi.nets = vec![net];
                vi.r = r;
                vi.clr = clr;
                vi.owner = Some(owner);
                self.rho_max = self.rho_max.max(r + clr);
            }
            None => {
                let vi = &mut self.verts[v];
                vi.nets = Vec::new();
                vi.r = 0.0;
                vi.clr = 0.0;
                vi.owner = None;
            }
        }
        let around = self.faces_of_vertex[v].clone();
        let all = self.face_edges(&around);
        let mut inc: Vec<usize> = all
            .iter()
            .copied()
            .filter(|&e| self.edges[e].a == v || self.edges[e].b == v)
            .collect();
        inc.sort_unstable();
        inc.dedup();
        for &e in &inc {
            let (a, b) = (self.edges[e].a, self.edges[e].b);
            let (pa, pb) = (&self.verts[a], &self.verts[b]);
            let cap = (pb.at - pa.at).len() - pa.r - pb.r;
            let c_e = pa.clr.max(pb.clr);
            self.edges[e].cap = cap;
            self.edges[e].c_e = c_e;
            self.edges[e].free = [is_free(pa), is_free(pb)];
        }
        self.bump(all);
        inc
    }

    /// Faces whose edges a path leg `start -> goal` through `faces` depends on (P4): `faces`
    /// itself, or for a direct leg (`faces` empty) the faces of the edge `start-goal`.
    /// O(|faces|).
    pub fn leg_faces(&self, start: usize, goal: usize, faces: &[usize]) -> Vec<usize> {
        if !faces.is_empty() {
            return faces.to_vec();
        }
        self.edge(start, goal)
            .map(|e| self.edges[e].faces.iter().flatten().copied().collect())
            .unwrap_or_default()
    }

    /// True iff every edge of every face in `faces` has `edge_version <= t0`. O(|faces|).
    pub fn faces_unchanged_since(&self, faces: &[usize], t0: u64) -> bool {
        faces
            .iter()
            .all(|&f| self.faces[f].e.iter().all(|&e| self.edge_version[e] <= t0))
    }

    pub fn edge(&self, a: usize, b: usize) -> Option<usize> {
        self.edge_of.get(&(a.min(b), a.max(b))).copied()
    }

    pub fn faces_of_vertex(&self, v: usize) -> &[usize] {
        &self.faces_of_vertex[v]
    }

    /// `(left, right)` vertices of edge `e` for a path crossing it out of face `prev`: entering
    /// from the face left of `a -> b` means travelling clockwise of `a -> b`, so `b` is then on
    /// the left of the direction of travel.
    pub fn crossing_lr(&self, e: usize, prev: usize) -> (usize, usize) {
        let ge = &self.edges[e];
        if ge.faces[0] == Some(prev) {
            (ge.b, ge.a)
        } else {
            (ge.a, ge.b)
        }
    }

    /// Both faces inside the outline, no wire running along it, and (constraint edges) `net`
    /// is an owner net.
    pub fn passable(&self, e: usize, net: i32) -> bool {
        self.edges[e].along.is_none() && self.passable_static(e, net)
    }

    /// [`RouteGraph::passable`] without the dynamic part (a wire running along the edge): both
    /// faces inside the outline and, for a constraint edge, `net` an owner net. A static fact of
    /// the board (F3: terminal pre-check, `dead` set).
    pub fn passable_static(&self, e: usize, net: i32) -> bool {
        let g = &self.edges[e];
        let inside = |f: Option<usize>| f.is_some_and(|f| self.faces[f].inside);
        inside(g.faces[0]) && inside(g.faces[1]) && (!g.constraint || g.owner_nets.contains(&net))
    }

    /// A further connection of width `w`, clearance `c` still fits edge `e` (10 §4).
    pub fn fits(&self, e: usize, w: f64, c: f64) -> bool {
        self.fits_at(e, w, c, self.rules.eps)
    }

    /// [`RouteGraph::fits`] with the build margin `eps` instead of the graph's (the zero-slack
    /// retry tests a corridor at margin 0).
    pub fn fits_at(&self, e: usize, w: f64, c: f64, eps: f64) -> bool {
        let g = &self.edges[e];
        let used: f64 = g.cross.iter().map(|x| x.width + eps + x.clr.max(c)).sum();
        used + w + eps + g.end_gaps() as f64 * c.max(g.c_e) <= g.cap
    }

    /// Net of strand `conn` ([`UNKNOWN_NET`] when never given). O(1).
    pub fn strand_net(&self, conn: usize) -> i32 {
        self.strand_net.get(conn).copied().unwrap_or(UNKNOWN_NET)
    }

    /// `net` is one of the nets of vertex `v` (anchor net or tie net; an owned via site's
    /// net). A dormant site and an unowned vertex have no nets. O(log nets).
    pub fn is_own(&self, v: usize, net: i32) -> bool {
        net != UNKNOWN_NET && self.verts[v].nets.binary_search(&net).is_ok()
    }

    /// End terms and demand of the own-net capacity rule (design note
    /// `m4-capacity-steiner-design.md` §1) for a new strand of net `net`, width `w`, clearance
    /// `c` on edge `e`, at margin `eps`:
    ///
    /// - `E_v = 0` for a free end and for an own end with no foreign strand on `e`;
    ///   `r_v + max_x max(c_x, c_e)` for an own end with foreign strands `x`; `r_v + max(c, c_e)`
    ///   for any other end (the M3 end term);
    /// - `K_v = min(r_v, w_min / 2)` for an own end with `E_v = 0` (the strand may run over the
    ///   own core, but its centreline must cross the segment, so its copper reaches at most half
    ///   the narrowest strand's width past the vertex; design note: `r_v`, equal whenever
    ///   `r_v <= w/2`);
    /// - `g(x) = 0` for a same-net strand, else `max(c_x, c)`.
    ///
    /// `lo = E_a - K_a`, `hi = L - E_b + K_b`, `need = Σ_x (w_x + ε + g(x)) + w + ε`; `wall`
    /// iff an own end has `E_v = 0` (the wall term then applies). With no own end and no
    /// same-net strand `hi - lo - need` equals the M3 slack. O(n_max).
    pub fn fit_parts(&self, e: usize, net: i32, w: f64, c: f64, eps: f64) -> FitParts {
        self.fit_parts_of(e, net, w, c, eps, &self.edges[e].cross)
    }

    /// [`RouteGraph::fit_parts`] of the empty edge (no strand). O(1).
    pub fn fit_parts_empty(&self, e: usize, net: i32, w: f64, c: f64, eps: f64) -> FitParts {
        self.fit_parts_of(e, net, w, c, eps, &[])
    }

    fn fit_parts_of(
        &self,
        e: usize,
        net: i32,
        w: f64,
        c: f64,
        eps: f64,
        cross: &[CrossEntry],
    ) -> FitParts {
        let g = &self.edges[e];
        let mut need = w + eps;
        for x in cross {
            need += x.width + eps;
            if self.strand_net(x.conn) != net || net == UNKNOWN_NET {
                need += x.clr.max(c);
            }
        }
        // The copper of the strand next to an own end reaches at most half its width past the
        // vertex (its centreline must cross the segment `ab`): `K_v = min(r_v, w_min / 2)`.
        let half = cross.iter().map(|x| x.width).fold(w, f64::min) * 0.5;
        let mut wall = false;
        let mut end = |v: usize, free: bool| -> (f64, f64) {
            if free {
                return (0.0, 0.0);
            }
            let vi = &self.verts[v];
            if self.is_own(v, net) {
                let foreign_gap = cross
                    .iter()
                    .filter(|x| !self.is_own(v, self.strand_net(x.conn)))
                    .map(|x| x.clr.max(g.c_e))
                    .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x))));
                match foreign_gap {
                    Some(gap) => (vi.r + gap, 0.0),
                    None => {
                        wall = true;
                        (0.0, vi.r.min(half))
                    }
                }
            } else {
                (vi.r + c.max(g.c_e), 0.0)
            }
        };
        let (ea, ka) = end(g.a, g.free[0]);
        let (eb, kb) = end(g.b, g.free[1]);
        FitParts {
            lo: ea - ka,
            hi: g.len - eb + kb,
            need,
            wall,
            c_wall: cross.iter().map(|x| x.clr).fold(c, f64::max),
        }
    }

    /// Wall term of the capacity rule (design note §1 `W_n(e)`): the longest piece of `[lo,
    /// hi]` (parameters along `a -> b`, copper extents like the end terms) that no foreign wall
    /// of `walls_near(e)` keeps the copper of net `net`, clearance `c`, out of: a wall `W`
    /// blocks `{t : dist(p(t), core(W)) < r_W + clr(W)}`. Deviation from the note (which
    /// grows the blocked set by `w/2 + eps` and compares it with the copper demand `need`, so
    /// counts the width twice: a 0.2 mm track out of a 0.25 mm pad tip at 0.5 mm pitch, 0.275
    /// mm from each neighbour, came out unfit); `w` and `eps` are in `need` (`clr` as the
    /// search's `wall_clearance`:
    /// holes `hole_clearance`, edges `edge_clearance`, keepouts 0, else `max(own, c,
    /// min_clearance)`); walls ending at the anchor of `a` or `b` (the end terms price them),
    /// same-net / tie walls and DRC spots not listing the net are exempt; arcs are split into
    /// chords whose sagitta is added to the radius (conservative). `hi - lo` when nothing
    /// blocks. O(k log k), `k = |walls_near(e)|` (+ arc pieces).
    pub fn wall_free_len(&self, e: usize, net: i32, c: f64, lo: f64, hi: f64) -> f64 {
        use crate::obstacle::{Core, ObstacleSet};
        if hi <= lo {
            return 0.0;
        }
        let wn = &*self.walls_near;
        let (Some(&s0), Some(&s1)) = (wn.start.get(e), wn.start.get(e + 1)) else {
            return hi - lo;
        };
        let g = &self.edges[e];
        let (pa, pb) = (self.verts[g.a].at, self.verts[g.b].at);
        let Some(u) = (pb - pa).unit() else {
            return hi - lo;
        };
        let r = &self.rules;
        let ends = [self.verts[g.a].anchor, self.verts[g.b].anchor];
        let mut blocked: Vec<(f64, f64)> = Vec::new();
        for &wi in &wn.idx[s0 as usize..s1 as usize] {
            let wl = &wn.walls[wi as usize];
            if ObstacleSet::exempt(wl.net, &wl.tie_nets, net)
                || (!wl.only_nets.is_empty() && !wl.only_nets.contains(&net))
            {
                continue;
            }
            let clr = match wl.kind {
                ObsKind::Hole => r.hole_clearance,
                ObsKind::Edge => r.edge_clearance,
                ObsKind::Keepout => 0.0,
                _ => wl.own_clr.max(c).max(r.min_clearance),
            };
            if wl.ends.iter().any(|&x| ends.contains(&Some(x))) {
                continue;
            }
            let rr = wl.r_core + clr;
            let mut push = |iv: Option<(f64, f64)>| {
                if let Some((x, y)) = iv {
                    let (x, y) = (x.max(lo), y.min(hi));
                    if x < y {
                        blocked.push((x, y));
                    }
                }
            };
            match &wl.core {
                Core::Point(q) => push(stadium_interval(pa, u, *q, *q, rr)),
                Core::Seg(s) => push(stadium_interval(pa, u, s.a, s.b, rr)),
                Core::Arc(ar) => {
                    for piece in ar.split_sagitta(ARC_SAG_NM) {
                        let sag = piece.sagitta();
                        push(stadium_interval(
                            pa,
                            u,
                            piece.start(),
                            piece.end(),
                            rr + sag,
                        ));
                    }
                }
            }
        }
        if blocked.is_empty() {
            return hi - lo;
        }
        blocked.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.total_cmp(&y.1)));
        let (mut best, mut at) = (0.0f64, lo);
        for (x, y) in blocked {
            if x > at {
                best = best.max(x - at);
            }
            at = at.max(y);
        }
        best.max(hi - at)
    }

    /// Transit pinch (`cap_mode.transit`): a strand that enters face `f` across
    /// `e1` and leaves it across `e2` passes around their shared vertex `o`, inside the wedge of
    /// `f` at `o`. A foreign wall whose core reaches into that wedge closer to `o` than `r_o +
    /// gap_o + w + eps + r_W + clr(W)` leaves no room for the strand (eprom: a via 0.25 mm from
    /// the board edge, where the vertex-only edge capacity sees only the outline corner 5 mm
    /// away). `gap_o = max(c, clr_o)` (0 for a free vertex); an own `o` never pinches (the
    /// strand may run over the own copper); walls touching `o`'s centre (its own item, the wall
    /// a split vertex lies on) are skipped; same-net / tie walls exempt. Static (strands on the
    /// edges are the edge capacity's business). Candidates: `walls_near(e1) ∪ walls_near(e2)`.
    /// O(k), arcs split into chords (sagitta added).
    #[allow(clippy::too_many_arguments)]
    pub fn transit_fits(
        &self,
        f: usize,
        e1: usize,
        e2: usize,
        net: i32,
        w: f64,
        c: f64,
        eps: f64,
    ) -> bool {
        use crate::obstacle::{Core, ObstacleSet};
        if !self.cap_mode.transit {
            return true;
        }
        let (x, y) = (&self.edges[e1], &self.edges[e2]);
        let Some(o) = [x.a, x.b].into_iter().find(|&v| v == y.a || v == y.b) else {
            return true;
        };
        let ov = &self.verts[o];
        let free = ov.site.is_some() && ov.owner.is_none();
        if self.is_own(o, net) {
            return true;
        }
        let other = |g: &GEdge| if g.a == o { g.b } else { g.a };
        let (po, px, py) = (ov.at, self.verts[other(x)].at, self.verts[other(y)].at);
        let (dx, dy) = (px - po, py - po);
        let s = dx.cross(dy);
        if s.abs() < 1e-9 || !self.faces[f].v.contains(&o) {
            return true;
        }
        let sg = s.signum();
        let gap = if free { 0.0 } else { c.max(ov.clr) };
        let base = ov.r + gap + w + eps;
        let r = &self.rules;
        let wn = &*self.walls_near;
        // distance from `o` to the part of segment `a b` inside the wedge (None: outside)
        let wedge_dist = |a: Vec2, b: Vec2| -> Option<f64> {
            let (mut t0, mut t1) = (0.0f64, 1.0f64);
            for (n0, n1) in [
                (sg * dx.cross(a - po), sg * dx.cross(b - po)),
                (sg * (a - po).cross(dy), sg * (b - po).cross(dy)),
            ] {
                // keep n(t) = n0 + (n1 - n0) t >= 0
                let dn = n1 - n0;
                if dn.abs() < 1e-12 {
                    if n0 < 0.0 {
                        return None;
                    }
                } else {
                    let t = -n0 / dn;
                    if dn > 0.0 {
                        t0 = t0.max(t);
                    } else {
                        t1 = t1.min(t);
                    }
                }
            }
            if t0 > t1 {
                return None;
            }
            let (qa, qb) = (a + (b - a) * t0, a + (b - a) * t1);
            Some(crate::geom::point_seg(
                po,
                &crate::geom::Seg { a: qa, b: qb },
            ))
        };
        for &e in [e1, e2].iter() {
            let (Some(&s0), Some(&s1)) = (wn.start.get(e), wn.start.get(e + 1)) else {
                continue;
            };
            for &wi in &wn.idx[s0 as usize..s1 as usize] {
                let wl = &wn.walls[wi as usize];
                if ObstacleSet::exempt(wl.net, &wl.tie_nets, net)
                    || (!wl.only_nets.is_empty() && !wl.only_nets.contains(&net))
                {
                    continue;
                }
                let clr = match wl.kind {
                    ObsKind::Hole => r.hole_clearance,
                    ObsKind::Edge => r.edge_clearance,
                    ObsKind::Keepout => 0.0,
                    _ => wl.own_clr.max(c).max(r.min_clearance),
                };
                let need = base + wl.r_core + clr;
                let mut pieces: Vec<(Vec2, Vec2, f64)> = Vec::new();
                match &wl.core {
                    Core::Point(q) => pieces.push((*q, *q, 0.0)),
                    Core::Seg(sg2) => pieces.push((sg2.a, sg2.b, 0.0)),
                    Core::Arc(ar) => {
                        for pc in ar.split_sagitta(ARC_SAG_NM) {
                            pieces.push((pc.start(), pc.end(), pc.sagitta()));
                        }
                    }
                }
                // a wall through o's centre is o's own item (or the wall o is split on)
                let touch = pieces.iter().any(|&(a, b, sag)| {
                    crate::geom::point_seg(po, &crate::geom::Seg { a, b }) <= TOUCH_NM + sag
                });
                if touch {
                    continue;
                }
                for &(a, b, sag) in &pieces {
                    if let Some(d) = wedge_dist(a, b) {
                        if d - sag < need {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    /// A further connection of net `net`, width `w`, clearance `c` still fits edge `e` under
    /// the graph's capacity rule ([`RouteGraph::cap_mode`]), at the graph's margin. See
    /// [`RouteGraph::fits_for_at`].
    pub fn fits_for(&self, e: usize, net: i32, w: f64, c: f64) -> bool {
        self.fits_for_at(e, net, w, c, self.rules.eps)
    }

    /// Own-net / same-net capacity, wall-aware (M4-D1, design note §1): `need <= hi - lo`
    /// ([`RouteGraph::fit_parts`]) and, when an own end gave its gap back and `wall_aware` is
    /// on, `need <=` the longest wall-free piece of `[lo, hi]`
    /// ([`RouteGraph::wall_free_len`]). `own_net == false`: the net-blind M3 rule
    /// ([`RouteGraph::fits_at`]). O(n_max + k log k).
    pub fn fits_for_at(&self, e: usize, net: i32, w: f64, c: f64, eps: f64) -> bool {
        self.fits_for_with(e, net, w, c, eps, |lo, hi, cw| {
            self.wall_free_len(e, net, cw, lo, hi)
        })
    }

    /// [`RouteGraph::fits_for_at`] with the wall term supplied by the caller (`wall(lo, hi,
    /// c_wall)` = the longest wall-free piece; the search memoises it per edge). O(n_max) + the
    /// closure.
    pub fn fits_for_with(
        &self,
        e: usize,
        net: i32,
        w: f64,
        c: f64,
        eps: f64,
        wall: impl FnOnce(f64, f64, f64) -> f64,
    ) -> bool {
        if !self.cap_mode.own_net || !self.own_rule_applies(e, net, true) {
            return self.fits_at(e, w, c, eps);
        }
        self.parts_fit(&self.fit_parts(e, net, w, c, eps), wall)
    }

    /// The own-net rule can differ from M3 on `e` for net `net`: an end owned by `net` or (with
    /// `strands`) a strand of `net` on `e`. Otherwise the M3 formula is evaluated as such, so
    /// boards without the case stay bit-identical. O(n_max).
    fn own_rule_applies(&self, e: usize, net: i32, strands: bool) -> bool {
        let g = &self.edges[e];
        (!g.free[0] && self.is_own(g.a, net))
            || (!g.free[1] && self.is_own(g.b, net))
            || (strands
                && net != UNKNOWN_NET
                && g.cross.iter().any(|x| self.strand_net(x.conn) == net))
    }

    /// The empty edge `e` fits one strand of net `net` (the static fit: `!` of the search's
    /// `hard` capacity part). `own_net == false`: `cap >= w + eps + end_gaps * max(c, c_e)`
    /// (M3). O(1) + the closure.
    pub fn fits_empty_with(
        &self,
        e: usize,
        net: i32,
        w: f64,
        c: f64,
        eps: f64,
        wall: impl FnOnce(f64, f64, f64) -> f64,
    ) -> bool {
        if !self.cap_mode.own_net || !self.own_rule_applies(e, net, false) {
            let ge = &self.edges[e];
            return ge.cap >= w + eps + ge.end_gaps() as f64 * c.max(ge.c_e);
        }
        self.parts_fit(&self.fit_parts_empty(e, net, w, c, eps), wall)
    }

    fn parts_fit(&self, p: &FitParts, wall: impl FnOnce(f64, f64, f64) -> f64) -> bool {
        if p.need > p.hi - p.lo {
            return false;
        }
        !(p.wall && self.cap_mode.wall_aware) || p.need <= wall(p.lo, p.hi, p.c_wall)
    }

    /// Overuse of edge `e` under the graph's capacity rule: the strands present, in slot
    /// order, need `Σ (w_x + ε)` plus `max(c_i, c_j)` between consecutive strands of different
    /// nets (0 for the same net) plus, at each end, `r_v + max(c_s, c_e)` for the strand `s`
    /// next to it, or 0 for a free end or when `s`'s net owns `v` (then `min(r_v, w_s / 2)` is
    /// credited);
    /// `overuse = (need - L - credits).max(0)`. `own_net == false`: the net-blind M3
    /// accounting. Equal to M3 when no strand's net owns an end and no two neighbours share a
    /// net. O(n_max).
    pub fn overuse(&self, e: usize) -> f64 {
        if !self.cap_mode.own_net {
            return self.overuse_m3(e);
        }
        let g = &self.edges[e];
        let (Some(first), Some(last)) = (g.cross.first(), g.cross.last()) else {
            return 0.0;
        };
        let own_end = (!g.free[0] && self.is_own(g.a, self.strand_net(first.conn)))
            || (!g.free[1] && self.is_own(g.b, self.strand_net(last.conn)));
        let same_pair = g.cross.windows(2).any(|p| {
            let n0 = self.strand_net(p[0].conn);
            n0 != UNKNOWN_NET && n0 == self.strand_net(p[1].conn)
        });
        if !own_end && !same_pair {
            return self.overuse_m3(e);
        }
        let eps = self.rules.eps;
        let mut need: f64 = g.cross.iter().map(|x| x.width + eps).sum();
        need += g
            .cross
            .windows(2)
            .map(|p| {
                let (n0, n1) = (self.strand_net(p[0].conn), self.strand_net(p[1].conn));
                if n0 == n1 && n0 != UNKNOWN_NET {
                    0.0
                } else {
                    p[0].clr.max(p[1].clr)
                }
            })
            .sum::<f64>();
        let mut credit = 0.0;
        for (v, free, s) in [(g.a, g.free[0], first), (g.b, g.free[1], last)] {
            if free {
                continue;
            }
            let r = self.verts[v].r;
            if self.is_own(v, self.strand_net(s.conn)) {
                credit += r.min(0.5 * s.width);
            } else {
                need += r + s.clr.max(g.c_e);
            }
        }
        (need - g.len - credit).max(0.0)
    }

    /// The net-blind M3 overuse: `(need_all - cap).max(0)`.
    fn overuse_m3(&self, e: usize) -> f64 {
        let g = &self.edges[e];
        let (Some(first), Some(last)) = (g.cross.first(), g.cross.last()) else {
            return 0.0;
        };
        let eps = self.rules.eps;
        let mut need: f64 = g.cross.iter().map(|x| x.width + eps).sum();
        need += g
            .cross
            .windows(2)
            .map(|p| p[0].clr.max(p[1].clr))
            .sum::<f64>();
        if !g.free[0] {
            need += first.clr.max(g.c_e);
        }
        if !g.free[1] {
            need += last.clr.max(g.c_e);
        }
        (need - g.cap).max(0.0)
    }

    pub fn slot_count(&self, e: usize) -> usize {
        self.edges[e].cross.len()
    }

    /// Connection with a wire running along edge `e`, if any.
    pub fn along(&self, e: usize) -> Option<usize> {
        self.edges[e].along
    }

    pub fn occupancy(&self, conn: usize) -> &[(usize, usize)] {
        self.occupancy.get(conn).map_or(&[], |v| v.as_slice())
    }

    fn ensure(&mut self, conn: usize) {
        if self.occupancy.len() <= conn {
            self.occupancy.resize(conn + 1, Vec::new());
            self.paths.resize(conn + 1, None);
        }
        if self.strand_net.len() <= conn {
            self.strand_net.resize(conn + 1, UNKNOWN_NET);
        }
    }

    /// Record that entry `x` (formerly at index `old`) now sits at index `new` on edge `e`.
    fn move_entry(&mut self, e: usize, x: CrossEntry, old: usize, new: usize) {
        let occ = &mut self.occupancy[x.conn];
        let pos = match x.k {
            Some(k) => Some(k),
            None => occ.iter().position(|&p| p == (e, old)),
        };
        if let Some(p) = pos {
            occ[p] = (e, new);
        }
    }

    /// Plain insertion at `slot` (capacity-only unless `entry.k` is set): later slots shift.
    /// The occupancy list of `conn` gets `(e, slot)` appended.
    pub fn insert(&mut self, conn: usize, e: usize, slot: usize, entry: CrossEntry) {
        debug_assert_eq!(conn, entry.conn);
        self.ensure(conn);
        let slot = slot.min(self.edges[e].cross.len());
        self.bump([e]);
        self.edges[e].cross.insert(slot, entry);
        for i in (slot + 1..self.edges[e].cross.len()).rev() {
            let x = self.edges[e].cross[i];
            self.move_entry(e, x, i - 1, i);
        }
        match entry.k {
            Some(k) => {
                let occ = &mut self.occupancy[conn];
                debug_assert_eq!(occ.len(), k);
                occ.push((e, slot));
            }
            None => self.occupancy[conn].push((e, slot)),
        }
    }

    /// Insert a routed path: `crossings` are `(edge, slot)` in travel order with slots as seen
    /// by the search (the graph state before this call); `faces.len() == crossings.len() + 1`
    /// (the faces entered, start face first). An empty path runs along `edge(start_v, goal_v)`.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_path(
        &mut self,
        conn: usize,
        start_v: usize,
        goal_v: usize,
        crossings: &[(usize, usize)],
        faces: &[usize],
        width: f64,
        clr: f64,
    ) {
        self.insert_path_net(
            conn,
            UNKNOWN_NET,
            start_v,
            goal_v,
            crossings,
            faces,
            width,
            clr,
        );
    }

    /// [`RouteGraph::insert_path`] recording the strand's net (own-net / same-net capacity,
    /// [`RouteGraph::fits_for`], [`RouteGraph::overuse`]); the net is forgotten again by
    /// [`RouteGraph::remove_conn`]. Same cost as `insert_path`.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_path_net(
        &mut self,
        conn: usize,
        net: i32,
        start_v: usize,
        goal_v: usize,
        crossings: &[(usize, usize)],
        faces: &[usize],
        width: f64,
        clr: f64,
    ) {
        self.ensure(conn);
        self.strand_net[conn] = net;
        debug_assert!(self.paths[conn].is_none() && self.occupancy[conn].is_empty());
        let mut along = None;
        if crossings.is_empty() {
            along = self.edge(start_v, goal_v);
            if let Some(e) = along {
                self.edges[e].along = Some(conn);
            }
        } else {
            // `faces.len() == crossings.len()`: a junction leg (A5) whose last crossing is the
            // half crossing next to the sibling it ends on (no face after it).
            debug_assert!(faces.len() == crossings.len() + 1 || faces.len() == crossings.len());
        }
        // An edge crossed twice: the second slot refers to the state before this call.
        let mut earlier: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (k, &(e, s0)) in crossings.iter().enumerate() {
            let prev = earlier.entry(e).or_default();
            let slot = s0 + prev.iter().filter(|&&p| p <= s0).count();
            prev.push(s0);
            self.insert(
                conn,
                e,
                slot,
                CrossEntry {
                    conn,
                    width,
                    clr,
                    k: Some(k),
                },
            );
        }
        if !crossings.is_empty() && faces.len() == crossings.len() {
            // The junction half crossing overlaps the sibling's copper: zero width and gap, so
            // it keeps its place in the slot order (Rule T) without taking capacity.
            if let Some(&(e, i)) = self.occupancy[conn].last() {
                let x = &mut self.edges[e].cross[i];
                x.width = 0.0;
                x.clr = 0.0;
            }
        }
        let mut touched = self.face_edges(faces);
        if let Some(e) = along {
            let fs: Vec<usize> = self.edges[e].faces.iter().flatten().copied().collect();
            touched.extend(self.face_edges(&fs));
        }
        self.bump(touched);
        self.paths[conn] = Some(PathRec {
            start: start_v,
            goal: goal_v,
            faces: faces.to_vec(),
            along,
        });
    }

    /// The other end of the strand of entry `i` on edge `e` inside `face` (one of the two faces
    /// of `e`). `Detached` for entries from a plain `insert`.
    pub fn strand(&self, e: usize, i: usize, face: usize) -> Port {
        let x = self.edges[e].cross[i];
        let (Some(k), Some(Some(p))) = (x.k, self.paths.get(x.conn)) else {
            return Port::Detached;
        };
        let occ = &self.occupancy[x.conn];
        if p.faces.get(k) == Some(&face) {
            if k == 0 {
                Port::Vertex(p.start)
            } else {
                Port::Cross(occ[k - 1].0, occ[k - 1].1)
            }
        } else if p.faces.get(k + 1) == Some(&face) {
            if k + 1 == occ.len() {
                Port::Vertex(p.goal)
            } else {
                Port::Cross(occ[k + 1].0, occ[k + 1].1)
            }
        } else {
            Port::Detached
        }
    }

    /// `(crossings (edge, current slot), faces, (start, goal))` of a path inserted with
    /// `insert_path`.
    pub fn path_of(&self, conn: usize) -> Option<PathView<'_>> {
        let p = self.paths.get(conn)?.as_ref()?;
        Some((&self.occupancy[conn], &p.faces, (p.start, p.goal)))
    }

    /// Remove every entry of `conn`, clear its `along`, and renumber the slots of the other
    /// connections on the touched edges. O(occupancy · n_max).
    pub fn remove_conn(&mut self, conn: usize) {
        if conn >= self.occupancy.len() {
            return;
        }
        let mut touched: Vec<usize> = self.occupancy[conn].iter().map(|&(e, _)| e).collect();
        touched.sort_unstable();
        touched.dedup();
        let touched_edges = touched.clone();
        for e in touched {
            let old = std::mem::take(&mut self.edges[e].cross);
            let mut kept = Vec::with_capacity(old.len());
            for (i, x) in old.into_iter().enumerate() {
                if x.conn == conn {
                    continue;
                }
                let new = kept.len();
                kept.push(x);
                if new != i {
                    self.move_entry(e, x, i, new);
                }
            }
            self.edges[e].cross = kept;
        }
        self.occupancy[conn].clear();
        if let Some(n) = self.strand_net.get_mut(conn) {
            *n = UNKNOWN_NET;
        }
        let mut bumped = touched_edges;
        if let Some(p) = self.paths[conn].take() {
            bumped.extend(self.face_edges(&p.faces));
            if let Some(e) = p.along {
                self.edges[e].along = None;
                let fs: Vec<usize> = self.edges[e].faces.iter().flatten().copied().collect();
                bumped.extend(self.face_edges(&fs));
            }
        }
        self.bump(bumped);
    }

    /// Vertices whose position lies inside `bb`, sorted.
    pub fn verts_near(&self, bb: Aabb) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .vgrid
            .query(bb)
            .into_iter()
            .filter(|&i| {
                let p = self.verts[i].at;
                p.x >= bb.min.x && p.x <= bb.max.x && p.y >= bb.min.y && p.y <= bb.max.y
            })
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Largest `r + clr` over all vertices.
    pub fn rho_max(&self) -> f64 {
        self.rho_max
    }
}

#[cfg(test)]
mod outline_tests {
    use super::*;
    use crate::geom::{outline_prims, IVec2, TrackPrim};

    fn iv(x: i32, y: i32) -> IVec2 {
        IVec2 { x, y }
    }

    /// 40 x 40 mm body whose right edge is a 180 degree arc (r 20 mm) bulging to x = 60 mm.
    fn d_outline() -> Vec<TrackPrim> {
        let mm = 1_000_000i32;
        vec![
            TrackPrim::Seg {
                a: iv(0, 0),
                b: iv(40 * mm, 0),
            },
            TrackPrim::Arc {
                start: iv(40 * mm, 0),
                mid: iv(60 * mm, 20 * mm),
                end: iv(40 * mm, 40 * mm),
            },
            TrackPrim::Seg {
                a: iv(40 * mm, 40 * mm),
                b: iv(0, 40 * mm),
            },
            TrackPrim::Seg {
                a: iv(0, 40 * mm),
                b: iv(0, 0),
            },
        ]
    }

    #[test]
    fn inside_outline_handles_arc_pockets() {
        let mm = 1e6;
        let pocket = Vec2 {
            x: 48.0 * mm,
            y: 20.0 * mm,
        };
        let outside = Vec2 {
            x: 61.0 * mm,
            y: 20.0 * mm,
        };
        let o = d_outline();
        let collapsed: Vec<(Vec2, Vec2)> = o
            .iter()
            .map(|t| chord(&crate::geom::dequantise(t)))
            .collect();
        assert!(
            !inside_outline(pocket, &collapsed),
            "chords must misclassify the pocket"
        );
        let prims = outline_prims(&o);
        let chords: Vec<(Vec2, Vec2)> = prims.iter().map(chord).collect();
        assert!(inside_outline(pocket, &chords));
        assert!(inside_outline(
            Vec2 {
                x: 10.0 * mm,
                y: 20.0 * mm
            },
            &chords
        ));
        assert!(!inside_outline(outside, &chords));
    }
}
