//! Pour check of the steps after drawing (TW2, qmtech MAX_I2S0 forensics 2026-11-18): the fuse
//! (D6b, [`super::draw::fuse_candidates`]) moves a branch end onto its trunk's copper where that
//! is shorter and legal, and the octilinear polish ([`super::draw::Drawer::octi_polish`]) redraws
//! the segments not at 0/45/90 degrees as an elbow or Z. Neither saw the pour pieces its new
//! copper cuts off: on qmtech the reduced, fused and polished copper left 21 GND pieces the
//! stitching could not join against 20 of the copper as drawn (the fuse alone left 19, the
//! polish after it 21; pieces beside the ADV7511 and at C9.2), so that variant counted 44 errors
//! against 43 and the variant table picked the one without the via reduction (+49 mm, +3 vias).
//!
//! The check judges these moves as a whole by the variant table's first criterion (errors;
//! "a design-rule violation cannot be compensated by any reduction of wire length", Polubasov
//! 2010; TopoR 6.3 manual p. 72: variants sorted by violations, then by length): the copper with
//! and without the fuse moves is stitched ([`super::stitch::stitch_until`], the same pass the
//! variant table runs), and when the moved copper leaves more pieces cut off, the moves nearest
//! the pieces only it leaves are undone (with the moves that depend on them), at most
//! [`ROUNDS`] times; still worse, every move is undone. Length gain stays the tie-break: a fuse
//! that leaves no more pieces cut off is kept whole. The pure parts live here (unit tests); the
//! saved wire states and the undo are in [`super::core`].

use crate::geom::{point_seg, Seg, Vec2};
use crate::model::BoardModel;
use crate::pourcheck::Cut;
use std::collections::BTreeSet;

/// Two pads, vias or pad-less centroids closer than this (nm) are the same place: a matching
/// tolerance between two stitchings of nearly the same copper (a piece's centroid moves a little
/// when a neighbouring track moves), not a routing judgement. The rounds of "undo the nearest
/// moves, stitch again" are `[search] fuse_pour_rounds`.
const SAME_NM: f64 = 1_000_000.0;

/// No wire: the `onto` of a move that joins no other wire (a polish).
pub const NO_WIRE: usize = usize::MAX;

/// One move after drawing: wire `w` fused one end onto wire `onto` (a polish: [`NO_WIRE`]);
/// `path` = polylines (math frame) where its copper changed.
#[derive(Clone, Debug, Default)]
pub struct Move {
    pub w: usize,
    pub onto: usize,
    pub path: Vec<Vec<Vec2>>,
}

/// The new copper of a redrawn wire: the segments of `new` runs with an end that is no vertex of
/// the `old` run on the same index (2-point polylines). O(points log points).
pub fn changed(old: &[(usize, Vec<Vec2>)], new: &[(usize, Vec<Vec2>)]) -> Vec<Vec<Vec2>> {
    let key = |p: &Vec2| (p.x.to_bits(), p.y.to_bits());
    let mut out = Vec::new();
    for (r, (_, pts)) in new.iter().enumerate() {
        let had: BTreeSet<(u64, u64)> = old.get(r).map(|(_, o)| o.iter().map(key).collect()).unwrap_or_default();
        for x in pts.windows(2) {
            if !had.contains(&key(&x[0])) || !had.contains(&key(&x[1])) {
                out.push(vec![x[0], x[1]]);
            }
        }
    }
    out
}

/// `a` and `b` are the same cut-off piece in two stitchings of slightly different copper: one
/// net, and a pad in common, else a via within [`SAME_NM`], else (neither holds a pad or a via)
/// centroids within [`SAME_NM`]. O(pads log pads + vias²).
pub fn same_piece(a: &Cut, b: &Cut) -> bool {
    if a.net != b.net {
        return false;
    }
    if a.pads.iter().any(|p| b.pads.binary_search(p).is_ok()) {
        return true;
    }
    if a.vias.iter().any(|&v| b.vias.iter().any(|&u| (u - v).len() < SAME_NM)) {
        return true;
    }
    a.pads.is_empty() && b.pads.is_empty() && a.vias.is_empty() && b.vias.is_empty() && (a.at - b.at).len() < SAME_NM
}

/// The pieces of `fused` that `unfused` does not leave cut off. O(F x U x piece).
pub fn new_pieces<'a>(fused: &'a [Cut], unfused: &[Cut]) -> Vec<&'a Cut> {
    fused.iter().filter(|c| !unfused.iter().any(|u| same_piece(c, u))).collect()
}

/// Where a piece is (math frame): its pads' centres, its vias, and its islands' centroid.
pub fn piece_points(model: &BoardModel, c: &Cut) -> Vec<Vec2> {
    let mut pts: Vec<Vec2> = c.pads.iter().map(|&p| model.pads[p].center.to_vec2()).collect();
    pts.extend(c.vias.iter().copied());
    if !c.islands.is_empty() {
        pts.push(c.at);
    }
    pts
}

/// Distance from `p` to the polyline `path` (a single point: to it). O(path).
fn dist_path(p: Vec2, path: &[Vec2]) -> f64 {
    match path {
        [] => f64::INFINITY,
        [q] => (p - *q).len(),
        _ => path.windows(2).map(|x| point_seg(p, &Seg { a: x[0], b: x[1] })).fold(f64::INFINITY, f64::min),
    }
}

/// The live move (`live[i]`) whose changed copper comes nearest to any of `pts`; ties to the
/// earlier move. `None`: no live move or no point. O(moves x path x pts).
pub fn nearest_move(moves: &[Move], live: &[bool], pts: &[Vec2]) -> Option<usize> {
    let mut best: Option<(f64, usize)> = None;
    for (i, m) in moves.iter().enumerate().filter(|(i, _)| live[*i]) {
        let d = pts.iter().flat_map(|&p| m.path.iter().map(move |q| dist_path(p, q))).fold(f64::INFINITY, f64::min);
        if d.is_finite() && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

/// `seed` with every move that depends on one in it: a later move of the same wire (its saved
/// state holds the earlier move) and a later move onto that wire (its end lies on the copper the
/// undo takes back), to the fixpoint. Moves are in the order they were made. O(moves²).
pub fn with_dependents(moves: &[Move], seed: &BTreeSet<usize>) -> BTreeSet<usize> {
    let mut set = seed.clone();
    loop {
        let mut grew = false;
        for j in 0..moves.len() {
            if set.contains(&j) {
                continue;
            }
            if set.iter().any(|&i| i < j && (moves[j].w == moves[i].w || moves[j].onto == moves[i].w)) {
                set.insert(j);
                grew = true;
            }
        }
        if !grew {
            return set;
        }
    }
}

/// The moves to undo after a partial undo (verify V3): the moves kept were legality-checked
/// against the copper as it was then, with the undone moves' copper in place; the copper that
/// comes back may clash with them. For each wire with a kept move whose copper `clashes(wire)`
/// against the drawing now, its earliest kept move (the later ones of that wire depend on it,
/// [`with_dependents`]). The caller undoes them and asks again until none clashes: each round
/// undoes at least one move, and the copper with no move kept is the drawing's own, so this ends.
/// O(moves + wires x clashes).
pub fn clash_seed(moves: &[Move], live: &[bool], clashes: &dyn Fn(usize) -> bool) -> BTreeSet<usize> {
    let mut first: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    for (i, m) in moves.iter().enumerate().filter(|(i, _)| live[*i]) {
        first.entry(m.w).or_insert(i);
    }
    first.into_iter().filter(|(w, _)| clashes(*w)).map(|(_, i)| i).collect()
}

/// The re-check of the moves kept after a partial undo (verify V3): the moves to undo next,
/// `viols(wire)` being the rule violations of the wire's tracks now and `pre` those of each wire
/// with a kept move before the undo (no entry = 0). A wire clashes when its count went UP: its own
/// forced violations (never drop) are in both counts and undo nothing, but one an undone move had
/// cleared and the undo brings back raises the count, which counts as a clash (safe: undoing goes
/// back toward the drawing's own copper). The earliest kept move of each clashing wire, with
/// [`with_dependents`]. O(moves + wires x viols).
pub fn recheck_seed(moves: &[Move], live: &[bool], pre: &std::collections::BTreeMap<usize, usize>, viols: &dyn Fn(usize) -> usize) -> BTreeSet<usize> {
    with_dependents(moves, &clash_seed(moves, live, &|w| viols(w) > pre.get(&w).copied().unwrap_or(0)))
}

/// The moved copper is kept as it is when it leaves no more pieces cut off than the copper
/// without the moves (the errors criterion first; length decides the rest). O(1).
pub fn fused_ok(fused_left: usize, unfused_left: usize) -> bool {
    fused_left <= unfused_left
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x: x * 1e6, y: y * 1e6 }
    }

    fn cut(net: i32, pads: &[usize], at: Vec2) -> Cut {
        Cut { net, pads: pads.to_vec(), islands: vec![(0, 0, 1)], at, ..Cut::default() }
    }

    #[test]
    fn a_piece_is_matched_by_a_pad_a_via_or_its_place() {
        let a = cut(1, &[3, 7], v(0.0, 0.0));
        assert!(same_piece(&a, &cut(1, &[7], v(9.0, 9.0))), "a pad in common");
        assert!(!same_piece(&a, &cut(2, &[7], v(0.0, 0.0))), "another net");
        assert!(!same_piece(&a, &cut(1, &[4], v(0.0, 0.0))), "pads, none in common");
        let island = |at| cut(1, &[], at);
        assert!(same_piece(&island(v(1.0, 1.0)), &island(v(1.5, 1.0))));
        assert!(!same_piece(&island(v(1.0, 1.0)), &island(v(3.0, 1.0))));
        let via = |at| Cut { vias: vec![at], ..cut(1, &[], v(50.0, 50.0)) };
        assert!(same_piece(&via(v(5.0, 5.0)), &via(v(5.2, 5.0))));
    }

    #[test]
    fn only_the_pieces_the_fuse_adds_are_new() {
        // qmtech: as drawn leaves X2.2 cut off, the fused copper ADV7511.31 and C9.2 instead.
        let unfused = [cut(1, &[10], v(0.0, 0.0)), cut(1, &[20], v(5.0, 0.0))];
        let fused = [cut(1, &[20], v(5.0, 0.0)), cut(1, &[30], v(9.0, 0.0)), cut(1, &[40], v(12.0, 0.0))];
        let new: Vec<Vec<usize>> = new_pieces(&fused, &unfused).iter().map(|c| c.pads.clone()).collect();
        assert_eq!(new, vec![vec![30], vec![40]]);
        assert!(!fused_ok(fused.len(), unfused.len()));
        assert!(fused_ok(2, 2), "as many pieces: the fuse stays (length decides)");
    }

    #[test]
    fn the_move_nearest_the_new_piece_is_undone_first() {
        let moves = [
            Move { w: 1, onto: 9, path: vec![vec![v(0.0, 0.0), v(10.0, 0.0)]] },
            Move { w: 2, onto: NO_WIRE, path: vec![vec![v(0.0, 9.0)], vec![v(0.0, 5.0), v(10.0, 5.0)]] },
            Move { w: 3, onto: 9, path: vec![vec![v(0.0, 2.0), v(10.0, 2.0)]] },
        ];
        let pts = [v(5.0, 4.0)];
        assert_eq!(nearest_move(&moves, &[true, true, true], &pts), Some(1));
        assert_eq!(nearest_move(&moves, &[true, false, true], &pts), Some(2), "an undone move is not a candidate");
        assert_eq!(nearest_move(&moves, &[false, false, false], &pts), None);
        assert_eq!(nearest_move(&moves, &[true, true, true], &[]), None);
    }

    #[test]
    fn a_clash_undoes_the_earliest_kept_move_of_the_clashing_wire() {
        let moves = [
            Move { w: 1, onto: 9, ..Move::default() },
            Move { w: 2, onto: 9, ..Move::default() },
            Move { w: 2, onto: NO_WIRE, ..Move::default() },
            Move { w: 3, onto: NO_WIRE, ..Move::default() },
        ];
        // Move 0 undone; wire 2's copper clashes with what came back: its first kept move.
        assert_eq!(clash_seed(&moves, &[false, true, true, true], &|w| w == 2), BTreeSet::from([1]));
        assert_eq!(clash_seed(&moves, &[false, false, true, true], &|w| w == 2), BTreeSet::from([2]));
        assert!(clash_seed(&moves, &[false, true, true, true], &|_| false).is_empty());
        assert!(clash_seed(&moves, &[false, true, true, true], &|w| w == 1).is_empty(), "wire 1 has no kept move");
    }

    #[test]
    fn the_recheck_undoes_a_wire_only_where_its_violations_went_up() {
        use std::collections::BTreeMap;
        let moves = [
            Move { w: 1, onto: 9, ..Move::default() },
            Move { w: 2, onto: 9, ..Move::default() },
            Move { w: 3, onto: 2, ..Move::default() }, // onto wire 2: depends on its move
            Move { w: 4, onto: 9, ..Move::default() },
        ];
        let live = [true; 4];
        let pre = BTreeMap::from([(1, 0), (2, 0), (3, 0), (4, 2)]);
        let seed = |now: [usize; 5]| recheck_seed(&moves, &live, &pre, &|w| now[w]);
        // Nothing went up: none.
        assert!(seed([0, 0, 0, 0, 2]).is_empty());
        // A real clash (0 -> 1) is undone, the move onto it with it (v1: comparison false).
        assert_eq!(seed([0, 0, 1, 0, 2]), BTreeSet::from([1, 2]));
        // Wire 4's own forced violations (2 before, 2 now) undo nothing (v2: compare against 0).
        assert!(seed([0, 0, 0, 0, 2]).is_empty());
        // Fewer than before: none. One more than before: undone.
        assert!(seed([0, 0, 0, 0, 1]).is_empty());
        assert_eq!(seed([0, 0, 0, 0, 3]), BTreeSet::from([3]));
        // The own-copper case: an undone move had cleared wire 1's forced violations, the undo
        // brings them back (0 -> 2): a clash, wire 1's kept move is undone too.
        assert_eq!(seed([0, 2, 0, 0, 2]), BTreeSet::from([0]));
        // A wire with no entry in `pre` counts from 0; an undone move is not a candidate.
        assert_eq!(recheck_seed(&moves, &live, &BTreeMap::new(), &|w| usize::from(w == 4)), BTreeSet::from([3]));
        assert!(recheck_seed(&moves, &[true, true, true, false], &pre, &|_| 9).contains(&0));
        assert!(!recheck_seed(&moves, &[true, true, true, false], &pre, &|_| 9).contains(&3));
    }

    #[test]
    fn a_polish_moves_only_the_segments_it_redrew() {
        let old = vec![(0usize, vec![v(0.0, 0.0), v(4.0, 3.0), v(10.0, 3.0)])];
        let new = vec![(0usize, vec![v(0.0, 0.0), v(1.0, 0.0), v(4.0, 3.0), v(10.0, 3.0)])];
        assert_eq!(changed(&old, &new), vec![vec![v(0.0, 0.0), v(1.0, 0.0)], vec![v(1.0, 0.0), v(4.0, 3.0)]]);
        assert!(changed(&old, &old).is_empty());
    }

    #[test]
    fn undoing_a_move_takes_the_moves_that_depend_on_it() {
        let moves = [
            Move { w: 1, onto: 9, ..Move::default() },
            Move { w: 2, onto: 1, ..Move::default() }, // fused onto wire 1's moved copper
            Move { w: 1, onto: 8, ..Move::default() }, // wire 1's other end, saved after move 0
            Move { w: 3, onto: 2, ..Move::default() }, // onto wire 2: depends on move 1
            Move { w: 4, onto: 9, ..Move::default() },
            Move { w: 3, onto: NO_WIRE, ..Move::default() }, // wire 3's polish: depends on move 3
        ];
        let got = with_dependents(&moves, &BTreeSet::from([0]));
        assert_eq!(got, BTreeSet::from([0, 1, 2, 3, 5]));
        // A move made before the seed does not depend on it.
        assert_eq!(with_dependents(&moves, &BTreeSet::from([2])), BTreeSet::from([2]));
    }
}
