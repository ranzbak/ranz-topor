//! Iterations like TopoR (M-B0): the fluctuation schedule of the whole-route cycles and the
//! "Routes stable" test that replaces a fixed round count as the end of a run.
//!
//! * **Cycle** = one whole route (projection, P2 search, P3 layers, P4 rip-up, P6 drawing) whose
//!   result variants go to the archive ([`super::archive`]). TopoR iterates the same way: the
//!   optimisation goes on, every pass may save a variant, and the user stops it (TopoR 6.3 manual
//!   p. 73: "The optimization process can continue indefinitely until it is stopped manually").
//! * **Fluctuation schedule** ([`Schedule`]): the factor weights of a cycle are multiplied by
//!   seeded factors in `[1 - width, 1 + width]` (Polubasov 2010, «Перекладка проводников»:
//!   the coefficients fluctuate so the search leaves local minima). The width starts wide and is
//!   multiplied by `narrowing` after every cycle (geometric cooling; Kirkpatrick, Gelatt & Vecchi
//!   1983, Science 220:671-680), and is set back to `reheat` when the run is *stuck*: no gain of the
//!   best result for `stuck_window` cycles while it is still incomplete (geometric reheating;
//!   Abramson, Krishnamoorthy & Dang 1999, APJOR 16:1-22, the reheating schedules). After a reheat
//!   the width narrows again.
//! * **Stable** ([`Meter`]): the gain of the best result over the last `window` cycles is below
//!   `min_gain_pct` (TopoR 6.3 p. 73: "When there have not been any improvements to the existing
//!   variants in a long time, the system shows the *Autorouting may be stopped* message"; "Do not
//!   stop the optimization process before the value in the Level column reaches 5-10": stable
//!   needs `window + 1` cycles, 5 with the default window of 4).
//!
//! The best result is compared by [`Score`]: lexicographic (incomplete count, objective value).
//! `incomplete` is the archive's error count: since M-B1 ("never drop") the connections whose
//! written copper still breaks a rule plus the connections without copper. The value includes the violation price;
//! its relative gain is measured against the objective's own value (`scale`), so a large
//! violation price does not hide the gains in length and vias.
//!
//! Deterministic: no clock, no randomness (the seeded weights are drawn in `factors`).

/// The best result after a cycle: incomplete count first, then the objective value (lower is
/// better in both).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Score {
    /// Connections not complete: with rule violations in their copper, or without copper.
    pub incomplete: usize,
    /// The board objective's value (`length + mm_per_via x vias`, or the area) plus the
    /// violation price.
    pub value: f64,
    /// The denominator of a relative gain: the objective's own value (0: `value`).
    pub scale: f64,
}

impl Score {
    /// Gain in percent from `old` to `self` (`self` is at least as good: the best only improves).
    /// Fewer incomplete counts as 100 % (any completion is worth more than any length).
    pub fn gain_pct(&self, old: &Score) -> f64 {
        if self.incomplete < old.incomplete {
            100.0
        } else if self.incomplete > old.incomplete {
            0.0
        } else if old.value > 1e-9 {
            let den = if old.scale > 1e-9 { old.scale } else { old.value };
            (100.0 * (old.value - self.value) / den).max(0.0)
        } else {
            0.0
        }
    }
}

/// The fluctuation width per cycle: start wide, narrow every cycle, reheat when stuck.
#[derive(Clone, Debug, PartialEq)]
pub struct Schedule {
    pub start: f64,
    pub narrowing: f64,
    pub stuck_window: usize,
    pub reheat: f64,
    /// Width of the cycle that runs next.
    pub width: f64,
    /// Cycles since the last gain or reheat.
    since: usize,
    /// Reheats so far.
    pub reheats: usize,
}

impl Schedule {
    pub fn new(start: f64, narrowing: f64, stuck_window: usize, reheat: f64) -> Schedule {
        Schedule { start, narrowing: narrowing.clamp(0.0, 1.0), stuck_window, reheat, width: start.max(0.0), since: 0, reheats: 0 }
    }

    /// After a cycle: `gained` = the best result improved in it, `incomplete` = the best result
    /// is not complete. Sets the width of the next cycle; true when that is a reheat.
    pub fn after_cycle(&mut self, gained: bool, incomplete: bool) -> bool {
        if gained {
            self.since = 0;
        } else {
            self.since += 1;
        }
        if incomplete && self.stuck_window > 0 && self.since >= self.stuck_window && self.reheat > 0.0 {
            self.width = self.reheat;
            self.since = 0;
            self.reheats += 1;
            return true;
        }
        self.width *= self.narrowing;
        false
    }
}

/// The "Routes stable" test over the best result after every cycle.
#[derive(Clone, Debug, Default)]
pub struct Meter {
    pub window: usize,
    pub min_gain_pct: f64,
    /// Best score after each cycle (index = cycle - 1).
    pub best: Vec<Score>,
}

impl Meter {
    pub fn new(window: usize, min_gain_pct: f64) -> Meter {
        Meter { window: window.max(1), min_gain_pct, best: Vec::new() }
    }

    /// Record the best score after a cycle; true when it is better than the one before.
    pub fn push(&mut self, s: Score) -> bool {
        let gained = self.best.last().is_none_or(|p| s.incomplete < p.incomplete || (s.incomplete == p.incomplete && s.value < p.value - 1e-9));
        self.best.push(s);
        gained
    }

    /// Gain (%) of the best over the last `window` cycles; `None` before `window + 1` cycles.
    pub fn gain_pct(&self) -> Option<f64> {
        let n = self.best.len();
        (n > self.window).then(|| self.best[n - 1].gain_pct(&self.best[n - 1 - self.window]))
    }

    /// Gain below the threshold over a full window.
    pub fn stable(&self) -> bool {
        self.gain_pct().is_some_and(|g| g < self.min_gain_pct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(i: usize, v: f64) -> Score {
        Score { incomplete: i, value: v, scale: 0.0 }
    }

    #[test]
    fn gain_is_lexicographic() {
        assert_eq!(sc(3, 900.0).gain_pct(&sc(4, 100.0)), 100.0);
        assert!((sc(3, 99.0).gain_pct(&sc(3, 100.0)) - 1.0).abs() < 1e-9);
        assert_eq!(sc(3, 100.0).gain_pct(&sc(3, 100.0)), 0.0);
        // With a violation price on top, the gain is relative to the objective's own value.
        let old = Score { incomplete: 3, value: 10_100.0, scale: 100.0 };
        let new = Score { incomplete: 3, value: 10_099.0, scale: 99.0 };
        assert!((new.gain_pct(&old) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn stable_needs_a_full_window_without_gain() {
        let mut m = Meter::new(2, 0.5);
        assert!(m.push(sc(5, 100.0)));
        assert!(!m.stable());
        assert!(m.push(sc(4, 110.0)));
        assert!(!m.push(sc(4, 110.0)));
        // window 2: best now vs two cycles ago = 4 vs 5 incomplete -> 100 %
        assert!(!m.stable());
        assert!(!m.push(sc(4, 110.0)));
        assert!(m.stable());
        assert!(m.push(sc(4, 100.0)));
        assert!(!m.stable(), "a 9 % gain in the window");
    }

    #[test]
    fn schedule_narrows_and_reheats_when_stuck() {
        let mut s = Schedule::new(0.3, 0.5, 2, 0.25);
        assert_eq!(s.width, 0.3);
        assert!(!s.after_cycle(true, true));
        assert!((s.width - 0.15).abs() < 1e-12);
        assert!(!s.after_cycle(false, true));
        assert!((s.width - 0.075).abs() < 1e-12);
        assert!(s.after_cycle(false, true), "two cycles without gain while incomplete: reheat");
        assert_eq!(s.width, 0.25);
        assert_eq!(s.reheats, 1);
        assert!(!s.after_cycle(false, true));
        assert!((s.width - 0.125).abs() < 1e-12);
    }

    #[test]
    fn complete_runs_never_reheat() {
        let mut s = Schedule::new(0.2, 0.5, 1, 0.2);
        for _ in 0..5 {
            assert!(!s.after_cycle(false, false));
        }
        assert!(s.width < 0.01);
    }
}
