import pytest

from topor.config import load_config, render_toml
from topor.layers import resolve_layer_roles, resolve_layer_roles_ex


def cu(n, types=None):
    names = ["F.Cu"] + [f"In{i}.Cu" for i in range(1, n - 1)] + ["B.Cu"]
    return [(nm, (types or {}).get(nm, "signal")) for nm in names]


@pytest.mark.parametrize("n,expect", [
    (2, "S S"), (4, "S G P S"), (6, "S G P S G S"), (8, "S G S P G S G S")])
def test_defaults_by_count(n, expect):
    roles, findings = resolve_layer_roles(cu(n), {})
    assert findings == []
    m = {"S": "signal", "G": "gnd", "P": "pwr"}
    assert [roles[nm] for nm, _ in cu(n)] == [m[c] for c in expect.split()]


@pytest.mark.parametrize("n", [3, 10])
def test_other_counts_need_roles(n):
    roles, findings = resolve_layer_roles(cu(n), {})
    assert [f.code for f in findings] == ["LAYER_ROLES_NEEDED"]
    assert findings[0].severity == "error" and str(n) in findings[0].message
    assert "In1.Cu" in findings[0].fix


def test_cfg_covering_all_layers_resolves_odd_count():
    cfg = {nm: "signal" for nm, _ in cu(3)}
    cfg["In1.Cu"] = "gnd"
    roles, findings = resolve_layer_roles(cu(3), cfg)
    assert findings == [] and roles["In1.Cu"] == "gnd"


def test_cfg_wins_over_kicad_and_defaults():
    layers = cu(4, {"In1.Cu": "power"})
    roles, src, _ = resolve_layer_roles_ex(layers, {"In1.Cu": "signal", "F.Cu": "mixed"})
    assert roles["In1.Cu"] == "signal" and src["In1.Cu"] == "config"
    assert roles["F.Cu"] == "mixed"
    assert src["In2.Cu"] == "kicad"


def test_kicad_types_only_when_any_non_signal():
    layers = cu(4, {"In1.Cu": "power", "In2.Cu": "jumper"})
    roles, src, f = resolve_layer_roles_ex(layers, {})
    assert f == []
    assert [roles[n] for n, _ in layers] == ["signal", "power", "jumper", "signal"]
    assert set(src.values()) == {"kicad"}
    roles, src, _ = resolve_layer_roles_ex(cu(4), {})
    assert set(src.values()) == {"default"} and roles["In1.Cu"] == "gnd"


def test_kicad_types_cover_odd_counts():
    roles, f = resolve_layer_roles(cu(3, {"In1.Cu": "power"}), {})
    assert f == [] and roles["In1.Cu"] == "power"


def test_layer_roles_bare_toml_form_roundtrips(tmp_path):
    (tmp_path / "topor.toml").write_text(
        'version = 1\n[layers]\nF.Cu = "signal"\nIn1.Cu = "gnd"\n"In2.Cu" = "pwr"\nB.Cu = "mixed"\n')
    cfg, findings = load_config(tmp_path)
    assert findings == []
    assert cfg.layer_roles == {"F.Cu": "signal", "In1.Cu": "gnd", "In2.Cu": "pwr", "B.Cu": "mixed"}
    p = tmp_path / "again"
    p.mkdir()
    (p / "topor.toml").write_text(render_toml(cfg))
    cfg2, f2 = load_config(p)
    assert f2 == [] and cfg2.layer_roles == cfg.layer_roles


def test_layer_roles_invalid_and_unknown(tmp_path):
    (tmp_path / "topor.toml").write_text('version = 1\n[layers]\nF.Cu = "plane"\nfoo = 1\n')
    cfg, findings = load_config(tmp_path)
    codes = [f.code for f in findings]
    assert codes.count("CFG_VALUE") == 1 and codes.count("CFG_UNKNOWN_KEY") == 1
    assert "F.Cu" not in cfg.layer_roles
