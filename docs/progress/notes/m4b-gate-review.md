# M4b gate review (Task B12 Step 3, B3 "F review at the gate")

Reviewer: independent F, read-only, main @ 3bc69f5 (version 0.4.0). Scope: plan
`docs/superpowers/plans/2026-11-01-topor-m4-in-kicad.md` B12 Step 3 + B3 + Global Constraints / Review Focus,
rulings M4-R1..R8 (STATE) and C0..C27 (ledger). Workers' reports were treated as claims to check, not facts.

Commands run (focused, no gate / full suite): `pytest tests/plugin/test_no_pcbnew.py test_preflight.py
test_scope.py` (45 passed); small `.venv/bin/python -c` probes, quoted in the findings. No repo file was changed.

**Counts: BLOCKING 1 · IMPORTANT 7 · MINOR 14.**

| # | Sev | One line |
|---|---|---|
| B-1 | BLOCKING | An engine panic hangs the plugin forever: `RouteJob.done()` never becomes true, `run.route_fn` polls `done()` without end; the panic test models a `done()` the real engine does not have |
| I-1 | IMPORTANT | A failure after `port.commit` (refill, run_action, report write) reports `status "error"` + INTERNAL *"The board was not changed"* although the board was changed |
| I-2 | IMPORTANT | Accept is commit + `refill_zones()` (+ optional cleanup/teardrop actions) = likely 2+ undo steps; docs and checklist R7 promise one Ctrl+Z reverts all |
| I-3 | IMPORTANT | `via_setup_missing` reads `.kicad_pro` key `via_sizes_list`, which KiCad does not write (it is `via_dimensions`); tests use the invented key; VIA_NOT_SET_UP fires on boards that do have pre-defined vias |
| I-4 | IMPORTANT | Via sizes / rule floors come from a hidden JLCPCB profile (engine adds 0.25–0.5 mm drill vias from `routing_vias`, KiCad's pre-defined vias are never read, design values below JLCPCB minimums are raised); the user guide says KiCad's board setup decides |
| I-5 | IMPORTANT | Failures before `run()`'s `try` (report dir not writable, log file, KiCad API connection) end the GUI process with no dialog at all |
| I-6 | IMPORTANT | In the default `existing_copper = "fixed"` path the pour fills (and pour reach) come from the live board's possibly stale fills; operate.md says "the copy is refilled first"; stale fills can over-reach, against M4-R8 |
| I-7 | IMPORTANT | The self-documenting `topor.toml` comments and the generated configuration.md describe effects that 0.4.0 does not have (`detour_reference`, `pour_served`, whole-board `ignore_pads`, `existing_copper = "smooth"`) |

---

## BLOCKING

### B-1 Engine panic → infinite hang (exception path "engine panic", Review Focus "never panic")

Evidence:
- `engine/src/py.rs:141-157` `route_async`: the worker thread runs `route_with(...)` and only then
  `f2.store(true, Ordering::Release)` (line 150). A panic unwinds out of the closure before line 150, so
  `finished` stays `false` for ever. `done()` (line 117) returns only that flag. There is no `catch_unwind`,
  no drop guard and no `panic = "abort"` profile anywhere in `engine/` (grep).
- `plugin/topor/run.py:499-516` `route_fn`: `while True: done = job.done(); ... if done: break; time.sleep(0.02)`.
  `job.wait()` (which would turn the join error into `ValueError("routing thread panicked")`, py.rs:127) is
  only reached after `done()` is true. With a panicked thread the loop never ends. Cancel calls
  `job.cancel()` once (line 512) and keeps polling; the progress window's close box is vetoed
  (`ui_wx.py:689-692`). The user has to kill the plugin process; nothing is written to the report or the
  crash log.
- `tests/plugin/test_run_flow.py:151-170` `PanicJob.done()` returns `True` after the "panic" and `wait()`
  raises. That is not what the real `RouteJob` does, so `test_run_engine_panic_reported_not_crashed` passes
  while the real behaviour is a hang (vacuous for the real engine). `troubleshoot.md:103` and `app.py`'s
  docstring ("Rust panic ... gives exit 1") describe the intended behaviour, not the actual one.

Fix shape: in `py.rs` set `finished` from a drop guard inside the thread closure (`struct Done(Arc<AtomicBool>);
impl Drop for Done { fn drop(&mut self) { self.0.store(true, Release) } }`), or make `done()` also return
`handle.as_ref().map_or(true, |h| h.is_finished())`. Add a regression test that drives the REAL `RouteJob`
through a panic (e.g. a test-only `Options` flag / `#[cfg(test)]` hook that panics in `route_with`, or a Rust
unit test of `RouteJob` with an injected closure) and asserts `done()` becomes true and `wait()` raises.
Belt and braces in `run.py`: after `job.cancel()`, stop polling after a bounded grace period and raise
`RuntimeError("engine did not stop")` so the normal INTERNAL path reports it.

---

## IMPORTANT

### I-1 Errors after the commit are reported as "board not changed" (exception paths, undo atomicity)

`run.py:610-631`: after `out.commit = port.commit(req)` the flow calls `port.refill_zones()`, optionally
`port.run_action(...)`, then `report.write(...)` and `ui.done(...)`. Any exception there (kipy
`refill_zones` polls only `max_poll_seconds=30` and later calls return `AS_BUSY` on a large board; an unknown
action name; disk full) goes to `run()`'s handler (`run.py:353-369`), which sets `status = "error"`, builds
`INTERNAL` and shows the error dialog. The catalogue text for INTERNAL (`messages.py:62`,
`troubleshoot.md:261`) and the exit-code section (`troubleshoot.md:103`) say *"The board was not changed"*,
but the tracks are already committed. `result.json` says `error`, `report.md` outcome is the error.

Fix shape: wrap everything after a successful `port.commit` in its own `try`; on failure keep
`status = "accepted"`, append a warning finding (new code, e.g. `POST_COMMIT_FAILED`, cause "applied, but
refill / cleanup / report failed", fix "press B / run the action yourself", troubleshoot section), and make
the generic INTERNAL text conditional ("the board was not changed" only when `out.commit is None`). Test:
FakeBoard whose `refill_zones` raises → status accepted, one commit, warning present.

### I-2 "Accept = one undo step" vs. a separate zone refill (and cleanup/teardrop actions)

`run.py:610-619` pushes the commit, then calls `IpcBoard.refill_zones()` (`ipc.py:242-243`, kipy
`Board.refill_zones` = API `RefillZones`, outside the commit) and optionally `run_action` for cleanup /
teardrops. In KiCad the refill runs the zone-filler tool, which pushes its own commit ("Fill Zone(s)"), i.e.
its own undo entry (UNVERIFIED live, but the call is certainly outside `begin_commit`/`push_commit`); the
cleanup and teardrop dialogs certainly push their own commits. `operate.md:170` ("then zones are refilled.
One Edit → Undo (Ctrl+Z) reverts all of it"), README status table, the wireframe's done wording and
checklist R7 ("All topor tracks disappear ... in one step") promise one Ctrl+Z. If the refill is an undo
entry, the first Ctrl+Z only reverts the refill and R7 fails — a hard M4b gate item. The checklist row for
`refill_zones` (gui-test-checklist.md:117 (B2 rows)) only checks that zones are refilled, not the undo stack.

Fix shape: decide before the GUI test. Either (a) drop the post-commit refill and tell the user to press B
(the DoneDialog text and operate.md say so; KiCad's DRC refills anyway), or (b) keep it and document "Accept
= one undo step for the routing; zone refill and the optional cleanup / teardrops are further steps" in
operate.md, README, the done wording and R7; add a checklist row "after Accept, Edit → Undo shows 'Topor:
routed …' as the top entry / count the undo entries". Make it a `[run]` key if in doubt (memory: configurable
with a sane default).

### I-3 VIA_NOT_SET_UP reads a key KiCad does not write (Q-b correctness)

`preflight.py:131`: `if ds.get("via_sizes_list"): return False`. KiCad's `.kicad_pro` stores pre-defined vias
as `board.design_settings.via_dimensions` (every snapshot project has it; KiCad always writes a
`{"diameter": 0.0, "drill": 0.0}` placeholder first, e.g. wiccon has only the placeholder, bornhack has 4
real entries). `via_sizes_list` exists nowhere except `tests/plugin/test_preflight.py:56` and
`test_run_flow.py:533`, so the tests confirm the invented key. Probe:

```
pro = {via_dimensions: [{0,0}, {0.6,0.3}], classes: [Default 0.8/0.4]}
via_setup_missing(pro) -> True     # "KiCad has no via sizes set up" although 0.6/0.3 is set up
```

Effect: on a board with pre-defined vias but factory netclass vias the warning is wrong, and
`apply_via_fallback` (run.py:405-414) patches every per-net rule with the factory or default size to the
`[vias] fallback_*` values; with a non-default fallback in topor.toml that overrides what the user set up.
The six snapshot boards happen not to hit it (their classes are non-factory or their lists are empty), so
the QB12 numbers are unaffected. The module docstring (preflight.py:14) repeats the wrong key.

Fix shape: read `via_dimensions`, ignore entries with `diameter == 0 and drill == 0`; fixtures from a real
`.kicad_pro` (copy the structure of a snapshot project, not a hand-made dict); one test with a placeholder-only
list (fires) and one with a real entry (does not fire).

### I-4 Hidden JLCPCB profile decides via sizes and floors; docs say KiCad decides

- `toporlib/rules.py:21-41, 176-196, 269-291`: every resolved rule is floored to `JLCPCB_1OZ`
  (`apply_fab_floors=True`, `run.resolve_rules` does not change it), and the rules carry
  `routing_vias = [250_000, 300_000, 400_000, 500_000]` from the profile. KiCad's pre-defined vias
  (`via_dimensions`) are never read (grep: no reader).
- `engine/src/vias.rs:28-51` (`via_sizes`, line 32) `via_sizes`: after the net's own via, the engine may use every
  `routing_vias` drill in `[min_via_drill, via_drill)` with `d = max(drill + 2·min_annular, min_via_diameter)`.
  So on wiccon (only the factory 0.8 / 0.4 via, VIA_NOT_SET_UP) the router may place e.g. 0.5 / 0.3 mm vias
  the user never defined, although the finding says "topor falls back to 0.8 / 0.4 mm".
- `operate.md:27-38` ("KiCad's board setup for everything KiCad already knows … Via sizes and pre-defined
  vias … Pre-defined Sizes → Vias"), 46 §1 ("There is no fab profile that overrides the board") and 46 row 2
  ("per net, the netclass via first, then other pre-defined vias") disagree with the code; README's status
  table lists "via rules from the board setup" under 0.5.0, i.e. the README admits it, the user guide does not.
  No user doc mentions JLCPCB, floors or the extra via sizes (grep).

The floors and drill ≥ 0.25 mm are a recorded user decision (project memory "JLCPCB fallback", Global
Constraints "fab floors"), so the behaviour itself is not the defect; the undocumented behaviour and the
VIA_NOT_SET_UP wording are. Fix shape: operate.md "Where the settings come from" + Limitations: name the
fab floor profile (values, which rules it raises, that the report's `floors_applied` lists them), the extra
smaller via sizes and their drills; VIA_NOT_SET_UP message: "0.8 / 0.4 mm, plus smaller fab-profile vias
where space requires"; list `floors_applied` in the run report Settings block if it is not there. If the user
wants 46 row 2 literally, build `routing_vias` from `via_dimensions` (non-zero entries) and fall back to the
profile only when the list is empty — that is a 0.5.0 item per README, so record it as deferred.

### I-5 Errors before `run()`'s `try` show nothing in the GUI

- `run.py:336-339`: `report_dir = _unique(cfg.report_dir_for(...))`, `report_dir.mkdir(...)` and
  `_Log(report_dir / "log.txt")` run before the `try` at line 350. A non-writable report root
  (`[output] report_dir` pointing into a read-only project, a full disk, a cache dir owned by root) raises
  out of `run()`; `app.main` (`app.py:156-171`) prints to stderr and the crash log and returns 1. The wx
  dialog is never shown, so in KiCad the button does nothing visible.
- Same for `run_gui` (`app.py:127-151`): `KiCad(...)`/`kicad.get_board()` failing (API off — the most likely
  install error, troubleshoot.md "Cannot connect to KiCad") ends without a dialog although `WxUi` already
  exists (it is used for the ENGINE_VERSION case two lines earlier).

Fix shape: in `run()`, fall back to a temp report dir (and a finding `REPORT_DIR_UNWRITABLE` with the
`output.report_dir` key as fix, troubleshoot section) instead of raising; in `run_gui`, catch the connection
error and `ui.error(Finding("KICAD_API_UNAVAILABLE", ...))` (fix: Preferences → Plugins → Enable KiCad API),
then exit 1. `app.main`'s `BaseException` handler could also try `ui.error` when a wx app exists.

### I-6 Pour fills / pour reach from stale live fills in the default path (M4-R8, docs vs behaviour)

`IpcBoard.read_model` → `export_model_ipc` takes `zone.filled_polygons` from the LIVE board
(`model_ipc.py:579-600`, `zone_to_pour`); its docstring (line 684) says the zones "should be refilled before
the call", but the flow never refills before reading (correctly: Reject must not refill the live board).
Only when copper is ripped does `run.py:473-482` refill the ripped copy with kicad-cli and replace the fills
(ruling C20). With the default `existing_copper = "fixed"` (and `smooth`) the engine sees:
- stale fills: after the user moved parts without pressing B, a stale fill can still cover a pad that a
  refill no longer reaches → `pour_reach` says "reached" → no connection is routed → `unconnected` after
  refill. That is the non-conservative direction M4-R8 forbids ("pour reach errs toward not reached");
- never-filled zones: the outline stands in as same-net copper (`model_ipc.py:598-602`), i.e. the whole
  zone area blocks every other net on that layer.
`operate.md:274` claims "Zone fills are taken exactly as KiCad filled them (the copy is refilled first)".

Fix shape (cheap, no extra kicad-cli run): run the original DRC on the copy with `save_board=True`
(`run.py:429`; it already uses `--refill-zones`) and apply `refill_pours(model, pcb_copy.read_text())` to the
engine model in every path (the ripped path keeps its own refill). Keep `copy_text` (pre-refill) only where
byte identity matters, or re-read it. Test: FakeBoard whose model fills cover a pad that the refilled copy
does not → the connection is routed.

### I-7 topor.toml / configuration.md describe effects 0.4.0 does not have

`schema.py` docs (rendered into every new `topor.toml` and into `docs/user/configuration.md`):
`run.detour_reference` "Compare every routed connection with a DRC-feasible direct path and report detours"
(no reader in plugin/ or engine, grep); `nets.pour_served` "force or forbid serving a net by a copper pour"
(no reader); `nets.ignore_pads` "skipped as terminals" (only in selection scopes; the engine has no
`ignore_pads`, `build_options` drops it, `run.py:177-180`); `run.existing_copper` "smooth = keep topology but
smooth" (`engine/src/route.rs:2589-2591`: "ignored ... treated as fixed"). Only operate.md Limitations says
so (B11 report concern 1, not resolved by R8/QB12). The toml file is the "self-documenting" interface the
user edits (46 §1.1); the generated reference is the canonical key table.

Fix shape: mark them `since="0.5.0"` (commented out, like the other future keys), or append "(no effect in
0.4.0)" / "(selection only in 0.4.0)" to the doc strings and regenerate configuration.md; add a test that
every live key with `engine_field=None` has a consumer in plugin/ (grep of `cfg.get(section, key)`) or the
"no effect" marker.

---

## MINOR

1. **pcbfile via layers on 4+ layers.** `toporlib/pcbfile.py:107` `la = it.get("layers") or
   sorted(layer_names)[:1] + sorted(layer_names)[-1:]`; engine vias always have `layers: []`
   (`engine/src/route.rs:670-681`), so on a 4-layer board (ids 0, 2, 4, 6) the DRC/preview copy gets
   `(layers "F.Cu" "In2.Cu")` (probe output). KiCad probably sanitises a through via, but the file is wrong.
   Fix: default to the top-down first/last copper (F.Cu, B.Cu) as `to_kipy_item` does.
2. **No-pcbnew guard misses lazily imported modules.** `tests/plugin/test_no_pcbnew.py:59-74` imports a fixed
   list; `topor.model_ipc` (the main IPC module, only imported inside `IpcBoard.read_model` and
   `run.refill_pours`) and `topor.docs_gen` are never imported by the probe (checked: not in `sys.modules`),
   and the AST scan only looks for the name `pcbnew`, not `harness` (which imports pcbnew). No violation
   today. Fix: walk `topor` with `pkgutil.walk_packages` like `toporlib`, and add `harness` to the forbidden
   module names of the AST scan.
3. **kipy outside ipc.py / model_ipc.py.** `app.py:131` `from kipy import KiCad`; `run.py:142` imports
   `model_ipc` (kipy at module level) for the pure `pour_reach` / `dangling_ends`, so the port-agnostic run
   flow needs kipy for any ripped FakeBoard run. Fix: `IpcBoard.connect()` in ipc.py; move the two pure
   functions to `toporlib/geom.py` (or a pure `topor/pourreach.py`).
4. **`timeout_s` "whole run including DRC rounds" is not what is charged.** `drcloop.py:192-199` charges only
   `route_fn` time (`router_s`); the pre-flight DRC, the refill, every DRC round and the preview's kicad-cli
   runs are not counted, so a 60 s limit can run well past 60 s. Fix: charge wall time since `t0` (pass a
   deadline into drcloop) or reword the doc to "router time, DRC rounds excluded".
5. **Help opens a raw Markdown file.** `ui_wx.py:605-607` `LaunchDefaultBrowser(file://…/operate.md#options)`:
   browsers show `.md` as plain text or offer a download; the `#options` fragment has no effect. Fix: ship an
   HTML rendering in build.sh (anchors from the same slug rule as test_docs_links), or open the file with
   `open_path` and drop the anchor claim from the wireframe/checklist R11.
6. **INTERNAL fix text points at a file the user cannot get.** `run.py:359` "file a bug with the save_as copy"
   — the work dir is removed in `run_gui`'s `finally` (`app.py:148-151`). messages.py was fixed in B11; this
   string was not. Fix: use the catalogue fix (`messages.fix_for("INTERNAL")`).
7. **Detour reference wording.** `operate.md:255` "detours measured against the straight line between the
   pads" — the ratio is routed length / topological ghost length (`toporlib/detour.py:1-15`). Reword.
8. **DO_NOT_TOUCH fix names a dialog that does not list nets.** `messages.py:102` "Inspect → Board Statistics →
   Nets shows them": Board Statistics lists counts, not net names; the Net Inspector panel does. Add to the
   GUI checklist's menu-path row or reword.
9. **`KICAD_DOCUMENTS_HOME` semantics.** `tools/build.sh:7,14` treats it as the versioned dir
   (`…/kicad/10.0`); in KiCad the variable is the unversioned root (`~/.local/share/kicad`), so a user who
   copies KiCad's value installs into `<root>/plugins`. Rename the override (`TOPOR_KICAD_HOME`) or append
   `10.0`.
10. **Dialog frozen during kicad-cli.** `WxUi` pumps events only inside `ui.progress` (`ui_wx.py:1079-1089`);
    `run_drc` / the refill / the SVG export block for 5–20 s each (`toporlib/drc.py:68`, no timeout either),
    so Cancel is not processed and GTK may grey the window; there is no window at all during the IPC model
    export before the first progress call. Fix: `subprocess.Popen` + poll with `ui.progress` (kill on cancel
    in the pre-flight phase), a `timeout` on kicad-cli, and an early `emit("setup")` before `read_model`.
    The checklist's "Cancel latency within a second" only holds in the route phase — say so.
11. **Board edited while topor runs.** KiCad stays editable during the run; Accept commits a result computed
    on the start-of-run copy without checking that the board is unchanged (a deleted ripped track makes the
    commit fail → dropped, fine; a moved part is silently overlapped). Fix: re-check a cheap fingerprint
    (item count + a hash of `get_tracks`/`get_footprints` positions) before `begin_commit` and refuse with a
    finding; at least document "do not edit the board while topor runs".
12. **Gate test could pin two more facts.** `tests/gate/test_m4b_gate.py:131-133`: FakeBoard silently skips
    unknown uuids (`fake.py:85-86`), so add `out.commit.removed == len(out.ripped)`; add
    `board_port.refills == 1 and board_port.actions == []`. The kicad-cli judgement of the committed copy
    (`unconnected == 0` absolute, `routing_new == 0`) is real, not vacuous.
13. **Net-code alignment and the dru source.** `run.resolve_rules` (`run.py:268-276`) lets
    `resolve_rules_from_project` derive codes from the keys of `get_netclass_for_nets`, while
    `export_model_ipc` derives them from `get_nets()` (`model_ipc.py:697-698`); any difference shifts every
    per-net rule silently. Pass `net_codes=` explicitly from one net list. `export_model_ipc._dru_text`
    (`model_ipc.py:667-675`) reads `.kicad_dru` from the live project dir, the rules read it from the copy;
    pass the copy's dru text from `IpcBoard.read_model`.
14. **Live parity test details.** `test_ipc_parity.py:145-147` compares keepouts by exact polygon, but IPC
    flattens arc edges outward and the harness inscribed, so a conservative difference fails; compare by
    containment like copper shapes. `test_ipc_parity` builds the harness model from the file on disk but
    refills the live board first (`:351-353`); with a copy that was never refilled on disk the fills differ for
    a reason unrelated to the exporter — refill/save the copy first or compare against a pcbnew refill.

---

## Checked and found in order

- **Port boundaries (1).** No `pcbnew` import under plugin/ or toporlib/ (AST + probe green); no `harness`
  import in plugin/ or toporlib/. kipy only in ipc.py, model_ipc.py and the two places in MINOR 3.
- **Settings (2).** Every 46 §2 row maps to a `schema.py` key or a KiCad-only comment entry; every
  `engine_field` exists in `topor_engine.default_options()` (probe: all 22 present, `default_options` vs
  `Config.defaults().engine_options()` values identical); unit scaling mm→nm / µm→nm correct; `since="0.5.0"`
  keys are written commented out and skipped by `engine_options`. Source order: KiCad first for rules /
  via sizes (with the I-3/I-4 caveats), netclass before `power_pattern` (priority.py:89-99), `[layers]` >
  KiCad types > defaults (layers.py, 46 row 13 / M4-R7), priority marker > topor_pN > table (C22).
- **Messages (3).** Every emitted code (preflight `_mk` catalogue lookups, layers, scope, priority, run, app,
  config) is in `messages.py`; troubleshoot.md's per-code part is generated from it (test-pinned).
- **Reject (4).** No port call after `ui.preview` returns False; no refill, no commit; `save_as` uses
  `SaveCopyOfDocument` (kipy board.py:589-601). Accept: `begin_commit` → remove → create → group →
  `push_commit`, `drop_commit` on any exception (ipc.py:200-220).
- **kicad-cli missing** → `KICAD_CLI_MISSING` error at pre-flight before any kicad-cli call; failing
  kicad-cli → `RuntimeError` → INTERNAL before any commit (correct text there).
- **Cancel** in the route phase: `job.cancel()`, the DRC loop reuses the last result and the cached judgement,
  bounded by `loop_cap`; nothing routed → `cancelled`, no preview.
- **UI (5).** Button rows, default / escape ids, tab-order tuples, mnemonics and the wording table match the
  wireframes (test_ui_wx compares the wording table with the note); Help path finds the installed layout
  (`parents[1]/docs/user`) and the repo; build.sh ships docs/user.
- **B3 / M4-R8 (7).** Copper graphics circumscribed (tangent construction, 500 nm vertex error), text box
  grown 0.5 × size, fills exact, pour reach by probes strictly inside the pad and via centre, `via_free`
  missing → True, free vias never ripped (`scope.py:264`), `keep_unconnected_vias` keeps untouched vias;
  tests assert the directions (`minus_area(pcbnew, IPC + 1 µm) == 0`, reach IPC ⊆ pcbnew, flags == GetIsFree)
  and `test_compare_models_asserts_the_conservative_directions` shows a shrunken obstacle / IPC-only reach /
  fill change / flag flip fail. Exception: I-6 (stale live fills are outside what those tests see).
- **Q-b (8).** `apply_via_fallback` writes `via_d` / `via_drill` in nm (the fields `engine/src/vias.rs:32-37`
  reads) on the default rule and every per-net rule with the factory or default size, clamps to the board
  minimums, patches both the rules and `model["rules"]`, and reports in log.txt, report.md and result.json.
  Caveats I-3 (trigger) and I-4 (extra profile vias).
- **Gate test (9).** One FakeBoard commit equal to the flow's items / rip list, 27/27 and 62/62, kicad-cli DRC
  of the committed copy with absolute `unconnected == 0` and `routing_new == 0`, report-dir files and
  `result.json`, topor.toml created on the first run and read back, `docs_gen --check`, snapshot hash
  unchanged — the claims hold for the headless path (MINOR 12 for two extra assertions). It cannot cover
  B-1 (fake panic), I-2 (undo stack) or I-6 (harness model fills).
