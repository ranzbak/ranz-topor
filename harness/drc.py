"""kicad-cli DRC runner: re-export of toporlib.drc (kept for the harness public names)."""
from toporlib.drc import *  # noqa: F401,F403
from toporlib.drc import (  # noqa: F401  (private helpers some callers import)
    INCOMPLETE_TYPES, ROUTING_TYPES, SUMMARY_MAX_LINES, SUMMARY_MAX_TYPES, DrcReport, Violation,
    _is_pre, _key, _split, _unconnected_desc, _unconnected_key, _uuids, classify, is_clean,
    kicad_cli, new_routing, split_unconnected, summarise,
)
from harness.kicad_env import run_drc  # noqa: F401,E402  (toporlib's, in an isolated KiCad home)
