"""PNG of an M3 radii pattern (harness/radii_patterns.py): problem left (pads, rule area, ratsnest), reference right
(the expected copper, KiCad DRC 0 / 0) with the radii and the order the pattern tests marked. Drawn with matplotlib
from the pattern's own geometry; axes in mm, KiCad frame (y down)."""
from __future__ import annotations

import math
import textwrap
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.patches import Polygon  # noqa: E402

from harness.fit_patterns_png import _color, _frame, _ratsnest, _tracks  # noqa: E402

MARK = "#c0392b"


def _octagon(c, rho):
    """Octagon of in-radius rho about c, sides facing 0/45/90... degrees (vertices at 22.5 + k 45)."""
    R = rho / math.cos(math.radians(22.5))
    return [(c[0] + R * math.cos(math.radians(22.5 + 45 * k)), c[1] + R * math.sin(math.radians(22.5 + 45 * k)))
            for k in range(8)]


def _labels(ax, pat):
    for i, t in enumerate(pat.ref_tracks):
        (x0, y0) = t.pts[0]
        ax.text(x0 - 0.15, y0 - 0.45 - 0.3 * i, f"{t.net} {t.width:.2f}", fontsize=7, color=_color(pat, t.net), ha="right",
                zorder=7)


def _three_diagonals(ax, pat):
    m = pat.extra["m3"]
    y = 8.0
    xs = []
    for t in pat.ref_tracks:
        for a, b in zip(t.pts, t.pts[1:]):
            if min(a[1], b[1]) <= y <= max(a[1], b[1]) and a[1] != b[1]:
                xs.append(a[0] + (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]))
    ax.annotate("", xy=(max(xs) + 0.25, y + 0.25), xytext=(min(xs) - 0.25, y - 0.25),
                arrowprops=dict(arrowstyle="-", color=MARK, lw=0.8, ls="--"))
    ax.text(max(xs) + 0.4, y + 0.3, f"order P1, S, P2\ndiagonal pitch {m['numbers']['diagonal_pitch_perpendicular']:.3f}"
            f"\n(need {m['numbers']['needed_P_to_S']:.3f})", fontsize=7.5, color=MARK, zorder=8)
    ax.text(0.3, 13.6, "draw order today: P1, P2 (0.40) first, S (0.25) last", fontsize=7.5, color=MARK)


def _nested_wraps(ax, pat):
    c, rho = pat.extra["corner"], pat.extra["rho"]
    for n in ("C", "B", "A"):
        oc = _octagon(c, rho[n])
        ax.add_patch(Polygon(oc, closed=True, fc="none", ec=_color(pat, n), lw=0.7, ls="--", zorder=6))
    ax.plot([c[0]], [c[1]], "o", color=MARK, ms=3, zorder=8)
    ax.text(c[0] + 1.3, c[1] - 3.6, "stack at M1's corner,\ninside-out C, B, A\nradii " +
            " / ".join(f"{rho[n]:.3f}" for n in ("C", "B", "A")), fontsize=7.5, color=MARK, zorder=8)
    ax.text(0.3, 0.8, "net ids A < B < C; draw order today: B, A (0.40), C (0.25) last", fontsize=7.5, color=MARK)


def _distant_radius(ax, pat):
    A, rho, ys = pat.extra["A"], pat.extra["rho"], pat.extra["ys"]
    for n in ("W1", "W2", "W3"):
        ax.add_patch(Polygon(_octagon(A, rho[n]), closed=True, fc="none", ec=_color(pat, n), lw=0.7, ls="--",
                             zorder=6))
    ax.plot([A[0], A[0]], [A[1] - 0.4, 0.5], color=MARK, lw=0.8, ls="--", zorder=6)
    ax.text(A[0] + 0.15, 1.2, f"A's cut: W1, W2, W3\nradius for W3 = {rho['W3']:.3f}", fontsize=7.5, color=MARK, zorder=8)
    ax.plot([1.0, 15.0], [ys["W3"], ys["W3"]], color=_color(pat, "W3"), lw=0.6, ls=(0, (1, 2)), zorder=6)
    ax.text(10.2, ys["W3"] - 0.2, "W3's zero-width band (touches nothing)", fontsize=7, color=_color(pat, "W3"))
    for n in ("W1", "W2"):
        ax.plot([1.0, 15.0], [ys[n], ys[n]], color=_color(pat, n), lw=0.5, ls=(0, (1, 3)), zorder=5)
    ax.text(A[0], A[1] + 0.75, "A", fontsize=9, ha="center", color="#333333", zorder=8)
    ax.text(0.3, 10.6, "dotted: straight lines J1-J2; only W1 runs into A", fontsize=7.5, color="#555555")
    ax.text(0.3, 11.2, "draw order today: W3 (shortest) first", fontsize=7.5, color=MARK)


MARKS = {"three_diagonals": _three_diagonals, "nested_wraps": _nested_wraps, "distant_radius": _distant_radius}


def render(pat, doc: dict, out: Path) -> Path:
    out = Path(out)
    W, H = pat.size
    ph = 7.0
    pw = max(4.2, ph * (W + 0.8) / (H + 0.8))
    fig = plt.figure(figsize=(2 * pw + 0.8, ph + 2.4))
    ax_p = fig.add_subplot(1, 2, 1)
    ax_r = fig.add_subplot(1, 2, 2)
    layers = ["F.Cu"]
    _frame(ax_p, pat, layers, "problem: pads, rule area (B.Cu closed), ratsnest")
    _ratsnest(ax_p, pat)
    drc = doc.get("reference_drc")
    drc_txt = f"KiCad DRC {drc['violations']} / {drc['unconnected']}" if drc else "KiCad DRC not run"
    _frame(ax_r, pat, layers, f"expected copper (F.Cu), {drc_txt}")
    _tracks(ax_r, pat, layers)
    _labels(ax_r, pat)
    MARKS[pat.name](ax_r, pat)
    from harness.radii_patterns import STATUS
    fig.suptitle(f"{pat.name}: {pat.title}   [{STATUS.get(pat.name, 'PROPOSED')}]", fontsize=13)
    fig.text(0.01, 0.005, "\n".join(textwrap.wrap("Expected: " + pat.answer, width=int(16 * (2 * pw + 0.8)))),
             fontsize=8.5, ha="left", va="bottom")
    fig.tight_layout(rect=(0, 0.09, 1, 0.97))
    out.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out, dpi=150)
    plt.close(fig)
    return out
