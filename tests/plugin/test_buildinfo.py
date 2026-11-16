"""The splash version string: version + build stamp from the generated _build_info module, plain without it."""
from types import SimpleNamespace

from topor.buildinfo import display_version


def test_without_build_info_the_version_is_plain():
    assert display_version("0.4.0") == "0.4.0"
    assert display_version(None) is None


def test_with_build_info_date_and_revision():
    info = SimpleNamespace(BUILD_DATE="2026-11-14 01:05 UTC", BUILD_REV="7e320c1-dirty")
    assert display_version("0.4.0", info) == "0.4.0 · built 2026-11-14 01:05 UTC · 7e320c1-dirty"


def test_without_revision_or_version():
    assert display_version("0.4.0", SimpleNamespace(BUILD_DATE="2026-11-14 01:05 UTC", BUILD_REV="")) == "0.4.0 · built 2026-11-14 01:05 UTC"
    assert display_version(None, SimpleNamespace(BUILD_DATE="2026-11-14 01:05 UTC")) == "built 2026-11-14 01:05 UTC"
    assert display_version("0.4.0", SimpleNamespace(BUILD_DATE="")) == "0.4.0"
