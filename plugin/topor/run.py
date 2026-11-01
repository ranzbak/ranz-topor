"""The run flow (B5): ``save_as`` copy -> rules -> model -> pre-flight -> rip plan -> route (progress, cancel) ->
DRC loop on the copy -> SVG preview -> Accept = one commit / Reject = nothing -> report folder.

``run(port, ui, cfg, engine, *, tmp_root)`` never touches the live board except in the Accept branch, which is
exactly one ``port.commit`` (= one undo step), followed only on request by ``refill_zones``
(``run.refill_after_accept``, default false: the user presses B) and the KiCad cleanup / teardrop actions, each its
own undo step (ruling C28); a failure after the commit keeps ``accepted`` + POST_COMMIT_STEP_FAILED (C30). Every
board that is judged or rendered derives from the ``save_as`` copy (live, including unsaved edits), never from
the file on disk (Review Focus 2). No pcbnew (M4-R1).

Seams (plug-in points for the later tasks; each is a plain function here):
    * ``preflight_findings``        B6: replace / extend with the full pre-flight checks (messages.py codes).
    * ``connection_estimate``       B6: exact connection counts per scope for ``RunSummary``.
    * ``rip_plan`` / ``filter_model``   B7: scope-aware rip list (scope, priorities, do_not_touch) and the model
                                    filtered by it.
    * ``build_options``             B7: net_priority / exclude_nets / ignore_pads / scope are added here (only
                                    keys the engine's ``default_options`` knows are passed).
    * ``report.RunReport``          B8: the full report (the run fills ``report.facts``; ``write`` is the only
                                    call made on it besides ``summary_lines`` / ``set_outcome``).
    * ``apply_accepted_findings``   B6: persists accepted PAD_OUTSIDE_BOARD / DRILL_OUTSIDE_FILTER choices.
    * ``apply_via_fallback``        Q-b / ruling C27: VIA_NOT_SET_UP -> ``vias.fallback_*`` written into the
                                    engine's via rules (before read_model, and into the model's own rules).

Complexity: O(1) rounds of kicad-cli (original DRC, ripped DRC when copper is ripped, one DRC per router round
(<= drc_loop_cap), two SVG exports) around one engine run; the python glue is O(len(board text) + items) per
round (pcbfile edits, JSON). Progress polling is O(run time / 0.25 s).
"""

from __future__ import annotations

import json
import re
import statistics
import tempfile
import time
import traceback
import uuid as _uuid
from dataclasses import asdict, dataclass, field
from datetime import datetime
from pathlib import Path

from toporlib import drcloop, pcbfile
from toporlib import rules as rules_mod
from toporlib.drc import classify, run_drc
from toporlib.drcloop import DrcJudgement

from . import messages
from . import preflight as preflight_mod
from . import preview as preview_mod
from .config import CONFIG_NAME, Config, load_config, update_config
from .findings import Finding
from .layers import resolve_layer_roles
from .pourreach import dangling_ends, pour_reach
from .priority import net_priorities
from .ports import BoardPort, CommitRequest, CommitResult, Decision, Progress, RunSummary, Selection, UiPort
from .report import RunReport, before_stats_from_model, build_report
from .schema import CURRENT_VERSION
from .scope import connections_by_scope, do_not_touch_nets, rip_plan, scope_from_selection

ACTION_CLEANUP = "pcbnew.GlobalEdit.cleanupTracksAndVias"
ACTION_TEARDROPS = "pcbnew.GlobalEdit.editTeardrops"
POLL_S = 0.25
NO_BUDGET_S = 1.0e9  # timeout_s = 0: the DRC loop gets no budget cut; drc_loop_cap still bounds it
#: After the user's Cancel the engine gets this long to reach a stop check and end its thread; then the run ends
#: with ENGINE_UNRESPONSIVE instead of polling for ever (gate review B-1, ruling C31). Internal, not a toml key.
CANCEL_GRACE_S = 30
_ROUTE_OVERRIDES = ("scope_mode", "existing_copper", "respect_locked", "partial_routes", "timeout_s", "threads")


class EngineUnresponsive(RuntimeError):
    """The engine did not finish within CANCEL_GRACE_S after Cancel (-> ENGINE_UNRESPONSIVE)."""


@dataclass
class RunOutcome:
    """Result of one run. `status`: aborted_preflight | cancelled | rejected | accepted | error."""
    status: str
    report: RunReport | None = None
    commit: CommitResult | None = None
    report_dir: Path | None = None
    findings: list = field(default_factory=list)
    error: Finding | None = None
    routed: int = 0
    total: int = 0
    stop_reason: str | None = None
    cancelled: bool = False
    items: list = field(default_factory=list)  # the routed items (committed on Accept), group set
    ripped: list = field(default_factory=list)  # uuids removed on Accept
    model: dict | None = None  # the model as read from the port (before rip filtering)
    drc: dict | None = None  # toporlib.drc.classify of the final routed copy
    base_drc: object = None  # DrcReport of the ripped, unrouted copy
    orig_drc: object = None  # DrcReport of the untouched copy
    rounds: int = 0
    router_s: float = 0.0
    wall_s: float = 0.0
    work_dir: Path | None = None
    pcb_copy: Path | None = None
    refilled_pcb: Path | None = None  # kicad-cli refilled copy of the untouched board (original DRC, ruling C29)
    ripped_pcb: Path | None = None
    routed_pcb: Path | None = None
    via_fallback: dict | None = None  # apply_via_fallback info when VIA_NOT_SET_UP patched the engine rules


class _Log:
    """report_dir/log.txt, flushed per line. O(1) per line."""

    def __init__(self, path: Path):
        self.path = path
        self.t0 = time.monotonic()
        path.write_text("")

    def line(self, msg: str) -> None:
        with self.path.open("a") as f:
            f.write(f"[{time.monotonic() - self.t0:7.2f}s] {msg}\n")

    def exc(self) -> None:
        with self.path.open("a") as f:
            f.write(traceback.format_exc())


# ---------------------------------------------------------------- seams
def preflight_findings(cfg: Config, config_findings, copy_text: str, *, model=None, rules=None, sel=None,
                       pro=None, orig_drc=None) -> list:
    """Pre-flight findings (B6, ``preflight.preflight``): config, format, layer roles + plane zones, vias,
    pads outside the outline, rules, copper, scope, defaults. Without `model` only the text-based checks run."""
    return preflight_mod.preflight(model or {}, rules or {}, cfg, sel or Selection(), orig_drc,
                                   list(config_findings), pcb_text=copy_text, pro=pro)


def connection_estimate(model: dict) -> tuple:
    """(connections, nets with >= 2 pads): spanning-tree estimate pads - 1 per net (``preflight.connection_count``)."""
    return preflight_mod.connection_count(model)



def filter_model(model: dict, ripped) -> dict:
    """The model the engine sees after the rip: ripped tracks / vias, dangling ends of ripped tracks and pour
    reach through ripped vias are gone. Does not modify `model`. O(model items)."""
    gone = set(ripped)
    if not gone:
        return model
    return {**model,
            "kept": [k for k in model.get("kept", []) if k["uuid"] not in gone],
            "vias": [v for v in model.get("vias", []) if v["uuid"] not in gone],
            "dangling_ends": [d for d in model.get("dangling_ends", []) if d.get("item") not in gone],
            "pours": [{**p, "vias": [u for u in p.get("vias", []) if u not in gone]} for p in model.get("pours", [])]}


def refill_pours(model: dict, refilled_text: str) -> dict:
    """The model with pour fills, pour reach (pads / vias) and dangling ends recomputed from a kicad-cli REFILLED
    copy: the ripped copy (harness order rip -> refill -> export, ruling C20) or, when nothing is ripped, the
    untouched copy refilled by the original DRC (ruling C29), so the engine never sees the live board's possibly
    stale fills. Zones are matched by uuid. A zone that the refilled file lists WITHOUT filled polygons on its
    layers keeps its old fill only as an obstacle (a superset of no copper) and reaches nothing (M4-R8: "not
    reached"; `warning` says so); a zone absent from the file keeps its model entry. Same functions as the IPC
    exporter (``topor.pourreach``; no kipy). O(F * (pads + vias))."""
    fills = pcbfile.parse_zone_fills(refilled_text)
    ids = {l["name"]: int(l["id"]) for l in model.get("layers", [])}
    pours = []
    for p in model.get("pours", []):
        polys = fills.get(p["uuid"]) or []
        fl = [{"layer": ids[q["layer"]], "outer": q["pts"], "holes": []} for q in polys
              if q["layer"] in ids and ids[q["layer"]] in p["layers"]]
        if not fl:
            if p["uuid"] in fills:  # refilled, but nothing filled: connects nothing
                pours.append({**p, "pads": [], "vias": [],
                              "warning": "no fill after the kicad-cli refill; old fill kept as obstacle, reaching nothing"})
            else:
                pours.append(p)
            continue
        rp, rv = pour_reach(p["net"], p["layers"], fl, model["pads"], model["vias"])
        pours.append({k: v for k, v in {**p, "fills": fl, "pads": rp, "vias": rv}.items() if k != "warning"})
    cu = [int(l["id"]) for l in model.get("layers", [])]
    return {**model, "pours": pours,
            "dangling_ends": dangling_ends(model["kept"], model["vias"], model["pads"], cu)}


def via_costs_by_net(cfg: Config, model: dict, priorities: dict | None) -> dict:
    """``Options.via_cost_by_net`` (net name -> via cost nm) from ``[vias] cost_by_netclass`` and
    ``cost_by_priority``; precedence per net: by netclass > by priority > ``via_cost_mm`` (nets without an entry are
    left out and use the global cost). A composite class (``"Power,Default"``) takes the first listed class that has
    an entry. Non-numeric entries are skipped. O(#nets * classes per net)."""
    by_class = {str(k): v for k, v in dict(cfg.get("vias", "cost_by_netclass")).items()}
    by_prio = {str(k): v for k, v in dict(cfg.get("vias", "cost_by_priority")).items()}
    if not by_class and not by_prio:
        return {}

    def nm(v):
        return int(round(float(v) * 1_000_000)) if isinstance(v, (int, float)) and not isinstance(v, bool) else None

    per_net = (model.get("rules") or {}).get("per_net") or {}
    out: dict = {}
    for n in model.get("nets", []):
        name = n.get("name")
        if not name:
            continue
        cost = None
        for c in str((per_net.get(str(n.get("code"))) or {}).get("netclass") or "").split(","):
            if c in by_class and nm(by_class[c]) is not None:
                cost = nm(by_class[c])
                break
        if cost is None and priorities and name in priorities:
            cost = nm(by_prio.get(str(priorities[name])))
        if cost is not None:
            out[name] = cost
    return out


def build_options(cfg: Config, engine, threads_override=None, *, plan=None, rip=None, model=None,
                  priorities=None) -> dict:
    """Engine options: ``cfg.engine_options()`` restricted to keys the engine knows, plus the plugin's own
    additions: ``scope`` (``plan.engine_scope``, scope.py), ``existing_copper`` (``rip.engine_existing_copper``:
    the plugin does the ripping, so only ``smooth`` is passed on), and, when the engine knows the key,
    ``net_priority`` (net name -> 1..3), ``exclude_nets`` (``do_not_touch`` expanded against `model`) and
    ``ignore_pads``. O(#nets) for the expansion."""
    known = json.loads(engine.default_options())
    opts = {k: v for k, v in cfg.engine_options().items() if k in known}
    opts["existing_copper"] = rip.engine_existing_copper if rip is not None else "fixed"
    if isinstance(known.get("scope"), dict):
        if plan is not None:
            opts["scope"] = {**known["scope"], **plan.engine_scope}
        elif cfg.get("run", "unrouted_only"):
            opts["scope"] = {**known["scope"], "unrouted_only": True}
    excluded = sorted(do_not_touch_nets(model, cfg)) if model is not None else list(cfg.get("nets", "do_not_touch"))
    for key, val in (("exclude_nets", excluded), ("ignore_pads", list(cfg.get("nets", "ignore_pads"))),
                     ("net_priority", dict(priorities or {}))):
        if key in known and val:
            opts[key] = val
    if "layer_direction" in known:
        angle = {"h": 0.0, "v": 90.0}
        dirs = {str(k): angle[str(v).lower()] for k, v in dict(cfg.get("layers", "preferred_direction")).items()
                if str(v).lower() in angle}
        if dirs:
            opts["layer_direction"] = dirs
    if "via_cost_by_net" in known and model is not None:
        costs = via_costs_by_net(cfg, model, priorities)
        if costs:
            opts["via_cost_by_net"] = costs
    if threads_override is not None:
        opts["threads"] = int(threads_override)
    return opts


FACTORY_VIA_NM = (800_000, 400_000)  # KiCad's factory net class via (diameter, drill)


def apply_via_fallback(rules: dict, cfg: Config) -> tuple:
    """VIA_NOT_SET_UP (ruling C27): the rules with ``vias.fallback_diameter_mm`` / ``vias.fallback_drill_mm``
    written into the fields the engine reads for the via size (``NetRule.via_d`` / ``via_drill``, nm) of the
    default rule and of every per-net rule that carries the "not set up" size: KiCad's factory 0.8 / 0.4 mm or
    the default rule's own (pre-patch) size. A per-net size somebody set (another value) is kept. The board's
    minimums (``min_via_diameter`` / ``min_via_drill``) still apply (``clamped`` when they raise a value).
    The same size is also written to ``rules["via_fallback"]``, the field the engine's via rule
    (``engine/src/vias.rs`` ``via_sizes``) applies under the same condition (no pre-defined via, class at the
    factory size): the engine is the single place of truth, this function only makes the report and the
    ``NetRule`` sizes agree with it. Returns ``(new_rules, info)``; `rules` is not modified. O(#per-net rules)."""
    want_d = int(round(float(cfg.get("vias", "fallback_diameter_mm")) * 1_000_000))
    want_r = int(round(float(cfg.get("vias", "fallback_drill_mm")) * 1_000_000))
    d = max(want_d, int(rules.get("min_via_diameter") or 0))
    r = max(want_r, int(rules.get("min_via_drill") or 0))
    default = dict(rules.get("default") or {})
    unset = {FACTORY_VIA_NM, (default.get("via_d"), default.get("via_drill"))}
    per_net, n = {}, 0
    for k, nr in (rules.get("per_net") or {}).items():
        if (nr.get("via_d"), nr.get("via_drill")) in unset:
            per_net[k] = {**nr, "via_d": d, "via_drill": r}
            n += 1
        else:
            per_net[k] = nr
    info = {"diameter_nm": d, "drill_nm": r, "default_before": [default.get("via_d"), default.get("via_drill")],
            "per_net": n, "per_net_total": len(per_net), "clamped": (d, r) != (want_d, want_r)}
    return {**rules, "via_fallback": [d, r], "default": {**default, "via_d": d, "via_drill": r},
            "per_net": per_net}, info


def via_fallback_text(info: dict) -> str:
    """Report line for an applied via fallback. O(1)."""
    return (f"via fallback {info['diameter_nm'] / 1e6:g} / {info['drill_nm'] / 1e6:g} mm (diameter / drill) "
            f"applied to the default rule and {info['per_net']}/{info['per_net_total']} net rules (VIA_NOT_SET_UP"
            + (", raised to the board minimums" if info.get("clamped") else "") + ")")


def apply_accepted_findings(decision: Decision, findings: list, cfg: Config, toml_path: Path) -> Config:
    """Persist accepted PAD_OUTSIDE_BOARD (-> ``nets.ignore_pads``) and DRILL_OUTSIDE_FILTER (-> appended to
    ``vias.allowed_drills_mm``) choices with ``update_config``; returns the updated Config. (B6 owns the
    finding contents; here `Finding.items` hold the pad refs / drill sizes in mm.)"""
    changes: dict = {}
    for f in findings:
        if f.code not in decision.accepted_codes:
            continue
        if f.code == "PAD_OUTSIDE_BOARD":
            cur = list(changes.get(("nets", "ignore_pads"), cfg.get("nets", "ignore_pads")))
            changes[("nets", "ignore_pads")] = cur + [i for i in f.items if i not in cur]
        elif f.code == "DRILL_OUTSIDE_FILTER":
            cur = list(changes.get(("vias", "allowed_drills_mm"), cfg.get("vias", "allowed_drills_mm")))
            try:
                add = [float(i) for i in f.items]
            except (TypeError, ValueError):
                add = []
            changes[("vias", "allowed_drills_mm")] = cur + [d for d in add if d not in cur]
    if not changes:
        return cfg
    toml_path = Path(toml_path)
    if toml_path.is_file():
        update_config(toml_path, {f"{s}.{k}": v for (s, k), v in changes.items()})
    return cfg.with_values(changes)


# ---------------------------------------------------------------- helpers
@dataclass
class _W:
    median_nm: int


_WIDTH = re.compile(r"\(width\s+([0-9.]+)\)")
_NET = re.compile(r'\(net\s+"((?:[^"\\]|\\.)*)"\)')


def widths_before(text: str) -> dict:
    """Median track width per net name of the copy's segments / arcs (rules: width_source existing_copper)."""
    by: dict = {}
    for s in pcbfile.find_items(text, ("segment", "arc")):
        chunk = text[s.start:s.end]
        w, n = _WIDTH.search(chunk), _NET.search(chunk)
        if w and n:
            by.setdefault(n.group(1), []).append(int(round(float(w.group(1)) * 1_000_000)))
    return {n: _W(int(statistics.median(v))) for n, v in by.items()}


def resolve_rules(port: BoardPort, pcb_copy: Path, copy_text: str) -> dict:
    """Rules from the copy's .kicad_pro + .kicad_dru and the port's net classes (toporlib.rules). The ``per_net``
    keys use the port's ONE net-code table (``port.net_codes()``, the same the model is exported with; gate
    review MINOR 13); a port without one (FakeBoard: its model carries codes and rules) lets the resolver derive
    them from the net classes. O(N * R)."""
    pro_p, dru_p = pcb_copy.with_suffix(".kicad_pro"), pcb_copy.with_suffix(".kicad_dru")
    pro = json.loads(pro_p.read_text()) if pro_p.is_file() else {}
    dru = dru_p.read_text() if dru_p.is_file() else None
    classes, of = port.netclasses()
    codes = port.net_codes() if hasattr(port, "net_codes") else None
    n_cu = max(2, len(pcbfile.parse_layers(copy_text)))
    return rules_mod.resolve_rules_from_project(pro, dru, netclass_of=of, classes=classes,
                                                widths_before=widths_before(copy_text), copper_layer_count=n_cu,
                                                net_codes=codes)


def _variant(src_pcb: Path, text: str, out_dir: Path) -> Path:
    """`text` as `out_dir/<name>.kicad_pcb` next to copies of the project files (so kicad-cli DRC applies the
    copied .kicad_pro / .kicad_dru)."""
    out_dir.mkdir(parents=True, exist_ok=True)
    for ext in (".kicad_pro", ".kicad_dru"):
        s = src_pcb.with_suffix(ext)
        if s.is_file():
            (out_dir / s.name).write_bytes(s.read_bytes())
    p = out_dir / src_pcb.name
    p.write_text(text)
    return p


def _unique(path: Path) -> Path:
    if not path.exists() or not any(path.iterdir()):
        return path
    for i in range(2, 1000):
        cand = path.with_name(f"{path.name}-{i}")
        if not cand.exists():
            return cand
    return path


def _zone_reach(model: dict) -> float:
    r = [p.get("clearance", 0) + p.get("min_thickness", 0) for p in model.get("pours", [])]
    return 2.0 * max(r) if r else 1_000_000.0


def _facts_result(status: str, out: RunOutcome, report: dict | None) -> dict:
    slim = {k: v for k, v in (report or {}).items() if k != "items"}
    return {"status": status, "routed": out.routed, "total": out.total, "stop_reason": out.stop_reason,
            "cancelled": out.cancelled, "rounds": out.rounds, "router_s": round(out.router_s, 3),
            "wall_s": round(out.wall_s, 3), "items": len(out.items), "ripped": len(out.ripped), "drc": out.drc,
            "findings": [asdict(f) for f in out.findings], "route": slim, "via_fallback": out.via_fallback,
            "pcb_copy": str(out.pcb_copy) if out.pcb_copy else None}


# ---------------------------------------------------------------- the flow
def run(port: BoardPort, ui: UiPort, cfg: Config | None = None, engine=None, *, tmp_root: Path,
        now: datetime | None = None, config_findings=(), versions: dict | None = None) -> RunOutcome:
    """Run Topor on the board behind `port`; see the module doc. `cfg` None loads ``topor.toml`` from the
    project directory (first run: ``ui.first_run_notice``); `engine` defaults to ``topor_engine``.

    Returns a :class:`RunOutcome`; exceptions (not BaseException: the PyO3 panic is caught by ``app.main``) are
    shown with ``ui.error`` and give ``status == "error"`` (INTERNAL; ENGINE_UNRESPONSIVE when the engine ignored
    Cancel), except after a successful commit: then the status stays ``accepted`` with the POST_COMMIT_STEP_FAILED
    warning (ruling C30). A report folder that cannot be created / written is the REPORT_DIR_UNWRITABLE error
    before the board is read (ruling C32)."""
    t0 = time.monotonic()
    if engine is None:
        import topor_engine as engine  # noqa: PLC0415
    project_dir = Path(port.project_dir())
    board_name = Path(port.board_name()).stem
    findings = list(config_findings)
    if cfg is None:
        cfg, cf = load_config(project_dir)
        findings += cf
        for f in cf:
            if f.code == "CFG_CREATED":
                ui.first_run_notice(project_dir / CONFIG_NAME)
    ts = (now or datetime.now()).strftime("%Y%m%d-%H%M%S")
    ver = dict(versions or {})
    ver.setdefault("plugin", CURRENT_VERSION)
    ver.setdefault("engine", str(engine.version()) if hasattr(engine, "version") else "?")
    report_dir = cfg.report_dir_for(project_dir, board_name, ts)
    try:
        report_dir = _unique(report_dir)
        report_dir.mkdir(parents=True, exist_ok=True)
        log = _Log(report_dir / "log.txt")
    except OSError as e:  # ruling C32: an error dialog, never a silent end
        err = Finding("REPORT_DIR_UNWRITABLE", "error",
                      f"The report folder {report_dir} could not be created or written: {type(e).__name__}: {e}",
                      messages.fix_for("REPORT_DIR_UNWRITABLE"), (str(report_dir),))
        findings.append(err)
        ui.set_context(board=board_name, report_dir=None, log_path=None, versions=ver)
        ui.error(err)
        return RunOutcome("error", findings=findings, error=err, wall_s=time.monotonic() - t0)
    report = RunReport(report_dir, {"version": ver["engine"], "board": board_name, "timestamp": ts,
                               "versions": dict(ver)})
    out = RunOutcome("error", report=report, report_dir=report_dir, findings=findings)

    def ctx() -> None:
        ui.set_context(board=board_name, report_dir=report_dir, log_path=report_dir / "log.txt", versions=ver)

    try:
        ctx()
        return _run(port, ui, cfg, engine, out, report, log, ctx, Path(tmp_root), project_dir, board_name, ts, t0)
    except Exception as e:  # noqa: BLE001 - reported, never crashes the host
        log.exc()
        log.line(f"error: {type(e).__name__}: {e}")
        out.wall_s = time.monotonic() - t0
        if out.commit is not None:  # ruling C30: the board WAS changed -> still accepted, with a warning
            w = post_commit_finding("a step after the commit", e)
            out.findings.append(w)
            out.status = "accepted"
            report.facts["post_commit_failed"] = report.facts.get("post_commit_failed", []) + [w]
            try:
                report.write(result=_facts_result("accepted", out, None))
            except Exception:  # noqa: BLE001
                log.exc()
            ctx()
            ui.error(w)
            return out
        out.status = "error"
        if isinstance(e, EngineUnresponsive):
            out.error = Finding("ENGINE_UNRESPONSIVE", "error", str(e), messages.fix_for("ENGINE_UNRESPONSIVE"))
        else:  # incl. the engine panic: RouteJob.wait raises RuntimeError("routing thread panicked: ...")
            out.error = Finding("INTERNAL", "error", f"{type(e).__name__}: {e}", messages.fix_for("INTERNAL"))
        code = out.error.code
        out.findings.append(out.error)
        report.facts["findings"] = out.findings
        report.set_outcome("error", code)
        try:
            report.write(result=_facts_result("error", out, None))
        except Exception:  # noqa: BLE001
            log.exc()
        ctx()
        ui.error(out.error)
        return out


def post_commit_finding(step: str, e: BaseException) -> Finding:
    """POST_COMMIT_STEP_FAILED (ruling C30): the routes were committed, then `step` failed. O(1)."""
    return Finding("POST_COMMIT_STEP_FAILED", "warning",
                   f"The routes were committed, then {step} failed: {type(e).__name__}: {e}",
                   messages.fix_for("POST_COMMIT_STEP_FAILED"), (step,))


def _run(port, ui, cfg, engine, out, report, log, ctx, tmp_root, project_dir, board_name, ts, t0) -> RunOutcome:
    findings = out.findings
    report_dir = out.report_dir
    state = {"cancel": False, "last": Progress(), "judged_key": None, "judged": None, "last_result": None,
             "rounds": 0}

    def emit(phase: str) -> None:
        p = state["last"]
        q = Progress(phase, p.iteration, p.routed, p.total, p.vias, time.monotonic() - t0)
        if ui.progress(q) is False:
            state["cancel"] = True

    def finish(status: str, *, error_code: str = "") -> RunOutcome:
        out.status = status
        out.wall_s = time.monotonic() - t0
        report.set_outcome(status, error_code)
        return out

    tmp_root.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="topor-", dir=tmp_root))
    out.work_dir = work
    # 1. the live board, as KiCad holds it right now (unsaved edits included)
    pcb_copy = port.save_copy(work / "current")
    out.pcb_copy = pcb_copy
    copy_text = pcb_copy.read_text()
    log.line(f"save_as copy: {pcb_copy} ({len(copy_text)} bytes)")
    rules = resolve_rules(port, pcb_copy, copy_text)
    report.facts["floors_applied"] = list(rules.get("floors_applied") or [])  # report.md Settings (F I-4)
    pro_p = pcb_copy.with_suffix(".kicad_pro")
    try:
        pro = json.loads(pro_p.read_text()) if pro_p.is_file() else {}
    except ValueError:
        pro = {}
    # VIA_NOT_SET_UP (ruling C27): the fallback via size goes into the rules the engine receives
    via_fb = preflight_mod.via_setup_missing(pro, cfg)
    if via_fb:
        rules, out.via_fallback = apply_via_fallback(rules, cfg)
    model = port.read_model(rules, pcb_copy, layer_roles=cfg.layer_roles)
    if via_fb and model.get("rules"):
        # the engine reads model["rules"]: patch them too (idempotent for the IPC model, which embeds `rules`;
        # needed for a port whose model carries its own rules, e.g. FakeBoard with a harness model). The
        # reported info is the one of the engine's input.
        mr, out.via_fallback = apply_via_fallback(model["rules"], cfg)
        model = {**model, "rules": mr}
    out.model = model
    log.line(f"model: {len(model.get('pads', []))} pads, {len(model.get('kept', []))} tracks, "
             f"{len(model.get('vias', []))} vias, {len(model.get('nets', []))} nets")
    if out.via_fallback:
        log.line(via_fallback_text(out.via_fallback))

    # 2. pre-flight
    sel = port.selection()
    findings += [f for f in preflight_findings(cfg, (), copy_text, model=model, rules=rules, sel=sel, pro=pro)
                 if f not in findings]
    errors = [f for f in findings if f.severity == "error"]
    orig_drc = None
    if not errors:
        emit("preflight_drc")
        # ruling C29: this DRC refills the zones (--refill-zones) and saves the refilled board into its own file
        # (the save_as copy stays as KiCad wrote it); the engine's pour fills / reach / dangling ends come from it,
        # never from the live board's possibly stale fills. No extra kicad-cli run.
        orig_pcb = _variant(pcb_copy, copy_text, work / "orig")
        orig_drc = run_drc(orig_pcb, work / "drc-orig.json", save_board=True)
        out.refilled_pcb = orig_pcb
        out.orig_drc = orig_drc
        log.line(f"DRC of the untouched copy: {len(orig_drc.violations)} violations, "
                 f"{len(orig_drc.unconnected)} unconnected, {orig_drc.seconds:.1f}s")
        pf = preflight_mod.preexisting_drc(orig_drc, cfg)
        if pf is not None:
            findings.append(pf)
    conns, nets = connection_estimate(model)
    before_stats = before_stats_from_model(model)
    summary = RunSummary(connections=conns, nets=nets, layers=len(model.get("layers", [])),
                         engine_version=report.facts["version"], board=board_name,
                         has_selection=bool(sel.footprints or sel.pads),
                         connections_by_scope=connections_by_scope(sel, model, cfg))
    classes_of = port.netclasses()[1]
    priorities, prio_findings = net_priorities([n["name"] for n in model.get("nets", [])], classes_of, cfg)
    pre_plan = scope_from_selection(sel, model, cfg)
    findings += [f for f in prio_findings + pre_plan.findings if f not in findings]
    report.facts.update(findings=findings, nets=nets, layers=summary.layers)
    decision = ui.preflight(findings, summary, cfg)
    errors = [f for f in findings if f.severity == "error"]
    if errors or not decision.proceed or state["cancel"]:
        log.line("aborted at pre-flight: " + (", ".join(f.code for f in errors) or "declined"))
        report.write(result=_facts_result("aborted_preflight", out, None))
        report.set_outcome("aborted_preflight")
        report.write(result=_facts_result("aborted_preflight", out, None))
        return finish("aborted_preflight")
    toml = project_dir / CONFIG_NAME
    cfg = apply_accepted_findings(decision, findings, cfg, toml)
    over = {k: v for k, v in decision.overrides.items() if k in _ROUTE_OVERRIDES}
    if over:
        cfg = cfg.with_values({("run", k): v for k, v in over.items()})
        if decision.save_to_toml and toml.is_file():
            update_config(toml, {f"run.{k}": v for k, v in over.items()})
    # `scope_whole` (dialog: whole board although a selection exists) ignores the selection; never written to topor.toml.
    plan = scope_from_selection(sel, model, cfg, whole=bool(decision.overrides.get("scope_whole")))
    log.line(f"decision: overrides {decision.overrides}, accepted {decision.accepted_codes}")

    # 3. rip plan -> the unrouted baseline
    rip = rip_plan(model, plan, cfg)
    ripped = rip.remove_uuids
    out.ripped = ripped
    ripped_text = pcbfile.remove_items(copy_text, ripped)
    ripped_pcb = _variant(pcb_copy, ripped_text, work / "ripped")
    out.ripped_pcb = ripped_pcb
    if ripped:
        emit("preflight_drc")
        base_drc = run_drc(ripped_pcb, work / "drc-ripped.json", save_board=True)  # saved = zones refilled
    else:
        base_drc = orig_drc
    out.base_drc = base_drc
    log.line(f"rip plan: {len(ripped)} items; baseline DRC {len(base_drc.violations)} violations, {len(base_drc.unconnected)} unconnected")
    eng_model = filter_model(model, ripped)
    if ripped:  # ruling C20: the ripped copy refilled by its own DRC
        eng_model = refill_pours(eng_model, ripped_pcb.read_text())
    elif out.refilled_pcb is not None:  # ruling C29: the untouched copy refilled by the original DRC
        eng_model = refill_pours(eng_model, out.refilled_pcb.read_text())

    # 4. route + DRC loop
    opts = build_options(cfg, engine, plan=plan, rip=rip, model=model, priorities=priorities)
    budget = float(opts.get("wall_clock_s", 60.0))
    no_budget = budget <= 0
    geometry = cfg.get("output", "geometry")
    max_err = int(rules.get("max_error") or 5000)
    layer_names = {int(l["id"]): l["name"] for l in model.get("layers", [])}
    net_names = {int(n["code"]): n["name"] for n in model.get("nets", [])}
    routed_dir = work / "routed"

    def route_fn(m, o):
        if state["cancel"] and state["last_result"] is not None:
            return state["last_result"]  # cancelled: no further rounds, the best result stays
        if no_budget:
            o = {**o, "wall_clock_s": 0.0}
        job = engine.route_async(json.dumps(m), json.dumps(o))
        last_poll, cancelled_at = -1e9, None
        while True:  # ends when the job is done (also after an engine panic: B-1) or CANCEL_GRACE_S after Cancel
            done = job.done()
            now = time.monotonic()
            if done or now - last_poll >= POLL_S:
                last_poll = now
                p = Progress.from_json(json.loads(job.progress()))
                state["last"] = p
                if ui.progress(p) is False:
                    state["cancel"] = True
                if state["cancel"] and cancelled_at is None:
                    cancelled_at = now
                    job.cancel()
            if done:
                break
            if cancelled_at is not None and now - cancelled_at > CANCEL_GRACE_S:
                log.line(f"engine did not stop within {CANCEL_GRACE_S} s after Cancel")
                raise EngineUnresponsive(f"The routing engine did not stop within {CANCEL_GRACE_S} s after Cancel.")
            time.sleep(0.02)
        res = json.loads(job.wait())  # a panicked job raises RuntimeError("routing thread panicked: ...")
        items = res.get("items") or []
        if geometry == "segments":
            items = preview_mod.arcs_to_segments(items, max_err)
            res["items"] = items
        state["last_result"] = {"items": items, "remove_uuids": [], "report": res}
        return state["last_result"]

    def drc_fn(items, remove_uuids=()):
        key = json.dumps([items, sorted(remove_uuids)], sort_keys=True, default=str)
        if key == state["judged_key"]:  # a cancelled run returns the same result again: judged already
            return state["judged"]
        emit("drc")
        state["rounds"] += 1
        # every call starts from the UNROUTED copy text (drcloop contract)
        text = pcbfile.remove_items(ripped_text, list(remove_uuids))
        with_ids = [{**it, "uuid": str(_uuid.uuid4())} for it in items]
        text = pcbfile.append_items(text, with_ids, layer_names=layer_names, net_names=net_names)
        pcb = _variant(pcb_copy, text, routed_dir)
        out.routed_pcb = pcb
        rep = run_drc(pcb, routed_dir / "drc-routed.json", save_board=True)  # refilled zones for the preview
        log.line(f"DRC round {state['rounds']}: {len(rep.violations)} violations, {len(rep.unconnected)} unconnected, "
                 f"{rep.seconds:.1f}s")
        j = DrcJudgement(rep, {w["uuid"]: orig for w, orig in zip(with_ids, items)})
        state["judged_key"], state["judged"] = key, j
        return j

    clearance = float(max(rules.get("min_clearance", 0), (rules.get("default") or {}).get("clearance", 0)))
    loop = drcloop.route_with_drc_loop(
        eng_model, opts, route_fn=route_fn, drc_fn=drc_fn, base_drc=base_drc, orig_drc=orig_drc,
        budget_s=NO_BUDGET_S if no_budget else budget, loop_cap=int(opts.get("drc_loop_cap", 5)),
        reach_nm=_zone_reach(model), clearance_nm=clearance)
    rr = loop.router_report or {}
    out.items = [{**it, "group": it.get("group") or "topor"} for it in loop.items]
    out.routed, out.total = int(rr.get("routed", 0)), int(rr.get("connections_total", 0))
    out.stop_reason = rr.get("stop_reason")
    out.cancelled = bool(state["cancel"] or out.stop_reason == "cancel")
    out.rounds, out.router_s = loop.rounds, loop.router_s
    classified = classify(loop.routed_drc, base_drc, loop.created, unconnected_baseline=orig_drc)
    out.drc = classified
    inc = classified["incomplete"]
    partial = int(rr.get("partial", 0))
    scope_info = {"mode": cfg.get("run", "scope_mode") if (sel.footprints or sel.pads) else "whole board",
                  "ignored_pads": list(cfg.get("nets", "ignore_pads")),
                  "accepted_codes": list(decision.accepted_codes),
                  "fallbacks": [via_fallback_text(out.via_fallback)] if out.via_fallback else []}
    build_report({**rr, "items": out.items}, classified, cfg, findings, scope_info, before_stats,
                 {"router_s": out.router_s, "drc_rounds": loop.rounds}, report=report,
                 facts={"layers": report.facts.get("layers"), "versions": report.facts.get("versions"),
                        "notes": [f"geometry: {geometry}", f"DRC rounds: {loop.rounds}"] + (
                            ["time budget exhausted"] if loop.budget_exhausted else [])})
    log.line(f"routed {out.routed}/{out.total}, stop {out.stop_reason}, rounds {loop.rounds}, "
             f"routing_new {classified['routing_new']}, unconnected_new {inc['unconnected_new']}")
    drc_src = routed_dir / "drc-routed.json"
    if not out.items:  # nothing to show or apply (cancelled early, or the router discarded everything)
        if out.routed == 0 and not out.cancelled:
            findings.append(Finding("NOTHING_ROUTED", "warning", "The router produced no copper.",
                                    "See the connection reasons in the report; relax a setting or route by hand."))
        status = "cancelled" if out.cancelled else "rejected"
        report.write(result=_facts_result(status, out, rr), drc_src=drc_src)
        report.set_outcome(status)
        report.write(result=_facts_result(status, out, rr), drc_src=drc_src)
        ctx()
        ui.done(status, report, report_dir)
        return finish(status)

    # 5. preview (report.md + preview.png exist before the dialog opens)
    emit("preview")
    png = report_dir / "preview.png"
    caption = [report.summary_lines()[i] for i in (0, 1, 5)]
    try:
        _, notes = preview_mod.render_ex(
            pcb_copy, out.routed_pcb, png, caption, new_items=out.items, outline=model.get("outline"),
            layer_names=[l["name"] for l in model.get("layers", []) if l.get("routable", True)] or None)
        report.facts["notes"] = report.facts["notes"] + notes
    except Exception as e:  # noqa: BLE001 - the numbers still allow a decision
        log.exc()
        log.line(f"preview failed: {e}")
        findings.append(Finding("PREVIEW_FAILED", "warning", f"The preview could not be rendered: {e}",
                                f"See {report_dir / 'log.txt'}; the DRC numbers in the summary are still valid."))
    report.write(result=_facts_result("pending", out, rr), drc_src=drc_src)
    ctx()
    accepted = bool(ui.preview(png, report))
    if not accepted:
        report.set_outcome("rejected")
        report.write(result=_facts_result("rejected", out, rr), drc_src=drc_src)
        ctx()
        ui.done("rejected", report, report_dir)
        return finish("rejected")

    # 6. Accept: one commit = one undo step. Everything after it is a separate step (ruling C28: refill only with
    # run.refill_after_accept; cleanup / teardrops opt-in) and a failure there keeps "accepted" (ruling C30).
    n_partial = sum(1 for it in out.items if it.get("group") == "topor-partial")
    req = CommitRequest(remove_uuids=list(ripped), items=out.items,
                        message=f"Topor: routed {out.routed}/{out.total}", group=True)
    out.commit = port.commit(req)
    status = "accepted"
    out.status = status
    log.line(f"committed: {out.commit.removed} removed, {out.commit.created} created")
    post_failed: list = []

    def after_commit(step: str, fn) -> bool:
        try:
            fn()
            return True
        except Exception as e:  # noqa: BLE001 - the routes are on the board: report, never "not changed"
            log.exc()
            log.line(f"after the commit, {step} failed: {type(e).__name__}: {e}")
            w = post_commit_finding(step, e)
            findings.append(w)
            post_failed.append(w)
            report.facts["post_commit_failed"] = list(post_failed)
            return False

    report.facts["zones_refilled"] = False
    if cfg.get("run", "refill_after_accept"):
        report.facts["zones_refilled"] = after_commit("zone refill", port.refill_zones)
    skipped = []
    for key, action in (("kicad_cleanup", ACTION_CLEANUP), ("kicad_teardrops", ACTION_TEARDROPS)):
        if not cfg.get("run", key):
            continue
        if n_partial or partial:
            skipped.append(key)
        else:
            after_commit(f"the KiCad action {action}", lambda a=action: port.run_action(a))
    if skipped:
        findings.append(Finding("CLEANUP_SKIPPED_PARTIAL", "info",
                                "KiCad cleanup / teardrops were skipped because the result has partial routes.",
                                "Finish the partial connections by hand, then run KiCad → Tools → Cleanup Tracks & Vias "
                                "yourself.", tuple(skipped)))
    report.facts["notes"] = report.facts["notes"] + list(out.commit.notes)
    report.facts["findings"] = list(findings)  # incl. CLEANUP_SKIPPED_PARTIAL / POST_COMMIT_STEP_FAILED
    report.set_outcome(status)
    out.wall_s = time.monotonic() - t0
    after_commit("writing the report", lambda: report.write(result=_facts_result(status, out, rr), drc_src=drc_src))
    ctx()
    if not after_commit("the done notice", lambda: ui.done(status, report, report_dir)):
        try:
            ui.error(post_failed[-1])  # the notice failed: tell the user the routes are on the board
        except Exception:  # noqa: BLE001
            log.exc()
    return finish(status)
