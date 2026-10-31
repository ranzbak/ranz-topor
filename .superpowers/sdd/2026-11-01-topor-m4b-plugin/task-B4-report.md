# Task B4 report — UI design: wireframes for every dialog (F design, draft for review)

**Status:** DONE (draft; awaits the independent F review → fixes → final commit `docs(ui): M4 dialog wireframes (reviewed)`).
**Branch / commit:** `worktree-agent-a0997c83131a7685a` @ `76f9b77` (`docs(ui): M4 dialog wireframes (draft for review)`).
**Files:** `docs/progress/notes/m4-ui-wireframes.md` (new, 516 lines; ~4 pages of prose + wireframes + wording tables). No code.

## What the note contains
- §0 common rules: native wx widgets only, `wx.StdDialogButtonSizer` (platform button order, Cancel left of default on GTK),
  `SetAffirmativeId`/`SetEscapeId` → Enter = default, Esc = Cancel everywhere; sizes in DIP via `FromDIP`; all colours from
  `wx.SystemSettings` except the preview tint; icon + word for severity (never colour only); labels before every control; states.
- §1–§6 one ASCII wireframe per dialog with behaviour, states (empty/busy/error), focus order, keyboard map, min size,
  and the `UiPort` method it implements. §7 size table (100 % / 200 %), §8 "Skill guidance applied" (taken / rejected per skill).
- `## Wording`: six tables `| dialog | id | kind | text |`, 128 rows, validated with a throwaway parser (dialog ∈ the six names,
  kind ∈ the seven kinds, `{placeholders}` for values). Text = `GetLabelText()` (mnemonic stripped) — B9's test should compare that.

## Design decisions (and why)
1. **Pre-flight DRC runs under the progress dialog** (phase `preflight_drc`, indeterminate gauge) *before* dialog 2 — the kicad-cli
   DRC on the copy takes 2–20 s; a frozen UI there would be the first thing a user sees. Needs B5 to call `ui.progress()` early.
2. **Progress dialog lifecycle:** modeless, opened on the first `progress()` call, hidden on `phase == "done"` or on any other `UiPort`
   call. `WxUi.progress` updates widgets + `wx.YieldIfNeeded()` and returns `not cancelled`, so B5's 0.25 s polling loop stays the
   driver (no threads in the UI). Cancel disables itself and shows "Cancelling — keeping the best result so far…".
3. **Route disabled with a visible reason** (`Cannot route: {n} error(s) — see Fix above.`); warnings never block; acceptable
   findings (`PAD_OUTSIDE_BOARD`, `DRILL_OUTSIDE_FILTER`) get a per-code Accept checkbox → `Decision.accepted_codes`.
4. **Option mapping:** Scope radios ↔ `scope_mode` (+ "Whole board" when nothing is selected, radios disabled with hint);
   Existing copper choice `Keep as obstacles / Keep and smooth / Rip up and re-route` ↔ `fixed|smooth|rip`; checkboxes ↔
   `respect_locked`, `partial_routes`; spins ↔ `timeout_s` (0 = no limit), `threads` (0 = all cores); `Save these options to topor.toml`
   ↔ `save_to_toml`. Pre-filled from `cfg` (no redundant entry).
5. **Preview highlight = the one hard-coded colour:** `PREVIEW_HIGHLIGHT_RGB = (0,139,139)` (#008B8B): 4.15:1 on white, 3.32:1 on
   #2D2D2D, 4.61:1 on KiCad's #001023; fallback `(208,64,208)` (3.94 / 3.49) chosen at render time if the actual panel background
   gives < 3:1. Against the copper colours themselves the luminance contrast is ≈ 1:1, so the distinction is carried by the halo
   (2 × width underneath) and by drawing existing copper at 60 % alpha in the Result panel — not colour only. Constants proposed to
   live in `plugin/topor/preview.py` (B5) and be imported by B9 for the legend swatch.
6. **Report folder** shown as a selectable monospace field + `Open report folder` (`wx.LaunchDefaultApplication`) in preview, done
   notice and error (M4-R4, C1); default path and the relative/absolute rules restated in §5.
7. **Summary block** (13 template lines, ≤ 20) is the same text in the preview pane, `report.md` §1 and the done notice; `report.md`
   has nine fixed sections (headings in the Wording table so B8 and B9 share strings).
8. **Mnemonics only on buttons/checkboxes/radios** (GTK `wx.StaticText` mnemonics do not move focus); spin controls reached by Tab.
9. **Dialog titles carry the board name** (`Topor — Route {board}`) because the plugin is a separate process with top-level windows.

## Open questions (for the reviewer / coordinator)
- **Q1 (B2/B5):** the "done" notice after Accept/Reject needs a `UiPort` method that B2's protocol lacks — proposed
  `done(outcome: str, report: Report, report_dir: Path) -> None`. If rejected, the preview's summary + `Open report folder` already
  satisfy M4-R4 and §5 stays the `report.md` layout only; the `report` wording rows for the notice become unused.
- **Q2 (B5):** `Open report folder` in the preview only works if B5 writes `report.md`/`preview.png` to the folder *before* calling
  `ui.preview()` and updates `Outcome:` afterwards; the note designs for that (otherwise the button shows `(not written yet)`).
- **Q3 (B2/B5):** progress phases — the note maps `preflight_drc / metrize / route / drc / smooth / done` to user text; P1's
  `Progress.phase` strings should use these names (unknown phases are shown raw).
- **Q4 (B6/B7):** the header count is for `cfg.scope_mode` only; live recount when the user switches Between/Touching needs
  `RunSummary` to carry counts per mode (`connections_by_scope`). Not blocking.
- **Q5 (B5, UNVERIFIED):** whether `kicad-cli pcb export svg` writes the theme background; the note handles both cases.
- **Q6 (B9):** `wx.ListCtrl` with icons per row needs an `ImageList`; if GTK rendering at 200 % is poor, fall back to word-only
  severity (the word is mandatory anyway).

## Verification
- Skills invoked in order via the Skill tool: `ecc:frontend-design-direction`, `ecc:make-interfaces-feel-better`, `ecc:accessibility`
  (§8 lists what was taken / rejected as web-only).
- Contrast ratios computed with the WCAG relative-luminance formula (python, scratchpad).
- Wording tables parsed with a throwaway script: 128 rows, 0 malformed, counts first_run 6 / preflight 37 / progress 19 /
  preview 16 / report 37 / error 13.
- No tests to run (docs-only task); markdownlint warnings (fence language, blank lines) fixed.

## Concerns
- GTK AT-SPI will not announce progress label changes (no live region in wx) — noted as a limitation in §8.
- At 200 % the preflight/preview minimum sizes exceed a 1366×768 display; acceptable since 200 % implies a large display, and the
  list/image areas scroll.
- The attribution line used is the executing session's (`Claude Fable 5.1`, per global constraints), not the contract's `Opus 5.5`.
