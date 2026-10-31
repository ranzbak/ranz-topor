# SDD ledger — plan: docs/superpowers/plans/2026-11-01-topor-m4-in-kicad.md (Part 1, M4b plugin 0.4.0)
Coordinator rotation 1 (2026-10-31). Spec: docs/design/45-mvp-plan.md + 46-configuration.md (reachable). BASE main@07864ea.
Projects tree sha (filtered find, sha256sum | sha256sum, 16 hex) before rotation 1: 8153f4de51ba4d65 (= baseline).
Ledger path per main (not the plan's 2026-11-01-topor-m4-in-kicad). Briefs extracted by awk on "### Task <ID>" (task-brief script needs numeric ids).

## Main rulings (STATE M4-R1..R6) — verbatim in global-constraints.md

## Preflight scan
| pair / task | produces vs consumes | finding |
|---|---|---|
| P1 ‖ B0 | P1 engine (route.rs stop rule, RouteResult.stop_reason, py route_async); B0 harness/bench.py → toporlib.drcloop | no shared file; B0's pinned fixture JSON may shift if P1 changes the fixture result → Ruling C6 |
| P1 ‖ B1 | P1 default_options(); B1 test_every_engine_option_is_a_setting_or_internal | plan: xfail(strict=False) until P1 merged; coordinator re-runs after both merge |
| P1 ‖ B4 | none | ok |
| B0 ‖ B1 | B0 toporlib/*; B1 plugin/topor/{schema,config,docs_gen} | no shared file; both may create tests/plugin/conftest.py → merge-time trivial |
| B2 ← B0 | toporlib.pcbfile used by FakeBoard.commit — but pcbfile is created in B3 | Ruling C7 |
| B3 ← B0,B2 | pure rules (toporlib/rules.py) vs harness/rules.py SWIG wrapper | sequential, ok |
| B5 ← P1,B1,B2,B3 (B6/B7/B8 later) | run() consumes preflight/scope/report "whole-board / minimal until then" | α path skips B6–B8 → Ruling C8 |
| B1 ↔ M4-R4 | B1 `report_dir = "topor-report"`; B5 writes project_dir/report_dir | conflicts with M4-R4 → Ruling C1 |
| B9 ← B4 | wording table parsed from the note | B4 must emit a machine-parsable wording table → told in B4 dispatch |
| B10 ↔ B12 | B10 plugin.json 0.4.0; B12 Cargo.toml/SHIPPED_VERSION 0.4.0 | inconsistent α install → Ruling C9 |
| P1 self | cancel test "dcsgone model" in a cargo test | needs a model fixture → Ruling C10 |
| B0 self | test_no_pcbnew imports topor.app (B10) — until then topor.ports (B2) | B0 lands before B2: guard imports `topor` / toporlib only until B2 → Ruling C11 |
| B1 self | consistent (since-0.5.0 keys commented out) | ok |
| B2 self | test_kipy_surface needs plugin venv interpreter | present, kipy 0.8.0 → ok; .venv also has kipy now (C2) |
| B3 self | parity tolerance "0 nm, hulls" vs M4-R5 "polygon-equivalent within 1 µm" | M4-R5 wins for pads; other coordinates 0 nm |
| B5 self | report.write to project_dir/report_dir | C1 |
| B10 self | build.sh test with scratch KICAD_DOCUMENTS_HOME | C3 |

## Rulings
- Ruling C0: execution as M3 P0 — worktree workers in parallel, no per-task review agents (acceptance commands + coordinator verification after merge); F review only for B4 (wireframes), B3 at the gate, B12; exactly one worker owns the shared .venv at a time (group 1: P1) — cost if wrong: defects found at merge/gate instead of per task.
- Ruling C1: report_dir default "" → ${TOPOR_REPORT_ROOT:-${XDG_CACHE_HOME:-~/.cache}/kicad/10.0/topor}/<project>/<timestamp>/, relative → under project dir, absolute as-is; tests set TOPOR_REPORT_ROOT — implements M4-R4 over the plan's "topor-report" — cost if wrong: one default string + docs.
- Ruling C2: kicad-python 0.8.0 + protobuf 5.29.6 + pynng 0.9.0 installed into .venv (same as plugin venv) so plugin tests can import kipy — cost if wrong: system protobuf 4.21 shadowed in .venv (pcbnew import verified ok).
- Ruling C3: only user/main runs tools/build.sh --dev — it writes the real KiCad plugin venv — cost: none.
- Ruling C6: B0 pins harness-level outputs of the fixture run (routed/total, routed_drc summary, drc_loop rounds, item count + sorted item digest), not engine-internal report fields that P1 adds; if P1's merge changes the fixture result the coordinator re-pins only with P1's report explaining it — cost if wrong: one re-pin.
- Ruling C7: toporlib/pcbfile.py `find_items/remove_items/append_items` move from B3 into B2 (FakeBoard.commit needs them); B3 keeps parse_layers + the snapshot roundtrip test (B2 writes the find/remove/append part of it) — cost if wrong: none (same code, earlier).
- Ruling C8: M4b-α = P1,B0,B1,B2,B3,B4(+F review),B5,B9,B10; B5 uses a whole-board scope, no-op preflight (cfg findings only) and a minimal report (summary_lines ≤ 20, report.md, result.json, drc.json, preview.png, log.txt) until B6/B7/B8 — cost if wrong: α lacks pre-flight checks (GUI test notes it).
- Ruling C9: B10 bumps engine Cargo.toml, plugin.json and SHIPPED_VERSION to 0.4.0 together (tests pinning 0.3.0 updated) so the α install is consistent; B12 verifies — cost if wrong: none.
- Ruling C10: P1's cancel test may use any self-contained model whose uncancelled route takes ≥ 5 s (e.g. the 2-layer synth generator used by the scaling test, or a committed dcsgone model JSON if small enough); assertions as the plan — cost if wrong: weaker realism than dcsgone.
- Ruling C11: B0's test_no_pcbnew blocks pcbnew and imports every toporlib module (+ `topor` package if present); B2 extends to topor.ports, B10 to topor.app — cost: none.
- Ruling C12: B0/B1/B4 never touch .venv; P1 owns it in group 1 and runs maturin develop from its worktree; after merging P1 the coordinator rebuilds from main — cost if wrong: transient engine swap under a parallel pytest.

## Log
