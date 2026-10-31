### Task B4 (F design + F review): UI design — wireframes for every dialog — `docs/progress/notes/m4-ui-wireframes.md`

**Files:** Create the note. Before writing, invoke via the Skill tool: `ecc:frontend-design-direction`, then `ecc:make-interfaces-feel-better`, then `ecc:accessibility`; adapt to wxPython inside KiCad (native widgets, KiCad dialog look, light/dark theme from `wx.SystemSettings`, HiDPI via `FromDIP`).

**Interfaces:**
- Produces ASCII wireframes + behaviour notes for: (1) **first-run `topor.toml` notice** (file created at `<project>/topor.toml`, "Open file" / "Continue"); (2) **options + pre-flight dialog** (findings grouped by severity with cause and fix, scope mode, existing copper, respect locked, partial routes, timeout, threads, "Save to topor.toml", default button **Route**, Esc = Cancel; errors disable Route and say why); (3) **progress** (phase, iteration, routed/total, vias, elapsed, indeterminate gauge during metrization, Cancel = keep best so far); (4) **preview Accept/Reject** (current vs result side by side, both layers colour-coded, new copper highlighted, ≤ 20 summary lines, "Open report folder", default **Accept**, Esc = Reject); (5) **run report** (`report.md` structure + the summary shown in the preview); (6) **error dialog** (code, cause, fix, "Copy details"). Each: focus order, keyboard map, min sizes at 100 %/200 %, dark-theme colours, wording table (every label/button text), states (empty, busy, error), `UiPort` method it implements.

- [ ] **Step 1:** write the note (≤ 4 pages); **Step 2:** independent F review (checklist: the three skills' criteria, KiCad look, keyboard, scaling, wording) → fixes; **Step 3:** commit `docs(ui): M4 dialog wireframes (reviewed)`.

