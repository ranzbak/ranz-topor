"""``IpcBoard``: the real BoardPort over kicad-python (kipy 0.8) and a running KiCad (IPC API, M4-R1: no
pcbnew). Anything that needs a live KiCad is UNVERIFIED until the GUI checklist ran; the pure mapping
helpers (``to_kipy_item``, ``selection_from_kipy``, ``pcbnew_to_kipy_layer``) are unit-tested.

Complexity: every method is O(#items it transfers) plus one IPC round trip per call (the API has no
batching beyond ``create_items`` / ``remove_items_by_id``, which are used).
"""

from __future__ import annotations

import hashlib
import shutil
from pathlib import Path

from kipy.board_types import (ArcTrack, BoardLayer, BoardPolygon, FootprintInstance, Net, Pad, Track, Via,
                              ViaType, Zone)
from kipy.geometry import Vector2
from kipy.proto.common.types import KIID

from .ports import CommitRequest, CommitResult, NetClassValues, PadRef, Selection

# ---------------------------------------------------------------- layers


def pcbnew_to_kipy_layer(i: int) -> int:
    """Engine / pcbnew copper layer id (F.Cu 0, B.Cu 2, In<n>.Cu 2n+2) -> kipy ``BoardLayer`` value
    (BL_F_Cu 3, BL_In<n>_Cu 3+n, BL_B_Cu 34). O(1)."""
    i = int(i)
    if i == 0:
        return BoardLayer.BL_F_Cu
    if i == 2:
        return BoardLayer.BL_B_Cu
    if i >= 4 and i % 2 == 0 and i <= 62:
        return BoardLayer.Value(f"BL_In{(i - 4) // 2 + 1}_Cu")
    raise ValueError(f"not a copper layer id: {i}")


def kipy_to_pcbnew_layer(b: int) -> int:
    """Inverse of :func:`pcbnew_to_kipy_layer`. O(1)."""
    name = BoardLayer.Name(b)
    if name == "BL_F_Cu":
        return 0
    if name == "BL_B_Cu":
        return 2
    if name.startswith("BL_In") and name.endswith("_Cu"):
        return 2 * int(name[5:-3]) + 2
    raise ValueError(f"not a copper layer: {name}")


def _xy(p) -> Vector2:
    return Vector2.from_xy(int(p[0]), int(p[1]))


def to_kipy_item(item: dict, layers: dict | None, nets: dict | None):
    """Engine item dict (``seg`` / ``arc`` track or ``via``, as ``RouteResult.items``) -> kipy ``Track`` /
    ``ArcTrack`` / ``Via``. All lengths nm. `layers`: engine layer id -> kipy BoardLayer (default: the
    standard mapping); `nets`: net code -> net name (``item["net_name"]`` wins). O(1)."""
    lmap = layers if layers is not None else {}

    def lay(i):
        return lmap[int(i)] if int(i) in lmap else pcbnew_to_kipy_layer(i)

    name = item.get("net_name")
    if name is None:
        code = int(item["net"])
        name = (nets or {}).get(code)
        if name is None:
            raise KeyError(f"no net name for code {code}")
    net = Net(name=name)
    if item.get("kind") == "via":
        v = Via()
        v.position = _xy(item["at"])
        v.diameter = int(item["d"])
        if item.get("drill"):
            v.drill_diameter = int(item["drill"])
        v.net = net
        la = item.get("layers") or []
        if len(la) >= 2:
            top, bot = lay(la[0]), lay(la[-1])
            if (top, bot) != (BoardLayer.BL_F_Cu, BoardLayer.BL_B_Cu):  # blind / buried
                v.type = ViaType.VT_BLIND_BURIED
                v.padstack.drill.start_layer = top
                v.padstack.drill.end_layer = bot
        return v
    p = item["prim"]
    if p["kind"] == "arc":
        t = ArcTrack()
        t.start, t.mid, t.end = _xy(p["start"]), _xy(p["mid"]), _xy(p["end"])
    else:
        t = Track()
        t.start, t.end = _xy(p["a"]), _xy(p["b"])
    t.width = int(item["width"])
    t.layer = lay(item["layer"])
    t.net = net
    return t


# ---------------------------------------------------------------- selection


def _outline_pts(poly_with_holes) -> list:
    pts = []
    for n in poly_with_holes.outline.nodes:
        if n.has_point:
            pts.append((n.point.x, n.point.y))
        elif n.has_arc:  # arc corner: keep its start point (the area is a coarse selection filter)
            pts.append((n.arc.start.x, n.arc.start.y))
    return pts


def selection_from_kipy(items, fp_ref_by_id: dict | None = None) -> Selection:
    """kipy selection items -> :class:`Selection`. `fp_ref_by_id`: footprint KIID string -> reference,
    needed to name selected pads (a pad's ``parent`` is its footprint's id). O(#items)."""
    fp_ref_by_id = fp_ref_by_id or {}
    s = Selection()
    for it in items:
        if isinstance(it, Pad):
            par = it.parent
            ref = fp_ref_by_id.get(par.value if par is not None else "")
            if ref is not None:
                s.pads.append(PadRef(ref, it.number))
        elif isinstance(it, FootprintInstance):
            s.footprints.append(it.reference_field.text.value)
        elif isinstance(it, (Track, ArcTrack)):
            s.tracks.append(it.id.value)
        elif isinstance(it, Via):
            s.vias.append(it.id.value)
        elif isinstance(it, Zone):
            if it.is_rule_area() and s.area is None:
                s.area = _outline_pts(it.outline)
        elif isinstance(it, BoardPolygon):
            polys = list(it.polygons)
            if polys and s.area is None:
                s.area = _outline_pts(polys[0])
    return s


def netclass_values(nc, name: str | None = None) -> NetClassValues:
    return NetClassValues(nc.name if name is None else name, nc.clearance, nc.track_width, nc.via_diameter,
                          nc.via_drill)


def effective_netclass_name(nc) -> str:
    """KiCad's name of an effective net class (``NETCLASS::GetName``): the constituent names joined by ``,`` in
    priority order (``"Signal,Default"``) for a composite, else its own name. The API serialises a composite with
    its internal name ``"Effective for net: <net>"`` (``NETCLASS::Serialize``) and the constituents beside it. O(k)."""
    parts = getattr(nc, "constituents", None) or []
    return ",".join(parts) if len(parts) >= 2 else nc.name


def effective_netclasses(project_classes, per_net: dict) -> tuple:
    """(class name -> NetClassValues, net name -> effective class name) from the project's explicit classes and
    KiCad's per-net answer (``Board.get_netclass_for_nets``: the class KiCad itself resolved for the net from
    explicit assignments, ``netclass_patterns`` and the schematic). A composite is registered under
    :func:`effective_netclass_name` with the values KiCad composed for it (the pcbnew front end,
    ``harness.rules._swig_inputs``, does the same with ``GetEffectiveNetClass``). O(C + N)."""
    classes = {nc.name: netclass_values(nc) for nc in project_classes}
    of: dict = {}
    for net, nc in per_net.items():
        name = effective_netclass_name(nc)
        of[net] = name
        if name != nc.name or name not in classes:
            classes[name] = netclass_values(nc, name)
    return classes, of


# ---------------------------------------------------------------- fingerprint


def board_fingerprint(tracks, vias, footprints) -> str:
    """Item counts + sha256 of the sorted track / via (uuid, position, layer) and footprint (reference,
    position, orientation) lines, from kipy objects (or any object with the same attributes). Moving, adding or
    deleting a track, via or footprint changes it. O(n log n)."""
    lines = []
    for t in tracks:
        end = t.end
        lines.append(f"t {t.id.value} {t.start.x} {t.start.y} {end.x} {end.y} {int(t.layer)}")
    for v in vias:
        lines.append(f"v {v.id.value} {v.position.x} {v.position.y}")
    for f in footprints:
        lines.append(f"f {f.reference_field.text.value} {f.position.x} {f.position.y} {f.orientation.degrees:.4f}")
    lines.sort()
    h = hashlib.sha256("\n".join(lines).encode()).hexdigest()[:16]
    return f"t{len(tracks)}v{len(vias)}f{len(footprints)}:{h}"


# ---------------------------------------------------------------- the board


def _with_document(msg, field: int, doc) -> None:
    """Append ``ItemHeader{document: doc}`` as field ``field`` to ``msg``. KiCad 10.0.7 added this
    header to ``BeginCommit`` (field 1) and ``EndCommit`` (field 4) and refuses a commit without it
    when several editors are open; kicad-python 0.8 has no such field, so it travels as an unknown
    field (KiCad before 10.0.7 skips it)."""
    from kipy.proto.common.types.base_types_pb2 import ItemHeader

    if "header" in msg.DESCRIPTOR.fields_by_name:
        msg.header.document.CopyFrom(doc)
        return
    raw = ItemHeader(document=doc).SerializeToString()
    n, size = len(raw), b""
    while True:
        b, n = n & 0x7F, n >> 7
        size += bytes([b | (0x80 if n else 0)])
        if not n:
            break
    msg.MergeFromString(bytes([(field << 3) | 2]) + size + raw)


def begin_commit(board):
    """``board.begin_commit()`` naming the board's document (:func:`_with_document`). A board
    without a kipy client (test stubs) uses its own ``begin_commit``."""
    if not hasattr(board, "_kicad"):
        return board.begin_commit()
    from kipy.common_types import Commit
    from kipy.proto.common.commands.editor_commands_pb2 import BeginCommit, BeginCommitResponse

    cmd = BeginCommit()
    _with_document(cmd, 1, board.document)
    return Commit(board._kicad.send(cmd, BeginCommitResponse).id)


def end_commit(board, commit, message: str = "", drop: bool = False) -> None:
    """``board.push_commit`` / ``drop_commit`` naming the board's document (field 4 of
    ``EndCommit``); a board without a kipy client uses its own methods."""
    if not hasattr(board, "_kicad"):
        if drop:
            board.drop_commit(commit)
        else:
            board.push_commit(commit, message)
        return
    from kipy.proto.common.commands.editor_commands_pb2 import CommitAction, EndCommit, EndCommitResponse

    cmd = EndCommit()
    cmd.id.CopyFrom(commit.id)
    cmd.action = CommitAction.CMA_DROP if drop else CommitAction.CMA_COMMIT
    cmd.message = message
    _with_document(cmd, 4, board.document)
    board._kicad.send(cmd, EndCommitResponse)


class IpcBoard:
    """BoardPort over kipy. `kicad` is a connected ``kipy.KiCad``; the board is fetched lazily with
    ``kicad.get_board()`` (the board open in the PCB editor that launched the plugin)."""

    def __init__(self, kicad, board=None):
        self._kicad = kicad
        self._board = board
        self._net_names: dict = {}
        self._nets: list | None = None  # one get_nets() answer per run (classes, rules and model share it)

    @classmethod
    def connect(cls, socket_path: str | None = None, token: str | None = None) -> "IpcBoard":
        """Connect to the KiCad that launched the plugin (``KiCad(socket_path, kicad_token, client_name)``) and
        fetch its board; raises (kipy ``ConnectionError`` / ``ApiError``) when the API is off or no board is open.
        The only place the plugin creates a kipy connection (gate review MINOR 3). UNVERIFIED live. O(1) round
        trips."""
        from kipy import KiCad  # noqa: PLC0415

        kicad = KiCad(socket_path=socket_path, kicad_token=token, client_name="topor")
        return cls(kicad, kicad.get_board())

    @property
    def board(self):
        if self._board is None:
            self._board = self._kicad.get_board()
        return self._board

    def project_dir(self) -> Path:
        return Path(self.board.get_project().path)

    def board_name(self) -> str:
        return self.board.name

    def save_copy(self, dst_dir: Path) -> Path:
        """``Board.save_as(dst/name, overwrite=True, include_project=True)``; the .kicad_pro /
        .kicad_dru are copied from the project dir afterwards if save_as did not write them."""
        dst_dir = Path(dst_dir)
        dst_dir.mkdir(parents=True, exist_ok=True)
        out = dst_dir / self.board_name()
        self.board.save_as(str(out), overwrite=True, include_project=True)
        stem = Path(self.board_name()).stem
        for ext in (".kicad_pro", ".kicad_dru"):
            src, dst = self.project_dir() / (stem + ext), dst_dir / (stem + ext)
            if src.is_file() and not dst.exists():
                shutil.copy2(src, dst)
        return out

    def selection(self) -> Selection:
        items = list(self.board.get_selection())
        refs = {}
        if any(isinstance(i, Pad) for i in items):
            refs = {fp.id.value: fp.reference_field.text.value for fp in self.board.get_footprints()}
        return selection_from_kipy(items, refs)

    def read_model(self, rules: dict, pcb_copy: Path, *, layer_roles: dict | None = None) -> dict:
        """``export_model_ipc`` with the shared net codes (:meth:`net_codes`) and the ``.kicad_dru`` of the
        save_as COPY ("" when the copy has none; never the live project dir's file, gate review MINOR 13)."""
        from . import model_ipc  # B3

        pcb_copy = Path(pcb_copy)
        dru = pcb_copy.with_suffix(".kicad_dru")
        model = model_ipc.export_model_ipc(self.board, self.board.get_project(), pcb_copy.read_text(), rules,
                                           name=self.board_name(), layer_roles=layer_roles,
                                           dru_text=dru.read_text() if dru.is_file() else "",
                                           net_codes=self.net_codes())
        self._net_names = {int(n["code"]): n["name"] for n in model.get("nets", [])}
        return model

    def _net_list(self) -> list:
        if self._nets is None:
            self._nets = list(self.board.get_nets())
        return self._nets

    def net_codes(self) -> dict:
        """Net name -> code (``toporlib.rules.net_codes_by_name``) of the board's net list, fetched once and
        shared by :meth:`netclasses`, the rules (``run.resolve_rules``) and :meth:`read_model`. O(N log N)."""
        from toporlib.rules import net_codes_by_name  # noqa: PLC0415

        return net_codes_by_name([n.name for n in self._net_list()])

    def netclasses(self) -> tuple:
        """(class name -> NetClassValues, net name -> effective class name): :func:`effective_netclasses` of the
        project's classes and KiCad's effective class per net. Two IPC round trips."""
        return effective_netclasses(self.board.get_project().get_net_classes(),
                                    self.board.get_netclass_for_nets(self._net_list()))

    def fingerprint(self) -> str:
        """:func:`board_fingerprint` of the live board: three IPC round trips (get_tracks, get_vias,
        get_footprints). UNVERIFIED live (GUI checklist)."""
        b = self.board
        return board_fingerprint(list(b.get_tracks()), list(b.get_vias()), list(b.get_footprints()))

    def commit(self, req: CommitRequest) -> CommitResult:
        board = self.board
        notes: list = []
        commit = begin_commit(board)
        try:
            if req.remove_uuids:
                board.remove_items_by_id([KIID(value=u) for u in req.remove_uuids])
            objs = [to_kipy_item(it, None, self._net_names) for it in req.items]
            created = list(board.create_items(objs)) if objs else []
            if req.zone_fills:
                self._set_fills(req.zone_fills, notes)
            grouped = False
            if req.group and created:
                grouped = self._group(created, req.items, notes)
            end_commit(board, commit, req.message)
        except Exception:
            try:
                end_commit(board, commit, drop=True)
            except Exception:
                pass
            raise
        return CommitResult(removed=len(req.remove_uuids), created=len(created),
                            created_ids=[c.id.value for c in created], grouped=grouped, notes=notes)

    def _set_fills(self, fills: dict, notes: list) -> None:
        """Inside the open commit: the zones whose uuid is a key of `fills` get those filled polygons
        (``CommitRequest.zone_fills``), so Accept hands the board back with fills that hold the new copper in the
        same undo step (KiCad's API reads a zone's ``filled_polygons`` on update, ``ZONE::Deserialize``). A zone
        whose layer name is unknown keeps its fill and a note says so. O(zones + fill points)."""
        from kipy.proto.board import board_types_pb2

        changed = []
        for z in self.board.get_zones():
            polys = fills.get(z.id.value)
            if polys is None:
                continue
            by_layer: dict = {}
            try:
                for p in polys:
                    by_layer.setdefault(BoardLayer.Value("BL_" + p["layer"].replace(".", "_")), []).append(p["pts"])
            except ValueError as e:
                notes.append(f"Zone {z.id.value}: fill not replaced ({e}); refill the zones (B).")
                continue
            del z._proto.filled_polygons[:]
            for layer, rings in by_layer.items():
                fp = board_types_pb2.ZoneFilledPolygons(layer=layer)
                for pts in rings:
                    poly = fp.shapes.polygons.add()
                    for x, y in pts:
                        node = poly.outline.nodes.add()
                        node.point.x_nm, node.point.y_nm = int(x), int(y)
                    poly.outline.closed = True
                z._proto.filled_polygons.append(fp)
            z._proto.filled = bool(by_layer)
            changed.append(z)
        if changed:
            self.board.update_items(changed)

    def _group(self, created: list, items: list, notes: list) -> bool:
        """Put created items into their ``topor`` / ``topor-partial`` groups. UNVERIFIED (kipy 0.8
        ``Group`` + ``create_items``); on any failure the items stay ungrouped and a note says so."""
        try:
            from kipy.board_types import Group
            from kipy.proto.board import board_types_pb2

            by: dict = {}
            for obj, it in zip(created, items):
                if it.get("group"):
                    by.setdefault(it["group"], []).append(obj)
            for name, objs in by.items():
                g = Group(board_types_pb2.Group(name=name))
                g.items = objs
                self.board.create_items(g)
            return bool(by)
        except Exception as e:  # noqa: BLE001 - grouping is best effort
            notes.append(f"Items were not grouped ({type(e).__name__}: {e}); select them by net instead.")
            return False

    def refill_zones(self) -> None:
        self.board.refill_zones()

    def run_action(self, action: str) -> None:
        self._kicad.run_action(action)


__all__ = ["IpcBoard", "to_kipy_item", "selection_from_kipy", "pcbnew_to_kipy_layer", "kipy_to_pcbnew_layer",
           "netclass_values", "effective_netclass_name", "effective_netclasses", "board_fingerprint"]
