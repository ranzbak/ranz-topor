"""A10 / A11: the plugin turns [vias] / [layers] settings into engine options (units, precedence, tables)."""

from __future__ import annotations

import json

from topor import run as run_mod
from topor.config import Config


class _Engine:
    """default_options of an engine that knows the A10 / A11 fields."""

    @staticmethod
    def default_options():
        return json.dumps({"via_cost_nm": 2_000_000, "via_cost_by_net": {}, "layer_direction": {},
                           "direction_cost": 0.0, "pour_cut_cost_nm": 0, "allowed_drills_nm": [], "via_in_pad": False,
                           "reduction_len_slack_pct": 10.0})


def _model():
    return {"nets": [{"code": 1, "name": "GND"}, {"code": 2, "name": "SIG"}, {"code": 3, "name": "HS"},
                     {"code": 4, "name": "PLAIN"}],
            "rules": {"per_net": {"1": {"netclass": "Power,Default"}, "2": {"netclass": "Default"},
                                  "3": {"netclass": "Default"}, "4": {"netclass": "Default"}}}}


def test_via_costs_by_net_precedence_netclass_over_priority_over_global():
    cfg = Config.defaults().with_values({("vias", "cost_by_netclass"): {"Power": 0.5},
                                         ("vias", "cost_by_priority"): {"1": 5, "2": 3}})
    prios = {"GND": 1, "SIG": 2, "HS": 1}
    out = run_mod.via_costs_by_net(cfg, _model(), prios)
    # GND: netclass Power (0.5) beats its priority 1 (5); SIG / HS: by priority; PLAIN: global (absent)
    assert out == {"GND": 500_000, "SIG": 3_000_000, "HS": 5_000_000}
    assert run_mod.via_costs_by_net(Config.defaults(), _model(), prios) == {}


def test_build_options_maps_vias_and_layer_settings():
    cfg = Config.defaults().with_values({
        ("vias", "allowed_drills_mm"): [0.3, 0.4], ("vias", "via_in_pad"): True,
        ("vias", "reduction_len_slack_pct"): 25.0, ("vias", "cost_by_netclass"): {"Power": 0.5},
        ("layers", "preferred_direction"): {"F.Cu": "h", "B.Cu": "V", "In1.Cu": "none", "In2.Cu": "diag"},
        ("layers", "direction_cost"): 0.7, ("layers", "pour_cut_cost_mm"): 0.25})
    o = run_mod.build_options(cfg, _Engine, model=_model(), priorities={"GND": 2})
    assert o["allowed_drills_nm"] == [300_000, 400_000] and o["via_in_pad"] is True
    assert o["reduction_len_slack_pct"] == 25.0
    assert o["pour_cut_cost_nm"] == 250_000 and o["direction_cost"] == 0.7
    assert o["layer_direction"] == {"F.Cu": 0.0, "B.Cu": 90.0}  # "none" / unknown values are not passed
    assert o["via_cost_by_net"] == {"GND": 500_000}


def test_defaults_pass_no_a10_a11_extras():
    o = run_mod.build_options(Config.defaults(), _Engine, model=_model(), priorities={})
    assert "layer_direction" not in o and "via_cost_by_net" not in o
    assert o["pour_cut_cost_nm"] == 0 and o["allowed_drills_nm"] == [] and o["via_in_pad"] is False
