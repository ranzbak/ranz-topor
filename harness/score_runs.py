"""The standard scorecard runner: default run plus seeds 1-5 per board, under one fixed, hard-to-fudge limit.

    .venv/bin/python -m harness.score_runs <root> <board> [<board> ...] [--seeds 5] [--only 0,3]

Writes `<root>/<board>/s<k>/` (k = 0..5; s0 = the default run, s<k> = `--set optimize.seed=k`), the layout
`python -m harness.scorecard <root>` reads, by calling `tools/route_board.sh` once per run, ONE RUN AT A TIME
(`jobs` = 1: no two boards share cores, which on a laptop means two hardware threads per core and a slower CPU
second). The limit is fixed on purpose (user 2026-11-07: raising `timeout_s` to 1800 skewed the algorithm
comparison): CPU time, 2400 s (= the old 600 s wall x 4 rayon threads), RAYON_NUM_THREADS=4. CPU time is summed over
all threads, so a busy machine or extra threads neither buy nor cost budget (`[run] timeout_clock`, docs/user/operate.md).
After every run its M1 compass row (predicted / drawn / KiCad DRC per violation class, `toporlib.compass`) is appended
to `<root>/compass.tsv` and printed. Every run has `[capacity] compass_cut_check = true` (a measurement: the
read-only cut test of the plan runs after the drawing, outside the time limit, so the compass's model-gap rows are
measured; `--no-cut-check` leaves it off; recorded in limits.json). Changing a default is possible (`--clock`, `--timeout`, `--threads`) but is written to `<root>/limits.json` with the
git hash, the engine path and the load average at start and end, and `harness.scorecard --compare` refuses to
compare roots whose clock, limit or thread count differ unless `--force`.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CLOCK, TIMEOUT_S, THREADS, JOBS, SEEDS = "cpu", 2400, 4, 1, 5


def git_hash() -> str:
    try:
        h = subprocess.run(["git", "rev-parse", "--short=12", "HEAD"], cwd=ROOT, capture_output=True, text=True,
                           timeout=20).stdout.strip()
        dirty = subprocess.run(["git", "status", "--porcelain", "--untracked-files=no"], cwd=ROOT,
                               capture_output=True, text=True, timeout=20).stdout.strip()
        return h + ("+dirty" if dirty else "")
    except Exception:  # noqa: BLE001
        return "unknown"


def engine_path() -> str:
    """The topor_engine that tools/route_board.sh will import (TOPOR_ENGINE_SITE wins over the venv's)."""
    import importlib.util
    site = os.environ.get("TOPOR_ENGINE_SITE")
    if site and (Path(site) / "topor_engine").exists():
        return str(next(iter(sorted((Path(site) / "topor_engine").glob("__init__.py*"))), Path(site) / "topor_engine"))
    spec = importlib.util.find_spec("topor_engine")
    return str(spec.origin) if spec and spec.origin else "unknown"


def limits(clock: str, timeout_s: float, threads: int, jobs: int, seeds: int, boards: list) -> dict:
    return {"clock": clock, "limit_s": timeout_s, "threads": threads, "jobs": jobs, "seeds": seeds,
            "boards": list(boards), "git_hash": git_hash(), "engine": engine_path(),
            "load_avg_start": list(os.getloadavg()), "load_avg_end": None,
            "started": time.strftime("%Y-%m-%d %H:%M:%S")}


# M1 compass measurement every scorecard run makes (outside the time limit; never changes the routing).
CUT_CHECK = "capacity.compass_cut_check=true"


def run_command(board: str, out_dir: Path, seed: int, *, clock: str, timeout_s: float, png: bool = True,
                sets: tuple = ()) -> list:
    """The route_board command of one run (seed 0 = defaults); `sets` are extra `section.key=value` settings."""
    cmd = [str(ROOT / "tools" / "route_board.sh"), board, str(out_dir), "--clock", clock, "--timeout", f"{timeout_s:g}"]
    if seed:
        cmd += ["--set", f"optimize.seed={seed}"]
    for kv in sets:
        cmd += ["--set", kv]
    if not png:
        cmd.append("--no-png")
    return cmd


def compass_row(root: Path, board: str, k: int, out: Path) -> str | None:
    """M1 compass columns of one finished run (`<out>/*--compass.json`, written by route_board) appended to
    `<root>/compass.tsv` (board, run, then p_/d_/k_<class> = predicted / drawn / KiCad DRC per class, detour_gt_1_5,
    vias_plan_ne_drawn; `harness.predict_vs_drawn <root>` prints the tables). Returns the run's compass line."""
    from toporlib import compass as compass_mod
    hits = sorted(out.glob("*--compass.json"))
    if not hits:
        return None
    cp = json.loads(hits[0].read_text())
    cols = compass_mod.columns(cp)
    if not cols:
        return None
    # M2.TL: the time limit's columns (reached, the step that ran then, seconds past it on the run's clock)
    mets = sorted(out.glob("*--metrics.json"))
    met = json.loads(mets[0].read_text()) if mets else {}
    cols.update({"limit_reached": met.get("limit_reached"), "limit_step": met.get("limit_step"),
                 "overrun_s": met.get("overrun_s"), "router_cpu_s": met.get("router_cpu_s"),
                 # F3: the overrun cap fired (the run then acted as with the hard limit)
                 "overrun_cap_hit": met.get("overrun_cap_hit")})
    tsv = root / "compass.tsv"
    keys = sorted(cols)
    new = not tsv.exists()
    with tsv.open("a") as f:
        if new:
            f.write("\t".join(["board", "run", *keys]) + "\n")
        f.write("\t".join([board, f"s{k}", *("" if cols[c] is None else str(cols[c]) for c in keys)]) + "\n")
    return compass_mod.summary_line(cp)


def main(argv: list | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("root", type=Path, help="output root: <root>/<board>/s<k>/, <root>/limits.json")
    ap.add_argument("boards", nargs="+", help="BENCH_BOARDS keys or .kicad_pcb paths")
    ap.add_argument("--seeds", type=int, default=SEEDS, help=f"seeds 1..N after the default run (default {SEEDS})")
    ap.add_argument("--only", default=None, help="comma list of run numbers to run (0 = default), e.g. 0,3")
    ap.add_argument("--clock", choices=("wall", "cpu"), default=CLOCK, help=f"default {CLOCK}")
    ap.add_argument("--timeout", type=float, default=TIMEOUT_S, help=f"limit in seconds of that clock (default {TIMEOUT_S})")
    ap.add_argument("--threads", type=int, default=THREADS, help=f"RAYON_NUM_THREADS (default {THREADS})")
    ap.add_argument("--no-png", action="store_true")
    ap.add_argument("--dry-run", action="store_true", help="print the commands and write limits.json only")
    ap.add_argument("--no-cut-check", action="store_true",
                    help="leave [capacity] compass_cut_check off (the compass's model-gap rows read 'not measured')")
    ap.add_argument("--set", action="append", default=[], metavar="SECTION.KEY=VALUE",
                    help="extra setting for every run (debug dumps such as capacity.cut_report=DIR; recorded in "
                         "limits.json). A {board} and {k} in the value become the board name and run number.")
    a = ap.parse_args(argv)
    runs = [int(x) for x in a.only.split(",")] if a.only else list(range(0, a.seeds + 1))
    a.root.mkdir(parents=True, exist_ok=True)
    lim = limits(a.clock, a.timeout, a.threads, JOBS, a.seeds, a.boards)
    if not a.no_cut_check and not any(kv.startswith("capacity.compass_cut_check=") for kv in a.set):
        a.set.append(CUT_CHECK)
    lim["sets"] = list(a.set)
    lf = a.root / "limits.json"
    lf.write_text(json.dumps(lim, indent=1))
    env = {**os.environ, "RAYON_NUM_THREADS": str(a.threads)}
    rc = 0
    try:
        for b in a.boards:
            name = Path(b).stem if b.endswith(".kicad_pcb") else b
            for k in runs:
                out = a.root / name / f"s{k}"
                sets = tuple(kv.replace("{board}", name).replace("{k}", str(k)) for kv in a.set)
                cmd = run_command(b, out, k, clock=a.clock, timeout_s=a.timeout, png=not a.no_png, sets=sets)
                print(" ".join(cmd), flush=True)
                if a.dry_run:
                    continue
                out.parent.mkdir(parents=True, exist_ok=True)
                with (a.root / f"{name}-s{k}.log").open("w") as log:
                    r = subprocess.run(cmd, env=env, stdout=log, stderr=subprocess.STDOUT)
                if r.returncode:
                    print(f"  run failed (exit {r.returncode}): see {a.root / f'{name}-s{k}.log'}", file=sys.stderr)
                    rc = 1
                line = compass_row(a.root, name, k, out)
                if line:
                    print(f"  {line}", flush=True)
                # F3 run.timeout_overrun_s (default auto = timeout_s headless): say when the cap fired, the run then
                # acted as with the hard limit (limits.json overrun_cap_hit lists those runs)
                mets = sorted(out.glob("*--metrics.json"))
                met = json.loads(mets[0].read_text()) if mets else {}
                if met.get("overrun_cap_hit"):
                    print(f"  WARNING: {name} s{k}: time limit overrun cap reached ({met.get('overrun_cap_s')} s past the "
                          f"{a.timeout:g} s {a.clock} limit): the run stopped as with the hard limit", flush=True)
                    lim.setdefault("overrun_cap_hit", []).append(f"{name}/s{k}")
    finally:
        lim["load_avg_end"] = list(os.getloadavg())
        lim["finished"] = time.strftime("%Y-%m-%d %H:%M:%S")
        lf.write_text(json.dumps(lim, indent=1))
    return rc


if __name__ == "__main__":
    raise SystemExit(main())
