# Troubleshooting

Every message topor shows has a **code**, a **cause** and a **fix**; each code has a section under
[Findings](#findings) (`### CODE`). Severity: **error** stops the run, **warning** needs your attention,
**info** is for the record. Problems that happen before topor can show a dialog (installation, plugin
environment, exit codes) come first.

Where to look:

1. The dialog itself (code, message, Fix). **Copy details** in the error dialog copies what a bug report
   needs.
2. The run's report folder (**Open report folder**; default `~/.cache/kicad/10.0/topor/<board>/<timestamp>/`):
   `log.txt` (every step), `report.md`, `result.json`, `drc.json` — see [operate.md](operate.md#report-folder).
3. The crash log `~/.cache/kicad/10.0/topor/topor-crash.log` (or `$TOPOR_REPORT_ROOT/topor-crash.log`):
   every fatal error of the plugin process is appended there with a time stamp and the Python traceback,
   also when no dialog could be shown.
4. Still stuck: [docs/dev/debug.md](../dev/debug.md#filing-a-bug) explains how to file a bug.

## Installation and start-up

### The Topor button does not appear

**Cause.** KiCad did not load the plugin: it is not installed, the KiCad API is off, the plugin
environment is still being created or failed, or the configured Python is older than 3.11.

**Fix.** Check that `~/.local/share/kicad/10.0/plugins/org.topor.router/plugin.json` exists (else run
`tools/build.sh`, [install.md](install.md)); tick **Enable KiCad API** in KiCad → Preferences → Plugins;
restart the PCB Editor and wait a minute on the first start. Look for **Topor: route** in the toolbar and
under **Tools → External Plugins**. Then see the sections below.

### Cannot connect to KiCad (API server off)

**Cause.** The plugin talks to KiCad over its IPC API. With the API disabled the plugin cannot read the
board: an error dialog shows [KICAD_API_CONNECT](#kicad_api_connect), the crash log has the connection
error from `kipy`, exit code 1.

**Fix.** KiCad → Preferences → Plugins → tick **Enable KiCad API**, restart the PCB Editor, run again.

### Python interpreter older than 3.11

**Cause.** `plugin.json` requires Python ≥ 3.11 (topor reads `topor.toml` with `tomllib`). KiCad builds the
plugin environment with the interpreter set in its preferences; an older one is refused or fails on import.

**Fix.** KiCad → Preferences → Plugins → **Python interpreter**: choose a Python 3.11+ (normally
`/usr/bin/python3`), then **Recreate Plugin Environment** (below).

### Plugin environment not created or pip fails

**Cause.** On first load KiCad creates `~/.cache/kicad/10.0/python-environments/org.topor.router/` and runs
`pip install -r requirements.txt` there: `kicad-python` 0.8.x comes from PyPI (needs network), the engine
wheel `topor-engine==0.4.0` from the plugin's `wheels/` folder (`--only-binary`, so only a built wheel works).
A missing wheel, no network, or a half-finished environment from an interrupted install stops it.

**Fix.** Check that `~/.local/share/kicad/10.0/plugins/org.topor.router/wheels/topor_engine-0.4.0-*.whl`
exists (else `tools/build.sh`). Then KiCad → Preferences → Plugins, right-click the Topor action →
**Recreate Plugin Environment**. If that is not offered, close KiCad, delete
`~/.cache/kicad/10.0/python-environments/org.topor.router/` and start KiCad again — it rebuilds the
environment from scratch.

### No module named topor_engine

**Cause.** The engine wheel is not installed in the plugin environment (the crash log shows
`ModuleNotFoundError: No module named 'topor_engine'`, exit code 1).

**Fix.** Rebuild with `tools/build.sh`, then **Recreate Plugin Environment** (above). Developers can
reinstall it in place:
`~/.cache/kicad/10.0/python-environments/org.topor.router/bin/pip install --force-reinstall --no-deps plugin/wheels/topor_engine-0.4.0-*.whl`
(this is what `tools/build.sh --dev` does).

### No module named kipy

**Cause.** `kicad-python` (module `kipy`) is missing from the plugin environment — usually the first
install had no network.

**Fix.** Connect to the network and **Recreate Plugin Environment** (above).

### No module named wx

**Cause.** The dialogs use the system wxPython; the plugin environment sees system packages only when it
was built from the system Python.

**Fix.** Install KiCad's wxPython package (Ubuntu: `python3-wxgtk4.0`), set the Python interpreter in
KiCad → Preferences → Plugins to `/usr/bin/python3`, then **Recreate Plugin Environment**.

### ENGINE_VERSION after an update

The engine in the plugin environment is older or newer than the plugin. See
[ENGINE_VERSION](#engine_version): rebuild, then **Recreate Plugin Environment** or restart the PCB Editor.

## Exit codes

KiCad runs topor as a separate process; when it ends with a non-zero exit code KiCad may report that the
plugin "exited with code N". The codes:

### The plugin exited with code 0

Normal end: the result was accepted or rejected, the run was cancelled, or it stopped at the pre-flight
dialog. Nothing to do — except after a [POST_COMMIT_STEP_FAILED](#post_commit_step_failed) warning (the
routes are on the board, a later step such as the zone refill failed).

### The plugin exited with code 1

**Cause.** An error, shown in a *Topor — Error* dialog: the engine version check failed
([ENGINE_VERSION](#engine_version)), a module is missing (sections above), KiCad could not be reached
([KICAD_API_CONNECT](#kicad_api_connect)), the report folder could not be written
([REPORT_DIR_UNWRITABLE](#report_dir_unwritable)), the engine did not stop after Cancel
([ENGINE_UNRESPONSIVE](#engine_unresponsive)), a kicad-cli call ran out of time
([KICAD_CLI_TIMEOUT](#kicad_cli_timeout)), the board was edited during the run
([BOARD_CHANGED_DURING_RUN](#board_changed_during_run)), or an unexpected exception or engine panic happened
([INTERNAL](#internal)). The board is not changed in any of these cases.

**Fix.** Read the last entry of `~/.cache/kicad/10.0/topor/topor-crash.log` (and `log.txt` in the report
folder if the run got that far); it names the cause. If it is not one of the sections here, file a bug
([debug.md](../dev/debug.md#filing-a-bug)).

### The plugin exited with code 2

**Cause.** Wrong command-line arguments. KiCad passes none, so this only happens when topor is run by hand
(`python -m topor.app --headless …` without `--model`, or a misspelt option).

**Fix.** Use the usage line printed on stderr, or [debug.md](../dev/debug.md#headless-runs).

### The plugin exited with another code

**Cause.** The process was killed — e.g. 137 or -9 is SIGKILL, typically the system running out of memory
on a very large board; a negative number is the signal.

**Fix.** Route a selection instead of the whole board, set a time limit (`[run] timeout_s`), or fewer
threads (`[run] threads`); file a bug with the board if it recurs.

## Results

### Nothing to route on a routed board

**Cause.** With *Existing copper: Keep as obstacles* (`[run] existing_copper = "fixed"`, the default) topor
only routes what is not connected yet; on a fully routed board that is nothing
([NOTHING_ROUTED](#nothing_routed) or 0 connections in the header).

**Fix.** Choose *Rip up and re-route* in the [Options](operate.md#options), or route a selection.

### The result has new DRC errors in KiCad

**Cause.** topor checks its result with KiCad's DRC before the preview (the *DRC new* line of the summary).
A difference to KiCad's interactive DRC usually means the board changed after the run, or zones were not
refilled.

Since never drop ([`VIOLATIONS`](#violations)), the topological core writes a connection it could not draw
legally on its best geometry; those places break a rule on purpose and the report lists each of them.

**Fix.** Refill zones (KiCad → Edit → Fill All Zones, B), run KiCad → Inspect → Design Rules Checker again,
and compare with `drc.json` in the report folder. A new error at a place the report lists under VIOLATIONS is
expected (fix it by hand, or set `[optimize] allow_violations = false` for legal copper only). If the router's
copper violates a rule anywhere else, file a bug.

## Findings

### BOARD_CHANGED_DURING_RUN

*Severity: error.*

**Cause.** The board was edited while Topor ran (items were added, deleted or moved), so the computed routes may no longer fit. Nothing was committed and the board is exactly as you left it.

**Fix.** Undo your edits (KiCad → Edit → Undo) or run Topor again; do not edit the board while Topor runs.

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

**Fix.** Finish the partial connections by hand, then run KiCad → Tools → Cleanup Tracks & Vias yourself (run.kicad_cleanup).

### CONNECTION_WIDTH

*Severity: warning.*

**Cause.** A trace would run so close past a pad or via of its own net that the copper between them gets thinner than KiCad's minimum connection width, so the trace was not drawn.

**Fix.** Route these connections by hand, or check the minimum in KiCad → File → Board Setup → Design Rules → Constraints → Minimum connection width.

### DANGLING_ENDS

*Severity: info.*

**Cause.** Existing tracks end at nothing (dead ends); topor continues them along their direction, keeps them as fixed points or drops them (search.dangling_ends).

**Fix.** Set search.dangling_ends to continue, fixed or drop, or tidy them first in KiCad → Inspect → Design Rules Checker.

### DEFAULTS_USED

*Severity: info.*

**Cause.** Some values were not set in KiCad, so topor's built-in defaults were used (e.g. the default track width for nets without a netclass, existing copper or setting).

**Fix.** Set the values in KiCad → File → Board Setup → Design Rules → Net Classes / Pre-defined Sizes.

### DIFF_PAIRS

*Severity: info.*

**Cause.** Differential pairs were routed as coupled pairs: two tracks at the pair gap from a fan-out stub at each pad pair. Each line gives one pair connection: the lengths of P and N, their skew, the share coupled at the gap, the uncoupled length at each end and the vias. KiCad's DRC checks pairs only against custom rules (diff_pair_gap, diff_pair_uncoupled, skew); without them these numbers are information.

**Fix.** Check the pairs in the preview. To make KiCad's DRC hold them to limits, add custom rules in KiCad → File → Board Setup → Design Rules → Custom Rules (diff_pair_uncoupled, skew within_diff_pairs); topor then prices uncoupled length and skew more steeply past those limits. Tune skew by hand with KiCad's skew tuner (topor does not meander yet).

### DIFF_PAIRS_SKIPPED

*Severity: warning.*

**Cause.** Differential pairs (two nets whose names end in +/- or P/N, KiCad's rule) are not routed because diff-pair routing is switched off ([diffpair] enabled = false), or their pair connection could not be routed; their connections stay unrouted.

**Fix.** Set [diffpair] enabled = true in topor.toml to route them as coupled pairs, or route them by hand with KiCad's differential-pair router (KiCad → Route → Differential Pair, hotkey 6). To keep topor off them completely, list them in nets.do_not_touch.

### DO_NOT_TOUCH

*Severity: warning.*

**Cause.** nets.do_not_touch lists nets whose copper is fixed; or one of its patterns matches no net on this board.

**Fix.** Check the pattern spelling in nets.do_not_touch against the net names (KiCad → View → Panels → Net Inspector lists them).

### DRILL_OUTSIDE_FILTER

*Severity: warning.*

**Cause.** A via drill that the board uses is not in the fab drill filter, so the router could place a drill your fab filter forbids.

**Fix.** Add the drill to vias.allowed_drills_mm (accepting the finding does it) or change the via sizes in KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias.

### ENGINE_UNRESPONSIVE

*Severity: error.*

**Cause.** After Cancel the routing engine did not stop within the grace period (it is stuck outside its stop checks), so the run was ended without a result. The board was not changed.

**Fix.** Run again with a shorter run.timeout_s; if it recurs, file a bug with log.txt and result.json from the report folder (docs/dev/debug.md). If KiCad stays slow, save and restart it (KiCad → File → Exit).

### ENGINE_VERSION

*Severity: error.*

**Cause.** The routing engine installed in the plugin environment is not the version this plugin was shipped with.

**Fix.** Reinstall the plugin with tools/build.sh (docs/user/install.md) and restart KiCad (File → Quit, then start KiCad again).

### GROUPS_UNAVAILABLE

*Severity: info.*

**Cause.** The board contains groups; the KiCad API does not expose group membership, so grouped items are treated one by one and new tracks are not grouped.

**Fix.** Select the footprints themselves, or ungroup first (KiCad → Edit → Ungroup).

### IGNORED_RULE

*Severity: warning.*

**Cause.** Some copper breaks a KiCad check whose severity is set to ignore; topor prices such places (the ignored_rule social factor) instead of forbidding them, as KiCad's own DRC would not complain.

**Fix.** Set the check's severity in KiCad → File → Board Setup → Design Rules → Violation Severity to error to make topor keep away from it, or raise factors.weight ignored_rule.

### INTERNAL

*Severity: error.*

**Cause.** An unexpected internal error stopped the run. The board was not changed.

**Fix.** Run again; if it recurs, file a bug with log.txt and result.json from the report folder (Open report folder in the dialog) and a copy of the project folder saved with KiCad → File → Save (docs/dev/debug.md).

### KEEPOUT_PAD

*Severity: warning.*

**Cause.** A pad of a net that needs routing lies inside a rule area that forbids tracks on every copper layer of the pad, so no trace can reach it.

**Fix.** Move the pad's footprint or change the rule area in KiCad (select the rule area → Properties: untick Tracks), or route that connection by hand.

### KICAD_API_CONNECT

*Severity: error.*

**Cause.** topor could not connect to KiCad over its IPC API (the API server is off, KiCad is still starting, or no board is open in the PCB Editor). The board was not changed.

**Fix.** KiCad → Preferences → Plugins → tick Enable KiCad API, restart the PCB Editor with the board open, then press the Topor button again.

### KICAD_CLI_MISSING

*Severity: error.*

**Cause.** kicad-cli, which topor uses for DRC and the preview, was not found.

**Fix.** Install KiCad 10 with its command-line tools (KiCad → Help → About KiCad → Copy Version Info shows the install), or set the KICAD_CLI environment variable.

### KICAD_CLI_TIMEOUT

*Severity: error.*

**Cause.** A kicad-cli call (the DRC with zone refill, or the SVG export of the preview) did not finish within run.kicad_cli_timeout_s and was stopped. On a large board or a busy machine KiCad's own checks can take minutes. The board was not changed.

**Fix.** Raise run.kicad_cli_timeout_s in topor.toml (seconds; 0 = no timeout), close other heavy programs, then run Topor again; if kicad-cli hangs even on a small board, run it by hand on a saved copy (docs/dev/debug.md).

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

**Cause.** The @N marker in the net name disagrees with the topor_pN netclass or with nets.priority; the stronger source wins: the @N marker, then the topor_pN netclass, then nets.priority.

**Fix.** Remove one source: edit the net label or KiCad → File → Board Setup → Design Rules → Net Classes, or nets.priority.

### MARKER_IGNORED

*Severity: warning.*

**Cause.** An @N priority marker in a net name could not be used (out of range or on a net that is not routed); it is ignored.

**Fix.** Use @1, @2 or @3 in the net name (KiCad → Schematic → net label) or set nets.priority.

### MARKER_PAIR

*Severity: warning.*

**Cause.** The two halves of a differential pair carry different @N markers; the lower number is used for both.

**Fix.** Give both nets the same marker (KiCad → Schematic → net label) or set nets.priority.

### NECKED

*Severity: info.*

**Cause.** Some traces leave their pad narrower than the net's track width (necking) because the full width does not fit out of the pad; the rest of the trace keeps its width.

**Fix.** Accept them, or forbid it with search.neck_to_pad = false; the narrowest allowed width is KiCad → File → Board Setup → Design Rules → Constraints → Minimum track width.

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

### PLANE_PAD_SIGNAL

*Severity: info.*

**Cause.** A pad of a plane net (a net whose pour is on a plane layer) had no room for a short stub and via into its pour, so it is joined to its net's copper by an ordinary trace instead.

**Fix.** Nothing to do if the trace is acceptable; otherwise make room next to the pad or allow vias closer to it (vias.plane_via_max_mm, vias.via_in_pad).

### PLANE_VIA_OUTSIDE_POUR

*Severity: warning.*

**Cause.** A via that should drop a pad onto its plane ended outside the filled copper of that plane, so it does not connect there.

**Fix.** Refill the zones (KiCad → Edit → Fill All Zones, B) and check the via; move it into the pour by hand or enlarge the zone (KiCad → zone properties).

### POST_COMMIT_STEP_FAILED

*Severity: warning.*

**Cause.** The routes were committed to the board, then a later step failed (zone refill, a KiCad cleanup / teardrop action, writing the report or showing the done notice); the routes stay on the board.

**Fix.** Ctrl+Z (Edit → Undo) removes the routes; or keep them and do the failed step yourself: refill zones with B (KiCad → Edit → Fill All Zones), run KiCad → Tools → Cleanup Tracks & Vias, or check the report folder (output.report_dir).

### POUR_ISLANDS

*Severity: warning.*

**Cause.** The new copper cuts pieces of a copper pour off from the rest of the pour, and topor found no place for a stitching via or pad stub to join them back; after a zone refill KiCad either removes those pieces or lists them as unconnected.

**Fix.** Refill the zones (KiCad → Edit → Fill All Zones, B), look at the listed places and add a via by hand (KiCad → Place → Via) or move a trace; planes.stitch_islands controls topor's own stitching.

### POUR_STITCHED

*Severity: info.*

**Cause.** The new copper cut pieces off a copper pour; topor joined them back with stitching vias inside the pieces or short stubs from their pads (each one is listed).

**Fix.** Nothing to do; check them after a zone refill (KiCad → Edit → Fill All Zones, B). planes.stitch_islands = false only reports the pieces.

### POUR_UNJOINED

*Severity: warning.*

**Cause.** KiCad's DRC of the routed copy, after a zone refill, reports pieces of a copper pour cut off from the rest of the pour, or pads / vias that the pour no longer reaches. In preview.png such a pad has a blue ring; a cut-off piece is only counted in the legend (KiCad gives no position for it).

**Fix.** Refill the zones (KiCad → Edit → Fill All Zones, B), run KiCad → Inspect → Design Rules Checker to see each one, then add a stitching via (KiCad → Place → Via) or move the trace that cuts the pour; planes.stitch_islands lets topor stitch what it can.

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

### REPORT_DIR_UNWRITABLE

*Severity: error.*

**Cause.** The run's report folder could not be created or written (read-only folder, full disk, or a file where the folder should be), so topor stopped before reading the board. The board was not changed.

**Fix.** Point output.report_dir in topor.toml at a folder you can write (e.g. "topor-reports" inside the project folder), or fix the permissions / free disk space for the folder named in the message.

### SCOPE_EMPTY

*Severity: warning.*

**Cause.** There is no connection to route in the chosen scope.

**Fix.** Select other footprints in the PCB editor, change run.scope_mode, or check that nets are assigned (KiCad → Tools → Update PCB from Schematic).

### TIME_BUDGET_HIT

*Severity: warning.*

**Cause.** The time limit (or a Cancel) stopped the topological core before its routes were stable ([optimize] stable_window / stable_gain_pct); the result is the best so far and may differ between runs and machines. The stop reason says which clock fired: timeout_wall (seconds on the clock) or timeout_cpu (CPU seconds summed over all threads, run.timeout_clock = cpu).

**Fix.** Raise run.timeout_s (0 = no limit; Cancel always works; run.timeout_clock chooses wall or cpu seconds) and route again, or lower [optimize] runs (best of N runs share the threads); wait for "Routes stable" and press OK for a reproducible result.

### TRACK_ANGLE_RULE

*Severity: error.*

**Cause.** A custom rule restricts track angle or segment length; topor routes curved traces and cannot honour it, so the result would violate your rule.

**Fix.** Remove or scope the rule in KiCad → File → Board Setup → Design Rules → Custom Rules, or route that area by hand.

### UNROUTED

*Severity: warning.*

**Cause.** Some connections could not be finished; the report's Connections table names each one with its reason (no room for a via, no room at the clearance when the trace was drawn, ...). The open connections are drawn blue in preview.png.

**Fix.** Route the remaining connections by hand (KiCad → Inspect → Design Rules Checker lists them as unconnected items), or make room: smaller vias in KiCad → File → Board Setup → Design Rules → Net Classes, fewer obstacles, or another layer; then run topor again with run.existing_copper = "fixed" to keep the result.

### VIA_IN_PAD_UNPROTECTED

*Severity: warning.*

**Cause.** vias.via_in_pad is on but KiCad's via protection is not filled and capped, so vias in pads risk solder wicking and voids.

**Fix.** Set filling and capping in KiCad → File → Board Setup → Board Stackup → Board Finish → Via protection, or turn vias.via_in_pad off.

### VIA_NOT_SET_UP

*Severity: warning.*

**Cause.** KiCad has no via sizes set up (no pre-defined via, and every net class via is KiCad's factory 0.8 / 0.4 mm), so every net with that via is routed with vias.fallback_diameter_mm / vias.fallback_drill_mm (0.8 / 0.4 mm by default) and, where space requires, smaller vias from topor's fab profile (a drill of 0.25, 0.3, 0.4 or 0.5 mm below the fallback drill, never below the board's minimum via drill and annular ring).

**Fix.** Set the via size and hole of the net classes in KiCad → File → Board Setup → Design Rules → Net Classes (topor routes each net with its net class via), or set vias.fallback_diameter_mm and vias.fallback_drill_mm. A via in KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias also clears this finding: topor routes with the net class via and uses the smaller pre-defined vias where space requires.

### VIA_SQUEEZE

*Severity: warning.*

**Cause.** Some vias stand in a narrow pass: less room than half a via diameter is left next to them (the via_squeeze social factor). They pass KiCad's DRC but leave little room for later edits.

**Fix.** Accept them, or raise the price of narrow passes with factors.weight via_squeeze (topor.toml [factors] weight = { via_squeeze = 10 }) and route again; factors.report via_squeeze = "off" hides the list.

### VIOLATIONS

*Severity: warning.*

**Cause.** Never drop: some connections had no legal geometry when drawn (a gap too narrow, no free site for a via, a crossing that cannot be layered, a branch meeting its trace on another layer); topor kept them and wrote them on their best geometry, so the copper breaks a rule there. Each violation is listed with its place, type (clearance, short, hole_clearance, edge_clearance, connection_width) and cause; KiCad's DRC shows the same places. The summary groups them by origin: at a pad exit (a neighbour pin of the connection's own part: the trace does not get out of its pad at full width; pad exits and necking are planned in a later version), from a via without a legal site, at a crossing with another net, against other routed copper in a narrow passage, or against other pads, holes or the board edge.

**Fix.** Fix each listed place by hand (move the trace or via, or a part), or give the router more room or time (run.timeout_s, [optimize] runs) and route again. [optimize] allow_violations = false writes legal copper only (those connections stay unrouted).
