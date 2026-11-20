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
    let s = f64::from(stuck);
    detour_excess(len, straight) + crowd.max(0.0) + s / (1.0 + s)
}

/// The detour of a path of length `len` between ends `straight` apart: `len / straight - 1`
/// (>= 0; 0 for a straight path and without a reference: `straight <= 0`, an infinite `len`).
/// The ratio of the run report's detour metric minus one. O(1).
pub fn detour_excess(len: f64, straight: f64) -> f64 {
    if straight > 0.0 && len.is_finite() {
        (len / straight - 1.0).max(0.0)
    } else {
        0.0
    }
}

/// TW6 r4 (`[optimize] site_move_detour`): the graded detour price, in violations, of a path
/// [`detour_excess`] `e` over its straight line on the scale `scale` (an excess ratio):
/// `(e / scale)^2`, the shape of the via-pair stretch price
/// ([`super::viaopt::pair_len_price`]): a near-straight path costs next to nothing, one whose
/// excess equals the scale costs one violation, and the price keeps growing smoothly (no cutoff).
/// `scale <= 0` = no price (0). O(1).
pub fn detour_price(len: f64, straight: f64, scale: f64) -> f64 {
    if scale <= 0.0 || !scale.is_finite() {
        return 0.0;
    }
    let r = detour_excess(len, straight) / scale;
    r * r
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
    /// site-price-cost ([`Throttle::new_work`]): the re-lay work past the due point carries over to
    /// the next wait (a re-lay is not small against an escape: resetting to 0 lost on average half a
    /// re-lay per escape, esp 0.29 for 0.35); false = reset (before).
    pub carry: bool,
}

impl Throttle {
    /// A throttle at share `share` (clamped to (0, 0.5]), `kappa` units per wire of a global
    /// layering, for a board of `alive` wires.
    pub fn new(share: f64, kappa: f64, alive: usize) -> Throttle {
        let share = if share.is_finite() { share.clamp(1e-3, 0.5) } else { 0.5 };
        let kappa = kappa.max(0.0);
        let mut t = Throttle { share, kappa, since: 0.0, need: 0.0, runs: 0, esc_work: 0.0, relay_work: 0.0, carry: false };
        t.need = t.cost(alive) * (1.0 / share - 1.0);
        t
    }
    /// site-price-cost (`[optimize] escape_budget = "cost"`): a throttle at share `share` (as
    /// [`Throttle::new`]) whose escapes report their measured work ([`Throttle::ran_work`]); the
    /// first escape is due after `first x (1/s - 1)` re-lay work, `first` the measured cost of one
    /// whole-board layering and measure ([`cost`]). `kappa` is 0 (unused). The re-lay work past a
    /// due point counts towards the next wait ([`Throttle::carry`]), so over a run the escapes take
    /// the share itself, not less (an escape may come one re-lay early: the share can pass `s` by
    /// at most one re-lay's work over the run).
    pub fn new_work(share: f64, first: f64) -> Throttle {
        let mut t = Throttle::new(share, 0.0, 0);
        t.carry = true;
        let first = if first.is_finite() { first.max(0.0) } else { 0.0 };
        t.need = first * (1.0 / t.share - 1.0);
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
        self.since = if self.carry { (self.since - self.need).max(0.0) } else { 0.0 };
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

/// site-price-cost (`[optimize] escape_budget = "cost"`, docs/dev/design/site-price-cost.md): the
/// weights of the TopoR loop's deterministic cost counter, in units of one wire a re-lay's local
/// judge lays or models (the old work unit, about 0.8 ms at 4 threads). Only counts enter (alive
/// wires, passes, the refinement's candidate evaluations, which the lane executor keeps equal to the
/// serial loop's), so the copper never depends on the clock, the thread or the lane count.
///
/// Source: least squares of the wall time per escape and per sweep re-lay over 4 deterministic runs
/// (qmtech + esp-rust-board seed 0, `optimize.rounds = 1`, `runs = 1`, arms X = every TW opt-in
/// but `[vias] site_price` and `preloop_p4 = false`, and X with `site_price = "graded"`; 315
/// escapes, 884 re-lays; 4 threads, local, 2026-11-20, branch site-price-cost after the point_free
/// memo). Escape-vs-re-lay real/predicted ratio per run: 0.87, 1.02, 1.01, 1.13. Refit:
/// `tools/escape_cost_fit.py` on the profile JSONs (`TOPO_PROF=1`, `topo_route --prof FILE`; their
/// `work` rows) of such runs. The measured throttle carries a re-lay's excess work over to the next
/// escape (`Throttle::carry`), so the share holds for re-lays dearer than one escape's due.
pub mod cost {
    /// A whole-board layering: per alive wire, per candidate evaluation of its refinement, per pass.
    pub const GLOBAL_ALIVE: f64 = 0.2146;
    pub const GLOBAL_EVAL: f64 = 0.0818;
    pub const GLOBAL_PASS: f64 = 1.341e-3;
    /// A whole-board measure (capacity model of every wire): per pass.
    pub const MEASURE_PASS: f64 = 1.305e-3;
    /// A local layering: per candidate evaluation of its refinement (its judge's wires are the old
    /// unit, 1 per wire).
    pub const LOCAL_EVAL: f64 = 0.2357;
    /// One sweep re-lay (path search, placing, uncrossing): per pass of the board.
    pub const RELAY_PASS: f64 = 6.15e-4;

    /// The cost of a whole-board layering of `alive` wires over `passes` passes whose refinement
    /// evaluated `evals` candidates. O(1).
    pub fn global(alive: usize, evals: usize, passes: usize) -> f64 {
        GLOBAL_ALIVE * alive as f64 + GLOBAL_EVAL * evals as f64 + GLOBAL_PASS * passes as f64
    }
    /// The cost of a whole-board measure over `passes` passes. O(1).
    pub fn measure(passes: usize) -> f64 {
        MEASURE_PASS * passes as f64
    }
    /// The cost of a local layering's refinement of `evals` candidates and a local judge of
    /// `wires` laid and modelled wires. O(1).
    pub fn local(evals: usize, wires: usize) -> f64 {
        LOCAL_EVAL * evals as f64 + wires as f64
    }
    /// The fixed cost of one sweep re-lay on a board of `passes` passes. O(1).
    pub fn relay(passes: usize) -> f64 {
        RELAY_PASS * passes as f64
    }
}

/// TW7 B (`[optimize] pattern_share`, docs/dev/design/tw7-moves.md §11): the graded work budget of
/// the loop's pattern moves, in the work units of [`Throttle`] (deterministic: never the clock).
/// The loop's other work `O` earns the moves a credit of `O x s / (1 - s)`; each move spends its
/// measured work. Moves run while the credit is positive, the most excess length first, so a
/// move that overdraws is paid back from the next cycles' work (no cutoff: over a run the moves
/// take about the share `s` of the work, a cycle with little other work runs fewer of them).
/// `s >= 1` = no budget (every candidate, up to the loop's rip-up limit). O(1) per call.
#[derive(Clone, Debug, PartialEq)]
pub struct Share {
    pub share: f64,
    pub credit: f64,
    /// The moves' work and the loop's other work so far (the measured share).
    pub spent: f64,
    pub base: f64,
    pub moves: usize,
}

impl Share {
    /// A budget at share `share` (clamped to [0.01, 1]; not finite = 1).
    pub fn new(share: f64) -> Share {
        let share = if share.is_finite() { share.clamp(0.01, 1.0) } else { 1.0 };
        Share { share, credit: 0.0, spent: 0.0, base: 0.0, moves: 0 }
    }
    /// Whether the budget is off (`share >= 1`).
    pub fn unbounded(&self) -> bool {
        self.share >= 1.0
    }
    /// The loop's other work of `units` since the last call.
    pub fn earn(&mut self, units: f64) {
        let u = if units.is_finite() { units.max(0.0) } else { 0.0 };
        self.base += u;
        if !self.unbounded() {
            self.credit += u * self.share / (1.0 - self.share);
        }
    }
    /// Whether the next move may run.
    pub fn allows(&self) -> bool {
        self.unbounded() || self.credit > 0.0
    }
    /// A move of measured work `w` ran.
    pub fn spend(&mut self, w: f64) {
        let w = if w.is_finite() { w.max(0.0) } else { 0.0 };
        self.moves += 1;
        self.spent += w;
        if !self.unbounded() {
            self.credit -= w;
        }
    }
    /// The moves' share of all work so far (0 without work).
    pub fn measured_share(&self) -> f64 {
        let t = self.spent + self.base;
        if t > 0.0 {
            self.spent / t
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

    /// site-price-cost: with measured escape costs (`escape_budget = "cost"`) the throttle keeps
    /// the share whatever an escape costs against a re-lay (cheap, dear, varying), and the first
    /// escape waits for `first x (1/s - 1)` re-lay work.
    #[test]
    fn the_measured_throttle_keeps_the_share_for_any_escape_cost() {
        for &(share, first, esc_mean) in &[(0.35, 40.0, 40.0), (0.175, 22.0, 300.0), (0.5, 5.0, 1.0), (0.35, 100.0, 2.0)] {
            let mut t = Throttle::new_work(share, first);
            assert!((t.need - first * (1.0 / share - 1.0)).abs() < 1e-9, "{share}: first due after {}", t.need);
            assert!(!t.due() && t.kappa == 0.0);
            let mut s = 7u64;
            let mut rnd = |m: f64| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                m * (0.5 + ((s >> 33) % 1000) as f64 / 1000.0)
            };
            for _ in 0..20_000 {
                t.add(rnd(1.0));
                if t.due() {
                    let w = rnd(esc_mean);
                    t.ran_work(w);
                }
            }
            assert!(t.runs > 10, "{share}: {} runs", t.runs);
            let m = t.measured_share();
            assert!((m - share).abs() <= 0.02, "{share} (escape {esc_mean}): measured {m}");
        }
        // The carry: re-lays as large as the wait, escapes of equal work. Without the carry the
        // share falls to s / (s + 1.5 (1 - s)) (half a re-lay lost per escape); with it, s.
        for carry in [false, true] {
            let mut t = Throttle::new_work(0.35, 10.0);
            t.carry = carry;
            for _ in 0..3000 {
                t.add(13.0);
                if t.due() {
                    t.ran_work(10.0);
                }
            }
            let m = t.measured_share();
            if carry {
                assert!((m - 0.35).abs() < 0.01, "carry: {m}");
            } else {
                assert!(m < 0.32, "reset: {m}");
            }
        }
        // Not a finite first cost: no first wait beyond any re-lay.
        assert_eq!(Throttle::new_work(0.35, f64::NAN).need, 0.0);
        assert_eq!(Throttle::new_work(0.35, -3.0).need, 0.0);
    }

    /// site-price-cost: the cost counter's terms are the weighted counts (each grows with its count
    /// at its weight, 0 for none), so an escape on a board of 320 alive wires and 27k passes with
    /// 2005 evaluations costs about what the 4 calibration runs measured (231 ms at 0.81 ms per
    /// unit, within 15 %).
    #[test]
    fn the_cost_terms_are_the_weighted_counts() {
        assert_eq!(cost::global(0, 0, 0), 0.0);
        assert_eq!(cost::measure(0) + cost::local(0, 0) + cost::relay(0), 0.0);
        assert!((cost::global(10, 0, 0) - 10.0 * cost::GLOBAL_ALIVE).abs() < 1e-12);
        assert!((cost::global(0, 10, 0) - 10.0 * cost::GLOBAL_EVAL).abs() < 1e-12);
        assert!((cost::global(0, 0, 10) - 10.0 * cost::GLOBAL_PASS).abs() < 1e-12);
        assert!((cost::measure(1000) - 1000.0 * cost::MEASURE_PASS).abs() < 1e-12);
        assert!((cost::local(10, 3) - (10.0 * cost::LOCAL_EVAL + 3.0)).abs() < 1e-12);
        assert!((cost::relay(1000) - 1000.0 * cost::RELAY_PASS).abs() < 1e-12);
        let esc_ms = 0.813 * (cost::global(320, 2005, 26993) + 0.6 * cost::measure(26993));
        assert!((esc_ms / 231.0 - 1.0).abs() < 0.15, "qmtech X escape {esc_ms} ms");
        let relay_ms = 0.813 * (cost::local(15, 34) + cost::relay(26993));
        assert!((relay_ms / 49.8 - 1.0).abs() < 0.25, "qmtech X re-lay {relay_ms} ms");
    }

    #[test]
    fn the_pattern_share_keeps_its_share_and_is_graded() {
        // Moves of random work against random other work: over a run the moves take about the
        // share, never much more (an overdraw is paid back), and a larger share runs more moves.
        let mut last_moves = 0usize;
        for &share in &[0.05, 0.15, 0.3, 0.6] {
            let mut b = Share::new(share);
            let mut s = 7u64;
            let mut rnd = |m: u64| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((s >> 33) % m) as f64
            };
            for _ in 0..5_000 {
                b.earn(rnd(40));
                for _ in 0..8 {
                    if !b.allows() {
                        break;
                    }
                    let w = 1.0 + rnd(30);
                    b.spend(w);
                }
            }
            let m = b.measured_share();
            assert!(m <= share * 1.02 + 1e-3, "{share}: measured {m}");
            assert!(m >= 0.9 * share.min(8.0 * 15.5 / (8.0 * 15.5 + 19.5)), "{share}: measured {m} (moves run while allowed)");
            assert!(b.moves > last_moves, "graded: share {share} runs more moves ({} after {last_moves})", b.moves);
            last_moves = b.moves;
        }
        // No credit before any other work; one unit of work at share 0.5 allows one move.
        let mut b = Share::new(0.5);
        assert!(!b.allows());
        b.earn(1.0);
        assert!(b.allows());
        b.spend(3.0);
        assert!(!b.allows(), "an overdraw waits for the next work");
        b.earn(1.9);
        assert!(!b.allows());
        b.earn(0.2);
        assert!(b.allows(), "paid back: the moves may run again");
        // share >= 1 (and not finite) = no budget.
        for x in [1.0, 3.0, f64::NAN] {
            let mut u = Share::new(x);
            assert!(u.unbounded() && u.allows());
            u.spend(1e9);
            assert!(u.allows());
        }
        assert_eq!(Share::new(0.0).share, 0.01);
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
