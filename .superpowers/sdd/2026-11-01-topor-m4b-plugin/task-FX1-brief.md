### Task FX1 (S): M4b gate F-review fixes — run flow, engine job, app — `engine/src/py.rs`, `plugin/topor/{run,app,ipc,model_ipc,messages,schema}.py`, `toporlib/`, `tests/`

Review: /home/paul/work/kicad/plugins/topor/docs/progress/notes/m4b-gate-review.md (read the findings named here in full, incl. their evidence and fix shapes).
Coordinator rulings C28–C32 in global-constraints.md are binding. Fix, each TDD (failing test first):
1. **B-1 (C31)** engine panic hang: `RouteJob.done()` true after a panic (drop guard setting `finished`, or `JoinHandle::is_finished`), `wait()`
   raises a Python exception carrying the panic message (run.py maps it to the existing engine-panic finding). Rust unit test with a panicking
   closure if the job code allows it; Python test with a REAL panicking job via a test-only hook that is NOT reachable from topor.toml / user
   options (e.g. a `_`-prefixed pyo3 function, or an env var read only in a `#[cfg(...)]`/test path — your choice, justify). run.py: after the user
   cancels, wait at most `CANCEL_GRACE_S = 30` s (module constant) for done(), then end with error finding `ENGINE_UNRESPONSIVE` (new code in
   messages.py + `### ENGINE_UNRESPONSIVE` in docs/user/troubleshoot.md); test with a fake job that never finishes (patch the constant small).
   Fix the existing `PanicJob` fake in tests/plugin/test_run_flow.py to behave like the real engine now does. You OWN the shared .venv for this task:
   after the Rust change run `VIRTUAL_ENV=/home/paul/work/kicad/plugins/topor/.venv /home/paul/work/kicad/plugins/topor/.venv/bin/maturin develop --release --manifest-path engine/Cargo.toml --features python`
   from your worktree; `cargo test --release` + `cargo clippy --all-targets -- -D warnings` clean.
2. **I-1 (C30)** failure after a successful commit (refill, run_action, report write) → status "accepted" + warning `POST_COMMIT_STEP_FAILED` (new
   code + troubleshoot section; fix text names Ctrl+Z / Edit → Undo); never INTERNAL "board not changed". Test each step failing (FakeBoard hooks).
3. **I-2 (C28)** new key `[run] refill_after_accept` (bool, default false; doc: "After Accept, refill all zones from topor (a second undo step in
   KiCad). false = Accept is exactly one undo step; press B (Edit → Fill All Zones) yourself."; kicad place "KiCad → Edit → Fill All Zones (B)").
   run.py: refill_zones() after the commit only when true. Done dialog / report summary line tells the user to press B when false. Update
   operate.md "Accept, Reject and undo" (one undo step; refill / cleanup / teardrops are separate undo steps) + gui-test-checklist row (verify whether
   KiCad's IPC refill is its own undo step). Tests: default → FakeBoard.refills == 0 after Accept; true → 1. Regenerate configuration.md (docs_gen).
4. **I-5 (C32)** failures before run()'s try (report dir / log unwritable `run.py:336-339`; KiCad API connect failure `app.py:127-151`) → error
   dialog in GUI mode (`WxUi.error`, never shown in tests — use a recording fake UI) with a message code (`REPORT_DIR_UNWRITABLE`, `KICAD_API_CONNECT`;
   reuse an existing code if one already fits; troubleshoot sections), exit 1. Tests for both.
5. **I-6 (C29)** every run takes pour fills / reach / dangling ends from the kicad-cli refilled save_as copy (reuse the original-board DRC run with
   `--refill-zones --save-board`, then the existing `refill_pours` path; no extra kicad-cli run if avoidable). Test: a model whose live fills are stale
   (FakeBoard model with a deliberately wrong fill) gets the refilled copy's fills with existing_copper = "fixed". operate.md:274 then is true.
6. MINOR 3: move the pure `pour_reach` / `dangling_ends` (and helpers they need) out of model_ipc.py into a pure module (toporlib/geom.py or
   plugin/topor/pourreach.py) so run.py does not import kipy; keep model_ipc re-exporting them; `KiCad` connect moved behind `ipc.py` (e.g.
   `IpcBoard.connect()` / a function in ipc.py) so app.py does not import kipy directly.
7. MINOR 6: run.py INTERNAL fix text → the catalogue text (messages.py).
8. MINOR 12: tests/gate/test_m4b_gate.py also asserts `out.commit.removed == len(out.ripped)` (or the equivalent fields) and, with the new default,
   `board_port.refills == 0` and `board_port.actions == []`.
9. MINOR 13: pass `net_codes=` explicitly from ONE net list to both `resolve_rules_from_project` and `export_model_ipc`; `export_model_ipc` gets the
   dru text of the save_as COPY from `IpcBoard.read_model` (not the live project dir). Tests with the stub kipy board.
Report the eprom / wiccon numbers after your changes (run-flow fixtures: routed/total, vias, routing_new, unconnected) and the gate command result:
`.venv/bin/pytest tests/gate/test_m4b_gate.py -m "gate or not gate" -q -s`. Numbers must stay eprom 27/27, wiccon 62/62, routing_new 0,
unconnected 0 — if not, STOP and report.
Acceptance: cargo test --release + clippy clean; `.venv/bin/pytest -q tests/plugin` green; default `.venv/bin/pytest -q` green; docs_gen --check clean.
Commits (conventional, one per logical fix group is fine), e.g. `fix(engine): RouteJob reports a panic instead of hanging` / `fix(plugin): …`.
