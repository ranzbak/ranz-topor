//! The TopoR loop on the topology (shopping list 5.1–5.3, 5.5; roadmap P5
//! `optimise(topology, archive) → archive`): after the initial routing, cycles that change only the
//! topology (no drawing inside the loop, no drawing feedback), each topological variant judged by
//! the plan's predicted criteria and kept in an archive of variants; the picked one is drawn once
//! afterwards (`[optimize] loop = "topor"`, [`super::core`]).
//!
//! Literature (TopoR):
//! * Polubasov 2010 (`docs/literature/polubasov2010-topor-inside.html`, «Оптимизация разводки»):
//!   "the main mechanism of the optimisation is removing the wires one at a time and laying them
//!   again along cheaper paths"; the price of a path is `S = Σ kᵢ xᵢ` over about a hundred factors;
//!   "the coefficients are not fixed: first, they are chosen automatically for every wire being
//!   rerouted (depending on the quality of its path and its surroundings), and second, they
//!   oscillate within some limits" ([`k_osc`]). The evaluation of a whole topology uses only the
//!   factors obvious to the designer, with constant coefficients ([`Criteria`]).
//! * Same, «Оптимизация расслоения»: layer re-optimisation per pair of layers, "the layering should
//!   be chosen at random" among the equal ones ([`super::layers::DomainGraph::solve_ties`]); the
//!   wires whose via count rose are rerouted next; a reroute counter keeps a wire from being
//!   rerouted too often (the `reroute` factor); its time stays below 50 % of the whole.
//! * Same, «Процедура всплытия»: floating up, one wire and the stretches it crosses to another
//!   layer (here: the label descent, [`super::descent::descend_labels`]).
//! * Same, «Многовариантная оптимизация»: the variants are kept; "TopoR periodically stops working
//!   on the current variant and picks a variant from the table at random for further
//!   optimisation"; every variant has a generation (it descends from another one).
//! * TopoR 6.3 manual (`docs/literature/eremex2017-topor63-user-manual.pdf` pp. 72–73): four
//!   criteria (violations, vias, length, shrinkages); a variant superseded when a new one is better
//!   on all of them; the length is that of the unsmoothed connections (10–15 % over the final).
//! * Luzin & Polubasov 2006 (`docs/literature/luzin-polubasov2006-eda-expert-topor.pdf` p. 75):
//!   "every optimisation cycle is split in two: at its start more attention goes to the total
//!   length, at its end to the number of vias" ([`Emphasis`]).
//!
//! The loop ([`optimise`]) is generic over a [`Space`] (the topology with its moves), so it is
//! tested on its own; one cycle of the real space is in [`super::core`]. The single-wire move is
//! [`reroute_set`].
//!
//! Reproducible: every choice comes from ChaCha8 streams seeded by the run's seed and the cycle;
//! the stops that change the result are work caps (cycles) and the stable test (no gain over a
//! window of cycles), never time. The time limit is only an emergency stop (reported).
//!
//! Complexity: a cycle is what the space does (one search per wire plus one layer assignment);
//! an offer to the archive is O(archive).

use super::archive::{Archive, Criteria, Objective, Variant};
use super::schedule::{Meter, Schedule, Score};
use super::state::{State, WireId};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

const EPS: f64 = 1e-6;

/// The factors that stand for vias while the topology is planned: the via itself and a crossing
/// of two nets (the layer change P3 makes for it). The cycle's [`Emphasis`] scales them.
pub const VIA_FACTORS: [&str; 2] = ["via", "crossing"];

/// The emphasis of one cycle (Luzin & Polubasov 2006 p. 75: length first, vias later in each
/// cycle): the via-standing factors ([`VIA_FACTORS`]) are weighted `via_start` at the start of the
/// cycle's sweep and `via_end` at its end, graded by a smoothstep in between; length keeps its
/// weight. `via_start` = `via_end` = 1: no emphasis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emphasis {
    pub via_start: f64,
    pub via_end: f64,
}

impl Default for Emphasis {
    fn default() -> Self {
        Emphasis { via_start: 0.5, via_end: 1.5 }
    }
}

impl Emphasis {
    /// The via weight at the fraction `t` (0 start, 1 end) of the cycle. O(1).
    pub fn via_at(&self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        (self.via_start + (self.via_end - self.via_start) * s).max(0.0)
    }
}

fn mix(a: u64, b: u64) -> u64 {
    // SplitMix64 finaliser of a ^ golden-ratio multiple of b.
    let mut z = a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The per-wire coefficient kᵢ of factor `factor` for connection `conn` at its `visit`-th try
/// (Polubasov 2010: the coefficients oscillate within limits, per wire): `1 + amp x sin(φ + 2π x
/// visit / P)` with the phase φ and the period P (3 to 7 tries) drawn per (seed, connection,
/// factor). `amp` = 0 gives 1. In `[1 - amp, 1 + amp]`, clamped at 0. O(1).
pub fn k_osc(seed: u64, conn: usize, factor: usize, visit: u32, amp: f64) -> f64 {
    if amp <= 0.0 {
        return 1.0;
    }
    let h = mix(mix(seed, conn as u64 + 1), factor as u64 + 1);
    let phase = (h & 0xFFFF) as f64 / 65536.0 * std::f64::consts::TAU;
    let period = 3.0 + ((h >> 16) % 5) as f64;
    (1.0 + amp * (phase + std::f64::consts::TAU * f64::from(visit) / period).sin()).max(0.0)
}

/// The amplitude of a wire's coefficients (Polubasov 2010: chosen per wire "depending on the quality
/// of its path"): the cycle's width `amp`, widened smoothly for a wire whose tries keep failing to
/// gain (`stuck` fruitless tries in a row): `amp x (1 + stuck / (1 + stuck))`, at most twice `amp`.
pub fn wire_amp(amp: f64, stuck: u32) -> f64 {
    let s = f64::from(stuck);
    amp * (1.0 + s / (1.0 + s))
}

/// What one cycle of the loop gets.
#[derive(Clone, Copy, Debug)]
pub struct CycleCtx {
    pub cycle: usize,
    /// Seed of this cycle (from the run's seed and the cycle number).
    pub seed: u64,
    /// Width of the coefficients' oscillation (the fluctuation schedule, [`Schedule`]).
    pub amp: f64,
    pub emphasis: Emphasis,
}

/// What one cycle did (for the report).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CycleStats {
    /// Single-wire reroutes tried / kept in the sweep.
    pub tried: usize,
    pub kept: usize,
    /// Wires whose via count rose in the layer re-optimisation, rerouted again / kept.
    pub relaid: usize,
    pub relaid_kept: usize,
    /// Layer re-optimisations, those taken (the board's evaluation did not rise) and their time
    /// (s; report only).
    pub layer_runs: usize,
    pub layer_kept: usize,
    pub layer_s: f64,
    /// Floating-up (label) moves kept.
    pub float_kept: usize,
    /// Crossing-rule passes kept (clinches, double crossings, unlayerable crossings).
    pub cross_kept: usize,
    /// Whole-board moves (floating up, crossing passes) undone because the board's evaluation
    /// rose ([`evaluation`]).
    pub undone: usize,
    /// The cycle's time (s; report only).
    pub busy_s: f64,
    /// 5.4 `[vias] pair_min`: via pairs removed by the pair lift (Luzin-Popov 2013), and wires
    /// with a via pair laid again round what they dived under / kept.
    pub pair_lifts: usize,
    pub pair_relaid: usize,
    pub pair_relaid_kept: usize,
    /// 5.8 `[vias] pair_move`: via pairs whose crossing wires were laid again round the stretch
    /// between the vias (Luzin-Popov 2013 step 8.1) / kept (the board's evaluation dropped), and
    /// the wires those tries laid again.
    pub pair_moved: usize,
    pub pair_moved_kept: usize,
    pub pair_moved_wires: usize,
    /// Kept moves whose stretch was longer than the `[vias] pair_max_mm` scale (its graded price
    /// grows with the square of the length over that scale; warned).
    pub pair_moved_long: usize,
}

impl CycleStats {
    pub fn add(&mut self, o: &CycleStats) {
        self.tried += o.tried;
        self.kept += o.kept;
        self.relaid += o.relaid;
        self.relaid_kept += o.relaid_kept;
        self.layer_runs += o.layer_runs;
        self.layer_kept += o.layer_kept;
        self.layer_s += o.layer_s;
        self.float_kept += o.float_kept;
        self.cross_kept += o.cross_kept;
        self.undone += o.undone;
        self.busy_s += o.busy_s;
        self.pair_lifts += o.pair_lifts;
        self.pair_relaid += o.pair_relaid;
        self.pair_relaid_kept += o.pair_relaid_kept;
        self.pair_moved += o.pair_moved;
        self.pair_moved_kept += o.pair_moved_kept;
        self.pair_moved_wires += o.pair_moved_wires;
        self.pair_moved_long += o.pair_moved_long;
    }
}

/// The topology the loop works on: its current state, a copy of it, its predicted criteria and one
/// optimisation cycle.
pub trait Space {
    /// A topological variant (what the archive keeps).
    type Var: Clone;
    /// The predicted criteria of the current state (from the plan, nothing drawn).
    fn criteria(&mut self) -> Criteria;
    /// A copy of the current state.
    fn snapshot(&mut self) -> Self::Var;
    /// Make `v` the current state.
    fn restore(&mut self, v: &Self::Var);
    /// One optimisation cycle on the current state; `stop()` true = end it early (time limit).
    fn cycle(&mut self, c: &CycleCtx, stop: &mut dyn FnMut() -> bool) -> CycleStats;
}

/// An archived topological variant: the state, its generation (the variant it descends from plus
/// one; TopoR's "generation number", Polubasov 2010), the cycle it came from and when (s after
/// the loop started; report only).
#[derive(Clone, Debug)]
pub struct Gen<V> {
    pub var: V,
    pub gen: usize,
    pub cycle: usize,
    pub at_s: f64,
}

/// Parameters of [`optimise`].
#[derive(Clone, Copy, Debug)]
pub struct LoopParams {
    /// Most cycles (a work cap: `[optimize] rounds`).
    pub cycles: usize,
    /// Cycles without a gain of the pick after which the loop goes on from a variant of the
    /// archive picked at random (0 = never).
    pub restart_after: usize,
    pub seed: u64,
    /// The coefficients' oscillation width: start, narrowing per cycle, cycles without gain while
    /// incomplete before a reheat, reheat width (the M-B0 schedule).
    pub fluctuation: f64,
    pub narrowing: f64,
    pub stuck: usize,
    pub reheat: f64,
    /// Stable: the pick gained less than `stable_gain_pct` % over `stable_window` cycles; the loop
    /// ends `stable_patience` cycles after that.
    pub stable_window: usize,
    pub stable_gain_pct: f64,
    pub stable_patience: usize,
    pub emphasis: Emphasis,
    pub obj: Objective,
}

/// What [`optimise`] did.
#[derive(Clone, Debug, Default)]
pub struct LoopReport {
    pub cycles: usize,
    /// Restarts: (cycle, label of the variant it went on from).
    pub restarts: Vec<(usize, String)>,
    /// Variants offered / taken into the archive (the initial plan included).
    pub offered: usize,
    pub archived: usize,
    /// Seconds after the start when the first variant of a loop cycle was archived.
    pub first_variant_s: Option<f64>,
    pub stable_at: Option<usize>,
    /// Why it ended: "stable", "cycle cap" or "time limit".
    pub stop: String,
    /// The pick's objective value after every offer (the initial plan first): never rises.
    pub pick_values: Vec<f64>,
    pub stats: CycleStats,
    /// One line per cycle.
    pub lines: Vec<String>,
}

/// The TopoR loop: offer the current state (generation 0), then up to `p.cycles` cycles; after
/// each, the state's predicted criteria are offered to `arch` (a copy taken only when no archived
/// variant covers it). After `p.restart_after` cycles in which the pick did not get better the
/// loop goes on from an archived variant drawn at random (seeded). Ends at stable, the cycle cap or
/// when `stop()` before a cycle (the time limit, Cancel; then the report says so). `inner()` ends
/// a running cycle early (M2.TL: Cancel only with the soft limit); a cycle it cut short is not
/// offered (Cancel keeps the best variant archived so far). The caller restores the pick.
pub fn optimise<S: Space>(sp: &mut S, arch: &mut Archive<Gen<S::Var>>, p: &LoopParams, stop: &mut dyn FnMut() -> bool, inner: &mut dyn FnMut() -> bool, clock: &dyn Fn() -> f64) -> LoopReport {
    let obj = p.obj;
    let mut rep = LoopReport { stop: "cycle cap".into(), ..LoopReport::default() };
    let mut rng = ChaCha8Rng::seed_from_u64(mix(p.seed, 0x5EED_7090));
    let mut sched = Schedule::new(p.fluctuation, p.narrowing, p.stuck, p.reheat);
    let mut meter = Meter::new(p.stable_window, p.stable_gain_pct);
    let score = |a: &Archive<Gen<S::Var>>| -> Option<(Score, f64)> {
        let c = a.items[a.pick(&obj)?].crit;
        Some((Score { incomplete: c.errors, shorts: c.shorts, ranked: c.ranked(), value: obj.value(&c), scale: obj.base(&c) }, obj.value(&c)))
    };
    let offer = |sp: &mut S, arch: &mut Archive<Gen<S::Var>>, label: String, gen: usize, cycle: usize, rep: &mut LoopReport| -> bool {
        let crit = sp.criteria();
        rep.offered += 1;
        let kept = if arch.items.iter().any(|o| o.crit.covers(&crit)) {
            arch.offered += 1;
            false
        } else {
            let var = sp.snapshot();
            arch.offer(Variant { label, crit, payload: Gen { var, gen, cycle, at_s: clock() } }, &obj)
        };
        if kept {
            rep.archived += 1;
        }
        kept
    };
    offer(sp, arch, "initial plan (gen 0)".into(), 0, 0, &mut rep);
    let Some((s0, v0)) = score(arch) else { return rep };
    meter.push(s0);
    rep.pick_values.push(v0);
    let mut gen = 0usize;
    let mut since_gain = 0usize;
    for cycle in 1..=p.cycles.max(1) {
        if stop() {
            rep.stop = "time limit".into();
            break;
        }
        if p.restart_after > 0 && since_gain >= p.restart_after && !arch.items.is_empty() {
            let i = rng.gen_range(0..arch.items.len());
            sp.restore(&arch.items[i].payload.var);
            gen = arch.items[i].payload.gen;
            rep.restarts.push((cycle, arch.items[i].label.clone()));
            since_gain = 0;
        }
        let ctx = CycleCtx { cycle, seed: mix(p.seed, cycle as u64), amp: sched.width, emphasis: p.emphasis };
        let cs = sp.cycle(&ctx, inner);
        rep.stats.add(&cs);
        rep.cycles = cycle;
        if inner() {
            // cut short inside (Cancel): not a finished cycle, not a variant
            rep.stop = "time limit".into();
            break;
        }
        gen += 1;
        let kept = offer(sp, arch, format!("cycle {cycle} (gen {gen})"), gen, cycle, &mut rep);
        if kept {
            rep.first_variant_s.get_or_insert_with(clock);
        }
        let (sc, v) = score(arch).expect("the archive holds the initial plan's line");
        rep.pick_values.push(v);
        let gained = meter.push(sc);
        since_gain = if gained { 0 } else { since_gain + 1 };
        sched.after_cycle(gained, sc.incomplete > 0);
        if meter.stable() && rep.stable_at.is_none() {
            rep.stable_at = Some(cycle);
        }
        rep.lines.push(format!(
            "{cycle}: amp {:.3}, reroutes {}/{}, via-rose {}/{}, layerings {}/{}, floating up {}, crossing passes {}, undone {}, {}{}, best {:.1}{}",
            ctx.amp,
            cs.kept,
            cs.tried,
            cs.relaid_kept,
            cs.relaid,
            cs.layer_kept,
            cs.layer_runs,
            cs.float_kept,
            cs.cross_kept,
            cs.undone,
            if kept { "archived" } else { "not archived" },
            rep.restarts.last().filter(|r| r.0 == cycle).map(|r| format!(" (restart from {})", r.1)).unwrap_or_default(),
            v,
            if gained { " (new best)" } else { "" }
        ));
        if rep.stable_at.is_some_and(|s| cycle >= s + p.stable_patience) {
            rep.stop = "stable".into();
            break;
        }
    }
    rep
}

/// The predicted violations of a topology with its layering, as the loop's archive counts them
/// and as every loop move pays for them (shopping list 5.2: the price of a move and the criterion
/// of the archive are one measure). `count` = the archive's errors: over-full edge-layers (cut
/// model: over-full cuts) and vias without a site ([`super::refine::CapModel::violations`]),
/// crossings no layer separates, connections placed without a legal path. `graded` >= `count`:
/// each of them 1 plus how far it is over (an unlayerable crossing and a forced connection 1),
/// the `violation` factor's own amount (1 per violation plus its depth, `factors/violation.rs`).
/// Polubasov 2010 («Пространство решений»): the evaluation of a whole topology uses "only the
/// factors obvious to the designer ... with constant coefficients"; the violations are the first
/// of them, so their weight here never oscillates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Predicted {
    pub count: usize,
    pub graded: f64,
    /// Shopping list 2.4 (`[capacity] model = "cuts"`): the necked stretches the cut model's
    /// necks add to the plan's own (the archive's shrinkages count them), and the shrinkage price
    /// of the cuts that fit only necked (mm, `[factors] shrinkage`; part of [`Predicted::price`]).
    /// 0 for the per-edge model.
    pub shrink_n: usize,
    pub shrink_mm: f64,
    /// Of `count`: the predicted shorts, crossings no layer separates and traversals through
    /// copper of another net (both a short when drawn: KiCad `shorting_items` /
    /// `tracks_crossing`; a traversal of a no-tracks rule area is a violation, no short); the
    /// archive's [`Criteria::shorts`] of a plan. 0 with `[optimize] pick_shorts` off.
    pub shorts: usize,
    /// `[optimize] short_weight` minus 1 ([`Criteria::short_extra`]); 0 with `pick_shorts` off.
    pub short_extra: f64,
}

impl Predicted {
    /// From the capacity model's `(count, graded)`, the unlayerable crossings and the forced
    /// connections. O(1).
    pub fn of(cap: (usize, f64), unlayerable: usize, forced: usize) -> Predicted {
        let n = unlayerable + forced;
        Predicted { count: cap.0 + n, graded: cap.1 + n as f64, shrink_n: 0, shrink_mm: 0.0, shorts: unlayerable, short_extra: 0.0 }
    }
    /// Plus the traversals through foreign copper `(count, graded, shorts)` ([`foreign_copper`]:
    /// the ones through a no-tracks rule area are violations but no shorts). O(1).
    pub fn with_foreign(self, f: (usize, f64, usize)) -> Predicted {
        Predicted { count: self.count + f.0, graded: self.graded + f.1, shorts: self.shorts + f.2, ..self }
    }
    /// The same prediction with the shorts ranked at weight `k` (`[optimize] short_weight`, at
    /// least 1) when `on` (`[optimize] pick_shorts`); off: not ranked (they stay in `count` and
    /// `graded`, as before 2026-11-12). O(1).
    pub fn shorts_ranked(self, on: bool, k: f64) -> Predicted {
        Predicted { shorts: if on { self.shorts } else { 0 }, short_extra: if on { k.max(1.0) - 1.0 } else { 0.0 }, ..self }
    }
    /// With the cut model's shrinkages `(stretches added, price mm)` (2.4). O(1).
    pub fn with_shrink(self, n: usize, mm: f64) -> Predicted {
        Predicted { shrink_n: n, shrink_mm: mm, ..self }
    }
    /// What the violations cost a move (mm): the `violation` weight `viol_mm` x the graded amount,
    /// so one more predicted violation costs at least `viol_mm` and one fewer gains as much.
    pub fn price(&self, viol_mm: f64) -> f64 {
        if self.shrink_mm == 0.0 {
            viol_mm * self.graded
        } else {
            viol_mm * self.graded + self.shrink_mm
        }
    }
    /// The archive's graded part ([`Criteria::penalty`]): the amount beyond 1 per violation at the
    /// `violation` weight (the count itself is [`Criteria::errors`]).
    pub fn penalty(&self, viol_mm: f64) -> f64 {
        viol_mm * (self.graded - self.count as f64).max(0.0)
    }
}

/// Planned traversals through foreign copper (Q14b): a wire whose traversal of a triangle can
/// lie on no layer that copper of another net (a pad, an existing via, a copper shape; or a
/// no-tracks rule area) leaves free there, under the layering `layers` ([`layers_now`]: the layer
/// of the run, both layers inside a via interval, so a via that can still go on the free side is
/// no violation). In the drawing such a wire is a short (KiCad `shorting_items`), so the plan
/// counts it as a violation like an over-full cut: costly, not failed (the wire stays placed and
/// the loop's moves can take it out). One per wire and object it runs through; graded like the
/// `violation` factor (`factors/violation.rs`: 1 + depth x length per 0.01 mm²) with the wire's
/// pitch (width + clearance, the whole overlap) as the depth over the length of its traversals
/// inside the object (straight chords between its passes). Returns `(count, graded, shorts)`:
/// `shorts` = the traversals of copper of another net (a pad, a via, a copper shape: KiCad
/// `shorting_items` when drawn); a no-tracks rule area (`net = None`) is in `count` and `graded`
/// but no short (KiCad reports it as a rule-area violation, not as a short).
/// Polubasov 2010 («Многовариантная оптимизация»): "a design-rule violation cannot be compensated
/// by any reduction of wire length". O(traversals x obstacles per triangle).
pub fn foreign_copper(st: &State, layers: &[Vec<u64>]) -> (usize, f64, usize) {
    let pr = st.proj;
    let all = if pr.n_layers >= 64 { u64::MAX } else { (1u64 << pr.n_layers) - 1 };
    let (mut count, mut graded, mut shorts) = (0usize, 0.0f64, 0usize);
    for (w, wi) in st.wires.iter().enumerate() {
        if !wi.alive || wi.tris.is_empty() {
            continue;
        }
        let Some(lw) = layers.get(w).filter(|l| l.len() == wi.tris.len()) else { continue };
        let n = wi.passes.len();
        // Per object the length (nm) of this wire's traversals inside it.
        let mut inside: Vec<(usize, f64)> = Vec::new();
        for (k, &t) in wi.tris.iter().enumerate() {
            let on = lw[k] & all;
            if on == 0 {
                continue;
            }
            let block = |o: usize| {
                let ob = &pr.obstacles[o];
                if ob.drilled { all } else { ob.layers.0 & all }
            };
            let foreign: Vec<usize> = pr.tri_obs[t].iter().copied().filter(|&o| pr.obstacles[o].net != Some(wi.net)).collect();
            let blocked = foreign.iter().fold(0u64, |m, &o| m | block(o));
            if on & !blocked != 0 {
                continue;
            }
            let from = if k == 0 { pr.pos[wi.a] } else { st.pass_point(wi.passes[k - 1]) };
            let to = if k == n { pr.pos[wi.b] } else { st.pass_point(wi.passes[k]) };
            let len = (to - from).len();
            for o in foreign.into_iter().filter(|&o| block(o) & on != 0) {
                match inside.iter_mut().find(|x| x.0 == o) {
                    Some(x) => x.1 += len,
                    None => inside.push((o, len)),
                }
            }
        }
        for (o, len) in inside {
            count += 1;
            graded += super::factors::violation_amount(wi.w + wi.s, len);
            if pr.obstacles[o].net.is_some() {
                shorts += 1;
            }
        }
    }
    (count, graded, shorts)
}

/// The loop's criteria of one topology (what the archive keeps): errors = violations = the
/// predicted count, shorts = the predicted shorts among them, penalty = its graded part
/// ([`Predicted::penalty`]). O(1).
pub fn criteria_of(p: Predicted, viol_mm: f64, vias: usize, length_mm: f64, shrinkages: usize, area_mm2: f64) -> Criteria {
    Criteria { errors: p.count, shorts: p.shorts, short_extra: p.short_extra, open: 0, violations: p.count, penalty: p.penalty(viol_mm), vias, length_mm, shrinkages, warnings: 0, area_mm2 }
}

/// The evaluation of a whole topology that every whole-board loop move (layer re-optimisation,
/// floating up, crossing passes) is judged by: the archive's objective value plus the `violation`
/// weight per predicted violation, `base + viol_mm x graded` ([`Predicted::price`]). Lower is
/// better. O(1).
pub fn evaluation(obj: &Objective, c: &Criteria, viol_mm: f64) -> f64 {
    obj.value(c) + viol_mm * c.errors as f64
}

/// The layers of every traversal of every alive wire under the layering `events` (`(traversal,
/// fraction, layer)` per wire, as [`super::layers::LayerReport::events`]): the layer of the run
/// it lies in, both layers inside a via interval (the via may sit anywhere between its two events:
/// Polubasov 2001 p. 4, "if the ends of a stretch between two neighbouring crossings belong to
/// different layers, the stretch needs a via"); a wire without events lies on `bottom`
/// ([`super::draw::plan`]). Bit `l` = layer `l`. O(traversals + events).
pub fn layers_now(st: &State, events: &[Vec<(usize, f64, i32)>], bottom: i32) -> Vec<Vec<u64>> {
    let bit = |l: i32| 1u64 << l.clamp(0, 63);
    (0..st.wires.len())
        .map(|w| {
            let wi = &st.wires[w];
            let n = wi.tris.len();
            if !wi.alive || n == 0 {
                return Vec::new();
            }
            let ev = events.get(w).map_or(&[][..], |e| &e[..]);
            let (Some(first), Some(last)) = (ev.first(), ev.last()) else { return vec![bit(bottom); n] };
            let at = |k: usize| k.min(n - 1);
            let mut m = vec![0u64; n];
            for x in &mut m[..=at(first.0)] {
                *x |= bit(first.2);
            }
            for x in &mut m[at(last.0)..] {
                *x |= bit(last.2);
            }
            for p in ev.windows(2) {
                for x in &mut m[at(p[0].0)..=at(p[1].0).max(at(p[0].0))] {
                    *x |= bit(p[0].2) | bit(p[1].2);
                }
            }
            m
        })
        .collect()
}

/// One single-wire rip-up and reroute ([`reroute_set`]): how the price of a wire is read and how
/// the removed connections are placed again.
pub trait Rerouter<'p> {
    /// The price of wire `w` where it lies (in the state), with this try's coefficients: the
    /// wire's own and social factors (Polubasov 2010: `S = Σ kᵢ xᵢ`), without the predicted
    /// violations of the board ([`Rerouter::violations`]).
    fn price(&mut self, st: &mut State<'p>, w: WireId) -> f64;
    /// Place the connections of the removed wires `gone` again; the new wires, or `None` when one
    /// of them could not be placed at all (its pads lie in parts no edge joins).
    fn place(&mut self, st: &mut State<'p>, gone: &[WireId]) -> Option<Vec<WireId>>;
    /// The price (mm) of the predicted violations of the whole state, the archive's own measure
    /// at the `violation` weight ([`Predicted::price`]): `after` false = the state before the try
    /// (the caller's cached value), true = the state now, with the new wires placed. 0 = no
    /// violations priced (a rerouter that has no board measure).
    fn violations(&mut self, _st: &mut State<'p>, _after: bool) -> f64 {
        0.0
    }
    /// The try was undone (the state is back as before): undo the bookkeeping of `place`.
    fn undo(&mut self);
    /// The try was kept (adopt what `violations(after)` computed for the new state).
    fn keep(&mut self) {}
}

/// Result of [`reroute_set`]: kept or not, the price before and after (`new` is infinite when a
/// connection could not be placed again), each with the predicted violations' price in it
/// (`viol_old`, `viol_new`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RerouteOut {
    pub kept: bool,
    pub old: f64,
    pub new: f64,
    pub viol_old: f64,
    pub viol_new: f64,
}

/// Single-wire rip-up and reroute (Polubasov 2010: "removing the wires one at a time and laying
/// them again along cheaper paths"): the wires of `set` (a wire and the branches that end on its
/// junctions) are taken out and placed again; the change is kept when the price of the new wires
/// plus `extra` (the `reroute` price of moving them once more) plus the predicted violations of
/// the board after it is below the price of the old wires plus the violations before. The
/// violations are the archive's own measure ([`Rerouter::violations`], shopping list 5.2), so a
/// kept move never adds a violation the archive counts without paying the `violation` weight for
/// it, and a move that removes one gains as much. Otherwise the state before comes back exactly
/// (every wire, pass and order). A price, not a gate: nothing is refused by a threshold, only by
/// the price comparison. O(state) for the copy plus the searches of `place` and the measure.
pub fn reroute_set<'p>(st: &mut State<'p>, set: &[WireId], r: &mut dyn Rerouter<'p>, extra: f64) -> RerouteOut {
    let saved = st.topology();
    let viol_old = r.violations(st, false);
    let old: f64 = set.iter().map(|&w| r.price(st, w)).sum::<f64>() + viol_old;
    for &w in set {
        super::tree::remove_wire(st, w);
    }
    let (new, viol_new) = match r.place(st, set) {
        Some(ws) => {
            let local: f64 = ws.iter().map(|&w| r.price(st, w)).sum();
            let v = r.violations(st, true);
            (local + v, v)
        }
        None => (f64::INFINITY, f64::INFINITY),
    };
    if new + extra < old - EPS {
        r.keep();
        return RerouteOut { kept: true, old, new, viol_old, viol_new };
    }
    st.restore(saved);
    r.undo();
    RerouteOut { kept: false, old, new, viol_old, viol_new }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A toy topology: per "wire" a length; a wire with an odd length has a via. A cycle shortens
    /// or lengthens wires at random (seeded) and keeps a change when its price (length + via weight
    /// x via, with the cycle's emphasis) drops.
    #[derive(Clone)]
    struct Toy {
        len: Vec<i64>,
        starts: Vec<Vec<i64>>,
    }

    impl Space for Toy {
        type Var = Vec<i64>;
        fn criteria(&mut self) -> Criteria {
            Criteria { vias: self.len.iter().filter(|&&l| l % 2 != 0).count(), length_mm: self.len.iter().sum::<i64>() as f64, ..Criteria::default() }
        }
        fn snapshot(&mut self) -> Vec<i64> {
            self.len.clone()
        }
        fn restore(&mut self, v: &Vec<i64>) {
            self.len = v.clone();
        }
        fn cycle(&mut self, c: &CycleCtx, _stop: &mut dyn FnMut() -> bool) -> CycleStats {
            self.starts.push(self.len.clone());
            let mut rng = ChaCha8Rng::seed_from_u64(c.seed);
            let n = self.len.len();
            let mut cs = CycleStats::default();
            for j in 0..n {
                let i = rng.gen_range(0..n);
                let d: i64 = rng.gen_range(-3..=3);
                let via = 25.0 * c.emphasis.via_at(j as f64 / n as f64) * (1.0 + c.amp * rng.gen_range(-1.0..=1.0));
                let price = |l: i64| l as f64 + if l % 2 != 0 { via } else { 0.0 };
                let nl = (self.len[i] + d).max(1);
                cs.tried += 1;
                if price(nl) < price(self.len[i]) - EPS {
                    self.len[i] = nl;
                    cs.kept += 1;
                }
            }
            cs
        }
    }

    fn params(seed: u64, restart_after: usize) -> LoopParams {
        LoopParams {
            cycles: 30,
            restart_after,
            seed,
            fluctuation: 0.3,
            narrowing: 0.8,
            stuck: 2,
            reheat: 0.3,
            stable_window: 30,
            stable_gain_pct: 0.0,
            stable_patience: 0,
            emphasis: Emphasis::default(),
            obj: Objective::LengthVias { mm_per_via: 25.0 },
        }
    }

    fn toy() -> Toy {
        Toy { len: vec![41, 17, 30, 9, 55, 23, 12, 7], starts: Vec::new() }
    }

    fn run(seed: u64, restart_after: usize) -> (Toy, Archive<Gen<Vec<i64>>>, LoopReport) {
        let mut sp = toy();
        let mut a = Archive::new(6);
        let t = std::cell::Cell::new(0.0);
        let clock = || {
            t.set(t.get() + 1.0);
            t.get()
        };
        let rep = optimise(&mut sp, &mut a, &params(seed, restart_after), &mut || false, &mut || false, &clock);
        (sp, a, rep)
    }

    #[test]
    fn the_archive_stays_free_of_dominated_variants_and_the_pick_never_worsens() {
        for seed in 0..6 {
            let (_, a, rep) = run(seed, 2);
            for i in 0..a.items.len() {
                for j in 0..a.items.len() {
                    assert!(i == j || !a.items[i].crit.dominates(&a.items[j].crit), "seed {seed}: {} dominates {}", a.items[i].label, a.items[j].label);
                }
            }
            assert_eq!(rep.pick_values.len(), rep.cycles + 1);
            assert!(rep.pick_values.windows(2).all(|w| w[1] <= w[0] + 1e-9), "seed {seed}: {:?}", rep.pick_values);
            assert!(rep.pick_values.last() < rep.pick_values.first(), "seed {seed}: the loop improves the toy");
            // Generations: every variant of a cycle descends from one before it.
            assert!(a.items.iter().all(|v| v.payload.cycle == 0 || v.payload.gen >= 1));
        }
    }

    #[test]
    fn a_restart_goes_on_from_an_archived_variant_and_is_reproducible_by_seed() {
        let (sp1, a1, r1) = run(7, 1);
        let (sp2, a2, r2) = run(7, 1);
        assert!(!r1.restarts.is_empty(), "restart_after = 1 restarts");
        assert_eq!(r1.restarts, r2.restarts);
        assert_eq!(r1.lines, r2.lines);
        assert_eq!(sp1.len, sp2.len);
        assert_eq!(a1.items.iter().map(|v| (v.label.clone(), v.payload.var.clone())).collect::<Vec<_>>(), a2.items.iter().map(|v| (v.label.clone(), v.payload.var.clone())).collect::<Vec<_>>());
        // The cycle after a restart starts from the restored variant (a state some archive held).
        let (sp3, _, r3) = run(7, 1);
        for (cycle, label) in &r3.restarts {
            let start = &sp3.starts[cycle - 1];
            assert!(label == "initial plan (gen 0)" && *start == toy().len || label.starts_with("cycle "), "{label}");
        }
        // Another seed takes other restarts or another path.
        let (_, _, r4) = run(8, 1);
        assert!(r4.restarts != r1.restarts || r4.lines != r1.lines);
    }

    #[test]
    fn without_restarts_the_loop_never_restores() {
        let (sp, _, rep) = run(3, 0);
        assert!(rep.restarts.is_empty());
        // Each cycle starts where the one before ended.
        for k in 1..sp.starts.len() {
            assert!(sp.starts[k] != sp.starts[k - 1] || rep.lines[k - 1].contains("reroutes 0/"));
        }
    }

    #[test]
    fn the_time_limit_ends_the_loop_and_says_so() {
        let mut sp = toy();
        let mut a = Archive::new(4);
        let mut n = 0;
        let mut stop = || {
            n += 1;
            n > 3
        };
        let rep = optimise(&mut sp, &mut a, &params(1, 2), &mut stop, &mut || false, &|| 0.0);
        assert_eq!(rep.stop, "time limit");
        assert_eq!(rep.cycles, 3);
        assert!(!a.items.is_empty());
    }

    #[test]
    fn a_cycle_cut_short_inside_is_not_offered() {
        // M2.TL: Cancel inside a cycle stops it; its half-done state is no variant.
        let mut sp = toy();
        let mut a = Archive::new(4);
        let mut n = 0;
        let mut inner = || {
            n += 1;
            n > 2
        };
        let rep = optimise(&mut sp, &mut a, &params(1, 2), &mut || false, &mut inner, &|| 0.0);
        assert_eq!(rep.stop, "time limit");
        assert_eq!(rep.cycles, 3, "the third cycle was cut inside");
        assert_eq!(rep.offered, 3, "the initial plan and the two finished cycles only");
    }

    #[test]
    fn stable_ends_the_loop() {
        let mut sp = toy();
        let mut a = Archive::new(4);
        let p = LoopParams { stable_window: 2, stable_gain_pct: 50.0, ..params(1, 0) };
        let rep = optimise(&mut sp, &mut a, &p, &mut || false, &mut || false, &|| 0.0);
        assert_eq!(rep.stop, "stable");
        assert_eq!(rep.stable_at, Some(2), "a gain below 50 % over 2 cycles");
        assert_eq!(rep.cycles, 2);
    }

    #[test]
    fn the_emphasis_puts_length_first_and_vias_later_graded() {
        let e = Emphasis { via_start: 0.5, via_end: 1.5 };
        assert!((e.via_at(0.0) - 0.5).abs() < 1e-12 && (e.via_at(1.0) - 1.5).abs() < 1e-12);
        assert!((e.via_at(0.5) - 1.0).abs() < 1e-12, "the mean weight in the middle");
        let xs: Vec<f64> = (0..=20).map(|i| e.via_at(i as f64 / 20.0)).collect();
        assert!(xs.windows(2).all(|w| w[1] >= w[0]), "rising");
        // Graded: no step bigger than a tenth of the range between neighbouring points.
        assert!(xs.windows(2).all(|w| w[1] - w[0] < 0.1 + 1e-12));
        assert_eq!(Emphasis { via_start: 1.0, via_end: 1.0 }.via_at(0.3), 1.0);
    }

    #[test]
    fn per_wire_coefficients_oscillate_within_their_width() {
        assert_eq!(k_osc(1, 2, 3, 4, 0.0), 1.0);
        let ks: Vec<f64> = (0..70).map(|v| k_osc(9, 5, 1, v, 0.3)).collect();
        assert!(ks.iter().all(|&k| (0.7 - 1e-12..=1.3 + 1e-12).contains(&k)));
        let mean = ks.iter().sum::<f64>() / ks.len() as f64;
        assert!((mean - 1.0).abs() < 0.05, "mean {mean}");
        assert!(ks.iter().any(|&k| k > 1.1) && ks.iter().any(|&k| k < 0.9), "it oscillates");
        // Another wire or factor: another phase / period.
        assert!((0..10).any(|v| (k_osc(9, 6, 1, v, 0.3) - k_osc(9, 5, 1, v, 0.3)).abs() > 1e-6));
        assert_eq!(wire_amp(0.2, 0), 0.2);
        assert!((wire_amp(0.2, 1) - 0.3).abs() < 1e-12 && wire_amp(0.2, 100) < 0.4);
    }
}
