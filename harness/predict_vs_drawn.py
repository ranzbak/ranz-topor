"""M1 compass tables (stage 6.3): per violation class predicted vs drawn vs KiCad DRC, per connection detour and vias
planned vs drawn, for route_board runs or a score_runs root.

    .venv/bin/python -m harness.predict_vs_drawn <run_dir | score_runs root> [...] [--cuts DIR] [--json OUT] [--conns N]

A run directory is a `tools/route_board.sh` output: its `<name>--compass.json` (written by route_board) is printed;
an older run without one is rebuilt from `reports/**/result.json` (the engine result must carry `compass`) and the
DRC files `<name>--drc-routed.json` / `<name>--drc-input.json`. A score_runs root prints every `<board>/s<k>/` and a
one-line summary per run.

`--cuts DIR` (with a run of `[capacity] model = "edges"` and `--set capacity.cut_report=DIR`): the per-layer cut test
of the picked plan (`harness.cutclass`) turns a drawing loss whose wire crosses an over-full cut into a model gap (the
per-edge plan said fits, the cut test says over-full; docs/dev/inflated-sketch.md §6.1). With `model = "cuts"` the
engine's compass already has the per-edge evidence and `--cuts` is not needed.

Columns (see toporlib/compass.py): predicted = what the plan handed to the drawing predicts (items, connections in
them); predicted, drawn clean = predicted items no drawn violation is near; drawn = violations of the written copper
by our checker, classified against the plan; KiCad DRC = new routing DRC items of the written board by the class of the
nearest drawn violation (0.5 mm), "KiCad only" = none near. Reading (the instrument M1 is for): drawn in a class the
plan predicts = a fault of that built step; drawn where the plan says "not predicted" = a missing method step; drawing
loss = the drawing; KiCad only = our checker. Source: TopoR 6.3 manual pp. 72-73 (the variant table shows predicted
criteria) and p. 183 (DRC after the metric stage). O(runs x (violations x items + DRC items x violations)).
"""
from __future__ import annotations

import argparse
import glob
import json
import sys
from pathlib import Path

from toporlib import compass as compass_mod


def _rebuild(run: Path, cuts: Path | None, join: dict | None = None) -> dict | None:
    hits = sorted(glob.glob(str(run / "reports" / "**" / "result.json"), recursive=True))
    if not hits:
        return None
    res = json.loads(Path(hits[-1]).read_text())
    route = res.get("route", res)
    drc_new, unc = None, None
    rd = sorted(run.glob("*--drc-routed.json"))
    if rd:
        from toporlib.drc import new_routing
        routed = compass_mod.drc_from_json(rd[0])
        ip = sorted(run.glob("*--drc-input.json"))
        base = compass_mod.drc_from_json(ip[0]) if ip else None
        drc_new = new_routing(routed, base, None)
        unc = (res.get("drc") or {}).get("incomplete", {}).get("unconnected_new")
    gap = None
    if cuts is not None:
        from harness import cutclass
        r = res if "route" in res else {"route": res}
        k, c = cutclass.picked(r)
        rep = json.loads((cuts / f"run{k}" / f"cycle{c}.json").read_text())
        gap = {row["i"]: f"cut test: over-full ({row['sub']}); the per-edge plan: fits"
               for row in cutclass.classify(r, rep) if row["class"] == "planner: over-full cut"}
    return compass_mod.build(route, drc_new, unc, model_gap=gap, **(join or {}))


def load(run: Path, cuts: Path | None = None, rebuild: bool = False, join: dict | None = None) -> dict | None:
    """The compass of one route_board run directory (see the module doc); `rebuild`: from result.json and the DRC
    files even when `<name>--compass.json` exists (after a classifier change); `join`: build() keywords of the KiCad
    join (drc_radius_mm, join_nets), implies a rebuild."""
    if cuts is None and not rebuild and not join:
        hits = sorted(run.glob("*--compass.json"))
        if hits:
            return json.loads(hits[0].read_text())
    return _rebuild(run, cuts, join)


def runs_of(root: Path) -> list[Path]:
    if list(root.glob("*--compass.json")) or (root / "reports").exists():
        return [root]
    return sorted(p for p in root.glob("*/s*") if p.is_dir())


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("paths", nargs="+", type=Path)
    ap.add_argument("--cuts", type=Path, default=None, help="cut_report directory of the run (model gap, edges model)")
    ap.add_argument("--rebuild", action="store_true", help="classify again from result.json + DRC files")
    ap.add_argument("--join-mm", type=float, default=None,
                    help="KiCad join radius in mm (default: the run's [output] compass_join_mm, 0.5)")
    ap.add_argument("--join-any-net", action="store_true", help="join KiCad items by place alone (compass_join_nets = false)")
    ap.add_argument("--json", type=Path, default=None, help="write all compasses as one JSON file")
    ap.add_argument("--conns", type=int, default=15, help="flagged connections listed per run")
    a = ap.parse_args(argv)
    allc = {}
    rc = 0
    for p in a.paths:
        for run in runs_of(p):
            join = {k: v for k, v in (("drc_radius_mm", a.join_mm), ("join_nets", False if a.join_any_net else None))
                    if v is not None}
            cp = load(run, a.cuts, a.rebuild, join)
            label = f"{run.parent.name}/{run.name}" if run.name.startswith("s") and run.name[1:].isdigit() else run.name
            print(f"## {label}\n")
            if cp is None:
                print("no compass (no route_board output or no engine result)\n")
                rc = 1
                continue
            print("\n".join(compass_mod.table_md(cp, max_conns=a.conns)) + "\n")
            if any(r["class"] == "model_gap" and r["predicted"] is None for r in cp.get("rows") or []):
                print("(model gap not measured: run it with --set capacity.compass_cut_check=true, as harness.score_runs "
                      "does; the cut test then runs after the drawing, outside the time limit)\n")
            print(compass_mod.summary_line(cp) or "", "\n")
            allc[label] = cp
    if a.json:
        a.json.write_text(json.dumps(allc, indent=1))
    return rc


if __name__ == "__main__":
    sys.exit(main())
