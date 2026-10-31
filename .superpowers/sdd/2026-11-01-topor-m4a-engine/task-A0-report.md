# Task A0 report: M3 review minors D-1..D-10 + harness m4 prep

Branch `worktree-agent-a40e6f49811ef949b`, base 6ea3040. Engine version string untouched (0.4.0; coordinator bumps at the gate).

## What changed

| Item | Where | Notes |
|---|---|---|
| D-4 | `engine/src/multigraph.rs::pour_membership`, test `site_inside_fill_hole_is_not_in_pour` (`engine/tests/multigraph.rs`) | point in `outer` and in no hole, annulus (`max_d/2`) clear of the outer ring AND every hole ring; `O(S * sum(|outer| + sum|holes|))`. |
| D-9 | `engine/src/pourcheck.rs` | capsule knockouts are one scanline interval per raster row (`capsule_row_span`, O(1)); `STAMP_CELL_BUDGET = 16_000_000` per run (each net's rasters split the budget left at the net's start equally, so it is deterministic under rayon); new `check_pour_reach_until(.., deadline)` (the old `check_pour_reach` keeps its signature, wraps it with `None`); stop polled every 64 stamps (deadline + cancel flag); a cut raster leaves a `... knockout stamping cut short ...` note (optimistic check). `PourReach.stamped_cells` exposed. `route.rs` passes `deadline_after(wall_clock_s, t0)` at both call sites. Tests `pourcheck_long_diagonal_track_stamps_linear` (stamped <= 4*L/res*w/res + 64) and `stamp_budget_exhaustion_is_noted_not_fatal`. |
| D-10 | `multigraph.rs` | `free_ids: BTreeSet<usize>`, `pop_first`; existing `leg_ids_are_allocated_from_the_sorted_free_list` green. |
| D-1 | `Options.strict_unfit_edges` (default false), `SearchParams.strict_unfit_edges`, `Search.strict_unfit`; `push_state` returns when `strict_unfit && !fits(e)`; ghost never strict. Test `strict_unfit_edge_rejected_in_search` (`search_layers.rs`: default finds a path through the pinch edge, strict is `Blocked`). Plugin: `[search] strict_unfit_edges` is live (`engine="strict_unfit_edges"`, `since` removed, doc text without "available from"); `docs/user/configuration.md` regenerated, `--check` clean. | |
| relax_iter_cap dedupe | `Options.relax_iter_cap` field removed; single source `expert.relax_iter_cap`; `Options::from_json` folds a legacy top-level `relax_iter_cap` into `expert` (explicit `expert.relax_iter_cap` wins); `relax_cap()` returns the expert value. `default_options()` lists no duplicate. `relax_stacks.rs::relax_iter_cap_respected` sets `opts.expert.relax_iter_cap = 1` (same assertion). Removed from plugin `ENGINE_INTERNAL`. | |
| D-5 | `route_layers.rs:562` now asserts absence of the real text `"without pour after"` | |
| D-2 | new `route_split_vertex_bend_fails_with_reason` (`route_engine.rs`): locked net-5 X + pads in the left/right wedge: `status == "failed"`, reason starts with `"sketch: path bends at a wall split vertex"` | the real reason text is prefixed `sketch: `; the brief's "prefix split vertex" is the substring. Old D5 test left as is. |
| D-3 | new e2e `rule_t_via_counter_example` (`route_layers.rs`): 2/2 routed, net-1 via 1, DRC clean, threads 1 vs 4 byte-identical | passes on the unchanged engine (regression pin; it did not expose a bug). |
| D-6 | `route.rs::listed_pour_vias`, `leg_terminals` (`Terminal::Via` for a last leg that ends on a listed via), goal vertices + terminals for pour targets; **behind `Options.pour_listed_via_goals` (default false)**; schema `[search] pour_listed_via_goals`; test `pour_connection_uses_listed_same_net_via_as_goal` (off: 1 new via; on: 0 vias, track ends on the listed via, clean, threads 1/4 identical). Note P1a added. | see Concerns: ON changes wiccon. |
| D-7 | `docs/progress/notes/m3-layers-vias-design.md` §7 amendment (phase-2 ghost precompute, `usable` reuse) + P1a | |
| D-8 | `harness/snapshot.py::snapshot_board`: existing snapshot is verified only on its own pcb hash and returned with its stored manifest; sidecar edits/removals in the source no longer raise; created snapshots are still verified. Test `test_snapshot_existing_pin_survives_source_edit` (sidecar edit, new sidecar, removed sidecar, edited pcb = new dir, tampered snapshot pcb still detected). | |
| Harness | `harness/progress.py`: `render_progress(..., milestone=None, options=None)` (topor router default `m4`, smoother `m1`), `progress_png_name` (`<date>-<milestone>-<task>[-<board>].png`; m1/m3 keep their board-less names), `board_options(board, milestone)` (dcsgone m4 = `{"wall_clock_s": 600, "partial_routes": False}`, m3 stays 60 s), `merged_options`, CLI `--options '<json>'`, `--milestone` default m4; caption already ended with `stop_reason` (kept, documented). `harness/bench.py`: `bench_columns()` stored as `result["columns"]` = `detour` (reference, from per-connection `detour_ref_len_nm`, None until A7), `detour_ghost`, `stop_reason`, `vias_per_net` (report's own `vias_per_net` else derived from `report["nets"]`), plus a `columns:` line in the summary. `toporlib/detour.py::detour_stats(conns, ref_key="ghost_len_nm")`. Tests: `test_progress_m4_name_and_caption_lines`, `test_progress_options_and_names_are_pure_helpers`; `test_progress_m3_caption_and_path` now passes `milestone="m3"` explicitly (same assertions). | `result["detour"]` is unchanged (still the ghost stats, many consumers); the reference column lives in `result["columns"]["detour"]`. A7 must emit `detour_ref_len_nm` per connection (or tell me another name). |

## Options / settings added (judgement calls)
- `strict_unfit_edges` (default false) - `[search] strict_unfit_edges`.
- `pour_listed_via_goals` (default false) - `[search] pour_listed_via_goals`.
- `STAMP_CELL_BUDGET` is a constant (16 M cells), not user-facing.

## Numbers
Byte-identical check: `engine.route` JSON minus `runtime_s`/`threads_used`, models = `harness.bench <board> <dir>` ripped-board model JSONs (snapshot boards), options `{"threads":4}`, baseline = engine built from 6ea3040.

| board | base sha256[:16] | new (defaults) sha256[:16] | routed | vias | outside nm |
|---|---|---|---|---|---|
| eprom-to-kernal | 2dcde8ce40237b0c | 2dcde8ce40237b0c | 27/27 | 0 | 64 627 898 |
| wiccon-2023 | d24bb79b58c7cbeb | d24bb79b58c7cbeb (threads 1 also d24bb79b58c7cbeb) | 62/62 | 37 | 1 373 553 063 |

Strict D2/D10 retest (A4 input), eprom model, `strict_unfit_edges: true`:
- two routable layers (the real model): identical to default (sha 2dcde8ce40237b0c), 27/27, outside 64.63 mm, 0 vias, router 0.14 s.
- front only (the M2/`route_engine` configuration): default 27/27, outside 67.03 mm, 0.66 s; **strict 26/27**, outside 56.57 mm, 0.14 s (the +5V corridor through the unfit edge fails). So strict is a no-op on the two-layer board and costs a connection on the front-only one: keep it off by default.

D-6 ON (`pour_listed_via_goals: true`) on wiccon: 62/62, vias 36 (-1), outside 1 381 414 333 nm (+7.86 mm, over the 1 380 000 000 gate bound), GND J2.2 pour connection 13.9 mm without a via instead of 6.0 mm with one. Hence default off (budgets are not relaxed).

## Tests
- `cargo test --release --manifest-path engine/Cargo.toml`: all green (incl. new tests above).
- `cargo clippy --all-targets -- -D warnings`: clean. (`cargo fmt --check` is not clean at the base commit either; not reformatted to avoid conflicts with parallel workers.)
- pytest default suite with `PYTHONPATH=target/pyeng`: 454 passed, 7 skipped, 26 deselected (17:35 min).
- `PYTHONPATH=plugin python -m topor.docs_gen --check`: up to date.

## Concerns
- D-6 changes wiccon output when on (default off keeps byte-identical).
- Worktree-agnostic: `scratch` symlink at the worktree root is untracked (not committed).
- One `test_engine_surface.py::test_route_async_cancel_returns_anytime_result` failure seen once while a build and a bench ran concurrently (timing); passes alone and in the final full run.
