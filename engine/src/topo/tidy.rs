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

/// KiCad's `ERROR_INSIDE` polygon approximation of pads and track ends (default board maximum
/// error 5 µm): a segment within this of lying wholly inside a pad is treated as inside, as the
/// cleaner's polygon test may.
const PAD_TOL_NM: f64 = 5_000.0;

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
}

impl TidyStats {
    pub fn changed(&self) -> bool {
        *self != TidyStats::default()
    }

    /// One report line.
    pub fn line(&self) -> String {
        format!(
            "cleanup-safe output (KiCad's Cleanup Tracks & Vias leaves it unchanged): {} null and {} duplicate segments dropped, {} segments a pad of their net makes redundant dropped, {} T junctions made exact vertices ({} track ends moved onto the trunk), {} collinear segments merged",
            self.null, self.duplicate, self.in_pad, self.t_split, self.t_moved, self.merged
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
    let ec = model.rules.edge_clearance as f64;
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
            let free = |q: P| ends.get(&(t.net, t.layer, q)).copied().unwrap_or(0) <= 1 && !via_at.contains(&(t.net, q));
            for &pi in pads {
                if !pad_on(pi, t.layer) {
                    continue;
                }
                let sk = pad_sk(pi, t.layer);
                let (da, db) = (depth(&sk, math(a)), depth(&sk, math(b)));
                let inside = da >= half - PAD_TOL_NM && db >= half - PAD_TOL_NM;
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
            if model.kept.iter().any(|k| {
                k.net == net && k.layer == layer && {
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
        // dcsgonebadge VCC: a 0.22 mm stub (0.5 mm wide) ends 0.243 mm off a vertical trunk; its own
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

    #[test]
    fn a_split_that_repeats_a_segment_leaves_one_copy() {
        // dcsgonebadge I2C_SDA (90 s run): a free end moved onto a trunk whose new half is the same
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
