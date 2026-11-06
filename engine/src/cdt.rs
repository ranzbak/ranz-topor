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

impl Cdt {
    /// Points are deduplicated (coincident inputs share a vertex). Constraints
    /// are index pairs; crossing ones are split. `Err` for non-finite points, and when spade's
    /// constraint splitting fails on every attempt below.
    ///
    /// spade 2.15's `add_constraint_and_split` can panic (`!previous_region.is_empty() || from ==
    /// final_vertex`) where two constraints cross at a nearly degenerate position (K1: copper
    /// graphics of dcsgonebadge as KiCad's IPC API exports them, overlapping arcs and segments). The
    /// build then runs again with the constraints in reverse order (same geometry), then with the
    /// points snapped to a [`SNAP_NM`] grid (the projection only; the drawing checks the exact
    /// copper). The first attempt that succeeds is used, so boards that never hit the panic get the
    /// same triangulation as before. O(attempts * spade build).
    pub fn build(points: &[Vec2], constraints: &[(usize, usize)]) -> Result<Cdt, String> {
        if let Some(i) = points.iter().position(|p| !p.x.is_finite() || !p.y.is_finite()) {
            return Err(format!("point {i} is not finite"));
        }
        let mut last = String::new();
        for attempt in 0..3 {
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Self::build_once(points, constraints, attempt == 1, attempt == 2)
            }));
            match run {
                Ok(r) => return r,
                Err(e) => {
                    last = e
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| e.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "panic".into());
                }
            }
        }
        Err(format!("triangulation failed on degenerate crossing constraints: {last}"))
    }

    fn build_once(points: &[Vec2], constraints: &[(usize, usize)], reverse: bool, snap: bool) -> Result<Cdt, String> {
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
        let order: Box<dyn Iterator<Item = &(usize, usize)>> =
            if reverse { Box::new(constraints.iter().rev()) } else { Box::new(constraints.iter()) };
        for &(a, b) in order {
            let ca = *map
                .get(a)
                .ok_or_else(|| format!("constraint index {a} out of range"))?;
            let cb = *map
                .get(b)
                .ok_or_else(|| format!("constraint index {b} out of range"))?;
            if ca == cb {
                continue;
            }
            // Crossing constraints are split at the intersection; overlapping and duplicate
            // constraints reuse the existing constraint edges.
            tri.add_constraint_and_split(fh(ca), fh(cb), |p| V { pos: p });
        }
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
