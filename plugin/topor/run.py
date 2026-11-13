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
import math
import os
import re
import statistics
import tempfile
import time
import traceback
import uuid as _uuid
from dataclasses import asdict, dataclass, field, replace
from datetime import datetime
from pathlib import Path

from toporlib import compass as compass_mod
from toporlib import drcloop, pcbfile
from toporlib import rules as rules_mod
from toporlib.drc import classify, new_routing, run_drc
from toporlib.drcloop import DrcJudgement
from toporlib.kicad_cli import KicadCliCancelled, KicadCliTimeout

from . import messages
from . import preflight as preflight_mod
from . import postrun, runlock
from . import preview as preview_mod
from . import probe as probe_mod
from .config import CONFIG_NAME, Config, cache_root, load_config, update_config
from .findings import Finding
from .layers import resolve_layer_roles
from .pourreach import dangling_ends, pour_reach
from .priority import net_priorities
from .ports import BoardPort, CommitRequest, CommitResult, Decision, Progress, RunSummary, Selection, UiPort
from .report import RunReport, before_stats_from_model, build_report, limit_text, time_limit_text
from .schema import CURRENT_VERSION
from .scope import connections_by_scope, do_not_touch_nets, rip_plan, scope_from_selection

ACTION_CLEANUP = "pcbnew.GlobalEdit.cleanupTracksAndVias"
ACTION_TEARDROPS = "pcbnew.GlobalEdit.editTeardrops"
POLL_S = 0.25
NO_BUDGET_S = 1.0e9  # timeout_s = 0: the DRC loop gets no budget cut; drc_loop_cap still bounds it
_ROUTE_OVERRIDES = ("scope_mode", "existing_copper", "respect_locked", "timeout_s", "timeout_clock", "drawing_style")
#: pre-flight override -> (section, key) in topor.toml; the others are [run] keys of the same name
_OVERRIDE_KEY = {"drawing_style": ("drawing", "style")}


class BoardChanged(RuntimeError):
    """The board's fingerprint differs from the start of the run (-> BOARD_CHANGED_DURING_RUN, nothing committed)."""


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
    router_cpu_s: float = 0.0  # CPU seconds of this process during the engine jobs, all threads summed
    wall_s: float = 0.0
    work_dir: Path | None = None
    pcb_copy: Path | None = None
    refilled_pcb: Path | None = None  # kicad-cli refilled copy of the untouched board (original DRC, ruling C29)
    ripped_pcb: Path | None = None
    routed_pcb: Path | None = None
    via_fallback: dict | None = None  # apply_via_fallback info when VIA_NOT_SET_UP patched the engine rules
    #: the time limit of the routing: {"clock": wall|cpu, "start_s", "final_s" (0 = none), "changes": [{"at_s",
    #: "limit_s"}]}; changes are the user's, made in the progress dialog while the router ran (time_limit_text)
    time_limit: dict | None = None
    #: M1 compass (toporlib.compass.build): per violation class predicted vs drawn vs KiCad DRC, per connection
    compass: dict | None = None


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


def round_log_lines(p: Progress, seen: int) -> list[str]:
    """log.txt lines of the rounds the engine ended since round number `seen` (``Progress.round_lines`` keeps the
    last few: one line per round, its score in the pick order and why it is or is not the best); a gap the
    engine's list no longer covers is said once. O(len(round_lines))."""
    lines = list(p.round_lines)
    first = p.rounds_ended - len(lines) + 1  # round number of lines[0]
    out = [f"round log: {first - seen - 1} round lines missed"] if first > seen + 1 else []
    out += [f"round ended: {ln}" for i, ln in enumerate(lines) if first + i > seen]
    return out


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


def _routed_fills(cfg, out, state: dict, log) -> dict:
    """``run.commit_fills``: the zone fills of the kicad-cli refilled routed copy (the DRC round that judged exactly
    the items being committed), keyed by zone uuid, for the Accept commit. Empty when off, when the judged copy
    is missing or when it was judged for other items. O(len(board text))."""
    if not cfg.get("run", "commit_fills") or out.routed_pcb is None or not Path(out.routed_pcb).is_file():
        return {}
    judged = state.get("judged_items")
    if judged is not None and [{k: v for k, v in it.items() if k != "group"} for it in judged] != \
            [{k: v for k, v in it.items() if k != "group"} for it in out.items]:
        log.line("zone fills not committed: the refilled copy was judged for other items")
        return {}
    fills = {u: p for u, p in pcbfile.parse_zone_fills(Path(out.routed_pcb).read_text()).items() if not u.startswith("#")}
    log.line(f"zone fills of the refilled routed copy committed with the copper: {len(fills)} zones")
    return fills


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


def build_options(cfg: Config, engine, *, plan=None, rip=None, model=None, priorities=None) -> dict:
    """Engine options: ``cfg.engine_options()`` restricted to keys the engine knows, plus the plugin's own
    additions: ``scope`` (``plan.engine_scope``, scope.py), ``existing_copper`` (``rip.engine_existing_copper``:
    the plugin does the ripping, so only ``smooth`` is passed on), and, when the engine knows the key,
    ``net_priority`` (net name -> 1..3; the numbers are the engine tiers 1:1: 1 is routed first and pays the
    most per via, 3 is the normal tier, the plugin never produces 4), ``exclude_nets`` (``do_not_touch``
    expanded against `model`), ``ignore_pads`` (``REF.PAD`` strings exactly as stored in topor.toml, the
    engine's ``PadRef`` format) and ``pour_served`` (net name -> bool; non-boolean entries are dropped).
    O(#nets) for the expansion."""
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
                     ("net_priority", dict(priorities or {})),
                     ("pour_served", {str(k): v for k, v in dict(cfg.get("nets", "pour_served") or {}).items()
                                      if isinstance(v, bool)})):
        if key in known and val:
            opts[key] = val
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


def resolve_rules(port: BoardPort, pcb_copy: Path, copy_text: str, cfg: Config | None = None) -> dict:
    """Rules from the copy's .kicad_pro + .kicad_dru and the port's net classes (toporlib.rules), under
    ``[rules] fab_profile`` of `cfg` (None: the default profile). The ``per_net``
    keys use the port's ONE net-code table (``port.net_codes()``, the same the model is exported with; gate
    review MINOR 13); a port without one (FakeBoard: its model carries codes and rules) lets the resolver derive
    them from the net classes. O(N * R)."""
    pro_p, dru_p = pcb_copy.with_suffix(".kicad_pro"), pcb_copy.with_suffix(".kicad_dru")
    pro = json.loads(pro_p.read_text()) if pro_p.is_file() else {}
    dru = dru_p.read_text() if dru_p.is_file() else None
    classes, of = port.netclasses()
    codes = port.net_codes() if hasattr(port, "net_codes") else None
    cu = pcbfile.parse_layers(copy_text)
    n_cu = max(2, len(cu))
    return rules_mod.resolve_rules_from_project(pro, dru, netclass_of=of, classes=classes,
                                                widths_before=widths_before(copy_text), copper_layer_count=n_cu,
                                                net_codes=codes, layer_names={l["id"]: l["name"] for l in cu},
                                                fab_profile=fab_profile_of(cfg))


def fab_profile_of(cfg: Config | None) -> str:
    """``[rules] fab_profile`` of `cfg` (the schema default without one). O(1)."""
    return str(cfg.get("rules", "fab_profile")) if cfg is not None else rules_mod.DEFAULT_FAB_PROFILE


def attach_diff_pairs(model: dict, rules: dict, pcb_copy: Path, pro: dict, cfg: Config, netclass_of: dict) -> tuple:
    """D7: the board's differential pairs (``toporlib.diffpair``: KiCad's ``MatchDpSuffix`` plus ``[diffpair] pairs``)
    with their width, gap, via gap and rule limits, put into ``model["rules"]["diff_pairs"]`` for the engine.
    Returns (model, report lines). O(N x L + P x R)."""
    from toporlib import diffpair  # noqa: PLC0415

    dru_p = pcb_copy.with_suffix(".kicad_dru")
    dru = dru_p.read_text() if dru_p.is_file() else None
    codes = {n["name"]: int(n["code"]) for n in model.get("nets", []) if n.get("name") and int(n.get("code") or 0)}
    settings = {k: cfg.get("diffpair", k) for k in ("detect", "width_source", "gap_mm", "via_gap_mm",
                                                       "gap_tolerance_mm", "max_uncoupled_mm", "skew_max_mm", "pairs")}
    mr = model.get("rules") or rules
    pairs, notes = diffpair.resolve_pairs(codes, pro=pro, dru_text=dru, netclass_of=netclass_of, rules=mr,
                                          settings=settings)
    if not pairs and not mr.get("diff_pairs"):
        return model, notes  # no pair: the model stays as read
    return {**model, "rules": {**mr, "diff_pairs": pairs}}, notes


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


def _ripup_notes(rr: dict) -> list:
    """Run-report note for the rip-up escalation (engine ``ripup`` report; none when nothing was ripped)."""
    rp = rr.get("ripup") or {}
    if not rp.get("total"):
        return []
    per = rp.get("rips_per_connection") or []
    pairs = rp.get("pairs_at_backstop") or []
    line = (f"rip-up: {rp['total']} rips, {sum(1 for n in per if n)} connections ripped, "
            f"max {rp.get('max_pair', 0)} per pair")
    if pairs:
        line += f"; backstop (max_per_pair) reached by {len(pairs)} pair(s): " + ", ".join(f"{a}->{b}" for a, b in pairs[:10])
    return [line]


def _via_relax_notes(rr: dict) -> list:
    """Run-report note for the via-relaxation pass (engine ``via_relaxation``; none when off / not run)."""
    v = rr.get("via_relaxation")
    return [f"via relaxation: {v}"] if v and v not in ("off", "not run") else []


def _facts_result(status: str, out: RunOutcome, report: dict | None) -> dict:
    slim = {k: v for k, v in (report or {}).items() if k != "items"}
    return {"status": status, "routed": out.routed, "total": out.total, "stop_reason": out.stop_reason,
            "cancelled": out.cancelled, "rounds": out.rounds, "router_s": round(out.router_s, 3), "router_cpu_s": round(out.router_cpu_s, 3),
            "wall_s": round(out.wall_s, 3), "items": len(out.items), "ripped": len(out.ripped), "drc": out.drc,
            "findings": [asdict(f) for f in out.findings], "route": slim, "via_fallback": out.via_fallback,
            "time_limit": out.time_limit, "compass": out.compass,
            "pcb_copy": str(out.pcb_copy) if out.pcb_copy else None}


# ---------------------------------------------------------------- the flow
def run(port: BoardPort, ui: UiPort, cfg: Config | None = None, engine=None, *, tmp_root: Path,
        now: datetime | None = None, config_findings=(), versions: dict | None = None) -> RunOutcome:
    """Run Topor on the board behind `port`; see the module doc. `cfg` None loads ``topor.toml`` from the
    project directory (first run: ``ui.first_run_notice``); `engine` defaults to ``topor_engine``.

    Returns a :class:`RunOutcome`; exceptions (not BaseException: the PyO3 panic is caught by ``app.main``) are
    shown with ``ui.error`` and give ``status == "error"`` (INTERNAL), except after a successful commit: then the status stays ``accepted`` with the POST_COMMIT_STEP_FAILED
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

    lock = None
    if cfg.get("run", "single_instance"):
        try:
            lock = runlock.acquire(project_dir, board_name, cache_root(), report_dir=report_dir)
            if lock is None:
                log.line("run lock skipped: fcntl is not available on this platform")
            else:
                log.line(f"run lock taken: {lock.path}")
        except runlock.RunLockBusy as busy:
            return _abort_locked(busy, ui, out, report, log, ctx, t0)
        except runlock.RunLockUnavailable as e:  # never fatal: the run goes ahead unprotected, with a warning
            w = Finding("RUN_LOCK_UNAVAILABLE", "warning",
                        f"The run lock {e.path} could not be taken ({type(e.cause).__name__}: {e.cause}); "
                        "this run is not protected against a second start for the same project.",
                        messages.fix_for("RUN_LOCK_UNAVAILABLE"), (str(e.path),))
            findings.append(w)
            log.line("run lock unavailable: " + w.message)
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
        if isinstance(e, KicadCliTimeout):  # MINOR 10: kicad-cli killed after run.kicad_cli_timeout_s
            out.error = Finding("KICAD_CLI_TIMEOUT", "error", str(e), messages.fix_for("KICAD_CLI_TIMEOUT"))
        elif isinstance(e, BoardChanged):  # MINOR 11: nothing was committed
            out.error = Finding("BOARD_CHANGED_DURING_RUN", "error", str(e), messages.fix_for("BOARD_CHANGED_DURING_RUN"))
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
    finally:
        if lock is not None:
            lock.release()


def _abort_locked(busy, ui, out: "RunOutcome", report, log, ctx, t0) -> "RunOutcome":
    """Second start for the same project: no board access, outcome "aborted at pre-flight" in log and report,
    the RUN_ALREADY_RUNNING message through ``ui.error``."""
    f = Finding("RUN_ALREADY_RUNNING", "error",
                f"Topor is already running for this project ({runlock.holder_text(busy.holder)}). "
                "Finish or cancel that run first.",
                messages.fix_for("RUN_ALREADY_RUNNING"), (str(busy.path),))
    out.findings.append(f)
    out.error = f
    log.line("aborted at pre-flight: " + f.message)
    if busy.holder:
        log.line(f"lock holder: {busy.holder}")
    report.facts["findings"] = out.findings
    out.status = "aborted_preflight"
    out.wall_s = time.monotonic() - t0
    report.set_outcome("aborted_preflight")
    try:
        report.write(result=_facts_result("aborted_preflight", out, None))
    except Exception:  # noqa: BLE001
        log.exc()
    ctx()
    ui.error(f)
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
             "rounds": 0, "router_s": 0.0, "router_cpu_s": 0.0,  # router time / CPU time of the engine jobs (progress dialog)
             "limit_hit": None}  # "cpu": the CPU limit fired and the plugin cancelled the job

    def emit(phase: str) -> None:
        q = replace(state["last"], phase=phase, elapsed_s=state["router_s"], cpu_s=state["router_cpu_s"],
                    wall_s=time.monotonic() - t0, limit_s=state.get("limit"))
        if ui.progress(q) is False:
            state["cancel"] = True
        if state["last_result"] is None:  # a change before the engine job starts applies to it
            take_limit()

    def take_limit() -> bool:
        """A time limit the user set in the progress dialog (``ui.time_limit_request``, interactive runs only:
        headless UIs have no such method, so their limit stays fixed): applied to ``state["limit"]``, logged and
        recorded in ``out.time_limit``. <= 0 = no limit; a limit below the router time spent becomes the time spent
        (the run stops at its next check and keeps the best result). True when the limit changed. O(1)."""
        req = getattr(ui, "time_limit_request", None)
        if "limit" not in state or not callable(req):  # before the routing's limit exists: left for later
            return False
        new = req()
        if new is None:
            return False
        clock = out.time_limit["clock"]
        spent = state["router_cpu_s"] if clock == "cpu" else state["router_s"]
        new = float(new)
        new = 0.0 if not math.isfinite(new) or new <= 0 else max(new, spent)
        old = state["limit"]
        if new == old:
            return False
        state["limit"] = new
        out.time_limit["changes"].append({"at_s": round(spent, 3), "limit_s": round(new, 3)})
        out.time_limit["final_s"] = new
        how = ("removed by the user (no limit)" if new <= 0 else "set by the user" if old <= 0
               else "extended by the user" if new > old else "lowered by the user")
        log.line(f"time limit {how} at {spent:.1f} s router time ({clock} clock): {limit_text(old)} -> "
                 f"{limit_text(new)}" + (" = the time spent: the run stops at its next check" if 0 < new <= spent else ""))
        return True

    def finish(status: str, *, error_code: str = "") -> RunOutcome:
        out.status = status
        out.wall_s = time.monotonic() - t0
        report.set_outcome(status, error_code)
        return out

    def cli_kw(phase: str, *, cancellable: bool) -> dict:
        """kicad-cli options (MINOR 10): the run.kicad_cli_timeout_s limit and a poll callback that pumps the UI
        every 0.25 s via ``emit(phase)``. `cancellable`: Cancel kills the process (pre-flight phase); after the
        routing the Cancel only stops further rounds, the best result is still judged and previewed."""
        limit = float(cfg.get("run", "kicad_cli_timeout_s") or 0)

        def poll():
            emit(phase)
            return not (cancellable and state["cancel"])

        return {"timeout_s": limit or None, "on_poll": poll}

    def fingerprint(when: str):
        """BoardPort.fingerprint, never fatal: a failing fingerprint (UNVERIFIED over IPC) only disables the check."""
        try:
            fp = port.fingerprint()
            log.line(f"board fingerprint {when}: {fp}")
            return fp
        except Exception as e:  # noqa: BLE001
            log.exc()
            log.line(f"board fingerprint {when} failed ({type(e).__name__}: {e}); the changed-board check is off")
            return None

    def abort_preflight(reason: str) -> RunOutcome:
        log.line("aborted at pre-flight: " + reason)
        report.write(result=_facts_result("aborted_preflight", out, None))
        report.set_outcome("aborted_preflight")
        report.write(result=_facts_result("aborted_preflight", out, None))
        return finish("aborted_preflight")

    emit("setup")  # a window / progress before the first slow call (save_as, IPC model export)
    tmp_root.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="topor-", dir=tmp_root))
    out.work_dir = work
    start_fp = fingerprint("at start")  # MINOR 11: compared again right before the commit
    # 1. the live board, as KiCad holds it right now (unsaved edits included)
    pcb_copy = port.save_copy(work / "current")
    out.pcb_copy = pcb_copy
    copy_text = pcb_copy.read_text()
    log.line(f"save_as copy: {pcb_copy} ({len(copy_text)} bytes)")
    rules = resolve_rules(port, pcb_copy, copy_text, cfg)
    report.facts["floors_applied"] = list(rules.get("floors_applied") or [])  # report.md Settings (F I-4)
    report.facts["fab_profile"] = rules.get("profile")
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
    model, dp_notes = attach_diff_pairs(model, rules, pcb_copy, pro, cfg, port.netclasses()[1])
    for line in dp_notes:
        log.line(line)
    report.facts["diff_pairs"] = dp_notes
    out.model = model
    log.line(f"model: {len(model.get('pads', []))} pads, {len(model.get('kept', []))} tracks, "
             f"{len(model.get('vias', []))} vias, {len(model.get('nets', []))} nets")
    if out.via_fallback:
        log.line(via_fallback_text(out.via_fallback))
    # fail fast (robust-cdt): a board the engine cannot read stops here, before the pre-flight DRC and the dialog
    pr = probe_mod.probe_board(engine, model, build_options(cfg, engine, model=model), on_poll=lambda: emit("setup"))
    if pr is not None:
        report.facts["probe"] = {k: pr.get(k) for k in ("ok", "path", "seconds", "error", "places", "notes")}
        log.line(f"engine probe: {'ok' if pr.get('ok') else 'FAILED'}, {float(pr.get('seconds') or 0):.1f} s"
                 + (f", triangulation {pr['path']}" if pr.get("path") else ""))
        for n in pr.get("notes") or []:
            log.line(f"engine probe: {n}")
        if not pr.get("ok"):
            err = probe_mod.unreadable_finding(pr)
            log.line(f"aborted: the engine could not read this board: {pr.get('error')}")
            out.error = err
            findings.append(err)
            report.facts["findings"] = findings
            report.set_outcome("error", err.code)
            out.wall_s = time.monotonic() - t0
            report.write(result=_facts_result("error", out, None))
            ctx()
            ui.error(err)
            return finish("error", error_code=err.code)

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
        try:
            orig_drc = run_drc(orig_pcb, work / "drc-orig.json", save_board=True,
                               **cli_kw("preflight_drc", cancellable=True))
        except KicadCliCancelled:
            return abort_preflight("cancelled during the pre-flight DRC")
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
        return abort_preflight(", ".join(f.code for f in errors) or "declined")
    toml = project_dir / CONFIG_NAME
    cfg = apply_accepted_findings(decision, findings, cfg, toml)
    over = {k: v for k, v in decision.overrides.items() if k in _ROUTE_OVERRIDES}
    if over:
        where = {k: _OVERRIDE_KEY.get(k, ("run", k)) for k in over}
        cfg = cfg.with_values({where[k]: v for k, v in over.items()})
        if decision.save_to_toml and toml.is_file():
            update_config(toml, {f"{where[k][0]}.{where[k][1]}": v for k, v in over.items()})
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
        try:  # saved = zones refilled
            base_drc = run_drc(ripped_pcb, work / "drc-ripped.json", save_board=True,
                               **cli_kw("preflight_drc", cancellable=True))
        except KicadCliCancelled:
            return abort_preflight("cancelled during the baseline DRC")
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
    if getattr(ui, "waits_for_ok", False) and "stop_when_stable" in json.loads(engine.default_options()):
        # M-B0: with the progress dialog the user ends the run (OK once the routes are stable); headless runs
        # (harness, MCP) stop at "Routes stable" + [optimize] stable_patience by themselves.
        opts["stop_when_stable"] = False
    budget = float(opts.get("wall_clock_s", 60.0))
    # run.timeout_clock = "cpu": timeout_s counts CPU seconds of the engine jobs (time.process_time deltas, all
    # threads summed); the plugin cancels the job when it is reached and the engine's own wall limit is off.
    # The limit is live (state["limit"], 0 = none): the progress dialog may change it while the router runs
    # (take_limit); on the wall clock the engine job gets the change (RouteJob.set_time_limit), on the CPU clock the
    # poll loop below compares against it.
    cpu_clock = cfg.get("run", "timeout_clock") == "cpu"
    soft_limit = str(opts.get("timeout_mode", "soft")) != "hard"  # M2.TL
    no_budget = budget <= 0 or cpu_clock
    state["limit"] = max(0.0, budget)
    out.time_limit = {"clock": "cpu" if cpu_clock else "wall", "start_s": state["limit"], "final_s": state["limit"],
                      "changes": []}
    geometry = cfg.get("output", "geometry")
    max_err = int(rules.get("max_error") or 5000)
    layer_names = {int(l["id"]): l["name"] for l in model.get("layers", [])}
    net_names = {int(n["code"]): n["name"] for n in model.get("nets", [])}
    routed_dir = work / "routed"

    def engine_limit(router_before: float) -> float:
        """The engine job's wall limit: seconds since the job started (its clock starts with the job), 0 = none
        (also on the CPU clock, which the plugin enforces). A limit already spent gives the job a moment: it stops
        at its first check. O(1)."""
        lim = state["limit"]
        return 0.0 if cpu_clock or lim <= 0 else max(1e-3, lim - router_before)

    def note_limit(res: dict) -> None:
        """M2.TL: what the time limit did (out.time_limit: mode, reached, the step that ran then, overrun_s past it on
        the run's clock), with a log line. O(1)."""
        lim, eng = state["limit"], res.get("limit") or {}
        spent = state["router_cpu_s"] if cpu_clock else state["router_s"]
        reached = bool(lim > 0 and (state["limit_hit"] or spent >= lim or eng.get("reached")))
        out.time_limit.update({"mode": "soft" if soft_limit else "hard", "reached": reached,
                               "step": str(eng.get("step") or ""),
                               "overrun_s": round(max(0.0, spent - lim), 3) if reached else 0.0})
        if reached:
            log.line(f"time limit ({out.time_limit['mode']}): reached during {out.time_limit['step'] or 'the run'}; "
                     f"the result was ready {out.time_limit['overrun_s']:.1f} s past the limit "
                     f"({'CPU' if cpu_clock else 'wall'} clock)")

    def route_fn(m, o):
        if (state["cancel"] or state["limit_hit"]) and state["last_result"] is not None:
            return state["last_result"]  # cancelled: no further rounds, the best result stays
        o = {**o, "wall_clock_s": engine_limit(state["router_s"])}
        mj, oj = json.dumps(m), json.dumps(o)
        dump = os.environ.get("TOPOR_DUMP_ENGINE_INPUT")  # debugging (docs/dev/debug.md): the engine's exact input
        if dump:
            d = Path(dump)
            d.mkdir(parents=True, exist_ok=True)
            n = len(list(d.glob("model-*.json"))) + 1
            (d / f"model-{n}.json").write_text(mj)
            (d / f"options-{n}.json").write_text(oj)
            log.line(f"engine input written to {d}/model-{n}.json, options-{n}.json (TOPOR_DUMP_ENGINE_INPUT)")
        job = engine.route_async(mj, oj)
        job_t0, router_before = time.monotonic(), state["router_s"]
        cpu_t0, cpu_before = time.process_time(), state["router_cpu_s"]
        last_poll, cancelled_at = -1e9, None
        rounds_seen = 0  # round lines of this job written to log.txt (round_log_lines)
        # Ends when the job is done (also after an engine panic: B-1). After Cancel / OK / the CPU limit the engine
        # finishes the stage it is in and returns its best result so far; that can take a while on a big board, so
        # the flow waits for it (the progress dialog says so) instead of giving up and losing the result.
        while True:
            done = job.done()
            now = time.monotonic()
            if done or now - last_poll >= POLL_S:
                last_poll = now
                p = Progress.from_json(json.loads(job.progress()))
                # honest times (K1): router time over all jobs (what run.timeout_s limits) and the wall time of
                # the whole run; the engine's own elapsed_s restarts with every whole-route round
                p.elapsed_s = router_before + (now - job_t0)
                p.wall_s = now - t0
                state["router_s"] = p.elapsed_s
                p.cpu_s = state["router_cpu_s"] = cpu_before + (time.process_time() - cpu_t0)
                state["last"] = p
                for line in round_log_lines(p, rounds_seen):
                    log.line(line)
                rounds_seen = max(rounds_seen, p.rounds_ended)
                # the engine's measurements outside the limit (M1 compass cut test, one thread) do not count
                if (cpu_clock and 0 < state["limit"] <= p.cpu_s - p.measure_s and not state["limit_hit"] and not done):
                    state["limit_hit"] = "cpu"
                    log.line(f"CPU-time limit reached: {p.cpu_s:.1f} s of {state['limit']:.0f} s (router wall {p.elapsed_s:.1f} s)")
                p.limit_s = state["limit"]
                if ui.progress(p) is False:
                    state["cancel"] = True
                if take_limit() and not done and not cpu_clock:
                    setter = getattr(job, "set_time_limit", None)  # the engine reads its limit live
                    if callable(setter):
                        setter(engine_limit(router_before))
                    else:
                        log.line("this engine cannot change its time limit during a run; the change is ignored")
                if state["limit_hit"] and cancelled_at is None:
                    cancelled_at = now
                    reach = getattr(job, "reach_limit", None)
                    if soft_limit and callable(reach):
                        # M2.TL: the CPU clock's limit is the engine's soft limit: the step that runs finishes and the
                        # best kept variant is drawn (Cancel still stops now)
                        reach()
                    else:
                        job.cancel()
                        log.line("time limit: waiting for the engine to finish its current stage")
                if p.limit_reached and not state.get("limit_noted"):
                    state["limit_noted"] = True
                    log.line(p.notice or "time limit reached, waiting for the current step to finish")
                if state["cancel"] and cancelled_at is None:
                    cancelled_at = now
                    job.cancel()
                    log.line("Cancel / OK: waiting for the engine to finish its current stage")
                    if p.stable:  # M-B0: OK after "Routes stable": the best so far is the board the user gets
                        state["ok_stable"] = True
                        log.line(f"OK at Routes stable: best so far {p.best_routed}/{p.best_total} routed, "
                                 f"{p.best_length_mm / 1000:.3f} m, {p.best_vias} vias (run {p.best_run} of {p.runs})")
            if done:
                break
            time.sleep(0.02)
        if cancelled_at is not None:
            log.line(f"engine stopped {time.monotonic() - cancelled_at:.1f} s after the stop request")
        res = json.loads(job.wait())  # a panicked job raises RuntimeError("routing thread panicked: ...")
        note_limit(res)
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
        rep = run_drc(pcb, routed_dir / "drc-routed.json", save_board=True,  # refilled zones for the preview
                      **cli_kw("drc", cancellable=False))
        log.line(f"DRC round {state['rounds']}: {len(rep.violations)} violations, {len(rep.unconnected)} unconnected, "
                 f"{rep.seconds:.1f}s")
        j = DrcJudgement(rep, {w["uuid"]: orig for w, orig in zip(with_ids, items)})
        state["judged_key"], state["judged"], state["judged_items"] = key, j, list(items)
        return j

    clearance = float(max(rules.get("min_clearance", 0), (rules.get("default") or {}).get("clearance", 0)))
    loop = drcloop.route_with_drc_loop(
        eng_model, opts, route_fn=route_fn, drc_fn=drc_fn, base_drc=base_drc, orig_drc=orig_drc,
        # The topological core takes no DRC feedback (no pins, no DRC spots): a re-route would repeat the same
        # result, and since M-B1 (never drop) the violations it reports are written on purpose. One DRC round.
        budget_s=NO_BUDGET_S if no_budget else budget, loop_cap=0,
        reach_nm=_zone_reach(model), clearance_nm=clearance)
    rr = loop.router_report or {}
    out.items = [{**it, "group": it.get("group") or "topor"} for it in loop.items]
    out.routed, out.total = int(rr.get("routed", 0)), int(rr.get("connections_total", 0))
    out.stop_reason = rr.get("stop_reason")
    out.router_cpu_s = state["router_cpu_s"]
    if state["limit_hit"] == "cpu" and out.stop_reason != "stable":
        # the plugin's own cancel: the engine says "cancel", the clock that fired is "cpu"
        out.stop_reason = "timeout_cpu"
        changed = (f"; changed during the run, run.timeout_s = {limit_text(out.time_limit['start_s'])}"
                   if out.time_limit["changes"] else "")
        rr = {**rr, "stop_reason": "timeout_cpu", "timed_out": True, "warnings": list(rr.get("warnings") or []) + [
            f"time budget hit: CPU-time limit of {state['limit']:.0f} s reached ({out.router_cpu_s:.0f} CPU s of router "
            f"work, {state['router_s']:.0f} s on the clock, run.timeout_clock = cpu{changed}): result may differ between runs"]}
    elif out.stop_reason == "timeout":
        out.stop_reason = "timeout_wall"
        rr = {**rr, "stop_reason": "timeout_wall"}
    # M-B0: OK after "Routes stable" is not a cancel (stop_reason "stable"): the user took the stable result.
    out.cancelled = bool((state["cancel"] and not state.get("ok_stable")) or out.stop_reason == "cancel")
    out.rounds, out.router_s = loop.rounds, loop.router_s
    # K1: the engine's report lines that need the user's attention become findings (postrun.py); a post-run
    # finding replaces a pre-flight one of the same code (e.g. DIFF_PAIRS_SKIPPED: "will not" -> "were not").
    post = postrun.route_findings(rr)
    mismatch = None
    if loop.routed_drc is not None:  # KiCad's own view of the pours after the refill (K1)
        post += postrun.drc_pour_findings(loop.routed_drc.unconnected)
        # KiCad's new violations: a warning with counts and places (never blocks Accept), and the self-check that
        # the clearances KiCad required are the ones the engine was given (RULES_MISMATCH, a loud error below)
        new_v = new_routing(loop.routed_drc, base_drc, loop.created)
        if new_v:
            try:
                tracks = pcbfile.track_lines(Path(out.routed_pcb).read_text()) if out.routed_pcb else None
            except OSError:
                tracks = None
            post += postrun.drc_new_findings(new_v, tracks)
            mismatch = postrun.rules_mismatch(new_v, eng_model)
    post_codes = {f.code for f in post}
    findings[:] = [f for f in findings if f.code not in post_codes] + post
    for f in post:
        log.line(f"after routing: {f.code} ({f.severity}): {f.message}")
    classified = classify(loop.routed_drc, base_drc, loop.created, unconnected_baseline=orig_drc)
    out.drc = classified
    try:  # M1 compass: a measurement, never a failed run
        out.compass = compass_mod.build(
            rr, new_routing(loop.routed_drc, base_drc, loop.created) if loop.routed_drc is not None else None,
            classified["incomplete"].get("unconnected_new") if loop.routed_drc is not None else None,
            drc_radius_mm=float(cfg.get("output", "compass_join_mm")), join_nets=bool(cfg.get("output", "compass_join_nets")))
    except Exception as e:  # noqa: BLE001
        out.compass = {"available": False, "error": f"{type(e).__name__}: {e}"}
    inc = classified["incomplete"]
    partial = int(rr.get("partial", 0))
    scope_info = {"mode": cfg.get("run", "scope_mode") if (sel.footprints or sel.pads) else "whole board",
                  "ignored_pads": list(cfg.get("nets", "ignore_pads")),
                  "accepted_codes": list(decision.accepted_codes),
                  "fallbacks": [via_fallback_text(out.via_fallback)] if out.via_fallback else []}
    build_report({**rr, "items": out.items}, classified, cfg, findings, scope_info, before_stats,
                 {"router_s": out.router_s, "router_cpu_s": out.router_cpu_s, "drc_rounds": loop.rounds}, report=report,
                 facts={"layers": report.facts.get("layers"), "versions": report.facts.get("versions"),
                        "time_limit": out.time_limit, "compass": out.compass,
                        "notes": [f"geometry: {geometry}", f"DRC rounds: {loop.rounds}"] + _ripup_notes(rr) + _via_relax_notes(rr) + (
                            ["time budget exhausted"] if loop.budget_exhausted else [])})
    log.line(f"routed {out.routed}/{out.total}, stop {out.stop_reason}, router {out.router_s:.1f} s, CPU {out.router_cpu_s:.1f} s, rounds {loop.rounds}, "
             f"routing_new {classified['routing_new']}, unconnected_new {inc['unconnected_new']}, {time_limit_text(out.time_limit)}")
    drc_src = routed_dir / "drc-routed.json"
    if mismatch is not None and out.items:
        # fail fast: the result was routed to rules KiCad does not agree with; nothing is applied, the error dialog
        # names the nets and both clearances (the report keeps the result and KiCad's DRC)
        log.line(f"error: {mismatch.message}")
        out.error = mismatch
        findings.append(mismatch)
        report.facts["findings"] = findings
        report.set_outcome("error", mismatch.code)
        out.wall_s = time.monotonic() - t0
        report.write(result=_facts_result("error", out, rr), drc_src=drc_src)
        ctx()
        ui.error(mismatch)
        return finish("error", error_code=mismatch.code)
    engine_err = probe_mod.engine_failure(rr)  # robust-cdt: an engine error is loud, never a quiet empty run
    nothing = next((str(w) for w in rr.get("warnings") or [] if str(w).startswith("ERROR: nothing to draw yet")), None)
    if nothing is not None and not out.items:
        # M2.TL (fail fast): Cancel before any plan was kept is a loud error, never a quiet cancelled run
        engine_err = Finding("NOTHING_TO_DRAW", messages.MESSAGES["NOTHING_TO_DRAW"].severity, nothing.removeprefix("ERROR: "),
                             messages.MESSAGES["NOTHING_TO_DRAW"].fix)
        out.cancelled = False
    if engine_err is not None and not out.items and not out.cancelled:
        log.line(f"error: {engine_err.message}")
        out.error = engine_err
        findings.append(engine_err)
        report.facts["findings"] = findings
        report.set_outcome("error", engine_err.code)
        report.write(result=_facts_result("error", out, rr), drc_src=drc_src)
        ctx()
        ui.error(engine_err)
        return finish("error", error_code=engine_err.code)
    if engine_err is not None:  # most connections failed on it, some copper exists: listed as an error finding
        findings.append(engine_err)
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
            layer_names=[l["name"] for l in model.get("layers", []) if l.get("routable", True)] or None,
            unconnected_before=(orig_drc.unconnected if orig_drc is not None else None),
            unconnected_after=(loop.routed_drc.unconnected if loop.routed_drc is not None else None),
            **cli_kw("preview", cancellable=False))
        report.facts["notes"] = report.facts["notes"] + notes
    except KicadCliTimeout as e:  # the numbers still allow a decision; the timeout is named in the findings
        log.exc()
        log.line(f"preview timed out: {e}")
        findings.append(Finding("KICAD_CLI_TIMEOUT", "warning", f"The preview could not be rendered: {e}",
                                messages.fix_for("KICAD_CLI_TIMEOUT")))
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

    # 6. MINOR 11: the board must be exactly what it was when the run started (KiCad stays editable meanwhile)
    if start_fp is not None:
        now_fp = fingerprint("before the commit")
        if now_fp is not None and now_fp != start_fp:
            log.line(f"board changed during the run: {start_fp} -> {now_fp}; nothing committed")
            raise BoardChanged("The board was edited while Topor ran (items were added, deleted or moved), "
                               "so nothing was committed.")

    # 7. Accept: one commit = one undo step. Everything after it is a separate step (ruling C28: refill only with
    # run.refill_after_accept; cleanup / teardrops opt-in) and a failure there keeps "accepted" (ruling C30).
    n_partial = sum(1 for it in out.items if it.get("group") == "topor-partial")
    req = CommitRequest(remove_uuids=list(ripped), items=out.items,
                        message=f"Topor: routed {out.routed}/{out.total}", group=True,
                        zone_fills=_routed_fills(cfg, out, state, log))
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
    report.facts["zone_fills_committed"] = len(req.zone_fills)
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
