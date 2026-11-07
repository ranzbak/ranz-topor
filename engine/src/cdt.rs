//! Constrained Delaunay triangulation wrapper (spade) and polyline crossing
//! sequences (topology docs 10, 11 §2). Coordinates are nm in `f64`.
use crate::geom::Vec2;
use spade::handles::FixedVertexHandle;
use spade::{
    ConstrainedDelaunayTriangulation, HasPosition, Intersection, LineIntersectionIterator, Point2,
    Triangulation,
};

/// Grid (nm) of the last fallback in [`Cdt::build`].
pub const SNAP_NM: f64 = 128.0;

/// Maximum number of triangulation-edge crossings per polyline (45 §5a).
pub const MAX_SLEEVE: usize = 2000;

/// One triangulation edge crossed by a polyline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crossing {
    /// Canonical vertex index on the left of the direction of travel.
    pub left: usize,
    /// Canonical vertex index on the right of the direction of travel.
    pub right: usize,
    /// Fraction from `left` to `right` of the crossing point.
    pub t: f64,
}

#[derive(Clone, Copy, Debug)]
struct V {
    pos: Point2<f64>,
}

impl HasPosition for V {
    type Scalar = f64;
    fn position(&self) -> Point2<f64> {
        self.pos
    }
}

type Tri = ConstrainedDelaunayTriangulation<V>;

/// Combinatorial view of the triangulation: sorted edges and inner faces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Topology {
    /// Canonical `(a, b)` with `a < b`, `is_constraint`; sorted.
    pub edges: Vec<(usize, usize, bool)>,
    /// Counter-clockwise vertex triples of the inner faces, each rotated so the smallest
    /// vertex comes first; sorted.
    pub faces: Vec<[usize; 3]>,
}

pub struct Cdt {
    tri: Tri,
    /// input index -> canonical (spade vertex) index
    map: Vec<usize>,
    /// Vertex count after the point loop; canonical ids `>= n_inputs` are split vertices.
    n_inputs: usize,
}

fn p2(v: Vec2) -> Point2<f64> {
    Point2::new(v.x, v.y)
}

fn v2(p: Point2<f64>) -> Vec2 {
    Vec2 { x: p.x, y: p.y }
}

fn fh(i: usize) -> FixedVertexHandle {
    FixedVertexHandle::from_index(i)
}

/// Grid (nm) of the noded path of [`Cdt::build`]: KiCad's internal unit. Board items are
/// stored in integer nm, so the grid moves no coordinate of the board file; it moves only computed
/// points (arc and circle approximations, LCT feet, crossings) by at most 0.71 nm, far below any
/// clearance (and below the [`SNAP_NM`] last fallback). Points closer than this merge.
pub const NODE_GRID_NM: f64 = 1.0;

/// How [`Cdt::build`] got its triangulation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CdtPath {
    /// spade's own constraint splitting (`add_constraint_and_split`) on the input as given.
    #[default]
    Direct,
    /// The constraints noded first ([`crate::noding`], snap rounding on [`NODE_GRID_NM`]), then
    /// inserted without crossings.
    Noded,
    /// Last fallback: spade's splitting with the constraints in reverse order.
    Reversed,
    /// Last fallback: spade's splitting on points snapped to [`SNAP_NM`].
    Snapped,
}

impl CdtPath {
    pub fn name(self) -> &'static str {
        match self {
            CdtPath::Direct => "direct",
            CdtPath::Noded => "noded",
            CdtPath::Reversed => "reversed order",
            CdtPath::Snapped => "snapped to 128 nm",
        }
    }
}

/// A failed [`Cdt::build_reported`]: the error and the report (which constraint spade failed on).
pub type CdtFailure = Box<(String, CdtReport)>;

/// What [`Cdt::build_reported`] did besides the triangulation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CdtReport {
    pub path: CdtPath,
    /// spade's panic message of the direct attempt (None when it succeeded or was skipped).
    pub panic: Option<String>,
    /// The input constraint spade was inserting when it panicked.
    pub panic_at: Option<usize>,
    /// Why the noded attempt failed, when it did.
    pub noded_error: Option<String>,
    /// Input constraints left out (noded path): a piece of each still crossed a constraint
    /// after the noding. Sorted, distinct.
    pub dropped: Vec<usize>,
    /// Proper crossings among the input constraints (noded path).
    pub crossings: usize,
    /// Distinct input points merged into one vertex by the noding grid.
    pub merged: usize,
}

/// Debugging switch `TOPOR_CDT_NODING` (docs/dev/debug.md): `always` skips the direct attempt
/// (the noded path on every input), `never` skips the noded path (the behaviour before it, e.g.
/// to see the plugin's fail-fast dialog on a board that needs it). Unset: both, in order.
fn noding_env() -> Option<String> {
    std::env::var("TOPOR_CDT_NODING").ok()
}

fn panic_text(e: Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| e.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".into())
}

impl Cdt {
    /// [`Cdt::build_reported`] without the report.
    pub fn build(points: &[Vec2], constraints: &[(usize, usize)]) -> Result<Cdt, String> {
        Self::build_reported(points, constraints).map(|r| r.0).map_err(|e| e.0)
    }

    /// Points are deduplicated (coincident inputs share a vertex). Constraints are index pairs;
    /// crossing ones are split (the split vertices get the canonical ids `>= num_inputs()`).
    /// `Err` for non-finite points and out-of-range indices, and when every attempt below fails.
    ///
    /// 1. **Direct**: spade's `add_constraint_and_split` on the input as given. Boards that
    ///    never hit spade's panic keep exactly this triangulation.
    /// 2. **Noded** (after spade panicked): spade 2.15's splitting can panic (`!previous_region.
    ///    is_empty() || from == final_vertex`) where constraints cross at a nearly degenerate
    ///    position (K1: copper graphics of dcsgonebadge; bornhack2026 2026-11-07: see
    ///    `engine/tests/topo_cdt_noding.rs`). The arrangement is then computed here: the points
    ///    snapped to [`NODE_GRID_NM`], the constraints noded by snap rounding
    ///    ([`crate::noding`]), and the crossing-free pieces inserted with `try_add_constraint`.
    ///    A piece that still meets a constraint is left out and its input constraint listed in
    ///    [`CdtReport::dropped`] (the caller names it); the build never fails on one segment.
    /// 3. **Reversed**, then **Snapped**: the old retries (constraints in reverse order; points on
    ///    a [`SNAP_NM`] grid), kept as the last fallback.
    ///
    /// The projection only: the drawing checks the exact copper. O(attempts · spade build); the
    /// noding is O((n + k) · c) (see [`crate::noding`]).
    pub fn build_reported(points: &[Vec2], constraints: &[(usize, usize)]) -> Result<(Cdt, CdtReport), CdtFailure> {
        let mut rep = CdtReport::default();
        if let Some(i) = points.iter().position(|p| !p.x.is_finite() || !p.y.is_finite()) {
            return Err(Box::new((format!("point {i} is not finite"), rep)));
        }
        if let Some(&(a, b)) = constraints.iter().find(|c| c.0 >= points.len() || c.1 >= points.len()) {
            return Err(Box::new((format!("constraint index {} out of range", a.max(b)), rep)));
        }
        let env = noding_env();
        if env.as_deref() != Some("always") {
            let at = std::cell::Cell::new(None);
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Self::build_once(points, constraints, false, false, &at))) {
                Ok(r) => return r.map(|c| (c, rep.clone())).map_err(|e| Box::new((e, rep.clone()))),
                Err(e) => {
                    rep.panic = Some(panic_text(e));
                    rep.panic_at = at.get();
                }
            }
        }
        if env.as_deref() == Some("never") {
            rep.noded_error = Some("skipped (TOPOR_CDT_NODING=never)".into());
        } else {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Self::build_noded(points, constraints, &mut rep))) {
                Ok(Ok(c)) => {
                    rep.path = CdtPath::Noded;
                    return Ok((c, rep));
                }
                Ok(Err(e)) => rep.noded_error = Some(e),
                Err(e) => rep.noded_error = Some(format!("panic: {}", panic_text(e))),
            }
        }
        rep.dropped.clear();
        let mut last = rep.panic.clone().unwrap_or_default();
        for (path, reverse, snap) in [(CdtPath::Reversed, true, false), (CdtPath::Snapped, false, true)] {
            let at = std::cell::Cell::new(None);
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Self::build_once(points, constraints, reverse, snap, &at))) {
                Ok(Ok(c)) => {
                    rep.path = path;
                    return Ok((c, rep));
                }
                Ok(Err(e)) => return Err(Box::new((e, rep))),
                Err(e) => last = panic_text(e),
            }
        }
        let noded = rep.noded_error.clone().unwrap_or_default();
        Err(Box::new((format!("triangulation failed on degenerate crossing constraints: {last} (noded attempt: {noded})"), rep)))
    }

    /// One path of [`Cdt::build_reported`] only, without the fallbacks (tests, debugging). A panic
    /// of spade is an `Err`.
    pub fn build_forced(points: &[Vec2], constraints: &[(usize, usize)], path: CdtPath) -> Result<(Cdt, CdtReport), String> {
        if let Some(i) = points.iter().position(|p| !p.x.is_finite() || !p.y.is_finite()) {
            return Err(format!("point {i} is not finite"));
        }
        if let Some(&(a, b)) = constraints.iter().find(|c| c.0 >= points.len() || c.1 >= points.len()) {
            return Err(format!("constraint index {} out of range", a.max(b)));
        }
        let mut rep = CdtReport { path, ..CdtReport::default() };
        let at = std::cell::Cell::new(None);
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match path {
            CdtPath::Direct => Self::build_once(points, constraints, false, false, &at),
            CdtPath::Reversed => Self::build_once(points, constraints, true, false, &at),
            CdtPath::Snapped => Self::build_once(points, constraints, false, true, &at),
            CdtPath::Noded => Self::build_noded(points, constraints, &mut rep),
        }));
        match run {
            Ok(r) => r.map(|c| (c, rep)),
            Err(e) => Err(format!("panic: {}", panic_text(e))),
        }
    }

    /// spade's own splitting; `at` = the input constraint being inserted (for the report).
    fn build_once(points: &[Vec2], constraints: &[(usize, usize)], reverse: bool, snap: bool, at: &std::cell::Cell<Option<usize>>) -> Result<Cdt, String> {
        let mut tri = Tri::new();
        let mut map = Vec::with_capacity(points.len());
        for (i, p) in points.iter().enumerate() {
            let p = if snap {
                Vec2 { x: (p.x / SNAP_NM).round() * SNAP_NM, y: (p.y / SNAP_NM).round() * SNAP_NM }
            } else {
                *p
            };
            let h = tri
                .insert(V { pos: p2(p) })
                .map_err(|e| format!("insert point {i}: {e:?}"))?;
            map.push(h.index());
        }
        let n_inputs = tri.num_vertices();
        let order: Box<dyn Iterator<Item = (usize, &(usize, usize))>> =
            if reverse { Box::new(constraints.iter().enumerate().rev()) } else { Box::new(constraints.iter().enumerate()) };
        for (k, &(a, b)) in order {
            let (ca, cb) = (map[a], map[b]);
            if ca == cb {
                continue;
            }
            at.set(Some(k));
            // Crossing constraints are split at the intersection; overlapping and duplicate
            // constraints reuse the existing constraint edges.
            tri.add_constraint_and_split(fh(ca), fh(cb), |p| V { pos: p });
        }
        at.set(None);
        Ok(Cdt { tri, map, n_inputs })
    }

    /// The noded path (see [`Cdt::build_reported`]). `Err` when a coordinate exceeds the
    /// noding range ([`crate::noding::MAX_COORD`] grid units).
    fn build_noded(points: &[Vec2], constraints: &[(usize, usize)], rep: &mut CdtReport) -> Result<Cdt, String> {
        use crate::noding::{snap_round, IPt, MAX_COORD};
        let lim = MAX_COORD as f64;
        let mut q: Vec<IPt> = Vec::with_capacity(points.len());
        for p in points {
            let (x, y) = ((p.x / NODE_GRID_NM).round(), (p.y / NODE_GRID_NM).round());
            if x.abs() > lim || y.abs() > lim {
                return Err(format!("a point at ({:.0}, {:.0}) nm is outside the noding range", p.x, p.y));
            }
            q.push([x as i64, y as i64]);
        }
        let mut tri = Tri::new();
        let mut map = Vec::with_capacity(points.len());
        for (i, p) in q.iter().enumerate() {
            let pos = Point2::new(p[0] as f64 * NODE_GRID_NM, p[1] as f64 * NODE_GRID_NM);
            let h = tri.insert(V { pos }).map_err(|e| format!("insert point {i}: {e:?}"))?;
            map.push(h.index());
        }
        let n_inputs = tri.num_vertices();
        // Distinct input positions that share a vertex only after the snapping.
        let mut first: Vec<Option<Vec2>> = vec![None; n_inputs];
        for (i, &c) in map.iter().enumerate() {
            match first[c] {
                None => first[c] = Some(points[i]),
                Some(p) if p != points[i] => rep.merged += 1,
                _ => {}
            }
        }
        let mut vpos: Vec<IPt> = vec![[0, 0]; n_inputs];
        for (i, &c) in map.iter().enumerate() {
            vpos[c] = q[i];
        }
        // Distinct non-degenerate segments, with the first input constraint of each.
        let mut seen: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
        let (mut segs, mut origin) = (Vec::new(), Vec::new());
        for (k, &(a, b)) in constraints.iter().enumerate() {
            let (ca, cb) = (map[a], map[b]);
            if ca != cb && seen.insert((ca.min(cb), ca.max(cb))) {
                segs.push((ca, cb));
                origin.push(k);
            }
        }
        let nd = snap_round(&vpos, &segs);
        rep.crossings = nd.crossings;
        for (j, p) in nd.new_pts.iter().enumerate() {
            let h = tri
                .insert(V { pos: Point2::new(p[0] as f64 * NODE_GRID_NM, p[1] as f64 * NODE_GRID_NM) })
                .map_err(|e| format!("insert crossing {j}: {e:?}"))?;
            if h.index() != n_inputs + j {
                return Err(format!("crossing {j} coincides with vertex {}", h.index()));
            }
        }
        let mut dropped = Vec::new();
        for (s, chain) in nd.chains.iter().enumerate() {
            for w in chain.windows(2) {
                let (a, b) = (fh(w[0]), fh(w[1]));
                if tri.exists_constraint(a, b) {
                    continue;
                }
                if tri.try_add_constraint(a, b).is_empty() {
                    dropped.push(origin[s]);
                }
            }
        }
        dropped.sort_unstable();
        dropped.dedup();
        rep.dropped = dropped;
        Ok(Cdt { tri, map, n_inputs })
    }

    pub fn num_vertices(&self) -> usize {
        self.tri.num_vertices()
    }

    /// Number of vertices before constraint insertion (deduplicated inputs).
    pub fn num_inputs(&self) -> usize {
        self.n_inputs
    }

    /// Whether canonical vertex `v` was created by splitting crossing constraints.
    pub fn is_split_vertex(&self, v: usize) -> bool {
        v >= self.n_inputs
    }

    /// Constraint edges as canonical `(a, b)` with `a < b`, sorted.
    pub fn constraint_edges(&self) -> Vec<(usize, usize)> {
        let mut out: Vec<(usize, usize)> = self
            .tri
            .undirected_edges()
            .filter(|e| self.tri.is_constraint_edge(e.fix()))
            .map(|e| {
                let [a, b] = e.vertices().map(|v| v.fix().index());
                (a.min(b), a.max(b))
            })
            .collect();
        out.sort_unstable();
        out
    }

    /// Sorted edges and inner (finite) faces. O(V log V).
    pub fn topology(&self) -> Topology {
        let mut edges: Vec<(usize, usize, bool)> = self
            .tri
            .undirected_edges()
            .map(|e| {
                let [a, b] = e.vertices().map(|v| v.fix().index());
                (a.min(b), a.max(b), self.tri.is_constraint_edge(e.fix()))
            })
            .collect();
        edges.sort_unstable();
        let mut faces: Vec<[usize; 3]> = self
            .tri
            .inner_faces()
            .map(|f| {
                let [a, b, c] = f.vertices().map(|v| v.fix().index());
                if a <= b && a <= c {
                    [a, b, c]
                } else if b <= c {
                    [b, c, a]
                } else {
                    [c, a, b]
                }
            })
            .collect();
        faces.sort_unstable();
        Topology { edges, faces }
    }

    /// Canonical index after dedup of input point `input`.
    pub fn vertex_index(&self, input: usize) -> usize {
        self.map[input]
    }

    /// Position of canonical vertex `v`.
    pub fn vertex_pos(&self, v: usize) -> Vec2 {
        v2(self.tri.vertex(fh(v)).position())
    }

    fn edge(&self, a: usize, b: usize) -> Option<spade::handles::FixedUndirectedEdgeHandle> {
        let (ca, cb) = (*self.map.get(a)?, *self.map.get(b)?);
        self.tri
            .get_edge_from_neighbors(fh(ca), fh(cb))
            .map(|e| e.fix().as_undirected())
    }

    /// Whether input points `a` and `b` are joined by a triangulation edge.
    pub fn is_edge(&self, a: usize, b: usize) -> bool {
        self.edge(a, b).is_some()
    }

    /// Whether input points `a` and `b` are joined by a constraint edge.
    pub fn is_constraint(&self, a: usize, b: usize) -> bool {
        self.edge(a, b)
            .is_some_and(|e| self.tri.is_constraint_edge(e))
    }

    /// Triangulation edges crossed by the polyline, in order, oriented by
    /// travel direction (`left` / `right` of the direction of travel). `Err` if
    /// a polyline vertex other than the first/last coincides with (or lies on)
    /// the triangulation, or more than `MAX_SLEEVE` crossings. O(k) in the
    /// number of crossed edges via triangle adjacency.
    pub fn crossings(&self, polyline: &[Vec2]) -> Result<Vec<Crossing>, String> {
        let mut out: Vec<Crossing> = Vec::new();
        let n = polyline.len();
        for (si, w) in polyline.windows(2).enumerate() {
            let (p, q) = (w[0], w[1]);
            if p == q {
                continue;
            }
            let dir = q - p;
            let last_seg = si + 2 == n;
            for isect in LineIntersectionIterator::new(&self.tri, p2(p), p2(q)) {
                match isect {
                    Intersection::EdgeIntersection(e) => {
                        let (a, b) = (e.from(), e.to());
                        let (pa, pb) = (v2(a.position()), v2(b.position()));
                        let s = dir.cross(pb - pa);
                        let (l, r, pl, pr) = if s > 0.0 {
                            (b.fix().index(), a.fix().index(), pb, pa)
                        } else {
                            (a.fix().index(), b.fix().index(), pa, pb)
                        };
                        // An interior polyline vertex lying exactly on an edge is ambiguous.
                        let (op, oq) = ((pr - pl).cross(p - pl), (pr - pl).cross(q - pl));
                        if (op == 0.0 && si > 0) || (oq == 0.0 && !last_seg) {
                            return Err("polyline vertex lies on a triangulation edge".into());
                        }
                        let denom = (pr - pl).cross(dir);
                        if denom == 0.0 {
                            return Err("degenerate crossing (parallel edge)".into());
                        }
                        let t = ((p - pl).cross(dir) / denom).clamp(0.0, 1.0);
                        if out.len() >= MAX_SLEEVE {
                            return Err(format!("sleeve exceeds MAX_SLEEVE = {MAX_SLEEVE}"));
                        }
                        out.push(Crossing {
                            left: l,
                            right: r,
                            t,
                        });
                    }
                    Intersection::VertexIntersection(v) => {
                        let pos = v2(v.position());
                        let ok = (pos == p && si == 0) || (pos == q && last_seg);
                        if !ok {
                            return Err(format!(
                                "polyline passes through triangulation vertex {}",
                                v.fix().index()
                            ));
                        }
                    }
                    Intersection::EdgeOverlap(_) => {
                        return Err("polyline overlaps a triangulation edge".into());
                    }
                }
            }
        }
        Ok(out)
    }

    /// Cancel adjacent crossings of the same edge in opposite directions
    /// (11 §2 step 3), repeatedly.
    pub fn reduce(seq: &[Crossing]) -> Vec<Crossing> {
        let mut st: Vec<Crossing> = Vec::with_capacity(seq.len());
        for &c in seq {
            match st.last() {
                Some(l) if l.left == c.right && l.right == c.left => {
                    st.pop();
                }
                _ => st.push(c),
            }
        }
        st
    }
}
