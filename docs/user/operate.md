# Operating topor 0.4.0 in KiCad

topor routes the connections of a KiCad 10 board as smooth curved traces (straight segments joined by
tangent arcs). It never changes your board until you press **Accept** in its preview, and an accepted
result is **one undo step** (Edit → Undo). Install it first: [install.md](install.md).

Contents: [Quick start](#quick-start) · [Where the settings come from](#where-the-settings-come-from) ·
[First run](#first-run) · [Pre-flight findings](#pre-flight-findings) · [Options](#options) ·
[Scope by selection](#scope-by-selection) · [Progress and Cancel](#progress-and-cancel) ·
[Findings after routing](#findings-after-routing) ·
[Preview](#preview) · [Accept, Reject and undo](#accept-reject-and-undo) · [Report folder](#report-folder) ·
[Net priorities](#net-priorities) · [Keyboard](#keyboard) · [Limitations](#limitations) ·
[License and warranty](#license-and-warranty)

## Quick start

1. Save your work and try topor on a **copy** of the project first.
2. Set the design rules in KiCad (clearances, track widths, via sizes, net classes — see the table below).
3. Click **Topor: route** in the PCB Editor toolbar (or **Tools → External Plugins → Topor: route**).
4. Read the findings, set the [Options](#options) (on a board that is already routed choose
   *Existing copper: Rip up and re-route*, otherwise there is nothing to route), press **Route**.
5. Watch the progress; press **Cancel** at any time to keep the best result so far (during the initial routing there is none yet: [`NOTHING_TO_DRAW`](troubleshoot.md#nothing_to_draw)).
6. Compare *Current board* and *Result* in the preview, then **Accept** (applied as one undo step) or
   **Reject** (nothing changes).
7. Run KiCad's own DRC (**Inspect → Design Rules Checker**) — KiCad is the judge, not topor.

## Where the settings come from

Three sources, in this order (46 §1): **KiCad's board setup** for everything KiCad already knows
(rules, widths, via sizes, net classes, layer types, locks) → **`topor.toml`** next to the `.kicad_pro`
for topor's own choices → topor's built-in defaults. Every finding names the place to change. One thing is
not yours to set in 0.4.0: KiCad's design-rule minimums below topor's fab profile are raised to it, and the
router may add smaller fab-profile vias ([Fab minimums and via sizes](#fab-minimums-and-via-sizes)).

### Settings topor reads from KiCad

| What | Where in KiCad |
|---|---|
| Clearance; minimum via, hole and annular ring | KiCad → File → Board Setup → Design Rules → Constraints (values below the [fab floors](#fab-minimums-and-via-sizes) are raised unless `[rules] fab_profile = "board"`) |
| Copper-to-edge clearance | KiCad → File → Board Setup → Design Rules → Constraints → Copper to Edge Clearance |
| Track widths | KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Track Widths and netclass track width |
| Via sizes | KiCad → File → Board Setup → Design Rules → Net Classes (via size / hole): each net routes with its net class via, plus smaller [fab-profile vias](#fab-minimums-and-via-sizes) where space requires. Pre-defined Sizes → Vias are **not** routed with in 0.4.0; they only stop [`VIA_NOT_SET_UP`](troubleshoot.md#via_not_set_up), which uses `[vias] fallback_diameter_mm` / `fallback_drill_mm` when nothing is set up |
| Net classes and net assignments; power net classes; `topor_p1`..`topor_p3` priority classes | KiCad → File → Board Setup → Design Rules → Net Classes |
| Custom rules (`.kicad_dru`): clearance, edge / hole clearance, track width, via size; `disallow track/via` areas become keepouts, track-angle rules are refused ([Custom design rules](#custom-design-rules)) | KiCad → File → Board Setup → Design Rules → Custom Rules |
| Layer types (signal / power / mixed / jumper) → plane or routed layer | KiCad → File → Board Setup → Board Editor Layers |
| Via protection (filled + capped), needed for vias in pads | KiCad → File → Board Setup → Board Stackup → Board Finish → Via protection |
| Locked tracks and vias | KiCad → item Properties → Locked |
| What to route (scope) | KiCad → select footprints in the PCB editor before you click the button ([Scope by selection](#scope-by-selection)) |
| Priority marker `@1`..`@3` in a net name | the net label in the schematic (Eeschema), then Tools → Update PCB from Schematic |
| Zones and their fill | Place → Add Filled Zone; Edit → Fill All Zones (B) |

Everything is read from a copy of the board as it is in the editor **right now**, including unsaved
edits (topor saves that copy itself; your file on disk is not touched).

### Fab minimums and via sizes

topor's rule resolver (`toporlib/rules.py`) treats a built-in fab profile, **JLCPCB 1 oz** (`JLCPCB_1OZ`), as
**floors**: every KiCad design-rule minimum below the profile's value is raised to it; values above it are used
as they are. This is the default, `[rules] fab_profile = "jlcpcb_1oz"`. **`[rules] fab_profile = "board"`**
turns the profile off: Board Setup, the net classes and the custom rules are then the only authority, exactly as
KiCad's DRC judges them (a minimum of 0 means no minimum, a custom rule's clearance or width is used as given,
also below the Board Setup minimum and inside a courtyard / rule area, and only the net class vias and KiCad's
pre-defined vias are used, not the profile's drill list below). Use it when your fab makes finer copper than the
profile and your Board Setup says so (e.g. a BGA neck-down rule with 0.1 mm clearance). KiCad's DRC judges the
result with your own rules either way, so with the default the floors only make topor more conservative than your
board needs.

| KiCad rule (`.kicad_pro` `board.design_settings.rules`) | topor floor |
|---|---|
| Minimum clearance (`min_clearance`) | 0.15 mm |
| Minimum track width (`min_track_width`) | 0.2 mm |
| Minimum via diameter (`min_via_diameter`) | 0.45 mm |
| Minimum through hole (`min_through_hole_diameter`) → minimum via drill | 0.25 mm |
| Minimum annular width (`min_via_annular_width`) | 0.1 mm (2 copper layers), 0.075 mm (4 or more) |
| Copper to hole clearance (`min_hole_clearance`) | 0.35 mm |
| Hole to hole clearance (`min_hole_to_hole`) | 0.25 mm |
| Copper to edge clearance (`min_copper_edge_clearance`) | 0.2 mm |

- A rule left at 0 (unset) silently takes the floor; a rule set below the floor is raised and listed. The
  raised ones are in the run report: **report.md → Settings → Fab floors raised** (`name design → floor mm`).
  A custom rule's value below the floor (a net's width or clearance, the clearance inside a courtyard / rule
  area) is raised too and named in [`CUSTOM_RULE_LIMITED`](troubleshoot.md#custom_rule_limited).
- Per net, the clearance and the track width are at least these minimums (a net class clearance or width below
  them is raised too), and the net's via is its net class *Via Size* / *Via Hole* (a class without them: the
  Default class, else KiCad's built-in 0.6 / 0.3 mm), raised to the minimum via diameter / drill.
- **Via sizes the router uses** (`engine/src/vias.rs`, `via_sizes`): for each net first its own via, then,
  where space requires, smaller vias from the profile's drill list **0.25, 0.3, 0.4, 0.5 mm**: every such drill
  that is at least the (floored) minimum via drill and smaller than the net's drill, with diameter = drill +
  2 × minimum annular width (at least the minimum via diameter), kept only when that diameter is smaller than
  the net's via. Example: wiccon-2023 (factory 0.8 / 0.4 mm net class, minimum via drill 0.3 mm, annular
  0.1 mm) may get 0.8 / 0.4 and 0.5 / 0.3 mm vias.
- KiCad's **pre-defined vias** (KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Vias,
  `via_dimensions` in the `.kicad_pro`) **are used** for routing: with `[vias] size = "net_class"` the net's
  class via comes first, then every pre-defined via (and every profile via above) that is smaller than it and
  meets the minimums, largest first (`engine/src/vias.rs` `sizes_for`). They also decide whether
  [`VIA_NOT_SET_UP`](troubleshoot.md#via_not_set_up) fires: no pre-defined via and only KiCad's factory
  0.8 / 0.4 mm net class via → the nets with that via get `[vias] fallback_diameter_mm` /
  `fallback_drill_mm` (0.8 / 0.4 mm by default) as their own via, plus the smaller profile vias above.

### topor.toml

`topor.toml` holds topor's own choices, one section per topic; every key has a comment and a safe default.
The full reference, generated from the code, is **[configuration.md](configuration.md)** — every key with
type, default, unit, range and the KiCad place that influences it.

| Section | What it controls |
|---|---|
| `[run]` | time limit, threads, partial routes, existing-copper policy, locked items, selection mode, unrouted-only, pre-existing DRC, KiCad cleanup / teardrops after Accept |
| `[nets]` | do-not-touch nets, power / ground name patterns, priorities, pads to ignore |
| `[rules]` | design-rule authority: KiCad's rules with the JLCPCB 1 oz minimums as floors (default) or exactly the board's own rules (`fab_profile = "board"`) |
| `[vias]` | via cost and count, via reduction, fallback via size |
| `[layers]` | the role of each copper layer (`F.Cu = "signal"`, `In1.Cu = "gnd"`, …) |
| `[planes]` | whether signal copper may run on plane layers (gnd / pwr roles) and at what price |
| `[search]` | detour factor, safety margin, zero-slack retry |
| `[ripup]` | rip-up escalation |
| `[iterations]` | when the optimiser stops (cap, patience, minimum gain) |
| `[optimize]` | topological core: how a run iterates (`loop`: TopoR's loop on the topology by default, `"cycles"` = the whole-route cycles before M0; `initial`; fluctuation schedule, *Routes stable*, best of N `runs`), the safety and work caps that make a run reproducible (`rounds`, `max_ripped`, `max_expansions`, `max_moves`), the P4 moves, the board objective (`objective` = `length_vias` or `area`, `mm_per_via`) and how many result variants are kept (`variants`) |
| `[capacity]` | topological core: the per-layer cut test (`mixed_clearance`, `cut_radius_mm`) and its debug report (`cut_report`: a folder that gets the cut test of every routing cycle, for `harness/cutclass.py`); by default (`model = "edges"`) the per-edge room is the capacity price, its over-full edge-layers priced (`edges_test = "price"`) or refused as before M0 (`"refuse"`); `model = "cuts"` makes the cut test the price of the layer assignment and of the rip-up rounds (with `overflow_softness`, `cut_vias`), for now an option: too slow for a large board within the time limit |
| `[factors]` | topological core: weight, scope and report level of each social factor (via, crossing, squeeze, blocks_pad, reroute, …; see the factor table in configuration.md) |
| `[output]` | arcs or straight segments; report folder |
| `[expert]` | router constants — leave them alone unless you know the router |

Keys marked *since 0.5.0* in the reference are written commented-out: they are accepted, but the engine
feature behind them arrives in 0.5.0. Unknown keys give [`CFG_UNKNOWN_KEY`](troubleshoot.md#cfg_unknown_key);
a wrong value gives [`CFG_VALUE`](troubleshoot.md#cfg_value) and the default is used. The file is read on
every run; edit it with any text editor (comments are kept when topor writes to it).

### Custom design rules

KiCad 10 keeps custom rules in `<board>.kicad_dru` (KiCad → File → Board Setup → Design Rules → Custom
Rules). Neither the IPC API of KiCad 10 nor its Python module can evaluate them for the plugin, so topor
reads the file itself (`toporlib/druexpr.py`, `toporlib/custom_rules.py`) and follows KiCad's own resolution:
rules are tried in file order, a condition is tried for (A, B) and (B, A), and the **last matching rule wins**,
also when its value is smaller than the net class's. The test suite checks this against KiCad's DRC engine
(`tests/test_custom_rules.py`).

Honoured constraints and how the router uses them:

| Constraint | Used as |
|---|---|
| `clearance` | per-net clearance (net / net class / layer conditions); a **clearance region** for `A.intersectsCourtyard('REF')`, `A.insideCourtyard('REF')`, `A.intersectsArea('name')`, `A.insideArea('name')`, `A.enclosedByArea('name')` (inside, the rule's value replaces both nets' clearances); a **pad clearance** for pad conditions (`A.Pad_Type == 'SMD'`, `A.memberOfFootprint('REF')`, `A.Type == 'Pad'`, `A.isPlated()`) |
| `edge_clearance`, `hole_clearance` | board-wide copper-to-edge / hole clearance (the largest value any routed net gets); a track into a pad that itself lies within the edge clearance ends inside that pad off its centre, where the track keeps the clearance (the pad's own violation is pre-existing) |
| `track_width` | per-net width: `opt` is the target, `min` / `max` bound it (`min` is also the floor a necked pad exit may not go below) |
| `via_diameter`, `hole_size` | per-net via size (`opt`, raised to `min`) |
| `disallow track` / `via` with `A.insideArea('name')` | keepout ([Fab minimums and via sizes](#fab-minimums-and-via-sizes) floors do not apply) |
| `diff_pair_gap`, `diff_pair_uncoupled`, `skew` | [Differential pairs](#differential-pairs) |

Conditions topor evaluates: `A.NetClass` / `A.NetName` (`==`, `!=`, wildcards, case-insensitive, composite
classes), `A.hasNetclass()`, `A.Type`, `A.Pad_Type`, `A.Layer`, `A.memberOfFootprint()`, `A.inDiffPair()`,
`A.isPlated()`, the courtyard and area tests above, `B.` versions of all of them, `&&`, `||`, `!` and `(layer
outer | inner | <name>)`.

Where topor's model is coarser than the rule, it errs on the safe side and says so in the pre-flight finding
[`CUSTOM_RULE_LIMITED`](troubleshoot.md#custom_rule_limited), naming the rule:

- a condition that depends on the other item (`A.NetClass == 'x' && B.NetClass == 'y'`) or that topor cannot
  read (`A.getField()`, `A.Position_X`, `A.fromTo()`, ...): a **stricter** value is applied to every net it may
  concern, a **looser** one is not used;
- one clearance and one track width per net: a value that differs per layer (`(layer inner)`) or between
  tracks and vias is applied as the largest clearance; the width is the one most layers share, clamped into
  every layer's min / max (a board with layer-specific impedance widths gets a warning per rule);
- a pad clearance can only add clearance: a looser rule for some pads is not used;
- a courtyard / area rule whose value inside depends on the net is not a region (handled as above);
- `length`, `via_count`, `track_angle`, `physical_clearance` and other constraints are not honoured (named
  warning; KiCad's DRC still checks them).

Rules with `(severity ignore)` are skipped: KiCad does not check them, so topor keeps the net class values.
Rules for silkscreen, courtyards, mask, paste and zone fills (`B.Type == 'Zone'`) do not change the routing.
The [fab floors](#fab-minimums-and-via-sizes) apply to custom values too: a 0.1 mm rule below the JLCPCB
0.15 mm floor is raised to it (the finding says so). [`CUSTOM_RULES`](troubleshoot.md#custom_rules) lists
which rule is used how.

## First run

When the project has no `topor.toml`, topor creates one with every key at its default and shows
**Topor — Settings file created** with the path. **Open file** opens it in your editor (the dialog stays
open), **Continue** goes on. If the folder is not writable you get
[`CFG_WRITE`](troubleshoot.md#cfg_write) and the built-in defaults are used.

## Pre-flight findings

Before anything is routed, topor saves a copy of the live board, reads it, lets the engine build its map of
the board (a few seconds, no routing), runs KiCad's DRC on the copy (progress: *Checking the board (KiCad
DRC)*) and opens **Topor — Route &lt;board&gt;**:

- If the engine cannot read the board, topor stops right there, before the DRC and this dialog, with the
  [`ENGINE_BOARD_UNREADABLE`](troubleshoot.md#engine_board_unreadable) error: it names the copper where the
  engine failed (footprint, layer, net, coordinates) and what to do. The board is not changed.

- The first line counts the connections in scope, the nets, the copper layers and the engine version.
- **Findings** lists everything topor is about to assume about your board, sorted Error → Warn → Note; each row
  carries its severity's icon, word and colour (error red, warning amber, note grey), and the list is as tall as its
  rows. Without errors and warnings the heading reads *No problems found*. Select a row: **Cause** says what topor found, **Fix** says where to change it (a KiCad menu path or a
  `topor.toml` key). Every code has a section in [troubleshoot.md](troubleshoot.md).
- **Error** findings block the run: **Route** is disabled and the line next to the buttons says
  *Cannot route: n error(s) — see Fix above.* Warnings and notes never block.
- Two findings can be accepted right here with the **Accept** checkbox under the Fix text:
  [`PAD_OUTSIDE_BOARD`](troubleshoot.md#pad_outside_board) (skip those pads, saved to
  `[nets] ignore_pads`) and [`DRILL_OUTSIDE_FILTER`](troubleshoot.md#drill_outside_filter) (add the drills
  to `[vias] allowed_drills_mm`). Accepted choices are written to `topor.toml` when you press **Route**.
- [`PREEXISTING_DRC`](troubleshoot.md#preexisting_drc) lists what KiCad's DRC already reports before
  routing; those are not blamed on topor. `[run] fail_on_preexisting = true` makes it an error.

**Route** (Enter, the bold button on the right) starts the run; **Cancel** (or Esc) closes the dialog; nothing is
written, not even accepted findings. **Help** (left) opens this guide.

## Options

The lower half of the same dialog. The values start from `topor.toml`; what you change applies to this run.
Tick **Save these options to topor.toml** to keep them (only these `[run]` keys are written).

| Control | Values | `topor.toml` key |
|---|---|---|
| Scope | Whole board · Between selected footprints · Touching selected footprints | `[run] scope_mode` (between / touching); see [Scope by selection](#scope-by-selection) |
| Existing copper | Keep as obstacles · Keep and smooth · Rip up and re-route | `[run] existing_copper` = `fixed` / `smooth` / `rip` |
| Respect locked tracks and vias | on: locked copper is never ripped | `[run] respect_locked` |
| Trace shape | *45° (octilinear)* (default: straight and 45° segments, 90° bends only where needed) or *Any angle (smooth)* | `[drawing] style` |
| Time limit | seconds of router time, default 600 (an emergency stop: the topological core ends at *Routes stable* or your OK); `0` = no limit; KiCad's DRC, the refill and the preview export are not counted | `[run] timeout_s` |
| Time limit clock | *wall* (default, seconds on the clock: what you wait) or *cpu* (CPU seconds of router work summed over all threads; comparable across machine load and thread count) | `[run] timeout_clock` |

*Existing copper* decides what happens to tracks and vias that are already on the board:
**Keep as obstacles** (default) routes only what is not connected yet and treats existing copper as fixed;
**Rip up and re-route** removes the tracks and vias of the nets in scope first — except locked ones (with
*Respect locked*), nets listed in `[nets] do_not_touch`, vias KiCad marks as free, and stitching vias that no
track of their net touches (`[run] keep_unconnected_vias`). `[run] rip_free_vias = true` removes those
free and stitching vias too, when they block routes (locked vias and `do_not_touch` nets stay). On a fully hand-routed
board *Keep as obstacles* finds nothing to route. **Keep and smooth** is accepted but in 0.4.0 behaves like
*Keep as obstacles* (see [Limitations](#limitations)).

Every other `topor.toml` key (`unrouted_only`, `[vias]`, `[iterations]`, …) is file-only: edit the file.
**Help** opens this guide (`operate.md` of the installed plugin) in the program your desktop uses for `.md`
files (a browser or a text editor), at the top of the file. **Route** (Enter) starts routing.

## Scope by selection

Select footprints (or pads) in the PCB Editor **before** clicking **Topor: route**; the two selection
radios are enabled only then (otherwise they are greyed with *(nothing selected in KiCad)*).

- **Between selected footprints** — only the connections whose pads are all on the selection: for each
  net with at least two selected pads, those pads are connected among themselves.
- **Touching selected footprints** — every connection that has at least one selected pad (it may run to an
  unselected part).
- **Whole board** — ignore the selection.
- Selected tracks or vias add their whole net; a selected rule area or graphic polygon (together with a
  footprint selection) adds the pads inside it.

The header count follows the radio you pick. Everything outside the scope stays as it is and is an obstacle.
A selection that yields nothing gives [`SCOPE_EMPTY`](troubleshoot.md#scope_empty). Grouped items: see
[`GROUPS_UNAVAILABLE`](troubleshoot.md#groups_unavailable).

## Progress and Cancel

**Topor — Routing &lt;board&gt;** shows what runs now: *Preparing geometry* (topor reads the board), *Checking the
board (KiCad DRC)*, then *Routing — &lt;stage&gt;* with the stage the topological core is in (*plane stubs*, *projection
of the board*, *topological search*, *layer assignment, rip-up and reroute*, *drawing the traces*, *via reduction and
fuse*, *pour stitching*), then *Verifying with KiCad DRC* and *Rendering the preview*. Below it: **Round** — the
whole-route round (cycle) now running; **Gain** — how much the best result improved over the last
`[optimize] stable_window` rounds; **Runs** — how many of the `[optimize] runs` seeded runs are stable; routed/total;
vias; **Router time** and, next to it, the **CPU time** the run has used so far (all threads summed), against the time limit (on the clock you chose); **Total time** since you pressed Route (KiCad's DRC and the preview
included); and the **score** of the **Last round** and of the **Best so far**.

The score rows show run, round, **Errors** (connections open or drawn with rule violations), **Open** (connections
without copper), **Vias** and **Length** — the order in which topor judges results (`[optimize] pick`, default
`errors_vias_length`: fewest errors, then fewest connections with a short, then fewest open, fewest vias, the shortest
length, then fewest shrinkages; `[optimize] pick_shorts`). A short (copper over another net's copper) is one of the
errors and, at the same error count, worse than an open connection: KiCad's DRC fails the board on it and Cleanup
removes the track. `[optimize] short_weight` (k, default 1) grades this: the ranked count is errors + (k - 1) x
shorts, the shorts a tie-break after it. The round lines in log.txt name the connections with a short. The same order decides when the
best result improved (the stable meter: reheats and the stable stop), so `pick_shorts` changes which rounds a run makes,
not only the final pick among equal error counts.
*Last round* is the round any run finished last; *Best so far* is the best result over all runs, exactly the board OK
or Cancel keeps; until the first round ends their place reads *Waiting for round 1…*. The line below says how the
last round compares, on the first criterion that differs: *Last round is worse than the best: errors +2.* (amber; more
errors decide before fewer vias), *Last round is the new best: length -0.030 m.* (green), or *… scores the same as the
best; the earlier result stays.* The numbers change in place: nothing in the dialog moves or widens while it runs.
Hover *Cancel keeps the best result so far.* for what Cancel and OK do exactly. When *Best so far* stays at an early round, the
later rounds were worse on that criterion — log.txt in the report folder has one `round ended:` line per round with
its score and the verdict, e.g. `round ended: run 3, round 2: errors 5, open 0, vias 168, length 7.412 m, shrinkages
4; rule violations 5, routed 426 of 426: worse than the best (run 1, round 1): errors +2`.

### Routes stable and OK

The topological core iterates like TopoR: there is no fixed number of rounds. Every round routes the whole board
again — with the passages the drawing found too small lowered, and with its factor weights shaken by a seeded
*fluctuation* that starts wide, narrows every round and widens again when the run is stuck (no better result for
`fluctuation_stuck` rounds while connections are still open) — and offers its result to the variant table. Several
independent runs (`[optimize] runs`, default 5, each with its own seed) go on at once on the CPU's threads; the best
result over all of them is the one you get.

When the best result has gained less than `[optimize] stable_gain_pct` (0.5 %) over the last `stable_window` (4)
rounds in every run, the dialog says **Routes stable** (also in its window title) and the **Cancel** button reads
**OK**. It is the same button with the same action: stop and keep the best result so far — exactly the board shown
under *Best so far*, which then goes to KiCad's DRC and the preview. By default the run also stops there by itself,
like a normal finish (each run stops at the round in which it becomes stable; `[optimize] finish_patience` > 0 lets
it look for small gains that many more rounds first). `finish_when_stable = false` keeps it going after *Routes
stable* for marginal gains until OK, Cancel, the safety cap `[optimize] rounds` or the time limit (the TopoR way). Pressing OK at *Routes stable* is not a cancel: the summary
says *Stopped: stable*. Headless runs (harness, MCP) stop at *Routes stable* by themselves (plus `stable_patience`
rounds). The first round of the first run always uses the plain weights, and every run is reproducible: the same
board and settings give the same result.

How long a run takes with the default best of 5 runs (measured headless with 4 threads and two boards at once; the
router time is most of the total): eprom-to-kernal 2 s, wiccon-2023 about 25 s, dcsgonebadge (231 connections)
about 450 s of router time (320–710 s over six seeds), qmtech (426 connections, 4 layers) about 1000 s (680–1300 s).
That is 5–8 times the fixed four rounds of earlier versions; in exchange dcsgonebadge connects 80.5 % instead of
77.6 % and qmtech 81.5 % instead of 79.6 % (KiCad DRC). `[optimize] runs = 1` takes roughly a quarter to a half of the
time for a smaller gain; *Routes stable* and OK let you stop as soon as the result is good enough.
`[run] timeout_s` (default 600 s of router time) is only an emergency stop: when it fires before the routes are
stable the result is the best so far, may differ between runs, and the findings say so
([`TIME_BUDGET_HIT`](troubleshoot.md#time_budget_hit)). Runs on large boards (dcsgonebadge, qmtech with the default
best of 5 runs) may hit it and keep the best result so far. `0` = no limit.

**The limit is soft** (`[run] timeout_mode = "soft"`, the default since 2026-11-13). When it is reached, the
progress dialog says *Time limit reached, waiting for the current step to finish (<step>); press Cancel to stop
now*. The step that runs finishes and its plan is kept as a variant: the initial routing up to its rip-up rounds,
the TopoR loop cycle that runs, or (with `[optimize] loop = "cycles"`) the whole-route cycle that runs. No new step
begins, and the best kept variant is drawn to its end, so a limit never leaves half-drawn copper. **Cancel** stops
at once and draws the best variant kept so far; when none is kept yet (Cancel during the initial routing of every
run) the run ends with the error [`NOTHING_TO_DRAW`](troubleshoot.md#nothing_to_draw) and writes no copper. How far
the run went past the limit, and in which step it was reached, is in log.txt, report.md (*Time limit* line) and
`result.json` (`time_limit.reached`, `step`, `overrun_s` on the run's clock; measurements outside the limit, such as
`[capacity] compass_cut_check`, are left out as they are left out of the limit; the engine's own report line gives
the overrun on the wall clock). With several runs each run finishes its own step; the steps named are those of the
runs still computing when the limit was reached (a run waiting for the others' picks computes nothing). On a large board a
TopoR loop cycle plus the drawing can take minutes (qmtech, 2026-11-13: about 4 minutes on the clock past the limit);
press Cancel to stop at once.

**The overrun has a cap** (`[run] timeout_overrun_s`): seconds past `timeout_s`, on the same clock, that the step
running at the limit may still take. Past it the run acts as with `hard` (below): the step that runs is cut and the
kept plan (the best variant so far, or the initial plan) is drawn in full; when the drawing itself is the step that
runs when the cap fires, the drawing is cut there and the connections it had not drawn stay unrouted. The warning
[`TIME_OVERRUN_CAP`](troubleshoot.md#time_overrun_cap) says which in log.txt, the findings, report.md (*Time limit*
line) and `result.json` (`time_limit.cap_s`, `time_limit.cap_hit`, and `time_limit.cut`: `plan`, `drawing`
or `none` when the cap passed only after the steps it stops had ended; `harness/score_runs.py` prints a WARNING
line and lists the run in `limits.json` `overrun_cap_hit`). The default `auto` is equal to `timeout_s` in headless
runs (harness, MCP, scripts, and direct calls of the engine's `route` / `route_async`, whose own default
`timeout_overrun_s` is `"auto"`), so such a run takes at most twice its limit plus the drawing of one plan, and no
cap in the KiCad dialog, where Cancel stops a run at once. A number sets the cap; `0` or `"none"` = no cap. The cap
never changes `timeout_s` itself, and has no effect with `timeout_mode = "hard"` or without a limit.

`hard` cuts the step that runs at the limit and draws the kept plan (the best variant so far, or the initial plan)
in full, the same path as past the overrun cap (TopoR judges its variants before any copper and draws the chosen
one, 6.3 manual pp. 72-73, p. 237); the progress dialog says *Time limit reached (hard): the current step stops now
(<step>)*. An initial routing the limit cut leaves the connections it had not reached unrouted, its rip-up rounds
stop and their plan so far is kept, and a TopoR loop cycle the limit cut is thrown away. A limit reached while the
drawing runs (its via reduction and pour stitching included) cuts the drawing: the connections it had not drawn
stay unrouted. A hard run so takes the limit plus the drawing of one plan. What 2026-11-13 changed holds in both
modes: Cancel during the initial routing (up to its rip-up rounds) keeps no plan
([`NOTHING_TO_DRAW`](troubleshoot.md#nothing_to_draw)), and only the picked variant is drawn (below).

**Only the picked variant is drawn** (`[drawing] candidates = 1`, the default since 2026-11-13, with `[optimize]
loop = "topor"`). Every run judges its plans on the topology, by their predicted violations, vias, length and
shrinkages; only the best pick over all runs is drawn (TopoR judges its variants before any copper, 6.3 manual
pp. 72-73, and computes the exact geometry once, Polubasov 2010). The other runs draw nothing; the report's
*best of N runs* line names whose pick was drawn. A higher `candidates` draws more of the runs' picks and keeps
the best drawn one (more drawing time); `[optimize] runs` or more is the behaviour before (every run draws its pick).

**Wall or CPU time.** `[run] timeout_clock` (also a choice next to the limit in the pre-flight dialog) says what
`timeout_s` counts. `wall` (the default) is seconds on the clock, what you wait for. `cpu` is CPU seconds of router
work summed over all threads of the run (`time.process_time` over the engine jobs): a 4-thread run uses it up about 4
times as fast, so the limit means the same amount of work on a busy machine as on an idle one and with any thread
count, and more threads do not buy a better result. Use `cpu` to compare settings or versions (the benchmark runner
`harness/score_runs.py` uses `cpu` with 2400 s, the old 600 s wall x 4 threads). One caveat: on a laptop with two
hardware threads per core and turbo clocks, a CPU-second under heavy load still does somewhat less work than on an
idle machine. When the limit fires, the best result so far is kept either way and the summary says which clock:
*Stopped: timeout_wall* or *Stopped: timeout_cpu*. The progress dialog, the report summary (*CPU ... s*), `result.json`
(`router_cpu_s`) and the finished dialog show the CPU time in both modes.

**Changing the time limit while the router runs.** A run that takes longer than you planned need not lose its
progress: the progress dialog has a **Time limit** row with the limit in force (seconds on the run's clock, *s wall
time* or *s CPU time*). **+5 min** and **+15 min** add to it; or type a new value and press **Set limit** (or Enter
in the field). You can raise it, lower it down to the router time already spent (a value below that becomes the time
spent: the run stops at its next check and keeps the best result so far, exactly as when the limit fires — not a
Cancel), or set `0` for no limit (then only OK / Cancel end the run, and the + buttons are off: there is nothing to add
to). The row works while the router runs; it is off after Cancel / OK, once the limit is reached, and during the
DRC and the preview (the limit counts router time only). The change applies to this run only: `topor.toml` keeps
`[run] timeout_s`. The log says *time limit extended by the user …* (or lowered / set / removed), and the report and
`result.json` (`time_limit`: clock, start, every change with the router time it was made at, final) record it; the
summary's *Stopped* line names the final limit (e.g. *limit 900 s wall (raised during the run from 600 s, 1 change)*),
and so does the time-budget warning. Headless runs (harness, MCP) have no such control, so their limit stays fixed.

**Cancel** (or Esc / Enter outside the time-limit field; **OK** after *Routes stable*) works at any time: the label changes to *Cancel pressed:
finishing the current stage…* (after *Routes stable*: *OK pressed: finishing the current stage…*); while routing, the best result found so far goes on to KiCad DRC and the preview; if nothing was routed yet
the run ends with *Cancelled — nothing was changed on the board.* topor polls the running `kicad-cli` every 0.25 s,
so Cancel also works during the pre-flight DRC (the run then ends with nothing changed) and the DRC rounds / preview
export (the DRC and the preview of the best result still finish). The engine finishes the stage it is in and then
returns its best result so far: normally within a second or two, on a large board sometimes longer. Meanwhile the
label reads *Cancel pressed: finishing the current stage (…)* with the seconds since Cancel; topor waits for the
engine and never drops the result it is finishing. A `kicad-cli` call that runs longer than `[run] kicad_cli_timeout_s` (default 120 s, `0` = no
limit) is stopped and the run ends with [KICAD_CLI_TIMEOUT](troubleshoot.md#kicad_cli_timeout) (board unchanged).
Before the routing starts the progress dialog already shows *Preparing geometry* while topor reads the board.

**One run per project.** Topor cannot be started twice for the same KiCad project: a second start shows
[RUN_ALREADY_RUNNING](troubleshoot.md#run_already_running) ("Topor is already running for this project (started
20:31, process 12345). Finish or cancel that run first.") and ends at pre-flight without touching the board; its
report folder says so. Other projects are not affected. The lock is an operating-system file lock on
`run-<hash>.lock` in `~/.cache/kicad/10.0/topor/<project>/` (not in your project folder); it is released when the
run ends (OK, Cancel, error) and by the system when topor crashes, so it never goes stale. `[run] single_instance =
false` turns it off.

After routing, the result is written into the board copy and checked with KiCad's DRC (`kicad-cli`), once: the
topological core takes no DRC feedback (a second route would give the same result), and the rule violations it
writes on purpose are its own report's (see *Never drop* below).

### Never drop: violations instead of open connections

Like TopoR, the topological core does not drop a connection whose drawing finds no legal geometry — a gap that is
too narrow when drawn, a via without a free site, a crossing that cannot be put on two layers, a branch that meets
its trace on another layer. The connection stays in the router and is drawn anyway on its best geometry: the
drawing tries its normal attempts, the exact path with the clearance relaxed by the least amount that gives one,
and the funnel through the passages, and keeps the one whose violations cost least. Every rule it breaks is priced
very high by the `violation` factor (1000 mm per violation plus 1000 mm per 0.01 mm² of depth × length, `[factors]
weight violation`), so the next rounds work to remove it; the best result counts errors first (errors =
connections with violations plus connections without copper; "a design-rule violation cannot be compensated by any reduction of
wire length", Polubasov 2010), then fewest connections with a short (`[optimize] pick_shorts`), then fewest vias, then the shortest length (`[optimize] pick`). A complete route with a violation can still be fixed by hand or
improved; a dropped one connects nothing.

The violating copper is written to the board. The findings list every violation
([`VIOLATIONS`](troubleshoot.md#violations)) with the connection, its type (`clearance`, `short` = copper overlap,
`hole_clearance`, `edge_clearance`, `connection_width`), the place, how deep and how long it is, what it is too
close to and the cause; KiCad's DRC shows the same places, and the progress dialog shows the violations of the best
result under *Best so far*. `[optimize] allow_violations = false` writes legal copper only: such connections are
reported unrouted and nothing of them is written (the behaviour before).

**The method by default (M0).** A run follows TopoR's method (Polubasov 2010): the initial routing once
(`[optimize] initial = "once"`), lengths priced as the shortest path that keeps the clearances (`[search] length
= "taut"`), the per-edge room as the capacity price (`[capacity] model = "edges"`, `edges_test = "price"`; the
per-layer cut test, `model = "cuts"`, is an option for now: too slow for a large board within the time limit), then TopoR's loop on the
topology only (`[optimize] loop = "topor"`), and the picked variant drawn once, without drawing feedback
(`[search] drawing_feedback = "auto"`). Every judgement is one price (the sum of weight × amount over the
factors): an over-full passage, a crossing on one layer or a trace through foreign copper is costly, never a
refusal. The whole-route cycles below are the method before M0, still available with `loop = "cycles"`
(`initial = "per_cycle"`, `length = "even"`, `[capacity] model = "edges"`, `edges_test = "refuse"` and
`drawing_feedback = "on"` give exactly the routing before M0).

**How the cycles remove violations (M-B2, `loop = "cycles"`).** Each cycle feeds what went wrong back to the next one *locally*:
the history (a price) goes only on the stretch of the trace where its copper broke a rule, weighted by the
distance to the place, and on the passages that were over-full; a passage the drawing found too narrow gets a
lowered room (a cut). Old findings fade every cycle (`[optimize] history_fade`, default 0.5), and when a cycle
comes out worse than the best so far the cuts turn into a price instead of a hard limit (`[optimize]
soft_cuts`). Before the drawing, the planner applies the crossing rules (`[optimize] crossing_rules`): a trace of
another net that wraps around a connection's terminal pad (so the connection has to cross it or go around it), two
traces crossing twice, and a crossing that cannot be put on two layers are re-planned, the two traces routed again
in the other order, and kept when the plan gets better. The `VIOLATIONS` finding groups what is left by origin;
violations *at a pad exit* (a neighbour pin of the connection's own part) are not crossings: the trace does not get
out of its pad at full width, which a later version plans (pad exits and necking in the planner).

**Widths, pad exits and via room in the plan (M-C).** The planner decides the width of every stretch while it still
untangles the traces in one plane: where a trace's net width does not fit a passage (a bottleneck, or the way out
of a fine-pitch pad), it plans the trace at the minimum width (KiCad → Board Setup → Design Rules → Constraints →
Minimum track width, or the net's custom-rule minimum) from the nearest pad of the connection up to that passage
(`[search] neck_from_pad`), and pays the *necking* factor once per narrowed stretch, more the deeper it narrows.
A trace leaving its own pad needs half its width plus the clearance to the next pin, so the planner no longer plans
exits through the gap between two fine-pitch pins. The drawing step draws the planned widths: the full width where
it fits, the minimum only inside the planned stretches; it no longer narrows a trace on its own
(`[search] neck_in_drawing = true` brings that back). Every narrowed stretch is a warning in the report with its
width and length. A via the plan puts at a layer change takes its spot: another net's via planned in the same
small spot is moved or its trace rerouted before the drawing (`[vias] reserve_sites`).

**Octilinear drawing (M-45).** By default the traces are drawn with straight and 45 degree segments, as most
boards are routed by hand (`[drawing] style = "octilinear"`; `"smooth"` keeps the shortest any-angle path). Every
trace is first found as the shortest path its topology allows, then redrawn with 0/45/90 degree segments inside its
own lane (the room the plan gave it in each passage, so it takes no room planned for a trace drawn later): few
bends (`[drawing] bend_mm` per 45 degrees of bend, a short piece between two bends pays more,
`bend_spacing_mm`), 90 degrees only where needed, an elongated pad left along its long axis until past its tip. A
trace that passes a pad or another trace runs parallel to it at the clearance, and it follows a trace drawn beside
it (`[drawing] bundle_pull`), so a bundle keeps its pitch through its bends; each such bend makes the outer trace
0.83 x pitch longer (the report's *bundles* line). The layer assignment and rip-up count the room an octilinear
trace takes (`[drawing] octilinear_room`). Where 45 degrees does not fit (a passage at another angle, just wide
enough), that stretch keeps its angle: it costs the *any_angle* factor and is a warning with its place and angle, so
you can review it (`[drawing] any_angle = "forbidden"` sends such a trace back to the router instead); a bend
sharper than 90 degrees costs *acute_bend* and is a warning too (`[drawing] acute`). The style never leaves a
connection open or adds a violation: a trace drawn by force (never drop) keeps its least violating shape, which is
usually not octilinear.

## Findings after routing

After the routing, the engine's own report is turned into findings with a code, the numbers, the places and a fix;
the preview summary names the warnings (*After routing, check: …*), report.md lists each with its places
(*Findings*), and every code has a section in [troubleshoot.md](troubleshoot.md):

| Code | What it means |
|---|---|
| [`DIFF_PAIRS`](troubleshoot.md#diff_pairs) | (info) differential pairs routed coupled: one line per pair connection with lengths, skew, coupled share, uncoupled ends and vias |
| [`DIFF_PAIRS_SKIPPED`](troubleshoot.md#diff_pairs_skipped) | differential pairs left unrouted because `[diffpair] enabled = false` (also announced in the pre-flight); see [Differential pairs](#differential-pairs) |
| [`UNROUTED`](troubleshoot.md#unrouted) | connections that could not be finished, counted by reason (no room for a via, no room at the clearance when drawn, …) |
| [`CONNECTION_WIDTH`](troubleshoot.md#connection_width) | traces not drawn because they would graze their own pad or via (KiCad's minimum connection width) |
| [`POUR_ISLANDS`](troubleshoot.md#pour_islands) / [`POUR_STITCHED`](troubleshoot.md#pour_stitched) | pour pieces the new copper cut off: left over / joined again by stitching vias or pad stubs |
| [`POUR_UNJOINED`](troubleshoot.md#pour_unjoined) | KiCad's own DRC after the zone refill: pour pieces cut off (counted in the preview legend) and pads not joined to their pour (blue ring in the preview) |
| [`PLANE_PAD_SIGNAL`](troubleshoot.md#plane_pad_signal) / [`PLANE_VIA_OUTSIDE_POUR`](troubleshoot.md#plane_via_outside_pour) | plane pads joined by a trace instead of a via into their pour / a plane via outside the fill |
| [`VIA_SQUEEZE`](troubleshoot.md#via_squeeze) | vias in a narrow pass (the via_squeeze social factor) |
| [`NECKED`](troubleshoot.md#necked) | traces narrower than their net width where they leave a pad |
| [`IGNORED_RULE`](troubleshoot.md#ignored_rule) | copper that breaks a KiCad check set to *ignore* (priced, not forbidden) |
| [`VIOLATIONS`](troubleshoot.md#violations) | never drop: copper written although it breaks a rule (no legal geometry was found); each violation with place, type and cause |
| [`TIME_BUDGET_HIT`](troubleshoot.md#time_budget_hit) | the time limit stopped the run before its work caps |
| [`DRC_NEW_VIOLATIONS`](troubleshoot.md#drc_new_violations) | KiCad's DRC of the routed copy finds new violations: counted by type, the first places with nets and position (mm); a warning only — Accept still works (costly, not failed), then fix them in KiCad → Inspect → Design Rules Checker |
| [`RULES_MISMATCH`](troubleshoot.md#rules_mismatch) | error, self-check: KiCad's DRC required a larger clearance between two nets than topor gave the router (topor read the design rules wrong); the run stops before the preview with an error dialog naming the nets and both values, nothing is applied — please report it |
| [`ENGINE_FAILED`](troubleshoot.md#engine_failed) | error: all or most connections failed with the same engine error (e.g. the board's projection could not be built); with nothing routed the run ends in an error dialog naming the cause, never as a normal run that routed nothing |

The pre-flight also warns about pads inside a track keepout on all their layers
([`KEEPOUT_PAD`](troubleshoot.md#keepout_pad)).

## Preview

**Topor — Preview: &lt;board&gt;** shows *Current board* (left) and *Result* (right) as KiCad renders them
(F.Cu, B.Cu, outline), each as its own image under its own heading, scaled to fill its pane; the two panes scroll
together. New copper is drawn with a highlight halo, and the
open connections (KiCad's DRC *unconnected items* after the zone refill, before and after routing) are drawn as
blue lines between the items that are still apart; the legend swatches sit top right. **Zoom**: Fit, 100 %, 200 %
(Ctrl + mouse wheel steps it). The **Summary** pane is the top of the run report, the facts to decide on first and
bold — routed/total, *DRC new / pre-existing / unconnected*, vias (vs. current board), length outside pads — and
the rest smaller below: partial/failed, detours, why the router stopped, scope and policy, settings and findings.
The *DRC new* number comes from KiCad's DRC of the routed copy — it is the number to decide on: when it is above 0
the Summary starts with a red line *⚠ DRC new N (types) — see findings*, and the findings list below the images
(the pre-flight's list, now with the findings after routing) has
[`DRC_NEW_VIOLATIONS`](troubleshoot.md#drc_new_violations) with the first places (nets, position in mm). New
violations never block **Accept** (costly, not failed): accept and fix them by hand in KiCad → Inspect → Design
Rules Checker, or Reject and route again. The report folder is the field under the findings. If the image could not
be rendered you get [`PREVIEW_FAILED`](troubleshoot.md#preview_failed); the numbers are still valid and Accept still
works.

If KiCad's DRC requires a larger clearance between two nets than topor gave the router, topor read the design rules
wrong: the run stops before the preview with the error [`RULES_MISMATCH`](troubleshoot.md#rules_mismatch), naming
the nets and both values, and nothing is applied. Please report it.

## Accept, Reject and undo

- **Accept** (Enter) applies the result as **one** KiCad commit: the ripped tracks/vias are removed and the
  new ones created. That is **one undo step**: one **Edit → Undo** (Ctrl+Z) reverts all of it. New tracks
  are put in a group `topor` (`topor-partial` for partial routes) when KiCad allows it.
- **Zones come back filled for the new copper** (`[run] commit_fills = true`, the default): the fill that KiCad's
  own filler made for the routed result (kicad-cli, on the copy the preview's DRC judged) is written into the zones
  in the **same** commit, so the one undo step reverts copper and fills together. With `commit_fills = false` the
  zones keep the fill they had before the run: press **B** (KiCad → Edit → Fill All Zones) yourself; KiCad's DRC
  also refills when you run it. That refill is its own undo step. With `refill_after_accept = true` topor also
  refills the zones live right after the commit — a **second** undo step: the first Ctrl+Z reverts the refill,
  the next one the routes.
- After Accept, `[run] kicad_cleanup = true` / `kicad_teardrops = true` open KiCad's own *Cleanup Tracks
  and Vias* / teardrop dialogs (you confirm them there); each is a **separate** undo step after the
  routes. They are skipped when the result has partial routes
  ([`CLEANUP_SKIPPED_PARTIAL`](troubleshoot.md#cleanup_skipped_partial)).
- If one of these later steps fails (refill, cleanup / teardrops, writing the report), the routes stay on
  the board and the run still counts as accepted, with the warning
  [`POST_COMMIT_STEP_FAILED`](troubleshoot.md#post_commit_step_failed); Ctrl+Z removes the routes.
- **Reject** (Esc) writes nothing; the board is exactly as before and Undo has nothing new.
- **topor's copper survives Cleanup Tracks & Vias.** KiCad → Edit → Cleanup Tracks & Vias, with every option
  on, removes and changes nothing of what topor wrote ([below](#topors-copper-survives-cleanup-tracks--vias)).
- **Topor — Finished** confirms what happened, says whether the zones were refilled (press B when not) and
  shows the report folder. With new DRC violations on the board it shows a warning icon and starts with
  *⚠ N new DRC violations (types) — run Inspect → Design Rules Checker*.
- Then run **Inspect → Design Rules Checker** in KiCad.

### topor's copper survives Cleanup Tracks & Vias

KiCad's **Edit → Cleanup Tracks & Vias** is the yardstick for the copper topor writes (`[output] cleanup_safe =
true`, the default): with every option on, it finds nothing to remove or merge in topor's traces and vias. What
topor does for that (each item is a rule of KiCad's cleaner, `pcbnew/tracks_cleaner.cpp` in KiCad 10):

| Cleanup option | What KiCad removes or changes | What topor writes instead |
|---|---|---|
| Merge co-linear tracks | null segments, duplicate segments, two same-width segments on one line whose joint touches nothing else | no null or duplicate segments; such segments are already one |
| Delete tracks fully inside pads | a segment with both ends inside one pad and nothing outside it | no such segment: the trace starts where it leaves the pad's copper, inside that pad |
| Delete tracks unconnected at one end | a track end that is not on connected copper; a short segment whose both ends are reached by the same neighbour counts as connected at one end only | every junction is an exact shared vertex: a branch (T) that joins a trace in its middle splits that trace there |
| Delete vias connected on only one layer | a via whose other layers reach nothing, e.g. a plane via in a zone whose fill is out of date | the zones come back filled for the new copper (`[run] commit_fills`, above) |
| Delete tracks connecting different nets | a track that touches copper of another net | nothing to add here: this is a DRC short (the report lists it as a violation; [Never drop](#never-drop-violations-instead-of-open-connections)) |
| Delete redundant vias | two vias at one place, a via on a through-hole pad | not produced |

The run report has one line `cleanup-safe output (...)` with what was dropped, split or merged. A trace topor
writes **with a rule violation** (never drop) is the one exception: when it shorts another net, the cleanup's
"Delete tracks connecting different nets" removes it (and "Delete tracks fully inside pads" a piece lying inside the
other net's pad), and the rest of that trace then counts as unconnected at one end and goes too. Fix such violations
first (Inspect → Design Rules Checker), then run the cleanup. With "Refill zones before and after cleanup" checked,
KiCad may still ask "Zone fills are out-of-date. Refill?" on large boards (qmtech: two of five zones differed by a
few of 39 000 outline points from a live refill); refilling changes nothing the cleanup does.

## Report folder

Every run writes a folder (also when you reject or cancel), shown in the preview, the finished notice and
the error dialog, with **Open report folder**:

| File | Content |
|---|---|
| `report.md` | the summary block, connections, vias, lengths, DRC, findings, settings, files, versions, the compass (below) |
| `preview.png` | the before/after image of the preview (composite, with caption and legend) |
| `preview-before.png`, `preview-after.png`, `preview-legend.json` | the two boards as separate images and the legend entries, which the preview dialog shows |
| `result.json` | machine-readable result: status, routed/total, stop reason, DRC classification, findings |
| `drc.json` | KiCad's DRC report of the routed copy |
| `log.txt` | every step with timings; the first thing to attach to a bug report |

### The compass: what the plan predicted, what was drawn, what KiCad finds

`report.md → Compass` (and `result.json → compass`, the summary line *Compass (predicted/drawn/KiCad)*) is one table
per run. Topor first plans every connection topologically (which side of every pad, which layer, where the vias go),
judges that plan by what it predicts (like TopoR's variant table, TopoR 6.3 manual pp. 72-73) and then draws the copper
once from it; KiCad's DRC judges the written board (p. 183). The table puts the three side by side per kind of problem:

| Row | Meaning |
|---|---|
| planner: over-full | a passage the plan knows is too narrow for the traces it sends through (over-full cut / edge); "[n bottlenecks]" = the cuts of one channel (its two ends) counted once |
| planner: over-full nearby | a violation next to another trace's over-full passage that this trace does not cross (a plan error, not a drawing fault) |
| planner: shrinkage | a passage that fits only with traces necked below their net-class width (not a violation) |
| planner: at a planned neck | a violation where the plan necked the trace (the neck width or its drawing) |
| planner: via site | a via the plan needs where no via fits |
| planner: unlayerable crossing | two traces that must cross and cannot be put on different layers |
| planner: foreign copper | a trace the plan runs through copper of another net (a short if not moved) |
| planner: forced path | a connection the plan placed without any legal path |
| pad exit | a violation at the exit from the trace's own fine-pitch pad; "not predicted" = this version does not plan pad exits yet |
| model gap (per-edge fits, cut test over-full) | a passage the per-edge measure (the drawing's, and the plan's with `[capacity] model = "edges"`) finds fitting but the per-layer cut test finds too narrow; with model = "edges" measured by a read-only cut test after the drawing, outside the time limit (`[capacity] compass_cut_check`, off by default: "not measured (compass_cut_check off)"; `harness.score_runs` turns it on) |
| per-edge over-predicted (cut test fits) | the reverse: the per-edge measure finds a passage too narrow that the cut test finds fitting |
| drawing loss | the plan said it fits, the drawing still broke a rule there (a drawing fault) |
| open (no copper) | connections without copper; KiCad column = its new unconnected items |
| KiCad only | KiCad DRC items near no violation topor reported (topor's own check missed them) |

Columns: **predicted** (the plan's count; connections in brackets), **predicted, drawn clean** (predicted problems
with no drawn violation within reach), **drawn** (rule violations of the written copper, connections in brackets),
**KiCad DRC** (KiCad's new routing violations, each counted in the row of the nearest topor violation within
`[output] compass_join_mm` (0.5 mm) of a net and layer the KiCad item names, `compass_join_nets`). KiCad counts are not
1:1 with drawn counts: one drawn stretch below a rule can be several KiCad items, several drawn violations one.
Below the table: the connections with a detour above 1.5 x their reference length, drawn much longer than planned,
with a different via count than planned, or with violations. How to read it: a problem in a "planner" row is
predicted, so the fix is in the plan or your rules (more room, another layer, a via site); a "drawing loss" or
"KiCad only" is a topor bug worth reporting with the report folder.

Default location: `~/.cache/kicad/10.0/topor/<board file name>/<YYYYmmdd-HHMMSS>/`
(`$TOPOR_REPORT_ROOT`, else `$XDG_CACHE_HOME/kicad/10.0/topor`). `[output] report_dir` in `topor.toml`
redirects it: a relative path is inside the project folder (`<project>/<report_dir>/<timestamp>/`), an
absolute path is used as is. Fatal errors of the plugin process go to `~/.cache/kicad/10.0/topor/topor-crash.log`.

## Differential pairs

Topor routes differential pairs as coupled pairs (`[diffpair] enabled = true`, the default; see
[configuration.md](configuration.md)).

- **Which nets.** KiCad's own rule, on the full net name: trailing digits and `_` are skipped, then the name must
  end in `+`/`-` or upper-case `P`/`N`, and the net with that character swapped must exist (`CLK_N1` pairs with
  `CLK_P1`, `USB+` with `USB-`, `USB+` with `USB_N` does not). Extra pairs: `[diffpair] pairs`. The pairs, with
  their width and gap and where those came from, are listed in the run log.
- **Sizes.** Width = the net class's DP width (KiCad → File → Board Setup → Design Rules → Net Classes), gap = its
  DP gap or a custom rule `diff_pair_gap (opt ..)`, both raised to the board minimums (topor's, after the fab
  floors: see Fab minimums). Between the two tracks of a pair the gap is the clearance, as in KiCad. Via pairs:
  two vias side by side, edge gap = the pair gap raised to the hole-to-hole and copper-to-hole rules.
- **How.** A pair connection is planned as one wire as wide as both tracks and the gap: it is placed early (by its
  width), crosses other wires once and changes layer with one via pair, whose site is checked as the disc around
  both vias. It is drawn as two tracks at the gap about that wire's centre line, 45 degree corners in the
  octilinear style, and joined to the pads by short fan-out stubs: each stub leaves square to the line through
  its two pads (KiCad's pad "gateways") or turns into the pair at 45 degrees, and the two tracks meet at the
  nearest point where every clearance holds (`[diffpair] zip_max_mm`). Where nothing legal fits, the least
  violating fan-out is drawn and reported (never drop).
- **Polarity.** P and N never cross in a coupled pair. A connection whose pads swap sides between its ends (the A
  and B rows of a USB-C connector, or a part placed mirrored) is routed as two single tracks at the pair width,
  and the run log says so; such a pair needs a via on one track, as in KiCad.
- **Report.** One line per pair connection in the engine notes (report.md and result.json `diff_pairs`): length of
  P and N, skew (and the 45 degree bend estimate, 0.83 x pitch per bend; opposite bends cancel), the coupled share
  at the gap, the uncoupled length at each end, vias. KiCad's own violation texts ("Differential uncoupled length
  too long", "Skew between tracks out of range", "Differential pair gap out of range") appear only when a custom
  rule sets that limit; without rules KiCad's DRC checks nothing about pairs, and topor prices uncoupled length
  and skew with graded factors (`dp_uncoupled`, `dp_skew`, ..., [docs/dev/factors.md](../dev/factors.md)).
- **Where KiCad sets them:** KiCad → File → Board Setup → Design Rules → Custom Rules diff_pair_uncoupled; KiCad → File → Board Setup → Design Rules → Custom Rules skew; KiCad → File → Board Setup → Design Rules → Net Classes → DP Gap, Custom Rules diff_pair_gap; KiCad → File → Board Setup → Design Rules → Net Classes → DP Via Gap; KiCad → File → Board Setup → Design Rules → Net Classes → DP Width; KiCad → File → Board Setup → Net Classes; KiCad → net names ending in +/- or P/N.
- **Not yet.** Length and skew tuning (meanders) and pair-to-pair length matching; splitting a pair around an
  obstacle; a P/N swap with a via pair. `[diffpair] enabled = false` leaves pairs unrouted
  ([`DIFF_PAIRS_SKIPPED`](troubleshoot.md#diff_pairs_skipped)).

## Net priorities

A priority 1..3 (1 = routed first) comes from, strongest first:

1. an **`@N` marker** in the net name (`CLK@1`, `USB_D@1+` / `USB_D@1-`, `DQ@1_3`) — `@` followed by 1, 2 or
   3 and then a non-digit or the end; `DQ@13` is not a marker ([`MARKER_IGNORED`](troubleshoot.md#marker_ignored));
2. a **net class named `topor_p1`, `topor_p2` or `topor_p3`** (KiCad → File → Board Setup → Design Rules → Net Classes);
3. **`[nets] priority`** in `topor.toml` (`"CLK" = 1`, `"DQ*" = 2`; an exact name wins over a glob).

Nets in a power net class (or, while still in *Default*, matching `[nets] power_pattern`) get priority 2
when nothing else sets one ([`POWER_BY_NAME`](troubleshoot.md#power_by_name)). Both halves of a
differential pair get the higher priority ([`MARKER_PAIR`](troubleshoot.md#marker_pair)); sources that
disagree give [`MARKER_CONFLICT`](troubleshoot.md#marker_conflict). The engine routes priority 1 first and
gives it the cheapest search weight (and the highest via cost); nets without a priority are tier 3.

## Keyboard

Every dialog: **Enter** = the default button (Route / Accept / Continue / Close), **Esc** = Cancel / Reject /
Close, **Tab** follows the reading order. Mnemonics (Alt + letter): pre-flight **R**oute, **A**ccept,
**W**hole board, **B**etween, **T**ouching, respect **L**ocked, **K**eep partial, **S**ave, **H**elp,
**C**ancel; progress Set **l**imit, +**5** min, +**1**5 min, **C**ancel (Enter in the time-limit field = Set limit);
preview **A**ccept, **R**eject, **O**pen report folder; error Copy **d**etails, **O**pen report folder, **C**lose.

## Limitations

What 0.4.0 does not do, or does approximately. Approximations are always on the safe side: an obstacle is
never smaller than in KiCad, a connection is never assumed that KiCad would not make, a via topor is
unsure about is kept. KiCad's DRC of the result stays the judge.

### Topological core (the engine of this version)

- **Differential pairs** are routed coupled but not tuned: no meanders for length or skew, no pair-to-pair length
  matching, no P/N swap with vias (a connection whose pads swap sides is routed as two single tracks); the
  fan-out at pads can leave a skew of up to about a millimetre ([Differential pairs](#differential-pairs)).
- Two or more signal layers are needed; through vias only.
- Not every connection finishes legally on dense boards. Never drop draws them anyway (dcsgonebadge 222/231
  connections drawn, about 35 of them with violations; qmtech 400/426, about 50 with violations; measured headless,
  cut at 600 s): every violation is listed ([`VIOLATIONS`](troubleshoot.md#violations)) and needs a fix by hand,
  and there are many on these boards (hundreds, about half of them copper over a foreign pad or trace).
  `[optimize] allow_violations = false` leaves those connections open instead
  ([`UNROUTED`](troubleshoot.md#unrouted), drawn blue in the preview). Small changes elsewhere on the board can
  move the result by a few connections.
- Pour islands that the new copper cuts off are stitched afterwards (stitching vias, pad stubs), not prevented.
- The settings of the classic router (`[iterations]`, `[ripup]`, the via cost keys, `[run] threads`, …) were removed
  in the CL cleanup (2026-11-09; list in [configuration.md](configuration.md#removed-keys)): an old `topor.toml`
  that still sets one gets [`CFG_REMOVED_KEY`](troubleshoot.md#cfg_removed_key) and the key is ignored. The
  `[factors]` weights, the `[optimize]` stable test and work caps take their place.

### Platform and board

- **Linux only** (x86_64, Ubuntu 24.04 tested); KiCad 10.0 only (older board files are refused with
  [`PCB_FORMAT_OLD`](troubleshoot.md#pcb_format_old)).
- **Layers.** The engine routes every copper layer whose role is `signal` or `mixed` and uses through vias
  only (no blind, buried or micro vias). Plane layers (`gnd` / `pwr` roles, defaults by copper count in
  [configuration.md](configuration.md#layers)) are not routed; through-hole pads reach them through their
  zone, and an SMD pad of a plane net gets a through via onto the plane at the cheapest free via site near
  the pad, inside the pad when `[vias] via_in_pad` is on (`[vias] plane_vias`, since 0.4.2). A pad with no site that close fails with *no via site within … of the
  terminal (plane pour)*. In the topological core a pour is a terminal of its net like a pad (`[planes]
  terminals`, default on): pads near each other share one plane via, sited where their stubs are shortest
  together, a pad may end on an existing through via standing in the fill, and a pad too far for a stub is
  wired to the pour's nearest filled copper instead of to another pad of the net. Only two-layer boards are benchmarked (eprom-to-kernal 27/27,
  wiccon-2023 62/62); 4+ layer boards route their signal layers but inner-layer quality is not tuned yet. Copper counts
  other than 2, 4, 6, 8 need roles in `[layers]` ([`LAYER_ROLES_NEEDED`](troubleshoot.md#layer_roles_needed)).
- **Groups.** The KiCad API does not expose group membership: grouped items are treated one by one, and
  selecting a group may not select its footprints for the scope (UNVERIFIED) — select the footprints
  themselves ([`GROUPS_UNAVAILABLE`](troubleshoot.md#groups_unavailable)).
  Grouping the new tracks into `topor` / `topor-partial` is attempted (UNVERIFIED on a live KiCad); if it
  fails the tracks are created ungrouped and the report says so.
- **Curved traces only.** Custom rules restricting track angle or segment length are refused
  ([`TRACK_ANGLE_RULE`](troubleshoot.md#track_angle_rule)); `[output] geometry = "segments"` approximates
  the arcs by straight segments. A 45° mode is planned (backlog).

### Settings not applied by the engine yet (0.5.0)

- **Net priorities** (`@N`, `topor_pN`, `[nets] priority`), **`[nets] do_not_touch`**, **`[nets] ignore_pads`**
  and **`[nets] pour_served`** are applied by the engine (0.5.0): priorities set the routing order and weight;
  do-not-touch nets are never routed and their copper stays a fixed obstacle (status `excluded` in the net
  table); ignored pads are neither terminals nor obstacles and their connections are listed as `ignored: pad
  outside board` (they are not counted in the connection total); `pour_served = true` makes the net's
  connections only pour connections, `false` routes it pad to pad although it has a pour.
- **Existing copper `smooth`**: the engine treats it as `fixed` and says so in the report.
- **Detour ratio**: the report's detour ratio is the routed length over the connection's topological ghost path
  (`toporlib/detour.py`). For a net with three or more pads (a tree) the report adds a second figure next to it,
  "tree nets (tree / MST of pads)": the net's whole drawn copper over the shortest spanning tree of its pads (a
  Steiner tree can be up to 13 % shorter, so about 0.87 is the best possible). A connection of a tree may end at
  another pad or branch of its net than the one it is named after, so its own ratio can look large while the tree is
  fine; the per-connection list still shows it, so a branch that really wanders stays visible. The DRC-feasible reference search (`[run] detour_reference`) and the pad exit stubs
  (`[search] exit_stubs`, `pad_bridge_max_mm`) of the classic router were removed in the CL cleanup; necking at
  pads is `[search] neck_to_pad` / `neck_max_mm` of the topological core (never below KiCad's minimum track width,
  KiCad → File → Board Setup → Design Rules → Constraints → Minimum track width, or a custom-rule `track_width`
  minimum).
- Keys marked *since 0.5.0* in [configuration.md](configuration.md) are written commented-out.

### While topor runs

- Cancel reacts within about a second in every phase (see [Progress and Cancel](#progress-and-cancel)).
- Do not edit the board while topor runs; Accept commits the result computed from the board as it was when you
  pressed Route (precisely: when you clicked **Topor: route**, when topor saved its copy of the board).
  This is checked, always (no setting): topor takes a fingerprint of the board (counts, track / via ids and
  positions, footprint references, positions and orientations) at the start and again right before it
  commits. If they differ nothing is committed and the run ends with
  [BOARD_CHANGED_DURING_RUN](troubleshoot.md#board_changed_during_run); undo your edits or run Topor again.

### Reading the board over the KiCad API (IPC): approximations and their safe direction

Measured on six snapshot boards (offline tests against KiCad's own polygons); the few corners that are
*not* on the safe side are marked and none of them occurs on those boards.

- **Copper graphics** (lines, arcs, circles, rectangles incl. corner radius, polygons) are polygonised by
  topor outward: the obstacle contains KiCad's shape and is at most 0.5 µm larger (holes of a filled polygon
  are ignored, i.e. filled). **Copper beziers** are grown by the board's max error on their concave side;
  if KiCad's DRC flattens beziers more coarsely than that, the difference is not covered (*not safe*,
  UNVERIFIED).
- **Hatched graphic fills** (KiCad 10): the API reports only filled / unfilled, so a hatched shape may
  arrive as its outline only (*not safe*, UNVERIFIED).
- **Copper text** becomes its bounding box grown by half the text size on every side (KiCad's own box
  misses descenders and accents by up to a third of the size): larger than the glyphs, never smaller.
  Footprint **fields** (reference / value) placed on a copper layer are not read (*not safe*; move them to
  silkscreen).
- **Zone fills** are taken exactly as KiCad filled them (the copy is refilled first). A zone that has no
  fill uses its outline as an obstacle and connects nothing.
- **Pour reach** (which pads and vias a filled zone connects) is decided by probe points strictly inside each
  pad and by the via centre. Where unsure it says "not reached" and routes a trace that KiCad might not
  need; KiCad's DRC (unconnected items) is the judge. Custom, trapezoid and chamfered pads are taken as their
  convex hull, so on a strongly concave pad a probe could lie outside the copper (possible over-reach, never
  seen on the test boards).
- **Pour pieces cut off by the new copper** (topological core, `[planes] stitch_islands`): after routing,
  topor estimates the refill (the copy's fill minus the new copper of other nets grown by the clearance,
  minus features thinner than the zone's minimum width, islands dropped or kept as KiCad → zone properties →
  Remove islands says). A piece of a pour net that is no longer joined to the rest of its pour gets a
  stitching via inside it near its pad (or a short stub from the pad), into the pour on another layer; the
  report lists each piece and each stitch, and the pieces it could not join. KiCad's DRC is the judge.
- **Pads** are exact; custom, trapezoid, chamfered and offset pads become the convex hull of KiCad's
  polygon (larger, never smaller).
- **Free vias**: KiCad's `free` flag is read from the saved board copy; a via whose flag cannot be read is
  treated as free, i.e. kept as a fixed obstacle and never ripped (unless `[run] rip_free_vias = true`).
- **Stitching vias** that no track end of their net touches are kept even when ripping
  (`[run] keep_unconnected_vias`, default true); a via a track only passes over is also kept. KiCad's own
  flag is set per via in KiCad → via Properties → Free (exact dialog wording UNVERIFIED, see the GUI checklist).
- **Net codes**: the API has none, so nets are numbered by their sorted names (only inside topor; your
  board keeps KiCad's numbering).
- **Keepouts** (rule areas) are applied on copper layers only; their arc edges are flattened outward.
- **Board outline**: segments, arcs and rectangles are exact; circles, beziers and polygons are flattened
  with their points on the curve, so a circular cut-out inside the board is up to the max error smaller
  (*not safe* by at most a few µm, far below any edge clearance).
- **Pad rotation** is taken from the pad's padstack angle (absolute, as KiCad's API reports it) —
  UNVERIFIED on a live KiCad until the GUI checklist ran.
- **Via sizes**: when KiCad has no via sizes set up ([`VIA_NOT_SET_UP`](troubleshoot.md#via_not_set_up)),
  topor uses `[vias] fallback_diameter_mm` / `[vias] fallback_drill_mm` (0.8 / 0.4 mm) for every net whose
  via size is KiCad's factory default. Smaller KiCad pre-defined vias are used where the class via does not
  fit, and so are smaller fab-profile vias unless `[rules] fab_profile = "board"`
  ([Fab minimums and via sizes](#fab-minimums-and-via-sizes)).

**Not yet**: same-net T-junctions, via relaxation and per-layer costs
(0.5.0); free via placement and the 45° mode (backlog). Plugin calls that need a running KiCad are listed
as UNVERIFIED in [gui-test-checklist.md](gui-test-checklist.md) until that checklist has been run.

## License and warranty

topor is free software under the GNU General Public License, version 3 only
([LICENSE](../../LICENSE)). It is distributed in the hope that it will be useful, but **WITHOUT ANY
WARRANTY**; without even the implied warranty of **MERCHANTABILITY** or **FITNESS FOR A PARTICULAR PURPOSE**
(sections 15 and 16 of the license). The copper topor draws is not guaranteed to be correct, complete or
manufacturable: run KiCad's DRC and review the board yourself before you order it.
