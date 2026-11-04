"""harness.strip: router-input preparation (tracks, vias and teardrop zones out; zones and the rest stay)."""
from pathlib import Path

from harness import strip
from harness.route_board import _kv, summary

BOARD = """(kicad_pcb
\t(version 20260206)
\t(footprint "R"
\t\t(uuid "00000000-0000-4000-8000-000000000001")
\t\t(via_like_name "kept")
\t)
\t(segment
\t\t(start 1 1)
\t\t(end 2 2)
\t\t(width 0.2)
\t\t(layer "F.Cu")
\t\t(net 1)
\t\t(uuid "00000000-0000-4000-8000-000000000002")
\t)
\t(arc
\t\t(start 1 1)
\t\t(mid 2 2)
\t\t(end 3 1)
\t\t(width 0.2)
\t\t(layer "F.Cu")
\t\t(net 1)
\t\t(uuid "00000000-0000-4000-8000-000000000003")
\t)
\t(via
\t\t(at 5 5)
\t\t(size 0.6)
\t\t(drill 0.3)
\t\t(layers "F.Cu" "B.Cu")
\t\t(net 1)
\t\t(uuid "00000000-0000-4000-8000-000000000004")
\t)
\t(zone
\t\t(net 1)
\t\t(layer "F.Cu")
\t\t(attr
\t\t\t(teardrop
\t\t\t\t(type padvia)
\t\t\t)
\t\t)
\t\t(uuid "00000000-0000-4000-8000-000000000005")
\t)
\t(zone
\t\t(net 2)
\t\t(layer "In1.Cu")
\t\t(uuid "00000000-0000-4000-8000-000000000006")
\t)
)
"""


def test_strip_text_removes_copper_and_teardrops_only():
    out, rep = strip.strip_text(BOARD)
    assert (rep.tracks, rep.arcs, rep.vias, rep.teardrop_zones) == (1, 1, 1, 1)
    assert "(segment" not in out and "(arc" not in out and "(via\n" not in out
    assert "type padvia" not in out
    assert '(layer "In1.Cu")' in out and "(footprint" in out  # the pour and the footprint stay
    assert out.count("(zone") == 1
    assert out.rstrip().endswith(")")
    again, rep2 = strip.strip_text(out)  # idempotent
    assert again == out and (rep2.tracks, rep2.arcs, rep2.vias, rep2.teardrop_zones) == (0, 0, 0, 0)


def test_prepare_copies_and_leaves_source_alone(tmp_path: Path):
    src = tmp_path / "proj"
    src.mkdir()
    pcb = src / "b.kicad_pcb"
    pcb.write_text(BOARD)
    (src / "b.kicad_pro").write_text("{}")
    before = pcb.read_text()
    hand, inp, rep = strip.prepare(str(pcb), tmp_path / "out")
    assert pcb.read_text() == before
    assert hand.pcb.read_text() == before  # the hand reference is untouched
    assert "(segment" not in inp.pcb.read_text()
    assert (inp.pcb.parent / "b.kicad_pro").is_file()
    assert rep.vias == 1


def test_prepare_whole_project_keeps_other_files(tmp_path: Path):
    src = tmp_path / "proj"
    (src / "library").mkdir(parents=True)
    (src / "library" / "x.txt").write_text("x")
    (src / "b.kicad_pcb").write_text(BOARD)
    hand, inp, _ = strip.prepare(str(src / "b.kicad_pcb"), tmp_path / "out", whole_project=True, name="nm")
    assert (inp.pcb.parent / "library" / "x.txt").is_file() and inp.pcb.parent.name == "nm-input"
    assert hand.pcb.parent.name == "nm-hand"


def test_cli_helpers():
    assert _kv("run.timeout_s=30") == (("run", "timeout_s"), 30)
    assert _kv("run.existing_copper=rip") == (("run", "existing_copper"), "rip")
    m = {"board": "x", "status": "rejected", "engine_version": "0.4.0", "routed": 0, "connections": 5,
         "routed_pct": 0.0, "unconnected": 5, "unconnected_hand": 0, "unconnected_input": 5, "routing_new": 0,
         "vias": 0, "vias_hand": 3, "detour": None, "ladder_warnings": None, "runtime_s": 1.0,
         "run_flow_s": 0.5, "router_s": 0.0, "stop_reason": "no engine", "findings": [], "png": None}
    assert "routed 0/5 (0.0 %)" in summary(m)
