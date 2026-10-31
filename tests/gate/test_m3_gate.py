"""M3 gate: topor's two-layer router (layers + vias) re-routes the ripped eprom-to-kernal and wiccon-2023.

Thresholds are the plan's (docs/superpowers/plans/2026-10-31-topor-m3-layers-vias.md Task 12).
M3-R4: zero_slack_wires is printed, not asserted 0; a kept zero-slack wire must be DRC-accepted.
M3-R5: wiccon's >= 90 % uses `connections_total`, which includes pour-completion connections; the
signal-only ratio (to.kind != "pour") is printed as well.
"""
import json

import pytest

from harness.bench import run_bench
from harness.copy import assert_unchanged, sha256_tree
from harness.paths import BENCH_BOARDS
from harness.router import route_router
from tests.gate.test_m0_gate import png_size
from tests.gate.test_m1_gate import measured_hand_facts


@pytest.mark.gate
def test_m3_gate_eprom_two_layers(engine, tmp_path):
    src = BENCH_BOARDS["eprom-to-kernal"]
    hashes = sha256_tree(src.parent)
    facts = measured_hand_facts(tmp_path)
    r = run_bench("eprom-to-kernal", tmp_path / "bench", router=route_router, rip="all", threads=4)
    rep, rt, drc, hs = r["router_report"], r["route"], r["routed_drc"], r["hand_stats"]
    assert rep["nets_total"] == rep["nets_routed"] == 25
    assert next(n for n in rep["nets"] if n["name"] == "GND")["status"] == "pour_served"
    assert rep["connections_total"] == rep["routed"] == facts["wires"] and rep["partial"] == rep["failed"] == 0
    assert drc["routing_new"] == 0 and drc["incomplete"]["unconnected_items"] == 0, drc
    assert r["width_match"]["compared"] == r["width_match"]["matched"] == 25
    ra = r["routed_all"]
    assert ra["arcs"] >= 1 and all("SS" not in c["pattern"] for c in rep["connections"])
    assert rep["length_outside_pads_nm"] <= hs["hand_outside_pads_nm"]
    assert r["router_s"] < 2.0 and rep["threads_used"] == 4 and not rep["timed_out"] and rep["iterations"] <= 50
    # target: zero_slack_wires == 0 (printed; Open question 4). A kept zero-slack wire must be DRC-accepted.
    assert not rep["zero_slack_wires"] or r["zero_slack"]["status"] == "kept"
    model_json = (tmp_path / "bench" / "eprom-to-kernal--model.json").read_text()
    runs = {t: json.loads(engine.route(model_json, json.dumps({"threads": t}))) for t in (1, 2, 4)}
    runs["again"] = json.loads(engine.route(model_json, json.dumps({"threads": 4})))
    for x in runs.values():
        x.pop("runtime_s")
        x.pop("threads_used")
    assert len({json.dumps(x, sort_keys=True) for x in runs.values()}) == 1
    assert png_size(tmp_path / "bench" / "eprom-to-kernal-compare.png")[0] > 0
    assert_unchanged(src.parent, hashes)
    print(f"GATE M3 eprom: routed {rep['routed']}/{rep['connections_total']} nets {rep['nets_routed']}/{rep['nets_total']} vias {rep['vias']} "
          f"layers {rep['length_by_layer_nm']} outside-pads {rep['length_outside_pads_nm']/1e6:.3f} (hand {hs['hand_outside_pads_nm']/1e6:.3f}) mm "
          f"drc_new {drc['routing_new']} unconnected {drc['incomplete']['unconnected_items']} arcs {ra['arcs']} router_s {r['router_s']} "
          f"iters {rep['iterations']} zero_slack_wires {len(rep['zero_slack_wires'])} via_reduction '{rep['via_reduction']}'")


@pytest.mark.gate
def test_m3_gate_wiccon(engine, tmp_path):
    src = BENCH_BOARDS["wiccon-2023"]
    hashes = sha256_tree(src.parent)
    r = run_bench("wiccon-2023", tmp_path, router=route_router, rip="all", threads=4, options={"partial_routes": False})
    rep, rt, drc = r["router_report"], r["route"], r["routed_drc"]
    sig = [c for c in rep["connections"] if c["to"]["kind"] != "pour"]
    sig_routed = sum(1 for c in sig if c["status"] == "routed")
    print(f"GATE M3 wiccon: routed {rep['routed']}/{rep['connections_total']} (all, incl. pour completion) "
          f"signal-only {sig_routed}/{len(sig)} vias {rep['vias']} drc_new {drc['routing_new']} "
          f"unconnected {drc['incomplete']['unconnected_items']} router_s {r['router_s']} iters {rep['iterations']} "
          f"zero_slack_wires {len(rep['zero_slack_wires'])}")
    assert rep["routed"] >= 0.9 * rep["connections_total"], (rep["routed"], rep["connections_total"])
    assert drc["routing_new"] == 0 and r["router_s"] < 10.0 and not rep["timed_out"]
    model_json = (tmp_path / "wiccon-2023--model.json").read_text()
    runs = [json.loads(engine.route(model_json, json.dumps({"threads": t}))) for t in (1, 4)]
    for x in runs:
        x.pop("runtime_s")
        x.pop("threads_used")
    assert json.dumps(runs[0], sort_keys=True) == json.dumps(runs[1], sort_keys=True)
    assert_unchanged(src.parent, hashes)


@pytest.mark.gate
@pytest.mark.parametrize("board", ["dcsgonebadge"])
def test_m3_router_other_boards_complete(board, tmp_path):
    """Informational: must complete, no new routing DRC, anytime wall clock respected."""
    src = BENCH_BOARDS[board]
    hashes = sha256_tree(src.parent)
    r = run_bench(board, tmp_path, router=route_router, rip="all", threads=4,
                  options={"wall_clock_s": 60, "partial_routes": False})
    rt = r["route"]
    print(f"GATE-INFO M3 {board}: routed {rt['routed']}/{rt['connections']} failed {rt['failed']} "
          f"drc_new={r['routed_drc']['routing_new']} router_s={r['router_s']}")
    assert r["routed_drc"]["routing_new"] == 0
    assert r["router_s"] <= 65.0  # anytime: wall_clock_s 60 plus the final metrization/self-check pass
    assert_unchanged(src.parent, hashes)
