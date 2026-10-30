import pathlib

import pytest

ROOT = pathlib.Path(__file__).resolve().parents[1]


@pytest.fixture(scope="session")
def root():
    return ROOT


@pytest.fixture(scope="session")
def engine():
    """Import the topor_engine wheel; skip if it is not built into the venv."""
    try:
        import topor_engine
    except ImportError:
        pytest.skip("topor_engine wheel not built (run maturin develop in engine/)")
    return topor_engine
