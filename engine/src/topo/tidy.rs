//! Output tidy: copper that KiCad's **Edit → Cleanup Tracks & Vias** leaves unchanged.
//!
//! KiCad's cleaner (`pcbnew/tracks_cleaner.cpp`, KiCad 10.0 @ 86a7a3f) is the specification the
//! written copper must satisfy, with every option on. This pass runs on the final item list of a
//! route, after drawing, stitching and every check, and removes each cause the cleaner acts on
//! (rule, KiCad function, what is done here):
//!
//! * **null segment** (`TRACKS_CLEANER::cleanup`, `IsNull`): a segment with start == end is dropped;
//! * **duplicate segment** (`cleanup`, both ends on the other's ends, same width and layer): the
//!   second copy is dropped;
//! * **track a pad makes redundant**: wholly inside one pad of its net
//!   (`TRACKS_CLEANER::deleteTracksInPads`), or with both ends within reach of that pad, one end free
//!   (the terminal) and the other, the joint to the rest of the trace, at least as near to the
//!   pad's anchor: `TestTrackEndpointDangling` counts a pad that reaches both ends of a track once,
//!   for the end nearer to the pad's anchor, so the terminal end counts as unconnected (qmtech
//!   ADV_TX2- at USB1.3, an 86 µm piece). Such a segment is dropped (repeated until none is left);
//!   everything that met it met it within reach of that pad, so the connection is unchanged. A
//!   piece from the pad's anchor out to its edge (anchor = the terminal, nearer) stays;
//! * **T junction** (`CONNECTIVITY_DATA::TestTrackEndpointDangling`,
//!   `pcbnew/connectivity/connectivity_data.cpp`: an end is connected when it lies within width/2
//!   of a connected item, but an item that reaches *both* ends of a track counts once, for the end
//!   nearer to that item's own ends): a track end that stops on the middle of a same-net segment
//!   is moved onto that segment's centre line and the segment is split there, so the junction is
//!   one exact vertex shared by three track ends. Without it a short segment next to a T (both
//!   its ends within reach of the trunk and of its other neighbour) is judged dangling and the
//!   cleaner removes it and, in cascade, the rest of the trace (qmtech ADV_DVDD, ADV_CEC);
//! * **collinear consecutive segments** (`TRACKS_CLEANER::mergeCollinearSegments`: same width,
//!   `SEG::ApproxCollinear` within 1 nm, the shared end touching nothing else): merged into one,
//!   so the cleaner has nothing to merge.
//!
//! Through-own-pad junctions, vias and arcs are kept as drawn. A stub into a pour ends in the pour's
//! *refilled* copper; the plugin hands the board back with the zones refilled for that (KiCad's
//! cleaner judges the zones with the fill they have, `TestTrackEndpointDangling`).
//!
//! Complexity: O(I log I) for I items plus O(S_g²) per net and layer group of S_g segments (the
//! T search), O(S · P_net) for the pad test.
use std::collections::{BTreeMap, HashMap, HashSet};

/// draw-via-neck r7 (verifier vfy-draw-via-neck-r6 F1): the slack (nm) of the in-pad test, which
/// is KiCad's own (`geom::kicad::track_inside`: the track's `ERROR_INSIDE` polygon at the board's
/// maximum error inside the pad; r6 and before: both round ends within 5 um of lying inside,
/// which kept a 0.1407 mm jog KiCad deletes, its polygon 5.35 um inside its true round end): a
/// polygon vertex up to this far outside the pad counts as inside (KiCad's integer coordinates).
const PAD_SLACK_NM: f64 = 2.0;

use crate::geom::dist::Skeleton;
use crate::geom::quant::{IVec2, TrackPrim};
use crate::geom::Vec2;
use crate::model::{BoardModel, Item, TrackItem};


/// What [`tidy`] changed, for the run report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TidyStats {
    pub null: usize,
    pub duplicate: usize,
    pub in_pad: usize,
    pub t_split: usize,
    pub t_moved: usize,
    pub merged: usize,
    /// Uuids of the kept (hand) tracks a new collinear segment took over (step 4a): the caller
    /// removes them with the same commit (`RouteResult::remove_uuids`).
    pub replaced_kept: Vec<String>,
}

impl TidyStats {
    pub fn changed(&self) -> bool {
        *self != TidyStats::default()
    }

    /// One report line.
    pub fn line(&self) -> String {
        format!(
            "cleanup-safe output (KiCad's Cleanup Tracks & Vias leaves it unchanged): {} null and {} duplicate segments dropped, {} segments a pad of their net makes redundant dropped, {} T junctions made exact vertices ({} track ends moved onto the trunk), {} collinear segments merged, {} existing tracks merged into the new segment that continues them straight on (replaced in the same step)",
            self.null, self.duplicate, self.in_pad, self.t_split, self.t_moved, self.merged, self.replaced_kept.len()
        )
    }
}

type P = (i64, i64);

fn p_of(v: IVec2) -> P {
    (i64::from(v.x), i64::from(v.y))
}

fn iv(p: P) -> IVec2 {
    IVec2 { x: p.0 as i32, y: p.1 as i32 }
}

/// KiCad frame (items) -> math frame (model).
fn math(p: P) -> Vec2 {
    Vec2 { x: p.0 as f64, y: -(p.1 as f64) }
}

fn seg_of(it: &Item) -> Option<(P, P)> {
    match it {
        Item::Track(TrackItem { prim: TrackPrim::Seg { a, b }, .. }) => Some((p_of(*a), p_of(*b))),
        _ => None,
    }
}

fn set_seg(it: &mut Item, a: P, b: P) {
    if let Item::Track(t) = it {
        t.prim = TrackPrim::Seg { a: iv(a), b: iv(b) };
    }
}

/// Depth of `q` inside the pad copper `sk` (distance to its boundary; negative outside). O(v).
fn depth(sk: &Skeleton, q: Vec2) -> f64 {
    match sk {
        Skeleton::Poly { pts, r } if pts.len() >= 3 => {
            let n = pts.len();
            let inside = (0..n).all(|i| (pts[(i + 1) % n] - pts[i]).cross(q - pts[i]) >= 0.0);
            if inside {
                let edge = (0..n)
                    .map(|i| {
                        let (a, b) = (pts[i], pts[(i + 1) % n]);
                        let d = b - a;
                        let l = d.len();
                        if l == 0.0 {
                            (q - a).len()
                        } else {
                            d.cross(q - a) / l
                        }
                    })
                    .fold(f64::INFINITY, f64::min);
                r + edge
            } else {
                -crate::geom::dist::point_skeleton(q, sk)
            }
        }
        _ => -crate::geom::dist::point_skeleton(q, sk),
    }
}

/// KiCad `SEG::ApproxCollinear` (threshold 1 nm): both ends of the shorter segment within 1 nm
/// of the longer one's line. Integer arithmetic as KiCad's `mutualDistanceSquared`. O(1).
fn approx_collinear(s1: (P, P), s2: (P, P)) -> bool {
    let l2 = |s: (P, P)| {
        let (dx, dy) = (i128::from(s.1 .0 - s.0 .0), i128::from(s.1 .1 - s.0 .1));
        dx * dx + dy * dy
    };
    let (a, b) = if l2(s1) < l2(s2) { (s2, s1) } else { (s1, s2) };
    let p = i128::from(a.0 .1) - i128::from(a.1 .1);
    let q = i128::from(a.1 .0) - i128::from(a.0 .0);
    let r = -p * i128::from(a.0 .0) - q * i128::from(a.0 .1);
    let l = p * p + q * q;
    if l == 0 {
        return false;
    }
    let near = |pt: P| {
        let det = p * i128::from(pt.0) + q * i128::from(pt.1) + r;
        // det² / l <= 1, rounded as KiCad's rescale
        let d2 = (det * det + l / 2) / l;
        d2 <= 1
    };
    near(b.0) && near(b.1)
}

/// Copper layers a via of the output spans (empty = through all).
fn via_on(layers: &[u32], layer: u32, all: &[u32]) -> bool {
    if layers.len() < 2 {
        return true;
    }
    let rank = |l: u32| all.iter().position(|&x| x == l).unwrap_or(usize::MAX);
    let (a, b) = (rank(layers[0]), rank(layers[layers.len() - 1]));
    let r = rank(layer);
    a.min(b) <= r && r <= a.max(b)
}

/// One piece of a track end pulled back inside its pad ([`pad_ends_off_edge`]), KiCad frame:
/// net, layer, the cut-away stretch `from`-`to` and the track width.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeTrim {
    pub net: i32,
    pub layer: u32,
    pub from: (i64, i64),
    pub to: (i64, i64),
    pub width: i64,
}

/// Track ends at the centre of a pad that itself lies within the board-edge clearance
/// (`rules.edge_clearance`; a `.kicad_dru` `edge_clearance` rule raises it): the end is moved
/// along its segment, still inside the pad, to the first point where the track keeps the edge
/// clearance. KiCad connects a track end anywhere inside the pad
/// (`CONNECTIVITY_DATA::TestTrackEndpointDangling`) and its DRC measures the track, not the pad
/// it enters (`DRC_TEST_PROVIDER_EDGE_CLEARANCE`), so the stub from the centre to the pad's
/// inner side would be a violation of its own while the pad's is pre-existing.
///
/// Conservative: the new end keeps the whole track end disc inside the pad (depth ≥ width/2, so
/// the copper and the connection width are unchanged), the rest of the segment must keep the
/// clearance, the segment is only ever shortened (no new clearance to anything), a via at the
/// end keeps it, and a board whose edge check is `ignore`d is left alone. Returns the cut-away
/// stretches. O(T · (P_net + E)) for T tracks, E outline segments.
pub fn pad_ends_off_edge(model: &BoardModel, items: &mut [Item]) -> Vec<EdgeTrim> {
    let mut out = Vec::new();
    let ec = model.rules.edge_clear() as f64;
    if ec <= 0.0 || model.outline.is_empty() || model.rules.ignored_checks.iter().any(|c| c == "copper_edge_clearance") {
        return out;
    }
    let outline = super::board::outline_segments(model);
    let d_edge = |q: Vec2| outline.iter().map(|&(a, b)| super::draw::seg_dist(q, a, b)).fold(f64::INFINITY, f64::min);
    let seg_edge = |p: Vec2, q: Vec2| outline.iter().map(|&(a, b)| super::draw::seg_seg_dist(p, q, a, b)).fold(f64::INFINITY, f64::min);
    let via_at: HashSet<(i32, P)> = items
        .iter()
        .filter_map(|it| if let Item::Via(v) = it { Some((v.net, (v.at.x, v.at.y))) } else { None })
        .chain(model.vias.iter().map(|v| (v.net, (v.at.x, -v.at.y))))
        .collect();
    let mut pads_of: HashMap<i32, Vec<usize>> = HashMap::new();
    for (i, p) in model.pads.iter().enumerate() {
        if p.net != 0 {
            pads_of.entry(p.net).or_default().push(i);
        }
    }
    // a margin above the 1 nm grid rounding and KiCad's polygon error
    const MARGIN: f64 = 2_000.0;
    for it in items.iter_mut() {
        let (Some((a, b)), Item::Track(t)) = (seg_of(it), &*it) else { continue };
        let (net, layer, width) = (t.net, t.layer, t.width);
        let half = 0.5 * width as f64;
        let req = ec + half + MARGIN;
        let Some(pads) = pads_of.get(&net) else { continue };
        let mut new = (a, b);
        let mut cut = Vec::new();
        for (end, other) in [(a, b), (b, a)] {
            if via_at.contains(&(net, end)) {
                continue;
            }
            let (pe, po) = (math(end), math(other));
            if d_edge(pe) >= req {
                continue;
            }
            let Some(pi) = pads.iter().copied().find(|&pi| {
                let p = &model.pads[pi];
                (p.layers.is_empty() || p.layers.contains(&layer)) && (p.center.x as i64, -(p.center.y as i64)) == end
            }) else {
                continue;
            };
            let sk = model.skeleton(&model.pads[pi], layer);
            let len = (po - pe).len();
            if len < 1.0 {
                continue;
            }
            let u = (po - pe) * (1.0 / len);
            // first point along the segment that keeps the clearance (10 µm steps, then halving)
            let step = 10_000.0;
            let mut hi = None;
            let mut s = step;
            while s < len {
                if d_edge(pe + u * s) >= req {
                    hi = Some(s);
                    break;
                }
                s += step;
            }
            let Some(mut hi) = hi else { continue };
            let mut lo = hi - step;
            for _ in 0..12 {
                let m = 0.5 * (lo + hi);
                if d_edge(pe + u * m) >= req {
                    hi = m;
                } else {
                    lo = m;
                }
            }
            let q = pe + u * hi;
            let qk: P = (q.x.round() as i64, (-q.y).round() as i64);
            if depth(&sk, math(qk)) < half || d_edge(math(qk)) < ec + half || seg_edge(math(qk), po) < ec + half {
                continue;
            }
            if end == a {
                new.0 = qk;
            } else {
                new.1 = qk;
            }
            cut.push(EdgeTrim { net, layer, from: end, to: qk, width });
        }
        if new != (a, b) && new.0 != new.1 {
            set_seg(it, new.0, new.1);
            out.extend(cut);
        }
    }
    out
}

/// pad-snap (A1b): the track ends that end inside a pad of their net off its centre, as written
/// (after [`tidy`]): `(count, largest distance from the centre nm)`. A terminal end (no other track
/// end of the net and layer at that point, no via there) inside a pad of its net (depth >= 0 on the
/// track's layer) more than 1 um from the pad's anchor. KiCad's connectivity connects it, but the
/// push-and-shove router links a pad only at its anchor (`pns_node.cpp` `NODE::addSolid`), so for
/// dragging and footprint moves the end is loose. O(T x P_net) for T terminal ends and P_net
/// pads of their net.
pub fn off_centre_pad_ends(model: &BoardModel, items: &[Item]) -> (usize, f64) {
    let mut ends: HashMap<(i32, u32, P), usize> = HashMap::new();
    let mut vias: HashSet<(i32, P)> = model.vias.iter().map(|v| (v.net, (v.at.x, -v.at.y))).collect();
    // pad-snap r2b debug (`TOPO_PADSNAPDBG`): the wire of each off-centre end
    let dbg = std::env::var_os("TOPO_PADSNAPDBG").is_some();
    let mut wire_of: HashMap<(i32, u32, P), Option<usize>> = HashMap::new();
    for it in items {
        match it {
            Item::Track(t) => {
                let (a, b) = match t.prim {
                    TrackPrim::Seg { a, b } => (a, b),
                    TrackPrim::Arc { start, end, .. } => (start, end),
                };
                for q in [p_of(a), p_of(b)] {
                    *ends.entry((t.net, t.layer, q)).or_insert(0) += 1;
                    if dbg {
                        wire_of.insert((t.net, t.layer, q), t.wire);
                    }
                }
            }
            Item::Via(v) => {
                vias.insert((v.net, (v.at.x, v.at.y)));
            }
        }
    }
    for k in &model.kept {
        let (a, b) = match k.prim {
            TrackPrim::Seg { a, b } => (a, b),
            TrackPrim::Arc { start, end, .. } => (start, end),
        };
        for q in [a, b] {
            *ends.entry((k.net, k.layer, (i64::from(q.x), -i64::from(q.y)))).or_insert(0) += 1;
        }
    }
    let mut pads_of: HashMap<i32, Vec<usize>> = HashMap::new();
    for (pi, pad) in model.pads.iter().enumerate() {
        pads_of.entry(pad.net).or_default().push(pi);
    }
    let mut sk_cache: HashMap<(usize, u32), Skeleton> = HashMap::new();
    let (mut n, mut worst) = (0usize, 0.0f64);
    let mut keys: Vec<&(i32, u32, P)> = ends.iter().filter(|(_, &c)| c == 1).map(|(k, _)| k).collect();
    keys.sort_unstable();
    for &(net, layer, q) in keys {
        if net == 0 || vias.contains(&(net, q)) {
            continue;
        }
        for &pi in pads_of.get(&net).map_or(&[][..], |v| &v[..]) {
            let pad = &model.pads[pi];
            if !(pad.layers.is_empty() || pad.layers.contains(&layer)) {
                continue;
            }
            let sk = sk_cache.entry((pi, layer)).or_insert_with(|| model.skeleton(pad, layer));
            if depth(sk, math(q)) < 0.0 {
                continue;
            }
            let d = ((q.0 - pad.center.x) as f64).hypot((q.1 + pad.center.y) as f64);
            if d > 1_000.0 {
                n += 1;
                worst = worst.max(d);
                if dbg {
                    eprintln!("PADOFF wire {:?} net {net} L{layer} end ({:.4},{:.4}) pad {}.{} off {:.4} mm", wire_of.get(&(net, layer, q)).copied().flatten(), q.0 as f64 / 1e6, q.1 as f64 / 1e6, pad.ref_, pad.number, d / 1e6);
                }
            }
            break;
        }
    }
    (n, worst)
}

/// How a track meets a pad of its net off the pad's centre ([`pad_end_breaches`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PadEndKind {
    /// A track end with nothing else of the net at that point lies inside the pad off its
    /// centre: the trace ends there.
    Terminal,
    /// Two or more track ends meet inside the pad off its centre (a bend or a width step inside
    /// the pad) and no chain of segments joins that point to a track end at the centre.
    Junction,
    /// A segment's copper touches the pad (its centre line crosses it, or its round end overlaps
    /// it) with no end inside it and no chain to the centre: KiCad may connect it, but the trace
    /// does not end at the pad's centre.
    PassThrough,
    /// A track end inside the pad off its centre that lies on the centre line of a segment of
    /// the net whose chain reaches the centre (a T onto the trace that ends there, as
    /// `fuse_branches` makes it): connected to the centre through that copper, but the trace
    /// itself ends beside the centre. A hint, as every kind: KiCad's real Cleanup keeps such a T
    /// (r7, user's method 2026-11-26 "if the track is gone after the cleanup, it was not good":
    /// the project `harness.kicad_accepts` pad_centre, T joins 0.17 / 0.40 / 0.75 mm off the
    /// centre on an axis host and on a 45-degree host, all kept; tests/test_kicad_accepts.py). It
    /// stays joined to the centre only while its host survives (esp D5.1: Cleanup removed the
    /// host in a short cascade, the same on main, and the T became an off-centre Terminal): the
    /// board check judges the board after the real Cleanup too (`pad_centre_check.py --cleanup`).
    Tee,
}

/// One breach of the pad-centre invariant ([`pad_end_breaches`]).
#[derive(Clone, Debug, PartialEq)]
pub struct PadEndBreach {
    pub kind: PadEndKind,
    pub net: i32,
    pub layer: u32,
    /// `ref.number` of the pad.
    pub pad: String,
    /// The track end (KiCad frame, nm); for [`PadEndKind::PassThrough`] the segment's start.
    pub at: (i64, i64),
    /// Distance from the pad centre (nm).
    pub off: f64,
    /// The pad centre lies within the board-edge clearance (0 included) plus half the track's
    /// width of the outline: the drawing's pad anchor (padend.rs) and [`pad_ends_off_edge`] move
    /// such an end off the centre on purpose (edge-clear), still inside the pad.
    pub edge: bool,
    /// The track's round end at the pad centre breaks the clearance to a pad of another net or
    /// to a hole (a track wider than the room round its pad, e.g. a QFN pad next to its
    /// neighbours): the drawing's pad anchor (padend.rs `pad_anchor`, edge-clear) moves the end
    /// inside the pad on purpose. The stub from the centre to the moved end would lie wholly in
    /// the pad, and KiCad's Cleanup deletes such a stub (`TRACKS_CLEANER::deleteTracksInPads`,
    /// emulated by [`tidy`]), so the end stays where the anchor put it.
    pub centre_blocked: bool,
    /// The wire of the track (`TrackItem::wire`).
    pub wire: Option<usize>,
}

impl PadEndBreach {
    /// A centre-distance HINT (r7, lead 2026-11-26 after the KiCad test project
    /// `harness.kicad_accepts`): a track end off the centre of a pad whose centre could take it
    /// (not an edge-clear pad, not a blocked centre), every kind. Not a failure: KiCad's real
    /// Cleanup keeps off-centre T joins and off-centre terminals alike (user: "if the track is
    /// gone after the cleanup, it was not good"; 2026-11-21: "off center is not detached, the
    /// only way to know is to check the KiCad file"). The pass / fail criterion is survival of
    /// KiCad's Cleanup (`pad_centre_check.py --cleanup`, the in-pad test [`tidy`] shares with
    /// KiCad).
    pub fn hint(&self) -> bool {
        !self.edge && !self.centre_blocked
    }
}

/// draw-via-neck r6 (user 2026-11-26: "every trace still connects to the MIDDLE of its pad,
/// otherwise KiCad Cleanup deletes it"; r7: KiCad's real Cleanup keeps off-centre ends and T
/// joins, so these are hints, [`PadEndBreach::hint`]): the pad-centre listing of `items` (KiCad frame, as
/// written; the kept tracks of `model` count as copper of their net). Per pad of a net and per
/// copper layer of the pad, the segments of the net on that layer whose copper touches the pad
/// (copper gap <= 0) must all reach a track end at the pad's centre (within 1 um) along chains of
/// such segments (shared ends): a bend or a width step inside the pad on the way out from the
/// centre is fine, the trace still ends at the centre. A touching segment that does not is a
/// breach, by its end in the pad: [`PadEndKind::Terminal`] / [`PadEndKind::Junction`] (one /
/// several track ends there), [`PadEndKind::Tee`] (the end lies on a reached segment), or
/// [`PadEndKind::PassThrough`] (no end in the pad). Ends at a via of the net are no trace ends: a
/// segment with no other end in the pad is skipped when one of its via ends lies in the pad (a
/// via in the pad holds it; r7, verifier r6 F3: a via outside the pad no longer hides a
/// PassThrough). [`off_centre_pad_ends`] counts only terminal ends
/// in the pad. A pad no track touches is not judged (the connectivity check's job). Sorted.
/// O(S x P_net x chain rounds) for S segments and P_net pads of their net.
pub fn pad_end_breaches(model: &BoardModel, items: &[Item]) -> Vec<PadEndBreach> {
    // (net, layer) -> segments (a, b, wire, width, drawn); arcs by their chord
    type L = (P, P, Option<usize>, i64, bool);
    let mut links: HashMap<(i32, u32), Vec<L>> = HashMap::new();
    let mut count: HashMap<(i32, u32, P), usize> = HashMap::new();
    let mut vias: HashSet<(i32, P)> = model.vias.iter().map(|v| (v.net, (v.at.x, -v.at.y))).collect();
    let mut add = |net: i32, layer: u32, a: P, b: P, wire: Option<usize>, width: i64, drawn: bool| {
        links.entry((net, layer)).or_default().push((a, b, wire, width, drawn));
        for q in [a, b] {
            *count.entry((net, layer, q)).or_insert(0) += 1;
        }
    };
    for it in items {
        match it {
            Item::Track(t) => {
                let (a, b) = match t.prim {
                    TrackPrim::Seg { a, b } => (p_of(a), p_of(b)),
                    TrackPrim::Arc { start, end, .. } => (p_of(start), p_of(end)),
                };
                add(t.net, t.layer, a, b, t.wire, t.width, true);
            }
            Item::Via(v) => {
                vias.insert((v.net, (v.at.x, v.at.y)));
            }
        }
    }
    for k in &model.kept {
        let (a, b) = match k.prim {
            TrackPrim::Seg { a, b } => (a, b),
            TrackPrim::Arc { start, end, .. } => (start, end),
        };
        let f = |q: IVec2| (i64::from(q.x), -i64::from(q.y));
        add(k.net, k.layer, f(a), f(b), None, k.width, false);
    }
    let mut pads_of: HashMap<i32, Vec<usize>> = HashMap::new();
    for (pi, pad) in model.pads.iter().enumerate() {
        if pad.net != 0 {
            pads_of.entry(pad.net).or_default().push(pi);
        }
    }
    // Edge-clear pads: the end's round copper at the centre comes closer to the outline than the
    // edge clearance (0 included: the copper would stick out of the board), so the drawing's pad
    // anchor (padend.rs) or `pad_ends_off_edge` moves the end inside the pad on purpose.
    let ec = (model.rules.edge_clear() as f64).max(0.0);
    let edge_on = !model.outline.is_empty() && !model.rules.ignored_checks.iter().any(|c| c == "copper_edge_clearance");
    let outline = if edge_on { super::board::outline_segments(model) } else { Vec::new() };
    let near_edge = |c: Vec2, width: i64| edge_on && outline.iter().map(|&(a, b)| super::draw::seg_dist(c, a, b)).fold(f64::INFINITY, f64::min) < ec + 0.5 * width as f64 + 2_000.0;
    let seg = |a: P, b: P| crate::geom::prim::Seg { a: math(a), b: math(b) };
    // Blocked centre: the end's disc (width `width`) at the pad centre comes closer than the
    // required clearance (less 1 um) to a pad of another net on the layer or to a hole.
    let blocked = |pi: usize, layer: u32, width: i64| {
        let pad = &model.pads[pi];
        let c = pad.center.to_vec2();
        let t = crate::geom::dist::Track { prim: crate::geom::prim::Prim::Seg(crate::geom::prim::Seg { a: c, b: c }), w: width as f64 };
        let reach = 0.5 * width as f64 + 5_000_000.0;
        let pads = model.pads.iter().filter(|q| q.net != pad.net && (q.layers.is_empty() || q.layers.contains(&layer)) && (q.center.to_vec2() - c).len() < reach).any(|q| {
            crate::geom::dist::copper_gap_skeleton(&t, &model.skeleton(q, layer)) < model.required_clearance(pad.net, q) - 1_000.0
        });
        let hc = model.rules.hole_clearance.max(0) as f64;
        let holes = model.holes.iter().any(|h| {
            let hc2 = h.at.to_vec2();
            (hc2 - c).len() > 1_000.0 && (hc2 - c).len() - 0.5 * (h.d as f64) - 0.5 * (width as f64) < hc - 1_000.0
        });
        pads || holes
    };
    let mut out = Vec::new();
    let mut groups: Vec<(i32, u32)> = links.keys().copied().filter(|k| k.0 != 0).collect();
    groups.sort_unstable();
    for (net, layer) in groups {
        let ls = &links[&(net, layer)];
        for &pi in pads_of.get(&net).map_or(&[][..], |v| &v[..]) {
            let pad = &model.pads[pi];
            if !(pad.layers.is_empty() || pad.layers.contains(&layer)) {
                continue;
            }
            let sk = model.skeleton(pad, layer);
            let c = pad.center.to_vec2();
            // The segments whose copper touches the pad.
            let touch: Vec<usize> = (0..ls.len())
                .filter(|&i| {
                    let (a, b, _, w, _) = ls[i];
                    let t = crate::geom::dist::Track { prim: crate::geom::prim::Prim::Seg(seg(a, b)), w: w as f64 };
                    crate::geom::dist::copper_gap_skeleton(&t, &sk) <= 0.0
                })
                .collect();
            if !touch.is_empty() {
                let at_c = |q: P| (math(q) - c).len() <= 1_000.0;
                let mut reached = vec![false; touch.len()];
                let mut pts: HashSet<P> = HashSet::new();
                for (k, &i) in touch.iter().enumerate() {
                    let (a, b, ..) = ls[i];
                    if at_c(a) || at_c(b) {
                        reached[k] = true;
                        pts.insert(a);
                        pts.insert(b);
                    }
                }
                let mut grew = true;
                while grew {
                    grew = false;
                    for (k, &i) in touch.iter().enumerate() {
                        let (a, b, ..) = ls[i];
                        if !reached[k] && (pts.contains(&a) || pts.contains(&b)) {
                            reached[k] = true;
                            pts.insert(a);
                            pts.insert(b);
                            grew = true;
                        }
                    }
                }
                let on_reached = |q: P| touch.iter().enumerate().any(|(k, &i)| reached[k] && crate::geom::dist::point_seg(math(q), &seg(ls[i].0, ls[i].1)) <= 1_000.0);
                for (k, &i) in touch.iter().enumerate() {
                    let (a, b, wire, width, drawn) = ls[i];
                    if reached[k] || !drawn {
                        continue;
                    }
                    let end = [a, b].into_iter().find(|&q| !vias.contains(&(net, q)) && depth(&sk, math(q)) >= 0.0);
                    if end.is_none() && [a, b].iter().any(|&q| vias.contains(&(net, q)) && depth(&sk, math(q)) >= 0.0) {
                        continue;
                    }
                    let (kind, at) = match end {
                        Some(q) if on_reached(q) => (PadEndKind::Tee, q),
                        Some(q) if count[&(net, layer, q)] == 1 => (PadEndKind::Terminal, q),
                        Some(q) => (PadEndKind::Junction, q),
                        None => (PadEndKind::PassThrough, a),
                    };
                    let off = if kind == PadEndKind::PassThrough { crate::geom::dist::point_seg(c, &seg(a, b)) } else { (math(at) - c).len() };
                    out.push(PadEndBreach { kind, net, layer, pad: format!("{}.{}", pad.ref_, pad.number), at, off, edge: near_edge(c, width), centre_blocked: blocked(pi, layer, width), wire });
                }
            }
        }
    }
    out.sort_by(|x, y| (x.kind, x.net, x.layer, x.at, &x.pad).cmp(&(y.kind, y.net, y.layer, y.at, &y.pad)));
    out.dedup();
    out
}

/// Make `items` (KiCad frame, with `conn[i]` the connection of item `i`) stable under KiCad's
/// Cleanup Tracks & Vias (module doc). O(see module).
pub fn tidy(model: &BoardModel, items: &mut Vec<Item>, conn: &mut Vec<usize>) -> TidyStats {
    let mut st = TidyStats::default();
    debug_assert_eq!(items.len(), conn.len());
    // physical copper order (F.Cu, In1..., B.Cu) for blind / buried via spans
    let mut cu: Vec<u32> = model.layers.iter().map(|l| l.id).collect();
    cu.sort_by_key(|&l| match l {
        0 => 0,
        2 => u32::MAX,
        x => x,
    });
    let pads_of: HashMap<i32, Vec<usize>> = model.pads.iter().enumerate().filter(|(_, p)| p.net != 0).fold(HashMap::new(), |mut m, (i, p)| {
        m.entry(p.net).or_insert_with(Vec::new).push(i);
        m
    });
    let mut sk_cache: HashMap<(usize, u32), Skeleton> = HashMap::new();
    let mut pad_sk = |pi: usize, layer: u32| -> Skeleton { sk_cache.entry((pi, layer)).or_insert_with(|| model.skeleton(&model.pads[pi], layer)).clone() };
    let pad_on = |pi: usize, layer: u32| {
        let p = &model.pads[pi];
        p.layers.is_empty() || p.layers.contains(&layer)
    };

    // 1. null and duplicate segments
    let mut keep = vec![true; items.len()];
    let mut seen: HashSet<(i32, u32, i64, P, P)> = HashSet::new();
    for (i, it) in items.iter().enumerate() {
        let (Some((a, b)), Item::Track(t)) = (seg_of(it), it) else { continue };
        if a == b {
            keep[i] = false;
            st.null += 1;
            continue;
        }
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        if !seen.insert((t.net, t.layer, t.width, lo, hi)) {
            keep[i] = false;
            st.duplicate += 1;
        }
    }

    // 2. segments a pad makes redundant, repeated until none is left: (a) wholly inside a pad of
    // their net (deleteTracksInPads); (b) both ends within reach of such a pad, one end free (the
    // terminal) and the other, the joint to the rest of the trace, at least as near to the pad's
    // anchor: TestTrackEndpointDangling counts the pad for that joint end only, so the terminal
    // end counts as unconnected
    let via_at: HashSet<(i32, P)> = items
        .iter()
        .filter_map(|it| if let Item::Via(v) = it { Some((v.net, (v.at.x, v.at.y))) } else { None })
        .chain(model.vias.iter().map(|v| (v.net, (v.at.x, -v.at.y))))
        .collect();
    loop {
        let mut ends: HashMap<(i32, u32, P), usize> = HashMap::new();
        for (i, it) in items.iter().enumerate() {
            if !keep[i] {
                continue;
            }
            if let Item::Track(t) = it {
                let (a, b) = match t.prim {
                    TrackPrim::Seg { a, b } => (a, b),
                    TrackPrim::Arc { start, end, .. } => (start, end),
                };
                for p in [p_of(a), p_of(b)] {
                    *ends.entry((t.net, t.layer, p)).or_insert(0) += 1;
                }
            }
        }
        for k in &model.kept {
            let (a, b) = match k.prim {
                TrackPrim::Seg { a, b } => (a, b),
                TrackPrim::Arc { start, end, .. } => (start, end),
            };
            for p in [a, b] {
                *ends.entry((k.net, k.layer, (i64::from(p.x), -i64::from(p.y)))).or_insert(0) += 1;
            }
        }
        let mut dropped = 0;
        for (i, it) in items.iter().enumerate() {
            if !keep[i] {
                continue;
            }
            let (Some((a, b)), Item::Track(t)) = (seg_of(it), it) else { continue };
            let Some(pads) = pads_of.get(&t.net) else { continue };
            let half = 0.5 * t.width as f64;
            let err = model.rules.max_error as f64;
            let free = |q: P| ends.get(&(t.net, t.layer, q)).copied().unwrap_or(0) <= 1 && !via_at.contains(&(t.net, q));
            for &pi in pads {
                if !pad_on(pi, t.layer) {
                    continue;
                }
                let sk = pad_sk(pi, t.layer);
                let (da, db) = (depth(&sk, math(a)), depth(&sk, math(b)));
                // KiCad's `deleteTracksInPads`: both ends in the pad and the track's polygon in it
                let inside = da >= 0.0 && db >= 0.0 && crate::geom::kicad::track_inside(math(a), math(b), t.width as f64, err, |q| depth(&sk, q), PAD_SLACK_NM);
                let redundant = da >= -half && db >= -half && {
                    let c = model.pads[pi].center;
                    let d = |q: P| ((q.0 - c.x) as f64).hypot((q.1 + c.y) as f64);
                    match (free(a), free(b)) {
                        (true, false) => d(b) <= d(a),
                        (false, true) => d(a) <= d(b),
                        _ => false,
                    }
                };
                if inside || redundant {
                    if std::env::var_os("TOPO_PADSNAPDBG").is_some() {
                        // pad-snap r2 debug: which run left a stub in its pad (wire id: OCTI / OCTIFORCEDBG lines)
                        let c = model.pads[pi].center;
                        eprintln!("PADSTUB wire {:?} net {} L{} w {} ({:.4},{:.4})-({:.4},{:.4}) pad ({:.4},{:.4}) {}", t.wire, t.net, t.layer, t.width, a.0 as f64 / 1e6, a.1 as f64 / 1e6, b.0 as f64 / 1e6, b.1 as f64 / 1e6, c.x as f64 / 1e6, -c.y as f64 / 1e6, if inside { "inside" } else { "redundant" });
                    }
                    keep[i] = false;
                    st.in_pad += 1;
                    dropped += 1;
                    break;
                }
            }
        }
        if dropped == 0 {
            break;
        }
    }
    let mut pairs: Vec<(Item, usize)> = std::mem::take(items).into_iter().zip(std::mem::take(conn)).zip(keep).filter_map(|(x, k)| k.then_some(x)).collect();

    // 3. T junctions: a free track end on the middle of a same-net segment becomes a shared vertex
    let in_own_pad = |net: i32, layer: u32, q: P, pad_sk: &mut dyn FnMut(usize, u32) -> Skeleton| -> bool {
        pads_of.get(&net).is_some_and(|ps| ps.iter().any(|&pi| pad_on(pi, layer) && depth(&pad_sk(pi, layer), math(q)) >= 0.0))
    };
    let mut groups: BTreeMap<(i32, u32), Vec<usize>> = BTreeMap::new();
    for (i, (it, _)) in pairs.iter().enumerate() {
        if let Item::Track(t) = it {
            groups.entry((t.net, t.layer)).or_default().push(i);
        }
    }
    let mut via_pts: HashMap<i32, Vec<(P, Vec<u32>)>> = HashMap::new();
    for (it, _) in &pairs {
        if let Item::Via(v) = it {
            via_pts.entry(v.net).or_default().push(((v.at.x, v.at.y), v.layers.clone()));
        }
    }
    for v in &model.vias {
        via_pts.entry(v.net).or_default().push(((v.at.x, -v.at.y), v.layers.clone()));
    }
    for (&(net, layer), idx) in &groups {
        let mut idx: Vec<usize> = idx.clone();
        // ends of every track of the group, arcs and kept copper included
        let mut ends: HashMap<P, usize> = HashMap::new();
        let end_pts = |it: &Item| -> Vec<P> {
            match it {
                Item::Track(TrackItem { prim: TrackPrim::Seg { a, b }, .. }) => vec![p_of(*a), p_of(*b)],
                Item::Track(TrackItem { prim: TrackPrim::Arc { start, end, .. }, .. }) => vec![p_of(*start), p_of(*end)],
                Item::Via(_) => Vec::new(),
            }
        };
        for &i in &idx {
            for p in end_pts(&pairs[i].0) {
                *ends.entry(p).or_insert(0) += 1;
            }
        }
        for k in model.kept.iter().filter(|k| k.net == net && k.layer == layer) {
            let (a, b) = match k.prim {
                TrackPrim::Seg { a, b } => (a, b),
                TrackPrim::Arc { start, end, .. } => (start, end),
            };
            for p in [a, b] {
                *ends.entry((i64::from(p.x), -i64::from(p.y))).or_insert(0) += 1;
            }
        }
        let vias: Vec<P> = via_pts.get(&net).map(|v| v.iter().filter(|(_, ls)| via_on(ls, layer, &cu)).map(|(p, _)| *p).collect()).unwrap_or_default();
        let mut k = 0;
        while k < idx.len() {
            let i = idx[k];
            k += 1;
            for end in 0..2 {
                let Some((a, b)) = seg_of(&pairs[i].0) else { break };
                let e = if end == 0 { a } else { b };
                if ends.get(&e).copied().unwrap_or(0) > 1 || vias.contains(&e) || in_own_pad(net, layer, e, &mut pad_sk) {
                    continue;
                }
                let wi = match &pairs[i].0 {
                    Item::Track(t) => t.width as f64,
                    _ => 0.0,
                };
                // nearest same-net segment interior within reach
                let mut best: Option<(f64, usize, Vec2)> = None;
                for &j in &idx {
                    if j == i {
                        continue;
                    }
                    // a neighbour sharing an end with this segment is no trunk to join (its foot
                    // would be that shared end)
                    if let (Some((ia, ib)), Some((ja, jb))) = (seg_of(&pairs[i].0), seg_of(&pairs[j].0)) {
                        if ia == ja || ia == jb || ib == ja || ib == jb {
                            continue;
                        }
                    }
                    let (Some((sa, sb)), Item::Track(tj)) = (seg_of(&pairs[j].0), &pairs[j].0) else { continue };
                    let (fa, fb, fe) = (Vec2 { x: sa.0 as f64, y: sa.1 as f64 }, Vec2 { x: sb.0 as f64, y: sb.1 as f64 }, Vec2 { x: e.0 as f64, y: e.1 as f64 });
                    let d = fb - fa;
                    let l2 = d.len2();
                    if l2 == 0.0 {
                        continue;
                    }
                    let t = ((fe - fa).dot(d) / l2).clamp(0.0, 1.0);
                    let q = fa + d * t;
                    let dist = (fe - q).len();
                    if dist <= 0.5 * (tj.width as f64).max(wi) && best.as_ref().is_none_or(|b| dist < b.0) {
                        best = Some((dist, j, q));
                    }
                }
                let Some((_, j, q)) = best else { continue };
                let (sa, sb) = seg_of(&pairs[j].0).expect("segment");
                let qp: P = (q.x.round() as i64, q.y.round() as i64);
                if qp == sa || qp == sb {
                    // the foot is the trunk's own end: join there
                    let target = if qp == sa { sa } else { sb };
                    let (na, nb) = if end == 0 { (target, b) } else { (a, target) };
                    if na == nb {
                        continue;
                    }
                    set_seg(&mut pairs[i].0, na, nb);
                    *ends.entry(e).or_insert(1) -= 1;
                    *ends.entry(target).or_insert(0) += 1;
                    st.t_moved += 1;
                    continue;
                }
                if qp != e {
                    st.t_moved += 1;
                }
                let (na, nb) = if end == 0 { (qp, b) } else { (a, qp) };
                if na == nb {
                    continue;
                }
                set_seg(&mut pairs[i].0, na, nb);
                *ends.entry(e).or_insert(1) -= 1;
                // split the trunk at the foot
                let (mut second, c) = (pairs[j].0.clone(), pairs[j].1);
                set_seg(&mut pairs[j].0, sa, qp);
                set_seg(&mut second, qp, sb);
                *ends.entry(qp).or_insert(0) += 3;
                // the new half takes part in later searches of this group
                pairs.push((second, c));
                idx.push(pairs.len() - 1);
                st.t_split += 1;
            }
        }
    }

    // 4a. a new segment that continues a kept (hand) track straight on, e.g. the lead of a
    // continued dangling end (exist-term): KiCad's cleaner merges the two
    // (`mergeCollinearSegments`: same width, `ApproxCollinear`, the shared end touching nothing
    // else, neither locked), so the output does it already: the new segment takes the kept
    // track's far end and the kept track is listed in `replaced_kept` (the caller removes it in
    // the same undo step). Before step 4, so a lead merged here joins its collinear run there.
    // Straight unlocked kept tracks only; one merge per kept track (a second continuation at its
    // other end meets the merged segment, a new-new joint for step 4).
    let mut gone: HashSet<usize> = HashSet::new();
    {
        let kseg = |k: &crate::model::KeptItem| -> Option<(P, P)> {
            match k.prim {
                TrackPrim::Seg { a, b } => Some(((i64::from(a.x), -i64::from(a.y)), (i64::from(b.x), -i64::from(b.y)))),
                TrackPrim::Arc { .. } => None,
            }
        };
        let kends = |k: &crate::model::KeptItem| -> [P; 2] {
            let (a, b) = match k.prim {
                TrackPrim::Seg { a, b } => (a, b),
                TrackPrim::Arc { start, end, .. } => (start, end),
            };
            [(i64::from(a.x), -i64::from(a.y)), (i64::from(b.x), -i64::from(b.y))]
        };
        // does copper other than new item `i` and kept track `ki` of (net, layer) touch point `j`
        // (an end there, or its body within half its width: KiCad's connectivity at the joint)?
        let touched = |pairs: &[(Item, usize)], gone: &HashSet<usize>, net: i32, layer: u32, j: P, i: usize, ki: usize| -> bool {
            let jp = Vec2 { x: j.0 as f64, y: j.1 as f64 };
            let near = |a: P, b: P, w: i64| {
                let (fa, fb) = (Vec2 { x: a.0 as f64, y: a.1 as f64 }, Vec2 { x: b.0 as f64, y: b.1 as f64 });
                crate::topo::draw::seg_seg_dist(jp, jp, fa, fb) <= 0.5 * w as f64
            };
            pairs.iter().enumerate().any(|(x, (it, _))| {
                x != i && matches!(it, Item::Track(t) if t.net == net && t.layer == layer && match t.prim {
                    TrackPrim::Seg { a, b } => near(p_of(a), p_of(b), t.width),
                    TrackPrim::Arc { start, end, .. } => p_of(start) == j || p_of(end) == j,
                })
            }) || model.kept.iter().enumerate().any(|(x, k)| {
                x != ki && !gone.contains(&x) && k.net == net && k.layer == layer && match kseg(k) {
                    Some((a, b)) => near(a, b, k.width),
                    None => kends(k).contains(&j),
                }
            })
        };
        for (ki, k) in model.kept.iter().enumerate() {
            let Some((ka, kb)) = kseg(k) else { continue };
            if k.locked || ka == kb {
                continue;
            }
            for (j, kfar) in [(ka, kb), (kb, ka)] {
                // the one new segment of the net, layer and width that ends at `j`
                let at_j: Vec<usize> = pairs
                    .iter()
                    .enumerate()
                    .filter(|(_, (it, _))| matches!(it, Item::Track(t) if t.net == k.net && t.layer == k.layer))
                    .filter(|(_, (it, _))| match it {
                        Item::Track(TrackItem { prim: TrackPrim::Seg { a, b }, .. }) => p_of(*a) == j || p_of(*b) == j,
                        Item::Track(TrackItem { prim: TrackPrim::Arc { start, end, .. }, .. }) => p_of(*start) == j || p_of(*end) == j,
                        _ => false,
                    })
                    .map(|(i, _)| i)
                    .collect();
                let [i] = at_j[..] else { continue };
                let (Item::Track(t), Some(si)) = (&pairs[i].0, seg_of(&pairs[i].0)) else { continue };
                if t.width != k.width || si.0 == si.1 {
                    continue;
                }
                if via_pts.get(&k.net).is_some_and(|vs| vs.iter().any(|(p, ls)| *p == j && via_on(ls, k.layer, &cu))) || in_own_pad(k.net, k.layer, j, &mut pad_sk) {
                    continue;
                }
                if !approx_collinear(si, (ka, kb)) || touched(&pairs, &gone, k.net, k.layer, j, i, ki) {
                    continue;
                }
                let ifar = if si.0 == j { si.1 } else { si.0 };
                let (d1, d2) = ((kfar.0 - j.0, kfar.1 - j.1), (ifar.0 - j.0, ifar.1 - j.1));
                if i128::from(d1.0) * i128::from(d2.0) + i128::from(d1.1) * i128::from(d2.1) >= 0 {
                    continue; // a fold-back is no continuation
                }
                set_seg(&mut pairs[i].0, kfar, ifar);
                gone.insert(ki);
                st.replaced_kept.push(k.uuid.clone());
                break;
            }
        }
    }

    // 4. collinear consecutive segments: merge chains through joints that touch nothing else
    loop {
        let mut joint: HashMap<(i32, u32, P), Vec<usize>> = HashMap::new();
        let mut alive = vec![true; pairs.len()];
        for (i, (it, _)) in pairs.iter().enumerate() {
            if let Item::Track(t) = it {
                let (a, b) = match t.prim {
                    TrackPrim::Seg { a, b } => (a, b),
                    TrackPrim::Arc { start, end, .. } => (start, end),
                };
                for p in [p_of(a), p_of(b)] {
                    joint.entry((t.net, t.layer, p)).or_default().push(i);
                }
            }
        }
        let mut merged = 0;
        let mut dead = vec![false; pairs.len()];
        let mut keys: Vec<(i32, u32, P)> = joint.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            let (net, layer, j) = key;
            let v = &joint[&key];
            if v.len() != 2 || v[0] == v[1] || !alive[v[0]] || !alive[v[1]] {
                continue;
            }
            let (i1, i2) = (v[0], v[1]);
            let (Item::Track(t1), Item::Track(t2)) = (&pairs[i1].0, &pairs[i2].0) else { continue };
            if t1.width != t2.width {
                continue;
            }
            let (Some(s1), Some(s2)) = (seg_of(&pairs[i1].0), seg_of(&pairs[i2].0)) else { continue };
            if via_pts.get(&net).is_some_and(|vs| vs.iter().any(|(p, ls)| *p == j && via_on(ls, layer, &cu))) {
                continue;
            }
            if model.kept.iter().enumerate().any(|(ki, k)| {
                !gone.contains(&ki) && k.net == net && k.layer == layer && {
                    let (a, b) = match k.prim {
                        TrackPrim::Seg { a, b } => (a, b),
                        TrackPrim::Arc { start, end, .. } => (start, end),
                    };
                    [a, b].iter().any(|p| (i64::from(p.x), -i64::from(p.y)) == j)
                }
            }) {
                continue;
            }
            if in_own_pad(net, layer, j, &mut pad_sk) || !approx_collinear(s1, s2) {
                continue;
            }
            let f1 = if s1.0 == j { s1.1 } else { s1.0 };
            let f2 = if s2.0 == j { s2.1 } else { s2.0 };
            // a fold-back (both far ends on one side of the joint) is no continuation
            let (d1, d2) = ((f1.0 - j.0, f1.1 - j.1), (f2.0 - j.0, f2.1 - j.1));
            if i128::from(d1.0) * i128::from(d2.0) + i128::from(d1.1) * i128::from(d2.1) >= 0 {
                continue;
            }
            // keep the first segment's direction
            let (na, nb) = if s1.0 == j { (f2, f1) } else { (f1, f2) };
            set_seg(&mut pairs[i1].0, na, nb);
            alive[i2] = false;
            // the merged segment's far end entries now point at i1; stop using this pass's map near it
            alive[i1] = false;
            dead[i2] = true;
            merged += 1;
        }
        if merged == 0 {
            break;
        }
        st.merged += merged;
        pairs = pairs.into_iter().zip(dead).filter_map(|(x, d)| (!d).then_some(x)).collect();
    }

    // 5. a moved end or a split can make a segment null or equal to another one: drop those too
    let mut seen: HashSet<(i32, u32, i64, P, P)> = HashSet::new();
    pairs.retain(|(it, _)| {
        let (Some((a, b)), Item::Track(t)) = (seg_of(it), it) else { return true };
        if a == b {
            st.null += 1;
            return false;
        }
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        if seen.insert((t.net, t.layer, t.width, lo, hi)) {
            true
        } else {
            st.duplicate += 1;
            false
        }
    });

    let (it, cn): (Vec<Item>, Vec<usize>) = pairs.into_iter().unzip();
    *items = it;
    *conn = cn;
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(net: i32, a: P, b: P) -> Item {
        Item::Track(TrackItem { net, layer: 0, width: 250_000, prim: TrackPrim::Seg { a: iv(a), b: iv(b) }, group: None, wire: None })
    }

    fn segs(items: &[Item]) -> Vec<(P, P)> {
        let mut v: Vec<(P, P)> = items.iter().filter_map(seg_of).collect();
        v.sort_unstable();
        v
    }

    fn model_with_pad(net: i32, at: P, w: i64, h: i64) -> BoardModel {
        // KiCad frame on the wire, as the plugin sends it
        let j = serde_json::json!({
            "layers": [{"id": 0, "name": "F.Cu"}],
            "pads": [{"net": net, "center": [at.0, at.1], "layers": [0], "shapes": {"0": {"kind": "rect", "w": w, "h": h}}}],
        });
        let mut base = serde_json::to_value(BoardModel::default()).expect("model json");
        for (k, v) in j.as_object().expect("object") {
            base[k] = v.clone();
        }
        BoardModel::from_json(&base.to_string()).expect("model")
    }

    fn run(m: &BoardModel, mut items: Vec<Item>) -> (Vec<Item>, TidyStats) {
        let mut c = vec![0; items.len()];
        let st = tidy(m, &mut items, &mut c);
        assert_eq!(items.len(), c.len());
        (items, st)
    }

    /// pad-snap: a terminal end inside its pad off the centre counts, at the centre it does not, a
    /// joined end (two track ends) or an end on a via does not, nor an end of another net or outside
    /// the pad; the largest distance is reported.
    #[test]
    fn off_centre_pad_ends_counts_terminals_off_the_anchor() {
        let m = model_with_pad(1, (0, 0), 600_000, 1_950_000);
        let at_centre = vec![seg(1, (0, 0), (0, -2_000_000))];
        assert_eq!(off_centre_pad_ends(&m, &at_centre), (0, 0.0));
        let off = vec![seg(1, (0, -525_000), (500_000, -1_100_000))];
        assert_eq!(off_centre_pad_ends(&m, &off), (1, 525_000.0));
        let joined = vec![seg(1, (0, -525_000), (500_000, -1_100_000)), seg(1, (0, -525_000), (0, -2_000_000))];
        assert_eq!(off_centre_pad_ends(&m, &joined), (0, 0.0));
        let via = vec![seg(1, (0, -525_000), (500_000, -1_100_000)), Item::Via(crate::model::ViaItem { kind: "via".into(), net: 1, at: crate::model::Pt { x: 0, y: -525_000 }, d: 600_000, drill: 300_000, layers: vec![0, 2], group: None, wire: None })];
        assert_eq!(off_centre_pad_ends(&m, &via).0, 0);
        let foreign = vec![seg(2, (0, -525_000), (500_000, -1_100_000))];
        assert_eq!(off_centre_pad_ends(&m, &foreign).0, 0);
        let outside = vec![seg(1, (0, -1_200_000), (0, -2_000_000))];
        assert_eq!(off_centre_pad_ends(&m, &outside).0, 0);
    }

    /// draw-via-neck r6: every kind of [`pad_end_breaches`] on one 0.6 x 1.95 mm pad of net 1 at
    /// the origin (KiCad frame), a 0.25 mm track: a trace from the centre (with a bend inside
    /// the pad) is no breach; an end off the centre is a Terminal, two there a Junction, a
    /// segment across the pad with no end in it a PassThrough, an end on the centre trace a Tee
    /// (a hint like the others: KiCad keeps it); an end at a via in the pad, a foreign net and copper
    /// outside the pad are not judged, a segment through the pad to a via outside it is a
    /// PassThrough (r7). A narrow pad next to a pad of another net (the disc at the centre breaks the
    /// clearance) is a blocked centre and the edge band an edge-clear pad: listed, not counted.
    #[test]
    fn pad_end_breaches_classify_every_way_a_track_meets_its_pad() {
        let m = model_with_pad(1, (0, 0), 600_000, 1_950_000);
        let kinds = |items: &[Item]| -> Vec<(PadEndKind, bool, i64)> { pad_end_breaches(&m, items).iter().map(|b| (b.kind, b.hint(), b.off.round() as i64)).collect() };
        assert!(kinds(&[seg(1, (0, 0), (0, -2_000_000))]).is_empty());
        assert!(kinds(&[seg(1, (0, 0), (0, -500_000)), seg(1, (0, -500_000), (500_000, -1_100_000))]).is_empty(), "a bend inside the pad");
        assert_eq!(kinds(&[seg(1, (0, -525_000), (500_000, -1_100_000))]), vec![(PadEndKind::Terminal, true, 525_000)]);
        let j = kinds(&[seg(1, (0, -525_000), (500_000, -1_100_000)), seg(1, (0, -525_000), (-500_000, -1_100_000))]);
        assert_eq!(j, vec![(PadEndKind::Junction, true, 525_000)], "one entry per point (dedup)");
        assert_eq!(kinds(&[seg(1, (-1_000_000, -300_000), (1_000_000, -300_000))]), vec![(PadEndKind::PassThrough, true, 300_000)]);
        assert_eq!(kinds(&[seg(1, (0, 0), (0, -2_000_000)), seg(1, (0, -600_000), (500_000, -1_100_000))]), vec![(PadEndKind::Tee, true, 600_000)]);
        let via = Item::Via(crate::model::ViaItem { kind: "via".into(), net: 1, at: crate::model::Pt { x: 0, y: -525_000 }, d: 600_000, drill: 300_000, layers: vec![0, 2], group: None, wire: None });
        assert!(kinds(&[seg(1, (0, -525_000), (500_000, -1_100_000)), via.clone()]).is_empty());
        // r7 (verifier r6 F3): a via outside the pad does not hide a segment across the pad
        let via_out = |x: i64| Item::Via(crate::model::ViaItem { kind: "via".into(), net: 1, at: crate::model::Pt { x, y: -300_000 }, d: 600_000, drill: 300_000, layers: vec![0, 2], group: None, wire: None });
        assert_eq!(kinds(&[seg(1, (-2_000_000, -300_000), (2_000_000, -300_000)), via_out(2_000_000)]), vec![(PadEndKind::PassThrough, true, 300_000)]);
        assert!(kinds(&[seg(1, (0, -300_000), (2_000_000, -300_000)), via_out(0)]).is_empty(), "the via in the pad holds it");
        assert!(kinds(&[seg(2, (0, -525_000), (500_000, -1_100_000))]).is_empty());
        assert!(kinds(&[seg(1, (0, -1_200_000), (0, -2_000_000))]).is_empty());
        // Blocked centre: a 0.2 mm pad of net 1 and one of net 2 0.25 mm beside it, clearance
        // 0.24 mm: the 0.25 mm disc at the centre comes 0.225 mm near the other pad.
        let pads = |gap: i64| {
            let j = serde_json::json!({
                "layers": [{"id": 0, "name": "F.Cu"}],
                "pads": [{"net": 1, "center": [0, 0], "layers": [0], "shapes": {"0": {"kind": "rect", "w": 200_000, "h": 1_000_000}}},
                         {"net": 2, "center": [200_000 + gap, 0], "layers": [0], "shapes": {"0": {"kind": "rect", "w": 200_000, "h": 1_000_000}}}],
            });
            let mut base = serde_json::to_value(BoardModel::default()).expect("model json");
            for (k, v) in j.as_object().expect("object") {
                base[k] = v.clone();
            }
            let mut m = BoardModel::from_json(&base.to_string()).expect("model");
            m.rules.min_clearance = 240_000;
            m
        };
        let off = [seg(1, (0, -60_000), (0, -2_000_000))];
        let b = pad_end_breaches(&pads(250_000), &off);
        assert_eq!(b.iter().map(|b| (b.kind, b.centre_blocked, b.hint())).collect::<Vec<_>>(), vec![(PadEndKind::Terminal, true, false)]);
        let b = pad_end_breaches(&pads(400_000), &off);
        assert_eq!(b.iter().map(|b| (b.kind, b.centre_blocked, b.hint())).collect::<Vec<_>>(), vec![(PadEndKind::Terminal, false, true)], "room at the centre");
        // Edge-clear: the pad of `edge_model` within the edge clearance (its centre 1.26 mm from the edge).
        let em = edge_model(1_200_000, false);
        let b = pad_end_breaches(&em, &[Item::Track(TrackItem { net: 1, layer: 0, width: 250_000, prim: TrackPrim::Seg { a: iv((2_500_000, 0)), b: iv((-1_000_000, 0)) }, group: None, wire: None })]);
        assert_eq!(b.iter().map(|b| (b.kind, b.edge, b.hint())).collect::<Vec<_>>(), vec![(PadEndKind::Terminal, true, false)]);
        let em = edge_model(500_000, false);
        let b = pad_end_breaches(&em, &[Item::Track(TrackItem { net: 1, layer: 0, width: 250_000, prim: TrackPrim::Seg { a: iv((2_500_000, 0)), b: iv((-1_000_000, 0)) }, group: None, wire: None })]);
        assert_eq!(b.iter().map(|b| (b.kind, b.edge, b.hint())).collect::<Vec<_>>(), vec![(PadEndKind::Terminal, false, true)], "out of the edge band");
    }

    fn edge_model(edge_clear: i64, ignored: bool) -> BoardModel {
        // round 1.6 mm pad of net 1 at (2.74, 0) mm; board edge the vertical line x = 4 mm
        let j = serde_json::json!({
            "layers": [{"id": 0, "name": "F.Cu"}],
            "pads": [{"net": 1, "center": [2_740_000, 0], "layers": [0], "shapes": {"0": {"kind": "circle", "d": 1_600_000}}}],
            "outline": [{"kind": "seg", "a": [4_000_000, -10_000_000], "b": [4_000_000, 10_000_000]}],
        });
        let mut base = serde_json::to_value(BoardModel::default()).expect("model json");
        for (k, v) in j.as_object().expect("object") {
            base[k] = v.clone();
        }
        let mut m = BoardModel::from_json(&base.to_string()).expect("model");
        m.rules.edge_clearance = edge_clear;
        if ignored {
            m.rules.ignored_checks.push("copper_edge_clearance".into());
        }
        m
    }

    #[test]
    fn pad_end_in_the_edge_band_moves_inside_the_pad_to_the_clearance() {
        // eprom-edge: 1.5 mm edge clearance, the pad centre 1.26 mm from the edge
        let m = edge_model(1_500_000, false);
        let mut items = vec![seg(1, (2_740_000, 0), (0, 0))];
        let cut = pad_ends_off_edge(&m, &mut items);
        assert_eq!(cut.len(), 1);
        let (a, b) = seg_of(&items[0]).expect("seg");
        assert_eq!(b, (0, 0), "the far end stays");
        // edge 4 mm - 1.5 mm - 0.125 mm half width - margin; inside the pad (left rim 1.94 mm)
        assert!(a.0 <= 2_375_000 - 2_000 && a.0 > 2_300_000 && a.1 == 0, "{a:?}");
    }

    #[test]
    fn pad_end_stays_when_the_edge_is_kept_ignored_or_a_via_holds_it() {
        // already clear (0.8 mm rule), the check ignored, or the band reaching past the pad
        for (ec, ign) in [(800_000, false), (1_500_000, true), (2_700_000, false)] {
            let m = edge_model(ec, ign);
            let mut items = vec![seg(1, (2_740_000, 0), (0, 0))];
            assert!(pad_ends_off_edge(&m, &mut items).is_empty(), "{ec} {ign}");
            assert_eq!(seg_of(&items[0]), Some(((2_740_000, 0), (0, 0))));
        }
        // a via in the pad: the track end is its connection
        let m = edge_model(1_500_000, false);
        let via = Item::Via(crate::model::ViaItem { kind: "via".into(), net: 1, at: crate::model::Pt { x: 2_740_000, y: 0 }, d: 600_000, drill: 300_000, layers: Vec::new(), group: None, wire: None });
        let mut items = vec![seg(1, (2_740_000, 0), (0, 0)), via];
        assert!(pad_ends_off_edge(&m, &mut items).is_empty());
        // another net's track: not its pad
        let mut items = vec![seg(2, (2_740_000, 0), (0, 0))];
        assert!(pad_ends_off_edge(&m, &mut items).is_empty());
    }

    #[test]
    fn null_and_duplicate_segments_are_dropped() {
        let m = BoardModel::default();
        let (out, st) = run(&m, vec![seg(1, (0, 0), (1_000_000, 0)), seg(1, (1_000_000, 0), (0, 0)), seg(1, (5, 5), (5, 5))]);
        assert_eq!((st.null, st.duplicate), (1, 1));
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn segment_inside_its_pad_is_dropped_and_the_exit_kept() {
        // pad 1.6 x 0.6 mm at the origin; the stub centre -> inside, then out of the pad
        let m = model_with_pad(1, (0, 0), 1_600_000, 600_000);
        let (out, st) = run(&m, vec![seg(1, (0, 0), (400_000, 0)), seg(1, (400_000, 0), (3_000_000, 0)), seg(2, (0, 0), (100_000, 0))]);
        assert_eq!(st.in_pad, 1, "only the net-1 segment inside the net-1 pad");
        assert_eq!(segs(&out), vec![((0, 0), (100_000, 0)), ((400_000, 0), (3_000_000, 0))]);
    }

    /// draw-via-neck r7 (verifier vfy-draw-via-neck-r6 F1, qmtech one cycle ADV7511KSTZ1.29): the
    /// 0.1407 mm 45-degree jog from the centre of a 0.3 x 1.6 mm pad to 65.011 um from its long
    /// side. Its true round end sticks out 5.34 um, so r6's tolerance (5 um) kept it; KiCad's
    /// polygon of it lies inside the pad and KiCad's Cleanup deleted it (track_in_pad): so does
    /// the tidy now. 1 um further out KiCad keeps it, and so does the tidy.
    #[test]
    fn a_jog_inside_its_pad_by_kicads_polygon_is_dropped() {
        let m = model_with_pad(1, (0, 0), 300_000, 1_600_000);
        let jog = |x: i64| {
            let mut s = [seg(1, (0, 0), (-x, x)), seg(1, (-x, x), (-x, 1_000_000))];
            for it in s.iter_mut() {
                if let Item::Track(t) = it {
                    t.width = 140_700;
                }
            }
            s.to_vec()
        };
        let (out, st) = run(&m, jog(84_989));
        assert_eq!(st.in_pad, 1, "KiCad deletes the jog");
        assert_eq!(segs(&out), vec![((-84_989, 84_989), (-84_989, 1_000_000))]);
        let (out, st) = run(&m, jog(86_000));
        assert_eq!(st.in_pad, 0, "its polygon sticks out: KiCad keeps it");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn t_on_a_trunk_middle_splits_the_trunk_at_an_exact_vertex() {
        // qmtech ADV_DVDD: a 43 µm segment ends on the middle of a vertical trunk
        let m = BoardModel::default();
        let (out, st) = run(
            &m,
            vec![seg(1, (137_499_642, 84_030_608), (126_957_608, 84_030_608)), seg(1, (126_957_608, 84_030_608), (126_927_000, 84_000_000)), seg(1, (126_927_000, 84_608_107), (126_927_000, 74_369_074))],
        );
        assert_eq!(st.t_split, 1);
        let s = segs(&out);
        assert!(s.contains(&((126_927_000, 84_608_107), (126_927_000, 84_000_000))) || s.contains(&((126_927_000, 74_369_074), (126_927_000, 84_000_000))) || s.iter().filter(|(a, b)| *a == (126_927_000, 84_000_000) || *b == (126_927_000, 84_000_000)).count() == 3);
        assert_eq!(s.iter().filter(|(a, b)| *a == (126_927_000, 84_000_000) || *b == (126_927_000, 84_000_000)).count(), 3, "{s:?}");
    }

    #[test]
    fn piece_from_the_pad_anchor_to_its_edge_stays() {
        // fine-pitch exit: centre -> tip of a 1.6 x 0.3 mm pad, then out; the terminal (centre) is
        // the end nearer to the anchor, so KiCad counts the pad for it: stable, kept
        let m = model_with_pad(1, (11_000_000, 10_000_000), 1_600_000, 300_000);
        let (out, st) = run(&m, vec![seg(1, (11_000_000, 10_000_000), (10_200_000, 10_000_000)), seg(1, (10_200_000, 10_000_000), (8_000_000, 12_200_000))]);
        assert_eq!(st.in_pad, 0);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn short_segment_with_both_ends_in_a_pad_is_dropped() {
        // qmtech ADV_TX2- at USB1.3 (0.3 x 2.6 mm): 86 µm, its capsule pokes out of the pad's side
        let m = model_with_pad(1, (131_227_000, 55_100_000), 300_000, 2_600_000);
        let w = |a: P, b: P| Item::Track(TrackItem { net: 1, layer: 0, width: 200_000, prim: TrackPrim::Seg { a: iv(a), b: iv(b) }, group: None, wire: None });
        let (out, st) = run(&m, vec![w((130_309_862, 55_539_471), (131_166_471, 55_539_471)), w((131_166_471, 55_539_471), (131_227_000, 55_600_000))]);
        assert_eq!(st.in_pad, 1);
        assert_eq!(segs(&out), vec![((130_309_862, 55_539_471), (131_166_471, 55_539_471))]);
    }

    #[test]
    fn t_end_next_to_its_own_neighbour_joins_the_trunk() {
        // a 2-layer badge, VCC: a 0.22 mm stub (0.5 mm wide) ends 0.243 mm off a vertical trunk; its own
        // neighbour's shared end is nearer, but is no trunk
        let m = BoardModel::default();
        let w = |a: P, b: P| Item::Track(TrackItem { net: 1, layer: 0, width: 500_000, prim: TrackPrim::Seg { a: iv(a), b: iv(b) }, group: None, wire: None });
        let (out, st) = run(
            &m,
            vec![w((120_848_259, 122_648_259), (120_848_259, 107_301_741)), w((120_937_500, 108_062_500), (119_937_500, 108_062_500)), w((120_937_500, 108_062_500), (121_091_091, 108_216_091))],
        );
        assert_eq!((st.t_split, st.t_moved), (1, 1));
        let foot = (120_848_259, 108_216_091);
        assert_eq!(segs(&out).iter().filter(|(a, b)| *a == foot || *b == foot).count(), 3, "{:?}", segs(&out));
    }

    #[test]
    fn collinear_chain_is_merged_but_not_through_a_t_or_via() {
        let m = BoardModel::default();
        let (out, st) = run(&m, vec![seg(1, (0, 0), (1_000_000, 0)), seg(1, (1_000_000, 0), (2_000_000, 0)), seg(1, (2_000_000, 0), (3_000_000, 0))]);
        assert_eq!(st.merged, 2);
        assert_eq!(segs(&out), vec![((0, 0), (3_000_000, 0))]);
        // a T at the joint keeps it
        let (out, st) = run(&m, vec![seg(1, (0, 0), (1_000_000, 0)), seg(1, (1_000_000, 0), (2_000_000, 0)), seg(1, (1_000_000, 0), (1_000_000, 1_000_000))]);
        assert_eq!(st.merged, 0);
        assert_eq!(out.len(), 3);
        // a bend is no merge
        let (_, st) = run(&m, vec![seg(1, (0, 0), (1_000_000, 0)), seg(1, (1_000_000, 0), (2_000_000, 1_000_000))]);
        assert_eq!(st.merged, 0);
    }

    /// `m` with kept tracks `(uuid, KiCad-frame a, b, width, locked)` of net 1 on layer 0.
    fn with_kept(mut m: BoardModel, kept: &[(&str, P, P, i64, bool)]) -> BoardModel {
        let k = |p: P| IVec2 { x: p.0 as i32, y: -p.1 as i32 };
        m.kept = kept.iter().map(|&(u, a, b, w, locked)| crate::model::KeptItem { uuid: u.into(), net: 1, layer: 0, width: w, prim: TrackPrim::Seg { a: k(a), b: k(b) }, locked }).collect();
        m
    }

    #[test]
    fn a_lead_that_continues_a_kept_track_replaces_it() {
        // exist-term r2 (verifier F5: KiCad's Cleanup "merge co-linear tracks" found 18 leads
        // continuing their hand tracks): the new segment takes the kept track's far end and the
        // kept track is listed for removal; a collinear run after the lead joins it in step 4.
        let w = 250_000;
        let m = with_kept(BoardModel::default(), &[("k", (0, 0), (1_000_000, 0), w, false)]);
        let (out, st) = run(&m, vec![seg(1, (1_000_000, 0), (1_450_000, 0)), seg(1, (1_450_000, 0), (3_000_000, 0)), seg(1, (3_000_000, 0), (3_000_000, 1_000_000))]);
        assert_eq!(st.replaced_kept, vec!["k".to_string()]);
        assert_eq!(st.merged, 1, "the run after the lead");
        assert_eq!(segs(&out), vec![((0, 0), (3_000_000, 0)), ((3_000_000, 0), (3_000_000, 1_000_000))]);
        // a vertical kept track (KiCad frame y down), the lead going on down
        let m = with_kept(BoardModel::default(), &[("v", (0, 0), (0, 1_000_000), w, false)]);
        let (out, st) = run(&m, vec![seg(1, (0, 1_000_000), (0, 1_200_000))]);
        assert_eq!(st.replaced_kept, vec!["v".to_string()]);
        assert_eq!(segs(&out), vec![((0, 0), (0, 1_200_000))]);
        // continued at both ends: one segment over all three, one replacement
        let m = with_kept(BoardModel::default(), &[("k", (0, 0), (1_000_000, 0), w, false)]);
        let (out, st) = run(&m, vec![seg(1, (1_000_000, 0), (1_400_000, 0)), seg(1, (-300_000, 0), (0, 0))]);
        assert_eq!(st.replaced_kept, vec!["k".to_string()]);
        assert_eq!(segs(&out), vec![((-300_000, 0), (1_400_000, 0))]);
    }

    #[test]
    fn a_kept_track_stays_where_kicad_would_not_merge_it() {
        // every condition of mergeCollinearSegments that keeps the two apart keeps the kept track
        let w = 250_000;
        let lead = || seg(1, (1_000_000, 0), (1_400_000, 0));
        let kept = |locked: bool, width: i64| with_kept(BoardModel::default(), &[("k", (0, 0), (1_000_000, 0), width, locked)]);
        let none = |m: &BoardModel, items: Vec<Item>, why: &str| {
            let (_, st) = run(m, items);
            assert!(st.replaced_kept.is_empty(), "{why}: {st:?}");
        };
        none(&kept(true, w), vec![lead()], "locked");
        none(&kept(false, 200_000), vec![lead()], "another width");
        none(&kept(false, w), vec![seg(1, (1_000_000, 0), (1_400_000, 300_000))], "a bend");
        none(&kept(false, w), vec![seg(1, (1_000_000, 0), (600_000, 0))], "a fold-back");
        none(&kept(false, w), vec![lead(), seg(1, (1_000_000, 0), (1_000_000, 900_000))], "a third track end at the joint");
        none(&kept(false, w), vec![lead(), seg(1, (1_000_000, -500_000), (1_000_000, 500_000))], "a track body through the joint");
        let mut m = kept(false, w);
        m.kept.push(crate::model::KeptItem { uuid: "k2".into(), net: 1, layer: 0, width: w, prim: TrackPrim::Seg { a: IVec2 { x: 1_000_000, y: 0 }, b: IVec2 { x: 1_000_000, y: -700_000 } }, locked: false });
        none(&m, vec![lead()], "another kept track at the joint");
        let mut m = kept(false, w);
        m.vias.push(crate::model::Via { uuid: "v".into(), net: 1, at: crate::model::Pt { x: 1_000_000, y: 0 }, d: 600_000, drill: 300_000, layers: vec![0, 2], free: false, locked: false });
        none(&m, vec![lead()], "a via at the joint");
        let mut other = seg(1, (1_000_000, 0), (1_400_000, 0));
        if let Item::Track(t) = &mut other {
            t.net = 2;
        }
        none(&kept(false, w), vec![other], "another net");
        let mut arc = kept(false, w);
        arc.kept[0].prim = TrackPrim::Arc { start: IVec2 { x: 0, y: 0 }, mid: IVec2 { x: 500_000, y: 100_000 }, end: IVec2 { x: 1_000_000, y: 0 } };
        none(&arc, vec![lead()], "a kept arc");
        let pad = with_kept(model_with_pad(1, (1_000_000, 0), 600_000, 600_000), &[("k", (0, 0), (1_000_000, 0), w, false)]);
        // a lead that leaves the pad's reach (a shorter one is dropped by step 2 before 4a sees it)
        let (out, st) = run(&pad, vec![seg(1, (1_000_000, 0), (2_000_000, 0))]);
        assert!(st.replaced_kept.is_empty(), "the joint in an own pad: {st:?}");
        assert_eq!(segs(&out), vec![((1_000_000, 0), (2_000_000, 0))], "the lead stays as it is");
    }

    #[test]
    fn a_split_that_repeats_a_segment_leaves_one_copy() {
        // a 2-layer badge, I2C_SDA (90 s run): a free end moved onto a trunk whose new half is the same
        // segment as an existing one
        let m = BoardModel::default();
        let (out, st) = run(
            &m,
            vec![seg(1, (0, 0), (2_000_000, 0)), seg(1, (1_000_000, 0), (2_000_000, 0)), seg(1, (1_000_000, 50_000), (1_000_000, 1_000_000))],
        );
        let s = segs(&out);
        let mut d = s.clone();
        d.dedup();
        assert_eq!(s, d, "{st:?}");
    }

    #[test]
    fn approx_collinear_matches_kicad_threshold() {
        assert!(approx_collinear(((0, 0), (10, 0)), ((10, 0), (20, 1))));
        assert!(!approx_collinear(((0, 0), (10, 0)), ((10, 0), (20, 3))));
        assert!(approx_collinear(((0, 0), (1_000_000, 1_000_000)), ((1_000_000, 1_000_000), (2_000_000, 2_000_000))));
    }
}
