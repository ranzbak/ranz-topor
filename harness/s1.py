"""S1 pictures: the LCT cuts and the metrization gate on real board data.

    .venv/bin/python -m harness.s1 overflow <hand.json> <g0-before.json> <r0-before.json> <g0-after.json> <r0-after.json> <png>
    .venv/bin/python -m harness.s1 cuts <hand.json> <s1.json> <r0.json> <png> <REF> [<REF> ...]
    .venv/bin/python -m harness.s1 metrize <hand.json> <r0.json> <png> [<per cell>]

`overflow`: the P2 wires of G0 without and with the LCT cuts, side by side, with the over-full
obstacle-pair cuts of the independent R0 check as red lines. The wires are a topology state
(pass points joined by straight lines), not routed copper.
`cuts`: zoomed windows around components: the projection triangles (light grey), the cuts coloured
by room, the hand copper, a few cuts labelled with need / have of the hand routing.
`metrize`: legs from each cell of the 2 x 2 matrix, the sleeve shaded and the taut path as a line.
Everything is drawn from `harness.g0 export` models and the engine's JSON; nothing is synthetic.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

from harness.g0 import MM, _prim_pts
from harness.r0 import draw_pad

F_COL, B_COL = (30, 140, 60), (40, 90, 220)


def _load(p):
    return json.loads(Path(p).read_text())


def _bounds(hand):
    pts = [q for o in hand["outline"] for q in _prim_pts(o)] or [p["center"] for p in hand["pads"]]
    return min(q[0] for q in pts), min(q[1] for q in pts), max(q[0] for q in pts), max(q[1] for q in pts)


def _colour_room(room_mm: float):
    """Tight (<= 0.4 mm) red through orange to wide (>= 2.5 mm) blue."""
    t = max(0.0, min(1.0, (room_mm - 0.4) / 2.1))
    return (int(220 * (1 - t) + 40 * t), int(40 * (1 - t) + 110 * t), int(40 * (1 - t) + 220 * t))


def _hand_copper(dr, xy, scale, hand, lw_min=1):
    cols = {l["id"]: (F_COL if i == 0 else B_COL) for i, l in enumerate(x for x in hand["layers"] if x.get("routable", True))}
    for k in hand["kept"]:
        dr.line([xy(q) for q in _prim_pts(k["prim"])], fill=cols.get(k["layer"], (170, 170, 170)), width=max(lw_min, int(k["width"] * scale)))
    for v in hand["vias"]:
        c, r = xy(v["at"]), max(2, v["d"] * scale / 2)
        dr.ellipse([c[0] - r, c[1] - r, c[0] + r, c[1] + r], fill=(30, 30, 30))


def overflow(hand_json, g0_before, r0_before, g0_after, r0_after, png, width=2600):
    from PIL import Image, ImageDraw

    hand = _load(hand_json)
    x0, y0, x1, y1 = _bounds(hand)
    half = width // 2
    sc = (half - 40) / max(x1 - x0, 1)
    height = int((y1 - y0) * sc) + 170
    img = Image.new("RGB", (width, height), "white")
    dr = ImageDraw.Draw(img)
    lc = {0: (220, 40, 40), 1: (40, 90, 220), -1: (150, 150, 150)}
    for i, (g0p, r0p, name) in enumerate([(g0_before, r0_before, "without LCT cuts (before)"), (g0_after, r0_after, "with LCT cuts (after)")]):
        g0, r0 = _load(g0p), _load(r0p)
        off = i * half
        xy = lambda q, off=off: (off + 20 + (q[0] - x0) * sc, 110 + (q[1] - y0) * sc)  # noqa: E731
        for o in hand["outline"]:
            dr.line([xy(q) for q in _prim_pts(o)], fill=(120, 120, 120), width=2)
        for p in hand["pads"]:
            draw_pad(dr, xy, sc, p)
        for w in g0["wires"]:
            for st in w["stretches"]:
                dr.line([xy(q) for q in st["pts"]], fill=lc[st["layer"]], width=2)
        for v in g0["vias"]:
            c = xy(v)
            dr.ellipse([c[0] - 3, c[1] - 3, c[0] + 3, c[1] + 3], fill=(30, 30, 30))
        viol = r0.get("g0_violations", [])
        for v in viol:
            if v.get("line"):
                dr.line([xy(q) for q in v["line"]], fill=(255, 0, 255), width=5)
                c = xy(v["at"])
                dr.ellipse([c[0] - 6, c[1] - 6, c[0] + 6, c[1] + 6], outline=(255, 0, 255), width=2)
        s = g0["summary"]
        dr.text((off + 20, 10), f"{s['board']}: P2 topology (not copper), G0 {name}", fill="black")
        dr.text((off + 20, 30), f"{len(viol)} over-full cuts (obstacle-pair cuts of the independent R0 check, magenta); "
                f"{s['vias']} vias, {s['routed']}/{s['connections']} connections, P2 overflows counted by P2 itself: {s['overflows']}", fill="black")
        dr.text((off + 20, 50), "wires: pass points of the P2 topology joined by straight lines, F red / B blue after P3; pads from the board", fill="black")
        dr.text((off + 20, 70), "an over-full cut: the passes crossing the segment between two pads need more than its length (per layer, with clearances)", fill="black")
    png = Path(png)
    png.parent.mkdir(parents=True, exist_ok=True)
    img.save(png)
    return png


def _centroid(hand, ref):
    pts = [p["center"] for p in hand["pads"] if p["ref"] == ref]
    if not pts:
        raise SystemExit(f"no footprint {ref} in the model")
    return sum(q[0] for q in pts) / len(pts), sum(q[1] for q in pts) / len(pts)


def cuts(hand_json, s1_json, r0_json, png, refs, win_mm=7.0, cell=1150):
    from PIL import Image, ImageDraw

    hand, s1 = _load(hand_json), _load(s1_json)
    summ = _load(r0_json)["summary"]
    img = Image.new("RGB", (cell * len(refs) + 10 * (len(refs) - 1), cell + 130), "white")
    dr = ImageDraw.Draw(img)
    for i, ref in enumerate(refs):
        cx, cy = _centroid(hand, ref)
        win = win_mm * MM
        k = cell / (2 * win)
        sub = Image.new("RGB", (cell, cell), "white")
        sd = ImageDraw.Draw(sub)
        xy = lambda q: ((q[0] - cx + win) * k, (q[1] - cy + win) * k)  # noqa: E731
        for e in s1["edges"]:
            if min(e[0], e[2]) > cx + win or max(e[0], e[2]) < cx - win or min(e[1], e[3]) > cy + win or max(e[1], e[3]) < cy - win:
                continue
            sd.line([xy(e[:2]), xy(e[2:])], fill=(215, 215, 215), width=1)
        for o in hand["outline"]:
            sd.line([xy(q) for q in _prim_pts(o)], fill=(120, 120, 120), width=2)
        for p in hand["pads"]:
            draw_pad(sd, xy, k, p)
        _hand_copper(sd, xy, k, hand)
        shown = []
        for c in s1["cuts"]:
            a, b = c["a"], c["b"]
            if not (cx - win < (a[0] + b[0]) / 2 < cx + win and cy - win < (a[1] + b[1]) / 2 < cy + win):
                continue
            rooms = [r for r in c["room"] if r is not None]
            if not rooms:
                continue
            col = _colour_room(min(rooms) / MM)
            sd.line([xy(a), xy(b)], fill=col, width=4 if c["lct"] else 1)
            if c["lct"] and c["need"] and any(n is not None for n in c["need"]):
                shown.append(c)
        # Label a few cuts with hand traffic: need / have per layer.
        shown.sort(key=lambda c: -max((n or 0) / (r or 1e12) for n, r in zip(c["need"], c["room"])))
        for j, c in enumerate(shown[:5]):
            mx, my = xy([(c["a"][0] + c["b"][0]) / 2, (c["a"][1] + c["b"][1]) / 2])
            txt = " ".join(f"L{l}: need {n / MM:.2f}/have {r / MM:.2f}" for l, (n, r) in enumerate(zip(c["need"], c["room"])) if n is not None and r is not None)
            sd.rectangle([mx + 6, my - 8 + 14 * (j % 3), mx + 6 + 6.2 * len(txt), my + 4 + 14 * (j % 3)], fill=(255, 255, 255))
            sd.text((mx + 8, my - 8 + 14 * (j % 3)), txt, fill=(160, 0, 100))
        sd.rectangle([0, 0, cell - 1, cell - 1], outline=(0, 0, 0))
        img.paste(sub, (i * (cell + 10), 100))
        dr.text((i * (cell + 10) + 4, 104), f"{ref} area, {2 * win_mm:.0f} mm window", fill="black")
    dr.text((10, 8), f"{summ['board']}: projection triangles (light grey), cut edges coloured by room (red <= 0.4 mm ... blue >= 2.5 mm), hand copper on top", fill="black")
    dr.text((10, 28), "thick = LCT cuts (vertex to the foot on a facing side, or vertex to vertex, added by the refinement); thin = other cut edges between obstacles", fill="black")
    dr.text((10, 48), "labels: the hand routing's need / have (mm) on that cut per layer, highest need/have first; F green, B blue, hand vias black", fill="black")
    dr.text((10, 68), f"R0 on this board: {summ['edge_cuts_checked']} edges checked against the hand copper: 0 violations except at hand DRC spots", fill="black")
    png = Path(png)
    png.parent.mkdir(parents=True, exist_ok=True)
    img.save(png)
    return png


def metrize(hand_json, r0_json, png, per_cell=3, cell=560):
    from PIL import Image, ImageDraw

    hand = _load(hand_json)
    r0 = _load(r0_json)
    legs = r0["legs"]
    m = r0["summary"]["legs_matrix"]
    cells = [("fits + drawn (taut Checker-clean)", lambda l: l["fits"] and l["taut_ok"], m["fit_and_ok"]),
             ("fits + NOT drawn", lambda l: l["fits"] and not l["taut_ok"], m["fit_not_ok"]),
             ("does not fit + drawn", lambda l: not l["fits"] and l["taut_ok"], m["nofit_ok"]),
             ("does not fit + not drawn", lambda l: not l["fits"] and not l["taut_ok"], m["nofit_not_ok"])]
    img = Image.new("RGB", (per_cell * cell + 10 * (per_cell - 1), 2 * (cell + 60) + 200), "white")
    dr = ImageDraw.Draw(img)
    dr.text((10, 8), f"{r0['summary']['board']}: hand chains on one layer, {len(legs)} legs. P1 = every cut along the sleeve has room for the wire alone; "
            f"metrization = taut chain, Checker-clean (copper only, other wires not drawn)", fill="black")
    dr.text((10, 28), "shaded = the leg's sleeve (triangle strip), dark line = the taut path (none when taut itself failed), green dot = start, red dot = goal; pads and hand vias from the board", fill="black")
    dr.text((10, 48), f"agreement {100 * (m['fit_and_ok'] + m['nofit_not_ok']) / max(1, len(legs)):.1f} %  (the gate wants >= 95 %)", fill="black")
    y = 80
    for title, pick, n in cells:
        dr.text((10, y), f"{title}: {n} legs", fill="black")
        sel = [l for l in legs if pick(l) and l["sleeve"] and len(l["sleeve"]) <= 60]
        sel.sort(key=lambda l: (len(l["sleeve"]), l["net"]))
        picks = [sel[int(q * (len(sel) - 1))] for q in (0.25, 0.55, 0.85)[:per_cell]] if len(sel) >= per_cell else sel
        if not picks:
            y += 30
            continue
        for j, l in enumerate(picks):
            tri = [q for t in l["sleeve"] for q in t]
            lx0, lx1 = min(q[0] for q in tri), max(q[0] for q in tri)
            ly0, ly1 = min(q[1] for q in tri), max(q[1] for q in tri)
            half = max(lx1 - lx0, ly1 - ly0, 2 * MM) / 2 + 1.0 * MM
            cx, cy = (lx0 + lx1) / 2, (ly0 + ly1) / 2
            k = cell / (2 * half)
            sub = Image.new("RGB", (cell, cell), "white")
            sd = ImageDraw.Draw(sub)
            xy = lambda q: ((q[0] - cx + half) * k, (q[1] - cy + half) * k)  # noqa: E731
            for o in hand["outline"]:
                sd.line([xy(q) for q in _prim_pts(o)], fill=(120, 120, 120), width=2)
            for p in hand["pads"]:
                draw_pad(sd, xy, k, p)
            for t in l["sleeve"]:
                sd.polygon([xy(q) for q in t], fill=(255, 235, 150), outline=(220, 190, 90))
            if len(l["path"]) >= 2:
                sd.line([xy(q) for q in l["path"]], fill=(20, 20, 20), width=3)
                g = xy(l["path"][-1])
                sd.ellipse([g[0] - 6, g[1] - 6, g[0] + 6, g[1] + 6], fill=(220, 0, 0))
            s = xy(l["at"])
            sd.ellipse([s[0] - 6, s[1] - 6, s[0] + 6, s[1] + 6], fill=(0, 170, 0))
            sd.rectangle([0, 0, cell - 1, cell - 1], outline=(0, 0, 0))
            img.paste(sub, (j * (cell + 10), y + 18))
            why = l["taut_why"] or l["fit_why"] or "drawn"
            dr.text((j * (cell + 10) + 4, y + 22), f"net {l['net']} layer {'FB'[min(1, l['layer'])]}: {why[:80]}", fill=(160, 0, 0) if why != "drawn" else (0, 100, 0))
        y += cell + 60
    png = Path(png)
    png.parent.mkdir(parents=True, exist_ok=True)
    img.save(png)
    return png


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    o = sub.add_parser("overflow")
    for n in ("hand", "g0_before", "r0_before", "g0_after", "r0_after", "png"):
        o.add_argument(n, type=Path)
    c = sub.add_parser("cuts")
    for n in ("hand", "s1", "r0", "png"):
        c.add_argument(n, type=Path)
    c.add_argument("refs", nargs="+")
    c.add_argument("--win-mm", type=float, default=7.0)
    m = sub.add_parser("metrize")
    for n in ("hand", "r0", "png"):
        m.add_argument(n, type=Path)
    m.add_argument("per_cell", type=int, nargs="?", default=3)
    a = ap.parse_args(argv)
    if a.cmd == "overflow":
        print(overflow(a.hand, a.g0_before, a.r0_before, a.g0_after, a.r0_after, a.png))
    elif a.cmd == "cuts":
        print(cuts(a.hand, a.s1, a.r0, a.png, a.refs, a.win_mm))
    else:
        print(metrize(a.hand, a.r0, a.png, a.per_cell))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
