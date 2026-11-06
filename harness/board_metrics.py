"""Metrics of a routed board FILE, judged by KiCad's DRC only: the numbers `tools/route_board.sh` reports, for a
board routed elsewhere (the live KiCad plugin run of `tools/gui_e2e.sh`, a board saved by hand).

    .venv/bin/python -m harness.board_metrics <routed.kicad_pcb> --input <stripped input pcb> --hand <hand pcb>
        [--report-dir <topor report folder>] [--out <metrics.json>] [--png <compare.png>] [--name N]

All three boards are copies (never the read-only originals). Metrics: unconnected (DRC unconnected items after
`--refill-zones`), unconnected_hand, routing_new (routing-type violations involving copper that is in the routed
board but not in the input: every track / via of a stripped input is new), vias, tracks; with `--report-dir` also
routed / total / stop_reason / router_s from the plugin run's ``result.json``. `--png` renders hand (left) and
routed (right) with the open ratsnest (DRC unconnected items) in blue, as route_board does.

Complexity: three kicad-cli DRC runs + O(len(board text)).
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def copper_uuids(text: str) -> list:
    """uuids of every segment / arc / via of a board text. O(len(text))."""
    from toporlib import pcbfile
    return [s.uuid for s in pcbfile.find_items(text, ("segment", "arc", "via"))]


def filled_copy(pcb: Path, dst: Path) -> Path:
    """A copy of `pcb` (with its .kicad_pro / .kicad_dru, so zone clearances follow the project rules) in `dst`, its
    zones refilled by kicad-cli DRC `--save-board`. A board saved by KiCad after Accept keeps the fills from before
    routing until the user refills (B); pictures of it must show the refilled pours. O(one DRC run)."""
    import shutil
    from toporlib.drc import run_drc
    pcb = Path(pcb)
    dst.mkdir(parents=True, exist_ok=True)
    for ext in (".kicad_pcb", ".kicad_pro", ".kicad_dru"):
        src = pcb.with_suffix(ext)
        if src.is_file():
            shutil.copy(src, dst / src.name)
    out = dst / pcb.name
    run_drc(out, dst / f"{pcb.stem}--drc-fill.json", save_board=True)
    return out


def board_metrics(routed: Path, input_pcb: Path, hand: Path, *, work: Path, report_dir: Path | None = None,
                  png: Path | None = None, name: str = "board") -> dict:
    from toporlib import pcbfile
    from toporlib.drc import classify, run_drc

    work.mkdir(parents=True, exist_ok=True)
    rtext, itext = Path(routed).read_text(), Path(input_pcb).read_text()
    before = set(copper_uuids(itext))
    created = [u for u in copper_uuids(rtext) if u not in before]
    base = run_drc(Path(input_pcb), work / f"{name}--drc-input.json")
    hand_drc = run_drc(Path(hand), work / f"{name}--drc-hand.json")
    routed_drc = run_drc(Path(routed), work / f"{name}--drc-routed.json")
    cls = classify(routed_drc, base, created, unconnected_baseline=hand_drc)
    m = {"board": name, "unconnected": len(routed_drc.unconnected), "unconnected_hand": len(hand_drc.unconnected),
         "unconnected_input": len(base.unconnected), "routing_new": cls["routing_new"],
         "drc_by_type_new": cls["by_type_new"],
         "vias": len(pcbfile.find_items(rtext, ("via",))),
         "tracks": len(pcbfile.find_items(rtext, ("segment", "arc"))), "items_new": len(created)}
    if report_dir is not None:
        r = json.loads((Path(report_dir) / "result.json").read_text())
        m.update(routed=r.get("routed"), connections=r.get("total"), status=r.get("status"),
                 stop_reason=r.get("stop_reason"), router_s=r.get("router_s"), report_dir=str(report_dir))
    if png is not None:
        from harness.render import board_bbox_nm, caption, compose_compare, draw_ratsnest, render_png
        hr = render_png(Path(hand), work / f"{name}-hand-raw.png")
        hr = draw_ratsnest(hr, hr, hand_drc.unconnected, bbox_nm=board_bbox_nm(Path(hand)))
        from toporlib.drc import ratsnest_caption, ratsnest_marks
        h = caption(hr, f"{name}: hand ({m['unconnected_hand']} unconnected, blue"
                        f"{ratsnest_caption(ratsnest_marks(hand_drc.unconnected))})", work / f"{name}-hand.png")
        filled = filled_copy(Path(routed), work / "routed-filled")  # the pours as the DRC above saw them
        tr = render_png(filled, work / f"{name}-routed-raw.png")
        tr = draw_ratsnest(tr, tr, routed_drc.unconnected, bbox_nm=board_bbox_nm(filled))
        routed_txt = f"{m['routed']}/{m['connections']} routed, " if "routed" in m else ""
        t = caption(tr, f"{name}: topor in KiCad ({routed_txt}{m['unconnected']} unconnected, blue"
                        f"{ratsnest_caption(ratsnest_marks(routed_drc.unconnected))})",
                    work / f"{name}-routed.png")
        compose_compare(h, t, Path(png))
        m["png"] = str(png)
    return m


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("routed", type=Path)
    ap.add_argument("--input", type=Path, required=True, help="the stripped input board the run started from")
    ap.add_argument("--hand", type=Path, required=True, help="the hand-routed reference copy")
    ap.add_argument("--report-dir", type=Path, default=None, help="the plugin run's report folder (result.json)")
    ap.add_argument("--out", type=Path, default=None, help="write the metrics JSON here")
    ap.add_argument("--png", type=Path, default=None, help="write the hand-vs-routed PNG here")
    ap.add_argument("--name", default="board")
    a = ap.parse_args(argv)
    work = (a.out.parent if a.out else Path(a.routed).parent) / f"{a.name}-metrics-work"
    m = board_metrics(a.routed, a.input, a.hand, work=work, report_dir=a.report_dir, png=a.png, name=a.name)
    if a.out:
        a.out.write_text(json.dumps(m, indent=1))
    print(json.dumps({k: v for k, v in m.items() if k != "drc_by_type_new"}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
