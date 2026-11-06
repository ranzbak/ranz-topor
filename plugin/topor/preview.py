"""Result preview (M4-R6, 31 wireframe section 4): ``kicad-cli pcb export svg`` of the current board and of the
routed copy, rendered with ``wx.svg`` and composed side by side with caption lines in a ``wx.MemoryDC``; the
new copper is drawn on top of the routed export with a halo (the one hard-coded colour,
``PREVIEW_HIGHLIGHT_RGB``).

SVG command (kicad-cli 10.0.8, verified with ``kicad-cli pcb export svg --help``)::

    kicad-cli pcb export svg --layers F.Cu,B.Cu,Edge.Cuts --exclude-drawing-sheet --page-size-mode 2
                             --mode-single -o <out.svg> <board.kicad_pcb>

``--page-size-mode 2`` = board area only: the viewBox is in mm with its origin at the minimum corner of the
Edge.Cuts centre lines (checked on eprom: pad at 187.5/71.0 mm, outline min 184/62 mm -> svg 3.5/9.0 mm), which is
what lets the overlay place the routed items without a second export. The overlay is skipped (with a note) when
the SVG size disagrees with the model outline by more than 0.3 mm.

Highlight: new tracks and vias are appended to the routed SVG as a translucent halo (3 x width, tint) with the
copper drawn over it in the layer's default KiCad colour. Existing copper is not dimmed (it would need a
re-styled export); recorded in the report notes.

Pure functions ``arcs_to_segments`` / ``contrast`` / ``overlay_svg`` need no wx; ``render`` needs a ``wx.App``
(the caller's; never creates a window). Complexity: ``arcs_to_segments`` O(items + output segments);
``overlay_svg`` O(items); ``render`` is dominated by three kicad-cli runs (two exports) and the rasterisation,
O(pixels).
"""

from __future__ import annotations

import math
import re
from pathlib import Path

from toporlib import geom
from toporlib.kicad_cli import kicad_cli, run_cli

PREVIEW_HIGHLIGHT_RGB = (0, 139, 139)
#: open connections (KiCad DRC unconnected items after the zone refill), as the harness PNGs draw them
RATSNEST_RGB = (0, 80, 255)
PREVIEW_HIGHLIGHT_FALLBACK_RGB = (208, 64, 208)
MIN_CONTRAST = 3.0  # WCAG graphics

#: KiCad default theme colours of the copper layers (engine layer id -> RGB), for the overlay core only.
_LAYER_RGB = {0: (200, 52, 52), 2: (77, 127, 196)}
_INNER_RGB = (127, 200, 127)

SVG_LAYERS = ("F.Cu", "B.Cu", "Edge.Cuts")
MM = 1_000_000


# ---------------------------------------------------------------- geometry (pure)
def arcs_to_segments(items: list, max_error: int) -> list:
    """Replace every arc item by straight segment items (chord error <= `max_error` nm), keeping all other
    fields (net, layer, width, group, wire, ...); segments and vias pass through unchanged, order kept.
    O(items + output segments)."""
    out: list = []
    for it in items:
        p = it.get("prim")
        if not p or p.get("kind") != "arc":
            out.append(it)
            continue
        pts = geom.flatten_arc(p["start"], p["mid"], p["end"], max(1, int(max_error)))
        for a, b in zip(pts, pts[1:]):
            if a == b:
                continue
            out.append({**it, "prim": {"kind": "seg", "a": list(a), "b": list(b)}})
    return out


def _lin(c: float) -> float:
    c /= 255.0
    return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4


def luminance(rgb) -> float:
    r, g, b = (_lin(c) for c in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast(a, b) -> float:
    """WCAG contrast ratio of two RGB triples. O(1)."""
    la, lb = luminance(a), luminance(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


def pick_tint(background) -> tuple:
    """The highlight colour: primary unless it is below 3:1 against `background`, then the fallback."""
    if contrast(PREVIEW_HIGHLIGHT_RGB, background) >= MIN_CONTRAST:
        return PREVIEW_HIGHLIGHT_RGB
    return PREVIEW_HIGHLIGHT_FALLBACK_RGB


def outline_origin(outline: list) -> tuple | None:
    """(xmin, ymin, xmax, ymax) nm of the Edge.Cuts prims of the model (arcs flattened), or None. O(prims)."""
    xs, ys = [], []
    for p in outline or []:
        if p.get("kind") == "arc":
            pts = geom.flatten_arc(p["start"], p["mid"], p["end"], 1000)
        elif p.get("kind") == "seg":
            pts = [p["a"], p["b"]]
        else:
            continue
        xs += [q[0] for q in pts]
        ys += [q[1] for q in pts]
    return (min(xs), min(ys), max(xs), max(ys)) if xs else None


def _hex(rgb) -> str:
    return "#%02X%02X%02X" % tuple(rgb)


def overlay_svg(svg_text: str, items: list, origin_nm: tuple, tint, *, layer_rgb: dict | None = None) -> str:
    """Append the new copper to `svg_text` (the board-area export): halo + layer-coloured core per item,
    coordinates ``(nm - origin) / 1e6`` mm. O(items)."""
    ox, oy = origin_nm
    col = {**_LAYER_RGB, **(layer_rgb or {})}

    def mm(p):
        return f"{(p[0] - ox) / MM:.4f},{(p[1] - oy) / MM:.4f}"

    halo, core = [], []
    for it in items:
        if it.get("kind") == "via":
            x, y = (it["at"][0] - ox) / MM, (it["at"][1] - oy) / MM
            r = it["d"] / 2.0 / MM
            halo.append(f'<circle cx="{x:.4f}" cy="{y:.4f}" r="{r * 1.8:.4f}" />')
            core.append(f'<circle cx="{x:.4f}" cy="{y:.4f}" r="{r:.4f}" fill="#E0E0E0" />')
            continue
        p = it["prim"]
        pts = (geom.flatten_arc(p["start"], p["mid"], p["end"], 2000) if p["kind"] == "arc" else [p["a"], p["b"]])
        d = " ".join(mm(q) for q in pts)
        w = it["width"] / MM
        rgb = col.get(int(it["layer"]), _INNER_RGB)
        halo.append(f'<polyline points="{d}" stroke-width="{3 * w:.4f}" />')
        core.append(f'<polyline points="{d}" stroke="{_hex(rgb)}" stroke-width="{w:.4f}" />')
    t = _hex(tint)
    extra = (f'<g fill="none" stroke="{t}" stroke-opacity="0.55" stroke-linecap="round" stroke-linejoin="round" '
             f'style="fill:{t};fill-opacity:0.55">' + "".join(halo) + "</g>\n"
             '<g fill="none" stroke-opacity="1" stroke-linecap="round" stroke-linejoin="round">' + "".join(core)
             + "</g>\n")
    i = svg_text.rfind("</svg>")
    return svg_text[:i] + extra + svg_text[i:] if i >= 0 else svg_text


def ratsnest_svg(svg_text: str, unconnected: list, origin_nm: tuple, *, width_mm: float = 0.25) -> str:
    """Append KiCad's DRC ``unconnected_items`` (each: two item anchors ``items[i].pos`` in mm, KiCad frame) as
    blue lines with a dot at each end, the open ratsnest after the zone refill (K1: the report's preview shows
    what is still open, as the harness compare PNGs do). The anchors are KiCad's item positions (pad centre,
    track start), so a line says which items are apart, not the exact gap. O(U)."""
    from toporlib.drc import ratsnest_marks  # noqa: PLC0415

    ox, oy = origin_nm[0] / MM, origin_nm[1] / MM
    mk = ratsnest_marks(unconnected)
    parts = []
    for x0, y0, x1, y1 in mk["lines"]:
        a, b = (x0 - ox, y0 - oy), (x1 - ox, y1 - oy)
        parts.append(f'<line x1="{a[0]:.4f}" y1="{a[1]:.4f}" x2="{b[0]:.4f}" y2="{b[1]:.4f}" />')
        parts += [f'<circle cx="{x:.4f}" cy="{y:.4f}" r="{1.6 * width_mm:.4f}" style="fill:{_hex(RATSNEST_RGB)}" />'
                  for x, y in (a, b)]
    # a zone's pos is its outline anchor, not the break: the other end gets a ring ("not joined to its pour"); a
    # zone <-> zone pair (a pour piece cut off) is only counted in the legend
    parts += [f'<circle cx="{x - ox:.4f}" cy="{y - oy:.4f}" r="{4 * width_mm:.4f}" />' for x, y, _ in mk["rings"]]
    if not parts:
        return svg_text
    extra = (f'<g fill="none" stroke="{_hex(RATSNEST_RGB)}" stroke-width="{width_mm:.4f}" stroke-linecap="round">'
             + "".join(parts) + "</g>\n")
    i = svg_text.rfind("</svg>")
    return svg_text[:i] + extra + svg_text[i:] if i >= 0 else svg_text


# ---------------------------------------------------------------- kicad-cli
def export_svg(pcb: Path, out_svg: Path, layers=SVG_LAYERS, *, timeout_s: float | None = None,
               on_poll=None) -> Path:
    """Board-area SVG of `pcb` (command in the module doc). Raises RuntimeError on failure; `timeout_s` / `on_poll` as in
    :func:`toporlib.kicad_cli.run_cli` (KicadCliTimeout / KicadCliCancelled). O(kicad-cli)."""
    cli = kicad_cli()
    if not Path(cli).is_file():
        raise RuntimeError(f"kicad-cli not found: {cli}")
    out_svg = Path(out_svg)
    out_svg.parent.mkdir(parents=True, exist_ok=True)
    cmd = [str(cli), "pcb", "export", "svg", "--layers", ",".join(layers), "--exclude-drawing-sheet",
           "--page-size-mode", "2", "--mode-single", "-o", str(out_svg), str(pcb)]
    proc = run_cli(cmd, timeout_s=timeout_s, on_poll=on_poll)
    if proc.returncode != 0 or not out_svg.is_file():
        raise RuntimeError(f"kicad-cli svg export failed (exit {proc.returncode}) on {pcb}: {proc.stderr.strip()}")
    return out_svg


_FILL_GROUP = re.compile(r'<g style="fill:#([0-9A-Fa-f]{6}); fill-opacity:1\.0000; stroke:none;">')


def layer_colours(svg_text: str, copper_ids=(0, 2)) -> dict:
    """Copper layer colours of the export (theme-independent): the distinct opaque fill groups appear in the
    order of the exported layers, so the first is F.Cu, the second B.Cu (white = drill holes). {} when the
    export has fewer groups. O(len(svg))."""
    seen: list = []
    for m in _FILL_GROUP.finditer(svg_text):
        c = tuple(int(m.group(1)[i:i + 2], 16) for i in (0, 2, 4))
        if c != (255, 255, 255) and c not in seen:
            seen.append(c)
    return {lid: c for lid, c in zip(copper_ids, seen)}


_SVG_SIZE = re.compile(r'width="([0-9.]+)mm"\s+height="([0-9.]+)mm"')


def _svg_mm(text: str) -> tuple | None:
    m = _SVG_SIZE.search(text)
    return (float(m.group(1)), float(m.group(2))) if m else None


# ---------------------------------------------------------------- render
def _rasterise(svg: Path, scale: float):
    import wx.svg

    img = wx.svg.SVGimage.CreateFromFile(str(svg))
    w, h = max(1, int(round(img.width * scale))), max(1, int(round(img.height * scale)))
    return img.ConvertToBitmap(scale=scale, width=w, height=h), img


def render(before_pcb: Path, after_pcb: Path, out_png: Path, caption, *, new_items: list | None = None,
           outline: list | None = None, layer_names: list | None = None, panel_px: tuple = (700, 760),
           timeout_s: float | None = None, on_poll=None) -> Path:
    """Compose the preview PNG; see :func:`render_ex` (this returns only the path)."""
    return render_ex(before_pcb, after_pcb, out_png, caption, new_items=new_items, outline=outline,
                     layer_names=layer_names, panel_px=panel_px, timeout_s=timeout_s, on_poll=on_poll)[0]


def render_ex(before_pcb: Path, after_pcb: Path, out_png: Path, caption, *, new_items: list | None = None,
              outline: list | None = None, layer_names: list | None = None,
              panel_px: tuple = (700, 760), timeout_s: float | None = None, on_poll=None,
              unconnected_before: list | None = None, unconnected_after: list | None = None) -> tuple:
    """Export both boards, draw them side by side with `caption` (str or list of lines) above, the panel titles
    ``Current board`` / ``Result`` and a legend below; write `out_png`. Returns ``(out_png, notes)``; `notes`
    are report lines about what the render could not do (e.g. highlight skipped). `unconnected_before` /
    `unconnected_after`: KiCad DRC unconnected items of the two boards, drawn blue (:func:`ratsnest_svg`). Needs a
    ``wx.App``."""
    import wx
    import wx.svg

    if wx.GetApp() is None:
        raise RuntimeError("preview.render needs a wx.App (the caller creates it; no window is shown)")
    out_png = Path(out_png)
    work = out_png.parent / "preview-work"
    work.mkdir(parents=True, exist_ok=True)
    layers = tuple(layer_names or ()) + ("Edge.Cuts",) if layer_names else SVG_LAYERS
    notes: list = []
    svg_b = export_svg(before_pcb, work / "before.svg", layers, timeout_s=timeout_s, on_poll=on_poll)
    svg_a = export_svg(after_pcb, work / "after.svg", layers, timeout_s=timeout_s, on_poll=on_poll)

    bg = wx.SystemSettings.GetColour(wx.SYS_COLOUR_WINDOW)
    fg = wx.SystemSettings.GetColour(wx.SYS_COLOUR_WINDOWTEXT)
    tint = pick_tint((bg.Red(), bg.Green(), bg.Blue()))
    svg_a_draw = svg_a
    text = svg_a.read_text()
    lay_rgb = layer_colours(text)
    if new_items:
        org, size = outline_origin(outline), _svg_mm(text)
        if org is None or size is None:
            notes.append("highlight skipped: no board outline / SVG size")
        elif abs(size[0] - (org[2] - org[0]) / MM) > 0.3 or abs(size[1] - (org[3] - org[1]) / MM) > 0.3:
            notes.append("highlight skipped: SVG size does not match the model outline")
        else:
            svg_a_draw = work / "after-highlight.svg"
            svg_a_draw.write_text(overlay_svg(text, new_items, (org[0], org[1]), tint, layer_rgb=lay_rgb))
    notes.append("existing copper is not dimmed in the Result panel (needs a re-styled export)")
    org = outline_origin(outline)
    if org is not None and (unconnected_before or unconnected_after):
        for name, unc in (("before", unconnected_before), ("after", unconnected_after)):
            if not unc:
                continue
            src = svg_b if name == "before" else svg_a_draw
            dst = work / f"{name}-ratsnest.svg"
            dst.write_text(ratsnest_svg(Path(src).read_text(), unc, (org[0], org[1])))
            if name == "before":
                svg_b = dst
            else:
                svg_a_draw = dst

    probe = wx.svg.SVGimage.CreateFromFile(str(svg_b))
    pw, ph = float(probe.width) or 1.0, float(probe.height) or 1.0
    scale = min(panel_px[0] / pw, panel_px[1] / ph)
    scale = max(0.5, scale)
    bmp_b, _ = _rasterise(svg_b, scale)
    bmp_a, _ = _rasterise(svg_a_draw, scale)

    lines = [caption] if isinstance(caption, str) else list(caption)
    m, gap, lh = 16, 16, 20
    pw_px, ph_px = bmp_b.GetWidth(), bmp_b.GetHeight()
    head_h = lh * (len(lines) + 1) + 4
    total_w = max(2 * pw_px + gap + 2 * m, 480)
    total_h = m + head_h + ph_px + lh * 2 + m
    canvas = wx.Bitmap(total_w, total_h)
    dc = wx.MemoryDC(canvas)
    dc.SetBackground(wx.Brush(bg))
    dc.Clear()
    font = wx.Font(wx.FontInfo(10).Family(wx.FONTFAMILY_SWISS))
    bold = wx.Font(wx.FontInfo(10).Family(wx.FONTFAMILY_SWISS).Bold())
    dc.SetTextForeground(fg)
    dc.SetFont(font)
    y = m
    for ln in lines:
        dc.DrawText(str(ln), m, y)
        y += lh
    y += 4
    x0 = m + (total_w - 2 * m - (2 * pw_px + gap)) // 2
    dc.SetFont(bold)
    dc.DrawText("Current board", x0, y)
    dc.DrawText("Result", x0 + pw_px + gap, y)
    y += lh
    for i, bmp in enumerate((bmp_b, bmp_a)):
        px = x0 + i * (pw_px + gap)
        dc.DrawBitmap(bmp, px, y, True)
        dc.SetPen(wx.Pen(wx.SystemSettings.GetColour(wx.SYS_COLOUR_GRAYTEXT), 1))
        dc.SetBrush(wx.TRANSPARENT_BRUSH)
        dc.DrawRectangle(px - 1, y - 1, pw_px + 2, ph_px + 2)
    y += ph_px + 6
    dc.SetFont(font)
    dc.SetTextForeground(fg)
    x = m
    dc.DrawText("Legend:", x, y)
    x += dc.GetTextExtent("Legend:")[0] + 10
    legend = [("F.Cu", lay_rgb.get(0)), ("B.Cu", lay_rgb.get(2)), ("new copper (highlighted)", tint)]
    if unconnected_before or unconnected_after:
        from toporlib.drc import ratsnest_caption, ratsnest_marks  # noqa: PLC0415
        legend.append((f"open connection (KiCad DRC unconnected: {len(unconnected_before or [])} before, "
                       f"{len(unconnected_after or [])} after"
                       f"{ratsnest_caption(ratsnest_marks(unconnected_after))})", RATSNEST_RGB))
    for label, rgb in legend:
        if rgb is not None:
            dc.SetBrush(wx.Brush(wx.Colour(*rgb)))
            dc.SetPen(wx.Pen(wx.SystemSettings.GetColour(wx.SYS_COLOUR_GRAYTEXT), 1))
            dc.DrawRectangle(x, y + 3, 12, 12)
            x += 18
        dc.DrawText(label, x, y)
        x += dc.GetTextExtent(label)[0] + 16
    dc.SelectObject(wx.NullBitmap)
    if not canvas.SaveFile(str(out_png), wx.BITMAP_TYPE_PNG):
        raise RuntimeError(f"could not write {out_png}")
    return out_png, notes
