//! Neck depth (tw4 T1, docs/dev/design/tw4-eval.md): a planned neck costs this weight per unit of
//! d² per bottleneck stretch, d = how deep the neck must go into the room between the nominal
//! width and the neck floor (`(need - room) / (w - floor)` on the stretch's tightest edge,
//! clamped to [0, 1]: 0 where the nominal widths just fit, 1 where only the floor fits;
//! `State::pass_d`). Graded, no cutoff, per stretch (not per mm). TopoR 6.3 manual p. 56: the
//! minimum width is used in bottlenecks "unless a different path is available", so the loop must
//! see a deep neck as dearer than a clean path a little longer. Part of the loop's measure
//! (`Predicted::with_neck`, the graded part, no count); prices no P2 event. Default 0 (opt-in
//! until the tw4 A/B; the violation weight, 1000, prices a floor neck like one violation).

use super::{Event, Factor};

pub struct NeckDepth;

impl Factor for NeckDepth {
    fn name(&self) -> &'static str {
        "neck_depth"
    }
    fn doc(&self) -> &'static str {
        "TopoR loop measure: a planned neck in a bottleneck, per stretch, graded by d² (d = 0 where the nominal widths just fit, 1 where only the minimum track width fits). 0 = off (default until measured)."
    }
    fn unit(&self) -> &'static str {
        "per necked stretch at the floor (d² = 1)"
    }
    fn default_weight(&self) -> f64 {
        0.0
    }
    fn amount(&self, _ev: &Event) -> f64 {
        0.0
    }
}
