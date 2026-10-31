"""B6: the message catalogue and the troubleshooting guide."""

from __future__ import annotations

import re
from pathlib import Path

from topor import messages
from topor.findings import SEVERITIES
from topor.schema import SETTINGS_BY_KEY

ROOT = Path(__file__).resolve().parents[2]
_EMIT = re.compile(r"""(?:Finding|_mk)\(\s*["']([A-Z][A-Z0-9_]+)["']""")


def emitted_codes() -> set:
    found = set()
    for base in ("plugin", "toporlib"):
        for p in (ROOT / base).rglob("*.py"):
            found |= set(_EMIT.findall(p.read_text()))
    return found


def test_every_code_has_cause_and_fix():
    em = emitted_codes()
    assert {"CFG_CREATED", "LAYER_ROLES_NEEDED", "INTERNAL", "ENGINE_VERSION", "PLANE_NO_ZONE",
            "PAD_OUTSIDE_BOARD"} <= em
    missing = em - set(messages.MESSAGES)
    assert not missing, f"emitted but not in MESSAGES: {sorted(missing)}"
    keys = {f"{s}.{k}" for s, k in SETTINGS_BY_KEY}
    for code, m in messages.MESSAGES.items():
        assert m.cause.strip() and m.fix.strip(), code
        assert m.severity in SEVERITIES, code
        assert "→" in m.fix or any(k in m.fix for k in keys), \
            f"{code}: fix names neither a KiCad menu path nor a SETTINGS key"


def test_catalogue_covers_brief_codes():
    brief = """ENGINE_VERSION KICAD_CLI_MISSING CFG_CREATED CFG_UNKNOWN_KEY CFG_VALUE CFG_VERSION CFG_VALUE_BACKLOG
    CFG_WRITE CFG_PARSE VIA_NOT_SET_UP DRILL_OUTSIDE_FILTER VIA_IN_PAD_UNPROTECTED POWER_BY_NAME MARKER_IGNORED
    MARKER_CONFLICT MARKER_PAIR PAD_OUTSIDE_BOARD PREEXISTING_DRC TRACK_ANGLE_RULE LOCKED_COPPER DANGLING_ENDS
    DO_NOT_TOUCH LAYER_TYPES SCOPE_EMPTY NON_DEFAULTS DEFAULTS_USED GROUPS_UNAVAILABLE PLANE_NO_ZONE
    LAYER_ROLES_NEEDED NOTHING_ROUTED PREVIEW_FAILED CLEANUP_SKIPPED_PARTIAL PCB_FORMAT_OLD INTERNAL""".split()
    assert not [c for c in brief if c not in messages.MESSAGES]


def test_fix_for():
    assert messages.fix_for("PLANE_NO_ZONE").strip()
    assert messages.fix_for("NOPE") == ""


def test_troubleshoot_has_section_per_code():
    text = (ROOT / "docs" / "user" / "troubleshoot.md").read_text()
    heads = set(re.findall(r"^### (\S+)\s*$", text, re.M))
    assert set(messages.MESSAGES) <= heads, sorted(set(messages.MESSAGES) - heads)


def test_troubleshoot_matches_catalogue():
    text = (ROOT / "docs" / "user" / "troubleshoot.md").read_text()
    for code, m in messages.MESSAGES.items():
        sec = text.split(f"### {code}\n", 1)[1].split("\n### ", 1)[0]
        assert m.cause in sec and m.fix in sec, code
