### Task B2 (S): ports and fakes; `IpcBoard` ids/selection/`save_as`/commit/refill/run_action; kipy surface test — `plugin/topor/ports.py`, `fake.py`, `ipc.py`

**Files:** Create `ports.py`, `fake.py`, `ipc.py`; tests `tests/plugin/test_fake_board.py`, `test_ipc_mapping.py`, `test_kipy_surface.py`.

**Interfaces:**
- Produces:
  ```python
  class BoardPort(Protocol):
      def project_dir(self) -> Path
      def board_name(self) -> str
      def save_copy(self, dst_dir: Path) -> Path        # Board.save_as(str(dst / name), overwrite=True, include_project=True) → .kicad_pcb path
      def selection(self) -> Selection                  # pads: list[PadRef], footprints: list[str], tracks/vias: list[str], area: list[Pt] | None
      def read_model(self, rules: dict, pcb_copy: Path) -> dict   # BoardModel dict (B3)
      def netclasses(self) -> tuple[dict[str, NetClassValues], dict[str, str]]   # by class name; net → class name
      def commit(self, req: CommitRequest) -> CommitResult        # remove_uuids, items (engine Item dicts), message, group
      def refill_zones(self) -> None
      def run_action(self, action: str) -> None
  class UiPort(Protocol):
      def first_run_notice(self, toml_path: Path) -> None
      def preflight(self, findings: list[Finding], summary: RunSummary, cfg: Config) -> Decision   # proceed, accepted_codes, overrides (scope_mode, existing_copper, respect_locked, partial_routes, timeout_s, threads, save_to_toml)
      def progress(self, p: Progress) -> bool           # False = cancelled
      def preview(self, png: Path, report: Report) -> bool
      def error(self, finding: Finding) -> None
  @dataclass class Finding: code: str; severity: Literal["error","warn","info"]; message: str; fix: str; items: list[str] = field(default_factory=list)
  ```
  `FakeBoard(pcb: Path, model: dict, *, live_text: str | None = None)`: `save_copy` writes `live_text or pcb.read_text()` (so tests can make the live state differ from disk), `read_model` returns `model` (tests build it with the harness oracle), `selection` settable, `commit` records requests and applies them to an in-memory item list (`toporlib.pcbfile` on its text, so a second `save_copy` shows the result), `refill_zones`/`run_action` recorded. `ScriptedUi(decisions, accept, cancel_at_iteration)`.
  `IpcBoard(kicad: kipy.KiCad)`: `project_dir` from `board.get_project().path`; `save_copy` via `Board.save_as`; `selection` via `get_selection()` (pads, footprints, tracks/vias; a selected rule-area zone or polygon shape → `area`); `netclasses` via `Project.get_net_classes()` + `Board.get_netclass_for_nets(get_nets())`; `commit` = `begin_commit()` → `remove_items_by_id([KIID…])` → `create_items([Track|ArcTrack|Via…])` → `push_commit(commit, message)`; `refill_zones()`; `run_action(name)`. Groups (`topor`/`topor-partial`): UNVERIFIED in kipy 0.8 → `CommitResult.grouped = False` + report line when unavailable.
  `test_kipy_surface`: asserts, against the installed kipy source via `inspect`, the exact names/signatures used: `Board.save_as(filename, overwrite, include_project)`, `begin_commit() -> Commit`, `push_commit(commit, message)`, `drop_commit`, `create_items`, `remove_items_by_id`, `get_selection(types)`, `get_tracks/get_vias/get_pads/get_footprints/get_shapes/get_zones/get_nets`, `get_netclass_for_nets`, `get_pad_shapes_as_polygons`, `get_stackup`, `get_enabled_layers`, `get_layer_name`, `refill_zones`, `KiCad.run_action(action)`, `Project.get_net_classes/path`, `Zone.filled_polygons/outline/layers/is_rule_area/net/name/locked`, proto `Zone.rule_area_settings.{keepout_tracks, keepout_vias}`, `Footprint.pads`, `FootprintInstance.{reference_field, position, orientation, locked, definition}`, `Pad.{position, padstack, number, net, pad_type}`, `PadStack.{copper_layers, drill, angle}`, `PadStackLayer.{shape, size, offset, layers?}`, `Track.{start,end,width,layer,net,locked,id}`, `ArcTrack.{start,mid,end,…}`, `Via.{position, padstack, drill_diameter, …}`, `NetClass.{clearance, track_width, via_diameter, via_drill, name}`. Any missing name fails the test → the task adapts and records the real name.

- [ ] **Step 1:** `test_kipy_surface` (run inside the KiCad plugin venv interpreter path discovered from `harness.paths.venv_dir()`; skipped if absent). Fix names until green.
- [ ] **Step 2:** `test_fake_save_copy_uses_live_text`, `test_fake_commit_applies_and_records`, `test_scripted_ui_records_calls`; `test_item_to_kipy_track_arc_via` (engine `seg`/`arc`/`via` → kipy objects, nm, width, layer id, net name; pure `to_kipy_item(item, layers, nets)`), `test_selection_from_kipy_items`. Run → FAIL. Implement. PASS. `test_ipc_live_smoke` (only with `KICAD_API_SOCKET`): `save_copy` produces a parsable file. Commit `feat(plugin): board/ui ports, FakeBoard, ScriptedUi, IpcBoard, kipy surface test`.

