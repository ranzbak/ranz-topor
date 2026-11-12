//! Shrinkage (shopping list 2.4): with `[capacity] model = "cuts"`, a per-layer cut whose wires
//! fit only when they neck below their netclass width (down to the neck floor: Board Setup
//! minimum track width or the net's custom rule) costs this weight per track pitch (the nominal
//! width plus the clearance) of width reduction it needs, graded from 0 where the nominal widths just fit up to
//! all the necking there is at the floors; beyond the floors the rest is a violation
//! (`topo::cutmodel`, three-level cut price). TopoR 6.3 manual p. 56 (the minimum width "will be
//! used in bottlenecks unless a different path is available") and pp. 72-73 (shrinkages, "width
//! reductions in nominally-wide wires", a criterion of their own next to violations, vias and
//! length). Prices no P2 event: the cut model reads the weight (`CutOpts::shrink_mm`). Unless set,
//! it is the `necking` weight ([`Factor::default_follows`]).

use super::{Event, Factor};

pub struct Shrinkage;

impl Factor for Shrinkage {
    fn name(&self) -> &'static str {
        "shrinkage"
    }
    fn doc(&self) -> &'static str {
        "Cut model (capacity model = cuts): a per-layer cut whose traces fit only necked below their netclass width (not below the minimum track width), graded by the width reduction it needs; beyond the minimum widths the rest is a violation. Unset: the necking weight."
    }
    fn unit(&self) -> &'static str {
        "per track pitch of width reduction in a cut"
    }
    fn default_weight(&self) -> f64 {
        30.0
    }
    fn default_follows(&self) -> Option<&'static str> {
        Some("necking")
    }
    fn amount(&self, _ev: &Event) -> f64 {
        0.0
    }
}
