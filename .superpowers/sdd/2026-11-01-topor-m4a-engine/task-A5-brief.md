### Task A5 (S): same-net Steiner attachment and net ordering (M4-D3) — `decompose.rs`, `search.rs`, `route.rs`, `wires.rs`

**Interfaces:** `Goal::OwnStrand { conn, leg, edge }`; `ConnReport.junction: bool`; `Options.steiner: bool = true`. One extra goal set per sibling, `O(Σ sibling leg length)` per search.

- [ ] **Step 1:** `t_junction_onto_routed_sibling` (#207 fixture: ≤ 1.2 mm T-junction, 0 vias, sibling split, self-check clean), `t_junction_onto_hand_copper`, `steiner_off_is_m3_behaviour`, `junction_angle_below_60_is_rejected` (`engine/tests/route_steiner.rs`). FAIL → implement → PASS.
- [ ] **Step 2:** eprom/wiccon not worse; threads byte-identical; private-badge-b 60 s `count > 1.5` reported (expected ≤ 6). PNG `--task A5 --board private-badge-b`. Commit `feat(route): same-net Steiner T-junctions and net ordering`.


## Coordinator additions (binding)
- Design: docs/progress/notes/m4-capacity-steiner-design.md §2 (A3). Diagnosis: the private-badge-b completion diagnosis (note removed: private board) §6 item 5
  (≈ 8–10 on private-badge-b: sibling connections of one net hold the single exit of a fine-pitch pad — #75 U4.2→U4.4 + #77 U4.2→C22.1,
  #144/145, #192/193, #212/213, #196/#32 detours; re-identify by labels on current main).
- Steiner changes eprom (two 2-connection nets): "not worse", not identity (A3 concern 3). The M2 front-only guard must stay green.
- Revisit Ruling A-C6: with OwnStrand goals, should listed same-net pour vias also be goals (Options.pour_listed_via_goals)? Measure
  wiccon on/off (vias, outside length) and recommend; keep the default false unless wiccon is not worse.
- Option `[search] steiner` (schema since 0.5.0 → live key), junction angle limit as an Options field (default 60°).
- PNG `--milestone m4 --task A5 --board private-badge-b` (+ wiccon/eprom if numbers moved).

- Acceptance (every task, main session): completion AND detours are non-regression items: on eprom, wiccon and private-badge-b report
  routed/connections_total, failed, vias, routing_new, unconnected, router_s, AND detour (reference, A7) median/p90/max/count > 1.5
  plus detour_ghost; count > 1.5 and max must not get worse than main at your base (numbers for both in the report), unless a
  connection newly routes (say which). Not-worse gate numbers (global-constraints.md) for eprom/wiccon; NOTE wiccon outside-pad length
  headroom is small (bound 1380 mm) — report it.
- private-badge-b at the gate options (A-C3; harness.progress --milestone m4 applies them). Gate predicate A-C11.
- License: topor is GPL-2.0-or-later (main 93f2126). Porting code from the gEDA PCB toporouter (src/toporouter.c, GPL-2.0-or-later,
  Anthony Blake) is allowed when it helps: keep the original copyright notice + attribution at the port site, name the origin in the
  commit message and report it (coordinator records a ruling). No new Apache-2.0-only runtime crates (no new crates at all without asking).
