"""topor test / measurement harness. Importing it isolates every kicad-cli the toporlib runner starts from the
user's KiCad config (harness/kicad_env.py)."""
from harness import kicad_env as _kicad_env

_kicad_env.install()
