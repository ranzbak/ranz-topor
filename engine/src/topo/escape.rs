//! TW5 ESCAPE (docs/dev/design/tw5-escape.md): the parts of the TopoR loop's escapes that do not
//! need the loop's state, so each is tested on its own. The loop itself is in [`super::core`].
//!
//! * [`bounded_amp`]: the per-wire amplitude of the oscillating coefficients, graded by how bad
//!   the wire's path and surroundings are, bounded, and never cooled (Polubasov 2010
//!   `docs/literature/polubasov2010-topor-inside.html` «Перекладка проводников»: the coefficients
//!   "are chosen automatically for every wire being rerouted (depending on the quality of its path
//!   and its surroundings) and they oscillate within some limits").
//! * [`Throttle`]: when the next layering escape runs, in deterministic work units, so it takes at
//!   most a share `s` of the loop's work (same source, «Оптимизация расслоения»: the layer
//!   optimisation runs "not sooner than after a time equal to its own last duration", so at most
//!   half of the time).
//! * [`History`]: the fading history price of edges that stay over-full inside the loop
//!   (McMurchie & Ebeling 1995, PathFinder, FPGA '95 pp. 111-117: eq. 1 p. 113 `c_n = (b_n + h_n)
//!   p_n`, the history `h_n` grows in every iteration the resource is over-used; p. 112 the
//!   penalty is "gradually increased"); here it also fades, so it is bounded and a cut that
//!   empties is forgotten.
//! * [`free_diff`]: how many free positions two layerings put on different layers (the escape's
//!   meter: TW5 asks for >= 20 % at equal vias).
//!
//! Graded factors throughout: no threshold decides; every value grows or falls smoothly.

/// The amplitude of a wire's coefficients (TW5 a): `lo + (hi - lo) x b / (1 + b)` for the badness
/// `b >= 0` (0 = a straight wire in free surroundings that keeps gaining: `lo`; a wire far from its
/// straight line, in crowded cuts, whose tries keep failing: towards `hi`). Bounded in `[lo, hi]`
/// (`hi < lo` is read as `lo = hi`), the same in every cycle (no cooling). O(1).
pub fn bounded_amp(lo: f64, hi: f64, b: f64) -> f64 {
    let (lo, hi) = (lo.max(0.0), hi.max(lo.max(0.0)));
    let b = if b.is_finite() { b.max(0.0) } else { 0.0 };
    lo + (hi - lo) * b / (1.0 + b)
}

/// The badness of a wire for [`bounded_amp`] (TW5 a): its detour `len / straight - 1` (0 for a
/// straight wire), the mean loop history on its passes (`crowd`, [`History`]) and its fruitless
/// tries in a row as `stuck / (1 + stuck)`. Each term >= 0, unbounded terms enter as they are (the
/// amplitude itself is bounded). O(1).
pub fn badness(len: f64, straight: f64, crowd: f64, stuck: u32) -> f64 {
    let detour = if straight > 0.0 && len.is_finite() { (len / straight - 1.0).max(0.0) } else { 0.0 };
    let s = f64::from(stuck);
    detour + crowd.max(0.0) + s / (1.0 + s)
}

/// When the next layering escape runs (TW5 b), counted in work units (deterministic: the copper
/// never depends on the clock or the thread count). A global layering costs `kappa` units per
/// alive wire; a re-lay costs the wires its local judge layered and measured (or every alive wire
/// when it was judged globally). After an escape of work `W` the next one is due when the re-lays
/// since then add up to `W x (1/s - 1)`, so escapes take at most the share `s` of the work
/// (`s <= 0.5`: not sooner than its own last duration). The first one is due after as much re-lay
/// work as if one had just run.
#[derive(Clone, Debug, PartialEq)]
pub struct Throttle {
    pub share: f64,
    pub kappa: f64,
    /// Re-lay work since the last escape, and the work at which the next one is due.
    pub since: f64,
    pub need: f64,
    /// Escapes run and their work, re-lay work in all (the measured share).
    pub runs: usize,
    pub esc_work: f64,
    pub relay_work: f64,
}

impl Throttle {
    /// A throttle at share `share` (clamped to (0, 0.5]), `kappa` units per wire of a global
    /// layering, for a board of `alive` wires.
    pub fn new(share: f64, kappa: f64, alive: usize) -> Throttle {
        let share = if share.is_finite() { share.clamp(1e-3, 0.5) } else { 0.5 };
        let kappa = kappa.max(0.0);
        let mut t = Throttle { share, kappa, since: 0.0, need: 0.0, runs: 0, esc_work: 0.0, relay_work: 0.0 };
        t.need = t.cost(alive) * (1.0 / share - 1.0);
        t
    }
    /// The work of one global layering of `alive` wires.
    pub fn cost(&self, alive: usize) -> f64 {
        self.kappa * alive as f64
    }
    /// A re-lay (or any local step) of `units` work.
    pub fn add(&mut self, units: f64) {
        let u = units.max(0.0);
        self.since += u;
        self.relay_work += u;
    }
    /// Whether the next escape is due.
    pub fn due(&self) -> bool {
        self.since >= self.need
    }
    /// An escape ran on a board of `alive` wires.
    pub fn ran(&mut self, alive: usize) {
        let w = self.cost(alive);
        self.runs += 1;
        self.esc_work += w;
        self.since = 0.0;
        self.need = w * (1.0 / self.share - 1.0);
    }
    /// A whole-board step of measured work `w` (TW5 (f): an in-loop P4 round) ran: it draws on the
    /// same budget as an escape (the next escape or round is due after `w x (1/s - 1)` re-lay work).
    pub fn ran_work(&mut self, w: f64) {
        let w = if w.is_finite() { w.max(0.0) } else { 0.0 };
        self.runs += 1;
        self.esc_work += w;
        self.since = 0.0;
        self.need = w * (1.0 / self.share - 1.0);
    }
    /// The escapes' share of all work so far (0 without work).
    pub fn measured_share(&self) -> f64 {
        let t = self.esc_work + self.relay_work;
        if t > 0.0 {
            self.esc_work / t
        } else {
            0.0
        }
    }
}

/// The loop's history of over-full edges (TW5 c): per edge `h <- fade x h + (1 + d) / (2 + d)`
/// in every update in which the edge is over-full by the graded depth `d >= 0` (a just over-full
/// edge adds 1/2, a deeply over-full one almost 1), `h <- fade x h` otherwise. With `fade < 1` it
/// is bounded by `1 / (1 - fade)` and an edge that stays free is forgotten geometrically.
/// `apply` writes it onto the search's per-edge history (`State::history`, priced by the
/// `congestion` factor per pass) on top of the history the loop started with.
#[derive(Clone, Debug, PartialEq)]
pub struct History {
    pub fade: f64,
    pub h: Vec<f64>,
    /// Updates so far.
    pub updates: usize,
}

impl History {
    pub fn new(edges: usize, fade: f64) -> History {
        History { fade: if fade.is_finite() { fade.clamp(0.0, 0.999) } else { 0.8 }, h: vec![0.0; edges], updates: 0 }
    }
    /// One update with the over-full edges `over` (edge, depth); an edge may come more than once
    /// (several layers or cuts): its deepest counts.
    pub fn update(&mut self, over: &[(usize, f64)]) {
        let mut inc: Vec<(usize, f64)> = over.iter().filter(|x| x.0 < self.h.len()).map(|&(e, d)| (e, if d.is_finite() { d.max(0.0) } else { 0.0 })).collect();
        inc.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
        inc.dedup_by_key(|x| x.0);
        for h in &mut self.h {
            *h *= self.fade;
        }
        for (e, d) in inc {
            self.h[e] += (1.0 + d) / (2.0 + d);
        }
        self.updates += 1;
    }
    /// The bound every entry stays below.
    pub fn bound(&self) -> f64 {
        1.0 / (1.0 - self.fade)
    }
    /// `base + h` per edge into `out` (`base` = the history the loop started with).
    pub fn apply(&self, base: &[f64], out: &mut [f64]) {
        for (e, o) in out.iter_mut().enumerate() {
            *o = base.get(e).copied().unwrap_or(0.0) + self.h.get(e).copied().unwrap_or(0.0);
        }
    }
    /// The mean history over the edges `edges` (a wire's passes; 0 for none).
    pub fn mean_on(&self, edges: impl Iterator<Item = usize>) -> f64 {
        let (mut s, mut n) = (0.0, 0usize);
        for e in edges {
            s += self.h.get(e).copied().unwrap_or(0.0);
            n += 1;
        }
        if n == 0 {
            0.0
        } else {
            s / n as f64
        }
    }
}

/// Of the positions (wire, traversal) whose allowed layers are more than one (`free[w][k]`), how
/// many and how many of them have a different layer mask in `a` and `b` (per wire per traversal,
/// as [`super::optimise::layers_now`]). Wires missing in either side are skipped. O(positions).
pub fn free_diff(a: &[Vec<u64>], b: &[Vec<u64>], free: &[Vec<bool>]) -> (usize, usize) {
    let (mut n, mut d) = (0usize, 0usize);
    for (w, f) in free.iter().enumerate() {
        let (Some(x), Some(y)) = (a.get(w), b.get(w)) else { continue };
        if x.len() != f.len() || y.len() != f.len() {
            continue;
        }
        for k in 0..f.len() {
            if f[k] {
                n += 1;
                d += usize::from(x[k] != y[k]);
            }
        }
    }
    (n, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_amplitude_is_bounded_graded_and_never_cools() {
        assert_eq!(bounded_amp(0.1, 0.4, 0.0), 0.1);
        let mut last = 0.0;
        for i in 0..200 {
            let a = bounded_amp(0.1, 0.4, i as f64 * 0.25);
            assert!((0.1..0.4).contains(&a), "{a}");
            assert!(a >= last, "graded: never falls with the badness");
            last = a;
        }
        assert!(bounded_amp(0.1, 0.4, 1e12) <= 0.4 && bounded_amp(0.1, 0.4, 1e12) > 0.399);
        assert_eq!(bounded_amp(0.1, 0.4, f64::NAN), 0.1);
        // hi below lo: no width range, the lower bound.
        assert_eq!(bounded_amp(0.3, 0.1, 5.0), 0.3);
        // Badness: a straight, free, gaining wire is 0; each term adds.
        assert_eq!(badness(10.0, 10.0, 0.0, 0), 0.0);
        assert!((badness(15.0, 10.0, 0.0, 0) - 0.5).abs() < 1e-12);
        assert!((badness(10.0, 10.0, 1.5, 1) - 2.0).abs() < 1e-12);
        assert_eq!(badness(5.0, 10.0, 0.0, 0), 0.0);
    }

    #[test]
    fn the_throttle_keeps_the_escape_share_on_any_work_stream() {
        for &(share, kappa, alive) in &[(0.35, 2.0, 40usize), (0.5, 1.0, 7), (0.1, 5.0, 300)] {
            let mut t = Throttle::new(share, kappa, alive);
            assert!(!t.due(), "the first escape waits for re-lay work");
            // Re-lay steps small against one escape's work (kappa x alive), as on a board: a step
            // larger than the whole budget can only overshoot, the share then falls below the target.
            let step = 1 + (kappa * alive as f64 / 4.0) as u64;
            let mut s = 1u64;
            for _ in 0..20_000 {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                t.add(((s >> 33) % step) as f64);
                if t.due() {
                    t.ran(alive);
                }
            }
            assert!(t.runs > 10, "{share}: {} runs", t.runs);
            assert!(t.measured_share() <= share + 1e-9, "{share}: measured {}", t.measured_share());
            assert!(t.measured_share() >= 0.5 * share, "{share}: measured {} (escapes run as often as allowed)", t.measured_share());
        }
        let mut t = Throttle::new(9.0, 1.0, 10);
        assert_eq!(t.share, 0.5, "never above half (not sooner than its own last duration)");
        t.add(10.0);
        assert!(t.due(), "at 0.5 one escape's work of re-lays makes the next due");
    }

    #[test]
    fn the_history_grows_on_full_edges_fades_and_stays_bounded() {
        let mut h = History::new(3, 0.8);
        for _ in 0..500 {
            h.update(&[(0, 0.0), (1, 3.0), (1, 0.1)]);
        }
        assert!(h.h[0] > 2.4 && h.h[0] < h.bound());
        assert!(h.h[1] > h.h[0], "deeper over-full adds more");
        assert!(h.h[1] < h.bound());
        assert_eq!(h.h[2], 0.0, "never over-full: no history");
        let h1 = h.h[1];
        h.update(&[]);
        assert!((h.h[1] - 0.8 * h1).abs() < 1e-12, "fades once free");
        for _ in 0..200 {
            h.update(&[]);
        }
        assert!(h.h[1] < 1e-12);
        let mut out = vec![0.0; 3];
        h.update(&[(2, 0.0)]);
        h.apply(&[1.0, 0.0], &mut out);
        assert!((out[0] - 1.0).abs() < 1e-9 && (out[2] - 0.5).abs() < 1e-12);
        assert!((h.mean_on([2usize, 1].into_iter()) - 0.25).abs() < 1e-9);
        assert_eq!(h.mean_on(std::iter::empty()), 0.0);
    }

    #[test]
    fn free_diff_counts_only_free_positions() {
        let a = vec![vec![1u64, 1, 2], vec![2]];
        let b = vec![vec![2u64, 1, 1], vec![1]];
        let free = vec![vec![true, true, false], vec![true]];
        assert_eq!(free_diff(&a, &b, &free), (3, 2));
        assert_eq!(free_diff(&a, &b, &[vec![true; 2]]), (0, 0), "length mismatch skipped");
    }
}
