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
    /// along its axis): every ERBS run that ends in its own elongated pad through the pad's long
    /// side (the band reached the centre through the pad's side triangle, the plan's spoke passes
    /// of the pad itself) is re-anchored at the pad's tip: the band's points within reach of the
    /// pad are replaced by the axis point `t` past the tip (`t` from one trace radius out, the
    /// smallest that keeps every rule at the full width against the copper indexed so far: the
    /// static board and the placed vias). Only where nothing of another net lies between the old
    /// tail and the new one (the class is kept); else, or with no legal `t`, the run stays.
    /// The octilinear stage's axis exit then applies (its previous node lies past the tip).
    /// Returns the number of re-anchored ends. O(runs x 9 tries x checker query).
    pub fn anchor_pad_tips(&mut self) -> usize {
        let mut n_done = 0;
        let wires: Vec<WireId> = self.erbs.keys().copied().collect();
        for w in wires {
            let runs = self.erbs[&w].clone();
            let last = runs.len().saturating_sub(1);
            let mut new_runs = runs.clone();
            for (ri, rb) in runs.iter().enumerate() {
                for end in [0usize, 1] {
                    if (end == 0 && ri != 0) || (end == 1 && ri != last) || rb.path.len() < 2 {
                        continue;
                    }
                    let cur = &new_runs[ri].path;
                    if let Some(p) = self.tip_anchor_path(w, end, rb.layer, cur) {
                        new_runs[ri].path = p;
                        n_done += 1;
                    }
                }
            }
            self.erbs.insert(w, new_runs);
        }
        n_done
    }

    /// The re-anchored band of [`Drawer::anchor_pad_tips`] at end `end` of wire `w`, or `None`.
    fn tip_anchor_path(&self, w: WireId, end: usize, layer: usize, path: &[Vec2]) -> Option<Vec<Vec2>> {
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
            if self.foreign_inside(wi.net, layer, &ring) {
                if dbg {
                    eprintln!("TIPREJ wire {w} t {:.3}: foreign copper between", t / 1e6);
                }
                continue;
            }
            let pts: Vec<Vec2> = if end == 1 { np.iter().rev().copied().collect() } else { np.clone() };
            let items = self.run_items(w, layer, &pts[..]);
            if !self.checker.check_against(&self.idx, &items, &[]).violations.is_empty() || !self.track_own_pad_ok(w, &items) {
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
