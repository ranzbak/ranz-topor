"""A10 / FX2 concern: the pre-flight's via rule (``preflight.smaller_profile_vias``) and the engine's
(``engine/src/vias.rs`` ``via_sizes``, called through ``topor_engine.via_sizes``) agree on the six snapshot projects:
same rules in, same via size list out, with and without the plugin's via fallback and an allowed-drill filter."""

from __future__ import annotations

import json

import pytest

from topor import preflight as pf
from topor import run as run_mod
from topor.config import Config
from toporlib.rules import classes_from_pro, resolve_rules_from_project

topor_engine = pytest.importorskip("topor_engine")
if not hasattr(topor_engine, "via_sizes"):
    pytest.skip("topor_engine.via_sizes not in the installed engine (rebuild)", allow_module_level=True)

BOARDS = ["bornhack2026", "dcsgonebadge", "eprom-to-kernal", "nametag-sao", "wedding-gift", "wiccon-2023"]


def _rules(board):
    from harness.paths import board_pcb

    pro = json.loads(board_pcb(board).with_suffix(".kicad_pro").read_text())
    classes = classes_from_pro(pro)
    names = {"": "Default", **{f"NET_{c}": c for c in classes if c != "Default"}}
    codes = {"": 0, **{n: i for i, n in enumerate(sorted(n for n in names if n), start=1)}}
    return pro, resolve_rules_from_project(pro, None, netclass_of=names, classes=classes, widths_before={},
                                           net_codes=codes)


def _python_sizes(rules, net_rule, allowed_nm):
    d, r = int(net_rule["via_d"]), int(net_rule["via_drill"])
    return [[d, r]] + [list(s) for s in pf.smaller_profile_vias(rules, d, r, allowed_nm)]


@pytest.mark.parametrize("allowed_mm", [[], [0.25, 0.3, 0.4]])
@pytest.mark.parametrize("fallback", [False, True])
@pytest.mark.parametrize("board", BOARDS)
def test_engine_and_preflight_via_rule_agree(board, fallback, allowed_mm):
    pro, rules = _rules(board)
    if fallback:
        cfg = Config.defaults().with_values({("vias", "fallback_diameter_mm"): 0.7, ("vias", "fallback_drill_mm"): 0.35})
        if pf.via_setup_missing(pro):
            rules, _ = run_mod.apply_via_fallback(rules, cfg)
    allowed_nm = [int(round(x * 1e6)) for x in allowed_mm]
    opts = json.dumps({"allowed_drills_nm": allowed_nm})
    nets = {"0": rules["default"], **rules["per_net"]}
    assert len(nets) >= 1
    for code, nr in nets.items():
        got = json.loads(topor_engine.via_sizes(json.dumps(rules), int(code), opts))
        want = _python_sizes(rules, nr, allowed_nm)
        # a net without a per-net rule uses the default rule (net 0 here)
        assert got == want, (board, code, got, want)


def test_engine_via_fallback_agrees_with_plugin_patch():
    """A board that sets nothing up: the engine's own fallback (rules.via_fallback, factory class, no pre-defined
    via) gives the same first size as the plugin's pre-patched NetRule."""
    pro, rules = _rules("wiccon-2023")
    assert pf.via_setup_missing(pro)
    cfg = Config.defaults().with_values({("vias", "fallback_diameter_mm"): 0.7, ("vias", "fallback_drill_mm"): 0.35})
    patched, info = run_mod.apply_via_fallback(rules, cfg)
    unpatched = {**rules, "via_fallback": patched["via_fallback"]}
    a = json.loads(topor_engine.via_sizes(json.dumps(patched), 0, "{}"))
    b = json.loads(topor_engine.via_sizes(json.dumps(unpatched), 0, "{}"))
    assert a[0] == b[0] == [info["diameter_nm"], info["drill_nm"]]
    assert a == b
