"""harness.learn: metrics of a RouteResult, verdicts, candidate factors and the weight snippet."""
from harness import learn


def _result(items, conns):
    return {"items": items, "connections": conns, "connections_total": len(conns),
            "routed": sum(c["status"] == "routed" for c in conns), "vias": 2, "runtime_s": 1.0}


def _seg(net, layer, a, b):
    return {"net": net, "layer": layer, "prim": {"kind": "seg", "a": a, "b": b}}


def test_samelayer_counts_different_net_proper_crossings_on_one_layer():
    items = [_seg(1, 0, [0, 0], [10, 10]), _seg(2, 0, [0, 10], [10, 0]),  # cross on layer 0
             _seg(3, 2, [0, 10], [10, 0]),                                # other layer: no
             _seg(1, 0, [0, 10], [10, 0])]                                # same net: no
    assert learn.samelayer_crossings(_result(items, [])) == 1


def test_metrics_detour_quantiles():
    conns = [{"status": "routed", "length_nm": l, "ghost_len_nm": 100} for l in (100, 110, 120, 200)]
    conns.append({"status": "failed", "length_nm": 0, "ghost_len_nm": 100})
    m = learn.metrics(_result([], conns))
    assert (m["routed"], m["connections"], m["vias"], m["same_layer"]) == (4, 5, 2, 0)
    assert m["detour_median"] == 1.2 and m["detour_p90"] == 2.0


def test_verdict():
    d = {"routed": 10, "vias": 5, "detour_p90": 1.3, "same_layer": 0}
    assert learn.verdict(d, dict(d, routed=11)) == "better"
    assert learn.verdict(d, dict(d, vias=7)) == "worse"
    assert learn.verdict(d, dict(d, routed=11, vias=7)) == "mixed"
    assert learn.verdict(d, dict(d)) == "same"
    assert learn.verdict(d, dict(d, routed=12, same_layer=1)).startswith("worse")


def _stat(board, hand, engine):
    return {"board": board, "hand": {"events": dict(hand, length_mm=1000.0)},
            "engine": {"totals": {"events": dict(engine, length_mm=1000.0)}}}


def test_candidates_split_penalties_and_preferences():
    st = [_stat("a", {"crossings": 1.0, "vias": 5.0}, {"crossings": 5.0, "vias": 1.0}),
          _stat("b", {"crossings": 1.0, "vias": 5.0}, {"crossings": 3.0, "vias": 2.0}),
          _stat("c", {"crossings": 1.0, "vias": 5.0}, {"crossings": 1.0, "vias": 5.0})]
    c = learn.candidates(st)
    assert [r["event"] for r in c["penalties"]] == ["crossings"]
    assert [r["event"] for r in c["preferences"]] == ["vias"]
    rates = next(r for r in c["all"] if r["event"] == "crossings")["rates"]
    assert rates[0] == {"board": "a", "hand": 0.1, "engine": 0.5}  # per 100 mm of 1000 mm


def test_snippet_and_factor_args():
    s = learn.snippet({"via": 20.0, "crossing": 12.5}, "test")
    assert "[factors]\nweight = { crossing = 12.500, via = 20.000 }\n" in s
    assert learn.factor_args({"via": 20.0}) == ["--factor", "via=20.000000"]
