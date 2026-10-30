# Benchmarks — routing quality over time

One folder per run, named `YYYY-MM-DD-<router>-<version>`, each with the same structure so
runs can be compared side by side:

```
benchmarks/
  <run>/
    README.md      what was run, settings, metrics table, observations
    images/        <board>-hand.png, <board>-<router>.png, <board>-compare.png
    metrics/       <board>--metrics-*.json, --drc-*.json, --widths-*.json
```

## Benchmark boards

| Board | Layers | Role | Notes |
|---|---|---|---|
| eprom-to-kernal | 2 | simple | 26 nets, THT only, hand routing uses 0.4 mm power traces |
| wiccon-2023 | 2 | medium | 38 nets, round board, most parts on B.Cu |
| dcsgonebadge | 2 | robustness only | GND pour instead of planes; **not a quality reference** (user decision) |
| bornhack2026 = **Cyber aegg** (`badge.team/bornhack2026/bornhack2026-hardware`) | 4 | advanced + complex runtime reference | keepouts, RF paths, ~877 vias (many stitching), 200 footprints, 1416 segments; budget **≤ 2 h** full routing; not yet run |
| tanmatsu (`badge.team/tanmatsu/tanmatsu-hardware`) | 4 | complex, runtime reference | ~1820 nets, 436 footprints, 4933 segments, 1099 vias; budget **≤ 2 h** full routing |

Source projects live in `~/work/kicad/projects` and are **never modified**: every run works on
copies (verify with sha256 before/after).

## Run procedure (keep identical between runs)

1. Copy the project directory; on the copy remove routed tracks and their transition vias.
   Keep stitching/fence vias, RF nets and locked items, zones, keepouts and outline.
2. Record per-net track widths of the original before ripping.
3. Route the ripped copy with the router under test; save as an openable project.
4. Refill zones, run `kicad-cli pcb drc --format json --refill-zones --all-track-errors`.
5. Metrics: routed / unconnected, DRC violations by type, total length (same nets as hand),
   vias, segments vs arcs, width match per net, runtime, thread count.
6. Images: copper layers F.Cu + B.Cu (+ inner layers for 4-layer boards) and Edge.Cuts,
   zone fills removed, board area only, rendered with `kicad-cli pcb export svg` and
   converted to PNG at 1100 px height; hand-routed left, router right.

## Runs

| Run | Router | Summary |
|---|---|---|
| [2026-10-30-topola-d832b164](2026-10-30-topola-d832b164/README.md) | Topola `develop` d832b164 | baseline: only the simple board routes; see [43](../design/43-topola-assessment.md) |

## Progress PNGs

`progress/<YYYY-MM-DD>-m1-<TaskId>.png` — one image per M1 task that changes the eprom-to-kernal
routing outcome (hand left, topor right, zone fills removed). Produced by
`.venv/bin/python -m harness.progress --task <TaskId>`; the caption carries nets/wires converted,
new DRC routing violations and total copper length vs the hand length of the board under test (175.6 mm for eprom-to-kernal, re-pinned 2026-10-30 after the KiCad 10 hand re-route).
