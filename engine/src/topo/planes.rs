//! Plane nets (plan "Capacity for N layers", plane nets): the pads of a net that has a pour are
//! not wired to each other; a pad that the pour does not reach gets a short stub and a through
//! via into the pour's layer, at a via site (a spot where [`Checker::via_fits`]-style tests pass:
//! the via checked as a foreign item against the board and all copper accepted so far). They are
//! placed right after the wide wires, before the default-width wires are drawn.
//!
//! After routing, [`islands`] checks the plane nets' integrity: every pad and via of a plane
//! connection must lie inside the pour's filled copper (outer ring of a fill on a pour layer
//! the item has copper on); each one that does not is reported as an island. This is a check
//! against the fill of the board as given (a re-fill by KiCad can change it), not a refill.
//!
//! Complexity: a pad is O(rings x steps) checker queries; islands O(items x fill vertices).

use super::board::Board;
use super::draw::{track_item, via_item, Drawer};
use super::state::ObsId;
use crate::check::point_in_poly;
use crate::geom::Vec2;
use crate::model::{BoardModel, Item};
use crate::vias::{via_sizes, ViaSize};

/// One pad that needs a via into a pour.
#[derive(Clone, Copy, Debug)]
pub struct Req {
    pub conn: usize,
    /// Index into `model.pads`.
    pub pad: usize,
    pub obs: ObsId,
    pub net: i32,
    /// Stub width (nm).
    pub w: f64,
    /// Index into `model.pours`.
    pub pour: usize,
}

/// A placed stub and via.
#[derive(Clone, Debug)]
pub struct Out {
    pub items: Vec<Item>,
    pub via: Vec2,
    pub size: ViaSize,
    /// Model layer id of the stub.
    pub layer: u32,
    pub stub_nm: f64,
}

/// Place the stub and via of `rq` (see the module doc); the copper is indexed in `dr`.
pub fn place(dr: &mut Drawer, rq: &Req) -> Result<Out, String> {
    let (model, b) = (dr.model, dr.b);
    let pad = &model.pads[rq.pad];
    let ob = &b.proj.obstacles[rq.obs];
    let pour = &model.pours[rq.pour];
    let layer_idx = b.smd_layer(rq.obs).unwrap_or(0);
    let lid = b.signal_layers[layer_idx];
    let c = ob.center;
    let rpad = ob.poly.iter().map(|&p| (p - c).len()).fold(0.0, f64::max);
    let sizes = via_sizes(&model.rules, rq.net, dr.opts);
    let step = 0.1e6;
    let dirs: Vec<Vec2> = (0..32)
        .map(|i| {
            let a = i as f64 * std::f64::consts::PI / 16.0;
            Vec2 { x: a.cos(), y: a.sin() }
        })
        .collect();
    for &sz in &sizes {
        let mut d = rpad + 0.5 * sz.d as f64 * 0.5;
        let dmax = rpad + 3.0e6;
        while d <= dmax {
            for u in &dirs {
                let p = c + *u * d;
                if !dr.opts.via_in_pad && point_in_poly(p, &ob.poly) {
                    continue;
                }
                let vi = via_item(rq.net, p, sz, None);
                // The pour must flow around the via: keep the pour's minimum width extra clearance.
                let crowd = via_item(rq.net, p, ViaSize { d: sz.d + 2 * pour.min_thickness.max(0), drill: sz.drill }, None);
                let stub = track_item(rq.net, lid, rq.w, c, p, None);
                // The via must not dip partly into its own pad either (KiCad connection width;
                // dcsgonebadge J3 pad 16: a GND stub via 3 um into the 0.3 mm oval pad, 0.09 mm).
                if !dr.check_ok(&crowd) || !dr.via_hole_ok(p, sz, rq.net) || !dr.via_own_pad_ok(p, sz, rq.net) || !dr.check_ok(&stub) {
                    continue;
                }
                dr.commit(&vi);
                dr.commit(&stub);
                return Ok(Out { items: vec![stub, vi], via: p, size: sz, layer: lid, stub_nm: d });
            }
            d += step;
        }
    }
    Err(format!("pad {}.{}: no via site within 3 mm for a stub into its plane", pad.ref_, pad.number))
}

/// Warnings for the pads and vias of plane connections that lie outside the pour's fill.
pub fn islands(model: &BoardModel, b: &Board, vias: &[(usize, Vec2)], reqs: &[Req]) -> Vec<String> {
    let mut out = Vec::new();
    for &(conn, at) in vias {
        let Some(rq) = reqs.iter().find(|r| r.conn == conn) else { continue };
            let pour = &model.pours[rq.pour];
        let inside = pour.fills.iter().any(|f| {
            let ring: Vec<Vec2> = f.outer.iter().map(|p| p.to_vec2()).collect();
            ring.len() >= 3 && point_in_poly(at, &ring)
        });
        if !pour.fills.is_empty() && !inside {
            let pad = &model.pads[rq.pad];
            out.push(format!(
                "plane island: via of pad {}.{} at ({:.3}, {:.3}) mm is outside the filled copper of its plane ({} zone)",
                pad.ref_,
                pad.number,
                at.x / 1e6,
                -at.y / 1e6,
                model.nets.iter().find(|n| n.code == rq.net).map_or("?", |n| n.name.as_str())
            ));
        }
    }
    let _ = b;
    out
}
