import tomllib

import pytest

from topor import schema
from topor.config import Config, load_config, render_toml, update_config


def codes(findings):
    return [f.code for f in findings]


def test_default_file_roundtrips_to_defaults(tmp_path):
    cfg, findings = load_config(tmp_path)
    assert codes(findings) == ["CFG_CREATED"]
    assert (tmp_path / "topor.toml").exists()
    assert cfg.non_defaults() == {}
    cfg2, findings2 = load_config(tmp_path)
    assert findings2 == []
    assert cfg2.values == cfg.values
    for s in schema.SETTINGS:
        assert cfg2.get(s.section, s.key) == s.default, (s.section, s.key)
    tomllib.loads((tmp_path / "topor.toml").read_text())


def test_refill_after_accept_key(tmp_path):
    """Ruling C28 (gate review I-2): Accept = exactly one undo step by default; the refill is opt-in."""
    s = schema.SETTINGS_BY_KEY[("run", "refill_after_accept")]
    assert s.type == "bool" and s.default is False and s.plugin_field == "run.refill_after_accept"
    assert s.engine_field is None
    assert "second undo step" in s.doc and "press B (Edit → Fill All Zones)" in s.doc
    assert s.kicad_place == "KiCad → Edit → Fill All Zones (B)"
    (tmp_path / "topor.toml").write_text('version = 1\n[run]\nrefill_after_accept = true\n')
    cfg, findings = load_config(tmp_path)
    assert findings == [] and cfg.get("run", "refill_after_accept") is True


def test_unknown_key_warns_not_ignored(tmp_path):
    (tmp_path / "topor.toml").write_text(
        'version = 1\n[run]\ntimeut_s = 5\n[bogus]\nx = 1\n'
    )
    cfg, findings = load_config(tmp_path)
    unknown = [f for f in findings if f.code == "CFG_UNKNOWN_KEY"]
    assert len(unknown) == 2
    assert all(f.severity == "warning" for f in unknown)
    assert any("run.timeut_s" in f.message for f in unknown)
    assert cfg.get("run", "timeout_s") == 600


def test_value_out_of_range_finding(tmp_path):
    (tmp_path / "topor.toml").write_text(
        'version = 1\n[run]\ntimeout_s = -3\nexisting_copper = "melt"\nkicad_cli_timeout_s = "x"\n'
        '[output]\ngeometry = "45deg"\n'
    )
    cfg, findings = load_config(tmp_path)
    assert codes(findings).count("CFG_VALUE") == 3
    assert "CFG_VALUE_BACKLOG" in codes(findings)
    # invalid values fall back to the default
    assert cfg.get("run", "timeout_s") == 600
    assert cfg.get("run", "existing_copper") == "fixed"
    assert cfg.get("output", "geometry") == "arcs"
    assert all(f.fix for f in findings)


def test_version_mismatch(tmp_path):
    (tmp_path / "topor.toml").write_text("version = 2\n")
    _, findings = load_config(tmp_path)
    assert "CFG_VERSION" in codes(findings)


def test_parse_error_gives_defaults(tmp_path):
    (tmp_path / "topor.toml").write_text("this is [not toml\n")
    cfg, findings = load_config(tmp_path)
    assert "CFG_PARSE" in codes(findings)
    assert cfg.non_defaults() == {}


def test_timeout_none_means_disabled(tmp_path):
    (tmp_path / "topor.toml").write_text('version = 1\n[run]\ntimeout_s = "none"\n')
    cfg, findings = load_config(tmp_path)
    assert findings == []
    assert cfg.engine_options()["wall_clock_s"] == 0.0


def test_engine_options_units(tmp_path):
    cfg, _ = load_config(tmp_path)
    o = cfg.engine_options()
    assert o["wall_clock_s"] == 600.0
    assert o["margin_nm"] == 0
    assert o["existing_copper"] == "fixed"
    assert o["drc_loop_cap"] == 5
    assert o["plane_vias"] is True
    # every engine length field (…_nm) gets an int: the engine's serde refuses a float for an i64
    for s in schema.SETTINGS:
        if s.engine_field and s.engine_field.endswith("_nm") and s.engine_field in o:
            v = o[s.engine_field]
            assert all(isinstance(x, int) for x in v) if isinstance(v, list) else isinstance(v, int), s.key
    assert o["detour_factor"] == 2.0
    # settings not available yet are not sent; removed ones (CL cleanup) are gone
    assert "dangling_ends" not in o
    assert not {"expert", "threads", "partial_routes", "pathfinder_iter_cap", "patience", "min_gain_pct",
                "via_cost_nm", "rip_pass", "continue_rounds", "exit_stubs"} & set(o)
    assert o["steiner"] is False  # 4.2: the Steiner grid is an option again, default off
    c0 = cfg.with_values({("run", "timeout_s"): 0})
    assert c0.engine_options()["wall_clock_s"] == 0.0
    assert isinstance(c0.engine_options()["wall_clock_s"], float)


def test_update_config_keeps_comments(tmp_path):
    load_config(tmp_path)
    path = tmp_path / "topor.toml"
    before = path.read_text()
    n_comments = sum(1 for ln in before.splitlines() if ln.startswith("#"))
    update_config(path, {"vias.allowed_drills_mm": [0.3, 0.4], "run.timeout_s": 120,
                         "nets.do_not_touch": ["GND", "/a b"]})
    after = path.read_text()
    assert sum(1 for ln in after.splitlines() if ln.startswith("#")) >= n_comments - 1
    cfg2, findings = load_config(tmp_path)
    assert findings == []
    assert cfg2.get("run", "timeout_s") == 120
    assert cfg2.get("vias", "allowed_drills_mm") == [0.3, 0.4]
    assert cfg2.get("nets", "do_not_touch") == ["GND", "/a b"]
    assert cfg2.non_defaults() == {"run.timeout_s": 120, "vias.allowed_drills_mm": [0.3, 0.4],
                                   "nets.do_not_touch": ["GND", "/a b"]}
    # user comment and unrelated user edits survive
    path.write_text("# my note\n" + after.replace("respect_locked = true", "respect_locked = true  # mine"))
    update_config(path, {"run.timeout_s": 90})
    t = path.read_text()
    assert "# my note" in t and "respect_locked = true  # mine" in t
    assert load_config(tmp_path)[0].get("run", "timeout_s") == 90


def test_update_config_rejects_bad_change(tmp_path):
    load_config(tmp_path)
    with pytest.raises(ValueError):
        update_config(tmp_path / "topor.toml", {"run.nope": 1})
    with pytest.raises(ValueError):
        update_config(tmp_path / "topor.toml", {"run.timeout_s": -1})


def test_update_config_since_key_uncomments(tmp_path):
    load_config(tmp_path)
    p = tmp_path / "topor.toml"
    update_config(p, {"search.dangling_ends": "fixed"})
    assert load_config(tmp_path)[0].get("search", "dangling_ends") == "fixed"


def test_report_dir_for(tmp_path, monkeypatch):
    root = tmp_path / "root"
    monkeypatch.setenv("TOPOR_REPORT_ROOT", str(root))
    proj = tmp_path / "proj"
    ts = "20261101-120000"
    cfg = Config.defaults()
    assert cfg.report_dir_for(proj, "eprom", ts) == root / "eprom" / ts
    rel = cfg.with_values({("output", "report_dir"): "topor-report"})
    assert rel.report_dir_for(proj, "eprom", ts) == proj / "topor-report" / ts
    ab = cfg.with_values({("output", "report_dir"): str(tmp_path / "abs")})
    assert ab.report_dir_for(proj, "eprom", ts) == tmp_path / "abs" / ts
    monkeypatch.delenv("TOPOR_REPORT_ROOT")
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "xdg"))
    assert cfg.report_dir_for(proj, "eprom", ts) == tmp_path / "xdg/kicad/10.0/topor/eprom" / ts
    monkeypatch.delenv("XDG_CACHE_HOME")
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    assert cfg.report_dir_for(proj, "eprom", ts) == tmp_path / "home/.cache/kicad/10.0/topor/eprom" / ts


def test_render_toml_each_key_preceded_by_doc():
    lines = render_toml(Config.defaults()).splitlines()
    for s in schema.SETTINGS:
        idx = [i for i, ln in enumerate(lines) if ln.lstrip("# ").startswith(f"{s.key} =")]
        assert idx, s.key
        assert lines[idx[0] - 1].startswith("# ") and "default" in lines[idx[0] - 1], s.key


def test_topological_core_settings_reach_the_engine():
    """K1: every setting of the topological core was marked since 0.5.0 while the plugin is 0.4.0, so
    engine_options() dropped them: [optimize], [factors], net_tree, fuse_branches, stitch_islands ... set in
    topor.toml never reached the engine. They are available now (their defaults equal the engine's)."""
    from topor.schema import SETTINGS, available
    cfg = Config.defaults().with_values({("optimize", "rounds"): 2, ("search", "fuse_branches"): False,
                                         ("factors", "weight"): {"via": 30.0}, ("planes", "stitch_islands"): False})
    o = cfg.engine_options()
    assert o["topo_rounds"] == 2 and o["fuse_branches"] is False and o["stitch_islands"] is False
    assert o["factors"]["weight"] == {"via": 30.0}
    for key in ("rounds", "max_ripped", "objective", "mm_per_via", "pick", "variants", "moves", "fluctuation"):
        s = next(x for x in SETTINGS if x.section == "optimize" and x.key == key)
        assert available(s), key


def test_removed_keys_are_ignored_with_a_warning(tmp_path):
    """CL cleanup: an old topor.toml that sets removed keys (also whole removed sections) still loads; each such
    key gives CFG_REMOVED_KEY ("ignored, removed in ...") and nothing else changes."""
    from topor.schema import REMOVED, SETTINGS_BY_KEY
    assert not set(REMOVED) & set(SETTINGS_BY_KEY)
    (tmp_path / "topor.toml").write_text(
        "version = 1\n[run]\nthreads = 4\npartial_routes = true\ntimeout_s = 30\n"
        "[vias]\nreduction = false\n[ripup]\nrip_pass = false\ngrowth = 2.0\n"
        "[iterations]\ncontinue_rounds = 0\n[expert]\nhist_fail = 2.0\ndrc_loop_cap = 3\n")
    cfg, findings = load_config(tmp_path)
    removed = sorted(f.items[0] for f in findings if f.code == "CFG_REMOVED_KEY")
    assert removed == sorted(["run.threads", "run.partial_routes", "vias.reduction", "ripup.rip_pass", "ripup.growth",
                              "iterations.continue_rounds", "expert.hist_fail"])
    assert all("ignored: removed in" in f.message and f.severity == "warning"
               for f in findings if f.code == "CFG_REMOVED_KEY")
    assert [f.code for f in findings if f.code != "CFG_REMOVED_KEY"] == []
    assert cfg.get("run", "timeout_s") == 30.0 and cfg.get("expert", "drc_loop_cap") == 3
    text = render_toml(Config.defaults())
    assert "[ripup]" not in text and "[iterations]" not in text and "threads =" not in text
