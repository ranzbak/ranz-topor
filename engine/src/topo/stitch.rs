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

/// stitch-vias: foreign-pad proximity price at contact (nm) and its reach from the via's edge
/// (nm): `PROX_PRICE_NM * max(0, 1 - d / PROX_REACH_NM)²`, graded (keeps pin rows and their
/// fanout room free; no cutoff inside the reach).
pub const PROX_PRICE_NM: f64 = 1.0e6;
pub const PROX_REACH_NM: f64 = 1.0e6;

/// One stitching lattice pitch (nm): rounded to 0.05 mm, at least 0.05 mm.
fn round_pitch(nm: i64, step: f64) -> f64 {
    ((nm.max(0) as f64 / step).round() * step).max(step)
}

/// The grid the stitching vias stand on (`[planes] stitch_on_grid`, user 2026-11-22: "the vias
/// should use that as well"): the user's KiCad via grid (KiCad's via grid override, else its
/// grid; `Options::grid_via_nm` / `grid_track_nm`, resolved by the plugin, topor/grid.py) from
/// the board's grid origin (`Options::grid_origin_nm`, the only origin source). [`Self::grid_at`]
/// is a function of the position so a region grid (BGA-GRID: a courtyard's ball array) can plug in
/// later; today one grid holds everywhere. No grid (none found, or `stitch_on_grid` off): the
/// lattice still starts at the grid origin, sites stay where the raster puts them.
#[derive(Clone, Copy, Debug)]
pub struct StitchGrid {
    grid: Option<super::gridsnap::Lattice>,
    /// The grid origin, math frame (nm).
    pub origin: Vec2,
}

impl StitchGrid {
    pub fn from_opts(opts: &Options) -> StitchGrid {
        let o = opts.grid_origin_nm;
        let step = if opts.grid_via_nm[0] > 0 && opts.grid_via_nm[1] > 0 { opts.grid_via_nm } else { opts.grid_track_nm };
        let grid = if opts.stitch_on_grid { super::gridsnap::Lattice::new(step, o) } else { None };
        StitchGrid { grid, origin: Vec2 { x: o[0] as f64, y: -(o[1] as f64) } }
    }

    /// The grid that holds at `p` (math frame), None without one. O(1).
    pub fn grid_at(&self, _p: Vec2) -> Option<super::gridsnap::Lattice> {
        self.grid
    }

    /// The grid at the lattice origin: the one the lattice pitch is a multiple of.
    pub fn base(&self) -> Option<super::gridsnap::Lattice> {
        self.grid_at(self.origin)
    }
}

/// The stitching lattice x and y pitch of `opts` (nm): `[planes] stitch_pitch_x_mm` /
/// `stitch_pitch_y_mm`, each rounded to a multiple of the via grid (at least one step) when
/// [`StitchGrid`] has one, else to 0.05 mm (at least 0.05 mm).
pub fn lattice_xy(opts: &Options) -> (f64, f64) {
    match StitchGrid::from_opts(opts).base() {
        Some(g) => (round_pitch(opts.stitch_pitch_x_nm, g.g[0]), round_pitch(opts.stitch_pitch_y_nm, g.g[1])),
        None => (round_pitch(opts.stitch_pitch_x_nm, 50_000.0), round_pitch(opts.stitch_pitch_y_nm, 50_000.0)),
    }
}

/// The x shift of every odd lattice row (nm): half the x pitch on a staggered lattice, rounded to
/// the via grid when there is one (so every lattice site is a grid point); 0 when straight.
pub fn lattice_shift(opts: &Options) -> f64 {
    if !lattice_staggered(opts) {
        return 0.0;
    }
    let px = lattice_xy(opts).0;
    match StitchGrid::from_opts(opts).base() {
        Some(g) => (0.5 * px / g.g[0]).round() * g.g[0],
        None => 0.5 * px,
    }
}

/// The notes for a lattice pitch the via grid changed (user pitch not a multiple of the grid).
pub fn lattice_grid_notes(opts: &Options) -> Vec<String> {
    let Some(g) = StitchGrid::from_opts(opts).base() else { return Vec::new() };
    let (px, py) = lattice_xy(opts);
    let mut out = Vec::new();
    for (k, want, got, step) in [("x", opts.stitch_pitch_x_nm, px, g.g[0]), ("y", opts.stitch_pitch_y_nm, py, g.g[1])] {
        if (want as f64 - got).abs() > 1.0 {
            out.push(format!(
                "stitching vias: stitch_pitch_{k}_mm {} is not a multiple of the via grid {} mm: the lattice uses {} mm",
                want as f64 / 1e6,
                step / 1e6,
                got / 1e6
            ));
        }
    }
    out
}

/// Whether the lattice is staggered (`[planes] stitch_lattice = "staggered"`): every other row
/// shifted by half the x pitch.
pub fn lattice_staggered(opts: &Options) -> bool {
    opts.stitch_lattice == "staggered"
}

/// The stitching lattice's scalar pitch (nm): sqrt(x pitch · y pitch), so one via per pitch² is
/// one via per lattice cell; also the value of a via and the spread distance of the stitching.
pub fn lattice_pitch(opts: &Options) -> f64 {
    let (px, py) = lattice_xy(opts);
    (px * py).sqrt()
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
    /// The site's graded cost, nm (squeeze, foreign pads, BGA, plane neck): ranks the sites, and
    /// classes 2-4 weigh it against the value of a via. With a via grid every site stands on it
    /// (round 5: off the grid never).
    cost: f64,
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
    /// Lattice x / y pitch (nm) and stagger (odd rows shifted by px/2).
    px: f64,
    py: f64,
    staggered: bool,
    /// x shift of the odd lattice rows (nm; 0 straight).
    shift: f64,
    grid: StitchGrid,
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
    /// Round 5 (user 2026-11-22: off the via grid NEVER): legal raster sites of the current piece
    /// left out because they stand off the via grid (only named in the piece's warning when it
    /// gets fewer vias than wanted; [`Ctx::place_piece`]).
    off_grid_legal: std::cell::Cell<usize>,
    /// Why the last [`Ctx::place_piece`] placed fewer vias than wanted ("" = it did not).
    why: String,
    /// r5c: [`Ctx::sites`] also offers legal raster points off the via grid, priced cost +
    /// `off_grid_nm` · amount (graded): only for a cut piece's first via, its only connection.
    off_ok: bool,
    /// The `off_grid` price (nm per site fully off the grid, [`super::gridsnap::price_nm`]).
    off_grid_nm: f64,
    /// `stitch_nudge_nm` (nm; 0 = a blocked lattice point is dropped).
    nudge: f64,
    /// Round 5: how the vias were placed.
    kinds: Kinds,
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
            px: lattice_xy(opts).0,
            py: lattice_xy(opts).1,
            staggered: lattice_staggered(opts),
            shift: lattice_shift(opts),
            grid: StitchGrid::from_opts(opts),
            origin: StitchGrid::from_opts(opts).origin,
            min_vias: opts.stitch_min_vias,
            per_area: opts.stitch_per_area,
            // stitch_thin = 0 switches class 3 off, not the antipad-neck price: 1 mm then.
            thin: if opts.stitch_thin_nm > 0 { opts.stitch_thin_nm as f64 } else { 1.0e6 },
            pad_buckets,
            masks: mask_holes(model, items),
            off_grid_legal: std::cell::Cell::new(0),
            why: String::new(),
            off_ok: false,
            off_grid_nm: super::gridsnap::price_nm(opts),
            nudge: opts.stitch_nudge_nm.max(0) as f64,
            kinds: Kinds::default(),
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

    /// The site tests and own cost (nm) of a via of `net` (size `sz`) at `p` in the fill `ras` of
    /// layer `l` (`keep` is the caller's): None unless the fill of another pour layer of `net` in
    /// its main component is `need` deep there, the via stays off the net's own pads and out of
    /// solder-mask openings that expose another net; else the graded price of squeeze on both
    /// layers, foreign-pad proximity, plane neck and a BGA (last resort). O(pours · layers + pads).
    #[allow(clippy::too_many_arguments)]
    fn site_cost(&self, reach: &PourReach, ras: &crate::pourcheck::Raster, l: u32, net: i32, sz: crate::vias::ViaSize, need: f64, p: Vec2) -> Option<f64> {
        let r = 0.5 * sz.d as f64;
        let (_, _, d2) = main_elsewhere(reach, self.model, net, Some(l), p, need)?;
        if !self.own_pad_ok(p, r, net) || self.under_foreign_mask(p, r, net) {
            return None;
        }
        let d1 = ras.depth_at(p);
        let mut price = self.squeeze(d1 - r, sz.d as f64) + self.squeeze(d2 - r, sz.d as f64) + self.prox(p, r, net);
        price += self.plane_neck(reach, p, r, net);
        if super::planes::in_bga(&self.bga, p) {
            // Power-via rule (user 2026-11-13): under a BGA a via is a last resort.
            price += self.via_price_nm * 100.0;
        }
        Some(price)
    }

    /// Candidate sites in `islands` (filtered by `keep(island index, p)`): lattice sites, then
    /// (with `fallback`) the D4 cell lattice; each with fill depth ≥ `need` on its own layer and
    /// in the main fill of another pour layer of `net` with the same depth, off the net's own pads.
    #[allow(clippy::too_many_arguments)]
    fn sites(&self, reach: &PourReach, net: i32, sz: crate::vias::ViaSize, need: f64, islands: &[(usize, u32, u32)], keep: &dyn Fn(usize, Vec2) -> bool, fallback: bool) -> Vec<Site> {
        let mut out: Vec<Site> = Vec::new();
        // per site (rounded nm, layer): its price, None when it fails the site tests
        let mut memo: std::collections::HashMap<(i64, i64, u32), Option<f64>> = std::collections::HashMap::new();
        for (k, &(zi, l, isl)) in islands.iter().enumerate() {
            let Some(ras) = reach.raster(zi, l) else { continue };
            let lat = ras.lattice_sites(isl, need, self.px, self.py, self.shift, self.origin);
            let fb = if fallback { ras.island_sites(isl, need, STEP) } else { Vec::new() };
            // stitch_on_grid (round 5, user 2026-11-22: off the grid NEVER): with a via grid a
            // fallback (raster) site offers only the via-grid points of its cell, each through the
            // same tests; the raster point itself is never a site (a legal one is counted for the
            // piece's warning). Without a grid (stitch_on_grid false, or no grid known) the raster
            // point is the site.
            // Round 5 (verifier r2, lead 2026-11-22): no lattice-offset price any more. It pulled the
            // D4 single via up to 2.98 mm from its pad towards a lattice point; the lattice is now a
            // placement rule of its own (Ctx::place_lattice), not a price.
            let site_price = |p: Vec2| -> Option<f64> {
                if !keep(k, p) {
                    return None;
                }
                self.site_cost(reach, ras, l, net, sz, need, p)
            };
            let key = |p: Vec2| (p.x.round() as i64, p.y.round() as i64, l);
            for p in lat {
                if let std::collections::hash_map::Entry::Vacant(e) = memo.entry(key(p)) {
                    let pr = site_price(p);
                    e.insert(pr);
                    if let Some(cost) = pr {
                        out.push(Site { p, cost });
                    }
                }
            }
            for p in fb {
                if let Some(g) = self.grid.grid_at(p) {
                    let qs: Vec<Vec2> = if g.on(p) { vec![g.nearest(p)] } else { g.cell(p).to_vec() };
                    let mut any = false;
                    for q in qs {
                        if ras.island_at(q) != isl || ras.depth_at(q) < need {
                            continue;
                        }
                        let pr = match memo.get(&key(q)) {
                            Some(&pr) => pr,
                            None => {
                                let pr = site_price(q);
                                memo.insert(key(q), pr);
                                if let Some(cost) = pr {
                                    out.push(Site { p: q, cost });
                                }
                                pr
                            }
                        };
                        any |= pr.is_some();
                    }
                    if !g.on(p) && (self.off_ok || !any) && !memo.contains_key(&key(p)) {
                        let pr = site_price(p);
                        memo.insert(key(p), pr);
                        if let Some(cost) = pr {
                            if !any {
                                self.off_grid_legal.set(self.off_grid_legal.get() + 1);
                            }
                            if self.off_ok {
                                // r5c: a connection via may stand off the grid, at the graded
                                // off_grid price (the gridsnap factor), never ahead of a legal
                                // grid point of the same score.
                                out.push(Site { p, cost: off_grid_site_cost(cost, self.off_grid_nm, g.amount(p)) });
                            }
                        }
                    }
                    continue;
                }
                if memo.contains_key(&key(p)) {
                    continue;
                }
                let pr = site_price(p);
                memo.insert(key(p), pr);
                let Some(cost) = pr else { continue };
                out.push(Site { p, cost });
            }
        }
        out
    }

    /// The D4 rule (round 5: used for `stitch_min_vias` 1 and a one-via piece only, "1 = the
    /// behaviour before stitching vias"; [`Ctx::place_piece`]). Place up to `n` stitching vias of
    /// `sz` for `net` in `islands` (see [`Ctx::sites`]): farthest-point greedy — the first via (no
    /// `anchors`) at the site nearest `targets` plus its price, each next one at the site
    /// maximising min(distance to the anchors and the vias placed, pitch) − price, one graded
    /// comparison (no lattice preference since round 5); a site closer than `min_gap` to an anchor or placed via is not eligible; with
    /// `value`, a site priced at or above it is not worth a via and is skipped (classes 2-4).
    /// Each candidate is checked (own-net copper, then the checker); at most [`MAX_TRIES`]
    /// checker refusals per via. Returns the sites placed (their vias are in
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
        // A site whose own cost is at or above the value of one via is not worth one (classes
        // 2-4): it is left out, the cheaper sites stay (verifier V2: a priced-out lattice site
        // ended the piece).
        let mut dead: Vec<bool> = cands.iter().map(|c| value.is_some_and(|v| c.cost >= v)).collect();
        let mut placed: Vec<Vec2> = Vec::new();
        let mut near: Vec<Vec2> = anchors.to_vec();
        let pitch = self.pitch;
        'via: while placed.len() < n {
            let mut tries = 0usize;
            loop {
                // One graded score (verifier V1): distance and the site's own cost.
                let score = |s: &Site| -> f64 {
                    if near.is_empty() {
                        let t = targets.iter().map(|&t| (t - s.p).len()).fold(f64::INFINITY, f64::min);
                        -(if t.is_finite() { t } else { 0.0 } + s.cost)
                    } else {
                        let d = near.iter().map(|&a| (a - s.p).len()).fold(f64::INFINITY, f64::min);
                        d.min(pitch) - s.cost
                    }
                };
                let eligible = |i: usize| {
                    !dead[i] && (min_gap <= 0.0 || near.iter().all(|&a| (a - cands[i].p).len() >= min_gap))
                };
                let mut best: Option<(f64, usize)> = None;
                for i in 0..cands.len() {
                    if !eligible(i) {
                        continue;
                    }
                    let s = score(&cands[i]);
                    let better = match best {
                        None => true,
                        Some((bs, bi)) => s > bs || (s == bs && (cands[i].p.x, cands[i].p.y) < (cands[bi].p.x, cands[bi].p.y)),
                    };
                    if better {
                        best = Some((s, i));
                    }
                }
                let Some((_, i)) = best else { break 'via };
                let p = cands[i].p;
                dead[i] = true;
                // Own-net copper refusals are filtered, not counted (as before stitch-vias): only
                // checker refusals count towards MAX_TRIES.
                if !self.own_copper_ok(p, r, net) {
                    continue;
                }
                let v = via_item(net, p, sz, None);
                if self.ok(std::slice::from_ref(&v)) {
                    self.add(&v);
                    self.new.push(v);
                    placed.push(p);
                    near.push(p);
                    continue 'via;
                }
                tries += 1;
                if n != usize::MAX && tries >= MAX_TRIES {
                    break 'via;
                }
            }
        }
        placed
    }

    /// Try a via of `net` at `p`: own-net copper gap, then the checker; placed (and in
    /// [`Ctx::take_new`]) when both accept. O(checker query).
    fn try_via(&mut self, p: Vec2, net: i32, sz: crate::vias::ViaSize) -> Option<bool> {
        if !self.own_copper_ok(p, 0.5 * sz.d as f64, net) {
            return None; // filtered, not a checker refusal (as place_spread)
        }
        let v = via_item(net, p, sz, None);
        if self.ok(std::slice::from_ref(&v)) {
            self.add(&v);
            self.new.push(v);
            Some(true)
        } else {
            Some(false)
        }
    }

    /// stitch-vias round 5 (user 2026-11-22: "looks like random placement, I expected either for
    /// the placer to search for the corners, or to have a placement grid"): the vias of one piece
    /// (`islands`, filtered by `keep`), at most `n`.
    /// - `stitch_min_vias` 1 and a one-via piece: the D4 rule, unchanged ([`Ctx::place_spread`],
    ///   first via near `targets`): "1 = the behaviour before stitching vias".
    /// - **Lattice fill**: a piece holding at least `stitch_min_vias` legal board-lattice points
    ///   gets vias on lattice points only, at most `n` and never more than the lattice holds
    ///   ([`Ctx::place_lattice`]; a blocked point is dropped or moved along the via grid by at most
    ///   `stitch_nudge_mm`).
    /// - **Extremities**: below `stitch_min_vias` (a piece with fewer lattice points, or lattice
    ///   points refused), the rest up to `stitch_min_vias` at the piece's extremities on the via
    ///   grid ([`Ctx::place_extremes`]). This is the only way a via off the lattice is added.
    ///
    /// O(lattice fill + extremities).
    #[allow(clippy::too_many_arguments)]
    fn place_piece(&mut self, reach: &PourReach, net: i32, sz: crate::vias::ViaSize, need: f64, islands: &[(usize, u32, u32)], keep: &dyn Fn(usize, Vec2) -> bool, targets: &[Vec2], n: usize, value: Option<f64>) -> Vec<Vec2> {
        let n_min = self.min_vias.max(1);
        self.off_grid_legal.set(0);
        let mut lattice_ok = true;
        let placed = if n_min <= 1 && n <= 1 {
            self.place_spread(reach, net, sz, need, islands, keep, targets, &[], n, value, 0.0, true)
        } else {
            let legal = self.sites(reach, net, sz, need, islands, keep, false).iter().filter(|s| value.is_none_or(|v| s.cost < v)).count();
            lattice_ok = legal >= n_min;
            let mut placed = if lattice_ok { self.place_lattice(reach, net, sz, need, islands, keep, &[], n, value, 0.0) } else { Vec::new() };
            if placed.len() < n_min {
                let anchors = placed.clone();
                placed.extend(self.place_extremes(reach, net, sz, need, islands, keep, &anchors, n_min - placed.len(), value));
            }
            placed
        };
        self.why = self.short_reason(placed.len(), n, n_min, lattice_ok);
        placed
    }

    /// Why a piece got `placed` of `n` wanted vias (minimum `n_min`); "" when it got them all.
    /// Round 5 (user 2026-11-22): stitching vias never leave the via grid, so a piece whose only
    /// room lies between grid points gets fewer vias (pattern piece E), and the warning says so.
    fn short_reason(&self, placed: usize, n: usize, n_min: usize, lattice_ok: bool) -> String {
        if placed >= n {
            return String::new();
        }
        if placed >= n_min.min(n) {
            return if lattice_ok {
                "fewer legal board-lattice points than wanted (area vias stand on the lattice only)".to_string()
            } else {
                format!("fewer legal board-lattice points than stitch_min_vias {n_min}: the minimum at the piece's extremities, no more")
            };
        }
        let k = self.off_grid_legal.get();
        match self.grid.base() {
            Some(g) if k > 0 => format!(
                "no more legal site on the via grid {} x {} mm; {k} legal spot(s) between grid points not used: stitching vias never leave the via grid (a finer KiCad via grid, or more room in the pour, gives this piece its vias)",
                g.g[0] / 1e6,
                g.g[1] / 1e6
            ),
            _ => "no more legal site: no spot of the piece has room for a via that also stands in the net's fill on another layer".to_string(),
        }
    }

    /// Lattice fill (stitch-vias round 5): vias on the board lattice points of `islands` only
    /// (filtered by `keep`), at most `n`; a point closer than `min_gap` to `anchors` or a placed via
    /// is skipped; with `value`, a point whose site cost reaches it is not worth a via. Order:
    /// farthest-point over the legal points (first the one farthest from the anchors, or from the
    /// points' centroid without anchors), score = distance − site cost, ties by (x, y): with `n`
    /// below the lattice count the chosen points spread to the piece's ends. A **blocked** point
    /// (no room: the site tests fail; or own-net copper / the checker refuses it) is dropped, or
    /// with `stitch_nudge_mm` > 0 moved to the cheapest legal via-grid point at most that far
    /// (site cost + 2 · distance, graded; never off the grid). O(points · placed + checker queries).
    #[allow(clippy::too_many_arguments)]
    fn place_lattice(&mut self, reach: &PourReach, net: i32, sz: crate::vias::ViaSize, need: f64, islands: &[(usize, u32, u32)], keep: &dyn Fn(usize, Vec2) -> bool, anchors: &[Vec2], n: usize, value: Option<f64>, min_gap: f64) -> Vec<Vec2> {
        // every lattice point inside the fill (any depth): (p, island index, site cost if legal)
        let mut pts: Vec<(Vec2, usize, Option<f64>)> = Vec::new();
        let mut seen: std::collections::HashMap<(i64, i64), usize> = std::collections::HashMap::new();
        for (k, &(zi, l, isl)) in islands.iter().enumerate() {
            let Some(ras) = reach.raster(zi, l) else { continue };
            for p in ras.lattice_sites(isl, 0.0, self.px, self.py, self.shift, self.origin) {
                if !keep(k, p) {
                    continue;
                }
                let c = if ras.depth_at(p) >= need { self.site_cost(reach, ras, l, net, sz, need, p) } else { None };
                let c = c.filter(|&c| value.is_none_or(|v| c < v));
                match seen.entry((p.x.round() as i64, p.y.round() as i64)) {
                    std::collections::hash_map::Entry::Occupied(e) => {
                        // the same point on another layer's island: keep the cheaper legal one
                        let j = *e.get();
                        if c.is_some_and(|c| pts[j].2.is_none_or(|o| c < o)) {
                            pts[j] = (p, k, c);
                        }
                    }
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(pts.len());
                        pts.push((p, k, c));
                    }
                }
            }
        }
        let nudge = self.nudge;
        let cap = n.min(pts.iter().filter(|q| q.2.is_some() || nudge > 0.0).count());
        let centre = if pts.is_empty() { Vec2::default() } else { pts.iter().fold(Vec2::default(), |a, q| a + q.0) * (1.0 / pts.len() as f64) };
        let mut near: Vec<Vec2> = anchors.to_vec();
        let mut placed: Vec<Vec2> = Vec::new();
        let mut done = vec![false; pts.len()];
        let mut blocked: Vec<usize> = Vec::new();
        let gap_ok = |near: &[Vec2], p: Vec2| min_gap <= 0.0 || near.iter().all(|&a| (a - p).len() >= min_gap);
        // farthest-point order over the legal points, then (nudge) the blocked ones
        let mut served = vec![false; pts.len()];
        for phase in 0..2 {
            if phase == 1 {
                if nudge <= 0.0 {
                    break;
                }
                for &i in &blocked {
                    done[i] = false;
                }
            }
            loop {
                if placed.len() >= cap {
                    break;
                }
                let mut best: Option<(f64, usize)> = None;
                for (i, q) in pts.iter().enumerate() {
                    let pool = if phase == 0 { q.2.is_some() } else { blocked.contains(&i) || q.2.is_none() };
                    if done[i] || !pool || !gap_ok(&near, q.0) {
                        continue;
                    }
                    let d = if near.is_empty() { (q.0 - centre).len() } else { near.iter().map(|&a| (a - q.0).len()).fold(f64::INFINITY, f64::min) };
                    let s = d - q.2.unwrap_or(0.0);
                    let better = match best {
                        None => true,
                        Some((bs, bi)) => s > bs || (s == bs && (q.0.x, q.0.y) < (pts[bi].0.x, pts[bi].0.y)),
                    };
                    if better {
                        best = Some((s, i));
                    }
                }
                let Some((_, i)) = best else { break };
                done[i] = true;
                let p = pts[i].0;
                if phase == 0 {
                    if self.try_via(p, net, sz) == Some(true) {
                        served[i] = true;
                        self.kinds.lattice += 1;
                        placed.push(p);
                        near.push(p);
                    } else {
                        self.kinds.refused += 1;
                        blocked.push(i);
                    }
                    continue;
                }
                // phase 1: move along the via grid, at most `nudge`
                let (zi, l, isl) = islands[pts[i].1];
                let k = pts[i].1;
                let Some(ras) = reach.raster(zi, l) else { continue };
                let Some(g) = self.grid.grid_at(p) else { continue };
                let base = g.nearest(p);
                let m = (nudge / g.g[0].min(g.g[1])).ceil() as i64;
                let mut cand: Vec<(f64, Vec2)> = Vec::new();
                for j in -m..=m {
                    for ii in -m..=m {
                        let q = base + Vec2 { x: ii as f64 * g.g[0], y: j as f64 * g.g[1] };
                        let d = (q - p).len();
                        if d < 1.0 || d > nudge + 1.0 || ras.island_at(q) != isl || ras.depth_at(q) < need || !keep(k, q) || !gap_ok(&near, q) {
                            continue;
                        }
                        let Some(c) = self.site_cost(reach, ras, l, net, sz, need, q) else { continue };
                        if value.is_some_and(|v| c >= v) {
                            continue;
                        }
                        cand.push((c + 2.0 * d, q));
                    }
                }
                cand.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.x.total_cmp(&b.1.x)).then(a.1.y.total_cmp(&b.1.y)));
                let mut tries = 0usize;
                for &(_, q) in &cand {
                    match self.try_via(q, net, sz) {
                        Some(true) => {
                            served[i] = true;
                            self.kinds.nudged.push(q);
                            placed.push(q);
                            near.push(q);
                            break;
                        }
                        Some(false) => {
                            tries += 1;
                            if tries >= MAX_TRIES {
                                break;
                            }
                        }
                        None => {}
                    }
                }
            }
        }
        // Wanted lattice points left without a via (blocked: no room, priced out or refused, and
        // not moved): the count was not reached and no via stands within `min_gap` of them.
        if placed.len() < n {
            self.kinds.dropped += (0..pts.len()).filter(|&i| !served[i] && gap_ok(&near, pts[i].0)).count();
        }
        placed
    }

    /// Extremities (stitch-vias round 5, the minimum-via repair): up to `n` vias at the piece's
    /// extremities: farthest-point sampling over the piece's legal sites on the via grid (all of
    /// them: lattice, grid points of the raster cells), the first at the site farthest from the
    /// site area's centroid (or, with `anchors`, from them), each next one farthest from the vias
    /// chosen, score = distance − site cost (graded: squeeze, clearance neck, foreign pads, BGA;
    /// no saturation, no lattice preference), ties by (x, y). A rectangle gets two opposite corners
    /// first, then the other two; a strip or a part behind a thin neck its two ends. Off the via
    /// grid only when the piece has no legal grid site (counted). O(sites · n + checker queries).
    #[allow(clippy::too_many_arguments)]
    fn place_extremes(&mut self, reach: &PourReach, net: i32, sz: crate::vias::ViaSize, need: f64, islands: &[(usize, u32, u32)], keep: &dyn Fn(usize, Vec2) -> bool, anchors: &[Vec2], n: usize, value: Option<f64>) -> Vec<Vec2> {
        if n == 0 {
            return Vec::new();
        }
        let all = self.sites(reach, net, sz, need, islands, keep, true);
        let alive: Vec<Site> = all.into_iter().filter(|s| value.is_none_or(|v| s.cost < v)).collect();
        // centroid of the site area (the raster sample of the islands)
        let mut sum = Vec2::default();
        let mut cnt = 0usize;
        for (k, &(zi, l, isl)) in islands.iter().enumerate() {
            let Some(ras) = reach.raster(zi, l) else { continue };
            for p in ras.island_sites(isl, need, STEP) {
                if keep(k, p) {
                    sum = sum + p;
                    cnt += 1;
                }
            }
        }
        let centre = if cnt > 0 { sum * (1.0 / cnt as f64) } else { Vec2::default() };
        // With a via grid every site is a grid point (Ctx::sites): no off-grid path.
        let cands = alive;
        let mut dead = vec![false; cands.len()];
        let mut near: Vec<Vec2> = anchors.to_vec();
        let mut placed: Vec<Vec2> = Vec::new();
        'via: while placed.len() < n {
            let mut tries = 0usize;
            loop {
                let mut best: Option<(f64, usize)> = None;
                for (i, s) in cands.iter().enumerate() {
                    if dead[i] {
                        continue;
                    }
                    let d = if near.is_empty() { (s.p - centre).len() } else { near.iter().map(|&a| (a - s.p).len()).fold(f64::INFINITY, f64::min) };
                    let sc = d - s.cost;
                    let better = match best {
                        None => true,
                        Some((bs, bi)) => sc > bs || (sc == bs && (s.p.x, s.p.y) < (cands[bi].p.x, cands[bi].p.y)),
                    };
                    if better {
                        best = Some((sc, i));
                    }
                }
                let Some((_, i)) = best else { break 'via };
                dead[i] = true;
                let p = cands[i].p;
                match self.try_via(p, net, sz) {
                    Some(true) => {
                        self.kinds.corners.push(p);
                        placed.push(p);
                        near.push(p);
                        continue 'via;
                    }
                    Some(false) => {
                        tries += 1;
                        if tries >= MAX_TRIES {
                            break 'via;
                        }
                    }
                    None => {}
                }
            }
        }
        placed
    }
}

/// stitch-vias round 5: how the stitching vias were placed (notes; tools/stitchvias_lattice_share.py).
#[derive(Clone, Debug, Default)]
struct Kinds {
    /// Vias on a board-lattice point.
    lattice: usize,
    /// Vias at a piece's extremities (the minimum-via repair), math frame.
    corners: Vec<Vec2>,
    /// Vias moved off their blocked lattice point along the via grid (`stitch_nudge_mm`).
    nudged: Vec<Vec2>,
    /// Legal lattice points refused (own-net copper gap or the checker).
    refused: usize,
    /// Lattice points wanted but left without a via (no room, priced out or refused; dropped).
    dropped: usize,
}

impl Kinds {
    /// One note: the share and the positions of the vias off the lattice.
    fn note(&self, what: &str) -> Option<String> {
        if self.lattice == 0 && self.corners.is_empty() && self.nudged.is_empty() && self.dropped == 0 {
            return None;
        }
        let pts = |v: &[Vec2]| v.iter().map(|p| format!(" {}", kicad_mm(*p))).collect::<String>();
        Some(format!(
            "{what} placement: {} on the board lattice, {} at piece extremities, {} moved along the via grid (stitch_nudge_mm); {} lattice point(s) refused by the checker, {} wanted lattice point(s) dropped; extremities at{}; nudged at{}",
            self.lattice,
            self.corners.len(),
            self.nudged.len(),
            self.refused,
            self.dropped,
            pts(&self.corners),
            pts(&self.nudged)
        ))
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
            let placed = if again { Vec::new() } else { cx.place_piece(&reach, c.net, sz, need, &c.islands, &|_, _| true, &targets, want, None) };
            let done_vias = !placed.is_empty();
            for p in &placed {
                out.notes.push(format!("stitch via {} mm at {} for {}", sz.d as f64 / 1e6, kicad_mm(*p), cut_text(model, c)));
            }
            if done_vias && placed.len() < want {
                out.notes.push(format!("stitch vias: {} of {} wanted for {} ({})", placed.len(), want, cut_text(model, c), cx.why));
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
            // r5c (user 2026-11-22: "when it doesn't fit wires can be pushed off grid, same like in
            // KiCAD"): a pad stub is the pad's only connection, so with a via grid the coarse lattice
            // first tries every grid point of every pad, then (off_pass) the ray points off the grid,
            // nearest first, each named in a warning; the fine lattice (after the coarse passes, every
            // grid point already tried) uses only the ray points. Stitching extras stay on the grid.
            let has_grid = cx.grid.base().is_some();
            for off_pass in [false, true] {
            if off_pass && (fine || !has_grid) {
                break;
            }
            // r5c (user 2026-11-22: connection copper goes off the grid where it does not fit, as in
            // KiCad): after every grid point (island vias and pad stubs) failed, the piece's first
            // via, its only connection, may stand off the grid (graded off_grid price, warned), before
            // an off-grid pad stub; further vias stay on the grid.
            if off_pass && !c.islands.is_empty() {
                cx.off_ok = true;
                let first = cx.place_spread(&reach, c.net, sz, need, &c.islands, &|_, _| true, &targets, &[], 1, None, 0.0, true);
                cx.off_ok = false;
                for p in &first {
                    out.notes.push(format!("stitch via {} mm at {} for {}", sz.d as f64 / 1e6, kicad_mm(*p), cut_text(model, c)));
                    if cx.grid.grid_at(*p).is_some_and(|g| !g.on(*p)) {
                        out.notes.push(format!(
                            "stitch via off the via grid at {} for {}: the piece's only connection, no legal via-grid point in it nor for a pad stub (connection copper goes off the grid where it does not fit, as in KiCad; stitching extras never do)",
                            kicad_mm(*p),
                            cut_text(model, c)
                        ));
                    }
                }
                if !first.is_empty() {
                    out.island_vias += first.len();
                    out.items.extend(cx.take_new());
                    done = true;
                    break 'lat;
                }
            }
            let use_grid = has_grid && !fine && !off_pass;
            let off_grid = has_grid && !use_grid;
            for &pi in &c.pads {
                let pad = &model.pads[pi];
                let cpos = pad.center.to_vec2();
                let w = model.rules.net_rule(c.net).width.max(1) as f64;
                let layers: Vec<u32> = pad.layers.iter().copied().filter(|l| model.layers.iter().any(|x| x.id == *l && x.routable)).collect();
                let mut tries = 0usize;
                // stitch_on_grid (round 5b, user 2026-11-22: off the via grid NEVER): every via-grid
                // point out to d_max, nearest the pad first (ties by x, y), ring by ring; snapping
                // the ray points instead stepped over grid points between two rays (qmtech C6.2,
                // C32.1). Without a grid the ray points, as before.
                let grid_pts: Option<Vec<(f64, Vec2)>> = cx.grid.grid_at(cpos).filter(|_| use_grid).map(|g| {
                    let base = g.nearest(cpos);
                    let m = (d_max / g.g[0].min(g.g[1]).max(1.0)).ceil() as i64 + 1;
                    let mut v: Vec<(f64, Vec2)> = Vec::new();
                    for j in -m..=m {
                        for i in -m..=m {
                            let q = base + Vec2 { x: i as f64 * g.g[0], y: j as f64 * g.g[1] };
                            let rr = (q - cpos).len();
                            if rr <= d_max {
                                v.push((rr, q));
                            }
                        }
                    }
                    v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.x.total_cmp(&b.1.x)).then(a.1.y.total_cmp(&b.1.y)));
                    v
                });
                let mut gi = 0usize;
                // Out from the pad centre; the own-pad test keeps a via off the pad.
                let mut d: f64 = 0.2e6;
                while d <= d_max && tries < 4 * MAX_TRIES && !(fine && out.fine_points >= FINE_POINTS_MAX) {
                    let ring: Vec<Vec2> = match &grid_pts {
                        Some(gp) => {
                            let from = gi;
                            while gi < gp.len() && gp[gi].0 <= d {
                                gi += 1;
                            }
                            gp[from..gi].iter().map(|x| x.1).collect()
                        }
                        None => (0..n_dir)
                            .map(|k| {
                                let a = k as f64 * std::f64::consts::TAU / n_dir as f64;
                                cpos + Vec2 { x: a.cos(), y: a.sin() } * d
                            })
                            .collect(),
                    };
                    out.fine_points += usize::from(fine) * ring.len();
                    for p in ring {
                        {
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
                                    if off_grid && cx.grid.grid_at(p).is_some_and(|g| !g.on(p)) {
                                        out.notes.push(format!(
                                            "pad stub {}.{} off the via grid at {}: the pad's only connection, no legal via-grid point within {:.0} mm of the pad (connection copper goes off the grid where it does not fit, as in KiCad; stitching extras never do)",
                                            pad.ref_,
                                            pad.number,
                                            kicad_mm(p),
                                            STUB_MAX_NM / 1e6
                                        ));
                                    }
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
                    }
                    d += d_step;
                }
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
    // Round 5b: the reason names the via grid (stitching vias never leave it).
    let why = match cx.grid.base() {
        Some(g) => format!(
            " (no legal point of the via grid {} x {} mm for an island via, nor for a pad stub within {:.0} mm of its pads: stitching vias never leave the via grid)",
            g.g[0] / 1e6,
            g.g[1] / 1e6,
            STUB_MAX_NM / 1e6
        ),
        None => String::new(),
    };
    out.notes.extend(unplaced.into_iter().map(|s| format!("no stitch site for {s}{why}")));

    if let Some(n) = cx.kinds.note("stitch vias (cut pieces)") {
        out.notes.push(n);
    }
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
            let placed = cx.place_piece(&reach, d.net, sz, need, std::slice::from_ref(&d.island), &|_, _| true, &[d.at], want, Some(pitch));
            out.dropped[1] += usize::from(!placed.is_empty());
            out.dropped[2] += placed.len();
            out.items.extend(cx.take_new());
            out.notes.push(format!(
                "stitch (dropped island): {} on {} {:.2} mm² near {}: {} of {} via(s){}{}",
                net_name(d.net),
                layer_name(d.island.1),
                d.area / 1e12,
                kicad_mm(d.at),
                placed.len(),
                want,
                placed.iter().map(|p| format!(" {}", kicad_mm(*p))).collect::<String>(),
                if cx.why.is_empty() { String::new() } else { format!(" ({})", cx.why) }
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
                    let placed = cx.place_piece(&reach, z.net, sz, need, &[(zi, l, isl)], &keep, &[], want, Some(pitch));
                    out.thin[1] += usize::from(!placed.is_empty());
                    out.thin[2] += placed.len();
                    out.items.extend(cx.take_new());
                    out.notes.push(format!(
                        "stitch (thin neck < {:.2} mm): {} on {} core {:.2} mm²: {} of {} via(s){}{}",
                        2.0 * half / 1e6,
                        net_name(z.net),
                        layer_name(l),
                        area / 1e12,
                        placed.len(),
                        want,
                        placed.iter().map(|p| format!(" {}", kicad_mm(*p))).collect::<String>(),
                        if cx.why.is_empty() { String::new() } else { format!(" ({})", cx.why) }
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
                let placed = cx.place_lattice(&reach, z.net, sz, need, &isls, &|_, _| true, &anchors, usize::MAX, Some(pitch), pitch);
                out.area[1] += usize::from(!placed.is_empty());
                out.area[2] += placed.len();
                out.items.extend(cx.take_new());
                out.notes.push(format!(
                    "stitch (area, lattice {:.2} x {:.2} mm): {} on {}: {} via(s){}",
                    cx.px / 1e6,
                    cx.py / 1e6,
                    net_name(z.net),
                    layer_name(l),
                    placed.len(),
                    placed.iter().map(|p| format!(" {}", kicad_mm(*p))).collect::<String>()
                ));
            }
        }
    }
    out.notes.insert(
        0,
        format!(
            "stitching vias (lattice {:.2} x {:.2} mm {} from the grid origin{}, at least {} per piece): dropped islands {} of {} stitched ({} vias), thin-neck cores {} of {} ({} vias), area {} vias; {} vias in all",
            cx.px / 1e6,
            cx.py / 1e6,
            if cx.staggered { "staggered" } else { "straight" },
            match cx.grid.base() {
                Some(g) => format!(", on the via grid {} x {} mm", g.g[0] / 1e6, g.g[1] / 1e6),
                None => String::new(),
            },
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
    for (k, n) in lattice_grid_notes(opts).into_iter().enumerate() {
        out.notes.insert(1 + k, n);
    }
    if let Some(n) = cx.kinds.note("stitching vias") {
        out.notes.push(n);
    }
    out
}

/// Cost of a connection-via site off the via grid (r5c): its own price plus the graded `off_grid`
/// price (nm) times the off-grid `amount` (0 on the grid, 1 at a cell centre), so a site off the
/// grid never ranks ahead of a legal grid site of the same own price and a nearer-to-grid spot
/// ranks ahead of a farther one. O(1).
fn off_grid_site_cost(cost: f64, off_grid_nm: f64, amount: f64) -> f64 {
    cost + off_grid_nm * amount
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_grid_site_ranks_behind_a_grid_site_of_the_same_price_and_by_distance() {
        let price = 2.0e6;
        let grid = Site { p: Vec2 { x: 0.0, y: 0.0 }, cost: 5.0e5 };
        let near = Site { p: Vec2 { x: 1.0e5, y: 0.0 }, cost: off_grid_site_cost(5.0e5, price, 0.1) };
        let far = Site { p: Vec2 { x: 5.0e5, y: 0.0 }, cost: off_grid_site_cost(5.0e5, price, 0.8) };
        assert!(grid.cost < near.cost && near.cost < far.cost);
        assert_eq!(off_grid_site_cost(5.0e5, price, 0.0), grid.cost, "on the grid costs nothing extra");
        assert!((far.cost - near.cost - 0.7 * price).abs() < 1e-6);
        // a zero price makes the sites tie (the factor off), so the price is what ranks them
        assert_eq!(off_grid_site_cost(5.0e5, 0.0, 0.8), 5.0e5);
    }
}
