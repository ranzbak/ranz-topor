### Task QB12 (S): Q-b VIA_NOT_SET_UP fallback applied + B12 Step 1 gate tests — `plugin/topor/run.py`, `plugin/topor/preflight.py`, `toporlib/rules.py` (if needed), `plugin/topor/report.py`, `tests/gate/test_m4b_gate.py`, tests

Part A — Q-b (main ruling, verbatim): "VIA_NOT_SET_UP must actually apply the 0.4/0.8 fallback to the rules". Coordinator ruling C27
(global-constraints.md): when VIA_NOT_SET_UP fires, run() writes `vias.fallback_diameter_mm` / `vias.fallback_drill_mm` (default 0.8 / 0.4) into the
resolved rules the engine receives (default class + every per-net entry that carries the factory via size) before routing; the run report lists it.
Find which rules fields the engine reads for the via size (engine/src/model.rs Rules / per-net) and set exactly those, in nm. TDD:
- `test_via_not_set_up_applies_fallback_to_engine_rules`: synthetic pro/board where VIA_NOT_SET_UP fires; capture the model/rules passed to the
  route function (run.py has an injectable route_fn/engine — see tests/plugin/test_run_flow.py) → via diameter 800_000, drill 400_000;
- with topor.toml `vias.fallback_diameter_mm = 0.7`, `fallback_drill_mm = 0.35` → 700_000 / 350_000;
- when via sizes ARE set up (finding absent) → rules unchanged (identity);
- report.md / result.json mention the applied fallback.
Report: does VIA_NOT_SET_UP fire on eprom / wiccon snapshots? If yes, the numbers before/after on that board (routed/total, vias, routing_new,
unconnected). If the fallback changes a gate number for the worse, STOP and report — do not tune.

Part B — B12 Step 1 (plan, verbatim): "`test_m4b_gate_headless_eprom_and_wiccon` (marked `gate`): `FakeBoard` on snapshot copies with harness-built
models, `ScriptedUi(accept=True)`, real engine: `status == "accepted"`, one commit, eprom 27/27 / wiccon 62/62 routed, `routing_new == 0` and
`unconnected == 0` by `kicad-cli` on the committed copy, report dir written, `topor.toml` created, `docs_gen --check` clean;
`test_m4b_gate_scope_selection`. PASS. `pytest tests/gate -m "gate or not gate"` green (M0–M3 + M4b)."
Details:
- Snapshot boards from scratch/boards/<board>/<sha12>/ via harness.paths (never ~/work/kicad/projects). The wiccon snapshot is a KiCad-6 file
  (ruling C21): the TEST re-saves the copy through pcbnew (`pcbnew.LoadBoard` + `SaveBoard` into tmp) before the plugin flow; eprom likewise if it is
  older than KiCad 10. topor.toml written beside the copy with `[run] existing_copper = "rip"` (the hand routing is ripped, as rotation 1's headless
  numbers: eprom 27/27 vias 0; wiccon 62/62 vias 37). Mirror the existing run-flow fixtures (tests/plugin/test_run_flow.py, test_app.py) — reuse
  helpers, do not duplicate a second flow. "topor.toml created": also assert a run with no toml beside the copy creates it (first-run path) — if the
  real flow only creates it in a specific case, assert that case and say so.
- Assert from the run report / result.json: status accepted, exactly one FakeBoard commit, routed == total (27 / 62), routing_new 0 and
  unconnected 0 as judged by kicad-cli on the committed copy (the flow's own judge — assert where its numbers come from), report dir contains
  report.md, result.json, drc.json, preview.png, log.txt; print a numbers line per board (routed/total, vias, routing_new, unconnected, outside mm,
  router s, DRC rounds).
- `test_m4b_gate_scope_selection`: eprom, selection of two footprints (B7 used U1+U2: between 26 est / touching 30), scope_mode between and touching:
  only connections within scope are routed (assert counts from connections_by_scope and that every routed connection belongs to the scope nets);
  accepted, routing_new 0; unrouted out-of-scope connections are expected (do not assert unconnected 0 there).
- Then: `.venv/bin/pytest tests/gate -m "gate or not gate" -q -s` green (report pass count and runtime), and progress PNG for the gate run:
  `.venv/bin/python -m harness.progress --milestone m4 --task B12 --router topor --board <board>` for eprom-to-kernal and wiccon-2023 if that
  harness entry still works; otherwise copy the gate test's preview.png per board to
  docs/benchmarks/progress/2026-10-31-m4-B12-gate-<board>.png with the numbers in the caption/report. `git add -f` for docs/benchmarks/.

Acceptance: tests/plugin green; default `.venv/bin/pytest -q` green (test_scaffold: if it fails under load rerun alone and report); gate command green.
Commits: `fix(plugin): VIA_NOT_SET_UP applies the via fallback to the rules (Q-b)` and `test(gate): M4b headless gate eprom + wiccon, scope selection`.
