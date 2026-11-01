### Task A6 (S): pad exit stubs — `route.rs`, `wires.rs`, `decompose.rs`, `model.rs`

**Interfaces:** `Terminal::Stub { pad: Vec2, end: Vec2, layer: u32 }`; `Options.exit_stubs: String` (`"auto" | "all" | "off"`, default `"auto"`); `ConnReport.stub_nm: i64`; generation `O(T·q)`.

- [ ] **Step 1:** `stub_generated_for_unfit_fine_pitch_pad`, `stub_skipped_when_infeasible` (reason `"stub infeasible"`), `exit_stubs_off_matches_previous`, `exit_stubs_all_on_every_smd_pad` (`engine/tests/route_stubs.rs`). FAIL → implement → PASS.
- [ ] **Step 2:** eprom byte-identical (THT only); wiccon not worse; dcsgone #201 routed. PNG `--task A6 --board dcsgonebadge`. Commit `feat(route): pad exit stubs (auto/all/off)`.


## Coordinator additions (binding; Rulings A-C9 necking, A2 finding)
- Design: docs/progress/notes/m4-capacity-steiner-design.md §3 (A3). This task ALSO implements trace necking (DX fix NX, TopoR
  "necking", Ruling A-C9): docs/progress/notes/m4-dcsgone-diagnosis.md §6 item 2 (≈ 12 on dcsgone: 0.5 mm nets 33, 47, D23-A, VCC at U3
  entering pads narrower than w + 2c: U4 0.25, U5 0.3, J1 0.6, U3 0.6): the stub / first-last piece of a connection is drawn at
  min(w, what fits) but ≥ the effective min track width (KiCad min_track_width + netclass/custom-rule minima as resolved in rules)
  for ≤ neck_max_mm; keys `[search] neck_to_pad` (bool) and `[search] neck_max_mm` (default from DX: 2 × pad length — measure);
  default neck_to_pad = true only if DRC-clean (routing_new 0 incl. track_width / connection_width) on all snapshot boards, else false
  with numbers. Necked pieces are reported per connection (`neck_nm`, width) and in the run report.
- A2 finding (.superpowers/sdd/2026-11-01-topor-m4a-engine/task-A2-report.md "Key finding"): U7 middle pads (UDFN-10, 0.625 × 0.25 mm,
  0.5 mm pitch; U7.2/4/7/9, decompose #201/202/203/227/228: U1.13→U7.4, U7.4→U7.7, U7.4→J1.B7, U7.2→U7.9, U7.9→J1.B5) cannot be
  entered from the pad CENTRE (a 0.2 mm wire at the centre is 0.175 mm from the GND pads U7.3/U7.8, clearance 0.2). A DRC-clean entry
  exists off-centre: wire centre at y ∈ [121.530, 121.570] (40 µm corridor inside the pad), leaving through the tip. The stub must be
  allowed to START off-centre inside the own pad (copper of the stub overlaps the own pad; KiCad connectivity: the stub end inside the
  pad connects) — reuse search.rs `edge_window`. Without this 231/231 is unreachable. Checker-verified before use.
- `exit_stubs` auto|all|off (schema since 0.5.0 → live key). eprom byte-identical expected (THT only) — report.
- PNG `--milestone m4 --task A6 --board dcsgonebadge`.

- Acceptance (every task, main session): completion AND detours are non-regression items: on eprom, wiccon and dcsgone report
  routed/connections_total, failed, vias, routing_new, unconnected, router_s, AND detour (reference, A7) median/p90/max/count > 1.5
  plus detour_ghost; count > 1.5 and max must not get worse than main at your base (numbers for both in the report), unless a
  connection newly routes (say which). Not-worse gate numbers (global-constraints.md) for eprom/wiccon; NOTE wiccon outside-pad length
  headroom is small (bound 1380 mm) — report it.
- dcsgone at the gate options (A-C3; harness.progress --milestone m4 applies them). Gate predicate A-C11.
- License: topor is GPL-2.0-or-later (main 93f2126). Porting code from the gEDA PCB toporouter (src/toporouter.c, GPL-2.0-or-later,
  Anthony Blake) is allowed when it helps: keep the original copyright notice + attribution at the port site, name the origin in the
  commit message and report it (coordinator records a ruling). No new Apache-2.0-only runtime crates (no new crates at all without asking).
