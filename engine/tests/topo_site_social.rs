//! TW6 part B, the search term (`[factors] via_site_social`, docs/dev/design/tw6-via-sites.md B):
//! a crossing of another net's stretch costs weight x min(own site amount, crossed wire's site
//! amount); 0 where either wire has a via site, so the path round (0 vias, longer) competes with
//! a crossing that predicts a via without a site (Polubasov 2010 «Перекладка проводников», fig. 4).

use topor_engine::geom::Vec2;
use topor_engine::model::{BoardModel, Options};
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::init::{Conn, InitWeights};
use topor_engine::topo::pack::Packer;
use topor_engine::topo::state::{point_in_poly, State};

const MM: f64 = 1e6;

/// Two-layer board 40 x 40 mm (KiCad frame): net A (1) pads at (12, 20) and (28, 20), net B (2)
/// pads at (20, 3) and (20, 37); with `ko`, a no-via keepout over (14, 14) .. (26, 26).
fn board(ko: bool) -> BoardModel {
    let p = |v: f64| (v * MM) as i64;
    let c = [(0.0, 0.0), (40.0, 0.0), (40.0, 40.0), (0.0, 40.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads: Vec<String> = [("A1", 1, 12.0, 20.0), ("A2", 1, 28.0, 20.0), ("B1", 2, 20.0, 3.0), ("B2", 2, 20.0, 37.0)]
        .iter()
        .map(|&(r, net, x, y)| {
            format!(
                r#"{{"uuid":"{r}","ref":"{r}","number":"1","net":{net},"attr":"smd","center":[{},{}],"rotation_deg":0.0,
                    "layers":[0],"shapes":{{"0":{{"kind":"rect","w":{},"h":{}}}}}}}"#,
                p(x),
                p(y),
                p(1.0),
                p(1.0)
            )
        })
        .collect();
    let poly = format!("[[{},{}],[{},{}],[{},{}],[{},{}]]", p(14.0), p(14.0), p(26.0), p(14.0), p(26.0), p(26.0), p(14.0), p(26.0));
    let extra = if ko { format!(r#","keepouts":[{{"name":"k","layers":[0,2],"poly":{poly},"no_tracks":false,"no_vias":true}}]"#) } else { String::new() };
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}}],
            "pads":[{}],"outline":[{}],
            "rules":{{"default":{{"clearance":200000,"width":250000}},"min_connection":127000,"max_error":5000,"per_net":{{}}}}{extra}}}"#,
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn opts() -> BuildOpts {
    BuildOpts { existing_vias: true, keepouts: true, copper_shapes: true, ..BuildOpts::default() }
}

/// Route B, then A with `site_mm` (the `via_site_social` price per unit), the state holding the
/// packer's site rooms; returns A's different-net crossings, the largest min(own, crossed) site
/// amount over them (0: every crossing has a via site on one side) and A's traversal count.
fn route_a(ko: bool, site_mm: f64) -> (usize, f64, usize) {
    route_a_with(ko, site_mm, false)
}

/// [`route_a`]; `b_via`: B carries a layering with one via interval over its whole wire (its via
/// has a site, outside the keepout), so a crossing of B is free of the term wherever it lies.
fn route_a_with(ko: bool, site_mm: f64, b_via: bool) -> (usize, f64, usize) {
    let m = board(ko);
    let b = board::build(&m, &opts()).unwrap();
    let pk = Packer::for_model(&m, &Options::default(), &b.proj);
    assert!(pk.via_d(1) > 0.0 && pk.via_d(2) > 0.0, "the board needs via sizes");
    let mut st = State::new(&b.proj);
    st.site_rooms = Some(pk.site_rooms());
    let ob = |r: &str| m.pads.iter().position(|p| p.ref_ == r).map(|i| b.pad_of.iter().position(|&x| x == i).unwrap()).unwrap();
    let conn = |net: i32, a: &str, z: &str| {
        let (pa, pb) = (ob(a), ob(z));
        Conn { net, pa, pb, a: b.centre[pa].unwrap(), b: b.centre[pb].unwrap(), w: 0.25 * MM, s: 0.2 * MM }
    };
    let (wb, _) = st.route_conn(&conn(2, "B1", "B2"), &InitWeights::default(), 1.0).expect("B routes");
    if b_via {
        let n = st.wires[wb].tris.len();
        let mut ev = vec![Vec::new(); st.wires.len()];
        ev[wb] = vec![(0, 0.0, 0), (n - 1, 1.0, 1)];
        st.mark_site_k(&ev);
    }
    let wt = InitWeights { site_mm, ..InitWeights::default() };
    let (wa, _) = st.route_conn(&conn(1, "A1", "A2"), &wt, 1.0).expect("A routes");
    let xs: Vec<_> = st.crossings(wa).into_iter().filter(|x| st.wires[x.other].net != 1).collect();
    let worst = xs.iter().map(|x| st.site_own(x.tri, 1, 0.2 * MM).min(st.site_k_at(x.other, x.other_k))).fold(0.0, f64::max);
    (xs.len(), worst, st.wires[wa].tris.len())
}

/// The site amount reads the packer's one measure: 2 (1 + the capped deficit d / (d + s) ~ 1)
/// inside a no-via keepout, 0 where a via fits; a state without site rooms prices nothing.
#[test]
fn site_amounts_read_the_packer_room() {
    let m = board(true);
    let b = board::build(&m, &opts()).unwrap();
    let pk = Packer::for_model(&m, &Options::default(), &b.proj);
    let tri_at = |x: f64, y: f64| {
        let p = Vec2 { x: x * MM, y: -y * MM };
        (0..b.proj.tris.len()).find(|&t| point_in_poly(p, &b.proj.tris[t].map(|v| b.proj.pos[v]))).unwrap()
    };
    let (inside, outside) = (tri_at(20.0, 20.0), tri_at(5.0, 35.0));
    let mut st = State::new(&b.proj);
    assert_eq!(st.site_own(inside, 1, 0.2 * MM), 0.0, "no site rooms: off");
    st.site_rooms = Some(pk.site_rooms());
    let a_in = st.site_own(inside, 1, 0.2 * MM);
    let d = pk.via_d(1);
    assert!((a_in - (1.0 + d / (d + 0.2 * MM))).abs() < 1e-9, "inside the keepout: {a_in}");
    assert_eq!(st.site_own(outside, 1, 0.2 * MM), 0.0);
    // The same value as the packer's own room through `Packer::site_amount`.
    assert_eq!(a_in, Packer::site_amount(pk.site_room(inside, 1, 0.2 * MM), d, 0.2 * MM));
}

/// Where neither wire has a via site at the crossing (a no-via keepout over the middle), the
/// price moves the crossing to where a via fits (out of the keepout: a short detour, still one
/// crossing); with the price 0 the search crosses in the keepout as before; with via room at the
/// crossing (no keepout) the term is 0 and the path stays the off path.
#[test]
fn a_crossing_without_sites_moves_to_a_site() {
    let (x0, a0, n0) = route_a(true, 0.0);
    assert_eq!(x0, 1, "off: A crosses B once");
    assert!(a0 > 1.0, "off: the crossing lies in the keepout (amount {a0})");
    let (x1, a1, n1) = route_a(true, 100.0);
    assert_eq!(x1, 1, "on: A still crosses B once (going round costs more)");
    assert_eq!(a1, 0.0, "on: the crossing has a via site on one side");
    assert_ne!(n1, n0, "on: another path");
    // Room at the crossing: the term is 0, the same path as off.
    let (x2, a2, n2) = route_a(false, 100.0);
    let (_, _, n3) = route_a(false, 0.0);
    assert_eq!((x2, a2, n2), (1, 0.0, n3), "no keepout: the off path");
}

/// The smaller of the two amounts: where the crossed wire's via has a site (its interval reaches
/// out of the keepout), the crossing costs nothing more even where the crossing wire has none, so
/// the path is the off path.
#[test]
fn the_term_takes_the_smaller_amount() {
    let (_, _, n0) = route_a(true, 0.0);
    let (x, _, n) = route_a_with(true, 100.0, true);
    assert_eq!((x, n), (1, n0), "B's via has a site: A keeps the off path");
}

/// The crossed wire's amount comes from its via interval under the layering: inside an interval
/// the best room over the interval (the via may sit anywhere in it), elsewhere the traversal's own
/// room; no layering events = the own room everywhere.
#[test]
fn crossed_wire_amount_takes_its_via_interval() {
    let m = board(true);
    let b = board::build(&m, &opts()).unwrap();
    let pk = Packer::for_model(&m, &Options::default(), &b.proj);
    let mut st = State::new(&b.proj);
    st.site_rooms = Some(pk.site_rooms());
    let ob = |r: &str| m.pads.iter().position(|p| p.ref_ == r).map(|i| b.pad_of.iter().position(|&x| x == i).unwrap()).unwrap();
    let (pa, pb) = (ob("B1"), ob("B2"));
    let c = Conn { net: 2, pa, pb, a: b.centre[pa].unwrap(), b: b.centre[pb].unwrap(), w: 0.25 * MM, s: 0.2 * MM };
    let (w, _) = st.route_conn(&c, &InitWeights::default(), 1.0).expect("B routes");
    let n = st.wires[w].tris.len();
    let own: Vec<f64> = (0..n).map(|k| st.site_k_at(w, k)).collect();
    let mid = (0..n).max_by(|&i, &j| own[i].total_cmp(&own[j])).unwrap();
    assert!(own[mid] > 1.0, "B runs through the keepout: {own:?}");
    assert!(own.iter().any(|&a| a == 0.0), "and has sites outside it: {own:?}");
    // A layering with one via interval over the whole wire: every traversal takes the best room.
    let mut ev = vec![Vec::new(); st.wires.len()];
    ev[w] = vec![(0, 0.0, 0), (n - 1, 1.0, 1)];
    st.site_strict = false;
    st.mark_site_k(&ev);
    assert!((0..n).all(|k| st.site_k_at(w, k) == 0.0), "the interval has a site: {:?}", (0..n).map(|k| st.site_k_at(w, k)).collect::<Vec<_>>());
    // No layer change: each traversal its own room again.
    ev[w] = vec![(0, 0.0, 0), (n - 1, 1.0, 0)];
    st.mark_site_k(&ev);
    assert_eq!((0..n).map(|k| st.site_k_at(w, k)).collect::<Vec<_>>(), own);
}

/// Opt-in: the factor's default weight is 0, so the search weight is 0.
#[test]
fn via_site_social_is_opt_in() {
    assert_eq!(InitWeights::default().site_mm, 0.0);
}
