"""TW0: result.json carries the engine profile with the plugin's python split (real engine result, `profile` patched in)."""
import json

import pytest

pytest.importorskip("wx")
pytest.importorskip("topor_engine")

from test_run_flow import CFG, ProxyEngine, app, do_run, eprom, reports  # noqa: E402,F401
from topor.fake import ScriptedUi  # noqa: E402


def test_run_writes_the_profile_into_result_json(app, eprom, tmp_path, reports):
    """A engine result with `profile` (TOPO_PROF=1) ends in result.json with the python split; without one the key is null."""
    import json

    def patch(out_json):
        d = json.loads(out_json)
        d["profile"] = {"version": 1, "engine_s": 0.0, "stages": [], "cycles": []}
        return json.dumps(d)

    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(), cfg=CFG, engine=ProxyEngine(hold=0, patch=patch))
    res = json.loads((out.report_dir / "result.json").read_text())
    py = res["profile"]["python"]
    assert set(py) == {"before_s", "job_s", "after_s", "total_s"} and py["before_s"] >= 0 and py["job_s"] > 0
    assert abs(py["before_s"] + py["job_s"] + py["after_s"] - py["total_s"]) < 0.01
    assert "profile" not in res["route"], "the profile is kept once, at the top level"
    _, _, plain = do_run(eprom, tmp_path / "b", ui=ScriptedUi(), cfg=CFG, engine=ProxyEngine(hold=0))
    assert json.loads((plain.report_dir / "result.json").read_text())["profile"] is None
