"""The five S3c tools as plain functions: route, report, explain, set_factor, review.

No MCP SDK import here. Rules (plan S3c): board copies only (paths under ~/work/kicad/projects are refused),
results land next to the copy, nothing runs inside the optimizer, and every tool has a manual equivalent
(topor.toml + the run report; docs/dev/mcp.md).

`route` runs `tools/route_board.sh` in a subprocess: the router and pcbnew stay out of the server process
(stdout is the MCP protocol) and a crash cannot take the server down.
"""
from __future__ import annotations

import json
import math
import os
import re
import shutil
import subprocess
import sys
import time
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
for _p in (REPO / "plugin", REPO):
    if str(_p) not in sys.path:
        sys.path.insert(0, str(_p))

VERDICTS = ("good", "bad", "neutral")
REPORT_LEVELS = ("off", "info", "warning")
# Per-unit defaults (docs/user/configuration.md [factors]); used only to rank derived factor events in `explain`.
DEFAULT_WEIGHTS = {"length": 1.0, "via": 25.0, "crossing": 20.0, "necking": 30.0}
_FACTOR_RE = re.compile(r"^[a-z][a-z0-9_]*$")


class ToolError(ValueError):
    """A refusal or bad input; the message is shown to the caller as is."""


# ------------------------------------------------------------------ guards and state

def protected_roots() -> list[Path]:
    roots = [Path.home() / "work" / "kicad" / "projects"]
    roots += [Path(p) for p in os.environ.get("TOPOR_MCP_PROTECTED", "").split(os.pathsep) if p]
    return [r.resolve() for r in roots]


def check_board_path(path: str | Path) -> Path:
    """Resolve `path` (symlinks too) and refuse anything under a protected root."""
    p = Path(path).expanduser().resolve()
    for root in protected_roots():
        if p == root or root in p.parents:
            raise ToolError(f"refused: {p} is under {root}, which topor never touches. Copy the project to a "
                            f"scratch directory first and pass the copy.")
    return p


def _state_file() -> Path:
    d = Path(os.environ.get("TOPOR_MCP_STATE") or Path.home() / ".cache" / "topor" / "mcp")
    d.mkdir(parents=True, exist_ok=True)
    return d / "runs.json"


def _load_index() -> dict:
    try:
        return json.loads(_state_file().read_text())
    except (OSError, ValueError):
        return {}


def _remember(run_id: str, info: dict) -> None:
    idx = _load_index()
    idx[run_id] = info
    _state_file().write_text(json.dumps(idx, indent=1))
    global _last_board
    _last_board = info["board_copy"]


_last_board: str | None = None


def _run(run_id: str) -> dict:
    info = _load_index().get(run_id)
    if info is None:
        raise ToolError(f"unknown run_id {run_id!r}; known: {', '.join(sorted(_load_index())[-5:]) or 'none'}")
    check_board_path(info["board_copy"])
    return info


def _result(info: dict) -> dict:
    d = info.get("report_dir")
    p = Path(d) / "result.json" if d else None
    if p is None or not p.is_file():
        raise ToolError(f"run {info['run_id']} has no result.json (status: {info.get('status')}); see {d or info['out_dir']}")
    return json.loads(p.read_text())


def _route_part(res: dict) -> dict:
    return res.get("route") or res


# ------------------------------------------------------------------ 1. route

def route(board_copy: str, options: dict | None = None) -> dict:
    """Route a copy of a board headless; return the summary, PNG path and (if the report has one) the variant table."""
    pcb = check_board_path(board_copy)
    if pcb.suffix != ".kicad_pcb" or not pcb.is_file():
        raise ToolError(f"{pcb} is not a .kicad_pcb file")
    opts = dict(options or {})
    unknown = set(opts) - {"timeout_s", "png", "set", "whole_project", "run_timeout_s"}
    if unknown:
        raise ToolError(f"unknown options {sorted(unknown)}; allowed: timeout_s, png, set, whole_project, run_timeout_s")
    run_id = f"{pcb.stem}-{time.strftime('%Y%m%d-%H%M%S')}"
    out_dir = pcb.parent.parent / f"{pcb.parent.name}.topor-mcp" / run_id  # outside the source tree (strip.prepare hashes it)
    sets = _toml_overrides(pcb.parent)
    for k, v in (opts.get("set") or {}).items():
        sets[k] = v
    cmd = ["bash", str(REPO / "tools" / "route_board.sh"), str(pcb), str(out_dir), "--name", pcb.stem]
    if opts.get("timeout_s") is not None:
        cmd += ["--timeout", str(float(opts["timeout_s"]))]
    if opts.get("png", True) is False:
        cmd.append("--no-png")
    if opts.get("whole_project"):
        cmd.append("--whole-project")
    for k, v in sets.items():
        cmd += ["--set", f"{k}={json.dumps(v)}"]
    from .drc import _cli_env
    # pcbnew / kicad-cli of the route run in an isolated KiCad home (they rewrite the user's config otherwise)
    env = dict(_cli_env(out_dir / "kicad-home", keep_display=True), TOPOR_REPORT_ROOT=str(out_dir / "reports"))
    try:
        cp = subprocess.run(cmd, capture_output=True, text=True, env=env, stdin=subprocess.DEVNULL,
                            timeout=float(opts.get("run_timeout_s", 3600)), cwd=REPO)
    except subprocess.TimeoutExpired:
        raise ToolError(f"route did not finish within run_timeout_s; partial files in {out_dir}") from None
    mfile = out_dir / f"{pcb.stem}--metrics.json"
    if cp.returncode != 0 or not mfile.is_file():
        raise ToolError(f"route failed (exit {cp.returncode}): {(cp.stderr or cp.stdout)[-1500:]}")
    m = json.loads(mfile.read_text())
    info = {"run_id": run_id, "board_copy": str(pcb), "out_dir": str(out_dir), "report_dir": m.get("report_dir"),
            "status": m.get("status"), "name": pcb.stem}
    _remember(run_id, info)
    out = {
        "run_id": run_id, "status": m["status"], "routed": m["routed"], "connections": m["connections"],
        "routed_pct": m.get("routed_pct"), "unconnected": m["unconnected"], "unconnected_hand": m.get("unconnected_hand"),
        "routing_new": m["routing_new"], "vias": m["vias"], "vias_hand": m.get("vias_hand"), "detour": m.get("detour"),
        "runtime_s": m["runtime_s"], "router_s": m.get("router_s"), "stop_reason": m.get("stop_reason"),
        "png": m.get("png"), "report_dir": m.get("report_dir"), "settings_from_topor_toml": sets,
        "variants": None,
    }
    if m.get("stop_reason") == "no engine":
        out["hint"] = ("the engine routed nothing ('no engine'): the installed topor_engine lacks the topological "
                       "core; set TOPOR_ENGINE_SITE to a newer wheel's site directory (docs/dev/route-board.md)")
    if m.get("png_error"):
        out["png_error"] = m["png_error"]
    try:
        out["variants"] = _variants(_result(info))
    except ToolError:
        pass
    return out


def _toml_overrides(board_dir: Path) -> dict:
    """{"section.key": value} for the non-default settings of <board_dir>/topor.toml (read only; never created)."""
    if not (board_dir / "topor.toml").is_file():
        return {}
    from topor.config import load_config
    cfg, _ = load_config(board_dir)
    return dict(cfg.non_defaults())


def _variants(res: dict) -> list | None:
    v = res.get("variants")
    if v is None:
        v = _route_part(res).get("variants")
    return v if isinstance(v, list) and v else None


# ------------------------------------------------------------------ 2. report

_NOISE = re.compile(r"skipped: no other terminal")
_KINDS = (("necking", "necking"), ("ignored_rule", "ignored_rule"), ("ignored rule", "ignored_rule"),
          ("time budget", "time_budget"), ("timed out", "time_budget"), ("time limit", "time_budget"),
          ("capacity", "capacity"), ("planes", "planes"), ("drawing yield", "drawing_yield"),
          ("cut", "cut"), ("overflow", "overflow"), ("squeeze", "squeeze"))


def _kind(line: str) -> str:
    low = line.lower()
    for needle, kind in _KINDS:
        if needle in low:
            return kind
    return "other"


def normalize_reason(reason: str) -> str:
    """Group key for a failure reason: numbers, refs and ids replaced so equal causes collapse."""
    r = re.sub(r"\b(?:pad|item|pour):\S+", lambda m: m.group(0).split(":")[0] + ":<id>", reason or "unknown")
    r = re.sub(r"\bpad [A-Za-z]+\d*\.\w+", "pad <ref>", r)
    r = re.sub(r"-?\d+(?:\.\d+)?", "#", r)
    return r.strip()


def _select_variant(res: dict, variant: str | None) -> dict:
    if variant is None:
        return res
    vs = _variants(res)
    if not vs:
        raise ToolError("this report has no variant table (single result); call report without `variant`")
    for i, v in enumerate(vs):
        if variant in (str(v.get("id", i)), str(v.get("name", ""))):
            sub = v.get("route") or v.get("result")
            return {"route": sub} if isinstance(sub, dict) else res
    raise ToolError(f"unknown variant {variant!r}; have {[v.get('id', v.get('name', i)) for i, v in enumerate(vs)]}")


def report(run_id: str, variant: str | None = None) -> dict:
    info = _run(run_id)
    res = _select_variant(_result(info), variant)
    rt = _route_part(res)
    lines = [str(w) for w in rt.get("warnings") or []]
    skipped = sum(1 for w in lines if _NOISE.search(w))
    warnings = defaultdict(list)
    for w in lines:
        if not _NOISE.search(w):
            warnings[_kind(w)].append(w)
    cuts = [w for k in ("cut", "capacity", "overflow", "squeeze") for w in warnings.get(k, [])]
    groups: dict[str, dict] = {}
    for c in rt.get("connections") or []:
        if c.get("status") == "routed":
            continue
        g = groups.setdefault(normalize_reason(c.get("reason", "")), {"count": 0, "nets": []})
        g["count"] += 1
        if c.get("name") not in g["nets"] and len(g["nets"]) < 8:
            g["nets"].append(c.get("name"))
    failures = [{"reason": k, **v} for k, v in sorted(groups.items(), key=lambda kv: -kv[1]["count"])]
    return {
        "run_id": run_id, "variant": variant, "status": res.get("status"),
        "routed": rt.get("routed"), "failed": rt.get("failed"), "partial": rt.get("partial"),
        "connections": rt.get("connections_total"), "stop_reason": rt.get("stop_reason"),
        "timed_out": rt.get("timed_out"), "warnings": dict(warnings), "warnings_skipped_nets": skipped,
        "cuts": cuts, "failures_by_reason": failures,
        "findings": [f for f in res.get("findings") or [] if f.get("severity") != "info"],
        "report_md": str(Path(info["report_dir"]) / "report.md") if info.get("report_dir") else None,
    }


# ------------------------------------------------------------------ 3. explain

def explain(run_id: str, net: str) -> dict:
    info = _run(run_id)
    rt = _route_part(_result(info))
    nets = rt.get("nets") or []
    want = str(net)
    hits = [n for n in nets if n.get("name") == want or str(n.get("net")) == want]
    if not hits:
        hits = [n for n in nets if want.lower() in str(n.get("name", "")).lower()]
    if not hits:
        raise ToolError(f"no net matching {net!r} in run {run_id}")
    if len({n["net"] for n in hits}) > 1:
        raise ToolError(f"{net!r} is ambiguous: {[n['name'] for n in hits][:10]}")
    n = hits[0]
    weights = dict(DEFAULT_WEIGHTS)
    weights.update(((_toml_overrides(Path(info["board_copy"]).parent) or {}).get("factors.weight")) or {})
    conns = []
    for c in rt.get("connections") or []:
        if c.get("net") != n["net"]:
            continue
        ev = _factor_events(c, weights)
        d = {"from": (c.get("from") or {}).get("label"), "to": (c.get("to") or {}).get("label"),
             "status": c.get("status"), "length_mm": round((c.get("length_nm") or 0) / 1e6, 3), "vias": c.get("vias"),
             "crossings": c.get("crossings"), "factors": ev}
        if c.get("status") != "routed":
            d["reason"] = c.get("reason")
            d["blocking"] = c.get("blocking")
        conns.append(d)
    unrouted = [c for c in conns if c["status"] != "routed"]
    return {"run_id": run_id, "net": n["name"], "net_number": n["net"], "status": n.get("status"),
            "connections": conns, "why_unrouted": [{"conn": f"{c['from']} -> {c['to']}", "reason": c["reason"],
                                                    "blocking": c["blocking"]} for c in unrouted],
            "factor_note": "factor entries are derived from report fields (engine emits no per-wire factor costs yet); "
                           "weights are topor.toml overrides or documented defaults"}


def _factor_events(c: dict, weights: dict) -> list:
    ev = []
    mm = (c.get("length_outside_pads_nm") or 0) / 1e6
    if mm:
        ev.append(("length", mm))
    if c.get("vias"):
        ev.append(("via", c["vias"]))
    if c.get("crossings"):
        ev.append(("crossing", c["crossings"]))
    if c.get("neck_nm"):
        ev.append(("necking", 1))
    rows = [{"factor": f, "amount": round(a, 3), "weight": weights.get(f, 0), "cost": round(a * weights.get(f, 0), 2)}
            for f, a in ev]
    return sorted(rows, key=lambda r: -r["cost"])[:5]


# ------------------------------------------------------------------ 4. set_factor

def set_factor(name: str, weight: float | None = None, report: str | None = None,
               board_copy: str | None = None) -> dict:
    """Write [factors] weight / report for `name` into topor.toml next to the board copy (same format as by hand)."""
    if weight is None and report is None:
        raise ToolError("give weight and/or report")
    if not _FACTOR_RE.match(name or ""):
        raise ToolError(f"bad factor name {name!r}: lowercase letters, digits and underscores")
    if weight is not None and (not isinstance(weight, (int, float)) or isinstance(weight, bool)
                               or not math.isfinite(weight) or weight < 0):
        raise ToolError(f"weight must be a finite number >= 0, got {weight!r}")
    if report is not None and report not in REPORT_LEVELS:
        raise ToolError(f"report must be one of {REPORT_LEVELS}, got {report!r}")
    board = board_copy or _last_board
    if not board:
        raise ToolError("no board: pass board_copy (or call route first in this session)")
    pcb = check_board_path(board)
    from topor.config import CONFIG_NAME, load_config, update_config
    d = pcb.parent if pcb.suffix == ".kicad_pcb" else pcb
    cfg, findings = load_config(d)  # creates a commented default topor.toml when missing
    bad = [f for f in findings if f.code in ("CFG_PARSE", "CFG_WRITE")]
    if bad:
        raise ToolError(bad[0].message)
    changes = {}
    if weight is not None:
        changes["factors.weight"] = {**cfg.get("factors", "weight"), name: float(weight)}
    if report is not None:
        changes["factors.report"] = {**cfg.get("factors", "report"), name: report}
    update_config(d / CONFIG_NAME, changes)
    return {"topor_toml": str(d / CONFIG_NAME), "factor": name,
            "weights": changes.get("factors.weight", cfg.get("factors", "weight")),
            "report_levels": changes.get("factors.report", cfg.get("factors", "report")),
            "next": "call route again to see the effect"}


# ------------------------------------------------------------------ 5. review

def review(run_id: str, spot: str, verdict: str, note: str = "") -> dict:
    if verdict not in VERDICTS:
        raise ToolError(f"verdict must be one of {VERDICTS}, got {verdict!r}")
    info = _run(run_id)
    log = Path(info["board_copy"]).parent / "reviews.jsonl"
    row = {"ts": time.strftime("%Y-%m-%dT%H:%M:%S"), "run_id": run_id, "board": info["name"],
           "spot": str(spot), "verdict": verdict, "note": note}
    with log.open("a", encoding="utf-8") as f:
        f.write(json.dumps(row, ensure_ascii=False) + "\n")
    return {"log": str(log), "lines": sum(1 for _ in log.open(encoding="utf-8")), "recorded": row}


# ------------------------------------------------------------------ 6. DRC feedback (drc, compare, drc_spots, cleanup)
# Everything below works on copies inside the run's out dir (<out_dir>/mcp/); KiCad itself only in the Xephyr
# session of mcp_server.kicad_gui (isolated home, helper by widget label). docs/dev/mcp.md "DRC feedback".

ROWS_MAX = 200
FOCUS_TOL_MM = 1.0  # drc_spots: focus scale floor (mm); KiCad's centring is px-exact at zoom 20
FOCUS_EXTENT_SHARE = 0.5  # + this share of the row's geometry extent (long tracks / polygons: the marker may sit anywhere on them)
MATCH_MIN = 0.5  # default drc_spots match_min: focus_score at focus_d_mm == the scale


def marker_px(f: dict, x_mm: float, y_mm: float):
    """Screen pixel of the board point (x_mm, y_mm) in the view `Session.focus()` measured: its focus point sits at
    spot_px, the scale is mm_px (None: the status bar gave no scale)."""
    kx, ky = f["mm_px"]
    if not kx or not ky:
        return None
    return f["spot_px"][0] + (x_mm - f["focus_mm"][0]) / kx, f["spot_px"][1] + (y_mm - f["focus_mm"][1]) / ky


def focus_score(d_mm, scale_mm: float):
    """Graded focus quality 1 / (1 + (d / scale)^2): 1 on the items, 0.5 at one scale away, smoothly lower further
    (no cutoff); None distance (no geometry) -> 1."""
    return 1.0 if d_mm is None else round(1.0 / (1.0 + (d_mm / max(scale_mm, 1e-6)) ** 2), 3)
DEFAULT_CLEANUP = ("Delete tracks connecting different nets", "Delete redundant vias", "Merge co-linear tracks",
                   "Delete tracks unconnected at one end")


def register_run(routed_copy: str, input_copy: str, result: str | None = None, name: str | None = None) -> dict:
    """Register a board routed elsewhere (e.g. in real KiCad by harness/gui_e2e.py) for drc / drc_spots / cleanup /
    compare: `routed_copy` = the routed .kicad_pcb, `input_copy` = the unrouted input it came from (same project),
    `result` = the plugin's result.json (optional: engine connection ids and violations)."""
    r, i = check_board_path(routed_copy), check_board_path(input_copy)
    for p in (r, i):
        if p.suffix != ".kicad_pcb" or not p.is_file():
            raise ToolError(f"{p} is not a .kicad_pcb file")
    res = check_board_path(result) if result else None
    if res is not None and not res.is_file():
        raise ToolError(f"{res} does not exist")
    run_id = f"{name or r.stem}-reg-{time.strftime('%Y%m%d-%H%M%S')}"
    out_dir = r.parent.parent / f"{r.parent.name}.topor-mcp" / run_id
    out_dir.mkdir(parents=True, exist_ok=True)
    info = {"run_id": run_id, "board_copy": str(r), "out_dir": str(out_dir),
            "report_dir": str(res.parent) if res else None, "status": "registered", "name": name or r.stem,
            "routed_pcb": str(r), "input_pcb": str(i)}
    _remember(run_id, info)
    return {"run_id": run_id, "out_dir": str(out_dir)}


def _boards(info: dict) -> tuple:
    """(input pcb, routed pcb) of a run: register_run's paths, else route_board's <name>-input/ and <name>-topor/."""
    if info.get("routed_pcb"):
        return Path(info["input_pcb"]), Path(info["routed_pcb"])
    out = Path(info["out_dir"])
    inp = sorted(out.glob("*-input/*.kicad_pcb"))
    rout = sorted(out.glob("*-topor/*.kicad_pcb"))
    if len(inp) != 1 or len(rout) != 1:
        raise ToolError(f"run {info['run_id']}: expected one <name>-input/ and one <name>-topor/ board in {out}, "
                        f"found {len(inp)} / {len(rout)}")
    return inp[0], rout[0]


def _result_or_none(info: dict) -> dict | None:
    try:
        return _result(info)
    except ToolError:
        return None


def _drc_full(run_id: str, timeout_s: float = 600.0) -> dict:
    """Both DRCs (input, routed; cached) with the project proof, and the index. Internal: full rows."""
    from . import drc as D
    info = _run(run_id)
    inp, rout = _boards(info)
    for p in (inp, rout):
        check_board_path(p)
    work = Path(info["out_dir"]) / "mcp" / "drc"
    project = D.project_snapshot(inp)  # the board's rules, read once; written from memory beside every DRC copy
    routed_project = D.project_snapshot(rout)
    try:
        a = D.run_checked(inp, work, "input", project=project, timeout_s=timeout_s)
        b = D.run_checked(rout, work, "routed", project=project, timeout_s=timeout_s)
    except D.DrcError as ex:
        raise ToolError(str(ex)) from None
    res = _result_or_none(info)
    idx = D.index(inp.read_text(), a["doc"], rout.read_text(), b["doc"], res)
    differ = sorted(e for e in set(project) | set(routed_project) if project.get(e) != routed_project.get(e))
    return {"run_id": run_id, "info": info, "input": a, "routed": b, "index": idx, "result": res,
            "project": {"files": sorted(project), "proof_input": a["proof"], "proof_routed": b["proof"],
                        "routed_copy_differs": differ}}


SEVERITIES = ("error", "warning")  # kicad-cli DRC JSON severities (exclusions are not reported)


def _severities(severities) -> tuple:
    """The severities a spot list keeps: default errors only (fewer rows to go through); ["error", "warning"] (or
    "errors" / "warnings") brings warnings back."""
    if severities is None:
        return ("error",)
    vals = [severities] if isinstance(severities, str) else list(severities)
    out = tuple(sorted({str(v).strip().lower().removesuffix("s") for v in vals}))
    bad = [v for v in out if v not in SEVERITIES]
    if bad or not out:
        raise ToolError(f"severities must be a non-empty subset of {list(SEVERITIES)}, not {severities!r}")
    return out


def _sev_counts(rows: list, key: str = "sev") -> dict:
    c = defaultdict(int)
    for r in rows:
        c[str(r.get(key) or "")] += 1
    return {k: c.get(k, 0) for k in SEVERITIES} | {k: v for k, v in c.items() if k not in SEVERITIES}


def _sev_line(counts: dict, sev: tuple) -> str:
    shown = " + ".join(f"{v}s" for v in sev)
    return (f"ours: {counts.get('error', 0)} errors, {counts.get('warning', 0)} warnings; listed: {shown}"
            + ("" if "warning" in sev else " (severities=['error', 'warning'] lists warnings too)"))


def _write(path: Path, obj) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, indent=1, default=str))
    return str(path)


def drc(run_id: str, limit: int = ROWS_MAX, timeout_s: float = 600.0, severities=None) -> dict:
    """kicad-cli DRC of the run's routed copy as a DELTA vs its unrouted input, with the project provably used.
    `rows` (the spot list) holds the `severities` rows only (default errors); counts cover both severities."""
    sev = _severities(severities)
    full = _drc_full(run_id, timeout_s)
    idx = full["index"]
    path = _write(Path(full["info"]["out_dir"]) / "mcp" / "drc-index.json",
                  {k: v for k, v in full.items() if k not in ("info", "result", "input", "routed")}
                  | {"input_json": full["input"]["json"], "routed_json": full["routed"]["json"]})
    counts = _sev_counts(idx["rows"])
    rows = [r for r in idx["rows"] if r["sev"] in sev]
    return {"run_id": run_id, "rules": full["project"]["proof_routed"]["rules"]["verdict"],
            "project_used": full["project"]["proof_routed"]["project_used"], "project": full["project"],
            "summary": _sev_line(counts, sev), "severities": list(sev), "ours_by_severity": counts,
            "by_class": idx["by_class"], "ours_total": len(idx["rows"]), "rows_listed": len(rows),
            "other_new": idx["other_new"],
            "ours_items": idx["ours_items"], "unconnected_totals": idx["unconnected_totals"],
            "unconnected": {n: v for n, v in idx["unconnected"].items() if v["routed"] != v["input"]},
            "rows": rows[:max(0, int(limit))], "rows_truncated": max(0, len(rows) - max(0, int(limit))),
            "index_json": path, "input_drc": full["input"]["json"], "routed_drc": full["routed"]["json"],
            "count_note": "KiCad caps a type at 199/499 violations: a count there is a cap, not data"}


def compare(run_a: str, run_b: str) -> dict:
    """Per-class DRC delta (our new violations) between two runs of the same board (builds / settings)."""
    from . import drc as D
    fa, fb = _drc_full(run_a), _drc_full(run_b)
    ia, ib = _boards(fa["info"])[0], _boards(fb["info"])[0]
    same_input = D.sha256(ia.read_bytes()) == D.sha256(ib.read_bytes())
    same_rules = fa["project"]["proof_routed"]["sha256"] == fb["project"]["proof_routed"]["sha256"]
    if not same_rules:
        raise ToolError("the two runs use different project rules (.kicad_pro/.kicad_dru sha differ): not comparable")
    ua, ub = fa["index"]["unconnected_totals"]["routed"], fb["index"]["unconnected_totals"]["routed"]
    return {"run_a": run_a, "run_b": run_b, "same_input_board": same_input,
            "by_class": D.class_delta(fa["index"]["by_class"], fb["index"]["by_class"]),
            "ours_total": {"a": len(fa["index"]["rows"]), "b": len(fb["index"]["rows"]),
                           "delta": len(fb["index"]["rows"]) - len(fa["index"]["rows"])},
            "unconnected": {"a": ua, "b": ub, "delta": ub - ua},
            "ours_items": {"a": fa["index"]["ours_items"], "b": fb["index"]["ours_items"]},
            "note": None if same_input else "the input boards differ (different strip/board revision): compare with care"}


def gui_session(action: str = "status") -> dict:
    """The real-KiCad session of this server: `status` or `stop`."""
    from . import kicad_gui as G
    if action == "status":
        return G.session_status()
    if action == "stop":
        return G.stop_session()
    raise ToolError("action must be status or stop")


def _wanted_rows(rows: list, cls: str | None, per_class: int) -> list:
    if cls:
        sel = [r for r in rows if r["class"] == cls or r["class"].startswith(cls + " ") or r["type"] == cls]
        if not sel:
            raise ToolError(f"no row of class {cls!r}; classes: {sorted({r['class'] for r in rows})}")
        return sel[:per_class] if per_class > 0 else sel
    out, n = [], defaultdict(int)
    for r in rows:
        if per_class <= 0 or r["type"] in ("shorting_items", "tracks_crossing") or n[r["class"]] < per_class:
            out.append(r)
            n[r["class"]] += 1
    return out


def _gui_for_run(info: dict, idle_s: float | None = None):
    """The ONE real-KiCad session of a run: a copy of its routed board under <out_dir>/mcp/gui/board/, opened once
    and reused by drc_spots and cleanup until stopped (gui_session stop, idle timeout, error, or cleanup's end).
    The copy is (re)made only when no live session has it open. Caller holds kicad_gui._LOCK."""
    from . import kicad_gui as G
    _, rout = _boards(info)
    gdir = Path(info["out_dir"]) / "mcp" / "gui"
    live = gdir / "board" / rout.name
    s = G._SESSION
    if s is not None and s.board == live and s.alive():
        return G.session_for(live, gdir / "session", **({"idle_s": idle_s} if idle_s else {}))
    G.stop_session()
    G.copy_board(rout, live.parent)
    return G.session_for(live, gdir / f"session-{time.strftime('%Y%m%d-%H%M%S')}",
                         **({"idle_s": idle_s} if idle_s else {}))


def drc_spots(run_id: str, cls: str | None = None, per_class: int = 0, zoom: float = 20.0,
              keep_session: bool = True, drc_timeout_s: float = 600.0, severities=None,
              match_min: float = MATCH_MIN) -> dict:
    """KiCad's own DRC dialog in the run's ONE Xephyr session: DRC run once (per board state), then every wanted
    index row (all, or class `cls`; `per_class` > 0 caps each class, shorts/crossings always kept) is selected as a
    user does, in list order, and the dialog + the board view at the marker are kept. A failed row is reported in
    its record and the walk goes on in the same session. match = the GUI row has the same text and items and the
    selection landed on it; focus_mm = the board point KiCad centred (its marker), focus_d_mm = its distance from the
    row's nearest geometry (our track / via pieces, other items' points; else the items' mean position).
    focus_score = 1 / (1 + (focus_d_mm / scale)^2), scale = FOCUS_TOL_MM + FOCUS_EXTENT_SHARE * the row's geometry
    extent (graded, larger for long tracks / polygons); match also needs focus_score >= match_min.
    `severities` (default errors only): the dialog's severity filter is set to these before the walk (Warnings
    unticked by default) and only rows of these severities are wanted; the summary counts both."""
    from . import drc as D
    from . import kicad_gui as G
    sev = _severities(severities)
    full = _drc_full(run_id)
    counts = _sev_counts(full["index"]["rows"])
    rows = [r for r in full["index"]["rows"] if r["sev"] in sev]
    if not rows:
        return {"run_id": run_id, "records": [], "summary": _sev_line(counts, sev), "severities": list(sev),
                "ours_by_severity": counts,
                "note": f"no {' / '.join(sev)} violation of ours: nothing to show"}
    want = _wanted_rows(rows, cls, per_class)
    geom = D.board_items(_boards(full["info"])[1].read_text())  # the routed copy's tracks / vias, for focus_d_mm
    out = Path(full["info"]["out_dir"]) / "mcp"
    shots = out / "spots"
    shots.mkdir(parents=True, exist_ok=True)
    recs: list = []
    errors: list = []
    with G._LOCK:
        try:
            starts0 = G.GuiSession.starts_total
            s = _gui_for_run(full["info"])
            if s.board_state != "as opened":
                s.fail(f"drc_spots: the open board is {s.board_state!r}, not the routed board the index describes; "
                       f"stop the session (gui_session stop) first")
            s.drc_open()
            if s.drc_state_of != s.board_state:
                s.step("zoom", str(s.set_zoom(zoom)))
                s.step("grid", str(s.must("finest_grid", "grid")))  # focus_mm snaps to the grid
                st = s.drc_run(drc_timeout_s)
                tabs = st["tabs"][:2]
            else:
                tabs = None
                s.step("drc reused", f"{len(s.drc_rows)} rows of board state {s.board_state!r}")
            if s.drc_severities != sev:
                s.drc_filter(sev)  # Warnings unticked by default: the dialog lists (and the walk visits) fewer rows
            gui = s.drc_rows
            free = defaultdict(list)
            for g in gui:
                free[G.gui_key(g)].append(g)
            plan = []
            for r in want:
                g = G.find_row(free, G.cli_key(r))
                if g is None:
                    recs.append({"id": r["id"], "class": r["class"], "nets": r["nets"], "x": r["x"], "y": r["y"],
                                 "cli_text": r["text"], "cli_items": [i["desc"] for i in r["items"]], "match": False,
                                 "why": "no row with this text and these items in KiCad's DRC dialog"})
                else:
                    plan.append((g["k"], r, g))
            plan.sort(key=lambda t: t[0])
            by_k = {k: (r, g) for k, r, g in plan}
            main_x, dlg_x = s.main, s.xid(G.DRC_TITLE)
            prev_focus = None
            for k, selected, err in s.drc_walk([k for k, _, _ in plan]):
                r, g = by_k[k]
                rec = {"id": r["id"], "class": r["class"], "nets": r["nets"], "x": r["x"], "y": r["y"], "row": k,
                       "cli_text": r["text"], "cli_items": [i["desc"] for i in r["items"]],
                       "gui_text": g["text"], "gui_items": g["items"]}
                try:
                    if err:
                        raise G.GuiError(err)
                    ok_sel = selected == g["text"]
                    f = s.focus()
                    if prev_focus is not None and f["focus_mm"] == prev_focus[0] and \
                            math.hypot(r["x"] - prev_focus[1]["x"], r["y"] - prev_focus[1]["y"]) > 1.0:
                        # the same view as the previous row's although the marker moved: KiCad did not move it
                        # (verify r6 N1); the ring below is still put on the row's own marker
                        rec["focus_repeat"] = True
                        s.step("focus repeat", f"{r['id']}: same focus point as the previous row")
                    prev_focus = (f["focus_mm"], r)
                    d_mm = D.marker_dist(f["focus_mm"], r, geom)
                    scale = round(FOCUS_TOL_MM + FOCUS_EXTENT_SHARE * D.row_extent(r, geom), 3)
                    score = focus_score(d_mm, scale)
                    # the ring goes on the ROW's marker, from the view transform (KiCad centres the row's items, for
                    # tracks the middle of their bounding box, which may be several mm from the marker)
                    sx, sy = f["spot_px"]
                    mpx = marker_px(f, r["x"], r["y"])
                    visible = mpx is not None and f["view_px"][0] + 12 <= mpx[0] <= f["view_px"][2] - 12 and \
                        f["view_px"][1] + 12 <= mpx[1] <= f["view_px"][3] - 12
                    board_png = s.e.shot(f"{r['id']}-board", main_x)
                    dlg_png = s.e.shot(f"{r['id']}-dlg", dlg_x) if dlg_x else None
                    half = 350
                    cx, cy = (round(mpx[0]), round(mpx[1])) if visible else (sx, sy)  # crop and ring on the marker
                    rec.update(selected_ok=ok_sel, focus_mm=f["focus_mm"], focus_d_mm=d_mm, focus_scale_mm=scale,
                               focus_score=score, marker_px=[round(mpx[0]), round(mpx[1])] if mpx else None,
                               marker_visible=bool(visible),
                               board_png=G.crop(board_png, shots / f"{r['id']}-board.png",
                                                (cx - half, cy - half, 2 * half, 2 * half), mark=(cx, cy)),
                               dlg_png=str(shutil.copy(dlg_png, shots / f"{r['id']}-dlg.png")) if dlg_png else None,
                               match=bool(ok_sel) and bool(visible) and score >= match_min)
                    if not rec["match"]:
                        rec["why"] = ("the dialog selection is not this row" if not ok_sel else
                                      "the row's marker is not on screen (KiCad did not scroll to it)" if not visible
                                      else f"KiCad focused {d_mm} mm from the row's items: focus_score {score} < "
                                      f"match_min {match_min} (scale {scale} mm)")
                except G.STEP_ERRORS as ex:  # this row failed: say so, go on with the next row in the same session
                    rec.update(match=False, error=G.describe(ex))
                    errors.append(f"{r['id']} (row {k}): {G.describe(ex)}")
                    if not s.alive():
                        errors.append("KiCad exited: the remaining rows were not visited")
                        recs.append(rec)
                        break
                recs.append(rec)
                s.step("spot", f"{r['id']} row {k} match {rec.get('match')}")
            cli_all = full["routed"]["doc"].get("violations") or []
            cli_n = sum(1 for v in cli_all if v.get("severity") in sev)
            summary = {"run_id": run_id, "class": cls, "board_state": s.board_state, "gui_tabs": tabs,
                       "summary": _sev_line(counts, sev), "severities": list(sev), "ours_by_severity": counts,
                       "cli_by_severity": _sev_counts(cli_all, "severity"),
                       "gui_violations": len(gui), "cli_violations": cli_n, "gui_equals_cli": len(gui) == cli_n,
                       "wanted": len(want), "records": sorted(recs, key=lambda x: x["id"]),
                       "matched": sum(1 for x in recs if x.get("match")), "match_min": match_min,
                       "focus_score_min": min((x["focus_score"] for x in recs if "focus_score" in x), default=None),
                       "errors": errors,
                       "complete": not errors, "kicad_starts_this_call": G.GuiSession.starts_total - starts0,
                       "session": {"display": s.display, "pcbnew_pid": s.pid(), "board": str(s.board)},
                       "session_steps": s.steps[-60:], "shots_dir": str(shots)}
        except G.STEP_ERRORS as ex:
            G.stop_session()
            raise ToolError(f"drc_spots: {G.describe(ex)}") from None
        finally:
            if not keep_session:
                G.stop_session()
    summary["records_json"] = _write(out / f"spots-{time.strftime('%Y%m%d-%H%M%S')}.json", summary)
    return summary


def cleanup_labels(options) -> dict:
    """{KiCad label: bool} for every action box of the Cleanup dialog from `options`: a list of labels (on), or a
    dict {label or alias: bool}; boxes not named are off. Aliases: kicad_gui.CLEANUP_LABELS keys."""
    from .kicad_gui import CLEANUP_LABELS
    labels = list(CLEANUP_LABELS.values())
    by_norm = {" ".join(k.casefold().split()): v for k, v in CLEANUP_LABELS.items()}
    by_norm.update({" ".join(v.casefold().split()): v for v in labels})
    if options is None:
        options = list(DEFAULT_CLEANUP)
    items = options.items() if isinstance(options, dict) else ((o, True) for o in options)
    out = {lab: False for lab in labels}
    for k, val in items:
        lab = by_norm.get(" ".join(str(k).casefold().split()))
        if lab is None:
            raise ToolError(f"unknown Cleanup option {k!r}; KiCad's labels: {labels}")
        out[lab] = bool(val)
    return out


def cleanup(run_id: str, options=None, replay: bool = False, keep_session: bool = False,
            timeout_s: float = 900.0) -> dict:
    """KiCad's own Tools -> Cleanup Tracks & Vias in the run's ONE session (the board drc_spots opened, when still
    open), then kicad-cli DRC: what was removed, the newly unconnected connections per net (with the engine
    connection ids) and the DRC delta per class (after vs before). The session ends afterwards unless
    keep_session (the open board is then the cleaned one; drc_spots refuses it)."""
    from . import drc as D
    from . import kicad_gui as G
    boxes = cleanup_labels(options)
    full = _drc_full(run_id)
    info = full["info"]
    inp, rout = _boards(info)
    stamp = time.strftime("%Y%m%d-%H%M%S")
    out = Path(info["out_dir"]) / "mcp"
    res: dict = {"run_id": run_id, "options": boxes}
    with G._LOCK:
        try:
            starts0 = G.GuiSession.starts_total
            s = _gui_for_run(info)  # the run's one session (drc_spots' when it is still open)
            live = s.board
            if s.board_state != "as opened":
                s.fail(f"cleanup: the open board is already {s.board_state!r}; stop the session (gui_session stop) "
                       f"to clean the routed board again")
            res["session"] = {"display": s.display, "pcbnew_pid": s.pid(),
                              "reused": G.GuiSession.starts_total == starts0}
            snap0 = s.e.snapshot()
            s.must("menu", "cleanup open", path=["Tools", "Cleanup Tracks & Vias"])
            s.wait_for("cleanup open: dialog shown", 30, lambda: s.window(G.CLEANUP_TITLE))
            want = {**boxes, **{f: False for f in G.CLEANUP_FILTERS}}
            got = {b["label"]: b["value"] for b in s.must("set_checkboxes", "cleanup options", title=G.CLEANUP_TITLE,
                                                          values=want)}
            if any(got.get(k) != v for k, v in want.items()):
                s.fail(f"cleanup options: dialog shows {got}, wanted {want}")
            s.step("cleanup options", json.dumps(boxes))
            res["shot_dialog"] = str(s.e.shot("cleanup-dialog", s.xid(G.CLEANUP_TITLE)))
            t0 = time.monotonic()
            s.must("button", "build changes", title=G.CLEANUP_TITLE, label="Build Changes")
            s.wait_for("build changes: 'Update PCB' shown", timeout_s,
                       lambda: any(b["label"] == "Update PCB" and b["enabled"]
                                   for b in s.call("buttons", title=G.CLEANUP_TITLE)),
                       every=1.0, on_poll=s.answer_confirmation)
            res["build_s"] = round(time.monotonic() - t0, 1)
            changes = s.must("list", "change list", title=G.CLEANUP_TITLE, index=0)
            res["changes"] = len(changes)
            byt: dict = defaultdict(int)
            for c in changes:
                byt[re.sub(r"\s*\[.*$|\s+on\s+\S+\.Cu.*$", "", c["text"]).strip()] += 1
            res["changes_by_text"] = dict(sorted(byt.items()))
            res["shot_changes"] = str(s.e.shot("cleanup-changes", s.xid(G.CLEANUP_TITLE)))
            t1 = time.monotonic()
            s.must("button", "update pcb", title=G.CLEANUP_TITLE, label="Update PCB")
            s.wait_for("update pcb: dialog closed", timeout_s, lambda: not s.window(G.CLEANUP_TITLE),
                       every=1.0, on_poll=s.answer_confirmation)
            res["update_s"] = round(time.monotonic() - t1, 1)
            snap1 = s.e.snapshot()
            sha0 = D.sha256(live.read_bytes())
            s.e.ipc_retry("board.save()")
            s.wait_for("save: board file written", 60, lambda: D.sha256(live.read_bytes()) != sha0 or
                       snap1 == snap0)
            s.step("saved", str(live))
            s.board_state = f"after cleanup {stamp}"
            res["shot_board"] = str(s.e.shot("board-after-cleanup", s.main))
        except G.STEP_ERRORS as ex:
            G.stop_session()
            raise ToolError(f"cleanup: {G.describe(ex)}") from None
        finally:
            if not keep_session:
                G.stop_session()
    removed = {u: snap0[u] for u in snap0 if u not in snap1}
    added = {u: snap1[u] for u in snap1 if u not in snap0}
    changed = {u for u in snap1 if u in snap0 and snap1[u] != snap0[u]}
    rk: dict = defaultdict(int)
    for v in removed.values():
        rk[v["kind"]] += 1
    res.update(removed=dict(rk), added=len(added), changed=len(changed), board_after=str(live))
    try:
        after = D.run_checked(live, out / "drc", f"cleanup-{stamp}", project=D.project_snapshot(inp))
    except D.DrcError as ex:
        raise ToolError(str(ex)) from None
    idx_after = D.index(inp.read_text(), full["input"]["doc"], live.read_text(), after["doc"], full["result"])
    res["drc_by_class"] = D.class_delta(full["index"]["by_class"], idx_after["by_class"])
    res["drc_note"] = "a = before cleanup, b = after (our new violations per class)"
    rt = (full["result"] or {}).get("route") or {}
    before_items = D.board_items(rout.read_text())
    ours_map = D.conn_map(before_items, {u for u in before_items if u not in D.board_items(inp.read_text())},
                          rt.get("connections") or [])
    res["newly_unconnected"] = D.newly_unconnected(full["routed"]["doc"], after["doc"], rt.get("connections") or [],
                                                   before_items, set(removed), ours_map)
    res["newly_unconnected_nets"] = sorted(n for n, v in res["newly_unconnected"].items() if v["new"])
    res["newly_unconnected_total"] = sum(len(v["new"]) for v in res["newly_unconnected"].values())
    res["unconnected_totals"] = {"before": full["index"]["unconnected_totals"]["routed"],
                                 "after": idx_after["unconnected_totals"]["routed"]}
    res["after_drc"] = after["json"]
    if replay:
        from harness.gui_e2e import CLEANUP_BOXES, replay as _replay
        short = {v: k for k, v in G.CLEANUP_LABELS.items()}
        opts = {short[lab]: on for lab, on in boxes.items()}
        rp = _replay(rout, {k: opts.get(k, False) for k in CLEANUP_BOXES}, out / f"cleanup-{stamp}-replay.json",
                     dict(os.environ))
        res["replay"] = {"removed": len(rp), "only_dialog": sorted(set(removed) - set(rp))[:20],
                         "only_replay": sorted(set(rp) - set(removed))[:20],
                         "agrees": set(rp) == set(removed)}
    res["record_json"] = _write(out / f"cleanup-{stamp}.json", res)
    return res
