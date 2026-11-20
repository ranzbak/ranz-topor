# MCP interface (S3c)

A thin MCP server (stdio) around the headless run flow and the report JSON, for reviewing and improving routes
with an AI client. Nothing here is required to use topor: every tool has a manual equivalent (last column below).
Code: `mcp_server/` (`tools.py` has the five tools as plain functions, `server.py` registers them).

Rules: board **copies** only (a path under `~/work/kicad/projects`, or a symlink into it, is refused; extra
protected roots via `TOPOR_MCP_PROTECTED`, `:`-separated); results reach KiCad only through the plugin, with the
user's approval; not inside the optimizer; `route` runs `tools/route_board.sh` in a subprocess (the router and
pcbnew stay out of the server process, whose stdout is the protocol).

## Install (optional dev dependency)

The MCP Python SDK is **not** a plugin requirement. Install it only where the server runs:

    uv pip install --python .venv/bin/python mcp          # or: .venv/bin/pip install mcp
    # without touching the venv:
    uv pip install --python .venv/bin/python --target scratch/mcp-site mcp   # then PYTHONPATH=scratch/mcp-site

Tested with `mcp` 2.3.0 (`MCPServer`); `server.py` falls back to `FastMCP` of mcp 1.x. The server needs a
`topor_engine` with the topological core: the venv's, or `TOPOR_ENGINE_SITE=<dir>` (see
[route-board.md](route-board.md)). `TOPOR_VENV` selects the venv with pcbnew (default `<repo>/.venv`).

## Start

    PYTHONPATH=<repo> .venv/bin/python -m mcp_server

Claude Code (`.mcp.json` in the project, or `claude mcp add`):

```json
{
  "mcpServers": {
    "topor": {
      "command": "/path/to/topor/.venv/bin/python",
      "args": ["-m", "mcp_server"],
      "cwd": "/path/to/topor",
      "env": {
        "PYTHONPATH": "/path/to/topor:/path/to/topor/scratch/mcp-site",
        "TOPOR_ENGINE_SITE": "/path/to/engine-wheel-site"
      }
    }
  }
}
```

    claude mcp add topor -e PYTHONPATH=/path/to/topor -- /path/to/topor/.venv/bin/python -m mcp_server

State: run ids are kept in `~/.cache/topor/mcp/runs.json` (`TOPOR_MCP_STATE` = another directory).

## Tools

| Tool | Does | Manual equivalent |
|---|---|---|
| `route(board_copy, options)` | Routes a copy. Settings come from `topor.toml` next to the board (non-defaults only), then `options.set` (`{"section.key": value}`). Other options: `timeout_s`, `png` (default true), `whole_project`, `run_timeout_s`. Returns routed/connections, DRC `unconnected`, `routing_new`, `vias`, `detour`, runtime, `run_id`, `png`, and `variants` (the report's variant table, `null` while the report has none). Output goes to `<board dir>.topor-mcp/<run_id>/` (outside the board directory), the run report to `reports/` below it. | `tools/route_board.sh <pcb> <out_dir> --set section.key=value` ([route-board.md](route-board.md)) |
| `report(run_id, variant=None)` | Warnings grouped by kind (necking, ignored_rule, time_budget, capacity, planes, ...), cuts (cut / capacity / overflow / squeeze lines), failures grouped by reason with counts and example nets, non-info findings. `variant` selects a row of the variant table when the report has one, else it errors. | `report.md` and `result.json` (`route.warnings`, `route.connections`) in the run's report folder |
| `explain(run_id, net)` | One net (name, number or unique substring): per connection status, reason and `blocking` when it failed, vias, crossings, and the heaviest factor events of its wires. | the net's rows in `report.md` "Connections"; `result.json` `route.connections` |
| `set_factor(name, weight=None, report=None, board_copy=None)` | Writes `[factors] weight` / `report` for the factor into `topor.toml` next to the board copy (default: the last one routed; the file is created with its comments when missing). Then call `route` again. | edit `topor.toml`: `[factors]` `weight = { via = 40 }`, `report = { necking = "warning" }` ([configuration.md](../user/configuration.md#factors)) |
| `review(run_id, spot, verdict, note="")` | Appends `{ts, run_id, board, spot, verdict, note}` to `reviews.jsonl` in the board copy's directory; `verdict` is `good`, `bad` or `neutral`. Input for the learning loop (S3b). | append a JSON line to `reviews.jsonl` by hand |
| `register_run(routed_copy, input_copy, result=None, name=None)` | Registers a board routed elsewhere (real KiCad, `harness/gui_e2e.py`) for the DRC tools: the routed copy, the unrouted input it came from, optionally the plugin's `result.json` (engine connection ids). Same protected-roots rule. | - |
| `drc(run_id, limit=200, severities=None)` | kicad-cli DRC (`--refill-zones --all-track-errors`) of the input and the routed copy with the board's `.kicad_pro`/`.kicad_dru` provably used, as a DELTA: one row per new violation that names our copper (id, type, class, nets, layers, x/y, item uuids, our tracks/vias with their engine connection ids, nearest engine violation), counts per class (input / routed / ours), `other_new`, unconnected per net. `rows` holds errors only by default (`severities=["error", "warning"]` adds warnings); `summary` / `ours_by_severity` count both. Fails with `DRC_WITHOUT_PROJECT` / `RULES_MISMATCH` instead of numbers under other rules. | `kicad-cli pcb drc --format json --refill-zones --all-track-errors` on both boards, diff by hand |
| `drc_spots(run_id, cls=None, per_class=0, zoom=20, keep_session=True, severities=None, match_min=0.5)` | Real KiCad (Xephyr, isolated home): Inspect -> Design Rules Checker -> Run DRC once, the dialog's Warnings box unticked (errors only by default; `severities=["error", "warning"]` keeps warnings), then every wanted row (all; `cls` = one class or type; `per_class` caps each class, shorts/crossings always kept) selected as a user does; per row the dialog and the board view at the marker (yellow ring) as PNG, GUI vs kicad-cli text, `match` (text + items + selection, and `focus_score` >= `match_min`: 1 / (1 + (focus_d_mm / scale)^2), focus_d_mm = KiCad's focus point to the row's nearest geometry, scale = 1 mm + half the row's geometry extent, graded, no cutoff), `focus_mm`/`focus_d_mm`. A row the walk cannot reach (the dialog shows other item lines: relocated by text first) is reported and the walk goes on. One KiCad for the whole list (`kicad_starts_this_call`); a failed row is reported and the walk goes on. | DRC dialog, click each row, look |
| `cleanup(run_id, options=None, replay=False, keep_session=False)` | Real Tools -> Cleanup Tracks & Vias in the same session. `options` = KiCad's box labels (list = these on; dict = label: bool; short aliases `refill short vias dangling_vias merge unconnected in_pads`); default on: "Delete tracks connecting different nets", "Delete redundant vias", "Merge co-linear tracks", "Delete tracks unconnected at one end". Returns the options as applied, KiCad's change list by text, removed items by kind, **newly unconnected per net with the engine connection ids** (`new[].conns`, `lost_copper`), DRC delta per class after vs before; `replay` = `harness.cleanup_check` cross-check. | Cleanup dialog, then DRC |
| `compare(run_a, run_b)` | Per-class delta of our new violations (a, b, delta), totals, unconnected; refuses runs whose `.kicad_pro` differs. | two DRC reports side by side |
| `gui_session(action="status")` | The server's one KiCad session: `status` (display, board, pcbnew pid, idle) or `stop`. | - |


Notes and limits:

- `explain` derives its factor events (length, via, crossing, necking) from report fields, weighted with the
  `topor.toml` weights or the documented defaults. The engine does not emit per-wire factor costs yet; when it
  does, `explain` should read them instead.
- The variant table is read from `variants` in `result.json` (top level or in `route`): a list of rows, each with
  `id` and/or `name` and optionally the variant's own `route` dict. The report has no such table yet, so `route`
  returns `variants: null` and `report(variant=...)` says so.
- Layer roles from `topor.toml [layers]` are not passed to `route` (`harness/route_board.py` resolves roles from the
  board's KiCad layer types).
- Tests: `pytest tests/mcp_server` (unit, no SDK, no router). The routing and stdio tests carry the `gate` marker:
  `PYTHONPATH=<mcp-site> TOPOR_ENGINE_SITE=<site> pytest -m gate tests/mcp_server`.

## DRC feedback (mcp-drc)

The loop "route -> KiCad DRC -> look at each spot -> Cleanup -> what broke" (skill `drc-feedback`): `route` (or
`register_run`) -> `drc` -> `drc_spots` -> `cleanup` -> `compare` between builds/settings; `gui_session stop` at the
end. Design note: [design/mcp-drc.md](design/mcp-drc.md).

- **Isolated KiCad home, always.** Every KiCad process the tools start (`kicad-cli version`, `kicad-cli pcb drc`,
  `route`'s route_board.sh with its pcbnew / kicad-cli, the GUI session) runs with HOME/XDG/KICAD_CONFIG_HOME in
  a directory of the run: even `kicad-cli version` rewrites `~/.config/kicad/10.0/kicad_common.json` otherwise.
  One helper for the whole repo's agent paths: `harness/kicad_env.py` (`isolated_env`, `run`, `run_drc`; importing
  `harness` makes it toporlib's run_cli default); `tests/test_kicad_env.py` guards every entry point.
  Tested: the env of every subprocess call, and a real kicad-cli run leaves `~/.config/kicad` byte- and
  mtime-identical.
- **DRC.** `mcp_server/drc.py`. No `.kicad_pro` beside the input board = `DRC_WITHOUT_PROJECT` (no numbers). Each DRC copy lives in `<out>/mcp/drc/<tag>-<key>/` with the project files written
  from the snapshot taken once (`topor.run.project_snapshot` / `_variant` / `project_missing`, the plugin's
  drc-project-fix code); they must be there with the same sha256 before and after kicad-cli, and every clearance
  kicad-cli quotes must be one the project defines. kicad-cli runs with an isolated HOME (template library tables
  copied, else every footprint is a `lib_footprint_issues`). Results are cached by board + project + kicad-cli
  version. New violation = (type + sorted uuids) not in the input DRC and at least one item is new copper; class =
  type + item-kind pair (`clearance track-pad`). Our copper -> engine connections: connected components of our
  tracks/vias per net, matched to `route.connections` whose both ends lie on the component (1.5 mm), else the
  nearest one flagged `approx` (the engine writes no per-item wire id).
- **Session.** `mcp_server/kicad_gui.py` `GuiSession` (reuses `harness/gui_e2e.py` `E2E`): free display :20..:29,
  isolated home under `<out>/mcp/gui/session-*`, pcbnew on `<out>/mcp/gui/board/<copy>`. One session per server
  process, reused while the same board copy is open and pcbnew lives; idle timeout 600 s, stopped at exit and on
  SIGTERM / SIGHUP (`kicad_gui.install_signal_stop`, called by the server's `main`), every process stopped by its
  own pid. Every X tool call is bounded (`harness.gui_e2e.X_TIMEOUT_S` 30 s, replay 600 s); a timeout is a step
  error like any other. Every step has a timeout and verifies its effect (window present, results page +
  "Completed", "Update PCB" shown, dialog gone, file sha changed) and otherwise fails loudly with a screenshot.
  `drc_spots` and `cleanup` refuse a board already changed by a cleanup (`board_state`).
- **Helper.** `harness/gui_helper/topor_gui_helper.py`, test-only: copied only into the session home's
  `scripting/plugins/` (never shipped: tools/build.sh copies no harness/, tested), active only with the session's
  socket/token env. Unix socket in the 0700 session home, 0600 token, fixed commands: `windows menu place buttons
  button checkboxes set_checkboxes list drc_state drc_rows drc_list_rect drc_selected dialog_status status
  canvas_rect zoom finest_grid ping`; no code execution. Buttons and menu items are posted wx events (a modal dialog
  never blocks the helper); menu events go to the item's own menu (KiCad's ACTION_MENU).
- **KiCad facts (10.0.8).** kipy 0.8 has no DRC or cleanup message: `run_action` opens the dialogs but cannot set,
  run or read them, and the modal Cleanup dialog blocks the API. A synthetic `wx.dataview` select/ensure-visible of
  a solder_mask_bridge row crashed pcbnew three times; a real click + Down keys never did. The DRC dialog's gear
  toggles live in pcbnew.json `"DRC"`: `report_all_track_errors` (= `--all-track-errors`), `crossprobe`,
  `scroll_on_crossprobe`. "Delete tracks connecting different nets" deletes the copper of BOTH nets around a short
  (wiccon e2e: one synthetic MOSI-SCLK short -> 4 "Remove track shorting two nets", SCLK newly unconnected).
- **Tests.** `tests/mcp_server/test_mcp_drc.py` (recorded public wiccon fixtures in `fixtures/drc/`; no KiCad), and
  `tests/mcp_server/test_mcp_drc_gui.py` (marker `gui`: wiccon hand copy + one synthetic short + one dangling stub
  (a warning the default walk skips); drc_spots over all classes and cleanup in one pcbnew).

### Borrowed code (provenance)

topor is GPL-3.0-only; only code under compatible licences is copied, with its notice kept in the file.

| Source | Commit | Files | Licence | Used |
|---|---|---|---|---|
| [mixelpixx/KiCAD-MCP-Server](https://github.com/mixelpixx/KiCAD-MCP-Server) | 1e654552f409f9f484034d31ec5ffb0ed2af6a2d | `gui_driver_plugin/plugins/listener.py`, `driver.py` | MIT | adapted into `harness/gui_helper/topor_gui_helper.py` (listener thread + `wx.CallAfter` executor, label normalisation, button lookup / posted click); MIT notice in the file |
| [lamaalrajih/kicad-mcp](https://github.com/lamaalrajih/kicad-mcp) | 98c9ea41 | DRC tools | MIT | read, nothing copied (plain kicad-cli DRC, no refill/project proof) |
| ProductOfAmerica/mcp-server-kicad | f972bfb2 | DRC tools | GPL-3.0 | read, nothing copied |
| nmlsports/kicad-mcp | 526119e8 | IPC socket lookup | MIT | read, nothing copied |
| Seeed-Studio/kicad-mcp-server | - | - | none | not read (no licence) |
