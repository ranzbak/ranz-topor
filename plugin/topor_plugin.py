"""Topor KiCad plugin entry point: puts the plugin directory on ``sys.path`` (so ``topor`` and ``toporlib``
import inside KiCad's plugin venv) and hands over to ``topor.app.main()``."""
import os
import sys

# Rewritten by tools/build.sh to the engine version this plugin was shipped with.
SHIPPED_VERSION = "0.4.0"

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.insert(0, _HERE)


def main() -> int:
    from topor.app import main as app_main

    return app_main(sys.argv[1:], shipped_version=SHIPPED_VERSION)


if __name__ == "__main__":
    sys.exit(main())
