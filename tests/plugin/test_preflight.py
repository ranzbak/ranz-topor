"""B6: pre-flight checks, one test per finding code, on small synthetic models."""

from __future__ import annotations

import copy

from topor import preflight as pf
from topor.config import Config
from topor.ports import Selection
from toporlib.drc import DrcReport

MM = 1_000_000


def _layer(i, name, role, kind="signal", src="default"):
    return {"id": i, "name": name, "kind": kind, "role": role, "role_source": src,
            "routable": role in ("signal", "mixed")}


def _pad(ref, num, net, x, y, uuid=None):
    return {"uuid": uuid or f"{ref}.{num}", "ref": ref, "number": num, "net": net, "attr": "smd",
            "center": [x, y], "rotation_deg": 0.0, "layers": [0],
            "shapes": {"0": {"kind": "rect", "w": 600_000, "h": 600_000}}, "drill": None}


def base_model(**over):
    m = {
        "units": "nm", "board": "t",
        "layers": [_layer(0, "F.Cu", "signal"), _layer(4, "In1.Cu", "gnd"), _layer(6, "In2.Cu", "pwr"),
                   _layer(2, "B.Cu", "signal")],
        "nets": [{"code": 1, "name": "GND"}, {"code": 2, "name": "+3V3"}, {"code": 3, "name": "SIG"}],
        "pads": [_pad("U1", "1", 3, 10 * MM, 10 * MM), _pad("U2", "1", 3, 20 * MM, 10 * MM)],
        "vias": [], "kept": [], "copper_shapes": [], "holes": [],
        "outline": [{"kind": "seg", "a": [0, 0], "b": [40 * MM, 0]},
                    {"kind": "seg", "a": [40 * MM, 0], "b": [40 * MM, 30 * MM]},
                    {"kind": "seg", "a": [40 * MM, 30 * MM], "b": [0, 30 * MM]},
                    {"kind": "seg", "a": [0, 30 * MM], "b": [0, 0]}],
        "keepouts": [], "net_ties": [], "pours": [], "dangling_ends": [], "rules": {},
    }
    m.update(over)
    return m


def pour(layer, net):
    return {"uuid": f"z{layer}{net}", "net": net, "layers": [layer], "pads": [], "vias": [],
            "fills": [{"layer": layer, "outer": [[0, 0], [1, 0], [1, 1]], "holes": []}]}


def rules(**over):
    r = {"default": {"via_d": 600_000, "via_drill": 300_000}, "per_net": {}, "warnings": []}
    r.update(over)
    return r


PCB_OK = '(kicad_pcb (version 20260206) (generator "pcbnew") (layers (0 "F.Cu" signal) (2 "B.Cu" signal)) (setup))'
PRO_VIAS = {"board": {"design_settings": {"via_sizes_list": [{"diameter": 0.6, "drill": 0.3}]}}}


def run(model=None, *, rl=None, cfg=None, sel=None, drc=None, cfg_findings=(), pcb=PCB_OK, pro=PRO_VIAS):
    return pf.preflight(model or base_model(), rl or rules(), cfg or Config.defaults(), sel or Selection(), drc,
                        list(cfg_findings), pcb_text=pcb, pro=pro)


def codes(fs):
    return [f.code for f in fs]


def one(fs, code):
    got = [f for f in fs if f.code == code]
    assert len(got) == 1, codes(fs)
    return got[0]


def test_clean_model_has_only_info():
    fs = run(base_model(pours=[pour(4, 1), pour(6, 2)]))
    assert not [f for f in fs if f.severity in ("warning", "error")], fs


def test_cfg_findings_pass_through():
    from topor.findings import Finding
    f = Finding("CFG_CREATED", "info", "x", "y")
    assert f in run(cfg_findings=[f])


# ---- M4-R7 plane check
def test_plane_no_zone_gnd_exact_fix():
    f = one(run(base_model(pours=[pour(6, 2)])), "PLANE_NO_ZONE")
    assert f.severity == "warning" and "In1.Cu" in f.message and "ground" in f.message
    assert f.fix.startswith("add a zone on In1.Cu with net GND covering the board outline "
                            "(Place → Add Filled Zone), then refill zones (B)")
    assert "topor.toml [layers]" in f.fix
    assert f.items == ("In1.Cu",)


def test_plane_with_ground_zone_no_finding():
    assert "PLANE_NO_ZONE" not in codes(run(base_model(pours=[pour(4, 1), pour(6, 2)])))


def test_plane_pwr_with_gnd_zone_is_wrong_kind():
    fs = run(base_model(pours=[pour(4, 1), pour(6, 1)]))
    f = one(fs, "PLANE_NO_ZONE")
    assert f.items == ("In2.Cu",) and "power" in f.message
    assert f.fix.startswith("add a zone on In2.Cu with net +3V3 covering the board outline "
                            "(Place → Add Filled Zone), then refill zones (B)")


def test_plane_kicad_power_accepts_either_kind():
    m = base_model(layers=[_layer(0, "F.Cu", "signal"), _layer(4, "In1.Cu", "power", "power", "kicad"),
                           _layer(6, "In2.Cu", "power", "power", "kicad"), _layer(2, "B.Cu", "signal")],
                   pours=[pour(4, 1), pour(6, 2)])
    assert "PLANE_NO_ZONE" not in codes(run(m))
    m["pours"] = []
    assert codes(run(m)).count("PLANE_NO_ZONE") == 2


def test_plane_unfilled_zone_does_not_count():
    z = pour(4, 1)
    z["warning"] = "filled polygon list unavailable (zone not filled); using zone outline"
    f = one(run(base_model(pours=[z, pour(6, 2)])), "PLANE_NO_ZONE")
    assert f.items == ("In1.Cu",)


def test_plane_no_net_of_kind_names_generic():
    fs = run(base_model(nets=[{"code": 3, "name": "SIG"}]))
    fixes = [f.fix for f in fs if f.code == "PLANE_NO_ZONE"]
    assert any("with net a ground net" in x for x in fixes) and any("with net a power net" in x for x in fixes)


def test_plane_ground_pattern_from_cfg():
    cfg = Config.defaults().with_values({("nets", "ground_pattern"): "^EARTH"})
    m = base_model(nets=[{"code": 1, "name": "EARTH"}, {"code": 2, "name": "+3V3"}], pours=[pour(4, 1), pour(6, 2)])
    assert "PLANE_NO_ZONE" not in codes(run(m, cfg=cfg))


def test_layer_roles_needed_surfaces_for_3_layer_board():
    pcb = '(kicad_pcb (version 20260206) (layers (0 "F.Cu" signal) (4 "In1.Cu" signal) (2 "B.Cu" signal)) (setup))'
    m = base_model(layers=[_layer(0, "F.Cu", None), _layer(4, "In1.Cu", None), _layer(2, "B.Cu", None)])
    assert one(run(m, pcb=pcb), "LAYER_ROLES_NEEDED").severity == "error"


# ---- per code
def test_pcb_format_old():
    pcb = '(kicad_pcb (version 20221018) (layers (0 "F.Cu" signal) (2 "B.Cu" signal)))'
    assert one(run(pcb=pcb), "PCB_FORMAT_OLD").severity == "error"


def test_kicad_cli_missing(monkeypatch, tmp_path):
    monkeypatch.setattr(pf, "kicad_cli", lambda: tmp_path / "nope")
    assert one(run(), "KICAD_CLI_MISSING").severity == "error"


def test_via_not_set_up():
    f = one(run(pro={}), "VIA_NOT_SET_UP")
    assert f.severity == "warning" and "vias.fallback_diameter_mm" in f.fix and "Pre-defined Sizes" in f.fix
    assert "0.8" in f.message and "0.4" in f.message


def test_via_set_up_in_class_no_finding():
    pro = {"net_settings": {"classes": [{"name": "Default", "via_diameter": 0.6, "via_drill": 0.3}]}}
    assert "VIA_NOT_SET_UP" not in codes(run(pro=pro))


def _via(drill):
    return {"uuid": "v", "net": 3, "at": [0, 0], "d": 800_000, "drill": drill, "layers": [0, 2],
            "free": False, "locked": False}


def test_drill_outside_filter():
    cfg = Config.defaults().with_values({("vias", "allowed_drills_mm"): [0.3]})
    f = one(run(base_model(vias=[_via(400_000)]), cfg=cfg), "DRILL_OUTSIDE_FILTER")
    assert f.items == ("0.4",) and "vias.allowed_drills_mm" in f.fix


def test_drill_in_filter_no_finding():
    cfg = Config.defaults().with_values({("vias", "allowed_drills_mm"): [0.3]})
    assert "DRILL_OUTSIDE_FILTER" not in codes(run(base_model(vias=[_via(300_000)]), cfg=cfg))


def test_drill_filter_roundtrips_through_apply_accepted(tmp_path):
    from topor.ports import Decision
    from topor.run import apply_accepted_findings
    cfg = Config.defaults().with_values({("vias", "allowed_drills_mm"): [0.3]})
    fs = run(base_model(vias=[_via(400_000)]), cfg=cfg)
    out = apply_accepted_findings(Decision(proceed=True, accepted_codes=["DRILL_OUTSIDE_FILTER"]), fs, cfg,
                                  tmp_path / "topor.toml")
    assert out.get("vias", "allowed_drills_mm") == [0.3, 0.4]


def test_via_in_pad_unprotected():
    cfg = Config.defaults().with_values({("vias", "via_in_pad"): True})
    f = one(run(cfg=cfg), "VIA_IN_PAD_UNPROTECTED")
    assert f.severity == "warning" and "Via protection" in f.fix
    protected = PCB_OK.replace("(setup)", "(setup (capping yes) (filling yes))")
    assert "VIA_IN_PAD_UNPROTECTED" not in codes(run(cfg=cfg, pcb=protected))
    assert "VIA_IN_PAD_UNPROTECTED" not in codes(run())  # via_in_pad off


def test_pad_outside_board():
    m = base_model()
    m["pads"].append(_pad("J1", "3", 3, 41 * MM, 10 * MM))  # 1 mm outside the 40 mm outline
    f = one(run(m), "PAD_OUTSIDE_BOARD")
    assert f.items == ("J1.3",) and "nets.ignore_pads" in f.fix
    cfg = Config.defaults().with_values({("nets", "ignore_pads"): ["J1.3"]})
    assert "PAD_OUTSIDE_BOARD" not in codes(run(m, cfg=cfg))


def test_pad_inside_board_and_no_outline():
    assert "PAD_OUTSIDE_BOARD" not in codes(run())
    assert "PAD_OUTSIDE_BOARD" not in codes(run(base_model(outline=[])))


def test_preexisting_drc_info_and_error():
    rep = DrcReport(path=None, exit_code=5, seconds=0.1, violations=[object()], unconnected=[],
                    by_type={"clearance": 1})
    assert one(run(drc=rep), "PREEXISTING_DRC").severity == "info"
    cfg = Config.defaults().with_values({("run", "fail_on_preexisting"): True})
    assert one(run(drc=rep, cfg=cfg), "PREEXISTING_DRC").severity == "error"
    clean = DrcReport(path=None, exit_code=0, seconds=0.1, violations=[], unconnected=[], by_type={})
    assert "PREEXISTING_DRC" not in codes(run(drc=clean))


def test_track_angle_rule_error():
    f = one(run(rl=rules(warnings=["REFUSE: rule diag restricts track angle"])), "TRACK_ANGLE_RULE")
    assert f.severity == "error" and f.items == ("diag",)


def test_locked_copper():
    m = base_model(kept=[{"uuid": "t", "net": 3, "layer": 0, "width": 200_000,
                          "prim": {"kind": "seg", "a": [0, 0], "b": [1, 1]}, "locked": True}])
    f = one(run(m), "LOCKED_COPPER")
    assert f.severity == "info" and "run.respect_locked" in f.fix
    cfg = Config.defaults().with_values({("run", "respect_locked"): False})
    assert one(run(m, cfg=cfg), "LOCKED_COPPER").severity == "warning"
    assert "LOCKED_COPPER" not in codes(run())


def test_dangling_ends():
    m = base_model(dangling_ends=[{"uuid": "t", "net": 3, "layer": 0, "at": [1, 1]}])
    f = one(run(m), "DANGLING_ENDS")
    assert f.severity == "info" and "search.dangling_ends" in f.fix


def test_do_not_touch():
    cfg = Config.defaults().with_values({("nets", "do_not_touch"): ["+3*", "NOPE*"]})
    fs = [f for f in run(cfg=cfg) if f.code == "DO_NOT_TOUCH"]
    assert sorted(f.severity for f in fs) == ["info", "warning"]
    assert next(f for f in fs if f.severity == "info").items == ("+3V3",)
    assert next(f for f in fs if f.severity == "warning").items == ("NOPE*",)


def test_layer_types_mismatch():
    m = base_model(layers=[_layer(0, "F.Cu", "signal"), _layer(4, "In1.Cu", "signal", "power", "config"),
                           _layer(6, "In2.Cu", "pwr"), _layer(2, "B.Cu", "signal")])
    f = one(run(m), "LAYER_TYPES")
    assert f.severity == "warning" and f.items == ("In1.Cu",) and "Board Editor Layers" in f.fix
    assert "LAYER_TYPES" not in codes(run())


def test_scope_empty():
    m = base_model(pads=[_pad("U1", "1", 3, 10 * MM, 10 * MM)])
    assert one(run(m), "SCOPE_EMPTY").severity == "warning"
    assert "SCOPE_EMPTY" not in codes(run())


def test_non_defaults_and_defaults_used():
    cfg = Config.defaults().with_values({("run", "timeout_s"): 10.0})
    f = one(run(cfg=cfg), "NON_DEFAULTS")
    assert f.items == ("run.timeout_s",) and f.severity == "info"
    assert "NON_DEFAULTS" not in codes(run())
    rl = rules(per_net={"3": {"width_source": "default"}, "2": {"width_source": "netclass"}})
    d = one(run(rl=rl), "DEFAULTS_USED")
    assert d.severity == "info" and d.items == ("SIG",)


def test_groups_unavailable():
    pcb = PCB_OK.replace("(setup)", '(setup)\n  (group "g" (uuid "1") (members "2" "3"))')
    f = one(run(pcb=pcb), "GROUPS_UNAVAILABLE")
    assert f.severity == "info" and "Ungroup" in f.fix
    assert "GROUPS_UNAVAILABLE" not in codes(run())


def test_every_finding_has_message_and_fix():
    m = base_model()
    m["pads"].append(_pad("J1", "3", 3, 41 * MM, 10 * MM))
    for f in run(m, pro={}):
        assert f.message and f.fix, f.code


def test_preflight_does_not_mutate_inputs():
    m = base_model(pours=[pour(4, 1)])
    before = copy.deepcopy(m)
    run(m)
    assert m == before
