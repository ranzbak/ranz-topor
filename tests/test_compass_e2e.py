"""M1 compass end to end: the engine routes two user-approved fit fixtures (engine/tests/fixtures/fit/, exported from
harness/fit_patterns.py) and toporlib/compass.py classifies the real result. The expected counts come from the fixture
geometry (derivations in engine/tests/topo_compass.rs), not from the tool:

- channel_overfull: 4 wires through a 1.60 mm slot that needs 2.00 (over-full by 0.40; every other way across is
  over-full by 0.45, so the least violation is in the slot). The cut test predicts the slot: 2 cuts (its two ends),
  1 bottleneck, 4 connections; the per-edge packer holds the same wires on the same segments (no model gap). The
  drawing cannot avoid a violation there, and every drawn violation belongs to a wire that crosses the slot: all drawn
  violations are "planner: over-full", none is a drawing loss, the bottleneck is not drawn clean.
- model_gap_cut: 8 wires across a 3.70 mm opening that needs 3.80 (over-full by 0.10); the per-edge packer has no
  edge holding all 8 (TP1 / TP2 split the opening): with either model the cuts AC and its twin are the model gap
  (with model = "edges" found by the read-only cut test). Every wire crosses AC, so a drawn violation is a plan error
  (over-full, model gap or over-full nearby), never a drawing loss; AC is not drawn clean.

Needs an engine that writes the compass (skipped with an older one, e.g. the venv's main wheel)."""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from toporlib import compass

FIX = Path(__file__).resolve().parents[1] / "engine" / "tests" / "fixtures" / "fit"
PLAN_ERRORS = {"planner_overfull", "model_gap", "overfull_nearby"}


def _route(name: str, model: str) -> dict:
    eng = pytest.importorskip("topor_engine")
    doc = json.loads((FIX / f"{name}.json").read_text())
    opts = {"runs": 1, "wall_clock_s": 120.0, "topo_rounds": 1, "capacity_model": model}
    r = json.loads(eng.route(json.dumps(doc["model"]), json.dumps(opts)))
    if r.get("compass") is None:
        pytest.skip("this engine writes no compass")
    return r


def _row(cp, cls):
    return next(r for r in cp["rows"] if r["class"] == cls)


def test_channel_overfull_drawn_violations_are_the_predicted_bottleneck():
    r = _route("channel_overfull", "cuts")
    cp = compass.build(r, None, None)
    o = _row(cp, "planner_overfull")
    assert (o["predicted"], o["bottlenecks"], o["predicted_conns"]) == (2, 1, 4), o
    assert _row(cp, "model_gap")["predicted"] == 0
    assert cp["drawn_violations"] >= 1, "over-full by design: the drawing must violate somewhere"
    assert o["drawn"] == cp["drawn_violations"], [(v["class"], v["sub"], v["at_mm"]) for v in cp["violations"]]
    assert _row(cp, "drawing_loss")["drawn"] == 0
    assert o["predicted_clean"] == 0


@pytest.mark.parametrize("model", ["cuts", "edges"])
def test_model_gap_cut_drawn_violations_are_plan_errors_at_the_opening(model):
    r = _route("model_gap_cut", model)
    cp = compass.build(r, None, None)
    g = _row(cp, "model_gap")
    assert (g["predicted"], g["bottlenecks"], g["predicted_conns"]) == (2, 1, 8), g
    assert cp["drawn_violations"] >= 1, "over-full by design: the drawing must violate somewhere"
    by = {v["class"] for v in cp["violations"]}
    assert by <= PLAN_ERRORS, [(v["class"], v["sub"], v["at_mm"]) for v in cp["violations"]]
    assert _row(cp, "drawing_loss")["drawn"] == 0
    assert g["predicted_clean"] == 0
