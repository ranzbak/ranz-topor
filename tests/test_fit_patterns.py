"""Fit patterns (shopping list 2.1, harness/fit_patterns.py): each pattern regenerates, its boards pass KiCad DRC as
expected, and expected.json agrees with the geometry read back from the board file."""
import json
import math

import pcbnew
import pytest
from shapely.geometry import LineString, Point, box

from harness import fit_patterns as fp
from harness.drc import run_drc
from harness.paths import kicad_cli

NAMES = list(fp.PATTERNS)
MM = 1e6


@pytest.fixture(scope="module")
def generated(tmp_path_factory):
    out = tmp_path_factory.mktemp("fit")
    return {n: fp.generate(n, out, png=False) for n in NAMES}


def _pad_shapes(pcb, layer_name):
    """{"REF.num": shapely geometry} of the pads on a copper layer, read from the board file."""
    b = pcbnew.LoadBoard(str(pcb))
    lid = b.GetLayerID(layer_name)
    out = {}
    for f in b.GetFootprints():
        for p in f.Pads():
            if not p.IsOnLayer(lid):
                continue
            x, y = p.GetPosition().x / MM, p.GetPosition().y / MM
            w, h = p.GetSize().x / MM, p.GetSize().y / MM
            assert p.GetOrientationDegrees() == 0
            if p.GetShape() == pcbnew.PAD_SHAPE_CIRCLE:
                out[f"{f.GetReference()}.{p.GetNumber()}"] = ("circle", (x, y), w / 2)
            else:
                out[f"{f.GetReference()}.{p.GetNumber()}"] = ("rect", box(x - w / 2, y - h / 2, x + w / 2, y + h / 2))
    return out


def _dist_to_boundary(shape, pt):
    if shape[0] == "circle":
        return abs(math.dist(shape[1], pt) - shape[2])
    return shape[1].exterior.distance(Point(pt))


def _geom(shape):
    return Point(shape[1]).buffer(shape[2], quad_segs=256) if shape[0] == "circle" else shape[1]


@pytest.mark.parametrize("name", NAMES)
def test_expected_matches_board_geometry(generated, name):
    doc = json.loads(generated[name]["expected"].read_text())
    pro = json.loads(generated[name]["problem"].with_suffix(".kicad_pro").read_text())
    clear = {c["name"]: c["clearance"] for c in pro["net_settings"]["classes"]}
    width = {c["name"]: c["track_width"] for c in pro["net_settings"]["classes"]}
    cls = {p["pattern"]: p["netclass"] for p in pro["net_settings"]["netclass_patterns"]}
    for c in doc["cuts"]:
        pads = _pad_shapes(generated[name]["problem"], c["layer"])
        a, b = c["from"].split()[0], c["to"].split()[0]
        p, q = tuple(c["p"]), tuple(c["q"])
        # The cut's ends lie on the two features' boundaries ...
        assert _dist_to_boundary(pads[a], p) < 2e-4, (c["id"], a)
        assert _dist_to_boundary(pads[b], q) < 2e-4, (c["id"], b)
        # ... it is the closest pair of the two features (Euclid) ...
        d = _geom(pads[a]).distance(_geom(pads[b]))
        assert abs(d - c["length_euclid"]) < 2e-4, (c["id"], d)
        # ... and its open segment touches no copper of the layer (a cut, Maley p. 32).
        seg = LineString([p, q])
        inner = seg.interpolate(1e-3).coords[0], seg.interpolate(seg.length - 1e-3).coords[0]
        open_seg = LineString(inner)
        for k, sh in pads.items():
            assert not open_seg.intersects(_geom(sh)), (c["id"], k)
        # Lengths in both norms from the coordinates.
        dx, dy = q[0] - p[0], q[1] - p[1]
        assert abs(math.hypot(dx, dy) - c["length_euclid"]) < 1e-4
        assert abs(fp.oct_len(dx, dy) - c["length_octagonal"]) < 1e-4
        # Demand from the project's rules: widths + clearances between nets + one clearance at each end.
        nets = [i["net"] for i in c["crossing"]]
        if not nets:
            assert c["demand"] == 0
            continue
        ws = [width[cls.get(n, "Default")] for n in nets]
        ss = [clear[cls.get(n, "Default")] for n in nets]
        need = sum(ws) + sum(max(s0, s1) for s0, s1 in zip(ss, ss[1:])) + 2 * max(ss[0], 0.2)
        assert abs(need - c["demand"]) < 1e-4
        assert abs(c["length_euclid"] - need - c["slack_euclid"]) < 2e-4
        assert abs(c["length_octagonal"] - need - c["slack_octagonal"]) < 2e-4


@pytest.mark.parametrize("name", NAMES)
def test_witness_crosses_its_cuts(generated, name):
    """The planned topology (centre lines) crosses every cut the wire is listed on once (an odd number of times) and
    every other cut an even number of times (entanglement 0, Maley p. 32: crossings that cancel do not count; e.g. the
    innermost model_gap_cut wire dips under TP1 past the line BD and back)."""
    doc = json.loads(generated[name]["expected"].read_text())
    for w in doc["wires"]:
        if not w["witness"]:
            continue
        line = LineString(w["witness"])
        for c in doc["cuts"]:
            if c["layer"] != w["witness_layer"]:
                continue
            x = line.intersection(LineString([c["p"], c["q"]]))
            n = 0 if x.is_empty else len(getattr(x, "geoms", [x]))
            assert n % 2 == (c["id"] in w["crosses"]), (w["net"], c["id"], n)


@pytest.mark.parametrize("name", NAMES)
def test_binding_cut_verdict(name):
    pat = fp.PATTERNS[name]()
    fp.check_hand(pat)
    c = next(c for c in pat.cuts if c.role == "binding")
    v = fp.cut_values(pat, c)
    s = v.get("slack_floor", v["slack_e"])
    if pat.kind == "solvable":
        assert s >= 0
    elif pat.kind == "unsolvable":
        assert s < 0
    else:  # smooth fits, octilinear over-full
        assert v["slack_e"] > 0 > v["slack_o"]


@pytest.mark.skipif(not kicad_cli().exists(), reason="no kicad-cli")
@pytest.mark.parametrize("name", NAMES)
def test_kicad_drc(generated, name):
    doc = json.loads(generated[name]["expected"].read_text())
    pb = generated[name]["problem"]
    r = run_drc(pb, pb.with_name("drc.json"))
    assert r.violations == [], [v.description for v in r.violations]
    assert len(r.unconnected) == doc["connections"]
    if "reference" in generated[name]:
        rb = generated[name]["reference"]
        r = run_drc(rb, rb.with_name("drc.json"))
        assert r.violations == [], [v.description for v in r.violations]
        assert r.unconnected == []
