# Task B3 report: IPC model exporter, pure rules, dru disallow keepouts, parse_layers

Branch: worktree-agent-a6855a687daba04eb (merged `main` 4dc5cea first: the worktree base predated B2).

## What changed
- `toporlib/pcbfile.py`: `parse_layers(text)`.
- `toporlib/rules.py` (new): pure `resolve_rules_from_project`, `classes_from_pro`, `netclass_of_from_pro`,
  `rule_values_from_pro`, `net_codes_by_name`, `NetClassVals`, `JLCPCB_1OZ`.
- `toporlib/dru.py`: `DruRule.disallow`, `dru_keepouts`, `merge_dru_keepouts`, `_disallow_areas`.
- `toporlib/geom.py` (new): hull, arc/capsule/circle/bezier flattening, point-in-ring, `track_item`, `find_dangling`
  (moved out of harness/model.py so harness and IPC exporter share one dangling-end rule).
- `harness/rules.py`: SWIG wrapper (`_swig_inputs` -> `resolve_rules_from_project`); re-exports unchanged names.
- `harness/model.py`: `export_model(..., dru_text=None)` merges dru keepouts; dangling ends via `toporlib.geom`.
  `harness/bench.py`, `harness/measure.py` now pass their `dru_text` (no snapshot board has a `disallow` rule: behaviour unchanged).
- `plugin/topor/model_ipc.py` (new): the IPC exporter and its pure mapping functions.
- Tests: `tests/plugin/test_pcbfile.py` (+3 parse_layers), `tests/test_rules.py` (+8), `tests/plugin/test_model_ipc.py` (15),
  `tests/plugin/test_ipc_parity.py` (2 offline self-tests + 6 live parametrised, skipped without KICAD_API_SOCKET).

## Public signatures
```
parse_layers(text: str) -> list[dict]          # [{"id": pcbnew copper id, "name", "type": signal|power|mixed|jumper}], F.Cu, In*, B.Cu
                                               # ids derived from the NAME (legacy files number B.Cu 31; wiccon-2023 snapshot is legacy)
layer_plan(pcb_copy_text: str, cfg_roles: dict[str,str]) -> tuple[list[dict], list[Finding]]   # plugin/topor/model_ipc.py
   entries: {"id","name","kind"(KiCad type),"role","role_source": config|kicad|default|none,"routable": role in (signal,mixed)}
   unresolved layer: role None, routable False + LAYER_ROLES_NEEDED finding.
resolve_rules_from_project(pro: dict, dru_text: str|None, *, netclass_of: dict[str,str], classes: dict[str,NetClassVals-like],
      widths_before, settings_table=None, profile=JLCPCB_1OZ, apply_fab_floors=True, copper_layer_count=2,
      net_codes: dict[str,int]|None=None) -> dict
   # net_codes (name->code) is an addition: per_net is keyed by code. Default = net_codes_by_name(netclass_of) (sorted names, 1-based),
   # the SAME assignment export_model_ipc uses for model["nets"].
export_model_ipc(board, project, pcb_copy_text: str, rules: dict, *, name: str,
      layer_roles: dict|None=None, dru_text: str|None=None) -> dict
```
Other additions: `toporlib.rules.netclass_of_from_pro(pro, net_names)` (pattern/regex/assignment matching from the .kicad_pro; used by the equality
tests and a usable fallback for FakeBoard), `classes_from_pro(pro)`.

## Inputs IpcBoard.read_model must pass (ipc.py NOT edited; its current call still works with defaults)
1. `rules` resolved with `toporlib.rules.resolve_rules_from_project(pro, dru_text, netclass_of=<netclasses()[1] plus "" -> "Default">,
   classes=<netclasses()[0]>, ..., copper_layer_count=board.get_copper_layer_count())` and default `net_codes`
   (so `per_net` keys match `model["nets"]` codes). `pro` = json of `<project>.kicad_pro` from the save_as copy.
2. `pcb_copy_text` = text of the save_as copy (layer types).
3. `layer_roles=cfg.layer_roles` (read_model currently does not forward it: without it roles come from KiCad types / defaults only;
   the port protocol `read_model(rules, pcb_copy)` has no slot, so B-core/B7 must either extend it or call `layer_plan` itself and
   patch `model["layers"]`). `layer_plan` findings (LAYER_ROLES_NEEDED) are not returned by `export_model_ipc`; call `layer_plan` for them.
4. `dru_text`: optional; default is `<project.path>/<board stem>.kicad_dru` if present.
5. Call `board.refill_zones()` first, otherwise pours fall back to their outline with a `warning`.

## Tests run
- `.venv/bin/pytest -q tests/plugin tests/test_rules.py tests/test_model_export.py` -> 113 passed, 7 skipped (6 live parity + 1 pre-existing), 3 deselected.
- `.venv/bin/pytest -q tests/gate/test_m3_gate.py -k eprom -m gate` -> 1 passed (46 s).
- Full default suite: see FULL_SUITE line at the end of this file.
- `test_resolve_rules_pure_equals_swig` (+ eprom, wiccon snapshots): pure `resolve_rules_from_project` fed only from the .kicad_pro JSON +
  net names equals the SWIG result dict-for-dict (including warnings order, floors_applied, per_net).
- `test_dru_disallow_rule_becomes_keepout`, `test_dru_disallow_skips_and_warns`, `test_harness_export_merges_dru_keepouts`.
- `test_export_model_ipc_end_to_end_and_engine_loads_it`: stub kipy Board (protos) -> model -> `topor_engine.check_items` accepts it
  with the extra layer keys. Layer roles: 4-layer synthetic text gives S, power, power, S (In1/In2 not routable); with all-`signal` types the
  default stackup S, gnd, pwr, S; config override and the LAYER_ROLES_NEEDED case are tested.

## Decisions and deviations (please review)
- **Pad rotation = `padstack.angle` only**, not "footprint orientation + padstack angle" as the brief says: kipy's own `FootprintInstance.orientation`
  setter adds the delta to `padstack.angle`, i.e. it is absolute (as pcbnew's `GetOrientation`). Adding the footprint angle would double count. UNVERIFIED live.
- **Net codes**: IPC has none (deprecated); sorted-name 1..N (`net_codes_by_name`). Parity compares by net name. In `test_ipc_parity` the harness
  `rules` (pcbnew codes) are passed to the IPC exporter only because rules are not compared there.
- **`free` via flag** is not in the IPC API: approximated as "no same-net track end touches the via". Not compared in parity.
- **Copper text** (no polygon API): its bounding box (`get_item_bounding_box`). **Copper graphics** polygonised here (capsule chains), not KiCad's
  polygons: parity for `copper_shapes` is (uuid, layer) sets + bbox within 2*max_error, not 0 nm. Outline: seg/arc/rect exact; circle/bezier/polygon
  flattened by `toporlib.geom` (not point-identical to KiCad; parity compares total length within 1% for those).
- **Keepout layers** are copper layers only (harness lists every layer in the zone's layer set). **Layer names** are the canonical file names
  (`In1.Cu`), the harness uses user-renamed names.
- **Pour reach** (pads/vias) is geometric: probe points of the pad (centre, corners, edge midpoints) inside the fill (even-odd, all rings). The harness used
  pcbnew connectivity; M4-R6 makes the kicad-cli DRC the real oracle. Thermal-relief pads rely on the edge-midpoint probes touching spokes: UNVERIFIED.
- **dru `disallow`**: supported shape = `(constraint disallow track|via ...)` with a condition of `A|B.insideArea('x')` / `intersectsArea('x')` terms (and `&&` with
  `A.Type == '...'`), optional `(layer "F.Cu"|outer|inner)`. One keepout `dru:<rule>` per outline of every rule area named `x`. Anything else
  (no condition, `||`, `!`, unknown area) -> `REFUSE:` warning in `rules["warnings"]` (area-less case from `resolve_rules_from_project`, unknown area from the merge) and no keepout.
- `NetClassVals` in toporlib.rules duplicates `topor.ports.NetClassValues` (toporlib must not import plugin); the resolver duck-types on attributes.
- `harness/bench.py` / `measure.py` now pass `dru_text` to `export_model`.

## UNVERIFIED (live-only kipy calls; GUI checklist)
- `Board.get_enabled_layers()`, `get_nets()`, `get_footprints()` (+ `definition.items` carrying pads/shapes/zones/text with absolute coordinates),
  `get_tracks()`, `get_vias()`, `get_shapes()`, `get_text()`, `get_zones()` (rule areas + `filled_polygons` populated after `refill_zones`).
- `Board.get_pad_shapes_as_polygons(pad, layer)` (single-pad form; absolute coordinates assumed, holes ignored, outline hull taken) for custom/trapezoid/chamfered/offset pads.
- `Board.get_item_bounding_box(text)` for copper text.
- Pad fields as the server serialises them: `padstack.angle` absolute; `padstack.layers` listing all copper layers of a PTH; `padstack.copper_layers`
  (PST_NORMAL = single entry for all layers); `copper_clearance_override` set only when overridden; `zone_settings.thermal_spokes.angle/gap` present only when set
  (absent angle -> key omitted -> engine uses 90); footprint-embedded rule-area zones inside `definition.items`; `definition.net_ties[].pad_number`;
  zone `copper_settings.connection.thermal_spokes.gap/width` presence (defaults 0.5 mm otherwise).
- Via `padstack.copper_layers[0].size.x` as the diameter; blind/buried span from `padstack.drill.start_layer/end_layer`.
- `test_ipc_parity` itself (needs the snapshot board open in KiCad; pytest skips otherwise).

## Parity tolerance used (M4-R5)
Pads: polygon-equivalent within 1 um (same native kind -> exact numbers, roundrect radius within 1 um; mixed kinds compared as polygons, circles tessellated at 50 nm);
all other coordinates 0 nm, except the documented approximations above (copper_shapes bbox 2*max_error, flattened outline length 1%, pour fill area 0.5%).
Per-board live results: none yet (no KICAD_API_SOCKET here); the user runs the six boards per the GUI checklist.

## Concerns
- Live-only paths are untested against a real server; the field-presence assumptions above are the likely first failures.
- `layer_roles` is not forwarded by `IpcBoard.read_model` (see inputs, item 3).

FULL_SUITE: .venv/bin/pytest -q -> 242 passed, 7 skipped, 22 deselected in 523 s.
