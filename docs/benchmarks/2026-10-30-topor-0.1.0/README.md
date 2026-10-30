# Run 2026-10-30 — topor 0.1.0 (M1 taut smoother gate)

Engine `topor-engine` 0.1.0 (taut smoother, T9a zero-slack classification), harness at the M1 gate, KiCad 10.0.8.
Run is `run_bench(router=smooth_router, rip="none", options={"existing_copper": "smooth"})` on copies of the three
benchmark boards (`tools/bench.sh <board> <dir> --rip none --router smoother`): the hand copper is converted
to taut chains of segments and native arcs, a wire that cannot be converted keeps its hand copper (fallback).
Structure follows [0.0.1](../2026-10-30-topor-0.0.1/README.md). Gate test: `tests/gate/test_m1_gate.py`.

## Metrics

| Board | Nets taut | Wires taut | Zero-slack fallbacks | Segs / arcs (hand) | Outside-pad length emitted vs hand (gate) | KiCad track+arc vs hand (info) | DRC routing_new | Unconnected | router_s / runtime |
|---|---|---|---|---|---|---|---|---|---|
| eprom-to-kernal | 24 / 26 (GND has no tracks, +5V partial) | 25 / 27 | 2 (wires 22, 23, net +5V) | 41 / 8 (53 / 0) | **71.494 mm vs 76.598 mm** | 114.894 mm vs 112.886 mm | **0** | **0** | 0.005 s / 6.5 s |
| wiccon-2023 (info) | 0 / 38 | 0 / 87 | 0 | 381 / 0 (381 / 0) | 1692.938 vs 1692.938 mm (all fallback) | 1831.971 vs 1831.971 mm | 0 | 0 | 0.010 s / 7.2 s |
| dcsgonebadge (info) | 23 / 86 | 385 / 595 | 21 (210 fallbacks in all) | 1176 / 160 (1369 / 14) | 3811.438 vs 3902.782 mm | 4119.052 vs 4210.279 mm | **3** | 6 items, 1 `via_dangling` | 1.07 s / 14.7 s |

Gate criteria (user rulings 2026-10-30): every wire taut or a zero-slack fallback; every net taut or partial/fallback
only through zero-slack wires; length **outside pad copper** emitted <= hand (fallback wires count their hand length
on both sides); `routing_new == 0`, unconnected 0, arcs >= 1, no `SS`, width match, `router_s < 2.0`, two runs
byte-identical (the report's wall-clock `runtime_s` excluded). eprom passes all of them. The KiCad track+arc total
is information only: it also counts the copper inside the terminal pads and the fallback wires' hand copper.

## Images

eprom-to-kernal, hand (left) vs topor (right), zone fills removed:

![eprom-to-kernal compare](images/eprom-to-kernal-compare.png)

Further images in `images/`: `*-hand.png`, `*-topor.png`, `*-compare.png` for eprom-to-kernal, wiccon-2023, dcsgonebadge.
The progress image of this gate task is [../progress/2026-10-30-m1-T1.4-gate.png](../progress/2026-10-30-m1-T1.4-gate.png).

## Observations

- eprom-to-kernal: 27 wires, 25 taut, 8 native arcs, 0 new DRC routing violations, 0 unconnected. `+5V` wires 22 and 23
  are zero-slack fallbacks: hand slack 0 nm to a foreign obstacle (board edge / J1), reason `infeasible: wall edge grazed
  mid-span by 10000 nm`; they keep their DRC-clean hand copper and are reported as `fallback: zero slack`. The net is
  `partial` (1 of 3 wires taut). Emitted length outside pads is 5.1 mm (6.7 %) below the hand routing.
- The KiCad total (114.894 vs 112.886 mm) is higher than the hand total because the smoother emits chains that start at the
  pad centre (copper inside the pad counts in KiCad's total but not in the gate length) and the zero-slack wires keep hand
  copper; the gate compares only length outside pad copper.
- wiccon-2023 (informational, not an M1 criterion): every wire falls back (`sketch: cdt: constraint (n, n+1) crosses another
  constraint`, 87 wires); the board is returned unchanged (routing_new 0). Open issue for M1.5: the constrained triangulation of
  the board rejects a constraint pair, which fails all wires at once.
- dcsgonebadge (informational): 385 of 595 wires taut, 210 fallbacks (21 zero slack; others: `inside obstacle: edge` 50,
  `sketch: hand vertex on CDT vertex` 38, `sketch: polyline passes through triangulation vertex` 17, `self-check: kept` 15,
  `infeasible: wall pad` 15, ...). **3 new DRC routing violations** (`items_not_allowed` on an `I2C_SCL` track, `connection_width`
  1 -> 2 on a GND via/zone, `via_dangling` on a +3V3 via): the informational test `test_m1_smoother_other_boards_complete[dcsgonebadge]`
  fails on `routing_new == 0`. Not an M1 criterion; to be diagnosed.
- Deferred items of the [M1 gate review](../../progress/notes/m1-gate-review.md):
  F2 `SS` from degenerate arcs (mitre fix, M1.5); F3 wall-clock deadline not consulted outside relax (M1.5);
  F4 O(k^2) spots A1-A5 and the disc test in drop rule (e) (M2); F5 stack-order fan, harness-side T-split of kept items,
  wall foot anchors, hand-arc chord length (M1.5).
- Raw data: [metrics/](metrics/).
