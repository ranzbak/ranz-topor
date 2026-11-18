//! TopoR-style topological core: the router behind `route()` ([`core::route_topo`]).
//!
//! Fixed board: the projection ([`board`], [`cdt`](crate::cdt) on the fixed copper) and the plane
//! stubs ([`planes`]). Topology: initial routing in one plane with crossings allowed and net trees
//! ([`init`], [`state`], [`tree`]), layer assignment after Polubasov 2001 with the capacity
//! refinement ([`layers`], [`refine`], [`pack`]), crossing rules and negotiated rip-up and reroute
//! ([`xrules`], [`moves`], [`descent`]), priced by the social factors ([`factors`]). Drawing:
//! [`draw`] (with [`exact`], [`octi`]), then the steps after the drawing ([`reduce`], fuse,
//! [`stitch`], [`tidy`]). Iteration: whole-route cycles with a fluctuation schedule ([`schedule`])
//! and an archive of variants ([`archive`]). Measurement only: [`cutflow`] / [`sheet`] (the
//! per-layer cut test), [`gate`], [`witness`], [`dwitness`], [`learn`], [`stats`].

pub mod board;
pub mod capacity;
pub mod compass;
pub mod archive;
pub mod core;
pub mod descent;
pub mod cuts;
pub mod cutflow;
pub mod cutmodel;
pub mod demand;
pub mod diffpair;
pub mod draw;
pub mod erbs;
pub mod dwitness;
pub mod exact;
pub mod factors;
pub mod funnel;
pub mod gate;
pub mod init;
pub mod layers;
pub mod learn;
pub mod planes;
pub mod reduce;
pub mod schedule;
pub mod sheet;
pub mod radii;
pub mod refine;
pub mod metrize;
pub mod moves;
pub mod octi;
pub mod optimise;
pub mod pack;
pub mod padend;
pub mod state;
pub mod stitch;
pub mod tidy;
pub mod stats;
pub mod tree;
pub mod viaopt;
pub mod xrules;
pub mod witness;

/// Debug switch for the 6b/M6 measurement (branch `m6-fire`; never a setting): the
/// environment variable `TOPO_M6_OFF` holds a comma list of drawing-side stand-ins to switch off,
/// so a run shows what the method does without them. Keys: `force_via` (never drop: a via without
/// a legal site goes to its least violating site), `force_wire` (never drop: a wire with no legal
/// drawing is drawn on its least violating geometry), `force_join` (never drop: a branch meeting
/// its trunk on another layer is joined by a via / track), `ladder` (S2a funnel + repair attempts 3..5 of the
/// drawing ladder); `all` = every key. Unset or empty: nothing changes. Read once per process.
pub fn m6_off(key: &str) -> bool {
    static KEYS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let keys = KEYS.get_or_init(|| std::env::var("TOPO_M6_OFF").map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default());
    keys.iter().any(|k| k == key || k == "all")
}
