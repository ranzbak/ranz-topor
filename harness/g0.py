"""G0 flat-plane gate helpers: export a board copy as models, render the gate PNG.

    .venv/bin/python -m harness.g0 export <board|pcb> <out_dir>
    .venv/bin/python -m harness.g0 render <out_dir>/<name>--hand.json <g0.json> <png>

`export` copies the project (the source is never written), exports the untouched copy as
`<name>--hand.json` (the hand reference: tracks in `kept`, vias in `vias`) and the copy with every
track and via removed as `<name>--input.json` (the router input). `render` draws the hand routing
(left) and the G0 wires (right) on the same scale: F red, B blue, vias as dots, errors as rings.
"""
from __future__ import annotations

import argparse
import json
import math
from pathlib import Path

MM = 1_000_000


def export(src: str, out_dir: Path) -> tuple[Path, Path]:
    from harness.copy import copy_project
    from harness.model import export_model, load_board, write_model_json
    from harness.paths import BENCH_BOARDS
    from harness.rip import record_widths
    from harness.rules import resolve_rules

    pcb = BENCH_BOARDS[src] if src in BENCH_BOARDS else Path(src)
    name = src if src in BENCH_BOARDS else Path(src).stem
    out_dir.mkdir(parents=True, exist_ok=True)
    cp = copy_project(pcb, out_dir / f"{name}-work")
    b = load_board(cp)
    dru = cp.dru.read_text() if cp.dru is not None else None
    widths = record_widths(b)
    rules = resolve_rules(b, dru_text=dru, widths_before=widths)
    model = export_model(b, rules, name=name, dru_text=dru)
    # The hand reference carries the board's own rules (netclasses, design settings, .kicad_dru),
    # NOT raised to the fab profile: the hand copper was DRC-checked against those, so a witness
    # test against the floored rules would report the profile's capability limit as a model error.
    hand_rules = resolve_rules(b, dru_text=dru, widths_before=widths, apply_fab_floors=False)
    hand_model = export_model(b, hand_rules, name=name, dru_text=dru)
    # A copper layer with a pour and no track is a plane (qmtech In1/In2 are declared "signal"
    # in the pcb): not routable, as the plugin's plane detection decides for the real run.
    for mdl in (model, hand_model):
        tracked = {k["layer"] for k in mdl["kept"]}
        poured = {l for p in mdl["pours"] for l in p["layers"]}
        for l in mdl["layers"]:
            if l["routable"] and l["id"] in poured and l["id"] not in tracked:
                l["routable"] = False
    hand = write_model_json(hand_model, out_dir / f"{name}--hand.json")
    # The ripped copy: every track and via removed (pcbnew's SWIG objects are unreliable after
    # Remove(), so this is done on the exported model rather than on the board).
    model = {**model, "kept": [], "vias": [], "dangling_ends": []}
    return hand, write_model_json(model, out_dir / f"{name}--input.json")


def _arc_pts(p) -> list:
    s, m, e = p["start"], p["mid"], p["end"]
    ax, ay, bx, by, cx, cy = s[0], s[1], m[0], m[1], e[0], e[1]
    d = 2 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by))
    if abs(d) < 1e-9:
        return [s, e]
    ux = ((ax * ax + ay * ay) * (by - cy) + (bx * bx + by * by) * (cy - ay) + (cx * cx + cy * cy) * (ay - by)) / d
    uy = ((ax * ax + ay * ay) * (cx - bx) + (bx * bx + by * by) * (ax - cx) + (cx * cx + cy * cy) * (bx - ax)) / d
    r = math.hypot(ax - ux, ay - uy)
    a0, a1, a2 = (math.atan2(q[1] - uy, q[0] - ux) for q in (s, m, e))
    sweep = (a2 - a0) % (2 * math.pi)
    if (a1 - a0) % (2 * math.pi) > sweep:
        sweep -= 2 * math.pi
    return [[ux + r * math.cos(a0 + sweep * i / 12), uy + r * math.sin(a0 + sweep * i / 12)] for i in range(13)]


def _prim_pts(p) -> list:
    return [p["a"], p["b"]] if "a" in p else _arc_pts(p)


def render(hand_json: Path, g0_json: Path, png: Path, width: int = 2400) -> Path:
    from PIL import Image, ImageDraw

    hand = json.loads(Path(hand_json).read_text())
    g0 = json.loads(Path(g0_json).read_text())
    pts = [q for o in hand["outline"] for q in _prim_pts(o)] or [p["center"] for p in hand["pads"]]
    x0, x1 = min(q[0] for q in pts), max(q[0] for q in pts)
    y0, y1 = min(q[1] for q in pts), max(q[1] for q in pts)
    half = width // 2
    sc = (half - 40) / max(x1 - x0, 1)
    height = int((y1 - y0) * sc) + 120
    img = Image.new("RGB", (width, height), "white")
    dr = ImageDraw.Draw(img)
    layer_ids = [l["id"] for l in hand["layers"]]
    col = {layer_ids[0]: (220, 40, 40), layer_ids[-1]: (40, 90, 220)}

    def xy(q, off):  # KiCad frame (Y down) -> image
        return (off + 20 + (q[0] - x0) * sc, 100 + (q[1] - y0) * sc)

    def board(off):
        for o in hand["outline"]:
            dr.line([xy(q, off) for q in _prim_pts(o)], fill=(120, 120, 120), width=2)
        for p in hand["pads"]:
            c = xy(p["center"], off)
            r = max(2, 0.4 * MM * sc)
            dr.rectangle([c[0] - r, c[1] - r, c[0] + r, c[1] + r], outline=(170, 150, 60))

    for off in (0, half):
        board(off)
    for k in hand["kept"]:
        dr.line([xy(q, 0) for q in _prim_pts(k["prim"])], fill=col.get(k["layer"], (90, 90, 90)),
                width=max(1, int(k["width"] * sc)))
    for v in hand["vias"]:
        c, r = xy(v["at"], 0), max(2, v["d"] * sc / 2)
        dr.ellipse([c[0] - r, c[1] - r, c[0] + r, c[1] + r], fill=(30, 30, 30))
    lc = {0: (220, 40, 40), 1: (40, 90, 220), -1: (150, 150, 150)}
    for w in g0["wires"]:
        for st in w["stretches"]:
            dr.line([xy(q, half) for q in st["pts"]], fill=lc[st["layer"]], width=2)
    for v in g0["vias"]:
        c = xy(v, half)
        dr.ellipse([c[0] - 4, c[1] - 4, c[0] + 4, c[1] + 4], fill=(30, 30, 30))
    for e in g0["errors"]:
        c = xy(e, half)
        dr.ellipse([c[0] - 9, c[1] - 9, c[0] + 9, c[1] + 9], outline=(255, 0, 0), width=3)
    s = g0["summary"]
    dr.text((20, 10), f"hand: {s['hand_vias']} vias ({s['hand_signal_vias']} signal)", fill="black")
    dr.text((half + 20, 10),
            f"G0: {s['vias']} vias ({s['vias_no_site']} without site), crossings {s['crossings_before']}"
            f" -> {s['crossings_after']}, unlayerable {s['unlayerable']}, {s['runtime_s']:.2f} s",
            fill="black")
    dr.text((half + 20, 30), "F red, B blue, grey = unassigned, red ring = error", fill="black")
    png = Path(png)
    png.parent.mkdir(parents=True, exist_ok=True)
    img.save(png)
    return png


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    e = sub.add_parser("export")
    e.add_argument("board")
    e.add_argument("out_dir", type=Path)
    r = sub.add_parser("render")
    r.add_argument("hand_json", type=Path)
    r.add_argument("g0_json", type=Path)
    r.add_argument("png", type=Path)
    a = ap.parse_args(argv)
    if a.cmd == "export":
        for p in export(a.board, a.out_dir):
            print(p)
    else:
        print(render(a.hand_json, a.g0_json, a.png))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
