"""S3b learning loop driver: hold-out rotation, evaluation with the full engine, candidate factors.

    .venv/bin/python -m harness.learn all     [--out scratch/topo-s3b] [--boards a,b,...] [--jobs 3]
    .venv/bin/python -m harness.learn folds   ...   # train one fold per board (examples/learn)
    .venv/bin/python -m harness.learn stats   ...   # hand vs engine event statistics (examples/learn --stats)
    .venv/bin/python -m harness.learn eval    ...   # full engine (examples/topo_route) default vs learned
    .venv/bin/python -m harness.learn report  ...   # report.md + learned-avg.toml from the JSON above

Inputs are the R1 exports `scratch/r1/<dir>/<name>--hand.json` / `--input.json`. Every fold trains
on all boards but one and evaluates the held-out board with the full engine (`topo_route
--timeout 0`, so the result does not depend on machine load) at the default weights and at the
fold's learned weights: routed, vias, detour (median and p90 of length / straight pad distance)
and different-net same-layer crossings of the emitted tracks (must be 0). The weights averaged
over the folds are evaluated on every board. Learned weights are output only: a `topor.toml
[factors]` snippet the user may copy; nothing is applied by default.
"""
from __future__ import annotations

import argparse
import collections
import json
import statistics
import subprocess
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
R1 = Path("/home/paul/work/kicad/plugins/topor/scratch/r1")
# name -> (directory under scratch/r1, file prefix)
BOARDS = {
    "eprom": ("eprom-to-kernal", "eprom-to-kernal"),
    "wiccon": ("wiccon-2023", "wiccon-2023"),
    "dcsgonebadge": ("dcsgonebadge", "dcsgonebadge"),
    "qmtech": ("qmtech", "qmtech-minimig"),
    "tanmatsu": ("tanmatsu", "tanmatsu"),
    "bornhack2026": ("bornhack2026", "bornhack2026"),
}
# Event kinds of examples/learn --stats compared hand vs engine, per 100 mm of copper (hand chains and
# engine connections join pads differently, so a per-wire rate would not compare).
EVENT_KINDS = ["vias", "layer_forced", "crossings", "same_net_crossings", "squeezes", "overflows",
               "blocks_pad", "ignored_rule", "excess_length_mm", "passes"]


def paths(name: str, r1: Path = R1) -> tuple[Path, Path]:
    d, n = BOARDS[name]
    return r1 / d / f"{n}--hand.json", r1 / d / f"{n}--input.json"


def board_args(names: list[str], r1: Path = R1) -> list[str]:
    out: list[str] = []
    for n in names:
        h, i = paths(n, r1)
        out += ["--board", n, str(h), str(i)]
    return out


def run(cmd: list[str], log: Path | None = None) -> str:
    r = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if log is not None:
        log.write_text(r.stdout + r.stderr)
    if r.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd[:3])} ...: exit {r.returncode}\n{r.stderr[-2000:]}")
    return r.stdout


# ---------------------------------------------------------------- metrics of a RouteResult

def _orient(a, b, c) -> float:
    return (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])


def samelayer_crossings(result: dict, grid: int = 2_000_000) -> int:
    """Different-net track segments that properly cross on one layer (the check of tools/samelayer.py)."""
    segs: dict = collections.defaultdict(list)
    for it in result.get("items", []):
        p = it.get("prim")
        if p and p.get("kind") == "seg":
            segs[it["layer"]].append((it["net"], p["a"], p["b"]))
    n = 0
    for ss in segs.values():
        cells: dict = collections.defaultdict(list)
        for i, s in enumerate(ss):
            xs, ys = (s[1][0], s[2][0]), (s[1][1], s[2][1])
            for gx in range(min(xs) // grid, max(xs) // grid + 1):
                for gy in range(min(ys) // grid, max(ys) // grid + 1):
                    cells[(gx, gy)].append(i)
        seen = set()
        for cell in cells.values():
            for x in range(len(cell)):
                for y in range(x + 1, len(cell)):
                    a, b = cell[x], cell[y]
                    if (a, b) in seen:
                        continue
                    seen.add((a, b))
                    s, t = ss[a], ss[b]
                    if s[0] == t[0]:
                        continue
                    o1, o2 = _orient(s[1], s[2], t[1]), _orient(s[1], s[2], t[2])
                    o3, o4 = _orient(t[1], t[2], s[1]), _orient(t[1], t[2], s[2])
                    if o1 * o2 < 0 and o3 * o4 < 0:
                        n += 1
    return n


def metrics(result: dict) -> dict:
    """routed, vias, detour median / p90 (routed connections, length / straight pad distance) and
    same-layer crossings of a topo_route --out JSON."""
    ratios = sorted(c["length_nm"] / c["ghost_len_nm"] for c in result["connections"]
                    if c["status"] == "routed" and (c.get("ghost_len_nm") or 0) > 0)

    def q(f: float) -> float:
        return ratios[round((len(ratios) - 1) * f)] if ratios else 0.0

    return {"connections": result["connections_total"], "routed": result["routed"], "vias": result["vias"],
            "detour_median": round(q(0.5), 4), "detour_p90": round(q(0.9), 4),
            "same_layer": samelayer_crossings(result), "runtime_s": round(result.get("runtime_s", 0.0), 2)}


def verdict(default: dict, learned: dict) -> str:
    """better / worse / mixed / same: routed up is better, vias and detour p90 down are better."""
    keys = (("routed", 1), ("vias", -1), ("detour_p90", -1))
    signs = []
    for k, good in keys:
        d = (learned[k] - default[k]) * good
        signs.append(0 if abs(d) < 1e-9 else (1 if d > 0 else -1))
    if learned["same_layer"] > 0:
        return "worse (same-layer crossings)"
    if all(s >= 0 for s in signs) and any(s > 0 for s in signs):
        return "better"
    if all(s <= 0 for s in signs) and any(s < 0 for s in signs):
        return "worse"
    return "same" if all(s == 0 for s in signs) else "mixed"


# ---------------------------------------------------------------- steps

def folds(out: Path, names: list[str], bins: Path, jobs: int, extra: list[str]) -> None:
    def one(h: str) -> None:
        run([str(bins / "learn"), *board_args(names), "--hold-out", h, "--out", str(out / f"fold-{h}.json"),
             "--toml", str(out / f"fold-{h}.toml"), *extra], out / f"fold-{h}.log")
    with ThreadPoolExecutor(jobs) as ex:
        list(ex.map(one, names))


def stats(out: Path, names: list[str], bins: Path) -> None:
    run([str(bins / "learn"), *board_args(names), "--stats", str(out / "stats.json"), "--stats-only"], out / "stats.log")


def factor_args(w: dict) -> list[str]:
    return [a for k, v in sorted(w.items()) for a in ("--factor", f"{k}={v:.6f}")]


def route(bins: Path, name: str, weights: dict, dest: Path) -> dict:
    _, inp = paths(name)
    run([str(bins / "topo_route"), str(inp), "--timeout", "0", "--out", str(dest), *factor_args(weights)])
    return metrics(json.loads(dest.read_text()))


def averaged(out: Path, names: list[str]) -> dict:
    ws = [json.loads((out / f"fold-{n}.json").read_text())["weights"] for n in names]
    return {k: statistics.fmean(w[k] for w in ws) for k in sorted(ws[0])}


def evaluate(out: Path, names: list[str], bins: Path, jobs: int) -> None:
    tasks = []
    for n in names:
        w = json.loads((out / f"fold-{n}.json").read_text())["weights"]
        tasks += [(n, "default", {}), (n, "learned", w)]
    avg = averaged(out, names)
    tasks += [(n, "averaged", avg) for n in names]
    (out / "runs").mkdir(exist_ok=True)

    def one(t):
        n, kind, w = t
        return n, kind, w, route(bins, n, w, out / "runs" / f"{n}-{kind}.json")

    with ThreadPoolExecutor(jobs) as ex:
        res = list(ex.map(one, tasks))
    j: dict = {"averaged_weights": avg, "boards": {}}
    for n, kind, w, m in res:
        j["boards"].setdefault(n, {})[kind] = {"weights": w, **m}
    (out / "eval.json").write_text(json.dumps(j, indent=1))


# ---------------------------------------------------------------- candidates

def candidates(st: list[dict], hi: float = 2.0, lo: float = 0.5, floor: float = 0.05) -> dict:
    """Event kinds whose rate per 100 mm of copper in the engine is at least `hi` times the hand's
    (candidate penalties) or at most `lo` times (candidate preferences) on a majority of the boards
    where either side has a rate >= `floor`. Rates per board are kept for the report."""
    rows = []
    for k in EVENT_KINDS:
        per = []
        for b in st:
            he, ee = b["hand"]["events"], b["engine"]["totals"]["events"]
            hl, el = max(he.get("length_mm", 0.0), 1e-9) / 100, max(ee.get("length_mm", 0.0), 1e-9) / 100
            h, e = he.get(k, 0.0) / hl, ee.get(k, 0.0) / el
            per.append((b["board"], h, e))
        seen = [(n, h, e) for n, h, e in per if max(h, e) >= floor]
        more = sum(1 for _, h, e in seen if e >= hi * h)
        less = sum(1 for _, h, e in seen if e <= lo * h)
        kind = "penalty" if seen and more > len(seen) / 2 else "preference" if seen and less > len(seen) / 2 else ""
        rows.append({"event": k, "candidate": kind, "boards": len(seen), "engine_more": more, "engine_less": less,
                     "rates": [{"board": n, "hand": round(h, 4), "engine": round(e, 4)} for n, h, e in per]})
    return {"penalties": [r for r in rows if r["candidate"] == "penalty"],
            "preferences": [r for r in rows if r["candidate"] == "preference"], "all": rows}


# ---------------------------------------------------------------- report

def snippet(w: dict, note: str) -> str:
    body = ", ".join(f"{k} = {v:.3f}" for k, v in sorted(w.items()))
    return f"# Learned by the S3b learning loop ({note}). Not applied by default:\n# copy into topor.toml to try it.\n[factors]\nweight = {{ {body} }}\n"


def _row(n: str, kind: str, m: dict, w: dict) -> str:
    ws = ", ".join(f"{k} {v:.2f}" for k, v in sorted(w.items())) or "defaults"
    return (f"| {n} | {kind} | {ws} | {m['routed']}/{m['connections']} | {m['vias']} | {m['detour_median']:.3f} | "
            f"{m['detour_p90']:.3f} | {m['same_layer']} |")


def report(out: Path, names: list[str]) -> str:
    ev = json.loads((out / "eval.json").read_text())
    lines = ["# S3b learning loop: default vs learned weights", "",
             "Full engine (`topo_route --timeout 0`) on each held-out board. detour = length / straight pad distance of",
             "the routed connections; same-layer = different-net track segments crossing on one layer (must be 0).", "",
             "## Per fold: held-out board", "",
             "| held out | weights | values | routed | vias | detour median | detour p90 | same-layer |", "|---|---|---|---|---|---|---|---|"]
    for n in names:
        b = ev["boards"][n]
        lines.append(_row(n, "default", b["default"], {}))
        lines.append(_row(n, "learned", b["learned"], b["learned"]["weights"]))
        lines.append(f"| | verdict | {verdict(b['default'], b['learned'])} | | | | | |")
    lines += ["", "## Weights averaged over the folds, every board", "",
              "| board | weights | values | routed | vias | detour median | detour p90 | same-layer |", "|---|---|---|---|---|---|---|---|"]
    for n in names:
        b = ev["boards"][n]
        lines.append(_row(n, "default", b["default"], {}))
        lines.append(_row(n, "averaged", b["averaged"], ev["averaged_weights"]))
        lines.append(f"| | verdict | {verdict(b['default'], b['averaged'])} | | | | | |")
    lines += ["", "## Learning curves", ""]
    for n in names:
        f = json.loads((out / f"fold-{n}.json").read_text())["result"]
        its = f["iterations"]
        ls = [round(statistics.fmean(i["loss"]), 3) for i in its]
        lines.append(f"* hold-out {n}: {len(its)} iterations{' (converged)' if f['converged'] else ''}, mean loss-augmented "
                     f"training loss per iteration {ls}; last iterate {', '.join(f'{k} {v:.2f}' for k, v in sorted(f['last'].items()))}")
    st = out / "stats.json"
    if st.exists():
        c = candidates(json.loads(st.read_text()))
        lines += ["", "## Candidate factors (hand vs engine event rates; engine = P2 + P3 at default weights on the witness board)", ""]
        for kind in ("penalties", "preferences"):
            lines.append(f"* candidate {kind}: " + (", ".join(r["event"] for r in c[kind]) or "none"))
        lines += ["", "| event | candidate | " + " | ".join(r["board"] for r in c["all"][0]["rates"]) + " |",
                  "|---|---|" + "---|" * len(c["all"][0]["rates"])]
        for r in c["all"]:
            cells = " | ".join(f"{x['hand']:.3f} / {x['engine']:.3f}" for x in r["rates"])
            lines.append(f"| {r['event']} | {r['candidate'] or '-'} | {cells} |")
        lines.append("")
        lines.append("Cells: hand / engine, events per 100 mm of copper (excess_length_mm: mm per 100 mm, i.e. %).")
    text = "\n".join(lines) + "\n"
    (out / "report.md").write_text(text)
    (out / "learned-avg.toml").write_text(snippet(ev["averaged_weights"], f"averaged over the hold-out folds of {', '.join(names)}"))
    return text


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("step", choices=["all", "folds", "stats", "eval", "report"])
    ap.add_argument("--out", type=Path, default=Path("/home/paul/work/kicad/plugins/topor/scratch/topo-s3b"))
    ap.add_argument("--boards", default=",".join(BOARDS))
    ap.add_argument("--bins", type=Path, default=ROOT / "target" / "release" / "examples")
    ap.add_argument("--jobs", type=int, default=3)
    ap.add_argument("--learn-args", default="", help="extra flags for examples/learn, e.g. '--eta 5 --iters 12'")
    a = ap.parse_args()
    names = a.boards.split(",")
    a.out.mkdir(parents=True, exist_ok=True)
    extra = a.learn_args.split()
    if a.step in ("all", "stats"):
        stats(a.out, names, a.bins)
    if a.step in ("all", "folds"):
        folds(a.out, names, a.bins, a.jobs, extra)
    if a.step in ("all", "eval"):
        evaluate(a.out, names, a.bins, a.jobs)
    if a.step in ("all", "report"):
        print(report(a.out, names))


if __name__ == "__main__":
    main()
