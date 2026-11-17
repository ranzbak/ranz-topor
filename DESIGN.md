# par-refine: parallel, incremental candidate evaluation in `refine` (design note; no product code)

Base main 74ff26a (file:line = main). Evidence: profile-74ff26a stacks split by me (% of ALL samples, qm-s1 / esp-s0 / bh-det)
and a throw-away recording run of every candidate (§7). Long version + tables: reports/par-refine-design-*.md.

## 1. One candidate, and the state it reads / writes
Stream: `refine` (refine.rs:779) walks over-full edge-layers (820-834), `moves_around` (860) builds 1-3 label moves per event
(869-884) and calls `try_move` (887) until the first that lowers the price. A candidate = `try_move_in` (924-1021): set labels
(939), snapshot wires (953), `relayout` (624) = `default_ks`/`rank`/`site_taken` (477/428/442) + `apply` (503) + `reprice`
(564) + `optimise` (589: per via a sweep over its interval, `apply`+`reprice` per traversal, float sum `run`, key rounded
609); delta from the undo log (976-986); keep (992) or restore (998-1019). `optimise` is ~80 % of a candidate (33.1 / 21.0 /
69.6 of 41.4 / 27.0 / 83.2 %); `eval` (659) 41.6 / 26.5 / 62.0, of it `Packer::bounds` 28.7 / 18.7 / 42.6, margin cache
19.1 / 13.2 / 28.0, SipHash 13.1 / 9.8 / 20.4, malloc/free 14.4 / 8.4 / 21.8. Grain 100-800 us per candidate.
READS: `evl` of its wires; `pass_layer`/`pass_disc` of EVERY pass on each repriced edge (`items` 636), `cost`/`sq` there,
`via_k` of other wires in triangles `site_taken` checks (448), own `via_iv/via_k/nosite`; static `State`, `dirs`, Packer
config; memo caches (pure functions, RefCell): `Packer.cache` (pack.rs:193), `free_cache` (203), `end_sites` (refine.rs:317).
WRITES (kept on accept, restored on reject): `evl` labels, the wires' pass layers/discs, `via_iv/via_k/nosite`, repriced
`cost/sq`, cut-model state (default `capacity_model = "edges"`, model.rs:1510: out of scope v1), `meter` (thread-local OFF).

## 2. Parallel with the SAME accepted sequence: ordered speculation
I1 (tested): a rejected candidate leaves every non-memo field bit-identical. I2: verdict and floats = f(state, move) only.
A cursor replays the serial generator (sweep, list, pass, event, move, incl. the `cost<=0` / `max_evals` checks 811/827/887)
ASSUMING REJECTION and feeds a lane pool in stream order. Lanes evaluate on the read-only master through a private overlay
(§3) and return {verdict, exact written values}. The run thread consumes results in stream order; the FIRST accept is
committed by copying its values (bit copies), the cursor advances as serial does (`break` of the event, `improved`), all
later in-flight results are dropped, the stream restarts from the new master. Proof by induction: before that accept,
serial evaluates exactly these candidates on exactly this state (I1), so (I2) same verdicts, same first accept, same
committed values; `evals` counts like serial. No cross-lane float reduction (each delta summed in one lane in today's
order: BTreeMap 976, sorted edges 565); no HashMap iteration; no time input. Lane count never enters the result. W = 1 is
the serial path (one implementation; the old try/undo stays as test reference). Deterministic reservations (keep results
whose read set misses the accept's write set) bought <= 6 % in §7: not worth its generator proof.
Literature: Müller, Radke, Vygen 2011 (docs/literature/mueller-radke-vygen2011-resource-sharing.pdf pp. 9-11 §6):
BonnRoute's parallel resource sharing evaluates block solvers concurrently on shared prices, validates each against the
prices in a fixed order and rejects/recomputes violators; 5.4-7.1x on 8 threads (Table 2, p. 22). Their order is an atomic
time stamp (nondeterministic); fixing it to the serial index is Blelloch-Fineman-Gibbons-Shun's deterministic reservations
(PPoPP 2012 §4) and deterministic parallel PathFinder (Gort & Anderson, FPT 2010) — both not in docs/literature, pages unchecked.

## 3. Incremental evaluation, data layout (all exact: representation / memo only)
- Lane overlay, not replicas: epoch-stamped dense arrays per lane over passes (stamp u32 + layer + disc) and wires (via
  data), small sorted Vec for touched `cost/sq`; clear = epoch+1; 24 B x passes per lane, allocated once per run (qmtech
  15 751 passes = 0.4 MB, bornhack 26 264 = 0.6 MB). No per-call model copy (qmtech cost/sq alone = 3.4 MB x 5617 calls).
- Per-pass static item data (net, w, s, infl) in flat arrays at `build`; per-pass end/disc margins lazily in a
  `Vec<AtomicU64>` (NaN = unset; racing fills store the same bits): `bounds` stops hashing, `items` stops allocating.
- `bounds` into scratch (pack.rs:574-608 builds 5 Vecs/call); reuse `apply`/`optimise` buffers (`ks.clone()` 605).
- `reprice` evaluates only layers whose item list changed (layer move: old+new; disc: all): skipped layers would return
  equal values and log nothing (573), so it is exact.
- Remaining caches Sync: sharded `Mutex<FxHashMap>` (inline FxHasher, no new crate); `trans`/`unary` -> `&(dyn Fn + Sync)`
  (layers.rs:800, 1152). `build`'s all-edges initial `eval` (395-399) is pure per (e,l): parallel with ordered writes.
- Later, measured first: (edge, layer, 128-bit content hash) -> (cost, sq) memo across `build`s (the loop re-layers almost
  the same state each try, and `measure`, core.rs:2957, builds a 2nd model per try).

## 4. Cores: runs and lanes share one budget
Today one std::thread per run (core.rs:1183), `Slots` = min(runs, rayon threads) compute (core.rs:417-470, 611). New: `Slots`
sized to the rayon pool (RAYON_NUM_THREADS = the threads option, schema.py:375); a computing run holds 1 slot and, per
refine call, `try_take`s spare slots as lanes without waiting (none while a run waits) and returns them. runs=5 on 4-8
cores (KiCad default): no spare, serial path, no rayon call; lanes appear for runs=1, runs < cores (VM 16 vCPU) and the
tail when runs finish. Option `[optimize] refine_lanes` (auto = spare slots, 1 = off) to pin it for benchmarks.

## 5. Cancel / time limit
`stop_requested()` is THREAD-LOCAL (control.rs:660, 691): pool threads never see it. The run thread checks it before each
dispatch and before each commit; a stop drops in-flight results uncommitted, so a parallel run equals a serial run stopped
at a candidate boundary. Lanes get an `&AtomicBool` checked between via positions. Gap = one candidate (0.1-0.8 ms), as
today; it keeps limit-wait's loop-scope check (branch limit-wait 4fd997e, refine.rs:887); no new wait.

## 6. Tests (VM)
1. proptest: random small models + candidate streams, lanes {1,2,3,8,16} vs old `try_move`: same verdict sequence, same
   final bits (`pass_layer/disc`, `via_k`, `cost/sq`, `evl`); I1 on every reject; overlay vs direct reads.
2. Plans: eprom / wiccon / esp s0 / qmtech s1, runs=1, RAYON 1/4/16, lanes forced 1/2/8: per-refine-call hash of the
   accepted sequence + final copper hash = main 74ff26a cached baseline; ladder_diff --json 0 deltas.
3. Mutants: commit best not first accept; keep results after an accept; stale cursor; overlay ignores `via_k` in
   `site_taken`; lane deltas in completion order; reprice skips a disc layer; stop only at refine start.
4. Timing: VM --exclusive, <= 8 threads; esp s0 / qmtech s1 / bornhack s0, runs=1, lanes 1/2/4/8, 3 repeats (TOPO_TIMES
   loop s, move_cost, wall, CPU); runs=4 at RAYON 8.

## 7. Evidence, gain, risks
Recording (throw-away par-refine-exp 17e6cae, shared VM, runs=1, 1 thread; plan unchanged: esp 2313/54088/3518 in 2 runs),
replayed as a lane pool (in-order commit, 5 us dispatch, work started before an accept is seen = wasted):

| board | refine calls | cand/call | accepted | cand s | build s | pool speedup T2/T4/T8/T16 | work eff T4/T8 |
|---|---:|---:|---:|---:|---:|---|---|
| esp s0 | 2313 | 23.4 | 6.50 % | 29.1 | 19.4 | 1.76/2.74/3.63/4.10 | 0.80/0.68 |
| bornhack s0 (rounds=1) | 44 | 44 684 | 0.02 % | 515.9 | 5.6 | lockstep 1.65/2.59/3.98/6.24; pool ~T | 1.00/1.00 |
| qmtech s1 | 5617 | 412 | 0.48 % | 263.5 | 187.8 | 1.85/3.47/5.98/8.98 | 0.87/0.77 |

qmtech reproduces main exactly (2 322 868 moves, as the profile's runs=1 run). Gain (Amdahl: lanes on the candidates +
parallel initial eval, which is 168 s on qmtech / 16 s on esp over all builds incl. `measure`): qmtech router 1042 →
~735 s at 4 lanes (1.42x), ~680 s at 8 (1.53x); bornhack P2..loop 637 → ~270 s (2.4x); esp 195 → ~165 s (1.18x). So
"~2x" holds for bornhack-like streams; on qmtech the rest is `build` (188 s in refine + `measure`'s second build per try):
its per-wire via `optimise` is a commit-always sequence (needs reservations, v2) or an incremental build (§3 memo).
Under `timeout_clock = cpu` the waste costs CPU seconds (13 % qmtech, 20 % esp at 4 lanes).
Risks: missed write in a lane result = silent divergence (tests 1-2 in CI); Sync caches cost a little serially; HT beyond 8
lanes; cut model stays serial; KiCad default runs=5 sees almost no gain.

## 8. Cheap single-thread items first (exact, ~3-5 days)
FxHash (SipHash self 16.6 / 12.8 / 20.4 %), scratch buffers (malloc 10.5 / 7.1 / 10.4 %), per-pass item/margin tables
(margin 19.1 / 13.2 / 28.0 %), reprice changed layers only: all inside the candidate, they cut serial AND lane work and
remove the RefCell blockers, so they come first and the parallel factors above become upper bounds. New from the
recording: every `build` re-evaluates all 215k edge-layers (qmtech: 168 s, 16 % of the run): skip pass-less edges and
copying unchanged edges from the previous model (content memo, exact) is likely cheaper than parallelising it (measure). `free_radius` index
(9.5 / 3.4 / 0 %), `in_piece` precompute (0 / 0 / 8.8 %), search arrays (esp expand 11.7 %): outside refine, they shrink
the serial part. Order: items 1-4 → re-measure → lanes → parallel initial eval → outside items.
