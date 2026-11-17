"""fx_models.py BOARDS_DIR OUT_DIR: engine model JSON of each approved fx pattern problem board (engine/tests/fixtures/fx/model-<name>.json)."""
import sys
from pathlib import Path

from harness.copy import copy_project
from harness.model import export_model, load_board, write_model_json
from harness.rip import record_widths
from harness.rules import resolve_rules

src, out = Path(sys.argv[1]), Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)
for n in ("collinear_row", "diag_exit", "via_fold", "qfn_two_ends"):
    cp = copy_project(src / n / "problem" / f"{n}.kicad_pcb", Path('/tmp/fxw') / n)
    b = load_board(cp)
    dru = cp.dru.read_text() if cp.dru else None
    rules = resolve_rules(b, dru_text=dru, widths_before=record_widths(b))
    write_model_json(export_model(b, rules, name=n, dru_text=dru), out / f"model-{n}.json")
    print(n)
