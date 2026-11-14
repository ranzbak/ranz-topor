"""Crop a harness model export to a window (dev helper for the search fixtures, A2).

Usage: python3 crop_model.py MODEL.json X0 Y0 X1 Y1 OUT.json [SOURCE]   (KiCad mm, y down)

Keeps the pads, vias and holes whose centre lies in the window, every net and the rules; replaces
the outline by the window rectangle; drops pours, copper shapes, kept items, keepouts, net ties
and dangling ends (the fixtures test the slot search's pad entry on the empty board, not pours).
SOURCE (upstream URL, commit, licence of the board) is stored as the model's `source` attribution;
crop only public boards (CONTRIBUTING.md "Public and private boards").
"""
import json
import sys

MM = 1_000_000


def main() -> None:
    src, x0, y0, x1, y1, out = sys.argv[1:7]
    x0, y0, x1, y1 = (int(float(v) * MM) for v in (x0, y0, x1, y1))
    m = json.load(open(src))

    def inside(p):
        return x0 <= p[0] <= x1 and y0 <= p[1] <= y1

    m["pads"] = [p for p in m["pads"] if inside(p["center"])]
    m["vias"] = [v for v in m["vias"] if inside(v["at"])]
    m["holes"] = [h for h in m["holes"] if inside(h["at"])]
    corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    m["outline"] = [{"kind": "seg", "a": list(corners[i]), "b": list(corners[(i + 1) % 4])}
                    for i in range(4)]
    for k in ("pours", "copper_shapes", "kept", "keepouts", "net_ties", "dangling_ends"):
        m[k] = []
    m["board"] = f"{m.get('board', 'board')}-crop"
    if len(sys.argv) > 7:
        m["source"] = f"{sys.argv[7]}; harness model export cropped to x {x0 / MM:g}-{x1 / MM:g}, y {y0 / MM:g}-{y1 / MM:g} mm"
    json.dump(m, open(out, "w"), separators=(",", ":"), sort_keys=True)
    print(f"{len(m['pads'])} pads, {len(m['vias'])} vias, {len(m['holes'])} holes -> {out}")


if __name__ == "__main__":
    main()
