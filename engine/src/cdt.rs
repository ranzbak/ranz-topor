//! Constrained Delaunay triangulation wrapper (spade) and polyline crossing
//! sequences (topology docs 10, 11 §2). Coordinates are nm in `f64`.
use crate::geom::Vec2;
use spade::handles::FixedVertexHandle;
use spade::{
    ConstrainedDelaunayTriangulation, HasPosition, Intersection, LineIntersectionIterator, Point2,
    Triangulation,
};

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

pub struct Cdt {
    tri: Tri,
    /// input index -> canonical (spade vertex) index
    map: Vec<usize>,
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
    /// are index pairs (outline edges).
    pub fn build(points: &[Vec2], constraints: &[(usize, usize)]) -> Result<Cdt, String> {
        let mut tri = Tri::new();
        let mut map = Vec::with_capacity(points.len());
        for (i, p) in points.iter().enumerate() {
            if !p.x.is_finite() || !p.y.is_finite() {
                return Err(format!("point {i} is not finite"));
            }
            let h = tri
                .insert(V { pos: p2(*p) })
                .map_err(|e| format!("insert point {i}: {e:?}"))?;
            map.push(h.index());
        }
        for &(a, b) in constraints {
            let ca = *map
                .get(a)
                .ok_or_else(|| format!("constraint index {a} out of range"))?;
            let cb = *map
                .get(b)
                .ok_or_else(|| format!("constraint index {b} out of range"))?;
            if ca == cb {
                continue;
            }
            if !tri.can_add_constraint(fh(ca), fh(cb)) {
                return Err(format!("constraint ({a}, {b}) crosses another constraint"));
            }
            tri.add_constraint(fh(ca), fh(cb));
        }
        Ok(Cdt { tri, map })
    }

    pub fn num_vertices(&self) -> usize {
        self.tri.num_vertices()
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
