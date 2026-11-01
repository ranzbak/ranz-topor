//! Via sizes and via sites (M3 / T3.2; design note `m3-layers-vias-design.md` §1).
//!
//! A *via site* is a point that is a vertex of every routable layer's CDT and where a through
//! via of at least the smallest wanted size fits (judged by the exact [`Checker`], so pads,
//! tracks, copper shapes, NPTH, existing via drills, the outline and `no_vias` keepouts are
//! respected). Sites are fixed before the first search; only their occupancy changes
//! ([`crate::multigraph::MultiGraph`]).

use crate::cdt::Cdt;
use crate::check::Checker;
use crate::geom::dist::dist_prim_skeleton;
use crate::geom::prim::{Prim, Seg};
use crate::geom::{outline_prims, Aabb, Grid, Skeleton, Vec2};
use crate::graph::{chord, inside_outline};
use crate::model::{BoardModel, Item, Options, PadAttr, Pt, Rules, ViaItem};
use crate::obstacle::ObstacleSet;
use serde::Serialize;

/// Minimum distance between two sites, nm (0.1 mm).
pub const MIN_SITE_PITCH_NM: f64 = 100_000.0;

/// A through-via size: copper diameter and drill, nm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ViaSize {
    pub d: i64,
    pub drill: i64,
}

/// KiCad's factory net class via (diameter, drill), nm: "the board sets no via up".
pub const FACTORY_VIA: [i64; 2] = [800_000, 400_000];

/// The fallback applies to `net`: the board lists no pre-defined via and the net's class via is
/// KiCad's factory 0.8 / 0.4 mm, and the configured fallback is a different size. O(1).
pub fn via_fallback_used(rules: &Rules, net: i32) -> bool {
    let nr = rules.net_rule(net);
    rules.predefined_vias.is_empty()
        && [nr.via_d, nr.via_drill] == FACTORY_VIA
        && rules.via_fallback != FACTORY_VIA
}

/// Via sizes for `net`, largest first, never empty (see [`via_sizes_baked`]); `opts.allowed_drills_nm`
/// is the drill filter. O(|predefined| + |routing_vias|) plus the sort.
pub fn via_sizes(rules: &Rules, net: i32, opts: &Options) -> Vec<ViaSize> {
    sizes_for(rules, net, &opts.allowed_drills_nm)
}

/// [`via_sizes`] with the drill filter baked into `rules.allowed_drills` by `route` (the search
/// has the rules but not the options).
pub fn via_sizes_baked(rules: &Rules, net: i32) -> Vec<ViaSize> {
    sizes_for(rules, net, &rules.allowed_drills)
}

/// Via sizes for `net`, largest first, never empty, single place of truth (46 row 2):
///
/// 1. the net's own `(via_d, via_drill)` (netclass or default) -- or, only when the board sets
///    no via up (`predefined_vias` empty and the class at the factory 0.8 / 0.4), `via_fallback`
///    clamped to the board minimums ([`via_fallback_used`]);
/// 2. then the smaller alternatives: every `predefined_vias` entry and every `routing_vias`
///    (fab profile) drill with `d = max(drill + 2 * min_annular, min_via_diameter)`, kept when
///    `drill >= min_via_drill`, `d >= min_via_diameter`, `(d - drill) / 2 >= min_annular`,
///    `d < first.d`, `drill < first.drill` and (when `allowed` is not empty) `drill` in `allowed`.
///    The list is sorted by `(d, drill)` descending and deduplicated.
///
/// Never a drill outside the predefined / profile lists. O(P log P) for P candidate sizes.
fn sizes_for(rules: &Rules, net: i32, allowed: &[i64]) -> Vec<ViaSize> {
    let nr = rules.net_rule(net);
    let first = if via_fallback_used(rules, net) {
        ViaSize {
            d: rules.via_fallback[0].max(rules.min_via_diameter),
            drill: rules.via_fallback[1].max(rules.min_via_drill),
        }
    } else {
        ViaSize {
            d: nr.via_d,
            drill: nr.via_drill,
        }
    };
    let profile = rules.routing_vias.iter().map(|&dr| ViaSize {
        d: (dr + 2 * rules.min_annular).max(rules.min_via_diameter),
        drill: dr,
    });
    let predefined = rules
        .predefined_vias
        .iter()
        .map(|&[d, drill]| ViaSize { d, drill });
    let mut rest: Vec<ViaSize> = predefined
        .chain(profile)
        .filter(|s| {
            s.drill >= rules.min_via_drill
                && s.d >= rules.min_via_diameter
                && 2 * rules.min_annular <= s.d - s.drill
                && s.d < first.d
                && s.drill < first.drill
                && (allowed.is_empty() || allowed.contains(&s.drill))
        })
        .collect();
    rest.sort_by_key(|b| std::cmp::Reverse((b.d, b.drill)));
    rest.dedup();
    let mut out = vec![first];
    out.extend(rest);
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SiteOrigin {
    /// Incentre of a face of the union triangulation.
    Face,
    /// Next to the SMD terminal pad with this model pad index.
    Escape(usize),
    /// Inside the SMD pad with this model pad index (`Options.via_in_pad`); usable by the
    /// pad's net only ([`ViaSite::net`]).
    InPad(usize),
}

#[derive(Clone, Debug)]
pub struct ViaSite {
    /// Math frame, nm.
    pub at: Vec2,
    /// Largest of the wanted sizes that fits here (> 0 for every returned site).
    pub max_d: i64,
    pub max_drill: i64,
    pub origin: SiteOrigin,
    /// Only this net may use the site (a site inside its own pad); `None` = any net.
    pub net: Option<i32>,
}

/// [`Checker::via_fits`] for a via of net `net` (its own copper is exempt). O(q).
fn via_fits_net(checker: &Checker, at: Vec2, size: ViaSize, net: i32) -> bool {
    let item = Item::Via(ViaItem {
        kind: "via".into(),
        net,
        at: Pt {
            x: at.x.round() as i64,
            y: (-at.y).round() as i64,
        },
        d: size.d,
        drill: size.drill,
        layers: Vec::new(),
        group: None,
        wire: None,
    });
    checker.check_items(&[item]).violations.is_empty()
}

fn hull_radius(sk: &Skeleton, c: Vec2) -> f64 {
    match sk {
        Skeleton::Point { p, r } => (*p - c).len() + r,
        Skeleton::Seg { s, r } => (s.a - c).len().max((s.b - c).len()) + r,
        Skeleton::Poly { pts, r } => pts.iter().map(|p| (*p - c).len()).fold(0.0, f64::max) + r,
    }
}

/// Via sites of the board.
///
/// Candidates: four escape sites per pad of `escape_pads` at `hull_r + c_pad + d_default/2 +
/// eps` along the pad axes, then the incentre of every face of the union triangulation of all
/// anchors of `layers` whose centroid lies inside the outline and whose inradius is at least
/// `d_default/2 + c_max + eps` (a cull only). Each candidate keeps the first of `sizes`
/// (largest first) that `checker.via_fits`; none drops it. Walking escape sites first, then
/// face sites, each by `(x, y)`, a candidate within [`MIN_SITE_PITCH_NM`] of a kept site is
/// dropped. Output sorted by `(x, y)`; the index is the site id for the whole run.
///
/// Complexity: `O(N log N)` union CDT over `N` anchors, `O((F + 4T) * |sizes| * q)` fits for
/// `F` faces, `T` escape pads and `q` grid candidates per check, `O((F + 4T) * k)` dedup with
/// `k` the kept sites within one pitch (grid, O(1) on average).
pub fn via_sites(
    model: &BoardModel,
    obs: &ObstacleSet,
    checker: &Checker,
    layers: &[u32],
    escape_pads: &[usize],
    sizes: &[ViaSize],
    eps: f64,
) -> Vec<ViaSite> {
    via_sites_opts(model, obs, checker, layers, escape_pads, sizes, eps, false)
}

/// Offset of an in-pad site from the pad centre (the terminal vertex), as a fraction of the
/// pad's long half-extent, with a floor so that the two vertices are never coincident.
const IN_PAD_OFFSET_FRAC: f64 = 0.35;
const IN_PAD_OFFSET_MIN_NM: f64 = 150_000.0;

/// Long half-extent and axis angle (rad, math frame) of an SMD pad's main shape, nm.
fn pad_axis(model: &BoardModel, pad: &crate::model::Pad, layer: u32) -> (f64, f64) {
    use crate::model::PadShape;
    let rot = pad.rotation_deg.to_radians();
    let shape = pad
        .shapes
        .get(&layer)
        .or_else(|| pad.shapes.values().next());
    let _ = model;
    match shape {
        Some(PadShape::Circle { d }) => (0.5 * *d as f64, rot),
        Some(PadShape::Rect { w, h })
        | Some(PadShape::RoundRect { w, h, .. })
        | Some(PadShape::Oval { w, h }) => {
            if w >= h {
                (0.5 * *w as f64, rot)
            } else {
                (0.5 * *h as f64, rot + std::f64::consts::FRAC_PI_2)
            }
        }
        _ => (0.0, rot),
    }
}

/// [`via_sites`] with `in_pad` (`Options.via_in_pad`): additionally two candidates per pad of
/// `escape_pads` that is an SMD pad, on the pad's long axis at `max(0.35 * half-length, 0.15 mm)`
/// either side of its centre, kept only when the centre lies inside that pad and a via fits
/// there with the pad's own net (so the pad's copper is not an obstacle but everything else
/// is). Such a site belongs to the pad's net ([`ViaSite::net`]) and is walked before the
/// escape sites. Extra complexity `O(T * |sizes| * q)`.
#[allow(clippy::too_many_arguments)]
pub fn via_sites_opts(
    model: &BoardModel,
    obs: &ObstacleSet,
    checker: &Checker,
    layers: &[u32],
    escape_pads: &[usize],
    sizes: &[ViaSize],
    eps: f64,
    in_pad: bool,
) -> Vec<ViaSite> {
    via_sites_stubs(model, obs, checker, layers, escape_pads, sizes, eps, in_pad, &[])
}

/// [`via_sites_opts`] with the pads that carry an exit stub (A6, `crate::stubs`): their four
/// escape positions are additionally offered as sites of the pad's own net (walked before the
/// plain escape sites, fitted with the net's own copper exempt), so the stub of the pad does not
/// remove the escape via that the pad had without it. Extra `O(4 S · |sizes| · q)`.
#[allow(clippy::too_many_arguments)]
pub fn via_sites_stubs(
    model: &BoardModel,
    obs: &ObstacleSet,
    checker: &Checker,
    layers: &[u32],
    escape_pads: &[usize],
    sizes: &[ViaSize],
    eps: f64,
    in_pad: bool,
    stub_pads: &[usize],
) -> Vec<ViaSite> {
    via_sites_ring(model, obs, checker, layers, escape_pads, sizes, eps, in_pad, stub_pads, &[])
}

/// Directions per ring of [`via_sites_ring`].
pub const RING_DIRS: usize = 16;
/// Rings per pad of [`via_sites_ring`]: the escape radius, then half a default via further out.
pub const RING_COUNT: usize = 2;

/// [`via_sites_stubs`] with a ring of candidates around each pad of `ring_pads` (the SMD pads
/// of plane-net connections, `Options.plane_via_ring`): [`RING_DIRS`] directions on
/// [`RING_COUNT`] circles, the first at the escape radius (`hull_r + c_pad + d_default/2 +
/// eps`), each next one `d_default/2` further out, so a plane via can sit on any free side of
/// its pad instead of only on its four axes. Walked after the escape sites, before the face
/// sites, with the same fit and pitch rules. Extra `O(R · 32 · |sizes| · q)` for `R` ring pads.
#[allow(clippy::too_many_arguments)]
pub fn via_sites_ring(
    model: &BoardModel,
    obs: &ObstacleSet,
    checker: &Checker,
    layers: &[u32],
    escape_pads: &[usize],
    sizes: &[ViaSize],
    eps: f64,
    in_pad: bool,
    stub_pads: &[usize],
    ring_pads: &[usize],
) -> Vec<ViaSite> {
    let r = &model.rules;
    let d_def = r.default.via_d as f64;
    let mut c_max = r
        .default
        .clearance
        .max(r.min_clearance)
        .max(r.hole_clearance)
        .max(r.edge_clearance);
    c_max = r
        .per_net
        .values()
        .map(|n| n.clearance)
        .fold(c_max, i64::max);
    c_max = model
        .pads
        .iter()
        .filter_map(|p| p.local_clearance)
        .fold(c_max, i64::max);
    let cull = 0.5 * d_def + c_max as f64 + eps;

    let mut escapes: Vec<(Vec2, usize)> = Vec::new();
    let mut stub_escapes: Vec<(Vec2, usize)> = Vec::new();
    let mut esc_pads: Vec<(usize, bool)> = escape_pads.iter().map(|&p| (p, false)).collect();
    esc_pads.extend(stub_pads.iter().map(|&p| (p, true)));
    for (pi, own_net) in esc_pads {
        let pad = &model.pads[pi];
        let c = pad.center.to_vec2();
        let layer = pad
            .layers
            .iter()
            .copied()
            .find(|l| layers.contains(l))
            .or_else(|| pad.layers.first().copied())
            .unwrap_or(0);
        let hull = hull_radius(&model.skeleton(pad, layer), c);
        let big_r = hull + model.required_clearance(0, pad) + 0.5 * d_def + eps;
        let rot = pad.rotation_deg.to_radians();
        for k in 0..4 {
            let u = Vec2 { x: 1.0, y: 0.0 }.rot(rot + k as f64 * std::f64::consts::FRAC_PI_2);
            if own_net {
                if pad.net > 0 {
                    stub_escapes.push((c + u * big_r, pi));
                }
            } else {
                escapes.push((c + u * big_r, pi));
            }
        }
    }
    escapes.sort_by(|a, b| a.0.x.total_cmp(&b.0.x).then(a.0.y.total_cmp(&b.0.y)));
    let mut rings: Vec<(Vec2, usize)> = Vec::new();
    for &pi in ring_pads {
        let pad = &model.pads[pi];
        let c = pad.center.to_vec2();
        let layer = pad
            .layers
            .iter()
            .copied()
            .find(|l| layers.contains(l))
            .or_else(|| pad.layers.first().copied())
            .unwrap_or(0);
        let hull = hull_radius(&model.skeleton(pad, layer), c);
        let r0 = hull + model.required_clearance(0, pad) + 0.5 * d_def + eps;
        let rot = pad.rotation_deg.to_radians();
        for ring in 0..RING_COUNT {
            let rr = r0 + ring as f64 * 0.5 * d_def;
            for k in 0..RING_DIRS {
                let a = rot + std::f64::consts::TAU * k as f64 / RING_DIRS as f64;
                rings.push((c + Vec2 { x: 1.0, y: 0.0 }.rot(a) * rr, pi));
            }
        }
    }
    rings.sort_by(|a, b| a.0.x.total_cmp(&b.0.x).then(a.0.y.total_cmp(&b.0.y)));
    stub_escapes.sort_by(|a, b| a.0.x.total_cmp(&b.0.x).then(a.0.y.total_cmp(&b.0.y)));

    let mut faces_c: Vec<Vec2> = Vec::new();
    let mut pts: Vec<Vec2> = Vec::new();
    for &l in layers {
        pts.extend(obs.layer(l).anchors.iter().map(|a| a.c));
    }
    if pts.len() >= 3 {
        if let Ok(cdt) = Cdt::build(&pts, &[]) {
            let chords: Vec<(Vec2, Vec2)> =
                outline_prims(&model.outline).iter().map(chord).collect();
            for f in cdt.topology().faces {
                let [a, b, c] = f.map(|v| cdt.vertex_pos(v));
                let cen = (a + b + c) * (1.0 / 3.0);
                if !inside_outline(cen, &chords) {
                    continue;
                }
                let (la, lb, lc) = ((b - c).len(), (c - a).len(), (a - b).len());
                let per = la + lb + lc;
                let area2 = (b - a).cross(c - a).abs();
                if per <= 0.0 || area2 / per < cull {
                    continue;
                }
                faces_c.push((a * la + b * lb + c * lc) * (1.0 / per));
            }
        }
    }
    faces_c.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));

    let mut own: Vec<(Vec2, usize)> = Vec::new();
    if in_pad {
        for &pi in escape_pads {
            let pad = &model.pads[pi];
            if pad.attr != PadAttr::Smd || pad.net <= 0 {
                continue;
            }
            let Some(layer) = pad.layers.iter().copied().find(|l| layers.contains(l)) else {
                continue;
            };
            let (half, ang) = pad_axis(model, pad, layer);
            if half <= 0.0 {
                continue;
            }
            let off = (IN_PAD_OFFSET_FRAC * half)
                .max(IN_PAD_OFFSET_MIN_NM)
                .min(0.8 * half);
            let c = pad.center.to_vec2();
            let u = Vec2 { x: 1.0, y: 0.0 }.rot(ang);
            let sk = model.skeleton(pad, layer);
            for sgn in [-1.0, 1.0] {
                let q = c + u * (sgn * off);
                let pt = Prim::Seg(Seg { a: q, b: q });
                if dist_prim_skeleton(&pt, &sk) <= 0.0 {
                    own.push((q, pi));
                }
            }
        }
        own.sort_by(|a, b| a.0.x.total_cmp(&b.0.x).then(a.0.y.total_cmp(&b.0.y)));
    }

    let cands = own
        .into_iter()
        .map(|(p, pi)| (p, SiteOrigin::InPad(pi), Some(model.pads[pi].net)))
        .chain(
            stub_escapes
                .into_iter()
                .map(|(p, pi)| (p, SiteOrigin::Escape(pi), Some(model.pads[pi].net))),
        )
        .chain(
            escapes
                .into_iter()
                .map(|(p, pi)| (p, SiteOrigin::Escape(pi), None)),
        )
        .chain(
            rings
                .into_iter()
                .map(|(p, pi)| (p, SiteOrigin::Escape(pi), None)),
        )
        .chain(faces_c.into_iter().map(|p| (p, SiteOrigin::Face, None)));

    let mut kept: Vec<ViaSite> = Vec::new();
    let mut grid: Grid<usize> = Grid::new(MIN_SITE_PITCH_NM);
    for (at, origin, net) in cands {
        let fits = |s: &ViaSize| match net {
            None => checker.via_fits(at, *s),
            Some(n) => via_fits_net(checker, at, *s, n),
        };
        let Some(size) = sizes.iter().find(|s| fits(s)) else {
            continue;
        };
        let bb = Aabb { min: at, max: at }.grow(MIN_SITE_PITCH_NM);
        if grid
            .query(bb)
            .iter()
            .any(|&k| (kept[k].at - at).len() < MIN_SITE_PITCH_NM)
        {
            continue;
        }
        grid.insert(Aabb { min: at, max: at }, kept.len());
        kept.push(ViaSite {
            at,
            max_d: size.d,
            max_drill: size.drill,
            origin,
            net,
        });
    }
    kept.sort_by(|a, b| a.at.x.total_cmp(&b.at.x).then(a.at.y.total_cmp(&b.at.y)));
    kept
}
