# How KiCad 10 handles and routes differential pairs

Research for topor D7 (diff-pair routing). Written 2026-11-07. Nothing here is implemented; it is the reference
for "do what KiCad does" when D7 is built.

Sources and conventions:

- **KiCad source**: GitLab `kicad/code/kicad`, branch `10.0`, tip commit `86a7a3f4146f6a1630877865b3e9af4d5a2f5de2`
  (2026-11-05) at the time of reading. Every file was fetched from that branch and read directly. A citation
  `path:line` means this commit; the permalink is
  `https://gitlab.com/kicad/code/kicad/-/blob/86a7a3f4146f6a1630877865b3e9af4d5a2f5de2/<path>#L<line>`.
- **KiCad docs**: `https://docs.kicad.org/10.0/en/pcbnew/pcbnew.html` (single page; quoted by section title).
- **Measured**: local `kicad-cli` 10.0.8, run only on copies (scratchpad), plus my own geometry script that was
  checked against KiCad's own DRC numbers (section 5).
- **Not verified** items are marked "NOT VERIFIED". The list is at the end.

## 0. Summary

1. **Detection is by name only**, in two identical copies of one function (`BOARD::MatchDpSuffix`,
   `DRC_ENGINE::MatchDpSuffix`): skip trailing digits and `_`, then the last char must be `+`/`-` or upper-case
   `P`/`N`. The partner name is the same string with the polarity char swapped. No partner net = not a pair, silently.
   Netclasses play no part in detection.
2. **There is no pair object anywhere.** A routed pair is two ordinary nets of segments/arcs/vias in `.kicad_pcb`;
   neither `.kicad_pcb` nor `.kicad_pro` stores the pairing. Only tuning patterns are stored as `(generated ...)`.
3. **Sizes**: pair width = netclass `diff_pair_width`, pair gap (edge to edge) = netclass `diff_pair_gap`; both fall
   back to board minimum track width and minimum clearance. A custom rule `diff_pair_gap (opt ..)` overrides the
   netclass because custom rules are evaluated last-in-file first and are loaded after the implicit netclass rules.
4. **DRC checks nothing about pairs by default.** The netclass gap is only an `opt` (router target); its `min` is the
   board minimum clearance and it has no `max`. Gap, uncoupled-length and skew violations exist only when a custom
   rule (or a KiCad 10 tuning profile) sets `diff_pair_gap`, `diff_pair_uncoupled` or `skew`. Confirmed: 0 pair
   violations on the hand boards.
5. **The pair router is a gateway machine**: it builds candidate "gateway" pairs at the start pad pair and the target
   (pad pair, existing pair end, or cursor), fits P and N as two 45-degree polylines between each gateway pair,
   scores the combinations and keeps the best. Fan-out at pads is a short uncoupled stub, no length limit unless a DRC rule.
6. **Only 45-degree mitred corners** are produced by the pair placer (no reference to corner mode or arcs in
   `pns_diff_pair_placer.cpp`/`pns_diff_pair.cpp`). P and N are built independently from their own anchors, so skew
   per bend is not compensated while routing (2 p tan(theta/2) = 0.83 p per 45-degree bend, p = centre pitch).
7. **No "uncouple to make it fit" fallback.** If no gateway pair fits, the pair simply does not place (walkaround and
   shove are tried first). Local uncoupling happens only as a side effect of walking around an obstacle.
8. **Via pairs**: two vias side by side, centre pitch = via diameter + `EffectiveDiffPairViaGap`
   (`max(via gap, hole-to-hole - 2*annular ring, copper-to-hole - annular ring)`). In netclass mode the via gap is
   taken from the pair gap rule, not from the netclass field `diff_pair_via_gap`.
9. **Tuning**: DP length tuner meanders both wires about the coupled centre line; skew tuner meanders one (the
   shorter) wire only. Both are `(generated (type tuning_pattern))` groups; the DRC pair test ignores their members.
10. **Our boards**: qmtech 6 name-pairs (26 connections: confirmed; 13 pair-connections), dcsgonebadge 1 pair
    (8 connections: confirmed), wiccon-2023 0 pairs. The hand routing is 41-45 % coupled at the intended gap on
    qmtech (loose 0.8-0.9 mm coupling elsewhere), 80 % on dcs; skew is exactly 0 on three of four qmtech HDMI pairs.

## 1. Pair detection

### 1.1 The rule (source)

`BOARD::MatchDpSuffix(aNetName, aComplementNet)` at `pcbnew/board.cpp:2729`, identical logic in
`DRC_ENGINE::MatchDpSuffix` at `pcbnew/drc/drc_engine.cpp:2442` (used by the DRC and `inDiffPair()`):

- Walk the **full net name** (including the `/sheet/` path) from the last character.
- Characters `0-9` and `_` are skipped (counted).
- First other character decides: `+` -> polarity +1 (complement `-`); `-` -> -1 (complement `+`);
  `P` -> +1 (complement `N`); `N` -> -1 (complement `P`); anything else (including lower case `p`/`n`) -> not a pair.
- Complement name = name with that one character swapped, the skipped trailing characters kept:
  `CLK_N1` pairs with `CLK_P1`, `USB+` with `USB-`, `/x/RXP` with `/x/RXN`.
- Consequences: the styles cannot mix (`USB+` + `USB_N` are not a pair, also stated in the docs, "Routing
  differential pairs"). There is no `_DP/_DM`, `_p/_n`, `_H/_L`. A name like `EN` "matches" (polarity -1, complement `EP`)
  but has no partner.

`BOARD::DpCoupledNet` (`board.cpp:2777`) = `FindNet(complement)`; `null` if that net does not exist.

### 1.2 Unpaired nets and netclasses

- Router: `PNS_PCBNEW_RULE_RESOLVER::DpNetPair` (`pns_kicad_iface.cpp:1374`) returns false when the complement net is
  missing, and `DIFF_PAIR_PLACER::FindDpPrimitivePair` (`pns_diff_pair_placer.cpp:522`) reports "Unable to find
  complementary differential pair nets. Make sure the names ... end with either N/P or +/-." The route does not start
  (`pns_router.cpp:343`).
- DRC: `DRC_ENGINE::IsNetADiffPair` (`drc_engine.cpp:2492`) returns false; the pair tests skip the net;
  `inDiffPair('*')` is false for it (docs, "Custom design rule functions").
- Real example: dcsgonebadge net `/Microcontrol/STRAP_8` matches the suffix rule (skips `8`, `_`, hits `P`) but
  `/Microcontrol/STRAN_8` does not exist, so it is not a pair.
- A pair is **not** limited to one netclass. For router sizes, the queries use the start net as A and the coupled net
  as B (`pns_kicad_iface.cpp:1236-1268`), so the start net's netclass decides. NOT VERIFIED what happens when the
  halves have different netclasses in the DRC (the engine evaluates per item).

### 1.3 Where topor differs today

| | KiCad | topor Python (`priority._pair_key`) | topor Rust (`core.rs diff_pair_nets`) |
|---|---|---|---|
| Name used | full name with sheet path | full name | **last `/` segment only**: `/a/CLK_P` pairs with `/b/CLK_N` |
| Trailing digits/`_` skipped | yes | no (`D0P1` not detected) | no |
| Mixed styles | no | `FOOP` + `FOO_N` pair (the `_P/_N` rule is tried first, then `P/N` with the same key) | same |
| `@N` priority marker | n/a | stripped (topor-only) | stripped |
| Min pads | none | halves need >= 2 pads (preflight) | n/a |

On the three boards both rules give identical sets (checked), so the differences only bite on other boards. For D7
port `MatchDpSuffix` exactly, with the full net name, in one place (the plugin) and pass the pairs to the engine.

## 2. Rules: where width, gap, via gap, uncoupled, skew come from

### 2.1 Fields

| Quantity | Where it lives | Default if absent |
|---|---|---|
| diff pair width | netclass `diff_pair_width` (`.kicad_pro` `net_settings.classes[]`); per-layer in a KiCad 10 tuning profile | board minimum track width (`pns_kicad_iface.cpp:1222`) |
| diff pair gap (edge to edge) | netclass `diff_pair_gap`; custom rule `diff_pair_gap` (min/opt/max); tuning profile per layer | board minimum clearance (`:1223`) |
| diff pair via gap | netclass `diff_pair_via_gap`; "Differential pair dimensions" list in Board Setup; custom value in the router dialog | see 3.4 |
| max uncoupled length | custom rule `diff_pair_uncoupled (max ..)` only | none |
| skew | custom rule `skew (min/opt/max) [(within_diff_pairs)]` only; target for the skew tuner | none |
| length | custom rule `length (min/opt/max)` | none |
| pre-defined pair sizes | `.kicad_pro` `board.design_settings.diff_pair_dimensions` (list of width/gap/via_gap) and session "custom" values (defaults 0.125 / 0.18 / 0.18 mm: `board_design_settings.h:72-74`) | |
| tuning pattern defaults | `.kicad_pro` `diff_pair_defaults`, `diff_pair_skew_defaults` (min_amplitude, max_amplitude, spacing, corner_style, corner_radius_percentage, single_sided) | spacing 1.0 mm (pair) / 0.6 mm (skew, single); amplitude 0.2..1.0 mm; corner radius 80 % (`board_design_settings.h:76-77`, `pns_meander.cpp:42-56`) |

The router picks the source in `PNS_KICAD_IFACE_BASE::GetSizes` (`pns_kicad_iface.cpp:1222-1283`):

- If "Use net class values" is on (`BOARD_DESIGN_SETTINGS::UseNetClassDiffPair`, `board_design_settings.h:314`:
  predefined-size index 0 and no custom size), width = `max(min track width, rule value)` (unless width is inherited
  from the connected track) and gap = `max(min clearance, rule value)`, both obtained by running the DRC rule query for
  `CT_WIDTH` and `CT_DIFF_PAIR_GAP` on two dummy tracks (net P, net N).
- Otherwise the pre-defined size or the custom size is used.

### 2.2 Precedence

- Implicit rules are generated from the netclass fields (`drc_engine.cpp:313-360`) and added by
  `loadImplicitRules()` **before** the user rules from the `.kicad_dru` (`drc_engine.cpp:891-892`). Tuning-profile
  rules are added after the netclass rules (`:463`), so they win over the netclass.
- The docs state the rule: "Rules are evaluated in reverse order, meaning the last rule in the file is checked first.
  Once a matching rule is found ... no further rules will be checked" (docs, "Custom design rules"). So the order of
  strength is: custom `.kicad_dru` rules (later in file stronger) > tuning profile > netclass > board minimum. I did
  not read the iteration loop in `EvalRules` itself; the order is from the docs plus the load order above.
- A board minimum is applied at the end: `diff_pair_gap` `min` is raised to the board minimum clearance
  (`drc_engine.cpp:2060-2072`).
- The netclass gap rule is `min = board min clearance, opt = netclass gap`, **no max** (`drc_engine.cpp:330-344`).
  The docs say it too: "No DRC violations will be reported unless a minimum and/or maximum are configured using
  custom rules" (docs, DRC "Differential pair gap out of range"). A tuning profile is different: min = max(board
  min, profile gap), opt = max = profile gap, i.e. an exact gap (`:525-529`).
- **Coupled clearance override**: if the pair gap is smaller than the netclass clearance, an extra rule gives
  `clearance min = diff_pair_gap` for `AB.isCoupledDiffPair()` (P against N) (`drc_engine.cpp:346-362`). So P-N
  spacing is judged against the pair gap, everything else against the netclass clearance.

### 2.3 What DRC checks, types and severities

| Violation (JSON key) | Text | Needs | Source |
|---|---|---|---|
| `diff_pair_gap_out_of_range` | "Differential pair gap out of range" | a `diff_pair_gap` constraint with min and/or max; tuning profile | `drc_item.cpp:277`, `drc_test_provider_diff_pair_coupling.cpp:491-778` |
| `diff_pair_uncoupled_length_too_long` | "Differential uncoupled length too long" | rule `diff_pair_uncoupled (max ..)` | same file `:660-707` |
| `skew_out_of_range` | "Skew between tracks out of range" | rule `skew (max ..)`; `(within_diff_pairs)` tests each pair separately | `drc_test_provider_matched_length.cpp:240` |
| `length_out_of_range` | "Track length out of range" | rule `length` | same |
| `too_many_vias` (via count) | "Too many or too few vias on a connection" | rule `via_count` | same |
| `tuning_profile_track_geometries` | tuning-profile geometry mismatch | tuning profile | `board_design_settings.cpp:211` |

- Severities: every DRC code starts as `error` (`board_design_settings.cpp:169`); only a list of exceptions is
  warning/ignore. None of the pair codes is an exception, so all are **error** (confirmed in the `.kicad_pro` files:
  `"diff_pair_gap_out_of_range": "error"`, `"diff_pair_uncoupled_length_too_long": "error"`, `"skew_out_of_range": "error"`).
  The tuning-profile implicit rules are `ignore` by default (`:211`). A rule with `(severity ..)` overrides, and
  `ignore` disables the report but the rule still matches and shadows earlier rules (docs, "Severity Clause").
- Message formats (for reporting in KiCad's own terms):
  `(<rule> minimum gap <v>; actual <g>)`, `(<rule> maximum gap <v>; actual <g>)`,
  `(<rule> maximum uncoupled length <v>; actual <u>)`,
  skew: `(rule '<rule>' max skew <v>; actual <s>; target net length <L> (from <net>); actual <L2>)`.
  Verified from real output below.
- **What counts as coupled** (DRC, `extractDiffPairCoupledItems`, `drc_test_provider_diff_pair_coupling.cpp:283`):
  every P segment against every N segment on a shared layer that does not intersect; the *common projection* of the
  two segments gives a clipped piece; gap = nearest distance of the clips minus half the widths (`:333`). There is
  **no parallelism test for straight segments** in the DRC code (only the arc variant tests angle and radius),
  although the docs say "not tested on non-parallel portions". A candidate is dropped if any other copper lies
  between the two nearest points (and not a pad the track exits). Members of a tuning pattern are skipped
  (`isInTuningPattern`, `:280`, used at `:296`).
- **Uncoupled length** = `max(total length N, total length P) - sum(coupled length)` over the *whole board* for that
  pair, i.e. summed over all its connections (`:660`). Coupled pieces whose gap is outside min/max do not count as
  coupled. Gap violations are reported only if the uncoupled rule is violated or there is no uncoupled rule (`:707`).
- **Skew** (`matched_length`): length difference to the longest net of the group, per pair with
  `within_diff_pairs`; the length includes via barrel height from the stackup (default on:
  `m_UseHeightForLengthCalcs = true`, `board_design_settings.cpp:217`). Verified: qmtech USB_C_D geometric P-N is
  +0.094 mm, KiCad reports -3.195 mm (two vias on P only, about 1.6 mm each).

## 3. Interactive routing: PNS `DIFF_PAIR_PLACER`

Files: `pcbnew/router/pns_diff_pair_placer.cpp`, `pns_diff_pair.cpp/.h`, `pns_optimizer.cpp`, `pns_topology.cpp`.

### 3.1 Coupled routing, gap kept

- `gap()` = `DiffPairGap + DiffPairWidth` is the **centre pitch** (`pns_diff_pair_placer.cpp:623`); the edge-to-edge
  gap is `DiffPairGap`. `DIFF_PAIR::SetGap` stores `RANGED_NUM(gap, 10000, 10000)`, i.e. a **+-10 um** tolerance
  band for "coupled" (`pns_diff_pair.h:434-437`, `ranged_num.h`).
- Start (`Start`, `:662`): from a pad/via/open track end of either net; the partner primitive is the closest dangling
  anchor of the coupled net of the same kind (`FindDpPrimitivePair`, `:522`).
- Each cursor move (`routeHead`, `:684`): build entry gateways at the previous pair end, target gateways at the
  target pair (snap) or at the cursor (`BuildForCursor`), then `FitGateways`; result = two polylines `CP()`, `CN()`.
- `DIFF_PAIR::BuildInitial` (`pns_diff_pair.cpp:229`) builds P and N each with `DIRECTION_45::BuildInitialTrace`
  from their own anchors (so each wire is a 0/45/90 polyline), prepends/appends the gateway entry lines, and
  **rejects** the candidate unless: entry/exit angles are straight or obtuse (plus the gateway's allowed angles),
  `checkGap` holds (every P segment >= gap - 100 nm from every N segment, `:182`), neither wire self-intersects and
  P does not intersect N. A narrower start gap (neckdown, BGA) is allowed only within a transition radius of
  `4 * gap` around the start (`:284`).
- After fixing, `FixRoute` (`:815`) adds both lines, simplifies, and stores the end primitives for chaining; unless
  "fix all segments" is on, the last segment pair is dropped so the pair can still turn. Chain direction is flipped
  (`m_initialDiagonal`).

### 3.2 Pad entry/exit gateways

A `DP_GATEWAY` is a pair of anchor points (P, N) with optional entry lines (from the pad) and a priority.
`DP_GATEWAYS::BuildFromPrimitivePair` (`pns_diff_pair.cpp:453`):

- **Pair end is segment/arc pair** (continuation, `buildDpContinuation`, `:634`): the gateway at the existing anchors
  (priority 100); if the end is directional and P/N are aligned (delta along x, y or diagonal) two angled gateways
  per wire are added, extending one wire by `gap * sin(22.5 deg)` (priority 20) and by `gap * sin(23.5 deg)`
  (priority 5; a workaround for KiCad issue 12459), for turning by 45 degrees.
- **Pair end is pad/via pair**: pad shape drives a *fan distance*. Circle: generic only. Rect: `orthoFanDistance =
  1.5 * (w + 1)` with `w` the longer side, `diagFanDistance = w - h`. Segment (oval) pad: `w + length` and `length`.
  Polygon/compound: bounding box like rect (`:489-545`). If the two pad centres are axis- or diagonal-aligned, four
  fan-out gateways are made: stub of `orthoFanDistance` or `diagFanDistance` away from the pad pair, then widening
  to the target gap (`d = max(0, pad distance - gap)`), priorities 100 (ortho) and 99 (diagonal) (`:556-579`). So
  the stub leaves the pad *perpendicular to the line joining the two pads*, in both directions.
- **Generic gateways** (`BuildGeneric`, `:684`) for any pad pair: for the straight and diagonal lines through each
  pad, (a) the *midpoint exit* between the pads (collinear case; priority 2 when pad distance > 3 * gap, else 1; allows
  a right-angle join), plus four side-by-side exits; (b) intersections of the pads' straight/diagonal lines
  (diagonal-diagonal, straight-straight), gateway pair at `gap / sqrt2` from the intersection, obtuse entry only,
  priority 10 (pads far apart, > 3 * gap) or 20 (close); (c) mixed straight-diagonal intersections ("weirder exits",
  priority 0), skipping acute angles.
- **Target at the cursor** (`BuildForCursor`, `:581`): 8 gateways (4 diagonal-axis + 4 orthogonal-axis, each
  centred on the cursor, pitch = gap). If the cursor is within `(gap)/2` of the extension of the starting direction,
  only the straight ones are kept, projected onto that line (`routeHead`, `:722-733`); otherwise all (a 45-degree turn
  is allowed).
- **Selection** `FitGateways` (`:371`): all entry x target pairs, each tried with the preferred and the opposite
  posture (the opposite gets -3), `score = posture + entry.priority + target.priority`; best score wins, valid
  candidate required. The result is the "fan out" the docs mention: "If the starting or ending location ... is a
  different distance apart from the configured gap, the router will create a short 'fan out' section to minimize the
  length of track where the differential pair is not coupled" (docs, "Routing differential pairs").
- Uncoupled stubs therefore have a length of about 1.5 * pad length (rect pad) at each end, and nothing bounds them
  except a `diff_pair_uncoupled` rule.

### 3.3 Corners, inner/outer wire, skew per bend

- The placer produces **mitred 45-degree polylines only** (`BuildInitialTrace`, default mode); I found no use of
  `CORNER_MODE`, arcs or fillets in `pns_diff_pair_placer.cpp` or `pns_diff_pair.cpp`. The 45/90/arc corner setting
  in the router menu (`router_tool.cpp:616-640`) is the single-track placer's. NOT VERIFIED in the GUI that the pair
  placer ignores it; DRC and tuning code do handle arcs in pairs (arc-arc coupling in the DRC, `:108`).
- Because P and N are two separate polylines, the outer wire of a bend is longer. For a bend of angle theta with centre
  pitch p, the miter difference is `2 p tan(theta/2)`: 0.828 p at 45 degrees, 2 p at 90 degrees (this is the plan's
  0.83 x pitch). KiCad does nothing about it while routing; `DIFF_PAIR::Skew` = `lenP - lenN` (`:863`) is used only to
  score walkaround candidates and by the tuners.

### 3.4 Via pairs

- `ToggleVia` (`:100`, hotkey V) makes the target gateways *via-fitting* (`SetFitVias`). In `BuildForCursor` the
  anchor distance becomes `viaGap + viaDiameter` (centre to centre) and `BuildGeneric(..., aViaMode=true)` builds
  entry lines only (no midpoint exit): the two vias sit symmetrically about the cursor with their axis at a multiple
  of 45 degrees chosen by `FitGateways`.
- `viaGap()` = `SIZES_SETTINGS::EffectiveDiffPairViaGap` (`pns_sizes_settings.h:146`) =
  `max(DiffPairViaGap, DiffPairHoleToHole - 2*annular, DiffPairCopperToHole - annular)`, annular = (via diameter -
  drill)/2. Hole-to-hole and copper-to-hole come from the DRC engine constraint queries
  (`pns_kicad_iface.cpp:1286-1325`). The docs: "These vias will be placed as close as possible to each other while
  respecting the design rules for copper and hole-to-hole clearance."
- **Which via gap**: in netclass mode `diffPairViaGap = max(min clearance, pair gap rule value)`
  (`pns_kicad_iface.cpp:1264`); the netclass field `diff_pair_via_gap` is *not* read there. It is read only through
  `GetCurrentDiffPairViaGap` (`board_design_settings.cpp:1596`, default netclass) in the non-netclass branch, and
  copied to the custom size in `BOARD::SynchronizeNetsAndNetClasses` (`board.cpp:3074`). Read from source, not tested
  in the GUI.
- Both vias are added on `FixRoute` with the pair's nets (`:826-831`); the next layer starts from the via pair
  (`SetLayer`, `:620+`; layer change only when the previous end is a via pair).
- Measured: dcs USB vias are 0.6 / 0.3 mm at centre pitch 0.85 mm = edge gap 0.25 mm = the netclass pair gap,
  consistent with the formula.

### 3.5 When the gap does not fit

- There is **no uncoupling fallback** in `routeHead`: when `FitGateways` finds no valid candidate it returns the last
  good trace (`m_currentTraceOk`), the mode `rhMarkObstacles` reports `m_fitOk = false` and `FixRoute` refuses to
  fix unless "allow DRC violations" (`:815`). The user sees the pair stop at the last position that fits.
- Uncoupling arises only inside walkaround/shove (3.6): the second wire is shoved with `ForceClearance(gap - 2 *
  PNS_HULL_MARGIN)` (`:254`) and the optimizer re-merges segments to maximise coupled length. The docs call this out:
  "or becomes uncoupled to pass around another object such as a via" (docs, constraint `diff_pair_uncoupled`).

### 3.6 Walkaround and shove as one object

- `rhWalkOnly`/`rhShoveOnly` (`:331`, `:359`): `tryWalkDp` (`:286`) tries four variants (P first/N first x
  clockwise/anticlockwise). `attemptWalk` (`:207`) walks one wire around the obstacle (`WALKAROUND`, shortest
  policy), then *shoves* the other wire out of the way of the first with the pair-gap clearance; up to 3 iterations
  alternating which wire walks. Candidates are scored `1 + coupledLength + 3 * |skew|` and the **smallest score**
  is taken (`:297-312`; that favours short coupled length, which looks unintended: NOT VERIFIED whether upstream
  intended `max`). The winner goes through `OPTIMIZER::Optimize(DIFF_PAIR)` = `mergeDpSegments`
  (`pns_optimizer.cpp:1372`): merges vertices and picks bypasses that increase the coupled length while keeping the
  gap range.
- In shove mode the walked pair is then inserted as two heads into `SHOVE`, so other nets are pushed.
- `propagateDpHeadForces` (`:125`) pushes the cursor away from solid obstacles using a virtual disc of diameter
  `gap + 2 * width` (the pair's whole envelope) before building gateways.

### 3.7 Tuning

- **DP length tuner** (`PNS::DP_MEANDER_PLACER`, `pns_dp_meander_placer.cpp`): works only on the **coupled** segment
  pairs between the start and the cursor (`CoupledSegmentPairs`); baseline = midpoints of the coupled P/N segments,
  meanders drawn on the centre line with `SetBaselineOffset((gap + width)/2)` so both wires follow (`:276`).
  Uncoupled stretches cannot be length-tuned with it (docs, "Length tuning").
  Target: `length` rule or pattern property; compares `max(lengthP, lengthN)`.
- **Skew tuner** (`PNS::MEANDER_SKEW_PLACER`, `pns_meander_skew_placer.cpp`): meanders **one wire** (the one clicked,
  normally the shorter) to reach `coupledLength + target skew` (`:210`); the target skew is the `opt` of the `skew`
  rule, else `min`, else 0 (docs, constraint `skew`).
- Shapes (`pns_meander.h`): single-sided or double; rounded (arc, default radius 80 %) or chamfered corners; initial
  side left/right/default. Settings: `min_amplitude` 0.2 mm, `max_amplitude` 1.0 mm, `spacing` 1.0 mm (pair) /
  0.6 mm (single, skew), step 0.05 mm, length tolerance 0. Interactive: keys 1/2 spacing, 3/4 amplitude
  (docs, "Length tuning"). Meanders are placed at the cursor along the chosen track; there is no automatic placement
  "where the mismatch arises".
- Length used: physical length with via stackup height (default on), pad-to-die, or propagation delay with a tuning
  profile (time domain).

## 4. File format

### 4.1 A routed pair

Two nets of ordinary items; nothing marks them as a pair. From the qmtech hand board:

    (segment (start ...) (end ...) (width 0.2) (layer "F.Cu") (net "ADV_TX0+") (uuid "..."))
    (segment (start ...) (end ...) (width 0.2) (layer "F.Cu") (net "ADV_TX0-") (uuid "..."))

(KiCad 10 files name nets as strings; the older 20211014 format used numeric ids with a net table.) Vias are two
`(via ...)` items, one per net. The pair relation exists only through net names plus the netclass/rules.
`.kicad_pro` has no pair list (grep of all three projects: only `diff_pair_*` netclass fields, the
`diff_pair_dimensions` size list and meander defaults).

### 4.2 Tuning patterns

Serialised by `PCB_IO_KICAD_SEXPR::format(const PCB_GENERATOR*)` (`pcb_io_kicad_sexpr.cpp:2513`) as a
`(generated ...)` item (a group). Real sample from the qmtech hand board (single-ended; only `single` mode occurs in
our boards, no pair-mode pattern exists in them):

    (generated (uuid "576d8174-...") (type tuning_pattern) (name "Tuning Pattern") (layer "F.Cu")
      (base_line (pts (xy 106.904282 122.822718) (xy 103.339408 126.387592)))
      (corner_radius_percent 80) (end (xy 103.339408 126.387592)) (initial_side "left") (is_time_domain no)
      (last_diff_pair_gap 0.18) (last_netname "FPGA_SD_CLK") (last_status "too_short") (last_track_width 0.25)
      (last_tuning_length 0) (max_amplitude 0.5) (min_amplitude 0.2) (min_spacing 0.75)
      (origin (xy 106.904282 122.822718)) (override_custom_rules no) (rounded yes) (single_sided no)
      (target_delay ...) (target_length ...) (target_length_min/max ...) (target_skew 0)
      (target_skew_min -0.1) (target_skew_max 0.1) (tuning_mode "single")
      (members "uuid-of-track-1" "uuid-of-track-2" ...))

- Keys (`PCB_TUNING_PATTERN::GetProperties`, `pcb_tuning_pattern.cpp:1799`): `tuning_mode` (`single`, `diff_pair`,
  `diff_pair_skew`), `initial_side` (`default|left|right`), `last_status` (`too_long|too_short|tuned`),
  `is_time_domain`, `end`, `origin`, `base_line` (and `base_line_coupled` for pair mode), `corner_radius_percent`,
  `single_sided`, `rounded`, `max_amplitude`, `min_amplitude`, `min_spacing`, `target_length[_min|_max]`,
  `target_delay[_min|_max]`, `target_skew[_min|_max]`, `last_track_width`, `last_diff_pair_gap`, `last_tuning_length`,
  `last_netname`, `override_custom_rules`.
- The serpentine itself is **ordinary `segment`/`arc` items**, referenced by uuid in `(members ...)`. A pattern with
  an empty member list is not saved (`:2517`).
- Why it matters for topor: DRC skips pattern members in the pair-coupling test (`isInTuningPattern`). A serpentine
  written as plain tracks would be measured as coupled/uncoupled by geometry and may trigger
  `diff_pair_gap_out_of_range` or `..._uncoupled_length_too_long`; written as a pattern it is invisible to those two
  checks (length and skew still count it).
- What topor would need to write for an editable pattern: the `(generated)` block with `type tuning_pattern`, the
  layer, `origin`, `end`, `base_line` (+ `base_line_coupled`), `tuning_mode`, the `last_*` values, the amplitude/
  spacing/corner values used, and the uuids of the serpentine members. Missing keys appear to take defaults (the
  loader uses `get_to`, which leaves the member untouched when a key is absent), but I did not run KiCad on a
  hand-written pattern: NOT VERIFIED. Recommended: first release (D7) writes plain tracks and no patterns; only S7
  (length/phase matching) writes patterns, and then only after testing "Update All Tuning Patterns" on a copy.

## 5. Our boards

Boards: `scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb`, `dcsgonebadge/.../dcsgonebadge.kicad_pcb`,
`wiccon-2023/.../2023.kicad_pcb`. No `.kicad_dru` exists for any of them (so no pair constraints at all). Script:
`<scratchpad>/diffpair/measure.py` (copies only). Method: segments only (no arcs exist in the pair nets), coupled =
parallel within 1 degree, same layer, overlapping projection, edge gap <= 0.35 mm; bends = degree-2 vertices turning
> 1 degree; lengths exclude vias.

**Validation against KiCad**: with a probe `.kicad_dru` (`diff_pair_uncoupled (max 0mm)`, `diff_pair_gap (max 0.5mm)`,
`skew (max 0mm) (within_diff_pairs)`) on copies, KiCad reports uncoupled length qmtech TX0 8.6252, TX1 9.3555, TX2 9.2741,
TXC 9.3614, USB_C 1.2894, dcs USB 8.6329 mm. My script gives 8.625, 9.356, 9.274, 9.361, 1.289, 8.633. Agreement to
the third decimal. With a very loose gap limit (2 mm) KiCad reports less coupled length (its "nothing in between"
test removes candidates), so use the tight limit.

### 5.1 Pairs, rules, counts

| Board | KiCad name-pairs | Connections (nets with n pads give n-1) | Netclass of pair | Rules |
|---|---|---|---|---|
| qmtech | `ADV_TX0+/-`, `ADV_TX1+/-`, `ADV_TX2+/-`, `ADV_TXC+/-` (4 pads per net), `USB_C_D+/-` (2 pads), `CLK_74.25_P/N` (1 pad each: nothing to route) | 4 x 2 x 3 + 2 x 1 = **26** (plan confirmed) | `ADV_TX*`: class `HDMI_TX` (track 0.2, dp width 0.16, dp gap 0.2, dp via gap 0.25, clearance 0.127, via 0.45/0.3); `USB_C_D*`: Default (track 0.25, dp width 0.2, gap 0.25, clearance 0.15, via 0.8/0.4) | none; board min clearance 0.09, min track 0.09 |
| dcsgonebadge | `USB+/-` (5 pads per net) | 2 x 4 = **8** (plan confirmed) | Default (track 0.2, dp width 0.2, gap 0.25, via gap 0.25, clearance 0.2, via 0.6/0.3) | none; min clearance 0.1 |
| wiccon-2023 | **none** (KiCad 6 format file, 38 nets with copper) | 0 | Default | none |

Note qmtech: the hand routing of the HDMI pairs does not follow the `HDMI_TX` class (width 0.2, gap 0.25 = the
Default class's pair values, versus class 0.16 / 0.2). Hand layout is reference only.

### 5.2 Measured hand routing

| Pair | Length P / N (mm) | Skew P-N (mm) | Widths | Coupled at gap <= 0.35 (P = N) | Uncoupled (max len - coupled) | Gap histogram (mm: coupled length mm) | Bends P / N (angles) | Vias P / N |
|---|---|---|---|---|---|---|---|---|
| qm ADV_TX0 | 15.780 / 15.780 | 0.000 | 0.2 / 0.2 | 7.154 (45.3 %) | 8.625 | 0.25: 6.9, 0.9: 2.3, 0.8: 1.95, 0.86: 1.1 | 13 / 13 (all 45) | 0 / 0 |
| qm ADV_TX1 | 15.896 / 15.819 | +0.077 | 0.2 / 0.2 | 6.541 (41.2 %) | 9.356 | 0.25: 3.7, 0.9: 3.2, 0.33: 2.8, 0.8: 1.95 | 16 / 16 (14x45, 97, 38) | 0 / 0 |
| qm ADV_TX2 | 15.889 / 15.889 | 0.000 | 0.2 / 0.2 | 6.615 (41.6 %) | 9.274 | 0.25: 6.5, 0.9: 3.2, 0.8: 1.95 | 16 / 16 (14x45, 97, 38) | 0 / 0 |
| qm ADV_TXC | 15.906 / 15.906 | 0.000 | 0.2 / 0.2 | 6.544 (41.1 %) | 9.361 | 0.25: 6.3, 0.9: 3.2, 0.8: 1.95 | 16 / 16 (14x45, 97, 38) | 0 / 0 |
| qm USB_C_D | 2.652 / 2.557 | +0.094 (KiCad, with via height: -3.195) | 0.28 / 0.28 | 1.362 (52.3 %) | 1.289 | 0.22: 0.63, 0.27: 0.58, 0.34: 0.1 | 4 / 2 (43, 45, 45, 44 / 45, 45) | 2 / 0 (unpaired) |
| dcs USB | 40.391 / 41.192 | -0.801 (KiCad: -0.779) | 0.2 / 0.2 | 32.559 (79.8 %) | 8.633 | 0.25: 25.9, 0.20: 5.1, 0.30: 1.6, 0.65: 1.4 | 13 / 18 (12x45 + one 90 / 18x45) | 2 / 2, pitch 0.85, edge gap 0.25 |

- **Actual gap** is mostly the Default-class pair gap, 0.25 mm; the HDMI pairs also run loosely coupled at
  0.8-0.9 mm over about 5 mm (near the HDMI connector and the ESD array), which is why only 41-45 % is "coupled at the
  intended gap". With a 2.0 mm limit those loose parts count as coupled and the coupled share is about 84 %.
- **Uncoupled at the ends** (length from the pad to the first coupled segment): qmtech ADV7511 pads 0 (the pair
  leaves the pad already coupled); the HDMI connector pads (USB1) 2.34 mm on both P and N (a fan-out at gap 0.9);
  the ESD array pads (U3) 0.14-1.8 mm; USB_C_D: 0 at U9. dcs: U1 pads 1.76 mm both, J1 3.05 mm on N pads, 0 to 1.8 mm
  elsewhere. The lead numbers on multi-pad parts are noisy (several tracks touch one pad); treat them as order of
  magnitude.
- **Bends**: all hand bends are 45 degrees except one 90 on dcs P and one 97 and one 38 degree pair on three
  qmtech pairs (the hand route is not strictly octilinear). Skew is exactly 0 on three of the four HDMI pairs with
  13-16 bends each, so the hand layout keeps bends symmetric (cancelling).
- **Vias**: only USB_C_D (P has two, N none: skew 3.2 mm in KiCad's accounting) and dcs USB (two matched pairs).

### 5.3 KiCad DRC on the hand boards (`kicad-cli pcb drc --severity-all`, 10.0.8, copies)

| Board | Total | Pair violations (`diff_pair_gap_out_of_range`, `..._uncoupled_length_too_long`, `skew_out_of_range`, `too_many_vias`) |
|---|---|---|
| qmtech | 171 (15 clearance, 4 hole clearance, 1 invalid outline, 113 + 25 library, 10 silk edge, 2 silk over copper, 1 footprint type) | **0** |
| dcsgonebadge | 207 (8 clearance, 11 courtyards, 5 unconnected, 3 starved thermal, silk/text warnings) | **0** (3 plain clearance errors on USB- pads vs U7 GND pads: inherent footprint pitch 0.15 / 0.175 mm vs clearance 0.2) |
| wiccon-2023 | 48 (library, silk, text, 4 starved thermal, 2 padstack) | **0** (no pairs) |

With the probe rules above, qmtech and dcs report uncoupled-length and skew violations as quoted in the validation
paragraph; real violation text example:
`Differential uncoupled length too long (dp_uncoupled_probe maximum uncoupled length 0,0000 mm; actual 8,6329 mm)`.
So in a normal topor + DRC pipeline pair rules cannot fail unless the user wrote them.

## 6. What topor should copy (proposal for D7)

Plan decision: "Diff pairs = a wide wire: demand `2w + gap + s`, atomic, 1 node in the crossing graph, early in the
order, via pair side by side with its own fit check (2 cells in a BGA), fan-out zone at the pads (max uncoupled
length), octilinear skew ~ 0.83 x pitch per 45-degree bend (prefer cancelling bends); smooth has a constant gap."
(`wise-hatching-bunny.md`, "Diff pairs = a wide wire", "D7 diff pairs", "Length matching", "Phase matching".)

### 6.1 What matches KiCad

- Pair as one object with a fixed centre pitch `width + gap` and envelope `2w + gap`: KiCad's own propagation head
  uses `gap + 2 * width` (`pns_diff_pair_placer.cpp:136`).
- Via pair side by side with its own fit check, axis at 45-degree multiples.
- Fan-out at pads as an uncoupled zone, with a length limit as the only control (`diff_pair_uncoupled`).
- The 0.83 x pitch skew per 45-degree bend equals the miter geometry KiCad also produces (and does not compensate).
- No pair-specific violation by default; violations only from user rules.

### 6.2 What KiCad does that the plan lacks

1. **Separate pair width**: `diff_pair_width` (netclass) is not the netclass track width. On qmtech HDMI_TX these are
   0.16 vs 0.2 mm. The wide-wire demand must use the pair width.
2. **Pair gap source and precedence**: netclass `diff_pair_gap` < tuning profile < custom rule `diff_pair_gap (opt)`,
   floored at the board minimum clearance; plus min/max for checking only.
3. **Coupled clearance override**: P-N spacing is checked against the pair gap, not the netclass clearance, when the
   gap is smaller (`isCoupledDiffPair`). The plan's `s` is the clearance to other nets; inside the pair it is the gap.
4. **Via pair pitch** = via diameter + `max(via gap = pair gap, hole-to-hole - 2 annular, copper-to-hole - annular)`;
   the plan only says "own fit check".
5. **Gateway catalogue** at pads (midpoint exit, side exits, stub of 1.5 x pad length for rect pads, 22.5-degree
   continuation gateways) with priorities; the plan has a "fan-out zone" without shapes.
6. **Gap tolerance +-10 um** for "coupled" and a **transition window (4 x gap)** for pairs that start narrower than the
   gap (BGA neckdown).
7. **Pair-aware walkaround and optimiser** that maximises the coupled length after bypassing an obstacle.
8. **Name detection with trailing digits/`_`** (section 1) and **full-name** pairing.
9. **Length = path with via heights, pad-to-die, optional time domain** (tuning profiles) for the skew check.
10. **Tuning patterns as `(generated)` groups** that DRC ignores for coupling.

### 6.3 Where the plan conflicts with KiCad

1. **Atomic, always coupled** (plan) vs KiCad allows local uncoupling around obstacles (a pair may part to pass a
   via; the DRC reports it only when `diff_pair_uncoupled` limits it). Resolution: keep atomic as the planner
   default but allow an explicit, graded "split around an obstacle" move; never a hard failure.
2. **"Smooth has a constant gap"** (plan, arcs) vs KiCad's pair placer which never draws arcs. A smooth pair with
   concentric arcs is fine for DRC (it handles arc-arc coupling) but it is our extension, not KiCad's behaviour;
   make it a setting, default as the `[drawing]` style.
3. **Prefer cancelling bends / skew compensation at the bend** (plan, S7) vs KiCad compensating only on demand with
   the skew tuner on one wire at a user-chosen place. Treat as a graded factor, not a rule.
4. **Pair always early/wide, no polarity** (plan) vs KiCad's `BuildInitial` rejects any candidate where P crosses N:
   polarity is fixed by the pad order. The planner must treat a polarity swap (P and N cross) as not allowed; a swap
   needs a via pair on a layer change, which KiCad never does automatically.
5. **Violations**: the plan counts violations as soft; KiCad raises none without rules. Topor should report
   pair statistics always, and violations in KiCad's terms only when a matching rule exists.
6. **Detection** (section 1.3): the current Rust rule uses the last `/` segment and no digit skipping, i.e. it
   disagrees with KiCad's `inDiffPair` and DRC.

### 6.4 Concrete D7 proposal (settings, factors, reporting)

Settings (topor.toml, `[diffpair]`; defaults taken from KiCad):

| Setting | Default | Meaning |
|---|---|---|
| `enabled` | `true` | route pairs; `false` leaves them to DIFF_PAIRS_SKIPPED as today |
| `detect` | `"kicad"` | exact `MatchDpSuffix` on the full net name; `"off"`; extra `[diffpair.pairs]` table for manual `P = "..", N = ".."` entries |
| `width_source` | `"pair"` | `"pair"` = netclass `diff_pair_width` (else board min track); `"track"` = netclass track width |
| `gap_source` | `"kicad"` | netclass `diff_pair_gap`, then tuning profile, then custom rule `diff_pair_gap opt`; `max()` with board min clearance. Numeric value overrides |
| `via_gap` | `"kicad"` | pair gap, floored by hole-to-hole and copper-to-hole (`EffectiveDiffPairViaGap`). Numeric overrides |
| `gap_tolerance_mm` | `0.01` | "coupled" band (KiCad `RANGED_NUM(gap, 10000, 10000)`) |
| `max_uncoupled_mm` | from rule `diff_pair_uncoupled max`, else none | soft cap of the fan-out sum per pair |
| `skew_max_mm` | from rule `skew max (within_diff_pairs)`, else none | soft cap on P-N |
| `fanout` | `"kicad"` | gateway shapes of 3.2 (midpoint exit, side exits, rect/oval pad stub, 22.5-degree continuation) |
| `via_pair_axes` | `[0, 45, 90, 135]` | allowed via-pair axis angles |
| `split_around_obstacle` | `false` first, later `true` | allow local uncoupling as a graded move |
| `smooth` | follows `[drawing] style` | arc corners with constant gap (topor extension) |

Factors (graded, not thresholds; names in the style of `docs/dev/factors.md`):

- `dp_uncoupled`: cost grows smoothly with uncoupled length per pair, steeper once it passes `max_uncoupled_mm`
  (the smooth knee, no cut-off); warning "diff pair gap" / "uncoupled length" text in KiCad terms.
- `dp_skew`: estimated skew `sum(sense_i * 2 p tan(theta_i/2))` plus via-height difference; cost grows with
  `|skew|`, more once above `skew_max_mm`. Bends of opposite sense cancel automatically in the sum.
- `dp_gap_deviation`: cost for coupled stretches whose gap differs from the target by more than the tolerance
  (relevant for `split_around_obstacle` and BGA neckdown).
- `dp_via_unpaired`: a via on one wire without its partner (the qmtech USB_C_D hand layout has one).
- `dp_polarity_cross`: P and N crossing, effectively prohibitive.

Reporting (KiCad terms): `Differential pair gap out of range (<rule> minimum|maximum gap X; actual Y)`,
`Differential uncoupled length too long (<rule> maximum uncoupled length X; actual Y)`,
`Skew between tracks out of range (rule '<rule>' max skew X; actual Y; ...)`, with the rule name taken from the
implicit rule text (`netclass 'HDMI_TX' (diff pair)`, `tuning profile 'X'`) or the custom rule name. Emit them only
when the corresponding constraint exists; otherwise emit one summary line per pair (length P/N, skew, coupled %, vias).

Order of work for D7:

1. Port `MatchDpSuffix`, resolve pair width/gap/via gap/uncoupled/skew from netclass, tuning profile and custom rules
   (plugin side, pass to engine in the model). Test with synthetic fixtures (names with digits, no partner, mixed styles,
   different sheets).
2. Wide-wire demand and the via-pair fit check in the planner; pad gateways as the fan-out zone model.
3. Drawing: two offset polylines (mitred 45-degree, then smooth), fan-out stubs by the gateway rules.
4. Skew estimate and the factors; statistics and rule-conditional violations.
5. Only later (S7): tuning patterns as `(generated)` groups.

## 7. NOT VERIFIED / limits

- No interactive KiCad session: GUI behaviour (corner mode with pairs, key bindings, menu) taken from the source and
  docs only.
- Rule evaluation direction inside `DRC_ENGINE::EvalRules` was taken from the docs plus load order, not read from
  the loop.
- Netclass pattern matching (for assigning HDMI_TX etc.) is approximated with `fnmatch`; the netclass of each net was
  not read back from KiCad.
- The `tryWalkDp` selection looks reversed (smallest score of `1 + coupled + 3 |skew|`); not confirmed as intended.
- Tuning patterns in pair mode: only single mode exists on our boards; pair-mode keys are from source. Whether KiCad
  accepts a hand-written `(generated)` with missing keys is untested.
- The netclass field `diff_pair_via_gap` being unused by the router in netclass mode is read from source (see 3.4).
- Lead (pad end) lengths on multi-pad parts are approximate. The uncoupled/coupled lengths are validated against
  KiCad's DRC at gap limit 0.5 mm for qmtech and dcs.
- Literature: TopoR also treats a pair as "zipped" wires with a movable "slider" where the pair splits, minimum 3 x
  pair width straight length to zip (Eremex TopoR 6.3 manual, "Routing Differential Pairs", PDF p. 171), pairs
  restored by wires that obey the gap (p. 160), and serpentines on zipped or unzipped parts (Lysenko & Polubasov,
  Electronics NTB 2/2010, in `docs/literature/lysenko-polubasov-electronics-ru-highspeed-topor.txt`). Same idea as
  KiCad's coupled/uncoupled split, with the split point as an explicit object. I did not find numeric KiCad-style
  fan-out rules in TopoR's documentation.
