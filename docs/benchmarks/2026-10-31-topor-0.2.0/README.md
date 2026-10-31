# Run 2026-10-31 — topor 0.2.0 (M2 single-layer router)

Engine `topor-engine` 0.2.0 (single-layer F.Cu PathFinder router, taut/arc smoother), KiCad 10.0.8.
Run is `run_bench(router=route_router, rip="all")` on copies of the benchmark boards: all tracks are ripped
(stitching/free vias, zones, keepouts and outline kept), the engine re-routes on F.Cu only, KiCad DRC with zone refill.
eprom-to-kernal ran via `tools/bench.sh eprom-to-kernal <dir> --rip all --router topor`; the other boards ran via
`run_bench(..., options={"wall_clock_s": 60, "partial_routes": False})` (the CLI has no options flag; nametag-sao and
wedding-gift via `pcb_override` on scratch copies of the project directories, sources untouched).
Structure follows [0.1.0](../2026-10-30-topor-0.1.0/README.md).

## Metrics

| Board | Routed / connections | Nets routed / total | Partial / failed | Segs / arcs | Outside-pad length vs hand | KiCad track+arc vs hand (info) | DRC routing_new | Unconnected | router_s / runtime | Iterations | Clinches | zero_slack_wires |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| eprom-to-kernal | **27 / 27** | **25 / 25** (GND pour-served) | **0 / 0** | 39 / 12 | **67.034 mm vs 76.598 mm** | 110.502 mm vs 112.886 mm | **0** | **0** | 0.108 s / 8.7 s | 16 | 0 | 1 (wire 24, +5V, kept, DRC accepted) |
| wiccon-2023 (info) | 8 / 60 | 4 / 27 | 0 / 52 | 13 / 5 | 368.262 mm vs 1690.138 mm | 380.742 mm vs 1831.971 mm | 0 | 49 | 0.026 s / 9.0 s | 1 | 0 | 0 |
| dcsgonebadge (info) | 22 / 231 | 6 / 66 | 0 / 209 | 39 / 17 | 192.384 mm vs 3890.918 mm | 218.436 mm vs 4167.082 mm | 0 | 192 | 47.482 s / 63.1 s | 9 | 0 | 0 |
| nametag-sao (info, user board) | 14 / 31 | 0 / 6 | 0 / 17 | 61 / 47 | 684.143 mm vs 1086.795 mm | 708.802 mm vs 1137.066 mm | 0 | 17 | 18.906 s / 28.7 s | 10 | 0 | 0 |
| wedding-gift (info, user board) | 32 / 81 | 21 / 43 | 0 / 49 | 60 / 28 | 321.053 mm vs 226.124 mm | 367.572 mm vs 259.522 mm | 0 | 49 | 1.069 s / 9.2 s | 12 | 0 | 0 |

"Partial" is the connection-level count (the run uses `partial_routes: false`); at net level wiccon-2023 has 2 partial
nets, dcsgonebadge 10, nametag-sao 3, wedding-gift 1. Unconnected counts equal the failed connections (nothing is
left dangling; `routing_new` 0 on all boards).

## M2 gate criteria (plan Global Constraints "M2 gate numbers") — eprom-to-kernal

| Criterion | Value |
|---|---|
| nets routed 25/25 (GND pour-served, not counted) | 25 / 25, GND `pour_served` |
| connections routed = all | 27 / 27 |
| partial == 0, failed == 0 | 0, 0 |
| `routing_new == 0`, unconnected 0 after `--refill-zones` | 0, 0 |
| width_match compared == matched == 25 | 25 / 25 |
| arcs >= 1, no `SS` pattern | 12 arcs |
| `length_outside_pads_nm` <= hand | 67.034 mm <= 76.598 mm |
| `router_s` < 2.0, threads_used 1, timed_out false | 0.108 s, 1, false |
| iterations <= 50 | 16 |
| two runs byte-identical | asserted by `tests/gate/test_m2_gate.py`; not re-measured in this benchmark run |

**The M2 gate is not closed.** The scaling test `tests/scaling/test_route_scaling.py` currently FAILS (n=100: 75/100
routed in 25.3 s; n=1000: timed out at 120 s) and is under diagnosis. This benchmark does not claim M2 done.

## Images

eprom-to-kernal, hand (left) vs topor (right), zone fills removed:

![eprom-to-kernal compare](images/eprom-to-kernal-compare.png)

Further images in `images/`: `*-hand.png`, `*-topor.png`, `*-compare.png` for eprom-to-kernal, wiccon-2023, dcsgonebadge, nametag-sao, wedding-gift.
Progress images of this gate: [eprom](../progress/2026-10-31-m2-gate.png),
[nametag-sao](../progress/2026-10-31-m2-gate-nametag-sao.png), [wedding-gift](../progress/2026-10-31-m2-gate-wedding-gift.png).

## Observations

- eprom-to-kernal: all 27 connections routed on F.Cu, 0 new DRC routing violations, 0 unconnected, outside-pad length
  9.6 mm (12.5 %) below hand. One zero-slack wire (`+5V` wire 24) is kept (DRC accepted). The solver stalled after 16
  iterations (no better routing for 8), warning `routable layers B.Cu unused`.
- Single-layer router: connections whose terminals have no F.Cu copper or that need a layer change are left unrouted
  on the 2-layer boards. That is expected and informational (M3 adds layers). All failed connections are reported
  as failed, nothing dangling.
- wiccon-2023: 8 / 60 routed (most parts sit on B.Cu, terminal not on layer F.Cu). 0.026 s.
- dcsgonebadge: 22 / 231 routed, 47.5 s router time within the 60 s wall clock (not timed out), 9 iterations; one zero-slack
  candidate rejected (`infeasible at margin 0: self-check: hole ...`). Width match 13 / 16 compared nets (informational).
- nametag-sao: 14 / 31 routed, 0 / 6 nets complete. The routed wires are long diagonal pairs; outside-pad length is
  684 mm vs 1087 mm hand because only part of the board is routed.
- wedding-gift: 32 / 81 routed, 21 / 43 nets. Routed length (321 mm) exceeds the hand length (226 mm) although fewer
  connections are routed: the hand board is not fully connected (58 unconnected items in the hand board) and the
  router routes many connections the hand board leaves open. Not a gate criterion.
- Raw data: [metrics/](metrics/).
