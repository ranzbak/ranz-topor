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

/// Test hook (`Options::draw_test_reverse`, or `TOPO_PAR_REVERSE` set): unit `i` of the first 16
/// waits `16 - i` ms before it starts, so with several lanes the first units finish last. Only
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
    let n = items.len();
    let rev = reverse_on();
    let unit = |i: usize| {
        if rev && i < 16 {
            std::thread::sleep(std::time::Duration::from_millis(16 - i as u64));
        }
        // a unit runs without the drawing's mode: it never fans out again
        with_mode(None, false, || f(i, &items[i]))
    };
    let mode = MODE.with(|c| c.get());
    let lease = match mode {
        Some(m) if n > 1 && !serial_debug() => Some(lanes::lease_with(m)),
        _ => None,
    };
    let k = lease.as_ref().map_or(1, |l| l.lanes()).min(n);
    if k <= 1 {
        UNITS_SERIAL.fetch_add(n, Ordering::Relaxed);
        return (0..n).map(unit).collect();
    }
    UNITS_LANED.fetch_add(n, Ordering::Relaxed);
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

    /// A panic in a unit reaches the caller.
    #[test]
    fn a_unit_panic_reaches_the_caller() {
        let items: Vec<usize> = (0..8).collect();
        let r = std::panic::catch_unwind(|| with_mode(Some(Mode::Fixed(4)), false, || map(&items, |i, _| if i == 5 { panic!("unit 5") } else { i })));
        assert!(r.is_err());
    }
}
