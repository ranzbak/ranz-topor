# copper-check: reading copper as numbers

`harness/copper_check.py` reviews a ROUTED `.kicad_pcb` without pictures. Use it in every verifier and RCA report instead of PNG review
of spots (user 2026-11-28). Local, one process, about 15 s on qmtech (70 s on the 11k-segment hand board), no routing, no pcbnew.

## One line for agents (paste into a report or brief)

    python3 harness/copper_check.py <routed.kicad_pcb> --out <dir> [--result <run>/result.json] [--spots harness/copper_check_spots.json]

Quote the top rows of `<dir>/copper-check.md` (kind, net, connection, x/y mm, size, minutes) and the total, never a screenshot.

## Requirements

- The board's `.kicad_pro` must sit next to the `.kicad_pcb` (same stem, or the only one in the folder) or be given with `--pro`: it holds the
  net classes (track width, clearance, `diff_pair_width`) every width and clearance judgement uses. Without it everything is judged against
  a 0.2 mm Default class and the necks are wrong.
- `kicad-cli` (KiCad 10) for the generated review rules; it runs in an isolated KiCad home (`harness/kicad_env`). `--no-drc` skips it,
  `--drc <drc.json>` reuses a DRC you already have (e.g. the plugin's `drc.json`).
- `--result result.json` (the plugin's report) only adds the topor connection id to rows.

## How to read the table (`copper-check.md`)

Rows are ranked by `minutes` (estimated hand cleanup; a ranking guess from the MINUTES table, not a measurement). `source`: `kicad` = a stock
KiCad DRC row (clearance, short, crossing...), `rule` = a row of the generated review `.kicad_dru` (severity warning; written to a temp copy,
never to the project; its minutes are 0 when an own row restates it), `own` = our check. The total at the top is a how-bad-is-the-board number.
`copper-check.json` has the same rows in the kicad-cli DRC JSON shape (`type`, `severity`, `description`, `items[].pos`) plus `net`,
`connection`, `size_mm`, `minutes`, `source`, so one reader handles both.

| kind | meaning |
|---|---|
| `neck_room_fits` / `neck_room_partial` / `neck_forced` | track narrower than the net's width (pair members: the class `diff_pair_width`); the free corridor beside it would fit the class width / more than drawn / nothing more (`corridor_fit_mm`) |
| `via_in_pad`, `via_overlaps_pad` | via centre inside / outline over an own-net SMD pad |
| `detour` | pad-to-pad (or junction-to-junction) piece over 1.3x the direct distance and over 0.5 mm longer |
| `end_off_centre`, `double_entry` | trace end inside a pad but not at its centre; two separate trace ends in one pad |
| `off_axis_entry` | the entering track's line misses the pad centre by more than 0.4 x the pad width (lane beside the axis, corner entry), fine-pitch pads |
| `wrong_side_entry` | entered from the body side of a ring footprint (angled = warning, straight out of the inner tip = info) |
| `power_chain_narrow_pin` | a net wider than 1/0.7 x a pad is chained through that pad |
| `bend_excess`, `acute_angle`, `free_angle` | many bends (info: the hand layout has as many), spike turn, copper off the 45 deg grid |
| `min_clearance_run` | run at exactly the minimum clearance (info) |
| `decap_far`, `diffpair_skew` | capacitor far from its supply pin; pair length mismatch |
| `*_rule` rows | the same findings as KiCad emitted them (KiCad caps a type at 199 rows) |

All thresholds are constants at the top of `copper_check.py`. Dropped: KiCad's `track_angle` rule parses but never fires (probed on 10.0.8).

## Calibration (2026-11-28)

Rates per 100 connections on the hand layout (qmtech-bh26, esp-rust-board) vs the user's topor run: see reports/copper-check-20261128-*.md.
Checks the hand layout also trips often (end_off_centre, acute_angle, free_angle, detour, bend_excess) do not discriminate; read them as
board-wide numbers, not as defects. Recall: `harness/copper_check_spots.json` holds the 15 user-circled qmtech spots; `--spots` reports recall.
