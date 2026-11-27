//! Track end anchors (edge-clear): where a run ends on a pad, its track ends at the pad centre
//! unless the end's copper there breaks a static clearance; then it ends at the nearest point
//! inside the pad that keeps every one.
//!
//! The case: a track wider than its pad (a 0.25 mm track on a 0.2 mm pad, the board's minimum
//! track width above the pad's width, so no neck helps) next to the board edge or another
//! static feature that keeps exactly its clearance to the PAD. The pad is legal, the track's
//! round end at the pad centre is not (it sticks out of the pad on both sides), and every path
//! that starts there is illegal: the exact search has no legal start node, the drawing forces the
//! straight line with the violation (fx pattern `edge_pads`: x = 0.600, KiCad copper-to-edge
//! "actual 0.4750 mm" < 0.5). KiCad's connectivity only needs the track end inside the pad
//! (its reference answer ends the track 0.027 mm off the centres, DRC 0 and connected), so the
//! end may move inside the pad by the deficit.
//!
//! Static only (pads, holes, board edge, foreign shapes and kept copper, keepouts: the
//! [`Drawer`]'s first `n_static` objects), so the anchor is fixed before ERBS and drawing and
//! independent of the draw order; the clearance test is the drawing's own (`need_of`, `seg_clear`
//! of `exact`). The planner keeps the centre vertex: the topology is the same, the shift lies
//! inside the pad. Centre legal (every ordinary pad) = the centre itself, so nothing else changes.
//!
//! Search: from the centre, push away from the worst violated object by its deficit (exact for one
//! straight feature such as the board edge; repeated for a corner), and a bisection along 16 rays
//! inside the pad as the fallback; the legal candidate nearest the centre wins. None legal: the
//! centre (never drop prices the violation as before) and the end is counted.
//! O(static objects near the pad x (push steps + 16 x ray steps)) per end, ends only.

use super::draw::{bbox_of, seg_core, Drawer};
use super::exact::Wp;
use super::state::{point_in_poly, VertKind, WireId, NO_OBS};
use crate::geom::Vec2;

/// Tolerance on a clearance test (nm), as `exact`.
const TOL: f64 = 0.5;
/// Push steps from the centre.
const PUSHES: usize = 16;
/// Ray directions of the fallback.
const RAYS: usize = 16;

/// Where one run end lies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// The pad centre keeps every static clearance (or the end is no pad).
    Centre,
    /// Moved to this point inside the pad.
    Moved(Vec2),
    /// No point inside the pad keeps them at the wire's width: the centre stays.
    None,
}

impl Drawer<'_> {
    /// The anchor of the end `end` (0: start, 1: end) of wire `w` on `layer` (see the module doc).
    pub fn pad_anchor(&self, w: WireId, end: usize, layer: usize) -> Anchor {
        let (st, pr) = (self.st, &self.b.proj);
        let wi = &st.wires[w];
        let (ob, v) = if end == 0 { (wi.pa, wi.a) } else { (wi.pb, wi.b) };
        if ob == NO_OBS || ob >= pr.obstacles.len() || matches!(pr.vkind[v], VertKind::Node) {
            return Anchor::Centre;
        }
        let poly = &pr.obstacles[ob].poly;
        if poly.len() < 3 {
            return Anchor::Centre;
        }
        let c = pr.pos[v];
        let wp = Wp { wire: w, net: wi.net, w: wi.w, s: wi.s, layer, own: [wi.pa, wi.pb], neck: None, relax: 0.0 };
        let reach = poly.iter().map(|&p| (p - c).len()).fold(0.0, f64::max);
        let ents: Vec<usize> = self.local_ents(&wp, bbox_of(poly).grow(self.qgrow + wi.w)).into_iter().filter(|&i| i < self.n_static).collect();
        let ok = |p: Vec2| self.seg_clear(&ents, &wp, p, p);
        if ok(c) {
            return Anchor::Centre;
        }
        // Inside the pad by at least the drawing's tolerance (KiCad's connectivity: the end in the pad).
        let margin = self.eps.max(1000.0);
        let inside = |p: Vec2| point_in_poly(p, poly) && boundary_dist(p, poly) >= margin;
        let mut best: Option<Vec2> = None;
        let take = |p: Vec2, best: &mut Option<Vec2>| {
            if best.is_none_or(|b| (p - c).len2() < (b - c).len2()) {
                *best = Some(p);
            }
        };
        // Push.
        let mut p = c;
        for _ in 0..PUSHES {
            let mut worst: Option<(f64, Vec2)> = None;
            for &i in &ents {
                let need = self.need_of(i, &wp, wi.w);
                let (d, q1, q2) = seg_core(p, p, &self.ents[i].core);
                if d >= need - TOL {
                    continue;
                }
                // The object's point: the one of the two that is not `p`.
                let q = if (q1 - p).len2() >= (q2 - p).len2() { q1 } else { q2 };
                let (dir, amount) = if d > 1.0 {
                    ((p - q) * (1.0 / d), need - d)
                } else {
                    // `p` on or inside the object: out through its nearest boundary point.
                    let l = (q - p).len();
                    if l < 1.0 {
                        continue;
                    }
                    ((q - p) * (1.0 / l), l + need)
                };
                if worst.is_none_or(|(a, _)| amount > a) {
                    worst = Some((amount, dir * (amount + 2.0 * TOL + 1.0)));
                }
            }
            match worst {
                None => break,
                Some((_, step)) => p = p + step,
            }
        }
        if (p - c).len2() > 0.0 && inside(p) && ok(p) {
            take(p, &mut best);
        }
        // Rays: the first legal point along each, bisected.
        let step = (reach / 64.0).max(2000.0);
        for k in 0..RAYS {
            let a = 2.0 * std::f64::consts::PI * k as f64 / RAYS as f64;
            let u = Vec2 { x: a.cos(), y: a.sin() };
            let mut lo = 0.0;
            let mut t = step;
            let mut hit = None;
            while t <= reach {
                let q = c + u * t;
                if !inside(q) {
                    break;
                }
                if ok(q) {
                    hit = Some(t);
                    break;
                }
                lo = t;
                t += step;
            }
            let Some(mut hi) = hit else { continue };
            for _ in 0..24 {
                let mid = 0.5 * (lo + hi);
                if ok(c + u * mid) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            take(c + u * hi, &mut best);
        }
        match best {
            Some(p) => Anchor::Moved(p),
            None => Anchor::None,
        }
    }

    /// The anchors of every pad end of the given wires on the layer of the run touching it
    /// (`layers`: first and last layer of the wire's plan): the moved ones into
    /// [`Drawer::pad_end`], counted in [`Drawer::stats`]. Deterministic.
    pub fn set_pad_ends(&mut self, ends: &[(WireId, [usize; 2])]) {
        for &(w, layers) in ends {
            for (end, &layer) in layers.iter().enumerate() {
                match self.pad_anchor(w, end, layer) {
                    Anchor::Centre => {}
                    Anchor::Moved(p) => {
                        let wi = &self.st.wires[w];
                        let c = self.b.proj.pos[if end == 0 { wi.a } else { wi.b }];
                        self.stats.pad_end_moved += 1;
                        self.stats.pad_end_max = self.stats.pad_end_max.max((p - c).len());
                        self.pad_end.insert((w, end), p);
                    }
                    Anchor::None => self.stats.pad_end_none += 1,
                }
            }
        }
    }

    /// Where run end `end` of wire `w` lies: a branch end on its trunk (`end_at`), else the pad
    /// anchor, else the terminal vertex.
    pub fn run_end(&self, w: WireId, end: usize) -> Vec2 {
        let wi = &self.st.wires[w];
        self.end_at.get(&(w, end)).or_else(|| self.pad_end.get(&(w, end))).copied().unwrap_or(self.b.proj.pos[if end == 0 { wi.a } else { wi.b }])
    }
}

/// Distance from `p` to the boundary of `poly` (any simple polygon). O(n).
pub(super) fn boundary_dist(p: Vec2, poly: &[Vec2]) -> f64 {
    let n = poly.len();
    (0..n).map(|i| super::draw::seg_dist(p, poly[i], poly[(i + 1) % n])).fold(f64::INFINITY, f64::min)
}


/// draw-via-neck C (`[drawing] pad_tip_anchor`): the long axis of a pad outline round `c`
/// (direction of its longest edge), its half length along it and half width across it.
pub(super) fn pad_axis(poly: &[Vec2], c: Vec2) -> Option<(Vec2, f64, f64)> {
    let n = poly.len();
    if n < 3 {
        return None;
    }
    let e = (0..n).map(|i| poly[(i + 1) % n] - poly[i]).max_by(|a, b| a.len2().total_cmp(&b.len2()))?;
    let l = e.len();
    if l < 1.0 {
        return None;
    }
    let d = e * (1.0 / l);
    let nrm = Vec2 { x: -d.y, y: d.x };
    let hl = poly.iter().map(|&p| (p - c).dot(d).abs()).fold(0.0, f64::max);
    let hw = poly.iter().map(|&p| (p - c).dot(nrm).abs()).fold(0.0, f64::max);
    Some((d, hl, hw))
}

impl Drawer<'_> {
    /// draw-via-neck C (`[drawing] pad_tip_anchor`; TopoR 6.3 manual p. 71: a trace leaves a pad
    /// along its axis): the band `path` of a run of wire `w` on `layer` that ends (`end` 0: at
    /// its start, 1: at its end) in its own elongated pad through the pad's long side (or whose
    /// node next to the pad is off the axis past the tip) re-anchored at the pad's tip: the band's
    /// points within reach of the pad are replaced by the axis point `t` past the tip (`t` from
    /// one trace radius out, the smallest that keeps every rule at the full width). r3: decided
    /// when the run is drawn ([`Drawer::draw_wire`], attempt -1), against the copper in the index
    /// then: the wires drawn before it as drawn (their octilinear shapes) and the reserved bands
    /// of the joint commit as committed; `skip` = the index ids of the run's own reserved copper
    /// (the old tail). Only where nothing of another net lies between the old tail and the new
    /// one (static copper, drawn or reserved copper, and the bands of the wires whose copper is
    /// not in the index yet: the class is kept); else, or with no legal `t`, `None` (the run
    /// stays). The octilinear stage's axis exit then applies (its previous node lies past the
    /// tip). O(9 tries x (checker query + live objects + bands)).
    /// draw-via-neck C r5 (verifier F4, tw2_metric reverse_draw_order): the tip anchors of the
    /// reserved end runs, decided once after the joint commit and before any run is drawn, so the
    /// draw order decides nothing (TopoR way: decide early; the drawing only realises). r3 decided
    /// each anchor when its run was drawn, against the anchors already drawn: of three diagonals
    /// into a pad row the middle one anchored only when its right neighbour had been drawn (and
    /// anchored) before it. Here every reserved end run (`r` = 0 or the last run of the plan) is
    /// tried in (wire, run) order against the index as the joint commit left it (the reserved
    /// bands, the anchors taken so far) and the bands not reserved; an anchor replaces the run's
    /// reservation (its copper at the full width, as r3's attempt -1 drew it). Sweeps repeat until
    /// none adds an anchor (an anchor frees the next pad's tip; at most 4 sweeps), so the set is a
    /// fixpoint of the order-free rule. Runs without a reservation are still decided when drawn.
    /// O(sweeps x reserved end runs x `tip_anchor_path`).
    pub fn tip_anchor_reserved(&mut self, plans: &std::collections::BTreeMap<WireId, super::draw::Plan>) {
        if !self.opts.pad_tip_anchor {
            return;
        }
        let mut done: std::collections::BTreeSet<(WireId, usize)> = std::collections::BTreeSet::new();
        for _ in 0..4 {
            let mut added = false;
            let keys: Vec<(WireId, usize)> = self.joint.reserved.keys().copied().filter(|k| !done.contains(k)).collect();
            for (w, r) in keys {
                let Some(plan) = plans.get(&w) else { continue };
                let n = plan.layers.len();
                if r != 0 && r + 1 != n {
                    continue;
                }
                let Some(res) = self.joint.reserved.get(&(w, r)) else { continue };
                // draw-absorb A1: a band the joint commit necked against a gap partner keeps its
                // neck (re-anchoring draws it at full width; never wider after the neck).
                if res.pts.len() < 2 || res.zones.is_some() {
                    continue;
                }
                let layer = plan.layers[r];
                let mut skip: Vec<usize> = res.handles.iter().map(|&h| self.handles[h].0).collect();
                skip.sort_unstable();
                let (s0, e0) = (res.pts[0], res.pts[res.pts.len() - 1]);
                // draw-absorb A1b meter: the reserved neck this re-anchor drops. The anchored band
                // is drawn at full width (main; lead 2026-11-27: `reserved_keep_neck` does not apply
                // here, the dropped neck is the plan's hook into the pad side, and the user-approved
                // Q3 expectation is the pad-axis entry at full width).
                let dropped = res.pneck.map_or(0.0, |p| p.1.iter().sum::<f64>());
                let mut p = res.pts.clone();
                let mut moved = false;
                for end in [0usize, 1] {
                    if (end == 0 && r != 0) || (end == 1 && r + 1 != n) {
                        continue;
                    }
                    if let Some(np) = self.tip_anchor_path(w, end, layer, &p, &skip) {
                        p = np;
                        moved = true;
                    }
                }
                if !moved {
                    continue;
                }
                if dropped > 0.0 {
                    let room = match (self.plan_neck(w), res.pneck) {
                        (Some(n), Some((wn, _))) => self.room_taken(w, &p, &n, wn),
                        _ => 0.0,
                    };
                    if std::env::var_os("TOPO_A1BDBG").is_some() {
                        eprintln!("A1B tip wire {w} net {} run {r} dropped {:.3} mm room {:.3} mm old {:?} new {:?}", self.st.wires[w].net, dropped / 1e6, room / 1e6, res.pts.iter().map(|q| ((q.x / 1e3).round() / 1e3, (-q.y / 1e3).round() / 1e3)).collect::<Vec<_>>(), p.iter().map(|q| ((q.x / 1e3).round() / 1e3, (-q.y / 1e3).round() / 1e3)).collect::<Vec<_>>());
                    }
                    self.joint.unnecked.push((w, r, dropped, room, "tip"));
                }
                let items = self.run_items(w, layer, &p);
                let old = self.joint.reserved.remove(&(w, r)).expect("reserved");
                let cur = self.joint.cur.replace(w);
                for h in old.handles {
                    self.uncommit(h);
                }
                self.joint.cur = cur;
                let hs: Vec<usize> = items.iter().map(|it| self.commit(it)).collect();
                for &h in &hs {
                    let id = self.handles[h].0;
                    self.joint.owner.insert(h, w);
                    self.joint.band_ids.insert(id, (w, h));
                }
                // The band the drawing reads (attempt -1, the later wires' corridors) is the anchored one.
                if let Some(rb) = self.erbs.get_mut(&w).and_then(|v| v.iter_mut().find(|b| b.layer == layer && (b.s - s0).len() < 1000.0 && (b.e - e0).len() < 1000.0)) {
                    rb.path = p.clone();
                }
                self.joint.reserved.insert((w, r), super::joint::Reserved { pts: p, items, handles: hs, pneck: None, octi: false, zones: None });
                self.stats.tip_anchored += 1;
                done.insert((w, r));
                added = true;
            }
            if !added {
                break;
            }
        }
    }

    /// draw-absorb A1b (`[drawing] reserved_keep_neck`): the tracks of the run `pts` of wire `w`
    /// with the reserved neck `z` (the plan's zones at the reserved width `z.min_w`), each zone cut
    /// to what the checker needs and grown where the full width would take planned room of traces
    /// not drawn yet ([`Drawer::fit_neck_with`], `[drawing] neck_trim`; without it the plan's
    /// zones as they are): never wider than reserved where the plan's neck holds someone's room.
    /// `(items, necked length per zone)`.
    pub(super) fn kept_neck_items(&self, w: WireId, layer: usize, pts: &[Vec2], z: &super::exact::Neck) -> (Vec<crate::model::Item>, [f64; super::exact::NECK_ZONES]) {
        let lid = self.layer_id(layer);
        let nk = if self.opts.neck_trim { self.fit_neck_with(w, lid, pts, z, z.min_w, true) } else { *z };
        self.neck_items(w, lid, pts, &nk, z.min_w)
    }

    pub(super) fn tip_anchor_path(&self, w: WireId, end: usize, layer: usize, path: &[Vec2], skip: &[usize]) -> Option<Vec<Vec2>> {
        let (st, pr) = (self.st, &self.b.proj);
        let wi = &st.wires[w];
        let ob = if end == 0 { wi.pa } else { wi.pb };
        if ob == NO_OBS || ob >= pr.obstacles.len() {
            return None;
        }
        let poly = &pr.obstacles[ob].poly;
        // The band from the pad outwards.
        let mut q: Vec<Vec2> = path.to_vec();
        if end == 1 {
            q.reverse();
        }
        let c = q[0];
        if !point_in_poly(c, poly) {
            return None;
        }
        let (d, hl, hw) = pad_axis(poly, pr.obstacles[ob].center)?;
        if hl < 1.2 * hw {
            return None;
        }
        let pc = pr.obstacles[ob].center;
        // Where the band leaves the pad: through the tip end, nothing to do.
        let j_out = q.iter().position(|&p| !point_in_poly(p, poly))?;
        let (a, b) = (q[j_out - 1], q[j_out]);
        let mut lo = 0.0f64;
        let mut hi = 1.0f64;
        for _ in 0..30 {
            let m = 0.5 * (lo + hi);
            if point_in_poly(a + (b - a) * m, poly) {
                lo = m;
            } else {
                hi = m;
            }
        }
        let exit = a + (b - a) * lo;
        let ax = (exit - pc).dot(d);
        let dbg = std::env::var("TOPO_TIPDBG").is_ok();
        if dbg {
            eprintln!("TIPEND wire {w} end {end} pad ({:.3},{:.3}) exit ({:.3},{:.3}) axial {:.3} hl {:.3} hw {:.3}", pc.x / 1e6, -pc.y / 1e6, exit.x / 1e6, -exit.y / 1e6, ax / 1e6, hl / 1e6, hw / 1e6);
        }
        // Re-anchored when the band reaches the pad through its long side, or when it does not
        // keep every rule at the full width (the hook round the tip's corner, necked by the
        // neighbour pad): then the axis leg is tried.
        // Or when the band's node next to the pad does not lie on the axis past the tip (a
        // diagonal approach to the tip's corner: the octilinear axis exit needs that node).
        let side = ax.abs() < hl - 0.5 * wi.w;
        let nrm = Vec2 { x: -d.y, y: d.x };
        let q1 = q[1];
        let on_axis = (q1 - pc).dot(nrm).abs() <= 1000.0 && (q1 - pc).dot(d).abs() >= hl + 0.5 * wi.w;
        if !side && on_axis {
            return None;
        }
        // The tip the band heads for: the side of the axis where it goes on.
        let far = q.iter().find(|&&p| poly_dist_out(p, poly) > wi.w + wi.s).copied().unwrap_or(q[q.len() - 1]);
        let sg = (far - pc).dot(d);
        if sg.abs() < 1.0 {
            return None;
        }
        let u = d * sg.signum();
        let pitch = wi.w + wi.s;
        let mut tries: Vec<f64> = (0..9).map(|k| 0.5 * wi.w + k as f64 * pitch * 0.25).collect();
        tries.sort_by(f64::total_cmp);
        for t in tries {
            let tp = pc + u * (hl + t);
            let reach = t + 0.5 * wi.w + wi.s;
            // The first band point beyond the pad's reach.
            let Some(j) = q.iter().position(|&p| poly_dist_out(p, poly) > reach && (p - pc).dot(u) >= hl) else { continue };
            let mut np: Vec<Vec2> = vec![c, tp];
            np.extend_from_slice(&q[j..]);
            // The swept area between the old and the new tail holds nothing of another net.
            let mut ring: Vec<Vec2> = vec![c, tp];
            ring.extend(q[1..=j].iter().rev().copied());
            if self.foreign_inside(wi.net, layer, &ring) || self.drawn_inside(wi.net, layer, &ring) {
                if dbg {
                    eprintln!("TIPREJ wire {w} t {:.3}: foreign copper between", t / 1e6);
                }
                continue;
            }
            // Nor of another net's band on the layer (its wire is not drawn yet): none inside the
            // swept area or across the new tail (the class), none closer to it than to the old
            // tail where that was under the pitch (its planned room).
            if let Some(why) = self.band_in_way(w, layer, &ring, &[c, tp, q[j]], &q[..=j]) {
                if dbg {
                    eprintln!("TIPREJ wire {w} t {:.3}: band of wire {why} in the way", t / 1e6);
                }
                continue;
            }
            let pts: Vec<Vec2> = if end == 1 { np.iter().rev().copied().collect() } else { np.clone() };
            let items = self.run_items(w, layer, &pts[..]);
            if !self.checker.check_against(&self.idx, &items, skip).violations.is_empty() || !self.track_own_pad_ok(w, &items) {
                if dbg {
                    eprintln!("TIPREJ wire {w} t {:.3}: rules", t / 1e6);
                }
                continue;
            }
            if std::env::var("TOPO_TIPDBG").is_ok() {
                eprintln!("TIPANCHOR wire {w} end {end} pad ({:.3},{:.3}) exit axial {:.3} hl {:.3} t {:.3}", pc.x / 1e6, -pc.y / 1e6, ax / 1e6, hl / 1e6, t / 1e6);
            }
            return Some(pts);
        }
        None
    }

    /// The first wire of another net whose ERBS band on `layer` has a point inside `ring`, crosses
    /// the polyline `new`, or comes closer to `new` than its pitch (half the two widths plus the
    /// larger clearance) and than to the polyline `old`; `None` when no band is in the way. r3:
    /// only the bands whose copper is not in the index (wire not drawn yet, run not reserved by
    /// the joint commit): a drawn or reserved run is judged by its copper (the checker,
    /// [`Drawer::drawn_inside`]), as the drawing made it (an octilinear shape leaves its band).
    /// O(bands x segments).
    fn band_in_way(&self, w: WireId, layer: usize, ring: &[Vec2], new: &[Vec2], old: &[Vec2]) -> Option<WireId> {
        let st = self.st;
        let wi = &st.wires[w];
        let bb = bbox_of(ring);
        let dmin = |a: &[Vec2], b: &[Vec2]| a.windows(2).flat_map(|x| b.windows(2).map(move |y| super::draw::seg_seg_dist(x[0], x[1], y[0], y[1]))).fold(f64::INFINITY, f64::min);
        for (&ow, runs) in &self.erbs {
            let wo = &st.wires[ow];
            if ow == w || wo.net == wi.net || self.drawn_once.get(ow).copied().unwrap_or(false) {
                continue;
            }
            let need = 0.5 * (wi.w + wo.w) + wi.s.max(wo.s);
            let reserved = |rb: &super::erbs::RunBand| self.joint.reserved.range((ow, 0)..=(ow, usize::MAX)).any(|(_, r)| r.pts.first().is_some_and(|p| (*p - rb.path[0]).len() < 1_000.0));
            for rb in runs.iter().filter(|r| r.layer == layer && r.path.len() >= 2 && !reserved(r)) {
                let p = &rb.path;
                let inside = p.iter().any(|&x| x.x >= bb.min.x && x.x <= bb.max.x && x.y >= bb.min.y && x.y <= bb.max.y && point_in_poly(x, ring));
                let cross = p.windows(2).any(|a| new.windows(2).any(|b| super::draw::seg_cross_sos(a[0], a[1], b[0], b[1])));
                let dn = dmin(p, new);
                if inside || cross || dn < need.min(dmin(p, old)) - 1.0 {
                    return Some(ow);
                }
            }
        }
        None
    }

    /// Copper in the index of another net (drawn or reserved tracks and vias: the drawer's live
    /// dynamic objects) on `layer` has a point inside the polygon `ring` (a segment: an end or
    /// its middle; the checker sees the rest where it comes near the new tail). O(live objects).
    fn drawn_inside(&self, net: i32, layer: usize, ring: &[Vec2]) -> bool {
        let bb = bbox_of(ring);
        let bit = 1u64.checked_shl(layer as u32).unwrap_or(0);
        let inside = |p: Vec2| p.x >= bb.min.x && p.x <= bb.max.x && p.y >= bb.min.y && p.y <= bb.max.y && point_in_poly(p, ring);
        self.ents[self.n_static..].iter().any(|e| {
            e.live
                && e.net != Some(net)
                && e.mask & bit != 0
                && match &e.core {
                    super::draw::Core::Seg(a, b) => inside(*a) || inside(*b) || inside((*a + *b) * 0.5),
                    super::draw::Core::Pt(c) => inside(*c),
                    super::draw::Core::Poly(p) => p.iter().any(|&q| inside(q)),
                }
        })
    }

    /// An obstacle of another net (or of none) on `layer` has a point inside the polygon `ring`.
    fn foreign_inside(&self, net: i32, layer: usize, ring: &[Vec2]) -> bool {
        let bb = bbox_of(ring);
        self.b.proj.obstacles.iter().any(|o| {
            o.net != Some(net)
                && (o.layers.has(layer) || o.drilled)
                && std::iter::once(o.center).chain(o.poly.iter().copied()).any(|p| p.x >= bb.min.x && p.x <= bb.max.x && p.y >= bb.min.y && p.y <= bb.max.y && point_in_poly(p, ring))
        })
    }
}

/// Distance from `p` to the outline `poly` when outside it, 0 inside.
fn poly_dist_out(p: Vec2, poly: &[Vec2]) -> f64 {
    if point_in_poly(p, poly) {
        0.0
    } else {
        super::draw::poly_dist(p, poly)
    }
}
