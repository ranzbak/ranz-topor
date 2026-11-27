//! TW2 METRIC: the joint band commit of the ERBS drawing (`[drawing] realise = "erbs"`).
//!
//! All runs' extended rubber bands come from the plan at once (`erbs::realise`). Drawing them one
//! after the other let a rejected band's sequential copper take room that later bands owned, so
//! they were rejected too (the cascade). Here every band is committed at once and checked once:
//!
//! 1. Every run with two fixed ends (pad end or placed via; a branch end on its trunk and a wire
//!    with a moved junction are left to the drawing) gets its band items. A band that breaks a
//!    rule against a static item (pad, via, hole, outline, existing copper) is rejected (class
//!    *static*).
//! 2. The others are committed together. Each is checked once against the index. A violation
//!    against another band is a conflict edge.
//! 3. Per conflict component a maximum independent set is kept ([`super::erbs::keep_bands`]):
//!    `[drawing] erbs_conflict_keep` = tier (priority tier first, then count) or count, ties by the
//!    route-order rank. The others are taken out: class *gap* when the ERBS check found a model gap
//!    on the band (`RunBand::gap`), else *predicted*.
//! 4. Kept bands stay committed (reserved). `draw_wire` adopts them at attempt -1, so their copper
//!    is never checked against a fallback's copper. Every other run goes through the ladder and the
//!    never-drop pass against an index that already holds the kept bands.
//!
//! So nothing depends on the draw order: with no conflict every band is kept (Dayan 1997, PDF
//! p. 26: the ERBS paths satisfy the spacing iff the class is routable; PDF p. 17: simultaneous
//! routing gives no net a higher priority). The tier / count / rank rule is our own (the smallest
//! rule that keeps "order-free" exact without conflicts and bounded with them).
//!
//! Invariant I1 (no cascade): a reserved band leaves the copper only through its own wire
//! ([`JointState::lost_to_other`] counts the others; 0 by construction).

use std::collections::{BTreeMap, BTreeSet};

use super::draw::{Drawer, Plan, WireOut};
use super::erbs::{keep_bands, KeepMode, KEEP_EXACT_CAP};
use super::state::WireId;
use crate::geom::Vec2;
use crate::model::Item;
use crate::route::ErbsRunReport;
use crate::vias::ViaSize;

/// Why a run with an ERBS mode was not drawn from its band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutClass {
    /// The band breaks a rule against a static item.
    Static,
    /// The band lost a conflict with another band, no model gap on it (an over-full cut, a planned crossing).
    Predicted,
    /// The band lost a conflict with another band and the ERBS check found a model gap on it (`erbs_gap`).
    Gap,
    /// No band for the run (no sleeve, the time limit, a branch end on its trunk, a moved junction).
    NoBand,
    /// A band that was not reserved (a branch end, a moved junction) and failed its check in the drawing.
    Junction,
    /// A kept band whose own wire was drawn again (its failure, the never-drop pass, the feedback) and
    /// no longer fit.
    Redrawn,
}

impl CutClass {
    /// Its snake-case name (draw-absorb A0 provenance, result.json `violations[].prov.cut`).
    pub fn name(self) -> &'static str {
        match self {
            CutClass::Static => "static",
            CutClass::Predicted => "predicted",
            CutClass::Gap => "gap",
            CutClass::NoBand => "no_band",
            CutClass::Junction => "junction",
            CutClass::Redrawn => "redrawn",
        }
    }
}

/// How a drawn run came about (one per [`WireOut::runs`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RunSrc {
    /// `realise = "sequential"`.
    #[default]
    Seq,
    /// From its ERBS (legal). `octi`: in its octilinear form. `octi_vs_band`: its octilinear form
    /// broke a rule against another band, so the any-angle band stayed (the D11 transform's share).
    Erbs { octi: bool, octi_vs_band: bool },
    /// A cheaper sequential run replaced the legal ERBS (length factor).
    Replaced,
    /// Drawn by the sequential attempts (legal), for the reason `class`.
    Cut(CutClass),
    /// Drawn by never drop with violations; `erbs`: on its own band.
    Forced { erbs: bool, class: CutClass },
}

/// A band reserved by the joint commit: its points, items, their handles and the plan neck it took.
#[derive(Clone, Debug)]
pub struct Reserved {
    pub pts: Vec<Vec2>,
    pub items: Vec<Item>,
    pub handles: Vec<usize>,
    pub pneck: Option<(f64, [f64; super::exact::NECK_ZONES])>,
    /// In the band's octilinear form (the any-angle band missed a clearance its octilinear form keeps).
    pub octi: bool,
    /// draw-absorb A1 (`[drawing] forced_shrink`): the neck zones of `pneck` when the joint commit
    /// necked the band against a gap partner (the plan's zones and the gap's); `None` = the plan's.
    pub zones: Option<[Option<(Vec2, f64)>; super::exact::NECK_ZONES]>,
}

/// The deepest overlap of a band pair's clearance (nm), where (KiCad frame, nm) and the layer id.
type Overlap = (i64, (i64, i64), u32);

/// Two bands kept together across a model gap (`[drawing] erbs_gap_keep_max_mm`): the band that
/// yields by key (`wire`, `run`), the other wire, the deepest overlap of the clearance (nm), where
/// (KiCad frame, nm) and the layer id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GapKept {
    pub wire: WireId,
    pub run: usize,
    pub other: WireId,
    pub overlap_nm: i64,
    pub at: (i64, i64),
    pub layer: u32,
}

/// The joint commit's state in the [`Drawer`].
#[derive(Clone, Debug, Default)]
pub struct JointState {
    /// The commit ran (`realise = "erbs"` with bands).
    pub on: bool,
    /// Kept bands not adopted yet, per (wire, run).
    pub reserved: BTreeMap<(WireId, usize), Reserved>,
    /// Bands not kept, with the reason.
    pub class: BTreeMap<(WireId, usize), CutClass>,
    /// Every band kept by the commit (adopted or not).
    pub kept: BTreeSet<(WireId, usize)>,
    /// Handle of a reserved band item -> its wire (until adopted).
    pub owner: BTreeMap<usize, WireId>,
    /// Index id of a band item (reserved or adopted) -> (wire, handle).
    pub band_ids: BTreeMap<usize, (WireId, usize)>,
    /// The wire whose drawing runs now ([`Drawer::draw_wire`]); `None` outside.
    pub cur: Option<WireId>,
    /// Reserved band items taken out by anything but their own wire (I1: always 0).
    pub lost_to_other: usize,
    /// Reserved bands no wire adopted or released by the end of the drawing loop (invariant 0).
    pub left_reserved: usize,
    /// Runs drawn with no ERBS stretch at their ends: (wire, run) -> (layer, why, end moved nm).
    pub no_band: BTreeMap<(WireId, usize), (u32, &'static str, i64)>,
    pub bands: usize,
    pub n_static: usize,
    pub n_predicted: usize,
    pub n_gap: usize,
    pub comp_exact: usize,
    pub comp_greedy: usize,
    /// Bands that lost a conflict (or broke a static rule) any-angle and are kept in their octilinear form.
    pub octi_rescued: usize,
    /// Conflicts kept across a model gap (both bands kept).
    pub gap_kept: Vec<GapKept>,
    /// Both runs of every `gap_kept` pair: the one drawn second records the violation.
    pub gap_runs: BTreeSet<(WireId, usize)>,
    /// draw-absorb A1 (`[drawing] forced_shrink`): model-gap pairs resolved by the plan's neck.
    pub gap_shrunk: usize,
    /// draw-absorb A1b meter: reserved bands the plan had necked that the drawing redrew at full
    /// width (no `[drawing] reserved_keep_neck`): (wire, run, necked length dropped nm, length of
    /// the full-width copper in the plan's zones that takes planned room of undrawn wires nm
    /// ([`Drawer::room_taken`]), where: "tip" = the pad-tip re-anchor, "octi" = the adopted
    /// band's octilinear form, "octi_wider" = that form necked, but wider than reserved).
    pub unnecked: Vec<(WireId, usize, f64, f64, &'static str)>,
    /// With `reserved_keep_neck`: the redraws that kept the band's neck (re-anchor, octilinear form).
    pub kept_neck: usize,
    /// draw-absorb A3 (`[drawing] band_passes`): the iterated band realise's counts.
    pub band_pass: super::erbs::PassStats,
    /// draw-absorb A1 r2: the wires A1 necked (a joint-commit band or a forced run), for the
    /// supply-width neck warning.
    pub a1_wires: BTreeSet<WireId>,
    /// draw-absorb A1 r2 ([`Drawer::a1_finish`]): A1 runs whose necks were joined into one
    /// stretch touching a run end (never wider again after a neck), and the length narrowed for it
    /// (nm); pad-end stubs moved out of KiCad's in-pad delete.
    pub a1_mono: usize,
    pub a1_mono_nm: f64,
    pub a1_stubs: usize,
}

/// draw-absorb A1 r2: what [`Drawer::a1_finish`] changed in a run's necked copper.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct A1Fix {
    /// Length (nm) narrowed to join the necks into one stretch (0: already one).
    pub mono_nm: f64,
    /// Pad-end stubs moved so KiCad's Cleanup keeps them.
    pub stubs: usize,
}

impl JointState {
    /// Bookkeeping when `handle` (index id `i`) leaves the index. O(log n).
    pub fn on_uncommit(&mut self, handle: usize, i: usize) {
        if self.band_ids.get(&i).is_some_and(|x| x.1 == handle) {
            self.band_ids.remove(&i);
        }
        if let Some(o) = self.owner.remove(&handle) {
            if self.cur != Some(o) {
                self.lost_to_other += 1;
            }
        }
    }
}

impl<'a> Drawer<'a> {
    /// The joint band commit (module doc). `order`: the wires to draw; `plans`, `placed`: their
    /// plans and placed vias; `skip`: (wire, run) left to the drawing (a branch end on its trunk);
    /// `key`: the yield key (priority tier, route-order rank) of a wire. Splits are skipped
    /// ([`Drawer::split_at`]). O(bands x checker query + conflict components).
    pub fn reserve_bands(&mut self, order: &[WireId], plans: &BTreeMap<WireId, Plan>, placed: &BTreeMap<WireId, Vec<(usize, Vec2, ViaSize)>>, skip: &BTreeSet<(WireId, usize)>, key: &dyn Fn(WireId) -> (u8, usize), mode: KeepMode) {
        self.joint.on = true;
        struct Band {
            w: WireId,
            r: usize,
            pts: Vec<Vec2>,
            items: Vec<Item>,
            pneck: Option<(f64, [f64; super::exact::NECK_ZONES])>,
            zones: Option<[Option<(Vec2, f64)>; super::exact::NECK_ZONES]>,
            gap: bool,
            octi: bool,
            n: usize,
        }
        // par-draw phase A: every run's band is an order-free unit (its realise and octilinear
        // form see only the vias and the static copper; nothing is committed until all are
        // formed), so the units run on lanes ([`super::par::map`]) and the bands are taken in
        // order (wire order, then run).
        struct Job<'a> {
            w: WireId,
            r: usize,
            n: usize,
            layer: usize,
            pts: Vec<Vec2>,
            gap: bool,
            vs: &'a [(usize, Vec2, ViaSize)],
            pneck: std::sync::Arc<Option<super::exact::Neck>>,
        }
        let mut jobs: Vec<Job> = Vec::new();
        for &w in order {
            let (Some(pl), Some(vs)) = (plans.get(&w), placed.get(&w)) else { continue };
            if self.split_at.get(&w).is_some_and(|v| !v.is_empty()) {
                continue;
            }
            let n = pl.layers.len();
            if n == 0 || vs.len() + 1 < n {
                continue;
            }
            let pneck = std::sync::Arc::new(self.plan_neck(w));
            for r in 0..n {
                if skip.contains(&(w, r)) {
                    continue;
                }
                // The run's ends as `draw_wire` takes them.
                let s = if r == 0 { self.run_end(w, 0) } else { vs[r - 1].1 };
                let e = if r == n - 1 { self.run_end(w, 1) } else { vs[r].1 };
                let layer = pl.layers[r];
                let Some(rb) = self.erbs.get(&w).and_then(|v| v.iter().find(|b| b.layer == layer && (b.s - s).len() < 1000.0 && (b.e - e).len() < 1000.0)) else { continue };
                if rb.path.len() < 2 {
                    continue;
                }
                let gap = rb.gap;
                let mut pts = rb.path.clone();
                pts[0] = s;
                let m = pts.len();
                pts[m - 1] = e;
                self.joint.bands += 1;
                jobs.push(Job { w, r, n, layer, pts, gap, vs: vs.as_slice(), pneck: pneck.clone() });
            }
        }
        type Formed = Option<(bool, Option<Vec<Vec2>>, Vec<Item>, Option<(f64, [f64; super::exact::NECK_ZONES])>)>;
        let this: &Self = self;
        let formed: Vec<Formed> = super::par::map(&jobs, |_, j| {
            // Static check: only vias and static copper are in the index now.
            let lid = this.layer_id(j.layer);
            match this.realise(j.w, j.layer, lid, &j.pts, &j.pneck) {
                Some((items, pn)) if !items.is_empty() => Some((false, None, items, pn)),
                // As attempt -1 does: the band's octilinear form where the any-angle one misses.
                _ => this.band_octi(j.w, j.r, j.n, j.vs, j.layer, &j.pts, &j.pneck).map(|(o, items, pn)| (true, Some(o), items, pn)),
            }
        });
        let mut bands: Vec<Band> = Vec::with_capacity(jobs.len());
        for (j, f) in jobs.into_iter().zip(formed) {
            match f {
                Some((octi, o, items, pn)) => bands.push(Band { w: j.w, r: j.r, pts: o.unwrap_or(j.pts), items, pneck: pn, zones: None, gap: j.gap, octi, n: j.n }),
                None => {
                    self.joint.class.insert((j.w, j.r), CutClass::Static);
                    self.joint.n_static += 1;
                }
            }
        }
        // All at once.
        let mut handles: Vec<Vec<usize>> = Vec::with_capacity(bands.len());
        let mut of_id: BTreeMap<usize, usize> = BTreeMap::new();
        for (bi, b) in bands.iter().enumerate() {
            let hs: Vec<usize> = b.items.iter().map(|it| self.commit(it)).collect();
            for &h in &hs {
                let id = self.handles[h].0;
                of_id.insert(id, bi);
                self.joint.owner.insert(h, b.w);
                self.joint.band_ids.insert(id, (b.w, h));
            }
            handles.push(hs);
        }
        // Each once: conflict edges between bands (a violation against anything else is static),
        // with the deepest overlap of the clearance (nm) and where it is (KiCad frame).
        let mut deep: BTreeMap<(usize, usize), Overlap> = BTreeMap::new();
        let mut stat = vec![false; bands.len()];
        // par-draw phase A: each band against the fixed index (all bands committed), on lanes.
        let this: &Self = self;
        let reps: Vec<_> = super::par::map(&bands, |_, b| this.checker.check_against(&this.idx, &b.items, &[]));
        for ((bi, b), rep) in bands.iter().enumerate().zip(reps) {
            for v in &rep.violations {
                match v.against.strip_prefix("item:").and_then(|t| t.parse::<usize>().ok()).and_then(|id| of_id.get(&id)) {
                    Some(&bj) if bj != bi => {
                        let at = match b.items.get(v.item) {
                            Some(Item::Track(t)) => {
                                let (s, e) = (t.prim.start(), t.prim.end());
                                ((s.x as i64 + e.x as i64) / 2, (s.y as i64 + e.y as i64) / 2)
                            }
                            _ => (0, 0),
                        };
                        let d = v.required - v.gap;
                        let x = deep.entry((bi.min(bj), bi.max(bj))).or_insert((d, at, v.layer));
                        if d > x.0 {
                            *x = (d, at, v.layer);
                        }
                    }
                    Some(_) => {}
                    None => stat[bi] = true,
                }
            }
        }
        // A band that breaks a static rule only together with the others (cannot happen: the
        // static check saw the same static items) is rejected as static; it takes part in no conflict.
        deep.retain(|&(a, b), _| !stat[a] && !stat[b]);
        if self.test_joint_conflict {
            // test-only fault: every two bands next in the order conflict (an overlap far past
            // any gap bound), so the selector rejects bands whose copper is in fact legal.
            for a in 0..bands.len().saturating_sub(1) {
                if !stat[a] && !stat[a + 1] {
                    deep.entry((a, a + 1)).or_insert((i64::MAX / 4, (0, 0), 0));
                }
            }
        }
        // A model gap of the ERBS (`RunBand::gap`: no over-full cut predicts it; Dayan 1997 PDF
        // p. 26: bands of a routable class keep the spacing) up to `[drawing]
        // erbs_gap_keep_max_mm` keeps both bands; the gap stays as a counted violation of the band
        // drawn second. A deeper one, or a predicted conflict, goes to the selector.
        let gap_max = (self.opts.erbs_gap_keep_max_mm.max(0.0) * 1e6).round() as i64;
        // draw-absorb A1 (`[drawing] forced_shrink`): before a model-gap pair is kept with its
        // gap as a violation, a band the plan necks is realised again with that neck against the
        // other band (the first realise saw only the static copper, which the full width clears;
        // TopoR 6.3 manual p. 56: the minimum width in the bottleneck). The band that would yield
        // by key first, then the other. The neck is taken at the widest width at which the band
        // breaks no rule against its gap partners and hits nothing its full width did not
        // ([`Drawer::neck_band`]); its pairs are then measured again from both sides. A band
        // whose neck does not clear keeps its copper. Only ever narrower than the reserved band.
        if self.opts.forced_shrink {
            let dbg = std::env::var_os("TOPO_ABSORBDBG").is_some();
            let gapb: Vec<bool> = bands.iter().map(|b| b.gap).collect();
            let is_gap = |deep: &BTreeMap<(usize, usize), Overlap>, a: usize, c: usize| deep.get(&(a.min(c), a.max(c))).is_some_and(|&(d, _, _)| (gapb[a] || gapb[c]) && d <= gap_max);
            let pairs: Vec<(usize, usize)> = deep.keys().filter(|&&(a, c)| is_gap(&deep, a, c)).copied().collect();
            for (a, c) in pairs {
                if !deep.contains_key(&(a, c)) {
                    continue;
                }
                let (ka, kc) = (key(bands[a].w), key(bands[c].w));
                let (yi, oi) = if (ka, bands[a].w, bands[a].r) > (kc, bands[c].w, bands[c].r) { (a, c) } else { (c, a) };
                for x in [yi, oi] {
                    let w = bands[x].w;
                    let pn = self.plan_neck(w);
                    let partners: BTreeSet<usize> = deep.keys().filter(|&&(p, q)| (p == x || q == x) && is_gap(&deep, p, q)).map(|&(p, q)| if p == x { q } else { p }).collect();
                    if dbg {
                        let net = |b: usize| self.st.wires[bands[b].w].net;
                        eprintln!("ABSORBDBG pair (w{} net {}, w{} net {}) depth {:?} x w{w} pneck {} partners {:?}", bands[a].w, net(a), bands[c].w, net(c), deep.get(&(a, c)).map(|d| d.0), pn.as_ref().map_or("none".to_string(), |n| format!("min {:.0} zones {:?}", n.min_w, n.ends.iter().flatten().map(|z| ((z.0.x / 1e3).round() / 1e3, (-z.0.y / 1e3).round() / 1e3, (z.1 / 1e3).round() / 1e3)).collect::<Vec<_>>())), partners.iter().map(|&q| (bands[q].w, net(q))).collect::<Vec<_>>());
                    }
                    let layer = self.layer_of_band(&bands[x].items);
                    let lid = self.layer_id(layer);
                    // What the reserved band hits now (labels; its own items are its net's).
                    let rep0 = self.checker.check_against(&self.idx, &bands[x].items, &[]);
                    let old: BTreeSet<String> = rep0.violations.iter().map(|v| v.against.clone()).collect();
                    // The neck zones: the plan's, and one round each stretch where the band breaks
                    // the rule against a gap partner (the model gap: no over-full cut of the plan
                    // predicts it, so the plan's zones need not reach it; A1, user-approved: neck
                    // to the net class minimum outside the pad exit), the deepest first.
                    let wi = &self.st.wires[w];
                    let mut nk = pn.unwrap_or(super::exact::Neck { min_w: self.st.w_min(wi.net, wi.w), ends: [None; super::exact::NECK_ZONES] });
                    if nk.min_w >= wi.w - 0.5 {
                        continue;
                    }
                    let free = nk.ends.iter().filter(|z| z.is_none()).count();
                    let partner_hit = |v: &crate::check::Violation| v.against.strip_prefix("item:").and_then(|t| t.parse::<usize>().ok()).and_then(|id| of_id.get(&id)).is_some_and(|b| partners.contains(b));
                    let spots = self.hit_zones(w, &bands[x].items, &rep0, &partner_hit, free);
                    let slots: Vec<usize> = (0..super::exact::NECK_ZONES).filter(|&i| nk.ends[i].is_none()).collect();
                    for (i, z) in slots.into_iter().zip(&spots) {
                        nk.ends[i] = Some(*z);
                    }
                    if dbg {
                        eprintln!("ABSORBDBG   spots {:?} pts {:?}", spots.iter().map(|z| ((z.0.x / 1e3).round() / 1e3, (z.0.y / 1e3).round() / 1e3, (z.1 / 1e3).round() / 1e3)).collect::<Vec<_>>(), bands[x].pts.iter().map(|p| ((p.x / 1e3).round() / 1e3, (p.y / 1e3).round() / 1e3)).collect::<Vec<_>>());
                    }
                    if nk.ends.iter().all(|z| z.is_none()) {
                        continue;
                    }
                    let pn = nk;
                    let cur = self.joint.cur.replace(w);
                    for h in std::mem::take(&mut handles[x]) {
                        of_id.remove(&self.handles[h].0);
                        self.uncommit(h);
                    }
                    self.joint.cur = cur;
                    let partner_of = |lbl: &str| lbl.strip_prefix("item:").and_then(|t| t.parse::<usize>().ok()).and_then(|id| of_id.get(&id)).is_some_and(|b| partners.contains(b));
                    let got = self.neck_band(w, lid, &bands[x].pts, &pn, &|lbl: &str| !partner_of(lbl) && old.contains(lbl), dbg);
                    let ok = got.is_some();
                    if let Some((its, pnr, z, fix)) = got {
                        bands[x].items = its;
                        bands[x].pneck = Some(pnr);
                        bands[x].zones = Some(z.ends);
                        self.joint.a1_wires.insert(w);
                        self.joint.a1_mono += usize::from(fix.mono_nm > 0.0);
                        self.joint.a1_mono_nm += fix.mono_nm;
                        self.joint.a1_stubs += fix.stubs;
                    }
                    let hs: Vec<usize> = bands[x].items.iter().map(|it| self.commit(it)).collect();
                    for &h in &hs {
                        let id = self.handles[h].0;
                        of_id.insert(id, x);
                        self.joint.owner.insert(h, w);
                        self.joint.band_ids.insert(id, (w, h));
                    }
                    handles[x] = hs;
                    if ok {
                        // Every pair of x measured again, from both sides (another net's rule can
                        // be wider).
                        let others: BTreeSet<usize> = deep.keys().filter(|&&(p, q)| p == x || q == x).map(|&(p, q)| if p == x { q } else { p }).collect();
                        deep.retain(|&(p, q), _| p != x && q != x);
                        let add = |deep: &mut BTreeMap<(usize, usize), Overlap>, from: usize, items: &[Item], rep: &crate::check::CheckReport, want: &dyn Fn(usize) -> bool| {
                            for v in &rep.violations {
                                let Some(&bj) = v.against.strip_prefix("item:").and_then(|t| t.parse::<usize>().ok()).and_then(|id| of_id.get(&id)) else { continue };
                                if bj == from || !want(bj) {
                                    continue;
                                }
                                let at = match items.get(v.item) {
                                    Some(Item::Track(t)) => {
                                        let (s, e) = (t.prim.start(), t.prim.end());
                                        ((s.x as i64 + e.x as i64) / 2, (s.y as i64 + e.y as i64) / 2)
                                    }
                                    _ => (0, 0),
                                };
                                let d = v.required - v.gap;
                                let e = deep.entry((from.min(bj), from.max(bj))).or_insert((d, at, v.layer));
                                if d > e.0 {
                                    *e = (d, at, v.layer);
                                }
                            }
                        };
                        let rep = self.checker.check_against(&self.idx, &bands[x].items, &[]);
                        add(&mut deep, x, &bands[x].items, &rep, &|_| true);
                        for o in others {
                            let rep = self.checker.check_against(&self.idx, &bands[o].items, &[]);
                            add(&mut deep, o, &bands[o].items, &rep, &|b| b == x);
                        }
                    }
                    if dbg {
                        eprintln!("ABSORBDBG   x w{w}: neck {}, pair now {:?}", if ok { "taken" } else { "none" }, deep.get(&(a, c)));
                    }
                    if !deep.contains_key(&(a, c)) {
                        break;
                    }
                }
                if !deep.contains_key(&(a, c)) {
                    self.joint.gap_shrunk += 1;
                }
            }
        }
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for (&(a, c), &(d, at, layer)) in &deep {
            if (bands[a].gap || bands[c].gap) && d <= gap_max {
                let (ka, kc) = (key(bands[a].w), key(bands[c].w));
                // The band that would have yielded (the later key) reports the violation.
                let (yi, oi) = if (ka, bands[a].w, bands[a].r) > (kc, bands[c].w, bands[c].r) { (a, c) } else { (c, a) };
                self.joint.gap_runs.insert((bands[a].w, bands[a].r));
                self.joint.gap_runs.insert((bands[c].w, bands[c].r));
                self.joint.gap_kept.push(GapKept { wire: bands[yi].w, run: bands[yi].r, other: bands[oi].w, overlap_nm: d, at, layer });
            } else {
                edges.push((a, c));
            }
        }
        let keys: Vec<(u8, usize)> = bands.iter().map(|b| {
            let (t, rank) = key(b.w);
            (t, rank.saturating_mul(64).saturating_add(b.r.min(63)))
        }).collect();
        let kept = keep_bands(&keys, &edges, mode, KEEP_EXACT_CAP);
        self.joint.comp_exact += kept.exact;
        self.joint.comp_greedy += kept.greedy;
        // The bands not kept leave the index first, so the octilinear forms below see only kept copper.
        let mut losers: Vec<(Band, Vec<usize>)> = Vec::new();
        for (bi, b) in bands.into_iter().enumerate() {
            let hs = std::mem::take(&mut handles[bi]);
            if kept.keep[bi] && !stat[bi] {
                self.joint.octi_rescued += b.octi as usize;
                self.joint.kept.insert((b.w, b.r));
                self.joint.reserved.insert((b.w, b.r), Reserved { pts: b.pts, items: b.items, handles: hs, pneck: b.pneck, octi: b.octi, zones: b.zones });
                continue;
            }
            let cur = self.joint.cur.replace(b.w);
            for &h in &hs {
                self.uncommit(h);
            }
            self.joint.cur = cur;
            losers.push((b, vec![bi]));
        }
        // As attempt -1 does: a band whose any-angle form misses the clearance to a kept band is
        // kept in its octilinear form where that form is legal against everything kept so far
        // (best key first, so the result does not depend on the draw order).
        losers.sort_by_key(|(b, v)| (keys[v[0]], b.w, b.r));
        for (b, v) in losers {
            let bi = v[0];
            if !stat[bi] && !b.octi {
                let vs = &placed[&b.w];
                let pneck = self.plan_neck(b.w);
                if let Some((o, items, pn)) = self.band_octi(b.w, b.r, b.n, vs, self.layer_of_band(&b.items), &b.pts, &pneck) {
                    let hs: Vec<usize> = items.iter().map(|it| self.commit(it)).collect();
                    for &h in &hs {
                        let id = self.handles[h].0;
                        self.joint.owner.insert(h, b.w);
                        self.joint.band_ids.insert(id, (b.w, h));
                    }
                    self.joint.octi_rescued += 1;
                    self.joint.kept.insert((b.w, b.r));
                    self.joint.reserved.insert((b.w, b.r), Reserved { pts: o, items, handles: hs, pneck: pn, octi: true, zones: None });
                    continue;
                }
            }
            let class = if stat[bi] {
                self.joint.n_static += 1;
                CutClass::Static
            } else if b.gap {
                self.joint.n_gap += 1;
                CutClass::Gap
            } else {
                self.joint.n_predicted += 1;
                CutClass::Predicted
            };
            self.joint.class.insert((b.w, b.r), class);
        }
    }

    /// draw-absorb A1: band `pts` of wire `w` (layer id `lid`) with the plan's neck zones `pn`,
    /// at the widest width in `[floor, nominal]` at which every violation it has is `allowed`
    /// (not against a gap partner, and against something the full-width band hit too), each zone
    /// trimmed to what the checker needs where `neck_trim` is on (`Drawer::fit_neck`); no track
    /// grazes its own pad. `None` when even the floor leaves a violation that is not allowed.
    /// `(items, (width, necked length per zone), zones)`. O(14 checks of the band + the fit).
    #[allow(clippy::type_complexity)]
    fn neck_band(&self, w: WireId, lid: u32, pts: &[Vec2], pn: &super::exact::Neck, allowed: &dyn Fn(&str) -> bool, dbg: bool) -> Option<(Vec<Item>, (f64, [f64; super::exact::NECK_ZONES]), super::exact::Neck, A1Fix)> {
        let wi = &self.st.wires[w];
        // r2: the copper as drawn: one neck stretch per run, pad stubs kept ([`Drawer::a1_finish`]).
        let draw = |nk: &super::exact::Neck, wn: f64| self.a1_finish(w, self.neck_items(w, lid, pts, nk, wn).0);
        let ok = |nk: &super::exact::Neck, wn: f64| {
            let its = draw(nk, wn).0;
            let rep = self.checker.check_against(&self.idx, &its, &[]);
            rep.violations.iter().all(|v| allowed(&v.against))
        };
        if !ok(pn, pn.min_w) {
            if dbg {
                let its = draw(pn, pn.min_w).0;
                let rep = self.checker.check_against(&self.idx, &its, &[]);
                eprintln!("ABSORBDBG   floor {:.0} still hits {:?}", pn.min_w, rep.violations.iter().filter(|v| !allowed(&v.against)).map(|v| (v.against.clone(), v.required - v.gap, v.item, match &its[v.item] { Item::Track(t) => format!("{:?} w {}", t.prim, t.width), _ => "via".into() })).collect::<Vec<_>>());
                eprintln!("ABSORBDBG   zones {:?}", pn.ends.iter().flatten().map(|z| ((z.0.x / 1e3).round() / 1e3, (z.0.y / 1e3).round() / 1e3, (z.1 / 1e3).round() / 1e3)).collect::<Vec<_>>());
            }
            return None;
        }
        let (mut lo, mut hi) = (pn.min_w, wi.w);
        for _ in 0..12 {
            let mid = 0.5 * (lo + hi);
            if ok(pn, mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let wn = lo.floor().max(pn.min_w);
        let nk = if self.opts.neck_trim {
            let f = self.fit_neck(w, lid, pts, pn, wn);
            if ok(&f, wn) { f } else { *pn }
        } else {
            *pn
        };
        let (items, fix) = draw(&nk, wn);
        // One stretch: its whole length on the first zone (the warnings read one neck).
        let mut ls = [0.0; super::exact::NECK_ZONES];
        if let (Some((_, _, l)), Some(i)) = (neck_of(&items, wi.w), nk.ends.iter().position(Option::is_some)) {
            ls[i] = l;
        }
        (!items.is_empty() && self.track_own_pad_ok(w, &items)).then_some((items, (wn, ls), nk, fix))
    }

    /// draw-absorb A1 r2 (verifier vfy-draw-absorb-r1 F1/F2; user 2026-11-26 "never widen again
    /// after a neck", lead 2026-11-27): the necked copper `items` of one run of wire `w` (its
    /// tracks in path order, KiCad frame) as A1 draws it:
    /// 1. one necked stretch that touches a run end, or the whole run ([`one_neck_stretch`]): a
    ///    neck in the middle reaches on to the nearer run end (the pad side, neck from pad; a T
    ///    split or a via where the run ends there), necks that would make two stretches join;
    /// 2. collinear pieces of one width joined (the zone cuts), as KiCad's Cleanup would;
    /// 3. a narrowed end stub in an own-net pad that KiCad's Cleanup would delete as a track
    ///    inside the pad (its in-pad vertex was placed for the full width, e.g. the pad-tip axis
    ///    point one trace radius in) moved out of that delete ([`stub_out_of_pad`]), so the trace
    ///    still ends at the pad centre (qmtech det s0: 7 ends off centre, up to 0.83 mm).
    ///
    /// Only narrows, and moves a vertex only inside the full width's copper (a disc of radius
    /// w/2 round it): the result lies inside the envelope of the same run at full width, so it
    /// hits nothing new (the callers still check it). Debug arms (`TOPO_FD45_OFF`): `dab_mono`,
    /// `dab_stub`. O(tracks + end pads of the net).
    pub(super) fn a1_finish(&self, w: WireId, items: Vec<Item>) -> (Vec<Item>, A1Fix) {
        let wi = &self.st.wires[w];
        let mut fix = A1Fix::default();
        let mut ws = Vec::with_capacity(items.len());
        let mut ls = Vec::with_capacity(items.len());
        for it in &items {
            let Item::Track(t) = it else { return (items, fix) };
            let (a, b) = (t.prim.start(), t.prim.end());
            ws.push(t.width as f64);
            ls.push(f64::hypot(f64::from(a.x) - f64::from(b.x), f64::from(a.y) - f64::from(b.y)));
        }
        let mut items = items;
        if !super::octi::fd45_off("dab_mono") {
            if let Some(nw) = one_neck_stretch(&ws, &ls) {
                for (k, it) in items.iter_mut().enumerate() {
                    if let Item::Track(t) = it {
                        if nw[k] < ws[k] - 0.5 {
                            fix.mono_nm += ls[k];
                            t.width = nw[k].round() as i64;
                        }
                    }
                }
            }
        }
        let mut items = merge_collinear(items);
        if !super::octi::fd45_off("dab_stub") {
            let pads: Vec<usize> = (0..self.model.pads.len()).filter(|&i| self.model.pads[i].net == wi.net && wi.net != 0).collect();
            let in_pad = |a: crate::geom::quant::IVec2, b: crate::geom::quant::IVec2, width: f64, layer: u32| pads.iter().any(|&pi| super::tidy::track_in_pad(self.model, pi, layer, a, b, width));
            fix.stubs = stub_out_of_pad(&mut items, wi.w, &in_pad);
        }
        (items, fix)
    }

    /// The signal layer of a band's track items.
    fn layer_of_band(&self, items: &[Item]) -> usize {
        let lid = items.iter().find_map(|i| match i {
            Item::Track(t) => Some(t.layer),
            _ => None,
        });
        lid.and_then(|l| self.b.signal_layers.iter().position(|&x| x == l)).unwrap_or(0)
    }

    /// The octilinear form of band `pts` (run `r` of `n` of wire `w`, vias `vs`) when its tracks
    /// pass the checker against the index as `draw_wire`'s attempt -1 takes it (`Drawer::realise`).
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn band_octi(&self, w: WireId, r: usize, n: usize, vs: &[(usize, Vec2, ViaSize)], layer: usize, pts: &[Vec2], pneck: &Option<super::exact::Neck>) -> Option<(Vec<Vec2>, Vec<Item>, Option<(f64, [f64; super::exact::NECK_ZONES])>)> {
        if !self.octi.on || pts.len() < 2 {
            return None;
        }
        let k0 = if r == 0 { 0 } else { vs[r - 1].0 };
        let k1 = if r == n - 1 { self.st.wires[w].passes.len() } else { vs[r].0 };
        let exits = [if r == 0 { self.pad_exit(w, 0, pts[1]) } else { None }, if r == n - 1 { self.pad_exit(w, 1, pts[pts.len() - 2]) } else { None }];
        let o = self.octilinear(w, k0, k1, layer, pts, None, exits, None)?;
        let (items, pn) = self.realise(w, layer, self.layer_id(layer), &o.pts, pneck)?;
        (!items.is_empty()).then_some((o.pts, items, pn))
    }

    /// Takes the reserved bands of wire `w` back out (its drawing is skipped: the time limit, a
    /// test fault; or it failed before its later runs). O(runs of w).
    pub fn release_bands(&mut self, w: WireId) {
        let keys: Vec<(WireId, usize)> = self.joint.reserved.range((w, 0)..=(w, usize::MAX)).map(|(k, _)| *k).collect();
        let cur = self.joint.cur.replace(w);
        for k in keys {
            if let Some(r) = self.joint.reserved.remove(&k) {
                for h in r.handles {
                    self.uncommit(h);
                }
            }
        }
        self.joint.cur = cur;
    }

    /// The violations of a run kept across a model gap (`items` of run `r` of wire `w`, in the
    /// index): against every copper drawn so far, not against bands still reserved (their own
    /// drawing records the violation when it comes second). Empty for any other run.
    pub(super) fn gap_viols(&self, w: WireId, r: usize, items: &[Item]) -> Vec<super::draw::Viol> {
        if !self.joint.gap_runs.contains(&(w, r)) {
            return Vec::new();
        }
        let mut rep = self.checker.check_against(&self.idx, items, &[]);
        rep.violations.retain(|v| {
            let reserved = v.against.strip_prefix("item:").and_then(|t| t.parse::<usize>().ok()).and_then(|id| self.joint.band_ids.get(&id)).is_some_and(|x| self.joint.owner.contains_key(&x.1));
            !reserved
        });
        self.viols_of(items, &rep)
    }

    /// Whether the run `pts` of wire `w` on `layer` breaks a rule against another wire's band copper.
    pub(super) fn hits_band(&self, w: WireId, layer: usize, pts: &[Vec2]) -> bool {
        let items = self.run_items(w, layer, pts);
        let rep = self.checker.check_against(&self.idx, &items, &[]);
        rep.violations.iter().any(|v| v.against.strip_prefix("item:").and_then(|t| t.parse::<usize>().ok()).and_then(|id| self.joint.band_ids.get(&id)).is_some_and(|x| x.0 != w))
    }

    /// The class of run `r` of wire `w` when it is not drawn from its band (`band`: it has one).
    pub(super) fn cut_class(&self, w: WireId, r: usize, band: bool) -> CutClass {
        if !band {
            return CutClass::NoBand;
        }
        if let Some(&c) = self.joint.class.get(&(w, r)) {
            return c;
        }
        if self.joint.kept.contains(&(w, r)) {
            return CutClass::Redrawn;
        }
        CutClass::Junction
    }
}

/// draw-absorb A1 r2 (verifier vfy-draw-absorb-r1 F1; user 2026-11-07 "never narrow-wide-narrow",
/// 2026-11-26 "never widen again after a neck"; lead 2026-11-27: at most one necked stretch per
/// run, touching a run end, or the whole run; the invariant of branch neck-regrow): the track
/// widths `ws` (lengths `ls`) of one run in path order, narrowed so that seen from one of the run's
/// ends the width never grows again: the cheaper (narrowed width x length) of the running minimum
/// from the start (every neck reaches on to the run's end) and from the end (back to its start).
/// One neck in the middle so goes to the nearer end; two necks that would make two stretches join
/// over the stretch between them, and the whole run is necked where both ends are. Only narrows,
/// to widths the run already has. `None`: the run is already one stretch. O(n).
pub fn one_neck_stretch(ws: &[f64], ls: &[f64]) -> Option<Vec<f64>> {
    let n = ws.len();
    let mut fwd = ws.to_vec();
    for i in 1..n {
        fwd[i] = fwd[i].min(fwd[i - 1]);
    }
    let mut bwd = ws.to_vec();
    for i in (0..n.saturating_sub(1)).rev() {
        bwd[i] = bwd[i].min(bwd[i + 1]);
    }
    let same = |v: &[f64]| v.iter().zip(ws).all(|(a, b)| (a - b).abs() <= 0.5);
    if same(&fwd) || same(&bwd) {
        return None;
    }
    let cost = |v: &[f64]| v.iter().zip(ws).zip(ls).map(|((a, b), l)| (b - a) * l).sum::<f64>();
    Some(if cost(&bwd) < cost(&fwd) { bwd } else { fwd })
}

/// Consecutive tracks of `items` (one run, path order) with the same net, layer and width that
/// meet end to start on one straight line (KiCad's `mergeCollinearSegments`, 1 nm) joined into
/// one: the zone cuts of a neck whose pieces got one width. O(n).
pub fn merge_collinear(items: Vec<Item>) -> Vec<Item> {
    use crate::geom::quant::TrackPrim;
    let mut out: Vec<Item> = Vec::with_capacity(items.len());
    for it in items {
        if let (Some(Item::Track(p)), Item::Track(t)) = (out.last_mut(), &it) {
            if let (TrackPrim::Seg { a: pa, b: pb }, TrackPrim::Seg { a: ta, b: tb }) = (p.prim, t.prim) {
                let (d1, d2) = ((i64::from(pb.x - pa.x), i64::from(pb.y - pa.y)), (i64::from(tb.x - ta.x), i64::from(tb.y - ta.y)));
                let cross = (d1.0 as i128 * d2.1 as i128 - d1.1 as i128 * d2.0 as i128) as f64;
                let l1 = ((d1.0 * d1.0 + d1.1 * d1.1) as f64).sqrt().max(1.0);
                let l2 = ((d2.0 * d2.0 + d2.1 * d2.1) as f64).sqrt().max(1.0);
                // both distances of the shared line within 1 nm: |cross| / len of each
                let straight = d1.0 * d2.0 + d1.1 * d2.1 > 0 && cross.abs() / l1 <= 1.0 && cross.abs() / l2 <= 1.0;
                if p.net == t.net && p.layer == t.layer && p.width == t.width && pb == ta && straight {
                    p.prim = TrackPrim::Seg { a: pa, b: tb };
                    continue;
                }
            }
        }
        out.push(it);
    }
    out
}

/// draw-absorb A1 r2 (verifier F2, qmtech det s0: A1 ends 29 -> 34 not on a pad centre, via or
/// T): at each end of the run `items` (tracks in path order, KiCad frame) the end track
/// `p0 -> p1` (`p0` the run's end) narrower than the wire's width `full_w` that KiCad's Cleanup
/// deletes as a track wholly inside a pad of its net (`in_pad(a, b, width, layer)`, tidy's
/// `deleteTracksInPads` test) gets `p1` moved on along `p0 -> p1` by `(full_w - width) / 2`, the
/// next track starting there: the narrow stub then reaches as far out as the full-width one did
/// (its in-pad vertex was placed for the full width), and its copper lies in the full width's
/// disc round the old `p1`, so nothing new is hit (convex hull of discs inside the old envelope).
/// Without the move the stub is deleted and the trace ends at `p1`, inside the pad off its centre
/// (KiCad keeps it; the pad-centre rule does not). Kept only where the moved stub stays
/// (`!in_pad`). Returns the number moved. O(ends x in_pad).
pub fn stub_out_of_pad(items: &mut [Item], full_w: f64, in_pad: &dyn Fn(crate::geom::quant::IVec2, crate::geom::quant::IVec2, f64, u32) -> bool) -> usize {
    use crate::geom::quant::{IVec2, TrackPrim};
    let n = items.len();
    if n < 2 {
        return 0;
    }
    let seg = |it: &Item| match it {
        Item::Track(t) => match t.prim {
            TrackPrim::Seg { a, b } => Some((a, b, t.width as f64, t.layer)),
            _ => None,
        },
        _ => None,
    };
    let mut moved = 0;
    for first in [true, false] {
        let (k, nb) = if first { (0, 1) } else { (n - 1, n - 2) };
        let (Some((a, b, w, layer)), Some((na, nbb, _, _))) = (seg(&items[k]), seg(&items[nb])) else { continue };
        // p0 = the run's end, p1 = the joint to the next track.
        let (p0, p1) = if first { (a, b) } else { (b, a) };
        let joined = if first { na == p1 } else { nbb == p1 };
        if !joined || w >= full_w - 0.5 || !in_pad(p0, p1, w, layer) {
            continue;
        }
        let (dx, dy) = (f64::from(p1.x) - f64::from(p0.x), f64::from(p1.y) - f64::from(p0.y));
        let l = dx.hypot(dy);
        if l < 1.0 {
            continue;
        }
        let s = 0.5 * (full_w - w) / l;
        let q = IVec2 { x: (f64::from(p1.x) + dx * s).round() as i32, y: (f64::from(p1.y) + dy * s).round() as i32 };
        if in_pad(p0, q, w, layer) {
            continue;
        }
        let set = |it: &mut Item, a: IVec2, b: IVec2| {
            if let Item::Track(t) = it {
                t.prim = TrackPrim::Seg { a, b };
            }
        };
        if first {
            set(&mut items[0], p0, q);
            set(&mut items[1], q, nbb);
        } else {
            set(&mut items[n - 1], q, p0);
            set(&mut items[n - 2], na, q);
        }
        moved += 1;
    }
    moved
}

/// draw-absorb A1 r2: the necked stretch of one run's tracks `items` (path order, KiCad frame) of
/// a wire of width `full_w`: `(the run end it touches, math frame; its narrowest width; its
/// length)`, nm; `None` when nothing is narrower. After [`one_neck_stretch`] there is one stretch,
/// touching an end (the start when both: the whole run). O(n).
pub fn neck_of(items: &[Item], full_w: f64) -> Option<(Vec2, f64, f64)> {
    let tr: Vec<&crate::model::TrackItem> = items.iter().filter_map(|i| if let Item::Track(t) = i { Some(t) } else { None }).collect();
    let narrow = |t: &crate::model::TrackItem| (t.width as f64) < full_w - 0.5;
    let len: f64 = tr.iter().filter(|t| narrow(t)).map(|t| {
        let (a, b) = (t.prim.start(), t.prim.end());
        f64::hypot(f64::from(a.x) - f64::from(b.x), f64::from(a.y) - f64::from(b.y))
    }).sum();
    let wmin = tr.iter().filter(|t| narrow(t)).map(|t| t.width as f64).fold(f64::INFINITY, f64::min);
    if len <= 0.0 || !wmin.is_finite() {
        return None;
    }
    let p = if tr.first().is_some_and(|t| narrow(t)) || !tr.last().is_some_and(|t| narrow(t)) { tr[0].prim.start() } else { tr[tr.len() - 1].prim.end() };
    Some((Vec2 { x: f64::from(p.x), y: -f64::from(p.y) }, wmin, len))
}

/// The ERBS counts of the drawn wires `drawn` (one [`RunSrc`] per run) and of the joint commit `j`.
/// O(runs).
pub fn tally<'w>(drawn: impl Iterator<Item = &'w WireOut>, j: &JointState, keep: &str) -> ErbsRunReport {
    let mut r = ErbsRunReport {
        keep: keep.to_string(),
        bands: j.bands,
        kept: j.kept.len(),
        rejected_static: j.n_static,
        rejected_predicted: j.n_predicted,
        rejected_gap: j.n_gap,
        components_exact: j.comp_exact,
        components_greedy: j.comp_greedy,
        lost_to_other: j.lost_to_other,
        left_reserved: j.left_reserved,
        octi_rescued: j.octi_rescued,
        gap_kept: j.gap_kept.len(),
        gap_shrunk: j.gap_shrunk,
        a1_one_stretch: j.a1_mono,
        a1_one_stretch_mm: j.a1_mono_nm / 1e6,
        a1_stub_out: j.a1_stubs,
        unnecked: j.unnecked.len(),
        unnecked_mm: j.unnecked.iter().map(|u| u.2).sum::<f64>() / 1e6,
        unnecked_list: j.unnecked.iter().map(|&(w, r, l, room, at)| crate::route::ErbsUnnecked { wire: w, run: r, mm: l / 1e6, room_mm: room / 1e6, at: at.to_string() }).collect(),
        kept_neck: j.kept_neck,
        band_passes: j.band_pass.passes,
        band_pass_tried: j.band_pass.tried,
        band_pass_moved: j.band_pass.moved,
        band_pass_gaps_before: j.band_pass.gaps_before,
        band_pass_gaps_after: j.band_pass.gaps_after,
        gap_kept_max_mm: j.gap_kept.iter().map(|g| g.overlap_nm).max().unwrap_or(0) as f64 / 1e6,
        gap_kept_list: j
            .gap_kept
            .iter()
            .map(|g| crate::route::ErbsGapKept { wire: g.wire, run: g.run, other_wire: g.other, overlap_mm: g.overlap_nm as f64 / 1e6, x_mm: g.at.0 as f64 / 1e6, y_mm: g.at.1 as f64 / 1e6, layer: g.layer })
            .collect(),
        no_band_list: j
            .no_band
            .iter()
            .map(|(&(w, r), &(layer, why, d))| crate::route::ErbsNoBand { wire: w, run: r, layer, why: why.to_string(), end_moved_mm: d as f64 / 1e6, ..Default::default() })
            .collect(),
        ..ErbsRunReport::default()
    };
    for wo in drawn {
        for src in &wo.run_src {
            r.runs += 1;
            let class = match *src {
                RunSrc::Seq => None,
                RunSrc::Erbs { octi, octi_vs_band } => {
                    r.from_erbs += 1;
                    r.octi_form += octi as usize;
                    r.octi_vs_band += octi_vs_band as usize;
                    None
                }
                RunSrc::Replaced => {
                    r.replaced += 1;
                    None
                }
                RunSrc::Cut(c) => {
                    r.sequential += 1;
                    Some(c)
                }
                RunSrc::Forced { erbs, class } => {
                    r.forced += 1;
                    if erbs {
                        r.forced_erbs += 1;
                        None
                    } else {
                        Some(class)
                    }
                }
            };
            if let Some(c) = class {
                r.cut_back += 1;
                match c {
                    CutClass::Static => r.cut_static += 1,
                    CutClass::Predicted => r.cut_predicted += 1,
                    CutClass::Gap => r.cut_gap += 1,
                    CutClass::NoBand => r.cut_no_band += 1,
                    CutClass::Junction => r.cut_junction += 1,
                    CutClass::Redrawn => r.cut_redrawn += 1,
                }
            }
        }
    }
    r
}

/// The ERBS line's "drawn:" part (`r` from [`tally`]). O(1).
pub fn drawn_text(r: &ErbsRunReport) -> String {
    format!(
        "drawn: {} runs from their ERBS ({} in its octilinear form, {} kept any-angle because their octilinear form hit another band: octi_vs_band), {} by the sequential attempts after the check rejected the ERBS, {} replaced by a cheaper sequential run (length factor), {} drawn by never drop ({} of them on their ERBS); cut back to the sequential way {} of {} runs = {:.1} % (static {}, predicted {}, gap {}, no band {}, junction {}, redrawn {}); joint check ([drawing] erbs_conflict_keep = \"{}\"): {} bands, {} kept, rejected {} static, {} predicted, {} gap; {} kept in their octilinear form (the any-angle band missed a clearance); {} pairs kept across a model gap ([drawing] erbs_gap_keep_max_mm; the gap stays as a clearance violation, worst {:.3} mm); {} conflict components exact, {} greedy (more than {} bands); {} reserved band items lost to other wires",
        r.from_erbs,
        r.octi_form,
        r.octi_vs_band,
        r.sequential,
        r.replaced,
        r.forced,
        r.forced_erbs,
        r.cut_back,
        r.runs,
        if r.runs > 0 { 100.0 * r.cut_back as f64 / r.runs as f64 } else { 0.0 },
        r.cut_static,
        r.cut_predicted,
        r.cut_gap,
        r.cut_no_band,
        r.cut_junction,
        r.cut_redrawn,
        r.keep,
        r.bands,
        r.kept,
        r.rejected_static,
        r.rejected_predicted,
        r.rejected_gap,
        r.octi_rescued,
        r.gap_kept,
        r.gap_kept_max_mm,
        r.components_exact,
        r.components_greedy,
        KEEP_EXACT_CAP,
        r.lost_to_other
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wo(src: &[RunSrc]) -> WireOut {
        WireOut { run_src: src.to_vec(), ..WireOut::default() }
    }

    #[test]
    fn tally_counts_every_class_once() {
        let a = wo(&[RunSrc::Erbs { octi: true, octi_vs_band: false }, RunSrc::Erbs { octi: false, octi_vs_band: true }, RunSrc::Replaced]);
        let b = wo(&[
            RunSrc::Cut(CutClass::Static),
            RunSrc::Cut(CutClass::Predicted),
            RunSrc::Cut(CutClass::Gap),
            RunSrc::Cut(CutClass::NoBand),
            RunSrc::Cut(CutClass::Junction),
            RunSrc::Cut(CutClass::Redrawn),
            RunSrc::Forced { erbs: true, class: CutClass::Gap },
            RunSrc::Forced { erbs: false, class: CutClass::Predicted },
        ]);
        let j = JointState { bands: 9, n_static: 1, n_predicted: 2, n_gap: 3, comp_exact: 4, comp_greedy: 5, lost_to_other: 6, kept: [(1, 0), (2, 0)].into_iter().collect(), ..JointState::default() };
        let r = tally([a, b].iter(), &j, "count");
        assert_eq!(r.runs, 11);
        assert_eq!((r.from_erbs, r.octi_form, r.octi_vs_band, r.replaced), (2, 1, 1, 1));
        assert_eq!(r.sequential, 6);
        assert_eq!((r.cut_back, r.cut_static, r.cut_predicted, r.cut_gap, r.cut_no_band, r.cut_junction, r.cut_redrawn), (7, 1, 2, 1, 1, 1, 1));
        assert_eq!((r.forced, r.forced_erbs), (2, 1));
        assert_eq!((r.bands, r.kept, r.rejected_static, r.rejected_predicted, r.rejected_gap), (9, 2, 1, 2, 3));
        assert_eq!((r.components_exact, r.components_greedy, r.lost_to_other), (4, 5, 6));
        assert_eq!(r.keep, "count");
        let t = drawn_text(&r);
        assert!(t.starts_with("drawn: 2 runs from their ERBS (1 in its octilinear form, 1 kept any-angle"), "{t}");
        assert!(t.contains(", 6 by the sequential attempts after the check rejected the ERBS, 1 replaced by a cheaper sequential run"), "{t}");
        assert!(t.contains("2 drawn by never drop (1 of them on their ERBS); cut back to the sequential way 7 of 11 runs = 63.6 % (static 1, predicted 2, gap 1, no band 1, junction 1, redrawn 1)"), "{t}");
        assert!(t.contains("erbs_conflict_keep = \"count\"): 9 bands, 2 kept, rejected 1 static, 2 predicted, 3 gap; 0 kept in their octilinear form (the any-angle band missed a clearance); 0 pairs kept across a model gap ([drawing] erbs_gap_keep_max_mm; the gap stays as a clearance violation, worst 0.000 mm); 4 conflict components exact, 5 greedy (more than 20 bands); 6 reserved band items lost"), "{t}");
    }

    #[test]
    fn sequential_runs_count_as_runs_only() {
        let r = tally([wo(&[RunSrc::Seq, RunSrc::Seq])].iter(), &JointState::default(), "tier");
        assert_eq!((r.runs, r.from_erbs, r.cut_back, r.forced), (2, 0, 0, 0));
    }

    #[test]
    fn uncommit_bookkeeping_counts_only_foreign_removals() {
        let mut j = JointState::default();
        j.owner.insert(3, 7);
        j.owner.insert(4, 7);
        j.band_ids.insert(10, (7, 3));
        j.band_ids.insert(11, (7, 4));
        j.cur = Some(7);
        j.on_uncommit(3, 10);
        assert_eq!(j.lost_to_other, 0);
        assert!(!j.band_ids.contains_key(&10));
        j.cur = Some(8);
        j.on_uncommit(4, 11);
        assert_eq!(j.lost_to_other, 1);
        // a reused index id of another handle stays
        j.band_ids.insert(12, (9, 5));
        j.on_uncommit(6, 12);
        assert!(j.band_ids.contains_key(&12));
        assert_eq!(j.lost_to_other, 1);
    }

    // ---- draw-absorb A1 r2: one neck stretch per run, pad stubs out of the in-pad delete ----

    const F: f64 = 250_000.0;
    const N: f64 = 90_000.0;

    /// User rule "never widen again after a neck" (lead 2026-11-27): a neck in the middle goes to
    /// the nearer run end; a neck at an end, a staircase down from one end and the whole run are
    /// already one stretch; two necks join over the stretch between them, from the end where that
    /// narrows less; necks at both ends make the whole run.
    #[test]
    fn one_neck_stretch_reaches_the_nearer_run_end() {
        let l = |v: &[f64]| v.to_vec();
        // already one stretch
        assert_eq!(one_neck_stretch(&[N, N, F, F], &[1.0; 4]), None);
        assert_eq!(one_neck_stretch(&[F, F, N], &[1.0; 3]), None);
        assert_eq!(one_neck_stretch(&[F, 200_000.0, N], &[1.0; 3]), None, "a staircase down from one end");
        assert_eq!(one_neck_stretch(&[N, N], &[1.0; 2]), None, "the whole run");
        assert_eq!(one_neck_stretch(&[F, F], &[1.0; 2]), None);
        // a mid neck nearer the end: to the end
        assert_eq!(one_neck_stretch(&[F, F, N, F], &l(&[5.0, 5.0, 1.0, 1.0])), Some(vec![F, F, N, N]));
        // nearer the start: to the start
        assert_eq!(one_neck_stretch(&[F, N, F, F], &l(&[1.0, 1.0, 5.0, 5.0])), Some(vec![N, N, F, F]));
        // by length, not by item count
        assert_eq!(one_neck_stretch(&[F, F, F, N, F], &l(&[0.1, 0.1, 0.1, 1.0, 3.0])), Some(vec![N, N, N, N, F]));
        // two mid necks: one stretch over both, from the cheaper end
        assert_eq!(one_neck_stretch(&[F, N, F, N, F], &l(&[1.0, 1.0, 1.0, 1.0, 9.0])), Some(vec![N, N, N, N, F]));
        // necks at both ends: the whole run
        assert_eq!(one_neck_stretch(&[N, F, F, N], &[1.0; 4]), Some(vec![N, N, N, N]));
        // two widths: the running minimum (never wider again)
        assert_eq!(one_neck_stretch(&[200_000.0, F, N, F], &l(&[1.0, 1.0, 1.0, 9.0])), Some(vec![N, N, N, F]));
    }

    fn tr(w: f64, a: (i32, i32), b: (i32, i32)) -> Item {
        use crate::geom::quant::{IVec2, TrackPrim};
        Item::Track(crate::model::TrackItem { net: 1, layer: 0, width: w as i64, prim: TrackPrim::Seg { a: IVec2 { x: a.0, y: a.1 }, b: IVec2 { x: b.0, y: b.1 } }, group: None, wire: Some(0) })
    }

    fn ends(it: &Item) -> ((i32, i32), (i32, i32), i64) {
        let Item::Track(t) = it else { panic!("track") };
        let (a, b) = (t.prim.start(), t.prim.end());
        ((a.x, a.y), (b.x, b.y), t.width)
    }

    /// Zone cuts of one width on one line are one track again; a bend, another width or a gap
    /// stays a joint.
    #[test]
    fn merge_collinear_joins_cut_pieces_only() {
        let its = vec![tr(N, (0, 0), (1_000, 0)), tr(N, (1_000, 0), (3_000, 0)), tr(N, (3_000, 0), (4_000, 1_000)), tr(F, (4_000, 1_000), (5_000, 2_000)), tr(F, (5_000, 2_000), (6_000, 3_000)), tr(F, (6_001, 3_000), (7_000, 3_000))];
        let m: Vec<_> = merge_collinear(its).iter().map(ends).collect();
        assert_eq!(m, vec![((0, 0), (3_000, 0), N as i64), ((3_000, 0), (4_000, 1_000), N as i64), ((4_000, 1_000), (6_000, 3_000), F as i64), ((6_001, 3_000), (7_000, 3_000), F as i64)]);
        // a reversal is no straight continuation
        let back: Vec<_> = merge_collinear(vec![tr(N, (0, 0), (2_000, 0)), tr(N, (2_000, 0), (1_000, 0))]).iter().map(ends).collect();
        assert_eq!(back.len(), 2);
    }

    /// verifier F2 (qmtech ADV_BGVDD at ADV7511.26): a narrowed end stub whose in-pad vertex was
    /// placed for the full width lies wholly inside its pad at the neck width; KiCad's Cleanup
    /// (tidy's `deleteTracksInPads` test) deletes it and the trace ends off the pad centre. The
    /// vertex moves on by half the width lost, so the stub reaches out of the pad as the full
    /// width did and the trace keeps its centre end; where the moved stub would still be inside
    /// (the full width was inside too), nothing moves; a full-width stub, a stub with no next
    /// track joined to it, or one not in a pad is left alone.
    #[test]
    fn a_narrowed_pad_stub_stays_out_of_the_in_pad_delete() {
        // fx-narrow-gap: pad A1 of net 1, 1 x 1 mm round (3, 10) mm on F.Cu (id 0).
        let m = crate::model::BoardModel::from_json(include_str!("../../tests/fixtures/fx-narrow-gap.json")).unwrap();
        let pi = m.pads.iter().position(|p| p.ref_ == "A1").unwrap();
        let in_pad = |a: crate::geom::quant::IVec2, b: crate::geom::quant::IVec2, w: f64, l: u32| super::super::tidy::track_in_pad(&m, pi, l, a, b, w);
        let c = (3_000_000, 10_000_000);
        // full width: the stub to x 3.4 reaches 3.525 > 3.5 (out); at 0.09 mm 3.445 (inside)
        let p1 = (3_400_000, 10_000_000);
        let ip = |a: (i32, i32), b: (i32, i32), w: f64| in_pad(crate::geom::quant::IVec2 { x: a.0, y: a.1 }, crate::geom::quant::IVec2 { x: b.0, y: b.1 }, w, 0);
        assert!(ip(c, p1, N) && !ip(c, p1, F), "precondition: only the narrow stub is deleted");
        let next = (5_000_000, 12_000_000);
        let mut its = vec![tr(N, c, p1), tr(N, p1, next), tr(F, next, (9_000_000, 12_000_000))];
        assert_eq!(stub_out_of_pad(&mut its, F, &in_pad), 1);
        let q = (3_400_000 + ((F - N) / 2.0) as i32, 10_000_000);
        assert_eq!(ends(&its[0]), (c, q, N as i64));
        assert_eq!(ends(&its[1]), (q, next, N as i64));
        assert!(!ip(c, q, N), "the moved stub is kept by the Cleanup");
        // the same at the run's end (path reversed)
        let mut rev = vec![tr(F, (9_000_000, 12_000_000), next), tr(N, next, p1), tr(N, p1, c)];
        assert_eq!(stub_out_of_pad(&mut rev, F, &in_pad), 1);
        assert_eq!((ends(&rev[1]).1, ends(&rev[2])), (q, (q, c, N as i64)));
        // deeper in: the full width is inside too; no move
        let deep = (3_200_000, 10_000_000);
        let mut its = vec![tr(N, c, deep), tr(N, deep, next)];
        assert_eq!(stub_out_of_pad(&mut its, F, &in_pad), 0);
        assert_eq!(ends(&its[0]).1, deep);
        // full width, not joined, outside the pad
        assert_eq!(stub_out_of_pad(&mut vec![tr(F, c, p1), tr(F, p1, next)], F, &in_pad), 0);
        assert_eq!(stub_out_of_pad(&mut vec![tr(N, c, p1), tr(N, next, (9_000_000, 12_000_000))], F, &in_pad), 0);
        assert_eq!(stub_out_of_pad(&mut vec![tr(N, (6_000_000, 10_000_000), (6_400_000, 10_000_000)), tr(N, (6_400_000, 10_000_000), next)], F, &in_pad), 0);
    }

    /// The stretch's run end, narrowest width and length.
    #[test]
    fn neck_of_names_the_stretch_at_its_run_end() {
        assert_eq!(neck_of(&[tr(F, (0, 0), (1_000, 0))], F), None);
        let at_end = neck_of(&[tr(F, (0, 0), (1_000, 0)), tr(N, (1_000, 0), (1_000, 2_000)), tr(120_000.0, (1_000, 2_000), (1_000, 5_000))], F).unwrap();
        assert_eq!(at_end, (Vec2 { x: 1_000.0, y: -5_000.0 }, N, 5_000.0));
        let at_start = neck_of(&[tr(N, (0, 0), (1_000, 0)), tr(F, (1_000, 0), (1_000, 2_000))], F).unwrap();
        assert_eq!(at_start, (Vec2 { x: 0.0, y: 0.0 }, N, 1_000.0));
    }
}
