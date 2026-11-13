# Task NT report: net-tie short + connection_width

Status: **DONE**. Branch `worktree-agent-af3823b217b46fe3e`, base main `bde9602` (engine = `ea1f064`, rotation-2 base).
Code commit `09229f3` `fix(obstacle): net-tie exemption only where KiCad allows it`.
Every Python / harness number comes from my own engine build loaded via `PYTHONPATH=<worktree>/target/pyeng`
(`topor_engine.__file__` = `<worktree>/target/pyeng/topor_engine/__init__.py`). Base numbers come from a copy of the base build
(`target/pyeng-base`).

## 1. Reproduction

The M3 info config (`wall_clock_s 60, partial_routes False, threads 4`) at base gave `routing_new 1`, a single `shorting_items`:
`Pad 1 [Net-(JPx-A)] of JP1 on B.Cu` ↔ `Track [Net-(Jx-Pin_n)] on B.Cu, length 1.9363 mm`. The base run with
`wall_clock_s 0` shows the same item. JP1 is `Jumper:SolderJumper-3_P1.3mm_Bridged12_RoundedPad1.0x1.5mm` with
`net_tie_pad_groups "1,2"` and a copper `fp_poly` bridge between pads 1 and 2. Pad 2 is net Net-(Jx-Pin_n). The track leaves pad 2
and runs through pad 1, which sits 0.25 mm away. The engine exempted every pad of a tie group for every net of that group
(`pad_group_nets`, used in both `obstacle.rs` and `check.rs`). So the router and its self-check accepted copper that KiCad rejects.
None of these runs reported a `connection_width` item.

## 2. KiCad 10 semantics (kicad-cli 10.0.8, measured, not recalled)

`tools/net_tie_drc_probe.py <out>` writes a small board and runs `kicad-cli pcb drc` on it. The board has one 3-pad net-tie
footprint per case: tie pads 1 (net A) and 2 (net B), untied pad 3 (net C), and the bridge where the case calls for it. The script
checks each case against the expected result and exits 0 (all as expected). Each track starts in a pad of its own net. I found
that KiCad's net propagation gives an unanchored track that touches only one pad the net of that pad (control cases 29/30 in the
first probe), and that hides the short.

| case | geometry | KiCad |
|---|---|---|
| 1, 20, 21, 22 | B track touches gapped tie pad 1 (A) outside pad 2 | shorting_items |
| 8 | B track 0.1 mm from pad 1 (clearance 0.2), outside pad 2 | clearance |
| 31, 32 | tie pads 0.1 mm apart, B inside pad 2 but within clearance of pad 1 | clearance |
| 33 | same pads, B 0.3 mm away | clean |
| 24, 35 | overlapping tie pads, B ends inside pad 2 (in the overlap / 0.1 mm from pad 1) | clean |
| 23 | overlapping tie pads, B touches pad 1 from the far side | shorting_items |
| 25 | A touches the bridge (net-less footprint copper) away from both pads | clean |
| 26 | untied C touches the bridge | shorting_items |
| 27 | A ends in pad 1, overlapping the bridge inside pad 1 | clean |
| 7 | untied C into tie pad 2 | shorting_items |

This matches KiCad's `DRC_ENGINE::IsNetTieExclusion`. A collision between a track of net N and copper of a net-tie footprint is
allowed only where the collision lies inside a tie-group pad of net N. The footprint's net-less copper graphics are allowed for the
tied nets anywhere. A gapped tie pad is therefore an ordinary obstacle for the other tied net.

## 3. Changes

| file | change |
|---|---|
| `engine/src/model.rs` | `tie_group_pads_of(pi)`: the other tie-group pads of a net-tie footprint, O(T) for an ordinary footprint, O(T+P) for a tie. `pad_fused_tie_nets(pi)`: nets of tie-group pads whose copper touches or overlaps pad `pi` on a shared layer, O(T+P+k·v²). |
| `engine/src/obstacle.rs` | A pad's `tie_nets` is now `pad_fused_tie_nets` instead of the whole group's nets. A tie pad's PTH drill gets no tie exemption. Copper shapes keep `tie_nets_of(owner)` (case 25 is legal). |
| `engine/src/check.rs` | Pads lose `tie_nets`. A new field `Obstacle.tie_pads` holds `(net, skeleton)` for each tie-group pad of the same footprint on that layer. A violation against such a pad is dropped only when `tie_exclusion` holds. The check samples the item centreline (step 2 µm, at most 1024 samples, first minimum, deterministic), takes the point of the pad copper nearest to it as the collision position, and requires both points to lie inside the item net's tie pad. O(s·v), and it runs only for a violation against a tie pad. Tie-pad drills are strict. |
| `engine/src/geom/dist.rs` (+ `mod.rs`) | `skeleton_closest_point`, `point_skeleton`, `skeleton_gap` (Big-O in doc comments), with a unit test. |
| `engine/tests/net_tie.rs` (new) | The KiCad cases above on `Checker`, plus two router tests: the private-badge-b JP1 geometry routes net B around pad 1 (copper ≥ 0.2 mm from JP.1, Checker clean), and overlapping (fused) tie pads stay routable. |
| `engine/tests/obstacle_model.rs` | `same_net_and_net_tie_are_exempt` now pins the legal exemptions: same net, fused tie pads exempt for each other's net, touching counts as fused. The illegal exemption it used to pin (a lone tie-group pad exempt for every group net) is inverted in the new `gapped_net_tie_pad_is_an_obstacle_for_the_other_tied_net`, with the KiCad evidence in the comments. New: `net_tie_pad_drill_is_an_obstacle_for_the_other_tied_net`. `copper_shape_polygon_and_tie_owner` is unchanged and still passes. |
| `tools/net_tie_drc_probe.py` (new) | The re-runnable KiCad evidence. |
| `docs/design/44-feature-gap-analysis.md` | The net-tie row gains the verified semantics. |

Decisions:
- **Fused tie pads stay exempt in the routing model, not position-exact.** The obstacle graph cannot express "exempt only inside
  pad Q". Without the exemption, a track could not end in its own pad when that pad overlaps the other tie pad, so the net would be
  unroutable (design 44 row). The Checker self-check enforces the exact rule and rejects any collision outside the own pad. Gapped
  ties, which covers every tie pad on the snapshot boards, are fully strict.
- No new Options field. This is plain KiCad semantics, not a judgement call. The sampling cap and step are internal constants.
- TDD: `net_tie.rs` was written first. Four tests failed at base, including the router test ("net B copper -100000 nm from tie pad
  JP.1"). The two legal-exemption tests passed at base and still pass.

## 4. connection_width

Not reproduced at this base. No `connection_width` item appeared in any of these private-badge-b runs:
- base: 60 s, wall_clock 0
- head: 60 s (×3: my repro, M2 info, M3 info), wall_clock 0, and the gate-options progress run

eprom and wiccon also showed none. A-C9 holds. Exit-stub necks never go below `max(min_track_width, NetRule.min_width)`
(`stubs.rs::min_width`); the fab floor is 0.2 mm, and private-badge-b's `min_connection` is 0.127 mm. So a neck cannot be the source of a
KiCad `connection_width` item, and nothing was changed for it. If it comes back on a 600 s run, it would be a junction geometry
(a track overlapping a via or pad by a sliver), not a neck-width rule. That needs the run's `drc-routed.json` item positions.

## 5. Numbers (base `bde9602` vs head `09229f3`)

Machine shared: 1-minute load average 16–35 during every run, which is above 6. All comparisons therefore use `wall_clock_s 0`
(deterministic). The gate-options run stopped on "no gain", so it equals the wall_clock-0 run.

| board | | routed | failed | vias | routing_new | unconnected | outside mm | router_s | detour med/p90/max/>1.5 | ghost max/>1.5 | stop |
|---|---|---|---|---|---|---|---|---|---|---|---|
| private-badge-b | base | 205/241 | 36 | 196 | **1** (shorting JP1) | 38 | 1814.469 | 403.9 (load) | 1.000/1.294/4.624/9 | 4.709/15 | no gain |
| private-badge-b | head | 205/241 | 36 | 196 | **0** | 38 | 1814.429 | 403.9 (load) | 1.000/1.294/4.624/9 | 4.709/15 | no gain |
| eprom-to-kernal | base | 27/27 | 0 | 0 | 0 | 0 | 64.628 | 0.196 | 1.000/1.006/1.147/0 | 1.312/0 | converged |
| eprom-to-kernal | head | 27/27 | 0 | 0 | 0 | 0 | 64.628 | 0.225 (load 32) | 1.000/1.006/1.147/0 | 1.312/0 | converged |
| wiccon-2023 | base | 62/62 | 0 | 34 | 0 | 0 | 1363.375 | 4.641 | 1.000/1.053/1.282/0 | 1.295/0 | no gain |
| wiccon-2023 | head | 62/62 | 0 | 34 | 0 | 0 | 1363.375 | 7.576 (load 32) | 1.000/1.053/1.282/0 | 1.295/0 | no gain |

- private-badge-b gate options (600 s, threads 4, via `harness.progress`, load 16–19, stop "no gain", router 169 s, runtime 261 s): 205/241,
  failed 36, vias 196, `routing_new 0`, preexisting 4 (clearance, U7 hand pads), unconnected 38. The rotation-2 base reference is
  204/241, unconnected 39. The gate predicate (N/N) is not met; that is another task's job, and completion did not get worse.
- The private-badge-b 60 s info config (timed out, as designed) gave 200/229, `routing_new 0`.
- eprom and wiccon outcomes are byte-for-byte the same numbers (neither board has a net tie). Only router_s changed, from load.
- detours: unchanged (private-badge-b > 1.5: 9, max 4.62 U1-EN; ghost max 4.71 / 15).

Progress PNG: `/home/paul/work/kicad/plugins/topor/.claude/worktrees/agent-af3823b217b46fe3e/docs/benchmarks/progress/2026-11-01-m4-NT-private-badge-b.png`
(not committed). Caption: "NT: 205/241 connections, 45/M nets, partial 0, failed 36, vias 196, arcs 232, zero slack 0, iters 24,
pcb (private pin) / DRC routing 0 preexisting 4, unconnected 38, outside pads 1814.4 vs hand (hand length) mm, F.Cu 1035.7 / B.Cu 998.9 mm,
router 168.99 s on 4 threads / detours > 1.5: 9 (max 4.6) — Net-(U1-EN) 4.6, VCC 4.2, TACT_B 3.6 vs DRC-feasible reference
(ghost fallbacks 0, ghost < 2 mm 52) / stop_reason: no gain".

## 6. Tests

- `pytest tests/gate/test_m2_gate.py tests/gate/test_m3_gate.py -m "gate or not gate" -q -s`: **6 passed** (814 s, loaded). M2
  private-badge-b info: 200/229, `drc_new=0`. M3 private-badge-b info: 200/229, `drc_new=0`, preexisting 4. eprom: 27/27, router 0.127 / 0.177 s.
  wiccon: 62/62, 34 vias, router 7.25 / 4.25 s.
- `cargo test --release --no-fail-fast`: everything passes except the base reds:
  - route_steiner 4/6
  - route_detour_ref::reference_is_feasible_not_topological
  - graph_ownnet::front_only_plus5v_needs_zero_slack_for_failed
  - route_engine::anytime_timeout_emits_committed_subset
  - route_control::route_async_cancel_returns_anytime_result
  - route_stubs::exit_stubs_off_matches_previous

  That last one is not in the rotation-2 base list. I confirmed it is a base red: a build of the untouched base source
  (`git archive HEAD`) fails with the identical digest. Its fixture has no net ties.
- New tests: `net_tie` 6/6, `obstacle_model` 21/21, lib 12/12.
- `cargo clippy --all-targets -- -D warnings`: clean. rustfmt was applied to the changed files only. The rest of the repo is not
  rustfmt-clean, so I left it untouched.
- `.venv/bin/pytest -q` (default suite): 503 passed, 7 skipped, 4 failed. All 4 are listed base reds: test_config_docs,
  test_router progress caption, test_run_flow wiccon 63 vs 62, and test_scaffold (= cargo reds).
- Determinism: the route_threads / *_threads_byte_identical tests pass, except the base-red steiner one. The new code iterates no
  HashMap: pads are taken in index order and samples in order with the first minimum.

## Concerns

1. Overlapping (fused) tie pads: the routing model exempts the whole fused pad. A route touching it outside the own pad is legal
   to the router but rejected by the Checker self-check, so the connection fails rather than producing a DRC error. No snapshot
   board has fused tie pads (all ties are gapped, including bornhack's NetTie-2_SMD_Pad0.5mm and the U6 group).
2. connection_width: not reproduced at this base (section 4), so not changed.
3. route_stubs::exit_stubs_off_matches_previous is a base red that is missing from the rotation-2 red list (the golden digest is
   stale at base).
