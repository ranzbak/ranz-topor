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


def test_troubleshoot_generated_part_is_current():
    """The part after '## Findings' is exactly ``render_troubleshoot()`` (regenerate after editing MESSAGES)."""
    text = (ROOT / "docs" / "user" / "troubleshoot.md").read_text()
    assert text.split("\n## Findings\n\n", 1)[1].rstrip("\n") == messages.render_troubleshoot().rstrip("\n")


def _app_exit_codes() -> set:
    """Exit codes topor.app.main returns (``return 0|1|2`` and ``_exit_code``), plus argparse's usage exit 2."""
    src = (ROOT / "plugin" / "topor" / "app.py").read_text()
    return {int(c) for c in re.findall(r"return\s+(\d)\b", src)} | {0, 1, 2}


def test_troubleshoot_covers_exit_codes_and_install_problems():
    """B11: besides one section per code, troubleshoot.md explains every exit code of the plugin process
    ("exited with code N") and the wheel / plugin-environment problems."""
    text = (ROOT / "docs" / "user" / "troubleshoot.md").read_text()
    heads = re.findall(r"^### (.+?)\s*$", text, re.M)
    low = [h.lower() for h in heads]
    for n in sorted(_app_exit_codes()):
        assert any(f"exited with code {n}" in h for h in low), f"no '### ... exited with code {n}' section"
    for topic in ("no module named topor_engine", "no module named kipy", "button", "plugin environment",
                  "python", "api"):
        assert any(topic in h for h in low), f"no troubleshooting section about {topic!r}"
    assert "topor-crash.log" in text and "Recreate Plugin Environment" in text


def test_marker_conflict_text_matches_priority_order():
    """Ruling C22: the catalogue states the precedence priority.py implements (marker > netclass > table)."""
    cause = messages.MESSAGES["MARKER_CONFLICT"].cause
    order = cause.split("wins", 1)[1]  # "... wins: the @N marker, then the topor_pN netclass, then nets.priority"
    assert order.index("marker") < order.index("netclass") < order.index("nets.priority"), cause
