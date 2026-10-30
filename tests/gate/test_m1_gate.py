"""M1 gate: the taut smoother converts the hand-routed eprom-to-kernal board.

User rulings 2026-10-30: the length gate is the length outside pad copper (engine report), every
wire is taut or a zero-slack fallback, nothing about the (user-edited) board is pinned: facts are
measured on a private copy in the same run. Source trees are sha256-verified before and after.
"""
import json
import re

import pytest

from harness.bench import run_bench
from harness.copy import assert_unchanged, copy_project, sha256_tree
from harness.measure import copper_facts, wire_count
from harness.model import export_model, load_board, write_model_json
from harness.paths import BENCH_BOARDS
from harness.rip import record_widths
from harness.rules import resolve_rules
from harness.smoother import smooth_router
from tests.gate.test_m0_gate import png_size


def measured_hand_facts(tmp):
    """Hand copper facts and engine wire count read from a private copy of the source board."""
    cp = copy_project(BENCH_BOARDS["eprom-to-kernal"], tmp / "facts")
    b = load_board(cp)
    f = copper_facts(b)
    rules = resolve_rules(b, dru_text=cp.dru.read_text() if cp.dru else None,
                          widths_before=record_widths(b))
    mj = write_model_json(export_model(b, rules, name="eprom-to-kernal"), tmp / "facts-model.json")
    m = re.match(r"(\d+)", wire_count(mj))
    assert m, "engine wire count unavailable"
    return {**f, "wires": int(m.group(1))}


@pytest.mark.gate
def test_m1_gate_eprom_taut_smoother(engine, tmp_path):
    src = BENCH_BOARDS["eprom-to-kernal"]
    hashes = sha256_tree(src.parent)
    facts = measured_hand_facts(tmp_path)
    out = tmp_path / "bench"
    r = run_bench("eprom-to-kernal", out, router=smooth_router, rip="none",
                  options={"existing_copper": "smooth"})
    s, rep = r["smooth"], r["router_report"]
    wires, nets = rep["wires"], rep["nets"]

    # every wire taut, or a zero-slack fallback; no other fallback
    assert s["wires"] == len(wires) == facts["wires"]
    zs = [w for w in wires if w["status"] == "fallback" and w["zero_slack"] is True]
    bad = [w for w in wires if w["status"] != "taut" and w not in zs]
    assert not bad, bad
    assert s["wires_taut"] + len(zs) == s["wires"] and s["wires_zero_slack"] == len(zs)
    # every net with a hand wire: taut, or partial/fallback only through zero-slack wires
    for n in nets:
        if n["wires"] == 0:
            continue
        assert n["status"] == "taut" or (
            n["status"] in ("partial", "fallback") and n["zero_slack"] >= 1
            and n["taut"] + n["zero_slack"] == n["wires"]), n

    assert r["routed_drc"]["routing_new"] == 0, r["routed_drc"]
    assert r["routed_drc"]["incomplete"]["unconnected_items"] == 0, r["routed_drc"]
    # length gate: outside pad copper (fallback wires count with hand length on both sides)
    assert rep["length_outside_pads_nm"] <= rep["hand_outside_pads_nm"], \
        (rep["length_outside_pads_nm"], rep["hand_outside_pads_nm"])
    ra = r["routed_all"]
    kicad_mm = ra["track_mm"] + ra["arc_mm"]  # information only
    assert ra["arcs"] >= 1 and all("SS" not in w["pattern"] for w in wires)
    assert r["width_match"]["compared"] == r["width_match"]["matched"] > 0
    # replaced = hand segments + arcs minus the hand items kept by zero-slack wires
    model_json = (out / "eprom-to-kernal--model.json").read_text()
    model = json.loads(model_json)
    assert len(model["kept"]) == facts["segments"] + facts["arcs"]
    unreplaced = [k for k in model["kept"] if k["uuid"] not in set(rep["replaced"])]
    assert bool(unreplaced) == bool(zs)
    assert r["replaced"] == facts["segments"] + facts["arcs"] - len(unreplaced) == len(rep["replaced"])
    assert r["removed"]["removed_tracks"] == 0
    assert r["router_s"] < 2.0 and rep["threads_used"] == 1 and not rep["timed_out"]
    o = json.dumps({"existing_copper": "smooth"})
    # byte-identical output; only the wall-clock field `runtime_s` of the report differs between runs
    runs = [json.loads(engine.smooth(model_json, o)) for _ in range(2)]
    for x in runs:
        x.pop("runtime_s")
    assert json.dumps(runs[0], sort_keys=True) == json.dumps(runs[1], sort_keys=True)
    assert png_size(out / "eprom-to-kernal-compare.png")[0] > 0
    assert_unchanged(src.parent, hashes)

    for w in zs:
        print(f"fallback: zero slack wire {w['id']} net {w['net']} "
              f"hand_slack_nm {w['hand_slack_nm']} reason {w['reason']}")
    mm = 1e6
    print(f"GATE M1 eprom: nets taut {s['nets_taut']}/{s['nets']} wires taut {s['wires_taut']}/{s['wires']} "
          f"zero-slack {len(zs)} outside-pads {rep['length_outside_pads_nm'] / mm:.3f} "
          f"(hand {rep['hand_outside_pads_nm'] / mm:.3f}) mm; KiCad track+arc {kicad_mm:.3f} "
          f"(hand {facts['track_mm']}) info; drc_new {r['routed_drc']['routing_new']} "
          f"unconnected {r['routed_drc']['incomplete']['unconnected_items']} arcs {ra['arcs']} "
          f"replaced {r['replaced']} router_s {r['router_s']} iters {rep['iterations']}")


@pytest.mark.gate
@pytest.mark.parametrize("board", ["wiccon-2023", "dcsgonebadge"])
def test_m1_smoother_other_boards_complete(board, tmp_path):
    """Informational (not an M1 criterion): the smoother must complete; fallback keeps the board DRC-equal."""
    src = BENCH_BOARDS[board]
    hashes = sha256_tree(src.parent)
    r = run_bench(board, tmp_path, router=smooth_router, rip="none",
                  options={"existing_copper": "smooth", "wall_clock_s": 30})
    s = r["smooth"]
    print(f"GATE-INFO {board}: taut {s['nets_taut']}/{s['nets']} nets {s['wires_taut']}/{s['wires']} wires "
          f"zero-slack {s['wires_zero_slack']} fallback {len(s['fallback_reasons'])} "
          f"drc_new={r['routed_drc']['routing_new']} router_s={r['router_s']}")
    assert r["routed_drc"]["routing_new"] == 0
    assert_unchanged(src.parent, hashes)
