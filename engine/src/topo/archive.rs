//! Board evaluation (plan "Board evaluation" and `[optimize] objective`): the Pareto archive of
//! result variants over TopoR's four criteria and the pick of the variant that goes to KiCad.
//!
//! * A variant is a complete result (every whole-route round gives two: as drawn, and after the
//!   via reduction) with its [`Criteria`]: errors (connections not routed, which includes every
//!   violation the core refuses to emit), vias, total length and shrinkages (necked stretches).
//! * The archive keeps every non-dominated variant: a new one is refused when an archived one is
//!   at least as good on all four criteria; it removes the archived ones it dominates. On top,
//!   the fewest-warnings variant is kept even when the four criteria dominate it (plan: warnings
//!   are a column, not a fifth dimension). When the archive is over its cap the variant worst by
//!   the objective goes first; the champions (fewest errors, shortest, fewest vias, fewest
//!   shrinkages, fewest warnings) are never pruned.
//! * The pick: fewest ranked errors first ([`Criteria::ranked`]: errors + (k - 1) x shorts, k =
//!   `[optimize] short_weight`, default 1 = the plain count), then the fewest connections with a
//!   short as a tie-break (`[optimize] pick_shorts`, default on; see [`Criteria::shorts`]), then by `objective` and
//!   `[optimize] pick`:
//!   `length_vias` with `pick = "errors_vias_length"` (default) = fewest vias, then the shortest
//!   length; with `pick = "errors_weighted"` = least `length + mm_per_via x vias` (the 25 mm/via
//!   rule of the TopoR manual: a step to fewer vias is taken while it costs at most `mm_per_via`
//!   of length per via removed); `area` = least occupied area (a trace counts `L x (w + s)`, a via
//!   its disc of pad plus clearance on every layer). Ties: fewer shrinkages, then the earlier
//!   variant. Only the pick differs: the archive membership (Pareto) and every internal price
//!   (`mm_per_via` in the rip-up rounds and in the cap pruning) are the same in both modes.
//!
//! Deterministic: Vec in offer order, no hashing. Complexity: an offer is O(archive size).

use serde::Serialize;

/// The board-level numbers of one variant.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Criteria {
    /// Errors: connections not complete, i.e. without copper or with rule violations in their
    /// written copper (M-B1 never drop). TopoR's variant table counts design-rule violations next
    /// to length and vias, and "a design-rule violation cannot be compensated by any reduction of
    /// wire length" (Polubasov 2010, «Многовариантная оптимизация»): errors come first in the
    /// pick. Counted per connection, so an open connection never counts less than one drawn with
    /// violations; how many rules and how badly is the graded [`Criteria::penalty`].
    pub errors: usize,
    /// Of `errors`: connections whose written copper overlaps copper of another net (a short:
    /// KiCad's `shorting_items` / `tracks_crossing`, a DRC error that breaks the board, and the
    /// track KiCad's Cleanup removes, so it ends as an open plus the error). In a plan (the
    /// TopoR loop's variants, the P4 score): the predicted shorts, crossings no layer separates
    /// and traversals through copper of another net (not a no-tracks rule area: a violation, no
    /// short). Ranked right after `errors`, before `open`: within
    /// one error count a short is worse than an open (user 2026-11-12, "shorts should be in the
    /// routing criteria"). Not counted again in `errors` (a shorted connection is one error, like
    /// any other connection with violations). 0 when `[optimize] pick_shorts` is off (the ranking
    /// before 2026-11-12; the cap pruning in [`Archive::offer`] keeps the pick either way).
    pub shorts: usize,
    /// `[optimize] short_weight` k minus 1 (PS1, user 2026-11-13): what a connection with a short
    /// counts on top of its one error in the ranked count [`Criteria::ranked`]. 0 (k = 1, the
    /// default and `Default`): a short counts once, like any error, as TopoR's variant table
    /// counts violations (6.3 manual p. 72: "sorted by number of violations, then by ascending
    /// length"). Set by the builders; 0 when `pick_shorts` is off.
    pub short_extra: f64,
    /// Connections without copper (unrouted: no path, the time limit, legal-only output).
    pub open: usize,
    /// Rule violations of the written copper (never drop).
    pub violations: usize,
    /// The `violation` factor's price of the violations and the open connections (mm; the graded
    /// part: how deep and long they are), added to the objective's value.
    pub penalty: f64,
    pub vias: usize,
    pub length_mm: f64,
    /// Necked stretches (TopoR's "shrinkages").
    pub shrinkages: usize,
    /// pick-necks (user 2026-11-27): necked copper of the drawn tracks, mm2: sum of `(w_full -
    /// w_drawn) x length`, every segment at its own width (a taper steps down, not at its narrowest
    /// width). 0 for a plan.
    pub neck_mm2: f64,
    /// pick-necks: vias smaller than the net class via (neck vias, e.g. down to the Board Setup
    /// minimum) of the drawn copper. 0 for a plan.
    pub neck_vias: usize,
    /// pick-necks: the necking term of `pick = best`, mm equivalent: `[optimize] neck_mm_per_mm2 x
    /// neck_mm2 + neck_via_mm x neck_vias`. Added to length + `mm_per_via` x vias in the value the
    /// pick ranks after errors and shorts (user 2026-11-27: "necking width reduced x length x
    /// constant"); shrinkages stay the last tie-break. 0 for a plan.
    pub necking: f64,
    /// Warning-level factor findings (a column, not a Pareto criterion).
    pub warnings: usize,
    /// Occupied area (mm²) for the `area` objective.
    pub area_mm2: f64,
}

impl Criteria {
    /// The ranked error count (PS1): `errors + (k - 1) x shorts` with k = `[optimize]
    /// short_weight`, so a connection with a short counts k errors and any other incomplete
    /// connection one. Graded, no hard key: k = 1 is the plain error count (TopoR p. 72), a k
    /// above 1 lets a variant with fewer shorts win over one with somewhat fewer errors. Still
    /// first in every pick: "a design-rule violation cannot be compensated by any reduction of
    /// wire length" (Polubasov 2010, «Многовариантная оптимизация»). O(1).
    pub fn ranked(&self) -> f64 {
        self.errors as f64 + self.short_extra * self.shorts as f64
    }
    /// At least as good on all four criteria (errors with their shorts and their price, vias,
    /// length, shrinkages). The shorts are part of the errors criterion, so a variant with more
    /// shorts never covers one with fewer: the archive keeps what the pick prefers.
    pub fn covers(&self, o: &Criteria) -> bool {
        self.errors <= o.errors && self.shorts <= o.shorts && self.penalty <= o.penalty + 1e-9 && self.vias <= o.vias && self.length_mm <= o.length_mm + 1e-9 && self.shrinkages <= o.shrinkages && self.necking <= o.necking + 1e-9
    }
    /// Covers `o` and is better on at least one criterion.
    pub fn dominates(&self, o: &Criteria) -> bool {
        self.covers(o) && (self.errors < o.errors || self.shorts < o.shorts || self.penalty < o.penalty - 1e-9 || self.vias < o.vias || self.length_mm < o.length_mm - 1e-9 || self.shrinkages < o.shrinkages || self.necking < o.necking - 1e-9)
    }
}

/// The board objective (`[optimize] objective`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Objective {
    /// Pareto archive pick: fewest errors, then least `length + mm_per_via x vias`.
    LengthVias { mm_per_via: f64 },
    /// The final pick strictly lexicographic: fewest errors, then fewest vias, then the shortest
    /// length, then fewest shrinkages. `mm_per_via` is kept for the internal prices
    /// ([`Objective::value`]: archive cap pruning, the P4 round pick, the stable meter), which
    /// stay the graded `length + mm_per_via x vias`.
    ErrorsViasLength { mm_per_via: f64 },
    /// Fewest errors, then least occupied area.
    Area,
    /// TW3 / D5 (user 2026-11-18), `[optimize] pick = "best"` (the default): fewest ranked errors
    /// (shorts first among equals), then least `length + mm_per_via x vias` (TopoR 6.3 manual
    /// pp. 72-73: the table sorted by violations, then by length; "good results are achieved at
    /// the ratio of 25 mm/via", p. 73; `mm_per_via` is the option, never a constant), then fewer
    /// shrinkages. `penalty` (`[optimize] best_penalty`, default off): the graded violation price
    /// ([`Criteria::penalty`]) is added to the value, which orders exactly as `LengthVias`.
    /// The internal prices ([`Objective::value`]) are those of `LengthVias`.
    Best { mm_per_via: f64, penalty: bool },
}

/// TW3: the user's four bests of the variant table (topor-way plan, Appendix E). `Overall` is the
/// objective's pick; the other three are taken among the variants with the fewest errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Role {
    /// The objective's order ([`Objective::better`]).
    Overall,
    /// Fewest ranked errors, then fewest shorts, then the least violation price (how deep and long
    /// the violations are), then the objective.
    Errors,
    /// Shortest among the fewest errors (ties: the objective).
    Shortest,
    /// Least vias among the fewest errors (ties: the objective).
    Vias,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Overall, Role::Errors, Role::Shortest, Role::Vias];
    /// The report's name of the role (also the `[optimize] pick` value that applies it).
    pub fn name(self) -> &'static str {
        match self {
            Role::Overall => "best",
            Role::Errors => "errors",
            Role::Shortest => "shortest",
            Role::Vias => "vias",
        }
    }
    /// `[optimize] pick` -> the role whose drawn row goes to KiCad: "errors" / "shortest" /
    /// "vias"; anything else (best, errors_weighted, errors_vias_length) = `Overall`.
    pub fn of_pick(pick: &str) -> Role {
        match pick {
            "errors" => Role::Errors,
            "shortest" => Role::Shortest,
            "vias" => Role::Vias,
            _ => Role::Overall,
        }
    }
}

/// The variants of `crits` that are the four bests ([`Role`]), by index: `[overall, errors,
/// shortest, vias]`; ties go to the lower index. `None` only for an empty slice. O(n).
pub fn bests(crits: &[&Criteria], obj: &Objective) -> [Option<usize>; 4] {
    let n = crits.len();
    if n == 0 {
        return [None; 4];
    }
    let mut overall = 0;
    for i in 1..n {
        if obj.better(crits[i], crits[overall]) {
            overall = i;
        }
    }
    let key_err = |c: &Criteria| (c.ranked(), c.shorts, c.penalty);
    let min_err = (0..n).map(|i| crits[i].ranked()).fold(f64::INFINITY, f64::min);
    let pool: Vec<usize> = (0..n).filter(|&i| (crits[i].ranked() - min_err).abs() <= 1e-9).collect();
    let min_by = |lt: &dyn Fn(&Criteria, &Criteria) -> std::cmp::Ordering| -> Option<usize> {
        let mut b: Option<usize> = None;
        for &i in &pool {
            if b.is_none_or(|j| {
                let o = lt(crits[i], crits[j]);
                o == std::cmp::Ordering::Less || o == std::cmp::Ordering::Equal && obj.better(crits[i], crits[j])
            }) {
                b = Some(i);
            }
        }
        b
    };
    let errors = (0..n).min_by(|&a, &b| {
        let (ka, kb) = (key_err(crits[a]), key_err(crits[b]));
        ka.0.total_cmp(&kb.0).then(ka.1.cmp(&kb.1)).then(ka.2.total_cmp(&kb.2)).then(if obj.better(crits[a], crits[b]) {
            std::cmp::Ordering::Less
        } else if obj.better(crits[b], crits[a]) {
            std::cmp::Ordering::Greater
        } else {
            a.cmp(&b)
        })
    });
    let shortest = min_by(&|a, b| if (a.length_mm - b.length_mm).abs() <= 1e-9 { std::cmp::Ordering::Equal } else { a.length_mm.total_cmp(&b.length_mm) });
    let vias = min_by(&|a, b| a.vias.cmp(&b.vias));
    [Some(overall), errors, shortest, vias]
}

impl Objective {
    /// `"length_vias"` | `"area"`; anything else is `length_vias` with the weighted pick (the
    /// internal prices use this one).
    pub fn parse(s: &str, mm_per_via: f64) -> Objective {
        if s == "area" {
            Objective::Area
        } else {
            Objective::LengthVias { mm_per_via }
        }
    }
    /// The objective of a run: [`Objective::parse`] with the final pick `[optimize] pick`:
    /// `"errors_weighted"` = errors, then `length + mm_per_via x vias`, then shrinkages; anything
    /// else = `errors_vias_length` (errors, vias, length, shrinkages). `area` ignores `pick`.
    /// TW3 (D5): `"best"` (the default) and the role picks `"errors"` / `"shortest"` / `"vias"`
    /// order by [`Objective::Best`] (without the penalty; [`Objective::with_best_penalty`]);
    /// `"errors_vias_length"` is the order of 2026-11-10 to 2026-11-18.
    pub fn from_options(objective: &str, mm_per_via: f64, pick: &str) -> Objective {
        match Objective::parse(objective, mm_per_via) {
            Objective::LengthVias { mm_per_via } if pick == "errors_vias_length" => Objective::ErrorsViasLength { mm_per_via },
            Objective::LengthVias { mm_per_via } if pick != "errors_weighted" => Objective::Best { mm_per_via, penalty: false },
            o => o,
        }
    }
    /// `[optimize] best_penalty`: with `Best`, add the violation price to the ranked value. O(1).
    pub fn with_best_penalty(self, on: bool) -> Objective {
        match self {
            Objective::Best { mm_per_via, .. } => Objective::Best { mm_per_via, penalty: on },
            o => o,
        }
    }
    /// The objective's value within one error count (lower is better): its own value plus the
    /// violation price ([`Criteria::penalty`]).
    pub fn value(&self, c: &Criteria) -> f64 {
        self.base(c) + c.penalty
    }
    /// The objective's own value, without the violation price (the scale of a relative gain).
    pub fn base(&self, c: &Criteria) -> f64 {
        match *self {
            Objective::Best { mm_per_via, .. } => c.length_mm + mm_per_via * c.vias as f64 + c.necking,
            Objective::LengthVias { mm_per_via } | Objective::ErrorsViasLength { mm_per_via } => c.length_mm + mm_per_via * c.vias as f64,
            Objective::Area => c.area_mm2,
        }
    }
    /// Lexicographic key: the ranked error count ([`Criteria::ranked`]: errors, a short counted
    /// `[optimize] short_weight` times), shorts, objective value, shrinkages (`ErrorsViasLength`:
    /// ranked errors, shorts, open connections, vias, length, shrinkages; lengths within 1e-9 mm
    /// are equal). The shorts after the ranked count are a tie-break only (PS1): at the same
    /// ranked count the variant with fewer shorts wins (KiCad's DRC fails the board on a short and
    /// Cleanup removes the track), but they never outrank the count. With k = 1 this is the
    /// order of 2026-11-12 (errors, then shorts) unchanged.
    pub fn better(&self, a: &Criteria, b: &Criteria) -> bool {
        let (ra, rb) = (a.ranked(), b.ranked());
        if (ra - rb).abs() > 1e-9 {
            return ra < rb;
        }
        if a.shorts != b.shorts {
            return a.shorts < b.shorts;
        }
        if let Objective::ErrorsViasLength { .. } = self {
            // Within the same error count, a connection drawn with a violation beats one left open:
            // drawn copper can still be optimised, a dropped trace cannot (user 2026-11-10, "costly
            // traces made can be optimized for, traces that just failed cannot").
            if a.open != b.open {
                return a.open < b.open;
            }
            if a.vias != b.vias {
                return a.vias < b.vias;
            }
            if (a.length_mm - b.length_mm).abs() > 1e-9 {
                return a.length_mm < b.length_mm;
            }
            return a.shrinkages < b.shrinkages;
        }
        let (va, vb) = (self.ranked_value(a), self.ranked_value(b));
        va < vb - 1e-9 || (va - vb).abs() <= 1e-9 && a.shrinkages < b.shrinkages
    }
    /// The value [`Objective::better`] compares within one ranked error count: `value` (with the
    /// violation price), for `Best` without it unless `penalty` (D5). O(1).
    pub fn ranked_value(&self, c: &Criteria) -> f64 {
        match *self {
            Objective::Best { penalty: false, .. } => self.base(c),
            _ => self.value(c),
        }
    }
    /// The criterion on which [`Objective::better`] decides between `a` and `b`, in the pick
    /// order, with `a` minus `b` on it (`a` is better exactly when that is negative); `None` when
    /// they score the same. Names: `"errors"`, `"shorts"`, `"open"`, `"vias"`, `"length"` (mm),
    /// `"shrinkages"`, and for the other picks `"score"` (`errors_weighted`: length +
    /// mm_per_via x vias + the violation price, mm) or `"area"` (mm² + the violation price).
    /// The progress dialog and the per-round log line say with it why a round is not the best. O(1).
    pub fn decider(&self, a: &Criteria, b: &Criteria) -> Option<(&'static str, f64)> {
        let d = |x: usize, y: usize| x as f64 - y as f64;
        let dr = a.ranked() - b.ranked();
        if dr.abs() > 1e-9 {
            return Some(("errors", dr));
        }
        if a.shorts != b.shorts {
            return Some(("shorts", d(a.shorts, b.shorts)));
        }
        if let Objective::ErrorsViasLength { .. } = self {
            if a.open != b.open {
                return Some(("open", d(a.open, b.open)));
            }
            if a.vias != b.vias {
                return Some(("vias", d(a.vias, b.vias)));
            }
            if (a.length_mm - b.length_mm).abs() > 1e-9 {
                return Some(("length", a.length_mm - b.length_mm));
            }
        } else {
            let dv = self.ranked_value(a) - self.ranked_value(b);
            if dv.abs() > 1e-9 {
                // pick-necks: `Best` adds the necking to length + vias; the necking is named when
                // it turns the comparison (without it the other one would win or tie).
                if matches!(self, Objective::Best { .. }) {
                    let rest = dv - (a.necking - b.necking);
                    if rest.abs() <= 1e-9 || (rest < 0.0) != (dv < 0.0) {
                        return Some(("necking", dv));
                    }
                }
                return Some((if matches!(self, Objective::Area) { "area" } else { "score" }, dv));
            }
        }
        (a.shrinkages != b.shrinkages).then(|| ("shrinkages", d(a.shrinkages, b.shrinkages)))
    }
}

/// One archived variant: a label, its criteria and the result it stands for.
#[derive(Clone, Debug)]
pub struct Variant<T> {
    pub label: String,
    pub crit: Criteria,
    pub payload: T,
}

/// A row of the variant table in the run report.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct VariantRow {
    pub label: String,
    pub errors: usize,
    /// Of `errors`: rule violations of the written copper (never drop).
    pub violations: usize,
    /// Of `errors`: connections with a short ([`Criteria::shorts`]).
    pub shorts: usize,
    /// The `violation` factor's price of the errors (mm).
    pub penalty_mm: f64,
    pub vias: usize,
    pub length_mm: f64,
    pub shrinkages: usize,
    /// pick-necks: necked copper (mm2), necked vias, and the necking term ([`Criteria::necking`], mm-eq).
    pub neck_mm2: f64,
    pub neck_vias: usize,
    pub necking: f64,
    pub warnings: usize,
    pub area_mm2: f64,
    /// Length added per via removed against the previous row of the same error count (sorted
    /// by errors, then length); `None` when this row does not remove vias.
    pub mm_per_via: Option<f64>,
    /// Criteria this variant is the best on ("shortest", "fewest vias", ...).
    pub champion: Vec<String>,
    pub picked: bool,
}

/// The Pareto archive.
#[derive(Clone, Debug)]
pub struct Archive<T> {
    pub items: Vec<Variant<T>>,
    cap: usize,
    /// Variants offered in all (accepted or not).
    pub offered: usize,
}

impl<T> Archive<T> {
    /// An empty archive holding at most `cap` variants besides the champions (min 1).
    pub fn new(cap: usize) -> Self {
        Archive { items: Vec::new(), cap: cap.max(1), offered: 0 }
    }

    /// Champion names of variant `i` (each within the fewest errors, plus "fewest errors").
    pub fn champions(&self, i: usize) -> Vec<String> {
        let Some(min_err) = self.items.iter().map(|v| v.crit.errors).min() else { return Vec::new() };
        let pool: Vec<usize> = (0..self.items.len()).filter(|&j| self.items[j].crit.errors == min_err).collect();
        let mut out = Vec::new();
        let best = |key: &dyn Fn(&Criteria) -> f64| -> Option<usize> {
            pool.iter().copied().min_by(|&a, &b| key(&self.items[a].crit).total_cmp(&key(&self.items[b].crit)).then(a.cmp(&b)))
        };
        type Key<'k> = &'k dyn Fn(&Criteria) -> f64;
        let names: [(&str, Key); 4] = [
            ("shortest", &|c| c.length_mm),
            ("fewest vias", &|c| c.vias as f64),
            ("fewest shrinkages", &|c| c.shrinkages as f64),
            ("fewest warnings", &|c| c.warnings as f64),
        ];
        if best(&|c| c.errors as f64) == Some(i) {
            out.push("fewest errors".to_string());
        }
        for (n, k) in names {
            if best(k) == Some(i) {
                out.push(n.to_string());
            }
        }
        out
    }

    /// Offer a variant: kept when no archived one covers it (or it has the fewest warnings);
    /// archived variants it dominates go. Returns whether it was kept. O(archive).
    pub fn offer(&mut self, v: Variant<T>, obj: &Objective) -> bool {
        self.offered += 1;
        let fewest_warn = self.items.iter().all(|o| v.crit.warnings < o.crit.warnings);
        let covered = self.items.iter().any(|o| o.crit.covers(&v.crit));
        if covered && !fewest_warn {
            return false;
        }
        // Dominated ones go, except a variant that still has strictly the fewest warnings.
        let min_warn_other = |items: &[Variant<T>], skip: usize| items.iter().enumerate().filter(|(j, _)| *j != skip).map(|(_, o)| o.crit.warnings).min();
        let mut j = 0;
        while j < self.items.len() {
            let keep_for_warn = min_warn_other(&self.items, j).is_none_or(|m| self.items[j].crit.warnings < m) && self.items[j].crit.warnings < v.crit.warnings;
            if v.crit.dominates(&self.items[j].crit) && !keep_for_warn {
                self.items.remove(j);
            } else {
                j += 1;
            }
        }
        self.items.push(v);
        while self.items.len() > self.cap {
            // The worst by the objective (ranked errors, shorts, value) that is no champion and not the
            // variant the pick takes goes (the pick's own order has more keys than the
            // champions, e.g. shorts and open: pruning it would hand KiCad a worse variant).
            let pick = self.pick(obj);
            // TW3: the user's four bests are never pruned (topor-way plan, Appendix F point 9).
            let four = self.bests(obj);
            let worst = (0..self.items.len())
                .filter(|&i| Some(i) != pick && !four.contains(&Some(i)) && self.champions(i).is_empty())
                .max_by(|&a, &b| {
                    let (ca, cb) = (&self.items[a].crit, &self.items[b].crit);
                    ca.ranked().total_cmp(&cb.ranked()).then(ca.shorts.cmp(&cb.shorts)).then(obj.value(ca).total_cmp(&obj.value(cb))).then(a.cmp(&b))
                });
            match worst {
                Some(i) => {
                    self.items.remove(i);
                }
                None => break,
            }
        }
        true
    }

    /// TW3: the four bests ([`Role`], [`bests`]) by index: `[overall, errors, shortest, vias]`.
    /// O(archive).
    pub fn bests(&self, obj: &Objective) -> [Option<usize>; 4] {
        let cs: Vec<&Criteria> = self.items.iter().map(|v| &v.crit).collect();
        bests(&cs, obj)
    }

    /// The roles ([`Role`]) variant `i` holds. O(archive).
    pub fn roles(&self, i: usize, obj: &Objective) -> Vec<Role> {
        let b = self.bests(obj);
        Role::ALL.iter().zip(b).filter(|(_, x)| *x == Some(i)).map(|(r, _)| *r).collect()
    }

    /// Index of the variant the objective picks; `None` when empty.
    pub fn pick(&self, obj: &Objective) -> Option<usize> {
        let mut best: Option<usize> = None;
        for i in 0..self.items.len() {
            if best.is_none_or(|b| obj.better(&self.items[i].crit, &self.items[b].crit)) {
                best = Some(i);
            }
        }
        best
    }

    /// The variant table, sorted by errors, then length; `picked` marks the pick.
    pub fn table(&self, picked: Option<usize>) -> Vec<VariantRow> {
        let mut idx: Vec<usize> = (0..self.items.len()).collect();
        idx.sort_by(|&a, &b| {
            let (ca, cb) = (&self.items[a].crit, &self.items[b].crit);
            ca.errors.cmp(&cb.errors).then(ca.length_mm.total_cmp(&cb.length_mm)).then(a.cmp(&b))
        });
        let mut rows: Vec<VariantRow> = Vec::new();
        let mut prev: Option<&Criteria> = None;
        for &i in &idx {
            let c = &self.items[i].crit;
            let mm_per_via = prev
                .filter(|p| p.errors == c.errors && p.vias > c.vias)
                .map(|p| (c.length_mm - p.length_mm) / (p.vias - c.vias) as f64);
            rows.push(VariantRow {
                label: self.items[i].label.clone(),
                errors: c.errors,
                violations: c.violations,
                shorts: c.shorts,
                penalty_mm: c.penalty.round(),
                vias: c.vias,
                length_mm: (c.length_mm * 1000.0).round() / 1000.0,
                shrinkages: c.shrinkages,
                neck_mm2: (c.neck_mm2 * 1000.0).round() / 1000.0,
                neck_vias: c.neck_vias,
                necking: (c.necking * 1000.0).round() / 1000.0,
                warnings: c.warnings,
                area_mm2: (c.area_mm2 * 1000.0).round() / 1000.0,
                mm_per_via: mm_per_via.map(|x| (x * 100.0).round() / 100.0),
                champion: self.champions(i),
                picked: picked == Some(i),
            });
            prev = Some(c);
        }
        rows
    }
}

/// The variant table as report lines (one per variant).
pub fn table_text(rows: &[VariantRow], obj: &Objective) -> String {
    let objective = match obj {
        Objective::LengthVias { mm_per_via } => format!("length_vias, pick errors_weighted, {mm_per_via} mm/via"),
        Objective::ErrorsViasLength { .. } => "length_vias, pick errors_vias_length (errors, shorts, open, vias, length)".to_string(),
        Objective::Area => "area".to_string(),
        Objective::Best { mm_per_via, penalty } => format!("length_vias, pick best (errors, shorts, then length + {mm_per_via} mm/via + necking{})", if *penalty { " + violation price" } else { "" }),
    };
    let mut s = format!("variants ({} kept, objective {objective}; errors (violations, shorts) / vias / length mm / shrinkages / necking mm-eq (necked mm2 + neck vias) / warnings / area mm2 / mm per via):", rows.len());
    for r in rows {
        s.push_str(&format!(
            " [{}{}] {} ({}, {}) / {} / {:.1} / {} / {:.1} ({:.1} + {}) / {} / {:.1} / {}{};",
            if r.picked { "PICKED " } else { "" },
            r.label,
            r.errors,
            r.violations,
            r.shorts,
            r.vias,
            r.length_mm,
            r.shrinkages,
            r.necking,
            r.neck_mm2,
            r.neck_vias,
            r.warnings,
            r.area_mm2,
            r.mm_per_via.map_or("-".to_string(), |x| format!("{x:.1}")),
            if r.champion.is_empty() { String::new() } else { format!(" ({})", r.champion.join(", ")) }
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(errors: usize, vias: usize, length_mm: f64, shrinkages: usize, warnings: usize) -> Criteria {
        Criteria { errors, open: errors, vias, length_mm, shrinkages, warnings, area_mm2: length_mm * 0.4 + vias as f64 * 2.0, ..Criteria::default() }
    }

    fn arch(cs: &[Criteria]) -> Archive<usize> {
        let mut a = Archive::new(16);
        let obj = Objective::LengthVias { mm_per_via: 25.0 };
        for (i, x) in cs.iter().enumerate() {
            a.offer(Variant { label: format!("v{i}"), crit: *x, payload: i }, &obj);
        }
        a
    }

    #[test]
    fn dominated_variants_leave_and_are_refused() {
        let a = arch(&[c(2, 10, 100.0, 0, 0), c(1, 12, 110.0, 0, 0), c(1, 11, 105.0, 0, 0), c(1, 11, 105.0, 0, 0)]);
        // v0 has more errors but fewer vias and less length: non-dominated, it stays.
        let labels: Vec<&str> = a.items.iter().map(|v| v.label.as_str()).collect();
        assert_eq!(labels, vec!["v0", "v2"], "v1 dominated by v2, the copy of v2 refused");
        assert_eq!(a.offered, 4);
    }

    #[test]
    fn the_pick_is_fewest_errors_then_25_mm_per_via() {
        let obj = Objective::LengthVias { mm_per_via: 25.0 };
        // Within 0 errors: 20 vias / 500 mm (1000) vs 18 vias / 540 mm (990): two vias removed for
        // 40 mm (20 mm/via < 25) is worth it; 16 vias / 600 mm (1000) is not (30 mm/via).
        let a = arch(&[c(0, 20, 500.0, 0, 0), c(0, 18, 540.0, 0, 0), c(0, 16, 600.0, 0, 0), c(3, 5, 300.0, 0, 0)]);
        let p = a.pick(&obj).unwrap();
        assert_eq!(a.items[p].label, "v1");
        let t = a.table(Some(p));
        assert_eq!(t.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), vec!["v0", "v1", "v2", "v3"]);
        assert_eq!(t[1].mm_per_via, Some(20.0));
        assert_eq!(t[2].mm_per_via, Some(30.0));
        assert!(t[1].picked && !t[0].picked);
        assert!(t[0].champion.contains(&"shortest".to_string()));
        assert!(t[2].champion.contains(&"fewest vias".to_string()));
        assert!(t[3].champion.is_empty(), "fewest vias overall but not within the fewest errors");
        // The area objective picks the least area within the fewest errors.
        assert_eq!(a.items[a.pick(&Objective::Area).unwrap()].label, "v0");
    }

    #[test]
    fn violations_count_as_errors_and_their_price_ranks_within_a_count() {
        // M-B1 never drop: a drawn connection with one violation (price 3000) against the same
        // connection left open (price 20000, 5 mm shorter): one error each, the drawn one wins.
        let obj = Objective::LengthVias { mm_per_via: 25.0 };
        let drawn = Criteria { errors: 1, violations: 1, penalty: 3000.0, vias: 2, length_mm: 105.0, ..Criteria::default() };
        let open = Criteria { errors: 1, open: 1, penalty: 20000.0, vias: 2, length_mm: 100.0, ..Criteria::default() };
        let clean = Criteria { errors: 0, vias: 9, length_mm: 400.0, ..Criteria::default() };
        assert!(obj.better(&drawn, &open));
        assert!(obj.better(&clean, &drawn), "a violation is not compensated by any length (Polubasov 2010)");
        assert!(!drawn.covers(&open) && !open.covers(&drawn), "the price is a criterion: neither dominates");
        assert_eq!(obj.base(&drawn), 155.0);
        assert_eq!(obj.value(&drawn), 3155.0);
        let a = arch(&[open, drawn]);
        let t = a.table(a.pick(&obj));
        assert!(t.iter().any(|r| r.picked && r.violations == 1 && r.penalty_mm == 3000.0), "{t:?}");
    }

    #[test]
    fn the_fewest_warnings_variant_stays_even_when_dominated() {
        let a = arch(&[c(0, 10, 100.0, 1, 5), c(0, 12, 120.0, 2, 1), c(0, 9, 90.0, 0, 3)]);
        let labels: Vec<&str> = a.items.iter().map(|v| v.label.as_str()).collect();
        assert_eq!(labels, vec!["v1", "v2"], "v0 dominated by v2; v1 dominated too but has the fewest warnings");
        let t = a.table(None);
        assert!(t.iter().find(|r| r.label == "v1").unwrap().champion.contains(&"fewest warnings".to_string()));
    }

    #[test]
    fn the_cap_prunes_the_worst_non_champion() {
        let obj = Objective::LengthVias { mm_per_via: 25.0 };
        let mut a: Archive<usize> = Archive::new(3);
        for (i, x) in [c(0, 10, 100.0, 3, 0), c(0, 9, 140.0, 3, 0), c(0, 8, 200.0, 3, 0), c(0, 7, 400.0, 0, 0)].iter().enumerate() {
            a.offer(Variant { label: format!("v{i}"), crit: *x, payload: i }, &obj);
        }
        // All four are non-dominated; v0 shortest, v3 fewest vias and shrinkages, v1 the best
        // objective (365), v2 (400) is the worst non-champion.
        let labels: Vec<&str> = a.items.iter().map(|v| v.label.as_str()).collect();
        assert_eq!(labels, vec!["v0", "v1", "v3"]);
    }

    /// pick-necks (user 2026-11-27): `pick = best` ranks errors, shorts, then length + mm_per_via x vias
    /// + necking (one value), then shrinkages.
    #[test]
    fn best_pick_necking_is_part_of_the_value() {
        let o = Objective::from_options("length_vias", 25.0, "best");
        let mk = |len: f64, necking: f64| Criteria { length_mm: len, necking, ..Criteria::default() };
        // 100 + 12 = 112 > 90 + 3 = 93 ...
        let (short_necked, long_plain) = (mk(100.0, 12.0), mk(90.0 + 0.0, 3.0));
        assert!(o.better(&long_plain, &short_necked));
        // ... but necking is a price, not a strict key: 180 + 3 loses against 100 + 12
        let way_longer = mk(180.0, 3.0);
        assert!(o.better(&short_necked, &way_longer), "112 < 183: necking is a price, not a strict key");
        assert!(!o.better(&way_longer, &short_necked));
        assert_eq!(o.decider(&long_plain, &short_necked).map(|d| d.0), Some("score"), "length alone gives the same order: not the necking's doing");
        // length decides when the necking does not turn the comparison
        let (a, b) = (mk(80.0, 3.0), mk(100.0, 5.0));
        assert_eq!(o.decider(&a, &b).map(|d| d.0), Some("score"));
        let c2 = Criteria { vias: 1, ..mk(60.0, 3.0) };
        assert!(o.better(&a, &c2), "80 < 60 + 25 mm/via at equal necking");
        // the necking turns it against the shorter one
        let (s1, s2) = (mk(80.0, 30.0), mk(100.0, 3.0));
        assert!(o.better(&s2, &s1));
        assert_eq!(o.decider(&s2, &s1).map(|d| d.0), Some("necking"));
        // an exact tie in length + vias: the necking alone decides, whichever way the difference points
        let (t1, t2) = (mk(100.0, 12.0), mk(100.0, 3.0));
        assert_eq!(o.decider(&t1, &t2), Some(("necking", 9.0)));
        assert_eq!(o.decider(&t2, &t1), Some(("necking", -9.0)));
        // errors and shorts stay first
        let err = Criteria { errors: 1, ..mk(10.0, 0.0) };
        assert!(o.better(&short_necked, &err), "fewer errors beat less necking");
        // the other picks do not look at it
        let w = Objective::from_options("length_vias", 25.0, "errors_weighted");
        assert!(w.better(&short_necked, &way_longer));
        let e = Objective::from_options("length_vias", 25.0, "errors_vias_length");
        assert!(e.better(&short_necked, &way_longer));
    }

    /// pick-necks r2: a variant that necks more never covers one that necks less (the archive keeps the
    /// less necked drawn row), and an equal one is not dominated.
    #[test]
    fn necking_is_a_pareto_criterion() {
        let mk = |n: f64| Criteria { necking: n, ..Criteria::default() };
        assert!(mk(1.0).covers(&mk(2.0)) && !mk(2.0).covers(&mk(1.0)));
        assert!(mk(1.0).dominates(&mk(2.0)) && !mk(2.0).dominates(&mk(1.0)));
        assert!(mk(1.0).covers(&mk(1.0)) && !mk(1.0).dominates(&mk(1.0)));
        assert!(mk(1.0 + 1e-12).covers(&mk(1.0)), "within 1e-9");
    }

    #[test]
    fn errors_vias_length_pick_order() {
        let w = Objective::from_options("length_vias", 25.0, "errors_weighted");
        let n = Objective::from_options("length_vias", 25.0, "errors_vias_length");
        assert_eq!(w, Objective::LengthVias { mm_per_via: 25.0 });
        assert_eq!(n, Objective::ErrorsViasLength { mm_per_via: 25.0 });
        assert_eq!(Objective::from_options("length_vias", 25.0, "anything"), Objective::Best { mm_per_via: 25.0, penalty: false }, "unknown pick = the default (TW3 D5: best)");
        assert_eq!(Objective::from_options("area", 25.0, "errors_vias_length"), Objective::Area, "area ignores pick");
        // (1 error, 0 vias, short) loses to (0 errors, 9 vias, long): errors first, in both modes.
        let (e1, e0) = (c(1, 0, 10.0, 0, 0), c(0, 9, 900.0, 0, 0));
        assert!(n.better(&e0, &e1) && w.better(&e0, &e1));
        // (0 errors, 2 vias, 200 mm) beats (0 errors, 3 vias, 100 mm): vias before length. The old
        // weighted order prefers the second (200 + 50 = 250 vs 100 + 75 = 175 -> the second is lower).
        let (a, b) = (c(0, 2, 200.0, 0, 0), c(0, 3, 100.0, 0, 0));
        assert!(n.better(&a, &b) && !n.better(&b, &a));
        assert!(w.better(&b, &a) && !w.better(&a, &b), "the old mode reproduces the old order");
        // Length breaks a via tie; shrinkages break a length tie.
        let (s, l) = (c(0, 3, 100.0, 0, 0), c(0, 3, 101.0, 0, 0));
        assert!(n.better(&s, &l) && !n.better(&l, &s));
        let (z0, z1) = (c(0, 3, 100.0, 0, 0), c(0, 3, 100.0, 2, 0));
        assert!(n.better(&z0, &z1) && !n.better(&z1, &z0));
        // Same error count: drawn with a violation (open 0) beats left open, even with more vias.
        let open = c(1, 0, 10.0, 0, 0);
        let drawn = Criteria { open: 0, violations: 1, ..c(1, 4, 50.0, 0, 0) };
        assert!(n.better(&drawn, &open) && !n.better(&open, &drawn));
        // The pick over an archive (Pareto membership is the same in both modes).
        let a2 = arch(&[c(0, 20, 500.0, 0, 0), c(0, 18, 540.0, 0, 0), c(0, 16, 600.0, 0, 0), c(0, 14, 900.0, 0, 0), c(3, 1, 300.0, 0, 0)]);
        assert_eq!(a2.items.len(), 5);
        assert_eq!(a2.items[a2.pick(&n).unwrap()].label, "v3", "fewest vias within the fewest errors");
        assert_eq!(a2.items[a2.pick(&w).unwrap()].label, "v1", "old pick: least length + 25 mm per via");
    }

    #[test]
    fn decider_names_the_criterion_better_decides_on() {
        let n = Objective::from_options("length_vias", 25.0, "errors_vias_length");
        let w = Objective::from_options("length_vias", 25.0, "errors_weighted");
        let base = Criteria { errors: 3, open: 1, violations: 2, vias: 166, length_mm: 7380.0, shrinkages: 4, ..Criteria::default() };
        assert_eq!(n.decider(&base, &base), None, "the same score: no decider");
        let worse = Criteria { errors: 5, open: 0, vias: 160, ..base };
        assert_eq!(n.decider(&worse, &base), Some(("errors", 2.0)));
        assert_eq!(n.decider(&Criteria { open: 0, violations: 3, ..base }, &base), Some(("open", -1.0)));
        assert_eq!(n.decider(&Criteria { shorts: 1, open: 0, violations: 3, ..base }, &base), Some(("shorts", 1.0)), "shorts before open");
        assert_eq!(w.decider(&Criteria { shorts: 2, vias: 100, ..base }, &base), Some(("shorts", 2.0)), "shorts before the weighted score");
        assert_eq!(n.decider(&Criteria { vias: 168, length_mm: 1.0, ..base }, &base), Some(("vias", 2.0)));
        let (dn, dl) = n.decider(&Criteria { length_mm: 7410.0, ..base }, &base).expect("length differs");
        assert!(dn == "length" && (dl - 30.0).abs() < 1e-9);
        assert_eq!(n.decider(&Criteria { shrinkages: 1, ..base }, &base), Some(("shrinkages", -3.0)));
        assert_eq!(w.decider(&Criteria { vias: 167, ..base }, &base), Some(("score", 25.0)));
        assert_eq!(Objective::Area.decider(&Criteria { area_mm2: 2.0, ..base }, &base), Some(("area", 2.0)));
        // The sign of the decider agrees with `better` for every pick, on a grid of criteria.
        let mut grid = Vec::new();
        for errors in 0..3 {
            for open in 0..=errors.min(1) {
                for shorts in 0..=(errors - open).min(1) {
                    for vias in [10, 11] {
                        for length_mm in [100.0, 100.0 + 1e-12, 125.0] {
                            for shrinkages in 0..2 {
                                grid.push(Criteria { errors, shorts, open, vias, length_mm, shrinkages, area_mm2: length_mm * 0.4, ..Criteria::default() });
                            }
                        }
                    }
                }
            }
        }
        for obj in [n, w, Objective::Area] {
            for a in &grid {
                for b in &grid {
                    assert_eq!(obj.better(a, b), obj.decider(a, b).is_some_and(|(_, d)| d < 0.0), "{obj:?} {a:?} {b:?}");
                    assert_eq!(obj.decider(a, b).is_none(), !obj.better(a, b) && !obj.better(b, a), "{obj:?} {a:?} {b:?}");
                }
            }
        }
    }

    #[test]
    fn shorts_rank_after_errors_before_open_in_every_pick() {
        // User 2026-11-12 "shorts should be in the routing criteria": a connection with a short is
        // one of the errors (not counted twice) and, within one error count, worse than an open
        // connection or a clearance violation (KiCad's DRC fails the board on it, Cleanup removes
        // the track), whatever the vias and the length.
        let n = Objective::from_options("length_vias", 25.0, "errors_vias_length");
        let w = Objective::from_options("length_vias", 25.0, "errors_weighted");
        let short = Criteria { errors: 2, shorts: 1, open: 0, violations: 2, penalty: 4000.0, vias: 10, length_mm: 100.0, ..Criteria::default() };
        let open = Criteria { errors: 2, shorts: 0, open: 1, violations: 1, penalty: 20000.0, vias: 12, length_mm: 130.0, ..Criteria::default() };
        let clear = Criteria { errors: 2, shorts: 0, open: 0, violations: 2, penalty: 9000.0, vias: 14, length_mm: 150.0, ..Criteria::default() };
        for obj in [n, w, Objective::Area] {
            assert!(obj.better(&open, &short) && !obj.better(&short, &open), "{obj:?}: an open beats a short at the same errors");
            assert!(obj.better(&clear, &short) && !obj.better(&short, &clear), "{obj:?}: a clearance violation beats a short");
            assert_eq!(obj.decider(&short, &open), Some(("shorts", 1.0)), "{obj:?}");
        }
        // Without shorts on either side the old order holds: drawn (open 0) beats open.
        assert!(n.better(&clear, &open), "same errors and shorts: open decides, as before");
        // Errors stay first: one error that is a short beats two clean-of-shorts errors.
        let one = Criteria { errors: 1, shorts: 1, violations: 1, vias: 30, length_mm: 300.0, ..Criteria::default() };
        assert!(n.better(&one, &open) && w.better(&one, &open) && Objective::Area.better(&one, &open));
        // Pareto: more shorts never covers fewer, so the archive keeps the variant the pick takes.
        let cheap_short = Criteria { penalty: 3000.0, vias: 9, length_mm: 90.0, ..short };
        assert!(!cheap_short.covers(&clear) && !clear.covers(&cheap_short));
        let a = arch(&[cheap_short, clear]);
        assert_eq!(a.items.len(), 2, "both kept: neither dominates");
        assert_eq!(a.items[a.pick(&n).unwrap()].crit.shorts, 0, "the pick takes the one without a short");
        let t = a.table(a.pick(&n));
        assert!(t.iter().any(|r| r.picked && r.shorts == 0) && t.iter().any(|r| !r.picked && r.shorts == 1), "{t:?}");
        assert!(table_text(&t, &n).contains("(2, 1)"), "the table shows violations and shorts");
        // `pick_shorts` off: the builders leave `shorts` at 0 and the order is the old one.
        let (s0, o0) = (Criteria { shorts: 0, ..short }, open);
        assert!(n.better(&s0, &o0), "off: drawn with a violation beats open, as before 2026-11-12");
    }

    #[test]
    fn short_weight_grades_the_error_count_and_k_1_is_the_plain_count() {
        // PS1 (user 2026-11-13): ranked errors = errors + (k - 1) x shorts, the shorts only a
        // tie-break after it; k = 1 (short_extra 0) is the plain error count (TopoR 6.3 p. 72).
        let n = Objective::from_options("length_vias", 25.0, "errors_vias_length");
        let w = Objective::from_options("length_vias", 25.0, "errors_weighted");
        let few_err = Criteria { errors: 50, shorts: 25, violations: 50, vias: 190, length_mm: 4700.0, ..Criteria::default() };
        let few_sh = Criteria { errors: 55, shorts: 17, violations: 55, vias: 175, length_mm: 4480.0, ..Criteria::default() };
        for obj in [n, w, Objective::Area] {
            assert!(obj.better(&few_err, &few_sh), "{obj:?}: k = 1: fewer errors win (the 2026-11-12 order)");
            assert_eq!(obj.decider(&few_err, &few_sh), Some(("errors", -5.0)), "{obj:?}");
            // k = 2: 50 + 25 = 75 against 55 + 17 = 72: the one with fewer shorts wins, graded.
            let k = |c: Criteria, k: f64| Criteria { short_extra: k - 1.0, ..c };
            assert!(obj.better(&k(few_sh, 2.0), &k(few_err, 2.0)), "{obj:?}: k = 2: 8 shorts fewer outweigh 5 errors more");
            assert_eq!(obj.decider(&k(few_sh, 2.0), &k(few_err, 2.0)), Some(("errors", -3.0)), "{obj:?}");
            // k = 1.5: 62.5 against 63.5: fewer errors still win; no hard shorts key.
            assert!(obj.better(&k(few_err, 1.5), &k(few_sh, 1.5)), "{obj:?}: k = 1.5: not enough");
            // The same ranked count: the shorts break the tie.
            let a = Criteria { errors: 10, shorts: 2, short_extra: 1.0, ..Criteria::default() };
            let b = Criteria { errors: 12, shorts: 0, short_extra: 1.0, vias: 0, ..Criteria::default() };
            assert!((a.ranked() - b.ranked()).abs() < 1e-9 && obj.better(&b, &a), "{obj:?}: at 12 = 12 the one without shorts");
        }
        // The stable meter ranks the same way.
        use super::super::schedule::Meter;
        let sc = |c: &Criteria| super::super::schedule::Score { incomplete: c.errors, shorts: c.shorts, ranked: c.ranked(), value: 1.0, scale: 1.0 };
        let mut m = Meter::new(4, 0.5);
        assert!(m.push(sc(&Criteria { short_extra: 1.0, ..few_err })));
        assert!(m.push(sc(&Criteria { short_extra: 1.0, ..few_sh })), "k = 2: fewer shorts is a gain for the meter too");
    }

    #[test]
    fn cap_pruning_never_removes_the_pick() {
        // Over the cap the worst non-champion by (errors, shorts, value) goes, never the variant the
        // pick takes: the pick's order has keys the champions do not cover (shorts, open).
        let n = Objective::from_options("length_vias", 25.0, "errors_vias_length");
        let mk = |shorts: usize, open: usize, vias: usize, length_mm: f64, shrinkages: usize| Criteria { errors: 3, shorts, open, violations: 3 - open, vias, length_mm, shrinkages, ..Criteria::default() };
        let mut a: Archive<usize> = Archive::new(2);
        // v0 shortest, v1 fewest vias, v2 fewest shrinkages; v3 has no short but is worst by value.
        for (i, x) in [mk(1, 0, 10, 100.0, 3), mk(1, 0, 5, 200.0, 3), mk(1, 0, 8, 150.0, 0), mk(0, 1, 12, 400.0, 2)].iter().enumerate() {
            a.offer(Variant { label: format!("v{i}"), crit: *x, payload: i }, &n);
        }
        let p = a.pick(&n).expect("non-empty");
        assert_eq!(a.items[p].label, "v3", "{:?}", a.items.iter().map(|v| &v.label).collect::<Vec<_>>());
    }

    #[test]
    fn the_four_bests_are_never_pruned_at_any_cap() {
        // TW3 (Appendix F point 9): whatever the cap, after every offer the archive's four bests are
        // as good as the four bests over everything offered so far (a best leaves only for a
        // variant that dominates it, which then is at least as good on that role).
        use rand::{Rng, SeedableRng};
        let obj = Objective::from_options("length_vias", 25.0, "best");
        for seed in 0..40u64 {
            let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);
            let cap = 1 + (seed as usize % 4);
            let mut a: Archive<usize> = Archive::new(cap);
            let mut all: Vec<Criteria> = Vec::new();
            for i in 0..30 {
                let errors = rng.gen_range(0..4);
                let x = Criteria { errors, violations: errors, shorts: rng.gen_range(0..=errors.min(1)), penalty: rng.gen_range(0..3) as f64 * 100.0, vias: rng.gen_range(0..8), length_mm: rng.gen_range(80..120) as f64, shrinkages: rng.gen_range(0..2), ..Criteria::default() };
                all.push(x);
                a.offer(Variant { label: format!("v{i}"), crit: x, payload: i }, &obj);
                let refs: Vec<&Criteria> = all.iter().collect();
                let rb = bests(&refs, &obj);
                let ab = a.bests(&obj);
                let (r, b) = (|k: usize| refs[rb[k].unwrap()], |k: usize| &a.items[ab[k].unwrap()].crit);
                assert!(!obj.better(r(0), b(0)), "seed {seed} cap {cap}: overall lost");
                let ke = |c: &Criteria| (c.ranked(), c.shorts, c.penalty);
                let (x0, y0) = (ke(r(1)), ke(b(1)));
                assert!(y0.0 < x0.0 || y0.0 == x0.0 && (y0.1 < x0.1 || y0.1 == x0.1 && y0.2 <= x0.2), "seed {seed} cap {cap}: fewest errors lost");
                assert!(b(2).ranked() == r(2).ranked() && b(2).length_mm <= r(2).length_mm + 1e-9, "seed {seed}: shortest lost");
                assert!(b(3).ranked() == r(3).ranked() && b(3).vias <= r(3).vias, "seed {seed}: least vias lost");
            }
        }
    }

    #[test]
    fn best_orders_by_errors_then_length_plus_mm_per_via_and_reads_the_option() {
        // D5: fewest errors, then length + mm_per_via x vias; the penalty only with best_penalty.
        let b25 = Objective::from_options("length_vias", 25.0, "best");
        let b10 = Objective::from_options("length_vias", 10.0, "best");
        let (a, z) = (c(0, 2, 200.0, 0, 0), c(0, 3, 180.0, 0, 0));
        assert!(b25.better(&a, &z), "250 < 255 at 25 mm/via");
        assert!(b10.better(&z, &a), "210 < 220 at 10 mm/via: mm_per_via is read, not 25");
        let deep = Criteria { penalty: 5000.0, ..c(1, 0, 10.0, 0, 0) };
        let shallow = Criteria { penalty: 100.0, ..c(1, 0, 50.0, 0, 0) };
        assert!(b25.better(&deep, &shallow), "the penalty is not in the D5 order");
        let bp = b25.with_best_penalty(true);
        assert!(bp.better(&shallow, &deep), "best_penalty adds it");
        let w = Objective::from_options("length_vias", 25.0, "errors_weighted");
        for (x, y) in [(deep, shallow), (a, z), (z, a)] {
            assert_eq!(bp.better(&x, &y), w.better(&x, &y), "best + penalty = errors_weighted");
        }
        // the four roles: fewest errors keeps the penalty as its second key
        let rows = [deep, shallow, c(2, 0, 1.0, 0, 0)];
        let refs: Vec<&Criteria> = rows.iter().collect();
        assert_eq!(bests(&refs, &b25), [Some(0), Some(1), Some(0), Some(0)]);
        assert_eq!(Role::of_pick("shortest"), Role::Shortest);
        assert_eq!(Role::of_pick("errors_weighted"), Role::Overall);
    }
}
