//! Social-factor plug-in structure (plan "Social factors" and item G): the price of what a route
//! does, as small independent factors summed per move.
//!
//! * An [`Event`] is something a candidate move *does* (a length of copper, a via, a crossing, a
//!   necked stretch, a violated ignored rule), with typed fields. Events are derived from a
//!   [`Move`] and the read-only [`State`] by [`events`].
//! * A [`Factor`] prices events: [`Factor::amount`] says how many of its units an event is (0 for
//!   events it does not price); the cost of a factor is `weight x amount`, in mm of copper. One
//!   file per factor in this directory; [`Registry::standard`] lists them.
//! * A [`Registry`] holds the factors with their weight, scope and report level, resolved from
//!   `topor.toml [factors]` through the engine options ([`FactorConfig`]): `weight` (mm per unit,
//!   default in the factor file), `scope` (`"all"` or `"tier:N"`), `report` (`"off"`, `"info"`,
//!   `"warning"`: events of the factor become report lines).
//! * [`Registry::cost`] is the per-move evaluation `cost(state, move) -> f64`; it only reads the
//!   state (moves write, factors read). P2 takes its search costs from [`Registry::price`].
//!
//! Hard rules are not factors: fab constraints read from KiCad never bend. An ignored KiCad
//! check (severity `ignore`) is not hard either: it becomes the `ignored_rule` factor plus a
//! report warning where the rule is violated; the rule value is read from KiCad, never from here.
//!
//! Complexity: pricing an event is O(#factors); [`events`] of an insert is O(path x traversals of
//! the triangles it crosses).

mod blocks_pad;
mod congestion;
mod crossing;
mod ignored_rule;
mod length;
mod necking;
mod overflow;
mod plane_signal;
mod reroute;
mod squeeze;
mod via;
mod via_squeeze;

pub use squeeze::squeeze_amount;

use super::demand::Item;
use super::state::{interleave, EdgeId, ObsId, State, TriId, Wire, WireId};
use crate::geom::Vec2;
use std::collections::BTreeMap;

const MM: f64 = 1e6;

/// Something a move does. Fields are typed; the unit of each is in its doc.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Copper of `wire`: `mm` of centre line, `scale` = (w+s)/(w0+s0) of the wire.
    Length { wire: WireId, mm: f64, scale: f64 },
    /// `count` vias of `wire` (through vias); `at` when the position is known.
    Via { wire: WireId, count: u32, at: Option<Vec2> },
    /// A pass through a gap or triangle that fixes the layer (over another net's SMD pad, or a
    /// gap too narrow on some layer): up to one via.
    LayerForced { wire: WireId, edge: EdgeId },
    /// Two wires cross inside `tri`.
    Crossing { a: WireId, b: WireId, tri: TriId, same_net: bool },
    /// `mm` of `wire` on a plane layer (signal on a plane).
    PlaneLength { wire: WireId, layer: usize, mm: f64 },
    /// `wire` cuts the plane of `layer` (a slot in the reference plane).
    PlaneCut { wire: WireId, layer: usize },
    /// A necked stretch: `wire` narrowed from `from_nm` to `to_nm` over `mm`.
    Necking { wire: WireId, from_nm: f64, to_nm: f64, mm: f64 },
    /// A pass through an edge whose room is exceeded (a violation the search prices).
    Overflow { wire: WireId, edge: EdgeId },
    /// A pass of `wire` through `edge` between two other stretches that leaves `slack_nm` of the
    /// cut free; `pitch_nm` = the wire's width plus clearance.
    Squeeze { wire: WireId, edge: EdgeId, slack_nm: f64, pitch_nm: f64 },
    /// A via of `wire` at `at` (diameter `via_nm`) next to a passage-layer left with `slack_nm`
    /// of room (the packer's least `rmax - lmin` there).
    ViaSqueeze { wire: WireId, at: Vec2, slack_nm: f64, via_nm: f64 },
    /// `wire` runs through `tri`, the last free entry sector of the pad obstacle `pad` of another
    /// net, which has no wire yet.
    BlocksPad { wire: WireId, tri: TriId, pad: ObsId },
    /// A pass of `wire` through `edge`, which was over-full in `history` earlier rounds.
    Congestion { wire: WireId, edge: EdgeId, history: f64 },
    /// A KiCad check set to `ignore` is violated here: `rule` is its KiCad name
    /// (`copper_edge_clearance`, ...), `at` the place, `amount_nm` how far below the rule.
    IgnoredRuleViolated { rule: String, at: Vec2, amount_nm: f64 },
    /// `wire`'s connection moves once more after `count` earlier reroutes (P4 reroute counter).
    Rerouted { wire: WireId, count: u32 },
}

/// How a factor's events appear in the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Report {
    Off,
    Info,
    Warning,
}

impl Report {
    pub fn parse(s: &str) -> Option<Report> {
        match s {
            "off" => Some(Report::Off),
            "info" => Some(Report::Info),
            "warning" => Some(Report::Warning),
            _ => None,
        }
    }
}

/// Which wires a factor applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    /// Wires of this tier (1 = routed first).
    Tier(u8),
}

impl Scope {
    /// `"all"` or `"tier:N"`.
    pub fn parse(s: &str) -> Option<Scope> {
        if s == "all" {
            return Some(Scope::All);
        }
        s.strip_prefix("tier:").and_then(|t| t.parse().ok()).map(Scope::Tier)
    }

    pub fn covers(self, w: &WireCtx) -> bool {
        match self {
            Scope::All => true,
            Scope::Tier(t) => w.tier == t,
        }
    }
}

/// The wire an event belongs to, as far as scope and weights care.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WireCtx {
    pub net: i32,
    pub tier: u8,
}

impl Default for WireCtx {
    fn default() -> Self {
        WireCtx { net: 0, tier: 3 }
    }
}

/// A factor: prices events. Factors are trusted code (docs/dev/factors.md): no sandbox. They are
/// deterministic (no HashMap iteration, seeded RNG only) and never mutate the state.
pub trait Factor: Send + Sync {
    /// Name in `topor.toml` (`[factors]` tables) and in report lines.
    fn name(&self) -> &'static str;
    /// One sentence for the generated reference.
    fn doc(&self) -> &'static str;
    /// What one unit of [`Factor::amount`] is, for the reference ("per mm", "per via").
    fn unit(&self) -> &'static str;
    /// Default weight: mm of copper per unit.
    fn default_weight(&self) -> f64;
    /// Default report level of the factor's events.
    fn default_report(&self) -> Report {
        Report::Off
    }
    /// Units of this factor in `ev` (0 for events it does not price).
    fn amount(&self, ev: &Event) -> f64;
    /// One report line for an event this factor prices.
    fn describe(&self, ev: &Event) -> String {
        format!("{}: {:?}", self.name(), ev)
    }
}

/// Settings of `topor.toml [factors]` as the engine options carry them.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FactorConfig {
    /// Factor name -> weight (mm per unit).
    pub weight: BTreeMap<String, f64>,
    /// Factor name -> `"all"` | `"tier:N"`.
    pub scope: BTreeMap<String, String>,
    /// Factor name -> `"off"` | `"info"` | `"warning"`.
    pub report: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug)]
struct Resolved {
    weight: f64,
    scope: Scope,
    report: Report,
}

/// One line for the run report.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub level: Report,
    pub factor: &'static str,
    pub text: String,
}

/// The factors with their resolved weight, scope and report level.
#[derive(Clone)]
pub struct Registry {
    factors: Vec<std::sync::Arc<dyn Factor>>,
    res: Vec<Resolved>,
}

/// The standard factor set, in report order.
pub fn standard_factors() -> Vec<Box<dyn Factor>> {
    vec![
        Box::new(length::Length),
        Box::new(via::Via),
        Box::new(crossing::Crossing),
        Box::new(crossing::SameNetCrossing),
        Box::new(plane_signal::PlaneSignal),
        Box::new(plane_signal::PlaneCut),
        Box::new(necking::Necking),
        Box::new(overflow::Overflow),
        Box::new(congestion::Congestion),
        Box::new(squeeze::Squeeze),
        Box::new(via_squeeze::ViaSqueeze),
        Box::new(blocks_pad::BlocksPad),
        Box::new(ignored_rule::IgnoredRule),
        Box::new(reroute::Reroute),
    ]
}

impl Registry {
    /// The standard set with its default weights, scopes (all) and report levels.
    pub fn standard() -> Registry {
        Registry::from_config(standard_factors(), &FactorConfig::default()).0
    }

    /// `factors` with the weight / scope / report of `cfg`. The second value lists the problems
    /// (unknown factor name, bad value): the setting is ignored and the default used.
    pub fn from_config(factors: Vec<Box<dyn Factor>>, cfg: &FactorConfig) -> (Registry, Vec<String>) {
        let mut problems = Vec::new();
        let known = |n: &str| factors.iter().any(|f| f.name() == n);
        for (kind, keys) in [("weight", cfg.weight.keys().collect::<Vec<_>>()), ("scope", cfg.scope.keys().collect()), ("report", cfg.report.keys().collect())] {
            for k in keys {
                if !known(k) {
                    problems.push(format!("[factors] {kind}: unknown factor {k:?}"));
                }
            }
        }
        let res = factors
            .iter()
            .map(|f| {
                let name = f.name();
                let weight = match cfg.weight.get(name) {
                    Some(&w) if w.is_finite() && w >= 0.0 => w,
                    Some(w) => {
                        problems.push(format!("[factors] weight {name}: {w} must be a number >= 0"));
                        f.default_weight()
                    }
                    None => f.default_weight(),
                };
                let scope = match cfg.scope.get(name).map(|s| (s, Scope::parse(s))) {
                    Some((_, Some(s))) => s,
                    Some((s, None)) => {
                        problems.push(format!("[factors] scope {name}: {s:?} must be \"all\" or \"tier:N\""));
                        Scope::All
                    }
                    None => Scope::All,
                };
                let report = match cfg.report.get(name).map(|s| (s, Report::parse(s))) {
                    Some((_, Some(r))) => r,
                    Some((s, None)) => {
                        problems.push(format!("[factors] report {name}: {s:?} must be off, info or warning"));
                        f.default_report()
                    }
                    None => f.default_report(),
                };
                Resolved { weight, scope, report }
            })
            .collect();
        (Registry { factors: factors.into_iter().map(std::sync::Arc::from).collect(), res }, problems)
    }

    /// Fluctuating coefficients (plan P4): every weight times `1 + amp x u`, `u` uniform in
    /// [-1, 1] from a ChaCha8 stream seeded by `seed` (one draw per factor, in registry order).
    /// `amp` = 0 gives the same weights. Deterministic. O(#factors).
    pub fn fluctuated(&self, seed: u64, amp: f64) -> Registry {
        use rand::{Rng, SeedableRng};
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);
        let mut r = self.clone();
        for x in &mut r.res {
            let u: f64 = rng.gen_range(-1.0..=1.0);
            if amp > 0.0 {
                x.weight *= (1.0 + amp * u).max(0.0);
            }
        }
        r
    }
    pub fn names(&self) -> Vec<&'static str> {
        self.factors.iter().map(|f| f.name()).collect()
    }

    pub fn weight(&self, name: &str) -> Option<f64> {
        self.factors.iter().position(|f| f.name() == name).map(|i| self.res[i].weight)
    }

    /// Price of one event for `w`: the sum over the factors in scope of `weight x amount` (mm).
    pub fn price(&self, ev: &Event, w: &WireCtx) -> f64 {
        self.factors
            .iter()
            .zip(&self.res)
            .filter(|(_, r)| r.scope.covers(w))
            .map(|(f, r)| r.weight * f.amount(ev))
            .sum()
    }

    /// Per-move evaluation: the events of `mv` in `st` priced for the wire `w`. Read-only.
    pub fn cost(&self, st: &State, mv: &Move, w: &WireCtx) -> f64 {
        events(st, mv).iter().map(|e| self.price(e, w)).sum()
    }

    /// Report lines for `evs`: one per event a factor with `report` above `off` prices.
    pub fn findings(&self, evs: &[Event], w: &WireCtx) -> Vec<Finding> {
        let mut out = Vec::new();
        for e in evs {
            for (f, r) in self.factors.iter().zip(&self.res) {
                if r.report > Report::Off && r.scope.covers(w) && f.amount(e) > 0.0 {
                    out.push(Finding { level: r.report, factor: f.name(), text: f.describe(e) });
                }
            }
        }
        out
    }

    /// Name, unit, default weight and doc of every factor, for the generated reference.
    pub fn reference(&self) -> Vec<(&'static str, &'static str, f64, &'static str)> {
        self.factors.iter().map(|f| (f.name(), f.unit(), f.default_weight(), f.doc())).collect()
    }
}

/// A candidate change of the State, to be priced.
pub enum Move<'a> {
    /// P2: insert `wire` along `path` (gaps `(edge, g)` before the insert) through `tris`.
    Insert { wire: &'a Wire, path: &'a [(EdgeId, usize)], tris: &'a [TriId], scale: f64 },
}

/// The events of `mv` in `st`. Read-only; O(path x traversals per triangle).
pub fn events(st: &State, mv: &Move) -> Vec<Event> {
    match *mv {
        Move::Insert { wire, path, tris, scale } => insert_events(st, wire, path, tris, scale),
    }
}

fn insert_events(st: &State, wire: &Wire, path: &[(EdgeId, usize)], tris: &[TriId], scale: f64) -> Vec<Event> {
    let pr = st.proj;
    let id: WireId = st.wires.len();
    let mut out = Vec::new();
    // Polyline: terminal, gap points, terminal.
    let mut pts: Vec<Vec2> = vec![pr.pos[wire.a]];
    pts.extend(path.iter().map(|&(e, g)| st.gap_point(e, g)));
    pts.push(pr.pos[wire.b]);
    let mm: f64 = pts.windows(2).map(|p| (p[1] - p[0]).len()).sum::<f64>() / MM;
    out.push(Event::Length { wire: id, mm, scale });
    // Boundary coordinates of the ends of traversal k: the new wire has no passes yet, so its own
    // earlier passes on the same edge are ignored (a path crosses an edge once in practice).
    let end = |t: TriId, k: usize, entry: bool| -> f64 {
        if entry {
            if k == 0 {
                pr.corner_pos(t, wire.a)
            } else {
                st.gap_bpos(t, path[k - 1].0, path[k - 1].1)
            }
        } else if k == path.len() {
            pr.corner_pos(t, wire.b)
        } else {
            st.gap_bpos(t, path[k].0, path[k].1)
        }
    };
    let me = Item { net: wire.net, w: wire.w, s: wire.s };
    for (k, &t) in tris.iter().enumerate() {
        let (a1, a2) = (end(t, k, true), end(t, k, false));
        for &(o, ok) in &st.tri_trav[t] {
            for (b1, b2) in st.chords(o, ok) {
                if interleave(a1, a2, b1, b2) {
                    out.push(Event::Crossing { a: id, b: o, tri: t, same_net: st.wires[o].net == wire.net });
                }
            }
        }
        for pad in st.blocked_pads(t, wire.net) {
            out.push(Event::BlocksPad { wire: id, tri: t, pad });
        }
        // A triangle over another net's pad fixes the layer.
        if pr.tri_obs[t].iter().any(|&o| pr.obstacles[o].net != Some(wire.net)) && k > 0 {
            out.push(Event::LayerForced { wire: id, edge: path[k - 1].0 });
        }
    }
    for (j, &(e, g)) in path.iter().enumerate() {
        let (nt0, nt1) = (tris[j], tris[j + 1]);
        let alone = pr.edge_demand(e, &[me]);
        if (0..pr.n_layers).any(|l| alone > pr.room_for(e, l, wire.net)) {
            out.push(Event::LayerForced { wire: id, edge: e });
        }
        if st.overflows(e, g, nt0, nt1, wire.net, wire.w, wire.s) {
            out.push(Event::Overflow { wire: id, edge: e });
        }
        if st.history[e] > 0.0 {
            out.push(Event::Congestion { wire: id, edge: e, history: st.history[e] });
        }
        if let (Some(m0), Some(m1)) = (pr.allowed(nt0, wire.net), pr.allowed(nt1, wire.net)) {
            if let Some(amount_nm) = st.ignored_violation(e, m0.0 & m1.0, wire.net, pr.edge_demand(e, &[me])) {
                let [p, q] = pr.edges[e];
                out.push(Event::IgnoredRuleViolated { rule: "copper_edge_clearance".into(), at: (pr.pos[p] + pr.pos[q]) * 0.5, amount_nm });
            }
        }
        if let Some(room) = st.room_best_layer(e, nt0, nt1, wire.net) {
            let need = st.edge_need(e, Some((g, me)));
            let n = st.edge_order[e].len();
            if g > 0 && g < n {
                out.push(Event::Squeeze { wire: id, edge: e, slack_nm: room - need, pitch_nm: wire.w + wire.s });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev_cross() -> Event {
        Event::Crossing { a: 0, b: 1, tri: 0, same_net: false }
    }

    #[test]
    fn defaults_are_the_plan_table() {
        let r = Registry::standard();
        let w = WireCtx::default();
        assert_eq!(r.price(&Event::Length { wire: 0, mm: 3.0, scale: 2.0 }, &w), 6.0);
        assert_eq!(r.price(&Event::Via { wire: 0, count: 2, at: None }, &w), 50.0);
        assert_eq!(r.price(&ev_cross(), &w), 20.0);
        assert_eq!(r.price(&Event::Crossing { a: 0, b: 1, tri: 0, same_net: true }, &w), 0.0);
        assert_eq!(r.price(&Event::PlaneLength { wire: 0, layer: 1, mm: 2.0 }, &w), 20.0);
        assert_eq!(r.price(&Event::PlaneCut { wire: 0, layer: 1 }, &w), 25.0);
        assert_eq!(r.price(&Event::Necking { wire: 0, from_nm: 200e3, to_nm: 150e3, mm: 4.0 }, &w), 30.0);
    }

    #[test]
    fn fluctuation_is_seeded_and_bounded() {
        let r = Registry::standard();
        let w = WireCtx::default();
        let via = |g: &Registry| g.price(&Event::Via { wire: 0, count: 1, at: None }, &w);
        let (a, b, c) = (r.fluctuated(7, 0.2), r.fluctuated(7, 0.2), r.fluctuated(8, 0.2));
        assert_eq!(via(&a), via(&b), "same seed, same weights");
        assert_ne!(via(&a), via(&c), "another seed, other weights");
        assert!((20.0..=30.0).contains(&via(&a)), "{}", via(&a));
        assert_eq!(via(&r.fluctuated(7, 0.0)), 25.0);
    }

    #[test]
    fn config_sets_weight_scope_and_report() {
        let cfg = FactorConfig {
            weight: BTreeMap::from([("crossing".into(), 40.0), ("nope".into(), 1.0)]),
            scope: BTreeMap::from([("via".into(), "tier:1".into()), ("length".into(), "bad".into())]),
            report: BTreeMap::from([("crossing".into(), "info".into())]),
        };
        let (r, problems) = Registry::from_config(standard_factors(), &cfg);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert_eq!(r.price(&ev_cross(), &WireCtx::default()), 40.0);
        let via = Event::Via { wire: 0, count: 1, at: None };
        assert_eq!(r.price(&via, &WireCtx { net: 1, tier: 3 }), 0.0, "out of scope");
        assert_eq!(r.price(&via, &WireCtx { net: 1, tier: 1 }), 25.0);
        let f = r.findings(&[ev_cross()], &WireCtx::default());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].level, Report::Info);
    }
}
