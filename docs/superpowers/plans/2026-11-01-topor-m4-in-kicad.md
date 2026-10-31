# Topor M4 In-KiCad Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Execution method (user, 45 §4, as in M1–M3):** rotating coordinator (O) dispatching one worker per task at the tier in the task title (H/S/F); independent tasks in parallel in isolated worktrees (R3: workers never touch the shared `.venv`); F only for the design notes, failed-gate diagnosis and the gate reviews. Every worker report is folded into `docs/progress/STATE.md` (≤ 5 lines); SDD ledger `.superpowers/sdd/2026-11-01-topor-m4-in-kicad/progress.md`; handover at a gate or at ~50 % context / ~15 round-trips. R1: work on main, **never push**.

**Goal:** Route eprom-to-kernal and wiccon-2023 from inside KiCad 10 through the IPC plugin — pre-flight report, progress dialog with Cancel, PNG preview with Accept/Reject, one undo step — configured by a self-documenting `topor.toml`; and bring the engine to the M4 hard numbers: dcsgonebadge (snapshot `3e7e66f862a7`) 231/231 routed, 0 router DRC, ≤ 10 min, locked edge GND kept (M3-R14), with eprom/wiccon not worse than M3.

**Architecture:** M4 is split into two milestones with separate gates that run **in parallel** on disjoint files. **M4a (engine, version 0.4.0)** finishes the router: slot-search speed (clearance-bend candidates), fine-pitch entry, own-net capacity and same-net Steiner attachment (T-junctions on routed and hand copper), pad exit stubs, G1 continuation of hand-started traces, escalating rip-up cost, the stop rule, cancel + progress, priority tiers, via rules from the board setup, via position relaxation, layer cost terms, a DRC-feasible detour reference, and dcsgone completion. **M4b (plugin)** adds a thin KiCad side behind two ports — `BoardPort` (IPC via kicad-python 0.8; a `FakeBoard` for headless tests) and `UiPort` (wx dialogs; `ScriptedUi` for tests) — with the run flow *pre-flight → route (progress, cancel) → DRC loop + preview on a temporary copy → Accept → one IPC commit*. The plugin ships the `harness/` package (board model export, writer, DRC, render) and the engine wheel in the plugin directory; `topor.toml` is generated from one settings schema that also generates the config reference, so docs cannot drift. **M4b-α** (Tasks B1, B2, B5, B8, B9) gives the user a GUI-testable whole-board route first; the remaining M4b tasks add scope, pre-flight depth, reports and docs.

**Tech Stack:** as M3 (Rust 1.96, spade 2.15.1, rayon 1.10, pyo3 0.23 abi3-py39, maturin in `.venv`, Python 3.12 + pcbnew SWIG for the harness, KiCad 10.0.8 `kicad-cli`, rsvg-convert, ImageMagick) plus, plugin side only: **kicad-python `>=0.8,<0.9`** (already pinned in `requirements.txt`), **wxPython** (present in KiCad's plugin venv on Linux, verified `import wx` in `~/.cache/kicad/10.0/python-environments/org.topor.router`), `tomllib` (Python ≥ 3.11 → `plugin.json` `min_version` becomes `"3.11"`). No new Rust crates.

**Spec:** `docs/design/45-mvp-plan.md` (§1 M4 row + gate, §3 M4 row, §5a, configurability + "usable without AI" rules); `docs/design/46-configuration.md` (§1 sources, §1.1 documentation requirement, §2 settings #1–#22 + engine/expert/fixed rows, §2.1 `@N` rules); `docs/kicad/30-kicad-constraints.md` §2–§4.2, §5; `docs/kicad/31-rust-engine-and-integration.md` §2.1, §3, §6, §6.1; `docs/progress/STATE.md` (M3 DONE header, M4 carry-over list, rulings M3-R13/R14/R15); `docs/progress/notes/m3-gate-review.md` (§8 fix shape, DEFERRED D-1..D-11); `docs/progress/notes/m3-detour-diagnosis.md` (M4-D1..D4); M3 plan `docs/superpowers/plans/2026-10-31-topor-m3-layers-vias.md` ("Deferred to M4" 1–8; names reused verbatim); `docs/topology/12-sketch-generation.md` §1, §3.1; `docs/geometry/25-relaxation-and-reflow.md` §3; `docs/geometry/22-smoothness-and-biarcs.md` (T-junctions, G1).

## Global Constraints

All M0–M3 Global Constraints and rulings still apply (Linux, `.venv` only, read-only `~/work/kicad/projects` — the harness reads boards **only** from `scratch/boards/<board>/<sha12>/` snapshots via `harness.paths.BENCH_BOARDS` and gate tests hash the snapshot dir; no KiCad types in the engine; KiCad-frame integer nm with one Y flip each way; `EPS_BUILD_NM` 5 000 / `margin_nm` 5 000; fab floors; length gate = outside-pad copper; R6/R6b/R8 zero-slack retry; M3-R8 `wall_clock_s` is the whole run incl. DRC rounds; compact ≤ 20-line summaries; conventional commits ending with the attribution line of the executing session's system reminder). In addition for M4:

- **Hard M4a gate (user, M3-R14):** dcsgonebadge snapshot `3e7e66f862a7`, `run_bench(router=route_router, rip="all", threads=4, options={"wall_clock_s": 600, "partial_routes": False})`: `routed == connections_total == 231`, `partial == failed == 0`, `routing_new == 0`, `unconnected_items == 0` after refill, `runtime_s ≤ 600` (whole bench run), the 22 locked edge-GND items kept with identical geometry and 0 DRC pairs with router copper (`tests/gate/test_locked_copper.py` stays green), threads 1/2/4 + repeat byte-identical. Pre-existing violations (4 clearance in hand copper) stay `preexisting`.
- **Not worse than M3 (user):** eprom 27/27, 25/25 nets, `routing_new 0`, `unconnected 0`, `length_outside_pads_nm ≤ 64_700_000` (M3: 64.628 mm) and `≤ hand`, `router_s < 2.0`, zero_slack 0 or DRC-kept; the M2 F.Cu-only guard `eprom_ripped_front_only_is_the_m2_result` (27/27, outside ≤ 67.8 mm) stays. wiccon 62/62 (signal 57/57), `vias ≤ 37`, `routing_new 0`, `unconnected 0`, `length_outside_pads_nm ≤ 1_380_000_000` (M3: 1373.553 mm), `router_s < 10.0`. Scaling `tests/scaling -m scaling` L1/L2 within the M2 ratio bounds (`t100/t10 ≤ 40`, `t1000/t100 ≤ 30`). Any task that breaks one of these numbers stops and reports; **budgets are never relaxed by a worker or coordinator** (45 §5a) — one F diagnosis round, then escalate to the user with numbers.
- **Complexity (45 §5a):** every new component states its Big-O in its task and in the code comment; no exponential/factorial stage, no order permutations, no all-pairs board loops; every loop has a cap, every long call checks the deadline **and the cancel flag**. Rip-up bound `O(max_per_pair · m²)`.
- **Determinism:** byte-identical result JSON (minus `runtime_s`) for `threads ∈ {1, 2, 4}` and across runs, for every board and every new option; no `HashMap` iteration order reaches the output.
- **Configurability (45):** every judgement call introduced in M4 is a `topor.toml` key with a sane default in the schema (`plugin/topor/schema.py`), surfaces in the run report when non-default, and maps 1:1 to an `Options` field or a harness/plugin field. Fixed constants stay fixed only for correctness (tolerances, cell sizes).
- **Sources (46 §1):** KiCad board setup wins → `topor.toml` in the project dir (created with all defaults + comments on first run, unknown keys warn, `version = 1`) → built-in defaults (listed in the report).
- **Messages (46 §1.1):** every pre-flight finding / error has a code in `plugin/topor/messages.py` with cause + fix naming the KiCad place or the `topor.toml` key; `docs/user/troubleshoot.md` has a section per code (tested).
- **Progress PNG (user):** every outcome-affecting task ends with `.venv/bin/python -m harness.progress --milestone m4 --task <TaskId> --router topor --board <board>` → `docs/benchmarks/progress/<YYYY-MM-DD>-m4-<TaskId>[-<board>].png` (hand left, topor right, both layers, caption with routed/total, vias, DRC, `router_s`, **detour line** `detour median/p90/max/>1.5` and **pcb sha12**), committed, path + one-line numbers sent to main. Never a test pattern as a result.
- **Versions:** engine 0.4.0 at the M4a gate (`engine/Cargo.toml`, `plugin.json`, `SHIPPED_VERSION`); the wheel is rebuilt (`VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release --manifest-path engine/Cargo.toml --features python`) before every pytest run that uses the engine; `bash tools/build.sh --dev` before the gates; `cargo clippy --all-targets --manifest-path engine/Cargo.toml -- -D warnings` clean; `.venv/bin/pytest -q` green.
- **Backlog, not M4:** 45° routing mode (`bend_90`, `bend_acute`), missing-width warning dialog (0.25 mm default), power-as-traces mode, fan-out vias for 4+ layers (46 §3). `[output] geometry` accepts `"arcs"` and `"segments"` only; `"45deg"` → finding `CFG_VALUE_BACKLOG`.

## Review Focus

1. **Cancel during a batch search or a completion pass** must return the best result so far within one deadline check interval (≤ 16 expansions / one connection), `stop_reason == "cancel"`, and never panic — the plugin process must not die mid-run. → `route_async_cancel_returns_anytime_result` (Task A8).
2. **Stale on-disk board** (user edited after the last save): the preview/DRC copy would route a different board than the one KiCad shows. Pre-flight must detect the id-set mismatch and stop with `BOARD_UNSAVED` before any routing. → `test_preflight_stale_file_blocks_run` (Task B4).
3. **Reject** after a routed preview must leave the live board untouched (no commit begun, no refill), and **Accept** must be exactly one undo step containing both removals and creations. → `test_run_reject_leaves_board_untouched`, `test_run_accept_single_commit` (Task B5).
4. **`@N` marker edge cases** — `DQ@1[0..7]` (index merges into the digit → ignored + warning), diff-pair halves with different markers (higher wins + warning), marker vs `topor_pN` netclass conflict (marker wins + warning), `~{RESET@2}`. → `test_marker_parse_table` (Task B6).
5. **Timeout disabled (`timeout_s = 0`) with a connection that can never route** must still terminate by the stop rule (cap / no gain), not spin. → `stop_rule_no_gain_terminates_without_timeout` (Task A8).

## File Structure

```
engine/
  Cargo.toml                   version 0.4.0 (A15)
  src/model.rs                 Options: iterations/stop rule, cancel-aware deadline, ripup, priorities, exclude_nets, ignore_pads,
                               via rules, layer costs, detour_reference, expert (A8, A9, A10, A11); Rules.predefined_vias (A10)
  src/control.rs               NEW: RunControl { cancel: Arc<AtomicBool>, progress: Arc<Mutex<Progress>> }, Progress (A8)
  src/py.rs                    route_async → RouteJob (progress/cancel/wait), default_options() (A8)
  src/route.rs                 stop rule + stop_reason, cancel checks, tiers from options, ignore_pads/exclude_nets, detour reference,
                               via relaxation pass, layer cost plumbing (A7, A8, A9, A11, A13); G1 start legs (A14)
  src/negotiate.rs             escalating per-pair rip cost, rip report (A16); stop rule inputs (A8)
  src/search.rs                face clear radius skip, memoised clearance_bend, early stop (A1); state_point/los fixes (A2);
                               same-net attach goals (A5); stub terminals (A6); layer cost terms (A11)
  src/graph.rs                 clear_r per face (A1); own-net / same-net capacity, wall-aware fits (A4)
  src/decompose.rs             tier from Options.net_priority, exclude_nets, ignore_pads, Steiner ordering (A5, A9)
  src/vias.rs                  via_sizes from predefined list + fallback, allowed_drills, via_in_pad sites (A10)
  src/relax.rs / smooth.rs     via position relaxation (A13); stub + G1 constraints in metrization (A6, A14)
  src/pourcheck.rs             D-9 scanline stamps + cell budget (A0)
  src/multigraph.rs            D-4 in_pour holes, D-10 BTreeSet free list (A0)
  tests/search_speed.rs, search_entry.rs, graph_ownnet.rs, route_steiner.rs, route_stubs.rs, route_g1.rs, route_control.rs,
        negotiate_ripup.rs, route_priority.rs, via_rules.rs, route_layer_cost.rs, route_detour_ref.rs, route_via_relax.rs
docs/progress/notes/m4-capacity-steiner-design.md   F note (A3): D1, D3/Steiner, exit stubs, G1, via relaxation, escalating rip cost
docs/progress/notes/m4-dcsgone-diagnosis.md         F diagnosis if A12 misses the gate
docs/progress/notes/m4a-gate-review.md, m4b-gate-review.md   F gate reviews (A15, B11)
harness/
  drcloop.py                   NEW: route_with_drc_loop() extracted from bench.run_bench (H0)
  bench.py                     uses drcloop; columns detour (ref) / detour_ghost / stop_reason / vias per net vs hand (H0, A7)
  progress.py                  --milestone m4 naming; caption detour + sha (H0)
  metrics.py                   detour_stats(ref_field="ref_len_nm"), detour_ghost (A7)
  rules.py                     resolve_rules split: resolve_rules_from_project(pro: dict, dru_text, netclass_of, widths_before, …) pure (B3)
  model.py                     keepouts: per-layer footprint rule areas already exported; `disallow` dru keepouts (H0)
  snapshot.py                  D-8 (A0)
plugin/
  plugin.json                  version 0.4.0, min_version 3.11, action "route" (B9)
  topor_plugin.py              entry: version handshake, sys.path, topor.app.main() (B9)
  topor/schema.py              SETTINGS table + KICAD_ONLY comment entries (B1)
  topor/config.py              load_config, write_default, render_toml, update_config (B1)
  topor/docs_gen.py            render_reference_md, `--check` (B1)
  topor/ports.py               BoardPort, UiPort protocols; Selection, Finding, Progress, CommitRequest, Decision (B2)
  topor/fake.py                FakeBoard (fixture .kicad_pcb via harness), ScriptedUi (B2)
  topor/ipc.py                 IpcBoard over kipy: ids, selection, commit, refill, run_action, read_model (file source) (B2, B3)
  topor/priority.py            parse_marker, net_priorities (B6)
  topor/scope.py               scope_from_selection, rip_plan (B6)
  topor/preflight.py           preflight() → findings (B4)
  topor/messages.py            MESSAGES: code → (cause, fix) (B4)
  topor/preview.py             temp copy, rip, write, DRC loop, PNG compare (B5)
  topor/report.py              build_report, write_report (B7)
  topor/run.py                 run(): the flow (B5)
  topor/ui_wx.py               WxUi (B8)
  topor/app.py                 main(): KiCad client, project dir, config, run (B9)
tools/build.sh                 ships plugin/topor/, harness/, docs/user/ (B9)
docs/user/{install,configuration,operate,troubleshoot,gui-test-checklist}.md, docs/dev/debug.md, CONTRIBUTING.md (B10)
tests/plugin/test_{config,config_docs,fake_board,ipc_mapping,preflight,priority,scope,run_flow,report,messages}.py
tests/gate/test_m4_gate.py     M4a numbers (A15); tests/gate/test_m4b_gate.py headless e2e (B11)
docs/benchmarks/<date>-topor-0.4.0/{README.md, images/, metrics/}   (A15)
```

**Dependencies and parallel groups.** M4a and M4b run concurrently (engine/harness vs plugin files).
- M4a group 1 (parallel worktrees): **A0** · **H0** · **A1** (search.rs, graph.rs) · **A3** (F note) · **A8** (model.rs, control.rs, py.rs, route.rs stop/cancel). A8 ← nothing; A1 ∥ A8 touch `route.rs` in different functions (merge A1 first).
- Group 2: **A2** ← A1 (search.rs) · **A7** ← H0, A8 · **A9** ← A8 · **A16** ← A3, A8 (negotiate.rs).
- Group 3: **A4** ← A3, A2 (graph.rs/search.rs) · **A10** ← A9 (model.rs, vias.rs) · **A11** ← A9 (search cost, route.rs).
- Group 4: **A5** ← A4 · **A6** ← A4 · **A14** ← A3, A6 (wires/relax) · **A13** ← A3, A10 (relax).
- Group 5: **A12** ← A1–A6, A16 (dcsgone completion, F diagnosis if needed) → **A15** gate.
- M4b: **B1** ∥ **B2** (B1's engine-field test needs A8's `default_options`; until then it runs against the names in this plan with `xfail(strict=False)`) → **B5** ← B1, B2, H0, A8 · **B3** ← B2 · **B4** ← B1, B2 · **B6** ← B2 · **B7** ← B5 → **B8** ← B5 → **B9** ← B8 (M4b-α: B1, B2, B5, B8, B9 first) → **B10** ← all → **B11**.

**Model tiers:** A0 S (D-5/D-7/D-8/D-10 steps H) · H0 S · A1 S (F review at the gate) · A2 S (F review) · A3 **F** · A4 S · A5 S · A6 S · A7 S · A8 S · A9 S · A10 S · A11 S · A12 S + **F** diagnosis if the gate misses · A13 S · A14 S · A15 S + **F** gate review · A16 S (design check **F** inside A3) · B1 S (doc text H) · B2 S · B3 S · B4 S · B5 S · B6 S · B7 S · B8 S (dialog layout H) · B9 S · B10 S (H for mechanical sections) · B11 S + **F** gate review. Any task failing its acceptance twice → F diagnosis note (45 §5).

---

## M4a — engine (version 0.4.0)

### Task A0 (S; H sub-steps): M3 gate-review minors D-1..D-10 — `multigraph.rs`, `pourcheck.rs`, `route.rs`, `negotiate.rs`, tests, `harness/snapshot.py`

**Files:** Modify `engine/src/multigraph.rs:210` (D-4), `:381-387` (D-10); `engine/src/pourcheck.rs:186-195` (D-9); `engine/src/search.rs` (`push_state`, D-1 toggle); `engine/src/route.rs:2730` (D-6); `engine/tests/route_engine.rs:740-776` (D-2), `engine/tests/search_layers.rs:636` (D-3), `engine/tests/route_layers.rs:562` (D-5); `docs/progress/notes/m3-layers-vias-design.md` §7 (D-7); `harness/snapshot.py:33-37` (D-8); test `tests/test_snapshots.py`.

**Interfaces:**
- Produces: `Options.strict_unfit_edges: bool` (default `false`; D-1: `!fits(e)` rejects in `push_state` instead of overuse pricing); `MultiGraph::in_pour` honours `PourFill.holes`; `alloc_legs` free list is a `BTreeSet<usize>`; `pourcheck` stamps per scanline row with a per-run cell budget `STAMP_CELL_BUDGET = 16_000_000` and a deadline check per pour.

- [ ] **Step 1 (D-4):** test `site_inside_fill_hole_is_not_in_pour` in `engine/tests/multigraph.rs`: a pour with one hole; a site at the hole centre has `in_pour[s]` empty; outside the hole it contains the pour. Run → FAIL. Implement (point-in-polygon on `outer` minus `holes`, `O(|outer| + Σ|holes|)` per site). PASS.
- [ ] **Step 2 (D-9):** test `pourcheck_long_diagonal_track_stamps_linear` in `engine/tests/pourcheck.rs` (new file if absent): one 100 mm diagonal 0.2 mm track; assert `stamped_cells ≤ 4 · L/res · w/res + 64` (expose a counter behind `#[cfg(test)]`/`pub fn last_stamp_cells()`). Implement scanline per row (`O(Σ rows · span)`), budget + deadline (`Instant` from `with_deadline`). PASS.
- [ ] **Step 3 (D-10, H):** replace `free_ids.sort(); remove(0)` with `BTreeSet::pop_first`; existing `multigraph` tests green.
- [ ] **Step 4 (D-1):** add `Options.strict_unfit_edges` + test `strict_unfit_edge_rejected_in_search` (`engine/tests/search_sleeve.rs`: the `statically_unfit_edge_is_hard` fixture, strict on → `Blocked`, off → routed with overuse). Run two-layer eprom once with `strict_unfit_edges: true` via `route_probe`, record numbers in the ledger (decision for A4 input; no gate change).
- [ ] **Step 5 (D-2, D-3, D-5, H):** D-2: fixture where the split-vertex bend still fails → assert `status == "failed"` and reason starts `"split vertex"`; D-3: e2e fixture `rule_t_via_counter_example` in `route_layers.rs` (two nets, one site, the strand hand-inserted today becomes a routed connection); D-5: assert the emitted warning text `"without pour after"`.
- [ ] **Step 6 (D-6):** test `pour_connection_uses_listed_same_net_via_as_goal` (`route_layers.rs`): a pour listing a free same-net via that the pad can reach by track → routed with 0 new vias, `to.kind == "via"`. Implement goal vertices for `Pour.vias` on `L_pour`. PASS.
- [ ] **Step 7 (D-7, H):** note §7 amendment "phase-2 ghost precompute and `usable` reuse" (≤ 10 lines). **(D-8)** `snapshot_board` verifies the source sha only on creation; test `test_snapshot_existing_pin_survives_source_edit`.
- [ ] **Step 8:** `cargo test --release`, clippy, `.venv/bin/pytest -q` green; eprom/wiccon gate tests byte-identical to M3 (`tests/gate/test_m3_gate.py -k "eprom or wiccon"`). Commit `fix(engine): M3 gate-review minors D-1..D-10`.

### Task H0 (S): harness preparation — DRC loop extraction, M4 progress naming, keepout `disallow` rules, bench columns

**Files:** Create `harness/drcloop.py`; modify `harness/bench.py:262-420` (call the extracted loop), `harness/progress.py` (`--milestone`, `_render_progress_m3` → `_render_progress_topor(milestone)`), `harness/rules.py` (`disallow` rule → keepout), `harness/model.py` (`_export_keepouts` adds dru keepouts), `harness/metrics.py`; tests `tests/test_drcloop.py`, `tests/test_detour.py`, `tests/test_rules.py`, `tests/test_router.py`.

**Interfaces:**
- Produces: `harness.drcloop.route_with_drc_loop(model: dict, options: dict, cp: ProjectCopy, *, router, base_drc, orig_drc, out_dir, board, budget_s) -> LoopResult` (dataclass: `router_report, items, created, routed_drc, rounds, pinned, spots, zero_slack, budget_exhausted, router_s`) — the exact loop now inside `run_bench` (DRC spots, zero-slack judge, M3-R8 remaining budget). `run_bench` output dict unchanged. `render_progress(task, *, milestone="m4", ...)` → `<date>-<milestone>-<task>[-<board>].png`, caption = `m3_caption` + `stop_reason` when present. `parse_dru` recognises `(rule … (condition "A.intersectsArea('NAME')") (constraint disallow track|via))` → keepout entry `{name, layers (from (layer …) or all copper), no_tracks, no_vias}` merged by `export_model`.

- [ ] **Step 1:** test `test_drcloop_matches_run_bench_on_fixture` (fixture board, `route_router`): `run_bench(...)["routed_drc"]`/`["drc_loop"]` equal before/after the refactor (pin the pre-refactor JSON from a run at this commit into the test as expected values). Extract. PASS.
- [ ] **Step 2:** test `test_progress_m4_name_and_caption_lines` (`tests/test_detour.py` style): `render_progress("X", milestone="m4", ...)` on the fixture names `…-m4-X.png`, caption contains `detour` line and `pcb <sha12>`. Implement. PASS.
- [ ] **Step 3:** test `test_dru_disallow_rule_becomes_keepout` (`tests/test_rules.py`): dru text with `disallow via` on area `NOVIA` layer `F.Cu` → keepout `{"no_vias": True, "no_tracks": False, "layers": [0]}`; unknown area name → warning `"disallow: area NOVIA not found"`. Implement. PASS.
- [ ] **Step 4:** bench summary/README columns: `detour` (reference), `detour_ghost`, `stop_reason`, `vias_per_net` (router vs hand per net, top 5 diff) — fields read from the report when present, `None` otherwise (A7/A8 fill them). `.venv/bin/pytest -q` green. Commit `feat(harness): drcloop extraction, m4 progress naming, disallow keepouts, bench columns`.

### Task A1 (S, F review at the gate): slot-search speed — face clear radius, memoised `clearance_bend`, early stop — `engine/src/search.rs`, `engine/src/graph.rs`

**Files:** Modify `engine/src/graph.rs` (`RouteGraph::build`: `clear_r: Vec<f64>` per face), `engine/src/search.rs:1480-1560` (`clearance_bend`, `cached_near`), `:1758` (`los`); test `engine/tests/search_speed.rs`.

**Interfaces:**
- Produces: `RouteGraph.clear_r[f]` = distance from face `f`'s incentre to the nearest vertex not of `f` and nearest wall (one grid query per face at build, `O(F · q)`); `Search::clearance_bend` (a) skips the candidate scan for a sub-chord between consecutive portals whose both endpoints lie inside `D(incentre(f), clear_r[f] − ρ)` (then only the face's own 3 vertices are tested exactly — the disk is convex, so every point of the sub-chord is ≥ ρ from every non-face obstacle), (b) memoises its result per search on the key `(portal edge, side, apex vertex id, target vertex id)` in a `RefCell<HashMap>` (membership only; never iterated), (c) tests candidates in ascending chord parameter and stops at the first violation (the `offer` minimum is unchanged because lists are ascending supersets). Per call `O((C + 1) · hit)` with `hit` the first-violation prefix. Counters in `search_totals()`: `bends`, `candidates`, `skipped_faces`, `memo_hits`.

- [ ] **Step 1:** test `clear_radius_skip_is_conservative` (`search_speed.rs`, proptest 200 cases): random face + random sub-chord inside the disk ⇒ exact scan finds no bend from non-face obstacles; outside the disk the skip is not taken. Run → FAIL (`clear_r` missing). Implement (a). PASS.
- [ ] **Step 2:** test `memoised_bend_equals_recomputed` and `early_stop_bend_equals_full_scan` (fixtures from `search_sleeve.rs`; compare `probe_clearance_bend` with and without `TOPOR_SEARCH_NOMEMO` cfg flag, byte-equal apex). Implement (b), (c). PASS.
- [ ] **Step 3:** measure on dcsgone: `cargo run --release --example route_probe -- scratch/boards/dcsgonebadge/3e7e66f862a7/model.json --options '{"wall_clock_s":60,"partial_routes":false}' --profile` → record `candidates/query` (was 162) and search share (was 94 %) in the ledger; target `≥ 5×` fewer candidates. If < 2×, stop and report (no further knobs).
- [ ] **Step 4:** eprom and wiccon results byte-identical to M3 (`tests/gate/test_m3_gate.py -k "eprom or wiccon"` + `route_threads` tests); dcsgone 60 s run routed ≥ 148. Progress PNG `--task A1 --board dcsgonebadge`. Commit `perf(search): face clear radius, memoised clearance_bend, early stop`.

### Task A2 (S, F review at the gate): fine-pitch entry #201 and state-point / los convergence (M4-D2) — `engine/src/search.rs`

**Files:** Modify `engine/src/search.rs:1373` (`state_point`), `:1758` (`los`), terminal entry at roundrect corners; test `engine/tests/search_entry.rs`.

**Interfaces:**
- Produces: `state_point(e, apex, goal)` shrinks the usable sub-segment of edge `e` by every foreign vertex within `r_v + c + w/2 + eps` of the edge (`verts_near` query, `O(q)`), and at roundrect corner anchors discretises the crossing at the tangent point of the corner circle rather than the pad corner; `los` returns `ok = false` when the bend loop does not converge (apex not approaching the target within `LOS_PORTAL_CAP` or cap hit) instead of accepting accumulated length. `SearchParams` unchanged.

- [ ] **Step 1:** test `fine_pitch_entry_u7_4_routes_direct` (`search_entry.rs`): fixture = dcsgone U1.13→U7.4 geometry (0.5 mm pitch roundrect pads from the model JSON, extracted into a fixture by `gate_diag --detour 201` numbers) → search on the empty graph routes with 0 vias and `est_len ≤ 1.5 × straight distance`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** test `los_nonconverging_bend_loop_is_rejected` (the #208 USB+ J1.A6→J1.B6 pad trio): search alone finds a 0-via path `≤ 1.3 × hand 2.93 mm`; `apex_len` never exceeds `4 × chord`. Implement. PASS.
- [ ] **Step 3:** eprom byte-identical or, if the eprom result changes, outside-pad length not worse and all gate numbers hold (report the diff); wiccon not worse. Progress PNG `--task A2 --board dcsgonebadge` (detour max expected < 3.49). Commit `fix(search): fine-pitch entry, state point shrink, los convergence`.

### Task A3 (F): design note — own-net capacity, same-net Steiner attachment, exit stubs, G1 continuation, via relaxation, escalating rip cost — `docs/progress/notes/m4-capacity-steiner-design.md`

**Files:** Create the note (≤ 3 pages). Read: `m3-detour-diagnosis.md` (M4-D1, D3 and the withdrawn F6), `graph.rs:462-500` (`fits`, `fits_at`, `overuse`), `wires.rs` (junction terminals, T-split), `relax.rs`, `negotiate.rs:36-58, 470-495`, `docs/geometry/22 §T-junctions`, `25 §3`, `12 §1`.

**Interfaces:**
- Produces the rules Tasks A4, A5, A6, A13, A14, A16 implement, fixing: (1) **own-net / same-net capacity (D1), wall-aware:** own-net ends and same-net neighbours charge no gap and give the core radius back; an edge whose end is own-net additionally respects walls that are not edge ends via a per-edge wall query cached at build (`O(E · q)`), so the eprom U2.28 outline-wall case (F6 revert) stays unfit; exact formula for `cap`/`need` per net class of the crossing strand; which tests from the diagnosis flip. (2) **Same-net attachment (D3 / Steiner):** a connection may terminate on same-net copper — committed legs of earlier connections of the net and kept hand copper — as a T-junction: goal = the strand's slot on the crossed edge (`Goal::OwnStrand(conn, leg, edge)`), accepted only if the junction angle ≥ 60°; the sibling wire is split at the junction into two wires sharing a `Terminal::Junction` (as `wires.rs` does for hand copper); MST ordering so a pad is left at most once where a shorter total results (pick per net between star and path by est length, `O(p log p)`); the 120° Steiner relaxation of `25 §3` as a bounded relax step (junction moves along the sibling, ≤ `relax_iter_cap`). (3) **Pad exit stubs:** terminal kind `Terminal::Stub { from: pad, to: point, layer }` — a straight stub of the net width from the pad centre to `hull + c + w/2 + eps` along the pad's long axis away from the nearest unfit neighbour, generated for SMD pads whose own tip/spoke edges are unfit at the net width (`exit_stubs = "auto"`), always (`"all"`) or never (`"off"`); the stub is fixed copper of the connection (counts in length), its far end the search terminal; DRC-feasibility checked by the `Checker` before use (else no stub, reason reported). (4) **G1 continuation of dangling ends:** the first leg from a `Terminal::Dangling { at, direction }` leaves tangent to `direction`: metrization adds a virtual anchor pair ("rails") of radius `w/2 + eps` at `at ± n̂·(w/2 + c)` for the first `STUB_LEN = max(2 w, 0.5 mm)` so the taut chain exits straight, then bends; the search treats the hand trace's crossing sequence as a topology hint (its sketch sides are fixed for the sleeve edges it crosses); `g1_angle_deg` reported = angle between `direction` and the leg's exit tangent (target ≤ 1°). (5) **Via position relaxation (M3-R3, Weiszfeld, bounded):** after via reduction, each via moves toward the force balance of its two legs (`25 §3`) in ≤ `VIA_RELAX_STEPS = 8` steps, each step confined to the union of faces incident to the site vertex on both layers (so the slot topology is unchanged), accepted iff `via_fits` (hole-to-hole, keepouts, pours) and the two legs re-metrize and self-check clean and the connection length decreases; `O(V_via · steps · (metrize + q))`. (6) **Escalating per-pair rip cost (coordinator, 46 row 17):** the k-th rip of blocker B by connection A adds `HIST_BLOCK · growth^(k−1)` to B's history (`growth = 2.0`), hard backstop `max_per_pair = 8`; the note checks the history-cost scale against length costs (`HIST_*` are per-edge multipliers of the present cost: show the geometric sum `HIST_BLOCK · (growth^k − 1)/(growth − 1)` stays below `HIST_FAIL · k` for k ≤ 2 and above it for k ≥ 3, so a three-way contention still settles (the F.Cu-only eprom case) and ping-pong pairs are priced out before the backstop); rip bound `O(max_per_pair · m²)`; report fields. (7) Complexity table per 45 §5a for (1)–(6) and the test per rule (names from Tasks A4–A6, A13, A14, A16).

- [ ] **Step 1:** write the note; **Step 2:** commit `docs(design): M4 capacity/Steiner/stubs/G1/via-relax/rip-cost note`. Send the note path to main; rulings go to the ledger.

### Task A4 (S): own-net / same-net capacity, wall-aware (M4-D1) — `engine/src/graph.rs`, `engine/src/search.rs`

**Files:** Modify `engine/src/graph.rs:203` (`build`: per-edge wall distance `wall_r[e]`), `:462-500` (`fits`, `fits_at`, `overuse` with net-aware gaps); `engine/src/search.rs` (`leave_vertex`, ghost softness); tests `engine/tests/graph_ownnet.rs`.

**Interfaces:**
- Consumes: note §1. Produces: `RouteGraph::fits_for(e, net, w, c) -> bool` (replaces `fits(e, w, c)` at call sites that know the net) and `RouteGraph.wall_r: Vec<f64>`; `need(e, net)` charges 0 gap and gives back the core radius at own-net ends; same-net strands already on `e` charge 0 gap to the new strand. Build cost `O(E · q)`.

- [ ] **Step 1:** tests (from the note): `own_tip_edge_fits_own_net` (0.6 × 0.3 rect pad, 0.5 mm track: `fits_for(tip, own) == true`, foreign false), `two_same_net_strands_share_one_tip_edge`, `own_end_edge_against_outline_wall_stays_unfit` (the U2.28 case), `search_layers (1)` flips to "fits, ghost not soft". Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `cargo test --release route_engine` incl. `eprom_ripped_front_only_is_the_m2_result` (27/27, outside ≤ 67.8); eprom two-layer identity re-verified (report if changed, numbers not worse); wiccon not worse (expected vias 37 → ≤ 33 per the F6 measurement). Progress PNG `--task A4 --board dcsgonebadge` and `--board wiccon-2023`. Commit `feat(graph): own-net and same-net capacity, wall-aware`.

### Task A5 (S): same-net Steiner attachment and net ordering (M4-D3) — `decompose.rs`, `search.rs`, `route.rs`, `wires.rs`

**Files:** Modify `engine/src/decompose.rs:254` (`decompose`: star/path choice per net), `engine/src/search.rs` (`Goal::OwnStrand`), `engine/src/route.rs` (junction split of the sibling wire, report `to.kind == "copper"` with `junction: true`), `engine/src/wires.rs`; tests `engine/tests/route_steiner.rs`.

**Interfaces:**
- Consumes: note §2, `fits_for` (A4). Produces: `Goal::OwnStrand { conn, leg, edge }` in `search::Goal`; `ConnReport.junction: bool`; `Options.steiner: bool` (default `true`; `false` = M3 behaviour). Cost: one extra goal set per same-net sibling, `O(Σ sibling leg length)` per search.

- [ ] **Step 1:** tests: `t_junction_onto_routed_sibling` (U7.6→J1.A6 fixture from #207: routed as a ≤ 1.2 mm T-junction, 0 vias, `junction == true`, sibling split into two wires, self-check clean), `t_junction_onto_hand_copper` (kept same-net track between the pads: attaches, `to.kind == "copper"`), `steiner_off_is_m3_behaviour` (byte-identical to the M3 result on the fixture), `junction_angle_below_60_is_rejected`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** eprom/wiccon not worse; threads 1/2/4 byte-identical; dcsgone 60 s: `count > 1.5` detours reported (expected ≤ 6, was 12). Progress PNG `--task A5 --board dcsgonebadge`. Commit `feat(route): same-net Steiner T-junctions and net ordering`.

### Task A6 (S): pad exit stubs — `engine/src/route.rs`, `engine/src/wires.rs`, `engine/src/decompose.rs`

**Files:** Modify `engine/src/wires.rs` (`Terminal::Stub`), `engine/src/decompose.rs` (`TermRef.stub`), `engine/src/route.rs` (stub generation before the search, stub items emitted first, `ConnReport.stub_nm`), `engine/src/model.rs` (`Options.exit_stubs: String` = `"auto" | "all" | "off"`, default `"auto"`); tests `engine/tests/route_stubs.rs`.

**Interfaces:**
- Consumes: note §3, `fits_for` (A4), `Checker`. Produces: `Terminal::Stub { pad: Vec2, end: Vec2, layer: u32 }`; `ConnReport.stub_nm: i64`. Generation `O(T · q)`.

- [ ] **Step 1:** tests: `stub_generated_for_unfit_fine_pitch_pad` (0.25 × 0.625 pads at 0.5 mm pitch, `exit_stubs: "auto"` → stub length `hull + c + w/2 + eps` along the long axis, items start with the stub segment, DRC-clean via `Checker`), `stub_skipped_when_infeasible` (neighbour pad blocks the stub → no stub, reason `"stub infeasible"`), `exit_stubs_off_matches_previous`, `exit_stubs_all_on_every_smd_pad`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** eprom (THT only) byte-identical; wiccon not worse; dcsgone #201 routed. Progress PNG `--task A6 --board dcsgonebadge`. Commit `feat(route): pad exit stubs (auto/all/off)`.

### Task A7 (S): DRC-feasible detour reference (M4-D4, M3-R15) — `engine/src/route.rs`, `harness/metrics.py`, `harness/bench.py`

**Files:** Modify `engine/src/route.rs` (`detour_reference(model, conns, layers, opts) -> Vec<Option<i64>>`), `engine/src/model.rs` (`Options.detour_reference: bool` default `false`, `Options.detour_reference_budget_pct: f64` default `10.0`), `ConnReport.ref_len_nm: Option<i64>`, `ConnReport.ref_kind: String` (`"feasible" | "ghost" | "none"`); `harness/metrics.py` (`detour_stats(connections, field="ref_len_nm")`, `detour_ghost` from `ghost_len_nm`), `harness/bench.py` (`detour`, `detour_ghost` columns), `harness/progress.py` (caption uses `detour` and flags `ghost < 2 mm`); tests `engine/tests/route_detour_ref.rs`, `tests/test_detour.py`.

**Interfaces:**
- Produces: per connection one slot search with full clearance/width on a `MultiGraph` built from fixed copper only (kept/locked/pre-existing, no other connections), same layers, via rules and `via_cost`; run in parallel (`par_iter`, collected in order) under `detour_reference_budget_pct` of the wall clock; failure → `ref_len_nm = ghost_len_nm`, `ref_kind = "ghost"`. Cost `O(m · search)`.

- [ ] **Step 1:** tests: `reference_is_feasible_not_topological` (#208 trio: `ref_len_nm ≈ 2.9 mm`, ghost 1.0 mm), `reference_off_leaves_fields_none`, `reference_budget_exhausted_falls_back_to_ghost` (`wall_clock_s: 0.001` → `ref_kind == "ghost"` for every connection, run still completes), `reference_threads_byte_identical`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `test_detour_stats_uses_reference_and_reports_ghost` (`tests/test_detour.py`): stats carry `detour` (ref) and `detour_ghost`; `detour_line` prints the reference numbers. Bench sets `detour_reference: true` (bench/offline only). PASS.
- [ ] **Step 3:** Progress PNG `--task A7 --board dcsgonebadge` (caption now vs reference). Commit `feat(route): DRC-feasible detour reference (M3-R15)`.

### Task A8 (S): run control — stop rule, timeout disable, cancel + progress (`route_async`), expert/ripup/search options, `default_options` — `engine/src/control.rs`, `model.rs`, `route.rs`, `negotiate.rs`, `py.rs`

**Files:** Create `engine/src/control.rs`; modify `engine/src/model.rs:632-687` (`Options`), `engine/src/route.rs` (stop rule, `stop_reason`, cancel checks at every deadline check), `engine/src/negotiate.rs` (constants → options), `engine/src/smooth.rs:216` (`deadline_after(0.0)` = far future when `wall_clock_s == 0`), `engine/src/py.rs`; tests `engine/tests/route_control.rs`, `tests/test_engine_surface.py`.

**Interfaces:**
- Produces (all serde `default`, names are the `topor.toml` keys of B1 minus the section):
  `Options.pathfinder_iter_cap` default **30** (46 #17b), `patience: u32 = 5`, `min_gain_pct: f64 = 1.0`, `wall_clock_s: f64 = 60.0` (**`0.0` = disabled**), `detour_factor: f64 = 2.0`, `ripup_growth: f64 = 2.0`, `ripup_max_per_pair: u32 = 8` (A16 consumes), `expert: Expert { hist_fail: 1.0, hist_block: 0.5, hist_overuse: 0.5, present_overuse: 4.0, zero_slack_candidates: 4 }`.
  `RouteResult.stop_reason: String` ∈ `{"converged", "cap", "no gain", "timeout", "cancel", "stuck"}` (replaces the `STALL_ITERS = 8` stall stop: stop when over the last `patience` iterations no connection was newly routed **and** the best cost `Σ length + via_cost · vias` improved by `< min_gain_pct` in total).
  `control::RunControl { cancel: Arc<AtomicBool>, progress: Arc<Mutex<Progress>> }`, `Progress { phase: String, iteration: u32, routed: usize, total: usize, vias: usize, elapsed_s: f64 }` updated per iteration and per completion/reduction pass; `route_with(model, opts, ctl: &RunControl) -> RouteResult`; `route()` = `route_with(…, &RunControl::default())`. `Ctx::stop(&self) -> bool` = deadline passed **or** cancel set; every existing `Instant::now() >= deadline` site calls it (O(1)).
  PyO3: `route_async(model_json, options_json) -> RouteJob` (`#[pyclass]`: `progress() -> str` JSON, `cancel()`, `done() -> bool`, `wait() -> str` result JSON; the job runs on a `std::thread`, `wait` releases the GIL); `default_options() -> str` (JSON of `Options::default()`); `route()` unchanged.

- [ ] **Step 1:** tests (`route_control.rs`): `stop_rule_no_gain_terminates_without_timeout` (unroutable fixture, `wall_clock_s: 0.0` → ends with `stop_reason == "no gain"` in ≤ `pathfinder_iter_cap` iterations), `stop_rule_counts_newly_routed_as_progress` (a fixture that routes one more connection every iteration for 8 iterations with < 1 % gain must not stop early), `route_async_cancel_returns_anytime_result` (dcsgone model, cancel after 2 s → `stop_reason == "cancel"`, `routed ≥ 1`, items self-checked, no panic), `progress_is_monotone` (iteration non-decreasing, `routed ≤ total`), `timeout_zero_is_disabled` (`deadline` ≥ now + 1 year). Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `tests/test_engine_surface.py`: `test_default_options_lists_every_field` (keys of `json.loads(engine.default_options())` ⊇ the names above), `test_route_async_roundtrip_fixture` (`wait()` JSON == `route()` JSON minus `runtime_s`).
- [ ] **Step 3:** eprom (1 iteration) byte-identical; wiccon with cap 30 / patience 5: 62/62 and `iterations ≤ 30` (report if the stop rule changes the result; not worse); scaling tests green. Progress PNG `--task A8 --board wiccon-2023`. Commit `feat(engine): stop rule, cancel + progress, route_async, options for every 46 knob`.

### Task A9 (S): priority tiers, do-not-touch nets, ignored pads — `decompose.rs`, `model.rs`, `route.rs`

**Files:** Modify `engine/src/model.rs` (`Options.net_priority: BTreeMap<String, u8>`, `exclude_nets: Vec<String>`, `ignore_pads: Vec<PadRef>`), `engine/src/decompose.rs:495-530` (tier), `engine/src/route.rs` (excluded nets' copper → fixed obstacles, `NetRouteReport.status == "excluded"`; ignored pads → no terminal, no obstacle, `ConnReport.status == "ignored"`, reason `"pad outside board"`); tests `engine/tests/route_priority.rs`.

**Interfaces:**
- Produces: `Connection.tier` = `opts.net_priority[net name]` if present (1..4), else M3 inference (2 if wider than default, else 3); `order()` unchanged (tier, wide first, short first); `tier_weight`/`via_weight` apply to tier 4. Excluded nets: never decomposed, their `kept` items locked. Plugin-side resolution of `@N`/netclass/power pattern produces `net_priority` (B6).

- [ ] **Step 1:** tests: `priority_table_orders_and_weights` (two contended nets; `@1` net routes straight, tier-4 net detours; swapping tiers swaps the outcome), `tier_4_pays_lower_via_cost`, `excluded_net_copper_is_fixed_obstacle` (`status == "excluded"`, other nets route around it), `ignored_pad_skipped_and_reported`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** eprom/wiccon byte-identical (no options set). Commit `feat(route): priority tiers from options, do-not-touch nets, ignored pads`.

### Task A10 (S): via rules from the board setup — predefined vias, fallback 0.8/0.4, `allowed_drills`, via-in-pad, per-net via cost — `vias.rs`, `model.rs`, `route.rs`, `harness/rules.py`

**Files:** Modify `engine/src/model.rs` (`Rules.predefined_vias: Vec<[i64; 2]>` `(d, drill)`, `Rules.via_fallback: [i64; 2]` default `[800_000, 400_000]`, `Options.allowed_drills_nm: Vec<i64>` (empty = no filter), `Options.via_in_pad: bool = false`, `Options.via_cost_by_net: BTreeMap<String, i64>`, `Options.reduction_len_slack_pct: f64 = 10.0`); `engine/src/vias.rs:32` (`via_sizes`), `:95-200` (sites inside own-net SMD pads when `via_in_pad`); `engine/src/route.rs:1745-1857` (reduction slack from options; `RouteResult.vias_in_pads: Vec<[i64;2]>`); `harness/rules.py` (`m_ViasDimensionsList` → `predefined_vias`, KiCad factory-default detection → `warnings += "via sizes not set up"`); tests `engine/tests/via_rules.rs`, `tests/test_rules.py`.

**Interfaces:**
- Produces: `via_sizes(rules, net, opts)`: the net's netclass via first, then every smaller predefined via (filtered by `allowed_drills_nm`, `≥ min_via_drill`), fallback `via_fallback` with `warnings += "via fallback 0.8/0.4 used"` only when the predefined list is empty **and** the netclass values are KiCad factory defaults (0.6/0.3 with `netclass == "Default"` and `width_source == "default"`); `RouteResult.vias_in_pads` lists via positions inside SMD pads. Costs unchanged (`O(|sizes| log)`).

- [ ] **Step 1:** tests: `via_sizes_prefer_netclass_then_predefined_smaller`, `allowed_drills_filters_predefined`, `fallback_only_when_nothing_set_up` (warning text asserted), `via_in_pad_site_inside_own_smd_pad_only_when_enabled` (listed in `vias_in_pads`), `reduction_slack_from_options`, `via_cost_by_net_overrides_default`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `test_rules_exports_predefined_vias_and_factory_default_warning` (`tests/test_rules.py`, fixture `.kicad_pro` with/without `m_ViasDimensionsList`). PASS.
- [ ] **Step 3:** eprom/wiccon byte-identical (JLCPCB profile values unchanged). Commit `feat(vias): predefined via list, fallback warning, allowed_drills, via-in-pad, per-net via cost`.

### Task A11 (S): layer cost terms — pour-aware layer cost and preferred direction — `search.rs`, `route.rs`, `model.rs`

**Files:** Modify `engine/src/model.rs` (`Options.pour_cut_cost_nm: i64 = 1_000_000` per mm of leg inside a foreign pour, `Options.layer_direction: BTreeMap<String, f64>` layer name → degrees, `Options.direction_cost: f64 = 0.0` extra fraction per mm for legs ≥ 45° off the preferred direction), `engine/src/search.rs` (edge step cost terms, admissibility: costs only add), `engine/src/route.rs` (plumbing); tests `engine/tests/route_layer_cost.rs`.

- [ ] **Step 1:** tests: `leg_through_foreign_pour_prefers_other_layer_when_cost_set` (two-layer fixture with a B.Cu GND pour; `pour_cut_cost_nm` 0 → B.Cu leg, default → F.Cu leg + via-free), `preferred_direction_cost_tilts_choice` (`direction_cost 0.5`, F.Cu 0°, B.Cu 90°), `costs_zero_match_m3`. Run → FAIL. Implement (`O(1)` per step using `in_pour` of the face). PASS.
- [ ] **Step 2:** eprom B.Cu GND pour: run and report islands (`unconnected_items`) before/after; not worse. Progress PNG `--task A11`. Commit `feat(search): pour-cut and preferred-direction layer costs`.

### Task A12 (S; F diagnosis if the gate misses): dcsgonebadge completion — M3-R14 numbers

**Files:** Create `tests/gate/test_m4_gate.py::test_m4_gate_dcsgone_complete`; fixes in files named by the diagnosis.

- [ ] **Step 1:** write the gate test with the Global Constraints numbers (`wall_clock_s: 600`, `routed == 231`, `routing_new == 0`, `unconnected_items == 0`, `runtime_s ≤ 600`, locked GND 22 items identical — reuse `tests/gate/test_locked_copper.py` helpers — threads 1/2/4 + repeat byte-identical, pcb sha `3e7e66f862a7` printed). Run → record numbers.
- [ ] **Step 2:** if it fails: one F diagnosis note `docs/progress/notes/m4-dcsgone-diagnosis.md` (where the 83 go: capacity, entry, time; profile per iteration from `--profile`), fix tasks named there (S), re-run. If still failing after the fixes: commit, numbers + profile to STATE, escalate to the user (no relaxation).
- [ ] **Step 3:** Progress PNG `--task A12 --board dcsgonebadge` (231/231 expected). Commit `test(gate): M4 dcsgone 231/231 (+ fixes)`.

### Task A13 (S): via position relaxation (M3-R3, bounded Weiszfeld) — `relax.rs`, `route.rs`

**Files:** Modify `engine/src/route.rs` (pass after via reduction: `relax_vias(...)`, `RouteResult.via_relaxation: String`, `Options.via_relaxation: bool = true`), `engine/src/relax.rs`; tests `engine/tests/route_via_relax.rs`.

- [ ] **Step 1:** tests (note §5): `via_moves_toward_force_balance_within_face_star` (two-layer dogleg fixture: via moves, total length decreases, position stays inside the incident faces, `via_fits` true), `via_relaxation_rejects_move_into_keepout`, `via_relaxation_off_matches_previous`, `via_relaxation_threads_byte_identical`. Run → FAIL. Implement (`≤ VIA_RELAX_STEPS = 8` per via, deadline/cancel per step). PASS.
- [ ] **Step 2:** wiccon: vias unchanged count, outside length ≤ previous, DRC 0. Progress PNG `--task A13 --board wiccon-2023`. Commit `feat(route): bounded via position relaxation`.

### Task A14 (S): finishing hand-started traces — G1 continuation from dangling ends — `wires.rs`, `relax.rs`, `route.rs`

**Files:** Modify `engine/src/wires.rs` (`Terminal::Dangling { at, direction }`), `engine/src/relax.rs` / `smooth.rs` (rails for the first `STUB_LEN`), `engine/src/route.rs` (`g1_angle_deg` computed; topology hint = sketch sides of the hand trace's crossings), `engine/src/model.rs` (`Options.dangling_ends: String` = `"continue" | "fixed"`, default `"continue"`); harness fixture `make_fixture_board(..., dangling_trace=True)` (`harness/fixtures.py`); tests `engine/tests/route_g1.rs`, `tests/test_router.py`.

- [ ] **Step 1:** tests: `dangling_end_continues_tangent` (hand segment ending mid-air at 30° → routed leg's first element tangent within `g1_angle_deg ≤ 1.0`, same width and layer, hand copper untouched), `dangling_hint_keeps_side` (the hand trace already passed obstacle X on the left → continuation keeps that side), `dangling_fixed_mode_is_m3`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `test_bench_dangling_fixture_g1` (harness e2e: `g1_angle_deg` reported, DRC 0). Progress PNG `--task A14` on the fixture is a test pattern → **do not** publish; use eprom (unchanged) only if any eprom number moved. Commit `feat(route): G1 continuation of hand-started traces`.

### Task A16 (S; F check in A3): escalating per-pair rip cost — `engine/src/negotiate.rs`, `route.rs`

**Files:** Modify `engine/src/negotiate.rs:36-58, 470-495` (`rip()`), `engine/src/route.rs` (report), `engine/src/model.rs` (`ripup_growth`, `ripup_max_per_pair` from A8); tests `engine/tests/negotiate_ripup.rs`.

**Interfaces:**
- Produces: `rip()` adds `opts.expert.hist_block · growth^(k−1)` for the k-th rip of blocker B by A (`rip_pairs[(A,B)] = k`), refuses when `k ≥ ripup_max_per_pair`; `RIP_PAIR_CAP` removed; `RouteResult.ripup: RipReport { rips_per_connection: Vec<u32>, pairs_at_backstop: Vec<[usize; 2]>, total: u32 }`. Bound `O(max_per_pair · m²)`.

- [ ] **Step 1:** tests: `ping_pong_pair_converges_before_backstop` (two connections sharing one class, contention fixture from `negotiate_loop.rs`: converges with `max(rip_pairs) ≤ 4 < 8`, `pairs_at_backstop` empty), `backstop_stops_rips` (`ripup_max_per_pair: 1` → listed), `growth_one_is_constant_cost`, `ripup_threads_byte_identical`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** eprom incl. `eprom_ripped_front_only_is_the_m2_result` and wiccon not worse (iterations reported). Progress PNG `--task A16 --board wiccon-2023`. Commit `feat(negotiate): escalating per-pair rip cost with backstop`.

### Task A15 (S, gate review F): M4a gate, version 0.4.0, benchmark 0.4.0, review

**Files:** `tests/gate/test_m4_gate.py` (+ eprom/wiccon "not worse" tests, scaling re-run), `engine/Cargo.toml`, `plugin/plugin.json`, `plugin/topor_plugin.py` (0.4.0), `docs/benchmarks/<date>-topor-0.4.0/`, `docs/benchmarks/README.md`, `docs/progress/notes/m4a-gate-review.md`, `docs/design/45-mvp-plan.md` (M4 row: split recorded), `README.md` status table.

- [ ] **Step 1:** `test_m4_gate_eprom_not_worse`, `test_m4_gate_wiccon_not_worse` with the Global Constraints numbers; `pytest tests/gate -m "gate or not gate" -q -s` → all green (M0–M4a); `pytest tests/scaling -m scaling` green.
- [ ] **Step 2:** version 0.4.0, `bash tools/build.sh --dev`; benchmark run on all boards (eprom, wiccon, dcsgone, nametag, wedding; bornhack optional) with `tools/bench.sh`, README table with the H0/A7 columns, images, gate PNG `--task A15-gate` for eprom, wiccon, dcsgone.
- [ ] **Step 3 (F):** independent gate review: complexity of every new loop (A1, A4–A6, A7, A13, A14, A16), note conformance, determinism audit (no HashMap order to output), D-1..D-10 closure, gate tests vs numbers; findings BLOCKING/DEFERRED; blocking ones fixed before STATE "M4a DONE". Commit `docs(state): M4a DONE (0.4.0)`.

---

## M4b — plugin (IPC, config, UI, docs)

### Task B1 (S; doc text H): settings schema, `topor.toml` loader/writer, generated config reference — `plugin/topor/schema.py`, `config.py`, `docs_gen.py`

**Files:** Create `plugin/topor/__init__.py`, `schema.py`, `config.py`, `docs_gen.py`; `docs/user/configuration.md` (generated); tests `tests/plugin/test_config.py`, `tests/plugin/test_config_docs.py`.

**Interfaces:**
- Produces: `Setting(key, section, type, default, unit, range, doc, kicad_place=None, engine_field=None, harness_field=None, plugin_field=None)`; `SETTINGS: list[Setting]` with sections and defaults:
  `[run]` `timeout_s = 60` (0 = disabled; engine `wall_clock_s`), `threads = 0` (auto), `partial_routes = false`, `existing_copper = "fixed"` (`fixed|smooth|rip`), `respect_locked = true`, `scope_mode = "between"` (`between|touching`), `unrouted_only = false`, `fail_on_preexisting = false`, `kicad_cleanup = false`, `kicad_teardrops = false`, `detour_reference = true`;
  `[nets]` `do_not_touch = []`, `power_pattern = "^(\\+|-)?\\d|^(V|VCC|VDD|VBAT|GND|AGND|PWR)"`, `priority = {}` (net → 1..4), `pour_served = {}` (net → true/false override), `ignore_pads = []`;
  `[vias]` `via_cost_mm = 2.0`, `max_per_connection = 4`, `reduction = true`, `reduction_len_slack_pct = 10`, `via_in_pad = false`, `allowed_drills_mm = []`, `fallback_drill_mm = 0.4`, `fallback_diameter_mm = 0.8`, `cost_by_priority = {}`, `cost_by_netclass = {}`, `via_relaxation = true`;
  `[layers]` `route = {}` (layer name → bool override of the KiCad layer type rule), `preferred_direction = {}` (layer → degrees), `direction_cost = 0.0`, `pour_cut_cost_mm = 1.0`;
  `[search]` `detour_factor = 2.0`, `margin_um = 5`, `zero_slack_retry = true`, `exit_stubs = "auto"`, `steiner = true`, `dangling_ends = "continue"`, `strict_unfit_edges = false`;
  `[ripup]` `growth = 2.0`, `max_per_pair = 8`;
  `[iterations]` `max = 30`, `patience = 5`, `min_gain_pct = 1.0`;
  `[output]` `geometry = "arcs"` (`arcs|segments`), `report_dir = "topor-report"`;
  `[expert]` `hist_fail = 1.0`, `hist_block = 0.5`, `hist_overuse = 0.5`, `present_overuse = 4.0`, `zero_slack_candidates = 4`, `relax_iter_cap = 200`, `drc_loop_cap = 5`, `board_source = "file"` (`file|ipc`, B3);
  plus `KICAD_ONLY: list[(comment_key, kicad_place)]` for clearance, widths, via sizes/predefined vias, edge clearance, netclasses, custom rules, layer types, locks, via protection (46 §1.1, comment-only entries).
  `load_config(project_dir: Path) -> tuple[Config, list[Finding]]` (creates the file with `write_default` when missing → finding `CFG_CREATED`; unknown key → `CFG_UNKNOWN_KEY`; wrong type/range → `CFG_VALUE`; `version` ≠ 1 → `CFG_VERSION`); `Config.engine_options() -> dict` (mm → nm, s, the `engine_field` mapping), `Config.non_defaults() -> list[(key, value, default)]`; `render_toml(cfg) -> str` (every key preceded by `# <doc> [unit] default <d>, range <r>`; KiCad-only lines `# <name>: KiCad → <place> (not set here)`); `update_config(path, changes: dict) -> None` (rewrites values, keeps comments); `render_reference_md(SETTINGS) -> str` (one table per section + KiCad-only table); CLI `python -m topor.docs_gen --check` exits 1 when `docs/user/configuration.md` differs.

- [ ] **Step 1:** tests (`test_config.py`): `test_default_file_roundtrips_to_defaults`, `test_unknown_key_warns_not_ignored`, `test_value_out_of_range_finding`, `test_engine_options_units` (`timeout_s 60 → wall_clock_s 60.0`, `via_cost_mm 2.0 → via_cost_nm 2_000_000`, `margin_um 5 → margin_nm 5000`, `timeout_s 0 → wall_clock_s 0.0`), `test_update_config_keeps_comments`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** tests (`test_config_docs.py`): `test_every_setting_documented` (doc, unit, default, range non-empty), `test_reference_md_matches_committed_file`, `test_every_engine_option_is_a_setting_or_internal` (`json.loads(engine.default_options()).keys()` ⊆ `{s.engine_field} ∪ ENGINE_INTERNAL`, `ENGINE_INTERNAL = {"pinned", "drc_spots", "skip_self_check", "profile", "seed", "scope", "net_priority", "via_cost_by_net", "detour_reference_budget_pct"}` each with a one-line reason: set by the harness DRC loop, derived by the plugin from the selection / `@N` markers / `cost_by_*` tables, or diagnostics; every other `Options` field — including `exclude_nets` ← `[nets] do_not_touch`, `ignore_pads`, `allowed_drills_nm` ← `allowed_drills_mm`, `layer_direction` ← `[layers] preferred_direction`, `expert.*` ← `[expert]` — is a Setting's `engine_field`), `test_toml_comments_name_kicad_places`. Generate `docs/user/configuration.md`. PASS. Commit `feat(plugin): settings schema, topor.toml, generated config reference`.

### Task B2 (S): ports and fakes — `BoardPort`, `UiPort`, `FakeBoard`, `ScriptedUi`, `IpcBoard` (ids, selection, commit, refill, run_action) — `plugin/topor/ports.py`, `fake.py`, `ipc.py`

**Files:** Create `plugin/topor/ports.py`, `fake.py`, `ipc.py`; tests `tests/plugin/test_fake_board.py`, `tests/plugin/test_ipc_mapping.py`.

**Interfaces:**
- Produces:
  ```python
  class BoardPort(Protocol):
      def project_dir(self) -> Path
      def pcb_path(self) -> Path
      def live_ids(self) -> LiveIds            # frozensets: tracks, vias, pads, zones (uuids)
      def selection(self) -> Selection         # pads: list[PadRef], footprints: list[str], tracks/vias: list[str], area: list[Pt] | None, nets: list[str]
      def read_model(self, rules: dict) -> dict        # BoardModel JSON (dict), KiCad frame
      def commit(self, req: CommitRequest) -> CommitResult   # req: remove_uuids, items (engine Item dicts), message, group
      def refill_zones(self) -> None
      def run_action(self, action: str) -> None
  class UiPort(Protocol):
      def preflight(self, findings: list[Finding], summary: RunSummary) -> Decision   # proceed: bool, accepted_codes: set[str]
      def progress(self, p: Progress) -> bool          # returns False when the user cancelled
      def preview(self, png: Path, report: Report) -> bool   # Accept
      def error(self, finding: Finding) -> None
  @dataclass class Finding: code: str; severity: Literal["error","warn","info"]; message: str; fix: str; items: list[str] = []
  ```
  `FakeBoard(pcb: Path)`: loads the file with `harness.model.load_board`, `read_model` = `harness.model.export_model`, `live_ids` from the file, `selection` settable, `commit` records `CommitRequest`s and applies them to its in-memory board with `harness.writer` (`write_items`, `remove_items`) so a second `read_model` sees the result; `refill_zones`/`run_action` record calls. `ScriptedUi(decisions: list[Decision], accept: bool, cancel_at_iteration: int | None)` records every call.
  `IpcBoard(kicad: kipy.KiCad)`: `project_dir` = parent of the board file named by `board.document` (kipy `DocumentSpecifier`, field name to confirm in 0.8), `live_ids` via `get_tracks()/get_vias()/get_pads()/get_zones()` ids, `selection` via `get_selection()` (pads/footprints/tracks/vias; a selected rule area or polygon → `area`), `commit` = `begin_commit()` → `remove_items_by_id(remove_uuids)` → `create_items(items → kipy Track/ArcTrack/Via)` → `push_commit(commit, message)` (one undo step; group creation UNVERIFIED in kipy 0.8 → recorded in `CommitResult.grouped = False` and listed in the report when unavailable), `refill_zones()`, `run_action(name)`; `read_model` = file exporter on `pcb_path()` (SWIG `pcbnew`, available in the plugin venv; B3 adds the IPC exporter).

- [ ] **Step 1:** tests (`test_fake_board.py`): `test_fake_read_model_matches_harness_export` (fixture board), `test_fake_commit_applies_and_records` (remove 2 uuids, add 3 items → second read has them; `commits == [req]`), `test_scripted_ui_records_calls`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** tests (`test_ipc_mapping.py`, no live KiCad): `test_item_to_kipy_track_arc_via` (engine `seg`/`arc`/`via` dicts → kipy objects with nm coordinates, width, layer, net name; pure functions `to_kipy_item(item, layer_names, nets)`), `test_selection_from_kipy_items` (pure function over kipy wrappers built from protos). `test_ipc_live_smoke` runs only when `KICAD_API_SOCKET` is set (manual): `live_ids` non-empty. PASS. Commit `feat(plugin): board/ui ports, FakeBoard, ScriptedUi, IpcBoard`.

### Task B3 (S): IPC model export parity and pure rule resolution — `plugin/topor/ipc.py`, `harness/rules.py`

**Files:** Modify `harness/rules.py:178-330` (split into `resolve_rules_from_project(pro: dict, dru_text, *, netclass_of: Callable[[str], NetClassValues], widths_before, settings_table, profile, apply_fab_floors, copper_layers) -> dict` pure + the SWIG wrapper `resolve_rules(board, …)` that reads `.kicad_pro` and calls it), `plugin/topor/ipc.py` (`read_model(source="ipc")`: pads/padstacks via `get_pads()` + `get_pad_shapes_as_polygons`, tracks/vias, zones with `filled_polygons`, rule areas (`is_rule_area`), outline from `get_shapes()` on Edge.Cuts, nets, netclasses via `get_netclass_for_nets`, layer types from `get_stackup()` — UNVERIFIED whether the stackup carries the signal/power/mixed type; fallback: all copper layers routable + finding `LAYER_TYPE_UNKNOWN`); tests `tests/test_rules.py`, `tests/plugin/test_ipc_parity.py` (live only).

- [ ] **Step 1:** `test_resolve_rules_pure_equals_swig` (fixture project: both paths give the same dict). Run → FAIL. Refactor. PASS (eprom gate unchanged).
- [ ] **Step 2:** `test_ipc_model_parity` (skipped without `KICAD_API_SOCKET`): IPC export of the open fixture board equals the file export on pads, nets, kept items, keepouts, pours' `fills` outer rings (sorted, nm-exact). Manual run recorded in the ledger; `read_model` default stays `"file"` until parity is green, then `"ipc"` with `"file"` as the documented fallback (`[expert] board_source`). Commit `feat(plugin): pure rule resolution, IPC model export (parity-tested)`.

### Task B4 (S): pre-flight checks and messages — `plugin/topor/preflight.py`, `messages.py`

**Files:** Create `preflight.py`, `messages.py`; tests `tests/plugin/test_preflight.py`, `tests/plugin/test_messages.py`.

**Interfaces:**
- Produces: `preflight(model: dict, rules: dict, cfg: Config, sel: Selection, live: LiveIds, file_ids: LiveIds, orig_drc: DrcReport, cfg_findings) -> list[Finding]` with codes (each in `MESSAGES` with cause + fix): `BOARD_UNSAVED` (error; live ids ≠ file ids: "Save the board (File → Save) and run Topor again"), `PCBNEW_MISSING` (error), `ENGINE_VERSION` (error), `CFG_CREATED`/`CFG_UNKNOWN_KEY`/`CFG_VALUE`/`CFG_VERSION`/`CFG_VALUE_BACKLOG`, `VIA_NOT_SET_UP` (warn; fallback 0.8/0.4; fix names Board Setup → Pre-defined Sizes → Vias and `[vias] fallback_*`), `DRILL_OUTSIDE_FILTER` (warn; lists hole sizes in the design outside `allowed_drills_mm`; accepting adds them via `update_config`), `VIA_IN_PAD_UNPROTECTED` (warn when `via_in_pad` and the board's via protection is not filled + capped — read from `.kicad_pro` `via_protection`/`m_FillVias`… UNVERIFIED key names, documented as such), `POWER_BY_NAME` (info; nets classified power by `power_pattern`), `MARKER_IGNORED`/`MARKER_CONFLICT`/`MARKER_PAIR` (from B6), `PAD_OUTSIDE_BOARD` (warn; pad centre outside the Edge.Cuts polygon: ref, pad, net; accept → `ignore_pads` written to `topor.toml`), `PREEXISTING_DRC` (info, or error when `fail_on_preexisting`), `TRACK_ANGLE_RULE` (error; `.kicad_dru` `track_angle`/`track_segment_length` constraint), `LOCKED_COPPER` (info; count kept as obstacles, or ripped when `respect_locked = false`), `DANGLING_ENDS` (info; count continued), `DO_NOT_TOUCH` (info), `LAYER_TYPES` (info: which layers route and why; `LAYER_TYPE_UNKNOWN` warn), `SCOPE_EMPTY` (error: selection chosen but nothing routable), `NON_DEFAULTS` (info: `cfg.non_defaults()`), `DEFAULTS_USED` (info: values from built-in defaults, 46 §1.3). `RunSummary` = connections in scope, nets, layers, threads, timeout.

- [ ] **Step 1:** tests, one per code, on `FakeBoard` fixtures (point-in-polygon for `PAD_OUTSIDE_BOARD` with a pad at 1 mm outside the outline; stale ids; dru text with `track_angle`; factory-default vias; `allowed_drills_mm = [0.3]` with a 0.4 drill on the board). Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `test_every_code_has_cause_and_fix` (`MESSAGES` complete, fix text names a KiCad menu path or a `topor.toml` key matching `SETTINGS`), `test_preflight_stale_file_blocks_run`. PASS. Commit `feat(plugin): pre-flight findings and actionable messages`.

### Task B5 (S): run flow — route with progress/cancel, DRC loop + preview on a temporary copy, Accept → single commit — `plugin/topor/run.py`, `preview.py`

**Files:** Create `run.py`, `preview.py`; tests `tests/plugin/test_run_flow.py`.

**Interfaces:**
- Consumes: `harness.drcloop.route_with_drc_loop` (H0), `topor_engine.route_async` (A8), `harness.copy.copy_project`, `harness.rip.rip_tracks`, `harness.writer`, `harness.drc`, `harness.render.compose_compare`, `topor.preflight`, `topor.scope` (B6; until then whole board), `topor.report` (B7; until then a minimal `Report`).
- Produces: `run(port: BoardPort, ui: UiPort, cfg: Config, engine, *, now=...) -> RunOutcome` (`status ∈ {"aborted_preflight", "cancelled", "rejected", "accepted", "error"}`, `report`, `commit: CommitResult | None`):
  1. `load_config` → `preflight` → `ui.preflight` (any `error` finding or `proceed == False` → `aborted_preflight`; accepted `PAD_OUTSIDE_BOARD`/`DRILL_OUTSIDE_FILTER` write back to `topor.toml`).
  2. `preview.prepare_copy(port.pcb_path(), tmp) -> ProjectCopy`; rip per `rip_plan` on the copy; `export_model`; `Options` = `cfg.engine_options()` + scope + `net_priority` + `exclude_nets` + `ignore_pads` + `threads`.
  3. `engine.route_async(...)`; poll `progress()` every 0.25 s → `ui.progress(p)`; `False` → `job.cancel()`; `wait()`; the DRC loop (`route_with_drc_loop`, remaining budget per M3-R8) on the copy; `geometry == "segments"` → `preview.arcs_to_segments(items, max_error)`.
  4. `preview.render(copy, result) -> png` (current board left, result right, both layers, new copper highlighted, caption = report summary) + `report`; `ui.preview(png, report)`; `False` → `rejected` (no `commit`, no `refill`).
  5. `port.commit(CommitRequest(remove_uuids=ripped, items=items, message=f"Topor: routed {n}/{total}", group="topor"))`; partial routes → second group name `topor-partial` in the same commit; `port.refill_zones()`; `kicad_cleanup`/`kicad_teardrops` → `port.run_action("pcbnew.GlobalEdit.cleanupTracksAndVias" / "pcbnew.GlobalEdit.editTeardrops")` (skipped with finding `CLEANUP_SKIPPED_PARTIAL` when partial routes exist, 30 §4.2); `report.write(project_dir / cfg.report_dir / <timestamp>/)` (report.md, preview.png, result.json, drc json).
  Every exception → `ui.error(Finding("INTERNAL", …, fix="see <report_dir>/…/log.txt; file a bug with the board copy (docs/dev/debug.md)"))`, `status == "error"`; `BaseException` (PyO3 panic) caught at `app.main` (31 §6).

- [ ] **Step 1:** tests on `FakeBoard` + `ScriptedUi` with the fixture board and the real engine: `test_run_accept_single_commit` (exactly one `CommitRequest`; it contains the ripped uuids and every routed item; second `read_model` of the fake routes nothing new), `test_run_reject_leaves_board_untouched` (no commit, no refill, report written with `status rejected`), `test_run_cancel_keeps_best_result_and_previews` (cancel at iteration 1 on a 20-connection fixture → `stop_reason == "cancel"`, preview shown), `test_run_aborts_on_error_finding` (stale ids → no routing started: engine not called), `test_run_segments_geometry_has_no_arcs`, `test_run_partial_routes_group`, `test_run_engine_panic_reported_not_crashed` (monkeypatched `route_async` raising `BaseException`). Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `.venv/bin/pytest tests/plugin -q` green. Commit `feat(plugin): run flow — preflight, route with progress/cancel, DRC loop, preview, single commit`.

### Task B6 (S): routing scope from the selection, existing-copper and lock policies, priorities (`@N`, `topor_pN`, power pattern) — `plugin/topor/scope.py`, `priority.py`

**Files:** Create `scope.py`, `priority.py`; tests `tests/plugin/test_scope.py`, `tests/plugin/test_priority.py`.

**Interfaces:**
- Produces: `scope_from_selection(sel: Selection, model: dict, cfg: Config) -> ScopePlan` (`engine_scope: dict` = `Scope {pads, footprints, nets, netclasses, area, unrouted_only}`; `mode`: `between` → connections whose pads are all on selected footprints / selected pads, `touching` → at least one; empty selection → whole board; `unrouted_only` keeps all existing copper fixed and routes only open connections); `rip_plan(model, plan, cfg) -> RipPlan` (`remove_uuids` by `existing_copper` policy: `fixed` → none; `rip` → in-scope nets' tracks/vias except locked when `respect_locked`, except `do_not_touch`; `smooth` → `existing_copper = "smooth"` to the engine), `keep_locked = cfg.respect_locked`.
  `parse_marker(net_name: str) -> int | None` (46 §2.1: last path segment, `~{…}` stripped, `@` + one digit 1–3 followed by end or a non-digit); `net_priorities(nets: list[str], netclass_of: dict[str, str], cfg: Config) -> tuple[dict[str, int], list[Finding]]` (marker > `topor_pN` netclass > `cfg.priority` table > none; conflicts → `MARKER_CONFLICT` (marker wins), merged index → `MARKER_IGNORED`, diff-pair halves differing → `MARKER_PAIR` (higher wins); power nets by netclass name containing `power` or `cfg.power_pattern` for nets still in `Default` → tier 2 unless set, finding `POWER_BY_NAME`).

- [ ] **Step 1:** `test_marker_parse_table` (`USB_D@1+`→1, `CLK@1_P`→1, `DQ@1_3`→1, `DQ@13`→None + ignored, `/ddr/DQ@1_0`→1, `~{RESET}@2`→2, `~{RESET@2}`→2, `A@4`→None, `@2`→2), `test_priority_sources_and_conflicts`, `test_power_pattern_only_for_default_netclass`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `test_scope_between_vs_touching` (fixture with 3 footprints: selecting U1, U2 → `between` routes only U1–U2 connections, `touching` adds U1–J1), `test_scope_pads_two_pins`, `test_scope_area_polygon`, `test_scope_unrouted_only_keeps_copper`, `test_rip_plan_policies` (fixed/rip/smooth × respect_locked × do_not_touch). PASS. Wire into `run.py` (B5) and the pre-flight summary. Commit `feat(plugin): routing scope from the selection, copper policies, @N priorities`.

### Task B7 (S): run report — `plugin/topor/report.py`

**Files:** Create `report.py`; modify `preview.py` (caption from the report); tests `tests/plugin/test_report.py`.

**Interfaces:**
- Produces: `build_report(result: dict, drc: dict, cfg: Config, findings, scope: ScopePlan, hand_stats, timing) -> Report` with sections: summary (routed/total, nets, partial, failed with reasons and blocking points, vias per net vs current board, length vs current, stop reason, elapsed, threads), DRC (`routing_new` by type, `preexisting` separately, unconnected), detours (`detour`/`detour_ghost` stats + top 10), zero-slack wires, vias in pads, partial routes list, ignored pads, accepted fallbacks (widths, drills, via fallback), non-default settings and built-in defaults used (46 §1.3), findings; `Report.summary_lines() -> list[str]` (≤ 20, caption) and `Report.to_markdown()`; `write_report(dir, report, png, result_json, drc_json)`.

- [ ] **Step 1:** `test_report_sections_present_and_values` (fixture run: every section header present, numbers equal the result dict, non-defaults listed only when set), `test_report_summary_lines_le_20`. Run → FAIL. Implement. PASS. Commit `feat(plugin): run report (markdown + caption)`.

### Task B8 (S; layout H): wx UI — pre-flight dialog, progress with Cancel, preview Accept/Reject — `plugin/topor/ui_wx.py`

**Files:** Create `ui_wx.py`; tests `tests/plugin/test_ui_wx.py` (skipped without `wx`/display; `wx.App` headless smoke with `DISPLAY` unset is not required).

**Interfaces:**
- Produces: `WxUi(UiPort)`: `preflight` dialog (findings list by severity with cause/fix, scope mode radio `between|touching`, existing copper choice, respect locked, partial routes, timeout, threads — values pre-filled from `topor.toml`, "Save to topor.toml" checkbox → `update_config`; buttons Route / Cancel); `progress` modal with phase, iteration, routed/total, vias, elapsed, Cancel button (runs on the main thread; the engine job polls from a `wx.Timer`); `preview` dialog showing the PNG (scaled to the screen) and `report.summary_lines()`, buttons Accept / Reject / "Open report folder"; `error` message box. No `alert()`-style blocking in the run loop other than these dialogs.

- [ ] **Step 1:** `test_wx_ui_constructs_without_display` (import + class attributes; skipped if `wx` missing). Implement. Manual check in KiCad per `docs/user/gui-test-checklist.md` (B10) — the GUI test is the user's. Commit `feat(plugin): wx dialogs (preflight, progress + cancel, preview)`.

### Task B9 (S): entry point, app wiring, build/install — `plugin/topor_plugin.py`, `plugin/topor/app.py`, `plugin/plugin.json`, `tools/build.sh`

**Files:** Modify `plugin/topor_plugin.py`, `plugin/plugin.json` (`version` 0.4.0, `min_version` `"3.11"`, action `route`: name "Topor: route", description "Route the board or the selection with Topor"), `tools/build.sh` (copy `plugin/topor/`, `harness/` (no tests/pycache), `docs/user/` into the plugin dir; `requirements.txt` unchanged: `--find-links wheels`, `kicad-python>=0.8,<0.9`, `topor-engine==<version>`); create `plugin/topor/app.py`; tests `tests/test_scaffold.py` (build layout), `tests/plugin/test_app.py`.

**Interfaces:**
- Produces: `app.main(argv) -> int`: engine version handshake (existing), `KiCad(socket_path=os.environ.get("KICAD_API_SOCKET"), kicad_token=os.environ.get("KICAD_API_TOKEN"), client_name="topor")`, `board = kicad.get_board()`, `IpcBoard`, `WxUi`, `load_config(project_dir)`, `run(...)`; `--headless <pcb> [--accept] [--selection JSON]` runs the same flow on `FakeBoard` + `ScriptedUi` (dev/debug path, no KiCad); all `BaseException` caught → stderr message + exit 1 (KiCad shows stderr). `tools/build.sh` layout test: plugin dir contains `plugin.json`, `topor_plugin.py`, `topor/`, `harness/`, `wheels/*.whl`, `requirements.txt`, `docs/user/`.

- [ ] **Step 1:** `test_build_layout_in_scratch_kicad_home` (`KICAD_DOCUMENTS_HOME=<tmp> bash tools/build.sh` → files present; real plugin dir untouched), `test_headless_run_on_fixture_accepts` (`app.main(["--headless", fixture, "--accept"])` → exit 0, report dir written). Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `bash tools/build.sh --dev`; the user runs the GUI checklist on **copies** of eprom and wiccon (`harness.copy.copy_project` into `scratch/gui/<board>/`) — M4b-α milestone: whole-board route from KiCad works end to end. Record the user's result in STATE. Commit `feat(plugin): entry point, app wiring, build/install of the full plugin`.

### Task B10 (S; H for mechanical sections): user guide, GUI test checklist, CONTRIBUTING, debug guide — `docs/user/`, `docs/dev/`, `CONTRIBUTING.md`, `README.md`

**Files:** Create `docs/user/install.md` (build script, wheel, KiCad API server enable (Preferences → Plugins → Enable API), plugin environment, "Recreate Plugin Environment", requirements), `docs/user/operate.md` (first run, options dialog, scope by selection, pre-flight findings and what to do, progress/cancel, preview, Accept/Reject, undo, report folder, known limitations: Linux only, two layers, file must be saved, SWIG dependency for preview), `docs/user/troubleshoot.md` (one `### <CODE>` section per `MESSAGES` code: symptom → cause → fix; plus "plugin exited with code N", "wheel not found", "stale venv"), `docs/user/gui-test-checklist.md` (numbered steps for eprom and wiccon copies: run action → pre-flight content → progress + Cancel once (best result shown) → preview → Reject (board unchanged, Edit → Undo greyed) → run again → Accept → one Ctrl+Z reverts everything → DRC 0 new in KiCad's DRC dialog → report folder exists → selection scope: select two footprints, `between`, only those connections change; results table to fill in), `docs/dev/debug.md` (`route_probe`, `gate_diag --detour`, `--profile` timers, progress PNGs, `harness.measure`, `app --headless`, reproducing a plugin run from the report's `result.json`/board copy, filing a bug), `CONTRIBUTING.md` (repo layout, build, tests: unit/gate/scaling/benchmarks, harness on a copied board, read-only projects rule, commit style, where settings live (schema → toml → docs)); update `README.md` status table and `docs/README.md` index; tests `tests/plugin/test_messages.py::test_troubleshoot_has_section_per_code`, `tests/test_docs_links.py` (every relative link in `docs/user/*.md`, `CONTRIBUTING.md` resolves).

- [ ] **Step 1:** write the tests → FAIL; write the docs; PASS. `python -m topor.docs_gen --check` clean. Commit `docs(user): install/operate/troubleshoot/GUI checklist; CONTRIBUTING and debug guide`.

### Task B11 (S, gate review F): M4b gate — headless e2e, GUI checklist result, docs tests, review

**Files:** Create `tests/gate/test_m4b_gate.py`; `docs/progress/notes/m4b-gate-review.md`; STATE.

- [ ] **Step 1:** `test_m4b_gate_headless_eprom_and_wiccon` (marked `gate`): `FakeBoard` on the snapshot copies, `ScriptedUi(accept=True)`, real engine: `status == "accepted"`, one commit, routed == M3 numbers (eprom 27/27, wiccon 62/62), `routing_new == 0` on the fake's saved board after `kicad-cli` DRC, report dir written, `topor.toml` created with all defaults, `docs_gen --check` clean; `test_m4b_gate_scope_selection` (two footprints, `between`). Run → PASS.
- [ ] **Step 2:** user executes `docs/user/gui-test-checklist.md` on eprom and wiccon copies in KiCad 10 (pre-flight, progress + Cancel, preview, Reject untouched, Accept one undo step, DRC 0 new); results recorded in STATE under "M4b GUI checklist (user)". Any failure → fix task, re-run.
- [ ] **Step 3 (F):** independent review of the plugin: port boundaries (no kipy outside `ipc.py`, no SWIG outside `harness/` + `preview.py`), every 46 setting present with its source order, message coverage, exception paths (`BaseException`), undo atomicity, docs vs behaviour; BLOCKING fixed before STATE "M4b DONE / M4 DONE". Commit `docs(state): M4b DONE — M4 complete`.

---

## Deferred (recorded for STATE.md, with reasons)

1. **45° routing mode, missing-width dialog, power-as-traces, 4+ layer fan-out vias** — backlog per 45 §3 / 46 §3 (user).
2. **Multi-variant search (N routings with different seeds, Pareto)** — M5 (31 §6.1; `Options.seed` stays unused); M4 uses threads for batches/self-check/reference searches.
3. **Planar max-cut layer assignment** — M5 (> 2 layers), as M3 deferred 3.
4. **Full IPC model export as the default source** — B3 delivers it parity-tested; the default flips to `"ipc"` only when the live parity test is green; the file exporter (SWIG, Linux) stays as fallback until KiCad 11 headless IPC. Reason: the exporter is the largest untested surface; the user gate needs a reliable path first.
5. **Groups via IPC** (`topor` / `topor-partial`) — UNVERIFIED in kipy 0.8; listed in the report when unavailable; no gate number depends on it.
6. **Re-sketch only changed wires; zero-slack pass searches** — unchanged from M3 deferred 6 (bounded already).
7. **wedding-gift / nametag quality** — re-measured in benchmark 0.4.0 (Steiner A5, stubs A6 expected to help); no gate number.

## Self-review notes

- **Spec coverage.** 45 M4 row: IPC plugin + wheel → B2/B3/B9; `topor.toml` → B1; priority tiers → A9 + B6; Steiner → A3/A5; exit stubs → A6; keepouts → H0 (dru `disallow`, footprint areas already exported) + engine (M3); DRC loop → H0 + B5; preview + Accept/Reject → B5/B8; progress + Cancel → A8/B8; routing scope / partial reroute (pads, components between/touching, nets, netclasses, area, unrouted-only) → B6 + engine `Scope`; partially routed boards + attach to any point → A5 (T-junction on hand copper) + existing `copper` terminals; finishing hand-started traces G1 → A14; partial routes option → B5 (`topor-partial`); pre-flight report → B4. 46 table: #2 vias/fallback/`allowed_drills` → A10 + B4; #3 edge clearance (M0); #4 power recognition → B6; #5 `@N`/`topor_pN` → B6; #6 via minimisation knobs + overrides → A10/B1; #7 detour limit → A8 (`detour_factor`); #8a pour-served override → B1 (`[nets] pour_served`) + engine (M3 auto); #8b do-not-touch → A9/B6; #9 via-in-pad → A10/B4; #10 output geometry → B5; #11 cleanup/teardrops → B5; #12 preferred direction → A11; #13 routing layers by type → model export (M3) + `[layers] route` override B1; #14 respect locked → B6; #17 rip limit → A16 (coordinator addition, `[ripup]`); #17b stop rule → A8; #18 timeout/disable/Cancel → A8/B8; #19 preview → B5/B8; #20 pre-existing listed → B4/B7; #21 pins outside board → B4 + A9; #22 selected components between/touching → B6; expert row → A8 `[expert]`. STATE carry-overs: (1) search speed → A1; (2) #201 → A2; (3) dcsgone completion → A12; (4) M4-D1 → A4, D2 → A2, D3 → A5, D4 → A7; (5) D-1..D-10 → A0, D-11 → A1/A12/A2/A4–A7; (6) plan-deferred 1 G1 → A14, 2 Weiszfeld → A13, 3 max-cut → deferred 3, 4 variants → deferred 2, 5 tiers → A9, 6 → deferred 6, 7 wedding/nametag → deferred 7 + A15 benchmark, 8 pour cuts → A11. Coordinator addition (escalating rip cost) → A3 §6 + A16.
- **Type consistency.** `RunControl`/`Progress`/`route_async` (A8) are consumed by B5; `default_options()` (A8) by B1's docs test; `Finding` (B2) by B4/B5/B7/B8; `Selection`/`ScopePlan`/`RipPlan` (B2/B6) by B5; `route_with_drc_loop` (H0) by B5; `ref_len_nm`/`ref_kind` (A7) by `metrics.detour_stats` and B7; `fits_for`/`wall_r` (A4) by A5/A6; `Terminal::Stub` (A6) and `Terminal::Dangling { direction }` (A14) by `wires.rs` consumers; `ripup_growth`/`ripup_max_per_pair` (A8) by A16; `stop_reason` (A8) by H0 bench columns and B7.
- **Proportion.** Signatures, tests and values only; algorithms with a choice live in the F note (A3) as in M2/M3.
- **Risks.** (i) A4 touched `eprom_ripped_front_only` in M3 (F6 revert) — the wall-aware rule is the fix, the guard is the test. (ii) `pathfinder_iter_cap` 50 → 30 could change wiccon's 26-iteration run; A8 reports before/after and escalates rather than tuning. (iii) kipy 0.8 group creation and stackup layer types are UNVERIFIED — both have documented fallbacks and no gate number depends on them. (iv) The preview path needs `pcbnew` in the plugin venv (system-site-packages on Linux); pre-flight `PCBNEW_MISSING` names the fix.
