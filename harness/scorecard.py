"""Engine-neutral scorecard over several runs per board, in % (the method in the plan, "Measure").

    .venv/bin/python -m harness.scorecard <root> [--json out.json]

`<root>/<board>/s<k>/` are `tools/route_board.sh` outputs of one board with seed k (s0 = the default run,
fluctuation 0; s1.. = fluctuating weights with that seed). Every number is judged by KiCad's DRC only and is relative
to KiCad's own count of unconnected items on the stripped input (`unconnected_input`), the one denominator every
engine shares (engines count "connections" differently):

- connected %    = 1 - unconnected / unconnected_input (after the refill)
- total length   = all tracks and arcs in m, and as % of the hand board's
- PNG            = the default run's hand-vs-topor picture, for visual inspection

The table is kept simple on purpose (user 2026-11-06: "connected, total length, and visual inspection"; compare
against hand routed only). Vias, new DRC and detour stay in the --json rows. Seeds s1.. give the spread: median and
min–max. Known blind spots are listed under the table.
Complexity: O(runs x board text).
"""
from __future__ import annotations

import argparse
import json
import math
import re
import statistics
from pathlib import Path

_AT = re.compile(r"\(at\s+([-\d.]+)\s+([-\d.]+)")
_START = re.compile(r"\(start\s+([-\d.]+)\s+([-\d.]+)\)")
_END = re.compile(r"\(end\s+([-\d.]+)\s+([-\d.]+)\)")
_NET = re.compile(r"\(net\s+(\"[^\"]*\"|\d+)")
_MID = re.compile(r"\(mid\s+([-\d.]+)\s+([-\d.]+)\)")


def _arc_len(a, m, b) -> float:
    """Length of the circular arc a -> m -> b (chord length when the three points are collinear)."""
    ax, ay = a
    bx, by = m
    cx, cy = b
    d = 2 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by))
    chord = math.dist(a, b)
    if abs(d) < 1e-12:
        return chord
    ux = ((ax * ax + ay * ay) * (by - cy) + (bx * bx + by * by) * (cy - ay) + (cx * cx + cy * cy) * (ay - by)) / d
    uy = ((ax * ax + ay * ay) * (cx - bx) + (bx * bx + by * by) * (ax - cx) + (cx * cx + cy * cy) * (bx - ax)) / d
    r = math.dist((ux, uy), a)
    ang = lambda p: math.atan2(p[1] - uy, p[0] - ux)  # noqa: E731
    sweep = (ang(b) - ang(a)) % (2 * math.pi)
    if (ang(m) - ang(a)) % (2 * math.pi) > sweep:  # the mid point lies on the other way round
        sweep = 2 * math.pi - sweep
    return r * sweep


def total_length_mm(pcb: Path) -> float:
    """Sum of all track segment and arc lengths of a board file, in mm. O(len(text))."""
    from toporlib import pcbfile
    text = Path(pcb).read_text()
    tot = 0.0
    for sp in pcbfile.find_items(text, ("segment", "arc")):
        body = text[sp.start:sp.end]
        a, b = _START.search(body), _END.search(body)
        if not (a and b):
            continue
        pa, pb = (float(a.group(1)), float(a.group(2))), (float(b.group(1)), float(b.group(2)))
        m = _MID.search(body) if sp.kind == "arc" else None
        tot += _arc_len(pa, (float(m.group(1)), float(m.group(2))), pb) if m else math.dist(pa, pb)
    return tot


def via_split(pcb: Path) -> tuple[int, int]:
    """(signal vias, stitching vias) of a board file: a via is signal when a track or arc of its net ends at its
    centre (1 µm grid), else stitching. O(len(text))."""
    from toporlib import pcbfile
    text = Path(pcb).read_text()
    ends: set = set()
    vias = []
    for sp in pcbfile.find_items(text, ("segment", "arc", "via")):
        body = text[sp.start:sp.end]
        net = _NET.search(body)
        net = net.group(1) if net else ""
        if sp.kind == "via":
            m = _AT.search(body)
            if m:
                vias.append((net, round(float(m.group(1)), 3), round(float(m.group(2)), 3)))
            continue
        for rx in (_START, _END):
            m = rx.search(body)
            if m:
                ends.add((net, round(float(m.group(1)), 3), round(float(m.group(2)), 3)))
    sig = sum(1 for v in vias if v in ends)
    return sig, len(vias) - sig


def run_row(d: Path, board: str) -> dict | None:
    """One run's numbers, or None when the run left no metrics."""
    mf = d / f"{board}--metrics.json"
    if not mf.is_file():
        return None
    m = json.loads(mf.read_text())
    n = m["unconnected_input"] or 1
    topor = next(iter(sorted((d / f"{board}-topor").glob("*.kicad_pcb"))), None)
    hand = next(iter(sorted((d / f"{board}-hand").glob("*.kicad_pcb"))), None)
    sig, stitch = via_split(topor) if topor else (m["vias"], 0)
    hsig, hstitch = via_split(hand) if hand else (m["vias_hand"], 0)
    length = total_length_mm(topor) / 1000 if topor else None
    hand_length = total_length_mm(hand) / 1000 if hand else None
    png = d / f"{board}-compare.png"
    return {"n": n, "done": 100 * (1 - m["unconnected"] / n), "hand_done": 100 * (1 - m["unconnected_hand"] / n),
            "length_m": length, "hand_length_m": hand_length,
            "length_pct": 100 * length / hand_length if length and hand_length else None,
            "png": str(png) if png.is_file() else None,
            "sig": 100 * sig / n, "stitch": 100 * stitch / n, "hand_sig": 100 * hsig / n,
            "hand_stitch": 100 * hstitch / n, "drc": 100 * m["routing_new"] / n,
            "det90": (m.get("detour") or {}).get("p90"), "router_s": m.get("router_s"),
            "unconnected": m["unconnected"], "routed": m.get("routed"), "connections": m.get("connections")}


def _spread(xs: list, fmt: str = ".1f") -> str:
    """Median and [min–max] of the runs (a single value alone)."""
    xs = [x for x in xs if x is not None]
    if not xs:
        return "–"
    if len(xs) == 1:
        return f"{xs[0]:{fmt}}"
    return f"{statistics.median(xs):{fmt}} [{min(xs):{fmt}}–{max(xs):{fmt}}]"


def scorecard(root: Path) -> tuple[str, dict]:
    """The markdown table and the raw rows per board."""
    root = Path(root)
    rows: dict = {}
    for bd in sorted(p for p in root.iterdir() if p.is_dir()):
        runs = {}
        for sd in sorted(bd.glob("s*")):
            r = run_row(sd, bd.name)
            if r:
                runs[int(sd.name[1:])] = r
        if runs:
            rows[bd.name] = runs
    head = ("| board | input | connected % default | connected % seeds 1–5 | hand connected % | "
            "total length m default (% of hand) | total length % of hand, seeds | PNG (default run) |\n"
            "|---|---|---|---|---|---|---|---|\n")
    lines = []
    for b, runs in rows.items():
        s0 = runs.get(0)
        sk = [r for k, r in runs.items() if k > 0] or [s0]
        any_r = s0 or sk[0]
        default_done = f"{s0['done']:.1f}" if s0 else "–"
        default_len = (f"{s0['length_m']:.2f} ({s0['length_pct']:.0f} %)"
                       if s0 and s0["length_m"] is not None and s0["length_pct"] is not None else "–")
        lines.append(
            f"| {b} | {any_r['n']} | {default_done} | {_spread([r['done'] for r in sk])} | {any_r['hand_done']:.1f} | "
            f"{default_len} | {_spread([r['length_pct'] for r in sk], '.0f')} | {(s0 or {}).get('png') or '–'} |")
    blind = ("\nBlind spots (judged by KiCad DRC, which sees only what it checks): pad-to-pad violations inside a "
             "footprint count as new when routing connects those pads; diff pairs are skipped by the topological "
             "core and count as unconnected; zone fills are judged after kicad-cli's refill, not as saved; total "
             "length compares routing styles too: a hand board that routes a pour net (GND) with tracks is longer "
             "than topor's pour + stubs (dcsgonebadge hand: 1.5 m of GND track).\n")
    return head + "\n".join(lines) + "\n" + blind, rows


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("root", type=Path)
    ap.add_argument("--json", type=Path, default=None)
    a = ap.parse_args(argv)
    md, rows = scorecard(a.root)
    print(md)
    if a.json:
        a.json.write_text(json.dumps(rows, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
