### Task FF (opus, rotation 2): fine-pitch fan-out — pour-net pads bridged inside the footprint, no via in front of a neighbour's tip (DG R2.4 item 2)

Read rotation-2-common.md (incl. "Round 2 update") and the private-badge-b completion diagnosis (note removed: private board) R2.3 (d) 1 and R2.4 item 2.
Problem: every GND pad of a QFN/DFN (U4, U5, U6, U7, J1 on private-badge-b) gets its own pour via 0.5–1 mm in front of its tip, and other
nets' vias land in the same front row: adjacent signal pads are boxed in (+5V U4.2's three connections never route in 13 runs although
each routes alone; U4.8, U4.12, U7.4/U7.9 likewise). The hand bridges GND pads into the exposed pad (EP) or the neighbouring GND pad of
the same footprint and puts one via away from the signal side (U4.3→U4.21, U4.9→U4.10, U4.11→U4.21, U5.1/U5.4→U5.9, U6.4→U6.9,
U7.3→U7.8, J1.A1/B12).
Do (design choices are yours, measured): (1) decompose.rs pour edges: one pour connection per same-footprint group of pour-net pads
joined by short bridges (A6 pad bridges ≤ pad_bridge_max_mm, or a bridge into the EP), an EP preferred as the group's pour terminal;
pour reach must count a bridged pad as reached through its group (pourcheck). (2) optional second half, by measurement: a via site in the
front row of a fine-pitch pad (within one pitch in front of its tip) is not offered to another net while that pad's connection is
unrouted, or front-row sites are staggered/pushed out. Options: `[layers] pour_pad_bridges` (default true only if eprom / wiccon not
worse — wiccon has a GND pour: vias ≤ 37, outside ≤ 1380 mm) + whatever (2) needs; schema + docs regenerated.
Test: a private-badge-b U4 crop fixture: U4.3, U4.9–U4.11 joined to U4.21 with no via in front of them, U4.2→U4.4 routes; pour-reach unchanged;
threads byte-identical. Expected (DG): ≈ 5 direct (U4.2 ×3, U4.8→R13.1, R8.2→U4.12) + 2–4 in the USB corner. PNG `--task FF --board
private-badge-b`. Commit `feat(decompose): bridge pour-net pads inside fine-pitch footprints` (+ via-site commit). Report task-FF-report.md.
