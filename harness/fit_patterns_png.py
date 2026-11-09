"""PNG of a fit pattern (harness/fit_patterns.py): problem left, reference (or the marked impossibility) right, the
critical cuts drawn with their capacity / demand / slack. Drawn with matplotlib from the pattern's own geometry
(the same numbers the board files are written from); axes in mm, KiCad frame (y down)."""
from __future__ import annotations

import math
import textwrap
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.patches import Circle, Polygon, Rectangle  # noqa: E402
from shapely.geometry import LineString  # noqa: E402

NET_COLORS = ["#1f77b4", "#2ca02c", "#9467bd", "#ff7f0e", "#17becf", "#8c564b", "#e377c2", "#bcbd22"]
LAYER_STYLE = {"F.Cu": dict(alpha=0.85), "In1.Cu": dict(alpha=0.55, hatch="////"), "B.Cu": dict(alpha=0.5)}
OK, BAD, CTX, EMPTY = "#138a36", "#d62728", "#1f5fbf", "#777777"

# Label anchor offsets (mm, from the cut's midpoint) where the default (above) would sit on copper.
LABEL_AT = {
    ("channel_fit", "C1"): (3.3, -2.2), ("channel_overfull", "C1"): (3.3, -2.2),
    ("neck_fit", "N1"): (0.0, 2.5), ("neck_too_narrow", "N1"): (0.0, -2.0),
    ("model_gap_cut", "AC"): (4.6, 0.0), ("model_gap_cut", "AB"): (-4.2, -2.6),
    ("model_gap_cut", "AD"): (-4.2, 2.6), ("model_gap_cut", "BC"): (4.2, -2.6),
    ("model_gap_cut", "DC"): (4.2, 2.6), ("model_gap_cut", "BD"): (0.0, 4.6),
    ("inner_layer_room", "F1"): (-0.6, -2.9), ("inner_layer_room", "F2"): (0.6, 5.0),
    ("inner_layer_room", "I1"): (0.0, 2.9),
    ("diagonal_cut_norm", "C6"): (3.0, 2.6),
}


def _color(pat, net):
    return NET_COLORS[pat.nets.index(net) % len(NET_COLORS)] if net in pat.nets else "#555555"


def _pads(ax, pat, layers):
    for f in pat.fps:
        if not any(p.drill is not None or p.layer in layers for p in f.pads):
            continue
        for p in f.pads:
            on = p.drill is not None or p.layer in layers
            if not on:
                continue
            col = _color(pat, p.net) if p.net else "#6b6b6b"
            x, y = p.pos
            if p.shape == "circle":
                ax.add_patch(Circle((x, y), p.size[0] / 2, fc=col, ec="k", lw=0.5, alpha=0.75, zorder=2))
            else:
                w, h = p.size
                ax.add_patch(Rectangle((x - w / 2, y - h / 2), w, h, fc=col, ec="k", lw=0.5, alpha=0.75, zorder=2))
            if p.drill:
                ax.add_patch(Circle((x, y), p.drill / 2, fc="white", ec="k", lw=0.4, zorder=3))
        x0 = min(p.pos[0] for p in f.pads)
        y0 = min(p.pos[1] - p.size[1] / 2 for p in f.pads)
        if f.role != "wall" or len(f.pads) < 3:
            ax.text(x0, y0 - 0.12, f.ref, fontsize=7, color="#333333", ha="center", va="bottom", zorder=6, clip_on=True)
        else:
            ax.text(x0, y0 - 0.12, f"{f.ref}.{f.pads[0].num}..{f.pads[-1].num}", fontsize=7, color="#333333",
                    ha="left", va="bottom", zorder=6, clip_on=True)


def _frame(ax, pat, layers, title):
    W, H = pat.size
    ax.add_patch(Rectangle((0, 0), W, H, fc="#fbfbf6", ec="k", lw=1.0, zorder=0))
    for k in pat.keepouts:
        if set(k.layers) & (set(layers) | {"B.Cu", "In2.Cu"}):
            what = "+".join(["tracks"] * k.tracks + ["vias"] * k.vias)
            ax.add_patch(Polygon(k.poly, closed=True, fc="none", ec="#dadada", hatch="..", lw=0.5, zorder=1,
                                 label=f"rule area {'/'.join(k.layers)}: no {what}"))
    _pads(ax, pat, layers)
    ax.set_xlim(-0.4, W + 0.4)
    ax.set_ylim(H + 0.4, -0.4)
    ax.set_aspect("equal")
    ax.grid(True, lw=0.3, color="#dddddd")
    ax.set_xlabel("x (mm)")
    ax.set_ylabel("y (mm)")
    ax.set_title(title, fontsize=11)


def _tracks(ax, pat, layers):
    for t in pat.ref_tracks:
        if t.layer not in layers:
            continue
        poly = LineString(t.pts).buffer(t.width / 2, cap_style="round", join_style="round", quad_segs=16)
        st = LAYER_STYLE.get(t.layer, {})
        ax.add_patch(Polygon(list(poly.exterior.coords), closed=True, fc=_color(pat, t.net), ec="k", lw=0.3,
                             alpha=st.get("alpha", 0.8), hatch=st.get("hatch"), zorder=4))
    for v in pat.ref_vias:
        ax.add_patch(Circle(v.pos, v.dia / 2, fc=_color(pat, v.net), ec="k", lw=0.5, zorder=5))
        ax.add_patch(Circle(v.pos, v.drill / 2, fc="white", ec="k", lw=0.3, zorder=5))


def _ratsnest(ax, pat):
    pos = {f"{f.ref}.{p.num}": p.pos for f in pat.fps for p in f.pads}
    for net, a, b in pat.connections:
        (x0, y0), (x1, y1) = pos[a], pos[b]
        ax.plot([x0, x1], [y0, y1], ls=(0, (2, 2)), lw=0.8, color=_color(pat, net), zorder=3)


def _witness(ax, pat, layers):
    for net, (layer, pts) in pat.witness.items():
        if layer not in layers:
            continue
        xs, ys = zip(*pts)
        ax.plot(xs, ys, ls=(0, (4, 2)), lw=1.2, color=_color(pat, net), zorder=4)
        ax.text(xs[0], ys[0] + 0.55, net, fontsize=7, color=_color(pat, net), ha="center", zorder=6, clip_on=True)


def _cut_color(c, v, octi):
    if c.role == "empty":
        return EMPTY
    if c.role == "context":
        return CTX
    s = min(v["slack_e"], v["slack_o"]) if octi else v["slack_e"]
    if "slack_floor" in v:
        s = v["slack_floor"]
    return OK if s >= -1e-9 else BAD


def _label(pat, c, v, doc_cut):
    lines = [f"{c.id} ({c.layer})  {c.fp[0]}.{c.fp[1]} - {c.fq[0]}.{c.fq[1]}"]
    if abs(v["len_e"] - v["len_o"]) > 1e-6:
        lines.append(f"capacity {v['len_e']:.4f} Euclid / {v['len_o']:.4f} octagonal")
    else:
        lines.append(f"capacity {v['len_e']:.4f} (both norms)")
    k = len(c.crossing)
    if k == 0:
        lines.append("no wire crosses (empty cut)")
        return "\n".join(lines)
    lines.append(f"demand {k} w + {k + 1} s = {v['need']:.4f}")
    se, so = v["slack_e"], v["slack_o"]
    word = lambda s: f"slack {s:+.4f}" if s >= -1e-9 else f"OVER-FULL by {-s:.4f}"  # noqa: E731
    if abs(se - so) > 1e-6:
        lines.append(f"{word(se)} Euclid / {word(so)} oct")
    else:
        lines.append(word(se))
    if "need_floor" in v:
        lines.append(f"at floor {pat.extra['neck']['floor']:.2f}: demand {v['need_floor']:.2f}, "
                     f"{word(v['slack_floor'])}")
    return "\n".join(lines)


def _cuts(ax, pat, doc, layers, *, labels: bool, octi: bool):
    from harness.fit_patterns import cut_values

    for c in pat.cuts:
        if c.layer not in layers:
            continue
        v = cut_values(pat, c)
        col = _cut_color(c, v, octi)
        lw = 2.4 if c.role == "binding" else 1.4
        ax.plot([c.p[0], c.q[0]], [c.p[1], c.q[1]], color=col, lw=lw, zorder=7, solid_capstyle="butt")
        ax.plot([c.p[0], c.q[0]], [c.p[1], c.q[1]], "o", color=col, ms=3.5, zorder=7)
        mx, my = (c.p[0] + c.q[0]) / 2, (c.p[1] + c.q[1]) / 2
        if not labels:
            ax.text(mx, my - 0.18, c.id, fontsize=8, color=col, ha="center", va="bottom", zorder=8,
                    bbox=dict(fc="white", ec="none", alpha=0.7, pad=0.5))
            continue
        dc = next(d for d in doc["cuts"] if d["id"] == c.id)
        ox, oy = LABEL_AT.get((pat.name, c.id), (0.0, -1.6))
        ax.annotate(_label(pat, c, v, dc), xy=(mx, my), xytext=(mx + ox, my + oy), fontsize=7.5, ha="center",
                    va="center", color=col, zorder=9,
                    bbox=dict(fc="white", ec=col, lw=0.8, alpha=0.92, boxstyle="round,pad=0.3"),
                    arrowprops=dict(arrowstyle="-", color=col, lw=0.8))
        if c.role == "binding" and col == BAD:
            r = max(0.35, 0.6 * max(abs(c.q[0] - c.p[0]), abs(c.q[1] - c.p[1])))
            ax.add_patch(Circle((mx, my), r, fc="none", ec=BAD, lw=1.6, ls="--", zorder=8))


def _neck_note(ax, pat):
    """Dimension arrow of the reference neck (left of the wire) with its width and length."""
    neck = pat.extra.get("neck")
    if not neck or "reference" not in neck:
        return
    ref = neck["reference"]
    (x0, y0), (_, y1) = ref["from"], ref["to"]
    xa = x0 - 0.75
    ax.annotate("", xy=(xa, y0), xytext=(xa, y1), arrowprops=dict(arrowstyle="<->", color="#333333", lw=0.8),
                zorder=9)
    ax.text(xa - 0.08, (y0 + y1) / 2 - 0.6, f"neck {ref['width']:.2f}\nlength {ref['length']:.2f}", fontsize=7.5,
            ha="right", va="center", zorder=9, bbox=dict(fc="white", ec="#333333", lw=0.5, alpha=0.9, pad=0.3))


def neck_title(pat):
    neck = pat.extra.get("neck")
    if not neck:
        return ""
    if "reference" in neck:
        return (f"\nneck {neck['max_neck_width']:.2f} (floor {neck['floor']:.2f}) from U2.1 to "
                f"{neck['full_width_clear_from_pin_line']:.4f} past the pins: {neck['neck_length_from_pad_centre']:.4f}")
    return f"\nfloor {neck['floor']:.2f}: the floor alone needs {neck['floor'] + 2 * 0.2:.2f} > 0.84"


def _gaps(pat, c, layer_tracks):
    """Copper-free stretches along the cut p -> q: [(t0, t1)] in mm from p, with the intervals of the wires."""
    from shapely.geometry import LineString as LS

    L = math.dist(c.p, c.q)
    seg = LS([c.p, c.q])
    iv = []
    for width, pts in layer_tracks:
        g = LS(pts).buffer(width / 2, cap_style="round", join_style="round", quad_segs=32).intersection(seg)
        for part in getattr(g, "geoms", [g]):
            if part.is_empty or part.length <= 0:
                continue
            ts = sorted(math.dist(c.p, xy) for xy in part.coords)
            iv.append((ts[0], ts[-1]))
    iv.sort()
    gaps, cur = [], 0.0
    for a, b in iv:
        gaps.append((cur, a))
        cur = max(cur, b)
    gaps.append((cur, L))
    return gaps, iv


def _detail(ax, pat, doc, layers, octi):
    """Zoom on the view's binding cuts: copper (reference, or the planned wires at their width when unsolvable)
    and every copper-free stretch along the cut, labelled with its length (red when below the 0.20 clearance)."""
    cuts = [c for c in pat.cuts if c.layer in layers and c.role == "binding"] or \
        [c for c in pat.cuts if c.layer in layers]
    xs = [x for c in cuts for x in (c.p[0], c.q[0])]
    ys = [y for c in cuts for y in (c.p[1], c.q[1])]
    m = 0.9
    x0, x1, y0, y1 = min(xs) - m, max(xs) + m, min(ys) - m, max(ys) + m
    _frame(ax, pat, layers, "")
    if pat.solvable:
        _tracks(ax, pat, layers)
        lt = [(t.width, t.pts) for t in pat.ref_tracks if t.layer in layers]
        what = "reference"
    else:
        lt = []
        for net, (layer, pts) in pat.witness.items():
            if layer not in layers:
                continue
            w = class_w(pat, net)
            if pat.extra.get("neck"):
                w = min(w, pat.extra["neck"]["floor"])
            poly = LineString(pts).buffer(w / 2, cap_style="round", join_style="round", quad_segs=16)
            ax.add_patch(Polygon(list(poly.exterior.coords), closed=True, fc=_color(pat, net), ec="k", lw=0.3,
                                 alpha=0.45, zorder=4))
            lt.append((w, pts))
        what = ("planned wire drawn at the neck floor" if pat.extra.get("neck") else
                "planned wires drawn at full width")
    from harness.fit_patterns import cut_values

    for c in cuts:
        v = cut_values(pat, c)
        col = _cut_color(c, v, octi)
        ax.plot([c.p[0], c.q[0]], [c.p[1], c.q[1]], color=col, lw=1.2, zorder=7)
        gaps, _ = _gaps(pat, c, lt)
        ux, uy = (c.q[0] - c.p[0]) / math.dist(c.p, c.q), (c.q[1] - c.p[1]) / math.dist(c.p, c.q)
        for i, (a, b) in enumerate(gaps):
            g = b - a
            mx, my = c.p[0] + ux * (a + b) / 2, c.p[1] + uy * (a + b) / 2
            off = 0.22 if i % 2 == 0 else -0.22
            tx, ty = mx - uy * off, my + ux * off
            bad = g < 0.2 - 1e-6
            ax.annotate(f"{g:.4f}".rstrip("0").rstrip(".") if g > 0 else f"{g:.3f}", xy=(mx, my), xytext=(tx, ty),
                        fontsize=8, ha="center", va="center", clip_on=True, color=BAD if bad else "#222222", zorder=9,
                        bbox=dict(fc="white", ec="none", alpha=0.8, pad=0.4),
                        arrowprops=dict(arrowstyle="-", color="#888888", lw=0.5))
    if octi:
        for c in cuts:
            yb = c.p[1] + 0.55
            ax.annotate("", xy=(c.p[0], yb), xytext=(c.q[0], yb),
                        arrowprops=dict(arrowstyle="<->", color=BAD, lw=0.8), zorder=9)
            for x in (c.p[0], c.q[0]):
                ax.plot([x, x], [min(c.p[1], c.q[1]), yb + 0.1], ls=":", color=BAD, lw=0.7, zorder=8)
            ax.text((c.p[0] + c.q[0]) / 2, yb + 0.08, f"dx = {abs(c.q[0] - c.p[0]):.2f} = octagonal capacity:\n"
                    "the room of 90 deg wires (need 1.55)", fontsize=7.5, color=BAD, ha="center", va="top",
                    zorder=9, bbox=dict(fc="white", ec="none", alpha=0.85, pad=0.3))
    ax.set_xlim(x0, x1)
    ax.set_ylim(y1, y0)
    ax.set_title(f"detail {'/'.join(layers)}: {what};\ncopper-free lengths along the cut (mm)" + neck_title(pat), fontsize=9)


def class_w(pat, net):
    from harness.fit_patterns import class_of

    return class_of(pat, net)["track_width"]


def render(pat, doc: dict, out: Path) -> Path:
    out = Path(out)
    W, H = pat.size
    views = pat.views
    ph = 7.0
    pw = max(4.2, ph * (W + 0.8) / (H + 0.8))
    dw = 6.5
    fig = plt.figure(figsize=(2 * pw + dw + 0.8, ph + 2.2))
    gs = fig.add_gridspec(len(views), 3, width_ratios=[pw, pw, dw])
    ax_p = fig.add_subplot(gs[:, 0])
    ax_r = fig.add_subplot(gs[:, 1])
    all_layers = sorted({ly for v in views for ly in v} | {"F.Cu"})
    octi = pat.name == "diagonal_cut_norm"
    _frame(ax_p, pat, all_layers, "problem: pads, rule areas, ratsnest; critical cuts")
    _ratsnest(ax_p, pat)
    _cuts(ax_p, pat, doc, all_layers, labels=False, octi=False)
    lname = "/".join(all_layers)
    if pat.solvable:
        _frame(ax_r, pat, all_layers, f"reference copper ({lname}), KiCad DRC 0 / 0")
        _tracks(ax_r, pat, all_layers)
        _neck_note(ax_r, pat)
    else:
        _frame(ax_r, pat, all_layers, "planned topology (dashed centre lines), over-full cut ringed")
        _witness(ax_r, pat, all_layers)
    _cuts(ax_r, pat, doc, all_layers, labels=True, octi=octi)
    for i, layers in enumerate(views):
        _detail(fig.add_subplot(gs[i, 2]), pat, doc, layers, octi)
    handles, labs = ax_p.get_legend_handles_labels()
    if handles:
        ax_p.legend(handles[:2], labs[:2], loc="lower left", fontsize=7, framealpha=0.9)
    fig.suptitle(f"{pat.name}: {pat.title}   [{pat.kind}]", fontsize=13)
    txt = "Expected: " + pat.answer
    fig.text(0.01, 0.005, "\n".join(textwrap.wrap(txt, width=int(15.5 * (2 * pw + dw + 0.8)))), fontsize=8.5,
             ha="left", va="bottom")
    fig.tight_layout(rect=(0, 0.075, 1, 0.97))
    out.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out, dpi=150)
    plt.close(fig)
    return out
