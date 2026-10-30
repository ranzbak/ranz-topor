# topor — progress state

## Current milestone
M0 (harness + engine skeleton) — GATE PASSED 2026-10-30 (F review: PASS WITH CONDITIONS; C1 fixed in 7cbc33e). main@7cbc33e.
Next: M1 (taut smoother). The plan has only an M1 outline; a detailed M1 plan is needed first (new coordinator).
SDD ledger (full history, rulings, briefs, reports): .superpowers/sdd/2026-10-30-topor-m0-harness/
F gate note: docs/progress/notes/m0-gate-review.md.

## Task status (plan task # / 45 id / tier)
| # | Task | Status |
|---|------|--------|
| 1 | scaffold (T0.1, H) | DONE d5dbec7 |
| 2 | geom vec/prim/tangent (T0.5a, S) | DONE ..10389ef, reviewed |
| 3 | geom distances/skeletons (T0.5b, S, F review) | DONE ..0503bea, reviewed |
| 4 | grid (T0.5c, S) | DONE 95d5507..c7d40b0 (2 fix rounds, re-review APPROVE) |
| 5 | quant + arc centre (T0.5d, S) | DONE ..5f5f2fb (merged 884f8dd) |
| 6 | model/check/PyO3/build (T0.1+T0.2, S) | DONE 6384da3, fix 950714f (poly containment, keepouts, via drill vs holes) |
| 7 | harness paths/copy/fixtures | DONE 83acf4a |
| 8 | harness rip | DONE 1ab8d35; SWIG fix 94fd585 (board.Delete instead of Remove) |
| 9 | harness model export | DONE 5675ade |
| 10 | rules (T0.4, S) | DONE 2d15ec0 |
| 11 | writer (T0.6, S) | DONE 5cf8d54 |
| 12 | DRC runner | DONE 1a44c0a |
| 13 | render/metrics/bench (H) | DONE bc7c6ac |
| 14 | fuzz + M0 gate (S, F review) | DONE e4fed3c + gate conditions 7cbc33e |

## Decisions / rulings
- Arc-centre agreement with KiCad <= 100 nm (tested: 200 random arcs vs pcbnew, engine + writer). EPS_BUILD_NM = 5000 nm.
- kicad-python pinned `>=0.8,<0.9`. Source boards read-only (sha256 checked in every gate test). Python only via .venv.
- Wheel tag is cp39-abi3-manylinux_2_34_x86_64 (maturin default), not linux_x86_64.
- build.sh was run with KICAD_DOCUMENTS_HOME=<scratch>; the real KiCad plugin dir is untouched (install = `bash tools/build.sh --dev`).
- Grid: i64 cells clamped ±2^31 nm; non-finite bbox → try_insert Err / insert panics; items > 65 536 cells in a side list; remove() uses the stored range.
- Clearance = max(item net, other net/pad net, pad local, min_clearance) (7cbc33e).
- rip uses board.Delete (Remove corrupts SWIG state on real boards).
- Fuzz gate: track_dangling ≥ 1 (not ≥ 1000: same-net items merge); written item count asserted.
- dcsgonebadge orig DRC 192 vs baseline 210: diff only courtyards_overlap/lib_footprint_issues/starved_thermal (environment); other types exact.
- dcsgonebadge rip keeps 2 free vias / removes 201 (baseline 7/196) — pinned; rip policy open (see Open issues).
- Image compare: baseline alpha flattened on white, 60 px caption cropped. Topola-routed images cannot be regenerated (boards gone).
- Rulings T5/T6/T8/T9 from rotation 1 still hold (see ledger).

## Open issues
- QUESTION for user (before M3): stitching/fence-via policy for rip. README step 1 says keep stitching/fence vias; rip keeps only GetIsFree() vias. On dcsgonebadge that keeps 2 (baseline 7). F suggests "no track of its net touches it" (gives 4) or another definition.
- Before M2 (F C2): existing vias are copper-only obstacles (model.holes = pad drills only); new vias carry no hole → via hole-to-hole and track-vs-via-hole unchecked; same-net new vias may overlap.
- Minor (F C3): copper polygons exported as outer ring only → polygon holes give false positives (conservative).
- Minor: fuzz own cell 2 mm constant; hole_to_hole not in checker cell size formula; grid 65 536-cell items ~5 MB each; grid tests' 2 s timeout may flake under load.
- Worktrees under .claude/worktrees/ (ignored) are all merged; they can be removed.

## Last gate results (M0, main@7cbc33e)
- cargo test --release: 9 suites ok; clippy --all-targets -D warnings clean; pytest default 63 passed; gate+scaling 9 passed (30 s).
- No-router bench runtime_s: eprom 5.2, wiccon 6.6, dcsgone 12.5 (< 60). Orig metrics exact (dcsgone track 4167.082 + arc 43.197 mm). Orig DRC 93 / 48 / 192.
- Hand image RMSE vs baseline: 0.0036 / 0.0003 / 0.0025 (< 0.02). Images: docs/benchmarks/2026-10-30-topor-0.0.1/images/{eprom-to-kernal,wiccon-2023,dcsgonebadge}-hand.png.
- Fuzz (fixture, margin 5000): seeds 1/2/3 → 1000 accepted each (2814/2544/3620 attempts), truncated false, KiCad routing_new 0. Eprom ripped copy seed 1: 1000/20252, routing_new 0. Negative-margin control (−20 µm): 59–98 clearance violations.
- Scaling: grid_probe 1k→100k candidates/query 4.87→4.92 (1.01×), seconds 1.19× (bounds 4× / 20×).

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
1. Ask the user the rip stitching-via question (Open issues 1).
2. Write the detailed M1 plan (taut smoother, T1.1–T1.4; F design notes for T1.2/T1.3) from the outline in the M0 plan.
3. Schedule F-C2 (via holes as obstacles) as an early M2 task.
