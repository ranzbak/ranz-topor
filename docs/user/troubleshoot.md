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
board; the crash log shows a connection error from `kipy`, exit code 1.

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
dialog. Nothing to do.

### The plugin exited with code 1

**Cause.** An error: the engine version check failed ([ENGINE_VERSION](#engine_version)), a module is
missing (sections above), KiCad could not be reached, or an unexpected exception or engine panic happened
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

**Fix.** Refill zones (KiCad → Edit → Fill All Zones, B), run KiCad → Inspect → Design Rules Checker again,
and compare with `drc.json` in the report folder. If the router's copper really violates a rule, file a bug.

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

**Fix.** Finish the partial connections by hand, then run KiCad → Tools → Cleanup Tracks & Vias yourself (run.kicad_cleanup).

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

**Fix.** Check the pattern spelling in nets.do_not_touch against the net names (KiCad → View → Panels → Net Inspector lists them).

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

**Fix.** Run again; if it recurs, file a bug with log.txt and result.json from the report folder (Open report folder in the dialog) and a copy of the project folder saved with KiCad → File → Save (docs/dev/debug.md).

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

**Cause.** KiCad has no via sizes set up (no pre-defined via, and every net class via is KiCad's factory 0.8 / 0.4 mm), so every net with that via is routed with vias.fallback_diameter_mm / vias.fallback_drill_mm (0.8 / 0.4 mm by default) and, where space requires, smaller vias from topor's fab profile (a drill of 0.25, 0.3, 0.4 or 0.5 mm below the fallback drill, never below the board's minimum via drill and annular ring).

**Fix.** Set the via size and hole of the net classes in KiCad → File → Board Setup → Design Rules → Net Classes (topor routes each net with its net class via), or set vias.fallback_diameter_mm and vias.fallback_drill_mm. A via in KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias also clears this finding, but topor 0.4.0 does not route with pre-defined vias (from 0.5.0).
