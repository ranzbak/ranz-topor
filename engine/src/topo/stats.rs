//! R0 event statistics of a routing (hand or G0): the baseline for factor learning (S3b) and
//! the default weights. Stored, not fitted.
//!
//! Complexity: O(chains · passes) plus O(vias²) for the nearest-neighbour via distances.

use super::state::ObsKind;
use super::witness::{Chain, G0In, Witness, MM};
use crate::geom::Vec2;
use crate::model::BoardModel;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn quant(v: &mut [f64]) -> Value {
    if v.is_empty() {
        return json!({"n": 0});
    }
    v.sort_by(f64::total_cmp);
    let q = |f: f64| v[((v.len() as f64 - 1.0) * f).round() as usize];
    json!({"n": v.len(), "min": q(0.0), "p10": q(0.1), "median": q(0.5), "p90": q(0.9), "p99": q(0.99), "max": q(1.0),
           "mean": v.iter().sum::<f64>() / v.len() as f64})
}

fn nearest_via(v: &[Vec2]) -> Vec<f64> {
    (0..v.len())
        .filter_map(|i| (0..v.len()).filter(|&j| j != i).map(|j| (v[i] - v[j]).len() / MM).min_by(f64::total_cmp))
        .collect()
}

/// Turn angle histogram of the real pieces of a chain, joined at same-layer vertices.
fn bends(ch: &Chain, hist: &mut BTreeMap<&'static str, usize>) {
    for i in 1..ch.meta.len() {
        let (m0, m1) = (ch.meta[i - 1], ch.meta[i]);
        if m0.virt || m1.virt || m0.layer != m1.layer {
            continue;
        }
        let (d0, d1) = (ch.pts[i] - ch.pts[i - 1], ch.pts[i + 1] - ch.pts[i]);
        if d0.len() < 1.0 || d1.len() < 1.0 {
            continue;
        }
        let turn = d0.cross(d1).atan2(d0.dot(d1)).abs().to_degrees();
        let k = if turn < 1.0 {
            "straight"
        } else if (turn - 45.0).abs() <= 3.0 {
            "45"
        } else if (turn - 90.0).abs() <= 3.0 {
            "90"
        } else if turn > 93.0 {
            "acute"
        } else {
            "other"
        };
        *hist.entry(k).or_default() += 1;
    }
}

/// The hand's statistics, and G0's where they can be computed.
pub fn collect(m: &BoardModel, w: &Witness, g0: Option<&G0In>) -> Value {
    let b = &w.board;
    let pr = &b.proj;
    // ---- per chain
    let mut len = Vec::new();
    let mut ratio = Vec::new();
    let mut excess = Vec::new();
    let mut vias = Vec::new();
    let mut xs = Vec::new();
    let mut over_pad = Vec::new();
    let mut layer_len: BTreeMap<u32, [f64; 2]> = BTreeMap::new(); // [signal nets, poured nets]
    let mut hist: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut chain_of = vec![usize::MAX; w.wire_crossings.len()];
    for (c, wi) in w.wire_of.iter().enumerate() {
        if let Some(wi) = wi {
            chain_of[*wi] = c;
        }
    }
    for (ci, (ch, wk)) in w.chains.iter().zip(&w.walked).enumerate() {
        let total: f64 = ch.pts.windows(2).map(|p| (p[1] - p[0]).len()).sum();
        let real: f64 = (0..ch.meta.len()).filter(|&i| !ch.meta[i].virt).map(|i| (ch.pts[i + 1] - ch.pts[i]).len()).sum();
        for i in 0..ch.meta.len() {
            if !ch.meta[i].virt {
                let id = b.signal_layers[ch.meta[i].layer];
                layer_len.entry(id).or_default()[usize::from(b.poured.contains(&ch.net))] += (ch.pts[i + 1] - ch.pts[i]).len() / MM;
            }
        }
        bends(ch, &mut hist);
        if b.poured.contains(&ch.net) {
            continue;
        }
        let straight = (ch.pts[ch.pts.len() - 1] - ch.pts[0]).len();
        len.push(real / MM);
        if straight >= 0.5 * MM {
            ratio.push(total / straight);
            excess.push((total - straight) / MM);
        }
        vias.push(ch.vias as f64);
        if let Some(wi) = w.wire_of[ci] {
            xs.push(w.wire_crossings[wi] as f64);
        }
        // Length over an other-net SMD pad on the other layer.
        let mut op = 0.0;
        for k in 0..=wk.passes.len() {
            let t = wk.tris[k];
            let s0 = if k == 0 { 0.0 } else { wk.passes[k - 1].s };
            let s1 = if k == wk.passes.len() { total } else { wk.passes[k].s };
            let over = pr.tri_obs[t].iter().any(|&o| {
                let ob = &pr.obstacles[o];
                ob.net != Some(ch.net) && ob.kind == ObsKind::Pad && !ob.drilled
            });
            if over {
                op += (s1 - s0).max(0.0);
            }
        }
        over_pad.push(op / MM);
    }
    let _ = chain_of;
    // ---- width floor per netclass
    let mut classes: BTreeMap<String, (f64, Vec<(f64, f64)>)> = BTreeMap::new(); // nominal, (width, length)
    for ch in &w.chains {
        let r = m.rules.net_rule(ch.net);
        let e = classes.entry(r.netclass.clone()).or_insert((r.width as f64 / MM, Vec::new()));
        for i in 0..ch.meta.len() {
            if !ch.meta[i].virt {
                e.1.push((ch.meta[i].width / MM, (ch.pts[i + 1] - ch.pts[i]).len() / MM));
            }
        }
    }
    let width_floor: Vec<Value> = classes
        .iter()
        .map(|(name, (nom, ws))| {
            let mut s = ws.clone();
            s.sort_by(|a, c| a.0.total_cmp(&c.0));
            let tot: f64 = s.iter().map(|x| x.1).sum();
            let mut acc = 0.0;
            let mut med = 0.0;
            for x in &s {
                acc += x.1;
                if acc >= 0.5 * tot {
                    med = x.0;
                    break;
                }
            }
            let necked: f64 = s.iter().filter(|x| x.0 < nom - 1e-6).map(|x| x.1).sum();
            json!({"netclass": name, "nominal_mm": nom, "min_mm": s.first().map(|x| x.0), "median_mm": med,
                   "length_mm": tot, "necked_mm": necked})
        })
        .collect();
    // ---- vias
    let sig_vias: Vec<Vec2> = m.vias.iter().filter(|v| !b.poured.contains(&v.net)).map(|v| v.at.to_vec2()).collect();
    let mut vd = nearest_via(&sig_vias);
    let hand = json!({
        "wire_length_mm": quant(&mut len),
        "detour_ratio": quant(&mut ratio),
        "detour_abs_mm": quant(&mut excess),
        "vias_per_wire": quant(&mut vias),
        "crossings_per_wire": quant(&mut xs),
        "over_other_net_smd_pad_mm": quant(&mut over_pad),
        "chains": w.chains.len(),
        "signal_vias": sig_vias.len(),
        "tight_passes_lt_0_2mm": w.summary.tight_passes,
        "bends_turn_angle": hist,
        "via_nearest_neighbour_mm": quant(&mut vd),
        "layer_length_mm": layer_len.iter().map(|(k, v)| json!({"layer": k, "signal_nets": v[0], "poured_nets": v[1]})).collect::<Vec<_>>(),
    });
    // ---- G0
    let g0v = g0.map(|g| {
        let mut len = Vec::new();
        let mut ratio = Vec::new();
        let mut excess = Vec::new();
        let mut vias = Vec::new();
        let mut layer_len: BTreeMap<i32, f64> = BTreeMap::new();
        for wr in &g.wires {
            let pts: Vec<Vec2> = wr.stretches.iter().flat_map(|s| s.pts.iter().map(|p| Vec2 { x: p[0], y: -p[1] })).collect();
            if pts.len() < 2 {
                continue;
            }
            let total: f64 = wr.stretches.iter().map(|s| s.pts.windows(2).map(|p| (Vec2 { x: p[1][0], y: p[1][1] } - Vec2 { x: p[0][0], y: p[0][1] }).len()).sum::<f64>()).sum();
            for s in &wr.stretches {
                let l: f64 = s.pts.windows(2).map(|p| (Vec2 { x: p[1][0], y: p[1][1] } - Vec2 { x: p[0][0], y: p[0][1] }).len()).sum();
                *layer_len.entry(s.layer).or_default() += l / MM;
            }
            let straight = (pts[pts.len() - 1] - pts[0]).len();
            len.push(total / MM);
            if straight >= 0.5 * MM {
                ratio.push(total / straight);
                excess.push((total - straight) / MM);
            }
            vias.push(wr.stretches.windows(2).filter(|p| p[0].layer != p[1].layer).count() as f64);
        }
        let gv: Vec<Vec2> = g.vias.iter().map(|p| Vec2 { x: p[0], y: -p[1] }).collect();
        let mut vd = nearest_via(&gv);
        let xa = g.summary["crossings_after"].as_f64().unwrap_or(f64::NAN);
        let nw = g.wires.len().max(1) as f64;
        json!({
            "wire_length_mm": quant(&mut len),
            "detour_ratio": quant(&mut ratio),
            "detour_abs_mm": quant(&mut excess),
            "vias_per_wire": quant(&mut vias),
            "crossings_per_wire": xa * 2.0 / nw,
            "crossings_after_p2_single_plane": xa,
            "vias": g.vias.len(),
            "via_nearest_neighbour_mm": quant(&mut vd),
            "layer_length_mm": layer_len.iter().map(|(k, v)| json!({"layer_index": k, "length_mm": v})).collect::<Vec<_>>(),
            "not_computed": ["bends (G0 polylines run through gap midpoints)", "over_other_net_smd_pad", "tight passes"],
        })
    });
    json!({"hand": hand, "g0": g0v, "width_floor": width_floor,
           "bga": "not computed (skipped)", "nonsignal_layer_copper_mm": w.summary.nonsignal_len_mm})
}
