"""Synthetic routing model for the router scaling test (no pcbnew needed)."""
from __future__ import annotations

import math
import random

MM = 1_000_000
PITCH = 3 * MM
PAD_D = 1 * MM
MARGIN = 2 * MM
REACH = 9 * MM


def synth_model(n_pairs: int, seed: int) -> dict:
    """`n_pairs` two-pad nets on a ceil(sqrt(2n)) grid at 3 mm pitch, short seeded pairings (<= 9 mm)."""
    cols = math.ceil(math.sqrt(2 * n_pairs))
    cells = [(i % cols, i // cols) for i in range(2 * n_pairs)]
    rng = random.Random(seed)

    def dist(a, b):
        return math.hypot(a[0] - b[0], a[1] - b[1]) * PITCH

    order = list(range(len(cells)))
    rng.shuffle(order)
    free = set(order)
    pairs = []
    for i in order:
        if i not in free:
            continue
        free.discard(i)
        near = sorted(j for j in free if dist(cells[i], cells[j]) <= REACH)
        if not near:  # leftover cell with no partner in reach: nearest free cell
            near = sorted(free, key=lambda j: (dist(cells[i], cells[j]), j))[:1]
        j = near[rng.randrange(len(near))]
        free.discard(j)
        pairs.append((i, j))

    pads = []
    for net, (i, j) in enumerate(pairs, start=1):
        for k, c in enumerate((i, j), start=1):
            x, y = cells[c][0] * PITCH, cells[c][1] * PITCH
            pads.append({"uuid": f"p{net}-{k}", "ref": f"P{net}", "number": str(k), "net": net,
                         "attr": "smd", "center": [x, y], "rotation_deg": 0.0, "layers": [0],
                         "shapes": {"0": {"kind": "circle", "d": PAD_D}},
                         "drill": None, "local_clearance": None, "net_tie_group": None})
    rows = max(c[1] for c in cells) + 1
    lo, hi = -MARGIN - PAD_D // 2, ((cols - 1) * PITCH + MARGIN + PAD_D // 2, (rows - 1) * PITCH + MARGIN + PAD_D // 2)
    corners = [(lo, lo), (hi[0], lo), (hi[0], hi[1]), (lo, hi[1])]
    outline = [{"kind": "seg", "a": list(corners[k]), "b": list(corners[(k + 1) % 4])} for k in range(4)]
    rule = {"clearance": 200_000, "width": 250_000, "via_d": 600_000, "via_drill": 300_000,
            "netclass": "Default", "width_source": "default"}
    return {
        "units": "nm", "board": f"synth-{n_pairs}-{seed}",
        "layers": [{"id": 0, "name": "F.Cu", "kind": "signal", "routable": True}],
        "nets": [{"code": n, "name": f"N{n}"} for n in range(1, len(pairs) + 1)],
        "pads": pads, "vias": [], "kept": [], "copper_shapes": [], "holes": [],
        "outline": outline, "keepouts": [], "net_ties": [], "dangling_ends": [],
        "rules": {"min_clearance": 200_000, "min_track_width": 200_000, "min_via_diameter": 600_000,
                  "min_via_drill": 300_000, "min_annular": 200_000, "hole_clearance": 350_000,
                  "hole_to_hole": 250_000, "edge_clearance": 250_000, "max_error": 1500,
                  "profile": "jlcpcb_1oz", "drill_list": [300_000], "routing_vias": [300_000],
                  "default": rule, "per_net": {}},
    }
