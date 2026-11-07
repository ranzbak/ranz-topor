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
| `via` | 25 per via | a via; where the layers a wire can use run out (P2's layer pin: a stretch held to one layer by a narrow gap or another net's pad meets one held to another layer, or its end pad's) counts as one, once per change |
| `crossing` | 20 per crossing | two nets cross in P2 |
| `same_net_crossing` | 0 | a net crossing itself (a loop) |
| `plane_signal` | 10 per mm | signal copper on a plane layer, on top of its length |
| `plane_cut` | 25 per cut | a signal wire cutting a plane |
| `necking` | 30 per stretch x (1 + depth / width), report warning | a stretch narrower than the netclass width; P2 prices it where a stretch starts to neck (M-C: the plan decides the widths, TopoR 6.3 manual p. 56, pp. 72-73) |
| `overflow` | 50 per pass | a pass through an overfull cut (P2 search penalty) |
| `congestion` | 10 per pass per round | a pass through a cut that was over-full in earlier P4 rounds (history, PathFinder) |
| `squeeze` | 5 per fully squeezed pass | a pass between two stretches that leaves the cut less than one track pitch of slack on its roomiest layer (linear) |
| `via_squeeze` | 5 per fully squeezed passage-layer, report warning | a via whose passage next to it (any layer: a through via blocks all) keeps less slack than one via diameter (linear); P3/P4 put each via where this is least, the drawing takes the valid site with the most room; a drawn via with less than half a diameter of slack is reported ("via in a narrow pass") |
| `blocks_pad` | 30 per pad | a stretch takes the last free entry sector (ring triangle) of another net's pad that has no wire yet |
| `ignored_rule` | 100 per violation, report warning | a violated KiCad check set to ignore |
| `reroute` | 5 per earlier reroute | a connection the P4 search moves again after `count` earlier reroutes (reroute counter; also: the negotiated rip-up takes such a wire out last) |
| `branch` | 1 per branch point or through-pad, **subtracted** (a reward) | a connection end that joins its net's routed copper at a branch point on a trace or at a pad that already carries one trace (D5 net trees, `[search] net_tree = "free"`); a goal-cost adjustment in the multi-source search (A* stays exact), so a separate path still wins when the branch costs more |
| `smd_through` | 0 per extra trace end on an SMD pad | assembly cost of a through-pad on a single-layer pad (off by default so it does not cancel `branch`) |
| `junction_angle` | 10 × (w + s) mm × ψ(θ), ψ(θ) = (sec(60° − θ/2) − 1) · sin 60° / sin(θ/2) below 120°, 0 from 120° (D6b; Ras, Swanepoel & Thomas arXiv 1605.01172 Prop. 7.3 p. 12 times the wedge factor) | a new trace that leaves a junction (branch point on a trace: the trunk's two arms; a pad that already carries traces: their directions) at angle θ to the nearest arm; 0 from 120° (Steiner optimum, Popov & Popov EDN 2013) up, no cutoff; priced in the search at the goal (direction from the junction to the last gap point) and when leaving a junction seed |
| `any_angle` | 15 per stretch + 2 per mm (amount 1 + mm x 2/15), report warning (M-45) | `[drawing] style = "octilinear"`: a stretch of drawn copper not at 0/45/90 degrees, `Event::AnyAngle { wire, at, deg, mm }`; consecutive any-angle segments are one stretch. The octilinear drawing (`topo/octi.rs`) weighs it against length and bends and keeps any angle only where no 0/45/90 shape fits in the wire's lane (plan "45° is a cost, not a law"); `[drawing] any_angle = "forbidden"` makes such a run a drawing failure. No social side of its own: a shape that takes room from another wire is caught by the clearance check and the lane walls |
| `acute_bend` | 50 per bend x (1 + (90 - interior) / 90), report warning (M-45) | a bend sharper than 90 degrees (interior angle below 90: acid trap, impedance), `Event::AcuteBend { wire, at, interior_deg }`; the octilinear drawing prices it per turn, `[drawing] acute = "forbidden"` never makes one |
| `violation` | 1000 per violation + 1000 per 0.01 mm² of depth × length, report warning (M-B1 never drop) | a rule the written copper breaks where the drawing found no legal geometry and drew the connection anyway on its least violating geometry (`[optimize] allow_violations`), `Event::Violation { kind, at, layer, depth_nm, len_nm, against, cause }`; a connection without copper is `Event::Open { pitch_nm, len_nm, cause }`, its pitch as the depth over its straight length (the worst case). The archive counts each connection with violations as one error (as a connection without copper) before the objective (Polubasov 2010, «Многовариантная оптимизация»: a violation is not compensated by any length) and adds the price to the objective's value within one count |
| `dp_uncoupled` | 1 per mm + 4 per mm past the rule's maximum (smooth knee, `factors/diffpair.rs` `knee`) (D7) | a differential-pair connection's length not coupled at the gap (KiCad's measure: max(len P, len N) minus the coupled length), `Event::DpUncoupled { mm, max_mm }`; `max_mm` from a custom rule `diff_pair_uncoupled (max ..)` or `[diffpair] max_uncoupled_mm`, 0 = none (then no knee). In the plan the fan-out zone is the pair wire's necked pad end, priced by `necking`; this factor prices the drawn result (the archive's penalty) |
| `dp_skew` | 10 per mm + 4 per mm past the rule's maximum (D7) | length difference P - N of a pair connection, `Event::DpSkew { mm, max_mm }`; the report also gives the bend estimate `sum 2 s p tan(theta/2)` of the centre line (0.83 x pitch per 45-degree bend, opposite bends cancel; doc kicad-diff-pairs.md 3.3); `max_mm` from a custom rule `skew (max ..)` or `[diffpair] skew_max_mm` |
| `dp_gap_deviation` | 5 per mm x dev / (dev + tolerance) (D7) | parallel pair tracks (within twice the gap) at a gap off the target by more than `[diffpair] gap_tolerance_mm`, `Event::DpGapDeviation { mm, dev_mm, tol_mm }` (the fan-out stubs at pad pitch count here) |
| `dp_via_unpaired` | 25 per via, report warning (D7) | a via on one track of a pair connection without a partner via, `Event::DpViaUnpaired { count }` (reserved: the coupled drawing always places vias in pairs; pairs routed as two single tracks are not counted yet) |
| `dp_polarity_cross` | 1000 per connection, report warning (D7) | P and N swap sides between the ends of a coupled pair connection, `Event::DpPolarityCross { at }` (the planner routes such a connection pair as two single tracks, so this only fires when the drawn centre line arrives from the other side) |

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

## Negotiated rip-up and reroute (P4)

Two loops use the `overflow` and `congestion` factors (code: `topo/core.rs`, `topo/refine.rs`,
`topo/pack.rs`):

1. **Inside a route** (after P2 and P3, `search.reroute_rounds` rounds at most). The layer assignment is
   first refined against capacity (`refine.rs`: a crossing flip or relabel that lowers layer changes plus
   the `overflow` price of the over-full edge-layers; it never adds a via unless `TOPO_CAPVIA=1` is set for
   an experiment). What is still over-full by the drawing's packer (`pack.rs`, the same numbers
   `Drawer::pack` uses) is ripped up and routed again with a history (`congestion`, per round an edge was
   over-full) added to the edges that were over-full; P3 runs again. Per over-full edge-layer the wires are
   taken out least-moved first (the `reroute` factor's price of their reroute count), then routed-last
   first, until the rest fits. Every round prices its searches with fluctuating coefficients
   (`optimize.fluctuation`, seeded). Stops when nothing is over-full, after two rounds without gain, at
   `search.reroute_rounds` or when `optimize.max_ripped` wires were taken out. The kept round is the one
   with the lowest board objective: the `overflow` price of the labelling plus `length + mm_per_via x vias`
   (`length_vias`) or the occupied area (`area`).
   A via without a site in its interval puts the history on the edges that bound that interval only (M-B2;
   before: on all edges of its wire). In the first round the wires whose planned via count rose against
   the cycle before are taken out first (Polubasov 2010, «Оптимизация расслоения»). The present `overflow`
   price is kept level with the largest history (`max(overflow, congestion x max history)`, BoxRouter 2.0,
   Cho et al. 2009, p. 32:8–9: else history dominates and the negotiation "spins out of control"). The score
   counts the crossings that cannot be layered as predicted violations next to the over-full edge-layers.
2. **Across routes** (the topology feedback, the cycles of `optimize.rounds` at most), local and fading
   (M-B2; Dr. CU, IEEE TCAD 2020, §IV-C; Haas 2026 §VI-E). Where a connection's copper breaks a rule (or its
   drawing was rejected), the passes of its wire near the place get a history that only that connection pays,
   weighted by their distance to the violation (1 on it, `1 / (1 + (d / r)²)` around it, r = half the
   violation's length or one pitch), not its whole sleeve. Every edge that was over-full under the packer gets
   the shared history, and the cut nearest to a failure gets a lowered room. Everything carried over fades by
   `optimize.history_fade` (0.5) per cycle. When a cycle comes out worse than the best so far, the lowered
   rooms turn soft: their passages get a history (a price) instead (`optimize.soft_cuts`). Every route offers
   its results to the variant archive (below).
3. **Crossing rules** (`topo/xrules.rs`, `optimize.crossing_rules`), before and after loop 1: the patterns
   that force a crossing near a terminal or twice — a wire of another net in the ring of a terminal pad that
   crosses the terminal's wire or its straight line (a clinch at a terminal, Luzin & Popov 2012 sign 1,
   including the detour of their Fig. 3a), two wires of different nets crossing twice (a half-loop, Luzin
   2013 Fig. 16), a crossing that cannot be layered — are re-planned pairwise: both wires out, the rule's
   first wire routed first (`Topo::try_joint`), kept when the pair's price drops by more than their `reroute`
   prices; the pass is kept when the board score gets better. Wires of a net tree with branch points keep
   their place.

Set `TOPO_NOCAP=1` to switch the capacity refinement off (debug comparison only).

## P4 moves (`topo/moves.rs`, `topo/descent.rs`)

After the negotiated rip-up (`optimize.moves`, off by default, at most `optimize.max_moves` evaluations per route) a
descent tries local moves, each priced through the registry: a wire's price is the price of its insert
events (`Registry::cost` of `Move::Insert`) with the wire itself taken out (`State::detach` /
`State::reattach`), so the social factors it causes count. A move is kept only when the price drops (a
reroute must also beat the `reroute` factor's price of its count).

| move | what it changes |
|---|---|
| reroute | the wire is searched again on the current state |
| attachment | the connection goes to another pad pair across the same cut of its net's tree (up to 3 nearest pairs); the report shows the new pads |
| merge | a stretch running parallel to a stretch of its own net (two or more shared edges) is put right next to it |
| channel swap | a crossing slides to the far end of the run of edges the two wires share |
| joint move | two crossing wires are taken out and routed again in the other order |
| double-crossing swap | `State::uncross_all`, reused from P2 |
| crossing flip | the two events of a crossing swap layers (`refine::try_move`, reused) |
| via shift | a layer change moves over one event along its wire |
| segment lifting | a stretch between two layer changes goes onto another signal layer |

Capacity test: a topological move may not add passes through cuts that are over-full now (packer) or
whose passes do not all fit on the narrowest layer; a label move may not make any edge-layer more
over-full and may not add vias. The topological descent as a whole is kept only when the board objective
(above) after P3 is lower than before it; otherwise the state before the moves stays. The run report
lists tried / applied moves per type.

## Result variants (`topo/archive.rs`)

Every whole route offers two variants: as drawn, and after the via reduction. The archive keeps every
variant no other one beats on all four TopoR criteria (errors = connections not routed, vias, length,
shrinkages = necked stretches), plus the fewest-warnings variant; at most `optimize.variants`, the
champions are never pruned. The pick: fewest errors, then `optimize.objective`. The variant table (with
the mm/via column against the previous row) is in the run report.

## Reproducibility

Every stop that changes the result is a work cap (`optimize.rounds`, `max_ripped`, `max_expansions`,
`max_moves`, `search.reroute_rounds`, the fixed sweep caps of the descent and the via reduction); the
randomness is ChaCha8 seeded by `Options::seed`. `run.timeout_s` is only an emergency stop: when it fires
the result carries the warning "time budget hit: result may differ between runs". `TOPO_TIMES=1` prints
phase times per route on stderr (never part of the result).

## Via reduction after drawing (S4, plan P6)

After all copper is drawn (`topo/reduce.rs`, `vias.reduce_after_drawing`), every wire with vias is tried
again with one layer run moved onto the layer of an adjacent run (or all runs onto one layer), which drops
the vias between them. Runs that touch an SMD pad keep the pad's layer; plane layers are not touched; wires of a net that has a pour stay as they are (a track over the pour may be what joins it). The
wire is drawn again in its own sleeve (`Drawer::relayer` gives the passes their new layers and packs the
changed edge-layers again, `draw_wire` draws it); the checker decides, and a rejected try puts the old
tracks and vias back. At most 6 tries per wire and sweep, 3 sweeps; the clock is polled between wires only for the emergency stop.
The run report has a `via reduction after drawing` line with the vias by cause before and after.
`TOPO_NOREDUCE=1` switches it off for a comparison. The topological (pre-drawing) variant and the clinch
fixes of the plan are not implemented.

## Learning weights from hand routings (S3b)

`topo/learn.rs` fits factor weights to hand-routed boards by maximum margin planning (Ratliff 2006,
subgradient method); the module doc defines the factor totals, the loss (hand stretches whose topology
word the engine does not reproduce) and the loss-augmented P2 (`State::search_hooked`, a price per pass
through an edge the hand's net uses). Learned weights are output only, never applied by default:

```bash
cd engine && cargo run --release --example learn -- --board NAME HAND.json INPUT.json [--board ...] \
    --hold-out NAME --out learned.json --toml learned.toml        # one fold
.venv/bin/python -m harness.learn all --out scratch/topo-s3b      # hold-out rotation + full-engine evaluation
```

`harness.learn` writes `report.md` (default vs learned per held-out board, candidate factors from the
hand vs engine event rates) and `learned-avg.toml`, a `[factors] weight = { ... }` snippet to copy into
`topor.toml` by hand.
