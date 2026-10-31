# Task B11 report: user guide, GUI test checklist, CONTRIBUTING, debug guide (M4b, 0.4.0)

Status: DONE_WITH_CONCERNS (see Concerns). Branch `worktree-agent-ad92ae9abcddabbf1`; the worktree started at d9aab67
(pre-B5) and was fast-forwarded to main 8b2a7b6 before any work.

## What changed, per file
- `docs/user/install.md` (new): requirements (Linux, KiCad 10.0 + kicad-cli, Python ≥ 3.11 for the plugin env, system wxPython,
  Rust, network on first env creation), `tools/dev-venv.sh` + `tools/build.sh` (installed layout incl. `docs/user/`), `--dev`,
  Preferences → Plugins → Enable KiCad API / Python interpreter, plugin environment path + requirements.txt, Recreate Plugin
  Environment (+ delete-the-venv fallback), first check on a copy, update, uninstall, file locations.
- `docs/user/operate.md` (new): quick start; settings sources; table of every KiCad setting read, with the exact menu paths
  (all `schema.KICAD_ONLY` places + every `Setting.kicad_place`, tested); `topor.toml` sections → link to configuration.md;
  first run; pre-flight findings (accept PAD_OUTSIDE_BOARD / DRILL_OUTSIDE_FILTER); `## Options` (anchor used by Help; control ↔ key
  table); scope by selection (between / touching / whole, counts); progress + Cancel; preview; Accept / Reject / undo; report
  folder; net priorities (C22 order); keyboard; `## Limitations` (Linux; layers: engine routes every signal/mixed role layer with
  through vias, only 2-layer boards benchmarked; groups; curved only; engine knobs not applied in 0.4.0; every IPC approximation with
  its safe direction per M4-R8 / brief list).
- `docs/user/troubleshoot.md`: hand-written part before `## Findings` — where to look (dialog, report folder, crash log); install /
  start-up sections (button missing, API off, Python < 3.11, plugin env / pip, No module named topor_engine / kipy / wx, ENGINE_VERSION
  after update); exit codes 0 / 1 / 2 / other; results (nothing to route on a routed board, new DRC errors). Per-code part regenerated
  from `messages.render_troubleshoot()` (test pins it).
- `docs/user/gui-test-checklist.md` (new): setup per B10 (`bash tools/build.sh --dev`, Enable KiCad API, Recreate Plugin Environment,
  restart), fresh `scratch/gui/<board>/` copies from the snapshots (bfaf705c0f25 / 2d3063e70ed5; they are hand-routed → choose Rip up);
  run sequence R1–R13 (plan order: first-run → pre-flight → progress + Cancel → preview → Reject → run again → Accept → one Ctrl+Z →
  KiCad DRC → report folder → selection `between` on U1+U2, Help, optional error / blocked states) with expected results and the
  scope-count estimates computed from the harness models (eprom whole 30 / U1+U2 between 26 touching 30; wiccon whole 84 / between 4
  touching 31); UI acceptance (tab order per wireframe, Esc, Enter, mnemonics, min sizes, 100 % / 200 %, dark/light); IPC parity
  procedure (`KICAD_API_SOCKET=ipc:///tmp/kicad/api.sock .venv/bin/pytest -q -rs tests/plugin/test_ipc_parity.py -k eprom`, unmodified
  copy open, kipy default socket / Flatpak path, token not needed); UNVERIFIED rows from B2 (9 IpcBoard calls), B3 (live kipy fields),
  B5 (flow/preview, kicad-cli SVG background C15 Q5), B6 (via protection, VIA_NOT_SET_UP, groups, TRACK_ANGLE_RULE), B9 (visual), B10
  (install/connect/toolbar/handshake), plus "every KiCad menu path in the guides exists"; results table board × step.
- `docs/dev/debug.md` (new): only commands I ran — `harness.bench` (also `--pcb` with a free label), `harness.measure --board`,
  `cargo run --release --example route_probe -- <model> --quiet --profile` (+ `--options`, `--iters`, `--dump`), `gate_diag --iters 5
  --detour 19`, `harness.progress --milestone m4 …` (`--help` only; it writes into docs/benchmarks), `python -m topor.app --headless`
  (run on an eprom copy: `topor: rejected routed 27/27`, exit 0), reproducing a plugin run (model-export script tested on an eprom
  snapshot copy), filing a bug.
- `CONTRIBUTING.md` (new): layout incl. `toporlib/`, build (maturin develop, build.sh, `--dev` caveat, version files), tests, read-only
  projects / snapshots / copies, schema → toml → configuration.md, MESSAGES → troubleshoot.md (regeneration snippet), code rules
  (Big-O, determinism, UNVERIFIED, no detours, progress PNGs), commit style.
- `README.md`: 0.4.0 / M4b status table (works now vs 0.5.0 M4a: dcsgone 231/231 etc.), requirements (KiCad 10, Python ≥ 3.11),
  user-guide links, quick start, dev section, layout incl. toporlib. `docs/README.md`: "User guide" + "Developer" sections.
- `docs/design/45-mvp-plan.md`: M4 row records the split M4b (0.4.0 plugin) / M4a (0.5.0 engine) (M4-R2).
- `plugin/topor/schema.py`: `[nets] priority` doc aligned with C22 / priority.py (marker > topor_pN > table; exact > glob; computed and
  reported in 0.4.0, used by the engine from 0.5.0) and its `kicad_place` path corrected to "KiCad → File → Board Setup → Design Rules →
  Net Classes (…)" (was "KiCad → Board Setup → Net Classes"). `docs/user/configuration.md` regenerated (`--check` clean).
- `plugin/topor/messages.py`: MARKER_CONFLICT cause stated the reverse order ("nets.priority, then netclass, then marker") → now
  marker, then topor_pN netclass, then nets.priority (C22). CLEANUP_SKIPPED_PARTIAL fix "Inspect → Cleanup Tracks & Vias" →
  "Tools → Cleanup Tracks & Vias". INTERNAL fix promised "the board copy from the report folder", which does not exist (the save_as
  copy is in a tmp dir removed after the run) → now log.txt + result.json + a project copy saved with File → Save (debug.md).
- `plugin/topor/ui_wx.py`: Help-button bug fixed — `find_help_doc(module_file)` looks at `parents[1]/docs/user/operate.md` (installed
  `org.topor.router/topor/` layout) then `parents[2]` (repo); `HELP_DOC = find_help_doc(__file__)` (None if absent), `HELP_ANCHOR =
  "options"`; `_on_help` guards None. Nothing else in ui_wx.py touched.
- Tests: `tests/test_docs_links.py` (new, 15 incl. parametrised: required docs exist, GitHub slug rules, every relative link + anchor
  in README / CONTRIBUTING / docs/README / docs/user / docs/dev resolves, every `#anchor` and HELP_ANCHOR of ui_wx.py is a heading
  of operate.md, operate.md reaches every KiCad place + links configuration.md + names all 9 sections, Limitations names the required
  items); `tests/plugin/test_help_doc.py` (new, 5: installed layout, repo layout, installed wins / missing None, this checkout,
  build.sh ships docs next to topor); `tests/plugin/test_messages.py` (+3: generated part == render_troubleshoot, exit-code + install
  sections, MARKER_CONFLICT order); `tests/plugin/test_priority.py` (+1: doc wording vs net_priorities behaviour).

## Decisions
- `test_troubleshoot_has_section_per_code` already existed (B6); extended by separate tests instead of changing it.
- "exited with code N": headings "### The plugin exited with code 0/1/2/another code"; codes derived from app.py's `return N` + argparse 2.
- Recreate Plugin Environment: wording from doc 31 §2 (right-click the plugin action on Preferences → Plugins) — exact location is a
  GUI checklist row; the delete-the-venv fallback is always given.
- The schema `kicad_place` of `[nets] priority` was edited along with the doc string (both render into the same configuration.md
  cell; the old path lacked "File → … Design Rules").

## Tests
- Red first: 10 failed (missing docs, no find_help_doc, schema "overrides", missing troubleshoot sections); MARKER_CONFLICT test was
  first too weak (passed on the wrong text) and was tightened to the "wins …" clause before the fix.
- `.venv/bin/pytest -q tests/test_docs_links.py tests/plugin/test_help_doc.py tests/plugin/test_messages.py tests/plugin/test_priority.py
  tests/plugin/test_config_docs.py tests/plugin/test_ui_wx.py` → 63 passed.
- `PYTHONPATH=plugin .venv/bin/python -m topor.docs_gen --check` → "configuration reference is up to date".
- Default suite `.venv/bin/pytest -q`: 368 passed, 7 skipped (live KiCad), 22 deselected in 1320 s (green). No Rust touched (engine examples only built/run).

## UNVERIFIED (in the GUI checklist)
- KiCad 10 menu wording: Preferences → Plugins (Enable KiCad API, Python interpreter, Recreate Plugin Environment via right-click),
  Tools → External Plugins, Tools → Cleanup Tracks & Vias, Board Stackup → Board Finish → Via protection, Edit → Unlock Selected,
  Inspect → Board Statistics (messages.py texts not written by me).
- Whether KiCad shows "exited with code N" for a failing plugin, and where plugin stderr is visible (the crash log is the reliable path).

## Concerns
1. Keys accepted without effect in 0.4.0 and NOT marked `since 0.5.0` in schema.py: `[nets] pour_served`, `[run] detour_reference`
   (no reader in plugin/ or engine), `[nets] priority` (engine has no net_priority), `[nets] ignore_pads` on the whole board (engine
   has no ignore_pads; only selection scopes drop the pads), `existing_copper = "smooth"` (engine treats as fixed). Documented in
   operate.md Limitations; the schema flags are the coordinator's call (not changed: schema.py edit limited to the priority entry).
2. run.py's own CLEANUP_SKIPPED_PARTIAL Finding fix text still says "Inspect → Cleanup Tracks & Vias" (run.py is QB12's; one string).
3. operate.md describes `[run] keep_unconnected_vias` and the via-free / fallback behaviour as the brief states (R8 / QB12 in progress);
   reconcile after merge. R8 also regenerates configuration.md → trivial merge conflict on that file and schema.py (rerun docs_gen --write).
4. The save_as copy is deleted after a GUI run, so a user's report folder alone cannot reproduce a run; debug.md asks for a project copy.
   Keeping the copy in the report folder (opt-in) would make bug reports self-contained — suggestion, not done.
5. Headless runs always log PREVIEW_FAILED (preview needs a wx.App); documented as expected.
