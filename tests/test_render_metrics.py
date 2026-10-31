import hashlib
import struct
import subprocess
from pathlib import Path

import pcbnew
import pytest

from harness.bench import run_bench
from harness.copy import sha256_tree
from harness.metrics import copper_metrics
from harness.render import caption, compose_compare, render_png, strip_zone_fills


def _png_size(path: Path) -> tuple[int, int]:
    with open(path, "rb") as f:
        head = f.read(24)
    assert head[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    return struct.unpack(">II", head[16:24])


def _solid_png(path: Path, w: int, h: int) -> Path:
    subprocess.run(["convert", "-size", f"{w}x{h}", "xc:white", str(path)], check=True)
    return path


def test_render_fixture_dimensions(fixture_board, tmp_path):
    png = render_png(fixture_board.pcb, tmp_path / "fx.png")
    w, h = _png_size(png)
    assert h == 1100 and 1300 <= w <= 1450  # 50x40 mm board -> 1376x1100 observed


def test_render_strips_zone_fills(fixture_board, tmp_path):
    # Make a filled copy of the fixture (the fixture itself may ship unfilled zones).
    filled = tmp_path / "filled" / "fx.kicad_pcb"
    filled.parent.mkdir()
    b = pcbnew.LoadBoard(str(fixture_board.pcb))
    filler = pcbnew.ZONE_FILLER(b)
    filler.Fill(b.Zones())
    assert b.Zones()[0].IsFilled()
    pcbnew.SaveBoard(str(filled), b)

    src_hashes = sha256_tree(filled.parent)
    dst = strip_zone_fills(filled, tmp_path / "stripped.kicad_pcb")
    assert dst.is_file()
    assert not pcbnew.LoadBoard(str(dst)).Zones()[0].IsFilled()
    assert pcbnew.LoadBoard(str(filled)).Zones()[0].IsFilled()
    assert sha256_tree(filled.parent) == src_hashes


def test_caption_and_compare_sizes(tmp_path):
    left = _solid_png(tmp_path / "l.png", 300, 1100)
    right = _solid_png(tmp_path / "r.png", 500, 1100)
    lc = caption(left, "hand", tmp_path / "lc.png")
    rc = caption(right, "router", tmp_path / "rc.png")
    assert _png_size(lc)[1] == 1160 and _png_size(rc)[1] == 1160
    out = compose_compare(lc, rc, tmp_path / "cmp.png")
    w, h = _png_size(out)
    assert h == 1200 and w == 300 + 500 + 4 * 20


def test_copper_metrics_fixture(fixture_board):
    m = copper_metrics(pcbnew.LoadBoard(str(fixture_board.pcb)))
    assert m["segments"] == 1 and m["arcs"] == 1 and m["vias"] == 1
    assert m["track_mm"] == pytest.approx(16.0, abs=0.01)
    # arc R 2.5 mm, chord 4 mm -> sweep 2*asin(0.8) = 1.8546 rad -> length R*sweep = 4.636 mm
    assert m["arc_mm"] == pytest.approx(4.636, abs=0.01)


def test_run_bench_no_router_on_fixture(fixture_board, tmp_path):
    r = run_bench("fx", tmp_path, router=None, pcb_override=fixture_board.pcb)
    assert (tmp_path / "fx--metrics.json").exists()
    assert (tmp_path / "fx-hand.png").exists()
    assert r["routed_all"]["segments"] == 1  # only the locked track survives


def test_metrics_json_carries_input_pcb_sha256(fixture_board, tmp_path):
    import json

    r = run_bench("fx", tmp_path, router=None, pcb_override=fixture_board.pcb)
    want = hashlib.sha256(fixture_board.pcb.read_bytes()).hexdigest()
    assert r["pcb_sha256"] == want
    assert json.loads((tmp_path / "fx--metrics.json").read_text())["pcb_sha256"] == want
