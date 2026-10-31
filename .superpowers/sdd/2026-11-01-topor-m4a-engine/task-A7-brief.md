### Task A7 (S): DRC-feasible detour reference (M4-D4, M3-R15) — `route.rs`, `model.rs`, `toporlib/detour.py`, `harness/bench.py`, `harness/progress.py`

**Interfaces:** `Options.detour_reference: bool = false`, `Options.detour_reference_budget_pct: f64 = 10.0`; `ConnReport.ref_len_nm: Option<i64>`, `ConnReport.ref_kind: String` (`"feasible" | "ghost" | "none"`): one slot search per connection with full clearance/width on a `MultiGraph` of fixed copper only, same layers/via rules/`via_cost`, `par_iter` collected in order, under the budget share; failure → ghost fallback flagged. `O(m · search)`. `detour_stats(connections, field="ref_len_nm")` → `detour`, `ghost_len_nm` → `detour_ghost`; bench sets `detour_reference: true` (offline only); caption flags `ghost < 2 mm`.

- [ ] **Step 1:** `reference_is_feasible_not_topological` (#208: `ref ≈ 2.9 mm`, ghost 1.0), `reference_off_leaves_fields_none`, `reference_budget_exhausted_falls_back_to_ghost`, `reference_threads_byte_identical` (`engine/tests/route_detour_ref.rs`); `test_detour_stats_uses_reference_and_reports_ghost`. FAIL → implement → PASS.
- [ ] **Step 2:** PNG `--task A7 --board dcsgonebadge`. Commit `feat(route): DRC-feasible detour reference (M3-R15)`.


## Coordinator additions (binding; M3-R15, Ruling A-C5)
- Plugin: `[run] detour_reference` (schema, default true) maps to `Options.detour_reference`; the plugin run passes it with
  `detour_reference_budget_pct` (ENGINE_INTERNAL, 10 % of the remaining budget); the run report shows `detour` (reference) and
  `detour_ghost` lines; remove the "No effect in 0.4.0" tail from its doc string, regenerate configuration.md, `--check` clean.
  NOTE the plan sets the engine default `false` (bench/harness sets true): keep engine default false, plugin default per schema (true).
- Harness: A0 added bench `columns` with an empty `detour` (reference) column reading `detour_ref_len_nm` — wire it to your
  `ref_len_nm` field (rename either side consistently), `detour_ghost` stays the ghost ratio, `result["detour"]` becomes the
  reference stats (ghost fallback flagged per connection, `ref_kind`). PNG caption detour line uses the reference ratio and says
  "vs DRC-feasible reference" (+ count of ghost fallbacks).
- The reference search must not change routing results: route JSON minus the new ref fields/runtime_s byte-identical on eprom and
  wiccon with detour_reference on vs off; threads 1/2/4 identical with it on.
- Report per board (eprom, wiccon, dcsgone at the gate options A-C3): detour median/p90/max/count>1.5/top 5 by reference AND by ghost,
  number of ghost fallbacks, reference time share. PNG `--milestone m4 --task A7 --board dcsgonebadge`.
