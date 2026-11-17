//! `TOPO_REFINE_TRACE=<dir>` (debug, par-refine): one line per layer refinement (P3, the label
//! moves of [`super::refine::refine`]) with a hash of the labels before and after and its counts,
//! appended to `<dir>/refine-<pid>.txt` in call order. Two engines whose refinements keep the
//! same moves write the same lines; on a run the time limit stops, the lines before the stop are
//! compared (the soft limit never cuts a refinement). Off (one env lookup, cached) otherwise.

use super::refine::{LEv, RefineStats};
use std::hash::{Hash, Hasher};
use std::io::Write;

fn dir() -> Option<&'static str> {
    static D: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    D.get_or_init(|| std::env::var("TOPO_REFINE_TRACE").ok().filter(|d| !d.is_empty())).as_deref()
}

/// Whether the trace is on. O(1).
pub fn on() -> bool {
    dir().is_some()
}

/// A hash of the labels, bit for bit. O(events).
pub fn hash(evl: &[Vec<LEv>]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (w, l) in evl.iter().enumerate() {
        (w, l.len()).hash(&mut h);
        for &(k, s, lay) in l {
            (k, s.to_bits(), lay).hash(&mut h);
        }
    }
    h.finish()
}

/// Appends the line of one refinement: `h_in h_out moves before after`. O(events).
pub fn note(site: &str, h_in: u64, evl: &[Vec<LEv>], rs: &RefineStats) {
    let Some(d) = dir() else { return };
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let line = format!("{site} {h_in:016x} {:016x} {} {} {}\n", hash(evl), rs.moves, rs.before, rs.after);
    let _g = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let _ = std::fs::create_dir_all(d);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{d}/refine-{}.txt", std::process::id())) {
        let _ = f.write_all(line.as_bytes());
    }
}
