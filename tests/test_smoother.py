import json
import subprocess
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
                       "wires": [{"status": "taut"}, {"status": "taut"},
                                {"id": 2, "net": 9, "status": "fallback", "zero_slack": True,
                                 "reason": "zero slack: x"}],
                       "wires_zero_slack": 1,
                       "nets_total": 2, "nets_taut": 1, "wires_total": 3, "wires_taut": 2,
                       "total_length_nm": 5 * MM,
                       "hand_length_nm": 6 * MM, "length_outside_pads_nm": 3 * MM,
                       "hand_outside_pads_nm": 4 * MM, "iterations": 7, "timed_out": False}}


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
    assert r["smooth"] == {"nets_taut": 1, "nets": 2, "wires_taut": 2, "wires": 3,
                           "length_mm": 5.0, "hand_mm": 6.0, "outside_mm": 3.0,
                           "hand_outside_mm": 4.0, "iterations": 7, "timed_out": False,
                           "wires_zero_slack": 1,
                           "fallback_reasons": [{"id": 2, "net": 9, "zero_slack": True,
                                                 "reason": "zero slack: x"}],
                           "length_outside_pads_mm": 3.0, "hand_outside_pads_mm": 4.0}
    out = capsys.readouterr().out
    assert "smooth: taut 1/2 nets, 2/3 wires, 5.0 mm (hand 6.0), outside pads 3.0 (hand 4.0), iters 7" in out


def test_run_bench_list_router_still_works(fixture_board, tmp_path, capsys):
    r = bench.run_bench("fixture", tmp_path / "o", router=_list_router, rip="none",
                        pcb_override=fixture_board.pcb)
    assert r["replaced"] == 0 and r["routed_items"] == 1
    assert r["smooth"] is None and r["router_report"] is None
    assert "smooth:" not in capsys.readouterr().out


def test_caption_multiline_grows_band_and_fits_width(tmp_path):
    from PIL import Image

    from harness.render import caption

    base = tmp_path / "base.png"
    subprocess.run(["convert", "-size", "300x400", "xc:white", str(base)], check=True)
    long_text = "x" * 200 + "\n" + "y" * 200 + " 100% of 50%"
    out = caption(base, long_text, tmp_path / "long.png")
    assert Image.open(out).size == (300, 400 + 96)  # wide lines shrink the font, width is kept


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
    d = res["routed_drc"]
    line1 = f"T1.4a: 1/2 nets, 2/3 wires, fallback 1 (zero slack 1), arcs {n['arcs']}"
    line2 = (f"DRC routing {d['routing_new']}, unconnected {d['incomplete']['unconnected_items']}, "
             f"{n['track_mm'] + n['arc_mm']:.1f} mm vs hand "
             f"{res['orig_all']['track_mm'] + res['orig_all']['arc_mm']:.1f} mm, "
             f"outside pads 3.0 vs 4.0 mm")
    assert res["caption"] == f"{line1}\n{line2}"
    # hand image is rendered from the same board at the same height: compare the sizes.
    # The two-line topor caption band is 28 + 36 + 2*16 = 96 px; the one-line hand band is 60 px.
    from harness.render import render_png, caption as cap
    raw = render_png(fixture_board.pcb, tmp_path / "h.png")
    w, h = Image.open(raw).size
    assert Image.open(png).size == (2 * (w + 40), h + 96 + 40)
    assert Image.open(cap(raw, "x", tmp_path / "hc.png")).size == (w, h + 60)


def _td_zone(net, layer, x0, y0, x1, y1):
    return (f'\t(zone\n\t\t(net "{net}")\n\t\t(layer "{layer}")\n\t\t(hatch none 0.1)\n'
            f'\t\t(attr\n\t\t\t(teardrop\n\t\t\t\t(type padvia)\n\t\t\t)\n\t\t)\n'
            f'\t\t(connect_pads yes\n\t\t\t(clearance 0)\n\t\t)\n\t\t(min_thickness 0.0254)\n'
            f'\t\t(polygon\n\t\t\t(pts\n\t\t\t\t(xy {x0} {y0}) (xy {x1} {y0}) (xy {x1} {y1}) (xy {x0} {y1})\n'
            f'\t\t\t)\n\t\t)\n\t)\n')


def _board_with_teardrops(fixture_board, tmp_path):
    """Fixture board with SIG tracks at y=40 and teardrop zones: on track 0 (SIG, F.Cu), far away
    (SIG, F.Cu), on track 0 but another net (GND), on track 0 but another layer (B.Cu)."""
    b = pcbnew.LoadBoard(str(fixture_board.pcb))
    net = b.GetNetcodeFromNetname("SIG")
    ids = writer.write_items(b, [_sig_item(net, i * 10) for i in range(3)])  # x 40-41, 50-51, 60-61
    (tmp_path / "src").mkdir()
    path = writer.save(b, tmp_path / "src" / "td.kicad_pcb")
    text = path.read_text()
    zones = (_td_zone("SIG", "F.Cu", 39.5, 39.5, 41.5, 40.5)
             + _td_zone("SIG", "F.Cu", 100, 100, 101, 101)
             + _td_zone("GND", "F.Cu", 39.5, 39.5, 41.5, 40.5)
             + _td_zone("SIG", "B.Cu", 39.5, 39.5, 41.5, 40.5))
    path.write_text(text.rstrip()[:-1] + zones + ")\n")
    return pcbnew.LoadBoard(str(path)), ids


def test_teardrops_removed_with_replaced_track(fixture_board, tmp_path):
    b, ids = _board_with_teardrops(fixture_board, tmp_path)
    tds = [z for z in b.Zones() if z.IsTeardropArea()]
    assert len(tds) == 4
    n, t = writer.remove_items_and_teardrops(b, [ids[0]])
    assert (n, t) == (1, 1)
    left = [(z.GetNetname(), int(z.GetLayer())) for z in b.Zones() if z.IsTeardropArea()]
    assert sorted(left) == sorted([("SIG", int(pcbnew.F_Cu)), ("GND", int(pcbnew.F_Cu)),
                                   ("SIG", int(pcbnew.B_Cu))])
    # a removal that touches no teardrop keeps them all
    n, t = writer.remove_items_and_teardrops(b, [ids[2]])
    assert (n, t) == (1, 0)
    assert writer.remove_items_and_teardrops(b, []) == (0, 0)


def test_bench_reports_teardrops_removed(fixture_board, tmp_path, capsys):
    b, ids = _board_with_teardrops(fixture_board, tmp_path)
    pcb = tmp_path / "src" / "td.kicad_pcb"
    victim = ids[0]

    def router(model, options):
        return {"items": [], "remove_uuids": [victim], "report": {"nets": [], "wires": []}}

    r = bench.run_bench("fixture", tmp_path / "o", router=router, rip="none", pcb_override=pcb)
    assert r["replaced"] == 1 and r["teardrops_removed"] == 1
    assert "teardrops_removed 1" in capsys.readouterr().out
