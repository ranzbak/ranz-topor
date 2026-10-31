# M4 UI design — wireframes for the Topor KiCad 10 plugin dialogs (Task B4, draft for review)

**Scope.** Six screens of the IPC plugin (wxPython 4.2.1, own process started by KiCad 10, Linux/GTK
first): (1) first-run `topor.toml` notice, (2) options + pre-flight, (3) progress, (4) preview
Accept/Reject, (5) run report, (6) error. B9 implements `WxUi(UiPort)` from this note verbatim; B5's
flow calls it: `first_run_notice` → `preflight` → `progress`… → `preview` → (report) / `error`.

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
  Esc = Cancel** on every dialog; the window-manager close box = Esc. Mnemonics (`&Route`) are set on
  every button and checkbox; the Wording tables list `GetLabelText()` (mnemonic stripped).
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
  `ART_ERROR / ART_WARNING / ART_INFORMATION`, `FromDIP(16)`), never by colour alone. The single
  exception is the preview's new-copper highlight (§4).
- **Text.** Default GUI font; paths and details in a read-only `wx.TextCtrl` (selectable, copyable,
  `wx.FONTFAMILY_TELETYPE`), `wx.TE_MULTILINE | wx.TE_READONLY | wx.TE_BESTWRAP` for prose. Long
  messages wrap; nothing truncates silently. Every finding/error shows **code, cause, fix** (46 §1.1).
- **Keyboard.** Tab order = reading order (top-left → bottom-right, buttons last). Focus starts on
  the first editable control, or on the default button when there is none. Every control has a
  label (`wx.StaticText` immediately before it, so GTK's a11y tree names it).
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

Min 440×200. Focus: `Continue` (default). Keys: Enter/Esc = Continue, Alt+O = Open file (opens the
file in the system editor via `wx.LaunchDefaultApplication`, dialog stays open), Tab cycles path
field → Open file → Continue. Path field: read-only, monospace, selectable. No empty/busy state;
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
|   [ ] Keep partial routes                                                  |
|   Time limit  [  60 ]^ s   0 = no limit                                    |
|   Threads     [   0 ]^     0 = all cores                                   |
|   [ ] Save these options to topor.toml                                     |
|                                                                            |
| [ Help ]                                           [ Cancel ]  [ Route ]   |
+----------------------------------------------------------------------------+
```

Resizable, min 640×560, opens at 720×620; the findings list takes the extra height. Default `Route`.

**Behaviour.**

- Header line = `RunSummary` (connections in scope, nets, layers, engine version).
- Findings list: `wx.ListCtrl` report mode, sorted error → warn → info, then by code; icon + word in
  column 1 (`Error` / `Warn` / `Note`). Selecting a row fills the Cause/Fix pane (read-only
  `TextCtrl`, 3 lines min, wraps). The `Accept` checkbox is visible only for acceptable codes
  (`PAD_OUTSIDE_BOARD` → "skip these pads … `ignore_pads`", `DRILL_OUTSIDE_FILTER` → "add these
  drills to `allowed_drills_mm`"); each checked code goes to `Decision.accepted_codes`. Row 1 is
  selected on open. Message column = `Finding.message`; items (`Finding.items`) appear in the Cause
  text, up to 10, then "… and {n} more (all in the report)".
- **Error present → `Route` disabled** and the status text left of the buttons reads
  `Cannot route: {n} error(s) — see Fix above.` (so the disabled button has a visible reason).
  Warnings never block. `fail_on_preexisting` turns `PREEXISTING_DRC` into an error (B6).
- Scope radios: enabled only when the KiCad selection contains footprints; otherwise the two
  selection radios are disabled and `Whole board` is checked, with the grey hint
  `(nothing selected in KiCad)` after it. Initial value = `cfg.scope_mode` if a selection exists.
- Existing copper `wx.Choice`: `Keep as obstacles` (`fixed`), `Keep and smooth` (`smooth`),
  `Rip up and re-route` (`rip`). Respect locked = `respect_locked`; partial routes =
  `partial_routes`; Time limit = `timeout_s` (`SpinCtrl` 0…86 400, step 10); Threads = `threads`
  (0…256). `Save these options to topor.toml` → `Decision.save_to_toml` (B1 `update_config`,
  comments kept). Initial values = `cfg`; the header count is for `cfg.scope_mode` (see open Q4).
- Returns `Decision(proceed, accepted_codes, overrides)`; Cancel/Esc → `proceed=False`, nothing
  written (not even accepted codes).
- **Empty:** no findings → list has zero rows; the Cause pane shows
  `No findings. The board setup covers everything Topor needs.`; `Route` enabled. **Busy:** none —
  pre-flight (incl. the kicad-cli DRC on the copy) runs *before* this dialog under dialog 3 with phase
  `Checking the board (KiCad DRC)`. **Error:** see "Error present".
- Keys: Tab list → Cause pane → Accept → Scope radios (arrows) → Existing copper → Respect locked →
  Keep partial → Time limit → Threads → Save → Help → Cancel → Route. Enter = Route (also from the
  list), Esc = Cancel, Alt+R Route, Alt+A Accept, Alt+L Respect locked, Alt+K Keep partial, Alt+S
  Save, Alt+H Help (opens `docs/user/operate.md#options` via the default browser). Mnemonics only on
  buttons/checkboxes/radios (GTK `wx.StaticText` mnemonics do not move focus); spin controls are
  reached by Tab.

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
| Cancel keeps the best result so far and shows the preview.|
|                                                           |
|                                              [ Cancel ]   |
+-----------------------------------------------------------+
```

Min 460×240, not resizable. Opened on the first `progress()` call (modeless `Show()`), updated on
every call (B5 polls every 0.25 s), hidden when `p.phase == "done"` or when any other `UiPort`
method is called (belt and braces). The dialog never blocks: `WxUi.progress` updates the widgets,
calls `wx.YieldIfNeeded()` and returns `not self._cancelled`.

- Phase label text = `Progress.phase` mapped through a table (B9): `preflight_drc` →
  `Checking the board (KiCad DRC)`, `metrize` → `Preparing geometry`, `route` →
  `Routing — negotiation pass`, `drc` → `Verifying with KiCad DRC`, `smooth` → `Smoothing`,
  unknown → the raw phase string. Gauge: determinate `routed/total` when `total > 0` and phase is
  `route`; otherwise `Pulse()` (indeterminate) every call.
- Numbers right-aligned in fixed-width labels. Elapsed `m:ss`; limit shown as `/ {limit}` or
  `/ no limit` when `timeout_s == 0` (then Cancel is the only stop — the hint line says so).
- **Cancel** (default and escape; Enter also triggers it): label becomes
  `Cancelling — keeping the best result so far…`, button disabled, gauge pulses; the method returns
  `False` from then on; B5 calls `job.cancel()` and continues with DRC + preview.
- **Empty:** before the first engine iteration `routed/total` shows `– / {total}`. **Busy:** is the
  normal state. **Error:** none; an exception closes this dialog and opens dialog 6.
- Keys: Esc/Enter/Alt+C = Cancel; Tab stays on Cancel.

## 4. Preview — `UiPort.preview(png, report) -> bool`

```text
+--------------------------------------------------------------------------------------+
| Topor — Preview: eprom-to-kernal                                                 [x] |
+--------------------------------------------------------------------------------------+
| Zoom [ Fit v ]            Legend: F.Cu  B.Cu  ■ new copper (highlighted)              |
| +----------------------------------+ +----------------------------------+            |
| |         Current board            | |            Result                | | Summary   |
| |                                  | |                                  | |-----------|
| |   (kicad-cli SVG export,         | |   (same export of the routed     | | Routed    |
| |    KiCad theme colours,          | |    copy; new copper drawn with   | | 27/27 …   |
| |    F.Cu + B.Cu + Edge.Cuts)      | |    a highlight halo)             | | ≤ 20 lines|
| |                                  | |                                  | | read-only |
| +----------------------------------+ +----------------------------------+ | TextCtrl  |
|                                                                           +-----------+
| Report folder [ ~/.cache/kicad/10.0/topor/eprom-to-kernal/20261101-101530 ]           |
|                                                                                      |
| [ Open report folder ]                                        [ Reject ]  [ Accept ] |
+--------------------------------------------------------------------------------------+
```

Resizable, min 960×600, opens at 85 % of the display (`wx.Display.GetClientArea()`), remembers its
size per session only. Default `Accept`, Esc = `Reject`. Accept = one KiCad commit (one undo step
`Edit → Undo`), Reject = nothing written (B5).

- Image: the one PNG B5 renders (both panels + caption lines) in a `wx.ScrolledWindow` with a
  `wx.StaticBitmap`; `Zoom` choice `Fit` (default, keeps aspect) / `100 %` / `200 %`; mouse wheel
  scrolls, Ctrl+wheel steps the zoom. The titles `Current board` / `Result` and the legend are part
  of the PNG caption (B5) **and** repeated as dialog labels, so a screen reader gets them.
- **Highlight (the one hard-coded colour).** New copper is drawn in its layer colour with a halo
  `2 × width` underneath in `PREVIEW_HIGHLIGHT_RGB = (0, 139, 139)` (#008B8B). Contrast against
  white #FFFFFF 4.2:1 and against a dark window #2D2D2D 3.3:1 (WCAG 3:1 for graphics, both themes);
  distinguishable from F.Cu #C83434 and B.Cu #4D7FC4 by hue only (luminance contrast against the
  copper colours is ≈ 1:1), which is why the halo *width* (shape) and the existing copper drawn at
  60 % alpha in the Result panel carry the distinction — the cue is never colour only.
  B5 computes the contrast of the tint against the actual panel background at render time (relative
  luminance, WCAG formula) and falls back to `PREVIEW_HIGHLIGHT_FALLBACK_RGB = (208, 64, 208)`
  (#D040D0, 3.9:1 / 3.5:1) if the primary is < 3:1. Panel background = KiCad theme
  `board.background` when the export carries it (UNVERIFIED for `kicad-cli pcb export svg`), else
  `SYS_COLOUR_WINDOW`. Both constants live in `plugin/topor/preview.py` (B5); B9 imports them for
  the legend swatch.
- Summary pane: `report.summary_lines()` (≤ 20, §5) in a read-only `TextCtrl`, min width 260.
  Report folder: read-only monospace field with the full path; `Open report folder` →
  `wx.LaunchDefaultApplication(dir)`; disabled with grey hint `(not written yet)` only if B5 has not
  written the folder (see open Q2).
- **Empty:** image missing/unreadable → the image area shows `Preview could not be rendered
  (see log.txt in the report folder)`; Accept stays enabled (the DRC numbers in the summary are the
  decision basis). **Busy:** none. **Error:** none (exceptions → dialog 6).
- Keys: Tab Zoom → image (arrows/PgUp/PgDn scroll) → Summary → Report folder → Open report folder →
  Reject → Accept. Enter = Accept, Esc = Reject, Alt+O Open report folder, Alt+A Accept, Alt+R
  Reject.

## 5. Run report — `report.md` + the summary shown in the preview (B8)

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

`{outcome}` ∈ `applied as one undo step` / `rejected — board unchanged` / `cancelled — best result
shown` / `aborted at pre-flight` / `error {code}`.

**`report.md` structure** (headings in the Wording table): 1 Summary (block above) · 2 Connections
(table: net, from → to, status routed/partial/failed, reason + blocking point for non-routed, rips)
· 3 Vias (per net: result vs current; vias in pads listed) · 4 Length (per net, vs current, detour
ratio) · 5 DRC (`routing_new` by type, `preexisting` list, unconnected items; which DRC round) ·
6 Findings (pre-flight list with code, cause, fix, accepted?) · 7 Settings (non-defaults with
source, built-in defaults used, accepted fallbacks, `topor.toml` path) · 8 Files (the five files) ·
9 Versions (engine, plugin, kipy, KiCad, kicad-cli). Empty sections print `none`.

**Done notice** (after Accept/Reject; UiPort addition proposed in Q1), min 440×220:

```text
+------------------------------------------------------------------+
| Topor — Finished                                             [x] |
+------------------------------------------------------------------+
| (i)  Routed 27/27 connections — applied to the board as one      |
|      undo step (Edit → Undo).                                     |
|      Report folder                                               |
|      [ ~/.cache/kicad/10.0/topor/eprom-to-kernal/20261101-101530 ]|
| [ Open report folder ]                               [ Close ]   |
+------------------------------------------------------------------+
```

Second line variants: `Nothing was changed on the board.` (Reject), `Cancelled — the best result
was shown; nothing was changed.` (Cancel + Reject). Enter/Esc = Close, Alt+O = Open report folder.

## 6. Error — `UiPort.error(finding)`

```text
+------------------------------------------------------------------+
| Topor — Error                                                [x] |
+------------------------------------------------------------------+
| (x)  KICAD_CLI_MISSING                                           |
|      kicad-cli was not found, so the result cannot be verified.  |
|                                                                  |
|      Fix                                                         |
|      Install KiCad 10 with kicad-cli on PATH, or set              |
|      [paths] kicad_cli in topor.toml.                            |
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
folder existed. Enter/Esc = Close, Alt+C Copy details, Alt+O Open report folder. For `INTERNAL`
(panic/exception) the message is B5's generic text and the fix names `log.txt` + the bug-report
guide. Nothing is written to the board on this path.

## 7. Sizes, keyboard and theme — checklist for the GUI test

| Dialog | Min DIP | At 200 % (px) | Default | Esc | Resizable |
|---|---|---|---|---|---|
| first_run | 440×200 | 880×400 | Continue | Continue | no |
| preflight | 640×560 | 1280×1120 | Route (disabled on error) | Cancel | yes |
| progress | 460×240 | 920×480 | Cancel | Cancel | no |
| preview | 960×600 | 1920×1200 | Accept | Reject | yes |
| report (done) | 440×220 | 880×440 | Close | Close | no |
| error | 520×320 | 1040×640 | Close | Close | yes |

At 200 % the preflight and preview minimums exceed a 1366×768 laptop at 100 % — fine, since 200 %
implies a ≥ 2560-wide display; on smaller displays `wx.Dialog` clamps to the client area and the
list/image scroll. Dark theme: every surface is a system colour; icons from the GTK icon theme;
highlight tint checked ≥ 3:1 at runtime (§4). GUI checklist acceptance (B11): tab order as listed,
Esc = Cancel, Enter = default, readable at 100 % / 200 %, dark theme.

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
  mappings, `aria-live` (progress updates are plain label changes — GTK AT-SPI announces focused
  widgets only; noted as a limitation).

## Wording

Text is the exact user-visible string (`GetLabelText()`, mnemonic stripped); `{…}` = runtime value.

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
| preflight | partial_routes | checkbox | Keep partial routes |
| preflight | timeout_label | label | Time limit |
| preflight | timeout_unit | label | s |
| preflight | timeout_hint | label | 0 = no limit |
| preflight | threads_label | label | Threads |
| preflight | threads_hint | label | 0 = all cores |
| preflight | save_toml | checkbox | Save these options to topor.toml |
| preflight | blocked_status | label | Cannot route: {n} error(s) — see Fix above. |
| preflight | help | button | Help |
| preflight | cancel | button | Cancel |
| preflight | route | button | Route |

### progress

| dialog | id | kind | text |
|---|---|---|---|
| progress | title | title | Topor — Routing {board} |
| progress | phase_preflight_drc | text | Checking the board (KiCad DRC) |
| progress | phase_metrize | text | Preparing geometry |
| progress | phase_route | text | Routing — negotiation pass |
| progress | phase_drc | text | Verifying with KiCad DRC |
| progress | phase_smooth | text | Smoothing |
| progress | phase_cancelling | text | Cancelling — keeping the best result so far… |
| progress | gauge_value | text | {routed} / {total} |
| progress | iteration_label | label | Iteration |
| progress | iteration_value | text | {iteration} of up to {cap} |
| progress | routed_label | label | Routed |
| progress | routed_value | text | {routed} / {total} |
| progress | routed_pending | text | – / {total} |
| progress | vias_label | label | Vias |
| progress | elapsed_label | label | Elapsed |
| progress | elapsed_value | text | {elapsed} / {limit} |
| progress | elapsed_nolimit | text | {elapsed} / no limit |
| progress | cancel_hint | label | Cancel keeps the best result so far and shows the preview. |
| progress | cancel | button | Cancel |

### preview

| dialog | id | kind | text |
|---|---|---|---|
| preview | title | title | Topor — Preview: {board} |
| preview | zoom_label | label | Zoom |
| preview | zoom_fit | choice | Fit |
| preview | zoom_100 | choice | 100 % |
| preview | zoom_200 | choice | 200 % |
| preview | legend_label | label | Legend: F.Cu  B.Cu  new copper (highlighted) |
| preview | before_heading | heading | Current board |
| preview | after_heading | heading | Result |
| preview | summary_heading | heading | Summary |
| preview | no_image | text | Preview could not be rendered (see log.txt in the report folder) |
| preview | report_dir_label | label | Report folder |
| preview | report_dir | text | {report_dir} |
| preview | report_dir_pending | label | (not written yet) |
| preview | open_report_folder | button | Open report folder |
| preview | reject | button | Reject |
| preview | accept | button | Accept |

### report

| dialog | id | kind | text |
|---|---|---|---|
| report | title | title | Topor — Finished |
| report | done_accepted | text | Routed {routed}/{total} connections — applied to the board as one undo step (Edit → Undo). |
| report | done_rejected | text | Nothing was changed on the board. |
| report | done_cancelled | text | Cancelled — the best result was shown; nothing was changed. |
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
| report | outcome_cancelled | text | cancelled — best result shown |
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
