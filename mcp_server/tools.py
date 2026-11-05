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
    env = dict(os.environ, TOPOR_REPORT_ROOT=str(out_dir / "reports"))
    try:
        cp = subprocess.run(cmd, capture_output=True, text=True, env=env,
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
