"""Rule C (toporlib.edge_zero, [rules] edge_clearance_zero): what tracks and vias keep from the board edge when the
board's copper-to-edge clearance is 0. Every mode x (board 0 / non-zero / unset) x (value found / none), the texts of
every case, the schema default and choices, the pre-flight finding and the report line."""
import pytest

from toporlib import edge_zero as ez

MM = 1_000_000


def square(x0, y0, x1, y1):
    return [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]


def model(edge=0, pours=None, outline=None):
    out = outline if outline is not None else [
        {"kind": "seg", "a": [0, 0], "b": [20 * MM, 0]}, {"kind": "seg", "a": [20 * MM, 0], "b": [20 * MM, 10 * MM]},
        {"kind": "seg", "a": [20 * MM, 10 * MM], "b": [0, 10 * MM]}, {"kind": "seg", "a": [0, 10 * MM], "b": [0, 0]}]
    return {"rules": {"edge_clearance": edge}, "outline": out, "pours": pours or []}


def pour(clearance, fills):
    return {"clearance": clearance, "fills": [{"layer": l, "outer": ring, "holes": []} for l, ring in fills]}


# two pours: GND on both layers 0.3 mm from the edge, a local island on layer 0 far inside with the bigger clearance
GND = pour(254_000, [(0, square(300_000, 300_000, 19_700_000, 9_700_000)), (2, square(100_000, 100_000, 19_900_000, 9_900_000))])
ISLAND = pour(508_000, [(0, square(8 * MM, 4 * MM, 9 * MM, 5 * MM))])


def test_default_and_schema():
    from topor.schema import SETTINGS  # noqa: PLC0415

    s = next(x for x in SETTINGS if (x.section, x.key) == ("rules", "edge_clearance_zero"))
    assert s.default == ez.DEFAULT_MODE == "zone_clearance"
    assert tuple(s.choices) == ez.MODES == ("zone_clearance", "board", "pours")


def test_zone_clearance_is_the_biggest_pour_clearance_for_all_layers():
    m, rec = ez.apply(model(pours=[GND, ISLAND]), "zone_clearance")
    assert rec == {"mode": "zone_clearance", "value_nm": 508_000, "board_nm": 0}
    assert m["rules"]["edge_clearance"] == 508_000
    assert ez.text(rec) == ("tracks and vias keep 0.508 mm from the board edge ([rules] edge_clearance_zero = "
                            "\"zone_clearance\": the biggest clearance of the board's copper pours); KiCad's DRC still checks 0")


def test_pours_is_the_biggest_layer_setback_of_fills_near_the_edge():
    # layer 0: GND 0.3 mm (the island 4 mm inside does not count), layer 2: 0.1 mm -> biggest = 0.3 mm
    m, rec = ez.apply(model(pours=[GND, ISLAND]), "pours")
    assert rec["value_nm"] == 300_000 and m["rules"]["edge_clearance"] == 300_000
    assert ez.text(rec).startswith("tracks and vias keep 0.300 mm from the board edge ([rules] edge_clearance_zero = \"pours\": "
                                   "the biggest distance the board's filled pours keep from the edge)")


def test_pours_measure_arcs_and_holes():
    # a round board (two half-circle arcs, radius 10 mm about (10, 10)); the fill is a hole-free disc 0.2 mm inside
    import math  # noqa: PLC0415

    c, r = 10 * MM, 10 * MM
    arcs = [{"kind": "arc", "start": [c - r, c], "mid": [c, c - r], "end": [c + r, c]},
            {"kind": "arc", "start": [c + r, c], "mid": [c, c + r], "end": [c - r, c]}]
    ring = [[int(c + (r - 200_000) * math.cos(2 * math.pi * k / 64)), int(c + (r - 200_000) * math.sin(2 * math.pi * k / 64))]
            for k in range(64)]
    _, rec = ez.apply(model(pours=[pour(100_000, [(0, ring)])], outline=arcs), "pours")
    assert abs(rec["value_nm"] - 200_000) < 2_000, rec


def test_board_mode_and_nothing_found():
    m0 = model(pours=[GND])
    m, rec = ez.apply(m0, "board")
    assert m is m0 and rec == {"mode": "board", "value_nm": 0, "board_nm": 0}
    assert ez.text(rec) == "copper-to-edge 0 used as it is ([rules] edge_clearance_zero = \"board\", as KiCad's DRC)"
    for mode, what in (("zone_clearance", "the biggest clearance of the board's copper pours"),
                       ("pours", "the biggest distance the board's filled pours keep from the edge")):
        m1 = model()
        m, rec = ez.apply(m1, mode)  # no pours at all
        assert m is m1 and rec["value_nm"] == 0
        assert ez.text(rec) == f"copper-to-edge 0 used as it is: [rules] edge_clearance_zero = \"{mode}\" found no value ({what}: none)"


def test_pours_reaching_the_edge_give_zero():
    touching = pour(254_000, [(0, square(0, 0, 20 * MM, 10 * MM))])
    _, rec = ez.apply(model(pours=[touching]), "pours")
    assert rec["value_nm"] == 0


@pytest.mark.parametrize("mode", ez.MODES)
def test_a_non_zero_board_value_wins(mode):
    m0 = model(edge=200_000, pours=[GND, ISLAND])
    m, rec = ez.apply(m0, mode)
    assert m is m0 and rec is None and ez.text(rec) == ""
    m1 = {"rules": {}, "pours": [GND]}  # no resolved value at all: untouched
    assert ez.apply(m1, mode) == (m1, None)


def test_unknown_mode():
    with pytest.raises(ValueError, match="edge_clearance_zero"):
        ez.resolve(model(), "fab")


def test_input_not_mutated():
    m0 = model(pours=[GND])
    m, _ = ez.apply(m0, "zone_clearance")
    assert m0["rules"]["edge_clearance"] == 0 and m["rules"]["edge_clearance"] == 254_000


def test_preflight_finding_names_the_value():
    from topor.preflight import _edge_zero_findings  # noqa: PLC0415

    for mode in ez.MODES:
        rec = ez.resolve(model(pours=[GND]), mode)
        (f,) = _edge_zero_findings({"edge_clearance": 0, "edge_zero": rec})
        assert f.code == "EDGE_CLEARANCE_ZERO" and f.severity == "warning"
        assert ("Copper to edge clearance" in f.message) and (ez.text(rec) in f.message), f.message
    (f,) = _edge_zero_findings({"edge_clearance": 0})  # no record (an older caller): the plain text
    assert f.message.endswith("Copper to edge clearance).")
    assert _edge_zero_findings({"edge_clearance": 100_000, "edge_zero": None}) == []


def test_report_settings_line(tmp_path):
    from topor.report import RunReport  # noqa: PLC0415

    rec = ez.resolve(model(pours=[GND]), "zone_clearance")
    r = RunReport(tmp_path, {"edge_zero": ez.text(rec)})
    assert "Copper to edge clearance 0 on the board: " + ez.text(rec) in r._settings()
    assert not any(l.startswith("Copper to edge clearance 0") for l in RunReport(tmp_path, {})._settings())


def test_pours_ignore_a_layer_whose_fills_stay_far_inside():
    # layer 2: GND 0.1 mm from the edge; layer 0: only the island 4 mm inside (outside the 1 mm window): 0.1 mm, not 4
    gnd2 = pour(254_000, [(2, square(100_000, 100_000, 19_900_000, 9_900_000))])
    _, rec = ez.apply(model(pours=[gnd2, ISLAND]), "pours")
    assert rec["value_nm"] == 100_000, rec
