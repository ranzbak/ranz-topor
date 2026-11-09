"""Engine fixtures of the fit patterns (shopping list 2.2 / 2.3): the standalone cut-model test input.

    .venv/bin/python -m harness.fit_fixtures [PATTERNS_DIR] [OUT_DIR]

For every pattern under PATTERNS_DIR (default `scratch/fit-patterns`, written by `harness.fit_patterns`) the
problem board is exported as the engine's BoardModel (`harness.model.export_model`, rules from
`harness.rules.resolve_rules`, layer roles as the plugin resolves them, the same steps as `harness.route_board`)
and written with the cut and wire data of its `expected.json` to OUT_DIR/<name>.json (default
`engine/tests/fixtures/fit`). `engine/tests/topo_fit.rs` feeds the expected wires (their witness polylines) to the
cut model on these boards and checks the predicted capacity, demand and slack per cut against the hand numbers.
Nothing here routes. Deterministic: the patterns are.
"""
from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
_KEEP_CUT = ("id", "role", "layer", "p", "q", "length_euclid", "length_octagonal", "demand", "slack_euclid",
             "slack_octagonal", "crossing")
_KEEP_WIRE = ("net", "width", "clearance", "crosses", "witness_layer", "witness")


def export_one(pdir: Path) -> dict:
    sys.path.insert(0, str(ROOT / "plugin"))
    from harness.copy import copy_project
    from harness.model import export_model, load_board
    from harness.rip import record_widths
    from harness.rules import resolve_rules
    from topor.layers import is_routed, resolve_layer_roles_ex

    name = pdir.name
    exp = json.loads((pdir / "expected.json").read_text())
    with tempfile.TemporaryDirectory() as tmp:
        cp = copy_project(pdir / "problem" / f"{name}.kicad_pcb", Path(tmp) / name)
        b = load_board(cp)
        dru = cp.dru.read_text() if cp.dru is not None else None
        rules = resolve_rules(b, dru_text=dru, widths_before=record_widths(b))
        model = export_model(b, rules, name=name, dru_text=dru)
        del b
    roles, _, _ = resolve_layer_roles_ex([(l["name"], l["kind"]) for l in model["layers"]], {})
    for l in model["layers"]:
        if l["name"] in roles:
            l["routable"] = is_routed(roles[l["name"]])
    return {
        "name": name,
        "kind": exp["kind"],
        "model": model,
        "cuts": [{k: c.get(k) for k in _KEEP_CUT} for c in exp["cuts"]],
        "wires": [{k: w.get(k) for k in _KEEP_WIRE} for w in exp["wires"]],
    }


def main(argv: list[str] | None = None) -> int:
    a = list(sys.argv[1:] if argv is None else argv)
    src = Path(a[0]) if a else ROOT / "scratch" / "fit-patterns"
    out = Path(a[1]) if len(a) > 1 else ROOT / "engine" / "tests" / "fixtures" / "fit"
    out.mkdir(parents=True, exist_ok=True)
    for pdir in sorted(p for p in src.iterdir() if (p / "expected.json").exists()):
        doc = export_one(pdir)
        (out / f"{pdir.name}.json").write_text(json.dumps(doc, separators=(",", ":"), sort_keys=True))
        print(f"{pdir.name}: {len(doc['model']['pads'])} pads, {len(doc['cuts'])} cuts -> {out / (pdir.name + '.json')}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
