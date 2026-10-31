"""toporlib.pcbfile: balanced-paren item scan / remove / append (find/remove/append parts; B3 adds parse_layers)."""
import json
import re
import subprocess

import pytest

from harness.paths import board_pcb
from toporlib import kicad_cli, pcbfile

MM = 1_000_000


@pytest.fixture(scope="module")
def eprom():
    p = board_pcb("eprom-to-kernal")
    return p, p.read_text()


def test_find_items_small():
    t = ('(kicad_pcb (segment (start 1 2) (end 3 4) (net "a(b)") (uuid "u1"))\n'
         ' (footprint "x" (via (at 1 1)))\n (via (at 0 0) (uuid "u2")) (arc (uuid "u3")))')
    spans = pcbfile.find_items(t)
    assert [(s.kind, s.uuid) for s in spans] == [("segment", "u1"), ("via", "u2"), ("arc", "u3")]
    for s in spans:
        assert t[s.start] == "(" and t[s.end - 1] == ")"
    assert [s.kind for s in pcbfile.find_items(t, kinds=("via",))] == ["via"]


def test_find_items_string_literals():
    t = '(kicad_pcb (segment (net "x)\\"(") (uuid "u1")) (segment (uuid "u2")))'
    assert [s.uuid for s in pcbfile.find_items(t)] == ["u1", "u2"]


def test_pcbfile_roundtrip_on_snapshot(eprom, tmp_path):
    pcb, text = eprom
    spans = pcbfile.find_items(text)
    expected = len(re.findall(r"^\t\((?:segment|arc|via)$", text, re.M))
    assert len(spans) == expected and expected > 10

    # remove 3 uuids -> the rest is byte-identical
    victims = [spans[0], spans[5], spans[-1]]
    out = pcbfile.remove_items(text, [s.uuid for s in victims])
    rest = text
    for s in sorted(victims, key=lambda s: -s.start):
        # span plus the whitespace run before it (indent + newline)
        a = s.start
        while a > 0 and text[a - 1] in " \t":
            a -= 1
        if a > 0 and text[a - 1] == "\n":
            a -= 1
        rest = rest[:a] + rest[s.end:]
    assert out == rest
    assert len(pcbfile.find_items(out)) == expected - 3
    assert pcbfile.remove_items(text, []) == text
    assert pcbfile.remove_items(text, ["nope"]) == text

    # append seg / arc / via
    names = {0: "F.Cu", 2: "B.Cu"}
    items = [
        {"net": 1, "layer": 0, "width": 250_000,
         "prim": {"kind": "seg", "a": [100 * MM, 100 * MM], "b": [101 * MM + 123, 100 * MM]}},
        {"net": 1, "layer": 0, "width": 250_000,
         "prim": {"kind": "arc", "start": [101 * MM + 123, 100 * MM], "mid": [102 * MM, 101 * MM],
                  "end": [103 * MM, 101 * MM + 456_000]}},
        {"kind": "via", "net": 1, "at": [103 * MM, 101 * MM + 456_000], "d": 800_000, "drill": 400_000,
         "layers": [0, 2]},
    ]
    net1 = {1: "GND"}
    new = pcbfile.append_items(text, items, layer_names=names, net_names=net1)
    assert new.rstrip().endswith(")") and len(pcbfile.find_items(new)) == expected + 3
    assert new.startswith(text[:2000])
    dst = tmp_path / "t.kicad_pcb"
    dst.write_text(new)

    r = subprocess.run([str(kicad_cli.kicad_cli()), "pcb", "drc", "--format", "json", "-o",
                        str(tmp_path / "drc.json"), str(dst)], capture_output=True, text=True)
    assert r.returncode in (0, 5), r.stdout + r.stderr
    assert json.loads((tmp_path / "drc.json").read_text())

    import pcbnew  # oracle, tests only
    b = pcbnew.LoadBoard(str(dst))
    segs = [t for t in b.GetTracks() if t.GetClass() == "PCB_TRACK" and t.GetStart().x == 100 * MM
            and t.GetStart().y == 100 * MM]
    assert len(segs) == 1 and (segs[0].GetEnd().x, segs[0].GetEnd().y) == (101 * MM + 123, 100 * MM)
    assert segs[0].GetWidth() == 250_000 and b.GetLayerName(segs[0].GetLayer()) == "F.Cu"
    assert segs[0].GetNetname() == "GND"
    arcs = [t for t in b.GetTracks() if t.GetClass() == "PCB_ARC"]
    assert len(arcs) == 1
    a = arcs[0]
    assert (a.GetMid().x, a.GetMid().y) == (102 * MM, 101 * MM)
    assert (a.GetEnd().x, a.GetEnd().y) == (103 * MM, 101 * MM + 456_000)
    vias = [t for t in b.GetTracks() if t.GetClass() == "PCB_VIA" and t.GetPosition().x == 103 * MM
            and t.GetPosition().y == 101 * MM + 456_000]
    assert len(vias) == 1 and vias[0].GetDrillValue() == 400_000 and vias[0].GetWidth(0) == 800_000


def test_append_items_uses_given_uuid_and_validates():
    t = "(kicad_pcb\n\t(version 1)\n)\n"
    it = {"net": 2, "layer": 2, "width": 200_000, "uuid": "abc",
          "prim": {"kind": "seg", "a": [0, 0], "b": [1_000_000, 0]}}
    out = pcbfile.append_items(t, [it], layer_names={2: "B.Cu"}, net_names={2: "N"})
    assert '(uuid "abc")' in out and '(layer "B.Cu")' in out and '(net "N")' in out
    assert out.rstrip().endswith(")")
    with pytest.raises(KeyError):
        pcbfile.append_items(t, [{**it, "layer": 9}], layer_names={2: "B.Cu"}, net_names={2: "N"})


def test_parse_layers_synthetic_4layer():
    t = ('(kicad_pcb (version 20260101)\n\t(layers\n\t\t(0 "F.Cu" signal)\n\t\t(4 "In1.Cu" power)\n'
         '\t\t(6 "In2.Cu" power "PWR plane")\n\t\t(2 "B.Cu" signal)\n\t\t(9 "F.Adhes" user "F.Adhesive")\n'
         '\t\t(5 "F.SilkS" user)\n\t)\n\t(segment (layers "F.Cu"))\n)')
    got = pcbfile.parse_layers(t)
    assert [(g["id"], g["name"], g["type"]) for g in got] == [
        (0, "F.Cu", "signal"), (4, "In1.Cu", "power"), (6, "In2.Cu", "power"), (2, "B.Cu", "signal")]


def test_parse_layers_orders_top_to_bottom_and_types():
    t = ('(kicad_pcb (layers (0 "F.Cu" mixed) (8 "In3.Cu" jumper) (4 "In1.Cu" signal) (6 "In2.Cu" signal) '
         '(2 "B.Cu" signal)))')
    assert [(g["name"], g["type"]) for g in pcbfile.parse_layers(t)] == [
        ("F.Cu", "mixed"), ("In1.Cu", "signal"), ("In2.Cu", "signal"), ("In3.Cu", "jumper"), ("B.Cu", "signal")]
    assert pcbfile.parse_layers("(kicad_pcb)") == []


def test_parse_layers_on_snapshots():
    from harness.paths import board_pcb
    assert [g["name"] for g in pcbfile.parse_layers(board_pcb("eprom-to-kernal").read_text())] == ["F.Cu", "B.Cu"]
    w = pcbfile.parse_layers(board_pcb("wiccon-2023").read_text())  # legacy file: B.Cu is numbered 31
    assert [(g["id"], g["name"]) for g in w] == [(0, "F.Cu"), (2, "B.Cu")]
    b = pcbfile.parse_layers(board_pcb("bornhack2026").read_text())
    assert [(g["id"], g["name"], g["type"]) for g in b] == [
        (0, "F.Cu", "signal"), (4, "In1.Cu", "power"), (6, "In2.Cu", "power"), (2, "B.Cu", "signal")]
