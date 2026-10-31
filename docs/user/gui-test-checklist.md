# GUI test checklist — topor 0.4.0 (M4b gate)

What can only be checked with a running KiCad 10 on a desktop: the plugin flow, the dialogs, and every
plugin call marked UNVERIFIED in the code. Run it on **eprom-to-kernal** and **wiccon-2023**, on the copies
in `scratch/gui/<board>/` — never on the originals under `~/work/kicad/projects` and never on the snapshots
in `scratch/boards/`. Tick each box, and fill in the [results table](#results) at the end (pass / fail / notes;
for a fail: what you saw, the report folder path, and the last lines of `log.txt`).

Background: [install.md](install.md) · [operate.md](operate.md) · [troubleshoot.md](troubleshoot.md) ·
wireframes `docs/progress/notes/m4-ui-wireframes.md` (dialog layout, tab order, wording).

## Setup

- [ ] **Build and install.** In the repo: `bash tools/build.sh --dev`. Expected: ends with
  `Installed …topor_engine-0.4.0-….whl. Restart PCB Editor or use Recreate Plugin Environment.` and
  `Plugin dir: ~/.local/share/kicad/10.0/plugins/org.topor.router`; that folder holds `plugin.json`,
  `topor_plugin.py`, `requirements.txt`, `topor/`, `toporlib/`, `docs/user/` (with `operate.md`), `wheels/`.
- [ ] **Enable the API.** KiCad → Preferences → Plugins → **Enable KiCad API** ticked; the Python interpreter
  shown there is ≥ 3.11. Expected: the checkbox and the interpreter field are on that page (record the exact
  wording if it differs).
- [ ] **Plugin environment.** On the Plugins page, right-click the Topor action → **Recreate Plugin
  Environment** (record whether this entry exists and where), then restart the PCB Editor. Expected:
  `~/.cache/kicad/10.0/python-environments/org.topor.router/` exists and contains `topor_engine` 0.4.0 and
  `kipy` 0.8.x (`…/bin/python -c "import topor_engine, kipy; print(topor_engine.version())"` → `0.4.0`).
- [ ] **Fresh copies.** For each board (`eprom-to-kernal`: snapshot `bfaf705c0f25`, file
  `eprom-to-kernal.kicad_pcb`; `wiccon-2023`: snapshot `2d3063e70ed5`, file `2023.kicad_pcb`):
  `rm -rf scratch/gui/<board> && cp -r scratch/boards/<board>/<sha12> scratch/gui/<board> && rm scratch/gui/<board>/MANIFEST.json`.
  Expected: no `topor.toml` in the copy (so the first-run notice appears). Both copies are **hand-routed**:
  choose *Existing copper: Rip up and re-route* in the dialog, otherwise there is nothing to route.
- [ ] **Toolbar.** Open the copy in the PCB Editor (wiccon is a KiCad 6 file: KiCad converts it in memory;
  do not save over the snapshot). Expected: the toolbar shows **Topor: route** (tooltip "Route the board or
  the selection with Topor") and **Tools → External Plugins** lists it.

## Run sequence (each board)

| # | Do | Expected |
|---|---|---|
| R1 | Click **Topor: route**. | **Topor — Settings file created** with the path `scratch/gui/<board>/topor.toml`; the file exists. **Open file** opens it in the text editor and the dialog stays; **Continue** goes on. |
| R2 | Look at the pre-flight dialog. | Progress *Checking the board (KiCad DRC)* first, then **Topor — Route &lt;board&gt;**. Header: *N connections in scope · nets · 2 copper layers · engine 0.4.0* (N is an estimate: eprom 30, wiccon 84 from the harness model; the routed total later is 27 / 62). Findings: no error; eprom headless reference shows `POWER_BY_NAME` (GND, +5V) — record the list. Selecting a row fills Cause / Fix. Scope radios greyed with *(nothing selected in KiCad)*. |
| R3 | Choose *Rip up and re-route*; on **wiccon** set *Threads* 1 (slower, so Cancel can be hit); **Route**; press **Cancel** once while *Routing — negotiation pass* is shown. | Progress shows phase, routed/total, iteration of up to 30, vias, elapsed / 1:00. After Cancel the label reads *Cancelling — keeping the best result so far…*, the button greys, the dialog closes by itself, and either the preview opens with the best result (summary *Stopped: cancel*) or *Cancelled — nothing was changed on the board.* (nothing routed yet). eprom routes in ~0.1 s: Cancel is usually too late — note it and continue. |
| R4 | Preview (run again without Cancel if R3 ended without one). | **Topor — Preview: &lt;board&gt;**: *Current board* left, *Result* right; new copper highlighted with a halo visible on the panel background; Zoom Fit / 100 % / 200 % and Ctrl+wheel work; Summary pane = the summary block (*Routed 27/27* eprom, *62/62* wiccon; *DRC new 0 · … · unconnected 0*); report folder path shown. |
| R5 | **Reject**. | **Topor — Finished**: *Nothing was changed on the board.* The board looks exactly as before (same tracks, same ratsnest); **Edit → Undo** is greyed or shows nothing from topor. |
| R6 | Run again → (no first-run notice) → *Rip up and re-route* → **Route** → **Accept**. | **Topor — Finished**: *Routed 27/27 connections — applied to the board as one undo step (Edit → Undo).* (wiccon 62/62). The board shows the new curved tracks; zones refilled. |
| R7 | Press **Ctrl+Z** once. | All topor tracks disappear and the hand routing is back in **one** step; **Edit → Redo** (Ctrl+Shift+Z) brings the result back. |
| R8 | Redo, then **Inspect → Design Rules Checker** → Run DRC (refill zones). | 0 new violations compared with a DRC of the untouched copy; 0 unconnected items. Record the counts. |
| R9 | **Open report folder** (preview or finished notice). | The file manager opens `~/.cache/kicad/10.0/topor/<board file name>/<YYYYmmdd-HHMMSS>/` with `report.md`, `preview.png`, `result.json`, `drc.json`, `log.txt`; `report.md` *Outcome* matches what you did (rejected / applied). |
| R10 | Select two footprints (eprom: **U1** and **U2**; wiccon: **U1** and **U2**), click **Topor: route**, *Rip up and re-route*. | Scope radios enabled; the header count changes with the radio (eprom U1+U2: between 26, touching 30, whole 30; wiccon U1+U2: between 4, touching 31, whole 84 — estimates). Route with **Between selected footprints**: only connections among U1/U2 pads are routed, the rest of the board unchanged; Accept, then Ctrl+Z. |
| R11 | Pre-flight **Help** (Alt+H). | The browser opens `…/org.topor.router/docs/user/operate.md#options` (the *Options* section). |
| R12 | Optional — error dialog: in the installed `topor_plugin.py` set `SHIPPED_VERSION = "0.0.0"`, run, then `bash tools/build.sh` to restore. | **Topor — Error** with `ENGINE_VERSION`, Fix *Restart the PCB Editor or use Recreate Plugin Environment.*; **Copy details** copies and flips to *Copied* for 2 s; *Open report folder* greyed with *(no report folder yet)*; Close; nothing changed on the board. |
| R13 | Optional — blocked pre-flight: put `[run] fail_on_preexisting = true` in `topor.toml` on a board with existing DRC violations (or start KiCad with `KICAD_CLI=/nonexistent`). | Route disabled; status line *Cannot route: 1 error(s) — see Fix above.* with an error icon; Cancel works. |

## UI acceptance (every dialog that appeared)

Dialogs: first-run, pre-flight, progress, preview, finished, error. Per the wireframes §0 / §7:

- [ ] **Tab order** = reading order. Pre-flight: list → Cause/Fix → Accept → Scope radios (arrows) →
  Existing copper → Respect locked → Keep partial → Time limit → Threads → Save → Help → Cancel → Route.
  Preview: Zoom → image (arrows / PgUp / PgDn scroll) → Summary → Report folder → Open report folder →
  Reject → Accept. First-run: path → Open file → Continue. Error: Details → Copy details → Open report
  folder → Close.
- [ ] **Esc = Cancel** (pre-flight Cancel, progress Cancel, preview Reject, others Close/Continue); the window
  close box does the same.
- [ ] **Enter = default button** (Route, Accept, Continue, Close; in progress Enter = Cancel); Enter in the
  findings list = Route (no-op while Route is disabled).
- [ ] **Mnemonics** work and do not collide: pre-flight R A W B T L K S H C; preview A R O; error D O C;
  first-run O C; progress C; finished O C.
- [ ] **100 % scaling**: every dialog readable, nothing cut off, minimum sizes respected (first-run 440×200,
  pre-flight 640×480, progress 460×240, preview 800×520, finished 440×220, error 520×320 DIP).
- [ ] **200 % scaling** (GNOME Settings → Displays → Scale 200 %, or start KiCad with `GDK_SCALE=2`): same
  checks; the pre-flight (1280×960 px) and preview (1600×1040 px) fit a 1920×1080 screen; icons sharp or
  dropped (the severity word always shown); preview bitmap crisp at Fit.
- [ ] **Dark theme** (GNOME Settings → Appearance → Dark, or `GTK_THEME=Adwaita:dark`): all text readable,
  list / Cause / Summary colours follow the theme, grey hints readable, new-copper halo and legend swatch
  visible (tint switches to magenta #D040D0 when teal is below 3:1 contrast).
- [ ] Light theme: the same; progress numbers do not make the layout jump.

## IPC parity

`tests/plugin/test_ipc_parity.py` compares the model topor reads over the KiCad API with the harness model
(pcbnew) of the same file, with the tolerances and conservative directions of
[operate.md → Limitations](operate.md#limitations). It needs the board open in a running KiCad and skips
otherwise.

1. Open an **unmodified** copy of the snapshot in the PCB Editor (same file name as the snapshot; a fresh
   `scratch/gui/<board>/` copy from [Setup](#setup), before any Accept — or File → Revert). Do not save.
2. In a terminal in the repo:

   ```bash
   KICAD_API_SOCKET=ipc:///tmp/kicad/api.sock .venv/bin/pytest -q -rs tests/plugin/test_ipc_parity.py -k "eprom"
   ```

   (`-k "wiccon"` for the other board; `KICAD_API_TOKEN` is not needed for an external client; the socket
   path is KiCad's default on Linux — Flatpak: `~/.var/app/org.kicad.KiCad/cache/tmp/kicad/api.sock`).
   The test refills zones in the open board (it is not saved).
3. Expected: `test_ipc_parity[<board>]` **passed**; the other parametrised boards are skipped with
   *KiCad has '…' open*. A failure prints the differing fields — copy them into the results table.

- [ ] eprom-to-kernal parity passed
- [ ] wiccon-2023 parity passed
- [ ] optional: the other snapshot boards (dcsgonebadge, bornhack2026, nametag-sao, wedding-gift), each
  opened from a copy of `scratch/boards/<board>/<sha12>/`

## UNVERIFIED items

Each is code that could not be exercised without a live KiCad. Tick when the expected result is seen
(most are covered by the run sequence above; the row says where).

**B2 — IpcBoard calls** (`plugin/topor/ipc.py`)

- [ ] `KiCad(socket_path=$KICAD_API_SOCKET, kicad_token=$KICAD_API_TOKEN)` connects when KiCad launches the plugin, and `get_board()` is the board that launched it (R1).
- [ ] `board.get_project().path` = the copy's folder (first-run notice path, R1) and `board.name` = the file name (dialog titles, report folder).
- [ ] `board.save_as(…, overwrite=True, include_project=True)` writes the live state **including unsaved edits** (move a part without saving, run, Reject: the preview's *Current board* shows the moved part).
- [ ] `board.get_selection()` + `get_footprints()` (pad → reference) give the selected footprints / pads (R10).
- [ ] `get_project().get_net_classes()` + `get_nets()` + `get_netclass_for_nets()` give the net classes (pre-flight `POWER_BY_NAME` lists only Default-class nets; a net class width shows in the routed widths).
- [ ] `begin_commit` / `remove_items_by_id` / `create_items` / `push_commit` = **one** undo step; `drop_commit` on failure (R6, R7).
- [ ] `Group` creation: new tracks in group `topor` (select one track → it selects the group), else the report *Notes* say "Items were not grouped (…)".
- [ ] `board.refill_zones()` after Accept (zones refilled without pressing B).
- [ ] `kicad.run_action("pcbnew.GlobalEdit.cleanupTracksAndVias")` / `("pcbnew.GlobalEdit.editTeardrops")` open KiCad's dialogs with `[run] kicad_cleanup = true` / `kicad_teardrops = true` in `topor.toml` (record whether the action names are accepted).

**B3 — live kipy fields of the model exporter** (`plugin/topor/model_ipc.py`; covered by [IPC parity](#ipc-parity))

- [ ] `get_enabled_layers`, `get_nets`, `get_footprints` (pads / shapes / zones / text inside `definition.items` with absolute coordinates), `get_tracks`, `get_vias`, `get_shapes`, `get_text`, `get_zones` (rule areas, `filled_polygons` after `refill_zones`).
- [ ] `get_pad_shapes_as_polygons(pad, layer)` for custom / trapezoid / chamfered / offset pads (absolute coordinates).
- [ ] `get_item_bounding_box(text)` for copper text.
- [ ] Pad fields: `padstack.angle` absolute (pad rotation), `padstack.layers` lists all copper layers of a PTH, `copper_clearance_override` only when set, thermal spoke angle / gap present only when set, footprint rule areas, `definition.net_ties[].pad_number`.
- [ ] Via `padstack.copper_layers[0].size.x` = diameter; drill span from `padstack.drill.start_layer` / `end_layer`.

**R8 — conservative approximations, live-only corners** (`plugin/topor/model_ipc.py`; offline-tested against pcbnew)

- [ ] `get_item_bounding_box(text)` returns the same box as pcbnew's `GetBoundingBox()` (the exporter grows it by 0.5 × text size; `test_ipc_parity` asserts IPC ⊇ harness).
- [ ] A KiCad 10 **hatched** copper graphic: how it arrives over IPC (filled / unfilled) — note it; unfilled = only the outline stroke is an obstacle (not safe).
- [ ] A copper **Bezier**: KiCad's DRC flattening tolerance ≤ the board max error (`test_ipc_parity` on a copy with a copper Bezier).
- [ ] Via `free` flag: a via placed as free-standing in KiCad shows `(free yes)` in the saved copy and is not ripped with `existing_copper = "rip"`; note the exact KiCad 10 place where the flag is set (guides say "via Properties → Free").

**B5 — run flow and preview** (`plugin/topor/run.py`, `preview.py`)

- [ ] The progress and preview dialogs driven by the real flow; `wx.GetApp()` present in the plugin process (R3, R4).
- [ ] Preview legible in the light theme and at 200 % (UI acceptance).
- [ ] `kicad-cli pcb export svg` background (ruling C15 Q5): does it follow the KiCad colour theme? Record the panel background colour in light and dark; the halo must stay visible on both.
- [ ] Highlight aligned with the new tracks (also on wiccon's round outline).
- [ ] `IpcBoard.read_model(layer_roles=…)` honours `[layers]` roles from `topor.toml`.
- [ ] Cancel latency: the dialog reacts within about a second (R3).

**B6 — pre-flight detection from the board file** (`plugin/topor/preflight.py`)

- [ ] Via protection: set filling + capping in KiCad → File → Board Setup → Board Stackup → Board Finish → Via protection, save a copy, set `vias.via_in_pad = true` (since 0.5.0, uncomment it): `VIA_IN_PAD_UNPROTECTED` appears without protection and disappears with it (record the token KiCad 10 writes).
- [ ] `VIA_NOT_SET_UP` appears on a board with only KiCad's factory via (0.8 / 0.4 mm, no Pre-defined Sizes → Vias) and not on eprom / wiccon if they have via sizes set up.
- [ ] Groups: on a copy with a group (select two items → right-click → Grouping → Group, save), `GROUPS_UNAVAILABLE` appears.
- [ ] `TRACK_ANGLE_RULE`: a custom rule `(constraint track_angle …)` gives the error.

**B9 — visual checks of the dialogs** (`plugin/topor/ui_wx.py`; see UI acceptance)

- [ ] Real sizes at 100 / 200 %; preview opens at about 85 % of the screen, pre-flight at 720×620 DIP clamped to 90 %.
- [ ] Button order: Cancel left of Route (GTK); Help at the left; status text between them when blocked.
- [ ] Copy details → clipboard + *Copied* for 2 s (R12).
- [ ] The preview shows *Fit* correctly right after opening.
- [ ] Esc / Enter in the modeless progress dialog; Enter in the findings list.

**B10 — install, connect, toolbar, handshake** (`plugin/topor/app.py`, `tools/build.sh`)

- [ ] KiCad honours `min_version` "3.11" and shows the action *Topor: route* in the toolbar and Tools → External Plugins (Setup).
- [ ] `import topor` / `toporlib` resolve inside the plugin environment (no `ModuleNotFoundError` in `~/.cache/kicad/10.0/topor/topor-crash.log`).
- [ ] The engine handshake error renders in the error dialog (R12).
- [ ] The first-run notice path comes from IPC `get_project().path` (R1).

**Documentation — KiCad 10 menu paths used in the guides**

- [ ] Each exists with this wording in KiCad 10.0.8 (note the real one if not): File → Board Setup →
  Design Rules → Constraints / Pre-defined Sizes → Track Widths, Vias / Net Classes / Custom Rules;
  File → Board Setup → Board Editor Layers; File → Board Setup → Board Stackup → Board Finish → Via
  protection; Preferences → Plugins (Enable KiCad API, Python interpreter, Recreate Plugin Environment);
  Tools → External Plugins; Tools → Cleanup Tracks & Vias; Inspect → Design Rules Checker; Edit → Fill All
  Zones (B); Edit → Undo; item Properties → Locked; and the paths in the *Fix* texts of
  [troubleshoot.md](troubleshoot.md#findings) (e.g. Edit → Unlock Selected, Inspect → Board Statistics,
  Edit → Ungroup) — wrong ones are fixed in `plugin/topor/messages.py`.

## Results

Fill in one row per step and board. *Result*: pass / fail / n.a.

| Board | Step | Result | Notes (numbers, report folder, what differed) |
|---|---|---|---|
| eprom-to-kernal | Setup | | |
| eprom-to-kernal | R1 first-run notice | | |
| eprom-to-kernal | R2 pre-flight contents | | |
| eprom-to-kernal | R3 progress + Cancel | | |
| eprom-to-kernal | R4 preview | | |
| eprom-to-kernal | R5 Reject, board unchanged, Undo greyed | | |
| eprom-to-kernal | R6 Accept (27/27) | | |
| eprom-to-kernal | R7 one Ctrl+Z reverts all | | |
| eprom-to-kernal | R8 KiCad DRC 0 new, 0 unconnected | | |
| eprom-to-kernal | R9 report folder | | |
| eprom-to-kernal | R10 selection scope between (U1+U2) | | |
| eprom-to-kernal | R11 Help | | |
| eprom-to-kernal | R12 / R13 optional | | |
| eprom-to-kernal | UI acceptance 100 % / 200 % / dark | | |
| eprom-to-kernal | IPC parity | | |
| wiccon-2023 | Setup | | |
| wiccon-2023 | R1 first-run notice | | |
| wiccon-2023 | R2 pre-flight contents | | |
| wiccon-2023 | R3 progress + Cancel | | |
| wiccon-2023 | R4 preview | | |
| wiccon-2023 | R5 Reject, board unchanged, Undo greyed | | |
| wiccon-2023 | R6 Accept (62/62) | | |
| wiccon-2023 | R7 one Ctrl+Z reverts all | | |
| wiccon-2023 | R8 KiCad DRC 0 new, 0 unconnected | | |
| wiccon-2023 | R9 report folder | | |
| wiccon-2023 | R10 selection scope between (U1+U2) | | |
| wiccon-2023 | R11 Help | | |
| wiccon-2023 | R12 / R13 optional | | |
| wiccon-2023 | UI acceptance 100 % / 200 % / dark | | |
| wiccon-2023 | IPC parity | | |
| both | UNVERIFIED items (list the unticked ones) | | |

Tested by: ______ · date: ______ · KiCad version (Help → About KiCad): ______ · commit: ______
