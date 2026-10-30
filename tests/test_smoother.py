import json
import sys
import types

import pcbnew
import pytest

from harness import bench, progress, writer
from harness.smoother import smooth_router

MM = 1_000_000


def _sig_item(net, i):
    return {"prim": {"kind": "seg", "a": [(40 + i) * MM, 40 * MM], "b": [(41 + i) * MM, 40 * MM]},
            "width": 250_000, "layer": int(pcbnew.F_Cu), "net": net}


def _fake_router(model, options):
    net = next(n["code"] if "code" in n else n["id"] for n in model["nets"] if n["name"] == "SIG")
    victim = next(k["uuid"] for k in model["kept"] if not k["locked"])
    return {"items": [_sig_item(net, i) for i in range(3)], "remove_uuids": [victim],
            "report": {"nets": [{"status": "taut"}, {"status": "partial"}],
                       "wires": [{"status": "taut"}] * 3, "total_length_nm": 5 * MM,
                       "hand_length_nm": 6 * MM, "iterations": 7, "timed_out": False}}


def _list_router(model, options):
    net = next(n["code"] if "code" in n else n["id"] for n in model["nets"] if n["name"] == "SIG")
    return [_sig_item(net, 0)]


def test_remove_items_by_uuid(fixture_board, tmp_path):
    b = pcbnew.LoadBoard(str(fixture_board.pcb))
    net = b.GetNetcodeFromNetname("SIG")
    ids = writer.write_items(b, [_sig_item(net, i) for i in range(3)])
    path = writer.save(b, tmp_path / "w.kicad_pcb")
    b = pcbnew.LoadBoard(str(path))
    before = len(list(b.GetTracks()))
    assert writer.remove_items(b, [ids[0], ids[1], "no-such-uuid"]) == 2
    assert len(list(b.GetTracks())) == before - 2


def test_run_bench_accepts_dict_router(fixture_board, tmp_path, capsys):
    r = bench.run_bench("fixture", tmp_path / "o", router=_fake_router, rip="none",
                        pcb_override=fixture_board.pcb)
    assert r["replaced"] == 1 and r["routed_items"] == 3
    assert r["smooth"] == {"nets_taut": 1, "nets": 2, "wires_taut": 3, "wires": 3,
                           "length_mm": 5.0, "hand_mm": 6.0, "iterations": 7, "timed_out": False}
    out = capsys.readouterr().out
    assert "smooth: taut 1/2 nets, 3/3 wires, 5.0 mm (hand 6.0), iters 7" in out


def test_run_bench_list_router_still_works(fixture_board, tmp_path, capsys):
    r = bench.run_bench("fixture", tmp_path / "o", router=_list_router, rip="none",
                        pcb_override=fixture_board.pcb)
    assert r["replaced"] == 0 and r["routed_items"] == 1
    assert r["smooth"] is None and r["router_report"] is None
    assert "smooth:" not in capsys.readouterr().out


def test_smooth_router_forces_smooth_option(monkeypatch):
    seen = {}

    def fake_smooth(model_json, options_json):
        seen["model"] = json.loads(model_json)
        seen["options"] = json.loads(options_json)
        return json.dumps({"items": [{"x": 1}], "replaced": ["u1"], "nets": []})

    mod = types.ModuleType("topor_engine")
    monkeypatch.setitem(sys.modules, "topor_engine", mod)
    monkeypatch.setattr(mod, "smooth", fake_smooth, raising=False)
    out = smooth_router({"m": 1}, {"existing_copper": "keep", "threads": 4})
    assert seen["options"]["existing_copper"] == "smooth" and seen["options"]["threads"] == 4
    assert seen["model"] == {"m": 1}
    assert set(out) == {"items", "remove_uuids", "report"}
    assert out["items"] == [{"x": 1}] and out["remove_uuids"] == ["u1"]
    assert out["report"]["nets"] == []


def test_progress_png_caption_and_path(fixture_board, tmp_path, monkeypatch):
    from PIL import Image  # noqa: F401  (skip below if unavailable)

    monkeypatch.setitem(bench.BENCH_BOARDS, "fixture", fixture_board.pcb)
    monkeypatch.setattr(progress, "smooth_router", _fake_router)
    res = progress.render_progress("T1.4a", board="fixture", date="2026-10-30",
                                   out_dir=tmp_path / "prog")
    png = tmp_path / "prog" / "2026-10-30-m1-T1.4a.png"
    assert png.is_file()
    n = res["routed_all"]
    expected = (f"T1.4a: 1/2 nets, 3/3 wires, DRC routing {res['routed_drc']['routing_new']}, "
                f"{n['track_mm'] + n['arc_mm']:.1f} mm vs 189.9 mm")
    assert res["caption"] == expected
    # hand image is rendered from the same board at the same height: compare the sizes
    from harness.render import render_png, caption as cap
    hand = cap(render_png(fixture_board.pcb, tmp_path / "h.png"), "x", tmp_path / "hc.png")
    w, h = Image.open(hand).size
    assert Image.open(png).size == (2 * (w + 40), h + 40)
