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
