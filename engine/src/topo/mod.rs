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
pub mod archive;
pub mod core;
pub mod descent;
pub mod cuts;
pub mod cutflow;
pub mod cutmodel;
pub mod demand;
pub mod diffpair;
pub mod draw;
pub mod dwitness;
pub mod exact;
pub mod factors;
pub mod gate;
pub mod init;
pub mod layers;
pub mod learn;
pub mod planes;
pub mod reduce;
pub mod schedule;
pub mod sheet;
pub mod refine;
pub mod metrize;
pub mod moves;
pub mod octi;
pub mod optimise;
pub mod pack;
pub mod state;
pub mod stitch;
pub mod tidy;
pub mod stats;
pub mod tree;
pub mod viaopt;
pub mod xrules;
pub mod witness;
