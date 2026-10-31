"""The kipy names/signatures topor.ipc and model_ipc (B3) rely on exist in the *plugin venv's* kipy.

Runs an inspection script with the KiCad plugin-venv interpreter (read-only; skipped if absent).
A missing name fails the test; the task then adapts the code and records the real name.
"""
import json
import pathlib
import subprocess

import pytest

from harness.paths import venv_dir

ROOT = pathlib.Path(__file__).resolve().parents[2]
PY = venv_dir() / "bin" / "python"

# class -> attributes (properties or methods) that must exist
ATTRS = {
    "kipy.board.Board": ["name", "get_project", "save_as", "begin_commit", "push_commit", "drop_commit",
                         "create_items", "remove_items_by_id", "get_selection", "get_tracks", "get_vias",
                         "get_pads", "get_footprints", "get_shapes", "get_zones", "get_nets", "get_groups",
                         "get_netclass_for_nets", "get_pad_shapes_as_polygons", "get_stackup",
                         "get_enabled_layers", "get_layer_name", "refill_zones", "get_items_by_id",
                         "get_text", "get_item_bounding_box"],
    "kipy.KiCad": ["run_action", "get_board"],
    "kipy.project.Project": ["get_net_classes", "path"],
    "kipy.board_types.Zone": ["filled_polygons", "outline", "layers", "is_rule_area", "net", "name",
                              "locked", "proto"],
    "kipy.board_types.Footprint": ["pads"],
    "kipy.board_types.FootprintInstance": ["reference_field", "position", "orientation", "locked",
                                           "definition", "id"],
    "kipy.board_types.Pad": ["position", "padstack", "number", "net", "pad_type", "parent"],
    "kipy.board_types.PadStack": ["copper_layers", "drill", "angle"],
    # brief lists `layers?`: kipy 0.8 has the singular `layer`
    "kipy.board_types.PadStackLayer": ["shape", "size", "offset", "layer"],
    "kipy.board_types.Track": ["start", "end", "width", "layer", "net", "locked", "id"],
    "kipy.board_types.ArcTrack": ["start", "mid", "end", "width", "layer", "net", "locked", "id"],
    "kipy.board_types.Via": ["position", "padstack", "drill_diameter", "diameter", "net", "locked", "type", "id"],
    "kipy.board_types.Group": ["items", "name", "id"],
    "kipy.project_types.NetClass": ["clearance", "track_width", "via_diameter", "via_drill", "name"],
    "kipy.board_types.Net": ["name"],
    "kipy.board_types.BoardText": ["attributes", "layer", "id"],  # R8: text margin from the text size
    "kipy.common_types.TextAttributes": ["size"],
    "kipy.geometry.Vector2": ["from_xy", "x", "y"],
}
# class -> {method: [parameter names, in order, after self]}
SIGS = {
    "kipy.board.Board": {"save_as": ["filename", "overwrite", "include_project"], "push_commit": ["commit", "message"],
                         "drop_commit": ["commit"], "create_items": ["items"], "remove_items_by_id": ["items"],
                         "get_selection": ["types"], "get_netclass_for_nets": ["nets"],
                         "refill_zones": ["block"]},
    "kipy.KiCad": {"run_action": ["action"]},
}
# proto message -> field paths
PROTO = {
    "kipy.proto.board.board_types_pb2.Zone": ["rule_area_settings.keepout_tracks", "rule_area_settings.keepout_vias"],
    "kipy.proto.board.board_types_pb2.Group": ["name", "items"],
    "kipy.proto.common.types.base_types_pb2.GraphicRectangleAttributes": ["corner_radius"],  # R8: KiCad 10 rounded rect
}
# module -> names
MODS = {
    "kipy.board_types": ["BoardLayer", "ViaType", "BoardPolygon", "Group", "Net", "Track", "ArcTrack", "Via"],
    "kipy.proto.common.types": ["KIID", "KiCadObjectType"],
}

SCRIPT = r"""
import importlib, inspect, json, sys
spec = json.loads(sys.argv[1])
miss = []
def load(path):
    parts = path.split('.')
    for i in range(len(parts), 0, -1):
        try:
            m = importlib.import_module('.'.join(parts[:i]))
        except ImportError:
            continue
        o = m
        for p in parts[i:]:
            o = getattr(o, p)
        return o
    raise ImportError(path)
for cls, names in spec['attrs'].items():
    try:
        c = load(cls)
    except Exception as e:
        miss.append(f'{cls}: {e!r}'); continue
    for n in names:
        if not hasattr(c, n):
            miss.append(f'{cls}.{n}')
for cls, ms in spec['sigs'].items():
    c = load(cls)
    for m, params in ms.items():
        got = list(inspect.signature(getattr(c, m)).parameters)[1:]
        if got[:len(params)] != params:
            miss.append(f'{cls}.{m}{got} != {params}')
for pb, paths in spec['proto'].items():
    try:
        c = load(pb)
    except Exception as e:
        miss.append(f'{pb}: {e!r}'); continue
    for path in paths:
        d = c.DESCRIPTOR
        for part in path.split('.'):
            f = d.fields_by_name.get(part)
            if f is None:
                miss.append(f'{pb}.{path}'); break
            d = f.message_type if f.message_type else None
            if d is None and part != path.split('.')[-1]:
                miss.append(f'{pb}.{path}'); break
for mod, names in spec['mods'].items():
    m = importlib.import_module(mod)
    for n in names:
        if not hasattr(m, n):
            miss.append(f'{mod}.{n}')
# the plugin's own adapter must import in this interpreter
sys.path[:0] = [spec['plugin'], spec['root']]
try:
    import topor.ipc, topor.fake, topor.ports
except Exception as e:
    miss.append(f'import topor.ipc: {e!r}')
import kipy
print(json.dumps({'missing': miss, 'kipy': getattr(kipy, '__version__', '?')}))
"""


@pytest.mark.skipif(not PY.exists(), reason=f"plugin venv interpreter not found: {PY}")
def test_kipy_surface():
    spec = json.dumps({"attrs": ATTRS, "sigs": SIGS, "proto": PROTO, "mods": MODS, "plugin": str(ROOT / "plugin"),
                       "root": str(ROOT)})
    r = subprocess.run([str(PY), "-I", "-c", SCRIPT, spec], capture_output=True, text=True, cwd=str(ROOT),
                       env={"PYTHONPATH": str(ROOT)})
    assert r.returncode == 0, r.stderr
    out = json.loads(r.stdout.strip().splitlines()[-1])
    assert out["missing"] == [], out["missing"]
