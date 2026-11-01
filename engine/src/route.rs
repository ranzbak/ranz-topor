//! `route()` orchestration (M2 / T2.4a, M3 / T3.1-T3.2): decompose -> obstacles -> via sites ->
//! one graph per routable layer (`MultiGraph`) -> negotiation over layers and via sites ->
//! per-leg metrization with the M1/M2 `relax` -> quantisation -> `Checker` self-check with
//! per-connection feedback -> `RouteResult`.
//!
//! Every routable copper layer is searched at once: a connection starts on every layer of its
//! source terminal, ends on every layer of its target (a pour target is a via site inside the
//! pour) and changes layer at via sites. One routed path is a list of *legs*, one wire per leg
//! (wire id = leg id of the `MultiGraph`, P2): leg `j` runs from the real terminal or the via
//! `j - 1` to the via `j` or the target; a via is a `ViaItem` (through, `layers: []`) attached to
//! the leg that ends at it. A connection is routed iff every leg and via passes the self-check.
//! Every PathFinder iteration searches the active connections, sketches every routed leg on the
//! triangulation of the *real* obstacles (`base_crossings`: the dormant via-site vertices that the
//! search winds around bend no wire) and metrizes incrementally (`MetrState`, Fix-B2): only the
//! searched / ripped-up connections, those whose anchor sequence or nesting changed and, as chains
//! move, their neighbours are re-tautened, re-quantised and re-checked; the first call completes a
//! pass over every wire before the deadline counts (anytime). A connection failing relax or the
//! self-check is penalised (`penalise_failed`) and re-searched. A connection whose terminal lies
//! inside a foreign obstacle (`InsideObstacle`) is class-independent: it is failed once, removed
//! from the graph and never searched again (design note §4). Early failures (no search):
//! `"terminal not on any routable layer"`, `"no via site inside pour"`.
//! Caps: iterations `pathfinder_iter_cap`, self-check rounds <= routed legs + 1, the wall clock is
//! checked before every search (negotiate), relax call and self-check round. Output is ordered
//! by connection id; no `HashMap` iteration reaches it.
//!
//! Stop rule (`Options::patience`, `min_gain_pct`): the loop ends (`stop_reason` `no gain`) when
//! over the last `patience` iterations no connection was newly routed and the best cost
//! `Σ length + via_cost · vias` improved by less than `min_gain_pct` percent in total; cancel
//! (`RunControl`) ends it like a timeout. Zero-slack retry (R6, `Options::zero_slack_retry`): after the
//! loop, [`zero_slack_pass`] lets one connection take a corridor pinched below ε with margin 0
//! against that corridor only (candidates: single-leg paths), re-routes the detoured connections
//! once and keeps the result when it routes at least as much with a shorter outside-pad length;
//! the harness DRC is the final judge. Partial stubs grow on the first leg of a blocked
//! connection's ghost (D6). The via-reduction pass is Task 10.

use crate::cdt::Crossing;
use crate::check::{Checker, ItemIndex};
use crate::decompose::{decompose, terminal_of, Connection, TermKind, TermRef};
use crate::geom::quant::{quantise_chain, round_nm, TrackPrim};
use crate::geom::{dequantise, dist_prim_skeleton, point_skeleton, Prim, Seg, Skeleton, Vec2};
use crate::graph::{GraphRules, RouteGraph, TerminalInfo};
use crate::model::{
    flip_track, BoardModel, ExistingCopper, Item, Options, PadAttr, PadRef, Pt, Scope, TrackItem,
    ViaItem,
};
use crate::control::RunControl;
use crate::multigraph::MultiGraph;
use crate::negotiate::{
    detect_clinches, overused_conns, BlockInfo, ClinchReport, Negotiator, RipReport,
};
use crate::obstacle::{ObstacleSet, TerminalObstacle};
use crate::pourcheck::{check_pour_reach_until, with_routed_copper, PourReach};
use crate::relax::{corridor_static, relax, Relaxer, RunOpts, SlotNesting, ZeroSlack};
use crate::search::{
    default_expansion_cap, ghost, search, tier_weight, via_weight, Goal, SearchOutcome,
    SearchParams, SlotPath,
};
use crate::sketch::{
    build_layer_cdt_masked, sketch_from_crossings, stack_order, unanchored_bends, AnchorRef,
    ConstraintSet, LayerSketch, Sketch,
};
use crate::smooth::{deadline_after, hand_outside, outside_len, terminal_skeletons};
use crate::stubs::{attach as attach_stubs, plan_stubs, with_stubs, StubOut, StubPlan};
use crate::taut::TautError;
use crate::vias::{via_fallback_used, via_sites_stubs, via_sizes, ViaSize};
use crate::wires::{extract_wires, Terminal, Wire};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
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

/// DG diagnosis (profile only): a short class of a failure reason, for the per-iteration
/// histogram [`IterProfile::fail_classes`]: `pinch site-site` (taut cannot join two anchors),
/// `wall pad` (a wall grazed mid-span), `check viahole` (self-check against a via drill),
/// `sketch`, `junction <class>`, … Labels and numbers are dropped. O(len).
pub fn reason_class(reason: &str) -> String {
    if let Some(rest) = reason.strip_prefix("junction: ") {
        return format!("junction {}", reason_class(rest));
    }
    let kind = |label: &str| -> String {
        label
            .trim()
            .split(':')
            .next()
            .unwrap_or("")
            .trim_start_matches('(')
            .to_string()
    };
    if let Some(rest) = reason.strip_prefix("infeasible: ") {
        if let Some(w) = rest.strip_prefix("wall ") {
            // depth bucket: below the 5 um build margin, a graze, or the chord through the core
            let depth = w
                .rsplit_once(" by ")
                .and_then(|(_, d)| d.trim_end_matches(" nm").parse::<f64>().ok());
            let bucket = match depth {
                Some(d) if d < 5_000.0 => " <5um",
                Some(d) if d < 100_000.0 => " graze",
                Some(_) => " deep",
                None => "",
            };
            return format!("wall {}{bucket}", kind(w));
        }
        if let Some((a, b)) = rest.split_once(" <-> ") {
            let (mut ka, mut kb) = (kind(a), kind(b));
            if kb < ka {
                std::mem::swap(&mut ka, &mut kb);
            }
            return format!("pinch {ka}-{kb}");
        }
        return format!("infeasible {}", rest.split_whitespace().next().unwrap_or(""));
    }
    if let Some(rest) = reason.strip_prefix("self-check: ") {
        return format!("check {}", kind(rest));
    }
    if let Some((head, _)) = reason.split_once(':') {
        return head.to_string();
    }
    reason
        .split_whitespace()
        .take_while(|w| !w.chars().any(|c| c.is_ascii_digit()))
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
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

fn kicad(p: Vec2) -> [i64; 2] {
    [p.x.round() as i64, (-p.y).round() as i64]
}

fn term_out(t: &TermRef) -> TermOut {
    TermOut {
        kind: match t.kind {
            TermKind::Pad(_) | TermKind::Stub { .. } => "pad",
            TermKind::Via(_) => "via",
            TermKind::Copper(_) => "copper",
            TermKind::Dangling(..) => "dangling",
            TermKind::Pour(_) => "pour",
        }
        .into(),
        label: t.label.clone(),
        at: kicad(t.at),
    }
}

fn bits(p: Vec2) -> (u64, u64) {
    ((p.x + 0.0).to_bits(), (p.y + 0.0).to_bits())
}

/// One routed connection after metrization and self-check.
#[derive(Clone, Debug)]
struct ConnOk {
    /// Worst G1 angle of a continued dangling end, `None` without one (A14).
    g1: Option<f64>,
    /// Tracks of every leg and the connection's vias, in leg order.
    items: Vec<Item>,
    len: f64,
    out: f64,
    pattern: String,
    anchors: usize,
    crossings: usize,
    vias: usize,
    /// Layer ids of the legs (dedup, in path order).
    layers: Vec<u32>,
    /// Via positions, KiCad frame nm.
    via_sites: Vec<[i64; 2]>,
    /// A5: the path ends in a T-junction; its measured angle (degrees, 0..=90).
    junction: Option<f64>,
}

/// One metrized leg: its tracks and the via that ends it (if any).
#[derive(Clone, Debug)]
struct LegOk {
    /// Worst angle (deg) between a continued dangling end's hand direction and the leg's tangent
    /// there (A14); 0 without one.
    g1: f64,
    items: Vec<Item>,
    len: f64,
    out: f64,
    pattern: String,
    anchors: usize,
    crossings: usize,
}

#[derive(Clone, Debug)]
enum Res {
    Ok(ConnOk),
    Fail {
        reason: String,
        /// Class-independent failure: never searched again.
        perm: bool,
        blocking: Option<[i64; 2]>,
    },
}

fn fail(reason: impl Into<String>) -> Res {
    Res::Fail {
        reason: reason.into(),
        perm: false,
        blocking: None,
    }
}

fn taut_reason(e: &TautError) -> Res {
    match e {
        TautError::Sleeve(s) => fail(format!("sleeve: {s}")),
        TautError::InsideObstacle(s) => Res::Fail {
            reason: format!("inside obstacle: {s}"),
            perm: true,
            blocking: None,
        },
        TautError::Infeasible(s) => fail(format!("infeasible: {s}")),
        TautError::Cap => fail("infeasible: pass cap"),
    }
}

struct Ctx<'a> {
    model: &'a BoardModel,
    opts: &'a Options,
    /// Routable layer ids, sorted; a layer index is the position in this list (P7: the obstacle
    /// set is passed explicitly, `Ctx` does not hold it).
    layers: &'a [u32],
    conns: &'a [Connection],
    checker: &'a Checker,
    deadline: Instant,
}

impl Ctx<'_> {
    /// Deadline passed or run cancelled (O(1)); every deadline site of the pass uses this.
    fn stop(&self) -> bool {
        crate::control::expired(self.deadline)
    }
}

struct Metrized {
    /// `(live connection index, result)` for every connection in `routed`.
    res: Vec<(usize, Res)>,
    relax_iterations: u32,
    rounds: u32,
    /// Seconds spent in the self-check rounds (profile).
    check_s: f64,
    timed_out: bool,
    /// Zero-slack identification pass (`ZsSpec` without corridor): the static obstacles that
    /// pinch the relaxed wire, `(anchors, walls)` (`corridor_obstacles`).
    zs_corridor: Option<(Vec<usize>, Vec<usize>)>,
    /// A5: `(connection, sibling)` junctions the metrization rejected (angle below
    /// `steiner_min_angle_deg`, end off the sibling's copper); `usize::MAX` = hand copper.
    rejects: Vec<(usize, usize)>,
}

/// Zero-slack corridor of live connection `live` (one leg) for [`metrize_zs`]: margin 0 against
/// the listed anchors / walls of the layer obstacles, `None` = against every obstacle
/// (identification).
struct ZsSpec {
    live: usize,
    anchors: Option<Vec<usize>>,
    walls: Option<Vec<usize>>,
}

/// Crossings of the live path of leg `k` of `g` in the direction of travel (graph edges,
/// `t` from the slot index).
fn path_crossings(g: &RouteGraph, k: usize) -> Option<Vec<Crossing>> {
    let (cross, faces, _) = g.path_of(k)?;
    if cross.is_empty() {
        return Some(Vec::new());
    }
    // A junction leg (A5) has no face after its last (half) crossing.
    if faces.len() != cross.len() + 1 && faces.len() != cross.len() {
        return None;
    }
    Some(
        cross
            .iter()
            .zip(faces)
            .map(|(&(e, slot), &prev)| {
                let ge = &g.edges[e];
                let (left, right) = g.crossing_lr(e, prev);
                let from_a = (slot as f64 + 1.0) / (ge.cross.len() as f64 + 1.0);
                let t = if left == ge.a { from_a } else { 1.0 - from_a };
                Crossing { left, right, t }
            })
            .collect(),
    )
}

/// Offset (nm) of the sleeve polyline's interior points along the direction of travel, which
/// takes a point that lies exactly on an edge of `base` off it without changing its order
/// relative to the other wires on the edge.
const POLY_SHIFT_NM: f64 = 1.0;

/// Crossings of `base` (the triangulation of the real obstacles: dormant via sites deleted) by
/// the polyline `start -> slot points of cr -> end`. The polyline follows the slot path through
/// the same faces of `g`, so it is in the slot path's homotopy class relative to every vertex, in
/// particular relative to every real obstacle, and the polylines of different wires keep the
/// order of their slots; the free (dormant) site vertices the slot path winds around no longer
/// constrain the sketch (M3 §5). `None` on a degenerate position. O(|cr| + crossings).
fn base_crossings(
    g: &RouteGraph,
    cr: &[Crossing],
    start: Vec2,
    end: Vec2,
    base: &LayerSketch,
) -> Option<Vec<Crossing>> {
    let mut pts: Vec<Vec2> = Vec::with_capacity(cr.len() + 2);
    pts.push(start);
    pts.extend(cr.iter().map(|c| crossing_point(g, c)));
    pts.push(end);
    let n = pts.len();
    let mut poly = pts.clone();
    for i in 1..n - 1 {
        let d = pts[i + 1] - pts[i - 1];
        let l = d.len();
        if l > 0.0 {
            poly[i] = pts[i] + d * (POLY_SHIFT_NM / l);
        }
    }
    base.cdt.crossings(&poly).ok()
}

/// Crossings of the live path of leg `k` of `g` with `base` (see [`base_crossings`]).
fn leg_crossings(
    g: &RouteGraph,
    k: usize,
    start: Vec2,
    goal: Vec2,
    base: &LayerSketch,
) -> Option<Vec<Crossing>> {
    let cr = path_crossings(g, k)?;
    if cr.is_empty() {
        // A direct leg along an edge of the graph: the straight segment between the terminals.
        return Some(base.cdt.crossings(&[start, goal]).unwrap_or_default());
    }
    base_crossings(g, &cr, start, goal, base)
}

/// Nesting of the legs `legs` (`(leg id, reduced crossings of the sketch triangulation base)`)
/// of one layer, read off the order of their crossings along the edges of `base` incident to
/// an anchor: the wires nearer the anchor on a fan edge are nested inside. As
/// `relax::slot_nesting_of`, but on the triangulation of the real obstacles, so a wire that
/// passes a pad beyond a free site vertex is still nested outside the wires between it and the pad.
/// O(Σ crossings · log).
fn base_nesting(legs: &[(usize, &[Crossing])], base: &LayerSketch, out: &mut SlotNesting) {
    use crate::geom::Orient;
    // edge (min, max) -> [(leg, fraction from the min vertex)]
    let mut on_edge: BTreeMap<(usize, usize), Vec<(usize, f64)>> = BTreeMap::new();
    let frac_min = |c: &Crossing| if c.left < c.right { c.t } else { 1.0 - c.t };
    for &(k, cs) in legs {
        for c in cs {
            on_edge
                .entry((c.left.min(c.right), c.left.max(c.right)))
                .or_default()
                .push((k, frac_min(c)));
        }
    }
    let mut both: BTreeSet<(usize, usize)> = BTreeSet::new();
    for &(k, cs) in legs {
        for c in cs {
            let key = (c.left.min(c.right), c.left.max(c.right));
            let list = &on_edge[&key];
            for (v, sigma) in [(c.left, Orient::Ccw), (c.right, Orient::Cw)] {
                let Some(anchor) = base.point_to_anchor[v] else {
                    continue;
                };
                let dist = |f: f64| if v == key.0 { f } else { 1.0 - f };
                let mine = dist(frac_min(c));
                let mut inner: Vec<(f64, usize)> = list
                    .iter()
                    .filter(|&&(j, f)| j != k && dist(f) < mine)
                    .map(|&(j, f)| (dist(f), j))
                    .collect();
                inner.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                out.inner
                    .entry((k, AnchorRef { anchor, sigma }))
                    .or_default()
                    .push(inner.into_iter().map(|x| x.1).collect());
                match out.sides.insert((k, anchor), sigma) {
                    Some(s0) if s0 != sigma => {
                        both.insert((k, anchor));
                    }
                    _ => {}
                }
            }
        }
    }
    for key in both {
        out.sides.remove(&key);
    }
}

/// Sketch triangulations of the real obstacles per layer: the layer's anchors minus the dormant
/// via-site anchors, plus its terminals. O(Σ_L N_L log N_L).
fn real_sketches(obs: &ObstacleSet, mg: &MultiGraph) -> Vec<Arc<LayerSketch>> {
    mg.layers
        .iter()
        .map(|c| {
            let lo = obs.layer(c.layer);
            build_layer_cdt_masked(lo, &c.terminals, ConstraintSet::Routing, &|i| {
                let a = &lo.anchors[i];
                a.site.is_none() || a.enabled
            })
            .map(Arc::new)
            .unwrap_or_else(|_| c.ls.clone())
        })
        .collect()
}

/// D-6: the same-net vias `pour` lists (`pour.vias`, model vias of the connection's net) as
/// goal terminals of a pour connection, restricted to the main fill component when `reach` is
/// given. O(vias * listed).
fn listed_pour_vias(
    model: &BoardModel,
    zi: usize,
    net: i32,
    routable: &[u32],
    reach: Option<&PourReach>,
) -> Vec<TermRef> {
    let all = model.all_layer_ids();
    let listed = &model.pours[zi].vias;
    model
        .vias
        .iter()
        .enumerate()
        .filter(|(_, v)| v.net == net && listed.contains(&v.uuid))
        .filter(|(_, v)| reach.is_none_or(|r| r.main_vias.get(&zi).is_some_and(|m| m.contains(&v.uuid))))
        .filter_map(|(vi, v)| {
            let layers: Vec<u32> = crate::model::via_layers(&v.layers, &all)
                .into_iter()
                .filter(|l| routable.contains(l))
                .collect();
            (!layers.is_empty()).then(|| TermRef {
                kind: TermKind::Via(vi),
                label: v.uuid.clone(),
                at: v.at.to_vec2(),
                layers,
            })
        })
        .collect()
}

fn legs_of(mg: &MultiGraph, conn: usize) -> &[usize] {
    mg.conn_wires.get(conn).map_or(&[][..], |v| v.as_slice())
}

/// The two terminals of leg `ord` of live connection `conn` (P6): the real terminals at the
/// path ends, a `Terminal::Site` at every via; `None` when the path and the site list disagree.
fn leg_terminals(
    cx: &Ctx,
    mg: &MultiGraph,
    via_at: &BTreeMap<usize, Vec2>,
    conn: usize,
    ord: usize,
    nlegs: usize,
) -> Option<(Terminal, Terminal)> {
    let c = &cx.conns[conn];
    let sites = mg.sites_of(conn);
    let site = |s: usize| Terminal::Site {
        site: s,
        at: via_at.get(&s).copied().unwrap_or(mg.sites[s].at),
    };
    let jn = mg.junction_of(conn);
    let start = if ord == 0 {
        // A5: a reversed junction path runs from the `to` terminal.
        terminal_of(if jn.is_some_and(|j| j.reversed) {
            &c.to
        } else {
            &c.from
        })
    } else {
        site(*sites.get(ord - 1)?)
    };
    let pour = matches!(c.to.kind, TermKind::Pour(_));
    let goal = if let (true, Some(j)) = (ord + 1 == nlegs, jn) {
        // A5: the junction point (the metrization moves it onto the sibling's chain).
        Terminal::Junction { at: j.at }
    } else if ord + 1 == nlegs && !pour {
        terminal_of(&c.to)
    } else if let Some(&s) = sites.get(ord) {
        site(s)
    } else if ord + 1 == nlegs {
        // D-6: a pour connection that ended on a same-net via the pour lists (no new via)
        let k = *mg.conn_wires.get(conn)?.get(ord)?;
        let li = mg.leg_layer(k)?;
        let (_, _, (_, gv)) = mg.layers[li].g.path_of(k)?;
        let at = mg.layers[li].g.verts[gv].at;
        let vi = cx.model.vias.iter().position(|v| (v.at.to_vec2() - at).len() <= 2.0 && v.net == c.net)?;
        Terminal::Via { via: vi, at }
    } else {
        return None;
    };
    Some((start, goal))
}

/// Metrization state kept across PathFinder iterations (Fix-B2, design note
/// `m2-metrize-design.md` §2.2, `m3-layers-vias-design.md` §5): one wire per *leg* (P2), wire id
/// = leg id `k` of the `MultiGraph`. Only the wires whose sketch, nesting, zero-slack corridor or
/// neighbourhood changed are re-tautened, re-quantised and re-checked; the others keep their
/// chains, items and results. A connection is `Ok` iff every one of its legs passed.
#[derive(Clone)]
struct MetrState {
    rx: Relaxer,
    /// Leg ids of each live connection as last installed (empty: none).
    conn_legs: Vec<Vec<usize>>,
    /// Per leg: layer index, connection.
    leg_layer: Vec<usize>,
    leg_conn: Vec<usize>,
    /// Last sketch sequence per leg (`None`: not in the relaxer).
    seqs: Vec<Option<Vec<AnchorRef>>>,
    ncross: Vec<usize>,
    /// Reduced crossings of the sketch triangulation per leg (the nesting input).
    bcross: Vec<Vec<Crossing>>,
    /// Quantised result of the current chain, before the self-check.
    ok: Vec<Option<LegOk>>,
    /// Failure of the leg (taut, quantisation, sketch or self-check).
    fail: Vec<Option<Res>>,
    /// `ok[k]` must be (re-)checked: its chain changed since its last check, or the check failed.
    pending: Vec<bool>,
    /// Items of the legs that passed their self-check (and whose connection is complete),
    /// indexed once and kept across calls: a round checks only the pending legs against it,
    /// O(changed * q) instead of re-walking every emitted item.
    idx: ItemIndex,
    /// Ids in `idx` per leg (empty: the leg is not indexed).
    item_ids: Vec<Vec<usize>>,
    /// Grid candidates visited by the self-checks so far.
    check_visited: usize,
    /// Sketch triangulations of the real obstacles, with the sorted active sites they were
    /// built for (rebuilt when a via is added or released).
    base: Option<(Vec<usize>, Vec<Arc<LayerSketch>>)>,
    /// A5, per live connection: the junction point its last leg is installed with (on the
    /// sibling's chain after refinement), and the angle measured at it.
    jat: Vec<Option<Vec2>>,
    jangle: Vec<Option<f64>>,
    /// A5, per live connection: the junction point fell back to its base point because the
    /// refined point's leg did not relax (`revert_failed_junctions`); it is not refined again
    /// until the connection's legs change.
    jpin: Vec<bool>,
    /// Via positions moved off their site by the via-relaxation pass (A13), site -> math frame
    /// nm; empty during the negotiation.
    via_at: BTreeMap<usize, Vec2>,
}

impl MetrState {
    fn new(cx: &Ctx, obs: &ObstacleSet) -> MetrState {
        MetrState {
            rx: Relaxer::with_layers(cx.layers, obs),
            conn_legs: vec![Vec::new(); cx.conns.len()],
            leg_layer: Vec::new(),
            leg_conn: Vec::new(),
            seqs: Vec::new(),
            ncross: Vec::new(),
            bcross: Vec::new(),
            ok: Vec::new(),
            fail: Vec::new(),
            pending: Vec::new(),
            idx: ItemIndex::new(cx.checker.cell()),
            item_ids: Vec::new(),
            check_visited: 0,
            base: None,
            jat: vec![None; cx.conns.len()],
            jangle: vec![None; cx.conns.len()],
            jpin: vec![false; cx.conns.len()],
            via_at: BTreeMap::new(),
        }
    }

    /// The sketch triangulations for the current via occupancy of `mg`. O(S) when cached,
    /// O(Σ_L N_L log N_L) after a via was added or released.
    fn base_for(&mut self, obs: &ObstacleSet, mg: &MultiGraph) -> Vec<Arc<LayerSketch>> {
        let key: Vec<usize> = (0..mg.sites.len())
            .filter(|&s| mg.owner[s].is_some())
            .collect();
        if let Some((k, v)) = &self.base {
            if *k == key {
                return v.clone();
            }
        }
        let v = real_sketches(obs, mg);
        self.base = Some((key, v.clone()));
        v
    }

    /// Grow the per-leg tables to hold leg `k`. O(1) amortised.
    fn ensure(&mut self, k: usize) {
        if self.seqs.len() <= k {
            let n = k + 1;
            self.leg_layer.resize(n, 0);
            self.leg_conn.resize(n, 0);
            self.seqs.resize(n, None);
            self.ncross.resize(n, 0);
            self.bcross.resize(n, Vec::new());
            self.ok.resize(n, None);
            self.fail.resize(n, None);
            self.pending.resize(n, false);
            self.item_ids.resize(n, Vec::new());
        }
    }

    /// Remove leg `k`'s items from the index. O(cells of its items).
    fn unindex(&mut self, k: usize) {
        for id in std::mem::take(&mut self.item_ids[k]) {
            self.idx.remove(id);
        }
    }

    fn drop_leg(&mut self, k: usize) {
        self.unindex(k);
        if self.seqs[k].is_some() {
            self.rx.remove(k);
        }
        self.seqs[k] = None;
        self.ok[k] = None;
        self.fail[k] = None;
        self.pending[k] = false;
    }
}

/// A5: a junction point that moves less than this (nm) is not re-relaxed.
const JUNCTION_MOVE_TOL_NM: f64 = 1_000.0;
/// A5: absolute allowance (nm) on top of `expert.steiner_len_slack` times the search estimate
/// before a junction's metrized length counts as an overshoot.
const JUNCTION_LEN_ABS_NM: f64 = 1_000_000.0;
/// A5: least distance (nm) within which a chain that misses an edge's segment still counts as
/// being at that edge (see `record_chain_cross`; otherwise 3 track pitches of the leg).
const JUNCTION_NEAR_MIN_NM: f64 = 500_000.0;
/// A5: junction refinement rounds per metrization (each re-relaxes the moved junction legs).
const JUNCTION_ROUNDS: usize = 3;
/// A5: arcs of a chain are sampled at this many points (junction base point, chain crossings).
const JUNCTION_ARC_SAMPLES: usize = 16;
/// A5: a sibling track is split at a junction only when both pieces are at least this long (nm;
/// shorter arc pieces would not survive the integer-nm mid point).
const JUNCTION_SPLIT_MIN_NM: f64 = 20_000.0;

/// Step 1 of [`metrize_zs`] for leg `k` (layer `li`) of live connection `i`: install its wire
/// when `install` (fresh, or its junction point moved), sketch it on `ls` and hand a changed
/// anchor sequence to the relaxer. `false` when the sketch failed (the leg is dropped with the
/// reason). O(crossings + sleeve).
#[allow(clippy::too_many_arguments)]
fn sketch_leg(
    cx: &Ctx,
    mg: &MultiGraph,
    st: &mut MetrState,
    ls: &LayerSketch,
    i: usize,
    k: usize,
    li: usize,
    start_t: &Terminal,
    goal_t: &Terminal,
    install: bool,
) -> bool {
    let c = &cx.conns[i];
    if install {
        st.leg_layer[k] = li;
        st.leg_conn[k] = i;
        st.rx.replace(
            k,
            Wire {
                id: k,
                net: c.net,
                layer: cx.layers[li],
                width: c.width,
                hand: vec![start_t.at(), goal_t.at()],
                hand_len: (goal_t.at() - start_t.at()).len(),
                start: start_t.clone(),
                goal: goal_t.clone(),
                kept_uuids: Vec::new(),
            },
        );
    }
    let g = &mg.layers[li].g;
    let Some(path) = leg_crossings(g, k, start_t.at(), goal_t.at(), ls) else {
        st.drop_leg(k);
        st.fail[k] = Some(fail("sketch: inconsistent slot path"));
        return false;
    };
    let (start, goal) = (start_t.at(), goal_t.at());
    if let Some(p) = unanchored_bends(start, goal, &path, ls).first() {
        st.drop_leg(k);
        st.fail[k] = Some(fail(format!(
            "sketch: path bends at a wall split vertex ({}, {})",
            kicad(*p)[0],
            kicad(*p)[1]
        )));
        return false;
    }
    let s = sketch_from_crossings(k, start, goal, path, ls);
    st.ncross[k] = s.crossings.len();
    st.bcross[k] = s.crossings.clone();
    if st.seqs[k].as_ref() != Some(&s.seq) {
        st.rx.set(k, &s.seq);
        st.seqs[k] = Some(s.seq);
        st.unindex(k);
        st.ok[k] = None;
        st.fail[k] = None;
        st.pending[k] = false;
    }
    true
}

/// A5: `(junction leg, sibling leg)` pairs of the routed junction connections whose legs are
/// both installed. O(routed).
fn junction_pairs(mg: &MultiGraph, st: &MetrState, routed: &[usize]) -> Vec<(usize, usize)> {
    routed
        .iter()
        .filter_map(|&i| {
            let (s, ord) = mg.junction_of(i)?.sibling?;
            let kb = *st.conn_legs[i].last()?;
            let ka = *st.conn_legs.get(s)?.get(ord)?;
            (st.seqs.get(kb)?.is_some() && st.seqs.get(ka)?.is_some()).then_some((kb, ka))
        })
        .collect()
}

/// The slot nesting of the present legs on the sketch triangulations; a junction leg and its
/// sibling are not nested against each other (same net, the junction leg ends on the sibling's
/// copper, so neither may push the other). O(Σ crossings · log + entries · pairs).
fn metr_nesting(
    st: &MetrState,
    base: &[Arc<LayerSketch>],
    present: &[usize],
    pairs: &[(usize, usize)],
) -> SlotNesting {
    let mut nesting = SlotNesting::default();
    for (li, sk) in base.iter().enumerate() {
        let legs: Vec<(usize, &[Crossing])> = present
            .iter()
            .filter(|&&k| st.leg_layer[k] == li)
            .map(|&k| (k, st.bcross[k].as_slice()))
            .collect();
        if !legs.is_empty() {
            base_nesting(&legs, sk, &mut nesting);
        }
    }
    if !pairs.is_empty() {
        // Mutation only: the map's iteration order does not reach the result.
        for ((k, _), lists) in nesting.inner.iter_mut() {
            for &(kb, ka) in pairs {
                let other = if *k == kb {
                    ka
                } else if *k == ka {
                    kb
                } else {
                    continue;
                };
                for l in lists.iter_mut() {
                    l.retain(|&x| x != other);
                }
            }
        }
    }
    nesting
}

/// Distance from `p` to a primitive. O(1).
fn point_prim_dist(p: Vec2, prim: &Prim) -> f64 {
    match prim {
        Prim::Seg(s) => crate::geom::point_seg(p, s),
        Prim::Arc(a) => crate::geom::point_arc(p, a),
    }
}

/// Parameter interval `[t0, t1]` of segment `a b` inside triangle `tri` (boundary included),
/// `None` when it misses it. O(1).
fn clip_seg_tri(a: Vec2, b: Vec2, tri: &[Vec2; 3]) -> Option<(f64, f64)> {
    let area = (tri[1] - tri[0]).cross(tri[2] - tri[0]);
    if area.abs() < 1e-9 {
        return None;
    }
    let sg = area.signum();
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for k in 0..3 {
        let (p, q) = (tri[k], tri[(k + 1) % 3]);
        let e = q - p;
        let tol = 1e-6 * e.len();
        let fa = sg * e.cross(a - p);
        let fb = sg * e.cross(b - p);
        if fa < -tol && fb < -tol {
            return None;
        }
        if fa < -tol {
            t0 = t0.max(fa / (fa - fb));
        } else if fb < -tol {
            t1 = t1.min(fa / (fa - fb));
        }
    }
    (t0 <= t1).then_some((t0, t1))
}

fn closest_on(a: Vec2, b: Vec2, p: Vec2) -> Vec2 {
    let d = b - a;
    let l2 = d.len2();
    if l2 <= 0.0 {
        return a;
    }
    a + d * ((p - a).dot(d) / l2).clamp(0.0, 1.0)
}

/// A5: what a junction of live connection `i` must end on: the sibling leg's current chain
/// (and the sibling's width), or the hand track's constraint edge (and `i`'s width). `None`
/// when the sibling leg has no chain. O(chain).
fn junction_target(
    cx: &Ctx,
    mg: &MultiGraph,
    st: &MetrState,
    i: usize,
    jn: &crate::search::JunctionEnd,
) -> Option<(Vec<Prim>, f64)> {
    match jn.sibling {
        Some((s, ord)) => {
            let ka = *st.conn_legs.get(s)?.get(ord)?;
            st.seqs.get(ka)?.as_ref()?;
            let ch = st.rx.chains().get(ka)?.as_ref().ok()?;
            Some((ch.prims.clone(), cx.conns[s].width as f64))
        }
        None => {
            let g = &mg.layers.get(jn.layer_idx)?.g;
            let ge = g.edges.get(jn.edge)?;
            Some((
                vec![Prim::Seg(Seg {
                    a: g.verts[ge.a].at,
                    b: g.verts[ge.b].at,
                })],
                cx.conns[i].width as f64,
            ))
        }
    }
}

/// Start of the last straight piece of leg `k`'s chain (where the joining wire comes from), or
/// `start` when it has no chain. O(1).
fn approach_point(st: &MetrState, k: usize, start: Vec2) -> Vec2 {
    match st.rx.chains().get(k) {
        Some(Ok(ch)) => match ch.prims.last() {
            Some(Prim::Seg(s)) => s.a,
            Some(Prim::Arc(a)) => a.point_at(1.0),
            None => start,
        },
        _ => start,
    }
}

/// The usable parameter interval of a straight target piece `a b` for a junction end of
/// width `w`: `w` away from both ends (the tangent points where the sibling wraps an obstacle
/// at exactly its clearance), or the middle of a piece shorter than `2 w`. O(1).
fn seg_interior(a: Vec2, b: Vec2, w: f64) -> (Vec2, Vec2) {
    let l = (b - a).len();
    if l <= 2.0 * w || l <= 0.0 {
        let m = (a + b) * 0.5;
        return (m, m);
    }
    let u = (b - a) * (1.0 / l);
    (a + u * w, b - u * w)
}

/// A5: the junction point of target `tgt` for a wire of width `w` approaching from `pb`: the
/// point of a straight piece of the target nearest `pb`, kept `w` away from the piece's ends
/// ([`seg_interior`]: never on an arc or a tangent point, where the sibling runs at exactly
/// its clearance and an end of the same width would lie on the obstacle's boundary); for a
/// sibling chain restricted to the two faces of the junction edge (the topology the search
/// checked). `None` when no straight piece passes through them. O(prims).
fn junction_point(
    mg: &MultiGraph,
    jn: &crate::search::JunctionEnd,
    tgt: &[Prim],
    pb: Vec2,
    w: f64,
) -> Option<Vec2> {
    if jn.sibling.is_none() {
        let Some(Prim::Seg(sg)) = tgt.first() else {
            return None;
        };
        return Some(closest_on(sg.a, sg.b, pb));
    }
    let g = &mg.layers.get(jn.layer_idx)?.g;
    let win: Vec<[Vec2; 3]> = g
        .edges
        .get(jn.edge)?
        .faces
        .iter()
        .flatten()
        .map(|&f| {
            let v = g.faces[f].v;
            [g.verts[v[0]].at, g.verts[v[1]].at, g.verts[v[2]].at]
        })
        .collect();
    let mut best: Option<(f64, Vec2)> = None;
    for prim in tgt {
        let Prim::Seg(sg) = prim else { continue };
        let (a, b) = seg_interior(sg.a, sg.b, w);
        for tri in &win {
            if let Some((t0, t1)) = clip_seg_tri(a, b, tri) {
                let d = b - a;
                let q = closest_on(a + d * t0, a + d * t1, pb);
                let dq = (q - pb).len();
                if best.is_none_or(|x| dq < x.0) {
                    best = Some((dq, q));
                }
            }
        }
    }
    best.map(|b| b.1)
}

/// A5: the point of the target chain nearest `p` (arcs sampled at their exact points).
/// O(prims · samples).
fn nearest_on_prims(tgt: &[Prim], p: Vec2) -> Option<Vec2> {
    let mut best: Option<(f64, Vec2)> = None;
    for prim in tgt {
        let mut take = |q: Vec2| {
            let d = (q - p).len();
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, q));
            }
        };
        match prim {
            Prim::Seg(sg) => take(closest_on(sg.a, sg.b, p)),
            Prim::Arc(ar) => {
                for n in 0..=2 * JUNCTION_ARC_SAMPLES {
                    take(ar.point_at(n as f64 / (2 * JUNCTION_ARC_SAMPLES) as f64));
                }
            }
        }
    }
    best.map(|b| b.1)
}

/// A5: move every routed junction's point onto its target (nearest the joining wire's approach,
/// see [`junction_point`]); a move of more than [`JUNCTION_MOVE_TOL_NM`] re-installs and
/// re-sketches the junction leg. Returns the moved legs (sorted by connection) and, for those
/// moved to a refined point other than the base point, `(connection, leg, base point)`: the
/// fallback of [`revert_failed_junctions`]. Connections pinned by that fallback are skipped.
/// O(junctions · (chain · samples + sketch)).
fn refine_junctions(
    cx: &Ctx,
    mg: &MultiGraph,
    st: &mut MetrState,
    base: &[Arc<LayerSketch>],
    routed: &[usize],
) -> (Vec<usize>, Vec<(usize, usize, Vec2)>) {
    let mut moved = Vec::new();
    let mut refined = Vec::new();
    for &i in routed {
        let Some(jn) = mg.junction_of(i) else {
            continue;
        };
        if st.jpin[i] {
            continue;
        }
        let Some(&kb) = st.conn_legs[i].last() else {
            continue;
        };
        if st.seqs.get(kb).is_none_or(|q| q.is_none()) {
            continue;
        }
        let (Some(j0), Some((tgt, _))) = (st.jat[i], junction_target(cx, mg, st, i, jn)) else {
            continue;
        };
        let start_t = st.rx.wires()[kb].start.clone();
        let pb = approach_point(st, kb, start_t.at());
        let w = cx.conns[i].width as f64;
        // The base point: the target's point nearest the search's junction estimate (where the
        // sibling's copper crosses the junction edge). A straight-piece point nearer the
        // approach replaces it only when it shortens the approach and stays within the
        // approach distance of it (no sliding along the sibling past where the search put it).
        let Some(b0) = nearest_on_prims(&tgt, jn.at) else {
            continue;
        };
        let reach = (pb - b0).len();
        let j1 = match junction_point(mg, jn, &tgt, pb, w) {
            Some(c) if (pb - c).len() < reach && (c - b0).len() <= reach.max(2.0 * w) => c,
            _ => b0,
        };
        if (j1 - j0).len() <= JUNCTION_MOVE_TOL_NM {
            continue;
        }
        st.jat[i] = Some(j1);
        let li = st.leg_layer[kb];
        st.drop_leg(kb);
        let goal_t = Terminal::Junction { at: j1 };
        if sketch_leg(cx, mg, st, &base[li], i, kb, li, &start_t, &goal_t, true) {
            moved.push(kb);
            if (j1 - b0).len() > JUNCTION_MOVE_TOL_NM {
                refined.push((i, kb, b0));
            }
        }
    }
    (moved, refined)
}

/// A5 (TE): a junction leg moved by [`refine_junctions`] to a refined point (nearest the joining
/// wire's approach) whose chain then fails to relax goes back to its base point (the target's
/// point nearest the search's junction estimate) and is pinned there. The refined point is
/// only a shortening: the joining wire's last piece can come out exactly along the clearance
/// ring of an obstacle it passes (the refined point is the foot of the approach on the
/// sibling), where the relaxation wraps that obstacle the wrong way (a near-full arc) and
/// reports the leg infeasible although the base point routes. Returns the re-sketched legs.
/// O(refined · sketch).
fn revert_failed_junctions(
    cx: &Ctx,
    mg: &MultiGraph,
    st: &mut MetrState,
    base: &[Arc<LayerSketch>],
    refined: &[(usize, usize, Vec2)],
) -> Vec<usize> {
    let mut back = Vec::new();
    for &(i, kb, b0) in refined {
        if !matches!(st.rx.chains().get(kb), Some(Err(_))) {
            continue;
        }
        let start_t = st.rx.wires()[kb].start.clone();
        st.jat[i] = Some(b0);
        st.jpin[i] = true;
        let li = st.leg_layer[kb];
        st.drop_leg(kb);
        let goal_t = Terminal::Junction { at: b0 };
        if sketch_leg(cx, mg, st, &base[li], i, kb, li, &start_t, &goal_t, true) {
            back.push(kb);
        }
    }
    back
}

/// A5: first point where the chain `prims` crosses segment `s` (arcs sampled), `None` when it
/// does not. O(prims · samples).
fn chain_seg_cross(prims: &[Prim], s: &Seg) -> Option<Vec2> {
    let d = s.b - s.a;
    let side = |p: Vec2| d.cross(p - s.a);
    let on_seg = |p: Vec2| {
        let t = (p - s.a).dot(d) / d.len2().max(1e-12);
        (-1e-9..=1.0 + 1e-9).contains(&t)
    };
    let pieces = |p: &Prim| -> Vec<(Vec2, Vec2)> {
        match p {
            Prim::Seg(sg) => vec![(sg.a, sg.b)],
            Prim::Arc(ar) => (0..JUNCTION_ARC_SAMPLES)
                .map(|n| {
                    let f = |k: usize| ar.point_at(k as f64 / JUNCTION_ARC_SAMPLES as f64);
                    (f(n), f(n + 1))
                })
                .collect(),
        }
    };
    for prim in prims {
        for (a, b) in pieces(prim) {
            let (fa, fb) = (side(a), side(b));
            if (fa <= 0.0 && fb >= 0.0) || (fa >= 0.0 && fb <= 0.0) {
                let t = if fa == fb { 0.0 } else { fa / (fa - fb) };
                let q = a + (b - a) * t;
                if on_seg(q) {
                    return Some(q);
                }
            }
        }
    }
    None
}

/// A5: the chain point nearest segment `s` when it lies within `tol` of it. O(prims · samples).
fn chain_seg_near(prims: &[Prim], s: &Seg, tol: f64) -> Option<Vec2> {
    let mut best: Option<(f64, Vec2)> = None;
    let mut take = |q: Vec2| {
        let d = crate::geom::point_seg(q, s);
        if best.is_none_or(|b| d < b.0) {
            best = Some((d, q));
        }
    };
    for prim in prims {
        match prim {
            Prim::Seg(sg) => {
                take(closest_on(sg.a, sg.b, s.a));
                take(closest_on(sg.a, sg.b, s.b));
                take(sg.a);
                take(sg.b);
            }
            Prim::Arc(ar) => {
                for n in 0..=JUNCTION_ARC_SAMPLES {
                    take(ar.point_at(n as f64 / JUNCTION_ARC_SAMPLES as f64));
                }
            }
        }
    }
    best.filter(|b| b.0 <= tol).map(|b| b.1)
}

/// A5: after a metrization, record for every routed leg where its chain crosses each edge of its
/// slot path ([`MultiGraph::set_chain_cross`]), the junction estimate of later searches.
/// O(Σ crossings · prims).
fn record_chain_cross(mg: &mut MultiGraph, st: &MetrState, routed: &[usize]) {
    for &i in routed {
        for &k in &st.conn_legs[i] {
            let Some(Ok(ch)) = st.rx.chains().get(k) else {
                continue;
            };
            let Some(li) = mg.leg_layer(k) else {
                continue;
            };
            let g = &mg.layers[li].g;
            let Some((cross, _, _)) = g.path_of(k) else {
                continue;
            };
            // A chain that passes a dormant-site end of the edge instead of crossing it still
            // has copper there when it passes within a few track pitches.
            let c = &st.rx.wires()[k];
            let near = (3.0 * (c.width as f64 + mg.rules.net_rule(c.net).clearance as f64))
                .max(JUNCTION_NEAR_MIN_NM);
            let pts: Vec<Option<Vec2>> = cross
                .iter()
                .map(|&(e, _)| {
                    let ge = &g.edges[e];
                    let s = Seg {
                        a: g.verts[ge.a].at,
                        b: g.verts[ge.b].at,
                    };
                    chain_seg_cross(&ch.prims, &s).or_else(|| chain_seg_near(&ch.prims, &s, near))
                })
                .collect();
            mg.set_chain_cross(k, pts);
        }
    }
}

/// Acute angle (degrees, 0..=90) between two directions; 90 when either is degenerate. O(1).
fn acute_angle(u: Vec2, v: Vec2) -> f64 {
    let (lu, lv) = (u.len(), v.len());
    if lu <= 1.0 || lv <= 0.0 {
        return 90.0;
    }
    (u.dot(v) / (lu * lv)).abs().min(1.0).acos().to_degrees()
}

/// A5: every quantised junction leg must end on its target (within a quarter of the narrower
/// width, so the end lies inside the sibling's copper) at an angle of at least
/// `expert.steiner_min_angle_deg` against the target's tangent; otherwise the leg fails and the
/// pair is recorded in `rejects` (the next search leaves that junction out). O(junctions · chain).
fn validate_junctions(
    cx: &Ctx,
    mg: &MultiGraph,
    st: &mut MetrState,
    routed: &[usize],
    rejects: &mut Vec<(usize, usize)>,
) {
    for &i in routed {
        let Some(jn) = mg.junction_of(i) else {
            continue;
        };
        let Some(&kb) = st.conn_legs[i].last() else {
            continue;
        };
        // An end the relaxation finds inside an obstacle is the junction point's fault, not a
        // class-independent failure of the connection: retry without that junction.
        if let Some(Some(Res::Fail { reason, perm, .. })) = st.fail.get_mut(kb) {
            if *perm {
                *perm = false;
                *reason = format!("junction: {reason}");
                rejects.push((i, jn.sibling.map_or(usize::MAX, |x| x.0)));
            }
            continue;
        }
        if st.ok.get(kb).is_none_or(|o| o.is_none()) {
            continue;
        }
        let Some(j) = st.jat[i] else { continue };
        let wb = cx.conns[i].width as f64;
        let verdict: Option<(String, bool)> = match junction_target(cx, mg, st, i, jn) {
            None => Some(("junction: sibling not relaxed".into(), false)),
            Some((tgt, wa)) => {
                let mut near: Option<(f64, &Prim)> = None;
                for p in &tgt {
                    let d = point_prim_dist(j, p);
                    if near.is_none_or(|n| d < n.0) {
                        near = Some((d, p));
                    }
                }
                match near {
                    None => Some(("junction: empty sibling chain".into(), false)),
                    Some((d, _)) if d > 0.25 * wa.min(wb) => Some((
                        format!("junction: end {:.1} um off the sibling copper", d * 1e-3),
                        true,
                    )),
                    Some((_, prim)) => {
                        let tan = match prim {
                            Prim::Seg(sg) => sg.b - sg.a,
                            Prim::Arc(ar) => {
                                let r = j - ar.c;
                                Vec2 { x: -r.y, y: r.x }
                            }
                        };
                        let start = st.rx.wires()[kb].start.at();
                        let ang = acute_angle(j - approach_point(st, kb, start), tan);
                        st.jangle[i] = Some(ang);
                        let min = cx.opts.expert.steiner_min_angle_deg;
                        let len: f64 = st.conn_legs[i]
                            .iter()
                            .filter_map(|&k| st.ok.get(k).and_then(|o| o.as_ref()).map(|o| o.len))
                            .sum();
                        let cap = cx.opts.expert.steiner_len_slack.max(1.0) * jn.est_len
                            + JUNCTION_LEN_ABS_NM;
                        if ang + 1e-9 < min {
                            Some((format!("junction: angle {ang:.1} deg < {min:.0} deg"), true))
                        } else if len > cap {
                            Some((
                                format!(
                                    "junction: metrized {:.2} mm > {:.2} mm (search estimate {:.2} mm)",
                                    len * 1e-6,
                                    cap * 1e-6,
                                    jn.est_len * 1e-6
                                ),
                                true,
                            ))
                        } else {
                            None
                        }
                    }
                }
            }
        };
        if let Some((reason, reject)) = verdict {
            st.unindex(kb);
            st.ok[kb] = None;
            st.fail[kb] = Some(fail(reason));
            st.pending[kb] = false;
            if reject {
                rejects.push((i, jn.sibling.map_or(usize::MAX, |x| x.0)));
            }
        }
    }
}

/// Split `items`' track on `layer` that passes through `p` (KiCad frame) into two pieces at
/// `p` (the sibling side of a T-junction), unless `p` is already an end or a piece would be
/// shorter than [`JUNCTION_SPLIT_MIN_NM`]. O(items).
fn split_track_at(items: &mut Vec<Item>, layer: u32, p: crate::geom::quant::IVec2) {
    let pf = p.to_f64();
    let mut best: Option<(usize, f64)> = None;
    for (n, it) in items.iter().enumerate() {
        if let Item::Track(t) = it {
            if t.layer != layer {
                continue;
            }
            let d = point_prim_dist(pf, &dequantise(&t.prim));
            if d <= 2.0 && best.is_none_or(|b| d < b.1) {
                best = Some((n, d));
            }
        }
    }
    let Some((n, _)) = best else { return };
    let Item::Track(t) = items[n].clone() else {
        return;
    };
    let (a, b) = (t.prim.start(), t.prim.end());
    if a == p
        || b == p
        || (a.to_f64() - pf).len() < JUNCTION_SPLIT_MIN_NM
        || (b.to_f64() - pf).len() < JUNCTION_SPLIT_MIN_NM
    {
        return;
    }
    let pieces = match t.prim {
        TrackPrim::Seg { a, b } => [TrackPrim::Seg { a, b: p }, TrackPrim::Seg { a: p, b }],
        TrackPrim::Arc { start, end, .. } => {
            let Prim::Arc(ar) = dequantise(&t.prim) else {
                return;
            };
            let v = pf - ar.c;
            let ang = v.y.atan2(v.x);
            let rel = ((ang - ar.a_start) * ar.sigma.sign()).rem_euclid(std::f64::consts::TAU);
            if ar.sweep <= 0.0 || rel <= 0.0 || rel >= ar.sweep {
                return;
            }
            let f = rel / ar.sweep;
            [
                TrackPrim::Arc {
                    start,
                    mid: round_nm(ar.point_at(0.5 * f)),
                    end: p,
                },
                TrackPrim::Arc {
                    start: p,
                    mid: round_nm(ar.point_at(0.5 * (1.0 + f))),
                    end,
                },
            ]
        }
    };
    let mk = |prim: TrackPrim| Item::Track(TrackItem { prim, ..t.clone() });
    items.splice(n..=n, pieces.into_iter().map(mk));
}

/// A5 epilogue of [`metrize_zs`]: a junction connection whose sibling did not come out `Ok`
/// fails too (to a fixed point over chains of attachments), and every `Ok` sibling's track is
/// split at the junction end. O(res · chain depth + junctions · items).
fn finish_junctions(mg: &MultiGraph, res: &mut [(usize, Res)]) {
    let idx: BTreeMap<usize, usize> = res.iter().enumerate().map(|(n, (i, _))| (*i, n)).collect();
    loop {
        let mut changed = false;
        for n in 0..res.len() {
            let i = res[n].0;
            let Some((s, _)) = mg.junction_of(i).and_then(|j| j.sibling) else {
                continue;
            };
            if !matches!(res[n].1, Res::Ok(_)) {
                continue;
            }
            if !idx.get(&s).is_some_and(|&m| matches!(res[m].1, Res::Ok(_))) {
                res[n].1 = fail("junction: sibling not routed");
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut splits: Vec<(usize, u32, crate::geom::quant::IVec2)> = Vec::new();
    for (i, r) in res.iter() {
        let (Some((s, _)), Res::Ok(b)) = (mg.junction_of(*i).and_then(|j| j.sibling), r) else {
            continue;
        };
        let end = b.items.iter().rev().find_map(|it| match it {
            Item::Track(t) => Some((t.layer, t.prim.end())),
            _ => None,
        });
        if let (Some(&m), Some((layer, p))) = (idx.get(&s), end) {
            splits.push((m, layer, p));
        }
    }
    for (m, layer, p) in splits {
        if let Res::Ok(a) = &mut res[m].1 {
            split_track_at(&mut a.items, layer, p);
        }
    }
}

/// Sketch, relax, quantise and self-check the connections `routed` (live indices) of `mg`,
/// incrementally from `st` (`work`: connections searched or ripped up this iteration).
fn metrize(
    cx: &Ctx,
    obs: &ObstacleSet,
    mg: &MultiGraph,
    routed: &[usize],
    st: &mut MetrState,
    work: &[usize],
) -> Metrized {
    metrize_zs(cx, obs, mg, routed, None, st, work)
}

/// Distance (nm) from the copper of terminal `t` to a plane via at `at` (math frame): from the
/// pad's outline for a pad (0 inside it), else from the terminal point. O(pad vertices).
fn plane_via_gap(model: &BoardModel, t: &TermRef, at: Vec2) -> f64 {
    match t.kind {
        TermKind::Pad(pi) => {
            let pad = &model.pads[pi];
            let layer = t.layers.first().copied().unwrap_or(0);
            point_skeleton(at, &model.skeleton(pad, layer)).max(0.0)
        }
        _ => (at - t.at).len(),
    }
}

/// The via item of site `s` for connection `conn` (KiCad frame).
fn via_item(mg: &MultiGraph, at: Vec2, s: usize, net: i32, conn: usize) -> Item {
    let size = mg.owner[s].map_or(ViaSize { d: 0, drill: 0 }, |(_, z)| z);
    let [x, y] = kicad(at);
    Item::Via(ViaItem {
        kind: "via".into(),
        net,
        at: Pt { x, y },
        d: size.d,
        drill: size.drill,
        layers: Vec::new(),
        group: None,
        wire: Some(conn),
    })
}

/// [`metrize`] with one connection's zero-slack corridor (R6): that connection's (single) leg is
/// relaxed at margin 0 against the corridor and its self-check tolerates [`ZS_QUANT_TOL_NM`]
/// (integer-nm rounding of an exact tangency); every other wire is judged as in [`metrize`].
///
/// Anytime (45 §5a): the first relaxation of a fresh state completes one pass over every wire
/// regardless of the deadline, and a timed-out call still quantises and checks (one round) what
/// it has, so a result exists. `timed_out` is reported either way.
///
/// Complexity: `O(changed legs * (relax + q))` plus `O(legs)` bookkeeping and `SlotNesting`
/// `O(sum occupancy * n_max)` per layer.
fn metrize_zs(
    cx: &Ctx,
    obs: &ObstacleSet,
    mg: &MultiGraph,
    routed: &[usize],
    zs: Option<&ZsSpec>,
    st: &mut MetrState,
    work: &[usize],
) -> Metrized {
    let mut out = Metrized {
        res: Vec::new(),
        relax_iterations: 0,
        rounds: 0,
        check_s: 0.0,
        timed_out: false,
        zs_corridor: None,
        rejects: Vec::new(),
    };
    let m = cx.conns.len();
    let mut in_routed = vec![false; m];
    for &i in routed {
        in_routed[i] = true;
    }
    let mut searched = vec![false; m];
    for &i in work {
        searched[i] = true;
    }
    // Step 0: connections that left, were searched again, or whose legs were re-allocated drop
    // all their legs first (a leg id may move to another connection).
    for i in 0..m {
        let stale = !in_routed[i] || searched[i] || st.conn_legs[i] != legs_of(mg, i);
        if stale {
            for k in std::mem::take(&mut st.conn_legs[i]) {
                st.drop_leg(k);
            }
            st.jat[i] = None;
            st.jangle[i] = None;
            st.jpin[i] = false;
        }
    }
    // Step 1: re-sketch; a leg whose anchor sequence changed starts from scratch.
    let base = st.base_for(obs, mg);
    for &i in routed {
        let ks = legs_of(mg, i).to_vec();
        let installing = st.conn_legs[i].is_empty();
        if installing {
            st.conn_legs[i] = ks.clone();
        }
        let jn = mg.junction_of(i);
        st.jat[i] = jn.map(|j| st.jat[i].unwrap_or(j.at));
        for (ord, &k) in ks.iter().enumerate() {
            st.ensure(k);
            let (Some(li), Some((start_t, mut goal_t))) =
                (mg.leg_layer(k), leg_terminals(cx, mg, &st.via_at, i, ord, ks.len()))
            else {
                st.drop_leg(k);
                st.fail[k] = Some(fail("sketch: inconsistent slot path"));
                continue;
            };
            if let (true, Some(j)) = (ord + 1 == ks.len(), st.jat[i]) {
                goal_t = Terminal::Junction { at: j };
            }
            sketch_leg(cx, mg, st, &base[li], i, k, li, &start_t, &goal_t, installing);
        }
    }
    let nk = st.seqs.len();
    let present: Vec<usize> = (0..nk).filter(|&k| st.seqs[k].is_some()).collect();
    // Stacks from the nesting of the crossings on the sketch triangulation (Fix-B1 (a)); leg ids (unique
    // across layers, so the per-layer nestings merge without collisions).
    let jpairs = junction_pairs(mg, st, routed);
    let nesting = metr_nesting(st, &base, &present, &jpairs);
    st.rx.set_nesting(&nesting);
    let zs_leg = zs.and_then(|z| match legs_of(mg, z.live) {
        [k] if st.seqs.get(*k).is_some_and(|s| s.is_some()) => Some(*k),
        _ => None,
    });
    let zsw: Option<ZeroSlack> = zs.zip(zs_leg).map(|(z, k)| ZeroSlack {
        wire: k,
        anchors: z.anchors.clone(),
        walls: z.walls.clone(),
    });
    // Step 2: incremental Gauss–Seidel.
    let work_legs: Vec<usize> = work
        .iter()
        .flat_map(|&i| st.conn_legs[i].iter().copied())
        .filter(|&k| st.seqs[k].is_some())
        .collect();
    let run = st.rx.run(
        &work_legs,
        obs,
        &RunOpts {
            opts: cx.opts,
            deadline: cx.deadline,
            zs: zsw.as_ref(),
            anytime: true,
        },
    );
    out.relax_iterations = run.sweeps;
    out.timed_out = run.timed_out;
    let mut run_changed = run.changed.clone();
    // A5: move every junction point onto its sibling's chain (closest to the joining wire's
    // approach), re-relax the moved legs; at most JUNCTION_ROUNDS rounds.
    if !jpairs.is_empty() || routed.iter().any(|&i| mg.junction_of(i).is_some()) {
        for _ in 0..JUNCTION_ROUNDS {
            if cx.stop() {
                break;
            }
            let (moved, refined) = refine_junctions(cx, mg, st, &base, routed);
            if moved.is_empty() {
                break;
            }
            let present: Vec<usize> = (0..st.seqs.len()).filter(|&k| st.seqs[k].is_some()).collect();
            let jpairs = junction_pairs(mg, st, routed);
            let nesting = metr_nesting(st, &base, &present, &jpairs);
            st.rx.set_nesting(&nesting);
            let r2 = st.rx.run(
                &moved,
                obs,
                &RunOpts {
                    opts: cx.opts,
                    deadline: cx.deadline,
                    zs: zsw.as_ref(),
                    anytime: true,
                },
            );
            out.relax_iterations += r2.sweeps;
            out.timed_out |= r2.timed_out;
            run_changed.extend(r2.changed.iter().copied());
            // A refined junction point whose leg does not relax falls back to the base point.
            let back = revert_failed_junctions(cx, mg, st, &base, &refined);
            if !back.is_empty() {
                let present: Vec<usize> = (0..st.seqs.len())
                    .filter(|&k| st.seqs[k].is_some())
                    .collect();
                let jpairs = junction_pairs(mg, st, routed);
                let nesting = metr_nesting(st, &base, &present, &jpairs);
                st.rx.set_nesting(&nesting);
                let r3 = st.rx.run(
                    &back,
                    obs,
                    &RunOpts {
                        opts: cx.opts,
                        deadline: cx.deadline,
                        zs: zsw.as_ref(),
                        anytime: true,
                    },
                );
                out.relax_iterations += r3.sweeps;
                out.timed_out |= r3.timed_out;
                run_changed.extend(r3.changed.iter().copied());
            }
        }
    }
    let nk = st.seqs.len();
    let present: Vec<usize> = (0..nk).filter(|&k| st.seqs[k].is_some()).collect();
    if let Some(z) = zsw
        .as_ref()
        .filter(|z| z.anchors.is_none() && z.walls.is_none())
    {
        if let Ok(ch) = &st.rx.chains()[z.wire] {
            let band = obs.margin();
            out.zs_corridor = Some(corridor_static(
                st.rx.wires(),
                z.wire,
                ch,
                obs,
                Some(z),
                band,
            ));
        }
    }
    let zs_wire = zsw.as_ref().map(|z| z.wire);
    // Quantise the legs whose chain changed (or that have no result yet).
    let mut changed = vec![false; nk];
    for &k in &run_changed {
        if k < nk {
            changed[k] = true;
        }
    }
    for &k in &present {
        if !changed[k] && (st.ok[k].is_some() || st.fail[k].is_some()) {
            continue;
        }
        st.unindex(k);
        st.ok[k] = None;
        st.pending[k] = false;
        match &st.rx.chains()[k] {
            Ok(ch) => {
                let q = quantise_chain(&ch.prims);
                if q.is_empty() {
                    st.fail[k] = Some(fail("empty chain after quantisation"));
                    continue;
                }
                let w = &st.rx.wires()[k];
                let conn = st.leg_conn[k];
                let g1 = g1_angle(&q, w);
                if g1 > G1_TOL_DEG {
                    st.fail[k] = Some(fail(format!("g1: {g1:.2} deg off the hand direction")));
                    continue;
                }
                let mut items: Vec<Item> = q
                    .into_iter()
                    .map(|t| {
                        Item::Track(TrackItem {
                            net: w.net,
                            layer: w.layer,
                            width: w.width,
                            prim: flip_track(t),
                            group: None,
                            wire: Some(conn),
                        })
                    })
                    .collect();
                let sks: Vec<Skeleton> = terminal_skeletons(cx.model, w);
                let pattern: String = items
                    .iter()
                    .filter_map(|t| match t {
                        Item::Track(t) => Some(match t.prim {
                            TrackPrim::Seg { .. } => 'S',
                            TrackPrim::Arc { .. } => 'A',
                        }),
                        _ => None,
                    })
                    .collect();
                let len: f64 = items
                    .iter()
                    .filter_map(|t| match t {
                        Item::Track(t) => Some(dequantise(&t.prim).length()),
                        _ => None,
                    })
                    .sum();
                let ord = st.conn_legs[conn].iter().position(|&x| x == k).unwrap_or(0);
                if let Some(&s) = mg.sites_of(conn).get(ord) {
                    let at = st.via_at.get(&s).copied().unwrap_or(mg.sites[s].at);
                    items.push(via_item(mg, at, s, w.net, conn));
                }
                st.ok[k] = Some(LegOk {
                    g1,
                    items,
                    len,
                    out: outside_len(&ch.prims, &sks),
                    pattern,
                    anchors: ch.seq.len(),
                    crossings: st.ncross[k],
                });
                st.fail[k] = None;
                st.pending[k] = true;
            }
            Err(e) => st.fail[k] = Some(taut_reason(e)),
        }
    }

    // A5: a junction leg must end on its sibling's chain at no less than the junction angle.
    validate_junctions(cx, mg, st, routed, &mut out.rejects);

    // Step 3: self-check rounds (fallback granularity = connection) of the pending legs
    // against the checked ones (listed first: a violation is charged to the later item, the
    // wire that moved). A failed leg stays pending for the next call.
    for &k in &present {
        if st.pending[k] && st.ok[k].is_some() {
            st.fail[k] = None;
        }
    }
    let t_check = Instant::now();
    loop {
        if cx.opts.skip_self_check || out.rounds as usize > present.len() {
            break;
        }
        let pend: Vec<usize> = present
            .iter()
            .copied()
            .filter(|&k| st.pending[k] && st.ok[k].is_some())
            .collect();
        if pend.is_empty() {
            break;
        }
        if cx.stop() {
            out.timed_out = true;
            if out.rounds >= 1 {
                break;
            }
        }
        out.rounds += 1;
        // Each pending leg is checked against the persistent index (the checked legs) and the
        // pending legs before it (inserted as the walk proceeds, removed after the round): a
        // violation is charged to the later leg, the one that moved.
        let mut temp: Vec<usize> = Vec::new();
        let mut any = false;
        for &k in &pend {
            let items: Vec<Item> = st.ok[k]
                .as_ref()
                .map(|o| o.items.clone())
                .unwrap_or_default();
            let rep = cx.checker.check_against(&st.idx, &items, &[]);
            st.check_visited += rep.candidates_visited;
            temp.extend(items.iter().map(|it| st.idx.insert(it)));
            for v in &rep.violations {
                if zs_wire == Some(k) && v.gap + ZS_QUANT_TOL_NM >= v.required {
                    continue;
                }
                if st.fail[k].is_none() {
                    st.fail[k] = Some(fail(format!(
                        "self-check: {} (gap {} < {} nm)",
                        v.against, v.gap, v.required
                    )));
                    st.pending[k] = true;
                    any = true;
                }
            }
        }
        for id in temp {
            st.idx.remove(id);
        }
        if !any {
            break;
        }
    }
    out.check_s = t_check.elapsed().as_secs_f64();
    // Index the legs that passed, once their whole connection is complete.
    for &i in routed {
        let ks = st.conn_legs[i].clone();
        let broken = ks
            .iter()
            .any(|&k| st.fail[k].is_some() || st.ok[k].is_none());
        if broken {
            for &k in &ks {
                st.unindex(k);
                st.pending[k] = st.ok[k].is_some();
            }
            continue;
        }
        for &k in &ks {
            if st.pending[k] {
                st.pending[k] = false;
                let ids: Vec<usize> = st.ok[k]
                    .iter()
                    .flat_map(|o| o.items.iter())
                    .map(|it| st.idx.insert(it))
                    .collect();
                st.item_ids[k] = ids;
            }
        }
    }
    for &i in routed {
        let ks = &st.conn_legs[i];
        let r = if ks.is_empty() {
            fail("not metrized")
        } else if let Some(f) = ks.iter().find_map(|&k| st.fail[k].clone()) {
            f
        } else if ks.iter().any(|&k| st.ok[k].is_none() || st.pending[k]) {
            fail("not metrized")
        } else {
            let legs: Vec<&LegOk> = ks.iter().filter_map(|&k| st.ok[k].as_ref()).collect();
            let has_dangling = [&cx.conns[i].from, &cx.conns[i].to]
                .iter()
                .any(|t| matches!(t.kind, TermKind::Dangling(_, Some(_))));
            let sites = mg.sites_of(i);
            let mut layers: Vec<u32> = Vec::new();
            for &k in ks {
                let l = cx.layers[st.leg_layer[k]];
                if !layers.contains(&l) {
                    layers.push(l);
                }
            }
            Res::Ok(ConnOk {
                g1: has_dangling.then(|| legs.iter().map(|o| o.g1).fold(0.0, f64::max)),
                items: legs.iter().flat_map(|o| o.items.iter().cloned()).collect(),
                len: legs.iter().map(|o| o.len).sum(),
                out: legs.iter().map(|o| o.out).sum(),
                pattern: legs
                    .iter()
                    .map(|o| o.pattern.as_str())
                    .collect::<Vec<_>>()
                    .join("|"),
                anchors: legs.iter().map(|o| o.anchors).sum(),
                crossings: legs.iter().map(|o| o.crossings).sum(),
                vias: sites.len(),
                layers,
                via_sites: sites
                    .iter()
                    .map(|&s| kicad(st.via_at.get(&s).copied().unwrap_or(mg.sites[s].at)))
                    .collect(),
                junction: mg.junction_of(i).map(|_| st.jangle[i].unwrap_or(90.0)),
            })
        };
        out.res.push((i, r));
    }
    finish_junctions(mg, &mut out.res);
    out
}

/// A continued dangling end must be left within this angle of the hand direction, deg (A14).
const G1_TOL_DEG: f64 = 1.0;

/// Unit tangent of `p` at its start / end, travel direction (`None` for a degenerate prim).
fn prim_tangent(p: &Prim, at_end: bool) -> Option<Vec2> {
    match p {
        Prim::Seg(s) => s.dir(),
        Prim::Arc(a) => {
            let pt = if at_end { a.end() } else { a.start() };
            let t = (pt - a.c).unit()?.perp();
            Some(if a.sigma == crate::geom::Orient::Ccw { t } else { -t })
        }
    }
}

/// Worst angle (deg) between the hand direction of a continued dangling start / goal terminal
/// of `w` and the tangent of the quantised chain `q` there (math frame); 0 when `w` has none.
/// O(1).
fn g1_angle(q: &[TrackPrim], w: &Wire) -> f64 {
    let ang = |t: Option<Vec2>, d: Vec2| -> f64 {
        match (t, d.unit()) {
            (Some(t), Some(d)) => t.dot(d).clamp(-1.0, 1.0).acos().to_degrees(),
            _ => 0.0,
        }
    };
    let mut worst = 0.0f64;
    if let (Terminal::Dangling { direction: Some(d), .. }, Some(first)) = (&w.start, q.first()) {
        worst = worst.max(ang(prim_tangent(&dequantise(first), false), *d));
    }
    if let (Terminal::Dangling { direction: Some(d), .. }, Some(last)) = (&w.goal, q.last()) {
        worst = worst.max(ang(prim_tangent(&dequantise(last), true), -*d));
    }
    worst
}

/// Bisection steps of the stub length (`λ` resolution < 3 nm on a board).
const STUB_BISECT_STEPS: u32 = 24;
/// Group name of the emitted stubs.
const PARTIAL_GROUP: &str = "topor-partial";

enum Partial {
    Stubs(PartialOk),
    /// Stubs were drawn but none survived the self-check.
    Dropped,
    /// Stubs were drawn but the deadline passed before they were metrized and checked.
    Skipped,
}

struct PartialOk {
    items: Vec<TrackItem>,
    stubs: usize,
    len: f64,
    out: f64,
    pattern: String,
    anchors: usize,
    crossings: usize,
}

/// Crossing of edge `e` at ghost slot `slot` entered from face `prev` (ghost entries are not in
/// the graph, so the slot interval count is `cross.len() + 2`).
fn ghost_crossing(g: &RouteGraph, e: usize, slot: usize, prev: usize) -> Crossing {
    let ge = &g.edges[e];
    let (left, right) = if ge.faces[0] == Some(prev) {
        (ge.b, ge.a)
    } else {
        (ge.a, ge.b)
    };
    let from_a = (slot.min(ge.cross.len()) as f64 + 1.0) / (ge.cross.len() as f64 + 2.0);
    let t = if left == ge.a { from_a } else { 1.0 - from_a };
    Crossing { left, right, t }
}

fn crossing_point(g: &RouteGraph, c: &Crossing) -> Vec2 {
    let (l, r) = (g.verts[c.left].at, g.verts[c.right].at);
    l + (r - l) * c.t
}

/// Straight stub item (engine frame in, KiCad frame out) of connection `c`.
fn stub_item(c: &Connection, layer: u32, a: Vec2, b: Vec2) -> TrackItem {
    TrackItem {
        net: c.net,
        layer,
        width: c.width,
        prim: flip_track(TrackPrim::Seg {
            a: round_nm(a),
            b: round_nm(b),
        }),
        group: None,
        wire: None,
    }
}

/// One stub candidate: the wire from `start` along `crossings` to the junction `end`.
struct StubSpec {
    conn: usize,
    /// Layer index of the stub.
    layer: usize,
    start: Terminal,
    end: Vec2,
    crossings: Vec<Crossing>,
}

/// Longest stub `q -> p0` (largest `λ`) that stays clear at margin `ε` of the model and `base`.
fn bisect_stub(
    checker: &Checker,
    c: &Connection,
    layer: u32,
    base: &[Item],
    q: Vec2,
    p0: Vec2,
) -> Option<Vec2> {
    let ok = |lam: f64| {
        let p = q + (p0 - q) * lam;
        let mut items: Vec<Item> = base.to_vec();
        items.push(Item::Track(stub_item(c, layer, q, p)));
        let last = items.len() - 1;
        !checker
            .check_items(&items)
            .violations
            .iter()
            .any(|v| v.item == last)
    };
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    if ok(1.0) {
        return Some(p0);
    }
    for _ in 0..STUB_BISECT_STEPS {
        let mid = 0.5 * (lo + hi);
        if ok(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo > 0.0).then(|| q + (p0 - q) * lo)
}

/// A located block: both stubs of a failed connection aim at `p0`.
struct Blk {
    /// Layer index of the stubs (the ghost's first leg).
    layer: usize,
    p0: Vec2,
    /// Reported blocking point (KiCad frame) when `Res::Fail` carries none.
    report: Option<[i64; 2]>,
    /// Face steps the geometric probe ran (`None` for a ghost block).
    steps: Option<usize>,
    /// D6: forward stub start `Q_f` (crossing point of `last_reached`) and the ghost crossings
    /// before the blocked edge; `None` => the stub starts at the terminal.
    fwd: Option<(Vec2, Vec<Crossing>)>,
}

/// Consecutive unchanged iterations (no overuse, no rip-up) after which `route` gives up.
const STUCK_REPEATS: u32 = 3;
/// Zero-slack retry: smallest estimated-length gain (nm) that makes a connection a candidate.
const ZS_GAIN_MIN_NM: f64 = 100_000.0;
/// Zero-slack retry outcome when the wall clock runs out during the pass.
const ZS_DEADLINE: &str = "skipped: deadline";
/// Zero-slack retry: self-check tolerance of the zero-slack wire, nm. An exact tangency rebuilt
/// from integer-nm points (arc centre from three rounded points) is short by under a nanometre;
/// KiCad's own `DRCEpsilon` is 500 nm. The board file cannot express less than 1 nm.
const ZS_QUANT_TOL_NM: i64 = 1;

/// Uniform samples per portal edge of the geometric block probe.
const PROBE_SAMPLES: usize = 16;
/// Extra samples at these distances (nm) from either end of a portal edge (narrow places sit
/// next to the obstacles that are the edge's endpoints).
const PROBE_NEAR_NM: [f64; 6] = [
    400_000.0,
    800_000.0,
    1_200_000.0,
    1_600_000.0,
    2_400_000.0,
    3_200_000.0,
];

fn edge_samples(g: &RouteGraph, e: usize) -> Vec<Vec2> {
    let ge = &g.edges[e];
    let (a, b) = (g.verts[ge.a].at, g.verts[ge.b].at);
    let len = (b - a).len().max(1.0);
    let mut ts: Vec<f64> = (1..=PROBE_SAMPLES)
        .map(|k| k as f64 / (PROBE_SAMPLES as f64 + 1.0))
        .collect();
    for d in PROBE_NEAR_NM {
        if d < 0.5 * len {
            ts.push(d / len);
            ts.push(1.0 - d / len);
        }
    }
    ts.iter().map(|&t| a + (b - a) * t).collect()
}

/// Most face steps the block probe examines per connection (D9).
pub const PROBE_STEP_CAP: usize = 64;

/// First face step of a slot path that no straight wire can cross (the path is search-feasible
/// but geometrically blocked, e.g. a gap between a pad and a wall narrower than the wire):
/// returns the tightest spot of the least-violating crossing. At most `PROBE_STEP_CAP` steps
/// are examined and the deadline is checked before every step (`None` on either limit);
/// `steps` receives the number of steps run. Cost O(steps · (|base| + 784) · q) (D9).
#[allow(clippy::too_many_arguments)]
fn probe_path(
    g: &RouteGraph,
    checker: &Checker,
    c: &Connection,
    layer: u32,
    base: &[Item],
    crossings: &[(usize, usize)],
    from: Vec2,
    to: Vec2,
    deadline: Instant,
    steps: &mut usize,
) -> Option<Vec2> {
    let n = crossings.len();
    for j in 0..=n {
        if *steps >= PROBE_STEP_CAP || crate::control::expired(deadline) {
            return None;
        }
        *steps += 1;
        let a: Vec<Vec2> = if j == 0 {
            vec![from]
        } else {
            edge_samples(g, crossings[j - 1].0)
        };
        let b: Vec<Vec2> = if j == n {
            vec![to]
        } else {
            edge_samples(g, crossings[j].0)
        };
        let mut items = base.to_vec();
        let mut pairs: Vec<(Vec2, Vec2)> = Vec::new();
        for &p in &a {
            for &q in &b {
                items.push(Item::Track(stub_item(c, layer, p, q)));
                pairs.push((p, q));
            }
        }
        let rep = checker.check_items(&items);
        let mut deficit = vec![0.0f64; pairs.len()];
        for v in &rep.violations {
            if v.item >= base.len() {
                let d = &mut deficit[v.item - base.len()];
                *d = d.max((v.required - v.gap) as f64);
            }
        }
        if deficit.contains(&0.0) {
            continue;
        }
        let k = (0..pairs.len()).min_by(|&x, &y| deficit[x].total_cmp(&deficit[y]))?;
        // Pinch: the tightest spot along the least-violating segment.
        let (p, q) = pairs[k];
        let mut items = base.to_vec();
        let dots: Vec<Vec2> = (0..=32).map(|m| p + (q - p) * (m as f64 / 32.0)).collect();
        items.extend(dots.iter().map(|&d| Item::Track(stub_item(c, layer, d, d))));
        // Most obstacles squeezing the dot, then the largest shortfall, then nearest the middle.
        let mut hits: Vec<Vec<&str>> = vec![Vec::new(); dots.len()];
        let mut worst = vec![0.0f64; dots.len()];
        let rep = checker.check_items(&items);
        for v in &rep.violations {
            if v.item >= base.len() {
                let k = v.item - base.len();
                worst[k] = worst[k].max((v.required - v.gap) as f64);
                if !hits[k].contains(&v.against.as_str()) {
                    hits[k].push(v.against.as_str());
                }
            }
        }
        let m = (0..dots.len())
            .max_by(|&x, &y| {
                hits[x]
                    .len()
                    .cmp(&hits[y].len())
                    .then(worst[x].total_cmp(&worst[y]))
                    .then(x.abs_diff(16).cmp(&y.abs_diff(16)).reverse())
            })
            .unwrap_or(0);
        return Some(dots[m]);
    }
    None
}

/// Blocks of the failed connections of the best iteration: ghost blocks (`BlockInfo`) and, for
/// connections that found a slot path which then failed metrization, the geometric probe.
/// Stubs are only drawn on the ghost's / path's first leg (D6 rule): a block on a later leg or
/// at a via site yields no stub.
fn collect_blocks(
    cx: &Ctx,
    mg: &MultiGraph,
    best: &Snap,
    checker: &Checker,
    base: &[Item],
) -> BTreeMap<usize, Blk> {
    let mut out: BTreeMap<usize, Blk> = BTreeMap::new();
    // End of the first leg of `path`: the connection's target, or the first via.
    let leg0_end = |c: &Connection, p: &SlotPath| -> Vec2 {
        match p.sites.first() {
            Some(&(s, _)) if p.legs.len() > 1 || matches!(c.to.kind, TermKind::Pour(_)) => {
                mg.sites[s].at
            }
            _ => c.to.at,
        }
    };
    for (&i, b) in &best.blocked {
        if matches!(best.res.get(i), Some(Res::Fail { perm: true, .. })) {
            continue;
        }
        let Some(gp) = &b.ghost else { continue };
        let Some(leg0) = gp.legs.first() else {
            continue;
        };
        let g0 = &mg.layers[leg0.layer_idx].g;
        let c = &cx.conns[i];
        let Some((li_b, e_b)) = b.edge else {
            // Topological block (no blocked edge, e.g. a bend pinched against a wall): the ghost
            // path is search-infeasible but geometrically probe-able (fallback, note A).
            if b.site.is_none()
                && (leg0.faces.len() == leg0.crossings.len() + 1 || leg0.crossings.is_empty())
            {
                let mut steps = 0;
                if let Some(p0) = probe_path(
                    g0,
                    checker,
                    c,
                    cx.layers[leg0.layer_idx],
                    base,
                    &leg0.crossings,
                    c.from.at,
                    leg0_end(c, gp),
                    cx.deadline,
                    &mut steps,
                ) {
                    out.insert(
                        i,
                        Blk {
                            layer: leg0.layer_idx,
                            p0,
                            report: Some(kicad(p0)),
                            steps: Some(steps),
                            fwd: None,
                        },
                    );
                }
            }
            continue;
        };
        // The blocked edge must lie on the first leg.
        if li_b != leg0.layer_idx {
            continue;
        }
        let Some(k) = leg0.crossings.iter().position(|&(e, _)| e == e_b) else {
            continue;
        };
        if leg0.faces.len() != leg0.crossings.len() + 1 {
            continue;
        }
        let p0 = crossing_point(
            g0,
            &ghost_crossing(g0, leg0.crossings[k].0, leg0.crossings[k].1, leg0.faces[k]),
        );
        // D6: the forward stub starts at the crossing point of `last_reached` (the crossing
        // before the blocked edge) and follows the ghost crossings up to it.
        let fwd = (k > 0).then(|| {
            let last = ghost_crossing(
                g0,
                leg0.crossings[k - 1].0,
                leg0.crossings[k - 1].1,
                leg0.faces[k - 1],
            );
            let cr: Vec<Crossing> = (0..k)
                .map(|j| {
                    ghost_crossing(g0, leg0.crossings[j].0, leg0.crossings[j].1, leg0.faces[j])
                })
                .collect();
            (crossing_point(g0, &last), cr)
        });
        out.insert(
            i,
            Blk {
                layer: leg0.layer_idx,
                p0,
                report: None,
                steps: None,
                fwd,
            },
        );
    }
    for (i, r) in best.res.iter().enumerate() {
        let Res::Fail { perm: false, .. } = r else {
            continue;
        };
        if out.contains_key(&i) || best.blocked.contains_key(&i) {
            continue;
        }
        let Some(Some(path)) = best.paths.get(i) else {
            continue;
        };
        let Some(leg0) = path.legs.first() else {
            continue;
        };
        if leg0.faces.len() != leg0.crossings.len() + 1 && !leg0.crossings.is_empty() {
            continue;
        }
        let c = &cx.conns[i];
        let mut steps = 0;
        if let Some(p0) = probe_path(
            &mg.layers[leg0.layer_idx].g,
            checker,
            c,
            cx.layers[leg0.layer_idx],
            base,
            &leg0.crossings,
            c.from.at,
            leg0_end(c, path),
            cx.deadline,
            &mut steps,
        ) {
            out.insert(
                i,
                Blk {
                    layer: leg0.layer_idx,
                    p0,
                    report: Some(kicad(p0)),
                    steps: Some(steps),
                    fwd: None,
                },
            );
        }
    }
    out
}

/// Stubs of the failed connections of the best iteration (note §6).
fn partial_pass(
    cx: &Ctx,
    obs: &ObstacleSet,
    mg: &MultiGraph,
    blocks: &BTreeMap<usize, Blk>,
    eps_checker: &Checker,
    mut base: Vec<Item>,
    orig: &[usize],
) -> BTreeMap<usize, Partial> {
    let mut result: BTreeMap<usize, Partial> = BTreeMap::new();
    let routed = base.clone();
    let sk_base = real_sketches(obs, mg);
    let mut specs: Vec<StubSpec> = Vec::new();
    for (&i, b) in blocks {
        let c = &cx.conns[i];
        let layer = cx.layers[b.layer];
        let ls = &*sk_base[b.layer];
        let g0 = &mg.layers[b.layer].g;
        for (side, start) in [terminal_of(&c.from), terminal_of(&c.to)]
            .into_iter()
            .enumerate()
        {
            // The reverse stub needs copper of the goal terminal on the stub's layer (a pour
            // target has none: its via is not a terminal).
            if side == 1
                && (matches!(c.to.kind, TermKind::Pour(_)) || !c.to.layers.contains(&layer))
            {
                continue;
            }
            // D6: the forward stub grows from the `last_reached` crossing point through the
            // ghost crossings; the reverse stub from the goal terminal as before.
            let fwd: Option<&(Vec2, Vec<Crossing>)> = if side == 0 { b.fwd.as_ref() } else { None };
            let q = fwd.map_or(start.at(), |f| f.0);
            let Some(p) = bisect_stub(eps_checker, c, layer, &base, q, b.p0) else {
                continue;
            };
            let crossings = match fwd {
                Some(f) => match base_crossings(g0, &f.1, start.at(), p, ls) {
                    Some(cr) => cr,
                    None => continue,
                },
                None => match ls.cdt.crossings(&[q, p]) {
                    Ok(cr) => cr,
                    Err(_) => continue,
                },
            };
            base.push(Item::Track(stub_item(c, layer, q, p)));
            specs.push(StubSpec {
                conn: i,
                layer: b.layer,
                start,
                end: p,
                crossings,
            });
        }
    }
    if specs.is_empty() {
        return result;
    }
    for sp in &specs {
        result.entry(sp.conn).or_insert(Partial::Dropped);
    }
    // Metrize every stub together (one relax), quantise, then self-check at margin 0 against the
    // routed items (earlier) and earlier stubs.
    let mut wires: Vec<Wire> = Vec::new();
    let mut sketches: Vec<Sketch> = Vec::new();
    let mut spec_of: Vec<usize> = Vec::new();
    for (si, sp) in specs.iter().enumerate() {
        let c = &cx.conns[sp.conn];
        let start = sp.start.at();
        let ls = &*sk_base[sp.layer];
        if !unanchored_bends(start, sp.end, &sp.crossings, ls).is_empty() {
            continue;
        }
        let k = wires.len();
        sketches.push(sketch_from_crossings(
            k,
            start,
            sp.end,
            sp.crossings.clone(),
            ls,
        ));
        wires.push(Wire {
            id: k,
            net: c.net,
            layer: cx.layers[sp.layer],
            width: c.width,
            start: sp.start.clone(),
            goal: Terminal::Junction { at: sp.end },
            hand: vec![start, sp.end],
            kept_uuids: Vec::new(),
            hand_len: 0.0,
        });
        spec_of.push(si);
    }
    // A deadline here (or in the relax below) is reported as such: the stubs were not
    // checked, so "failed self-check" would be wrong (seen in the unoptimised build, whose
    // block probe alone runs for seconds).
    let skipped = |result: &mut BTreeMap<usize, Partial>| {
        for p in result.values_mut() {
            *p = Partial::Skipped;
        }
    };
    if wires.is_empty() {
        return result;
    }
    if cx.stop() {
        skipped(&mut result);
        return result;
    }
    let mut orders: HashMap<u32, HashMap<AnchorRef, Vec<usize>>> = HashMap::new();
    for (li, &l) in cx.layers.iter().enumerate() {
        let on: Vec<Sketch> = sketches
            .iter()
            .filter(|sk| wires[sk.wire].layer == l)
            .cloned()
            .collect();
        if !on.is_empty() {
            orders.insert(l, stack_order(&on, &sk_base[li]));
        }
    }
    let r = relax(&wires, &sketches, obs, &orders, cx.opts, cx.deadline);
    if r.timed_out {
        skipped(&mut result);
        return result;
    }
    let n = wires.len();
    let mut emit: Vec<Option<Vec<TrackItem>>> = vec![None; n];
    let mut chains = Vec::new();
    for (k, ch) in r.chains.into_iter().enumerate() {
        match ch {
            Ok(ch) => {
                let q = quantise_chain(&ch.prims);
                if !q.is_empty() {
                    let w = &wires[k];
                    emit[k] = Some(
                        q.into_iter()
                            .map(|t| TrackItem {
                                net: w.net,
                                layer: w.layer,
                                width: w.width,
                                prim: flip_track(t),
                                group: Some(PARTIAL_GROUP.into()),
                                wire: Some(orig[specs[spec_of[k]].conn]),
                            })
                            .collect(),
                    );
                }
                chains.push(Some(ch));
            }
            Err(_) => chains.push(None),
        }
    }
    let mut rounds = 0usize;
    while rounds <= n {
        rounds += 1;
        let mut items = routed.clone();
        let mut owner: Vec<usize> = Vec::new();
        for (k, e) in emit.iter().enumerate() {
            if let Some(e) = e {
                for t in e {
                    items.push(Item::Track(t.clone()));
                    owner.push(k);
                }
            }
        }
        if owner.is_empty() || cx.opts.skip_self_check {
            break;
        }
        let rep = cx.checker.check_items(&items);
        let mut any = false;
        for v in &rep.violations {
            if v.item >= routed.len() && emit[owner[v.item - routed.len()]].take().is_some() {
                any = true;
            }
        }
        if !any {
            break;
        }
    }
    for k in 0..n {
        let (Some(e), Some(ch)) = (emit[k].take(), chains[k].as_ref()) else {
            continue;
        };
        let sks: Vec<Skeleton> = terminal_skeletons(cx.model, &wires[k]);
        let len: f64 = e.iter().map(|t| dequantise(&t.prim).length()).sum();
        let pat: String = e
            .iter()
            .map(|t| match t.prim {
                TrackPrim::Seg { .. } => 'S',
                TrackPrim::Arc { .. } => 'A',
            })
            .collect();
        let out = outside_len(&ch.prims, &sks);
        let ncross = sketches[k].crossings.len();
        let slot = result
            .get_mut(&specs[spec_of[k]].conn)
            .expect("seeded above");
        match slot {
            Partial::Stubs(p) => {
                p.items.extend(e);
                p.stubs += 1;
                p.len += len;
                p.out += out;
                p.pattern = format!("{}|{}", p.pattern, pat);
                p.crossings += ncross;
            }
            Partial::Dropped | Partial::Skipped => {
                *slot = Partial::Stubs(PartialOk {
                    items: e,
                    stubs: 1,
                    len,
                    out,
                    pattern: pat,
                    anchors: 0,
                    crossings: ncross,
                });
            }
        }
    }
    result
}

#[derive(Clone)]
struct Snap {
    routed: usize,
    /// Vias of the routed connections.
    vias: usize,
    len: f64,
    /// Σ outside-pad length of the routed connections.
    out: f64,
    iteration: u32,
    res: Vec<Res>,
    overuse_edges: usize,
    clinches: ClinchReport,
    relax_iterations: u32,
    rounds: u32,
    blocked: BTreeMap<usize, BlockInfo>,
    paths: Vec<Option<SlotPath>>,
    /// The routing graphs and site occupancy as they stood in this iteration (the retry pass
    /// starts from it; the obstacle anchors are re-synced from `mg.owner`, P7).
    mg: MultiGraph,
    /// Metrization state of this iteration (the retry pass continues from it).
    metr: MetrState,
}

/// `(routed, vias, Σlen, Σoutside)` of a result vector.
fn tally(res: &[Res]) -> (usize, usize, f64, f64) {
    res.iter().fold(
        (0usize, 0usize, 0.0f64, 0.0f64),
        |(n, v, l, o), r| match r {
            Res::Ok(k) => (n + 1, v + k.vias, l + k.len, o + k.out),
            _ => (n, v, l, o),
        },
    )
}

/// Best-iteration order (note §4): more routed, then fewer vias, then shorter.
fn better_than(nok: usize, vias: usize, len: f64, b: &Snap) -> bool {
    nok > b.routed || (nok == b.routed && (vias < b.vias || (vias == b.vias && len < b.len)))
}

/// Stop rule (`patience`, `min_gain_pct`): over the last `patience` iterations the best routed
/// count did not grow and the best cost fell by less than `min_gain_pct` percent in total.
/// `hist` holds the best `(routed, cost)` after every iteration. O(1).
pub fn no_gain(hist: &[(usize, f64)], opts: &Options) -> bool {
    let p = opts.patience.max(1) as usize;
    if hist.len() <= p {
        return false;
    }
    let (r0, c0) = hist[hist.len() - 1 - p];
    let (r1, c1) = hist[hist.len() - 1];
    r1 <= r0 && c0 - c1 < opts.min_gain_pct / 100.0 * c0
}

/// Per-run search inputs shared by the loop and the retry pass.
struct Route<'a> {
    starts: &'a [Vec<(usize, usize)>],
    goals: &'a [Goal],
    /// Net of each live connection (obstacle anchor re-sync).
    nets: Vec<i32>,
}

impl Route<'_> {
    fn net_of(&self) -> impl Fn(usize) -> i32 + '_ {
        move |c| self.nets[c]
    }
}

/// Via-reduction cost factor: the per-via cost of the reduction search is this many times the
/// loop's (TopoR lifting, note §6).
pub const REDUCTION_VIA_FACTOR: f64 = 4.0;
// A reduction may lengthen the connection's outside-pad length by at most
// `Options::reduction_len_slack()` (`1 + reduction_len_slack_pct / 100`, default 1.10).

/// Counters of [`via_reduction_pass`].
struct Reduction {
    /// Vias removed.
    removed: usize,
    /// Candidate connections tried.
    candidates: usize,
    /// The deadline cut the pass short.
    timed_out: bool,
}

/// Put the planned paths of the failed (not perm) connections `i` of snapshot `b` with
/// `want(i)` back into `mg` (a path whose via site is taken in `mg` stays out; see
/// [`needs_back`] for when the passes want them); returns them. The
/// negotiation metrizes an iteration with these paths and their vias in the graph and removes
/// them only afterwards, so the snapshot's wires were sketched with them: a re-metrize without
/// them may sketch a routed wire differently and fail it (qmtech: two fine-pitch exits), which
/// rejects every candidate of a pass that keeps all routed connections. Undo with
/// [`take_back`] before the graph is stored or its overuse counted. O(failed · path).
fn put_back(
    cx: &Ctx,
    mg: &mut MultiGraph,
    obs: &mut ObstacleSet,
    b: &Snap,
    want: &dyn Fn(usize) -> bool,
) -> Vec<usize> {
    let mut back = Vec::new();
    for (i, conn) in cx.conns.iter().enumerate() {
        if !want(i) {
            continue;
        }
        let (Res::Fail { perm: false, .. }, Some(Some(p))) = (&b.res[i], b.paths.get(i)) else {
            continue;
        };
        if !mg.conn_wires.get(i).is_none_or(|v| v.is_empty())
            || p.sites.iter().any(|&(s, _)| mg.owner.get(s).copied().flatten().is_some())
        {
            continue;
        }
        mg.insert_path(conn, p, obs);
        back.push(i);
    }
    back
}

/// Whether passes on snapshot `b` need [`put_back`]: a no-op re-metrize of the routed
/// connections on `b.mg` alone fails one of them. Where it does not, the passes keep the graph
/// without the failed paths (their wires constrain nothing that is emitted). One metrization.
fn needs_back(cx: &Ctx, obs: &mut ObstacleSet, rt: &Route, b: &Snap) -> bool {
    let routed = routed_with(b, &[], None);
    let mut metr = b.metr.clone();
    b.mg.sync_anchors(obs, &rt.net_of());
    let mt = metrize(cx, obs, &b.mg, &routed, &mut metr, &[]);
    !mt.timed_out && !ok_kept(&mt, &routed, b, None)
}

/// Remove the paths [`put_back`] inserted. O(Σ path).
fn take_back(mg: &mut MultiGraph, obs: &mut ObstacleSet, back: &[usize]) {
    for &i in back {
        mg.remove_conn(i, obs);
    }
}

/// The connections to metrize for snapshot `b` with `back` put back and `extra` added: the
/// routed ones, `back`, `extra`; sorted. O(m).
fn routed_with(b: &Snap, back: &[usize], extra: Option<usize>) -> Vec<usize> {
    (0..b.res.len())
        .filter(|&i| matches!(b.res[i], Res::Ok(_)) || back.contains(&i) || Some(i) == extra)
        .collect()
}

/// Every connection of `routed` that `b` had routed is routed in `mt`, and so is `c` (when
/// given); the put-back ones may fail. O(|routed|).
fn ok_kept(mt: &Metrized, routed: &[usize], b: &Snap, c: Option<usize>) -> bool {
    mt.res.len() == routed.len()
        && mt.res.iter().all(|(i, r)| {
            matches!(r, Res::Ok(_)) || (Some(*i) != c && !matches!(b.res[*i], Res::Ok(_)))
        })
}

/// `b.res` with the results of `mt` for the connections `b` had routed and for `c`. O(m).
fn merged_res(b: &Snap, mt: Metrized, c: Option<usize>) -> Vec<Res> {
    let mut res = b.res.clone();
    for (i, r) in mt.res {
        if Some(i) == c || matches!(b.res[i], Res::Ok(_)) {
            res[i] = r;
        }
    }
    res
}

/// Via-reduction (M3-R2, TopoR lifting): one pass over the connections with `vias >= 1`, in
/// order `(-vias, id)`. Each leaves the graph and is searched again with the via cost times
/// [`REDUCTION_VIA_FACTOR`] against the final history; the result replaces the old path iff it has
/// fewer vias, metrizes with every previously routed connection still routed, adds no overuse
/// and its outside-pad length is at most `Options::reduction_len_slack()` times the old one. A trial runs
/// on a copy of the best snapshot's graphs and metrization state (P7), so a rejection simply
/// drops it; the obstacle anchors are then re-synced from `best.mg`. Pour-goal connections
/// (their via is the goal) are not candidates. `best` is updated in place and `obs` ends synced
/// to it.
/// Complexity: `O(m_v * (V' log V' + metrize))` plus one graph/state copy per candidate.
fn via_reduction_pass(
    cx: &Ctx,
    obs: &mut ObstacleSet,
    rt: &Route,
    neg: &Negotiator,
    best: &mut Snap,
) -> Reduction {
    let live = cx.conns;
    let m = live.len();
    // A5: a connection with junctions attached keeps its path (removing it would cascade).
    let mut cands: Vec<(usize, usize)> = (0..m)
        .filter_map(|i| match &best.res[i] {
            Res::Ok(k)
                if k.vias >= 1
                    && rt.goals[i].pour_sites.is_empty()
                    && best.mg.dependents(i).is_empty() =>
            {
                Some((k.vias, i))
            }
            _ => None,
        })
        .collect();
    cands.sort_by_key(|&(v, i)| (std::cmp::Reverse(v), i));
    let use_back = !cands.is_empty() && needs_back(cx, obs, rt, best);
    let mut out = Reduction {
        removed: 0,
        candidates: 0,
        timed_out: false,
    };
    for (_, c) in cands {
        if cx.stop() {
            out.timed_out = true;
            break;
        }
        out.candidates += 1;
        let Res::Ok(old) = best.res[c].clone() else {
            continue;
        };
        let mut mg = best.mg.clone();
        mg.sync_anchors(obs, &rt.net_of());
        mg.remove_conn(c, obs);
        let params = SearchParams {
            history: &neg.history,
            site_history: &neg.site_history,
            weight: tier_weight(live[c].tier),
            present_overuse: cx.opts.expert.present_overuse,
            expansion_cap: default_expansion_cap(&mg),
            deadline: cx.deadline,
            eps: None,
            via_cost: cx.opts.via_cost_for(live[c].net) as f64
                * via_weight(live[c].tier)
                * REDUCTION_VIA_FACTOR,
            max_vias: cx.opts.max_vias_per_connection,
            strict_unfit_edges: cx.opts.strict_unfit_edges,
        };
        let p = match search(&mg, obs, &live[c], &rt.starts[c], &rt.goals[c], &params) {
            SearchOutcome::Found(p) if p.vias() < old.vias => p,
            SearchOutcome::Timeout => {
                out.timed_out = true;
                break;
            }
            _ => continue,
        };
        mg.insert_path(&live[c], &p, obs);
        let back = put_back(cx, &mut mg, obs, best, &|_| use_back);
        let routed = routed_with(best, &back, None);
        let mut metr = best.metr.clone();
        let mt = metrize(cx, obs, &mg, &routed, &mut metr, &[c]);
        if mt.timed_out {
            out.timed_out = true;
            break;
        }
        let all_ok = ok_kept(&mt, &routed, best, Some(c));
        take_back(&mut mg, obs, &back);
        let overuse: usize = mg
            .layers
            .iter()
            .map(|l| {
                (0..l.g.edges.len())
                    .filter(|&e| l.g.overuse(e) > 0.0)
                    .count()
            })
            .sum();
        let short = mt.res.iter().any(|(i, r)| {
            *i == c && matches!(r, Res::Ok(k) if k.out <= old.out * cx.opts.reduction_len_slack())
        });
        if !(all_ok && short && overuse <= best.overuse_edges) {
            continue;
        }
        let (relax_iterations, rounds) = (mt.relax_iterations, mt.rounds);
        let res = merged_res(best, mt, Some(c));
        let mut paths = best.paths.clone();
        out.removed += old.vias - p.vias();
        paths[c] = Some(p);
        let (nok, vias, len, outl) = tally(&res);
        best.clinches = detect_clinches(&mg, live, &paths, rt.starts);
        best.routed = nok;
        best.vias = vias;
        best.len = len;
        best.out = outl;
        best.res = res;
        best.overuse_edges = overuse;
        best.relax_iterations = relax_iterations;
        best.rounds = rounds;
        best.paths = paths;
        best.mg = mg;
        best.metr = metr;
    }
    best.mg.sync_anchors(obs, &rt.net_of());
    out
}

/// A5 junction repair, one pass from the best iteration: a connection whose junction the
/// metrization rejected (angle, end off the sibling, overshoot) is failed in that iteration
/// although its pad route (or another junction) is usually free; it is searched once more in the
/// best graph (the rejected pair excluded, `MultiGraph::steiner_reject`) and kept when every
/// routed connection still metrizes. Returns `(repaired, candidates)`. Complexity: at most one
/// search + one incremental metrization per rejected connection; deadline / cancel checked per
/// candidate.
fn junction_repair_pass(
    cx: &Ctx,
    obs: &mut ObstacleSet,
    rt: &Route,
    neg: &Negotiator,
    best: &mut Snap,
) -> (usize, usize) {
    let live = cx.conns;
    let m = live.len();
    let cands: Vec<usize> = (0..m)
        .filter(|&i| {
            matches!(&best.res[i], Res::Fail { reason, perm: false, .. } if reason.starts_with("junction:"))
        })
        .collect();
    let use_back = !cands.is_empty() && needs_back(cx, obs, rt, best);
    let mut fixed = 0usize;
    for &c in &cands {
        if cx.stop() {
            break;
        }
        let mut mg = best.mg.clone();
        mg.sync_anchors(obs, &rt.net_of());
        mg.remove_conn(c, obs);
        if !mg.take_cascaded().is_empty() {
            continue;
        }
        let params = SearchParams {
            history: &neg.history,
            site_history: &neg.site_history,
            weight: tier_weight(live[c].tier),
            present_overuse: cx.opts.expert.present_overuse,
            expansion_cap: default_expansion_cap(&mg),
            deadline: cx.deadline,
            eps: None,
            via_cost: cx.opts.via_cost_for(live[c].net) as f64 * via_weight(live[c].tier),
            max_vias: cx.opts.max_vias_per_connection,
            strict_unfit_edges: cx.opts.strict_unfit_edges,
        };
        let p = match search(&mg, obs, &live[c], &rt.starts[c], &rt.goals[c], &params) {
            SearchOutcome::Found(p) => p,
            SearchOutcome::Timeout => break,
            SearchOutcome::Blocked { .. } => continue,
        };
        mg.insert_path(&live[c], &p, obs);
        let back = put_back(cx, &mut mg, obs, best, &|i| use_back && i != c);
        let routed = routed_with(best, &back, Some(c));
        let mut metr = best.metr.clone();
        let mt = metrize(cx, obs, &mg, &routed, &mut metr, &[c]);
        if mt.timed_out {
            break;
        }
        take_back(&mut mg, obs, &back);
        if !ok_kept(&mt, &routed, best, Some(c)) {
            continue;
        }
        let (relax_iterations, rounds) = (mt.relax_iterations, mt.rounds);
        let res = merged_res(best, mt, Some(c));
        let mut paths = best.paths.clone();
        paths[c] = Some(p);
        let (nok, vias, len, outl) = tally(&res);
        best.clinches = detect_clinches(&mg, live, &paths, rt.starts);
        best.routed = nok;
        best.vias = vias;
        best.len = len;
        best.out = outl;
        best.res = res;
        best.overuse_edges = mg
            .layers
            .iter()
            .map(|l| (0..l.g.edges.len()).filter(|&e| l.g.overuse(e) > 0.0).count())
            .sum();
        best.relax_iterations = relax_iterations;
        best.rounds = rounds;
        best.paths = paths;
        best.mg = mg;
        best.metr = metr;
        fixed += 1;
    }
    best.mg.sync_anchors(obs, &rt.net_of());
    (fixed, cands.len())
}

/// Via-relaxation steps per via (note §5): Weiszfeld iterations of one via.
pub const VIA_RELAX_STEPS: usize = 8;
/// A relaxation step is kept only when the connection's length falls by at least this (nm).
pub const VIA_RELAX_MIN_GAIN_NM: f64 = 1000.0;
/// Fraction of the distance from the site vertex to the boundary of its star a via may use:
/// keeps the moved via off the link edges (the checker has the last word).
const VIA_STAR_FRACTION: f64 = 0.9;

/// Counters of [`via_relaxation_pass`].
struct ViaRelax {
    /// Candidate vias (one per via of a routed connection that is not a pour goal).
    vias: usize,
    /// Vias that moved.
    moved: usize,
    /// Total length gained, nm.
    gain_nm: f64,
    timed_out: bool,
}

impl ViaRelax {
    fn status(&self) -> String {
        let head = format!(
            "moved {} of {} vias, -{:.0} um",
            self.moved,
            self.vias,
            self.gain_nm / 1000.0
        );
        if self.timed_out {
            format!("skipped: timeout ({head})")
        } else {
            head
        }
    }
}

/// Largest `t` in `[0, 1]` such that `v + t * (target - v)` lies in the star of vertex `v` in
/// layer graph `g` (the union of its incident faces inside the outline) and no further than
/// [`VIA_STAR_FRACTION`] of the way to the star's boundary; 0 when the direction leaves the
/// star through a face outside the outline. The star is star-shaped around `v`, so the ray meets
/// its boundary once: the link edge of the face whose wedge contains the direction.
/// O(deg v).
fn star_clip(g: &RouteGraph, v: usize, target: Vec2) -> f64 {
    let o = g.verts[v].at;
    let d = target - o;
    if d.len() == 0.0 {
        return 0.0;
    }
    for &f in g.faces_of_vertex(v) {
        let face = &g.faces[f];
        let Some(i) = face.v.iter().position(|&x| x == v) else {
            continue;
        };
        let b = g.verts[face.v[(i + 1) % 3]].at;
        let c = g.verts[face.v[(i + 2) % 3]].at;
        // direction inside the (counter-clockwise) wedge o -> b, o -> c
        if (b - o).cross(d) >= 0.0 && d.cross(c - o) >= 0.0 {
            if !face.inside {
                return 0.0;
            }
            let bc = c - b;
            let den = d.cross(bc);
            if den == 0.0 {
                return 0.0;
            }
            let t = (b - o).cross(bc) / den;
            return (t * VIA_STAR_FRACTION).clamp(0.0, 1.0);
        }
    }
    0.0
}

/// Weiszfeld step of the via at `p` between the anchors `a1`, `a2` (the last taut anchor of
/// each leg before the via; a straight leg: its terminal): the weighted point of `a1 a2`.
/// O(1).
fn weiszfeld_step(p: Vec2, a1: Vec2, a2: Vec2) -> Vec2 {
    let w1 = 1.0 / (p - a1).len().max(1.0);
    let w2 = 1.0 / (p - a2).len().max(1.0);
    (a1 * w1 + a2 * w2) * (1.0 / (w1 + w2))
}

/// Via position relaxation (A13, design note `m4-capacity-steiner-design.md` §5): after the
/// via reduction, one sweep over the vias in site order. A via joins two legs on layers `L1`,
/// `L2`; its position is moved by up to [`VIA_RELAX_STEPS`] Weiszfeld steps towards the segment
/// between the last taut anchors of its two legs, clipped into the star of its site vertex in
/// both layer graphs (so the slot paths stay valid: only the final faces change, not the
/// homotopy w.r.t. any obstacle). A step is kept iff the moved via fits
/// (`Checker::via_fits`: holes, keepouts, clearance, edge), the connection metrizes again with
/// every other connection still routed, its length falls by at least
/// [`VIA_RELAX_MIN_GAIN_NM`] and its outside-pad length does not grow; the first rejected step
/// ends that via. The site vertex keeps its CDT position (fully free placement is backlog);
/// only the emitted via and its two legs move. Pour-goal connections (their via is the goal)
/// are skipped. Deterministic: sequential, sorted by site.
/// Complexity: `O(V_via * steps * metrize(1 conn))` plus one state copy per step; the deadline
/// and the cancel flag are checked before every step.
fn via_relaxation_pass(cx: &Ctx, obs: &mut ObstacleSet, rt: &Route, best: &mut Snap) -> ViaRelax {
    let live = cx.conns;
    let m = live.len();
    let mut out = ViaRelax {
        vias: 0,
        moved: 0,
        gain_nm: 0.0,
        timed_out: false,
    };
    // (site, connection, ordinal of the leg ending at the site)
    let mut vias: Vec<(usize, usize, usize)> = Vec::new();
    for c in 0..m {
        if !matches!(best.res[c], Res::Ok(_)) || !rt.goals[c].pour_sites.is_empty() {
            continue;
        }
        let sites = best.mg.sites_of(c);
        if legs_of(&best.mg, c).len() != sites.len() + 1 {
            continue;
        }
        vias.extend(sites.iter().enumerate().map(|(j, &s)| (s, c, j)));
    }
    vias.sort_unstable();
    out.vias = vias.len();
    // The graph as the snapshot was metrized (the moves change no path, only `via_at`).
    let mut mgx = best.mg.clone();
    mgx.sync_anchors(obs, &rt.net_of());
    let use_back = !vias.is_empty() && needs_back(cx, obs, rt, best);
    let back = put_back(cx, &mut mgx, obs, best, &|_| use_back);
    for (s, c, j) in vias {
        let ks = legs_of(&best.mg, c).to_vec();
        let (k1, k2) = (ks[j], ks[j + 1]);
        let (Some(l1), Some(l2)) = (best.mg.leg_layer(k1), best.mg.leg_layer(k2)) else {
            continue;
        };
        let site_at = best.mg.sites[s].at;
        let (Some(v1), Some(v2)) = (
            best.mg.vertex_of(l1, site_at),
            best.mg.vertex_of(l2, site_at),
        ) else {
            continue;
        };
        let size = best.mg.owner[s].map_or(ViaSize { d: 0, drill: 0 }, |(_, z)| z);
        let mut moved = false;
        for _ in 0..VIA_RELAX_STEPS {
            if cx.stop() {
                out.timed_out = true;
                break;
            }
            let cur = best.metr.via_at.get(&s).copied().unwrap_or(site_at);
            let (Some(Ok(ch1)), Some(Ok(ch2))) =
                (best.metr.rx.chains().get(k1), best.metr.rx.chains().get(k2))
            else {
                break;
            };
            let (Some(last), Some(first)) = (ch1.prims.last(), ch2.prims.first()) else {
                break;
            };
            let a1 = match last {
                Prim::Seg(sg) => sg.a,
                Prim::Arc(a) => a.start(),
            };
            let a2 = match first {
                Prim::Seg(sg) => sg.b,
                Prim::Arc(a) => a.end(),
            };
            let target = weiszfeld_step(cur, a1, a2);
            let t = star_clip(&best.mg.layers[l1].g, v1, target)
                .min(star_clip(&best.mg.layers[l2].g, v2, target));
            let goal_p = site_at + (target - site_at) * t;
            // back off towards the current position until the moved via fits
            let Some(new) = [1.0, 0.5, 0.25]
                .iter()
                .map(|f| cur + (goal_p - cur) * *f)
                .take_while(|q| (*q - cur).len() >= 1000.0)
                .find(|q| cx.checker.via_fits(*q, size))
            else {
                break;
            };
            let Res::Ok(old) = best.res[c].clone() else {
                break;
            };
            let routed = routed_with(best, &back, None);
            let mut metr = best.metr.clone();
            metr.via_at.insert(s, new);
            let mt = metrize(cx, obs, &mgx, &routed, &mut metr, &[c]);
            if mt.timed_out {
                out.timed_out = true;
                break;
            }
            let all_ok = ok_kept(&mt, &routed, best, Some(c));
            let gain = mt.res.iter().find_map(|(i, r)| match r {
                Res::Ok(k) if *i == c && k.out <= old.out => Some(old.len - k.len),
                _ => None,
            });
            if !all_ok || !gain.is_some_and(|g| g >= VIA_RELAX_MIN_GAIN_NM) {
                break;
            }
            let (relax_iterations, rounds) = (mt.relax_iterations, mt.rounds);
            let res = merged_res(best, mt, Some(c));
            let (nok, vias_n, len, outl) = tally(&res);
            if nok != best.routed {
                break;
            }
            moved = true;
            out.gain_nm += gain.unwrap_or(0.0);
            best.vias = vias_n;
            best.len = len;
            best.out = outl;
            best.res = res;
            best.relax_iterations = relax_iterations;
            best.rounds = rounds;
            best.metr = metr;
        }
        if moved {
            out.moved += 1;
        }
        if out.timed_out {
            break;
        }
    }
    best.mg.sync_anchors(obs, &rt.net_of());
    out
}

/// Shove distances tried per pinch (nm), smallest first: the first one that lets the pinched
/// connection fit is kept, so a via moves no further than it must (to this resolution).
const SHOVE_STEPS_NM: [f64; 6] = [
    25_000.0, 50_000.0, 100_000.0, 200_000.0, 400_000.0, 800_000.0,
];
/// Shove rounds per connection: each keeps one via move (a pinch freed, possibly passing the
/// push on to a neighbour).
const SHOVE_ROUNDS: usize = 4;

/// Bisection steps between the last shove distance that did not help and the first that did
/// (the kept move is within `step / 2^SHOVE_BISECT` of the shortest that helps).
const SHOVE_BISECT: usize = 3;

/// Shove directions, in the order tried: straight away from the pinch partner, then 45° and
/// 90° to either side (sliding past it). `d` is a unit vector. O(1).
fn shove_dirs(d: Vec2) -> [Vec2; 5] {
    let rot = |a: f64| Vec2 {
        x: d.x * a.cos() - d.y * a.sin(),
        y: d.x * a.sin() + d.y * a.cos(),
    };
    let q = std::f64::consts::FRAC_PI_4;
    [d, rot(q), rot(-q), rot(2.0 * q), rot(-2.0 * q)]
}

/// Where a via site (CDT vertex at `home`, now at `cur`) lands when shoved `dist` along `dir`:
/// clipped into the star of its vertex in every layer graph; `None` when that moves it less
/// than 1 um or a via of `size` does not fit there. O(layers · deg + q).
#[allow(clippy::too_many_arguments)]
fn shove_goal(
    cx: &Ctx,
    mg: &MultiGraph,
    home: Vec2,
    cur: Vec2,
    dir: Vec2,
    dist: f64,
    size: ViaSize,
) -> Option<Vec2> {
    let target = cur + dir * dist;
    let mut t = 1.0f64;
    for (li, l) in mg.layers.iter().enumerate() {
        t = t.min(mg.vertex_of(li, home).map_or(0.0, |v| star_clip(&l.g, v, target)));
    }
    let goal = home + (target - home) * t;
    ((goal - cur).len() >= 1000.0 && cx.checker.via_fits(goal, size)).then_some(goal)
}

/// Metrize with site `s` moved from `cur` to `goal` (anchors and emitted via), re-tautening
/// `work`; the anchors are back at `cur` on return. O(metrize).
#[allow(clippy::too_many_arguments)]
fn shove_try(
    cx: &Ctx,
    obs: &mut ObstacleSet,
    mg: &MultiGraph,
    metr: &MetrState,
    routed: &[usize],
    work: &[usize],
    s: usize,
    cur: Vec2,
    goal: Vec2,
) -> (Metrized, MetrState) {
    obs.move_site(&mg.site_anchor[s], goal);
    let mut m2 = metr.clone();
    m2.via_at.insert(s, goal);
    m2.base = None;
    let mt = metrize(cx, obs, mg, routed, &mut m2, work);
    obs.move_site(&mg.site_anchor[s], cur);
    (mt, m2)
}

/// Counters of [`via_shove_pass`].
struct ViaShove {
    /// Failed connections with an `infeasible` pinch against another connection's via.
    candidates: usize,
    /// Of those, routed after a shove.
    routed: usize,
    /// Vias moved.
    shoved: usize,
    timed_out: bool,
}

impl ViaShove {
    fn status(&self) -> String {
        let head = format!(
            "routed {} of {} pinched connections, shoved {} vias",
            self.routed, self.candidates, self.shoved
        );
        if self.timed_out {
            format!("skipped: timeout ({head})")
        } else {
            head
        }
    }
}

/// The two obstacle labels of an `infeasible: <a> <-> <b>` metrization failure.
fn pinch_labels(reason: &str) -> Option<(&str, &str)> {
    reason.strip_prefix("infeasible: ")?.split_once(" <-> ")
}

/// Site index of a `site:<i>` anchor label.
fn site_of_label(label: &str) -> Option<usize> {
    label.strip_prefix("site:")?.parse().ok()
}

/// Position of the pinch partner `label` nearest to `near`: a site (its current anchor
/// position), `start` / `goal` (the nearest terminal or own via of connection `c`) or an anchor
/// of that label on any layer. O(anchors) for a pad label.
fn pinch_partner(
    cx: &Ctx,
    obs: &ObstacleSet,
    mg: &MultiGraph,
    c: usize,
    label: &str,
    near: Vec2,
) -> Option<Vec2> {
    let nearest = |pts: &mut dyn Iterator<Item = Vec2>| {
        pts.min_by(|a, b| (*a - near).len().total_cmp(&(*b - near).len()))
    };
    if let Some(s) = site_of_label(label) {
        return obs.site_at(mg.site_anchor.get(s)?);
    }
    if label == "start" || label == "goal" {
        let conn = &cx.conns[c];
        let own = mg.sites_of(c).iter().filter_map(|&s| obs.site_at(&mg.site_anchor[s]));
        return nearest(&mut [conn.from.at, conn.to.at].into_iter().chain(own));
    }
    nearest(&mut cx.layers.iter().flat_map(|&l| {
        obs.layer(l)
            .anchors
            .iter()
            .filter(|a| a.label == label)
            .map(|a| a.c)
    }))
}

/// Via shove, one pass from the best iteration (after the zero-slack retry): a planned trace
/// that does not fit at its width pushes the vias in its way aside. Candidates: connections
/// whose metrization failed with `infeasible: a <-> b` where `a` or `b` is the via site of
/// another routed connection (not a pour goal, not junction-linked). Per candidate `c` the graph
/// is rebuilt as the negotiation metrized the snapshot (every failed connection's planned slot
/// path back in it, so every wire keeps its anchor sequence; `c` is skipped when its path no
/// longer fits back) and metrized. Then up to [`SHOVE_ROUNDS`] rounds: the first pinched
/// failure (`c` first, then a routed connection a move broke) pushes the via of its pinch away
/// from the pinch partner ([`shove_dirs`]: straight away, then 45° / 90° to either side) by
/// [`SHOVE_STEPS_NM`] (smallest first, then [`SHOVE_BISECT`] bisections down to the shortest
/// distance that helps), clipped into the star of its site vertex in every layer graph (so every
/// slot path stays valid) and only where `Checker::via_fits`; the obstacle anchors move with the via ([`ObstacleSet::move_site`]), so
/// the other wires see it where it is emitted. A move is kept when fewer connections fail, or
/// when `c` routes and no more fail. The result is kept only when `c` and every routed
/// connection metrize (self-checked); else every move is undone. The other failed connections
/// keep their failure and leave the graph again. Deterministic: candidates by id, failures and
/// sites in pinch order. Complexity: `O(C · rounds · dirs · (|steps| + bisect) · metrize)` for `C` candidates; the
/// deadline and the cancel flag are checked before every candidate and step.
fn via_shove_pass(cx: &Ctx, obs: &mut ObstacleSet, rt: &Route, best: &mut Snap) -> ViaShove {
    let live = cx.conns;
    let m = live.len();
    let mut out = ViaShove {
        candidates: 0,
        routed: 0,
        shoved: 0,
        timed_out: false,
    };
    let net_of = rt.net_of();
    let movable = |b: &Snap, s: usize, c: usize| -> Option<(usize, ViaSize)> {
        let (o, size) = b.mg.owner.get(s).copied().flatten()?;
        (o != c
            && matches!(b.res[o], Res::Ok(_))
            && rt.goals[o].pour_sites.is_empty()
            && !b.mg.has_junction_link(o))
        .then_some((o, size))
    };
    let pinched = |b: &Snap, c: usize, reason: &str| -> bool {
        pinch_labels(reason).is_some_and(|(x, y)| {
            [x, y]
                .iter()
                .filter_map(|l| site_of_label(l))
                .any(|s| movable(b, s, c).is_some())
        })
    };
    let cands: Vec<usize> = (0..m)
        .filter(|&i| {
            matches!(&best.res[i], Res::Fail { reason, perm: false, .. } if pinched(best, i, reason))
        })
        .collect();
    out.candidates = cands.len();
    let use_back = !cands.is_empty() && needs_back(cx, obs, rt, best);
    'conn: for &c in &cands {
        if cx.stop() {
            out.timed_out = true;
            break;
        }
        // The graph as the negotiation metrized the snapshot ([`put_back`]) when that matters,
        // else the best graph; `c`'s planned path in both.
        let mut mg = best.mg.clone();
        mg.sync_anchors(obs, &net_of);
        let back = put_back(cx, &mut mg, obs, best, &|i| use_back || i == c);
        let Some(p) = back.contains(&c).then(|| best.paths[c].clone()).flatten() else {
            best.mg.sync_anchors(obs, &net_of);
            continue;
        };
        let was_ok = |i: usize| matches!(best.res[i], Res::Ok(_));
        let routed = routed_with(best, &back, None);
        let mut metr = best.metr.clone();
        let mt = metrize(cx, obs, &mg, &routed, &mut metr, &[c]);
        if mt.timed_out {
            out.timed_out = true;
            break;
        }
        if mt.res.len() != routed.len() {
            best.mg.sync_anchors(obs, &net_of);
            continue;
        }
        // Shove rounds: the first pinched failure (`c` first, then a routed connection a move
        // broke) pushes the via of its pinch; a move is kept when fewer connections fail, or
        // when `c` routes and no more fail (the next round frees the one it pinched). If `c`
        // or anything routed still fails after the rounds, every move is undone; the other
        // failed connections may stay failed.
        let bad = |mt: &Metrized| -> Vec<(usize, String)> {
            let mut v: Vec<(usize, String)> = mt
                .res
                .iter()
                .filter_map(|(i, r)| match r {
                    Res::Fail { reason, .. } if *i == c || was_ok(*i) => Some((*i, reason.clone())),
                    _ => None,
                })
                .collect();
            v.sort_by_key(|(i, _)| (*i != c, *i));
            v
        };
        let (mut mt, mut metr) = (mt, metr);
        // (site, position before the move), in move order
        let mut moved: Vec<(usize, Vec2)> = Vec::new();
        let undo = |obs: &mut ObstacleSet, mg: &MultiGraph, moved: &[(usize, Vec2)]| {
            for &(s, p) in moved.iter().rev() {
                obs.move_site(&mg.site_anchor[s], p);
            }
        };
        'rounds: for _ in 0..SHOVE_ROUNDS {
            let fails = bad(&mt);
            if fails.is_empty() {
                break;
            }
            let c_failed = fails.iter().any(|(i, _)| *i == c);
            for (f, reason) in &fails {
                let Some((la, lb)) = pinch_labels(reason) else {
                    continue;
                };
                for (me, other) in [(la, lb), (lb, la)] {
                    let Some(s) = site_of_label(me) else {
                        continue;
                    };
                    let Some((o, size)) = mg.owner.get(s).copied().flatten().filter(|&(o, _)| {
                        o != *f
                            && (o == c || was_ok(o))
                            && rt.goals[o].pour_sites.is_empty()
                            && !mg.has_junction_link(o)
                    }) else {
                        continue;
                    };
                    let home = mg.sites[s].at;
                    let Some(cur) = obs.site_at(&mg.site_anchor[s]) else {
                        continue;
                    };
                    let Some(partner) = pinch_partner(cx, obs, &mg, *f, other, cur) else {
                        continue;
                    };
                    let away = cur - partner;
                    if away.len() == 0.0 {
                        continue;
                    }
                    let mut work = vec![c, *f, o];
                    work.sort_unstable();
                    work.dedup();
                    let helps = |after: &[(usize, String)]| {
                        let c_freed = c_failed && !after.iter().any(|(i, _)| *i == c);
                        after.len() < fails.len() || (c_freed && after.len() <= fails.len())
                    };
                    for dir in shove_dirs(away * (1.0 / away.len())) {
                        // `lo`: the longest distance tried that did not help (or did not fit)
                        let mut lo = 0.0f64;
                        for step in SHOVE_STEPS_NM {
                            if cx.stop() {
                                undo(obs, &mg, &moved);
                                out.timed_out = true;
                                break 'conn;
                            }
                            let Some(goal) = shove_goal(cx, &mg, home, cur, dir, step, size)
                            else {
                                lo = step;
                                continue;
                            };
                            let (mt2, m2) = shove_try(cx, obs, &mg, &metr, &routed, &work, s, cur, goal);
                            if mt2.timed_out {
                                undo(obs, &mg, &moved);
                                out.timed_out = true;
                                break 'conn;
                            }
                            if !helps(&bad(&mt2)) {
                                lo = step;
                                continue;
                            }
                            // Bisect down to the shortest distance that still helps.
                            let (mut hi, mut keep) = (step, (goal, mt2, m2));
                            for _ in 0..SHOVE_BISECT {
                                let mid = 0.5 * (lo + hi);
                                let Some(g) = shove_goal(cx, &mg, home, cur, dir, mid, size) else {
                                    lo = mid;
                                    continue;
                                };
                                let (mt3, m3) = shove_try(cx, obs, &mg, &metr, &routed, &work, s, cur, g);
                                if !mt3.timed_out && helps(&bad(&mt3)) {
                                    hi = mid;
                                    keep = (g, mt3, m3);
                                } else {
                                    lo = mid;
                                }
                            }
                            obs.move_site(&mg.site_anchor[s], keep.0);
                            moved.push((s, cur));
                            mt = keep.1;
                            metr = keep.2;
                            continue 'rounds;
                        }
                    }
                }
            }
            break;
        }
        if !bad(&mt).is_empty() {
            undo(obs, &mg, &moved);
            best.mg.sync_anchors(obs, &net_of);
            continue;
        }
        out.shoved += moved.len();
        let (relax_iterations, rounds) = (mt.relax_iterations, mt.rounds);
        let res = merged_res(best, mt, Some(c));
        let (nok, vias, len, outl) = tally(&res);
        if nok <= best.routed {
            best.mg.sync_anchors(obs, &net_of);
            continue;
        }
        // The snapshot keeps only the routed paths in its graph (as the negotiation does).
        let others: Vec<usize> = back.iter().copied().filter(|&i| i != c).collect();
        take_back(&mut mg, obs, &others);
        let mut paths = best.paths.clone();
        paths[c] = Some(p);
        out.routed += 1;
        best.clinches = detect_clinches(&mg, live, &paths, rt.starts);
        best.routed = nok;
        best.vias = vias;
        best.len = len;
        best.out = outl;
        best.res = res;
        best.overuse_edges = mg
            .layers
            .iter()
            .map(|l| (0..l.g.edges.len()).filter(|&e| l.g.overuse(e) > 0.0).count())
            .sum();
        best.relax_iterations = relax_iterations;
        best.rounds = rounds;
        best.paths = paths;
        best.mg = mg;
        best.metr = metr;
    }
    best.mg.sync_anchors(obs, &net_of);
    out
}

/// Zero-slack retry (R6), one pass from the best iteration's graph. The detoured connections
/// (`est_len > DETOUR_FACTOR × straight`) leave the graph; every routed connection is searched
/// once at margin 0 (empty history). The candidates (single-leg paths, estimated gain >
/// [`ZS_GAIN_MIN_NM`]) are tried by descending gain (ties: lowest id), at most
/// `expert.zero_slack_candidates` of them, each in a fresh copy of the best graph
/// ([`zero_slack_attempt`]); the first that routes at least as much with a shorter outside-pad
/// length wins. Returns the new snapshot and the zero-slack connections (empty when the
/// candidate's new path turned out feasible at ε); `Err(reason)` (the last attempt's) leaves the
/// best iteration unchanged. `obs` is left in the state of the last attempt: the caller re-syncs
/// it. Complexity: one margin-0 search per routed connection plus ≤ `expert.zero_slack_candidates`
/// metrizations.
fn zero_slack_pass(
    cx: &Ctx,
    obs: &mut ObstacleSet,
    rt: &Route,
    best: &Snap,
) -> Result<(Snap, Vec<usize>), String> {
    let live = cx.conns;
    let m = live.len();
    let mut mg = best.mg.clone();
    mg.sync_anchors(obs, &rt.net_of());
    let ok = |i: usize| matches!(best.res[i], Res::Ok(_));
    // A5: junction-linked connections keep their paths (a removal would cascade).
    let detoured: Vec<usize> = (0..m)
        .filter(|&i| {
            ok(i)
                && !mg.has_junction_link(i)
                && best.paths[i].as_ref().is_some_and(|p| {
                    p.legs.iter().any(|l| !l.crossings.is_empty())
                        && p.est_len > cx.opts.detour_factor * (live[i].from.at - live[i].to.at).len()
                })
        })
        .collect();
    for &d in &detoured {
        mg.remove_conn(d, obs);
    }
    let cap = default_expansion_cap(&mg);
    let params = |net: i32, tier: u8, eps: Option<f64>| SearchParams {
        history: &[],
        site_history: &[],
        weight: tier_weight(tier),
        present_overuse: cx.opts.expert.present_overuse,
        expansion_cap: cap,
        deadline: cx.deadline,
        eps,
        via_cost: cx.opts.via_cost_for(net) as f64 * via_weight(tier),
        max_vias: cx.opts.max_vias_per_connection,
        strict_unfit_edges: cx.opts.strict_unfit_edges,
    };
    // Candidates: margin-0 searches over the routed connections (a detoured one with the other
    // detoured ones absent; any other with its own path taken out and put back).
    let mut cands: Vec<(f64, usize, SlotPath)> = Vec::new();
    // A4 (`Options.zero_slack_failed`): a connection that failed (not permanently) is a
    // candidate too, ahead of every length gain (it routes one more): its only corridor may be
    // the one pinched below ε (eprom F.Cu: +5V along the top edge).
    let failed = |i: usize| {
        cx.opts.zero_slack_failed && matches!(best.res[i], Res::Fail { perm: false, .. })
    };
    let linked: Vec<bool> = (0..m).map(|i| mg.has_junction_link(i)).collect();
    for i in (0..m).filter(|&i| (ok(i) || failed(i)) && !linked[i]) {
        let old = match best.paths[i].as_ref() {
            Some(p) if ok(i) => p.est_len,
            _ if !ok(i) => f64::INFINITY,
            _ => continue,
        };
        if cx.stop() {
            return Err(ZS_DEADLINE.into());
        }
        let out = if detoured.contains(&i) || (!ok(i) && mg.take_path(i).is_none()) {
            search(
                &mg,
                obs,
                &live[i],
                &rt.starts[i],
                &rt.goals[i],
                &params(live[i].net, live[i].tier, Some(0.0)),
            )
        } else {
            let Some(taken) = mg.take_path(i) else {
                continue;
            };
            mg.remove_conn(i, obs);
            let out = search(
                &mg,
                obs,
                &live[i],
                &rt.starts[i],
                &rt.goals[i],
                &params(live[i].net, live[i].tier, Some(0.0)),
            );
            mg.insert_path(&live[i], &taken, obs);
            out
        };
        match out {
            SearchOutcome::Found(p) => {
                let gain = if old.is_finite() {
                    old - p.est_len
                } else {
                    f64::MAX
                };
                if gain > ZS_GAIN_MIN_NM
                    && p.legs.len() == 1
                    && p.sites.is_empty()
                    && p.junction.is_none()
                {
                    cands.push((gain, i, p));
                }
            }
            SearchOutcome::Blocked { .. } => {}
            SearchOutcome::Timeout => return Err(ZS_DEADLINE.into()),
        }
    }
    cands.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    drop(mg);
    let mut last = "no candidate".to_string();
    for (_, c, p) in cands.into_iter().take(cx.opts.expert.zero_slack_candidates) {
        match zero_slack_attempt(cx, obs, rt, best, &detoured, c, p, &params) {
            Ok(r) => return Ok(r),
            Err(e) if e == ZS_DEADLINE => return Err(e),
            Err(e) => last = format!("#{}: {e}", live[c].id),
        }
    }
    Err(last)
}

/// One candidate of [`zero_slack_pass`]: connection `c` takes `p` in a copy of the best graph
/// without the detoured connections; the other detoured connections are re-routed once at ε.
/// Metrization: at ε first; if `c` fails, at margin 0 against every obstacle to name its
/// corridor (the obstacles pinching the chain, `corridor_obstacles`), then at margin 0 against
/// that corridor only.
#[allow(clippy::too_many_arguments)]
fn zero_slack_attempt<'a>(
    cx: &Ctx,
    obs: &mut ObstacleSet,
    rt: &Route,
    best: &Snap,
    detoured: &[usize],
    c: usize,
    p: SlotPath,
    params: &dyn Fn(i32, u8, Option<f64>) -> SearchParams<'a>,
) -> Result<(Snap, Vec<usize>), String> {
    let live = cx.conns;
    let m = live.len();
    let ok = |i: usize| matches!(best.res[i], Res::Ok(_));
    let mut mg = best.mg.clone();
    mg.sync_anchors(obs, &rt.net_of());
    for &d in detoured {
        mg.remove_conn(d, obs);
    }
    mg.remove_conn(c, obs);
    mg.insert_path(&live[c], &p, obs);
    let mut work: Vec<usize> = vec![c];
    let mut paths: Vec<Option<SlotPath>> = best.paths.clone();
    paths[c] = Some(p);
    let mut blocked = best.blocked.clone();
    let mut res: Vec<Res> = best.res.clone();
    for &d in detoured.iter().filter(|&&d| d != c) {
        if cx.stop() {
            return Err(ZS_DEADLINE.into());
        }
        work.push(d);
        match search(
            &mg,
            obs,
            &live[d],
            &rt.starts[d],
            &rt.goals[d],
            &params(live[d].net, live[d].tier, None),
        ) {
            SearchOutcome::Found(p) => {
                mg.insert_path(&live[d], &p, obs);
                paths[d] = Some(p);
            }
            SearchOutcome::Blocked {
                ghost,
                blocked_edge,
                blocked_site,
                last_reached,
                static_reason,
            } => {
                paths[d] = None;
                res[d] = fail("blocked: no path (zero-slack retry)");
                blocked.insert(
                    d,
                    BlockInfo {
                        edge: blocked_edge,
                        site: blocked_site,
                        ghost,
                        last_reached,
                        static_reason,
                    },
                );
            }
            SearchOutcome::Timeout => return Err(ZS_DEADLINE.into()),
        }
    }
    let routed: Vec<usize> = (0..m)
        .filter(|&i| paths[i].is_some() && (ok(i) || i == c))
        .collect();
    let res_of = |mt: &Metrized, i: usize| -> Option<Res> {
        mt.res.iter().find(|(j, _)| *j == i).map(|(_, r)| r.clone())
    };
    let reason = |r: Option<Res>| match r {
        Some(Res::Fail { reason, .. }) => reason,
        _ => "not metrized".into(),
    };
    let mut st_a = best.metr.clone();
    let mt_a = metrize_zs(cx, obs, &mg, &routed, None, &mut st_a, &work);
    if mt_a.timed_out {
        return Err(ZS_DEADLINE.into());
    }
    let (mt, zs, metr) = if matches!(res_of(&mt_a, c), Some(Res::Ok(_))) {
        (mt_a, Vec::new(), st_a)
    } else {
        // Identification: margin 0 against everything names the corridor.
        let ident = ZsSpec {
            live: c,
            anchors: None,
            walls: None,
        };
        let mut st_b = st_a.clone();
        let mt_b = metrize_zs(cx, obs, &mg, &routed, Some(&ident), &mut st_b, &[]);
        if mt_b.timed_out {
            return Err(ZS_DEADLINE.into());
        }
        if !matches!(res_of(&mt_b, c), Some(Res::Ok(_))) {
            return Err(format!(
                "infeasible at margin 0: {}",
                reason(res_of(&mt_b, c))
            ));
        }
        let (anchors, walls) = mt_b.zs_corridor.unwrap_or_default();
        let spec = ZsSpec {
            live: c,
            anchors: Some(anchors),
            walls: Some(walls),
        };
        let mut st_c = st_a;
        let mt_c = metrize_zs(cx, obs, &mg, &routed, Some(&spec), &mut st_c, &[]);
        if mt_c.timed_out {
            return Err(ZS_DEADLINE.into());
        }
        if !matches!(res_of(&mt_c, c), Some(Res::Ok(_))) {
            return Err(format!(
                "infeasible with corridor margin 0: {}",
                reason(res_of(&mt_c, c))
            ));
        }
        (mt_c, vec![c], st_c)
    };
    for (i, r) in mt.res {
        res[i] = r;
    }
    let (nok, vias, len, out) = tally(&res);
    if !(nok > best.routed || (nok == best.routed && out < best.out)) {
        return Err("no length gain".into());
    }
    let overuse_edges = mg
        .layers
        .iter()
        .map(|l| {
            (0..l.g.edges.len())
                .filter(|&e| l.g.overuse(e) > 0.0)
                .count()
        })
        .sum();
    let clinches = detect_clinches(&mg, live, &paths, rt.starts);
    Ok((
        Snap {
            routed: nok,
            vias,
            len,
            out,
            iteration: best.iteration,
            res,
            overuse_edges,
            clinches,
            relax_iterations: mt.relax_iterations,
            rounds: mt.rounds,
            blocked,
            paths,
            mg,
            metr,
        },
        zs,
    ))
}

fn layer_name(model: &BoardModel, id: u32) -> String {
    match model.layers.iter().find(|l| l.id == id) {
        Some(l) if !l.name.is_empty() => l.name.clone(),
        _ if id == 0 => "F.Cu".into(),
        _ if id == 31 => "B.Cu".into(),
        _ => format!("layer {id}"),
    }
}

/// Route `model`: runs [`route_inner`] on a rayon pool of `opts.effective_threads()` threads
/// (`threads_used` = pool size; if the pool cannot be built the run is sequential with the
/// warning `thread pool unavailable: 1 thread`). Every thread count gives the same routing
/// (deterministic in-order batch commit, note m3 §7).
pub fn route(model: &BoardModel, opts: &Options) -> RouteResult {
    route_with(model, opts, &RunControl::default())
}

/// [`route`] under a [`RunControl`]: `ctl.cancel` ends the run at the next deadline check with
/// the best result so far (`stop_reason == "cancel"`), `ctl.progress` is updated per iteration
/// and per pass. The cancel flag is installed on every thread of the run's private pool.
pub fn route_with(model: &BoardModel, opts: &Options, ctl: &RunControl) -> RouteResult {
    if opts.ignore_pads.is_empty() {
        return route_with_pool(model, opts, ctl);
    }
    let (filtered, ignored) = drop_ignored_pads(model, opts);
    let mut r = route_with_pool(&filtered, opts, ctl);
    append_ignored(&mut r, model, &ignored);
    r
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
        });
    }
}

fn route_with_pool(model: &BoardModel, opts: &Options, ctl: &RunControl) -> RouteResult {
    // Resolve the options that the search reads by net code / through the rules (A10): the
    // per-net via cost by net name, the drill filter. The caller's inputs are not modified.
    let mut opts_r;
    let mut model_r;
    let (mut model, mut opts) = (model, opts);
    if !opts.via_cost_by_net.is_empty() {
        opts_r = opts.clone();
        opts_r.via_cost_by_code = model
            .nets
            .iter()
            .filter_map(|n| opts.via_cost_by_net.get(&n.name).map(|&c| (n.code, c)))
            .collect();
        opts = &opts_r;
    }
    if !opts.allowed_drills_nm.is_empty() {
        model_r = model.clone();
        model_r.rules.allowed_drills = opts.allowed_drills_nm.clone();
        model = &model_r;
    }
    let mut r = route_with_resolved(model, opts, ctl);
    r.vias_in_pads = vias_in_pads(model, &r.items);
    r
}

/// Positions (KiCad frame, nm) of the routed vias whose centre lies inside an SMD pad of their
/// own net (via-in-pad: the fab must fill and cap them). O(V * P) for V vias and P pads.
fn vias_in_pads(model: &BoardModel, items: &[Item]) -> Vec<[i64; 2]> {
    let mut out: Vec<[i64; 2]> = Vec::new();
    for it in items {
        let Item::Via(v) = it else { continue };
        let q = Vec2 {
            x: v.at.x as f64,
            y: -(v.at.y as f64),
        };
        let pt = Prim::Seg(Seg { a: q, b: q });
        let inside = model.pads.iter().any(|p| {
            p.net == v.net
                && p.attr == PadAttr::Smd
                && p.layers.first().is_some_and(|&l| {
                    dist_prim_skeleton(&pt, &model.skeleton(p, l)) <= 0.0
                })
        });
        if inside {
            out.push([v.at.x, v.at.y]);
        }
    }
    out.sort_unstable();
    out
}

fn route_with_resolved(model: &BoardModel, opts: &Options, ctl: &RunControl) -> RouteResult {
    let n = opts.effective_threads().max(1);
    let flag = ctl.cancel.clone();
    let built = rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .start_handler({
            let flag = flag.clone();
            move |_| crate::control::install_cancel(Some(flag.clone()))
        })
        .build();
    match built {
        Ok(pool) => {
            let mut r = pool.install(|| route_inner(model, opts, ctl));
            r.threads_used = n;
            r
        }
        Err(_) => {
            crate::control::install_cancel(Some(flag));
            let mut r = route_inner(model, opts, ctl);
            crate::control::install_cancel(None);
            r.warnings.push("thread pool unavailable: 1 thread".into());
            r.threads_used = 1;
            r
        }
    }
}

/// Pour-completion passes after the first routing pass (`pourcheck`): pads and vias that were
/// connected through a pour and lost their fill island to the routed copper get a
/// pour-completion connection. Pass 1 routes it against the routed copper as fixed obstacles;
/// when that is blocked, pass 2 unfreezes the routed foreign connections within the lost item's
/// escape radius and re-routes them together with the completion (negotiation); one more pass
/// catches a reach the re-routing broke.
pub const POUR_REACH_PASSES: u32 = 3;

/// Pad indices at the ends of the routed connections of `r`.
fn routed_pads(model: &BoardModel, r: &RouteResult) -> Vec<usize> {
    let mut out: Vec<usize> = r
        .connections
        .iter()
        .filter(|c| c.status == "routed")
        .flat_map(|c| [&c.from, &c.to])
        .filter(|t| t.kind == "pad")
        .filter_map(|t| pad_of_label(model, &t.label))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn pad_of_label(model: &BoardModel, label: &str) -> Option<usize> {
    let pr = PadRef::try_from(label.to_string()).ok()?;
    model
        .pads
        .iter()
        .position(|p| p.ref_ == pr.ref_ && p.number == pr.number)
}

fn pad_label(model: &BoardModel, pi: usize) -> String {
    format!("{}.{}", model.pads[pi].ref_, model.pads[pi].number)
}

/// Radius of the smallest disc about the pad centre containing its copper on any layer.
fn pad_hull_r(model: &BoardModel, pi: usize) -> f64 {
    let pad = &model.pads[pi];
    let c = pad.center.to_vec2();
    pad.layers
        .iter()
        .map(|&l| match model.skeleton(pad, l) {
            Skeleton::Point { p, r } => (p - c).len() + r,
            Skeleton::Seg { s, r } => (s.a - c).len().max((s.b - c).len()) + r,
            Skeleton::Poly { pts, r } => pts.iter().map(|p| (*p - c).len()).fold(0.0, f64::max) + r,
        })
        .fold(0.0, f64::max)
}

/// Remove the connections `ids` (reports, items, zero-slack entries) and renumber the rest
/// 0..n in order.
fn drop_conns(r: &mut RouteResult, ids: &BTreeSet<usize>) {
    let keep: Vec<bool> = r.item_conn.iter().map(|c| !ids.contains(c)).collect();
    let mut k = 0;
    r.items.retain(|_| {
        k += 1;
        keep[k - 1]
    });
    r.item_conn.retain(|c| !ids.contains(c));
    r.connections.retain(|c| !ids.contains(&c.id));
    r.zero_slack_wires.retain(|z| !ids.contains(&z.conn));
    let map: BTreeMap<usize, usize> = r
        .connections
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id, i))
        .collect();
    for (i, c) in r.connections.iter_mut().enumerate() {
        c.id = i;
    }
    for c in &mut r.item_conn {
        *c = map[c];
    }
    for z in &mut r.zero_slack_wires {
        z.conn = map[&z.conn];
    }
}

/// Append a pass's result to `r` (connection ids continue after `r`'s) and recompute the
/// per-net reports, counters and lengths from the reports and items.
fn merge_pass(r: &mut RouteResult, model: &BoardModel, mut r2: RouteResult) {
    let off = r.connections.len();
    for c in &mut r2.connections {
        c.id += off;
    }
    r.items.extend(r2.items);
    r.item_conn
        .extend(r2.item_conn.into_iter().map(|c| c + off));
    r.connections.extend(r2.connections);
    for n2 in r2.nets {
        if !r.nets.iter().any(|n| n.net == n2.net) {
            r.nets.push(n2);
        }
    }
    for n in &mut r.nets {
        let cs: Vec<&ConnReport> = r.connections.iter().filter(|c| c.net == n.net).collect();
        let count = |s: &str| cs.iter().filter(|c| c.status == s).count();
        n.connections = cs.len();
        n.routed = count("routed");
        n.partial = count("partial");
        n.failed = count("failed");
        n.vias = cs.iter().map(|c| c.vias).sum();
        n.length_nm = cs.iter().map(|c| c.length_nm).sum();
        n.length_outside_pads_nm = cs.iter().map(|c| c.length_outside_pads_nm).sum();
        if !cs.is_empty() {
            n.status = if n.routed == cs.len() {
                "routed"
            } else if n.routed == 0 && n.partial == 0 {
                "failed"
            } else {
                "partial"
            }
            .into();
        }
    }
    r.connections_total = r.connections.len();
    r.routed = r
        .connections
        .iter()
        .filter(|c| c.status == "routed")
        .count();
    r.partial = r
        .connections
        .iter()
        .filter(|c| c.status == "partial")
        .count();
    r.failed = r
        .connections
        .iter()
        .filter(|c| c.status == "failed")
        .count();
    r.nets_total = r.nets.iter().filter(|n| n.connections > 0).count();
    r.nets_routed = r
        .nets
        .iter()
        .filter(|n| n.connections > 0 && n.status == "routed")
        .count();
    let ok = r.connections.iter().filter(|c| c.status == "routed");
    r.total_length_nm = ok.clone().map(|c| c.length_nm).sum();
    r.length_outside_pads_nm = ok.map(|c| c.length_outside_pads_nm).sum();
    r.vias = r.connections.iter().map(|c| c.vias).sum();
    let mut by_layer: BTreeMap<String, f64> = BTreeMap::new();
    for it in &r.items {
        if let Item::Track(t) = it {
            *by_layer.entry(layer_name(model, t.layer)).or_insert(0.0) +=
                dequantise(&t.prim).length();
        }
    }
    r.length_by_layer_nm = by_layer
        .into_iter()
        .map(|(k, v)| (k, v.round() as i64))
        .collect();
    r.iterations += r2.iterations;
    r.converged &= r2.converged;
    r.overuse_edges += r2.overuse_edges;
    r.relax_iterations += r2.relax_iterations;
    r.self_check_rounds = r.self_check_rounds.max(r2.self_check_rounds);
    r.check_candidates_visited += r2.check_candidates_visited;
    if r2.timed_out && !r.timed_out {
        r.stop_reason = r2.stop_reason.clone();
    }
    r.timed_out |= r2.timed_out;
    r.warnings.extend(
        r2.warnings
            .into_iter()
            .map(|w| format!("pour completion pass: {w}")),
    );
    for mut z in r2.zero_slack_wires {
        z.conn += off;
        r.zero_slack_wires.push(z);
    }
    r.stubs.extend(r2.stubs);
    if let (Some(p), Some(p2)) = (r.profile.as_mut(), r2.profile) {
        p.extend(p2);
    }
}

/// `r.items` without the duplicate exit-stub items ([`crate::stubs::stub_duplicates`]). O(I log S).
fn unique_items(r: &RouteResult) -> Vec<Item> {
    let dup = crate::stubs::stub_duplicates(&r.items, &r.stubs);
    r.items
        .iter()
        .zip(&dup)
        .filter(|(_, d)| d.is_none())
        .map(|(it, _)| it.clone())
        .collect()
}

/// Remove the duplicate exit-stub items of `r` (and their share of the totals): the final
/// result carries every stub once. O(I log S).
fn dedupe_stub_items(r: &mut RouteResult, model: &BoardModel) {
    let dup = crate::stubs::stub_duplicates(&r.items, &r.stubs);
    if dup.iter().all(Option::is_none) {
        return;
    }
    let (mut keep_items, mut keep_conn) = (Vec::new(), Vec::new());
    let items = std::mem::take(&mut r.items);
    let mut conns = std::mem::take(&mut r.item_conn);
    conns.resize(items.len(), usize::MAX);
    for ((it, c), d) in items.into_iter().zip(conns).zip(&dup) {
        match d {
            Some(s) => {
                r.total_length_nm -= s.length_nm;
                r.length_outside_pads_nm -= s.outside_nm;
                if let Some(v) = r.length_by_layer_nm.get_mut(&layer_name(model, s.layer)) {
                    *v -= s.length_nm;
                }
            }
            None => {
                keep_items.push(it);
                keep_conn.push(c);
            }
        }
    }
    r.items = keep_items;
    r.item_conn = keep_conn;
}

/// Route the unconnected nets of `model` (`route_pass`), then verify the pour reach of every
/// pour-connected pad and via against the routed copper (`pourcheck`) and, for the items that
/// lost their fill island, run up to `POUR_REACH_PASSES` pour-completion passes (see the
/// constant). The result is a pure function of the model and options (every pass is), so the
/// thread count does not change it; a model without pour fills returns the first pass as is.
fn route_inner(model: &BoardModel, opts: &Options, ctl: &RunControl) -> RouteResult {
    let t0 = Instant::now();
    let reach_deadline = deadline_after(opts.wall_clock_s, t0);
    ctl.update("setup", 0, 0, 0, 0, 0.0);
    let mut r = route_pass(model, opts, None, ctl, 0, t0);
    let has_fill = model
        .pours
        .iter()
        .any(|z| z.net != 0 && z.fills.iter().any(|f| f.outer.len() >= 3));
    if !has_fill {
        dedupe_stub_items(&mut r, model);
        ctl.update("done", r.iterations, r.routed, r.connections_total, r.vias, t0.elapsed().as_secs_f64());
        return r;
    }
    let mut passes = 0u32;
    let mut unfroze = false;
    loop {
        let reach =
            check_pour_reach_until(model, &unique_items(&r), &routed_pads(model, &r), Some(reach_deadline));
        if reach.is_clean() {
            break;
        }
        let lost_pads: Vec<String> = reach
            .lost_pads
            .iter()
            .map(|&pi| pad_label(model, pi))
            .collect();
        let lost: Vec<String> = lost_pads
            .iter()
            .cloned()
            .chain(reach.lost_vias.iter().map(|u| format!("via {u}")))
            .collect();
        r.warnings.extend(reach.notes.iter().cloned());
        let stop = |r: &mut RouteResult, why: &str| {
            r.warnings.push(format!(
                "pour reach: {} item(s) without pour after {passes} completion pass(es) ({why}): {}",
                lost.len(),
                lost.join(", ")
            ));
        };
        // Earlier completion attempts for the still-lost items that did not route are retried;
        // on a retry the routed foreign connections within the lost item's escape radius
        // (hull + 2 · (default via + clearance)) are unfrozen and re-routed with it.
        let lost_labels: BTreeSet<&str> = lost_pads
            .iter()
            .map(String::as_str)
            .chain(reach.lost_vias.iter().map(String::as_str))
            .collect();
        let retry: BTreeSet<usize> = r
            .connections
            .iter()
            .filter(|c| {
                c.status != "routed"
                    && c.to.kind == "pour"
                    && lost_labels.contains(c.from.label.as_str())
            })
            .map(|c| c.id)
            .collect();
        let mut unfreeze: BTreeSet<usize> = BTreeSet::new();
        if !retry.is_empty() {
            if unfroze {
                stop(&mut r, "blocked after re-routing the neighbours");
                break;
            }
            let rule = &model.rules.default;
            let reach_r = 2.0 * (rule.via_d + rule.clearance) as f64;
            let mut centres: Vec<(i32, Vec2, f64)> = reach
                .lost_pads
                .iter()
                .map(|&pi| {
                    (
                        model.pads[pi].net,
                        model.pads[pi].center.to_vec2(),
                        pad_hull_r(model, pi) + reach_r,
                    )
                })
                .collect();
            centres.extend(
                model
                    .vias
                    .iter()
                    .filter(|v| reach.lost_vias.contains(&v.uuid))
                    .map(|v| (v.net, v.at.to_vec2(), 0.5 * v.d as f64 + reach_r)),
            );
            for (k, it) in r.items.iter().enumerate() {
                let cid = r.item_conn[k];
                if r.connections[cid].status != "routed" {
                    continue;
                }
                for &(lnet, c, radius) in &centres {
                    let (inet, dist) = match it {
                        Item::Track(t) => (
                            t.net,
                            dist_prim_skeleton(
                                &dequantise(&flip_track(t.prim)),
                                &Skeleton::Point { p: c, r: 0.0 },
                            ) - 0.5 * t.width as f64,
                        ),
                        Item::Via(v) => {
                            let mut at = v.at;
                            at.flip();
                            (v.net, (at.to_vec2() - c).len() - 0.5 * v.d as f64)
                        }
                    };
                    if inet != lnet && dist <= radius {
                        unfreeze.insert(cid);
                    }
                }
            }
            if unfreeze.is_empty() {
                stop(&mut r, "blocked by fixed copper");
                break;
            }
            unfroze = true;
        }
        if passes >= POUR_REACH_PASSES {
            stop(&mut r, "pass cap");
            break;
        }
        // `wall_clock_s == 0.0` disables the timeout: the remaining budget stays 0.0 (disabled).
        let disabled = opts.wall_clock_s == 0.0;
        let remaining = if disabled {
            0.0
        } else {
            opts.wall_clock_s - t0.elapsed().as_secs_f64()
        };
        if r.timed_out || ctl.is_cancelled() || (!disabled && remaining <= 0.0) {
            stop(&mut r, "no budget");
            if !r.timed_out {
                r.stop_reason = if ctl.is_cancelled() { "cancel" } else { "timeout" }.into();
            }
            r.timed_out = true;
            break;
        }
        passes += 1;
        // Terminals of the unfrozen connections join the scope; their copper and the retried
        // attempts leave the result before the pass.
        let mut scope_pads: Vec<PadRef> = reach
            .lost_pads
            .iter()
            .map(|&pi| PadRef {
                ref_: model.pads[pi].ref_.clone(),
                number: model.pads[pi].number.clone(),
            })
            .collect();
        let mut scope_vias: Vec<String> = reach.lost_vias.clone();
        let mut unfrozen_names: Vec<String> = Vec::new();
        for c in r.connections.iter().filter(|c| unfreeze.contains(&c.id)) {
            unfrozen_names.push(format!("{} #{}", c.name, c.id));
            for t in [&c.from, &c.to] {
                match t.kind.as_str() {
                    "pad" => {
                        if let Ok(pr) = PadRef::try_from(t.label.clone()) {
                            scope_pads.push(pr);
                        }
                    }
                    "via" => scope_vias.push(t.label.clone()),
                    _ => {}
                }
            }
        }
        let drop: BTreeSet<usize> = retry.union(&unfreeze).copied().collect();
        drop_conns(&mut r, &drop);
        // The fixed copper is what remains; the pour goals keep the main component of the
        // check above (with every routed item present: the robustly connected fill).
        let fixed = unique_items(&r);
        let now = check_pour_reach_until(model, &fixed, &routed_pads(model, &r), Some(reach_deadline));
        let mut m2 = with_routed_copper(model, &fixed, &now);
        for z in &mut m2.pours {
            z.pads.retain(|s| !lost_labels.contains(s.as_str()));
            z.vias.retain(|u| !lost_labels.contains(u.as_str()));
        }
        let opts2 = Options {
            scope: Scope {
                pads: scope_pads,
                vias: scope_vias,
                ..Scope::default()
            },
            wall_clock_s: remaining,
            existing_copper: ExistingCopper::Fixed,
            ..opts.clone()
        };
        let r2 = route_pass(&m2, &opts2, Some(&reach), ctl, r.iterations, t0);
        r.warnings.push(format!(
            "pour reach pass {passes}: {} item(s) lost the pour after routing ({}); {} \
             connection(s) routed {}/{}, vias {}{}",
            lost.len(),
            lost.join(", "),
            if unfreeze.is_empty() {
                "completion"
            } else {
                "completion + re-routed neighbour"
            },
            r2.routed,
            r2.connections_total,
            r2.vias,
            if unfrozen_names.is_empty() {
                String::new()
            } else {
                format!("; re-routed: {}", unfrozen_names.join(", "))
            }
        ));
        merge_pass(&mut r, model, r2);
    }
    dedupe_stub_items(&mut r, model);
    r.runtime_s = t0.elapsed().as_secs_f64();
    ctl.update("done", r.iterations, r.routed, r.connections_total, r.vias, r.runtime_s);
    r
}

/// A5: the Steiner context of one pass: per live connection its net, the clusters of its ends
/// (`Decomposition::conn_clusters`), whether it targets a pour, the vertex owner labels of its
/// terminals; per cluster the constraint edges of its kept tracks (both ends kept-track vertices
/// of that cluster or wall split vertices). O(m + Σ_L E_L).
fn steiner_nets(
    model: &BoardModel,
    opts: &Options,
    dec: &crate::decompose::Decomposition,
    live: &[Connection],
    orig: &[usize],
    mg: &MultiGraph,
) -> crate::steiner::SteinerNets {
    use crate::steiner::{owner_labels, SConn, SteinerNets};
    let conns: Vec<SConn> = live
        .iter()
        .enumerate()
        .map(|(i, c)| {
            // A connection the decomposition gave no clusters joins nothing (unique ids).
            let (f, t) = dec
                .conn_clusters
                .get(orig[i])
                .copied()
                .unwrap_or((usize::MAX - 2 * i, usize::MAX - 2 * i - 1));
            SConn {
                net: c.net,
                from_cl: f,
                to_cl: t,
                pour: matches!(c.to.kind, TermKind::Pour(_)),
                owners: [owner_labels(&c.from), owner_labels(&c.to)],
            }
        })
        .collect();
    let mut by_net: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for (i, c) in live.iter().enumerate() {
        by_net.entry(c.net).or_default().push(i);
    }
    let kept_cl: BTreeMap<String, usize> = dec
        .kept_cluster
        .iter()
        .filter_map(|(&ki, &cl)| model.kept.get(ki).map(|k| (format!("kept:{}", k.uuid), cl)))
        .collect();
    // Pads / vias -> their cluster, by every vertex owner label they appear under (a hand
    // track that ends at a pad centre shares that vertex with the pad).
    let mut term_cl: BTreeMap<String, usize> = BTreeMap::new();
    for (&pi, &cl) in &dec.pad_cluster {
        let p = &model.pads[pi];
        term_cl.entry(format!("{}.{}", p.ref_, p.number)).or_insert(cl);
        term_cl.entry(format!("pad:{}:{}", p.ref_, p.number)).or_insert(cl);
    }
    for (&vi, &cl) in &dec.via_cluster {
        let v = &model.vias[vi];
        term_cl.entry(v.uuid.clone()).or_insert(cl);
        term_cl.entry(format!("via:{}", v.uuid)).or_insert(cl);
    }
    let mut copper: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    for (li, lc) in mg.layers.iter().enumerate() {
        let g = &lc.g;
        for (e, ge) in g.edges.iter().enumerate() {
            if !ge.constraint {
                continue;
            }
            // (cluster, is a kept-track vertex, owner) of an end; `None` = not track copper.
            let end = |v: usize| -> Option<(Option<usize>, bool, Option<&str>)> {
                let vi = &g.verts[v];
                match vi.owner.as_deref() {
                    Some(o) => match (kept_cl.get(o), term_cl.get(o)) {
                        (Some(&c), _) => Some((Some(c), true, Some(o))),
                        (None, Some(&c)) => Some((Some(c), false, Some(o))),
                        _ => None,
                    },
                    None if vi.anchor.is_none() && vi.site.is_none() => Some((None, false, None)),
                    None => None,
                }
            };
            let (Some((ca, ka, oa)), Some((cb, kb, ob))) = (end(ge.a), end(ge.b)) else {
                continue;
            };
            let cl = match (ca, cb) {
                (Some(x), Some(y)) if x == y => x,
                (Some(x), None) | (None, Some(x)) => x,
                _ => continue,
            };
            // A track piece: a kept-track end, or two different terminals (one pad's own
            // outline is not a track).
            let key = |o: &str| -> String {
                if let Some(r) = o.strip_prefix("pad:") {
                    r.replacen(':', ".", 1)
                } else {
                    o.strip_prefix("via:").unwrap_or(o).to_string()
                }
            };
            let track = ka
                || kb
                || matches!((oa, ob), (Some(x), Some(y)) if key(x) != key(y));
            if track {
                copper.entry(cl).or_default().push((li, e));
            }
        }
    }
    SteinerNets {
        min_angle_deg: opts.expert.steiner_min_angle_deg.clamp(0.0, 90.0),
        conns,
        by_net,
        copper,
    }
}

/// One routing pass over the unconnected nets of `model` on every routable copper layer, with
/// through vias at pre-placed sites (M3): decompose, obstacles, via sites, one graph per layer,
/// PathFinder over layers and sites, per-leg metrization with a self-check, partial stubs,
/// reports. `reach` (completion passes) restricts the pour goals to via sites inside the main
/// fill component.
///
/// Complexity: dominated by the PathFinder loop, `O(iters · m · V' log V')` with
/// `V' = Σ_L 2(E_L + X_L) + 2S`, plus `O(changed · q)` self-checks per iteration (see the
/// module header, `negotiate` and `multigraph`).
fn route_pass(
    model: &BoardModel,
    opts: &Options,
    reach: Option<&PourReach>,
    ctl: &RunControl,
    iter_base: u32,
    run_t0: Instant,
) -> RouteResult {
    let t0 = Instant::now();
    let deadline = deadline_after(opts.wall_clock_s, t0);
    let mut warnings: Vec<String> = Vec::new();
    if opts.existing_copper == ExistingCopper::Smooth {
        warnings.push("existing_copper=smooth ignored by route(); treated as fixed".into());
    }
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
    let all = &dec.connections;

    // Connections that can be searched, renumbered 0..m (the negotiator indexes by id).
    let mut early: Vec<Option<String>> = vec![None; all.len()];
    let mut live: Vec<Connection> = Vec::new();
    let mut orig: Vec<usize> = Vec::new();
    for c in all {
        if routable.is_empty() {
            early[c.id] = Some("no routable layer".into());
        } else if c.from.layers.is_empty() || c.to.layers.is_empty() {
            early[c.id] = Some("terminal not on any routable layer".into());
        } else {
            let mut l = c.clone();
            l.id = live.len();
            orig.push(c.id);
            live.push(l);
        }
    }
    let m = live.len();

    // Pad exit stubs (A6 / NX, `crate::stubs`): planned on the board of this pass, then fixed
    // copper of the net for every later stage (obstacles, CDT, via sites, self-check); the stub
    // ends are the connections' terminals. `all` / the reports keep the pad terminals.
    let stub_plan: StubPlan = if m > 0 {
        plan_stubs(model, opts, &live, &routable, &|| {
            ctl.is_cancelled() || crate::control::expired(deadline)
        })
    } else {
        StubPlan::default()
    };
    warnings.extend(stub_plan.warnings.iter().cloned());
    // Bridged connections (one straight same-net track between two pads of a footprint) are
    // routed by the plan: they leave the live set (original id -> stub index).
    let mut bridged: BTreeMap<usize, usize> = BTreeMap::new();
    if !stub_plan.bridges.is_empty() {
        let (old_live, old_orig) = (std::mem::take(&mut live), std::mem::take(&mut orig));
        for (mut c, o) in old_live.into_iter().zip(old_orig) {
            if let Some(&k) = stub_plan.bridges.get(&c.id) {
                bridged.insert(o, k);
                continue;
            }
            c.id = live.len();
            orig.push(o);
            live.push(c);
        }
    }
    let m = live.len();
    let conn_stubs = attach_stubs(&mut live, &stub_plan);
    let stub_model;
    let model: &BoardModel = if stub_plan.is_empty() {
        model
    } else {
        stub_model = with_stubs(model, &stub_plan);
        &stub_model
    };
    let mut stub_used = vec![false; stub_plan.stubs.len()];

    let mut profile: Vec<IterProfile> = Vec::new();
    let mut best: Option<Snap> = None;
    let (mut iterations, mut converged, mut timed_out) = (0u32, false, false);
    // Why the loop ended when not by convergence / timeout (`stuck`, `no gain`).
    let mut stop_why: Option<&'static str> = None;
    let mut loop_ran = false;
    // Best `(routed, cost)` after each iteration (stop rule).
    let mut best_hist: Vec<(usize, f64)> = Vec::new();
    let mut setup_fail: Option<String> = None;
    let mut partials: BTreeMap<usize, Partial> = BTreeMap::new();
    let mut blocks: BTreeMap<usize, Blk> = BTreeMap::new();
    let mut zs_wires: Vec<usize> = Vec::new();
    let mut n_sites = 0usize;
    // Detour reference per live connection (M3-R11): the ghost on the empty graph.
    let mut ghost_ref: Vec<Option<f64>> = vec![None; m];
    // Ghost cost (`est_len + via_cost * vias`) per live connection: the A5 junction cap.
    let mut ghost_cost: Vec<Option<f64>> = vec![None; m];
    // DRC-feasible reference per live connection (M3-R15): (length, is_feasible).
    let mut feas_ref: Vec<Option<f64>> = vec![None; m];
    let mut ripup = RipReport::default();
    let mut relax_status: String = if opts.via_relaxation { "not run" } else { "off" }.into();
    let mut shove_status: String = if opts.via_shove { "not run" } else { "off" }.into();
    let mut via_status: String = if opts.via_reduction { "not run" } else { "off" }.into();
    let mut zs_status: String = if opts.zero_slack_retry {
        "not run"
    } else {
        "off"
    }
    .into();

    if m > 0 {
        // Obstacles: junction / dangling ends of copper attach points are terminals.
        let mut tmap: BTreeMap<(u32, (u64, u64), i32), TerminalObstacle> = BTreeMap::new();
        for c in &live {
            for t in [&c.from, &c.to] {
                if let (
                    TermKind::Copper(_) | TermKind::Dangling(..) | TermKind::Stub { .. },
                    Some(&layer),
                ) = (&t.kind, t.layers.first())
                {
                    let e = tmap.entry((layer, bits(t.at), c.net)).or_insert_with(|| {
                        TerminalObstacle {
                            layer,
                            at: t.at,
                            net: c.net,
                            width: c.width,
                            label: format!("terminal:net{}", c.net),
                        }
                    });
                    e.width = e.width.max(c.width);
                }
            }
        }
        let tobs: Vec<TerminalObstacle> = tmap.into_values().collect();
        let mut obs = ObstacleSet::build(model, opts, &HashSet::new(), &tobs);

        // Via sites (P3): after the obstacles, before the graphs. Escape sites next to the SMD
        // terminal pads; sizes = the union of the live nets' via sizes, largest first.
        let escape: Vec<usize> = live
            .iter()
            .flat_map(|c| [&c.from, &c.to])
            .filter_map(|t| match t.kind {
                TermKind::Pad(pi) | TermKind::Stub { pad: pi, .. }
                    if model.pads[pi].attr == PadAttr::Smd =>
                {
                    Some(pi)
                }
                _ => None,
            })
            .collect::<BTreeSet<usize>>()
            .into_iter()
            .collect();
        let mut sizes: Vec<ViaSize> = live
            .iter()
            .map(|c| c.net)
            .collect::<BTreeSet<i32>>()
            .into_iter()
            .flat_map(|n| via_sizes(&model.rules, n, opts))
            .collect();
        sizes.sort_by_key(|z| std::cmp::Reverse((z.d, z.drill)));
        sizes.dedup();
        if iter_base == 0 && live.iter().any(|c| via_fallback_used(&model.rules, c.net)) {
            warnings.push(format!(
                "via fallback {:.1}/{:.1} used",
                model.rules.via_fallback[0] as f64 / 1e6,
                model.rules.via_fallback[1] as f64 / 1e6
            ));
        }
        let fit_checker = Checker::new(model, opts);
        // With one routable layer there is no layer to change to: a site can only serve as a
        // pour goal, so sites exist only when a live connection targets a pour. Dormant site
        // vertices otherwise just multiply the faces of the single graph (synth n=1000: 11 837
        // sites, 12x the expansions and 70x the clearance candidates per search, overuse on
        // the free-ended edges that never clears, 25 stalled iterations instead of 7).
        let needs_sites =
            routable.len() >= 2 || live.iter().any(|c| matches!(c.to.kind, TermKind::Pour(_)));
        let sites = if needs_sites {
            let stub_pads: Vec<usize> = stub_plan.by_pad.keys().copied().collect();
            via_sites_stubs(
                model,
                &obs,
                &fit_checker,
                &routable,
                &escape,
                &sizes,
                opts.margin_nm as f64,
                opts.via_in_pad,
                &stub_pads,
            )
        } else {
            Vec::new()
        };
        n_sites = sites.len();

        // Terminals of the graphs: one vertex per position, on the layers it has copper on.
        let mut term_at: BTreeMap<(u64, u64), usize> = BTreeMap::new();
        let mut terms: Vec<(TerminalInfo, Vec<u32>)> = Vec::new();
        // A pour target contributes the same-net vias the pour lists (D-6).
        let pour_vias: Vec<Vec<TermRef>> = live
            .iter()
            .map(|c| match c.to.kind {
                TermKind::Pour(zi) if opts.pour_listed_via_goals => {
                    listed_pour_vias(model, zi, c.net, &routable, reach)
                }
                _ => Vec::new(),
            })
            .collect();
        for (ci, c) in live.iter().enumerate() {
            for t in [&c.from, &c.to].into_iter().chain(pour_vias[ci].iter()) {
                if matches!(t.kind, TermKind::Pour(_)) {
                    continue;
                }
                match term_at.get(&bits(t.at)) {
                    Some(&i) => {
                        for l in &t.layers {
                            if !terms[i].1.contains(l) {
                                terms[i].1.push(*l);
                            }
                        }
                    }
                    None => {
                        term_at.insert(bits(t.at), terms.len());
                        terms.push((
                            TerminalInfo {
                                at: t.at,
                                net: c.net,
                                owner: t.label.clone(),
                                clr: (c.clearance as f64).max(model.rules.min_clearance as f64),
                                r: match t.kind {
                                    TermKind::Copper(_)
                                    | TermKind::Dangling(..)
                                    | TermKind::Stub { .. } => 0.5 * c.width as f64,
                                    _ => 0.0,
                                },
                            },
                            t.layers.clone(),
                        ));
                    }
                }
            }
        }
        let rules = GraphRules {
            eps: opts.margin_nm as f64,
            min_clearance: model.rules.min_clearance as f64,
            edge_clearance: model.rules.edge_clearance as f64,
            hole_clearance: model.rules.hole_clearance as f64,
        };
        match MultiGraph::build_on(model, &mut obs, sites, &routable, &terms, &rules) {
            Err(e) => setup_fail = Some(format!("sketch: {e}")),
            Ok(mut mg) => {
                mg.set_capacity_mode(crate::graph::CapacityMode {
                    own_net: opts.expert.own_net_capacity,
                    wall_aware: opts.expert.wall_aware_capacity,
                    transit: opts.expert.transit_pinch,
                });
                mg.set_layer_costs(model, opts);
                if opts.steiner {
                    mg.steiner = Some(Arc::new(steiner_nets(model, opts, &dec, &live, &orig, &mg)));
                }
                // Plane pours (fills only on non-routed layers, `Options.plane_vias`): their
                // sites are those inside a plane fill; reach is judged on the plane layers.
                let plane_zi: BTreeSet<usize> = live
                    .iter()
                    .filter_map(|c| match c.to.kind {
                        TermKind::Pour(zi) if !c.to.layers.iter().any(|l| routable.contains(l)) => {
                            Some(zi)
                        }
                        _ => None,
                    })
                    .collect();
                let mut plane_layers: Vec<u32> = plane_zi
                    .iter()
                    .flat_map(|&zi| model.pours[zi].fills.iter().map(|f| f.layer))
                    .collect();
                plane_layers.sort_unstable();
                plane_layers.dedup();
                if !plane_zi.is_empty() {
                    let extra = MultiGraph::pour_membership(model, &mg.sites, &plane_layers);
                    for (s, zs) in extra.into_iter().enumerate() {
                        for zi in zs.into_iter().filter(|zi| plane_zi.contains(zi)) {
                            if !mg.in_pour[s].contains(&zi) {
                                mg.in_pour[s].push(zi);
                            }
                        }
                        mg.in_pour[s].sort_unstable();
                    }
                }
                if let Some(reach) = reach {
                    for s in 0..mg.sites.len() {
                        let at = mg.sites[s].at;
                        mg.in_pour[s].retain(|&zi| {
                            let ls = if plane_zi.contains(&zi) {
                                &plane_layers
                            } else {
                                &routable
                            };
                            reach.site_in_main(zi, ls, at)
                        });
                    }
                }
                let mut perm: Vec<Option<String>> = vec![None; m];
                // Start vertices on every layer of the source terminal; the goal is the same
                // for a terminal, the sites inside the pour for a pour target.
                let vertices = |t: &TermRef| -> Vec<(usize, usize)> {
                    routable
                        .iter()
                        .enumerate()
                        .filter(|(_, l)| t.layers.contains(l))
                        .filter_map(|(li, _)| mg.vertex_of(li, t.at).map(|v| (li, v)))
                        .collect()
                };
                let mut starts: Vec<Vec<(usize, usize)>> = Vec::with_capacity(m);
                let mut goals: Vec<Goal> = Vec::with_capacity(m);
                for (i, c) in live.iter().enumerate() {
                    let st = vertices(&c.from);
                    let goal = match c.to.kind {
                        TermKind::Pour(zi) => {
                            let plane = plane_zi.contains(&zi);
                            let pour_sites: Vec<usize> = (0..mg.sites.len())
                                .filter(|&s| mg.in_pour[s].contains(&zi))
                                .filter(|&s| {
                                    !plane
                                        || plane_via_gap(model, &c.from, mg.sites[s].at)
                                            <= opts.plane_via_max_nm as f64
                                })
                                .collect();
                            // D-6: every listed same-net via is a goal vertex on each of its
                            // routable layers (the path ends on it, no new via).
                            let vertices: Vec<(usize, usize)> =
                                pour_vias[i].iter().flat_map(&vertices).collect();
                            if pour_sites.is_empty() && vertices.is_empty() {
                                perm[i] = Some(if plane {
                                    format!(
                                        "no via site within {:.2} mm of the terminal (plane pour)",
                                        opts.plane_via_max_nm as f64 * 1e-6
                                    )
                                } else {
                                    "no via site inside pour".into()
                                });
                            }
                            Goal {
                                vertices,
                                pour_layer: routable.iter().position(|l| c.to.layers.contains(l)),
                                pour_sites,
                                ..Default::default()
                            }
                        }
                        _ => Goal {
                            vertices: vertices(&c.to),
                            pour_sites: Vec::new(),
                            pour_layer: None,
                            ..Default::default()
                        },
                    };
                    if perm[i].is_none()
                        && (st.is_empty()
                            || (goal.vertices.is_empty() && goal.pour_sites.is_empty()))
                    {
                        perm[i] = Some("terminal not on any routable layer".into());
                    }
                    starts.push(st);
                    goals.push(goal);
                }
                let check_opts = Options {
                    margin_nm: 0,
                    ..opts.clone()
                };
                let checker = Checker::new(model, &check_opts);
                let cx = Ctx {
                    model,
                    opts,
                    layers: &routable,
                    conns: &live,
                    checker: &checker,
                    deadline,
                };
                // Static ghost per connection on the empty graph, in parallel, index order
                // (deterministic: the ghost is a pure function of the frozen graph).
                {
                    let cap = default_expansion_cap(&mg);
                    let (mg_ro, obs_ro) = (&mg, &obs);
                    let gh: Vec<Option<(f64, f64)>> = (0..m)
                        .into_par_iter()
                        .map(|i| {
                            if perm[i].is_some() {
                                return None;
                            }
                            let c = &live[i];
                            let params = SearchParams {
                                history: &[],
                                site_history: &[],
                                weight: tier_weight(c.tier),
                                present_overuse: opts.expert.present_overuse,
                                expansion_cap: cap,
                                deadline,
                                eps: None,
                                via_cost: opts.via_cost_for(c.net) as f64 * via_weight(c.tier),
                                max_vias: opts.max_vias_per_connection,
                                strict_unfit_edges: opts.strict_unfit_edges,
                            };
                            ghost(mg_ro, obs_ro, c, &starts[i], &goals[i], &params)
                                .map(|(gp, ..)| (gp.est_len, gp.est_len + params.via_cost * gp.vias() as f64))
                        })
                        .collect();
                    ghost_ref = gh.iter().map(|g| g.map(|x| x.0)).collect();
                    ghost_cost = gh.iter().map(|g| g.map(|x| x.1)).collect();
                }
                // A5: a junction may cost at most `steiner_ghost_factor` times the pad route.
                if opts.steiner {
                    let f = opts.expert.steiner_ghost_factor.max(0.0);
                    for (g, c) in goals.iter_mut().zip(&ghost_cost) {
                        g.junction_cap = c.map(|c| f * c);
                    }
                }
                // DRC-feasible reference (M3-R15): one slot search per connection on the same
                // empty graph (fixed copper only), after the ghost, under its own deadline share.
                // `O(m * search)`, `par_iter` collected in index order; capped by the expansion
                // cap and the share deadline (and the run deadline / cancel through `stop`).
                if opts.detour_reference {
                    let cap = default_expansion_cap(&mg);
                    let (mg_ro, obs_ro) = (&mg, &obs);
                    let share = (opts.detour_reference_budget_pct / 100.0).clamp(0.0, 1.0);
                    let ref_deadline = if opts.wall_clock_s == 0.0 {
                        deadline
                    } else {
                        let left = deadline
                            .saturating_duration_since(Instant::now())
                            .mul_f64(share);
                        Instant::now() + left
                    };
                    feas_ref = (0..m)
                        .into_par_iter()
                        .map(|i| {
                            if perm[i].is_some() || ctl.is_cancelled() {
                                return None;
                            }
                            let c = &live[i];
                            let params = SearchParams {
                                history: &[],
                                site_history: &[],
                                weight: tier_weight(c.tier),
                                present_overuse: opts.expert.present_overuse,
                                expansion_cap: cap,
                                deadline: ref_deadline,
                                eps: None,
                                via_cost: opts.via_cost_nm as f64 * via_weight(c.tier),
                                max_vias: opts.max_vias_per_connection,
                                strict_unfit_edges: opts.strict_unfit_edges,
                            };
                            match search(mg_ro, obs_ro, c, &starts[i], &goals[i], &params) {
                                SearchOutcome::Found(p) => Some(p.est_len),
                                _ => None,
                            }
                        })
                        .collect();
                }
                let rt = Route {
                    starts: &starts,
                    goals: &goals,
                    nets: live.iter().map(|c| c.net).collect(),
                };
                let mut neg = Negotiator::new(&mg, m);
                neg.configure(opts);
                let mut active: Vec<usize> = (0..m).filter(|&i| perm[i].is_none()).collect();
                let (mut stuck, mut last_sig) = (0u32, None::<(usize, Vec<usize>)>);
                let mut metr = MetrState::new(&cx, &obs);
                ctl.update("route", iter_base, 0, 0, 0, run_t0.elapsed().as_secs_f64());
                for _ in 1..=opts.pathfinder_iter_cap {
                    loop_ran = true;
                    if crate::control::expired(deadline) {
                        timed_out = true;
                        break;
                    }
                    let out = neg.iteration(
                        &mut mg, &mut obs, &live, &active, &starts, &goals, opts, deadline,
                    );
                    if out.timed_out {
                        timed_out = true;
                        // Anytime (coordinator ruling Q2): with a result already recorded the
                        // partial iteration is discarded as before; without one, the paths it
                        // committed are metrized (the first pass and one self-check round run
                        // regardless of the deadline) and emitted, so a board whose first
                        // iteration exceeds the budget still returns what it routed.
                        if best.is_some() {
                            break;
                        }
                    }
                    let it = neg.iteration;
                    let routed: Vec<usize> = (0..m)
                        .filter(|&i| {
                            out.paths[i].is_some()
                                && perm[i].is_none()
                                && !legs_of(&mg, i).is_empty()
                        })
                        .collect();
                    let mut work: Vec<usize> = active.clone();
                    work.extend(out.ripped.iter().copied());
                    // A5: connections searched because their junction sibling left the graph.
                    work.extend(out.cascaded.iter().copied());
                    let _ = mg.take_cascaded();
                    let t_metrize = Instant::now();
                    let mt = metrize(&cx, &obs, &mg, &routed, &mut metr, &work);
                    let metrize_s = t_metrize.elapsed().as_secs_f64();
                    if mg.steiner.is_some() {
                        record_chain_cross(&mut mg, &metr, &routed);
                    }
                    // Anytime: a timed-out metrization still yields what it completed; the
                    // loop ends after this iteration's result is recorded.
                    if mt.timed_out {
                        timed_out = true;
                        if mt.res.is_empty() {
                            break;
                        }
                    }
                    iterations = it;
                    if opts.profile {
                        profile.push(IterProfile {
                            iteration: it,
                            search_s: out.search_s,
                            commit_s: out.commit_s,
                            metrize_s,
                            check_s: mt.check_s,
                            searched: out.searched,
                            re_searched: out.resolved_sequentially,
                            revalidated: out.revalidated,
                            ripped: out.ripped.len(),
                            routed: routed.len(),
                            overuse_edges: out.overuse_edges,
                            steiner_researched: out.steiner_researched,
                            ..Default::default()
                        });
                    }
                    let mut res: Vec<Res> = (0..m)
                        .map(|i| match (&perm[i], out.blocked.get(&i)) {
                            (Some(r), _) => Res::Fail {
                                reason: r.clone(),
                                perm: true,
                                blocking: None,
                            },
                            (None, Some(b)) => Res::Fail {
                                reason: match &b.static_reason {
                                    Some(r) => r.clone(),
                                    None if b.edge.is_some() || b.site.is_some() => {
                                        "blocked: no path (wall, capacity or via sites)".into()
                                    }
                                    None => "blocked: no path".into(),
                                },
                                perm: false,
                                blocking: b
                                    .edge
                                    .map(|(li, e)| {
                                        let g = &mg.layers[li].g;
                                        let ge = &g.edges[e];
                                        kicad((g.verts[ge.a].at + g.verts[ge.b].at) * 0.5)
                                    })
                                    .or_else(|| b.site.map(|s| kicad(mg.sites[s].at))),
                            },
                            (None, None) => fail(if out.timed_out {
                                "timeout"
                            } else {
                                "not routed"
                            }),
                        })
                        .collect();
                    mg.steiner_reject.extend(mt.rejects.iter().copied());
                    for (i, r) in mt.res {
                        res[i] = r;
                    }
                    // Feedback: failures first (class-independent ones leave the graph).
                    let mut failed: Vec<usize> = Vec::new();
                    for (i, r) in res.iter().enumerate() {
                        if let Res::Fail {
                            reason, perm: p, ..
                        } = r
                        {
                            if out.blocked.contains_key(&i) {
                                continue;
                            }
                            if *p {
                                if perm[i].is_none() {
                                    perm[i] = Some(reason.clone());
                                    mg.remove_conn(i, &mut obs);
                                }
                            } else {
                                failed.push(i);
                            }
                        }
                    }
                    for &i in &failed {
                        neg.penalise_failed(&mg, i);
                    }
                    for (&i, b) in &out.blocked {
                        neg.penalise_blocked(i, b);
                    }
                    // A failed path's reserved room is not a congestion the others can
                    // resolve: it leaves the graph before the overuse count.
                    for &i in &failed {
                        mg.remove_conn(i, &mut obs);
                    }
                    // A5: junctions that lost their sibling with these removals route again.
                    let cascaded = mg.take_cascaded();
                    let overuse = neg.penalise_overuse(&mg);
                    let (nok, vias, len, out_len) = tally(&res);
                    let better = best.as_ref().is_none_or(|b| better_than(nok, vias, len, b));
                    if let Some(p) = profile.last_mut() {
                        // Profile only (`Options::profile`): the outcome of this iteration.
                        p.ok = nok;
                        p.blocked = out.blocked.len();
                        p.metrize_failed = failed.len();
                        for &i in &failed {
                            if let Res::Fail { reason, .. } = &res[i] {
                                *p.fail_classes.entry(reason_class(reason)).or_default() += 1;
                            }
                        }
                        for b in out.blocked.values() {
                            let k = if b.static_reason.is_some() {
                                "static"
                            } else if b.edge.is_some() {
                                "edge"
                            } else if b.site.is_some() {
                                "site"
                            } else {
                                "none"
                            };
                            *p.blocked_classes.entry(k.into()).or_default() += 1;
                        }
                        p.vias = vias;
                        p.cost_nm = len + opts.via_cost_nm as f64 * vias as f64;
                        p.best = better;
                    }
                    if better {
                        best = Some(Snap {
                            routed: nok,
                            vias,
                            len,
                            out: out_len,
                            iteration: it,
                            res,
                            overuse_edges: overuse,
                            clinches: detect_clinches(&mg, &live, &out.paths, &starts),
                            relax_iterations: mt.relax_iterations,
                            rounds: mt.rounds,
                            blocked: out.blocked.clone(),
                            paths: out.paths.clone(),
                            mg: mg.clone(),
                            metr: metr.clone(),
                        });
                    }
                    if let Some(b) = &best {
                        best_hist.push((b.routed, b.len + opts.via_cost_nm as f64 * b.vias as f64));
                        ctl.update(
                            "route",
                            iter_base + it,
                            b.routed,
                            live.len(),
                            b.vias,
                            run_t0.elapsed().as_secs_f64(),
                        );
                    }
                    if timed_out {
                        break;
                    }
                    if failed.is_empty()
                        && out.blocked.is_empty()
                        && out.ripped.is_empty()
                        && overuse == 0
                    {
                        converged = true;
                        break;
                    }
                    // Stuck: same routed count and same failed / blocked set, nothing left
                    // to negotiate (no overuse, no rip-up) for `STUCK_REPEATS` iterations.
                    let mut fb: Vec<usize> = failed.clone();
                    fb.extend(out.blocked.keys().copied());
                    fb.sort_unstable();
                    let sig = (nok, fb);
                    if overuse == 0 && out.ripped.is_empty() && last_sig.as_ref() == Some(&sig) {
                        stuck += 1;
                    } else {
                        stuck = 0;
                    }
                    last_sig = Some(sig);
                    if stuck >= STUCK_REPEATS {
                        warnings.push(format!("stuck after {it} iterations"));
                        stop_why = Some("stuck");
                        break;
                    }
                    if no_gain(&best_hist, opts) {
                        warnings.push(format!(
                            "no gain: no connection newly routed and cost improved < {}% \
                             over {} iterations, stopped after {it}",
                            opts.min_gain_pct, opts.patience
                        ));
                        stop_why = Some("no gain");
                        break;
                    }
                    let mut next: BTreeSet<usize> = failed.into_iter().collect();
                    next.extend(cascaded);
                    next.extend(out.blocked.keys().copied());
                    next.extend(out.ripped.iter().copied());
                    next.extend(overused_conns(&mg));
                    next.retain(|&i| perm[i].is_none());
                    active = next.into_iter().collect();
                    if active.is_empty() {
                        converged = true;
                        break;
                    }
                }
                if opts.steiner && !timed_out {
                    if let Some(b) = best.as_mut() {
                        let (fixed, n) = junction_repair_pass(&cx, &mut obs, &rt, &neg, b);
                        if n > 0 {
                            warnings.push(format!(
                                "junction repair: {fixed} of {n} connection(s) whose junction was \
                                 rejected routed again"
                            ));
                        }
                    }
                }
                ripup = neg.rip_report(m);
                if !opts.via_reduction {
                    via_status = "off".into();
                } else if timed_out {
                    via_status = "skipped: timeout".into();
                } else if let Some(b) = best.as_mut() {
                    let v0 = b.vias;
                    let r = via_reduction_pass(&cx, &mut obs, &rt, &neg, b);
                    via_status = if r.timed_out {
                        format!(
                            "skipped: timeout (removed {} of {v0} vias in {} candidates)",
                            r.removed, r.candidates
                        )
                    } else {
                        format!(
                            "removed {} of {v0} vias in {} candidates",
                            r.removed, r.candidates
                        )
                    };
                }
                if !opts.via_relaxation {
                    relax_status = "off".into();
                } else if timed_out {
                    relax_status = "skipped: timeout".into();
                } else if let Some(b) = best.as_mut() {
                    let r = via_relaxation_pass(&cx, &mut obs, &rt, b);
                    relax_status = r.status();
                }
                if !timed_out && opts.zero_slack_retry {
                    if let Some(b) = &best {
                        match zero_slack_pass(&cx, &mut obs, &rt, b) {
                            Ok((snap, zs)) => {
                                zs_wires = zs;
                                zs_status = "kept".into();
                                best = Some(snap);
                            }
                            Err(r) => zs_status = r,
                        }
                    }
                }
                if let Some(b) = &best {
                    // The obstacle anchors follow the best iteration (P7).
                    b.mg.sync_anchors(&mut obs, &rt.net_of());
                }
                if opts.via_shove {
                    if timed_out {
                        shove_status = "skipped: timeout".into();
                    } else if let Some(b) = best.as_mut() {
                        shove_status = via_shove_pass(&cx, &mut obs, &rt, b).status();
                    }
                }
                if !timed_out {
                    if let Some(b) = &best {
                        let eps_checker = Checker::new(model, opts);
                        let base: Vec<Item> = b
                            .res
                            .iter()
                            .filter_map(|r| match r {
                                Res::Ok(o) => Some(o),
                                _ => None,
                            })
                            .flat_map(|o| o.items.iter().cloned())
                            .collect();
                        blocks = collect_blocks(&cx, &b.mg, b, &eps_checker, &base);
                        if opts.partial_routes {
                            partials =
                                partial_pass(&cx, &obs, &b.mg, &blocks, &eps_checker, base, &orig);
                        }
                        // The probe and the stubs stop at the deadline: a run that ended "stuck"
                        // before it but lost its block points / stubs to it is reported as a timeout.
                        if crate::control::expired(cx.deadline) {
                            timed_out = true;
                        }
                    }
                }
            }
        }
    }

    // A cancel that arrived while a late stage (via reduction, zero-slack retry, stubs) was
    // running ends those stages early: the result is the anytime result, reported as cancelled.
    if ctl.is_cancelled() {
        timed_out = true;
    }
    let stop_reason: &str = if timed_out {
        if ctl.is_cancelled() {
            "cancel"
        } else {
            "timeout"
        }
    } else if converged {
        "converged"
    } else if let Some(w) = stop_why {
        w
    } else if loop_ran {
        "cap"
    } else if setup_fail.is_some() {
        "stuck"
    } else {
        "converged"
    };
    // Assemble.
    let mut by_orig: BTreeMap<usize, usize> = BTreeMap::new();
    for (i, &o) in orig.iter().enumerate() {
        by_orig.insert(o, i);
    }
    let no_result = if timed_out {
        "timeout"
    } else {
        "not routed: no completed iteration"
    };
    let mut items: Vec<Item> = Vec::new();
    let mut item_conn: Vec<usize> = Vec::new();
    let mut reports: Vec<ConnReport> = Vec::with_capacity(all.len());
    let (mut total_len, mut out_total) = (0.0, 0.0);
    let mut by_layer: BTreeMap<String, f64> = BTreeMap::new();
    let first_layer = routable.first().copied().unwrap_or(0);
    for c in all {
        let name = dec
            .nets
            .iter()
            .find(|n| n.net == c.net)
            .map(|n| n.name.clone())
            .unwrap_or_default();
        let mut rep = ConnReport {
            id: c.id,
            net: c.net,
            name,
            layer: first_layer,
            width: c.width,
            status: "failed".into(),
            reason: String::new(),
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
        };
        if let Some(&k) = bridged.get(&c.id) {
            // A6 bridge: the connection is the one planned track (checked with the plan).
            let st = &stub_plan.stubs[k];
            stub_used[k] = true;
            let (l, o) = (st.len(), st.outside_len(model));
            rep.status = "routed".into();
            rep.layer = st.layer;
            rep.layers = vec![layer_name(model, st.layer)];
            rep.length_nm = l.round() as i64;
            rep.length_outside_pads_nm = o.round() as i64;
            rep.pattern = "S".into();
            rep.stub_nm = l.round() as i64;
            rep.ghost_len_nm = Some(l.round() as i64);
            if opts.detour_reference {
                rep.ref_len_nm = Some(l.round() as i64);
                rep.ref_kind = "feasible".into();
            }
            *by_layer.entry(layer_name(model, st.layer)).or_insert(0.0) += l;
            total_len += l;
            out_total += o;
            items.push(st.item(None));
            item_conn.resize(items.len(), rep.id);
            reports.push(rep);
            continue;
        }
        if let Some(r) = &early[c.id] {
            rep.reason = r.clone();
        } else if let Some(s) = setup_fail.as_ref() {
            rep.reason = s.clone();
        } else {
            let i = by_orig[&c.id];
            // The references run between the stub ends; the stubs are part of every pad-to-pad
            // path, so they count on both sides of the detour ratio (A6).
            let stubs_i: Vec<usize> = conn_stubs[i].iter().flatten().copied().collect();
            let stub_len: f64 = stubs_i.iter().map(|&k| stub_plan.stubs[k].len()).sum();
            rep.ghost_len_nm = ghost_ref[i].map(|l| (l + stub_len).round() as i64);
            if opts.detour_reference {
                match (feas_ref[i], ghost_ref[i]) {
                    (Some(l), _) => {
                        rep.ref_len_nm = Some((l + stub_len).round() as i64);
                        rep.ref_kind = "feasible".into();
                    }
                    (None, Some(g)) => {
                        rep.ref_len_nm = Some((g + stub_len).round() as i64);
                        rep.ref_kind = "ghost".into();
                    }
                    _ => {}
                }
            }
            // Stub items of the connection (first / last item) with their lengths (A6 / NX).
            let mut emit_stubs = |k: usize,
                                  rep: &mut ConnReport,
                                  items: &mut Vec<Item>,
                                  item_conn: &mut Vec<usize>,
                                  by_layer: &mut BTreeMap<String, f64>|
             -> (f64, f64) {
                let st = &stub_plan.stubs[k];
                stub_used[k] = true;
                let (l, o) = (st.len(), st.outside_len(model));
                items.push(st.item(Some(i)));
                item_conn.resize(items.len(), rep.id);
                *by_layer.entry(layer_name(model, st.layer)).or_insert(0.0) += l;
                rep.stub_nm += l.round() as i64;
                if st.necked() {
                    rep.neck_nm += l.round() as i64;
                    rep.neck_width_nm =
                        Some(rep.neck_width_nm.map_or(st.width, |w| w.min(st.width)));
                }
                (l, o)
            };
            match &best {
                None => rep.reason = no_result.into(),
                Some(b) => {
                    rep.iteration = b.iteration;
                    rep.zero_slack = zs_wires.contains(&i);
                    match &b.res[i] {
                        Res::Ok(o) => {
                            rep.status = "routed".into();
                            rep.layer = o.layers.first().copied().unwrap_or(first_layer);
                            rep.vias = o.vias;
                            rep.layers = o.layers.iter().map(|&l| layer_name(model, l)).collect();
                            rep.via_sites = o.via_sites.clone();
                            for it in &o.items {
                                if let Item::Track(t) = it {
                                    *by_layer.entry(layer_name(model, t.layer)).or_insert(0.0) +=
                                        dequantise(&t.prim).length();
                                }
                            }
                            let (mut len, mut out) = (o.len, o.out);
                            if let Some(k) = conn_stubs[i][0] {
                                let (l, ou) = emit_stubs(
                                    k,
                                    &mut rep,
                                    &mut items,
                                    &mut item_conn,
                                    &mut by_layer,
                                );
                                len += l;
                                out += ou;
                            }
                            items.extend(o.items.iter().cloned());
                            item_conn.resize(items.len(), rep.id);
                            if let Some(k) = conn_stubs[i][1] {
                                let (l, ou) = emit_stubs(
                                    k,
                                    &mut rep,
                                    &mut items,
                                    &mut item_conn,
                                    &mut by_layer,
                                );
                                len += l;
                                out += ou;
                            }
                            rep.length_nm = len.round() as i64;
                            rep.length_outside_pads_nm = out.round() as i64;
                            rep.pattern = o.pattern.clone();
                            rep.anchors = o.anchors;
                            rep.crossings = o.crossings;
                            rep.junction = o.junction.is_some();
                            rep.junction_angle_deg = o.junction;
                            rep.g1_angle_deg = o.g1;
                            total_len += len;
                            out_total += out;
                        }
                        Res::Fail {
                            reason, blocking, ..
                        } => {
                            rep.reason = reason.clone();
                            rep.blocking = *blocking;
                            if rep.blocking.is_none() {
                                rep.blocking = blocks.get(&i).and_then(|b| b.report);
                            }
                            rep.probe_steps = blocks.get(&i).and_then(|b| b.steps);
                            match partials.get(&i) {
                                Some(Partial::Stubs(p)) => {
                                    rep.status = "partial".into();
                                    rep.reason = format!(
                                        "partial: {} stub(s) toward the blocking point",
                                        p.stubs
                                    );
                                    let (mut len, mut out) = (p.len, p.out);
                                    // Exit stubs join the partial pieces to their pads.
                                    for k in stubs_i.iter().copied() {
                                        let mut by_layer_p = BTreeMap::new();
                                        let (l, ou) = emit_stubs(
                                            k,
                                            &mut rep,
                                            &mut items,
                                            &mut item_conn,
                                            &mut by_layer_p,
                                        );
                                        len += l;
                                        out += ou;
                                    }
                                    rep.length_nm = len.round() as i64;
                                    rep.length_outside_pads_nm = out.round() as i64;
                                    rep.pattern = p.pattern.clone();
                                    rep.anchors = p.anchors;
                                    rep.crossings = p.crossings;
                                    items.extend(p.items.iter().cloned().map(Item::Track));
                                    item_conn.resize(items.len(), rep.id);
                                }
                                Some(Partial::Dropped) => {
                                    rep.reason = "partial stub failed self-check".into();
                                }
                                Some(Partial::Skipped) => {
                                    rep.reason = "partial stubs skipped: deadline".into();
                                }
                                None => {}
                            }
                        }
                    }
                }
            }
        }
        reports.push(rep);
    }

    let mut nets: Vec<NetRouteReport> = Vec::with_capacity(dec.nets.len());
    for n in &dec.nets {
        let cs: Vec<&ConnReport> = n.connections.iter().map(|&i| &reports[i]).collect();
        let count = |s: &str| cs.iter().filter(|r| r.status == s).count();
        let (routed, partial, failed) = (count("routed"), count("partial"), count("failed"));
        let status = if n.excluded {
            "excluded"
        } else if n.pour_served {
            "pour_served"
        } else if !n.in_scope {
            "out_of_scope"
        } else if cs.is_empty() {
            "complete"
        } else if routed == cs.len() {
            "routed"
        } else if routed == 0 && partial == 0 {
            "failed"
        } else {
            "partial"
        };
        nets.push(NetRouteReport {
            net: n.net,
            name: n.name.clone(),
            status: status.into(),
            connections: cs.len(),
            routed,
            partial,
            failed,
            vias: cs.iter().map(|r| r.vias).sum(),
            length_nm: cs.iter().map(|r| r.length_nm).sum(),
            length_outside_pads_nm: cs.iter().map(|r| r.length_outside_pads_nm).sum(),
        });
    }
    let nets_total = nets.iter().filter(|n| n.connections > 0).count();
    let nets_routed = nets
        .iter()
        .filter(|n| n.connections > 0 && n.status == "routed")
        .count();
    let routed = reports.iter().filter(|r| r.status == "routed").count();
    let partial = reports.iter().filter(|r| r.status == "partial").count();
    let failed = reports.iter().filter(|r| r.status == "failed").count();
    let zero_slack_wires: Vec<ZeroSlackWire> = reports
        .iter()
        .filter(|r| r.zero_slack)
        .map(|r| ZeroSlackWire {
            conn: r.id,
            net: r.net,
            name: r.name.clone(),
        })
        .collect();
    let reports_vias: usize = reports.iter().map(|r| r.vias).sum();
    RouteResult {
        items,
        connections_total: reports.len(),
        connections: reports,
        nets,
        routed,
        partial,
        failed,
        nets_total,
        nets_routed,
        total_length_nm: total_len.round() as i64,
        length_outside_pads_nm: out_total.round() as i64,
        vias: reports_vias,
        length_by_layer_nm: by_layer
            .into_iter()
            .map(|(k, v)| (k, v.round() as i64))
            .collect(),
        sites: n_sites,
        vias_in_pads: Vec::new(),
        via_reduction: via_status,
        via_relaxation: relax_status,
        via_shove: shove_status,
        ripup,
        iterations,
        converged,
        overuse_edges: best.as_ref().map_or(0, |b| b.overuse_edges),
        clinches: best
            .as_ref()
            .map(|b| b.clinches.clone())
            .unwrap_or_default(),
        relax_iterations: best.as_ref().map_or(0, |b| b.relax_iterations),
        self_check_rounds: best.as_ref().map_or(0, |b| b.rounds),
        check_candidates_visited: best.as_ref().map_or(0, |b| b.metr.check_visited),
        timed_out,
        stop_reason: stop_reason.to_string(),
        runtime_s: t0.elapsed().as_secs_f64(),
        threads_used: 1,
        warnings,
        zero_slack_wires,
        zero_slack: zs_status,
        profile: opts.profile.then_some(profile),
        item_conn,
        stubs: stub_plan
            .stubs
            .iter()
            .zip(&stub_used)
            .map(|(st, &u)| st.out(model, u))
            .collect(),
    }
}
