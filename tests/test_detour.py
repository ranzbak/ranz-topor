"""Detour metric (M3-R11): stats, caption line, PNG highlighting. No engine or kicad-cli needed."""
import subprocess

import pytest

from harness import progress
from harness.metrics import detour_line, detour_stats
from harness.render import highlight_points


def _c(i, length, ghost, status="routed", at_from=(0, 0), at_to=(1_000_000, 0)):
    return {"id": i, "name": f"N{i}", "status": status, "length_nm": length,
            "ghost_len_nm": ghost, "from": {"at": list(at_from)}, "to": {"at": list(at_to)}}


def test_stats_normal():
    conns = [_c(1, 10_000_000, 10_000_000), _c(2, 20_000_000, 10_000_000),
             _c(3, 12_000_000, 10_000_000), _c(4, 16_000_000, 10_000_000)]
    s = detour_stats(conns)
    assert s["n"] == 4 and s["no_reference"] == 0
    assert s["median"] == 1.4 and s["max"] == 2.0 and s["count_gt_1_5"] == 2
    assert s["p90"] == 1.88
    assert [r["id"] for r in s["top10"]] == [2, 4, 3, 1]
    t = s["top10"][0]
    assert t["name"] == "N2" and t["length_mm"] == 20.0 and t["ghost_mm"] == 10.0
    assert t["ratio"] == 2.0 and t["from"] == {"at": [0, 0]}


def test_stats_ties_by_id_and_top10_cap():
    conns = [_c(i, 20_000_000, 10_000_000) for i in (7, 3, 5)]
    assert [r["id"] for r in detour_stats(conns)["top10"]] == [3, 5, 7]
    many = [_c(i, 10_000_000 + i, 10_000_000) for i in range(30)]
    assert len(detour_stats(many)["top10"]) == 10


def test_stats_nulls_and_unrouted_excluded():
    conns = [_c(1, 15_000_000, 10_000_000), _c(2, 9, None), _c(3, 9, 0),
             _c(4, 0, 10_000_000, status="failed"), _c(5, 9, 5, status="partial")]
    s = detour_stats(conns)
    assert s["n"] == 1 and s["no_reference"] == 4 and s["max"] == 1.5
    assert s["count_gt_1_5"] == 0


def test_stats_missing_field_and_empty():
    assert detour_stats([{"id": 1, "status": "routed", "length_nm": 5}]) is None
    assert detour_stats([]) is None
    assert detour_stats(None) is None
    allnull = detour_stats([_c(1, 5, None)])
    assert allnull["n"] == 0 and allnull["median"] is None and allnull["top10"] == []
    assert detour_line(allnull) is None


def test_detour_line():
    s = detour_stats([_c(1, 10_000_000, 10_000_000), _c(2, 20_000_000, 10_000_000)])
    assert detour_line(s) == "detour median 1.500 p90 1.900 max 2.000 >1.5: 1"
    assert detour_line(None) is None


def test_caption_line():
    s = detour_stats([_c(1, 10_000_000, 10_000_000), _c(2, 25_000_000, 10_000_000),
                      _c(3, 20_000_000, 10_000_000), _c(4, 18_000_000, 10_000_000),
                      _c(5, 16_000_000, 10_000_000)])
    assert progress.detour_caption(s) == "detours > 1.5: 4 (max 2.5) — N2 2.5, N3 2.0, N4 1.8"
    assert progress.detour_caption(None) is None
    assert progress.detour_caption(detour_stats([_c(1, 5, None)])) is None


def test_m3_caption_has_required_fields():
    result = {"route": {"routed": 9, "connections": 10, "nets_routed": 3, "nets": 4,
                        "partial": 0, "failed": 1, "vias": 2, "iterations": 5,
                        "length_by_layer_mm": {}, "threads_used": 4},
              "routed_all": {"arcs": 0}, "orig_all": {}, "zero_slack": {},
              "routed_drc": {"routing_new": 3, "incomplete": {"unconnected_items": 1}},
              "router_s": 1.234}
    text = progress.m3_caption("T", result, 4)
    assert "9/10 connections" in text and "vias 2" in text
    assert "DRC routing 3" in text and "unconnected 1" in text and "router 1.23 s" in text
    assert "detours" not in text
    result["detour"] = detour_stats([_c(1, 20_000_000, 10_000_000)])
    assert text in progress.m3_caption("T", result, 4)
    assert progress.m3_caption("T", result, 4).endswith("detours > 1.5: 1 (max 2.0) — N1 2.0")


def test_highlight_changes_pixel(tmp_path):
    Image = pytest.importorskip("PIL.Image")
    src = tmp_path / "in.png"
    subprocess.run(["convert", "-size", "200x100", "xc:white", str(src)], check=True)
    # bbox 0..20mm x 0..10mm onto 200x100 px => 10 px/mm
    conns = [_c(1, 20, 10, at_from=(5_000_000, 5_000_000), at_to=(15_000_000, 5_000_000))]
    out = highlight_points(src, tmp_path / "out.png", [(c["from"]["at"], c["to"]["at"])
                                                         for c in conns],
                           bbox_nm=(0, 0, 20_000_000, 10_000_000), radius_px=10)
    im = Image.open(out).convert("RGB")
    assert im.size == (200, 100)
    # the ring around (50,50) passes through (60,50); centre stays untouched
    assert im.getpixel((60, 50)) != (255, 255, 255)
    assert im.getpixel((150 - 10, 50)) != (255, 255, 255)
    assert im.getpixel((10, 10)) == (255, 255, 255)
    assert im.getpixel((50, 30)) == (255, 255, 255)


def test_progress_accepts_long_board():
    assert progress.board_options("esp-rust-board") == {"wall_clock_s": 60, "partial_routes": False}
    assert progress.board_options("eprom-to-kernal") is None


def test_detour_stats_uses_reference_and_reports_ghost():
    def rc(i, length, ref, ghost, kind="feasible"):
        c = _c(i, length, ghost)
        c.update(ref_len_nm=ref, ref_kind=kind)
        return c

    conns = [rc(1, 29_000_000, 29_000_000, 10_000_000),          # ghost says 2.9, reference 1.0
             rc(2, 30_000_000, 20_000_000, 10_000_000, "ghost"),  # fallback: ref == ghost length
             rc(3, 15_000_000, 1_000_000, 1_000_000)]             # short reference (< 2 mm)
    conns[1]["ref_len_nm"] = conns[1]["ghost_len_nm"] = 10_000_000
    ref = detour_stats(conns, field="ref_len_nm")
    ghost = detour_stats(conns, field="ghost_len_nm")
    assert ref["field"] == "ref_len_nm" and ghost["field"] == "ghost_len_nm"
    assert [r["id"] for r in ref["top10"]] == [3, 2, 1] or ref["top10"][0]["id"] == 3
    r1 = next(r for r in ref["top10"] if r["id"] == 1)
    g1 = next(r for r in ghost["top10"] if r["id"] == 1)
    assert r1["ratio"] == 1.0 and g1["ratio"] == 2.9
    assert ref["ghost_fallbacks"] == 1 and ref["short_ref"] == 1
    assert ghost["ghost_fallbacks"] == 0
    assert detour_line(ref).endswith("(ghost fallbacks 1)")
    assert "fallbacks" not in detour_line(ghost)
    # old engine: no ref field at all
    assert detour_stats([_c(1, 5, 5)], field="ref_len_nm") is None


def _tree_conns():
    # A tree on a line 0 - 4 - 10 mm with two connections; the first one wanders (9 mm against a
    # 4 mm ghost), the second is short. Plus a two-pad net.
    mm = 1_000_000
    pad = lambda x: {"kind": "pad", "at": [x * mm, 0]}
    return [
        {"id": 1, "net": 7, "name": "T", "status": "routed", "length_nm": 9 * mm, "ghost_len_nm": 4 * mm,
         "from": pad(0), "to": pad(4)},
        {"id": 2, "net": 7, "name": "T", "status": "routed", "length_nm": 2 * mm, "ghost_len_nm": 6 * mm,
         "from": pad(4), "to": pad(10)},
        {"id": 3, "net": 8, "name": "S", "status": "routed", "length_nm": 3 * mm, "ghost_len_nm": 2 * mm,
         "from": pad(0), "to": pad(2)},
    ]


def test_tree_net_metric_sits_next_to_the_per_connection_rows():
    s = detour_stats(_tree_conns())
    # Per connection unchanged: the wandering branch stays visible as the worst row.
    assert s["n"] == 3 and s["max"] == 2.25 and s["top10"][0]["id"] == 1
    # Tree metric beside it: (9 + 2) / MST 10.
    assert s["trees"]["n"] == 1 and s["trees"]["max"] == 1.1 and s["trees"]["count_gt_1_5"] == 0
    assert s["trees"]["top10"][0]["pads"] == 3 and s["trees"]["top10"][0]["ids"] == [1, 2]
    assert "tree nets" in detour_line(s)


def test_tree_with_an_unrouted_connection_has_no_tree_row_and_keeps_no_reference_count():
    c = _tree_conns()
    c[1]["status"] = "failed"
    s = detour_stats(c)
    assert s["trees"]["n"] == 0
    # The routed connections of that net still count per connection; the failed one is no reference.
    assert s["n"] == 2 and s["no_reference"] == 1
