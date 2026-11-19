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
use std::sync::Mutex;
use crate::fxhash::{map as fxmap, FxHashMap};
use std::sync::Arc;

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
/// towards end 1 of the edge (0: none on that side), and (tw4-r2 (2b), `[capacity]
/// via_disc_height`) the signed height of the via centre off the edge line (nm, + on the left of
/// end 0 -> end 1, [`signed_height`]; 0 = on the line, the model before (2b)).
pub type Disc = [f64; 3];

/// The signed distance of `v` from the line of edge `e` (nm, + on the left of end 0 -> end 1). O(1).
pub fn signed_height(pr: &Projection, e: EdgeId, v: Vec2) -> f64 {
    let [p, q] = pr.edges[e];
    let (pp, pq) = (pr.pos[p], pr.pos[q]);
    let ed = (pq - pp) * (1.0 / (pq - pp).len().max(1.0));
    ed.cross(v - pp)
}

/// The lean of a pass through edge `e` crossed in direction `d` (tw4-r2 (2b)): the component
/// along the edge (towards end 1) of the unit crossing direction, oriented into the edge's left
/// half-plane (+ height). In -1 .. 1; 0 when `d` is shorter than 1 nm (undefined), as
/// [`infl_dir`] reads it. O(1).
pub fn lean_dir(pr: &Projection, e: EdgeId, d: Vec2) -> f64 {
    let [p, q] = pr.edges[e];
    let (pp, pq) = (pr.pos[p], pr.pos[q]);
    let ed = (pq - pp) * (1.0 / (pq - pp).len().max(1.0));
    let dl = d.len();
    if dl < 1.0 {
        return 0.0;
    }
    let u = d * (1.0 / dl);
    let u = if ed.cross(u) < 0.0 { -u } else { u };
    u.dot(ed)
}

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
    /// tw4-r2 (2b): the track's [`lean_dir`] (read only next to a disc with a height).
    pub lean: f64,
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
///
/// tw4-r2 (2b): a disc with a height `h` (`disc[2]`) is the via at `h` off the edge line, not on
/// it. Against a track crossing at sin θ = 1 / infl (the oblique factor's floor 0.4) and leaning
/// `c` towards the disc in the disc's half-plane, the centre distance `x` along the edge keeps the
/// copper gap when `x sin θ - h c >= R` (R = d / 2 + gap + w / 2): `max(0, (R + h c) / sin θ)`.
/// Two discs at signed heights `h1`, `h2` need `sqrt(max(0, R^2 - (h1 - h2)^2))`. A disc-only item
/// (`w` 0, the wire on another layer here) has no track, so with a height its track terms go.
/// At `h` = 0 every term is the one before, bit for bit.
pub fn pair_req(a: &PackItem, b: &PackItem, gap: f64) -> f64 {
    let (da, db) = (a.disc[1], b.disc[0]);
    let (ha, hb) = (if da > 0.0 { a.disc[2] } else { 0.0 }, if db > 0.0 { b.disc[2] } else { 0.0 });
    if ha != 0.0 || hb != 0.0 {
        return pair_req_h(a, b, gap, da, db, ha, hb);
    }
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

/// [`pair_req`] with a disc height on either side (tw4-r2 (2b)). Continuous in the heights: at
/// both 0 it is `pair_req`'s value (the dropped track terms of a disc-only item are below its disc
/// terms there). O(1).
fn pair_req_h(a: &PackItem, b: &PackItem, gap: f64, da: f64, db: f64, ha: f64, hb: f64) -> f64 {
    let (ta, tb) = (!(a.w == 0.0 && da > 0.0), !(b.w == 0.0 && db > 0.0));
    let mut r: f64 = 0.0;
    if ta && tb {
        r = (0.5 * (a.w + b.w) + gap) * a.infl.max(b.infl);
    }
    // Disc a (nearer end 0) against track b: the via at (-x, ha) from b's crossing; b leaning
    // towards end 0 (lean < 0) on the via's side comes nearer.
    if da > 0.0 && tb {
        r = r.max((0.5 * (da + b.w) + gap - b.lean * ha).max(0.0) * b.infl);
    }
    if db > 0.0 && ta {
        r = r.max((0.5 * (a.w + db) + gap + a.lean * hb).max(0.0) * a.infl);
    }
    if da > 0.0 && db > 0.0 {
        let rr = 0.5 * (da + db) + gap;
        let dh = ha - hb;
        r = r.max((rr * rr - dh * dh).max(0.0).sqrt());
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
    let mut out: Vec<Disc> = vec![[0.0; 3]; n];
    for (&k, &d) in ks.iter().zip(ds) {
        if d <= 0.0 {
            continue;
        }
        for j in [k.checked_sub(1), Some(k)].into_iter().flatten().filter(|&j| j < n) {
            out[j] = [out[j][0].max(d), out[j][1].max(d), 0.0];
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
            items.push(PackItem { net: it.net, w: 0.0, s: it.s, infl: 1.0, disc: d, lean: 0.0 });
        }
    }
    (ids, items)
}

/// The end margin of item `i` at end `end` of edge `e` as [`Packer::bounds`] takes it, given its
/// track's (or disc-alone item's) margin `m` on its layer: at least its via disc's
/// [`Packer::margin_all`] where it has one. tw4-r2 (2b): a disc with a height is tested at its
/// centre (`margin_all_h`), and a disc-only item (`w` 0) then has no track margin. O(1) cached.
pub fn disc_margin(pk: &Packer, e: EdgeId, end: usize, i: &PackItem, _layer: usize, m: f64) -> f64 {
    let d = i.disc[end];
    if d <= 0.0 {
        return m;
    }
    let h = i.disc[2];
    if h == 0.0 {
        return m.max(pk.margin_all(e, end, i.net, d, i.s));
    }
    let ma = pk.margin_all_h(e, end, i.net, d, i.s, h);
    if i.w == 0.0 {
        ma
    } else {
        m.max(ma)
    }
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

/// [`Packer::via_cands`] cache key: triangle, net, radius bits, clearance bits.
type ViaCandKey = (usize, i32, u64, u64);

/// Margin cache key: edge, end, net, width bits, clearance bits, layer (`u8::MAX`: any layer),
/// height bits (tw4-r2 (2b): the centre off the edge line; 0 for a centre line on it).
type MarginKey = (usize, u8, i32, u64, u64, u8, u64);

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

/// The body of [`Packer::site_room`] over the cache `cache` (shared by [`SiteRooms`]).
fn site_room_of(pr: &Projection, eps: f64, cache: &Mutex<FxHashMap<(usize, i32), f64>>, t: super::state::TriId, net: i32, s: f64) -> f64 {
    if pr.tri_out[t] || !pr.tri_obs[t].is_empty() {
        return f64::NEG_INFINITY;
    }
    let key = (t, net ^ ((s as i64 as i32) << 8));
    if let Some(&f) = lock(cache).get(&key) {
        return f;
    }
    let [a, b, c] = pr.tris[t].map(|v| pr.pos[v]);
    let pts = tri_site_points(a, b, c);
    let mut free = [f64::INFINITY; 9];
    for o in pr.obstacles.iter().filter(|o| pr.via_foreign(o, net)) {
        for (q, f) in pts.iter().zip(free.iter_mut()) {
            let d = if super::state::point_in_poly(*q, &o.poly) { f64::NEG_INFINITY } else { poly_dist(*q, &o.poly) - o.clear.max(s) - eps };
            *f = f.min(d);
        }
    }
    // A keepout that forbids vias: the disc may not touch it (a hard rule, D6a).
    for k in &pr.no_via {
        for (q, f) in pts.iter().zip(free.iter_mut()) {
            let d = if super::state::point_in_poly(*q, k) { f64::NEG_INFINITY } else { poly_dist(*q, k) - eps };
            *f = f.min(d);
        }
    }
    let f = free.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    lock(cache).insert(key, f);
    f
}

/// TW6 part B search (`[factors] via_site_social`): the packer's site rooms ([`Packer::site_room`],
/// same cache) and via sizes, held by the state so the search prices a crossing where neither
/// wire has a via site. Cheap to clone (shared maps).
#[derive(Clone)]
pub struct SiteRooms<'a> {
    pr: &'a Projection,
    eps: f64,
    cache: Arc<Mutex<FxHashMap<(usize, i32), f64>>>,
    via_d: Arc<FxHashMap<i32, f64>>,
    via_d0: f64,
}

impl std::fmt::Debug for SiteRooms<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SiteRooms").field("eps", &self.eps).field("via_d0", &self.via_d0).finish()
    }
}

impl SiteRooms<'_> {
    /// [`Packer::site_room`]. O(obstacles) per new key.
    pub fn room(&self, t: super::state::TriId, net: i32, s: f64) -> f64 {
        site_room_of(self.pr, self.eps, &self.cache, t, net, s)
    }
    /// [`Packer::via_d`].
    pub fn via_d(&self, net: i32) -> f64 {
        self.via_d.get(&net).copied().unwrap_or(self.via_d0)
    }
    /// [`Packer::site_amount`] of a via of `net` (clearance `s`) at the best room over `tris`.
    pub fn amount(&self, tris: impl Iterator<Item = super::state::TriId>, net: i32, s: f64) -> f64 {
        let room = tris.map(|t| self.room(t, net, s)).fold(f64::NEG_INFINITY, f64::max);
        Packer::site_amount(room, self.via_d(net), s)
    }
}

pub struct Packer<'a> {
    pr: &'a Projection,
    outline: Vec<(Vec2, Vec2)>,
    /// Extra distance on top of every clearance (nm).
    eps: f64,
    /// Copper-to-edge clearance kept (nm; 0 when that check is ignored).
    edge_clear: f64,
    cache: Mutex<FxHashMap<MarginKey, f64>>,
    taut_cache: Mutex<FxHashMap<TautKey, Arc<Taut>>>,
    /// CM1: [`super::cutflow::via_point`] per (triangle, net, radius bits, clearance bits); static
    /// board only, so it lives as long as the packer (every CapModel built on it shares it).
    pub via_pt: Mutex<FxHashMap<(usize, i32, u64, u64), Vec2>>,
    /// tw4 T2: the static part of a planned via's gap cuts ([`super::viacut::via_static`]) per
    /// (triangle, net, point bits): a function of the projection and the static board, so it is
    /// shared by every capacity model built on this packer (the CM1 cost was its rebuild per
    /// model). Filled only for the packer's own projection ([`Packer::same_proj`]).
    pub(crate) via_static: super::viacut::GeoCache,
    /// tw4-r2: [`Packer::via_cands`] per (triangle, net, radius bits, clearance bits).
    via_cands: Mutex<FxHashMap<ViaCandKey, Arc<Vec<Vec2>>>>,
    /// Reach diameter of the first via size of every net (nm, [`crate::vias::via_reach`] x 2:
    /// the copper disc, or larger where the drill's hole clearance reaches further; 5.4 task 1);
    /// nets not listed use `via_d0`. 0 = the planner gives vias no room (a packer built without
    /// a board model).
    via_d: FxHashMap<i32, f64>,
    via_d0: f64,
    /// Free radius against other nets' obstacles at the incentre / centroid of a triangle, per
    /// (triangle, net). Cached.
    free_cache: Arc<Mutex<FxHashMap<(usize, i32), f64>>>,
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
    /// CM1 (`[capacity] via_cuts`): the planned vias are features of the capacity test (their
    /// gap cuts, [`super::viacut`]), measured in this planning norm; `None` = off.
    pub via_cuts: Option<super::sheet::Norm>,
    /// tw4-r2 (`[capacity] via_best_point`): price a via's cuts at the least-over site point of
    /// its triangle ([`Packer::via_cands`]).
    pub via_best: bool,
    /// tw4-r2 (`[capacity] via_over_ramp`, nm): the over-full via price ramps from 0 over this
    /// overage ([`super::viacut::price`]); 0 = the step at zero.
    pub via_ramp: f64,
    /// tw4-r2 (2b) (`[capacity] via_disc_height`): a planned via's disc packs on its wire's entry
    /// and exit edges at its height off each edge line ([`pair_req`], [`disc_margin`]).
    pub via_disc_h: bool,
    /// TW6 part A (`[vias] site_price = "graded"`): a via without a site pays the graded deficit
    /// of its room besides the count ([`Packer::site_room`]); the pad exits' via term (c) is then
    /// that price (counted once).
    pub site_price: bool,
    /// TW6 part C (with `site_price`): the continued dangling ends whose lead has a via end
    /// ([`super::board::KeptEndOut::via_end`], exist-term r4): the plan takes the via end as the
    /// site of a layer change at the lead end, as the drawing's `lead_via_site` does. Set by the
    /// router after [`Packer::for_model`].
    pub lead_ends: Vec<super::board::KeptEndOut>,
    /// tw4-r2 (1) (`[capacity] pad_exits`): the planned pad exits are terms of the capacity model
    /// ([`super::padexit`]); with via_cuts only.
    pub pad_exits: bool,
    /// tw4-r2 (1): with `pad_exits`, also the static term (a) (`[capacity] pad_exit_static`).
    pub pad_exit_static: bool,
    /// tw4-r2 (1): the neck floor of every net (nm, `Drawer::neck_floor` before the `min(w)`); nets
    /// not listed: `exit_floor0`.
    pub(crate) exit_floor: FxHashMap<i32, f64>,
    pub(crate) exit_floor0: f64,
    /// tw4-r2 (1): [`Packer::exit_static`] per (pad, side, net, width, clearance, floor bits).
    #[allow(clippy::type_complexity)]
    pub(crate) exit_cache: Mutex<FxHashMap<(usize, super::padexit::Side, i32, u64, u64, u64), Option<Arc<super::padexit::ExitStatic>>>>,
    /// M-45: the drawing is octilinear; a pass's oblique factor is the octilinear one
    /// ([`super::demand::octi_stretch`]).
    pub octi: bool,
    /// P1a (`[capacity] model = "cuts"`): the planner's capacity price is the per-layer cut
    /// test ([`super::cutmodel`]) instead of this packer's per-edge packing.
    pub cuts: Option<super::cutmodel::CutCtx>,
    /// The capacity model's build memo ([`super::refine::EdgeMemo`], par-refine item 2).
    pub(crate) edge_memo: std::sync::Mutex<super::refine::EdgeMemo>,
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

/// The packer's memo caches are behind mutexes (the packer is shared read-only by the lanes of
/// the parallel refinement, par-refine §3); a poisoned lock (a panic while one was held) still
/// holds a consistent map: a memo of pure values, filled one whole entry at a time. O(1).
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl<'a> Packer<'a> {
    pub fn new(pr: &'a Projection, outline: Vec<(Vec2, Vec2)>, eps: f64, edge_clear: f64) -> Self {
        Packer {
            pr,
            outline,
            eps,
            edge_clear,
            cache: Mutex::new(fxmap()),
            taut_cache: Mutex::new(fxmap()),
            via_pt: Mutex::new(fxmap()),
            via_static: Mutex::new(fxmap()),
            via_d: fxmap(),
            via_d0: 0.0,
            free_cache: Arc::new(Mutex::new(fxmap())),
            via_squeeze: 0.0,
            reserve_sites: false,
            via_between: false,
            via_cuts: None,
            via_best: false,
            via_ramp: 0.0,
            via_disc_h: false,
            site_price: false,
            lead_ends: Vec::new(),
            pad_exits: false,
            pad_exit_static: false,
            exit_floor: fxmap(),
            exit_floor0: 0.0,
            exit_cache: Mutex::new(fxmap()),
            via_cands: Mutex::new(fxmap()),
            octi: false,
            cuts: None,
            edge_memo: std::sync::Mutex::new(Default::default()),
        }
    }

    /// The projection the packer packs on.
    pub(crate) fn proj(&self) -> &'a Projection {
        self.pr
    }

    /// tw4-r2 (1): the neck floor of a wire of `net` and width `w` (nm; `Drawer::neck_floor`).
    pub fn exit_floor(&self, net: i32, w: f64) -> f64 {
        self.exit_floor.get(&net).copied().unwrap_or(self.exit_floor0).min(w)
    }

    /// Whether `pr` is this packer's projection (the build memo's edge ids are its edges). O(1).
    pub fn same_proj(&self, pr: &Projection) -> bool {
        std::ptr::eq(self.pr, pr)
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
        self.site_room(t, net, s) >= r
    }

    /// TW6 part A: the site room of triangle `t` for a via of `net` with clearance `s` (nm): the
    /// largest radius a via disc can have at one of the triangle's static candidate points
    /// ([`tri_site_points`]) against every other net's obstacle (its clearance, at least `s`) and
    /// the via keepouts; `-inf` outside the board or with an obstacle inside. One measure for the
    /// plan ([`Packer::via_site`] = room >= r, the graded site price), the drawing and the pad
    /// exits' via term. Cached per (triangle, net, s). O(obstacles) per new key.
    pub fn site_room(&self, t: super::state::TriId, net: i32, s: f64) -> f64 {
        site_room_of(self.pr, self.eps, &self.free_cache, t, net, s)
    }

    /// TW6 part B search: the site rooms of this packer as a value the state can hold
    /// ([`SiteRooms`]; the same cache, so plan, layering and search read one measure). O(nets).
    pub fn site_rooms(&self) -> SiteRooms<'a> {
        SiteRooms { pr: self.pr, eps: self.eps, cache: Arc::clone(&self.free_cache), via_d: Arc::new(self.via_d.clone()), via_d0: self.via_d0 }
    }

    /// TW6 part A (`[vias] site_price = "graded"`): how far a via of diameter `d` falls short of
    /// room where the room is `room` (nm, 0: it fits), capped at `d` (inside an obstacle). The
    /// graded site price is `1 + deficit / (d + s)` per via without a site (Luzin-Popov 2013
    /// p. 133: S = sum k_i x_i; graded, no threshold). O(1).
    pub fn site_deficit(room: f64, d: f64) -> f64 {
        (0.5 * d - room).max(0.0).min(d)
    }

    /// TW6: the graded site amount of a via of diameter `d` and clearance `s` where the best room
    /// is `room`: `1 + deficit / (d + s)` without a site ([`Packer::site_deficit`] > 0), 0 with
    /// one (the count of the predicted violation plus its graded depth, as CM1's `1 + over /
    /// pitch`). 0 when `d` <= 0 (no via sizes). O(1).
    pub fn site_amount(room: f64, d: f64, s: f64) -> f64 {
        if d <= 0.0 {
            return 0.0;
        }
        let def = Packer::site_deficit(room, d);
        if def > 0.0 {
            1.0 + def / (d + s).max(1.0)
        } else {
            0.0
        }
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
        p.via_cuts = opts.via_cuts.then(|| super::sheet::Norm::for_style(&opts.draw_style));
        p.via_best = opts.via_best_point;
        p.via_ramp = opts.via_over_ramp_mm.max(0.0) * 1e6;
        p.via_disc_h = opts.via_disc_height;
        p.site_price = opts.via_site_price == "graded";
        p.pad_exits = opts.pad_exits && opts.via_cuts;
        p.pad_exit_static = opts.pad_exit_static;
        let floor = |net: i32| model.rules.min_track_width.max(model.rules.net_rule(net).min_width).max(2 * model.rules.min_connection) as f64;
        p.exit_floor0 = floor(0);
        p.exit_floor = model.nets.iter().map(|n| (n.code, floor(n.code))).collect();
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

    /// The board outline's segments (math frame, nm).
    pub fn outline_segs(&self) -> &[(Vec2, Vec2)] {
        &self.outline
    }

    /// CM1: [`super::cutflow::via_point`] of a via of `net` (diameter `d`, clearance `s`) in
    /// triangle `t`, cached per (triangle, net, radius, clearance) ([`Packer::via_pt`]). O(1)
    /// after the first call.
    pub fn via_point_cached(&self, t: usize, net: i32, d: f64, s: f64) -> Vec2 {
        let key = (t, net, (0.5 * d).to_bits(), s.to_bits());
        if let Some(&p) = self.via_pt.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key) {
            return p;
        }
        let p = super::cutflow::via_point(self.pr, self, t, net, 0.5 * d, s).0;
        self.via_pt.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(key, p);
        p
    }

    /// tw4-r2 (`[capacity] via_best_point`): the site points of triangle `t`
    /// ([`tri_site_points`], in their order) where a via of `net` (diameter `d`, clearance `s`)
    /// has static room ([`super::cutflow::point_free`] at least the radius); the first is
    /// [`Packer::via_point_cached`]'s point when any has room. Cached per (triangle, net, radius,
    /// clearance). O(9 x obstacles) on the first call, O(1) after.
    pub fn via_cands(&self, t: usize, net: i32, d: f64, s: f64) -> Arc<Vec<Vec2>> {
        let key = (t, net, (0.5 * d).to_bits(), s.to_bits());
        if let Some(c) = lock(&self.via_cands).get(&key) {
            return c.clone();
        }
        let [a, b, c] = self.pr.tris[t].map(|v| self.pr.pos[v]);
        let pts: Vec<Vec2> = tri_site_points(a, b, c).into_iter().filter(|&q| super::cutflow::point_free(self.pr, self, q, net, s) >= 0.5 * d).collect();
        lock(&self.via_cands).entry(key).or_insert(Arc::new(pts)).clone()
    }

    /// tw4 T2: the static gap cuts of a via of `net` at `at` in triangle `t` of the projection
    /// `pr` ([`super::viacut::via_static`]), cached for the packer's own projection. O(1) after
    /// the first call.
    pub(crate) fn via_static_of(&self, pr: &Projection, t: usize, net: i32, at: Vec2, norm: super::sheet::Norm) -> Arc<super::viacut::ViaGeom> {
        if !self.same_proj(pr) {
            return Arc::new(super::viacut::via_static(pr, self, t, net, at, norm));
        }
        let key = (t, net, at.x.to_bits(), at.y.to_bits());
        if let Some(g) = self.via_static.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key) {
            return g.clone();
        }
        let g = Arc::new(super::viacut::via_static(pr, self, t, net, at, norm));
        self.via_static.lock().unwrap_or_else(std::sync::PoisonError::into_inner).entry(key).or_insert(g).clone()
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
        self.margin_h(e, end, net, w, s, layer, 0.0)
    }

    /// [`Packer::margin`] for a centre `h` off the edge line (signed, [`signed_height`]; tw4-r2
    /// (2b)): the first distance along the edge at which the point `h` off it keeps the clearance.
    /// `h` = 0 is `margin`. Cached.
    #[allow(clippy::too_many_arguments)]
    pub fn margin_h(&self, e: EdgeId, end: usize, net: i32, w: f64, s: f64, layer: usize, h: f64) -> f64 {
        let key = (e, end as u8, net, w.to_bits(), s.to_bits(), layer as u8, h.to_bits());
        if let Some(&m) = lock(&self.cache).get(&key) {
            return m;
        }
        let m = self.margin_raw(e, end, net, w, s, layer, h);
        lock(&self.cache).insert(key, m);
        m
    }

    /// [`Packer::margin`] on whichever layer needs the least: the taut path is one per wire, not
    /// per layer, and a pass may run on any layer, under an SMD pad of another layer too (a
    /// drilled obstacle is a wall on every layer and keeps its margin). Cached.
    pub fn margin_any(&self, e: EdgeId, end: usize, net: i32, w: f64, s: f64) -> f64 {
        let key = (e, end as u8, net, w.to_bits(), s.to_bits(), u8::MAX, 0);
        if let Some(&m) = lock(&self.cache).get(&key) {
            return m;
        }
        let m = (0..self.pr.n_layers.max(1)).map(|l| self.margin(e, end, net, w, s, l)).fold(f64::INFINITY, f64::min);
        lock(&self.cache).insert(key, m);
        m
    }

    /// [`Packer::margin`] on whichever layer needs the most (a through via's disc). Cached.
    pub fn margin_all(&self, e: EdgeId, end: usize, net: i32, w: f64, s: f64) -> f64 {
        self.margin_all_h(e, end, net, w, s, 0.0)
    }

    /// [`Packer::margin_all`] of a disc centred `h` off the edge line ([`Packer::margin_h`]). Cached.
    pub fn margin_all_h(&self, e: EdgeId, end: usize, net: i32, w: f64, s: f64, h: f64) -> f64 {
        let key = (e, end as u8, net, w.to_bits(), s.to_bits(), u8::MAX - 1, h.to_bits());
        if let Some(&m) = lock(&self.cache).get(&key) {
            return m;
        }
        let m = (0..self.pr.n_layers.max(1)).map(|l| self.margin_h(e, end, net, w, s, l, h)).fold(0.0, f64::max);
        lock(&self.cache).insert(key, m);
        m
    }

    /// The taut path of wire `w` of `st` (see the module doc). Cached per wire content (net,
    /// width, clearance, terminals, turns and pass edges), so an unchanged wire is never
    /// recomputed, whatever else of the State changes. O(passes of the wire).
    pub fn taut(&self, st: &State, w: WireId) -> Arc<Taut> {
        let wi = &st.wires[w];
        let mut key: TautKey = Vec::with_capacity(6 + 2 * wi.turns.len() + wi.passes.len());
        key.extend([wi.net as i64 as u64, wi.w.to_bits(), wi.s.to_bits(), wi.a as u64, wi.b as u64, wi.turns.len() as u64]);
        for &(k, v) in &wi.turns {
            key.extend([k as u64, v as u64]);
        }
        key.extend(wi.passes.iter().map(|&p| st.passes[p].edge as u64));
        if let Some(t) = lock(&self.taut_cache).get(&key) {
            return t.clone();
        }
        let t = Arc::new(self.taut_raw(st, w));
        lock(&self.taut_cache).insert(key, t.clone());
        t
    }

    /// The crossing directions of every pass of every wire (empty for dead wires), indexed like
    /// [`State::wires`] and the pass lists. O(passes) when cached.
    pub fn taut_dirs(&self, st: &State) -> Vec<Vec<Vec2>> {
        let _pg = super::prof::g(super::prof::K::TautDirs);
        (0..st.wires.len()).map(|w| if st.wires[w].alive { self.taut(st, w).dir.clone() } else { Vec::new() }).collect()
    }

    /// [`Packer::taut_dirs`] for the wires `only[w]`, empty for the others (loop-local: the
    /// directions [`super::refine::CapModel::new_only`] reads).
    pub fn taut_dirs_only(&self, st: &State, only: &[bool]) -> Vec<Vec<Vec2>> {
        (0..st.wires.len()).map(|w| if st.wires[w].alive && only.get(w).copied().unwrap_or(false) { self.taut(st, w).dir.clone() } else { Vec::new() }).collect()
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

    #[allow(clippy::too_many_arguments)]
    fn margin_raw(&self, e: EdgeId, end: usize, net: i32, w: f64, s: f64, layer: usize, h: f64) -> f64 {
        let pr = self.pr;
        let v = pr.edges[e][end];
        let u = pr.edges[e][1 - end];
        let (pv, pu) = (pr.pos[v], pr.pos[u]);
        let len = (pu - pv).len();
        // tw4-r2 (2b): the test point `h` off the line (+ on the left of end 0 -> end 1).
        let off = if h != 0.0 {
            let [p, q] = pr.edges[e];
            ((pr.pos[q] - pr.pos[p]) * (1.0 / (pr.pos[q] - pr.pos[p]).len().max(1.0))).perp() * h
        } else {
            Vec2::default()
        };
        match pr.vkind[v] {
            VertKind::Outline(o) => {
                let ob = &pr.obstacles[o];
                if ob.net == Some(net) || !(ob.drilled || ob.layers.has(layer)) {
                    return 0.0;
                }
                let req = self.clear_to(ob, e, s) + 0.5 * w + self.eps;
                let at = |d: f64| pv + (pu - pv) * (d / len.max(1.0)) + off;
                if h != 0.0 && poly_dist(at(0.0), &ob.poly) >= req {
                    return 0.0;
                }
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
                let at = |d: f64| pv + (pu - pv) * (d / len.max(1.0)) + off;
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
        let n = items.len();
        // A via disc is copper on every layer: its end margins are the largest over the layers.
        let mar = |i: &PackItem, end: usize| disc_margin(self, e, end, i, layer, self.margin(e, end, i.net, i.w, i.s, layer));
        let lo: Vec<f64> = items.iter().map(|i| mar(i, 0)).collect();
        let hi: Vec<f64> = items.iter().map(|i| len - mar(i, 1)).collect();
        let mut req = Vec::with_capacity(n.saturating_sub(1));
        self.req_into(regime, items, &mut req);
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

    /// The centre distances `req` of neighbours `i`, `i + 1` of `items` (edge with `regime`), as
    /// [`Packer::bounds`] packs them. O(items).
    fn req_into(&self, regime: Option<f64>, items: &[PackItem], req: &mut Vec<f64>) {
        let s_of = |it: &PackItem| regime.unwrap_or(it.s);
        req.clear();
        req.extend(items.windows(2).map(|x| {
            let (a, b) = (&x[0], &x[1]);
            pair_req(a, b, if a.net != b.net { s_of(a).max(s_of(b)) + self.eps } else { 0.0 })
        }));
    }

    /// [`Bounds::worst`] of [`Packer::bounds`] on edge `e` for `items` whose allowed spans
    /// `lo`, `hi` the caller computed (as `bounds` does: the end margins), without the five
    /// vectors `bounds` returns: the same operations in the same order, so the same bits (the
    /// capacity model's hot path, par-refine §3). `req`, `rmax`: scratch. O(items).
    pub fn worst_into(&self, e: EdgeId, items: &[PackItem], lo: &[f64], hi: &[f64], req: &mut Vec<f64>, rmax: &mut Vec<f64>) -> f64 {
        let n = items.len();
        self.req_into(self.pr.regime(e), items, req);
        rmax.clear();
        rmax.resize(n, 0.0);
        for i in (0..n).rev() {
            rmax[i] = if i == n - 1 { hi[i] } else { hi[i].min(rmax[i + 1] - req[i]) };
        }
        let mut worst = f64::NEG_INFINITY;
        let mut lmin = 0.0;
        for i in 0..n {
            lmin = if i == 0 { lo[0] } else { lo[i].max(lmin + req[i - 1]) };
            worst = worst.max(lmin - rmax[i]);
        }
        worst
    }

    /// Length of edge `e` as [`Packer::bounds`] takes it (nm, at least 1). O(1).
    pub fn edge_len(&self, e: EdgeId) -> f64 {
        let [p, q] = self.pr.edges[e];
        (self.pr.pos[q] - self.pr.pos[p]).len().max(1.0)
    }
}

/// The packer is shared read-only by the lanes of the parallel refinement (par-refine §2-3).
#[allow(dead_code)]
fn _packer_is_sync() {
    fn sync<T: Sync>() {}
    sync::<Packer<'static>>();
    sync::<State<'static>>();
}
