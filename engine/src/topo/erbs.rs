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
    /// Rounds of order flips per solve: gate pairs that meet inside both portals and on which
    /// the path is pinned (both points at the meeting point) swap their order when that makes
    /// the path shorter.
    pub flip_rounds: usize,
}

impl Default for ErbsOpts {
    fn default() -> Self {
        ErbsOpts { arc_step_deg: 22.5, tol: 0.1, sweeps_per_gate: 20, refine_rounds: 8, flip_rounds: 8 }
    }
}

/// The least distance (nm) within which both points of a free meeting count as pinned on it (the
/// gates' width when larger, [`solve_flip`]).
const PIN: f64 = 5000.0;

/// One solve of a stretch's gates in a given constraint state.
struct Solved {
    /// The gates in path order.
    og: Vec<Gate>,
    /// One point per gate of `og`.
    p: Vec<Vec2>,
    len: f64,
    sweeps: usize,
    converged: bool,
    counts: [usize; 3],
    frees: Vec<FreeMeet>,
}

/// Order the `base` gates (with `forced`) and solve from the points `pt` (by id).
fn solve_ordered(a: Vec2, e: Vec2, base: &[Gate], forced: &BTreeSet<(usize, usize)>, prio: Option<&BTreeMap<usize, usize>>, pt: &BTreeMap<usize, Vec2>, opts: &ErbsOpts) -> Solved {
    let (ord, counts, frees) = order_gates(base, forced, prio);
    let og: Vec<Gate> = ord.iter().map(|&i| base[i]).collect();
    let segs: Vec<(Vec2, Vec2)> = og.iter().map(|g| (g.a, g.b)).collect();
    // Two starts: the funnel ([`warm_start`]) and the points of the earlier solve (each gate's
    // own point `p` for a new gate); the shorter one (Gauss-Seidel only shortens).
    let funnel = warm_start(a, e, &og);
    let kept: Vec<Vec2> = og.iter().map(|g| pt.get(&g.id).copied().unwrap_or(g.p)).collect();
    let plen = |q: &[Vec2]| poly_len(&std::iter::once(a).chain(q.iter().copied()).chain(std::iter::once(e)).collect::<Vec<_>>());
    let init = if plen(&funnel) <= plen(&kept) { funnel } else { kept };
    let (p, sweeps, converged) = solve(a, e, &segs, &init, opts.tol, opts.sweeps_per_gate * segs.len() + 100);
    let mut all = Vec::with_capacity(p.len() + 2);
    all.push(a);
    all.extend(p.iter().copied());
    all.push(e);
    let len = poly_len(&all);
    Solved { og, p, len, sweeps, converged, counts, frees }
}

/// Solve, then unpin: a wrong order of two gates that meet inside both portals (a free meeting,
/// [`Meet::Free`]) pins the path on the meeting point `X` (both points there), and the other
/// order keeps that path feasible. The side of `X` the path should pass is the side of the chord
/// from the last point before the pin to the first point after it (the straight leg the path
/// takes once unpinned); from it the meeting rule ([`order_by_side`]) gives the order of every
/// pinned pair. All of a round's decisions are tried together, then one by one; a decision that
/// closes a cycle of constraints or does not shorten the path is fixed in the current order.
/// The order of a re-solve keeps the current one wherever no constraint moves it. `prio`: the
/// current order (by id) of an earlier solve.
#[allow(clippy::too_many_arguments)]
fn solve_flip(a: Vec2, e: Vec2, base: &[Gate], forced: &mut BTreeSet<(usize, usize)>, prio: Option<&BTreeMap<usize, usize>>, pt: &mut BTreeMap<usize, Vec2>, opts: &ErbsOpts, sweeps: &mut usize, flips: &mut usize) -> Solved {
    let dbg = std::env::var_os("TOPO_ERBS_DEBUG").is_some();
    let by_id: BTreeMap<usize, &Gate> = base.iter().map(|g| (g.id, g)).collect();
    let mut cur = solve_ordered(a, e, base, forced, prio, pt, opts);
    *sweeps += cur.sweeps;
    for _ in 0..opts.flip_rounds {
        let pos: BTreeMap<usize, usize> = cur.og.iter().enumerate().map(|(k, g)| (g.id, k)).collect();
        let at: BTreeMap<usize, Vec2> = cur.og.iter().zip(&cur.p).map(|(g, &x)| (g.id, x)).collect();
        let mut all = Vec::with_capacity(cur.p.len() + 2);
        all.push(a);
        all.extend(cur.p.iter().copied());
        all.push(e);
        let mut want: Vec<(usize, usize)> = Vec::new();
        let mut keep: Vec<(usize, usize)> = Vec::new();
        for &(i, j, x) in &cur.frees {
            // Pinned: both points within a wire width of the meeting point (nested_wraps: a
            // group of four gates held the wire 80 um from it, a fixed 5 um never saw the
            // wrong order; fault B). The swap is accepted only when it shortens the path.
            let pin = by_id[&i].w.max(by_id[&j].w).max(PIN);
            if (at[&i] - x).len() >= pin || (at[&j] - x).len() >= pin {
                continue;
            }
            let (f, l) = if pos[&i] < pos[&j] { (i, j) } else { (j, i) };
            if forced.contains(&(f, l)) {
                continue;
            }
            // The chord past the pin (`all` index = position + 1).
            let mut k = pos[&f] + 1;
            while k > 0 && (all[k] - x).len() < pin {
                k -= 1;
            }
            let mut m = pos[&l] + 1;
            while m + 1 < all.len() && (all[m] - x).len() < pin {
                m += 1;
            }
            let side = (all[m] - all[k]).cross(x - all[k]);
            if dbg {
                eprintln!("ERBS free {f}<{l} at ({:.3},{:.3}) side {:.3e} -> {:?}", x.x / 1e6, x.y / 1e6, side, order_by_side(by_id[&f], by_id[&l], side > 0.0));
            }
            if side.abs() <= 1e-9 * (all[m] - all[k]).len2() {
                continue;
            }
            match order_by_side(by_id[&f], by_id[&l], side > 0.0) {
                Some(false) => want.push((l, f)),
                Some(true) => keep.push((f, l)),
                None => {}
            }
        }
        forced.extend(keep);
        if want.is_empty() {
            break;
        }
        let try_set = |set: &[(usize, usize)], forced: &BTreeSet<(usize, usize)>, sweeps: &mut usize| -> (Solved, BTreeSet<(usize, usize)>) {
            let mut f2 = forced.clone();
            for &(i, j) in set {
                f2.remove(&(j, i));
                f2.insert((i, j));
            }
            let r = solve_ordered(a, e, base, &f2, Some(&pos), &at, opts);
            *sweeps += r.sweeps;
            (r, f2)
        };
        let ok = |r: &Solved, cur: &Solved| r.counts[2] <= cur.counts[2] && r.len < cur.len - 1.0;
        let (nxt, f2) = try_set(&want, forced, sweeps);
        if dbg {
            eprintln!("ERBS unpin: {} pairs {:?} len {:.1} -> {:.1} cycles {} -> {}", want.len(), &want[..want.len().min(6)], cur.len, nxt.len, cur.counts[2], nxt.counts[2]);
        }
        // The batch may also leave the length as it is: a pin of three or more gates moves to
        // the next pair, which the next round decides.
        if nxt.counts[2] <= cur.counts[2] && nxt.len <= cur.len + 1.0 {
            *flips += want.len();
            *forced = f2;
            cur = nxt;
            continue;
        }
        let mut any = false;
        for &w in &want {
            let (nxt, f2) = try_set(&[w], forced, sweeps);
            if ok(&nxt, &cur) {
                *flips += 1;
                *forced = f2;
                cur = nxt;
                any = true;
            } else {
                forced.insert((w.1, w.0));
            }
        }
        if !any {
            break;
        }
    }
    for (g, &x) in cur.og.iter().zip(&cur.p) {
        pt.insert(g.id, x);
    }
    cur
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
    /// The spoke roots: the gate lies on the segment `p q` (`p` on `a`'s side; a ray's `q` is
    /// far away). The spokes `p a` and `b q` are the obstacles the path keeps out of.
    pub p: Vec2,
    pub q: Vec2,
    /// `p` (and the spoke `p a`) lies left of the path (`cross(direction, p - x) > 0`).
    pub p_left: bool,
    /// Stable id within the stretch (the order changes, the id does not).
    pub id: usize,
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
    /// The gates in order (debug dump, `TOPO_ERBS_DUMP`).
    pub gate_list: Vec<Gate>,
    /// Of the final order ([`order_gates`]): gate pairs whose band order the meeting of their
    /// cuts reversed, pairs that contradict each other, and cycles broken.
    pub reordered: usize,
    pub conflicts: usize,
    pub cycles: usize,
    /// Free meetings whose order a flip changed ([`ErbsOpts::flip_rounds`]).
    pub flips: usize,
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
    /// [`Erbs::reordered`], [`Erbs::conflicts`], [`Erbs::cycles`], [`Erbs::flips`] summed.
    pub reordered: usize,
    pub conflicts: usize,
    pub cycles: usize,
    pub flips: usize,
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

/// The start points of [`solve`]: the funnel (string pulling, [`super::draw::funnel`]) through
/// the gates as portals, each gate's tip on the path's left side as the left end, gives the
/// taut path round the spoke tips when the order holds (the optimum bends only at tips: a
/// point inside a gate is not a bend); each gate's point is where that path crosses it (the
/// nearest gate point to the path otherwise). Gauss-Seidel then only polishes, instead of
/// creeping along a chain of hundreds of gates (O(gates^2) sweeps from a poor start).
/// O(gates x path).
fn warm_start(s: Vec2, e: Vec2, og: &[Gate]) -> Vec<Vec2> {
    let portals: Vec<(Vec2, Vec2)> = og.iter().map(|g| if g.p_left { (g.a, g.b) } else { (g.b, g.a) }).collect();
    let path = super::draw::funnel(s, e, &portals);
    let mut k = 0usize;
    og.iter()
        .map(|g| {
            // The first path segment from `k` on that crosses the gate.
            let from = k;
            for j in from..path.len().saturating_sub(1) {
                let (c, d) = (path[j], path[j + 1]);
                let (r, x, _) = seg_seg(c, d, g.a, g.b);
                if r <= 1.0 {
                    k = j;
                    return x;
                }
            }
            // Else the gate point nearest the path from `k` on.
            let mut best = (f64::INFINITY, g.a);
            for j in k..path.len().saturating_sub(1) {
                let (r, _, y) = seg_seg(path[j], path[j + 1], g.a, g.b);
                if r < best.0 {
                    best = (r, y);
                }
            }
            best.1
        })
        .collect()
}

/// The best pair of points on two consecutive gates between `pa` and `pb`: minimises
/// `|pa - x| + |x - y| + |y - pb|` with `x` on gate `g` and `y` on gate `h`. The function of
/// `x`'s gate parameter with the best `y` for it ([`min_on_gate`]) is convex (a partial minimum
/// of a jointly convex function), so a golden-section search finds it. O(100).
fn min_on_gates(pa: Vec2, pb: Vec2, g: (Vec2, Vec2), h: (Vec2, Vec2)) -> (Vec2, Vec2) {
    let at = |t: f64| g.0 + (g.1 - g.0) * t;
    let f = |t: f64| {
        let x = at(t);
        let y = min_on_gate(x, pb, h.0, h.1);
        ((pa - x).len() + (x - y).len() + (y - pb).len(), x, y)
    };
    let r = 0.5 * (5f64.sqrt() - 1.0);
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    let glen = (g.1 - g.0).len().max(1.0);
    let (mut c, mut d) = (hi - r * (hi - lo), lo + r * (hi - lo));
    let (mut fc, mut fd) = (f(c).0, f(d).0);
    for _ in 0..200 {
        if (hi - lo) * glen < 0.01 {
            break;
        }
        if fc <= fd {
            hi = d;
            d = c;
            fd = fc;
            c = hi - r * (hi - lo);
            fc = f(c).0;
        } else {
            lo = c;
            c = d;
            fc = fd;
            d = lo + r * (hi - lo);
            fd = f(d).0;
        }
    }
    let (_, x, y) = f(0.5 * (lo + hi));
    (x, y)
}

/// Symmetric Gauss-Seidel sweeps ([`min_on_gate`]) with the pair moves ([`min_on_gates`]), at
/// most `max_sweeps`: (sweeps, converged).
fn exact_sweeps(s: Vec2, e: Vec2, gates: &[(Vec2, Vec2)], p: &mut [Vec2], tol: f64, max_sweeps: usize) -> (usize, bool) {
    let m = gates.len();
    let mut sweeps = 0;
    let len = |p: &[Vec2]| (s - p.first().copied().unwrap_or(e)).len() + p.windows(2).map(|w| (w[1] - w[0]).len()).sum::<f64>() + (p.last().copied().unwrap_or(s) - e).len();
    let mut last = f64::INFINITY;
    let mut stalled = false;
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
        // The pair moves at convergence, and every 25 sweeps: a stalled pair keeps its
        // neighbours creeping (each sweep moves a point more than `tol`), so the sweeps alone
        // may never converge (nested_wraps: the sweep budget ran out on all three wires).
        let creeping = moved > tol;
        if !creeping || sweeps % 25 == 0 {
            // Every consecutive pair, not only points closer than a threshold: two points a
            // micrometre apart on gates that cross there stall as firmly as coincident ones
            // (nested_wraps: a cut and an arc spoke on one line, the points 1 um apart, held
            // the wire 15 um longer and bent outward; fault B). O(gates x 100) per pass.
            for i in 0..m.saturating_sub(1) {
                let pa = if i == 0 { s } else { p[i - 1] };
                let pb = if i + 2 == m { e } else { p[i + 2] };
                let old = (pa - p[i]).len() + (p[i] - p[i + 1]).len() + (p[i + 1] - pb).len();
                let (x, y) = min_on_gates(pa, pb, gates[i], gates[i + 1]);
                let new = (pa - x).len() + (x - y).len() + (y - pb).len();
                if new < old - 0.5 * tol {
                    moved = moved.max((x - p[i]).len()).max((y - p[i + 1]).len());
                    p[i] = x;
                    p[i + 1] = y;
                }
            }
            if !creeping && moved <= tol {
                return (sweeps, true);
            }
            // The length stopped falling (less than 1 nm in 25 sweeps): points still creeping
            // along a flat stretch of the objective change nothing that matters, so a sweep
            // budget that ends there counts as converged. The sweeps go on (stopping there
            // early left three_diagonals' S 3.5 % longer once drawn).
            let l = len(p);
            stalled = last - l < 1.0;
            last = l;
        }
    }
    (sweeps, m == 0 || stalled)
}

/// The shortest path from `s` to `e` through `gates` in order, from the initial points `init`
/// (one per gate): symmetric Gauss-Seidel sweeps until no point moves more than `tol`, at most
/// `max_sweeps`. Returns the points, the sweeps and whether it converged. O(gates x sweeps).
///
/// The objective is convex but not smooth: two neighbouring points that meet where their gates
/// cross stall coordinate descent (each is already the best point given the other), although
/// the path bends there; such pairs are moved together ([`exact_sweeps`]). (A smoothed length,
/// `sqrt(d^2 + eps^2)` annealed, was tried for longer groups: slower, more not converged, no
/// gain on nested_wraps.)
pub fn solve(s: Vec2, e: Vec2, gates: &[(Vec2, Vec2)], init: &[Vec2], tol: f64, max_sweeps: usize) -> (Vec<Vec2>, usize, bool) {
    let mut p: Vec<Vec2> = init.to_vec();
    let (sweeps, converged) = exact_sweeps(s, e, gates, &mut p, tol, max_sweeps);
    (p, sweeps, converged)
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
/// feature of another net: (breach, feature, gate start, outward direction, the feature's closest
/// point). O(features near).
/// Features in `skip` (ends of an over-full cut the stretch is on: the plan squeezed that
/// clearance on purpose, the tips hold it) are left out. A gate sits 2 nm past the requirement
/// (the solver's tolerance).
#[allow(clippy::too_many_arguments)]
fn breach(sh: &LayerSheet, g: &Grid, fclear: f64, s: &Stretch, w: f64, a: Vec2, b: Vec2, skip: &BTreeSet<usize>, near: &mut BTreeSet<usize>) -> Option<(f64, usize, Vec2, Vec2, Vec2)> {
    let it = Item { net: s.net, w, s: s.s };
    let (lo, hi) = bbox(a, b, 0.5 * w + fclear.max(s.s));
    g.query(lo, hi, near);
    let mut best: Option<(f64, usize, Vec2, Vec2, Vec2)> = None;
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
        best = Some((req - d, f, c + u * (req + 2.0), u, c));
    }
    best
}

/// Where the ray `at + t u` (`t` in `0..FAR`) first meets a feature other than `own`: (`t`, the
/// feature). Features met by `at` itself are left out (the ray starts on the clearance ring of
/// `own`, inside nothing else unless the plan is already short of room). Searched in growing
/// boxes (2 mm, x4) through the feature grid. O(features near x sides).
fn ray_hit(sh: &LayerSheet, g: &Grid, at: Vec2, u: Vec2, own: usize, near: &mut BTreeSet<usize>) -> Option<(f64, usize)> {
    let mut reach: f64 = 2.0e6;
    loop {
        let reach_c = reach.min(FAR);
        let end = at + u * reach_c;
        let (lo, hi) = bbox(at, end, 0.0);
        g.query(lo, hi, near);
        let mut best: Option<(f64, usize)> = None;
        for &f in near.iter() {
            let poly = &sh.feats[f].poly;
            if f == own || poly.len() < 3 || point_in_poly(at, poly) {
                continue;
            }
            for i in 0..poly.len() {
                let (c, d) = (poly[i], poly[(i + 1) % poly.len()]);
                let e = d - c;
                let den = u.cross(e);
                if den.abs() < 1e-12 * e.len().max(1.0) {
                    continue;
                }
                let w0 = c - at;
                let (t, r) = (w0.cross(e) / den, w0.cross(u) / den);
                if (0.0..=reach_c).contains(&t) && (0.0..=1.0).contains(&r) && best.is_none_or(|b| t < b.0) {
                    best = Some((t, f));
                }
            }
        }
        if best.is_some() || reach_c >= FAR {
            return best;
        }
        reach *= 4.0;
    }
}

/// The gates of stretch `si` (unordered), with the item index of every cut item.
#[allow(clippy::too_many_arguments)]
fn gates_of(sh: &LayerSheet, fg: &Grid, s: &Stretch, si: usize, o: &LayerOrder, items: &[(usize, usize)], radius: &BTreeMap<(usize, usize, usize), f64>, opts: &ErbsOpts) -> Vec<Gate> {
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
        let mut p_left = false;
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
                            p_left = d.cross(c.p - band[j]) > 0.0;
                        }
                    }
                }
                (best.map_or(0.0, |bb| bb.1), 0.5)
            }
            (k, Some(cr)) => {
                let u = if k == 1 { c.q - c.p } else { c.p - c.q };
                // The wrapped vertex is `p` (kind 1) or `q` (kind 2), on the wrap's side.
                let left = s.wraps[cr].is_some_and(|w| w.1);
                p_left = if k == 1 { left } else { !left };
                (cr as f64, corner_share(cr, u).unwrap_or(0.5))
            }
            _ => continue,
        };
        gs.push(Gate { a, b, w: x.w_tip, src: GateSrc::Cut(ci), key, p: c.p, q: c.q, p_left, id: 0 });
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
    let mut near = BTreeSet::new();
    #[allow(clippy::needless_range_loop)]
    for k in 1..band.len().saturating_sub(1) {
        let Some((v, left)) = s.wraps[k] else { continue };
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
                let a = at + u * (rho * grow).max(t);
                // The spoke ends at the first obstacle beyond (Dayan 1997 PDF p. 33, Fig. 11),
                // the portal at that obstacle's clearance before it; a ray over the board would
                // meet gates of unrelated places.
                let own = sh.vfeat[v].unwrap_or(usize::MAX);
                let (b, q) = match ray_hit(sh, fg, a, u, own, &mut near) {
                    Some((t, h)) => {
                        let cl = 0.5 * w + sh.feats[h].bound.clearance(&Item { net: s.net, w, s: s.s }).max(0.0);
                        (a + u * (t - cl - 2.0).max(1.0), a + u * t)
                    }
                    None => (a + u * FAR, a + u * FAR),
                };
                gs.push(Gate { a, b, w, src: GateSrc::Arc(v), key: (k as f64, f), p: at, q, p_left: left, id: 0 });
            }
        }
    }
    gs.sort_by(|a, b| a.key.0.total_cmp(&b.key.0).then(a.key.1.total_cmp(&b.key.1)));
    for (i, g) in gs.iter_mut().enumerate() {
        g.id = i;
    }
    gs
}

/// Parameter of `x` along `p q` (0 at `p`, 1 at `q`).
fn param(p: Vec2, q: Vec2, x: Vec2) -> f64 {
    let e = q - p;
    let l2 = e.len2();
    if l2 > 0.0 {
        (x - p).dot(e) / l2
    } else {
        0.0
    }
}

/// How two gates constrain their order along the path ([`gate_before`]).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Meet {
    /// Their segments `p q` do not meet: the band's order holds. The reduced sequence of
    /// disjoint obstacle-to-obstacle segments a path crosses is a homotopy invariant (as the
    /// triangle edges of a sleeve), so the ERBS crosses them in the band's order.
    Disjoint,
    /// They meet where the side of the meeting point is known: `true` when the first gate comes
    /// first.
    Before(bool),
    /// They meet, but the meeting point does not fix the order: inside both portals (either
    /// order is possible), sides that contradict each other (counted), or a gate with an empty
    /// spoke (a cut from the stretch's own terminal or junction: the path may cross it more
    /// than once) met inside its portal.
    Free,
}

/// Which of two gates the path crosses first, from where their segments `p q` meet.
///
/// The meeting point `X` lies on a spoke of one of them (outside its portal), so its side of the
/// path is known: the side of that spoke's root. With `e` the direction of a gate from the
/// path's left to its right, a path with `X` on its left crosses `g` before `h` when
/// `cross(e_g, e_h) > 0`, with `X` on its right when `cross(e_g, e_h) < 0` (the path, `X` and
/// the two crossings bound a triangle of that orientation). This is the order of the cuts along
/// the ERBS; the band's own order can differ where cuts meet between the band and the ERBS
/// (near a wrapped corner, the ERBS is a stack radius away from the band). Cuts cross each other
/// freely (they are not a planar subdivision, Maley 1987), so the order along the band is not a
/// homotopy invariant for them.
fn gate_before(g: &Gate, h: &Gate, conflict: &mut usize) -> Meet {
    let (d1, d2) = (g.q - g.p, h.q - h.p);
    let den = d1.cross(d2);
    if den.abs() <= 1e-12 * d1.len() * d2.len() {
        return Meet::Disjoint;
    }
    let w0 = h.p - g.p;
    let (t, u) = (w0.cross(d2) / den, w0.cross(d1) / den);
    const E: f64 = 1e-9;
    if !(-E..=1.0 + E).contains(&t) || !(-E..=1.0 + E).contains(&u) {
        return Meet::Disjoint;
    }
    // The side of X by one gate: left / right of the path, or unknown (inside the portal).
    let side = |gg: &Gate, tt: f64| -> Option<bool> {
        let (ta, tb) = (param(gg.p, gg.q, gg.a), param(gg.p, gg.q, gg.b));
        if tt < ta.min(tb) - E {
            Some(gg.p_left)
        } else if tt > ta.max(tb) + E {
            Some(!gg.p_left)
        } else {
            None
        }
    };
    let (sg, sh) = (side(g, t), side(h, u));
    // A gate with an empty spoke (its tip on the root: a cut from the stretch's own terminal or
    // junction) may be crossed more than once (the path starts inside the root's pad, which has
    // no side), so the place of its crossing follows from a meeting only when the meeting point
    // lies on its own other spoke.
    let empty = |gg: &Gate| (gg.a - gg.p).len() < 1.0 || (gg.q - gg.b).len() < 1.0;
    if (empty(g) && sg.is_none()) || (empty(h) && sh.is_none()) {
        return Meet::Free;
    }
    let x_left = match (sg, sh) {
        (Some(a), Some(b)) if a != b => {
            *conflict += 1;
            return Meet::Free;
        }
        (Some(a), _) | (None, Some(a)) => a,
        (None, None) => return Meet::Free,
    };
    order_by_side(g, h, x_left).map_or(Meet::Free, Meet::Before)
}

/// The order of two meeting gates when their meeting point lies on the given side of the path
/// (the last step of [`gate_before`]): `Some(true)` when `g` comes first.
fn order_by_side(g: &Gate, h: &Gate, x_left: bool) -> Option<bool> {
    let (d1, d2) = (g.q - g.p, h.q - h.p);
    let e1 = if g.p_left { d1 } else { -d1 };
    let e2 = if h.p_left { d2 } else { -d2 };
    let c = e1.cross(e2);
    (c != 0.0).then_some((c > 0.0) == x_left)
}

/// A meeting of two gates inside both portals: the ids and the meeting point. Which side of it
/// the path passes decides their order ([`erbs_layer`] flips pinned pairs).
type FreeMeet = (usize, usize, Vec2);

/// The gates in the order the path crosses them: the band order (`gs` as given) for gates that
/// do not meet, the meeting point's order for gates that do ([`gate_before`]), `forced` pairs
/// (ids, first before second) over both. Kahn's topological sort of the pairwise constraints;
/// among the free gates the earliest by `prio` (ids to rank; else the given order) first; a
/// cycle (contradicting constraints) is broken at the earliest remaining gate. Returns the
/// order, the counts (meeting constraints against the given order; conflicts; cycle breaks) and
/// the meetings inside both portals. O(n^2) pair tests.
pub fn order_gates(gs: &[Gate], forced: &BTreeSet<(usize, usize)>, prio: Option<&BTreeMap<usize, usize>>) -> (Vec<usize>, [usize; 3], Vec<FreeMeet>) {
    let n = gs.len();
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    let mut conflict = 0usize;
    let mut frees = Vec::new();
    let boxes: Vec<(Vec2, Vec2)> = gs.iter().map(|g| bbox(g.p, g.q, 0.0)).collect();
    let mut against = 0usize;
    for i in 0..n {
        for j in i + 1..n {
            let (a, b) = (boxes[i], boxes[j]);
            let meet = if forced.contains(&(gs[i].id, gs[j].id)) {
                Meet::Before(true)
            } else if forced.contains(&(gs[j].id, gs[i].id)) {
                Meet::Before(false)
            } else if a.1.x < b.0.x || b.1.x < a.0.x || a.1.y < b.0.y || b.1.y < a.0.y {
                Meet::Disjoint
            } else {
                let m = gate_before(&gs[i], &gs[j], &mut conflict);
                if m == Meet::Free {
                    if let Some(x) = meet_point(&gs[i], &gs[j]) {
                        frees.push((gs[i].id, gs[j].id, x));
                    }
                }
                if matches!(m, Meet::Before(false)) {
                    against += 1;
                }
                m
            };
            match meet {
                Meet::Disjoint | Meet::Before(true) => {
                    succ[i].push(j);
                    indeg[j] += 1;
                }
                Meet::Before(false) => {
                    succ[j].push(i);
                    indeg[i] += 1;
                }
                Meet::Free => {}
            }
        }
    }
    // Among the free gates the earliest by `prio` (the current order, so a flip moves only what
    // it must), else by the given order.
    let rank: Vec<usize> = (0..n).map(|i| prio.and_then(|m| m.get(&gs[i].id).copied()).unwrap_or(i)).collect();
    let mut free: BTreeSet<(usize, usize)> = (0..n).filter(|&i| indeg[i] == 0).map(|i| (rank[i], i)).collect();
    let mut done = vec![false; n];
    let mut out = Vec::with_capacity(n);
    let mut cycles = 0usize;
    while out.len() < n {
        let i = match free.pop_first() {
            Some((_, i)) => i,
            None => {
                cycles += 1;
                (0..n).filter(|&i| !done[i]).min_by_key(|&i| (rank[i], i)).expect("a gate left")
            }
        };
        if done[i] {
            continue;
        }
        done[i] = true;
        out.push(i);
        for &j in &succ[i] {
            indeg[j] = indeg[j].saturating_sub(1);
            if indeg[j] == 0 && !done[j] {
                free.insert((rank[j], j));
            }
        }
    }
    (out, [against, conflict, cycles], frees)
}

/// Where the segments `p q` of two gates meet, if they do.
fn meet_point(g: &Gate, h: &Gate) -> Option<Vec2> {
    let (d1, d2) = (g.q - g.p, h.q - h.p);
    let den = d1.cross(d2);
    if den.abs() <= 1e-12 * d1.len() * d2.len() {
        return None;
    }
    let w0 = h.p - g.p;
    let (t, u) = (w0.cross(d2) / den, w0.cross(d1) / den);
    ((-1e-9..=1.0 + 1e-9).contains(&t) && (-1e-9..=1.0 + 1e-9).contains(&u)).then(|| g.p + d1 * t)
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
            let mut base = gates_of(sh, &fg, s, si, o, per.get(&si).unwrap_or(&none), &radius, opts);
            let (a, e) = (s.band[0], s.band[s.band.len() - 1]);
            // Start on the band: the gate point nearest the band's own crossing (a for arcs).
            let mut pt: BTreeMap<usize, Vec2> = base
                .iter()
                .map(|g| {
                    let x = match g.src {
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
                    };
                    (g.id, x)
                })
                .collect();
            let mut forced: BTreeSet<(usize, usize)> = BTreeSet::new();
            let mut sweeps = 0;
            let mut flips = 0usize;
            let mut cur = solve_flip(a, e, &base, &mut forced, None, &mut pt, opts, &mut sweeps, &mut flips);
            // Feature refinement: a chord between two gates may come nearer a feature than its
            // bare clearance (the spokes sample the plane at the cuts only): a gate off that
            // feature, outward from its closest point, then solve again from where it was.
            let mut refined = 0;
            let mut near = BTreeSet::new();
            let mut spokes: BTreeSet<(usize, usize, usize)> = BTreeSet::new();
            for _ in 0..opts.refine_rounds {
                let (og, p) = (&cur.og, &cur.p);
                let mut add: Vec<(usize, Gate)> = Vec::new();
                for k in 0..=p.len() {
                    let x0 = if k == 0 { a } else { p[k - 1] };
                    let x1 = if k == p.len() { e } else { p[k] };
                    let w0 = if k == 0 { s.w } else { og[k - 1].w };
                    let w1 = if k == p.len() { s.w } else { og[k].w };
                    if let Some((_, f, at, u, c)) = breach(sh, &fg, fclear, s, w0.min(w1), x0, x1, squeezed.get(&si).unwrap_or(&no_skip), &mut near) {
                        // Octilinear spokes of the feature's vertex nearest the breach (Dayan
                        // 1997 PDF p. 33, Fig. 11: up to 8 spokes per point, octilinear, each to
                        // the nearest obstacle; they approximate the arc round the point). The
                        // one or two nearest the outward direction `u`; their portals start on
                        // the octagon about the vertex (in-radius the requirement, vertex radius
                        // req / cos 22.5 deg, so a chord between two neighbouring spokes keeps
                        // the clearance). A spoke is added once per stretch (feature, vertex,
                        // direction): at most 8 per vertex, so the refinement ends. The earlier
                        // gate (a ray from the closest point, along `u`, to the board's end)
                        // met every gate of the stretch, its meeting constraints contradicted
                        // the others' and each round's `u` differed a little (three_diagonals:
                        // 274 k conflicts, 1002 cycle breaks, 1031 gates for one wire): fault A.
                        let wd = w0.min(w1);
                        let req = (at - c).len() - 2.0;
                        let poly = &sh.feats[f].poly;
                        let vi = (0..poly.len()).min_by(|&i, &j| (poly[i] - c).len2().total_cmp(&(poly[j] - c).len2())).unwrap_or(0);
                        let v = poly.get(vi).copied().unwrap_or(c);
                        let rv = req / FRAC_PI_8.cos() + 2.0;
                        let after = if k == 0 { 0 } else { base.iter().position(|g| g.id == og[k - 1].id).map_or(0, |i| i + 1) };
                        for dk in 0..8usize {
                            let ang = dk as f64 * 2.0 * FRAC_PI_8;
                            let d = Vec2 { x: ang.cos(), y: ang.sin() };
                            if d.dot(u) < std::f64::consts::FRAC_1_SQRT_2 - 1e-9 || !spokes.insert((f, vi, dk)) {
                                continue;
                            }
                            let ga = v + d * rv;
                            // The spoke root lies on the side of the chord away from the spoke.
                            let p_left = (x1 - x0).cross(-d) > 0.0;
                            // The portal ends the far obstacle's clearance before it.
                            let (gb, q) = match ray_hit(sh, &fg, ga, d, f, &mut near) {
                                Some((t, h)) => {
                                    let cl = 0.5 * wd + sh.feats[h].bound.clearance(&Item { net: s.net, w: wd, s: s.s }).max(0.0);
                                    (ga + d * (t - cl - 2.0).max(1.0), ga + d * t)
                                }
                                None => (ga + d * FAR, ga + d * FAR),
                            };
                            let id = base.len() + add.len();
                            add.push((after, Gate { a: ga, b: gb, w: wd, src: GateSrc::Feature(f), key: (0.0, 0.0), p: v, q, p_left, id }));
                            pt.insert(id, ga);
                        }
                    }
                }
                if add.is_empty() {
                    break;
                }
                add.sort_by_key(|x| std::cmp::Reverse(x.0));
                for (after, g) in add {
                    base.insert(after, g);
                    refined += 1;
                }
                let prio: BTreeMap<usize, usize> = cur.og.iter().enumerate().map(|(k, g)| (g.id, k)).collect();
                cur = solve_flip(a, e, &base, &mut forced, Some(&prio), &mut pt, opts, &mut sweeps, &mut flips);
            }
            let (gs, p, converged) = (cur.og, cur.p, cur.converged);
            let counts = [cur.counts[0], cur.counts[1], cur.counts[2], flips];
            let mut pts = Vec::with_capacity(p.len() + 2);
            pts.push(a);
            pts.extend(p);
            pts.push(e);
            let mut w = Vec::with_capacity(pts.len());
            w.push(s.w);
            w.extend(gs.iter().map(|g| g.w));
            w.push(s.w);
            let path = simplify(&pts, 0.5);
            Erbs { stretch: si, len: poly_len(&path), band_len: poly_len(&s.band), pts, w, path, gates: gs.len(), refined, sweeps, converged, gate_list: gs, reordered: counts[0], conflicts: counts[1], cycles: counts[2], flips: counts[3] }
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
        r.reordered += e.reordered;
        r.conflicts += e.conflicts;
        r.cycles += e.cycles;
        r.flips += e.flips;
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

/// Debug dump of one layer (`TOPO_ERBS_DUMP`, mm, engine frame): features, bands, gates with
/// their source, the solved points and the violations.
pub fn dump(sh: &LayerSheet, sts: &[Stretch], es: &[Erbs], vs: &[Violation]) -> Value {
    let mm = |p: Vec2| json!([p.x / 1e6, p.y / 1e6]);
    let feats: Vec<Value> = sh.feats.iter().map(|f| json!(f.poly.iter().map(|&p| mm(p)).collect::<Vec<_>>())).collect();
    let ers: Vec<Value> = es
        .iter()
        .map(|e| {
            let s = &sts[e.stretch];
            let gates: Vec<Value> = e
                .gate_list
                .iter()
                .map(|g| {
                    let (k, i) = match g.src {
                        GateSrc::Cut(c) => ("cut", c),
                        GateSrc::Arc(v) => ("arc", v),
                        GateSrc::Feature(f) => ("feat", f),
                    };
                    let b = if (g.b - g.a).len() > 1e8 { g.a + (g.b - g.a) * (2e6 / (g.b - g.a).len()) } else { g.b };
                    json!({"a": mm(g.a), "b": mm(b), "src": k, "id": i, "gid": g.id, "key": [g.key.0, g.key.1], "w": g.w / 1e6, "p": mm(g.p), "q": mm(if (g.q - g.p).len() > 1e8 { g.p + (g.q - g.p) * (2e7 / (g.q - g.p).len()) } else { g.q }), "p_left": g.p_left})
                })
                .collect();
            json!({"stretch": e.stretch, "wire": s.wire, "net": s.net, "w": s.w / 1e6, "s": s.s / 1e6, "band": s.band.iter().map(|&p| mm(p)).collect::<Vec<_>>(),
                   "wraps": s.wraps.iter().map(|w| w.map(|(v, l)| json!([v, l]))).collect::<Vec<_>>(),
                   "pts": e.pts.iter().map(|&p| mm(p)).collect::<Vec<_>>(), "path": e.path.iter().map(|&p| mm(p)).collect::<Vec<_>>(),
                   "gates": gates, "refined": e.refined, "converged": e.converged, "reordered": e.reordered, "conflicts": e.conflicts, "len": e.len / 1e6, "band_len": e.band_len / 1e6})
        })
        .collect();
    let viol: Vec<Value> = vs.iter().map(|v| json!({"stretch": v.stretch, "other": format!("{:?}", v.other), "at": mm(v.at), "mm": v.amount / 1e6, "predicted": v.predicted})).collect();
    json!({"layer": sh.l, "feats": feats, "outline": sh.outline.iter().map(|&(a, b)| json!([mm(a), mm(b)])).collect::<Vec<_>>(), "erbs": ers, "violations": viol})
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
        "reordered": r.reordered, "conflicts": r.conflicts, "cycles": r.cycles, "flips": r.flips,
        "length_over_band": if r.band_len > 0.0 { r.len / r.band_len } else { 1.0 },
        "predicted": r.predicted, "erbs_gap": r.erbs_gap, "worst_gap_mm": r.worst_gap / 1e6, "gaps": gaps,
    })
}

/// One run of a wire as its ERBS (`[drawing] realise = "erbs"`): layer, end points, the path.
#[derive(Clone, Debug)]
pub struct RunBand {
    pub layer: usize,
    pub s: Vec2,
    pub e: Vec2,
    pub path: Vec<Vec2>,
    /// The check found a violation no over-full cut predicts on this band (an `erbs_gap`).
    pub gap: bool,
}

/// The ERBS of every wire for the drawing, its precedes order and the counts.
#[derive(Clone, Debug, Default)]
pub struct Realised {
    pub runs: BTreeMap<usize, Vec<RunBand>>,
    /// The wires in the precedes order ([`precedes`]), ties in the order given.
    pub order: Vec<usize>,
    pub report: ErbsReport,
    /// Stretches the measurement could not band (walk failed): drawn the sequential way.
    pub unbanded: usize,
    pub seconds: f64,
}

/// The precedes order (Staepelaere 1992, OrderSegments, PDF p. 29: a wire is made octilinear
/// after every wire between it and what it bends round, so each one has its "previous boundary"
/// drawn, PDF pp. 18, 33-34): a wire that bends at a spoke tip of a cut comes after the items
/// of that cut between it and that end, one that bends on a stack arc after the stack's inner
/// members. Kahn's sort over the wires with the given order as the tie-break, a cycle broken at
/// the earliest. O(bends x items of the cut + wires^2 worst).
pub fn precedes(es: &[Erbs], sts: &[Stretch], o: &LayerOrder, given: &[usize], before: &mut BTreeSet<(usize, usize)>) {
    for e in es {
        let wire = sts[e.stretch].wire;
        for (k, g) in e.gate_list.iter().enumerate() {
            let x = e.pts[k + 1];
            let (u, v) = (e.pts[k], e.pts[k + 2]);
            if (v - x).cross(x - u).abs() <= 1e-6 * (v - x).len() * (x - u).len() + 1.0 {
                continue;
            }
            let at_a = (x - g.a).len() < 2.0;
            let at_b = (x - g.b).len() < 2.0;
            if !at_a && !at_b {
                continue;
            }
            match g.src {
                GateSrc::Cut(ci) => {
                    let items = &o.cuts[ci].items;
                    let Some(ii) = items.iter().position(|it| it.stretch == e.stretch) else { continue };
                    let range = if at_a { 0..ii } else { ii + 1..items.len() };
                    for it in &items[range] {
                        let w2 = sts[it.stretch].wire;
                        if w2 != wire {
                            before.insert((w2, wire));
                        }
                    }
                }
                GateSrc::Arc(v) => {
                    for st in o.stacks.iter().filter(|st| st.v == v) {
                        if let Some(i) = st.order.iter().position(|&m| m == e.stretch) {
                            for &m in &st.order[..i] {
                                let w2 = sts[m].wire;
                                if w2 != wire {
                                    before.insert((w2, wire));
                                }
                            }
                        }
                    }
                }
                GateSrc::Feature(_) => {}
            }
        }
    }
    let _ = given;
}

/// Topological order of `given` under `before` (pairs of wires), the given order as tie-break.
pub fn precedes_order(given: &[usize], before: &BTreeSet<(usize, usize)>) -> Vec<usize> {
    let pos: BTreeMap<usize, usize> = given.iter().enumerate().map(|(i, &w)| (w, i)).collect();
    let n = given.len();
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    for &(a, b) in before {
        if let (Some(&i), Some(&j)) = (pos.get(&a), pos.get(&b)) {
            if i != j {
                succ[i].push(j);
                indeg[j] += 1;
            }
        }
    }
    let mut free: BTreeSet<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut done = vec![false; n];
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let i = free.pop_first().unwrap_or_else(|| (0..n).find(|&i| !done[i]).expect("left"));
        if done[i] {
            continue;
        }
        done[i] = true;
        out.push(given[i]);
        for &j in &succ[i] {
            indeg[j] = indeg[j].saturating_sub(1);
            if indeg[j] == 0 && !done[j] {
                free.insert(j);
            }
        }
    }
    out
}

/// The ERBS of every wire for the drawing (`[drawing] realise = "erbs"`): the plan measured with
/// the drawing's placed vias (sheets with the via discs, bands, cut items), the order, tips and
/// radii (steps a, b), the ERBS per layer and their check, and the precedes order of `given`.
#[allow(clippy::too_many_arguments)]
pub fn realise(st: &super::state::State, events: &[Vec<super::refine::LEv>], packer: &super::pack::Packer, bottom: i32, conn: &dyn Fn(usize) -> Option<usize>, placed: &BTreeMap<usize, Vec<(usize, Vec2)>>, outline: &[(Vec2, Vec2)], mo: &super::cutflow::MeasureOpts, layers: &[usize], given: &[usize]) -> Result<Realised, String> {
    let t0 = std::time::Instant::now();
    let none: Vec<Vec<usize>> = Vec::new();
    let plan = super::cutflow::Plan { st, events, via_k: &none, bottom, packer, conn, placed: Some(placed) };
    let m = super::cutflow::measure(&plan, outline, mo, layers)?;
    let ords = super::radii::from_measure(&m, &|p| st.proj.regime_at(p));
    let mut out = Realised::default();
    let mut before: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (lf, o) in m.layers.iter().zip(&ords) {
        out.unbanded += lf.walk_failed;
        let es = erbs_layer(&lf.sheet, &m.stretches, &lf.stretches, o, &ErbsOpts::default());
        let vs = check_layer(&lf.sheet, &m.stretches, o, &es);
        let r = report(&es, &vs);
        let gap: BTreeSet<usize> = vs.iter().filter(|v| !v.predicted).flat_map(|v| match v.other {
            Party::Stretch(b) => vec![v.stretch, b],
            _ => vec![v.stretch],
        }).collect();
        precedes(&es, &m.stretches, o, given, &mut before);
        for e in &es {
            let s = &m.stretches[e.stretch];
            out.runs.entry(s.wire).or_default().push(RunBand { layer: s.layer, s: e.pts[0], e: e.pts[e.pts.len() - 1], path: e.path.clone(), gap: gap.contains(&e.stretch) });
        }
        let t = &mut out.report;
        t.stretches += r.stretches;
        t.gates += r.gates;
        t.not_converged += r.not_converged;
        t.refined += r.refined;
        t.reordered += r.reordered;
        t.conflicts += r.conflicts;
        t.cycles += r.cycles;
        t.flips += r.flips;
        t.max_sweeps = t.max_sweeps.max(r.max_sweeps);
        t.len += r.len;
        t.band_len += r.band_len;
        t.predicted += r.predicted;
        t.erbs_gap += r.erbs_gap;
        t.worst_gap = t.worst_gap.max(r.worst_gap);
    }
    out.order = precedes_order(given, &before);
    out.seconds = t0.elapsed().as_secs_f64();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x: x * 1e6, y: y * 1e6 }
    }

    /// A gate on the cut `p q` with the portal `a b`.
    fn gate(p: Vec2, q: Vec2, a: Vec2, b: Vec2, p_left: bool, id: usize) -> Gate {
        Gate { a, b, w: 0.0, src: GateSrc::Cut(id), key: (id as f64, 0.0), p, q, p_left, id }
    }

    #[test]
    fn meeting_on_a_spoke_orders_the_gates() {
        // The path runs along +x. `g` (vertical, x = 3) and `h` (slanted, crossing the path at
        // x = 6) meet at (3, 3): on both upper spokes, left of the path, so `g` comes first.
        let g = gate(v(3.0, 5.0), v(3.0, -5.0), v(3.0, 1.0), v(3.0, -1.0), true, 0);
        let h = gate(v(2.0, 4.0), v(8.0, -2.0), v(5.0, 1.0), v(7.0, -1.0), true, 1);
        let mut c = 0;
        assert_eq!(gate_before(&g, &h, &mut c), Meet::Before(true));
        assert_eq!(gate_before(&h, &g, &mut c), Meet::Before(false));
        assert_eq!(c, 0);
        // The band gives them the wrong way round: the meeting rule wins.
        let (ord, counts, frees) = order_gates(&[h, g], &BTreeSet::new(), None);
        assert_eq!(ord, vec![1, 0]);
        assert_eq!(counts, [1, 0, 0]);
        assert!(frees.is_empty());
    }

    #[test]
    fn meeting_inside_both_portals_is_free() {
        let g = gate(v(3.0, 5.0), v(3.0, -5.0), v(3.0, 1.0), v(3.0, -1.0), true, 0);
        let h = gate(v(1.0, 3.0), v(5.0, -3.0), v(1.0 + 4.0 / 3.0, 1.0), v(1.0 + 8.0 / 3.0, -1.0), true, 1);
        let mut c = 0;
        assert_eq!(gate_before(&g, &h, &mut c), Meet::Free);
        let (_, _, frees) = order_gates(&[g, h], &BTreeSet::new(), None);
        assert_eq!(frees.len(), 1);
        assert!((frees[0].2 - v(3.0, 0.0)).len() < 1.0);
        // A forced pair overrides the geometry.
        let forced: BTreeSet<(usize, usize)> = [(1, 0)].into_iter().collect();
        assert_eq!(order_gates(&[g, h], &forced, None).0, vec![1, 0]);
    }

    #[test]
    fn disjoint_gates_keep_the_band_order() {
        // Parallel cuts never meet: the band order holds, whatever the geometry suggests (the
        // reduced crossing sequence is a homotopy invariant).
        let g = gate(v(6.0, 5.0), v(6.0, -5.0), v(6.0, 1.0), v(6.0, -1.0), true, 0);
        let h = gate(v(3.0, 5.0), v(3.0, -5.0), v(3.0, 1.0), v(3.0, -1.0), true, 1);
        assert_eq!(order_gates(&[g, h], &BTreeSet::new(), None).0, vec![0, 1]);
    }

    #[test]
    fn pair_move_breaks_a_stall_where_gates_cross() {
        // Two gates cross at X = (5, 3), off the straight line s e. Both points on X are each the
        // best point given the other (coordinate descent stalls at 2 |s X|); the optimum is the
        // straight line (length 10 mm) crossing g at x = 4.25 and h at x = 5.75.
        let (s, e) = (v(0.0, 0.0), v(10.0, 0.0));
        let g = (v(5.5, 5.0), v(3.75, -2.0));
        let h = (v(4.5, 5.0), v(6.25, -2.0));
        let x = v(5.0, 3.0);
        let (p, _, conv) = solve(s, e, &[g, h], &[x, x], 1.0, 1000);
        assert!(conv);
        let len = (p[0] - s).len() + (p[1] - p[0]).len() + (e - p[1]).len();
        assert!(len < 10e6 + 50.0, "len {len}");
        assert!((p[0] - v(4.25, 0.0)).len() < 100.0 && (p[1] - v(5.75, 0.0)).len() < 100.0, "{p:?}");
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
