import pathlib
import sys

_PLUGIN = str(pathlib.Path(__file__).resolve().parents[2] / "plugin")
if _PLUGIN not in sys.path:
    sys.path.insert(0, _PLUGIN)
