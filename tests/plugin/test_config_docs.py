import json
import re
import subprocess
import sys
from pathlib import Path

import pytest

from topor import schema
from topor.config import Config, render_toml
from topor.docs_gen import render_reference_md

ROOT = Path(__file__).resolve().parents[2]


def test_every_setting_documented():
    seen = set()
    for s in schema.SETTINGS:
        assert (s.section, s.key) not in seen
        seen.add((s.section, s.key))
        assert s.doc.strip() and "\n" not in s.doc, (s.section, s.key)
        # a sentence; a trailing "(... 0.4.0 ...)" statement (ruling C34) ends with ".)"
        assert s.doc.rstrip().endswith((".", ".)")), (s.section, s.key)
        assert s.type in {"int", "float", "bool", "str", "list", "table"}
    for name, place in schema.KICAD_ONLY:
        assert name and place.startswith("KiCad")


def test_reference_md_matches_committed_file():
    committed = (ROOT / "docs/user/configuration.md").read_text()
    assert render_reference_md(schema.SETTINGS) == committed


def test_reference_md_has_every_key():
    md = render_reference_md(schema.SETTINGS)
    for s in schema.SETTINGS:
        assert f"`{s.key}`" in md
    for sec in {s.section for s in schema.SETTINGS}:
        assert f"## `[{sec}]`" in md


def test_toml_comments_name_kicad_places():
    text = render_toml(Config.defaults())
    for name, place in schema.KICAD_ONLY:
        assert f"# {name}: {place} (not set here)" in text
    for s in schema.SETTINGS:
        if s.kicad_place:
            assert s.kicad_place in text
    assert "Clearance: KiCad → File → Board Setup → Design Rules → Constraints" in text


def test_since_keys_are_commented_out_until_available():
    text = render_toml(Config.defaults())
    n = 0
    for s in schema.SETTINGS:
        if s.since and not schema.available(s):
            n += 1
            assert f"# {s.key} =" in text
            assert not re.search(rf"^{s.key} =", text, re.M)
    assert n >= 10
    assert "(available from 0.5.0)" in text


def _flatten(d):
    out = set()
    for k, v in d.items():
        if k == "expert" and isinstance(v, dict):
            out |= {f"expert.{kk}" for kk in v}
        else:
            out.add(k)
    return out


def test_every_engine_option_is_a_setting_or_internal(engine):
    if not hasattr(engine, "default_options"):
        pytest.xfail("topor_engine.default_options not merged yet (P1)")
    keys = _flatten(json.loads(engine.default_options()))
    known = {s.engine_field for s in schema.SETTINGS if s.engine_field}
    missing = keys - known - set(schema.ENGINE_INTERNAL)
    assert not missing, f"engine options without a Setting or ENGINE_INTERNAL entry: {sorted(missing)}"
    gone = {s.engine_field for s in schema.SETTINGS
            if s.engine_field and schema.available(s)} - keys
    assert not gone, f"settings mapped to engine fields the engine lacks: {sorted(gone)}"


def test_engine_internal_has_reasons():
    assert set(schema.ENGINE_INTERNAL) >= {"pinned", "drc_spots", "skip_self_check", "profile", "seed", "scope",
                                           "net_priority", "via_cost_by_net", "detour_reference_budget_pct"}
    assert all(r.strip() for r in schema.ENGINE_INTERNAL.values())


def test_cli_check_passes():
    r = subprocess.run([sys.executable, "-m", "topor.docs_gen", "--check"], cwd=ROOT,
                       env={"PYTHONPATH": str(ROOT / "plugin"), "PATH": "/usr/bin"},
                       capture_output=True, text=True)
    assert r.returncode == 0, r.stderr + r.stdout


NO_EFFECT = "(No effect in 0.4.0; applied from 0.5.0.)"
#: live keys whose 0.4.0 effect is partial: the doc string ends with the precise statement (ruling C34)
PARTIAL = {
    ("run", "existing_copper"): "smooth is treated as fixed in 0.4.0",
}


def _consumers() -> str:
    """Source of every plugin / toporlib module that reads settings (not the schema, loader or doc generator)."""
    skip = {"schema.py", "config.py", "docs_gen.py"}
    return "\n".join(p.read_text() for base in ("plugin/topor", "toporlib")
                     for p in sorted((ROOT / base).glob("*.py")) if p.name not in skip)


def test_keys_without_effect_say_so():
    """Ruling C34 / F I-7: a live key that 0.4.0 does not apply keeps its key, and its doc string (rendered into
    topor.toml and configuration.md) ends with the no-effect marker or the precise partial statement."""
    by = schema.SETTINGS_BY_KEY
    for k in (("run", "detour_reference"),):  # A9: pour_served / priority / ignore_pads are applied now
        assert by[k].doc.endswith(NO_EFFECT), (k, by[k].doc)
        assert schema.available(by[k])  # stays a live key: a user's value carries into 0.5.0
    for k, words in PARTIAL.items():
        last = by[k].doc.rsplit("(", 1)[-1]
        assert "0.4.0" in last and words in last and by[k].doc.endswith(".)"), (k, by[k].doc)
    md = (ROOT / "docs/user/configuration.md").read_text()
    text = render_toml(Config.defaults())
    assert md.count(NO_EFFECT) >= 1 and text.count(NO_EFFECT) >= 1


def test_live_plugin_keys_have_a_consumer_or_the_marker():
    """Every live key the engine does not take (engine_field None) is read somewhere in plugin/ or toporlib/,
    or says that it has no effect in 0.4.0."""
    src = _consumers()
    for s in schema.SETTINGS:
        if s.engine_field or not schema.available(s) or s.section == "layers":
            continue
        if NO_EFFECT in s.doc:
            assert f'"{s.key}"' not in src, f"{s.section}.{s.key} is read but documented as without effect"
            continue
        assert f'"{s.key}"' in src, f"{s.section}.{s.key}: no consumer and no '{NO_EFFECT}'"


def test_timeout_doc_says_what_is_charged():
    """F MINOR 4: drcloop charges only router time (route_fn); pre-flight DRC, DRC rounds, exports are not."""
    doc = schema.SETTINGS_BY_KEY[("run", "timeout_s")].doc
    assert "router time" in doc and "not counted" in doc and "whole run" not in doc, doc
