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
    "planes": "Signal copper on plane layers (layers whose role is gnd / pwr, not routed).",
    "layers": "Stackup roles per copper layer (F.Cu = \"signal\", In1.Cu = \"gnd\", ...).",
    "search": "Path search tuning.",
    "optimize": "Topological core: how a run iterates (whole-route cycles with a fluctuation schedule until the routes are stable, like TopoR; best of N seeded runs), its safety caps (work caps, so a run is reproducible), and which result it keeps.",
    "drawing": "Topological core: how the routed topology is drawn as copper (drawing style and its prices).",
    "diffpair": "Differential pairs (topological core): which nets pair up (KiCad's own name rule), their width, gap and via gap (from KiCad's net classes and custom rules unless set here), and how the pair is drawn. A pair is planned as one wide wire and drawn as two coupled tracks (docs/dev/kicad-diff-pairs.md).",
    "output": "Result geometry and report location.",
    "capacity": "Topological core: the per-layer cut test (inflated sketch; docs/dev/inflated-sketch.md). A cut is a straight segment between two pieces of copper on one layer; the traces that must cross it need their widths and clearances along it. These settings shape that test. By default it only measures (cut_report); model = \"cuts\" makes it the price of the layer assignment (P3). The drawing does not use it.",
    "expert": "Settings of the harness's DRC loop; the plugin and the topological core do not read them.",
    "factors": "Price of what a route does (social factors), in mm of copper per unit: weight, scope and report level per factor name (docs/dev/factors.md). Factor code is trusted code: only add factor files you trust.",
}

SETTINGS: list[Setting] = [
    # ---- [run]
    _s("run", "timeout_s", "float", 600, "Limit on router time (seconds on the clock chosen by timeout_clock: wall = elapsed time, cpu = CPU seconds summed over all threads): all routing passes of the run together stop after this long. The topological core ends at \"Routes stable\" (headless), at your OK, or by itself after it (finish_when_stable), so this limit is only an emergency stop: when it fires before the routes are stable the report warns (TIME_BUDGET_HIT) that the result may differ between runs. Measured router times with the default best of 5 runs (4 threads, two boards at once, M-B0): eprom-to-kernal 2 s, wiccon-2023 about 25 s, dcsgonebadge about 450 s (320-710), qmtech about 1000 s (680-1300); a single run ([optimize] runs = 1) takes roughly a quarter to a half of that. Runs on large boards (dcsgonebadge, qmtech with runs = 5) may hit this limit; they keep the best result so far and the report warns (TIME_BUDGET_HIT). The pre-flight DRC, the DRC rounds and the kicad-cli refill / preview exports are not counted. 0 or \"none\" disables it (Cancel always works and keeps the best result so far).",
       unit="s", range=(0, 86400), engine="wall_clock_s"),
    _s("run", "timeout_clock", "str", "wall", "Which clock timeout_s counts. wall = seconds on the clock, what you wait for (the default). cpu = CPU seconds of router work, summed over all threads of the run, so a 4-thread run uses its limit up 4 times as fast as the wall clock: the limit then means the same amount of work on a busy machine as on an idle one and with any thread count, which makes it hard to buy a better result by adding threads, and is what benchmark comparisons use (harness/score_runs.py: cpu, 2400 s = the old 600 s wall x 4 threads). Caveat: on a laptop with two hardware threads per core and turbo clocks, a CPU-second under heavy load still does somewhat less work than on an idle machine. When the limit fires the best result so far is kept and the report says which clock (stop reason timeout_wall / timeout_cpu, TIME_BUDGET_HIT). The CPU time spent is shown in the progress dialog and the report either way.",
       choices=("wall", "cpu")),
    _s("run", "stop_after", "str", "", "Measurement: plan = end every run of the topological core after the plan of its first cycle (projection, topology, layers, vias), before any copper is drawn. The run writes no copper; result.json gets the plan metrics (a warning line starting with \"plan metrics:\") and, with [capacity] cut_report, the cut report of that plan. For comparing planners on a board or a crop (harness/compare.py --plan-only). Empty = the full run (default).",
       choices=("", "plan"), engine="stop_after"),
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
    _s("run", "single_instance", "bool", True, "Run lock: topor cannot be started twice for the same KiCad project. A second start shows a message (who started when) and ends at pre-flight; the lock is released when the run ends, also after an error or a crash. The lock file is run-<hash>.lock in the report root (not in the project folder). false = no lock."),
    _s("run", "kicad_cleanup", "bool", False, "After Accept, open KiCad's own Cleanup Tracks and Vias dialog (you confirm it there)."),
    _s("run", "kicad_teardrops", "bool", False, "After Accept, open KiCad's teardrop dialog (you confirm it there)."),
    _s("run", "kicad_cli_timeout_s", "float", 120, "Limit for each kicad-cli call (DRC with zone refill, preview export): a call that runs longer is killed and the run ends with KICAD_CLI_TIMEOUT, board unchanged. Raise it for very large boards. 0 disables the limit (Cancel still works).",
       unit="s", range=(0, 86400)),
    _s("run", "commit_fills", "bool", True, "Accept also writes the zone fills that KiCad's own filler made for the routed result (kicad-cli, the refill of the preview's DRC) into the zones, in the same undo step as the copper. The board then comes back with pours that hold the new traces and vias, so KiCad's Edit → Cleanup Tracks & Vias (which judges a stub into a pour, or a via that reaches a plane, by the fill it finds) keeps them. false = the zones keep the fill they had before the run until you refill (B); the cleanup may then remove plane stubs and plane vias.",
       kicad="KiCad → Edit → Fill All Zones (B); Edit → Cleanup Tracks & Vias"),
    _s("run", "refill_after_accept", "bool", False, "After Accept, refill all zones from topor (a second undo step in KiCad). false = Accept is exactly one undo step; press B (Edit → Fill All Zones) yourself.",
       kicad="KiCad → Edit → Fill All Zones (B)"),
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
    _s("planes", "signals", "str", "costly", "Topological core: whether signal copper may use plane layers. costly = yes, at the price of the plane_signal factor (10 per mm on top of the length) plus plane_cut per cut of the plane, so a plane carries a signal only where the signal layers cannot; forbidden = never, plane layers are left to their pour. The run report lists the signal length on plane layers and the cuts.",
       choices=("costly", "forbidden"), kicad="KiCad → File → Board Setup → Board Editor Layers (layer roles)", engine="plane_signals"),
    _s("planes", "stitch_islands", "bool", True, "Topological core: after routing, the refilled pours are estimated (the fill of the ripped board, minus the routed copper of other nets grown by the clearance, minus features thinner than the zone's minimum width; islands kept or dropped as the zone's island removal setting says). A piece of a pour net's copper that the routed copper cut off from the rest of its pour (an island holding a pad or via of the net, or a pad whose thermal spokes are all cut) gets a through via inside that island near its pad, joining it to the pour on another layer where that pour is solid, or a short stub from the pad; the via sites prefer room (the via_squeeze factor). The run report lists every piece cut off and every stitch. false = the pieces are only reported; KiCad's DRC then lists them as missing connections.",
       kicad="KiCad → zone properties (Clearance, Minimum width, Remove islands)", engine="stitch_islands"),
    _s("planes", "terminals", "bool", True, "Topological core: a copper pour is a terminal of its net like a pad, one that may carry vias (also with via_in_pad off). A pad joined to its net's pour may end on the net's copper already on the pour (another pad's plane via, an existing through via standing in the fill) instead of a via of its own; such a shared via moves to where the stubs of its pads are shortest together, and costs extra only where its pads' trace widths together exceed pi x drill. A pad too far from the pour for a stub (or with no clean stub site) is wired to the pour's nearest filled copper, at least the zone's minimum width plus the clearance inside the fill KiCad filled before the run, instead of to another pad of the net. The run report lists the shared vias and the pads wired to their pour. false = one via per plane pad, and a pad no stub serves is joined to another pad of its net.",
       kicad="KiCad → zone properties (net, layers, Minimum width); KiCad → Edit → Fill All Zones (B) before the run", engine="plane_terminals"),
    _s("vias", "via_in_pad", "bool", False, "Allow vias inside the net's own SMD pads (two extra via sites per pad, on its long axis); the run report lists every via placed in a pad. Pre-flight warns when KiCad's via protection is not filled and capped.",
       kicad="KiCad → File → Board Setup → Board Stackup → Board Finish → Via protection", engine="via_in_pad"),
    _s("vias", "plane_vias", "bool", True, "A pad whose net's pour lies only on plane layers (role gnd / pwr, not routed) gets a through via onto the plane at the nearest via site within plane_via_max_mm of the pad (inside the pad when via_in_pad is on); false reports such pads as 'terminal not on any routable layer'.",
       kicad="KiCad → File → Board Setup → Board Editor Layers (layer roles)", engine="plane_vias"),
    _s("vias", "allowed_drills_mm", "list", [], "Optional fab filter: of the smaller alternative vias (KiCad's pre-defined vias and the fab profile) only these drill diameters may be used; empty = no filter. A net class via is always used as set in KiCad (the pre-flight warns when its drill is not listed).",
       unit="mm", elem="float", engine="allowed_drills_nm"),
    _s("vias", "fallback_drill_mm", "float", 0.4, "Drill used when KiCad has no via sizes set up (no pre-defined via and every net class at KiCad's factory 0.8 / 0.4 mm). The plugin writes it into the engine rules; the engine's via rule (engine/src/vias.rs) is the single place that applies it.", unit="mm", range=(0.05, 10),
       kicad="KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias"),
    _s("vias", "fallback_diameter_mm", "float", 0.8, "Via diameter used when KiCad has no via sizes set up.", unit="mm", range=(0.1, 20),
       kicad="KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias"),
    # ---- [search]
    _s("search", "detour_factor", "float", 2.0, "A connection longer than this multiple of its straight pad-to-pad distance is treated as a detour: the topological core searches it again with cheaper crossings (the layer assignment then resolves a crossing with a via) instead of accepting a wandering path; 0 = off.",
       range=(0.0, 100), engine="detour_factor"),
    _s("search", "net_tree", "str", "free", "Topological core: how a net's connections form its tree. \"free\" = the tree follows the routing: each connection of the net joins the two parts of the net's routed copper its pads lie in, at whichever of their pads, or at a branch point (T-junction) on one of their routed traces, gives the cheapest path; a branch ends on its trunk's trace, one layer through the branch point (a via goes on the branch). \"fixed\" = pad to pad as planned before routing (the shortest straight-line tree), no branch points.",
       choices=["free", "fixed"], engine="net_tree"),
    _s("search", "steiner", "bool", False, "Topological core: free Steiner points on a 2 mm grid in the open areas of the board, as extra vertices of the projection (the triangulation the topology is planned on) and as candidate branch points: a trace of a multi-pin net may then split at a free point on another trace of its net (a Y) instead of joining at a pad. false (default) = the projection is built from the fixed items only (pads, vias, keepouts, copper graphics, the board outline), once per run, as in TopoR; a net's tree joins pad to pad or through a pad already on a trace of its net (both the pad-to-pad chain and the Y are correct routing; the Y can be shorter, the grid makes the projection larger and the run slower). Only with net_tree = \"free\" does the grid add branch points.",
       engine="steiner"),
    _s("search", "length", "str", "even", "Topological core: how the path search prices a trace's length while it plans the topology. \"taut\" = the shortest path through the trace's corridor of triangles that keeps the clearances: to the pads and the board edge at the ends of every crossed edge of the triangulation and to the traces already crossing it there (TopoR's initial routing takes \"the shortest path on the triangulation with the clearances needed\", Polubasov 2010); this works the same with and without the Steiner grid (steiner). \"even\" (default, the behaviour before 4.3) = between evenly spaced points on each crossed edge: a good estimate only on short edges, so without the Steiner grid (long edges) it overrates straight paths and plans detours (measured 4.2: wiccon-2023 GPIO0 4.45 x its straight length).",
       choices=["even", "taut"], engine="search_length"),
    _s("search", "crossing_vias", "str", "crossing", "Topological core: how the search prices a crossing of two nets. \"crossing\" (default) = the crossing factor ([factors] crossing, 20 mm: TopoR takes a 13-20 mm detour rather than a via pair) stands for the layer change the layer assignment makes; only a crossing that no layer assignment can separate (both traces held to the same single layer where they cross, e.g. over another net's pad on the other side) is priced as a violation (the overflow factor). \"via\" = also price, with the via factor, the via that a crossing of two traces held to the same layer implies, and keep the trace on the other layer after it (measured D6b: on qmtech 16 fewer connections, more vias; on dcsgonebadge the same).",
       choices=["crossing", "via"], engine="crossing_vias"),
    _s("search", "branch_relocate", "bool", False, "Topological core, net_tree = \"free\": the plan moves each branch point (T junction) of a net to where its three arms are shortest together: the Steiner (Fermat) point, where they meet at 120 degrees, when every angle of the three arm ends is below 120 degrees; when the two arms at a pad meet at less than 120 degrees the junction is pulled out of the pad to that point; when an angle at a pad is 120 degrees or more the junction goes into that pad (Popov & Popov 2013; TopoR FreeStyle \"move T-points\"). With [draw] style = \"octilinear\" the point is the one of least octilinear length instead (the legs run at 0 / 45 / 90 degrees, where the 120 degree point is not the shortest). A move is kept only where the new arms keep their clearances.",
       engine="branch_relocate"),
    _s("search", "junction_pad_mm", "float", 0.0, "Topological core, net_tree = \"free\": a branch point on a trace closer than this to a pad of the copper it joins is not used; the pad itself is the junction (the trace runs through the pad centre) instead of a T next to it. 0 = no such distance (default): the junction_angle and branch factors decide between a T next to a pad and the pad itself on price.",
       unit="mm", range=(0, 20), engine="junction_pad_mm"),
    _s("search", "fuse_branches", "bool", True, "Topological core, net_tree = \"free\": after the drawing, a trace that leaves a branch point or a pad right beside another trace of its net leaves that trace's copper where the two part instead: one copper and a T there, no two parallel traces (TopoR moves T-points the same way in its post-optimisation). Done only where it is shorter, the junction_angle factor included, and the new track keeps every clearance.",
       engine="fuse_branches"),
    _s("search", "margin_um", "float", 0, "Safety margin added to every clearance during search and drawing; 0 = exactly the design rule (KiCad DRC accepts copper at exactly the clearance, and the routed copper is checked with the exact rule). Raise it only for headroom against later manual edits.", unit="µm", range=(0, 1000), engine="margin_nm"),
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
    _s("vias", "size", "str", "net_class", "Topological core: which via is put at a layer change, in the plan (room, via sites) and in the drawing. net_class = the net class via first, then the smaller pre-defined vias (Board Setup -> Pre-defined Sizes) and fab-profile vias where it does not fit (the behaviour before 5.4). board_min = the smallest via your Board Setup minimums allow (the supplier's capability: minimum via diameter, minimum through-hole drill, minimum annular ring) that still carries the trace's current: drill = the smallest with pi x drill >= the trace width (a diff pair: its diff pair width), rounded up to drill_grid_mm; diameter = the larger of the minimum via diameter and drill + 2 x minimum annular ring. Example (minimums 0.45 / 0.25 / 0.10): a 0.25 mm trace gets 0.45 / 0.25, a 1.0 mm trace 0.52 / 0.32, a 2.0 mm trace 0.84 / 0.64, never the net class via.",
       choices=["net_class", "board_min"], engine="via_size",
       kicad="KiCad → File → Board Setup → Design Rules → Constraints (Minimum via diameter, Minimum through hole, Minimum annular width)"),
    _s("vias", "drill_grid_mm", "float", 0.01, "Topological core, size = board_min: the via drill is rounded up to a multiple of this. Many fabs offer drills in 0.05 mm steps; set 0.05 to get those (a 1.0 mm trace then gets a 0.35 mm drill instead of 0.32). 0 = no rounding.",
       unit="mm", range=(0.0, 1.0), engine="via_drill_grid_nm"),
    _s("vias", "pair_min", "bool", False, "Topological core: local via-pair minimisation in the plan (Luzin, Popov & Popov 2013). A trace that dives to another layer and comes back (two vias) is laid again on its own layer round what it dived under, when the way round costs less than the two vias (length + via price); two vias joined by a short stretch whose layer could be changed are merged (the stretch moves to the layer of its neighbours). Priced by the plan's own price, never forced. Runs in every cycle of the TopoR loop (optimize.loop = \"topor\") and once before the drawing otherwise.",
       engine="via_pair_min"),
    _s("vias", "pair_max_mm", "float", 10.0, "Topological core, pair_min = true: only via pairs joined by a stretch at most this long are looked at (Luzin, Popov & Popov 2013: \"shorter than a given length, for example 10 mm\").",
       unit="mm", range=(0.0, 1000.0), engine="via_pair_max_mm"),
    _s("vias", "relocate", "str", "auto", "Topological core: \"auto\" (default) = on with [optimize] loop = \"topor\" and [search] length = \"taut\", off otherwise (measured on wiccon-2023, TopoR loop: with relocate and between_crossings the vias get their room; size = board_min, pair_min and [search] branch_relocate stay off, they added clearance errors and a detour there); \"on\" / \"off\" (or true / false) force it. When on, the plan moves every via along its trace, inside the stretch where it may sit (between the obstacles it must pass on each layer), to where the copper around it pushes equally from both sides (a force balance: equal clearance between two neighbours; TopoR FreeStyle \"shift vias\"). Its own pads push too (a via between its pad and a wall sits in the middle). The drawing takes that site first. A via is never moved where the plan predicts a violation the old site did not have. Needs [search] length = \"taut\" (the plan's path is then the wire's path; with \"even\" it is skipped with a warning).",
       choices=["auto", "on", "off"], engine="via_relocate"),
    _s("vias", "between_crossings", "str", "auto", "Topological core: \"auto\" (default) = on with [optimize] loop = \"topor\" and [search] length = \"taut\", off otherwise; \"on\" / \"off\" (or true / false) force it. When on, a via sits on the stretch of its trace strictly between the two places that fix its layers (crossings with other traces, junctions, one-layer areas, gaps too narrow on one layer), never in the triangle where its trace crosses another one (Polubasov 2001: a layer change lies between two crossings). The plan counts a layer change with no room between its two crossings as a via without a site, so layer assignment and the TopoR loop move it; the drawing searches only that stretch. Off = the via may also be planned in a crossing's own triangle, where the drawing cannot put it (measured: wiccon-2023, TopoR loop, the 3.3v trace J1.1 to U1.3 forced across other nets).",
       choices=["auto", "on", "off"], engine="via_between"),
    _s("vias", "reduce_after_drawing", "bool", True, "Topological core: after all copper is drawn, each connection with vias is tried again with one stretch between two vias (or between a via and a through-hole pad) moved onto the layer of its neighbouring stretch, which drops the vias between them; the change is kept where the redrawn trace passes every clearance check, else the old copper stays. Fewer vias at the cost of some runtime.",
       engine="via_reduce"),
    _s("search", "neck_max_mm", "float", 0.0, "Longest necked stub; 0 = twice the pad's long side.", unit="mm", range=(0, 100),
       engine="neck_max_nm"),
    _s("search", "dangling_ends", "str", "continue", "What to do with an existing track that ends at nothing (a hand-started trace): continue = the new copper leaves its end along the same direction (smooth, same width and layer); fixed = the end stays a fixed point and the new copper may leave at an angle; drop = remove that track and route from the pad.",
       choices=("continue", "fixed", "drop"), engine="dangling_ends", since=V5),
    # ---- [optimize]
    _s("optimize", "rounds", "int", 30, "Topological core: safety cap on whole-route cycles per run (the first one included). A run does not stop at a fixed count: like TopoR it keeps iterating (every cycle routes again with the passages the drawing found too small lowered and with freshly fluctuated weights, and offers its result variants) until its routes are stable (see stable_window), the user presses OK / Cancel, or run.timeout_s fires; this cap only ends a run that never settles. A work cap, not a time cap: the same board and settings give the same result on any machine, loaded or not, and with any thread count.",
       range=(1, 1000), engine="topo_rounds"),
    _s("optimize", "initial", "str", "per_cycle", "Topological core: when the initial routing (every connection laid one at a time: net priority tier first (1 = first), then widest, then shortest) runs. \"once\" = once per run, in its first cycle, as in TopoR: double crossings are removed after every trace (not once after all) and the traces that took part in a removal are laid anew, the new lay kept only when its price (length and crossings) is lower, and every trace is placed: a trace with no legal topological path still goes through, at the very high price of the violation factor ([factors] violation), and so does a crossing that no layer assignment can separate (a crossing on one layer); the later cycles start from the plan of the cycle before and improve it (rip-up and reroute, crossing rules) instead of laying everything again. A crossing on one layer that survives into the final copper is reported as a CRITICAL warning (traces X and Y cross on layer L), never as a dropped trace; so is a drawn trace whose copper overlaps a pad, via, track or no-tracks rule area of another net (trace X runs through ... on layer L). \"per_cycle\" (default, the behaviour before 4.3) = every cycle lays all traces again from scratch.",
       choices=["per_cycle", "once"], engine="initial"),
    _s("optimize", "loop", "str", "cycles", "Topological core: how a run iterates. \"cycles\" (default, the behaviour before 5.1) = whole-route cycles: every cycle plans the topology and draws it, and what the drawing found too narrow feeds into the next cycle's plan. \"topor\" = TopoR's loop (Polubasov 2010): the initial routing runs once (as initial = \"once\"), then the run iterates on the topology only, nothing drawn inside the loop: every cycle takes the traces out one at a time and lays them again where that is cheaper, each trace with its own oscillating weights; then the layers are assigned again (a random one of the layerings with the same number of vias), the traces whose vias rose are laid again, the label moves move stretches to the other layer (floating up) and the crossing rules run. Each cycle's plan is judged by its predicted violations (cuts too narrow for what passes them, including the gap between a pad and the board edge; crossings no layer assignment separates; a trace planned through a pad or other copper of another net), vias, length and shrinkages and kept in an archive of variants (as the drawn ones were); after loop_restart cycles without a better pick the loop goes on from an archived one picked at random (seeded, so the same settings give the same result). The loop ends when the plan is stable (loop_stable_window, stable_gain_pct), at rounds cycles, or at loop_time_share of the time left; the picked variant (by [optimize] pick: fewest errors, then fewest vias, then the shortest length; errors_weighted = fewest errors, then length + mm_per_via per via) is drawn once. An OK / Cancel during the loop ends the loop and still draws the pick. Set [search] length = \"taut\" with it (the loop compares lengths of plans).",
       choices=["cycles", "topor"], engine="opt_loop"),
    _s("optimize", "loop_restart", "int", 2, "Topological core, loop = \"topor\": cycles in which the picked variant did not get better after which the loop goes on from an archived variant picked at random (seeded; TopoR \"periodically stops working on the current variant and picks one from the table at random\", Polubasov 2010). 0 = never restart (the loop always goes on from where the last cycle ended).",
       range=(0, 1000), engine="loop_restart"),
    _s("optimize", "loop_stable_window", "int", 8, "Topological core, loop = \"topor\": the loop's plan is stable (and the loop ends) when its picked variant gained less than stable_gain_pct % over this many loop cycles. A loop cycle draws nothing and is cheap, so this window is longer than stable_window (the drawn cycles of loop = \"cycles\"). A work cap like rounds: the same settings give the same result on any machine.",
       range=(1, 1000), engine="loop_stable_window"),
    _s("optimize", "loop_via_start", "float", 0.5, "Topological core, loop = \"topor\": weight of the via factors (via, crossing: a crossing of two nets stands for the via the layer assignment makes) at the start of every cycle's sweep over the traces, relative to their [factors] weight. With loop_via_end above it, each cycle first shortens the traces and later removes vias (TopoR: \"at the start of a cycle more attention goes to the total length, at its end to the number of vias\", Luzin & Polubasov 2006). The weight changes smoothly from start to end; 1 and 1 = no emphasis.",
       range=(0.0, 10.0), engine="loop_via_start"),
    _s("optimize", "loop_via_end", "float", 1.5, "Topological core, loop = \"topor\": weight of the via factors at the end of every cycle's sweep (see loop_via_start); the traces whose vias rose in the layer assignment are laid again at this weight.",
       range=(0.0, 10.0), engine="loop_via_end"),
    _s("optimize", "loop_time_share", "float", 0.5, "Topological core, loop = \"topor\": the share of the time left (run.timeout_s) that the loop may use at most; the rest is kept for the one drawing. An emergency stop like the time limit (the loop normally ends when its plan is stable); when it fires the result says so.",
       range=(0.05, 1.0), engine="loop_time_share"),
    _s("optimize", "loop_crossing", "float", 0.25, "Topological core, loop = \"topor\": share of the crossing factor's weight ([factors] crossing) that a crossing of two nets keeps in the loop's reroutes. The loop lays each trace into the present layering: a crossing the trace can take on another layer than the crossed stretch costs this share only, a crossing that forces the trace to change layer costs the via (and the via back, where it has to return to its pad's layer) at the via weight on top (TopoR: before the optimisation every stretch gets a layer, and a crossing of a stretch with a via is penalised, Polubasov 2010). 1 = the full crossing price on top of the vias; 0 = only the vias.",
       range=(0.0, 1.0), engine="loop_crossing"),
    _s("optimize", "max_ripped", "int", 5000, "Topological core: most connections one whole-route round may take out and route again (negotiated rip-up-and-reroute, drawing feedback and the P4 moves together). A work cap (see rounds).",
       range=(0, 1000000), engine="max_ripped"),
    _s("optimize", "max_expansions", "int", 2000000, "Topological core: search cap per connection (expanded search nodes); the one retry of a connection that found no path uses four times this. A work cap (see rounds): a connection that needs more is reported as failed.",
       range=(1000, 1000000000), engine="max_expansions"),
    _s("optimize", "objective", "str", "length_vias", "Topological core: how the result variants are judged. Every whole-route round gives two variants (as drawn, and after the via reduction); the archive keeps every variant no other beats on all four TopoR criteria (errors = connections not routed, vias, total length, shrinkages = necked stretches). length_vias = fewest errors first, then by [optimize] pick (default: fewest vias, then the shortest length; errors_weighted: the least length + mm_per_via x vias); area = fewest errors first, then the least occupied area (a trace counts length x (width + clearance), a via its pad-plus-clearance disc on every layer). The run report lists the variant table and marks the pick; the picked variant goes to KiCad.",
       choices=("length_vias", "area"), engine="objective"),
    _s("optimize", "pick", "str", "errors_vias_length", "Topological core: how the final result is chosen among complete results (the archive's variants, and the seeded runs of runs > 1), for objective = length_vias. errors_vias_length (default) = strictly in this order: fewest errors (connections left open or with a rule violation in their copper, pour pieces cut off included; the count the engine has on the drawn copper), then fewest connections left open (a trace drawn with a violation can still be improved, a dropped one cannot), then fewest vias, then the shortest total trace length, then fewest shrinkages (necked stretches). errors_weighted = the order before 2026-11-10: fewest errors, then the least length + mm_per_via x vias (a step to fewer vias is taken while it costs at most mm_per_via of length per via removed), then fewest shrinkages. Only the final choice changes: the archive keeps the same variants, and the search itself (rip-up rounds, factors, mm_per_via as a price) is the same. objective = area ignores it. KiCad's own DRC runs once on the one result that is picked; it is not used to rank.",
       choices=("errors_vias_length", "errors_weighted"), engine="pick"),
    _s("optimize", "mm_per_via", "float", 25.0, "Via price in mm of trace per via (TopoR: good results at 25 mm/via): the price the rip-up-and-reroute uses to pick its best round and that the archive uses to prune variants when it is over its cap; also the final pick when pick = errors_weighted (objective = length_vias).",
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
    _s("output", "cleanup_safe", "bool", True, "The written copper is made stable under KiCad's Edit → Cleanup Tracks & Vias (every option on): no zero-length or duplicate segments, no segment lying wholly inside a pad of its own net (the copper next to it still ends in that pad), every T junction an exact vertex shared by three track ends (KiCad judges a short segment next to a T on the middle of a trace as unconnected at one end and removes it, then the rest of the trace), collinear segments merged. The run report has one line with the counts. false = the copper is written as drawn; KiCad's cleanup may then remove or merge parts of it.",
       kicad="KiCad → Edit → Cleanup Tracks & Vias", engine="cleanup_safe"),
    _s("output", "report_dir", "str", "", "Report folder: \"\" = ${TOPOR_REPORT_ROOT:-${XDG_CACHE_HOME:-~/.cache}/kicad/10.0/topor}/<project>/<YYYYmmdd-HHMMSS>/; relative = <project dir>/<report_dir>/<timestamp>/; absolute = <report_dir>/<timestamp>/."),
    # ---- [capacity]
    _s("capacity", "mixed_clearance", "str", "conservative", "Topological core, per-layer cut test: how two neighbouring traces of different nets with different clearances are spaced along a cut. conservative = every gap on the cut at the largest clearance of the traces crossing it (the cut test's own model, safe when net classes differ; costs room next to high-clearance nets); pairwise = the larger of the two neighbours' clearances, as KiCad's DRC checks it (more room, but the test is then no longer exact where clearances differ). Both values are written to the cut report; this one is the cut's need.",
       choices=("conservative", "pairwise"), engine="mixed_clearance",
       kicad="KiCad → File → Board Setup → Design Rules → Net Classes (Clearance column) and Custom Rules (clearance)"),
    _s("capacity", "cut_radius_mm", "float", 0.0, "Topological core, per-layer cut test: only cuts up to this long are checked (mm); 0 = off, every critical cut of the layer whatever its length (the exact test). Faster on large boards, but a squeeze between two pads further apart than this goes unseen, so the cut report then says routability is not proven.",
       unit="mm", range=(0, 1000), engine="cut_radius_mm"),
    _s("capacity", "model", "str", "edges", "Topological core: what the layer assignment (P3 capacity refinement) prices as over-full. edges = the room of each triangulation edge per layer (default, the tested path); cuts = the per-layer cut test (Leiserson-Maley critical cuts between two pieces of copper of one layer, drill to drill on inner layers): a cut whose crossing traces need more than its length even at their minimum track widths is over-full, priced graded and smooth by its overflow (overflow_softness); a cut that fits only with its traces necked below their netclass widths is a shrinkage, priced by the [factors] shrinkage weight per pitch of width reduction, and the plan necks those traces there (the drawing necks exactly there, the variants count it as a shrinkage), with the octagonal length for an octilinear drawing and the straight length for a smooth one ([drawing] style). With cuts, the first routing (P2) also takes each layer's room from that layer's copper: a passage next to a pad that is not on the layer reaches on to the next copper of the layer (the board edge, a pad on it), and a layer closed there by a rule area has no room, where the per-edge model saw unlimited room. The rip-up rounds (P4) are priced by the cuts too: the wires of an over-full cut are ripped up and routed again, every move is kept or undone by its change of the total price (the over-full price is a cost like any other, not a hard limit), and the best round is the one with the lowest objective plus over-full price. The drawing stays as it is. The plan metrics line then also counts the over-full cuts.",
       choices=("edges", "cuts"), engine="capacity_model"),
    _s("capacity", "overflow_softness", "float", 0.1, "Topological core, model = \"cuts\": how soft the over-full price of a cut is, in track pitches (width + clearance). The price per cut grows smoothly with the overflow u (in pitches): about 0 while the cut has more than a pitch of room, 0.57 at exactly full, 1 + u far over. Larger = the planner already pays for cuts that are nearly full and makes room early; smaller = closer to a sharp full/over-full step. Never a cutoff.",
       unit="pitches", range=(0.001, 10), engine="overflow_softness"),
    _s("capacity", "cut_vias", "bool", True, "Topological core, model = \"cuts\": the layer assignment may add a via when that removes cut overflow worth more than the via's price (the per-edge model only relabels, it adds no via). Every via is still priced by the via factor and checked for a site. false = relabel only, as the per-edge model.",
       engine="cut_vias", kicad="KiCad → File → Board Setup → Design Rules → Constraints (minimum via diameter, hole) and Net Classes (via size)"),
    _s("capacity", "cut_report", "str", "", "Debug: a directory. When set, every routing cycle writes its per-layer cut test there (run<k>/cycle<c>.json: the cut counts after the layer assignment and after every rip-up round, and for the plan handed to the drawing every cut that traces cross, with its room, its need and its slack). Slower; never changes the routing. Empty = off (default). harness/cutclass.py classifies a run's violations with it.",
       engine="cut_report"),
    # ---- [factors]
    _s("factors", "weight", "table", {}, "Factor name → weight (mm per unit, ≥ 0); absent = the factor's default. Factors and defaults: length 1 per mm (scaled by width + clearance), via 25 per via, crossing 20 per crossing of two nets, same_net_crossing 0, plane_signal 10 per mm on a plane layer, plane_cut 25 per cut of a plane, necking 30 per necked stretch, shrinkage 30 (unset: the necking weight) per track pitch (width + clearance) of width reduction a per-layer cut needs ([capacity] model = \"cuts\": a cut whose traces fit only when necked below their netclass width, down to the minimum track width of KiCad → File → Board Setup → Design Rules → Constraints or a custom track_width rule, is a shrinkage, graded from 0 where the netclass widths just fit; beyond the minimum widths the rest is an over-full cut; the plan necks those traces there and the drawing follows), overflow 50 per pass through an overfull cut, congestion 10 per pass per earlier over-full round of the cut, squeeze 5 per pass that leaves a cut less than a track pitch of slack between two other traces, via_squeeze 5 per passage-layer next to a via left with no slack (linear from one via diameter of slack, free, to none; the planner puts a via where the passages around it keep the most room), blocks_pad 30 per trace that takes the last free way out of another net's unrouted pad, ignored_rule 100 per violation of a KiCad check set to ignore, reroute 5 per earlier reroute of a connection that the P4 search moves again, branch 1 per branch point on a trace or through-pad (a reward: subtracted; net_tree = \"free\" lets a connection end on its net's routed copper there instead of a separate trace to the planned pad, and a separate path still wins when it is cheaper; 0 = only where the branch is cheaper on length alone), smd_through 0 per second trace ending on an SMD pad (assembly cost of a through-pad; off by default so it does not cancel branch), junction_angle 10 per mm of width + clearance per unit of psi(a) for a new trace that leaves a junction (a branch point, or a pad that already carries a trace of its net) at angle a to the nearest arm already there, psi(a) = (sec(60° - a/2) - 1) x sin 60° / sin(a/2) below 120° (0 from 120°, the Steiner optimum, up; 90° 0.04, 60° 0.27, 48° 0.5, 30° 1.4, 15° 4.3: shallow T splits and traces leaving a pad side by side get ever more expensive, with no cutoff), any_angle 15 per stretch of an octilinear drawing that is not at 0/45/90 degrees plus 2 per mm of it ([drawing] style, any_angle), acute_bend 50 per bend sharper than 90 degrees, up to twice that as the angle closes ([drawing] acute), planned_room 5 per mm of drawn trace inside the corridor the plan keeps for a neighbour drawn later, times d/p + (d/p)^2 (d = depth past that neighbour's planned path less the lane spacing, p = width + clearance; graded, no cutoff; the same weight prices a via the drawing places inside such a corridor, or a via pair that leaves the wires planned between its two vias less than they need), violation 1000 per rule the written copper breaks plus per 0.01 mm² of its depth x length, and a connection without copper as its width + clearance over its straight length (never drop, [optimize] allow_violations: very high, so the cycles work to remove it; the best result counts the connections with violations first); differential pairs ([diffpair]): dp_uncoupled 1 per mm of a pair connection not coupled at the gap plus 4 per mm past a diff_pair_uncoupled rule's maximum, dp_skew 10 per mm of length difference between the two tracks plus 4 per mm past a skew rule's maximum, dp_gap_deviation 5 per mm of parallel pair track at a gap off the target by more than the tolerance (weighted by deviation / (deviation + tolerance)), dp_via_unpaired 25 per via on one track without a partner via, dp_polarity_cross 1000 per pair connection whose two tracks swap sides (effectively prohibitive).",
       unit="mm", engine="factors.weight"),
    _s("factors", "scope", "table", {}, "Factor name → which wires the factor prices: \"all\" (default) or \"tier:N\" (wires of priority tier N only).",
       engine="factors.scope"),
    _s("factors", "report", "table", {}, "Factor name → \"off\", \"info\" or \"warning\": events of the factor become lines of that level in the run report. Defaults: necking, ignored_rule, via_squeeze, any_angle, acute_bend, violation, dp_via_unpaired and dp_polarity_cross warning (via_squeeze: one line per drawn via with less slack than half its diameter next to it, \"via in a narrow pass\"; violation: one line per rule the written copper breaks, with place, type and cause), the rest off.",
       engine="factors.report"),
    _s("expert", "drc_loop_cap", "int", 5, "Maximum route / DRC / re-route rounds. Not used by the topological core: it takes no DRC feedback, so the result is judged by one DRC round (a re-route would repeat it).", range=(1, 50), engine="drc_loop_cap"),
]

#: Settings removed in the CL cleanup (2026-11-09): the topological core never read them (most were settings of
#: the classic router, removed in the green field 2f1430a). An old topor.toml that still sets one gets the warning
#: CFG_REMOVED_KEY ("ignored, removed in ..."), never an error. (section, key) -> what replaced it, if anything.
REMOVED_IN = "the CL cleanup of 2026-11-09"
_CLASSIC = "a setting of the classic router; the topological core prices vias, crossings and detours with the [factors] weights and stops at the [optimize] work caps"
REMOVED = {
    **{k: _CLASSIC for k in (
        ("run", "detour_reference"), ("vias", "via_cost_mm"), ("vias", "max_per_connection"), ("vias", "reduction"),
        ("vias", "reduction_len_slack_pct"), ("vias", "plane_via_max_mm"), ("vias", "cost_by_priority"),
        ("vias", "cost_by_netclass"), ("vias", "via_shove"), ("vias", "plane_via_ring"), ("vias", "thermal_pad_vias"),
        ("vias", "plane_via_share"), ("vias", "plane_via_share_max_mm"), ("vias", "via_relaxation"),
        ("layers", "preferred_direction"), ("layers", "direction_cost"), ("layers", "pour_cut_cost_mm"),
        ("search", "zero_slack_retry"), ("search", "zero_slack_failed"), ("search", "strict_unfit_edges"),
        ("search", "pour_listed_via_goals"), ("search", "exit_stubs"),
        ("search", "pad_bridge_max_mm"),
        ("ripup", "growth"), ("ripup", "blame_vias"), ("ripup", "failed_pressure"), ("ripup", "rip_pass"),
        ("ripup", "max_per_pair"), ("iterations", "max"), ("iterations", "patience"), ("iterations", "min_gain_pct"),
        ("iterations", "continue_rounds"), ("expert", "hist_fail"), ("expert", "hist_block"),
        ("expert", "hist_overuse"), ("expert", "present_overuse"), ("expert", "present_growth"),
        ("expert", "zero_slack_candidates"), ("expert", "relax_iter_cap"), ("expert", "own_net_capacity"),
        ("expert", "wall_aware_capacity"), ("expert", "transit_pinch"), ("expert", "steiner_min_angle_deg"),
        ("expert", "steiner_len_slack"), ("expert", "steiner_ghost_factor"))},
    ("run", "threads"): "the engine's thread count comes from its thread pool (RAYON_NUM_THREADS), [optimize] runs sets how many runs share it",
    ("run", "partial_routes"): "the topological core always keeps every routed connection (never drop: [optimize] allow_violations)",
}

SETTINGS_BY_KEY = {(s.section, s.key): s for s in SETTINGS}

# Values that are planned but not implemented: reported as CFG_VALUE_BACKLOG, the default is used.
BACKLOG_VALUES = {("output", "geometry"): ("45deg",)}

# Engine Options fields the plugin deliberately does not expose, with the reason.
ENGINE_INTERNAL = {
    "stop_when_stable": "set per run by the plugin: false with the progress dialog (OK / finish_when_stable end it), true headless",
    "drc_spots": "set by the DRC loop from KiCad's own violations, not a user choice",
    "skip_self_check": "test-only switch that disables the engine's self check",
    "scope": "built per run from the KiCad selection and scope_mode, not stored",
    "net_priority": "built per run from @N markers, netclasses and [nets] priority",
    "exclude_nets": "built per run from [nets] do_not_touch (patterns expanded against the board's net names)",
    "ignore_pads": "passed through from [nets] ignore_pads (REF.PAD strings) by run.build_options",
    "pour_served": "passed through from [nets] pour_served (boolean entries only) by run.build_options",
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
