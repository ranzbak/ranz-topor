"""Settings schema of ``topor.toml`` (46 §2): the single source for the file comments, the loader,
the generated reference (docs/user/configuration.md) and the engine-option mapping.

Complexity: module-level constants; iteration O(#settings); SETTINGS_BY_KEY lookup O(1).
"""

from __future__ import annotations

from dataclasses import dataclass, replace
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
    "planes": "Signal copper on plane layers (layers whose role is gnd / pwr, not routed).",
    "layers": "Stackup roles per copper layer (F.Cu = \"signal\", In1.Cu = \"gnd\", ...) and per-layer preferences.",
    "search": "Path search tuning.",
    "ripup": "Rip-up and re-route escalation.",
    "iterations": "Optimisation stop rule.",
    "optimize": "Topological core: how a run iterates (whole-route cycles with a fluctuation schedule until the routes are stable, like TopoR; best of N seeded runs), its safety caps (work caps, so a run is reproducible), and which result it keeps.",
    "drawing": "Topological core: how the routed topology is drawn as copper (drawing style and its prices).",
    "diffpair": "Differential pairs (topological core): which nets pair up (KiCad's own name rule), their width, gap and via gap (from KiCad's net classes and custom rules unless set here), and how the pair is drawn. A pair is planned as one wide wire and drawn as two coupled tracks (docs/dev/kicad-diff-pairs.md).",
    "output": "Result geometry and report location.",
    "expert": "PathFinder constants; change only when you know the router.",
    "factors": "Price of what a route does (social factors), in mm of copper per unit: weight, scope and report level per factor name (docs/dev/factors.md). Factor code is trusted code: only add factor files you trust.",
}

SETTINGS: list[Setting] = [
    # ---- [run]
    _s("run", "timeout_s", "float", 600, "Limit on router time (seconds on the clock chosen by timeout_clock: wall = elapsed time, cpu = CPU seconds summed over all threads): all routing passes of the run together stop after this long. The topological core ends at \"Routes stable\" (headless), at your OK, or by itself after it (finish_when_stable), so this limit is only an emergency stop: when it fires before the routes are stable the report warns (TIME_BUDGET_HIT) that the result may differ between runs. Measured router times with the default best of 5 runs (4 threads, two boards at once, M-B0): eprom-to-kernal 2 s, wiccon-2023 about 25 s, dcsgonebadge about 450 s (320-710), qmtech about 1000 s (680-1300); a single run ([optimize] runs = 1) takes roughly a quarter to a half of that. Runs on large boards (dcsgonebadge, qmtech with runs = 5) may hit this limit; they keep the best result so far and the report warns (TIME_BUDGET_HIT). The pre-flight DRC, the DRC rounds and the kicad-cli refill / preview exports are not counted. 0 or \"none\" disables it (Cancel always works and keeps the best result so far).",
       unit="s", range=(0, 86400), engine="wall_clock_s"),
    _s("run", "timeout_clock", "str", "wall", "Which clock timeout_s counts. wall = seconds on the clock, what you wait for (the default). cpu = CPU seconds of router work, summed over all threads of the run, so a 4-thread run uses its limit up 4 times as fast as the wall clock: the limit then means the same amount of work on a busy machine as on an idle one and with any thread count, which makes it hard to buy a better result by adding threads, and is what benchmark comparisons use (harness/score_runs.py: cpu, 2400 s = the old 600 s wall x 4 threads). Caveat: on a laptop with two hardware threads per core and turbo clocks, a CPU-second under heavy load still does somewhat less work than on an idle machine. When the limit fires the best result so far is kept and the report says which clock (stop reason timeout_wall / timeout_cpu, TIME_BUDGET_HIT). The CPU time spent is shown in the progress dialog and the report either way.",
       choices=("wall", "cpu")),
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
    _s("planes", "signals", "str", "costly", "Topological core: whether signal copper may use plane layers. costly = yes, at the price of the plane_signal factor (10 per mm on top of the length) plus plane_cut per cut of the plane, so a plane carries a signal only where the signal layers cannot; forbidden = never, plane layers are left to their pour. The run report lists the signal length on plane layers and the cuts.",
       choices=("costly", "forbidden"), kicad="KiCad → File → Board Setup → Board Editor Layers (layer roles)", engine="plane_signals"),
    _s("planes", "stitch_islands", "bool", True, "Topological core: after routing, the refilled pours are estimated (the fill of the ripped board, minus the routed copper of other nets grown by the clearance, minus features thinner than the zone's minimum width; islands kept or dropped as the zone's island removal setting says). A piece of a pour net's copper that the routed copper cut off from the rest of its pour (an island holding a pad or via of the net, or a pad whose thermal spokes are all cut) gets a through via inside that island near its pad, joining it to the pour on another layer where that pour is solid, or a short stub from the pad; the via sites prefer room (the via_squeeze factor). The run report lists every piece cut off and every stitch. false = the pieces are only reported; KiCad's DRC then lists them as missing connections.",
       kicad="KiCad → zone properties (Clearance, Minimum width, Remove islands)", engine="stitch_islands"),
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
    _s("search", "detour_factor", "float", 2.0, "A connection longer than this multiple of its straight pad-to-pad distance is treated as a detour: the topological core searches it again with cheaper crossings (the layer assignment then resolves a crossing with a via) instead of accepting a wandering path; 0 = off.",
       range=(0.0, 100), engine="detour_factor"),
    _s("search", "net_tree", "str", "free", "Topological core: how a net's connections form its tree. \"free\" = the tree follows the routing: each connection of the net joins the two parts of the net's routed copper its pads lie in, at whichever of their pads, or at a branch point (T-junction) on one of their routed traces, gives the cheapest path; a branch ends on its trunk's trace, one layer through the branch point (a via goes on the branch). \"fixed\" = pad to pad as planned before routing (the shortest straight-line tree), no branch points.",
       choices=["free", "fixed"], engine="net_tree"),
    _s("search", "crossing_vias", "str", "crossing", "Topological core: how the search prices a crossing of two nets. \"crossing\" (default) = the crossing factor ([factors] crossing, 20 mm: TopoR takes a 13-20 mm detour rather than a via pair) stands for the layer change the layer assignment makes; only a crossing that no layer assignment can separate (both traces held to the same single layer where they cross, e.g. over another net's pad on the other side) is priced as a violation (the overflow factor). \"via\" = also price, with the via factor, the via that a crossing of two traces held to the same layer implies, and keep the trace on the other layer after it (measured D6b: on qmtech 16 fewer connections, more vias; on dcsgonebadge the same).",
       choices=["crossing", "via"], engine="crossing_vias"),
    _s("search", "junction_pad_mm", "float", 0.0, "Topological core, net_tree = \"free\": a branch point on a trace closer than this to a pad of the copper it joins is not used; the pad itself is the junction (the trace runs through the pad centre) instead of a T next to it. 0 = no such distance (default): the junction_angle and branch factors decide between a T next to a pad and the pad itself on price.",
       unit="mm", range=(0, 20), engine="junction_pad_mm"),
    _s("search", "fuse_branches", "bool", True, "Topological core, net_tree = \"free\": after the drawing, a trace that leaves a branch point or a pad right beside another trace of its net leaves that trace's copper where the two part instead: one copper and a T there, no two parallel traces (TopoR moves T-points the same way in its post-optimisation). Done only where it is shorter, the junction_angle factor included, and the new track keeps every clearance.",
       engine="fuse_branches"),
    _s("search", "margin_um", "float", 0, "Safety margin added to every clearance during search and drawing; 0 = exactly the design rule (KiCad DRC accepts copper at exactly the clearance, and the routed copper is checked with the exact rule). Raise it only for headroom against later manual edits.", unit="µm", range=(0, 1000), engine="margin_nm"),
    _s("search", "zero_slack_retry", "bool", True, "One extra pass lets a connection use a corridor pinched below the margin but not below the rules.",
       engine="zero_slack_retry"),
    _s("search", "zero_slack_failed", "bool", True, "The zero-slack pass also tries connections that failed, before any length gain of routed ones (a connection whose only corridor is pinched below the margin still gets its attempt).",
       engine="zero_slack_failed", since=V5),
    _s("search", "exit_stubs", "str", "auto", "Pad exit stubs: a short straight track that leaves a pad before the routed trace starts. auto = only for pads that cannot be left from their centre at the net width (fine-pitch rows; the stub may start off the pad centre line or be narrower, see neck_to_pad); all = also every SMD pad with a long side; off = traces start at the pad centre. A pad that needs a stub but has no room for one is reported as \"stub infeasible\".",
       choices=("auto", "all", "off"), engine="exit_stubs", since=V5),
    _s("search", "neck_to_pad", "bool", True, "Necking: an exit stub may be narrower than the net's track width (down to KiCad's minimum track width or the net's custom-rule minimum) where the full width does not fit out of the pad; the rest of the trace keeps its width. The run report lists every necked stub.",
       kicad="KiCad → File → Board Setup → Design Rules → Constraints → Minimum track width (and custom rules: track_width min)",
       engine="neck_to_pad"),
    _s("search", "neck_from_pad", "bool", True, "Topological core, width in the plan: where a trace's net width does not fit a passage (a bottleneck, or the way out of a fine-pitch pad), the planner narrows it down to the minimum width, from the nearest pad of the connection up to that passage (a neck starts at the footprint and may reach further back to make room for other traces; never a narrow stretch in the middle of a full-width trace). Off: only the bottleneck passages themselves are narrowed. Every necked stretch is a warning in the report and costs the necking factor.",
       kicad="KiCad → File → Board Setup → Design Rules → Constraints → Minimum track width (and custom rules: track_width min)",
       engine="neck_from_pad"),
    _s("search", "neck_in_drawing", "bool", False, "Topological core: also let the drawing step narrow a pad end of a trace that the plan kept at full width, when the full-width copper is rejected there (the behaviour before the width moved into the plan). Off: the drawing narrows a trace only where the plan narrowed it.",
       engine="neck_in_drawing"),
    _s("search", "drawing_feedback", "bool", True, "Topological core: a connection whose copper the drawing step could not fit (a clearance to a pad or to a trace drawn earlier would be violated) is drawn first and the traces that block its corridor are drawn again around it, once per connection, instead of being reported failed at once.",
       engine="drawing_feedback"),
    _s("drawing", "style", "str", "octilinear", "Topological core: the shape of the drawn traces. octilinear = straight and 45 degree segments, few bends, 90 degree bends only where needed (the usual hand-routed style): every trace is first found as the shortest path its topology allows, then redrawn with 0/45/90 degree segments inside the same corridor where that keeps every clearance; a trace that hugs a pad or another trace runs parallel to it at the clearance, so bundles keep their pitch through their bends, and a trace leaves an elongated pad along its long axis until it is past the pad's tip. Where 45 degrees does not fit, that stretch keeps its any angle and pays the any_angle factor (reported as a warning with place and angle; never an unrouted trace or a new violation because of the style). smooth = the shortest any-angle path as it is (straight chords, no arcs).",
       choices=("octilinear", "smooth"), engine="draw_style"),
    _s("drawing", "any_angle", "str", "costly", "Topological core, octilinear style: a stretch that is not at 0, 45 or 90 degrees. costly = allowed where 45 degrees does not fit, at the price of the any_angle factor (15 mm per stretch plus 2 per mm of it), each one a warning with place and angle; forbidden = such a trace counts as not drawn and goes back to the router like a trace that did not fit (never drop still writes it on its last attempt, with the warning).",
       choices=("costly", "forbidden"), engine="any_angle"),
    _s("drawing", "acute", "str", "costly", "Topological core, octilinear style: a bend sharper than 90 degrees (an acid trap, an impedance step). costly = allowed where nothing else fits, at the price of the acute_bend factor (50 mm per bend, up to twice that as the angle closes), each one a warning with place and angle; forbidden = the octilinear drawing never makes one (the shortest path keeps its own shape there).",
       choices=("costly", "forbidden"), engine="acute"),
    _s("drawing", "octilinear_room", "bool", True, "Topological core, octilinear style: the layer assignment, rip-up and the drawing's packing count the room an octilinear trace takes in a passage (around a corner it runs on an octagon, up to 8 % wider than the any-angle arc; a slanted stretch is drawn with 0 and 45 degree pieces that cross a passage at their own angles) instead of the any-angle room, so the plan leaves the room the 45 degree drawing needs. false = they count the any-angle room.",
       engine="octi_room"),
    _s("drawing", "bundle_pull", "float", 0.3, "Topological core, octilinear style: share of a segment's length taken off its price where it runs beside an already drawn trace of another net at the clearance, so a trace follows its neighbour and a bundle keeps its pitch through its 45 degree bends, as by hand (each bend then makes the outer trace 0.83 x pitch longer, reported per bundle). 0 = every trace takes its own shortest shape.",
       range=(0, 0.9), engine="bundle_pull"),
    _s("drawing", "bend_mm", "float", 0.25, "Topological core, octilinear style: price of one 45 degree bend in mm of extra trace (a 90 degree bend costs twice): among shapes of about the same length the drawing takes the one with fewer bends, and it accepts up to this much more length per 45 degrees of bend it saves.",
       unit="mm", range=(0, 100), engine="bend_mm"),
    _s("drawing", "bend_spacing_mm", "float", 0.5, "Topological core, octilinear style: preferred least length of a segment between two bends; a shorter one (a jog) pays more the shorter it is, up to twice bend_mm. In pin fields the limit drops to twice the trace's width plus clearance, so short pieces between pins stay possible.",
       unit="mm", range=(0, 50), engine="bend_spacing_mm"),
    # ---- [diffpair]
    _s("diffpair", "enabled", "bool", True, "Topological core: route differential pairs as coupled pairs: each pair connection is planned as one wire as wide as both tracks and the gap (so it takes its place early, crosses other wires once and changes layer with one via pair) and drawn as two tracks at the pair gap, joined to the pads by short fan-out stubs. false = their connections stay unrouted and are listed (DIFF_PAIRS_SKIPPED), the behaviour before 0.4.2.",
       engine="diffpair_route", kicad="KiCad → net names ending in +/- or P/N (digits and _ after them allowed): Schematic net labels"),
    _s("diffpair", "detect", "str", "kicad", "Which nets form a pair. kicad = KiCad's own rule (MatchDpSuffix): the full net name, skipping trailing digits and _, ends in + or - or upper-case P or N, and the net with that character swapped exists (CLK_N1 pairs with CLK_P1, USB+ with USB-; USB+ with USB_N does not). off = no pairs. Extra pairs: [diffpair] pairs.",
       choices=("kicad", "off")),
    _s("diffpair", "pairs", "table", {}, "Extra pairs KiCad's name rule does not find: positive net name → negative net name (\"/USB/DP\" = \"/USB/DM\").",
       kicad="KiCad → File → Board Setup → Net Classes (pairs are found by name in KiCad)"),
    _s("diffpair", "width_source", "str", "pair", "Track width of a pair. pair = the net class's differential pair width (KiCad's diff_pair_width; the board's minimum track width when unset), raised to the board minimum as KiCad does; track = the net's normal track width.",
       choices=("pair", "track"), kicad="KiCad → File → Board Setup → Design Rules → Net Classes → DP Width"),
    _s("diffpair", "gap_mm", "float", 0.0, "Gap between the two tracks, edge to edge; 0 = KiCad's: the net class's diff_pair_gap, overridden by a custom rule diff_pair_gap (opt ..), at least the board's minimum clearance. Between the two tracks of a pair this gap is the clearance (KiCad does the same when it is below the net class clearance).",
       unit="mm", range=(0, 10), kicad="KiCad → File → Board Setup → Design Rules → Net Classes → DP Gap, Custom Rules diff_pair_gap"),
    _s("diffpair", "via_gap_mm", "float", 0.0, "Gap between the two vias of a via pair, edge to edge; 0 = KiCad's: the pair gap, raised so the holes keep the hole-to-hole and copper-to-hole clearances.",
       unit="mm", range=(0, 10), kicad="KiCad → File → Board Setup → Design Rules → Net Classes → DP Via Gap"),
    _s("diffpair", "gap_tolerance_mm", "float", 0.01, "Two parallel pair tracks count as coupled when their gap is within this of the pair gap (KiCad's own band: 0.01 mm).",
       unit="mm", range=(0, 1)),
    _s("diffpair", "max_uncoupled_mm", "float", 0.0, "Uncoupled length per pair connection above which the dp_uncoupled price rises four times as steeply (a graded limit, no cut-off); 0 = KiCad's custom rule diff_pair_uncoupled (max ..) when there is one, else no limit. Only a KiCad rule makes KiCad's DRC report it.",
       unit="mm", range=(0, 1000), kicad="KiCad → File → Board Setup → Design Rules → Custom Rules diff_pair_uncoupled"),
    _s("diffpair", "skew_max_mm", "float", 0.0, "Length difference between the two tracks of a pair connection above which the dp_skew price rises four times as steeply; 0 = KiCad's custom rule skew (max ..) when there is one, else no limit.",
       unit="mm", range=(0, 1000), kicad="KiCad → File → Board Setup → Design Rules → Custom Rules skew (within_diff_pairs)"),
    _s("diffpair", "zip_max_mm", "float", 3.0, "Fan-out zone: how far from a pad (or a via pair) along the pair the two tracks may run apart before they meet at the pair gap. The drawing takes the nearest point where both stubs keep every clearance; when none within this distance does, the least violating one (reported).",
       unit="mm", range=(0, 50), engine="diffpair_zip_max_nm"),
    _s("diffpair", "via_pair_axes", "list", [0.0, 45.0, 90.0, 135.0], "Directions (degrees) allowed for the line through the two vias of a via pair; the one nearest to square to the pair is used, so both tracks enter their vias side by side.",
       unit="°", engine="diffpair_via_axes"),
    _s("vias", "reserve_sites", "bool", True, "Topological core: a via the plan puts at a layer change takes its site; another net's via planned in the same small spot (too small for two vias) counts as a via without a site, so layer assignment and rip-up move it or reroute its trace before the drawing step instead of finding no room for it there.",
       engine="reserve_via_sites"),
    _s("vias", "offset_sites", "bool", True, "Topological core: when no sample point of the corridor where a connection changes layer has room for a via, also try positions a via-radius plus clearance away from the corners of nearby pads and copper (and from the traces drawn before), nearest to the connection's path first. More connections get a via site, at the cost of some runtime.",
       engine="via_offset_sites"),
    _s("search", "feedback_max_rip", "int", 8, "Work cap: most already drawn traces one such feedback round may take out and draw again (a larger value rescues more connections and costs more runtime). Whether they are taken out at all is a graded price, not this count: the reroute factor's price per blocking trace (more for one redrawn before in the same round) against the violation factor's price of drawing the rejected connection through them; the cheapest rescues go first.",
       range=(0, 100), engine="feedback_max_rip"),
    _s("search", "reroute_rounds", "int", 6, "Topological core: most rounds of negotiated rip-up-and-reroute. After the layers are assigned, the connections that run through a passage holding more copper on one layer than fits are taken out and routed again, with a growing price (the congestion factor) on the passages that were over-full, until none is over-full or a round brings no gain; the best round is kept. 0 = off.",
       range=(0, 50), engine="reroute_rounds"),
    _s("vias", "reduce_after_drawing", "bool", True, "Topological core: after all copper is drawn, each connection with vias is tried again with one stretch between two vias (or between a via and a through-hole pad) moved onto the layer of its neighbouring stretch, which drops the vias between them; the change is kept where the redrawn trace passes every clearance check, else the old copper stays. Fewer vias at the cost of some runtime.",
       engine="via_reduce"),
    _s("search", "neck_max_mm", "float", 0.0, "Longest necked stub; 0 = twice the pad's long side.", unit="mm", range=(0, 100),
       engine="neck_max_nm"),
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
    # ---- [optimize]
    _s("optimize", "rounds", "int", 30, "Topological core: safety cap on whole-route cycles per run (the first one included). A run does not stop at a fixed count: like TopoR it keeps iterating (every cycle routes again with the passages the drawing found too small lowered and with freshly fluctuated weights, and offers its result variants) until its routes are stable (see stable_window), the user presses OK / Cancel, or run.timeout_s fires; this cap only ends a run that never settles. A work cap, not a time cap: the same board and settings give the same result on any machine, loaded or not, and with any thread count.",
       range=(1, 1000), engine="topo_rounds"),
    _s("optimize", "max_ripped", "int", 5000, "Topological core: most connections one whole-route round may take out and route again (negotiated rip-up-and-reroute, drawing feedback and the P4 moves together). A work cap (see rounds).",
       range=(0, 1000000), engine="max_ripped"),
    _s("optimize", "max_expansions", "int", 2000000, "Topological core: search cap per connection (expanded search nodes); the one retry of a connection that found no path uses four times this. A work cap (see rounds): a connection that needs more is reported as failed.",
       range=(1000, 1000000000), engine="max_expansions"),
    _s("optimize", "objective", "str", "length_vias", "Topological core: how the result variants are judged. Every whole-route round gives two variants (as drawn, and after the via reduction); the archive keeps every variant no other beats on all four TopoR criteria (errors = connections not routed, vias, total length, shrinkages = necked stretches). length_vias = fewest errors first, then the least length + mm_per_via x vias (a step to fewer vias is taken while it costs at most mm_per_via of length per via removed); area = fewest errors first, then the least occupied area (a trace counts length x (width + clearance), a via its pad-plus-clearance disc on every layer). The run report lists the variant table and marks the pick; the picked variant goes to KiCad.",
       choices=("length_vias", "area"), engine="objective"),
    _s("optimize", "mm_per_via", "float", 25.0, "Via price of objective = length_vias, in mm of trace per via (TopoR: good results at 25 mm/via); also the price the rip-up-and-reroute uses to pick its best round.",
       unit="mm", range=(0, 1000), engine="mm_per_via"),
    _s("optimize", "variants", "int", 8, "Most result variants the archive keeps (population size); the best variant on each criterion (fewest errors, shortest, fewest vias, fewest shrinkages, fewest warnings) is always kept on top.",
       range=(1, 100), engine="variants"),
    _s("optimize", "moves", "bool", False, "Topological core: after the rip-up-and-reroute, a descent of local moves, each kept only when it lowers the price of the social factors ([factors]): reroute a connection, connect it to another pad of its net, put it next to a parallel trace of its net, slide a crossing along a shared channel, reroute two crossing connections in the other order, remove double crossings; then on the layers: swap the layers of a crossing, shift a via along its trace, lift a stretch onto another layer. The descent is kept only when the board objective after the layer assignment improves. Off by default: measured on the gate boards it routed 6 more connections on tanmatsu but 7 fewer on hackerhotel2027 and 1 fewer on bornhack2026 (the drawing does not always follow the topological objective); try it per board.",
       engine="moves"),
    _s("optimize", "max_moves", "int", 400, "Most move evaluations of that descent per whole-route round (the layer moves have the same cap). A work cap (see rounds).",
       range=(0, 1000000), engine="max_moves"),
    _s("optimize", "fluctuation", "float", 0.2, "Fluctuation schedule, start width: in every cycle each factor weight is multiplied by a seeded random factor between 1 - width and 1 + width (search, rip-up-and-reroute and descent), so the search leaves local optima (TopoR's fluctuating coefficients). The width starts here, narrows every cycle (fluctuation_narrowing) and is widened again when the run is stuck (fluctuation_stuck, fluctuation_reheat): annealing with reheating. The first cycle of the first run always uses the plain weights. The seed is fixed, so runs stay reproducible; 0 = fixed weights throughout.",
       range=(0, 0.9), engine="fluctuation"),
    _s("optimize", "fluctuation_narrowing", "float", 0.7, "Fluctuation schedule: the width is multiplied by this after every cycle (1 = constant width, smaller = narrows faster).",
       range=(0, 1), engine="fluctuation_narrowing"),
    _s("optimize", "fluctuation_stuck", "int", 2, "Fluctuation schedule: after this many cycles without a better result while connections are still not complete, the width is set back to fluctuation_reheat and narrows again from there; 0 = never widen again.",
       range=(0, 100), engine="fluctuation_stuck"),
    _s("optimize", "fluctuation_reheat", "float", 0.2, "Fluctuation schedule: the width after a reheat (see fluctuation_stuck).",
       range=(0, 0.9), engine="fluctuation_reheat"),
    _s("optimize", "stable_window", "int", 4, "Routes stable: the gain of the best result is measured over this many recent cycles (so stable comes at cycle stable_window + 1 at the earliest; TopoR: do not stop before 5-10 rounds).",
       range=(1, 100), engine="stable_window"),
    _s("optimize", "stable_gain_pct", "float", 0.5, "Routes stable: when the best result gained less than this over the last stable_window cycles (a connection more routed always counts as a gain; else the objective's relative improvement), the progress dialog shows \"Routes stable\" and its Cancel button reads OK (same action: stop and keep the best result so far, which is exactly the board you get). The run keeps iterating for marginal gains until you press OK. Runs without the dialog (harness, MCP) stop at stable plus stable_patience cycles.",
       unit="%", range=(0, 100), engine="stable_gain_pct"),
    _s("optimize", "stable_patience", "int", 0, "Runs without the progress dialog (harness, MCP): cycles to keep iterating after the routes are stable before stopping.",
       range=(0, 1000), engine="stable_patience"),
    _s("optimize", "finish_when_stable", "bool", True, "With the progress dialog: the run finishes by itself at \"Routes stable\" (a normal finish, no time-limit warning; with finish_patience > 0 only after that many more rounds without a better result); you can press OK at any time before. false = keep iterating for marginal gains after \"Routes stable\" until you press OK / Cancel, the safety cap (rounds) or run.timeout_s (the TopoR way). Runs without the dialog always stop at stable (+ stable_patience).",
       engine="finish_when_stable"),
    _s("optimize", "finish_patience", "int", 0, "Rounds without a better result after \"Routes stable\" before a run with the progress dialog finishes by itself (see finish_when_stable); 0 = it stops at the round that becomes stable (user decision 2026-11-07: \"When route stable appears it should stop indeed\").",
       range=(0, 1000), engine="finish_patience"),
    _s("optimize", "runs", "int", 5, "Best of N: independent seeded runs (each with its own fluctuation stream); the best result by the objective is offered. They run in parallel on the engine's threads (shared, not multiplied: more runs than cores take longer); the progress dialog shows the best so far and how many runs are stable. 1 = a single run.",
       range=(1, 64), engine="runs"),
    _s("optimize", "allow_violations", "bool", True, "Never drop (like TopoR): a connection whose drawing finds no legal geometry (a gap too narrow when drawn, a via without a free site, a crossing that cannot be layered, a branch that meets its trace on another layer) stays in the router and is drawn anyway on its best geometry; every rule it breaks is priced very high by the violation factor ([factors] weight violation: 1000 mm per violation plus per 0.01 mm² of depth x length), so the later cycles work to remove it, and the best result counts errors first (connections with violations plus connections without copper), then the objective with the violation price. The violating copper is written to the board and each violation is listed in the report (VIOLATIONS) with place, type and cause, so you can fix it by hand or move a part; KiCad's DRC shows them too. false = legal copper only: such a connection is reported unrouted and nothing of it is written (the behaviour before never drop).",
       engine="allow_violations"),
    _s("optimize", "history_fade", "float", 0.5, "Local feedback between cycles: what a cycle hands to the next one (the history of passages that were over-full, the history on the stretches where a connection's copper broke a rule, the soft cuts) is multiplied by this before the new cycle adds its own, so old findings fade (Dr. CU, IEEE TCAD 2020, section IV-C). The history of a violation lies only on the stretch where it happened (weighted by the distance to it), not on the whole trace. 1 = never fades, 0 = no memory between cycles.",
       range=(0, 1), engine="history_fade"),
    _s("optimize", "soft_cuts", "bool", True, "Local feedback between cycles: a passage the drawing found too narrow gets a lowered room (a cut) for the next cycles; when a cycle comes out worse than the best so far, all such cuts turn into a price on their passages instead (Haas 2026, section VI-E: hard cuts become soft), so they no longer forbid what the plan needs. false = the lowered rooms stay for the whole run.",
       engine="soft_cuts"),
    _s("optimize", "crossing_rules", "bool", True, "Crossing rules in the planner: before and after the rip-up and reroute, the planner looks for the topological patterns that force vias, crossings on one layer or detours, and re-plans the two traces of each in the other order, kept when their price (the factors) and the plan get better: a trace of another net wrapped around a connection's terminal pad (a clinch or half-loop: the connection must cross it or go around; Luzin & Popov 2012), two traces of different nets crossing twice, a crossing whose two stretches are both held to one layer. Traces of a net tree with branch points keep their place.",
       engine="crossing_rules"),
    _s("optimize", "seed", "int", 0, "Seed of the fluctuating coefficients (no effect while fluctuation = 0); run k of [optimize] runs uses a seed derived from it. The same seed gives the same result; other seeds give other, equally valid variants (used to measure the spread of results over several runs).",
       range=(0, 1000000), engine="seed"),
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
    # ---- [factors]
    _s("factors", "weight", "table", {}, "Factor name → weight (mm per unit, ≥ 0); absent = the factor's default. Factors and defaults: length 1 per mm (scaled by width + clearance), via 25 per via, crossing 20 per crossing of two nets, same_net_crossing 0, plane_signal 10 per mm on a plane layer, plane_cut 25 per cut of a plane, necking 30 per necked stretch, overflow 50 per pass through an overfull cut, congestion 10 per pass per earlier over-full round of the cut, squeeze 5 per pass that leaves a cut less than a track pitch of slack between two other traces, via_squeeze 5 per passage-layer next to a via left with no slack (linear from one via diameter of slack, free, to none; the planner puts a via where the passages around it keep the most room), blocks_pad 30 per trace that takes the last free way out of another net's unrouted pad, ignored_rule 100 per violation of a KiCad check set to ignore, reroute 5 per earlier reroute of a connection that the P4 search moves again, branch 1 per branch point on a trace or through-pad (a reward: subtracted; net_tree = \"free\" lets a connection end on its net's routed copper there instead of a separate trace to the planned pad, and a separate path still wins when it is cheaper; 0 = only where the branch is cheaper on length alone), smd_through 0 per second trace ending on an SMD pad (assembly cost of a through-pad; off by default so it does not cancel branch), junction_angle 10 per mm of width + clearance per unit of psi(a) for a new trace that leaves a junction (a branch point, or a pad that already carries a trace of its net) at angle a to the nearest arm already there, psi(a) = (sec(60° - a/2) - 1) x sin 60° / sin(a/2) below 120° (0 from 120°, the Steiner optimum, up; 90° 0.04, 60° 0.27, 48° 0.5, 30° 1.4, 15° 4.3: shallow T splits and traces leaving a pad side by side get ever more expensive, with no cutoff), any_angle 15 per stretch of an octilinear drawing that is not at 0/45/90 degrees plus 2 per mm of it ([drawing] style, any_angle), acute_bend 50 per bend sharper than 90 degrees, up to twice that as the angle closes ([drawing] acute), violation 1000 per rule the written copper breaks plus per 0.01 mm² of its depth x length, and a connection without copper as its width + clearance over its straight length (never drop, [optimize] allow_violations: very high, so the cycles work to remove it; the best result counts the connections with violations first); differential pairs ([diffpair]): dp_uncoupled 1 per mm of a pair connection not coupled at the gap plus 4 per mm past a diff_pair_uncoupled rule's maximum, dp_skew 10 per mm of length difference between the two tracks plus 4 per mm past a skew rule's maximum, dp_gap_deviation 5 per mm of parallel pair track at a gap off the target by more than the tolerance (weighted by deviation / (deviation + tolerance)), dp_via_unpaired 25 per via on one track without a partner via, dp_polarity_cross 1000 per pair connection whose two tracks swap sides (effectively prohibitive).",
       unit="mm", engine="factors.weight"),
    _s("factors", "scope", "table", {}, "Factor name → which wires the factor prices: \"all\" (default) or \"tier:N\" (wires of priority tier N only).",
       engine="factors.scope"),
    _s("factors", "report", "table", {}, "Factor name → \"off\", \"info\" or \"warning\": events of the factor become lines of that level in the run report. Defaults: necking, ignored_rule, via_squeeze, any_angle, acute_bend, violation, dp_via_unpaired and dp_polarity_cross warning (via_squeeze: one line per drawn via with less slack than half its diameter next to it, \"via in a narrow pass\"; violation: one line per rule the written copper breaks, with place, type and cause), the rest off.",
       engine="factors.report"),
    _s("expert", "drc_loop_cap", "int", 5, "Maximum route / DRC / re-route rounds. Not used by the topological core: it takes no DRC feedback, so the result is judged by one DRC round (a re-route would repeat it).", range=(1, 50), engine="drc_loop_cap"),
]

#: Settings of the classic engine that the topological core (the engine of this branch) does not read (K1: checked
#: against engine/src/topo by grep); their docs say so, so nobody tunes a key that does nothing. The factors in
#: [factors] (via, crossing, squeeze, ...) and the [optimize] work caps take their place.
CLASSIC_ONLY = {
    ("run", "detour_reference"), ("vias", "via_cost_mm"), ("vias", "max_per_connection"), ("vias", "reduction"),
    ("vias", "reduction_len_slack_pct"), ("vias", "plane_via_max_mm"), ("vias", "cost_by_priority"),
    ("vias", "cost_by_netclass"), ("vias", "via_shove"), ("vias", "plane_via_ring"), ("vias", "thermal_pad_vias"),
    ("vias", "plane_via_share"), ("vias", "plane_via_share_max_mm"), ("layers", "preferred_direction"),
    ("layers", "direction_cost"), ("layers", "pour_cut_cost_mm"), ("search", "zero_slack_retry"),
    ("search", "strict_unfit_edges"), ("search", "pour_listed_via_goals"),
    ("ripup", "growth"), ("ripup", "blame_vias"), ("ripup", "failed_pressure"), ("ripup", "rip_pass"),
    ("ripup", "max_per_pair"), ("iterations", "max"), ("iterations", "patience"), ("iterations", "min_gain_pct"),
    ("iterations", "continue_rounds"), ("expert", "hist_fail"), ("expert", "hist_block"), ("expert", "hist_overuse"),
    ("expert", "present_overuse"), ("expert", "present_growth"), ("expert", "zero_slack_candidates"),
    ("expert", "relax_iter_cap"),
}
CLASSIC_NOTE = (" Classic engine only: the topological core does not use this setting (it prices vias, crossings and "
                "detours with the [factors] weights and stops at the [optimize] work caps).")
SETTINGS = [replace(s, doc=s.doc + CLASSIC_NOTE) if (s.section, s.key) in CLASSIC_ONLY else s for s in SETTINGS]

SETTINGS_BY_KEY = {(s.section, s.key): s for s in SETTINGS}

# Values that are planned but not implemented: reported as CFG_VALUE_BACKLOG, the default is used.
BACKLOG_VALUES = {("output", "geometry"): ("45deg",)}

# Engine Options fields the plugin deliberately does not expose, with the reason.
ENGINE_INTERNAL = {
    "stop_when_stable": "set per run by the plugin: false with the progress dialog (OK / finish_when_stable end it), true headless",
    "pinned": "set by the DRC loop from KiCad's own violations, not a user choice",
    "drc_spots": "set by the DRC loop from KiCad's own violations, not a user choice",
    "skip_self_check": "test-only switch that disables the engine's self check",
    "profile": "diagnostics only; timings must not enter the deterministic result",
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
