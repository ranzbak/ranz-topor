//! Plane nets (plan "Capacity for N layers", plane nets): the pads of a net that has a pour are
//! not wired to each other; a pad that the pour does not reach gets a short stub: a track alone
//! into the fill where the pour fills a layer the pad has copper on (D6b), else a track and a
//! through via into the pour's layer, at a via site (a spot where via-fit tests of the [`crate::check::Checker`] pass:
//! the via checked as a foreign item against the board and all copper accepted so far). They are
//! placed once per route on the static board, before any topology (D6a), and enter the
//! projection as obstacles ([`extras`]): the planner routes around them instead of the drawing
//! finding them in the way (wiccon-2023 GPIO0 against the MODE1.2 GND stub, D5). Of the sites of
//! the largest via size that has one, the cheapest wins: stub length plus the `via_squeeze` price
//! of the room left around the via (a via between pads takes their fanout room; the user's
//! power-via rules 5 and 7).
//!
//! After routing, [`islands`] checks the plane nets' integrity: every pad and via of a plane
//! connection must lie inside the pour's filled copper (outer ring of a fill on a pour layer
//! the item has copper on); each one that does not is reported as an island. This is a check
//! against the fill of the board as given (a re-fill by KiCad can change it), not a refill.
//!
//! Complexity: a pad is O(rings x steps) checker queries; islands O(items x fill vertices).

use super::board::{Board, Extra};
use super::draw::{track_item, via_item, Drawer};
use super::state::ObsId;
use crate::check::point_in_poly;
use crate::geom::Vec2;
use crate::model::{BoardModel, Item};
use super::factors::{Event, Registry, WireCtx};
use crate::vias::{via_sizes, ViaSize};

/// One pad that needs a via into a pour.
#[derive(Clone, Copy, Debug)]
pub struct Req {
    pub conn: usize,
    /// Index into `model.pads`.
    pub pad: usize,
    pub obs: ObsId,
    pub net: i32,
    /// Stub width (nm).
    pub w: f64,
    /// Index into `model.pours`.
    pub pour: usize,
}

/// A placed stub and via, or a track-only stub into a fill on the pad's own layer (`via` None:
/// `size` is unused then).
#[derive(Clone, Debug)]
pub struct Out {
    pub items: Vec<Item>,
    pub via: Option<Vec2>,
    pub size: ViaSize,
    /// The stub's far end (the via centre, or the track's end inside the fill).
    pub end: Vec2,
    /// Model layer id of the stub.
    pub layer: u32,
    pub stub_nm: f64,
    /// 4.4: the stub ends on own-net copper already on the plane (index into the
    /// [`PlaneVia`] list passed to [`place`]): no via of its own (`via` None, `end` = that via's
    /// centre).
    pub shared: Option<usize>,
    /// The [`Drawer::commit`] handles of `items` (for re-siting a shared via, [`resite`]).
    pub handles: Vec<usize>,
}

/// Own-net copper on a plane that a plane pad's stub may end on (4.4, planes as terminals, user
/// rule 2026-11-10: a plane is a terminal of its net like a pad, and it may carry vias): a stub
/// via placed for another pad of the net, or an existing via of the board standing in the fill.
#[derive(Clone, Debug)]
pub struct PlaneVia {
    pub at: Vec2,
    pub size: ViaSize,
    pub net: i32,
    /// Sum of the widths of the stubs ending on it (nm): a via carries their current when
    /// pi * drill >= that sum (the user's power-via rule); past it the share is priced
    /// ([`load_price`]).
    pub load: f64,
    /// The request (index in the stage's order) whose own via it is; `None`: an existing via.
    pub owner: Option<usize>,
    /// The requests whose stubs end on it (besides `owner`).
    pub users: Vec<usize>,
    /// Given up: its owner's stub now ends on another via ([`place`] skips it).
    pub dead: bool,
}

/// Graded price of one more stub of width `w` on via `v` (4.4): 0 while pi * drill covers the
/// widths it carries, then the `via` price per drill circumference of excess (a second via is
/// what the excess asks for, so at 100 % excess sharing costs as much as a via of its own).
pub fn load_price(v: &PlaneVia, w: f64, via_price: f64) -> f64 {
    let cap = std::f64::consts::PI * v.size.drill.max(1) as f64;
    via_price * ((v.load + w - cap).max(0.0) / cap)
}

/// Prices of a stub site (D6a): `mm` per mm of stub (the `length` factor) and the
/// `via_squeeze` price of the room a via leaves to the foreign copper around it.
///
/// `live` (planes as terminals, `[planes] terminals`; `None` = off, every fill counts): the
/// pour connectivity of the board before the run ([`crate::pourcheck::check_pour_reach`] with no
/// routed copper: the KiCad fill of the ripped board, its existing tracks and vias, the pads).
/// Only fill in its net's main component is the net's copper (user 2026-11-12: a pour is a
/// terminal "given that the pour is connected to the rest of the net"); a stub or a wire that ends
/// in a piece cut off from it (an island) connects nothing.
pub struct SiteCost<'r> {
    pub mm: f64,
    pub reg: &'r Registry,
    pub live: Option<&'r crate::pourcheck::PourReach>,
}

impl SiteCost<'_> {
    /// The `via` factor's price of one via (a via site against a track-only stub).
    fn via(&self) -> f64 {
        self.reg.price(&Event::Via { wire: 0, count: 1, at: None }, &WireCtx::default())
    }
    fn squeeze(&self, slack: f64, d: f64) -> f64 {
        self.reg.price(&Event::ViaSqueeze { wire: 0, at: Vec2::default(), slack_nm: slack, via_nm: d }, &WireCtx::default())
    }
    /// Whether `p` on model layer `layer` lies in fill of pour `zi` that is connected to the rest
    /// of its net before the run (see `live`); true without `live`.
    pub fn connected(&self, zi: usize, layer: u32, p: Vec2) -> bool {
        self.live.is_none_or(|r| r.in_main(zi, layer, p))
    }
    /// [`in_fill`] restricted to connected fill: a disc of radius `r` at `p` inside a fill of pour
    /// `zi` on some layer where that fill is connected. O(fill vertices).
    pub fn in_live_fill(&self, pour: &crate::model::Pour, zi: usize, p: Vec2, r: f64) -> bool {
        let mut layers: Vec<u32> = pour.fills.iter().map(|f| f.layer).collect();
        layers.sort_unstable();
        layers.dedup();
        layers.into_iter().any(|l| in_fill_on(pour, Some(l), p, r) && self.connected(zi, l, p))
    }
    /// The pours a stub of `rq` may end in: with `live`, every pour of its net (each one is the
    /// net's copper where it is connected; the decomposition named the nearest, which may be a
    /// cut-off island), else its own pour only (before 4.4).
    pub fn pours_of(&self, model: &BoardModel, rq: &Req) -> Vec<usize> {
        if self.live.is_some() {
            (0..model.pours.len()).filter(|&k| model.pours[k].net == rq.net).collect()
        } else {
            vec![rq.pour]
        }
    }
    /// A via disc of radius `r` at `p` stands in connected fill of one of the pours `zs`.
    pub fn via_ok_at(&self, model: &BoardModel, zs: &[usize], p: Vec2, r: f64) -> bool {
        zs.iter().any(|&k| self.in_live_fill(&model.pours[k], k, p, r))
    }
    /// A track end of half width `r` at `p` on model layer `layer` lies in connected fill of one
    /// of the pours `zs`.
    pub fn track_ok_at(&self, model: &BoardModel, zs: &[usize], layer: u32, p: Vec2, r: f64) -> bool {
        zs.iter().any(|&k| in_fill_on(&model.pours[k], Some(layer), p, r) && self.connected(k, layer, p))
    }
}

/// Distance from `p` to the segment `a`-`b`.
fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let l2 = ab.dot(ab);
    let t = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t - p).len()
}

/// Whether a disc of radius `r` at `p` lies inside the filled copper of `pour` (some fill of it,
/// outside the fill's holes, at least `r` from every edge): a via there reaches the pour (D6b:
/// qmtech 1V8 stub vias stood in bare board next to a small local pour). O(fill vertices).
pub fn in_fill(pour: &crate::model::Pour, p: Vec2, r: f64) -> bool {
    in_fill_on(pour, None, p, r)
}

/// [`in_fill`] restricted to the fills on model layer `layer` (`None`: any layer).
pub fn in_fill_on(pour: &crate::model::Pour, layer: Option<u32>, p: Vec2, r: f64) -> bool {
    let ring = |pts: &[crate::model::Pt]| -> Vec<Vec2> { pts.iter().map(|q| q.to_vec2()).collect() };
    let far = |ps: &[Vec2]| (0..ps.len()).all(|i| seg_dist(p, ps[i], ps[(i + 1) % ps.len()]) >= r);
    pour.fills.iter().filter(|f| layer.is_none_or(|l| f.layer == l)).any(|f| {
        let outer = ring(&f.outer);
        outer.len() >= 3
            && point_in_poly(p, &outer)
            && far(&outer)
            && f.holes.iter().all(|h| {
                let h = ring(h);
                h.len() < 3 || (!point_in_poly(p, &h) && far(&h))
            })
    })
}

/// Place the stub and via of `rq` (see the module doc); the copper is indexed in `dr`. The via
/// stands inside the pour's fill ([`in_fill`]); a pad with no such site within reach is no
/// plane pad (the caller joins it by a signal connection instead, D6b).
///
/// Of the sites of the largest via size that has one, the cheapest: the stub's length price plus
/// the `via_squeeze` price of the room the via leaves (a via in a narrow pass between pads takes
/// the way other pads fan out by, power-via rules 5 and 7). The rings are searched outward until
/// the length price alone exceeds the best site's price (the squeeze price is never negative).
/// `vias`: the stub vias placed before (centre, radius); a stub keeps off them and off the
/// other pads of its net (no sliver connection, [`Drawer::stub_own_ok`]), its via keeps a gap
/// to them (or KiCad's minimum connection width check flags the lens between the two discs).
///
/// `shared` (4.4, `[planes] terminals`; empty = off): own-net copper already on the plane. A
/// stub may end on one of them instead of a via of its own, within the same reach, priced by
/// its length and the via's current load ([`load_price`]); it competes with an own via (which
/// pays the `via` price) as the via-like option.
pub fn place(dr: &mut Drawer, rq: &Req, cost: &SiteCost, vias: &[(Vec2, f64)], shared: &[PlaneVia]) -> Result<Out, String> {
    let (model, b) = (dr.model, dr.b);
    let pad = &model.pads[rq.pad];
    let ob = &b.proj.obstacles[rq.obs];
    let pour = &model.pours[rq.pour];
    let layer_idx = b.smd_layer(rq.obs).unwrap_or(0);
    let lid = b.signal_layers[layer_idx];
    let c = ob.center;
    let zs = cost.pours_of(model, rq);
    let rpad = ob.poly.iter().map(|&p| (p - c).len()).fold(0.0, f64::max);
    let sizes = via_sizes(&model.rules, rq.net, dr.opts);
    let step = 0.1e6;
    let dirs: Vec<Vec2> = (0..32)
        .map(|i| {
            let a = i as f64 * std::f64::consts::PI / 16.0;
            Vec2 { x: a.cos(), y: a.sin() }
        })
        .collect();
    // A track alone (D6b): where the pour has a fill on a layer the pad has copper on, a stub on
    // that layer ending inside the fill joins the pad with no via (qmtech C7.1 beside the F.Cu-only
    // 1V8 pour). Priced by length only; a via site pays the `via` factor on top.
    let mut best_track: Option<(f64, Vec2, u32)> = None;
    for l in (0..b.signal_layers.len()).filter(|&l| ob.layers.has(l)) {
        let tl = b.signal_layers[l];
        if !zs.iter().any(|&k| model.pours[k].fills.iter().any(|f| f.layer == tl)) {
            continue;
        }
        let mut d = step;
        while d <= rpad + 3.0e6 {
            if best_track.is_some_and(|(bc, ..)| cost.mm * d / 1e6 >= bc) {
                break;
            }
            for u in &dirs {
                let p = c + *u * d;
                if point_in_poly(p, &ob.poly) || !cost.track_ok_at(model, &zs, tl, p, 0.5 * rq.w) {
                    continue;
                }
                let price = cost.mm * d / 1e6;
                if best_track.is_some_and(|(bc, ..)| price >= bc) {
                    continue;
                }
                let stub = track_item(rq.net, tl, rq.w, c, p, None);
                if !dr.check_ok(&stub) || !dr.stub_own_ok(&stub, rq.pad, vias) {
                    continue;
                }
                best_track = Some((price, p, tl));
            }
            d += step;
        }
    }
    let via_price = cost.via();
    let mut best_via: Option<(f64, Vec2, ViaSize)> = None;
    for &sz in &sizes {
        let mut d = rpad + 0.5 * sz.d as f64 * 0.5;
        let dmax = rpad + 3.0e6;
        let mut best: Option<(f64, Vec2)> = None;
        while d <= dmax {
            if best.is_some_and(|(bc, _)| cost.mm * d / 1e6 >= bc) {
                break;
            }
            for u in &dirs {
                let p = c + *u * d;
                if !dr.opts.via_in_pad && point_in_poly(p, &ob.poly) {
                    continue;
                }
                if !cost.via_ok_at(model, &zs, p, 0.5 * sz.d as f64) {
                    continue;
                }
                let price = cost.mm * d / 1e6 + cost.squeeze(dr.point_room(p, sz.d as f64, rq.net), sz.d as f64);
                if best.is_some_and(|(bc, _)| price >= bc) {
                    continue;
                }
                // The pour must flow around the via: keep the pour's minimum width extra clearance.
                let crowd = via_item(rq.net, p, ViaSize { d: sz.d + 2 * pour.min_thickness.max(0), drill: sz.drill }, None);
                let stub = track_item(rq.net, lid, rq.w, c, p, None);
                // The via must not dip partly into its own pad either (KiCad connection width;
                // dcsgonebadge J3 pad 16: a GND stub via 3 um into the 0.3 mm oval pad, 0.09 mm).
                let r = 0.5 * sz.d as f64;
                if vias.iter().any(|&(q, rq2)| (q - p).len() < r + rq2 + 1_000.0) {
                    continue;
                }
                if !dr.check_ok(&crowd) || !dr.via_hole_ok(p, sz, rq.net) || !dr.via_own_pad_ok(p, sz, rq.net) || !dr.check_ok(&stub) || !dr.stub_own_ok(&stub, rq.pad, vias) {
                    continue;
                }
                best = Some((price, p));
            }
            d += step;
        }
        if let Some((price, p)) = best {
            best_via = Some((price + via_price, p, sz));
            break;
        }
    }
    // 4.4: own-net copper already on the plane within the same reach. The stub ends on that via's
    // centre (no sliver); it pays its length and the via's load, not a via.
    let mut best_share: Option<(f64, usize)> = None;
    for (k, v) in shared.iter().enumerate() {
        let len = (v.at - c).len();
        if v.dead || v.net != rq.net || len > rpad + 3.0e6 || !cost.via_ok_at(model, &zs, v.at, 0.5 * v.size.d as f64) {
            continue;
        }
        let price = cost.mm * len / 1e6 + load_price(v, rq.w, via_price);
        if best_share.is_some_and(|(bc, _)| price >= bc) {
            continue;
        }
        let stub = track_item(rq.net, lid, rq.w, c, v.at, None);
        if !dr.check_ok(&stub) || !dr.stub_own_ok(&stub, rq.pad, vias) {
            continue;
        }
        best_share = Some((price, k));
    }
    // The via-like option (reaches every layer the pour fills): the cheaper of an own via and a
    // share.
    let share_wins = best_share.is_some_and(|(ps, _)| best_via.is_none_or(|(pv, ..)| ps < pv));
    if let (true, Some((ps, k))) = (share_wins, best_share) {
        best_via = Some((ps, shared[k].at, shared[k].size));
    }
    // A via reaches every layer the pour fills; a track only the fill on its own layer, which the
    // signals routed later on that layer can cut off (dcsgonebadge, D6b: with tracks preferred
    // wherever cheaper, 35 of 35 GND stubs became F.Cu tracks, 16 pour pieces were cut off
    // instead of 6, 12 connections fewer). So where the pour also fills a layer the pad has no
    // copper on, a via site wins and a track is the fallback; where it fills only the pad's own
    // layers, the via adds no access and the cheaper of the two wins.
    let other_layers = zs.iter().flat_map(|&k| model.pours[k].fills.iter()).any(|f| !b.signal_layers.iter().enumerate().any(|(l, &id)| id == f.layer && ob.layers.has(l)));
    let take_track = match (best_track, best_via) {
        (Some(_), None) => true,
        (Some((bt, ..)), Some((bv, ..))) => !other_layers && bt <= bv,
        _ => false,
    };
    match (best_track, best_via) {
        (Some((_, p, tl)), _) if take_track => {
            let stub = track_item(rq.net, tl, rq.w, c, p, None);
            let h = dr.commit(&stub);
            return Ok(Out { items: vec![stub], via: None, size: sizes.first().copied().unwrap_or(ViaSize { d: 0, drill: 0 }), end: p, layer: tl, stub_nm: (p - c).len(), shared: None, handles: vec![h] });
        }
        (_, Some((_, p, sz))) if share_wins => {
            let stub = track_item(rq.net, lid, rq.w, c, p, None);
            let h = dr.commit(&stub);
            return Ok(Out { items: vec![stub], via: None, size: sz, end: p, layer: lid, stub_nm: (p - c).len(), shared: best_share.map(|(_, k)| k), handles: vec![h] });
        }
        (_, Some((_, p, sz))) => {
            let vi = via_item(rq.net, p, sz, None);
            let stub = track_item(rq.net, lid, rq.w, c, p, None);
            let hv = dr.commit(&vi);
            let hs = dr.commit(&stub);
            return Ok(Out { items: vec![stub, vi], via: Some(p), size: sz, end: p, layer: lid, stub_nm: (p - c).len(), shared: None, handles: vec![hs, hv] });
        }
        _ => {}
    }
    if std::env::var("TOPO_VIADBG").is_ok() {
        // Diagnosis (D1, report only): would a site be clean on the static board alone (pads,
        // holes, edge, keepouts, existing copper), i.e. is the pad crowded out by the copper
        // placed before it (wide wires, other vias and stubs)?
        let mut static_ok = 0usize;
        let mut tried = 0usize;
        for &sz in &sizes {
            let mut d = rpad + 0.5 * sz.d as f64 * 0.5;
            while d <= rpad + 3.0e6 {
                for u in &dirs {
                    let p = c + *u * d;
                    if !dr.opts.via_in_pad && point_in_poly(p, &ob.poly) {
                        continue;
                    }
                    tried += 1;
                    let crowd = via_item(rq.net, p, ViaSize { d: sz.d + 2 * pour.min_thickness.max(0), drill: sz.drill }, None);
                    let stub = track_item(rq.net, lid, rq.w, c, p, None);
                    if dr.checker.check_items(&[crowd]).violations.is_empty() && dr.via_hole_ok(p, sz, rq.net) && dr.via_own_pad_ok(p, sz, rq.net) && dr.checker.check_items(&[stub]).violations.is_empty() {
                        static_ok += 1;
                    }
                }
                d += step;
            }
        }
        eprintln!("PLANEDBG pad {}.{} net {} at ({:.3},{:.3}) sites {tried} clean-on-static-board {static_ok}", pad.ref_, pad.number, rq.net, c.x / 1e6, -c.y / 1e6);
    }
    Err(format!("pad {}.{}: no via site inside the pour's fill and no track into a fill on its own layer within 3 mm for a stub into its plane", pad.ref_, pad.number))
}

/// Where a wire from a plane pad that no stub serves ends on its pour (4.4, planes as terminals):
/// the pour is a terminal of the net like a pad, so the pad is wired to the pour's nearest
/// filled copper instead of to another pad of the net (user rule 2026-11-10).
#[derive(Clone, Debug)]
pub struct FarTerm {
    pub conn: usize,
    pub at: Vec2,
    /// Model layers where the fill covers the end (the wire may end on any of them).
    pub layers: Vec<u32>,
    /// Side of the terminal square (the wire's width).
    pub size: f64,
    /// Straight price of the candidate (mm: distance plus a via where the pad has no copper on
    /// its layers); the path search picks among the candidates.
    pub price: f64,
}

/// Most far-terminal candidates per pad ([`far_candidates`]).
pub const FAR_CANDIDATES: usize = 24;

/// Whether a disc of radius `r` at `p` lies inside the one fill piece `f` (inside its outer ring,
/// outside its holes, at least `r` from every edge of them). O(piece vertices).
fn in_piece(f: &crate::model::PourFill, p: Vec2, r: f64) -> bool {
    let ring = |pts: &[crate::model::Pt]| -> Vec<Vec2> { pts.iter().map(|q| q.to_vec2()).collect() };
    let far = |ps: &[Vec2]| (0..ps.len()).all(|i| seg_dist(p, ps[i], ps[(i + 1) % ps.len()]) >= r);
    let outer = ring(&f.outer);
    outer.len() >= 3
        && point_in_poly(p, &outer)
        && far(&outer)
        && f.holes.iter().all(|h| {
            let h = ring(h);
            h.len() < 3 || (!point_in_poly(p, &h) && far(&h))
        })
}

/// Spacing of the boundary samples of [`far_candidates`] and the least distance between two of
/// its candidates, nm.
pub const FAR_SAMPLE_NM: f64 = 0.5e6;
pub const FAR_SPACING_NM: f64 = 1.0e6;

/// Where the wire of `rq` may end on its net's pours when no stub serves it (4.4, planes as
/// terminals; user 2026-11-12: "Routing over pours should be zero length, because the trace ends
/// there, given that the pour is connected to the rest of the net"). The region is the fill
/// KiCad filled before the run (`model.pours[..].fills`) of every pour of the pad's net, on the
/// signal layers, only where it is connected to the rest of the net (`cost.live`, the main
/// component; a cut-off island would leave the pad floating). Each candidate keeps a margin: the
/// track end's disc of radius w/2 + the zone's minimum width + the net clearance lies inside one
/// fill piece. Candidates: the boundary of every piece sampled every [`FAR_SAMPLE_NM`] (and the
/// boundary point nearest the pad on each side), each stepped inward until the disc fits: along
/// the pad-to-boundary direction first, else straight across the edge (a side edge that
/// direction runs along), and at a vertex along the corner's bisector; priced by distance plus the `via` price where the pad has no
/// copper on that layer. Returned cheapest first, at most [`FAR_CANDIDATES`], at least
/// [`FAR_SPACING_NM`] apart: the caller lets the path search pick among them, so a point behind
/// an obstacle loses to one the trace reaches directly (no detour to the nearest point). Inside
/// the region the trace has no length: it ends at the region's edge. Empty: no such point (the
/// caller joins the pad by a signal connection as before).
/// O(B log B) in the boundary samples B, plus O(piece vertices) per tried sample.
pub fn far_candidates(dr: &Drawer, rq: &Req, cost: &SiteCost) -> Vec<FarTerm> {
    let (model, b) = (dr.model, dr.b);
    let ob = &b.proj.obstacles[rq.obs];
    let c = ob.center;
    let via_price = cost.via();
    // Boundary samples (lower bound of the price, point, directions to step in, zone, fill).
    let mut samples: Vec<(f64, Vec2, [Vec2; 3], usize, usize)> = Vec::new();
    for (zi, pour) in model.pours.iter().enumerate().filter(|(_, z)| z.net == rq.net) {
        for (fi, f) in pour.fills.iter().enumerate().filter(|(_, f)| b.signal_layers.contains(&f.layer)) {
            let li = b.signal_layers.iter().position(|&l| l == f.layer).unwrap_or(0);
            let extra = if ob.layers.has(li) { 0.0 } else { via_price };
            for ring in std::iter::once(&f.outer).chain(f.holes.iter()) {
                let pts: Vec<Vec2> = ring.iter().map(|q| q.to_vec2()).collect();
                if pts.len() < 3 {
                    continue;
                }
                let unit = |v: Vec2| v * (1.0 / v.len().max(1.0));
                // Normal of edge i (one side; the other side is tried as its negative).
                let normal = |i: usize| {
                    let ab = pts[(i + 1) % pts.len()] - pts[i];
                    unit(Vec2 { x: -ab.y, y: ab.x })
                };
                for i in 0..pts.len() {
                    let (a, e) = (pts[i], pts[(i + 1) % pts.len()]);
                    let ab = e - a;
                    let l = ab.len();
                    let tn = ((c - a).dot(ab) / ab.len2().max(1.0)).clamp(0.0, 1.0);
                    let n = (l / FAR_SAMPLE_NM).floor() as usize;
                    let nrm = normal(i);
                    // At the vertex: also the bisector of the two edges' normals (into a corner).
                    let bis = unit(nrm + normal((i + pts.len() - 1) % pts.len()));
                    for (k, t) in std::iter::once((usize::MAX, tn)).chain((0..=n).map(|k| (k, k as f64 * FAR_SAMPLE_NM / l.max(1.0)))) {
                        let q = a + ab * t.min(1.0);
                        let d0 = (q - c).len();
                        // Into the fill: away from the pad past the boundary point (the pad lies outside
                        // the fill or in a hole of it), or across the side where the pad sits on it;
                        // then straight across the edge (a side edge the pad's direction runs along),
                        // and at a vertex along the corner's bisector (either sign: `in_piece` keeps
                        // the inward one).
                        let u = if d0 > 1.0 { (q - c) * (1.0 / d0) } else { nrm };
                        samples.push((cost.mm * d0 / 1e6 + extra, q, [u, nrm, if k == 0 { bis } else { Vec2::default() }], zi, fi));
                    }
                }
            }
        }
    }
    samples.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.x.total_cmp(&y.1.x)).then(x.1.y.total_cmp(&y.1.y)));
    let mut out: Vec<FarTerm> = Vec::new();
    for (lb, q, dirs, zi, fi) in samples {
        if out.len() >= FAR_CANDIDATES && out.iter().all(|f| f.price <= lb) {
            break;
        }
        let pour = &model.pours[zi];
        let f = &pour.fills[fi];
        let margin = 0.5 * rq.w + pour.min_thickness.max(0) as f64 + model.rules.net_rule(rq.net).clearance as f64;
        let extra = lb - cost.mm * (q - c).len() / 1e6;
        let mut found = None;
        'dirs: for u in dirs.into_iter().filter(|u| u.len2() > 0.25).flat_map(|u| [u, u * -1.0]) {
            let mut s = margin;
            while s <= 4.0 * margin + 0.5e6 {
                let p = q + u * s;
                if in_piece(f, p, margin) && cost.connected(zi, f.layer, p) {
                    found = Some(p);
                    break 'dirs;
                }
                s += 0.25 * margin;
            }
        }
        let Some(at) = found else { continue };
        let price = cost.mm * (at - c).len() / 1e6 + extra;
        if out.iter().any(|g| (g.at - at).len() < FAR_SPACING_NM) {
            continue;
        }
        // The layers where some connected fill of the net covers the end with its margin.
        let layers: Vec<u32> = model
            .pours
            .iter()
            .enumerate()
            .filter(|(_, z)| z.net == rq.net)
            .flat_map(|(zj, z)| {
                let m = 0.5 * rq.w + z.min_thickness.max(0) as f64 + model.rules.net_rule(rq.net).clearance as f64;
                z.fills.iter().filter(move |g| b.signal_layers.contains(&g.layer) && in_piece(g, at, m) && cost.connected(zj, g.layer, at)).map(|g| g.layer)
            })
            .collect::<std::collections::BTreeSet<u32>>()
            .into_iter()
            .collect();
        if layers.is_empty() {
            continue;
        }
        out.push(FarTerm { conn: rq.conn, at, layers, size: rq.w, price });
        out.sort_by(|x, y| x.price.total_cmp(&y.price));
        while out.len() > FAR_CANDIDATES {
            out.pop();
        }
    }
    out
}

/// Geometric median of `pts` (Weiszfeld's iteration, 50 steps from the centroid): the point
/// whose summed distance to them is least. O(50 · |pts|).
pub fn weber(pts: &[Vec2]) -> Vec2 {
    let n = pts.len().max(1) as f64;
    let mut x = pts.iter().fold(Vec2::default(), |a, &p| a + p) * (1.0 / n);
    for _ in 0..50 {
        let (mut num, mut den) = (Vec2::default(), 0.0);
        for &p in pts {
            let d = (p - x).len();
            if d < 1.0 {
                return p; // on a point: the median for 3+ points spread round it, or near enough
            }
            num = num + p * (1.0 / d);
            den += 1.0 / d;
        }
        x = num * (1.0 / den);
    }
    x
}

/// Re-site the own via of request `owner` once other pads' stubs share it (4.4): the via moves
/// to the legal site where the stubs of all its pads are shortest together (search outward from
/// their geometric median, [`weber`]) plus the `via_squeeze` price there; kept where it was when
/// no site is cheaper. Every stub stays within the reach of [`place`] (3 mm past its pad) and
/// passes the same checks as there. `outs` (per request, in the stage's order) and `v` are
/// updated; `vias` = every stub via's (centre, radius) for the keep-off checks (this one's own
/// entry is skipped). Returns whether the via moved.
#[allow(clippy::too_many_arguments)]
pub fn resite(dr: &mut Drawer, reqs: &[Req], outs: &mut [Option<Out>], v: &mut PlaneVia, cost: &SiteCost, vias: &[(Vec2, f64)]) -> bool {
    let Some(owner) = v.owner else { return false };
    if v.users.is_empty() {
        return false;
    }
    let (model, b) = (dr.model, dr.b);
    let members: Vec<usize> = std::iter::once(owner).chain(v.users.iter().copied()).collect();
    if members.iter().any(|&i| outs[i].is_none()) {
        return false;
    }
    let pour = &model.pours[reqs[owner].pour];
    let sz = v.size;
    let r = 0.5 * sz.d as f64;
    let ctr = |i: usize| b.proj.obstacles[reqs[i].obs].center;
    let rpad = |i: usize| {
        let ob = &b.proj.obstacles[reqs[i].obs];
        ob.poly.iter().map(|&p| (p - ob.center).len()).fold(0.0, f64::max)
    };
    let lid = |i: usize| b.signal_layers[b.smd_layer(reqs[i].obs).unwrap_or(0)];
    for &i in &members {
        for &h in &outs[i].as_ref().expect("member placed").handles {
            dr.uncommit(h);
        }
    }
    let others: Vec<(Vec2, f64)> = vias.iter().copied().filter(|&(q, _)| (q - v.at).len() > 1.0).collect();
    let price_at = |dr: &Drawer, p: Vec2| -> f64 { members.iter().map(|&i| cost.mm * (ctr(i) - p).len() / 1e6).sum::<f64>() + cost.squeeze(dr.point_room(p, sz.d as f64, v.net), sz.d as f64) };
    let legal = |dr: &Drawer, p: Vec2| -> bool {
        if members.iter().any(|&i| (ctr(i) - p).len() > rpad(i) + 3.0e6 || (!dr.opts.via_in_pad && point_in_poly(p, &b.proj.obstacles[reqs[i].obs].poly))) {
            return false;
        }
        if !cost.via_ok_at(model, &cost.pours_of(model, &reqs[owner]), p, r) || others.iter().any(|&(q, rq2)| (q - p).len() < r + rq2 + 1_000.0) {
            return false;
        }
        let crowd = via_item(v.net, p, ViaSize { d: sz.d + 2 * pour.min_thickness.max(0), drill: sz.drill }, None);
        if !dr.check_ok(&crowd) || !dr.via_hole_ok(p, sz, v.net) || !dr.via_own_pad_ok(p, sz, v.net) {
            return false;
        }
        members.iter().all(|&i| {
            let stub = track_item(v.net, lid(i), reqs[i].w, ctr(i), p, None);
            dr.check_ok(&stub) && dr.stub_own_ok(&stub, reqs[i].pad, &others)
        })
    };
    let now = price_at(dr, v.at);
    let pts: Vec<Vec2> = members.iter().map(|&i| ctr(i)).collect();
    let w = weber(&pts);
    let rmax = pts.iter().map(|&p| (p - w).len()).fold(0.0, f64::max);
    let mut best: Option<(f64, Vec2)> = None;
    let step = 0.05e6;
    let mut d = 0.0;
    while d <= rmax {
        let n = if d == 0.0 { 1 } else { 32 };
        for k in 0..n {
            let a = k as f64 * std::f64::consts::PI / 16.0;
            let p = w + Vec2 { x: a.cos(), y: a.sin() } * d;
            let price = price_at(dr, p);
            if price >= now - 1e-6 || best.is_some_and(|(bp, _)| price >= bp) || !legal(dr, p) {
                continue;
            }
            best = Some((price, p));
        }
        d += step;
    }
    let at = best.map_or(v.at, |(_, p)| p);
    for &i in &members {
        let o = outs[i].as_mut().expect("member placed");
        let c = ctr(i);
        let stub = track_item(v.net, lid(i), reqs[i].w, c, at, None);
        if i == owner {
            let vi = via_item(v.net, at, sz, None);
            let hv = dr.commit(&vi);
            let hs = dr.commit(&stub);
            o.items = vec![stub, vi];
            o.via = Some(at);
            o.handles = vec![hs, hv];
        } else {
            let hs = dr.commit(&stub);
            o.items = vec![stub];
            o.handles = vec![hs];
        }
        o.end = at;
        o.stub_nm = (at - c).len();
    }
    let moved = (at - v.at).len() > 1.0;
    v.at = at;
    moved
}

/// Warnings for the pads and vias of plane connections that lie outside the pour's fill.
pub fn islands(model: &BoardModel, b: &Board, vias: &[(usize, Vec2)], reqs: &[Req]) -> Vec<String> {
    let mut out = Vec::new();
    for &(conn, at) in vias {
        let Some(rq) = reqs.iter().find(|r| r.conn == conn) else { continue };
        // Any pour of the net: with planes as terminals a stub may end in another pour of its net.
        let pours = || model.pours.iter().filter(|z| z.net == rq.net);
        let inside = pours().flat_map(|z| z.fills.iter()).any(|f| {
            let ring: Vec<Vec2> = f.outer.iter().map(|p| p.to_vec2()).collect();
            ring.len() >= 3 && point_in_poly(at, &ring)
        });
        if pours().any(|z| !z.fills.is_empty()) && !inside {
            let pad = &model.pads[rq.pad];
            out.push(format!(
                "plane island: via of pad {}.{} at ({:.3}, {:.3}) mm is outside the filled copper of its plane ({} zone)",
                pad.ref_,
                pad.number,
                at.x / 1e6,
                -at.y / 1e6,
                model.nets.iter().find(|n| n.code == rq.net).map_or("?", |n| n.name.as_str())
            ));
        }
    }
    let _ = b;
    out
}

/// The copper of a placed stub as projection obstacles (D6a: the planner routes around the
/// stubs, which are placed on the static board before the topology): the via, and the stub
/// track trimmed to the stretch between the pad's polygon and the via's (so no obstacle
/// overlaps another). A via inside its pad (`[vias] via_in_pad`) has no track part.
/// No pour ring around the via (`Extra::Via::ring` 0): a hard ring of the zone's minimum width
/// (against the neck KiCad's connection width check flagged at a GND plane via on dcsgonebadge in
/// D5, (126.79, 121.1) mm, 0.1038 mm) cost qmtech 16 connections in D6a; the pour needs room on
/// one side only, which wants a graded factor, not a ring.
pub fn extras(b: &Board, rq: &Req, o: &Out) -> Vec<Extra> {
    let ob = &b.proj.obstacles[rq.obs];
    let c = ob.center;
    // A track-only stub (no via) runs from the pad to its end inside the fill.
    // A stub onto a shared via (4.4) ends at that via, whose obstacle its owner already gave.
    let (p, r, mut out) = match o.via {
        Some(p) => (p, 0.5 * o.size.d as f64, vec![Extra::Via { net: rq.net, at: p, d: o.size.d as f64, ring: 0.0 }]),
        None if o.shared.is_some() => (o.end, 0.5 * o.size.d as f64, Vec::new()),
        None => (o.end, 0.0, Vec::new()),
    };
    let l = (p - c).len();
    if l <= r {
        return out;
    }
    let u = (p - c) * (1.0 / l);
    let nrm = Vec2 { x: -u.y, y: u.x } * (0.5 * rq.w);
    let inside = |q: Vec2| point_in_poly(q, &ob.poly);
    // The first point along the stub where its whole width has left the pad polygon.
    let step = 0.01e6;
    let mut t = 0.0;
    while t < l - r && (inside(c + u * t) || inside(c + u * t + nrm) || inside(c + u * t - nrm)) {
        t += step;
    }
    let t1 = l - r;
    if t1 - t > step {
        out.push(Extra::Track { net: rq.net, layer: o.layer, a: c + u * t, b: c + u * t1, width: rq.w });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weber_point_of_three_points_at_120_degrees_is_their_centre() {
        let c = Vec2 { x: 12e6, y: 12e6 };
        let pts: Vec<Vec2> = [-90.0f64, 30.0, 150.0].iter().map(|d| c + Vec2 { x: d.to_radians().cos(), y: d.to_radians().sin() } * 1.5e6).collect();
        assert!((weber(&pts) - c).len() < 1_000.0);
        // Two points: anywhere between them; Weiszfeld from the centroid stays at the midpoint.
        let m = weber(&[Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 2e6, y: 0.0 }]);
        assert!((m - Vec2 { x: 1e6, y: 0.0 }).len() < 1_000.0);
    }

    #[test]
    fn load_price_is_zero_within_pi_drill_and_grows_past_it() {
        let v = PlaneVia { at: Vec2::default(), size: ViaSize { d: 600_000, drill: 300_000 }, net: 1, load: 500_000.0, owner: Some(0), users: vec![], dead: false };
        // pi * 0.3 mm = 0.942 mm >= 0.5 + 0.25 mm.
        assert_eq!(load_price(&v, 250_000.0, 25.0), 0.0);
        let full = PlaneVia { load: 942_477.8, ..v.clone() };
        let p1 = load_price(&full, 250_000.0, 25.0);
        let p2 = load_price(&full, 500_000.0, 25.0);
        assert!(p1 > 0.0 && p2 > p1, "{p1} {p2}");
        // 100 % excess costs one via.
        assert!((load_price(&PlaneVia { load: 0.0, ..v }, 2.0 * 942_477.8, 25.0) - 25.0).abs() < 1e-3);
    }
}
