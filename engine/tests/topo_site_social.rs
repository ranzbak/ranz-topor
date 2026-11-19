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

// ---------------------------------------------------------------- part E (site_move, branch_move)

use topor_engine::topo::viaopt::{bounding_wires, siteless_intervals, tee_arms};

/// B routed straight through the keepout (net 2, x = 20 mm from y = 3 to 37), then A across it:
/// the state, the packer (via intervals strict, the default), B's wire and A's.
fn with_b<R>(f: impl FnOnce(&State, &Packer, usize, usize) -> R) -> R {
    let m = board(true);
    let b = board::build(&m, &opts()).unwrap();
    let pk = Packer::for_model(&m, &Options::default(), &b.proj);
    let mut st = State::new(&b.proj);
    let ob = |r: &str| m.pads.iter().position(|p| p.ref_ == r).map(|i| b.pad_of.iter().position(|&x| x == i).unwrap()).unwrap();
    let (pa, pb) = (ob("B1"), ob("B2"));
    let c = Conn { net: 2, pa, pb, a: b.centre[pa].unwrap(), b: b.centre[pb].unwrap(), w: 0.25 * MM, s: 0.2 * MM };
    let (w, _) = st.route_conn(&c, &InitWeights::default(), 1.0).expect("B routes");
    let (qa, qb) = (ob("A1"), ob("A2"));
    let ca = Conn { net: 1, pa: qa, pb: qb, a: b.centre[qa].unwrap(), b: b.centre[qb].unwrap(), w: 0.25 * MM, s: 0.2 * MM };
    let (wa, _) = st.route_conn(&ca, &InitWeights::default(), 1.0).expect("A routes");
    f(&st, &pk, w, wa)
}

/// A layer change whose interval lies inside the keepout has no site (amount > 1, its traversals
/// reported); one whose interval reaches out of it has one (not listed); no layer change, no entry;
/// the largest amount comes first.
#[test]
fn siteless_intervals_read_the_room_of_the_whole_interval() {
    with_b(|st, pk, w, _| {
        let n = st.wires[w].tris.len();
        let wi = &st.wires[w];
        let no_site = |k: usize| Packer::site_amount(pk.site_room(wi.tris[k], wi.net, wi.s), pk.via_d(wi.net), wi.s) > 0.0;
        // The run of site-less traversals round the middle (y = 20 mm, inside the keepout).
        let mid = n / 2;
        assert!(no_site(mid), "B's middle traversal has no site");
        let (mut k0, mut k1) = (mid, mid);
        while k0 > 1 && no_site(k0 - 1) {
            k0 -= 1;
        }
        while k1 + 2 < n && no_site(k1 + 1) {
            k1 += 1;
        }
        assert!(k1 >= k0 + 2 && k0 > 1 && k1 + 2 < n, "a site-less run inside the keepout: {k0}..={k1} of {n}");
        // Events at the first and last site-less traversal, strict: the interval between them.
        let ev = vec![(0, 0.0, 0), (k0, 0.5, 0), (k1, 0.5, 1), (n - 1, 1.0, 1)];
        let got = siteless_intervals(st, pk, w, &ev);
        assert_eq!(got.len(), 1, "{got:?}");
        let (i, lo, hi, a) = got[0];
        assert_eq!((i, lo, hi), (1, k0 + 1, k1 - 1));
        assert!(a > 1.0, "amount {a}");
        // The interval from the start: it reaches pad B1's surroundings outside the keepout.
        let ev2 = vec![(0, 0.0, 0), (k1, 0.5, 1), (n - 1, 1.0, 1)];
        assert!(siteless_intervals(st, pk, w, &ev2).is_empty());
        assert!(siteless_intervals(st, pk, w, &[(0, 0.0, 0), (n - 1, 1.0, 0)]).is_empty());
        // Two site-less intervals: the larger amount first, ties by event order.
        let ev3 = vec![(0, 0.0, 0), (k0, 0.5, 0), (k1, 0.5, 1), (k1, 0.6, 1), (k1, 0.7, 0), (n - 1, 1.0, 0)];
        let got3 = siteless_intervals(st, pk, w, &ev3);
        assert!(got3.windows(2).all(|x| x[0].3 > x[1].3 || (x[0].3 == x[1].3 && x[0].0 < x[1].0)), "{got3:?}");
    });
}

/// The bounding wires are the crossing partners of the interval's two events: other nets only,
/// alive, with a connection, each once.
#[test]
fn bounding_wires_are_the_interval_partners() {
    with_b(|st, _, w, wa| {
        let all = |_: usize| true;
        // B's events 1 and 2 crossed by A, event 3 by B itself (same net: never).
        let p = vec![None, Some((wa, 0)), Some((wa, 1)), Some((w, 2))];
        assert_eq!(bounding_wires(st, w, &p, 1, &all), vec![wa], "A once");
        assert_eq!(bounding_wires(st, w, &p, 2, &all), vec![wa], "same net left out");
        assert!(bounding_wires(st, w, &p, 1, &|x| x != wa).is_empty(), "no connection");
        assert!(bounding_wires(st, w, &[], 0, &all).is_empty());
    });
}

/// A second wire of the crossed wire's own net is never a bounding wire (its crossing is
/// electrically harmless and lays no via).
#[test]
fn bounding_wires_leave_out_the_same_net() {
    let m = board(true);
    let b = board::build(&m, &opts()).unwrap();
    let mut st = State::new(&b.proj);
    let ob = |r: &str| m.pads.iter().position(|p| p.ref_ == r).map(|i| b.pad_of.iter().position(|&x| x == i).unwrap()).unwrap();
    let (pa, pb) = (ob("B1"), ob("B2"));
    let c = Conn { net: 2, pa, pb, a: b.centre[pa].unwrap(), b: b.centre[pb].unwrap(), w: 0.25 * MM, s: 0.2 * MM };
    let (w1, _) = st.route_conn(&c, &InitWeights::default(), 1.0).expect("B routes");
    let (w2, _) = st.route_conn(&c, &InitWeights::default(), 1.0).expect("B routes again");
    let p = vec![None, Some((w2, 0)), Some((w2, 1))];
    assert!(bounding_wires(&st, w1, &p, 1, &|_| true).is_empty(), "same net");
}

/// The tee arms of an interval: a branch ending on a turn of the wire inside it, or the wire itself
/// when its own junction end lies in it; outside the interval or without a connection, none.
#[test]
fn tee_arms_are_the_junctions_inside_the_interval() {
    with_b(|st, _, w, _| {
        let n = st.wires[w].tris.len();
        let all = |_: usize| true;
        let none = |_: usize| false;
        // (branch, end, trunk, traversal, vertex): branch 7 on B's traversal 5, B the branch of
        // trunk 9 at its end 1.
        let j = vec![(7, 0, w, 5, 0), (w, 1, 9, 2, 0)];
        assert_eq!(tee_arms(st, &j, w, 4, 6, &all), vec![7]);
        assert!(tee_arms(st, &j, w, 6, 8, &all).is_empty());
        let mut both = vec![7, w];
        both.sort_unstable();
        assert_eq!(tee_arms(st, &j, w, 4, n - 1, &all), both);
        assert!(tee_arms(st, &j, w, 4, 6, &none).is_empty());
        let j0 = vec![(w, 0, 9, 2, 0)];
        assert_eq!(tee_arms(st, &j0, w, 0, 3, &all), vec![w]);
        assert!(tee_arms(st, &j0, w, 1, 3, &all).is_empty());
    });
}

#[test]
fn site_and_branch_moves_are_opt_in_and_parse() {
    let d = Options::default();
    assert!(!d.opt_site_move && !d.opt_branch_move);
    let o = Options::from_json(r#"{"opt_site_move": true, "opt_branch_move": true}"#).expect("options parse");
    assert!(o.opt_site_move && o.opt_branch_move);
}

/// The report line names exactly the moves that are on, with their counts.
#[test]
fn site_move_line_names_the_moves_on() {
    use topor_engine::topo::core::site_move_line;
    use topor_engine::topo::optimise::CycleStats;
    let s = CycleStats { site_moved: 7, site_kept: 2, branch_moved: 5, branch_kept: 1, ..CycleStats::default() };
    let a = site_move_line(true, false, &s);
    assert!(a.contains("2 of 7 site moves kept") && a.contains("site_move") && !a.contains("branch"), "{a}");
    let b = site_move_line(false, true, &s);
    assert!(b.contains("1 of 5 branch moves kept") && b.contains("branch_move") && !b.contains("site moves"), "{b}");
    let c = site_move_line(true, true, &s);
    let (i, j) = (c.find("2 of 7 site moves kept").expect("site part"), c.find("; 1 of 5 branch moves kept").expect("branch part"));
    assert!(i < j, "{c}");
}

fn qmtech_crop() -> BoardModel {
    let path = format!("{}/tests/fixtures/qmtech-usb-crop.json", env!("CARGO_MANIFEST_DIR"));
    BoardModel::from_json(&std::fs::read_to_string(path).expect("fixture")).expect("fixture parses")
}

/// Off = the default copper: the search term at weight 0 and both moves false give the routed
/// items of the options absent; with all on (and the site price) the run completes and reports
/// the moves.
#[test]
fn off_is_the_default_copper_and_on_reports() {
    use topor_engine::control::RunControl;
    use topor_engine::route::route_with;
    let m = qmtech_crop();
    let base = r#""wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 1, "via_cuts": true"#;
    let run = |extra: &str| {
        let o = Options::from_json(&format!("{{{base}{extra}}}")).expect("options parse");
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        route_with(&m, &o, &ctl)
    };
    let items = |r: &topor_engine::route::RouteResult| format!("{:?}", r.items);
    let r0 = run("");
    assert_eq!(items(&r0), items(&run(r#", "opt_site_move": false, "opt_branch_move": false, "factors": {"weight": {"via_site_social": 0.0}}"#)));
    assert!(!r0.warnings.iter().any(|w| w.contains("[optimize] site_move") || w.contains("[optimize] branch_move")));
    let r1 = run(r#", "via_site_price": "graded", "opt_site_move": true, "opt_branch_move": true, "factors": {"weight": {"via_site_social": 50.0}}"#);
    assert!(r1.warnings.iter().any(|w| w.contains("site moves kept") && w.contains("branch moves kept")), "{:?}", r1.warnings);
    // The crop has a via without a site in the loop: the site move runs (1 of 2 kept at the time
    // of writing; the count, not the outcome, is asserted).
    let line = r1.warnings.iter().find(|w| w.contains("site moves kept")).expect("line");
    let tried: usize = line.split(" site moves kept").next().and_then(|a| a.rsplit(" of ").next()).and_then(|n| n.trim().parse().ok()).expect("count");
    assert!(tried > 0, "{line}");
}
