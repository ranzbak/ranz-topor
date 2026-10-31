"""B11: the pre-flight Help button finds docs/user/operate.md in the repo AND in the installed plugin layout.

Repo:      <repo>/plugin/topor/ui_wx.py            -> <repo>/docs/user/operate.md          (parents[2])
Installed: <plugins>/org.topor.router/topor/ui_wx.py -> org.topor.router/docs/user/operate.md (parents[1],
           tools/build.sh ships docs/user/ next to topor/)
No dialog is created or shown.
"""
from pathlib import Path

import pytest

pytest.importorskip("wx")

from topor import ui_wx  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]


def _touch(p: Path) -> Path:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("# Operating topor\n\n## Options\n")
    return p


def test_help_doc_installed_layout(tmp_path):
    plug = tmp_path / "plugins" / "org.topor.router"
    mod = _touch(plug / "topor" / "ui_wx.py")
    doc = _touch(plug / "docs" / "user" / "operate.md")
    assert ui_wx.find_help_doc(mod) == doc


def test_help_doc_repo_layout(tmp_path):
    mod = _touch(tmp_path / "repo" / "plugin" / "topor" / "ui_wx.py")
    doc = _touch(tmp_path / "repo" / "docs" / "user" / "operate.md")
    assert ui_wx.find_help_doc(mod) == doc


def test_help_doc_installed_wins_and_missing_is_none(tmp_path):
    mod = _touch(tmp_path / "a" / "b" / "topor" / "ui_wx.py")
    assert ui_wx.find_help_doc(mod) is None
    both_inst = _touch(tmp_path / "a" / "b" / "docs" / "user" / "operate.md")
    _touch(tmp_path / "a" / "docs" / "user" / "operate.md")
    assert ui_wx.find_help_doc(mod) == both_inst


def test_help_doc_of_this_checkout():
    assert ui_wx.HELP_DOC == ROOT / "docs" / "user" / "operate.md"
    assert ui_wx.HELP_ANCHOR == "options"


def test_build_ships_operate_md_next_to_topor():
    """tools/build.sh copies docs/user/ into the plugin dir, which is where find_help_doc looks first."""
    sh = (ROOT / "tools" / "build.sh").read_text()
    assert 'cp -r docs/user/. "$d/docs/user/"' in sh and 'cp -r plugin/topor "$d/topor"' in sh
