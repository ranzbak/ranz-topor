"""IPC-vs-harness model parity (M4-R5, M4-R8). LIVE: needs a running KiCad (KICAD_API_SOCKET) with the snapshot
board open in the PCB editor; skipped otherwise (never started or touched by the test suite). The comparison helpers
are self-tested offline.

Every coordinate must be equal (0 nm), except where M4-R8 allows a CONSERVATIVE approximation; there the test
asserts the direction, not equality:
  * pad shapes: polygon-equivalent within 1 um (M4-R5; a native shape on one side vs a polygon on the other is
    compared as polygons; same native kind -> exact numbers, roundrect radius within 1 um);
  * copper_shapes (graphics and text): per (uuid, layer) the IPC obstacle CONTAINS the harness polygon within
    1 um (IPC ⊇ pcbnew; the bounded excess is proven offline in test_ipc_conservative.py). An arc whose stored
    mid point is not its angular midpoint is compared with pcbnew's polygon of the same arc drawn with the true
    midpoint (pcbnew's own polygon of such an arc is off-centre; KiCad's DRC shape is the true arc);
  * pours: same uuid / net / layers / settings; fills polygon-equivalent (symmetric difference <= 1 um^2);
    reached pads / vias of the IPC side ⊆ the harness side (pour reach errs toward "not reached", C26);
  * via ``free``: exact (C23; the IPC side reads KiCad's flag from the save_as copy).
Net codes differ between the sides (pcbnew's vs sorted-name), so everything is compared by net NAME. Polygon
areas use pcbnew SHAPE_POLY_SET (test side only)."""
import math
import os

import pytest

from toporlib import geom

UM = 1000
EPS_AREA = 1.0 * UM * UM  # nm^2

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

    from tests.plugin.test_ipc_conservative import inflated, minus_area, sps, sps_fills  # pcbnew: test side only

    def shapes(m):
        out: dict = {}
        for c in m["copper_shapes"]:
            out.setdefault((c["uuid"], c["layer"]), []).append(c["poly"])
        return out

    sh, si = shapes(h), shapes(i)
    if set(sh) != set(si):
        bad.append(f"copper_shapes (uuid, layer) sets differ: -{sorted(set(sh) - set(si))[:3]} "
                   f"+{sorted(set(si) - set(sh))[:3]}")
    for k in sorted(set(sh) & set(si)):
        miss = minus_area(sps(sh[k]), inflated(sps(si[k]), UM))
        if miss > EPS_AREA:
            bad.append(f"copper_shape {k}: {miss:.0f} nm^2 of the harness polygon outside the IPC obstacle")

    hz, iz = {z["uuid"]: z for z in h["pours"]}, {z["uuid"]: z for z in i["pours"]}
    if set(hz) != set(iz):
        bad.append("pour uuid sets differ")
    for u in set(hz) & set(iz):
        a, b = hz[u], iz[u]
        for f in ("layers", "clearance", "min_thickness", "thermal_gap", "spoke_width", "pad_connection"):
            if a[f] != b[f]:
                bad.append(f"pour {u} {f}: {a[f]} vs {b[f]}")
        if nh[a["net"]] != ni[b["net"]]:
            bad.append(f"pour {u} net")
        for f in ("pads", "vias"):
            extra = sorted(set(b[f]) - set(a[f]))
            if extra:
                bad.append(f"pour {u} {f}: IPC reaches what pcbnew does not: {extra[:5]}")
        for l in a["layers"]:
            fa, fb = sps_fills(a["fills"], l), sps_fills(b["fills"], l)
            sym = minus_area(fa, fb) + minus_area(fb, fa)
            if sym > EPS_AREA:
                bad.append(f"pour {u} layer {l}: fill symmetric difference {sym:.0f} nm^2")

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


def test_compare_models_asserts_the_conservative_directions():
    """M4-R8: a bigger IPC obstacle / fewer reached items pass; a smaller obstacle, an IPC-only reach, a fill
    difference or a different free flag fail."""
    import copy

    from tests.plugin.test_model_ipc import StubProject, stub_board
    from topor import model_ipc as M
    from toporlib import rules as R

    rules = R.resolve_rules_from_project({}, None, netclass_of={"": "Default"}, classes={}, widths_before={})
    h = M.export_model_ipc(stub_board(), StubProject(), '(kicad_pcb (layers (0 "F.Cu" signal) (2 "B.Cu" signal)))',
                           rules, name="x.kicad_pcb")
    assert h["copper_shapes"] and h["pours"] and h["pours"][0]["pads"] and h["vias"]

    def scaled(m, k):
        out = copy.deepcopy(m)
        for c in out["copper_shapes"]:
            cx = sum(q[0] for q in c["poly"]) / len(c["poly"])
            cy = sum(q[1] for q in c["poly"]) / len(c["poly"])
            c["poly"] = [[int(cx + k * (q[0] - cx)), int(cy + k * (q[1] - cy))] for q in c["poly"]]
        return out

    assert compare_models(h, scaled(h, 1.01)) == []  # IPC obstacle bigger: conservative
    assert any("outside the IPC obstacle" in b for b in compare_models(h, scaled(h, 0.99)))
    fewer = copy.deepcopy(h)
    fewer["pours"][0]["pads"] = []
    assert compare_models(h, fewer) == []  # IPC reaches less: conservative
    assert any("IPC reaches" in b for b in compare_models(fewer, h))
    holed = copy.deepcopy(h)
    f = holed["pours"][0]["fills"][0]
    x0, y0 = min(q[0] for q in f["outer"]), min(q[1] for q in f["outer"])
    f["holes"] = [[[x0 + 10_000, y0 + 10_000], [x0 + 20_000, y0 + 10_000], [x0 + 20_000, y0 + 20_000],
                   [x0 + 10_000, y0 + 20_000]]]
    assert any("fill symmetric difference" in b for b in compare_models(h, holed))
    flag = copy.deepcopy(h)
    flag["vias"][0]["free"] = not flag["vias"][0]["free"]
    assert any(b.startswith("vias ") for b in compare_models(h, flag))


def test_true_arc_reference_replaces_skewed_arcs_only():
    import pcbnew

    from tests.plugin.test_ipc_conservative import kipy_shape

    b = pcbnew.BOARD()
    MM = 1_000_000
    shapes = []
    for mid in ((7_071_068, 7_071_068), (0, 10 * MM)):  # 180 deg arc: skewed mid (45 deg) / true midpoint (90 deg)
        s = pcbnew.PCB_SHAPE(b, pcbnew.SHAPE_T_ARC)
        s.SetLayer(pcbnew.F_Cu)
        s.SetWidth(300_000)
        s.SetArcGeometry(pcbnew.VECTOR2I(10 * MM, 0), pcbnew.VECTOR2I(*mid), pcbnew.VECTOR2I(-10 * MM, 0))
        b.Add(s)
        shapes.append(s)
    from topor import model_ipc as M

    h = {"rules": {"max_error": 5000}, "copper_shapes": [
        {"uuid": s.m_Uuid.AsString(), "net": 0, "layer": 0, "poly": [[0, 0], [1, 0], [1, 1]], "owner_ref": None}
        for s in shapes]}
    out = with_true_arc_reference(h, b)
    by = {c["uuid"]: c for c in out["copper_shapes"]}
    assert by[shapes[1].m_Uuid.AsString()]["poly"] == [[0, 0], [1, 0], [1, 1]]  # untouched
    fixed = by[shapes[0].m_Uuid.AsString()]["poly"]
    assert len(fixed) > 10
    ipc = M.shape_to_polys(kipy_shape(shapes[0]), 5000)
    i = {**out, "copper_shapes": [{"uuid": shapes[0].m_Uuid.AsString(), "layer": 0, "poly": r} for r in ipc]}
    from tests.plugin.test_ipc_conservative import inflated, minus_area, sps

    assert minus_area(sps([fixed]), inflated(sps(ipc), UM)) <= EPS_AREA
    del i


def with_true_arc_reference(h: dict, b) -> dict:
    """The harness model with the copper_shapes polygons of every arc whose stored mid point is skewed replaced by
    pcbnew's polygon of the same arc drawn with its true midpoint (test_ipc_conservative.reference_item; KiCad's
    DRC shape is that arc, test_ipc_conservative.assert_drc_shape_is_the_arc proves it offline)."""
    from tests.plugin.test_ipc_conservative import arc_mid_skew, copper_items, item_poly

    max_err = int(h["rules"].get("max_error") or 5000)
    fix = {}
    for it in copper_items(b, ("PCB_SHAPE",)):
        if arc_mid_skew(it) >= 1e-4:
            ps = item_poly(it, max_err)
            fix[(it.m_Uuid.AsString(), int(it.GetLayer()))] = [
                [[int(ps.Outline(k).CPoint(j).x), int(ps.Outline(k).CPoint(j).y)]
                 for j in range(ps.Outline(k).PointCount())] for k in range(ps.OutlineCount())]
    if not fix:
        return h
    keep = [c for c in h["copper_shapes"] if (c["uuid"], c["layer"]) not in fix]
    for (u, l), rings in fix.items():
        orig = next((c for c in h["copper_shapes"] if c["uuid"] == u), {"net": 0, "owner_ref": None})
        keep += [{"uuid": u, "net": orig["net"], "layer": l, "poly": r, "owner_ref": orig["owner_ref"]} for r in rings]
    return {**h, "copper_shapes": keep}


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
    h = with_true_arc_reference(h, b)
    m = export_model_ipc(board, board.get_project(), pc.pcb.read_text(), rules, name=board_name, dru_text=dru)
    assert compare_models(h, m) == []
