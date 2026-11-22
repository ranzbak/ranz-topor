"""thick-layer F4 ([output] plan_report): the engine's plan of the drawn row (``route.plan``) is written to
``plan.json.gz`` in the report folder, once per plan, and left out of ``result.json``; ``toporlib.plandump`` reads and
summarises it; the schema key maps to the engine option."""
import gzip
import json

from topor import schema
from topor.config import Config
from topor.report import RunReport
from toporlib import plandump

PLAN = {
    "format": "topor-plan", "version": 1,
    "row": {"label": "run 14 cycle 9 (gen 8)", "roles": ["shortest"], "round": 9, "seed": 7, "loop": "topor"},
    "layers": ["F.Cu", "B.Cu"], "predicted": {"via_site": 1}, "predicted_violations": 1, "foreign": 0,
    "conns": {"405": {"vias": 2, "taut_mm": 38.65, "necks": 0, "predicted": {"via_site": 1}}},
    "wires": [
        {"wire": 3, "net": 12, "net_name": "VIN5V", "conn": 405, "w": 1.0, "s": 0.15, "a": [0, 0], "b": [10, 0],
         "layers_now": [1, 1, 2, 2, 1], "nosite": 1,
         "vias": [{"iv": [1, 2], "k": 2, "at": [4.0, 0.0], "site": True, "deficit": 0.0},
                  {"iv": [3, 4], "k": 4, "at": [8.0, 0.0], "site": False, "deficit": 0.124}],
         "crossings": [{"k": 3, "other": 5, "other_k": 1, "other_net": 30, "layers": [1], "other_layers": [0]},
                       {"k": 0, "other": 6, "other_k": 2, "other_net": 31, "layers": [0], "other_layers": [1]}]},
        {"wire": 5, "net": 30, "net_name": "SD_CLK", "conn": 187, "w": 0.2, "s": 0.15, "a": [5, -5], "b": [5, 5],
         "layers_now": [1, 1], "nosite": 0, "vias": [], "crossings": []},
    ],
}


def _result(plan):
    return {"status": "accepted", "route": {"routed": 2, "plan": plan, "compass": {"round": 9}}}


def test_the_plan_goes_to_plan_json_gz_once_and_not_into_result_json(tmp_path):
    text = json.dumps(PLAN)
    rep = RunReport(tmp_path)
    rep.write(result=_result(text))
    res = json.loads((tmp_path / "result.json").read_text())
    assert "plan" not in res["route"] and res["route"]["compass"] == {"round": 9}
    assert res["plan_file"]["file"] == "plan.json.gz" and res["plan_file"]["bytes"] > 0
    assert "run 14 cycle 9 (gen 8)" in res["plan_file"]["row"] and "seed 7" in res["plan_file"]["row"]
    with gzip.open(tmp_path / "plan.json.gz", "rt") as f:
        assert json.loads(f.read()) == PLAN
    md = (tmp_path / "report.md").read_text()
    assert "plan.json.gz: the plan handed to the drawing of the drawn row (plan of run 14 cycle 9 (gen 8) [shortest]" in md
    # a rewrite with the same plan text does not write the file again; a new plan does
    (tmp_path / "plan.json.gz").write_bytes(b"sentinel")
    rep.write(result=_result(text))
    assert (tmp_path / "plan.json.gz").read_bytes() == b"sentinel"
    other = json.dumps({**PLAN, "row": {**PLAN["row"], "label": "run 2 cycle 1 (gen 1)"}})
    rep.write(result=_result(other))
    assert plandump.load(tmp_path)["row"]["label"] == "run 2 cycle 1 (gen 1)"
    # the caller's dict is not changed (the flow rewrites from the same engine result)
    r = _result(text)
    rep.write(result=r)
    assert r["route"]["plan"] == text


def test_without_a_plan_nothing_is_written_and_a_bad_plan_is_said_not_raised(tmp_path):
    rep = RunReport(tmp_path)
    rep.write(result={"status": "error", "route": {"routed": 0}})
    assert not (tmp_path / "plan.json.gz").exists()
    assert "plan_file" not in json.loads((tmp_path / "result.json").read_text())
    assert "plan.json.gz" not in (tmp_path / "report.md").read_text()
    rep.write(result=_result("{not json"))
    res = json.loads((tmp_path / "result.json").read_text())
    assert res["plan_file"]["file"] is None and res["plan_file"]["error"].startswith("JSONDecodeError")
    assert "plan.json.gz" not in (tmp_path / "report.md").read_text()


def test_plandump_reads_selects_and_summarises(tmp_path, capsys):
    plandump.write(tmp_path, PLAN)
    p = plandump.load(tmp_path / "plan.json.gz")
    assert plandump.row_line(p).startswith("plan of run 14 cycle 9 (gen 8) [shortest], round 9, seed 7, loop topor; 2 wires")
    assert [w["wire"] for w in plandump.select(p, conns=[405])] == [3]
    assert [w["wire"] for w in plandump.select(p, nets=["SD_CLK"])] == [5]
    assert len(plandump.select(p)) == 2
    lines = plandump.wire_lines(p, p["wires"][0])
    text = "\n".join(lines)
    assert "layers along it: F.Cu | B.Cu | F.Cu" in text
    assert "via at [8.0, 0.0] (traversal 4 in [3, 4]): NO SITE, static room short 0.124 mm" in text
    assert "via at [4.0, 0.0] (traversal 2 in [1, 2]): site" in text
    # crossings sorted along the wire (k), with both layers
    assert "net 31 F.Cu/B.Cu, net 30 B.Cu/F.Cu" in text
    assert "planned vias 2, taut 38.65 mm, necks 0, predicted: via_site 1" in text
    # a plain JSON file reads too; a foreign JSON is refused
    (tmp_path / "x.json").write_text(json.dumps(PLAN))
    assert plandump.load(tmp_path / "x.json")["version"] == 1
    (tmp_path / "y.json").write_text("{}")
    try:
        plandump.load(tmp_path / "y.json")
        raise AssertionError("not refused")
    except ValueError as e:
        assert "not a topor plan" in str(e)
    assert plandump.main([str(tmp_path), "--net", "VIN5V"]) == 0
    out = capsys.readouterr().out
    assert "wire 3 net VIN5V conn 405" in out and "SD_CLK" not in out


def test_plan_report_setting_maps_to_the_engine_and_is_on_by_default():
    s = schema.SETTINGS_BY_KEY[("output", "plan_report")]
    assert s.default is True and s.engine_field == "plan_report" and schema.available(s)
    assert Config.defaults().engine_options()["plan_report"] is True
    off = Config.defaults().with_values({("output", "plan_report"): False})
    assert off.engine_options()["plan_report"] is False
