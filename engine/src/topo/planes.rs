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
}

/// Prices of a stub site (D6a): `mm` per mm of stub (the `length` factor) and the
/// `via_squeeze` price of the room a via leaves to the foreign copper around it.
pub struct SiteCost<'r> {
    pub mm: f64,
    pub reg: &'r Registry,
}

impl SiteCost<'_> {
    /// The `via` factor's price of one via (a via site against a track-only stub).
    fn via(&self) -> f64 {
        self.reg.price(&Event::Via { wire: 0, count: 1, at: None }, &WireCtx::default())
    }
    fn squeeze(&self, slack: f64, d: f64) -> f64 {
        self.reg.price(&Event::ViaSqueeze { wire: 0, at: Vec2::default(), slack_nm: slack, via_nm: d }, &WireCtx::default())
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
pub fn place(dr: &mut Drawer, rq: &Req, cost: &SiteCost, vias: &[(Vec2, f64)]) -> Result<Out, String> {
    let (model, b) = (dr.model, dr.b);
    let pad = &model.pads[rq.pad];
    let ob = &b.proj.obstacles[rq.obs];
    let pour = &model.pours[rq.pour];
    let layer_idx = b.smd_layer(rq.obs).unwrap_or(0);
    let lid = b.signal_layers[layer_idx];
    let c = ob.center;
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
        if !pour.fills.iter().any(|f| f.layer == tl) {
            continue;
        }
        let mut d = step;
        while d <= rpad + 3.0e6 {
            if best_track.is_some_and(|(bc, ..)| cost.mm * d / 1e6 >= bc) {
                break;
            }
            for u in &dirs {
                let p = c + *u * d;
                if point_in_poly(p, &ob.poly) || !in_fill_on(pour, Some(tl), p, 0.5 * rq.w) {
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
                if !in_fill(pour, p, 0.5 * sz.d as f64) {
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
    // A via reaches every layer the pour fills; a track only the fill on its own layer, which the
    // signals routed later on that layer can cut off (dcsgonebadge, D6b: with tracks preferred
    // wherever cheaper, 35 of 35 GND stubs became F.Cu tracks, 16 pour pieces were cut off
    // instead of 6, 12 connections fewer). So where the pour also fills a layer the pad has no
    // copper on, a via site wins and a track is the fallback; where it fills only the pad's own
    // layers, the via adds no access and the cheaper of the two wins.
    let other_layers = pour.fills.iter().any(|f| !b.signal_layers.iter().enumerate().any(|(l, &id)| id == f.layer && ob.layers.has(l)));
    let take_track = match (best_track, best_via) {
        (Some(_), None) => true,
        (Some((bt, ..)), Some((bv, ..))) => !other_layers && bt <= bv,
        _ => false,
    };
    match (best_track, best_via) {
        (Some((_, p, tl)), _) if take_track => {
            let stub = track_item(rq.net, tl, rq.w, c, p, None);
            dr.commit(&stub);
            return Ok(Out { items: vec![stub], via: None, size: sizes.first().copied().unwrap_or(ViaSize { d: 0, drill: 0 }), end: p, layer: tl, stub_nm: (p - c).len() });
        }
        (_, Some((_, p, sz))) => {
            let vi = via_item(rq.net, p, sz, None);
            let stub = track_item(rq.net, lid, rq.w, c, p, None);
            dr.commit(&vi);
            dr.commit(&stub);
            return Ok(Out { items: vec![stub, vi], via: Some(p), size: sz, end: p, layer: lid, stub_nm: (p - c).len() });
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

/// Warnings for the pads and vias of plane connections that lie outside the pour's fill.
pub fn islands(model: &BoardModel, b: &Board, vias: &[(usize, Vec2)], reqs: &[Req]) -> Vec<String> {
    let mut out = Vec::new();
    for &(conn, at) in vias {
        let Some(rq) = reqs.iter().find(|r| r.conn == conn) else { continue };
            let pour = &model.pours[rq.pour];
        let inside = pour.fills.iter().any(|f| {
            let ring: Vec<Vec2> = f.outer.iter().map(|p| p.to_vec2()).collect();
            ring.len() >= 3 && point_in_poly(at, &ring)
        });
        if !pour.fills.is_empty() && !inside {
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
    let (p, r, mut out) = match o.via {
        Some(p) => (p, 0.5 * o.size.d as f64, vec![Extra::Via { net: rq.net, at: p, d: o.size.d as f64, ring: 0.0 }]),
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
