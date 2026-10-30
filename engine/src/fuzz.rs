//! Deterministic arc/segment fuzz: items placed at the clearance limit around pads.
//!
//! Every candidate is quantised, then re-verified against the board (`Checker::check_items`)
//! and against the items accepted so far (own per-layer `Grid`, never all-pairs). Attempts are
//! capped at `50·n`; when the cap is hit the accepted items are returned with `truncated`.

use crate::check::Checker;
use crate::geom::prim::Aabb;
use crate::geom::{
    copper_gap, dequantise, quantise, Arc, Grid, Orient, Prim, Seg, Skeleton, Track, TrackPrim,
    Vec2,
};
use crate::model::{flip_track, BoardModel, Item, Options, Pad, TrackItem};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::Serialize;
use std::collections::HashMap;
use std::f64::consts::TAU;

const WIDTHS: [i64; 3] = [200_000, 250_000, 400_000];

#[derive(Clone, Debug, Serialize)]
pub struct FuzzResult {
    /// Items in the KiCad frame (wire format).
    pub items: Vec<Item>,
    /// Debug flag per item: the item was generated as an arc (it may be a segment after
    /// quantisation: the degenerate-arc fallback).
    pub from_arc: Vec<bool>,
    pub truncated: bool,
    pub attempts: usize,
}

/// Anchor circle (centre, radius) on a pad skeleton plus the outward base direction.
fn anchor(rng: &mut ChaCha8Rng, sk: &Skeleton, pad_c: Vec2) -> (Vec2, f64, f64) {
    let (p, r) = match sk {
        Skeleton::Point { p, r } => (*p, *r),
        Skeleton::Seg { s, r } => (if rng.gen_bool(0.5) { s.a } else { s.b }, *r),
        Skeleton::Poly { pts, r } => (pts[rng.gen_range(0..pts.len())], *r),
    };
    let out = p - pad_c;
    let base = if out.len() > 1.0 {
        out.angle() + rng.gen_range(-0.8..0.8)
    } else {
        rng.gen_range(0.0..TAU)
    };
    (p, r, base)
}

fn unit(a: f64) -> Vec2 {
    Vec2 {
        x: a.cos(),
        y: a.sin(),
    }
}

/// `(primitive in the math frame, generated as arc)` at copper gap `gap` to the anchor circle.
fn place(rng: &mut ChaCha8Rng, c: Vec2, r: f64, base: f64, gap: f64, w: f64) -> (Prim, bool) {
    let rho = r + gap + 0.5 * w;
    let u = unit(base);
    if rng.gen_bool(0.5) {
        let len = rng.gen_range(500_000.0..5_000_000.0);
        let t = c + u * rho;
        let dir = u.perp();
        let mid = rng.gen_range(-0.5 * len..0.5 * len);
        let a = t + dir * (mid - 0.5 * len);
        let b = t + dir * (mid + 0.5 * len);
        return (Prim::Seg(Seg { a, b }), false);
    }
    let sigma = if rng.gen_bool(0.5) {
        Orient::Ccw
    } else {
        Orient::Cw
    };
    if rng.gen_bool(0.02) {
        // Flat arc, tangent at distance `gap` (convex away from the pad), sagitta < 5 nm.
        let big_r: f64 = rng.gen_range(20e6..50e6);
        let sag = rng.gen_range(1.0..4.5);
        let sweep = (8.0 * sag / big_r).sqrt();
        let apex = c + u * rho;
        let centre = apex + u * big_r;
        let a_start = (-u).angle() - sigma.sign() * 0.5 * sweep;
        return (
            Prim::Arc(Arc {
                c: centre,
                r: big_r,
                sigma,
                a_start,
                sweep,
            }),
            true,
        );
    }
    let sweep = rng.gen_range(10f64.to_radians()..300f64.to_radians());
    let a_start = base - sigma.sign() * rng.gen_range(0.0..sweep);
    (
        Prim::Arc(Arc {
            c,
            r: rho,
            sigma,
            a_start,
            sweep,
        }),
        true,
    )
}

fn pad_layers(model: &BoardModel, pad: &Pad) -> Vec<u32> {
    pad.layers
        .iter()
        .copied()
        .filter(|l| {
            pad.shapes.contains_key(l)
                && (model.layers.is_empty()
                    || model.layers.iter().any(|x| x.id == *l && x.routable))
        })
        .collect()
}

/// Generate up to `n` self-checked fuzz items (see the module docs).
pub fn fuzz_items(model: &BoardModel, opts: &Options, seed: u64, n: usize) -> FuzzResult {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let checker = Checker::new(model, opts);
    let mut res = FuzzResult {
        items: Vec::new(),
        from_arc: Vec::new(),
        truncated: false,
        attempts: 0,
    };
    let pads: Vec<&Pad> = model
        .pads
        .iter()
        .filter(|p| p.net_tie_group.is_none() && !pad_layers(model, p).is_empty())
        .collect();
    let nets: Vec<i32> = model
        .nets
        .iter()
        .map(|x| x.code)
        .filter(|c| *c != 0)
        .collect();
    let cap = 50 * n;
    let margin = opts.margin_nm as f64;
    let mut accepted: Vec<Track> = Vec::new();
    let mut acc_net: Vec<i32> = Vec::new();
    let mut grids: HashMap<u32, Grid<usize>> = HashMap::new();
    let mut acc_layer: Vec<u32> = Vec::new();
    let cell = 2_000_000.0;
    let mut ring_net: HashMap<(usize, u32), i32> = HashMap::new();

    while res.items.len() < n {
        if res.attempts >= cap || pads.is_empty() || nets.len() < 2 {
            res.truncated = true;
            break;
        }
        res.attempts += 1;
        let pad_idx = rng.gen_range(0..pads.len());
        let pad = pads[pad_idx];
        let layers = pad_layers(model, pad);
        let layer = layers[rng.gen_range(0..layers.len())];
        let others: Vec<i32> = nets.iter().copied().filter(|c| *c != pad.net).collect();
        if others.is_empty() {
            continue;
        }
        // Dense boards: half of the time reuse the net last accepted on this pad ring so that
        // same-net items can stack (different nets cannot share a ring at the clearance limit).
        let ring = (pad_idx, layer);
        let net = match ring_net.get(&ring) {
            Some(&n) if n != pad.net && rng.gen_bool(0.5) => n,
            _ => others[rng.gen_range(0..others.len())],
        };
        let w = WIDTHS[rng.gen_range(0..WIDTHS.len())] as f64;
        let req = model.required_clearance(net, pad);
        let gap = req + margin + rng.gen_range(0.0..50_000.0);
        let sk = model.skeleton(pad, layer);
        let (c, r, base) = anchor(&mut rng, &sk, pad.center.to_vec2());
        let (prim, from_arc) = place(&mut rng, c, r, base, gap, w);
        let Some(q) = quantise(&prim) else { continue };
        let tp: TrackPrim = flip_track(q);
        let item = Item::Track(TrackItem {
            net,
            layer,
            width: w as i64,
            prim: tp,
            group: None,
        });
        let track = Track {
            prim: dequantise(&q),
            w,
        };
        if !opts.skip_self_check {
            if !checker
                .check_items(std::slice::from_ref(&item))
                .violations
                .is_empty()
            {
                continue;
            }
            let bb = track.prim.bbox().grow(0.5 * w);
            let clash = grids.get(&layer).is_some_and(|g| {
                g.query(bb).into_iter().any(|k| {
                    // Same-net items may overlap (no clearance rule between them).
                    acc_net[k] != net && acc_layer[k] == layer && {
                        let req = model
                            .rules
                            .net_rule(net)
                            .clearance
                            .max(model.rules.net_rule(acc_net[k]).clearance)
                            .max(model.rules.min_clearance)
                            as f64
                            + margin;
                        copper_gap(&track, &accepted[k]) < req
                    }
                })
            });
            if clash {
                continue;
            }
        }
        let bb: Aabb = track.prim.bbox().grow(0.5 * w);
        grids
            .entry(layer)
            .or_insert_with(|| Grid::new(cell))
            .insert(bb, accepted.len());
        accepted.push(track);
        acc_net.push(net);
        acc_layer.push(layer);
        ring_net.insert(ring, net);
        res.items.push(item);
        res.from_arc.push(from_arc);
    }
    res
}
