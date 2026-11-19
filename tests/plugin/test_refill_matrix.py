"""DoD item 13 matrix (from the verifier of refill-default r1) for ZONE_FILLS_SKIPPED and the
refill default flip. commit_fills x refill_after_accept (on / off / on-but-fails) x skip path (none, no_copy,
other_items, not_applied all, not_applied partial): finding, result.json, report Outcome line, error dialogs."""
import json
import re

import pytest

from tests.plugin.test_run_flow import CFG, app, do_run, eprom, make_board, reports  # noqa: F401
from topor import run as run_mod

BOGUS = {"no-such-zone-uuid": [{"layer": "F.Cu", "pts": [[0, 0], [1, 0], [1, 1]]}]}


def _skip_patch(monkeypatch, skip):
    real = run_mod._routed_fills
    if skip == "none":
        return
    if skip in ("no_copy", "other_items"):
        monkeypatch.setattr(run_mod, "_routed_fills", lambda *a: ({}, skip))
    elif skip == "na_all":
        monkeypatch.setattr(run_mod, "_routed_fills", lambda *a: (dict(BOGUS), None))
    elif skip == "na_partial":
        def part(*a):
            f, s = real(*a)
            assert f and s is None
            return {**f, **BOGUS}, None
        monkeypatch.setattr(run_mod, "_routed_fills", part)


CASES = [(False, r, "off") for r in ("on", "off", "fails")] + \
        [(True, r, s) for r in ("on", "off", "fails") for s in ("none", "no_copy", "other_items", "na_all", "na_partial")]


@pytest.mark.parametrize("commit,refill,skip", CASES, ids=[f"commit{int(c)}-{r}-{s}" for c, r, s in CASES])
def test_refill_matrix(app, eprom, tmp_path, reports, monkeypatch, commit, refill, skip):  # noqa: F811
    _skip_patch(monkeypatch, skip)
    cfg = CFG.with_values({("run", "commit_fills"): commit, ("run", "refill_after_accept"): refill != "off"})
    board = make_board(eprom)
    if refill == "fails":
        def boom():
            raise RuntimeError("AS_BUSY")
        board.refill_zones = boom
    from topor.fake import ScriptedUi
    _b, ui, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=True), cfg=cfg, board=board)
    assert out.status == "accepted"
    refilled = refill == "on"
    assert out.report.facts["zones_refilled"] is refilled
    warn = commit and skip != "none" and not refilled
    zf = [f for f in out.findings if f.code == "ZONE_FILLS_SKIPPED"]
    assert len(zf) == int(warn), out.findings
    key = {"na_all": "not_applied", "na_partial": "not_applied"}.get(skip, skip)
    if commit:
        assert out.report.facts["zone_fills_skipped"] == ("" if skip == "none" else key)
    else:
        assert out.report.facts["zone_fills_skipped"] == "off"
    # result.json carries the finding with its key
    res = json.loads((out.report_dir / "result.json").read_text())
    rz = [f for f in res["findings"] if f["code"] == "ZONE_FILLS_SKIPPED"]
    assert len(rz) == int(warn)
    if warn:
        assert list(rz[0]["items"]) == [key]
        why = {"no_copy": "refilled copy of the routed result is missing", "other_items": "judged for other items",
               "na_all": "1 of 1 zone fill(s) could not be written", "na_partial": "could not be written"}[skip]
        assert why in zf[0].message, zf[0].message
        if skip == "na_partial":
            assert re.search(r"1 of \d+ zone fill\(s\)", zf[0].message) and "1 of 1 " not in zf[0].message
        # never tell the user to set what is already true
        assert ("refill_after_accept = true" in zf[0].fix) == (refill == "off"), zf[0].fix
        assert "Fill All Zones (B)" in zf[0].fix
    # report Outcome line
    md = (out.report_dir / "report.md").read_text()
    line = [ln for ln in md.splitlines() if "Outcome:" in ln][0]
    assert "one undo step" not in line  # the refill makes it two: no claim of either in the head of the line
    if refilled:
        assert "zones refilled by topor" in line
    elif warn:
        assert "zone fills NOT written" in line and "press B" in line
    elif commit and skip == "none":
        assert "same undo step" in line
    else:
        assert "zones not refilled: press B" in line
    # fail fast, no finished window: one error dialog per step that failed after the commit, in order
    errs = [c[1][0].code for c in ui.calls if c[0] == "error"]
    assert errs == (["POST_COMMIT_STEP_FAILED"] if refill == "fails" else []) + (["ZONE_FILLS_SKIPPED"] if warn else []), errs
    assert ui.names()[-1] == ("error" if errs else "preview")
