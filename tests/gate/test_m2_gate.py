"""M2 gate: topor's single-layer router re-routes the ripped eprom-to-kernal board.

Criteria and thresholds are the plan's (docs/superpowers/plans/2026-10-31-topor-m2-router.md, Global
Constraints "M2 gate numbers"). Nothing about the (user-edited) board is pinned: hand facts are measured
on a private copy in the same run; source trees are sha256-verified before and after.
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
def test_m2_gate_eprom_router(engine, tmp_path):
    src = BENCH_BOARDS["eprom-to-kernal"]
    hashes = sha256_tree(src.parent)
    facts = measured_hand_facts(tmp_path)
    out = tmp_path / "bench"
    r = run_bench("eprom-to-kernal", out, router=route_router, rip="all")
    rep, rt, drc, hs = r["router_report"], r["route"], r["routed_drc"], r["hand_stats"]
    zs = rep["zero_slack_wires"]

    assert r["removed"]["removed_tracks"] == facts["segments"] and r["kept"]["kept_free_vias"] == 17
    assert rep["nets_total"] == rep["nets_routed"] == 25
    assert next(n for n in rep["nets"] if n["name"] == "GND")["status"] == "pour_served"
    assert rep["connections_total"] == rep["routed"] == facts["wires"]
    assert rep["partial"] == rep["failed"] == 0
    assert drc["routing_new"] == 0 and drc["incomplete"]["unconnected_items"] == 0, drc
    assert r["width_match"]["compared"] == r["width_match"]["matched"] == 25, r["width_match"]
    ra = r["routed_all"]
    assert ra["arcs"] >= 1 and all("SS" not in c["pattern"] for c in rep["connections"])
    # length gate: outside pad copper, both sides measured in this run
    assert rep["length_outside_pads_nm"] <= hs["hand_outside_pads_nm"], \
        (rep["length_outside_pads_nm"], hs["hand_outside_pads_nm"])
    assert r["router_s"] < 2.0 and rep["threads_used"] == r["threads"] and not rep["timed_out"]
    assert rep["iterations"] <= 50
    # R6/R8: a zero-slack wire is only kept when kicad-cli DRC accepted it (routing_new == 0 above)
    assert not zs or r["zero_slack"]["status"] == "kept", (zs, r["zero_slack"])
    assert len(zs) == len(rt["zero_slack_wires"])

    model_json = (out / "eprom-to-kernal--model.json").read_text()
    # byte-identical output; only the wall-clock field `runtime_s` of the report differs between runs
    runs = [json.loads(engine.route(model_json, "{}")) for _ in range(2)]
    for x in runs:
        x.pop("runtime_s")
    assert json.dumps(runs[0], sort_keys=True) == json.dumps(runs[1], sort_keys=True)
    assert png_size(out / "eprom-to-kernal-compare.png")[0] > 0
    assert_unchanged(src.parent, hashes)

    mm = 1e6
    cl = rep["clinches"]
    zs_names = ", ".join(sorted({w["name"] for w in zs}))
    print(f"GATE M2 eprom: routed {rep['routed']}/{rep['connections_total']} nets {rep['nets_routed']}/{rep['nets_total']} "
          f"outside-pads {rep['length_outside_pads_nm'] / mm:.3f} (hand {hs['hand_outside_pads_nm'] / mm:.3f}) mm "
          f"KiCad {ra['track_mm'] + ra['arc_mm']:.3f} (hand {facts['track_mm']}) info "
          f"drc_new {drc['routing_new']} unconnected {drc['incomplete']['unconnected_items']} arcs {ra['arcs']} "
          f"router_s {r['router_s']} iters {rep['iterations']} "
          f"clinches regular {len(cl['regular'])} u {len(cl['u'])} "
          f"zero_slack_wires {len(zs)} [{zs_names}]")


@pytest.mark.gate
@pytest.mark.parametrize("board", ["wiccon-2023", "dcsgonebadge"])
def test_m2_router_other_boards_complete(board, tmp_path):
    """Informational: must complete, no new routing DRC, anytime wall clock respected."""
    src = BENCH_BOARDS[board]
    hashes = sha256_tree(src.parent)
    r = run_bench(board, tmp_path, router=route_router, rip="all",
                  options={"wall_clock_s": 60, "partial_routes": False})
    rt = r["route"]
    print(f"GATE-INFO {board}: routed {rt['routed']}/{rt['connections']} failed {rt['failed']} "
          f"drc_new={r['routed_drc']['routing_new']} router_s={r['router_s']}")
    assert r["routed_drc"]["routing_new"] == 0
    if board == "dcsgonebadge":  # M3-R13 (user): dcsgonebadge info only, robustness board
        print(f"GATE-INFO {board} router_s={r['router_s']} (<=65: {r['router_s'] <= 65.0})")
    else:
        assert r["router_s"] <= 65.0  # anytime: wall_clock_s 60 plus the final metrization/self-check pass
    assert_unchanged(src.parent, hashes)
