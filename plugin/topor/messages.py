"""Message catalogue (46 §1.1): every finding code has a default severity, a cause and a fix.

The fix names the KiCad place (``KiCad → ...`` menu path) or a ``topor.toml`` key (``section.key``, see
``schema.SETTINGS``); ``test_every_code_has_cause_and_fix`` enforces that for every code and that every code
emitted anywhere in ``plugin/`` and ``toporlib/`` is catalogued. ``render_troubleshoot`` produces the
per-code sections of ``docs/user/troubleshoot.md`` (``### <CODE>`` + cause + fix), so the guide cannot drift.

Findings built by the code carry a message with the concrete numbers / names; ``Msg.cause`` is the general
explanation and ``Msg.fix`` the general remedy (a finding may carry a more specific ``fix``).

Complexity: constant tables; ``render_troubleshoot`` O(#codes).
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Msg:
    severity: str  # default severity: "info" | "warning" | "error"
    cause: str
    fix: str


_CFG = "topor.toml (next to the .kicad_pro)"

MESSAGES: dict[str, Msg] = {
    # ---------------------------------------------------------------- configuration
    "CFG_CREATED": Msg("info",
        "There was no topor.toml in the project folder, so one was created with every setting at its default and a comment per key.",
        "Edit topor.toml in the project folder to change settings; the key reference is docs/user/configuration.md (e.g. run.timeout_s)."),
    "CFG_WRITE": Msg("warning",
        "topor.toml could not be created (folder not writable); the built-in defaults are used and no choice can be saved.",
        "Make the project folder writable (KiCad → File → Save As to a folder you own) or accept the built-in defaults."),
    "CFG_PARSE": Msg("warning",
        "topor.toml is not valid TOML, so the whole file is ignored and the built-in defaults are used.",
        "Fix the syntax at the reported line, or delete topor.toml to get a fresh commented one on the next run (run.timeout_s is the first key)."),
    "CFG_VERSION": Msg("warning",
        "topor.toml carries a version this plugin does not read; keys may be misread.",
        "Set version = 1 at the top of topor.toml and check the keys (e.g. run.timeout_s) against docs/user/configuration.md."),
    "CFG_UNKNOWN_KEY": Msg("warning",
        "topor.toml contains a section or key topor does not know (usually a typo); it is ignored.",
        "Correct the spelling against docs/user/configuration.md (e.g. run.timeout_s), or remove the entry."),
    "CFG_REMOVED_KEY": Msg("warning",
        "topor.toml sets a key that was removed (a setting the topological core never read, most of them from the classic router); it is ignored.",
        "Delete the line from topor.toml; the finding names what takes its place, and docs/user/configuration.md lists the current keys (e.g. optimize.runs)."),
    "CFG_VALUE": Msg("warning",
        "A value in topor.toml has the wrong type or is outside its allowed range; the default is used instead.",
        "Correct the value named in the finding (allowed range is in docs/user/configuration.md, e.g. run.timeout_s) or use a layer role signal|mixed|gnd|pwr|jumper in [layers]."),
    "CFG_VALUE_BACKLOG": Msg("warning",
        "The value is planned but not implemented in this version; the default is used.",
        "Use the default for now, e.g. output.geometry = \"arcs\" or \"segments\"."),
    # ---------------------------------------------------------------- environment / engine
    "ENGINE_VERSION": Msg("error",
        "The routing engine installed in the plugin environment is not the version this plugin was shipped with.",
        "Reinstall the plugin with tools/build.sh (docs/user/install.md) and restart KiCad (File → Quit, then start KiCad again)."),
    "KICAD_CLI_MISSING": Msg("error",
        "kicad-cli, which topor uses for DRC and the preview, was not found.",
        "Install KiCad 10 with its command-line tools (KiCad → Help → About KiCad → Copy Version Info shows the install), or set the KICAD_CLI environment variable."),
    "PCB_FORMAT_OLD": Msg("error",
        "The board file is older than the KiCad 10 format; topor edits the file text and would silently skip items written in the old format.",
        "Open the board in KiCad 10 and save it (KiCad → File → Save), then run topor again."),
    "KICAD_API_CONNECT": Msg("error",
        "topor could not connect to KiCad over its IPC API (the API server is off, KiCad is still starting, or no board is open in the PCB Editor). The board was not changed.",
        "KiCad → Preferences → Plugins → tick Enable KiCad API, restart the PCB Editor with the board open, then press the Topor button again."),
    "REPORT_DIR_UNWRITABLE": Msg("error",
        "The run's report folder could not be created or written (read-only folder, full disk, or a file where the folder should be), so topor stopped before reading the board. The board was not changed.",
        "Point output.report_dir in topor.toml at a folder you can write (e.g. \"topor-reports\" inside the project folder), or fix the permissions / free disk space for the folder named in the message."),
    "RUN_ALREADY_RUNNING": Msg("error",
        "Topor is already running for this project (another KiCad window or an earlier run that has not ended), so this start was cancelled before it touched the board. One run per project avoids two routers writing to the same board.",
        "Finish or cancel the other run first (its progress window shows OK / Cancel), then press the Topor button again. A crashed run frees the lock by itself. To allow parallel runs set run.single_instance = false in topor.toml."),
    "RUN_LOCK_UNAVAILABLE": Msg("warning",
        "The run lock file in the topor cache folder could not be created or locked (read-only folder or a file system without locks), so this run goes ahead without protection against a second start for the same project.",
        "Make the cache folder named in the message writable (or set TOPOR_REPORT_ROOT to a folder you own), or set run.single_instance = false in topor.toml to stop checking."),
    "KICAD_CLI_TIMEOUT": Msg("error",
        "A kicad-cli call (the DRC with zone refill, or the SVG export of the preview) did not finish within run.kicad_cli_timeout_s and was stopped. On a large board or a busy machine KiCad's own checks can take minutes. The board was not changed.",
        "Raise run.kicad_cli_timeout_s in topor.toml (seconds; 0 = no timeout), close other heavy programs, then run Topor again; if kicad-cli hangs even on a small board, run it by hand on a saved copy (docs/dev/debug.md)."),
    "BOARD_CHANGED_DURING_RUN": Msg("error",
        "The board was edited while Topor ran (items were added, deleted or moved), so the computed routes may no longer fit. Nothing was committed and the board is exactly as you left it.",
        "Undo your edits (KiCad → Edit → Undo) or run Topor again; do not edit the board while Topor runs."),
    "POST_COMMIT_STEP_FAILED": Msg("warning",
        "The routes were committed to the board, then a later step failed (zone refill, a KiCad cleanup / teardrop action, writing the report or showing the done notice); the routes stay on the board.",
        "Ctrl+Z (Edit → Undo) removes the routes; or keep them and do the failed step yourself: refill zones with B (KiCad → Edit → Fill All Zones), run KiCad → Tools → Cleanup Tracks & Vias, or check the report folder (output.report_dir)."),
    "ENGINE_BOARD_UNREADABLE": Msg("error",
        "The routing engine could not read this board: building its map of the board (one triangulation of every pad, via, rule area, copper graphic and the outline, on all layers at once) failed, so every connection would fail. Topor checks this right after reading the board and stopped before the pre-flight DRC and the settings dialog. The board was not changed. The message names the copper where it failed, with its layer, net and place.",
        "Look at the named copper (KiCad → Edit → Find by its footprint reference, or go to the coordinates): copper graphics, vias or rule areas whose outlines lie almost exactly on top of each other on different layers (e.g. a footprint's copper arcs and lines that meet end to end on two layers) are the usual cause. Move one by a few µm or merge the duplicates, save, and press the Topor button again. Please report the board (docs/dev/debug.md, TOPOR_DUMP_ENGINE_INPUT) so the engine can read it as it is."),
    "NOTHING_TO_DRAW": Msg("error",
        "Cancel / OK came before the router had kept any plan (before the initial routing of any run had ended: its topology search, layer assignment and rip-up rounds), so there was nothing to draw: no copper was written and the board was not changed. A Cancel later in the run draws the best plan kept so far.",
        "Route again and let the initial routing finish (the progress dialog names the step; on a large board it can take a minute or more), then press OK / Cancel; or let the time limit end the run (run.timeout_s, [run] timeout_mode = soft: the step that runs then finishes and its plan is drawn)."),
    "ENGINE_FAILED": Msg("error",
        "All or most connections failed with the same routing-engine error (e.g. the board's projection could not be built), so this is not a normal run that happened to route nothing. No copper was written; the board was not changed.",
        "Read the engine message in the details; for a projection error look at the named copper as for ENGINE_BOARD_UNREADABLE (KiCad → Edit → Find by its footprint reference). Run again; if it recurs, file a bug with log.txt and result.json from the report folder (docs/dev/debug.md)."),
    "INTERNAL": Msg("error",
        "An unexpected internal error stopped the run. The board was not changed.",
        "Run again; if it recurs, file a bug with log.txt and result.json from the report folder (Open report folder in the dialog) and a copy of the project folder saved with KiCad → File → Save (docs/dev/debug.md)."),
    # ---------------------------------------------------------------- stackup
    "LAYER_ROLES_NEEDED": Msg("error",
        "The board has a copper-layer count with no default stackup (not 2, 4, 6 or 8) and no role is set for some layer.",
        "Set each layer's role in topor.toml [layers], e.g. In1.Cu = \"gnd\", or set layer types in KiCad → File → Board Setup → Board Editor Layers."),
    "PLANE_NO_ZONE": Msg("warning",
        "A layer whose role is a plane (gnd, pwr, or a KiCad power layer) has no filled zone on a matching net, so plane connections cannot be served by the pour.",
        "Add a zone on that layer with a ground/power net covering the board outline (KiCad → Place → Add Filled Zone), then refill zones (KiCad → Edit → Fill All Zones, B), or change the role in topor.toml [layers]."),
    "LAYER_TYPES": Msg("warning",
        "A layer role set in topor.toml [layers] contradicts the layer type KiCad has for that layer (e.g. KiCad says power, topor.toml says signal); the topor.toml role wins.",
        "Make them agree: change the type in KiCad → File → Board Setup → Board Editor Layers, or the role in topor.toml [layers]."),
    # ---------------------------------------------------------------- vias
    "VIA_NOT_SET_UP": Msg("warning",
        "KiCad has no via sizes set up (no pre-defined via, and every net class via is KiCad's factory 0.8 / 0.4 mm), so every net with that via is routed with vias.fallback_diameter_mm / vias.fallback_drill_mm (0.8 / 0.4 mm by default) and, where space requires, smaller vias from topor's fab profile (a drill of 0.25, 0.3, 0.4 or 0.5 mm below the fallback drill, never below the board's minimum via drill and annular ring).",
        "Set the via size and hole of the net classes in KiCad → File → Board Setup → Design Rules → Net Classes (topor routes each net with its net class via), or set vias.fallback_diameter_mm and vias.fallback_drill_mm. A via in KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias also clears this finding: topor routes with the net class via and uses the smaller pre-defined vias where space requires."),
    "DRILL_OUTSIDE_FILTER": Msg("warning",
        "A via drill that the board uses is not in the fab drill filter, so the router could place a drill your fab filter forbids.",
        "Add the drill to vias.allowed_drills_mm (accepting the finding does it) or change the via sizes in KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias."),
    "VIA_IN_PAD_UNPROTECTED": Msg("warning",
        "vias.via_in_pad is on but KiCad's via protection is not filled and capped, so vias in pads risk solder wicking and voids.",
        "Set filling and capping in KiCad → File → Board Setup → Board Stackup → Board Finish → Via protection, or turn vias.via_in_pad off."),
    # ---------------------------------------------------------------- nets and pads
    "POWER_BY_NAME": Msg("info",
        "Nets still in the Default netclass whose name matches nets.power_pattern are treated as power nets.",
        "Put them in a power netclass (KiCad → File → Board Setup → Design Rules → Net Classes) or adjust nets.power_pattern."),
    "MARKER_IGNORED": Msg("warning",
        "An @N priority marker in a net name could not be used (out of range or on a net that is not routed); it is ignored.",
        "Use @1, @2 or @3 in the net name (KiCad → Schematic → net label) or set nets.priority."),
    "MARKER_CONFLICT": Msg("warning",
        "The @N marker in the net name disagrees with the topor_pN netclass or with nets.priority; the stronger source wins: the @N marker, then the topor_pN netclass, then nets.priority.",
        "Remove one source: edit the net label or KiCad → File → Board Setup → Design Rules → Net Classes, or nets.priority."),
    "MARKER_PAIR": Msg("warning",
        "The two halves of a differential pair carry different @N markers; the lower number is used for both.",
        "Give both nets the same marker (KiCad → Schematic → net label) or set nets.priority."),
    "PAD_OUTSIDE_BOARD": Msg("warning",
        "A pad's centre lies outside the Edge.Cuts outline, so no route can reach it.",
        "Move the footprint inside the outline (KiCad → Edit → Properties), or accept the finding to add the pad to nets.ignore_pads."),
    "DO_NOT_TOUCH": Msg("warning",
        "nets.do_not_touch lists nets whose copper is fixed; or one of its patterns matches no net on this board.",
        "Check the pattern spelling in nets.do_not_touch against the net names (KiCad → View → Panels → Net Inspector lists them)."),
    # ---------------------------------------------------------------- rules
    "TRACK_ANGLE_RULE": Msg("error",
        "A custom rule restricts track angle or segment length; topor routes curved traces and cannot honour it, so the result would violate your rule.",
        "Remove or scope the rule in KiCad → File → Board Setup → Design Rules → Custom Rules, or route that area by hand."),
    "CUSTOM_RULES": Msg("info",
        "The board's custom design rules (.kicad_dru) were read: each rule that restricts routing is applied as a per-net value, a board-wide edge / hole clearance, a clearance region (courtyard / rule area) or a pad clearance; the line names each rule and how it is used. Rules for silkscreen, courtyards, mask, zone fills and rules with severity ignore do not change the routing.",
        "Nothing to do; to change a rule edit it in KiCad → File → Board Setup → Design Rules → Custom Rules (docs/user/operate.md, Custom design rules)."),
    "CUSTOM_RULE_LIMITED": Msg("warning",
        "A custom rule could not be used exactly: its condition depends on the other item or uses a function topor does not evaluate (its value is then applied only where it is stricter, never where it is looser), its values differ per layer where topor has one width / clearance per net, a value below the fab profile's minimum was raised to it (a note starting with \"floor:\"; [rules] fab_profile = \"board\" uses custom values as given), or the constraint is not honoured at all (e.g. length, via_count).",
        "Check the named rules after routing with KiCad → Inspect → Design Rules Checker; simplify the condition (A.NetClass, A.NetName, A.intersectsCourtyard('REF'), A.intersectsArea('name'), A.Pad_Type) or route that part by hand. See docs/user/operate.md, Custom design rules."),
    "LOCKED_COPPER": Msg("info",
        "Locked tracks or vias exist; with run.respect_locked on they are fixed obstacles, with it off topor may rip or smooth them.",
        "Set run.respect_locked, or unlock items in KiCad → Edit → Unlock Selected."),
    "DANGLING_ENDS": Msg("info",
        "Existing tracks end at nothing (dead ends); topor continues them along their direction, keeps them as fixed points or drops them (search.dangling_ends).",
        "Set search.dangling_ends to continue, fixed or drop, or tidy them first in KiCad → Inspect → Design Rules Checker."),
    "PREEXISTING_DRC": Msg("info",
        "KiCad DRC already reports violations or unconnected items on the board before routing; they are listed as pre-existing.",
        "Nothing to do, or fix them first (KiCad → Inspect → Design Rules Checker); run.fail_on_preexisting = true turns this into an error."),
    "SCOPE_EMPTY": Msg("warning",
        "There is no connection to route in the chosen scope.",
        "Select other footprints in the PCB editor, change run.scope_mode, or check that nets are assigned (KiCad → Tools → Update PCB from Schematic)."),
    "NON_DEFAULTS": Msg("info",
        "Some topor.toml settings differ from the defaults; listed so a result can be reproduced.",
        "Nothing to do; revert a key in topor.toml (e.g. run.timeout_s) to its default to remove it from this list."),
    "DEFAULTS_USED": Msg("info",
        "Some values were not set in KiCad, so topor's built-in defaults were used (e.g. the default track width for nets without a netclass, existing copper or setting).",
        "Set the values in KiCad → File → Board Setup → Design Rules → Net Classes / Pre-defined Sizes."),
    "GROUPS_UNAVAILABLE": Msg("info",
        "The board contains groups; the KiCad API does not expose group membership, so grouped items are treated one by one and new tracks are not grouped.",
        "Select the footprints themselves, or ungroup first (KiCad → Edit → Ungroup)."),
    # ---------------------------------------------------------------- run
    "NOTHING_ROUTED": Msg("warning",
        "The router produced no copper.",
        "See the connection reasons in the report; relax a setting (run.timeout_s, optimize.rounds) or route by hand."),
    "PREVIEW_FAILED": Msg("warning",
        "The preview image could not be rendered; the DRC numbers in the summary are still valid.",
        "Check kicad-cli in KiCad → Help → About KiCad and the log in the report folder."),
    # ---------------------------------------------------------------- after routing (postrun.py) and pre-flight
    "DIFF_PAIRS_SKIPPED": Msg("warning",
        "Differential pairs (two nets whose names end in +/- or P/N, KiCad's rule) are not routed because diff-pair routing is switched off ([diffpair] enabled = false), or their pair connection could not be routed; their connections stay unrouted.",
        "Set [diffpair] enabled = true in topor.toml to route them as coupled pairs, or route them by hand with KiCad's differential-pair router (KiCad → Route → Differential Pair, hotkey 6). To keep topor off them completely, list them in nets.do_not_touch."),
    "DIFF_PAIRS": Msg("info",
        "Differential pairs were routed as coupled pairs: two tracks at the pair gap from a fan-out stub at each pad pair. Each line gives one pair connection: the lengths of P and N, their skew, the share coupled at the gap, the uncoupled length at each end and the vias. KiCad's DRC checks pairs only against custom rules (diff_pair_gap, diff_pair_uncoupled, skew); without them these numbers are information.",
        "Check the pairs in the preview. To make KiCad's DRC hold them to limits, add custom rules in KiCad → File → Board Setup → Design Rules → Custom Rules (diff_pair_uncoupled, skew within_diff_pairs); topor then prices uncoupled length and skew more steeply past those limits. Tune skew by hand with KiCad's skew tuner (topor does not meander yet)."),
    "UNROUTED": Msg("warning",
        "Some connections could not be finished; the report's Connections table names each one with its reason (no room for a via, no room at the clearance when the trace was drawn, ...). The open connections are drawn blue in preview.png.",
        "Route the remaining connections by hand (KiCad → Inspect → Design Rules Checker lists them as unconnected items), or make room: smaller vias in KiCad → File → Board Setup → Design Rules → Net Classes, fewer obstacles, or another layer; then run topor again with run.existing_copper = \"fixed\" to keep the result."),
    "CONNECTION_WIDTH": Msg("warning",
        "A trace would run so close past a pad or via of its own net that the copper between them gets thinner than KiCad's minimum connection width, so the trace was not drawn.",
        "Route these connections by hand, or check the minimum in KiCad → File → Board Setup → Design Rules → Constraints → Minimum connection width."),
    "POUR_ISLANDS": Msg("warning",
        "The new copper cuts pieces of a copper pour off from the rest of the pour, and topor found no place for a stitching via or pad stub to join them back; after a zone refill KiCad either removes those pieces or lists them as unconnected.",
        "Refill the zones (KiCad → Edit → Fill All Zones, B), look at the listed places and add a via by hand (KiCad → Place → Via) or move a trace; planes.stitch_islands controls topor's own stitching."),
    "POUR_UNJOINED": Msg("warning",
        "KiCad's DRC of the routed copy, after a zone refill, reports pieces of a copper pour cut off from the rest of the pour, or pads / vias that the pour no longer reaches. In preview.png such a pad has a blue ring; a cut-off piece is only counted in the legend (KiCad gives no position for it).",
        "Refill the zones (KiCad → Edit → Fill All Zones, B), run KiCad → Inspect → Design Rules Checker to see each one, then add a stitching via (KiCad → Place → Via) or move the trace that cuts the pour; planes.stitch_islands lets topor stitch what it can."),
    "POUR_STITCHED": Msg("info",
        "The new copper cut pieces off a copper pour; topor joined them back with stitching vias inside the pieces or short stubs from their pads (each one is listed).",
        "Nothing to do; check them after a zone refill (KiCad → Edit → Fill All Zones, B). planes.stitch_islands = false only reports the pieces."),
    "PLANE_PAD_SIGNAL": Msg("info",
        "A pad of a plane net (a net whose pour is on a plane layer) had no room for a short stub and via into its pour, so it is joined by an ordinary trace instead: to the pour's nearest filled copper (planes.terminals, the default), or to another pad of its net where the pour has no filled copper to end on.",
        "Nothing to do if the trace is acceptable; otherwise make room next to the pad or allow vias in pads (vias.via_in_pad)."),
    "PLANE_VIA_OUTSIDE_POUR": Msg("warning",
        "A via that should drop a pad onto its plane ended outside the filled copper of that plane, so it does not connect there.",
        "Refill the zones (KiCad → Edit → Fill All Zones, B) and check the via; move it into the pour by hand or enlarge the zone (KiCad → zone properties)."),
    "VIA_SQUEEZE": Msg("warning",
        "Some vias stand in a narrow pass: less room than half a via diameter is left next to them (the via_squeeze social factor). They pass KiCad's DRC but leave little room for later edits.",
        "Accept them, or raise the price of narrow passes with factors.weight via_squeeze (topor.toml [factors] weight = { via_squeeze = 10 }) and route again; factors.report via_squeeze = \"off\" hides the list."),
    "NECKED": Msg("info",
        "Some traces leave their pad narrower than the net's track width (necking) because the full width does not fit out of the pad; the rest of the trace keeps its width.",
        "Accept them, or forbid it with search.neck_to_pad = false; the narrowest allowed width is KiCad → File → Board Setup → Design Rules → Constraints → Minimum track width."),
    "IGNORED_RULE": Msg("warning",
        "Some copper breaks a KiCad check whose severity is set to ignore; topor prices such places (the ignored_rule social factor) instead of forbidding them, as KiCad's own DRC would not complain.",
        "Set the check's severity in KiCad → File → Board Setup → Design Rules → Violation Severity to error to make topor keep away from it, or raise factors.weight ignored_rule."),
    "VIOLATIONS": Msg("warning",
        "Never drop: some connections had no legal geometry when drawn (a gap too narrow, no free site for a via, a crossing that cannot be layered, a branch meeting its trace on another layer); topor kept them and wrote them on their best geometry, so the copper breaks a rule there. Each violation is listed with its place, type (clearance, short, hole_clearance, edge_clearance, connection_width) and cause; KiCad's DRC shows the same places. The summary groups them by origin: at a pad exit (a neighbour pin of the connection's own part: the trace does not get out of its pad at full width; pad exits and necking are planned in a later version), from a via without a legal site, at a crossing with another net, against other routed copper in a narrow passage, or against other pads, holes or the board edge.",
        "Fix each listed place by hand (move the trace or via, or a part), or give the router more room or time (run.timeout_s, [optimize] runs) and route again. [optimize] allow_violations = false writes legal copper only (those connections stay unrouted)."),
    "TIME_BUDGET_HIT": Msg("warning",
        "The time limit (or a Cancel) stopped the topological core before its routes were stable ([optimize] stable_window / stable_gain_pct); the result is the best so far and may differ between runs and machines. The stop reason says which clock fired: timeout_wall (seconds on the clock) or timeout_cpu (CPU seconds summed over all threads, run.timeout_clock = cpu).",
        "Raise run.timeout_s (0 = no limit; Cancel always works; run.timeout_clock chooses wall or cpu seconds) and route again, or add time while the router runs (progress dialog: +5 min / +15 min or a new Time limit), or lower [optimize] runs (best of N runs share the threads); wait for \"Routes stable\" and press OK for a reproducible result."),
    "TIME_OVERRUN_CAP": Msg("warning",
        "The soft time limit (run.timeout_mode = soft) let the step that ran go on past run.timeout_s, until the overrun cap run.timeout_overrun_s (on the same clock); then the run stopped as with the hard limit: the step that ran was cut and the plan that existed was drawn (to its end, unless the drawing itself was the step that ran). A cut initial routing or drawing leaves connections unrouted, a cut TopoR loop cycle is not kept. By default the cap is equal to run.timeout_s in headless runs (harness, MCP, scripts: at most twice the limit) and off in the KiCad dialog, where Cancel stops a run.",
        "Raise run.timeout_overrun_s (seconds past the limit; 0 or \"none\" = no cap) or run.timeout_s and route again, or lower [optimize] runs; in the KiCad dialog leave the cap off (auto) and press Cancel when the overrun is too long."),
    "DRC_NEW_VIOLATIONS": Msg("warning",
        "KiCad's DRC of the routed copy finds rule violations in the new copper that the board did not have before (clearance, short, hole clearance, edge clearance, ...). The finding counts them by type and names the first places with their nets and position. The result can still be accepted: the violations then stay on the board until you fix them.",
        "After Accept, run KiCad → Inspect → Design Rules Checker to see each one; fix them by hand or route again (Reject, then give the router more room or time: run.timeout_s, [optimize] runs)."),
    "RULES_MISMATCH": Msg("error",
        "Self-check after routing: KiCad's DRC requires a larger clearance between two nets than the clearance topor gave the router for them. topor read the design rules wrong, so the whole result was routed to the wrong rule; nothing is applied to the board. The finding names the nets and both values.",
        "Please report this at https://github.com/ranzbak/ranz-topor/issues with the report folder (report.md, drc.json, log.txt); meanwhile check the net classes and custom rules in KiCad → File → Board Setup → Design Rules."),
    "KEEPOUT_PAD": Msg("warning",
        "A pad of a net that needs routing lies inside a rule area that forbids tracks on every copper layer of the pad, so no trace can reach it.",
        "Move the pad's footprint or change the rule area in KiCad (select the rule area → Properties: untick Tracks), or route that connection by hand."),
    "CLEANUP_SKIPPED_PARTIAL": Msg("info",
        "KiCad cleanup / teardrops were skipped because the result has partial routes.",
        "Finish the partial connections by hand, then run KiCad → Tools → Cleanup Tracks & Vias yourself (run.kicad_cleanup)."),
}


def fix_for(code: str) -> str:
    """Catalogue fix of `code`, or "" when unknown. O(1)."""
    m = MESSAGES.get(code)
    return m.fix if m else ""


def render_troubleshoot() -> str:
    """The per-code part of docs/user/troubleshoot.md: ``### CODE``, severity, cause, fix. O(#codes)."""
    out = []
    for code in sorted(MESSAGES):
        m = MESSAGES[code]
        out.append(f"### {code}\n\n*Severity: {m.severity}.*\n\n**Cause.** {m.cause}\n\n**Fix.** {m.fix}\n")
    return "\n".join(out)
