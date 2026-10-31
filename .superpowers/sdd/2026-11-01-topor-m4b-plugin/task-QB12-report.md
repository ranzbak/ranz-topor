# Task QB12 report: Q-b via fallback applied + B12 Step 1 headless gate

Status: DONE. Branch `worktree-agent-ad9d130561c12ce9a`. The worktree base was stale (d9aab67, had no run.py), so I
fast-forwarded it to main 8b2a7b6 first. Symlink `scratch -> /home/paul/work/kicad/plugins/topor/scratch` is
untracked and not committed.

## Part A: Q-b, ruling C27 (commit 4536a28)
- The engine reads the via size from `Rules.default` / `Rules.per_net[code]` → `NetRule.via_d` / `via_drill`
  (engine/src/vias.rs `via_sizes`, plus check.rs and obstacle.rs for the max inflation). Those fields, in nm, are
  the only ones set.
- `plugin/topor/run.py`:
  - New `apply_via_fallback(rules, cfg) -> (new_rules, info)`. It is pure and O(#per-net rules).
    - It sets `default.via_d` / `via_drill` from `vias.fallback_diameter_mm` / `fallback_drill_mm`.
    - It sets the same on every per-net rule that carries the "not set up" size: KiCad's factory
      (800_000, 400_000), or the default rule's own pre-patch size. A per-net size set to any other value is kept.
    - The board minimums `min_via_diameter` / `min_via_drill` still apply; `info["clamped"]` is set when they
      raise a value.
  - New `via_fallback_text(info)` gives the report line.
  - The flow loads `.kicad_pro` right after `resolve_rules`, so the pro load moved earlier. When
    `preflight.via_setup_missing(pro)` is true:
    - The rules are patched before `port.read_model`, so the IPC model embeds them.
    - The model's own `rules` are patched as well (FakeBoard / harness model; idempotent for IPC).
    - `RunOutcome.via_fallback` holds the info for the engine's input.
    - The fallback is listed in log.txt, in report.md under Settings → "Accepted fallbacks: via fallback 0.8 / 0.4 mm
      (diameter / drill) applied to the default rule and N/M net rules (VIA_NOT_SET_UP)", and in result.json as
      `"via_fallback": {...}` (null when not applied).
- `plugin/topor/preflight.py`: `_via_setup_missing` is now public as `via_setup_missing(pro, cfg=None)`, with a
  docstring and Big-O. The finding logic is unchanged.
- `toporlib/rules.py` and `report.py` are unchanged: the report already had a `fallbacks` slot.
- Tests (tests/plugin/test_run_flow.py, eprom, real kicad-cli). The engine double `CaptureEngine` records the
  model and options sent to `route_async`.
  - `test_via_not_set_up_applies_fallback_to_engine_rules`: every net class is 0.8/0.4 with no via list, and the
    model rules are set to toporlib's unset 0.6/0.3, so the patch is discriminating. Result: the engine rules are
    all (800_000, 400_000); result.json and report.md mention it.
  - `test_via_fallback_from_topor_toml`: topor.toml sets 0.7 / 0.35 and is read with cfg=None. Every rule
    becomes (700_000, 350_000).
  - `test_via_sizes_set_up_leaves_rules_unchanged`: eprom's own pro (Signal 0.6/0.3). No finding; the engine
    rules equal the model rules; `via_fallback` is null; no report line.
  - `test_apply_via_fallback_pure`: tests the per-net selection, no mutation of the input, and the clamp to the
    minimums.
  - The tests failed before the change (AttributeError / wrong sizes) and pass after it.
- Snapshots:
  - VIA_NOT_SET_UP does **not** fire on eprom: the Signal class is 0.6/0.3.
  - It **fires on wiccon-2023**: one Default class at 0.8/0.4, no via list. Wiccon's resolved rules already carry
    (800_000, 400_000) on the default rule and all 38 net rules, so `apply_via_fallback` with the defaults is an
    identity (`new == rules`, checked).
  - Wiccon before / after: identical engine input, so identical numbers. 62/62, vias 37, routing_new 0,
    unconnected 0, outside 1373.553 mm (B5 fix round: same). No gate number changed.
  - It also fires on wedding-gift (Default 0.8/0.4), which I did not run.

## Part B: B12 Step 1 (tests/gate/test_m4b_gate.py)
- `project(board)`: `copy_project` from the BENCH_BOARDS snapshot, `load_board` (pcbnew, test side). When the
  file version is below KICAD10_VERSION (wiccon is v6, ruling C21), the copy is re-saved with
  `harness.writer.save` (pcbnew SaveBoard) and reloaded; eprom is already KiCad 10. Then it runs harness
  `resolve_rules` + `export_model`. This is the same construction as test_run_flow.py. The flow is
  `topor.run.run` itself; `_ok` / `_result` are imported from test_run_flow, so there is no second flow.
- `test_m4b_gate_headless_eprom_and_wiccon[eprom-to-kernal|wiccon-2023]` (marked `gate`):
  1. First-run path: the copy has no topor.toml. `run(..., cfg=None)` with `Decision(proceed=False)` gives:
     - topor.toml created;
     - a CFG_CREATED finding;
     - `first_run_notice(path)` called once;
     - `load_config` of the new file gives no findings and equals the defaults;
     - the file is commented.
     The flow creates the file only in this case: `cfg is None` and no file (`config.load_config`).
  2. `update_config(toml, {"run.existing_copper": "rip"})`, which is the user's one edit. Then
     `run(..., cfg=None)` with `ScriptedUi(accept=True)` and the real engine. Assertions:
     - status accepted, no CFG_CREATED, existing_copper "rip" in the report facts;
     - exactly one FakeBoard commit (items == out.items, removals == the rip plan);
     - routed == total == 27 / 62.
  3. The committed board (FakeBoard's second `save_copy`) is judged by kicad-cli (`run_drc`, refill zones) and
     classified against the flow's baseline and original DRC:
     - routing_new 0, `len(unconnected) == 0` (absolute) and unconnected_new 0;
     - the flow's own `out.drc` is 0/0;
     - report-dir `drc.json` equals the flow's kicad-cli `routed/drc-routed.json`, which is where its numbers come from.
  4. The report dir has report.md, result.json, drc.json, preview.png and log.txt, all non-empty. result.json shows
     status accepted, routed/total and drc.routing_new 0. report.md has "Outcome: applied as one undo step".
  5. `python -m topor.docs_gen --check` exits 0.
  6. The snapshot dir hash is unchanged.
- `test_m4b_gate_scope_selection[between-26|touching-30]` (gate): eprom, `Selection(footprints=["U1","U2"])`,
  existing_copper rip.
  - `scope_from_selection(...).connections == connections_by_scope[mode] ==` 26 / 30, and the pre-flight summary
    carries the same `connections_by_scope`.
  - Accepted with one commit.
  - Every connection in the result and every routed connection / item net is in the scope nets; total ≤ the
    estimate.
  - The ripped copper belongs to scope nets.
  - routing_new 0 (committed copy and flow). Unconnected is printed, not asserted.
  - I first added a `touching < whole` check of my own. It was wrong because U1+U2 touch every net (touching ==
    whole == 30), so it is now `<=`. This was not a brief assertion.

### Numbers (from the full gate command run)
```
GATE M4b eprom-to-kernal: routed 27/27 vias 0 routing_new 0 unconnected 0 (new 0) outside 64.628 mm router 0.06 s DRC rounds 1 stop converged wall 11.2 s
GATE M4b wiccon-2023: routed 62/62 vias 37 routing_new 0 unconnected 0 (new 0) outside 1373.553 mm router 4.42 s DRC rounds 1 stop no gain wall 19.5 s
GATE M4b scope between U1+U2: est 26 routed 24/24 nets 23/24 vias 0 ripped 49 routing_new 0 unconnected 1 (out of scope, not asserted) router 0.07 s
GATE M4b scope touching U1+U2: est 30 routed 27/27 nets 25/26 vias 0 ripped 53 routing_new 0 unconnected 0 (out of scope, not asserted) router 0.15 s
```
Wiccon's router_s was 5.30 s in the first standalone run and 4.42 s in the gate run. Both are under 10 s (budget).

### Commands
- `.venv/bin/pytest -q tests/plugin`: 182 passed, 7 skipped (235 s), with Part A.
- `.venv/bin/pytest tests/gate -m "gate or not gate" -q -s`: **21 passed in 562.8 s** (M0–M3, locked copper,
  fuzz and M4b). The M2/M3 eprom and wiccon numbers are unchanged (27/27 vias 0; 62/62 vias 37).
- `.venv/bin/pytest -q` (default suite): **349 passed, 7 skipped, 26 deselected in 1360 s**. The machine was loaded
  by two other workers' pytest runs; test_scaffold passed.
- Commits: 4536a28 `fix(plugin): VIA_NOT_SET_UP applies the via fallback to the rules (Q-b)`, 903025b
  `test(gate): M4b headless gate eprom + wiccon, scope selection` (tests and the two PNGs), plus this report.
- Progress PNGs. The harness entry still works:
  `.venv/bin/python -m harness.progress --milestone m4 --task B12 --router topor --board <b>` produced
  - docs/benchmarks/progress/2026-10-31-m4-B12-eprom-to-kernal.png: "27/27, vias 0, DRC routing 0, unconnected 0,
    outside 64.6 vs hand 76.6 mm, router 0.07 s, detours > 1.5: 0 (max 1.3), pcb bfaf705c0f25";
  - docs/benchmarks/progress/2026-10-31-m4-B12-wiccon-2023.png: "62/62, vias 37, DRC routing 0, unconnected 0,
    outside 1373.6 vs hand 1690.1 mm, router 6.42 s, detours > 1.5: 0 (max 1.4), pcb 2d3063e70ed5".
  These are harness (pcbnew bench) renders of the same engine run. The plugin flow's own preview.png sits in each
  gate report dir.

## Concerns / notes
- R8 (`keep_unconnected_vias`, rip_plan) may change which hand vias wiccon rips. The coordinator re-runs
  `tests/gate/test_m4b_gate.py` after merging; ripped counts are not asserted (only routed == total, one commit,
  routing_new 0, unconnected 0).
- The scope 26 / 30 estimates are asserted exactly (B7 values, deterministic, harness model).
- The 0.8/0.4 default fallback is an identity on every snapshot board where it fires. Only a changed toml value (or
  a project whose classes lack via fields) changes engine input.
- The gate test imports helpers from tests/plugin/test_run_flow.py (`_ok`, `_result`). Renaming them there breaks
  the gate import.
