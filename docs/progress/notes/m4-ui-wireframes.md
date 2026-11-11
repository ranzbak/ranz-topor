# M4 UI design — wireframes for the Topor KiCad 10 plugin dialogs (Task B4, reviewed)

**Scope.** Six screens of the IPC plugin (wxPython 4.2.1, own process started by KiCad 10, Linux/GTK
first): (1) first-run `topor.toml` notice, (2) options + pre-flight, (3) progress, (4) preview
Accept/Reject, (5) run report + done notice, (6) error. B9 implements `WxUi(UiPort)` from this note
verbatim; B5's flow calls it: `first_run_notice` → `progress`(pre-flight DRC) → `preflight` →
`progress`… → `preview` → `done` / `error`. Reviewed (F): `.superpowers/sdd/2026-11-01-topor-m4b-plugin/task-B4-review.md`.

## 0. Common rules (apply to every dialog)

- **Native, KiCad-like.** Plain `wx.Dialog` (the plugin is a separate process, so `parent=None`,
  `CentreOnScreen()`, title `Topor — …`, `wx.DEFAULT_DIALOG_STYLE | wx.RESIZE_BORDER` where noted).
  Native widgets only: `wx.StaticText`, `wx.Button`, `wx.CheckBox`, `wx.RadioButton`, `wx.Choice`,
  `wx.SpinCtrl`, `wx.ListCtrl` (report mode), `wx.TextCtrl` (read-only), `wx.Gauge`, `wx.StaticBitmap`
  in a `wx.ScrolledWindow`. No custom-drawn controls, no cards-in-cards, no decorative colour.
- **Buttons.** Every dialog ends in a `wx.StdDialogButtonSizer`: affirmative = `wx.ID_OK` with a
  custom label (`Route`, `Accept`, `Continue`, `Close`), negative = `wx.ID_CANCEL` with a custom
  label where needed (`Reject`). The sizer puts Cancel **left** of the default on GTK/macOS and right
  on Windows — never hand-order them. Extra actions (`Open file`, `Open report folder`, `Copy
  details`, `Help`) sit in a left-aligned row of the same bottom line (`wx.ID_HELP` goes in the
  std sizer). `SetAffirmativeId(wx.ID_OK)`, `SetEscapeId(wx.ID_CANCEL)` → **Enter = default,
  Esc = Cancel** on every dialog; the window-manager close box = Esc. `AddButton` takes stock ids
  only, so the bottom line is `wx.BoxSizer(HORIZONTAL)`: extra buttons, stretch spacer, std sizer (the stretch is a
  spacer of its own: a status sizer whose children are hidden takes no stretch). The primary action of the
  pre-flight (`Route`) and the preview (`Accept`) is also visually primary: a bold label (wx/GTK draws no
  suggested-action colour on a `wx.Button`).
  Mnemonics (`&Route`) are set on every button, checkbox and radio; each dialog lists its set below
  and no letter repeats within a dialog (`Cancel`/`Close` always own `C`); the Wording tables list
  `GetLabelText()` (mnemonic stripped).
- **Sizes / HiDPI.** All sizes below are DIP; code uses `self.FromDIP(wx.Size(w, h))` /
  `FromDIP(n)`; at 200 % scaling the physical size doubles automatically. Minimum button width
  `FromDIP(88)`; vertical spacing between rows `FromDIP(6)`, between groups `FromDIP(12)`, dialog
  border `FromDIP(12)`. Numbers that change while visible (progress) live in right-aligned labels with
  a fixed `SetMinSize(FromDIP(w))` so digits do not shift the layout (native stand-in for tabular
  numerals).
- **Theme.** No hard-coded colours. Backgrounds/text come from
  `wx.SystemSettings.GetColour(wx.SYS_COLOUR_BTNFACE / WINDOW / WINDOWTEXT / GRAYTEXT / HIGHLIGHT /
  HIGHLIGHTTEXT)`; dark/light is whatever GTK gives the process (`wx.SystemSettings.GetAppearance()
  .IsDark()` only for the preview tint check). Severity is shown by **icon + word** (`wx.ArtProvider`
  `ART_ERROR / ART_WARNING / ART_INFORMATION`, `FromDIP(16)`, via a `wx.ImageList` in the list;
  if GTK renders the icons poorly at 200 % the icon is dropped and the word stays — the word is
  mandatory), never by colour alone. On top of icon + word (user 2026-11-11, "problems visible at a glance") the
  text of a finding row, the preview's DRC line, the done notice's DRC line and the progress verdict carry a
  tone colour: error red, warning amber, ok green, note `SYS_COLOUR_GRAYTEXT` (`ui_wx.TONE_RGB`, one light and
  one dark value each, chosen by the luminance of `SYS_COLOUR_WINDOW`; contrast >= 4.5:1 on GTK's default light /
  dark window). The other fixed colours are the preview's new-copper highlight and legend swatches (§4).
- **Window icon.** Every dialog (`_Base`) carries the topor icon in its title bar / task switcher: a
  `wx.IconBundle` of `icons/topor-{light,dark}-{24,64}.png` (the toolbar button's images, plugin.json),
  the dark pair when `wx.SystemSettings.GetAppearance().IsDark()`. A missing or unreadable file is skipped
  silently; without any the dialog opens without an icon.
- **Text.** Default GUI font; paths and details in a read-only `wx.TextCtrl` (selectable, copyable,
  `wx.FONTFAMILY_TELETYPE`), `wx.TE_MULTILINE | wx.TE_READONLY | wx.TE_BESTWRAP` for prose. Long
  messages wrap; nothing truncates silently. Every finding/error shows **code, cause, fix** (46 §1.1);
  a fix names only a KiCad menu path or a key that exists in `plugin/topor/schema.py`, never an
  invented one.
- **Keyboard.** Tab order = reading order (top-left → bottom-right, buttons last). Focus starts on
  the first editable control, or on the default button when there is none. Every control has a
  label: a `wx.StaticText` immediately before it, **and** — because wx/GTK sets no ATK label-for
  relation — `SetName(label)` + `SetToolTip(label)` on controls whose text is not their own
  (`Choice`, `SpinCtrl`, `TextCtrl`, `ListCtrl`, the image window).
- **States.** Each dialog defines *empty*, *busy* and *error* below; where a state does not exist
  it is stated. Never a disabled control without a visible reason.

## 1. First-run notice — `UiPort.first_run_notice(toml_path)`

```text
+------------------------------------------------------------------+
| Topor — Settings file created                                [x] |
+------------------------------------------------------------------+
| (i)  Topor created a settings file for this project:             |
|                                                                  |
|      [ /home/me/boards/eprom/topor.toml                        ] |
|                                                                  |
|      It holds every option with its default and a comment.       |
|      Design rules, widths and via sizes still come from KiCad's  |
|      Board Setup. Edit the file any time; it is read on each run.|
|                                                                  |
| [ Open file ]                                     [ Continue ]   |
+------------------------------------------------------------------+
```

Min 440×200. Focus: `Continue` (default). Keys: Enter/Esc = Continue; mnemonics Alt+O Open file
(opens the file in the system editor via `wx.LaunchDefaultApplication`, dialog stays open), Alt+C
Continue; Tab cycles path field → Open file → Continue. Path field: read-only, monospace, selectable. No empty/busy state;
error state: if the file could not be written B5 raises → dialog 6 (`CFG_CREATED` is info only).

## 2. Options + pre-flight — `UiPort.preflight(findings, summary, cfg) -> Decision`

```text
+----------------------------------------------------------------------------+
| Topor — Route eprom-to-kernal                                          [x] |
+----------------------------------------------------------------------------+
| 27 connections in scope · 12 nets · 2 copper layers · engine 0.4.0         |
|                                                                            |
| Findings — 0 errors, 2 warnings, 1 note                                    |
| +------------------------------------------------------------------------+ |
| | Severity  | Code              | Message                                | |
| |-----------+-------------------+----------------------------------------| |
| | /!\ Warn  | VIA_NOT_SET_UP    | No via sizes set up; using 0.8/0.4 mm  | |
| | /!\ Warn  | PAD_OUTSIDE_BOARD | 2 pads lie outside the board outline   | |
| | (i) Note  | DEFAULTS_USED     | 3 built-in defaults in use             | |
| +------------------------------------------------------------------------+ |
| Cause  2 pads lie outside the Edge.Cuts outline: U3.7 (GND), J1.2 (VBUS)   |
| Fix    Move the parts inside the outline, or accept to skip these pads.    |
| [ ] Accept: skip these pads (saved to topor.toml ignore_pads)              |
|                                                                            |
| Options                                                                    |
|   Scope             (o) Whole board                                        |
|                     ( ) Between selected footprints                        |
|                     ( ) Touching selected footprints                       |
|   Existing copper   [ Keep as obstacles               v ]                  |
|   [x] Respect locked tracks and vias                                       |
|   Time limit  [  60 ]^ s  [ Wall time (what you wait) v ]                  |
|               0 = no limit                                (grey)          |
|   [ ] Save these options to topor.toml                                     |
|                                                                            |
| [ Help ]                                           [ Cancel ]  [ Route ]   |
+----------------------------------------------------------------------------+
   error state, bottom line:
| [ Help ]  (x) Cannot route: 1 error — see Fix above.  [ Cancel ]  [ Route ]|  (Route disabled)
```

Resizable, min 640×420, opens at its fitted size clamped to 90 % of the display client area: the findings
list is as tall as its rows (up to 8, then it scrolls and takes the extra height; user 2026-11-11), the Cause pane
3 lines. Without errors and warnings the heading reads `No problems found` (with the note count; green); without
any finding the list and the Cause pane are hidden. Default `Route`.

**Behaviour.**

- Header line = `RunSummary` (connections in scope, nets, layers, engine version). The connection
  count follows the Scope radio live, from `RunSummary.connections_by_scope[mode]` (B6/B7).
- Findings list: `wx.ListCtrl` report mode, sorted error → warning → info, then by code; icon + word
  in column 1 from `Finding.severity`: `error` → `Error`, `warning` → `Warn`, `info` → `Note`.
  Selecting a row fills the Cause/Fix pane (read-only `TextCtrl`, 3 lines min, wraps). **One**
  `Accept` checkbox sits under the pane, shown only while the selected row's code is acceptable
  and relabelled per code (`PAD_OUTSIDE_BOARD` → `accept_pads`, `DRILL_OUTSIDE_FILTER` →
  `accept_drills`); its checked state is kept per code, and the checked codes form
  `Decision.accepted_codes`. Row 1 is selected on open. Message column = `Finding.message`; items
  (`Finding.items`) appear in the Cause text, up to 10, then "… and {n} more (all in the report)".
- **Error present → `Route` disabled** and the status text left of the buttons (error icon + text)
  reads `Cannot route: {n} error(s) — see Fix above.` (so the disabled button has a visible
  reason). Warnings never block. `fail_on_preexisting` turns `PREEXISTING_DRC` into an error (B6).
- Scope radios: enabled only when the KiCad selection contains footprints; otherwise the two
  selection radios are disabled and `Whole board` is checked, with the grey hint
  `(nothing selected in KiCad)` after it. Initial value = `cfg.scope_mode` if a selection exists.
- Existing copper `wx.Choice`: `Keep as obstacles` (`fixed`), `Keep and smooth` (`smooth`),
  `Rip up and re-route` (`rip`). Respect locked = `respect_locked`; Time limit = `timeout_s`
  (`SpinCtrl` 0…86 400, step 10). (Keep partial routes and Threads were removed in the CL cleanup,
  2026-11-09: the topological core never read `partial_routes` or `threads`.) `Save these options to topor.toml` → `Decision.save_to_toml` (B1 `update_config`,
  comments kept). Initial values = `cfg`. Every other `topor.toml` key (`unrouted_only`,
  `[iterations]`, `[vias]`, …) is file-only and never gets a dialog control.
- Returns `Decision(proceed, accepted_codes, overrides)`; Cancel/Esc → `proceed=False`, nothing
  written (not even accepted codes).
- **Empty:** no findings → list has zero rows; the Cause pane shows
  `No findings. The board setup covers everything Topor needs.`; `Route` enabled. **Busy:** none —
  pre-flight (incl. the kicad-cli DRC on the copy) runs *before* this dialog under dialog 3 with phase
  `Checking the board (KiCad DRC)`. **Error:** see "Error present".
- Keys: Tab list → Cause pane → Accept → Scope radios (arrows) → Existing copper → Respect locked →
  Time limit → Save → Help → Cancel → Route. Enter = Route; inside the
  GTK list Enter raises `EVT_LIST_ITEM_ACTIVATED`, which B9 binds to the Route handler (no-op while
  disabled). Esc = Cancel. Mnemonics: Alt+R Route, Alt+A Accept, Alt+W Whole board, Alt+B Between,
  Alt+T Touching, Alt+L Respect locked, Alt+S Save, Alt+H Help (opens
  `docs/user/operate.md#options` via the default browser), Alt+C Cancel. Mnemonics only on
  buttons/checkboxes/radios (GTK `wx.StaticText` mnemonics do not move focus); `Choice` and spin
  controls are reached by Tab.

## 3. Progress — `UiPort.progress(p) -> bool`

```text
+-----------------------------------------------------------+
| Topor — Routing eprom-to-kernal                       [x] |
+-----------------------------------------------------------+
| Routing — negotiation pass                                |
|                                                           |
| [==================------------------------]  12 / 27     |
|                                                           |
| Iteration   3 of up to 30        Routed    12 / 27        |
| Vias        4                    Elapsed   0:12 / 1:00    |
|                                                           |
|              Run Round Errors Open Vias    Length         |
| Last round     3     2      5    0  168   7.412 m         |
| Best so far    1     1      3    0  166   7.384 m         |
| Last round is worse than the best: errors +2.  (amber)    |
|                                                           |
| Time limit  [   60 ] s wall time  [Set limit] [+5 min] [+15 min] |
| 0 = no limit. A limit below the time spent stops the run  |
| now, keeping the best result.                             |
|                                                           |
| Cancel keeps the best result so far.                      |
|                                                           |
|                                              [ Cancel ]   |
+-----------------------------------------------------------+
```

Min 460×240, not resizable. Opened on the first `progress()` call (modeless `Show()`), updated on
every call (B5 polls every 0.25 s) and **closed only by the next other `UiPort` call**
(`preflight`, `preview`, `error`, `done`) — never on a phase value, because the plugin phases
`drc` and `preview` come *after* the engine's `done`, and the DRC loop may re-enter `route`. The
dialog never blocks: `WxUi.progress` updates the widgets, calls `wx.YieldIfNeeded()` and returns
`not self._cancelled`.

- Phase label text = `Progress.phase` mapped through a table (B9). Engine phases (`control.rs`,
  exactly these): `setup` → `Preparing geometry`, `route` → `Routing` (K1: `Routing — {stage}` when the topological core names
  its stage in `Progress.stage`: projection, topological search, layer assignment, drawing, via reduction, pour
  stitching), `done` →
  `Routing finished`. Plugin phases (B5 sets them on its own `Progress` copies): `preflight_drc` →
  `Checking the board (KiCad DRC)`, `drc` → `Verifying with KiCad DRC`, `preview` →
  `Rendering the preview`. Anything else → the raw phase string. Gauge: determinate `routed/total`
  only when phase is `route` and `total > 0`; otherwise `Pulse()` (indeterminate) every call.
  Row *Round* = the engine's whole-route round `{round} of up to {rounds}` (`[optimize] rounds`, K1; before
  K1 the iteration of up to `[iterations] max`). Row *Router time* = router time over all DRC-loop rounds
  against `run.timeout_s`; row *Total time* = wall time since the run started (KiCad DRC, preview included).
- **Score** (user 2026-11-11: "It would help if I can see the current score, and the best score in the UI"): rows
  *Last round* (the round any run judged last) and *Best so far* (the best over all runs: exactly the board OK /
  Cancel keeps and the final pick takes) with run, round, errors, open, vias, length — the columns are the pick
  order (`[optimize] pick`, default `errors_vias_length`: fewest errors, then open, vias, length, shrinkages).
  Until the first round ends the table's place reads `Waiting for round 1…` (grey; the slot keeps the table's
  size). The verdict is green when the last round beat the best (`ui_wx.verdict_tone`), amber when it is worse. The line below says how the last round compares, on the first criterion that differs
  (`Progress.last_vs_best` / `last_decider` / `last_delta`): *Last round is worse than the best: errors +2.*, *… the
  new best: length -0.030 m.*, *The first result is the best so far.*, *… scores the same as the best; the earlier
  result stays.* or, from a lower run, *… and is the best now (ties go to the lower run).* The run flow writes one line per ended round to log.txt (`round ended: run 3, round 2: errors 5
  …: worse than the best (run 1, round 1): errors +2`).
- Numbers right-aligned in fixed-width labels, each as wide as the widest text it can show (digits as 8, in the
  UI font: its digits are tabular); the phase line has a fixed width and ends in `…` (full text in its tooltip);
  the verdict is a fixed box of two lines — nothing in the dialog moves or resizes while it runs. The hint under
  the buttons is one line, `Cancel keeps the best result so far.`, the full explanation its tooltip. Elapsed and
  limit both `m:ss` (`h:mm:ss` from one hour); limit shown as `/ {limit}` or `/ no limit` when `timeout_s == 0` (then Cancel is the only
  stop — the hint line says so).
- **Cancel** (default and escape; Enter also triggers it): label becomes
  `Cancel pressed: finishing the current stage (<stage>)… N s` (N counts up until the engine
  returns; the DRC and preview that follow keep their labels), button disabled, gauge pulses; the method returns
  `False` from then on; B5 calls `job.cancel()` and continues with DRC + preview when anything was
  routed, else ends with outcome `cancelled` (§5).
- **Empty:** before the first engine iteration `routed/total` shows `– / {total}`. **Busy:** is the
  normal state. **Error:** none; an exception closes this dialog and opens dialog 6.
- **Time limit during the run** (user 2026-11-07: "make the max run time editable during the run, so you don't
  lose progress when a run takes longer"): row *Time limit* = a number field (seconds on the run's clock,
  `run.timeout_clock`: unit `s wall time` or `s CPU time`), **Set limit**, **+5 min**, **+15 min**, and the hint
  below. +5 / +15 add to the limit in force; the field takes any value: raised, lowered down to the router time
  already spent (a lower value becomes the time spent: the run stops at its next check and keeps the best result,
  as when the limit fires), or 0 = no limit (then the + buttons are disabled: nothing to add to). The row is
  enabled only in the routing run after the pre-flight (`WxUi` passes `limit_editable`), and disabled after
  Cancel / OK, once the limit is reached and in the DRC / preview phases (the limit counts router time only). The
  change waits in `ProgressDialog.limit_request` until the run flow takes it (`WxUi.time_limit_request`, polled
  after every `progress` call); the run flow applies it (wall clock: the engine job's live limit,
  `RouteJob.set_time_limit`; CPU clock: its own poll loop), logs it ("time limit extended by the user …"), records
  start, every change and the final limit in `result.json` (`time_limit`) and the report (Stopped line, Settings,
  the time-budget warning), and reports the limit in force back in `Progress.limit_s`, which the elapsed line and
  the field show. Headless runs (harness, MCP) have no such control: their limit stays fixed.
- Keys: Esc/Alt+C = Cancel; Enter = Cancel, except in the limit field (Enter = Set limit) and on a focused limit
  button (that button). Tab order: limit field, Set limit, +5 min, +15 min, Cancel; Cancel has the focus at the
  start. Mnemonics: Alt+L Set limit, Alt+5 / Alt+1 the + buttons.

## 4. Preview — `UiPort.preview(png, report) -> bool`

```text
+--------------------------------------------------------------------------------------+
| Topor — Preview: eprom-to-kernal                                                 [x] |
+--------------------------------------------------------------------------------------+
| Zoom [ Fit v ]                      Legend: ■ F.Cu  ■ B.Cu  ■ new copper (highlighted) |
| Current board                       Result                              Summary          |
| +--------------------------------+ +--------------------------------+ ⚠ DRC new 9 (red) |
| |  (kicad-cli SVG export,        | |  (same export of the routed    | Routed 27/27 …   |
| |   KiCad theme colours,         | |   copy; new copper drawn with  | DRC new 9 · …    |
| |   F.Cu + B.Cu + Edge.Cuts;     | |   a highlight halo); scrolls   | Vias 0 …   (bold)|
| |   own image, Fit to its pane)  | |   together with the left pane  | Length …         |
| +--------------------------------+ +--------------------------------+ the rest, smaller|
| Findings — 0 errors, 1 warnings, 1 notes                                               |
| [ Severity | Code | Message  (the pre-flight's list) ]  [ Cause / Fix of the row ]     |
| Report folder [ ~/.cache/kicad/10.0/topor/eprom-to-kernal/20261101-101530 ]  (1 line)  |
|                                                                                      |
| [ Open report folder ]                                        [ Reject ]  [ Accept ] |
+--------------------------------------------------------------------------------------+
```

Resizable, min 800×560 (the image area scrolls, so the minimum is set by the summary pane and the
buttons), opens at 85 % of the display (`wx.Display.GetClientArea()`), remembers its size per
session only. Default `Accept`, Esc = `Reject`. Accept = one KiCad commit (one undo step
`Edit → Undo`), Reject = nothing written (B5).

- Images (user 2026-11-11): the two boards as separate PNGs B5 writes next to the composite
  (`preview-before.png` / `preview-after.png`, `preview.panel_paths`, rasterised at `PANEL_HIRES` = 2 x the
  composite's scale), each in its own `wx.ScrolledWindow` (`wx.WANTS_CHARS`, `SetCanFocus(True)`, 1 px
  `SYS_COLOUR_GRAYTEXT` border) under its own heading (`Current board` / `Result`), the `wx.StaticBitmap` centred;
  `Zoom` choice `Fit` (default: each image fits its own pane, keeps aspect) / `100 %` (one composite pixel per DIP)
  / `200 %`; the panes scroll together; mouse wheel scrolls, Ctrl+wheel steps the zoom. The legend row draws one
  swatch per entry of `preview-legend.json` (the composite's legend). An older report folder with only the
  composite `preview.png` shows that in the first pane and hides the second.
- **Highlight (the one hard-coded colour).** New copper is drawn in its layer colour with a halo
  `2 × width` underneath in `PREVIEW_HIGHLIGHT_RGB = (0, 139, 139)` (#008B8B). Contrast: white
  #FFFFFF 4.15:1, KiCad dark board #001023 4.61:1, dark window #2D2D2D 3.32:1, but Adwaita-dark
  #353535 only 2.96:1 — hence the runtime check below (WCAG 3:1 for graphics, both themes).
  Distinguishable from F.Cu #C83434 and B.Cu #4D7FC4 by hue only (luminance contrast against the
  copper colours is ≈ 1:1), which is why the halo *width* (shape) and the existing copper drawn at
  60 % alpha in the Result panel carry the distinction — the cue is never colour only.
  B5 computes the contrast of the tint against the actual panel background at render time (relative
  luminance, WCAG formula) and falls back to `PREVIEW_HIGHLIGHT_FALLBACK_RGB = (208, 64, 208)`
  (#D040D0: 3.94 / 4.85 / 3.49 / 3.11:1 on the same four backgrounds) if the primary is < 3:1.
  Panel background = KiCad theme `board.background` when the export carries it (**UNVERIFIED** for
  `kicad-cli pcb export svg`; GUI checklist item), else `SYS_COLOUR_WINDOW`. Both constants live in
  `plugin/topor/preview.py` (B5); B9 imports them for the legend swatch.
- Summary pane (`ui_wx.summary_parts`): with new DRC violations a red bold first line `⚠ DRC new {n} ({types}) —
  see findings`; then the key facts bold (the summary lines *Routed*, *DRC new*, *Vias*, *Length*); then the other
  summary lines (≤ 20, §5) smaller in a borderless read-only `TextCtrl` (selectable), without the `Report:` line.
  Findings: the run's findings (`report.facts["findings"]`, after-routing ones included) in the pre-flight's list
  widget (`_FindingsMixin`: icon, word, colour; up to 5 rows before it scrolls) with the Cause / Fix pane beside it.
  Report folder: read-only one-line monospace field with the full path (shown once); `Open report folder` →
  `wx.LaunchDefaultApplication(dir)`, always enabled: B5 writes `report.md` and `preview.png` into
  the folder *before* calling `preview()` and rewrites the `Outcome:` line afterwards.
- **Empty:** image missing/unreadable → the image area shows `Preview could not be rendered
  (see log.txt in the report folder)`; Accept stays enabled (the DRC numbers in the summary are the
  decision basis). **Busy:** none. **Error:** none (exceptions → dialog 6).
- Keys: Tab Zoom → Current board image → Result image (arrows/PgUp/PgDn scroll) → Summary → findings list →
  Cause / Fix → Report folder → Open report folder → Reject → Accept. Enter = Accept, Esc = Reject. Mnemonics: Alt+O Open report folder, Alt+A Accept,
  Alt+R Reject.

## 5. Run report (`report.md`, B8) and done notice — `UiPort.done(outcome: str, report, report_dir: Path) -> None`

Written to `<report_dir>/report.md` next to `preview.png`, `result.json`, `drc.json`, `log.txt`.
`report_dir` default (C1): `${TOPOR_REPORT_ROOT:-${XDG_CACHE_HOME:-~/.cache}/kicad/10.0/topor}/
<project>/<YYYYmmdd-HHMMSS>/`; `[output] report_dir` relative → `<project_dir>/<report_dir>/
<timestamp>/`, absolute → `<report_dir>/<timestamp>/`. The same path is shown in dialogs 4 and 6.

**Summary block** (`Report.summary_lines()`, ≤ 20 lines, identical in the preview pane, the
`report.md` top section and the `Done` notice; one fact per line, numbers first):

```text
Topor {version} — {board} — {timestamp}
Routed {routed}/{total} connections · {nets} nets · {layers} layers
Partial {partial} · failed {failed}
Vias {vias} (current board {vias_before})
Length outside pads {length_mm} mm (current {length_before_mm} mm)
DRC new {routing_new} · pre-existing {preexisting} · unconnected {unconnected}
Detours {detour_line}
Stopped: {stop_reason} after {iterations} iterations, {elapsed_s} s, {threads} threads
Scope {scope} · existing copper {existing_copper} · locked {respect_locked}
Settings: {non_default_count} non-default, {defaults_used_count} built-in defaults
Findings: {errors} errors · {warnings} warnings · {notes} notes
Outcome: {outcome}
Report: {report_dir}
```

`{outcome}` maps 1:1 to B5's `RunOutcome.status`: `accepted` → `applied as one undo step`,
`rejected` → `rejected — board unchanged`, `cancelled` → `cancelled — nothing applied` (Cancel with
nothing routed, so no preview was shown), `aborted_preflight` → `aborted at pre-flight`, `error` →
`error {code}`. Cancel followed by Accept/Reject is `accepted`/`rejected` with `Stopped: cancel`.

**`report.md` structure** (headings in the Wording table): 1 Summary (block above) · 2 Connections
(table: net, from → to, status routed/partial/failed, reason + blocking point for non-routed, rips)
· 3 Vias (per net: result vs current; vias in pads listed) · 4 Length (per net, vs current, detour
ratio) · 5 DRC (`routing_new` by type, `preexisting` list, unconnected items; which DRC round) ·
6 Findings (pre-flight list with code, cause, fix, accepted?) · 7 Settings (non-defaults with
source, built-in defaults used, accepted fallbacks, `topor.toml` path) · 8 Files (the five files) ·
9 Versions (engine, plugin, kipy, KiCad, kicad-cli). Empty sections print `none`.

**Done notice** — `UiPort.done(outcome, report, report_dir)`, called by B5 after Accept, Reject or a
Cancel without preview (never after `error`, which has its own dialog); modal, min 440×220:

```text
+------------------------------------------------------------------+
| Topor — Finished                                             [x] |
+------------------------------------------------------------------+
| (i)  Routed 27/27 connections — applied to the board as one      |
|      undo step (Edit → Undo).                                     |
|      Zones were not refilled: press B (Edit → Fill All Zones) to  |
|      refill them — a separate undo step.                          |
|      Report folder                                               |
|      [ ~/.cache/kicad/10.0/topor/eprom-to-kernal/20261101-101530 ]|
| [ Open report folder ]                               [ Close ]   |
+------------------------------------------------------------------+
```

Text by `outcome`: `accepted` → `done_accepted` (prefixed "Stopped early by Cancel: " when
`stop_reason == "cancel"`, row `done_accepted_cancelled`), `rejected` → `done_rejected`,
`cancelled` → `done_cancelled`. After `accepted` only (FX1, rulings C28 / C30): `done_refill_hint`
when `[run] refill_after_accept = false` (the default; report fact `zones_refilled` false) or
`done_refilled` when topor refilled the zones after the commit; one `done_post_commit` line per
POST_COMMIT_STEP_FAILED finding (report fact `post_commit_failed`). After `accepted` with new DRC violations
(report facts `routing_new` > 0, `drc_by_type_new`; user 2026-11-11) the icon is `ART_WARNING` instead of the
information icon and the first line is `done_drc_new` (amber, bold): `⚠ {n} new DRC violations ({types}) — run
Inspect → Design Rules Checker`. Focus `Close` (default); Enter/Esc = Close; mnemonics Alt+O Open
report folder, Alt+C Close. No empty/busy/error state.

## 6. Error — `UiPort.error(finding)`

```text
+------------------------------------------------------------------+
| Topor — Error                                                [x] |
+------------------------------------------------------------------+
| (x)  KICAD_CLI_MISSING                                           |
|      kicad-cli was not found, so the result cannot be verified.  |
|                                                                  |
|      Fix                                                         |
|      Install KiCad 10 (kicad-cli at /usr/bin/kicad-cli), or set   |
|      the KICAD_CLI environment variable before starting KiCad.   |
|                                                                  |
|      Details                                                     |
|      [ code: KICAD_CLI_MISSING                                 ] |
|      [ log:  ~/.cache/kicad/10.0/topor/eprom/…/log.txt         ] |
|      [ engine 0.4.0 · plugin 0.4.0 · kipy 0.8.0 · KiCad 10.0.8  ] |
|                                                                  |
| [ Copy details ] [ Open report folder ]               [ Close ]  |
+------------------------------------------------------------------+
```

Resizable, min 520×320. Headline = `Finding.code` (bold) then `Finding.message`; `Fix` =
`Finding.fix` (wraps). Details = read-only monospace `TextCtrl` with code, message, fix, items (≤ 20
then `… and {n} more`), log path, versions, timestamp — what a bug report needs
(`docs/dev/debug.md`). `Copy details` → `wx.TheClipboard` (label flips to `Copied` for 2 s via
`wx.CallLater`, then back — the label change is the only feedback, no toast).
`Open report folder` disabled with hint `(no report folder yet)` when the run failed before the
folder existed (the only dialog where this state exists). Focus `Close` (default); Enter/Esc =
Close; mnemonics Alt+D Copy details (`Copy &details` — `C` belongs to Close), Alt+O Open report
folder, Alt+C Close. Tab: Details → Copy details → Open report folder → Close. The example fix
above names the real resolution (`toporlib/kicad_cli.py`: `$KICAD_CLI`, else `/usr/bin/kicad-cli`);
B6's `messages.py` owns the final text. For `INTERNAL` (panic/exception) the message is B5's generic
text and the fix names `log.txt` + the bug-report guide. Nothing is written to the board on this path.

## 7. Sizes, keyboard and theme — checklist for the GUI test

| Dialog | Min DIP | At 200 % (px) | Default | Esc | Resizable |
|---|---|---|---|---|---|
| first_run | 440×200 | 880×400 | Continue | Continue | no |
| preflight | 640×480 | 1280×960 | Route (disabled on error) | Cancel | yes |
| progress | 460×240 | 920×480 | Cancel | Cancel | no |
| preview | 800×520 | 1600×1040 | Accept | Reject | yes |
| report (done) | 440×220 | 880×440 | Close | Close | no |
| error | 520×320 | 1040×640 | Close | Close | yes |

Every minimum fits a 1920×1080 panel at 200 % (GTK scales in integer steps, so 13″ 1080p laptops do
run at 200 %) and a 1366×768 panel at 100 %. Opening sizes are clamped to 90 % of
`wx.Display.GetClientArea()`; the findings list and the preview image scroll, nothing else needs
to. Dark theme: every surface is a system colour; icons from the GTK icon theme; highlight tint
checked ≥ 3:1 at runtime (§4). GUI checklist acceptance (B11): tab order as listed, Esc = Cancel,
Enter = default, mnemonics as listed, readable at 100 % / 200 %, dark theme, and — UNVERIFIED until
then — whether the `kicad-cli pcb export svg` background follows the KiCad theme (check the preview
in light and dark; the halo must stay visible on both).

## 8. Skill guidance applied

- **ecc:frontend-design-direction** — taken: one explicit direction, *utilitarian, dense, quiet,
  scannable* (a tool used on every board, not a landing page); the first viewport is the workflow
  itself (findings + options, then Route); stable dimensions (fixed-width number labels, min sizes,
  list grows not the layout); hierarchy = count line → findings → options → one default button;
  existing design system = KiCad's own dialogs (`StdDialogButtonSizer`, native widgets, `Board
  Setup`-style paths in messages). Rejected as web-only: palettes/CSS variables (system colours
  instead), typography choices (default GUI font), motion, hero/asset guidance.
- **ecc:make-interfaces-feel-better** — taken: tabular numbers → fixed-width right-aligned labels;
  explicit states (empty/busy/error/disabled-with-reason) per dialog; "Copied" feedback on Copy
  details; usable hit areas via native button sizes + `FromDIP`; optical consistency = equal
  spacing constants; a neutral outline on the preview image (1 px `SYS_COLOUR_GRAYTEXT` border of the
  scrolled window) so it does not blur into the surface. Rejected as web-only: concentric radii,
  shadows, `text-wrap`, font smoothing, CSS transitions/press scale, `will-change`.
- **ecc:accessibility** — taken: every control has a text label (name/role from native widgets);
  severity and highlight never colour-only (icon + word, halo + dimming); contrast ≥ 3:1 for the
  only custom colour, verified on both themes and re-checked at runtime; modal focus containment
  (wx modal dialogs) and Esc to leave; focus order and keyboard map per dialog; error messages carry
  a correction (Fix) and the disabled Route states why; no redundant entry (options pre-filled from
  `topor.toml`, "Save" writes them back). Rejected/not applicable: ARIA roles, iOS/Android
  mappings, `aria-live`. **Known wx/GTK limitations:** no live region (progress label changes are
  not announced; the `wx.Gauge` is at least exposed as a progress bar), and no label-for relation
  from a `wx.StaticText` (mitigated by `SetName`/`SetToolTip`, §0).

## Wording

Text is the exact user-visible string (`GetLabelText()`, mnemonic stripped); `{…}` = runtime value;
a literal `\n` in a cell = newline. B9's `test_wx_ui_wording_matches_wireframes` parses these six
tables (`dialog` ∈ first_run/preflight/progress/preview/report/error, `kind` ∈
title/label/button/checkbox/choice/heading/text).

### first_run

| dialog | id | kind | text |
|---|---|---|---|
| first_run | title | title | Topor — Settings file created |
| first_run | intro | label | Topor created a settings file for this project: |
| first_run | toml_path | text | {toml_path} |
| first_run | explain | label | It holds every option with its default and a comment. Design rules, widths and via sizes still come from KiCad's Board Setup. Edit the file any time; it is read on each run. |
| first_run | open_file | button | Open file |
| first_run | continue | button | Continue |

### preflight

| dialog | id | kind | text |
|---|---|---|---|
| preflight | title | title | Topor — Route {board} |
| preflight | summary_line | label | {connections} connections in scope · {nets} nets · {layers} copper layers · engine {engine_version} |
| preflight | findings_heading | heading | Findings — {errors} errors, {warnings} warnings, {notes} notes |
| preflight | col_severity | label | Severity |
| preflight | col_code | label | Code |
| preflight | col_message | label | Message |
| preflight | sev_error | text | Error |
| preflight | sev_warn | text | Warn |
| preflight | sev_info | text | Note |
| preflight | cause_label | label | Cause |
| preflight | fix_label | label | Fix |
| preflight | no_findings | text | No findings. The board setup covers everything Topor needs. |
| preflight | no_problems | heading | No problems found |
| preflight | no_problems_notes | heading | No problems found ({notes} notes) |
| preflight | items_more | text | … and {n} more (all in the report) |
| preflight | accept_pads | checkbox | Accept: skip these pads (saved to topor.toml ignore_pads) |
| preflight | accept_drills | checkbox | Accept: add these drills to topor.toml allowed_drills_mm |
| preflight | options_heading | heading | Options |
| preflight | scope_label | label | Scope |
| preflight | scope_whole | choice | Whole board |
| preflight | scope_between | choice | Between selected footprints |
| preflight | scope_touching | choice | Touching selected footprints |
| preflight | scope_hint | label | (nothing selected in KiCad) |
| preflight | existing_label | label | Existing copper |
| preflight | existing_fixed | choice | Keep as obstacles |
| preflight | existing_smooth | choice | Keep and smooth |
| preflight | existing_rip | choice | Rip up and re-route |
| preflight | respect_locked | checkbox | Respect locked tracks and vias |
| preflight | drawing_style_label | label | Trace shape |
| preflight | drawing_style_octilinear | choice | 45° (octilinear) |
| preflight | drawing_style_smooth | choice | Any angle (smooth) |
| preflight | timeout_label | label | Time limit |
| preflight | timeout_unit | label | s |
| preflight | timeout_hint | label | 0 = no limit |
| preflight | save_toml | checkbox | Save these options to topor.toml |
| preflight | blocked_status | label | Cannot route: {n} error(s) — see Fix above. |
| preflight | help | button | Help |
| preflight | cancel | button | Cancel |
| preflight | route | button | Route |

### progress

| dialog | id | kind | text |
|---|---|---|---|
| progress | title | title | Topor — Routing {board} |
| progress | phase_setup | text | Preparing geometry |
| progress | phase_route | text | Routing |
| progress | phase_route_stage | text | Routing — {stage} |
| progress | phase_done | text | Routing finished |
| progress | phase_preflight_drc | text | Checking the board (KiCad DRC) |
| progress | phase_drc | text | Verifying with KiCad DRC |
| progress | phase_preview | text | Rendering the preview |
| progress | phase_cancelling | text | Cancel pressed: finishing the current stage{stage}… {s} s |
| progress | gauge_value | text | {routed} / {total} |
| progress | iteration_label | label | Round |
| progress | iteration_value | text | {iteration} of up to {cap} |
| progress | routed_label | label | Routed |
| progress | routed_value | text | {routed} / {total} |
| progress | routed_pending | text | – / {total} |
| progress | vias_label | label | Vias |
| progress | elapsed_label | label | Router time |
| progress | elapsed_value | text | {elapsed} / {limit} · CPU {cpu} |
| progress | elapsed_cpu_value | text | {elapsed} · CPU {cpu} / {limit} |
| progress | elapsed_nolimit | text | {elapsed} · CPU {cpu} / no limit |
| progress | total_label | label | Total time |
| progress | limit_label | label | Time limit |
| progress | limit_unit_wall | text | s wall time |
| progress | limit_unit_cpu | text | s CPU time |
| progress | limit_set | button | Set limit |
| progress | limit_add5 | button | +5 min |
| progress | limit_add15 | button | +15 min |
| progress | limit_hint | text | 0 = no limit. A limit below the time spent stops the run now, keeping the best result. |
| progress | cancel_hint | label | Cancel keeps the best result so far. |
| progress | cancel_hint_tip | text | Cancel keeps the best result so far. Once the routes are stable the button reads OK: the same action, and the best result shown is exactly the board you get; waiting longer may still find small gains. The time limit counts router time only and is an emergency stop; KiCad's DRC and the preview come on top. |
| progress | cancel | button | Cancel |
| progress | ok | text | OK |
| progress | phase_stable | text | Routes stable |
| progress | title_stable | text | Topor — Routing {board} — Routes stable |
| progress | phase_finishing | text | OK pressed: finishing the current stage{stage}… {s} s |
| progress | round_value | text | {round} |
| progress | gain_label | label | Gain |
| progress | gain_value | text | {gain} % / {window} rounds |
| progress | gain_pending | text | – |
| progress | runs_label | label | Runs |
| progress | runs_value | text | {stable} of {runs} stable |
| progress | score_head_run | label | Run |
| progress | score_head_round | label | Round |
| progress | score_head_errors | label | Errors |
| progress | score_head_open | label | Open |
| progress | score_head_vias | label | Vias |
| progress | score_head_length | label | Length |
| progress | score_last | label | Last round |
| progress | score_best | label | Best so far |
| progress | score_length | text | {length} m |
| progress | score_pending | text | – |
| progress | score_waiting | text | Waiting for round 1… |
| progress | score_hint | text | Scores compare in the pick order: fewest errors (open connections and connections with rule violations), then fewest open, fewest vias, the shortest length ([optimize] pick). The best so far is the board OK or Cancel keeps. |
| progress | vs_first | text | The first result is the best so far. |
| progress | vs_new_best | text | Last round is the new best: {delta}. |
| progress | vs_worse | text | Last round is worse than the best: {delta}. |
| progress | vs_same | text | Last round scores the same as the best; the earlier result stays. |
| progress | vs_tie | text | Last round scores the same as the best and is the best now (ties go to the lower run). |
| progress | vs_better | text | Last round is better ({delta}), but the variant table kept the best. |
| progress | delta_errors | text | errors {d} |
| progress | delta_open | text | open {d} |
| progress | delta_vias | text | vias {d} |
| progress | delta_length | text | length {d} m |
| progress | delta_shrinkages | text | shrinkages {d} |
| progress | delta_score | text | weighted score {d} mm |
| progress | delta_area | text | area {d} mm² |

### preview

| dialog | id | kind | text |
|---|---|---|---|
| preview | title | title | Topor — Preview: {board} |
| preview | zoom_label | label | Zoom |
| preview | zoom_fit | choice | Fit |
| preview | zoom_100 | choice | 100 % |
| preview | zoom_200 | choice | 200 % |
| preview | legend_label | label | Legend: F.Cu · B.Cu · new copper (highlighted) |
| preview | before_heading | heading | Current board |
| preview | after_heading | heading | Result |
| preview | summary_heading | heading | Summary |
| preview | drc_alert | text | ⚠ DRC new {n} ({types}) — see findings |
| preview | no_image | text | Preview could not be rendered (see log.txt in the report folder) |
| preview | report_dir_label | label | Report folder |
| preview | report_dir | text | {report_dir} |
| preview | open_report_folder | button | Open report folder |
| preview | reject | button | Reject |
| preview | accept | button | Accept |

### report

| dialog | id | kind | text |
|---|---|---|---|
| report | title | title | Topor — Finished |
| report | done_accepted | text | Routed {routed}/{total} connections — applied to the board as one undo step (Edit → Undo). |
| report | done_accepted_cancelled | text | Stopped early by Cancel: routed {routed}/{total} connections — applied to the board as one undo step (Edit → Undo). |
| report | done_rejected | text | Nothing was changed on the board. |
| report | done_cancelled | text | Cancelled — nothing was changed on the board. |
| report | done_refill_hint | text | Zones were not refilled: press B (Edit → Fill All Zones) to refill them — a separate undo step. |
| report | done_refilled | text | Zones were refilled by topor — a second undo step: the first Edit → Undo reverts the refill, the next one the routes. |
| report | done_post_commit | text | {message} {fix} |
| report | done_drc_new | text | ⚠ {n} new DRC violations ({types}) — run Inspect → Design Rules Checker |
| report | report_dir_label | label | Report folder |
| report | report_dir | text | {report_dir} |
| report | open_report_folder | button | Open report folder |
| report | close | button | Close |
| report | md_title | heading | Topor run report — {board} — {timestamp} |
| report | sec_summary | heading | 1. Summary |
| report | sec_connections | heading | 2. Connections |
| report | sec_vias | heading | 3. Vias |
| report | sec_length | heading | 4. Length |
| report | sec_drc | heading | 5. DRC |
| report | sec_findings | heading | 6. Findings |
| report | sec_settings | heading | 7. Settings |
| report | sec_files | heading | 8. Files |
| report | sec_versions | heading | 9. Versions |
| report | sec_empty | text | none |
| report | sum_header | text | Topor {version} — {board} — {timestamp} |
| report | sum_routed | text | Routed {routed}/{total} connections · {nets} nets · {layers} layers |
| report | sum_partial | text | Partial {partial} · failed {failed} |
| report | sum_vias | text | Vias {vias} (current board {vias_before}) |
| report | sum_length | text | Length outside pads {length_mm} mm (current {length_before_mm} mm) |
| report | sum_drc | text | DRC new {routing_new} · pre-existing {preexisting} · unconnected {unconnected} |
| report | sum_detours | text | Detours {detour_line} |
| report | sum_stopped | text | Stopped: {stop_reason} after {iterations} iterations, {elapsed_s} s, {threads} threads |
| report | sum_scope | text | Scope {scope} · existing copper {existing_copper} · locked {respect_locked} |
| report | sum_settings | text | Settings: {non_default_count} non-default, {defaults_used_count} built-in defaults |
| report | sum_findings | text | Findings: {errors} errors · {warnings} warnings · {notes} notes |
| report | sum_outcome | text | Outcome: {outcome} |
| report | sum_report | text | Report: {report_dir} |
| report | outcome_accepted | text | applied as one undo step |
| report | outcome_rejected | text | rejected — board unchanged |
| report | outcome_cancelled | text | cancelled — nothing applied |
| report | outcome_preflight | text | aborted at pre-flight |
| report | outcome_error | text | error {code} |

### error

| dialog | id | kind | text |
|---|---|---|---|
| error | title | title | Topor — Error |
| error | code | heading | {code} |
| error | message | label | {message} |
| error | fix_label | label | Fix |
| error | fix | label | {fix} |
| error | details_label | label | Details |
| error | details | text | code: {code}\nmessage: {message}\nfix: {fix}\nitems: {items}\nlog: {log_path}\nengine {engine_version} · plugin {plugin_version} · kipy {kipy_version} · KiCad {kicad_version}\n{timestamp} |
| error | items_more | text | … and {n} more |
| error | copy_details | button | Copy details |
| error | copied | button | Copied |
| error | open_report_folder | button | Open report folder |
| error | no_report_folder | label | (no report folder yet) |
| error | close | button | Close |
