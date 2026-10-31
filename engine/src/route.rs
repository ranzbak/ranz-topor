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
//! Stall stop: the loop also ends after [`STALL_ITERS`] iterations without a better best
//! (by `(routed, vias, Σlen)`). Zero-slack retry (R6, `Options::zero_slack_retry`): after the
//! loop, [`zero_slack_pass`] lets one connection take a corridor pinched below ε with margin 0
//! against that corridor only (candidates: single-leg paths), re-routes the detoured connections
//! once and keeps the result when it routes at least as much with a shorter outside-pad length;
//! the harness DRC is the final judge. Partial stubs grow on the first leg of a blocked
//! connection's ghost (D6). The via-reduction pass is Task 10.

use crate::cdt::Crossing;
use crate::check::{Checker, ItemIndex};
use crate::decompose::{decompose, terminal_of, Connection, TermKind, TermRef};
use crate::geom::quant::{quantise_chain, round_nm, TrackPrim};
use crate::geom::{dequantise, Skeleton, Vec2};
use crate::graph::{GraphRules, RouteGraph, TerminalInfo};
use crate::model::{
    flip_track, BoardModel, ExistingCopper, Item, Options, PadAttr, Pt, TrackItem, ViaItem,
};
use crate::multigraph::MultiGraph;
use crate::negotiate::{
    detect_clinches, overused_conns, BlockInfo, ClinchReport, Negotiator, DETOUR_FACTOR,
    PRESENT_OVERUSE,
};
use crate::obstacle::{ObstacleSet, TerminalObstacle};
use crate::relax::{corridor_static, relax, Relaxer, RunOpts, SlotNesting, ZeroSlack};
use crate::search::{
    default_expansion_cap, search, tier_weight, via_weight, Goal, SearchOutcome, SearchParams,
    SlotPath,
};
use crate::sketch::{
    build_layer_cdt_masked, sketch_from_crossings, stack_order, unanchored_bends, AnchorRef,
    ConstraintSet, LayerSketch, Sketch,
};
use crate::smooth::{deadline_after, hand_outside, outside_len, terminal_skeletons};
use crate::taut::TautError;
use crate::vias::{via_sites, via_sizes, ViaSize};
use crate::wires::{extract_wires, Terminal, Wire};
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
    /// `routed`, `partial` or `failed`.
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
    /// `routed`, `partial`, `failed`, `pour_served`, `complete` or `out_of_scope`.
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
    /// Via-reduction pass outcome (Task 10): `not run` until then.
    pub via_reduction: String,
    pub iterations: u32,
    pub converged: bool,
    pub overuse_edges: usize,
    pub clinches: ClinchReport,
    pub relax_iterations: u32,
    pub self_check_rounds: u32,
    /// Grid candidates visited by the self-checks up to the reported iteration (scaling diagnostic).
    pub check_candidates_visited: usize,
    pub timed_out: bool,
    pub runtime_s: f64,
    pub threads_used: usize,
    pub warnings: Vec<String>,
    /// Zero-slack retry (R6) connections, by id.
    pub zero_slack_wires: Vec<ZeroSlackWire>,
    /// Outcome of the zero-slack retry: `off`, `kept`, `no candidate`, `no length gain`,
    /// `infeasible: …`, `skipped: deadline` or `not run`.
    pub zero_slack: String,
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
            TermKind::Pad(_) => "pad",
            TermKind::Via(_) => "via",
            TermKind::Copper(_) => "copper",
            TermKind::Dangling(_) => "dangling",
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
}

/// One metrized leg: its tracks and the via that ends it (if any).
#[derive(Clone, Debug)]
struct LegOk {
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

struct Metrized {
    /// `(live connection index, result)` for every connection in `routed`.
    res: Vec<(usize, Res)>,
    relax_iterations: u32,
    rounds: u32,
    timed_out: bool,
    /// Zero-slack identification pass (`ZsSpec` without corridor): the static obstacles that
    /// pinch the relaxed wire, `(anchors, walls)` (`corridor_obstacles`).
    zs_corridor: Option<(Vec<usize>, Vec<usize>)>,
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
    if faces.len() != cross.len() + 1 {
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

fn legs_of(mg: &MultiGraph, conn: usize) -> &[usize] {
    mg.conn_wires.get(conn).map_or(&[][..], |v| v.as_slice())
}

/// The two terminals of leg `ord` of live connection `conn` (P6): the real terminals at the
/// path ends, a `Terminal::Site` at every via; `None` when the path and the site list disagree.
fn leg_terminals(
    cx: &Ctx,
    mg: &MultiGraph,
    conn: usize,
    ord: usize,
    nlegs: usize,
) -> Option<(Terminal, Terminal)> {
    let c = &cx.conns[conn];
    let sites = mg.sites_of(conn);
    let site = |s: usize| Terminal::Site {
        site: s,
        at: mg.sites[s].at,
    };
    let start = if ord == 0 {
        terminal_of(&c.from)
    } else {
        site(*sites.get(ord - 1)?)
    };
    let pour = matches!(c.to.kind, TermKind::Pour(_));
    let goal = if ord + 1 == nlegs && !pour {
        terminal_of(&c.to)
    } else {
        site(*sites.get(ord)?)
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

/// The via item of site `s` for connection `conn` (KiCad frame).
fn via_item(mg: &MultiGraph, s: usize, net: i32, conn: usize) -> Item {
    let size = mg.owner[s].map_or(ViaSize { d: 0, drill: 0 }, |(_, z)| z);
    let [x, y] = kicad(mg.sites[s].at);
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
        timed_out: false,
        zs_corridor: None,
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
        }
    }
    // Step 1: re-sketch; a leg whose anchor sequence changed starts from scratch.
    let base = st.base_for(obs, mg);
    for &i in routed {
        let c = &cx.conns[i];
        let ks = legs_of(mg, i).to_vec();
        let installing = st.conn_legs[i].is_empty();
        if installing {
            st.conn_legs[i] = ks.clone();
        }
        for (ord, &k) in ks.iter().enumerate() {
            st.ensure(k);
            let (Some(li), Some((start_t, goal_t))) =
                (mg.leg_layer(k), leg_terminals(cx, mg, i, ord, ks.len()))
            else {
                st.drop_leg(k);
                st.fail[k] = Some(fail("sketch: inconsistent slot path"));
                continue;
            };
            if installing {
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
            let ls = &*base[li];
            let Some(path) = leg_crossings(g, k, start_t.at(), goal_t.at(), ls) else {
                st.drop_leg(k);
                st.fail[k] = Some(fail("sketch: inconsistent slot path"));
                continue;
            };
            let (start, goal) = (start_t.at(), goal_t.at());
            if let Some(p) = unanchored_bends(start, goal, &path, ls).first() {
                st.drop_leg(k);
                st.fail[k] = Some(fail(format!(
                    "sketch: path bends at a wall split vertex ({}, {})",
                    kicad(*p)[0],
                    kicad(*p)[1]
                )));
                continue;
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
        }
    }
    let nk = st.seqs.len();
    let present: Vec<usize> = (0..nk).filter(|&k| st.seqs[k].is_some()).collect();
    // Stacks from the nesting of the crossings on the sketch triangulation (Fix-B1 (a)); leg ids (unique
    // across layers, so the per-layer nestings merge without collisions).
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
    for &k in &run.changed {
        changed[k] = true;
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
                    items.push(via_item(mg, s, w.net, conn));
                }
                st.ok[k] = Some(LegOk {
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

    // Step 3: self-check rounds (fallback granularity = connection) of the pending legs
    // against the checked ones (listed first: a violation is charged to the later item, the
    // wire that moved). A failed leg stays pending for the next call.
    for &k in &present {
        if st.pending[k] && st.ok[k].is_some() {
            st.fail[k] = None;
        }
    }
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
        if Instant::now() >= cx.deadline {
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
            let sites = mg.sites_of(i);
            let mut layers: Vec<u32> = Vec::new();
            for &k in ks {
                let l = cx.layers[st.leg_layer[k]];
                if !layers.contains(&l) {
                    layers.push(l);
                }
            }
            Res::Ok(ConnOk {
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
                via_sites: sites.iter().map(|&s| kicad(mg.sites[s].at)).collect(),
            })
        };
        out.res.push((i, r));
    }
    out
}

/// Bisection steps of the stub length (`λ` resolution < 3 nm on a board).
const STUB_BISECT_STEPS: u32 = 24;
/// Group name of the emitted stubs.
const PARTIAL_GROUP: &str = "topor-partial";

enum Partial {
    Stubs(PartialOk),
    /// Stubs were drawn but none survived the self-check.
    Dropped,
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
/// Consecutive iterations without a better best (by `(routed, Σlen)`) after which `route` stops
/// (stall stop, R6).
pub const STALL_ITERS: u32 = 8;
/// Zero-slack retry: smallest estimated-length gain (nm) that makes a connection a candidate.
const ZS_GAIN_MIN_NM: f64 = 100_000.0;
/// Zero-slack retry: candidates tried (by descending gain) before the pass gives up.
const ZS_MAX_CANDIDATES: usize = 4;
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
        if *steps >= PROBE_STEP_CAP || Instant::now() >= deadline {
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
    if wires.is_empty() || Instant::now() >= cx.deadline {
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
            Partial::Dropped => {
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
/// A reduction may lengthen the connection's outside-pad length by at most this factor.
pub const REDUCTION_LEN_SLACK: f64 = 1.10;

/// Counters of [`via_reduction_pass`].
struct Reduction {
    /// Vias removed.
    removed: usize,
    /// Candidate connections tried.
    candidates: usize,
    /// The deadline cut the pass short.
    timed_out: bool,
}

/// Via-reduction (M3-R2, TopoR lifting): one pass over the connections with `vias >= 1`, in
/// order `(-vias, id)`. Each leaves the graph and is searched again with the via cost times
/// [`REDUCTION_VIA_FACTOR`] against the final history; the result replaces the old path iff it has
/// fewer vias, metrizes with every previously routed connection still routed, adds no overuse
/// and its outside-pad length is at most [`REDUCTION_LEN_SLACK`] times the old one. A trial runs
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
    let mut cands: Vec<(usize, usize)> = (0..m)
        .filter_map(|i| match &best.res[i] {
            Res::Ok(k) if k.vias >= 1 && rt.goals[i].pour_sites.is_empty() => Some((k.vias, i)),
            _ => None,
        })
        .collect();
    cands.sort_by_key(|&(v, i)| (std::cmp::Reverse(v), i));
    let mut out = Reduction {
        removed: 0,
        candidates: 0,
        timed_out: false,
    };
    for (_, c) in cands {
        if Instant::now() >= cx.deadline {
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
            present_overuse: PRESENT_OVERUSE,
            expansion_cap: default_expansion_cap(&mg),
            deadline: cx.deadline,
            eps: None,
            via_cost: cx.opts.via_cost_nm as f64 * via_weight(live[c].tier) * REDUCTION_VIA_FACTOR,
            max_vias: cx.opts.max_vias_per_connection,
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
        let routed: Vec<usize> = (0..m)
            .filter(|&i| matches!(best.res[i], Res::Ok(_)))
            .collect();
        let mut metr = best.metr.clone();
        let mt = metrize(cx, obs, &mg, &routed, &mut metr, &[c]);
        if mt.timed_out {
            out.timed_out = true;
            break;
        }
        let all_ok =
            mt.res.len() == routed.len() && mt.res.iter().all(|(_, r)| matches!(r, Res::Ok(_)));
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
            *i == c && matches!(r, Res::Ok(k) if k.out <= old.out * REDUCTION_LEN_SLACK)
        });
        if !(all_ok && short && overuse <= best.overuse_edges) {
            continue;
        }
        let mut res = best.res.clone();
        for (i, r) in mt.res {
            res[i] = r;
        }
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
        best.relax_iterations = mt.relax_iterations;
        best.rounds = mt.rounds;
        best.paths = paths;
        best.mg = mg;
        best.metr = metr;
    }
    best.mg.sync_anchors(obs, &rt.net_of());
    out
}

/// Zero-slack retry (R6), one pass from the best iteration's graph. The detoured connections
/// (`est_len > DETOUR_FACTOR × straight`) leave the graph; every routed connection is searched
/// once at margin 0 (empty history). The candidates (single-leg paths, estimated gain >
/// [`ZS_GAIN_MIN_NM`]) are tried by descending gain (ties: lowest id), at most
/// [`ZS_MAX_CANDIDATES`] of them, each in a fresh copy of the best graph
/// ([`zero_slack_attempt`]); the first that routes at least as much with a shorter outside-pad
/// length wins. Returns the new snapshot and the zero-slack connections (empty when the
/// candidate's new path turned out feasible at ε); `Err(reason)` (the last attempt's) leaves the
/// best iteration unchanged. `obs` is left in the state of the last attempt: the caller re-syncs
/// it. Complexity: one margin-0 search per routed connection plus ≤ `ZS_MAX_CANDIDATES`
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
    let detoured: Vec<usize> = (0..m)
        .filter(|&i| {
            ok(i)
                && best.paths[i].as_ref().is_some_and(|p| {
                    p.legs.iter().any(|l| !l.crossings.is_empty())
                        && p.est_len > DETOUR_FACTOR * (live[i].from.at - live[i].to.at).len()
                })
        })
        .collect();
    for &d in &detoured {
        mg.remove_conn(d, obs);
    }
    let cap = default_expansion_cap(&mg);
    let params = |tier: u8, eps: Option<f64>| SearchParams {
        history: &[],
        site_history: &[],
        weight: tier_weight(tier),
        present_overuse: PRESENT_OVERUSE,
        expansion_cap: cap,
        deadline: cx.deadline,
        eps,
        via_cost: cx.opts.via_cost_nm as f64 * via_weight(tier),
        max_vias: cx.opts.max_vias_per_connection,
    };
    // Candidates: margin-0 searches over the routed connections (a detoured one with the other
    // detoured ones absent; any other with its own path taken out and put back).
    let mut cands: Vec<(f64, usize, SlotPath)> = Vec::new();
    for i in (0..m).filter(|&i| ok(i)) {
        let Some(old) = best.paths[i].as_ref().map(|p| p.est_len) else {
            continue;
        };
        if Instant::now() >= cx.deadline {
            return Err(ZS_DEADLINE.into());
        }
        let out = if detoured.contains(&i) {
            search(
                &mg,
                obs,
                &live[i],
                &rt.starts[i],
                &rt.goals[i],
                &params(live[i].tier, Some(0.0)),
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
                &params(live[i].tier, Some(0.0)),
            );
            mg.insert_path(&live[i], &taken, obs);
            out
        };
        match out {
            SearchOutcome::Found(p) => {
                let gain = old - p.est_len;
                if gain > ZS_GAIN_MIN_NM && p.legs.len() == 1 && p.sites.is_empty() {
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
    for (_, c, p) in cands.into_iter().take(ZS_MAX_CANDIDATES) {
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
    params: &dyn Fn(u8, Option<f64>) -> SearchParams<'a>,
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
        if Instant::now() >= cx.deadline {
            return Err(ZS_DEADLINE.into());
        }
        work.push(d);
        match search(
            &mg,
            obs,
            &live[d],
            &rt.starts[d],
            &rt.goals[d],
            &params(live[d].tier, None),
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
                    },
                );
            }
            SearchOutcome::Timeout => return Err(ZS_DEADLINE.into()),
        }
    }
    let routed: Vec<usize> = (0..m).filter(|&i| paths[i].is_some() && ok(i)).collect();
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

/// Route the unconnected nets of `model` on every routable copper layer, with through vias at
/// pre-placed sites (M3): decompose, obstacles, via sites, one graph per layer, PathFinder over
/// layers and sites, per-leg metrization with a self-check, partial stubs, reports.
///
/// Complexity: dominated by the PathFinder loop, `O(iters · m · V' log V')` with
/// `V' = Σ_L 2(E_L + X_L) + 2S`, plus `O(changed · q)` self-checks per iteration (see the
/// module header, `negotiate` and `multigraph`).
pub fn route(model: &BoardModel, opts: &Options) -> RouteResult {
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

    let mut best: Option<Snap> = None;
    let (mut iterations, mut converged, mut timed_out) = (0u32, false, false);
    let mut setup_fail: Option<String> = None;
    let mut partials: BTreeMap<usize, Partial> = BTreeMap::new();
    let mut blocks: BTreeMap<usize, Blk> = BTreeMap::new();
    let mut zs_wires: Vec<usize> = Vec::new();
    let mut n_sites = 0usize;
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
                if let (TermKind::Copper(_) | TermKind::Dangling(_), Some(&layer)) =
                    (&t.kind, t.layers.first())
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
                TermKind::Pad(pi) if model.pads[pi].attr == PadAttr::Smd => Some(pi),
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
            .flat_map(|n| via_sizes(&model.rules, n))
            .collect();
        sizes.sort_by_key(|z| std::cmp::Reverse((z.d, z.drill)));
        sizes.dedup();
        let fit_checker = Checker::new(model, opts);
        let sites = via_sites(
            model,
            &obs,
            &fit_checker,
            &routable,
            &escape,
            &sizes,
            opts.margin_nm as f64,
        );
        n_sites = sites.len();

        // Terminals of the graphs: one vertex per position, on the layers it has copper on.
        let mut term_at: BTreeMap<(u64, u64), usize> = BTreeMap::new();
        let mut terms: Vec<(TerminalInfo, Vec<u32>)> = Vec::new();
        for c in &live {
            for t in [&c.from, &c.to] {
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
                                    TermKind::Copper(_) | TermKind::Dangling(_) => {
                                        0.5 * c.width as f64
                                    }
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
                            let pour_sites: Vec<usize> = (0..mg.sites.len())
                                .filter(|&s| mg.in_pour[s].contains(&zi))
                                .collect();
                            if pour_sites.is_empty() {
                                perm[i] = Some("no via site inside pour".into());
                            }
                            Goal {
                                vertices: Vec::new(),
                                pour_layer: routable.iter().position(|l| c.to.layers.contains(l)),
                                pour_sites,
                            }
                        }
                        _ => Goal {
                            vertices: vertices(&c.to),
                            pour_sites: Vec::new(),
                            pour_layer: None,
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
                let rt = Route {
                    starts: &starts,
                    goals: &goals,
                    nets: live.iter().map(|c| c.net).collect(),
                };
                let mut neg = Negotiator::new(&mg, m);
                let mut active: Vec<usize> = (0..m).filter(|&i| perm[i].is_none()).collect();
                let (mut stuck, mut last_sig) = (0u32, None::<(usize, Vec<usize>)>);
                let mut stale = 0u32;
                let mut metr = MetrState::new(&cx, &obs);
                for _ in 1..=opts.pathfinder_iter_cap {
                    if Instant::now() >= deadline {
                        timed_out = true;
                        break;
                    }
                    let out = neg.iteration(
                        &mut mg, &mut obs, &live, &active, &starts, &goals, opts, deadline,
                    );
                    if out.timed_out {
                        timed_out = true;
                        break;
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
                    let mt = metrize(&cx, &obs, &mg, &routed, &mut metr, &work);
                    // Anytime: a timed-out metrization still yields what it completed; the
                    // loop ends after this iteration's result is recorded.
                    if mt.timed_out {
                        timed_out = true;
                        if mt.res.is_empty() {
                            break;
                        }
                    }
                    iterations = it;
                    let mut res: Vec<Res> = (0..m)
                        .map(|i| match (&perm[i], out.blocked.get(&i)) {
                            (Some(r), _) => Res::Fail {
                                reason: r.clone(),
                                perm: true,
                                blocking: None,
                            },
                            (None, Some(b)) => Res::Fail {
                                reason: if b.edge.is_some() || b.site.is_some() {
                                    "blocked: no path (wall, capacity or via sites)".into()
                                } else {
                                    "blocked: no path".into()
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
                            (None, None) => fail("not routed"),
                        })
                        .collect();
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
                    let overuse = neg.penalise_overuse(&mg);
                    let (nok, vias, len, out_len) = tally(&res);
                    let better = best.as_ref().is_none_or(|b| better_than(nok, vias, len, b));
                    if better {
                        stale = 0;
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
                    } else {
                        stale += 1;
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
                        break;
                    }
                    if stale >= STALL_ITERS {
                        warnings.push(format!(
                            "stalled: no better routing for {STALL_ITERS} iterations, \
                             stopped after {it}"
                        ));
                        break;
                    }
                    let mut next: BTreeSet<usize> = failed.into_iter().collect();
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
                    }
                }
            }
        }
    }

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
        };
        if let Some(r) = &early[c.id] {
            rep.reason = r.clone();
        } else if let Some(s) = setup_fail.as_ref() {
            rep.reason = s.clone();
        } else {
            let i = by_orig[&c.id];
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
                            rep.length_nm = o.len.round() as i64;
                            rep.length_outside_pads_nm = o.out.round() as i64;
                            rep.pattern = o.pattern.clone();
                            rep.anchors = o.anchors;
                            rep.crossings = o.crossings;
                            total_len += o.len;
                            out_total += o.out;
                            items.extend(o.items.iter().cloned());
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
                                    rep.length_nm = p.len.round() as i64;
                                    rep.length_outside_pads_nm = p.out.round() as i64;
                                    rep.pattern = p.pattern.clone();
                                    rep.anchors = p.anchors;
                                    rep.crossings = p.crossings;
                                    items.extend(p.items.iter().cloned().map(Item::Track));
                                }
                                Some(Partial::Dropped) => {
                                    rep.reason = "partial stub failed self-check".into();
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
        let status = if n.pour_served {
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
        via_reduction: via_status,
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
        runtime_s: t0.elapsed().as_secs_f64(),
        threads_used: 1,
        warnings,
        zero_slack_wires,
        zero_slack: zs_status,
    }
}
