### Task FG (opus, rotation 2): geometry-model consistency — via drill hole clearance, taut deep wall, sub-margin grazes, signal-net polygons (DG R2.4 items 3 + 5(i))

Read rotation-2-common.md (incl. "Round 2 update") and the private-badge-b completion diagnosis (note removed: private board) R2.3 (d) 2–4, R2.4 items 3 and 5.
(a) Via drill hole clearance is not in the graph / site model: a router via is a vertex of radius d/2 + copper clearance; for the
    0.45/0.25 pre-defined via the hole term wins (0.125 + 0.35 = 0.475 > 0.425), so search/taut use a corridor 50 µm too narrow and the
    self-check rejects it (`viahole:topor:193 (gap 305000 < 350000)`). Model vias of earlier passes get copper + Hole anchors at one
    point but RouteGraph::build keeps one radius. Fix in MultiGraph::activate / ObstacleSet::set_site / RouteGraph::build (take the
    larger of co-located anchors). This is ALSO the root cause of the wiccon plugin-path failure (task-PY-report.md item 3: with the
    plugin's net_priority {+4V:2, 3.3v:2, 4v:2, GND:2} wiccon gives 63 total, 62 routed, 1 failed GND via→pour, `self-check:
    viahole:topor:249 (gap 304999 < 350000 nm)`): make tests/plugin/test_run_flow.py::test_run_wiccon_refill_gives_harness_
    connection_count and tests/gate/test_m4b_gate.py headless wiccon green (62/62, routing_new 0, unconnected 0) — confirm the 63rd
    connection count is legitimate (completion pass) or not, by root cause.
(b) taut deep wall: a chord crossing the core of a long thin pad that is not a member is reported "grazed mid-span" (depth w/2 + c + ε
    = 305 µm) instead of wrapping the pad end on the slot path's side (`taut::penetrations` adds an end anchor only if it covers the
    penetration point). Fix; recovers ≈ 4 (U1.2→R1.1, J3.17→R16.2, TP17.1→J3.2, U1.14→U7.5).
(c) Sub-margin grazes (< margin_nm 5 µm, DRC-clean at margin 0): accept per wall at margin 0 inside taut with a self-check at margin 0;
    option `[expert] accept_submargin_graze` (default by measurement; must keep routing_new 0 on every snapshot board).
(d) Signal-net copper polygons (copper_shapes net ≠ 0) become terminals in decompose (IR_RX and +3V3 islands: 2 unconnected_items with no
    failed connection on private-badge-b).
Unit tests per item (0.45/0.25 via with hole_clearance 0.35: no corridor at 0.43 mm from its centre; chord through a 0.3×1.1 pad's middle
wraps its end; 1 µm graze accepted and self-checks clean at margin 0; a signal polygon island is a terminal). (b)/(c) touch the M2
guard eprom_ripped_front_only_is_the_m2_result: run it (default options). PNG `--task FG --board private-badge-b`. One commit per item.
Report task-FG-report.md.
