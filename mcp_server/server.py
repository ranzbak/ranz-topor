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

    @srv.tool()
    def register_run(routed_copy: str, input_copy: str, result: str | None = None, name: str | None = None) -> dict:
        """Register a board routed elsewhere (e.g. in real KiCad) for drc/drc_spots/cleanup/compare: the routed
        .kicad_pcb copy, the unrouted input copy it came from, optionally the plugin's result.json."""
        return tools.register_run(routed_copy, input_copy, result, name)

    @srv.tool()
    def drc(run_id: str, limit: int = 200) -> dict:
        """kicad-cli DRC of the run's routed copy with the board's .kicad_pro provably used (sha before/after +
        quoted clearances), as a DELTA vs the unrouted input: one row per new violation touching our copper (type,
        class, nets, layers, position mm, item uuids, our track/via uuids with their engine connection ids, nearest
        engine violation), counts per class, unconnected per net. Fails (RULES_MISMATCH / DRC_WITHOUT_PROJECT)
        instead of returning numbers under the wrong rules."""
        return tools.drc(run_id, limit)

    @srv.tool()
    def drc_spots(run_id: str, cls: str | None = None, per_class: int = 0, zoom: float = 20.0,
                  keep_session: bool = True) -> dict:
        """Real KiCad (Xephyr, isolated home): DRC dialog -> Run DRC -> per drc row (cls = one class, e.g.
        "clearance track-track"; per_class > 0 caps a class; default all rows) select the dialog row with the same
        text in ONE KiCad session (DRC run once), screenshot the dialog and the board view at the marker (yellow
        ring). Records: PNG paths, GUI vs kicad-cli text, match (row found + selected), KiCad's focus point; a failed
        row is reported and the walk goes on. The session stays open for cleanup (idle timeout 600 s)."""
        return tools.drc_spots(run_id, cls, per_class, zoom, keep_session)

    @srv.tool()
    def cleanup(run_id: str, options: list | dict | None = None, replay: bool = False,
                keep_session: bool = False) -> dict:
        """Real KiCad Tools -> Cleanup Tracks & Vias on a copy of the routed board. options = KiCad's box labels
        (list = these on), e.g. ["Delete tracks connecting different nets", "Delete redundant vias",
        "Merge co-linear tracks", "Delete tracks unconnected at one end"] (the default). Returns removed items,
        KiCad's change list by text, newly unconnected connections per net with engine connection ids, and the
        DRC delta per class (before -> after). replay = also harness.cleanup_check's replay as a cross-check."""
        return tools.cleanup(run_id, options, replay, keep_session)

    @srv.tool()
    def compare(run_a: str, run_b: str) -> dict:
        """Per-class delta of our new DRC violations between two runs of the same board (builds / settings)."""
        return tools.compare(run_a, run_b)

    @srv.tool()
    def gui_session(action: str = "status") -> dict:
        """The server's real-KiCad session: status (display, board, pid, idle) or stop."""
        return tools.gui_session(action)

    return srv


def main() -> None:
    from . import kicad_gui
    kicad_gui.install_signal_stop()  # a SIGTERM'd server stops its KiCad session (no orphaned Xephyr / pcbnew)
    build_server().run("stdio") if _is_v2() else build_server().run()


def _is_v2() -> bool:
    try:
        import mcp.server.mcpserver  # noqa: F401
        return True
    except ImportError:
        return False
