# Task A2 report: fine-pitch entry (#201 class) and state point / los convergence (M4-D2)

Status: DONE_WITH_CONCERNS. M4-D2 is fixed: the #208 runaway is gone, the J1 USB-C pair routes direct, and the
dcsgone worst detour drops from 11.6× to 2.7×. The U7 "#201 class" is **not** routable by any search fix. With
the pad-centre attachment it is DRC-infeasible at 0.2 mm clearance; see "Key finding". The brief's 0-via
assertion for U1.13→U7.4 also contradicts the hand layout (2 vias). The test therefore pins the feasible
behaviour and the deviation is explained below.

Branch `worktree-agent-ae6b4e0b56c729daa`, based on main `cba0e2a`.

## Key finding: U7 middle pads cannot be entered from their centre (A6 needs this)
U7 is a UDFN-10 with 0.625 × 0.25 mm pads at 0.5 mm pitch. The middle pads U7.2, U7.4, U7.7 and U7.9 sit
0.15 mm from the 0.45 mm GND pads U7.3 and U7.8. A 0.2 mm wire attached at U7.4's centre (90.8875, 121.5) has
0.175 mm copper gap to U7.3. That holds at the attachment point itself, before any routing choice. The hand
board's own DRC flags the same thing: `Clearance violation (clearance 0,2000 mm; actual 0,1750 mm) Track [USB-]
… Pad 3 [GND] of U7`. CC2 at U7.2 / U7.9 is identical.

A DRC-clean entry exists, but only off-centre. The wire centre must run at y ∈ [121.530, 121.570] (a 40 µm
corridor inside the pad) and leave through the tip. Necking does not help: min_track_width is 0.2 mm.

So DX's 5 "A2 direct" cases (#191/192/193/212/213 = decompose #201/202/203/227/228: U1.13→U7.4, U7.4→U7.7,
U7.4→J1.B7, U7.2→U7.9, U7.9→J1.B5) stay BLOCKED. That is correct: a found path would be rejected by the checker
or by KiCad DRC.

**A6 (exit stubs) must start the stub off-centre for these pads**, using the in-pad corridor above. `edge_window`
in search.rs computes such windows for edges and could be reused. At 0.15 mm clearance (the gap the hand layout
uses) the same pads route direct (see the test).

## Changes (engine/src/search.rs only, plus test + fixture)
- **`state_point` / `edge_window` (new):** the usable sub-segment of an edge is the end-margin interval minus,
  for every foreign vertex `u` within `r_u + max(clr_u, c) + w/2 + eps` (the `verts_near` query through the
  lattice cache `cached_near`), the parameter interval where the centreline would come closer than that.
  - The state point is the point of the remaining intervals nearest to `apex → goal`.
  - When nothing is left (an unfit edge crossed as overuse), the old end-margin point is kept.
  - Memoised per edge per search (`sp_memo`, membership only).
  - Cost: O(q log q) once per edge, O(k) per call.
  - Effect on J1.A6's 0.3 mm tip: the state point moves from the own corner (0.2 mm from A7) into the 90 µm
    window x ∈ [90.205, 90.295].
- **`scan_bend` (in `clearance_bend`), wrap-apex rule:** when the apex is a foreign vertex (a bend, detected by
  `wrap_apex`, an O(1) cached point query), obstacles at or behind the apex (chord parameter ≤ 0) are skipped.
  This was the #208 spiral: from A7's corner, the other corner of the same pad 0.3 mm *behind* the apex was
  taken as the next bend.
  - The result is still a pure function of `(a, t)`, so A1's memo keys stay valid.
  - A1's mode-equality tests pass.
- **`push_state`, `apex_past`:** if the apex returned for a crossing of `e` is the entered face's third vertex
  (a clearance bend at a foreign corner across `e`), the node's `since` is 0. `e` lies behind the string, and
  keeping it as a portal sent the next line back to `e`'s near end. Restricted to that vertex, because dropping
  the chain for farther apexes could let chords cut corners. O(1).
- **`los` convergence:** for a line leaving a terminal (root node, not ghost), the line is inadmissible when
  either:
  - `LOS_STALL_CAP` (= 2) bends in a row do not bring the apex closer to the target (`LOS_TOL` 1 nm), or
  - `LOS_PORTAL_CAP + 4` bends end without a clear chord.

  Later lines keep the old behaviour; see the decisions below.
- **`probe_entry` (doc(hidden) test hook):** returns the state point and the los of one exit.
- `engine/tests/search_entry.rs` (new, 2 tests) and `engine/tests/fixtures/dcsgone-usb-crop.json` (39 KB). The
  fixture is a crop of the harness model export of snapshot 3e7e66f862a7: window x 84–100, y 97–129, pads / vias
  / holes inside, window outline, no pours or kept copper. It is made by `engine/tests/fixtures/crop_model.py`
  (dev helper, committed).
- Big-O is stated in each doc comment. Every new loop is bounded: intervals ≤ candidates, bends ≤
  `LOS_PORTAL_CAP + 4`.

## Decisions / deviations (please review)
1. **The convergence rule applies only to lines leaving a terminal (spec deviation).** As specified, it would
   reject every non-converging line. On every line it breaks the M2 guard `eprom_ripped_front_only_is_the_m2_result`:
   27/27 but 92.35 mm outside-pad copper (bound 67.8). A13 then detours 10.75 → 31.5 mm, because pinched gaps
   between two DIP pins (the string oscillates between two corners) were the price the F.Cu-only negotiation
   relies on. Variants measured on that guard:
   | variant | outside-pad length | result |
   |---|---|---|
   | strict everywhere | 92.35 mm | — |
   | re-anchor at the state point, then reject | 92.35 mm | — |
   | re-anchor, then plain chord | — | 26/27 |
   | root only | 67.03 mm | 27/27, +5V zero slack (guard green) |

   The #208 runaway itself is removed at its cause by the window, wrap-apex and apex_past changes. The root rule
   only stops pinched terminal exits early: at U7.4, two bends instead of a 5–9 mm spiral.
2. **`LOS_STALL_CAP = 2` instead of "within LOS_PORTAL_CAP".** On the dcsgone empty graph, the version with the
   rule effectively off and only the cap active gives identical searches (218 found, 101 vias). The value only
   bounds the wasted bends.
3. **`fine_pitch_entry_u7_4_routes_direct` content.** At the board's 0.2 mm clearance it asserts:
   - U7.4→U7.7, U7.4→J1.B7 and U7.2→U7.9 are not found (DRC-infeasible, see above);
   - every U7.4 exit is decided without a runaway: admissible exits have string ≤ 4 × chord, rejected exits
     have `apex_len ≤ 1 mm` (base: 5.1–9.4 mm, so this is the red part).

   At 0.15 mm it asserts:
   - those three connections route with 0 vias, ≤ 1.5 × straight;
   - U1.13→U7.4 is found with ≤ 1.5 × straight and **≤ 2 vias**, not 0. The hand uses 2 vias (it crosses U1 on
     F.Cu). The 0-via B.Cu path around U1's pad rows is 34.4 mm = 1.52 × straight, so the brief's "0 vias and
     ≤ 1.5 × straight" cannot both hold.
4. `los_nonconverging_bend_loop_is_rejected` is as briefed:
   - J1.A6→J1.B6 and J1.A7→J1.B7 give 0-via paths ≤ 1.3 × 2.93 mm (3.09 and 3.49 mm), also with vias allowed;
   - U7.6→J1.A6 is ≤ 1.3 × 2.41 mm;
   - every admissible exit of J1.A6 has string ≤ 4 × chord.
5. **The "roundrect-corner tangent point" discretisation is not implemented.** Own-own edges (pad sides between
   corner circles) keep their end-margin point. It only matters with an off-centre attachment (U7 at 0.2 mm),
   which is A6's domain. At 0.15 mm the midpoint is inside the window.
6. **No Options added.** `LOS_TOL` and `LOS_STALL_CAP` are internal geometry-consistency constants next to
   `LOS_PORTAL_CAP`, not user judgement calls. A toggle would need `SearchParams` plumbing that A4 and A10 also
   touch.

## Tests
- Red on main (a scratch copy of main's search.rs plus the `probe_entry` hook):
  - `los_nonconverging…`: "J1.A6 -> J1.B6: est 34.892 mm > 1.3 x 2.93 mm";
  - `fine_pitch…`: "U7.4 edge 617: bend loop ran away, apex_len 5.565 mm".

  Green after the change.
- `cargo clippy --all-targets -D warnings`: clean.
- `cargo test --release --no-fail-fast`: 381 passed, 2 failed. Both failures are load-timing tests at load
  average 26–30:
  - `anytime_timeout_emits_committed_subset`: main fails it identically under the same load;
  - `route_async_cancel_returns_anytime_result`: passes on rerun.

  `eprom_ripped_front_only_is_the_m2_result` and `eprom_ripped_two_layers…` are green, as are all of A1's
  `search_speed` tests.
- pytest default suite (PYTHONPATH=target/pyeng3, 28 min under load): 469 passed, 7 skipped, 1 failed.
  - The failure is `tests/test_scaffold.py::test_cargo_workspace_builds`, which runs `cargo test` in the debug
    profile. In that profile the only failing test is the load-timing `anytime_timeout_emits_committed_subset`
    (see above; main fails it too under this load).
  - `tests/test_engine_surface.py::test_route_async_cancel_returns_anytime_result` (2 s cancel timing) failed
    once under load and passed when rerun alone. It was deselected from the full run.

## Numbers
All runs use gate options (`wall_clock_s` 600, `partial_routes` false, threads 4), `run_bench rip=all`. The
machine was shared: load average 23–30 on 8 cores, so times are noisy; counts are exact.

### dcsgonebadge 3e7e66f862a7
| | main cba0e2a | A2 |
|---|---|---|
| routed / total | 188/255 | **180/239** |
| failed | 67 | **59** |
| partial | 0 | 0 |
| vias | 199 | **146** |
| routing_new | 0 | 0 |
| preexisting | 4 | 4 |
| unconnected | 62 (via_dangling 4) | **60** (via_dangling 2) |
| router_s | 290.9 | 163.1 (PNG run: 127.0) |
| runtime_s | 420.0 | 340.7 (PNG run: 262.5) |
| iterations | 42 | 32 |
| stop_reason | no gain | no gain |
| detour (ghost) median | 1.003 | 1.000 |
| detour p90 | 1.523 | **1.281** |
| detour max | 11.62 | **2.711** |
| detours > 1.5 | 20 / 188 | **11 / 180** |
| outside-pad length | 1725.8 mm | 1374.8 mm |

- Top 5 detours after A2: USB+ J1.A6→J1.B6 2.711 (ghost 1.00 goes straight through A7; the hand needs 2.93 mm,
  so the true ratio is ≈ 0.93), GND SW1.0→pour 2.315, GND J3.1→pour 2.143, D14-DOUT D14.2→D15.4 2.072,
  +3V3 R1.1→R16.2 1.977.
- Top 5 on main: VCC U3.1→U3.3 11.62, GND J1.B1→pour 8.24, GND J1.B12→pour 7.27, USB- J1.A7→J1.B7 5.18,
  VCC topor:27→D3.1 3.67.
- VCC U3.1→U3.3: **3.29 mm, 0 vias** (main: 31.0 mm, 4 vias).
- Failed set by (net, from, to): 33 fixed, 25 newly failed, net −8. The newly failed are mostly negotiation
  effects (I2C / VCC / GND completions).
- The gate is not met. 231/231 still needs A4, A6+NX, HC and A5.
- The PNG run reproduced the gate numbers exactly (deterministic).

Empty-graph search (`gate_diag --iters 0`, static, all 231 connections):

| | main | A2 |
|---|---|---|
| found | 218 | 218 |
| vias over found | 133 | 101 |
| est length | 2231.0 mm | 2158.7 mm |
| connections changed | — | 39 |
| changed connections with more vias | — | 1 (J4.1→J6.3: 0 → 1 via, 2.1 mm shorter) |

| connection | main | A2 |
|---|---|---|
| #34 U3.1→U3.3 | 2 vias, 8.22 mm | 0 vias, 2.43 mm |
| #208 J1.A6→J1.B6 | 2 vias, 24.9 mm | 0 vias, 3.09 mm |
| #204 J1.A7→J1.B7 | 0 vias, 24.9 mm | 0 vias, 3.49 mm |
| #77 U4.2→U4.4 | 4 vias, 4.62 mm | 0 vias, 2.44 mm |

The U7 cases stay blocked (see the key finding).

### Which DX A2 cases route with A2 alone (gate run)
- #197 USB+ J1.A6→J1.B6: **routed**, 2.711 mm, 0 vias. On main it failed with "wall pad:J1:A7 grazed".
- #191/192/193/212/213 (U7 middle pads): still failed, BLOCKED. DRC-infeasible from the pad centre; needs A6
  with an off-centre stub start.
- The 5.2× detour USB- J1.A7→J1.B7 (main: 2 vias, 5.18 mm) is now **failed**: "infeasible site:302 ↔ site:305".
  The empty-graph search finds 0 vias / 3.49 mm. In the negotiation it competes with A6→B6 for the same
  tip-wrap space.
- The 3.6× detour USB+ U7.6→J1.A6 (main: 2 vias, 8.6 mm) is now **failed**: self-check gap 0.062 mm to item 291.
  The empty-graph search is direct, 2.41 mm.

  Both are the same-net / sibling interplay of the J1/U7 corner (A5 Steiner, A4 capacity), not the los mechanism.

### eprom-to-kernal
**Byte-identical** to main: route JSON minus runtime, threads 1/2/4 plus a repeat, all hash 2dcde8ce4023.
27/27, outside 64.628 mm, vias 0, routing_new 0, unconnected 0, router_s 0.15. The M2 guard is green
(F.Cu-only 27/27, 67.03 mm, +5V zero slack kept).

### wiccon-2023
Not identical, but deterministic: threads 1/2/4 plus a repeat give the same hash, 5caac369fcdc.

| | main | A2 | M3 gate |
|---|---|---|---|
| routed | 62/62 | 62/62 | 62/62 |
| vias | 37 | 35 | ≤ 37 |
| routing_new / unconnected | 0 / 0 | 0 / 0 | 0 / 0 |
| outside-pad length | 1373.55 mm | 1379.36 mm | ≤ 1380 mm |
| router_s | 4.6 | 4.7 | < 10 |
| iterations | 20 | 8 | — |
| detours > 1.5 | 0 (max 1.449) | 0 (max 1.405) | — |

Where the length goes:
- GPIO0 U1.18→FLASH1.1: 2 vias / 14.8 mm → 0 vias / 12.3 mm.
- RST RESET1.1→U1.1: 2 vias / 39.6 mm → 0 vias / 27.4 mm.
- GND U1.15→pour: 1.78 → 17.5 mm. Its ghost with the others present is 14.3, so the ratio is only 1.22, but the
  absolute length is worse. Its escape via spot is taken by the now via-free GPIO0 / RST routes.
- SCLK: +3.5 mm.

Not worse by every gate number. The length sits only 0.64 mm under its bound: a concern.

## PNG
`/home/paul/work/kicad/plugins/topor/.claude/worktrees/agent-ae6b4e0b56c729daa/docs/benchmarks/progress/2026-11-01-m4-A2-dcsgonebadge.png`
(not committed)

Caption: "A2: 180/239 connections, 39/66 nets, partial 0, failed 59, vias 146, arcs 184, zero slack 0, iters 32,
pcb 3e7e66f862a7 / DRC routing 0 preexisting 4, unconnected 60, outside pads 1374.8 vs hand 3574.7 mm, F.Cu
839.1 / B.Cu 737.5 mm, router 127.00 s on 4 threads / detours > 1.5: 11 (max 2.7) — USB+ 2.7, GND 2.3, GND 2.1
/ stop_reason: no gain".

## Concerns
1. U7.2/4/7/9 are DRC-infeasible from the pad centre at 0.2 mm. A6 must attach off-centre, in the 40 µm in-pad
   corridor. Otherwise the M4a gate (231/231) is unreachable for these 5 connections.
2. Spec deviations: the los rule is root-only (the M2 guard needs it), `LOS_STALL_CAP` is 2, the U1.13→U7.4
   assertion allows ≤ 2 vias at 0.15 mm, and the corner-tangent discretisation is not implemented.
3. wiccon outside-pad length is 1379.36 mm against the 1380 mm bound: within the bound, but with only 0.64 mm
   margin. One GND escape grew 1.8 → 17.5 mm.
4. On dcsgone, J1.A7→J1.B7 and U7.6→J1.A6 went from routed with a detour to failed (negotiation interplay; see
   above). Net failures are still down 67 → 59.
