//! Fuse pour check (TW2, qmtech MAX_I2S0 forensics 2026-11-18): the fuse after drawing (D6b,
//! [`super::draw::fuse_candidates`]) moves a branch end onto its trunk's copper where that is
//! shorter and legal. It never saw the pour pieces its new copper cuts off: on qmtech two moves
//! of 0.4 mm length gain each narrowed a GND pour piece beside the ADV7511 (4.80 -> 1.59 mm²)
//! and cut a capacitor pad off its piece, the stitching found no site for either, and the fused
//! variant counted one error more than the copper as drawn (43 vs 44: the variant table then
//! picked the variant without the via reduction, +49 mm and +3 vias).
//!
//! The check judges the fuse as a whole by the variant table's first criterion (errors;
//! "a design-rule violation cannot be compensated by any reduction of wire length", Polubasov
//! 2010; TopoR 6.3 manual p. 72: variants sorted by violations, then by length): the copper with
//! and without the fuse moves is stitched ([`super::stitch::stitch_until`], the same pass the
//! variant table runs), and when the fused copper leaves more pieces cut off, the moves nearest
//! the pieces only it leaves are undone (with the moves that depend on them), at most
//! [`ROUNDS`] times; still worse, every move is undone. Length gain stays the tie-break: a fuse
//! that leaves no more pieces cut off is kept whole. The pure parts live here (unit tests); the
//! saved wire states and the undo are in [`super::core`].

use crate::geom::{point_seg, Seg, Vec2};
use crate::model::BoardModel;
use crate::pourcheck::Cut;
use std::collections::BTreeSet;

/// Rounds of "undo the moves nearest the new pieces, stitch again" before every move is undone.
pub const ROUNDS: usize = 2;

/// Two pads, vias or pad-less centroids closer than this (nm) are the same place.
const SAME_NM: f64 = 1_000_000.0;

/// One fuse move: wire `w` fused one end onto wire `onto`; `path` = the points of its run before
/// and after the move (math frame), where its copper changed.
#[derive(Clone, Debug, Default)]
pub struct Move {
    pub w: usize,
    pub onto: usize,
    pub path: Vec<Vec2>,
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
        let d = pts.iter().map(|&p| dist_path(p, &m.path)).fold(f64::INFINITY, f64::min);
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

/// The fused copper is kept as it is when it leaves no more pieces cut off than the copper
/// without the fuse (the errors criterion first; length decides the rest). O(1).
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
            Move { w: 1, onto: 9, path: vec![v(0.0, 0.0), v(10.0, 0.0)] },
            Move { w: 2, onto: 9, path: vec![v(0.0, 5.0), v(10.0, 5.0)] },
            Move { w: 3, onto: 9, path: vec![v(0.0, 2.0), v(10.0, 2.0)] },
        ];
        let pts = [v(5.0, 4.0)];
        assert_eq!(nearest_move(&moves, &[true, true, true], &pts), Some(1));
        assert_eq!(nearest_move(&moves, &[true, false, true], &pts), Some(2), "an undone move is not a candidate");
        assert_eq!(nearest_move(&moves, &[false, false, false], &pts), None);
        assert_eq!(nearest_move(&moves, &[true, true, true], &[]), None);
    }

    #[test]
    fn undoing_a_move_takes_the_moves_that_depend_on_it() {
        let moves = [
            Move { w: 1, onto: 9, ..Move::default() },
            Move { w: 2, onto: 1, ..Move::default() }, // fused onto wire 1's moved copper
            Move { w: 1, onto: 8, ..Move::default() }, // wire 1's other end, saved after move 0
            Move { w: 3, onto: 2, ..Move::default() }, // onto wire 2: depends on move 1
            Move { w: 4, onto: 9, ..Move::default() },
        ];
        let got = with_dependents(&moves, &BTreeSet::from([0]));
        assert_eq!(got, BTreeSet::from([0, 1, 2, 3]));
        // A move made before the seed does not depend on it.
        assert_eq!(with_dependents(&moves, &BTreeSet::from([2])), BTreeSet::from([2]));
    }
}
