# Task A3 report (F): design note — capacity / Steiner / stubs / G1 / via relaxation / rip cost

Status: DONE. Note only, no engine code. Worktree branch `worktree-agent-a66da99f8e58495c2` from main 68a77b6.

## What was written
- `docs/progress/notes/m4-capacity-steiner-design.md` (new, ≈ 3 pages + 6 tables): §1 own-net / same-net capacity with
  exact `fits_for` / `overuse` formulas (`E_v`, `K_v`, `g(x)`, wall term `W_n(e)`), worked numbers for the six diagnosis
  cases + the U2.28 outline case + the width-wider-than-pad class, the tests that flip; §2 Steiner goal `OwnStrand`,
  slot-adjacency rule, 60° angle, sibling split sharing `Terminal::Junction`, cascade on rip, bounded 120° junction
  relaxation, `net_rank` ordering (byte-identical to today's order), batch-mode re-search rule; §3 exit stubs
  (`Terminal::Stub`, end formula `h + c + w/2 + eps`, auto/all/off semantics, Checker pre-pass, infeasible rule); §4 G1
  continuation (ray-walk topology prefix, tangent rail discs, `g1_angle_deg` ≤ 1 verification, three mode values);
  §5 bounded Weiszfeld via relaxation (formula, star confinement argument, acceptance rule, pass position); §6 rip cost
  escalation with the scale table (geometric sum vs `hist_fail·k` vs cost factor), ping-pong / three-way settlement
  argument, saturation check of the 2.0 / 8 defaults, bound and report struct; §7 complexity table, test-per-rule
  list (A4–A6, A13, A14, A16 names from the plan), options table (13 keys with default / range / cost if wrong), expected
  effect per board.
- This report.

## Decisions and why (each a configurable in §7 of the note)
1. Capacity: M3 end term kept verbatim for foreign ends (byte-identity where no own-net case exists); own ends charge 0
   gap and credit `r_v`; a foreign strand on an own-end edge keeps its gap (conservative). Wall term only on edges with
   an own end, cached `walls_near` at build (`O(E·q)`), per-search evaluation on the net's own vertices only; "longest
   free piece" semantics (exact for one wall). Reason: F6 broke the eprom guard exactly through a non-end wall.
2. Steiner: junction stays a `Terminal::Junction` (fixed for the taut loop, moved only by the bounded relaxation pass) —
   no new terminal kind for smoother / writer / self-check. Attachment needs slot adjacency (Rule T) + angle ≥ 60°;
   cascade rip of attached connections. Batch mode: ≤ 1 re-search per connection per iteration (reusing the existing
   sequential re-search path) instead of serialising all multi-pad nets.
3. Stubs: `auto` keyed on the post-§1 unfit test (so SOT-23 / THT pads get none, eprom stays identical); stub body is a
   kept-copper wall of the net; infeasible stubs are reported, never silently shrunk.
4. G1: the plan's rail formula (`± n̂(w/2 + c)`, radius `w/2 + ε`) is over-constrained with the net clearance and has
   0.09 mm play (≈ 10°) as clearance-0 segments; redefined as two tangent discs (`ρ_rail = w/2 + max(c, R_min)`,
   clearance 0, per-wire like `ZeroSlack`) giving exact tangency. Mode values: recommend `continue | fixed | drop`
   (schema 0.4.0 has `continue | drop`, plan A14 has `continue | fixed`) — needs a ruling.
5. Via relaxation: confinement `star(v, L1) ∩ star(v, L2)`, acceptance = `via_fits` + both legs re-metrize + self-check
   + gain ≥ 1 µm; pour / locked vias skipped; one sweep in site order.
6. Rip cost: formula as 46 row 17; the scale check shows 2.0 / 8 is consistent (3× alternative settles at k = 3, 10× at
   k = 5, saturation at k ≈ 7); overshoot bounded by `is_detour` + the stop rule; `growth 1 / max 3` = M3.

## Numbers used
- 0.3.0 committed dcsgone 60 s run: 148/231, 83 failed = 45 timeout, 16 blocked no path, 8 infeasible (wall/pad/site),
  5 self-check hole gap, 3 inconsistent slot path, 6 other; overuse_edges 192; wiccon 62/62 vias 37 outside 1373.55 mm
  (6.4 mm headroom to 1380 — flagged as the stub risk); eprom 27/27 64.628 mm. F6 measurements from the M3 diagnosis
  (wiccon vias 33 → 30, outside 1357 → 1352; eprom identical; guard 26/27) reused as the expectation for §1.
- No bench run by me (DX measures; Ruling A-C3 dcsgone runs cost ~10 min); no PNG (note-only task).

## Tests
None to run (no code). `git status` clean apart from the two files.

## Concerns
- `dangling_ends` value set conflict (schema vs plan A14): ruling needed before A14.
- Wiccon outside-length headroom (6.4 mm) vs stubs on `auto`: A6 must report the delta; if the bound is exceeded,
  escalate — never relax.
- Steiner changes eprom's result (two 2-connection nets): A5 acceptance is "not worse", identity is not claimed.
- Commit attribution: global-constraints.md says "attribution line of the executing session's system reminder"
  (Claude Fable 5.1), which differs from the worker-contract text (Opus 5.5); the session reminder line was used.
