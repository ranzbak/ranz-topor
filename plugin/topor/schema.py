"""Settings schema of ``topor.toml`` (46 §2): the single source for the file comments, the loader,
the generated reference (docs/user/configuration.md) and the engine-option mapping.

Complexity: module-level constants; iteration O(#settings); SETTINGS_BY_KEY lookup O(1).
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

CONFIG_VERSION = 1
CURRENT_VERSION = "0.4.0"  # version this schema ships with; a setting's `since` is compared to it


@dataclass(frozen=True)
class Setting:
    key: str
    section: str
    type: str  # "int" | "float" | "bool" | "str" | "list" | "table"
    default: Any
    unit: str = ""
    range: tuple | None = None  # numeric (lo, hi), either may be None (open)
    doc: str = ""
    kicad_place: str | None = None  # where KiCad itself influences this behaviour
    engine_field: str | None = None  # topor_engine Options field ("expert.x" = nested)
    harness_field: str | None = None
    plugin_field: str | None = None
    choices: tuple | None = None  # allowed strings
    elem: str | None = None  # list element type: "str" | "float"
    since: str | None = None  # engine feature lands in this version; commented out until then


def available(s: Setting) -> bool:
    if s.since is None:
        return True
    return tuple(int(x) for x in CURRENT_VERSION.split(".")) >= tuple(int(x) for x in s.since.split("."))


V5 = "0.5.0"
POWER_PATTERN = "^(\\+|-)?\\d|^(V|VCC|VDD|VBAT|GND|AGND|PWR)"


def _s(section, key, type_, default, doc, *, unit="", range=None, choices=None, elem=None, engine=None,
       kicad=None, since=None, harness=None):
    plugin = None if engine else f"{section}.{key}"
    return Setting(key=key, section=section, type=type_, default=default, unit=unit, range=range, doc=doc,
                   kicad_place=kicad, engine_field=engine, harness_field=harness, plugin_field=plugin,
                   choices=choices, elem=elem, since=since)


SECTIONS = {
    "run": "What a run does and how long it may take.",
    "nets": "Which nets are touched, how they are classified and prioritised.",
    "vias": "Via cost, count and size policy (sizes themselves come from KiCad).",
    "layers": "Stackup roles per copper layer (F.Cu = \"signal\", In1.Cu = \"gnd\", ...) and per-layer preferences.",
    "search": "Path search tuning.",
    "ripup": "Rip-up and re-route escalation.",
    "iterations": "Optimisation stop rule.",
    "output": "Result geometry and report location.",
    "expert": "PathFinder constants; change only when you know the router.",
}

SETTINGS: list[Setting] = [
    # ---- [run]
    _s("run", "timeout_s", "float", 60, "Wall-clock limit of the whole run including DRC rounds; 0 or \"none\" disables it (Cancel is then the only stop).",
       unit="s", range=(0, 86400), engine="wall_clock_s"),
    _s("run", "threads", "int", 0, "Worker threads; 0 = all cores but one.", range=(0, 256), engine="threads"),
    _s("run", "partial_routes", "bool", False, "Keep the connections that did route when others fail, instead of discarding the whole result.",
       engine="partial_routes"),
    _s("run", "existing_copper", "str", "fixed", "Policy for existing tracks and vias: fixed = obstacle, smooth = keep topology but smooth, rip = re-route.",
       choices=("fixed", "smooth", "rip"), engine="existing_copper"),
    _s("run", "respect_locked", "bool", True, "Treat locked tracks and vias as fixed obstacles; false lets topor rip or smooth them.",
       kicad="KiCad → item Properties → Locked"),
    _s("run", "keep_unconnected_vias", "bool", True, "Never rip a via that no track of its own net touches (stitching vias without KiCad's free flag); false rips them like routing vias.",
       kicad="KiCad → via Properties → Free"),
    _s("run", "scope_mode", "str", "between", "With a footprint selection: between = connections whose pads are all on selected footprints; touching = any connection with a selected pad.",
       choices=("between", "touching"), kicad="KiCad → select footprints in the PCB editor"),
    _s("run", "unrouted_only", "bool", False, "Route only connections that have no copper yet and leave all existing copper untouched."),
    _s("run", "fail_on_preexisting", "bool", False, "Abort when kept or locked copper already has DRC violations; false only lists them as pre-existing."),
    _s("run", "kicad_cleanup", "bool", False, "After Accept, open KiCad's own Cleanup Tracks and Vias dialog (you confirm it there)."),
    _s("run", "kicad_teardrops", "bool", False, "After Accept, open KiCad's teardrop dialog (you confirm it there)."),
    _s("run", "detour_reference", "bool", True, "Compare every routed connection with a DRC-feasible direct path and report detours in the run report."),
    # ---- [nets]
    _s("nets", "do_not_touch", "list", [], "Net names or glob patterns whose copper is a fixed obstacle and never routed, ripped or smoothed.",
       elem="str"),
    _s("nets", "power_pattern", "str", POWER_PATTERN, "Regex naming power nets, applied only to nets still in the Default netclass; the pre-flight lists every net classified this way.",
       kicad="KiCad → File → Board Setup → Design Rules → Net Classes (a netclass wins over this pattern)"),
    _s("nets", "ground_pattern", "str", "^/?(GND|AGND|DGND|PGND|SGND|VSS)", "Regex (case-insensitive) naming ground nets, used to check that a GND plane layer has a ground zone; power nets use power_pattern."),
    _s("nets", "priority", "table", {}, "Net name or glob → priority 1..3 (1 routed first). Precedence: the @N name marker wins over a topor_pN netclass, which wins over this table; an exact name here wins over a glob (lowest number among matching globs). In 0.4.0 priorities are computed and reported; the engine uses them from 0.5.0.",
       kicad="KiCad → File → Board Setup → Design Rules → Net Classes (netclass topor_p1..p3) or the @N marker in the net name"),
    _s("nets", "pour_served", "table", {}, "Net name → true/false to force or forbid serving a net by a copper pour; absent = automatic."),
    _s("nets", "ignore_pads", "list", [], "Pads (REF.PAD, e.g. \"U1.3\") skipped as terminals, typically pads outside the board; written by the pre-flight dialog.",
       elem="str"),
    # ---- [vias]
    _s("vias", "via_cost_mm", "float", 2.0, "Search cost of one via in mm of extra trace: the router accepts up to this much longer to avoid a via.",
       unit="mm", range=(0, 1000), engine="via_cost_nm"),
    _s("vias", "max_per_connection", "int", 4, "Maximum vias on one connection.", range=(0, 32), engine="max_vias_per_connection"),
    _s("vias", "reduction", "bool", True, "After convergence remove vias whose via-free route is not much longer.", engine="via_reduction"),
    _s("vias", "reduction_len_slack_pct", "float", 10, "A via is removed when the via-free route is at most this much longer.",
       unit="%", range=(0, 1000), since=V5),
    _s("vias", "via_in_pad", "bool", False, "Allow vias inside SMD pads; pre-flight warns when KiCad's via protection is not filled and capped.",
       kicad="KiCad → File → Board Setup → Board Stackup → Board Finish → Via protection", since=V5),
    _s("vias", "allowed_drills_mm", "list", [], "Optional fab filter: only these drill diameters may be used; empty = no filter.",
       unit="mm", elem="float", since=V5),
    _s("vias", "fallback_drill_mm", "float", 0.4, "Drill used when KiCad has no via sizes set up.", unit="mm", range=(0.05, 10),
       kicad="KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias"),
    _s("vias", "fallback_diameter_mm", "float", 0.8, "Via diameter used when KiCad has no via sizes set up.", unit="mm", range=(0.1, 20),
       kicad="KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias"),
    _s("vias", "cost_by_priority", "table", {}, "Priority (\"1\"..\"3\") → via cost in mm, e.g. 5 for high-speed nets.", since=V5),
    _s("vias", "cost_by_netclass", "table", {}, "Netclass name → via cost in mm, e.g. 0.5 for power.", since=V5),
    _s("vias", "via_relaxation", "bool", True, "Let the relaxation pass move vias a bounded distance to shorten traces.", since=V5),
    # ---- [layers]
    _s("layers", "preferred_direction", "table", {}, "Layer name → \"h\", \"v\" or \"none\"; steers traces along a direction at cost direction_cost.", since=V5),
    _s("layers", "direction_cost", "float", 0.0, "Extra cost factor for running against a layer's preferred direction; 0 = off.", range=(0, 100), since=V5),
    _s("layers", "pour_cut_cost_mm", "float", 1.0, "Cost in mm of trace for cutting through a copper pour.", unit="mm", range=(0, 1000), since=V5),
    # ---- [search]
    _s("search", "detour_factor", "float", 2.0, "A connection longer than this multiple of its ghost-line length is treated as a detour and ripped.",
       range=(1.0, 100), engine="detour_factor"),
    _s("search", "margin_um", "float", 5, "Safety margin added to every clearance during search.", unit="µm", range=(0, 1000), engine="margin_nm"),
    _s("search", "zero_slack_retry", "bool", True, "One extra pass lets a connection use a corridor pinched below the margin but not below the rules.",
       engine="zero_slack_retry"),
    _s("search", "exit_stubs", "str", "auto", "Pad exit stubs for fine-pitch pads: auto, all or off.", choices=("auto", "all", "off"), since=V5),
    _s("search", "steiner", "bool", True, "Allow same-net T-junctions in the middle of a track instead of only at pads.", since=V5),
    _s("search", "dangling_ends", "str", "continue", "What to do with existing track ends that connect to nothing: continue them or drop them.",
       choices=("continue", "drop"), since=V5),
    _s("search", "strict_unfit_edges", "bool", False, "Fail a connection when a graph edge is too narrow for its trace instead of widening the search.", since=V5),
    # ---- [ripup]
    _s("ripup", "growth", "float", 2.0, "Each time a connection rips the same blocker, the history cost left on it is multiplied by this.",
       range=(1.0, 10), engine="ripup_growth"),
    _s("ripup", "max_per_pair", "int", 8, "Hard backstop on how often one connection may rip the same other connection (bounds the runtime).",
       range=(1, 1000), engine="ripup_max_per_pair"),
    # ---- [iterations]
    _s("iterations", "max", "int", 30, "Hard cap on optimisation cycles.", range=(1, 1000), engine="pathfinder_iter_cap"),
    _s("iterations", "patience", "int", 5, "Stop early when for this many cycles no connection was newly routed and gain stayed below min_gain_pct.",
       range=(1, 100), engine="patience"),
    _s("iterations", "min_gain_pct", "float", 1.0, "Smallest total cost improvement over the patience window that counts as progress.",
       unit="%", range=(0, 100), engine="min_gain_pct"),
    # ---- [output]
    _s("output", "geometry", "str", "arcs", "Track geometry: arcs = native KiCad arcs, segments = straight-segment approximation.",
       choices=("arcs", "segments")),
    _s("output", "report_dir", "str", "", "Report folder: \"\" = ${TOPOR_REPORT_ROOT:-${XDG_CACHE_HOME:-~/.cache}/kicad/10.0/topor}/<project>/<YYYYmmdd-HHMMSS>/; relative = <project dir>/<report_dir>/<timestamp>/; absolute = <report_dir>/<timestamp>/."),
    # ---- [expert]
    _s("expert", "hist_fail", "float", 1.0, "PathFinder history weight added to a face where a connection failed.", range=(0, 100), engine="expert.hist_fail"),
    _s("expert", "hist_block", "float", 0.5, "PathFinder history weight added to a face that blocked a connection.", range=(0, 100), engine="expert.hist_block"),
    _s("expert", "hist_overuse", "float", 0.5, "PathFinder history weight added per unit of face overuse.", range=(0, 100), engine="expert.hist_overuse"),
    _s("expert", "present_overuse", "float", 4.0, "PathFinder present-congestion penalty per unit of overuse.", range=(0, 100), engine="expert.present_overuse"),
    _s("expert", "zero_slack_candidates", "int", 4, "Candidate corridors tried per connection in the zero-slack retry.", range=(1, 64), engine="expert.zero_slack_candidates"),
    _s("expert", "relax_iter_cap", "int", 200, "Iteration cap of the relaxation (smoothing) pass.", range=(1, 10000), engine="expert.relax_iter_cap"),
    _s("expert", "drc_loop_cap", "int", 5, "Maximum route / DRC / re-route rounds.", range=(1, 50), engine="drc_loop_cap"),
]

SETTINGS_BY_KEY = {(s.section, s.key): s for s in SETTINGS}

# Values that are planned but not implemented: reported as CFG_VALUE_BACKLOG, the default is used.
BACKLOG_VALUES = {("output", "geometry"): ("45deg",)}

# Engine Options fields the plugin deliberately does not expose, with the reason.
ENGINE_INTERNAL = {
    "pinned": "set by the DRC loop from KiCad's own violations, not a user choice",
    "drc_spots": "set by the DRC loop from KiCad's own violations, not a user choice",
    "skip_self_check": "test-only switch that disables the engine's self check",
    "relax_iter_cap": "legacy alias of expert.relax_iter_cap (Options::relax_cap resolves)",
    "profile": "diagnostics only; timings must not enter the deterministic result",
    "seed": "fixed for byte-identical, reproducible results",
    "scope": "built per run from the KiCad selection and scope_mode, not stored",
    "net_priority": "built per run from @N markers, netclasses and [nets] priority",
    "via_cost_by_net": "built per run from [vias] cost_by_priority / cost_by_netclass",
    "detour_reference_budget_pct": "internal budget of the detour reference search; run.detour_reference switches it",
}

# KiCad-owned behaviours, written as comment-only entries: (name, place)
KICAD_ONLY = [
    ("Clearance", "KiCad → File → Board Setup → Design Rules → Constraints"),
    ("Track widths", "KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Track Widths and netclass track width"),
    ("Via sizes and pre-defined vias", "KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias and netclass via size/drill"),
    ("Minimum via, hole and annular ring", "KiCad → File → Board Setup → Design Rules → Constraints"),
    ("Copper-to-edge clearance", "KiCad → File → Board Setup → Design Rules → Constraints → Copper to Edge Clearance"),
    ("Netclasses and net assignments", "KiCad → File → Board Setup → Design Rules → Net Classes"),
    ("Custom rules", "KiCad → File → Board Setup → Design Rules → Custom Rules (.kicad_dru)"),
    ("Layer types (signal/power/mixed/jumper)", "KiCad → File → Board Setup → Board Editor Layers"),
    ("Locked tracks and vias", "KiCad → item Properties → Locked"),
    ("Via protection (fill/cap)", "KiCad → File → Board Setup → Board Stackup → Board Finish → Via protection"),
]
