"""stitch-vias PNG: the routed board (tracks, pads, vias from `harness.render`) over KiCad's own refilled pours
(F.Cu red, B.Cu blue, translucent, even-odd so holes stay open), with a label per stitched piece (class, layer,
area, vias placed of wanted) taken from the run's notes, and per pattern piece the via count inside its box.

    python -m tools.stitchvias_png --out OUT.png \\
        --panel TITLE REFILLED.kicad_pcb [RESULT.json|-] [PIECES.json|-] [--panel ...]

REFILLED = the measured copy kicad-cli refilled and saved (tools/stitchvias_measure.py `<out>-work/`);
RESULT = the run's report result.json (route.warnings carry the stitch notes); PIECES = a pattern's
expected.json ({"expect": {"pieces": {name: [x0, y0, x1, y1] mm}}, "marks": [...]}). Two panels are put side
by side. O(fill points + notes).
"""
from __future__ import annotations

import argparse
import json
import re
import tempfile
from pathlib import Path

import harness  # noqa: F401  (KiCad isolated before pcbnew)
from harness import render
from PIL import Image, ImageChops, ImageDraw, ImageFont

FILL_RGBA = {"F.Cu": (215, 60, 60, 70), "B.Cu": (60, 100, 215, 70)}
LABEL_BG = (255, 255, 230, 225)
_XY = r"\((-?[\d.]+), (-?[\d.]+)\)"


def _font(px: int):
    f = render.text_font()
    return ImageFont.truetype(f, px) if f else ImageFont.load_default()


def piece_labels(notes: list[str]) -> list[tuple[tuple[float, float], str]]:
    """((x, y) mm KiCad frame, text) per stitched piece of the run's notes: class 2 / 3 lines, and class 1 vias
    grouped by the piece they were placed for."""
    out = []
    cut: dict[str, list] = {}
    for n in notes:
        m = re.match(r"stitch \((dropped island|thin neck < [\d.]+ mm)\): \S+ on (\S+) (?:core )?([\d.]+) mm²(?: near " + _XY
                     + r")?: (\d+)(?: of (\d+))? via\(s\)(.*)$", n)
        if m:
            kind = "dropped" if m.group(1).startswith("dropped") else "thin"
            vias = re.findall(_XY, m.group(8) or "")
            at = (float(m.group(4)), float(m.group(5))) if m.group(4) else (
                (float(vias[0][0]), float(vias[0][1])) if vias else None)
            if at is None:
                continue
            want = f"/{m.group(7)}" if m.group(7) else ""
            extra = " mask" if "solder-mask" in (m.group(8) or "") else ""
            out.append((at, f"{kind} {m.group(2)} {float(m.group(3)):.0f} mm²: {m.group(6)}{want} vias{extra}"))
            continue
        m = re.match(r"stitch via [\d.]+ mm at " + _XY + r" for (.*)$", n)
        if m:
            cut.setdefault(m.group(3), []).append((float(m.group(1)), float(m.group(2))))
    for text, pts in cut.items():
        a = re.search(r"([\d.]+) mm²", text)
        out.append((pts[0], f"cut {a.group(1) if a else '?'} mm²: {len(pts)} vias"))
    return out


def stitch_points(notes: list[str]) -> tuple[list[tuple[float, float]], set]:
    """The stitching via positions of the run's notes (KiCad frame, mm; class 1 "stitch via" lines and the region
    lines' positions) and the set of those the engine placed at piece extremities (round 5 "extremities at")."""
    pts, corners = [], set()
    for n in notes:
        m = re.match(r"stitch via [\d.]+ mm at " + _XY, n)
        if m:
            pts.append((float(m.group(1)), float(m.group(2))))
            continue
        m = re.match(r"stitch \((dropped island|thin neck < [\d.]+ mm)\): .*?: \d+(?: of \d+)? via\(s\)(.*)$", n)
        if m:
            pts += [(float(x), float(y)) for x, y in re.findall(_XY, m.group(2))]
            continue
        m = re.search(r"extremities at(.*?); nudged at", n)
        if m:
            corners |= {(round(float(x), 3), round(float(y), 3)) for x, y in re.findall(_XY, m.group(1))}
    return pts, corners


def on_lattice(p, lat) -> bool:
    """`lat` = (px, py, shift, ox, oy) mm: rows at oy + j·py, odd rows shifted by `shift`."""
    px, py, sh, ox, oy = lat
    j = round((p[1] - oy) / py)
    if abs(p[1] - oy - j * py) > 1e-3:
        return False
    x0 = ox + (sh if j % 2 else 0.0)
    return abs((p[0] - x0) / px - round((p[0] - x0) / px)) * px < 1e-3


def _vias_mm(pcb_text: str) -> list[tuple[float, float]]:
    return [(float(x), float(y)) for x, y in re.findall(r"\(via\b[^()]*(?:\([^()]*\)[^()]*)*?\(at (-?[\d.]+) (-?[\d.]+)\)", pcb_text)]


def panel(title: str, refilled: Path, result: Path | None, pieces: Path | None, out: Path, *, height: int = 1100,
          lattice=None) -> Path:
    from toporlib import pcbfile
    with tempfile.TemporaryDirectory(prefix="stitchpng-") as tmp:
        cu = Path(tmp) / "cu.png"
        render.render_png(refilled, cu, height=height)
        top = Image.open(cu).convert("RGBA")
    w, h = top.size
    x0, y0, x1, y1 = render.board_bbox_nm(refilled)
    sx, sy = w / (x1 - x0), h / (y1 - y0)

    def px(x_nm, y_nm):
        return ((x_nm - x0) * sx, (y_nm - y0) * sy)

    text = refilled.read_text()
    canvas = Image.new("RGBA", (w, h), (255, 255, 255, 255))
    fills = pcbfile.parse_zone_fills(text)
    for layer, rgba in FILL_RGBA.items():
        mask = Image.new("1", (w, h), 0)
        for polys in fills.values():
            for p in polys:
                if p["layer"] != layer:
                    continue
                ring = Image.new("1", (w, h), 0)
                ImageDraw.Draw(ring).polygon([px(x, y) for x, y in p["pts"]], fill=1)
                mask = ImageChops.logical_xor(mask, ring)
        layer_img = Image.new("RGBA", (w, h), rgba)
        canvas.alpha_composite(Image.composite(layer_img, Image.new("RGBA", (w, h), (0, 0, 0, 0)), mask))
    canvas.alpha_composite(top)
    d = ImageDraw.Draw(canvas)
    font = _font(max(11, h // 75))
    labels: list[tuple[tuple[float, float], str]] = []
    if result is not None:
        notes = json.loads(result.read_text())["route"]["warnings"]
        labels += piece_labels(notes)
    if pieces is not None:
        exp = json.loads(pieces.read_text())
        vias = _vias_mm(text)
        for name, (bx0, by0, bx1, by1) in exp["expect"]["pieces"].items():
            n = sum(1 for vx, vy in vias if bx0 < vx < bx1 and by0 < vy < by1)
            mark = next((m for m in exp.get("marks", []) if m["text"].startswith(name + " ")), None)
            at = tuple(mark["xy"]) if mark else (bx0, by0)
            labels.append((at, f"{mark['text'] if mark else name}: {n} vias"))
    if lattice is not None:
        # round 5: the board lattice as faint crosses, the stitching vias ringed by kind
        lpx, lpy, lsh, lox, loy = lattice
        k = max(3, h // 300)
        j0, j1 = int((y0 / 1e6 - loy) // lpy) - 1, int((y1 / 1e6 - loy) // lpy) + 1
        for j in range(j0, j1 + 1):
            xs = lox + (lsh if j % 2 else 0.0)
            i0, i1 = int((x0 / 1e6 - xs) // lpx) - 1, int((x1 / 1e6 - xs) // lpx) + 1
            for i in range(i0, i1 + 1):
                cx, cy = px((xs + i * lpx) * 1e6, (loy + j * lpy) * 1e6)
                d.line([cx - k, cy, cx + k, cy], fill=(110, 110, 110, 255), width=1)
                d.line([cx, cy - k, cx, cy + k], fill=(110, 110, 110, 255), width=1)
        if result is not None:
            pts, corners = stitch_points(json.loads(result.read_text())["route"]["warnings"])
            r = max(6, h // 110)
            for p in pts:
                kind = "corner" if (round(p[0], 3), round(p[1], 3)) in corners else ("lattice" if on_lattice(p, lattice) else "other")
                col = {"lattice": (20, 150, 40, 255), "corner": (240, 140, 0, 255), "other": (220, 0, 0, 255)}[kind]
                cx, cy = px(p[0] * 1e6, p[1] * 1e6)
                d.ellipse([cx - r, cy - r, cx + r, cy + r], outline=col, width=3)
            n = {kk: sum(1 for p in pts if ("corner" if (round(p[0], 3), round(p[1], 3)) in corners else
                                            ("lattice" if on_lattice(p, lattice) else "other")) == kk)
                 for kk in ("lattice", "corner", "other")}
            leg = (f"stitch vias: green = on the board lattice ({n['lattice']}), orange = piece corner/end ({n['corner']}), "
                   f"red = elsewhere ({n['other']}); + = lattice {lpx:g} x {lpy:g} mm")
            bb = d.textbbox((8, 8), leg, font=font)
            d.rectangle([bb[0] - 3, bb[1] - 2, bb[2] + 3, bb[3] + 2], fill=LABEL_BG, outline=(90, 90, 90, 255))
            d.text((8, 8), leg, fill=(0, 0, 0, 255), font=font)
    for (mx, my), s in labels:
        cx, cy = px(mx * 1e6, my * 1e6)
        bb = d.textbbox((cx + 6, cy - 6), s, font=font)
        d.rectangle([bb[0] - 2, bb[1] - 1, bb[2] + 2, bb[3] + 1], fill=LABEL_BG, outline=(90, 90, 90, 255))
        d.text((cx + 6, cy - 6), s, fill=(0, 0, 0, 255), font=font)
        d.ellipse([cx - 4, cy - 4, cx + 4, cy + 4], outline=(0, 0, 0, 255), width=2)
    raw = out.with_name(out.stem + "-raw.png")
    canvas.convert("RGB").save(raw)
    return render.caption(raw, title, out)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--panel", nargs="+", action="append", required=True,
                    metavar="ARG", help="TITLE REFILLED [RESULT|-] [PIECES|-] [LATTICE px,py,shift,ox,oy mm|-]")
    ap.add_argument("--height", type=int, default=1100)
    a = ap.parse_args(argv)
    pngs = []
    for i, p in enumerate(a.panel):
        title, refilled = p[0], Path(p[1])
        result = Path(p[2]) if len(p) > 2 and p[2] != "-" else None
        pieces = Path(p[3]) if len(p) > 3 and p[3] != "-" else None
        lat = tuple(float(v) for v in p[4].split(",")) if len(p) > 4 and p[4] != "-" else None
        pngs.append(panel(title, refilled, result, pieces, a.out.with_name(f"{a.out.stem}-p{i}.png"), height=a.height,
                          lattice=lat))
    if len(pngs) == 1:
        Path(pngs[0]).replace(a.out)
    else:
        render.compose_compare(pngs[0], pngs[1], a.out)
    print(a.out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
