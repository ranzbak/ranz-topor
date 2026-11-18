//! TW3 (topor-way plan, stage TW3; docs/dev/design/tw3-table.md): ONE variant table for all runs
//! of a call, kept deterministic.
//!
//! TopoR keeps the variants of its optimisation in one table and "periodically stops working on
//! the current variant and picks a variant from the table at random for further optimisation";
//! the descents are independent, so they can run in parallel from one table (Polubasov 2010
//! «Многовариантная оптимизация», fig. 13). The table holds the variants optimal for some
//! criterion; superseded ones leave (TopoR 6.3 manual pp. 72-73). Here the rows follow the
//! archive's Pareto rule ([`Archive::offer`]); the user's four bests ([`Role`]) are never pruned.
//!
//! Determinism (copper the same on any thread count and load): every run offers its state after
//! each cycle (cycle 0 = its initial plan); the offers are held and committed in (cycle, run)
//! order once every run still in its loop has finished that cycle. A run reads the table only at
//! its sync points ([`SharedTable::sync`]): there it waits until every other run has passed the
//! cycle before, so what it reads is the commit of all offers of the cycles before its own, a
//! function of the seeds alone. Complexity: an offer is O(log pending); a commit O(rows) per offer.

use super::archive::{bests, Archive, Criteria, Objective, Role, Variant};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Condvar, Mutex};

/// A row's payload: the state, where it came from and its fingerprint.
#[derive(Debug)]
pub struct Row<V> {
    pub var: Arc<V>,
    /// The run that offered it and the cycle; its generation (Polubasov 2010: every variant has a
    /// generation, it descends from another one).
    pub run: usize,
    pub cycle: usize,
    pub gen: usize,
    /// The state's fingerprint (equal states, equal fingerprints).
    pub fp: u64,
    /// The pre-loop key of its run (a row is only restored or drawn by a run with the same key).
    pub compat: u64,
}

impl<V> Clone for Row<V> {
    fn clone(&self) -> Self {
        Row { var: self.var.clone(), run: self.run, cycle: self.cycle, gen: self.gen, fp: self.fp, compat: self.compat }
    }
}

/// What a run reads at a sync point.
#[derive(Debug)]
pub struct View<V> {
    /// The committed rows: label, criteria, payload.
    pub rows: Vec<(String, Criteria, Row<V>)>,
    /// This run's offers taken into the table (cycles), ever.
    pub mine_accepted: Vec<usize>,
    /// The fingerprint every other run's state had at the end of the cycle before (runs still in
    /// their loop that offered it).
    pub fps_before: Vec<(usize, u64)>,
    /// The table's four bests gained nothing over the last `window` committed cycles and at least
    /// two restarts fell in them (D6: the headless stable stop).
    pub stable: bool,
}

struct Pending<V> {
    label: String,
    crit: Criteria,
    row: Row<V>,
    restarted: bool,
}

struct Inner<V> {
    arch: Archive<Row<V>>,
    /// Per run: the last cycle it offered (`None` before its initial plan) and whether its loop ended.
    done: Vec<Option<usize>>,
    ended: Vec<bool>,
    pending: BTreeMap<(usize, usize), Pending<V>>,
    /// The last cycle whose offers are all committed (`None`: none yet).
    committed: Option<usize>,
    accepted: BTreeSet<(usize, usize)>,
    fps: BTreeMap<(usize, usize), u64>,
    /// Per committed cycle: did a best change to a row of that cycle; restarts in that cycle.
    gain: Vec<bool>,
    restarts: Vec<usize>,
    offers: usize,
    /// The cycles each run restarted at (committed).
    restart_at: BTreeSet<(usize, usize)>,
}

/// The call's one variant table ([`Row`]s), shared by its runs.
pub struct SharedTable<V> {
    inner: Mutex<Inner<V>>,
    cv: Condvar,
    obj: Objective,
    window: usize,
}

/// The identity of a row (run, cycle) for the gain test.
fn ident<V>(a: &Archive<Row<V>>, obj: &Objective) -> [Option<(usize, usize)>; 4] {
    a.bests(obj).map(|b| b.map(|i| (a.items[i].payload.run, a.items[i].payload.cycle)))
}

impl<V> SharedTable<V> {
    /// A table for `runs` runs holding at most `cap` rows besides the protected ones; `window` =
    /// the stable window in committed cycles (`[optimize] loop_stable_window`).
    pub fn new(runs: usize, cap: usize, obj: Objective, window: usize) -> SharedTable<V> {
        let n = runs.max(1);
        SharedTable {
            inner: Mutex::new(Inner { arch: Archive::new(cap), done: vec![None; n], ended: vec![false; n], pending: BTreeMap::new(), committed: None, accepted: BTreeSet::new(), fps: BTreeMap::new(), gain: Vec::new(), restarts: Vec::new(), offers: 0, restart_at: BTreeSet::new() }),
            cv: Condvar::new(),
            obj,
            window: window.max(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner<V>> {
        // A poisoned lock = a run panicked while it held the table: the call's scope passes that
        // panic on; the table's state is consistent between its statements, so take it as it is.
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Run `run` offers its state after cycle `cycle` (0 = the initial plan); `restarted`: the
    /// cycle began with a restart. Commits what is ready. O(log pending) + the commits.
    pub fn offer(&self, run: usize, cycle: usize, label: String, crit: Criteria, row: Row<V>, restarted: bool) {
        let mut g = self.lock();
        if run >= g.done.len() || g.ended[run] {
            return;
        }
        g.fps.insert((cycle, run), row.fp);
        g.pending.insert((cycle, run), Pending { label, crit, row, restarted });
        g.done[run] = Some(cycle);
        self.commit(&mut g);
        drop(g);
        self.cv.notify_all();
    }

    /// Run `run`'s loop ended (also: it never began, or its thread unwinds): it holds no cycle back
    /// any more. Idempotent. O(commits).
    pub fn end(&self, run: usize) {
        let mut g = self.lock();
        if run < g.ended.len() && !g.ended[run] {
            g.ended[run] = true;
            self.commit(&mut g);
        }
        drop(g);
        self.cv.notify_all();
    }

    /// The first cycle some run still in its loop has not finished (usize::MAX: every run ended).
    fn horizon(g: &Inner<V>) -> usize {
        (0..g.done.len()).filter(|&r| !g.ended[r]).map(|r| g.done[r].map_or(0, |c| c + 1)).min().unwrap_or(usize::MAX)
    }

    /// Commit the held offers of every cycle all runs have passed, in (cycle, run) order.
    fn commit(&self, g: &mut Inner<V>) {
        let h = Self::horizon(g);
        while let Some((c, _)) = g.pending.keys().next().copied() {
            if c >= h {
                break;
            }
            let before = ident(&g.arch, &self.obj);
            let mut restarted = 0;
            // every offer of cycle c, in run order
            let keys: Vec<(usize, usize)> = g.pending.range((c, 0)..(c + 1, 0)).map(|(k, _)| *k).collect();
            for k in keys {
                let p = g.pending.remove(&k).expect("key");
                restarted += usize::from(p.restarted);
                if p.restarted {
                    g.restart_at.insert((k.1, k.0));
                }
                g.offers += 1;
                if g.arch.offer(Variant { label: p.label, crit: p.crit, payload: p.row }, &self.obj) {
                    g.accepted.insert((k.1, k.0));
                }
            }
            let after = ident(&g.arch, &self.obj);
            let gained = after.iter().zip(before.iter()).any(|(a, b)| a != b && a.is_some_and(|x| x.1 == c));
            // cycles without offers in between (a run that ended) count as no gain
            let from = g.committed.map_or(0, |x| x + 1);
            for _ in from..c {
                g.gain.push(false);
                g.restarts.push(0);
            }
            g.gain.push(gained);
            g.restarts.push(restarted);
            g.committed = Some(c);
        }
    }

    /// The headless stable test over the committed cycles (D6). O(window).
    fn stable_now(&self, g: &Inner<V>) -> bool {
        let n = g.gain.len();
        if n <= self.window {
            return false;
        }
        let w = n - self.window..n;
        !g.gain[w.clone()].iter().any(|&x| x) && g.restarts[w].iter().sum::<usize>() >= 2
    }

    /// Run `run` at the start of cycle `cycle` reads the table: waits (calling `before_wait` once
    /// and `after_wait` after) until every other run still in its loop has finished cycle `cycle -
    /// 1`; `stop()` true ends the wait without a view (Cancel, the limit). O(rows) + the wait.
    pub fn sync(&self, run: usize, cycle: usize, stop: &mut dyn FnMut() -> bool, before_wait: &dyn Fn(), after_wait: &dyn Fn()) -> Option<View<V>> {
        let need = cycle.saturating_sub(1);
        let mut g = self.lock();
        let ready = |g: &Inner<V>| Self::horizon(g) > need || (0..g.done.len()).all(|r| r == run || g.ended[r] || g.done[r].is_some_and(|c| c >= need));
        if !ready(&g) {
            // never wait (nor call `stop`, which may wait for a compute slot) holding the lock
            drop(g);
            before_wait();
            loop {
                if stop() {
                    after_wait();
                    return None;
                }
                let g2 = self.lock();
                if ready(&g2) {
                    break;
                }
                drop(self.cv.wait_timeout(g2, std::time::Duration::from_millis(100)).unwrap_or_else(std::sync::PoisonError::into_inner));
            }
            after_wait();
            g = self.lock();
        }
        // Commit through `need`: every run passed it (this one too: it offered cycle - 1).
        self.commit(&mut g);
        let rows = g
            .arch
            .items
            .iter()
            .filter(|v| v.payload.cycle <= need)
            .map(|v| (v.label.clone(), v.crit, v.payload.clone()))
            .collect();
        let mine_accepted = g.accepted.iter().filter(|(r, c)| *r == run && *c <= need).map(|(_, c)| *c).collect();
        let fps_before = g.fps.range((need, 0)..(need + 1, 0)).filter(|((_, r), _)| *r != run).map(|((_, r), f)| (*r, *f)).collect();
        // `stable` over the committed cycles up to `need` (later commits are not this run's view).
        let stable = {
            let n = (need + 1).min(g.gain.len());
            n > self.window && !g.gain[n - self.window..n].iter().any(|&x| x) && g.restarts[n - self.window..n].iter().sum::<usize>() >= 2
        };
        Some(View { rows, mine_accepted, fps_before, stable })
    }

    /// The rows of the final table that hold one of the four bests, distinct, in the role order
    /// (overall, errors, shortest, vias), each with its roles. Call after every run ended. O(rows).
    pub fn final_bests(&self) -> Vec<(Vec<Role>, String, Criteria, Row<V>)> {
        let mut g = self.lock();
        self.commit(&mut g);
        let cs: Vec<&Criteria> = g.arch.items.iter().map(|v| &v.crit).collect();
        let b = bests(&cs, &self.obj);
        let mut out: Vec<(Vec<Role>, String, Criteria, Row<V>)> = Vec::new();
        let mut seen: Vec<usize> = Vec::new();
        for (role, i) in Role::ALL.iter().zip(b) {
            let Some(i) = i else { continue };
            match seen.iter().position(|&s| s == i) {
                Some(p) => out[p].0.push(*role),
                None => {
                    seen.push(i);
                    let v = &g.arch.items[i];
                    out.push((vec![*role], v.label.clone(), v.crit, v.payload.clone()));
                }
            }
        }
        out
    }

    /// The table for the report (rows, offers, stable now), after every run ended.
    pub fn summary(&self) -> (Vec<super::archive::VariantRow>, usize, bool) {
        let mut g = self.lock();
        self.commit(&mut g);
        let p = g.arch.pick(&self.obj);
        (g.arch.table(p), g.offers, self.stable_now(&g))
    }

    /// The restarts of all runs and how many of them led to a new row (an offer of that run taken
    /// into the table before its next restart), after every run ended. O(restarts x accepted).
    pub fn restart_stats(&self) -> (usize, usize) {
        let mut g = self.lock();
        self.commit(&mut g);
        let rs: Vec<(usize, usize)> = g.restart_at.iter().copied().collect();
        let new_row = (0..rs.len())
            .filter(|&i| {
                let (r, c) = rs[i];
                let next = rs.get(i + 1).filter(|x| x.0 == r).map_or(usize::MAX, |x| x.1);
                g.accepted.iter().any(|&(ar, ac)| ar == r && ac >= c && ac < next)
            })
            .count();
        (rs.len(), new_row)
    }

    /// Rows now (committed). O(1).
    pub fn len(&self) -> usize {
        self.lock().arch.items.len()
    }

    /// Whether the table has no committed row. O(1).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Accepted offers per run after a cycle in `[from, to)` (the restarts that led to a new row). O(accepted).
    pub fn accepted_in(&self, run: usize, from: usize, to: usize) -> usize {
        self.lock().accepted.iter().filter(|(r, c)| *r == run && *c >= from && *c < to).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(errors: usize, vias: usize, length_mm: f64) -> Criteria {
        Criteria { errors, violations: errors, vias, length_mm, ..Criteria::default() }
    }

    fn row(run: usize, cycle: usize) -> Row<usize> {
        Row { var: Arc::new(run * 1000 + cycle), run, cycle, gen: cycle, fp: (run * 1000 + cycle) as u64, compat: 0 }
    }

    /// The same offers arriving in any order give the same table (commit order by cycle id).
    #[test]
    fn the_table_does_not_depend_on_the_arrival_order() {
        use rand::seq::SliceRandom;
        use rand::SeedableRng;
        let obj = Objective::from_options("length_vias", 25.0, "best");
        let offers: Vec<(usize, usize, Criteria)> = (0..3).flat_map(|r| (0..6).map(move |cy| (r, cy, c((7 * r + 3 * cy) % 5, (11 * r + 5 * cy) % 9, 100.0 + ((13 * r + 7 * cy) % 17) as f64)))).collect();
        let run_table = |order: &[(usize, usize, Criteria)]| {
            let t: SharedTable<usize> = SharedTable::new(3, 3, obj, 4);
            for &(r, cy, cr) in order {
                t.offer(r, cy, format!("r{r}c{cy}"), cr, row(r, cy), false);
            }
            for r in 0..3 {
                t.end(r);
            }
            let (rows, n, _) = t.summary();
            (rows.iter().map(|x| x.label.clone()).collect::<Vec<_>>(), n)
        };
        let base = run_table(&offers);
        assert_eq!(base.1, 18);
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(5);
        for _ in 0..20 {
            // a run's own cycles stay in order (a run offers cycle c after c - 1); runs interleave freely
            let mut per: Vec<Vec<(usize, usize, Criteria)>> = (0..3).map(|r| offers.iter().copied().filter(|o| o.0 == r).collect()).collect();
            let mut order = Vec::new();
            while per.iter().any(|p| !p.is_empty()) {
                let mut live: Vec<usize> = (0..3).filter(|&r| !per[r].is_empty()).collect();
                live.shuffle(&mut rng);
                order.push(per[live[0]].remove(0));
            }
            assert_eq!(run_table(&order), base);
        }
    }

    /// Offers are held until every run in its loop passed that cycle.
    #[test]
    fn offers_wait_for_the_slowest_run() {
        let obj = Objective::from_options("length_vias", 25.0, "best");
        let t: SharedTable<usize> = SharedTable::new(2, 8, obj, 4);
        t.offer(0, 0, "a0".into(), c(3, 1, 10.0), row(0, 0), false);
        t.offer(0, 1, "a1".into(), c(1, 1, 10.0), row(0, 1), false);
        assert_eq!(t.len(), 0, "run 1 has not offered its initial plan");
        t.offer(1, 0, "b0".into(), c(2, 1, 10.0), row(1, 0), false);
        assert_eq!(t.len(), 1, "cycle 0 committed: b0 dominates a0");
        t.end(1);
        assert_eq!(t.len(), 1, "a1 committed, dominates b0");
        let fb = t.final_bests();
        assert_eq!(fb.len(), 1);
        assert_eq!(fb[0].0, Role::ALL.to_vec());
        assert_eq!(fb[0].1, "a1");
    }

    /// A sync waits for the other runs and sees only the cycles before its own.
    #[test]
    fn sync_sees_the_commit_of_the_cycles_before() {
        let obj = Objective::from_options("length_vias", 25.0, "best");
        let t: Arc<SharedTable<usize>> = Arc::new(SharedTable::new(2, 8, obj, 4));
        t.offer(0, 0, "a0".into(), c(3, 1, 10.0), row(0, 0), false);
        t.offer(0, 1, "a1".into(), c(3, 0, 50.0), row(0, 1), false);
        let t2 = t.clone();
        let h = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            t2.offer(1, 0, "b0".into(), c(3, 2, 5.0), row(1, 0), false);
            t2.offer(1, 1, "b1".into(), c(0, 9, 900.0), row(1, 1), false);
            t2.offer(1, 2, "b2".into(), c(0, 1, 9.0), row(1, 2), false);
        });
        let waited = std::cell::Cell::new(0);
        let v = t.sync(0, 2, &mut || false, &|| waited.set(waited.get() + 1), &|| {}).expect("view");
        h.join().unwrap();
        let labels: Vec<&str> = v.rows.iter().map(|r| r.0.as_str()).collect();
        assert!(labels.contains(&"a1") && labels.contains(&"b1") && !labels.contains(&"b2"), "{labels:?}");
        assert_eq!(v.fps_before, vec![(1, 1001)]);
        assert_eq!(v.mine_accepted, vec![0, 1]);
        // a stop ends the wait without a view
        let t3: SharedTable<usize> = SharedTable::new(2, 8, obj, 4);
        t3.offer(0, 0, "a0".into(), c(1, 1, 1.0), row(0, 0), false);
        assert!(t3.sync(0, 1, &mut || true, &|| {}, &|| {}).is_none());
    }

    /// Stable (headless) needs a window without a gain of the four bests and two restarts in it.
    #[test]
    fn stable_needs_no_gain_and_two_restarts() {
        let obj = Objective::from_options("length_vias", 25.0, "best");
        for restarts in [0usize, 1, 2] {
            let t: SharedTable<usize> = SharedTable::new(1, 8, obj, 3);
            t.offer(0, 0, "a0".into(), c(1, 1, 10.0), row(0, 0), false);
            for cy in 1..=4 {
                t.offer(0, cy, format!("a{cy}"), c(2, 2, 20.0), row(0, cy), cy > 4 - restarts);
            }
            assert_eq!(t.summary().2, restarts >= 2, "restarts {restarts}");
        }
        // a gain inside the window: not stable
        let t: SharedTable<usize> = SharedTable::new(1, 8, obj, 3);
        t.offer(0, 0, "a0".into(), c(1, 1, 10.0), row(0, 0), true);
        for cy in 1..=4 {
            let cr = if cy == 3 { c(0, 1, 10.0) } else { c(2, 2, 20.0) };
            t.offer(0, cy, format!("a{cy}"), cr, row(0, cy), true);
        }
        assert!(!t.summary().2);
    }
}
