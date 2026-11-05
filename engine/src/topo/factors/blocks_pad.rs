//! Blocks pad: a stretch occupies the last free entry sector of another net's pad that has no
//! copper yet. The entry sectors of a pad are the triangles around its centre vertex; one is
//! free when no stretch of another net runs through it. Taking the last one forces whoever
//! routes that pad later across the stretch (a via, or no way at all). A social factor (plan
//! "Open points" 2).

use super::{Event, Factor};

pub struct BlocksPad;

impl Factor for BlocksPad {
    fn name(&self) -> &'static str {
        "blocks_pad"
    }
    fn doc(&self) -> &'static str {
        "A stretch takes the last free entry sector (triangle around its centre) of another net's pad that has no wire yet."
    }
    fn unit(&self) -> &'static str {
        "per blocked pad"
    }
    fn default_weight(&self) -> f64 {
        30.0
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::BlocksPad { .. } => 1.0,
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Event, Registry, WireCtx};

    #[test]
    fn a_blocked_pad_is_priced() {
        let r = Registry::standard();
        assert_eq!(r.price(&Event::BlocksPad { wire: 0, tri: 1, pad: 2 }, &WireCtx::default()), 30.0);
    }
}
