//! THROW-AWAY (branch nd-perf, measurement only): per-bucket call counts and seconds of the
//! drawing, enabled by `TOPO_NDPROF=1`; the never-drop pass prints one line per forced wire.
use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::Instant;

pub const N: usize = 24;
pub const NAMES: [&str; N] = [
    "place_vias", "force_via", "draw_wire", "strip", "exact0", "exact_relax", "local_path", "check",
    "octi0", "octi_relax", "repair", "pull_taut", "pull_pts", "realise", "via_reach", "lp_fail",
    "lp_nodes", "lp_edges", "exact_iters", "chord_sites", "via_cands", "neck", "diagnose", "runs",
];
pub const PLACE_VIAS: usize = 0;
pub const FORCE_VIA: usize = 1;
pub const DRAW_WIRE: usize = 2;
pub const STRIP: usize = 3;
pub const EXACT0: usize = 4;
pub const EXACT_RELAX: usize = 5;
pub const LOCAL_PATH: usize = 6;
pub const CHECK: usize = 7;
pub const OCTI0: usize = 8;
pub const OCTI_RELAX: usize = 9;
pub const REPAIR: usize = 10;
pub const PULL_TAUT: usize = 11;
pub const PULL_PTS: usize = 12;
pub const REALISE: usize = 13;
pub const VIA_REACH: usize = 14;
pub const LP_FAIL: usize = 15;
pub const LP_NODES: usize = 16;
pub const LP_EDGES: usize = 17;
pub const EXACT_ITERS: usize = 18;
pub const CHORD_SITES: usize = 19;
pub const VIA_CANDS: usize = 20;
pub const NECK: usize = 21;
pub const DIAGNOSE: usize = 22;
pub const RUNS: usize = 23;

thread_local! {
    static ACC: RefCell<[(u64, f64); N]> = const { RefCell::new([(0, 0.0); N]) };
}

pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TOPO_NDPROF").is_some())
}

pub struct T(usize, Instant);
impl Drop for T {
    fn drop(&mut self) {
        let s = self.1.elapsed().as_secs_f64();
        ACC.with(|a| {
            let mut a = a.borrow_mut();
            a[self.0].0 += 1;
            a[self.0].1 += s;
        });
    }
}

pub fn t(i: usize) -> Option<T> {
    on().then(|| T(i, Instant::now()))
}

pub fn add(i: usize, n: u64) {
    if on() {
        ACC.with(|a| a.borrow_mut()[i].0 += n);
    }
}

pub fn snap() -> [(u64, f64); N] {
    ACC.with(|a| *a.borrow())
}

pub fn diff(a: &[(u64, f64); N], b: &[(u64, f64); N]) -> String {
    let mut s = String::new();
    for i in 0..N {
        let (c, t) = (b[i].0 - a[i].0, b[i].1 - a[i].1);
        if c > 0 {
            s.push_str(&format!(" {}={}/{:.3}", NAMES[i], c, t));
        }
    }
    s
}
