"""B11: the user and developer documentation is complete and its links resolve.

* every relative link (and its ``#anchor``) in README.md, CONTRIBUTING.md, docs/README.md, docs/user/*.md and
  docs/dev/*.md points at an existing file / heading (GitHub heading slugs);
* every ``#anchor`` the plugin UI opens in operate.md (``plugin/topor/ui_wx.py``) is a heading there;
* operate.md reaches every KiCad setting the plugin reads (exact menu path) and links the generated
  ``topor.toml`` reference, and its Limitations section names the known limits of 0.4.0.

Complexity: O(total size of the scanned documents).
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT / "plugin") not in sys.path:
    sys.path.insert(0, str(ROOT / "plugin"))

from topor.schema import KICAD_ONLY, SETTINGS  # noqa: E402

USER = ROOT / "docs" / "user"
DEV = ROOT / "docs" / "dev"
REQUIRED = [ROOT / "README.md", ROOT / "CONTRIBUTING.md", ROOT / "docs" / "README.md",
            USER / "install.md", USER / "operate.md", USER / "troubleshoot.md", USER / "configuration.md",
            USER / "gui-test-checklist.md", DEV / "debug.md"]

_FENCE = re.compile(r"^(```|~~~).*?^\1[^\n]*$", re.M | re.S)
_CODE = re.compile(r"`[^`\n]*`")
_LINK = re.compile(r"!?\[[^\]\n]*\]\(\s*<?([^)\s>]+)>?(?:\s+\"[^\"]*\")?\s*\)")
_HEAD = re.compile(r"^#{1,6}\s+(.+?)\s*#*\s*$", re.M)
_ANCHOR = re.compile(r"""\+\s*["']#([A-Za-z0-9_-]+)["']""")


def docs() -> list[Path]:
    out = [ROOT / "README.md", ROOT / "CONTRIBUTING.md", ROOT / "docs" / "README.md"]
    out += sorted(USER.glob("*.md")) + sorted(DEV.glob("*.md"))
    return [p for p in out if p.is_file()]


def slug(heading: str) -> str:
    """GitHub heading anchor: inline markup dropped, lower case, punctuation removed, spaces -> '-'. O(len)."""
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", heading)  # [text](url) -> text
    text = text.replace("`", "").replace("*", "").strip().lower()
    text = re.sub(r"[^\w\- ]", "", text)
    return text.replace(" ", "-")


def anchors(path: Path) -> set[str]:
    """Every heading slug of `path` (duplicates get -1, -2 like GitHub) plus explicit ``<a id=...>``. O(len)."""
    text = _FENCE.sub("", path.read_text(encoding="utf-8"))
    seen: dict[str, int] = {}
    out = set(re.findall(r"""<a\s+(?:id|name)=["']([^"']+)["']""", text))
    for h in _HEAD.findall(text):
        s = slug(h)
        n = seen.get(s, 0)
        out.add(s if n == 0 else f"{s}-{n}")
        seen[s] = n + 1
    return out


def links(path: Path) -> list[str]:
    text = _CODE.sub("", _FENCE.sub("", path.read_text(encoding="utf-8")))
    return _LINK.findall(text)


def test_required_documents_exist():
    missing = [str(p.relative_to(ROOT)) for p in REQUIRED if not p.is_file()]
    assert not missing, missing


def test_slug_rules():
    assert slug("## Options".lstrip("# ")) == "options"
    assert slug("`[run]`") == "run"
    assert slug("CFG_CREATED") == "cfg_created"
    assert slug("Accept, Reject and undo") == "accept-reject-and-undo"
    assert slug("Step 3 — preview (Accept/Reject)") == "step-3--preview-acceptreject"


@pytest.mark.parametrize("doc", docs(), ids=lambda p: str(p.relative_to(ROOT)))
def test_relative_links_and_anchors_resolve(doc):
    bad = []
    for target in links(doc):
        if re.match(r"^[a-z][a-z0-9+.-]*:", target, re.I):  # http:, https:, mailto:, file: ...
            continue
        path, _, anchor = target.partition("#")
        dest = doc if not path else (doc.parent / path).resolve()
        if not dest.exists():
            bad.append(f"{target}: no such file")
            continue
        if anchor and dest.is_file() and dest.suffix == ".md" and anchor not in anchors(dest):
            bad.append(f"{target}: no heading #{anchor} in {dest.name}")
    assert not bad, bad


def test_ui_help_anchors_exist_in_operate_md():
    src = (ROOT / "plugin" / "topor" / "ui_wx.py").read_text(encoding="utf-8")
    wanted = set(_ANCHOR.findall(src)) | set(re.findall(r"""HELP_ANCHOR\s*=\s*["']([^"']+)["']""", src))
    assert "options" in wanted, "the Help button's anchor was not found in ui_wx.py"
    have = anchors(USER / "operate.md")
    assert wanted <= have, sorted(wanted - have)


def _place(p: str) -> str:
    """The menu-path part of a schema ``kicad_place`` (everything from the first `` (`` explanation on removed)."""
    return p.split(" (", 1)[0].strip()


def test_operate_md_reaches_every_kicad_setting_and_toml_key():
    text = (USER / "operate.md").read_text(encoding="utf-8")
    places = {_place(s.kicad_place) for s in SETTINGS if s.kicad_place} | {_place(p) for _, p in KICAD_ONLY}
    missing = sorted(p for p in places if p not in text)
    assert not missing, missing
    assert "configuration.md" in text  # the generated reference holds every topor.toml key
    ref = (USER / "configuration.md").read_text(encoding="utf-8")
    assert all(f"`{s.key}`" in ref for s in SETTINGS)
    for sec in {s.section for s in SETTINGS}:
        assert f"[{sec}]" in text, f"operate.md does not mention the [{sec}] section"


def test_operate_md_fab_floors_match_the_profile():
    """F I-4: operate.md documents the fab floors with the values of toporlib.rules.JLCPCB_1OZ, the extra via
    drills (routing_vias), where the report lists them, that pre-defined vias are not read and that
    ``[rules] fab_profile = "board"`` turns the floors off (M0v)."""
    from toporlib.rules import JLCPCB_1OZ as P

    text = (USER / "operate.md").read_text(encoding="utf-8")
    sec = text.split("### Fab minimums and via sizes", 1)[1].split("\n### ", 1)[0]
    mm = lambda nm: f"{nm / 1e6:g} mm"  # noqa: E731
    for k in ("clearance", "width", "via_d_min_auto", "via_drill_min_auto", "annular_2l", "annular_4l",
              "pth_hole_to_track", "hole_to_hole_via", "edge"):
        assert mm(P[k]) in sec, (k, mm(P[k]))
    assert ", ".join(f"{d / 1e6:g}" for d in P["routing_vias"]) + " mm" in sec
    for words in ("JLCPCB", "Fab floors raised", '`[rules] fab_profile = "board"`', "not read", "0.5.0",
                  "VIA_NOT_SET_UP"):
        assert words in sec, words
    lim = text.split("## Limitations", 1)[1]
    assert "Cancel reacts within about a second in every phase" in lim  # MINOR 10
    assert "Do not edit the board while topor runs" in lim


def test_operate_md_limitations_section():
    text = (USER / "operate.md").read_text(encoding="utf-8")
    assert "limitations" in anchors(USER / "operate.md")
    lim = text.split("## Limitations", 1)[1].split("\n## ", 1)[0]
    for word in ("Linux", "GROUPS_UNAVAILABLE", "do_not_touch", "ignore_pads",
                 "bounding box", "polygonised", "not reached", "free", "keep_unconnected_vias",
                 "sorted", "copper layers", "flattened", "padstack", "UNVERIFIED",
                 "fallback_diameter_mm", "fallback_drill_mm", "0.8", "0.4", "smooth"):
        assert word in lim, f"Limitations does not mention {word!r}"
