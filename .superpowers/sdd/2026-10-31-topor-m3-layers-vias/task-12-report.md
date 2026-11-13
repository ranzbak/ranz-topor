# Task 12 (Steps 1-4) report

Merged worktree-agent-a141224bcf3a2052b (fast-forward). Changes: version 0.3.0 (engine/Cargo.toml, Cargo.lock, plugin.json, SHIPPED_VERSION, plugin/requirements.txt pin which test_scaffold also checks);
synth_model(layers=1|2) + scaling parametrised over layers (vias == routed asserted for 2); tests/gate/test_m3_gate.py verbatim to spec (+ wiccon prints signal-only ratio; private-badge-b test also asserts_unchanged);
M2 gate threads assertion -> `== r["threads"]`; 45-mvp-plan.md §5a "Layer assignment" row reworded (M3-R1). rep["connections_total"] = reports.len(), which includes pour-completion connections (verified in route.rs): M3-R5 stricter denominator holds.

Build: tools/build.sh --dev fails in a worktree (no .venv/bin/maturin); used the explicit maturin command. test_version_matches_cargo green.

## Scaling
- layers=1: FAIL: t10 0.0146 t100 0.5459 t1000 17.69 s (999 routed, 25 iters); t100/t10 37.48 (<=40 ok), t1000/t100 32.40 (<=30 FAIL). (2nd run 31.8)
- layers=2: PASS: t 0.108 / 2.245 / 48.40 s, routed 10/100/1000, iters 1/2/4; ratios 20.87 / 21.56.
## Gates (11 passed, 2 failed of 13)
- M3 eprom PASS: GATE M3 eprom: routed 27/27 nets 25/25 vias 0 layers {B.Cu 17773415, F.Cu 90323030} outside-pads 64.628 (hand 76.598) mm drc_new 0 unconnected 0 arcs 13 router_s 0.095 iters 1 zero_slack_wires 0 via_reduction 'removed 0 of 0 vias in 0 candidates'
- M3 wiccon FAIL: routed 50/60 (all, incl pour completion) = 83 %, signal-only 48/57 (84 %), vias 26, drc_new 0, unconnected 10, router_s 3.536, iters 15, zero_slack 0.
- M3 private-badge-b FAIL: routed 160/231, failed 71, drc_new 0, router_s 120.218 > 65.
- M2 eprom (threads assertion updated) PASS: 27/27, 64.628 vs 76.598, router_s 0.095.
- M2 wiccon info: 50/60 drc_new 0 3.8 s. M2 private-badge-b info PASSED: routed 0/231, router_s 60.037 (M2 test uses default options; the router routes nothing in 60 s there).
- M0/M1 gates pass (M1 eprom outside-pads 71.494, drc_new 0).
## Other suites
- cargo test --release: all pass; clippy -D warnings clean.
- default pytest: 129 passed, 2 failed before the requirements pin fix; after: test_scaffold::test_cargo_workspace_builds still fails: it runs `cargo test -p topor-engine` in DEBUG profile; route_engine::partial_on_emits_two_checked_stubs and partial_off_emits_nothing_for_blocked_connection fail in debug (pass in release; probably deadline/probe-cap timing: reason "partial stub failed self-check"). Not caused by this task's edits (not touched by the version bump); needs a coordinator decision.
