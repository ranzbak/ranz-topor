"""IPC-vs-harness model parity (M4-R5). LIVE: needs a running KiCad (KICAD_API_SOCKET) with the snapshot board
open in the PCB editor; skipped otherwise (never started or touched by the test suite). The comparison helpers
are self-tested offline.

Tolerances: every coordinate 0 nm, except
  * pad shapes: polygon-equivalent within 1 um (a native shape on one side vs a polygon on the other is
    compared as polygons; same native kind -> exact numbers, roundrect radius within 1 um);
  * copper_shapes (stroked graphics are polygonised by two different engines): same uuid/layer/net, bounding
    box within 2 * max_error;
  * pours: same uuid/net/layers/settings and reached pads/vias; fills compared by area (0.5 %), because KiCad
    refills them on each side;
  * via ``free`` is approximated on the IPC side and not compared.
Net codes differ between the sides (pcbnew's vs sorted-name), so everything is compared by net NAME."""
import math
import os

import pytest

from toporlib import geom

MAX_ERR_SLACK = 2  # x max_error

PAD_TOL = 1000  # nm


# ---------------------------------------------------------------- canonicalisation / comparison
def _names(m):
    return {n["code"]: n["name"] for n in m["nets"]} | {0: ""}


def _pad_polygon(pad, layer):
    sh = pad["shapes"][str(layer)]
    cx, cy = pad["center"]
    th = math.radians(pad["rotation_deg"])
    c, s = math.cos(th), math.sin(th)

    def place(pts):
        return [[cx + x * c + y * s, cy - x * s + y * c] for x, y in pts]

    k = sh["kind"]
    if k == "poly":
        return [list(map(float, p)) for p in sh["pts"]]
    if k == "circle":
        return place(geom.circle_ring((0, 0), sh["d"] // 2, 50))
    w, h = sh["w"] / 2.0, sh["h"] / 2.0
    if k == "rect":
        return place([(-w, -h), (w, -h), (w, h), (-w, h)])
    r = min(w, h) if k == "oval" else min(sh["rc"], w, h)
    pts = []
    for (qx, qy, a0) in ((w - r, h - r, 0), (-(w - r), h - r, 90), (-(w - r), -(h - r), 180), (w - r, -(h - r), 270)):
        for i in range(0, 91, 3):
            a = math.radians(a0 + i)
            pts.append((qx + r * math.cos(a), qy + r * math.sin(a)))
    return place(pts)


def _dist_to_ring(p, ring):
    return min(geom.dist_pt_seg(p, ring[i], ring[(i + 1) % len(ring)]) for i in range(len(ring)))


def polygons_equivalent(a, b, tol=PAD_TOL):
    """Every vertex of each polygon lies within `tol` of the other polygon's boundary or inside it."""
    def within(p, ring):
        return geom.point_in_ring(p, ring) or _dist_to_ring(p, ring) <= tol

    def dense(ring):  # vertices plus edge midpoints
        return ring + [[(ring[i][0] + ring[(i + 1) % len(ring)][0]) / 2, (ring[i][1] + ring[(i + 1) % len(ring)][1]) / 2]
                       for i in range(len(ring))]

    return all(_dist_to_ring(p, b) <= tol for p in dense(a) if not geom.point_in_ring(p, b)) and \
        all(_dist_to_ring(p, a) <= tol for p in dense(b) if not geom.point_in_ring(p, a)) and \
        all(within(p, b) for p in a) and all(within(p, a) for p in b)


def _shape_equal(pa, a, pb, b, layer):
    if a["kind"] == b["kind"] and a["kind"] != "poly":
        if a["kind"] == "roundrect":
            return {k: v for k, v in a.items() if k != "rc"} == {k: v for k, v in b.items() if k != "rc"} \
                and abs(a["rc"] - b["rc"]) <= PAD_TOL
        return a == b
    return polygons_equivalent(_pad_polygon(pa, layer), _pad_polygon(pb, layer))


def _area(rings):
    tot = 0.0
    for i, r in enumerate(rings):
        a = abs(sum(r[j][0] * r[(j + 1) % len(r)][1] - r[(j + 1) % len(r)][0] * r[j][1] for j in range(len(r)))) / 2
        tot += a if i == 0 else -a
    return tot


def compare_models(h: dict, i: dict) -> list[str]:
    """Differences between the harness model `h` and the IPC model `i` (empty = parity)."""
    bad: list[str] = []
    nh, ni = _names(h), _names(i)
    if sorted(n["name"] for n in h["nets"]) != sorted(n["name"] for n in i["nets"]):
        bad.append("nets differ")
    pick = lambda ls, ks: [{k: x[k] for k in ks} for x in ls]  # noqa: E731
    if pick(h["layers"], ("id", "name", "kind", "routable")) != pick(i["layers"], ("id", "name", "kind", "routable")):
        bad.append(f"layers differ: {pick(h['layers'], ('id', 'name', 'kind'))} vs {pick(i['layers'], ('id', 'name', 'kind'))}")

    hp = {(p["ref"], p["number"]): p for p in h["pads"]}
    ip = {(p["ref"], p["number"]): p for p in i["pads"]}
    if set(hp) != set(ip):
        bad.append(f"pad sets differ: -{sorted(set(hp) - set(ip))[:5]} +{sorted(set(ip) - set(hp))[:5]}")
    for key in sorted(set(hp) & set(ip)):
        a, b = hp[key], ip[key]
        for f in ("uuid", "attr", "center", "layers", "drill", "local_clearance", "net_tie_group", "zone_connection",
                  "thermal_gap", "spoke_angle_deg"):
            if a.get(f) != b.get(f):
                bad.append(f"pad {key} {f}: {a.get(f)!r} vs {b.get(f)!r}")
        if nh[a["net"]] != ni[b["net"]]:
            bad.append(f"pad {key} net")
        if abs(((a["rotation_deg"] - b["rotation_deg"]) + 180) % 360 - 180) > 1e-6:
            bad.append(f"pad {key} rotation {a['rotation_deg']} vs {b['rotation_deg']}")
        for l in a["shapes"]:
            if l not in b["shapes"] or not _shape_equal(a, a["shapes"][l], b, b["shapes"][l], l):
                bad.append(f"pad {key} shape on layer {l}: {a['shapes'][l]} vs {b['shapes'].get(l)}")
    srt = lambda ls, f: sorted(ls, key=f)  # noqa: E731
    if srt(h["holes"], lambda x: str(x)) != srt(i["holes"], lambda x: str(x)):
        bad.append("holes differ")

    for sect in ("kept", "vias"):
        ha, ia = {x["uuid"]: x for x in h[sect]}, {x["uuid"]: x for x in i[sect]}
        if set(ha) != set(ia):
            bad.append(f"{sect} uuid sets differ")
        for u in set(ha) & set(ia):
            a, b = dict(ha[u]), dict(ia[u])
            if nh[a.pop("net")] != ni[b.pop("net")]:
                bad.append(f"{sect} {u} net")
            a.pop("free", None), b.pop("free", None)
            if a != b:
                bad.append(f"{sect} {u}: {a} vs {b}")

    if srt(h["outline"], str) != srt(i["outline"], str):
        lh = sum(math.hypot(*(a - b for a, b in zip(p.get("a", p.get("start")), p.get("b", p.get("end"))))) for p in h["outline"])
        li = sum(math.hypot(*(a - b for a, b in zip(p.get("a", p.get("start")), p.get("b", p.get("end"))))) for p in i["outline"])
        if abs(lh - li) > 0.01 * max(lh, 1):  # flattened circle / bezier edges are not point-identical
            bad.append("outline differs")

    key = lambda k: (k["name"], str(k["poly"]))  # noqa: E731
    if srt(h["keepouts"], key) != srt(i["keepouts"], key):
        bad.append("keepouts differ")
    if srt(h["net_ties"], str) != srt(i["net_ties"], str):
        bad.append("net_ties differ")

    def boxes(m):
        out = {}
        for c in m["copper_shapes"]:
            xs, ys = [q[0] for q in c["poly"]], [q[1] for q in c["poly"]]
            o = out.setdefault((c["uuid"], c["layer"]), [min(xs), min(ys), max(xs), max(ys)])
            o[:] = [min(o[0], *xs), min(o[1], *ys), max(o[2], *xs), max(o[3], *ys)]
        return out

    me = MAX_ERR_SLACK * h["rules"].get("max_error", 5000)
    bh, bi = boxes(h), boxes(i)
    if set(bh) != set(bi):
        bad.append("copper_shapes (uuid, layer) sets differ")
    for k in set(bh) & set(bi):
        if any(abs(x - y) > me for x, y in zip(bh[k], bi[k])):
            bad.append(f"copper_shape {k} bbox {bh[k]} vs {bi[k]}")

    hz, iz = {z["uuid"]: z for z in h["pours"]}, {z["uuid"]: z for z in i["pours"]}
    if set(hz) != set(iz):
        bad.append("pour uuid sets differ")
    for u in set(hz) & set(iz):
        a, b = hz[u], iz[u]
        for f in ("layers", "pads", "vias", "clearance", "min_thickness", "thermal_gap", "spoke_width", "pad_connection"):
            if a[f] != b[f]:
                bad.append(f"pour {u} {f}: {a[f]} vs {b[f]}")
        if nh[a["net"]] != ni[b["net"]]:
            bad.append(f"pour {u} net")
        fa = sum(_area([f["outer"], *f["holes"]]) for f in a["fills"])
        fb = sum(_area([f["outer"], *f["holes"]]) for f in b["fills"])
        if abs(fa - fb) > 0.005 * max(fa, 1):
            bad.append(f"pour {u} fill area {fa} vs {fb}")

    de = lambda ls: sorted(((x["item"], tuple(x["end"])) for x in ls))  # noqa: E731
    if de(h["dangling_ends"]) != de(i["dangling_ends"]):
        bad.append("dangling_ends differ")
    return bad


# ---------------------------------------------------------------- offline self-tests of the comparison
def test_polygon_equivalence_native_vs_polygon():
    pad = {"center": [0, 0], "rotation_deg": 90.0}
    rect = {"kind": "rect", "w": 2_000_000, "h": 1_000_000}
    poly = {"kind": "poly", "pts": [[-500_000, -1_000_000], [500_000, -1_000_000], [500_000, 1_000_000], [-500_000, 1_000_000]]}
    pr = {**pad, "shapes": {"0": rect}}
    pp = {**pad, "shapes": {"0": poly}}
    assert _shape_equal(pr, rect, pp, poly, "0")
    off = {"kind": "poly", "pts": [[p[0] + 2000, p[1]] for p in poly["pts"]]}
    assert not _shape_equal(pr, rect, {**pad, "shapes": {"0": off}}, off, "0")
    rr1 = {"kind": "roundrect", "w": 2_000_000, "h": 1_000_000, "rc": 250_000}
    assert _shape_equal(pr, rr1, pr, {**rr1, "rc": 250_900}, "0") and not _shape_equal(pr, rr1, pr, {**rr1, "rc": 252_000}, "0")
    circ = {"kind": "circle", "d": 1_000_000}
    pc = {**pad, "shapes": {"0": circ}}
    ring = geom.circle_ring((0, 0), 500_000, 200)
    assert _shape_equal(pc, circ, pc, {"kind": "poly", "pts": ring}, "0")


def test_compare_models_detects_a_difference():
    from tests.plugin.test_model_ipc import mk_pad, stub_board, StubProject  # noqa: F401  (offline model source)
    from topor import model_ipc as M
    from toporlib import rules as R

    rules = R.resolve_rules_from_project({}, None, netclass_of={"": "Default"}, classes={}, widths_before={})
    m = M.export_model_ipc(stub_board(), StubProject(), '(kicad_pcb (layers (0 "F.Cu" signal) (2 "B.Cu" signal)))',
                           rules, name="x.kicad_pcb")
    import copy
    assert compare_models(m, copy.deepcopy(m)) == []
    m2 = copy.deepcopy(m)
    m2["pads"][0]["center"][0] += 1
    m2["kept"][0]["width"] += 1
    bad = compare_models(m, m2)
    assert any("center" in b for b in bad) and any("kept" in b for b in bad)


# ---------------------------------------------------------------- live
BOARDS = ["eprom-to-kernal", "wiccon-2023", "dcsgonebadge", "bornhack2026", "nametag-sao", "wedding-gift"]


@pytest.mark.skipif(not os.environ.get("KICAD_API_SOCKET"), reason="needs a running KiCad (KICAD_API_SOCKET)")
@pytest.mark.parametrize("board_name", BOARDS)
def test_ipc_parity(board_name, tmp_path):
    """Open the snapshot board in KiCad first (GUI checklist); the harness model of the same file is the oracle."""
    from kipy import KiCad

    from harness.copy import copy_project
    from harness.model import export_model, load_board
    from harness.paths import BENCH_BOARDS
    from harness.rip import record_widths
    from harness.rules import resolve_rules
    from topor.model_ipc import export_model_ipc

    kicad = KiCad()
    board = kicad.get_board()  # UNVERIFIED: live
    src = BENCH_BOARDS[board_name]
    if board.name != src.name:
        pytest.skip(f"KiCad has {board.name!r} open, not {src.name!r}")
    pc = copy_project(src, tmp_path / "w")
    b = load_board(pc)
    dru = pc.dru.read_text() if pc.dru else None
    rules = resolve_rules(b, dru_text=dru, widths_before=record_widths(b))
    board.refill_zones()  # UNVERIFIED: live
    h = export_model(b, rules, name=board_name, dru_text=dru)
    m = export_model_ipc(board, board.get_project(), pc.pcb.read_text(), rules, name=board_name, dru_text=dru)
    assert compare_models(h, m) == []
