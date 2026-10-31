//! Dev tool (M2 task 2 step 1): why does `build_layer_cdt` reject a layer's constraints?
//!
//! `cargo run --release --example cdt_probe -- <model.json> [--routing]`
//!
//! For every layer with anchors: build an `ObstacleSet` (`Options::default()`, no exclusions, no
//! terminals) and call `build_layer_cdt(layer, &[])`. Then, independent of the result, replay the
//! constraint insertion with spade directly: every constraint that `can_add_constraint` rejects is
//! printed with the constraint edges it intersects (positions in mm, wall labels and kinds, and the
//! pair classification: crossing / collinear overlap / interior touch), skipped, and the replay
//! goes on so that every rejection is listed. Then a pairwise census of the wall chords of the
//! constraint set reports every geometrically conflicting pair (what a split-based insertion has
//! to resolve), and a second replay with `add_constraint_and_split` (the step 4 rule) reports the
//! split vertices it creates. `--routing` adds `Pad | Track | Shape | Keepout` walls to the set
//! (task 2 step 4 `ConstraintSet::Routing`). Not part of the PyO3 surface.
use spade::handles::FixedVertexHandle;
use spade::{
    ConstrainedDelaunayTriangulation, HasPosition, Intersection, LineIntersectionIterator, Point2,
    Triangulation,
};
use std::collections::{HashMap, HashSet};
use topor_engine::geom::Vec2;
use topor_engine::model::{BoardModel, Options};
use topor_engine::obstacle::{Core, LayerObstacles, ObsKind, ObstacleSet, Wall};
use topor_engine::sketch::build_layer_cdt;

#[derive(Clone, Copy)]
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

const MAX_EXAMPLES: usize = 12;

fn mm(p: Vec2) -> String {
    format!("({:.6}, {:.6})", p.x / 1e6, p.y / 1e6)
}

fn kind_name(k: ObsKind) -> &'static str {
    match k {
        ObsKind::Pad => "Pad",
        ObsKind::Via => "Via",
        ObsKind::Hole => "Hole",
        ObsKind::Edge => "Edge",
        ObsKind::Shape => "Shape",
        ObsKind::Track => "Track",
        ObsKind::Terminal => "Terminal",
        ObsKind::Keepout => "Keepout",
    }
}

fn core_name(w: &Wall) -> &'static str {
    match w.core {
        Core::Point(_) => "point",
        Core::Seg(_) => "seg",
        Core::Arc(_) => "arc",
    }
}

fn in_set(k: ObsKind, routing: bool) -> bool {
    match k {
        ObsKind::Edge => true,
        ObsKind::Pad | ObsKind::Track | ObsKind::Shape | ObsKind::Keepout => routing,
        _ => false,
    }
}

/// Orientation of `c` relative to the directed line `a -> b`, computed on coordinates translated
/// to `a` (exact for integer nm; arc endpoints carry a sub-nm fraction).
fn orient(a: Vec2, b: Vec2, c: Vec2) -> f64 {
    (b - a).cross(c - a)
}

/// Perpendicular distance (nm) of `c` from the line `a b`.
fn dist_line(a: Vec2, b: Vec2, c: Vec2) -> f64 {
    let d = b - a;
    let l = d.dot(d).sqrt();
    if l == 0.0 {
        return (c - a).dot(c - a).sqrt();
    }
    orient(a, b, c).abs() / l
}

/// Projection parameter of `c` onto `a b` (0 at `a`, 1 at `b`).
fn param(a: Vec2, b: Vec2, c: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.dot(d);
    if l2 == 0.0 {
        0.0
    } else {
        (c - a).dot(d) / l2
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Rel {
    /// Interiors cross at a single point.
    Crossing,
    /// Collinear with an overlap of positive length.
    CollinearOverlap,
    /// An endpoint of one lies in the interior of the other (T junction), no crossing.
    InteriorTouch,
    /// Share an endpoint (exactly) and are otherwise disjoint.
    SharedEnd,
    /// No contact.
    Disjoint,
}

/// Relation between the closed segments `p0 p1` and `q0 q1`.
fn relate(p0: Vec2, p1: Vec2, q0: Vec2, q1: Vec2) -> Rel {
    let d1 = orient(p0, p1, q0);
    let d2 = orient(p0, p1, q1);
    let d3 = orient(q0, q1, p0);
    let d4 = orient(q0, q1, p1);
    if d1 == 0.0 && d2 == 0.0 {
        // collinear
        let (ta, tb) = (param(p0, p1, q0), param(p0, p1, q1));
        let (lo, hi) = (ta.min(tb), tb.max(ta));
        let ov_lo = lo.max(0.0);
        let ov_hi = hi.min(1.0);
        if ov_hi > ov_lo {
            return Rel::CollinearOverlap;
        }
        let shares = p0 == q0 || p0 == q1 || p1 == q0 || p1 == q1;
        return if shares {
            Rel::SharedEnd
        } else {
            Rel::Disjoint
        };
    }
    let shares = p0 == q0 || p0 == q1 || p1 == q0 || p1 == q1;
    let strict_cross = ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0));
    if strict_cross {
        return Rel::Crossing;
    }
    let on = |o: f64, a: Vec2, b: Vec2, c: Vec2| {
        if o != 0.0 {
            return false;
        }
        let t = param(a, b, c);
        t > 0.0 && t < 1.0
    };
    if on(d1, p0, p1, q0) || on(d2, p0, p1, q1) || on(d3, q0, q1, p0) || on(d4, q0, q1, p1) {
        return Rel::InteriorTouch;
    }
    if shares {
        Rel::SharedEnd
    } else {
        Rel::Disjoint
    }
}

fn rel_name(r: Rel) -> &'static str {
    match r {
        Rel::Crossing => "crossing",
        Rel::CollinearOverlap => "collinear-overlap",
        Rel::InteriorTouch => "interior-touch",
        Rel::SharedEnd => "shared-end",
        Rel::Disjoint => "disjoint",
    }
}

/// Crossing point of the lines `p0 p1` and `q0 q1` (only meaningful for `Rel::Crossing`).
fn cross_point(p0: Vec2, p1: Vec2, q0: Vec2, q1: Vec2) -> Vec2 {
    let d = p1 - p0;
    let e = q1 - q0;
    let den = d.cross(e);
    let t = (q0 - p0).cross(e) / den;
    p0 + d * t
}

/// Distance (nm) from `x` to the nearest of the four endpoints.
fn nearest_end(x: Vec2, ends: [Vec2; 4]) -> f64 {
    ends.iter()
        .map(|e| {
            let d = *e - x;
            d.dot(d).sqrt()
        })
        .fold(f64::INFINITY, f64::min)
}

struct Chord {
    wall: usize,
    a: Vec2,
    b: Vec2,
}

fn chords(obs: &LayerObstacles, routing: bool) -> Vec<Chord> {
    obs.walls
        .iter()
        .enumerate()
        .filter(|(_, w)| in_set(w.kind, routing))
        .map(|(i, w)| Chord {
            wall: i,
            a: obs.anchors[w.ends[0]].c,
            b: obs.anchors[w.ends[1]].c,
        })
        .collect()
}

fn describe_wall(obs: &LayerObstacles, i: usize) -> String {
    let w = &obs.walls[i];
    format!(
        "wall {i} {} {} '{}' net {} ends {}->{}",
        kind_name(w.kind),
        core_name(w),
        w.label,
        w.net,
        mm(obs.anchors[w.ends[0]].c),
        mm(obs.anchors[w.ends[1]].c)
    )
}

fn describe_pair(obs: &LayerObstacles, ci: &Chord, cj: &Chord) -> String {
    let r = relate(ci.a, ci.b, cj.a, cj.b);
    let mut s = format!(
        "  {}\n    {}\n    -> {}",
        describe_wall(obs, ci.wall),
        describe_wall(obs, cj.wall),
        rel_name(r)
    );
    match r {
        Rel::Crossing => {
            let x = cross_point(ci.a, ci.b, cj.a, cj.b);
            s += &format!(
                " at {} ({:.3} nm from the nearest endpoint; endpoint offsets from the other line: {:.3} / {:.3} / {:.3} / {:.3} nm)",
                mm(x),
                nearest_end(x, [ci.a, ci.b, cj.a, cj.b]),
                dist_line(cj.a, cj.b, ci.a),
                dist_line(cj.a, cj.b, ci.b),
                dist_line(ci.a, ci.b, cj.a),
                dist_line(ci.a, ci.b, cj.b),
            );
        }
        Rel::CollinearOverlap => {
            let (ta, tb) = (param(ci.a, ci.b, cj.a), param(ci.a, ci.b, cj.b));
            s += &format!(" (second chord spans t = {ta:.6} .. {tb:.6} of the first)");
        }
        Rel::InteriorTouch => {
            let ends = [(cj.a, ci), (cj.b, ci)];
            for (e, c) in ends {
                if orient(c.a, c.b, e) == 0.0 {
                    let t = param(c.a, c.b, e);
                    if t > 0.0 && t < 1.0 {
                        s += &format!(" (endpoint {} at t = {t:.6} of the first)", mm(e));
                    }
                }
            }
            let ends = [(ci.a, cj), (ci.b, cj)];
            for (e, c) in ends {
                if orient(c.a, c.b, e) == 0.0 {
                    let t = param(c.a, c.b, e);
                    if t > 0.0 && t < 1.0 {
                        s += &format!(" (endpoint {} at t = {t:.6} of the second)", mm(e));
                    }
                }
            }
        }
        _ => {}
    }
    s
}

/// Pairwise census of the chords: counts per relation and the first examples.
fn census(obs: &LayerObstacles, ch: &[Chord], title: &str) {
    let mut counts: HashMap<Rel, usize> = HashMap::new();
    let mut examples: Vec<String> = Vec::new();
    let mut zero_len = 0usize;
    let mut near_ends = 0usize;
    let mut near_examples: Vec<String> = Vec::new();
    for c in ch {
        if c.a == c.b {
            zero_len += 1;
            if zero_len <= 4 {
                println!("  zero-length chord: {}", describe_wall(obs, c.wall));
            }
        }
    }
    for i in 0..ch.len() {
        for j in (i + 1)..ch.len() {
            let (ci, cj) = (&ch[i], &ch[j]);
            if ci.a == ci.b || cj.a == cj.b {
                continue;
            }
            let r = relate(ci.a, ci.b, cj.a, cj.b);
            let n = counts.entry(r).or_default();
            *n += 1;
            if matches!(
                r,
                Rel::Crossing | Rel::CollinearOverlap | Rel::InteriorTouch
            ) && *n <= MAX_EXAMPLES / 3
            {
                examples.push(describe_pair(obs, ci, cj));
            }
            // Endpoints that nearly (but not exactly) coincide: the dedup in Cdt::build keeps
            // both, which turns one shared corner into two vertices and a sliver.
            for e in [ci.a, ci.b] {
                for f in [cj.a, cj.b] {
                    let d = e - f;
                    let dd = d.dot(d).sqrt();
                    if dd > 0.0 && dd < 10.0 {
                        near_ends += 1;
                        if near_examples.len() < 4 {
                            near_examples.push(format!(
                                "  {} vs {} ({dd:.6} nm apart): walls {} / {}",
                                mm(e),
                                mm(f),
                                describe_wall(obs, ci.wall),
                                describe_wall(obs, cj.wall)
                            ));
                        }
                    }
                }
            }
        }
    }
    println!(
        "  census {title}: {} chords ({zero_len} zero-length), pairs: crossing {}, collinear-overlap {}, interior-touch {}, shared-end {}",
        ch.len(),
        counts.get(&Rel::Crossing).copied().unwrap_or(0),
        counts.get(&Rel::CollinearOverlap).copied().unwrap_or(0),
        counts.get(&Rel::InteriorTouch).copied().unwrap_or(0),
        counts.get(&Rel::SharedEnd).copied().unwrap_or(0),
    );
    for e in &examples {
        println!("{e}");
    }
    if near_ends > 0 {
        println!("  near-coincident endpoint pairs (< 10 nm apart, not equal): {near_ends}");
        for e in &near_examples {
            println!("{e}");
        }
    }
}

/// Replay `Cdt::build` with spade, skipping (instead of failing on) every rejected constraint.
fn replay(obs: &LayerObstacles, ch: &[Chord]) {
    // Points as in build_layer_cdt: anchors, (no terminals), 4 frame corners.
    let mut pts: Vec<Vec2> = obs.anchors.iter().map(|a| a.c).collect();
    if pts.is_empty() {
        return;
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
    let m = 1e6_f64.max(hi.x - lo.x).max(hi.y - lo.y);
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        pts.push(Vec2 {
            x: if sx < 0.0 { lo.x - m } else { hi.x + m },
            y: if sy < 0.0 { lo.y - m } else { hi.y + m },
        });
    }
    let mut tri = Tri::new();
    let mut map = Vec::with_capacity(pts.len());
    for p in &pts {
        let h = tri.insert(V {
            pos: Point2::new(p.x, p.y),
        });
        map.push(h.expect("insert").index());
    }
    let fh = FixedVertexHandle::from_index;
    // canonical (min, max) vertex pair -> wall of the chord
    let mut owner: HashMap<(usize, usize), usize> = HashMap::new();
    let mut rejected = 0usize;
    let mut added = 0usize;
    let mut same_vertex = 0usize;
    let mut printed = 0usize;
    for c in ch {
        let w = &obs.walls[c.wall];
        let (ca, cb) = (map[w.ends[0]], map[w.ends[1]]);
        if ca == cb {
            same_vertex += 1;
            continue;
        }
        if tri.can_add_constraint(fh(ca), fh(cb)) {
            tri.add_constraint(fh(ca), fh(cb));
            owner.entry((ca.min(cb), ca.max(cb))).or_insert(c.wall);
            added += 1;
            continue;
        }
        rejected += 1;
        if printed >= MAX_EXAMPLES {
            continue;
        }
        printed += 1;
        println!(
            "  REJECTED constraint ({}, {}) = {}",
            w.ends[0],
            w.ends[1],
            describe_wall(obs, c.wall)
        );
        let mut seen: HashSet<(usize, usize)> = HashSet::new();
        for isect in LineIntersectionIterator::new_from_handles(&tri, fh(ca), fh(cb)) {
            let e = match isect {
                Intersection::EdgeIntersection(e) => e,
                Intersection::EdgeOverlap(e) => e,
                Intersection::VertexIntersection(v) => {
                    let p = v.position();
                    println!(
                        "    passes through vertex {} at {}",
                        v.fix().index(),
                        mm(Vec2 { x: p.x, y: p.y })
                    );
                    continue;
                }
            };
            if !tri.is_constraint_edge(e.fix().as_undirected()) {
                continue;
            }
            let (f, t) = (e.from().fix().index(), e.to().fix().index());
            let k = (f.min(t), f.max(t));
            if !seen.insert(k) {
                continue;
            }
            let (pf, pt) = (e.from().position(), e.to().position());
            let (pf, pt) = (Vec2 { x: pf.x, y: pf.y }, Vec2 { x: pt.x, y: pt.y });
            let other = owner.get(&k).copied().or_else(|| {
                // sub-edge of a constraint that spade routed through an existing vertex
                ch.iter()
                    .find(|o| {
                        o.a != o.b
                            && orient(o.a, o.b, pf) == 0.0
                            && orient(o.a, o.b, pt) == 0.0
                            && (0.0..=1.0).contains(&param(o.a, o.b, pf))
                            && (0.0..=1.0).contains(&param(o.a, o.b, pt))
                    })
                    .map(|o| o.wall)
            });
            match other {
                Some(oi) => {
                    let o = Chord {
                        wall: oi,
                        a: obs.anchors[obs.walls[oi].ends[0]].c,
                        b: obs.anchors[obs.walls[oi].ends[1]].c,
                    };
                    println!(
                        "    intersects constraint edge {}->{} (piece of):",
                        mm(pf),
                        mm(pt)
                    );
                    println!("{}", describe_pair(obs, c, &o));
                }
                None => println!(
                    "    intersects constraint edge {}->{} (owner unknown)",
                    mm(pf),
                    mm(pt)
                ),
            }
        }
    }
    println!(
        "  replay: {} constraints, {added} added, {rejected} rejected, {same_vertex} with both ends on one vertex; {} vertices, {} constraint edges",
        ch.len(),
        tri.num_vertices(),
        tri.num_constraints()
    );
}

/// Replay with `add_constraint_and_split` (the task 2 step 4 rule): every constraint is inserted,
/// crossings get split vertices. Reports the split vertices and how close they come to existing
/// vertices (sub-nm neighbours would be a precision hazard for the sketch).
fn replay_split(obs: &LayerObstacles, ch: &[Chord]) {
    let mut pts: Vec<Vec2> = obs.anchors.iter().map(|a| a.c).collect();
    if pts.is_empty() {
        return;
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
    let m = 1e6_f64.max(hi.x - lo.x).max(hi.y - lo.y);
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        pts.push(Vec2 {
            x: if sx < 0.0 { lo.x - m } else { hi.x + m },
            y: if sy < 0.0 { lo.y - m } else { hi.y + m },
        });
    }
    let mut tri = Tri::new();
    let mut map = Vec::with_capacity(pts.len());
    for p in &pts {
        let h = tri.insert(V {
            pos: Point2::new(p.x, p.y),
        });
        map.push(h.expect("insert").index());
    }
    let n_inputs = tri.num_vertices();
    let fh = FixedVertexHandle::from_index;
    let mut pieces = 0usize;
    let mut multi = 0usize;
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for c in ch {
            let w = &obs.walls[c.wall];
            let (ca, cb) = (map[w.ends[0]], map[w.ends[1]]);
            if ca == cb {
                continue;
            }
            let v = tri.add_constraint_and_split(fh(ca), fh(cb), |p| V { pos: p });
            pieces += v.len();
            if v.len() > 1 {
                multi += 1;
            }
        }
    }));
    if res.is_err() {
        println!("  split replay: PANIC inside add_constraint_and_split");
        return;
    }
    let nv = tri.num_vertices();
    let split: Vec<usize> = (n_inputs..nv).collect();
    let mut min_d = f64::INFINITY;
    let mut min_pair = (0usize, 0usize);
    let mut off_line_max = 0.0_f64;
    for &s in &split {
        let p = tri.vertex(fh(s)).position();
        let p = Vec2 { x: p.x, y: p.y };
        for o in 0..nv {
            if o == s {
                continue;
            }
            let q = tri.vertex(fh(o)).position();
            let d = Vec2 { x: q.x, y: q.y } - p;
            let dd = d.dot(d).sqrt();
            if dd < min_d {
                min_d = dd;
                min_pair = (s, o);
            }
        }
        // distance of the split vertex from every chord it should lie on
        for c in ch {
            if c.a == c.b {
                continue;
            }
            let t = param(c.a, c.b, p);
            if (0.0..=1.0).contains(&t) {
                let dl = dist_line(c.a, c.b, p);
                if dl < 1.0 {
                    off_line_max = off_line_max.max(dl);
                }
            }
        }
    }
    println!(
        "  split replay: Ok; {} input vertices, {} split vertices, {} constraint edges ({} constraints in >1 piece, {pieces} pieces); nearest other vertex to a split vertex {:.3} nm (vertices {} / {}); max distance of a split vertex from a chord it splits {:.3e} nm",
        n_inputs,
        split.len(),
        tri.num_constraints(),
        multi,
        min_d,
        min_pair.0,
        min_pair.1,
        off_line_max
    );
    for &s in split.iter().take(6) {
        let p = tri.vertex(fh(s)).position();
        println!("    split vertex {s} at {}", mm(Vec2 { x: p.x, y: p.y }));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let routing = args.iter().any(|a| a == "--routing");
    let path = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .expect("usage: cdt_probe <model.json> [--routing]");
    let text = std::fs::read_to_string(path).expect("read model json");
    let model = BoardModel::from_json(&text).expect("parse model");
    let opts = Options::default();
    let obs = ObstacleSet::build(&model, &opts, &HashSet::new(), &[]);
    println!(
        "{}: {} outline prims, constraint set {}",
        model.board,
        model.outline.len(),
        if routing { "Routing" } else { "EdgeOnly" }
    );
    for l in obs.layers() {
        let lo = obs.layer(l);
        if lo.anchors.is_empty() {
            continue;
        }
        let name = model
            .layers
            .iter()
            .find(|x| x.id == l)
            .map_or("?", |x| x.name.as_str());
        let mut kinds: HashMap<&'static str, usize> = HashMap::new();
        for w in &lo.walls {
            *kinds.entry(kind_name(w.kind)).or_default() += 1;
        }
        let mut kinds: Vec<String> = kinds.iter().map(|(k, v)| format!("{k}={v}")).collect();
        kinds.sort();
        println!(
            "layer {l} ({name}): {} anchors, {} walls [{}]",
            lo.anchors.len(),
            lo.walls.len(),
            kinds.join(" ")
        );
        match build_layer_cdt(lo, &[]) {
            Ok(ls) => println!(
                "  build_layer_cdt (EdgeOnly, as shipped): Ok, {} vertices",
                ls.cdt.num_vertices()
            ),
            Err(e) => println!("  build_layer_cdt (EdgeOnly, as shipped): Err({e})"),
        }
        let ch = chords(lo, routing);
        replay(lo, &ch);
        census(lo, &ch, if routing { "Routing" } else { "EdgeOnly" });
        replay_split(lo, &ch);
    }
}
