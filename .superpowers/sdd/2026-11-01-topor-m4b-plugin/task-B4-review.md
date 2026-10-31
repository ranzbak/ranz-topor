# Task B4 — independent F review of `docs/progress/notes/m4-ui-wireframes.md`

Reviewer: F (Claude Fable 5.1), worktree `worktree-agent-a6762caf61d78a14a` (based on main @ e2e1348, which contains the draft).
The shared-checkout path `.superpowers/sdd/…/task-B4-review.md` was refused by the sandbox; this file lives at the same
relative path inside the worktree and is committed with the note.
Skills invoked via the Skill tool before reviewing: `ecc:frontend-design-direction`, `ecc:make-interfaces-feel-better`,
`ecc:accessibility`; criteria adapted to native wxPython 4.2.1 / GTK inside KiCad 10 (web-only items — CSS radii, shadows,
transitions, ARIA — not applied). Cross-checked against merged code: `plugin/topor/findings.py` (severity values),
`plugin/topor/schema.py` (keys, hints), `toporlib/kicad_cli.py` (kicad-cli resolution), `engine/src/control.rs` +
`engine/src/route.rs` (`Progress.phase` is exactly `setup` / `route` / `done`), and the B2/B5/B6/B8/B9 briefs.

Verdict on the draft: good direction (dense, quiet, native, one default per dialog, cause + fix everywhere, no colour-only
cues) — but two blocking inconsistencies with the coordinator rulings and the real engine, and a cluster of important
wording/keyboard/size issues. All BLOCKING and IMPORTANT findings are fixed in the note in this worktree; the MINOR ones too
except where stated.

## BLOCKING

| # | Finding | Fix applied |
|---|---|---|
| B1 | **Phase table does not match the ruled phase set (Q3)** and the lifecycle rule is wrong. The draft maps `metrize` and `smooth` (phases the engine never emits) and lacks `setup`, `done` and the plugin's `preview`. Worse, "hidden when `p.phase == "done"`" closes the progress window at the end of the *engine* run — before the plugin's `drc` loop (which may re-enter `route`) and the `preview` render, so the user stares at nothing for seconds. | §3 rewritten: the six phases `setup`, `route`, `done` (engine) + `preflight_drc`, `drc`, `preview` (plugin) each mapped to user text (wording rows `phase_*`), unknown → raw string; the dialog is closed only by the next `UiPort` call (`preflight`, `preview`, `error`, `done`); gauge determinate only in `route` with `total > 0`. Iteration cap sourced from `[iterations] max`. |
| B2 | **Minimum sizes do not fit at 200 %.** Preflight 640×560 DIP = 1280×1120 px and preview 960×600 DIP = 1920×1200 px exceed a 1920×1080 panel at 200 % (GTK scales in integers, so 13″ 1080p laptops do run at 200 %). The claim "200 % implies a ≥ 2560-wide display" is false, and the preflight dialog has no scrolling region besides the list, so the bottom buttons would be off-screen — the GUI checklist item "readable at 200 %" fails by design. | Preflight min 640×480 (list ≥ 4 rows, Cause pane 3 lines), preview min 800×520 (image area scrolls anyway); both open at `min(requested, 90 % of client area)`; §7 table and paragraph corrected (1280×960 / 1600×1040 px fit 1920×1080 at 200 %). |

## IMPORTANT

| # | Finding | Fix applied |
|---|---|---|
| I1 | Q1 ruled: `UiPort.done(outcome, report, report_dir)` exists. The draft calls it "proposed" and §5 does not name the method it implements (checklist: every dialog names its `UiPort` method). | §5 heading now `UiPort.done(outcome: str, report, report_dir: Path) -> None`; scope flow line includes `done`; "proposed in Q1" removed. |
| I2 | Q2 ruled: `report.md` + `preview.png` are written before `ui.preview()`, so the preview's `Open report folder` is always live. The draft keeps a `(not written yet)` disabled state and a `report_dir_pending` wording row that B9 would implement. | State and row removed; note states B5's guarantee. The `error` dialog keeps its `(no report folder yet)` state (a run can fail before the folder exists). |
| I3 | Q4 ruled: `RunSummary` carries per-mode counts. The draft leaves "see open Q4" dangling. | Header count now recounts live when the Scope radio changes, from `RunSummary.connections_by_scope`. |
| I4 | **Mnemonic collision** in the error dialog: `Copy details` (Alt+C) vs `Close` (GTK stock `&Close` = Alt+C). Also §0 promises mnemonics on radios but §2 assigns none, and `Cancel` (Alt+C) was not listed. | `Copy &details` (Alt+D) in error; full per-dialog mnemonic sets listed (preflight: R A W B T L K S H C; preview: A R O; error: D O C; first-run: O C; progress: C; done: O C) — no duplicates within any dialog. |
| I5 | Threads hint `0 = all cores` contradicts the config reference (`schema.py`: "0 = all cores but one"); the GUI and `topor.toml` would disagree. | Hint = `0 = all cores but one`. |
| I6 | The error example's fix names `[paths] kicad_cli in topor.toml`, a key that does not exist (`schema.py` has no `[paths]`); kicad-cli is resolved from `$KICAD_CLI`, else `/usr/bin/kicad-cli` (`toporlib/kicad_cli.py`). A wireframe that invents keys will be copied into `messages.py`. | Example fix text now names the real places; rule added in §0: fix text names only keys that exist in `schema.py` or a KiCad menu path. |
| I7 | Severity words: the draft maps from `warn`; merged `Finding.severity` is `info` / `warning` / `error`. | Mapping row `error → Error`, `warning → Warn`, `info → Note` added (§2). |
| I8 | Accept checkbox ambiguous when both acceptable codes (`PAD_OUTSIDE_BOARD`, `DRILL_OUTSIDE_FILTER`) are present: two checkboxes (two Alt+A) or one? | One checkbox, relabelled per selected row, checked state kept per code; `Decision.accepted_codes` = the checked codes. |
| I9 | Outcome strings overlap: `cancelled — best result shown` vs accepted/rejected after Cancel, and they do not map to B5's `RunOutcome.status` set (`aborted_preflight`, `cancelled`, `rejected`, `accepted`, `error`). The done-notice `done_cancelled` text ("best result was shown; nothing was changed") is wrong after Cancel → Accept. | `{outcome}` now maps 1:1 to B5's five statuses; Cancel + Accept = `accepted` with `Stopped: cancel`; done notice has four variants (`done_accepted`, `done_accepted_cancelled`, `done_rejected`, `done_cancelled`). |
| I10 | The disabled-Route reason (`blocked_status`) is in the wording table but not in the wireframe and had no icon, while §0 says severity is never colour-only and nothing is disabled without a visible reason. | Error-state variant drawn in the §2 wireframe with the error icon, left of the buttons. |

## MINOR

| # | Finding | Fix applied |
|---|---|---|
| M1 | Q5: kicad-cli SVG background UNVERIFIED is noted in §4 but not in the §7 GUI checklist. | Added to §7 as a checklist item (light + dark). |
| M2 | Q6: word-only severity fallback accepted but absent from the note. | §0: `ImageList` icons; if GTK renders them poorly at 200 %, word only (the word is mandatory). |
| M3 | In a GTK `wx.ListCtrl`, Enter fires `EVT_LIST_ITEM_ACTIVATED`, not the default button; "Enter = Route also from the list" needs an explicit binding. | Stated. |
| M4 | The image `wx.ScrolledWindow` only gets arrow/PgUp/PgDn keys with `wx.WANTS_CHARS` and `SetCanFocus(True)`. | Stated. |
| M5 | GTK a11y: a `wx.StaticText` before a control does not create an ATK label-for relation in wx; the control still has no accessible name. | Added `SetName(label)` + `SetToolTip(label)` on controls whose text is not their own (Choice, SpinCtrl, TextCtrl) and recorded the limitation in §8 next to the live-region one. Not fully fixable in wx — kept as limitation. |
| M6 | `legend_label` used double spaces (fragile for B9's equality test); `\n` inside the `details` row needs a stated convention. | `·` separators; "`\n` = newline" stated above the tables. |
| M7 | Elapsed/limit format: limit format unspecified; above one hour unspecified. | Both `m:ss`, `h:mm:ss` from 1 h. |
| M8 | `wx.StdDialogButtonSizer.AddButton` accepts stock ids only; the "left-aligned extra actions" need a wrapping sizer. | One sentence in §0. |
| M9 | Contrast numbers: #008B8B on Adwaita-dark `#353535` is 2.96:1 (< 3:1) — this is the concrete case for the runtime fallback, worth recording; other numbers verified (4.15 / 3.32 / 4.61). | Numbers corrected and the #353535 case added. |
| M10 | `topor.toml` keys not in the dialog (`unrouted_only`, `[iterations]`, `[vias]`, …) — the note should say they are file-only so nobody adds them to the dialog later. | One clause in §2. |
| M11 | Zoom semantics at 200 % unspecified. | `100 %` = one PNG pixel per DIP (so ×2 physical at 200 %); `Fit` fits the DIP client area. |
| M12 | The cancel hint promises "shows the preview"; after Cancel with nothing routed B5 may end in status `cancelled` without a preview. | Covered by I9 (`cancelled` outcome defined as "nothing applied"; hint softened to "keeps the best result so far"). |

## Checklist result (after fixes)

- Skill criteria: direction (utilitarian, dense, scannable; first view = workflow) ✓; states (empty/busy/error/disabled-with-reason) per dialog ✓; tabular numbers via fixed-width labels ✓; image outline ✓; hit areas native + `FromDIP(88)` buttons ✓; no colour-only meaning ✓; contrast ≥ 3:1 with runtime fallback ✓; keyboard-only operation ✓; focus containment (modal) ✓; redundant entry avoided ✓.
- KiCad look: native widgets, `StdDialogButtonSizer` (GTK order), titles `Topor — …` ✓. Esc = Cancel/Reject/Close, Enter = default on every dialog ✓. Mnemonics without collisions ✓ (I4).
- Scaling 100 % / 200 %: all mins fit 1920×1080 at 200 % ✓ (B2). Light + dark: system colours, one checked tint ✓.
- Wording: plain, actionable, consistent verbs (Route / Accept / Reject / Cancel / Close / Continue / Open …); every finding shows cause + fix ✓; hints match `schema.py` ✓ (I5).
- `UiPort` method named per dialog incl. `done` ✓. M4-R4: `Open report folder` + visible path in preview, done notice and error ✓.
- Wording tables: six tables `| dialog | id | kind | text |`, dialog ∈ the six names, kind ∈ the seven kinds — re-validated with a throwaway parser after the edits (counts in the final reply).

## Not fixed (and why)

- GTK AT-SPI does not announce label changes in the progress dialog and wx has no live-region API; recorded as a limitation (§8). A `wx.Gauge` with `SetRange/SetValue` is at least read as a progress bar by Orca.
- The exact `kicad-cli pcb export svg` background behaviour (Q5) stays UNVERIFIED until the GUI test.
