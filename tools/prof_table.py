#!/usr/bin/env python3
"""prof_table.py: the per-stage time table of one profiled run (TW0; docs/dev/profiling.md).

  python3 tools/prof_table.py <run-dir | X--metrics.json | result.json> [--name BOARD] [--seed N] [--json out.json] [--md]

Input: a run made with `TOPO_PROF=1` (tools/prof_run.sh does this): the metrics file of harness/route_board.py or the
report's result.json, both carry `profile` (engine counters + the plugin's python split). A run dir is searched for
`*--metrics.json`, then `**/result.json`.

Output (stdout): one table whose rows partition the run's wall time (python before, engine stages, python after;
shares sum to 100 %), the loop split (judge / search / layering / crossing / pair / other), cycles and re-lays per
600 s of loop time, and the hottest counter keys of the loop. `--json` writes the sidecar "metrics" object
`{"<name> s<seed>": {...}}` (flat numbers, see SIDECAR below) so it can be pasted into reports/<branch>.json.
Read-only; no engine needed.

Counter keys are inclusive and summed over all threads (a key inside the refine lanes can exceed the wall time); the
stage and loop rows are wall-clock laps on the run's own thread and add up exactly. Valid for `optimize.runs = 1`
(parallel runs interleave their stage marks; the tool says so when it sees several loops).
"""
import argparse
import json
import re
import sys
from pathlib import Path

STAGE_ROWS = [  # (engine stage id, table label)
    ("stubs", "plane stubs (place_plane_stubs)"),
    ("projection", "projection (Fixed::build)"),
    ("p2_p4", "P2-P4 (initial layering, rip-up and reroute)"),
    ("loop", "TopoR loop"),
    ("relocation", "via / branch relocation"),
    ("drawing", "drawing + feedback"),
    ("via_reduction", "via reduction"),
    ("tail", "tail (diff pairs, round result)"),
    ("other", "engine, unnamed window"),
]
# loop split: (row, [lap keys]); judge is a part of the sweep lap (the sweep minus the judge is "search")
LOOP_LAPS = {"setup": ["topo_setup"], "sweep": ["st_sweep"], "layering": ["st_uncross_reopt", "st_viarose", "st_float"],
             "crossing": ["st_cross"], "pair": ["st_pairrelay", "st_pairmove"]}
PY_KEYS = [("model:", "load + model"), ("engine probe", "engine probe"), ("DRC of the untouched copy", "baseline DRC (kicad-cli)"),
           ("round ended", "engine route (plugin wall)"), ("DRC round", "DRC after routing (kicad-cli)"), ("routed ", "findings"),
           ("board fingerprint before the commit", "pre-commit (refill, checks)"), ("committed:", "zone refill + commit")]


def load(path: Path):
    """(profile dict, metrics dict or {}, report_dir or None)."""
    p = Path(path)
    if p.is_dir():
        c = sorted(p.glob("**/*--metrics.json")) or sorted(p.glob("**/result.json"))
        if not c:
            sys.exit(f"{p}: no *--metrics.json or result.json (was the run made with TOPO_PROF=1?)")
        p = c[0]
    d = json.loads(p.read_text())
    prof = d.get("profile")
    if not prof:
        sys.exit(f"{p}: no `profile` (run with TOPO_PROF=1 and optimize.runs=1)")
    rd = d.get("report_dir")
    if not rd and p.name == "result.json":
        rd = str(p.parent)
    return prof, d, rd


def py_split(rd, before_s: float):
    """Sub-rows of python before / after from the plugin's log.txt stamps: [(label, s, 'before'|'after')]."""
    if not rd or not (Path(rd) / "log.txt").is_file():
        return []
    rows, prev = [], 0.0
    for line in (Path(rd) / "log.txt").read_text(errors="replace").splitlines():
        m = re.match(r"\[\s*([\d.]+)s\] (.*)", line)
        if not m:
            continue
        t, txt = float(m.group(1)), m.group(2)
        for k, lab in PY_KEYS:
            if txt.startswith(k) and not any(r[0] == lab for r in rows):
                rows.append((lab, t - prev, "before" if t <= before_s + 0.5 else "after"))
                prev = t
                break
    return [r for r in rows if r[0] != "engine route (plugin wall)"]


def build(prof: dict) -> dict:
    py = prof.get("python") or {}
    before, job, after = py.get("before_s", 0.0), py.get("job_s", 0.0), py.get("after_s", 0.0)
    stages = prof["stages"]
    eng = {}
    for s in stages:
        e = eng.setdefault(s["stage"], {"wall_s": 0.0, "keys": {}})
        e["wall_s"] += s["wall_s"]
        for k, v in s["keys"].items():
            a = e["keys"].setdefault(k, [0, 0.0, 0])
            a[0] += v[0]; a[1] += v[1]; a[2] += v[2]
    engine_s = sum(e["wall_s"] for e in eng.values())
    total = py.get("total_s") or (before + job + after) or engine_s
    rows = [("python before the engine", before)]
    for sid, lab in STAGE_ROWS:
        if sid in eng:
            rows.append((lab, eng[sid]["wall_s"]))
    rows.append(("engine job overhead (spawn, result JSON)", max(0.0, job - engine_s)))
    rows.append(("python after the engine", after))
    # loop split
    lp = eng.get("loop", {"wall_s": 0.0, "keys": {}})
    k = lp["keys"]
    sec = lambda n: k.get(n, [0, 0.0, 0])[1]
    judge = sec("rr_viol_after")
    laps = {r: sum(sec(x) for x in ks) for r, ks in LOOP_LAPS.items()}
    split = [("judge (whole-board layering + measure per re-lay)", judge), ("search (sweep minus judge: price, place, keep)", max(0.0, laps["sweep"] - judge)),
             ("layering (uncross, layer re-optimisation, via-rose, floating up)", laps["layering"]), ("crossing rules", laps["crossing"]),
             ("pair (pair_min relay, pair_move)", laps["pair"]), ("cycle setup (Topo clone, potentials, sort)", laps["setup"])]
    rest = max(0.0, lp["wall_s"] - sum(x[1] for x in split))
    split.append(("other (offer, archive, snapshot, gen 0, picking)", rest))
    cyc = prof.get("cycles") or []
    n_cyc = len(cyc)
    relays = k.get("reroute", [0, 0, 0])[0]
    kept = sum(c["kept"] for c in cyc)
    loop_s = lp["wall_s"]
    cyc_wall = sum(c["wall_s"] for c in cyc)
    out = {"total_s": total, "rows": rows, "loop_s": loop_s, "loop_split": split, "cycles": n_cyc, "relays": relays, "relays_kept": kept,
           "cycle_s_mean": (cyc_wall / n_cyc) if n_cyc else None,
           "cycles_per_600s": (n_cyc / loop_s * 600.0) if loop_s > 0 and n_cyc else None,
           "relays_per_600s": (relays / loop_s * 600.0) if loop_s > 0 and relays else None,
           "relay_ms_mean": (sec("reroute") / relays * 1000.0) if relays else None,
           "loop_keys": k, "engine_s": engine_s}
    return out


def render(b: dict, name: str, extra) -> str:
    T = b["total_s"] or 1e-9
    L = [f"## {name}: per-stage time (wall, one run, total {b['total_s']:.1f} s)", "", "| stage | s | share |", "|---|---:|---:|"]
    sub = {"python before the engine": [r for r in extra if r[2] == "before"], "python after the engine": [r for r in extra if r[2] == "after"]}
    acc = 0.0
    for lab, s in b["rows"]:
        L.append(f"| {lab} | {s:.2f} | {100 * s / T:.1f} % |")
        acc += s
        if lab == "TopoR loop":
            for sl, ss in b["loop_split"]:
                L.append(f"| &nbsp;&nbsp;of it: {sl} | {ss:.2f} | {100 * ss / T:.1f} % |")
        for sl, ss, _ in sub.get(lab, []):
            L.append(f"| &nbsp;&nbsp;of it: {sl} (log.txt) | {ss:.2f} | {100 * ss / T:.1f} % |")
    L.append(f"| **sum of the rows** | {acc:.2f} | {100 * acc / T:.1f} % |")
    if b["cycles"]:
        f = lambda v, d=1: "-" if v is None else f"{v:.{d}f}"
        L += ["", f"Loop: {b['cycles']} cycles in {b['loop_s']:.1f} s of loop time (mean cycle {f(b['cycle_s_mean'], 2)} s); {b['relays']} re-lays "
              f"({b['relays_kept']} kept in the sweeps; mean {f(b['relay_ms_mean'])} ms each). Per 600 s of loop time: "
              f"**{f(b['cycles_per_600s'])} cycles, {f(b['relays_per_600s'], 0)} re-lays**."]
    ks = sorted(b["loop_keys"].items(), key=lambda x: -x[1][1])[:12]
    if ks:
        L += ["", "Loop counter keys (inclusive, summed over threads; overlapping, not a partition):", "", "| key | calls | s | % of loop wall | units |", "|---|---:|---:|---:|---:|"]
        for kk, v in ks:
            L.append(f"| {kk} | {v[0]} | {v[1]:.3f} | {100 * v[1] / max(b['loop_s'], 1e-9):.1f} | {v[2]} |")
    return "\n".join(L)


def sidecar(b: dict) -> dict:
    """Flat numbers for reports/<branch>.json "metrics"."""
    T = b["total_s"] or 1e-9
    m = {"prof_total_s": round(b["total_s"], 2), "prof_sum_share_pct": round(100 * sum(s for _, s in b["rows"]) / T, 1)}
    for lab, s in b["rows"]:
        key = re.sub(r"[^a-z0-9]+", "_", lab.lower()).strip("_").split("_(")[0][:32]
        m["share_" + key] = round(100 * s / T, 1)
    for sl, ss in b["loop_split"]:
        key = re.sub(r"[^a-z0-9]+", "_", sl.lower().split(" (")[0]).strip("_")
        m["loop_" + key + "_s"] = round(ss, 2)
    m.update({"cycles": b["cycles"], "relays": b["relays"], "cycles_per_600s": None if b["cycles_per_600s"] is None else round(b["cycles_per_600s"], 1),
              "relays_per_600s": None if b["relays_per_600s"] is None else round(b["relays_per_600s"]), "relay_ms_mean": None if b["relay_ms_mean"] is None else round(b["relay_ms_mean"], 1),
              "cycle_s_mean": None if b["cycle_s_mean"] is None else round(b["cycle_s_mean"], 2), "loop_s": round(b["loop_s"], 2)})
    return m


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("run")
    ap.add_argument("--name", default=None)
    ap.add_argument("--seed", default="0")
    ap.add_argument("--json", default=None, help="write {'<name> s<seed>': {...}} (sidecar metrics) here")
    a = ap.parse_args(argv)
    prof, d, rd = load(a.run)
    b = build(prof)
    name = a.name or d.get("board") or d.get("name") or Path(a.run).resolve().name
    print(render(b, name, py_split(rd, (prof.get("python") or {}).get("before_s", 0.0))))
    if len([s for s in prof["stages"] if s["stage"] == "loop"]) > 1:
        print("\nNOTE: several loop windows: the run had more than one loop (optimize.runs > 1?); read the shares as indicative.", file=sys.stderr)
    if a.json:
        Path(a.json).write_text(json.dumps({f"{name} s{a.seed}": sidecar(b)}, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
