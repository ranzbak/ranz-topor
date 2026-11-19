#!/usr/bin/env python3
"""prof_compare.py (TW0): before / after per-stage tables side by side.

  python3 tools/prof_compare.py <base-dir> <tip-dir> [--boards a,b] [--md out.md] [--json out.json] [--labels base,tip]

Each dir holds one prof_run.sh output dir per board (`<dir>/<board>/run/*--metrics.json`, as tools/prof_job.sh writes
them). Prints, per board, the stage seconds of both runs and the delta, the loop split, cycles and re-lays per 600 s of
loop time; `--json` gives {"<board>": {"base": sidecar, "tip": sidecar}} (the sidecar metrics of tools/prof_table.py).
Seconds from two revs are comparable only when both ran on the same exclusive VM with the same threads and seeds.
"""
import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import prof_table as pt  # noqa: E402


def one(d: Path, board: str):
    c = sorted((d / board).glob("**/*--metrics.json"))
    if not c:
        return None
    prof, _, _ = pt.load(c[0])
    return pt.build(prof)


def f(v, n=1):
    return "-" if v is None else f"{v:.{n}f}"


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("base", type=Path)
    ap.add_argument("tip", type=Path)
    ap.add_argument("--boards", default=None)
    ap.add_argument("--labels", default="base,tip")
    ap.add_argument("--md", default=None)
    ap.add_argument("--json", default=None)
    a = ap.parse_args(argv)
    lb, lt = a.labels.split(",")
    boards = a.boards.split(",") if a.boards else sorted(p.name for p in a.base.iterdir() if p.is_dir() and any(p.glob("**/*--metrics.json")))
    out, js = [], {}
    for b in boards:
        x, y = one(a.base, b), one(a.tip, b)
        if not x or not y:
            out.append(f"## {b}: missing run ({'base' if not x else 'tip'})\n")
            continue
        js[b] = {"base": pt.sidecar(x), "tip": pt.sidecar(y)}
        out += [f"## {b}", "", f"| stage | {lb} s | {lb} % | {lt} s | {lt} % | delta s |", "|---|---:|---:|---:|---:|---:|"]
        ry = dict(y["rows"])
        for lab, s in x["rows"]:
            t = ry.get(lab, 0.0)
            out.append(f"| {lab} | {s:.2f} | {100 * s / x['total_s']:.1f} | {t:.2f} | {100 * t / y['total_s']:.1f} | {t - s:+.2f} |")
            if lab == "TopoR loop":
                rl = dict(y["loop_split"])
                for sl, ss in x["loop_split"]:
                    tt = rl.get(sl, 0.0)
                    out.append(f"| &nbsp;&nbsp;of it: {sl} | {ss:.2f} | {100 * ss / x['total_s']:.1f} | {tt:.2f} | {100 * tt / y['total_s']:.1f} | {tt - ss:+.2f} |")
        out.append(f"| **total** | {x['total_s']:.2f} | 100 | {y['total_s']:.2f} | 100 | {y['total_s'] - x['total_s']:+.2f} |")
        out += ["", "| loop | " + lb + " | " + lt + " |", "|---|---:|---:|"]
        for lab, k in [("cycles", "cycles"), ("re-lays", "relays"), ("loop wall s", "loop_s"), ("mean cycle s", "cycle_s_mean"), ("mean re-lay ms", "relay_ms_mean"),
                       ("cycles per 600 s of loop", "cycles_per_600s"), ("re-lays per 600 s of loop", "relays_per_600s")]:
            out.append(f"| {lab} | {f(x[k], 2 if 's' in k and 'ms' not in k else 1)} | {f(y[k], 2 if 's' in k and 'ms' not in k else 1)} |")
        out.append("")
    text = "\n".join(out)
    print(text)
    if a.md:
        Path(a.md).write_text(text + "\n")
    if a.json:
        Path(a.json).write_text(json.dumps(js, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
