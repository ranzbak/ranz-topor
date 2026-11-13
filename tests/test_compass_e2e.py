"""M1 compass end to end: the engine routes two user-approved fit fixtures (engine/tests/fixtures/fit/, exported from
harness/fit_patterns.py) and toporlib/compass.py classifies the real result. The expected counts come from the fixture
geometry (derivations in engine/tests/topo_compass.rs), not from the tool:

- channel_overfull: 4 wires through a 1.60 mm slot that needs 2.00 (over-full by 0.40; every other way across is
  over-full by 0.45, so the least violation is in the slot). The cut test predicts the slot: 2 cuts (its two ends),
  1 bottleneck, 4 connections; the per-edge packer holds the same wires on the same segments (no model gap). The
  deficit lies in the slot, so the drawing violates there and every violation within reach of the slot is the plan's
  (planner: over-full), never a drawing loss; the slot is not drawn clean.
- model_gap_cut: 8 wires across a 3.70 mm opening that needs 3.80 (over-full by 0.10); the per-edge packer has no
  edge holding all 8 (TP1 / TP2 split the opening): with either model the cuts AC and its twin are the model gap
  (with model = "edges" found by the read-only cut test), 2 cuts in 1 bottleneck, 8 connections. Violations within
  reach of the opening are plan errors (over-full, model gap or over-full nearby), never a drawing loss.

Violations away from the bottleneck are not fixed by the geometry and are not asserted: in the first runs
(2026-11-13, topo_rounds 1) the drawing produced pad-exit near-misses at the J terminals (gap 0.198 < 0.200 mm,
"pad exit, not predicted"), drew R4 through the U2 pad row although the plan sent it through the opening (short with
U2.16) and crossed L2 / L3 below the rows: drawing losses the compass shows as such.

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
    opts = {"runs": 1, "wall_clock_s": 120.0, "topo_rounds": 1, "capacity_model": model, "compass_cut_check": True}
    r = json.loads(eng.route(json.dumps(doc["model"]), json.dumps(opts)))
    if r.get("compass") is None:
        pytest.skip("this engine writes no compass")
    return r


def _row(cp, cls):
    return next(r for r in cp["rows"] if r["class"] == cls)


def _at_box(v, box) -> bool:
    """The violation's stretch lies within its reach (max(0.5, 4 x 0.25 mm) + half its length) of the box."""
    (x0, y0), (x1, y1) = box
    x, y = v["at_mm"]
    dx, dy = max(x0 - x, 0.0, x - x1), max(y0 - y, 0.0, y - y1)
    return (dx * dx + dy * dy) ** 0.5 <= 1.0 + v["half_len_mm"]


def _at(cp, box):
    return [v for v in cp["violations"] if _at_box(v, box)]


def test_channel_overfull_drawn_violations_at_the_slot_are_the_predicted_bottleneck():
    r = _route("channel_overfull", "cuts")
    cp = compass.build(r, None, None)
    o = _row(cp, "planner_overfull")
    assert (o["predicted"], o["bottlenecks"], o["predicted_conns"]) == (2, 1, 4), o
    assert _row(cp, "model_gap")["predicted"] == 0
    slot = _at(cp, ((4.3, 5.6), (5.9, 6.4)))
    assert slot, "over-full by design: the drawing must violate in the slot"
    assert {v["class"] for v in slot} == {"planner_overfull"}, [(v["class"], v["sub"], v["at_mm"]) for v in slot]
    assert o["predicted_clean"] == 0


@pytest.mark.parametrize("model", ["cuts", "edges"])
def test_model_gap_cut_drawn_violations_at_the_opening_are_plan_errors(model):
    r = _route("model_gap_cut", model)
    cp = compass.build(r, None, None)
    g = _row(cp, "model_gap")
    assert (g["predicted"], g["bottlenecks"], g["predicted_conns"]) == (2, 1, 8), g
    opening = _at(cp, ((5.3, 7.6), (9.0, 8.4)))
    assert opening, "over-full by design: the drawing must violate in the opening"
    assert {v["class"] for v in opening} <= PLAN_ERRORS, [(v["class"], v["sub"], v["at_mm"]) for v in opening]
    assert g["predicted_clean"] == 0
