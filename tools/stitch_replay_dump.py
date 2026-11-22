"""Dump one finished route_board run as the stitching stage's input, for `examples/stitch_replay.rs`
(pad-pour-via: replay the pour stitching on a board in seconds, without routing it again).

    .venv/bin/python -m tools.stitch_replay_dump <run_dir> <out_dir>

`<run_dir>` is the `s<seed>` directory of a `harness.score_runs` / `tools/route_board.sh` run (it holds
`<name>-input/`, `<name>-topor/` and `reports/`). Writes to `<out_dir>`:
- `model.json`: the board model of the input copy, exported as `harness/route_board.py` does (rules, layer roles);
- `options.json`: the engine options of the run's non-default settings (`--set` values are read from the run's
  `report.md` settings line when present, else the defaults);
- `items.json`: the routed copper (KiCad frame, engine `Item` JSON): every track and via of the routed copy that is
  not in the input copy, minus the stitching items the run's notes name (stitch vias, pad stubs, region vias);
- `routed_pads.json`: `[{"label", "at"}]` of the routed connections' pad ends (the run's `result.json`).

Approximate: tidy/post may have merged a stitch stub with routed copper; the replay says what it dropped.
Read-only on the run; pcbnew is isolated by `harness` (no user config written).
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

import harness  # noqa: F401  isolate KiCad before pcbnew


def _items(board) -> tuple[list, list]:
    import pcbnew
    tracks, vias = [], []
    for t in board.GetTracks():
        net = int(t.GetNetCode())
        if t.GetClass() == "PCB_VIA":
            p = t.GetPosition()
            vias.append({"kind": "via", "net": net, "at": [int(p.x), int(p.y)], "d": int(t.GetWidth(pcbnew.F_Cu)),
                         "drill": int(t.GetDrillValue()), "layers": [], "group": None})
        elif t.GetClass() == "PCB_ARC":
            s, m, e = t.GetStart(), t.GetMid(), t.GetEnd()
            tracks.append({"net": net, "layer": int(t.GetLayer()), "width": int(t.GetWidth()),
                           "prim": {"kind": "arc", "start": [int(s.x), int(s.y)], "mid": [int(m.x), int(m.y)],
                                    "end": [int(e.x), int(e.y)]}, "group": None})
        else:
            a, b = t.GetStart(), t.GetEnd()
            tracks.append({"net": net, "layer": int(t.GetLayer()), "width": int(t.GetWidth()),
                           "prim": {"kind": "seg", "a": [int(a.x), int(a.y)], "b": [int(b.x), int(b.y)]}, "group": None})
    return tracks, vias


def project_copy(pcb: Path):
    from harness.copy import ProjectCopy
    pro = pcb.with_suffix(".kicad_pro")
    return ProjectCopy(pcb, pro if pro.exists() else None, None)


def _key(it: dict) -> tuple:
    if it.get("kind") == "via":
        return ("v", it["net"], *it["at"])
    p = it["prim"]
    pts = [tuple(p[k]) for k in (("a", "b") if p["kind"] == "seg" else ("start", "mid", "end"))]
    return ("t", it["net"], it["layer"], *sorted(pts))


_PT = r"\((-?\d+\.\d+), (-?\d+\.\d+)\)"


def stitch_points(warnings: list[str]) -> tuple[set, list]:
    """(via positions nm, pad stub ends nm) named by the run's stitching notes (KiCad frame, 1 µm grid)."""
    vias, stubs = set(), []
    for w in warnings:
        if w.startswith("stitch via "):
            m = re.search(r" at " + _PT, w)
            if m:
                vias.add((round(float(m[1]) * 1e3), round(float(m[2]) * 1e3)))
        elif w.startswith("pad stub "):
            m = re.search(r"-> " + _PT + r" \([\d.]+ mm(, via)?\)", w)
            if m:
                p = (round(float(m[1]) * 1e3), round(float(m[2]) * 1e3))
                stubs.append(p)
                if m[3]:
                    vias.add(p)
        elif w.startswith("stitch ("):
            tail = w.split(" via(s)", 1)[-1]
            for m in re.finditer(_PT, tail):
                vias.add((round(float(m[1]) * 1e3), round(float(m[2]) * 1e3)))
    return vias, stubs


def main(argv=None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    run, out = Path(argv[0]), Path(argv[1])
    out.mkdir(parents=True, exist_ok=True)
    from harness.model import export_model, load_board
    from harness.rules import resolve_rules
    from topor.config import Config
    from topor.layers import is_routed, resolve_layer_roles_ex
    inp_pcb = next(run.glob("*-input/*.kicad_pcb"))
    name = inp_pcb.parent.name.removesuffix("-input")
    routed_pcb = run / f"{name}-topor" / f"{name}.kicad_pcb"
    res = sorted(run.glob("reports/*/*/result.json"))[-1]
    result = json.loads(res.read_text())
    cfg = Config.defaults()
    sets = {}
    rep = res.parent / "report.md"
    if rep.exists():  # the NON_DEFAULTS finding: "N setting(s) differ from the defaults: a.b = v, c.d = 'w'. Fix: ..."
        import ast
        m = re.search(r"differ from the defaults: (.*?)\. Fix:", rep.read_text())
        for kv in (m[1].split(", ") if m else []):
            k, _, v = kv.partition(" = ")
            sec, _, key = k.partition(".")
            try:
                sets[(sec, key)] = ast.literal_eval(v)
            except (ValueError, SyntaxError):
                sets[(sec, key)] = v
    try:
        cfg = cfg.with_values(sets)
    except KeyError:
        pass
    dru_p = next(inp_pcb.parent.glob("*.kicad_dru"), None)
    dru = dru_p.read_text() if dru_p else None
    from harness.rip import record_widths
    hand_pcb = run / f"{name}-hand" / f"{name}.kicad_pcb"
    widths = record_widths(load_board(project_copy(hand_pcb))) if hand_pcb.exists() else {}
    # the engine saw the pours refilled on the ripped copy (run.refill_pours): export that copy when the run kept it
    ripped = sorted(run.glob(f"work/*/ripped/{name}.kicad_pcb"))
    mb = load_board(project_copy(ripped[-1])) if ripped else None
    ib = load_board(project_copy(inp_pcb))
    rules = resolve_rules(ib, dru_text=dru, widths_before=widths, fab_profile=str(cfg.get("rules", "fab_profile")))
    model = export_model(mb or ib, rules, name=name, dru_text=dru)
    roles, _, _ = resolve_layer_roles_ex([(l["name"], l["kind"]) for l in model["layers"]], {})
    for l in model["layers"]:
        if l["name"] in roles:
            l["routable"] = is_routed(roles[l["name"]])
    it, iv = _items(ib)
    have = {_key(x) for x in it + iv}
    rt, rv = _items(load_board(project_copy(routed_pcb)))
    warnings = result.get("route", {}).get("warnings", [])
    svias, stubs = stitch_points(warnings)
    near = lambda p, q: abs(p[0] / 1e3 - q[0]) <= 2 and abs(p[1] / 1e3 - q[1]) <= 2  # noqa: E731  nm vs µm, 2 µm
    items, dropped = [], {"vias": 0, "stubs": 0}
    for v in rv:
        if _key(v) in have:
            continue
        if any(near(v["at"], q) for q in svias):
            dropped["vias"] += 1
            continue
        items.append(v)
    for t in rt:
        if _key(t) in have:
            continue
        p = t["prim"]
        ends = [p["a"], p["b"]] if p["kind"] == "seg" else [p["start"], p["end"]]
        if any(near(e, q) for e in ends for q in stubs):
            dropped["stubs"] += 1
            continue
        items.append(t)
    conns = result.get("route", {}).get("connections", [])
    pads = [{"label": t["label"], "at": t["at"]} for c in conns if c.get("status") == "routed"
            for t in (c["from"], c["to"]) if t.get("kind") == "pad"]
    (out / "model.json").write_text(json.dumps(model))
    (out / "options.json").write_text(json.dumps(cfg.engine_options()))
    (out / "items.json").write_text(json.dumps(items))
    (out / "routed_pads.json").write_text(json.dumps(pads))
    print(f"{name}: {len(items)} routed items, {len(pads)} routed pad ends; dropped stitch items {dropped} "
          f"(notes: {len(svias)} via points, {len(stubs)} stub ends) -> {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
