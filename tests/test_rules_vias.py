"""A10: toporlib.rules exports the KiCad pre-defined via list and the factory-default via warning."""

from toporlib.rules import NetClassVals, resolve_rules_from_project


def _run(pro, d=0.8, r=0.4):
    cls = {"Default": NetClassVals("Default", None, None, int(d * 1e6), int(r * 1e6))}
    return resolve_rules_from_project(pro, None, netclass_of={"": "Default"}, classes=cls, widths_before={})


def test_rules_exports_predefined_vias_and_factory_default_warning():
    """`predefined_vias` = .kicad_pro board.design_settings.via_dimensions (diameter > 0, the {0, 0} placeholder
    dropped, duplicates dropped), and a warning when the board sets no via up (factory 0.8 / 0.4 only)."""
    pro = {"board": {"design_settings": {"via_dimensions": [{"diameter": 0.0, "drill": 0.0},
                                                            {"diameter": 0.35, "drill": 0.25},
                                                            {"diameter": 0.8, "drill": 0.4},
                                                            {"diameter": 0.8, "drill": 0.4}]}},
           "net_settings": {"classes": [{"name": "Default", "via_diameter": 0.8, "via_drill": 0.4}]}}
    r = _run(pro)
    assert r["predefined_vias"] == [[350_000, 250_000], [800_000, 400_000]]
    assert not any("via sizes not set up" in w for w in r["warnings"])
    # only the placeholder + the factory class: nothing is set up
    bare = {"board": {"design_settings": {"via_dimensions": [{"diameter": 0.0, "drill": 0.0}]}},
            "net_settings": {"classes": [{"name": "Default", "via_diameter": 0.8, "via_drill": 0.4}]}}
    r = _run(bare)
    assert r["predefined_vias"] == []
    assert any("via sizes not set up" in w and "0.8/0.4" in w for w in r["warnings"]), r["warnings"]
    # a class with its own via size is a set-up board
    own = {"net_settings": {"classes": [{"name": "Default", "via_diameter": 0.6, "via_drill": 0.3}]}}
    r = _run(own, 0.6, 0.3)
    assert r["predefined_vias"] == [] and not any("via sizes not set up" in w for w in r["warnings"])
