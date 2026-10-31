"""Measure a benchmark board (on a copy) for the gate: `python -m harness.measure --board eprom-to-kernal`.

Prints a <= 15-line summary: source sha256, copper counts, widths, netclasses, rules used, original
DRC counts and the wire count from `engine/examples/extract_wires.rs`. The source tree is only read
(copy first; sha256 of the board file printed, whole-tree hash verified before and after).
"""
from __future__ import annotations

import argparse
import hashlib
import re
import subprocess
import tempfile
from collections import Counter
from pathlib import Path

from harness.copy import assert_unchanged, copy_project
from harness.drc import ROUTING_TYPES, run_drc
from harness.model import export_model, load_board, write_model_json
from harness.paths import BENCH_BOARDS, MM
from harness.rip import record_widths
from harness.rules import resolve_rules

ROOT = Path(__file__).resolve().parents[1]


def copper_facts(board) -> dict:
    """Ground truth read straight from pcbnew: counts, length and widths of the board's copper."""
    segs = arcs = vias = free = 0
    track_nm = arc_nm = 0.0
    widths: Counter = Counter()
    nets: set[str] = set()
    for t in board.GetTracks():
        cls = t.GetClass()
        if cls == "PCB_VIA":
            vias += 1
            free += bool(t.GetIsFree())
            continue
        if cls == "PCB_ARC":
            arcs += 1
            arc_nm += float(t.GetRadius()) * abs(t.GetAngle().AsRadians())
        else:
            segs += 1
            track_nm += float(t.GetLength())
        widths[int(t.GetWidth())] += 1
        nets.add(str(t.GetNetname()))
    return {"segments": segs, "arcs": arcs, "vias": vias, "free_vias": free,
            "track_mm": round((track_nm + arc_nm) / MM, 3), "widths": dict(sorted(widths.items())),
            "nets_with_copper": sorted(nets)}


def wire_count(model_json: Path) -> str:
    """Run the extract_wires example on a model JSON and return `N (skipped S, hand H mm)`."""
    r = subprocess.run(
        ["cargo", "run", "--release", "-q", "--manifest-path", str(ROOT / "engine/Cargo.toml"),
         "--example", "extract_wires", "--", str(model_json)],
        capture_output=True, text=True, cwd=ROOT)
    if r.returncode != 0:
        tail = (r.stderr.strip().splitlines() or ["?"])[-1]
        return f"unavailable (cargo exit {r.returncode}: {tail})"
    w = re.search(r"wires: (\d+)", r.stdout)
    h = re.search(r"hand_len_mm: ([0-9.]+)", r.stdout)
    sk = re.search(r"skipped: (\d+)", r.stdout)
    if w and h and sk:
        return f"{w.group(1)} (skipped {sk.group(1)}, hand {h.group(1)} mm)"
    return r.stdout[:80]


def measure(board: str, work: Path) -> list[str]:
    src = BENCH_BOARDS[board]
    cp = copy_project(src, work / "work")
    sha = hashlib.sha256(src.read_bytes()).hexdigest()
    b = load_board(cp)
    f = copper_facts(b)
    widths = record_widths(b)
    rules = resolve_rules(b, dru_text=cp.dru.read_text() if cp.dru else None, widths_before=widths)
    drc = run_drc(cp.pcb, work / "drc.json")
    routing = sum(v for k, v in drc.by_type.items() if k in ROUTING_TYPES)
    model = export_model(b, rules, name=board, dru_text=cp.dru.read_text() if cp.dru else None)
    mj = write_model_json(model, work / "model.json")
    ncls = Counter(r["netclass"] for r in rules["per_net"].values())
    nets_cls = Counter(rules["per_net"][str(n["code"])]["netclass"] for n in model["nets"]
                       if n["name"] in f["nets_with_copper"])
    lines = [
        f"measure {board}: source sha256 {sha}",
        f"  copper: seg={f['segments']} arc={f['arcs']} via={f['vias']} (free {f['free_vias']}, "
        f"not free {f['vias'] - f['free_vias']}) track+arc={f['track_mm']} mm",
        "  widths: " + " ".join(f"{w / MM:g}mm x{c}" for w, c in f["widths"].items()),
        f"  nets with copper: {len(f['nets_with_copper'])} of {len(model['nets'])}",
        "  netclasses (all nets): " + ", ".join(f"{k} {v}" for k, v in sorted(ncls.items())),
        "  netclasses (nets with copper): " + ", ".join(f"{k} {v}" for k, v in sorted(nets_cls.items())),
        f"  rules: clearance {rules['min_clearance'] / MM:g} edge {rules['edge_clearance'] / MM:g} "
        f"min_width {rules['min_track_width'] / MM:g} mm; default netclass clr "
        f"{rules['default']['clearance'] / MM:g} w {rules['default']['width'] / MM:g}",
        "  floors applied: " + ("; ".join(f"{x['name']} {x['design'] / MM:g}->{x['floor'] / MM:g}"
                                         for x in rules["floors_applied"]) or "none"),
        f"  orig DRC: total {len(drc.violations)} routing {routing} unconnected {len(drc.unconnected)} "
        f"by type {dict(sorted(drc.by_type.items()))}",
        f"  engine wires: {wire_count(mj)}",
    ]
    assert_unchanged(src.parent, cp.src_hashes)
    lines.append("  source tree unchanged (sha256 verified)")
    return lines[:15]


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Measure a benchmark board on a copy.")
    ap.add_argument("--board", default="eprom-to-kernal", choices=sorted(BENCH_BOARDS))
    args = ap.parse_args(argv)
    with tempfile.TemporaryDirectory(prefix="topor-measure-") as tmp:
        print("\n".join(measure(args.board, Path(tmp))))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
