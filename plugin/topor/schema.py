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
    "vias": "Via cost, count and size policy (each net routes with its KiCad net class via, plus smaller fab-profile vias where space requires; see operate.md, Fab minimums).",
    "layers": "Stackup roles per copper layer (F.Cu = \"signal\", In1.Cu = \"gnd\", ...) and per-layer preferences.",
    "search": "Path search tuning.",
    "ripup": "Rip-up and re-route escalation.",
    "iterations": "Optimisation stop rule.",
    "output": "Result geometry and report location.",
    "expert": "PathFinder constants; change only when you know the router.",
}

SETTINGS: list[Setting] = [
    # ---- [run]
    _s("run", "timeout_s", "float", 60, "Limit on router time: all routing passes of the run together stop after this long; the pre-flight DRC, the DRC rounds and the kicad-cli refill / preview exports are not counted. 0 or \"none\" disables it (Cancel is then the only stop).",
       unit="s", range=(0, 86400), engine="wall_clock_s"),
    _s("run", "threads", "int", 0, "Worker threads; 0 = all cores but one.", range=(0, 256), engine="threads"),
    _s("run", "partial_routes", "bool", False, "Keep the connections that did route when others fail, instead of discarding the whole result.",
       engine="partial_routes"),
    _s("run", "existing_copper", "str", "fixed", "Policy for existing tracks and vias: fixed = obstacle, smooth = keep topology but smooth, rip = re-route. (smooth is treated as fixed in 0.4.0.)",
       choices=("fixed", "smooth", "rip"), engine="existing_copper"),
    _s("run", "respect_locked", "bool", True, "Treat locked tracks and vias as fixed obstacles; false lets topor rip or smooth them.",
       kicad="KiCad → item Properties → Locked"),
    _s("run", "keep_unconnected_vias", "bool", True, "Never rip a via that no track of its own net touches (stitching vias without KiCad's free flag); false rips them like routing vias.",
       kicad="KiCad → via Properties → Free"),
    _s("run", "rip_free_vias", "bool", False, "With existing_copper = rip, also remove the free vias (KiCad's free flag) and the unconnected stitching vias of the nets in scope, so they no longer block routes; overrides keep_unconnected_vias. Locked vias and do_not_touch nets are still kept.",
       kicad="KiCad → via Properties → Free"),
    _s("run", "scope_mode", "str", "between", "With a footprint selection: between = connections whose pads are all on selected footprints; touching = any connection with a selected pad.",
       choices=("between", "touching"), kicad="KiCad → select footprints in the PCB editor"),
    _s("run", "unrouted_only", "bool", False, "Route only connections that have no copper yet and leave all existing copper untouched."),
    _s("run", "fail_on_preexisting", "bool", False, "Abort when kept or locked copper already has DRC violations; false only lists them as pre-existing."),
    _s("run", "kicad_cleanup", "bool", False, "After Accept, open KiCad's own Cleanup Tracks and Vias dialog (you confirm it there)."),
    _s("run", "kicad_teardrops", "bool", False, "After Accept, open KiCad's teardrop dialog (you confirm it there)."),
    _s("run", "kicad_cli_timeout_s", "float", 120, "Limit for each kicad-cli call (DRC with zone refill, preview export): a call that runs longer is killed and the run ends with KICAD_CLI_TIMEOUT, board unchanged. Raise it for very large boards. 0 disables the limit (Cancel still works).",
       unit="s", range=(0, 86400)),
    _s("run", "refill_after_accept", "bool", False, "After Accept, refill all zones from topor (a second undo step in KiCad). false = Accept is exactly one undo step; press B (Edit → Fill All Zones) yourself.",
       kicad="KiCad → Edit → Fill All Zones (B)"),
    _s("run", "detour_reference", "bool", True, "Compare every routed connection with a DRC-feasible direct path (one extra search per connection, 10 % of the remaining time budget at most) and report detours against it in the run report; false = the report compares with the topological ghost line only.", engine="detour_reference"),
    # ---- [nets]
    _s("nets", "do_not_touch", "list", [], "Net names or glob patterns whose copper is a fixed obstacle and never routed, ripped or smoothed.",
       elem="str"),
    _s("nets", "power_pattern", "str", POWER_PATTERN, "Regex naming power nets, applied only to nets still in the Default netclass; the pre-flight lists every net classified this way.",
       kicad="KiCad → File → Board Setup → Design Rules → Net Classes (a netclass wins over this pattern)"),
    _s("nets", "ground_pattern", "str", "^/?(GND|AGND|DGND|PGND|SGND|VSS)", "Regex (case-insensitive) naming ground nets, used to check that a GND plane layer has a ground zone; power nets use power_pattern."),
    _s("nets", "priority", "table", {}, "Net name or glob → priority 1..3 (1 routed first). Precedence: the @N name marker wins over a topor_pN netclass, which wins over this table; an exact name here wins over a glob (lowest number among matching globs). Tier 1 is routed first and pays the most per via; unmarked nets are tier 3, power nets tier 2.",
       kicad="KiCad → File → Board Setup → Design Rules → Net Classes (netclass topor_p1..p3) or the @N marker in the net name"),
    _s("nets", "pour_served", "table", {}, "Net name → true/false to force or forbid serving a net by a copper pour; absent = automatic. true: only pour connections are made for the net (none when it has no usable pour); false: the net is routed pad to pad although it has a pour."),
    _s("nets", "ignore_pads", "list", [], "Pads (REF.PAD, e.g. \"U1.3\") skipped as terminals, typically pads outside the board; written by the pre-flight dialog. Their connections are reported as ignored; they are neither terminals nor obstacles.",
       elem="str"),
    # ---- [vias]
    _s("vias", "via_cost_mm", "float", 2.0, "Search cost of one via in mm of extra trace: the router accepts up to this much longer to avoid a via. Overridden per net by cost_by_netclass (strongest) and cost_by_priority; this value is used for every other net.",
       unit="mm", range=(0, 1000), engine="via_cost_nm"),
    _s("vias", "max_per_connection", "int", 4, "Maximum vias on one connection.", range=(0, 32), engine="max_vias_per_connection"),
    _s("vias", "reduction", "bool", True, "After convergence remove vias whose via-free route is not much longer.", engine="via_reduction"),
    _s("vias", "reduction_len_slack_pct", "float", 10, "A via is removed when the via-free route is at most this much longer.",
       unit="%", range=(0, 1000), engine="reduction_len_slack_pct"),
    _s("vias", "via_in_pad", "bool", False, "Allow vias inside the net's own SMD pads (two extra via sites per pad, on its long axis); the run report lists every via placed in a pad. Pre-flight warns when KiCad's via protection is not filled and capped.",
       kicad="KiCad → File → Board Setup → Board Stackup → Board Finish → Via protection", engine="via_in_pad"),
    _s("vias", "plane_vias", "bool", True, "A pad whose net's pour lies only on plane layers (role gnd / pwr, not routed) gets a through via onto the plane at the nearest via site within plane_via_max_mm of the pad (inside the pad when via_in_pad is on); false reports such pads as 'terminal not on any routable layer'.",
       kicad="KiCad → File → Board Setup → Board Editor Layers (layer roles)", engine="plane_vias"),
    _s("vias", "plane_via_max_mm", "float", 1.0, "Farthest a plane via may be from its pad's copper; keeps the trace to the plane (and its inductance) short. A pad with no free via site that close fails with 'no via site within … of the terminal (plane pour)'.",
       unit="mm", range=(0, 50), engine="plane_via_max_nm"),
    _s("vias", "allowed_drills_mm", "list", [], "Optional fab filter: of the smaller alternative vias (KiCad's pre-defined vias and the fab profile) only these drill diameters may be used; empty = no filter. A net class via is always used as set in KiCad (the pre-flight warns when its drill is not listed).",
       unit="mm", elem="float", engine="allowed_drills_nm"),
    _s("vias", "fallback_drill_mm", "float", 0.4, "Drill used when KiCad has no via sizes set up (no pre-defined via and every net class at KiCad's factory 0.8 / 0.4 mm). The plugin writes it into the engine rules; the engine's via rule (engine/src/vias.rs) is the single place that applies it.", unit="mm", range=(0.05, 10),
       kicad="KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias"),
    _s("vias", "fallback_diameter_mm", "float", 0.8, "Via diameter used when KiCad has no via sizes set up.", unit="mm", range=(0.1, 20),
       kicad="KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias"),
    _s("vias", "cost_by_priority", "table", {}, "Priority (\"1\"..\"3\") → via cost in mm, e.g. 5 for high-speed nets. Precedence per net: cost_by_netclass > cost_by_priority > via_cost_mm.", unit="mm"),
    _s("vias", "cost_by_netclass", "table", {}, "Netclass name → via cost in mm, e.g. 0.5 for power. Strongest of the three via cost settings (a composite class such as \"Power,Default\" takes the first listed class that has an entry).", unit="mm",
       kicad="KiCad → File → Board Setup → Design Rules → Net Classes"),
    _s("vias", "via_relaxation", "bool", True, "Let the relaxation pass move vias a bounded distance to shorten traces.", since=V5,
       engine="via_relaxation"),
    _s("vias", "via_shove", "bool", False, "When a planned trace does not fit at its width because another connection's via is in the way, push that via aside (inside the area around its site, up to 0.8 mm, passing the push on to at most three neighbours) instead of failing the trace.",
       engine="via_shove"),
    _s("vias", "plane_via_ring", "bool", True, "Offer plane vias candidate positions all around their SMD pad (two rings of 16), not only on the pad's four axes, so the via can go to whichever side is free.",
       engine="plane_via_ring"),
    _s("vias", "thermal_pad_vias", "bool", True, "Allow vias inside the exposed (thermal) centre pad of QFN / DFN-like packages (the largest pad of a footprint, at least 4x its other pads), even when via_in_pad is off: thermal vias there are standard.",
       engine="thermal_pad_vias"),
    _s("vias", "plane_via_share", "bool", True, "A pad of a plane net may also end on a via of its own net that another pad already drops onto the plane, within plane_via_share_max_mm, instead of needing its own via (fewer vias in crowded spots; pads with no free via site of their own still connect).",
       engine="plane_via_share"),
    _s("vias", "plane_via_share_max_mm", "float", 2.0, "Largest distance from a pad to a shared plane via (see plane_via_share).", unit="mm", range=(0, 50),
       engine="plane_via_share_max_nm"),
    # ---- [layers]
    _s("layers", "preferred_direction", "table", {}, "Layer name → \"h\" (along x), \"v\" (along y) or \"none\"; traces running across it pay direction_cost. Layers without an entry are free.",
       kicad="KiCad → File → Board Setup → Board Editor Layers (layer names)"),
    _s("layers", "direction_cost", "float", 0.0, "Extra cost factor for running against a layer's preferred direction: a step of length L at angle a to it costs an extra L * direction_cost * |sin a|; 0 = off.", range=(0, 100), engine="direction_cost"),
    _s("layers", "pour_cut_cost_mm", "float", 0.0, "Extra cost, in mm of trace per mm, of running inside the filled copper of another net's pour on a layer (cutting a pour into islands costs the pour its connectivity); 0 = off. Measured: 1.0 made wiccon 58/61 with 83 vias and eprom 1.2 mm longer, so it is off by default; try 0.1 to 0.5 on boards whose pour islands after routing.", unit="mm", range=(0, 1000), engine="pour_cut_cost_nm"),
    # ---- [search]
    _s("search", "detour_factor", "float", 2.0, "A connection longer than this multiple of its ghost-line length is treated as a detour and ripped.",
       range=(1.0, 100), engine="detour_factor"),
    _s("search", "margin_um", "float", 5, "Safety margin added to every clearance during search.", unit="µm", range=(0, 1000), engine="margin_nm"),
    _s("search", "zero_slack_retry", "bool", True, "One extra pass lets a connection use a corridor pinched below the margin but not below the rules.",
       engine="zero_slack_retry"),
    _s("search", "zero_slack_failed", "bool", True, "The zero-slack pass also tries connections that failed, before any length gain of routed ones (a connection whose only corridor is pinched below the margin still gets its attempt).",
       engine="zero_slack_failed", since=V5),
    _s("search", "exit_stubs", "str", "auto", "Pad exit stubs: a short straight track that leaves a pad before the routed trace starts. auto = only for pads that cannot be left from their centre at the net width (fine-pitch rows; the stub may start off the pad centre line or be narrower, see neck_to_pad); all = also every SMD pad with a long side; off = traces start at the pad centre. A pad that needs a stub but has no room for one is reported as \"stub infeasible\".",
       choices=("auto", "all", "off"), engine="exit_stubs", since=V5),
    _s("search", "neck_to_pad", "bool", True, "Necking: an exit stub may be narrower than the net's track width (down to KiCad's minimum track width or the net's custom-rule minimum) where the full width does not fit out of the pad; the rest of the trace keeps its width. The run report lists every necked stub.",
       kicad="KiCad → File → Board Setup → Design Rules → Constraints → Minimum track width (and custom rules: track_width min)",
       engine="neck_to_pad", since=V5),
    _s("search", "neck_max_mm", "float", 0.0, "Longest necked stub; 0 = twice the pad's long side.", unit="mm", range=(0, 100),
       engine="neck_max_nm", since=V5),
    _s("search", "pad_bridge_max_mm", "float", 2.0, "Two pads of one footprint on the same net at most this far apart are joined by one straight track when it fits (possibly off the pad centre line), instead of a searched route; with exit_stubs = auto only where a pad needs a stub. 0 = off.",
       unit="mm", range=(0, 100), engine="pad_bridge_max_nm", since=V5),
    _s("search", "steiner", "bool", True, "Allow same-net T-junctions: a connection may end on an already routed track of its own net (or on kept hand copper it connects to) instead of only at a pad, so sibling connections of one net share a pad exit (false = pad to pad only).",
       engine="steiner", since=V5),
    _s("search", "dangling_ends", "str", "continue", "What to do with an existing track that ends at nothing (a hand-started trace): continue = the new copper leaves its end along the same direction (smooth, same width and layer); fixed = the end stays a fixed point and the new copper may leave at an angle; drop = remove that track and route from the pad.",
       choices=("continue", "fixed", "drop"), engine="dangling_ends", since=V5),
    _s("search", "strict_unfit_edges", "bool", False, "Fail a connection when a graph edge is too narrow for its trace even when empty, instead of pricing the crossing as overuse (stricter; may leave tight boards incomplete).",
       engine="strict_unfit_edges"),
    _s("search", "pour_listed_via_goals", "bool", False, "A connection to a copper pour may end on a same-net via the pour already lists instead of always placing a new via (saves vias, may lengthen the trace).",
       engine="pour_listed_via_goals"),
    # ---- [ripup]
    _s("ripup", "growth", "float", 1.25, "Each time a connection rips the same blocker, the history cost left on it is multiplied by this.",
       range=(1.0, 10), engine="ripup_growth"),
    _s("ripup", "blame_vias", "bool", False, "Experimental: when a trace does not fit at its width, rip the connections whose vias it is squeezed against (with the same escalating cost and cap as other rips), so those vias can move to another site. Off by default: on the benchmarks the extra rips route fewer connections.",
       engine="blame_vias"),
    _s("ripup", "failed_pressure", "bool", False, "Experimental: when a planned trace does not fit next to the traces and vias already in its passage, those traces and vias are re-planned too (with extra cost on the shared passage), instead of only the failed trace trying again.",
       engine="failed_pressure"),
    _s("ripup", "rip_pass", "bool", True, "After the routing loop, rip the routed connection that blocks the most unrouted ones (traces and vias), route its victims first and then it again; kept only when more connections route, repeated worst offender first (each retry of the same offender ranks it lower by growth).",
       engine="rip_pass"),
    _s("ripup", "max_per_pair", "int", 8, "Hard backstop on how often one connection may rip the same other connection (bounds the runtime).",
       range=(1, 1000), engine="ripup_max_per_pair"),
    # ---- [iterations]
    _s("iterations", "max", "int", 30, "Hard cap on optimisation cycles.", range=(1, 1000), engine="pathfinder_iter_cap"),
    _s("iterations", "patience", "int", 5, "Stop early when for this many cycles no connection was newly routed and gain stayed below min_gain_pct.",
       range=(1, 100), engine="patience"),
    _s("iterations", "min_gain_pct", "float", 1.0, "Smallest total cost improvement over the patience window that counts as progress.",
       unit="%", range=(0, 100), engine="min_gain_pct"),
    _s("iterations", "continue_rounds", "int", 3, "Extra rounds for the connections that are still unrouted: each round routes them again against everything routed so far as fixed copper (they may join any copper of their net) and first tries releasing the routed connections that block them most; the bar for releasing drops each round. Stops when a round routes nothing new; 0 = off.",
       range=(0, 10), engine="continue_rounds"),
    # ---- [output]
    _s("output", "geometry", "str", "arcs", "Track geometry: arcs = native KiCad arcs, segments = straight-segment approximation.",
       choices=("arcs", "segments")),
    _s("output", "report_dir", "str", "", "Report folder: \"\" = ${TOPOR_REPORT_ROOT:-${XDG_CACHE_HOME:-~/.cache}/kicad/10.0/topor}/<project>/<YYYYmmdd-HHMMSS>/; relative = <project dir>/<report_dir>/<timestamp>/; absolute = <report_dir>/<timestamp>/."),
    # ---- [expert]
    _s("expert", "hist_fail", "float", 1.0, "PathFinder history weight added to a face where a connection failed.", range=(0, 100), engine="expert.hist_fail"),
    _s("expert", "hist_block", "float", 0.5, "PathFinder history weight added to a face that blocked a connection.", range=(0, 100), engine="expert.hist_block"),
    _s("expert", "hist_overuse", "float", 0.5, "PathFinder history weight added per unit of face overuse.", range=(0, 100), engine="expert.hist_overuse"),
    _s("expert", "present_overuse", "float", 4.0, "PathFinder present-congestion penalty per unit of overuse.", range=(0, 100), engine="expert.present_overuse"),
    _s("expert", "present_growth", "float", 1.0, "Each routing cycle multiplies the present-congestion penalty by this (PathFinder escalation), so traces sharing an overfull passage must give way; 1 keeps it constant.", range=(1.0, 10), engine="expert.present_growth"),
    _s("expert", "zero_slack_candidates", "int", 4, "Candidate corridors tried per connection in the zero-slack retry.", range=(1, 64), engine="expert.zero_slack_candidates"),
    _s("expert", "relax_iter_cap", "int", 200, "Iteration cap of the relaxation (smoothing) pass.", range=(1, 10000), engine="expert.relax_iter_cap"),
    _s("expert", "own_net_capacity", "bool", True, "Capacity rule: a pad's own edges charge no clearance gap for its own net and give its corner radius back, same-net tracks need no gap between each other (false = the net-blind 0.4.0 rule).",
       engine="expert.own_net_capacity", since=V5),
    _s("expert", "wall_aware_capacity", "bool", True, "Capacity rule: on an edge with an own-net end, walls along it that are not its ends (board edge, keepouts, long pad sides) remove the part they block.",
       engine="expert.wall_aware_capacity", since=V5),
    _s("expert", "transit_pinch", "bool", True, "Search: refuse to pass around a pad or via that a wall (board edge, keepout, other copper) pinches below the track's width plus clearances.",
       engine="expert.transit_pinch", since=V5),
    _s("expert", "steiner_min_angle_deg", "float", 60.0, "Same-net T-junctions: the joining track must meet the track it joins at no less than this angle (all three angles at the T are then at least this); below 60 the branch forms an acute sliver (acid trap).",
       unit="°", range=(0, 90), engine="expert.steiner_min_angle_deg", since=V5),
    _s("expert", "steiner_len_slack", "float", 1.1, "Same-net T-junctions: a junction whose final track is longer than this factor times the search's estimate (+ 1 mm) is dropped and the connection routed again without it.",
       range=(1.0, 10), engine="expert.steiner_len_slack", since=V5),
    _s("expert", "steiner_ghost_factor", "float", 1.0, "Same-net T-junctions: a junction is taken only while its length plus via cost stays within this factor of the connection's own pad-to-pad estimate on the empty board (1 = only junctions cheaper than the free pad route; larger also lets junctions dodge congestion).",
       range=(0.0, 10), engine="expert.steiner_ghost_factor", since=V5),
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
    "profile": "diagnostics only; timings must not enter the deterministic result",
    "seed": "fixed for byte-identical, reproducible results",
    "scope": "built per run from the KiCad selection and scope_mode, not stored",
    "net_priority": "built per run from @N markers, netclasses and [nets] priority",
    "exclude_nets": "built per run from [nets] do_not_touch (patterns expanded against the board's net names)",
    "ignore_pads": "passed through from [nets] ignore_pads (REF.PAD strings) by run.build_options",
    "pour_served": "passed through from [nets] pour_served (boolean entries only) by run.build_options",
    "via_cost_by_net": "built per run from [vias] cost_by_priority / cost_by_netclass (net name -> nm)",
    "layer_direction": "built per run from [layers] preferred_direction (\"h\" -> 0, \"v\" -> 90 degrees)",
    "via_cost_by_code": "run-time form of via_cost_by_net by net code, filled by the engine",
    "detour_reference_budget_pct": "internal: the run passes 10 (percent of the remaining budget) whenever run.detour_reference is on",
}

# KiCad-owned behaviours, written as comment-only entries: (name, place)
KICAD_ONLY = [
    ("Clearance", "KiCad → File → Board Setup → Design Rules → Constraints"),
    ("Track widths", "KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Track Widths and netclass track width"),
    ("Via sizes", "KiCad → File → Board Setup → Design Rules → Net Classes (via size / hole per net) and Pre-defined Sizes → Vias (the smaller ones are used where space requires)"),
    ("Minimum via, hole and annular ring", "KiCad → File → Board Setup → Design Rules → Constraints (values below topor's fab floors are raised; see operate.md, Fab minimums)"),
    ("Copper-to-edge clearance", "KiCad → File → Board Setup → Design Rules → Constraints → Copper to Edge Clearance"),
    ("Netclasses and net assignments", "KiCad → File → Board Setup → Design Rules → Net Classes"),
    ("Custom rules", "KiCad → File → Board Setup → Design Rules → Custom Rules (.kicad_dru)"),
    ("Layer types (signal/power/mixed/jumper)", "KiCad → File → Board Setup → Board Editor Layers"),
    ("Locked tracks and vias", "KiCad → item Properties → Locked"),
    ("Via protection (fill/cap)", "KiCad → File → Board Setup → Board Stackup → Board Finish → Via protection"),
]
