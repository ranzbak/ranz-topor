"""C0 compare tool: the frozen baseline engine and the candidate engine on the same input, plan first.

    .venv/bin/python -m harness.compare <board key | .kicad_pcb> [--seeds 0,1,..] [--set SECTION.KEY=VALUE ...]
                                        [--cand-set SECTION.KEY=VALUE ...]
                                        [--plan-only] [--baseline-only] [--out DIR] [--json FILE]

- **Baseline** = the detached worktree `scratch/baseline-wt` (tag baseline-2026-11-09) with its own engine
  (`<tree>/.engine-site`), harness and plugin code. It cannot stop after the plan, so it always runs in full and
  is cached under `scratch/compare-cache/<input hash>/<settings hash>/s<k>/` (main tree): every (input, settings,
  seed) runs once; `--plan-only` reads the plan out of the same cached run.
- **Candidate** = this tree with its engine in `<this tree>/.engine-site` (`maturin build` + `pip install
  --target`); results go to `scratch/compare-runs/<input>-<input hash>/<settings hash>/<plan|full>/s<k>/` of this
  tree and are re-run every time. `--plan-only` adds `run.stop_after=plan` (the candidate ends every run after the
  plan of its first cycle, no copper).
- Every run is `tools/route_board.sh` of its own tree with `capacity.cut_report=<run>/cuts` (the per-layer cut
  test of every cycle's plan, `harness/cutclass.py`), under the scorecard's fixed limits (`harness.score_runs`:
  CPU clock, 2400 s, RAYON_NUM_THREADS=4; never changed here), at most 2 runs at once (the baseline and the
  candidate of a seed side by side), all under one `flock` on the shared board lock.

Plan metrics (`plan_metrics`), read from the cut report of one (run, cycle) plan: `--plan-only` compares the plan
of run 1, cycle 1 (the reference cycle with the plain weights; the only plan a stop_after run makes); a full
compare the picked (run, cycle) of each side. Predicted violations by class: vias without a site, over-full cuts
of the per-layer cut test (sub-classes as cutclass: static = the wire alone overflows, neck = fits at the neck
floors, other), over-full edge-layers of the per-edge model, different-net band crossings on one layer; then
band crossings on different layers (the plan's layered crossings), planned vias, topological length (sum of the
planned rubber bands), CPU time. The candidate's own `plan metrics:` line (topology crossings, unlayerable
crossings, projection length) is added when it is there (the baseline has none). Without `--plan-only` the drawn
result too: routed, KiCad DRC (routing_new, unconnected), engine violations by class, vias, cleanup.
Complexity: the runs; the band crossing count is O(S log S + pairs in shared grid cells) per plan.
"""
from __future__ import annotations

import argparse
import concurrent.futures as cf
import fcntl
import glob
import hashlib
import json
import math
import os
import shutil
import subprocess
import sys
import time
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent  # the candidate tree


def _main_tree() -> Path:
    """The repository's main working tree (where scratch/ with the boards, the baseline and the cache lives)."""
    try:
        common = subprocess.run(["git", "rev-parse", "--path-format=absolute", "--git-common-dir"], cwd=ROOT,
                                capture_output=True, text=True, timeout=20, check=True).stdout.strip()
        return Path(common).parent
    except Exception:  # noqa: BLE001
        return ROOT


MAIN = _main_tree()
BASELINE_TREE = MAIN / "scratch" / "baseline-wt"
CACHE = MAIN / "scratch" / "compare-cache"
RUNS = ROOT / "scratch" / "compare-runs"
LOCK = Path("/tmp/claude-1000/-home-paul-work-kicad-plugins-topor/903b8b7b-c66c-489d-bf05-bdd6dc475bac/scratchpad/board.lock")
JOBS = 2  # runs at once
OVER = -0.001  # mm, as harness.cutclass
PLAN_PREFIX = "plan metrics: "


def _limits():
    from harness.score_runs import CLOCK, THREADS, TIMEOUT_S
    return CLOCK, TIMEOUT_S, THREADS


# ---------------------------------------------------------------- keys
def resolve_input(board: str) -> Path:
    """A BENCH_BOARDS key or a .kicad_pcb path -> the pcb path."""
    p = Path(board)
    if p.suffix == ".kicad_pcb":
        if not p.is_file():
            raise SystemExit(f"no such board: {board}")
        return p.resolve()
    from harness.paths import BENCH_BOARDS
    if board in BENCH_BOARDS:
        return Path(BENCH_BOARDS[board]).resolve()
    raise SystemExit(f"not a benchmark board key or a .kicad_pcb file: {board}")


def input_hash(pcb: Path) -> str:
    """sha256[:12] over the pcb and its project files of the same stem (.kicad_pro, .kicad_dru): what the run reads."""
    h = hashlib.sha256()
    for ext in (".kicad_pcb", ".kicad_pro", ".kicad_dru"):
        f = pcb.with_suffix(ext)
        h.update(ext.encode())
        h.update(f.read_bytes() if f.is_file() else b"<none>")
    return h.hexdigest()[:12]


def norm_sets(sets) -> list[str]:
    """`section.key=value` settings, whitespace-trimmed, sorted, duplicates dropped (the last value wins)."""
    d = {}
    for s in sets:
        k, _, v = s.partition("=")
        d[k.strip()] = v.strip()
    return [f"{k}={d[k]}" for k in sorted(d)]


def settings_hash(sets, *, clock: str, timeout_s: float, threads: int, baseline: str = "") -> str:
    """sha256[:12] of what decides a baseline run besides the input and the seed: the user settings (order-free),
    the fixed limits and the baseline commit."""
    blob = json.dumps({"sets": norm_sets(sets), "clock": clock, "timeout_s": float(timeout_s), "threads": int(threads),
                       "cut_report": True, "baseline": baseline}, sort_keys=True)
    return hashlib.sha256(blob.encode()).hexdigest()[:12]


def baseline_commit(tree: Path = BASELINE_TREE) -> str:
    try:
        return subprocess.run(["git", "rev-parse", "HEAD"], cwd=tree, capture_output=True, text=True, timeout=20,
                              check=True).stdout.strip()
    except Exception:  # noqa: BLE001
        return "unknown"


def cache_dir(ihash: str, shash: str, seed: int, root: Path = CACHE) -> Path:
    return root / ihash / shash / f"s{seed}"


def cached(d: Path) -> bool:
    """A finished baseline run: its meta.json (written last) says ok."""
    m = d / "meta.json"
    if not m.is_file():
        return False
    try:
        return json.loads(m.read_text()).get("ok") is True
    except ValueError:
        return False


# ---------------------------------------------------------------- runs
def run_command(tree: Path, pcb: Path, out: Path, seed: int, sets, *, plan_only: bool, png: bool, name: str) -> list:
    clock, timeout_s, _ = _limits()
    cmd = [str(tree / "tools" / "route_board.sh"), str(pcb), str(out), "--clock", clock, "--timeout", f"{timeout_s:g}",
           "--name", name, "--set", f"capacity.cut_report={out / 'cuts'}"]
    if seed:
        cmd += ["--set", f"optimize.seed={seed}"]
    for kv in norm_sets(sets):
        cmd += ["--set", kv]
    if plan_only:
        cmd += ["--set", "run.stop_after=plan"]
    if not png:
        cmd.append("--no-png")
    return cmd


def run_env(tree: Path, out: Path) -> dict:
    _, _, threads = _limits()
    return {**os.environ, "RAYON_NUM_THREADS": str(threads), "TOPOR_ENGINE_SITE": str(tree / ".engine-site"),
            "TOPOR_REPORT_ROOT": str(out / "reports"), "PYTHONDONTWRITEBYTECODE": "1"}


def _run(tree: Path, pcb: Path, out: Path, seed: int, sets, *, plan_only: bool, png: bool, name: str) -> dict:
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    cmd = run_command(tree, pcb, out, seed, sets, plan_only=plan_only, png=png, name=name)
    t = time.monotonic()
    with (out / "run.log").open("w") as log:
        log.write(" ".join(cmd) + "\n")
        log.flush()
        r = subprocess.run(cmd, env=run_env(tree, out), stdout=log, stderr=subprocess.STDOUT, cwd=tree)
    return {"ok": r.returncode == 0, "rc": r.returncode, "cmd": cmd, "wall_s": round(time.monotonic() - t, 2),
            "load_avg": list(os.getloadavg()), "finished": time.strftime("%Y-%m-%d %H:%M:%S")}


def run_baseline(pcb: Path, seed: int, sets, *, ihash: str, shash: str, name: str, tree: Path = BASELINE_TREE,
                 root: Path = CACHE) -> Path:
    """The cached baseline run (full, with PNG) of (input, settings, seed); runs it when it is not cached."""
    d = cache_dir(ihash, shash, seed, root)
    if cached(d):
        return d
    meta = _run(tree, pcb, d, seed, sets, plan_only=False, png=True, name=name)
    meta.update({"input": str(pcb), "input_hash": ihash, "settings_hash": shash, "sets": norm_sets(sets),
                 "seed": seed, "baseline_tree": str(tree), "baseline_commit": baseline_commit(tree), "dir": str(d)})
    # meta.json is written last: a run killed half-way is not a cache hit (the next compare runs it again)
    (d / "meta.json").write_text(json.dumps(meta, indent=1))
    if not meta["ok"]:
        raise RuntimeError(f"baseline run failed (exit {meta['rc']}): see {d / 'run.log'}")
    return d


def run_candidate(pcb: Path, seed: int, sets, *, ihash: str, shash: str, name: str, plan_only: bool,
                  tree: Path = ROOT) -> Path:
    d = RUNS / f"{name}-{ihash}" / shash / ("plan" if plan_only else "full") / f"s{seed}"
    meta = _run(tree, pcb, d, seed, sets, plan_only=plan_only, png=not plan_only, name=name)
    meta.update({"input": str(pcb), "seed": seed, "sets": norm_sets(sets), "plan_only": plan_only, "dir": str(d)})
    (d / "meta.json").write_text(json.dumps(meta, indent=1))
    if not meta["ok"]:
        raise RuntimeError(f"candidate run failed (exit {meta['rc']}): see {d / 'run.log'}")
    return d


class BoardLock:
    """flock(2) on the shared board lock (the same lock `flock(1)` takes), held for all runs of one compare."""

    def __init__(self, path: Path | None):
        self.path = path
        self.fd = None

    def __enter__(self):
        if self.path is None:
            return self
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.fd = os.open(self.path, os.O_RDWR | os.O_CREAT, 0o664)
        try:
            fcntl.flock(self.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print(f"waiting for the board lock {self.path} ...", flush=True)
            fcntl.flock(self.fd, fcntl.LOCK_EX)
        return self

    def __exit__(self, *exc):
        if self.fd is not None:
            fcntl.flock(self.fd, fcntl.LOCK_UN)
            os.close(self.fd)
            self.fd = None
        return False


# ---------------------------------------------------------------- metrics
def find_result(run: Path) -> Path | None:
    hits = sorted(glob.glob(str(run / "reports" / "**" / "result.json"), recursive=True))
    return Path(hits[-1]) if hits else None


def _proper_cross(a, b, c, d, eps=1e-9) -> bool:
    """Segments ab and cd cross at a point inside both (touching at an end point does not count)."""
    def orient(p, q, r):
        return (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    o1, o2, o3, o4 = orient(a, b, c), orient(a, b, d), orient(c, d, a), orient(c, d, b)
    return ((o1 > eps and o2 < -eps) or (o1 < -eps and o2 > eps)) and ((o3 > eps and o4 < -eps) or (o3 < -eps and o4 > eps))


def band_crossings(stretches: list[dict], cell: float = 2.0) -> tuple[int, int]:
    """(crossings on different layers, crossings on one layer) between the planned bands of different nets, each pair
    of stretches counted once per crossing point. A grid of `cell` mm buckets the segments."""
    segs = []
    for s in stretches:
        b = s.get("band") or []
        for j in range(len(b) - 1):
            segs.append((s["id"], s["net"], s["layer"], tuple(b[j]), tuple(b[j + 1])))
    grid: dict = {}
    for i, (_, _, _, a, b) in enumerate(segs):
        x0, x1 = sorted((a[0], b[0]))
        y0, y1 = sorted((a[1], b[1]))
        for gx in range(math.floor(x0 / cell), math.floor(x1 / cell) + 1):
            for gy in range(math.floor(y0 / cell), math.floor(y1 / cell) + 1):
                grid.setdefault((gx, gy), []).append(i)
    seen = set()
    other = same = 0
    for ids in grid.values():
        for x in range(len(ids)):
            for y in range(x + 1, len(ids)):
                i, j = ids[x], ids[y]
                si, sj = segs[i], segs[j]
                if si[1] == sj[1] or (i, j) in seen:
                    continue
                seen.add((i, j))
                if _proper_cross(si[3], si[4], sj[3], sj[4]):
                    if si[2] == sj[2]:
                        same += 1
                    else:
                        other += 1
    return other, same


def cut_classes(final: dict) -> dict:
    """Over-full cuts of the per-layer cut test by sub-class (as harness.cutclass) and their deficit."""
    out = Counter()
    deficit = 0.0
    for L in final.get("layers", []):
        for c in L["cuts"]:
            if c["slack"] >= OVER:
                continue
            deficit += -c["slack"]
            if any(it[6] < OVER for it in c["items"]):
                out["static"] += 1
            elif c["cap"] - c["need_floor"] >= OVER:
                out["neck"] += 1
            else:
                out["other"] += 1
    return {"total": sum(out.values()), "static": out["static"], "neck": out["neck"], "other": out["other"],
            "deficit_mm": round(deficit, 3)}


def plan_from_report(final: dict) -> dict:
    """Plan metrics of a cut report's `final` part (the plan handed to the drawing)."""
    st = final.get("stretches", [])
    length = sum(math.dist(s["band"][j], s["band"][j + 1]) for s in st for j in range(len(s["band"]) - 1))
    other, same = band_crossings(st)
    vias = final.get("vias", [])
    return {
        "predicted": {"via_site": sum(1 for v in vias if not v["site"]), "cut_overfull": cut_classes(final),
                      "edge_overfull": len(final.get("edge_over", [])), "same_layer_crossings": same},
        "crossings": other,
        "vias": len(vias),
        "topo_length_mm": round(length, 3),
        "stretches": len(st),
        "nosite_vias_at": [v["at"] for v in vias if not v["site"]],
    }


def plan_metrics(run: Path, which: str) -> dict:
    """The plan of `run` (a route_board output with `cuts/`): which = "first" (run 1, cycle 1) or "picked" (the
    (run, cycle) the result came from)."""
    from harness.cutclass import picked
    rp = find_result(run)
    if rp is None:
        raise RuntimeError(f"no result.json under {run}")
    res = json.loads(rp.read_text())
    route = res.get("route", res)
    k, c = (1, 1) if which == "first" else picked({"route": route})
    rep_path = run / "cuts" / f"run{k}" / f"cycle{c}.json"
    if not rep_path.is_file():
        raise RuntimeError(f"no cut report {rep_path}")
    rep = json.loads(rep_path.read_text())
    m = plan_from_report(rep.get("final") or {})
    m.update({"run": k, "cycle": c, "cut_report": str(rep_path), "result": str(rp)})
    lines = [json.loads(w[len(PLAN_PREFIX):]) for w in route.get("warnings", []) if w.startswith(PLAN_PREFIX)]
    eng = [x for x in lines if x.get("round") == c]
    m["engine"] = eng[0] if len(eng) == 1 else None
    mt = sorted(glob.glob(str(run / "*--metrics.json")))
    if mt:
        mj = json.loads(Path(mt[0]).read_text())
        m["cpu_s"] = mj.get("router_cpu_s")
    return m


def drawn_metrics(run: Path) -> dict:
    mt = sorted(glob.glob(str(run / "*--metrics.json")))
    if not mt:
        return {}
    m = json.loads(Path(mt[0]).read_text())
    rp = find_result(run)
    viol = Counter()
    if rp is not None:
        route = json.loads(rp.read_text()).get("route", {})
        viol = Counter(v.get("class") or v.get("kind") or "?" for v in route.get("violations", []))
    return {"routed": m.get("routed"), "connections": m.get("connections"), "unconnected": m.get("unconnected"),
            "routing_new": m.get("routing_new"), "drc_by_type_new": m.get("drc_by_type_new"), "vias": m.get("vias"),
            "cleanup_unstable": m.get("cleanup_unstable"), "violations": sum(viol.values()),
            "violations_by_class": dict(viol), "cpu_s": m.get("router_cpu_s"), "stop_reason": m.get("stop_reason"),
            "png": m.get("png")}


# ---------------------------------------------------------------- table
def rows(b: dict, c: dict | None, plan_only: bool) -> list[tuple]:
    """(label, baseline, candidate) rows of one seed."""
    def g(m, *path):
        for p in path:
            if m is None:
                return None
            m = m.get(p)
        return m
    bp, cp = b["plan"], (c or {}).get("plan")
    r = [
        ("predicted: vias without a site", g(bp, "predicted", "via_site"), g(cp, "predicted", "via_site")),
        ("predicted: over-full cuts", g(bp, "predicted", "cut_overfull", "total"), g(cp, "predicted", "cut_overfull", "total")),
        ("  static / neck / other", _triple(g(bp, "predicted", "cut_overfull")), _triple(g(cp, "predicted", "cut_overfull"))),
        ("  cut deficit (mm)", g(bp, "predicted", "cut_overfull", "deficit_mm"), g(cp, "predicted", "cut_overfull", "deficit_mm")),
        ("predicted: over-full edge-layers", g(bp, "predicted", "edge_overfull"), g(cp, "predicted", "edge_overfull")),
        ("predicted: same-layer band crossings", g(bp, "predicted", "same_layer_crossings"), g(cp, "predicted", "same_layer_crossings")),
        ("crossings (bands, layered)", g(bp, "crossings"), g(cp, "crossings")),
        ("planned vias", g(bp, "vias"), g(cp, "vias")),
        ("topological length (mm)", g(bp, "topo_length_mm"), g(cp, "topo_length_mm")),
        ("engine: topology crossings", g(bp, "engine", "crossings"), g(cp, "engine", "crossings")),
        ("engine: unlayerable crossings", g(bp, "engine", "unlayerable_crossings"), g(cp, "engine", "unlayerable_crossings")),
        ("CPU s" + (" (baseline: full run)" if plan_only else ""), g(bp, "cpu_s"), g(cp, "cpu_s")),
    ]
    if not plan_only:
        bd, cd = b.get("drawn"), (c or {}).get("drawn")
        r += [
            ("drawn: routed / connections", _frac(bd), _frac(cd)),
            ("drawn: DRC routing_new", g(bd, "routing_new"), g(cd, "routing_new")),
            ("drawn: DRC unconnected", g(bd, "unconnected"), g(cd, "unconnected")),
            ("drawn: engine violations", g(bd, "violations"), g(cd, "violations")),
            ("drawn: vias", g(bd, "vias"), g(cd, "vias")),
            ("drawn: cleanup-unstable", g(bd, "cleanup_unstable"), g(cd, "cleanup_unstable")),
        ]
    return r


def _triple(d):
    return None if d is None else f"{d['static']}/{d['neck']}/{d['other']}"


def _frac(d):
    return None if not d else f"{d.get('routed')}/{d.get('connections')}"


def _delta(a, b):
    if isinstance(a, (int, float)) and isinstance(b, (int, float)) and not isinstance(a, bool):
        d = b - a
        return f"{d:+.3f}".rstrip("0").rstrip(".") if isinstance(d, float) else f"{d:+d}"
    return ""


def table(rs: list[tuple], title: str) -> str:
    cell = lambda v: "-" if v is None else str(v)  # noqa: E731
    w0 = max(len(r[0]) for r in rs)
    w1 = max(8, *(len(cell(r[1])) for r in rs))
    w2 = max(9, *(len(cell(r[2])) for r in rs))
    out = [title, f"{'':{w0}}  {'baseline':>{w1}}  {'candidate':>{w2}}  delta"]
    for lab, a, b in rs:
        out.append(f"{lab:{w0}}  {cell(a):>{w1}}  {cell(b):>{w2}}  {_delta(a, b)}")
    return "\n".join(out)


# ---------------------------------------------------------------- main
def main(argv: list | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("input", help="BENCH_BOARDS key or .kicad_pcb (a board copy or an abstract crop)")
    ap.add_argument("--seeds", default="0", help="comma list of seeds (0 = the default run; k = optimize.seed=k)")
    ap.add_argument("--set", action="append", default=[], metavar="SECTION.KEY=VALUE",
                    help="setting for both engines (part of the cache key)")
    ap.add_argument("--cand-set", action="append", default=[], metavar="SECTION.KEY=VALUE",
                    help="setting for the candidate only (a setting the baseline does not know, e.g. a new option "
                         "under test); not part of the baseline cache key")
    ap.add_argument("--plan-only", action="store_true",
                    help="compare the plans only (candidate: run.stop_after=plan; baseline: the plan of its cached full run)")
    ap.add_argument("--baseline-only", action="store_true", help="run / read the baseline only (fills the cache)")
    ap.add_argument("--json", type=Path, default=None, help="write all metrics here")
    ap.add_argument("--no-lock", action="store_true", help="do not take the shared board lock")
    a = ap.parse_args(argv)
    pcb = resolve_input(a.input)
    name = pcb.stem
    seeds = [int(x) for x in a.seeds.split(",") if x.strip()]
    clock, timeout_s, threads = _limits()
    ihash = input_hash(pcb)
    shash = settings_hash(a.set, clock=clock, timeout_s=timeout_s, threads=threads, baseline=baseline_commit())
    # The candidate's run directory also keys its own settings.
    cshash = shash if not a.cand_set else settings_hash(a.set + a.cand_set, clock=clock, timeout_s=timeout_s,
                                                          threads=threads, baseline=baseline_commit())
    which = "first" if a.plan_only else "picked"
    print(f"compare {pcb} (input {ihash}, settings {shash}{', ' + ' '.join(norm_sets(a.set)) if a.set else ''}); "
          f"limits {clock} {timeout_s:g} s, {threads} threads, {JOBS} runs at once"
          + (f"; candidate only: {' '.join(norm_sets(a.cand_set))}" if a.cand_set else ""), flush=True)
    jobs = []
    for k in seeds:
        jobs.append(("baseline", k))
        if not a.baseline_only:
            jobs.append(("candidate", k))
    dirs: dict = {}
    with BoardLock(None if a.no_lock else LOCK), cf.ThreadPoolExecutor(max_workers=JOBS) as ex:
        fut = {}
        hit = {k for k in seeds if cached(cache_dir(ihash, shash, k))}
        for side, k in jobs:
            if side == "baseline":
                fut[ex.submit(run_baseline, pcb, k, a.set, ihash=ihash, shash=shash, name=name)] = (side, k)
            else:
                fut[ex.submit(run_candidate, pcb, k, a.set + a.cand_set, ihash=ihash, shash=cshash, name=name,
                              plan_only=a.plan_only)] = (side, k)
        rc = 0
        for f in cf.as_completed(fut):
            side, k = fut[f]
            try:
                dirs[(side, k)] = f.result()
                how = "cached" if side == "baseline" and k in hit else "ran"
                print(f"  {side} s{k} ({how}): {dirs[(side, k)]}", flush=True)
            except Exception as e:  # noqa: BLE001
                print(f"  {side} s{k}: {e}", file=sys.stderr, flush=True)
                rc = 1
    report = {"input": str(pcb), "input_hash": ihash, "settings_hash": shash, "sets": norm_sets(a.set),
              "candidate_sets": norm_sets(a.cand_set),
              "plan_only": a.plan_only, "plan": which, "seeds": {}}
    for k in seeds:
        sides = {}
        for side in ("baseline", "candidate"):
            d = dirs.get((side, k))
            if d is None:
                continue
            m = {"dir": str(d), "plan": plan_metrics(d, which)}
            if not a.plan_only:
                m["drawn"] = drawn_metrics(d)
            sides[side] = m
        report["seeds"][k] = sides
        if "baseline" in sides:
            what = "run 1, cycle 1" if a.plan_only else (
                f"picked: baseline run {sides['baseline']['plan']['run']} cycle {sides['baseline']['plan']['cycle']}"
                + (f", candidate run {sides['candidate']['plan']['run']} cycle {sides['candidate']['plan']['cycle']}"
                   if "candidate" in sides else ""))
            print()
            print(table(rows(sides["baseline"], sides.get("candidate"), a.plan_only), f"seed {k} — plan of {what}"))
    if a.json:
        a.json.parent.mkdir(parents=True, exist_ok=True)
        a.json.write_text(json.dumps(report, indent=1))
    return rc


if __name__ == "__main__":
    raise SystemExit(main())
