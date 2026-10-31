### Task 4 (S): harness and model preparation — pour fills, 2-layer fixture, via spots, M3 progress, layer colours

**Files:**
- Modify: `harness/model.py` (`_export_pours` :444), `harness/fixtures.py`, `harness/router.py` (`drc_spots`, `_layer_of`), `harness/bench.py` (`_route_summary` :55, summary print :131), `harness/progress.py`, `engine/src/model.rs`
- Test: `tests/test_router.py`, `tests/test_model_export.py`, `engine/tests/model_json.rs`

**Interfaces:**
- Produces (engine `model.rs`):

```rust
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)] #[serde(default)]
pub struct PourFill { pub layer: u32, pub outer: Vec<Pt> }        // KiCad frame on the wire, Y flipped by from_json like copper_shapes
// Pour: + pub fills: Vec<PourFill>
// Options: + pub via_cost_nm: i64 (default 2_000_000), pub max_vias_per_connection: u32 (default 4)
// ViaItem: + #[serde(default, skip_serializing_if = "Option::is_none")] pub wire: Option<usize>
```

- Produces (harness): `model["pours"][i]["fills"] = [{"layer": id, "outer": [[x, y], ...]}, ...]` — for every layer of the zone, every outline `k` of `z.GetFilledPolysList(layer)` (SWIG: `SHAPE_POLY_SET.OutlineCount()`, `.Outline(k)`, `SHAPE_LINE_CHAIN.PointCount()/CPoint(i)` — UNVERIFIED names; fall back to `z.Outline()` if the filled list is unavailable and record a warning in the model) as outer rings only (holes dropped, as M0 copper shapes), points nm; `make_fixture_board(..., b_pads=False, pour_gap_pad=False)`: `b_pads=True` makes SIG pads `"2"`, `"3"` SMD on **B.Cu** (layer set B.Cu only) so SIG needs a via between pad 1 (F.Cu SMD) and pad 2, and pads 2–3 route on B.Cu; `pour_gap_pad=True` adds a GND SMD pad `"9"` on F.Cu at `(10, 30)` that the B.Cu GND pour cannot reach (pour completion fixture). `drc_spots`: a via item named by a violation yields one spot per copper layer of the board (through via) with `r = clearance_nm + d`. `_route_summary` adds `vias`, `length_by_layer_mm: {name: mm}`, `threads_used`; summary line `  route: ... vias V, F.Cu X / B.Cu Y mm`. `progress.render_progress(task, router="topor", board=...)` → `_render_progress_m3`: file `<date>-m3-<task>.png`, caption line 1 `"{task}: {routed}/{connections} connections, {nets_routed}/{nets} nets, partial {p}, failed {f}, vias {v}, arcs {a}, zero slack {z}, iters {i}"`, line 2 `"DRC routing {n}, unconnected {u}, outside pads {x:.1f} vs hand {h:.1f} mm, F.Cu {f:.1f} / B.Cu {b:.1f} mm, router {s:.2f} s on {t} threads"`; `main` gains `--threads` (default 4, passed to `run_bench`). Rendering: `render_png` already renders `F.Cu,B.Cu,Edge.Cuts` with KiCad's theme colours (F.Cu red, B.Cu blue); the test below pins that B.Cu copper is visibly distinct.

- [ ] **Step 1: Write the failing tests**

```python
def test_export_pours_have_fill_outlines(fixture_board):      # pours[0]["fills"][0]["layer"] == B_Cu, >= 4 points, bbox inside the outline
def test_fixture_b_pads_puts_sig_pads_on_b_cu(tmp_path):     # pads 2,3 layers == [B_Cu]; model pads reflect it
def test_fixture_pour_gap_pad_is_not_reached(tmp_path):      # pad "9" of GND not in pours[0]["pads"]
def test_drc_spots_via_item_spots_every_layer():             # fake violation naming a via uuid -> 2 spots (layers 0 and 31), r == clearance + d
def test_route_summary_has_vias_and_layer_lengths():         # fake report with 2 vias, items on both layers -> vias 2, length_by_layer_mm keys F.Cu/B.Cu
def test_progress_m3_caption_and_path(engine, fixture_board, tmp_path):  # name ends "-m3-T4.png"; caption line 1 contains "vias "; line 2 contains "B.Cu"
def test_render_b_cu_copper_is_distinct(tmp_path):           # board with one B.Cu track vs same with one F.Cu track vs none: pairwise image RMSE > 0.002 (harness.render compare helper of test_render_metrics)
```

`engine/tests/model_json.rs::pour_fills_and_via_options_round_trip` — JSON with `fills` parses, Y flipped; `Options::from_json("{}")` gives `via_cost_nm 2_000_000`, `max_vias_per_connection 4`.

- [ ] **Step 2: Run** `.venv/bin/pytest tests/test_router.py tests/test_model_export.py -q` and `cargo test --test model_json` — Expected: FAIL.
- [ ] **Step 3: Implement** engine model types, `_export_pours` fills, fixture params, `drc_spots`, summary, progress M3, CLI `--threads`.
- [ ] **Step 4: Run** the suites again + `tests/test_render_metrics.py` + clippy — Expected: PASS (rebuild the wheel first).
- [ ] **Step 5: Commit:** `git commit -m "feat(harness): pour fill outlines, 2-layer fixture, via DRC spots, M3 progress caption and layer colours"`.

