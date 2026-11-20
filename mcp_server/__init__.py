"""Topor MCP server (S3c): a thin stdio wrapper around the headless run flow and the report JSON.

`mcp_server.tools` holds the five tools as plain functions (no MCP SDK needed); `mcp_server.server`
registers them with the SDK. See docs/dev/mcp.md for the manual equivalent of every tool.
"""
import sys
from pathlib import Path

_REPO = Path(__file__).resolve().parent.parent
for _p in (_REPO / "plugin", _REPO):  # `topor` (plugin/) and `harness` import before any tool module (verify r6 N4)
    if str(_p) not in sys.path:
        sys.path.insert(0, str(_p))
