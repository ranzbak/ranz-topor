# Global constraints — topor M4a engine 0.5.0 (plan docs/superpowers/plans/2026-11-01-topor-m4-in-kicad.md rev 2, Part 2)

Extracted verbatim from the plan (Global Constraints + Review Focus not repeated for M4b-only items):

## Global Constraints

All M0–M3 Global Constraints and rulings still apply (Linux; `.venv` only; read-only `~/work/kicad/projects` — the harness reads boards **only** from `scratch/boards/<board>/<sha12>/` via `harness.paths.BENCH_BOARDS`, gate tests hash the snapshot dir; no KiCad types in the engine; KiCad-frame integer nm with one Y flip each way; `EPS_BUILD_NM` 5 000 / `margin_nm` 5 000; fab floors; length gate = outside-pad copper; R6/R6b/R8; M3-R8 whole-run budget; compact ≤ 20-line summaries; conventional commits ending with the attribution line of the executing session's system reminder). In addition for M4:

- **No pcbnew in the plugin runtime (user):** nothing under `plugin/` or `toporlib/` imports `pcbnew`, directly or transitively; enforced by `tests/plugin/test_no_pcbnew.py` (AST scan + a subprocess that blocks the module and imports `topor.app`). Board I/O = kipy (IPC) + `Board.save_as` copies + our s-expression writer + `kicad-cli`. Every kipy call the plugin relies on is pinned by `tests/plugin/test_kipy_surface.py` against the installed kipy source (names, signatures, proto fields); whatever needs a running KiCad is marked UNVERIFIED in the task and appears in the GUI checklist.
- **Hard M4b gate (user):** a GUI-testable whole-board plugin on eprom + wiccon with the current engine behaviour: from KiCad 10, pre-flight → progress + Cancel → preview → Reject leaves the board untouched / Accept = **one undo step**; eprom 27/27 and wiccon 62/62 routed, `routing_new 0`, `unconnected 0` on the `save_as` copy judged by `kicad-cli`; headless e2e on `FakeBoard` reproduces the numbers; `topor.toml` self-documenting + generated reference + undocumented-key test; user guide / troubleshoot / GUI checklist / CONTRIBUTING / debug guide; IPC model parity with the harness model on every snapshot board (live test, user-run). Version 0.4.0 (engine + plugin) at this gate.
- **Hard M4a gate (user, M3-R14):** private-badge-b `3e7e66f862a7`, `run_bench(router=route_router, rip="all", threads=4, options={"wall_clock_s": 600, "partial_routes": False})`: `routed == connections_total == 231`, `partial == failed == 0`, `routing_new == 0`, `unconnected_items == 0`, `runtime_s ≤ 600` (whole run), 22 locked edge-GND items identical + 0 DRC pairs with router copper, threads 1/2/4 + repeat byte-identical. Version 0.5.0.
- **Not worse than M3 (user):** eprom 27/27, 25/25, `routing_new 0`, `unconnected 0`, `length_outside_pads_nm ≤ 64_700_000` and `≤ hand`, `router_s < 2.0`, zero_slack 0 or DRC-kept, the M2 guard `eprom_ripped_front_only_is_the_m2_result` (27/27, ≤ 67.8 mm) green; wiccon 62/62 (signal 57/57), `vias ≤ 37`, `routing_new 0`, `unconnected 0`, `length_outside_pads_nm ≤ 1_380_000_000`, `router_s < 10.0`; scaling L1/L2 within `t100/t10 ≤ 40`, `t1000/t100 ≤ 30`. **Budgets are never relaxed by a worker or coordinator**; one F diagnosis round, then escalate with numbers.
- **Complexity (45 §5a):** every new component states its Big-O in its task and code comment; no exponential/factorial stage; every loop capped; every long call checks the deadline **and the cancel flag**; rip-up bound `O(max_per_pair · m²)`.
- **Determinism:** byte-identical result JSON (minus `runtime_s`) for `threads ∈ {1, 2, 4}` and across runs, for every board and option; no `HashMap` iteration order reaches the output.
- **Configurability (45):** every judgement call is a `topor.toml` key with a sane default in `plugin/topor/schema.py`, listed in the run report when non-default, mapped 1:1 to an `Options`/harness/plugin field. Sources (46 §1): KiCad board setup → `topor.toml` (created with all defaults + comments on first run, unknown keys warn, `version = 1`) → built-in defaults (listed).
- **Messages (46 §1.1):** every finding/error has a code in `plugin/topor/messages.py` with cause + fix naming the KiCad place or the `topor.toml` key; `docs/user/troubleshoot.md` has a section per code (tested).
- **UI (user):** every plugin UI task invokes, via the Skill tool, `ecc:frontend-design-direction` (direction, layout), `ecc:make-interfaces-feel-better` (spacing, states, feedback, wording) and `ecc:accessibility` (keyboard, focus order, contrast, sizes, labels) **before** designing or implementing, adapting web guidance to wxPython inside KiCad: native widgets, KiCad dialog look, light and dark themes, HiDPI. ASCII wireframes per dialog (F, Task B4) are reviewed by an F agent before any implementation (S). GUI checklist acceptance: tab order, Esc = Cancel, Enter = default button, readable at 100 % and 200 % scaling, dark theme.
- **Progress PNG (user):** every outcome-affecting engine task ends with `.venv/bin/python -m harness.progress --milestone m4 --task <TaskId> --router topor --board <board>` → `docs/benchmarks/progress/<YYYY-MM-DD>-m4-<TaskId>[-<board>].png` (hand left, topor right, both layers, caption with routed/total, vias, DRC, `router_s`, **detour line**, **pcb sha12**), committed, sent to main. Never a test pattern as a result.
- **Builds:** wheel rebuilt (`VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release --manifest-path engine/Cargo.toml --features python`) before every engine-using pytest run; `bash tools/build.sh --dev` before the gates; clippy `-D warnings` clean; `.venv/bin/pytest -q` green.
- **Backlog, not M4:** 45° mode, missing-width dialog, power-as-traces, 4+ layer fan-out vias (46 §3); **fully free via placement (incremental CDT)** and **harness migration to the file/`kicad-cli` path before KiCad 11** (both added to 45 §3 in Task B0). `[output] geometry` accepts `arcs|segments`; `"45deg"` → `CFG_VALUE_BACKLOG`.

## Main / user rulings (binding; full text in docs/progress/STATE.md)
- PUSH RULE: NOBODY except the main session pushes. Never `git push`.
- STOP RULE (user): "Stop after M4 is complete, so we can do an evaluation" — nothing after the M4a gate (no M5).
- Read-only projects (user): "Don't touch the projects on disk though.. copy before modifying" — ~/work/kicad/projects is read only;
  only harness/snapshot.py reads it; everything uses scratch/boards/<board>/<sha12>/ snapshots.
- docs/benchmarks/ is gitignored and untracked (main ab42758, user): PNGs/benchmarks are written on disk only, never committed
  (no -f); tracked docs quote numbers inline; tests use tests/fixtures/. This supersedes "committed" in the Progress PNG bullet above.
- "The goal here is to have a robust routing algorithm not to reproduce the hand layout." Judge by completion, DRC, detour vs ghost /
  reference, vias, length, runtime, determinism — the hand layout is a reference only.
- No per-connection detours where a direct path exists (user): detour ratio stats (median/p90/max/count > 1.5/top 10) per board in
  every report; PNG captions highlight detours > 1.5.
- "If in doubt, make it configurable" with sane defaults (46). No pcbnew in plugin/ or toporlib/ (M4-R1). No deprecated KiCad APIs.
- M3-R13/R14: private-badge-b 3e7e66f862a7 231/231, 0 router DRC (routing_new 0), unconnected 0, ≤ 10 min whole run, 22 locked GND
  items kept identical with 0 DRC pairs vs router copper = HARD M4a gate. Pre-existing violations (hand copper) are `preexisting`,
  never router errors.
- M3-R15: detour reference = DRC-feasible shortest path on fixed copper only (A7); ghost stays heuristic + rip trigger; bench
  reports `detour` (reference) and `detour_ghost`; reference failure → ghost fallback, flagged.
- M4-R8 (plugin): IPC model approximations conservative; unchanged.
- Gate failure rule: if the private-badge-b gate fails after ONE diagnosis round → commit, numbers + profile, hand back. Never relax.

## M4 carry-over folded into M4a (coordinator mapping, see ledger A-C4)
1. search speed: clearance_bend ~162 candidates/query, 94 % of search time on private-badge-b → A1.
2. #201 U1.13→U7.4 fine-pitch entry, M4-D2 state point / los (#208) → A2.
3. private-badge-b completion (M3-R14) → A12 (+ diagnosis).
4. M4-D1 own-net capacity → A4; M4-D3 same-net siblings → A5; M4-D4 detour reference → A7.
5. M3 review D-1..D-10 → A0 (D-11 = items 1–4).
6. Plugin hand-back: engine must use net priorities (@1–@3 marker > topor_pN netclass > [nets] priority), do_not_touch/exclude_nets,
   whole-board ignore_pads (plugin computes + reports them already) → A9 incl. plugin wiring + doc strings ("No effect in 0.4.0" removed).
7. ripup growth 2.0 / max_per_pair 8 backstop replacing the hard rip limit; [iterations] max 30, patience 5, min_gain_pct 1 (newly
   routed counts as progress), wall_clock_s 0 = disabled (P1 did the stop rule — verify); dedupe relax_iter_cap → A16 (+A0 dedupe).
8. [vias] via_cost_mm 2.0, max_per_connection 4, reduction_len_slack_pct 10, per-netclass/per-priority cost overrides; via sizes from
   netclass + KiCad pre-defined vias (keep plugin preflight.smaller_profile_vias in sync with engine/src/vias.rs) → A10.
   Bounded via relaxation → A13 (fully free placement stays backlog).
9. existing_copper="smooth", [nets] pour_served, [run] detour_reference: see Ruling A-C5.
10. Plugin F-review MINOR 10 (kicad-cli blocking dialog → Popen + poll + timeout) and MINOR 11 (board fingerprint before commit) → AP.

## Coordinator rulings (M4a) — reasons + cost-if-wrong in the ledger
