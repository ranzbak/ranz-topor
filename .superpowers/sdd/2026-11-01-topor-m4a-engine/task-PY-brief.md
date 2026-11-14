### Task PY (sonnet, rotation 2): pytest reds — no-effect doc test, progress caption test, wiccon plugin path 63 vs 62 connections

Read rotation-2-common.md first. Root causes, never weaken a test; if a test's premise became obsolete, rewrite it so it still checks
the rule it encodes (say so in the commit + report).
1. `tests/plugin/test_config_docs.py::test_keys_without_effect_say_so`: requires `md.count(NO_EFFECT) >= 1`, but A9/A7 made every
   formerly no-effect key live. Rule C34: a key without effect says so. The test must keep checking: any key that has no consumer
   says so (the PARTIAL statements stay), and no live key carries the marker. Check also which doc strings still say "0.4.0" in a way
   that will be wrong at 0.5.0 (list them; the version bump is a later task A15 — do not bump versions now).
2. `tests/test_router.py::test_progress_m4_name_and_caption_lines`: find why it fails (run it; harness/progress.py caption lines vs
   the A7/A0 detour columns, stop_reason line, etc.) and fix the code or the test by root cause.
3. wiccon plugin path counts 63 connections, harness 62: `tests/plugin/test_run_flow.py::test_run_wiccon_refill_gives_harness_
   connection_count` and `tests/gate/test_m4b_gate.py` headless wiccon (total 63 ≠ 62). A6 merged overlapping same-net pads (private-badge-b
   total 243 → 241) — likely the plugin's IPC/FakeBoard model export or plugin-side connection count differs from the harness model
   (pad merge, pour completion, ignore_pads, refill). Diff the two models / RouteResults (connections list) and fix where they
   diverge. If the root cause is in the engine (Rust), stop and report it with the evidence instead of changing Rust.
Use the shared .venv engine (it is main's engine; you change Python only). Acceptance: `.venv/bin/pytest -q` default suite green except
test_scaffold (cargo reds belong to task TE); `.venv/bin/pytest tests/gate/test_m4b_gate.py -m "gate or not gate" -q -s` green; docs_gen
--check clean. Commits `test(config): ...`, `fix(progress): ...`, `fix(plugin): ...` (adapt). Report task-PY-report.md.
