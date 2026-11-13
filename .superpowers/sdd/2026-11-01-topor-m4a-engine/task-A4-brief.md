### Task A4 (S): own-net / same-net capacity, wall-aware (M4-D1) — `graph.rs`, `search.rs`

**Interfaces:** `RouteGraph::fits_for(e, net, w, c) -> bool`, `RouteGraph.wall_r: Vec<f64>`; own-net ends charge 0 gap + core radius back; same-net strands on `e` charge 0 gap. Build `O(E·q)`.

- [ ] **Step 1:** `own_tip_edge_fits_own_net`, `two_same_net_strands_share_one_tip_edge`, `own_end_edge_against_outline_wall_stays_unfit`, `search_layers (1)` flips to "fits, ghost not soft" (`engine/tests/graph_ownnet.rs`). FAIL → implement → PASS.
- [ ] **Step 2:** `eprom_ripped_front_only_is_the_m2_result` green; eprom two-layer identity re-verified; wiccon not worse (vias ≤ 33 expected). PNGs `--task A4` private-badge-b + wiccon. Commit `feat(graph): own-net and same-net capacity, wall-aware`.


## Coordinator additions (binding)
- Design = docs/progress/notes/m4-capacity-steiner-design.md §1 (A3, merged) — exact `fits_for` formulas, wall term, worked numbers;
  follow it, and record every deviation with numbers. Diagnosis = the private-badge-b completion diagnosis (note removed: private board) (DX, merged): A4 is fix #1
  (≈ 25–35 of 67 failed on private-badge-b: the 11 0.2 mm (b) own-pad exits at U4/U7/J3/U6, the 15 own-pad-edge pinches priced as overuse,
  e.g. #47 U4.12, #86 JP1.3, #232 U1.9↔U1.8, J1 trio; E1 shows the capacity rule makes search and metrization inconsistent).
  Re-run DX's classification (route_probe --dump + the classify script described in the note §2/§6) after your change and report the
  new per-mechanism table.
- Search and metrization must agree: a path the search accepts as fitting must not be rejected by metrization for the same pinch
  (report metrize_failed per iteration before/after from the Options.profile columns DX added).
- Guards: `eprom_ripped_front_only_is_the_m2_result` (27/27, ≤ 67.8 mm) green (the withdrawn F6 broke it — read m3-detour-diagnosis.md
  why); eprom two-layer identity re-verified (or not-worse with the diff explained); wiccon not worse (62/62, vias ≤ 37 — plan expects
  ≤ 33 —, outside ≤ 1380 mm, routing_new 0, unconnected 0).
- private-badge-b at the gate options (A-C3, wall 600, partial false, threads 4): routed / connections_total (and first-pass N count), failed,
  vias, routing_new, preexisting, unconnected, router_s, runtime_s, iterations, stop_reason, detour median/p90/max/count>1.5/top 5.
  PNGs `--milestone m4 --task A4 --board private-badge-b` and `--board wiccon-2023`.
- Any judgement call (e.g. wall term on/off, credit fraction) → Options field + default; name it in the report.
