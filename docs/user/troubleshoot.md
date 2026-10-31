# Troubleshooting

Every message topor shows has a code, a cause and a fix. Each code has a section below (`### CODE`); the
text is generated from `plugin/topor/messages.py`, so it cannot drift (a test fails if a code is missing
or the text differs). Run `.venv/bin/python -c "import sys; sys.path.insert(0, 'plugin'); from topor import messages; print(messages.render_troubleshoot())"`
to regenerate the sections after editing the catalogue. Severity: **error** stops the run, **warning**
needs your attention, **info** is for the record.

## Findings

### CFG_CREATED

*Severity: info.*

**Cause.** There was no topor.toml in the project folder, so one was created with every setting at its default and a comment per key.

**Fix.** Edit topor.toml in the project folder to change settings; the key reference is docs/user/configuration.md (e.g. run.timeout_s).

### CFG_PARSE

*Severity: warning.*

**Cause.** topor.toml is not valid TOML, so the whole file is ignored and the built-in defaults are used.

**Fix.** Fix the syntax at the reported line, or delete topor.toml to get a fresh commented one on the next run (run.timeout_s is the first key).

### CFG_UNKNOWN_KEY

*Severity: warning.*

**Cause.** topor.toml contains a section or key topor does not know (usually a typo); it is ignored.

**Fix.** Correct the spelling against docs/user/configuration.md (e.g. run.timeout_s), or remove the entry.

### CFG_VALUE

*Severity: warning.*

**Cause.** A value in topor.toml has the wrong type or is outside its allowed range; the default is used instead.

**Fix.** Correct the value named in the finding (allowed range is in docs/user/configuration.md, e.g. run.timeout_s) or use a layer role signal|mixed|gnd|pwr|jumper in [layers].

### CFG_VALUE_BACKLOG

*Severity: warning.*

**Cause.** The value is planned but not implemented in this version; the default is used.

**Fix.** Use the default for now, e.g. output.geometry = "arcs" or "segments".

### CFG_VERSION

*Severity: warning.*

**Cause.** topor.toml carries a version this plugin does not read; keys may be misread.

**Fix.** Set version = 1 at the top of topor.toml and check the keys (e.g. run.timeout_s) against docs/user/configuration.md.

### CFG_WRITE

*Severity: warning.*

**Cause.** topor.toml could not be created (folder not writable); the built-in defaults are used and no choice can be saved.

**Fix.** Make the project folder writable (KiCad → File → Save As to a folder you own) or accept the built-in defaults.

### CLEANUP_SKIPPED_PARTIAL

*Severity: info.*

**Cause.** KiCad cleanup / teardrops were skipped because the result has partial routes.

**Fix.** Finish the partial connections by hand, then run KiCad → Inspect → Cleanup Tracks & Vias yourself (run.kicad_cleanup).

### DANGLING_ENDS

*Severity: info.*

**Cause.** Existing tracks end at nothing (dead ends); topor either continues them or drops them (search.dangling_ends).

**Fix.** Set search.dangling_ends to continue or drop, or tidy them first in KiCad → Inspect → Design Rules Checker.

### DEFAULTS_USED

*Severity: info.*

**Cause.** Some values were not set in KiCad, so topor's built-in defaults were used (e.g. the default track width for nets without a netclass, existing copper or setting).

**Fix.** Set the values in KiCad → File → Board Setup → Design Rules → Net Classes / Pre-defined Sizes.

### DO_NOT_TOUCH

*Severity: warning.*

**Cause.** nets.do_not_touch lists nets whose copper is fixed; or one of its patterns matches no net on this board.

**Fix.** Check the pattern spelling in nets.do_not_touch against the net names (KiCad → Inspect → Board Statistics → Nets shows them).

### DRILL_OUTSIDE_FILTER

*Severity: warning.*

**Cause.** A via drill that the board uses is not in the fab drill filter, so the router could place a drill your fab filter forbids.

**Fix.** Add the drill to vias.allowed_drills_mm (accepting the finding does it) or change the via sizes in KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias.

### ENGINE_VERSION

*Severity: error.*

**Cause.** The routing engine installed in the plugin environment is not the version this plugin was shipped with.

**Fix.** Reinstall the plugin with tools/build.sh (docs/user/install.md) and restart KiCad (File → Quit, then start KiCad again).

### GROUPS_UNAVAILABLE

*Severity: info.*

**Cause.** The board contains groups; the KiCad API does not expose group membership, so grouped items are treated one by one and new tracks are not grouped.

**Fix.** Select the footprints themselves, or ungroup first (KiCad → Edit → Ungroup).

### INTERNAL

*Severity: error.*

**Cause.** An unexpected internal error stopped the run. The board was not changed.

**Fix.** Run again; if it recurs, attach log.txt and the board copy from the report folder (Open report folder in the dialog; KiCad → Edit → Undo shows nothing from topor).

### KICAD_CLI_MISSING

*Severity: error.*

**Cause.** kicad-cli, which topor uses for DRC and the preview, was not found.

**Fix.** Install KiCad 10 with its command-line tools (KiCad → Help → About KiCad → Copy Version Info shows the install), or set the KICAD_CLI environment variable.

### LAYER_ROLES_NEEDED

*Severity: error.*

**Cause.** The board has a copper-layer count with no default stackup (not 2, 4, 6 or 8) and no role is set for some layer.

**Fix.** Set each layer's role in topor.toml [layers], e.g. In1.Cu = "gnd", or set layer types in KiCad → File → Board Setup → Board Editor Layers.

### LAYER_TYPES

*Severity: warning.*

**Cause.** A layer role set in topor.toml [layers] contradicts the layer type KiCad has for that layer (e.g. KiCad says power, topor.toml says signal); the topor.toml role wins.

**Fix.** Make them agree: change the type in KiCad → File → Board Setup → Board Editor Layers, or the role in topor.toml [layers].

### LOCKED_COPPER

*Severity: info.*

**Cause.** Locked tracks or vias exist; with run.respect_locked on they are fixed obstacles, with it off topor may rip or smooth them.

**Fix.** Set run.respect_locked, or unlock items in KiCad → Edit → Unlock Selected.

### MARKER_CONFLICT

*Severity: warning.*

**Cause.** The @N marker in the net name disagrees with the topor_pN netclass or with nets.priority; the stronger source wins (nets.priority, then netclass, then marker).

**Fix.** Remove one source: edit the net label or KiCad → File → Board Setup → Design Rules → Net Classes, or nets.priority.

### MARKER_IGNORED

*Severity: warning.*

**Cause.** An @N priority marker in a net name could not be used (out of range or on a net that is not routed); it is ignored.

**Fix.** Use @1, @2 or @3 in the net name (KiCad → Schematic → net label) or set nets.priority.

### MARKER_PAIR

*Severity: warning.*

**Cause.** The two halves of a differential pair carry different @N markers; the lower number is used for both.

**Fix.** Give both nets the same marker (KiCad → Schematic → net label) or set nets.priority.

### NON_DEFAULTS

*Severity: info.*

**Cause.** Some topor.toml settings differ from the defaults; listed so a result can be reproduced.

**Fix.** Nothing to do; revert a key in topor.toml (e.g. run.timeout_s) to its default to remove it from this list.

### NOTHING_ROUTED

*Severity: warning.*

**Cause.** The router produced no copper.

**Fix.** See the connection reasons in the report; relax a setting (run.timeout_s, vias.max_per_connection) or route by hand.

### PAD_OUTSIDE_BOARD

*Severity: warning.*

**Cause.** A pad's centre lies outside the Edge.Cuts outline, so no route can reach it.

**Fix.** Move the footprint inside the outline (KiCad → Edit → Properties), or accept the finding to add the pad to nets.ignore_pads.

### PCB_FORMAT_OLD

*Severity: error.*

**Cause.** The board file is older than the KiCad 10 format; topor edits the file text and would silently skip items written in the old format.

**Fix.** Open the board in KiCad 10 and save it (KiCad → File → Save), then run topor again.

### PLANE_NO_ZONE

*Severity: warning.*

**Cause.** A layer whose role is a plane (gnd, pwr, or a KiCad power layer) has no filled zone on a matching net, so plane connections cannot be served by the pour.

**Fix.** Add a zone on that layer with a ground/power net covering the board outline (KiCad → Place → Add Filled Zone), then refill zones (KiCad → Edit → Fill All Zones, B), or change the role in topor.toml [layers].

### POWER_BY_NAME

*Severity: info.*

**Cause.** Nets still in the Default netclass whose name matches nets.power_pattern are treated as power nets.

**Fix.** Put them in a power netclass (KiCad → File → Board Setup → Design Rules → Net Classes) or adjust nets.power_pattern.

### PREEXISTING_DRC

*Severity: info.*

**Cause.** KiCad DRC already reports violations or unconnected items on the board before routing; they are listed as pre-existing.

**Fix.** Nothing to do, or fix them first (KiCad → Inspect → Design Rules Checker); run.fail_on_preexisting = true turns this into an error.

### PREVIEW_FAILED

*Severity: warning.*

**Cause.** The preview image could not be rendered; the DRC numbers in the summary are still valid.

**Fix.** Check kicad-cli in KiCad → Help → About KiCad and the log in the report folder.

### SCOPE_EMPTY

*Severity: warning.*

**Cause.** There is no connection to route in the chosen scope.

**Fix.** Select other footprints in the PCB editor, change run.scope_mode, or check that nets are assigned (KiCad → Tools → Update PCB from Schematic).

### TRACK_ANGLE_RULE

*Severity: error.*

**Cause.** A custom rule restricts track angle or segment length; topor routes curved traces and cannot honour it, so the result would violate your rule.

**Fix.** Remove or scope the rule in KiCad → File → Board Setup → Design Rules → Custom Rules, or route that area by hand.

### VIA_IN_PAD_UNPROTECTED

*Severity: warning.*

**Cause.** vias.via_in_pad is on but KiCad's via protection is not filled and capped, so vias in pads risk solder wicking and voids.

**Fix.** Set filling and capping in KiCad → File → Board Setup → Board Stackup → Board Finish → Via protection, or turn vias.via_in_pad off.

### VIA_NOT_SET_UP

*Severity: warning.*

**Cause.** KiCad has no via sizes set up (factory defaults only), so topor falls back to its own via size.

**Fix.** Set vias in KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias (or a net class), or set vias.fallback_diameter_mm and vias.fallback_drill_mm.
