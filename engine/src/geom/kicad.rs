//! KiCad's polygon approximation of a track (`PCB_TRACK::TransformShapeToPolygon` with
//! `ERROR_INSIDE`, KiCad 10.0.8: `TransformOvalToPolygon`, `GetArcToSegmentCount`), for the tests
//! KiCad's Cleanup makes on it. `TRACKS_CLEANER::deleteTracksInPads` deletes a track whose two
//! ends hit a pad of its net and whose polygon minus the pad's polygon is empty; the polygon's
//! round ends are inscribed in the true ones and lie up to `r (1 - cos(pi / n))` inside them,
//! which is more than the board's maximum error where the segment count rounds down (a 0.1407 mm
//! track at 5 um: n = 8, 5.35 um; qmtech ADV7511KSTZ1.29, verifier vfy-draw-via-neck-r6 F1).
//!
//! The model was fitted and checked against pcbnew's own polygon (scratch probe, 400 random
//! segments: widths 0.05-1.5 mm, maximum errors 1-10 um, any angle; every vertex within 2 nm):
//! n = max(round(360 / inc), 2) per full circle with inc = min(45, 2 acos(1 - err / r)) degrees
//! (45 when err >= r), rounded up to a multiple of 8; each round end has the two points
//! perpendicular to the segment at its end and n / 2 points at +-(step / 2 + k step) from the
//! end's outward direction (step = 360 / n degrees, k = 0 .. n / 4 - 1), all at the radius.
use super::Vec2;

/// Segments of a full circle of radius `r` (nm) at the maximum error `err` (nm), as KiCad's
/// round track ends have them (see the module doc). O(1).
pub fn circle_segments(r: f64, err: f64) -> usize {
    let err = err.max(1.0);
    let inc = if err < r { (2.0 * (1.0 - err / r).acos().to_degrees()).min(45.0) } else { 45.0 };
    let n = ((360.0 / inc + 0.5).floor() as usize).max(2);
    n.div_ceil(8) * 8
}

/// The vertices of KiCad's polygon of the track `a`-`b` of width `w` at the maximum error `err`
/// (any frame: the vertex set is symmetric about the segment). A null segment: the `n` points
/// of a circle round `a` from angle 0 (KiCad draws a circle there; its phase is not modelled,
/// see [`track_inside`]). O(n).
pub fn track_polygon(a: Vec2, b: Vec2, w: f64, err: f64) -> Vec<Vec2> {
    let r = 0.5 * w;
    let n = circle_segments(r, err);
    let step = std::f64::consts::TAU / n as f64;
    let d = b - a;
    let l = d.len();
    let at = |c: Vec2, th: f64| Vec2 { x: c.x + r * th.cos(), y: c.y + r * th.sin() };
    if l < 1.0 {
        return (0..n).map(|k| at(a, k as f64 * step)).collect();
    }
    let mut out = Vec::with_capacity(n + 4);
    for (c, base) in [(b, d.y.atan2(d.x)), (a, (-d.y).atan2(-d.x))] {
        out.push(at(c, base + std::f64::consts::FRAC_PI_2));
        out.push(at(c, base - std::f64::consts::FRAC_PI_2));
        for k in 0..n / 4 {
            let t = 0.5 * step + k as f64 * step;
            out.push(at(c, base + t));
            out.push(at(c, base - t));
        }
    }
    out
}

/// KiCad's `deleteTracksInPads` test of the track `a`-`b` (width `w`, maximum error `err`)
/// against a convex pad given by `depth` (signed distance into the pad's copper, nm; negative
/// outside): both ends in the pad (`PAD::HitTest`) and every vertex of the track's polygon
/// within it, a vertex up to `slack` nm outside counted as inside. The pad is taken at its true
/// shape: KiCad's `ERROR_INSIDE` pad polygon lies inside it, so this answers "inside" wherever
/// KiCad does (and may where a vertex falls between a pad's rounded corner and its polygon). A
/// null segment: its round end's inscribed circle `r cos(pi / n)` in the pad (KiCad's circle
/// phase is not modelled; inside wherever KiCad's is). O(n) depth calls.
pub fn track_inside(a: Vec2, b: Vec2, w: f64, err: f64, depth: impl Fn(Vec2) -> f64, slack: f64) -> bool {
    if depth(a) < -slack || depth(b) < -slack {
        return false;
    }
    if (b - a).len() < 1.0 {
        let n = circle_segments(0.5 * w, err);
        return depth(a) >= 0.5 * w * (std::f64::consts::PI / n as f64).cos() - slack;
    }
    track_polygon(a, b, w, err).into_iter().all(|q| depth(q) >= -slack)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x, y }
    }

    /// Segment counts KiCad 10.0.8 gives (pcbnew probe): 8 up to r ~ 74 um at 5 um, 16 above;
    /// a radius below the error: 8.
    #[test]
    fn circle_segments_match_kicad() {
        for (w, err, n) in [(140_700.0, 5_000.0, 8), (100_000.0, 5_000.0, 8), (250_000.0, 5_000.0, 16), (280_000.0, 5_000.0, 16), (500_000.0, 5_000.0, 16), (5_000.0, 5_000.0, 8), (1_000_000.0, 5_000.0, 24)] {
            assert_eq!(circle_segments(0.5 * w, err), n, "w {w} err {err}");
        }
    }

    /// pcbnew's vertices of a 0.1407 mm track from (0, 0) along +x (probe): the end caps at
    /// +-90, +-112.5 and +-157.5 degrees round the start, all at the radius.
    #[test]
    fn track_polygon_matches_kicad_vertices() {
        let p = track_polygon(v(0.0, 0.0), v(500_000.0, 0.0), 140_700.0, 5_000.0);
        assert_eq!(p.len(), 12);
        let mut start: Vec<i64> = p.iter().filter(|q| q.x < 250_000.0).map(|q| (q.y.atan2(q.x).to_degrees() * 10.0).round() as i64).collect();
        start.sort_unstable();
        assert_eq!(start, vec![-1575, -1125, -900, 900, 1125, 1575]);
        assert!(p.iter().all(|&q| ((q - v(0.0, 0.0)).len() - 70_350.0).abs() < 1e-6 || ((q - v(500_000.0, 0.0)).len() - 70_350.0).abs() < 1e-6));
    }

    /// The verifier's case (qmtech ADV7511KSTZ1.29, pad 0.3 x 1.6 mm at the origin here): the
    /// 0.1407 mm 45-degree jog from the centre to (-0.084989, 0.084989) mm, 65.011 um from the
    /// pad's long side. Its true round end sticks out by 5.34 um (more than the 5 um error: the
    /// old tidy tolerance kept it), KiCad's polygon does not (its vertices 22.5 degrees off the
    /// side's normal reach 64.996 um): KiCad deletes it, and so does this test. 1 um further out
    /// it sticks out of KiCad's polygon too.
    #[test]
    fn the_adv7511_jog_is_inside_as_kicad_judges_it() {
        let depth = |q: Vec2| (150_000.0 - q.x.abs()).min(800_000.0 - q.y.abs());
        let (c, j, w) = (v(0.0, 0.0), v(-84_989.0, 84_989.0), 140_700.0);
        assert!(depth(j) - 0.5 * w < -5_000.0, "the true round end sticks out past the old 5 um tolerance");
        assert!(track_inside(c, j, w, 5_000.0, depth, 2.0), "KiCad deletes it");
        assert!(!track_inside(c, v(-86_000.0, 86_000.0), w, 5_000.0, depth, 2.0));
        assert!(!track_inside(c, v(-84_989.0, 900_000.0), w, 5_000.0, depth, 2.0), "an end outside the pad");
        // a null segment deep in the pad, and one at the edge
        assert!(track_inside(c, c, w, 5_000.0, depth, 2.0));
        assert!(!track_inside(v(-100_000.0, 0.0), v(-100_000.0, 0.0), w, 5_000.0, depth, 2.0));
    }
}
