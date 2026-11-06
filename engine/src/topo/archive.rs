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
//! * The pick: fewest errors first (lexicographic), then by `objective`: `length_vias` = least
//!   `length + mm_per_via x vias` (the 25 mm/via rule of the TopoR manual: a step to fewer vias is
//!   taken while it costs at most `mm_per_via` of length per via removed); `area` = least occupied
//!   area (a trace counts `L x (w + s)`, a via its disc of pad plus clearance on every layer it
//!   spans). Ties: fewer shrinkages, then the earlier variant.
//!
//! Deterministic: Vec in offer order, no hashing. Complexity: an offer is O(archive size).

use serde::Serialize;

/// The board-level numbers of one variant.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Criteria {
    /// Connections not routed (errors: unrouted wires and the violations refused).
    pub errors: usize,
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
    /// At least as good on all four criteria (errors, vias, length, shrinkages).
    pub fn covers(&self, o: &Criteria) -> bool {
        self.errors <= o.errors && self.vias <= o.vias && self.length_mm <= o.length_mm + 1e-9 && self.shrinkages <= o.shrinkages
    }
    /// Covers `o` and is better on at least one criterion.
    pub fn dominates(&self, o: &Criteria) -> bool {
        self.covers(o) && (self.errors < o.errors || self.vias < o.vias || self.length_mm < o.length_mm - 1e-9 || self.shrinkages < o.shrinkages)
    }
}

/// The board objective (`[optimize] objective`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Objective {
    /// Pareto archive pick: fewest errors, then least `length + mm_per_via x vias`.
    LengthVias { mm_per_via: f64 },
    /// Fewest errors, then least occupied area.
    Area,
}

impl Objective {
    /// `"length_vias"` | `"area"`; anything else is `length_vias`.
    pub fn parse(s: &str, mm_per_via: f64) -> Objective {
        if s == "area" {
            Objective::Area
        } else {
            Objective::LengthVias { mm_per_via }
        }
    }
    /// The objective's value within one error count (lower is better).
    pub fn value(&self, c: &Criteria) -> f64 {
        match *self {
            Objective::LengthVias { mm_per_via } => c.length_mm + mm_per_via * c.vias as f64,
            Objective::Area => c.area_mm2,
        }
    }
    /// Lexicographic key: errors, objective value, shrinkages.
    pub fn better(&self, a: &Criteria, b: &Criteria) -> bool {
        a.errors < b.errors
            || a.errors == b.errors
                && (self.value(a) < self.value(b) - 1e-9 || (self.value(a) - self.value(b)).abs() <= 1e-9 && a.shrinkages < b.shrinkages)
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
        Objective::LengthVias { mm_per_via } => format!("length_vias, {mm_per_via} mm/via"),
        Objective::Area => "area".to_string(),
    };
    let mut s = format!("variants ({} kept, objective {objective}; errors / vias / length mm / shrinkages / warnings / area mm2 / mm per via):", rows.len());
    for r in rows {
        s.push_str(&format!(
            " [{}{}] {} / {} / {:.1} / {} / {} / {:.1} / {}{};",
            if r.picked { "PICKED " } else { "" },
            r.label,
            r.errors,
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
        Criteria { errors, vias, length_mm, shrinkages, warnings, area_mm2: length_mm * 0.4 + vias as f64 * 2.0 }
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
}
