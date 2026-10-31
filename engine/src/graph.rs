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

    /// `(need_all - cap).max(0)`.
    pub fn overuse(&self, e: usize) -> f64 {
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
        self.ensure(conn);
        debug_assert!(self.paths[conn].is_none() && self.occupancy[conn].is_empty());
        let mut along = None;
        if crossings.is_empty() {
            along = self.edge(start_v, goal_v);
            if let Some(e) = along {
                self.edges[e].along = Some(conn);
            }
        } else {
            debug_assert_eq!(faces.len(), crossings.len() + 1);
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
