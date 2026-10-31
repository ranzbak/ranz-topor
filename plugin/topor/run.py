"""The run flow (B5): ``save_as`` copy -> rules -> model -> pre-flight -> rip plan -> route (progress, cancel) ->
DRC loop on the copy -> SVG preview -> Accept = one commit / Reject = nothing -> report folder.

``run(port, ui, cfg, engine, *, tmp_root)`` never touches the live board except in the Accept branch, which is
exactly one ``port.commit`` followed by ``refill_zones`` and the optional KiCad cleanup / teardrop actions. Every
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
from toporlib.detour import detour_line, detour_stats
from toporlib.drc import classify, run_drc
from toporlib.drcloop import DrcJudgement
from toporlib.kicad_cli import kicad_cli

from . import preview as preview_mod
from .config import CONFIG_NAME, Config, load_config, update_config
from .findings import Finding
from .layers import resolve_layer_roles
from .ports import BoardPort, CommitRequest, CommitResult, Decision, Progress, RunSummary, UiPort
from .report import RunReport
from .schema import CURRENT_VERSION

ACTION_CLEANUP = "pcbnew.GlobalEdit.cleanupTracksAndVias"
ACTION_TEARDROPS = "pcbnew.GlobalEdit.editTeardrops"
POLL_S = 0.25
NO_BUDGET_S = 1.0e9  # timeout_s = 0: the DRC loop gets no budget cut; drc_loop_cap still bounds it
_ROUTE_OVERRIDES = ("scope_mode", "existing_copper", "respect_locked", "partial_routes", "timeout_s", "threads")


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
    ripped_pcb: Path | None = None
    routed_pcb: Path | None = None


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
def preflight_findings(cfg: Config, config_findings, copy_text: str) -> list:
    """Pre-flight findings known to the alpha: config findings, layer roles, kicad-cli presence. (B6 extends.)"""
    out = list(config_findings)
    ver = pcbfile.file_version(copy_text)
    if ver is None or ver < pcbfile.KICAD10_VERSION:
        out.append(Finding("PCB_FORMAT_OLD", "error",
                           f"The board file format (version {ver}) is older than KiCad 10 ({pcbfile.KICAD10_VERSION}); "
                           "topor edits the file text and would silently skip items written in the old format.",
                           "Open the board in KiCad 10 and save it (File → Save), then run Topor again.",
                           (str(ver),)))
    copper = [(l["name"], l["type"]) for l in pcbfile.parse_layers(copy_text)]
    out += resolve_layer_roles(copper, cfg.layer_roles)[1]
    if not Path(kicad_cli()).is_file():
        out.append(Finding("KICAD_CLI_MISSING", "error", f"kicad-cli was not found at {kicad_cli()}.",
                           "Install KiCad 10 with its command-line tools or set the KICAD_CLI environment variable "
                           "to the kicad-cli executable.", (str(kicad_cli()),)))
    return out


def connection_estimate(model: dict) -> tuple:
    """(connections, nets with >= 2 pads): spanning-tree estimate pads - 1 per net. (B6 makes it exact.)"""
    pads: dict = {}
    for p in model.get("pads", []):
        if p.get("net"):
            pads[p["net"]] = pads.get(p["net"], 0) + 1
    multi = [n for n, c in pads.items() if c >= 2]
    return sum(pads[n] - 1 for n in multi), len(multi)


def rip_plan(model: dict, cfg: Config) -> list:
    """uuids to remove on Accept. `existing_copper = fixed` / `smooth` (default fixed): nothing; `rip`: every
    track and via, except locked ones when `respect_locked` and free vias (copper that connects to nothing
    but a via, the harness rule); `unrouted_only` ripples nothing. (B7 refines with scope / do_not_touch.)"""
    if cfg.get("run", "existing_copper") != "rip" or cfg.get("run", "unrouted_only"):
        return []
    keep_locked = cfg.get("run", "respect_locked")
    ripped = [k["uuid"] for k in model.get("kept", []) if not (keep_locked and k.get("locked"))]
    ripped += [v["uuid"] for v in model.get("vias", [])
               if not (keep_locked and v.get("locked")) and not v.get("free")]
    return ripped


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
    """The model with pour fills, pour reach (pads / vias) and dangling ends recomputed from the refilled ripped
    copy, matching the harness order rip -> refill -> export (ruling C20). Zones are matched by uuid; a zone
    that is absent from the file or has no filled polygons keeps its model fill (warning in `notes`).
    Same functions as the IPC exporter (model_ipc.pour_reach / dangling_ends). O(F * (pads + vias))."""
    from .model_ipc import dangling_ends, pour_reach  # noqa: PLC0415 - imports kipy

    fills = pcbfile.parse_zone_fills(refilled_text)
    ids = {l["name"]: int(l["id"]) for l in model.get("layers", [])}
    pours = []
    for p in model.get("pours", []):
        polys = fills.get(p["uuid"]) or []
        fl = [{"layer": ids[q["layer"]], "outer": q["pts"], "holes": []} for q in polys
              if q["layer"] in ids and ids[q["layer"]] in p["layers"]]
        if not fl:
            pours.append(p)
            continue
        rp, rv = pour_reach(p["net"], p["layers"], fl, model["pads"], model["vias"])
        pours.append({k: v for k, v in {**p, "fills": fl, "pads": rp, "vias": rv}.items() if k != "warning"})
    cu = [int(l["id"]) for l in model.get("layers", [])]
    return {**model, "pours": pours,
            "dangling_ends": dangling_ends(model["kept"], model["vias"], model["pads"], cu)}


def build_options(cfg: Config, engine, threads_override=None) -> dict:
    """Engine options: ``cfg.engine_options()`` restricted to keys the engine knows, plus the plugin's own
    additions. The plugin does the ripping, so the engine always sees ``existing_copper = fixed``. (B7 adds
    scope / net_priority / exclude_nets / ignore_pads here.)"""
    known = json.loads(engine.default_options())
    opts = {k: v for k, v in cfg.engine_options().items() if k in known}
    opts["existing_copper"] = "fixed"
    if cfg.get("run", "unrouted_only") and isinstance(known.get("scope"), dict):
        opts["scope"] = {**known["scope"], "unrouted_only": True}
    for key, val in (("exclude_nets", list(cfg.get("nets", "do_not_touch"))),
                     ("ignore_pads", list(cfg.get("nets", "ignore_pads")))):
        if key in known and val:
            opts[key] = val
    if threads_override is not None:
        opts["threads"] = int(threads_override)
    return opts


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
    """Rules from the copy's .kicad_pro + .kicad_dru and the port's net classes (toporlib.rules). O(N * R)."""
    pro_p, dru_p = pcb_copy.with_suffix(".kicad_pro"), pcb_copy.with_suffix(".kicad_dru")
    pro = json.loads(pro_p.read_text()) if pro_p.is_file() else {}
    dru = dru_p.read_text() if dru_p.is_file() else None
    classes, of = port.netclasses()
    n_cu = max(2, len(pcbfile.parse_layers(copy_text)))
    return rules_mod.resolve_rules_from_project(pro, dru, netclass_of=of, classes=classes,
                                                widths_before=widths_before(copy_text), copper_layer_count=n_cu)


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


def _preexisting_finding(rep) -> Finding | None:
    n, u = len(rep.violations), len(rep.unconnected)
    if not n and not u:
        return None
    types = ", ".join(f"{t} {c}" for t, c in sorted(rep.by_type.items(), key=lambda kv: (-kv[1], kv[0]))[:6])
    return Finding("PREEXISTING_DRC", "info",
                   f"KiCad DRC already reports {n} violation(s) and {u} unconnected item(s) on the board before "
                   f"routing" + (f" ({types})" if types else "") + "; they are listed as pre-existing.",
                   "Nothing to do; run Inspect → Design Rules Checker in KiCad to see them.")


def _facts_result(status: str, out: RunOutcome, report: dict | None) -> dict:
    slim = {k: v for k, v in (report or {}).items() if k != "items"}
    return {"status": status, "routed": out.routed, "total": out.total, "stop_reason": out.stop_reason,
            "cancelled": out.cancelled, "rounds": out.rounds, "router_s": round(out.router_s, 3),
            "wall_s": round(out.wall_s, 3), "items": len(out.items), "ripped": len(out.ripped), "drc": out.drc,
            "findings": [asdict(f) for f in out.findings], "route": slim,
            "pcb_copy": str(out.pcb_copy) if out.pcb_copy else None}


# ---------------------------------------------------------------- the flow
def run(port: BoardPort, ui: UiPort, cfg: Config | None = None, engine=None, *, tmp_root: Path,
        now: datetime | None = None, config_findings=(), versions: dict | None = None) -> RunOutcome:
    """Run Topor on the board behind `port`; see the module doc. `cfg` None loads ``topor.toml`` from the
    project directory (first run: ``ui.first_run_notice``); `engine` defaults to ``topor_engine``.

    Returns a :class:`RunOutcome`; exceptions (not BaseException: the PyO3 panic is caught by ``app.main``) are
    shown with ``ui.error`` and give ``status == "error"``."""
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
    report_dir = _unique(cfg.report_dir_for(project_dir, board_name, ts))
    report_dir.mkdir(parents=True, exist_ok=True)
    log = _Log(report_dir / "log.txt")
    ver = dict(versions or {})
    ver.setdefault("plugin", CURRENT_VERSION)
    ver.setdefault("engine", str(engine.version()) if hasattr(engine, "version") else "?")
    report = RunReport(report_dir, {"version": ver["engine"], "board": board_name, "timestamp": ts})
    out = RunOutcome("error", report=report, report_dir=report_dir, findings=findings)

    def ctx() -> None:
        ui.set_context(board=board_name, report_dir=report_dir, log_path=report_dir / "log.txt", versions=ver)

    try:
        ctx()
        return _run(port, ui, cfg, engine, out, report, log, ctx, Path(tmp_root), project_dir, board_name, ts, t0)
    except Exception as e:  # noqa: BLE001 - reported, never crashes the host
        log.exc()
        log.line(f"error: {type(e).__name__}: {e}")
        out.status = "error"
        out.wall_s = time.monotonic() - t0
        out.error = Finding("INTERNAL", "error", f"{type(e).__name__}: {e}",
                            f"see {report_dir / 'log.txt'}; file a bug with the save_as copy (docs/dev/debug.md)")
        out.findings.append(out.error)
        report.facts["findings"] = out.findings
        report.set_outcome("error", "INTERNAL")
        try:
            report.write(result=_facts_result("error", out, None))
        except Exception:  # noqa: BLE001
            log.exc()
        ctx()
        ui.error(out.error)
        return out


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
    model = port.read_model(rules, pcb_copy, layer_roles=cfg.layer_roles)
    out.model = model
    log.line(f"model: {len(model.get('pads', []))} pads, {len(model.get('kept', []))} tracks, "
             f"{len(model.get('vias', []))} vias, {len(model.get('nets', []))} nets")

    # 2. pre-flight
    findings += [f for f in preflight_findings(cfg, (), copy_text) if f not in findings]
    errors = [f for f in findings if f.severity == "error"]
    orig_drc = None
    if not errors:
        emit("preflight_drc")
        orig_drc = run_drc(pcb_copy, work / "drc-orig.json")
        out.orig_drc = orig_drc
        log.line(f"DRC of the untouched copy: {len(orig_drc.violations)} violations, "
                 f"{len(orig_drc.unconnected)} unconnected, {orig_drc.seconds:.1f}s")
        pf = _preexisting_finding(orig_drc)
        if pf is not None:
            findings.append(pf)
    conns, nets = connection_estimate(model)
    sel = port.selection()
    summary = RunSummary(connections=conns, nets=nets, layers=len(model.get("layers", [])),
                         engine_version=report.facts["version"], board=board_name,
                         has_selection=bool(sel.footprints or sel.pads),
                         connections_by_scope={"whole": conns})
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
    # `scope_whole` is honoured (the alpha routes the whole board anyway) and never written to topor.toml.
    log.line(f"decision: overrides {decision.overrides}, accepted {decision.accepted_codes}")

    # 3. rip plan -> the unrouted baseline
    ripped = rip_plan(model, cfg)
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
    if ripped:
        eng_model = refill_pours(eng_model, ripped_pcb.read_text())

    # 4. route + DRC loop
    opts = build_options(cfg, engine)
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
        last_poll, cancelled_job = -1e9, False
        while True:
            done = job.done()
            now = time.monotonic()
            if done or now - last_poll >= POLL_S:
                last_poll = now
                p = Progress.from_json(json.loads(job.progress()))
                state["last"] = p
                if ui.progress(p) is False:
                    state["cancel"] = True
                if state["cancel"] and not cancelled_job:
                    cancelled_job = True
                    job.cancel()
            if done:
                break
            time.sleep(0.02)
        res = json.loads(job.wait())
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
    stats = detour_stats(rr.get("connections"))
    report.facts.update(
        routed=out.routed, total=out.total, partial=partial, failed=rr.get("failed"), nets=rr.get("nets_total", report.facts.get("nets")),
        vias=rr.get("vias"), vias_before=len(model.get("vias", [])),
        length_mm=None if rr.get("length_outside_pads_nm") is None else round(rr["length_outside_pads_nm"] / 1e6, 3),
        routing_new=classified["routing_new"], preexisting=classified["routing_preexisting"],
        unconnected=inc["unconnected_new"], detour_line=detour_line(stats), stop_reason=out.stop_reason,
        iterations=rr.get("iterations"), elapsed_s=round(out.router_s, 1), threads=rr.get("threads_used"),
        existing_copper=cfg.get("run", "existing_copper"), respect_locked=cfg.get("run", "respect_locked"),
        non_default=len(cfg.non_defaults()), findings=findings,
        notes=[f"geometry: {geometry}", f"DRC rounds: {loop.rounds}"] + (
            ["time budget exhausted"] if loop.budget_exhausted else []))
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

    # 6. Accept: one commit, one undo step
    n_partial = sum(1 for it in out.items if it.get("group") == "topor-partial")
    req = CommitRequest(remove_uuids=list(ripped), items=out.items,
                        message=f"Topor: routed {out.routed}/{out.total}", group=True)
    out.commit = port.commit(req)
    port.refill_zones()
    skipped = []
    for key, action in (("kicad_cleanup", ACTION_CLEANUP), ("kicad_teardrops", ACTION_TEARDROPS)):
        if not cfg.get("run", key):
            continue
        if n_partial or partial:
            skipped.append(key)
        else:
            port.run_action(action)
    if skipped:
        findings.append(Finding("CLEANUP_SKIPPED_PARTIAL", "info",
                                "KiCad cleanup / teardrops were skipped because the result has partial routes.",
                                "Finish the partial connections by hand, then run Inspect → Cleanup Tracks & Vias "
                                "yourself.", tuple(skipped)))
    report.facts["notes"] = report.facts["notes"] + list(out.commit.notes)
    status = "accepted"
    report.set_outcome(status)
    out.wall_s = time.monotonic() - t0
    report.write(result=_facts_result(status, out, rr), drc_src=drc_src)
    ctx()
    ui.done(status, report, report_dir)
    return finish(status)
