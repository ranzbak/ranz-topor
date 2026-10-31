"""Pure mapping parts of topor.ipc (no KiCad): engine items -> kipy objects, kipy selection -> Selection,
and IpcBoard against a recording stub of kipy's Board."""
import os
from pathlib import Path

import pytest

kipy = pytest.importorskip("kipy")
from kipy.board_types import (ArcTrack, BoardLayer, BoardPolygon, FootprintInstance, Net, Pad, Track, Via,
                              ViaType, Zone)
from kipy.geometry import Vector2
from kipy.proto.board import board_types_pb2
from kipy.proto.common.types import KIID

from topor import ipc
from topor.ports import CommitRequest, PadRef

MM = 1_000_000
NETS = {1: "GND", 2: "VCC"}
SEG = {"net": 1, "layer": 0, "width": 250_000, "prim": {"kind": "seg", "a": [1_000_001, 2], "b": [3 * MM, -4]}}
ARC = {"net": 2, "layer": 2, "width": 200_000,
       "prim": {"kind": "arc", "start": [0, 0], "mid": [MM, MM], "end": [2 * MM, 0]}}
VIA = {"kind": "via", "net": 1, "at": [5 * MM, 6 * MM], "d": 800_000, "drill": 400_000, "layers": [0, 2]}


def test_layer_mapping_roundtrip():
    assert ipc.pcbnew_to_kipy_layer(0) == BoardLayer.BL_F_Cu
    assert ipc.pcbnew_to_kipy_layer(2) == BoardLayer.BL_B_Cu
    assert ipc.pcbnew_to_kipy_layer(4) == BoardLayer.BL_In1_Cu
    assert ipc.pcbnew_to_kipy_layer(6) == BoardLayer.BL_In2_Cu
    assert ipc.pcbnew_to_kipy_layer(62) == BoardLayer.BL_In30_Cu
    for i in (0, 2, 4, 6, 30, 62):
        assert ipc.kipy_to_pcbnew_layer(ipc.pcbnew_to_kipy_layer(i)) == i
    with pytest.raises(ValueError):
        ipc.pcbnew_to_kipy_layer(3)


def test_item_to_kipy_track_arc_via():
    t = ipc.to_kipy_item(SEG, None, NETS)
    assert isinstance(t, Track) and not isinstance(t, ArcTrack)
    assert (t.start.x, t.start.y, t.end.x, t.end.y) == (1_000_001, 2, 3 * MM, -4)
    assert t.width == 250_000 and t.layer == BoardLayer.BL_F_Cu and t.net.name == "GND"
    a = ipc.to_kipy_item(ARC, None, NETS)
    assert isinstance(a, ArcTrack)
    assert (a.start.x, a.mid.x, a.mid.y, a.end.x) == (0, MM, MM, 2 * MM)
    assert a.width == 200_000 and a.layer == BoardLayer.BL_B_Cu and a.net.name == "VCC"
    v = ipc.to_kipy_item(VIA, None, NETS)
    assert isinstance(v, Via) and v.type == ViaType.VT_THROUGH
    assert (v.position.x, v.position.y, v.diameter, v.drill_diameter) == (5 * MM, 6 * MM, 800_000, 400_000)
    assert v.net.name == "GND"
    assert (v.padstack.drill.start_layer, v.padstack.drill.end_layer) == (BoardLayer.BL_F_Cu, BoardLayer.BL_B_Cu)


def test_item_to_kipy_custom_layer_map_blind_and_net_name():
    t = ipc.to_kipy_item({**SEG, "layer": 7, "net_name": "X"}, {7: BoardLayer.BL_In1_Cu}, None)
    assert t.layer == BoardLayer.BL_In1_Cu and t.net.name == "X"
    v = ipc.to_kipy_item({**VIA, "layers": [0, 4]}, None, NETS)
    assert v.type == ViaType.VT_BLIND_BURIED and v.padstack.drill.end_layer == BoardLayer.BL_In1_Cu
    with pytest.raises(KeyError):
        ipc.to_kipy_item(SEG, None, {})


def _fp(ref, uid):
    p = board_types_pb2.FootprintInstance()
    p.id.value = uid
    p.reference_field.text.text.text = ref
    return FootprintInstance(p)


def _pad(num, parent):
    p = board_types_pb2.Pad()
    p.number = num
    p.parent.value = parent
    return Pad(p)


def _poly_zone(pts, rule_area):
    z = Zone()
    z.type = board_types_pb2.ZoneType.ZT_RULE_AREA if rule_area else board_types_pb2.ZoneType.ZT_COPPER
    o = z.proto.outline.polygons.add()
    for x, y in pts:
        n = o.outline.nodes.add()
        n.point.x_nm, n.point.y_nm = x, y
    o.outline.closed = True
    return z


def test_selection_from_kipy_items():
    trk = Track()
    trk.proto.id.value = "t-1"
    arc = ArcTrack()
    arc.proto.id.value = "a-1"
    via = Via()
    via.proto.id.value = "v-1"
    sq = [(0, 0), (MM, 0), (MM, MM), (0, MM)]
    items = [_fp("U1", "fp-1"), _pad("3", "fp-1"), _pad("9", "unknown"), trk, arc, via,
             _poly_zone(sq, rule_area=True), _poly_zone([(5, 5), (6, 6), (7, 5)], rule_area=False)]
    s = ipc.selection_from_kipy(items, {"fp-1": "U1"})
    assert s.footprints == ["U1"] and s.pads == [PadRef("U1", "3")]
    assert s.tracks == ["t-1", "a-1"] and s.vias == ["v-1"]
    assert s.area == sq and not s.empty
    assert ipc.selection_from_kipy([]).empty


def test_selection_polygon_shape_is_area():
    proto = board_types_pb2.BoardGraphicShape()
    pl = proto.shape.polygon.polygons.add()
    for x, y in [(0, 0), (4, 0), (4, 4)]:
        n = pl.outline.nodes.add()
        n.point.x_nm, n.point.y_nm = x, y
    assert ipc.selection_from_kipy([BoardPolygon(proto)]).area == [(0, 0), (4, 0), (4, 4)]


class _StubBoard:
    def __init__(self, tmp):
        self.calls, self.tmp = [], tmp
        self.name = "b.kicad_pcb"

    def get_project(self):
        return self

    path = property(lambda self: str(self.tmp))

    def begin_commit(self):
        self.calls.append("begin")
        return "C"

    def remove_items_by_id(self, ids):
        self.calls.append(("remove", [i.value for i in ids]))

    def create_items(self, items):
        items = items if isinstance(items, list) else [items]
        self.calls.append(("create", [type(i).__name__ for i in items]))
        out = []
        for n, i in enumerate(items):
            if hasattr(i, "proto") and hasattr(i.proto, "id") and not i.proto.id.value:
                i.proto.id.value = f"new-{n}"
            out.append(i)
        return out

    def push_commit(self, c, msg):
        self.calls.append(("push", c, msg))

    def drop_commit(self, c):
        self.calls.append(("drop", c))

    def save_as(self, filename, overwrite=False, include_project=True):
        self.calls.append(("save_as", filename, overwrite, include_project))
        Path(filename).write_text("(kicad_pcb)")

    def refill_zones(self):
        self.calls.append("refill")


class _StubKiCad:
    def __init__(self):
        self.actions = []

    def run_action(self, a):
        self.actions.append(a)


def test_ipcboard_commit_sequence(tmp_path):
    b = _StubBoard(tmp_path)
    ib = ipc.IpcBoard(_StubKiCad(), b)
    ib._net_names = NETS
    res = ib.commit(CommitRequest(["u1", "u2"], [{**SEG, "group": "topor"}, VIA], "msg"))
    assert b.calls[0] == "begin" and b.calls[1] == ("remove", ["u1", "u2"])
    assert b.calls[2] == ("create", ["Track", "Via"])
    assert b.calls[-1] == ("push", "C", "msg")
    assert (res.removed, res.created, res.created_ids) == (2, 2, ["new-0", "new-1"])


def test_ipcboard_commit_drops_on_error(tmp_path):
    b = _StubBoard(tmp_path)
    ib = ipc.IpcBoard(_StubKiCad(), b)  # no net names -> KeyError while building items
    with pytest.raises(KeyError):
        ib.commit(CommitRequest([], [SEG], "m"))
    assert b.calls == ["begin", ("drop", "C")]


def test_ipcboard_save_copy_and_misc(tmp_path):
    proj = tmp_path / "proj"
    proj.mkdir()
    (proj / "b.kicad_pro").write_text("{}")
    (proj / "b.kicad_dru").write_text("(version 1)")
    b = _StubBoard(proj)
    k = _StubKiCad()
    ib = ipc.IpcBoard(k, b)
    out = ib.save_copy(tmp_path / "dst")
    assert out == tmp_path / "dst" / "b.kicad_pcb" and out.is_file()
    assert (tmp_path / "dst" / "b.kicad_dru").is_file() and (tmp_path / "dst" / "b.kicad_pro").is_file()
    assert ("save_as", str(out), True, True) in b.calls
    assert ib.project_dir() == proj and ib.board_name() == "b.kicad_pcb"
    ib.refill_zones()
    ib.run_action("pcbnew.X")
    assert "refill" in b.calls and k.actions == ["pcbnew.X"]


class _NetBoard(_StubBoard):
    """Stub with nets / net classes: `get_netclass_for_nets` only knows some nets (as KiCad may answer)."""

    def __init__(self, tmp, nets, classed):
        super().__init__(tmp)
        self.nets, self.classed, self.net_calls = nets, classed, 0

    def get_nets(self):
        self.net_calls += 1
        return [Net(name=n) for n in self.nets]

    def get_net_classes(self):  # Project.get_net_classes (get_project() is the stub itself)
        return []

    def get_netclass_for_nets(self, nets):
        class NC:
            name = "Default"
        return {n.name: NC() for n in nets if n.name in self.classed}


def test_ipcboard_one_net_list_for_rules_and_model(tmp_path, monkeypatch):
    """MINOR 13: the rules (run.resolve_rules) and the model (export_model_ipc) get net codes from ONE net
    list (IpcBoard.net_codes), even when KiCad's net-class answer misses a net; export_model_ipc gets the dru
    text of the save_as COPY, not of the live project dir."""
    from topor import model_ipc, run as run_mod

    live = tmp_path / "live"
    live.mkdir()
    (live / "b.kicad_dru").write_text('(version 1)\n(rule live_only (constraint clearance (min 9mm)))\n')
    b = _NetBoard(live, ["VCC", "GND", "SIG", ""], classed={"VCC"})
    ib = ipc.IpcBoard(_StubKiCad(), b)
    assert ib.net_codes() == {"GND": 1, "SIG": 2, "VCC": 3}
    copy_dir = tmp_path / "copy"
    copy_dir.mkdir()
    pcb = copy_dir / "b.kicad_pcb"
    pcb.write_text('(kicad_pcb (version 20260206)\n\t(layers\n\t\t(0 "F.Cu" signal)\n\t\t(2 "B.Cu" signal)\n\t)\n)\n')
    (copy_dir / "b.kicad_dru").write_text('(version 1)\n(rule copy_only (constraint clearance (min 0.3mm)))\n')
    rules = run_mod.resolve_rules(ib, pcb, pcb.read_text())
    assert sorted(rules["per_net"], key=int) == ["1", "2", "3"]  # every net, keyed by the shared codes
    seen = {}

    def fake_export(board, project, text, rules_, **kw):
        seen.update(kw)
        return {"nets": [{"code": c, "name": n} for n, c in kw["net_codes"].items()]}

    monkeypatch.setattr(model_ipc, "export_model_ipc", fake_export)
    ib.read_model(rules, pcb)
    assert seen["net_codes"] == ib.net_codes()
    assert seen["dru_text"] == (copy_dir / "b.kicad_dru").read_text() and "copy_only" in seen["dru_text"]
    assert b.net_calls == 1  # one get_nets() answer serves the classes, the rules and the model
    # a copy without .kicad_dru: "" (no custom rules), never the live project's file
    (copy_dir / "b.kicad_dru").unlink()
    ib.read_model(rules, pcb)
    assert seen["dru_text"] == ""


def test_ipcboard_connect_reports_connection_error(tmp_path):
    """MINOR 3: the KiCad connection is made in ipc.py; a missing API socket raises (fast) from connect()."""
    from kipy.errors import ConnectionError as KipyConnectionError

    with pytest.raises(KipyConnectionError, match="Failed to connect to KiCad"):
        ipc.IpcBoard.connect(socket_path=f"ipc://{tmp_path / 'nothing.sock'}", token="t")


@pytest.mark.skipif(not os.environ.get("KICAD_API_SOCKET"), reason="needs a running KiCad (KICAD_API_SOCKET)")
def test_ipc_live_smoke(tmp_path):
    from kipy import KiCad
    ib = ipc.IpcBoard(KiCad())
    out = ib.save_copy(tmp_path)
    text = out.read_text()
    from toporlib import pcbfile
    assert text.lstrip().startswith("(kicad_pcb") and isinstance(pcbfile.find_items(text), list)
    assert isinstance(ib.selection().empty, bool)
