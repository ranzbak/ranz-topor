"""kicad-cli path resolution (Linux). O(1)."""
from __future__ import annotations

import os
from pathlib import Path


def kicad_cli() -> Path:
    """`$KICAD_CLI` if set, else /usr/bin/kicad-cli."""
    return Path(os.environ.get("KICAD_CLI", "/usr/bin/kicad-cli"))
