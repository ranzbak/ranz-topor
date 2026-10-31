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
    "ENGINE_UNRESPONSIVE": Msg("error",
        "After Cancel the routing engine did not stop within the grace period (it is stuck outside its stop checks), so the run was ended without a result. The board was not changed.",
        "Run again with a shorter run.timeout_s; if it recurs, file a bug with log.txt and result.json from the report folder (docs/dev/debug.md). If KiCad stays slow, save and restart it (KiCad → File → Exit)."),
    "POST_COMMIT_STEP_FAILED": Msg("warning",
        "The routes were committed to the board, then a later step failed (zone refill, a KiCad cleanup / teardrop action, writing the report or showing the done notice); the routes stay on the board.",
        "Ctrl+Z (Edit → Undo) removes the routes; or keep them and do the failed step yourself: refill zones with B (KiCad → Edit → Fill All Zones), run KiCad → Tools → Cleanup Tracks & Vias, or check the report folder (output.report_dir)."),
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
        "KiCad has no via sizes set up (factory defaults only), so topor falls back to its own via size.",
        "Set vias in KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias (or a net class), or set vias.fallback_diameter_mm and vias.fallback_drill_mm."),
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
        "Check the pattern spelling in nets.do_not_touch against the net names (KiCad → Inspect → Board Statistics → Nets shows them)."),
    # ---------------------------------------------------------------- rules
    "TRACK_ANGLE_RULE": Msg("error",
        "A custom rule restricts track angle or segment length; topor routes curved traces and cannot honour it, so the result would violate your rule.",
        "Remove or scope the rule in KiCad → File → Board Setup → Design Rules → Custom Rules, or route that area by hand."),
    "LOCKED_COPPER": Msg("info",
        "Locked tracks or vias exist; with run.respect_locked on they are fixed obstacles, with it off topor may rip or smooth them.",
        "Set run.respect_locked, or unlock items in KiCad → Edit → Unlock Selected."),
    "DANGLING_ENDS": Msg("info",
        "Existing tracks end at nothing (dead ends); topor either continues them or drops them (search.dangling_ends).",
        "Set search.dangling_ends to continue or drop, or tidy them first in KiCad → Inspect → Design Rules Checker."),
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
        "See the connection reasons in the report; relax a setting (run.timeout_s, vias.max_per_connection) or route by hand."),
    "PREVIEW_FAILED": Msg("warning",
        "The preview image could not be rendered; the DRC numbers in the summary are still valid.",
        "Check kicad-cli in KiCad → Help → About KiCad and the log in the report folder."),
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
