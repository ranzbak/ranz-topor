# Task A6 + NX report: pad exit stubs (off-centre starts, bridges) and necking to the pad

Status: **DONE_WITH_CONCERNS**. Branch `worktree-agent-a52787a1d9b26a2bb`, based on main `946530c`
(fast-forwarded from `68a77b6`). Engine for every Python run: `PYTHONPATH` override
(`target/pyeng*/topor_engine/__init__.py`, printed by every run), never the shared `.venv` build.

dcsgone (gate options): **210/241 routed, failed 31** (main 201/243, failed 42), routing_new 0, unconnected 35
(41), vias 175 (174), detour max **4.56** (22.3), count > 1.5 unchanged (8 / ghost 15). eprom and wiccon
produce the same result as main (0 stubs). The gate is not reached; #201 U1.13→U7.4 routes alone but not
with the whole board (concern 1).

## What changed, per file

- `engine/src/stubs.rs` (new, self-contained): plans the stubs of one routing pass.
  - `ExitStub` (pad, start `S` inside the pad, end `E`, width, net width, lateral offset, `bridge`),
    `StubPlan` (`stubs`, `by_pad`, `bridges` by connection id, warnings, checks), `StubOut` report row,
    `plan_stubs`, `with_stubs`, `attach`, `stub_duplicates`.
  - **Exit stub**: straight track from `S` (inside the pad by ≥ 5 µm) along a pad axis to
    `E = c + d̂·(h + clr + w/2 + ε) + o·n̂` (design note §3).
  - **When (auto)**: test "T0" = the M3 attachment (centre, full width, along any axis, with the round end
    at `E`) violates the clearance in the `Checker` (margin ε). Otherwise no stub. THT pads never get one.
  - **all**: additionally every SMD terminal pad with a long axis.
  - **Candidate order**: directions ranked long axis first, then the free room at `E` (clamped at
    `clr + w`), then toward the connections' other ends. `E` must not lie on another pad (any net).
    Start points `t ∈ {0, L/2, L}` along the axis. Then:
    - A1: centred, full width;
    - B1: off-centre, full width (offset scan, 5 µm steps, ≤ 128 samples; the feasible run nearest the
      centre line, its middle sample);
    - A2: centred, necked;
    - B2: off-centre, necked.

    Necks are the widest clean width (bisection, whole µm), at least the effective minimum, and at most
    `neck_max` long.
  - **Effective minimum width**: `max(rules.min_track_width, NetRule.min_width)`.
  - **Bridges (phase 1)**: a connection between two SMD pads of one footprint, ≤ `pad_bridge_max_nm`
    apart, gets one straight track from inside A to inside B. The centre line is tried first; ends may
    be pulled in (`t` steps) so the round ends of a wide track stay off neighbours. Otherwise the
    offset run nearest the centre is used. In auto mode a bridge is only tried when one of the two pads
    fails T0. Such a connection is routed by that one track and leaves the routing pass.
  - **Guards**: a pad whose centre lies in a foreign clearance zone gets no stub (`stub infeasible`),
    which keeps the M3 "inside obstacle" class. Every candidate is one `Checker` call against the board
    and the stubs already accepted. Each pad is capped at `STUB_CHECK_CAP` (4096) checks. `stop`
    (deadline / cancel) is checked before every pad and every bridge.
  - Big-O in the module doc: `O((T + B) · K · q)`.
- `engine/src/route.rs`:
  - **Pass setup.** `route_pass` plans stubs after `decompose`. Bridged connections leave `live`.
    `attach` points pad terminals at `E` (`TermKind::Stub`). The pass then runs on
    `with_stubs(model)`: the stubs are kept copper of their net, so obstacles, CDT, via sites and the
    self-check all see them.
  - **Stub ends are terminals.** They are handled like copper junctions: terminal obstacle,
    `TerminalInfo.r = w/2`, escape-site pad.
  - **Assembly.** A routed connection emits its stubs as its first / last item. They count in
    `length_nm`, `length_outside_pads_nm` and `length_by_layer`. The ghost / reference lengths get the
    stub length added, so both sides of the detour ratio run pad to pad. A bridged connection is
    reported routed with its one track, `ghost = ref = length`, `ref_kind feasible`.
  - **Reporting.**
    - `ConnReport`: `stub_nm`, `neck_nm`, `neck_width_nm`.
    - `RouteResult.stubs`: `Vec<StubOut>`, with `used`, `necked`, `offset_nm`, `bridge` and
      `outside_nm`.
    - `merge_pass` concatenates the stub lists.
  - **Shared stubs.** Every connection carries its stubs, so a stub survives while any of them stays
    routed after `drop_conns`. `route_inner` uses `unique_items` for the pour checks and for
    `with_routed_copper`. `dedupe_stub_items` removes the duplicates at the end and corrects the totals.
- `engine/src/decompose.rs`:
  - `TermKind::Stub { pad, start }`; `terminal_of` maps it to `Terminal::Stub`.
  - **Bug fix**: same-net pads whose copper touches on a common layer are one cluster
    (`skeleton_gap`, `pad_reach` prefilter, `O(np²)` per net). The USB-C pad pairs J1.A4+B9 and J1.A9+B4
    were zero-length connections that failed with "sketch: inconsistent slot path".
- `engine/src/wires.rs`: `Terminal::Stub { pad, end, layer }` (`at()` = `end`, a fixed point).
- `engine/src/model.rs`:
  - New `Options` fields: `exit_stubs: String` ("auto"), `neck_to_pad: bool` (true),
    `neck_max_nm: i64` (0 = 2 × the pad's long side), `pad_bridge_max_nm: i64` (2 mm).
  - New `NetRule.min_width` (0 = none).
- `engine/src/vias.rs`: `via_sites_stubs` (`via_sites_opts` wraps it, signature unchanged). It adds the
  4 escape positions of a stubbed pad as sites of the pad's own net. Without them the pad's own stub
  removed its escape via, and the net-0 fit failed against the stub. This touches A10/A13's file in one
  additive function.
- `toporlib/rules.py`: `min_width` per net rule = the largest custom-rule `track_width (min …)` that
  matches (0 otherwise).
- Plugin:
  - `schema.py`: `exit_stubs` is now engine-mapped. New keys `neck_to_pad`, `neck_max_mm` and
    `pad_bridge_max_mm`, all `since=V5`.
  - `config.py`: scale factors.
  - `report.py`: lines "Exit stubs:" and "Necked stubs (narrower than the net width):".
  - `docs/user/configuration.md` regenerated.
  - `docs/user/operate.md`: a stubs bullet; stubs removed from "Not yet".
- Tests:
  - `engine/tests/route_stubs.rs` (8 tests): the 4 briefed ones plus
    `bridge_joins_same_net_pads_of_a_footprint_off_centre`, `touching_same_net_pads_are_one_cluster`,
    `neck_to_pad_narrows_wide_net_at_fine_pitch_pad` and `stubs_threads_byte_identical`.
  - `tests/test_rules.py::test_custom_rule_track_width_min_is_the_net_min_width`.
  - `tests/plugin/test_report.py::test_report_lists_exit_stubs_and_necks`.

## Decisions (deviations from the design note, please review)

1. **Auto criterion is geometric (T0), not "an own tip/spoke edge of the graph is unfit".**
   - It needs no graph before the terminals are known.
   - It is exactly the A2 finding (U7 middle pads) plus the NX class (net wider than the pad row allows).
   - Effect: eprom and wiccon get 0 stubs (identical results); dcsgone gets 6 exit stubs and 6 bridges
     (12 used).
2. **Bridges (`pad_bridge_max_mm`, new key, default 2.0 mm, same footprint only).**
   - Not in the design note. The off-centre start alone does not route U7.4→U7.7 / U7.2→U7.9: both pads
     get outward stubs, and their routes must circle the package. The U7.1–U7.10 and U7.5–U7.6 links
     close that ring, so these failed with "blocked".
   - The DRC-clean solution is one straight track 50 µm off the centre line. U4.14→U4.15 (0.5 mm net)
     needs the same, with ends pulled in.
   - Auto mode only bridges where a pad fails T0.
3. **Direction order adds "toward the connections' other ends"** after the design note's free room. A stub
   end may not lie on another pad. Without that, U4.14's stub ended inside U4.15 and the route from there
   failed in the sketch.
4. **Off-centre at full width (B1) is tried before necking (A2).** The net keeps its width where it fits.
   - dcsgone v2 has 0 necks: U4.14 / U5.2 are covered by B1 (0.1 / 0.125 mm off centre) and bridges.
   - With the opposite order (run v1), U5.2 / U5.3 were necked to 0.289 mm.
5. **`neck_to_pad` default true.** routing_new 0 on all six snapshot boards. Necks actually used:
   - bornhack2026: 2 (U3.21 0.309 mm and U3.22 0.200 mm, net 0.342 mm);
   - dcsgone: 0 in the final run, 2 in v1 (0.289 mm), also routing_new 0.
6. **`neck_max_mm` default 0 = 2 × the pad's long side** (the DX rule as a per-pad value, not a fixed mm).
   Measured neck lengths are 0.705 mm (U5, v1) and ≈ 0.71 mm (bornhack U3), all well under the cap.
7. **Overlapping same-net pads merge in decompose.** This is a correctness fix (KiCad connects
   overlapping pads). dcsgone `connections_total` 243 → 241.
8. **Stub length is added to `ghost_len_nm` / `ref_len_nm`** of stubbed connections (pad-to-pad ratio).

Options (judgement calls), each a `topor.toml` key, `since 0.5.0` (commented out until the version bump):
`[search] exit_stubs` (auto), `neck_to_pad` (true), `neck_max_mm` (0), `pad_bridge_max_mm` (2.0). Internal
constants: `OFFSET_STEP_NM` 5 µm, `OFFSET_SAMPLES_CAP` 128, `WIDTH_BISECT_STEPS` 12, `STUB_CHECK_CAP` 4096,
`START_DEPTH_NM` 5 µm.

## Tests

- **TDD.** The tests came after the first planner skeleton. Red was shown by disabling the planner
  (`A6_RED`): 4 of 5 fail.
  - `exit_stubs_off_matches_previous` is pinned to a golden digest from main 946530c: FNV-1a of the
    canonical items JSON plus the per-connection outcomes, computed with the main engine through Python.
  - Final: `cargo test --release --test route_stubs`: 8 passed.
- **Full `cargo test --release --no-fail-fast` on the final code: 41 of 43 suites green.** The 2 failures
  are not caused by A6:
  - `route_detour_ref::reference_is_feasible_not_topological` fails identically on main 946530c
    (checked by building `git archive 946530c` in `target/main-src`);
  - `route_engine::anytime_timeout_emits_committed_subset` also fails on main (known flaky).
  - Fixed along the way: `route_terminal_inside_foreign_obstacle_fails_once`, which the first draft broke
    and the centre-blocked guard now covers. The M2 guard `eprom_ripped_front_only_is_the_m2_result`
    passes.
- `cargo clippy --all-targets -- -D warnings`: clean.
- **Determinism.** dcsgone model with `route_probe`, wall clock off: threads 1, 2 and 4, plus a repeat, all
  give md5 `af5bcc6fdb03c79730b98124c6be3ae3` (210/241, 175 vias, 23 iterations). The crop test does
  threads 1, 2, 4, 4 in-test.
- **`pytest -q` default suite (engine override): 499 passed, 7 skipped, 4 failed.**
  - Fixed after the run: `test_docs_links::test_operate_md_reaches_every_kicad_setting_and_toml_key`.
    `operate.md` now names the new KiCad place, and the doc / config / engine-surface tests re-run green
    except the next item.
  - The other 3 also fail on main:
    - `test_config_docs::test_keys_without_effect_say_so` (main's `configuration.md` has 0 no-effect
      markers; confirmed on the main source);
    - `test_router::test_progress_m4_name_and_caption_lines` (fails with the main engine; A7's detour
      column);
    - `test_scaffold::test_cargo_workspace_builds` (the debug `cargo test` hits the main
      `route_detour_ref` failure).

## Numbers

Gate options, `run_bench(rip="all", threads=4)`, same loaded machine (load 15–25) for the base and the A6
column. Counts are exact; times are inflated. The main run reproduces the coordinator's base numbers
exactly.

### dcsgonebadge 3e7e66f862a7

| | main 946530c | A6 |
|---|---|---|
| routed / connections_total | 201/243 | **210/241** |
| failed / partial | 42 / 0 | **31** / 0 |
| vias | 174 | 175 |
| routing_new / preexisting | 0 / 4 | 0 / 4 |
| unconnected | 41 | **35** |
| router_s / runtime_s (loaded) | 152.3 / 235.3 | 80.3 / 152.9 (PNG run); 91.3 / 226.7 (bench) |
| iterations / stop | 25 / no gain | 23 / no gain |
| detour (reference) median / p90 / max / count > 1.5 | 1.000 / 1.296 / 22.31 / 8 | 1.000 / 1.243 / **4.56** / 8 |
| detour_ghost median / p90 / max / count > 1.5 | 1.000 / 1.335 / 22.31 / 15 | 1.000 / 1.313 / **5.44** / 15 |
| outside pads | 1696.8 mm | 1698.5 mm |
| stubs | – | 6 exit (5 used), 6 bridges, 8 off-centre, 0 necked |

- **Detour count.** The count > 1.5 is equal and the max improved, so this is not worse.
- **Top detours (A6).** GND 4.56, J1-CC1 3.36, U1-EN 2.01. On main: GND U3.2→pour 22.3.
- **Fixed (23).**
  - Net-(BT1-Pin_1): C27.2→U5.2, TP10.1→U4.14, U4.14→U4.15, U4.15→U4.16, U4.16→C26.1, U5.2→U5.3.
  - Net-(Q2-S): C25.2→U4.1, U4.1→U4.20.
  - Net-(J1-CC2): U7.2→U7.9, U7.9→J1.B5.
  - USB-: U7.4→U7.7.
  - D23-A: J1.A4→B9 and A9→B4 (merged pads).
  - Also: U4-STAT2 R11.1→U4.7, GND U7.8→pour, GND C24.1→pour, D12-DOUT, TFT_D~C, VCC C14.1→C15.1,
    VCC D11.1→C11.1, +3V3 J3.17→R16.2, STRAP_8, IR_TX.
- **Newly failed (12, negotiation interplay).** USB+ U1.14→U7.5, USB+ J1.A6→J1.B6, USB- J1.A7→J1.B7,
  CC1 R6.1→U7.10, CC2 J1.B5→R5.2, +5V U4.17→TP9.1, NTCLEV U4.5→BT1.3, I2C_SDA ×2, SPI_SCK, TACT_A,
  +3V3 R1.1→R16.2.
- **#201 U1.13→U7.4 and #203 U7.4→J1.B7 still fail on the whole board ("blocked: no path").** Routed alone
  on the same model (scope USB-), all 4 USB- connections route:
  - U1.13→U7.4 23.2 mm with 2 vias;
  - U7.4→U7.7 0.775 mm bridge;
  - U7.4→J1.B7 3.63 mm with 0 vias;
  - J1.A7→J1.B7 2.71 mm.

  The remaining failure is congestion in the U7/J1 corner (A5 Steiner, A16 rip-up), not entry geometry.
  On main these were statically BLOCKED even on the empty graph.

### eprom-to-kernal (not worse: identical)
27/27, vias 0, routing_new 0, unconnected 0, outside 64.628 mm (≤ 64.7), router 0.149 s. Detour max 1.15,
> 1.5: 0; ghost max 1.31, 0. Route JSON identical to main except the new fields: hash without A6 fields
`cba05eb5af88c17b` for both; items hash `7505c62367b92ec0` for both. 0 stubs. The M2 guard test
(`eprom_ripped_front_only_is_the_m2_result`) passes in the cargo suite.

### wiccon-2023 (not worse: identical)
62/62, vias 36, routing_new 0, unconnected 0, outside 1367.28 mm (bound 1380: 12.7 mm headroom, unchanged),
router 5.1 s. Detour max 1.27, > 1.5: 0; ghost max 1.28, 0. Identical to main without the A6 fields
(`f97378129e5e793d`, items `1b02c73682a9a1d6`). 0 stubs.

### Other snapshot boards (neck default; default options, 60 s, loaded, informational)

| board | main | A6 | stubs |
|---|---|---|---|
| nametag-sao | 32/32, 2 vias, routing_new 0 | 32/32, 2 vias, routing_new 0 | 0 |
| wedding-gift | 76/107 (timeout), routing_new 0 | 77/110 (no gain), routing_new 0 | 0 |
| bornhack2026 | 139/307 (timeout, 3 iterations), routing_new 0 | 131/288 (timeout, 2 iterations), routing_new 0 | 12 planned, 7 used, 2 necked |

## PNG
`/home/paul/work/kicad/plugins/topor/.claude/worktrees/agent-a52787a1d9b26a2bb/docs/benchmarks/progress/2026-11-01-m4-A6-dcsgonebadge.png`
(not committed). Caption: "A6: 210/241 connections, 48/66 nets, partial 0, failed 31, vias 175, arcs 182,
zero slack 0, iters 23, pcb 3e7e66f862a7 / DRC routing 0 preexisting 4, unconnected 35, outside pads 1698.5
vs hand 3574.7 mm, F.Cu 975.6 / B.Cu 953.1 mm, router 80.26 s on 4 threads / detours > 1.5: 8 (max 4.6) —
GND 4.6, Net-(J1-CC1) 3.4, Net-(U1-EN{slash}CHIP_PU) 2.0 vs DRC-feasible reference (ghost fallbacks 0,
ghost < 2 mm 55) / stop_reason: no gain".

## Concerns
1. **#201 / #203 (U7.4 to U1.13 / J1.B7) still fail on the whole board.** They route alone. Twelve
   connections newly fail through negotiation interplay; net failures are 42 → 31. The gate needs
   A5 / A16 in the USB corner.
2. **bornhack2026 is 131/288 vs 139/307**, both timeouts at 60 s on a loaded machine (2 vs 3 iterations).
   This is not separable from the load.
   - Its BGA pads produce 34 `stub infeasible` warnings (U6/U10 balls: axis stubs cannot leave a ball
     grid). These are warnings only.
   - Worth a re-run on a quiet machine.
3. **`vias.rs` changed** (A10/A13 file): one additive function, wrapper signature unchanged.
4. **`gate_diag` example is not updated.** It still builds the M3 terminals, so it does not see stubs.
5. **dcsgone runtime_s was 796 s in the first loaded run (v1).** The final code ran 153–227 s whole run,
   80–91 s router. Timings need a quiet machine.
