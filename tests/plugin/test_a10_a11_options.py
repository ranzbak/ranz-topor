"""A10: the plugin turns [vias] settings into engine options (units, tables). The A10 / A11 via-cost and
layer-direction settings were removed in the CL cleanup (the topological core never read them)."""

from __future__ import annotations

import json

from topor import run as run_mod
from topor.config import Config


class _Engine:
    """default_options of an engine that knows the A10 fields."""

    @staticmethod
    def default_options():
        return json.dumps({"allowed_drills_nm": [], "via_in_pad": False})


def _model():
    return {"nets": [{"code": 1, "name": "GND"}, {"code": 2, "name": "SIG"}, {"code": 3, "name": "HS"},
                     {"code": 4, "name": "PLAIN"}],
            "rules": {"per_net": {"1": {"netclass": "Power,Default"}, "2": {"netclass": "Default"},
                                  "3": {"netclass": "Default"}, "4": {"netclass": "Default"}}}}


def test_build_options_maps_vias_settings():
    cfg = Config.defaults().with_values({("vias", "allowed_drills_mm"): [0.3, 0.4], ("vias", "via_in_pad"): True})
    o = run_mod.build_options(cfg, _Engine, model=_model(), priorities={"GND": 2})
    assert o["allowed_drills_nm"] == [300_000, 400_000] and o["via_in_pad"] is True


def test_defaults_pass_no_removed_options():
    o = run_mod.build_options(Config.defaults(), _Engine, model=_model(), priorities={})
    assert "layer_direction" not in o and "via_cost_by_net" not in o and "threads" not in o
    assert o["allowed_drills_nm"] == [] and o["via_in_pad"] is False
