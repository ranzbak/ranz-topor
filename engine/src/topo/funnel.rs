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
//! CGTA 4:63-98) and Erickson, Computational Topology notes, ch. 5 sec. 5.5 "Growing funnels"
//! pp. 4-5 (local copy docs/literature/erickson-shortest-homotopic.html): an apex and two
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
//!   behind the line (the apex lies on it up to rounding), the portal is taken as seen the
//!   right way round from the apex (the limit of a mirror image an infinitesimal step behind).
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
//!   since the apex, up to the one before that chain's next vertex (up to the current one when
//!   it is the last), passes through the apex: that part of the chain constrains nothing.
//! * a point portal equal to the apex while a portal since the apex misses it: the path goes
//!   out along the shorter chain (touching those portals) and back to the point (Lee &
//!   Preparata's funnel never revisits the apex; the touching order requires it here).
//! * a right end equal to the apex is not added; when the left end then moves the apex on,
//!   the right end is added from the new apex (the portal is still to be touched).
//!
//! Corners are portal ends (with the index of the last portal each is an end of) and the two
//! terminals; a fold point from a reflection carries the index of the portal it touches and
//! is no portal end (callers that map corners to sheet vertices compare positions).

use crate::geom::predicates::{on_ray, orient, OnRay};
use crate::geom::Vec2;

/// The path corners with the index of a portal each is an end of (the last one before it became
/// a corner), first and last terminal included. See the module documentation.
pub fn funnel(ports: &[(Vec2, Vec2)]) -> Vec<(Vec2, usize)> {
    let n = ports.len();
    if n == 0 {
        return Vec::new();
    }
    let s = ports[0].0;
    let mut f = Funnel { ports, apex: (s, 0), mirrors: Vec::new(), chain: [Vec::new(), Vec::new()], head: [0, 0], out: vec![(s, 0)], pend: (s, 0, 1, usize::MAX) };
    for i in 1..n {
        let (l, r) = ports[i];
        if l == r && i >= 2 && ports[i - 1].0 != ports[i - 1].1 && orient(ports[i - 1].1, ports[i - 1].0, l) > 0 {
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
                if x != self.out[self.out.len() - 1].0 && x != c {
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
        if x != prev && x != t {
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
            // are still to be touched first: out along the shorter chain and back.
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
            if !on_segment(a, l, r) {
                self.pend.3 = self.pend.2;
            }
            self.pend.2 += 1;
        }
        self.pend.3 <= k
    }

    /// Whether the other chain's end `chain[o][k]`, which a new end of portal `i` crosses over,
    /// is slack: every portal since the apex up to the one before the next end on that chain
    /// (up to portal `i` when it is the last) passes through the apex. Those portals are
    /// touched at the apex already and the chain edge from it constrains nothing up to there,
    /// so it makes no corner (degenerate fans of portals collinear with the apex, radial edges
    /// of a turn vertex). Exact. O(1) amortised.
    fn slack(&mut self, o: usize, k: usize, i: usize) -> bool {
        let next = if k + 1 < self.chain[o].len() { self.chain[o][k + 1].1 } else { i + 1 };
        !self.pending(next - 1)
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
                    if orient(img, c, p) > 0 {
                        self.mirrors.push((p, c, i, a));
                        self.apex.0 = img;
                    }
                    break;
                }
                if cross {
                    let k = self.head[o];
                    self.head[o] += 1;
                    if !self.slack(o, k, i) {
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
    /// passed twice around a portal radial-ish to it): out along the shorter chain to touch the
    /// portal and back; on a tie the right chain. The plain path keeps this (it touches the
    /// portal before the point), so the point is not reflected although it lies behind it.
    #[test]
    fn point_equal_to_the_apex_goes_out_and_back() {
        let (s, t, e) = (v(-1.0, -1.0), v(0.0, 0.0), v(1.0, -1.0));
        let ports = vec![(s, s), (t, t), (v(-1.0, 2.0), v(1.0, 2.0)), (t, t), (e, e)];
        let f = funnel(&ports);
        assert_eq!(pts(&f), vec![(-1.0, -1.0), (0.0, 0.0), (1.0, 2.0), (0.0, 0.0), (1.0, -1.0)]);
        assert_eq!(f.iter().map(|c| c.1).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
        // The left chain shorter: out to its end.
        let ports = vec![(s, s), (t, t), (v(-1.0, 2.0), v(2.0, 2.0)), (t, t), (e, e)];
        assert_eq!(pts(&funnel(&ports)), vec![(-1.0, -1.0), (0.0, 0.0), (-1.0, 2.0), (0.0, 0.0), (1.0, -1.0)]);
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
