# Task AP report: kicad-cli pumps the UI (MINOR 10), board changed during the run (MINOR 11)

Status: DONE_WITH_CONCERNS (one combined code commit, see Concerns).

## What changed
- `toporlib/kicad_cli.py`: `run_cli(cmd, timeout_s=None, on_poll=None)` = Popen + poll loop (output to temp files, no pipe stall),
  `KicadCliTimeout`, `KicadCliCancelled`. `on_poll` at least every 0.25 s; False -> kill + KicadCliCancelled; timeout -> kill.
  Default (no callback) = blocking wait that still honours the timeout.
- `toporlib/drc.py` `run_drc(..., timeout_s=None, on_poll=None)`; `plugin/topor/preview.py` `export_svg` / `render` / `render_ex` take the same two args. Harness callers unchanged (defaults).
- `plugin/topor/schema.py`: new `[run] kicad_cli_timeout_s` (float, 120, unit s, range 0..86400, engine none, plugin_field `run.kicad_cli_timeout_s`); `docs/user/configuration.md` regenerated. Added as one block after `kicad_teardrops` (A9/A0 merge: schema.py conflict should be adjacent-line only).
- `plugin/topor/messages.py` + `docs/user/troubleshoot.md` (regenerated tail + exit-code-1 paragraph): `KICAD_CLI_TIMEOUT`, `BOARD_CHANGED_DURING_RUN` (fix: "Undo your edits (KiCad → Edit → Undo) or run Topor again; do not edit the board while Topor runs." — the KiCad menu path is needed by test_every_code_has_cause_and_fix).
- `plugin/topor/run.py` (focused; engine-option code untouched): `cli_kw(phase, cancellable)` helper, `emit("setup")` at the very start of `_run` (before save_as / read_model), DRC calls pass timeout + `emit(phase)` poll. Pre-flight DRC and baseline (ripped) DRC are cancellable: Cancel kills kicad-cli, status `aborted_preflight`. DRC rounds and preview are pumped but NOT killed by Cancel (the best result stays, Cancel only stops further rounds). Timeout in a DRC -> status `error` / KICAD_CLI_TIMEOUT (report + ui.error); timeout in the preview export -> warning finding KICAD_CLI_TIMEOUT, run goes on (like PREVIEW_FAILED).
- MINOR 11: `BoardPort.fingerprint()` (ports.py); `IpcBoard.fingerprint()` via `board_fingerprint(get_tracks, get_vias, get_footprints)` (counts + sha256 of sorted track/via uuid+positions+layer and footprint ref+position+orientation; UNVERIFIED live); `FakeBoard.fingerprint()` hashes the text of every segment/arc/via/footprint item. run.py takes it before `save_copy` and again after Accept right before the commit; mismatch -> `BoardChanged` -> status error + BOARD_CHANGED_DURING_RUN, no commit. A failing fingerprint call only logs and switches the check off (UNVERIFIED IPC must not block runs). Not configurable: no real judgement call.
- Docs: operate.md (Progress and Cancel, While topor runs), gui-test-checklist.md (R3 row, cancel-latency item now all phases, new rows: kicad_cli_timeout_s, board edited during run / IpcBoard.fingerprint).
- `tests/plugin/test_kipy_surface.py`: added `kipy.geometry.Angle.degrees`.

## Tests
- New `tests/plugin/test_kicad_cli_run.py` (run_cli blocking/300 KB output, timeout kill, poll <= 0.35 s gaps, cancel kill, 0 = no timeout, schema key, board_fingerprint), `tests/plugin/test_run_flow_guards.py` (setup progress first, pre-flight cancel -> aborted_preflight, timeout 0 -> None, real fake-CLI script timeout -> error finding KICAD_CLI_TIMEOUT with UI pumped, board moved / track deleted between preview and commit -> no commit + BOARD_CHANGED_DURING_RUN, unchanged -> commits, fingerprint taken twice, FakeBoard fingerprint sees a move).
- Updated two assertions that pinned the retired behaviour (spec change, no bound weakened): `test_run_aborts_on_error_finding` now expects exactly one progress call (`setup`) instead of none; `tests/test_docs_links.py` expects the new operate.md Cancel sentence.
- `.venv/bin/pytest -q tests/plugin`: 283 passed, 7 skipped, 1 failed (the progress assertion above, fixed afterwards and re-run green together with test_messages / kipy_surface / config_docs). `topor.docs_gen --check` clean. `tests/gate/test_m4b_gate.py -m "gate or not gate" -q -s`: passed on eprom 27/27 and wiccon 62/62 (routing_new 0, unconnected 0 asserted by the gate test); tests/test_drc.py, test_drcloop.py, test_docs_links.py green.
- The full default `.venv/bin/pytest -q` was NOT run to completion (machine saturated by parallel workers; tests/plugin alone took 19 min). Run it at merge.

## Concerns
- MINOR 10 and 11 share run.py/messages.py/troubleshoot.md hunks, so they are ONE code commit (ae72705) rather than two.
- Cancel during DRC rounds / preview does not kill kicad-cli by design (result is already routed; best result stays). Cancel during preview export therefore waits for the export (bounded by the timeout).
- IpcBoard.fingerprint and kicad-cli polling under real wx are UNVERIFIED live: checklist rows added.
