### Task A14 (S): finishing hand-started traces — G1 continuation — `wires.rs`, `relax.rs`, `route.rs`, `model.rs`, `harness/fixtures.py`

**Interfaces:** `Terminal::Dangling { at, direction }`; `Options.dangling_ends: String` (`"continue" | "fixed"`, default `"continue"`); `g1_angle_deg` computed; `make_fixture_board(..., dangling_trace=True)`.

- [ ] **Step 1:** `dangling_end_continues_tangent` (`g1_angle_deg ≤ 1.0`, same width/layer, hand copper untouched), `dangling_hint_keeps_side`, `dangling_fixed_mode_is_m3` (`engine/tests/route_g1.rs`); `test_bench_dangling_fixture_g1`. FAIL → implement → PASS. No PNG from the fixture (test pattern); eprom PNG only if its numbers moved. Commit `feat(route): G1 continuation of hand-started traces`.


## Coordinator additions (binding; Ruling A-C12)
- Design: docs/progress/notes/m4-capacity-steiner-design.md §4 (A3; tangent rail discs ρ = w/2 + max(c, R_min) replace the plan's rail
  formula — follow the note, report deviations). 45 §1 M4 row "finishing hand-started traces" is the spec.
- `[search] dangling_ends` ∈ {continue (default), fixed (M3 behaviour: hand stub stays fixed copper, route from the pads), drop (rip the
  dangling hand stub in scope, then route as usual)}; engine `Options.dangling_ends` takes the same strings; schema choices updated
  (since 0.5.0 → live key); plugin dangling-ends detection (toporlib/geom.py) feeds the engine; regenerate configuration.md.
- Snapshot boards: report how many dangling ends each snapshot has (harness model `dangling_ends`) and whether any routing result
  changes (eprom/wiccon/private-badge-b expected unchanged if they have none; if they have some, numbers not worse).
- No PNG from the fixture (test pattern). PNG only for a snapshot board whose numbers moved.

- Acceptance (every task, main session): completion AND detours are non-regression items: on eprom, wiccon and private-badge-b report
  routed/connections_total, failed, vias, routing_new, unconnected, router_s, AND detour (reference, A7) median/p90/max/count > 1.5
  plus detour_ghost; count > 1.5 and max must not get worse than main at your base (numbers for both in the report), unless a
  connection newly routes (say which). Not-worse gate numbers (global-constraints.md) for eprom/wiccon; NOTE wiccon outside-pad length
  headroom is small (bound 1380 mm) — report it.
- private-badge-b at the gate options (A-C3; harness.progress --milestone m4 applies them). Gate predicate A-C11.
- License: topor is GPL-2.0-or-later (main 93f2126). Porting code from the gEDA PCB toporouter (src/toporouter.c, GPL-2.0-or-later,
  Anthony Blake) is allowed when it helps: keep the original copyright notice + attribution at the port site, name the origin in the
  commit message and report it (coordinator records a ruling). No new Apache-2.0-only runtime crates (no new crates at all without asking).
