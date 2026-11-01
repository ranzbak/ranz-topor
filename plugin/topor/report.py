"""Run report (B5 seam, B8 full report): the summary block of the wireframe (section 5, at most 20 lines) and the
report folder files ``report.md``, ``preview.png`` (written by preview.py), ``result.json``, ``drc.json``,
``log.txt``. Satisfies ``ports.Report`` (``dir``, ``summary_lines()``) and carries ``stop_reason`` for the
done notice.

``RunReport.facts`` is the one dict of numbers; ``build_report`` fills it from the engine result, the classified
DRC, the Config, findings, scope, the stats of the current board and timing (pure Python, no pcbnew).
``write()`` is called twice per run (before the preview with outcome ``pending``, after the decision with the
final one).

Complexity: build_report O(connections + items + nets + vias * pads); to_markdown / write O(size).
"""

from __future__ import annotations

import json
import math
import shutil
from pathlib import Path

from toporlib.detour import detour_line, detour_stats

SUMMARY_MAX_LINES = 20
MM = 1_000_000

#: RunOutcome.status -> wording of the ``Outcome:`` line (wireframe section 5).
OUTCOME_TEXT = {
    "pending": "pending — waiting for Accept or Reject",
    "accepted": "applied as one undo step",
    "rejected": "rejected — board unchanged",
    "cancelled": "cancelled — nothing applied",
    "aborted_preflight": "aborted at pre-flight",
}


def outcome_text(status: str, code: str = "") -> str:
    if status == "error":
        return f"error {code}".strip()
    return OUTCOME_TEXT.get(status, status)


def _na(v, fmt="{}"):
    return "n/a" if v is None else fmt.format(v)


def _types(d) -> str:
    return " (" + ", ".join(f"{k} {v}" for k, v in sorted(d.items())) + ")" if d else ""


class RunReport:
    """Facts + files of one run. `facts` keys (all optional, missing -> n/a): version, board, timestamp, routed,
    total, nets, layers, partial, failed, vias, vias_before, length_mm, length_before_mm, routing_new,
    preexisting, unconnected, detour_line, stop_reason, iterations, elapsed_s, threads, scope, existing_copper,
    respect_locked, non_default, defaults_used, findings (list of Finding), notes (list of str), drc_rounds,
    floors_applied (toporlib.rules ``[{"name", "design", "floor"}]`` nm; set by run.py);
    set by build_report: connection_rows, net_rows, partial_routes, zero_slack_wires, vias_in_pads,
    ignored_pads, fallbacks, accepted_codes, non_defaults, versions, drc_by_type_*, unconnected_pre*."""

    def __init__(self, dir: Path, facts: dict | None = None):
        self.dir = Path(dir)
        self.facts: dict = dict(facts or {})
        self.status = "pending"
        self.error_code = ""

    @property
    def stop_reason(self):
        return self.facts.get("stop_reason")

    def set_outcome(self, status: str, error_code: str = "") -> None:
        self.status, self.error_code = status, error_code

    def _refill_note(self) -> str:
        """Tail of the Outcome line after Accept (ruling C28): how the zones get refilled. O(1)."""
        if self.status != "accepted" or "zones_refilled" not in self.facts:
            return ""
        if self.facts["zones_refilled"]:
            return " · zones refilled by topor (a second undo step)"
        return " · zones not refilled: press B (Edit → Fill All Zones), a separate undo step"

    # ---- summary block (wireframe section 5); fixed line set, so <= 20 lines whatever the board
    def summary_lines(self) -> list:
        f = self.facts
        fnd = f.get("findings") or []
        sev = lambda s: sum(1 for x in fnd if x.severity == s)  # noqa: E731
        lines = [
            f"Topor {f.get('version', '?')} — {f.get('board', '?')} — {f.get('timestamp', '?')}",
            f"Routed {_na(f.get('routed'))}/{_na(f.get('total'))} connections · {_na(f.get('nets'))} nets · "
            f"{_na(f.get('layers'))} layers",
            f"Partial {_na(f.get('partial'))} · failed {_na(f.get('failed'))}",
            f"Vias {_na(f.get('vias'))} (current board {_na(f.get('vias_before'))})",
            f"Length outside pads {_na(f.get('length_mm'))} mm"
            + (f" (current {f['length_before_mm']} mm)" if f.get("length_before_mm") is not None else ""),
            f"DRC new {_na(f.get('routing_new'))} · pre-existing {_na(f.get('preexisting'))} · "
            f"unconnected {_na(f.get('unconnected'))}",
        ]
        if f.get("detour_line"):
            lines.append(f"Detours {f['detour_line']}")
        if f.get("detour_ghost_line"):
            lines.append(f"Detours vs ghost line {f['detour_ghost_line']}")
        lines += [
            f"Stopped: {_na(f.get('stop_reason'))} after {_na(f.get('iterations'))} iterations, "
            f"{_na(f.get('elapsed_s'))} s, {_na(f.get('threads'))} threads",
            f"Scope {f.get('scope', 'whole board')} · existing copper {_na(f.get('existing_copper'))} · "
            f"locked {_na(f.get('respect_locked'))}",
            f"Settings: {_na(f.get('non_default'))} non-default, {_na(f.get('defaults_used'))} built-in defaults",
            f"Findings: {sev('error')} errors · {sev('warning')} warnings · {sev('info')} notes",
            f"Outcome: {outcome_text(self.status, self.error_code)}" + self._refill_note(),
            f"Report: {self.dir}",
        ]
        return lines[:SUMMARY_MAX_LINES]

    # ---- report.md
    def to_markdown(self) -> str:
        """The nine sections of wireframe 5; an empty section prints ``none``."""
        md = ["# Topor run report", "", "## Summary", "", *[f"    {ln}" for ln in self.summary_lines()], ""]
        for title, body in (("Connections", self._connections()), ("Vias", self._vias()),
                            ("Length", self._length()), ("DRC", self._drc()), ("Findings", self._findings()),
                            ("Settings", self._settings()), ("Files", self._files()),
                            ("Versions", self._versions())):
            md += [f"## {title}", "", *(body or ["none"]), ""]
        if self.facts.get("notes"):
            md += ["## Notes", "", *[f"- {n}" for n in self.facts["notes"]], ""]
        return "\n".join(md)

    def _connections(self) -> list:
        f = self.facts
        if f.get("routed") is None:
            return []
        out = [f"Routed {f['routed']}/{_na(f.get('total'))}; partial {_na(f.get('partial'))}; "
               f"failed {_na(f.get('failed'))}.", ""]
        rows = f.get("connection_rows") or []
        if rows:
            out += ["| net | from -> to | status | reason | blocked at |", "|---|---|---|---|---|"]
            out += [f"| {r['net']} | {r['from']} -> {r['to']} | {r['status']} | {r['reason'] or ''} | "
                    f"{r['blocking']} |" for r in rows]
            out.append("")
        out.append("Partial routes: " + (", ".join(f["partial_routes"]) if f.get("partial_routes") else "none"))
        out.append("Zero-slack connections: " + (", ".join(f["zero_slack_wires"]) if f.get("zero_slack_wires")
                                                  else "none"))
        out.append("Exit stubs: " + (", ".join(f["exit_stubs"]) if f.get("exit_stubs") else "none"))
        out.append("Necked stubs (narrower than the net width): " + (", ".join(f["necked_stubs"])
                                                                     if f.get("necked_stubs") else "none"))
        out.append("Ignored pads: " + (", ".join(f["ignored_pads"]) if f.get("ignored_pads") else "none"))
        out.append("Excluded nets (do not touch): " + (", ".join(f["excluded_nets"]) if f.get("excluded_nets")
                                                      else "none"))
        return out

    def _vias(self) -> list:
        f = self.facts
        if f.get("vias") is None:
            return []
        out = []
        rows = [r for r in f.get("net_rows") or [] if r["vias"] or r["vias_before"]]
        if rows:
            out += ["| net | vias | current board |", "|---|---|---|"]
            out += [f"| {r['net']} | {r['vias']} | {_na(r['vias_before'])} |" for r in rows] + [""]
        out.append(f"Total {f['vias']} (current board {_na(f.get('vias_before'))}).")
        out.append("Vias in pads: " + (", ".join(f["vias_in_pads"]) if f.get("vias_in_pads") else "none"))
        return out

    def _length(self) -> list:
        f = self.facts
        if f.get("length_mm") is None:
            return []
        out = []
        rows = [r for r in f.get("net_rows") or [] if r["length_mm"] or r["length_before_mm"]]
        if rows:
            out += ["| net | length mm | current mm |", "|---|---|---|"]
            out += [f"| {r['net']} | {r['length_mm']} | {_na(r['length_before_mm'])} |" for r in rows] + [""]
        out.append(f"Outside pads {f['length_mm']} mm (current {_na(f.get('length_before_mm'))} mm).")
        out.append(f"Detours: {f.get('detour_line') or 'n/a'}")
        if f.get("detour_ghost_line"):
            out.append(f"Detours vs ghost line: {f['detour_ghost_line']}")
        out += [f"- {r.get('name')} {r.get('from')} -> {r.get('to')}: ratio {r.get('ratio')}"
                for r in f.get("detour_top") or []]
        return out

    def _drc(self) -> list:
        f = self.facts
        if f.get("routing_new") is None:
            return []
        out = [f"routing_new {f['routing_new']}{_types(f.get('drc_by_type_new'))}",
               f"preexisting {_na(f.get('preexisting'))}{_types(f.get('drc_by_type_pre'))}",
               f"unconnected: {_na(f.get('unconnected'))} new, {_na(f.get('unconnected_pre'))} pre-existing",
               f"DRC rounds: {_na(f.get('drc_rounds'))}"]
        out += [f"- pre-existing unconnected: {d}" for d in (f.get("unconnected_pre_items") or [])[:20]]
        return out

    def _findings(self) -> list:
        acc = set(self.facts.get("accepted_codes") or ())
        return [f"- **{x.code}** ({x.severity}{', accepted' if x.code in acc else ''}): {x.message}"
                + (f" Fix: {x.fix}" if x.fix else "") for x in (self.facts.get("findings") or [])]

    def _settings(self) -> list:
        f = self.facts
        nd = f.get("non_defaults") or {}
        out = ["Non-default settings:"] + ([f"- {k} = {v!r}" for k, v in sorted(nd.items())] or ["- none"])
        out.append(f"Built-in defaults used: {_na(f.get('defaults_used'))} settings")
        out.append("Accepted fallbacks: " + (", ".join(f["fallbacks"]) if f.get("fallbacks") else "none"))
        if "floors_applied" in f:  # toporlib.rules: KiCad minimums below the fab profile, raised (F I-4)
            fl = f["floors_applied"] or []
            out.append("Fab floors raised (JLCPCB 1 oz profile): " + (", ".join(
                f"{x['name']} {x['design'] / MM:g} → {x['floor'] / MM:g} mm" for x in fl) if fl else "none"))
        if f.get("toml_path"):
            out.append(f"Config file: {f['toml_path']}")
        return out

    @staticmethod
    def _files() -> list:
        return [f"- {n}" for n in ("report.md", "preview.png", "result.json", "drc.json", "log.txt")]

    def _versions(self) -> list:
        v = self.facts.get("versions") or {"engine": self.facts.get("version", "?")}
        return [f"- {k}: {x}" for k, x in sorted(v.items())]

    # ---- files
    def write(self, *, result: dict | None = None, drc_src: Path | None = None) -> None:
        """(Re)write report.md and, when given, result.json / drc.json. O(size)."""
        self.dir.mkdir(parents=True, exist_ok=True)
        (self.dir / "report.md").write_text(self.to_markdown())
        if result is not None:
            (self.dir / "result.json").write_text(json.dumps(result, indent=1, default=str))
        if drc_src is not None and Path(drc_src).is_file():
            shutil.copyfile(drc_src, self.dir / "drc.json")


def write_report(report: RunReport, *, result: dict | None = None, drc_src: Path | None = None) -> Path:
    report.write(result=result, drc_src=drc_src)
    return report.dir / "report.md"


def _prim_len(prim: dict) -> float:
    """Length (nm) of a seg / arc primitive. O(1)."""
    if prim.get("kind") != "arc":
        return math.dist(prim["a"], prim["b"])
    a, m, e = prim["start"], prim["mid"], prim["end"]
    d = 2.0 * (a[0] * (m[1] - e[1]) + m[0] * (e[1] - a[1]) + e[0] * (a[1] - m[1]))
    chord = math.dist(a, e)
    if abs(d) < 1e-9:
        return chord
    sq = lambda p: p[0] ** 2 + p[1] ** 2  # noqa: E731
    ux = (sq(a) * (m[1] - e[1]) + sq(m) * (e[1] - a[1]) + sq(e) * (a[1] - m[1])) / d
    uy = (sq(a) * (e[0] - m[0]) + sq(m) * (a[0] - e[0]) + sq(e) * (m[0] - a[0])) / d
    r = math.dist((ux, uy), a)
    ang = lambda p: math.atan2(p[1] - uy, p[0] - ux)  # noqa: E731
    sweep = (ang(e) - ang(a)) % (2 * math.pi)
    if (ang(m) - ang(a)) % (2 * math.pi) > sweep:  # mid not on the ccw side: go the other way
        sweep = 2 * math.pi - sweep
    return r * sweep


def before_stats_from_model(model: dict | None) -> dict:
    """Vias / track length of the board as it is now, from the exported model (pure). O(vias + kept)."""
    model = model or {}
    names = {int(n["code"]): n["name"] for n in model.get("nets", [])}
    vias_by: dict = {}
    len_by: dict = {}
    for v in model.get("vias", []):
        n = names.get(int(v.get("net", 0)), str(v.get("net")))
        vias_by[n] = vias_by.get(n, 0) + 1
    for t in model.get("kept", []):
        if t.get("prim"):
            n = names.get(int(t.get("net", 0)), str(t.get("net")))
            len_by[n] = len_by.get(n, 0.0) + _prim_len(t["prim"])
    return {"vias": len(model.get("vias", [])), "vias_by_net": vias_by,
            "length_mm": round(sum(len_by.values()) / MM, 3),
            "length_by_net_mm": {k: round(v / MM, 3) for k, v in len_by.items()},
            "pads": model.get("pads", [])}


def _vias_in_pads(items, pads) -> list:
    """Labels (``REF.num``) of routed vias whose centre lies in the bounding box of a same-net pad. O(vias * pads)."""
    by_net: dict = {}
    for p in pads:
        by_net.setdefault(p.get("net"), []).append(p)
    out = []
    for it in items:
        if it.get("kind") != "via":
            continue
        x, y = it["at"]
        for p in by_net.get(it.get("net"), ()):
            sh = next(iter((p.get("shapes") or {}).values()), None) or {}
            w = sh.get("w") or sh.get("size") or 0
            h = sh.get("h") or w
            if not isinstance(w, (int, float)) or not isinstance(h, (int, float)) or not w:
                continue
            cx, cy = p["center"]
            if abs(x - cx) <= w / 2 and abs(y - cy) <= h / 2:
                out.append(f"{p.get('ref')}.{p.get('number')}")
                break
    return out


def _stub_text(s: dict) -> str:
    """`U7.4 0.62 mm x 0.20 mm` (+ `, 0.05 mm off centre`, `, to U7.7` for a bridge) of a used stub row."""
    t = f"{s.get('pad')} {s.get('length_nm', 0) / MM:.2f} mm x {s.get('width', 0) / MM:.2f} mm"
    if s.get("necked"):
        t += f" (net {s.get('net_width', 0) / MM:.2f} mm)"
    if s.get("offset_nm"):
        t += f", {abs(s['offset_nm']) / MM:.3f} mm off centre"
    if s.get("bridge"):
        t += f", to {s['bridge']}"
    return t


def build_report(result: dict, drc: dict | None, cfg, findings, scope, before_stats: dict | None,
                 timing: dict | None, *, dir: Path | None = None, report: RunReport | None = None,
                 facts: dict | None = None) -> RunReport:
    """Fill a RunReport (a new one for `dir`, or the given `report`) from the engine result dict, the classified
    DRC (``toporlib.drc.classify``), the Config, findings, the scope (str, or dict with ``mode``,
    ``ignored_pads``, ``accepted_codes``, ``fallbacks``), the stats of the current board
    (``before_stats_from_model``) and timing (``router_s`` / ``elapsed_s`` / ``drc_rounds``). Extra `facts`
    override. Pure Python, no pcbnew. O(connections + items + nets + vias * pads)."""
    from .schema import SETTINGS  # noqa: PLC0415

    rep = report if report is not None else RunReport(Path(dir) if dir else Path("."))
    result, drc, before, timing = result or {}, drc or {}, before_stats or {}, timing or {}
    scope = {"mode": scope} if isinstance(scope, str) else dict(scope or {})
    conns = result.get("connections") or []
    items = result.get("items") or []
    ref_stats = detour_stats(conns, field="ref_len_nm")
    ghost_stats = detour_stats(conns)
    # the DRC-feasible reference when the engine computed it (M3-R15), else the topological ghost
    stats = ref_stats if ref_stats and ref_stats.get("n") else ghost_stats
    inc = drc.get("incomplete") or {}
    nd = cfg.non_defaults() if cfg is not None else {}
    rows = []
    for c in conns:
        if c.get("status") == "routed":
            continue
        b = c.get("blocking")
        rows.append({"net": c.get("name"), "from": (c.get("from") or {}).get("label", "?"),
                     "to": (c.get("to") or {}).get("label", "?"), "status": c.get("status"),
                     "reason": c.get("reason"), "blocking": f"{b[0] / MM:.3f}, {b[1] / MM:.3f} mm" if b else ""})
    net_rows = [{"net": n.get("name"), "vias": n.get("vias", 0),
                 "vias_before": (before.get("vias_by_net") or {}).get(n.get("name"), 0),
                 "length_mm": round(n.get("length_nm", 0) / MM, 3),
                 "length_before_mm": (before.get("length_by_net_mm") or {}).get(n.get("name"), 0)}
                for n in result.get("nets") or []]
    lop = result.get("length_outside_pads_nm")
    el = timing.get("elapsed_s", timing.get("router_s"))
    new = {
        "routed": result.get("routed"), "total": result.get("connections_total"),
        "partial": result.get("partial"), "failed": result.get("failed"),
        "nets": result.get("nets_total"),
        "vias": result.get("vias", sum(1 for i in items if i.get("kind") == "via")),
        "vias_before": before.get("vias"),
        "length_mm": None if lop is None else round(lop / MM, 3),
        "length_before_mm": before.get("length_mm"),
        "routing_new": drc.get("routing_new"), "preexisting": drc.get("routing_preexisting"),
        "unconnected": inc.get("unconnected_new"), "unconnected_pre": inc.get("unconnected_preexisting"),
        "unconnected_pre_items": drc.get("unconnected_preexisting_items"),
        "drc_by_type_new": drc.get("by_type_new"), "drc_by_type_pre": drc.get("by_type_preexisting"),
        "drc_rounds": timing.get("drc_rounds"),
        "detour_line": detour_line(stats), "detour_top": (stats or {}).get("top10", [])[:5],
        "detour_ghost_line": detour_line(ghost_stats) if stats is not ghost_stats else None,
        "stop_reason": result.get("stop_reason"), "iterations": result.get("iterations"),
        "elapsed_s": None if el is None else round(el, 1), "threads": result.get("threads_used"),
        "scope": scope.get("mode"),
        "connection_rows": rows, "net_rows": net_rows,
        "partial_routes": [r["net"] for r in rows if r["status"] == "partial"],
        "zero_slack_wires": [w.get("name") or str(w.get("conn")) for w in result.get("zero_slack_wires") or []],
        "vias_in_pads": _vias_in_pads(items, before.get("pads") or []),
        "exit_stubs": [_stub_text(s) for s in result.get("stubs") or [] if s.get("used")],
        "necked_stubs": [_stub_text(s) for s in result.get("stubs") or [] if s.get("used") and s.get("necked")],
        "ignored_pads": list(scope.get("ignored_pads") or []),
        "excluded_nets": [n.get("name") for n in result.get("nets") or [] if n.get("status") == "excluded"],
        "fallbacks": list(scope.get("fallbacks") or []),
        "accepted_codes": list(scope.get("accepted_codes") or ()),
        "findings": list(findings or []),
        "non_defaults": nd, "non_default": len(nd), "defaults_used": len(SETTINGS) - len(nd),
    }
    if cfg is not None:
        new["existing_copper"] = cfg.get("run", "existing_copper")
        new["respect_locked"] = cfg.get("run", "respect_locked")
    rep.facts.update({k: v for k, v in new.items() if v is not None})
    rep.facts.update(facts or {})
    return rep
