//! Via site social (TW6 part B, the search term; docs/dev/design/tw6-via-sites.md "B"): a new
//! trace that crosses another net's stretch where neither wire has room for a via. A crossing of
//! two nets on a two-layer board needs a layer change of one of the two between neighbouring
//! crossings (Polubasov 2001 p. 4); where the crossing wire's own via has no site near the
//! crossing and the crossed wire's via interval there has none either, the crossing predicts a
//! via without a site. Polubasov 2010 «Перекладка проводников», fig. 4: "a penalty for crossing
//! a stretch of a wire that carries a via" (wire 1 takes a longer path so wire 2 passes without
//! vias). The amount is the smaller of the two graded site amounts (`1 + deficit / (d + s)`,
//! [`crate::topo::pack::Packer::site_amount`]): 0 where either has a site, so the path round (0
//! vias, longer) competes with the crossing in S itself. Graded, no threshold; opt-in (weight 0)
//! until the equal-time A/B.

use super::{Event, Factor, Report};

pub struct ViaSiteSocial;

impl Factor for ViaSiteSocial {
    fn name(&self) -> &'static str {
        "via_site_social"
    }
    fn doc(&self) -> &'static str {
        "A crossing of another net's stretch where neither wire has room for a via (the crossing wire's own via near the crossing, the crossed wire's via interval through it): weight x the smaller of the two site amounts, each 1 + deficit / (via diameter + clearance) without a site and 0 with one. Opt-in (0)."
    }
    fn unit(&self) -> &'static str {
        "per crossing where neither wire has a via site, per unit of site amount"
    }
    fn default_weight(&self) -> f64 {
        0.0
    }
    fn default_report(&self) -> Report {
        Report::Off
    }
    fn amount(&self, ev: &Event) -> f64 {
        match *ev {
            Event::ViaSite { amount, .. } => amount.max(0.0),
            _ => 0.0,
        }
    }
    fn describe(&self, ev: &Event) -> String {
        match *ev {
            Event::ViaSite { a, b, tri, amount } => format!("wires {a} and {b} cross in triangle {tri} where neither has a via site (amount {amount:.2})"),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Event, FactorConfig, Registry, WireCtx};

    #[test]
    fn off_by_default_and_linear_in_the_amount() {
        let ev = |amount: f64| Event::ViaSite { a: 0, b: 1, tri: 0, amount };
        assert_eq!(Registry::standard().price(&ev(1.5), &WireCtx::default()), 0.0);
        let mut cfg = FactorConfig::default();
        cfg.weight.insert("via_site_social".into(), 10.0);
        let r = Registry::from_config(super::super::standard_factors(), &cfg).0;
        assert_eq!(r.price(&ev(0.0), &WireCtx::default()), 0.0);
        assert!((r.price(&ev(1.0), &WireCtx::default()) - 10.0).abs() < 1e-9);
        assert!((r.price(&ev(1.5), &WireCtx::default()) - 15.0).abs() < 1e-9);
    }
}
