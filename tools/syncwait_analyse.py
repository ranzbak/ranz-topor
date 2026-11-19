#!/usr/bin/env python3
"""sync-wait: loop timing from the plugin log.txt cycle lines (docs/dev/design/sync-wait.md).

    syncwait_analyse.py <run-root> [<run-root> ...] [--json]

<run-root>: any directory; every log.txt below it with "loop cycle:" lines is one run. Per run:
  cycles (all runs of the call), per-run cycles, loop span (first cycle start to last cycle end, s), cycles/min,
  idle % = the engine's slot idle share (last "loop slots:" line: free slot-time while a loop ran),
  table % = table wait / (cycle + table wait), summed over the runs (the round-slow probe's "sync %"),
  slot % = slot wait / (cycle + table wait + slot wait) (waiting for a slot after a fair turn: not idle CPU),
  max table wait (s). O(lines).
"""
import json
import re
import sys
from pathlib import Path

CYC = re.compile(r"loop cycle: run (\d+), cycle (\d+), at ([\d.]+) s, cycle ([\d.]+) s, table wait ([\d.]+) s, slot wait ([\d.]+) s, errors (\d+)")
SLOTS = re.compile(r"loop slots: .*?so far ([\d.]+) % of the (\d+) compute slots")


def one(log: Path) -> dict | None:
    runs, idle = {}, None
    first, last = None, None
    for ln in log.read_text(errors="replace").splitlines():
        m = CYC.search(ln)
        if m:
            r, c, at, cs, tw, sw = int(m[1]), int(m[2]), float(m[3]), float(m[4]), float(m[5]), float(m[6])
            runs.setdefault(r, []).append((c, at, cs, tw, sw, int(m[7])))
            start = at - cs - tw
            first = start if first is None else min(first, start)
            last = at if last is None else max(last, at)
            continue
        m = SLOTS.search(ln)
        if m:
            idle = float(m[1])
    if not runs:
        return None
    cyc = sum(len(v) for v in runs.values())
    cs = sum(x[2] for v in runs.values() for x in v)
    tw = sum(x[3] for v in runs.values() for x in v)
    sw = sum(x[4] for v in runs.values() for x in v)
    span = (last - first) if first is not None else 0.0
    return {"run": str(log), "cycles": cyc, "per_run": [len(runs[k]) for k in sorted(runs)], "span_s": round(span, 1),
            "cycles_per_min": round(60.0 * cyc / span, 2) if span > 0 else None, "idle_pct": idle,
            "table_pct": round(100.0 * tw / max(cs + tw, 1e-9), 1), "slot_pct": round(100.0 * sw / max(cs + tw + sw, 1e-9), 1),
            "max_table_wait_s": round(max(x[3] for v in runs.values() for x in v), 1),
            "last_errors": {k: runs[k][-1][5] for k in sorted(runs)}}


def main(argv):
    as_json = "--json" in argv
    rows = []
    for root in [a for a in argv if not a.startswith("--")]:
        for log in sorted(Path(root).rglob("log.txt")):
            r = one(log)
            if r:
                rows.append(r)
    if as_json:
        print(json.dumps(rows, indent=1))
        return
    for r in rows:
        print(f"{r['run']}\n  cycles {r['cycles']} {r['per_run']} span {r['span_s']} s, {r['cycles_per_min']}/min, idle {r['idle_pct']} %, "
              f"table {r['table_pct']} %, slot {r['slot_pct']} %, max table wait {r['max_table_wait_s']} s")


if __name__ == "__main__":
    main(sys.argv[1:])
