"""harness.cleanup_check: KiCad's Cleanup Tracks & Vias replayed with pcbnew, one small board per rule."""
import pytest

pcbnew = pytest.importorskip("pcbnew")

from harness import cleanup_check  # noqa: E402
from harness.mechfix import MM, Fx, _v  # noqa: E402


def _track(f: Fx, net: str, a, b, w=0.25, layer=None):
    t = pcbnew.PCB_TRACK(f.b)
    t.SetStart(_v(*a))
    t.SetEnd(_v(*b))
    t.SetWidth(int(w * MM))
    t.SetLayer(pcbnew.F_Cu if layer is None else layer)
    t.SetNet(f.nets[net])
    f.b.Add(t)
    return t


def _board(tmp_path, build) -> str:
    f = Fx(30, 20, nets=("SIG", "OTHER"))
    f.pad("A1", "SIG", (3.0, 10.0), (1.0, 1.0))
    f.pad("B1", "SIG", (27.0, 10.0), (1.0, 1.0))
    build(f)
    return str(f.save(tmp_path / "b.kicad_pcb"))


def _rules(pcb, **kw):
    return [i["rule"] for i in cleanup_check.cleanup_items(pcb, **kw)]


def test_clean_trace_is_stable(tmp_path):
    pcb = _board(tmp_path, lambda f: (_track(f, "SIG", (3, 10), (15, 10)), _track(f, "SIG", (15, 10), (27, 12)),
                                      _track(f, "SIG", (27, 12), (27, 10))))
    assert _rules(pcb) == []


def test_collinear_pair_is_merged(tmp_path):
    pcb = _board(tmp_path, lambda f: (_track(f, "SIG", (3, 10), (15, 10)), _track(f, "SIG", (15, 10), (27, 10))))
    assert _rules(pcb) == ["merge_collinear"]


def test_duplicate_and_in_pad(tmp_path):
    def build(f):
        _track(f, "SIG", (3, 10), (15, 10.5))
        _track(f, "SIG", (15, 10.5), (27, 10))
        _track(f, "SIG", (15, 10.5), (3, 10))      # duplicate (reversed)
        _track(f, "SIG", (26.9, 10), (27.1, 10))   # wholly inside B1
    pcb = _board(tmp_path, build)
    assert sorted(_rules(pcb)) == ["duplicate_segment", "track_in_pad"]


def test_short_cascades_into_dangling(tmp_path):
    """A track over a pad of another net is removed as a short; its neighbour then dangles and goes too."""
    def build(f):
        f.pad("X1", "OTHER", (15.0, 10.0), (1.0, 1.0))
        _track(f, "SIG", (3, 10), (15, 10))
        _track(f, "SIG", (15, 10), (20, 14))
        _track(f, "SIG", (20, 14), (27, 10))
    pcb = _board(tmp_path, build)
    r = cleanup_check.cleanup_items(pcb)
    assert [i["rule"] for i in r] == ["shorting", "shorting", "dangling_track"], r
    alone = cleanup_check.cleanup_items(pcb, opts={k: k == "unconnected" for k in cleanup_check.OPTIONS})
    assert alone == []


def test_short_segment_next_to_a_tee_dangles(tmp_path):
    """qmtech ADV_DVDD: a 43 µm segment whose far end stops on the middle of a trunk; the trunk and the segment's
    other neighbour both reach both of its ends, each counts for one end only, so it is judged dangling
    (TestTrackEndpointDangling). With the trunk split at the T it is stable."""
    def build(f, split):
        f.pad("C1", "SIG", (15.0, 2.0), (1.0, 1.0))
        _track(f, "SIG", (3, 10.0306), (14.9694, 10.0306))
        _track(f, "SIG", (14.9694, 10.0306), (15, 10))
        if split:
            _track(f, "SIG", (15, 2), (15, 10))
            _track(f, "SIG", (15, 10), (15, 18))
        else:
            _track(f, "SIG", (15, 2), (15, 18))
        _track(f, "SIG", (15, 18), (27, 10))
    assert "dangling_track" in _rules(_board(tmp_path, lambda f: build(f, False)))
    assert _rules(_board(tmp_path, lambda f: build(f, True))) == []
