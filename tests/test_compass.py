"""M1 compass classifier (toporlib/compass.py): drawn violations against the plan's located items, KiCad DRC items
joined by place, per connection detour and vias planned vs drawn. Synthetic results with known counts."""
from toporlib import compass

MM = 1_000_000


def conn(i, status="routed", length=10, ref=8, vias=0, width=0.25):
    return {"id": i, "name": f"N{i}", "status": status, "width": int(width * MM), "vias": vias,
            "length_nm": int(length * MM), "ref_len_nm": int(ref * MM), "ghost_len_nm": int(ref * MM),
            "from": {"label": f"J1.{i}"}, "to": {"label": f"J2.{i}"}}


def viol(c, at, layer="F.Cu", cls="foreign_copper", kind="clearance"):
    return {"conn": c, "name": f"N{c}", "from": "a", "to": "b", "layer": layer, "at": [int(at[0] * MM), int(at[1] * MM)],
            "kind": kind, "class": cls}


def seg(a, b):
    return [[int(a[0] * MM), int(a[1] * MM)], [int(b[0] * MM), int(b[1] * MM)]]


def route(viols, items, conns, predicted, notes=()):
    return {"connections": conns, "violations": viols,
            "compass": {"model": "cuts", "opt_loop": "topor", "round": 1, "predicted": predicted,
                        "predicted_violations": sum(v for k, v in predicted.items() if k in compass.VIOLATION_CLASSES),
                        "loop_predicted": None, "drawn_shrinkages": 0, "items": items, "notes": list(notes),
                        "conns": [{"conn": c["id"], "vias": 0, "taut_nm": 9 * MM, "necks": 0} for c in conns]}}


def drc(t, pts):
    return {"type": t, "items": [{"pos": {"x": x, "y": y}} for x, y in pts]}


def row(cp, cls):
    return next(r for r in cp["rows"] if r["class"] == cls)


def test_overfull_cut_predicted_and_drawn_there():
    # Case 2 (channel_overfull): one over-full cut with four wires; the drawing violates at the cut (two wires).
    conns = [conn(i) for i in range(4)]
    it = {"class": "planner_overfull", "layers": ["F.Cu"], "conns": [0, 1, 2, 3], "segs": [seg((4.3, 5.6), (5.9, 5.6))],
          "over_nm": 400000}
    v = [viol(1, (5.0, 5.7)), viol(2, (5.3, 5.5))]
    cp = compass.build(route(v, [it], conns, {"planner_overfull": 1, "via_site": 0}),
                       [drc("clearance", [(5.0, 5.7), (5.1, 5.7)])], 0)
    r = row(cp, "planner_overfull")
    assert (r["predicted"], r["predicted_conns"], r["drawn"], r["drawn_conns"], r["kicad"]) == (1, 4, 2, 2, 1)
    assert r["predicted_clean"] == 0
    assert row(cp, "drawing_loss")["drawn"] == 0
    assert row(cp, "kicad_only")["kicad"] == 0
    assert row(cp, "pad_exit")["predicted"] is None  # this build does not predict pad exits
    assert cp["predicted_violations"] == 1 and cp["drawn_violations"] == 2


def test_clean_plan_drawn_violation_is_a_drawing_loss_and_kicad_only_is_a_checker_gap():
    # Case 1 (channel_fit) with a made-up drawn violation: the plan says fits, so the drawing lost it; a DRC item far
    # from any engine violation is "KiCad only".
    conns = [conn(i) for i in range(3)]
    cp = compass.build(route([viol(0, (1, 1), cls="static")], [], conns, {"planner_overfull": 0}),
                       [drc("clearance", [(1.1, 1.0)]), drc("shorting_items", [(8, 8)])], 0)
    assert row(cp, "drawing_loss")["drawn"] == 1 and row(cp, "drawing_loss")["kicad"] == 1
    assert row(cp, "kicad_only")["kicad"] == 1
    assert cp["kicad_only_by_type"] == {"shorting_items": 1}
    assert any(f.startswith("drawing loss") for f in cp["flags"])


def test_model_gap_is_the_sketch_61_direction_and_the_reverse_is_edge_overpredict():
    # §6.1: a bottleneck of conn 0 the cut test finds over-full and the per-edge packer fits = model gap; an edge-layer
    # of conn 1 the per-edge packer finds over-full and the cut test fits = per-edge over-predicted; conn 2 in no item.
    conns = [conn(i) for i in range(3)]
    gap = {"class": "model_gap", "layers": ["F.Cu"], "conns": [0], "segs": [seg((0, 0), (2, 0))], "over_nm": 100000}
    opd = {"class": "edge_overpredict", "layers": ["F.Cu"], "conns": [1], "segs": [seg((5, 0), (7, 0))], "over_nm": 0}
    v = [viol(0, (1, 0.1)), viol(1, (6, 0.1)), viol(2, (12, 0))]
    cp = compass.build(route(v, [gap, opd], conns, {"planner_overfull": 0, "model_gap": 1, "edge_overpredict": 1}),
                       None, None)
    assert (row(cp, "model_gap")["predicted"], row(cp, "model_gap")["drawn"]) == (1, 1)
    assert (row(cp, "edge_overpredict")["predicted"], row(cp, "edge_overpredict")["drawn"]) == (1, 1)
    assert row(cp, "drawing_loss")["drawn"] == 1  # conn 2
    assert row(cp, "planner_overfull")["kicad"] is None  # no DRC run
    # not measured (no cut test): None, shown so, never "missing method step"
    cp = compass.build(route([viol(0, (1, 0.1))], [], conns, {"planner_overfull": 0}), None, None)
    assert row(cp, "model_gap")["predicted"] is None
    assert "not measured" in "\n".join(compass.table_md(cp))
    assert not any("missing method step" in f for f in cp["flags"])


def test_plan_errors_do_not_hide_as_drawing_loss():
    # Over-full bottleneck of conns 0-1 next to conn 2's violation: "over-full nearby"; a violation at a planned neck of
    # conn 3: "at a planned neck". Neither is a drawing loss; both are flagged as plan errors.
    conns = [conn(i) for i in range(4)]
    over = {"class": "planner_overfull", "layers": ["F.Cu"], "conns": [0, 1], "segs": [seg((0, 0), (2, 0))],
            "over_nm": 200000}
    neck = {"class": "shrinkage", "layers": ["F.Cu"], "conns": [3], "segs": [seg((9, 0), (10, 0))], "over_nm": 50000}
    v = [viol(2, (1, 0.2), cls="static"), viol(3, (9.5, 0.1), cls="static")]
    cp = compass.build(route(v, [over, neck], conns, {"planner_overfull": 1, "shrinkage": 1}), None, None)
    assert row(cp, "overfull_nearby")["drawn"] == 1 and row(cp, "neck_loss")["drawn"] == 1
    assert row(cp, "drawing_loss")["drawn"] == 0
    assert row(cp, "overfull_nearby")["predicted"] is None and row(cp, "neck_loss")["predicted"] is None
    assert any("plan error" in f for f in cp["flags"]) and any("necked" in f for f in cp["flags"])
    # (b) drawn clean = no drawn violation within reach, whoever's: the over-full item is NOT clean (conn 2's
    # violation lies on it although it matched it only as "nearby")
    assert row(cp, "planner_overfull")["predicted_clean"] == 0


def test_kicad_join_radius_and_nets_are_options():
    conns = [conn(0), conn(1)]
    v = [viol(0, (1, 0)), viol(1, (1.3, 0))]
    item = {"description": "Track [N1] on F.Cu, length 3 mm", "pos": {"x": 1.25, "y": 0.0}}
    d = [{"type": "clearance", "items": [item]}]
    r = route(v, [], conns, {"planner_overfull": 0})
    by_net = compass.build(r, d, 0)  # the nearest drawn violation is conn 0's (N0), but KiCad names N1
    assert [x["kicad"] for x in by_net["rows"] if x["class"] == "drawing_loss"] == [1]
    assert by_net["join"] == {"radius_mm": 0.5, "nets": True}
    assert compass.build(r, d, 0, drc_radius_mm=0.01)["kicad_only_by_type"] == {"clearance": 1}
    other = [{"type": "clearance", "items": [dict(item, description="Track [N7] on F.Cu")]}]
    assert compass.build(r, other, 0)["kicad_only_by_type"] == {"clearance": 1}  # no drawn violation of N7
    assert compass.build(r, other, 0, join_nets=False)["kicad_only_by_type"] == {}
    md = "\n".join(compass.table_md(compass.build(r, d, 0, drc_radius_mm=0.8)))
    assert "within 0.8 mm" in md and "not 1:1" in md


def test_pad_exit_without_a_plan_object_is_a_missing_method_step():
    cp = compass.build(route([viol(0, (1, 1), cls="pad_exit")], [], [conn(0)], {"planner_overfull": 0}), [], 0)
    r = row(cp, "pad_exit")
    assert r["predicted"] is None and r["drawn"] == 1
    assert any("missing method step" in f for f in cp["flags"])


def test_via_site_and_unlayerable_and_far_items_do_not_match():
    conns = [conn(i) for i in range(3)]
    items = [{"class": "via_site", "layers": [], "conns": [0], "segs": [seg((9, 9), (9, 9))], "over_nm": 0},
             {"class": "unlayerable", "layers": [], "conns": [1, 2], "segs": [seg((3, 3), (3, 3))], "over_nm": 0}]
    v = [viol(0, (1, 1), cls="via_site", layer="all layers"), viol(1, (3.2, 3.1), cls="crossing"),
         viol(2, (7, 7), cls="crossing")]
    cp = compass.build(route(v, items, conns, {"via_site": 1, "unlayerable": 1}), None, None)
    assert row(cp, "via_site")["drawn"] == 1  # matched by connection (a via has no layer)
    assert row(cp, "unlayerable")["drawn"] == 1  # conn 1 near the crossing point
    assert row(cp, "drawing_loss")["drawn"] == 1  # conn 2 far from it
    assert row(cp, "unlayerable")["predicted_clean"] == 0


def test_per_connection_detour_and_vias_flags():
    conns = [conn(0, length=12, ref=8, vias=2), conn(1, length=8, ref=8), conn(2, status="failed")]
    r = route([], [], conns, {})
    r["compass"]["conns"][0]["vias"] = 1
    rows = {c["conn"]: c for c in compass.connection_rows(r)}
    assert rows[0]["detour"] == 1.5 and "detour" not in rows[0]["flags"]  # 1.5 is the mark, not over it
    assert "vias planned != drawn" in rows[0]["flags"] and rows[0]["vias_planned"] == 1 and rows[0]["vias_drawn"] == 2
    assert rows[1]["flags"] == []
    assert rows[2]["flags"] == ["failed"] and rows[2]["detour"] is None
    conns[0]["length_nm"] = 13 * MM
    assert "detour" in {c["conn"]: c for c in compass.connection_rows(r)}[0]["flags"]
    assert row(compass.build(r, [], 1), "open")["drawn"] == 1 and row(compass.build(r, [], 1), "open")["kicad"] == 1


def test_no_compass_is_reported_not_raised():
    cp = compass.build({"connections": [conn(0)], "violations": [viol(0, (1, 1))]}, None, None)
    assert cp["available"] is False
    assert row(cp, "planner_overfull")["predicted"] is None
    assert compass.table_md(cp)[0].startswith("Plan model") or cp["rows"]
    md = "\n".join(compass.table_md(cp))
    assert "drawing loss" in md
    assert compass.columns(cp)["d_drawing_loss"] == 1


def test_shrinkage_row_compares_planned_and_drawn_necks_and_long_stretches_join_their_drc_items():
    conns = [conn(0)]
    r = route([dict(viol(0, (5, 0), cls="static"), length_nm=6 * MM)], [], conns, {"shrinkage": 3})
    r["compass"]["drawn_shrinkages"] = 2
    # a DRC item 2.5 mm from the stretch's middle lies on the 6 mm stretch: it joins it
    cp = compass.build(r, [drc("clearance", [(7.5, 0.0)])], 0)
    s = row(cp, "shrinkage")
    assert (s["predicted"], s["drawn"], s["kicad"], s["predicted_clean"]) == (3, 2, None, None)
    assert row(cp, "drawing_loss")["kicad"] == 1 and row(cp, "kicad_only")["kicad"] == 0
    # the violation rows add up to the drawn violations
    assert sum(x["drawn"] for x in cp["rows"] if x["class"] not in ("open", "kicad_only", "shrinkage")) == 1


def test_pad_exit_with_the_510_slot_stub_items_locate_and_do_not_count():
    # 5.10 slot (compass.rs PlanView::pad_exits): a planned stub of conn 0 at (1, 1); the drawn pad_exit violation there
    # is the stub's realisation (predicted 0, drawn 1, sub-class "at a planned exit stub"); one far away is not.
    stub = {"class": "pad_exit_stub", "layers": ["F.Cu"], "conns": [0], "segs": [seg((1, 0.9), (1, 1.1))], "over_nm": 0}
    v = [viol(0, (1, 1), cls="pad_exit"), viol(0, (9, 9), cls="pad_exit")]
    cp = compass.build(route(v, [stub], [conn(0)], {"planner_overfull": 0, "pad_exit": 0}), [], 0)
    r = row(cp, "pad_exit")
    assert (r["predicted"], r["drawn"]) == (0, 2)
    assert [x["sub"] for x in cp["violations"]] == ["at a planned exit stub (the drawing left the stub)",
                                                     "not predicted (no planned exit of the connection there)"]
    assert not any("missing method step" in f for f in cp["flags"])
