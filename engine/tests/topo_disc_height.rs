//! tw4-r2 (2b) (`[capacity] via_disc_height`, docs/dev/design/tw4-r2.md (2b)): a planned via's
//! disc packs on an edge at its height off the edge line. The pair rule against a track (crossing
//! at sin θ = 1 / infl, leaning `c`): `max(0, (R + h c) / sin θ)`; against a disc at signed
//! heights: `sqrt(max(0, R^2 - (h1 - h2)^2))`; the end margin at the offset centre. Each number is
//! checked against the geometry it stands for (the copper distance at the packed position).

use topor_engine::geom::Vec2;
use topor_engine::model::{BoardModel, Options};
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::init::InitWeights;
use topor_engine::topo::pack::{disc_margin, lean_dir, pair_req, signed_height, PackItem, Packer};
use topor_engine::topo::refine::CapModel;
use topor_engine::topo::state::{State, VertKind};

const MM: f64 = 1e6;
const GAP: f64 = 0.2 * MM;
const D: f64 = 0.6 * MM;
const W: f64 = 0.25 * MM;

/// A track of width `W` crossing at sin θ = `sin`, leaning `lean` (`lean^2 + sin^2 = 1`).
fn track(net: i32, sin: f64, lean: f64) -> PackItem {
    PackItem { net, w: W, s: GAP, infl: 1.0 / sin, disc: [0.0; 3], lean }
}

/// A disc-only item (its wire on another layer here) at height `h`.
fn disc_only(net: i32, h: f64) -> PackItem {
    PackItem { net, w: 0.0, s: GAP, infl: 1.0, disc: [D, D, h], lean: 0.0 }
}

/// Distance from the via centre `(0, h)` to the centre line of a track crossing the edge at `x`
/// in direction `(lean, sin)` (along, normal), minus the copper rule R: >= 0 when it keeps the gap.
fn slack_disc_then_track(h: f64, x: f64, lean: f64, sin: f64) -> f64 {
    let (px, py) = (-x, h);
    (lean * py - sin * px).abs() - (0.5 * (D + W) + GAP)
}

#[test]
fn at_height_zero_the_rule_is_bit_equal_to_the_old_one() {
    let old = |a: &PackItem, b: &PackItem, gap: f64| {
        let (da, db) = (a.disc[1], b.disc[0]);
        let mut r = (0.5 * (a.w + b.w) + gap) * a.infl.max(b.infl);
        if da > 0.0 {
            r = r.max((0.5 * (da + b.w) + gap) * b.infl);
        }
        if db > 0.0 {
            r = r.max((0.5 * (a.w + db) + gap) * a.infl);
        }
        if da > 0.0 && db > 0.0 {
            r = r.max(0.5 * (da + db) + gap);
        }
        r
    };
    let t1 = track(1, 0.6, 0.8);
    let t2 = track(2, 0.9, -(1.0f64 - 0.81).sqrt());
    let v1 = PackItem { disc: [D, D, 0.0], ..t1 };
    let v2 = disc_only(2, 0.0);
    for (a, b) in [(t1, t2), (v1, t2), (t2, v1), (v1, v2), (v2, v1), (v2, t1), (t1, v2)] {
        assert_eq!(pair_req(&a, &b, GAP).to_bits(), old(&a, &b, GAP).to_bits(), "{a:?} {b:?}");
    }
}

#[test]
fn a_disc_against_a_perpendicular_track_keeps_its_room() {
    let b = track(2, 1.0, 0.0);
    for h in [0.0, 0.1 * MM, -0.3 * MM, 1.0 * MM] {
        let r = pair_req(&disc_only(1, h), &b, GAP);
        assert!((r - (0.5 * (D + W) + GAP)).abs() < 1e-6, "h {h}: {r}");
    }
}

#[test]
fn a_track_leaning_towards_the_disc_needs_more_and_away_less_clamped_at_zero() {
    let (sin, c) = (0.8, 0.6);
    let rr = 0.5 * (D + W) + GAP;
    let h = 0.3 * MM;
    // b is further along the edge (towards end 1); lean < 0 = towards end 0 = towards the disc.
    let toward = pair_req(&disc_only(1, h), &track(2, sin, -c), GAP);
    let away = pair_req(&disc_only(1, h), &track(2, sin, c), GAP);
    assert!((toward - (rr + h * c) / sin).abs() < 1e-6, "{toward}");
    assert!((away - (rr - h * c) / sin).abs() < 1e-6, "{away}");
    assert!(toward > rr / sin && away < rr / sin);
    // At the packed distance the copper is exactly at the rule (the geometry the term stands for).
    assert!(slack_disc_then_track(h, toward, -c, sin).abs() < 1e-3);
    assert!(slack_disc_then_track(h, away, c, sin).abs() < 1e-3);
    // The same on the other side of the edge: the lean flips with the half-plane.
    let below = pair_req(&disc_only(1, -h), &track(2, sin, c), GAP);
    assert!((below - toward).abs() < 1e-6, "{below} vs {toward}");
    // Leaning away from a via far enough off the line: the track passes it anywhere, 0.
    let far = pair_req(&disc_only(1, 2.0 * MM), &track(2, sin, c), GAP);
    assert_eq!(far, 0.0);
    // Disc on the far side (b) against a track a nearer end 0 leaning towards end 1: towards it.
    let vb = PackItem { disc: [D, D, h], ..disc_only(1, h) };
    let tb = pair_req(&track(2, sin, c), &vb, GAP);
    assert!((tb - (rr + h * c) / sin).abs() < 1e-6, "{tb}");
}

#[test]
fn two_discs_use_their_signed_height_difference() {
    let rr = D + GAP;
    let same = pair_req(&disc_only(1, 0.3 * MM), &disc_only(2, 0.2 * MM), GAP);
    let opp = pair_req(&disc_only(1, 0.3 * MM), &disc_only(2, -0.2 * MM), GAP);
    assert!((same - (rr * rr - (0.1 * MM).powi(2)).sqrt()).abs() < 1e-6, "{same}");
    assert!((opp - (rr * rr - (0.5 * MM).powi(2)).sqrt()).abs() < 1e-6, "{opp}");
    // Further apart across the line than the rule: no room needed along the edge.
    assert_eq!(pair_req(&disc_only(1, 0.5 * MM), &disc_only(2, -0.5 * MM), GAP), 0.0);
    // Continuous at height 0.
    let z = pair_req(&disc_only(1, 0.0), &disc_only(2, 0.0), GAP);
    let e = pair_req(&disc_only(1, 1e-6), &disc_only(2, 0.0), GAP);
    assert!((z - e).abs() < 1e-3, "{z} {e}");
}

#[test]
fn a_track_with_its_own_disc_keeps_its_track_terms() {
    // A track item with a via disc at a height still packs its track against the neighbour.
    let a = PackItem { disc: [D, D, 2.0 * MM], ..track(1, 1.0, 0.0) };
    let b = track(2, 1.0, 0.0);
    assert!((pair_req(&a, &b, GAP) - (W + GAP)).abs() < 1e-6);
}

fn board() -> BoardModel {
    let p = |v: f64| (v * MM).round() as i64;
    let c = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads: Vec<String> = [(1, 3.0, 5.0), (2, 7.0, 5.0), (1, 5.0, 2.0), (2, 5.0, 8.0)]
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y))| {
            format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"smd","center":[{},{}],
                    "rotation_deg":0.0,"layers":[0],"shapes":{{"0":{{"kind":"rect","w":1000000,"h":1000000}}}},"drill":null}}"#,
                p(x),
                p(y)
            )
        })
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}}],
            "pads":[{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

/// End margins at an offset centre, exact against the obstacle: an edge leaving a straight pad side
/// at a right angle keeps its margin for a centre moved along that side; an edge leaving a round
/// pad radially gets `sqrt((r + m0)^2 - h^2) - r`.
#[test]
fn the_end_margin_is_tested_at_the_offset_centre() {
    let m = board();
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let pr0 = &b.proj;
    // The longest edge with an obstacle vertex at end 0.
    let e = (0..pr0.edges.len())
        .filter(|&e| matches!(pr0.vkind[pr0.edges[e][0]], VertKind::Outline(_)))
        .max_by(|&x, &y| {
            let l = |e: usize| (pr0.pos[pr0.edges[e][1]] - pr0.pos[pr0.edges[e][0]]).len();
            l(x).total_cmp(&l(y))
        })
        .expect("an obstacle edge");
    let [v, u] = pr0.edges[e];
    let VertKind::Outline(o) = pr0.vkind[v] else { unreachable!() };
    let (pv, pu) = (pr0.pos[v], pr0.pos[u]);
    let len = (pu - pv).len();
    assert!(len > 2.0 * MM, "edge {len}");
    let ed = (pu - pv) * (1.0 / len);
    let nrm = ed.perp();
    let net = 99;
    let hs = [0.15 * MM, -0.15 * MM, 0.3 * MM];
    // Straight side through v, square to the edge, 4 mm long, the pad behind it.
    let mut pr = pr0.clone();
    pr.obstacles[o].poly = vec![pv + nrm * (2.0 * MM) - ed * MM, pv - nrm * (2.0 * MM) - ed * MM, pv - nrm * (2.0 * MM), pv + nrm * (2.0 * MM)];
    pr.obstacles[o].net = Some(1);
    let pk = Packer::new(&pr, Vec::new(), 0.0, 0.0);
    let m0 = pk.margin_all(e, 0, net, D, GAP);
    assert!(m0 > 0.3 * MM, "{m0}");
    for &h in &hs {
        let mh = pk.margin_all_h(e, 0, net, D, GAP, h);
        assert!((mh - m0).abs() < 1.0, "straight side h {h}: {mh} vs {m0}");
    }
    // Round pad of radius 1 mm touching v, the edge leaving it radially.
    let r = 1.0 * MM;
    let c = pv - ed * r;
    let n = 2048;
    let mut pr = pr0.clone();
    pr.obstacles[o].poly = (0..n).map(|i| {
        let a = std::f64::consts::TAU * i as f64 / n as f64;
        c + Vec2 { x: a.cos(), y: a.sin() } * r
    }).collect();
    pr.obstacles[o].net = Some(1);
    let pk = Packer::new(&pr, Vec::new(), 0.0, 0.0);
    let m0 = pk.margin_all(e, 0, net, D, GAP);
    for &h in &hs {
        let mh = pk.margin_all_h(e, 0, net, D, GAP, h);
        let want = ((r + m0).powi(2) - h * h).sqrt() - r;
        assert!(mh < m0, "round pad h {h}: {mh} vs {m0}");
        assert!((mh - want).abs() < 100.0, "round pad h {h}: {mh} vs {want}");
    }
    // Clear of the pad at the edge's start already: 0.
    assert_eq!(pk.margin_all_h(e, 0, net, D, GAP, 1.5 * MM), 0.0);
    // A disc alone (its wire on another layer) that far off the line has no end margin left: no
    // track margin of a zero-width track on the line either.
    let m_line = pk.margin(e, 0, net, 0.0, GAP, 0);
    assert!(m_line > 0.0);
    let far = PackItem { net, ..disc_only(net, 1.5 * MM) };
    assert_eq!(disc_margin(&pk, e, 0, &far, 0, m_line), 0.0);
    // A track with its own disc keeps its track margin.
    let tr = PackItem { net, disc: [D, D, 1.5 * MM], ..track(net, 1.0, 0.0) };
    assert_eq!(disc_margin(&pk, e, 0, &tr, 0, m_line), m_line);
    // At height 0 the disc's margin is the one on the line, whatever the item.
    let on = PackItem { net, ..disc_only(net, 0.0) };
    assert_eq!(disc_margin(&pk, e, 0, &on, 0, m_line).to_bits(), m_line.max(pk.margin_all(e, 0, net, D, GAP)).to_bits());
}

/// The capacity model writes each disc's height from its via's planned point: on the wire's two
/// passes round the via, the point's signed height off that pass's edge (the nearer of two vias),
/// and 0 everywhere with the option off. Two through-hole walls with a door, a wire through it
/// changing layer after the door (the s3c door case).
#[test]
fn the_capacity_model_writes_the_heights_of_the_planned_via_points() {
    let p = |v: f64| (v * MM) as i64;
    let c = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads: Vec<String> = [(7, 2.0, 5.0, 5.0), (8, 8.0, 5.0, 5.0), (1, 4.6, 1.0, 0.4), (1, 4.6, 9.0, 0.4), (2, 5.4, 1.0, 0.4), (2, 5.4, 9.0, 0.4)]
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y, d))| {
            let sh = format!(r#"{{"kind":"rect","w":{0},"h":{0}}}"#, p(d));
            format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"pth",
                    "center":[{x},{y}],"rotation_deg":0.0,"layers":[0, 2],
                    "shapes":{{"0":{sh},"2":{sh}}},"drill":{{"w":{dr},"h":{dr}}}}}"#,
                x = p(x),
                y = p(y),
                dr = p(d / 2.0)
            )
        })
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}}],
            "pads":[{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
        pads.join(","),
        segs.join(",")
    );
    let m = BoardModel::from_json(&s).unwrap();
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let conns = b.connections(&m);
    let mut st = State::new(&b.proj);
    for c in &conns {
        st.route_conn(c, &InitWeights::default(), (c.w + c.s) / 400_000.0).expect("routes");
    }
    let n = st.wires[0].passes.len();
    assert!(n >= 4, "{n} passes");
    let evl = vec![vec![(0, 0.5, 0), (n / 2, 0.5, 0), (n, 0.5, 1)], vec![(0, 0.5, 0)]];
    let mut nonzero = 0;
    for on in [false, true] {
        let o = Options { via_disc_height: on, ..Options::default() };
        let packer = Packer::for_model(&m, &o, &b.proj);
        let dirs = packer.taut_dirs(&st);
        let cm = CapModel::new(&st, &packer, &dirs, &evl, 1, 50.0);
        assert_eq!(cm.via_check().0, 0, "stored = fresh (discs, heights, leans, prices, packer over)");
        let pts = cm.planned_via_points(0);
        let ks = cm.via_k[0].clone();
        assert_eq!(pts.len(), ks.len());
        assert!(!ks.is_empty(), "a via");
        for (j, &pid) in st.wires[0].passes.iter().enumerate() {
            let d = cm.pass_disc[pid];
            let e = st.passes[pid].edge;
            let near = ks
                .iter()
                .zip(&pts)
                .filter(|(&k, _)| j + 1 == k || j == k)
                .filter_map(|(_, &q)| q)
                .map(|q| signed_height(&b.proj, e, q))
                .min_by(|x, y| x.abs().total_cmp(&y.abs()));
            match (on, near) {
                (true, Some(h)) => {
                    assert_eq!(d[2].to_bits(), h.to_bits(), "pass {j}: {d:?} vs {h}");
                    nonzero += usize::from(h != 0.0);
                }
                _ => assert_eq!(d[2], 0.0, "pass {j} (on {on}): {d:?}"),
            }
        }
    }
    assert!(nonzero >= 1, "a via off its edge lines");
}

#[test]
fn heights_and_leans_have_the_edge_frame_signs() {
    let m = board();
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let pr = &b.proj;
    let e = 0;
    let [p, q] = pr.edges[e];
    let (pp, pq) = (pr.pos[p], pr.pos[q]);
    let ed = (pq - pp) * (1.0 / (pq - pp).len());
    let mid = (pp + pq) * 0.5;
    assert!((signed_height(pr, e, mid + ed.perp() * MM) - MM).abs() < 1e-3);
    assert!((signed_height(pr, e, mid - ed.perp() * MM) + MM).abs() < 1e-3);
    // Lean: the along-edge part of the unit direction oriented into the left half-plane.
    let d = ed.perp() * 0.8 + ed * 0.6;
    assert!((lean_dir(pr, e, d * MM) - 0.6).abs() < 1e-9);
    assert!((lean_dir(pr, e, -(d * MM)) - 0.6).abs() < 1e-9);
    assert!(lean_dir(pr, e, ed.perp() * MM).abs() < 1e-9);
    assert_eq!(lean_dir(pr, e, Vec2::default()), 0.0);
}

#[test]
fn the_disc_height_option_is_opt_in() {
    assert!(!Options::default().via_disc_height);
    assert!(Options::from_json(r#"{"via_disc_height": true}"#).expect("options parse").via_disc_height);
}
