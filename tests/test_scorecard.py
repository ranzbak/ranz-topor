"""harness.scorecard: signal vs stitching via split and the median [min–max] spread."""
from harness.scorecard import _spread, via_split

BOARD = """(kicad_pcb (version 20250000)
  (segment (start 10 10) (end 20 10) (width 0.25) (layer "F.Cu") (net 1) (uuid "s1"))
  (via (at 20 10) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 1) (uuid "v1"))
  (via (at 30 30) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 2) (uuid "v2"))
  (via (at 10 10) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 2) (uuid "v3"))
)
"""


def test_via_split_signal_needs_a_track_of_its_net_ending_on_it(tmp_path):
    p = tmp_path / "b.kicad_pcb"
    p.write_text(BOARD)
    # v1: net-1 track ends on it -> signal; v2: nothing ends on it -> stitching;
    # v3: a track end lies on it but of another net -> stitching
    assert via_split(p) == (1, 2)


def test_spread_is_median_with_range():
    assert _spread([1.0, 3.0, 2.0]) == "2.0 [1.0–3.0]"
    assert _spread([1.234], ".2f") == "1.23"
    assert _spread([None]) == "–"


# ---- limits.json (CPU-time limit, user 2026-11-07)
import json  # noqa: E402

from harness import score_runs  # noqa: E402
from harness import scorecard as sc  # noqa: E402


def _run_dir(root, board, k, *, stop="stable", cpu=100.0):
    d = root / board / f"s{k}"
    d.mkdir(parents=True)
    (d / f"{board}--metrics.json").write_text(json.dumps({
        "unconnected_input": 10, "unconnected": 1, "unconnected_hand": 0, "routing_new": 0, "vias": 0,
        "vias_hand": 0, "stop_reason": stop, "router_cpu_s": cpu, "router_s": cpu / 4}))


def _root(tmp_path, name, **lim):
    root = tmp_path / name
    _run_dir(root, "b1", 0, cpu=120.0)
    _run_dir(root, "b1", 1, stop="timeout_cpu", cpu=2400.0)
    base = {"clock": "cpu", "limit_s": 2400, "threads": 4, "jobs": 1, "git_hash": "abc", "engine": "/e",
            "load_avg_start": [1.0, 1.0, 1.0], "load_avg_end": [2.0, 1.0, 1.0]}
    (root / "limits.json").write_text(json.dumps({**base, **lim}))
    return root


def test_scorecard_shows_limits_cpu_and_limit_hits(tmp_path):
    md, rows = sc.scorecard(_root(tmp_path, "a"))
    assert "Limits: clock cpu, limit 2400 s, threads 4, jobs 1, git abc" in md
    assert "load average 1.0 -> 2.0" in md
    assert "| cpu 2400 s | 120 | 2400 | 1 of 2 |" in md
    assert rows["b1"][1]["hit"] is True and rows["b1"][0]["hit"] is False


def test_compare_refuses_different_limits_unless_forced(tmp_path):
    a, b = _root(tmp_path, "a"), _root(tmp_path, "b", clock="wall", limit_s=600)
    text, rc = sc.compare(a, b)
    assert rc == 2 and text.startswith("REFUSED") and "clock: 'cpu' vs 'wall'" in text and "limit_s" in text
    text, rc = sc.compare(a, b, force=True)
    assert rc == 0 and "WARNING (--force)" in text and "| b1 |" in text
    assert sc.main([str(a), "--compare", str(b)]) == 2
    assert sc.main([str(a), "--compare", str(b), "--force"]) == 0


def test_compare_accepts_equal_limits_and_refuses_a_root_without_limits(tmp_path):
    a, b = _root(tmp_path, "a"), _root(tmp_path, "b", git_hash="other", jobs=2)  # only clock / limit / threads count
    assert sc.compare(a, b)[1] == 0
    c = _root(tmp_path, "c")
    (c / "limits.json").unlink()
    text, rc = sc.compare(a, c)
    assert rc == 2 and "missing" in text


def test_score_runs_commands_are_fixed_and_seeded(tmp_path):
    assert (score_runs.CLOCK, score_runs.TIMEOUT_S, score_runs.THREADS, score_runs.JOBS) == ("cpu", 2400, 4, 1)
    c0 = score_runs.run_command("eprom-to-kernal", tmp_path / "s0", 0, clock="cpu", timeout_s=2400)
    c3 = score_runs.run_command("eprom-to-kernal", tmp_path / "s3", 3, clock="cpu", timeout_s=2400)
    assert c0[-4:] == ["--clock", "cpu", "--timeout", "2400"] and "--set" not in c0
    assert c3[-2:] == ["--set", "optimize.seed=3"]


def test_score_runs_dry_run_writes_limits_json(tmp_path, capsys):
    root = tmp_path / "r"
    assert score_runs.main([str(root), "eprom-to-kernal", "--seeds", "2", "--dry-run"]) == 0
    lim = json.loads((root / "limits.json").read_text())
    assert (lim["clock"], lim["limit_s"], lim["threads"], lim["jobs"]) == ("cpu", 2400, 4, 1)
    assert lim["git_hash"] and lim["engine"] and len(lim["load_avg_start"]) == 3 and len(lim["load_avg_end"]) == 3
    assert capsys.readouterr().out.count("route_board.sh") == 3
