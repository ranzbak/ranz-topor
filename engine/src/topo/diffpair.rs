//! D7 differential pairs: a pair is one wide wire in the plan (plan "Diff pairs = a wide wire",
//! `wise-hatching-bunny.md`), two coupled tracks in the copper.
//!
//! * **Which nets** come from the model (`rules.diff_pairs`, the plugin's `toporlib.diffpair`: KiCad's
//!   `MatchDpSuffix` on the full net name, one definition). [`plan`] pairs the connections of the two
//!   halves: every pad of the positive net gets its nearest pad of the negative net as partner (same
//!   footprint first), and the negative net's connections are rebuilt to mirror the positive net's
//!   tree, so connection `i` of P and its mirror of N join partner pads. A pair whose pads do not pair
//!   up one to one (different pad counts, a partner far away, a terminal that is no pad) is routed as
//!   two single nets at the pair width and reported uncoupled (never dropped).
//! * **Planning**: the P connection is the pair's one wire (the *lead*), the N connection is carried by
//!   it. Its width is the envelope `w + p / cos 22.5°` (`p = w + gap`, the centre pitch: `2w + gap`
//!   plus the room a 45-degree mitre of the outer track needs), its clearance the larger of the two
//!   halves'. So the demand model (witness, P2, P3: one [`super::demand::Item`] per pass), the crossing
//!   graph (one node), the layer assignment (one via interval) and the order (by width: early) all see
//!   one wire. The planner works on an *alias* model ([`alias`]) in which the N pads, vias and copper
//!   carry the P net: the envelope may lie over the partner pad, every other net still sees them.
//!   The envelope may neck to the single track width at its pad ends (the planner's necking): that
//!   stretch is the fan-out zone, priced by the `necking` factor in P2/P4.
//! * **Via pairs**: the lead's via is the disc around both vias (diameter `pitch_v + d`, drill
//!   `pitch_v + drill`, `pitch_v = d + via gap`), so every via fit check (P3 via room, the drawing's
//!   site search, the packer) is the pair's own fit check. Drawn as two vias side by side, their axis
//!   the allowed direction (`[diffpair] via_pair_axes`) nearest to square to the pair's direction.
//! * **Drawing** ([`realise`]): the lead's drawn centre line per layer becomes two offset polylines at
//!   `+-p/2` (mitred, the `[drawing] style`'s corners: the centre line is octilinear or any-angle),
//!   joined to the pads (or vias) by fan-out stubs. The zip point (TopoR's movable split, 6.3 manual
//!   p. 171) is searched along the centre line from the pad: the nearest point where both stubs and
//!   the first coupled segments keep every clearance; if none does, the least violating one (never
//!   drop). P against N is checked against the pair gap (KiCad's `isCoupledDiffPair` override in the
//!   [`crate::check::Checker`]).
//! * **Statistics** per pair connection: length P/N, skew (actual, and the estimate
//!   `sum 2 s p tan(theta/2)` over the centre line's bends), coupled length and share at the gap (KiCad's
//!   coupling test: parallel segments on one layer, overlapping projection, gap within the tolerance),
//!   uncoupled length at each end, vias.
//!
//! Complexity: [`plan`] O(P^2) per pair for P pads; [`realise`] O(S x Z x q) per connection for S
//! segments, Z zip candidates and q checker queries.

use super::draw::{track_item, via_item, Viol};
use crate::check::{Checker, ItemIndex};
use crate::decompose::{Decomposition, TermKind, TermRef};
use crate::geom::Vec2;
use crate::model::{BoardModel, DiffPairRule, Item, Options};
use crate::vias::ViaSize;
use std::collections::{BTreeMap, BTreeSet};

const MM: f64 = 1e6;
/// cos 22.5 degrees: the outer track of a 45-degree mitre lies `1 / cos 22.5` x its offset from the
/// centre line's corner.
const COS225: f64 = 0.923_879_532_511_286_7;

/// One coupled pair: its rule, the connection pairs `(P conn, N conn)` and the pad partners.
#[derive(Clone, Debug)]
pub struct Coupled {
    pub rule: DiffPairRule,
    pub conns: Vec<(usize, usize)>,
    /// Model pad index -> its partner pad (both directions).
    pub partner: BTreeMap<usize, usize>,
    /// N pads that keep their own net in the plan (terminals of a connection pair routed as two single
    /// tracks).
    pub keep: BTreeSet<usize>,
}

impl Coupled {
    /// Centre pitch of the two tracks.
    pub fn pitch(&self) -> f64 {
        (self.rule.width + self.rule.gap) as f64
    }
    /// Width of the pair's one wire in the plan: both tracks, the gap and the mitre room.
    pub fn envelope(&self) -> f64 {
        self.pitch() / COS225 + self.rule.width as f64
    }
    /// Width that ranks the pair in the routing and drawing order (user 2026-11-08): the band the
    /// pair occupies, space + track + gap + track + space, with `clear` the pair's clearance.
    pub fn priority_width(&self, clear: f64) -> f64 {
        2.0 * clear + 2.0 * self.rule.width as f64 + self.rule.gap as f64
    }
}

/// The pairs of one route.
#[derive(Clone, Debug, Default)]
pub struct Pairs {
    pub coupled: Vec<Coupled>,
    /// P connection id -> (index into `coupled`, N connection id).
    pub lead: BTreeMap<usize, (usize, usize)>,
    /// N connection id -> P connection id.
    pub carried: BTreeMap<usize, usize>,
    /// Nets of every pair (coupled or not) -> the pair width.
    pub width: BTreeMap<i32, i64>,
    /// Nets of the coupled pairs' P halves (the lead nets).
    pub lead_nets: BTreeSet<i32>,
    /// Connection pairs `(P conn, N conn, rule index)` routed as two single tracks because P and N swap
    /// sides between their ends.
    pub single: Vec<(usize, usize, usize)>,
    pub notes: Vec<String>,
}

impl Pairs {
    pub fn of_lead(&self, ci: usize) -> Option<&Coupled> {
        self.lead.get(&ci).map(|&(k, _)| &self.coupled[k])
    }
}

/// The pairs of `model` with the connections of `dec` paired up; the N connections of a coupled
/// pair are rebuilt in place to mirror the P connections (same ids, partner pads). Every pair
/// connection gets the pair width. O(pairs x pads^2).
pub fn plan(model: &BoardModel, dec: &mut Decomposition) -> Pairs {
    let mut out = Pairs::default();
    for r in &model.rules.diff_pairs {
        if r.p == r.n || r.width <= 0 {
            continue;
        }
        out.width.insert(r.p, r.width);
        out.width.insert(r.n, r.width);
        for c in dec.connections.iter_mut().filter(|c| c.net == r.p || c.net == r.n) {
            c.width = r.width;
        }
        let pc: Vec<usize> = dec.connections.iter().filter(|c| c.net == r.p).map(|c| c.id).collect();
        let nc: Vec<usize> = dec.connections.iter().filter(|c| c.net == r.n).map(|c| c.id).collect();
        let pad_of = |t: &TermRef| if let TermKind::Pad(p) = t.kind { Some(p) } else { None };
        type Paired = (BTreeMap<usize, usize>, BTreeMap<usize, TermRef>);
        let why = (|| -> Result<Paired, String> {
            if pc.is_empty() {
                return Err("nothing to route".into());
            }
            if pc.len() != nc.len() {
                return Err(format!("{} connections on {} but {} on {}", pc.len(), r.p_name, nc.len(), r.n_name));
            }
            let mut terms: BTreeMap<usize, TermRef> = BTreeMap::new();
            let (mut pp, mut np) = (BTreeSet::new(), BTreeSet::new());
            for (ids, set) in [(&pc, &mut pp), (&nc, &mut np)] {
                for &i in ids {
                    let c = &dec.connections[i];
                    for t in [&c.from, &c.to] {
                        let p = pad_of(t).ok_or_else(|| format!("terminal {} is no pad", t.label))?;
                        set.insert(p);
                        terms.entry(p).or_insert_with(|| t.clone());
                    }
                }
            }
            if pp.len() != np.len() {
                return Err(format!("{} pads on {} but {} on {}", pp.len(), r.p_name, np.len(), r.n_name));
            }
            let partner = partners(model, &pp, &np);
            let far = (8.0 * (r.width + r.gap) as f64).max(3.0 * MM);
            for (&a, &b) in partner.iter().filter(|(a, _)| pp.contains(*a)) {
                let d = (model.pads[a].center.to_vec2() - model.pads[b].center.to_vec2()).len();
                // A partner farther than a few pitches is no pad pair (a different part of the tree).
                if d > far || model.pads[a].ref_ != model.pads[b].ref_ && d > 0.5 * far {
                    return Err(format!("pad {}.{} has no partner pad of {} within {:.1} mm", model.pads[a].ref_, model.pads[a].number, r.n_name, far / MM));
                }
            }
            Ok((partner, terms))
        })();
        match why {
            Ok((partner, terms)) => {
                // Mirror the P tree onto N, connection i of P to the i-th N connection id.
                let (mut conns, mut crossed) = (Vec::new(), Vec::new());
                let mut keep = BTreeSet::new();
                for (k, &ip) in pc.iter().enumerate() {
                    let c = dec.connections[ip].clone();
                    let (Some(a), Some(b)) = (pad_of(&c.from), pad_of(&c.to)) else { continue };
                    let (na, nb) = (partner[&a], partner[&b]);
                    let jn = nc[k];
                    let n = &mut dec.connections[jn];
                    n.from = terms[&na].clone();
                    n.to = terms[&nb].clone();
                    n.est_len = (n.from.at - n.to.at).len();
                    // P and N swap sides between the ends (their straight lines cross, or run along one
                    // line one behind the other, as between the A and B rows of a USB-C connector): the
                    // two tracks must cross, which needs vias KiCad never places for a pair. Such a
                    // connection pair is routed as two single tracks (its N pads stay N in the plan).
                    let at = |q: usize| model.pads[q].center.to_vec2();
                    if swaps(at(a), at(b), at(na), at(nb)) {
                        crossed.push((ip, jn));
                        keep.insert(na);
                        keep.insert(nb);
                    } else {
                        conns.push((ip, jn));
                    }
                }
                let ri = model.rules.diff_pairs.iter().position(|x| x == r).unwrap_or(0);
                out.single.extend(crossed.iter().map(|&(ip, jn)| (ip, jn, ri)));
                for &(ip, jn) in &crossed {
                    let lbl = |i: usize| format!("{} -> {}", dec.connections[i].from.label, dec.connections[i].to.label);
                    out.notes.push(format!("diff pair {} / {}: {} and {} swap sides between their ends (P and N must cross): routed as two single tracks at the pair width", r.p_name, r.n_name, lbl(ip), lbl(jn)));
                }
                if conns.is_empty() {
                    continue;
                }
                let k = out.coupled.len();
                for &(ip, jn) in &conns {
                    out.lead.insert(ip, (k, jn));
                    out.carried.insert(jn, ip);
                }
                out.lead_nets.insert(r.p);
                out.coupled.push(Coupled { rule: r.clone(), conns, partner, keep });
            }
            Err(e) => {
                if !pc.is_empty() || !nc.is_empty() {
                    out.notes.push(format!("diff pair {} / {}: routed as two single nets at the pair width ({e})", r.p_name, r.n_name));
                }
            }
        }
    }
    out
}

/// The planning model: the N half of every coupled pair carries the P net (pads, vias, kept copper,
/// copper shapes), the P net's rule gets the envelope width, the larger clearance of the two halves,
/// the single track width as its neck floor, and the via-pair disc as its via. O(model).
pub fn alias(model: &BoardModel, pairs: &Pairs, opts: &Options) -> BoardModel {
    let mut m = model.clone();
    for cp in &pairs.coupled {
        let (p, n) = (cp.rule.p, cp.rule.n);
        for (i, x) in m.pads.iter_mut().enumerate() {
            if x.net == n && !cp.keep.contains(&i) {
                x.net = p;
            }
        }
        for x in &mut m.vias {
            if x.net == n {
                x.net = p;
            }
        }
        for x in &mut m.kept {
            if x.net == n {
                x.net = p;
            }
        }
        for x in &mut m.copper_shapes {
            if x.net == n {
                x.net = p;
            }
        }
        // The room between two partner pads belongs to the pair (its fan-out): where another net's
        // track would fit between them, a copper bridge of the P net joins their centres (as thick as
        // the pads' narrow side), so no other net is planned between them (the N stub could not cross
        // it; dcsgonebadge U1.13 / U1.14 at 1.27 mm). Closer pads need none.
        let other = (model.rules.min_track_width + 2 * model.rules.default.clearance.max(model.rules.min_clearance)) as f64;
        for (&a, &b) in cp.partner.iter().filter(|(a, b)| model.pads[**a].net == p && !cp.keep.contains(*b)) {
            let (pa, pb) = (&model.pads[a], &model.pads[b]);
            for &l in pa.layers.iter().filter(|l| pb.layers.contains(l) && model.layers.iter().any(|x| x.id == **l)) {
                let gap = crate::geom::skeleton_gap(&model.skeleton(pa, l), &model.skeleton(pb, l));
                if gap < other {
                    continue;
                }
                let (ca, cb) = (pa.center.to_vec2(), pb.center.to_vec2());
                let narrow = |q: &crate::model::Pad| {
                    let c = pad_corners(q, l);
                    if c.len() < 4 { 0.0 } else { (c[1] - c[0]).len().min((c[2] - c[1]).len()) }
                };
                let t = 0.5 * narrow(pa).min(narrow(pb));
                let Some(u) = (cb - ca).unit() else { continue };
                let n = u.perp() * t;
                let poly = [ca - n, cb - n, cb + n, ca + n];
                m.copper_shapes.push(crate::model::CopperShape {
                    uuid: format!("dp-bridge-{a}-{b}-{l}"),
                    net: p,
                    layer: l,
                    poly: poly.iter().map(|q| crate::model::Pt { x: q.x.round() as i64, y: q.y.round() as i64 }).collect(),
                    owner_ref: None,
                });
            }
        }
        let (rp, rn) = (model.rules.net_rule(p).clone(), model.rules.net_rule(n).clone());
        let mut nr = rp.clone();
        nr.width = cp.envelope().round() as i64;
        nr.clearance = rp.clearance.max(rn.clearance);
        nr.min_width = cp.rule.width;
        let (d, drill) = via_of(model, &cp.rule, opts);
        let pv = (d + cp.rule.via_gap) as f64;
        nr.via_d = (pv + d as f64).round() as i64;
        nr.via_drill = (pv + drill as f64).round() as i64;
        m.rules.per_net.insert(p, nr);
    }
    m
}

/// The outline corners of `pad` on `layer` (math frame): its rectangle (a circle's square, a
/// polygon's points). O(points).
fn pad_corners(pad: &crate::model::Pad, layer: u32) -> Vec<Vec2> {
    use crate::model::PadShape;
    let c = pad.center.to_vec2();
    let rot = pad.rotation_deg.to_radians();
    let shape = pad.shapes.get(&layer).or_else(|| pad.shapes.values().next());
    let (w, h) = match shape {
        Some(PadShape::Circle { d }) => (*d as f64, *d as f64),
        Some(PadShape::Rect { w, h } | PadShape::RoundRect { w, h, .. } | PadShape::Oval { w, h }) => (*w as f64, *h as f64),
        Some(PadShape::Poly { pts }) => return pts.iter().map(|q| q.to_vec2()).collect(),
        None => return vec![c],
    };
    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].iter().map(|&(sx, sy)| c + Vec2 { x: sx * 0.5 * w, y: sy * 0.5 * h }.rot(rot)).collect()
}

/// The via size of a pair's vias: the rule's `via_d` / `via_drill` as the plugin resolved them
/// (`toporlib.diffpair.smallest_via`, `[diffpair] via = "smallest"` by default: the smallest via
/// the board allows that carries the pair width; user 2026-11-12), in either `[vias] size` mode.
///
/// A rule without a via (an older model): with `[vias] size = "board_min"` (5.4) the smallest
/// Board Setup via that carries the pair's `diff_pair_width` ([`crate::vias::rule_size`]), else
/// the P net's via. Never below the custom-rule via minimum of either net (`via_d_min` /
/// `via_drill_min`, [`crate::vias::pair_floor`]): KiCad's DRC checks each via against its net's rules.
pub fn via_of(model: &BoardModel, r: &DiffPairRule, opts: &Options) -> (i64, i64) {
    use crate::vias::{pair_floor, rule_size, ViaSize};
    if r.via_d > 0 && r.via_drill > 0 {
        let s = pair_floor(&model.rules, r, ViaSize { d: r.via_d, drill: r.via_drill });
        return (s.d, s.drill);
    }
    if opts.via_size == "board_min" {
        let w = if r.width > 0 { r.width } else { model.rules.net_rule(r.p).width };
        let s = pair_floor(&model.rules, r, rule_size(&model.rules, w, opts.via_drill_grid_nm, &opts.allowed_drills_nm));
        return (s.d, s.drill);
    }
    let nr = model.rules.net_rule(r.p);
    (nr.via_d, nr.via_drill)
}

/// Pad partners: within each footprint the assignment of P pads to N pads with the least total
/// distance (exhaustive up to 7 pads a side, else greedy nearest), then the rest greedy nearest.
/// Both directions in the map. O(k! ) for k <= 7 pads per footprint, O(P N log) otherwise.
fn partners(model: &BoardModel, pp: &BTreeSet<usize>, np: &BTreeSet<usize>) -> BTreeMap<usize, usize> {
    let d = |a: usize, b: usize| (model.pads[a].center.to_vec2() - model.pads[b].center.to_vec2()).len();
    let mut out = BTreeMap::new();
    let refs: BTreeSet<&str> = pp.iter().map(|&a| model.pads[a].ref_.as_str()).collect();
    for rf in refs {
        let ps: Vec<usize> = pp.iter().copied().filter(|&a| model.pads[a].ref_ == rf).collect();
        let ns: Vec<usize> = np.iter().copied().filter(|&b| model.pads[b].ref_ == rf).collect();
        if ps.len() == ns.len() && !ps.is_empty() && ps.len() <= 7 {
            // Exhaustive: every permutation of ns (Heap's algorithm).
            let mut perm: Vec<usize> = (0..ns.len()).collect();
            let cost = |pm: &[usize]| ps.iter().zip(pm).map(|(&a, &j)| d(a, ns[j])).sum::<f64>();
            let mut best = (cost(&perm), perm.clone());
            let mut c = vec![0usize; perm.len()];
            let mut i = 0;
            while i < perm.len() {
                if c[i] < i {
                    if i % 2 == 0 { perm.swap(0, i) } else { perm.swap(c[i], i) }
                    let v = cost(&perm);
                    if v < best.0 - 1.0 {
                        best = (v, perm.clone());
                    }
                    c[i] += 1;
                    i = 0;
                } else {
                    c[i] = 0;
                    i += 1;
                }
            }
            for (&a, &j) in ps.iter().zip(&best.1) {
                out.insert(a, ns[j]);
                out.insert(ns[j], a);
            }
        }
    }
    let mut cand: Vec<(f64, usize, usize)> = Vec::new();
    for &a in pp.iter().filter(|a| !out.contains_key(a)) {
        for &b in np.iter().filter(|b| !out.contains_key(b)) {
            let pen = if model.pads[a].ref_ == model.pads[b].ref_ { 0.0 } else { 1e9 };
            cand.push((d(a, b) + pen, a, b));
        }
    }
    cand.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)).then(x.2.cmp(&y.2)));
    for (_, a, b) in cand {
        if !out.contains_key(&a) && !out.contains_key(&b) {
            out.insert(a, b);
            out.insert(b, a);
        }
    }
    out
}

/// P runs `pa -> pb`, N `na -> nb`: do they swap sides (the straight lines cross, or lie on one
/// line overlapping)? O(1).
fn swaps(pa: Vec2, pb: Vec2, na: Vec2, nb: Vec2) -> bool {
    let (d1, d2) = (pb - pa, nb - na);
    let den = d1.cross(d2);
    let l = d1.len().max(d2.len()).max(1.0);
    if den.abs() < 1e-3 * l * l {
        // Parallel: crossing only when on one line (within a fifth of the pad pair spacing) and
        // overlapping.
        let off = (na - pa).cross(d1).abs() / d1.len().max(1.0);
        if off > 0.2 * (na - pa).len().min((nb - pb).len()) + 1_000.0 {
            return false;
        }
        let u = d1 * (1.0 / d1.len2().max(1.0));
        let (t0, t1) = ((na - pa).dot(u), (nb - pa).dot(u));
        return t0.min(t1) < 1.0 && t0.max(t1) > 0.0;
    }
    let t = (na - pa).cross(d2) / den;
    let v = (na - pa).cross(d1) / den;
    (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&v)
}

// ------------------------------------------------------------------ geometry

fn unit(v: Vec2) -> Vec2 {
    v.unit().unwrap_or(Vec2 { x: 1.0, y: 0.0 })
}

/// `pts` without consecutive duplicates (closer than 1 nm).
fn clean(pts: &[Vec2]) -> Vec<Vec2> {
    let mut o: Vec<Vec2> = Vec::with_capacity(pts.len());
    for &p in pts {
        if o.last().is_none_or(|q: &Vec2| (p - *q).len() > 1.0) {
            o.push(p);
        }
    }
    o
}

fn length(pts: &[Vec2]) -> f64 {
    pts.windows(2).map(|w| (w[1] - w[0]).len()).sum()
}

/// The point at arc length `t` of `pts` and the index of the segment it lies on.
fn at_len(pts: &[Vec2], t: f64) -> (Vec2, usize) {
    let mut acc = 0.0;
    for i in 0..pts.len().saturating_sub(1) {
        let l = (pts[i + 1] - pts[i]).len();
        if acc + l >= t {
            let u = if l > 0.0 { (t - acc) / l } else { 0.0 };
            return (pts[i] + (pts[i + 1] - pts[i]) * u, i);
        }
        acc += l;
    }
    (*pts.last().unwrap_or(&Vec2::default()), pts.len().saturating_sub(2))
}

/// `pts` between arc lengths `a` and `L - b`.
fn trim(pts: &[Vec2], a: f64, b: f64) -> Vec<Vec2> {
    let l = length(pts);
    let (a, e) = (a.max(0.0), (l - b).max(a.max(0.0) + 1.0).min(l));
    let (pa, ia) = at_len(pts, a);
    let (pe, ie) = at_len(pts, e);
    let mut o = vec![pa];
    o.extend_from_slice(&pts[ia + 1..=ie.min(pts.len() - 1)]);
    o.push(pe);
    clean(&o)
}

/// Mitred offset of a polyline by `o` (positive = left of the travel direction). Segments that
/// the offset turns around (an inner corner shorter than the offset) are dropped and their
/// neighbours joined. O(n^2) worst case, O(n) in practice.
pub fn offset(pts: &[Vec2], o: f64) -> Vec<Vec2> {
    let pts = clean(pts);
    if pts.len() < 2 {
        return pts;
    }
    // Offset lines (point, direction) per segment.
    let mut lines: Vec<(Vec2, Vec2, f64)> = pts.windows(2).map(|w| {
        let d = unit(w[1] - w[0]);
        (w[0] + d.perp() * o, d, (w[1] - w[0]).len())
    }).collect();
    let meet = |a: &(Vec2, Vec2, f64), b: &(Vec2, Vec2, f64)| -> Option<Vec2> {
        let den = a.1.cross(b.1);
        if den.abs() < 1e-9 {
            return None;
        }
        let t = (b.0 - a.0).cross(b.1) / den;
        Some(a.0 + a.1 * t)
    };
    for _ in 0..pts.len() {
        // Vertices, then drop the first segment whose offset turned around.
        let mut v = vec![lines[0].0];
        for i in 1..lines.len() {
            v.push(meet(&lines[i - 1], &lines[i]).unwrap_or(lines[i].0));
        }
        let last = lines[lines.len() - 1];
        v.push(last.0 + last.1 * last.2);
        let bad = (1..lines.len().saturating_sub(1)).find(|&i| (v[i + 1] - v[i]).dot(lines[i].1) < 0.0);
        match bad {
            Some(i) => {
                lines.remove(i);
            }
            None => return clean(&v),
        }
    }
    let mut v = vec![lines[0].0];
    v.push(lines[lines.len() - 1].0 + lines[lines.len() - 1].1 * lines[lines.len() - 1].2);
    v
}

/// Candidate stubs from `a` to `b`: the straight segment, and the two octilinear elbows (45-degree
/// leg first or straight leg first) when the straight one is not at a multiple of 45 degrees.
fn stubs(a: Vec2, b: Vec2, octi: bool) -> Vec<Vec<Vec2>> {
    let d = b - a;
    let mut out = vec![vec![a, b]];
    if !octi || d.len() < 1_000.0 {
        return out;
    }
    let ang = d.y.atan2(d.x).to_degrees().rem_euclid(45.0);
    if ang.min(45.0 - ang) < 0.5 {
        return out;
    }
    let (ax, ay) = (d.x.abs(), d.y.abs());
    let m = ax.min(ay);
    let diag = Vec2 { x: m * d.x.signum(), y: m * d.y.signum() };
    out.insert(0, vec![a, a + diag, b]);
    out.insert(1, vec![a, b - diag, b]);
    out
}

/// The via pair at a layer change at `v`: the allowed axis (degrees in `axes`) nearest to square to
/// the pair's direction (the bisector of `din` and `dout`), P on side `-s`, N on side `+s` of it.
fn via_pair(v: Vec2, din: Vec2, dout: Vec2, s: f64, pv: f64, axes: &[f64]) -> (Vec2, Vec2) {
    let b = unit(unit(din) + unit(dout));
    let want = b.perp();
    let axes: Vec<f64> = if axes.is_empty() { vec![0.0, 45.0, 90.0, 135.0] } else { axes.to_vec() };
    let mut best = (f64::NEG_INFINITY, want);
    for a in axes {
        let u = Vec2 { x: a.to_radians().cos(), y: a.to_radians().sin() };
        let c = u.dot(want).abs();
        if c > best.0 + 1e-12 {
            best = (c, if u.dot(want) >= 0.0 { u } else { -u });
        }
    }
    let u = best.1;
    (v - u * (s * 0.5 * pv), v + u * (s * 0.5 * pv))
}

/// Signed skew estimate `P - N` of a centre line whose N track lies on side `s` (+1 = left) at
/// pitch `p`: each bend of signed turn theta (left positive) makes the left track
/// `p tan(theta / 2)` shorter and the right one as much longer.
pub fn skew_estimate(pts: &[Vec2], s: f64, p: f64) -> f64 {
    let pts = clean(pts);
    let mut sk = 0.0;
    for w in pts.windows(3) {
        let (d0, d1) = (unit(w[1] - w[0]), unit(w[2] - w[1]));
        let th = d0.cross(d1).atan2(d0.dot(d1)).clamp(-3.0, 3.0);
        sk += 2.0 * s * p * (0.5 * th).tan();
    }
    sk
}

/// Coupled length of the P and N tracks (KiCad's coupling test, `drc_test_provider_diff_pair_coupling`):
/// a P segment and an N segment on one layer, parallel within 1 degree, their common projection;
/// it counts when the edge gap is within `tol` of `gap`. Also the largest deviation seen among the
/// parallel neighbours within twice the gap. Returns `(coupled nm, deviating nm, max deviation nm)`.
pub fn coupling(p: &[(u32, Vec2, Vec2)], n: &[(u32, Vec2, Vec2)], w: f64, gap: f64, tol: f64) -> (f64, f64, f64) {
    let (mut cpl, mut dev_len, mut dev_max) = (0.0, 0.0, 0.0f64);
    for &(lp, a, b) in p {
        let Some(d) = (b - a).unit() else { continue };
        let lp_len = (b - a).len();
        for &(ln, c, e) in n {
            if ln != lp {
                continue;
            }
            let Some(d2) = (e - c).unit() else { continue };
            if d.cross(d2).abs() > (1.0f64).to_radians().sin() {
                continue;
            }
            let (t0, t1) = ((c - a).dot(d), (e - a).dot(d));
            let (lo, hi) = (t0.min(t1).max(0.0), t0.max(t1).min(lp_len));
            if hi - lo < 1.0 {
                continue;
            }
            let g = ((c - a).cross(d)).abs() - w;
            if g < 0.0 || g > 2.0 * gap + w {
                continue;
            }
            if (g - gap).abs() <= tol {
                cpl += hi - lo;
            } else {
                dev_len += hi - lo;
                dev_max = dev_max.max((g - gap).abs());
            }
        }
    }
    (cpl, dev_len, dev_max)
}

// ------------------------------------------------------------------ realisation

/// What [`realise`] needs about the board around the pair.
pub struct Ctx<'a> {
    pub model: &'a BoardModel,
    pub opts: &'a Options,
    pub checker: &'a Checker,
    pub octi: bool,
}

/// One end of a run: the two anchors (pad centres or via centres), P first.
#[derive(Clone, Copy, Debug)]
pub struct Ends {
    pub p: Vec2,
    pub n: Vec2,
    /// The longer side of the anchors' pads (0 for vias): KiCad's fan-out runs 1.5 x that straight
    /// out before it turns.
    pub long: f64,
}

/// The longer side of `pad` (its first shape). O(1).
pub fn pad_long(pad: &crate::model::Pad) -> f64 {
    use crate::model::PadShape;
    match pad.shapes.values().next() {
        Some(PadShape::Circle { d }) => *d as f64,
        Some(PadShape::Rect { w, h } | PadShape::RoundRect { w, h, .. } | PadShape::Oval { w, h }) => (*w).max(*h) as f64,
        Some(PadShape::Poly { pts }) => {
            let (mut lo, mut hi) = (Vec2 { x: f64::INFINITY, y: f64::INFINITY }, Vec2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY });
            for q in pts {
                let v = q.to_vec2();
                lo = Vec2 { x: lo.x.min(v.x), y: lo.y.min(v.y) };
                hi = Vec2 { x: hi.x.max(v.x), y: hi.y.max(v.y) };
            }
            (hi.x - lo.x).max(hi.y - lo.y).max(0.0)
        }
        None => 0.0,
    }
}

/// Statistics of one realised pair connection (nm).
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct ConnStats {
    pub length_p: f64,
    pub length_n: f64,
    pub skew: f64,
    pub skew_estimate: f64,
    pub coupled: f64,
    pub coupled_pct: f64,
    /// Coupled stretches at another gap than the target (beyond the tolerance), and the largest
    /// deviation.
    pub gap_dev_len: f64,
    pub gap_dev_max: f64,
    /// Uncoupled (fan-out) length at the start and at the end, P and N.
    pub fan_start: [f64; 2],
    pub fan_end: [f64; 2],
    /// Fan-out length at every via pair (both tracks, both sides).
    pub fan_vias: f64,
    pub vias_p: usize,
    pub vias_n: usize,
    /// P and N swap sides between the two ends (they must cross: KiCad never does that).
    pub polarity_cross: bool,
    /// The N connection id (set by the caller).
    #[serde(skip)]
    pub conn_n: usize,
}

/// The realised copper of one pair connection.
pub struct Realised {
    pub p_items: Vec<Item>,
    pub n_items: Vec<Item>,
    pub p_vias: Vec<(Vec2, ViaSize)>,
    pub n_vias: Vec<(Vec2, ViaSize)>,
    pub stats: ConnStats,
}

/// Violations of `items` against the board and `idx`, and (when `other` is given) of `items`
/// against the copper `other` only (P against N at the pair gap).
pub fn viol_count(ctx: &Ctx, idx: &ItemIndex, items: &[Item], other: Option<&ItemIndex>) -> (usize, f64) {
    let rep = ctx.checker.check_against(idx, items, &[]);
    let mut n = rep.violations.len();
    let mut depth: f64 = rep.violations.iter().map(|v| (v.required - v.gap).max(1) as f64).sum();
    if let Some(ix) = other {
        let r2 = ctx.checker.check_against(ix, items, &[]);
        for v in r2.violations.iter().filter(|v| v.against.starts_with("item:")) {
            n += 1;
            depth += (v.required - v.gap).max(1) as f64;
        }
    }
    (n, depth)
}

/// An index of just `items` (the partner track of a pair).
pub fn index_of(ctx: &Ctx, items: &[Item]) -> ItemIndex {
    let mut ix = ItemIndex::new(ctx.checker.cell());
    for it in items {
        ix.insert(it);
    }
    ix
}

fn tracks(net: i32, layer: u32, w: f64, pts: &[Vec2]) -> Vec<Item> {
    pts.windows(2).filter(|s| (s[1] - s[0]).len() > 1.0).map(|s| track_item(net, layer, w, s[0], s[1], None)).collect()
}

/// The P and N copper of the lead's drawn centre line `runs` (`(layer id, polyline)` in wire order,
/// pad centre to via to ... to pad centre), the anchors at the two ends of the connection, the via
/// sites between the runs. `idx` holds all other copper (checked against). `centre`: the first and the
/// last leg of the centre line are moved sideways onto the middle of their pad pair (the lead's wire
/// ends at the P pad's centre), so the two tracks fan out symmetrically; the caller keeps the better
/// of both. O(runs x zip candidates).
pub fn realise(ctx: &Ctx, cp: &Coupled, runs: &[(u32, Vec<Vec2>)], start: Ends, end: Ends, idx: &ItemIndex, centre: bool) -> Realised {
    let r = &cp.rule;
    let (w, p) = (r.width as f64, cp.pitch());
    let runs: Vec<(u32, Vec<Vec2>)> = runs.iter().map(|(l, pts)| (*l, clean(pts))).filter(|(_, pts)| pts.len() >= 2).collect();
    let mut st = ConnStats::default();
    let (mut p_items, mut n_items) = (Vec::new(), Vec::new());
    let (mut p_vias, mut n_vias) = (Vec::new(), Vec::new());
    if runs.is_empty() {
        return Realised { p_items, n_items, p_vias, n_vias, stats: st };
    }
    // N side: +1 = N left of the travel direction at the start.
    let first = &runs[0].1;
    let d0 = unit(first[1] - first[0]);
    let s = if d0.cross(start.n - start.p) >= 0.0 { 1.0 } else { -1.0 };
    let last = &runs[runs.len() - 1].1;
    let de = unit(last[last.len() - 1] - last[last.len() - 2]);
    let s_end = if de.cross(end.n - end.p) >= 0.0 { 1.0 } else { -1.0 };
    st.polarity_cross = s_end != s;
    // 2 nm over half the pitch: the 45-degree offsets round to whole nm and must not end 1 nm inside
    // the gap.
    let (o_p, o_n) = (-s * (0.5 * p + 2.0), s * (0.5 * p + 2.0));
    let (vd, vdr) = via_of(ctx.model, r, ctx.opts);
    let vsize = ViaSize { d: vd, drill: vdr };
    let pv = (vd + r.via_gap) as f64;
    // Anchors per run end.
    let mut anchors: Vec<(Ends, Ends)> = Vec::new();
    let mut a0 = start;
    for k in 0..runs.len() {
        let pts = &runs[k].1;
        let e = if k + 1 == runs.len() {
            end
        } else {
            let nx = &runs[k + 1].1;
            let din = pts[pts.len() - 1] - pts[pts.len() - 2];
            let dout = nx[1] - nx[0];
            let v = pts[pts.len() - 1];
            let (vp, vn) = via_pair(v, din, dout, s, pv, &ctx.opts.diffpair_via_axes);
            p_vias.push((vp, vsize));
            n_vias.push((vn, vsize));
            Ends { p: vp, n: vn, long: 0.0 }
        };
        anchors.push((a0, e));
        a0 = e;
    }
    let all_p_vias: Vec<Item> = p_vias.iter().map(|&(v, sz)| via_item(r.p, v, sz, None)).collect();
    let all_n_vias: Vec<Item> = n_vias.iter().map(|&(v, sz)| via_item(r.n, v, sz, None)).collect();
    let zip_max = ctx.opts.diffpair_zip_max_nm.max(0) as f64;
    let nr = runs.len();
    for (k, (layer, pts0)) in runs.iter().enumerate() {
        let (sa, ea) = anchors[k];
        let pts = &if centre {
            let mid = |e: Ends| (e.p + e.n) * 0.5;
            let da = if k == 0 { (mid(start) - pts0[0]).dot(unit(pts0[1] - pts0[0]).perp()) } else { 0.0 };
            let m = pts0.len();
            let db = if k + 1 == nr { (mid(end) - pts0[m - 1]).dot(unit(pts0[m - 1] - pts0[m - 2]).perp()) } else { 0.0 };
            shift_ends(pts0, da, db)
        } else {
            pts0.clone()
        };
        let len = length(pts);
        let cap = zip_max.min(0.45 * len);
        // The two tracks' coupled lines (the centre line offset by -+ p/2), each trimmed at its own zip
        // point at either end: P first (its stubs against the board and all other copper), then N
        // (also against P's stubs and lines).
        let (op, on) = (offset(pts, o_p), offset(pts, o_n));
        let rev = |v: &[Vec2]| v.iter().rev().copied().collect::<Vec<_>>();
        let side = |anchor: Vec2, pair: Ends, line: &[Vec2], net: i32, partner: &[Item]| -> (f64, Vec<Vec2>) {
            pick_zip(ctx, idx, anchor, pair, line, net, *layer, w, p, cap, partner)
        };
        // The partner's vias are fixed before the stubs: each track keeps clear of them.
        let (tps, sps) = side(sa.p, sa, &op, r.p, &all_n_vias);
        let (tpe, spe) = side(ea.p, ea, &rev(&op), r.p, &all_n_vias);
        let lp = join(&sps, &trim(&op, tps, tpe), &spe);
        let p_tr = tracks(r.p, *layer, w, &lp);
        let mut p_ctx = p_tr.clone();
        p_ctx.extend(all_p_vias.iter().cloned());
        let (tns, sns) = side(sa.n, sa, &on, r.n, &p_ctx);
        let (tne, sne) = side(ea.n, ea, &rev(&on), r.n, &p_ctx);
        let ln = join(&sns, &trim(&on, tns, tne), &sne);
        p_items.extend(p_tr);
        n_items.extend(tracks(r.n, *layer, w, &ln));
        st.length_p += length(&lp);
        st.length_n += length(&ln);
        st.skew_estimate += skew_estimate(pts, s, p);
        // Uncoupled at an end: from the anchor to where both lines run coupled.
        let (fs, fe) = (tps.max(tns), tpe.max(tne));
        let fan = |stub: &[Vec2], t_own: f64, t_both: f64| length(stub) + (t_both - t_own).max(0.0);
        let (a0, a1) = ([fan(&sps, tps, fs), fan(&sns, tns, fs)], [fan(&spe, tpe, fe), fan(&sne, tne, fe)]);
        if k == 0 {
            st.fan_start = a0;
        } else {
            st.fan_vias += a0[0] + a0[1];
        }
        if k + 1 == runs.len() {
            st.fan_end = a1;
        } else {
            st.fan_vias += a1[0] + a1[1];
        }
    }
    p_items.extend(all_p_vias);
    n_items.extend(all_n_vias);
    st.skew = st.length_p - st.length_n;
    st.vias_p = p_vias.len();
    st.vias_n = n_vias.len();
    let segs = |its: &[Item]| -> Vec<(u32, Vec2, Vec2)> {
        its.iter()
            .filter_map(|it| match it {
                Item::Track(t) => match t.prim {
                    crate::geom::quant::TrackPrim::Seg { a, b } => Some((t.layer, Vec2 { x: f64::from(a.x), y: -f64::from(a.y) }, Vec2 { x: f64::from(b.x), y: -f64::from(b.y) })),
                    _ => None,
                },
                Item::Via(_) => None,
            })
            .collect()
    };
    let (cpl, dl, dm) = coupling(&segs(&p_items), &segs(&n_items), w, r.gap as f64, (r.tolerance.max(1)) as f64);
    st.coupled = cpl;
    st.coupled_pct = if st.length_p.max(st.length_n) > 0.0 { 100.0 * cpl / st.length_p.max(st.length_n) } else { 0.0 };
    st.gap_dev_len = dl;
    st.gap_dev_max = dm;
    Realised { p_items, n_items, p_vias, n_vias, stats: st }
}

/// `pts` with its first leg moved sideways by `da` and its last by `db` (left positive), the
/// corners re-cut where the moved legs meet their neighbours; a single leg moves by the mean.
pub fn shift_ends(pts: &[Vec2], da: f64, db: f64) -> Vec<Vec2> {
    let pts = clean(pts);
    let n = pts.len();
    if n < 2 {
        return pts;
    }
    let legs: Vec<(Vec2, Vec2)> = pts.windows(2).map(|w| (w[0], unit(w[1] - w[0]))).collect();
    let m = legs.len();
    let sh = |i: usize| if m == 1 { 0.5 * (da + db) } else if i == 0 { da } else if i + 1 == m { db } else { 0.0 };
    let lines: Vec<(Vec2, Vec2)> = legs.iter().enumerate().map(|(i, &(a, d))| (a + d.perp() * sh(i), d)).collect();
    let mut v = vec![lines[0].0];
    for i in 1..m {
        let (a, b) = (lines[i - 1], lines[i]);
        let den = a.1.cross(b.1);
        if den.abs() < 1e-9 {
            v.push(pts[i] + b.1.perp() * sh(i));
        } else {
            let t = (b.0 - a.0).cross(b.1) / den;
            v.push(a.0 + a.1 * t);
        }
    }
    v.push(pts[n - 1] + lines[m - 1].1.perp() * sh(m - 1));
    // A moved leg that turned around (shorter than the move) keeps the original line.
    if v.windows(2).zip(&legs).any(|(w, l)| (w[1] - w[0]).dot(l.1) <= 0.0) {
        return pts;
    }
    v
}

/// `stub` (anchor to line start), `body`, then `end` reversed (line end to anchor), cleaned.
fn join(stub: &[Vec2], body: &[Vec2], end: &[Vec2]) -> Vec<Vec2> {
    let mut v: Vec<Vec2> = stub.to_vec();
    v.extend_from_slice(body);
    v.extend(end.iter().rev());
    clean(&v)
}

/// The zip point of one track at one end: the distance `t` along its coupled line `line` (oriented
/// from this end) where it leaves its stub from `anchor`, and the stub. The nearest `t` whose stub
/// and first coupled segments keep every clearance (board, `idx`, the partner track `partner`)
/// wins; in the octilinear style first among stubs that turn at most 45 degrees into the line. None
/// clean within `cap`: the least violating (never drop). O(candidates x stubs x checker query).
#[allow(clippy::too_many_arguments)]
fn pick_zip(ctx: &Ctx, idx: &ItemIndex, anchor: Vec2, pair: Ends, line: &[Vec2], net: i32, layer: u32, w: f64, p: f64, cap: f64, partner: &[Item]) -> (f64, Vec<Vec2>) {
    // KiCad's pad gateways (pns_diff_pair.cpp:556-579): a stub leaves square to the line through the
    // two pads (either way) before it turns towards the coupled line.
    let axis = pair.n - pair.p;
    let gate: Vec<(Vec2, f64)> = match axis.unit() {
        Some(u) => {
            let g = u.perp();
            let d = axis.len();
            // Multiples of the pad spacing, and KiCad's straight run out of the pad
            // (`DP_GATEWAYS::BuildFromPrimitivePair`, pns_diff_pair.cpp @ 86a7a3f: 1.5 x (pad long side + 1)):
            // between connector pins with a shield pin in the middle the turn must come past the pin tips.
            let mut ls: Vec<f64> = [0.5, 1.0, 2.0].iter().map(|&k| k * d).collect();
            if pair.long > 0.0 {
                ls.extend([0.5 * pair.long + p, 1.5 * pair.long]);
            }
            ls.iter().flat_map(|&l| [(g, l), (-g, l)]).collect()
        }
        None => Vec::new(),
    };
    let mut ts = vec![0.0, 0.25 * p, 0.5 * p, 0.75 * p, p, 1.5 * p, 2.0 * p, 3.0 * p];
    let mut t = 4.0 * p;
    while t <= cap {
        ts.push(t);
        t += (0.5 * p).max(100_000.0);
    }
    ts.retain(|&x| x <= cap + 1.0);
    if ts.is_empty() {
        ts.push(0.0);
    }
    let pix = (!partner.is_empty()).then(|| index_of(ctx, partner));
    let mut best: Option<((usize, f64), f64, Vec<Vec2>)> = None;
    for gentle in [true, false] {
        if gentle && !ctx.octi {
            continue;
        }
        for &t in &ts {
            let body = trim(line, t, 0.0);
            if body.len() < 2 {
                continue;
            }
            let dir = unit(body[1] - body[0]);
            let mut cands = stubs(anchor, body[0], ctx.octi);
            for &(g, l) in &gate {
                let q = anchor + g * l;
                for mut tail in stubs(q, body[0], ctx.octi) {
                    tail.insert(0, anchor);
                    cands.push(tail);
                }
            }
            for stub in cands {
                let steep = stub.windows(2).any(|sg| (sg[1] - sg[0]).len() > 1_000.0 && unit(sg[1] - sg[0]).dot(dir) < 0.70);
                // The gentle pass takes the stubs that turn at most 45 degrees into the line, the second
                // only the others (in the octilinear style).
                if ctx.octi && gentle == steep {
                    continue;
                }
                let mut its = tracks(net, layer, w, &stub);
                its.extend(tracks(net, layer, w, &body[..body.len().min(3)]));
                let (n, d) = viol_count(ctx, idx, &its, pix.as_ref());
                if n == 0 {
                    return (t, stub);
                }
                if best.as_ref().is_none_or(|b| n < b.0 .0 || (n == b.0 .0 && d < b.0 .1 - 1.0)) {
                    best = Some(((n, d), t, stub));
                }
            }
        }
    }
    best.map_or_else(|| (0.0, vec![anchor, line[0]]), |b| (b.1, b.2))
}

/// Violations of the realised `items` against the board and `idx` (as the drawing reports them).
pub fn check(ctx: &Ctx, idx: &ItemIndex, items: &[Item]) -> Vec<Viol> {
    let rep = ctx.checker.check_against(idx, items, &[]);
    super::draw::viols_with(ctx.model, ctx.opts, ctx.checker, idx, items, &rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2 { x: x * MM, y: y * MM }
    }

    #[test]
    fn priority_width_is_the_whole_band_of_the_pair() {
        // 0.2 mm tracks, 0.15 mm gap, 0.2 mm clearance: 0.2 + 0.2 + 0.15 + 0.2 + 0.2 = 0.95 mm, wider
        // than a 0.5 mm power track (routed after the pair) and narrower than a 1 mm one.
        let rule = DiffPairRule { width: (0.2 * MM) as i64, gap: (0.15 * MM) as i64, ..Default::default() };
        let cp = Coupled { rule, conns: Vec::new(), partner: BTreeMap::new(), keep: BTreeSet::new() };
        assert!((cp.priority_width(0.2 * MM) - 0.95 * MM).abs() < 1.0);
    }

    #[test]
    fn offset_of_a_45_degree_bend_is_mitred_at_constant_distance() {
        let c = vec![v(0.0, 0.0), v(5.0, 0.0), v(10.0, 5.0)];
        let l = offset(&c, 0.2 * MM);
        assert_eq!(l.len(), 3);
        // Start 0.2 mm left (up) of the centre line, end 0.2 mm left of the diagonal.
        assert!((l[0] - v(0.0, 0.2)).len() < 1.0);
        let d = unit(v(5.0, 5.0));
        assert!(((l[2] - v(10.0, 5.0)) - d.perp() * (0.2 * MM)).len() < 1.0);
        // Inner (left) track of a left turn is shorter by 2 x 0.2 tan 22.5.
        let r = offset(&c, -0.2 * MM);
        let diff = length(&r) - length(&l);
        assert!((diff - 2.0 * 0.4 * MM * (22.5f64).to_radians().tan()).abs() < 10.0, "{diff}");
        assert!((skew_estimate(&c, 1.0, 0.4 * MM) - (length(&r) - length(&l))).abs() < 10.0);
    }

    #[test]
    fn offset_drops_a_turned_around_inner_segment() {
        // A U-turn with a 0.1 mm leg: the inner offset at 0.3 mm cannot keep the leg.
        let c = vec![v(0.0, 0.0), v(5.0, 0.0), v(5.0, 0.1), v(0.0, 0.1)];
        let l = offset(&c, 0.3 * MM);
        for w in l.windows(2) {
            assert!((w[1] - w[0]).len() > 0.0);
        }
        assert!(l.len() <= 3, "{l:?}");
    }

    #[test]
    fn coupling_counts_parallel_overlap_at_the_gap() {
        let p = vec![(0u32, v(0.0, 0.0), v(10.0, 0.0))];
        let n = vec![(0u32, v(2.0, 0.4), v(12.0, 0.4))];
        let (c, dl, _) = coupling(&p, &n, 0.2 * MM, 0.2 * MM, 0.01 * MM);
        assert!((c - 8.0 * MM).abs() < 1.0 && dl == 0.0, "{c} {dl}");
        let n2 = vec![(0u32, v(0.0, 0.5), v(10.0, 0.5))];
        let (c2, dl2, dm) = coupling(&p, &n2, 0.2 * MM, 0.2 * MM, 0.01 * MM);
        assert!(c2 == 0.0 && (dl2 - 10.0 * MM).abs() < 1.0 && (dm - 0.1 * MM).abs() < 1.0);
    }

    #[test]
    fn swapped_sides_are_found() {
        // qmtech USB_C_D: U9 D+ right of D-, U1 D+ left of D-: the straight lines cross.
        assert!(swaps(v(100.177, 0.0), v(99.627, -2.6), v(99.677, 0.0), v(100.177, -2.6)));
        // Two parallel rows, P above N at both ends (fx_dp_straight).
        assert!(!swaps(v(3.0, -9.75), v(27.0, -9.75), v(3.0, -10.25), v(27.0, -10.25)));
        // HDMI: P left of N at both ends.
        assert!(!swaps(v(133.72, 0.0), v(133.2, 5.2), v(134.22, 0.0), v(134.2, 5.2)));
        // USB-C rows: B6 -> A6 and A7 -> B7 on one line, overlapping.
        assert!(swaps(v(89.25, 0.0), v(90.25, 0.0), v(89.75, 0.0), v(90.75, 0.0)));
    }

    #[test]
    fn via_pair_axis_is_square_to_the_pair() {
        let (p, n) = via_pair(v(0.0, 0.0), v(1.0, 0.0), v(1.0, 0.0), 1.0, 0.65 * MM, &[0.0, 45.0, 90.0, 135.0]);
        assert!((p - v(0.0, -0.325)).len() < 1.0 && (n - v(0.0, 0.325)).len() < 1.0, "{p:?} {n:?}");
    }
}
