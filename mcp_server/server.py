"""MCP stdio server: registers mcp_server.tools with the MCP Python SDK (v1 FastMCP or v2 MCPServer).

Start:  PYTHONPATH=<repo>:<repo>/plugin python -m mcp_server      (the SDK must be importable; see docs/dev/mcp.md)
stdout carries the protocol, so nothing else may print to it; the router runs in a subprocess.
"""
from __future__ import annotations

from . import tools


def build_server():
    try:  # mcp 2.x
        from mcp.server.mcpserver import MCPServer as _Server
    except ImportError:  # mcp 1.x
        from mcp.server.fastmcp import FastMCP as _Server
    srv = _Server("topor")

    @srv.tool()
    def route(board_copy: str, options: dict | None = None) -> dict:
        """Route a COPY of a KiCad board (.kicad_pcb) with topor, headless. Refuses paths under ~/work/kicad/projects.
        Reads topor.toml next to the board. options: timeout_s, png (default true), set {"section.key": value}.
        Returns routed/total, DRC unconnected, routing_new, vias, detour, runtime, run_id, png and the variant table if any."""
        return tools.route(board_copy, options)

    @srv.tool()
    def report(run_id: str, variant: str | None = None) -> dict:
        """Warnings (necking, ignored_rule, time budget, ...), cuts and failures grouped by reason for a run."""
        return tools.report(run_id, variant)

    @srv.tool()
    def explain(run_id: str, net: str) -> dict:
        """Why a net is unrouted (reason, what blocks it) and the factor events on its wires. net: name or number."""
        return tools.explain(run_id, net)

    @srv.tool()
    def set_factor(name: str, weight: float | None = None, report: str | None = None,
                   board_copy: str | None = None) -> dict:
        """Write a topor.toml [factors] weight (mm per unit, >= 0) and/or report level (off|info|warning) next to the
        board copy (default: the last one routed in this session). Rerun `route` afterwards."""
        return tools.set_factor(name, weight, report, board_copy)

    @srv.tool()
    def review(run_id: str, spot: str, verdict: str, note: str = "") -> dict:
        """Record a review verdict (good|bad|neutral) for a spot of a run in reviews.jsonl next to the board copy."""
        return tools.review(run_id, spot, verdict, note)

    return srv


def main() -> None:
    build_server().run("stdio") if _is_v2() else build_server().run()


def _is_v2() -> bool:
    try:
        import mcp.server.mcpserver  # noqa: F401
        return True
    except ImportError:
        return False
