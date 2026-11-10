"""Via patterns (harness/via_patterns.py): hand numbers hold (build() runs check()), KiCad DRC passes."""
import json

import pytest

from harness import via_patterns as vp
from harness.paths import kicad_cli


@pytest.fixture(scope="module")
def generated(tmp_path_factory):
    out = tmp_path_factory.mktemp("vp")
    return {n: vp.generate(n, out, png=False, drc=False) for n in vp.NAMES}


@pytest.mark.parametrize("name", vp.NAMES)
def test_checks(name):
    pat = vp.build(name)
    assert pat.expected["built"]["reference"]["edge_ok"]
    assert len(pat.ref.vias) <= len(pat.careless.vias)


def test_numbers():
    assert len(vp.build("via_pair_redundant").ref.vias) == 0
    assert len(vp.build("via_pair_slide").ref.vias) == 2 and len(vp.build("via_pair_slide").careless.vias) == 4
    f = vp.build("via_force_relocate").expected
    assert f["min_gap_mm"]["reference"] > 1.0 > f["min_gap_mm"]["careless"]
    b = vp.build("branch_point_relocate")
    assert b.expected["reference"]["length_mm"] < b.expected["careless"]["length_mm"]
    s = vp.build("via_size_rule").expected["via_sizes"]
    assert (s["SIG"]["dia"], s["SIG"]["drill"]) == (0.45, 0.25) and s["PWR"]["drill"] == 0.32 and s["HEAVY"]["drill"] == 0.64
    assert all(v["pi_drill"] >= v["width"] for v in s.values())


def test_project_values(generated):
    pro = json.loads(generated["via_size_rule"]["reference"].with_suffix(".kicad_pro").read_text())
    d = next(c for c in pro["net_settings"]["classes"] if c["name"] == "Default")
    assert d["via_diameter"] == 0.8 and d["via_drill"] == 0.4
    assert pro["board"]["design_settings"]["rules"]["min_via_diameter"] == 0.45


@pytest.mark.skipif(not kicad_cli().exists(), reason="no kicad-cli")
@pytest.mark.parametrize("name", vp.NAMES)
def test_kicad_drc(name, generated):
    g = generated[name]
    assert vp.drc_of(g["problem"])["violations"] == 0
    for k in ("reference", "careless"):
        r = vp.drc_of(g[k])
        assert r["violations"] == 0 and r["unconnected"] == 0
