"""PNG renders of a board (zone fills stripped), captions and side-by-side comparisons."""
from __future__ import annotations

import subprocess
import tempfile
from pathlib import Path

import pcbnew

from harness.paths import kicad_cli

CAPTION_PX = 60
CAPTION_FONT_PT = 28
CAPTION_LINE_PX = 36  # line pitch for multi-line captions
CAPTION_PAD_PX = 16
COMPARE_PAD_PX = 20


def strip_zone_fills(src_pcb: Path, dst_pcb: Path) -> Path:
    """Save a copy of the board with every zone unfilled; the source is never written."""
    src_pcb = Path(src_pcb)
    dst_pcb = Path(dst_pcb)
    if src_pcb.resolve() == dst_pcb.resolve():
        raise ValueError("strip_zone_fills must write to a different file than its source")
    board = pcbnew.LoadBoard(str(src_pcb))
    for zone in board.Zones():
        zone.UnFill()
    dst_pcb.parent.mkdir(parents=True, exist_ok=True)
    pcbnew.SaveBoard(str(dst_pcb), board)
    return dst_pcb


def render_png(pcb: Path, out_png: Path, *, height: int = 1100,
               layers: str = "F.Cu,B.Cu,Edge.Cuts") -> Path:
    """Board -> PNG via a zone-stripped copy, `kicad-cli pcb export svg`, then rsvg-convert."""
    out_png = Path(out_png)
    out_png.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="topor-render-") as tmp:
        stripped = strip_zone_fills(Path(pcb), Path(tmp) / "stripped.kicad_pcb")
        svg = Path(tmp) / "board.svg"
        cmd = [
            str(kicad_cli()), "pcb", "export", "svg",
            "-l", layers, "--mode-single", "--page-size-mode", "2", "--exclude-drawing-sheet",
            "-o", str(svg), str(stripped),
        ]
        proc = subprocess.run(cmd, capture_output=True, text=True)
        if proc.returncode != 0 or not svg.is_file():
            raise RuntimeError(f"kicad-cli svg export failed (exit {proc.returncode}): "
                               f"{proc.stderr.strip()}")
        subprocess.run(["rsvg-convert", "-h", str(height), str(svg), "-o", str(out_png)],
                       check=True)
    return out_png


def _png_width(png: Path) -> int:
    out = subprocess.run(["identify", "-format", "%w", str(png)], check=True,
                         capture_output=True, text=True).stdout
    return int(out.strip())


def _text_width(text: str, pointsize: int) -> int:
    out = subprocess.run(["convert", "-pointsize", str(pointsize), f"label:{text}",
                          "-format", "%w", "info:"], check=True,
                         capture_output=True, text=True).stdout
    return int(out.strip())


def caption(png: Path, text: str, out: Path, *, strip_px: int = CAPTION_PX) -> Path:
    """Add a white strip above the image carrying `text`, one line per newline-separated line.

    The strip is at least `strip_px` high and grows with the line count. The font is 28pt, shrunk
    only if the widest line would not fit the image width. Single-line captions are unchanged.
    """
    out = Path(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    lines = text.split("\n")
    strip = max(strip_px,
                CAPTION_FONT_PT + (len(lines) - 1) * CAPTION_LINE_PX + 2 * CAPTION_PAD_PX)
    pointsize = CAPTION_FONT_PT
    avail = _png_width(png) - 2 * CAPTION_PAD_PX
    width = _text_width(text.replace("%", "%%"), pointsize)
    if width > avail:
        pointsize = max(8, pointsize * avail // width)
    top = max(1, (strip - (pointsize + (len(lines) - 1) * CAPTION_LINE_PX)) // 2)
    # Each line is annotated at an explicit y so the line pitch is fixed.
    cmd = ["convert", str(png), "-background", "white", "-gravity", "North",
           "-splice", f"0x{strip}", "-fill", "black", "-pointsize", str(pointsize)]
    for i, line in enumerate(lines):
        # ImageMagick treats '%' as an escape in -annotate text.
        cmd += ["-annotate", f"+0+{top + i * CAPTION_LINE_PX}", line.replace("%", "%%")]
    subprocess.run(cmd + [str(out)], check=True)
    return out


def _png_height(png: Path) -> int:
    out = subprocess.run(["identify", "-format", "%h", str(png)], check=True,
                         capture_output=True, text=True).stdout
    return int(out.strip())


def board_bbox_nm(pcb: Path) -> tuple[int, int, int, int]:
    """(x0, y0, x1, y1) in KiCad nm of the area `render_png` fits to the page (board edges)."""
    b = pcbnew.LoadBoard(str(pcb))
    bb = b.GetBoardEdgesBoundingBox()
    if bb.GetWidth() <= 0 or bb.GetHeight() <= 0:
        bb = b.GetBoundingBox()
    return (int(bb.GetX()), int(bb.GetY()),
            int(bb.GetX() + bb.GetWidth()), int(bb.GetY() + bb.GetHeight()))


def highlight_points(png: Path, out: Path, pairs, *, bbox_nm, radius_px: int = 14,
                     color: str = "#ff00cc", width_px: int = 4) -> Path:
    """Ring every `[x, y]` (KiCad-frame nm, Y down like the SVG) of `pairs` on `png`.

    `bbox_nm` = (x0, y0, x1, y1) is the board area the image was fitted to (`render_png` uses
    `--page-size-mode 2`, so the image spans exactly that box): pixel = (nm - origin) * size /
    extent. Engine report `at` values are already in the KiCad frame (the engine's Y flip is
    internal), so no flip is applied here. Each pair's two terminals are joined by a thin line.
    """
    out = Path(out)
    x0, y0, x1, y1 = bbox_nm
    w, h = _png_width(png), _png_height(png)
    sx, sy = w / (x1 - x0), h / (y1 - y0)

    def px(at):
        return (at[0] - x0) * sx, (at[1] - y0) * sy

    cmd = ["convert", str(png), "-fill", "none", "-stroke", color, "-strokewidth", str(width_px)]
    for pts in pairs:
        pp = [px(a) for a in pts]
        for cx, cy in pp:
            cmd += ["-draw", f"circle {cx:.1f},{cy:.1f} {cx + radius_px:.1f},{cy:.1f}"]
        if len(pp) == 2:
            cmd += ["-strokewidth", "1", "-draw",
                    f"line {pp[0][0]:.1f},{pp[0][1]:.1f} {pp[1][0]:.1f},{pp[1][1]:.1f}",
                    "-strokewidth", str(width_px)]
    subprocess.run(cmd + [str(out)], check=True)
    return out


RATSNEST_COLOR = "#0050ff"


def draw_ratsnest(png: Path, out: Path, unconnected: list[dict], *, bbox_nm,
                  color: str = RATSNEST_COLOR, width_px: int = 3, dot_px: int = 6) -> Path:
    """Draw KiCad DRC `unconnected_items` (the ratsnest left after the zone refill) on `png`.

    Each entry's two item positions (`items[i].pos`, mm in the KiCad frame) are joined by a line
    of `color` with a dot at each end; `bbox_nm` maps the board to pixels as in `highlight_points`.
    The positions are KiCad's item anchors (pad centre, track start), not the nearest points, so a
    line marks which items are apart, not the exact gap. O(U) in the unconnected items.
    """
    out = Path(out)
    x0, y0, x1, y1 = bbox_nm
    w, h = _png_width(png), _png_height(png)
    sx, sy = w / (x1 - x0), h / (y1 - y0)
    cmd = ["convert", str(png), "-stroke", color, "-fill", color, "-strokewidth", str(width_px)]
    from toporlib.drc import ratsnest_marks
    mk = ratsnest_marks(unconnected)
    px = lambda x, y: ((x * 1e6 - x0) * sx, (y * 1e6 - y0) * sy)  # noqa: E731
    for xa, ya, xb, yb in mk["lines"]:
        (ax, ay), (bx, by) = px(xa, ya), px(xb, yb)
        cmd += ["-draw", f"line {ax:.1f},{ay:.1f} {bx:.1f},{by:.1f}"]
        for cx, cy in ((ax, ay), (bx, by)):
            cmd += ["-draw", f"circle {cx:.1f},{cy:.1f} {cx + dot_px:.1f},{cy:.1f}"]
    # a zone's pos is the zone outline's anchor, not the break (K1): ring at the other end; zone <-> zone pairs (pour
    # pieces cut off) have no place to draw: ratsnest_caption counts them for the caption
    for x, y, _ in mk["rings"]:
        cx, cy = px(x, y)
        cmd += ["-fill", "none", "-draw", f"circle {cx:.1f},{cy:.1f} {cx + 3 * dot_px:.1f},{cy:.1f}", "-fill", color]
    subprocess.run(cmd + [str(out)], check=True)
    return out


VIOLATION_COLOR = "#ffd400"  # yellow (user 2026-11-07: "mark the violations with yellow in the images")


def violation_marks_of(pcb: Path, violations) -> list:
    """`toporlib.drc.violation_marks` with the track geometry of `pcb` (the board the DRC checked). O(len(pcb))."""
    from toporlib.drc import violation_marks
    from toporlib.pcbfile import track_lines
    return violation_marks(violations, track_lines(Path(pcb).read_text()))


def draw_violations(png: Path, out: Path, marks, *, bbox_nm, color: str = VIOLATION_COLOR,
                    radius_px: int = 9, width_px: int = 4) -> Path:
    """Ring every DRC violation marker ``(x, y, type)`` mm (`violation_marks_of`) on `png` in yellow, with a thin dark
    edge so the ring shows on white and on copper. O(V)."""
    out = Path(out)
    if not marks:
        if Path(png) != out:
            subprocess.run(["convert", str(png), str(out)], check=True)
        return out
    x0, y0, x1, y1 = bbox_nm
    w, h = _png_width(png), _png_height(png)
    sx, sy = w / (x1 - x0), h / (y1 - y0)
    pts = [((x * 1e6 - x0) * sx, (y * 1e6 - y0) * sy) for x, y, _ in marks]
    cmd = ["convert", str(png), "-fill", "none", "-stroke", "#5a4600", "-strokewidth", str(width_px + 2)]
    cmd += [a for cx, cy in pts for a in ("-draw", f"circle {cx:.1f},{cy:.1f} {cx + radius_px:.1f},{cy:.1f}")]
    cmd += ["-stroke", color, "-strokewidth", str(width_px)]
    cmd += [a for cx, cy in pts for a in ("-draw", f"circle {cx:.1f},{cy:.1f} {cx + radius_px:.1f},{cy:.1f}")]
    subprocess.run(cmd + [str(out)], check=True)
    return out


def compose_compare(left_png: Path, right_png: Path, out_png: Path, *,
                    pad_px: int = COMPARE_PAD_PX) -> Path:
    """Left and right images side by side, each surrounded by `pad_px` of white."""
    out_png = Path(out_png)
    out_png.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["convert", "-background", "white",
         "(", str(left_png), "-bordercolor", "white", "-border", str(pad_px), ")",
         "(", str(right_png), "-bordercolor", "white", "-border", str(pad_px), ")",
         "+append", str(out_png)],
        check=True,
    )
    return out_png


def main(argv: list[str] | None = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(description="Render a KiCad board to PNG (zone fills stripped).")
    ap.add_argument("pcb", type=Path)
    ap.add_argument("png", type=Path)
    ap.add_argument("--height", type=int, default=1100)
    args = ap.parse_args(argv)
    out = render_png(args.pcb, args.png, height=args.height)
    print(f"rendered {out} ({args.height}px high)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
