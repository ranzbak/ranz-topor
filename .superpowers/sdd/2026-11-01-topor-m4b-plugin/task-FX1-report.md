# Task FX1 report: M4b gate F-review fixes (run flow, engine job, app)

Status: **DONE**. Branch `worktree-agent-ac3931d5175950334` (from main 369e029, fast-forwarded). Commits:
`ce28490 fix(engine): RouteJob reports a panic instead of hanging`, then
`fix(plugin): M4b gate review fixes …` (see `git log`).

## What changed, per finding

### B-1 (ruling C31): engine panic no longer hangs
- `engine/src/control.rs`: new `Job` (+ `JobError { Failed, Panicked }`): the thread closure holds a drop guard
  (`DoneGuard`) that sets `finished` on return AND during unwind, so `done()` is true after a panic; `wait()` joins
  and maps the join error to `JobError::Panicked(<panic message>)` (`&str` / `String` payloads), idempotent.
  Big-O in the doc comment (done O(1), wait = remaining run time).
- `engine/src/py.rs`: `RouteJob { ctl, job: Job }`; `wait()` raises `ValueError` for `Failed`, `RuntimeError("routing
  thread panicked: <msg>")` for `Panicked`. Test hook `_route_async_panicking(message)`: a `_`-prefixed pyo3 function
  that returns a real `RouteJob` whose thread panics. Justification: it is not reachable from topor.toml or any
  engine `Options` (it takes only a message string, never a model/options) and the plugin never calls it; a
  `#[cfg(feature)]` hook would be missing from the wheel built by the brief's exact maturin command, so the Python
  test could not drive the REAL job.
- `engine/tests/job_panic.rs` (5 tests: ok, Err, panic with formatted message, `&str` payload, not-done-while-running).
- `plugin/topor/run.py`: `CANCEL_GRACE_S = 30` (module constant); `route_fn` records the Cancel time and raises
  `EngineUnresponsive` once `now - cancelled_at > CANCEL_GRACE_S`; `run()` maps it to the error finding
  `ENGINE_UNRESPONSIVE` (new code, messages.py + troubleshoot section). A panicked job: `done()` true → `wait()`
  raises RuntimeError → the existing INTERNAL engine-panic finding (message carries the panic text).
- Tests: `tests/plugin/test_engine_job.py` (REAL panicking RouteJob: done() true within 10 s, wait() raises
  RuntimeError with the message, idempotent; normal job unchanged); `test_run_flow.py`:
  `test_run_engine_panic_reported_not_crashed[PanicEngine|RealPanicEngine]` (the fake `PanicJob` now behaves like
  the real engine: done() True, RuntimeError("routing thread panicked: …")), `test_run_engine_unresponsive_after_cancel`
  (HangJob never done, CANCEL_GRACE_S patched to 0.5 → ENGINE_UNRESPONSIVE error dialog, no commit),
  `test_cancel_grace_default_is_30_s`.

### I-1 (ruling C30): failure after a successful commit
- run.py Accept branch: after `port.commit`, status is "accepted"; refill, each KiCad action, the report write and
  `ui.done` each run in `after_commit(step, fn)`: an exception → log + warning `POST_COMMIT_STEP_FAILED` (new code;
  cause "The routes were committed, then <step> failed", fix "Ctrl+Z (Edit → Undo) removes the routes; …"), the
  remaining steps still run; a failing done notice is followed by `ui.error(<that warning>)`. Safety net: `run()`'s
  outer handler, when `out.commit is not None`, keeps "accepted" + POST_COMMIT_STEP_FAILED, never INTERNAL.
  `report.facts["post_commit_failed"]` carries the warnings; the DoneDialog shows them (`done_post_commit`).
- Test `test_run_failure_after_commit_stays_accepted[refill|action|report|done]` (FailingBoard hooks, a patched
  `RunReport.write`, a UI whose `done` raises): status accepted, exit code 0, one commit, one warning, no INTERNAL.

### I-2 (ruling C28): `[run] refill_after_accept` (bool, default false)
- schema.py key with the brief's doc text and kicad place `KiCad → Edit → Fill All Zones (B)`; configuration.md
  regenerated (docs_gen --check clean). run.py calls `refill_zones()` after the commit only when true.
- report.py: the Outcome line after Accept gets " · zones not refilled: press B (Edit → Fill All Zones), a separate
  undo step" or " · zones refilled by topor (a second undo step)" (fact `zones_refilled`). ui_wx DoneDialog adds
  `done_refill_hint` / `done_refilled` (WORDING + wireframe note rows + note text; the wording test is the gate).
- operate.md "Accept, Reject and undo" rewritten (one undo step; refill / cleanup / teardrops are separate undo
  steps; POST_COMMIT_STEP_FAILED). gui-test-checklist: R6 expects no refill + the hint, R7 records the top undo
  entry, new R7b (B is its own undo step) and R7c (refill_after_accept = true: record whether KiCad's IPC refill is
  its own undo step — UNVERIFIED until the GUI test), B2 row updated, result rows added.
- Tests: accepted fixture asserts `refills == 0` (default; previously 1 — the spec changed, ruling C28); the
  "action" case of the post-commit test runs with `refill_after_accept = true` and asserts `refills == 1` +
  `zones_refilled True`; `test_refill_after_accept_key`, `test_report_outcome_line_says_how_zones_are_refilled`,
  `test_done_dialog_refill_hint_and_post_commit_warning`.

### I-5 (ruling C32): failures before run()'s try
- run.py: report dir `_unique` / `mkdir` / `_Log` in a try; `OSError` → `REPORT_DIR_UNWRITABLE` error (new code,
  fix names `output.report_dir`) via `ui.set_context` + `ui.error`, status error (exit 1), the board is not read.
- app.py `run_gui(shipped_version, engine, *, ui=None, connect=None)`: connection via `IpcBoard.connect(...)`;
  any exception → `KICAD_API_CONNECT` (new code; fix "KiCad → Preferences → Plugins → tick Enable KiCad API …")
  through `_fatal` (stderr + crash log + `ui.error`), exit 1. `ui` / `connect` are test seams (no wx.App then).
- Tests: `test_run_report_dir_unwritable_is_an_error` (a file where the report root should be),
  `test_main_headless_report_dir_unwritable_exits_1`, `test_run_gui_kicad_api_connect_failure_shows_error_and_exits_1`
  (REAL kipy on a nonexistent socket in tmp: fails in ~10 ms; never touches a running KiCad),
  `test_run_gui_connect_seam_failure`. All with a recording fake UI; no window shown.

### I-6 (ruling C29): fills from the refilled copy in every path
- run.py: the original DRC now runs on `work/orig/<board>` (a `_variant` of the save_as text) with
  `save_board=True` (same single kicad-cli run, `--refill-zones --save-board`); `out.refilled_pcb` = that file; the
  save_as copy stays as KiCad wrote it. No-rip path: `eng_model = refill_pours(filter_model(...), refilled text)`;
  rip path unchanged (ripped copy refilled by its own DRC, C20). No extra kicad-cli run.
- `refill_pours`: a zone that the refilled file lists WITHOUT fills on its layers now reaches nothing (old fill kept
  only as obstacle, `warning`) — before it kept the stale reach (non-conservative vs M4-R8). Zone absent → unchanged.
- operate.md:274 ("the copy is refilled first") is now true; left untouched (FX2 owns Limitations).
- Tests: `test_run_fixed_takes_pour_fills_from_the_refilled_copy` (eprom, existing_copper fixed, the GND pour's model
  fill replaced by one board-sized rectangle reaching "FAKE.1" / "fake-via": the engine model gets exactly the
  refilled copy's fills, reach = pour_reach of them, ⊆ the harness reach, the save_as copy unchanged),
  `test_refill_pours_zone_unfilled_after_refill_reaches_nothing`.

### MINOR 3, 6, 12, 13
- 3: new pure `plugin/topor/pourreach.py` (`pad_contains`, `pad_sample_points`, `pour_reach`, `dangling_ends`,
  helpers `_top_down`, `_in_span`; Big-O in the module doc), moved verbatim from model_ipc.py, which re-exports them
  (same objects). run.py imports pourreach at module level (no kipy). `IpcBoard.connect()` in ipc.py; app.py has no
  kipy import. Tests: `tests/plugin/test_pourreach.py` (subprocess with kipy blocked imports topor.pourreach +
  topor.run; re-export identity; a reach case), `test_app_and_run_do_not_import_kipy` (AST), 
  `test_ipcboard_connect_reports_connection_error`.
- 6: INTERNAL fix = `messages.fix_for("INTERNAL")` (asserted in the panic test).
- 12: gate test asserts `out.ripped and out.commit.removed == len(out.ripped)`, `created == len(items)`,
  `board_port.refills == 0 and board_port.actions == []`.
- 13: `IpcBoard.net_codes()` from ONE cached `get_nets()` list, used by `netclasses()`, `run.resolve_rules`
  (`net_codes=` passed to `resolve_rules_from_project`; `BoardPort.net_codes()` added, FakeBoard returns None = old
  behaviour) and `read_model` → `export_model_ipc(net_codes=…, dru_text=<copy's .kicad_dru or "">)`. Tests with the
  stub kipy board: `test_ipcboard_one_net_list_for_rules_and_model` (net-class answer missing nets → per_net still
  keyed by the shared codes; dru from the copy, "" without one; one get_nets call),
  `test_export_model_ipc_uses_the_given_net_codes`.

### Other files
messages.py: 4 new codes (KICAD_API_CONNECT, REPORT_DIR_UNWRITABLE, ENGINE_UNRESPONSIVE, POST_COMMIT_STEP_FAILED);
troubleshoot.md generated part regenerated + exit-code 0/1 and "Cannot connect" sections link the new codes.
ports.py / fake.py: `net_codes`. model_ipc.py: docstrings. app.py docstring: exit codes.

## Numbers (after the changes)
`.venv/bin/pytest tests/gate/test_m4b_gate.py -m "gate or not gate" -q -s` → **4 passed** (180 s):
- eprom-to-kernal: routed **27/27**, vias 0, routing_new **0**, unconnected **0** (new 0), outside 64.628 mm, router 0.25 s, 1 DRC round
- wiccon-2023: routed **62/62**, vias 37, routing_new **0**, unconnected **0** (new 0), outside 1373.553 mm, router 6.12 s, stop no gain
- scope between U1+U2 24/24 (est 26), touching 27/27 (est 30), routing_new 0
Run-flow fixtures (tests/plugin/test_run_flow.py, 33 passed): eprom accepted 27/27, routing_new 0, unconnected_new 0.

## Test commands
- `cargo test --release --manifest-path engine/Cargo.toml`: all 34 result groups ok (incl. job_panic 5/5).
- `cargo clippy --all-targets [--features python] --manifest-path engine/Cargo.toml -- -D warnings`: clean.
- maturin develop (brief's command) run from this worktree after the Rust change.
- `.venv/bin/pytest -q tests/plugin/test_run_flow.py`: 33 passed (315 s).
- `.venv/bin/pytest -q` (default suite): 430 passed, 7 skipped, 1 failed (test_docs_links: operate.md lacked the
  literal "KiCad → Edit → Fill All Zones" of the new key's kicad place) → fixed, test_docs_links + messages +
  config_docs re-run green (30 passed).
- `PYTHONPATH=plugin python -m topor.docs_gen --check`: up to date.

## UNVERIFIED (needs a running KiCad)
- Whether `Board.refill_zones()` over IPC makes its own undo entry (GUI checklist R7c); whether B is its own undo
  step (R7b, expected yes); the top undo entry text after Accept (R7).
- `IpcBoard.connect` against the real KiCad socket (only the failure path is tested, with real kipy).
- The DoneDialog's new lines at 100 % / 200 % / dark (constructed headless in test_ui_wx only).

## Concerns / notes for the merge
- FX2 also edits messages.py (DO_NOT_TOUCH / VIA_NOT_SET_UP texts): the generated part of troubleshoot.md will
  conflict; resolve by regenerating it from the merged MESSAGES (`render_troubleshoot`).
- schema.py: the new key sits right after `kicad_teardrops`; FX2 edits doc strings nearby (C34). I did NOT add
  "a separate undo step" to the kicad_cleanup / kicad_teardrops doc strings to stay out of FX2's lines; operate.md
  documents it.
- The `scratch` symlink shows as untracked in the worktree (the ignore rule is `scratch/`); not committed.
- `refill_pours` now also makes the rip path stricter for zones that the refilled ripped copy lists unfilled
  (e.g. teardrop zones of ripped tracks): reach nothing instead of the stale reach. Gate numbers unchanged.
