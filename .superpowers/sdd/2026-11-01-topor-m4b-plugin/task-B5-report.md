# Task B5 report — run flow (DONE_WITH_CONCERNS)

Base: worktree branch was behind main (no B2/B3/B9); fast-forwarded to main (d9aab67) first.

## API
`run(port, ui, cfg=None, engine=None, *, tmp_root, now=None, config_findings=(), versions=None) -> RunOutcome`
(cfg None -> `load_config` + `ui.first_run_notice` on CFG_CREATED; engine None -> `topor_engine`).
RunOutcome: status (aborted_preflight|cancelled|rejected|accepted|error), report (RunReport), commit (CommitResult),
report_dir, findings, error, routed, total, stop_reason, cancelled, items (committed items, group set), ripped, model,
drc (classify dict), base_drc, orig_drc, rounds, router_s, wall_s, work_dir, pcb_copy, ripped_pcb, routed_pcb.
Port changes (C19): `BoardPort.read_model(rules, pcb_copy, *, layer_roles=None)` (FakeBoard ignores, IpcBoard forwards),
`UiPort.set_context(...)` + ScriptedUi records it. `CommitRequest.group` is a bool in ports; the group NAME is per item
(`item["group"]` = engine's "topor-partial" else "topor").

## Seams (plugin/topor/run.py)
B6: `preflight_findings`, `connection_estimate`, `apply_accepted_findings` (PAD_OUTSIDE_BOARD -> nets.ignore_pads,
DRILL_OUTSIDE_FILTER -> vias.allowed_drills_mm; unit-tested). B7: `rip_plan`, `filter_model` (also filters dangling ends and
pour via reach), `build_options` (only keys in engine default_options are passed; exclude_nets/ignore_pads wired for A9).
B8: `report.RunReport` (`facts` dict, `write(result=, drc_src=)`, `summary_lines()`, `set_outcome`); outcome wording table there.
Flow order: set_context -> save_copy -> rules (toporlib.rules from copy .kicad_pro/.dru + port.netclasses) -> read_model ->
findings (config, layer roles, KICAD_CLI_MISSING; orig DRC -> PREEXISTING_DRC info) -> ui.preflight -> rip plan ->
baseline DRC -> route_with_drc_loop (route_async polled 0.25 s -> ui.progress; False -> job.cancel(), still wait()) ->
preview (report.md/preview.png written before ui.preview) -> Accept: one commit, refill, cleanup/teardrop run_action
(CLEANUP_SKIPPED_PARTIAL when partial) / Reject: nothing. Progress phases: engine setup|route|done + preflight_drc|drc|preview.
No items routed -> no preview, status cancelled (cancel) or rejected (NOTHING_ROUTED warning), never an empty commit
(would delete ripped copper). timeout_s=0 -> engine wall_clock_s 0, loop budget 1e9. Preview failure -> PREVIEW_FAILED warning, run continues.

## Preview
`kicad-cli pcb export svg --layers F.Cu,B.Cu,Edge.Cuts --exclude-drawing-sheet --page-size-mode 2 --mode-single -o out.svg board`
(before = save_as copy, after = routed copy). Two exports, wx.svg rasterise, wx.MemoryDC side by side, caption lines, titles,
legend with swatches taken from the export's own layer colours (default export uses KiCad classic theme, not the user's theme).
Highlight: SVG origin = Edge.Cuts centre-line bbox min (verified on eprom: pad 187.5/71.0 -> svg 3.5/9.0 vs outline min 184/62),
so new items are appended as SVG (halo 3x width, tint, translucent, + core in layer colour); skipped with a note if SVG size
differs from model outline by >0.3 mm. Tint = (0,139,139) unless <3:1 against SYS_COLOUR_WINDOW then (208,64,208) (it used the fallback on this dark desktop).
Not done: existing copper not dimmed in the Result panel (needs re-styled export). Zone fills hide B.Cu tracks of the Current board in the export (theme draw order).

## Numbers (Fake board, real engine, real kicad-cli, committed copy re-judged)
- eprom: routed 27/27, vias 0, routing_new 0, unconnected 0 (committed copy, baseline = ripped copy / original), DRC rounds 1, router_s 0.09, wall 15.6 s.
- wiccon-2023: routed 67/67 connections (not 62: see concern), vias 42, routing_new 0, unconnected 0, rounds 1, router_s 3.63 (22 iterations, stop "no gain"), wall 20.0 s, length outside pads 1384.0 mm.
PNGs: docs/benchmarks/progress/2026-10-31-m4-B5-preview-{eprom-to-kernal,wiccon-2023}.png.
Tests: tests/plugin/test_run_flow.py 15 passed in 130 s (accept flow setup 19 s, other flows 11-16 s each, each ~3 kicad-cli DRC + 2 svg). tests/plugin rest: 84 passed, 7 skipped (test_no_pcbnew includes topor.run/preview/report).

## Concerns / findings
1. wiccon: 67 connections / 42 vias / 1384 mm vs harness 62 / <=37 / <=1380. Cause: the model passed in is exported BEFORE the rip, so pour reach (pads/vias served by pours, fills) reflects hand copper; the harness exports after rip+save. Same would hold for the IPC model (fills of the live board). Needs a decision (re-derive reach after rip, or refill zones on the ripped copy) before the M4b gate numbers "62/62" can be met; not done in B5 (model fidelity, not flow).
2. Snapshot wiccon is a KiCad-6 file (tstamp, numeric nets): pcbfile.find_items/remove_items/append_items need KiCad-10 files, which `save_as` always produces; my wiccon script re-saves via pcbnew first (test-side). A raw v6 file as a FakeBoard input silently ripped nothing (354 DRC violations) — consider a guard in FakeBoard/pcbfile.
3. KiCad file-name vs project name: report folder uses the pcb stem ("2023" for wiccon).
4. RunReport is minimal (B8 replaces); length_before is n/a.

## UNVERIFIED (GUI checklist)
WxUi progress/preview dialogs driven by the real flow; PNG legibility on light theme and 200 % scaling; wx.GetApp present in KiCad plugin process; IpcBoard.read_model(layer_roles=...) live; svg export against a live-edited unsaved board; highlight alignment on boards whose Edge.Cuts bbox is not the SVG origin (board-area mode with other outlines); cancel latency of the engine from the dialog.
