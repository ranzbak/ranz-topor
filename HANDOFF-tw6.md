# TW6 VIA-SITES handoff (2026-11-19, updated round 4)

## Round 4 state (read first)
- Lead decisions on the r3 questions, implemented in 2659db8a (main 524465c2 merged in 8dc3fac3):
  1. E keep judgement: graded detour price `[optimize] site_move_detour` (excess-ratio scale, default 0.5, 0 = none),
     per relaid connection `viol x ((len/straight - 1)/scale)^2` after minus before (`core::conn_detour_price`,
     `core::site_keep`, `Rerouter::judge_extra` in `reroute_set` for (i), `Sp::reroute_d`). Symmetric: shortening a
     detour gains. Refusals counted (`loop.site_move_detour_refused`, report line).
  2. `[capacity] via_disc_height` is now "auto"/"on"/"off" (default auto = on with `[vias] site_price = "graded"`;
     config.py `_site_graded`); the tw6_ab cfg `s` therefore = r3's `sh`.
  3. planned_crossing_window -> CROSS-DRAW (F2/F3). Not worked on here.
  4. B search term (via_site_social) parked as is (opt-in, weight 0), no retry.
- Low findings: M13 -> `State::apply_layering` (layers_now + free wires + site_k in one call; no loop fixture showed a
  copper change: crop x 6 option sets, keepout board x 8). branch_move never tried -> `viaopt::tee_arms` now takes the
  interval's events: a junction's tee event (trunk k, fraction 0.5) bounds the strict interval, it is never inside it.
- tw6_ab.sh: suffixes z (site_move_detour 0), d (= $DET, default 0.25); mode r4 (tests + lint + off + pytest + eq).
- A/B tw6r4-8dc3fac done (report scratch/agent-wt/reports/tw6-via-sites-r4-20261119-1330.md): no flips; mj best qmtech (253->138), esp flat; s = sh. Next lever: detour price in the regular sweep keep (changes default copper, needs lead). Mutant D4 (relay_kept plumbing) survives, no fixture. Nothing in flight.


## Round 3 state (read first; older notes below)
- A/B re-run done (tw6acd2-0967792) and B search + E A/B done (tw6be-094cee3), both on topor-rsc-c0c953ad. Report:
  scratch/agent-wt/reports/tw6-via-sites-r3-20261119-1040.md (+ .json). No default flips; off = main copper.
- B search term DONE opt-in: `[factors] via_site_social` (d1fd284d). E DONE opt-in: `[optimize] site_move`,
  `branch_move` (8ccfb0e0). Tests engine/tests/topo_site_social.rs. Mutants 12/13 killed (M13 survivor: set_lr refresh).
- tw6_ab.sh: `be <cfgs>` mode (tests + clippy + off + pytest + pats + eq); suffixes b (SITE_W, default 50), m, j.
- Open for the lead/user: graded detour price in E's keep judge; site_price implies via_disc_height?; pcw is a
  metric/drawing defect (to branch cross-draw F2/F3); B weight 10-20 retry or park.
- Nothing in flight. The lib-test fix (ba566bae, factor order) has not run on the VM yet: include `--lib` next job.


Branch tw6-via-sites, worktree scratch/agent-wt/wt-tw6. main 12699012 (tw4-r2) is merged in (271153d7).
Design note: docs/dev/design/tw6-via-sites.md. Lead orders: parts in the order A+C, D, B (search term), E. Every part is
opt-in until the equal-time A/B (300 s, 4 threads, esp s0-4 + qmtech s0, numbers from ladder_diff --json). A default
flips only with no new det > 1.5 and no rise in KiCad new.

Avoid editing what tw4r2-fix touches: tests, the pour meter, pack.rs:292, and the TOPO_PX_TERMS hook.

## Done (opt-in, default copper = main)
- **A + C** (0f507d00), option `[vias] site_price` = "off"/"graded" (engine `via_site_price`):
  - `Packer::site_room` and `site_deficit`.
  - `Frame::site_over` inside `Frame::wire_over`, the CM1 entry.
  - `Frame::rank` breaks ties by deficit.
  - Pad-exit term (c) is skipped when the site price is on.
  - The drawing packs discs at the via-point height under via_disc_height: `Drawer::set_wire_layers`, `relayer`, and
    core.rs ~6762 / ~6841.
  - C: `Packer::lead_ends` (set in `route_round` after `Packer::for_model`) and `Frame::lead_via`.
- **D** (75c08578), option `[vias] pair_room` (engine `via_pair_room`):
  - In viaopt.rs: `ViaCtx::pair_room`, the `PairNeed` map, and `gap_at`'s pair need.
  - No move of a tight via without gain.
- Tests: engine/tests/topo_site_price.rs.
- Job script: tools/tw6_ab.sh.
  - Suffixes: s = site_price, h = via_disc_height, x = pad_exits, p = pair_room.
  - Modes: tests, off, pats, eq, ac.
- **B layering** (3c05d27c, under site_price): `layers::site_amount`, the `assign_two` edge weights, and the `assign_n`
  `Costs::site` / `trans_at`.
- **In flight: nothing.** Job tw6acd-3c05d27 (rev 3c05d27c, `ac - s p h sh shp`) started on topor-rso-c96c59d. It died
  when the local machine crashed at 09:19 and the VMs were reaped. No partial results were fetched (no
  hz-runs/tw6acd-3c05d27). **Re-run first:**
  `scratch/agent-wt/hetzner/hz.sh job --type ccx43 --wait --rev 3c05d27c --tag tw6acd-3c05d27 -- bash tools/tw6_ab.sh ac - s p h sh shp`
  - Start it with run_in_background and block on its rc. Mark the QUEUE line.
  - It runs: tests (topo_site_price, cm1_room, disc_height, pad_exit, lib), off K x4 (default = main copper), pats
    (pcw / site_or_stay / lead_via_end, K and M, cfgs - s p sh shp), and equal time on esp s0-4 + qmtech s0 for 6
    cfgs.
  - Nothing in this code has run on a VM yet; it is only checked locally with `cargo check --release --tests`.
  - Clippy and the full suite are still to do before a merge.
  - Read them with `python3 scratch/agent-wt/tools/ladder_diff.py <base> <tip> --json`. The base is the `-` runs of the
    same tag.
  - Each cfg is a separate dir: `$R/e0s/esp`, and so on.
  - Re-measure the tw4-r2 (2b) nets by name: IO6, ENABLE, IO7, IO5, IO10_SDA (esp); ADV_DVDD, JS1_1, Net-(U7-PVSS)
    (qmtech).
  - Re-measure the (1c) nets too: C6.1, IO7, D6-Pad2, USB_C_D+, U7-PVSS.

## Next: the rest of part B
1. **Layering, 2 layers. DONE.**
   - Where: layers.rs `assign_two` ~1383, `g.add_edge(x[0].node, x[1].node, ±p.via_mm)`.
   - Change: with site_price, the magnitude becomes `via_mm + cap_mm * amount(stretch x[0].k ..= x[1].k)`. The amount
     is `1 + deficit / (d + s)` when the best `packer.site_room` over the stretch's triangles is below r, else 0.
   - Use only the traversals strictly between the events with via_between (`refine::via_ranges`).
   - The packer is `Option<&Packer>`; amount 0 without it. Add a `LayerParams` flag.
   - Weights must stay deterministic. `StretchCache` keys need the flag (`CacheInner.params`).
2. **Layering, N layers. DONE.**
   - Where: `assign_n` ~1811, `tv = costs.trans(a, b)`.
   - Change: add the same per-stretch amount. This needs the wire and k of each event in `wev`; read how `np` / `wev`
     are built above 1780.
3. **Count.** Optionally make `wire_stretches`' `no_site` use `packer.site_room` (the two-tests finding in the note).
   - Leave it off by default unless the A/B says otherwise.
4. **Search term.**
   - Where: init.rs `arc_chords` / `fill_chords`.
   - Change: a crossing of (o, k) costs `site_mm * min(D_own(t), D_o(o, k))`. This is a new `[factors] via_site_social`
     (default 0), stored per wire and pass like `cross_k` in state.rs, refreshed in `set_lr`.
   - Design note "B" has the details.
5. **Test.** planned_crossing_window must lose its 4 clearance errors under `s` + B, via SDA diving (0.6 mm via) or the
   crossing moving into the window.
   - The dump recipe is `tools/tw6_job.sh round1` (TOPO_VIADBG / TOPO_VIAREL).

## Then part E (loop moves)
`site_move` / `branch_move` after `pair_move` in core.rs (`Sp::pair_move` ~5217; cycle order ~5560-5700). The design
note has the details.

## Rules reminder
- VM only for tests and runs; local work is limited to cargo check through slot.sh.
- Each run gets a QUEUE.md line.
- Report: .md, plus a .json checked with digest --check, plus a handback of at most 15 lines.
- Literature check per part, cited in the design note.
