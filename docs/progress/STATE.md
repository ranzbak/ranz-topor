# STATE

**M1 (taut smoother) — DONE 2026-10-31** (gate verified by main session: `pytest tests/gate -m "gate or not gate"` → 7 passed).
Rotation 3 coordinator ended on an auth error right after gate green; this header written by main.
eprom: 25/27 wires taut + 2 zero-slack fallbacks (+5V, user ruling), 8 arcs, DRC routing 0, unconnected 0,
outside-pad length 71.494 vs hand 76.598 mm, router ~4 ms. Benchmark docs/benchmarks/2026-10-30-topor-0.1.0/ (eprom, wiccon,
dcsgone, nametag-sao, wedding-gift). dcsgone routing_new fixed (882f94c). Version 0.1.0.
**Carry into M2 (first tasks):** F-C2 via holes as obstacles in the checker; CDT failure "constraint crosses another
constraint" (all wires fall back on wiccon-2023 and wedding-gift); M1.5 F2 SS mitre, F3–F5 (docs/progress/notes/m1-gate-review.md);
zero-slack check O(emitted×fallback) → spatial-index local query; nametag fallbacks (self-check 13, hand vertex on CDT vertex 8).

---

# topor — progress state

## Current milestone
M1 (taut smoother) — IN PROGRESS. Coordinator rotation 1 handed over 2026-10-30 at main@HEAD (after ~17 worker round-trips).
Plan: docs/superpowers/plans/2026-10-30-topor-m1-smoother.md. SDD ledger (rulings, briefs, reports, eprom dumps):
.superpowers/sdd/2026-10-30-topor-m1-smoother/progress.md. M0 history: .superpowers/sdd/2026-10-30-topor-m0-harness/.
Gate BLOCKED on 2 user rulings (see Open issues 1–2). Tasks 1–8 done; Task 9 (0.1.0, gate test, benchmark) + F gate review remain.

## M1 task status (plan task # / id / tier)
| # | Task | Status |
|---|------|--------|
| 1 | obstacle model (T1.1, S) | DONE 9fc15f5 |
| 2 | wire extraction (T1.2a, S) | DONE 34e615f |
| 3 | CDT + crossings, spade 2.15.1 (T1.2b, S) | DONE 3dce729 |
| 4 | funnel + sketch (T1.2c, F note → S) | DONE 5825b4e; note docs/progress/notes/m1-sketch-design.md |
| 5 | taut chain (T1.3a, F note → S) | DONE (merge 60e33df); note m1-taut-design.md |
| 6 | stacks + relax (T1.3b, S) | DONE 4773a89 |
| 7 | harness smoother/remove_items/bench/progress (T1.4b, S) | DONE 709f874; caption fix feced21 |
| 8 | smooth() + self-check + fallback + PyO3 (T1.4a, S) | DONE 155cd9f |
| R/R2/R3 | eprom re-baseline (KiCad 10 hand re-route), fab floors, teardrops, netclasses, harness.measure, edit-proof tests | DONE da7544d, 0c8b158, b32ba9f |
| Fix-1 | F diagnosis (m1-eprom-diagnosis.md) fixes 1–4 | DONE dedd7b3 |
| 9 | M1 gate, version 0.1.0, benchmark 0.1.0, images (S, F review) | TODO (needs rulings) |

## Decisions / rulings
- M1 (rotation 1): commit attribution Opus 5.5. Fab minimums are FLOORS (user): design values below the JLCPCB profile are raised and reported (`apply_fab_floors` opt-out); JLCPCB copper-edge floor 0.2 mm (user); hole_clearance floor = PTH 0.35 (not binding on eprom).
- Edge clearance read from .kicad_pro via LoadProject (eprom: 0.25 mm). Netclasses via GetEffectiveNetClass (regex patterns ok).
- Engine self-check runs at design clearance (margin 0); geometry is built with ε_build 5 µm (ε is headroom, KiCad DRC is the rule).
- Nets without hand tracks (eprom GND: pour + 17 free stitching vias) → net status "skipped", reason "no tracks (pour-served)", excluded from nets_total.
- Teardrops: when a wire is replaced, same-net teardrop zones touching removed items are deleted and counted (`teardrops_removed`); KiCad regenerates them. Pour/teardrop zones are not engine obstacles (rule areas only). DRC runs with --refill-zones.
- eprom tests are edit-proof: expectations derived from the copy at test time; `python -m harness.measure --board eprom-to-kernal` prints the numbers to pin before the gate.
- Engine is installed into .venv with `VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release --manifest-path engine/Cargo.toml --features python` (build.sh --dev touches the KiCad plugin dir).
- F-designed tasks 4–6 are reviewed in the F gate review (not per task).
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
1. USER RULING PENDING (asked via main): length gate. Emitted chains attach at pad centres; eprom KiCad track+arc 114.894 mm vs hand 112.886 mm (FAIL) while outside-pad length 71.494 vs 76.598 mm (PASS). Options: gate on outside-pad length, or clip at pad edge (connectivity risk). Do not relax without the ruling.
2. USER RULING PENDING: +5V wires 22/23 on eprom have zero slack (hand at exactly 0.25 mm to edge and J1:1/J1:3) → infeasible with ε → hand fallback; gate wants 27/27 taut. Options: accept zero-slack fallback / ε=0 for walls / gate fails.
3. eprom source may still be edited by the user: re-run harness.measure on a fresh copy right before the gate and report the pinned numbers.
4. Known engine limits (for F gate review): sketch stack-order fan weakness (wires entering a fan on different edges); taut drop rule (d) bbox branch removed in Fix-1 (negative arcs); degenerate arc→segment not merged (pattern may contain "SS", violates the plan constraint); T-split kept item replaced only if all its wires are taut; wall parallel runs need foot anchors (M1.5).
5. From M0: rip stitching-via policy question (before M3); F-C2 via holes as obstacles for NEW vias (before M2); polygon holes exported as outer ring only.
6. Old M0 worktrees under .claude/worktrees/ (9) are merged and can be removed; target/release/trace* are stray F scratch build outputs.

## Last gate results (M0, main@7cbc33e; eprom numbers = OLD KiCad 5 board)
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
  M1 additions: engine/src/{obstacle,wires,cdt,sketch,taut,relax,smooth}.rs (+ tests/*.rs per module),
    engine/examples/extract_wires.rs, py.rs smooth(); harness/{smoother,progress,measure}.py, writer.remove_items,
    bench --router smoother; docs/progress/notes/m1-{sketch,taut}-design.md, m1-eprom-diagnosis.md; docs/benchmarks/progress/

## Last results (M1, eprom-to-kernal KiCad 10 copy, src sha256 bfaf705c…)
- measure: 53 seg, 0 arc, 17 via (all free), 112.886 mm, widths 0.25×43 / 0.4×10, 25/26 nets with copper, orig DRC 0/0, engine wires 27.
- smoother (T1.4a-fix1): 24/25 nets, 25/27 wires taut, 8 arcs, DRC routing 0 / unconnected 0, width_match 25/25, router 0.004 s, KiCad length 114.9 mm, outside pads 71.5 vs 76.6 mm.
- Progress PNGs: docs/benchmarks/progress/2026-10-30-m1-T1.4a.png, 2026-10-30-m1-T1.4a-fix1.png (both sent to main).
- Tests: cargo all suites green, clippy clean; pytest default 76 passed.

## Next 3 actions
1. Get the user's rulings on Open issues 1–2 (main relays).
2. Task 9: version 0.1.0, tests/gate/test_m1_gate.py with the ruled criteria, benchmark 0.1.0 (eprom + informational wiccon/dcsgone runs: must complete, no new routing violations), progress PNG T1.5, re-measure eprom first.
3. F gate review (complexity of new loops, Tasks 4–6 algorithms, Open issue 4), then M1 handover.
