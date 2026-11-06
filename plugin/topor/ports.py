"""Ports of the plugin core: ``BoardPort`` (everything that touches KiCad) and ``UiPort`` (everything the
user sees). The core (preflight / run / apply) depends only on these Protocols, so it runs headless against
``FakeBoard`` / ``ScriptedUi`` (fake.py) and for real against ``IpcBoard`` (ipc.py) / the wx UI (B9).

No pcbnew (M4-R1). Dataclass construction is O(1); containers are plain lists / dicts.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Protocol, runtime_checkable

from .config import Config
from .findings import Finding  # re-exported: the one Finding type of the plugin

__all__ = ["Finding", "Pt", "PadRef", "Selection", "NetClassValues", "CommitRequest", "CommitResult",
           "RunSummary", "Decision", "Progress", "Report", "BoardPort", "UiPort"]

Pt = tuple  # (x_nm, y_nm)


@dataclass(frozen=True)
class PadRef:
    """A pad named by footprint reference and pad number (as in ``model["pads"]``)."""
    ref: str
    number: str


@dataclass
class Selection:
    """What is selected in the KiCad PCB editor. `area` is the outline of a selected rule-area zone or
    polygon shape (nm points), else None."""
    pads: list = field(default_factory=list)  # list[PadRef]
    footprints: list = field(default_factory=list)  # references
    tracks: list = field(default_factory=list)  # uuids (straight + arc)
    vias: list = field(default_factory=list)  # uuids
    area: list | None = None  # list[Pt] | None

    @property
    def empty(self) -> bool:
        return not (self.pads or self.footprints or self.tracks or self.vias or self.area)


@dataclass(frozen=True)
class NetClassValues:
    """Net-class values in nm; ``None`` = the class does not set it (KiCad then uses the default class)."""
    name: str
    clearance: int | None = None
    track_width: int | None = None
    via_diameter: int | None = None
    via_drill: int | None = None


@dataclass
class CommitRequest:
    """One undo step: delete `remove_uuids`, create `items` (engine item dicts, as RouteResult.items),
    push with `message`. `group` = also put the new items into their ``group`` (``topor`` /
    ``topor-partial``) when the port can."""
    remove_uuids: list = field(default_factory=list)
    items: list = field(default_factory=list)
    message: str = "Topor: route"
    group: bool = True


@dataclass
class CommitResult:
    removed: int = 0
    created: int = 0
    created_ids: list = field(default_factory=list)  # uuids of the created items, input order
    grouped: bool = False  # new items were put into their topor group
    notes: list = field(default_factory=list)  # report lines (e.g. why grouped is False)


@dataclass
class RunSummary:
    """Header line of the pre-flight dialog (46 wireframe §2)."""
    connections: int = 0
    nets: int = 0
    layers: int = 0
    engine_version: str = ""
    board: str = ""  # board name for the dialog titles
    has_selection: bool = False  # the KiCad selection contains footprints (enables the scope radios)
    connections_by_scope: dict = field(default_factory=dict)  # "whole"|"between"|"touching" -> count (live header)


@dataclass
class Decision:
    """Pre-flight answer. ``overrides`` holds any of ``scope_mode, existing_copper, respect_locked,
    partial_routes, timeout_s, threads`` (config key -> value); ``save_to_toml`` persists them."""
    proceed: bool
    accepted_codes: tuple = ()
    overrides: dict = field(default_factory=dict)
    save_to_toml: bool = False


@dataclass
class Progress:
    """Python mirror of the engine's ``control::Progress`` (JSON of ``RouteJob.progress()``)."""
    phase: str = ""
    iteration: int = 0
    routed: int = 0
    total: int = 0
    vias: int = 0
    elapsed_s: float = 0.0  # router time so far (what run.timeout_s limits); the run flow fills it in
    stage: str = ""  # engine: what the topological core does now ("drawing the traces", ...)
    round: int = 0  # engine: whole-route round now running, 0 = unknown
    rounds: int = 0  # engine: most rounds ([optimize] rounds)
    wall_s: float = 0.0  # plugin: wall time since the run started (KiCad DRC and preview included)

    @staticmethod
    def from_json(d: dict) -> "Progress":
        return Progress(str(d.get("phase", "")), int(d.get("iteration", 0)), int(d.get("routed", 0)),
                        int(d.get("total", 0)), int(d.get("vias", 0)), float(d.get("elapsed_s", 0.0)),
                        str(d.get("stage", "") or ""), int(d.get("round", 0) or 0), int(d.get("rounds", 0) or 0))


@runtime_checkable
class Report(Protocol):
    """Run report (B8). The ports need the summary block and the report folder (preview shows and opens it)."""
    dir: Path

    def summary_lines(self) -> list: ...


class BoardPort(Protocol):
    def project_dir(self) -> Path: ...
    def board_name(self) -> str: ...

    def save_copy(self, dst_dir: Path) -> Path:
        """Like ``Board.save_as(dst/name, overwrite=True, include_project=True)``: the live board
        and its project files (.kicad_pro / .kicad_dru) in `dst_dir`; returns the .kicad_pcb path."""

    def selection(self) -> Selection: ...
    def read_model(self, rules: dict, pcb_copy: Path, *, layer_roles: dict | None = None) -> dict:
        """BoardModel dict. `layer_roles` = ``cfg.layer_roles`` (copper layer name -> role); ports whose model
        is fixed (FakeBoard) ignore it."""

    def netclasses(self) -> tuple:
        """(class name -> NetClassValues, net name -> class name)."""

    def net_codes(self) -> dict | None:
        """Net name -> code of the board's ONE net list, used for the rules' ``per_net`` keys and the model's
        ``nets`` (gate review MINOR 13); None = the port's model carries its own codes (FakeBoard), the rules
        then derive them from :meth:`netclasses`."""

    def fingerprint(self) -> str:
        """Cheap board fingerprint (item counts + a hash of track / via uuids + positions and footprint
        references + positions + orientations). The run flow takes it at the start and again right before the
        commit; a difference means the board was edited during the run (BOARD_CHANGED_DURING_RUN)."""

    def commit(self, req: CommitRequest) -> CommitResult: ...
    def refill_zones(self) -> None: ...
    def run_action(self, action: str) -> None: ...


class UiPort(Protocol):
    def first_run_notice(self, toml_path: Path) -> None: ...
    def preflight(self, findings: list, summary: RunSummary, cfg: Config) -> Decision: ...

    def progress(self, p: Progress) -> bool:
        """Called every ~0.25 s; return False to cancel the run."""

    def preview(self, png: Path, report: Any) -> bool:
        """Show the result; True = Accept (apply to the board), False = Reject."""

    def done(self, outcome: str, report: Any, report_dir: Path) -> None:
        """Post-Accept/Reject notice (wireframe §5). `outcome`: ``accepted`` | ``rejected`` |
        ``cancelled``."""

    def error(self, finding: Finding) -> None: ...

    def set_context(self, board: str | None = None, report_dir: Any = None, log_path: Any = None,
                    versions: dict | None = None) -> None:
        """Board name, report folder, log.txt path and version strings the dialogs show; the run flow calls
        it before ``preview`` / ``done`` / ``error``."""
