//! TopoR-style topological core (plan "Pipeline of the new core"). Stage G0, the flat-plane
//! gate: the projection and topology state ([`state`], [`board`]), initial routing in one plane
//! with crossings allowed ([`init`], P2-lite) and the two-layer assignment of Polubasov 2001
//! ([`layers`], P3), measured by [`gate`]. Not wired into `route()`.

pub mod board;
pub mod capacity;
pub mod archive;
pub mod core;
pub mod descent;
pub mod cuts;
pub mod demand;
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
pub mod refine;
pub mod metrize;
pub mod moves;
pub mod pack;
pub mod state;
pub mod stitch;
pub mod stats;
pub mod tree;
pub mod witness;
