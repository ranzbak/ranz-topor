# Topor M4 In-KiCad Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Execution method (user, 45 §4, as in M1–M3):** rotating coordinator (O) dispatching one worker per task at the tier in the task title (H/S/F); independent tasks in parallel in isolated worktrees (R3: workers never touch the shared `.venv`); F for the design notes, the UI design, failed-gate diagnosis and the gate reviews. Every worker report is folded into `docs/progress/STATE.md` (≤ 5 lines); SDD ledger `.superpowers/sdd/2026-11-01-topor-m4-in-kicad/progress.md`; handover at a gate or at ~50 % context / ~15 round-trips. R1: work on main, **never push**.
>
> **Revision 2 (user rulings 2026-10-31):** plugin first (M4b, version 0.4.0), engine second (M4a, version 0.5.0); **no pcbnew/SWIG anywhere in the plugin runtime** (IPC + stable file formats + `kicad-cli` only; the dev/test harness keeps pcbnew as an oracle for now); via placement = bounded relaxation, free placement → backlog; iteration cap 30 accepted (wiccon before/after reported); kipy fallbacks accepted; `plugin.json` `min_version` 3.11; every plugin UI task runs the UI design skills, with F-designed ASCII wireframes reviewed before implementation.

**Goal:** First, route eprom-to-kernal and wiccon-2023 from inside KiCad 10 through the IPC plugin with today's engine behaviour — pre-flight report, progress dialog with Cancel, preview with Accept/Reject, one undo step, configured by a self-documenting `topor.toml`, usable without AI (M4b gate, 0.4.0). Then bring the engine to the M4 hard numbers: private-badge-b (snapshot `(private pin)`) N/N routed, 0 router DRC, ≤ 10 min, locked edge GND kept (M3-R14), eprom/wiccon not worse (M4a gate, 0.5.0).

**Architecture:** **M4b (plugin)** adds a thin KiCad side behind two ports — `BoardPort` (kicad-python 0.8 over the IPC API; `FakeBoard` for headless tests) and `UiPort` (wx dialogs designed from F wireframes; `ScriptedUi` for tests). Board data comes from IPC (items, pads with padstacks, footprints, zones with fills, rule areas, nets, netclasses) plus a `save_as` copy of the live board (which includes unsaved edits) for what IPC does not expose (`(layers …)` types) and for verification: the routed items are appended to that copy by our own s-expression writer, `kicad-cli pcb drc` judges it and `kicad-cli pcb export svg` → `wx.svg` renders the preview. Rules come from `.kicad_pro` JSON + `.kicad_dru` parsing, then IPC netclasses. Accept = one `begin_commit` / `create_items` / `push_commit`; Reject writes nothing. Shared pure code (DRC runner/classifier, dru parser, rule resolution, DRC loop, detour stats) moves into `toporlib/`, used by both the plugin and the harness; a test proves the plugin runtime never imports `pcbnew`. Three engine prerequisites (cancel + progress, `default_options`, stop rule) are the first M4b task. **M4a (engine)** then finishes the router: slot-search speed, fine-pitch entry, own-net capacity and same-net Steiner T-junctions, pad exit stubs, G1 continuation, escalating rip cost, priority tiers, via rules from the board setup, bounded via relaxation, layer cost terms, a DRC-feasible detour reference, private-badge-b completion.

**Tech Stack:** as M3 (Rust 1.96, spade 2.15.1, rayon 1.10, pyo3 0.23 abi3-py39, maturin in `.venv`, Python 3.12 + pcbnew SWIG **harness only**, KiCad 10.0.8 `kicad-cli`, rsvg-convert/ImageMagick harness only) plus, plugin: **kicad-python `>=0.8,<0.9`** (pinned), **wxPython 4.2.1 incl. `wx.svg`** (verified in `~/.cache/kicad/10.0/python-environments/org.topor.router`), `tomllib` (Python ≥ 3.11 → `plugin.json` `min_version` `"3.11"`). No new Rust crates.

**Spec:** `docs/design/45-mvp-plan.md` (§1 M4 row + gate, §3 M4 row, §5a, configurability + "usable without AI"); `docs/design/46-configuration.md` (§1, §1.1, §2 #1–#22 + engine/expert/fixed rows, §2.1); `docs/kicad/30-kicad-constraints.md` §2–§4.2, §5; `docs/kicad/31-rust-engine-and-integration.md` §2.1, §3, §6, §6.1; `docs/progress/STATE.md` (M3 DONE header, M4 carry-over, M3-R13/R14/R15); `docs/progress/notes/m3-gate-review.md` (§8, D-1..D-11); `docs/progress/notes/m3-detour-diagnosis.md` (M4-D1..D4); M3 plan ("Deferred to M4" 1–8); `docs/topology/12-sketch-generation.md` §1, §3.1; `docs/geometry/25-relaxation-and-reflow.md` §3; `docs/geometry/22-smoothness-and-biarcs.md`.

## Global Constraints

All M0–M3 Global Constraints and rulings still apply (Linux; `.venv` only; read-only `~/work/kicad/projects` — the harness reads boards **only** from `scratch/boards/<board>/<sha12>/` via `harness.paths.BENCH_BOARDS`, gate tests hash the snapshot dir; no KiCad types in the engine; KiCad-frame integer nm with one Y flip each way; `EPS_BUILD_NM` 5 000 / `margin_nm` 5 000; fab floors; length gate = outside-pad copper; R6/R6b/R8; M3-R8 whole-run budget; compact ≤ 20-line summaries; conventional commits ending with the attribution line of the executing session's system reminder). In addition for M4:

- **No pcbnew in the plugin runtime (user):** nothing under `plugin/` or `toporlib/` imports `pcbnew`, directly or transitively; enforced by `tests/plugin/test_no_pcbnew.py` (AST scan + a subprocess that blocks the module and imports `topor.app`). Board I/O = kipy (IPC) + `Board.save_as` copies + our s-expression writer + `kicad-cli`. Every kipy call the plugin relies on is pinned by `tests/plugin/test_kipy_surface.py` against the installed kipy source (names, signatures, proto fields); whatever needs a running KiCad is marked UNVERIFIED in the task and appears in the GUI checklist.
- **Hard M4b gate (user):** a GUI-testable whole-board plugin on eprom + wiccon with the current engine behaviour: from KiCad 10, pre-flight → progress + Cancel → preview → Reject leaves the board untouched / Accept = **one undo step**; eprom 27/27 and wiccon 62/62 routed, `routing_new 0`, `unconnected 0` on the `save_as` copy judged by `kicad-cli`; headless e2e on `FakeBoard` reproduces the numbers; `topor.toml` self-documenting + generated reference + undocumented-key test; user guide / troubleshoot / GUI checklist / CONTRIBUTING / debug guide; IPC model parity with the harness model on every snapshot board (live test, user-run). Version 0.4.0 (engine + plugin) at this gate.
- **Hard M4a gate (user, M3-R14):** private-badge-b `(private pin)`, `run_bench(router=route_router, rip="all", threads=4, options={"wall_clock_s": 600, "partial_routes": False})`: `routed == connections_total == N`, `partial == failed == 0`, `routing_new == 0`, `unconnected_items == 0`, `runtime_s ≤ 600` (whole run), 22 locked edge-GND items identical + 0 DRC pairs with router copper, threads 1/2/4 + repeat byte-identical. Version 0.5.0.
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
docs/progress/notes/m4b-gate-review.md, m4a-gate-review.md, the private-badge-b completion diagnosis (note removed: private board)
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


## Main-session rulings for M4 (binding, recorded in STATE as M4-R1..R6)
- M4-R1 (user): NO pcbnew/SWIG in the plugin runtime (plugin/, toporlib/); IPC (kipy 0.8) + stable file formats + kicad-cli only. Guard test tests/plugin/test_no_pcbnew.py (B0). The harness keeps pcbnew as the oracle (dev/test only).
- M4-R2 (user): plugin first (M4b, 0.4.0), then engine (M4a, 0.5.0).
- M4-R3 (user): UI tasks (B4, B9) MUST invoke via the Skill tool: ecc:frontend-design-direction, ecc:make-interfaces-feel-better, ecc:accessibility — adapted to wxPython inside KiCad; wireframes reviewed (F) before implementation.
- M4-R4 (user): run reports go to ~/.cache/kicad/10.0/topor/<project>/<timestamp>/ by default, with an "Open report folder" button in the preview; topor.toml `[output] report_dir` may redirect it (into the project).
- M4-R5 (main): IPC-vs-harness pad parity = polygon-equivalent within 1 µm (custom/trapezoid/chamfered compared as polygons) — acceptable.
- M4-R6 (main): pour reach oracle = kicad-cli DRC (unconnected items) on the save_as copy; harness pcbnew connectivity is secondary. Preview = kicad-cli SVG export with KiCad theme colours; routed copy highlighted if the export allows.

## Coordinator rulings (rotation 1) — see ledger for reasons
- C1 report location: `[output] report_dir` default "" = `${TOPOR_REPORT_ROOT:-${XDG_CACHE_HOME:-~/.cache}/kicad/10.0/topor}/<project name>/<YYYYmmdd-HHMMSS>/`; relative non-empty → `<project_dir>/<report_dir>/<timestamp>/`; absolute → `<report_dir>/<timestamp>/`. Tests ALWAYS set TOPOR_REPORT_ROOT (or report_dir) to a tmp dir — no test writes under ~/.cache/kicad.
- C2 kicad-python 0.8.0 (+ protobuf 5.29.6, pynng 0.9.0) is installed in the dev .venv (coordinator did it) — tests may `import kipy` from .venv. The kipy surface test still checks the plugin-venv kipy (~/.cache/kicad/10.0/python-environments/org.topor.router, read-only).
- C3 nobody but the user/main runs `tools/build.sh --dev` (it writes the real KiCad plugin venv). Workers test build.sh only with KICAD_DOCUMENTS_HOME=<scratch dir> and without --dev.
- C4 never touch a running KiCad; IPC live tests skip without KICAD_API_SOCKET.
- C5 boards only from scratch/boards/<board>/<sha12>/ snapshots (harness.paths); GUI copies go to scratch/gui/<board>/.

## Coordinator rulings (rotation 2) — see ledger for reasons and cost-if-wrong
- C23 Via `free` in the IPC model = the exact KiCad flag read from the save_as copy text (`(via … (free yes) … (uuid U))`, toporlib.pcbfile); a via whose uuid is not found / copy unparsable → free=True (unsure → kept as fixed obstacle). Harness model keeps pcbnew GetIsFree (parity: exact equality).
- C24 rip_plan keeps a via when it is free OR (new key `[run] keep_unconnected_vias = true`, default true) no same-net kept track end touches it on a layer of its span (stitching vias without the KiCad flag are never ripped). Same rule for FakeBoard/harness models and IPC models.
- C25 copper graphics → IPC obstacle polygons are CIRCUMSCRIBED (never inscribed): every arc/cap/circle vertex placed so the polygon contains the true curve; tested ⊇ pcbnew TransformShapeToPolygon within 1 µm and outward excess ≤ max_error + 1 µm. Text = bounding box, grown by a margin only if the offline test shows pcbnew's bbox does not contain the text polygon.
- C26 pour reach by probe points must be ⊆ pcbnew connectivity on every snapshot board (offline test feeding pcbnew-derived pads/vias/fills to model_ipc.pour_reach); any excess → stricter rule, never looser. kicad-cli DRC stays the oracle (M4-R6).
- C27 Q-b: when VIA_NOT_SET_UP fires, run() writes `vias.fallback_diameter_mm` / `vias.fallback_drill_mm` (default 0.8 / 0.4) into the resolved rules the engine receives (default class + every per-net entry that carries the factory via size) before routing; the run report lists it; tested end to end (engine input carries the values; a changed toml value changes them).
- Projects tree hash (rotation 2 command, run in ~/work/kicad/projects): `find . -type f -not -path '*/.git/*' -print0 | sort -z | xargs -0 sha256sum | sha256sum | cut -c1-16` → baseline caefdaad6d57d4bb (2026-10-31 19:53).
- C28 (F I-2) new key `[run] refill_after_accept` (bool, default **false**): Accept = exactly one commit = one undo step; with false the done dialog / report say "press B (Edit → Fill All Zones) to refill — a separate undo step"; true = refill_zones() after the commit (documented as a second undo step). kicad_cleanup / kicad_teardrops stay opt-in and are documented as separate undo steps. GUI checklist verifies whether IPC refill is its own undo step.
- C29 (F I-6) every run (every existing_copper policy) takes pour fills, pour reach and dangling ends from the kicad-cli REFILLED save_as copy (C20 path), reusing the original-board DRC run (`--refill-zones --save-board`) — never the live board's possibly stale fills.
- C30 (F I-1) a failure AFTER a successful commit (refill, run_action, report write) → status "accepted" + warning finding POST_COMMIT_STEP_FAILED (cause: "The routes were committed, then <step> failed"; fix: "Ctrl+Z (Edit → Undo) removes the routes; …") — never "error / board not changed".
- C31 (F B-1) engine panic: RouteJob.done() becomes true on panic (drop guard or JoinHandle::is_finished) and wait() raises a Python exception carrying the panic message; run.py after Cancel waits at most CANCEL_GRACE_S = 30 s (internal constant) then ends with ENGINE_UNRESPONSIVE error; tests with a REAL panicking job (test-only hook) and with a fake that never finishes.
- C32 (F I-5) failures before run()'s try (report dir / log unwritable, KiCad API connect failure) show an error dialog in GUI mode with a message code (REPORT_DIR_UNWRITABLE, KICAD_API_CONNECT — reuse an existing code if one fits) and exit 1.
- C33 (F I-3) VIA_NOT_SET_UP = no `board.design_settings.via_dimensions` entry with diameter > 0 in the .kicad_pro AND no net class with a non-factory via size; tested on the six snapshot .kicad_pro files (bornhack/private-badge-b/nametag set up; wiccon placeholder-only; eprom / wedding empty list).
- C34 (F I-7) keys without effect in 0.4.0 stay live keys (a user's value carries into 0.5.0) but their doc string ends with "(No effect in 0.4.0; applied from 0.5.0.)" or the precise partial statement; `existing_copper = "smooth"` documented as "treated as fixed in 0.4.0".
