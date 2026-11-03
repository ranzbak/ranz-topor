//! Pad exit stubs and necking to the pad (M4a task A6 + NX; design note
//! `docs/progress/notes/m4-capacity-steiner-design.md` §3, ruling A-C9, A2 "Key finding").
//!
//! A stub is a straight track of the pad's net from a point `S` inside the pad, along one of the
//! pad's axes `d̂`, to `E = c + d̂ · (h + clr + w/2 + ε) + o · n̂` just outside the pad (`h` = the
//! pad's extent along `d̂`, `c` its centre, `o` a lateral offset inside the pad, `w` the net
//! width, `clr` the connection's clearance). The connection then starts (or ends) at `E` like at
//! a hand-copper junction ([`crate::decompose::TermKind::Stub`]): the search, the metrization and
//! the self-check all see the stub as fixed copper of the net (the planned stubs are added to the
//! model as kept items, [`with_stubs`]), and the routed connection emits the stub as its first /
//! last item. Three escapes the pad centre does not have:
//! - **off-centre start** (A2 finding): a pad whose centre line is closer than the clearance to a
//!   neighbour (dcsgone U7 middle pads, 0.175 mm to the GND pads) is left on a parallel line
//!   inside the pad (`o ≠ 0`, the feasible run of offsets nearest to the centre, its middle);
//! - **necking** (NX, `Options.neck_to_pad`): a net wider than what fits between the neighbours
//!   leaves the pad with a narrower stub, `min_w ≤ width ≤ w`, the widest the [`Checker`] accepts
//!   (bisection), `min_w` = KiCad's minimum track width or the net's custom-rule minimum, at most
//!   `neck_max` long (`Options.neck_max_nm`, 0 = twice the pad's long side);
//! - **exit point outside the pad row**: the route starts where every incident edge fits.
//!
//! Modes (`Options.exit_stubs`): `"off"` = M3 (no stub); `"auto"` = a stub only for a pad whose
//! straight centre exit at full width (`S = c`, `o = 0`, along any axis, end disc at `E`
//! included; "T0") violates the clearance (THT pads never: eprom is byte-identical); `"all"` =
//! additionally every SMD terminal pad with a long axis. A pad that needs a stub and has no
//! feasible candidate gets the warning `stub infeasible: <pad>` and keeps its M3 terminal.
//! Candidate order per pad: directions (long axis first, then free room at `E`, then toward the
//! connections' other ends) × start points `t ∈ {0, L/2, L}` along the axis; A1 centred at full
//! width, B1 off-centre at full width, A2 centred necked, B2 off-centre necked (necks: the widest).
//!
//! Bridges (phase 1, `Options.pad_bridge_max_nm`): a connection between two SMD pads of one
//! footprint at most that far apart whose straight line (centre line, else the nearest clean
//! offset run) is clean is routed by that one track (auto: only when one of the pads fails T0) —
//! U7.4 → U7.7 across the UDFN 50 µm off the centre line, where any exit stub would have to go
//! around the package. Its connection leaves the routing pass; the pads' other connections get
//! exit stubs in phase 2.
//!
//! Every candidate is verified by the [`Checker`] (margin ε) against the board and the stubs
//! accepted before it (other nets), so stubs never violate the rules.
//!
//! Complexity: `O(T · K · q + B · K · q)` for `T` terminal pads, `B` bridge candidates, `K ≤
//! STUB_CHECK_CAP` candidate checks per pad (directions × start points × (width bisection +
//! offset samples), capped) and `q` grid candidates per check. The pass checks `stop` (deadline /
//! cancel) before every pad and every bridge.

use crate::check::{Checker, ItemIndex};
use crate::decompose::{Connection, TermKind};
use crate::geom::quant::{round_nm, TrackPrim};
use crate::geom::{dist_prim_skeleton, Prim, Seg, Skeleton, Vec2};
use crate::model::{flip_track, BoardModel, Item, KeptItem, Options, PadAttr, TrackItem};
use serde::Serialize;
use std::collections::BTreeMap;

/// Lateral offset sample step of the off-centre scan (nm): the margin ε, so a corridor of a few
/// ε (dcsgone U7: 40 µm) is found.
pub const OFFSET_STEP_NM: f64 = 5_000.0;
/// Most offset samples per scan (the step grows on wide pads).
pub const OFFSET_SAMPLES_CAP: usize = 128;
/// Bisection steps of the neck width (`(w − min_w) / 2^12` < 0.1 µm on a board).
pub const WIDTH_BISECT_STEPS: u32 = 12;
/// Most [`Checker`] calls per pad (backstop of the candidate loops).
pub const STUB_CHECK_CAP: usize = 4096;
/// Least depth (nm) of the stub start inside its pad.
pub const START_DEPTH_NM: f64 = 5_000.0;

/// `Options.exit_stubs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StubMode {
    Off,
    Auto,
    All,
}

impl StubMode {
    /// `"off" | "auto" | "all"`; anything else is `None`.
    pub fn parse(s: &str) -> Option<StubMode> {
        match s {
            "off" => Some(StubMode::Off),
            "auto" => Some(StubMode::Auto),
            "all" => Some(StubMode::All),
            _ => None,
        }
    }
}

/// One planned exit stub (math frame, integer nm coordinates).
#[derive(Clone, Debug, PartialEq)]
pub struct ExitStub {
    /// Index into `model.pads`.
    pub pad: usize,
    /// `"U7.4"`.
    pub label: String,
    pub net: i32,
    pub layer: u32,
    /// Inside the pad.
    pub start: Vec2,
    /// Just outside the pad: the connection's terminal.
    pub end: Vec2,
    /// Stub width; `< net_width` when necked.
    pub width: i64,
    pub net_width: i64,
    /// Lateral offset of the stub line from the pad centre, nm (0 = centred).
    pub offset: f64,
    /// A bridge (phase 1 of [`plan_stubs`]): `end` lies inside this other pad of the net, the
    /// stub is the whole connection.
    pub bridge: Option<usize>,
}

impl ExitStub {
    pub fn len(&self) -> f64 {
        (self.end - self.start).len()
    }

    /// Never true for a planned stub (`start != end`); clippy's `len` companion.
    pub fn is_empty(&self) -> bool {
        self.len() == 0.0
    }

    pub fn necked(&self) -> bool {
        self.width < self.net_width
    }

    /// The stub's primitive in the math frame.
    pub fn prim(&self) -> TrackPrim {
        TrackPrim::Seg {
            a: round_nm(self.start),
            b: round_nm(self.end),
        }
    }

    /// The stub as an output item (KiCad frame), owned by `wire`.
    pub fn item(&self, wire: Option<usize>) -> Item {
        Item::Track(TrackItem {
            net: self.net,
            layer: self.layer,
            width: self.width,
            prim: flip_track(self.prim()),
            group: None,
            wire,
        })
    }

    /// Length of the stub outside its pad's copper (the `length_outside_pads` share). O(1).
    pub fn outside_len(&self, model: &BoardModel) -> f64 {
        let mut sks = vec![model.skeleton(&model.pads[self.pad], self.layer)];
        if let Some(b) = self.bridge {
            sks.push(model.skeleton(&model.pads[b], self.layer));
        }
        crate::lengths::outside_len(
            &[Prim::Seg(Seg {
                a: self.start,
                b: self.end,
            })],
            &sks,
        )
    }

    /// Report row (KiCad frame).
    pub fn out(&self, model: &BoardModel, used: bool) -> StubOut {
        let k = |p: Vec2| [p.x.round() as i64, (-p.y).round() as i64];
        StubOut {
            pad: self.label.clone(),
            net: self.net,
            layer: self.layer,
            width: self.width,
            net_width: self.net_width,
            start: k(self.start),
            end: k(self.end),
            length_nm: self.len().round() as i64,
            outside_nm: self.outside_len(model).round() as i64,
            necked: self.necked(),
            offset_nm: self.offset.round() as i64,
            bridge: self.bridge.map(|b| {
                let p = &model.pads[b];
                format!("{}.{}", p.ref_, p.number)
            }),
            used,
        }
    }
}

/// `RouteResult.stubs` row: one planned stub of a pass.
#[derive(Clone, Debug, Serialize)]
pub struct StubOut {
    pub pad: String,
    pub net: i32,
    pub layer: u32,
    pub width: i64,
    pub net_width: i64,
    /// KiCad frame, nm.
    pub start: [i64; 2],
    pub end: [i64; 2],
    pub length_nm: i64,
    /// Part of `length_nm` outside the pad's copper.
    pub outside_nm: i64,
    /// Narrower than the net width (NX).
    pub necked: bool,
    /// Lateral offset from the pad centre line, nm (0 = centred).
    pub offset_nm: i64,
    /// A bridge: the other pad (`"U7.7"`) the stub ends in; the stub is the whole connection.
    pub bridge: Option<String>,
    /// Emitted with at least one routed (or partial) connection of the pass.
    pub used: bool,
}

/// The stubs of one routing pass.
#[derive(Clone, Debug, Default)]
pub struct StubPlan {
    pub stubs: Vec<ExitStub>,
    /// Pad index -> index into `stubs` (exit stubs).
    pub by_pad: BTreeMap<usize, usize>,
    /// `Connection.id` -> index into `stubs` (bridges: the connection is that one track).
    pub bridges: BTreeMap<usize, usize>,
    /// `stub infeasible: <pad>`, unknown mode, deadline.
    pub warnings: Vec<String>,
    /// Checker calls made (diagnostics).
    pub checks: usize,
}

impl StubPlan {
    pub fn is_empty(&self) -> bool {
        self.stubs.is_empty()
    }
}

/// Per terminal pad: what its connections need.
struct PadNeed {
    layer: u32,
    net: i32,
    width: i64,
    clr: i64,
    /// Positions of the other ends of its connections (direction preference).
    targets: Vec<Vec2>,
}

/// Support of the skeleton about `c` along unit `d` (extent of the copper from `c`). O(points).
fn support(sk: &Skeleton, c: Vec2, d: Vec2) -> f64 {
    match sk {
        Skeleton::Point { p, r } => (*p - c).dot(d) + r,
        Skeleton::Seg { s, r } => (s.a - c).dot(d).max((s.b - c).dot(d)) + r,
        Skeleton::Poly { pts, r } => {
            pts.iter().map(|p| (*p - c).dot(d)).fold(f64::MIN, f64::max) + r
        }
    }
}

/// The pad's own axis `u` (long side when it has one): polygon longest edge, oval axis, or the
/// pad rotation for a circle. O(points).
fn axis(sk: &Skeleton, rot_deg: f64) -> Vec2 {
    let fallback = Vec2 { x: 1.0, y: 0.0 }.rot(rot_deg.to_radians());
    match sk {
        Skeleton::Point { .. } => fallback,
        Skeleton::Seg { s, .. } => (s.b - s.a).unit().unwrap_or(fallback),
        Skeleton::Poly { pts, .. } => {
            let n = pts.len();
            let mut best: Option<Vec2> = None;
            let mut best_l = 0.0;
            for i in 0..n {
                let e = pts[(i + 1) % n] - pts[i];
                if e.len() > best_l + 1.0 {
                    best_l = e.len();
                    best = Some(e);
                }
            }
            best.and_then(Vec2::unit).unwrap_or(fallback)
        }
    }
}

/// Snap to integer nm (the model frame of kept items and terminals).
fn snap(p: Vec2) -> Vec2 {
    Vec2 {
        x: p.x.round(),
        y: p.y.round(),
    }
}

/// Coarse pad grid for the free-distance probe of the direction order. Build O(P), query O(q).
struct PadGrid {
    cell: f64,
    cells: BTreeMap<(i64, i64), Vec<usize>>,
}

impl PadGrid {
    const CELL_NM: f64 = 2_000_000.0;

    fn new(model: &BoardModel) -> PadGrid {
        let cell = Self::CELL_NM;
        let mut cells: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
        for (i, p) in model.pads.iter().enumerate() {
            let c = p.center.to_vec2();
            cells
                .entry(((c.x / cell).floor() as i64, (c.y / cell).floor() as i64))
                .or_default()
                .push(i);
        }
        PadGrid { cell, cells }
    }

    /// Pads whose centre lies within `r` (+ one cell, for the pad extent) of `p`.
    fn near(&self, p: Vec2, r: f64) -> Vec<usize> {
        let k = ((r / self.cell).ceil() as i64) + 1;
        let (cx, cy) = (
            (p.x / self.cell).floor() as i64,
            (p.y / self.cell).floor() as i64,
        );
        let mut out = Vec::new();
        for x in cx - k..=cx + k {
            for y in cy - k..=cy + k {
                if let Some(v) = self.cells.get(&(x, y)) {
                    out.extend(v.iter().copied());
                }
            }
        }
        out.sort_unstable();
        out
    }
}

/// Plans the exit stubs of the connections `conns` (see the module docs). Deterministic: pads in
/// index order, candidates in a fixed order, the [`Checker`] sequential.
pub fn plan_stubs(
    model: &BoardModel,
    opts: &Options,
    conns: &[Connection],
    routable: &[u32],
    stop: &dyn Fn() -> bool,
) -> StubPlan {
    let mut plan = StubPlan::default();
    let mode = match StubMode::parse(&opts.exit_stubs) {
        Some(m) => m,
        None => {
            plan.warnings.push(format!(
                "exit_stubs {:?} unknown (auto | all | off): auto used",
                opts.exit_stubs
            ));
            StubMode::Auto
        }
    };
    if mode == StubMode::Off {
        return plan;
    }
    let needs = pad_needs(model, conns, routable, &BTreeMap::new());
    if needs.is_empty() {
        return plan;
    }
    let checker = Checker::new(model, opts);
    let mut idx = ItemIndex::new(checker.cell());
    let grid = PadGrid::new(model);

    // Phase 1, bridges: a short same-net link between two pads of one footprint whose straight
    // line is clean, possibly only off the centre line (U7.4 -> U7.7 at 0.2 mm): the connection
    // is routed by that one track. Auto: only where one of the pads needs a stub (T0 fails).
    if opts.pad_bridge_max_nm > 0 {
        let mut t0: BTreeMap<usize, bool> = BTreeMap::new();
        for c in conns {
            if stop() {
                break;
            }
            let (Some((pa, la)), Some((pb, lb))) = (
                stub_pad(model, routable, &c.from),
                stub_pad(model, routable, &c.to),
            ) else {
                continue;
            };
            let (a, b) = (&model.pads[pa], &model.pads[pb]);
            let len = (b.center.to_vec2() - a.center.to_vec2()).len();
            if la != lb
                || a.ref_ != b.ref_
                || a.ref_.is_empty()
                || len > opts.pad_bridge_max_nm as f64
                || len == 0.0
            {
                continue;
            }
            if mode == StubMode::Auto {
                let mut need_stub = false;
                for pi in [pa, pb] {
                    let clean = match t0.get(&pi) {
                        Some(&v) => v,
                        None => {
                            let mut st =
                                Search::new(model, &checker, &idx, &grid, pi, &needs[&pi], opts);
                            let v = st.t0_clean();
                            plan.checks += st.checks;
                            t0.insert(pi, v);
                            v
                        }
                    };
                    need_stub |= !clean;
                }
                if !need_stub {
                    continue;
                }
            }
            let mut st = Search::new(model, &checker, &idx, &grid, pa, &needs[&pa], opts);
            let br = st.bridge(pb, c.width);
            plan.checks += st.checks;
            if let Some(s) = br {
                idx.insert(&s.item(None));
                plan.bridges.insert(c.id, plan.stubs.len());
                plan.stubs.push(s);
            }
        }
    }

    // Phase 2, exit stubs: the pads that keep a connection to route.
    let needs = if plan.bridges.is_empty() {
        needs
    } else {
        pad_needs(model, conns, routable, &plan.bridges)
    };
    for (&pi, need) in &needs {
        if stop() {
            plan.warnings
                .push("exit stubs: deadline, remaining pads keep their centre".into());
            break;
        }
        let mut st = Search::new(model, &checker, &idx, &grid, pi, need, opts);
        let res = st.run(mode);
        plan.checks += st.checks;
        match res {
            Outcome::NotNeeded => {}
            Outcome::Infeasible => {
                let p = &model.pads[pi];
                plan.warnings
                    .push(format!("stub infeasible: {}.{}", p.ref_, p.number));
            }
            Outcome::Stub(s) => {
                idx.insert(&s.item(None));
                if s.width < s.net_width {
                    // the full-width end disc is part of the connection's copper
                    idx.insert(&end_disc(&s));
                }
                plan.by_pad.insert(pi, plan.stubs.len());
                plan.stubs.push(s);
            }
        }
    }
    plan
}

/// The pad of a connection end that may carry a stub (SMD, one copper layer, routable) and its
/// layer. O(1).
fn stub_pad(
    model: &BoardModel,
    routable: &[u32],
    t: &crate::decompose::TermRef,
) -> Option<(usize, u32)> {
    let TermKind::Pad(pi) = t.kind else {
        return None;
    };
    let p = &model.pads[pi];
    if p.attr != PadAttr::Smd || p.layers.len() != 1 {
        return None;
    }
    let layer = p.layers[0];
    (routable.contains(&layer) && t.layers.contains(&layer)).then_some((pi, layer))
}

/// What the connections of each stub-capable terminal pad need, skipping the connections in
/// `skip` (by `Connection.id`). O(m log T).
fn pad_needs(
    model: &BoardModel,
    conns: &[Connection],
    routable: &[u32],
    skip: &BTreeMap<usize, usize>,
) -> BTreeMap<usize, PadNeed> {
    let mut needs: BTreeMap<usize, PadNeed> = BTreeMap::new();
    for c in conns.iter().filter(|c| !skip.contains_key(&c.id)) {
        for (t, o) in [(&c.from, &c.to), (&c.to, &c.from)] {
            let Some((pi, layer)) = stub_pad(model, routable, t) else {
                continue;
            };
            let e = needs.entry(pi).or_insert(PadNeed {
                layer,
                net: c.net,
                width: c.width,
                clr: c.clearance.max(model.rules.min_clearance),
                targets: Vec::new(),
            });
            e.width = e.width.max(c.width);
            e.clr = e.clr.max(c.clearance);
            e.targets.push(o.at);
        }
    }
    needs
}

/// Full-width disc at the stub end (KiCad frame item), the start of the routed track.
fn end_disc(s: &ExitStub) -> Item {
    let e = round_nm(s.end);
    Item::Track(TrackItem {
        net: s.net,
        layer: s.layer,
        width: s.net_width,
        prim: flip_track(TrackPrim::Seg { a: e, b: e }),
        group: None,
        wire: None,
    })
}

/// Of the runs of consecutive feasible samples (`ok[i]` at offset `offs[i]`, ascending), the one
/// nearest to offset 0 (first on ties), its middle sample: the most room to both sides. O(n).
fn nearest_run_mid(offs: &[f64], ok: &[bool]) -> Option<f64> {
    let mut best: Option<(f64, f64)> = None; // (distance to 0, offset)
    let mut i = 0;
    while i < ok.len() {
        if !ok[i] {
            i += 1;
            continue;
        }
        let j0 = i;
        while i < ok.len() && ok[i] {
            i += 1;
        }
        let (a, b) = (offs[j0], offs[i - 1]);
        let dist = if a <= 0.0 && b >= 0.0 {
            0.0
        } else {
            a.abs().min(b.abs())
        };
        let mid = offs[(j0 + i - 1) / 2];
        if best.is_none_or(|(d, _)| dist < d) {
            best = Some((dist, mid));
        }
    }
    best.map(|b| b.1)
}

enum Outcome {
    NotNeeded,
    Infeasible,
    Stub(ExitStub),
}

/// The candidate search of one pad.
struct Search<'a> {
    model: &'a BoardModel,
    checker: &'a Checker,
    idx: &'a ItemIndex,
    checks: usize,
    pi: usize,
    need: &'a PadNeed,
    eps: f64,
    opts: &'a Options,
    grid: &'a PadGrid,
}

/// One direction of a pad: unit axis `d`, normal `n`, extent `h` along `d`, lateral half extent.
#[derive(Clone, Copy)]
struct Dir {
    d: Vec2,
    n: Vec2,
    h: f64,
    half: f64,
}

impl<'a> Search<'a> {
    fn new(
        model: &'a BoardModel,
        checker: &'a Checker,
        idx: &'a ItemIndex,
        grid: &'a PadGrid,
        pi: usize,
        need: &'a PadNeed,
        opts: &'a Options,
    ) -> Search<'a> {
        Search {
            model,
            checker,
            idx,
            checks: 0,
            pi,
            need,
            eps: opts.margin_nm as f64,
            opts,
            grid,
        }
    }

    /// The pad's four axis directions `u, −u, n, −n` (`u` its long side when it has one).
    fn dirs(&self) -> (Vec2, Vec<Dir>) {
        let pad = &self.model.pads[self.pi];
        let sk = self.model.skeleton(pad, self.need.layer);
        let c = pad.center.to_vec2();
        let u = axis(&sk, pad.rotation_deg);
        let dirs = [u, -u, u.perp(), -u.perp()]
            .into_iter()
            .map(|d| Dir {
                d,
                n: d.perp(),
                h: support(&sk, c, d),
                half: support(&sk, c, d.perp()),
            })
            .collect();
        (c, dirs)
    }

    /// T0: the straight centre exit at full width (M3's attachment) is clean along an axis.
    /// At most 4 checks.
    fn t0_clean(&mut self) -> bool {
        let (c, dirs) = self.dirs();
        let (w, clr) = (self.need.width as f64, self.need.clr as f64);
        dirs.iter().any(|dr| {
            let (s, e) = self.ends(c, dr, 0.0, 0.0, w, clr);
            self.clean(s, e, self.need.width)
        })
    }

    /// The pad centre lies within clearance (+ ε) of foreign copper (pads overlapping a foreign
    /// clearance zone). One check.
    fn centre_blocked(&mut self) -> bool {
        let c = self.model.pads[self.pi].center.to_vec2();
        self.checks += 1;
        let it = Item::Track(TrackItem {
            net: self.need.net,
            layer: self.need.layer,
            width: 2,
            prim: flip_track(TrackPrim::Seg {
                a: round_nm(c),
                b: round_nm(c),
            }),
            group: None,
            wire: None,
        });
        !self
            .checker
            .check_against(self.idx, &[it], &[])
            .violations
            .is_empty()
    }

    /// A straight same-net track from inside this pad to inside pad `pb` at width `w`: the
    /// centre line first, else the offset run nearest to it (see [`nearest_run_mid`]), each
    /// with the ends at the centres or pulled in toward each other (`t` steps, so the round
    /// ends of a wide track stay off the neighbours). `O(3 · OFFSET_SAMPLES_CAP · q)`.
    fn bridge(&mut self, pb: usize, w: i64) -> Option<ExitStub> {
        if self.centre_blocked() {
            return None;
        }
        let pa = &self.model.pads[self.pi];
        let b = &self.model.pads[pb];
        let (ca, cb) = (pa.center.to_vec2(), b.center.to_vec2());
        let v = (cb - ca).unit()?;
        let n = v.perp();
        let ska = self.model.skeleton(pa, self.need.layer);
        let skb = self.model.skeleton(b, self.need.layer);
        let half = support(&ska, ca, n)
            .min(support(&ska, ca, -n))
            .min(support(&skb, cb, n))
            .min(support(&skb, cb, -n));
        let deep = |sk: &Skeleton, p: Vec2| {
            dist_prim_skeleton(&Prim::Seg(Seg { a: p, b: p }), sk) <= -START_DEPTH_NM
        };
        let tmax = (support(&ska, ca, v).min(support(&skb, cb, -v)) - START_DEPTH_NM)
            .min(0.5 * (cb - ca).len() - START_DEPTH_NM)
            .max(0.0);
        let try_at = |me: &mut Self, t: f64, o: f64| -> Option<ExitStub> {
            let (s, e) = (snap(ca + v * t + n * o), snap(cb - v * t + n * o));
            if !deep(&ska, s) || !deep(&skb, e) || me.checks >= STUB_CHECK_CAP {
                return None;
            }
            me.clean(s, e, w).then(|| ExitStub {
                pad: me.pi,
                label: format!("{}.{}", pa.ref_, pa.number),
                net: me.need.net,
                layer: me.need.layer,
                start: s,
                end: e,
                width: w,
                net_width: w,
                offset: o,
                bridge: Some(pb),
            })
        };
        let ts = [0.0, 0.5 * tmax, tmax];
        for t in ts {
            if let Some(s) = try_at(self, t, 0.0) {
                return Some(s);
            }
        }
        let range = half - START_DEPTH_NM;
        if range <= 0.0 {
            return None;
        }
        let step = OFFSET_STEP_NM.max(2.0 * range / OFFSET_SAMPLES_CAP as f64);
        let k = (range / step).floor() as i64;
        let offs: Vec<f64> = (-k..=k).map(|i| i as f64 * step).collect();
        for t in ts {
            let ok: Vec<bool> = offs.iter().map(|&o| try_at(self, t, o).is_some()).collect();
            if let Some(o) = nearest_run_mid(&offs, &ok) {
                return try_at(self, t, o);
            }
        }
        None
    }

    fn run(&mut self, mode: StubMode) -> Outcome {
        let (c, mut dirs) = self.dirs();
        let w = self.need.width as f64;
        let clr = self.need.clr as f64;
        let long = dirs[0].h > dirs[2].h + 1_000.0;
        if self.centre_blocked() {
            // The pad centre lies in a foreign clearance zone (overlapping pads, a board
            // error, a pad on the board edge): M3 terminal, the "inside obstacle" failure class.
            return Outcome::Infeasible;
        }
        let t0 = self.t0_clean();
        let wanted = match mode {
            StubMode::Off => false,
            StubMode::Auto => !t0,
            StubMode::All => !t0 || long,
        };
        if !wanted {
            return Outcome::NotNeeded;
        }
        // Direction order: long axis first, then the free room at the end point (clamped at
        // `clr + w`: enough room ties), then toward the connections' other ends.
        let mean: Vec2 = self
            .need
            .targets
            .iter()
            .filter_map(|&t| (t - c).unit())
            .fold(Vec2::default(), |a, v| a + v);
        let room = clr + w;
        let mut keyed: Vec<(i64, i64, i64, usize)> = dirs
            .iter()
            .enumerate()
            .map(|(i, dr)| {
                let (_, e) = self.ends(c, dr, 0.0, 0.0, w, clr);
                let free = self.free_at(e, room).min(room);
                let rank = i64::from(long && i >= 2);
                (
                    rank,
                    -(free.round() as i64),
                    -((dr.d.dot(mean) * 1e6).round() as i64),
                    i,
                )
            })
            .collect();
        keyed.sort_unstable();
        dirs = keyed.iter().map(|k| dirs[k.3]).collect();
        let min_w = self.min_width().min(self.need.width);
        let neck = self.opts.neck_to_pad && min_w < self.need.width;
        let neck_max = if self.opts.neck_max_nm > 0 {
            self.opts.neck_max_nm as f64
        } else {
            2.0 * 2.0 * dirs.iter().map(|d| d.h).fold(0.0, f64::max)
        };
        let starts = |dr: &Dir| -> [f64; 3] {
            let l = (dr.h - START_DEPTH_NM).max(0.0);
            [0.0, 0.5 * l, l]
        };
        // A1: centred, full width.
        for dr in &dirs {
            for t in starts(dr) {
                if let Some(s) = self.at(c, dr, t, 0.0, self.need.width, clr) {
                    return Outcome::Stub(s);
                }
            }
        }
        // B1: off-centre, full width (the net width is kept where it fits).
        for dr in &dirs {
            for t in starts(dr) {
                if let Some(o) = self.offset_scan(c, dr, t, self.need.width, clr) {
                    if let Some(s) = self.at(c, dr, t, o, self.need.width, clr) {
                        return Outcome::Stub(s);
                    }
                }
            }
        }
        // A2: centred, necked to the widest clean width.
        if neck {
            if let Some(s) =
                self.best_neck(c, &dirs, &starts, |_, _, _| Some(0.0), min_w, clr, neck_max)
            {
                return Outcome::Stub(s);
            }
        }
        // B2: off-centre, necked.
        if neck {
            let scan = |me: &mut Self, dr: &Dir, t: f64| me.offset_scan(c, dr, t, min_w, clr);
            if let Some(s) = self.best_neck(c, &dirs, &starts, scan, min_w, clr, neck_max) {
                return Outcome::Stub(s);
            }
        }
        Outcome::Infeasible
    }

    /// The widest clean neck over every direction / start (first on ties); `offset` picks the
    /// lateral offset per (direction, start).
    #[allow(clippy::too_many_arguments)]
    fn best_neck(
        &mut self,
        c: Vec2,
        dirs: &[Dir],
        starts: &dyn Fn(&Dir) -> [f64; 3],
        mut offset: impl FnMut(&mut Self, &Dir, f64) -> Option<f64>,
        min_w: i64,
        clr: f64,
        neck_max: f64,
    ) -> Option<ExitStub> {
        let mut best: Option<ExitStub> = None;
        for dr in dirs {
            for t in starts(dr) {
                let Some(o) = offset(self, dr, t) else {
                    continue;
                };
                let Some(s0) = self.at(c, dr, t, o, min_w, clr) else {
                    continue;
                };
                if s0.len() > neck_max {
                    continue;
                }
                // Bisection on the width: `lo` clean, `hi` not (full width failed in A1 / B1).
                let (mut lo, mut hi) = (min_w as f64, self.need.width as f64);
                for _ in 0..WIDTH_BISECT_STEPS {
                    if hi - lo <= 1_000.0 || self.checks >= STUB_CHECK_CAP {
                        break;
                    }
                    let mid = 0.5 * (lo + hi);
                    if self.at(c, dr, t, o, mid.round() as i64, clr).is_some() {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                // Whole µm, never below the minimum (both are clean: narrower than `lo`).
                let wn = (((lo / 1_000.0).floor() * 1_000.0) as i64).max(min_w);
                let s = self.at(c, dr, t, o, wn, clr).unwrap_or(s0);
                if best.as_ref().is_none_or(|b| s.width > b.width) {
                    best = Some(s);
                }
            }
        }
        best
    }

    /// Stub start / end for direction `dr`, axial start `t`, lateral offset `o`; the end clears
    /// the pad by `clr + w/2 + ε` (`w` the net width). Integer nm.
    fn ends(&self, c: Vec2, dr: &Dir, t: f64, o: f64, w: f64, clr: f64) -> (Vec2, Vec2) {
        let s = c + dr.d * t + dr.n * o;
        let e = c + dr.d * (dr.h + clr + 0.5 * w + self.eps) + dr.n * o;
        (snap(s), snap(e))
    }

    /// The stub `(dr, t, o)` at width `wn` when its start lies inside the pad and it is clean.
    fn at(&mut self, c: Vec2, dr: &Dir, t: f64, o: f64, wn: i64, clr: f64) -> Option<ExitStub> {
        let w = self.need.width as f64;
        let (s, e) = self.ends(c, dr, t, o, w, clr);
        // the route starts at `e`: in free space, not on another pad (same-net pads included)
        if !self.inside(s) || self.free_at(e, 1.0) < 0.0 || !self.clean(s, e, wn) {
            return None;
        }
        let p = &self.model.pads[self.pi];
        Some(ExitStub {
            pad: self.pi,
            label: format!("{}.{}", p.ref_, p.number),
            net: self.need.net,
            layer: self.need.layer,
            start: s,
            end: e,
            width: wn,
            net_width: self.need.width,
            offset: o,
            bridge: None,
        })
    }

    /// Offset of the feasible run nearest to the centre line (its middle sample), at width `wn`.
    fn offset_scan(&mut self, c: Vec2, dr: &Dir, t: f64, wn: i64, clr: f64) -> Option<f64> {
        let range = dr.half - START_DEPTH_NM;
        if range <= 0.0 {
            return None;
        }
        let step = OFFSET_STEP_NM.max(2.0 * range / OFFSET_SAMPLES_CAP as f64);
        let k = (range / step).floor() as i64;
        let offs: Vec<f64> = (-k..=k).map(|i| i as f64 * step).collect();
        let w = self.need.width as f64;
        let ok: Vec<bool> = offs
            .iter()
            .map(|&o| {
                let (s, e) = self.ends(c, dr, t, o, w, clr);
                self.checks < STUB_CHECK_CAP && self.inside(s) && self.clean(s, e, wn)
            })
            .collect();
        nearest_run_mid(&offs, &ok)
    }

    /// `p` lies inside the pad copper by at least [`START_DEPTH_NM`]. O(points).
    fn inside(&self, p: Vec2) -> bool {
        let pad = &self.model.pads[self.pi];
        let sk = self.model.skeleton(pad, self.need.layer);
        dist_prim_skeleton(&Prim::Seg(Seg { a: p, b: p }), &sk) <= -START_DEPTH_NM
    }

    /// The stub `s → e` at width `wn` plus the full-width disc at `e` violate nothing (board at
    /// margin ε and the stubs accepted so far). One [`Checker`] call, O(q).
    fn clean(&mut self, s: Vec2, e: Vec2, wn: i64) -> bool {
        self.checks += 1;
        let stub = ExitStub {
            pad: self.pi,
            label: String::new(),
            net: self.need.net,
            layer: self.need.layer,
            start: s,
            end: e,
            width: wn,
            net_width: self.need.width,
            offset: 0.0,
            bridge: None,
        };
        let mut items = vec![stub.item(None)];
        if wn < self.need.width {
            items.push(end_disc(&stub));
        }
        self.checker
            .check_against(self.idx, &items, &[])
            .violations
            .is_empty()
    }

    /// Effective minimum track width of the net: KiCad's minimum and the net's custom-rule
    /// minimum (`NetRule.min_width`). O(1).
    fn min_width(&self) -> i64 {
        let r = &self.model.rules;
        r.min_track_width
            .max(r.net_rule(self.need.net).min_width)
            .max(1)
    }

    /// Copper gap of a full-width disc at `e` to the nearest other pad (any net), capped at
    /// `cap`. O(q).
    fn free_at(&self, e: Vec2, cap: f64) -> f64 {
        let w = self.need.width as f64;
        let mut free = cap;
        for j in self.grid.near(e, cap + w) {
            if j == self.pi {
                continue;
            }
            let p = &self.model.pads[j];
            let Some(&l) = p.layers.iter().find(|&&l| l == self.need.layer) else {
                continue;
            };
            let sk = self.model.skeleton(p, l);
            let g = dist_prim_skeleton(&Prim::Seg(Seg { a: e, b: e }), &sk) - 0.5 * w;
            free = free.min(g);
        }
        free
    }
}

/// `model` plus every planned stub as kept copper of its net (obstacle, CDT constraint, Checker
/// item), so the routing pass sees it as fixed. O(model + stubs).
pub fn with_stubs(model: &BoardModel, plan: &StubPlan) -> BoardModel {
    let mut m = model.clone();
    for (i, s) in plan.stubs.iter().enumerate() {
        m.kept.push(KeptItem {
            uuid: format!("topor-stub:{i}:{}", s.label),
            net: s.net,
            layer: s.layer,
            width: s.width,
            prim: s.prim(),
            locked: false,
        });
    }
    m
}

/// Points every pad terminal of `conns` that has a stub at the stub end
/// ([`TermKind::Stub`]); returns the stub index per connection end `[from, to]`. O(m log T).
pub fn attach(conns: &mut [Connection], plan: &StubPlan) -> Vec<[Option<usize>; 2]> {
    conns
        .iter_mut()
        .map(|c| {
            let mut out = [None, None];
            for (k, t) in [&mut c.from, &mut c.to].into_iter().enumerate() {
                let TermKind::Pad(pi) = t.kind else { continue };
                let Some(&si) = plan.by_pad.get(&pi) else {
                    continue;
                };
                let s = &plan.stubs[si];
                if !t.layers.contains(&s.layer) {
                    continue;
                }
                t.kind = TermKind::Stub {
                    pad: pi,
                    start: s.start,
                };
                t.at = s.end;
                t.layers = vec![s.layer];
                out[k] = Some(si);
            }
            out
        })
        .collect()
}

/// Marks the duplicate exit-stub items of `items` (`true` = an identical stub item came
/// earlier): connections sharing a stubbed pad each carry its stub, so it survives while any of
/// them stays routed; the board gets it once. Returns per duplicate its stub row. O(I log S).
pub fn stub_duplicates<'a>(items: &[Item], stubs: &'a [StubOut]) -> Vec<Option<&'a StubOut>> {
    type Key = (i32, u32, i64, [i64; 2], [i64; 2]);
    let mut out = vec![None; items.len()];
    if stubs.is_empty() {
        return out;
    }
    let keys: BTreeMap<Key, &StubOut> = stubs
        .iter()
        .map(|s| ((s.net, s.layer, s.width, s.start, s.end), s))
        .collect();
    let mut seen: std::collections::BTreeSet<Key> = std::collections::BTreeSet::new();
    for (i, it) in items.iter().enumerate() {
        let Item::Track(t) = it else { continue };
        let TrackPrim::Seg { a, b } = t.prim else {
            continue;
        };
        let k: Key = (
            t.net,
            t.layer,
            t.width,
            [i64::from(a.x), i64::from(a.y)],
            [i64::from(b.x), i64::from(b.y)],
        );
        if let Some(s) = keys.get(&k) {
            if !seen.insert(k) {
                out[i] = Some(*s);
            }
        }
    }
    out
}
