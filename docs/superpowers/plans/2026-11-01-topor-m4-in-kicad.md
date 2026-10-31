# Topor M4 In-KiCad Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Execution method (user, 45 §4, as in M1–M3):** rotating coordinator (O) dispatching one worker per task at the tier in the task title (H/S/F); independent tasks in parallel in isolated worktrees (R3: workers never touch the shared `.venv`); F for the design notes, the UI design, failed-gate diagnosis and the gate reviews. Every worker report is folded into `docs/progress/STATE.md` (≤ 5 lines); SDD ledger `.superpowers/sdd/2026-11-01-topor-m4-in-kicad/progress.md`; handover at a gate or at ~50 % context / ~15 round-trips. R1: work on main, **never push**.
>
> **Revision 2 (user rulings 2026-10-31):** plugin first (M4b, version 0.4.0), engine second (M4a, version 0.5.0); **no pcbnew/SWIG anywhere in the plugin runtime** (IPC + stable file formats + `kicad-cli` only; the dev/test harness keeps pcbnew as an oracle for now); via placement = bounded relaxation, free placement → backlog; iteration cap 30 accepted (wiccon before/after reported); kipy fallbacks accepted; `plugin.json` `min_version` 3.11; every plugin UI task runs the UI design skills, with F-designed ASCII wireframes reviewed before implementation.

**Goal:** First, route eprom-to-kernal and wiccon-2023 from inside KiCad 10 through the IPC plugin with today's engine behaviour — pre-flight report, progress dialog with Cancel, preview with Accept/Reject, one undo step, configured by a self-documenting `topor.toml`, usable without AI (M4b gate, 0.4.0). Then bring the engine to the M4 hard numbers: dcsgonebadge (snapshot `3e7e66f862a7`) 231/231 routed, 0 router DRC, ≤ 10 min, locked edge GND kept (M3-R14), eprom/wiccon not worse (M4a gate, 0.5.0).

**Architecture:** **M4b (plugin)** adds a thin KiCad side behind two ports — `BoardPort` (kicad-python 0.8 over the IPC API; `FakeBoard` for headless tests) and `UiPort` (wx dialogs designed from F wireframes; `ScriptedUi` for tests). Board data comes from IPC (items, pads with padstacks, footprints, zones with fills, rule areas, nets, netclasses) plus a `save_as` copy of the live board (which includes unsaved edits) for what IPC does not expose (`(layers …)` types) and for verification: the routed items are appended to that copy by our own s-expression writer, `kicad-cli pcb drc` judges it and `kicad-cli pcb export svg` → `wx.svg` renders the preview. Rules come from `.kicad_pro` JSON + `.kicad_dru` parsing, then IPC netclasses. Accept = one `begin_commit` / `create_items` / `push_commit`; Reject writes nothing. Shared pure code (DRC runner/classifier, dru parser, rule resolution, DRC loop, detour stats) moves into `toporlib/`, used by both the plugin and the harness; a test proves the plugin runtime never imports `pcbnew`. Three engine prerequisites (cancel + progress, `default_options`, stop rule) are the first M4b task. **M4a (engine)** then finishes the router: slot-search speed, fine-pitch entry, own-net capacity and same-net Steiner T-junctions, pad exit stubs, G1 continuation, escalating rip cost, priority tiers, via rules from the board setup, bounded via relaxation, layer cost terms, a DRC-feasible detour reference, dcsgone completion.

**Tech Stack:** as M3 (Rust 1.96, spade 2.15.1, rayon 1.10, pyo3 0.23 abi3-py39, maturin in `.venv`, Python 3.12 + pcbnew SWIG **harness only**, KiCad 10.0.8 `kicad-cli`, rsvg-convert/ImageMagick harness only) plus, plugin: **kicad-python `>=0.8,<0.9`** (pinned), **wxPython 4.2.1 incl. `wx.svg`** (verified in `~/.cache/kicad/10.0/python-environments/org.topor.router`), `tomllib` (Python ≥ 3.11 → `plugin.json` `min_version` `"3.11"`). No new Rust crates.

**Spec:** `docs/design/45-mvp-plan.md` (§1 M4 row + gate, §3 M4 row, §5a, configurability + "usable without AI"); `docs/design/46-configuration.md` (§1, §1.1, §2 #1–#22 + engine/expert/fixed rows, §2.1); `docs/kicad/30-kicad-constraints.md` §2–§4.2, §5; `docs/kicad/31-rust-engine-and-integration.md` §2.1, §3, §6, §6.1; `docs/progress/STATE.md` (M3 DONE header, M4 carry-over, M3-R13/R14/R15); `docs/progress/notes/m3-gate-review.md` (§8, D-1..D-11); `docs/progress/notes/m3-detour-diagnosis.md` (M4-D1..D4); M3 plan ("Deferred to M4" 1–8); `docs/topology/12-sketch-generation.md` §1, §3.1; `docs/geometry/25-relaxation-and-reflow.md` §3; `docs/geometry/22-smoothness-and-biarcs.md`.

## Global Constraints

All M0–M3 Global Constraints and rulings still apply (Linux; `.venv` only; read-only `~/work/kicad/projects` — the harness reads boards **only** from `scratch/boards/<board>/<sha12>/` via `harness.paths.BENCH_BOARDS`, gate tests hash the snapshot dir; no KiCad types in the engine; KiCad-frame integer nm with one Y flip each way; `EPS_BUILD_NM` 5 000 / `margin_nm` 5 000; fab floors; length gate = outside-pad copper; R6/R6b/R8; M3-R8 whole-run budget; compact ≤ 20-line summaries; conventional commits ending with the attribution line of the executing session's system reminder). In addition for M4:

- **No pcbnew in the plugin runtime (user):** nothing under `plugin/` or `toporlib/` imports `pcbnew`, directly or transitively; enforced by `tests/plugin/test_no_pcbnew.py` (AST scan + a subprocess that blocks the module and imports `topor.app`). Board I/O = kipy (IPC) + `Board.save_as` copies + our s-expression writer + `kicad-cli`. Every kipy call the plugin relies on is pinned by `tests/plugin/test_kipy_surface.py` against the installed kipy source (names, signatures, proto fields); whatever needs a running KiCad is marked UNVERIFIED in the task and appears in the GUI checklist.
- **Hard M4b gate (user):** a GUI-testable whole-board plugin on eprom + wiccon with the current engine behaviour: from KiCad 10, pre-flight → progress + Cancel → preview → Reject leaves the board untouched / Accept = **one undo step**; eprom 27/27 and wiccon 62/62 routed, `routing_new 0`, `unconnected 0` on the `save_as` copy judged by `kicad-cli`; headless e2e on `FakeBoard` reproduces the numbers; `topor.toml` self-documenting + generated reference + undocumented-key test; user guide / troubleshoot / GUI checklist / CONTRIBUTING / debug guide; IPC model parity with the harness model on every snapshot board (live test, user-run). Version 0.4.0 (engine + plugin) at this gate.
- **Hard M4a gate (user, M3-R14):** dcsgonebadge `3e7e66f862a7`, `run_bench(router=route_router, rip="all", threads=4, options={"wall_clock_s": 600, "partial_routes": False})`: `routed == connections_total == 231`, `partial == failed == 0`, `routing_new == 0`, `unconnected_items == 0`, `runtime_s ≤ 600` (whole run), 22 locked edge-GND items identical + 0 DRC pairs with router copper, threads 1/2/4 + repeat byte-identical. Version 0.5.0.
- **Not worse than M3 (user):** eprom 27/27, 25/25, `routing_new 0`, `unconnected 0`, `length_outside_pads_nm ≤ 64_700_000` and `≤ hand`, `router_s < 2.0`, zero_slack 0 or DRC-kept, the M2 guard `eprom_ripped_front_only_is_the_m2_result` (27/27, ≤ 67.8 mm) green; wiccon 62/62 (signal 57/57), `vias ≤ 37`, `routing_new 0`, `unconnected 0`, `length_outside_pads_nm ≤ 1_380_000_000`, `router_s < 10.0`; scaling L1/L2 within `t100/t10 ≤ 40`, `t1000/t100 ≤ 30`. **Budgets are never relaxed by a worker or coordinator**; one F diagnosis round, then escalate with numbers.
- **Complexity (45 §5a):** every new component states its Big-O in its task and code comment; no exponential/factorial stage; every loop capped; every long call checks the deadline **and the cancel flag**; rip-up bound `O(max_per_pair · m²)`.
- **Determinism:** byte-identical result JSON (minus `runtime_s`) for `threads ∈ {1, 2, 4}` and across runs, for every board and option; no `HashMap` iteration order reaches the output.
- **Configurability (45):** every judgement call is a `topor.toml` key with a sane default in `plugin/topor/schema.py`, listed in the run report when non-default, mapped 1:1 to an `Options`/harness/plugin field. Sources (46 §1): KiCad board setup → `topor.toml` (created with all defaults + comments on first run, unknown keys warn, `version = 1`) → built-in defaults (listed).
- **Messages (46 §1.1):** every finding/error has a code in `plugin/topor/messages.py` with cause + fix naming the KiCad place or the `topor.toml` key; `docs/user/troubleshoot.md` has a section per code (tested).
- **UI (user):** every plugin UI task invokes, via the Skill tool, `ecc:frontend-design-direction` (direction, layout), `ecc:make-interfaces-feel-better` (spacing, states, feedback, wording) and `ecc:accessibility` (keyboard, focus order, contrast, sizes, labels) **before** designing or implementing, adapting web guidance to wxPython inside KiCad: native widgets, KiCad dialog look, light and dark themes, HiDPI. ASCII wireframes per dialog (F, Task B4) are reviewed by an F agent before any implementation (S). GUI checklist acceptance: tab order, Esc = Cancel, Enter = default button, readable at 100 % and 200 % scaling, dark theme.
- **Progress PNG (user):** every outcome-affecting engine task ends with `.venv/bin/python -m harness.progress --milestone m4 --task <TaskId> --router topor --board <board>` → `docs/benchmarks/progress/<YYYY-MM-DD>-m4-<TaskId>[-<board>].png` (hand left, topor right, both layers, caption with routed/total, vias, DRC, `router_s`, **detour line**, **pcb sha12**), committed, sent to main. Never a test pattern as a result.
- **Builds:** wheel rebuilt (`VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release --manifest-path engine/Cargo.toml --features python`) before every engine-using pytest run; `bash tools/build.sh --dev` before the gates; clippy `-D warnings` clean; `.venv/bin/pytest -q` green.
- **Backlog, not M4:** 45° mode, missing-width dialog, power-as-traces, 4+ layer fan-out vias (46 §3); **fully free via placement (incremental CDT)** and **harness migration to the file/`kicad-cli` path before KiCad 11** (both added to 45 §3 in Task B0). `[output] geometry` accepts `arcs|segments`; `"45deg"` → `CFG_VALUE_BACKLOG`.

## Review Focus

1. **Cancel during a batch search or completion pass** must return the best result so far within one deadline-check interval, `stop_reason == "cancel"`, never panic. → `route_async_cancel_returns_anytime_result` (P1).
2. **Unsaved edits in the editor:** the preview/DRC copy must come from `save_as` (live state), never from the on-disk file. → `test_run_uses_save_as_copy_not_disk_file` (B5: `FakeBoard.save_as` writes content that differs from `pcb_path()`; the routed temp file must derive from the former).
3. **Reject** leaves the live board untouched (no commit begun, no refill); **Accept** is exactly one commit with removals and creations. → `test_run_reject_leaves_board_untouched`, `test_run_accept_single_commit` (B5).
4. **`@N` marker edge cases** — `DQ@1[0..7]` merged index, diff-pair halves with different markers, marker vs `topor_pN` conflict, `~{RESET@2}`. → `test_marker_parse_table` (B7).
5. **Timeout disabled (`timeout_s = 0`) with an unroutable connection** terminates by the stop rule, not by spinning. → `stop_rule_no_gain_terminates_without_timeout` (P1).

## File Structure

```
toporlib/                      NEW pure-Python package shared by plugin and harness (no pcbnew) (B0)
  __init__.py
  kicad_cli.py                 kicad_cli() path resolution (from harness/paths.py)
  drc.py                       run_drc, classify, split_unconnected, summarise (moved from harness/drc.py)
  dru.py                       parse_dru, match_condition, disallow → keepouts (from harness/rules.py; B3 adds disallow)
  rules.py                     resolve_rules_from_project(pro, dru_text, netclass_of, …) pure (B3)
  drcloop.py                   route_with_drc_loop(model, options, *, route_fn, drc_fn, budget_s) (B0)
  detour.py                    detour_stats, detour_line (from harness/metrics.py)
  pcbfile.py                   s-expression: parse_layers, append_items, remove_items, find_items (B3)
harness/                       dev/test only; keeps pcbnew as the oracle; thin wrappers over toporlib (B0)
  drc.py, metrics.py, rules.py  re-export / wrap toporlib; bench.py uses drcloop with a pcbnew drc_fn
  progress.py                  --milestone m4 naming (A0)
engine/
  src/control.rs               NEW RunControl, Progress (P1)
  src/model.rs                 Options additions (P1, A9, A10, A11, A13, A14), Rules.predefined_vias (A10)
  src/py.rs                    route_async → RouteJob, default_options() (P1)
  src/route.rs                 stop rule/stop_reason/cancel (P1); tiers, exclude/ignore (A9); detour reference (A7);
                               via relaxation (A13); layer costs (A11); G1 (A14); Steiner split (A5); stubs (A6)
  src/negotiate.rs             escalating rip cost (A16); stop-rule inputs (P1)
  src/search.rs                clear radius / memo / early stop (A1); state_point/los (A2); own-strand goals (A5); stubs (A6); layer costs (A11)
  src/graph.rs                 clear_r per face (A1); own-net capacity, wall_r (A4)
  src/decompose.rs             tiers, exclude_nets, ignore_pads (A9); star/path ordering (A5)
  src/vias.rs                  predefined/fallback/allowed_drills/via-in-pad (A10)
  src/relax.rs, smooth.rs      via relaxation (A13); stub + G1 rails (A6, A14)
  src/pourcheck.rs, multigraph.rs   D-9, D-4, D-10 (A0)
  tests/route_control.rs, search_speed.rs, search_entry.rs, graph_ownnet.rs, route_steiner.rs, route_stubs.rs, route_g1.rs,
        negotiate_ripup.rs, route_priority.rs, via_rules.rs, route_layer_cost.rs, route_detour_ref.rs, route_via_relax.rs
plugin/
  plugin.json                  version 0.4.0, min_version 3.11 (B10)
  topor_plugin.py              entry: handshake, sys.path, topor.app.main() (B10)
  topor/schema.py, config.py, docs_gen.py          (B1)
  topor/ports.py, fake.py                          (B2)
  topor/ipc.py                 IpcBoard: ids, selection, save_as copy, commit, refill, run_action (B2); model exporter (B3)
  topor/model_ipc.py           export_model_ipc(board, project, pcb_copy_text, rules) → BoardModel dict (B3)
  topor/preflight.py, messages.py                  (B6)
  topor/priority.py, scope.py                      (B7)
  topor/preview.py             temp copy via save_as, pcbfile edits, DRC loop, svg → wx bitmap compare (B5)
  topor/report.py                                  (B8)
  topor/run.py                 the flow (B5)
  topor/ui_wx.py               WxUi from the B4 wireframes (B9)
  topor/app.py                 main(), --headless (B10)
docs/progress/notes/m4-ui-wireframes.md             F UI design (B4)
docs/progress/notes/m4-capacity-steiner-design.md   F engine note (A3)
docs/progress/notes/m4b-gate-review.md, m4a-gate-review.md, m4-dcsgone-diagnosis.md
docs/user/{install,configuration,operate,troubleshoot,gui-test-checklist}.md, docs/dev/debug.md, CONTRIBUTING.md (B11)
tests/plugin/test_{no_pcbnew,kipy_surface,config,config_docs,fake_board,ipc_mapping,ipc_parity,pcbfile,model_ipc,preflight,
                   priority,scope,run_flow,report,messages,ui_wx,app}.py
tests/gate/test_m4b_gate.py (B12), tests/gate/test_m4_gate.py (A12, A15)
docs/benchmarks/<date>-topor-0.5.0/   (A15)
```

**Dependencies and parallel groups.**
- **M4b group 1** (parallel): **P1** (engine) · **B0** (toporlib) · **B1** (schema/config; its engine-field test needs P1 → `xfail(strict=False)` until merged) · **B4** (F UI wireframes; needs only 46 and the flow description below).
- **Group 2:** **B2** ← B0 · **B3** ← B0, B2.
- **Group 3:** **B5** ← P1, B1, B2, B3 · **B6** ← B1, B2 · **B7** ← B2 · **B8** ← B1.
- **Group 4:** **B9** ← B4, B5 · **B10** ← B5, B9 → **M4b-α GUI test** (user) · **B11** ← all · **B12** gate.
- **M4a group 1:** **A0** · **A1** · **A3** (F note) · **A9** · **A7**; **group 2:** **A2** ← A1 · **A10** ← A9 · **A11** ← A9 · **A16** ← A3; **group 3:** **A4** ← A3, A2 · **A13** ← A3, A10; **group 4:** **A5** ← A4 · **A6** ← A4 · **A14** ← A3, A6; **group 5:** **A12** ← A1–A6, A16 → **A15** gate.

**Model tiers:** P1 S · B0 S · B1 S (doc text H) · B2 S · B3 S (F review at the gate) · B4 **F** (design) + **F** review · B5 S · B6 S · B7 S · B8 S · B9 S (from B4 wireframes) · B10 S · B11 S (H mechanical) · B12 S + **F** review · A0 S (H steps) · A1 S (F review) · A2 S (F review) · A3 **F** · A4–A7, A9–A11, A13, A14, A16 S · A12 S + **F** diagnosis if missed · A15 S + **F** review. Any task failing its acceptance twice → F diagnosis note (45 §5).

---

## Part 1 — M4b: plugin (version 0.4.0)

### Task P1 (S): engine prerequisites — stop rule, timeout disable, cancel + progress (`route_async`), `default_options`, options for the 46 knobs — `engine/src/control.rs`, `model.rs`, `route.rs`, `negotiate.rs`, `smooth.rs`, `py.rs`

**Files:** Create `engine/src/control.rs`; modify `engine/src/model.rs:632-687`, `engine/src/route.rs` (stop rule, `stop_reason`, cancel at every deadline check), `engine/src/negotiate.rs:36-58` (constants → options), `engine/src/smooth.rs:216` (`wall_clock_s == 0.0` → far-future deadline), `engine/src/py.rs`; tests `engine/tests/route_control.rs`, `tests/test_engine_surface.py`.

**Interfaces:**
- Produces (serde `default`, names = `topor.toml` keys of B1 minus the section): `Options.pathfinder_iter_cap` default **30**, `patience: u32 = 5`, `min_gain_pct: f64 = 1.0`, `wall_clock_s: f64 = 60.0` (**`0.0` = disabled**), `detour_factor: f64 = 2.0`, `ripup_growth: f64 = 2.0`, `ripup_max_per_pair: u32 = 8` (A16 consumes; until then `RIP_PAIR_CAP` reads `ripup_max_per_pair`), `expert: Expert { hist_fail: 1.0, hist_block: 0.5, hist_overuse: 0.5, present_overuse: 4.0, zero_slack_candidates: 4, relax_iter_cap: 200 }`.
  `RouteResult.stop_reason: String` ∈ `{"converged", "cap", "no gain", "timeout", "cancel", "stuck"}` — replaces `STALL_ITERS = 8`: stop when over the last `patience` iterations no connection was newly routed **and** the best cost `Σ length + via_cost · vias` improved by `< min_gain_pct` in total.
  `control::RunControl { cancel: Arc<AtomicBool>, progress: Arc<Mutex<Progress>> }`, `Progress { phase: String, iteration: u32, routed: usize, total: usize, vias: usize, elapsed_s: f64 }` updated per iteration and per pass; `route_with(model, opts, ctl: &RunControl) -> RouteResult`; `route()` = `route_with(…, &RunControl::default())`; `Ctx::stop(&self) -> bool` = deadline **or** cancel, used at every existing deadline site (O(1)).
  PyO3: `route_async(model_json, options_json) -> RouteJob` (`#[pyclass]`: `progress() -> str`, `cancel()`, `done() -> bool`, `wait() -> str`; job on a `std::thread`, `wait` releases the GIL); `default_options() -> str` (JSON of `Options::default()`).

- [ ] **Step 1:** tests (`route_control.rs`): `stop_rule_no_gain_terminates_without_timeout` (unroutable fixture, `wall_clock_s: 0.0` → `stop_reason == "no gain"`, iterations ≤ 30), `stop_rule_counts_newly_routed_as_progress`, `route_async_cancel_returns_anytime_result` (dcsgone model, cancel after 2 s → `"cancel"`, `routed ≥ 1`, self-checked, no panic), `progress_is_monotone`, `timeout_zero_is_disabled`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `tests/test_engine_surface.py`: `test_default_options_lists_every_field`, `test_route_async_roundtrip_fixture` (`wait()` == `route()` minus `runtime_s`).
- [ ] **Step 3:** eprom byte-identical; wiccon with cap 30 / patience 5 reported before/after (62/62 expected; if worse → escalate per the user's ruling, do not tune). Scaling green. Progress PNG `--task P1 --board wiccon-2023`. Commit `feat(engine): stop rule, cancel + progress, route_async, default_options, 46 options`.

### Task B0 (S): `toporlib/` extraction, DRC-loop injection, no-pcbnew guard, backlog rows — `toporlib/`, `harness/`, `docs/design/45-mvp-plan.md`

**Files:** Create `toporlib/{__init__,kicad_cli,drc,dru,detour,drcloop}.py`; modify `harness/drc.py`, `harness/metrics.py`, `harness/rules.py`, `harness/paths.py` (thin wrappers/re-exports), `harness/bench.py:262-420` (calls `route_with_drc_loop`), `docs/design/45-mvp-plan.md` §3 (two backlog rows); tests `tests/plugin/test_no_pcbnew.py`, `tests/test_drcloop.py`, existing harness tests unchanged.

**Interfaces:**
- Produces: `toporlib.drcloop.route_with_drc_loop(model: dict, options: dict, *, route_fn: Callable[[dict, dict], dict], drc_fn: Callable[[list[dict]], DrcJudgement], base_drc, orig_drc, budget_s: float, loop_cap: int) -> LoopResult` — the loop now inside `run_bench` (DRC spots, zero-slack judge, M3-R8 remaining budget) with the board writes and DRC behind `drc_fn(items) -> DrcJudgement(report: DrcReport, created: dict)`; `LoopResult(router_report, items, created, routed_drc, rounds, pinned, spots, zero_slack, budget_exhausted, router_s)`; `run_bench` output dict unchanged. `toporlib.drc` = today's `harness/drc.py` (kicad-cli only). `toporlib.detour.detour_stats/detour_line`. `toporlib.dru.parse_dru/match_condition`. `toporlib.kicad_cli.kicad_cli()`.
  `test_no_pcbnew`: AST scan of `plugin/**/*.py` and `toporlib/**/*.py` for `pcbnew` imports (none) + `subprocess.run([sys.executable, "-c", "import sys; sys.modules['pcbnew'] = None; import topor.app"])` exit 0 (B10 completes the import; until then `topor.ports`).

- [ ] **Step 1:** `test_drcloop_matches_run_bench_on_fixture` (pin pre-refactor `routed_drc`/`drc_loop` JSON of a fixture run as expected values). Run → FAIL. Extract. PASS; `.venv/bin/pytest -q` and `tests/gate/test_m3_gate.py -k eprom` unchanged.
- [ ] **Step 2:** `test_no_pcbnew` passes; 45 §3 backlog rows: "Fully free via placement (incremental CDT) — when bounded relaxation isn't enough, or as an optional routing mode for complex boards (user 2026-10-31)" and "Harness migration to the file/`kicad-cli` path (no pcbnew) before KiCad 11". Commit `refactor(toporlib): shared pure modules, injected DRC loop, no-pcbnew guard; backlog rows`.

### Task B1 (S; doc text H): settings schema, `topor.toml` loader/writer, generated config reference — `plugin/topor/schema.py`, `config.py`, `docs_gen.py`

**Files:** Create `plugin/topor/__init__.py`, `schema.py`, `config.py`, `docs_gen.py`; `docs/user/configuration.md` (generated); tests `tests/plugin/test_config.py`, `tests/plugin/test_config_docs.py`.

**Interfaces:**
- Produces: `Setting(key, section, type, default, unit, range, doc, kicad_place=None, engine_field=None, harness_field=None, plugin_field=None)`; `SETTINGS` sections/defaults:
  `[run]` `timeout_s = 60` (0 = disabled; engine `wall_clock_s`), `threads = 0`, `partial_routes = false`, `existing_copper = "fixed"` (`fixed|smooth|rip`), `respect_locked = true`, `scope_mode = "between"` (`between|touching`), `unrouted_only = false`, `fail_on_preexisting = false`, `kicad_cleanup = false`, `kicad_teardrops = false`, `detour_reference = true`;
  `[nets]` `do_not_touch = []`, `power_pattern = "^(\\+|-)?\\d|^(V|VCC|VDD|VBAT|GND|AGND|PWR)"`, `priority = {}`, `pour_served = {}`, `ignore_pads = []`;
  `[vias]` `via_cost_mm = 2.0`, `max_per_connection = 4`, `reduction = true`, `reduction_len_slack_pct = 10`, `via_in_pad = false`, `allowed_drills_mm = []`, `fallback_drill_mm = 0.4`, `fallback_diameter_mm = 0.8`, `cost_by_priority = {}`, `cost_by_netclass = {}`, `via_relaxation = true`;
  `[layers]` `route = {}`, `preferred_direction = {}`, `direction_cost = 0.0`, `pour_cut_cost_mm = 1.0`;
  `[search]` `detour_factor = 2.0`, `margin_um = 5`, `zero_slack_retry = true`, `exit_stubs = "auto"`, `steiner = true`, `dangling_ends = "continue"`, `strict_unfit_edges = false`;
  `[ripup]` `growth = 2.0`, `max_per_pair = 8`;
  `[iterations]` `max = 30`, `patience = 5`, `min_gain_pct = 1.0`;
  `[output]` `geometry = "arcs"` (`arcs|segments`), `report_dir = "topor-report"`;
  `[expert]` `hist_fail = 1.0`, `hist_block = 0.5`, `hist_overuse = 0.5`, `present_overuse = 4.0`, `zero_slack_candidates = 4`, `relax_iter_cap = 200`, `drc_loop_cap = 5`;
  plus `KICAD_ONLY: list[(comment_key, kicad_place)]` (clearance, widths, via sizes / pre-defined vias, edge clearance, netclasses, custom rules, layer types, locks, via protection). Keys whose engine feature lands in M4a (`exit_stubs`, `steiner`, `dangling_ends`, `via_relaxation`, `preferred_direction`, `direction_cost`, `pour_cut_cost_mm`, `cost_by_*`, `allowed_drills_mm`, `via_in_pad`, `strict_unfit_edges`) carry `since = "0.5.0"` and are written as commented-out lines with "(available from 0.5.0)" until then; the engine-field test skips them while absent.
  `load_config(project_dir: Path) -> tuple[Config, list[Finding]]` (missing → `write_default` + `CFG_CREATED`; unknown key → `CFG_UNKNOWN_KEY`; type/range → `CFG_VALUE`; `version` ≠ 1 → `CFG_VERSION`); `Config.engine_options() -> dict` (mm → nm, s, `engine_field` mapping); `Config.non_defaults()`; `render_toml(cfg)` (every key preceded by `# <doc> [unit] default <d>, range <r>`; KiCad-only lines `# <name>: KiCad → <place> (not set here)`); `update_config(path, changes)` (keeps comments); `render_reference_md(SETTINGS)`; CLI `python -m topor.docs_gen --check`.

- [ ] **Step 1:** `test_default_file_roundtrips_to_defaults`, `test_unknown_key_warns_not_ignored`, `test_value_out_of_range_finding`, `test_engine_options_units` (`timeout_s 60 → wall_clock_s 60.0`, `via_cost_mm 2.0 → via_cost_nm 2_000_000`, `margin_um 5 → margin_nm 5000`, `timeout_s 0 → 0.0`), `test_update_config_keeps_comments`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `test_every_setting_documented`, `test_reference_md_matches_committed_file`, `test_every_engine_option_is_a_setting_or_internal` (`json.loads(engine.default_options()).keys()` ⊆ `{s.engine_field} ∪ ENGINE_INTERNAL`, `ENGINE_INTERNAL = {"pinned", "drc_spots", "skip_self_check", "profile", "seed", "scope", "net_priority", "via_cost_by_net", "detour_reference_budget_pct"}` with one-line reasons; every other `Options` field is a Setting's `engine_field`), `test_toml_comments_name_kicad_places`. Generate `docs/user/configuration.md`. Commit `feat(plugin): settings schema, topor.toml, generated config reference`.

### Task B2 (S): ports and fakes; `IpcBoard` ids/selection/`save_as`/commit/refill/run_action; kipy surface test — `plugin/topor/ports.py`, `fake.py`, `ipc.py`

**Files:** Create `ports.py`, `fake.py`, `ipc.py`; tests `tests/plugin/test_fake_board.py`, `test_ipc_mapping.py`, `test_kipy_surface.py`.

**Interfaces:**
- Produces:
  ```python
  class BoardPort(Protocol):
      def project_dir(self) -> Path
      def board_name(self) -> str
      def save_copy(self, dst_dir: Path) -> Path        # Board.save_as(str(dst / name), overwrite=True, include_project=True) → .kicad_pcb path
      def selection(self) -> Selection                  # pads: list[PadRef], footprints: list[str], tracks/vias: list[str], area: list[Pt] | None
      def read_model(self, rules: dict, pcb_copy: Path) -> dict   # BoardModel dict (B3)
      def netclasses(self) -> tuple[dict[str, NetClassValues], dict[str, str]]   # by class name; net → class name
      def commit(self, req: CommitRequest) -> CommitResult        # remove_uuids, items (engine Item dicts), message, group
      def refill_zones(self) -> None
      def run_action(self, action: str) -> None
  class UiPort(Protocol):
      def first_run_notice(self, toml_path: Path) -> None
      def preflight(self, findings: list[Finding], summary: RunSummary, cfg: Config) -> Decision   # proceed, accepted_codes, overrides (scope_mode, existing_copper, respect_locked, partial_routes, timeout_s, threads, save_to_toml)
      def progress(self, p: Progress) -> bool           # False = cancelled
      def preview(self, png: Path, report: Report) -> bool
      def error(self, finding: Finding) -> None
  @dataclass class Finding: code: str; severity: Literal["error","warn","info"]; message: str; fix: str; items: list[str] = field(default_factory=list)
  ```
  `FakeBoard(pcb: Path, model: dict, *, live_text: str | None = None)`: `save_copy` writes `live_text or pcb.read_text()` (so tests can make the live state differ from disk), `read_model` returns `model` (tests build it with the harness oracle), `selection` settable, `commit` records requests and applies them to an in-memory item list (`toporlib.pcbfile` on its text, so a second `save_copy` shows the result), `refill_zones`/`run_action` recorded. `ScriptedUi(decisions, accept, cancel_at_iteration)`.
  `IpcBoard(kicad: kipy.KiCad)`: `project_dir` from `board.get_project().path`; `save_copy` via `Board.save_as`; `selection` via `get_selection()` (pads, footprints, tracks/vias; a selected rule-area zone or polygon shape → `area`); `netclasses` via `Project.get_net_classes()` + `Board.get_netclass_for_nets(get_nets())`; `commit` = `begin_commit()` → `remove_items_by_id([KIID…])` → `create_items([Track|ArcTrack|Via…])` → `push_commit(commit, message)`; `refill_zones()`; `run_action(name)`. Groups (`topor`/`topor-partial`): UNVERIFIED in kipy 0.8 → `CommitResult.grouped = False` + report line when unavailable.
  `test_kipy_surface`: asserts, against the installed kipy source via `inspect`, the exact names/signatures used: `Board.save_as(filename, overwrite, include_project)`, `begin_commit() -> Commit`, `push_commit(commit, message)`, `drop_commit`, `create_items`, `remove_items_by_id`, `get_selection(types)`, `get_tracks/get_vias/get_pads/get_footprints/get_shapes/get_zones/get_nets`, `get_netclass_for_nets`, `get_pad_shapes_as_polygons`, `get_stackup`, `get_enabled_layers`, `get_layer_name`, `refill_zones`, `KiCad.run_action(action)`, `Project.get_net_classes/path`, `Zone.filled_polygons/outline/layers/is_rule_area/net/name/locked`, proto `Zone.rule_area_settings.{keepout_tracks, keepout_vias}`, `Footprint.pads`, `FootprintInstance.{reference_field, position, orientation, locked, definition}`, `Pad.{position, padstack, number, net, pad_type}`, `PadStack.{copper_layers, drill, angle}`, `PadStackLayer.{shape, size, offset, layers?}`, `Track.{start,end,width,layer,net,locked,id}`, `ArcTrack.{start,mid,end,…}`, `Via.{position, padstack, drill_diameter, …}`, `NetClass.{clearance, track_width, via_diameter, via_drill, name}`. Any missing name fails the test → the task adapts and records the real name.

- [ ] **Step 1:** `test_kipy_surface` (run inside the KiCad plugin venv interpreter path discovered from `harness.paths.venv_dir()`; skipped if absent). Fix names until green.
- [ ] **Step 2:** `test_fake_save_copy_uses_live_text`, `test_fake_commit_applies_and_records`, `test_scripted_ui_records_calls`; `test_item_to_kipy_track_arc_via` (engine `seg`/`arc`/`via` → kipy objects, nm, width, layer id, net name; pure `to_kipy_item(item, layers, nets)`), `test_selection_from_kipy_items`. Run → FAIL. Implement. PASS. `test_ipc_live_smoke` (only with `KICAD_API_SOCKET`): `save_copy` produces a parsable file. Commit `feat(plugin): board/ui ports, FakeBoard, ScriptedUi, IpcBoard, kipy surface test`.

### Task B3 (S, F review at the gate): IPC model exporter, pure rule resolution, s-expression board file editing — `plugin/topor/model_ipc.py`, `toporlib/rules.py`, `toporlib/dru.py`, `toporlib/pcbfile.py`

**Files:** Create `plugin/topor/model_ipc.py`, `toporlib/rules.py`, `toporlib/pcbfile.py`; modify `toporlib/dru.py` (`disallow` keepouts), `harness/rules.py` (SWIG wrapper calls the pure function), `harness/model.py` (`_export_keepouts` merges dru keepouts); tests `tests/plugin/test_model_ipc.py`, `test_pcbfile.py`, `test_ipc_parity.py` (live), `tests/test_rules.py`.

**Interfaces:**
- Produces: `export_model_ipc(board: kipy.board.Board, project, pcb_copy_text: str, rules: dict, *, name: str) -> dict` — same schema as `harness.model.export_model` (`units, board, layers, nets, pads, vias, kept, copper_shapes, holes, outline, keepouts, net_ties, pours, dangling_ends, rules`): layers = copper layers from `get_enabled_layers()` with `kind` from `pcbfile.parse_layers(pcb_copy_text)` (`signal|power|mixed|jumper`; `routable = kind in ("signal", "mixed")`, `[layers] route` override applied by the caller); pads from `get_footprints()` → `definition.pads` (position, rotation = footprint orientation + padstack angle, per-layer `PadShape` from `PadStackLayer.shape/size/offset` for circle/rect/roundrect/oval, `get_pad_shapes_as_polygons` for trapezoid/chamfered/custom → `Custom { poly }`, drill from `PadStack.drill`, `attr` from `pad_type`, local clearance/zone settings, net tie groups from `definition.net_ties`); `kept`/`vias` from `get_tracks()`/`get_vias()` (incl. `locked`); `copper_shapes` from `get_shapes()` on copper layers and footprint shapes; `outline` from Edge.Cuts shapes (arcs/Béziers polygonised at `max_error`, same as the harness); `keepouts` from rule-area zones (`zone.proto.rule_area_settings.keepout_tracks/keepout_vias`, board + footprint-embedded) + dru `disallow`; `pours` from copper zones (`filled_polygons` → `fills` outer + holes, pads/vias reached after refill computed geometrically as the harness does); `dangling_ends` by the same `(net, layer)` bucket rule as `harness.model.find_dangling_ends`.
  `toporlib.rules.resolve_rules_from_project(pro: dict, dru_text: str | None, *, netclass_of: dict[str, str], classes: dict[str, NetClassValues], widths_before, settings_table, profile, apply_fab_floors, copper_layer_count) -> dict` — today's `resolve_rules` minus SWIG (`board.design_settings.rules.*`, `net_settings`, `netclass_patterns` from the JSON; effective class per net from `netclass_of` = IPC's `get_netclass_for_nets`, which answers 44's aggregation question); `harness.rules.resolve_rules(board, …)` builds its inputs with SWIG and calls it.
  `toporlib.pcbfile`: `parse_layers(text) -> list[dict]`; `find_items(text, kinds=("segment","arc","via")) -> list[ItemSpan(kind, uuid, start, end)]` (balanced-paren scan, `O(len)`); `remove_items(text, uuids) -> str`; `append_items(text, items: list[dict], *, layer_names: dict[int, str]) -> str` (engine `seg`/`arc`/`via` dicts → `(segment (start x y) (end x y) (width w) (layer "F.Cu") (net "NAME") (uuid "…"))`, `(arc (start)(mid)(end)…)`, `(via (at x y) (size d) (drill dr) (layers "F.Cu" "B.Cu") (net "NAME") (uuid "…"))`, mm with 6 decimals from nm, inserted before the final `)`; KiCad 10 nets by name as in the snapshot files).

- [ ] **Step 1:** `test_pcbfile_roundtrip_on_snapshot` (eprom snapshot text: `find_items` count == number of `(segment|arc|via` forms; `remove_items` of 3 uuids leaves the rest byte-identical; `append_items` of a seg/arc/via → `kicad-cli pcb drc` parses the file (exit 0 or 5, no parse error) and the harness oracle `pcbnew.LoadBoard` (test-only) reads back the 3 items with the exact nm coordinates). Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `test_resolve_rules_pure_equals_swig` (fixture project: identical dict). Refactor. PASS; `test_dru_disallow_rule_becomes_keepout`.
- [ ] **Step 3:** `test_model_ipc_from_fixture_protos` (pure mapping functions `pad_from_kipy`, `zone_to_pour`, `zone_to_keepout`, `shape_to_prims` fed with kipy wrappers built from protos: exact nm values). `test_ipc_parity` (needs a running KiCad with the board open; parametrised by snapshot board; skips otherwise): IPC model equals the harness model on every section after canonical sorting; tolerance 0 nm on coordinates, pad polygons compared as hulls. Record the per-board result in the ledger; parity on **all** snapshot boards is a gate item (user runs them per the GUI checklist). Commit `feat(plugin): IPC model exporter, pure rules, pcbfile s-expression editing`.

### Task B4 (F design + F review): UI design — wireframes for every dialog — `docs/progress/notes/m4-ui-wireframes.md`

**Files:** Create the note. Before writing, invoke via the Skill tool: `ecc:frontend-design-direction`, then `ecc:make-interfaces-feel-better`, then `ecc:accessibility`; adapt to wxPython inside KiCad (native widgets, KiCad dialog look, light/dark theme from `wx.SystemSettings`, HiDPI via `FromDIP`).

**Interfaces:**
- Produces ASCII wireframes + behaviour notes for: (1) **first-run `topor.toml` notice** (file created at `<project>/topor.toml`, "Open file" / "Continue"); (2) **options + pre-flight dialog** (findings grouped by severity with cause and fix, scope mode, existing copper, respect locked, partial routes, timeout, threads, "Save to topor.toml", default button **Route**, Esc = Cancel; errors disable Route and say why); (3) **progress** (phase, iteration, routed/total, vias, elapsed, indeterminate gauge during metrization, Cancel = keep best so far); (4) **preview Accept/Reject** (current vs result side by side, both layers colour-coded, new copper highlighted, ≤ 20 summary lines, "Open report folder", default **Accept**, Esc = Reject); (5) **run report** (`report.md` structure + the summary shown in the preview); (6) **error dialog** (code, cause, fix, "Copy details"). Each: focus order, keyboard map, min sizes at 100 %/200 %, dark-theme colours, wording table (every label/button text), states (empty, busy, error), `UiPort` method it implements.

- [ ] **Step 1:** write the note (≤ 4 pages); **Step 2:** independent F review (checklist: the three skills' criteria, KiCad look, keyboard, scaling, wording) → fixes; **Step 3:** commit `docs(ui): M4 dialog wireframes (reviewed)`.

### Task B5 (S): run flow — route with progress/cancel, DRC loop + preview on a `save_as` copy, Accept → single commit — `plugin/topor/run.py`, `preview.py`

**Files:** Create `run.py`, `preview.py`; tests `tests/plugin/test_run_flow.py`.

**Interfaces:**
- Consumes: `toporlib.drcloop.route_with_drc_loop` (B0), `topor_engine.route_async` (P1), `toporlib.pcbfile` (B3), `toporlib.drc.run_drc/classify` (B0), `model_ipc.export_model_ipc` (B3), `preflight` (B6; whole-board until then), `scope` (B7), `report` (B8; minimal until then).
- Produces: `run(port: BoardPort, ui: UiPort, cfg: Config, engine, *, tmp_root: Path) -> RunOutcome` (`status ∈ {"aborted_preflight", "cancelled", "rejected", "accepted", "error"}`, `report`, `commit`):
  1. `load_config` (first run → `ui.first_run_notice`) → `pcb_copy = port.save_copy(tmp)` → rules (`.kicad_pro` next to the copy, `.kicad_dru`, `port.netclasses()`) → `model = port.read_model(rules, pcb_copy)` → `preflight` → `ui.preflight` (error finding or `proceed == False` → `aborted_preflight`; accepted `PAD_OUTSIDE_BOARD`/`DRILL_OUTSIDE_FILTER` → `update_config`).
  2. `rip_plan` (B7) → `pcbfile.remove_items(copy_text, ripped)` → model `kept` filtered accordingly; `Options` = `cfg.engine_options()` + scope + `net_priority` + `exclude_nets` + `ignore_pads` + `threads` + overrides from the dialog.
  3. `engine.route_async(...)`; poll `progress()` every 0.25 s → `ui.progress(p)`; `False` → `job.cancel()`; `wait()`; DRC loop with `drc_fn(items) = write temp file (pcbfile.append_items) → run_drc(kicad-cli --refill-zones) → classify` under the remaining budget (M3-R8); `geometry == "segments"` → `preview.arcs_to_segments(items, max_error)`.
  4. `preview.render(before_pcb, after_pcb, out_png, caption) -> Path`: `kicad-cli pcb export svg --layers F.Cu,B.Cu,Edge.Cuts --exclude-drawing-sheet --page-size-mode 2` (board area; options verified in `kicad-cli 10.0.8 pcb export svg --help`) for both files → `wx.svg.SVGimage.CreateFromFile(...).ConvertToScaledBitmap(size)` side by side in a `wx.MemoryDC` with the caption lines and new copper tinted (items drawn from the result on top) → PNG; `ui.preview(png, report)`; `False` → `rejected` (no commit, no refill).
  5. `port.commit(CommitRequest(remove_uuids=ripped, items=items, message=f"Topor: routed {n}/{total}", group="topor"))` (partial routes → `topor-partial`, same commit); `port.refill_zones()`; `kicad_cleanup`/`kicad_teardrops` → `port.run_action("pcbnew.GlobalEdit.cleanupTracksAndVias" / "pcbnew.GlobalEdit.editTeardrops")` (skipped with `CLEANUP_SKIPPED_PARTIAL` when partial routes exist, 30 §4.2); `report.write(project_dir / cfg.report_dir / <timestamp>/)` (report.md, preview.png, result.json, drc.json, log.txt).
  Exceptions → `ui.error(Finding("INTERNAL", …, fix="see <report_dir>/…/log.txt; file a bug with the save_as copy (docs/dev/debug.md)"))`, `status == "error"`; `BaseException` (PyO3 panic) caught in `app.main` (31 §6).

- [ ] **Step 1:** tests on `FakeBoard` (eprom fixture + harness-built model) + `ScriptedUi`, real engine, real `kicad-cli`: `test_run_uses_save_as_copy_not_disk_file`, `test_run_accept_single_commit` (one `CommitRequest` with every ripped uuid and every routed item; a second `save_copy` contains them), `test_run_reject_leaves_board_untouched`, `test_run_cancel_keeps_best_result_and_previews`, `test_run_aborts_on_error_finding` (engine never called), `test_run_segments_geometry_has_no_arcs`, `test_run_partial_routes_group`, `test_run_engine_panic_reported_not_crashed`, `test_run_preview_png_written_with_caption` (PNG exists, size > 0, `report.summary_lines()` ≤ 20). Run → FAIL. Implement. PASS. Commit `feat(plugin): run flow — save_as copy, route with progress/cancel, DRC loop, svg preview, single commit`.

### Task B6 (S): pre-flight checks and messages — `plugin/topor/preflight.py`, `messages.py`

**Files:** Create both; tests `tests/plugin/test_preflight.py`, `test_messages.py`.

**Interfaces:**
- Produces: `preflight(model, rules, cfg, sel: Selection, orig_drc: DrcReport, cfg_findings) -> list[Finding]`, codes (each in `MESSAGES` with cause + fix): `ENGINE_VERSION`, `KICAD_CLI_MISSING`, `CFG_CREATED/CFG_UNKNOWN_KEY/CFG_VALUE/CFG_VERSION/CFG_VALUE_BACKLOG`, `VIA_NOT_SET_UP` (warn; fallback 0.8/0.4; fix: Board Setup → Pre-defined Sizes → Vias or `[vias] fallback_*`), `DRILL_OUTSIDE_FILTER` (accept → `update_config`), `VIA_IN_PAD_UNPROTECTED` (`via_in_pad` while `.kicad_pro` via protection is not filled + capped — JSON key names UNVERIFIED, documented), `POWER_BY_NAME`, `MARKER_IGNORED/MARKER_CONFLICT/MARKER_PAIR` (B7), `PAD_OUTSIDE_BOARD` (pad centre outside the Edge.Cuts polygon; accept → `ignore_pads`), `PREEXISTING_DRC` (info; error when `fail_on_preexisting`; from `kicad-cli` on the untouched copy), `TRACK_ANGLE_RULE` (error), `LOCKED_COPPER`, `DANGLING_ENDS`, `DO_NOT_TOUCH`, `LAYER_TYPES`, `SCOPE_EMPTY`, `NON_DEFAULTS`, `DEFAULTS_USED`, `GROUPS_UNAVAILABLE`. `RunSummary` = connections in scope, nets, layers, threads, timeout.

- [ ] **Step 1:** one test per code on fixture models (pad 1 mm outside the outline; dru with `track_angle`; factory-default vias; `allowed_drills_mm = [0.3]` with a 0.4 drill). Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `test_every_code_has_cause_and_fix` (fix names a KiCad menu path or a `SETTINGS` key). Wire into `run.py`. Commit `feat(plugin): pre-flight findings and actionable messages`.

### Task B7 (S): routing scope from the selection, copper/lock policies, priorities (`@N`, `topor_pN`, power pattern) — `plugin/topor/scope.py`, `priority.py`

**Files:** Create both; tests `tests/plugin/test_scope.py`, `test_priority.py`.

**Interfaces:**
- Produces: `scope_from_selection(sel, model, cfg) -> ScopePlan` (`engine_scope` = `Scope {pads, footprints, nets, netclasses, area, unrouted_only}`; `between` = connections whose pads are all on selected footprints/pads, `touching` = at least one; empty selection → whole board); `rip_plan(model, plan, cfg) -> RipPlan` (`remove_uuids` by `existing_copper`: `fixed` none; `rip` in-scope nets' tracks/vias except locked when `respect_locked` and except `do_not_touch`; `smooth` → engine `existing_copper = "smooth"`).
  `parse_marker(net_name) -> int | None` (46 §2.1); `net_priorities(nets, netclass_of, cfg) -> (dict[str, int], list[Finding])` (marker > `topor_pN` > `[nets] priority` > none; conflicts → `MARKER_CONFLICT` marker wins; merged index → `MARKER_IGNORED`; diff-pair halves → `MARKER_PAIR` higher wins; power nets by netclass name containing `power` or `power_pattern` for nets in `Default` → tier 2 unless set, `POWER_BY_NAME`).

- [ ] **Step 1:** `test_marker_parse_table` (`USB_D@1+`→1, `CLK@1_P`→1, `DQ@1_3`→1, `DQ@13`→None + ignored, `/ddr/DQ@1_0`→1, `~{RESET}@2`→2, `~{RESET@2}`→2, `A@4`→None, `@2`→2), `test_priority_sources_and_conflicts`, `test_power_pattern_only_for_default_netclass`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `test_scope_between_vs_touching`, `test_scope_pads_two_pins`, `test_scope_area_polygon`, `test_scope_unrouted_only_keeps_copper`, `test_rip_plan_policies`. Wire into `run.py`. Commit `feat(plugin): scope from selection, copper policies, @N priorities`.

### Task B8 (S): run report — `plugin/topor/report.py`

**Files:** Create `report.py`; tests `tests/plugin/test_report.py`. Layout per the B4 wireframe (5).

- [ ] **Step 1:** `build_report(result, drc, cfg, findings, scope, before_stats, timing) -> Report` (summary: routed/total, nets, partial/failed with reasons and blocking points, vias per net vs current board, length vs current, stop reason, elapsed, threads; DRC `routing_new` by type / `preexisting` / unconnected; detours; zero-slack; vias in pads; partial routes; ignored pads; accepted fallbacks; non-defaults and defaults used; findings), `Report.summary_lines() ≤ 20`, `to_markdown()`, `write_report(dir, …)`. Tests `test_report_sections_present_and_values`, `test_report_summary_lines_le_20`. Commit `feat(plugin): run report`.

### Task B9 (S, from the B4 wireframes): wx UI — `plugin/topor/ui_wx.py`

**Files:** Create `ui_wx.py`; tests `tests/plugin/test_ui_wx.py`. Before implementing, invoke `ecc:make-interfaces-feel-better` and `ecc:accessibility` via the Skill tool and follow the B4 note verbatim (wording table, focus order, keyboard map, sizes via `FromDIP`, system colours for dark theme).

- [ ] **Step 1:** `WxUi(UiPort)` implementing dialogs (1)–(6); `test_wx_ui_dialogs_construct_headless` (`wx.App(False)` with a virtual display if `DISPLAY` is unset is not required; skip without `wx`), `test_wx_ui_wording_matches_wireframes` (labels/buttons equal the B4 wording table parsed from the note). Commit `feat(plugin): wx dialogs from the reviewed wireframes`.

### Task B10 (S): entry point, app wiring, `--headless`, build/install — `plugin/topor_plugin.py`, `plugin/topor/app.py`, `plugin/plugin.json`, `tools/build.sh`

**Files:** Modify `plugin/topor_plugin.py`, `plugin/plugin.json` (`version` 0.4.0, `min_version` `"3.11"`, action name "Topor: route", description "Route the board or the selection with Topor"), `tools/build.sh` (ships `plugin/topor/`, `toporlib/`, `docs/user/`; `requirements.txt` unchanged: `--find-links wheels`, `kicad-python>=0.8,<0.9`, `topor-engine==<version>`); create `app.py`; tests `tests/test_scaffold.py`, `tests/plugin/test_app.py`.

- [ ] **Step 1:** `app.main(argv)`: handshake, `KiCad(socket_path=env KICAD_API_SOCKET, kicad_token=env KICAD_API_TOKEN, client_name="topor")`, `get_board()`, `IpcBoard`, `WxUi`, `run(...)`; `--headless <pcb> --model <json> [--accept] [--selection JSON]` on `FakeBoard` + `ScriptedUi`; all `BaseException` → stderr + exit 1. Tests `test_build_layout_in_scratch_kicad_home` (`KICAD_DOCUMENTS_HOME=<tmp>`; real plugin dir untouched; no `harness/` shipped), `test_headless_run_on_fixture_accepts`, `test_no_pcbnew` now imports `topor.app`. Commit `feat(plugin): entry point, app wiring, headless mode, build/install`.
- [ ] **Step 2 (M4b-α, user):** `bash tools/build.sh --dev`; the user routes **copies** of eprom and wiccon from KiCad (`harness.copy.copy_project` into `scratch/gui/<board>/`) following the draft checklist; result recorded in STATE.

### Task B11 (S; H mechanical): user guide, GUI test checklist, CONTRIBUTING, debug guide — `docs/user/`, `docs/dev/`, `CONTRIBUTING.md`, `README.md`

**Files:** `docs/user/install.md` (build script, wheel, enable the KiCad API server, plugin environment, "Recreate Plugin Environment", Python ≥ 3.11), `operate.md` (first run, options dialog, scope by selection, findings and what to do, progress/cancel, preview, Accept/Reject, undo, report folder, limitations: Linux, two layers, groups), `troubleshoot.md` (`### <CODE>` per `MESSAGES` code + "exited with code N", wheel/venv issues), `gui-test-checklist.md` (eprom and wiccon copies: run → first-run notice → pre-flight contents → progress + Cancel once → preview → Reject (board unchanged, Undo greyed) → run again → Accept → one Ctrl+Z reverts all → KiCad DRC 0 new → report folder → selection scope `between` on two footprints; **UI acceptance:** tab order per wireframe, Esc = Cancel, Enter = default button, readable at 100 % and 200 % scaling, dark theme; **IPC parity:** run `test_ipc_parity` with each snapshot board open; **UNVERIFIED items** from B2/B3/B5/B6 (groups, via-protection keys, `run_action` names); results table), `docs/dev/debug.md` (`route_probe`, `gate_diag --detour`, `--profile`, progress PNGs, `harness.measure`, `app --headless`, reproducing a plugin run from `result.json` + the `save_as` copy, filing a bug), `CONTRIBUTING.md` (layout incl. `toporlib/`, build, tests, harness on copies, read-only projects, commit style, schema → toml → docs); `README.md` + `docs/README.md` updates; tests `test_troubleshoot_has_section_per_code`, `tests/test_docs_links.py`.

- [ ] **Step 1:** tests → FAIL; write docs; PASS; `python -m topor.docs_gen --check` clean. Commit `docs(user): install/operate/troubleshoot/GUI checklist; CONTRIBUTING and debug guide`.

### Task B12 (S, gate review F): M4b gate — headless e2e, GUI checklist, parity, version 0.4.0, review

**Files:** `tests/gate/test_m4b_gate.py`, `engine/Cargo.toml` + `plugin.json` + `SHIPPED_VERSION` (0.4.0), `docs/progress/notes/m4b-gate-review.md`, `docs/design/45-mvp-plan.md` (M4 row: split + versions recorded), `README.md` status, STATE.

- [ ] **Step 1:** `test_m4b_gate_headless_eprom_and_wiccon` (marked `gate`): `FakeBoard` on snapshot copies with harness-built models, `ScriptedUi(accept=True)`, real engine: `status == "accepted"`, one commit, eprom 27/27 / wiccon 62/62 routed, `routing_new == 0` and `unconnected == 0` by `kicad-cli` on the committed copy, report dir written, `topor.toml` created, `docs_gen --check` clean; `test_m4b_gate_scope_selection`. PASS. `pytest tests/gate -m "gate or not gate"` green (M0–M3 + M4b).
- [ ] **Step 2 (user):** GUI checklist on eprom and wiccon copies incl. UI acceptance and `test_ipc_parity` on every snapshot board; results in STATE. Failures → fix task, re-run.
- [ ] **Step 3 (F):** review: port boundaries (kipy only in `ipc.py`/`model_ipc.py`; no pcbnew), every 46 setting present with its source order, message coverage, exception paths, undo atomicity, UI vs wireframes, docs vs behaviour; BLOCKING fixed → STATE "M4b DONE (0.4.0)". Commit `docs(state): M4b DONE (0.4.0)`.

---

## Part 2 — M4a: engine (version 0.5.0)

### Task A0 (S; H sub-steps): M3 gate-review minors D-1..D-10 + harness M4 prep — `multigraph.rs`, `pourcheck.rs`, `route.rs`, `search.rs`, tests, `harness/snapshot.py`, `harness/progress.py`, `harness/bench.py`

- [ ] **Step 1 (D-4):** `site_inside_fill_hole_is_not_in_pour` (`engine/tests/multigraph.rs`); implement point-in-`outer`-minus-`holes`, `O(|outer| + Σ|holes|)` per site.
- [ ] **Step 2 (D-9):** `pourcheck_long_diagonal_track_stamps_linear` (`stamped_cells ≤ 4·L/res·w/res + 64`); scanline per row, `STAMP_CELL_BUDGET = 16_000_000`, deadline per pour.
- [ ] **Step 3 (D-10, H):** `BTreeSet::pop_first` free list. **(D-1):** `Options.strict_unfit_edges` (default `false`; `!fits(e)` rejects in `push_state`), test `strict_unfit_edge_rejected_in_search`; two-layer eprom run with it on recorded in the ledger (A4 input).
- [ ] **Step 4 (D-2, D-3, D-5, H):** split-vertex fixture asserts `"failed"` + reason prefix `"split vertex"`; e2e `rule_t_via_counter_example`; assert the real warning text `"without pour after"`. **(D-6):** `pour_connection_uses_listed_same_net_via_as_goal`. **(D-7, H):** note §7 amendment. **(D-8):** `test_snapshot_existing_pin_survives_source_edit`.
- [ ] **Step 5 (harness):** `render_progress(task, *, milestone="m4", …)` → `<date>-<milestone>-<task>[-<board>].png`, caption adds `stop_reason`; bench summary/README columns `detour` (reference), `detour_ghost`, `stop_reason`, `vias_per_net` (filled by A7/P1 when present); `test_progress_m4_name_and_caption_lines`.
- [ ] **Step 6:** cargo/clippy/pytest green; eprom/wiccon byte-identical. Commit `fix(engine): M3 review minors D-1..D-10; harness m4 progress and bench columns`.

### Task A1 (S, F review at the gate): slot-search speed — face clear radius, memoised `clearance_bend`, early stop — `engine/src/search.rs`, `engine/src/graph.rs`

**Interfaces:** `RouteGraph.clear_r[f]` = distance from face `f`'s incentre to the nearest vertex not of `f` and nearest wall (one grid query per face at build, `O(F·q)`); `clearance_bend` (a) skips the candidate scan for a sub-chord between consecutive portals whose endpoints both lie inside `D(incentre(f), clear_r[f] − ρ)` (convex disk ⇒ every point of the sub-chord is ≥ ρ from every non-face obstacle; the face's own 3 vertices are still tested exactly), (b) memoises per search on `(portal edge, side, apex vertex, target vertex)` (`RefCell<HashMap>`, membership only), (c) tests candidates in ascending chord parameter and stops at the first violation. Per call `O((C+1)·hit)`. Counters in `search_totals()`: `bends`, `candidates`, `skipped_faces`, `memo_hits`.

- [ ] **Step 1:** `clear_radius_skip_is_conservative` (proptest 200 cases), `memoised_bend_equals_recomputed`, `early_stop_bend_equals_full_scan` (`engine/tests/search_speed.rs`). FAIL → implement → PASS.
- [ ] **Step 2:** measure on dcsgone (`route_probe … --profile`): candidates/query (was 162) and search share (was 94 %) in the ledger; target `≥ 5×`; `< 2×` → stop and report. eprom/wiccon byte-identical; dcsgone 60 s ≥ 148. PNG `--task A1 --board dcsgonebadge`. Commit `perf(search): face clear radius, memoised clearance_bend, early stop`.

### Task A2 (S, F review at the gate): fine-pitch entry #201 and state-point / los convergence (M4-D2) — `engine/src/search.rs:1373, :1758`

**Interfaces:** `state_point` shrinks the usable sub-segment by every foreign vertex within `r_v + c + w/2 + eps` (`verts_near`, `O(q)`) and discretises roundrect-corner crossings at the corner-circle tangent point; `los` returns `ok = false` when the bend loop does not converge (apex not approaching within `LOS_PORTAL_CAP` or cap hit).

- [ ] **Step 1:** `fine_pitch_entry_u7_4_routes_direct` (U1.13→U7.4 geometry extracted via `gate_diag --detour 201` into a fixture; 0 vias, `est_len ≤ 1.5 × straight`), `los_nonconverging_bend_loop_is_rejected` (#208 trio: 0-via path `≤ 1.3 × 2.93 mm`, `apex_len ≤ 4 × chord`) in `engine/tests/search_entry.rs`. FAIL → implement → PASS.
- [ ] **Step 2:** eprom byte-identical or not worse (diff reported); wiccon not worse. PNG `--task A2 --board dcsgonebadge`. Commit `fix(search): fine-pitch entry, state point shrink, los convergence`.

### Task A3 (F): design note — own-net capacity, same-net Steiner attachment, exit stubs, G1 continuation, via relaxation, escalating rip cost — `docs/progress/notes/m4-capacity-steiner-design.md`

Read: `m3-detour-diagnosis.md` (M4-D1, D3, withdrawn F6), `graph.rs:462-500`, `wires.rs`, `relax.rs`, `negotiate.rs:36-58, 470-495`, `22 §T-junctions`, `25 §3`, `12 §1`. The note (≤ 3 pages) fixes: (1) **own-net / same-net capacity, wall-aware:** own-net ends and same-net neighbours charge no gap and give the core radius back; an edge with an own-net end also respects walls that are not edge ends via a per-edge wall query cached at build (`O(E·q)`) so the U2.28 outline-wall case stays unfit; exact `cap`/`need` formulas; which diagnosis tests flip. (2) **Same-net attachment (Steiner):** goal `Goal::OwnStrand(conn, leg, edge)` on committed legs of earlier connections of the net and on kept hand copper; junction angle ≥ 60°; the sibling wire is split at the junction into two wires sharing a `Terminal::Junction` (as `wires.rs` does for hand copper); per-net star/path ordering by est length (`O(p log p)`); bounded 120° junction relaxation. (3) **Pad exit stubs:** `Terminal::Stub { pad, end, layer }`, straight stub of net width from the pad centre to `hull + c + w/2 + eps` along the long axis away from the nearest unfit neighbour; `auto` (own tip/spoke edges unfit) / `all` / `off`; `Checker`-verified before use. (4) **G1 continuation:** `Terminal::Dangling { at, direction }`; metrization rails of radius `w/2 + eps` at `at ± n̂(w/2 + c)` for `STUB_LEN = max(2w, 0.5 mm)`; the hand trace's crossing sides are a fixed topology hint; `g1_angle_deg` ≤ 1°. (5) **Via position relaxation (bounded Weiszfeld):** ≤ `VIA_RELAX_STEPS = 8` steps toward the two legs' force balance, confined to the union of faces incident to the site vertex on both layers, accepted iff `via_fits` and both legs re-metrize + self-check clean and the length decreases; `O(V_via · steps · (metrize + q))`. (6) **Escalating per-pair rip cost (46 row 17):** k-th rip of B by A adds `hist_block · growth^(k−1)` (`growth = 2.0`), backstop `max_per_pair = 8`; check of the history-cost scale vs length costs: geometric sum `hist_block·(growth^k − 1)/(growth − 1)` vs `hist_fail · k` — a three-way contention (F.Cu-only eprom +5V) must still settle, ping-pong pairs priced out before the backstop; bound `O(max_per_pair · m²)`; report fields. (7) Complexity table and the test per rule (A4–A6, A13, A14, A16).

- [ ] **Step 1:** write; **Step 2:** commit `docs(design): M4 capacity/Steiner/stubs/G1/via-relax/rip-cost note`; send to main.

### Task A4 (S): own-net / same-net capacity, wall-aware (M4-D1) — `graph.rs`, `search.rs`

**Interfaces:** `RouteGraph::fits_for(e, net, w, c) -> bool`, `RouteGraph.wall_r: Vec<f64>`; own-net ends charge 0 gap + core radius back; same-net strands on `e` charge 0 gap. Build `O(E·q)`.

- [ ] **Step 1:** `own_tip_edge_fits_own_net`, `two_same_net_strands_share_one_tip_edge`, `own_end_edge_against_outline_wall_stays_unfit`, `search_layers (1)` flips to "fits, ghost not soft" (`engine/tests/graph_ownnet.rs`). FAIL → implement → PASS.
- [ ] **Step 2:** `eprom_ripped_front_only_is_the_m2_result` green; eprom two-layer identity re-verified; wiccon not worse (vias ≤ 33 expected). PNGs `--task A4` dcsgone + wiccon. Commit `feat(graph): own-net and same-net capacity, wall-aware`.

### Task A5 (S): same-net Steiner attachment and net ordering (M4-D3) — `decompose.rs`, `search.rs`, `route.rs`, `wires.rs`

**Interfaces:** `Goal::OwnStrand { conn, leg, edge }`; `ConnReport.junction: bool`; `Options.steiner: bool = true`. One extra goal set per sibling, `O(Σ sibling leg length)` per search.

- [ ] **Step 1:** `t_junction_onto_routed_sibling` (#207 fixture: ≤ 1.2 mm T-junction, 0 vias, sibling split, self-check clean), `t_junction_onto_hand_copper`, `steiner_off_is_m3_behaviour`, `junction_angle_below_60_is_rejected` (`engine/tests/route_steiner.rs`). FAIL → implement → PASS.
- [ ] **Step 2:** eprom/wiccon not worse; threads byte-identical; dcsgone 60 s `count > 1.5` reported (expected ≤ 6). PNG `--task A5 --board dcsgonebadge`. Commit `feat(route): same-net Steiner T-junctions and net ordering`.

### Task A6 (S): pad exit stubs — `route.rs`, `wires.rs`, `decompose.rs`, `model.rs`

**Interfaces:** `Terminal::Stub { pad: Vec2, end: Vec2, layer: u32 }`; `Options.exit_stubs: String` (`"auto" | "all" | "off"`, default `"auto"`); `ConnReport.stub_nm: i64`; generation `O(T·q)`.

- [ ] **Step 1:** `stub_generated_for_unfit_fine_pitch_pad`, `stub_skipped_when_infeasible` (reason `"stub infeasible"`), `exit_stubs_off_matches_previous`, `exit_stubs_all_on_every_smd_pad` (`engine/tests/route_stubs.rs`). FAIL → implement → PASS.
- [ ] **Step 2:** eprom byte-identical (THT only); wiccon not worse; dcsgone #201 routed. PNG `--task A6 --board dcsgonebadge`. Commit `feat(route): pad exit stubs (auto/all/off)`.

### Task A7 (S): DRC-feasible detour reference (M4-D4, M3-R15) — `route.rs`, `model.rs`, `toporlib/detour.py`, `harness/bench.py`, `harness/progress.py`

**Interfaces:** `Options.detour_reference: bool = false`, `Options.detour_reference_budget_pct: f64 = 10.0`; `ConnReport.ref_len_nm: Option<i64>`, `ConnReport.ref_kind: String` (`"feasible" | "ghost" | "none"`): one slot search per connection with full clearance/width on a `MultiGraph` of fixed copper only, same layers/via rules/`via_cost`, `par_iter` collected in order, under the budget share; failure → ghost fallback flagged. `O(m · search)`. `detour_stats(connections, field="ref_len_nm")` → `detour`, `ghost_len_nm` → `detour_ghost`; bench sets `detour_reference: true` (offline only); caption flags `ghost < 2 mm`.

- [ ] **Step 1:** `reference_is_feasible_not_topological` (#208: `ref ≈ 2.9 mm`, ghost 1.0), `reference_off_leaves_fields_none`, `reference_budget_exhausted_falls_back_to_ghost`, `reference_threads_byte_identical` (`engine/tests/route_detour_ref.rs`); `test_detour_stats_uses_reference_and_reports_ghost`. FAIL → implement → PASS.
- [ ] **Step 2:** PNG `--task A7 --board dcsgonebadge`. Commit `feat(route): DRC-feasible detour reference (M3-R15)`.

### Task A9 (S): priority tiers, do-not-touch nets, ignored pads — `decompose.rs`, `model.rs`, `route.rs`

**Interfaces:** `Options.net_priority: BTreeMap<String, u8>`, `exclude_nets: Vec<String>`, `ignore_pads: Vec<PadRef>`; `Connection.tier` = table value (1..4) else M3 inference; excluded nets never decomposed, their copper fixed (`NetRouteReport.status == "excluded"`); ignored pads → no terminal/obstacle, `ConnReport.status == "ignored"`, reason `"pad outside board"`.

- [ ] **Step 1:** `priority_table_orders_and_weights`, `tier_4_pays_lower_via_cost`, `excluded_net_copper_is_fixed_obstacle`, `ignored_pad_skipped_and_reported` (`engine/tests/route_priority.rs`). FAIL → implement → PASS. eprom/wiccon byte-identical without options. Commit `feat(route): priority tiers from options, do-not-touch nets, ignored pads`.

### Task A10 (S): via rules from the board setup — `vias.rs`, `model.rs`, `route.rs`, `toporlib/rules.py`

**Interfaces:** `Rules.predefined_vias: Vec<[i64; 2]>`, `Rules.via_fallback: [i64; 2] = [800_000, 400_000]`; `Options.allowed_drills_nm: Vec<i64>`, `via_in_pad: bool = false`, `via_cost_by_net: BTreeMap<String, i64>`, `reduction_len_slack_pct: f64 = 10.0`; `via_sizes(rules, net, opts)`: netclass via first, then smaller predefined vias (filtered, `≥ min_via_drill`), fallback only when the list is empty **and** the netclass is at KiCad factory values (warning `"via fallback 0.8/0.4 used"`); `RouteResult.vias_in_pads: Vec<[i64;2]>`; `toporlib.rules` exports `predefined_vias` from `.kicad_pro` `m_ViasDimensionsList` and the factory-default warning.

- [ ] **Step 1:** `via_sizes_prefer_netclass_then_predefined_smaller`, `allowed_drills_filters_predefined`, `fallback_only_when_nothing_set_up`, `via_in_pad_site_inside_own_smd_pad_only_when_enabled`, `reduction_slack_from_options`, `via_cost_by_net_overrides_default` (`engine/tests/via_rules.rs`); `test_rules_exports_predefined_vias_and_factory_default_warning`. FAIL → implement → PASS; eprom/wiccon byte-identical. Commit `feat(vias): predefined via list, fallback warning, allowed_drills, via-in-pad, per-net via cost`.

### Task A11 (S): layer cost terms — pour cut and preferred direction — `search.rs`, `route.rs`, `model.rs`

**Interfaces:** `Options.pour_cut_cost_nm: i64 = 1_000_000` per mm inside a foreign pour, `Options.layer_direction: BTreeMap<String, f64>`, `Options.direction_cost: f64 = 0.0`; additive step costs (admissible), `O(1)` per step via `in_pour` of the face.

- [ ] **Step 1:** `leg_through_foreign_pour_prefers_other_layer_when_cost_set`, `preferred_direction_cost_tilts_choice`, `costs_zero_match_m3` (`engine/tests/route_layer_cost.rs`). FAIL → implement → PASS. eprom B.Cu GND pour islands before/after reported. PNG `--task A11`. Commit `feat(search): pour-cut and preferred-direction layer costs`.

### Task A12 (S; F diagnosis if the gate misses): dcsgonebadge completion — M3-R14

- [ ] **Step 1:** `tests/gate/test_m4_gate.py::test_m4_gate_dcsgone_complete` with the Global Constraints numbers (locked-copper helpers reused; threads 1/2/4 + repeat; pcb sha printed). Run → record.
- [ ] **Step 2:** if failing: one F note `docs/progress/notes/m4-dcsgone-diagnosis.md` (where the remaining connections go: capacity, entry, time; per-iteration profile), fix tasks (S), re-run; still failing → commit, numbers + profile to STATE, escalate (no relaxation).
- [ ] **Step 3:** PNG `--task A12 --board dcsgonebadge`. Commit `test(gate): M4 dcsgone 231/231 (+ fixes)`.

### Task A13 (S): bounded via position relaxation (M3-R3) — `relax.rs`, `route.rs`

**Interfaces:** `Options.via_relaxation: bool = true`; `RouteResult.via_relaxation: String`; pass after via reduction per note §5.

- [ ] **Step 1:** `via_moves_toward_force_balance_within_face_star`, `via_relaxation_rejects_move_into_keepout`, `via_relaxation_off_matches_previous`, `via_relaxation_threads_byte_identical` (`engine/tests/route_via_relax.rs`). FAIL → implement → PASS. wiccon: via count unchanged, outside ≤ previous, DRC 0. PNG `--task A13 --board wiccon-2023`. Commit `feat(route): bounded via position relaxation`.

### Task A14 (S): finishing hand-started traces — G1 continuation — `wires.rs`, `relax.rs`, `route.rs`, `model.rs`, `harness/fixtures.py`

**Interfaces:** `Terminal::Dangling { at, direction }`; `Options.dangling_ends: String` (`"continue" | "fixed"`, default `"continue"`); `g1_angle_deg` computed; `make_fixture_board(..., dangling_trace=True)`.

- [ ] **Step 1:** `dangling_end_continues_tangent` (`g1_angle_deg ≤ 1.0`, same width/layer, hand copper untouched), `dangling_hint_keeps_side`, `dangling_fixed_mode_is_m3` (`engine/tests/route_g1.rs`); `test_bench_dangling_fixture_g1`. FAIL → implement → PASS. No PNG from the fixture (test pattern); eprom PNG only if its numbers moved. Commit `feat(route): G1 continuation of hand-started traces`.

### Task A16 (S; F check in A3): escalating per-pair rip cost — `negotiate.rs`, `route.rs`

**Interfaces:** `rip()` adds `opts.expert.hist_block · growth^(k−1)` for the k-th rip of B by A, refuses at `k ≥ ripup_max_per_pair`; `RIP_PAIR_CAP` removed; `RouteResult.ripup: RipReport { rips_per_connection: Vec<u32>, pairs_at_backstop: Vec<[usize; 2]>, total: u32 }`; bound `O(max_per_pair · m²)`.

- [ ] **Step 1:** `ping_pong_pair_converges_before_backstop` (`max(rip_pairs) ≤ 4 < 8`, no backstop pair), `backstop_stops_rips`, `growth_one_is_constant_cost`, `ripup_threads_byte_identical` (`engine/tests/negotiate_ripup.rs`). FAIL → implement → PASS.
- [ ] **Step 2:** eprom incl. the F.Cu-only guard and wiccon not worse (iterations reported). PNG `--task A16 --board wiccon-2023`. Commit `feat(negotiate): escalating per-pair rip cost with backstop`.

### Task A15 (S, gate review F): M4a gate, version 0.5.0, benchmark 0.5.0, review

- [ ] **Step 1:** `test_m4_gate_eprom_not_worse`, `test_m4_gate_wiccon_not_worse` (Global Constraints numbers); `pytest tests/gate -m "gate or not gate"` all green (M0–M4b + M4a); `pytest tests/scaling -m scaling` green; `since = "0.5.0"` schema keys become active (B1 commented lines un-commented in `render_toml`, reference regenerated).
- [ ] **Step 2:** version 0.5.0; `bash tools/build.sh --dev`; benchmark `docs/benchmarks/<date>-topor-0.5.0/` on all boards with the A0/A7 columns; gate PNGs `--task A15-gate` for eprom, wiccon, dcsgone; `docs/benchmarks/README.md` row.
- [ ] **Step 3 (F):** independent review: complexity of every new loop (A1, A4–A7, A13, A14, A16), note conformance, determinism audit, D-1..D-10 closure, gate tests vs numbers; BLOCKING fixed → STATE "M4a DONE (0.5.0) — M4 complete". Commit `docs(state): M4a DONE (0.5.0)`.

---

## Deferred (recorded for STATE.md, with reasons)

1. **45° mode, missing-width dialog, power-as-traces, 4+ layer fan-out vias** — backlog (45 §3 / 46 §3).
2. **Fully free via placement (incremental CDT)** — backlog row added in B0 (user 2026-10-31); M4 ships bounded relaxation (A13).
3. **Harness migration to the file/`kicad-cli` path** (drop pcbnew before KiCad 11) — backlog row added in B0; harness keeps pcbnew as the oracle for parity and gate tests in M4.
4. **Multi-variant search / Pareto** — M5 (`Options.seed` unused).
5. **Planar max-cut layer assignment** — M5 (> 2 layers).
6. **Groups via IPC** — UNVERIFIED in kipy 0.8; report line when unavailable; no gate number depends on it.
7. **Re-sketch only changed wires; zero-slack pass searches** — unchanged from M3 deferred 6.
8. **wedding-gift / nametag quality** — re-measured in benchmark 0.5.0; no gate number.

## Self-review notes

- **Spec coverage.** 45 M4 row: IPC plugin + wheel → B2/B3/B10; `topor.toml` → B1; priority tiers → A9 + B7; Steiner → A3/A5; exit stubs → A6; keepouts → B3 (IPC rule areas incl. footprint-embedded, dru `disallow`); DRC loop → B0 + B5; preview + Accept/Reject → B4/B5/B9; progress + Cancel → P1/B9; routing scope / partial reroute → B7 + engine `Scope`; partially routed boards + attach to any point → A5 + existing `copper` terminals; G1 → A14; partial routes option → B5; pre-flight report → B6. 46: #2 → A10/B6; #4 → B7; #5 → B7; #6 → A10/B1; #7 → P1; #8a → B1; #8b → A9/B7; #9 → A10/B6; #10 → B5; #11 → B5; #12 → A11; #13 → B3 + `[layers] route`; #14 → B7; #17 → A16 (`[ripup]`); #17b → P1; #18 → P1/B9; #19 → B5/B9; #20 → B6/B8; #21 → B6 + A9; #22 → B7; expert → P1. User rulings rev. 2: no pcbnew → B0/B2/B3/B5 + `test_no_pcbnew`; `save_as` copy → B2/B5 (no `BOARD_UNSAVED`); kipy verification → `test_kipy_surface` (B2) + UNVERIFIED list in the GUI checklist (B11); parity on all snapshot boards → B3/B12; plugin first → Part 1; versions 0.4.0/0.5.0 → B12/A15; UI skills + wireframes + F review → B4/B9, checklist items → B11. STATE carry-overs: (1) → A1; (2) → A2; (3) → A12; (4) D1→A4, D2→A2, D3→A5, D4→A7; (5) → A0 (+A1/A2/A4–A7); (6) G1→A14, Weiszfeld→A13, max-cut→deferred 5, variants→deferred 4, tiers→A9, 6→deferred 7, 7→deferred 8, 8→A11. Coordinator addition (rip cost) → A3 §6 + A16.
- **Type consistency.** `RunControl`/`Progress`/`route_async`/`default_options` (P1) ← B1, B5; `route_with_drc_loop(route_fn, drc_fn)` (B0) ← B5, `harness/bench.py`; `pcbfile.{append_items, remove_items, parse_layers}` (B3) ← B2 `FakeBoard`, B5; `export_model_ipc` (B3) ← B2 `IpcBoard.read_model`; `Finding` (B2) ← B1/B6/B5/B8/B9; `Selection`/`ScopePlan`/`RipPlan` (B2/B7) ← B5; `Report` (B8) ← B5/B9; wireframe wording table (B4) ← B9 test; `ref_len_nm`/`ref_kind` (A7) ← `toporlib.detour`, B8; `fits_for`/`wall_r` (A4) ← A5/A6; `ripup_growth`/`ripup_max_per_pair` (P1) ← A16; `stop_reason` (P1) ← A0 columns, B8.
- **Proportion.** Signatures, tests and values; algorithms with a choice live in the F notes (A3, B4).
- **Risks.** (i) `export_model_ipc` is the largest new surface and only fully testable live → pure mapping tests on protos + per-board parity as a gate item. (ii) `kicad-cli pcb export svg` page/board-area flags verified in 10.0.8 `--help`; theme/colours of the SVG differ from the harness renders (acceptable for a preview). (iii) `pathfinder_iter_cap` 30 may change wiccon — reported, escalated if worse. (iv) kipy group creation and `.kicad_pro` via-protection keys UNVERIFIED — documented fallbacks, checklist items.
