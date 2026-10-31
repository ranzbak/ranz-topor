"""Synthetic routing model for the router scaling test (no pcbnew needed)."""
from __future__ import annotations

import math
import random

MM = 1_000_000
PITCH = 3 * MM
PAD_D = 1 * MM
MARGIN = 2 * MM
REACH = 9 * MM


def _pair_cells(n_pairs: int, seed: int, reach: float) -> tuple[list[tuple[int, int]], list[tuple[int, int]]]:
    """Seeded shuffle of grid cells; each cell pairs with a random free cell within `reach`.

    A cell with no free partner in reach is skipped (it gets no pad). Rows are added until
    `n_pairs` pairs exist. Returns (cells, pairs as index pairs into cells); the unpaired cells are
    the skipped ones.
    """
    cols = math.ceil(math.sqrt(2 * n_pairs))
    rows = cols
    while True:
        cells = [(i % cols, i // cols) for i in range(cols * rows)]
        rng = random.Random(seed)
        order = list(range(len(cells)))
        rng.shuffle(order)
        free = set(order)
        pairs: list[tuple[int, int]] = []
        for i in order:
            if len(pairs) == n_pairs:
                break
            if i not in free:
                continue
            free.discard(i)
            near = sorted(
                j for j in free
                if math.hypot(cells[i][0] - cells[j][0], cells[i][1] - cells[j][1]) * PITCH <= reach
            )
            if not near:
                continue
            j = near[rng.randrange(len(near))]
            free.discard(j)
            pairs.append((i, j))
        if len(pairs) == n_pairs:
            return cells, pairs
        rows += 1


def synth_model(n_pairs: int, seed: int, jitter_nm: int = 0) -> dict:
    """`n_pairs` two-pad nets, round pads d 1.0 mm on a 3 mm pitch grid, every pair within 9 mm (REACH).

    Cells that cannot be paired within reach are skipped; rows are added until exactly `n_pairs`
    pairs exist. The outline is the pad bbox plus the 2 mm margin. `jitter_nm=0` (default) is the
    exact collinear grid. `jitter_nm=J>0` offsets each pad centre by a seeded integer in [-J, J] per
    axis; pairing then uses a reach shortened by 2*sqrt(2)*J so every pair stays within REACH.
    """
    reach = REACH - math.ceil(2 * math.sqrt(2) * jitter_nm) if jitter_nm else REACH
    cells, pairs = _pair_cells(n_pairs, seed, reach)
    jit = random.Random(f"jitter:{seed}:{n_pairs}") if jitter_nm else None

    def centre(c):
        x, y = cells[c][0] * PITCH, cells[c][1] * PITCH
        if jit is not None:
            x += jit.randint(-jitter_nm, jitter_nm)
            y += jit.randint(-jitter_nm, jitter_nm)
        return [x, y]

    pads = []
    for net, (i, j) in enumerate(pairs, start=1):
        for k, c in enumerate((i, j), start=1):
            pads.append({"uuid": f"p{net}-{k}", "ref": f"P{net}", "number": str(k), "net": net,
                         "attr": "smd", "center": centre(c), "rotation_deg": 0.0, "layers": [0],
                         "shapes": {"0": {"kind": "circle", "d": PAD_D}},
                         "drill": None, "local_clearance": None, "net_tie_group": None})
    xs = [p["center"][0] for p in pads]
    ys = [p["center"][1] for p in pads]
    lo_x, hi_x = min(xs) - PAD_D // 2 - MARGIN, max(xs) + PAD_D // 2 + MARGIN
    lo_y, hi_y = min(ys) - PAD_D // 2 - MARGIN, max(ys) + PAD_D // 2 + MARGIN
    corners = [(lo_x, lo_y), (hi_x, lo_y), (hi_x, hi_y), (lo_x, hi_y)]
    outline = [{"kind": "seg", "a": list(corners[k]), "b": list(corners[(k + 1) % 4])} for k in range(4)]
    rule = {"clearance": 200_000, "width": 250_000, "via_d": 600_000, "via_drill": 300_000,
            "netclass": "Default", "width_source": "default"}
    return {
        "units": "nm", "board": f"synth-{n_pairs}-{seed}" + (f"-j{jitter_nm}" if jitter_nm else ""),
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
