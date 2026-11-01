# Operating topor 0.4.0 in KiCad

topor routes the connections of a KiCad 10 board as smooth curved traces (straight segments joined by
tangent arcs). It never changes your board until you press **Accept** in its preview, and an accepted
result is **one undo step** (Edit → Undo). Install it first: [install.md](install.md).

Contents: [Quick start](#quick-start) · [Where the settings come from](#where-the-settings-come-from) ·
[First run](#first-run) · [Pre-flight findings](#pre-flight-findings) · [Options](#options) ·
[Scope by selection](#scope-by-selection) · [Progress and Cancel](#progress-and-cancel) ·
[Preview](#preview) · [Accept, Reject and undo](#accept-reject-and-undo) · [Report folder](#report-folder) ·
[Net priorities](#net-priorities) · [Keyboard](#keyboard) · [Limitations](#limitations)

## Quick start

1. Save your work and try topor on a **copy** of the project first.
2. Set the design rules in KiCad (clearances, track widths, via sizes, net classes — see the table below).
3. Click **Topor: route** in the PCB Editor toolbar (or **Tools → External Plugins → Topor: route**).
4. Read the findings, set the [Options](#options) (on a board that is already routed choose
   *Existing copper: Rip up and re-route*, otherwise there is nothing to route), press **Route**.
5. Watch the progress; press **Cancel** at any time to keep the best result so far.
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
| Clearance; minimum via, hole and annular ring | KiCad → File → Board Setup → Design Rules → Constraints (values below the [fab floors](#fab-minimums-and-via-sizes) are raised) |
| Copper-to-edge clearance | KiCad → File → Board Setup → Design Rules → Constraints → Copper to Edge Clearance |
| Track widths | KiCad → File → Board Setup → Design Rules → Pre-defined Sizes → Track Widths and netclass track width |
| Via sizes | KiCad → File → Board Setup → Design Rules → Net Classes (via size / hole): each net routes with its net class via, plus smaller [fab-profile vias](#fab-minimums-and-via-sizes) where space requires. Pre-defined Sizes → Vias are **not** routed with in 0.4.0; they only stop [`VIA_NOT_SET_UP`](troubleshoot.md#via_not_set_up), which uses `[vias] fallback_diameter_mm` / `fallback_drill_mm` when nothing is set up |
| Net classes and net assignments; power net classes; `topor_p1`..`topor_p3` priority classes | KiCad → File → Board Setup → Design Rules → Net Classes |
| Custom rules (`.kicad_dru`; `disallow track/via` areas become keepouts, track-angle rules are refused) | KiCad → File → Board Setup → Design Rules → Custom Rules |
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
as they are. In 0.4.0 this is always on: there is **no `topor.toml` key to turn the floors off** (the switch
`apply_fab_floors` exists only in the developer harness). KiCad's DRC still judges the result with your own
rules, so the floors only make topor more conservative than your board needs.

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
  `via_dimensions` in the `.kicad_pro`) are **not read** for routing in 0.4.0; routing with them (and a fallback to the profile only
  when the list is empty) arrives in 0.5.0. They do decide whether
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
| `[vias]` | via cost and count, via reduction, fallback via size |
| `[layers]` | the role of each copper layer (`F.Cu = "signal"`, `In1.Cu = "gnd"`, …) |
| `[search]` | detour factor, safety margin, zero-slack retry |
| `[ripup]` | rip-up escalation |
| `[iterations]` | when the optimiser stops (cap, patience, minimum gain) |
| `[output]` | arcs or straight segments; report folder |
| `[expert]` | router constants — leave them alone unless you know the router |

Keys marked *since 0.5.0* in the reference are written commented-out: they are accepted, but the engine
feature behind them arrives in 0.5.0. Unknown keys give [`CFG_UNKNOWN_KEY`](troubleshoot.md#cfg_unknown_key);
a wrong value gives [`CFG_VALUE`](troubleshoot.md#cfg_value) and the default is used. The file is read on
every run; edit it with any text editor (comments are kept when topor writes to it).

## First run

When the project has no `topor.toml`, topor creates one with every key at its default and shows
**Topor — Settings file created** with the path. **Open file** opens it in your editor (the dialog stays
open), **Continue** goes on. If the folder is not writable you get
[`CFG_WRITE`](troubleshoot.md#cfg_write) and the built-in defaults are used.

## Pre-flight findings

Before anything is routed, topor saves a copy of the live board, reads it, runs KiCad's DRC on the copy
(progress: *Checking the board (KiCad DRC)*) and opens **Topor — Route &lt;board&gt;**:

- The first line counts the connections in scope, the nets, the copper layers and the engine version.
- **Findings** lists everything topor is about to assume about your board, sorted Error → Warn → Note.
  Select a row: **Cause** says what topor found, **Fix** says where to change it (a KiCad menu path or a
  `topor.toml` key). Every code has a section in [troubleshoot.md](troubleshoot.md).
- **Error** findings block the run: **Route** is disabled and the line next to the buttons says
  *Cannot route: n error(s) — see Fix above.* Warnings and notes never block.
- Two findings can be accepted right here with the **Accept** checkbox under the Fix text:
  [`PAD_OUTSIDE_BOARD`](troubleshoot.md#pad_outside_board) (skip those pads, saved to
  `[nets] ignore_pads`) and [`DRILL_OUTSIDE_FILTER`](troubleshoot.md#drill_outside_filter) (add the drills
  to `[vias] allowed_drills_mm`). Accepted choices are written to `topor.toml` when you press **Route**.
- [`PREEXISTING_DRC`](troubleshoot.md#preexisting_drc) lists what KiCad's DRC already reports before
  routing; those are not blamed on topor. `[run] fail_on_preexisting = true` makes it an error.

**Cancel** (or Esc) closes the dialog; nothing is written, not even accepted findings.

## Options

The lower half of the same dialog. The values start from `topor.toml`; what you change applies to this run.
Tick **Save these options to topor.toml** to keep them (only these `[run]` keys are written).

| Control | Values | `topor.toml` key |
|---|---|---|
| Scope | Whole board · Between selected footprints · Touching selected footprints | `[run] scope_mode` (between / touching); see [Scope by selection](#scope-by-selection) |
| Existing copper | Keep as obstacles · Keep and smooth · Rip up and re-route | `[run] existing_copper` = `fixed` / `smooth` / `rip` |
| Respect locked tracks and vias | on: locked copper is never ripped | `[run] respect_locked` |
| Keep partial routes | on: connections that cannot finish are drawn up to the blocking point (group `topor-partial`) | `[run] partial_routes` |
| Time limit | seconds of router time, `0` = no limit (Cancel is then the only stop); KiCad's DRC, the refill and the preview export are not counted | `[run] timeout_s` |
| Threads | `0` = all cores but one | `[run] threads` |

*Existing copper* decides what happens to tracks and vias that are already on the board:
**Keep as obstacles** (default) routes only what is not connected yet and treats existing copper as fixed;
**Rip up and re-route** removes the tracks and vias of the nets in scope first — except locked ones (with
*Respect locked*), nets listed in `[nets] do_not_touch`, vias KiCad marks as free, and stitching vias that no
track of their net touches (`[run] keep_unconnected_vias`). On a fully hand-routed
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

**Topor — Routing &lt;board&gt;** shows the phase (*Preparing geometry*, *Routing — negotiation pass*,
*Routing finished*, *Verifying with KiCad DRC*, *Rendering the preview*), routed/total, the optimisation
iteration out of `[iterations] max`, the via count and elapsed time against the time limit.
The run ends by itself when no more progress is made (`[iterations]`), when the router has used the time
limit (`[run] timeout_s` counts router time only: the pre-flight DRC, the DRC rounds and the kicad-cli
refill / preview exports come on top), or when you press **Cancel** (or Esc / Enter): the label changes to
*Cancelling — keeping the best result so far…* and the best result found so far goes on to KiCad DRC and
the preview. If nothing was routed yet, the run ends with *Cancelled — nothing was changed on the board.*
Cancel reacts within about a second in every phase: while routing, and also while `kicad-cli` runs (pre-flight
DRC with zone refill, the DRC rounds, the preview export) because topor polls the running `kicad-cli` every
0.25 s and keeps the dialog alive. Cancel during the pre-flight DRC stops `kicad-cli` and ends the run with
nothing changed; Cancel after the routing started keeps the best result (the DRC and the preview of that result
still finish). A `kicad-cli` call that runs longer than `[run] kicad_cli_timeout_s` (default 120 s, `0` = no
limit) is stopped and the run ends with [KICAD_CLI_TIMEOUT](troubleshoot.md#kicad_cli_timeout) (board unchanged).
Before the routing starts the progress dialog already shows *Preparing geometry* while topor reads the board.

After routing, the result is written into the board copy and checked with KiCad's DRC (`kicad-cli`). If
the router's copper causes violations, topor pins those spots and routes again, up to
`[expert] drc_loop_cap` rounds.

## Preview

**Topor — Preview: &lt;board&gt;** shows *Current board* (left) and *Result* (right) as KiCad renders them
(F.Cu, B.Cu, outline); new copper is drawn with a highlight halo. **Zoom**: Fit, 100 %, 200 %
(Ctrl + mouse wheel steps it). The **Summary** pane is the top of the run report: routed/total,
partial/failed, vias (vs. current board), length outside pads, *DRC new / pre-existing / unconnected*,
detours, why the router stopped, scope and policy, settings and findings. The *DRC new* number comes from
KiCad's DRC of the routed copy — it is the number to decide on. If the image could not be rendered you get
[`PREVIEW_FAILED`](troubleshoot.md#preview_failed); the numbers are still valid and Accept still works.

## Accept, Reject and undo

- **Accept** (Enter) applies the result as **one** KiCad commit: the ripped tracks/vias are removed and the
  new ones created. That is **one undo step**: one **Edit → Undo** (Ctrl+Z) reverts all of it. New tracks
  are put in a group `topor` (`topor-partial` for partial routes) when KiCad allows it.
- **Zones are not refilled** by default (`[run] refill_after_accept = false`): press **B**
  (KiCad → Edit → Fill All Zones) yourself; KiCad's DRC also refills when you run it. That refill is its own
  undo step. With `refill_after_accept = true` topor refills the zones right after the commit — a
  **second** undo step: the first Ctrl+Z reverts the refill, the next one the routes.
- After Accept, `[run] kicad_cleanup = true` / `kicad_teardrops = true` open KiCad's own *Cleanup Tracks
  and Vias* / teardrop dialogs (you confirm them there); each is a **separate** undo step after the
  routes. They are skipped when the result has partial routes
  ([`CLEANUP_SKIPPED_PARTIAL`](troubleshoot.md#cleanup_skipped_partial)).
- If one of these later steps fails (refill, cleanup / teardrops, writing the report), the routes stay on
  the board and the run still counts as accepted, with the warning
  [`POST_COMMIT_STEP_FAILED`](troubleshoot.md#post_commit_step_failed); Ctrl+Z removes the routes.
- **Reject** (Esc) writes nothing; the board is exactly as before and Undo has nothing new.
- **Topor — Finished** confirms what happened, says whether the zones were refilled (press B when not) and
  shows the report folder.
- Then run **Inspect → Design Rules Checker** in KiCad.

## Report folder

Every run writes a folder (also when you reject or cancel), shown in the preview, the finished notice and
the error dialog, with **Open report folder**:

| File | Content |
|---|---|
| `report.md` | the summary block, connections, vias, lengths, DRC, findings, settings, files, versions |
| `preview.png` | the before/after image of the preview |
| `result.json` | machine-readable result: status, routed/total, stop reason, DRC classification, findings |
| `drc.json` | KiCad's DRC report of the routed copy |
| `log.txt` | every step with timings; the first thing to attach to a bug report |

Default location: `~/.cache/kicad/10.0/topor/<board file name>/<YYYYmmdd-HHMMSS>/`
(`$TOPOR_REPORT_ROOT`, else `$XDG_CACHE_HOME/kicad/10.0/topor`). `[output] report_dir` in `topor.toml`
redirects it: a relative path is inside the project folder (`<project>/<report_dir>/<timestamp>/`), an
absolute path is used as is. Fatal errors of the plugin process go to `~/.cache/kicad/10.0/topor/topor-crash.log`.

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
**C**ancel; preview **A**ccept, **R**eject, **O**pen report folder; error Copy **d**etails, **O**pen report
folder, **C**lose.

## Limitations

What 0.4.0 does not do, or does approximately. Approximations are always on the safe side: an obstacle is
never smaller than in KiCad, a connection is never assumed that KiCad would not make, a via topor is
unsure about is kept. KiCad's DRC of the result stays the judge.

### Platform and board

- **Linux only** (x86_64, Ubuntu 24.04 tested); KiCad 10.0 only (older board files are refused with
  [`PCB_FORMAT_OLD`](troubleshoot.md#pcb_format_old)).
- **Layers.** The engine routes every copper layer whose role is `signal` or `mixed` and uses through vias
  only (no blind, buried or micro vias). Plane layers (`gnd` / `pwr` roles, defaults by copper count in
  [configuration.md](configuration.md#layers)) are not routed; pads reach them through their zone. In 0.4.0
  only two-layer boards are benchmarked (eprom-to-kernal 27/27, wiccon-2023 62/62); 4+ layer boards route
  their signal layers but fan-out vias to planes and inner-layer quality are not tuned yet. Copper counts
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
- **`[run] detour_reference`** (default on, 0.5.0): the report's detour ratio is the routed length divided by the
  length of a DRC-feasible reference path, one extra search per connection on the fixed copper only (full
  clearance and width, same layers and via rules; at most 10 % of the remaining time budget). A connection whose
  reference search fails or runs out of time falls back to its topological ghost path (the shortest path on the
  empty board that may squeeze through foreign pads) and is counted as a "ghost fallback"; the report also lists
  the ratio against the ghost line (`detour_ghost`). Off = ghost line only (`toporlib/detour.py`).
- **Pad exit stubs and necking** (`[search] exit_stubs`, `neck_to_pad`, `neck_max_mm`, `pad_bridge_max_mm`,
  0.5.0): a pad that cannot be left from its centre at the net's track width (fine-pitch rows) gets a short
  straight stub out of the pad row, if needed starting off the pad's centre line or narrower than the net width
  (never below KiCad's minimum track width, KiCad → File → Board Setup → Design Rules → Constraints → Minimum track width, or a custom-rule `track_width` minimum); two close pads of one
  footprint on the same net may be joined by one straight track. Every stub is checked against the clearance
  rules before routing; the report lists the stubs and the necked ones. A pad with no room for a stub is
  reported as `stub infeasible: <pad>` in the warnings and keeps the pad-centre start.
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
- **Pads** are exact; custom, trapezoid, chamfered and offset pads become the convex hull of KiCad's
  polygon (larger, never smaller).
- **Free vias**: KiCad's `free` flag is read from the saved board copy; a via whose flag cannot be read is
  treated as free, i.e. kept as a fixed obstacle and never ripped.
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
  via size is KiCad's factory default. On every board the router may add smaller fab-profile vias, and
  KiCad's pre-defined vias are not read ([Fab minimums and via sizes](#fab-minimums-and-via-sizes)).

**Not yet**: same-net T-junctions, via relaxation and per-layer costs
(0.5.0); free via placement and the 45° mode (backlog). Plugin calls that need a running KiCad are listed
as UNVERIFIED in [gui-test-checklist.md](gui-test-checklist.md) until that checklist has been run.
