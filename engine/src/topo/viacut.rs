//! CM1 (cm1-room, docs/dev/design/cm1-room.md): a planned via is a feature of the capacity
//! test, so the plan counts the room a via takes in a narrow place that other wires pass too.
//!
//! **Why.** The per-edge packer packs a via disc only on the two edges its own wire crosses
//! (`pack::wire_discs`), and the cut model counts a via only on a static cut within `r + s` of
//! it; every static cut starts at a feature vertex. A via in the middle of a pad gap is on
//! neither: wiccon (gui27 report, cluster A) planned TXD's 0.8 mm via in the 1.0 mm gap between
//! two ESP-12E pads and RST through the same gap, both models predicted 0, the drawing needs
//! 1.45 mm there.
//!
//! **Model** (Maley 1987: a cut's capacity is its length less half the widths of the islands at
//! its ends, p. 32; a critical cut runs from a feature to the closest point of another feature,
//! p. 40; the sketch is routable iff no critical cut is over-full, pp. 32-33). A planned via at
//! point `V` (the site the drawing tests first) is a feature of every layer. For every obstacle
//! of the layer near the via (those of the triangles that share a vertex with the via's
//! triangle: the CDT neighbourhood, scale-free) with closest point `q` to `V`, the *gap cut*
//! runs from `q` through `V` to the first obstacle of the layer the ray from `V` away from `q`
//! meets (none within the neighbourhood: no cut). It is the union of the two collinear Maley cuts
//! `V q` and `V h`: over-full means no via position on that line fits, so where exactly the via
//! sits on the line cannot raise a false alarm. Its items are the via (width `d`) and every
//! traversal of another net's wire in the neighbourhood whose taut segment crosses the cut,
//! anywhere, through the disc included (a foreign band through a planned via disc counts). The
//! need is [`super::demand::need`] (the packer's pairwise clearances; an own-net pad end may be
//! touched).
//!
//! Complexity: building the geometry of one via is O(obstacles x sides of the neighbourhood +
//! traversals of the neighbourhood x their chords); pricing it is O(cuts x crossers log).

use super::demand::{need, Bound, Item};
use super::pack::Packer;
use super::sheet::Norm;
use super::state::{point_in_poly, Projection, State, TriId, VertKind, WireId};
use crate::geom::Vec2;

/// tw4 T2: a run- or model-wide cache of via geometry per (triangle, net, point x bits, point y
/// bits).
pub type GeoCache = std::sync::Mutex<crate::fxhash::FxHashMap<(usize, i32, u64, u64), std::sync::Arc<ViaGeom>>>;

/// One gap cut through a planned via on one layer.
#[derive(Clone, Debug)]
pub struct GapCut {
    pub layer: usize,
    /// Ends (on the obstacles' copper) and their bounds.
    pub a: Vec2,
    pub b: Vec2,
    pub ba: Bound,
    pub bb: Bound,
    /// Capacity: the length in the planning norm less the ends' sagitta (nm).
    pub len: f64,
    /// Position of the via centre along `a -> b` (0..1).
    pub via_t: f64,
    /// Crossings by other nets' wires: `(wire, traversal, position along a -> b)`.
    pub cross: Vec<(WireId, usize, f64)>,
}

/// The gap cuts of a via at one point (fixed for a State).
#[derive(Clone, Debug, Default)]
pub struct ViaGeom {
    pub at: Vec2,
    pub cuts: Vec<GapCut>,
    /// The chords of other nets' traversals in the neighbourhood: `(wire, traversal, from, to)`
    /// (for the via-to-via cuts, [`pair_over`]).
    pub chords: Vec<(WireId, usize, Vec2, Vec2)>,
    /// The size of the neighbourhood (nm; the diagonal of its bounding box): the gap cuts end
    /// within it.
    pub reach: f64,
    /// The neighbourhood: the triangles that share a vertex with the via's triangle, ascending
    /// (tw4 T2: a via whose triangle is in it is a via-to-via partner; the relation is symmetric,
    /// so both vias of a pair see each other).
    pub tris: Vec<TriId>,
}

/// The over-full amount of a via's gap cuts: per layer the worst `(need - len, pitch)`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Over {
    /// Over-full via-layers (`need > len` on at least one cut of the layer).
    pub count: usize,
    /// Sum over the over-full layers of `1 + over / pitch` (the per-edge price factor).
    pub graded: f64,
}

/// Closest point of polygon `poly` (closed, any orientation) to `p`, and its distance.
pub fn closest_on_poly(p: Vec2, poly: &[Vec2]) -> (Vec2, f64) {
    let n = poly.len();
    let mut best = (poly.first().copied().unwrap_or(p), f64::INFINITY);
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let ab = b - a;
        let l2 = ab.len2();
        let t = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
        let q = a + ab * t;
        let d = (p - q).len();
        if d < best.1 {
            best = (q, d);
        }
    }
    best
}

/// Distance along the unit ray `p + s u` (s > 0) to the first crossing with segment `a b`.
fn ray_seg(p: Vec2, u: Vec2, a: Vec2, b: Vec2) -> Option<f64> {
    let e = b - a;
    let den = u.cross(e);
    if den.abs() < 1e-12 {
        return None;
    }
    let w = a - p;
    let s = w.cross(e) / den;
    let t = w.cross(u) / den;
    (s > 0.0 && (-1e-9..=1.0 + 1e-9).contains(&t)).then_some(s)
}

/// Parameters `(t on a->b, s on c->d)` of the crossing of two segments, `t` in (0, 1) open and
/// `s` in [0, 1) half open (a chord's joint counts once).
pub fn seg_cross(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> Option<(f64, f64)> {
    let r = b - a;
    let q = d - c;
    let den = r.cross(q);
    if den.abs() < 1e-12 {
        return None;
    }
    let w = c - a;
    let t = w.cross(q) / den;
    let s = w.cross(r) / den;
    (t > 0.0 && t < 1.0 && (0.0..1.0).contains(&s)).then_some((t, s))
}

/// What ends a cut: the bound of obstacle copper (`demand::Bound`).
fn bound_of(o: &super::state::Obstacle) -> Bound {
    if o.bare {
        Bound::Hole { clear: o.clear }
    } else {
        Bound::Pad { net: o.net, clear: o.clear }
    }
}

/// The static part of the gap cuts of a via of `net` at `at` in triangle `t` (tw4 T2: a function
/// of the projection and the packer's static board only, so the packer caches it for the run,
/// [`Packer::via_static`]): the cuts with no crossers, no chords. Near-parallel cuts on a layer
/// (within 10 degrees) keep the shorter. Deterministic (obstacle order). O(obstacles x sides of
/// the neighbourhood).
pub fn via_static(pr: &Projection, packer: &Packer, t: TriId, net: i32, at: Vec2, norm: Norm) -> ViaGeom {
    let _ = net;
    // The neighbourhood: every triangle that shares a vertex with `t`.
    let mut tris: Vec<TriId> = pr.tris[t].iter().flat_map(|&v| pr.vert_tris[v].iter().copied()).collect();
    tris.sort_unstable();
    tris.dedup();
    let mut obs: Vec<usize> = tris.iter().flat_map(|&tt| pr.tri_obs[tt].iter().copied()).collect();
    for &tt in &tris {
        for &v in &pr.tris[tt] {
            if let VertKind::Outline(o) | VertKind::Centre(o) = pr.vkind[v] {
                obs.push(o);
            }
        }
    }
    obs.sort_unstable();
    obs.dedup();
    let (mut lo, mut hi) = (at, at);
    for &tt in &tris {
        for &v in &pr.tris[tt] {
            let p = pr.pos[v];
            lo = Vec2 { x: lo.x.min(p.x), y: lo.y.min(p.y) };
            hi = Vec2 { x: hi.x.max(p.x), y: hi.y.max(p.y) };
        }
    }
    let reach = (hi - lo).len();
    let inside = |p: Vec2| p.x >= lo.x - reach && p.x <= hi.x + reach && p.y >= lo.y - reach && p.y <= hi.y + reach;
    let outline: Vec<(Vec2, Vec2)> = packer.outline_segs().iter().copied().filter(|&(a, b)| inside(a) || inside(b)).collect();
    let edge_bound = Bound::Edge { clear: packer.edge_clear() };
    let mut cuts: Vec<GapCut> = Vec::new();
    for l in 0..pr.n_layers {
        let on: Vec<usize> = obs.iter().copied().filter(|&o| pr.obstacles[o].drilled || pr.obstacles[o].layers.has(l)).filter(|&o| !point_in_poly(at, &pr.obstacles[o].poly)).collect();
        let mut mine: Vec<GapCut> = Vec::new();
        for &o in &on {
            let ob = &pr.obstacles[o];
            let (q, dq) = closest_on_poly(at, &ob.poly);
            let Some(u) = (at - q).unit().filter(|_| dq > 0.0) else { continue };
            // The first obstacle (or outline side) of the layer behind the via.
            let mut hit: Option<(f64, Bound, f64)> = None;
            for &o2 in on.iter().filter(|&&o2| o2 != o) {
                let p2 = &pr.obstacles[o2].poly;
                for i in 0..p2.len() {
                    if let Some(s) = ray_seg(at, u, p2[i], p2[(i + 1) % p2.len()]) {
                        if s <= reach && hit.is_none_or(|h| s < h.0) {
                            hit = Some((s, bound_of(&pr.obstacles[o2]), pr.obstacles[o2].sag));
                        }
                    }
                }
            }
            for &(a, b) in &outline {
                if let Some(s) = ray_seg(at, u, a, b) {
                    if s <= reach && hit.is_none_or(|h| s < h.0) {
                        hit = Some((s, edge_bound, 0.0));
                    }
                }
            }
            let Some((s, bb, sag_b)) = hit else { continue };
            let b = at + u * s;
            let len = norm.len(b - q) - ob.sag - sag_b;
            let via_t = dq / (dq + s);
            let cut = GapCut { layer: l, a: q, b, ba: bound_of(ob), bb, len, via_t, cross: Vec::new() };
            // Near-parallel duplicates: keep the shorter.
            match mine.iter_mut().find(|c| ((c.b - c.a).unit().map_or(0.0, |v| v.dot(u))).abs() > (10f64).to_radians().cos()) {
                Some(c) if c.len <= cut.len => {}
                Some(c) => *c = cut,
                None => mine.push(cut),
            }
        }
        cuts.extend(mine);
    }
    ViaGeom { at, cuts, chords: Vec::new(), reach, tris }
}

/// The gap cuts of `s` ([`via_static`] of a via of `net`) with their crossers under the state
/// `st`: the chords of every other net's traversal of the neighbourhood (taut segments,
/// [`Packer::taut`]) and, per cut, the chords that cross it. A function of the state (fixed for a
/// capacity model's life). Deterministic (traversal order). O(traversals of the neighbourhood x
/// their chords x cuts).
pub fn with_crossers(st: &State, packer: &Packer, s: &ViaGeom, net: i32) -> ViaGeom {
    let pr = st.proj;
    let mut chords: Vec<(WireId, usize, Vec2, Vec2)> = Vec::new();
    let mut travs: Vec<(WireId, usize)> = s.tris.iter().flat_map(|&tt| st.tri_trav[tt].iter().copied()).collect();
    travs.sort_unstable();
    travs.dedup();
    for (w, k) in travs {
        let wi = &st.wires[w];
        if !wi.alive || wi.net == net {
            continue;
        }
        let taut = packer.taut(st, w);
        let n = wi.passes.len();
        let from = if k == 0 { pr.pos[wi.a] } else { taut.at[k - 1] };
        let to = if k >= n { pr.pos[wi.b] } else { taut.at[k] };
        let mut pts = vec![from];
        pts.extend(wi.turns.iter().filter(|x| x.0 == k).map(|x| pr.pos[x.1]));
        pts.push(to);
        for q in pts.windows(2) {
            chords.push((w, k, q[0], q[1]));
        }
    }
    let cuts = s
        .cuts
        .iter()
        .map(|c| {
            let cross: Vec<(WireId, usize, f64)> = chords.iter().filter_map(|&(w, k, p, q)| seg_cross(c.a, c.b, p, q).map(|x| (w, k, x.0))).collect();
            GapCut { cross, ..c.clone() }
        })
        .collect();
    ViaGeom { at: s.at, cuts, chords, reach: s.reach, tris: s.tris.clone() }
}

/// The gap cuts of a via of `net` at `at` in triangle `t` with their crossers ([`via_static`] +
/// [`with_crossers`]).
pub fn via_geom(st: &State, packer: &Packer, t: TriId, net: i32, at: Vec2, norm: Norm) -> ViaGeom {
    with_crossers(st, packer, &via_static(st.proj, packer, t, net, at, norm), net)
}

/// The via-to-via cut between this via (`g`, item `a`) and another planned via at `q` (item
/// `b`): Maley's cut between two islands, capacity `|pq| - d_a / 2 - d_b / 2` (p. 32), its items
/// the other nets' chords of the neighbourhood that cross it (neither via's net). `on(w, k)`: the
/// traversal lies on a layer of the cut (a through via pair: any layer, the caller decides).
/// `None`: nothing crosses, or it fits; else `1 + over / pitch`. O(chords).
pub fn pair_over(g: &ViaGeom, a: Item, q: Vec2, b: Item, norm: Norm, on: &dyn Fn(WireId, usize) -> Option<usize>, item: &dyn Fn(WireId, usize) -> Item) -> Option<f64> {
    let p = g.at;
    let len = norm.len(q - p) - 0.5 * a.w - 0.5 * b.w;
    // Per layer the crossers, ordered along the cut.
    let mut by_layer: Vec<(usize, Vec<(f64, Item)>)> = Vec::new();
    for &(w, k, c, d) in &g.chords {
        let it = item(w, k);
        if it.net == a.net || it.net == b.net {
            continue;
        }
        let Some(l) = on(w, k) else { continue };
        let Some((t, _)) = seg_cross(p, q, c, d) else { continue };
        match by_layer.iter_mut().find(|x| x.0 == l) {
            Some(x) => x.1.push((t, it)),
            None => by_layer.push((l, vec![(t, it)])),
        }
    }
    let mut worst: Option<f64> = None;
    for (_, mut its) in by_layer {
        its.sort_by(|x, y| x.0.total_cmp(&y.0));
        let items: Vec<Item> = its.iter().map(|x| x.1).collect();
        let ov = need(&items, Bound::Pad { net: Some(a.net), clear: a.s }, Bound::Pad { net: Some(b.net), clear: b.s }) - len;
        if ov > 1e-6 {
            let pitch = (items.iter().map(|i| i.w + i.s).sum::<f64>() / items.len() as f64).max(1.0);
            let f = 1.0 + ov / pitch;
            worst = Some(worst.map_or(f, |x: f64| x.max(f)));
        }
    }
    worst
}

/// Price the gap cuts of `g` for the via `via` (net, width `d`, clearance): `on(w, k, l)` = the
/// traversal `k` of `w` lies on layer `l`, `item(w, k)` its pass item. Per layer the worst cut
/// counts. O(cuts x crossers log crossers).
pub fn over(g: &ViaGeom, via: Item, on: &dyn Fn(WireId, usize, usize) -> bool, item: &dyn Fn(WireId, usize) -> Item) -> Over {
    let mut worst: Vec<(usize, f64)> = Vec::new();
    for c in &g.cuts {
        let mut its: Vec<(f64, Item)> = c.cross.iter().filter(|x| on(x.0, x.1, c.layer)).map(|x| (x.2, item(x.0, x.1))).collect();
        if its.is_empty() {
            continue;
        }
        its.push((c.via_t, via));
        its.sort_by(|a, b| a.0.total_cmp(&b.0));
        let items: Vec<Item> = its.iter().map(|x| x.1).collect();
        // A via never dips into its own net's pad (KiCad connection width; `stitch.rs`, D1): next
        // to an own-net pad end it takes its whole diameter (no clearance), where an own-net
        // track may lie on the pad's copper (`demand::need`, half its width).
        let wall = |b: Bound, it: Option<&(f64, Item)>| match (b, it) {
            (Bound::Pad { net: Some(n), .. }, Some(x)) if n == via.net && x.0 == c.via_t && x.1 == via => Bound::Edge { clear: 0.0 },
            _ => b,
        };
        let ov = need(&items, wall(c.ba, its.first()), wall(c.bb, its.last())) - c.len;
        if ov <= 1e-6 {
            continue;
        }
        let pitch = (items.iter().map(|i| i.w + i.s).sum::<f64>() / items.len() as f64).max(1.0);
        let f = 1.0 + ov / pitch;
        match worst.iter_mut().find(|x| x.0 == c.layer) {
            Some(x) => x.1 = x.1.max(f),
            None => worst.push((c.layer, f)),
        }
    }
    Over { count: worst.len(), graded: worst.iter().map(|x| x.1).sum() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x, y }
    }

    #[test]
    fn segment_crossings_are_open_on_the_cut_and_half_open_on_the_chord() {
        let (a, b) = (v(0.0, 0.0), v(0.0, 10.0));
        assert!(seg_cross(a, b, v(-1.0, 5.0), v(1.0, 5.0)).is_some_and(|x| (x.0 - 0.5).abs() < 1e-12));
        assert!(seg_cross(a, b, v(0.0, 5.0), v(1.0, 5.0)).is_some(), "a chord starting on the cut crosses (s = 0)");
        assert!(seg_cross(a, b, v(-1.0, 5.0), v(0.0, 5.0)).is_none(), "a chord ending on the cut does not (s = 1): its next one does");
        assert!(seg_cross(a, b, v(-1.0, 0.0), v(1.0, 0.0)).is_none(), "through the end point: no crossing of the open cut");
        assert!(seg_cross(a, b, v(1.0, 0.0), v(1.0, 10.0)).is_none(), "parallel");
    }

    const MM: f64 = 1e6;

    /// The wiccon gui27 gap by hand: a 1.0 mm cut from TXD's pad (net 1) to RXD's pad (net 3)
    /// through TXD's 0.8 mm via, RST (net 2, 0.25 mm, clearance 0.2) crossing it on layer 0:
    /// need 0.8 + 0.2 + 0.25 + 0.2 = 1.45 mm, over 0.45 mm (pitch (0.8 + 0.2 + 0.25 + 0.2) / 2).
    fn gui27() -> (ViaGeom, Item) {
        let cut = GapCut {
            layer: 0,
            a: v(0.0, 0.0),
            b: v(0.0, MM),
            ba: Bound::Pad { net: Some(1), clear: 0.2 * MM },
            bb: Bound::Pad { net: Some(3), clear: 0.2 * MM },
            len: MM,
            via_t: 0.4,
            cross: vec![(7, 3, 0.8)],
        };
        (ViaGeom { at: v(0.0, 0.4 * MM), cuts: vec![cut], chords: Vec::new(), reach: 3.0 * MM, tris: Vec::new() }, Item { net: 1, w: 0.8 * MM, s: 0.2 * MM })
    }

    fn rst(_: WireId, _: usize) -> Item {
        Item { net: 2, w: 0.25 * MM, s: 0.2 * MM }
    }

    #[test]
    fn a_via_in_a_pad_gap_over_fills_it_with_the_wire_that_passes() {
        let (g, via) = gui27();
        let o = over(&g, via, &|_, _, l| l == 0, &rst);
        assert_eq!(o.count, 1);
        let pitch = (1.0 + 0.45) / 2.0;
        assert!((o.graded - (1.0 + 0.45 / pitch)).abs() < 1e-9, "{o:?}");
        // The wire on the other layer: nothing on this cut.
        assert_eq!(over(&g, via, &|_, _, l| l == 1, &rst), Over::default());
        // A wider gap (1.5 mm): fits.
        let mut wide = g.clone();
        wide.cuts[0].len = 1.5 * MM;
        assert_eq!(over(&wide, via, &|_, _, _| true, &rst).count, 0);
        // No crosser: no item, nothing to price (a via alone in a gap is the via site's matter).
        let mut empty = g.clone();
        empty.cuts[0].cross.clear();
        assert_eq!(over(&empty, via, &|_, _, _| true, &rst).count, 0);
    }

    /// A via never dips into its own pad: next to it the via takes its whole diameter. An own-net
    /// track there would only take half its width (`demand::need`), giving 1.05 mm here.
    #[test]
    fn the_via_takes_its_whole_diameter_next_to_its_own_pad() {
        let (mut g, via) = gui27();
        g.cuts[0].len = 1.2 * MM;
        assert_eq!(over(&g, via, &|_, _, _| true, &rst).count, 1, "1.45 > 1.2");
        // RST between the via and the own pad: the pad's clearance counts, the via is at the far end.
        g.cuts[0].cross = vec![(7, 3, 0.1)];
        g.cuts[0].len = 1.44 * MM;
        assert_eq!(over(&g, via, &|_, _, _| true, &rst).count, 1, "0.2 + 0.25 + 0.2 + 0.8 + 0.2 (RXD pad) = 1.65 > 1.44");
    }

    /// Per layer the worst cut counts once; two layers count twice.
    #[test]
    fn one_count_per_over_full_layer() {
        let (mut g, via) = gui27();
        let mut c2 = g.cuts[0].clone();
        c2.len = 1.1 * MM;
        g.cuts.push(c2.clone());
        assert_eq!(over(&g, via, &|_, _, _| true, &rst).count, 1);
        c2.layer = 1;
        g.cuts.push(c2);
        assert_eq!(over(&g, via, &|_, _, _| true, &rst).count, 2);
    }

    /// topo_reroute52's shape: two VCC vias (0.8) with SDA (0.25, clearance 0.2) between them on
    /// F.Cu. Needs 0.4 + 0.65 + 0.4 = 1.45 mm centre to centre: 1.36 apart is over-full, 1.67
    /// (the plan's sites there) fits; a crosser of either via's net is no item.
    #[test]
    fn a_wire_between_two_planned_vias_needs_their_gap() {
        let via = Item { net: 2, w: 0.8 * MM, s: 0.2 * MM };
        let sda = |_: WireId, _: usize| Item { net: 1, w: 0.25 * MM, s: 0.2 * MM };
        let g = ViaGeom { at: v(0.0, 0.0), cuts: Vec::new(), chords: vec![(5, 2, v(0.68 * MM, -MM), v(0.68 * MM, 1.0 * MM))], reach: 5.0 * MM, tris: Vec::new() };
        let on = |_: WireId, _: usize| Some(0usize);
        let f = pair_over(&g, via, v(1.36 * MM, 0.0), via, Norm::Euclid, &on, &sda).expect("over-full");
        assert!((f - (1.0 + 0.09 / 0.45)).abs() < 1e-9, "{f}");
        assert!(pair_over(&g, via, v(1.67 * MM, 0.0), via, Norm::Euclid, &on, &sda).is_none());
        // The chord misses the segment between them (the wire goes round): nothing.
        let g2 = ViaGeom { chords: vec![(5, 2, v(0.68 * MM, 0.2 * MM), v(0.68 * MM, 1.0 * MM))], ..g.clone() };
        assert!(pair_over(&g2, via, v(1.36 * MM, 0.0), via, Norm::Euclid, &on, &sda).is_none());
        // A VCC wire between them: own net, no item.
        assert!(pair_over(&g, via, v(1.36 * MM, 0.0), via, Norm::Euclid, &on, &|_, _| Item { net: 2, w: 0.25 * MM, s: 0.2 * MM }).is_none());
    }

    #[test]
    fn closest_point_of_a_square() {
        let sq = [v(0.0, 0.0), v(2.0, 0.0), v(2.0, 1.0), v(0.0, 1.0)];
        let (q, d) = closest_on_poly(v(1.0, 3.0), &sq);
        assert!((q - v(1.0, 1.0)).len() < 1e-12 && (d - 2.0).abs() < 1e-12);
    }
}
