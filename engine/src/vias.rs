//! Via sizes and via sites (M3 / T3.2; design note `m3-layers-vias-design.md` §1).
//!
//! A *via site* is a point that is a vertex of every routable layer's CDT and where a through
//! via of at least the smallest wanted size fits (judged by the exact [`Checker`], so pads,
//! tracks, copper shapes, NPTH, existing via drills, the outline and `no_vias` keepouts are
//! respected). Sites are fixed before the first search; only their occupancy changes
//! ([`crate::multigraph::MultiGraph`]).

use crate::cdt::Cdt;
use crate::check::Checker;
use crate::geom::{dequantise, Aabb, Grid, Skeleton, Vec2};
use crate::graph::{chord, inside_outline};
use crate::model::{BoardModel, Rules};
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

/// Via sizes for `net`, largest first, never empty: the net's own `(via_d, via_drill)` (netclass
/// or default), then every `rules.routing_vias` drill in `[min_via_drill, via_drill)` with
/// `d = max(drill + 2 * min_annular, min_via_diameter)`; a smaller drill whose `d` would not be
/// smaller than the net's own is skipped (the list stays sorted by `d` and `drill`). Never a
/// drill outside `routing_vias`. O(|routing_vias| log |routing_vias|).
pub fn via_sizes(rules: &Rules, net: i32) -> Vec<ViaSize> {
    let nr = rules.net_rule(net);
    let first = ViaSize {
        d: nr.via_d,
        drill: nr.via_drill,
    };
    let mut rest: Vec<ViaSize> = rules
        .routing_vias
        .iter()
        .copied()
        .filter(|&dr| dr >= rules.min_via_drill && dr < nr.via_drill)
        .map(|dr| ViaSize {
            d: (dr + 2 * rules.min_annular).max(rules.min_via_diameter),
            drill: dr,
        })
        .filter(|s| s.d < first.d)
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
}

#[derive(Clone, Debug)]
pub struct ViaSite {
    /// Math frame, nm.
    pub at: Vec2,
    /// Largest of the wanted sizes that fits here (> 0 for every returned site).
    pub max_d: i64,
    pub max_drill: i64,
    pub origin: SiteOrigin,
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
    for &pi in escape_pads {
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
            escapes.push((c + u * big_r, pi));
        }
    }
    escapes.sort_by(|a, b| a.0.x.total_cmp(&b.0.x).then(a.0.y.total_cmp(&b.0.y)));

    let mut faces_c: Vec<Vec2> = Vec::new();
    let mut pts: Vec<Vec2> = Vec::new();
    for &l in layers {
        pts.extend(obs.layer(l).anchors.iter().map(|a| a.c));
    }
    if pts.len() >= 3 {
        if let Ok(cdt) = Cdt::build(&pts, &[]) {
            let chords: Vec<(Vec2, Vec2)> = model
                .outline
                .iter()
                .map(|o| chord(&dequantise(o)))
                .collect();
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

    let cands = escapes
        .into_iter()
        .map(|(p, pi)| (p, SiteOrigin::Escape(pi)))
        .chain(faces_c.into_iter().map(|p| (p, SiteOrigin::Face)));

    let mut kept: Vec<ViaSite> = Vec::new();
    let mut grid: Grid<usize> = Grid::new(MIN_SITE_PITCH_NM);
    for (at, origin) in cands {
        let Some(size) = sizes.iter().find(|s| checker.via_fits(at, **s)) else {
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
        });
    }
    kept.sort_by(|a, b| a.at.x.total_cmp(&b.at.x).then(a.at.y.total_cmp(&b.at.y)));
    kept
}
