"""``IpcBoard``: the real BoardPort over kicad-python (kipy 0.8) and a running KiCad (IPC API, M4-R1: no
pcbnew). Anything that needs a live KiCad is UNVERIFIED until the GUI checklist ran; the pure mapping
helpers (``to_kipy_item``, ``selection_from_kipy``, ``pcbnew_to_kipy_layer``) are unit-tested.

Complexity: every method is O(#items it transfers) plus one IPC round trip per call (the API has no
batching beyond ``create_items`` / ``remove_items_by_id``, which are used).
"""

from __future__ import annotations

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


def netclass_values(nc) -> NetClassValues:
    return NetClassValues(nc.name, nc.clearance, nc.track_width, nc.via_diameter, nc.via_drill)


# ---------------------------------------------------------------- the board


class IpcBoard:
    """BoardPort over kipy. `kicad` is a connected ``kipy.KiCad``; the board is fetched lazily with
    ``kicad.get_board()`` (the board open in the PCB editor that launched the plugin)."""

    def __init__(self, kicad, board=None):
        self._kicad = kicad
        self._board = board
        self._net_names: dict = {}

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
        from .model_ipc import export_model_ipc  # B3

        model = export_model_ipc(self.board, self.board.get_project(), Path(pcb_copy).read_text(), rules,
                                 name=self.board_name(), layer_roles=layer_roles)
        self._net_names = {int(n["code"]): n["name"] for n in model.get("nets", [])}
        return model

    def netclasses(self) -> tuple:
        classes = {nc.name: netclass_values(nc) for nc in self.board.get_project().get_net_classes()}
        nets = list(self.board.get_nets())
        of = {name: nc.name for name, nc in self.board.get_netclass_for_nets(nets).items()}
        return classes, of

    def commit(self, req: CommitRequest) -> CommitResult:
        board = self.board
        notes: list = []
        commit = board.begin_commit()
        try:
            if req.remove_uuids:
                board.remove_items_by_id([KIID(value=u) for u in req.remove_uuids])
            objs = [to_kipy_item(it, None, self._net_names) for it in req.items]
            created = list(board.create_items(objs)) if objs else []
            grouped = False
            if req.group and created:
                grouped = self._group(created, req.items, notes)
            board.push_commit(commit, req.message)
        except Exception:
            try:
                board.drop_commit(commit)
            except Exception:
                pass
            raise
        return CommitResult(removed=len(req.remove_uuids), created=len(created),
                            created_ids=[c.id.value for c in created], grouped=grouped, notes=notes)

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
           "netclass_values"]
