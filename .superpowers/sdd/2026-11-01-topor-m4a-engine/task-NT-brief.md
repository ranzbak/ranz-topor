### Task NT (opus, rotation 2): net-tie short (drc_new 1 on the M2/M3 private-badge-b 60 s info tests) + connection_width

Read rotation-2-common.md first.

Symptom: `.venv/bin/pytest tests/gate/test_m2_gate.py tests/gate/test_m3_gate.py -m "gate or not gate" -q -s` → the private-badge-b 60 s info
tests report drc_new 1 (routing_new must be 0, M3-R13). A5 report concern 3 (task-A5-report.md): private-badge-b `shorting_items` JP1.1 ↔
Net-(Jx-Pin_n) in wall_clock-0 runs: pads of a net-tie group are exempt for the OTHER net's tracks (obstacle.rs / check.rs pad
`tie_nets`, pinned by `obstacle_model::same_net_and_net_tie_are_exempt`), so the engine lets a track of net B touch the tie pad of
net A; KiCad flags it. Also "connection_width (+5V via neck, also on main)" — check whether that is a second router DRC item.
Steps: (1) reproduce: read the routed_drc of those gate runs (which violation type/items exactly — confirm it is JP1 and not
something else); (2) fix by root cause: the net-tie exemption should apply only to the tie footprint's own copper / the footprint's
net-tie pad groups as KiCad defines them (KiCad allows only copper of the tied nets INSIDE the net-tie footprint to touch; a track of
net B may touch pad A only... verify against KiCad 10 DRC semantics with kicad-cli on a small board copy, not from memory); update
obstacle_model test to the correct semantics with the KiCad evidence in its comment (not weakened: the exemption that IS legal stays
tested, the illegal one gets a new test); (3) if a `connection_width` violation is router copper (neck below a KiCad
connection_width rule / netclass min), fix the neck width rule (A-C9: neck ≥ KiCad min track width incl. netclass/rules).
Acceptance: M2 + M3 gate private-badge-b info tests drc_new 0; full gate dir `pytest tests/gate -m "gate or not gate" -q -s` for M2/M3 green;
private-badge-b gate options routing_new 0 and not fewer routed than base (204/241); eprom/wiccon unchanged or not worse; cargo + clippy.
Commit `fix(obstacle): net-tie exemption only for the tie footprint's own nets` (adapt). Report task-NT-report.md.
