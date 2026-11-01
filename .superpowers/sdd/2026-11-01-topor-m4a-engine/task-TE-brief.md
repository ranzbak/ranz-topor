### Task TE (opus, rotation 2): engine red tests green by root cause — route_steiner 4/6, route_detour_ref, the two load flakes

Read rotation-2-common.md first (state, measuring rules, acceptance).

Red at base 789e050 (`cargo test --release --manifest-path engine/Cargo.toml --test route_steiner --test route_detour_ref`):
- route_steiner: `t_junction_onto_routed_sibling` ("routed","failed" vs ("routed","routed")), `junction_angle_below_60_is_rejected`
  (no junction formed at all, `!ts.is_empty()`), `steiner_threads_byte_identical` (no `"junction":true` in the result),
  `steiner_off_is_m3_behaviour` (steiner-off digest of the USB crop differs from the pinned one). All four already red at a233d62
  = the A5 merge on top of A6 (3cddcc9): the A6 stubs / in-pad off-centre starts / pad bridges (pad_bridge_max_mm, U7.4→U7.7,
  U7.2→U7.9) change the U7 / J1 USB crop fixture these tests use. Find WHY a T-junction onto a routed sibling no longer forms or no
  longer routes with A6 on (e.g. the junction target excludes stub/bridge copper, the bridge consumes the sibling the T should attach
  to, the stub start point is not a valid attachment, the junction angle check sees the stub direction) and fix the engine so A5 and
  A6 compose — this is also lever 1 of A12 ("why A5+A6 together route less than A6 alone": A6 alone dcsgone 210/241, merged 204/241).
  For `steiner_off_is_m3_behaviour`: the pinned digests were taken on the pre-A6 main; establish what steiner=false must equal now
  (the A6-only engine at 3cddcc9, plus whatever A16/A13/A14 legitimately changed — verify each) and re-pin only with that evidence in
  the test comment; the intent (steiner off = no junction code path changes the result) must stay tested.
- route_detour_ref `reference_is_feasible_not_topological`: reference == ghost (1220656), "reference must come from the slot
  search, not the ghost" — red since the A7 merge 4a339e6. Root cause (fallback taken? budget? fixture changed by A6?), fix.
- Load flakes: `route_engine::anytime_timeout_emits_committed_subset` (0.5 s) and `route_control::route_async_cancel_returns_anytime_
  result` fail under load (green alone on a quiet machine). Make them robust by root cause without weakening what they check (e.g. the
  test asserts an anytime subset: make the deadline relative to measured work, or make the engine's first-pass anytime commit
  independent of wall time granularity); they must pass with `stress -c 8`-like load (`for i in $(seq 8); do yes >/dev/null & done`
  for the duration of the test, then kill them).
- `test_scaffold` (pytest) runs the debug cargo suite: confirm it is green once the above are.
Not yours: graph_ownnet::front_only_plus5v_needs_zero_slack_for_failed (ripup growth, task GR), the JP1 net-tie short (task NT).

Acceptance: `cargo test --release` all suites green except graph_ownnet front_only (GR), clippy -D warnings clean; numbers per
rotation-2-common.md for eprom / wiccon / dcsgone (base and head) — dcsgone completion must not drop below 204/241; report whether
the A5⊕A6 fix changes dcsgone routed (and by how many); PNG `--task TE --board dcsgonebadge` if dcsgone changes.
Commit(s): `fix(steiner): A5 junctions compose with A6 exit stubs and pad bridges`, `fix(detour): ...`, `test: ... flake root cause`.
Report: task-TE-report.md.
