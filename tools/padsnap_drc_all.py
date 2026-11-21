"""pad-snap (A1b): kicad-cli DRC with every severity (warnings included: track_dangling is a warning by default).

    python tools/padsnap_drc_all.py <pcb> <out.json>

Prints one line: violations, track_dangling, unconnected. kicad-cli runs in the harness's isolated KiCad home.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import harness  # noqa: F401,E402  (isolated KiCad env)
from toporlib.drc import kicad_cli, run_cli  # noqa: E402


def main(pcb: str, out: str) -> int:
    r = run_cli([str(kicad_cli()), "pcb", "drc", "--format", "json", "--severity-all", "--all-track-errors",
                 "--refill-zones", "-o", out, pcb])
    d = json.loads(Path(out).read_text())
    v = d.get("violations", [])
    print(f"drc-all {Path(pcb).name} rc {r.returncode} violations {len(v)} "
          f"track_dangling {sum(1 for x in v if x.get('type') == 'track_dangling')} unconnected {len(d.get('unconnected_items', []))}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(*sys.argv[1:3]))
