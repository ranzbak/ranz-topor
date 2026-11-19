# last-preview: the progress dialog shows the last round's result (and the best so far on a toggle)

Branch `last-preview` off main 12699012. User 2026-11-19: "What ever the result is, I would like the last round result
to be displayed." No code before the lead's reply.

## What exists today (checked in the code)
- `control::Progress` (the only thing `RouteJob.progress()` sends) is scalars only: phase/stage, round, best_* and
  last_* scores, round log lines. **No geometry of any round reaches the plugin**; the only geometry is the final
  `RouteResult` from `wait()` (the pick = the best).
- **The progress dialog has no board image at all** (ui_wx.ProgressDialog: phase, gauge, numbers, Last round / Best so
  far rows, time limit). The only board image is the post-run PreviewDialog (two kicad-cli SVG exports of the
  current board and the result Accept writes = the best).
- With the default `[optimize] loop = "topor"` (draw once) a run is one "round" holding the TopoR loop; nothing is drawn
  until the loops end. Per loop cycle the engine reports only the archive pick's *predicted* criteria
  (`on_pick` -> `LiveLoop` -> Best so far). The **Last round row is filled only when a drawn round ends**
  (`Coord::ended` with `Judged`), i.e. at the end of a run. So during a long loop both rows and the round number look
  static: the user's complaint is real, and it is the score rows as well as the (missing) image.

## Proposal (engine: read-only hooks, no routing change; plugin: new image panel)
Engine
1. `Sketch` (control.rs): `{run, cycle, kind: "plan"|"copper", crit, wires: [{net, layer, pts}], vias: [{at, d}],
   finished_s}` in the KiCad frame (nm). `kind = "plan"`: a loop cycle's state: per alive wire its stretches per
   layer (pass-point polylines, `State::polyline` + layering), planned vias as dots; not drawn copper.
   `kind = "copper"`: a drawn round's items (the variant the round was judged by).
2. `RunControl` gets `sketches: Mutex<{last, best, seq}>` plus two flags: `enabled` (set once by the plugin at job
   start, `RouteJob.enable_sketches()`) and `want` (set by `RouteJob.request_sketch()`, cleared when filled).
   Headless/harness/MCP never enable it: zero work there.
3. Hook: `Space::after_cycle(&mut self, cycle, is_pick)` (default no-op) called by `optimise_in` after `on_pick`;
   core's impl builds the *last* sketch only when `want` is set (<= 1 build per plugin poll interval) and the *best*
   sketch when the cycle's state is its run's pick now (new best; only while `enabled`). `Coord::ended` stores a
   drawn round's items as `copper` sketches (last; best when "new best"). Best over runs = `shown_best`'s run.
4. Progress gains `loop_run` / `loop_cycle` (the TopoR loop cycle that finished last, any run) and `sketch_seq`;
   `RouteJob.sketch("last"|"best") -> Option<str>` (JSON).
Cost: building is O(wires + passes) (O(items) for copper), <= 1 per 2 s + 1 per new best; the per-cycle check is one
relaxed atomic load. The hooks only read the state, so copper is unchanged; tested by result-JSON equality with
sketches enabled vs not (fixture + eprom) and copper hash on esp/wiccon.

Plugin
5. ProgressDialog: a board image under the score rows, a **Last round / Best so far** toggle (radio, keyboard), a caption
   naming what is shown ("Last round: run 2, loop cycle 37 - plan sketch, not drawn copper" / "Best so far: run 1,
   cycle 12 ...") and "last round finished 14:02:11, cycle 37 (12 s ago)" (the plugin timestamps each change of
   `loop_cycle` / `rounds_ended`), so a long round shows as long. The Last round score row also follows the loop
   cycle (predicted criteria, marked "predicted") instead of staying empty until the end.
6. The base image (outline, pads, existing copper, pours) is drawn once per run from the model already in the plugin,
   in-process with wx (no kicad-cli, no DRC); the overlay is redrawn only when `sketch_seq` changes, at most every
   poll interval (constant 2 s; option only if you want one).
7. `[run] progress_preview = "last" | "best" | "off"`, default "last" (which one the dialog shows first; the toggle
   switches; off = no image, no sketches requested). Schema + docs/user/configuration.md, `since = V5` per since-loud.
8. Post-run PreviewDialog unchanged (it shows what Accept writes, the best). Decision for the user if he meant that
   preview: showing a last round there needs "Accept writes that round", an engine/output change, out of scope.

## Tests
Rust (release, targeted): sketch JSON shape and frame; `want` cleared after one build; best sketch = the pick's cycle;
result JSON equal with sketches on/off (fixture, rounds=1). Mutants on the new lines. Python: option default/choices/
off, progress handling (timestamps, seq changes, caption per kind, toggle), sketch-to-draw-list pure functions.
GUI e2e (Xephyr harness): esp-rust-board, two screenshots of the progress dialog in different loop cycles, the image
differs and the caption names the cycle; default copper = main (copper hash). Hetzner: one small job (<= EUR 1).

## Addendum 2026-11-19: OK writes the highlighted one of the four bests (user decision; NOT implemented, lead's reply first)
User: the score panel lists the 4 kept bests (overall, fewest errors, shortest + fewest errors, fewest vias + fewest
errors) as clickable rows; a click highlights the row and shows that variant in the image; default highlight = overall;
OK writes the highlighted variant. Last round / Last run stay (image toggle), not offered for OK unless one of the 4.

What exists (checked in the code, main 12699012):
- The 4 bests are the roles of the ONE variant table all runs share (`archive::Role::ALL`, `table.rs`); during the loop
  they are **plans** (predicted), nothing is drawn (draw once).
- At the end `Coord`/`DrawGate::assign` draws the rows holding the roles (`[drawing] candidates` = 4, default), then
  `core.rs four_bests` ranks the drawn rows by the same roles and `[optimize] pick` chooses which goes to KiCad.
- **After Cancel / OK only the overall best is drawn** (`DrawGate::why_not`: "after Cancel / OK only the overall best of
  the variant table is drawn"). With the dialog the run only ends by OK / Cancel / cap / time (no stop on stable), so in
  the plugin the other 3 bests are normally **never drawn**.
- The result carries the 4 bests' scores (`RouteResult.bests`, JSON: roles, run, predicted + drawn criteria) but the copper
  of the applied one only; the other drawn rows' `RouteResult`s are dropped in `four_bests`.
- `Progress` has no per-role rows; the plugin sees only the overall best (and since this branch Last round / Last run).
So: the 4 bests do **not** all reach the plugin, and only the overall one can be written on OK today.

Proposal (engine + plugin; no routing change, the search is the same):
E1. Progress `bests: [{role, run, cycle, errors, open, violations, vias, length_mm, kind}]` = the shared table's 4 role
    rows (updated where the table changes; O(4)). A row holding several roles is one entry with several roles.
E2. Role sketches: when a role's row changes and it is the publishing run's own end state or a stashed mid state (the
    same rule as the run-pick sketch here), publish it to a role slot; `RouteJob.sketch("best:errors")` etc. Rows offered
    by another run's table copy come with that run's sketch (each run publishes its own rows). Only while enabled.
E3. OK carries the choice: `RouteJob.cancel(ok, role="best"|"errors"|"shortest"|"vias")` (default "best" = today).
    After OK the engine draws the row holding that role (one drawing, as today for the overall best) and applies it
    (as `[optimize] pick = role` does). Cost after OK = one drawing, unchanged.
E4. Without OK (cap / time limit / headless) all candidates are drawn as today; the result additionally carries the other
    drawn roles' items (`alternatives: [{roles, items}]`, plugin-only, skipped in result.json) so the post-run Preview
    can switch between them and Accept writes the chosen one.
P1. Score panel: rows Overall / Fewest errors / Shortest / Fewest vias (+ Last round, Last run below, not selectable for
    OK unless they are one of the four); click / keyboard highlights; the image follows the highlight; caption names
    role, run, round, predicted vs drawn; the OK button's tip names what it writes ("OK writes: Shortest, run 3 round 41").
P2. Post-run Preview: the drawn copper of the chosen role (the drawn result may differ from the prediction: shown as
    predicted -> drawn); switching there (E4) only where several were drawn.
Open questions for the lead/user: (a) after OK, draw only the highlighted row (fast, E3) or all four (+3 drawings
of waiting) so the Preview can still switch? (b) If the highlighted plan draws worse than predicted (e.g. errors appear),
write it anyway (user's choice) with a warning, or fall back to the overall best? Proposal: write it + loud warning.
