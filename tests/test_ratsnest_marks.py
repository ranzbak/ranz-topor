"""K1 (user, qmtech compare PNG): KiCad DRC unconnected items with a Zone end were drawn as lines to the zone's
anchor point (177.031, 44.2303), off the board. A zone end is never drawn at its pos now: pad <-> zone = a ring at
the pad, zone <-> zone = a counted pour piece; the counts reach the captions and the POUR_UNJOINED finding."""
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "plugin"))

from toporlib.drc import ratsnest_caption, ratsnest_marks  # noqa: E402

ANCHOR = {"x": 177.031, "y": 44.2303}


def _it(desc, x, y):
    return {"description": desc, "pos": {"x": x, "y": y}, "uuid": desc}


UNCONNECTED = [  # as in qmtech--drc-routed.json (scratch/verify-d6b3/r1)
    *[{"items": [_it("Zone [GND] on F.Cu", **ANCHOR), _it("Zone [GND] on F.Cu", **ANCHOR)]} for _ in range(3)],
    {"items": [_it("Pad 20 [GND] of ADV7511KSTZ1 on F.Cu", 140.4, 75.3), _it("Zone [GND] on F.Cu", **ANCHOR)]},
    {"items": [_it("Pad 2 [GND] of C54 on B.Cu", 157.0, 96.4), _it("Zone [GND] on B.Cu", **ANCHOR)]},
    {"items": [_it("Pad 1 [SDA] of U1 on F.Cu", 100.0, 50.0), _it("Pad 3 [SDA] of U2 on F.Cu", 110.0, 52.0)]},
]


def test_zone_ends_are_never_drawn_at_their_anchor():
    mk = ratsnest_marks(UNCONNECTED)
    assert mk["lines"] == [(100.0, 50.0, 110.0, 52.0)]
    assert [(x, y) for x, y, _ in mk["rings"]] == [(140.4, 75.3), (157.0, 96.4)]
    assert mk["islands"] == {"GND": 3}
    drawn = [p for ln in mk["lines"] for p in (ln[:2], ln[2:])] + [r[:2] for r in mk["rings"]]
    assert (ANCHOR["x"], ANCHOR["y"]) not in drawn
    assert ratsnest_caption(mk) == " · 3 GND pour pieces cut off, 2 not joined to their pour (rings)"
    assert ratsnest_caption(ratsnest_marks([])) == ""


def test_pour_unjoined_finding_has_the_same_counts():
    from topor.postrun import drc_pour_findings
    (f,) = drc_pour_findings(UNCONNECTED)
    assert f.code == "POUR_UNJOINED" and f.severity == "warning"
    assert "3 GND pour pieces cut off" in f.message and "2 item(s) not joined to their pour" in f.message
    assert "ADV7511KSTZ1" in f.message and "C54" in f.message
    assert drc_pour_findings(UNCONNECTED[-1:]) == []


def test_harness_draw_ratsnest_has_no_line_to_the_anchor(tmp_path, monkeypatch):
    import harness.render as R
    seen = {}
    monkeypatch.setattr(R, "_png_width", lambda p: 1000)
    monkeypatch.setattr(R, "_png_height", lambda p: 1000)
    monkeypatch.setattr(R.subprocess, "run", lambda cmd, check: seen.setdefault("cmd", cmd))
    R.draw_ratsnest(tmp_path / "a.png", tmp_path / "b.png", UNCONNECTED, bbox_nm=(80e6, 30e6, 180e6, 130e6))
    draws = [c for c in seen["cmd"] if isinstance(c, str) and (c.startswith("line") or c.startswith("circle"))]
    ax, ay = (ANCHOR["x"] * 1e6 - 80e6) * 0.00001, (ANCHOR["y"] * 1e6 - 30e6) * 0.00001
    assert not any(f"{ax:.1f},{ay:.1f}" in c for c in draws)
    assert sum(c.startswith("line") for c in draws) == 1
