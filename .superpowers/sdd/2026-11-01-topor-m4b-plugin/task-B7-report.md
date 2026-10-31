# Task B7 report: scope, copper policies, priorities

Status: DONE_WITH_CONCERNS (see Concerns).

## Base
The worktree started at d9aab67 (no run.py). I fast-forwarded the branch to `main` (520ea88) before working.

## What changed
- `plugin/topor/priority.py` (new): `parse_marker`, `net_priorities(nets, netclass_of, cfg) -> (dict, findings)`. Precedence marker > `topor_pN` > `[nets] priority` (exact, then glob, lowest number wins) > power (netclass name contains `power`, or `power_pattern` for nets whose class is Default or missing; tier 2 only when no other source set one). Findings: MARKER_IGNORED (any `@<digit>` that is not a valid marker, one finding listing all), MARKER_CONFLICT, MARKER_PAIR, POWER_BY_NAME (info). Pair detection: last chars `_P/_N`, `+/-`, `P/N`, with the marker removed from the key so halves with different markers still pair; unmarked half follows its partner silently.
- `plugin/topor/scope.py` (new): `ScopePlan`, `RipPlan`, `scope_from_selection(sel, model, cfg, whole=False)`, `connections_by_scope(sel, model, cfg) -> {between, touching, whole}`, `rip_plan(model, plan, cfg)`, `do_not_touch_nets`. Big-O in every docstring and in the module docstring.
- `plugin/topor/run.py` (seams only): old `rip_plan` removed (imported from scope); `build_options` takes keyword `plan, rip, model, priorities`; call sites: `RunSummary.connections_by_scope` real counts, priority and SCOPE_EMPTY findings added before the pre-flight dialog, plan rebuilt after the decision with `whole=decision.overrides["scope_whole"]`, `rip = rip_plan(model, plan, cfg)`.
- Tests: `tests/plugin/test_priority.py` (18), `tests/plugin/test_scope.py` (9, incl. eprom oracle + real engine).

## Path chosen (engine vs Python filtering)
The engine already has `Options.scope` {pads, nets, area, unrouted_only} with MST/lone-terminal rules (decompose.rs), so the scope is passed to the engine (no Python model filtering). The engine lacks footprint-level `between`, so footprints are expanded in Python to pad lists:
- between: pads of selected footprints/pads/area whose net has >= 2 selected pads (engine then builds the MST among them only).
- touching: endpoints of every Euclidean-MST edge (per net, Prim, nets over 200 pads use one connection per selected pad) that has a selected endpoint; `area` is also passed natively.
- selected tracks/vias select their whole nets (`scope.nets`).
- `engine_scope.footprints` is always empty (expanded). `do_not_touch` nets: pads removed from lists; on the whole board `scope.nets` lists every other net (engine has no `exclude_nets`; build_options also passes `exclude_nets`, `ignore_pads`, `net_priority` when `default_options()` knows them; today it knows none, so priorities are computed and reported but not yet used by the engine).
- smooth: engine gets `existing_copper = "smooth"` (route() currently warns and treats it as fixed, route.rs:2590); fixed/rip: engine gets fixed (the plugin rips).

## Results
- eprom, select U1+U2: between = 26 estimated connections (engine decomposed 24 with copper removed; estimate ignores pours/ties), touching = 30. Real engine route with the `between` scope routes only nets that have >= 2 pads on U1/U2 (`routed_nets <= plan.nets`), and the `touching` scope yields more connections.
- `.venv/bin/pytest -q tests/plugin` (all, incl. test_run_flow with real engine and kicad-cli): 139 passed, 7 skipped (live-KiCad tests), 650 s.

## Concerns
- Brief precedence (marker > netclass > `[nets] priority`) contradicts the schema text for `[nets] priority` ("overrides the @N marker and topor_pN netclasses"). I followed the brief. B6/schema owner should align the text.
- Connection counts are spanning-tree estimates (no knowledge of existing copper joining pads).
- Engine does not consume `net_priority` / `exclude_nets` / `ignore_pads` yet (A9); nothing else changes when it does.
- `rip` in whole mode now only rips copper whose net is a named net and not in do_not_touch (old code ripped every track including net 0).
- The first TDD step could not be shown red for priority.py (tooling refused the combined command); tests were written before running, one test bug (pair key) and one implementation gap (pair key ignoring the marker) were fixed after the first run.
