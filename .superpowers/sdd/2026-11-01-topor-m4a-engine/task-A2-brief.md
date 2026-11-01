### Task A2 (S, F review at the gate): fine-pitch entry #201 and state-point / los convergence (M4-D2) — `engine/src/search.rs:1373, :1758`

**Interfaces:** `state_point` shrinks the usable sub-segment by every foreign vertex within `r_v + c + w/2 + eps` (`verts_near`, `O(q)`) and discretises roundrect-corner crossings at the corner-circle tangent point; `los` returns `ok = false` when the bend loop does not converge (apex not approaching within `LOS_PORTAL_CAP` or cap hit).

- [ ] **Step 1:** `fine_pitch_entry_u7_4_routes_direct` (U1.13→U7.4 geometry extracted via `gate_diag --detour 201` into a fixture; 0 vias, `est_len ≤ 1.5 × straight`), `los_nonconverging_bend_loop_is_rejected` (#208 trio: 0-via path `≤ 1.3 × 2.93 mm`, `apex_len ≤ 4 × chord`) in `engine/tests/search_entry.rs`. FAIL → implement → PASS.
- [ ] **Step 2:** eprom byte-identical or not worse (diff reported); wiccon not worse. PNG `--task A2 --board dcsgonebadge`. Commit `fix(search): fine-pitch entry, state point shrink, los convergence`.


## Coordinator additions (binding)
- A1 is merged (bend_ok/clearance_bend memo + face clear radius, search.rs); build on it — keep its memo keys valid if state_point changes
  (a memo entry must never return a result computed under different state-point geometry).
- #201 (U1.13→U7.4) and #208 (USB+ trio) ids are 0.3.0 connection ids — re-identify them on current main by net + pad labels
  (`cargo run --release --example gate_diag -- --help`); 0.4.0 dcsgone worst detours also include USB- J1.A7→J1.B7 4.40 vs ghost 1.00
  (4.4×) — check whether it is the same los/state-point mechanism and include it if so.
- Measure dcsgone with the GATE options (Ruling A-C3: wall_clock_s 600, partial_routes false, threads 4) before/after; report
  routed/total, failed, vias, routing_new, preexisting, unconnected, router_s, runtime_s, iterations, stop_reason, detour
  median/p90/max/count>1.5/top 5. PNG `--milestone m4 --task A2 --board dcsgonebadge` (A0's harness defaults apply the gate options).
- eprom + wiccon: byte-identical or not-worse with the diff explained (gate numbers in global-constraints.md "Not worse than M3").
- DX diagnosis (docs/progress/notes/m4-dcsgone-diagnosis.md, merged) item 4: A2 fixes 6 direct failures (#191/192/193/212/213/197 —
  U7's pads block the search itself: no state on any exit even when priced) + the 5.2× and 3.6× detours. Use its ids/labels. A parallel
  worker does A4 (own-net capacity in graph.rs fits_for/wall_r + search.rs callers) — keep your search.rs edits to state_point / los /
  verts_near so the merge stays clean; report which of the DX cases route with A2 alone.
