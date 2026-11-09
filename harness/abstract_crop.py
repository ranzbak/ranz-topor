"""C0: cut an abstract routing problem out of a real board (a crop) with the hand layout's copper as its reference.

    .venv/bin/python -m harness.abstract_crop <board key | .kicad_pcb copy> --name NAME
        (--window X0 Y0 X1 Y1 | --classes classes.json --ids 4,5,6 [--add X,Y ...] [--margin 2.0])
        [--out scratch/crops] [--no-png]

Never reads below ~/work/kicad/projects (refused): crops are made from the copies under scratch/boards.

Writes `<out>/<NAME>/`:
- `problem/NAME.kicad_pcb` (+ .kicad_pro / .kicad_dru copied from the source): the fixed items of the window and
  nothing routed. Footprints are never moved; a footprint keeps only its pads inside the window and a footprint
  without such pads is left out. The window grows until no pad and no via straddles its edge (a pad is never cut).
  The board outline is the window grown by the board's copper-to-edge clearance, so copper may use exactly the
  window. Copper zones and rule areas are clipped to the window (teardrop zones are dropped with the tracks).
- Every hand track that leaves the window ends in a fixed **boundary terminal**: a one-pad footprint `XT<n>` (round
  SMD pad on the track's layer, diameter = track width, the track's net) where the hand track crosses the window
  shrunk by half its width, so the pad lies inside the window and touches its edge. The nets that leave the window
  are split by KiCad's connectivity of the hand copper inside it (`NET` keeps the piece with the most pads, the
  others become `NET__crop<k>`), so pieces that the hand layout joins only outside the window are separate problems,
  as they are inside it; pour nets (a copper zone in the window) are not split. Known limit: a pour clipped to the
  window can fall into pieces that the hand layout joins outside it; the reference's DRC then lists them as
  unconnected (zone to zone), for every engine alike.
- `reference/NAME.kicad_pcb`: the same board plus the hand copper inside the window (tracks clipped at the
  terminals, vias inside): the **reference solution**. It proves the crop is solvable; it is not a target to copy.
- `crop.json`: source, requested and final window, terminals, net splits, the violations / points it was made for.
- `NAME-crop.png` (unless --no-png): problem (left) and reference (right), terminals ringed magenta, the class
  report's violations ringed yellow, the reference's own KiCad DRC in the caption. A test pattern: for user review
  before any algorithm uses it.
Complexity: O(board items) plus two kicad-cli runs (DRC of the reference, the render).
"""
from __future__ import annotations

import argparse
import json
import math
import shutil
from pathlib import Path

MM = 1_000_000
EPS_NM = 1


def _pcbnew():
    import pcbnew
    return pcbnew


# ---------------------------------------------------------------- geometry (mm or nm, plain tuples)
class Rect:
    def __init__(self, x0, y0, x1, y1):
        self.x0, self.y0, self.x1, self.y1 = min(x0, x1), min(y0, y1), max(x0, x1), max(y0, y1)

    def grown(self, d):
        return Rect(self.x0 - d, self.y0 - d, self.x1 + d, self.y1 + d)

    def contains(self, x, y, tol=0):
        return self.x0 - tol <= x <= self.x1 + tol and self.y0 - tol <= y <= self.y1 + tol

    def contains_box(self, b) -> bool:
        return self.x0 <= b.x0 and self.y0 <= b.y0 and b.x1 <= self.x1 and b.y1 <= self.y1

    def overlaps(self, b) -> bool:
        return not (b.x1 < self.x0 or b.x0 > self.x1 or b.y1 < self.y0 or b.y0 > self.y1)

    def union(self, b):
        return Rect(min(self.x0, b.x0), min(self.y0, b.y0), max(self.x1, b.x1), max(self.y1, b.y1))

    def as_list(self, scale=1.0):
        return [self.x0 / scale, self.y0 / scale, self.x1 / scale, self.y1 / scale]

    def valid(self):
        return self.x1 > self.x0 and self.y1 > self.y0


def clip_segment(a, b, r: Rect):
    """Liang-Barsky: the parameter interval (t0, t1) of a + t (b - a) inside `r`, or None."""
    dx, dy = b[0] - a[0], b[1] - a[1]
    t0, t1 = 0.0, 1.0
    for p, q in ((-dx, a[0] - r.x0), (dx, r.x1 - a[0]), (-dy, a[1] - r.y0), (dy, r.y1 - a[1])):
        if p == 0:
            if q < 0:
                return None
            continue
        t = q / p
        if p < 0:
            if t > t1:
                return None
            t0 = max(t0, t)
        else:
            if t < t0:
                return None
            t1 = min(t1, t)
    if t1 - t0 <= 1e-12:
        return None
    return t0, t1


def _lerp(a, b, t):
    return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t)


# ---------------------------------------------------------------- window
def window_from_classes(classes: Path, ids, extra=(), margin: float = 2.0) -> tuple[Rect, list[dict]]:
    """Bounding box (mm) of the class-report rows `ids` and the `extra` points, grown by `margin`."""
    rows = json.loads(Path(classes).read_text())["rows"]
    by = {r["i"]: r for r in rows}
    sel = [by[i] for i in ids]
    pts = [tuple(r["at_mm"]) for r in sel] + [tuple(p) for p in extra]
    xs, ys = [p[0] for p in pts], [p[1] for p in pts]
    return Rect(min(xs), min(ys), max(xs), max(ys)).grown(margin), sel


def _box(bb) -> Rect:
    return Rect(bb.GetLeft(), bb.GetTop(), bb.GetRight(), bb.GetBottom())


def grow_window(pads, vias, w: Rect, max_iter: int = 20) -> Rect:
    """Grow `w` (nm) until no pad box and no via box straddles its edge (overlaps it without being inside)."""
    boxes = [b for b in pads] + [b for b in vias]
    for _ in range(max_iter):
        grown = w
        for b in boxes:
            if w.overlaps(b) and not w.contains_box(b):
                grown = grown.union(b)
        if grown.as_list() == w.as_list():
            return w
        w = grown
    return w


# ---------------------------------------------------------------- the crop
def _refuse_projects(pcb: Path) -> None:
    from harness.paths import PROJECTS
    try:
        Path(pcb).resolve().relative_to(PROJECTS.resolve())
    except ValueError:
        return
    raise SystemExit(f"refused: {pcb} is under {PROJECTS} (read-only); crop a copy under scratch/boards")


def resolve_board(board: str) -> Path:
    p = Path(board)
    if p.suffix != ".kicad_pcb":
        from harness.paths import BENCH_BOARDS
        if board not in BENCH_BOARDS:
            raise SystemExit(f"not a benchmark board key or a .kicad_pcb file: {board}")
        p = Path(BENCH_BOARDS[board])
    if not p.is_file():
        raise SystemExit(f"no such board: {p}")
    _refuse_projects(p)
    return p.resolve()


def _add_terminal(pcbnew, b, n: int, net, layer: int, at, width: int):
    fp = pcbnew.FOOTPRINT(b)
    fp.SetReference(f"XT{n}")
    fp.SetValue("crop terminal")
    fp.SetPosition(pcbnew.VECTOR2I(int(round(at[0])), int(round(at[1]))))
    fp.SetLocked(True)
    fp.SetAttributes(pcbnew.FP_EXCLUDE_FROM_BOM | pcbnew.FP_EXCLUDE_FROM_POS_FILES)
    fp.Reference().SetVisible(False)
    fp.Value().SetVisible(False)
    b.Add(fp)
    p = pcbnew.PAD(fp)
    p.SetNumber("1")
    p.SetAttribute(pcbnew.PAD_ATTRIB_SMD)
    ls = pcbnew.LSET()
    ls.AddLayer(layer)
    p.SetLayerSet(ls)
    p.SetShape(pcbnew.PAD_SHAPE_CIRCLE)
    p.SetSize(pcbnew.VECTOR2I(width, width))
    p.SetPosition(pcbnew.VECTOR2I(int(round(at[0])), int(round(at[1]))))
    p.SetNet(net)
    fp.Add(p)
    return fp


def _new_track(pcbnew, b, a, c, width, layer, net):
    t = pcbnew.PCB_TRACK(b)
    t.SetStart(pcbnew.VECTOR2I(int(round(a[0])), int(round(a[1]))))
    t.SetEnd(pcbnew.VECTOR2I(int(round(c[0])), int(round(c[1]))))
    t.SetWidth(width)
    t.SetLayer(layer)
    t.SetNet(net)
    return t


def _arc_points(t, n: int = 16):
    """Polyline (nm) of an arc track: start, n - 1 inner points, end."""
    c = t.GetCenter()
    s, e = t.GetStart(), t.GetEnd()
    r = math.hypot(s.x - c.x, s.y - c.y)
    a0 = math.atan2(s.y - c.y, s.x - c.x)
    m = t.GetMid()
    a1 = math.atan2(e.y - c.y, e.x - c.x)
    am = math.atan2(m.y - c.y, m.x - c.x)
    sweep = (a1 - a0) % (2 * math.pi)
    if (am - a0) % (2 * math.pi) > sweep:
        sweep -= 2 * math.pi
    pts = [(s.x, s.y)] + [(c.x + r * math.cos(a0 + sweep * k / n), c.y + r * math.sin(a0 + sweep * k / n)) for k in range(1, n)]
    return pts + [(e.x, e.y)]


def _seg_dist(p, a, b) -> float:
    dx, dy = b[0] - a[0], b[1] - a[1]
    l2 = dx * dx + dy * dy
    t = 0.0 if l2 <= 0 else max(0.0, min(1.0, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2))
    return math.hypot(p[0] - (a[0] + t * dx), p[1] - (a[1] + t * dy))


def crop(src: Path, window_mm: Rect, out_dir: Path, name: str, *, split_nets: bool = True, marks=(), note: str = "") -> dict:
    """Build the crop of `src` (a board copy) for `window_mm`; see the module doc. Returns the crop.json dict."""
    pcbnew = _pcbnew()
    src = Path(src)
    _refuse_projects(src)
    b = pcbnew.LoadBoard(str(src))  # in memory only: the copy on disk is never written
    ds = b.GetDesignSettings()
    edge = max(0, int(ds.m_CopperEdgeClearance))
    w0 = Rect(window_mm.x0 * MM, window_mm.y0 * MM, window_mm.x1 * MM, window_mm.y1 * MM)
    pad_boxes = [_box(p.GetBoundingBox()) for p in b.GetPads()]
    via_boxes = [_box(t.GetBoundingBox()) for t in b.GetTracks() if t.GetClass() == "PCB_VIA"]
    w = grow_window(pad_boxes, via_boxes, w0)
    outline = w.grown(edge + EPS_NM)

    # -- footprints: pads inside the window only; nothing moves
    removed_pads, removed_fps, kept_fps = 0, 0, []
    # collect first, remove after (a footprint's item lists do not survive removals as Python sequences)
    plan = []
    for fp in list(b.GetFootprints()):
        gi, zs = fp.GraphicalItems(), fp.Zones()
        gr = [g for g in (gi[i].Cast() for i in range(len(gi)))
              if (g.IsOnCopperLayer() or g.GetLayer() == pcbnew.Edge_Cuts) and not w.contains_box(_box(g.GetBoundingBox()))]
        pads = [p for p in fp.Pads() if not w.contains_box(_box(p.GetBoundingBox()))]
        plan.append((fp, pads, gr, [zs[i] for i in range(len(zs))]))
    for fp, pads, gr, zs in plan:
        for p in pads:
            fp.Delete(p)
            removed_pads += 1
        for g in gr:
            fp.Delete(g)
        for z in zs:
            fp.Delete(z)
        if len(list(fp.Pads())) == 0:
            b.Delete(fp)
            removed_fps += 1
        else:
            kept_fps.append(fp.GetReference())

    # -- board drawings: copper ones inside the window, the outline replaced, the rest dropped
    for d in list(b.GetDrawings()):
        keep = d.IsOnCopperLayer() and w.contains_box(_box(d.GetBoundingBox())) if hasattr(d, "IsOnCopperLayer") else False
        if not keep:
            b.Delete(d)
    for (x0, y0), (x1, y1) in (((outline.x0, outline.y0), (outline.x1, outline.y0)), ((outline.x1, outline.y0), (outline.x1, outline.y1)),
                               ((outline.x1, outline.y1), (outline.x0, outline.y1)), ((outline.x0, outline.y1), (outline.x0, outline.y0))):
        s = pcbnew.PCB_SHAPE(b)
        s.SetShape(pcbnew.SHAPE_T_SEGMENT)
        s.SetLayer(pcbnew.Edge_Cuts)
        s.SetWidth(int(0.05 * MM))
        s.SetStart(pcbnew.VECTOR2I(int(x0), int(y0)))
        s.SetEnd(pcbnew.VECTOR2I(int(x1), int(y1)))
        b.Add(s)

    # -- zones clipped to the window (copper) / the outline (rule areas); teardrops dropped
    rect_ps = lambda r: _rect_polyset(pcbnew, r)  # noqa: E731
    pour_nets = set()
    for z in list(b.Zones()):
        if z.IsTeardropArea():
            b.Delete(z)
            continue
        ps = z.Outline()  # clipped in place (ZONE::SetOutline would take ownership of a Python-owned set)
        ps.BooleanIntersection(rect_ps(outline if z.GetIsRuleArea() else w))
        if ps.OutlineCount() == 0 or ps.Area() <= 0:
            b.Delete(z)
            continue
        z.UnFill()
        if not z.GetIsRuleArea() and z.GetNetname():
            pour_nets.add(z.GetNetname())

    # -- tracks: clipped at the window shrunk by half the width; a cut end becomes a terminal
    terminals, tracks_kept, vias_kept, arcs_split = [], 0, 0, 0
    for t in list(b.GetTracks()):
        cls = t.GetClass()
        if cls == "PCB_VIA":
            pos = t.GetPosition()
            if w.contains(pos.x, pos.y):
                vias_kept += 1
            else:
                b.Delete(t)
            continue
        width, layer, net, netname = t.GetWidth(), t.GetLayer(), t.GetNet(), t.GetNetname()
        # (pcbnew: BOARD.Delete frees the item, BOARD.Remove breaks the SWIG proxies of later lookups; read first)
        inner = w.grown(-width / 2)
        if cls == "PCB_ARC":
            pts = _arc_points(t)
            bb = Rect(min(p[0] for p in pts), min(p[1] for p in pts), max(p[0] for p in pts), max(p[1] for p in pts))
            if inner.contains_box(bb):
                tracks_kept += 1
                continue
            b.Delete(t)
            arcs_split += 1
            segs = list(zip(pts[:-1], pts[1:]))
        else:
            s, e = t.GetStart(), t.GetEnd()
            segs = [((s.x, s.y), (e.x, e.y))]
            b.Delete(t)
        for a, c in segs:
            iv = clip_segment(a, c, inner) if inner.valid() else None
            if iv is None:
                continue
            t0, t1 = iv
            pa, pc = _lerp(a, c, t0), _lerp(a, c, t1)
            nt = _new_track(pcbnew, b, pa, pc, width, layer, net)
            b.Add(nt)
            tracks_kept += 1
            for cut, p in ((t0 > 1e-9, pa), (t1 < 1 - 1e-9, pc)):
                if cut:
                    terminals.append({"net": netname, "layer": b.GetLayerName(layer), "layer_id": layer,
                                      "at_nm": [round(p[0]), round(p[1])], "width_nm": width, "netcode": net})
    # one terminal per (net, layer, point)
    uniq = {}
    for tm in terminals:
        uniq.setdefault((tm["net"], tm["layer_id"], round(tm["at_nm"][0] / 10), round(tm["at_nm"][1] / 10)), tm)
    # a cut end inside a pad of its own net needs no terminal: that pad already is where the net leaves
    pads_by = {}
    for pd in b.GetPads():
        if pd.GetNetname():
            pads_by.setdefault(pd.GetNetname(), []).append(pd)
    inside_pad = lambda tm: any(  # noqa: E731
        pd.IsOnLayer(tm["layer_id"]) and pd.GetEffectivePolygon(tm["layer_id"], pcbnew.ERROR_INSIDE).Contains(
            pcbnew.VECTOR2I(int(tm["at_nm"][0]), int(tm["at_nm"][1]))) for pd in pads_by.get(tm["net"], []))
    terminals = [tm for tm in uniq.values() if not inside_pad(tm)]
    for n, tm in enumerate(terminals, 1):
        fp = _add_terminal(pcbnew, b, n, tm["netcode"], tm["layer_id"], tm["at_nm"], tm["width_nm"])
        tm["ref"] = fp.GetReference()

    out_dir = Path(out_dir) / name
    if out_dir.exists():
        shutil.rmtree(out_dir)
    ref_pcb = out_dir / "reference" / f"{name}.kicad_pcb"
    prob_pcb = out_dir / "problem" / f"{name}.kicad_pcb"
    ref_pcb.parent.mkdir(parents=True)
    prob_pcb.parent.mkdir(parents=True)
    pcbnew.SaveBoard(str(ref_pcb), b)
    splits = _split_nets(pcbnew, ref_pcb, {tm["net"] for tm in terminals} - pour_nets) if split_nets else []
    renamed = {pad: s["new"] for s in splits for pad in s["pads"]}
    for tm in terminals:
        tm["net"] = renamed.get(f"{tm['ref']}.1", tm["net"])
    from harness.strip import strip_text
    text, _ = strip_text(ref_pcb.read_text())
    prob_pcb.write_text(text)
    for ext in (".kicad_pro", ".kicad_dru"):
        f = src.with_suffix(ext)
        if f.is_file():
            for d in (ref_pcb, prob_pcb):
                shutil.copyfile(f, d.with_suffix(ext))
    # zone fills by KiCad itself (pcbnew's ZONE_FILLER crashes on the edited board): the model export reads them
    from toporlib.drc import run_drc
    for d in (ref_pcb, prob_pcb):
        run_drc(d, d.with_name(f"{d.stem}--drc.json"), save_board=True)
    meta = {
        "name": name, "source": str(src), "note": note,
        "window_requested_mm": window_mm.as_list(), "window_mm": w.as_list(MM), "outline_mm": outline.as_list(MM),
        "edge_clearance_mm": edge / MM,
        "footprints_kept": sorted(kept_fps), "footprints_removed": removed_fps, "pads_removed": removed_pads,
        "reference": {"tracks": tracks_kept, "vias": vias_kept, "arcs_split": arcs_split},
        "terminals": [{k: v for k, v in tm.items() if k not in ("netcode", "layer_id")} | {"at_mm": [tm["at_nm"][0] / MM, tm["at_nm"][1] / MM]}
                      for tm in terminals],
        "net_splits": splits, "pour_nets": sorted(pour_nets),
        "marks": list(marks),
        "problem": str(prob_pcb), "reference_pcb": str(ref_pcb),
    }
    (out_dir / "crop.json").write_text(json.dumps(meta, indent=1))
    return meta


def _split_nets(pcbnew, pcb: Path, nets) -> list[dict]:
    """Split each of `nets` on the saved board `pcb` by KiCad's own connectivity of its pads, tracks and vias: the
    piece with the most pads keeps the name, every other piece (pads and the copper joined to them) becomes
    `NET__crop<k>`. Rewrites `pcb` when something was split. On a freshly loaded board: no item was deleted on it."""
    b = pcbnew.LoadBoard(str(pcb))
    b.BuildConnectivity()
    conn = b.GetConnectivity()
    by_net = {}
    for p in b.GetPads():
        if p.GetNetname() in nets:
            by_net.setdefault(p.GetNetname(), []).append(p)
    splits = []
    for n in sorted(by_net):
        pads, groups, seen = by_net[n], [], set()
        for p in pads:
            if p.m_Uuid.AsString() in seen:
                continue
            items = [p] + [i for i in conn.GetConnectedItems(p) if i.GetNetname() == n]
            grp = {"pads": [], "copper": []}
            for i in items:
                u = i.m_Uuid.AsString()
                if u in seen:
                    continue
                seen.add(u)
                if i.GetClass() == "PAD":
                    grp["pads"].append(i.Cast())
                elif i.GetClass() in ("PCB_TRACK", "PCB_ARC", "PCB_VIA"):
                    grp["copper"].append(i.Cast())
            groups.append(grp)
        if len(groups) < 2:
            continue
        name = lambda q: f"{q.GetParentFootprint().GetReference()}.{q.GetNumber()}"  # noqa: E731
        groups.sort(key=lambda g: (-len(g["pads"]), min(name(q) for q in g["pads"])))
        for k, g in enumerate(groups[1:], 1):
            nn = pcbnew.NETINFO_ITEM(b, f"{n}__crop{k}")
            b.Add(nn)
            for i in g["pads"] + g["copper"]:
                i.SetNet(nn)
            splits.append({"net": n, "new": f"{n}__crop{k}", "pads": sorted(name(q) for q in g["pads"]),
                           "copper": len(g["copper"])})
    if splits:
        pcbnew.SaveBoard(str(pcb), b)
    return splits


def _rect_polyset(pcbnew, r: Rect):
    ch = pcbnew.SHAPE_LINE_CHAIN()
    for x, y in ((r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)):
        ch.Append(pcbnew.VECTOR2I(int(x), int(y)))
    ch.SetClosed(True)
    return pcbnew.SHAPE_POLY_SET(ch)


# ---------------------------------------------------------------- checks and picture
def reference_drc(meta: dict, out: Path | None = None) -> dict:
    """KiCad's DRC (with zone refill) of the reference against the crop: routing violations by type, unconnected."""
    from toporlib.drc import run_drc
    from harness.cutclass import ROUTING_DRC
    ref = Path(meta["reference_pcb"])
    out = out or ref.with_name(f"{ref.stem}--drc.json")
    rep = run_drc(ref, out)
    raw = json.loads(Path(out).read_text())
    by = {}
    for v in raw.get("violations", []):
        if v.get("type") in ROUTING_DRC:
            by[v["type"]] = by.get(v["type"], 0) + 1
    return {"routing": by, "routing_total": sum(by.values()), "unconnected": len(rep.unconnected)}


def render(meta: dict, drc: dict | None, out_png: Path) -> Path:
    """Problem (left) and reference (right) side by side; terminals magenta, marks yellow."""
    from harness.render import board_bbox_nm, caption, compose_compare, draw_violations, highlight_points, render_png
    ref, prob = Path(meta["reference_pcb"]), Path(meta["problem"])
    tmp = out_png.parent / "png-parts"
    tmp.mkdir(parents=True, exist_ok=True)
    term = [[tm["at_nm"]] for tm in meta["terminals"]]
    marks = [(m["at_mm"][0], m["at_mm"][1], m.get("what", "")) for m in meta["marks"]]
    panels = []
    for label, pcb in (("problem", prob), ("reference (hand copper in the window)", ref)):
        raw = render_png(pcb, tmp / f"{label.split()[0]}-raw.png", height=900)
        bbox = board_bbox_nm(pcb)
        p = highlight_points(raw, tmp / f"{label.split()[0]}-t.png", term, bbox_nm=bbox, radius_px=8, width_px=3) if term else raw
        p = draw_violations(p, tmp / f"{label.split()[0]}-v.png", marks, bbox_nm=bbox, radius_px=11, width_px=3)
        if label == "problem":
            text = f"{meta['name']}: problem ({len(meta['terminals'])} boundary terminals magenta; yellow = {meta.get('note') or 'marks'})"
        else:
            d = "" if drc is None else f"; KiCad DRC: {drc['routing_total']} routing violations, {drc['unconnected']} unconnected"
            text = f"reference: {meta['reference']['tracks']} tracks, {meta['reference']['vias']} vias{d}"
        panels.append(caption(p, text, tmp / f"{label.split()[0]}.png"))
    compose_compare(panels[0], panels[1], out_png)
    return out_png


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("board", help="BENCH_BOARDS key or a .kicad_pcb copy (never below ~/work/kicad/projects)")
    ap.add_argument("--name", required=True)
    ap.add_argument("--window", nargs=4, type=float, metavar=("X0", "Y0", "X1", "Y1"), help="mm, KiCad frame")
    ap.add_argument("--classes", type=Path, help="a harness.cutclass classes.json")
    ap.add_argument("--ids", default="", help="comma list of class-report row ids (with --classes)")
    ap.add_argument("--add", action="append", default=[], metavar="X,Y", help="extra point (mm) for the window")
    ap.add_argument("--margin", type=float, default=2.0, help="mm around the ids / points (default 2)")
    ap.add_argument("--out", type=Path, default=None, help="default <main tree>/scratch/crops")
    ap.add_argument("--note", default="", help="what the marks are (PNG caption)")
    ap.add_argument("--no-split", action="store_true", help="keep the nets whole (no __crop<k> pieces)")
    ap.add_argument("--no-png", action="store_true")
    a = ap.parse_args(argv)
    src = resolve_board(a.board)
    extra = [tuple(float(v) for v in s.split(",")) for s in a.add]
    marks = []
    if a.classes:
        ids = [int(x) for x in a.ids.split(",") if x.strip()]
        win, rows = window_from_classes(a.classes, ids, extra, a.margin)
        marks = [{"i": r["i"], "at_mm": r["at_mm"], "what": f"{r['class']} {r['name']} {r['kind']}"} for r in rows]
    elif a.window:
        win = Rect(*a.window)
    else:
        ap.error("--window or --classes/--ids")
    marks += [{"at_mm": list(p), "what": "extra point"} for p in extra]
    if a.out is None:
        from harness.compare import MAIN
        a.out = MAIN / "scratch" / "crops"
    meta = crop(src, win, a.out, a.name, split_nets=not a.no_split, marks=marks, note=a.note)
    drc = reference_drc(meta)
    meta["reference_drc"] = drc
    if not a.no_png:
        meta["png"] = str(render(meta, drc, Path(meta["problem"]).parent.parent / f"{a.name}-crop.png"))
    (Path(meta["problem"]).parent.parent / "crop.json").write_text(json.dumps(meta, indent=1))
    print(f"crop {a.name}: window {[round(v, 3) for v in meta['window_mm']]} mm, {len(meta['footprints_kept'])} footprints, "
          f"{len(meta['terminals'])} terminals, {len(meta['net_splits'])} net splits; reference {meta['reference']}; "
          f"reference DRC {drc}")
    print(f"  problem:   {meta['problem']}\n  reference: {meta['reference_pcb']}\n  png:       {meta.get('png')}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
