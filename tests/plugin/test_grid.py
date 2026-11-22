"""grid-snap: the grid source chain (topor/grid.py) and the grid_mm setting (docs/dev/design/grid-snap.md)."""
import json
from pathlib import Path

from topor import grid
from topor.config import coerce
from topor.schema import SETTINGS_BY_KEY

PCB = "(kicad_pcb (setup (pad_to_mask_clearance 0) (grid_origin 75.927 45) (aux_axis_origin 150 100)))"


def _settings(tmp_path, g: dict):
    (tmp_path / "pcbnew.json").write_text(json.dumps({"window": {"grid": g}}))
    return tmp_path


SIZES = [{"name": "", "x": "1000 mil", "y": "1000 mil"}, {"name": "", "x": "0.5 mm", "y": "0.5 mm"},
         {"name": "", "x": "0.1 mm", "y": "0.1 mm"}, {"name": "User Grid", "x": "10 mil", "y": "5 mil"}]


def test_parse_size_units():
    assert grid.parse_size("10 mil") == 0.254
    assert grid.parse_size("0,254 mm") == 0.254
    assert grid.parse_size("0.25 mm") == 0.25
    assert grid.parse_size("3") == 3.0
    assert grid.parse_size(0.1) == 0.1
    for bad in ("", "x mm", "1 furlong", "-1 mm", "0 mm", None, True):
        assert grid.parse_size(bad) is None, bad


def test_origin_from_the_board():
    assert grid.grid_origin_nm(PCB) == (75_927_000, 45_000_000)
    assert grid.grid_origin_nm("(kicad_pcb (setup (aux_axis_origin 150 100)))") == (0, 0)
    assert grid.grid_origin_nm("") == (0, 0)


def test_kicad_grids_current_and_overrides():
    g = {"sizes": SIZES, "last_size": 3, "overrides_enabled": False, "override_wires": True, "override_wires_idx": 2}
    assert grid.kicad_grids({"window": {"grid": g}}) == ((0.254, 0.127), (0.254, 0.127), "current grid")
    g = {**g, "overrides_enabled": True}
    t, v, how = grid.kicad_grids({"window": {"grid": g}})
    assert (t, v, how) == ((0.1, 0.1), (0.254, 0.127), "wire override")
    g = {**g, "override_vias": True, "override_vias_idx": 1}
    assert grid.kicad_grids({"window": {"grid": g}}) == ((0.1, 0.1), (0.5, 0.5), "wire override / via override")
    # KiCad 6/7: plain strings, the user grid after the list
    old = {"sizes": ["0.5 mm", "0.1 mm"], "last_size": 2, "user_grid_x": "0,254 mm", "user_grid_y": "0,254 mm"}
    assert grid.kicad_grids({"window": {"grid": old}})[0] == (0.254, 0.254)
    assert grid.kicad_grids({"window": {"grid": {**old, "last_size": 1}}})[0] == (0.1, 0.1)
    assert grid.kicad_grids({})[0] is None


def test_resolve_toml_first(tmp_path):
    s = _settings(tmp_path, {"sizes": SIZES, "last_size": 1})
    o, lines = grid.resolve(0.1, PCB, s)
    assert o == {"grid_track_nm": [100_000, 100_000], "grid_via_nm": [100_000, 100_000], "grid_origin_nm": [75_927_000, 45_000_000]}
    assert len(lines) == 1 and "[drawing] grid_mm" in lines[0] and not lines[0].startswith("warning")


def test_resolve_kicad_settings_and_coarse(tmp_path):
    s = _settings(tmp_path, {"sizes": SIZES, "last_size": 0})
    o, lines = grid.resolve("auto", PCB, s)
    assert o["grid_track_nm"] == [25_400_000, 25_400_000]
    assert "pcbnew.json" in lines[0] and "current grid" in lines[0]
    assert lines[1].startswith("warning: grid snap: the grid is KiCad's last saved setting (pcbnew.json)")
    assert lines[2].startswith("warning: grid snap: the grid (25.4 mm) is coarser than 0.5 mm")
    o, lines = grid.resolve("auto", PCB, _settings(tmp_path, {"sizes": SIZES, "last_size": 1}))
    assert o["grid_track_nm"] == [500_000, 500_000] and len(lines) == 2, "0.5 mm is not coarse"


def test_resolve_missing_warns_and_turns_off(tmp_path):
    for settings in (None, tmp_path / "none", _settings(tmp_path, {"sizes": [], "last_size": 4})):
        o, lines = grid.resolve("auto", PCB, settings)
        assert o["grid_track_nm"] == [0, 0] and o["grid_via_nm"] == [0, 0]
        where = "" if settings is None else f" (looked in {settings / 'pcbnew.json'})"
        assert lines == ["warning: grid snap: no grid found (neither [drawing] grid_mm nor KiCad's grid settings"
                         f"{where}); the copper is drawn without the grid"]
    (tmp_path / "bad").mkdir()
    (tmp_path / "bad" / "pcbnew.json").write_text("{not json")
    assert grid.resolve("auto", PCB, tmp_path / "bad")[0]["grid_track_nm"] == [0, 0]


def test_settings_dir_follows_kicads_path_rule(tmp_path):
    """common/paths.cpp CalculateUserSettingsPath: base ($KICAD_CONFIG_HOME, else the platform config folder +
    kicad) + <major>.<minor> (GetSettingsVersion; nightly 9.99)."""
    h = tmp_path / "home"
    d = lambda v, env, plat="linux": grid.kicad_settings_dirs(v, env, plat, h)  # noqa: E731
    # KICAD_CONFIG_HOME keeps the version folder under it (harness/gui_e2e.py: <h>/.config/kicad + 10.0)
    assert d((10, 0), {"KICAD_CONFIG_HOME": "/k/cfg"}) == [Path("/k/cfg/10.0")]
    assert d((10, 0), {"KICAD_CONFIG_HOME": "", "XDG_CONFIG_HOME": "/x"}) == [Path("/x/kicad/10.0")], "empty = unset"
    assert d((9, 99), {}) == [h / ".config/kicad/9.99"], "nightly"
    assert d((9, 0), {"XDG_CONFIG_HOME": ""}) == [h / ".config/kicad/9.0"], "empty XDG = ~/.config"
    assert grid.kicad_settings_dirs((9, 0), {"HOME": "/u"}, "linux") == [Path("/u/.config/kicad/9.0")], "$HOME"
    assert d((9, 0), {}, "darwin") == [h / "Library/Preferences/kicad/9.0"]
    assert d((9, 0), {"APPDATA": "C:/Users/u/AppData/Roaming"}, "win32") == [Path("C:/Users/u/AppData/Roaming/kicad/9.0")]
    assert d((9, 0), {"KICAD_CONFIG_HOME": "/k", "XDG_CONFIG_HOME": "/x"}, "darwin") == [Path("/k/9.0")]


def test_settings_dir_unknown_version_takes_the_newest(tmp_path):
    base = tmp_path / ".config" / "kicad"
    for v in ("8.0", "9.0", "9.99", "10.0", "colors", "10.0.bak"):
        (base / v).mkdir(parents=True)
    dirs = grid.kicad_settings_dirs(None, {}, "linux", tmp_path)
    assert dirs == [base / "10.0", base / "9.99", base / "9.0", base / "8.0"], "numeric, not text order"
    assert grid.kicad_settings_dirs(None, {}, "linux", tmp_path / "nohome") == []
    assert grid.pick_settings_dir(dirs) == base / "10.0", "none has pcbnew.json: the first, for the warning"
    (base / "9.0" / "pcbnew.json").write_text("{}")
    assert grid.pick_settings_dir(dirs) == base / "9.0"
    assert grid.pick_settings_dir([]) is None


def test_grid_settings_values():
    gm = SETTINGS_BY_KEY[("drawing", "grid_mm")]
    assert coerce(gm, "auto") == ("auto", None)
    assert coerce(gm, " AUTO ") == ("auto", None)
    assert coerce(gm, 0.254) == (0.254, None)
    assert coerce(gm, "0.25")[1] is not None
    assert coerce(gm, 0.0)[1] is not None
    gs = SETTINGS_BY_KEY[("drawing", "grid_snap")]
    assert gs.default == "off" and gs.engine_field == "grid_snap"
    for v in ("off", "fallback", "prefer"):
        assert coerce(gs, v) == (v, None)
    assert coerce(gs, "on")[1] is not None
