# topor — progress state

## Current milestone
M0 (harness + engine skeleton). Plan: docs/superpowers/plans/2026-10-30-topor-m0-harness.md.
Coordinator rotation 1 ended mid-milestone (forced hand-back); a successor coordinator is needed.
SDD ledger (full history, rulings, briefs, reports, review diffs): .superpowers/sdd/2026-10-30-topor-m0-harness/
(progress.md = ledger; worker-rules.md + global-constraints.md + file-structure-and-pcbnew-facts.md are reused in every brief;
task-N-brief.md has already been extracted for all 14 tasks).

## Task status (plan task # / 45 id / tier)
| # | Task | Status |
|---|------|--------|
| 1 | scaffold (T0.1, H) | DONE d5dbec7 (attribution line says Haiku: amend blocked by hook) |
| 2 | geom vec/prim/tangent (T0.5a, S) | DONE aceef90..10389ef, reviewed, fix round 1 addressed |
| 3 | geom distances/skeletons (T0.5b, S, F review) | FIX ROUND 1 IN FLIGHT: uncommitted edits to dist.rs/dist_props.rs in main tree by worker a9d4e436482482d06. Then re-check: round_rect rc=min/2 regression tests, oracle tol <= 1 nm |
| 4 | grid (T0.5c, S) | committed 95d5507; review: CRITICAL open (fix round 1 NOT yet dispatched): non-finite/huge bbox -> i32 saturation/overflow + unbounded cell loops; query O(area) for big boxes; remove() silent no-op on bbox mismatch. Resume worker a3f319d5d02325c5b or start a fresh sonnet worker |
| 5 | quant + arc centre (T0.5d, S) | 267e9fb on branch worktree-agent-a3a30c1216025eb10 (NOT merged); FIX ROUND 1 IN FLIGHT by worker a3a30c1216025eb10 (drift-bound derivation, degenerate boundary tests, no zero-length seg). Merge into main after it finishes |
| 6 | model/check/PyO3/build (T0.1+T0.2, S) | pending: needs 2-5 |
| 7 | harness paths/copy/fixtures (T0.3a, S) | DONE 83acf4a |
| 8 | harness rip (T0.3b, S) | DONE 1ab8d35 |
| 9 | harness model export (T0.3c, S) | DONE 5675ade |
| 10 | rules (T0.4, S) | pending: needs 6 (one test uses engine check_items) |
| 11 | writer (T0.6, S) | pending: needs 6 |
| 12 | DRC runner (T0.7a, H) | DONE 1a44c0a |
| 13 | render/metrics/bench (T0.7b, H) | pending: needs 7-12 |
| 14 | fuzz + M0 gate (T0.8, S, gate review F) | pending |

## Decisions / rulings
- Arc-centre agreement with KiCad <= 100 nm; no bit-exact CalcArcCenter port. EPS_BUILD_NM = 5000 nm.
- kicad-python pinned `>=0.8,<0.9`. Source boards read-only. Python only via .venv.
- Per-task review only for F-marked tasks and geometry-kernel diffs; other tasks are reviewed by their acceptance tests.
- T4 scaling test uses constant density (the lattice grows with n), not a fixed 100 mm2 area, which is what O(1)/query means.
- T5 degenerate rule: sweep > pi is never degenerate (Arc::chord()->0 as sweep->2pi).
- T6 MUST add a pcbnew-based test: random (s,m,e) -> PCB_ARC.GetCenter vs engine.arc_center_nm, |delta| <= 100 nm (closes the T5 review gap).
- T6 serde must accept the extra hole key `"slot":[w,h]` emitted by harness/model.py (no deny_unknown_fields, or model it).
- T9: the plan's "eprom 5 distinct refs" is wrong. There are 5 footprints, but 2 are pad-less REF**, so refs = {J1,U1,U2}.
- T9: net-tie groups are parsed via PCB_IO_KICAD_SEXPR().Format(fp) (fp.Serialize needs protobuf); copper polys use the outer ring only; zone fills are not exported.
- T8: nets_to_route comes from GetConnectedPads (SWIG can't iterate the ratsnest); the free-via check precedes the RF check.

## Open issues
- T3 fix (in flight), T4 Critical fix (not dispatched), T5 fix (in flight, unmerged branch).
- The default pytest suite takes ~34 s (DRC runs); fine.
- Worktrees under .claude/worktrees/ (ignored) can be removed after merge.

## Last gate results
- None (M0 gate not run). Last suites on main@95d5507: cargo all green; pytest 28 passed, 1 deselected.

## File map
topor/
  .gitignore                     target/, plugin/wheels/, __pycache__/, .venv/, scratch/, *.kicad_pcb-bak, docs/.ruff_cache/
  Cargo.toml                     workspace: members = ["engine"]
  pytest.ini                     testpaths = tests; pythonpath = .; markers gate, scaling
  engine/
    Cargo.toml                   package topor-engine 0.0.1, lib name topor_engine, crate-type ["rlib","cdylib"],
                                 features python = ["dep:pyo3"]; pyo3 0.23 features ["abi3-py39","extension-module"]
    pyproject.toml               [build-system] maturin>=1.7; [tool.maturin] features=["python"], module-name="topor_engine"
    src/lib.rs                   pub mod geom; pub mod model; pub mod check; pub mod fuzz; #[cfg(feature="python")] mod py;
    src/geom/mod.rs              re-exports; pub const EPS_BUILD_NM: f64 = 5000.0; MIN_SAGITTA_NM = 5.0; MIN_CHORD_NM = 10.0
    src/geom/vec2.rs             Vec2, Orient
    src/geom/prim.rs             Circle, Seg, Arc, Aabb, Prim
    src/geom/tangent.rs          point_circle_tangent, bitangent, all_bitangents
    src/geom/dist.rs             point_seg … arc_arc, dist_prim, Skeleton, dist_prim_skeleton
    src/geom/grid.rs             Grid<T>
    src/geom/quant.rs            IVec2, round_nm, calc_arc_center, TrackPrim, quantise, dequantise
    src/model.rs                 serde types: BoardModel, Pad, PadShape, Hole, CopperShape, Outline, KeptItem, Rules, NetRule, Options; Y flip
    src/check.rs                 Checker: build grid from model, check_items() → CheckReport
    src/fuzz.rs                  fuzz_items(model, options, seed, n) → Vec<TrackPrim>
    src/py.rs                    PyO3 module topor_engine
    tests/                       cargo integration tests per module (props with proptest)
  plugin/
    plugin.json                  identifier "org.topor.router", runtime python, entrypoint "topor_plugin.py"
    topor_plugin.py              M0: prints engine version; checks engine version == shipped version
    requirements.txt             generated by tools/build.sh
    wheels/                      ignored
  harness/
    __init__.py
    paths.py                     kicad_cli(), plugin_dir(identifier), venv_dir(identifier), nm/mm helpers
    fixtures.py                  make_fixture_board(), write_project_file()
    copy.py                      copy_project(), sha256_tree()
    rip.py                       record_widths(), rip_tracks()
    model.py                     export_model(board, rules) → dict; load_board(pcb) (loads project first)
    rules.py                     resolve_rules(), parse_dru(), JLCPCB profile
    writer.py                    write_items(), group handling
    drc.py                       run_drc(), classify(), summarise()
    render.py                    strip_zone_fills(), render_png(), compose_compare()
    metrics.py                   copper_metrics(), runtime fields
    bench.py                     run_bench() end-to-end for one board
  tools/
    dev-venv.sh                  creates .venv (--system-site-packages), installs maturin pytest
    build.sh                     wheel → plugin/wheels → plugin dir; --dev reinstalls into KiCad venv
    bench.sh                     wrapper around harness.bench
    render.sh                    wrapper around harness.render
  tests/
    conftest.py                  fixtures: tmp fixture board path, engine import, kicad_cli skip guard
    test_scaffold.py … test_fuzz_gate.py
    gate/test_m0_gate.py         marked `gate`; uses benchmark board copies
    scaling/test_grid_scaling.py marked `scaling`
  docs/progress/STATE.md         living state (45 §4), created in Task 1

## Next 3 actions
1. Collect the T3 fix (check `git status`; worker commits dist.rs). Verify with cargo test, then ledger T3 complete. Then dispatch the T4 fix round (Critical above) in the main tree.
2. When the T5 fix returns: `git merge worktree-agent-a3a30c1216025eb10`, run cargo test, ledger T5 complete.
3. Dispatch T6 (sonnet) with the two T6 rulings above, then T10 ∥ T11 (worktrees), T13 (haiku), T14 (sonnet) + F gate review.
