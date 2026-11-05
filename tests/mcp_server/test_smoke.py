"""End-to-end: start `python -m mcp_server` over stdio, list the tools, route the eprom copy, ask for the report.

Needs the MCP SDK on PYTHONPATH (docs/dev/mcp.md) and a topological-core engine (venv, or TOPOR_ENGINE_SITE).
Run:  pytest -m gate tests/mcp_server/test_smoke.py
"""
from __future__ import annotations

import asyncio
import json
import os
import shutil
import sys
from pathlib import Path

import pytest

pytest.importorskip("mcp")
from mcp import ClientSession, StdioServerParameters  # noqa: E402
from mcp.client.stdio import stdio_client  # noqa: E402

from _helpers import eprom_dir, main_tree  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]


def _payload(res):
    sc = getattr(res, "structured_content", None) or getattr(res, "structuredContent", None)
    if isinstance(sc, dict):
        return sc.get("result", sc) if set(sc) == {"result"} else sc
    return json.loads(res.content[0].text)


@pytest.mark.gate
def test_stdio_smoke(tmp_path):
    board = tmp_path / "eprom"
    shutil.copytree(eprom_dir(), board)
    env = dict(os.environ, TOPOR_MCP_STATE=str(tmp_path / "state"), TOPOR_VENV=str(main_tree() / ".venv"),
               PYTHONPATH=os.pathsep.join(filter(None, [str(ROOT), os.environ.get("PYTHONPATH")])))
    params = StdioServerParameters(command=sys.executable, args=["-m", "mcp_server"], env=env, cwd=str(ROOT))

    async def go():
        async with stdio_client(params) as (r, w):
            async with ClientSession(r, w) as s:
                await s.initialize()
                names = {t.name for t in (await s.list_tools()).tools}
                assert names == {"route", "report", "explain", "set_factor", "review"}
                bad = await s.call_tool("route", {"board_copy": str(Path.home() / "work/kicad/projects/x/x.kicad_pcb")})
                assert bad.is_error if hasattr(bad, "is_error") else bad.isError
                routed = _payload(await s.call_tool("route", {"board_copy": str(board / "eprom-to-kernal.kicad_pcb"),
                                                              "options": {"timeout_s": 120}}))
                rep = _payload(await s.call_tool("report", {"run_id": routed["run_id"]}))
                return routed, rep

    routed, rep = asyncio.run(go())
    assert (routed["routed"], routed["connections"]) == (27, 27)
    assert Path(routed["png"]).is_file()
    assert rep["routed"] == 27 and rep["failures_by_reason"] == []
