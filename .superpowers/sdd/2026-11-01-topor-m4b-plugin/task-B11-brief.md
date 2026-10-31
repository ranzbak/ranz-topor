### Task B11 (S; H mechanical): user guide, GUI test checklist, CONTRIBUTING, debug guide — `docs/user/`, `docs/dev/`, `CONTRIBUTING.md`, `README.md`

**Files:** `docs/user/install.md` (build script, wheel, enable the KiCad API server, plugin environment, "Recreate Plugin Environment", Python ≥ 3.11), `operate.md` (first run, options dialog, scope by selection, findings and what to do, progress/cancel, preview, Accept/Reject, undo, report folder, limitations: Linux, two layers, groups), `troubleshoot.md` (`### <CODE>` per `MESSAGES` code + "exited with code N", wheel/venv issues), `gui-test-checklist.md` (eprom and wiccon copies: run → first-run notice → pre-flight contents → progress + Cancel once → preview → Reject (board unchanged, Undo greyed) → run again → Accept → one Ctrl+Z reverts all → KiCad DRC 0 new → report folder → selection scope `between` on two footprints; **UI acceptance:** tab order per wireframe, Esc = Cancel, Enter = default button, readable at 100 % and 200 % scaling, dark theme; **IPC parity:** run `test_ipc_parity` with each snapshot board open; **UNVERIFIED items** from B2/B3/B5/B6 (groups, via-protection keys, `run_action` names); results table), `docs/dev/debug.md` (`route_probe`, `gate_diag --detour`, `--profile`, progress PNGs, `harness.measure`, `app --headless`, reproducing a plugin run from `result.json` + the `save_as` copy, filing a bug), `CONTRIBUTING.md` (layout incl. `toporlib/`, build, tests, harness on copies, read-only projects, commit style, schema → toml → docs); `README.md` + `docs/README.md` updates; tests `test_troubleshoot_has_section_per_code`, `tests/test_docs_links.py`.

- [ ] **Step 1:** tests → FAIL; write docs; PASS; `python -m topor.docs_gen --check` clean. Commit `docs(user): install/operate/troubleshoot/GUI checklist; CONTRIBUTING and debug guide`.


#### Coordinator additions (rotation 2) — binding
- Audience: a KiCad user with no AI help (memory "usable without AI"): every topor.toml key and every KiCad setting the plugin reads is reachable
  from operate.md with the exact KiCad menu path (KiCad 10: File → Board Setup → …, Preferences → Plugins, etc.). Facts come from the code, not the
  plan: read plugin/topor/{schema,messages,preflight,run,scope,priority,report,ui_wx,app}.py, tools/build.sh, plugin/plugin.json, and the rotation-1
  reports in /home/paul/work/kicad/plugins/topor/.superpowers/sdd/2026-11-01-topor-m4b-plugin/ (task-B4-review.md, task-B5/B6/B7/B9/B10/B2/B3-report.md:
  UNVERIFIED lists, install steps in B10). docs/progress/notes/m4-ui-wireframes.md = dialog layout, tab order, wording.
- `docs/user/configuration.md` is GENERATED (topor.docs_gen) — never hand-edit; link to it. Align the `[nets] priority` doc string in
  plugin/topor/schema.py with Ruling C22 (order: name marker `@N` > `topor_pN` netclass > `[nets] priority`; i.e. [nets] priority does NOT override
  the marker / netclass — check plugin/topor/priority.py for the real order and describe exactly that), regenerate configuration.md, `--check` clean;
  add/adjust a test pinning the order wording vs priority.py behaviour.
- **Help button bug:** plugin/topor/ui_wx.py `HELP_DOC = Path(__file__).resolve().parents[2] / "docs/user/operate.md"` is right in the repo
  (plugin/topor → repo root) but WRONG in the installed layout (tools/build.sh ships org.topor.router/{topor/,docs/user/} → parents[1]). Fix: look up
  parents[1] then parents[2]; test both layouts (tmp dirs). Every anchor the UI opens (`#options` and any other `#…` in ui_wx.py) must exist as a
  heading slug in operate.md — test it (tests/test_docs_links.py may cover anchors).
- operate.md **Limitations** section lists, at least: Linux only; two copper layers routed (≥4-layer boards: planes by role, inner signal routing
  not in 0.4.0 — check what layer_plan/engine actually do); groups (GROUPS_UNAVAILABLE); engine knobs since 0.5.0 (net priorities / do_not_touch /
  ignore pads are computed and reported but not yet applied by the engine — check B7 report); and every IPC model approximation with its
  conservative direction (M4-R8): copper graphics polygonised outward (obstacle ⊇ KiCad shape, ≤ max_error larger); copper text = its bounding box
  (larger than the glyphs); pour reach by probe points (may report "not reached" where KiCad connects; KiCad DRC is the judge); via `free` flag read
  from the saved copy, unknown → kept as fixed obstacle; stitching vias that no track touches are kept (`[run] keep_unconnected_vias`, default true);
  net codes by sorted name (IPC has none); keepout layers = copper only; outline circles/beziers/polygons flattened; pad rotation from padstack angle
  (UNVERIFIED); VIA_NOT_SET_UP falls back to vias.fallback_diameter_mm / fallback_drill_mm (0.8 / 0.4 mm). (A parallel worker is implementing the
  M4-R8 / keep_unconnected_vias / fallback changes now — describe them as above; the coordinator reconciles wording after merge. Do NOT add the
  keep_unconnected_vias key to schema.py yourself.)
- `docs/user/gui-test-checklist.md`: the plan's sequence + UI acceptance + IPC parity + every UNVERIFIED item from the rotation-1 reports (B2 9 IPC
  calls, B3 live kipy fields, B5 preview/kicad-cli SVG background (C15 Q5), B6 via-protection / groups detection, B9 visual checks, B10 install/
  connect/toolbar/handshake list), each as a checkbox row with expected result, plus a results table (board × step × pass/fail/notes) for eprom and
  wiccon copies in scratch/gui/<board>/ (never the originals in ~/work/kicad/projects). Install steps per B10 report (`bash tools/build.sh --dev`,
  Preferences → Plugins → Enable KiCad API, Recreate Plugin Environment, restart PCB Editor). How to run `test_ipc_parity` live
  (KICAD_API_SOCKET / token env, which board open).
- README.md: status table for 0.4.0 (M4b: plugin) — what works now from inside KiCad, what comes in 0.5.0 (M4a engine: dcsgone 231/231 etc.);
  requirements (KiCad 10, Python ≥ 3.11 for the plugin venv); links to docs/user/*.md. docs/README.md: add a "User guide" + "Developer" section.
  docs/design/45-mvp-plan.md: M4 row records the split M4b (0.4.0 plugin) / M4a (0.5.0 engine) (B12 file item; small edit).
- docs/dev/debug.md: check each command exists (`python -m harness.route_probe --help` etc.) before documenting it; document only what runs.
- Tests: `test_troubleshoot_has_section_per_code` (every messages.MESSAGES code has `### <CODE>` in troubleshoot.md — may already exist in
  tests/plugin/test_messages.py; extend for "exited with code N" + wheel/venv sections), `tests/test_docs_links.py` (every relative link and anchor in
  README.md, CONTRIBUTING.md, docs/user/*.md, docs/dev/*.md resolves).
- Acceptance: `.venv/bin/pytest -q tests/plugin tests/test_docs_links.py` green; `PYTHONPATH=plugin .venv/bin/python -m topor.docs_gen --check`
  clean; default suite once before commit. Commit message from the plan step.
