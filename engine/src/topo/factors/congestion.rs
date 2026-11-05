//! Congestion history (PathFinder, McMurchie and Ebeling 1995): a cut that was over-full in an
//! earlier round of the negotiated rip-up-and-reroute (P4) or of the drawing's topology feedback
//! keeps a memory of it. A pass through that edge costs `weight x history`, where the history is
//! the number of rounds the edge was over-full in. The present overflow of the round in progress
//! is the `overflow` factor; this one is what makes wires leave a contested cut for good.

use super::{Event, Factor};

pub struct Congestion;

impl Factor for Congestion {
    fn name(&self) -> &'static str {
        "congestion"
    }
    fn doc(&self) -> &'static str {
        "A pass through a cut that was over-full in earlier rounds of the negotiated rip-up-and-reroute (P4); grows with the number of such rounds."
    }
    fn unit(&self) -> &'static str {
        "per pass per earlier over-full round"
    }
    fn default_weight(&self) -> f64 {
        10.0
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::Congestion { history, .. } => history,
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Event, Registry, WireCtx};

    #[test]
    fn history_scales_the_price() {
        let r = Registry::standard();
        let w = WireCtx::default();
        let p = |h: f64| r.price(&Event::Congestion { wire: 0, edge: 3, history: h }, &w);
        assert_eq!(p(0.0), 0.0);
        assert_eq!(p(3.0), 30.0);
    }
}
