//! Copper pieces of one net (fuse-octi r4, H1 part 2): how many connected pieces the drawn
//! copper of a net and its pads form, the split guard of the steps after the drawing (the fuse
//! and the octilinear polish, core.rs): a step that raises the count would split the net and is
//! not taken.
//!
//! Contact rule (KiCad's connectivity, the spec: "KiCad tools are the spec"): an item's anchors
//! are its ends (a track's two end points, a via's centre, a pad's centre); two items are joined
//! where an anchor of one lies on the copper of the other (a track: within half its width of its
//! centre line on the same layer; a via: within its radius, any layer; a pad: inside its outline
//! on a layer it occupies). Without its anchors on other copper a track end is dangling: KiCad
//! Cleanup's "dangling track" and DRC's `unconnected_items` (qmtech JS2_9 r3 and main's JS1_2,
//! a fused branch foot left 0.86 / 1.6 mm off its trunk's polished copper).
//!
//! A drawn wire is one piece in itself (its runs meet at its vias). O(anchors x net copper).

use super::state::{point_in_poly, LayerMask};
use crate::geom::Vec2;

/// The drawn copper of one wire: `(layer, polyline)` runs, vias `(centre, radius)` (nm), half
/// the track width (nm).
pub struct Piece<'a> {
    pub runs: &'a [(usize, Vec<Vec2>)],
    pub vias: Vec<(Vec2, f64)>,
    pub half_w: f64,
}

/// A pad (or other fixed copper) of the net: outline, layers, anchor (centre).
pub struct Term<'a> {
    pub poly: &'a [Vec2],
    pub layers: LayerMask,
    pub center: Vec2,
}

/// Slack on every contact test (nm): rounding of the KiCad frame's integer nm.
const TOL: f64 = 1_000.0;

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.len2().max(1.0)).clamp(0.0, 1.0);
    (a + ab * t - p).len()
}

/// Whether the point `p` (on `layer`, `None`: a via, every layer) lies on the copper of `pc`.
fn on_piece(p: Vec2, layer: Option<usize>, pc: &Piece) -> bool {
    pc.vias.iter().any(|&(c, r)| (p - c).len() <= r + TOL)
        || pc.runs.iter().any(|(l, pts)| layer.is_none_or(|x| x == *l) && (pts.windows(2).any(|s| seg_dist(p, s[0], s[1]) <= pc.half_w + TOL) || pts.len() == 1 && (p - pts[0]).len() <= pc.half_w + TOL))
}

/// Whether the point `p` (on `layer`, `None`: a via) lies on the pad `t`.
fn on_term(p: Vec2, layer: Option<usize>, t: &Term) -> bool {
    if layer.is_some_and(|l| !t.layers.has(l)) {
        return false;
    }
    point_in_poly(p, t.poly) || t.poly.iter().enumerate().any(|(i, &a)| seg_dist(p, a, t.poly[(i + 1) % t.poly.len()]) <= TOL)
}

/// The anchors of a piece: each run's end points (on its layer) and each via centre (any layer).
fn anchors(pc: &Piece) -> Vec<(Vec2, Option<usize>)> {
    let mut a: Vec<(Vec2, Option<usize>)> = Vec::new();
    for (l, pts) in pc.runs {
        if let (Some(&s), Some(&e)) = (pts.first(), pts.last()) {
            a.push((s, Some(*l)));
            a.push((e, Some(*l)));
        }
    }
    a.extend(pc.vias.iter().map(|&(c, _)| (c, None)));
    a
}

fn find(p: &mut [usize], mut x: usize) -> usize {
    while p[x] != x {
        p[x] = p[p[x]];
        x = p[x];
    }
    x
}

/// How many connected pieces the wires `pieces` and pads `terms` of one net form (a pad that no
/// copper reaches is a piece of its own). O((anchors) x (copper of the net)).
pub fn components(pieces: &[Piece], terms: &[Term]) -> usize {
    let n = pieces.len() + terms.len();
    let mut par: Vec<usize> = (0..n).collect();
    let join = |par: &mut Vec<usize>, a: usize, b: usize| {
        let (ra, rb) = (find(par, a), find(par, b));
        if ra != rb {
            par[ra.max(rb)] = ra.min(rb);
        }
    };
    for (i, pc) in pieces.iter().enumerate() {
        for (p, l) in anchors(pc) {
            for (j, other) in pieces.iter().enumerate() {
                if j != i && on_piece(p, l, other) {
                    join(&mut par, i, j);
                }
            }
            for (k, t) in terms.iter().enumerate() {
                if on_term(p, l, t) {
                    join(&mut par, i, pieces.len() + k);
                }
            }
        }
    }
    // A pad's anchor (its centre) on a track or via that does not end in it (KiCad joins those too).
    for (k, t) in terms.iter().enumerate() {
        for (j, pc) in pieces.iter().enumerate() {
            let hit = pc.vias.iter().any(|&(c, r)| (t.center - c).len() <= r + TOL) || pc.runs.iter().any(|(l, pts)| t.layers.has(*l) && pts.windows(2).any(|s| seg_dist(t.center, s[0], s[1]) <= pc.half_w + TOL));
            if hit {
                join(&mut par, j, pieces.len() + k);
            }
        }
    }
    (0..n).filter(|&x| find(&mut par, x) == x).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MM: f64 = 1e6;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x: x * MM, y: y * MM }
    }

    fn sq(c: Vec2, h: f64) -> Vec<Vec2> {
        vec![c + v(-h, -h), c + v(h, -h), c + v(h, h), c + v(-h, h)]
    }

    /// A trunk from pad A to pad B and a branch from pad C whose foot lies on the trunk: one piece;
    /// the trunk redrawn as an elbow that leaves the foot 1.6 mm off (main's qmtech JS1_2): two.
    #[test]
    fn a_branch_foot_off_its_trunk_splits_the_net() {
        let (a, b, c) = (v(0.0, 0.0), v(10.0, 5.0), v(6.0, -6.0));
        let (pa, pb, pc) = (sq(a, 0.5), sq(b, 0.5), sq(c, 0.5));
        let one = LayerMask(1);
        let terms = [Term { poly: &pa, layers: one, center: a }, Term { poly: &pb, layers: one, center: b }, Term { poly: &pc, layers: one, center: c }];
        let foot = v(6.0, 3.0);
        let trunk = vec![(0usize, vec![a, b])];
        let branch = vec![(0usize, vec![c, foot])];
        fn pieces<'a>(t: &'a [(usize, Vec<Vec2>)], branch: &'a [(usize, Vec<Vec2>)]) -> Vec<Piece<'a>> {
            vec![Piece { runs: t, vias: vec![], half_w: 0.1 * MM }, Piece { runs: branch, vias: vec![], half_w: 0.1 * MM }]
        }
        let p = pieces(&trunk, &branch);
        assert_eq!(components(&p, &terms), 1);
        // The polish's elbow of the trunk: 45 degrees to (5, 5), then east: the foot is off it.
        let elbow = vec![(0usize, vec![a, v(5.0, 5.0), b])];
        let p = pieces(&elbow, &branch);
        assert_eq!(components(&p, &terms), 2);
        // The elbow with the foot kept as a vertex (fuse-octi r4 part 1): one piece again.
        let kept = vec![(0usize, vec![a, v(3.0, 3.0), foot, v(8.0, 5.0), b])];
        let p = pieces(&kept, &branch);
        assert_eq!(components(&p, &terms), 1);
    }

    /// The contact tests: half the width of the track the anchor lands on, layers, vias on every
    /// layer, a pad on its own layers only, a pad centre on a passing track.
    #[test]
    fn contacts_follow_width_layer_via_and_pad_rules() {
        let (a, b) = (v(0.0, 0.0), v(10.0, 0.0));
        let pa = sq(a, 0.5);
        let one = LayerMask(1);
        let terms = [Term { poly: &pa, layers: one, center: a }];
        let trunk = vec![(0usize, vec![a, b])];
        // An end 0.09 mm off the trunk's centre line (half width 0.1): joined; 0.12 mm: not.
        for (dy, want) in [(0.09, 1), (0.12, 2)] {
            let br = vec![(0usize, vec![v(5.0, dy), v(5.0, 4.0)])];
            let p = [Piece { runs: &trunk, vias: vec![], half_w: 0.1 * MM }, Piece { runs: &br, vias: vec![], half_w: 0.1 * MM }];
            assert_eq!(components(&p, &terms), want, "dy {dy}");
        }
        // The same end on the other layer: not joined; with a via of the branch there: joined.
        let br = vec![(1usize, vec![v(5.0, 0.0), v(5.0, 4.0)])];
        let p = [Piece { runs: &trunk, vias: vec![], half_w: 0.1 * MM }, Piece { runs: &br, vias: vec![], half_w: 0.1 * MM }];
        assert_eq!(components(&p, &terms), 2);
        let p = [Piece { runs: &trunk, vias: vec![], half_w: 0.1 * MM }, Piece { runs: &br, vias: vec![(v(5.0, 0.0), 0.3 * MM)], half_w: 0.1 * MM }];
        assert_eq!(components(&p, &terms), 1);
        // A pad on layer 1 only: the trunk's end on layer 0 inside its outline does not reach it.
        let pb = sq(b, 0.5);
        let terms2 = [Term { poly: &pa, layers: one, center: a }, Term { poly: &pb, layers: LayerMask(2), center: b }];
        let p = [Piece { runs: &trunk, vias: vec![], half_w: 0.1 * MM }];
        assert_eq!(components(&p, &terms2), 2);
        // A pad whose centre a track passes over (no end in it): joined.
        let pm = sq(v(5.0, 0.0), 0.3);
        let terms3 = [Term { poly: &pa, layers: one, center: a }, Term { poly: &pm, layers: one, center: v(5.0, 0.0) }];
        assert_eq!(components(&p, &terms3), 1);
        // Nothing at all of the net drawn: every pad is a piece.
        assert_eq!(components(&[], &terms3), 2);
    }
}
