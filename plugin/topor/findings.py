"""Finding: one pre-flight / config / run message (46 §1.1).

Every finding carries a stable ``code`` (catalogued in messages.py by B6), a severity, the cause in
``message`` and the remedy in ``fix`` (naming the KiCad place or the topor.toml key). ``items`` lists the
affected objects (net names, footprint refs, keys, ...). Constructing a Finding is O(1).
"""

from __future__ import annotations

from dataclasses import dataclass

SEVERITIES = ("info", "warning", "error")


@dataclass(frozen=True)
class Finding:
    code: str
    severity: str  # "info" | "warning" | "error"
    message: str
    fix: str = ""
    items: tuple = ()

    def __post_init__(self):
        if self.severity not in SEVERITIES:
            raise ValueError(f"bad severity {self.severity!r}")
