"""Shared by the S3c tests: locate the checkout holding scratch/boards and .venv, and the eprom snapshot."""
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]


def main_tree() -> Path:
    """The checkout that holds scratch/ and .venv (this tree, or the main tree when run from a worktree)."""
    for p in (ROOT, *ROOT.parents):
        if (p / "scratch" / "boards").is_dir() and (p / ".venv").is_dir():
            return p
    pytest.skip("no scratch/boards + .venv found")


def eprom_dir() -> Path:
    d = main_tree() / "scratch/boards/eprom-to-kernal/bfaf705c0f25"
    if not d.is_dir():
        pytest.skip("eprom snapshot missing")
    return d
