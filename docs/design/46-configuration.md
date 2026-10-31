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

### 1.1 Documentation requirement (user, 2026-10-31)

The plugin must be usable **without AI** — installation, configuration, operation, contributing
and debugging. Concretely:

- **Self-documenting `topor.toml`:** every key carries a comment with meaning, unit, default,
  allowed range/values and what changes when you change it. Behaviours that are set **in KiCad**
  (clearances, widths, via sizes, edge clearance, netclasses, custom rules, layer types, locks)
  appear as comment-only entries naming the exact KiCad place, e.g.
  `# Clearance: KiCad → File → Board Setup → Design Rules → Constraints (not set here)`.
- **Config reference** (`docs/user/configuration.md`): one table per section, generated from the
  same source as the file comments, so they cannot drift (a test fails if a key is undocumented).
- **User guide** (`docs/user/`): install (build script, wheel, KiCad API server, plugin environment),
  first run, options dialog, pre-flight warnings and what to do about each, reading the run
  report, undo, known limitations; troubleshooting by symptom.
- **Contributor guide** (`CONTRIBUTING.md` + `docs/dev/`): repo layout, build, tests (unit, gate,
  scaling, benchmarks), running the harness on a copied board, debug tools (`route_probe`,
  profile timers, progress PNGs), how to file a bug with a reproducible board copy.
- Every warning/error message names its cause and the fix (which KiCad setting or `topor.toml`
  key), never only an internal code.

## 2. Settings

| # | Setting | Source | Default | Notes |
|---|---|---|---|---|
| 1 | Clearances, widths, via sizes, annular ring, hole sizes | board setup | — | fab-specific values are entered in KiCad, not in topor |
| 2 | **Via sizes** (diameter + drill) | KiCad: netclass via size/drill per net + Board Setup → Pre-defined Sizes → Vias (`m_ViasDimensionsList`), limited by Design Rules → Constraints (min via size, min hole, min annular) | if the pre-defined via list is empty **and** the netclass vias are KiCad's factory values: warn "via sizes not set up" and use **0.4 mm drill / 0.8 mm via** (configurable fallback) | user 2026-10-31: per net, the netclass via first, then other pre-defined vias (smaller ones only where space requires); `topor.toml` `allowed_drills` is an optional fab filter (empty = no filter); pre-flight warns about hole sizes in the design outside that filter and offers to add them |
| 3 | Copper-to-edge clearance | board setup | 0.5 mm only if the board sets none (fab floor 0.2) | |
| 4 | Power-net recognition | netclass first; name pattern only for nets still in `Default` (user: option c) | pattern `^(\+\|-)?\d\|^(V\|VCC\|VDD\|VBAT\|GND\|AGND\|PWR)` editable in `topor.toml` | pre-flight lists every net classified by name guess |
| 5 | Net priority | `@1`/`@2`/`@3` marker in the net name (user: accepted) + netclass `topor_p1..p3` alternative | no marker = normal | combination rules §2.1 |
| 6 | **Via minimisation** (user 2026-10-31) | `topor.toml` `[vias]` + per-netclass / per-priority override | `via_cost_mm = 2.0` (search: one via costs as much as 2 mm of extra trace, i.e. the router accepts up to 2 mm longer to avoid a via), `max_per_connection = 4`, `reduction_len_slack = 10 %` (clean-up pass removes a via if the via-free route is ≤ 10 % longer) | one meaningful unit: **mm of trace per via**; suggested overrides: high-speed / `@1` nets 5 mm, power 0.5 mm (vias are cheap, low inductance in parallel); report shows via count per net and vs hand |
| 7 | Detour limit before rip-up | plugin config | 2.0 × ghost length | tied to the detour metric |
| 8a | Pour-served nets (connect via pour + via-to-pour stubs) | plugin config | auto (net has a pour on a plane/mixed layer) | per-net override |
| 8b | **Do-not-touch nets** | plugin config (list, patterns allowed) | empty | their copper is a fixed obstacle; never routed, ripped, or smoothed |
| 9 | Via in SMD pad | KiCad Board Setup via protection (`m_FillVias`, `m_CapVias`, `m_PlugVias*`, `m_TentVias*` in KiCad 10) → allowed only when vias are filled + capped; `topor.toml` override | off unless filled + capped | depends on fab process |
| 10 | Output geometry | plugin option | native arcs | segment approximation and the 45° mode (`bend_90`, `bend_acute`) are other values |
| 11 | KiCad cleanup / teardrops after routing | plugin option | off | can be invoked from the plugin: IPC run-action opens KiCad's own dialog, user confirms ([30 §4.2](../kicad/30-kicad-constraints.md#42-delegating-post-processing-to-kicad-tools)); not headless |
| 12 | Preferred direction per layer | plugin config | none | |
| 13 | **Routing layers** | board setup layer type | route on `signal` and `mixed`; not on `power` (planes); `jumper` off | KiCad 10 copper layer types: signal, power, mixed, jumper (verified `pcbnew` 10.0.8 `LT_*`); per-layer override in plugin config |
| 14 | **Respect locked tracks/vias** | plugin option | on (locked copper = fixed obstacle) | off lets topor rip/smooth locked copper |
| 15 | Bend rules (45° mode) | plugin config | `bend_90: only_if_no_fit`, `bend_acute: never` | backlog |
| 16 | Missing-width fallback | pre-flight dialog | 0.25 mm, user clicks OK | backlog |
| 17 | Rip-up loop limit (times one connection may be ripped by the same blocker) | `topor.toml` | 3 (`RIP_PAIR_CAP`) | user 2026-10-31; also the PathFinder iteration cap |
| 17b | **Optimisation stop rule** (user 2026-10-31) | `topor.toml` `[iterations]` | `max = 30` (hard cap); stop early when, over the last `patience = 5` cycles, **no connection was newly routed** and the cost (length + via cost) of the best result improved by **< `min_gain_pct = 1`** in total; `timeout` (#18) still applies | gain measured over a window, not one cycle (PathFinder is noisy: eprom alternated 26/27 before converging); a newly routed connection always counts as progress (1 of 231 is only 0.4 % — a pure %-rule would stop dcsgone too early); replaces the fixed `STALL_ITERS = 8` / cap 50; report shows why it stopped (cap / no gain / timeout / cancel) |
| 18 | **Timeout** (whole run incl. DRC rounds, M3-R8) | `topor.toml` | 60 s; `0` / `none` = disabled | user 2026-10-31; when disabled the progress dialog's **Cancel** is the only stop and returns the best result so far; Big-O budget unaffected |
| 19 | **Preview before apply** | plugin flow (M4) | always on | user 2026-10-31: after routing, show a preview (PNG: hand/current vs result, both layers, new copper highlighted, report summary incl. unrouted, vias, detours, DRC) and ask **Accept / Reject**; only Accept writes to KiCad, as one undo step; Reject leaves the board untouched |
| 20 | **Pre-existing DRC violations** in kept/locked copper | pre-flight report | always listed, never blamed on the router | user 2026-10-31 (dcsgone: 59 edge-clearance errors in hand copper); report shows `preexisting` vs `routing_new` separately; optional `fail_on_preexisting = false` |
| 21 | **Pins outside the board** | pre-flight dialog | warn; on Accept they are ignored | user 2026-10-31: pads that lie outside the Edge.Cuts outline can never be routed → listed (footprint ref, pad, net); Accept → those pads are skipped (no terminal, no obstacle), their connections reported as `ignored: pad outside board`; Cancel aborts. Accepted pads saved in `topor.toml` `ignore_pads` |
| — | Engine options already exposed | `Options` (engine/src/model.rs) | margin 5 µm, threads, wall clock, caps, partial routes off, scope, existing-copper policy, zero-slack retry, via cost, max vias/connection, via reduction | |
| — | Expert (config file only) | plugin config `[expert]` | current constants | PathFinder history weights (`HIST_*`, `PRESENT_OVERUSE`), zero-slack retry limits |
| — | Fixed (not configurable) | code | — | numeric tolerances, grid cell sizes, parallel thresholds, `MIN_SAGITTA`, outline sub-arc sagitta: correctness, not taste |

### 2.1 Priority marker `@N` and other KiCad naming conventions

Parse rule: `@` + one digit 1–3, followed by end of name or a non-digit, in the **last path
segment** of the net name, after removing overbar markup `~{…}`. Names are never changed by
topor; the marker is only read. `@` in KiCad 10 net names: verified by the user (2026-10-31).

| Convention | How to combine | Example |
|---|---|---|
| Diff pair `+`/`-`, `P`/`N`, `_P`/`_N` (KiCad matches the **last** characters) | marker **before** the polarity suffix, identical on both halves — otherwise KiCad no longer pairs them | `USB_D@1+` / `USB_D@1-`, `CLK@1_P` / `CLK@1_N` |
| Bus `PREFIX[0..7]` (member = prefix + index) | marker plus a separator in the prefix, so the index does not merge into the digit | `DQ@1_[0..7]` → `DQ@1_0`…; `DQ@1[0..7]` would give `DQ@10` → ignored + warning |
| Bus group `{…}` | marker on the member names inside the group | `MEM{DQ@1_[0..7] DQS@1+ DQS@1-}` |
| Hierarchy `/sheet/…` | marker in the label text; sheet prefixes are ignored | `/ddr/DQ@1_0` |
| Overbar `~{…}` | inside or outside the braces | `~{RESET}@2`, `~{RESET@2}` |
| Power nets `+5V`, `GND` | prefer netclass (no symbol rename) | netclass `topor_p1` |
| KiCad netclass patterns | Board Setup → Net Classes → assignments can match the marker (`*@1*`), so the same name also gets widths/clearances | pattern `*@1*` → netclass `HighSpeed` |

Conflicts: marker and `topor_pN` netclass disagree → the marker wins (more specific), pre-flight
warns. Diff-pair halves with different markers → warning, the pair gets the higher priority.

## 3. Backlog items from this list (Could)

- **Power/ground as traces, last resort:** a mode where power and ground nets may be routed as
  tracks, but only as short as possible, after all signal nets, and never crossing high-speed
  (priority) traces.
- **Vias close to power/ground pins (4+ layers):** fan-out vias next to power/ground pins into the
  planes, removing most power routing. Risk: such vias block escape routes of neighbouring signal
  pins → placement must reserve escape channels (plan it together with exit stubs / fan-out, e.g.
  place after a dry-run escape check, cost = blocked escape capacity).
