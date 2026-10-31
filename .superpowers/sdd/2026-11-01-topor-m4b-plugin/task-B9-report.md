# Task B9 report: wx dialogs (`plugin/topor/ui_wx.py`)

Status: DONE_WITH_CONCERNS (no GUI check possible under Ruling C18; see "Verify by eye").
Skills invoked first: ecc:make-interfaces-feel-better, ecc:accessibility (applied as the note adapts them).
Branch merged `main` first (my worktree predated B2's ports.py); note used = the REVIEWED version (it changed under me: min sizes
640x480/800x520, single Accept checkbox, phase table, mnemonics, done variants). Wording table is parsed from that version.

## What each dialog does
1. FirstRunDialog: info icon, intro, monospace read-only path, explanation, `Open file` (xdg/LaunchDefaultApplication, dialog stays), `Continue` (default+Esc).
2. PreflightDialog: header (live connection count per Scope radio), findings list (icon+word, sorted error>warning>info then code), Cause/Fix pane (items <=10 then "... and n more"),
   ONE Accept checkbox relabelled per code (PAD_OUTSIDE_BOARD / DRILL_OUTSIDE_FILTER), state kept per code; scope radios (disabled w/o selection + grey hint), existing-copper choice,
   respect locked, partial routes, time limit (SpinCtrlDouble step 10, 0..86400), threads, save. Bottom line `[Help] (x) Cannot route: n error(s) - see Fix above. [Cancel][Route]`, Route disabled on error.
   Enter in list = Route (EVT_LIST_ITEM_ACTIVATED). `decision(proceed)` builds the Decision; Cancel -> `Decision(False)` (accepted codes dropped).
3. ProgressDialog (modeless via WxUi.progress): phase label from table, gauge (determinate only in `route` with total>0, else Pulse), fixed-width right-aligned number labels, elapsed m:ss (h:mm:ss from 1 h) / limit or "no limit". Cancel/Esc/Enter/close box -> label "Cancelling ...", button disabled, `progress()` returns False from then on.
   Closed only by the next other UiPort call (not on phase "done"), per the reviewed note.
4. PreviewDialog: zoom (Fit / 100 % = 1 PNG px per DIP / 200 %), Ctrl+wheel steps zoom, 1 px GRAYTEXT outline, legend swatch (only fixed colour, imports PREVIEW_HIGHLIGHT_RGB from `.preview` if B5 provides it, else (0,139,139)), headings, summary pane (`report.summary_lines()`), report-dir field, Open report folder (always enabled), Reject/Accept. Missing image -> `no_image` text, Accept stays enabled.
5. DoneDialog: text by outcome (`accepted` / `accepted` + `report.stop_reason == "cancel"` / `rejected` / `cancelled`); routed/total parsed from `summary_lines()[ "Routed a/b ..." ]`.
6. ErrorDialog: icon + bold code, message, Fix, monospace details (items <=20), Copy details (label "Copied" for 2 s), Open report folder disabled with hint when `ctx.report_dir` missing.
WxUi: `set_context(board, report_dir, log_path, versions)` for B5; `_present()` / `_pump()` are overridable seams (tests use them so nothing is ever shown).

## Contract additions (ports.py) - B5/B8 must provide
- `RunSummary.board: str`, `RunSummary.has_selection: bool` (footprints selected), `RunSummary.connections_by_scope: dict` ("whole"/"between"/"touching" -> count; missing key falls back to `connections`). Appended at the end, existing positional uses unchanged.
- `Report` Protocol: `dir: Path` (report folder). Optional, read with getattr: `stop_reason` (str; "cancel" selects the "Stopped early by Cancel" text in `done`). `summary_lines()` first "Routed a/b" line is parsed for the done text.
- Decision.overrides keys emitted (only values differing from cfg): scope_mode ("between"|"touching" only, valid toml values), existing_copper, respect_locked, partial_routes, timeout_s (float), threads, plus `scope_whole: bool` (only when a selection exists; True = user chose Whole board despite a selection). run() must honour `scope_whole` and must not save it to topor.toml.
- B5 must call `ui.set_context(board=..., report_dir=..., log_path=..., versions={engine,plugin,kipy,kicad})` before dialogs 4-6; `progress` phases used: setup, route, done, preflight_drc, drc, preview (else raw string).
- Progress dialog takes timeout_s from the preflight Decision (else default config), iteration cap from `cfg [iterations] max`.

## Deviations / things not exactly as the note
- Controls without own text get `SetName(label)` (a11y, note §0) after construction; the wording-id lookup is via `dlg.widget(id)` / `dlg.child_ids()` (registry built in `finish()`), not `FindWindowByName`, for exactly those controls (findings, cause, existing, timeout, threads, zoom, image, summary, report_dir, details). All other widgets keep `SetName(id)`.
- Help is a left "extra" button (ID_HELP outside the StdDialogButtonSizer) so the status text sits between Help and Cancel as in the error-state wireframe; Cancel/Route still ordered by StdDialogButtonSizer.
- Help opens docs/user/operate.md#options only if that file exists next to the repo; the installed plugin has no docs dir, so Help is currently a no-op there (operate.md does not exist yet).
- Wrap() on StaticText inserts newlines into GetLabel(); the wording test collapses whitespace.
- Done text falls back to "Applied to the board as one undo step (Edit -> Undo)." when no "Routed a/b" line is found (not in the table).
- Severity/status icons use FromDIP(16); first-run/done info icon uses 32 DIP (message-box icon).
- `test_no_pcbnew.py` probe list now also imports `topor.ui_wx` when wx is installed.

## Tests (`.venv/bin/pytest -q tests/plugin`)
tests/plugin/test_ui_wx.py: 9 tests green (construct headless incl. tab order/default/Esc/min size/mnemonics/Decision round trip; wording table vs. constructed widgets and vs. `ui_wx.WORDING`; progress returns False after simulated Cancel; Route disabled on error + reason; missing image; Accept relabel/state; live scope count + done variants; time format).
Full tests/plugin: 63 passed, 1 skipped, 1 error = `test_pcbfile_roundtrip_on_snapshot` (scratch/ board snapshot missing in this worktree, unrelated to B9).
Dialogs are constructed on DISPLAY=:0 but never Show()n; all Destroy()ed.

## Verify by eye (GUI test, B11)
- Real sizes at 100/200 % (min sizes 440x200, 640x480, 460x240, 800x520, 440x220, 520x320 DIP), preview opens 85 % of display, preflight 720x620 clamped to 90 %.
- Dark theme: list/cause/summary colours, grey hints (GRAYTEXT), halo/swatch visibility; GTK icons at 200 % (drop icon if poor, word stays).
- Preview bitmap crispness at 200 % (bitmap is built at physical pixels; Fit uses the physical client size; wx.Bitmap scale factor not set).
- Real Esc/Enter on the modeless progress dialog (handled in EVT_CHAR_HOOK) and Enter in the findings list; Tab order on GTK matches creation order.
- Button order (Cancel left of Route), mnemonics not colliding (R,A,W,B,T,L,K,S,H,C / O,C / D,O,C), Copy details clipboard + "Copied" flip.
- Does the preview show "Fit" correctly right after open (first render happens before final layout; EVT_SIZE re-renders).
