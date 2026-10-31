### Task AP (S): plugin follow-ups from the M4b F review — MINOR 10 (kicad-cli blocks the dialog) and MINOR 11 (board edited during the run)

Review text: /home/paul/work/kicad/plugins/topor/docs/progress/notes/m4b-gate-review.md MINOR 10 and MINOR 11 (read both in full).
Plugin only (plugin/topor/, toporlib/, tests/plugin/, docs/user/); no Rust changes; no engine rebuild needed.

1. MINOR 10: every kicad-cli call made by the plugin (toporlib/drc.py run_drc incl. `--refill-zones --save-board`, the refill path,
   the SVG export in plugin/topor/preview.py) runs via `subprocess.Popen` + poll loop with (a) a timeout — new Setting
   `[run] kicad_cli_timeout_s` (default 120, 0 = no timeout; doc string + unit + range; engine_field none, plugin_field set) →
   on timeout kill the process and raise a finding `KICAD_CLI_TIMEOUT` (new code in messages.py with cause + fix naming the key;
   `### KICAD_CLI_TIMEOUT` section in docs/user/troubleshoot.md), (b) a progress callback invoked at least every 0.25 s so the UI pumps
   events (`ui.progress(Progress(phase=...))` with the plugin phases preflight_drc|drc|preview; returning False = cancel → kill the
   process; in the pre-flight phase cancel → status "aborted_preflight"/"cancelled" as the existing flow defines; after routing the
   best result stays). Keep the harness callers working (callback optional, default None = blocking wait with timeout). Emit an early
   `setup` progress before `read_model`. Update docs/user/gui-test-checklist.md ("Cancel latency within a second" now also in
   pre-flight/DRC/preview phases; verify in GUI) and operate.md.
2. MINOR 11: before `begin_commit` compute a cheap board fingerprint (item counts + hash of track/via uuids+positions and footprint
   refs+positions+orientations, through the BoardPort — add `BoardPort.fingerprint() -> str`, implemented in IpcBoard via kipy
   get_tracks/get_vias/get_footprints (UNVERIFIED live → GUI checklist row) and in FakeBoard from its text/items) at run start and
   again right before the commit; mismatch → no commit, status "error" with new finding `BOARD_CHANGED_DURING_RUN` (cause: the board
   was edited while topor ran; fix: "Undo your edits or run Topor again; do not edit the board while Topor runs") + troubleshoot
   section. Test with FakeBoard mutated between preview and commit. Make it configurable only if there is a real judgement call
   (there is not: always check) — document in operate.md.
3. Keep `tests/plugin/test_kipy_surface.py` green (add any kipy names you use). No pcbnew (test_no_pcbnew).
Acceptance: `.venv/bin/pytest -q tests/plugin` green; default `.venv/bin/pytest -q` green; `PYTHONPATH=plugin .venv/bin/python -m
topor.docs_gen --check` clean (regenerate configuration.md/troubleshoot pieces it owns). Report the run-flow eprom/wiccon numbers stay
27/27 and 62/62, routing_new 0, unconnected 0 (tests/gate/test_m4b_gate.py -m "gate or not gate" -q -s).
Commits: `fix(plugin): kicad-cli runs pump the UI, timeout and cancel` + `fix(plugin): refuse to commit when the board changed during the run`.
