"""Engine-neutral scorecard over several runs per board, in % (the method in the plan, "Measure").

    .venv/bin/python -m harness.scorecard <root> [--json out.json] [--compare <other root> [--force]]

`<root>/<board>/s<k>/` are `tools/route_board.sh` outputs of one board with seed k (s0 = the default run,
fluctuation 0; s1.. = fluctuating weights with that seed). Every number is judged by KiCad's DRC only and is relative
to KiCad's own count of unconnected items on the stripped input (`unconnected_input`), the one denominator every
engine shares (engines count "connections" differently):

- connected %    = 1 - unconnected / unconnected_input (after the refill)
- total length   = all tracks and arcs in m, and as % of the hand board's
- PNG            = the default run's hand-vs-topor picture, for visual inspection
- cleanup-unstable items = what KiCad's Edit -> Cleanup Tracks & Vias (every option on) would remove or change in
                   topor's copper, replayed by harness.cleanup_check on the routed copy as the plugin hands it back
                   (route_board metric `cleanup_unstable`); 0 is the target

The table is kept simple on purpose (user 2026-11-06: "connected, total length, and visual inspection"; compare
against hand routed only). Vias, new DRC and detour stay in the --json rows. Seeds s1.. give the spread: median and
min–max. Known blind spots are listed under the table.
`<root>/limits.json` (written by `harness.score_runs`) holds the clock (wall | cpu), the limit, the thread count, the
git hash, the engine path and the load average of the runs. The table header shows it and the table has a CPU s column
and a "limit hits" count (runs whose stop reason is timeout_cpu / timeout_wall). `--compare` prints the two scorecards
side by side per board and REFUSES when the roots differ in clock, limit or threads (or one has no limits.json),
unless `--force`: numbers from different budgets do not compare (user 2026-11-07).
Complexity: O(runs x board text).
"""
from __future__ import annotations

import argparse
import json
import math
import re
import statistics
import sys
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
            "cpu_s": m.get("router_cpu_s"), "stop": m.get("stop_reason"),
            "hit": str(m.get("stop_reason") or "").startswith("timeout"),
            "unconnected": m["unconnected"], "routed": m.get("routed"), "connections": m.get("connections"),
            "cleanup": m.get("cleanup_unstable")}


def _spread(xs: list, fmt: str = ".1f") -> str:
    """Median and [min–max] of the runs (a single value alone)."""
    xs = [x for x in xs if x is not None]
    if not xs:
        return "–"
    if len(xs) == 1:
        return f"{xs[0]:{fmt}}"
    return f"{statistics.median(xs):{fmt}} [{min(xs):{fmt}}–{max(xs):{fmt}}]"


COMPARE_KEYS = ("clock", "limit_s", "threads")


def read_limits(root: Path) -> dict | None:
    """`<root>/limits.json`, or None when the root has none (runs made before the CPU limit existed)."""
    f = Path(root) / "limits.json"
    return json.loads(f.read_text()) if f.is_file() else None


def limits_line(lim: dict | None) -> str:
    if not lim:
        return "Limits: none recorded (no limits.json; runs of an older runner)"
    la = lim.get("load_avg_start") or []
    lb = lim.get("load_avg_end") or []
    return (f"Limits: clock {lim.get('clock')}, limit {lim.get('limit_s')} s, threads {lim.get('threads')}, "
            f"jobs {lim.get('jobs')}, git {lim.get('git_hash')}, engine {lim.get('engine')}, load average "
            f"{la[0]:.1f} -> {lb[0]:.1f}" if la and lb else
            f"Limits: clock {lim.get('clock')}, limit {lim.get('limit_s')} s, threads {lim.get('threads')}, "
            f"jobs {lim.get('jobs')}, git {lim.get('git_hash')}, engine {lim.get('engine')}")


def limits_mismatch(a: dict | None, b: dict | None) -> list:
    """Human-readable differences in clock / limit / threads between two roots' limits ([] = comparable)."""
    if a is None or b is None:
        return ["limits.json missing in " + ("both roots" if a is None and b is None else
                                              "the first root" if a is None else "the second root")]
    return [f"{k}: {a.get(k)!r} vs {b.get(k)!r}" for k in COMPARE_KEYS if a.get(k) != b.get(k)]


def _cleanup_cell(s0: dict | None, sk: list) -> str:
    """Cleanup-unstable items (harness.cleanup_check, 0 = KiCad's Cleanup Tracks & Vias changes nothing) of the default
    run, the seeds' spread in brackets; – for runs made before the check existed."""
    d = (s0 or {}).get("cleanup")
    seeds = [r.get("cleanup") for r in sk if r is not s0 and r.get("cleanup") is not None]
    cell = "–" if d is None else str(d)
    return cell + (f" ({_spread(seeds, '.0f')})" if seeds else "")


def scorecard(root: Path) -> tuple[str, dict]:
    """The markdown table and the raw rows per board."""
    root = Path(root)
    lim = read_limits(root)
    clock_cell = f"{lim['clock']} {lim['limit_s']} s" if lim else "–"
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
            "total length m default (% of hand) | total length % of hand, seeds | limit | CPU s default | "
            "CPU s seeds | limit hits | cleanup-unstable items default (seeds) | PNG (default run) |\n"
            "|---|---|---|---|---|---|---|---|---|---|---|---|---|\n")
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
            f"{default_len} | {_spread([r['length_pct'] for r in sk], '.0f')} | {clock_cell} | "
            f"{_spread([(s0 or {}).get('cpu_s')], '.0f')} | {_spread([r['cpu_s'] for r in sk], '.0f')} | "
            f"{sum(1 for r in runs.values() if r['hit'])} of {len(runs)} | "
            f"{_cleanup_cell(s0, sk)} | {(s0 or {}).get('png') or '–'} |")
    blind = ("\nBlind spots (judged by KiCad DRC, which sees only what it checks): pad-to-pad violations inside a "
             "footprint count as new when routing connects those pads; diff pairs are skipped by the topological "
             "core and count as unconnected; zone fills are judged after kicad-cli's refill, not as saved; total "
             "length compares routing styles too: a hand board that routes a pour net (GND) with tracks is longer "
             "than topor's pour + stubs (dcsgonebadge hand: 1.5 m of GND track).\n")
    return limits_line(lim) + "\n\n" + head + "\n".join(lines) + "\n" + blind, rows


def compare(root_a: Path, root_b: Path, *, force: bool = False) -> tuple[str, int]:
    """Per board: connected % (default / seeds median), length % of hand and CPU s of two roots side by side.
    Returns (text, exit code); 2 and a refusal when the limits differ and not `force`."""
    la, lb = read_limits(root_a), read_limits(root_b)
    diff = limits_mismatch(la, lb)
    if diff and not force:
        return ("REFUSED: the two roots were run under different limits (" + "; ".join(diff) + "). Results from "
                "different budgets do not compare. Re-run one root with the same clock / limit / threads, or pass "
                "--force to compare anyway.", 2)
    _, ra = scorecard(root_a)
    _, rb = scorecard(root_b)
    out = [f"A = {root_a}: {limits_line(la)}", f"B = {root_b}: {limits_line(lb)}"]
    if diff:
        out.append("WARNING (--force): limits differ: " + "; ".join(diff))

    def med(runs, key):
        return _spread([r[key] for k, r in runs.items() if k > 0] or [runs[0][key]], ".1f")

    out += ["", "| board | connected % default A | B | connected % seeds A | B | length % seeds A | B | CPU s seeds A | B | "
            "limit hits A | B |", "|---|---|---|---|---|---|---|---|---|---|---|"]
    for b in sorted(set(ra) | set(rb)):
        x, y = ra.get(b), rb.get(b)
        d = lambda r, k: f"{r[0][k]:.1f}" if r and 0 in r and r[0].get(k) is not None else "–"  # noqa: E731
        m = lambda r, k: med(r, k) if r else "–"  # noqa: E731
        h = lambda r: f"{sum(1 for v in r.values() if v['hit'])} of {len(r)}" if r else "–"  # noqa: E731
        out.append(f"| {b} | {d(x, 'done')} | {d(y, 'done')} | {m(x, 'done')} | {m(y, 'done')} | "
                   f"{m(x, 'length_pct')} | {m(y, 'length_pct')} | {_spread([r['cpu_s'] for r in (x or {}).values()], '.0f')} | "
                   f"{_spread([r['cpu_s'] for r in (y or {}).values()], '.0f')} | {h(x)} | {h(y)} |")
    return "\n".join(out), 0


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("root", type=Path)
    ap.add_argument("--json", type=Path, default=None)
    ap.add_argument("--compare", type=Path, default=None, metavar="OTHER_ROOT",
                    help="compare with another root; refuses when clock / limit / threads differ")
    ap.add_argument("--force", action="store_true", help="--compare although the limits differ")
    a = ap.parse_args(argv)
    if a.compare:
        text, rc = compare(a.root, a.compare, force=a.force)
        print(text, file=sys.stderr if rc else sys.stdout)
        return rc
    md, rows = scorecard(a.root)
    print(md)
    if a.json:
        a.json.write_text(json.dumps(rows, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
