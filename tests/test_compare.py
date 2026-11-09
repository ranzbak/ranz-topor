"""harness/compare.py (C0): cache keying, the baseline cache, plan metrics from a cut report. No engine runs."""
import json
from pathlib import Path

from harness import compare as cmp

LIM = {"clock": "cpu", "timeout_s": 2400, "threads": 4}


def _pcb(d: Path, text="(kicad_pcb)", pro=None) -> Path:
    d.mkdir(parents=True, exist_ok=True)
    p = d / "b.kicad_pcb"
    p.write_text(text)
    if pro is not None:
        p.with_suffix(".kicad_pro").write_text(pro)
    return p


def test_input_hash_follows_content_not_path(tmp_path):
    a = _pcb(tmp_path / "a", pro="{}")
    b = _pcb(tmp_path / "b", pro="{}")
    assert cmp.input_hash(a) == cmp.input_hash(b)
    b.with_suffix(".kicad_pro").write_text('{"rules": 1}')
    assert cmp.input_hash(a) != cmp.input_hash(b)  # the project file decides the rules
    c = _pcb(tmp_path / "c", text="(kicad_pcb 2)", pro="{}")
    assert cmp.input_hash(a) != cmp.input_hash(c)


def test_settings_hash_is_order_free_and_keys_limits_and_baseline():
    h = lambda sets, **k: cmp.settings_hash(sets, **{**LIM, **k})  # noqa: E731
    assert h(["a.b=1", "c.d=2"]) == h([" c.d=2", "a.b=1 "])
    assert h(["a.b=1", "a.b=2"]) == h(["a.b=2"])  # the last value wins
    assert h(["a.b=1"]) != h(["a.b=2"])
    assert h([]) != h([], threads=8)
    assert h([]) != h([], timeout_s=600)
    assert h([], baseline="x") != h([], baseline="y")


def test_cache_dir_layout(tmp_path):
    assert cmp.cache_dir("ih", "sh", 3, tmp_path) == tmp_path / "ih" / "sh" / "s3"


def test_baseline_runs_once_per_key(tmp_path, monkeypatch):
    calls = []

    def fake_run(tree, pcb, out, seed, sets, *, plan_only, png, name):
        calls.append((seed, plan_only))
        out.mkdir(parents=True, exist_ok=True)
        return {"ok": len(calls) != 2, "rc": 0 if len(calls) != 2 else 1}

    monkeypatch.setattr(cmp, "_run", fake_run)
    monkeypatch.setattr(cmp, "baseline_commit", lambda tree=None: "abc")
    pcb = _pcb(tmp_path / "in")
    kw = dict(ihash="ih", shash="sh", name="b", tree=tmp_path, root=tmp_path / "cache")
    d = cmp.run_baseline(pcb, 0, [], **kw)
    assert cmp.cached(d) and calls == [(0, False)]  # the baseline always runs in full
    assert cmp.run_baseline(pcb, 0, [], **kw) == d and len(calls) == 1  # cached: not run again
    try:
        cmp.run_baseline(pcb, 1, [], **kw)  # a failed run is not a cache hit
        raise AssertionError("no error")
    except RuntimeError:
        pass
    assert not cmp.cached(cmp.cache_dir("ih", "sh", 1, tmp_path / "cache"))
    cmp.run_baseline(pcb, 1, [], **kw)
    assert len(calls) == 3


def _st(i, net, layer, band):
    return {"id": i, "net": net, "layer": layer, "band": band, "conn": i, "w": 0.2, "s": 0.2}


def test_band_crossings():
    h = [[0, 1], [4, 1]]
    v = [[2, 0], [2, 3]]
    assert cmp.band_crossings([_st(0, 1, "F.Cu", h), _st(1, 2, "B.Cu", v)]) == (1, 0)
    assert cmp.band_crossings([_st(0, 1, "F.Cu", h), _st(1, 2, "F.Cu", v)]) == (0, 1)
    assert cmp.band_crossings([_st(0, 1, "F.Cu", h), _st(1, 1, "B.Cu", v)]) == (0, 0)  # same net
    assert cmp.band_crossings([_st(0, 1, "F.Cu", [[0, 0], [2, 0]]), _st(1, 2, "B.Cu", [[2, 0], [2, 3]])]) == (0, 0)  # touch


def test_plan_from_report():
    item = lambda alone: [5, 1, 7, 0.2, 0.5, 0, alone, 0]  # noqa: E731
    final = {
        "layers": [{"layer": "F.Cu", "cuts": [
            {"cap": 1.0, "need": 1.2, "need_floor": 1.1, "slack": -0.2, "items": [item(-0.1)]},   # static
            {"cap": 1.0, "need": 1.2, "need_floor": 0.9, "slack": -0.2, "items": [item(0.3)]},    # neck
            {"cap": 1.0, "need": 1.2, "need_floor": 1.2, "slack": -0.2, "items": [item(0.3)]},    # other
            {"cap": 1.0, "need": 0.5, "need_floor": 0.5, "slack": 0.5, "items": [item(0.3)]},     # fits
        ]}],
        "stretches": [_st(0, 1, "F.Cu", [[0, 1], [4, 1]]), _st(1, 2, "B.Cu", [[2, 0], [2, 3]])],
        "vias": [{"at": [1, 1], "d": 0.6, "site": True, "wire": 0}, {"at": [2, 2], "d": 0.6, "site": False, "wire": 1}],
        "edge_over": [{"a": [0, 0], "b": [1, 1], "layer": "F.Cu"}],
    }
    m = cmp.plan_from_report(final)
    assert m["predicted"]["via_site"] == 1
    assert m["predicted"]["cut_overfull"] == {"total": 3, "static": 1, "neck": 1, "other": 1, "deficit_mm": 0.6}
    assert m["predicted"]["edge_overfull"] == 1
    assert (m["crossings"], m["vias"], m["topo_length_mm"]) == (1, 2, 7.0)


def test_plan_metrics_reads_the_engine_line_of_its_cycle(tmp_path):
    run = tmp_path / "run"
    (run / "cuts" / "run1").mkdir(parents=True)
    (run / "cuts" / "run1" / "cycle1.json").write_text(json.dumps({"final": {"layers": [], "stretches": [], "vias": []}}))
    rp = run / "reports" / "b" / "t"
    rp.mkdir(parents=True)
    line = {"round": 1, "wires": 3, "crossings": 2, "vias": 0}
    (rp / "result.json").write_text(json.dumps({"route": {"warnings": [cmp.PLAN_PREFIX + json.dumps(line)]}}))
    m = cmp.plan_metrics(run, "first")
    assert (m["run"], m["cycle"]) == (1, 1) and m["engine"] == line
