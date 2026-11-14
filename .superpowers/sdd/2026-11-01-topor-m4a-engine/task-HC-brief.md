### Task HC (S): NPTH hole clearance in relax/taut (DX fix #3) + geometry proptest oracle + TopoR backlog rows

1. **NPTH hole clearance (engine).** the private-badge-b completion diagnosis (note removed: private board) (§2 "i-hole" rows, §6 item 3): NPTH pads (private-badge-b
   H1/H2/H4, D17/D18 slots, J1 locator) are anchored as net-0 pads at 0.2 mm clearance in relax/taut (metrization) while the Checker
   wants `hole_clearance` 0.35 mm → 6 connections (+1 zero-slack) fail metrization. Make every geometric consumer (graph capacity,
   search, sketch/taut anchors, relax, Checker) use the same clearance for an NPTH pad: `max(clearance, hole_clearance)` when
   `attr == npth` (or model it as a Hole kind) — one helper, used everywhere; Big-O unchanged. TDD: a fixture with an NPTH pad where a
   0.25 mm gap is legal for copper clearance 0.2 but illegal for hole_clearance 0.35 → the router keeps ≥ 0.35 from the hole, DRC clean;
   and a test that search and metrization agree (no "found path rejected by metrization" for that pinch).
   eprom/wiccon: byte-identical expected (report; if they change, explain which NPTH caused it and that DRC is still clean).
   private-badge-b gate options (A-C3: wall 600, partial false, threads 4): routed/connections_total, failed, vias, routing_new, unconnected,
   router_s, detour max/count>1.5; which of DX's 6 hole cases route now. PNG `--milestone m4 --task HC --board private-badge-b`.
   Commit `fix(engine): NPTH pads keep hole_clearance in every geometric stage`.
2. **Geometry proptest (test oracle).** Debug-profile `cargo test` (tests/test_scaffold.py runs it) found a counter-example in
   `engine/tests/dist_props.rs::prop_arc_arc_matches_sampling`: a = Arc { c: (-800549.6885420157, -807637.6065073739), r: 1966315.95029234,
   Cw, a_start: 2.5195661285126523, sweep: 3.2089898132921144 }, b = Arc { c: (-800000.0, -800000.0), r: 1972284.7145767987, Ccw,
   a_start: 3.383681098996437, sweep: 5.311841872798016 } → arc_arc 1524.798 nm vs sampled 1531.462 nm (oracle ≥ analytic by 6.7 nm;
   nearly concentric arcs). Decide with evidence (fine scan as in the existing `short_arc_pair` regression at dist_props.rs:572) whether
   the analytic value is right (oracle basin trap → fix the oracle: more seeds / refine from every endpoint-foot pair) or wrong (fix
   arc_arc's near-concentric branch). Add a deterministic regression test with this pair; commit
   `test(geom): near-concentric arc_arc counter-example` (or `fix(geom): …`). Never loosen the tolerance without proof.
3. **Docs (TopoR-inspired backlog, main session request).** Add rows (source: the TopoR Wikipedia link already in
   docs/design/01-bibliography* — cite it) to docs/design/44-feature-gap-analysis.md AND the backlog table of
   docs/design/45-mvp-plan.md §3 (style of the existing "Backlog (after M4)" rows): (a) BGA-area strategy; (b) global inter-layer via
   minimisation / single-layer solution search (Polubasov 2001); (c) re-optimising tracks after the ENGINEER moves a component or via
   (the router reacts to placement; it never places); (d) lines-only vs lines+arcs output mode — note that it overlaps the existing 45°
   mode backlog row; (e) "route all, repair later" (complete every connection allowing violations, then legalise) as a fallback for
   otherwise unroutable connections; (f) teardrop-style smoothing of track-to-pad transitions during routing. Auto placement is OUT OF
   SCOPE (user: "components are placed by the enigneer, this is not a task of the router.") — if 44 lists placement, mark it as a
   non-goal with that reason; add no placement row. Commit `docs(design): TopoR-inspired backlog rows; placement is a non-goal`.
   tests/test_docs_links.py green.
