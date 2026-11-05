//! Reroute counter (plan P4 "reroute counter per wire"): a connection that was taken out and
//! routed again many times pays more for moving once more, so the negotiated rip-up and the P4
//! moves stop oscillating. Priced per earlier reroute of the connection.

use super::{Event, Factor};

pub struct Reroute;

impl Factor for Reroute {
    fn name(&self) -> &'static str {
        "reroute"
    }
    fn doc(&self) -> &'static str {
        "Moving a connection that was already taken out and routed again (P4 reroute counter): per earlier reroute; such a connection is also ripped last."
    }
    fn unit(&self) -> &'static str {
        "per earlier reroute"
    }
    fn default_weight(&self) -> f64 {
        5.0
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::Rerouted { count, .. } => f64::from(count),
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Event, Registry, WireCtx};

    #[test]
    fn each_earlier_reroute_costs_more() {
        let r = Registry::standard();
        let p = |n: u32| r.price(&Event::Rerouted { wire: 0, count: n }, &WireCtx::default());
        assert_eq!(p(0), 0.0);
        assert_eq!(p(3), 15.0);
    }
}
