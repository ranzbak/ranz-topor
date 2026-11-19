"""topor test / measurement harness. Importing it isolates KiCad from the user's config before anything imports
pcbnew: the process's KiCad location variables and every kicad-cli the toporlib runner starts (harness/kicad_env.py).
Entry points (route_board, strip, bench, score_runs, gui_e2e, ...) import harness first by their package path."""
from harness import kicad_env as _kicad_env

_kicad_env.install()
