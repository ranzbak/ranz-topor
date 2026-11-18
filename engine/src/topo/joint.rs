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
            gap: bool,
            octi: bool,
            n: usize,
        }
        let mut bands: Vec<Band> = Vec::new();
        for &w in order {
            let (Some(pl), Some(vs)) = (plans.get(&w), placed.get(&w)) else { continue };
            if self.split_at.get(&w).is_some_and(|v| !v.is_empty()) {
                continue;
            }
            let n = pl.layers.len();
            if n == 0 || vs.len() + 1 < n {
                continue;
            }
            let pneck = self.plan_neck(w);
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
                // Static check: only vias and static copper are in the index now.
                let lid = self.layer_id(layer);
                match self.realise(w, layer, lid, &pts, &pneck) {
                    Some((items, pn)) if !items.is_empty() => bands.push(Band { w, r, pts, items, pneck: pn, gap, octi: false, n }),
                    // As attempt -1 does: the band's octilinear form where the any-angle one misses.
                    _ => match self.band_octi(w, r, n, vs, layer, &pts, &pneck) {
                        Some((o, items, pn)) => {
                            bands.push(Band { w, r, pts: o, items, pneck: pn, gap, octi: true, n });
                        }
                        None => {
                            self.joint.class.insert((w, r), CutClass::Static);
                            self.joint.n_static += 1;
                        }
                    },
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
        for (bi, b) in bands.iter().enumerate() {
            let rep = self.checker.check_against(&self.idx, &b.items, &[]);
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
        // A model gap of the ERBS (`RunBand::gap`: no over-full cut predicts it; Dayan 1997 PDF
        // p. 26: bands of a routable class keep the spacing) up to `[drawing]
        // erbs_gap_keep_max_mm` keeps both bands; the gap stays as a counted violation of the band
        // drawn second. A deeper one, or a predicted conflict, goes to the selector.
        let gap_max = (self.opts.erbs_gap_keep_max_mm.max(0.0) * 1e6).round() as i64;
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
                self.joint.reserved.insert((b.w, b.r), Reserved { pts: b.pts, items: b.items, handles: hs, pneck: b.pneck, octi: b.octi });
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
                    self.joint.reserved.insert((b.w, b.r), Reserved { pts: o, items, handles: hs, pneck: pn, octi: true });
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
        let o = self.octilinear(w, k0, k1, layer, pts, None, exits, 0.0)?;
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
        gap_kept_max_mm: j.gap_kept.iter().map(|g| g.overlap_nm).max().unwrap_or(0) as f64 / 1e6,
        gap_kept_list: j
            .gap_kept
            .iter()
            .map(|g| crate::route::ErbsGapKept { wire: g.wire, run: g.run, other_wire: g.other, overlap_mm: g.overlap_nm as f64 / 1e6, x_mm: g.at.0 as f64 / 1e6, y_mm: g.at.1 as f64 / 1e6, layer: g.layer })
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
}
