### Task A16 (S; F check in A3): escalating per-pair rip cost — `negotiate.rs`, `route.rs`

**Interfaces:** `rip()` adds `opts.expert.hist_block · growth^(k−1)` for the k-th rip of B by A, refuses at `k ≥ ripup_max_per_pair`; `RIP_PAIR_CAP` removed; `RouteResult.ripup: RipReport { rips_per_connection: Vec<u32>, pairs_at_backstop: Vec<[usize; 2]>, total: u32 }`; bound `O(max_per_pair · m²)`.

- [ ] **Step 1:** `ping_pong_pair_converges_before_backstop` (`max(rip_pairs) ≤ 4 < 8`, no backstop pair), `backstop_stops_rips`, `growth_one_is_constant_cost`, `ripup_threads_byte_identical` (`engine/tests/negotiate_ripup.rs`). FAIL → implement → PASS.
- [ ] **Step 2:** eprom incl. the F.Cu-only guard and wiccon not worse (iterations reported). PNG `--task A16 --board wiccon-2023`. Commit `feat(negotiate): escalating per-pair rip cost with backstop`.


### Task A13 (S): bounded via position relaxation (M3-R3) — `relax.rs`, `route.rs`

**Interfaces:** `Options.via_relaxation: bool = true`; `RouteResult.via_relaxation: String`; pass after via reduction per note §5.

- [ ] **Step 1:** `via_moves_toward_force_balance_within_face_star`, `via_relaxation_rejects_move_into_keepout`, `via_relaxation_off_matches_previous`, `via_relaxation_threads_byte_identical` (`engine/tests/route_via_relax.rs`). FAIL → implement → PASS. wiccon: via count unchanged, outside ≤ previous, DRC 0. PNG `--task A13 --board wiccon-2023`. Commit `feat(route): bounded via position relaxation`.


## Coordinator additions (binding) — one worker: A16 then A13, two commits
- Design: docs/progress/notes/m4-capacity-steiner-design.md §6 (rip cost) and §5 (via relaxation). 46 row 17 (user): `[ripup] growth = 2.0`,
  `max_per_pair = 8` (backstop only) replace the fixed RIP_PAIR_CAP with constant +0.5 per rip. P1 currently reads
  ripup_max_per_pair as the cap; the M3 damping test pins 3 — update it to the new semantics without weakening what it checks
  (ping-pong settles before the backstop). `RouteResult.ripup` report fields as the plan says; plugin run report lists them.
- DX: neither is a completion lever on private-badge-b; they must not regress it (and wiccon iterations reported before/after).
- A13: `[vias] via_relaxation` (since 0.5.0 → live key). wiccon: via count unchanged, outside ≤ previous, DRC 0. PNG
  `--milestone m4 --task A13 --board wiccon-2023`.

- Acceptance (every task, main session): completion AND detours are non-regression items: on eprom, wiccon and private-badge-b report
  routed/connections_total, failed, vias, routing_new, unconnected, router_s, AND detour (reference, A7) median/p90/max/count > 1.5
  plus detour_ghost; count > 1.5 and max must not get worse than main at your base (numbers for both in the report), unless a
  connection newly routes (say which). Not-worse gate numbers (global-constraints.md) for eprom/wiccon; NOTE wiccon outside-pad length
  headroom is small (bound 1380 mm) — report it.
- private-badge-b at the gate options (A-C3; harness.progress --milestone m4 applies them). Gate predicate A-C11.
- License: topor is GPL-2.0-or-later (main 93f2126). Porting code from the gEDA PCB toporouter (src/toporouter.c, GPL-2.0-or-later,
  Anthony Blake) is allowed when it helps: keep the original copyright notice + attribution at the port site, name the origin in the
  commit message and report it (coordinator records a ruling). No new Apache-2.0-only runtime crates (no new crates at all without asking).
