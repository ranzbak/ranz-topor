"""R0 witness helpers: render the hand routing with the model violations marked.

    .venv/bin/python -m harness.r0 render <name>--hand.json <name>-r0.json <png>

The left/main picture is the hand routing (F green, B blue, vias black, pads as squares); every
violation outside a hand DRC spot gets a numbered red ring and the violated cut (CDT edge or
obstacle-pair cut) as a red line; violations the hand itself causes (DRC spot or below the rule)
get an orange ring. Zoom insets of the first violations follow below the board.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

from harness.g0 import MM, _prim_pts


def render(hand_json: Path, r0_json: Path, png: Path, width: int = 2400, insets: int = 8) -> Path:
    from PIL import Image, ImageDraw

    hand = json.loads(Path(hand_json).read_text())
    r0 = json.loads(Path(r0_json).read_text())
    viol = r0["violations"]
    s = r0["summary"]
    pts = [q for o in hand["outline"] for q in _prim_pts(o)] or [p["center"] for p in hand["pads"]]
    x0, x1 = min(q[0] for q in pts), max(q[0] for q in pts)
    y0, y1 = min(q[1] for q in pts), max(q[1] for q in pts)
    sc = (width - 60) / max(x1 - x0, 1)
    h_board = int((y1 - y0) * sc) + 120
    layer_ids = [l["id"] for l in hand["layers"] if l.get("routable", True)]
    col = {layer_ids[0]: (30, 140, 60), layer_ids[-1]: (40, 90, 220)}

    def draw_board(dr, xy, scale, lw_min=1):
        for o in hand["outline"]:
            dr.line([xy(q) for q in _prim_pts(o)], fill=(120, 120, 120), width=2)
        for p in hand["pads"]:
            c = xy(p["center"])
            r = max(2, 0.4 * MM * scale)
            dr.rectangle([c[0] - r, c[1] - r, c[0] + r, c[1] + r], outline=(170, 150, 60))
        for k in hand["kept"]:
            if k["layer"] in col:
                dr.line([xy(q) for q in _prim_pts(k["prim"])], fill=col[k["layer"]], width=max(lw_min, int(k["width"] * scale)))
            else:
                dr.line([xy(q) for q in _prim_pts(k["prim"])], fill=(170, 170, 170), width=max(lw_min, int(k["width"] * scale)))
        for v in hand["vias"]:
            c, r = xy(v["at"]), max(2, v["d"] * scale / 2)
            dr.ellipse([c[0] - r, c[1] - r, c[0] + r, c[1] + r], fill=(30, 30, 30))

    def draw_viol(dr, xy, scale, ring, only=None):
        for v in viol:
            if only is not None and v["id"] not in only:
                continue
            soft = v["hand_drc"] or v.get("hand_below_rule_mm") is not None
            colr = (255, 150, 0) if soft else (255, 0, 0)
            if v.get("line"):
                dr.line([xy(q) for q in v["line"]], fill=colr, width=max(2, int(0.08 * MM * scale)))
            c = xy(v["at"])
            dr.ellipse([c[0] - ring, c[1] - ring, c[0] + ring, c[1] + ring], outline=colr, width=3)
            dr.text((c[0] + ring + 2, c[1] - ring), str(v["id"]), fill=colr)

    img = Image.new("RGB", (width, h_board + (370 if insets and viol else 0)), "white")
    dr = ImageDraw.Draw(img)
    xy = lambda q: (30 + (q[0] - x0) * sc, 100 + (q[1] - y0) * sc)  # noqa: E731
    draw_board(dr, xy, sc)
    draw_viol(dr, xy, sc, 14)
    c = s["counts"]
    dr.text((20, 10), f"{s['board']}: hand routing; model violations: edge fit {c['edge_fit']}, layer combo {c['layer_combo']}, "
            f"allowed {c['allowed']}, pair fit {c['pair_fit']}, same-layer crossings {c['same_layer']}", fill="black")
    dr.text((20, 30), f"hand DRC {c['hand_drc']}, hand below rule {c['hand_below_rule']} (orange); mapped {s['mapped_pct']:.2f} %; "
            f"P3 {s['p3_vias'] if s['p3_ran'] else 'n/a'} vias vs hand {s['hand_signal_vias']}", fill="black")
    dr.text((20, 50), "F green, B blue, red ring + line = model violation (numbered), orange = hand's own fault", fill="black")
    first = sorted(viol, key=lambda v: (v["hand_drc"] or v.get("hand_below_rule_mm") is not None, v["id"]))[:insets]
    if insets and first:
        cell, win = 290, 5 * MM
        for i, v in enumerate(first):
            ox, oy = 10 + i * (cell + 10), h_board + 20
            k = cell / (2 * win)
            cx, cy = v["at"]
            sub = Image.new("RGB", (cell, cell), "white")
            sd = ImageDraw.Draw(sub)
            sxy = lambda q: ((q[0] - cx + win) * k, (q[1] - cy + win) * k)  # noqa: E731
            draw_board(sd, sxy, k, 1)
            draw_viol(sd, sxy, k, 16, only={v["id"]})
            sd.rectangle([0, 0, cell - 1, cell - 1], outline=(0, 0, 0))
            img.paste(sub, (ox, oy))
            dr.text((ox + 3, oy + cell + 2), f"#{v['id']} {v['check']} L{v['layer']} need {v['need_mm']:.2f} have {v['have_mm']:.2f}", fill="black")
    png = Path(png)
    png.parent.mkdir(parents=True, exist_ok=True)
    img.save(png)
    return png


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("render")
    r.add_argument("hand_json", type=Path)
    r.add_argument("r0_json", type=Path)
    r.add_argument("png", type=Path)
    a = ap.parse_args(argv)
    print(render(a.hand_json, a.r0_json, a.png))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
