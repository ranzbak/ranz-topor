//! M3 step c (shopping list 6.2; design note `docs/progress/notes/m3-radii-spokes-design.md`
//! §4.3): the extended rubber band (ERBS) of every stretch, from the spoke tips and stack radii
//! of [`super::radii`], and its check. Measurement only: nothing here draws copper yet.
//!
//! 1. **Gates.** Every cut item of the stretch is a gate, the segment of its cut between its two
//!    spoke tips (`[tip_p, cap - tip_q]` from `p`, [`super::radii::tip_point`]): the centre line
//!    must cross the cut there (Dayan 1997, PDF p. 32: the spokes; the tip already holds the half
//!    width and the clearance, so a spoke is a zero-width obstacle). Every wrap adds the arc of
//!    its stack radius as gates on rays from the wrapped vertex, inside the wrap's wedge, beyond
//!    the circumscribed radius `rho / cos(step / 2)` (Dayan PDF p. 33: octilinear spokes as the
//!    arc): the octagon's vertices in the octagonal norm (the chords are then the octagon's own
//!    sides, exact), a polygon of `step` for the Euclidean norm (the chords stay outside the
//!    circle). Gates are ordered along the band (segment and position for a crossing, the
//!    corner and the angle round the vertex for a wrap).
//! 2. **Shortest path through the gates.** Popov 2012 p. 119-120 (Fig. 3) computes the path
//!    round the circles ordered along the topological path with a funnel of tangents. The same
//!    path is the minimum of `sum |P_i - P_i+1|` over one point per gate, a convex function of
//!    the gate parameters (a norm of an affine map, summed), so any local minimum is the global
//!    one. It is solved by symmetric Gauss-Seidel sweeps: each point moves to the point of its
//!    gate nearest the straight line from its neighbours (reflection when both lie on one side,
//!    then clamped; exact per coordinate, the one-dimensional function being convex) until no
//!    point moves more than `tol`. Not converged within the sweep budget is reported, never
//!    dropped. The gates sample the plane at the cuts and arcs only, so a chord between two gates
//!    can still come nearer a feature than its bare clearance (a pad of another net beside the
//!    path, the side next to a wrapped corner): then a gate off that feature (outward from its
//!    closest point, at the half width plus its clearance) is inserted and the path solved again,
//!    a few rounds (Popov p. 119: no sagging to the edge between primitives). An arc gate also
//!    starts no nearer than where its ray leaves the feature's offset by the stack radius.
//! 3. **Independent.** Every stretch is solved alone from the plan's tips and radii: no drawn
//!    copper takes part, so the result does not depend on any order.
//! 4. **Check.** All ERBS of a layer against each other (different nets: half widths plus
//!    KiCad's pairwise clearance) and against the sheet's features (pads, vias, holes, the
//!    outline). Dayan PDF p. 26 claims SURF's spacing holds when the sketch is feasible; the
//!    proofs were not obtained (inflated-sketch §2.5), so it is measured: a violation where an
//!    over-full cut carries both parties (or the plan crosses them) is **predicted**; any other
//!    is an **`erbs_gap`**, a model gap with its place.
//!
//! Debug: with `TOPO_RADII` the cut report's summary gets the ERBS counts per layer.
//!
//! Complexity per stretch: O(gates x sweeps); per layer the check O(segments x neighbours in the
//! grid cells).

use super::cutflow::Stretch;
use super::demand::{Bound, Item};
use super::draw::{poly_dist, seg_seg, seg_seg_dist};
use super::radii::{tip_point, LayerOrder};
use super::sheet::{LayerSheet, Norm};
use super::state::point_in_poly;
use crate::geom::Vec2;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::{FRAC_PI_8, TAU};

/// Far end of an arc gate (nm): a ray, in effect.
const FAR: f64 = 1e9;

/// ERBS options.
#[derive(Clone, Copy, Debug)]
pub struct ErbsOpts {
    /// Arc sample step of the Euclidean norm (degrees); the octagonal norm uses its 8 vertices.
    pub arc_step_deg: f64,
    /// Convergence: the largest point move of a sweep (nm).
    pub tol: f64,
    /// Sweep budget per stretch: `sweeps_per_gate x gates + 100`.
    pub sweeps_per_gate: usize,
    /// Rounds of the feature refinement (a gate where a chord comes nearer a feature than its
    /// bare clearance, then solve again).
    pub refine_rounds: usize,
}

impl Default for ErbsOpts {
    fn default() -> Self {
        ErbsOpts { arc_step_deg: 22.5, tol: 0.1, sweeps_per_gate: 20, refine_rounds: 8 }
    }
}

/// Where a gate comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GateSrc {
    /// A cut item (index into [`LayerOrder::cuts`]).
    Cut(usize),
    /// An arc sample round sheet vertex `v`.
    Arc(usize),
    /// A refinement gate off feature `f` (a chord came too near it).
    Feature(usize),
}

/// One gate: the centre line crosses the segment `a b`; `w` the width there.
#[derive(Clone, Copy, Debug)]
pub struct Gate {
    pub a: Vec2,
    pub b: Vec2,
    pub w: f64,
    pub src: GateSrc,
    /// Order along the band: (position, angle share round a wrapped vertex).
    pub key: (f64, f64),
}

/// The ERBS of one stretch.
#[derive(Clone, Debug)]
pub struct Erbs {
    pub stretch: usize,
    /// Start, one point per gate (in gate order), end.
    pub pts: Vec<Vec2>,
    /// Width at every point of `pts`.
    pub w: Vec<f64>,
    /// `pts` without repeated and collinear points (the drawn shape).
    pub path: Vec<Vec2>,
    pub gates: usize,
    /// Gates the feature refinement added.
    pub refined: usize,
    pub sweeps: usize,
    pub converged: bool,
    pub len: f64,
    pub band_len: f64,
}

/// The other party of a violation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Party {
    Stretch(usize),
    Feature(usize),
    Edge,
}

/// One clearance violation of an ERBS (the worst place per pair).
#[derive(Clone, Copy, Debug)]
pub struct Violation {
    pub stretch: usize,
    pub other: Party,
    pub at: Vec2,
    /// Required minus actual distance (nm).
    pub amount: f64,
    /// An over-full cut (or a planned crossing) carries both parties.
    pub predicted: bool,
}

/// Counts of one layer.
#[derive(Clone, Debug, Default)]
pub struct ErbsReport {
    pub stretches: usize,
    pub gates: usize,
    pub not_converged: usize,
    pub refined: usize,
    pub max_sweeps: usize,
    pub len: f64,
    pub band_len: f64,
    pub predicted: usize,
    pub erbs_gap: usize,
    pub worst_gap: f64,
}

/// Clockwise angle from `a` to `b` in `[0, 2 pi)`.
fn cw(a: Vec2, b: Vec2) -> f64 {
    let t = (-a.cross(b)).atan2(a.dot(b));
    if t < 0.0 {
        t + TAU
    } else {
        t
    }
}

/// Share of the way round a wrap (0 at the incoming side, 1 at the outgoing side) of the
/// direction `u` from the vertex, or `None` outside the wedge (as `cutflow::in_wedge`).
fn wedge_share(din: Vec2, dout: Vec2, left: bool, u: Vec2) -> Option<f64> {
    const TOL: f64 = 1e-9;
    let back = -din;
    let (a, lim) = if left { (cw(dout, u), cw(dout, back)) } else { (cw(u, dout), cw(back, dout)) };
    (a > TOL && a < lim - TOL).then(|| 1.0 - a / lim)
}

/// The point of segment `a b` that minimises `|A - X| + |X - B|`. O(1).
pub fn min_on_gate(pa: Vec2, pb: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let e = b - a;
    let l2 = e.len2();
    if l2 <= 0.0 {
        return a;
    }
    let (sa, sb) = (e.cross(pa - a), e.cross(pb - a));
    let target = if sa * sb > 0.0 {
        // Both on one side: the reflection of B through the gate's line.
        let n = e.perp() * (1.0 / l2.sqrt());
        pb - n * (2.0 * n.dot(pb - a))
    } else {
        pb
    };
    let d = target - pa;
    let den = e.cross(d);
    let t = if den.abs() > 1e-12 * l2.sqrt() * d.len().max(1.0) {
        (pa - a).cross(d) / den
    } else {
        // A and B on the gate's line (or A = B): the projection of their middle.
        ((pa + pb) * 0.5 - a).dot(e) / l2
    };
    a + e * t.clamp(0.0, 1.0)
}

/// The shortest path from `s` to `e` through `gates` in order, from the initial points `init`
/// (one per gate): symmetric Gauss-Seidel sweeps until no point moves more than `tol`, at most
/// `max_sweeps`. Returns the points, the sweeps and whether it converged. O(gates x sweeps).
pub fn solve(s: Vec2, e: Vec2, gates: &[(Vec2, Vec2)], init: &[Vec2], tol: f64, max_sweeps: usize) -> (Vec<Vec2>, usize, bool) {
    let m = gates.len();
    let mut p: Vec<Vec2> = init.to_vec();
    let mut sweeps = 0;
    while sweeps < max_sweeps {
        sweeps += 1;
        let mut moved: f64 = 0.0;
        let fwd = sweeps % 2 == 1;
        for j in 0..m {
            let i = if fwd { j } else { m - 1 - j };
            let pa = if i == 0 { s } else { p[i - 1] };
            let pb = if i + 1 == m { e } else { p[i + 1] };
            let x = min_on_gate(pa, pb, gates[i].0, gates[i].1);
            moved = moved.max((x - p[i]).len());
            p[i] = x;
        }
        if moved <= tol {
            return (p, sweeps, true);
        }
    }
    (p, sweeps, m == 0)
}

/// `pts` without repeated points and without points within `tol` of the line of their
/// neighbours.
fn simplify(pts: &[Vec2], tol: f64) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::with_capacity(pts.len());
    for &p in pts {
        if out.last().is_some_and(|&q| (q - p).len() <= tol) {
            continue;
        }
        while out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let d = p - a;
            let off = if d.len() > 0.0 { d.cross(b - a).abs() / d.len() } else { (b - a).len() };
            let inside = (b - a).dot(b - p) <= 0.0;
            if off <= tol && inside {
                out.pop();
            } else {
                break;
            }
        }
        out.push(p);
    }
    out
}

fn poly_len(p: &[Vec2]) -> f64 {
    p.windows(2).map(|w| (w[1] - w[0]).len()).sum()
}

/// Distance along the ray `at + t u` (`at` on the convex polygon's boundary, `u` outward) where
/// the distance to `poly` reaches `rho` (bisection, 1 nm). O(sides x 40).
fn exit_offset(at: Vec2, u: Vec2, rho: f64, poly: &[Vec2]) -> f64 {
    let (mut lo, mut hi) = (0.0, rho);
    let mut n = 0;
    while poly_dist(at + u * hi, poly) < rho && n < 40 {
        lo = hi;
        hi *= 2.0;
        n += 1;
    }
    for _ in 0..40 {
        if hi - lo <= 1.0 {
            break;
        }
        let m = 0.5 * (lo + hi);
        if poly_dist(at + u * m, poly) < rho {
            lo = m;
        } else {
            hi = m;
        }
    }
    hi
}

/// A uniform grid of feature boxes and the largest feature clearance (for the bare test).
fn feat_grid(sh: &LayerSheet, cell: f64) -> (Grid, f64) {
    let mut g = Grid { cell, cells: BTreeMap::new() };
    let mut fclear: f64 = 0.0;
    for (f, ft) in sh.feats.iter().enumerate() {
        if ft.poly.is_empty() {
            continue;
        }
        let lo = ft.poly.iter().fold(Vec2 { x: f64::INFINITY, y: f64::INFINITY }, |m, p| Vec2 { x: m.x.min(p.x), y: m.y.min(p.y) });
        let hi = ft.poly.iter().fold(Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY }, |m, p| Vec2 { x: m.x.max(p.x), y: m.y.max(p.y) });
        g.insert(lo, hi, f);
        fclear = fclear.max(match ft.bound {
            Bound::Pad { clear, .. } | Bound::Hole { clear } | Bound::Edge { clear } => clear,
            Bound::Open => 0.0,
        });
    }
    (g, fclear)
}

/// Closest points of segment `a b` and polygon (or point) `poly`: (distance, on the segment, on
/// the polygon); 0 when they cross or the segment lies inside.
fn seg_poly_near(a: Vec2, b: Vec2, poly: &[Vec2]) -> (f64, Vec2, Vec2) {
    if poly.len() == 1 {
        return seg_seg(a, b, poly[0], poly[0]);
    }
    let mut best = (f64::INFINITY, a, poly[0]);
    for i in 0..poly.len() {
        let r = seg_seg(a, b, poly[i], poly[(i + 1) % poly.len()]);
        if r.0 < best.0 {
            best = r;
        }
    }
    if poly.len() >= 3 && point_in_poly(a, poly) {
        return (0.0, a, best.2);
    }
    best
}

/// The deepest bare-clearance breach of segment `a b` (width `w`, net and clearance of `s`) by a
/// feature of another net: (breach, feature, gate start, outward direction). O(features near).
/// Features in `skip` (ends of an over-full cut the stretch is on: the plan squeezed that
/// clearance on purpose, the tips hold it) are left out. A gate sits 2 nm past the requirement
/// (the solver's tolerance).
#[allow(clippy::too_many_arguments)]
fn breach(sh: &LayerSheet, g: &Grid, fclear: f64, s: &Stretch, w: f64, a: Vec2, b: Vec2, skip: &BTreeSet<usize>, near: &mut BTreeSet<usize>) -> Option<(f64, usize, Vec2, Vec2)> {
    let it = Item { net: s.net, w, s: s.s };
    let (lo, hi) = bbox(a, b, 0.5 * w + fclear.max(s.s));
    g.query(lo, hi, near);
    let mut best: Option<(f64, usize, Vec2, Vec2)> = None;
    for &f in near.iter() {
        let ft = &sh.feats[f];
        let cl = ft.bound.clearance(&it);
        if cl <= 0.0 || skip.contains(&f) {
            continue;
        }
        let req = 0.5 * w + cl;
        let (d, x, c) = seg_poly_near(a, b, &ft.poly);
        if d >= req - 1.0 || best.is_some_and(|bb| bb.0 >= req - d) {
            continue;
        }
        // Outward: from the feature's closest point to the segment's; on contact, the segment's
        // normal on the side away from the feature's centre.
        let u = if d > 1.0 {
            (x - c) * (1.0 / d)
        } else {
            let cen = ft.poly.iter().fold(Vec2::default(), |m, &p| m + p) * (1.0 / ft.poly.len() as f64);
            let n = (b - a).perp().unit().unwrap_or(Vec2 { x: 0.0, y: 1.0 });
            if n.dot(x - cen) >= 0.0 {
                n
            } else {
                -n
            }
        };
        best = Some((req - d, f, c + u * (req + 2.0), u));
    }
    best
}

/// The gates of stretch `si` (unordered), with the item index of every cut item.
fn gates_of(sh: &LayerSheet, s: &Stretch, si: usize, o: &LayerOrder, items: &[(usize, usize)], radius: &BTreeMap<(usize, usize, usize), f64>, opts: &ErbsOpts) -> Vec<Gate> {
    let mut gs = Vec::new();
    let band = &s.band;
    let corner_share = |k: usize, u: Vec2| -> Option<f64> {
        let (v, left) = s.wraps[k]?;
        let _ = v;
        wedge_share(band[k] - band[k - 1], band[k + 1] - band[k], left, u)
    };
    for &(ci, ii) in items {
        let cu = &o.cuts[ci];
        let x = &cu.items[ii];
        let c = &sh.cuts[cu.cut];
        let lo = x.tip_p.min(c.cap - x.tip_q);
        let hi = (c.cap - x.tip_q).max(lo);
        let (a, b) = (tip_point(c, lo), tip_point(c, hi));
        let key = match (x.kind, x.corner) {
            (0, _) => {
                // The band segment crossing the cut nearest the item's band t.
                let e = c.q - c.p;
                let mut best: Option<(f64, f64)> = None;
                for j in 0..band.len().saturating_sub(1) {
                    let d = band[j + 1] - band[j];
                    let den = d.cross(e);
                    if den == 0.0 {
                        continue;
                    }
                    let w0 = c.p - band[j];
                    let (sp, tc) = (w0.cross(e) / den, w0.cross(d) / den);
                    if (-1e-9..=1.0 + 1e-9).contains(&sp) {
                        let score = (tc - x.t_band).abs();
                        if best.is_none_or(|bb| score < bb.0) {
                            best = Some((score, j as f64 + sp.clamp(0.0, 1.0)));
                        }
                    }
                }
                (best.map_or(0.0, |bb| bb.1), 0.5)
            }
            (k, Some(cr)) => {
                let u = if k == 1 { c.q - c.p } else { c.p - c.q };
                (cr as f64, corner_share(cr, u).unwrap_or(0.5))
            }
            _ => continue,
        };
        gs.push(Gate { a, b, w: x.w_tip, src: GateSrc::Cut(ci), key });
    }
    // Arc samples of every wrap at its stack radius.
    let (dirs, grow): (Vec<Vec2>, f64) = match sh.norm {
        Norm::Octagonal => ((0..8).map(|k| Vec2 { x: (FRAC_PI_8 + k as f64 * std::f64::consts::FRAC_PI_4).cos(), y: (FRAC_PI_8 + k as f64 * std::f64::consts::FRAC_PI_4).sin() }).collect(), 1.0 / FRAC_PI_8.cos()),
        Norm::Euclid => {
            let st = opts.arc_step_deg.clamp(1.0, 45.0).to_radians();
            let n = (TAU / st).round().max(8.0) as usize;
            let st = TAU / n as f64;
            ((0..n).map(|k| Vec2 { x: (k as f64 * st).cos(), y: (k as f64 * st).sin() }).collect(), 1.0 / (0.5 * st).cos())
        }
    };
    #[allow(clippy::needless_range_loop)]
    for k in 1..band.len().saturating_sub(1) {
        let Some((v, _)) = s.wraps[k] else { continue };
        let Some(&rho) = radius.get(&(v, si, k)) else { continue };
        let at = sh.pos[v];
        let w = s.width_at_idx(&s.pass_index(), band[k]);
        let poly = sh.vfeat[v].map(|f| &sh.feats[f].poly).filter(|p| p.len() >= 3);
        for &u in &dirs {
            if let Some(f) = corner_share(k, u) {
                // Past the arc, and past the feature's own rho-offset along the ray (a ray over a
                // side of the feature leaves the offset later than the corner's circle: Popov
                // p. 119, no sagging to the edge between primitives).
                let t = poly.map_or(0.0, |p| exit_offset(at, u, rho, p));
                gs.push(Gate { a: at + u * (rho * grow).max(t), b: at + u * FAR, w, src: GateSrc::Arc(v), key: (k as f64, f) });
            }
        }
    }
    gs.sort_by(|a, b| a.key.0.total_cmp(&b.key.0).then(a.key.1.total_cmp(&b.key.1)));
    gs
}

/// The ERBS of the stretches `mine` of a layer with its order, tips and radii (steps a, b).
/// Each stretch alone; the result is independent of the order of `mine` (sorted by stretch).
pub fn erbs_layer(sh: &LayerSheet, sts: &[Stretch], mine: &[usize], o: &LayerOrder, opts: &ErbsOpts) -> Vec<Erbs> {
    let mut per: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    for (ci, cu) in o.cuts.iter().enumerate() {
        for (ii, x) in cu.items.iter().enumerate() {
            per.entry(x.stretch).or_default().push((ci, ii));
        }
    }
    let mut radius: BTreeMap<(usize, usize, usize), f64> = BTreeMap::new();
    for st in &o.stacks {
        for (i, (&s, &k)) in st.order.iter().zip(&st.corner).enumerate() {
            if let Some(&r) = st.radius.get(i) {
                radius.insert((st.v, s, k), r);
            }
        }
    }
    let mut own: Vec<usize> = mine.to_vec();
    own.sort_unstable();
    own.dedup();
    let none = Vec::new();
    let (fg, fclear) = feat_grid(sh, 0.5e6);
    let mut squeezed: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for cu in o.cuts.iter().filter(|cu| cu.squeeze > 0.0) {
        let c = &sh.cuts[cu.cut];
        for x in &cu.items {
            squeezed.entry(x.stretch).or_default().extend([c.fp, c.fq]);
        }
    }
    let no_skip = BTreeSet::new();
    own.iter()
        .filter(|&&si| sts[si].band.len() >= 2)
        .map(|&si| {
            let s = &sts[si];
            let gs = gates_of(sh, s, si, o, per.get(&si).unwrap_or(&none), &radius, opts);
            let (a, e) = (s.band[0], s.band[s.band.len() - 1]);
            let segs: Vec<(Vec2, Vec2)> = gs.iter().map(|g| (g.a, g.b)).collect();
            // Start on the band: the gate point nearest the band's own crossing (a for arcs).
            let init: Vec<Vec2> = gs
                .iter()
                .map(|g| match g.src {
                    GateSrc::Arc(_) | GateSrc::Feature(_) => g.a,
                    GateSrc::Cut(_) => {
                        let j = (g.key.0.floor() as usize).min(s.band.len() - 2);
                        let t = g.key.0 - j as f64;
                        let x = s.band[j] + (s.band[j + 1] - s.band[j]) * t;
                        let e2 = g.b - g.a;
                        let l2 = e2.len2();
                        if l2 > 0.0 {
                            g.a + e2 * ((x - g.a).dot(e2) / l2).clamp(0.0, 1.0)
                        } else {
                            g.a
                        }
                    }
                })
                .collect();
            let mut gs = gs;
            let mut segs = segs;
            let budget = |n: usize| opts.sweeps_per_gate * n + 100;
            let (mut p, mut sweeps, mut converged) = solve(a, e, &segs, &init, opts.tol, budget(segs.len()));
            // Feature refinement: a chord between two gates may come nearer a feature than its
            // bare clearance (the spokes sample the plane at the cuts only): a gate off that
            // feature, outward from its closest point, then solve again from where it was.
            let mut refined = 0;
            let mut near = BTreeSet::new();
            for _ in 0..opts.refine_rounds {
                let mut add: Vec<(usize, Gate)> = Vec::new();
                for k in 0..=p.len() {
                    let x0 = if k == 0 { a } else { p[k - 1] };
                    let x1 = if k == p.len() { e } else { p[k] };
                    let w0 = if k == 0 { s.w } else { gs[k - 1].w };
                    let w1 = if k == p.len() { s.w } else { gs[k].w };
                    if let Some((_, f, at, u)) = breach(sh, &fg, fclear, s, w0.min(w1), x0, x1, squeezed.get(&si).unwrap_or(&no_skip), &mut near) {
                        add.push((k, Gate { a: at, b: at + u * FAR, w: w0.min(w1), src: GateSrc::Feature(f), key: (0.0, 0.0) }));
                    }
                }
                if add.is_empty() {
                    break;
                }
                for (k, g) in add.into_iter().rev() {
                    gs.insert(k, g);
                    segs.insert(k, (g.a, g.b));
                    p.insert(k, g.a);
                    refined += 1;
                }
                let r = solve(a, e, &segs, &p, opts.tol, budget(segs.len()));
                (p, converged) = (r.0, r.2);
                sweeps += r.1;
            }
            let mut pts = Vec::with_capacity(p.len() + 2);
            pts.push(a);
            pts.extend(p);
            pts.push(e);
            let mut w = Vec::with_capacity(pts.len());
            w.push(s.w);
            w.extend(gs.iter().map(|g| g.w));
            w.push(s.w);
            let path = simplify(&pts, 0.5);
            Erbs { stretch: si, len: poly_len(&path), band_len: poly_len(&s.band), pts, w, path, gates: gs.len(), refined, sweeps, converged }
        })
        .collect()
}

/// A uniform grid of boxes (cell `cell` nm).
struct Grid {
    cell: f64,
    cells: BTreeMap<(i64, i64), Vec<usize>>,
}

impl Grid {
    fn key(&self, p: Vec2) -> (i64, i64) {
        ((p.x / self.cell).floor() as i64, (p.y / self.cell).floor() as i64)
    }

    fn insert(&mut self, lo: Vec2, hi: Vec2, id: usize) {
        let (a, b) = (self.key(lo), self.key(hi));
        for x in a.0..=b.0 {
            for y in a.1..=b.1 {
                self.cells.entry((x, y)).or_default().push(id);
            }
        }
    }

    fn query(&self, lo: Vec2, hi: Vec2, out: &mut BTreeSet<usize>) {
        out.clear();
        let (a, b) = (self.key(lo), self.key(hi));
        for x in a.0..=b.0 {
            for y in a.1..=b.1 {
                if let Some(v) = self.cells.get(&(x, y)) {
                    out.extend(v.iter().copied());
                }
            }
        }
    }
}

fn bbox(a: Vec2, b: Vec2, r: f64) -> (Vec2, Vec2) {
    (Vec2 { x: a.x.min(b.x) - r, y: a.y.min(b.y) - r }, Vec2 { x: a.x.max(b.x) + r, y: a.y.max(b.y) + r })
}

/// Distance of segment `a b` to polygon `poly` (0 when it touches or lies inside; a single point
/// is a point feature).
fn seg_poly_dist(a: Vec2, b: Vec2, poly: &[Vec2]) -> f64 {
    if poly.len() == 1 {
        return seg_seg_dist(a, b, poly[0], poly[0]);
    }
    if poly.len() >= 3 && (point_in_poly(a, poly) || point_in_poly(b, poly)) {
        return 0.0;
    }
    (0..poly.len()).map(|i| seg_seg_dist(a, b, poly[i], poly[(i + 1) % poly.len()])).fold(f64::INFINITY, f64::min)
}

/// Check the ERBS of a layer against each other and the sheet's features; violations by party,
/// the worst place each, classed predicted (an over-full cut carries both, or the order reports
/// the pair crossing) or `erbs_gap`. Deterministic.
pub fn check_layer(sh: &LayerSheet, sts: &[Stretch], o: &LayerOrder, es: &[Erbs]) -> Vec<Violation> {
    // Predicted pairs: over-full cuts and the order's crossing pairs.
    let mut pred_pair: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut pred_feat: BTreeSet<(usize, usize)> = BTreeSet::new();
    for cu in &o.cuts {
        if cu.squeeze <= 0.0 {
            continue;
        }
        let c = &sh.cuts[cu.cut];
        let ss: BTreeSet<usize> = cu.items.iter().map(|x| x.stretch).collect();
        for &a in &ss {
            pred_feat.insert((a, c.fp));
            pred_feat.insert((a, c.fq));
            for &b in &ss {
                pred_pair.insert((a.min(b), a.max(b)));
            }
        }
    }
    for d in &o.report.examples {
        pred_pair.insert((d.a.min(d.b), d.a.max(d.b)));
    }
    // Segments: (erbs index, point index), width, half reach.
    let segs: Vec<(usize, usize)> = es.iter().enumerate().flat_map(|(i, e)| (0..e.pts.len().saturating_sub(1)).map(move |k| (i, k))).collect();
    let smax = es.iter().map(|e| sts[e.stretch].s).fold(0.0, f64::max);
    let wmax = es.iter().flat_map(|e| e.w.iter().copied()).fold(0.0, f64::max);
    let reach = wmax + smax;
    let cell = (4.0 * reach).max(0.5e6);
    let mut grid = Grid { cell, cells: BTreeMap::new() };
    for (id, &(i, k)) in segs.iter().enumerate() {
        let (lo, hi) = bbox(es[i].pts[k], es[i].pts[k + 1], 0.0);
        grid.insert(lo, hi, id);
    }
    let seg_w = |i: usize, k: usize| es[i].w[k].min(es[i].w[k + 1]);
    let mut worst: BTreeMap<(usize, Party), Violation> = BTreeMap::new();
    let mut put = |v: Violation| {
        let e = worst.entry((v.stretch, v.other)).or_insert(v);
        if v.amount > e.amount {
            *e = v;
        }
    };
    let mut near = BTreeSet::new();
    for (id, &(i, k)) in segs.iter().enumerate() {
        let (a, b) = (es[i].pts[k], es[i].pts[k + 1]);
        let sa = &sts[es[i].stretch];
        let (lo, hi) = bbox(a, b, reach);
        grid.query(lo, hi, &mut near);
        for &jd in near.iter().filter(|&&jd| jd > id) {
            let (j, l) = segs[jd];
            let sb = &sts[es[j].stretch];
            if sa.net == sb.net {
                continue;
            }
            let req = 0.5 * (seg_w(i, k) + seg_w(j, l)) + sa.s.max(sb.s);
            let (c, d) = (es[j].pts[l], es[j].pts[l + 1]);
            let dist = seg_seg_dist(a, b, c, d);
            if dist < req - 1.0 {
                let (x, y) = (es[i].stretch.min(es[j].stretch), es[i].stretch.max(es[j].stretch));
                put(Violation { stretch: x, other: Party::Stretch(y), at: (a + b + c + d) * 0.25, amount: req - dist, predicted: pred_pair.contains(&(x, y)) });
            }
        }
    }
    // Features: pads, vias, junctions, holes (in a grid of their boxes); the outline.
    let (fgrid, fclear) = feat_grid(sh, cell);
    for &(i, k) in &segs {
        let (a, b) = (es[i].pts[k], es[i].pts[k + 1]);
        let si = es[i].stretch;
        let s = &sts[si];
        let it = Item { net: s.net, w: seg_w(i, k), s: s.s };
        let (lo, hi) = bbox(a, b, 0.5 * it.w + fclear.max(s.s));
        fgrid.query(lo, hi, &mut near);
        for &f in &near {
            let ft = &sh.feats[f];
            let cl = ft.bound.clearance(&it);
            if cl <= 0.0 {
                continue;
            }
            let req = 0.5 * it.w + cl;
            let dist = seg_poly_dist(a, b, &ft.poly);
            if dist < req - 1.0 {
                put(Violation { stretch: si, other: Party::Feature(f), at: (a + b) * 0.5, amount: req - dist, predicted: pred_feat.contains(&(si, f)) });
            }
        }
        if let Some(ef) = sh.edge_feat {
            if let Bound::Edge { clear } = sh.feats[ef].bound {
                let req = 0.5 * it.w + clear;
                for &(p, q) in &sh.outline {
                    let dist = seg_seg_dist(a, b, p, q);
                    if dist < req - 1.0 {
                        put(Violation { stretch: si, other: Party::Edge, at: (a + b) * 0.5, amount: req - dist, predicted: pred_feat.contains(&(si, ef)) });
                    }
                }
            }
        }
    }
    worst.into_values().collect()
}

/// Counts of a layer's ERBS and their violations.
pub fn report(es: &[Erbs], vs: &[Violation]) -> ErbsReport {
    let mut r = ErbsReport { stretches: es.len(), ..ErbsReport::default() };
    for e in es {
        r.gates += e.gates;
        r.not_converged += usize::from(!e.converged);
        r.refined += e.refined;
        r.max_sweeps = r.max_sweeps.max(e.sweeps);
        r.len += e.len;
        r.band_len += e.band_len;
    }
    for v in vs {
        if v.predicted {
            r.predicted += 1;
        } else {
            r.erbs_gap += 1;
            r.worst_gap = r.worst_gap.max(v.amount);
        }
    }
    r
}

/// Debug summary of one layer (mm, KiCad frame), the first `erbs_gap` places included.
pub fn summary(r: &ErbsReport, vs: &[Violation], sts: &[Stretch]) -> Value {
    let gaps: Vec<Value> = vs
        .iter()
        .filter(|v| !v.predicted)
        .take(20)
        .map(|v| {
            let other = match v.other {
                Party::Stretch(b) => json!({"wire": sts[b].wire}),
                Party::Feature(f) => json!({"feature": f}),
                Party::Edge => json!("edge"),
            };
            json!({"wire": sts[v.stretch].wire, "other": other, "at": [v.at.x / 1e6, -v.at.y / 1e6], "mm": v.amount / 1e6})
        })
        .collect();
    json!({
        "stretches": r.stretches, "gates": r.gates, "not_converged": r.not_converged, "refined": r.refined, "max_sweeps": r.max_sweeps,
        "length_over_band": if r.band_len > 0.0 { r.len / r.band_len } else { 1.0 },
        "predicted": r.predicted, "erbs_gap": r.erbs_gap, "worst_gap_mm": r.worst_gap / 1e6, "gaps": gaps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x: x * 1e6, y: y * 1e6 }
    }

    #[test]
    fn gate_minimum() {
        // Crossing: the intersection.
        let x = min_on_gate(v(0.0, 0.0), v(10.0, 0.0), v(5.0, -1.0), v(5.0, 1.0));
        assert!((x - v(5.0, 0.0)).len() < 1e-6);
        // Clamped to the near end.
        let x = min_on_gate(v(0.0, 0.0), v(10.0, 0.0), v(5.0, 1.0), v(5.0, 3.0));
        assert!((x - v(5.0, 1.0)).len() < 1e-6);
        // Both on one side: the reflection (equal angles), here the middle.
        let x = min_on_gate(v(0.0, 1.0), v(4.0, 1.0), v(-5.0, 0.0), v(5.0, 0.0));
        assert!((x - v(2.0, 0.0)).len() < 1e-6, "{x:?}");
    }

    #[test]
    fn solve_straight_and_bent() {
        let gs = [(v(2.0, -1.0), v(2.0, 1.0)), (v(5.0, 1.0), v(5.0, 3.0)), (v(8.0, -1.0), v(8.0, 1.0))];
        let init = [v(2.0, 0.5), v(5.0, 2.0), v(8.0, -0.5)];
        let (p, _, ok) = solve(v(0.0, 0.0), v(10.0, 0.0), &gs, &init, 0.01, 1000);
        assert!(ok);
        // Bends at the near end of the middle gate (5, 1); the others on the straight legs.
        assert!((p[1] - v(5.0, 1.0)).len() < 1.0, "{p:?}");
        assert!((p[0] - v(2.0, 0.4)).len() < 1.0 && (p[2] - v(8.0, 0.4)).len() < 1.0, "{p:?}");
    }

    #[test]
    fn simplify_collinear() {
        let p = simplify(&[v(0.0, 0.0), v(1.0, 0.0), v(1.0, 0.0), v(2.0, 0.0), v(2.0, 1.0)], 0.5);
        assert_eq!(p, vec![v(0.0, 0.0), v(2.0, 0.0), v(2.0, 1.0)]);
    }
}
