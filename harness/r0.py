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
import math
from pathlib import Path

from harness.g0 import MM, _prim_pts


def pad_outline(p: dict, layer) -> list[list[float]]:
    """The copper outline (KiCad frame, nm) of pad `p` on `layer`: round / oval / rect / roundrect / poly
    at the pad's rotation, circles and rounded corners as polygons (24 steps per turn)."""
    sh = p["shapes"].get(str(layer))
    if sh is None:
        return []
    cx, cy = p["center"]
    th = math.radians(p.get("rotation_deg", 0.0))
    ct, st = math.cos(th), math.sin(th)

    def rot(x, y):  # KiCad frame (y down): a positive angle turns counter-clockwise on screen
        return [cx + x * ct + y * st, cy - x * st + y * ct]

    def arc(x0, y0, r, a0, a1, n=6):
        return [(x0 + r * math.cos(a0 + (a1 - a0) * i / n), y0 + r * math.sin(a0 + (a1 - a0) * i / n)) for i in range(n + 1)]

    k = sh["kind"]
    if k == "poly":
        return [list(q) for q in sh["pts"]]
    if k == "circle":
        r = sh["d"] / 2
        return [rot(*q) for q in arc(0, 0, r, 0, 2 * math.pi, 24)]
    w, h = sh["w"] / 2, sh["h"] / 2
    if k == "rect":
        return [rot(-w, -h), rot(w, -h), rot(w, h), rot(-w, h)]
    if k == "oval":
        r = min(w, h)
        if w >= h:
            pts = arc(w - r, 0, r, -math.pi / 2, math.pi / 2) + arc(-(w - r), 0, r, math.pi / 2, 3 * math.pi / 2)
        else:
            pts = arc(0, h - r, r, 0, math.pi) + arc(0, -(h - r), r, math.pi, 2 * math.pi)
        return [rot(*q) for q in pts]
    r = min(sh.get("rc", 0), w, h)  # roundrect
    pts = (arc(w - r, h - r, r, 0, math.pi / 2) + arc(-(w - r), h - r, r, math.pi / 2, math.pi)
           + arc(-(w - r), -(h - r), r, math.pi, 3 * math.pi / 2) + arc(w - r, -(h - r), r, 3 * math.pi / 2, 2 * math.pi))
    return [rot(*q) for q in pts]


def draw_pad(dr, xy, scale, p: dict) -> None:
    """Pads with their real shapes: the copper of each layer tinted like the layer, a drill as a white disc."""
    tint = {0: (190, 235, 200), 2: (190, 205, 245)}
    for layer in sorted(p["layers"], reverse=True):  # B first, F on top
        pts = pad_outline(p, layer)
        if len(pts) >= 3:
            both = len(p["layers"]) > 1
            dr.polygon([xy(q) for q in pts], fill=tint.get(layer, (225, 225, 225)), outline=(60, 60, 60) if both or layer == 0 else (90, 90, 150))
    d = p.get("drill")
    if d:
        c = xy(p["center"])
        r = max(1.0, 0.25 * (d["w"] + d["h"]) * scale)
        dr.ellipse([c[0] - r, c[1] - r, c[0] + r, c[1] + r], fill=(255, 255, 255), outline=(60, 60, 60))


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
            draw_pad(dr, xy, scale, p)
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
            colr = (255, 150, 0) if v["hand_drc"] else (255, 0, 0)
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
    dr.text((20, 30), f"in a kicad-cli DRC spot {c['hand_drc']} (orange); mapped {s['mapped_pct']:.2f} %; "
            f"P3 {s['p3_vias'] if s['p3_ran'] else 'n/a'} vias vs hand {s['hand_signal_vias']}", fill="black")
    dr.text((20, 50), "F green, B blue, red ring + line = model violation (numbered), orange = at a flagged kicad-cli DRC item", fill="black")
    first = sorted(viol, key=lambda v: (v["hand_drc"], v["id"]))[:insets]
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


COPPER_CHECKS = ("clearance", "copper_edge_clearance", "hole_clearance", "hole_to_hole", "shorting_items",
                 "tracks_crossing", "track_width", "via_diameter", "annular_width", "connection_width")


def drc(work: Path, out_json: Path, force_copper: bool) -> dict:
    """`kicad-cli pcb drc --severity-all` of the untouched copy in `work` (a `*-work` directory
    written by `harness.g0 export`). With `force_copper` the copper-geometry checks that the project
    set to ignore / warning are raised to error in a scratch copy (the original is not touched), so
    that a hand fault in a disabled check is reported by kicad-cli too. Returns {type: count}."""
    import collections
    import shutil
    import subprocess
    import tempfile

    pcb = next(Path(work).glob("*.kicad_pcb"))
    with tempfile.TemporaryDirectory() as td:
        t = Path(td)
        for p in Path(work).iterdir():
            if p.is_file() and not p.name.startswith("~"):
                shutil.copy(p, t / p.name)
            elif p.is_dir():
                shutil.copytree(p, t / p.name)
        pro = t / (pcb.stem + ".kicad_pro")
        sev = {}
        if pro.exists() and force_copper:
            d = json.loads(pro.read_text())
            rs = d["board"]["design_settings"]["rule_severities"]
            sev = {k: rs.get(k) for k in COPPER_CHECKS if rs.get(k) != "error"}
            for k in COPPER_CHECKS:
                rs[k] = "error"
            pro.write_text(json.dumps(d, indent=2))
        out_json.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["kicad-cli", "pcb", "drc", "--format", "json", "--severity-all", "--units", "mm",
                        "-o", str(out_json.resolve()), str(t / pcb.name)], check=True, capture_output=True)
    rep = json.loads(out_json.read_text())
    rep["forced_copper_checks"] = sev
    out_json.write_text(json.dumps(rep))
    return dict(collections.Counter(v["type"] for v in rep["violations"]))


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("render")
    r.add_argument("hand_json", type=Path)
    r.add_argument("r0_json", type=Path)
    r.add_argument("png", type=Path)
    d = sub.add_parser("drc")
    d.add_argument("work", type=Path)
    d.add_argument("out_json", type=Path)
    d.add_argument("--project-severities", action="store_true", help="do not force the copper checks to error")
    a = ap.parse_args(argv)
    if a.cmd == "drc":
        print(drc(a.work, a.out_json, not a.project_severities))
        return 0
    print(render(a.hand_json, a.r0_json, a.png))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
