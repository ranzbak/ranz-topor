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
        assert s.doc.rstrip().endswith("."), (s.section, s.key)
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
