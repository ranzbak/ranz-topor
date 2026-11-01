### Task FR (opus, rotation 2): reproducible re-metrization of the best snapshot + generalised repair (DG R2.4 item 1)

Read rotation-2-common.md (incl. "Round 2 update") and docs/progress/notes/m4-dcsgone-diagnosis.md R2.2 item 5, R2.3 (d) 5–6, R2.4 item 1.
Problem: the best iteration's snapshot does not re-metrize (a fresh `metrize(.., work = [])` from a copy of the same MetrState
rejects a wire the incremental loop accepted: base 1 of 194 — I2C_SCL #117 `infeasible: pass cap`; with steiner off the v5/v7 orders
also fail it). Every post-pass (via reduction, via relaxation, zero-slack, junction repair) keeps a change only if every routed connection
still metrizes, so that one wire vetoes them all: via reduction removes 0 of 194 vias (A6-only: 20), relaxation moves 0, repair 0.
Do:
1. Find the non-idempotence by root cause (candidates per DG: state carried in MetrState that a fresh run recomputes — junction points,
   chain crosses, stack orders, zero-slack walls; taut pass cap as the symptom). Fix it so metrization of an unchanged snapshot is
   idempotent. Unit test: the null re-metrization of the best snapshot reproduces it (dcsgone USB crop with steiner on, and a
   whole-board debug assertion/probe on dcsgone for the four orders), and dcsgone via reduction removes > 0 vias with steiner on.
   If full idempotence is not reachable, ALSO make the post-passes compare against a null re-metrization baseline (a connection that
   already fails the null run cannot veto a candidate) — say which you did and why.
2. Generalise `junction_repair_pass` to every non-permanent failure (DG prototype TOPOR_DG_REPAIR in the patch: search in the best
   graph with strict unfit edges, keep if everything still metrizes), bounded (≤ one search + one incremental metrization per failure,
   deadline + cancel per candidate). Options: `[search] repair_failed` (default true if measured a gain) and the strict-unfit choice as an
   Expert field; schema + docs regenerated. Optional if time allows and measured: depth-2 repair (rip one neighbour, re-route both — in
   the patch), behind its own option, default by measurement.
Expected (DG, steiner-off proxy): +9 repaired, dcsgone ~216/241, via reduction back (≈ −19 vias, −33 mm). eprom/wiccon not worse (their
via-reduction counts must not drop). PNG `--task FR --board dcsgonebadge`. Commits `fix(route): idempotent re-metrization of the best
snapshot`, `feat(route): repair pass for every failed connection`. Report task-FR-report.md.
