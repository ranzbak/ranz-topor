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
//! * The pick: fewest errors first (lexicographic), then by `objective` and `[optimize] pick`:
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
    /// Warning-level factor findings (a column, not a Pareto criterion).
    pub warnings: usize,
    /// Occupied area (mm²) for the `area` objective.
    pub area_mm2: f64,
}

impl Criteria {
    /// At least as good on all four criteria (errors with their price, vias, length, shrinkages).
    pub fn covers(&self, o: &Criteria) -> bool {
        self.errors <= o.errors && self.penalty <= o.penalty + 1e-9 && self.vias <= o.vias && self.length_mm <= o.length_mm + 1e-9 && self.shrinkages <= o.shrinkages
    }
    /// Covers `o` and is better on at least one criterion.
    pub fn dominates(&self, o: &Criteria) -> bool {
        self.covers(o) && (self.errors < o.errors || self.penalty < o.penalty - 1e-9 || self.vias < o.vias || self.length_mm < o.length_mm - 1e-9 || self.shrinkages < o.shrinkages)
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
    pub fn from_options(objective: &str, mm_per_via: f64, pick: &str) -> Objective {
        match Objective::parse(objective, mm_per_via) {
            Objective::LengthVias { mm_per_via } if pick != "errors_weighted" => Objective::ErrorsViasLength { mm_per_via },
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
            Objective::LengthVias { mm_per_via } | Objective::ErrorsViasLength { mm_per_via } => c.length_mm + mm_per_via * c.vias as f64,
            Objective::Area => c.area_mm2,
        }
    }
    /// Lexicographic key: errors, objective value, shrinkages (`ErrorsViasLength`: errors, open
    /// connections, vias, length, shrinkages; lengths within 1e-9 mm are equal).
    pub fn better(&self, a: &Criteria, b: &Criteria) -> bool {
        if let Objective::ErrorsViasLength { .. } = self {
            if a.errors != b.errors {
                return a.errors < b.errors;
            }
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
        a.errors < b.errors
            || a.errors == b.errors
                && (self.value(a) < self.value(b) - 1e-9 || (self.value(a) - self.value(b)).abs() <= 1e-9 && a.shrinkages < b.shrinkages)
    }
    /// The criterion on which [`Objective::better`] decides between `a` and `b`, in the pick
    /// order, with `a` minus `b` on it (`a` is better exactly when that is negative); `None` when
    /// they score the same. Names: `"errors"`, `"open"`, `"vias"`, `"length"` (mm),
    /// `"shrinkages"`, and for the other picks `"score"` (`errors_weighted`: length +
    /// mm_per_via x vias + the violation price, mm) or `"area"` (mm² + the violation price).
    /// The progress dialog and the per-round log line say with it why a round is not the best. O(1).
    pub fn decider(&self, a: &Criteria, b: &Criteria) -> Option<(&'static str, f64)> {
        let d = |x: usize, y: usize| x as f64 - y as f64;
        if a.errors != b.errors {
            return Some(("errors", d(a.errors, b.errors)));
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
            let dv = self.value(a) - self.value(b);
            if dv.abs() > 1e-9 {
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
    /// The `violation` factor's price of the errors (mm).
    pub penalty_mm: f64,
    pub vias: usize,
    pub length_mm: f64,
    pub shrinkages: usize,
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
            // The worst by the objective that is no champion goes.
            let worst = (0..self.items.len())
                .filter(|&i| self.champions(i).is_empty())
                .max_by(|&a, &b| {
                    let (ca, cb) = (&self.items[a].crit, &self.items[b].crit);
                    ca.errors.cmp(&cb.errors).then(obj.value(ca).total_cmp(&obj.value(cb))).then(a.cmp(&b))
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
                penalty_mm: c.penalty.round(),
                vias: c.vias,
                length_mm: (c.length_mm * 1000.0).round() / 1000.0,
                shrinkages: c.shrinkages,
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
        Objective::ErrorsViasLength { .. } => "length_vias, pick errors_vias_length (errors, vias, length)".to_string(),
        Objective::Area => "area".to_string(),
    };
    let mut s = format!("variants ({} kept, objective {objective}; errors (violations) / vias / length mm / shrinkages / warnings / area mm2 / mm per via):", rows.len());
    for r in rows {
        s.push_str(&format!(
            " [{}{}] {} ({}) / {} / {:.1} / {} / {} / {:.1} / {}{};",
            if r.picked { "PICKED " } else { "" },
            r.label,
            r.errors,
            r.violations,
            r.vias,
            r.length_mm,
            r.shrinkages,
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

    #[test]
    fn errors_vias_length_pick_order() {
        let w = Objective::from_options("length_vias", 25.0, "errors_weighted");
        let n = Objective::from_options("length_vias", 25.0, "errors_vias_length");
        assert_eq!(w, Objective::LengthVias { mm_per_via: 25.0 });
        assert_eq!(n, Objective::ErrorsViasLength { mm_per_via: 25.0 });
        assert_eq!(Objective::from_options("length_vias", 25.0, "anything"), n, "unknown pick = the default");
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
                for vias in [10, 11] {
                    for length_mm in [100.0, 100.0 + 1e-12, 125.0] {
                        for shrinkages in 0..2 {
                            grid.push(Criteria { errors, open, vias, length_mm, shrinkages, area_mm2: length_mm * 0.4, ..Criteria::default() });
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
}
