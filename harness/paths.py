"""Filesystem locations used by the harness (Linux only)."""
import os
from pathlib import Path

MM = 1_000_000  # nm per mm

PROJECTS = Path.home() / "work/kicad/projects"

BENCH_BOARDS = {
    "eprom-to-kernal": PROJECTS / "eprom-to-kernal/eprom-to-kernal/eprom-to-kernal.kicad_pcb",
    "wiccon-2023": PROJECTS / "Wiccon/wiccon-main-badge/2023.kicad_pcb",
    "dcsgonebadge": PROJECTS / "badge.team/dcsgonebadge-0.8.5/dcsgonebadge.kicad_pcb",
    "bornhack2026": PROJECTS
    / "badge.team/bornhack2026/bornhack2026-hardware/bornhack2026-hardware.kicad_pcb",
}


def kicad_cli() -> Path:
    return Path(os.environ.get("KICAD_CLI", "/usr/bin/kicad-cli"))


def plugin_dir(identifier: str = "org.topor.router") -> Path:
    return Path.home() / ".local/share/kicad/10.0/plugins" / identifier


def venv_dir(identifier: str = "org.topor.router") -> Path:
    return Path.home() / ".cache/kicad/10.0/python-environments" / identifier
