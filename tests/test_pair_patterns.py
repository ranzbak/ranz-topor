"""Via-pair pattern (harness/pair_patterns.py, shopping list 5.8; NEEDS USER APPROVAL): hand numbers hold, KiCad DRC passes."""
import pytest

from harness import pair_patterns as pp
from harness import via_patterns as vp
from harness.paths import kicad_cli


def test_numbers():
    p = vp.build("pair_move")
    assert len(p.ref.vias) == 0 and len(p.careless.vias) == 2
    e = p.expected
    assert e["reference"]["price"] < e["careless"]["price"]
    assert e["built"]["reference"]["edge_ok"]
    assert pp.YA_BOT > pp.Y_B2 > pp.Y_B1 > pp.YA_TOP


@pytest.mark.skipif(not kicad_cli().exists(), reason="no kicad-cli")
def test_kicad_drc(tmp_path):
    g = vp.generate("pair_move", tmp_path, png=False, drc=False)
    # valid test (harness.validity): the expected answer is built for the board's own rules, route with
    # [rules] fab_profile = "board" and the via rule the via patterns assume
    import json
    assert json.loads(g["expected"].read_text())["rules_assumed"] == vp.RULES_ASSUMED
    assert vp.drc_of(g["problem"])["violations"] == 0
    for k in ("reference", "careless"):
        r = vp.drc_of(g[k])
        assert r["violations"] == 0 and r["unconnected"] == 0
