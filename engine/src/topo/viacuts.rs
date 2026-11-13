//! Shopping list CM1 (user 2026-11-13): a foreign band planned through a planned via disc must
//! count in the cut model. The static sheet's cuts end at obstacles; a planned via is no sheet
//! feature (`cutmodel.rs`: it blocks the static cuts it lies on, the exact model "the via a feature
//! that splits the cut" is shopping list 3.1), so a band planned between two planned vias, with no
//! static cut across that gap, was not measured: `topo_reroute52` (planned_crossing_window, cut
//! model) plans SDA on F.Cu between VCC's two vias 1.36 mm apart where 0.4 + 0.2 + 0.25 + 0.2 + 0.4
//! = 1.45 mm are needed, predicts 0 violations, and the drawing then has no room.
//!
//! **Model.** Every two planned vias whose centres lie within reach of each other (their reaches
//! and clearances plus [`BAND_ROOM`]) span a *via cut* on every layer (a through via is copper on
//! every layer): the segment between the centres. Its items are the crossings of the other nets'
//! planned runs (the projection curve of each run, [`super::cutflow::Stretch::curve`]) with the
//! segment on that layer; its capacity is the centre distance; its need is the two via reaches
//! plus the items' widths plus a clearance between neighbours of different nets (the larger of
//! the two), as `cutflow::needs` counts a static cut. Priced like every cut: `price x phi(slack,
//! pitch, tau)` ([`super::cutmodel::phi`]); a via cut without items costs nothing (two vias alone
//! are the via room of `[vias] reserve_sites`, not this). Counted in the over-full count and the
//! graded amount like a static cut (Polubasov 2001 p. 4: the via lies on the wire piece between
//! its two events; Luzin & Polubasov 2008 p. 4: the real via size counts in the optimisation).
//!
//! **Scope (board-rules finding 3).** Cut model only (`[capacity] model = "cuts"`,
//! [`super::cutmodel::CutPrice`]): its runs ([`super::cutflow::Stretch`]) exist there. With the
//! per-edge model (the default since 2026-11-13) the plan does not price via cuts; its side of CM1
//! is the via relocation's band term ([`super::viaopt`] `gap_at`): two vias of one wire keep the
//! room of the other nets' bands the plan crosses between them, so the drawing finds it
//! (`topo_reroute52`). Pricing via cuts in the per-edge plan needs the runs of every wire in every
//! P3 / P4 price and is an open question (runtime).
//!
//! **Incremental.** [`ViaCuts::set_wire`] keeps the runs and vias of every wire; a change of wire
//! `w` re-prices the via cuts of `w`'s vias (dropped and rebuilt) and every via cut whose segment's
//! box meets the box of one of `w`'s old or new runs on that layer. [`ViaCuts::begin`] /
//! [`ViaCuts::rollback`] undo a move. O(via pairs + new via cuts x runs near them) per change.

use super::cutflow::Stretch;
use super::cutmodel::{phi, ViaItem, OVER_TOL};
use super::state::WireId;
use crate::geom::Vec2;
use std::collections::BTreeMap;

/// Room (nm) beyond the two discs and clearances within which two vias span a via cut: enough for
/// a few bands; farther apart a band between them has room to spare (`phi` ~ 0).
pub const BAND_ROOM: f64 = 2.0e6;

/// A via of a wire: (wire, index among its vias).
pub type ViaRef = (WireId, usize);

#[derive(Clone, Debug)]
struct Run {
    layer: usize,
    net: i32,
    w: f64,
    s: f64,
    pts: Vec<Vec2>,
    lo: Vec2,
    hi: Vec2,
}

impl Run {
    fn of(s: &Stretch) -> Run {
        let (mut lo, mut hi) = (Vec2 { x: f64::INFINITY, y: f64::INFINITY }, Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY });
        for p in &s.curve {
            lo = Vec2 { x: lo.x.min(p.x), y: lo.y.min(p.y) };
            hi = Vec2 { x: hi.x.max(p.x), y: hi.y.max(p.y) };
        }
        Run { layer: s.layer, net: s.net, w: s.w, s: s.s, pts: s.curve.clone(), lo, hi }
    }

    fn meets(&self, lo: Vec2, hi: Vec2) -> bool {
        self.lo.x <= hi.x && lo.x <= self.hi.x && self.lo.y <= hi.y && lo.y <= self.hi.y
    }
}

/// One via cut on one layer: slack (nm), item pitch (nm), price; `INFINITY` slack without items.
#[derive(Clone, Copy, Debug)]
pub struct VCut {
    pub slack: f64,
    pub pitch: f64,
    pub price: f64,
}

const EMPTY: VCut = VCut { slack: f64::INFINITY, pitch: 0.0, price: 0.0 };

#[derive(Clone, Debug)]
struct VPair {
    a: ViaItem,
    b: ViaItem,
    per: Vec<VCut>,
}

impl VPair {
    fn box_(&self) -> (Vec2, Vec2) {
        let (p, q) = (self.a.at, self.b.at);
        (Vec2 { x: p.x.min(q.x), y: p.y.min(q.y) }, Vec2 { x: p.x.max(q.x), y: p.y.max(q.y) })
    }
    fn price(&self) -> f64 {
        self.per.iter().map(|c| c.price).sum()
    }
}

/// Undo log of a move: the old value of every via cut the move changed (`None`: it did not exist),
/// so [`ViaCuts::begin`] copies nothing (board-rules finding 6: the old snapshot cloned every via
/// pair on every move).
struct Snap {
    pairs: BTreeMap<(ViaRef, ViaRef), Option<VPair>>,
    total: f64,
    wires: BTreeMap<WireId, (Vec<Run>, Vec<ViaItem>)>,
}

/// The via cuts of the cut model's planner ([`super::cutmodel::CutPrice`]).
pub struct ViaCuts {
    nl: usize,
    price: f64,
    tau: f64,
    runs: Vec<Vec<Run>>,
    vias: Vec<Vec<ViaItem>>,
    pairs: BTreeMap<(ViaRef, ViaRef), VPair>,
    total: f64,
    snap: Option<Snap>,
}

/// Parameter of the crossing of segment `p q` with segment `a b` along `p q` (0..1), if they
/// cross properly. O(1).
fn cross_t(p: Vec2, q: Vec2, a: Vec2, b: Vec2) -> Option<f64> {
    let r = q - p;
    let s = b - a;
    let den = r.cross(s);
    if den.abs() < 1e-9 {
        return None;
    }
    let t = (a - p).cross(s) / den;
    let u = (a - p).cross(r) / den;
    (t > 1e-9 && t < 1.0 - 1e-9 && (0.0..=1.0).contains(&u)).then_some(t)
}

impl ViaCuts {
    pub fn new(nl: usize, n_wires: usize, price: f64, tau: f64) -> Self {
        ViaCuts { nl, price, tau: tau.max(1e-3), runs: vec![Vec::new(); n_wires], vias: vec![Vec::new(); n_wires], pairs: BTreeMap::new(), total: 0.0, snap: None }
    }

    /// The via cut of `a` and `b` on layer `l` under the current runs. O(runs near the segment).
    fn eval(&self, a: &ViaItem, b: &ViaItem, l: usize) -> VCut {
        let (p, q) = (a.at, b.at);
        let lo = Vec2 { x: p.x.min(q.x), y: p.y.min(q.y) };
        let hi = Vec2 { x: p.x.max(q.x), y: p.y.max(q.y) };
        let mut items: Vec<(f64, f64, f64, i32)> = Vec::new();
        for rs in &self.runs {
            for r in rs.iter().filter(|r| r.layer == l && r.net != a.net && r.net != b.net && r.meets(lo, hi)) {
                for x in r.pts.windows(2) {
                    if let Some(t) = cross_t(p, q, x[0], x[1]) {
                        items.push((t, r.w, r.s, r.net));
                    }
                }
            }
        }
        if items.is_empty() {
            return EMPTY;
        }
        items.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut need = a.r + b.r;
        let (mut ps, mut pn) = (a.s, a.net);
        for &(_, w, s, net) in &items {
            if net != pn {
                need += ps.max(s);
            }
            need += w;
            (ps, pn) = (s, net);
        }
        if pn != b.net {
            need += ps.max(b.s);
        }
        let slack = (q - p).len() - need;
        let pitch = items.iter().map(|x| x.1 + x.2).sum::<f64>() / items.len() as f64;
        VCut { slack, pitch, price: self.price * phi(slack, pitch, self.tau) }
    }

    fn eval_all(&self, a: &ViaItem, b: &ViaItem) -> Vec<VCut> {
        (0..self.nl).map(|l| self.eval(a, b, l)).collect()
    }

    /// Replace the runs and vias of wire `w`; returns the change of [`ViaCuts::total`].
    pub fn set_wire(&mut self, w: WireId, runs: &[Stretch], vias: &[ViaItem]) -> f64 {
        if w >= self.runs.len() {
            return 0.0;
        }
        let t0 = self.total;
        if let Some(s) = self.snap.as_mut() {
            s.wires.entry(w).or_insert_with(|| (self.runs[w].clone(), self.vias[w].clone()));
        }
        let old: Vec<Run> = std::mem::take(&mut self.runs[w]);
        self.runs[w] = runs.iter().filter(|r| r.curve.len() >= 2).map(Run::of).collect();
        self.vias[w] = vias.to_vec();
        // The via cuts of w's vias: rebuilt below.
        let mine: Vec<(ViaRef, ViaRef)> = self.pairs.keys().filter(|k| k.0 .0 == w || k.1 .0 == w).copied().collect();
        for k in mine {
            self.log_pair(k);
            if let Some(p) = self.pairs.remove(&k) {
                self.total -= p.price();
            }
        }
        // The other via cuts w's old or new runs may cross: re-priced on that layer.
        let moved: Vec<&Run> = old.iter().chain(self.runs[w].iter()).collect();
        let mut redo: Vec<((ViaRef, ViaRef), usize)> = Vec::new();
        for (k, p) in &self.pairs {
            let (lo, hi) = p.box_();
            for l in 0..self.nl {
                if moved.iter().any(|r| r.layer == l && r.meets(lo, hi)) {
                    redo.push((*k, l));
                }
            }
        }
        for (k, l) in redo {
            self.log_pair(k);
            let (a, b) = {
                let p = &self.pairs[&k];
                (p.a, p.b)
            };
            let c = self.eval(&a, &b, l);
            let p = self.pairs.get_mut(&k).expect("pair");
            self.total += c.price - p.per[l].price;
            p.per[l] = c;
        }
        // New via cuts of w's vias with every via within reach.
        let mut add: Vec<((ViaRef, ViaRef), ViaItem, ViaItem)> = Vec::new();
        for (i, a) in self.vias[w].iter().enumerate() {
            for (u, vs) in self.vias.iter().enumerate() {
                for (j, b) in vs.iter().enumerate() {
                    if u == w && j <= i {
                        continue;
                    }
                    let reach = a.r + a.s + b.r + b.s + BAND_ROOM;
                    if (b.at - a.at).len2() >= reach * reach || (b.at - a.at).len() < 1.0 {
                        continue;
                    }
                    let (ra, rb) = ((w, i), (u, j));
                    let key = if ra < rb { (ra, rb) } else { (rb, ra) };
                    add.push((key, *a, *b));
                }
            }
        }
        for (key, a, b) in add {
            self.log_pair(key);
            let per = self.eval_all(&a, &b);
            let p = VPair { a, b, per };
            self.total += p.price();
            self.pairs.insert(key, p);
        }
        self.total - t0
    }

    /// Record the value of via cut `k` before its first change in the open move. O(log pairs).
    fn log_pair(&mut self, k: (ViaRef, ViaRef)) {
        let pairs = &self.pairs;
        if let Some(s) = self.snap.as_mut() {
            s.pairs.entry(k).or_insert_with(|| pairs.get(&k).cloned());
        }
    }

    /// Open a move ([`ViaCuts::rollback`] undoes every change after it). O(1).
    pub fn begin(&mut self) {
        self.snap = Some(Snap { pairs: BTreeMap::new(), total: self.total, wires: BTreeMap::new() });
    }

    pub fn commit(&mut self) {
        self.snap = None;
    }

    pub fn rollback(&mut self) {
        let Some(s) = self.snap.take() else { return };
        for (k, old) in s.pairs {
            match old {
                Some(p) => {
                    self.pairs.insert(k, p);
                }
                None => {
                    self.pairs.remove(&k);
                }
            }
        }
        self.total = s.total;
        for (w, (r, v)) in s.wires {
            self.runs[w] = r;
            self.vias[w] = v;
        }
    }

    /// Via pairs within reach (each spans a via cut on every layer).
    pub fn pairs(&self) -> usize {
        self.pairs.len()
    }

    /// Total price of the via cuts (mm).
    pub fn total(&self) -> f64 {
        self.total
    }

    fn cuts(&self) -> impl Iterator<Item = &VCut> {
        self.pairs.values().flat_map(|p| p.per.iter())
    }

    /// Over-full via cuts.
    pub fn overfull(&self) -> usize {
        self.cuts().filter(|c| c.slack < -OVER_TOL).count()
    }

    /// Graded amount of the over-full via cuts: `1 - slack / pitch` each.
    pub fn over_amount(&self) -> f64 {
        self.cuts().filter(|c| c.slack < -OVER_TOL).map(|c| 1.0 - c.slack / c.pitch.max(1.0)).sum()
    }

    /// Whether some via cut is over-full and more over-full than at [`ViaCuts::begin`].
    pub fn worse_somewhere(&self) -> bool {
        let Some(s) = self.snap.as_ref() else { return false };
        self.pairs.iter().any(|(k, p)| {
            p.per.iter().enumerate().any(|(l, c)| {
                // unchanged since begin: not worse; changed: against its old value (none: room)
                let before = match s.pairs.get(k) {
                    None => c.slack,
                    Some(o) => o.as_ref().map_or(f64::INFINITY, |o| o.per[l].slack),
                };
                c.slack < -OVER_TOL && c.slack < before - 1.0
            })
        })
    }

    /// The over-full via cuts `(layer, a, b, slack)` (report).
    pub fn overfull_list(&self) -> Vec<(usize, Vec2, Vec2, f64)> {
        let mut out = Vec::new();
        for p in self.pairs.values() {
            for (l, c) in p.per.iter().enumerate() {
                if c.slack < -OVER_TOL {
                    out.push((l, p.a.at, p.b.at, c.slack));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(layer: usize, net: i32, pts: &[(f64, f64)]) -> Stretch {
        let curve: Vec<Vec2> = pts.iter().map(|&(x, y)| Vec2 { x: x * 1e6, y: y * 1e6 }).collect();
        Stretch {
            wire: 0,
            conn: None,
            net,
            layer,
            s: 0.2e6,
            w: 0.25e6,
            w_floor: 0.25e6,
            curve_k: vec![0; curve.len()],
            curve,
            pass_w: Vec::new(),
            nosite: false,
            via_ends: [false; 2],
            band: Vec::new(),
            wraps: Vec::new(),
        }
    }

    fn via(wire: WireId, x: f64, y: f64) -> ViaItem {
        ViaItem { wire, at: Vec2 { x: x * 1e6, y: y * 1e6 }, r: 0.4e6, s: 0.2e6, net: 2, k: 0 }
    }

    #[test]
    fn a_band_between_two_planned_vias_counts() {
        // topo_reroute52's geometry: VCC vias 1.36 mm apart (0.8 mm vias, 0.2 clearance), SDA
        // (0.25 mm, net 1) planned between them on F.Cu: need 0.4 + 0.2 + 0.25 + 0.2 + 0.4 = 1.45.
        let mut vc = ViaCuts::new(2, 2, 1.0, 0.1);
        let d = 1.36;
        vc.set_wire(1, &[run(0, 1, &[(0.68, -3.0), (0.68, 3.0)])], &[]);
        assert_eq!(vc.total(), 0.0, "no vias, no via cut");
        vc.begin();
        let dt = vc.set_wire(0, &[], &[via(0, 0.0, 0.0), via(0, d, 0.0)]);
        assert!(dt > 0.5, "over-full by 0.09 mm: priced {dt}");
        assert_eq!(vc.overfull(), 1, "{:?}", vc.overfull_list());
        assert!((vc.overfull_list()[0].3 + 0.09e6).abs() < 1.0);
        assert!(vc.worse_somewhere());
        vc.rollback();
        assert_eq!(vc.total(), 0.0);
        assert_eq!(vc.overfull(), 0);
        // with the vias 1.6 mm apart the band fits: slack 0.15 mm = 1/3 pitch, priced on the
        // graded tail of phi only (the cuts' and edges' shape), on the one layer with the band
        vc.set_wire(0, &[], &[via(0, 0.0, 0.0), via(0, 1.6, 0.0)]);
        assert_eq!(vc.overfull(), 0);
        let tail = phi(0.15e6, 0.45e6, 0.1);
        assert!(tail < 0.05 && (vc.total() - tail).abs() < 1e-6, "{} vs {tail}", vc.total());
        // moving the band away re-prices the via cut (incremental) ...
        vc.set_wire(0, &[], &[via(0, 0.0, 0.0), via(0, d, 0.0)]);
        assert_eq!(vc.overfull(), 1);
        vc.set_wire(1, &[run(0, 1, &[(5.0, -3.0), (5.0, 3.0)])], &[]);
        assert_eq!(vc.overfull(), 0, "band moved off the gap");
        assert!(vc.total().abs() < 1e-9);
    }

    #[test]
    fn rollback_restores_repriced_removed_and_added_via_cuts() {
        // Finding 6: the undo log records only the via cuts a move changes; a rollback restores
        // a re-priced cut, a removed one and drops an added one.
        let mut vc = ViaCuts::new(2, 3, 1.0, 0.1);
        vc.set_wire(1, &[run(0, 1, &[(0.68, -3.0), (0.68, 3.0)])], &[]);
        vc.set_wire(0, &[], &[via(0, 0.0, 0.0), via(0, 1.36, 0.0)]);
        let (t0, n0, keys0) = (vc.total(), vc.overfull(), vc.pairs.keys().copied().collect::<Vec<_>>());
        assert_eq!(n0, 1);
        vc.begin();
        // re-priced: the band moves off the gap
        vc.set_wire(1, &[run(0, 1, &[(5.0, -3.0), (5.0, 3.0)])], &[]);
        assert_eq!(vc.overfull(), 0);
        // removed and added: wire 0's vias move, wire 2 adds a via near them
        vc.set_wire(0, &[], &[via(0, 10.0, 0.0), via(0, 11.36, 0.0)]);
        vc.set_wire(2, &[], &[via(2, 10.6, 1.0)]);
        assert!(!vc.pairs.is_empty());
        vc.rollback();
        assert!((vc.total() - t0).abs() < 1e-9, "{} vs {t0}", vc.total());
        assert_eq!(vc.overfull(), n0);
        assert_eq!(vc.pairs.keys().copied().collect::<Vec<_>>(), keys0);
        assert!(vc.vias[2].is_empty() && vc.vias[0][0].at.x == 0.0);
        // a second move after the rollback logs afresh
        vc.begin();
        vc.set_wire(1, &[run(0, 1, &[(5.0, -3.0), (5.0, 3.0)])], &[]);
        assert!(!vc.worse_somewhere());
        vc.commit();
        assert_eq!(vc.overfull(), 0);
    }

    #[test]
    fn own_net_runs_and_far_vias_make_no_via_cut() {
        let mut vc = ViaCuts::new(1, 2, 1.0, 0.1);
        // the via net's own run between its vias
        vc.set_wire(1, &[run(0, 2, &[(0.68, -3.0), (0.68, 3.0)])], &[]);
        vc.set_wire(0, &[], &[via(0, 0.0, 0.0), via(0, 1.36, 0.0)]);
        assert_eq!(vc.total(), 0.0);
        // far apart: no pair at all
        let mut vc = ViaCuts::new(1, 2, 1.0, 0.1);
        vc.set_wire(1, &[run(0, 1, &[(4.0, -3.0), (4.0, 3.0)])], &[]);
        vc.set_wire(0, &[], &[via(0, 0.0, 0.0), via(0, 8.0, 0.0)]);
        assert!(vc.pairs.is_empty());
    }
}
