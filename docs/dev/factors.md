# Writing a social factor

A factor prices one thing a route does. P2 (and later P3, P4) sum the prices of a candidate move; the
router picks the cheapest. This guide is for contributors adding or changing a factor. Users change
weights, scopes and report levels in `topor.toml [factors]`; see
[configuration.md](../user/configuration.md#factors).

Code: `engine/src/topo/factors/` (one file per factor, `mod.rs` for the trait, events, registry and the
per-move evaluation).

## Trust model

**Factors are trusted code.** A factor file (Rust today, a declarative file later) runs inside the router
with full access to the engine: there is no sandbox and no WASM. Only add factor files you trust, and
review a factor PR like any engine code. Two rules hold for every factor:

1. **Deterministic.** No `HashMap` iteration, no clocks, no unseeded randomness. The same state and the
   same move give the same price on every run and every thread count.
2. **Read-only.** A factor reads the topology state; only moves write it. `Factor::amount` gets an event,
   not the state; the event query interface below is the only way a factor learns about the State.

Hard rules are not factors. Fab limits read from KiCad (minimum width, clearance, hole size, copper to
edge) never bend. A KiCad check whose severity is set to `ignore` is not hard either; see
[ignored rules](#ignored-kicad-checks).

## The pieces

| Piece | What it is |
|---|---|
| `Event` | Something a move does, with typed fields: `Length`, `Via`, `LayerForced`, `Crossing`, `PlaneLength`, `PlaneCut`, `Necking`, `Overflow`, `IgnoredRuleViolated`. |
| `Factor` | Prices events: `amount(&Event) -> f64` is how many of the factor's units the event is (0 = not mine). Cost = weight x amount, in mm of copper. |
| `Registry` | The factors with weight, scope and report level resolved from `topor.toml` (`FactorConfig`). `price(event, wire)`, `cost(state, move, wire)`, `findings(events, wire)`. |
| `Move` | A candidate change, read-only. Today `Insert { wire, path, tris, scale }` (P2). `events(state, move)` derives its events. |
| State queries | `demand(e, mask)`, `room(e, layer)`, `crossings(wire)`, `passes_between(o1, o2)`: how factors and the report look at the state. |

```rust
pub trait Factor: Send + Sync {
    fn name(&self) -> &'static str;          // key in [factors] tables and in report lines
    fn doc(&self) -> &'static str;           // one sentence for the reference
    fn unit(&self) -> &'static str;          // "per mm", "per via", ...
    fn default_weight(&self) -> f64;         // mm per unit
    fn default_report(&self) -> Report { Report::Off }
    fn amount(&self, ev: &Event) -> f64;     // units of this factor in ev
    fn describe(&self, ev: &Event) -> String // one report line
}
```

## Template

Copy `engine/src/topo/factors/via.rs`:

```rust
use super::{Event, Factor};

pub struct MyFactor;

impl Factor for MyFactor {
    fn name(&self) -> &'static str { "my_factor" }
    fn doc(&self) -> &'static str { "What it prices, in one sentence." }
    fn unit(&self) -> &'static str { "per thing" }
    fn default_weight(&self) -> f64 { 0.0 }          // a new factor ships at 0 unless the plan says otherwise
    fn amount(&self, ev: &Event) -> f64 {
        match *ev { Event::Crossing { same_net: false, .. } => 1.0, _ => 0.0 }
    }
}
```

Then:

1. `mod my_factor;` and `Box::new(my_factor::MyFactor)` in `standard_factors()` (`mod.rs`).
2. If it needs a new event kind, add the variant to `Event` (typed fields, units in the doc comment) and
   emit it from `events()` or from the code that makes the thing happen. Existing factors ignore it.
3. Add the name and default to the `[factors] weight` text in `plugin/topor/schema.py`, run
   `PYTHONPATH=plugin python -m topor.docs_gen --write`, and extend the list in
   `the_factor_reference_matches_the_schema_text` (`engine/tests/topo_g0.rs`); `docs_gen --check` must be
   green.

## Weight, scope, report

`topor.toml`:

```toml
[factors]
weight = { crossing = 40.0, via = 30.0 }   # mm per unit; absent = the factor's default
scope  = { via = "tier:1" }                # "all" (default) or "tier:N"
report = { crossing = "info" }             # "off" | "info" | "warning"
```

An unknown factor name or a bad value is a problem string from `Registry::from_config`; the default is
used. Defaults of the rough router (plan "Default weights"):

| Factor | Default | Prices |
|---|---|---|
| `length` | 1 per mm x (w+s)/(w0+s0) | copper length |
| `via` | 25 per via | a via; a stretch forced onto one layer counts as one |
| `crossing` | 20 per crossing | two nets cross in P2 |
| `same_net_crossing` | 0 | a net crossing itself (a loop) |
| `plane_signal` | 10 per mm | signal copper on a plane layer, on top of its length |
| `plane_cut` | 25 per cut | a signal wire cutting a plane |
| `necking` | 30 per stretch, report warning | a stretch narrower than the netclass width |
| `overflow` | 50 per pass | a pass through an overfull cut (P2 search penalty) |
| `congestion` | 10 per pass per round | a pass through a cut that was over-full in earlier P4 rounds (history, PathFinder) |
| `squeeze` | 5 per fully squeezed pass | a pass between two stretches that leaves the cut less than one track pitch of slack on its roomiest layer (linear) |
| `blocks_pad` | 30 per pad | a stretch takes the last free entry sector (ring triangle) of another net's pad that has no wire yet |
| `ignored_rule` | 100 per violation, report warning | a violated KiCad check set to ignore |

Everything else is 0 until the learning loop fits it. Do not tune a default to make a gate agree.

## Ignored KiCad checks

When a check is set to `ignore` in KiCad (Board Setup, Violation Severity; for example
`copper_edge_clearance` in dcsgonebadge), topor does not treat it as a hard rule. A violation of it is an
`Event::IgnoredRuleViolated { rule, at, amount_nm }`, priced by `ignored_rule` (high by default) and
reported as a warning at `at`. The rule's value is read from KiCad every run and never written into
topor. P2 prices it (S3): with the check ignored the projection bounds the cuts at the outline with a
clearance of 0 (`Projection::edge_clear_ignored` keeps the rule's value), and a pass that fits on a layer
only without the rule pays `ignored_rule` once per cut. The drawing reports the segments that really
violate it. Tested in `engine/tests/topo_s3.rs`.

## Using a factor from P2

P2 does not hold constants. `InitWeights::from_registry(&registry, &WireCtx { net, tier })` asks the
registry for the price of the events P2 creates (a crossing, a layer-forcing pass, an overflow, one mm of
length) and the A* uses those prices; `route_all_reg` does it per connection, so a `scope` takes effect.
`Registry::cost(&state, &Move::Insert { .. }, &wire)` is the same total computed over the events of a
whole path, read-only on the state; use it for move evaluation in P4.

## Tests

* A factor test lives next to the factor (`ignored_rule.rs` shows the shape) or in
  `engine/tests/topo_g0.rs`: price a hand-made `Event`, or build a small board, `events()` a move and
  check the price.
* The G0 gate must not regress: changing a default weight changes P2 and so G0's numbers; say so in the
  commit message. `g0_gate` takes `--factor NAME=WEIGHT` to try a weight without editing code.

```bash
cd engine && cargo test --release --no-fail-fast && cargo clippy --all-targets -- -D warnings
cargo run --release --example g0_gate -- input.json hand.json --factor crossing=40
```
