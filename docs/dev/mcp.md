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
