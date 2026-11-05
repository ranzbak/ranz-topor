"""S3c MCP tools: unit tests (no SDK, no router) plus gate tests that route the eprom copy for real.

Unit tests run by default; the routing tests carry the `gate` marker:  pytest -m gate tests/mcp_server
"""
from __future__ import annotations

import json
import shutil
import tomllib
from pathlib import Path

import pytest

from _helpers import eprom_dir, main_tree
from mcp_server import tools


@pytest.fixture
def state(tmp_path, monkeypatch):
    monkeypatch.setenv("TOPOR_MCP_STATE", str(tmp_path / "state"))
    monkeypatch.setattr(tools, "_last_board", None)
    return tmp_path


@pytest.fixture
def eprom_copy(state):
    dst = state / "eprom"
    shutil.copytree(eprom_dir(), dst)
    return dst / "eprom-to-kernal.kicad_pcb"


def fake_run(state: Path, connections, warnings=(), extra=None) -> str:
    """Register a run with a hand-made result.json (shape of reports/<board>/<stamp>/result.json)."""
    board = state / "b"
    board.mkdir(exist_ok=True)
    (board / "b.kicad_pcb").write_text("")
    rep = board / "rep"
    rep.mkdir(exist_ok=True)
    nets = sorted({(c["net"], c["name"]) for c in connections})
    res = {"status": "ok", "findings": [{"code": "X", "severity": "info", "message": "m"}],
           "route": {"connections": connections, "connections_total": len(connections), "routed": 1, "failed": 2,
                     "partial": 0, "stop_reason": "converged", "timed_out": False, "warnings": list(warnings),
                     "nets": [{"net": n, "name": nm, "status": "partial"} for n, nm in nets]}}
    res.update(extra or {})
    (rep / "result.json").write_text(json.dumps(res))
    tools._remember("r1", {"run_id": "r1", "board_copy": str(board / "b.kicad_pcb"), "out_dir": str(board),
                           "report_dir": str(rep), "status": "ok", "name": "b"})
    return "r1"


CONNS = [
    {"net": 1, "name": "GND", "status": "routed", "from": {"label": "U1.1"}, "to": {"label": "U2.1"},
     "length_nm": 5_000_000, "length_outside_pads_nm": 5_000_000, "vias": 2, "crossings": 1, "neck_nm": 100},
    {"net": 1, "name": "GND", "status": "failed", "reason": "pad U3.25: no via site within 3 mm for a stub into its plane",
     "from": {"label": "U3.25"}, "to": {"label": "pour:abc"}, "blocking": {"kind": "pad", "label": "U4.1"}},
    {"net": 2, "name": "/SIG", "status": "failed", "reason": "via 1 of 2: no site in its interval (no free disc for a via)",
     "from": {"label": "R1.1"}, "to": {"label": "R2.1"}, "blocking": None},
    {"net": 2, "name": "/SIG", "status": "failed", "reason": "via 2 of 4: no site in its interval (no free disc for a via)",
     "from": {"label": "R1.2"}, "to": {"label": "R2.2"}, "blocking": None},
]


# ------------------------------------------------------------------ guard

def test_projects_path_refused(state):
    p = Path.home() / "work/kicad/projects/eprom-to-kernal/eprom-to-kernal/eprom-to-kernal.kicad_pcb"
    with pytest.raises(tools.ToolError, match="refused"):
        tools.route(str(p))
    with pytest.raises(tools.ToolError, match="refused"):
        tools.set_factor("via", 30.0, board_copy=str(p))


def test_projects_symlink_refused(state, tmp_path, monkeypatch):
    protected = tmp_path / "protected"
    protected.mkdir()
    (protected / "x.kicad_pcb").write_text("")
    link = tmp_path / "link.kicad_pcb"
    link.symlink_to(protected / "x.kicad_pcb")
    monkeypatch.setenv("TOPOR_MCP_PROTECTED", str(protected))
    with pytest.raises(tools.ToolError, match="refused"):
        tools.route(str(link))


def test_route_rejects_unknown_options(eprom_copy):
    with pytest.raises(tools.ToolError, match="unknown options"):
        tools.route(str(eprom_copy), {"bogus": 1})


# ------------------------------------------------------------------ set_factor / review

def test_set_factor_writes_valid_toml(eprom_copy):
    r = tools.set_factor("via", weight=40, report="info", board_copy=str(eprom_copy))
    tools.set_factor("necking", report="warning", board_copy=str(eprom_copy))
    data = tomllib.loads(Path(r["topor_toml"]).read_text())
    assert data["factors"]["weight"] == {"via": 40.0}
    assert data["factors"]["report"] == {"via": "info", "necking": "warning"}
    from topor.config import load_config
    cfg, findings = load_config(eprom_copy.parent)
    assert cfg.get("factors", "weight") == {"via": 40.0}
    assert not [f for f in findings if f.severity == "warning"]
    # the next route picks it up
    assert tools._toml_overrides(eprom_copy.parent)["factors.weight"] == {"via": 40.0}


def test_set_factor_validation(eprom_copy):
    for kw in ({"weight": -1}, {"weight": float("nan")}, {"report": "loud"}, {}):
        with pytest.raises(tools.ToolError):
            tools.set_factor("via", board_copy=str(eprom_copy), **kw)
    with pytest.raises(tools.ToolError):
        tools.set_factor("Bad Name", 1, board_copy=str(eprom_copy))
    assert not (eprom_copy.parent / "topor.toml").exists()


def test_review_appends_line(state):
    rid = fake_run(state, CONNS)
    a = tools.review(rid, "3", "bad", "necks next to the pad")
    b = tools.review(rid, "4", "good")
    assert (a["lines"], b["lines"]) == (1, 2)
    rows = [json.loads(x) for x in Path(b["log"]).read_text().splitlines()]
    assert [r["spot"] for r in rows] == ["3", "4"] and rows[0]["note"] == "necks next to the pad"
    assert Path(b["log"]).name == "reviews.jsonl"
    with pytest.raises(tools.ToolError):
        tools.review(rid, "5", "meh")


# ------------------------------------------------------------------ report / explain

def test_report_groups_failures_and_warnings(state):
    warns = ["net 7 skipped: no other terminal in the net", "necking: 2 stretches", "capacity: edge 4 overfull",
             "ignored_rule: 1 violation", "time budget hit at 10 s"]
    rid = fake_run(state, CONNS, warns)
    r = tools.report(rid)
    f = {x["reason"]: x for x in r["failures_by_reason"]}
    assert max(x["count"] for x in f.values()) == 2  # both "via # of #" lines collapse
    assert r["warnings_skipped_nets"] == 1
    assert set(r["warnings"]) >= {"necking", "capacity", "ignored_rule", "time_budget"}
    assert r["cuts"] == ["capacity: edge 4 overfull"]
    with pytest.raises(tools.ToolError, match="no variant table"):
        tools.report(rid, variant="a")


def test_report_variant_table(state):
    var = [{"id": "a", "name": "short", "route": {"connections": CONNS[:1], "warnings": ["necking: x"]}},
           {"id": "b", "name": "few-vias"}]
    rid = fake_run(state, CONNS, extra={"variants": var})
    assert tools.report(rid, "a")["warnings"] == {"necking": ["necking: x"]}
    assert tools.report(rid)["failed"] == 2
    with pytest.raises(tools.ToolError, match="unknown variant"):
        tools.report(rid, "zzz")


def test_explain(state):
    rid = fake_run(state, CONNS)
    e = tools.explain(rid, "GND")
    assert e["why_unrouted"][0]["blocking"] == {"kind": "pad", "label": "U4.1"}
    assert "no via site" in e["why_unrouted"][0]["reason"]
    top = e["connections"][0]["factors"]
    assert [x["factor"] for x in top][0] == "via" and top[0]["cost"] == 50.0
    assert tools.explain(rid, "2")["net"] == "/SIG"
    with pytest.raises(tools.ToolError, match="no net"):
        tools.explain(rid, "NOPE")


def test_unknown_run(state):
    with pytest.raises(tools.ToolError, match="unknown run_id"):
        tools.report("nope")


# ------------------------------------------------------------------ real routing (gate)

@pytest.mark.gate
def test_route_eprom_summary(eprom_copy, monkeypatch):
    monkeypatch.setenv("TOPOR_VENV", str(main_tree() / ".venv"))
    tools.set_factor("necking", report="warning", board_copy=str(eprom_copy))  # exercises the toml -> --set path
    r = tools.route(str(eprom_copy), {"timeout_s": 120})
    assert (r["routed"], r["connections"]) == (27, 27)
    assert r["png"] and Path(r["png"]).is_file()
    assert r["unconnected"] == 0 and "factors.report" in r["settings_from_topor_toml"]
    rep = tools.report(r["run_id"])
    assert rep["routed"] == 27 and rep["failures_by_reason"] == []
    assert [p.name for p in eprom_copy.parent.glob("*.kicad_pcb")] == [eprom_copy.name]
