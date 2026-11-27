# PAD-CENTRE-MAIN: what KiCad's Cleanup deletes on main (RCA, design options)

## Measured on main (tree b59e745e; cached det s0 runs hz-runs/dab3/off-*, rev 36ee0053 = same engine/plugin/harness)
Real dialog (Xephyr :6, own KiCad home, every box on; wt-pcm-runs/cl0), replay = dialog item for item:
- qmtech 179 = shorting 21 + dangling_track 147 + dangling_via 11; esp 59 = shorting 4 + dangling 48 + via 7;
  wiccon 0, eprom 0.
- `dangling_split`: 0 independent removals. Each option alone: only "Delete tracks connecting different nets"
  removes anything (21 / 4). in_pads, merge, dangling alone: 0.
- **No pad-centre class exists on main.** No track_in_pad, no merge_collinear, no off-centre end is deleted.
  All 238 items are ONE class: a written short and its cascade.

## RCA: the never-drop pass writes shorting copper; Cleanup then removes BOTH nets' traces
- core.rs:8800 (never drop, M-B1, `[optimize] allow_violations = true`) draws a wire that has no legal geometry
  on its best geometry, shorts included. KiCad's `removeShortingTrackSegments` deletes every track whose
  connectivity touches another net. That hits both sides. `deleteDanglingTracks` then deletes the rest of each
  trace up to its pads or T. postrun.py:265 already warns about this. docs/user/operate.md documents it as "the
  one exception".
- Split (wt-pcm-runs/sim/split.py):
  - qm: 110 items on the 6 nets whose own wire was forced (vbat 42, DDCSDA 24, DDCSCL 22, BGVDD 16, AVDD 4,
    U7-PVSS 2), plus **69 victim items on 4 nets drawn legally** (MAX_I2S0 29, VIN5V 19, MAX_LRCLK 12,
    ADV_DVDD 9).
  - esp: 39 forced (IO7 26, IO9_BOOT 13) + **20 victim** (IO10_SDA 13, +3V3 7).
- Why each short exists (compass class of the engine violation):
  - qm planner over-full / crossing F6/F2: AVDD-DDCSDA, BGVDD-I2S0/LRCLK.
  - qm via without a legal site: DDCSCL/DDCSDA, the ADV7511-DDC-SHORT item.
  - qm pad exit over a neighbour pin: U7-PVSS over U7.21.
  - qm drawing loss / foreign copper F4: vbat-VIN5V.
  - esp drawing loss / topology, a via passed on the wrong side: IO7-IO10_SDA, IO9_BOOT-+3V3 via.
  - These are the router's open quality items. No single mechanism fixes all of them.
- Engine "short" and KiCad "shorting" differ once. The engine counts ADV_HPD_C against HDMI5V as a short
  (depth 0.40 mm), but the written file has no such contact. The nearest HDMI5V F.Cu segment, (139.368, 57.399)
  to (140.967, 55.800), runs parallel 0.43 mm clear of HPD_C. So the engine's violation list is stale against a
  later pass that moved HDMI5V; this is a report bug, not copper. A plain mid-segment
  crossing is joined by KiCad and removed (shorting fixture, CROSS case).

## Options (a user decision: it touches the never-drop / costly-not-failed rule)
- **A. Keep (main).** Cleanup deletes forced and victim copper. The user can nudge a short by hand before Cleanup.
- **B. Never write a short (output step only; the search stays costly-not-failed).** After the final drawing, a
  connection whose copper shorts another net (engine checker kind `short`) is not written. It keeps its
  ratsnest and gets a critical warning with the place and cause. Clearance-only violations are still written,
  and Cleanup keeps those.
  - Option `[output] write_shorts = true|false`. The default is the user's call.
  - Simulated on the main boards: Cleanup qm 179 -> 0 and esp 59 -> 0. Not written: qm 110 and esp 39 items
    (6 / 2 nets). Victims kept: qm 69, esp 20.
  - After Cleanup each net is better off or equal. Before Cleanup, unconnected grows by those connections and
    the DRC shorts and clearance errors there go.
  - It breaks the brief's "unconnected not worse" by design.
- **C. Touch-free forced geometry.** Price gap <= 0 far above gap < rule, so the forced drawing narrows or shifts
  to a gap above 0. That keeps never-drop and is Cleanup-safe where it works.
  - Draw-absorb A1 (forced_shrink) only got KiCad shorts 21 -> 18. A general version is a drawing project, not
    a fix.
- **D. Remove the shorts at their causes** (the 4 sub-classes above). These are existing items: ADV7511-DDC-SHORT,
  via-plan, forced-draw, pinrow.

## Tests (whichever is chosen)
- kicad_accepts gets a `shorting` project, run in the real dialog: forced wire X crosses victim Y, where both
  have pads. It pins that KiCad removes both, and B's output where Cleanup removes nothing.
- Engine fixture (harness/fixtures.py): two crossing two-pin nets with no legal geometry. With write_shorts=false
  X is unrouted, Y is written, and the replay finds 0. With true, the copper is main's.
- Mutants on the drop gate and the victim keep.
