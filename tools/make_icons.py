"""Draw the plugin's toolbar icons (plugin/icons/topor-{light,dark}-{24,64}.png).

A smooth copper trace between two round pads, the router's signature output. Drawn at 8x and
downsampled for anti-aliasing. Run: python3 tools/make_icons.py (needs Pillow).
"""
from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parents[1] / "plugin" / "icons"
COPPER = (200, 117, 51, 255)
PAD = {"light": (40, 40, 40, 255), "dark": (225, 225, 225, 255)}
SS = 8  # supersampling factor


def bezier(p0, p1, p2, p3, n=64):
    pts = []
    for i in range(n + 1):
        t = i / n
        u = 1 - t
        pts.append((u**3 * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t**3 * p3[0],
                    u**3 * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t**3 * p3[1]))
    return pts


def draw(size: int, theme: str) -> Image.Image:
    s = size * SS
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    k = s / 24  # design grid is 24x24
    a, b = (5 * k, 18 * k), (19 * k, 6 * k)
    pts = bezier(a, (5 * k, 9 * k), (19 * k, 15 * k), b)
    w = round(2.6 * k)
    d.line(pts, fill=COPPER, width=w, joint="curve")
    for x, y in pts[:: max(1, len(pts) // 32)]:  # round the polyline joints
        d.ellipse([x - w / 2, y - w / 2, x + w / 2, y + w / 2], fill=COPPER)
    r_out, r_in = 4.2 * k, 1.6 * k
    for x, y in (a, b):
        d.ellipse([x - r_out, y - r_out, x + r_out, y + r_out], fill=PAD[theme])
        d.ellipse([x - r_in, y - r_in, x + r_in, y + r_in], fill=(0, 0, 0, 0))
    return img.resize((size, size), Image.LANCZOS)


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    for theme in ("light", "dark"):
        for size in (24, 64):
            draw(size, theme).save(OUT / f"topor-{theme}-{size}.png")
    print("icons written to", OUT)
