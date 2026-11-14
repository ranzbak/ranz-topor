//! The packer's capacity test, shared by the drawing and by P3/P4. The passes of one edge on one
//! layer, in edge order, are packed between the two end margins (the distance along the edge
//! from each end at which a centre line keeps its clearance to the obstacle there) with the
//! demand model's spacing between neighbours (half the widths plus, for different nets, the
//! larger clearance and a tolerance), each spacing stretched by the oblique-chord factor of the
//! wire crossing the edge at an angle (`1 / sin`, 0.4 .. 1). Leftmost positions `lmin` and
//! rightmost positions `rmax` follow from the two ends; the passes fit when `lmin <= rmax` for
//! every one. `Drawer::pack` takes its intervals from the same numbers, and
//! [`crate::topo::refine`] prices a labelling by how far `lmin` exceeds `rmax`, so P3 and the
//! drawing agree on what fits.
//!
//! The crossing direction of a pass (for the oblique factor) comes from the wire's *taut path*
//! ([`Packer::taut`]): the shortest path from terminal to terminal through its sleeve that
//! crosses every edge of its pass list inside the edge's end margins (string pulling, the
//! funnel algorithm over the per-edge intervals; the margins are the least over the layers,
//! since a pass may run on any layer). Where the taut path runs straight the direction is that
//! of the segment; where it bends at a pass it wraps the end vertex of that edge, and wires
//! packed there follow arcs about it, which cross the (radial) edge square when the edge lies in
//! the wrapped wedge, else the straight part before or after the bend. The evenly
//! spread pass points of [`State::polyline`] are not used for it: on short edges near pins they
//! zig-zag and drove `1 / sin` into its 2.5 cap where the wire runs straight (plan D1).
//!
//! Complexity: a margin is a 30-step bisection over the end obstacle's polygon, cached per
//! (edge, end, net, width, clearance, layer); a fit test is O(passes); a taut path is O(passes
//! of the wire) (each portal end enters and leaves the funnel once), cached per wire content.

use super::draw::{poly_dist, seg_dist};
use super::state::{EdgeId, Obstacle, Projection, State, VertKind, WireId};
use crate::geom::Vec2;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// The static via candidate points of a triangle `a b c`, in the drawing's order: incentre,
/// centroid, then seven barycentric points (shared by [`Packer::via_site`] and the drawing's
/// `via_candidates`, so the planner and the drawing test the same places). O(1).
pub fn tri_site_points(a: Vec2, b: Vec2, c: Vec2) -> [Vec2; 9] {
    let (la, lb, lc) = ((b - c).len(), (a - c).len(), (a - b).len());
    let inc = (a * la + b * lb + c * lc) * (1.0 / (la + lb + lc).max(1.0));
    let cen = (a + b + c) * (1.0 / 3.0);
    let bary = |u: f64, v: f64| a * u + b * v + c * (1.0 - u - v);
    [inc, cen, bary(0.6, 0.2), bary(0.2, 0.6), bary(0.2, 0.2), bary(0.4, 0.4), bary(0.4, 0.1), bary(0.1, 0.4), bary(0.1, 0.1)]
}

/// Via disc next to a pass, as packed on its edge: the diameter on the side towards end 0 and
/// towards end 1 of the edge (0: none on that side).
pub type Disc = [f64; 2];

/// One pass to pack: its net, width, clearance and the oblique-chord factor; `disc` is the via
/// disc of its wire next to the edge (see [`wire_discs`]; zeros: none). A pass whose wire is on
/// another layer here but has a via next to the edge is packed as the disc alone (`w` 0,
/// `infl` 1): a through via blocks every layer.
#[derive(Clone, Copy, Debug, Default)]
pub struct PackItem {
    pub net: i32,
    pub w: f64,
    pub s: f64,
    pub infl: f64,
    pub disc: Disc,
}

impl PackItem {
    /// The larger via disc diameter of the item (0: none).
    pub fn dmax(&self) -> f64 {
        self.disc[0].max(self.disc[1])
    }
}

/// Centre distance two neighbours `a`, `b` (`a` nearer end 0) need along the edge, with the
/// spacing `gap` between their copper (0 for one net): the tracks stretched by the larger
/// oblique factor, a via disc facing a track by the track's factor (a disc is round), two discs
/// by none; the largest of the pairs present.
pub fn pair_req(a: &PackItem, b: &PackItem, gap: f64) -> f64 {
    let (da, db) = (a.disc[1], b.disc[0]);
    let mut r = (0.5 * (a.w + b.w) + gap) * a.infl.max(b.infl);
    if da > 0.0 {
        r = r.max((0.5 * (da + b.w) + gap) * b.infl);
    }
    if db > 0.0 {
        r = r.max((0.5 * (a.w + db) + gap) * a.infl);
    }
    if da > 0.0 && db > 0.0 {
        r = r.max(0.5 * (da + db) + gap);
    }
    r
}

/// The via discs of the passes of wire `w` with vias at the traversals `ks` (diameters `ds`).
/// A via in traversal `k` sits on the wire's chord through that triangle, between its crossings
/// of the edge it enters by (pass `k - 1`) and the edge it leaves by (pass `k`): it is packed as
/// a full disc on both (Popov & Popov 2012, p. 117-118: a via is a round pad, and the room of a
/// passage counts what passes it). Measured (D3): a disc on the entry edge's far side only (the
/// via as a vertex on the exit crossing) lost wiccon 3.3v TP2.1 -> U1.8 again and routed 184
/// instead of 190 on a 2-layer badge, so both edges count it. O(passes + vias).
pub fn wire_discs(st: &State, w: WireId, ks: &[usize], ds: &[f64]) -> Vec<Disc> {
    let n = st.wires[w].passes.len();
    let mut out: Vec<Disc> = vec![[0.0, 0.0]; n];
    for (&k, &d) in ks.iter().zip(ds) {
        if d <= 0.0 {
            continue;
        }
        for j in [k.checked_sub(1), Some(k)].into_iter().flatten().filter(|&j| j < n) {
            out[j] = [out[j][0].max(d), out[j][1].max(d)];
        }
    }
    out
}

/// The items of edge-layer `l` from the passes `order` of an edge in edge order (shared by P3/P4
/// and the drawing): a pass on `l` packs its track (and its via disc, if any); a pass on
/// another layer whose wire has a via next to the edge packs the disc alone. Returns the pass
/// ids with a flag "disc only" and the items. O(passes of the edge).
pub fn layer_items(
    order: &[usize],
    l: usize,
    layer_of: impl Fn(usize) -> i32,
    disc_of: impl Fn(usize) -> Disc,
    item_of: impl Fn(usize) -> PackItem,
) -> (Vec<(usize, bool)>, Vec<PackItem>) {
    let mut ids = Vec::new();
    let mut items = Vec::new();
    for &p in order {
        let (pl, d) = (layer_of(p), disc_of(p));
        if pl == l as i32 {
            let mut it = item_of(p);
            it.disc = d;
            ids.push((p, false));
            items.push(it);
        } else if pl >= 0 && d[0].max(d[1]) > 0.0 {
            let it = item_of(p);
            ids.push((p, true));
            items.push(PackItem { net: it.net, w: 0.0, s: it.s, infl: 1.0, disc: d });
        }
    }
    (ids, items)
}

/// The packing numbers of the passes of one edge on one layer.
#[derive(Clone, Debug, Default)]
pub struct Bounds {
    /// Allowed span along the edge for each pass (distance from end 0).
    pub lo: Vec<f64>,
    pub hi: Vec<f64>,
    /// Leftmost and rightmost position of each pass.
    pub lmin: Vec<f64>,
    pub rmax: Vec<f64>,
    /// Required centre distance between neighbour `i` and `i + 1`.
    pub req: Vec<f64>,
}

impl Bounds {
    /// How far the packing is over: the largest `lmin - rmax` (nm; <= 0 when everything fits).
    pub fn worst(&self) -> f64 {
        self.lmin.iter().zip(&self.rmax).map(|(a, b)| a - b).fold(f64::NEG_INFINITY, f64::max)
    }

    pub fn fits(&self) -> bool {
        self.lmin.iter().zip(&self.rmax).all(|(a, b)| *a <= *b + 1e-6)
    }
}

/// The drawing's numeric tolerance on top of `margin_nm` (nm). The Checker judges the drawn
/// items after rounding to whole nm with the exact rule (`gap < required` fails), and the exact
/// path accepts 0.5 nm short ([`super::exact`]), so geometry computed at exactly the clearance
/// can fail by a fraction of a nm. Measured on the drawing witness A (hand topology, margin 0):
/// tolerance 0 rejects 116 / 103 runs on tanmatsu / hh2027, 2 nm and 10 nm reject 107 / 58,
/// 2 um (the S2a value) 115 / 60. 10 nm is far below any fab or DRC resolution.
pub const DRAW_TOL_NM: f64 = 10.0;

/// Margin cache key: edge, end, net, width bits, clearance bits, layer (`u8::MAX`: any layer).
type MarginKey = (usize, u8, i32, u64, u64, u8);

/// Taut path cache key: the wire's net, width and clearance bits, terminal vertices, turn
/// corners and the edges of its passes (everything the taut path depends on).
type TautKey = Vec<u64>;

/// The taut path of one wire (see the module doc).
#[derive(Clone, Debug, Default)]
pub struct Taut {
    /// Crossing point of every pass on its edge (math frame, nm).
    pub at: Vec<Vec2>,
    /// Crossing direction of every pass (not normalised; zero when undefined).
    pub dir: Vec<Vec2>,
}

pub struct Packer<'a> {
    pr: &'a Projection,
    outline: Vec<(Vec2, Vec2)>,
    /// Extra distance on top of every clearance (nm).
    eps: f64,
    /// Copper-to-edge clearance kept (nm; 0 when that check is ignored).
    edge_clear: f64,
    cache: RefCell<HashMap<MarginKey, f64>>,
    taut_cache: RefCell<HashMap<TautKey, Rc<Taut>>>,
    /// Reach diameter of the first via size of every net (nm, [`crate::vias::via_reach`] x 2:
    /// the copper disc, or larger where the drill's hole clearance reaches further; 5.4 task 1);
    /// nets not listed use `via_d0`. 0 = the planner gives vias no room (a packer built without
    /// a board model).
    via_d: HashMap<i32, f64>,
    via_d0: f64,
    /// Free radius against other nets' obstacles at the incentre / centroid of a triangle, per
    /// (triangle, net). Cached.
    free_cache: RefCell<HashMap<(usize, i32), f64>>,
    /// Price (mm) of a via disc that leaves its passage-layer no slack (the `via_squeeze`
    /// factor); 0 = off.
    pub via_squeeze: f64,
    /// M-C via room (`[vias] reserve_sites`): a via planned in a triangle takes its site, so
    /// another net's via planned in the same triangle has none there unless the triangle holds
    /// two discs ([`super::refine::CapModel`]).
    pub reserve_sites: bool,
    /// 5.4 `[vias] between_crossings`: via intervals exclude the triangles of their events
    /// ([`super::refine::via_ranges`]).
    pub via_between: bool,
    /// M-45: the drawing is octilinear; a pass's oblique factor is the octilinear one
    /// ([`super::demand::octi_stretch`]).
    pub octi: bool,
    /// P1a (`[capacity] model = "cuts"`): the planner's capacity price is the per-layer cut
    /// test ([`super::cutmodel`]) instead of this packer's per-edge packing.
    pub cuts: Option<super::cutmodel::CutCtx>,
}

/// The oblique-chord factor of a pass through edge `e` crossed in direction `d`: `1 / sin` of
/// the crossing angle, 1 .. 2.5 (1 when `d` is shorter than 1 nm, i.e. undefined).
pub fn infl_dir(pr: &Projection, e: EdgeId, d: Vec2) -> f64 {
    let [p, q] = pr.edges[e];
    let (pp, pq) = (pr.pos[p], pr.pos[q]);
    let ed = (pq - pp) * (1.0 / (pq - pp).len().max(1.0));
    let dl = d.len();
    if dl < 1.0 {
        1.0
    } else {
        1.0 / (d.cross(ed).abs() / dl).clamp(0.4, 1.0)
    }
}

/// The oblique-chord factor of the pass of a polyline `pl` at traversal `k` (the pass point is
/// `pl[k + 1]`) through edge `e`, the direction taken from its two neighbours. Used for the
/// hand's own crossings in the drawing witness; the router uses [`Packer::taut`] directions.
pub fn infl_of(pr: &Projection, e: EdgeId, pl: &[Vec2], k: usize) -> f64 {
    infl_dir(pr, e, pl[k + 2] - pl[k])
}

pub use super::funnel::{edge_point, funnel};

impl<'a> Packer<'a> {
    pub fn new(pr: &'a Projection, outline: Vec<(Vec2, Vec2)>, eps: f64, edge_clear: f64) -> Self {
        Packer {
            pr,
            outline,
            eps,
            edge_clear,
            cache: RefCell::new(HashMap::new()),
            taut_cache: RefCell::new(HashMap::new()),
            via_d: HashMap::new(),
            via_d0: 0.0,
            free_cache: RefCell::new(HashMap::new()),
            via_squeeze: 0.0,
            reserve_sites: false,
            via_between: false,
            octi: false,
            cuts: None,
        }
    }

    /// Reach diameter of the via a wire of `net` gets (its first size, nm; see the field): the
    /// disc that with the net's clearance round it keeps the copper and the hole rule. 0 when
    /// the packer has no via sizes.
    pub fn via_d(&self, net: i32) -> f64 {
        self.via_d.get(&net).copied().unwrap_or(self.via_d0)
    }

    /// Whether triangle `t` holds a via of `net` (radius `r`, clearance `s`) against the static
    /// board: not outside, no obstacle inside, and at one of the drawing's static candidate points
    /// of the triangle ([`tri_site_points`]: incentre, centroid, seven barycentric points) every
    /// other net's obstacle at least `r` plus the larger of `s` and the obstacle's clearance away.
    /// (D4: the incentre and centroid alone said "no site" for 15 planned vias of a 2-layer badge,
    /// of which the drawing placed 7, and P4 rerouted their wires for it.) Cached per
    /// (triangle, net, s). O(obstacles) per new key.
    pub fn via_site(&self, t: super::state::TriId, net: i32, r: f64, s: f64) -> bool {
        let pr = self.pr;
        if pr.tri_out[t] || !pr.tri_obs[t].is_empty() {
            return false;
        }
        let key = (t, net ^ ((s as i64 as i32) << 8));
        if let Some(&f) = self.free_cache.borrow().get(&key) {
            return f >= r;
        }
        let [a, b, c] = pr.tris[t].map(|v| pr.pos[v]);
        let pts = tri_site_points(a, b, c);
        let mut free = [f64::INFINITY; 9];
        for o in pr.obstacles.iter().filter(|o| pr.via_foreign(o, net)) {
            for (q, f) in pts.iter().zip(free.iter_mut()) {
                let d = if super::state::point_in_poly(*q, &o.poly) { f64::NEG_INFINITY } else { poly_dist(*q, &o.poly) - o.clear.max(s) - self.eps };
                *f = f.min(d);
            }
        }
        // A keepout that forbids vias: the disc may not touch it (a hard rule, D6a).
        for k in &pr.no_via {
            for (q, f) in pts.iter().zip(free.iter_mut()) {
                let d = if super::state::point_in_poly(*q, k) { f64::NEG_INFINITY } else { poly_dist(*q, k) - self.eps };
                *f = f.min(d);
            }
        }
        let f = free.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        self.free_cache.borrow_mut().insert(key, f);
        f >= r
    }

    /// The packer of `model` over the projection `pr`, with the drawing's tolerance and edge rule.
    pub fn for_model(model: &crate::model::BoardModel, opts: &crate::model::Options, pr: &'a Projection) -> Self {
        let edge_clear = if model.rules.ignored_checks.iter().any(|c| c == "copper_edge_clearance") { 0.0 } else { pr.edge_clear.max(pr.edge_clear_ignored) };
        let mut p = Packer::new(pr, super::board::outline_segments(model), opts.margin_nm as f64 + DRAW_TOL_NM, edge_clear);
        let first = |net: i32| crate::vias::via_sizes(&model.rules, net, opts).first().map_or(0.0, |&s| 2.0 * crate::vias::via_reach(&model.rules, net, s));
        p.via_d0 = first(0);
        p.via_d = model.nets.iter().map(|n| (n.code, first(n.code))).collect();
        p.reserve_sites = opts.reserve_via_sites;
        p.via_between = opts.via_between;
        p.octi = opts.draw_style == "octilinear" && opts.octi_room;
        if opts.capacity_model == "cuts" {
            p.cuts = Some(super::cutmodel::CutCtx::new(super::cutmodel::CutOpts::from_options(opts), opts.cut_vias, p.outline.clone()));
        }
        p
    }

    pub fn eps(&self) -> f64 {
        self.eps
    }

    pub fn edge_clear(&self) -> f64 {
        self.edge_clear
    }

    /// Clearance a wire with clearance `s` needs to the obstacle `ob` on edge `e` (nm).
    fn clear_to(&self, ob: &Obstacle, e: EdgeId, s: f64) -> f64 {
        match self.pr.regime(e) {
            Some(r) if !ob.bare => r,
            _ => {
                if ob.bare {
                    ob.clear
                } else {
                    ob.clear.max(s)
                }
            }
        }
    }

    /// Distance from end `end` of edge `e` along the edge at which a centre line of width `w` on
    /// `layer` keeps its clearance to the obstacle at that vertex (0 when none). Cached.
    pub fn margin(&self, e: EdgeId, end: usize, net: i32, w: f64, s: f64, layer: usize) -> f64 {
        let key = (e, end as u8, net, w.to_bits(), s.to_bits(), layer as u8);
        if let Some(&m) = self.cache.borrow().get(&key) {
            return m;
        }
        let m = self.margin_raw(e, end, net, w, s, layer);
        self.cache.borrow_mut().insert(key, m);
        m
    }

    /// [`Packer::margin`] on whichever layer needs the least: the taut path is one per wire, not
    /// per layer, and a pass may run on any layer, under an SMD pad of another layer too (a
    /// drilled obstacle is a wall on every layer and keeps its margin). Cached.
    pub fn margin_any(&self, e: EdgeId, end: usize, net: i32, w: f64, s: f64) -> f64 {
        let key = (e, end as u8, net, w.to_bits(), s.to_bits(), u8::MAX);
        if let Some(&m) = self.cache.borrow().get(&key) {
            return m;
        }
        let m = (0..self.pr.n_layers.max(1)).map(|l| self.margin(e, end, net, w, s, l)).fold(f64::INFINITY, f64::min);
        self.cache.borrow_mut().insert(key, m);
        m
    }

    /// [`Packer::margin`] on whichever layer needs the most (a through via's disc). Cached.
    pub fn margin_all(&self, e: EdgeId, end: usize, net: i32, w: f64, s: f64) -> f64 {
        let key = (e, end as u8, net, w.to_bits(), s.to_bits(), u8::MAX - 1);
        if let Some(&m) = self.cache.borrow().get(&key) {
            return m;
        }
        let m = (0..self.pr.n_layers.max(1)).map(|l| self.margin(e, end, net, w, s, l)).fold(0.0, f64::max);
        self.cache.borrow_mut().insert(key, m);
        m
    }

    /// The taut path of wire `w` of `st` (see the module doc). Cached per wire content (net,
    /// width, clearance, terminals, turns and pass edges), so an unchanged wire is never
    /// recomputed, whatever else of the State changes. O(passes of the wire).
    pub fn taut(&self, st: &State, w: WireId) -> Rc<Taut> {
        let wi = &st.wires[w];
        let mut key: TautKey = Vec::with_capacity(6 + 2 * wi.turns.len() + wi.passes.len());
        key.extend([wi.net as i64 as u64, wi.w.to_bits(), wi.s.to_bits(), wi.a as u64, wi.b as u64, wi.turns.len() as u64]);
        for &(k, v) in &wi.turns {
            key.extend([k as u64, v as u64]);
        }
        key.extend(wi.passes.iter().map(|&p| st.passes[p].edge as u64));
        if let Some(t) = self.taut_cache.borrow().get(&key) {
            return t.clone();
        }
        let t = Rc::new(self.taut_raw(st, w));
        self.taut_cache.borrow_mut().insert(key, t.clone());
        t
    }

    /// The crossing directions of every pass of every wire (empty for dead wires), indexed like
    /// [`State::wires`] and the pass lists. O(passes) when cached.
    pub fn taut_dirs(&self, st: &State) -> Vec<Vec<Vec2>> {
        (0..st.wires.len()).map(|w| if st.wires[w].alive { self.taut(st, w).dir.clone() } else { Vec::new() }).collect()
    }

    fn taut_raw(&self, st: &State, w: WireId) -> Taut {
        let pr = self.pr;
        let wi = &st.wires[w];
        let n = wi.passes.len();
        let (pa, pb) = (pr.pos[wi.a], pr.pos[wi.b]);
        let mut ports: Vec<(Vec2, Vec2)> = Vec::with_capacity(n + wi.turns.len() + 2);
        ports.push((pa, pa));
        let mut pass_port = Vec::with_capacity(n);
        let mut ti = 0;
        for k in 0..=n {
            while ti < wi.turns.len() && wi.turns[ti].0 == k {
                let v = pr.pos[wi.turns[ti].1];
                ports.push((v, v));
                ti += 1;
            }
            if k == n {
                break;
            }
            let e = st.passes[wi.passes[k]].edge;
            let [p, q] = pr.edges[e];
            let (pp, pq) = (pr.pos[p], pr.pos[q]);
            let len = (pq - pp).len().max(1.0);
            let m0 = self.margin_any(e, 0, wi.net, wi.w, wi.s);
            let m1 = self.margin_any(e, 1, wi.net, wi.w, wi.s);
            let (lo, hi) = (m0, len - m1);
            let (lo, hi) = if lo <= hi { (lo, hi) } else { (0.5 * (lo + hi), 0.5 * (lo + hi)) };
            let (lo, hi) = (lo.clamp(0.0, len), hi.clamp(0.0, len));
            let (a, b) = (edge_point(pp, pq, lo / len), edge_point(pp, pq, hi / len));
            // Left and right as seen coming from the triangle of traversal `k`.
            let t = pr.tris[wi.tris[k]];
            let c = (pr.pos[t[0]] + pr.pos[t[1]] + pr.pos[t[2]]) * (1.0 / 3.0);
            let left_is_q = (pq - pp).cross(c - pp) > 0.0;
            pass_port.push(ports.len());
            ports.push(if left_is_q { (b, a) } else { (a, b) });
        }
        ports.push((pb, pb));
        let path = funnel(&ports);
        if std::env::var("TOPO_TAUTDBG").is_ok_and(|v| v.split(',').any(|x| x == w.to_string())) {
            eprintln!("TAUTPATH wire {w}: {}", path.iter().map(|(p, i)| format!("({:.3},{:.3})@{i}", p.x / 1e6, -p.y / 1e6)).collect::<Vec<_>>().join(" "));
            eprintln!("TAUTPORTS wire {w}: {}", ports.iter().enumerate().map(|(i, (l, r))| format!("{i}:({:.3},{:.3})-({:.3},{:.3})", l.x / 1e6, -l.y / 1e6, r.x / 1e6, -r.y / 1e6)).collect::<Vec<_>>().join(" "));
        }
        // Corner directions are unit vectors scaled to 1 mm: `infl_dir` reads a direction
        // shorter than 1 nm as undefined (factor 1), and a unit vector rounds to either side of
        // 1.0 (D2: every wrapped corner lost its oblique factor about half the time).
        let unit = |d: Vec2| d.unit().map_or(Vec2::default(), |u| u * 1e6);
        let mut at = Vec::with_capacity(n);
        let mut dir = Vec::with_capacity(n);
        let mut wrapped = Vec::with_capacity(n);
        let mut j = 0usize;
        for (k, &i) in pass_port.iter().enumerate() {
            wrapped.push(false);
            while j + 1 < path.len() && path[j + 1].1 <= i {
                j += 1;
            }
            // `path[j]` is the last corner at or before portal `i`.
            let (l, r) = ports[i];
            let on = |c: Vec2| seg_dist(c, l, r) < 1.0;
            let corner = if path[j].1 == i || on(path[j].0) {
                Some(j)
            } else if j + 1 < path.len() && on(path[j + 1].0) {
                Some(j + 1)
            } else {
                None
            };
            match corner {
                Some(c) if c > 0 && c + 1 < path.len() => {
                    // The path wraps the end vertex `v` of the edge at this pass: wires packed
                    // around it follow arcs about `v`, which cross the edge (radial from `v`)
                    // square when the edge lies in the wrapped wedge (`d_in . u >= 0 >= d_out .
                    // u`, `u` the edge direction away from `v`); outside the wedge the edge cuts
                    // the straight part before (or after) the bend.
                    let (d_in, d_out) = (unit(path[c].0 - path[c - 1].0), unit(path[c + 1].0 - path[c].0));
                    let [p, q] = pr.edges[st.passes[wi.passes[k]].edge];
                    let (pp, pq) = (pr.pos[p], pr.pos[q]);
                    let x = path[c].0;
                    let u = if (x - pp).len2() <= (x - pq).len2() { unit(pq - pp) } else { unit(pp - pq) };
                    at.push(x);
                    dir.push(if d_in.dot(u) < 0.0 {
                        d_in
                    } else if d_out.dot(u) > 0.0 {
                        d_out
                    } else {
                        wrapped[k] = true;
                        u.perp()
                    });
                }
                Some(c) => {
                    // A pass at a terminal: the direction of the path's first / last segment.
                    let (a, b) = if c == 0 { (path[0].0, path[1.min(path.len() - 1)].0) } else { (path[c - 1].0, path[c].0) };
                    at.push(path[c].0);
                    dir.push(b - a);
                }
                None => {
                    let (c0, c1) = (path[j].0, path[(j + 1).min(path.len() - 1)].0);
                    // Where the segment c0 -> c1 meets the portal line.
                    let (d, pd) = (c1 - c0, r - l);
                    let den = d.cross(pd);
                    let x = if den.abs() < 1e-9 { l + pd * 0.5 } else { l + pd * ((c0 - l).cross(d) / -den).clamp(0.0, 1.0) };
                    at.push(x);
                    dir.push(d);
                }
            }
        }
        if self.octi {
            // M-45: the octilinear drawing's room (`demand::octi_stretch`), as the direction whose
            // oblique factor it is (`infl_dir` reads `1 / sin` of the crossing angle).
            for (k, d) in dir.iter_mut().enumerate() {
                let [p, q] = pr.edges[st.passes[wi.passes[k]].edge];
                let ed = pr.pos[q] - pr.pos[p];
                let Some(eu) = ed.unit() else { continue };
                let f = super::demand::octi_stretch(ed, *d, wrapped[k]).max(1.0);
                let sin = (1.0 / f).clamp(0.4, 1.0);
                let cos = (1.0 - sin * sin).max(0.0).sqrt();
                *d = (eu * cos + eu.perp() * sin) * 1e6;
            }
        }
        Taut { at, dir }
    }

    fn margin_raw(&self, e: EdgeId, end: usize, net: i32, w: f64, s: f64, layer: usize) -> f64 {
        let pr = self.pr;
        let v = pr.edges[e][end];
        let u = pr.edges[e][1 - end];
        let (pv, pu) = (pr.pos[v], pr.pos[u]);
        let len = (pu - pv).len();
        match pr.vkind[v] {
            VertKind::Outline(o) => {
                let ob = &pr.obstacles[o];
                if ob.net == Some(net) || !(ob.drilled || ob.layers.has(layer)) {
                    return 0.0;
                }
                let req = self.clear_to(ob, e, s) + 0.5 * w + self.eps;
                let at = |d: f64| pv + (pu - pv) * (d / len.max(1.0));
                // Whether or not the far end is closer than required as well (the edge is then
                // too narrow), the bisection finds the first distance that keeps the clearance.
                let (mut lo, mut hi) = (0.0, len);
                for _ in 0..30 {
                    let mid = 0.5 * (lo + hi);
                    if poly_dist(at(mid), &ob.poly) >= req {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                hi
            }
            VertKind::Edge => {
                let req = self.edge_clear + 0.5 * w + self.eps;
                let at = |d: f64| pv + (pu - pv) * (d / len.max(1.0));
                let dist = |p: Vec2| self.outline.iter().map(|&(a, b)| seg_dist(p, a, b)).fold(f64::INFINITY, f64::min);
                // The outline is not convex: take the last sample that is too close.
                let steps = 24;
                let mut m = 0.0;
                for i in 0..=steps {
                    let d = len * i as f64 / steps as f64;
                    if dist(at(d)) < req {
                        m = len * (i + 1).min(steps) as f64 / steps as f64;
                    }
                }
                m
            }
            _ => 0.0,
        }
    }

    /// The packing numbers of `items` (in edge order) on `layer` of edge `e`.
    pub fn bounds(&self, e: EdgeId, layer: usize, items: &[PackItem]) -> Bounds {
        let pr = self.pr;
        let [p, q] = pr.edges[e];
        let len = (pr.pos[q] - pr.pos[p]).len().max(1.0);
        let regime = pr.regime(e);
        let s_of = |it: &PackItem| regime.unwrap_or(it.s);
        let n = items.len();
        // A via disc is copper on every layer: its end margins are the largest over the layers.
        let mar = |i: &PackItem, end: usize| {
            let m = self.margin(e, end, i.net, i.w, i.s, layer);
            if i.disc[end] > 0.0 {
                m.max(self.margin_all(e, end, i.net, i.disc[end], i.s))
            } else {
                m
            }
        };
        let lo: Vec<f64> = items.iter().map(|i| mar(i, 0)).collect();
        let hi: Vec<f64> = items.iter().map(|i| len - mar(i, 1)).collect();
        let req: Vec<f64> = items
            .windows(2)
            .map(|x| {
                let (a, b) = (&x[0], &x[1]);
                pair_req(a, b, if a.net != b.net { s_of(a).max(s_of(b)) + self.eps } else { 0.0 })
            })
            .collect();
        let mut lmin = vec![0.0; n];
        let mut rmax = vec![0.0; n];
        for i in 0..n {
            lmin[i] = if i == 0 { lo[0] } else { lo[i].max(lmin[i - 1] + req[i - 1]) };
        }
        for i in (0..n).rev() {
            rmax[i] = if i == n - 1 { hi[i] } else { hi[i].min(rmax[i + 1] - req[i]) };
        }
        Bounds { lo, hi, lmin, rmax, req }
    }
}
