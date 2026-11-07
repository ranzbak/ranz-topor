"""Minimal `.kicad_pcb` s-expression editing: find / remove / append copper items (segment, arc, via).

Pure text, no pcbnew (M4-R1). Only the top-level children of ``(kicad_pcb ...)`` are considered, so
footprint graphics are never mistaken for tracks. ``parse_layers`` reads the ``(layers ...)`` table,
``via_free_flags`` KiCad's per-via "free" flag (not in the IPC API).

Complexity: ``find_items`` is O(len(text)) (one regex pass, string-literal aware); ``remove_items`` is
O(len(text) + #removed) ; ``append_items`` is O(len(text) + #items) (a final-paren search and one
concatenation; plus O(L log L) per via without layers, L = copper layers).
"""

from __future__ import annotations

import re
import uuid as _uuid
from dataclasses import dataclass

_TOKEN = re.compile(r'"(?:[^"\\]|\\.)*"|[()]')
_WORD = re.compile(r"[A-Za-z_][A-Za-z_0-9]*")
_UUID = re.compile(r'\(uuid\s+"?([0-9A-Za-z\-]+)"?\s*\)')


@dataclass(frozen=True)
class ItemSpan:
    kind: str  # "segment" | "arc" | "via" (as written in the file)
    uuid: str
    start: int  # index of the opening "("
    end: int  # one past the closing ")"


def find_items(text: str, kinds: tuple = ("segment", "arc", "via")) -> list[ItemSpan]:
    """Top-level items of the given kinds, in file order. O(len(text))."""
    out: list[ItemSpan] = []
    depth = 0
    cur = -1
    cur_kind = ""
    for m in _TOKEN.finditer(text):
        c = m.group()
        if c == "(":
            if depth == 1:
                w = _WORD.match(text, m.end())
                if w and w.group() in kinds:
                    cur, cur_kind = m.start(), w.group()
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 1 and cur >= 0:
                u = _UUID.search(text, cur, m.end())
                out.append(ItemSpan(cur_kind, u.group(1) if u else "", cur, m.end()))
                cur = -1
    return out


_TRACK_XY = {k: re.compile(r"\(" + k + r"\s+(-?[\d.]+)\s+(-?[\d.]+)") for k in ("start", "mid", "end", "at")}


def track_lines(text: str) -> dict[str, list[tuple[float, float, float, float]]]:
    """uuid -> straight pieces ``(x0, y0, x1, y1)`` mm of every top-level segment, arc (start-mid, mid-end) and via
    (a point: x0 = x1, y0 = y1). For placing DRC markers between the items a violation names. O(len(text))."""
    out: dict[str, list[tuple[float, float, float, float]]] = {}
    for s in find_items(text, ("segment", "arc", "via")):
        body = text[s.start:s.end]
        p = {k: r.search(body) for k, r in _TRACK_XY.items()}
        xy = {k: (float(m.group(1)), float(m.group(2))) for k, m in p.items() if m}
        if s.kind == "via" and "at" in xy:
            out[s.uuid] = [(*xy["at"], *xy["at"])]
        elif "start" in xy and "end" in xy:
            pts = [xy["start"], xy["mid"], xy["end"]] if s.kind == "arc" and "mid" in xy else [xy["start"], xy["end"]]
            out[s.uuid] = [(*a, *b) for a, b in zip(pts, pts[1:])]
    return out


def remove_items(text: str, uuids) -> str:
    """Delete the top-level items (segment, arc, via, zone: e.g. KiCad's teardrop zones of ripped tracks) whose
    uuid is in `uuids`, with their indentation and line break (everything else stays byte-identical).
    O(len(text))."""
    want = set(uuids)
    if not want:
        return text
    cuts = []
    for s in find_items(text, ("segment", "arc", "via", "zone")):
        if s.uuid in want:
            a = s.start
            while a > 0 and text[a - 1] in " \t":
                a -= 1
            if a > 0 and text[a - 1] == "\n":
                a -= 1
            cuts.append((a, s.end))
    if not cuts:
        return text
    parts = []
    pos = 0
    for a, b in cuts:
        parts.append(text[pos:a])
        pos = b
    parts.append(text[pos:])
    return "".join(parts)


def _mm(nm: int) -> str:
    """Integer nm -> exact mm string, at most 6 decimals, trailing zeros trimmed."""
    nm = int(nm)
    sign = "-" if nm < 0 else ""
    q, r = divmod(abs(nm), 1_000_000)
    s = f"{sign}{q}"
    if r:
        s += "." + f"{r:06d}".rstrip("0")
    return s


def _q(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def _net(it: dict, net_names: dict | None) -> str:
    name = it.get("net_name")
    if name is None:
        code = int(it["net"])
        name = (net_names or {}).get(code)
        if name is None:
            raise KeyError(f"no net name for code {code}")
    return f"\t\t(net {_q(name)})\n"


def _top_down(lid: int) -> tuple:
    """Sort key of KiCad 10 copper layer ids in stackup order: F.Cu (0), In1.Cu (4), In2.Cu (6), ..., B.Cu (2)."""
    lid = int(lid)
    return (lid == 2, lid)


def cu_name(layer_id: int, layer_names: dict) -> str:
    """Canonical file name of a copper layer: KiCad's ids are F.Cu 0, B.Cu 2, In<k>.Cu 2k + 2 (k >= 1);
    a board may *display* another name for it (``In1.Cu-POWER``, the user name), but the file's items
    must use the canonical one. Other ids keep `layer_names`' name. O(1)."""
    if layer_id == 0:
        return "F.Cu"
    if layer_id == 2:
        return "B.Cu"
    if layer_id >= 4 and layer_id % 2 == 0 and layer_id < 64:
        return f"In{(layer_id - 2) // 2}.Cu"
    return layer_names[layer_id]


def _one(it: dict, layer_names: dict, net_names: dict | None) -> str:
    uid = it.get("uuid") or str(_uuid.uuid4())
    if it.get("kind") == "via":
        # no layers (engine through via): first / last copper layer top-down, i.e. F.Cu / B.Cu as to_kipy_item
        order = sorted(layer_names, key=_top_down)
        la = it.get("layers") or order[:1] + order[-1:]
        top, bot = cu_name(int(la[0]), layer_names), cu_name(int(la[-1]), layer_names)
        s = (f"\t(via\n\t\t(at {_mm(it['at'][0])} {_mm(it['at'][1])})\n\t\t(size {_mm(it['d'])})\n")
        if it.get("drill"):
            s += f"\t\t(drill {_mm(it['drill'])})\n"
        s += f"\t\t(layers {_q(top)} {_q(bot)})\n" + _net(it, net_names)
        return s + f"\t\t(uuid {_q(uid)})\n\t)\n"
    p = it["prim"]
    lay = f"\t\t(layer {_q(cu_name(int(it['layer']), layer_names))})\n"
    w = f"\t\t(width {_mm(it['width'])})\n"
    if p["kind"] == "arc":
        s = (f"\t(arc\n\t\t(start {_mm(p['start'][0])} {_mm(p['start'][1])})\n"
             f"\t\t(mid {_mm(p['mid'][0])} {_mm(p['mid'][1])})\n"
             f"\t\t(end {_mm(p['end'][0])} {_mm(p['end'][1])})\n")
    else:
        s = (f"\t(segment\n\t\t(start {_mm(p['a'][0])} {_mm(p['a'][1])})\n"
             f"\t\t(end {_mm(p['b'][0])} {_mm(p['b'][1])})\n")
    return s + w + lay + _net(it, net_names) + f"\t\t(uuid {_q(uid)})\n\t)\n"


def append_items(text: str, items: list[dict], *, layer_names: dict, net_names: dict | None = None) -> str:
    """Insert engine ``seg`` / ``arc`` / ``via`` item dicts before the final ``)`` of the file.

    Layers are named via `layer_names` (KiCad layer id -> name). KiCad 10 refers to nets by name: the
    name comes from ``item["net_name"]`` or `net_names` (net code -> name; this keyword is an addition to
    the B3 signature because engine items carry net codes). An item may carry ``uuid``; otherwise a
    fresh uuid4 is written. O(len(text) + #items).
    """
    end = text.rstrip().rfind(")")
    if end < 0:
        raise ValueError("not a kicad_pcb file (no closing paren)")
    body = "".join(_one(it, layer_names, net_names) for it in items)
    head = text[:end]
    if not head.endswith("\n"):
        head += "\n"
    return head + body + text[end:]


_COPPER_TYPES = ("signal", "power", "mixed", "jumper")
_LAYERS_HEAD = re.compile(r"\(\s*layers\b")
_LAYER_ENTRY = re.compile(r'\(\s*(\d+)\s+"((?:[^"\\]|\\.)*)"\s+([A-Za-z_]+)')


def parse_layers(text: str) -> list[dict]:
    """Copper layers of the board file's top-level ``(layers ...)`` table, top -> bottom:
    ``[{"id": int, "name": str, "type": "signal"|"power"|"mixed"|"jumper"}]``. ``id`` is the engine /
    pcbnew copper id (F.Cu 0, B.Cu 2, In<n>.Cu 2n+2) derived from the layer NAME, so legacy files that
    number B.Cu 31 give the same ids. Non-copper (``user``) layers are skipped. Order: F.Cu, inner
    layers by number, B.Cu. O(len(text)) (one bounded scan of the table)."""
    head = _LAYERS_HEAD.search(text)
    if head is None:
        return []
    depth, end = 0, len(text)
    for m in _TOKEN.finditer(text, head.start()):
        if m.group() == "(":
            depth += 1
        elif m.group() == ")":
            depth -= 1
            if depth == 0:
                end = m.end()
                break
    out = []
    for m in _LAYER_ENTRY.finditer(text, head.end(), end):
        raw, name, typ = int(m.group(1)), m.group(2), m.group(3)
        if typ not in _COPPER_TYPES:
            continue
        inner = re.fullmatch(r"In(\d+)\.Cu", name)
        lid = 0 if name == "F.Cu" else 2 if name == "B.Cu" else 2 * int(inner.group(1)) + 2 if inner else raw
        out.append({"id": lid, "name": name, "type": typ})

    def top_down(g):
        return (0, 0) if g["id"] == 0 else (2, 0) if g["id"] == 2 else (1, g["id"])

    out.sort(key=top_down)
    return out


_VERSION = re.compile(r"\(\s*kicad_pcb\s*\(\s*version\s+(\d+)\s*\)")
_XY = re.compile(r"\(\s*xy\s+(-?[0-9.eE+-]+)\s+(-?[0-9.eE+-]+)\s*\)")
_FILLED = re.compile(r"\(\s*filled_polygon\b")
_LAYER_NAME = re.compile(r'\(\s*layer\s+"((?:[^"\\]|\\.)*)"\s*\)')
_Z_UUID = re.compile(r'\(\s*uuid\s+"?([0-9A-Za-z\-]+)"?\s*\)')

#: `(version N)` written by KiCad 10.0 (snapshot eprom-to-kernal, kicad-cli 10.0.8 `pcb drc --save-board`).
KICAD10_VERSION = 20260206


def file_version(text: str) -> int | None:
    """The ``(version N)`` of a board file header, None when absent. O(1) (head only)."""
    m = _VERSION.search(text[:2000])
    return int(m.group(1)) if m else None


def _nm(v: str) -> int:
    return int(round(float(v) * 1_000_000))


def parse_zone_fills(text: str) -> dict:
    """Filled polygons of the top-level zones: ``{zone uuid: [{"layer": name, "pts": [[x_nm, y_nm], ...]}]}``.
    Each ``filled_polygon`` is one ring (KiCad writes holes as slits / separate rings, so even-odd tests work on
    the union). Zones without uuid are keyed ``"#<index>"``. O(len(text))."""
    out: dict = {}
    depth, zi = 0, 0
    zstart = -1
    for m in _TOKEN.finditer(text):
        c = m.group()
        if c == "(":
            if depth == 1:
                w = _WORD.match(text, m.end())
                if w and w.group() == "zone":
                    zstart = m.start()
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 1 and zstart >= 0:
                chunk = text[zstart:m.end()]
                zstart = -1
                zi += 1
                polys = []
                heads = [h.start() for h in _FILLED.finditer(chunk)]
                for a, b in zip(heads, heads[1:] + [len(chunk)]):
                    part = chunk[a:b]
                    lm = _LAYER_NAME.search(part)
                    pts = [[_nm(x), _nm(y)] for x, y in _XY.findall(part)]
                    if lm and len(pts) >= 3:
                        polys.append({"layer": lm.group(1), "pts": pts})
                head = chunk[: heads[0]] if heads else chunk
                u = _Z_UUID.search(head)
                out[u.group(1) if u else f"#{zi - 1}"] = polys
    return out


_FREE = re.compile(r"\(\s*free(?:\s+(yes|no|true|false))?\s*\)")
_VIA_ID = re.compile(r'\(\s*(?:uuid|tstamp)\s+"?([0-9A-Za-z\-]+)"?\s*\)')


def via_free_flags(text: str) -> dict[str, bool]:
    """KiCad's "free" flag of every top-level via: ``{via uuid: bool}`` (ruling C23; the IPC API of KiCad 10 /
    kipy 0.8 does not expose it). KiCad 10 writes ``(free yes)`` and ``(uuid "…")``; legacy files (KiCad 6)
    ``(free)`` and ``(tstamp …)``; ``(free no)`` / no token = False. A via without an id is skipped (the caller
    treats a missing uuid as free = unsure). O(len(text)) (one tokenizer pass plus a bounded regex per via)."""
    out: dict[str, bool] = {}
    for sp in find_items(text, ("via",)):
        body = text[sp.start:sp.end]
        m = _VIA_ID.search(body)
        if not m:
            continue
        f = _FREE.search(body)
        out[m.group(1)] = bool(f) and (f.group(1) or "yes") in ("yes", "true")
    return out
