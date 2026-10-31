# 46 — Configuration: what the user controls, where it comes from, defaults

**Purpose.** One list of every routing/style/rule choice that is a judgement call, where its
value comes from, and its default. Rule (user, 2026-10-31): *if in doubt, make it configurable,
with a sane default*; every non-default value used in a run appears in the run report.
Input for the M4 plan (plugin options) and the backlog. Decisions below are the user's
(2026-10-31) unless marked *proposal* or *open*.

## 1. Sources, in order

1. **KiCad board setup** (design rules, netclasses, custom rules, stackup, drill/via sizes, edge
   clearance, layer types, locked items) — always wins where it has a value.
2. **Plugin config** (fab-agnostic) — everything the board setup cannot express. It is a
   **file in the project directory**, `topor.toml` next to the `.kicad_pro` (user, 2026-10-31), so
   it is project-specific and checked in together with the KiCad design. Plain TOML: readable,
   diff-friendly, comments allowed; not stored inside `.kicad_pro` (KiCad rewrites that file).
   If missing, the plugin creates it on first run with all defaults written out and commented, and
   says so in the report. Changes made from the plugin (e.g. "add to drill list?" → yes, accepted
   fallback widths) are written back to this file. Unknown keys → warning, not silent ignore; a
   `version` key allows later migration.
3. **Built-in defaults** (safe/conventional) — only where 1 and 2 are silent; the run report and
   pre-flight list every value that came from here.

There is no "fab profile" that overrides the board: the old built-in JLCPCB table becomes the
*template* for the plugin config's fab-agnostic fields (drill list, via-in-pad, …), not a rule source.

## 2. Settings

| # | Setting | Source | Default | Notes |
|---|---|---|---|---|
| 1 | Clearances, widths, via sizes, annular ring, hole sizes | board setup | — | fab-specific values are entered in KiCad, not in topor |
| 2 | **Drill list** (allowed via drills) | plugin config | 0.25 / 0.30 / 0.40 / 0.50 / 0.80 / 1.00 mm | pre-flight: every hole size used in the design but missing from the list → warning + question "add to drill list?" (yes updates the config) |
| 3 | Copper-to-edge clearance | board setup | 0.5 mm only if the board sets none (fab floor 0.2) | |
| 4 | Power-net recognition by name | plugin config | *open — explained to user, awaiting answer* | |
| 5 | Net priority | net-name convention (*proposal* `@1`…`@3` suffix) + netclass alternative | no suffix = normal | must not clash with KiCad conventions (`+`/`-` and `_P`/`_N` diff pairs, `~{…}` overbar, `[..]` buses, `{..}` groups, `/` hierarchy) |
| 6 | Extra length accepted to save a via | plugin config | 10 % | |
| 7 | Detour limit before rip-up | plugin config | 2.0 × ghost length | tied to the detour metric |
| 8a | Pour-served nets (connect via pour + via-to-pour stubs) | plugin config | auto (net has a pour on a plane/mixed layer) | per-net override |
| 8b | **Do-not-touch nets** | plugin config (list, patterns allowed) | empty | their copper is a fixed obstacle; never routed, ripped, or smoothed |
| 9 | Via in SMD pad | plugin config | off | depends on fab process (filled/capped vias) |
| 10 | Output geometry | plugin option | native arcs | segment approximation and the 45° mode (`bend_90`, `bend_acute`) are other values |
| 11 | KiCad cleanup / teardrops after routing | plugin option | off | can be invoked from the plugin: IPC run-action opens KiCad's own dialog, user confirms ([30 §4.2](../kicad/30-kicad-constraints.md#42-delegating-post-processing-to-kicad-tools)); not headless |
| 12 | Preferred direction per layer | plugin config | none | |
| 13 | **Routing layers** | board setup layer type | route on `signal` and `mixed`; not on `power` (planes); `jumper` off | KiCad 10 copper layer types: signal, power, mixed, jumper (verified `pcbnew` 10.0.8 `LT_*`); per-layer override in plugin config |
| 14 | **Respect locked tracks/vias** | plugin option | on (locked copper = fixed obstacle) | off lets topor rip/smooth locked copper |
| 15 | Bend rules (45° mode) | plugin config | `bend_90: only_if_no_fit`, `bend_acute: never` | backlog |
| 16 | Missing-width fallback | pre-flight dialog | 0.25 mm, user clicks OK | backlog |
| — | Engine options already exposed | `Options` (engine/src/model.rs) | margin 5 µm, threads, wall clock, caps, partial routes off, scope, existing-copper policy, zero-slack retry, via cost, max vias/connection, via reduction | |
| — | Expert (config file only) | plugin config `[expert]` | current constants | PathFinder history weights (`HIST_*`, `PRESENT_OVERUSE`), zero-slack retry limits |
| — | Fixed (not configurable) | code | — | numeric tolerances, grid cell sizes, parallel thresholds, `MIN_SAGITTA`, outline sub-arc sagitta: correctness, not taste |

## 3. Backlog items from this list (Could)

- **Power/ground as traces, last resort:** a mode where power and ground nets may be routed as
  tracks, but only as short as possible, after all signal nets, and never crossing high-speed
  (priority) traces.
- **Vias close to power/ground pins (4+ layers):** fan-out vias next to power/ground pins into the
  planes, removing most power routing. Risk: such vias block escape routes of neighbouring signal
  pins → placement must reserve escape channels (plan it together with exit stubs / fan-out, e.g.
  place after a dry-run escape check, cost = blocked escape capacity).
