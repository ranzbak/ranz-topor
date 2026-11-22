//! Pour stitching after routing (D4): join the pieces of a pour net that the routed copper cut
//! off from the rest of its pour back to it, the way a designer does by hand: look at each copper
//! layer for areas of the pour that are cut off and, where there is room, put a via in that area
//! near a pad of the net, joining it to the pour on another layer.
//!
//! The refilled pour is estimated per (pour, layer) by [`crate::pourcheck`] (the pre-route fill of
//! the ripped board, minus the routed foreign copper grown by the clearance, opened by half the
//! zone's minimum width as KiCad's filler does, islands kept by the zone's island removal mode,
//! thermal / solid pads, vias and tracks of the net joined by union-find). Every component it
//! reports cut off ([`crate::pourcheck::Cut`]) gets one try per pass:
//! - **island via**: a via inside one of the component's islands, at a cell whose fill depth on
//!   that layer and on another pour layer of the net is at least the via radius, where that
//!   other layer's fill belongs to the main component; candidates are ranked by the distance to
//!   the component's nearest pad (or via) plus the `via_squeeze` price of the slack the fill
//!   leaves around the via (the fill depth is the room to the nearest foreign copper less its
//!   clearance) and the first that the checker accepts (clearances, holes, edge, keepouts,
//!   against the routed copper and the stitches placed before) wins;
//! - **pad stub**: a pad of the component with no island to put a via in (its thermal spokes all
//!   cut) gets a short track of the net width on its own layer, ending in the main fill of that
//!   layer, or else in a via standing in the main fill of another pour layer (nearest first,
//!   rings 0.1 mm apart up to [`STUB_MAX_NM`], 32 directions), as the plane stage does;
//! - **island join** (`[planes] join_islands`): a component with no pad and no via site gets a
//!   track from a pad of the net in the main component into the island, priced by
//!   [`join_price`] (length, growing with the square of the distance from the pad over the
//!   [`STUB_MAX_NM`] scale; a warning beyond it), its end clear of the net's other copper.
//!
//! The pass repeats (the fill is estimated again with the stitches in) up to [`PASSES`] times or
//! until a pass places nothing. Literature: TopoR leaves islands of a poured area to the user
//! ("this may split the copper area into multiple unconnected parts. You can select and delete
//! the resulting islands", TopoR 6.3 manual p. 121; pour last, after routing, p. 184; TopoR 5.0
//! help: plane net integrity is not verified, "add wires to connect islands"), so this stage is
//! the user's own hand technique, not TopoR's.
//!
//! Cost: per pass one pour check (O(raster cells + items), see `pourcheck`) plus, per cut-off
//! component, O(island cells / STEP²) candidates and at most [`MAX_TRIES`] checker queries.

use super::draw::{track_item, via_item};
use super::factors::{Event, Registry, WireCtx};
use crate::check::{Checker, ItemIndex};
use crate::geom::Vec2;
use crate::model::{BoardModel, Item, Options};
use crate::pourcheck::{check_pour_reach_until, Cut, PourReach};
use crate::vias::via_sizes;

/// Most stitching passes (coarse lattice).
pub const PASSES: usize = 3;
/// Most passes of the fine stub lattice, after the coarse passes are over ([`STUB_LATTICES`]).
pub const FINE_PASSES: usize = 2;
/// Most checker queries per cut-off component and pass.
pub const MAX_TRIES: usize = 64;
/// Candidate lattice: every STEP-th raster cell.
pub const STEP: usize = 4;
/// Longest pad stub, nm; also the length scale of an island join (no cutoff there, see
/// [`join_price`]).
pub const STUB_MAX_NM: f64 = 3.0e6;
/// Pad stub search lattices, tried in this order until one gives a stub: `(ring step nm, directions,
/// largest ring nm)`. The coarse one (0.1 mm x 32 directions, out to [`STUB_MAX_NM`]) steps 0.18 mm
/// along a 0.9 mm ring, so a site in a narrower window between two of its points is missed (qmtech
/// erbs C27.1, radii-m3-clear r2: legal x 134.21..134.30 between k7 134.328 and k8 134.152); the fine
/// one (0.025 mm x 128, out to 1.5 mm) finds such a site near the pad, the way a designer nudges a
/// GND pad via by hand. The fine lattice runs only in its own passes after the coarse passes are
/// over ([`FINE_PASSES`]), so everything the coarse passes place is unchanged; a piece the coarse
/// lattice could still reach after another piece's fix gets its coarse stub there first.
pub const STUB_LATTICES: [(f64, usize, f64); 2] = [(0.1e6, 32, STUB_MAX_NM), (0.025e6, 128, 1.5e6)];
/// Work cap of the fine lattice over one stitching run (candidate points, all pads and passes
/// together): about 15 pads' full fine search, so a board with many boxed-in pads cannot blow up.
/// A piece the fine lattice failed on is not searched again in a later pass.
pub const FINE_POINTS_MAX: usize = 100_000;

/// What the stitching added and found.
#[derive(Clone, Debug, Default)]
pub struct StitchOut {
    /// New items (KiCad frame): vias and pad stubs.
    pub items: Vec<Item>,
    /// Cut-off components before and after.
    pub before: usize,
    pub after: usize,
    /// Island vias and pad stubs placed.
    pub island_vias: usize,
    pub pad_stubs: usize,
    /// Pad-less islands joined to a connected pad of their net by a track (`[planes] join_islands`).
    pub island_joins: usize,
    /// Fine stub lattice ([`STUB_LATTICES`]): cut pieces that needed it, the stubs it found, the
    /// candidate points it looked at (capped by [`FINE_POINTS_MAX`]) and its time (s).
    pub fine_cuts: usize,
    pub fine_stubs: usize,
    pub fine_points: usize,
    pub fine_s: f64,
    /// Report lines.
    pub notes: Vec<String>,
    /// The pieces still cut off at the end (what `after` counts).
    pub left: Vec<Cut>,
}

fn kicad_mm(p: Vec2) -> String {
    format!("({:.3}, {:.3})", p.x / 1e6, -p.y / 1e6)
}

fn cut_text(model: &BoardModel, c: &Cut) -> String {
    let name = model.nets.iter().find(|n| n.code == c.net).map_or("?", |n| n.name.as_str());
    let pads: Vec<String> = c.pads.iter().take(4).map(|&p| format!("{}.{}", model.pads[p].ref_, model.pads[p].number)).collect();
    let at = c
        .pads
        .first()
        .map(|&p| model.pads[p].center.to_vec2())
        .or_else(|| c.vias.first().copied())
        .unwrap_or(c.at);
    format!(
        "{name}: {} island(s) {:.2} mm², pads [{}{}], {} via(s), near {}",
        c.islands.len(),
        c.area / 1e12,
        pads.join(", "),
        if c.pads.len() > 4 { ", ..." } else { "" },
        c.vias.len(),
        kicad_mm(at)
    )
}

/// The nets' rasters on layers other than `layer`, where `p` is in the main fill with at least
/// `depth` of room: `(pour, layer, fill depth)`.
fn main_elsewhere(reach: &PourReach, model: &BoardModel, net: i32, layer: Option<u32>, p: Vec2, depth: f64) -> Option<(usize, u32, f64)> {
    let mut best: Option<(usize, u32, f64)> = None;
    for (zi, z) in model.pours.iter().enumerate().filter(|(_, z)| z.net == net) {
        for &l in &z.layers {
            if Some(l) == layer {
                continue;
            }
            let Some(r) = reach.raster(zi, l) else { continue };
            let d = r.depth_at(p);
            if d >= depth && reach.island_in_main(zi, l, r.island_at(p)) && best.is_none_or(|b| d > b.2) {
                best = Some((zi, l, d));
            }
        }
    }
    best
}

/// stitch-vias: weight of a fallback (off-lattice) site's distance to the nearest lattice point
/// in its price (nm per nm): a fallback site near the lattice wins over one far from it.
pub const LATTICE_OFFSET_WEIGHT: f64 = 0.25;
/// stitch-vias: foreign-pad proximity price at contact (nm) and its reach from the via's edge
/// (nm): `PROX_PRICE_NM * max(0, 1 - d / PROX_REACH_NM)²`, graded (keeps pin rows and their
/// fanout room free; no cutoff inside the reach).
pub const PROX_PRICE_NM: f64 = 1.0e6;
pub const PROX_REACH_NM: f64 = 1.0e6;

/// The stitching lattice pitch of `opts` (nm): `[planes] stitch_pitch_mm` rounded to 0.05 mm, at
/// least 0.05 mm.
pub fn lattice_pitch(opts: &Options) -> f64 {
    ((opts.stitch_pitch_nm.max(0) as f64 / 50_000.0).round() * 50_000.0).max(50_000.0)
}

/// Via count of a piece of `area` nm² (stitch-vias): at least `min_vias` (and 1), one per
/// `pitch`². O(1).
pub fn want_vias(area: f64, pitch: f64, min_vias: usize) -> usize {
    min_vias.max(1).max((area / (pitch * pitch)).ceil().max(0.0) as usize)
}

/// One candidate stitching site.
#[derive(Clone, Copy, Debug)]
struct Site {
    p: Vec2,
    /// Graded price, nm (squeeze, foreign pads, BGA, lattice offset).
    price: f64,
    lattice: bool,
}

/// Band height of [`MaskHole`]'s edge buckets, nm.
const MASK_BAND_NM: f64 = 1.0e6;
/// Sample step along a track when testing it against a mask opening, nm.
const MASK_SAMPLE_NM: f64 = 2.5e5;
/// Radius of a pad's exposed copper taken around its centre for the exposure test, nm (a pad
/// has its own mask opening; this only tells whether the opening also exposes it).
const MASK_PAD_R_NM: f64 = 2.0e5;

/// A solder-mask opening ([`crate::model::MaskOpening`], math frame) with its edges bucketed by
/// y-band, and the nets whose copper it exposes (stitch-vias F1: exposed copper of two nets in
/// one opening is KiCad's `solder_mask_bridge`).
struct MaskHole {
    layer: u32,
    poly: Vec<Vec2>,
    bb: crate::geom::Aabb,
    bands: Vec<Vec<usize>>,
    nets: Vec<i32>,
}

impl MaskHole {
    /// O(edges · bands an edge spans).
    fn new(layer: u32, poly: Vec<Vec2>) -> Option<MaskHole> {
        if poly.len() < 3 {
            return None;
        }
        let mut bb = crate::geom::Aabb { min: poly[0], max: poly[0] };
        for p in &poly {
            bb.min = Vec2 { x: bb.min.x.min(p.x), y: bb.min.y.min(p.y) };
            bb.max = Vec2 { x: bb.max.x.max(p.x), y: bb.max.y.max(p.y) };
        }
        let nb = (((bb.max.y - bb.min.y) / MASK_BAND_NM).floor() as usize) + 1;
        let mut bands = vec![Vec::new(); nb];
        for e in 0..poly.len() {
            let (a, b) = (poly[e], poly[(e + 1) % poly.len()]);
            let (j0, j1) = (Self::band_of(bb.min.y, a.y.min(b.y), nb), Self::band_of(bb.min.y, a.y.max(b.y), nb));
            for band in &mut bands[j0..=j1] {
                band.push(e);
            }
        }
        Some(MaskHole { layer, poly, bb, bands, nets: Vec::new() })
    }

    fn band_of(y0: f64, y: f64, nb: usize) -> usize {
        (((y - y0) / MASK_BAND_NM).floor().max(0.0) as usize).min(nb - 1)
    }

    /// Whether the disc of radius `r` at `p` overlaps the opening: centre inside (even-odd ray
    /// cast) or an edge within `r`. O(edges in the bands of [p.y − r, p.y + r]).
    fn hits(&self, p: Vec2, r: f64) -> bool {
        let g = self.bb.grow(r);
        if p.x < g.min.x || p.x > g.max.x || p.y < g.min.y || p.y > g.max.y {
            return false;
        }
        let nb = self.bands.len();
        let n = self.poly.len();
        let mut inside = false;
        for &e in &self.bands[Self::band_of(self.bb.min.y, p.y, nb)] {
            let (a, b) = (self.poly[e], self.poly[(e + 1) % n]);
            if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
                inside = !inside;
            }
        }
        if inside {
            return true;
        }
        if r <= 0.0 {
            return false;
        }
        let (j0, j1) = (Self::band_of(self.bb.min.y, p.y - r, nb), Self::band_of(self.bb.min.y, p.y + r, nb));
        (j0..=j1).any(|j| {
            self.bands[j].iter().any(|&e| {
                let (a, b) = (self.poly[e], self.poly[(e + 1) % n]);
                let ab = b - a;
                let t = if ab.len() > 0.0 { ((p - a).dot(ab) / ab.dot(ab)).clamp(0.0, 1.0) } else { 0.0 };
                (a + ab * t - p).len() <= r
            })
        })
    }

    /// Whether `net`'s copper here would bridge to another net's exposed copper.
    fn foreign_to(&self, net: i32) -> bool {
        self.nets.iter().any(|&n| n > 0 && n != net)
    }
}

/// The mask openings of `model` with the nets they expose: pads, kept tracks and vias of the board
/// and the routed copper `items` (KiCad frame). Pours are not included (their fill changes with
/// the stitching itself). O(openings · copper samples · edges per band).
fn mask_holes(model: &BoardModel, items: &[Item]) -> Vec<MaskHole> {
    let mut holes: Vec<MaskHole> = model.mask_openings.iter().filter_map(|m| MaskHole::new(m.layer, m.poly.iter().map(|p| p.to_vec2()).collect())).collect();
    if holes.is_empty() {
        return holes;
    }
    let kpt = |x: f64, y: f64| Vec2 { x, y: -y };
    let mut mark = |net: i32, layer: Option<u32>, p: Vec2, r: f64| {
        if net <= 0 {
            return;
        }
        for h in holes.iter_mut() {
            if layer.is_none_or(|l| l == h.layer) && !h.nets.contains(&net) && h.hits(p, r) {
                h.nets.push(net);
            }
        }
    };
    let seg = |net: i32, layer: u32, a: Vec2, b: Vec2, w: f64, mark: &mut dyn FnMut(i32, Option<u32>, Vec2, f64)| {
        let k = (((b - a).len() / MASK_SAMPLE_NM).ceil() as usize).max(1);
        for i in 0..=k {
            mark(net, Some(layer), a + (b - a) * (i as f64 / k as f64), 0.5 * w);
        }
    };
    for pad in &model.pads {
        for &l in &pad.layers {
            mark(pad.net, Some(l), pad.center.to_vec2(), MASK_PAD_R_NM);
        }
    }
    for v in &model.vias {
        mark(v.net, None, v.at.to_vec2(), 0.5 * v.d as f64);
    }
    let prim_pts = |prim: &crate::geom::quant::TrackPrim, flip: bool| -> Vec<Vec2> {
        let f = |q: crate::geom::IVec2| if flip { kpt(q.x as f64, q.y as f64) } else { Vec2 { x: q.x as f64, y: q.y as f64 } };
        match *prim {
            crate::geom::quant::TrackPrim::Seg { a, b } => vec![f(a), f(b)],
            crate::geom::quant::TrackPrim::Arc { start, mid, end } => vec![f(start), f(mid), f(end)],
        }
    };
    for k in &model.kept {
        let pts = prim_pts(&k.prim, false);
        for w in pts.windows(2) {
            seg(k.net, k.layer, w[0], w[1], k.width as f64, &mut mark);
        }
    }
    for it in items {
        match it {
            Item::Via(v) => mark(v.net, None, kpt(v.at.x as f64, v.at.y as f64), 0.5 * v.d as f64),
            Item::Track(t) => {
                let pts = prim_pts(&t.prim, true);
                for w in pts.windows(2) {
                    seg(t.net, t.layer, w[0], w[1], t.width as f64, &mut mark);
                }
            }
        }
    }
    holes
}

/// What the stitching stages share: the checker and the copper placed so far, the via site
/// prices (foreign-pad buckets, BGA areas) and the lattice.
struct Ctx<'a> {
    model: &'a BoardModel,
    reg: &'a Registry,
    checker: Checker,
    idx: ItemIndex,
    /// The routed copper and every stitch item so far (KiCad frame).
    all: Vec<Item>,
    /// Vias [`Ctx::place_spread`] placed since the last [`Ctx::take_new`].
    new: Vec<Item>,
    tol: f64,
    via_in_pad: bool,
    bga: Vec<crate::geom::Aabb>,
    via_price_nm: f64,
    pitch: f64,
    origin: Vec2,
    min_vias: usize,
    per_area: bool,
    /// `stitch_thin_nm` (nm; 1 mm when 0): the narrowest neck a stitching via's antipad may leave
    /// in another net's pour without a price ([`Ctx::plane_neck`]).
    thin: f64,
    /// Foreign-pad buckets: cell (i, j) of [`PAD_BUCKET_NM`] -> pad indices.
    pad_buckets: std::collections::HashMap<(i64, i64), Vec<usize>>,
    /// Solder-mask openings and the nets they expose (F1).
    masks: Vec<MaskHole>,
}

/// Bucket size of [`Ctx::pad_buckets`], nm.
const PAD_BUCKET_NM: f64 = 2.0e6;

impl<'a> Ctx<'a> {
    fn new(model: &'a BoardModel, opts: &Options, reg: &'a Registry, items: &[Item]) -> Ctx<'a> {
        let checker = Checker::new(model, opts);
        let mut idx = ItemIndex::new(checker.cell());
        for it in items {
            idx.insert(it);
        }
        let mut pad_buckets: std::collections::HashMap<(i64, i64), Vec<usize>> = std::collections::HashMap::new();
        for (pi, pad) in model.pads.iter().enumerate() {
            let c = pad.center.to_vec2();
            let key = ((c.x / PAD_BUCKET_NM).floor() as i64, (c.y / PAD_BUCKET_NM).floor() as i64);
            pad_buckets.entry(key).or_default().push(pi);
        }
        Ctx {
            model,
            reg,
            checker,
            idx,
            all: items.to_vec(),
            new: Vec::new(),
            // KiCad builds a pad's and a via's polygon up to `max_error` outside their arcs: clear
            // by that much, or the two touch in a sliver (D6a).
            tol: model.rules.max_error.max(0) as f64,
            via_in_pad: opts.via_in_pad,
            bga: super::planes::bga_footprints(model, &opts.plane_bga_refs).into_iter().map(|f| f.area).collect(),
            via_price_nm: reg.price(&Event::Via { wire: 0, count: 1, at: None }, &WireCtx::default()) * 1e6,
            pitch: lattice_pitch(opts),
            origin: Vec2 { x: model.grid_origin[0] as f64, y: model.grid_origin[1] as f64 },
            min_vias: opts.stitch_min_vias,
            per_area: opts.stitch_per_area,
            // stitch_thin = 0 switches class 3 off, not the antipad-neck price: 1 mm then.
            thin: if opts.stitch_thin_nm > 0 { opts.stitch_thin_nm as f64 } else { 1.0e6 },
            pad_buckets,
            masks: mask_holes(model, items),
        }
    }

    /// Price of the neck a via of `net` (radius `r`) at `p` leaves in another net's pour (D2: a
    /// GND via's antipad in a 3V3 plane): on every pour layer of another net whose fill holds `p`,
    /// the copper between the antipad (radius + the pour's clearance) and the nearest fill edge is
    /// `neck`; below `stitch_thin` it costs `pitch · (1 − neck/thin)²` (graded, a via's value at a
    /// zero neck, so classes 2-4 skip such a site and class 1 takes it last). O(pours · layers).
    fn plane_neck(&self, reach: &PourReach, p: Vec2, r: f64, net: i32) -> f64 {
        let mut price = 0.0;
        for (zi, z) in self.model.pours.iter().enumerate() {
            if z.net == net {
                continue;
            }
            for &l in &z.layers {
                let Some(ras) = reach.raster(zi, l) else { continue };
                let d = ras.depth_at(p);
                if d <= 0.0 {
                    continue;
                }
                let clear = if z.clearance > 0 { z.clearance } else { self.model.rules.net_rule(z.net).clearance };
                let neck = (d - r - clear.max(0) as f64).max(0.0);
                if neck < self.thin {
                    price += self.pitch * (1.0 - neck / self.thin).powi(2);
                }
            }
        }
        price
    }

    /// Whether a via of `net` (radius `r`) at `p` would lie in a solder-mask opening that exposes
    /// another net's copper (F1: KiCad's `solder_mask_bridge`); a through via shows on both outer
    /// layers. O(openings · edges per band).
    fn under_foreign_mask(&self, p: Vec2, r: f64, net: i32) -> bool {
        self.masks.iter().any(|h| h.foreign_to(net) && h.hits(p, r))
    }

    /// Whether island `isl` of `ras` (layer `layer`, net `net`) overlaps a mask opening on that
    /// layer that exposes another net's copper: keeping it would expose `net` there too (F1).
    /// O(island cells / 4 · openings).
    fn island_under_foreign_mask(&self, ras: &crate::pourcheck::Raster, isl: u32, layer: u32, net: i32) -> bool {
        let hs: Vec<&MaskHole> = self.masks.iter().filter(|h| h.layer == layer && h.foreign_to(net)).collect();
        !hs.is_empty() && ras.island_sites(isl, 0.0, 2).into_iter().any(|p| hs.iter().any(|h| h.hits(p, 0.0)))
    }

    fn want(&self, area: f64) -> usize {
        if self.per_area { want_vias(area, self.pitch, self.min_vias) } else { self.min_vias.max(1) }
    }

    fn ok(&self, its: &[Item]) -> bool {
        self.checker.check_against(&self.idx, its, &[]).violations.is_empty()
    }

    fn add(&mut self, it: &Item) {
        self.idx.insert(it);
        self.all.push(it.clone());
    }

    fn take_new(&mut self) -> Vec<Item> {
        std::mem::take(&mut self.new)
    }

    /// A via stays off the pads of its own net: no via in a pad unless `[vias] via_in_pad` (then
    /// centred inside it), and never dipping partly into one (KiCad connection width, D1).
    fn own_pad_ok(&self, p: Vec2, r: f64, net: i32) -> bool {
        let tol = self.tol;
        self.model.pads.iter().filter(|pad| pad.net == net).all(|pad| {
            pad.layers.iter().all(|&l| {
                let d = crate::geom::point_skeleton(p, &self.model.skeleton(pad, l));
                d >= r + tol || (self.via_in_pad && d <= 0.0)
            })
        })
    }

    /// Nor touch other copper of its net without overlapping it well (a via grazing an own-net
    /// via or track is a sliver KiCad flags: a 2-layer badge in D4, two GND vias 0.596 mm apart,
    /// "actual 0.074 mm"): it keeps a gap to every own-net via and track.
    fn own_copper_ok(&self, p: Vec2, r: f64, net: i32) -> bool {
        let gap = 1_000.0 + 2.0 * self.tol;
        let q = Vec2 { x: p.x, y: -p.y }; // KiCad frame of the items
        self.model.vias.iter().filter(|v| v.net == net).all(|v| (v.at.to_vec2() - p).len() >= r + 0.5 * v.d as f64 + gap)
            && self.all.iter().all(|it| match it {
                Item::Via(v) if v.net == net => (Vec2 { x: v.at.x as f64, y: v.at.y as f64 } - q).len() >= r + 0.5 * v.d as f64 + gap,
                Item::Track(tr) if tr.net == net => match crate::geom::quant::dequantise(&tr.prim) {
                    crate::geom::Prim::Seg(s) => crate::geom::point_seg(q, &s) >= r + 0.5 * tr.width as f64 + gap,
                    _ => true,
                },
                _ => true,
            })
    }

    fn squeeze(&self, slack: f64, d: f64) -> f64 {
        self.reg.price(&Event::ViaSqueeze { wire: 0, at: Vec2::default(), slack_nm: slack, via_nm: d }, &WireCtx::default()) * 1e6
    }

    /// Foreign-pad proximity price of a via of radius `r` at `p` ([`PROX_PRICE_NM`]). O(pads in
    /// the 3 x 3 buckets around `p`).
    fn prox(&self, p: Vec2, r: f64, net: i32) -> f64 {
        let (bi, bj) = ((p.x / PAD_BUCKET_NM).floor() as i64, (p.y / PAD_BUCKET_NM).floor() as i64);
        let mut d_min = f64::INFINITY;
        for dj in -1..=1 {
            for di in -1..=1 {
                for &pi in self.pad_buckets.get(&(bi + di, bj + dj)).map_or(&[][..], |v| v.as_slice()) {
                    let pad = &self.model.pads[pi];
                    if pad.net == net {
                        continue;
                    }
                    for &l in &pad.layers {
                        d_min = d_min.min(crate::geom::point_skeleton(p, &self.model.skeleton(pad, l)) - r);
                    }
                }
            }
        }
        let t = (1.0 - d_min.max(0.0) / PROX_REACH_NM).max(0.0);
        PROX_PRICE_NM * t * t
    }

    /// Distance from `p` to the nearest lattice point (nm).
    fn lattice_offset(&self, p: Vec2) -> f64 {
        let f = |v: f64, o: f64| {
            let k = ((v - o) / self.pitch).round();
            v - (o + k * self.pitch)
        };
        Vec2 { x: f(p.x, self.origin.x), y: f(p.y, self.origin.y) }.len()
    }

    /// Candidate sites in `islands` (filtered by `keep(island index, p)`): lattice sites, then
    /// (with `fallback`) the D4 cell lattice; each with fill depth ≥ `need` on its own layer and
    /// in the main fill of another pour layer of `net` with the same depth, off the net's own pads.
    #[allow(clippy::too_many_arguments)]
    fn sites(&self, reach: &PourReach, net: i32, sz: crate::vias::ViaSize, need: f64, islands: &[(usize, u32, u32)], keep: &dyn Fn(usize, Vec2) -> bool, fallback: bool) -> Vec<Site> {
        let r = 0.5 * sz.d as f64;
        let mut out: Vec<Site> = Vec::new();
        for (k, &(zi, l, isl)) in islands.iter().enumerate() {
            let Some(ras) = reach.raster(zi, l) else { continue };
            let lat = ras.lattice_sites(isl, need, self.pitch, self.origin);
            let fb = if fallback { ras.island_sites(isl, need, STEP) } else { Vec::new() };
            for (p, lattice) in lat.into_iter().map(|p| (p, true)).chain(fb.into_iter().map(|p| (p, false))) {
                if !keep(k, p) {
                    continue;
                }
                let Some((_, _, d2)) = main_elsewhere(reach, self.model, net, Some(l), p, need) else { continue };
                if !self.own_pad_ok(p, r, net) {
                    continue;
                }
                if self.under_foreign_mask(p, r, net) {
                    continue;
                }
                let d1 = ras.depth_at(p);
                let mut price = self.squeeze(d1 - r, sz.d as f64) + self.squeeze(d2 - r, sz.d as f64) + self.prox(p, r, net);
                price += self.plane_neck(reach, p, r, net);
                if super::planes::in_bga(&self.bga, p) {
                    // Power-via rule (user 2026-11-13): under a BGA a via is a last resort.
                    price += self.via_price_nm * 100.0;
                }
                if !lattice {
                    price += LATTICE_OFFSET_WEIGHT * self.lattice_offset(p);
                }
                out.push(Site { p, price, lattice });
            }
        }
        out
    }

    /// Place up to `n` stitching vias of `sz` for `net` in `islands` (see [`Ctx::sites`]):
    /// farthest-point greedy — the first via (no `anchors`) at the site nearest `targets` plus
    /// its price, each next one at the site maximising min(distance to the anchors and the vias
    /// placed, pitch) − price; lattice sites before fallback sites; a site closer than `min_gap`
    /// to an anchor or placed via is not eligible; with `value`, a site priced at or above it is
    /// not worth a via (classes 2-4). Each candidate is checked (checker, own-net copper); at
    /// most [`MAX_TRIES`] refused candidates per via. Returns the sites placed (their vias are in
    /// [`Ctx::take_new`]). O(n x candidates + checker queries).
    #[allow(clippy::too_many_arguments)]
    fn place_spread(
        &mut self,
        reach: &PourReach,
        net: i32,
        sz: crate::vias::ViaSize,
        need: f64,
        islands: &[(usize, u32, u32)],
        keep: &dyn Fn(usize, Vec2) -> bool,
        targets: &[Vec2],
        anchors: &[Vec2],
        n: usize,
        value: Option<f64>,
        min_gap: f64,
        fallback: bool,
    ) -> Vec<Vec2> {
        let r = 0.5 * sz.d as f64;
        let cands = self.sites(reach, net, sz, need, islands, keep, fallback);
        let mut dead = vec![false; cands.len()];
        let mut placed: Vec<Vec2> = Vec::new();
        let mut near: Vec<Vec2> = anchors.to_vec();
        let pitch = self.pitch;
        'via: while placed.len() < n {
            let mut tries = 0usize;
            loop {
                let score = |s: &Site| -> f64 {
                    if near.is_empty() {
                        let t = targets.iter().map(|&t| (t - s.p).len()).fold(f64::INFINITY, f64::min);
                        -(if t.is_finite() { t } else { 0.0 } + s.price)
                    } else {
                        let d = near.iter().map(|&a| (a - s.p).len()).fold(f64::INFINITY, f64::min);
                        d.min(pitch) - s.price
                    }
                };
                let eligible = |i: usize| {
                    !dead[i] && (min_gap <= 0.0 || near.iter().all(|&a| (a - cands[i].p).len() >= min_gap))
                };
                let mut best: Option<(bool, f64, usize)> = None;
                for i in 0..cands.len() {
                    if !eligible(i) {
                        continue;
                    }
                    let key = (cands[i].lattice, score(&cands[i]), i);
                    let better = match best {
                        None => true,
                        Some((bl, bs, bi)) => {
                            (key.0 && !bl)
                                || (key.0 == bl && (key.1 > bs || (key.1 == bs && (cands[i].p.x, cands[i].p.y) < (cands[bi].p.x, cands[bi].p.y))))
                        }
                    };
                    if better {
                        best = Some(key);
                    }
                }
                let Some((_, _, i)) = best else { break 'via };
                if value.is_some_and(|v| cands[i].price >= v) {
                    break 'via;
                }
                let p = cands[i].p;
                dead[i] = true;
                if self.own_copper_ok(p, r, net) {
                    let v = via_item(net, p, sz, None);
                    if self.ok(std::slice::from_ref(&v)) {
                        self.add(&v);
                        self.new.push(v);
                        placed.push(p);
                        near.push(p);
                        continue 'via;
                    }
                }
                tries += 1;
                if n != usize::MAX && tries >= MAX_TRIES {
                    break 'via;
                }
            }
        }
        placed
    }
}

/// Stitch the cut-off pour components of `items` (the routed copper, KiCad frame).
/// `routed_pads` = model pad indices at the ends of routed connections of pour nets.
pub fn stitch(model: &BoardModel, opts: &Options, reg: &Registry, items: &[Item], routed_pads: &[usize], deadline: Option<std::time::Instant>) -> StitchOut {
    stitch_until(model, opts, reg, items, routed_pads, deadline, &|| false)
}

/// [`stitch`] that also ends after the pass in progress once `stop()` holds (cancel-fix: the run's
/// Cancel). Every pass starts with a complete pour-reach check, so the pieces still cut off
/// (`after`) are counted correctly also when it stops early.
pub fn stitch_until(model: &BoardModel, opts: &Options, reg: &Registry, items: &[Item], routed_pads: &[usize], deadline: Option<std::time::Instant>, stop: &dyn Fn() -> bool) -> StitchOut {
    let mut out = StitchOut::default();
    if model.pours.iter().all(|z| z.fills.is_empty()) {
        return out;
    }
    let mut cx = Ctx::new(model, opts, reg, items);
    let expired = || deadline.is_some_and(|d| std::time::Instant::now() >= d) || crate::control::cancelled() || stop();
    let mut unplaced: Vec<String> = Vec::new();
    // Coarse passes first; once they are over, the fine lattice's own passes. `again`: the last
    // pass placed nothing, so its reach and every coarse search would come out the same.
    let (mut fine_on, mut fine_passes) = (false, 0usize);
    let mut fine_failed: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut kept: Option<PourReach> = None;
    let mut left: Vec<Cut> = Vec::new();
    for pass in 0..PASSES + FINE_PASSES {
        unplaced.clear();
        let again = kept.is_some();
        let reach = match kept.take() {
            Some(r) => r,
            None => check_pour_reach_until(model, &cx.all, routed_pads, deadline),
        };
        if let Ok(path) = std::env::var("TOPO_POURDUMP") {
            reach.dump(&format!("{path}.p{pass}"));
        }
        if pass == 0 {
            out.before = reach.cuts.len();
            for c in &reach.cuts {
                out.notes.push(format!("pour piece cut off: {}", cut_text(model, c)));
            }
        }
        out.after = reach.cuts.len();
        left = reach.cuts.clone();
        if reach.cuts.is_empty() || expired() {
            break;
        }
        let mut placed_any = false;
        for c in &reach.cuts {
            let Some(&sz) = via_sizes(&model.rules, c.net, opts).first() else { continue };
            let r = 0.5 * sz.d as f64;
            // The fill has to reach the via with a ring of at least the zone's minimum width
            // (thinner fill is removed by KiCad's filler, and what is left meets the via in a
            // neck KiCad's connection width check flags: a 2-layer badge in D5, a GND via,
            // 0.1038 mm < 0.127 mm; D6a).
            let ring = model.pours.iter().filter(|z| z.net == c.net).map(|z| z.min_thickness.max(0) as f64).fold(0.0, f64::max).max(model.rules.min_connection.max(0) as f64);
            let need = r + ring;
            let targets: Vec<Vec2> = if c.pads.is_empty() { c.vias.clone() } else { c.pads.iter().map(|&p| model.pads[p].center.to_vec2()).collect() };
            // Island vias (stitch-vias class 1): at least `stitch_min_vias`, one more per pitch²
            // of the piece, lattice sites first, spread by farthest-point.
            let want = cx.want(c.area);
            let placed = if again { Vec::new() } else { cx.place_spread(&reach, c.net, sz, need, &c.islands, &|_, _| true, &targets, &[], want, None, 0.0, true) };
            let done_vias = !placed.is_empty();
            for p in &placed {
                out.notes.push(format!("stitch via {} mm at {} for {}", sz.d as f64 / 1e6, kicad_mm(*p), cut_text(model, c)));
            }
            if done_vias && placed.len() < want {
                out.notes.push(format!("stitch vias: {} of {} wanted for {} (no more legal site)", placed.len(), want, cut_text(model, c)));
            }
            out.island_vias += placed.len();
            out.items.extend(cx.take_new());
            let mut done = done_vias;
            if done {
                placed_any = true;
                continue;
            }
            // Pad stub: a pad with no usable island. The coarse lattice first for every pad; only when
            // it finds no stub, the fine one ([`STUB_LATTICES`]).
            'lat: for (li, &(d_step, n_dir, d_max)) in STUB_LATTICES.iter().enumerate() {
            let fine = li > 0;
            if !fine && again {
                continue;
            }
            if fine {
                if !fine_on || out.fine_points >= FINE_POINTS_MAX || c.pads.is_empty() || fine_failed.contains(&c.pads[0]) {
                    break;
                }
                out.fine_cuts += 1;
            }
            let t_fine = std::time::Instant::now();
            for &pi in &c.pads {
                let pad = &model.pads[pi];
                let cpos = pad.center.to_vec2();
                let w = model.rules.net_rule(c.net).width.max(1) as f64;
                let layers: Vec<u32> = pad.layers.iter().copied().filter(|l| model.layers.iter().any(|x| x.id == *l && x.routable)).collect();
                let mut tries = 0usize;
                // Out from the pad centre; the own-pad test keeps a via off the pad.
                let mut d: f64 = 0.2e6;
                while d <= d_max && tries < 4 * MAX_TRIES && !(fine && out.fine_points >= FINE_POINTS_MAX) {
                    out.fine_points += usize::from(fine) * n_dir;
                    for k in 0..n_dir {
                        let a = k as f64 * std::f64::consts::TAU / n_dir as f64;
                        let p = cpos + Vec2 { x: a.cos(), y: a.sin() } * d;
                        for &sl in &layers {
                            // Track only, into the main fill of its own layer.
                            let same = model.pours.iter().enumerate().any(|(zi, z)| {
                                z.net == c.net && reach.raster(zi, sl).is_some_and(|ras| ras.depth_at(p) >= 0.5 * w && reach.island_in_main(zi, sl, ras.island_at(p)))
                            });
                            let via_ok = !same && main_elsewhere(&reach, model, c.net, None, p, need).is_some() && cx.own_pad_ok(p, r, c.net) && cx.own_copper_ok(p, r, c.net);
                            if !same && !via_ok {
                                continue;
                            }
                            tries += 1;
                            let mut its = vec![track_item(c.net, sl, w, cpos, p, None)];
                            if via_ok {
                                its.push(via_item(c.net, p, sz, None));
                            }
                            if cx.ok(&its) {
                                out.notes.push(format!(
                                    "pad stub {}.{} -> {} ({:.3} mm{}) for {}",
                                    pad.ref_,
                                    pad.number,
                                    kicad_mm(p),
                                    (p - cpos).len() / 1e6,
                                    if via_ok { ", via" } else { "" },
                                    cut_text(model, c)
                                ));
                                for it in its {
                                    cx.add(&it);
                                    out.items.push(it);
                                }
                                out.pad_stubs += 1;
                                out.fine_stubs += usize::from(fine);
                                done = true;
                                if fine {
                                    out.fine_s += t_fine.elapsed().as_secs_f64();
                                }
                                break 'lat;
                            }
                        }
                    }
                    d += d_step;
                }
            }
            if fine {
                out.fine_s += t_fine.elapsed().as_secs_f64();
                fine_failed.insert(c.pads[0]);
            }
            }
            // Island join (`[planes] join_islands`, user 2026-11-12): a piece holding no pad that no
            // via reaches is joined to the nearest pad of its net that is connected to the rest of
            // the net, by a track from the pad into the island (shortest first).
            if !done && c.pads.is_empty() && opts.plane_join_islands {
                let site_ok = |p: Vec2, r: f64| cx.own_copper_ok(p, r, c.net);
                let joined = join_island(model, &reach, c, &|it: &Item| cx.ok(std::slice::from_ref(it)), &site_ok);
                if let Some(it) = joined {
                    let (pi, len) = (it.1, it.2);
                    out.notes.push(format!(
                        "island join {}.{} -> {} ({:.3} mm) for {}",
                        model.pads[pi].ref_,
                        model.pads[pi].number,
                        kicad_mm(it.3),
                        len / 1e6,
                        cut_text(model, c)
                    ));
                    if it.4 > STUB_MAX_NM {
                        out.notes.push(format!(
                            "long island join {}.{} -> {}: {:.3} mm from the pad edge, over the {:.1} mm scale (check it by hand)",
                            model.pads[pi].ref_,
                            model.pads[pi].number,
                            kicad_mm(it.3),
                            it.4 / 1e6,
                            STUB_MAX_NM / 1e6
                        ));
                    }
                    cx.add(&it.0);
                    out.items.push(it.0);
                    out.island_joins += 1;
                    done = true;
                }
            }
            if done {
                placed_any = true;
            } else {
                unplaced.push(cut_text(model, c));
            }
        }
        fine_passes += usize::from(fine_on);
        let over = !placed_any || if fine_on { fine_passes == FINE_PASSES } else { pass + 1 == PASSES };
        if over && !fine_on && reach.cuts.iter().any(|c| !c.pads.is_empty()) {
            // The coarse passes are over: the fine lattice's passes for the pieces left.
            fine_on = true;
            if !placed_any {
                kept = Some(reach);
            }
            continue;
        }
        if over {
            if placed_any {
                let reach = check_pour_reach_until(model, &cx.all, routed_pads, deadline);
                if let Ok(path) = std::env::var("TOPO_POURDUMP") {
                    reach.dump(&format!("{path}.p{}", pass + 1));
                }
                out.after = reach.cuts.len();
                left = reach.cuts.clone();
            }
            break;
        }
    }
    if out.fine_cuts > 0 {
        out.notes.push(format!(
            "fine stub lattice: {} cut piece(s) searched, {} stub(s) found, {} points{} ({:.0} ms)",
            out.fine_cuts,
            out.fine_stubs,
            out.fine_points,
            if out.fine_points >= FINE_POINTS_MAX { ", work cap reached" } else { "" },
            out.fine_s * 1e3
        ));
    }
    out.notes.extend(unplaced.into_iter().map(|s| format!("no stitch site for {s}")));
    if std::env::var_os("TOPO_FUSEDBG").is_some() {
        // Debug: the pieces left cut off, to compare the variants' stitchings.
        for c in &left {
            eprintln!("STITCH left ({} before, {} after): {}", out.before, out.after, cut_text(model, c));
        }
    }
    out.left = left;
    out
}

/// The pads at the ends of the routed connections of pour nets in `conns` (model pad indices,
/// sorted): their copper must reach the main component too ([`stitch_until`]'s `routed_pads`).
/// A label can name several pads (a switch with two pads "2"): the one at the terminal.
/// O(conns x pads).
pub fn routed_pour_pads(model: &BoardModel, conns: &[crate::route::ConnReport]) -> Vec<usize> {
    let pour_nets: std::collections::BTreeSet<i32> = model.pours.iter().map(|z| z.net).collect();
    let pad_of = |label: &str, at: [i64; 2]| {
        let q = Vec2 { x: at[0] as f64, y: -(at[1] as f64) };
        (0..model.pads.len())
            .filter(|&i| format!("{}.{}", model.pads[i].ref_, model.pads[i].number) == label)
            .min_by(|&a, &b| (model.pads[a].center.to_vec2() - q).len().total_cmp(&(model.pads[b].center.to_vec2() - q).len()).then(a.cmp(&b)))
    };
    let mut routed_pads: Vec<usize> = Vec::new();
    for c in conns.iter().filter(|c| c.status == "routed" && pour_nets.contains(&c.net)) {
        for t in [&c.from, &c.to] {
            if t.kind == "pad" {
                if let Some(p) = pad_of(&t.label, t.at) {
                    routed_pads.push(p);
                }
            }
        }
    }
    routed_pads.sort_unstable();
    routed_pads.dedup();
    routed_pads
}

/// Price of an island join of length `len` that runs `edge` from the pad's edge (nm): the length,
/// growing smoothly with the square of `edge` over the [`STUB_MAX_NM`] scale (graded, no cutoff:
/// a join twice the scale costs five times its length). O(1).
pub fn join_price(len: f64, edge: f64) -> f64 {
    let e = edge.max(0.0) / STUB_MAX_NM;
    len * (1.0 + e * e)
}

/// The island join of a cut-off component `c` that holds no pad: candidates are pairs (pad of the
/// net in the main component with copper on a routable layer of one of `c`'s islands, a site of
/// that island at least w/2 + the zone's minimum width deep whose track end keeps a gap to the net's other vias and tracks,
/// `site_ok(end, w/2)`, the D4 sliver rule), priced by [`join_price`]; tried cheapest first, the
/// first track `ok` accepts wins. Returns (track, pad index, length nm, end point math frame,
/// distance from the pad edge nm). O(sites x pads log + MAX_TRIES checker queries).
fn join_island(model: &BoardModel, reach: &PourReach, c: &Cut, ok: &dyn Fn(&Item) -> bool, site_ok: &dyn Fn(Vec2, f64) -> bool) -> Option<(Item, usize, f64, Vec2, f64)> {
    let w = model.rules.net_rule(c.net).width.max(1) as f64;
    let mut main: Vec<usize> = model
        .pours
        .iter()
        .enumerate()
        .filter(|(_, z)| z.net == c.net)
        .flat_map(|(zi, _)| reach.main_pads.get(&zi).cloned().unwrap_or_default())
        .collect();
    main.sort_unstable();
    main.dedup();
    let mut cand: Vec<(f64, usize, u32, Vec2, f64, f64)> = Vec::new();
    for &(zi, l, isl) in &c.islands {
        if !model.layers.iter().any(|x| x.id == l && x.routable) {
            continue;
        }
        let Some(ras) = reach.raster(zi, l) else { continue };
        let pads: Vec<usize> = main.iter().copied().filter(|&pi| model.pads[pi].layers.contains(&l)).collect();
        if pads.is_empty() {
            continue;
        }
        // Deep enough that the fill still holds the end after KiCad's min-width opening.
        let deep = 0.5 * w + model.pours[zi].min_thickness.max(0) as f64;
        for p in ras.island_sites(isl, deep, STEP) {
            if !site_ok(p, 0.5 * w) {
                continue;
            }
            for &pi in &pads {
                let pad = &model.pads[pi];
                let len = (p - pad.center.to_vec2()).len();
                let edge = crate::geom::point_skeleton(p, &model.skeleton(pad, l)).max(0.0);
                cand.push((join_price(len, edge), pi, l, p, len, edge));
            }
        }
    }
    cand.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.3.x.total_cmp(&b.3.x)).then(a.3.y.total_cmp(&b.3.y)));
    for &(_, pi, l, p, len, edge) in cand.iter().take(MAX_TRIES) {
        let t = track_item(c.net, l, w, model.pads[pi].center.to_vec2(), p, None);
        if ok(&t) {
            return Some((t, pi, len, p, edge));
        }
    }
    None
}

/// What the region stitching (stitch-vias classes 2-4) added.
#[derive(Clone, Debug, Default)]
pub struct RegionOut {
    /// New vias (KiCad frame).
    pub items: Vec<Item>,
    /// Per class (dropped, thin, area): pieces looked at, pieces that got a via, vias placed.
    pub dropped: [usize; 3],
    pub thin: [usize; 3],
    pub area: [usize; 3],
    /// Report lines (one per piece and a summary).
    pub notes: Vec<String>,
}

impl RegionOut {
    pub fn vias(&self) -> usize {
        self.dropped[2] + self.thin[2] + self.area[2]
    }
}

/// The net's other pour layers exist (a stitching via has a second layer to join).
fn multi_layer(model: &BoardModel, net: i32) -> bool {
    let mut ls: Vec<u32> = model.pours.iter().filter(|z| z.net == net && !z.fills.is_empty()).flat_map(|z| z.layers.iter().copied()).collect();
    ls.sort_unstable();
    ls.dedup();
    ls.len() >= 2
}

/// Via size and the depth a site needs (via radius + the zone's minimum width or KiCad's
/// minimum connection width, D6a) for `net`.
fn via_need(model: &BoardModel, opts: &Options, net: i32) -> Option<(crate::vias::ViaSize, f64)> {
    let sz = *via_sizes(&model.rules, net, opts).first()?;
    let ring = model.pours.iter().filter(|z| z.net == net).map(|z| z.min_thickness.max(0) as f64).fold(0.0, f64::max).max(model.rules.min_connection.max(0) as f64);
    Some((sz, 0.5 * sz.d as f64 + ring))
}

/// The layer-joining copper of `net` on `layer` (math frame): its vias (board and `all`) and its
/// through-hole pads with copper on `layer`, with their radii. O(vias + items + pads).
fn anchors_on(model: &BoardModel, all: &[Item], net: i32, layer: u32) -> Vec<(Vec2, f64)> {
    let mut out: Vec<(Vec2, f64)> = Vec::new();
    for v in model.vias.iter().filter(|v| v.net == net) {
        out.push((v.at.to_vec2(), 0.5 * v.d as f64));
    }
    for it in all {
        if let Item::Via(v) = it {
            if v.net == net {
                out.push((Vec2 { x: v.at.x as f64, y: -(v.at.y as f64) }, 0.5 * v.d as f64));
            }
        }
    }
    for pad in model.pads.iter().filter(|p| p.net == net && p.attr == crate::model::PadAttr::Pth && p.layers.contains(&layer)) {
        let c = pad.center.to_vec2();
        let rr = crate::geom::point_skeleton(c, &model.skeleton(pad, layer)).abs();
        out.push((c, rr));
    }
    out
}

/// stitch-vias region stitching, run once on the picked result after the cut-off repair
/// ([`stitch_until`], class 1) and before the plugin's zone refill (user 2026-11-21: "Stitching
/// VIAs placement on the board after routing but before redoing the ground planes"). On the
/// refill estimate ([`crate::pourcheck`]), redone between classes:
/// 2. **dropped** (`[planes] stitch_dropped`): an island KiCad's island removal would drop, of at
///    least pitch²/4, gets [`want_vias`] vias so its copper stays;
/// 3. **thin** (`[planes] stitch_thin_mm` > 0): the cores of the main islands (cells at least
///    thin/2 deep, so a neck narrower than `thin` separates them) that hold no via or
///    through-hole pad of the net, of at least pitch²/4, get [`want_vias`] vias;
/// 4. **area** (`[planes] stitch_area`): every lattice site of a main island farther than the
///    pitch from every via of its net (classic area stitching).
///
/// Every via: the net's class via, on the lattice (pitch from the board's grid origin) where a
/// site is legal, standing in solid fill of the net on two layers (never dangling), checker-legal,
/// priced by [`Ctx::sites`]; in classes 2-4 a site priced at or above the pitch (the value of
/// one via, in nm) is not used, so a via under a BGA (100 via prices) is never placed there.
/// Literature: TopoR does not stitch (TopoR 6.3 manual p. 121, p. 184); pitch lambda_g/20 ..
/// lambda_g/10 (ST AN5923) / "no further apart than lambda/10 and preferably as often as
/// lambda/20", sites that break a design rule skipped (Altium, "Via Stitching & Via Shielding").
/// O(classes x (pour check + candidates x placed + checker queries)).
pub fn stitch_regions(model: &BoardModel, opts: &Options, reg: &Registry, items: &[Item], routed_pads: &[usize], deadline: Option<std::time::Instant>, stop: &dyn Fn() -> bool) -> RegionOut {
    let mut out = RegionOut::default();
    let on = opts.stitch_dropped || opts.stitch_thin_nm > 0 || opts.stitch_area;
    if !on || model.pours.iter().all(|z| z.fills.is_empty()) {
        return out;
    }
    let expired = || deadline.is_some_and(|d| std::time::Instant::now() >= d) || crate::control::cancelled() || stop();
    let mut cx = Ctx::new(model, opts, reg, items);
    let pitch = cx.pitch;
    let small = 0.25 * pitch * pitch;
    let net_name = |net: i32| model.nets.iter().find(|n| n.code == net).map_or("?".to_string(), |n| n.name.clone());
    let layer_name = |l: u32| model.layers.iter().find(|x| x.id == l).map_or(format!("L{l}"), |x| x.name.clone());
    // Class 2: dropped islands.
    if opts.stitch_dropped && !expired() {
        let reach = check_pour_reach_until(model, &cx.all, routed_pads, deadline);
        for d in &reach.dropped {
            if expired() {
                break;
            }
            if d.area < small || !multi_layer(model, d.net) {
                continue;
            }
            let Some((sz, need)) = via_need(model, opts, d.net) else { continue };
            out.dropped[0] += 1;
            if reach.raster(d.island.0, d.island.1).is_some_and(|ras| cx.island_under_foreign_mask(ras, d.island.2, d.island.1, d.net)) {
                out.notes.push(format!(
                    "stitch (dropped island): {} on {} {:.2} mm² near {}: 0 via(s), under a solder-mask opening that exposes another net (KiCad would report a solder mask bridge)",
                    net_name(d.net),
                    layer_name(d.island.1),
                    d.area / 1e12,
                    kicad_mm(d.at)
                ));
                continue;
            }
            let want = cx.want(d.area);
            let placed = cx.place_spread(&reach, d.net, sz, need, std::slice::from_ref(&d.island), &|_, _| true, &[d.at], &[], want, Some(pitch), 0.0, true);
            out.dropped[1] += usize::from(!placed.is_empty());
            out.dropped[2] += placed.len();
            out.items.extend(cx.take_new());
            out.notes.push(format!(
                "stitch (dropped island): {} on {} {:.2} mm² near {}: {} of {} via(s){}",
                net_name(d.net),
                layer_name(d.island.1),
                d.area / 1e12,
                kicad_mm(d.at),
                placed.len(),
                want,
                placed.iter().map(|p| format!(" {}", kicad_mm(*p))).collect::<String>()
            ));
        }
    }
    // Class 3: thinly connected cores of the main islands.
    if opts.stitch_thin_nm > 0 && !expired() {
        let reach = check_pour_reach_until(model, &cx.all, routed_pads, deadline);
        let half = 0.5 * opts.stitch_thin_nm as f64;
        for (zi, z) in model.pours.iter().enumerate() {
            if expired() || !multi_layer(model, z.net) {
                continue;
            }
            let Some((sz, need)) = via_need(model, opts, z.net) else { continue };
            for &l in &z.layers {
                let Some(ras) = reach.raster(zi, l) else { continue };
                let (core, info) = ras.cores(&|isl| reach.island_in_main(zi, l, isl), half);
                let mut anchored = vec![false; info.len()];
                let anchors = anchors_on(model, &cx.all, z.net, l);
                // An anchor reaches the cores its fill touches: the fill starts at most a thermal gap
                // from its copper and a core cell lies `half` deeper still (plus a cell of slack).
                let gap = model.pours.iter().filter(|q| q.net == z.net).map(|q| q.thermal_gap.max(0) as f64).fold(0.0, f64::max);
                for &(a, ra) in &anchors {
                    for k in ras.cores_within(&core, a, ra + gap + 2.0 * half + 2.0 * ras.res()) {
                        anchored[k as usize] = true;
                    }
                }
                for k in 1..info.len() {
                    let (isl, area) = info[k];
                    if anchored[k] || area < small || expired() {
                        continue;
                    }
                    out.thin[0] += 1;
                    let want = cx.want(area);
                    let kk = k as u32;
                    let keep = |_: usize, p: Vec2| ras.core_at(&core, p) == kk;
                    let placed = cx.place_spread(&reach, z.net, sz, need, &[(zi, l, isl)], &keep, &[], &[], want, Some(pitch), 0.0, true);
                    out.thin[1] += usize::from(!placed.is_empty());
                    out.thin[2] += placed.len();
                    out.items.extend(cx.take_new());
                    out.notes.push(format!(
                        "stitch (thin neck < {:.2} mm): {} on {} core {:.2} mm²: {} of {} via(s){}",
                        2.0 * half / 1e6,
                        net_name(z.net),
                        layer_name(l),
                        area / 1e12,
                        placed.len(),
                        want,
                        placed.iter().map(|p| format!(" {}", kicad_mm(*p))).collect::<String>()
                    ));
                }
            }
        }
    }
    // Class 4: area stitching on the lattice.
    if opts.stitch_area && !expired() {
        let reach = check_pour_reach_until(model, &cx.all, routed_pads, deadline);
        for (zi, z) in model.pours.iter().enumerate() {
            if expired() || !multi_layer(model, z.net) {
                continue;
            }
            let Some((sz, need)) = via_need(model, opts, z.net) else { continue };
            for &l in &z.layers {
                let Some(ras) = reach.raster(zi, l) else { continue };
                let anchors: Vec<Vec2> = anchors_on(model, &cx.all, z.net, l).into_iter().map(|a| a.0).collect();
                let isls: Vec<(usize, u32, u32)> = (1..=ras.islands() as u32).filter(|&i| reach.island_in_main(zi, l, i)).map(|i| (zi, l, i)).collect();
                if isls.is_empty() {
                    continue;
                }
                out.area[0] += isls.len();
                let placed = cx.place_spread(&reach, z.net, sz, need, &isls, &|_, _| true, &[], &anchors, usize::MAX, Some(pitch), pitch, false);
                out.area[1] += usize::from(!placed.is_empty());
                out.area[2] += placed.len();
                out.items.extend(cx.take_new());
                out.notes.push(format!("stitch (area, pitch {:.2} mm): {} on {}: {} via(s)", pitch / 1e6, net_name(z.net), layer_name(l), placed.len()));
            }
        }
    }
    out.notes.insert(
        0,
        format!(
            "stitching vias (pitch {:.2} mm from the grid origin, at least {} per piece): dropped islands {} of {} stitched ({} vias), thin-neck cores {} of {} ({} vias), area {} vias; {} vias in all",
            pitch / 1e6,
            opts.stitch_min_vias.max(1),
            out.dropped[1],
            out.dropped[0],
            out.dropped[2],
            out.thin[1],
            out.thin[0],
            out.thin[2],
            out.area[2],
            out.vias()
        ),
    );
    out
}
