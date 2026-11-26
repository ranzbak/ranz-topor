//! `route()` entry point and the result contract with the plugin (`RouteResult`).
//!
//! [`route_with`] drops the ignored pads, runs the topological core ([`crate::topo::core`]) and
//! makes the written copper cleanup-safe ([`crate::topo::tidy`]); `unrouted` is the result of a
//! board the core cannot route at all (every connection failed with one reason).

use crate::control::RunControl;
use crate::decompose::{decompose, TermKind, TermRef};
use crate::geom::Vec2;
use crate::lengths::{hand_outside, terminal_skeletons};
use crate::model::{BoardModel, ExistingCopper, Item, Options};
use crate::stubs::StubOut;
use crate::wires::extract_wires;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

#[derive(Clone, Debug, Serialize)]
pub struct TermOut {
    /// `pad`, `via`, `copper` or `dangling`.
    pub kind: String,
    pub label: String,
    /// KiCad frame, nm.
    pub at: [i64; 2],
}

#[derive(Clone, Debug, Serialize)]
pub struct ConnReport {
    pub id: usize,
    pub net: i32,
    pub name: String,
    pub layer: u32,
    pub width: i64,
    /// `routed`, `partial`, `failed` or `ignored` (pad in `Options.ignore_pads`; not counted in
    /// `connections_total`).
    pub status: String,
    pub reason: String,
    /// Vias of the routed connection.
    pub vias: usize,
    /// Names of the copper layers the legs run on (dedup, in path order); empty when not routed.
    pub layers: Vec<String>,
    /// Via positions (KiCad frame, nm), in path order.
    pub via_sites: Vec<[i64; 2]>,
    pub from: TermOut,
    pub to: TermOut,
    pub length_nm: i64,
    pub length_outside_pads_nm: i64,
    pub pattern: String,
    pub anchors: usize,
    pub crossings: usize,
    /// Where the connection was blocked (KiCad frame nm), when known.
    pub blocking: Option<[i64; 2]>,
    pub g1_angle_deg: Option<f64>,
    /// PathFinder iteration the report comes from.
    pub iteration: u32,
    /// Routed by the zero-slack retry with margin 0 against its corridor (R6).
    pub zero_slack: bool,
    /// Face steps the geometric block probe ran for this connection (D9); `None` when it was
    /// not probed (ghost block on an edge, routed, or timed out first). At most `PROBE_STEP_CAP`.
    pub probe_steps: Option<usize>,
    /// Detour reference (ruling M3-R11): `est_len` of the topological ghost on the empty graph
    /// (no other connection present, same layers and via rules), nm; `null` when the connection
    /// was never searched or the ghost found no path (unreachable goal, deadline).
    pub ghost_len_nm: Option<i64>,
    /// DRC-feasible detour reference (M3-R15, `Options::detour_reference`): `est_len` of the slot
    /// search of this connection alone on the fixed copper (full clearance / width, same layers,
    /// via rules and `via_cost`), nm. With `ref_kind == "ghost"` the search failed (or the budget
    /// ran out) and this is the ghost length. `None` when the option is off or neither exists.
    pub ref_len_nm: Option<i64>,
    /// `"feasible"` (slot search found the reference), `"ghost"` (fallback, flagged), `"none"`.
    pub ref_kind: String,
    /// Pad exit stubs (A6) of the routed connection: total length of its stubs, nm (counted in
    /// `length_nm` / `length_outside_pads_nm`); 0 without one.
    pub stub_nm: i64,
    /// Necked part (NX) of `stub_nm`: the stubs narrower than the net width, nm.
    pub neck_nm: i64,
    /// Narrowest necked stub width, nm; `None` without a neck.
    pub neck_width_nm: Option<i64>,
    /// A5: the connection ends in a same-net T-junction on another connection's (or kept hand)
    /// copper instead of at its `to` terminal (`from` for a reversed path).
    pub junction: bool,
    /// A5: the junction's angle against the copper it joins (degrees, 0..=90).
    pub junction_angle_deg: Option<f64>,
    /// M-B1 never drop: rule violations of the routed connection's copper (listed in
    /// [`RouteResult::violations`]); 0 for legal copper.
    pub violations: usize,
}

/// A rule the written copper breaks (M-B1 never drop: a connection the drawing could not make
/// legal is drawn anyway on its best geometry). KiCad frame, nm.
#[derive(Clone, Debug, Serialize, Default, PartialEq)]
pub struct ViolationReport {
    /// Connection id ([`ConnReport::id`]), net code and name, terminal labels.
    pub conn: usize,
    pub net: i32,
    pub name: String,
    pub from: String,
    pub to: String,
    /// `clearance`, `short` (copper overlap), `hole_clearance`, `edge_clearance`, `connection_width`.
    pub kind: String,
    /// Copper layer name (`all layers` for a through via's hole).
    pub layer: String,
    /// Middle of the stretch below the rule.
    pub at: [i64; 2],
    /// How far below the rule, and over how long (a via: its diameter).
    pub depth_nm: i64,
    pub length_nm: i64,
    /// What it is too close to (`pad:REF:NUM`, `item:N` = routed copper, `edge`, ...).
    pub against: String,
    /// Why the connection's drawing found no legal geometry: its first failed check, one per
    /// connection and the same on all its violations, so not necessarily about this one (a
    /// `short` of the written fallback copper can carry the positive clearance gap of the rejected
    /// legal attempt, e.g. "drawing: clearance gap 0.050 mm < 0.200 mm"). `kind` is this
    /// violation's own, from the written copper.
    pub cause: String,
    /// Price by the `violation` factor (mm).
    pub price_mm: f64,
    /// Where it comes from (M-B2, [`violation_class`]): `pad_exit` (a neighbour pad of its own
    /// terminal's footprint: pad exit / necking, planned in M-C), `via_site` (a via without a legal
    /// site), `crossing` (other-net copper it crosses or passes on the wrong side in the plan),
    /// `foreign_copper` (other routed copper in a passage too narrow), `static` (other pads,
    /// holes, the board edge).
    pub class: String,
}

/// The class of a violation (see [`ViolationReport::class`]) from what it is against, its cause
/// and the exit pins of the connection's terminal pads (`exits`: `REF:NUM` keys,
/// [`exit_pins`]). `pad_exit` only against a neighbour pin of a terminal pad (its own pad exit,
/// planned in M-C / 5.10), directly or as the pad its drawing first failed at; any other pad of
/// the terminal's part is a planner matter (cm1-room, wiccon gui27: RST against U1.15 / U1.20,
/// 13-15 mm from its terminal U1.1, was hidden as pad_exit).
pub fn violation_class(against: &str, cause: &str, exits: &[String]) -> &'static str {
    // `pad:REF:NUM` anywhere in `t` naming an exit pin.
    let exit_pad = |t: &str| {
        t.split("pad:").skip(1).any(|rest| {
            let mut it = rest.splitn(3, ':');
            let (r, n) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
            let n = n.split(|c: char| c.is_whitespace() || c == ')' || c == ',').next().unwrap_or("");
            !r.is_empty() && exits.iter().any(|e| e.split_once(':') == Some((r, n)))
        })
    };
    if against.starts_with("pad:") && exit_pad(against) || cause.starts_with("drawing:") && exit_pad(cause) {
        return "pad_exit";
    }
    if cause.contains("no site") {
        "via_site"
    } else if against.starts_with("track of") || against.starts_with("via of") || against.starts_with("item:") {
        if cause.contains("wrong side") || cause.contains("cannot be layered") || cause.contains("crossing") {
            "crossing"
        } else {
            "foreign_copper"
        }
    } else {
        "static"
    }
}

/// The exit pins of a terminal pad (`REF:NUM` keys): the other pads of its footprint whose centre
/// lies within 1.5 x the terminal pad's nearest same-footprint pitch (its row and diagonal
/// neighbours: sqrt 2 < 1.5 < 2, so a BGA ball's eight neighbours and nothing two pitches away).
/// `pads`: `(ref, number, centre)` of the board; `term`: the terminal pad's index. A pad alone on
/// its part has none. O(pads of the part).
pub fn exit_pins(pads: &[(&str, &str, crate::geom::Vec2)], term: usize) -> Vec<String> {
    let (r, n, c) = pads[term];
    let mates: Vec<(usize, f64)> = (0..pads.len()).filter(|&i| pads[i].0 == r && pads[i].1 != n).map(|i| (i, (pads[i].2 - c).len())).collect();
    let Some(pitch) = mates.iter().map(|x| x.1).filter(|&d| d > 0.0).min_by(f64::total_cmp) else { return Vec::new() };
    let mut out: Vec<String> = mates.iter().filter(|x| x.1 <= 1.5 * pitch).map(|x| format!("{}:{}", r, pads[x.0].1)).collect();
    out.sort();
    out.dedup();
    out
}

/// A connection the zero-slack retry routed with margin 0 against its corridor (R6).
#[derive(Clone, Debug, Serialize)]
pub struct ZeroSlackWire {
    /// Connection id (`ConnReport::id`).
    pub conn: usize,
    pub net: i32,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct NetRouteReport {
    pub net: i32,
    pub name: String,
    /// `routed`, `partial`, `failed`, `pour_served`, `complete`, `out_of_scope` or `excluded`
    /// (do-not-touch net).
    pub status: String,
    pub connections: usize,
    pub routed: usize,
    pub partial: usize,
    pub failed: usize,
    pub vias: usize,
    pub length_nm: i64,
    pub length_outside_pads_nm: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RouteResult {
    pub items: Vec<Item>,
    /// Uuids of existing (kept) tracks the new copper replaces: the tidy merged each into the new
    /// segment that continues it straight on (`[output] cleanup_safe`, as KiCad's Cleanup "merge
    /// co-linear tracks" would). The caller removes them in the same commit as it writes `items`.
    pub remove_uuids: Vec<String>,
    pub connections: Vec<ConnReport>,
    pub nets: Vec<NetRouteReport>,
    pub connections_total: usize,
    pub routed: usize,
    pub partial: usize,
    pub failed: usize,
    pub nets_total: usize,
    pub nets_routed: usize,
    pub total_length_nm: i64,
    pub length_outside_pads_nm: i64,
    /// Vias of the routed connections.
    pub vias: usize,
    /// Routed track length per copper layer name, nm.
    pub length_by_layer_nm: BTreeMap<String, i64>,
    /// Candidate via sites of the run.
    pub sites: usize,
    /// Routed vias whose centre lies inside an SMD pad of their own net (via-in-pad; only with
    /// `Options.via_in_pad`), KiCad frame nm, sorted.
    pub vias_in_pads: Vec<[i64; 2]>,
    /// Via-reduction pass outcome (Task 10): `not run` until then.
    pub via_reduction: String,
    /// Via-relaxation pass outcome (A13): `off`, `not run`, `moved k of n vias, -x um` or
    /// `skipped: timeout ...`.
    pub via_relaxation: String,
    /// Via-shove pass outcome: `off`, `not run`, `routed k of n pinched connections, shoved v
    /// vias` or `skipped: timeout ...`.
    pub via_shove: String,
    /// Blocker rip pass outcome: `off`, `not run`, `routed k more in a of t attempts` or
    /// `skipped: timeout ...`.
    pub rip_pass: String,
    /// Rip-up report of the routing loop (A16): rips per connection (as blocker), pairs that
    /// hit the `ripup_max_per_pair` backstop, total.
    pub ripup: RipReport,
    pub iterations: u32,
    pub converged: bool,
    pub overuse_edges: usize,
    pub clinches: ClinchReport,
    pub relax_iterations: u32,
    pub self_check_rounds: u32,
    /// Grid candidates visited by the self-checks up to the reported iteration (scaling diagnostic).
    pub check_candidates_visited: usize,
    pub timed_out: bool,
    /// Why the (first) routing pass ended: `converged`, `cap` (iteration cap), `no gain` (stop
    /// rule: `patience` iterations without a newly routed connection or `min_gain_pct` gain),
    /// `timeout`, `cancel` or `stuck`.
    pub stop_reason: String,
    pub runtime_s: f64,
    pub threads_used: usize,
    pub warnings: Vec<String>,
    /// Zero-slack retry (R6) connections, by id.
    pub zero_slack_wires: Vec<ZeroSlackWire>,
    /// Outcome of the zero-slack retry: `off`, `kept`, `no candidate`, `no length gain`,
    /// `infeasible: …`, `skipped: deadline` or `not run`.
    pub zero_slack: String,
    /// Per-iteration timers; only with `Options::profile` (never in the default output).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<Vec<IterProfile>>,
    /// `connections[..].id` of every item (same order as `items`); in-process only.
    #[serde(skip)]
    pub item_conn: Vec<usize>,
    /// Pad exit stubs (A6 / NX) planned by the routing passes, in pass and pad order.
    pub stubs: Vec<StubOut>,
    /// Topological core: the variant table of the Pareto archive (`[optimize] objective`), the
    /// picked one marked; this result is the picked variant. Empty for other engines.
    pub variants: Vec<crate::topo::archive::VariantRow>,
    /// Topological core: P4 moves applied per type (summed over the rounds of the picked variant).
    pub moves: BTreeMap<String, usize>,
    /// M-B1 never drop: every rule the written copper breaks, with place, type and cause
    /// (`[optimize] allow_violations`; empty with legal-only output).
    pub violations: Vec<ViolationReport>,
    /// D7: one entry per routed differential-pair connection (topological core): lengths, skew,
    /// coupled share at the gap, uncoupled length at each end, vias.
    pub diff_pairs: Vec<DiffPairReport>,
    /// M1 compass (topological core): what the plan handed to the drawing predicts per violation
    /// class, located, and per connection its planned vias and length ([`crate::topo::compass`]);
    /// `None` for other engines and for results without a plan.
    pub compass: Option<crate::topo::compass::Compass>,
    /// thick-layer F4 (`[output] plan_report`, topological core): the plan handed to the drawing
    /// of the row this result drew, as compact JSON ([`crate::topo::plandump::plan_json`]: per
    /// wire crossings in order with their layers, passes, layers, planned vias and sites, width
    /// and room; per connection the compass's vias, taut length and predicted items; the row's
    /// label, round and seed). The plugin writes it to `plan.json.gz` in the report folder and
    /// leaves it out of `result.json`. `None` without a plan or with the option off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// M2.TL: the time limit of the call and what it did (`None`: no limit set).
    pub limit: Option<LimitReport>,
    /// TW3: the four bests of the TopoR loop's one variant table as drawn (`role` = the roles the
    /// row held in the table by its predicted criteria; `drawn_roles` = the roles it holds among
    /// the drawn rows; `applied` = the row handed back; `predicted` / `drawn` criteria). Empty
    /// without the TopoR loop.
    pub bests: Vec<serde_json::Value>,
    /// TW2: how the ERBS drawing drew its runs (`[drawing] realise = "erbs"`; `None` otherwise).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub erbs: Option<ErbsRunReport>,
}

/// TW2: two bands kept across a model gap ([`ErbsRunReport::gap_kept_list`]): the yielding band
/// (wire, run), the other wire, the overlap of the clearance (mm), where (KiCad frame, mm), layer id.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct ErbsGapKept {
    pub wire: usize,
    pub run: usize,
    pub other_wire: usize,
    pub overlap_mm: f64,
    pub x_mm: f64,
    pub y_mm: f64,
    pub layer: u32,
}

/// TW2: a run of a drawn wire with no ERBS stretch at its ends ([`ErbsRunReport::no_band_list`],
/// class `cut_no_band`): the wire, run, layer id, connection (net, pad labels) and why: "no
/// stretch" (the ERBS drawing has none for the wire), "other layer" (none on the run's layer), or
/// "ends moved" (a stretch on the layer whose ends lie `end_moved_mm` from the run's: a via or
/// pad end placed elsewhere than the ERBS assumed).
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct ErbsNoBand {
    pub wire: usize,
    pub run: usize,
    pub layer: u32,
    pub net: String,
    pub from: String,
    pub to: String,
    pub why: String,
    pub end_moved_mm: f64,
}

/// TW2 (`[drawing] realise = "erbs"`): the runs of the drawn wires by how they came about, and the
/// joint band commit ([`crate::topo::joint`]). `runs` = `from_erbs + replaced + cut_back +
/// forced_erbs`; `cut_back` = the sum of the `cut_*` classes (legal sequential runs and runs drawn
/// by never drop off their band); `forced` counts every run drawn by never drop.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct ErbsRunReport {
    /// `[drawing] erbs_conflict_keep`.
    pub keep: String,
    /// Bands checked by the joint commit, kept, rejected per class.
    pub bands: usize,
    pub kept: usize,
    pub rejected_static: usize,
    pub rejected_predicted: usize,
    pub rejected_gap: usize,
    /// Conflict components solved exactly, and greedily (above `erbs::KEEP_EXACT_CAP` bands).
    pub components_exact: usize,
    pub components_greedy: usize,
    /// Of `kept`: bands kept in their octilinear form because the any-angle band missed a clearance
    /// (to a static item or a kept band) that the octilinear form keeps (octilinear style only).
    pub octi_rescued: usize,
    /// Conflicts across a model gap of the ERBS kept with both bands (`[drawing]
    /// erbs_gap_keep_max_mm`): the gap stays as a clearance violation of the band drawn second.
    /// Count, deepest overlap (mm), every pair (the TW2b list).
    pub gap_kept: usize,
    pub gap_kept_max_mm: f64,
    pub gap_kept_list: Vec<ErbsGapKept>,
    /// The runs drawn without a band (`cut_no_band` and forced ones), one entry each.
    pub no_band_list: Vec<ErbsNoBand>,
    /// Runs of the drawn wires.
    pub runs: usize,
    pub from_erbs: usize,
    /// Of `from_erbs`: drawn in their octilinear form; kept any-angle because the octilinear form
    /// broke a rule against another band.
    pub octi_form: usize,
    pub octi_vs_band: usize,
    /// Legal ERBS a cheaper sequential run replaced (length factor).
    pub replaced: usize,
    /// Legal runs of the sequential attempts (the ladder); part of `cut_back`.
    pub sequential: usize,
    pub cut_back: usize,
    pub cut_static: usize,
    pub cut_predicted: usize,
    pub cut_gap: usize,
    pub cut_no_band: usize,
    pub cut_junction: usize,
    pub cut_redrawn: usize,
    pub forced: usize,
    pub forced_erbs: usize,
    /// Reserved band items taken out by another wire's drawing (invariant: 0).
    pub lost_to_other: usize,
    /// Reserved bands left after the drawing loop, copper no wire owned (invariant: 0).
    pub left_reserved: usize,
}

/// M2.TL: the time limit of a call ([`RouteResult::limit`]): the limit (s since the start, as it
/// stood at the end), `[run] timeout_mode`, whether it was reached, the step that ran then (its
/// stage text) and the time past it until the result was ready (`overrun_s`, 0 when not reached).
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct LimitReport {
    pub limit_s: f64,
    pub mode: String,
    pub reached: bool,
    pub step: String,
    pub overrun_s: f64,
    /// F3 `[run] timeout_overrun_s` on the engine's clock (`None` = no cap here; the plugin's CPU
    /// clock caps on its own and hardens the limit, `cap_hit` then true).
    pub cap_s: Option<f64>,
    /// The overrun cap fired: from then on the soft limit acted as the hard one.
    pub cap_hit: bool,
    /// What the limit cut when it acted as hard (`hard`, or past the overrun cap; user
    /// 2026-11-13: one path for both): `"plan"` = a planning step was cut and the kept plan drawn
    /// in full; `"drawing"` = the drawing was the step that ran and was cut (connections left
    /// unrouted, or its via reduction or pour stitching cut); `"none"` = nothing (the soft limit,
    /// or it passed after the steps it stops).
    pub cut: String,
    /// The steps the limit cut, as the stop tests noted them (`crate::control::CutStep::name`:
    /// `plan`, `plane_stubs`, `drawing`, `via_reduction`, `stitching`); empty when `cut` is `"none"`.
    pub cut_steps: Vec<String>,
    /// `[run] limit_grace_s` (`None` = no bound): seconds past the limit the TopoR loop step may run.
    pub grace_s: Option<f64>,
    /// The grace ran out: the loop step was stopped at its next check and the cycle it was in dropped.
    pub grace_hit: bool,
    /// Seconds the step that ran when the limit passed (`step`: plane stubs, projection, initial
    /// routing, the TopoR loop, the drawing) went on past it before it ended.
    pub wait_s: f64,
    /// A TopoR loop cycle was cut by the limit (grace or cap) and dropped: the best variant so far was drawn.
    pub cycle_dropped: bool,
    /// `[run] drawing_grace_s` (`None` = no bound, or hard mode): seconds the drawing may run past the
    /// later of the limit and its start before its optional passes (via reduction, ERBS) stop.
    pub draw_grace_s: Option<f64>,
    /// The drawing bound ran out: an optional pass stopped (every wire still drawn).
    pub draw_grace_hit: bool,
    /// Wires drawn the sequential way because the drawing bound cut their ERBS (`realise = "erbs"`).
    pub draw_grace_erbs: usize,
    /// time-limit-fix r2: seconds on the limit's clock (optimizing time; with `scope = "all"` the
    /// whole call).
    pub optimize_s: f64,
    /// time-limit-fix r2: seconds of the final drawing, off the clock (0 with `scope = "all"`).
    pub draw_s: f64,
    /// `[run] timeout_scope`: `"optimizing"` (default: the final drawing is not counted) or `"all"`.
    pub scope: String,
}

/// A routed differential-pair connection (D7), nm. `from` / `to` are the P connection's pads.
#[derive(Clone, Debug, Serialize, Default, PartialEq)]
pub struct DiffPairReport {
    pub conn_p: usize,
    pub conn_n: usize,
    pub p_name: String,
    pub n_name: String,
    pub from: String,
    pub to: String,
    pub length_p_nm: i64,
    pub length_n_nm: i64,
    pub skew_nm: i64,
    pub skew_estimate_nm: i64,
    pub coupled_nm: i64,
    pub coupled_pct: f64,
    pub gap_nm: i64,
    pub gap_held: bool,
    pub gap_dev_nm: i64,
    pub uncoupled_start_nm: [i64; 2],
    pub uncoupled_end_nm: [i64; 2],
    pub uncoupled_vias_nm: i64,
    pub vias_p: usize,
    pub vias_n: usize,
    pub polarity_cross: bool,
}

/// One PathFinder iteration's cost (diagnostics, `Options::profile`). Seconds are wall clock.
#[derive(Clone, Debug, Default, Serialize)]
pub struct IterProfile {
    pub iteration: u32,
    /// Parallel search phase.
    pub search_s: f64,
    /// Sequential commit phase including the re-searches of invalidated paths.
    pub commit_s: f64,
    /// Metrization (sketch, relax, quantise, self-check), `check_s` included.
    pub metrize_s: f64,
    /// Self-check rounds inside the metrization.
    pub check_s: f64,
    /// Connections searched in the parallel phase.
    pub searched: usize,
    /// Paths invalidated by an earlier commit and searched again.
    pub re_searched: usize,
    /// Paths invalidated by an earlier commit and accepted by the O(k) re-validation (F4).
    pub revalidated: usize,
    /// Connections ripped up for the next iteration.
    pub ripped: usize,
    /// Paths the negotiator holds after the commit (before metrization).
    pub routed: usize,
    pub overuse_edges: usize,
    /// Connections metrized and self-checked OK this iteration (what `routed` reports when
    /// this is the best iteration). DX diagnosis instrumentation, profile only.
    pub ok: usize,
    /// Connections blocked by the search this iteration.
    pub blocked: usize,
    /// Paths found but rejected by sketch / relax / self-check (re-searched next iteration).
    pub metrize_failed: usize,
    /// Vias of the OK connections.
    pub vias: usize,
    /// Stop-rule cost of the OK connections: `Σ length + via_cost · vias` (nm).
    pub cost_nm: f64,
    /// This iteration became the best so far (`better_than`).
    pub best: bool,
    /// A5: commit-phase re-searches toward a sibling committed earlier in the iteration.
    pub steiner_researched: usize,
    /// DG diagnosis, profile only: the `metrize_failed` connections by [`reason_class`] of
    /// their failure reason (sums to `metrize_failed`).
    pub fail_classes: BTreeMap<String, usize>,
    /// DG diagnosis, profile only: the `blocked` connections by what the search last saw:
    /// `static` (a terminal that cannot be left / entered), `edge` / `site` (a priced edge or
    /// via site on the ghost path), `none` (sums to `blocked`).
    pub blocked_classes: BTreeMap<String, usize>,
}


/// Rip-up report of a run (A16): who was ripped how often, which pairs hit the backstop.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct RipReport {
    /// Per connection (index like `conns`): how many times it was ripped out as a blocker.
    pub rips_per_connection: Vec<u32>,
    /// `[ripper, blocker]` pairs for which a rip was refused by `ripup_max_per_pair`
    /// (sorted, no duplicates).
    pub pairs_at_backstop: Vec<[usize; 2]>,
    /// Rips performed in total (sum of `rips_per_connection`).
    pub total: u32,
    /// Largest rip count of any single `(ripper, blocker)` pair.
    pub max_pair: u32,
}

/// Clinches found in a routing: `(conn, edge)` pairs (edges of the layer graphs, all layers).
#[derive(Clone, Debug, Default, Serialize)]
pub struct ClinchReport {
    pub regular: Vec<(usize, usize)>,
    pub u: Vec<(usize, usize)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireStats {
    pub wires: usize,
    pub skipped: usize,
    pub hand_length_nm: i64,
    pub hand_outside_pads_nm: i64,
}

/// Hand-copper statistics of the unripped board (the length gate's reference).
pub fn wire_stats(model: &BoardModel) -> WireStats {
    let smooth = Options {
        existing_copper: ExistingCopper::Smooth,
        ..Options::default()
    };
    let ex = extract_wires(model, &smooth);
    let (mut len, mut out) = (0.0, 0.0);
    for w in &ex.wires {
        len += w.hand_len;
        out += hand_outside(w, &terminal_skeletons(model, w));
    }
    WireStats {
        wires: ex.wires.len(),
        skipped: ex.skipped.len(),
        hand_length_nm: len.round() as i64,
        hand_outside_pads_nm: out.round() as i64,
    }
}

impl ConnReport {
    /// A report with every measure empty; the caller fills what the connection has.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn blank(id: usize, net: i32, name: String, layer: u32, width: i64, status: &str, reason: &str, from: TermOut, to: TermOut) -> ConnReport {
        ConnReport {
            id,
            net,
            name,
            layer,
            width,
            status: status.into(),
            reason: reason.into(),
            vias: 0,
            layers: Vec::new(),
            via_sites: Vec::new(),
            from,
            to,
            length_nm: 0,
            length_outside_pads_nm: 0,
            pattern: String::new(),
            anchors: 0,
            crossings: 0,
            blocking: None,
            g1_angle_deg: None,
            iteration: 0,
            zero_slack: false,
            probe_steps: None,
            ghost_len_nm: None,
            ref_len_nm: None,
            ref_kind: "none".into(),
            stub_nm: 0,
            neck_nm: 0,
            neck_width_nm: None,
            junction: false,
            junction_angle_deg: None,
            violations: 0,
        }
    }
}

pub(crate) fn kicad(p: Vec2) -> [i64; 2] {
    [p.x.round() as i64, (-p.y).round() as i64]
}

pub(crate) fn term_out(t: &TermRef) -> TermOut {
    TermOut {
        kind: match t.kind {
            TermKind::Pad(_) => "pad",
            TermKind::Via(_) => "via",
            TermKind::Copper(_) => "copper",
            TermKind::Junction => "junction",
            TermKind::Dangling(..) => "dangling",
            TermKind::Pour(_) => "pour",
        }
        .into(),
        label: t.label.clone(),
        at: kicad(t.at),
    }
}


/// `model` without the pads of `opts.ignore_pads` (no terminal, no obstacle) and the removed pad
/// indices of `model`. O(P log I).
fn drop_ignored_pads(model: &BoardModel, opts: &Options) -> (BoardModel, Vec<usize>) {
    let ign: BTreeSet<(&str, &str)> = opts
        .ignore_pads
        .iter()
        .map(|p| (p.ref_.as_str(), p.number.as_str()))
        .collect();
    let mut filtered = model.clone();
    let mut removed = Vec::new();
    let mut i = 0usize;
    filtered.pads.retain(|p| {
        let hit = ign.contains(&(p.ref_.as_str(), p.number.as_str()));
        if hit {
            removed.push(i);
        }
        i += 1;
        !hit
    });
    (filtered, removed)
}

/// One `ignored` [`ConnReport`] per removed pad on a net (net 0 pads have no connections).
/// Not counted in `connections_total`. O(I).
fn append_ignored(r: &mut RouteResult, model: &BoardModel, removed: &[usize]) {
    for &pi in removed {
        let p = &model.pads[pi];
        if p.net == 0 {
            continue;
        }
        let t = TermRef {
            kind: TermKind::Pad(pi),
            label: format!("{}.{}", p.ref_, p.number),
            at: p.center.to_vec2(),
            layers: Vec::new(),
        };
        let name = model
            .nets
            .iter()
            .find(|n| n.code == p.net)
            .map(|n| n.name.clone())
            .unwrap_or_default();
        r.connections.push(ConnReport {
            id: r.connections.len(),
            net: p.net,
            name,
            layer: 0,
            width: model.rules.net_rule(p.net).width,
            status: "ignored".into(),
            reason: "pad outside board".into(),
            vias: 0,
            layers: Vec::new(),
            via_sites: Vec::new(),
            from: term_out(&t),
            to: term_out(&t),
            length_nm: 0,
            length_outside_pads_nm: 0,
            pattern: String::new(),
            anchors: 0,
            crossings: 0,
            blocking: None,
            g1_angle_deg: None,
            iteration: 0,
            zero_slack: false,
            probe_steps: None,
            ghost_len_nm: None,
            ref_len_nm: None,
            ref_kind: "none".into(),
            stub_nm: 0,
            neck_nm: 0,
            neck_width_nm: None,
            junction: false,
            junction_angle_deg: None,
            violations: 0,
        });
    }
}

/// Route `model` with the topological core ([`crate::topo::core`]).
pub fn route(model: &BoardModel, opts: &Options) -> RouteResult {
    route_with(model, opts, &RunControl::default())
}

/// [`route`] under a [`RunControl`] (cancellation and progress).
pub fn route_with(model: &BoardModel, opts: &Options, ctl: &RunControl) -> RouteResult {
    let (filtered, ignored) = if opts.ignore_pads.is_empty() { (None, Vec::new()) } else { let (f, i) = drop_ignored_pads(model, opts); (Some(f), i) };
    // The finishing of the copper runs inside the call, before its run time and time-limit report
    // are taken (a wait of the drawing, its last step, counts to the copper handed back).
    let mut post = |r: &mut RouteResult| {
        if filtered.is_some() {
            append_ignored(r, model, &ignored);
        }
        ctl.test_point("finish copper");
        finish_copper(model, opts, r);
    };
    crate::topo::core::route_topo(filtered.as_ref().unwrap_or(model), opts, ctl, &mut post)
}

/// The steps after the topological core on the whole board: track ends off pad centres within the
/// edge clearance, and (`cleanup_safe`) the tidy that keeps KiCad's Cleanup from changing the copper.
fn finish_copper(model: &BoardModel, opts: &Options, r: &mut RouteResult) {
    // track ends in pads that sit within the board-edge clearance: off the pad centre, so the
    // track keeps the clearance KiCad's DRC measures for it
    let trims = crate::topo::tidy::pad_ends_off_edge(model, &mut r.items);
    if !trims.is_empty() {
        let on_cut = |v: &ViolationReport| {
            v.kind == "edge_clearance"
                && trims.iter().any(|t| {
                    let f = |p: (i64, i64)| crate::geom::Vec2 { x: p.0 as f64, y: p.1 as f64 };
                    let at = crate::geom::Vec2 { x: v.at[0] as f64, y: v.at[1] as f64 };
                    t.net == v.net && crate::topo::draw::seg_seg_dist(at, at, f(t.from), f(t.to)) <= 0.5 * t.width as f64 + 50_000.0
                })
        };
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut r.violations).into_iter().partition(|v| on_cut(v));
        r.violations = kept;
        for v in &gone {
            if let Some(c) = r.connections.iter_mut().find(|c| c.id == v.conn) {
                c.violations = c.violations.saturating_sub(1);
            }
        }
        r.warnings.push(format!(
            "{} track ends in pads within the board-edge clearance moved off the pad centre (still inside the pad) so the track keeps the edge clearance; {} edge-clearance violations gone",
            trims.len(),
            gone.len()
        ));
    }
    if opts.cleanup_safe {
        // the copper as written must survive KiCad's Cleanup Tracks & Vias unchanged
        if r.item_conn.len() != r.items.len() {
            r.item_conn.resize(r.items.len(), usize::MAX);
        }
        let st = crate::topo::tidy::tidy(model, &mut r.items, &mut r.item_conn);
        r.remove_uuids.extend(st.replaced_kept.iter().cloned());
        if st.changed() {
            r.warnings.push(st.line());
        }
    }
    // pad-snap (A1b): track ends inside a pad of their net but off its centre, as written
    let (n, worst) = crate::topo::tidy::off_centre_pad_ends(model, &r.items);
    r.warnings.push(format!(
        "pad ends off centre: {n} track ends end inside a pad of their net off its centre (KiCad connects them, but its interactive router links a pad only at its centre: loose for drag and footprint moves), largest {:.3} mm",
        worst / 1e6
    ));
    // pad-centre-price: every trace end KiCad's Cleanup keeps inside its pad off the centre (the
    // centre-distance hints of `pad_end_breaches`, by kind), the count the `pad_off_centre` factor
    // works on.
    r.warnings.push(pad_off_centre_line(&crate::topo::tidy::pad_end_breaches(model, &r.items)));
}

/// pad-centre-price: the report line of the centre-distance hints ([`crate::topo::tidy::PadEndBreach::hint`]).
pub(crate) fn pad_off_centre_line(bs: &[crate::topo::tidy::PadEndBreach]) -> String {
    use crate::topo::tidy::PadEndKind as K;
    let hints: Vec<&crate::topo::tidy::PadEndBreach> = bs.iter().filter(|b| b.hint() && b.off > 1_000.0).collect();
    let n = |k: K| hints.iter().filter(|b| b.kind == k).count();
    let worst = hints.iter().map(|b| b.off).fold(0.0, f64::max);
    format!(
        "pad_off_centre: {} trace ends inside a pad of their net off its centre (Terminal {}, Junction {}, Tee {}, PassThrough {}; KiCad's Cleanup keeps them, its interactive router links a pad only at its centre; priced by [factors] pad_off_centre), largest {:.3} mm",
        hints.len(),
        n(K::Terminal),
        n(K::Junction),
        n(K::Tee),
        n(K::PassThrough),
        worst / 1e6
    )
}

/// Decompose `model` on its routable layers and report every connection as failed with
/// `reason`, nets summarised as the plugin expects. O(decompose + C·N).
pub(crate) fn unrouted(model: &BoardModel, opts: &Options, reason: &str) -> RouteResult {
    let t0 = Instant::now();
    let mut warnings: Vec<String> = vec![reason.to_string()];
    let mut routable: Vec<u32> = model
        .layers
        .iter()
        .filter(|l| l.routable)
        .map(|l| l.id)
        .collect();
    routable.sort_unstable();
    routable.dedup();
    if routable.is_empty() {
        warnings.push("no routable layer".into());
    }
    let dec = decompose(model, opts, &routable);
    warnings.extend(dec.warnings.iter().cloned());
    for (net, r) in &dec.skipped {
        warnings.push(format!("net {net} skipped: {r}"));
    }
    let first_layer = routable.first().copied().unwrap_or(0);
    let reports: Vec<ConnReport> = dec
        .connections
        .iter()
        .map(|c| ConnReport {
            id: c.id,
            net: c.net,
            name: dec
                .nets
                .iter()
                .find(|n| n.net == c.net)
                .map(|n| n.name.clone())
                .unwrap_or_default(),
            layer: first_layer,
            width: c.width,
            status: "failed".into(),
            reason: reason.into(),
            vias: 0,
            layers: Vec::new(),
            via_sites: Vec::new(),
            from: term_out(&c.from),
            to: term_out(&c.to),
            length_nm: 0,
            length_outside_pads_nm: 0,
            pattern: String::new(),
            anchors: 0,
            crossings: 0,
            blocking: None,
            g1_angle_deg: None,
            iteration: 0,
            zero_slack: false,
            probe_steps: None,
            ghost_len_nm: None,
            ref_len_nm: None,
            ref_kind: "none".into(),
            stub_nm: 0,
            neck_nm: 0,
            neck_width_nm: None,
            junction: false,
            junction_angle_deg: None,
            violations: 0,
        })
        .collect();
    let nets: Vec<NetRouteReport> = dec
        .nets
        .iter()
        .map(|n| {
            let count = n.connections.len();
            let status = if n.excluded {
                "excluded"
            } else if n.pour_served {
                "pour_served"
            } else if !n.in_scope {
                "out_of_scope"
            } else if count == 0 {
                "complete"
            } else {
                "failed"
            };
            NetRouteReport {
                net: n.net,
                name: n.name.clone(),
                status: status.into(),
                connections: count,
                routed: 0,
                partial: 0,
                failed: count,
                vias: 0,
                length_nm: 0,
                length_outside_pads_nm: 0,
            }
        })
        .collect();
    let nets_total = nets.iter().filter(|n| n.connections > 0).count();
    let failed = reports.len();
    RouteResult {
        items: Vec::new(),
        remove_uuids: Vec::new(),
        connections_total: reports.len(),
        connections: reports,
        nets,
        routed: 0,
        partial: 0,
        failed,
        nets_total,
        nets_routed: 0,
        total_length_nm: 0,
        length_outside_pads_nm: 0,
        vias: 0,
        length_by_layer_nm: BTreeMap::new(),
        sites: 0,
        vias_in_pads: Vec::new(),
        via_reduction: "not run".into(),
        via_relaxation: "not run".into(),
        via_shove: "not run".into(),
        rip_pass: "not run".into(),
        ripup: RipReport::default(),
        iterations: 0,
        converged: false,
        overuse_edges: 0,
        clinches: ClinchReport::default(),
        relax_iterations: 0,
        self_check_rounds: 0,
        check_candidates_visited: 0,
        timed_out: false,
        stop_reason: "no engine".into(),
        runtime_s: t0.elapsed().as_secs_f64(),
        threads_used: 1,
        warnings,
        zero_slack_wires: Vec::new(),
        zero_slack: "not run".into(),
        profile: None,
        item_conn: Vec::new(),
        stubs: Vec::new(),
        variants: Vec::new(),
        moves: BTreeMap::new(),
        violations: Vec::new(),
        diff_pairs: Vec::new(),
        compass: None,
        plan: None,
        limit: None,
        bests: Vec::new(),
        erbs: None,
    }
}

#[cfg(test)]
mod tests {

    /// pad-centre-price: the count line names every kind, counts only the hints (not an
    /// edge-clear or blocked-centre end, not one within 1 um of the centre) and the largest.
    #[test]
    fn pad_off_centre_line_counts_the_hints_by_kind() {
        use super::pad_off_centre_line;
        use crate::topo::tidy::{PadEndBreach, PadEndKind as K};
        let b = |kind: K, off: f64, edge: bool, blocked: bool| PadEndBreach { kind, net: 1, layer: 0, pad: "U1.1".into(), at: (0, 0), off, edge, centre_blocked: blocked, wire: None };
        assert!(pad_off_centre_line(&[]).starts_with("pad_off_centre: 0 trace ends inside a pad of their net off its centre (Terminal 0, Junction 0, Tee 0, PassThrough 0;"));
        let l = pad_off_centre_line(&[b(K::Terminal, 250_000.0, false, false), b(K::Terminal, 100_000.0, false, false), b(K::Junction, 50_000.0, false, false), b(K::Tee, 700_000.0, false, false), b(K::PassThrough, 30_000.0, false, false), b(K::PassThrough, 0.0, false, false), b(K::Terminal, 900_000.0, true, false), b(K::Terminal, 900_000.0, false, true)]);
        assert!(l.starts_with("pad_off_centre: 5 trace ends inside a pad of their net off its centre (Terminal 2, Junction 1, Tee 1, PassThrough 1;"), "{l}");
        assert!(l.ends_with("largest 0.700 mm"), "{l}");
    }

    use super::{exit_pins, violation_class};
    use crate::geom::Vec2;

    #[test]
    fn violation_classes() {
        let ex = vec!["U1:56".to_string(), "U1:58".to_string()];
        // A neighbour pin of its own terminal, directly or as the first failed check (M-C).
        assert_eq!(violation_class("pad:U1:58", "drawing: clearance gap", &ex), "pad_exit");
        assert_eq!(violation_class("track of D10 (item:3)", "drawing: clearance gap 0.097 mm < 0.150 mm against pad:U1:58 on layer F.Cu", &ex), "pad_exit");
        // cm1-room: another pin of the same part, not next to the terminal, is no pad exit.
        assert_eq!(violation_class("pad:U1:15", "drawing: clearance gap", &ex), "static");
        assert_eq!(violation_class("pad:U1:580", "drawing: clearance gap", &ex), "static", "a number is matched whole");
        assert_eq!(violation_class("track of D10 (item:3)", "drawing: clearance gap 0.097 mm < 0.150 mm against pad:U1:15 on layer F.Cu", &ex), "foreign_copper");
        assert_eq!(violation_class("track of X (item:3)", "via 1 of 2: no site in its interval", &ex), "via_site");
        assert_eq!(violation_class("via of X (item:3)", "drawing: passes a via of another net on the wrong side (topology)", &ex), "crossing");
        assert_eq!(violation_class("track of X (item:3)", "drawing: clearance gap 0.1 mm against item:9", &ex), "foreign_copper");
        assert_eq!(violation_class("pad:R3:1", "drawing: clearance gap 0.1 mm against pad:R3:1", &ex), "static");
        assert_eq!(violation_class("pad:U1:58", "drawing: clearance gap", &[]), "static", "no exit pins: no pad exit");
    }

    /// cm1-room: the exit pins of a terminal are its row and diagonal neighbours on its own part.
    #[test]
    fn exit_pins_are_the_row_and_diagonal_neighbours() {
        let v = |x: f64, y: f64| Vec2 { x, y };
        // A 3 x 3 ball grid at pitch 1 (centre ball "5"), a pin two pitches away, another part.
        let mut pads: Vec<(&str, &str, Vec2)> = Vec::new();
        let names = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];
        for (i, n) in names.iter().enumerate() {
            pads.push(("U1", n, v((i % 3) as f64, (i / 3) as f64)));
        }
        pads.push(("U1", "10", v(3.0, 1.0)));
        pads.push(("R1", "1", v(1.0, 2.0)));
        let ex = exit_pins(&pads, 4);
        assert_eq!(ex, ["U1:1", "U1:2", "U1:3", "U1:4", "U1:6", "U1:7", "U1:8", "U1:9"]);
        // A corner ball: three neighbours, not the far corner (2 sqrt 2).
        assert_eq!(exit_pins(&pads, 0), ["U1:2", "U1:4", "U1:5"]);
        // Pin 10 (2 pitches from 5, 1 from 6): only 3, 6, 9 are within 1.5 of it.
        assert_eq!(exit_pins(&pads, 9), ["U1:3", "U1:6", "U1:9"]);
        // A part with one pad: none.
        assert!(exit_pins(&pads, 10).is_empty());
    }
}
