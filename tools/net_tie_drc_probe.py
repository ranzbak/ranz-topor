"""KiCad net-tie DRC probe (task NT): which track / net-tie-pad contacts does kicad-cli flag?

Writes a small KiCad 10 board of 3-pad net-tie footprints (`net_tie_pad_groups "1,2"`, pad 1 =
net Ak, pad 2 = net Bk, pad 3 = untied net Ck; optional copper bridge `fp_poly` between pads 1
and 2, as `Jumper:SolderJumper-3_P1.3mm_Bridged12`, dcsgonebadge JP1), one footprint + one track
per case, runs `kicad-cli pcb drc` on it and compares each case with the expected outcome. The
engine pins the result in `engine/tests/net_tie.rs` (Checker) and `engine/tests/obstacle_model.rs`.

Every track starts inside a plain single-pad footprint of its own net (an "anchor") unless it
already starts in a pad of its own net: KiCad's net propagation gives an unanchored track that
touches a single pad that pad's net (case 29 vs 30), which would hide the short.

Usage: .venv/bin/python tools/net_tie_drc_probe.py <out_dir>    (exit 1 on a mismatch)
Verified with kicad-cli 10.0.8 (2026-11-01): all cases as expected.
"""
import json
import re
import subprocess
import sys
import uuid
from pathlib import Path

HEAD = """(kicad_pcb
\t(version 20260206)
\t(generator "pcbnew")
\t(generator_version "10.0")
\t(general (thickness 1.6) (legacy_teardrops no))
\t(paper "A4")
\t(layers
\t\t(0 "F.Cu" signal)
\t\t(2 "B.Cu" signal)
\t\t(1 "F.Mask" user)
\t\t(3 "B.Mask" user)
\t\t(25 "Edge.Cuts" user)
\t\t(35 "F.Fab" user)
\t)
\t(setup (pad_to_mask_clearance 0) (allow_soldermask_bridges_in_footprints no))
"""
GAPPED = ((-1.3, 1.0), (0.0, 1.0), (1.3, 1.0))   # (x offset, width) of pads 1..3, height 1.5
OVERLAP = ((-0.4, 1.0), (0.4, 1.0), (2.0, 1.0))  # pads 1 and 2 overlap in x -0.1..0.1
CLOSE = ((-0.55, 1.0), (0.55, 1.0), (2.5, 1.0))  # tie pads 0.1 mm apart (< 0.2 clearance)
BRIDGE = (-0.9, -0.4, -0.3, 0.3)
LONG_BRIDGE = (-0.9, -0.4, -1.5, 0.3)

# case: (description, pads, bridge, track (x0, y0, x1, y1, net letter, width), anchored, expect flag)
CASES = {
    1: ("dcsgone JP1: B from pad-2 centre across the bridge into pad 1", GAPPED, BRIDGE,
        (0, 0, -1.6, 0, "B", 0.2), False, True),
    7: ("untied C from pad-3 centre into pad 2", GAPPED, BRIDGE, (1.3, 0, 0.3, 0, "C", 0.2), False, True),
    8: ("B 0.1 mm from pad 1 (clearance), outside pad 2", GAPPED, BRIDGE,
        (-3, 0.95, 0, 0.95, "B", 0.2), False, True),
    20: ("B from the far side to pad-1 centre", GAPPED, BRIDGE, (-4, 0, -1.3, 0, "B", 0.2), True, True),
    21: ("B from below into pad-1 centre", GAPPED, BRIDGE, (-1.3, 4, -1.3, 0, "B", 0.2), True, True),
    22: ("B from the far side touching pad-1 edge", GAPPED, BRIDGE, (-4, 0, -1.6, 0, "B", 0.2), True, True),
    23: ("overlapping tie pads: B touches pad 1 from the far side", OVERLAP, None,
         (-4, 0.3, -0.85, 0.3, "B", 0.2), True, True),
    24: ("overlapping tie pads: B ends inside pad 2 in the overlap", OVERLAP, None,
         (0.05, -4, 0.05, 0, "B", 0.2), True, False),
    25: ("A touches the (long) bridge away from both pads", GAPPED, LONG_BRIDGE,
         (-0.65, -4, -0.65, -1.3, "A", 0.1), True, False),
    26: ("untied C touches the (long) bridge", GAPPED, LONG_BRIDGE,
         (-0.65, -4, -0.65, -1.3, "C", 0.1), True, True),
    27: ("A ends in pad 1 overlapping the bridge inside pad 1", GAPPED, BRIDGE,
         (-4, 0, -0.9, 0, "A", 0.2), True, False),
    31: ("tie pads 0.1 apart: B inside pad 2, copper 0.05 from pad 1", CLOSE, None,
         (0.10, -4, 0.10, 0, "B", 0.2), True, True),
    32: ("tie pads 0.1 apart: B copper edge on pad-2 edge, 0.1 from pad 1", CLOSE, None,
         (0.15, -4, 0.15, 0, "B", 0.2), True, True),
    33: ("tie pads 0.1 apart: B 0.3 from pad 1 (clean)", CLOSE, None,
         (0.35, -4, 0.35, 0, "B", 0.2), True, False),
    35: ("overlapping tie pads: B inside pad 2 only, 0.1 from pad 1", OVERLAP, None,
         (0.3, -4, 0.3, 0, "B", 0.2), True, False),
}


def u() -> str:
    return str(uuid.uuid4())


def footprint(ref, x, y, nets, pads, bridge, ties=True) -> str:
    lines = [f'\t(footprint "Test:{"NetTie3" if ties else "TP"}" (layer "F.Cu") (uuid "{u()}") (at {x} {y})',
             f'\t\t(property "Reference" "{ref}" (at 0 -2.5 0) (layer "F.Fab") (uuid "{u()}") '
             f'(effects (font (size 0.5 0.5) (thickness 0.1))))',
             f'\t\t(property "Value" "V" (at 0 2.5 0) (layer "F.Fab") (uuid "{u()}") '
             f'(effects (font (size 0.5 0.5) (thickness 0.1))))',
             "\t\t(attr smd)"]
    if ties:
        lines.append('\t\t(net_tie_pad_groups "1,2")')
    if bridge:
        a, b, y0, y1 = bridge
        lines.append(f'\t\t(fp_poly (pts (xy {a} {y1}) (xy {b} {y1}) (xy {b} {y0}) (xy {a} {y0})) '
                     f'(stroke (width 0) (type solid)) (fill yes) (layer "F.Cu") (uuid "{u()}"))')
    for i, ((px, w), n) in enumerate(zip(pads, nets), start=1):
        h = 1.5 if ties else 0.8
        lines.append(f'\t\t(pad "{i}" smd rect (at {px} 0) (size {w} {h}) (layers "F.Cu" "F.Mask") '
                     f'(net "{n}") (uuid "{u()}"))')
    return "\n".join(lines) + "\n\t)\n"


def board() -> str:
    body = []
    for k, (_, pads, bridge, (x0, y0, x1, y1, n, w), anchored, _) in CASES.items():
        x, y = 10 * k, 50
        body.append(footprint(f"JP{k}", x, y, [f"A{k}", f"B{k}", f"C{k}"], pads, bridge))
        body.append(f'\t(segment (start {x + x0} {y + y0}) (end {x + x1} {y + y1}) (width {w}) '
                    f'(layer "F.Cu") (net "{n}{k}") (uuid "{u()}"))\n')
        if anchored:
            body.append(footprint(f"TP{k}", x + x0, y + y0, [f"{n}{k}"], ((0.0, 0.8),), None, ties=False))
    edge = (f'\t(gr_rect (start 0 40) (end {10 * max(CASES) + 10} 60) (stroke (width 0.1) (type default)) '
            f'(fill no) (layer "Edge.Cuts") (uuid "{u()}"))\n')
    return HEAD + "".join(body) + edge + "\t(embedded_fonts no)\n)\n"


ROUTING = {"shorting_items", "clearance"}


def main(out: Path) -> int:
    out.mkdir(parents=True, exist_ok=True)
    pcb, rep = out / "net_tie_probe.kicad_pcb", out / "net_tie_probe-drc.json"
    pcb.write_text(board())
    subprocess.run(["kicad-cli", "pcb", "drc", "--format", "json", "--severity-all",
                    "--output", str(rep), str(pcb)], check=True, capture_output=True)
    flagged: dict[int, list[str]] = {}
    for v in json.loads(rep.read_text())["violations"]:
        if v["type"] not in ROUTING:
            continue
        desc = " || ".join(i["description"] for i in v["items"])
        for k in {int(m) for m in re.findall(r"of JP(\d+)", desc)}:
            flagged.setdefault(k, []).append(f"{v['type']}: {desc}")
    bad = 0
    for k, (d, *_, expect) in CASES.items():
        got = k in flagged
        bad += got != expect
        print(f"case {k:2d} {'FLAG' if got else 'ok  '} {'' if got == expect else 'UNEXPECTED '}{d}")
        for line in flagged.get(k, []):
            print(f"         {line}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(Path(sys.argv[1])))
