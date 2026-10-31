"""Checks the synthetic scaling fixture against its spec (no engine needed)."""
import math

import pytest

from tests.scaling.synth import MARGIN, MM, PAD_D, PITCH, REACH, synth_model

SIZES = (10, 100, 1000)
JITTER = 100_000


def _nets(m):
    by_net = {}
    for p in m["pads"]:
        by_net.setdefault(p["net"], []).append(p)
    return by_net


def _centres(m):
    return [tuple(p["center"]) for p in m["pads"]]


def _dist(a, b):
    return math.hypot(a[0] - b[0], a[1] - b[1])


@pytest.mark.parametrize("n", SIZES)
def test_net_and_pad_counts(n):
    m = synth_model(n, 1)
    nets = _nets(m)
    assert len(m["nets"]) == n
    assert len(nets) == n
    assert len(m["pads"]) == 2 * n
    assert all(len(pads) == 2 for pads in nets.values())


@pytest.mark.parametrize("n", SIZES)
def test_every_pair_within_reach(n):
    m = synth_model(n, 1)
    for pads in _nets(m).values():
        assert _dist(pads[0]["center"], pads[1]["center"]) <= REACH


@pytest.mark.parametrize("n", SIZES)
def test_no_two_pads_share_a_centre(n):
    c = _centres(synth_model(n, 1))
    assert len(set(c)) == len(c)


@pytest.mark.parametrize("n", SIZES)
def test_pads_inside_outline_with_margin(n):
    m = synth_model(n, 1)
    xs = [pt[0] for seg in m["outline"] for pt in (seg["a"], seg["b"])]
    ys = [pt[1] for seg in m["outline"] for pt in (seg["a"], seg["b"])]
    lo_x, hi_x, lo_y, hi_y = min(xs), max(xs), min(ys), max(ys)
    for x, y in _centres(m):
        assert x - PAD_D // 2 - lo_x >= MARGIN
        assert hi_x - (x + PAD_D // 2) >= MARGIN
        assert y - PAD_D // 2 - lo_y >= MARGIN
        assert hi_y - (y + PAD_D // 2) >= MARGIN


@pytest.mark.parametrize("n", SIZES)
def test_output_is_deterministic(n):
    assert synth_model(n, 1) == synth_model(n, 1)


def test_jitter_zero_equals_default():
    for n in SIZES:
        assert synth_model(n, 1, jitter_nm=0) == synth_model(n, 1)


def test_default_grid_has_collinear_triples():
    # Sanity for the jitter test below: the exact grid really is collinear.
    c = _centres(synth_model(100, 1))
    found = False
    for i in range(len(c)):
        for j in range(i + 1, len(c)):
            for k in range(j + 1, len(c)):
                (ax, ay), (bx, by), (cx, cy) = c[i], c[j], c[k]
                if (bx - ax) * (cy - ay) - (by - ay) * (cx - ax) == 0:
                    found = True
                    break
            if found:
                break
        if found:
            break
    assert found


def test_jitter_removes_exact_collinearity():
    c = _centres(synth_model(100, 1, jitter_nm=JITTER))
    assert len(c) == 200
    for i in range(len(c)):
        for j in range(i + 1, len(c)):
            (ax, ay), (bx, by) = c[i], c[j]
            for k in range(j + 1, len(c)):
                cx, cy = c[k]
                assert (bx - ax) * (cy - ay) - (by - ay) * (cx - ax) != 0


def test_jitter_offsets_are_integer_and_bounded():
    m = synth_model(100, 1, jitter_nm=JITTER)
    for x, y in _centres(m):
        assert isinstance(x, int) and isinstance(y, int)
        for v in (x, y):
            nominal = round(v / PITCH) * PITCH
            assert abs(v - nominal) <= JITTER


@pytest.mark.parametrize("n", SIZES)
def test_jitter_pairs_still_within_reach(n):
    m = synth_model(n, 1, jitter_nm=JITTER)
    assert len(m["nets"]) == n
    for pads in _nets(m).values():
        assert _dist(pads[0]["center"], pads[1]["center"]) <= REACH


def test_jitter_variant_has_same_contract():
    m = synth_model(100, 1, jitter_nm=JITTER)
    assert len(_nets(m)) == 100
    assert len(set(_centres(m))) == 200
    assert m["units"] == "nm" and MM == 1_000_000
