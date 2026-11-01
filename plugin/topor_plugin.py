"""Topor KiCad plugin entry point: puts the plugin directory on ``sys.path`` (so ``topor`` and ``toporlib``
import inside KiCad's plugin venv) and hands over to ``topor.app.main()``."""
import os
import site
import sys

# Rewritten by tools/build.sh to the engine version this plugin was shipped with.
SHIPPED_VERSION = "0.4.0"


def _venv_first() -> None:
    """KiCad starts plugins with a PYTHONPATH holding the system dist-packages (for its own pcbnew module), which
    then shadows the venv: Ubuntu's protobuf 4.21 wins over the venv's 5.x and ``import kipy`` fails with
    "cannot import name 'runtime_version' from 'google.protobuf'". Move the venv's site-packages to the front."""
    if sys.prefix == sys.base_prefix:  # not in a venv (tests, python -m): leave sys.path alone
        return
    venv_sites = [p for p in site.getsitepackages() if p.startswith(sys.prefix)]
    for p in reversed(venv_sites):
        if p in sys.path:
            sys.path.remove(p)
        sys.path.insert(0, p)


_venv_first()
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.insert(0, _HERE)


def main() -> int:
    from topor.app import main as app_main

    return app_main(sys.argv[1:], shipped_version=SHIPPED_VERSION)


if __name__ == "__main__":
    sys.exit(main())
