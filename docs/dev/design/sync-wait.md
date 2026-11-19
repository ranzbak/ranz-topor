# sync-wait: the variant table without lock-step waits

Branch `sync-wait` (off main 524465c2). Evidence: round-slow report (scratch/agent-wt/reports/round-slow-20261119-1045.md).

## Why the barrier exists
TW3 (docs/dev/design/tw3-table.md, commit 11eba44a): at a restart a run reads T(c-1), the commit of every
run's offers of cycles <= c-1, so its restart row is a function of the seeds (copper the same on any thread
count). Reading T(c-1) means waiting until every live run has finished cycle c-1: a lock-step in cycles.
Cycle times vary 12-34 % (CV) inside one run and the 5th run of runs = 5 on 4 slots gets its first slot only
when another run waits, 42-113 s late; every barrier then waits for it. Measured: 33-37 % of loop-thread time
in the barrier, qmtech -18 % cycles/min.

## What changes
1. **Bounded lag (stale-synchronous view).** At cycle c a run reads T(c-1-L), L = `[optimize] table_lag`
   (cycles; 0 = today's lock-step, copper = main; shipped default 0, lead 2026-11-19: the A/B of 0/1/2 decides
   a flip to 2). It waits only when another live run is more than
   L cycles behind it. Commits stop at `min(live done) - L` (no live run will read an older table); a reader
   ahead of the slowest run builds its view by replaying the held offers of cycles (committed, c-1-L] on a
   copy of the committed state (archive, gain/restart series, accepted set; rows are `Arc`s), so the view is
   T(c-1-L) exactly. Everything the view carries (rows, own accepted offers, the other runs' fingerprints,
   stable) is taken at c-1-L.
2. **Fair slots.** The slots go to the waiting run with the lowest key (not begun < loop cycles done < run
   index), and a run gives its slot at each cycle boundary to a waiting run with a lower key. 5 runs on 4 slots
   then share the slots round-robin (no late start), a barrier wait hands the slot to the run waited for, and a
   run that yielded (stable or not) gets a slot back as soon as it is the lowest (today: only when someone
   waits). Replaces `yield_to_first`. Copper-neutral: a run's cycles depend only on its state and its views.
3. **Per-cycle log.** One line per loop cycle to the plugin log.txt (Progress ring of 256 lines, written once
   each like the round lines): run, cycle, wall since the call start, cycle s, table wait s, slot wait s,
   predicted errors, pick value, restart source. Loop end: a warning line with the slot idle share (slot-time
   free while a loop runs, measured in `Slots`).

Literature: Polubasov 2010 («TopoR изнутри», section «Многовариантная оптимизация», next to fig. 13): the
descents are independent, so they can run "on different cores or on different computers in a local network",
i.e. loosely coupled, not in lock-step. Bounded staleness: Ho et al. 2013, "More effective distributed ML via a
stale synchronous parallel parameter server" (NIPS 26, 2013, pp. 1223-1231; section 2, the SSP model: a worker at
clock c sees every update of clocks up to c-s-1, and waits only while the slowest worker is more than s clocks
behind). It matches the wait rule; the difference is that SSP may show newer updates too ("at least"), here the
view is exactly T(c-1-L), which keeps it deterministic. (Not in docs/literature: page range from the NIPS
proceedings, not checked against a copy here.)

Simulated on the round-slow per-cycle logs (main r5, 4 slots, 300 s loop, sync points as logged):

| board | main (lock-step) idle / cycles | fair L=0 | fair L=1 | fair L=2 |
|---|---|---|---|---|
| esp r5 | 22.9 % / 148 | 12.3 % / 175 | 4.7 % / 188 | 2.6 % / 192 |
| qmtech r5 | 25.0 % / 45 | 1.8 % / 60 | 0.0 % / 62 | 0.0 % / 62 |
| badge r5 | 17.0 % / 111 | 9.9 % / 123 | 0.8 % / 140 | 0.2 % / 140 |
| esp r4 (4 slots) | 15.6 % / 165 | 15.6 % | 6.5 % | 2.7 % |

(idle = free slot-time / (4 x loop span); the sim reproduces main's measured cycles within 3-9 %.)

## Invariants
- The view at cycle c is T(c-1-L) for any thread count, slot count and arrival order (L fixed, seeds fixed).
- No deadlock: the run with the fewest finished cycles never waits at the table and has the slot priority.
- L = 0 gives main's views exactly (copper = main at fixed rounds).
- Yields never past the time limit (as today); the stop checks bound every wait as today.

## Tests (targeted, rule 14c)
- table.rs: replay view = T(c-1-L) under shuffled arrival and 12 seeded random interleavings per lag of a 3-run toy, L = 0..3
  (mutants: commit past `min done - L`; view without replay; lag off by one).
- optimise.rs threaded toys: same table/restarts at 1/2/4 threads and slots with L = 2; L = 0 = main's.
- Slots: priority order, cycle-boundary turn, a yielded run gets its slot back, no lane lent while one waits.
- Board: copper hash twice and at 1/2/4 threads (esp, rounds fixed); ccx33 timing slots esp/qmtech r5:
  idle share and cycles/min main vs tip; quality at 600 s wall per connection (ladder_diff).
- Plugin: cycle lines written once each, gap said once (pytest of run.py).

## Lead decisions (2026-11-19)
- Q1: implement `table_lag`, ship default 0 (= main copper); A/B L = 0/1/2 (each with fair slots) at equal 600 s
  wall on esp + qmtech, seeds 0-4 (bulk VM), per connection; the report proposes the flip to 2 with evidence.
  Timing (idle share, cycles/min) only from ccx33 slots; also the idle share of L = 0 + fair alone.
- Q2: fair rotation without an option, provided it is copper-neutral: copper hash = main at L = 0 on 4 boards
  and the same copper at 1/2/4 threads (= slots) in a test.

## Measured (2026-11-19, report scratch/agent-wt/reports/sync-wait-20261119-1400.md)
- Copper: tip at L = 0 equals main (524465c2 and 7ae8024c) on eprom, wiccon, esp and qmtech at fixed rounds. L = 0
  and L = 2 each give the same copper at 1, 2 and 4 threads.
- Slot idle share on a ccx33 timing slot (runs 5, 600 s wall), for L = 0 / 1 / 2:
  - esp: 10.9-11.2 % / 3.7 % / 1.6-3.1 %;
  - qmtech: 3.9-5.4 % / 1.9 % / 2.5-3.8 %.
  - Main's table wait was 35-36 %.
- Cycles/min on these slots depend on the neighbour slot's load (shared hyperthreads), so they are not comparable
  between runs that did not overlap.
- On esp, L = 2 runs fewer cycles than L = 0, each with about twice the predicted errors: staler restart rows.
- Quality at an equal 600 s wall (5 seeds, per connection): main, L = 0, 1 and 2 are level within seed noise.
- Proposal: default L = 1. It is not flipped in this branch.
