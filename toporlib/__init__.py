"""Pure-Python code shared by the KiCad plugin and the dev harness.

No module in this package may import `pcbnew` (directly or transitively); enforced by
tests/plugin/test_no_pcbnew.py. Board I/O here is kicad-cli, files and callables only.
"""
