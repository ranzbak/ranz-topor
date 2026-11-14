//! Shortest path through a sequence of portals (string pulling), shared by the taut paths of
//! [`super::pack`] and [`super::state`], the drawing ([`super::draw::funnel`]) and the cut
//! bands of [`super::cutflow`].
//!
//! `ports[i]` is `(left, right)` seen in the direction of travel; the first and the last are the
//! two terminals (`left == right`); a middle portal with `left == right` is a point the path
//! must pass (a turn vertex). The path is the shortest polyline from the first terminal to the
//! last that *touches every portal in order*; its corners are portal ends (and, where the
//! input asks the path to touch a portal from the side it is already on, the touch point; see
//! below).
//!
//! **Funnel.** On a proper sleeve (every portal shares one end with the one before it, each
//! triangle between two consecutive portals turning the way of travel) this is the funnel
//! algorithm of Lee & Preparata (1984, Networks 14:393-410), as in Hershberger & Snoeyink (1994,
//! CGTA 4:63-98) and J. Erickson's Computational Topology lecture notes, "Shortest (Homotopic)
//! Paths", section "Growing Funnels" (an HTML page without page numbers; local copy
//! docs/literature/erickson-shortest-homotopic.html, from
//! jeffe.cs.illinois.edu/teaching/comptop/2023/notes/05-shortest-homotopic.html): an apex and two
//! concave chains; a new end is tested against the *last edge of its own chain* (popping the
//! ends it sees past) and, once that chain is empty, against the *first edge of the other
//! chain* (each end it crosses over becomes a corner and the next apex). Chain-edge tests keep
//! a funnel that turns through more than 180 degrees right (the `cutflow` S bundle). Portals
//! with two new ends (margin-shrunk intervals) are two sleeve steps, right end first. O(portals).
//!
//! **Exact predicates.** Every turn test is the exact sign of an orientation of the `f64`
//! points as given ([`crate::geom::predicates::orient`], Shewchuk 1997), and collinear points
//! are ordered by coordinate comparisons ([`crate::geom::predicates::on_ray`]); so two tests on
//! the same points never disagree by rounding, and the ties below are decided on the true
//! geometry of the input.
//!
//! **Shared ends.** Exact tests need a vertex shared by two portals to be the same `f64` point
//! in both. On entry, an end within a few ulps of an end of the portal before it (a shared
//! vertex computed twice, along two edges) is replaced by that end, and a portal whose two ends
//! are that close is a point portal. The radius is a floating-point error bound relative to the
//! largest coordinate, far below any feature of the input ([`same_point_tol`]; at most 2e-6 nm
//! on a board); fold points (constructions) that close to the corner next to them are dropped.
//!
//! **Ties** (the apex `a`, the other chain's first vertex `c`, the new end `p` collinear):
//! `p` on the far side of `a` from `c` (the apex lies on the portal between its ends: it touches
//! it there) does not cross; `p` at or beyond `c` crosses over `c` (a corner without a turn);
//! `p` strictly between `a` and `c` (a portal on a ray from the apex, e.g. an edge radial from a
//! turn vertex) is a corner itself: every path from `a` to the portal passes `p`, so `p`
//! becomes the apex and `c` stays on the other chain. An end on the line of its own chain's
//! last edge pops that edge's end (the path runs straight through it).
//!
//! **Inputs that are no sleeve.** The callers pass margin-shrunk sub-intervals, turn vertices
//! as point portals followed by portals on edges radial from them, and drawing runs that start
//! on or in front of their first portal or end behind their last. Two situations a sleeve never
//! has are detected exactly and solved by reflection (Heron's principle; the unfolding used for
//! paths that must touch a sequence of lines or polygons: Dror, Efrat, Lubiw & Mitchell,
//! "Touring a sequence of polygons", STOC 2003, sec. 2):
//!
//! * the apex sees both (new) ends of portal `i` the wrong way round (left right of
//!   apex -> right): the apex lies in front of the portal's line, the path has to come back to
//!   touch it. The apex is replaced by its mirror image in that line (a virtual apex behind the
//!   portal); when the next corner is fixed, the straight leg from the virtual apex is folded
//!   back at the line, adding the touch point. If the rounded mirror image does not come out
//!   behind the line or is the apex itself within the same-point tolerance (the apex lies on
//!   the line up to rounding: a portal on an edge radial from a turn vertex, its ends computed
//!   along the edge), the collinear tie above decides, along the line: the apex inside the
//!   portal touches it there, else the portal's near end is a corner.
//! * a point portal behind the line of the portal before it (an end pad the last pass lies
//!   beyond, a turn vertex just behind its radial edge) whose plain funnel path misses that
//!   portal (tested exactly; a band wrapping a corner by more than 180 degrees comes back
//!   round the portal's end and keeps its plain path): the point is mirrored in that line, the
//!   funnel runs to the image and the last leg is folded back at the line.
//!
//! The mirror for a portal seen the wrong way round is used only while every portal since the
//! apex passes through the apex (none has to be touched on the way back to the line). Three
//! more rules keep portals that pass through the apex (degenerate fans, radial edges of a turn
//! vertex) from hiding the ones that do not, all exact:
//!
//! * an end crossing over a vertex of the other chain makes no corner there when every portal
//!   since the apex, up to the last one that vertex is an end of, passes through the apex: the
//!   vertex is on the chain only for portals touched at the apex already.
//! * a point portal equal to the apex while a portal since the apex misses it: the path goes
//!   out and comes back to the point (Lee & Preparata's funnel never revisits the apex; the
//!   touching order requires it here). With one such portal it goes to the portal's nearest
//!   point and back (shortest); with several, out along the shorter chain, touching them at its
//!   ends, and back (a heuristic: the shortest such tour may touch them between their ends).
//! * a right end equal to the apex is not added; when the left end then moves the apex on,
//!   the right end is added from the new apex (the portal is still to be touched).
//! * a segment portal that holds, up to rounding (within the same-point tolerance, not exactly),
//!   the point portal after it or the apex (while every portal since the apex passes through
//!   it) is touched at that point, in order, and is skipped: a terminal on its first or last
//!   portal, a turn vertex computed on a portal. Whether such a point lies in front of the
//!   line or behind it is rounding and decides nothing; exact incidences keep the exact rules.
//!
//! Corners are portal ends (with the index of the last portal each is an end of) and the two
//! terminals; a fold point (from a reflection, or the nearest point of an excursion) carries
//! the index of the portal it touches and is no portal end (callers that map corners to sheet
//! vertices compare positions).

use crate::geom::predicates::{on_ray, orient, OnRay};
use crate::geom::Vec2;

/// The path corners with the index of a portal each is an end of (the last one before it became
/// a corner), first and last terminal included. See the module documentation.
pub fn funnel(ports: &[(Vec2, Vec2)]) -> Vec<(Vec2, usize)> {
    let n = ports.len();
    if n == 0 {
        return Vec::new();
    }
    let tol = same_point_tol(ports);
    let snapped = snap_shared_ends(ports, tol);
    let ports: &[(Vec2, Vec2)] = snapped.as_deref().unwrap_or(ports);
    let s = ports[0].0;
    // A segment portal that holds the point portal after it up to rounding (a point computed on
    // it, the last terminal on the last portal: within the same-point tolerance, not exactly on
    // it) is touched at that point, which the path passes next: it is skipped. Whether such a
    // point lies in front of the line or behind it is rounding. A point exactly on the portal
    // is left to the exact tie rules.
    let skip: Vec<bool> = (0..n)
        .map(|i| {
            let ((l, r), q) = (ports[i], ports[(i + 1).min(n - 1)].0);
            i > 0 && i + 1 < n && l != r && ports[i + 1].1 == q && !on_segment(q, l, r) && near_segment(q, l, r, tol)
        })
        .collect();
    let mut f = Funnel { ports, tol, skip, apex: (s, 0), mirrors: Vec::new(), chain: [Vec::new(), Vec::new()], head: [0, 0], out: vec![(s, 0)], pend: (s, 0, 1, usize::MAX) };
    for i in 1..n {
        let (l, r) = ports[i];
        if f.skip[i] {
            continue;
        }
        if l != r && f.mirrors.is_empty() && !on_segment(f.apex.0, l, r) && near_segment(f.apex.0, l, r, tol) && !f.pending(i - 1) {
            // The apex lies on this portal up to rounding (a turn vertex or terminal computed on
            // it), and every portal since the apex passes through the apex: touched there, in
            // order. Skipped (and never pending), like a portal through the apex exactly.
            f.skip[i] = true;
            continue;
        }
        if l == r && i >= 2 && ports[i - 1].0 != ports[i - 1].1 && !f.skip[i - 1] && orient(ports[i - 1].1, ports[i - 1].0, l) > 0 {
            // A point behind the line of the portal before it: the plain funnel path to it may
            // still touch that portal (round one of its ends, e.g. a band wrapping a corner by
            // more than 180 degrees); only when it misses the portal is the point reflected.
            let snap = (f.chain.clone(), f.head, f.apex, f.mirrors.clone(), f.out.len());
            f.add_point(l, i);
            if !f.touches_since(snap.4 - 1, i - 1) {
                (f.chain, f.head, f.apex, f.mirrors) = (snap.0, snap.1, snap.2, snap.3);
                f.out.truncate(snap.4);
                if !f.point_behind(i) {
                    f.add_point(l, i);
                }
            }
            continue;
        }
        // A point portal (`l == r`, the terminals too): the right end goes on the right chain,
        // and the left end, the same point, then crosses over that chain up to itself.
        if l == r {
            f.add_point(l, i);
        } else {
            let a = f.apex.0;
            f.add(RIGHT, r, i);
            f.add(LEFT, l, i);
            if r == a && f.apex.0 != a && f.mirrors.is_empty() {
                // The right end was the apex (so not added), and the left end then moved the
                // apex on: from the new apex the right end is a chain end like any other (the
                // path still has to touch this portal; a sleeve never has this).
                f.add(RIGHT, r, i);
            }
        }
    }
    let last = ports[n - 1].0;
    let k = f.out.len();
    match f.out.last_mut() {
        Some(c) if c.1 == n - 1 && c.0 == last => {}
        Some(c) if c.0 == last && f.mirrors.is_empty() && k > 1 => c.1 = n - 1,
        _ => f.commit(last, n - 1),
    }
    f.out
}

/// The point at fraction `t` (0 ..= 1) of the edge `a -> b`, computed from the nearer end: `a`
/// exactly at 0 and `b` exactly at 1, so a portal interval that reaches an edge's end shares that
/// vertex bit for bit with the next portal (the funnel tests ends by equality; `a + (b - a) * 1`
/// can miss `b` by an ulp, and the funnel then sees a wall a few femtometres long, whose
/// direction is rounding noise). O(1).
pub fn edge_point(a: Vec2, b: Vec2, t: f64) -> Vec2 {
    if t <= 0.5 {
        a + (b - a) * t
    } else {
        b + (a - b) * (1.0 - t)
    }
}

/// Unit roundoff of `f64` (half an ulp of 1; Shewchuk's epsilon).
const UNIT_ROUNDOFF: f64 = f64::EPSILON * 0.5;

/// Width of the same-point tolerance in unit roundoffs of the largest coordinate. See
/// [`same_point_tol`].
const SAME_POINT_ROUNDOFFS: f64 = 16.0;

/// The radius (per coordinate) within which two portal ends are one vertex computed twice:
/// `16 u M`, `u` the unit roundoff (2^-53), `M` the largest coordinate magnitude of the input.
///
/// This is a correctness tolerance on the exact equality of shared vertices, not a judgement
/// threshold. Callers compute a portal end on a sheet edge `a b` by interpolation, `a + (b - a) t`
/// (or [`edge_point`]); a vertex shared with the next portal is then the same point computed
/// along two edges, which can come out an ulp or two apart. Each rounded operation errs by at
/// most `u` times the magnitude of its exact result (exact rounding: Shewchuk 1997, sec. 2.1
/// "Background" p. 4, and sec. 2.2 "Properties of Binary Arithmetic" p. 5,
/// `|err(a (*) b)| <= ulp(a (*) b) / 2`; page numbers of the CMU-CS-96-140R version). So one
/// coordinate of `a + (b - a) t` (`0 <= t <= 1`, `t` itself rounded: four operations) errs by
/// at most `3 u |b - a| + u |result| + O(u^2) <= 7 u M` when both edge ends lie within `M`
/// ([`edge_point`]: `t <= 1/2`, `1 - t` exact by Sterbenz's lemma, Lemma 5 p. 6: `<= 3 u M`).
/// `16 u M` covers that with room for an edge end about twice as far out as the portals. At
/// board scale (`M <= 2^30` nm, about 1 m) it is below 2e-6 nm, five orders of magnitude below
/// the 1 nm grid of the board data, so distinct vertices are never merged. The tolerance has to
/// be relative: an absolute one (1e-3 nm was tried) merges genuinely distinct ends of tiny or
/// far-translated features. O(n).
fn same_point_tol(ports: &[(Vec2, Vec2)]) -> f64 {
    let m = ports.iter().flat_map(|&(l, r)| [l.x, l.y, r.x, r.y]).fold(0.0f64, |m, c| m.max(c.abs()));
    SAME_POINT_ROUNDOFFS * UNIT_ROUNDOFF * m
}

/// `a` and `b` are one point within the tolerance `tol` ([`same_point_tol`]), per coordinate.
fn same_point(a: Vec2, b: Vec2, tol: f64) -> bool {
    (a.x - b.x).abs() <= tol && (a.y - b.y).abs() <= tol
}

/// The portals with every end that lies within `tol` of an end of the portal before it (after
/// that one's own snap) replaced by that end, and a portal whose two ends are within `tol` made
/// a point portal: a shared vertex computed along two edges becomes the same `f64` point, which
/// the funnel's exact tests need (an end an ulp off its shared vertex is a wall a few femtometres
/// long whose direction is rounding noise; the path can miss the portal by up to its width).
/// `None` when nothing moves (the common case: callers pass exact shared ends). O(n).
fn snap_shared_ends(ports: &[(Vec2, Vec2)], tol: f64) -> Option<Vec<(Vec2, Vec2)>> {
    if tol == 0.0 {
        return None;
    }
    let mut out: Option<Vec<(Vec2, Vec2)>> = None;
    for i in 1..ports.len() {
        let (pl, pr) = out.as_ref().map_or(ports[i - 1], |o| o[i - 1]);
        let fix = |x: Vec2| -> Vec2 {
            if x == pl || x == pr {
                return x;
            }
            let (nl, nr) = (same_point(x, pl, tol), same_point(x, pr, tol));
            match (nl, nr) {
                (true, true) => if (x - pl).len2() <= (x - pr).len2() { pl } else { pr },
                (true, false) => pl,
                (false, true) => pr,
                _ => x,
            }
        };
        let (l, r) = ports[i];
        let (sl, mut sr) = (fix(l), fix(r));
        if sl != sr && same_point(sl, sr, tol) {
            sr = sl;
        }
        if (sl, sr) != (l, r) || out.is_some() {
            out.get_or_insert_with(|| ports[..i].to_vec()).push((sl, sr));
        }
    }
    out
}

/// `q` lies on the segment `l r` within the same-point tolerance `tol` ([`same_point_tol`]): at
/// an end, or the rounded foot of its perpendicular is `q` (the foot errs by a few unit
/// roundoffs of the coordinates, well inside `tol`). O(1).
fn near_segment(q: Vec2, l: Vec2, r: Vec2, tol: f64) -> bool {
    if same_point(q, l, tol) || same_point(q, r, tol) {
        return true;
    }
    let d = r - l;
    let dd = d.dot(d);
    if dd == 0.0 {
        return false;
    }
    let u = (q - l).dot(d) / dd;
    (0.0..=1.0).contains(&u) && same_point(q, l + d * u, tol)
}

/// The point of the segment `l r` nearest to `t`: the foot of the perpendicular, or the nearer
/// end when the foot falls outside (then exactly that end). A construction: rounded. O(1).
fn nearest_on_segment(t: Vec2, l: Vec2, r: Vec2) -> Vec2 {
    let d = r - l;
    let dd = d.dot(d);
    if dd == 0.0 {
        return l;
    }
    let u = (t - l).dot(d) / dd;
    if u <= 0.0 {
        l
    } else if u >= 1.0 {
        r
    } else {
        edge_point(l, r, u)
    }
}

const LEFT: usize = 0;
const RIGHT: usize = 1;

/// Mirror image of `a` in the line through `p` and `q` (`p != q`). A construction: rounded.
fn reflect(a: Vec2, p: Vec2, q: Vec2) -> Vec2 {
    let d = q - p;
    let t = (a - p).dot(d) / d.dot(d);
    (p + d * t) * 2.0 - a
}

/// Where the segment `a b` meets the line through `p` and `q` (clamped to the segment; `b` when
/// parallel). A construction: rounded.
fn line_hit(a: Vec2, b: Vec2, p: Vec2, q: Vec2) -> Vec2 {
    let d = b - a;
    let e = q - p;
    let den = d.cross(e);
    if den == 0.0 {
        return b;
    }
    a + d * ((p - a).cross(e) / den).clamp(0.0, 1.0)
}

/// `p` on the closed segment `a b` (`a == b` allowed). Exact.
fn on_segment(p: Vec2, a: Vec2, b: Vec2) -> bool {
    p == a || p == b || (a != b && orient(a, b, p) == 0 && on_ray(a, b, p) == OnRay::Between)
}

/// The closed segments `u w` and `a b` meet. Exact: orientation signs; collinear overlaps by
/// [`on_segment`] of the four ends.
fn seg_meets(u: Vec2, w: Vec2, a: Vec2, b: Vec2) -> bool {
    let (o1, o2) = (orient(u, w, a), orient(u, w, b));
    if o1 == 0 && o2 == 0 {
        // Collinear (or `u == w`): they meet where one holds an end of the other.
        return on_segment(a, u, w) || on_segment(b, u, w) || on_segment(u, a, b) || on_segment(w, a, b);
    }
    o1 * o2 <= 0 && orient(a, b, u) * orient(a, b, w) <= 0
}

/// The funnel state of [`funnel`]: the apex and the two chains from it (apex excluded), each a
/// vector whose live part starts at `head` (popping from the apex end only moves `head`).
struct Funnel<'a> {
    ports: &'a [(Vec2, Vec2)],
    /// [`same_point_tol`] of the input: a fold point (a construction) this close to the corner
    /// before or after it is that corner.
    tol: f64,
    /// Portals touched at the point portal after them or at the apex up to rounding (see
    /// [`funnel`]): not processed, and never pending.
    skip: Vec<bool>,
    /// The apex the tests use: the last corner, or its mirror image (see `mirrors`).
    apex: (Vec2, usize),
    /// Reflections applied to the last corner, in order: (line end, line end, portal, the apex
    /// before this reflection). Empty while the apex is the last corner itself.
    mirrors: Vec<(Vec2, Vec2, usize, Vec2)>,
    chain: [Vec<(Vec2, usize)>; 2],
    head: [usize; 2],
    out: Vec<(Vec2, usize)>,
    /// Cache of [`Self::pending`]: (apex, apex portal, next portal to test, first portal that
    /// misses the apex or `usize::MAX`).
    pend: (Vec2, usize, usize, usize),
}

impl Funnel<'_> {
    /// `c` (an end of portal `ci`) is the next corner: emit the fold points of the reflected
    /// apex (last reflection first back to the first), then `c`, and make `c` the apex. O(folds).
    fn commit(&mut self, c: Vec2, ci: usize) {
        if !self.mirrors.is_empty() {
            let mut folds = Vec::with_capacity(self.mirrors.len());
            let (mut to, mut from) = (c, self.apex.0);
            for &(p, q, pi, before) in self.mirrors.iter().rev() {
                let x = line_hit(from, to, p, q);
                folds.push((x, pi));
                to = x;
                from = before;
            }
            for &(x, pi) in folds.iter().rev() {
                if !same_point(x, self.out[self.out.len() - 1].0, self.tol) && !same_point(x, c, self.tol) {
                    self.out.push((x, pi));
                }
            }
            self.mirrors.clear();
        }
        self.out.push((c, ci));
        self.apex = (c, ci);
    }

    /// Point portal `i` behind the line of portal `i - 1` (and the plain path to it misses that
    /// portal): run the funnel to its mirror image in that line and fold the last leg back at
    /// the line; the point is then the apex with empty chains. False (nothing done) when the
    /// rounded image is not in front of the line. O(1) amortised.
    fn point_behind(&mut self, i: usize) -> bool {
        let t = self.ports[i].0;
        let (pl, pr) = self.ports[i - 1];
        let img = reflect(t, pl, pr);
        if orient(pr, pl, img) >= 0 {
            // The rounded image is not in front: `t` lies on the line up to rounding.
            return false;
        }
        self.add(RIGHT, img, i);
        self.add(LEFT, img, i);
        let k = self.out.len();
        if self.apex.0 != img || k < 2 || self.out[k - 1].0 != img {
            // The image did not become a corner (cannot happen for a point portal in a
            // consistent funnel): take the point as it is.
            self.add(RIGHT, t, i);
            self.add(LEFT, t, i);
            return true;
        }
        self.out.pop();
        let prev = self.out[k - 2].0;
        let x = line_hit(prev, img, pl, pr);
        if !same_point(x, prev, self.tol) && !same_point(x, t, self.tol) {
            self.out.push((x, i - 1));
        }
        self.out.push((t, i));
        self.apex = (t, i);
        self.chain = [Vec::new(), Vec::new()];
        self.head = [0, 0];
        true
    }

    /// Adds point portal `t` (index `i`): its right end, then its left end (the same point)
    /// crosses over that chain up to itself; if it ends on the chains instead (an inconsistent
    /// funnel), [`Self::collapse`]. The point is then the apex. O(1) amortised.
    fn add_point(&mut self, t: Vec2, i: usize) {
        if t == self.apex.0 && self.mirrors.is_empty() && self.chain.iter().zip(self.head).any(|(c, h)| c.len() > h) && self.pending(i) {
            // The point is the apex, but portals since the apex that do not pass through it
            // are still to be touched first: the path goes out and comes back to the point.
            let ai = self.apex.1;
            let mut missing = (ai + 1..i).filter(|&j| !self.skip[j] && !on_segment(t, self.ports[j].0, self.ports[j].1));
            if let (Some(j), None) = (missing.next(), missing.next()) {
                // One portal to touch (the others pass through the point, touched there on the
                // way out or back): the shortest excursion is to its nearest point and back,
                // twice the distance from the point to the segment. Exact up to the rounded
                // foot of the perpendicular (a fold point, carrying the portal's index).
                let (l, r) = self.ports[j];
                self.commit(nearest_on_segment(t, l, r), j);
                self.commit(t, i);
                self.chain = [Vec::new(), Vec::new()];
                self.head = [0, 0];
                return;
            }
            // Several: out along the shorter chain (touching those portals at its ends) and
            // back. A heuristic: the shortest closed tour from the point touching the portals
            // in order (Dror, Efrat, Lubiw & Mitchell 2003, the "touring polygons" problem) can
            // touch them between their ends and is not computed here.
            let s = self.shorter_chain();
            let path: Vec<(Vec2, usize)> = self.chain[s][self.head[s]..].to_vec();
            for (q, qi) in path {
                self.commit(q, qi);
            }
            self.commit(t, i);
            self.chain = [Vec::new(), Vec::new()];
            self.head = [0, 0];
            return;
        }
        self.add(RIGHT, t, i);
        self.add(LEFT, t, i);
        if self.apex.0 != t {
            self.collapse(t, i);
        }
    }

    /// Whether a portal after the apex's, up to `k`, misses the apex, i.e. is still to be
    /// touched. Exact. Cached per apex (the first portal that misses it): amortised O(1) per
    /// portal while the apex stays.
    fn pending(&mut self, k: usize) -> bool {
        if !self.mirrors.is_empty() {
            return true;
        }
        let (a, ai) = self.apex;
        if self.pend.0 != a || self.pend.1 != ai {
            self.pend = (a, ai, ai + 1, usize::MAX);
        }
        while self.pend.3 == usize::MAX && self.pend.2 <= k {
            let (l, r) = self.ports[self.pend.2];
            if !self.skip[self.pend.2] && !on_segment(a, l, r) {
                self.pend.3 = self.pend.2;
            }
            self.pend.2 += 1;
        }
        self.pend.3 <= k
    }

    /// Whether the other chain's end `chain[o][k]`, which a new end crosses over, is slack:
    /// every portal since the apex up to the last one that end is an end of passes through the
    /// apex. The end is on the chain only for those portals, which are touched at the apex
    /// already, so it makes no corner (degenerate fans of portals collinear with the apex,
    /// radial edges of a turn vertex); the ends after it on the chain stay and are tested in
    /// turn. The bound is the end's own portal, not the portal before the next chain end: the
    /// portals in between have that next end (or one it popped) as their end on this side, and
    /// the next end, tested after this one, answers for them; with the wider bound an end that
    /// is the chain's last counted the current portal itself, which never passes through the
    /// apex, and made a corner the shortest path does not have (+17.5 mm on the verifier's fuzz).
    /// Checked against the exact reference by fuzz (degenerate fans, terminals on portal ends:
    /// the verifier's modes in scratch/agent-wt/verify-ff2-runs/fz, `zp+E` and the noisy modes)
    /// and the test `slack_is_bounded_by_the_vertex_own_portal`; not proven in general. Exact.
    /// O(1) amortised.
    fn slack(&mut self, o: usize, k: usize) -> bool {
        let ci = self.chain[o][k].1;
        !self.pending(ci)
    }

    /// The side whose chain (from the apex) is shorter; an empty chain does not count. O(chain).
    fn shorter_chain(&self) -> usize {
        let len = |s: usize| -> Option<f64> {
            let c = &self.chain[s][self.head[s]..];
            if c.is_empty() {
                return None;
            }
            let mut prev = self.apex.0;
            Some(c.iter().map(|&(q, _)| {
                let d = (q - prev).len();
                prev = q;
                d
            }).sum())
        };
        match (len(RIGHT), len(LEFT)) {
            (Some(a), Some(b)) => if a <= b { RIGHT } else { LEFT },
            (None, Some(_)) => LEFT,
            _ => RIGHT,
        }
    }

    /// Whether the path from corner `out[k]` on (to the apex) meets portal `j` (its segment,
    /// ends included). Exact (orientation signs and coordinate comparisons). O(corners).
    fn touches_since(&self, k: usize, j: usize) -> bool {
        let (l, r) = self.ports[j];
        self.out[k..].windows(2).any(|w| seg_meets(w[0].0, w[1].0, l, r))
    }

    /// Point portal `t` (index `i`) ended on both chains instead of crossing over one of them
    /// to itself (a funnel inconsistent by an input that is no sleeve): the path runs along the
    /// shorter chain to `t`, whose vertices become corners; `t` is the apex. O(chain).
    fn collapse(&mut self, t: Vec2, i: usize) {
        let len = |f: &Self, s: usize| -> Option<f64> {
            let c = &f.chain[s][f.head[s]..];
            if c.last().map(|x| x.0) != Some(t) {
                return None;
            }
            let mut prev = f.apex.0;
            Some(c.iter().map(|&(q, _)| {
                let d = (q - prev).len();
                prev = q;
                d
            }).sum())
        };
        let s = match (len(self, RIGHT), len(self, LEFT)) {
            (Some(a), Some(b)) => if a <= b { RIGHT } else { LEFT },
            (Some(_), None) => RIGHT,
            (None, Some(_)) => LEFT,
            (None, None) => {
                self.commit(t, i);
                self.chain = [Vec::new(), Vec::new()];
                self.head = [0, 0];
                return;
            }
        };
        let path: Vec<(Vec2, usize)> = self.chain[s][self.head[s]..].to_vec();
        for (q, qi) in path {
            self.commit(q, qi);
        }
        self.chain = [Vec::new(), Vec::new()];
        self.head = [0, 0];
    }

    /// Adds the end `p` of portal `i` on side `s`. O(1) amortised.
    fn add(&mut self, s: usize, p: Vec2, i: usize) {
        // Orientation sign: on the right a tightening end lies left of (or on) the chain edge,
        // on the left right of (or on) it.
        let sg = if s == RIGHT { 1 } else { -1 };
        if p == self.apex.0 {
            return;
        }
        if self.chain[s].len() > self.head[s] {
            let e = self.chain[s].last_mut().expect("non-empty chain");
            if e.0 == p {
                e.1 = i;
                return;
            }
        }
        // Pop the ends of the own chain that `p` sees past.
        while self.chain[s].len() > self.head[s] {
            let k = self.chain[s].len();
            let c = self.chain[s][k - 1].0;
            let b = if k - 1 > self.head[s] { self.chain[s][k - 2].0 } else { self.apex.0 };
            if sg * orient(b, c, p) >= 0 {
                self.chain[s].pop();
            } else {
                break;
            }
        }
        if self.chain[s].len() == self.head[s] {
            // The own chain is empty: `p` may cross over the other chain from the apex on.
            let o = 1 - s;
            while self.head[o] < self.chain[o].len() {
                let a = self.apex.0;
                let (c, ci) = self.chain[o][self.head[o]];
                let cross = match sg * orient(a, c, p) {
                    1 => true,
                    -1 => false,
                    _ => match on_ray(a, c, p) {
                        OnRay::Behind => false,
                        OnRay::AtOrBeyond => true,
                        OnRay::Between => {
                            self.commit(p, i);
                            self.chain[s].clear();
                            self.head[s] = 0;
                            return;
                        }
                    },
                };
                if cross && s == LEFT && p != c && c == self.ports[i].1 && c != self.ports[i - 1].1 && self.head[o] + 1 == self.chain[o].len() && !self.pending(i - 1) {
                    // The apex sees portal i (both ends new, a segment) the wrong way round: it
                    // lies in front of the portal's line, and every portal before it since the
                    // apex passes through the apex (none is to be touched on the way). Mirror
                    // the apex behind the line.
                    let img = reflect(a, p, c);
                    if orient(img, c, p) > 0 && !same_point(img, a, self.tol) {
                        self.mirrors.push((p, c, i, a));
                        self.apex.0 = img;
                        break;
                    }
                    // The apex lies on the portal's line up to rounding (its rounded image is not
                    // behind the line, or is the apex itself within the same-point tolerance:
                    // a portal on an edge radial from a turn vertex, its ends computed along
                    // the edge): the collinear tie, decided along the line as above. The apex
                    // inside the portal touches it there; else the portal's near end is the
                    // first point any path from the apex touches it at, and a corner (`p`
                    // nearer: `p` itself; `c` nearer: `p` crosses over it).
                    match on_ray(a, c, p) {
                        OnRay::Behind => break,
                        OnRay::Between => {
                            self.commit(p, i);
                            self.chain[s].clear();
                            self.head[s] = 0;
                            return;
                        }
                        OnRay::AtOrBeyond => {}
                    }
                }
                if cross {
                    let k = self.head[o];
                    self.head[o] += 1;
                    if !self.slack(o, k) {
                        self.commit(c, ci);
                    }
                } else {
                    break;
                }
            }
            self.chain[s].clear();
            self.head[s] = 0;
            if p == self.apex.0 {
                // Crossed over up to `p` itself (a point portal): `p` is the apex.
                return;
            }
        }
        self.chain[s].push((p, i));
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x: x * 1e6, y: y * 1e6 }
    }

    /// The previous "simple stupid" funnel (apex -> boundary-end tests, restart from the last
    /// corner's portal): a reference where every funnel turns through less than 180 degrees.
    fn funnel_ssf(ports: &[(Vec2, Vec2)]) -> Vec<(Vec2, usize)> {
        let n = ports.len();
        let mut out = vec![(ports[0].0, 0usize)];
        let (mut apex, mut ai) = (ports[0].0, 0usize);
        let (mut lp, mut li, mut rp, mut ri) = (apex, ai, apex, ai);
        let mut i = 1;
        while i < n {
            let (l, r) = ports[i];
            if (rp - apex).cross(r - apex) >= 0.0 {
                if apex == rp || (lp - apex).cross(r - apex) < 0.0 {
                    (rp, ri) = (r, i);
                } else {
                    if lp != apex {
                        out.push((lp, li));
                    }
                    (apex, ai) = (lp, li);
                    (lp, li, rp, ri) = (apex, ai, apex, ai);
                    i = ai + 1;
                    continue;
                }
            }
            if (lp - apex).cross(l - apex) <= 0.0 {
                if apex == lp || (rp - apex).cross(l - apex) > 0.0 {
                    (lp, li) = (l, i);
                } else {
                    if rp != apex {
                        out.push((rp, ri));
                    }
                    (apex, ai) = (rp, ri);
                    (lp, li, rp, ri) = (apex, ai, apex, ai);
                    i = ai + 1;
                    continue;
                }
            }
            i += 1;
        }
        if out.last().is_none_or(|c| c.1 != n - 1) {
            out.push((ports[n - 1].0, n - 1));
        }
        out
    }

    fn pts(c: &[(Vec2, usize)]) -> Vec<(f64, f64)> {
        c.iter().map(|(p, _)| ((p.x / 1e3).round() / 1e3, (p.y / 1e3).round() / 1e3)).collect()
    }

    #[test]
    fn straight_and_single_bends() {
        // Straight: no corner.
        let a = v(0.0, 0.0);
        let b = v(10.0, 0.0);
        let ports = vec![(a, a), (v(3.0, 1.0), v(3.0, -1.0)), (v(6.0, 1.0), v(6.0, -1.0)), (b, b)];
        assert_eq!(pts(&funnel(&ports)), vec![(0.0, 0.0), (10.0, 0.0)]);
        // A left bend round (5, 1): the target is up-left of the bend.
        let ports = vec![(a, a), (v(5.0, 1.0), v(5.0, -1.0)), (v(5.0, 1.0), v(7.0, -1.0)), (v(5.0, 1.0), v(7.0, 3.0)), (v(4.0, 6.0), v(4.0, 6.0))];
        let f = funnel(&ports);
        assert_eq!(pts(&f), vec![(0.0, 0.0), (5.0, 1.0), (4.0, 6.0)]);
        assert_eq!(f[1].1, 2, "the portal the corner was last an end of before it became one");
        // Mirrored (y -> -y, left and right swapped): a right bend round (5, -1).
        let m = |p: Vec2| Vec2 { x: p.x, y: -p.y };
        let mp: Vec<_> = ports.iter().map(|&(l, r)| (m(r), m(l))).collect();
        assert_eq!(pts(&funnel(&mp)), vec![(0.0, 0.0), (5.0, -1.0), (4.0, -6.0)]);
    }

    /// A point portal in the middle (a turn the path must pass) is a corner.
    #[test]
    fn point_portal_is_a_corner() {
        let a = v(0.0, 0.0);
        let t = v(5.0, 0.5);
        let ports = vec![(a, a), (v(3.0, 2.0), v(3.0, -2.0)), (t, t), (v(7.0, 2.0), v(7.0, -2.0)), (v(10.0, 0.0), v(10.0, 0.0))];
        let f = funnel(&ports);
        assert_eq!(pts(&f), vec![(0.0, 0.0), (5.0, 0.5), (10.0, 0.0)]);
        assert_eq!(f.iter().map(|c| c.1).collect::<Vec<_>>(), vec![0, 2, 4]);
    }

    /// A hairpin: up the left of a wall (x = 0, y 0..10), round its top (0, 10) on the right,
    /// down between it and a wall hanging from above (x = 4, y 2..20), round that wall's
    /// bottom (4, 2) on the left and up to the target. After the corner (0, 10) the funnel
    /// opens past 180 degrees; both corners are on the path (the left-side mirror of the
    /// `cutflow` S bundle, where the fault was on the right).
    #[test]
    fn hairpin_and_back_up() {
        let s = v(-1.0, 0.0);
        let e = v(6.0, 12.0);
        let (top, bot) = (v(0.0, 10.0), v(4.0, 2.0));
        let ports = vec![
            (s, s),
            (v(-3.0, 5.0), v(0.0, 5.0)),
            (v(-3.0, 11.0), top),
            (v(2.0, 13.0), top),
            (v(4.0, 8.0), top),
            (bot, top),
            (bot, v(0.0, 0.0)),
            (bot, v(6.0, 0.0)),
            (bot, v(8.0, 6.0)),
            (e, e),
        ];
        let f = funnel(&ports);
        assert_eq!(pts(&f), vec![(-1.0, 0.0), (0.0, 10.0), (4.0, 2.0), (6.0, 12.0)]);
        assert_eq!(f, funnel_ssf(&ports));
    }

    /// The `cutflow` S bundle's fault in miniature: the path comes down from the north-west,
    /// wraps (5, 2) on its left, runs east along (5, 2) -> (6, 2) and turns up to the end
    /// (6.9, 4.4). After the corner (5, 2) the right boundary end is (3, 0), south-west of it,
    /// and the end lies north-east: more than 180 degrees round. The previous funnel read that
    /// as widening on the right and went (5, 2) -> end straight, which does not even cross the
    /// last portal (6, 2) - (3, 0). Mirrored (x -> -x, sides swapped): the same on the left.
    #[test]
    fn funnel_past_180_degrees_both_sides() {
        let (s, e) = (v(3.2, 3.3), v(6.9, 4.4));
        let ports = vec![(s, s), (v(5.0, 2.0), v(3.0, 0.0)), (v(6.0, 2.0), v(3.0, 0.0)), (e, e)];
        let want = vec![(3.2, 3.3), (5.0, 2.0), (6.0, 2.0), (6.9, 4.4)];
        assert_eq!(pts(&funnel(&ports)), want);
        assert_eq!(pts(&funnel_ssf(&ports)), vec![(3.2, 3.3), (5.0, 2.0), (6.9, 4.4)], "the previous fault");
        let m = |p: Vec2| Vec2 { x: -p.x, y: p.y };
        let mp: Vec<_> = ports.iter().map(|&(l, r)| (m(r), m(l))).collect();
        let mw: Vec<_> = want.iter().map(|&(x, y)| (-x, y)).collect();
        assert_eq!(pts(&funnel(&mp)), mw);
        assert_eq!(pts(&funnel_ssf(&mp)).len(), 3, "the previous fault, left side");
    }

    /// On random gently bending corridors (total turn under 150 degrees, so no funnel opens past
    /// 180) the funnel and the previous one agree, corners and portal indices.
    #[test]
    fn agrees_with_simple_funnel_on_gentle_corridors() {
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut rnd = move || {
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
        };
        for case in 0..400 {
            let n = 2 + case % 17;
            let mut c = v(0.0, 0.0);
            let mut ang = 0.0f64;
            let mut ports = vec![(c, c)];
            for _ in 0..n {
                ang += (rnd() - 0.5) * 0.3;
                let d = Vec2 { x: ang.cos(), y: ang.sin() };
                c = c + d * (1e6 * (0.5 + rnd()));
                let nrm = d.perp();
                let (wl, wr) = (1e6 * (0.1 + rnd()), 1e6 * (0.1 + rnd()));
                ports.push((c + nrm * wl, c - nrm * wr));
            }
            c = c + Vec2 { x: ang.cos(), y: ang.sin() } * 1e6;
            ports.push((c, c));
            assert_eq!(funnel(&ports), funnel_ssf(&ports), "case {case}: {ports:?}");
        }
    }

    /// Whether the polyline `pts` touches every portal of `ports` in order (within 1 nm): the
    /// verifier's check (scratch/agent-wt/wt-funnel-runs/verify). O(points + portals).
    fn touches_in_order(pts: &[Vec2], ports: &[(Vec2, Vec2)]) -> Result<(), usize> {
        // (distance, parameter on p->q of the closest point, smallest among near ties)
        fn closest(p: Vec2, q: Vec2, a: Vec2, b: Vec2) -> (f64, f64) {
            let (d, e) = (q - p, b - a);
            let den = d.cross(e);
            if den.abs() > 1e-9 * (d.len() * e.len()).max(1e-30) {
                let (t, u) = ((a - p).cross(e) / den, (a - p).cross(d) / den);
                if (-1e-12..=1.0 + 1e-12).contains(&t) && (-1e-12..=1.0 + 1e-12).contains(&u) {
                    return (0.0, t.clamp(0.0, 1.0));
                }
            }
            let ps = |x: Vec2, s0: Vec2, s1: Vec2| {
                let w = s1 - s0;
                let u = if w.len2() > 0.0 { ((x - s0).dot(w) / w.len2()).clamp(0.0, 1.0) } else { 0.0 };
                ((s0 + w * u - x).len(), u)
            };
            let mut best = (f64::INFINITY, 0.0);
            for c in [(ps(p, a, b).0, 0.0), ps(a, p, q), ps(b, p, q), (ps(q, a, b).0, 1.0)] {
                if c.0 < best.0 - 1e-3 || (c.0 <= best.0 + 1e-3 && c.1 < best.1) {
                    best = c;
                }
            }
            best
        }
        let (mut seg, mut t) = (0usize, 0.0f64);
        for (k, &(l, r)) in ports.iter().enumerate() {
            let mut ok = false;
            while seg + 1 < pts.len() {
                let (d, tt) = closest(pts[seg], pts[seg + 1], l, r);
                if d <= 1.0 && tt >= t - 1e-9 {
                    t = tt;
                    ok = true;
                    break;
                }
                if d <= 1.0 && closest(pts[seg + 1], pts[seg + 1], l, r).0 <= 1.0 {
                    t = 1.0;
                    ok = true;
                    break;
                }
                seg += 1;
                t = 0.0;
            }
            if !ok {
                return Err(k);
            }
        }
        Ok(())
    }

    fn plen(c: &[(Vec2, usize)]) -> f64 {
        c.windows(2).map(|w| (w[1].0 - w[0].0).len()).sum()
    }

    /// Tie on the own chain: a portal end exactly on the line of its chain's last edge pops
    /// that edge's end (the path runs straight through it, no corner there). The right end
    /// (4, -2) lies on the line from the start through (2, -1).
    #[test]
    fn tie_own_chain_pops() {
        let s = v(0.0, 0.0);
        let e = v(6.0, -6.0);
        let ports = vec![(s, s), (v(2.0, 1.0), v(2.0, -1.0)), (v(4.0, 1.0), v(4.0, -2.0)), (e, e)];
        let f = funnel(&ports);
        assert_eq!(pts(&f), vec![(0.0, 0.0), (4.0, -2.0), (6.0, -6.0)]);
        assert_eq!(f.iter().map(|c| c.1).collect::<Vec<_>>(), vec![0, 2, 3]);
        // Mirrored (y -> -y, sides swapped): the left side's tie.
        let m = |p: Vec2| Vec2 { x: p.x, y: -p.y };
        let mp: Vec<_> = ports.iter().map(|&(l, r)| (m(r), m(l))).collect();
        assert_eq!(pts(&funnel(&mp)), vec![(0.0, 0.0), (4.0, 2.0), (6.0, 6.0)]);
    }

    /// Ties with the other chain's first edge (apex `a`, its end `c`, the new end `p` on one
    /// line): `p` at or beyond `c` crosses over it (a corner without a turn, which `cutflow`
    /// counts); `p` on the far side of `a` (the start lies on its first portal, between the
    /// ends: pair58 cutflow, +134 mm when it crossed) does not; `p` between `a` and `c` (a
    /// portal on a ray from the apex) is a corner itself.
    #[test]
    fn ties_with_the_other_chain() {
        // At or beyond: the end (6, 3) on the line from the start through the left end (2, 1).
        let s = v(0.0, 0.0);
        let ports = vec![(s, s), (v(2.0, 1.0), v(2.0, -1.0)), (v(6.0, 3.0), v(6.0, 3.0))];
        let f = funnel(&ports);
        assert_eq!(pts(&f), vec![(0.0, 0.0), (2.0, 1.0), (6.0, 3.0)]);
        assert_eq!(f.iter().map(|c| c.1).collect::<Vec<_>>(), vec![0, 1, 2]);
        // Behind: the start on its first portal, (0, 0) between (0, 2) and (0, -3).
        let ports = vec![(s, s), (v(0.0, 2.0), v(0.0, -3.0)), (v(3.0, 2.0), v(3.0, -2.0)), (v(6.0, 1.0), v(6.0, 1.0))];
        assert_eq!(pts(&funnel(&ports)), vec![(0.0, 0.0), (6.0, 1.0)]);
        // Mirrored: the left end first on the far side.
        let m = |p: Vec2| Vec2 { x: p.x, y: -p.y };
        let mp: Vec<_> = ports.iter().map(|&(l, r)| (m(r), m(l))).collect();
        assert_eq!(pts(&funnel(&mp)), vec![(0.0, 0.0), (6.0, -1.0)]);
        // Between: a turn vertex (0, 0), then a portal on the ray from it, (1, 0) near, (4, 0)
        // far; the path goes out to the near end and on to the target up-left.
        let t = v(0.0, 0.0);
        let ports = vec![(v(-3.0, -1.0), v(-3.0, -1.0)), (t, t), (v(1.0, 0.0), v(4.0, 0.0)), (v(-2.0, 3.0), v(-2.0, 3.0))];
        let f = funnel(&ports);
        assert_eq!(pts(&f), vec![(-3.0, -1.0), (0.0, 0.0), (1.0, 0.0), (-2.0, 3.0)]);
        assert_eq!(f.iter().map(|c| c.1).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
        // The same with the near end on the right.
        let ports = vec![(v(-3.0, 1.0), v(-3.0, 1.0)), (t, t), (v(4.0, 0.0), v(1.0, 0.0)), (v(-2.0, -3.0), v(-2.0, -3.0))];
        assert_eq!(pts(&funnel(&ports)), vec![(-3.0, 1.0), (0.0, 0.0), (1.0, 0.0), (-2.0, -3.0)]);
    }

    /// A drawing run that starts in front of its first portal (vias54: the via (8, -7) east of
    /// the portal x = 3.4): the path goes back to touch it at the end the rest of the path
    /// needs, (3.4, -11.6), not the other one (+0.86 mm). A start just in front of a long
    /// portal touches it at the foot of the perpendicular (a fold point, no portal end).
    #[test]
    fn start_in_front_of_the_first_portal() {
        let s = v(8.0, -7.0);
        let e = v(17.0, -7.0);
        let ports = vec![(s, s), (v(3.4, -11.6), v(3.4, -12.4)), (v(3.4, -11.6), v(16.6, -7.4)), (v(3.4, -2.4), v(16.6, -7.4)), (v(16.6, -6.6), v(16.6, -7.4)), (e, e)];
        let f = funnel(&ports);
        assert_eq!(pts(&f), vec![(8.0, -7.0), (3.4, -11.6), (17.0, -7.0)]);
        // (3.4, -11.6) became a corner while portal 2's right end was added: index 1, the last
        // portal it was an end of before it became one.
        assert_eq!(f.iter().map(|c| c.1).collect::<Vec<_>>(), vec![0, 1, 5]);
        // 0.1 in front of the portal y = 0 (travel -y): fold at (1, 0), then down to (1, -4).
        let s = v(1.0, -0.1);
        let ports = vec![(s, s), (v(5.0, 0.0), v(-5.0, 0.0)), (v(5.0, -2.0), v(-5.0, -2.0)), (v(1.0, -4.0), v(1.0, -4.0))];
        let f = funnel(&ports);
        assert_eq!(pts(&f), vec![(1.0, -0.1), (1.0, 0.0), (1.0, -4.0)]);
        assert_eq!(f[1].1, 1, "the fold point carries the portal it touches");
        let pp: Vec<Vec2> = f.iter().map(|c| c.0).collect();
        assert_eq!(touches_in_order(&pp, &ports), Ok(()));
    }

    /// An end pad behind the last pass (pick_shorts pack: the pad (102.6325, -52.0218) is the
    /// right end of the passes before, the last pass lies 0.25 mm beyond it): the path goes out
    /// to touch the last portal and comes back; before, it stopped at the pad (infeasible).
    #[test]
    fn end_behind_the_last_portal() {
        let e = v(0.0, 0.0);
        let s = v(-4.0, 0.0);
        let ports = vec![(s, s), (v(-2.0, 1.0), v(-2.0, -1.0)), (v(1.0, 1.0), v(1.0, -1.0)), (e, e)];
        let f = funnel(&ports);
        assert_eq!(pts(&f), vec![(-4.0, 0.0), (1.0, 0.0), (0.0, 0.0)]);
        assert_eq!(f.iter().map(|c| c.1).collect::<Vec<_>>(), vec![0, 2, 3]);
        // Beyond the portal's end: the fold is that end.
        let ports = vec![(s, s), (v(-2.0, 1.0), v(-2.0, -1.0)), (v(1.0, -1.0), v(1.0, -3.0)), (e, e)];
        let f = funnel(&ports);
        assert_eq!(pts(&f), vec![(-4.0, 0.0), (1.0, -1.0), (0.0, 0.0)]);
    }

    /// The real portal sequences on which the chain funnel of 074537a was worse than the simple
    /// funnel (or both failed), from the engine integration tests (verifier audit,
    /// scratch/agent-wt/wt-funnel-runs/verify/py/cases_*.txt): each path touches every portal in
    /// order and is no longer than the shortest such path (exact reference: visibility DAG over
    /// the portal ends, verifier). Columns: test, caller, reference length, old length (`inf`:
    /// infeasible), portal count, portals.
    #[test]
    fn real_cases_touch_every_portal_and_are_shortest() {
        let mut n = 0;
        for line in include_str!("funnel_cases.txt").lines() {
            let w: Vec<&str> = line.split(' ').collect();
            let refl: f64 = w[2].parse().unwrap();
            let k: usize = w[4].parse().unwrap();
            let c: Vec<f64> = w[5..].iter().map(|x| x.parse().unwrap()).collect();
            assert_eq!(c.len(), 4 * k);
            let ports: Vec<(Vec2, Vec2)> = c.chunks(4).map(|q| (Vec2 { x: q[0], y: q[1] }, Vec2 { x: q[2], y: q[3] })).collect();
            let f = funnel(&ports);
            let pp: Vec<Vec2> = f.iter().map(|q| q.0).collect();
            assert_eq!(touches_in_order(&pp, &ports), Ok(()), "{} {}: {:?}", w[0], w[1], pts(&f));
            assert!(plen(&f) <= refl + 10.0, "{} {}: {} > reference {}", w[0], w[1], plen(&f), refl);
            n += 1;
        }
        assert_eq!(n, 32);
    }

    /// A point portal equal to the apex while a portal since the apex misses it (a turn vertex
    /// passed twice around a portal radial-ish to it): out to touch the portal and back. With
    /// one such portal the excursion goes to the portal's nearest point, the foot of the
    /// perpendicular (0, 2): 6.83 mm in all, the shortest (out along a chain to (1, 2) was 7.30).
    /// The plain path keeps this (it touches the portal before the point), so the point is not
    /// reflected although it lies behind it.
    #[test]
    fn point_equal_to_the_apex_goes_out_and_back() {
        let (s, t, e) = (v(-1.0, -1.0), v(0.0, 0.0), v(1.0, -1.0));
        let ports = vec![(s, s), (t, t), (v(-1.0, 2.0), v(1.0, 2.0)), (t, t), (e, e)];
        let f = funnel(&ports);
        assert_eq!(pts(&f), vec![(-1.0, -1.0), (0.0, 0.0), (0.0, 2.0), (0.0, 0.0), (1.0, -1.0)]);
        assert_eq!(f.iter().map(|c| c.1).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4], "the fold point carries the portal it touches");
        assert!((plen(&f) - (2.0 * 2f64.sqrt() + 4.0) * 1e6).abs() < 1.0, "{}", plen(&f));
        // The foot outside the portal: its nearer end.
        let ports = vec![(s, s), (t, t), (v(1.0, 2.0), v(3.0, 2.0)), (t, t), (e, e)];
        assert_eq!(pts(&funnel(&ports)), vec![(-1.0, -1.0), (0.0, 0.0), (1.0, 2.0), (0.0, 0.0), (1.0, -1.0)]);
        // A portal through the point on the way out does not count (touched at the point).
        let ports = vec![(s, s), (t, t), (v(0.0, -2.0), v(0.0, 2.0)), (v(-1.0, 2.0), v(2.0, 2.0)), (t, t), (e, e)];
        let f = funnel(&ports);
        assert_eq!(pts(&f), vec![(-1.0, -1.0), (0.0, 0.0), (0.0, 2.0), (0.0, 0.0), (1.0, -1.0)]);
        assert_eq!(touches_in_order(&f.iter().map(|c| c.0).collect::<Vec<_>>(), &ports), Ok(()));
    }

    /// HEURISTIC, pinned: two portals to touch before the point comes again. The path goes out
    /// along the shorter chain (touching the portals at its ends, here only the end (-1, 3),
    /// on the way past the first) and back; it touches every portal in order, but the shortest
    /// tour runs straight up and back, (0, 2) and (0, 3), 6 mm from the point, where this one
    /// takes 2 sqrt(10) = 6.32 mm (the "touring polygons" problem; not solved here, see the
    /// module documentation).
    #[test]
    fn point_equal_to_the_apex_with_several_portals_is_a_heuristic() {
        let (s, t, e) = (v(-1.0, -1.0), v(0.0, 0.0), v(1.0, -1.0));
        let ports = vec![(s, s), (t, t), (v(-1.0, 2.0), v(1.0, 2.0)), (v(-1.0, 3.0), v(2.0, 3.0)), (t, t), (e, e)];
        let f = funnel(&ports);
        assert_eq!(touches_in_order(&f.iter().map(|c| c.0).collect::<Vec<_>>(), &ports), Ok(()));
        assert_eq!(pts(&f), vec![(-1.0, -1.0), (0.0, 0.0), (-1.0, 3.0), (0.0, 0.0), (1.0, -1.0)]);
    }

    /// The slack rule's bound is the crossed vertex's own last portal (the verifier's minimal
    /// case of 3419093/b197f5b): the start lies left of the portal x = 0, the end on the next
    /// portal near their shared end (0, -1). With the bound at the current portal the shared
    /// end (0, -1), the chain's last vertex, counted portal 2 (which never passes through the
    /// apex) and the path took a corner the shortest one does not have.
    #[test]
    fn slack_is_bounded_by_the_vertex_own_portal() {
        let (s, e) = (v(-1.831, -0.221), v(0.0324, -1.1155));
        let ports = vec![(s, s), (v(0.0, 1.0), v(0.0, -1.0)), (v(0.267, -1.952), v(0.0, -1.0)), (e, e)];
        let f = funnel(&ports);
        let pp: Vec<Vec2> = f.iter().map(|c| c.0).collect();
        assert_eq!(touches_in_order(&pp, &ports), Ok(()), "{:?}", pts(&f));
        assert!(plen(&f) <= reference(&ports) + 10.0, "{} > {}: {:?}", plen(&f), reference(&ports), pts(&f));
    }

    /// Ends an ulp off the vertex they share with the portal before (margins computed as
    /// `l + (r - l) * (1 - m)`, the noisy fuzz mode `zn`): 617056.8429656076 vs ...075 and
    /// -141014.76777668286 vs ...297. Snapped on entry they are one vertex; before, the exact
    /// tests saw a wall a few femtometres long and the path missed portals by up to 1.9 mm.
    #[test]
    fn ends_an_ulp_off_a_shared_vertex_are_that_vertex() {
        let c: &[[f64; 4]] = &[
            [-1897105.5578628038, -327237.5983770483, -1897105.5578628038, -327237.5983770483],
            [0.0, 1000000.0, 0.0, -1000000.0],
            [0.0, 1000000.0, 617056.8429656076, -388797.37397834193],
            [2691909.8675892516, 819060.4142728453, 617056.8429656075, -388797.37397834193],
            [2691909.8675892516, 819060.4142728453, 4763726.6761682555, -141014.76777668286],
            [5148740.002985071, 1659796.8945087874, 4763726.6761682555, -141014.76777668297],
            [5879111.632609725, -1699653.1500827018, 4763726.6761682555, -141014.76777668297],
            [4360343.280447775, -1446746.0026910503, 4360343.280447775, -1446746.0026910503],
        ];
        let ports: Vec<(Vec2, Vec2)> = c.iter().map(|q| (Vec2 { x: q[0], y: q[1] }, Vec2 { x: q[2], y: q[3] })).collect();
        let f = funnel(&ports);
        let pp: Vec<Vec2> = f.iter().map(|c| c.0).collect();
        assert_eq!(touches_in_order(&pp, &ports), Ok(()), "{:?}", pts(&f));
        assert!(plen(&f) <= reference(&ports) + 10.0, "{} > {}", plen(&f), reference(&ports));
        // The tolerance is relative (16 unit roundoffs of the largest coordinate): ends 1e-4 nm
        // apart near the origin stay distinct, as they do 1 nm apart at 1e9 nm.
        let ports = [(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 0.0, y: 0.0 }), (Vec2 { x: 1e-3, y: 1e-4 }, Vec2 { x: 1e-3, y: 0.0 }), (Vec2 { x: 1e-3, y: 1e-4 }, Vec2 { x: 2e-3, y: 1e-4 }), (Vec2 { x: 3e-3, y: 0.0 }, Vec2 { x: 3e-3, y: 0.0 })];
        assert!(snap_shared_ends(&ports, same_point_tol(&ports)).is_none());
        let (a, b) = (Vec2 { x: 1e9, y: 0.0 }, Vec2 { x: 1e9 + 1.0, y: 0.0 });
        assert!(!same_point(a, b, same_point_tol(&[(a, b)])));
        let b = Vec2 { x: 1e9 + 2.0 * f64::EPSILON * 1e9, y: 0.0 };
        assert!(same_point(a, b, same_point_tol(&[(a, b)])), "two ulps at 1e9 nm are one point");
    }

    /// Incidences that hold only up to rounding (the verifier's fuzz, exact coordinates in nm):
    /// a point computed on a portal and a portal on a ray from the apex. Each path touches every
    /// portal in order and is no longer than the exact reference.
    ///
    /// * `gp+E` 2660, `gp+M` 1190: the point after portal 3 lies on it up to rounding (the second
    ///   a 1-ulp portal, a point after the snap), and portal 3 lies on a ray from the turn vertex
    ///   (0, -1) (its ends computed along an edge from it). The point came out a hair behind the
    ///   portal and the path ran out to the portal's far end and back.
    /// * `og+E` 2044: the same with the start as the turn vertex and the end on the portal.
    /// * `zp+ERMK` 52 (translated to 1e9 nm, features of microns): the turn vertex 17 lies on
    ///   portals 16, 18 and 19 (one edge, repeated) up to rounding; portal 19, two steps on, took
    ///   the path out to its far end (+32 um).
    #[test]
    fn incidences_up_to_rounding_touch_at_the_point() {
        let cases: [&[[f64; 4]]; 4] = [
            &[[-1043886.2458930372, 347894.9575325762, -1043886.2458930372, 347894.9575325762], [0.0, 648254.927638604, 0.0, -198291.88089640578], [0.0, -1000000.0, 0.0, -1000000.0], [1716765.872748864, -2164643.9739124295, 936018.530719476, -1634989.5222039241], [1649727.2672907312, -2119165.374235181, 1649727.2672907312, -2119165.374235181]],
            &[[-1727838.2370644263, 566675.2768290214, -1727838.2370644263, 566675.2768290214], [0.0, 791912.5951591912, 0.0, -448785.6198602973], [0.0, -1000000.0, 0.0, -1000000.0], [757493.4408147834, 263612.35691007285, 158156.70299451635, -736170.9640454924], [637853.8048172364, 64035.54967051215, 637853.8048172365, 64035.54967051215], [783586.2980309145, -521808.9345213905, 783586.2980309145, -521808.9345213905]],
            &[[0.0, -1000000.0, 0.0, -1000000.0], [0.0, 779537.0566630729, 0.0, -1000000.0], [1389759.9976154664, -1822959.117094301, 587006.3705744924, -1347601.2011322682], [814936.2262949694, -1482571.9537406303, 814936.2262949694, -1482571.9537406303]],
            &[[1000000486.8188535, -1000000209.9251509, 1000000486.8188535, -1000000209.9251509], [1000000486.8188535, -999998929.156047, 1000000486.8188535, -1000000929.156047], [1000003362.3375208, -1000001773.1866497, 1000000486.8188535, -1000000929.156047], [999999416.0881014, -1000003338.8616737, 1000000486.8188535, -1000000929.156047], [999999416.0881014, -1000003338.8616737, 999999416.0881014, -1000003338.8616737], [999999416.0881014, -1000003338.8616737, 999997286.2394568, -1000001259.2385921], [999997752.9284766, -1000001714.9222671, 999997752.9284766, -1000001714.9222671], [999999416.0881014, -1000003338.8616737, 999995686.8734555, -1000005205.6052376], [999999416.0881014, -1000003338.8616737, 999995236.9541427, -1000008070.1998407], [999999416.0881014, -1000003338.8616737, 1000005593.6149997, -1000008905.70076], [999999416.0881014, -1000003338.8616737, 1000007358.9344044, -999995694.4569145], [999999416.0881014, -1000003338.8616737, 1000007358.9344044, -999995694.4569145], [999999416.0881014, -1000003338.8616737, 1000007358.9344044, -999995694.4569145], [999999416.0881014, -1000003338.8616737, 999994345.0664309, -999989206.4859796], [999999416.0881014, -1000003338.8616737, 999994345.0664309, -999989206.4859796], [999995153.7771331, -999991460.2731279, 999995153.7771331, -999991460.2731279], [999977681.2516439, -999972313.9209001, 999994345.0664309, -999989206.4859796], [999982233.03565, -999976928.1889132, 999982233.03565, -999976928.1889132], [999977681.2516439, -999972313.9209001, 999994345.0664309, -999989206.4859796], [999977681.2516439, -999972313.9209001, 999994345.0664309, -999989206.4859796], [999977681.2516439, -999972313.9209001, 999977791.6849725, -999961247.9042557], [999977681.2516439, -999972313.9209001, 999965572.1355872, -999967373.9138116], [999968845.3715855, -999969100.0398964, 999968845.3715855, -999969100.0398964]],
        ];
        for (k, c) in cases.iter().enumerate() {
            let ports: Vec<(Vec2, Vec2)> = c.iter().map(|q| (Vec2 { x: q[0], y: q[1] }, Vec2 { x: q[2], y: q[3] })).collect();
            let f = funnel(&ports);
            let pp: Vec<Vec2> = f.iter().map(|q| q.0).collect();
            assert_eq!(touches_in_order(&pp, &ports), Ok(()), "case {k}: {:?}", pts(&f));
            assert!(plen(&f) <= reference(&ports) + 10.0, "case {k}: {} > {}: {:?}", plen(&f), reference(&ports), pts(&f));
        }
    }

    /// One fuzz witness per rounding rule of the funnel (exact coordinates, nm; the fuzz
    /// tolerance `eps` and its exact reference length): each case breaks when its rule is
    /// removed. The funnel touches every portal in order and is no longer than the reference
    /// (by more than the fuzz slack 10 eps).
    #[test]
    fn each_rounding_rule_has_a_witness() {
        let cases: [(f64, f64, &[[f64; 4]]); 7] = [
            // a portal narrower than the tolerance is a point portal (snap collapse); also a portal
            // that holds the next point up to rounding (og+M case 911)
            (1.0, 3232327.403433013, &[[-1282131.187136807, 499834.4988346992, -1282131.187136807, 499834.4988346992], [0.0, 1000000.0, 0.0, -553048.5207174507], [428589.18558757537, 1335751.6481830182, 693246.5088932188, 1543081.0337384518], [550748.6153896323, 1431449.8862076392, 550748.6153896324, 1431449.8862076392], [-443013.38616109604, 2119753.305398148, 1009594.2143249944, 1790904.04717268], [743990.6107618841, 2571658.297919471, 743990.6107618841, 2571658.297919471]]),
            // an apex on the line of a portal seen the wrong way round: the collinear tie along that line (gpn+ERMK case 929)
            (1.6474125742912293e-05, 43.78736757110438, &[[1000000057.6339487, -999999861.9437298, 1000000057.6339487, -999999861.9437298], [1000000057.6339487, -999999853.8678572, 1000000057.6339487, -999999861.9437298], [1000000057.6339487, -999999861.3490999, 1000000057.6339487, -999999861.3490999], [1000000057.6339487, -999999844.8749741, 1000000057.6339487, -999999844.8749741], [1000000059.8586972, -999999841.5110543, 1000000061.4451718, -999999839.1122333], [1000000063.9141473, -999999835.3790314, 1000000063.9141473, -999999835.3790314], [1000000056.7104883, -999999841.8478053, 1000000061.6812856, -999999837.3841065], [1000000056.7104883, -999999841.8478053, 1000000061.6812856, -999999837.3841065], [1000000060.9484899, -999999838.0421457, 1000000060.9484899, -999999838.0421457], [1000000056.7104883, -999999841.8478053, 1000000061.6812856, -999999837.3841065], [1000000055.9079071, -999999836.730714, 1000000062.5164444, -999999835.6150037], [1000000055.9079071, -999999836.730714, 1000000062.5164444, -999999835.6150037], [1000000055.9079071, -999999836.730714, 1000000062.5164444, -999999835.6150037], [1000000068.0631971, -999999830.6888978, 1000000064.5871001, -999999834.6183178], [1000000066.6919932, -999999840.6839024, 1000000065.2909433, -999999838.0083078], [1000000059.0035495, -999999842.6488429, 1000000063.5029362, -999999835.9878018], [1000000063.5029362, -999999835.9878018, 1000000063.5029362, -999999835.9878018]]),
            // a mirror image within the same-point tolerance of the apex is no mirror (the collinear tie) (gpn+ERMK case 715)
            (0.0026287976929679577, 6113.565007058715, &[[1000000555.771123, -999999988.249697, 1000000555.771123, -999999988.249697], [1000000555.771123, -999999595.7646663, 1000000555.771123, -999999988.249697], [1000001244.4825562, -1000002132.7419765, 1000000863.3907655, -1000001425.437555], [1000001004.7771262, -1000001687.8499198, 1000001004.7771262, -1000001687.8499198], [1000001244.4825562, -1000002132.7419765, 1000000863.3907655, -1000001425.437555], [1000001446.0828388, -1000002506.9110736, 1000001446.0828388, -1000002506.9110736], [1000001446.0828388, -1000002506.9110736, 1000001446.0828388, -1000002506.9110736], [1000001015.2485651, -1000002738.7865442, 999999147.7936507, -1000003743.852793], [1000001015.2485651, -1000002738.7865442, 999999147.7936507, -1000003743.852793], [1000001015.2485651, -1000002738.7865442, 999999147.7936507, -1000003743.852793], [999999512.0080436, -1000003547.8322139, 999999512.0080436, -1000003547.8322139], [999999936.8150445, -1000004700.9464672, 999999936.8150445, -1000004700.9464672]]),
            // a portal that holds the next point up to rounding is touched at that point (thinm+E case 22)
            (0.09999999999999999, 438861.5737173551, &[[-49167.58153153888, -75714.73626541981, -49167.58153153888, -75714.73626541981], [-4.193519064074896, 2.722939194921622, 4.193519064074896, -2.722939194921622], [504024.4116775552, 769946.6913947258, 4224.316473270289, 6444.0049497773025], [190923.16011330977, 291648.1696610158, 190923.16011330977, 291648.1696610158]]),
            // a portal that holds the apex up to rounding is touched at the apex (gpn+ERMK case 532)
            (9.568336173805233e-06, 19.83628419762285, &[[1000000090.9296813, -999999202.3687571, 1000000090.9296813, -999999202.3687571], [1000000090.9296813, -999999202.3687571, 1000000090.9296813, -999999212.1617153], [1000000093.4597049, -999999211.5965433, 1000000092.7684509, -999999212.8736252], [1000000092.8419323, -999999212.7378693, 1000000092.8419323, -999999212.7378693], [1000000093.4597049, -999999211.5965433, 1000000092.7684509, -999999212.8736252], [1000000093.4597049, -999999211.5965433, 1000000092.7684509, -999999212.8736252], [1000000093.2024333, -999999212.0718486, 1000000093.2024333, -999999212.0718486], [1000000098.4240805, -999999218.8231815, 1000000098.4240805, -999999218.8231815]]),
            // ... but not while the portal before it is still to be touched (on case 425)
            (1.0, 5259533.78180249, &[[-1351424.2115583862, 641590.7425761361, -1351424.2115583862, 641590.7425761361], [0.0, 813011.3842585457, 0.0, -1000000.0], [0.0, 1000000.0, 504883.3604995946, -261986.29691416398], [2659519.527279334, -314630.60431768536, 1032526.6688745036, -353767.0792582098], [2413016.16507089, -964897.1793791646, 546281.508776986, -365463.4560583916], [-202748.34596443397, -792420.4013417795, 546281.5087769863, -365463.4560583916], [242268.0320350646, 910575.4359579084, 472983.0212292308, -57806.960565897636], [1458099.146883905, -1261032.7831399434, 546281.5087769863, -365463.4560583916], [332250.70144346176, -1907896.7340601534, 332250.70144346176, -1907896.7340601534]]),
            // exact incidences are no skip (the chain rules handle them) (zp+E case 538)
            (1.0, 2877476.3748416454, &[[0.0, -1000000.0, 0.0, -1000000.0], [0.0, 1000000.0, 0.0, -1000000.0], [2309985.2650672626, -3036596.5501610674, 0.0, -1000000.0], [0.0, -1000000.0, 0.0, -1000000.0], [-1661018.4255773458, -645535.3403434348, 0.0, -1000000.0], [1707775.084123028, -860871.3420136938, 0.0, -1000000.0], [1707775.084123028, -860871.3420136938, 721465.4533669839, -1272253.0463654299], [1707775.084123028, -860871.3420136938, 2965422.912297478, -753326.6953178273], [2133345.4836928323, -523554.154419974, 2965422.912297478, -753326.6953178273], [2133345.4836928323, -523554.154419974, 2536953.2393398248, -116963.14485914691], [2022858.019262054, 87540.8144233746, 2022858.019262054, 87540.8144233746]]),
        ];
        for (k, &(eps, refl, c)) in cases.iter().enumerate() {
            let ports: Vec<(Vec2, Vec2)> = c.iter().map(|q| (Vec2 { x: q[0], y: q[1] }, Vec2 { x: q[2], y: q[3] })).collect();
            let f = funnel(&ports);
            let pp: Vec<Vec2> = f.iter().map(|q| q.0).collect();
            assert_eq!(touches_in_order(&pp, &ports), Ok(()), "case {k}: {:?}", pts(&f));
            assert!(plen(&f) <= refl + 10.0 * eps, "case {k}: {} > {}: {:?}", plen(&f), refl, pts(&f));
        }
    }

    /// [`edge_point`] returns the edge's ends bit for bit at `t = 0` and `t = 1` (the naive
    /// `a + (b - a) t` misses `b` by an ulp for these board-scale coordinates: checked below), and
    /// stays within an ulp of the line in between.
    #[test]
    fn edge_point_hits_both_ends_bit_exact() {
        let xs = [(-198646752.25025728, 71173699.81907648), (-192772985.84858796, 115095321.5921737), (-177717107.59684262, 166427739.37679636), (135371712.4672529, -147771269.138752)];
        let mut naive_missed = 0;
        for &(p, q) in &xs {
            let (a, b) = (Vec2 { x: p, y: q }, Vec2 { x: q, y: p });
            assert_eq!(edge_point(a, b, 0.0), a);
            assert_eq!(edge_point(a, b, 1.0), b);
            assert_eq!(edge_point(b, a, 1.0), a);
            naive_missed += (a + (b - a) * 1.0 != b) as usize + (b + (a - b) * 1.0 != a) as usize;
            let m = edge_point(a, b, 0.5);
            assert!(((m - a).len() - (b - m).len()).abs() < 1e-6, "the midpoint");
        }
        assert!(naive_missed >= 4, "the naive form should miss an end here: {naive_missed}");
    }

    /// While every portal since the apex passes through it (a fan of portals ending at the apex
    /// or holding it, collinear grid fans), an end crossing over the other chain makes no
    /// corner; the first portal that misses the apex makes them count again (its own end
    /// included: (-1, 3) is a corner on the way to (-2, 4)).
    #[test]
    fn portals_through_the_apex_constrain_nothing() {
        let g = v;
        // Fan round (1, -3): portal 6 holds it, 7 and 8 end at it; (2, -3) is no corner.
        let ports = vec![(g(-1.0, -2.0), g(-1.0, -2.0)), (g(0.0, 1.0), g(0.0, -2.0)), (g(3.0, -4.0), g(0.0, -2.0)), (g(1.0, -3.0), g(0.0, -2.0)), (g(1.0, -3.0), g(1.0, -3.0)), (g(1.0, -3.0), g(-2.0, -3.0)), (g(2.0, -3.0), g(-2.0, -3.0)), (g(1.0, -3.0), g(-2.0, -3.0)), (g(1.0, -3.0), g(3.0, -3.0)), (g(1.0, -3.0), g(-1.0, -2.0)), (g(1.0, -3.0), g(0.0, -5.0)), (g(1.0, -3.0), g(4.0, -3.0)), (g(-1.0, -1.0), g(4.0, -3.0)), (g(-2.0, 1.0), g(-2.0, 1.0))];
        let f = funnel(&ports);
        assert_eq!(pts(&f), vec![(-1.0, -2.0), (0.0, -2.0), (1.0, -3.0), (-2.0, 1.0)]);
        let ports = vec![(g(-1.0, 0.0), g(-1.0, 0.0)), (g(0.0, 2.0), g(0.0, -2.0)), (g(0.0, 2.0), g(0.0, 0.0)), (g(0.0, 2.0), g(0.0, 2.0)), (g(0.0, 2.0), g(2.0, 3.0)), (g(0.0, 2.0), g(-1.0, 3.0)), (g(-2.0, 4.0), g(-1.0, 3.0)), (g(-2.0, 4.0), g(1.0, 5.0)), (g(-3.0, 7.0), g(-3.0, 7.0))];
        let f = funnel(&ports);
        assert_eq!(pts(&f), vec![(-1.0, 0.0), (0.0, 2.0), (-1.0, 3.0), (-3.0, 7.0)]);
    }

    /// Degenerate inputs the fuzz found (exact coordinates, nm): a right end equal to the apex
    /// that the left end then moves on from (it has to be added from the new apex, or the path
    /// misses that portal by 0.1 um); an end shared by two portals one ulp apart, so that the
    /// end pad lands on both chains without crossing over either (the collapse); and one where
    /// that ulp makes a portal look seen the wrong way round while the portal before it is still
    /// to be touched (no mirror).
    #[test]
    fn degenerate_fuzz_cases_touch_every_portal_and_are_shortest() {
        let cases: [&[[f64; 4]]; 3] = [
            &[[-1841097.8873727873, -208797.2039078317, -1841097.8873727873, -208797.2039078317], [0.0, 760300.973206882, 0.0, -1000000.0], [535135.1257152129, -3460254.6405418892, 0.0, -1000000.0], [-541924.2574602593, -2633862.091027379, -43713.71933451248, -1131793.673922611], [-214464.40712545527, -172988.53309784288, 0.0, -1000000.0], [383292.5940079415, 2463.123655845411, 383292.5940079415, 2463.123655845411]],
            &[[-1032781.538331049, 508082.34518565855, -1032781.538331049, 508082.34518565855], [0.0, 1000000.0, 0.0, -1000000.0], [0.0, 1000000.0, 288660.53347158193, 2118496.4688855894], [0.0, 1000000.0, -342380.6972296029, 2314003.1521048946], [-1721296.505376722, 1644588.0181968529, -342380.69722960284, 2314003.1521048946], [-1721296.505376722, 1644588.0181968529, -3923724.5170491515, 1311909.113548525], [-3441364.101808506, 962334.9395533181, -3441364.101808506, 962334.9395533181]],
            &[[-1272002.7217578352, 363005.8388931152, -1272002.7217578352, 363005.8388931152], [0.0, 1000000.0, 0.0, -1000000.0], [0.0, 1000000.0, 223919.86484004525, 58463.325159764034], [1382775.2183056858, -588170.3536353441, 223919.86484004534, 58463.325159764034], [1382775.2183056858, -588170.3536353441, 1363905.367012714, -1380445.1587787527], [2664018.6506470353, -723450.7955727347, 2664018.6506470353, -723450.7955727347]],
        ];
        for (k, c) in cases.iter().enumerate() {
            let ports: Vec<(Vec2, Vec2)> = c.iter().map(|q| (Vec2 { x: q[0], y: q[1] }, Vec2 { x: q[2], y: q[3] })).collect();
            let f = funnel(&ports);
            let pp: Vec<Vec2> = f.iter().map(|q| q.0).collect();
            assert_eq!(touches_in_order(&pp, &ports), Ok(()), "case {k}: {:?}", pts(&f));
            assert!(plen(&f) <= reference(&ports) + 10.0, "case {k}: {} > {}", plen(&f), reference(&ports));
        }
        // The first: the apex (0, -1) is revisited after the radial portal 3.
        let ports: Vec<(Vec2, Vec2)> = cases[0].iter().map(|q| (Vec2 { x: q[0], y: q[1] }, Vec2 { x: q[2], y: q[3] })).collect();
        assert_eq!(funnel(&ports).iter().filter(|c| c.0 == v(0.0, -1.0)).count(), 2);
    }

    /// A last terminal equal to the corner before it (a point portal at the end pad, then the
    /// pad itself) is not repeated; it carries the last portal's index.
    #[test]
    fn end_equal_to_the_last_corner_carries_the_last_index() {
        let (s, e) = (v(0.0, 0.0), v(4.0, 0.0));
        let ports = vec![(s, s), (v(2.0, 1.0), v(2.0, -1.0)), (e, e), (e, e)];
        assert_eq!(funnel(&ports), vec![(s, 0), (e, 3)]);
        // A zero-length run (both terminals one point): both terminals stay.
        assert_eq!(funnel(&[(s, s), (s, s)]), vec![(s, 0), (s, 1)]);
    }

    /// Exact reference (the verifier's): the shortest polyline from the first terminal to the
    /// last that bends at portal ends only and touches every portal in order, by a shortest path
    /// over the DAG of portal ends (an edge where the segment touches the portals between).
    /// O(portals^3).
    fn reference(ports: &[(Vec2, Vec2)]) -> f64 {
        let mut nodes: Vec<(Vec2, usize)> = Vec::new();
        for (i, &(l, r)) in ports.iter().enumerate() {
            nodes.push((l, i));
            if r != l {
                nodes.push((r, i));
            }
        }
        let mut dist = vec![f64::INFINITY; nodes.len()];
        dist[0] = 0.0;
        for a in 0..nodes.len() {
            if !dist[a].is_finite() {
                continue;
            }
            let (p, i) = nodes[a];
            for b in a + 1..nodes.len() {
                let (q, j) = nodes[b];
                if j > i && touches_in_order(&[p, q], &ports[i + 1..j]).is_ok() {
                    dist[b] = dist[b].min(dist[a] + (q - p).len());
                }
            }
        }
        (0..nodes.len()).filter(|&k| nodes[k].1 == ports.len() - 1).map(|k| dist[k]).fold(f64::INFINITY, f64::min)
    }

    /// Random triangle strips on a coarse integer grid (1 mm), so collinear ends, ends on the
    /// lines of chain edges and starts on portal lines happen all the time: every tie rule is
    /// exercised. Each new vertex lies in front of the last portal or on its line (a proper
    /// sleeve up to those ties); some pivots are point portals, the end lies beyond the last
    /// portal. The funnel touches every portal in order and is no longer than the reference.
    #[test]
    fn grid_strips_with_ties_match_the_reference() {
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        let mut rnd = move |n: i64| -> i64 {
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            ((x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) % n as u64) as i64
        };
        let g = |a: i64, b: i64| v(a as f64, b as f64);
        let mut ties = 0;
        for case in 0..1500 {
            let (mut l, mut r) = (g(0, 1 + rnd(2)), g(0, -1 - rnd(2)));
            let s = g(-1 - rnd(3), rnd(5) - 2);
            let mut ports = vec![(s, s), (l, r)];
            for _ in 0..(2 + case % 9) {
                // A new vertex in front of the portal l-r or on its line, near it.
                let (lo, hi) = (l.x.min(r.x) / 1e6, l.x.max(r.x) / 1e6);
                let mut nv = None;
                for _ in 0..20 {
                    let c = g(lo as i64 - 2 + rnd(6 + (hi - lo) as i64), (l.y.min(r.y) / 1e6) as i64 - 2 + rnd(6 + ((l.y - r.y).abs() / 1e6) as i64));
                    if orient(r, l, c) <= 0 && c != l && c != r {
                        ties += (orient(r, l, c) == 0) as usize;
                        nv = Some(c);
                        break;
                    }
                }
                let Some(c) = nv else { break };
                if rnd(2) == 0 {
                    if rnd(7) == 0 {
                        ports.push((r, r));
                    }
                    l = c;
                } else {
                    if rnd(7) == 0 {
                        ports.push((l, l));
                    }
                    r = c;
                }
                ports.push((l, r));
            }
            // The end: in front of the last portal or on its line.
            let mid = (l + r) * 0.5;
            let mut e = g((mid.x / 1e6).round() as i64, (mid.y / 1e6).round() as i64);
            for _ in 0..20 {
                let c = g((mid.x / 1e6) as i64 - 3 + rnd(7), (mid.y / 1e6) as i64 - 3 + rnd(7));
                if orient(r, l, c) < 0 {
                    e = c;
                    break;
                }
            }
            ports.push((e, e));
            let f = funnel(&ports);
            let pp: Vec<Vec2> = f.iter().map(|q| q.0).collect();
            let refl = reference(&ports);
            assert_eq!(touches_in_order(&pp, &ports), Ok(()), "case {case}: {ports:?} -> {:?}", pts(&f));
            assert!(plen(&f) <= refl + 10.0, "case {case}: {} > reference {refl}: {ports:?} -> {:?}", plen(&f), pts(&f));
        }
        assert!(ties > 100, "ties exercised: {ties}");
    }
}
