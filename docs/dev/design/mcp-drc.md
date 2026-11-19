# mcp-drc: KiCad-DRC feedback tools in topor's MCP server (design note)

> **As built (2026-11-19), where it differs from the plan below.** (a) One KiCad session per run board copy, started
> by the first GUI call and reused: DRC runs once per board state, `drc_spots` walks the whole row list in it, a
> failed row is reported and the walk goes on, `cleanup` runs in the same session (lead/user rule). (b) Rows are
> selected by one real click on the list's first line + Down keys, each step read back (`drc_selected`): KiCad
> 10.0.8 crashed 3x on a synthetic `DataViewCtrl.Select/EnsureVisible/GetItemRect` of a solder_mask_bridge row, a
> real click never did. (c) No screen->board transform: KiCad's cross-probe centres the marker in the canvas part
> left of the dialog; the pointer is moved there (no click) and the status bar X/Y read (`focus_mm`, finest grid);
> `match` = same text + items, selection on it, focus within 1 mm of the row's nearest track/via. (d) KiCad 10 keeps
> the DRC gear toggles in pcbnew.json `"DRC"` (`report_all_track_errors`, `crossprobe`, `scroll_on_crossprobe`);
> with the KiCad 9 `drc_dialog` keys the GUI showed 49 rows where kicad-cli `--all-track-errors` showed 51.
> (e) Helper commands as listed in docs/dev/mcp.md "DRC feedback".

**What.** Five tools next to route/report/explain: `drc(run_id)`, `drc_spots(run_id, cls=None)`,
`cleanup(run_id, options)`, `compare(run_a, run_b)`, plus `register_run(routed_copy, input_copy, result=None)` so
a board routed in real KiCad (gui_e2e) can use them too, and `gui_session(action)` (status/stop). Same
protected-roots rule; everything works on copies under the run's out dir (`<out>/mcp/...`).

**Probe results (KiCad 10.0.8, kipy 0.8.0, own Xephyr :9, closed by pid).** kipy has no DRC or cleanup message.
`run_action("pcbnew.DRCTool.runDRC")` opens the DRC dialog (RAS_OK, non-modal). `run_action(cleanupTracksAndVias)`
opens the modal dialog but the API call times out and KiCad stays "busy" until it closes. So the API can OPEN the
dialogs, never fill/run them. A test-only wx helper loaded inside KiCad can: a SWIG-era action plugin in the
isolated home's `scripting/plugins` is imported at start, wx works there, and with wx events (by widget id/label,
no pixel coordinates) it ran DRC (48 rows on a routed wiccon = kicad-cli's 48), read every row's text and item
lines from the dataview model, selected a row (KiCad cross-probes: pan + active layer), set Cleanup check boxes by
label, pressed Build Changes (label turns "Update PCB") and read the change list.

**How.**
1. `mcp_server/drc.py` (no GUI): kicad-cli `--refill-zones --all-track-errors` on a copy with the project
   snapshot. Proof the project was used: `.kicad_pro`/`.kicad_dru` bytes captured once, present beside the copy with
   the same sha256 before AND after kicad-cli, and every quoted clearance is one the .kicad_pro defines (net class
   or min_clearance). Else ToolError `DRC_WITHOUT_PROJECT` / `RULES_MISMATCH` (no numbers returned). Delta vs the
   unrouted input (`<name>-input/`): violation new iff (type + sorted uuids) not in the input DRC and one item is new
   copper. Row: id, type, class (type + item-kind pair), sev, nets, layers, x/y, uuids/kinds/ours, text, and
   `ours`: our track/via uuids with the engine connection ids they serve (connected component of our copper of that
   net, matched to `route.connections` whose from/to points it touches; the engine writes no per-item wire id) and
   the nearest engine violation (conn, cause). Ported from the stopped agent's `drc_feedback.py` (logic sound);
   results cached by board+project sha so compare reuses them.
2. `mcp_server/kicad_gui.py`: `GuiSession` reuses gui_e2e `E2E` (isolated home, Xephyr, metacity, pcbnew, ipc,
   shot, stop). One session at a time, explicit handle, free display from :20..:29, reused only for the same board
   copy while pcbnew's pid lives and the helper answers; idle timeout (default 600 s), atexit/SIGTERM stop, stop
   by own pids only. Helper `harness/gui_helper/topor_gui_helper.py`: localhost socket + 0600 token file, FIXED
   command set (no exec): windows, place, drc_open/run/rows/select, cleanup_open/set/build/changes/apply, close,
   status_text. Every step: bounded timeout, effect verified (dialog present, tab counts set, label "Update PCB",
   dialog gone, file sha changed), else loud ToolError naming the step + a screenshot; never retried by improvising.
   Helper not loaded (e.g. KiCad 11 drops legacy plugins) = loud error, no pixel fallback.
3. `drc_spots`: dialog placed at a fixed rect on the right; zoom preset fixed; per wanted row (rows matched to the
   index by text, not by position): select -> settle -> screenshot dialog + canvas; screen->board transform read
   from the status bar (pointer warped to two canvas points) so the board PNG is cropped centred on the marker;
   record = {id, class, row, dlg_png, board_png, gui_text, cli_text, match (row text+items == kicad-cli record),
   in_view}. Pictures still need a look; match/in_view catch wrong-row/wrong-spot.
4. `cleanup`: snapshot uuids (IPC) -> open via posted menu event -> boxes by label (refill, short, vias,
   dangling_vias, merge, unconnected, in_pads = gui_e2e CLEANUP_BOXES), filters off -> Build -> change list rows
   -> Update PCB -> save -> removed by uuid/kind -> kicad-cli DRC (step 1) -> per-class delta after vs before,
   newly unconnected per net. Optional `replay` = harness.cleanup_check cross-check.
5. `compare`: per-class ours-new counts a, b, delta; unconnected; refuses different input boards.

**Borrowing.** Pattern from mixelpixx/KiCAD-MCP-Server `gui_driver_plugin` (MIT, 1e654552): listener thread +
wx.CallAfter executor, label normalisation, event injection; copied parts keep the MIT notice; provenance table
in docs/dev/mcp.md. Evaluated, nothing copied: lamaalrajih/kicad-mcp (MIT; plain DRC without refill/project),
ProductOfAmerica/mcp-server-kicad (GPL-3.0; plain DRC), nmlsports (MIT; socket lookup). Seeed-Studio: no licence,
not read. Old ad-hoc tools: kicad_session/drc_records/drc_spots not reused (fixed sleeps, pixel rows, row order =
report order, board edited to zoom); drc_record_index replaced by text matching.

**Invariants.** Never the user's KiCad or ~/work/kicad/projects; every process we start is stopped by pid; a DRC
number is only returned when the project proof passed; GUI tools never return partial success silently.

**Tests.** Unit (no KiCad): recorded wiccon kicad-cli JSON fixtures (public) for delta/classes/ours matching,
project proof (missing/changed .kicad_pro, quoted clearance mismatch), compare; helper protocol with a stub
executor (token, unknown command, timeout). One Xephyr e2e (gate marker) on a routed wiccon copy: drc_spots for one
class (match true, PNGs exist) + cleanup (merge,vias,short,unconnected: removed counts, DRC delta) + session gone.
Skill drc-feedback rewritten to call the tools.
