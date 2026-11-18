//! par-draw phase A (docs/dev/design/par-draw.md): the drawing's order-free units (an ERBS
//! stretch, the per-layer work of the cut measurement) run on lanes, and their results come back
//! in unit index order, so the sequential fold after them sees exactly what it saw before: the
//! copper is the same bits for every lane count and every completion order.
//!
//! The lanes: `[drawing] draw_lanes` (`Options::draw_lanes`): `"auto"` (0) = one lane plus the
//! run's spare compute slots at that moment (the refinement's lane source,
//! [`super::refine_par::lanes`]), a number = that many lanes. The drawing sets the mode on its
//! thread ([`with_mode`]); a unit runs with no mode on its lane, so a unit never fans out again
//! (no nested lanes), and a caller outside the drawing (tests, the cut report) gets one lane.

use super::refine_par::lanes::{self, Mode};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::OnceLock;

thread_local! {
    static MODE: std::cell::Cell<Option<Mode>> = const { std::cell::Cell::new(None) };
    static REVERSE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Calls of [`map`] on this thread since the reverse hook was set (the hook acts on every 16th).
    static REV_N: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Units started on lanes (more than one lane) and on one lane, process-wide (tests).
pub static UNITS_LANED: AtomicUsize = AtomicUsize::new(0);
pub static UNITS_SERIAL: AtomicUsize = AtomicUsize::new(0);

/// Runs `f` with `mode` (`Some`: the drawing's lanes; `None`: one lane) and, for tests
/// (`Options::draw_test_reverse`), the reverse hook on this thread; the ones before come back
/// after `f`, also when it panics. O(1) + `f`.
pub fn with_mode<R>(mode: Option<Mode>, reverse: bool, f: impl FnOnce() -> R) -> R {
    let _r = set_mode(mode, reverse);
    f()
}

/// The drawing's mode on this thread until the guard drops (the one before comes back), for a
/// stage too long to pass as a closure ([`with_mode`]). O(1).
pub fn set_mode(mode: Option<Mode>, reverse: bool) -> ModeGuard {
    if reverse {
        REV_N.with(|c| c.set(0));
    }
    ModeGuard(MODE.with(|c| c.replace(mode)), REVERSE.with(|c| c.replace(reverse)))
}

/// Restores the mode [`set_mode`] replaced.
pub struct ModeGuard(Option<Mode>, bool);

impl Drop for ModeGuard {
    fn drop(&mut self) {
        MODE.with(|c| c.set(self.0));
        REVERSE.with(|c| c.set(self.1));
    }
}

/// Debug runs whose output must stay in unit order (`TOPO_ERBS_TIME`, `TOPO_ERBS_SLEEVE`): one lane.
fn serial_debug() -> bool {
    static S: OnceLock<bool> = OnceLock::new();
    *S.get_or_init(|| std::env::var_os("TOPO_ERBS_TIME").is_some() || std::env::var_os("TOPO_ERBS_SLEEVE").is_some())
}

/// Test hook (`Options::draw_test_reverse`, or `TOPO_PAR_REVERSE` set): in every 16th call on
/// this thread (the first one after the hook was set included; the octilinear DP calls [`map`]
/// once per node), unit `i` of the first 8 waits `8 - i` ms before it starts, so with several
/// lanes the first units finish last. Only
/// the timing changes, never a result.
fn reverse_on() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    REVERSE.with(|c| c.get()) || *E.get_or_init(|| std::env::var_os("TOPO_PAR_REVERSE").is_some())
}

/// `f(i, &items[i])` for every unit, the results in index order. With one lane (no drawing mode,
/// a debug print, one unit, no spare slot) the units run one after the other on this thread;
/// else on the lanes (this thread one of them), each lane taking the next unit not yet taken.
/// A panic in a unit comes back here. O(sum of the units / lanes).
pub fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(usize, &T) -> R + Sync) -> Vec<R> {
    match lease(items.len()) {
        Some(l) => map_leased(l, items, f),
        None => {
            UNITS_SERIAL.fetch_add(items.len(), Ordering::Relaxed);
            items.iter().enumerate().map(|(i, x)| with_mode(None, false, || f(i, x))).collect()
        }
    }
}

/// Lanes for `n` units, when there are more than one (the drawing's mode on this thread, no
/// debug print, more than one unit, a spare slot or a fixed lane count): the caller then runs
/// [`map_leased`], else its own serial loop (which may share state the lanes cannot). O(1).
pub fn lease(n: usize) -> Option<lanes::Lease> {
    let m = MODE.with(|c| c.get())?;
    if n <= 1 || serial_debug() {
        return None;
    }
    let l = lanes::lease_with(m);
    (l.lanes() > 1).then_some(l)
}

/// [`map`] on the lanes of `lease` (this thread one of them, each lane taking the next unit not
/// yet taken), the results in index order. A panic in a unit comes back here.
pub fn map_leased<T: Sync, R: Send>(lease: lanes::Lease, items: &[T], f: impl Fn(usize, &T) -> R + Sync) -> Vec<R> {
    let n = items.len();
    let rev = reverse_on() && REV_N.with(|c| {
        let k = c.get();
        c.set(k.wrapping_add(1));
        k % 16 == 0
    });
    let unit = |i: usize| {
        if rev && i < 8 {
            std::thread::sleep(std::time::Duration::from_millis(8 - i as u64));
        }
        // a unit runs without the drawing's mode: it never fans out again
        with_mode(None, false, || f(i, &items[i]))
    };
    let k = lease.lanes().min(n).max(1);
    UNITS_LANED.fetch_add(n, Ordering::Relaxed);
    meter::count(meter::MAP_LANED, 1);
    meter::count(meter::MAP_LANES, k as u64);
    meter::count(meter::MAP_UNITS, n as u64);
    let out: Vec<std::sync::Mutex<Option<R>>> = (0..n).map(|_| std::sync::Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let work = || loop {
        let i = next.fetch_add(1, Ordering::Relaxed);
        if i >= n || failed.load(Ordering::Relaxed) {
            break;
        }
        // a panicking unit stops the other lanes from taking more; the scope passes it on
        struct Flag<'a>(&'a AtomicBool);
        impl Drop for Flag<'_> {
            fn drop(&mut self) {
                if std::thread::panicking() {
                    self.0.store(true, Ordering::Relaxed);
                }
            }
        }
        let _g = Flag(&failed);
        let r = unit(i);
        if let Ok(mut c) = out[i].lock() {
            *c = Some(r);
        }
    };
    rayon::in_place_scope(|s| {
        for _ in 1..k {
            s.spawn(|_| work());
        }
        work();
    });
    drop(lease);
    out.into_iter().map(|c| c.into_inner().ok().flatten().expect("every unit ran")).collect()
}

/// The reverse hook's sampling (every 16th round on this thread after the hook was set). O(1).
fn reverse_round() -> bool {
    reverse_on()
        && REV_N.with(|c| {
            let k = c.get();
            c.set(k.wrapping_add(1));
            k % 16 == 0
        })
}

/// Persistent lanes for a sequence of rounds that run the same unit function (the octilinear
/// DP: one round per node, about 0.3 ms each, too short for a fork/join per round): the lanes
/// wait between rounds (a short spin, then a condvar), each round's units go to whichever lane
/// takes them next, and [`Crew::round`] returns the results in unit order, so the caller's fold
/// sees what a serial loop would. The lanes are the lease's (the drawing's lane source).
pub struct Crew<'a, J, U> {
    sh: &'a CrewShared<J, U>,
    unit: &'a (dyn Fn(&J, usize) -> U + Sync),
    k: usize,
}

struct CrewShared<J, U> {
    job: std::sync::RwLock<Option<J>>,
    out: std::sync::RwLock<Vec<std::sync::Mutex<Option<U>>>>,
    n: AtomicUsize,
    next: AtomicUsize,
    /// Units of the round finished (their result stored).
    fin: AtomicUsize,
    epoch: std::sync::atomic::AtomicU64,
    stop: AtomicBool,
    failed: AtomicBool,
    rev: AtomicBool,
    m: std::sync::Mutex<()>,
    cv: std::sync::Condvar,
}

/// Spins of a waiting lane before it sleeps on the condvar (~0.1-0.3 ms).
const CREW_SPIN: u32 = 4096;

fn crew_work<J, U>(sh: &CrewShared<J, U>, unit: &(dyn Fn(&J, usize) -> U + Sync)) {
    let job = sh.job.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    let out = sh.out.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(jb) = job.as_ref() else { return };
    let (n, rev) = (sh.n.load(Ordering::SeqCst), sh.rev.load(Ordering::SeqCst));
    loop {
        let i = sh.next.fetch_add(1, Ordering::SeqCst);
        if i >= n || sh.failed.load(Ordering::Relaxed) {
            break;
        }
        if rev && i < 8 {
            std::thread::sleep(std::time::Duration::from_millis(8 - i as u64));
        }
        // a panicking unit fails the round (its caller panics; the scope passes the panic on)
        struct Flag<'b>(&'b AtomicBool);
        impl Drop for Flag<'_> {
            fn drop(&mut self) {
                if std::thread::panicking() {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
        }
        let _g = Flag(&sh.failed);
        let u = with_mode(None, false, || unit(jb, i));
        let slot = i;
        if let Ok(mut c) = out[slot].lock() {
            *c = Some(u);
        }
        sh.fin.fetch_add(1, Ordering::SeqCst);
    }
}

fn crew_lane<J, U>(sh: &CrewShared<J, U>, unit: &(dyn Fn(&J, usize) -> U + Sync)) {
    let mut seen = 0u64;
    loop {
        let mut spins = 0u32;
        loop {
            let e = sh.epoch.load(Ordering::SeqCst);
            if e != seen {
                seen = e;
                break;
            }
            if spins < CREW_SPIN {
                std::hint::spin_loop();
                spins += 1;
                continue;
            }
            let g = sh.m.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let _g = sh.cv.wait_while(g, |_| sh.epoch.load(Ordering::SeqCst) == seen).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        if sh.stop.load(Ordering::SeqCst) {
            return;
        }
        // A lane joins the rounds it gets to (a lane that never starts, or starts late, because
        // the pool's threads are busy, holds nothing up: the round's caller takes every unit
        // nobody took).
        crew_work(sh, unit);
    }
}

/// Runs `body` with a crew of the lease's lanes (this thread one of them) running `unit`. A
/// panic in a unit or in `body` stops the crew and comes back here.
pub fn crew<J: Send + Sync, U: Send, R>(lease: lanes::Lease, unit: impl Fn(&J, usize) -> U + Sync, body: impl FnOnce(&mut Crew<'_, J, U>) -> R) -> R {
    let k = lease.lanes().max(1);
    let sh = CrewShared {
        job: std::sync::RwLock::new(None),
        out: std::sync::RwLock::new(Vec::new()),
        n: AtomicUsize::new(0),
        next: AtomicUsize::new(0),
        fin: AtomicUsize::new(0),
        epoch: std::sync::atomic::AtomicU64::new(0),
        stop: AtomicBool::new(false),
        failed: AtomicBool::new(false),
        rev: AtomicBool::new(false),
        m: std::sync::Mutex::new(()),
        cv: std::sync::Condvar::new(),
    };
    let unit_ref: &(dyn Fn(&J, usize) -> U + Sync) = &unit;
    let r = rayon::in_place_scope(|s| {
        for _ in 1..k {
            let sh = &sh;
            s.spawn(move |_| crew_lane(sh, unit_ref));
        }
        // the lanes leave when `body` ends, also when it panics
        struct Stop<'b, J, U>(&'b CrewShared<J, U>);
        impl<J, U> Drop for Stop<'_, J, U> {
            fn drop(&mut self) {
                self.0.stop.store(true, Ordering::SeqCst);
                self.0.epoch.fetch_add(1, Ordering::SeqCst);
                let _g = self.0.m.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                self.0.cv.notify_all();
            }
        }
        let _stop = Stop(&sh);
        let mut c = Crew { sh: &sh, unit: unit_ref, k };
        body(&mut c)
    });
    drop(lease);
    r
}

impl<J, U> Crew<'_, J, U> {
    /// The crew's lanes (this thread included).
    pub fn lanes(&self) -> usize {
        self.k
    }

    /// One round: units `0..n` of `job` on the lanes; the job back and the results in unit order.
    pub fn round(&mut self, job: J, n: usize) -> (J, Vec<U>) {
        let sh = self.sh;
        {
            // the round's state under the write locks: a lane reads it only after them
            let mut jw = sh.job.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut ow = sh.out.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            *jw = Some(job);
            *ow = (0..n).map(|_| std::sync::Mutex::new(None)).collect();
            sh.n.store(n, Ordering::SeqCst);
            sh.next.store(0, Ordering::SeqCst);
            sh.fin.store(0, Ordering::SeqCst);
            sh.rev.store(reverse_round(), Ordering::SeqCst);
        }
        UNITS_LANED.fetch_add(n, Ordering::Relaxed);
        meter::count(meter::MAP_LANED, 1);
        meter::count(meter::MAP_LANES, self.k as u64);
        meter::count(meter::MAP_UNITS, n as u64);
        sh.epoch.fetch_add(1, Ordering::SeqCst);
        {
            let _g = sh.m.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            sh.cv.notify_all();
        }
        crew_work(sh, self.unit);
        // the units the lanes took are running on live threads: wait for them only
        let mut spins = 0u32;
        while sh.fin.load(Ordering::SeqCst) < n && !sh.failed.load(Ordering::SeqCst) {
            if spins < CREW_SPIN {
                std::hint::spin_loop();
                spins += 1;
            } else {
                std::thread::yield_now();
            }
        }
        if sh.failed.load(Ordering::SeqCst) {
            panic!("a lane of the drawing's crew panicked");
        }
        let job = sh.job.write().unwrap_or_else(std::sync::PoisonError::into_inner).take().expect("the round's job");
        let out = std::mem::take(&mut *sh.out.write().unwrap_or_else(std::sync::PoisonError::into_inner));
        (job, out.into_iter().map(|c| c.into_inner().ok().flatten().expect("every unit ran")).collect())
    }
}

/// Where the drawing's parallel parts spend their time (`TOPO_TIMES`: one line at the end of
/// every drawing, then reset; process-wide, so concurrent drawings add up). O(1) per event.
pub mod meter {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    pub const OCTI: usize = 0;
    pub const OCTI_LANED: usize = 1;
    pub const ERBS_MEASURE: usize = 2;
    pub const ERBS_STRETCHES: usize = 3;
    pub const MAP_LANED: usize = 4;
    pub const MAP_LANES: usize = 5;
    pub const MAP_UNITS: usize = 6;
    pub const OCTI_CALLS: usize = 7;
    pub static M: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
    pub fn on() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("TOPO_TIMES").is_some())
    }
    pub fn time(i: usize, t: std::time::Instant) {
        if on() {
            M[i].fetch_add(t.elapsed().as_nanos() as u64, Relaxed);
        }
    }
    pub fn count(i: usize, n: u64) {
        if on() {
            M[i].fetch_add(n, Relaxed);
        }
    }
    /// The line, and the meters back to 0.
    pub fn report() -> String {
        let v: Vec<u64> = M.iter().map(|a| a.swap(0, Relaxed)).collect();
        let s = |x: u64| x as f64 * 1e-9;
        format!(
            "par-draw: octilinear {:.2} s in {} calls (nodes on lanes {:.2} s), ERBS measure {:.2} s, stretches {:.2} s; laned map calls {} ({} units, mean lanes {:.1})",
            s(v[OCTI]), v[OCTI_CALLS], s(v[OCTI_LANED]), s(v[ERBS_MEASURE]), s(v[ERBS_STRETCHES]), v[MAP_LANED], v[MAP_UNITS], v[MAP_LANES] as f64 / (v[MAP_LANED].max(1) as f64)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The results come back in index order for every lane count, with the reverse hook (the
    /// first units finish last): the completion order never reaches the caller.
    #[test]
    fn map_keeps_index_order_for_every_lane_count() {
        let items: Vec<u64> = (0..40).collect();
        let want: Vec<u64> = items.iter().map(|x| x * x + 7).collect();
        for k in [1usize, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(k).build().expect("pool");
            let got = pool.install(|| with_mode(Some(Mode::Fixed(k)), true, || map(&items, |i, &x| {
                assert_eq!(i as u64, x);
                x * x + 7
            })));
            assert_eq!(got, want, "lanes {k}");
        }
    }

    /// Without the drawing's mode a call has one lane, and a unit does not fan out again.
    #[test]
    fn no_mode_and_units_run_serial() {
        let items: Vec<usize> = (0..6).collect();
        let tid = std::thread::current().id();
        let same = map(&items, |_, _| std::thread::current().id() == tid);
        assert!(same.iter().all(|&s| s));
        let nested = with_mode(Some(Mode::Fixed(4)), false, || map(&items, |_, _| MODE.with(|c| c.get()).is_none()));
        assert!(nested.iter().all(|&s| s));
    }

    /// A crew's rounds return their results in unit order for every lane count, with the reverse
    /// hook, and a round sees the job it was given.
    #[test]
    fn crew_rounds_keep_unit_order() {
        for k in [1usize, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(k).build().expect("pool");
            pool.install(|| {
                with_mode(Some(Mode::Fixed(k)), true, || {
                    let unit = |j: &Vec<u64>, i: usize| j[i] * 3 + i as u64;
                    let got = match lease(2) {
                        Some(l) => crew(l, unit, |c| (0..40u64).map(|r| c.round((0..(r + 5)).map(|x| x + r).collect::<Vec<u64>>(), (r + 5) as usize).1).collect::<Vec<_>>()),
                        None => (0..40u64).map(|r| { let j: Vec<u64> = (0..(r + 5)).map(|x| x + r).collect(); (0..j.len()).map(|i| unit(&j, i)).collect() }).collect(),
                    };
                    for (r, g) in got.iter().enumerate() {
                        let want: Vec<u64> = (0..(r as u64 + 5)).map(|x| (x + r as u64) * 3 + x).collect();
                        assert_eq!(g, &want, "lanes {k} round {r}");
                    }
                })
            });
        }
    }

    /// More crew lanes than pool threads (a fixed `draw_lanes` above the threads, or a pool busy
    /// elsewhere): the lanes that never start hold no round up, the results stay in unit order.
    #[test]
    fn a_crew_with_more_lanes_than_threads_finishes() {
        for (threads, lanes) in [(1usize, 8usize), (2, 5), (3, 8)] {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
            pool.install(|| {
                with_mode(Some(Mode::Fixed(lanes)), true, || {
                    let unit = |j: &Vec<u64>, i: usize| j[i] * 3 + i as u64;
                    let l = lease(2).expect("lanes");
                    let got = crew(l, unit, |c| (0..60u64).map(|r| c.round((0..(r % 7 + 1)).map(|x| x + r).collect::<Vec<u64>>(), (r % 7 + 1) as usize).1).collect::<Vec<_>>());
                    for (r, g) in got.iter().enumerate() {
                        let r = r as u64;
                        let want: Vec<u64> = (0..(r % 7 + 1)).map(|x| (x + r) * 3 + x).collect();
                        assert_eq!(g, &want, "threads {threads} lanes {lanes} round {r}");
                    }
                })
            });
        }
    }

    /// A panic in a crew's unit reaches the caller (the lanes stop).
    #[test]
    fn a_crew_unit_panic_reaches_the_caller() {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().expect("pool");
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pool.install(|| with_mode(Some(Mode::Fixed(4)), false, || crew(lease(2).expect("lanes"), |_: &(), i: usize| if i == 3 { panic!("unit 3") } else { i }, |c| c.round((), 8).1)))
        }));
        assert!(r.is_err());
    }

    /// A panic in a unit reaches the caller.
    #[test]
    fn a_unit_panic_reaches_the_caller() {
        let items: Vec<usize> = (0..8).collect();
        let r = std::panic::catch_unwind(|| with_mode(Some(Mode::Fixed(4)), false, || map(&items, |i, _| if i == 5 { panic!("unit 5") } else { i })));
        assert!(r.is_err());
    }
}
