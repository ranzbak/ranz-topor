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
//! its sync points ([`SharedTable::sync`]): at cycle c it reads T(c-1-L), the commit of all offers
//! of the cycles through c-1-L (L = `[optimize] table_lag`, sync-wait; L = 0 = the cycle before its
//! own, TW3), and waits only while another run in its loop has not finished cycle c-1-L, so what
//! it reads is a function of the seeds alone. With L > 0 a run reads a table up to L cycles older
//! than the newest and seldom waits (bounded staleness, the stale synchronous parallel model:
//! Ho et al. 2013, NIPS, section 2; here the view is exactly T(c-1-L), not "at least", which keeps
//! it deterministic). Commits stop at `min(live done) - L`, the oldest table a run in its loop can
//! still read; a run ahead of the slowest one replays the held offers of the cycles in between on a
//! copy of the committed state (docs/dev/design/sync-wait.md). Complexity: an offer is
//! O(log pending); a commit O(rows) per offer; a replayed view O(rows + accepted) + O(rows) per
//! replayed offer.

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
    /// The table's four bests gained nothing for at least `window` committed cycles and, since
    /// their last gain, two period checks found a free descent (D6: the headless stable stop).
    pub stable: bool,
}

/// What a cycle began with: no period check, a check that found the descent free but no other row
/// to go to, a restart from a row. O(1).
pub const CHECK_NONE: u8 = 0;
pub const CHECK_FREE: u8 = 1;
pub const CHECK_RESTART: u8 = 2;

/// D6 stable over the first `n` committed cycles: the four bests gained nothing for at least
/// `window` cycles, and since their last gain at least two period checks found a free descent
/// (restarted, or nothing else to go to). Counting the checks since the last gain, not inside
/// the window: a run's checks are at least `restart_min_cycles` apart, so two of them never fall
/// in a window of that length. O(n).
fn stable_upto(gain: &[bool], checks: &[usize], n: usize, window: usize) -> bool {
    let from = gain[..n].iter().rposition(|&x| x).map_or(0, |i| i + 1);
    n - from >= window && checks[from..n].iter().sum::<usize>() >= 2
}

struct Pending<V> {
    label: String,
    crit: Criteria,
    row: Row<V>,
    check: u8,
}

/// The committed part of the table: every offer of the cycles through `committed`, in (cycle, run,
/// seq) order, and what the commits recorded. Cloned for a replayed view ([`SharedTable::sync`]).
struct Committed<V> {
    arch: Archive<Row<V>>,
    /// The last cycle whose offers are all committed (`None`: none yet).
    committed: Option<usize>,
    accepted: BTreeSet<(usize, usize)>,
    /// Per committed cycle: did a best change to a row of that cycle; restarts in that cycle.
    gain: Vec<bool>,
    restarts: Vec<usize>,
    offers: usize,
    /// The cycles each run restarted at (committed).
    restart_at: BTreeSet<(usize, usize)>,
}

impl<V> Clone for Committed<V> {
    fn clone(&self) -> Self {
        Committed { arch: self.arch.clone(), committed: self.committed, accepted: self.accepted.clone(), gain: self.gain.clone(), restarts: self.restarts.clone(), offers: self.offers, restart_at: self.restart_at.clone() }
    }
}

impl<V> Committed<V> {
    /// Commit the held offers of cycle `c` (every one of them final) in run order; a mid-cycle
    /// offer of a cycle its run never finished (cut short, ended) is dropped. Reads `pending` only
    /// (the commit removes cycle `c` afterwards; a replayed view leaves it). O(offers x rows).
    fn apply(&mut self, pending: &BTreeMap<(usize, usize, usize), Pending<V>>, c: usize, obj: &Objective) {
        let before = ident(&self.arch, obj);
        let mut restarted = 0;
        for (k, p) in pending.range((c, 0, 0)..(c + 1, 0, 0)) {
            if k.2 != usize::MAX && !pending.contains_key(&(c, k.1, usize::MAX)) {
                continue;
            }
            restarted += usize::from(p.check >= CHECK_FREE);
            if p.check == CHECK_RESTART {
                self.restart_at.insert((k.1, k.0));
            }
            self.offers += 1;
            if self.arch.offer(Variant { label: p.label.clone(), crit: p.crit, payload: p.row.clone() }, obj) {
                self.accepted.insert((k.1, k.0));
            }
        }
        let after = ident(&self.arch, obj);
        let gained = after.iter().zip(before.iter()).any(|(a, b)| a != b && a.is_some_and(|x| x.1 == c));
        // cycles without offers in between (a run that ended) count as no gain
        let from = self.committed.map_or(0, |x| x + 1);
        for _ in from..c {
            self.gain.push(false);
            self.restarts.push(0);
        }
        self.gain.push(gained);
        self.restarts.push(restarted);
        self.committed = Some(c);
    }
}

struct Inner<V> {
    st: Committed<V>,
    /// Per run: the last cycle it offered (`None` before its initial plan) and whether its loop ended.
    done: Vec<Option<usize>>,
    ended: Vec<bool>,
    /// Held offers by (cycle, run, seq): a run's mid-cycle offers ([`SharedTable::offer_mid`],
    /// seq 0, 1, ..) commit before its cycle-end offer (seq `usize::MAX`).
    pending: BTreeMap<(usize, usize, usize), Pending<V>>,
    /// Per run: the mid-cycle offers held for the cycle it is in.
    mid_seq: Vec<usize>,
    fps: BTreeMap<(usize, usize), u64>,
}

/// The call's one variant table ([`Row`]s), shared by its runs.
pub struct SharedTable<V> {
    inner: Mutex<Inner<V>>,
    cv: Condvar,
    obj: Objective,
    window: usize,
    /// `[optimize] table_lag` (sync-wait): a run at cycle c reads T(c-1-lag).
    lag: usize,
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
            inner: Mutex::new(Inner {
                st: Committed { arch: Archive::new(cap), committed: None, accepted: BTreeSet::new(), gain: Vec::new(), restarts: Vec::new(), offers: 0, restart_at: BTreeSet::new() },
                done: vec![None; n],
                ended: vec![false; n],
                pending: BTreeMap::new(),
                mid_seq: vec![0; n],
                fps: BTreeMap::new(),
            }),
            cv: Condvar::new(),
            obj,
            window: window.max(1),
            lag: 0,
        }
    }

    /// The same table read with lag `lag` cycles (`[optimize] table_lag`; 0 = TW3's lock-step):
    /// a run at cycle c reads T(c-1-lag). O(1).
    pub fn with_lag(mut self, lag: usize) -> SharedTable<V> {
        self.lag = lag;
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner<V>> {
        // A poisoned lock = a run panicked while it held the table: the call's scope passes that
        // panic on; the table's state is consistent between its statements, so take it as it is.
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Run `run` offers its state after cycle `cycle` (0 = the initial plan); `check`: what the
    /// cycle began with ([`CHECK_NONE`], [`CHECK_FREE`], [`CHECK_RESTART`]). Commits what is
    /// ready. O(log pending) + the commits.
    pub fn offer(&self, run: usize, cycle: usize, label: String, crit: Criteria, row: Row<V>, check: u8) {
        let mut g = self.lock();
        if run >= g.done.len() || g.ended[run] {
            return;
        }
        g.fps.insert((cycle, run), row.fp);
        g.pending.insert((cycle, run, usize::MAX), Pending { label, crit, row, check });
        g.mid_seq[run] = 0;
        g.done[run] = Some(cycle);
        self.commit(&mut g);
        drop(g);
        self.cv.notify_all();
    }

    /// TW5: run `run` offers a state reached inside cycle `cycle` (after an escape or an in-loop
    /// P4 round: "escapes offer their result to the table; the table keeps the bests"). Held with
    /// the run's cycle-end offer of `cycle` and committed before it, in the order offered, so the
    /// table stays a function of the seeds. Offers of a cycle the run never finishes (cut short)
    /// are dropped at the commit. O(log pending).
    pub fn offer_mid(&self, run: usize, cycle: usize, label: String, crit: Criteria, row: Row<V>) {
        let mut g = self.lock();
        if run >= g.done.len() || g.ended[run] || g.done[run].is_some_and(|c| c >= cycle) {
            return;
        }
        let seq = g.mid_seq[run];
        g.mid_seq[run] += 1;
        g.pending.insert((cycle, run, seq), Pending { label, crit, row, check: CHECK_NONE });
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

    /// Commit the held offers of every cycle all runs have passed and no run in its loop can still
    /// read an older table of (cycles below `horizon - lag`), in (cycle, run) order.
    fn commit(&self, g: &mut Inner<V>) {
        let h = Self::horizon(g);
        let lim = if h == usize::MAX { usize::MAX } else { h.saturating_sub(self.lag) };
        while let Some((c, _, _)) = g.pending.keys().next().copied() {
            if c >= lim {
                break;
            }
            g.st.apply(&g.pending, c, &self.obj);
            let keys: Vec<(usize, usize, usize)> = g.pending.range((c, 0, 0)..(c + 1, 0, 0)).map(|(k, _)| *k).collect();
            for k in keys {
                g.pending.remove(&k);
            }
        }
    }

    /// The headless stable test over the committed cycles (D6). O(window).
    fn stable_now(&self, g: &Inner<V>) -> bool {
        stable_upto(&g.st.gain, &g.st.restarts, g.st.gain.len(), self.window)
    }

    /// Run `run` at the start of cycle `cycle` reads T(`cycle` - 1 - lag): waits (calling
    /// `before_wait` once and `after_wait` after) until every other run still in its loop has
    /// finished cycle `cycle` - 1 - lag (saturating at the initial plans, cycle 0); `stop()` true
    /// ends the wait without a view (Cancel, the limit). Everything the view carries is taken at
    /// that cycle. O(rows + accepted) + the replayed offers + the wait.
    pub fn sync(&self, run: usize, cycle: usize, stop: &mut dyn FnMut() -> bool, before_wait: &dyn Fn(), after_wait: &dyn Fn()) -> Option<View<V>> {
        let need = cycle.saturating_sub(1).saturating_sub(self.lag);
        let mut g = self.lock();
        let ready = |g: &Inner<V>| (0..g.done.len()).all(|r| r == run || g.ended[r] || g.done[r].is_some_and(|c| c >= need));
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
        // Commit what is ready (never past `need`: this run holds the commits at its done - lag).
        self.commit(&mut g);
        // T(need): the committed state, plus the held offers of the cycles after it through `need`
        // (all final: every run in its loop passed `need`) replayed on a copy.
        let replay = g.st.committed.map_or(0, |c| c + 1);
        let cycles: Vec<usize> = {
            let mut v: Vec<usize> = g.pending.range((replay, 0, 0)..(need + 1, 0, 0)).map(|(k, _)| k.0).collect();
            v.dedup();
            v
        };
        let copied;
        let st = if cycles.is_empty() {
            &g.st
        } else {
            let mut c = g.st.clone();
            for &cy in &cycles {
                c.apply(&g.pending, cy, &self.obj);
            }
            copied = c;
            &copied
        };
        let rows = st.arch.items.iter().filter(|v| v.payload.cycle <= need).map(|v| (v.label.clone(), v.crit, v.payload.clone())).collect();
        let mine_accepted = st.accepted.iter().filter(|(r, c)| *r == run && *c <= need).map(|(_, c)| *c).collect();
        let fps_before = g.fps.range((need, 0)..(need + 1, 0)).filter(|((_, r), _)| *r != run).map(|((_, r), f)| (*r, *f)).collect();
        // `stable` over the committed cycles up to `need` (later commits are not this run's view).
        let stable = {
            let n = (need + 1).min(st.gain.len());
            stable_upto(&st.gain, &st.restarts, n, self.window)
        };
        Some(View { rows, mine_accepted, fps_before, stable })
    }

    /// The rows of the final table that hold one of the four bests, distinct, in the role order
    /// (overall, errors, shortest, vias), each with its roles. Call after every run ended. O(rows).
    pub fn final_bests(&self) -> Vec<(Vec<Role>, String, Criteria, Row<V>)> {
        let mut g = self.lock();
        self.commit(&mut g);
        let cs: Vec<&Criteria> = g.st.arch.items.iter().map(|v| &v.crit).collect();
        let b = bests(&cs, &self.obj);
        let mut out: Vec<(Vec<Role>, String, Criteria, Row<V>)> = Vec::new();
        let mut seen: Vec<usize> = Vec::new();
        for (role, i) in Role::ALL.iter().zip(b) {
            let Some(i) = i else { continue };
            match seen.iter().position(|&s| s == i) {
                Some(p) => out[p].0.push(*role),
                None => {
                    seen.push(i);
                    let v = &g.st.arch.items[i];
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
        let p = g.st.arch.pick(&self.obj);
        (g.st.arch.table(p), g.st.offers, self.stable_now(&g))
    }

    /// The restarts of all runs and how many of them led to a new row (an offer of that run taken
    /// into the table before its next restart), after every run ended. O(restarts x accepted).
    pub fn restart_stats(&self) -> (usize, usize) {
        let mut g = self.lock();
        self.commit(&mut g);
        let rs: Vec<(usize, usize)> = g.st.restart_at.iter().copied().collect();
        let new_row = (0..rs.len())
            .filter(|&i| {
                let (r, c) = rs[i];
                let next = rs.get(i + 1).filter(|x| x.0 == r).map_or(usize::MAX, |x| x.1);
                g.st.accepted.iter().any(|&(ar, ac)| ar == r && ac >= c && ac < next)
            })
            .count();
        (rs.len(), new_row)
    }

    /// Rows now (committed). O(1).
    pub fn len(&self) -> usize {
        self.lock().st.arch.items.len()
    }

    /// Whether the table has no committed row. O(1).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Accepted offers per run after a cycle in `[from, to)` (the restarts that led to a new row). O(accepted).
    pub fn accepted_in(&self, run: usize, from: usize, to: usize) -> usize {
        self.lock().st.accepted.iter().filter(|(r, c)| *r == run && *c >= from && *c < to).count()
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
                t.offer(r, cy, format!("r{r}c{cy}"), cr, row(r, cy), CHECK_NONE);
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
        t.offer(0, 0, "a0".into(), c(3, 1, 10.0), row(0, 0), CHECK_NONE);
        t.offer(0, 1, "a1".into(), c(1, 1, 10.0), row(0, 1), CHECK_NONE);
        assert_eq!(t.len(), 0, "run 1 has not offered its initial plan");
        t.offer(1, 0, "b0".into(), c(2, 1, 10.0), row(1, 0), CHECK_NONE);
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
        t.offer(0, 0, "a0".into(), c(3, 1, 10.0), row(0, 0), CHECK_NONE);
        t.offer(0, 1, "a1".into(), c(3, 0, 50.0), row(0, 1), CHECK_NONE);
        let t2 = t.clone();
        let h = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            t2.offer(1, 0, "b0".into(), c(3, 2, 5.0), row(1, 0), CHECK_NONE);
            t2.offer(1, 1, "b1".into(), c(0, 9, 900.0), row(1, 1), CHECK_NONE);
            t2.offer(1, 2, "b2".into(), c(0, 1, 9.0), row(1, 2), CHECK_NONE);
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
        t3.offer(0, 0, "a0".into(), c(1, 1, 1.0), row(0, 0), CHECK_NONE);
        assert!(t3.sync(0, 1, &mut || true, &|| {}, &|| {}).is_none());
    }

    /// D6: the free checks are counted since the four bests' last gain, so checks spaced further
    /// apart than the window (a run checks at most every `restart_min_cycles`) still make it stable.
    #[test]
    fn stable_counts_the_free_checks_since_the_last_gain() {
        let mut gain = vec![false; 30];
        let mut checks = vec![0usize; 30];
        gain[0] = true;
        checks[9] = 1;
        checks[18] = 1;
        assert!(!stable_upto(&gain, &checks, 18, 8), "one check since the gain");
        assert!(stable_upto(&gain, &checks, 19, 8), "two checks 9 cycles apart, window 8");
        gain[12] = true;
        assert!(!stable_upto(&gain, &checks, 19, 8), "a gain after the first check resets the count");
        assert!(!stable_upto(&gain, &checks, 30, 8), "one check since the gain at 12");
        let mut c2 = checks.clone();
        c2[25] = 1;
        assert!(stable_upto(&gain, &c2, 30, 8) && !stable_upto(&gain, &c2, 19, 8));
        // no gain for long, but fewer than two checks: not stable
        assert!(!stable_upto(&[true, false, false, false, false], &[0, 0, 0, 1, 0], 5, 2));
    }

    /// Stable (headless) needs a window without a gain of the four bests and two free checks since.
    #[test]
    fn stable_needs_no_gain_and_two_restarts() {
        let obj = Objective::from_options("length_vias", 25.0, "best");
        for restarts in [0usize, 1, 2] {
            let t: SharedTable<usize> = SharedTable::new(1, 8, obj, 3);
            t.offer(0, 0, "a0".into(), c(1, 1, 10.0), row(0, 0), CHECK_NONE);
            for cy in 1..=4 {
                t.offer(0, cy, format!("a{cy}"), c(2, 2, 20.0), row(0, cy), if cy > 4 - restarts { CHECK_RESTART } else { CHECK_NONE });
            }
            assert_eq!(t.summary().2, restarts >= 2, "restarts {restarts}");
        }
        // a gain inside the window: not stable
        let t: SharedTable<usize> = SharedTable::new(1, 8, obj, 3);
        t.offer(0, 0, "a0".into(), c(1, 1, 10.0), row(0, 0), CHECK_RESTART);
        for cy in 1..=4 {
            let cr = if cy == 3 { c(0, 1, 10.0) } else { c(2, 2, 20.0) };
            t.offer(0, cy, format!("a{cy}"), cr, row(0, cy), CHECK_RESTART);
        }
        assert!(!t.summary().2);
    }

    /// TW5: a state reached inside a cycle ([`SharedTable::offer_mid`]) commits with its run's
    /// cycle, before that cycle's own offer, whatever order the runs arrive in; one of a cycle its
    /// run never finished (it ended first) is dropped, not counted.
    #[test]
    fn mid_cycle_offers_commit_with_their_cycle_and_drop_when_unfinished() {
        let obj = Objective::from_options("length_vias", 25.0, "best");
        let r0 = |t: &SharedTable<usize>| {
            t.offer(0, 0, "r0c0".into(), c(2, 5, 100.0), row(0, 0), CHECK_NONE);
            t.offer_mid(0, 1, "r0c1 mid".into(), c(0, 1, 90.0), row(0, 1));
            t.offer(0, 1, "r0c1".into(), c(1, 4, 95.0), row(0, 1), CHECK_NONE);
            t.offer_mid(0, 2, "r0c2 mid unfinished".into(), c(0, 0, 10.0), row(0, 2));
            t.end(0);
        };
        let r1 = |t: &SharedTable<usize>| {
            t.offer(1, 0, "r1c0".into(), c(2, 5, 101.0), row(1, 0), CHECK_NONE);
            t.offer(1, 1, "r1c1".into(), c(1, 3, 96.0), row(1, 1), CHECK_NONE);
            t.end(1);
        };
        let run_table = |first0: bool| {
            let t: SharedTable<usize> = SharedTable::new(2, 8, obj, 4);
            if first0 {
                r0(&t);
                r1(&t);
            } else {
                r1(&t);
                r0(&t);
            }
            let (rows, n, _) = t.summary();
            (rows.iter().map(|x| x.label.clone()).collect::<Vec<_>>(), n)
        };
        let a = run_table(true);
        assert_eq!(a, run_table(false), "the arrival order of the runs does not matter");
        assert!(a.0.iter().any(|l| l == "r0c1 mid"), "the mid-cycle state (best on every criterion) is in the table: {:?}", a.0);
        assert!(!a.0.iter().any(|l| l.contains("unfinished")), "the unfinished cycle's state is dropped: {:?}", a.0);
        assert_eq!(a.1, 5, "four cycle offers and the finished cycle's mid offer committed");
    }

    type ViewKey = (Vec<(String, Criteria)>, Vec<usize>, Vec<(usize, u64)>, bool);

    fn key(v: &View<usize>) -> ViewKey {
        (v.rows.iter().map(|r| (r.0.clone(), r.1)).collect(), v.mine_accepted.clone(), v.fps_before.clone(), v.stable)
    }

    /// The toy offer stream of `runs` runs over cycles 0..=`last`: every cycle's end offer, plus a
    /// mid-cycle offer in some cycles; `check` marks a restart now and then (the stable series).
    fn toy_offers(runs: usize, last: usize) -> Vec<(usize, usize, bool, Criteria, u8)> {
        let mut v = Vec::new();
        for r in 0..runs {
            for cy in 0..=last {
                if cy > 0 && (r + cy) % 3 == 0 {
                    v.push((r, cy, true, c((5 * r + 3 * cy) % 4, (7 * r + cy) % 6, 50.0 + ((11 * r + 5 * cy) % 13) as f64), CHECK_NONE));
                }
                let check = if cy > 0 && (r + 2 * cy) % 4 == 0 { CHECK_RESTART } else { CHECK_NONE };
                v.push((r, cy, false, c((7 * r + 3 * cy) % 5, (11 * r + 5 * cy) % 9, 100.0 + ((13 * r + 7 * cy) % 17) as f64), check));
            }
        }
        v
    }

    fn put(t: &SharedTable<usize>, o: &(usize, usize, bool, Criteria, u8)) {
        let &(r, cy, mid, cr, check) = o;
        if mid {
            t.offer_mid(r, cy, format!("r{r}c{cy}m"), cr, Row { var: Arc::new(r * 1000 + cy + 500), run: r, cycle: cy, gen: cy, fp: (r * 1000 + cy + 500) as u64, compat: 0 });
        } else {
            t.offer(r, cy, format!("r{r}c{cy}"), cr, row(r, cy), check);
        }
    }

    /// T(need) as the lock-step table (lag 0) gives it: every run offered every cycle through `need`,
    /// read by `run` at cycle need + 1.
    fn reference(offers: &[(usize, usize, bool, Criteria, u8)], runs: usize, run: usize, need: usize) -> ViewKey {
        let obj = Objective::from_options("length_vias", 25.0, "best");
        let t: SharedTable<usize> = SharedTable::new(runs, 3, obj, 2);
        for o in offers.iter().filter(|o| o.1 <= need) {
            put(&t, o);
        }
        key(&t.sync(run, need + 1, &mut || panic!("every run passed need: no wait"), &|| {}, &|| {}).expect("view"))
    }

    /// sync-wait: with lag L a run at cycle c reads exactly T(c-1-L) (rows, its accepted offers, the
    /// others' fingerprints, stable), whatever the interleaving of the runs and wherever the
    /// commits stand; it reads without waiting when every other run passed c-1-L, and the final
    /// table is the same for every lag.
    #[test]
    fn a_lagged_view_is_the_table_of_its_cycle_on_any_interleaving() {
        use rand::seq::SliceRandom;
        use rand::{Rng, SeedableRng};
        let (runs, last) = (3usize, 9usize);
        let obj = Objective::from_options("length_vias", 25.0, "best");
        let offers = toy_offers(runs, last);
        let mut finals = Vec::new();
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(11);
        let mut read = 0;
        for lag in 0..=3usize {
            for _ in 0..12 {
                let t: SharedTable<usize> = SharedTable::new(runs, 3, obj, 2).with_lag(lag);
                // each run's own offers stay in order; the runs interleave at random, and a run that
                // just finished a cycle reads the table now and then, as at a sync point
                let mut per: Vec<Vec<(usize, usize, bool, Criteria, u8)>> = (0..runs).map(|r| offers.iter().copied().filter(|o| o.0 == r).collect()).collect();
                while per.iter().any(|p| !p.is_empty()) {
                    let mut live: Vec<usize> = (0..runs).filter(|&r| !per[r].is_empty()).collect();
                    live.shuffle(&mut rng);
                    let r = live[0];
                    let o = per[r].remove(0);
                    put(&t, &o);
                    if o.2 || o.1 == last || rng.gen_range(0..3) != 0 {
                        continue;
                    }
                    let cycle = o.1 + 1;
                    let need = cycle.saturating_sub(1).saturating_sub(lag);
                    let ready = per.iter().enumerate().all(|(s, p)| s == r || p.first().is_none_or(|x| x.1 > need));
                    let v = t.sync(r, cycle, &mut || true, &|| {}, &|| {});
                    assert_eq!(v.is_some(), ready, "lag {lag}: run {r} at cycle {cycle} reads without a wait exactly when every other run passed {need}");
                    if let Some(v) = v {
                        assert_eq!(key(&v), reference(&offers, runs, r, need), "lag {lag}: run {r} at cycle {cycle} reads T({need})");
                        read += 1;
                    }
                    let g = t.lock();
                    let min_done = (0..runs).filter(|&s| !g.ended[s]).map(|s| g.done[s].unwrap_or(0)).min().unwrap_or(0);
                    assert!(g.st.committed.is_none_or(|cm| cm + lag <= min_done), "lag {lag}: committed {:?} past min done {min_done} - lag", g.st.committed);
                }
                for r in 0..runs {
                    t.end(r);
                }
                let (rows, n, st) = t.summary();
                finals.push((rows.iter().map(|x| x.label.clone()).collect::<Vec<_>>(), n, st, t.restart_stats()));
            }
        }
        assert!(read > 100, "the test read the table often: {read}");
        assert!(finals.windows(2).all(|w| w[0] == w[1]), "the final table is the same for every lag and interleaving");
    }

    /// sync-wait: lag 2 lets a run 2 cycles ahead of another read without a wait; lag 0 waits.
    #[test]
    fn a_run_ahead_by_the_lag_does_not_wait() {
        let obj = Objective::from_options("length_vias", 25.0, "best");
        for (lag, waits) in [(0usize, true), (1, true), (2, false), (3, false)] {
            let t: SharedTable<usize> = SharedTable::new(2, 8, obj, 4).with_lag(lag);
            for cy in 0..=4 {
                t.offer(0, cy, format!("a{cy}"), c(1, 1, 10.0 + cy as f64), row(0, cy), CHECK_NONE);
            }
            for cy in 0..=2 {
                t.offer(1, cy, format!("b{cy}"), c(1, 1, 20.0 + cy as f64), row(1, cy), CHECK_NONE);
            }
            // run 0 at cycle 5 reads T(4 - lag); run 1 finished cycle 2
            let v = t.sync(0, 5, &mut || true, &|| {}, &|| {});
            assert_eq!(v.is_none(), waits, "lag {lag}");
            if let Some(v) = v {
                assert!(v.rows.iter().all(|r| r.2.cycle <= 4 - lag), "lag {lag}: nothing newer than T({}): {:?}", 4 - lag, v.rows.iter().map(|r| &r.0).collect::<Vec<_>>());
                assert_eq!(v.fps_before, vec![(1, (1000 + 4 - lag) as u64)], "lag {lag}: run 1's state at cycle {}", 4 - lag);
            }
            // the commits stop at min done - lag = 2 - lag
            assert_eq!(t.lock().st.committed, 2usize.checked_sub(lag), "lag {lag}");
        }
    }
}
