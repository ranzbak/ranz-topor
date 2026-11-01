# M4 dcsgonebadge completion diagnosis (F, task DX, 2026-11-01)

Scope: why the M4a hard gate (dcsgonebadge `3e7e66f862a7`, `rip="all"`, threads 4, `wall_clock_s 600`,
`partial_routes false`: 231/231, `routing_new 0`, `unconnected 0`) misses on main `cfff05d` (A1 merged), and what
recovers it. Method: harness gate run (`run_bench`), `route_probe --profile` (new profile-only columns `ok / blocked /
metrize_failed / vias / cost / best`, no behaviour change), `gate_diag` (empty graph + 30-iteration replay + escape dumps
+ `--detour`), the hand board's copper per net from an unripped model export, and option experiments. Machine shared
with 4 workers (load 7–13 on 8 cores): wall times are inflated 1.5–2×; counters are exact and thread-independent.

## 0. Gate run on main (harness, threads 4)

routed **188/255**, partial 0, failed 67, vias 199, iterations 42 (= 9 + 6 + 16 + 11 over four passes), `stop_reason`
`no gain`, `routing_new 0`, preexisting 4, `unconnected_items 62` (`via_dangling 4`), 22 locked GND items kept,
DRC-loop rounds 1, `router_s 153.5`, `runtime_s 197.0` (route_probe alone: 155.7 s loaded, 92.6 s less loaded), detour
(ghost) median 1.003 / p90 1.52 / max 11.6 / count > 1.5: 20 of 188. 0.4.0 baseline was 172/231 in 600 s (timeout).

## 1. Accounting: 231 vs 255

`decompose` yields **231** connections (20 single-terminal nets skipped, 1 GND pour with 119 listed pads + 6 vias).
`route_inner` runs `route_pass` once (231) and then up to `POUR_REACH_PASSES = 3` pour-completion passes
(`pourcheck`): pass 1 found 20 GND items cut off their fill by the routed copper → 26 completion connections (10 routed);
pass 2: 6 still lost → the 12 unrouted completions are dropped and retried together with **15 unfrozen neighbours**
(VCC #20/#28, +3V3 #88–90, TACT_B #139, …, 15 connections of the first pass removed and re-routed, 22 connections,
15 routed); pass 3: 3 lost → 3 routed. `merge_pass` renumbers and appends, so `connections_total` = 231 − 15 + 26 − 12
+ 22 + 3 = **255** (reports carry `iteration` 4 / 1 / 11 / 6 per pass: 216 + 14 + 22 + 3). The bench prints
`report["connections_total"]` (`_route_summary`, `harness/bench.py:102`); `drcloop` runs one round (no fresh DRC spots).
So the gate's "231" is the decomposition count, "255" the count after completion passes; `routed == connections_total`
is well defined (every connection incl. completions routed) but `== 231` is not whenever a completion pass runs — with
the fixes below pass 1 should find no lost items, so both readings coincide only when nothing is cut. **Recommendation:**
A12's gate asserts `routed == connections_total`, `failed == partial == 0`, `unconnected_items == 0`, and prints the
first-pass count (add `RouteResult.connections_initial = 231` for the record, cheap). Of the 231 first-pass connections
175 were routed at the best iteration (iteration 4 of 9); the three completion passes bring 28 more and lose 15 re-routed
ones partly (12 of 15 routed).

## 2. The 67 failed connections (final report ids; mechanism letters as in the brief, i = metrization)

Counts by mechanism: **b own-pad / fine-pitch capacity 23** (11 at 0.2 mm, 12 needing a neck because the net width
0.5 mm exceeds the pad: U4 0.825×0.25 @ 0.5 pitch, U5 0.5×0.3, U7 0.625×0.25, J1 0.6×1.45, J3 0.3×1.1, U6 0.75×0.3),
**i metrization rejects a found path 37** (taut corridor pinch 11, wall grazed 7, NPTH hole clearance 6, via-site pair 4,
sketch 4, self-check vs item 3, inside obstacle 1, taut pass cap 1), **g pour completion 5**, **e dynamic 2**. None is
"genuinely no room": the hand board routes every net, with **necked** stubs at the fine-pitch parts (net 33 is 0.5 mm
but enters U4/U5 with 0.2–0.4 mm pieces; net 47 with 0.375 mm; +5V/+3V3 enter U4/J3 at 0.2 mm) and 0 vias on nets 33/47/80.

| # | net | from → to | w | pads (layer shape w×h mm) | engine reason | mech |
|---|---|---|---|---|---|---|
| 4 | VCC | D5.1→D6.1 | 0.5 | F 1.5×0.9 / F 1.5×0.9 | self-check hole (124.5,63.375) gap 0.205 < 0.35 | i-hole H2 |
| 9 | VCC | D12.1→D11.1 | 0.5 | F 1.5×0.9 / F 1.5×0.9 | infeasible site:278 ↔ site:274 | i-via pair |
| 15 | VCC | D13.1→C12.1 | 0.5 | F 1.5×0.9 / B 1×1.45 | wall pad:SF1:5 grazed 13.6 µm | i-wall |
| 18 | VCC | D9.1→C23.1 | 0.5 | F 1.5×0.9 / B 1×1.45 | sketch: bends at wall split vertex (U3.1) | i-sketch |
| 20 | VCC | D11.1→Q2.3 | 0.5 | F 1.5×0.9 / B 1.475×0.6 | infeasible site:351 ↔ site:352 | i-via pair |
| 21 | VCC | D1.1→C2.1 | 0.5 | F 1.5×0.9 / B 1×1.45 | inside obstacle kept:9100e90a | i-obs |
| 22 | VCC | D1.1→C16.1 | 0.5 | F 1.5×0.9 / B 1×1.45 | self-check hole (55.5,63.375) 0.205 < 0.35 | i-hole H1 |
| 47 | GND | U4.11→pour | 0.2 | B 0.825×0.25 / pour | infeasible pad:U4:12 ↔ pad:U4:12 | b (+g) |
| 69 | GND | U5.6→pour | 0.2 | B 0.5×0.3 / pour | blocked: no path (wall, capacity, sites) | b (+g) |
| 75 | +5V | U4.2→U4.4 | 0.2 | B 0.825×0.25 / same | blocked | b |
| 77 | +5V | U4.2→C22.1 | 0.2 | B 0.825×0.25 / B 1×1.45 | blocked | b |
| 79 | +5V | U4.17→TP9.1 | 0.2 | B 0.25×0.825 / B ⌀3 | wall pad:TP10:1 grazed 1.4 µm | i-wall |
| 83 | +3V3 | J5.1→U3.5 | 0.2 | F+B ⌀1.73 / B 1.325×0.6 | wall pad:D10:4 grazed 395 µm | i-wall |
| 84 | +3V3 | J4.1→J6.3 | 0.2 | F+B ⌀1.73 / B 0.6×1.55 | infeasible shape ↔ site:180 | i-pinch |
| 85 | +3V3 | J4.1→Q1.2 | 0.2 | F+B ⌀1.73 / B 1.475×0.6 | wall pad:D13:3 grazed 0.6 µm | i-wall |
| 86 | +3V3 | R12.2→J3.20 | 0.2 | B 1.025×1.4 / B 0.3×1.1 | infeasible pad:JP1:3 ↔ pad:JP1:3 | i-pinch |
| 97 | +3V3 | J3.17→R16.2 | 0.2 | B 0.3×1.1 / B 1.025×1.4 | self-check item:323 gap 0.114 < 0.2 | i-item |
| 100 | +3V3 | TP8.1→U6.8 | 0.2 | B ⌀3 / B 0.75×0.3 | infeasible pad:TP1:1 ↔ site:566 | i-pinch |
| 102 | +3V3 | R1.1→R16.2 | 0.2 | B 1.025×1.4 / same | self-check item:323 gap 0.148 < 0.2 | i-item |
| 108 | I2C_SDA | U1.19→J6.2 | 0.2 | B 1.5×0.9 / B 0.6×1.55 | infeasible site:222 ↔ site:217 | i-via pair |
| 111 | I2C_SCL | J5.4→U5.7 | 0.2 | F+B ⌀1.73 / B 0.5×0.3 | infeasible: pass cap | i-cap (b) |
| 114 | I2C_SCL | TP1.1→U6.6 | 0.2 | B ⌀2 / B 0.75×0.3 | blocked | b |
| 118 | D13-DOUT | D14.4→D13.2 | 0.2 | F 1.5×0.9 / same | self-check hole (55.5,116.625) 0.205 < 0.35 | i-hole H4 |
| 119 | TACT_D | SW4.2→SW4.2 | 0.2 | F 1.1×0.7 / same | sketch: inconsistent slot path | i-sketch |
| 120 | TACT_D | SW4.2→U1.15 | 0.2 | F 1.1×0.7 / B 1.5×0.9 | infeasible pad:D10:4 ↔ site:457 | i-pinch |
| 127 | TACT_C | SW3.2→U1.11 | 0.2 | F 1.1×0.7 / B 1.5×0.9 | infeasible pad:U1:17 ↔ site:348 | i-pinch |
| 131 | D12-DOUT | D12.2→D13.4 | 0.2 | F 1.5×0.9 / same | infeasible shape ↔ site:180 | i-pinch |
| 134 | D8-DOUT | D8.2→D9.4 | 0.2 | F 1.5×0.9 / same | wall edge grazed 60 µm | i-wall |
| 141 | BT1-Pin_1 | TP10.1→U4.14 | 0.5 | F+B 2×2 / B 0.825×0.25 | blocked | b neck |
| 143 | BT1-Pin_1 | C27.2→U5.2 | 0.5 | B 1×1.45 / B 0.5×0.3 | blocked | b neck |
| 144 | BT1-Pin_1 | U4.14→U4.15 | 0.5 | B 0.825×0.25 / same | blocked | b neck |
| 145 | BT1-Pin_1 | U4.15→U4.16 | 0.5 | B 0.825×0.25 / B 0.25×0.825 | blocked | b neck |
| 146 | BT1-Pin_1 | U4.16→C26.1 | 0.5 | B 0.25×0.825 / B 1×1.45 | blocked | b neck |
| 148 | BT1-Pin_1 | U5.2→U5.3 | 0.5 | B 0.5×0.3 / same | blocked | b neck |
| 153 | D17-A | D17.2→R10.2 | 0.2 | F 0.95×1.75 / B 1.025×1.4 | self-check hole (124.5,81.5) 0.25 < 0.35 | i-hole D17 slot |
| 154 | D17-A | D17.2→D18.2 | 0.2 | F 0.95×1.75 / B 0.95×1.75 | self-check hole (124.5,81.5) 0.25 < 0.35 | i-hole D17 slot |
| 166 | Q2-S | C25.2→U4.1 | 0.5 | B 1×1.45 / B 0.825×0.25 | blocked | b neck |
| 167 | Q2-S | U4.1→U4.20 | 0.5 | B 0.825×0.25 / B 0.25×0.825 | blocked | b neck |
| 168 | U4-PROG1 | R7.2→U4.13 | 0.2 | B 1.025×1.4 / B 0.825×0.25 | blocked | b |
| 169 | U4-PROG3 | R8.2→U4.12 | 0.2 | B 1.025×1.4 / B 0.825×0.25 | blocked | b |
| 176 | U4-STAT2 | R11.1→U4.7 | 0.2 | B 1.025×1.4 / B 0.25×0.825 | infeasible site:450 ↔ pad:U2:4 | i-pinch (b) |
| 177 | J1-CC1 | R6.1→U7.10 | 0.2 | B 1.025×1.4 / B 0.625×0.25 | infeasible pad:J1:B5 ↔ site:274 | i-pinch (b) |
| 179 | J1-CC1 | U7.1→J1.A5 | 0.2 | B 0.625×0.25 / B 0.3×1.45 | infeasible site:324 ↔ site:317 | i-via pair (b) |
| 187 | SPI_SDO | TP17.1→J3.2 | 0.2 | B 2×2 / B 0.3×1.1 | self-check item:913 gap 0.2 < 0.25 | i-item |
| 191 | USB- | U1.13→U7.4 | 0.2 | B 1.5×0.9 / B 0.625×0.25 | blocked (static, empty graph too) | a+b (#201 class) |
| 192 | USB- | U7.4→U7.7 | 0.2 | B 0.625×0.25 / same | blocked (static) | a+b |
| 193 | USB- | U7.4→J1.B7 | 0.2 | B 0.625×0.25 / B 0.3×1.45 | blocked (static) | a+b |
| 197 | USB+ | J1.A6→J1.B6 | 0.2 | B 0.3×1.45 / same | wall pad:J1:A7 grazed 305 µm | i-wall (a) |
| 201 | TFT_D~C | TP12.1→J3.7 | 0.2 | B 2×2 / B 0.3×1.1 | infeasible pad:SW5:1 ↔ site:103 | i-pinch |
| 202 | D23-A | J1.A4→J1.A9 | 0.5 | B 0.6×1.45 / same | blocked | b neck |
| 203 | D23-A | J1.A4→J1.B9 | 0.5 | B 0.6×1.45 / same | sketch: inconsistent slot path | i-sketch (b neck) |
| 204 | D23-A | J1.A9→J1.B4 | 0.5 | B 0.6×1.45 / same | sketch: inconsistent slot path | i-sketch (b neck) |
| 211 | TFT_~TE | J3.4→TP15.1 | 0.2 | B 0.3×1.1 / B 1.5×1.5 | blocked | b |
| 212 | J1-CC2 | U7.2→U7.9 | 0.2 | B 0.625×0.25 / same | blocked (static) | a+b |
| 213 | J1-CC2 | U7.9→J1.B5 | 0.2 | B 0.625×0.25 / B 0.3×1.45 | blocked (static) | a+b |
| 214 | J1-CC2 | J1.B5→R5.2 | 0.2 | B 0.3×1.45 / B 1.025×1.4 | self-check hole (87.11,125.85) 0.205 < 0.35 | i-hole J1 locator |
| 223 | GND | BT1.2→pour | 0.2 | B 0.8×1.6 / pour | blocked (pass 3) | g |
| 227 | GND | C24.1→pour | 0.2 | B 1×1.45 / pour | blocked (pass 3) | g |
| 228 | GND | C23.2→pour | 0.2 | B 1×1.45 / pour | blocked (pass 3) | g |
| 229 | GND | C10.2→pour | 0.2 | B 1×1.45 / pour | blocked (pass 3) | g |
| 231 | VCC | D11.1→C11.1 | 0.5 | F 1.5×0.9 / F 1×1.45 | blocked (pass 3 re-route) | e |
| 232 | VCC | D11.1→topor:86 | 0.5 | F 1.5×0.9 / router via | infeasible pad:U1:9 ↔ pad:U1:8 | i-pinch |
| 233 | GND | U1.29→pour | 0.2 | B 0.8×0.8 (EP grid) / pour | blocked: no path (pass 3) | g |
| 235 | GND | J1.A1→pour | 0.2 | B 0.6×1.45 / pour | wall pad:D11:1 grazed 150 µm | i-wall (g) |
| 245 | D10-DOUT | D11.4→D10.2 | 0.2 | F 1.5×0.9 / same | blocked: no path (pass 3 re-route) | e |
| 247 | USB+ | U1.14→topor:755 | 0.2 | B 1.5×0.9 / router via | blocked: no path (pass 3 re-route) | e |
| 248 | D23-A | D23.2→J1.A4 | 0.5 | B 0.9×1.2 / B 0.6×1.45 | blocked | b neck |

Evidence per class. **(b)** `gate_diag` escape dumps: every exit edge of these pads is unfit for the pad's own net —
U4 tip `e1443` len 0.125 cap 0 (two own corners r 0.0625), long side `e1454` len 0.7 cap 0.575 < need 0.605 (0.2 mm:
w + ε + 2 × 0.2 gaps) and < 0.905 (0.5 mm); U5 `e4719` cap 0.3; U7 `e3904` cap 0; J3.3 `e3125` cap 0.3; R18.1 `e2703`
cap 0.025; C27.2 side cap 0.45 < 0.905 (0.5 mm). On the empty graph 13 of these are `BLOCKED` by the search itself
(nets 33/47/65/80: no state can be placed on any exit, 0.05–1.0 s each) and 9 more route alone but not with the others
present (the sibling of the same net takes the only priced exit, no T-junction: `#75 U4.2→U4.4`, `#191`, `#209`). With
A4's rule (own ends: no gap, corner radius given back) U4's long side becomes cap 0.825 ≥ 0.205: fits at 0.2 mm; at 0.5
mm the 0.25-wide pads at 0.5 mm pitch can never carry 0.505 + two 0.2 mm gaps to the neighbours — the hand necks.
**(a)** `#191/192/193/212/213` (U7, 0.625×0.25) are blocked on the empty graph in both directions; the long side
cap 0.375 is priced as overuse but `state_point`/`los` place no state (the A2 brief's #201 class); `#197` is the #208
los runaway (search alone 24.88 mm for a 1 mm pair). **(i)** 37 paths are found by the negotiator and rejected by the
metrization: `taut` cannot join two consecutive anchors whose stacks overlap (`A ↔ B`: the slot path crossed an unfit
edge priced as overuse — `PRESENT_OVERUSE` 4 is a price, not a wall, so the search commits to corridors the geometry
cannot realise; 15 cases), grazes a wall mid-span (7), or the self-check finds a 0.205 mm gap to an **NPTH** hole (H1,
H2, H4, the D17/D18 slots, J1's locator; 6 cases + the zero-slack retry of #4 at margin 0: the relax/taut anchor of an
NPTH pad is a net-0 *pad* with clearance 0.2 while `Checker` charges `hole_clearance` 0.35 — a plain bug, the graph
edges at that hole do carry `c_e 0.35`). **(g)** 5 pour completions blocked in pass 3 where the completion must reach a
site inside the pour's main component through copper routed in pass 1 (A11 pour-cut cost would avoid cutting them at
all). **(e)** 2 pass-3 re-routes of unfrozen neighbours (D11.4→D10.2, U1.14→router via) that no longer fit.

## 3. Per-iteration profile (route_probe, threads 4, loaded machine; counts exact)

| pass / iter | paths | ok (metrized) | blocked | metrize-failed | vias | cost mm | best |
|---|---|---|---|---|---|---|---|
| 1 / 1 | 208 | 170 | 23 | 38 | 148 | 2003 | * |
| 1 / 2 | 212 | 171 | 19 | 41 | 152 | 1900 | * |
| 1 / 3 | 212 | 172 | 19 | 39 | 178 | 2069 | * |
| 1 / 4 | 209 | **175** | 21 | 34 | 183 | 2203 | * |
| 1 / 5 | 215 | 166 | 15 | 49 | 169 | 1877 | |
| 1 / 6 | 209 | 163 | 21 | 46 | 182 | 1916 | |
| 1 / 7 | 208 | 165 | 22 | 43 | 196 | 2116 | |
| 1 / 8 | 210 | 157 | 20 | 53 | 190 | 1921 | |
| 1 / 9 | 205 | 156 | 25 | 49 | 203 | 1924 | stop: no gain |
| 2 / 1–6 | 10–17 of 26 | 9–10 | 9–16 | 0–8 | 9–10 | 32–40 | best 1; no gain after 6 |
| 3 / 1–16 | 17–19 of 22 | 9–15 | 3–5 | 2–10 | 5–22 | 44–286 | best 11; no gain after 16 |
| 4 / 1–11 | 3 of 3 | 2–3 | 0 | 0–1 | 2–3 | 7–27 | best 6; no gain after 11 |

Phases (pass 1, per iteration): search 1.9–3.5 s (parallel), **commit 2.4–5.3 s** (131–150 of ~190 found paths are
invalidated by earlier commits and re-searched sequentially: `re-searched` 132–150, `revalidated` 0–5 — F4's O(k)
re-validation almost never applies on dcsgone because every commit activates via sites near most paths), metrize
0.14–0.74 s, check 0.01 s. Totals: search 30.6 s, commit 41.1 s, metrize 4.3 s of 92.6 s (ghost 31.7 s CPU in 1498
runs; search 4161 runs, 21.5 ms/run). Rip-ups 1–9 per iteration, overuse edges 315→475 (growing). Why the stop rule
fires: the negotiator's path count is flat (205–215) while the metrized count **falls** after iteration 4 (175 → 156):
history pushes the re-searched connections into tighter corridors that taut rejects; `no_gain` (patience 5, 1 %) then
correctly stops a degrading run at 9 — more iterations do not help (experiment E2). The iteration cap (30) and the wall
clock (600 s) never bind; the whole run is 93–156 s loaded.

## 4. Detours (ghost reference; A7's DRC-feasible reference not on main yet)

| # | net | from → to | mm | ghost | ratio | vias | mechanism |
|---|---|---|---|---|---|---|---|
| 32 | VCC | U3.1→U3.3 | 31.0 | 2.67 | 11.6 | 4 | own 0.6 mm pad tips cap 0 for 0.5 mm (b): 2 escape vias each end, sibling #33 U3.1→C23.1 (same net) holds the exit, 4-via loop; hand 3.63 mm 0 vias (true detour 8.5) |
| 237 | GND | J1.B1→pour | 19.7 | 2.40 | 8.2 | 2 | pass-3 completion: nearest pour sites inside the main component are far (g) |
| 238 | GND | J1.B12→pour | 50.2 | 6.90 | 7.3 | 4 | same (g) |
| 194 | USB- | J1.A7→J1.B7 | 5.2 | 1.00 | 5.2 | 2 | ghost through A7's pad (underestimate, true ≈ 1.2–1.5) + los runaway on the 0.3 mm tips (a) — A2 |
| 230 | VCC | topor:27→D3.1 | 56.8 | 15.5 | 3.7 | 3 | pass-3 re-route from a router via with pass-1 copper frozen (e) |
| 196 | USB+ | U7.6→J1.A6 | 8.6 | 2.41 | 3.6 | 2 | same-net sibling holds U7.6's exits (c, A5) |
| 93 | +3V3 | R22.1→TP8.1 | 23.1 | 7.85 | 2.9 | 2 | iteration-4 corridor with 2 vias (history) — rip-up/stop (e) |
| 254 | GND | R20.2→pour | 18.1 | 6.54 | 2.8 | 1 | pass-4 completion (g) |
| 43 | GND | SW1.0→pour | 2.32 | 0.87 | 2.7 | 1 | own tip cap 0.600 < need 0.605 by ε (b): next site 2.3 mm away; ghost 0.87 inflates the ratio |
| 60 | GND | J3.8→pour | 2.37 | 1.08 | 2.2 | 1 | 0.3 mm J3 pad tip unfit (b) |

Pattern: 7 of 10 are own-pad capacity or its consequences (escape vias), 3 are completion-pass artefacts; 53 of 188
routed connections carry ≥ 2 vias (1.06 vias per connection vs the hand's 0.9 per net). `gate_diag --detour` on the
replay confirms the M3 detour note: #34 (U3.1→U3.3) search alone = 2-via 8.22 mm (cost 12.9 incl. overuse on the own
tips `e3700`), ghost 2.67 with one soft edge = that own tip; with history it grows to 4 vias / 31 mm. #204 (J1.A7→J1.B7)
search alone 24.88 mm with cost 61 (los runaway, static); with the others present it is blocked on A7's pad edge
`e4910`. #207 (U7.6→J1.A6) search alone = ghost = 2.41 mm direct; with the others present 2 vias / 5.94 mm because the
same-net sibling occupies `e3944` (cap 0).

## 5. Experiments (route_probe, same model, `wall_clock_s 600`, `partial_routes false`; defaults unchanged)

| experiment | routed | failed | vias | iters | stop | note |
|---|---|---|---|---|---|---|
| default (threads 4) | 188/255 | 67 | 199 | 42 | no gain | reference |
| E1 `strict_unfit_edges: true` | 64/232 | 168 | 53 | 16 | stuck | nearly every SMD exit is "unfit" under the current capacity rule: rejecting them kills the board — the rule, not the pricing, is wrong |
| E2 `patience 15, min_gain_pct 0` | 188/255 | 67 | 199 | 120 | cap/no gain | identical result after 410 s: iterations are not the lever; the stop rule is right to stop at 9 |
| E3 `via_cost_nm 500000` | 174/245 | 71 | 198 | 33 | no gain | cheaper vias: fewer pour cuts (9 lost items) but 14 fewer routed — via escapes do not replace fitting exits |
| E4 `max_vias_per_connection 6, ripup_max_per_pair 3` | 189/261 | 72 | 200 | 26 | no gain | +1 routed, 6 more completion connections (more pour cuts): via limits are not the lever |

None of the existing knobs moves completion by more than ±1 in the right direction; the capacity rule (A4), the
necks (NX), the NPTH clearance (HC) and the search entry (A2) are code changes, not settings.

## 6. Ranked fix list (recovered connections estimated against the 67 failed; the classes overlap, so the sum exceeds 67)

1. **A4 own-net / same-net capacity, wall-aware (M4-D1) — ≈ 25–35.** Direct: the 11 0.2 mm (b) cases (U4/U7/J3/U6 exits
   fit once own corners charge no gap and give the radius back; U7's 0.625×0.25 long side becomes 0.625 ≥ 0.205) and the
   15 `A ↔ B` / sketch cases whose pinch is an own-pad edge priced as overuse (`#47 U4.12↔U4.12`, `#86 JP1.3↔JP1.3`,
   `#232 U1.9↔U1.8`, the J1 trio); indirect: fewer escape vias (#32, #196, #43, #60), fewer `metrize_failed` (34–53 per
   iteration today), and the stop rule stops degrading. Must come **first**: E1 shows the capacity rule is what makes
   both the search (overuse everywhere, 315–475 edges) and the metrization (37 rejects) inconsistent.
2. **New small fix NX "neck to the pad" — ≈ 12.** 0.5 mm nets (33, 47, D23-A, VCC at U3) entering pads narrower than
   `w + 2c` (U4 0.25, U5 0.3, J1 0.6, U3 0.6): the connection's first/last segment uses `min(w, pad_width − 2ε)` (≥
   `min_track_width`) for a length `≤ neck_len` (default 2 × pad length), as the hand does (0.2–0.4 mm stubs). Options:
   `[routing] neck_to_pad = true`, `neck_max_mm`. Without it the 0.5 mm nets at U4/U5/J1 are unroutable by any
   capacity rule (0.5 + 2 × 0.2 > 0.5 pitch). Fits A6's "exit stub" machinery (the stub is the necked piece), so A6 +
   NX can be one task.
3. **HC NPTH hole clearance in relax/taut — 6 (+1 zero-slack).** NPTH pads (H1/H2/H4, D17/D18 slots, J1 locator) are
   anchored as net-0 pads at 0.2 mm; `Checker` wants `hole_clearance` 0.35. Make the obstacle/anchor of an NPTH pad a
   `Hole` kind (or `max(clr, hole_clearance)` when `attr == npth`). Half a day, deterministic, no option needed.
4. **A2 fine-pitch entry + los convergence — 6 direct (#191/192/193/212/213/197), + the 5.2× and 3.6× detours.**
   U7's pads block the search itself (no state on any exit even when priced); A2's `state_point` shrink and
   `los` rejection fix the two static search defects. Together with A4 the USB-/CC2 nets route on B.Cu like the hand.
5. **A5 same-net Steiner / T-junction + net ordering (M4-D3) — ≈ 8–10.** Sibling connections of one net hold the single
   exit of a fine-pitch pad (`#75 U4.2→U4.4` + `#77 U4.2→C22.1`, `#144/145`, `#192/193`, `#212/213`, #196/#32 detours):
   terminating on the sibling's copper removes the second exit entirely. After A4 (the exit must first fit once).
   Then: **A11 pour-cut cost** (the 26 + 22 + 3 completion connections and the 5 (g) failures exist only because pass-1
   copper cut 20 GND items off the fill; cost on pour-cutting edges avoids 3 passes and the 15 re-routes) ≈ 5 + the
   pass-3 (e) 2; **taut "wall grazed mid-span"** (7, `#79` by 1.4 µm, `#85` by 0.6 µm: an end-anchor slack band of
   `SLACK_BAND_NM` would accept sub-µm grazes; the 305/395/150 µm cases are A4/A2 pinches); **A16 / rip-up**: rips are
   1–9 per iteration and never ping-pong (`ripped` 0 after iteration 2 in the replay) — not a completion lever here;
   **A13 via relaxation / A10 via sizes**: no failure is a via-site shortage (0 "no via site" reasons, 679 sites); the 4
   `site ↔ site` rejects are adjacent sites both activated — A4/strict pricing fixes them, not relaxation.
   **Stop-rule params**: not a lever (E2). **Runtime**: commit's sequential re-search (132–150 per iteration, F4 revalidates
   0–5) is 44 % of the run; harmless for the 600 s gate, worth a site-aware `path_valid` later.

Expected after 1–4: the 23 (b) + 6 (i-hole) + most of the 37 (i) cases route; remaining risk is the (g) completions
(A11) and the pass-3 re-routes. A12 should re-run this note's `classify.py` table (scratch, reproducible from
`route_probe --dump`) after each fix and keep `routed == connections_total` as the gate predicate.

## Round 2 (rotation 2, base 789e050)

F task DG, 2026-11-01, worktree on main bde9602 (engine = ea1f064 = 789e050). No behaviour change: the only code
change is profile-only (`IterProfile.fail_classes` / `blocked_classes`, `route::reason_class`, printed by
`route_probe --profile`); the dcsgone result JSON with the new build is byte-identical to the base build's
(`profile` field removed, checked). Prototypes (net-order variants, generalised repair, depth-2 repair) were built in
a copy of the engine under `target/` and never touched `engine/`; their diff is
`docs/progress/notes/m4-dcsgone-dg-prototypes.patch` (for VR / the fix tasks, not for merge).

**Measuring.** The machine carried load 18–39 on 8 cores the whole time (4–5 other workers), so every dcsgone
number below is a `wall_clock_s 0` run (no deadline, deterministic, thread-independent; rotation-2 rule) and every
time is inflated 2–3×. Engine-only runs use `route_probe` on the harness's exported model (`--options
{"wall_clock_s":0,"partial_routes":false,"detour_reference":true,"threads":4}`); the harness numbers come from one
`run_bench(rip="all", threads=4, wall_clock_s 0)`. Old engines were built from `git archive` copies under
`target/` (3cddcc9 = A6 only, a233d62 = A6 + A5); all of them read the same model JSON, and the A6-only run
reproduces the A6 report exactly (210/241, 175 vias, 23 iterations).

**Harness caveat (new, small fix for the coordinator).** With `wall_clock_s 0` the harness DRC loop never runs a
second round: `toporlib/drcloop.py` `out_of_budget()` is `budget_s - router_s < 1.0`, true for a budget of 0, so
`drc_loop.budget_exhausted` is `True` after round 1. The engine reads 0 as "no deadline"; the DRC loop reads it as
"no budget". Consequence: the 0-s run reports `routing_new 1` (`shorting_items`: JP1.1 ↔ Net-(J3-Pin_22) track, the
NT task's net-tie case) that the 600-s gate run's second DRC round removes. So `wall_clock_s 0` reproduces the quiet
gate run's *router* numbers but not `routing_new` / `unconnected`. Fix: treat `budget_s <= 0` as unlimited in
`route_with_drc_loop` (one line + a test).

### R2.0 Numbers at base (= head)

| board (base 789e050, wall_clock 0 for dcsgone) | routed / total | failed | vias | routing_new | unconnected | outside mm | router_s | detour ref med / p90 / max / > 1.5 | ghost max / > 1.5 | stop |
|---|---|---|---|---|---|---|---|---|---|---|
| dcsgonebadge (harness, load 36→28) | 205/241 | 36 | 196 | 1 (DRC loop not run, see above) | 38 (+ track_dangling 1, via_dangling 2) | 1814.5 | 257.6 (runtime 407) | 1.000 / 1.294 / 4.62 / 9 | 4.71 / 15 | no gain, 24 it |
| eprom-to-kernal (gate options, load 22) | 27/27 | 0 | 0 | 0 | 0 | 64.628 | 0.33 | 1.000 / 1.006 / 1.15 / 0 | 1.31 / 0 | converged |
| wiccon-2023 (gate options, load 22→33) | 62/62 | 0 | 34 | 0 | 0 | 1363.375 | 13.4 (loaded; quiet 1.5) | 1.000 / 1.053 / 1.28 / 0 | 1.30 / 0 | no gain |

dcsgone top detours (reference): U1-EN SW5.2→SW5.2 4.62, VCC D15.1→C15.1 4.22, TACT_B 3.56. Head = base on all
three boards (profile-only change; dcsgone result JSON identical).

### R2.1 Accounting: 229 first pass → 241

`decompose` now yields **229** connections, not 231 (A-C20: overlapping same-net pads merged). Passes (base):

| pass | connections | how | failed at the end |
|---|---|---|---|
| 1 | 229 | first pass; 22 of them are later unfrozen and dropped → 207 survive | 29 |
| 2 (pour completion) | 11 for 12 GND items cut off their fill (D11.3, C11.2, C31.2, C25.1, U3.2, C24.1, U7.3, U7.8, C15.2, Q3.2, J1.A1, J1.B12) | 5 routed; the 4 still-lost failures are retried in pass 3 → 7 kept | 2 (C11.2, U7.8) |
| 3 (retry + unfreeze) | 26 = 4 retried + 22 unfrozen neighbours (VCC ×3, +3V3, IC1-VCC ×2, U1-EN ×3, D2-DOUT, J1-CC1 ×3, USB- ×2, USB+, D23-A, J1-SHIELD, J3-Pin_3 ×2, J1-CC2 ×2) | the USB corner is re-routed with pass-1/2 copper frozen | 5 (VCC C14.1→C15.1, CC1 U7.1→J1.A5, USB- U7.4→J1.B7, D23-A D23.2→J1.A4, CC2 U7.9→J1.B5) |
| 4 | 1 (U2.3) | | 0 |

229 − 22 + 7 + 26 + 1 = **241** = `connections_total`. `unconnected_items` 38 vs 36 failed: VCC has 3 failed but 2
islands (C14.1→C15.1's pads are joined through other VCC copper), +3V3 4 islands for 3 failures and IR_RX 1 island
with **0** failures — both extra islands are a net copper **polygon** (`copper_shapes` with a signal net: "Polygon
[IR_RX] on F.Cu" ↔ the IR_RX track, "Polygon [+3V3] on F.Cu" ↔ a +3V3 arc). `decompose` does not make a signal
net's copper polygon a terminal, so the router never joins it although the hand board does: a decomposition gap
worth a small fix (2 unconnected items, no failed connection shows it). GND: 2 failed, 3 islands (the dangling GND
via at (109.0, 110.14) in front of U4.9 is one, see R2.3 (d)).

**The 36 failed connections (base, final ids).** mech: DX letters — b/a static pad entry (blocked even alone),
e dynamic (blocked although it routes alone), i-* metrization rejects (i-wall, i-pinch = taut cannot join two
anchors, i-via pair = both anchors via sites, i-item, i-viahole = self-check against a via drill), g pour completion.
"alone" = the DRC-feasible reference (slot search of this connection alone on the fixed copper) found a path.
"hand" = shortest hand-copper path between the two pads on the unripped snapshot (length, vias, layers, narrowest
piece if narrower than the net width). "routed in" = how many of the 13 other runs of R2.2/R2.3 route it (same model).

| # | net | from → to | w | pads (layer w×h mm) | engine reason | mech | alone | hand | routed in |
|---|---|---|---|---|---|---|---|---|---|
| 14 | VCC | D13.1→C12.1 | 0.5 | F 1.5x0.9 / B 1x1.45 | self-check: item:338 gap −0.17 < 0.2 | i-item | yes | 16.7 mm v1 F/B neck 0.2 | 6/13 |
| 20 | VCC | D11.1→Q2.3 | 0.5 | F 1.5x0.9 / B 0.6x1.475 | wall pad:SW6:2 grazed 156 nm | i-wall <5 µm | yes | 19.5 mm v3 F/B neck 0.35 | 2/13 |
| 74 | +5V | U4.2→U4.4 | 0.2 | B 0.825x0.25 / same | blocked: no path | e | yes | 1.1 mm v0 B | **0/13** |
| 75 | +5V | U4.2→U4.19 | 0.2 | B 0.825x0.25 / B 0.25x0.825 | blocked: no path | e | yes | 6.3 mm v2 F/B | **0/13** |
| 76 | +5V | U4.2→C22.1 | 0.2 | B 0.825x0.25 / B 1.45x1 | blocked: no path | e | yes | 4.3 mm v0 B | 1/13 |
| 80 | +5V | D23.1→C22.1 | 0.2 | B 1.2x0.9 / B 1.45x1 | site:390 ↔ pad:Q2:1 | i-pinch | yes | (other tree) | 8/13 |
| 92 | +3V3 | R21.2→R2.2 | 0.2 | B 1.4x1.025 / same | pad:TP5:1 ↔ site:395 | i-pinch | yes | (other tree) | 11/13 |
| 96 | +3V3 | U1.2→R1.1 | 0.2 | B 0.9x1.5 / B 1.025x1.4 | wall pad:U1:1 grazed 305 µm | i-wall deep | yes | 15.8 mm v0 B | 10/13 |
| 98 | +3V3 | J3.17→R16.2 | 0.2 | B 1.1x0.3 / B 1.025x1.4 | wall pad:J3:15 grazed 305 µm | i-wall deep | yes | 26.1 mm v2 F/B | 6/13 |
| 105 | I2C_SDA | J4.3→J6.2 | 0.2 | F+B ⌀1.73 / B 0.6x1.55 | wall shape grazed 548 nm | i-wall <5 µm | yes | 33.8 mm v3 F/B | 4/13 |
| 111 | I2C_SCL | J5.4→U1.18 | 0.2 | F+B ⌀1.73 / B 0.9x1.5 | site:433 ↔ pad:D10:2 | i-pinch | yes | 94.0 mm v7 F/B | 1/13 |
| 112 | I2C_SCL | J5.4→U5.7 | 0.2 | F+B ⌀1.73 / B 0.3x0.5 | blocked: no path | e | yes | 22.2 mm v1 F/B | 4/13 |
| 130 | D12-DOUT | D12.2→D13.4 | 0.2 | F 1.5x0.9 / same | site:152 ↔ site:146 | i-via pair | yes | 16.2 mm v2 F/B | 9/13 |
| 145 | BT1-Pin_1 | U4.16→C26.1 | 0.5 | B 0.25x0.825 / B 1.45x1 | site:494 ↔ site:492 | i-via pair | yes | 8.2 mm v0 B | 10/13 |
| 149 | D10-DOUT | D11.4→D10.2 | 0.2 | F 1.5x0.9 / same | wall pad:SW3:0 grazed 182 µm | i-wall graze | yes | 15.3 mm v0 F | 5/13 |
| 153 | D17-A | D17.2→R10.2 | 0.2 | F 0.95x1.75 / B 1.025x1.4 | pad:D8:1 ↔ site:542 | i-pinch | yes | 43.1 mm v5 F/B | 9/13 |
| 169 | U4-PROG3 | R8.2→U4.12 | 0.2 | B 1.025x1.4 / B 0.825x0.25 | blocked: no path | e | yes | 2.8 mm v0 B | 10/13 |
| 178 | U4-STAT1 | U4.8→R13.1 | 0.2 | B 0.25x0.825 / B 1.025x1.4 | blocked: no path | e | yes | 25.9 mm v0 B | 3/13 |
| 180 | SPI_~CS1 | TP14.1→J3.5 | 0.2 | B 2x2 / B 1.1x0.3 | wall pad:TP15:1 grazed 79 µm | i-wall graze | yes | 16.3 mm v0 B | 9/13 |
| 184 | SPI_SDO | TP17.1→J3.2 | 0.2 | B 2x2 / B 1.1x0.3 | wall pad:J3:1 grazed 305 µm | i-wall deep | yes | 13.5 mm v0 B | 7/13 |
| 188 | USB- | U1.13→U7.4 | 0.2 | B 0.9x1.5 / B 0.625x0.25 | blocked: no path | e | yes | 35.2 mm v2 F/B | 5/13 |
| 189 | USB- | J1.A7→J1.B7 | 0.2 | B 0.3x1.45 / same | site:300 ↔ site:303 | i-via pair | yes | 2.7 mm v0 B | 3/13 |
| 190 | USB+ | U1.14→U7.5 | 0.2 | B 0.9x1.5 / B 0.625x0.25 | wall pad:U1:13 grazed 305 µm | i-wall deep | yes | 37.0 mm v2 F/B | 3/13 |
| 191 | USB+ | U7.6→J1.A6 | 0.2 | B 0.625x0.25 / B 0.3x1.45 | self-check: item:799 gap 0.06 < 0.25 | i-item | yes | 2.4 mm v0 B | 8/13 |
| 192 | USB+ | J1.A6→J1.B6 | 0.2 | B 0.3x1.45 / same | pad:J1:A7 ↔ site:291 | i-pinch | yes | 2.9 mm v0 B | 5/13 |
| 193 | IR_TX | U1.17→R18.1 | 0.2 | B 0.9x1.5 / B 1.025x1.4 | site:334 ↔ site:327 | i-via pair | yes | 69.0 mm v6 F/B | 6/13 |
| 197 | D23-A | J1.A4→J1.A9 | 0.5 | B 0.6x1.45 / same | blocked: no path | e | yes | 6.3 mm v0 B | 1/13 |
| 204 | TFT_~TE | J3.4→TP15.1 | 0.2 | B 1.1x0.3 / B 1.5x1.5 | site:89 ↔ site:87 | i-via pair | yes | 13.0 mm v2 F/B | 10/13 |
| 205 | J1-CC2 | J1.B5→R5.2 | 0.2 | B 0.3x1.45 / B 1.025x1.4 | site:255 ↔ pad:J1:S1 | i-pinch | yes | 6.1 mm v0 B | 7/13 |
| 208 | GND | C11.2→pour | 0.2 | F 1x1.45 / pour | blocked: no path (pass 2) | g | yes | pour | **0/13** |
| 212 | GND | U7.8→pour | 0.2 | B 0.575x0.45 / pour | blocked (pass 2) | g + b | no | pour (hand: U7.3–U7.8 bridged, 1 via) | 12/13 |
| 216 | VCC | C14.1→C15.1 | 0.5 | B 1x1.45 / same | blocked: no path (pass 3) | e | yes | 21.6 mm v2 F/B neck 0.2 | 7/13 |
| 230 | J1-CC1 | U7.1→J1.A5 | 0.2 | B 0.625x0.25 / B 0.3x1.45 | self-check: viahole:topor:193 gap 0.305 < 0.35 (pass 3) | i-viahole | yes | 5.0 mm v0 B | 3/13 |
| 232 | USB- | U7.4→J1.B7 | 0.2 | B 0.625x0.25 / B 0.3x1.45 | blocked: no path (pass 3) | e | yes | 2.9 mm v1 B | 4/13 |
| 234 | D23-A | D23.2→J1.A4 | 0.5 | B 1.2x0.9 / B 0.6x1.45 | blocked (pass 3) | b/a | no | 5.5 mm v0 B | 3/13 |
| 239 | J1-CC2 | U7.9→J1.B5 | 0.2 | B 0.625x0.25 / B 0.3x1.45 | self-check: viahole:topor:195 gap 0.305 < 0.35 (pass 3) | i-viahole | yes | (other tree) | 3/13 |

Counts: e 10, i-wall 8 (deep 4, < 5 µm 2, graze 2), i-pinch 6, i-via pair 5, i-item 2, i-viahole 2, g 1, g+b 1,
b/a 1. **34 of 36 route alone on the fixed copper** (DX's static classes b/a are down to 2); 33 of 36 route in at
least one of the 13 other runs (R2.3 (a)). Compared with round 1 (67 failed): the A4/NX/HC/A2/A6 classes are gone
(no NPTH hole case, no U7 static block, no 0.5 mm neck block except D23-A into J1.A4); what is left is interaction.

**Per-iteration profile (base, pass 1; new profile-only classes).** paths = negotiator-held paths, ok = metrized and
self-checked, mfail by `reason_class`, blocked by what the search last saw (`none` = no priced edge / site).

| it | paths | ok | blocked | mfail | mfail classes |
|---|---|---|---|---|---|
| 1 | 216 | 171 | 7 | 45 | pinch site-site 12, pinch pad-site 9, wall pad deep 5 / <5 µm 4 / graze 2, pinch pad-pad 4, check item 3, junction 3, sketch 2, pass cap 1 |
| 2 | 216 | 177 | 7 | 39 | wall deep 11 / <5 µm 4 / graze 2, pinch site-site 6, check item 5, pinch pad-pad 2, pad-site 2, junction 2, pass cap 2, … |
| 3 | 213 | 185 | 10 | 28 | pinch pad-site 8, wall deep 5 / graze 3 / <5 µm 1, pinch site-site 4, junction 2, sketch 2, pad-pad 2, pass cap 1 |
| **4 (best)** | 215 | **194** | 8 | 21 | pinch pad-site 6, pinch site-site 5, wall deep 5 / graze 1 / <5 µm 2, check item 2 |
| 5–9 | 212–216 | 182–186 | 7–11 | 26–33 | same mix; wall deep 1–5, pinch site-site 4–9, pad-site 2–9, check item 3–5, junction 1–3 |

Pass 2 (11 connections): 5 ok, 5 blocked, 1 sketch, stuck after 4. Pass 3 (26): best 19 ok, `check viahole` 2–3 in
every iteration (only in pass 3: against the small GND completion vias of pass 2), blocked 1–3. Stop rule: "no gain"
after 9 pass-1 iterations — correct (DX E2). Blocked connections are almost all `none` (the ghost found no priced
edge to blame): capacity-blocked, not site-blocked.

### R2.2 Lever 1: why A6 + A5 route less than A6 alone

| engine (same model, wall_clock 0) | routed / total | failed per pass (1 / 2 / 3 / 4) | vias | outside mm | detour ref max / > 1.5 | iterations |
|---|---|---|---|---|---|---|
| 3cddcc9 A6 only | 210/241 | 31 (30 / 0 / 1 / 0) | 175 | 1698.5 | 4.56 / 8 | 23 |
| a233d62 A6 + A5 | 205/241 | 36 (29 / 2 / 5 / 0) | 196 | 1802.2 | 4.22 / 8 | 29 |
| 789e050 base (+ A16, A13, A14) | 205/241 | 36 (29 / 2 / 5 / 0) | 196 | 1814.5 | 4.62 / 9 | 24 |
| base with `steiner: false` | 211/242 | 31 (30 / 0 / 1 / 0) | 178 | 1676.0 | 3.86 / 7 | 23 |

1. **A16 / A13 / A14 are neutral on dcsgone**: a233d62 and the base give the same counts (routed, failed per pass,
   vias); only lengths and the iteration count differ.
2. **The difference is the `steiner` switch**, and it is **not in pass 1**: pass-1 failures are 29 with A5 vs 30
   without (best pass-1 iteration: 194 vs 193 ok). The whole −5/−6 sits in the pour-completion passes (2 + 5 vs 0 + 1).
3. **Mechanism.** The different pass-1 copper (A5's junction trials and rejects — 1–3 `junction *` metrization
   failures per iteration, only 3 junctions survive: VCC D8.1→TP7.1, VCC D1.1→C16.1, GND C15.2→pour) cuts different
   GND items off the fill. A6-only cuts items in open areas (SW1.1, C21.2, R9.1, U1.1, C22.2, C20.2, …: all completed
   in pass 2, 7/11); with A5 the cut items are in the **USB corner** (U7.3, U7.8, J1.A1, J1.B12, C11.2, C15.2, D11.3).
   There pass 2 places 0.45/0.25 GND vias right in front of U7.3's and U7.8's tips ((91.73, 121.0) and (89.26, 121.0),
   0.98 / 1.9 mm from U7.4/U7.9) and fails U7.8 + C11.2; pass 3 then unfreezes 22 USB-corner neighbours and re-routes
   them against the frozen pass-1/2 copper: 5 fail — two of them (CC1 U7.1→J1.A5, CC2 U7.9→J1.B5) only because of the
   small-via **hole-clearance bug** (R2.3 (d) 2), the others in the same corner (USB- U7.4→J1.B7, D23-A D23.2→J1.A4,
   VCC C14.1→C15.1).
4. **Not implicated:** `net_rank` (does not change the routing order — pinned by A5's test; v2/v8 below reproduce
   the base exactly), the junction cost cap A-C21 (3 junctions in the result; the cap bounds only accepted Ts),
   "junction consuming a pad exit" (no final junction is at a fine-pitch pad), bridges vs Ts (the U7.4→U7.7 /
   U7.2→U7.9 bridges are routed in both engines).
5. **One systematic A5 effect: the post-passes are vetoed** (R2.3 (d) 5). With `steiner` on, the best snapshot does
   not re-metrize (one connection fails a null re-metrization), so via reduction removes 0 vias (A6 only: 20),
   via relaxation moves 0 (58), junction repair and zero-slack never keep anything. That is the vias jump
   175 → 196 and, through the extra vias, part of the different GND cuts. It does not touch pass 1's completion.
6. **Otherwise single runs are ±5 noisy.** The 13 runs of R2.3 differ only in ordering or one option, and give 29–46
   failed (205–219 routed). Apart from 5., the A5-vs-A6 completion difference is a trajectory effect (which GND items
   pass 1 happens to cut, and where), inside that spread. Any future A/B on dcsgone needs several orderings
   (R2.3 (a)), not one run.
7. **Task TE (U7 crop fixture, route_steiner red):** same corner (U7/J1) and the same A5 ⊕ A6 interaction (stubs and
   bridges change what the junction search sees there), but a different symptom: TE's fixture has no pour, my root
   cause on the whole board is the pour completion in that corner plus the via-hole bug. Fixing TE's tests will not
   by itself change these dcsgone numbers. For TE: `route_stubs::exit_stubs_off_matches_previous` (same USB crop,
   golden digest of 946530c) is **also red on the pristine base** (built from `git archive HEAD`, loaded machine;
   not in the coordinator's red list), and the veto of 5. may matter for the crop's junction tests too.

### R2.3 Levers by measurement

**(a) Variants (A-C10): deterministic net orders** (prototype `decompose::order_variant`, env-selected, worktree
only; the key replaces `order`'s `(width desc, est_len, net, id)` inside `(bumps, tier)`):

| v | order inside (bumps, tier) | routed / total | failed (1/2/3/4) | vias | detour ref max / > 1.5 |
|---|---|---|---|---|---|
| 0 | base: width desc, shorter first | 205/241 | 36 (29/2/5/0) | 196 | 4.62 / 9 |
| 1 | width desc, **longer** first | 206/240 | 34 (30/0/4) | 190 | 3.57 / 14 |
| 2 | shorter first, width ignored | = v0 | = v0 | = v0 | = v0 |
| 3 | width desc, seeded shuffle (seed 1) | 213/243 | 30 (23/0/7) | 217 | 4.23 / 12 |
| 4 | width desc, seeded shuffle (seed 2) | 211/252 | 41 (30/3/8/0) | 211 | 6.94 / 22 |
| 5 | width desc, **nets grouped in a seeded net order**, shorter first inside the net | **219/248** | **29** (20/2/7) | 201 | 5.29 / 10 |
| 6 | as 5, other seed | 210/244 | 34 (27/2/5) | 220 | 4.06 / 14 |
| 7 | the 36 base failures first, then v0 | 215/245 | 30 (22/3/4/1) | 208 | 2.55 / 11 |
| 8 | narrow first | = v0 | = v0 | = v0 | = v0 |
| 9 | pour connections after every pad-pad connection | 209/255 | 46 (32/7/7/0) | 192 | 12.4 / 12 |

(v2 / v8 equal v0: on dcsgone the width key never decides inside a tier.) **Union: 33 of the 36 base failures route
in at least one run**; only +5V U4.2→U4.4, U4.2→U4.19 (0 of 13 runs) and GND C11.2→pour never route; U4.2→C22.1
once. Best single variant v5: −7 failed (36 → 29) and +14 routed, at the price of more vias (+5) and a worse detour
max (5.3). Pass-1 failures range 20–32 by order alone. Cost: K runs (K = 4 in parallel at threads 1 each fits the
600 s budget on a quiet machine: one run is ~150 s at 4 threads, ~250 s at 1). Risk to eprom / wiccon: none if v0 is
always one of the variants and the pick is by (routed, routing_new, vias, length). Variants do **not** reach the
gate: best of 8 still fails 29.

**(b) Blake-style detour-optimisation rip-up (task DO): ≈ 0–3 failures freed.** Base has 9 routed connections with
reference detour > 1.5; only 3 of the 36 failures have one of them within 0.5 mm of their straight pad-to-pad line
(+5V U4.2→U4.19 ↔ D22-K 2.0×, I2C_SCL J5.4→U1.18 ↔ GND 1.8×, U4-STAT1 U4.8→R13.1 ↔ SPI_SCK 1.5×; mapping by item
`wire` index, 13 % of items unmapped — an estimate). Two of the three are better explained by (d) 1. A detour phase
is a quality lever, not a completion lever here.

**(c) Existing parameters** (single runs, base engine):

| option | routed / total | failed | note |
|---|---|---|---|
| `exit_stubs: "all"` | 196/239 | 43 | `wall deep` 4 → 1 but `sketch: path bends at a wall split vertex` 0 → 11 (stubs become walls the sketch cannot bend at); worse |
| `pour_listed_via_goals: true` (A-C21 asked for a dcsgone number) | 206/242 | 36 | neutral (+1 routed, 1 more completion connection); keep false |
| `allowed_drills_nm: [400000]` (no 0.45/0.25 vias) | 210/247 | 37 | `check viahole` gone (confirms (d) 2) but more completions; neutral |
| zero-slack (A4 `zero_slack_failed`) | — | — | by construction rescues at most **one** connection per run (the pass returns on the first success) and only single-leg, via-free paths; the two never-routing `< 5 µm` grazes (#20 156 nm, #105 548 nm) are multi-via hand routes → never candidates |

The ripup growth / pour-cut sweeps were left to GR. No A13 knob is implicated (0 failures name a via site shortage).

**(d) What the data shows beyond the knobs.**

1. **Vias in front of fine-pitch pad tips box in the neighbours (the dominant `e` mechanism).** U4 (0.5 mm pitch,
   0.25 mm pads): GND U4.3 → pour gets a 0.6/0.3 via at (105.64, 113.0), 0.5 mm beyond U4.3's tip; +5V U4.2 (between
   Q2-S U4.1 and GND U4.3, inner side closed by the GND EP) can then only leave west, where the via (copper edge
   0.1 mm from a 0.2 mm track at U4.2's row) and Q2-S's 0.5 mm track close it: **U4.2's three connections never route
   in 13 runs**, although each routes alone. Same at U4.8 (STAT1: a GND via in front of U4.9 and a STAT2 via in front
   of U4.7, 1.0 mm apart = 0.4 mm free < 0.6 needed), U4.12 (PROG3: an I2C_SDA via in front of U4.11), U7.4 / U7.9
   (the pass-2 GND vias in front of U7.3 / U7.8). The hand never does this: it bridges GND pads into the exposed pad
   or the neighbouring GND pad of the same footprint and puts one via away from the signal side — U4.3→U4.21 (EP),
   U4.9→U4.10, U4.11→U4.21, U5.1/U5.4→U5.9, U6.4→U6.9, U7.3→U7.8 (one 0.35 mm track across the IC, via at
   (89.0, 121.0) west of U7.8), J1.A1/B12. In the engine every one of these pads has its own pour via (0.8–0.9 mm
   long completions), 9 vias in front of fine-pitch rows. One of them is even `via_dangling` (GND (109.0, 110.14) in
   front of U4.9: the fill does not reach it).
2. **Via drill hole clearance is not in the graph / site model.** A router via is a vertex of radius d/2 with the
   copper clearance (`MultiGraph::activate` → `set_site(.., Some((net, d/2, clr, ..)))`, `ObstacleSet::set_site` sets
   `r_core = d/2`, `own_clr = clr`, no hole anchor); a model via (fixed copper of an earlier pass) gets a copper and a
   Hole anchor at the same point, but `RouteGraph::build` maps one CDT vertex to one anchor, so the graph keeps one of
   the two radii. For the default 0.8/0.4 via the copper term dominates (0.4 + 0.2 = 0.6 ≥ 0.2 + 0.35); for the
   0.45/0.25 pre-defined via (A10) the hole term wins (0.125 + 0.35 = 0.475 > 0.225 + 0.2 = 0.425): the search and
   taut use a corridor 50 µm too narrow, the self-check rejects it (`viahole:topor:193 (gap 305000 < 350000)`). Same
   class as round 1's NPTH bug (HC), for vias. 2–3 failures per pass-3 iteration, 2 final.
3. **Taut: the chord through a neighbour pad's core (`wall pad deep`, depth = w/2 + c + ε = 305 µm).** 1–11 per
   pass-1 iteration (5 at the best), 4 final (U1.2→R1.1 through U1.1, J3.17→R16.2 through J3.15, TP17.1→J3.2 through
   J3.1, U1.14→U7.5 through U1.13 — always the start pad's neighbour on a fine-pitch row). `taut::penetrations` adds a
   wall's end anchor only if that anchor covers the penetration point; a chord crossing the middle of a long thin pad
   (U1 0.9×1.5, J3 0.3×1.1) is reported "grazed mid-span" instead of being wrapped around the pad end on the slot
   path's side. The slot path itself passes the pad on one side (the search does not cross pads), so the sequence is
   missing that pad's end anchor.
4. **Sub-margin grazes** (`wall … <5 µm`, 1–4 per iteration, 2 final and both never-routing in other runs except
   2/13 and 4/13): the graze is below `margin_nm` (5 µm), i.e. DRC-clean at margin 0. A per-wall margin-0 acceptance
   inside taut (what zero-slack does for a whole connection) would keep them.
5. **With `steiner` on, every post-pass is a no-op: the best snapshot does not re-metrize.** Every post-pass
   (via reduction, via relaxation, zero-slack, junction repair) keeps a change only if *every* routed connection
   still metrizes. Null test (prototype: re-metrize the unchanged best graph from its own `MetrState` copy, nothing
   inserted): base **1 of 194 fails** — `I2C_SCL` (live #117, `infeasible: pass cap`) — with `steiner: false`
   **0 of 193**. That one non-reproducible connection vetoes every candidate: base via reduction "removed **0** of 194
   vias in 85 candidates", via relaxation "moved **0** of 159", junction repair 0 of 29 — in 11 of the 14 steiner-on
   runs of this round (A6 + A5, base, v2–v9, pourvia); with `steiner: false` (or A6 only) via reduction removes 19–20 and relaxation moves 58–71
   vias (−33 mm). This is most of A5's vias jump (175 → 196) and a share of its pour cuts (more vias → more GND
   islands). **It is not junction-specific:** with `steiner: false` the v5 and v7 orderings also fail the null
   re-metrization (v5: 1 of 197, `wall pad:R3:1 grazed 23 µm`; v7: 4 of 197 — pad-pad pinch, a 395 µm deep wall, a
   self-check item, the same R3 graze) and their via reduction / relaxation are 0 as well. The defect is that the
   metrization of the best snapshot is **not idempotent** (the loop's `metrize` with its incremental `MetrState`
   accepted these wires; a fresh `metrize(.., work = [])` from a copy of the same state rejects them); steiner merely
   triggers it in the default order. `drill04` (no 0.45/0.25 vias) happens to avoid it (via reduction −22), `v1` and
   `stubs-all` partly (−7). Taut's pass cap / wall grazes are the symptoms; candidates for the cause are state
   carried in `MetrState` that a fresh run recomputes (junction points, chain crosses, stack orders, zero-slack walls).
6. **Generalised repair (prototype): +9 to +11 once nothing vetoes it.** After the loop of every pass, every
   non-permanent failure is searched once more in the best iteration's graph with strict unfit edges (the
   junction-repair machinery with all failures as candidates; kept only if every routed connection still metrizes):
   - steiner on (`rep2`, `rep3` empty history, `rep4` + depth-2 re-route of the broken neighbour): 206/241, i.e. only
     pass 3's U7.4→J1.B7; instrumented, 10 of 29 pass-1 failures are blocked in the final graph (U4.2 ×3, U4.8, U4.12,
     J3.17, J5.4, U1.13, J1.A4, D11.1) and 19 find a path, 14 of which metrize themselves — and **all 14 "break" the
     same connection, #117**, the one the null test fails (5. above): an artefact, not congestion; the depth-2 retry
     of #117 fails for the same reason.
   - steiner off (`rep2ns`, proxy for "5. fixed"): **9 of 30 pass-1 failures repaired** (VCC D15.1, D11.1, +5V U4.17,
     +3V3 R1.1, I2C_SDA R22.2, USB- U7.4, USB+ J1.A6, D23-A D23.2, CC2 J1.B5), result **216/241, failed 25**
     (21 / 0 / 4 / 0), vias 186, outside 1778.9 mm, detour ref max 3.86, > 1.5: 7 (base: 9) — the best single
     variant-0 run of this round. The 11 found-but-rejected candidates fail their *own* metrization: wall grazes
     (deep 305 µm ×2, 1 µm ×2, 14.6 and 153 µm), taut pass cap, sketch wall split vertex, a J3 pad-pad pinch.
   - stacked with the best orderings (steiner off + repair): v5 216/249 (failed 33, 1 repaired), v7 207/247
     (failed 40, 0 repaired) — **vetoed again**: both orderings fail the null re-metrization (5. above), so these two
     runs say nothing about repair + variants; they only confirm the veto is general.

### R2.4 Ranked fix list

Ids are base final ids (table R2.1). Estimates are against the 36 base failures; classes overlap.

1. **FR — Reproducible re-metrization + generalised repair** (R2.3 (d) 5–6). *Mechanism:* the best snapshot's
   metrization is not idempotent (default order with steiner, and other orders without); one connection failing the
   null re-metrization vetoes via reduction, via relaxation, zero-slack and junction repair. Then extend `junction_repair_pass` to every non-permanent failure
   (strict unfit edges in its search, kept when everything still metrizes). *Recovers:* measured with steiner off as
   a proxy (`rep2ns` vs `base-nosteiner`): **+9 repaired in pass 1**, net 211/242 → **216/241**, failed 31 → 25
   (newly routed: VCC D15.1→C15.1, +5V U4.17→TP9.1, +3V3 R1.1→R16.2, I2C_SDA R22.2→U5.8, USB+ J1.A6→J1.B6, CC2
   J1.B5→R5.2 = base #205, plus three completion / re-route connections; lost to the changed copper: USB+ U7.6→J1.A6,
   CC2 U7.9→J1.B5, two GND completions), and the revived reduction / relaxation (−19 vias, −33 mm, already in
   A6-only). *Files:* `route.rs` (`metrize` / `refine_junctions` / `record_chain_cross` idempotence,
   `junction_repair_pass` candidates + params, a baseline "null" metrization the post-passes compare against
   instead of "every routed connection", so one flaky connection cannot veto), `taut.rs` if the pass cap is the
   symptom. *Effort:* M (S for the repair generalisation, M to find the non-idempotence). *Risk:* low for eprom /
   wiccon (they have no junction veto today: check via reduction counts unchanged); repair adds ≤ one search + one
   incremental metrization per failure, bounded, deadline/cancel per candidate. Options: `[search] repair_failed`
   (default true once measured), strict-unfit in repair as an `Expert` field. *Test:* a model whose best snapshot is
   re-metrized twice gives identical results (null test as a unit test on the dcsgone USB crop with steiner on);
   dcsgone via reduction removes > 0 vias with steiner on; repair recovers ≥ 1 on a crop where a failure has a free
   corridor in the final graph.
2. **FF — Fine-pitch fan-out: bridge pour pads inside the footprint, no via in front of a neighbour's tip**
   (R2.3 (d) 1). *Mechanism:* each GND pad of a QFN / DFN gets its own pour via 0.5–1 mm in front of its tip, and
   other nets' vias land in the same front row; adjacent signal pads are then boxed in. The hand bridges GND pads into
   the EP / the neighbouring GND pad of the same footprint and puts one via away from the signal side. *Recovers:*
   +5V #74 U4.2→U4.4, #75 U4.2→U4.19, #76 U4.2→C22.1 (0 of 13 runs route the first two), #178 U4.8→R13.1, #169
   R8.2→U4.12 directly (5); in the USB corner the pass-2 GND vias at U7.3 / U7.8 disappear (U7.3–U7.8 bridged, hand
   style), which frees U7.4 / U7.9 escapes (#188, #232, #239, #230) and removes the 0.45/0.25 vias of the viahole
   failures: ≈ 5 direct + 2–4. *Files:* `decompose.rs` (pour edges: one pour connection per same-footprint group of
   pour-net pads joined by A6 pad bridges ≤ `pad_bridge_max_mm`, an EP preferred as the group's pour terminal),
   `stubs.rs` (bridge planner already exists), `vias.rs` / `multigraph.rs` (optional second half: a via site within
   one pitch row in front of a fine-pitch pad is not offered to another net while that pad's connection is unrouted,
   or front-row sites are staggered). *Effort:* M. *Risk:* medium — changes the decomposition of pour nets; wiccon has
   a GND pour (vias ≤ 37 bound, outside ≤ 1380 mm); pour-reach must count a bridged pad as reached through its group.
   Option `[layers] pour_pad_bridges` (default true if wiccon / eprom not worse). *Test:* a dcsgone U4 crop: U4.3,
   U4.9–U4.11 joined to U4.21 with no via in front of them, U4.2→U4.4 routes on B.Cu; pour-reach unchanged.
3. **FG — Geometry-model consistency trio** (each S, one task). (a) **Via drill hole clearance** in the graph and the
   metrization anchors (R2.3 (d) 2): `MultiGraph::activate` / `ObstacleSet::set_site` add a Hole anchor (or the
   vertex radius becomes `max(d/2 + clr, drill/2 + hole_clearance)`), `RouteGraph::build` takes the larger of
   co-located anchors. Recovers #230 CC1 U7.1→J1.A5, #239 CC2 U7.9→J1.B5 (+ 2–3 per pass-3 iteration). (b) **taut
   deep wall** (R2.3 (d) 3): when the chord crosses the core of a pad wall that is not a member, add the wall end on
   the slot path's side (σ from the search) instead of `grazed mid-span`; recovers #96 U1.2→R1.1, #98 J3.17→R16.2,
   #184 TP17.1→J3.2, #190 U1.14→U7.5 (+ ≈ 5 per pass-1 iteration). (c) **sub-margin grazes** (R2.3 (d) 4): a wall
   grazed by less than `margin_nm` is accepted at margin 0 for that wall (what zero-slack does per connection); #20
   D11.1→Q2.3 (156 nm), #105 J4.3→J6.2 (548 nm). *Files:* `multigraph.rs`, `obstacle.rs`, `graph.rs`, `taut.rs`.
   *Risk:* (a) strictly more conservative near small vias (wiccon / eprom use none: check); (b)/(c) touch the M2
   guard `eprom_ripped_front_only_is_the_m2_result` (run it). *Test:* unit tests per item (a 0.45/0.25 via with
   hole_clearance 0.35: no corridor at 0.43 mm from its centre; a chord through a 0.3×1.1 pad's middle wraps its end;
   a 1 µm graze is accepted and self-checks clean at margin 0). ≈ 8 direct.
4. **Variants (A-C10, task VR)** after 1–3: K orderings, best by (routed, routing_new, vias, length), v0 always in.
   Measured on the base: best of 8 orders −7 failed (v5 nets grouped in a seeded net order: 219/248, failed 29);
   the stacking with the repair could not be measured (both stacked runs are vetoed, R2.3 (d) 6) — re-measure after
   FR. Use v0, v3, v5, v7 as the first K = 4 (largest
   spread of outcomes; v2/v8 duplicate v0 on dcsgone). Cost K runs; threads 1 per variant inside the 600 s.
5. **Small, independent:** (i) `decompose`: a signal net's copper polygon (`copper_shapes` net ≠ 0) becomes a
   terminal — IR_RX and +3V3 islands, 2 `unconnected_items` with no failed connection; (ii) harness
   `drcloop`: `wall_clock_s 0` must mean "unlimited" for the DRC loop too (one line); (iii) pour completion in a
   congested corner (#208 C11.2 never routes in 13 runs, #212 U7.8): after FF, re-measure; GR's pour-cut sweep is
   the other lever. Not levers: Blake detour rip-up (≤ 3, R2.3 (b)), `exit_stubs all`, `pour_listed_via_goals`,
   the via drill set, iteration / stop-rule knobs (DX E2).

**Gate reachability.** Not with ordering or existing parameters: the best of 13 runs still fails 29, and the best
prototype combination measured (repair, steiner off) fails 25. But the data does not show a connection the board
cannot carry: 34 of 36 failures route alone on the fixed copper, 33 of 36 route in at least one run, and the two
never-routed pad pairs (U4.2) have a specific cause (FF). The gate (241/241 with completions, `unconnected 0`) needs
**FR** (a repair step that is not vetoed, i.e. reproducible metrization — the capability the current post-passes
assume but do not have with junctions), **FF** and **FG**, then variants as the multiplier; whatever stays after
those is joint congestion that needs a multi-connection repair (rip one neighbour, re-route both — the depth-2
prototype is in the patch, untestable today because of the veto). Suggested dispatch: FR first (it also unblocks
measuring every later fix), FF and FG in parallel.
