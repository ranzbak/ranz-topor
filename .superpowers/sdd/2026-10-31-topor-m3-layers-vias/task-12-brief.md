### Task 12 (S, gate review F): M3 gate, 2-layer scaling, version 0.3.0, benchmark 0.3.0

**Files:**
- Create: `tests/gate/test_m3_gate.py`, `docs/benchmarks/<date>-topor-0.3.0/{README.md, images/, metrics/}`, `docs/progress/notes/m3-gate-review.md` (F)
- Modify: `tests/gate/test_m2_gate.py` (`threads_used == r["threads"]`), `tests/scaling/synth.py`, `tests/scaling/test_route_scaling.py`, `engine/Cargo.toml`, `plugin/plugin.json`, `plugin/topor_plugin.py` (0.3.0), `docs/benchmarks/README.md`, `docs/progress/STATE.md`

**Interfaces:**
- Consumes: `run_bench(board, out, router=route_router, rip="all", threads=4, options={...})`, `measured_hand_facts`, `sha256_tree`/`assert_unchanged`, `png_size`, `topor_engine.route`.
- Produces: `synth_model(n_pairs: int, seed: int, jitter_nm: int = 0, layers: int = 1) -> dict` — `layers == 2`: `layers` list gains B.Cu (id 31, routable), the second pad of every pair is SMD on B.Cu only (every connection needs one via); scaling test parametrised over `layers ∈ {1, 2}` with the M2 bounds and `vias == routed` asserted for `layers == 2`.

- [ ] **Step 1: Version 0.3.0** in `engine/Cargo.toml`, `plugin/plugin.json`, `SHIPPED_VERSION`; `bash tools/build.sh --dev`; `tests/test_engine_surface.py::test_version_matches_cargo` green.
- [ ] **Step 2: Scaling:** add the `layers` parameter and the 2-layer variant; run `.venv/bin/pytest tests/scaling -m scaling -q -s` — Expected: PASS for both variants (on failure: the one F diagnosis round of 45 §5a, then escalate).
- [ ] **Step 3: Write the gate test** (`tests/gate/test_m3_gate.py`)

```python
@pytest.mark.gate
def test_m3_gate_eprom_two_layers(engine, tmp_path):
    src = BENCH_BOARDS["eprom-to-kernal"]; hashes = sha256_tree(src.parent); facts = measured_hand_facts(tmp_path)
    r = run_bench("eprom-to-kernal", tmp_path / "bench", router=route_router, rip="all", threads=4)
    rep, rt, drc, hs = r["router_report"], r["route"], r["routed_drc"], r["hand_stats"]
    assert rep["nets_total"] == rep["nets_routed"] == 25 and next(n for n in rep["nets"] if n["name"] == "GND")["status"] == "pour_served"
    assert rep["connections_total"] == rep["routed"] == facts["wires"] and rep["partial"] == rep["failed"] == 0
    assert drc["routing_new"] == 0 and drc["incomplete"]["unconnected_items"] == 0, drc
    assert r["width_match"]["compared"] == r["width_match"]["matched"] == 25
    ra = r["routed_all"]; assert ra["arcs"] >= 1 and all("SS" not in c["pattern"] for c in rep["connections"])
    assert rep["length_outside_pads_nm"] <= hs["hand_outside_pads_nm"]
    assert r["router_s"] < 2.0 and rep["threads_used"] == 4 and not rep["timed_out"] and rep["iterations"] <= 50
    assert not rep["zero_slack_wires"] or r["zero_slack"]["status"] == "kept"      # target: zero_slack_wires == 0 (printed; Open question 4)
    model_json = (tmp_path / "bench" / "eprom-to-kernal--model.json").read_text()
    runs = {t: json.loads(engine.route(model_json, json.dumps({"threads": t}))) for t in (1, 2, 4)}
    runs["again"] = json.loads(engine.route(model_json, json.dumps({"threads": 4})))
    for x in runs.values(): x.pop("runtime_s"); x.pop("threads_used")
    assert len({json.dumps(x, sort_keys=True) for x in runs.values()}) == 1
    assert png_size(tmp_path / "bench" / "eprom-to-kernal-compare.png")[0] > 0
    assert_unchanged(src.parent, hashes)
    print(f"GATE M3 eprom: routed {rep['routed']}/{rep['connections_total']} nets {rep['nets_routed']}/{rep['nets_total']} vias {rep['vias']} "
          f"layers {rep['length_by_layer_nm']} outside-pads {rep['length_outside_pads_nm']/1e6:.3f} (hand {hs['hand_outside_pads_nm']/1e6:.3f}) mm "
          f"drc_new {drc['routing_new']} unconnected {drc['incomplete']['unconnected_items']} arcs {ra['arcs']} router_s {r['router_s']} "
          f"iters {rep['iterations']} zero_slack_wires {len(rep['zero_slack_wires'])} via_reduction '{rep['via_reduction']}'")

@pytest.mark.gate
def test_m3_gate_wiccon(engine, tmp_path):
    src = BENCH_BOARDS["wiccon-2023"]; hashes = sha256_tree(src.parent)
    r = run_bench("wiccon-2023", tmp_path, router=route_router, rip="all", threads=4, options={"partial_routes": False})
    rep, rt, drc = r["router_report"], r["route"], r["routed_drc"]
    assert rep["routed"] >= 0.9 * rep["connections_total"], (rep["routed"], rep["connections_total"])
    assert drc["routing_new"] == 0 and r["router_s"] < 10.0 and not rep["timed_out"]
    model_json = (tmp_path / "wiccon-2023--model.json").read_text()
    runs = [json.loads(engine.route(model_json, json.dumps({"threads": t}))) for t in (1, 4)]
    for x in runs: x.pop("runtime_s"); x.pop("threads_used")
    assert json.dumps(runs[0], sort_keys=True) == json.dumps(runs[1], sort_keys=True)
    assert_unchanged(src.parent, hashes)
    print(f"GATE M3 wiccon: routed {rep['routed']}/{rep['connections_total']} vias {rep['vias']} drc_new {drc['routing_new']} "
          f"unconnected {drc['incomplete']['unconnected_items']} router_s {r['router_s']} iters {rep['iterations']}")

@pytest.mark.gate
@pytest.mark.parametrize("board", ["dcsgonebadge"])
def test_m3_router_other_boards_complete(board, tmp_path):   # informational, as M2 (wall_clock_s 60, routing_new 0, router_s <= 65)
```

- [ ] **Step 4: Run** `.venv/bin/python -m harness.measure --board eprom-to-kernal` (re-measure), then `.venv/bin/pytest tests/gate -m "gate or not gate" -q -s` (M0–M3 gates; M2's threads assertion updated) — Expected: PASS. On an M3 criterion failure: one F diagnosis round (`m3-eprom-diagnosis.md` / `m3-wiccon-diagnosis.md`), fix tasks with progress PNGs, re-run; never relax a budget; if still failing, escalate to the user with the numbers.
- [ ] **Step 5: Benchmark 0.3.0:** `tools/bench.sh <board> docs/benchmarks/<date>-topor-0.3.0/work --rip all --router topor --threads 4` for eprom-to-kernal, wiccon-2023, dcsgonebadge, and nametag-sao / wedding-gift via `--pcb` on scratch copies (`wall_clock_s` 60 for the informational boards); copy `*-hand.png`, `*-topor.png`, `*-compare.png` to `images/`, `--metrics.json`/`--drc-*.json` to `metrics/`; `README.md` after the 0.2.0 template with new columns **vias**, **F.Cu / B.Cu mm**, **threads**, `via_reduction`; the gate criteria table for eprom and wiccon; observations per board; run row in `docs/benchmarks/README.md`; the `work/` directory is not committed. Progress PNGs `--task T12-gate` (eprom) and `--task T12-gate-wiccon`; send to main.
- [ ] **Step 6: Commit:** `git commit -m "feat: M3 gate, 2-layer scaling, benchmark 0.3.0, version 0.3.0"`.
- [ ] **Step 7 (F): Gate review** `docs/progress/notes/m3-gate-review.md`: complexity of every new loop (site generation, multigraph build/activation, layered search per expansion after RC3, batch commit, via-reduction pass, pour goals, persistent index, probe cap) against 45 §5a with file:line; Tasks 7–9 vs the design note; Review Focus tests 1–5 present and asserting the specified facts; determinism (no `HashMap` order reaches an output; batch commit proof); D1–D10 closure; findings BLOCKING / DEFERRED (M4). Fix BLOCKING items (S fix task + progress PNG), then STATE.md handover: M3 DONE header with numbers, carry-overs for M4.

## Deferred to M4 (recorded for STATE.md, with reasons)

1. **Dangling-end G1 continuation** — M4 "finishing hand-started traces" (45 §3); M3 routes from dangling ends as fixed terminals (unchanged from M2).
2. **Continuous via placement / Weiszfeld refinement (25 §3, T3.2 wording "in continuous space")** — M3 places vias at discrete sites that are CDT vertices; moving a via needs incremental CDT insert/remove plus remapping of `RouteGraph` indices and slot occupancy (10 §5); deferred to M4 together with incremental CDT (also needed by the DRC loop when spots arrive mid-run). Reason: scope; the site lattice already yields DRC-clean vias from the drill list.
3. **Planar max-cut layer assignment (13 §1, 45 §5a row "Layer assignment")** — not needed: the layered search assigns layers per connection as the shortest path in the layered graph (SURF step 4, 13 §2) and SMD pads are layer-specific obstacles that the crossing-first model does not represent; max-cut / SURF steepest descent return for > 2 layers in M5. **TopoR double-crossing removal (12 §3.2)** — no crossings exist in the per-layer slot model; its M3 equivalent is the via-reduction pass (Task 10).
4. **Multi-variant search (31 §6.1 "N complete routings with different seeds")** — M3 uses threads for batch searches / self-check / builds; variants (Pareto over via cost, 12 §4) are M5 (`Options.seed` stays unused).
5. **Priority tiers from a plugin table, tier-1 inference, via cost per tier beyond `via_weight`** — M4 (12 §3.1); M3 infers tier 2/3 as M2.
6. **Re-sketch only changed wires** (STATE carry-over) — B2 already re-sketches only the `work` set; the remaining cost is `path_crossings` per changed wire (O(k)); no task. **Zero-slack pass searches** (STATE carry-over) — bounded by R8 (≤ 4 candidates); unchanged in M3; revisit if the eprom gate no longer needs the pass (Open question 4).
7. **Nametag-sao / wedding-gift informational quality** (wedding 20/43 nets, nametag 30 iterations at 44 s) — re-measured in the 0.3.0 benchmark; improvements (Steiner, exit stubs) are M4 items.
8. **B.Cu legs cutting pours** (eprom GND pour on B.Cu): no pour-cut cost in M3; DRC (`unconnected_items`, starved thermals) judges and the spot loop re-routes; a pour-aware layer cost is an M4 item if the benchmarks show islands.

## Self-review notes

- Spec coverage: 45 T3.1 (layer assignment / via minimisation) → Tasks 5, 8, 10; T3.2 (via placement, drill list, CDT with vias) → Tasks 4, 7, 9; T3.3 gate → Task 12; 13 §4 feasible region → `via_fits` (Task 7); 12 §1 pour completion → Tasks 4, 6, 8, 9; 12 §3.1 via cost per tier → `via_weight` (Task 8); 12 §6 steps 4–5 → Tasks 9–10; 30 §5.2 drill list / 0.20 avoided → `via_sizes` (Task 7); 30 §4.1 DRC spots for vias → Task 4; 31 §6.1 threads → Task 11. STATE M3 carry-overs: vias/layers → 6–10; rayon → 11; G1 → deferred 1; RC3 → 2; persistent item index → 1; zero-slack pass searches → deferred 6; re-sketch only changed wires → deferred 6; D1 → 3; D2/D3/D7/D10 → 2; D4/D5/D6/D8/D9 → 3; `record_best` → 3; wedding/nametag → deferred 7.
- Type consistency: `SlotPath { legs, sites }` (Task 8) is what `MultiGraph::insert_path` (Task 7/9), `Negotiator` (Task 9), `MetrState` legs → `Wire`s (Task 9) and the reduction pass (Task 10) consume; `TermRef.layers` (Task 6) feeds `starts`/`goals` (Task 9); `ViaSize` (Task 7) appears in `SlotPath.sites`, `MultiGraph.owner`, `ViaItem` (d/drill); `ItemIndex` (Task 1) is used by `metrize` (Tasks 1, 9) and `check_against` is parallelised in Task 11; `edge_version` (Task 7) is the batch commit key (Task 11); `PourFill`/`in_pour` (Tasks 4, 7) feed `Goal.pour_sites` (Task 8).
- Proportion: interfaces, tests and values only; the layered search rules live in the F note (Task 5), as the M2 search rules did.
- Risk for the eprom "no zero-slack" target: it depends on the +5V link U2.28→J1.2 taking B.Cu (THT pads, no via needed) — the search finds it only if the B.Cu corridor under J1 is free of the 17 GND stitching vias' bands; if not, Task 10 Step 4 escalates with the measurement rather than touching `margin_nm`.
