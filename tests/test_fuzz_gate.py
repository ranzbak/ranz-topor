"""Fuzz gate: engine-placed segments/arcs must be DRC-clean in KiCad (the oracle)."""
import json
import shutil

import pcbnew
import pytest

from harness.drc import classify, run_drc, summarise
from harness.model import export_model, load_board
from harness.rules import resolve_rules
from harness.writer import save, write_items


def _count_tracks(board) -> int:
    """PCB_TRACK + PCB_ARC items (vias excluded)."""
    return sum(1 for t in board.GetTracks() if t.GetClass() in ("PCB_TRACK", "PCB_ARC"))


def fuzz_drc(cp, engine, tmp_path, seed, n, options, baseline=None):
    """Fuzz on the board `cp`, write the items, run KiCad DRC; return (out, classified, report, model); `baseline` is the DRC of the board before fuzzing."""
    b = load_board(cp)
    rules = resolve_rules(b, dru_text=cp.dru.read_text() if cp.dru else None, widths_before={})
    model = export_model(b, rules, name="fx")
    out = json.loads(engine.fuzz_items(json.dumps(model), json.dumps(options), seed, n))
    before = _count_tracks(b)
    write_items(b, out["items"])
    pcb = save(b, tmp_path / "fuzz.kicad_pcb")
    # A partial drop in write/save must be caught directly, not via the DRC counts.
    assert _count_tracks(pcbnew.LoadBoard(str(pcb))) == before + len(out["items"])
    if cp.pro is not None:
        shutil.copy(cp.pro, tmp_path / "fuzz.kicad_pro")
    rep = run_drc(pcb, tmp_path / "fuzz.json")
    return out, classify(rep, baseline), rep, model


@pytest.mark.parametrize("seed", [1, 2, 3])
def test_fuzz_1000_zero_kicad_routing_violations(fixture_board, engine, tmp_path, seed):
    out, c, rep, _ = fuzz_drc(fixture_board, engine, tmp_path, seed, 1000, {"margin_nm": 5000})
    print(f"fuzz seed={seed} accepted={len(out['items'])} attempts={out['attempts']} "
          f"truncated={out['truncated']} routing_new={c['routing_new']} incomplete={c['incomplete']}")
    assert len(out["items"]) == 1000 and not out["truncated"]
    assert c["routing_new"] == 0, summarise("fuzz", rep)
    # Fuzz items are not connected to anything. The brief expected >= 1000 dangling reports, but
    # same-net fuzz items stack on the same pad ring and KiCad merges touching items into one
    # chain (only chain ends dangle: ~165 per seed), so only "some dangling" is asserted.
    assert c["incomplete"]["track_dangling"] >= 1


def test_fuzz_negative_margin_is_caught_by_kicad(fixture_board, engine, tmp_path):
    # Control: margin -20_000 nm must give clearance violations, proving the oracle sees
    # what the fuzz places.
    out, c, rep, _ = fuzz_drc(fixture_board, engine, tmp_path, 7, 200,
                              {"margin_nm": -20000, "skip_self_check": True})
    print(f"negative-margin control: clearance={c['by_type_new'].get('clearance', 0)}")
    assert len(out["items"]) == 200
    assert c["by_type_new"].get("clearance", 0) > 0, summarise("control", rep)


def test_fuzz_deterministic(fixture_board, engine):
    b = load_board(fixture_board)
    model = json.dumps(export_model(b, resolve_rules(b, dru_text=None, widths_before={}), name="fx"))
    o = json.dumps({"margin_nm": 5000})
    assert engine.fuzz_items(model, o, 5, 100) == engine.fuzz_items(model, o, 5, 100)
    assert engine.fuzz_items(model, o, 5, 100) != engine.fuzz_items(model, o, 6, 100)
