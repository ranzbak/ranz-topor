//! Copper pieces of one net (fuse-octi r4, H1 part 2; r6): which connected pieces the drawn
//! copper of a net and its pads form, the split guard of the steps after the drawing (the fuse
//! and the octilinear polish, core.rs): a step that splits a piece that was joined is not taken.
//!
//! Contact rule (KiCad's connectivity is the spec, "KiCad tools are the spec"; fuse-octi r6,
//! checked with kicad-cli 10.0.8 on a probe board by the r5 verifier): KiCad joins copper that
//! OVERLAPS. Two tracks on one layer are joined where their centre lines come within the sum of
//! their half widths (a branch foot up to 0.2 mm off a 0.2 mm trunk's centre line with a 0.2 mm
//! branch, and two tracks that cross with no end on each other); a via joins a track within its
//! radius plus the track's half width and another via within the sum of the radii (any layer); a
//! track joins a pad on a layer the pad occupies where it reaches the pad's outline (outline grown
//! by the half width), a via any pad it overlaps. Copper that does not overlap anything of its net is
//! a separate piece: KiCad's `unconnected_items` (qmtech JS2_9 r3 and main's JS1_2, a fused branch
//! foot left 0.86 / 1.6 mm off its trunk's polished copper).
//!
//! A drawn wire is one piece in itself (its runs meet at its vias). The guard compares PARTITIONS,
//! not counts ([`splits`]): a step that cuts one piece off and joins two others of the same net has
//! fewer pieces but still splits what was joined. O(pairs of pieces with overlapping boxes x their
//! segments).

use super::state::{point_in_poly, LayerMask};
use crate::geom::Vec2;
use std::collections::BTreeSet;

/// The drawn copper of one wire: `(layer, polyline)` runs, vias `(centre, radius)` (nm), half
/// the track width (nm).
pub struct Piece<'a> {
    pub runs: &'a [(usize, Vec<Vec2>)],
    pub vias: Vec<(Vec2, f64)>,
    pub half_w: f64,
}

/// A pad (or other fixed copper) of the net: outline, layers, centre.
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

fn cross(o: Vec2, a: Vec2, b: Vec2) -> f64 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

/// Distance between the segments a-b and c-d (0 where they cross).
fn seg_seg(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> f64 {
    let (d1, d2, d3, d4) = (cross(c, d, a), cross(c, d, b), cross(a, b, c), cross(a, b, d));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0)) {
        return 0.0;
    }
    seg_dist(a, c, d).min(seg_dist(b, c, d)).min(seg_dist(c, a, b)).min(seg_dist(d, a, b))
}

/// The segments of a run (a single point: a zero-length segment).
fn segs(pts: &[Vec2]) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
    let one = (pts.len() == 1).then(|| (pts[0], pts[0]));
    pts.windows(2).map(|s| (s[0], s[1])).chain(one)
}

/// Bounding box of a piece grown by its reach (half width, via radius).
fn bbox(pc: &Piece) -> (Vec2, Vec2) {
    let (mut lo, mut hi) = (Vec2 { x: f64::INFINITY, y: f64::INFINITY }, Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY });
    let mut add = |p: Vec2, r: f64| {
        lo = Vec2 { x: lo.x.min(p.x - r), y: lo.y.min(p.y - r) };
        hi = Vec2 { x: hi.x.max(p.x + r), y: hi.y.max(p.y + r) };
    };
    for (_, pts) in pc.runs {
        for &p in pts {
            add(p, pc.half_w);
        }
    }
    for &(c, r) in &pc.vias {
        add(c, r);
    }
    (lo, hi)
}

fn boxes_meet(a: (Vec2, Vec2), b: (Vec2, Vec2)) -> bool {
    a.0.x <= b.1.x + TOL && b.0.x <= a.1.x + TOL && a.0.y <= b.1.y + TOL && b.0.y <= a.1.y + TOL
}

/// Whether the copper of `a` and `b` overlaps (KiCad joins them).
fn overlap(a: &Piece, b: &Piece) -> bool {
    let reach = a.half_w + b.half_w + TOL;
    for (la, pa) in a.runs {
        for (s0, s1) in segs(pa) {
            for (lb, pb) in b.runs {
                if la == lb && segs(pb).any(|(t0, t1)| seg_seg(s0, s1, t0, t1) <= reach) {
                    return true;
                }
            }
            if b.vias.iter().any(|&(c, r)| seg_dist(c, s0, s1) <= r + a.half_w + TOL) {
                return true;
            }
        }
    }
    for &(c, r) in &a.vias {
        if b.vias.iter().any(|&(d, q)| (c - d).len() <= r + q + TOL) {
            return true;
        }
        if b.runs.iter().any(|(_, pb)| segs(pb).any(|(t0, t1)| seg_dist(c, t0, t1) <= r + b.half_w + TOL)) {
            return true;
        }
    }
    false
}

/// Distance from `p` to the pad outline (0 inside).
fn pad_dist(p: Vec2, t: &Term) -> f64 {
    if point_in_poly(p, t.poly) {
        return 0.0;
    }
    (0..t.poly.len()).map(|i| seg_dist(p, t.poly[i], t.poly[(i + 1) % t.poly.len()])).fold(f64::INFINITY, f64::min)
}

/// Whether the copper of `pc` overlaps the pad `t`.
fn on_pad(pc: &Piece, t: &Term) -> bool {
    let edge = |s0: Vec2, s1: Vec2| (0..t.poly.len()).map(|i| seg_seg(s0, s1, t.poly[i], t.poly[(i + 1) % t.poly.len()])).fold(f64::INFINITY, f64::min);
    pc.vias.iter().any(|&(c, r)| pad_dist(c, t) <= r + TOL)
        || pc.runs.iter().any(|(l, pts)| t.layers.has(*l) && segs(pts).any(|(s0, s1)| pad_dist(s0, t) <= pc.half_w + TOL || edge(s0, s1) <= pc.half_w + TOL))
}

fn find(p: &mut [usize], mut x: usize) -> usize {
    while p[x] != x {
        p[x] = p[p[x]];
        x = p[x];
    }
    x
}

/// The piece of every item (the wires `pieces`, then the pads `terms`) of one net: the lowest item
/// index of its connected piece. A pad that no copper reaches is a piece of its own.
pub fn partition(pieces: &[Piece], terms: &[Term]) -> Vec<usize> {
    let n = pieces.len() + terms.len();
    let mut par: Vec<usize> = (0..n).collect();
    let join = |par: &mut Vec<usize>, a: usize, b: usize| {
        let (ra, rb) = (find(par, a), find(par, b));
        if ra != rb {
            par[ra.max(rb)] = ra.min(rb);
        }
    };
    let boxes: Vec<(Vec2, Vec2)> = pieces.iter().map(bbox).collect();
    for i in 0..pieces.len() {
        for j in i + 1..pieces.len() {
            if find(&mut par, i) != find(&mut par, j) && boxes_meet(boxes[i], boxes[j]) && overlap(&pieces[i], &pieces[j]) {
                join(&mut par, i, j);
            }
        }
        for (k, t) in terms.iter().enumerate() {
            if on_pad(&pieces[i], t) {
                join(&mut par, i, pieces.len() + k);
            }
        }
    }
    (0..n).map(|x| find(&mut par, x)).collect()
}

/// How many connected pieces the wires `pieces` and pads `terms` of one net form.
pub fn components(pieces: &[Piece], terms: &[Term]) -> usize {
    let p = partition(pieces, terms);
    (0..p.len()).filter(|&x| p[x] == x).count()
}

/// Whether the partition `after` splits anything that `before` joined (both of the same items in
/// the same order): two items in one piece before and in two after. Pieces joined by the step do
/// not pay for one it cuts off. O(items).
pub fn splits(before: &[usize], after: &[usize]) -> bool {
    let mut to: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    before.iter().zip(after).any(|(&b, &a)| *to.entry(b).or_insert(a) != a)
}

/// The dangling track ends of one net: `(piece, run, end)` (end 0: first point, 1: last) of every
/// run end that touches no other copper (KiCad's dangling test, Cleanup "dangling track" / DRC
/// `track_dangling`: an end is connected when it lies within the other copper grown by its own half
/// width: another wire's track within the sum of the half widths, a via within its radius plus the
/// half width, a pad's outline within the half width; the own wire's vias and other runs count).
/// O(ends x copper of the net).
pub fn dangling(pieces: &[Piece], terms: &[Term]) -> BTreeSet<(usize, usize, usize)> {
    let mut out = BTreeSet::new();
    for (i, pc) in pieces.iter().enumerate() {
        for (k, (l, pts)) in pc.runs.iter().enumerate() {
            for (e, p) in [(0usize, pts.first()), (1, pts.last())] {
                let Some(&p) = p else { continue };
                let h = pc.half_w;
                let own = pc.vias.iter().any(|&(c, r)| (p - c).len() <= r + h + TOL) || pc.runs.iter().enumerate().any(|(k2, (l2, q))| k2 != k && l2 == l && segs(q).any(|(a, b)| seg_dist(p, a, b) <= 2.0 * h + TOL));
                let other = pieces.iter().enumerate().any(|(j, o)| {
                    j != i && (o.vias.iter().any(|&(c, r)| (p - c).len() <= r + h + TOL) || o.runs.iter().any(|(l2, q)| l2 == l && segs(q).any(|(a, b)| seg_dist(p, a, b) <= o.half_w + h + TOL)))
                });
                let pad = terms.iter().any(|t| t.layers.has(*l) && pad_dist(p, t) <= h + TOL);
                if !(own || other || pad) {
                    out.insert((i, k, e));
                }
            }
        }
    }
    out
}

/// The connectivity of one net that the split guard compares: its [`partition`] and its
/// [`dangling`] ends.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Conn {
    pub part: Vec<usize>,
    pub dangling: BTreeSet<(usize, usize, usize)>,
}

/// [`partition`] and [`dangling`] of one net.
pub fn conn(pieces: &[Piece], terms: &[Term]) -> Conn {
    Conn { part: partition(pieces, terms), dangling: dangling(pieces, terms) }
}

impl Conn {
    /// How many pieces.
    pub fn n(&self) -> usize {
        (0..self.part.len()).filter(|&x| self.part[x] == x).count()
    }
}

/// Whether `after` is worse than `before` for KiCad: it splits what was joined ([`splits`]) or
/// has a dangling end that was not dangling (a branch foot left off its trunk while the branch
/// still crosses it: joined, yet Cleanup's "dangling track"). O(items + ends).
pub fn worse(before: &Conn, after: &Conn) -> bool {
    splits(&before.part, &after.part) || after.dangling.difference(&before.dangling).next().is_some()
}

/// The feet of other copper on the wire `own` (fuse-octi r6, r5 verifier L2): where an end of a
/// run or a via of `others` (same net) lies within KiCad's reach of a segment of `own` (the sum of
/// the half widths, a via's radius plus `own`'s half width), its projection onto that segment, when
/// it lies inside the segment. The polish keeps these as vertices, so a redrawn segment still
/// passes where another wire meets it (esp +3V3 wire 246: ends of other wires 0.12-0.16 mm off its
/// free-angle leg). O(own segments x others' ends).
pub fn feet_on(own: &Piece, others: &[Piece]) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::new();
    for o in others {
        let mut ends: Vec<(Vec2, Option<usize>, f64)> = Vec::new();
        for (l, pts) in o.runs {
            if let (Some(&s), Some(&e)) = (pts.first(), pts.last()) {
                ends.push((s, Some(*l), o.half_w));
                ends.push((e, Some(*l), o.half_w));
            }
        }
        ends.extend(o.vias.iter().map(|&(c, r)| (c, None, r)));
        for (p, layer, reach) in ends {
            for (l, pts) in own.runs {
                if layer.is_some_and(|x| x != *l) {
                    continue;
                }
                for s in pts.windows(2) {
                    let ab = s[1] - s[0];
                    let t = (p - s[0]).dot(ab) / ab.len2().max(1.0);
                    let q = s[0] + ab * t.clamp(0.0, 1.0);
                    if (q - p).len() <= reach + own.half_w + TOL && (q - s[0]).len() > TOL && (q - s[1]).len() > TOL && !out.iter().any(|&x| (x - q).len() <= TOL) {
                        out.push(q);
                    }
                }
            }
        }
    }
    out
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

    fn pc(runs: &[(usize, Vec<Vec2>)], half: f64) -> Piece<'_> {
        Piece { runs, vias: vec![], half_w: half * MM }
    }

    /// A trunk from pad A to pad B and a branch from pad C whose foot lies on the trunk: one piece;
    /// the trunk redrawn as an elbow that leaves the foot 1.6 mm off (main's qmtech JS1_2): two.
    #[test]
    fn a_branch_foot_off_its_trunk_splits_the_net() {
        let (a, b, c) = (v(0.0, 0.0), v(10.0, 5.0), v(6.0, -6.0));
        let (pa, pb, pcc) = (sq(a, 0.5), sq(b, 0.5), sq(c, 0.5));
        let one = LayerMask(1);
        let terms = [Term { poly: &pa, layers: one, center: a }, Term { poly: &pb, layers: one, center: b }, Term { poly: &pcc, layers: one, center: c }];
        let foot = v(6.0, 3.0);
        let trunk = vec![(0usize, vec![a, b])];
        let branch = vec![(0usize, vec![c, foot])];
        assert_eq!(components(&[pc(&trunk, 0.1), pc(&branch, 0.1)], &terms), 1);
        // The polish's elbow of the trunk: 45 degrees to (5, 5), then east: the foot is off it.
        let elbow = vec![(0usize, vec![a, v(5.0, 5.0), b])];
        let before = partition(&[pc(&trunk, 0.1), pc(&branch, 0.1)], &terms);
        let after = partition(&[pc(&elbow, 0.1), pc(&branch, 0.1)], &terms);
        assert_eq!(components(&[pc(&elbow, 0.1), pc(&branch, 0.1)], &terms), 2);
        assert!(splits(&before, &after));
        // The elbow with the foot kept as a vertex (fuse-octi r4 part 1): one piece again.
        let kept = vec![(0usize, vec![a, v(3.0, 3.0), foot, v(8.0, 5.0), b])];
        assert!(!splits(&before, &partition(&[pc(&kept, 0.1), pc(&branch, 0.1)], &terms)));
    }

    /// The r5 verifier's kicad-cli 10.0.8 probe (reports/vfy-fuse-octi-r5-20261121-0700, 3a): 0.2 mm
    /// trunk and branch, every free end on a pad. KiCad joins a foot 0.09 / 0.105 / 0.12 / 0.19 mm
    /// off the trunk's centre line and splits at 0.21 (copper apart); it joins two tracks that cross
    /// with no end on each other, and a branch via (d 0.6) whose centre is 0.25 off the trunk.
    #[test]
    fn contacts_follow_kicads_overlap_rule() {
        let (a, b) = (v(0.0, 0.0), v(10.0, 0.0));
        let (pa, pb) = (sq(a, 0.5), sq(b, 0.5));
        let one = LayerMask(1);
        let tip = v(5.0, 4.0);
        let pt = sq(tip, 0.5);
        let terms = [Term { poly: &pa, layers: one, center: a }, Term { poly: &pb, layers: one, center: b }, Term { poly: &pt, layers: one, center: tip }];
        let trunk = vec![(0usize, vec![a, b])];
        for (dy, want) in [(0.09, 1), (0.105, 1), (0.12, 1), (0.19, 1), (0.21, 2)] {
            let br = vec![(0usize, vec![v(5.0, dy), tip])];
            assert_eq!(components(&[pc(&trunk, 0.1), pc(&br, 0.1)], &terms), want, "foot {dy} mm off");
        }
        // XCROSS: a track from (5, -2) to the pad at (5, 4) crosses the trunk, no end on it.
        let pd = sq(v(5.0, -2.0), 0.5);
        let terms_x = [Term { poly: &pa, layers: one, center: a }, Term { poly: &pb, layers: one, center: b }, Term { poly: &pt, layers: one, center: tip }, Term { poly: &pd, layers: one, center: v(5.0, -2.0) }];
        let x = vec![(0usize, vec![v(5.0, -2.0), tip])];
        assert_eq!(components(&[pc(&trunk, 0.1), pc(&x, 0.1)], &terms_x), 1, "crossing tracks");
        // VMID: the branch on the other layer ends in a via 0.25 off the trunk (radius 0.3): joined.
        let br = vec![(1usize, vec![v(5.0, 0.25), tip])];
        let pt2 = sq(tip, 0.5);
        let terms2 = [Term { poly: &pa, layers: one, center: a }, Term { poly: &pb, layers: one, center: b }, Term { poly: &pt2, layers: LayerMask(3), center: tip }];
        let via = Piece { runs: &br, vias: vec![(v(5.0, 0.25), 0.3 * MM)], half_w: 0.1 * MM };
        assert_eq!(components(&[pc(&trunk, 0.1), via], &terms2), 1, "overlapping via");
        // Without the via the other layer's end does not reach the trunk.
        assert_eq!(components(&[pc(&trunk, 0.1), pc(&br, 0.1)], &terms2), 2, "other layer, no via");
        // A pad on layer 1 only: the trunk's end on layer 0 inside its outline does not reach it.
        let terms3 = [Term { poly: &pa, layers: one, center: a }, Term { poly: &pb, layers: LayerMask(2), center: b }];
        assert_eq!(components(&[pc(&trunk, 0.1)], &terms3), 2);
        // A track ending 0.05 mm short of a pad's edge (half width 0.1) overlaps it.
        let short = vec![(0usize, vec![a, v(9.45, 0.0)])];
        assert_eq!(components(&[pc(&short, 0.1)], &terms[..2]), 1);
        assert_eq!(components(&[], &terms[..2]), 2, "nothing drawn: every pad a piece");
    }

    /// The feet of other copper on a wire: a run end 0.12 mm off a segment (reach 0.2), a via 0.35
    /// off (radius 0.3 + 0.1), each projected onto the segment; an end at the segment's own end, one
    /// beyond reach and one on another layer are not.
    #[test]
    fn feet_of_other_copper_are_projected_onto_the_segment() {
        let own_runs = vec![(0usize, vec![v(0.0, 0.0), v(10.0, 0.0)])];
        let own = pc(&own_runs, 0.1);
        let r1 = vec![(0usize, vec![v(3.0, 0.12), v(3.0, 4.0)])];
        let r2 = vec![(1usize, vec![v(6.0, 0.35), v(6.0, 4.0)])];
        let r3 = vec![(0usize, vec![v(8.0, 0.25), v(8.0, 4.0)]), (1usize, vec![v(9.0, 0.0), v(9.0, 4.0)]), (0usize, vec![v(10.0, 0.0), v(10.0, 3.0)])];
        let others = [pc(&r1, 0.1), Piece { runs: &r2, vias: vec![(v(6.0, 0.35), 0.3 * MM)], half_w: 0.1 * MM }, pc(&r3, 0.1)];
        let f = feet_on(&own, &others);
        assert_eq!(f.len(), 2, "{f:?}");
        assert!((f[0] - v(3.0, 0.0)).len() < 1.0 && (f[1] - v(6.0, 0.0)).len() < 1.0, "{f:?}");
    }

    /// A branch whose foot is left 2 mm off the trunk but which still crosses it: one piece, yet a
    /// new dangling end (KiCad Cleanup "dangling track"); an end on a pad, on the own via or within
    /// the sum of half widths of other copper is not dangling.
    #[test]
    fn a_foot_off_its_trunk_dangles_even_where_the_branch_crosses_it() {
        let (a, b, c) = (v(0.0, 0.0), v(10.0, 0.0), v(5.0, 4.0));
        let ps: Vec<Vec<Vec2>> = [a, b, c].iter().map(|&p| sq(p, 0.5)).collect();
        let terms: Vec<Term> = ps.iter().zip([a, b, c]).map(|(p, ce)| Term { poly: p, layers: LayerMask(1), center: ce }).collect();
        let trunk = vec![(0usize, vec![a, b])];
        let on = vec![(0usize, vec![c, v(5.0, 0.15)])];
        let past = vec![(0usize, vec![c, v(5.0, -2.0)])];
        let before = conn(&[pc(&trunk, 0.1), pc(&on, 0.1)], &terms);
        let after = conn(&[pc(&trunk, 0.1), pc(&past, 0.1)], &terms);
        assert!(before.dangling.is_empty(), "{:?}", before.dangling);
        assert_eq!((after.n(), after.dangling.iter().copied().collect::<Vec<_>>()), (1, vec![(1, 0, 1)]));
        assert!(!splits(&before.part, &after.part) && worse(&before, &after));
        assert!(!worse(&after, &before), "joining a dangling end is no worse");
        // A run that ends in its own via (the next run goes on from it) does not dangle.
        let two = vec![(0usize, vec![c, v(5.0, 1.0)]), (1usize, vec![v(5.0, 1.0), v(5.0, 0.0)])];
        let vp = Piece { runs: &two, vias: vec![(v(5.0, 1.0), 0.3 * MM)], half_w: 0.1 * MM };
        let d = dangling(&[pc(&trunk, 0.1), vp], &terms);
        assert_eq!(d.iter().copied().collect::<Vec<_>>(), vec![(1, 1, 1)], "only the layer-1 end at (5, 0): no layer-1 copper there");
    }

    /// Partitions, not counts (r5 verifier M2): a step that cuts the branch to C off and joins the
    /// stub to D has as many pieces as before, yet splits what was joined.
    #[test]
    fn a_step_that_cuts_one_piece_off_and_joins_another_splits() {
        let (a, b, c, d) = (v(0.0, 0.0), v(10.0, 0.0), v(5.0, 4.0), v(8.0, -4.0));
        let ps: Vec<Vec<Vec2>> = [a, b, c, d].iter().map(|&p| sq(p, 0.5)).collect();
        let one = LayerMask(1);
        let terms: Vec<Term> = ps.iter().zip([a, b, c, d]).map(|(p, ce)| Term { poly: p, layers: one, center: ce }).collect();
        let trunk = vec![(0usize, vec![a, b])];
        let to_c = vec![(0usize, vec![v(5.0, 0.0), c])];
        let to_d_off = vec![(0usize, vec![v(8.0, -1.0), d])];
        let to_c_off = vec![(0usize, vec![v(5.0, 1.0), c])];
        let to_d = vec![(0usize, vec![v(8.0, 0.0), d])];
        let before = partition(&[pc(&trunk, 0.1), pc(&to_c, 0.1), pc(&to_d_off, 0.1)], &terms);
        let after = partition(&[pc(&trunk, 0.1), pc(&to_c_off, 0.1), pc(&to_d, 0.1)], &terms);
        let n = |p: &[usize]| (0..p.len()).filter(|&x| p[x] == x).count();
        assert_eq!(n(&before), n(&after), "the count does not see it");
        assert!(splits(&before, &after), "C was joined to the trunk, now it is not");
        assert!(!splits(&after, &after) && !splits(&before, &partition(&[pc(&trunk, 0.1), pc(&to_c, 0.1), pc(&to_d, 0.1)], &terms)), "a join alone splits nothing");
    }
}
