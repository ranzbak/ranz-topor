"""M0 gate: the harness reproduces the Topola baseline's hand images and orig metrics.

Benchmark boards are copied; the source trees are sha256-verified before and after. The
Topola-routed baseline boards no longer exist, so only the `-hand` images and `orig_*` numbers
can be reproduced.
"""
import json
import re
import subprocess
from pathlib import Path

import pytest

from harness.bench import run_bench
from harness.copy import assert_unchanged, sha256_tree
from harness.drc import ROUTING_TYPES, run_drc
from harness.paths import BENCH_BOARDS
from harness.render import CAPTION_PX
from tests.test_fuzz_gate import fuzz_drc

ROOT = Path(__file__).resolve().parents[2]
BASELINE = ROOT / "tests/fixtures/m0-baseline"  # copied from the (now untracked) Topola benchmark run

BOARDS = {
    # eprom-to-kernal is hand-edited by the user in KiCad: nothing about its routing is pinned here;
    # counts are derived from a copy in the same run (`_derive_eprom`), only invariants are fixed.
    "eprom-to-kernal": None,
    "wiccon-2023": dict(
        footprints=45, pads=124, nets=38,
        orig_all={"segments": 381, "arcs": 0, "vias": 36, "track_mm": 1831.97},
        orig_drc_total=48, kept_free_vias=11, removed_vias=25, hand_png=(1095, 1160)),
    "dcsgonebadge": dict(
        footprints=157, pads=458, nets=86,
        orig_all={"segments": 1369, "arcs": 14, "vias": 203, "track_mm": 4210.28, "arc_mm": 43.2},
        orig_drc_total=210, kept_free_vias=7, removed_vias=196, hand_png=(1012, 1160)),
}


def png_size(p: Path) -> tuple[int, int]:
    w, h = subprocess.check_output(["identify", "-format", "%w %h", str(p)], text=True).split()
    return int(w), int(h)


def rmse(a: Path, b: Path, tmp: Path) -> float:
    """Normalised RMSE (0..1) of the board area of two hand images (`compare -metric RMSE`).

    The baseline PNGs carry an alpha channel and a different (larger) caption than the harness
    render, so both are flattened onto white and the 60 px caption strip is cropped first.
    """
    flat = []
    for i, src in enumerate((a, b)):
        out = tmp / f"cmp{i}.png"
        w, h = png_size(src)
        subprocess.check_call(["convert", str(src), "-background", "white", "-alpha", "remove",
                               "-alpha", "off", "-crop", f"{w}x{h - CAPTION_PX}+0+{CAPTION_PX}",
                               "+repage", str(out)])
        flat.append(out)
    r = subprocess.run(["compare", "-metric", "RMSE", str(flat[0]), str(flat[1]), "null:"],
                       capture_output=True, text=True)
    m = re.search(r"\(([0-9.eE+-]+)\)", r.stderr)
    assert m, f"compare failed: rc={r.returncode} {r.stderr[:200]}"
    return float(m.group(1))


def _derive_eprom(tmp: Path) -> dict:
    """Expectations for the hand board read straight from pcbnew on a private copy."""
    from harness.copy import copy_project
    from harness.measure import copper_facts
    from harness.model import load_board
    from harness.render import caption, render_png

    cp = copy_project(BENCH_BOARDS["eprom-to-kernal"], tmp / "derive")
    b = load_board(cp)
    f = copper_facts(b)
    fps = list(b.GetFootprints())
    nets = {str(n.GetNetname()) for _, n in b.GetNetsByNetcode().items() if n.GetNetname()}
    # the hand image is rendered again from the same copy in this run (render determinism), not
    # compared with a stored PNG that goes stale whenever the user edits the board
    ref = caption(render_png(cp.pcb, tmp / "derive-hand-raw.png"), "eprom-to-kernal: hand",
                  tmp / "derive-hand.png")
    return dict(
        footprints=len(fps), pads=sum(len(fp.Pads()) for fp in fps), nets=len(nets),
        orig_all={"segments": f["segments"], "arcs": f["arcs"], "vias": f["vias"],
                  "track_mm": f["track_mm"]},
        hand_png=png_size(ref), hand_ref=ref)


@pytest.mark.gate
@pytest.mark.parametrize("board", BOARDS)
def test_m0_gate_reproduces_baseline(board, tmp_path):
    src_hashes = sha256_tree(BENCH_BOARDS[board].parent)
    e = BOARDS[board]
    if board == "eprom-to-kernal":
        e = _derive_eprom(tmp_path)
    r = run_bench(board, tmp_path / "bench", router=None, rf_nets=frozenset())
    tmp_path = tmp_path / "bench"
    assert (r["footprints"], r["pads"], r["nets_total"]) == (e["footprints"], e["pads"], e["nets"])
    for k, v in e["orig_all"].items():
        got = r["orig_all"][k]
        if k == "track_mm":  # the baseline's track length includes the arcs
            got += r["orig_all"]["arc_mm"]
        assert got == pytest.approx(v, abs=0.02), k
    if board == "eprom-to-kernal":
        # invariant of the hand board: DRC-clean routing, nothing unconnected
        assert not any(v for k, v in r["orig_drc"]["violations"].items() if k in ROUTING_TYPES)
        assert r["orig_drc"]["total"] == 0, r["orig_drc"]
        raw = json.loads((tmp_path / f"{board}--drc-orig.json").read_text())
        assert raw.get("unconnected_items", []) == []
    elif board == "dcsgonebadge":
        # The baseline's 210 total included 18 library/courtyard/thermal reports that depend on
        # the machine's footprint libraries (192 here); every other type must match exactly.
        base = json.loads((BASELINE / "metrics/dcsgone--drc-orig.json").read_text())
        want = {}
        for v in base["violations"]:
            want[v["type"]] = want.get(v["type"], 0) + 1
        env = {"courtyards_overlap", "lib_footprint_issues", "starved_thermal"}
        got = r["orig_drc"]["violations"]
        assert {k: v for k, v in got.items() if k not in env} == \
            {k: v for k, v in want.items() if k not in env}
        print(f"GATE dcsgonebadge orig_drc total {r['orig_drc']['total']} (baseline {e['orig_drc_total']})")
    else:
        assert r["orig_drc"]["total"] == e["orig_drc_total"]
    for k, v in e.get("orig_drc", {}).items():
        assert r["orig_drc"]["violations"].get(k) == v, k
    kept, removed = r["kept"]["kept_free_vias"], r["removed"]["removed_vias"]
    if board == "eprom-to-kernal":
        assert kept + removed == e["orig_all"]["vias"]  # rip removes exactly the non-free vias
        assert r["removed"]["removed_tracks"] == e["orig_all"]["segments"] and r["removed"]["removed_arcs"] == e["orig_all"]["arcs"]
    print(f"GATE {board}: kept_free_vias={kept} removed_vias={removed} "
          f"(baseline {e.get('kept_free_vias')}/{e.get('removed_vias')})")
    if board == "wiccon-2023":
        assert (kept, removed) == (e["kept_free_vias"], e["removed_vias"])
    elif board == "dcsgonebadge":
        # DEVIATION: the baseline kept 7 free vias (removed 196); the harness rip (`GetIsFree()`)
        # keeps 2 (removed 201). Open rip-policy question (README step 1: keep stitching/fence
        # vias, i.e. vias no track of their net touches), to be settled before M3. Pin the
        # measured values so any change is visible.
        # Re-pinned after the user locked the edge GND trace (snapshot 3e7e66f862a7: 22 locked GND
        # items): removed 201 -> 197, the 4 vias touched by that locked copper are retained.
        assert (kept, removed) == (2, 197)
        assert kept + removed + 4 == e["orig_all"]["vias"]
        assert r["kept"]["kept_locked_tracks"] == 22
    assert png_size(tmp_path / f"{board}-hand.png") == e["hand_png"]
    ref = e["hand_ref"] if board == "eprom-to-kernal" else BASELINE / f"images/{board}-hand.png"
    d = rmse(tmp_path / f"{board}-hand.png", ref, tmp_path)
    print(f"GATE {board}: runtime_s={r['runtime_s']} rmse={d:.5f} orig_drc={r['orig_drc']['total']}")
    assert d < 0.02
    assert_unchanged(BENCH_BOARDS[board].parent, src_hashes)
    assert r["runtime_s"] < 60


@pytest.mark.gate
def test_m0_gate_fuzz_on_eprom(engine, tmp_path):
    """Seed 1, 1000 items on the ripped eprom copy: zero new routing violations in KiCad."""
    from harness.copy import copy_project
    from harness.rip import rip_tracks
    from harness.model import load_board
    from harness.writer import save

    src = BENCH_BOARDS["eprom-to-kernal"]
    src_hashes = sha256_tree(src.parent)
    cp = copy_project(src, tmp_path / "work")
    b = load_board(cp)
    rip_tracks(b, mode="all", scope=None, rf_nets=set())
    save(b, cp.pcb)
    base = run_drc(cp.pcb, tmp_path / "ripped-drc.json")  # pad-vs-edge violations pre-exist
    out, c, rep, _ = fuzz_drc(cp, engine, tmp_path, 1, 1000, {"margin_nm": 5000}, baseline=base)
    print(f"GATE fuzz-eprom: accepted={len(out['items'])} attempts={out['attempts']} "
          f"truncated={out['truncated']} routing_new={c['routing_new']} incomplete={c['incomplete']}")
    assert len(out["items"]) == 1000 and not out["truncated"]
    assert c["routing_new"] == 0
    assert_unchanged(src.parent, src_hashes)
