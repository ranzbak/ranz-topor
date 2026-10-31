### Task A0 (S; H sub-steps): M3 gate-review minors D-1..D-10 + harness M4 prep — `multigraph.rs`, `pourcheck.rs`, `route.rs`, `search.rs`, tests, `harness/snapshot.py`, `harness/progress.py`, `harness/bench.py`

- [ ] **Step 1 (D-4):** `site_inside_fill_hole_is_not_in_pour` (`engine/tests/multigraph.rs`); implement point-in-`outer`-minus-`holes`, `O(|outer| + Σ|holes|)` per site.
- [ ] **Step 2 (D-9):** `pourcheck_long_diagonal_track_stamps_linear` (`stamped_cells ≤ 4·L/res·w/res + 64`); scanline per row, `STAMP_CELL_BUDGET = 16_000_000`, deadline per pour.
- [ ] **Step 3 (D-10, H):** `BTreeSet::pop_first` free list. **(D-1):** `Options.strict_unfit_edges` (default `false`; `!fits(e)` rejects in `push_state`), test `strict_unfit_edge_rejected_in_search`; two-layer eprom run with it on recorded in the ledger (A4 input).
- [ ] **Step 4 (D-2, D-3, D-5, H):** split-vertex fixture asserts `"failed"` + reason prefix `"split vertex"`; e2e `rule_t_via_counter_example`; assert the real warning text `"without pour after"`. **(D-6):** `pour_connection_uses_listed_same_net_via_as_goal`. **(D-7, H):** note §7 amendment. **(D-8):** `test_snapshot_existing_pin_survives_source_edit`.
- [ ] **Step 5 (harness):** `render_progress(task, *, milestone="m4", …)` → `<date>-<milestone>-<task>[-<board>].png`, caption adds `stop_reason`; bench summary/README columns `detour` (reference), `detour_ghost`, `stop_reason`, `vias_per_net` (filled by A7/P1 when present); `test_progress_m4_name_and_caption_lines`.
- [ ] **Step 6:** cargo/clippy/pytest green; eprom/wiccon byte-identical. Commit `fix(engine): M3 review minors D-1..D-10; harness m4 progress and bench columns`.


## Coordinator additions (binding)
- Step 3 (D-1): `Options.strict_unfit_edges` default false; the plugin schema already has `[search] strict_unfit_edges` with
  `since = "0.5.0"` — make it a live engine-mapped key (engine_field), doc string without "(available from 0.5.0)" text if the
  schema mechanism allows per-key activation; regenerate docs/user/configuration.md (`PYTHONPATH=plugin .venv/bin/python -m topor.docs_gen`)
  and keep `--check` clean. Record the two-layer eprom run with strict on (routed, outside length, vias, router_s) in your report.
- Dedupe `relax_iter_cap` (P1 concern): engine has both top-level `Options.relax_iter_cap` and `expert.relax_iter_cap`
  (`Options::relax_cap` resolves). Keep ONE source of truth (`expert.relax_iter_cap`); the top-level key stays accepted on input
  (serde alias / legacy field documented) so old JSON still parses, but `default_options()` no longer lists a duplicate;
  update plugin ENGINE_INTERNAL accordingly; tests green.
- Step 5 (harness, Ruling A-C3): `harness.progress` gets `--options '<json>'` (merged over the board defaults) and dcsgonebadge's
  default board options become the M4 gate options `{"wall_clock_s": 600, "partial_routes": false}` when `--milestone m4`
  (m3 keeps 60 s). Caption adds `stop_reason`. Bench summary columns `detour` (reference, empty until A7), `detour_ghost`,
  `stop_reason`, `vias_per_net`.
- Step 6: eprom + wiccon byte-identical vs main 6ea3040 (compare engine.route JSON minus runtime_s/threads_used on the bench model
  JSONs; print sha256 of both). No progress PNG needed unless numbers move (they must not).
- Commit message: `fix(engine): M3 review minors D-1..D-10; harness m4 progress and bench columns` (more commits fine).
