use std::collections::HashSet;
use std::time::{Duration, Instant};
use topor_engine::decompose::{order, Connection, TermKind, TermRef};
use topor_engine::graph::{CrossEntry, GraphRules};
use topor_engine::model::{BoardModel, Options};
use topor_engine::multigraph::MultiGraph;
use topor_engine::negotiate::*;
use topor_engine::obstacle::ObstacleSet;
use topor_engine::search::{Goal, SlotPath};
use topor_engine::vias::{SiteOrigin, ViaSite};

const MM: f64 = 1_000_000.0;

fn seg(a: (i64, i64), b: (i64, i64)) -> String {
    format!(
        r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
        a.0 * 1_000_000,
        a.1 * 1_000_000,
        b.0 * 1_000_000,
        b.1 * 1_000_000
    )
}

fn outline50x40() -> String {
    [
        ((0, 0), (50, 0)),
        ((50, 0), (50, 40)),
        ((50, 40), (0, 40)),
        ((0, 40), (0, 0)),
    ]
    .iter()
    .map(|&(a, b)| seg(a, b))
    .collect::<Vec<_>>()
    .join(",")
}

fn round_pad(name: &str, net: i32, x_mm: f64, y_mm: f64) -> String {
    format!(
        r#"{{ "uuid": "{name}", "ref": "{name}", "number": "1", "net": {net},
          "center": [{}, {}], "layers": [0],
          "shapes": {{ "0": {{ "kind": "circle", "d": 1000000 }} }} }}"#,
        (x_mm * MM) as i64,
        (y_mm * MM) as i64
    )
}

/// One routable layer (F.Cu): the M2 negotiation tests run through the layered API on a
/// single-layer `MultiGraph` without via sites.
struct Built {
    mg: MultiGraph,
    obs: ObstacleSet,
}

type Verts = Vec<(usize, usize)>;

fn starts_of(v: &Verts) -> Vec<Vec<(usize, usize)>> {
    v.iter().map(|&(s, _)| vec![(0, s)]).collect()
}

fn goals_of(v: &Verts) -> Vec<Goal> {
    v.iter()
        .map(|&(_, t)| Goal {
            vertices: vec![(0, t)],
            pour_sites: Vec::new(),
            pour_layer: None,
        })
        .collect()
}

/// Crossings `(edge, slot)` of the single leg of `p` (layer 0).
fn xs(p: &SlotPath) -> Vec<(usize, usize)> {
    p.crossings_on(0)
}

fn small_pad(name: &str, net: i32, x_mm: f64, y_mm: f64) -> String {
    round_pad(name, net, x_mm, y_mm).replace("1000000 }", "300000 }")
}

fn build(pads: &[(&str, i32, f64, f64)], extra: &str) -> Built {
    let pads_json: Vec<String> = pads
        .iter()
        .map(|&(n, net, x, y)| {
            if n.starts_with('s') {
                small_pad(n, net, x, y)
            } else {
                round_pad(n, net, x, y)
            }
        })
        .collect();
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0 }}, {{ "id": 2 }} ], "pads": [ {} ], {extra} "outline": [ {} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#,
        pads_json.join(","),
        outline50x40()
    );
    let m = BoardModel::from_json(&s).unwrap();
    let mut obs = ObstacleSet::build(&m, &Options::default(), &HashSet::new(), &[]);
    let rules = GraphRules {
        eps: 5_000.0,
        min_clearance: 0.0,
        edge_clearance: 500_000.0,
        hole_clearance: 250_000.0,
    };
    let mg = MultiGraph::build(&m, &mut obs, Vec::new(), &[0], &[], &rules).unwrap();
    Built { mg, obs }
}

fn vert(b: &Built, name: &str) -> usize {
    let label = format!("pad:{name}:1");
    let ai = b
        .obs
        .layer(0)
        .anchors
        .iter()
        .position(|a| a.label == label)
        .unwrap();
    b.mg.layers[0].ls.cdt.vertex_index(ai)
}

fn mkconn(b: &Built, id: usize, net: i32, from: &str, to: &str) -> (Connection, (usize, usize)) {
    let (s, t) = (vert(b, from), vert(b, to));
    let term = |v: usize, l: &str| TermRef {
        kind: TermKind::Pad(0),
        label: l.to_string(),
        at: b.mg.layers[0].g.verts[v].at,
        layers: vec![0],
    };
    let g = &b.mg.layers[0].g;
    let est = (g.verts[s].at - g.verts[t].at).len();
    (
        Connection {
            id,
            net,
            width: 250_000,
            clearance: 200_000,
            tier: 3,
            from: term(s, from),
            to: term(t, to),
            est_len: est,
        },
        (s, t),
    )
}

fn setup(b: &Built, specs: &[(i32, &str, &str)]) -> (Vec<Connection>, Vec<(usize, usize)>) {
    specs
        .iter()
        .enumerate()
        .map(|(i, &(n, f, t))| mkconn(b, i, n, f, t))
        .unzip()
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

fn run(
    b: &mut Built,
    conns: &[Connection],
    verts: &[(usize, usize)],
) -> (Negotiator, u32, Vec<Option<SlotPath>>, usize) {
    let (neg, over, last, fail, _) = run_first(b, conns, verts);
    (neg, over, last, fail)
}

type FirstRun = (
    Negotiator,
    u32,
    Vec<Option<SlotPath>>,
    usize,
    Vec<Option<SlotPath>>,
);

/// `run` plus the paths of iteration 1 (`IterOutcome.paths`).
fn run_first(b: &mut Built, conns: &[Connection], verts: &[(usize, usize)]) -> FirstRun {
    let mut first: Vec<Option<SlotPath>> = Vec::new();
    let mut neg = Negotiator::new(&b.mg, conns.len());
    let opts = Options::default();
    let (starts, goals) = (starts_of(&verts.to_vec()), goals_of(&verts.to_vec()));
    let mut active: Vec<usize> = (0..conns.len()).collect();
    let mut last = Vec::new();
    for _ in 0..opts.pathfinder_iter_cap {
        let out = neg.iteration(
            &mut b.mg,
            &mut b.obs,
            conns,
            &active,
            &starts,
            &goals,
            &opts,
            far(),
        );
        let over = neg.penalise_overuse(&b.mg);
        for (&c, bi) in &out.blocked {
            neg.penalise_blocked(c, bi);
        }
        last = out.paths.clone();
        if first.is_empty() {
            first = out.paths.clone();
        }
        if over == 0 && out.blocked.is_empty() && out.ripped.is_empty() {
            return (neg, out.overuse_edges as u32, last, 0, first);
        }
        let mut a: std::collections::BTreeSet<usize> = out.blocked.keys().copied().collect();
        a.extend(out.ripped.iter().copied());
        a.extend(overused_conns(&b.mg));
        active = a.into_iter().collect();
    }
    (neg, 99, last, 1, first)
}

/// A wall of pads at x = 25 with a single 2 mm gap (capacity for one wire) at y = 19..21;
/// the wall spans y = 13.4..26.6, so the detours above / below cost ~40 % more.
fn channel_board() -> Built {
    build(
        &[
            ("sa", 1, 10.0, 19.6),
            ("sb", 2, 10.0, 20.4),
            ("ta", 1, 40.0, 19.6),
            ("tb", 2, 40.0, 20.4),
            ("u1", 3, 25.0, 19.0),
            ("u2", 3, 25.0, 16.46),
            ("u3", 3, 25.0, 13.92),
            ("w1", 3, 25.0, 21.0),
            ("w2", 3, 25.0, 23.54),
            ("w3", 3, 25.0, 26.08),
        ],
        "",
    )
}

/// A sealed wall of pads (pitch 1.4 mm, gaps too narrow for a wire) at x = 12, 2 mm from the starts (the overuse penalty scales with the leg length) from y = 0.8 up
/// to y = 36.4 with the single 2 mm channel at y = 19..21 and an open top (the detour costs
/// ~2x): in iteration 1 both wires take the channel and only the history switches one away.
fn tall_channel_board() -> Built {
    let mut pads: Vec<(String, i32, f64, f64)> = vec![
        ("sa".into(), 1, 10.0, 19.6),
        ("sb".into(), 2, 10.0, 20.4),
        ("ta".into(), 1, 40.0, 19.6),
        ("tb".into(), 2, 40.0, 20.4),
    ];
    for k in 0..14 {
        pads.push((format!("u{}", k + 1), 3, 12.0, 19.0 - 1.4 * k as f64));
    }
    for k in 0..12 {
        pads.push((format!("w{}", k + 1), 3, 12.0, 21.0 + 1.4 * k as f64));
    }
    let refs: Vec<(&str, i32, f64, f64)> = pads
        .iter()
        .map(|(n, net, x, y)| (n.as_str(), *net, *x, *y))
        .collect();
    build(&refs, "")
}

fn run_channel() -> (Vec<Option<SlotPath>>, usize, Vec<usize>) {
    let mut b = channel_board();
    let (conns, verts) = setup(&b, &[(1, "sa", "ta"), (2, "sb", "tb")]);
    let (_, over, paths, fail) = run(&mut b, &conns, &verts);
    assert_eq!(fail, 0, "did not converge in the iteration cap");
    assert_eq!(over, 0);
    let g = &b.mg.layers[0].g;
    let ch = [g.edge(vert(&b, "u1"), vert(&b, "w1")).unwrap()];
    let users: Vec<usize> = (0..2)
        .filter(|&c| {
            let k = b.mg.conn_wires[c][0];
            let (x, _, _) = g.path_of(k).unwrap();
            x.iter().any(|(e, _)| ch.contains(e))
        })
        .collect();
    assert_eq!(
        (0..g.edges.len()).filter(|&e| g.overuse(e) > 0.0).count(),
        0
    );
    (paths, users.len(), users)
}

#[test]
fn negotiate_contended_channel_routes_both() {
    let (p1, n1, _) = run_channel();
    assert!(p1.iter().all(|p| p.is_some()), "both Found");
    assert_eq!(n1, 1, "exactly one wire uses the channel");
    let (p2, _, _) = run_channel();
    let key = |p: &Vec<Option<SlotPath>>| -> Vec<Vec<(usize, usize)>> {
        p.iter().map(|x| xs(x.as_ref().unwrap())).collect()
    };
    assert_eq!(key(&p1), key(&p2));
}

/// D4: the history switch is exercised: in iteration 1 both wires take the channel (overuse),
/// the overuse raises `history[channel]`, and the wire that ends up detouring is one that used
/// the channel in iteration 1.
#[test]
fn negotiate_contended_channel_history_switch() {
    let mut b = tall_channel_board();
    let (conns, verts) = setup(&b, &[(1, "sa", "ta"), (2, "sb", "tb")]);
    let (neg, over, last, fail, first) = run_first(&mut b, &conns, &verts);
    assert_eq!((fail, over), (0, 0));
    let ch = b.mg.layers[0]
        .g
        .edge(vert(&b, "u1"), vert(&b, "w1"))
        .unwrap();
    let uses = |p: &Option<SlotPath>| xs(p.as_ref().unwrap()).iter().any(|&(e, _)| e == ch);
    assert!(
        first.iter().all(uses),
        "both wires take the channel in iteration 1"
    );
    assert!(neg.history[0][ch] > 0.0, "history {}", neg.history[0][ch]);
    let detour: Vec<usize> = (0..2).filter(|&c| !uses(&last[c])).collect();
    assert_eq!(detour.len(), 1, "exactly one wire switched to the detour");
    assert!(uses(&first[detour[0]]));
}

#[test]
fn blocked_connection_is_bumped_first_next_iteration() {
    let b = channel_board();
    let (conns, _) = setup(&b, &[(1, "sa", "ta"), (2, "sb", "tb")]);
    let mut neg = Negotiator::new(&b.mg, 2);
    assert_eq!(order(&conns, &neg.bumps)[0], 0);
    let info = BlockInfo {
        edge: None,
        site: None,
        ghost: None,
        last_reached: None,
        static_reason: None,
    };
    neg.penalise_blocked(1, &info);
    assert_eq!(neg.bumps[1], 1);
    assert!(neg.history.iter().flatten().all(|&h| h == 0.0));
    assert!(neg.site_history.iter().all(|&h| h == 0.0));
    assert_eq!(order(&conns, &neg.bumps)[0], 1);
}

#[test]
fn overuse_raises_history_on_overused_edges_only() {
    let mut b = channel_board();
    let (u1, w1) = (vert(&b, "u1"), vert(&b, "w1"));
    let e = b.mg.layers[0].g.edge(u1, w1).unwrap();
    let cap = b.mg.layers[0].g.edges[e].cap;
    let mut neg = Negotiator::new(&b.mg, 8);
    assert_eq!(neg.penalise_overuse(&b.mg), 0);
    let mut c = 0;
    while b.mg.layers[0].g.overuse(e) == 0.0 {
        b.mg.layers[0].g.insert(
            c,
            e,
            c,
            CrossEntry {
                conn: c,
                width: 250_000.0,
                clr: 200_000.0,
                k: None,
            },
        );
        c += 1;
        assert!(c < 8, "cap {cap}");
    }
    assert_eq!(neg.penalise_overuse(&b.mg), 1);
    for (i, &h) in neg.history[0].iter().enumerate() {
        assert_eq!(h, if i == e { HIST_OVERUSE } else { 0.0 });
    }
}

#[test]
fn u_clinch_detected_and_regular_absent() {
    let mut b = build(
        &[
            ("n1", 1, 20.0, 20.0),
            ("n2", 1, 30.0, 20.0),
            ("f1", 2, 25.0, 28.0),
            ("f2", 2, 25.0, 12.0),
        ],
        "",
    );
    // F first so that it crosses the N edge, then N detours.
    let (conns, verts) = setup(&b, &[(2, "f1", "f2"), (1, "n1", "n2")]);
    let e = b.mg.layers[0]
        .g
        .edge(verts[1].0, verts[1].1)
        .expect("N edge exists");
    let opts = Options::default();
    let (starts, goals) = (starts_of(&verts), goals_of(&verts));
    let mut neg = Negotiator::new(&b.mg, 2);
    let out = neg.iteration(
        &mut b.mg,
        &mut b.obs,
        &conns,
        &[0],
        &starts,
        &goals,
        &opts,
        far(),
    );
    assert!(out.paths[0].is_some());
    assert!(
        b.mg.layers[0].g.edges[e]
            .cross
            .iter()
            .any(|x| b.mg.conns_of_legs(&[x.conn]) == vec![0]),
        "F crosses the N edge"
    );
    let out = neg.iteration(
        &mut b.mg,
        &mut b.obs,
        &conns,
        &[1],
        &starts,
        &goals,
        &opts,
        far(),
    );
    assert!(!xs(out.paths[1].as_ref().unwrap()).is_empty(), "N detours");
    let rep = detect_clinches(&b.mg, &conns, &out.paths, &starts);
    assert_eq!(rep.u, vec![(0, e)]);
    assert!(rep.regular.is_empty());
}

#[test]
fn iteration_respects_deadline() {
    let mut b = channel_board();
    let (conns, verts) = setup(&b, &[(1, "sa", "ta"), (2, "sb", "tb")]);
    let mut neg = Negotiator::new(&b.mg, 2);
    let out = neg.iteration(
        &mut b.mg,
        &mut b.obs,
        &conns,
        &[0, 1],
        &starts_of(&verts),
        &goals_of(&verts),
        &Options::default(),
        Instant::now(),
    );
    assert!(out.timed_out);
    assert!(out.paths.iter().all(|p| p.is_none()));
}

/// Fix B: A (a1-a2, 10 mm) is routed first and cuts B's only short class (the b1-b2 CDT edge,
/// 5 mm); B's alternative around a1 is more than twice as long, so B's ghost path rips A. A then
/// takes the longer way round b1 and both converge, B on its short class.
#[test]
fn blocked_by_strand_rips_up_blocker() {
    let mut b = build(
        &[
            ("a1", 1, 20.0, 20.0),
            ("a2", 1, 30.0, 20.0),
            ("b1", 2, 25.0, 17.5),
            ("b2", 2, 25.0, 22.5),
        ],
        "",
    );
    let (mut conns, verts) = setup(&b, &[(1, "a1", "a2"), (2, "b1", "b2")]);
    conns[0].tier = 2; // A is routed first
    let (_, _, paths, fail) = run(&mut b, &conns, &verts);
    assert_eq!(fail, 0, "did not converge in the iteration cap");
    let pb = paths[1].as_ref().expect("B routed");
    assert!(
        pb.est_len < 6.0 * MM,
        "B keeps its short class: {}",
        pb.est_len
    );
    let pa = paths[0].as_ref().expect("A routed");
    assert!(pa.est_len > 10.0 * MM, "A detours: {}", pa.est_len);
    let g = &b.mg.layers[0].g;
    assert_eq!(
        (0..g.edges.len()).filter(|&e| g.overuse(e) > 0.0).count(),
        0
    );
}

// ---------------------------------------------------------------------------------------------
// Via sites (M3 / T3.1): site ownership in the negotiation
// ---------------------------------------------------------------------------------------------

fn site_at(x: f64, y: f64, d: i64, drill: i64) -> ViaSite {
    ViaSite {
        at: topor_engine::geom::Vec2 {
            x: x * MM,
            y: -y * MM,
        },
        max_d: d,
        max_drill: drill,
        origin: SiteOrigin::Face,
    }
}

/// F.Cu-only (`front`) or B.Cu-only SMD pad of net `net` at kicad `(x, y)` mm.
fn smd(name: &str, net: i32, x: f64, y: f64, front: bool) -> String {
    let (l, sh) = if front { (0, "0") } else { (31, "31") };
    format!(
        r#"{{ "uuid": "{name}", "ref": "{name}", "number": "1", "net": {net},
          "center": [{}, {}], "layers": [{l}],
          "shapes": {{ "{sh}": {{ "kind": "circle", "d": 1000000 }} }} }}"#,
        (x * MM) as i64,
        (y * MM) as i64
    )
}

struct Two {
    mg: MultiGraph,
    obs: ObstacleSet,
}

fn two_layers(pads: &[String], sites: Vec<ViaSite>) -> Two {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0 }}, {{ "id": 31 }} ], "pads": [ {} ],
          "outline": [ {} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000,
            "per_net": {{ "1": {{ "clearance": 200000, "width": 250000, "via_d": 400000,
                                  "via_drill": 250000 }} }} }} }}"#,
        pads.join(","),
        outline50x40()
    );
    let m = BoardModel::from_json(&s).unwrap();
    let mut obs = ObstacleSet::build(&m, &Options::default(), &HashSet::new(), &[]);
    let rules = GraphRules {
        eps: 5_000.0,
        min_clearance: 0.0,
        edge_clearance: 500_000.0,
        hole_clearance: 250_000.0,
    };
    let mg = MultiGraph::build(&m, &mut obs, sites, &[0, 31], &[], &rules).unwrap();
    Two { mg, obs }
}

fn vertex_on(t: &Two, li: usize, name: &str) -> usize {
    let label = format!("pad:{name}:1");
    let layer = t.mg.layers[li].layer;
    let ai = t
        .obs
        .layer(layer)
        .anchors
        .iter()
        .position(|a| a.label == label)
        .unwrap_or_else(|| panic!("no pad {name} on layer {layer}"));
    t.mg.layers[li].ls.cdt.vertex_index(ai)
}

fn contention_sites() -> Vec<ViaSite> {
    vec![
        site_at(25.0, 15.0, 600_000, 300_000),
        site_at(25.7, 15.0, 600_000, 300_000),
        site_at(25.0, 35.0, 400_000, 250_000),
    ]
}

fn contention_pads() -> Vec<String> {
    vec![
        smd("a1", 1, 5.0, 10.0, true),
        smd("a2", 1, 45.0, 10.0, false),
        smd("b1", 2, 5.0, 20.0, true),
        smd("b2", 2, 45.0, 20.0, false),
    ]
}

/// Negotiate the two connections of `site_rip_up_frees_owner_for_blocked_connection`; returns
/// the negotiator, the last paths, every connection ripped and whether it converged.
fn negotiate_contention(t: &mut Two) -> (Negotiator, Vec<Option<SlotPath>>, Vec<usize>, bool) {
    let term = |t: &Two, n: &str, li: usize, layer: u32| TermRef {
        kind: TermKind::Pad(0),
        label: n.to_string(),
        at: t.mg.layers[li].g.verts[vertex_on(t, li, n)].at,
        layers: vec![layer],
    };
    let mk = |t: &Two, id: usize, net: i32, f: &str, b: &str| {
        let (from, to) = (term(t, f, 0, 0), term(t, b, 1, 31));
        let est = (from.at - to.at).len();
        Connection {
            id,
            net,
            width: 250_000,
            clearance: 200_000,
            tier: 3,
            from,
            to,
            est_len: est,
        }
    };
    let conns = vec![mk(t, 0, 1, "a1", "a2"), mk(t, 1, 2, "b1", "b2")];
    let starts: Vec<Vec<(usize, usize)>> = vec![
        vec![(0, vertex_on(t, 0, "a1"))],
        vec![(0, vertex_on(t, 0, "b1"))],
    ];
    let goals: Vec<Goal> = ["a2", "b2"]
        .iter()
        .map(|n| Goal {
            vertices: vec![(1, vertex_on(t, 1, n))],
            pour_sites: Vec::new(),
            pour_layer: None,
        })
        .collect();
    let mut neg = Negotiator::new(&t.mg, 2);
    let opts = Options::default();
    let mut active: Vec<usize> = vec![0, 1];
    let mut ripped_ever: Vec<usize> = Vec::new();
    let mut last = Vec::new();
    for _ in 0..opts.pathfinder_iter_cap {
        let out = neg.iteration(
            &mut t.mg,
            &mut t.obs,
            &conns,
            &active,
            &starts,
            &goals,
            &opts,
            far(),
        );
        neg.penalise_overuse(&t.mg);
        for (&c, bi) in &out.blocked {
            neg.penalise_blocked(c, bi);
        }
        ripped_ever.extend(out.ripped.iter().copied());
        last = out.paths.clone();
        if out.blocked.is_empty() && out.ripped.is_empty() && out.overuse_edges == 0 {
            return (neg, last, ripped_ever, true);
        }
        let mut a: std::collections::BTreeSet<usize> = out.blocked.keys().copied().collect();
        a.extend(out.ripped.iter().copied());
        a.extend(overused_conns(&t.mg));
        active = a.into_iter().collect();
    }
    (neg, last, ripped_ever, false)
}

/// A (net 1, small 0.4 mm vias) and B (net 2, 0.6 mm vias) both go F.Cu -> B.Cu through a middle
/// column with two sites 0.7 mm apart (an active via blocks the other) and one far site that only
/// fits the small via. A is routed first and takes a middle site; B is blocked, its ghost uses
/// the owned site, so A is ripped (history on its site) and B (bumped, now first) takes the
/// middle; A then finds the far site. Both routed, one via each, within the iteration cap.
#[test]
fn site_rip_up_frees_owner_for_blocked_connection() {
    let mut t = two_layers(&contention_pads(), contention_sites());
    let (neg, paths, ripped, done) = negotiate_contention(&mut t);
    assert!(done, "did not converge in the iteration cap");
    assert!(neg.iteration <= 50);
    assert!(
        ripped.contains(&0),
        "A (the site owner) was ripped: {ripped:?}"
    );
    assert!(
        neg.site_history[0] + neg.site_history[1] > 0.0,
        "history on the contested site: {:?}",
        neg.site_history
    );
    let (pa, pb) = (paths[0].as_ref().unwrap(), paths[1].as_ref().unwrap());
    assert_eq!((pa.vias(), pb.vias()), (1, 1));
    assert_eq!(pa.sites[0].0, 2, "A took the far small-via site");
    assert!(pb.sites[0].0 <= 1, "B took a middle site");
    assert_eq!(t.mg.owner[pa.sites[0].0].map(|o| o.0), Some(0));
    assert_eq!(t.mg.owner[pb.sites[0].0].map(|o| o.0), Some(1));
    // deterministic
    let mut t2 = two_layers(&contention_pads(), contention_sites());
    let (_, paths2, _, _) = negotiate_contention(&mut t2);
    let sig = |p: &Vec<Option<SlotPath>>| -> Vec<Vec<usize>> {
        p.iter()
            .map(|x| x.as_ref().unwrap().sites.iter().map(|s| s.0).collect())
            .collect()
    };
    assert_eq!(sig(&paths), sig(&paths2));
}

/// Run one iteration of the two-connection channel board on a pool of `threads`; returns
/// `(resolved_sequentially, per-connection path signature, channel users)`.
fn channel_iteration(threads: usize) -> (usize, Vec<String>, usize) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    pool.install(|| {
        let mut b = channel_board();
        let (conns, verts) = setup(&b, &[(1, "sa", "ta"), (2, "sb", "tb")]);
        let mut neg = Negotiator::new(&b.mg, conns.len());
        let out = neg.iteration(
            &mut b.mg,
            &mut b.obs,
            &conns,
            &[0, 1],
            &starts_of(&verts),
            &goals_of(&verts),
            &Options::default(),
            far(),
        );
        assert!(!out.timed_out);
        let sig: Vec<String> = out.paths.iter().map(|p| format!("{:?}", p)).collect();
        let g = &b.mg.layers[0].g;
        let ch = g.edge(vert(&b, "u1"), vert(&b, "w1")).unwrap();
        let users = (0..2)
            .filter(|&c| {
                let k = b.mg.conn_wires[c][0];
                g.path_of(k).unwrap().0.iter().any(|&(e, _)| e == ch)
            })
            .count();
        (out.resolved_sequentially, sig, users)
    })
}

/// Both connections are searched against the same frozen graph and both pick the cap-1 channel;
/// the commit of the first changes the channel's faces, so the second path is invalidated and
/// searched again (it detours, as in the sequential M2 order). Identical for every pool size.
#[test]
fn iteration_batch_commit_invalidates_shared_edge() {
    let (resolved, sig, users) = channel_iteration(1);
    assert_eq!(resolved, 1, "second path invalidated");
    assert_eq!(users, 1, "one wire detours around the wall");
    assert!(sig.iter().all(|s| s.starts_with("Some")), "{sig:?}");
    for n in [2, 4] {
        let (r2, sig2, u2) = channel_iteration(n);
        assert_eq!((r2, u2), (resolved, users), "threads {n}");
        assert_eq!(sig2, sig, "threads {n}");
    }
}

/// F3: `dead` holds static blocks only. (a) B is blocked by A's via at the only site that fits
/// its 0.6 mm via: a dynamic block (ghost through the owned site, no static reason) - B is not
/// dead, A is ripped and B routes. (b) A pad outside the outline (every face around its vertex
/// is outside): blocked before any search with the static reason, dead, and reported the same
/// way in every later iteration without a search.
#[test]
fn dead_only_for_static_blocks() {
    let mut t = two_layers(&contention_pads(), contention_sites());
    let (neg, paths, ripped, done) = negotiate_contention(&mut t);
    assert!(done && ripped.contains(&0));
    assert!(!neg.is_dead(1), "a block by another connection's via is not static");
    assert!(paths[1].is_some(), "B routed after the rip-up");
    let mut b = build(&[("a", 1, 10.0, 20.0), ("b", 1, 60.0, 20.0)], "");
    let (conns, verts) = setup(&b, &[(1, "a", "b")]);
    let mut neg = Negotiator::new(&b.mg, 1);
    let opts = Options::default();
    let (starts, goals) = (starts_of(&verts), goals_of(&verts));
    for it in 1..=2 {
        let out = neg.iteration(
            &mut b.mg,
            &mut b.obs,
            &conns,
            &[0],
            &starts,
            &goals,
            &opts,
            far(),
        );
        let info = out.blocked.get(&0).expect("blocked");
        let r = info.static_reason.as_deref().expect("static reason");
        assert!(
            r.starts_with("blocked: terminal cannot be entered"),
            "it {it}: {r}"
        );
        assert!(info.ghost.is_none() && info.edge.is_none() && info.site.is_none());
        assert!(neg.is_dead(0), "it {it}");
        assert!(out.ripped.is_empty());
    }
}

/// A wall of pads at x = 25 with one 6 mm gap (y = 17..23, room for both wires).
fn wide_gap_board() -> Built {
    build(
        &[
            ("sa", 1, 10.0, 19.6),
            ("sb", 2, 10.0, 20.4),
            ("ta", 1, 40.0, 19.6),
            ("tb", 2, 40.0, 20.4),
            ("u1", 3, 25.0, 17.0),
            ("u2", 3, 25.0, 14.46),
            ("u3", 3, 25.0, 11.92),
            ("w1", 3, 25.0, 23.0),
            ("w2", 3, 25.0, 25.54),
            ("w3", 3, 25.0, 28.08),
        ],
        "",
    )
}

/// `(resolved_sequentially, revalidated, overuse, crossings of both paths)`.
type WideGapRun = (usize, usize, usize, Vec<Vec<(usize, usize)>>);

/// One batch iteration of the two connections of `wide_gap_board` on a pool of `threads`.
fn wide_gap_iteration(threads: usize) -> WideGapRun {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    pool.install(|| {
        let mut b = wide_gap_board();
        let (conns, verts) = setup(&b, &[(1, "sa", "ta"), (2, "sb", "tb")]);
        let mut neg = Negotiator::new(&b.mg, conns.len());
        let out = neg.iteration(
            &mut b.mg,
            &mut b.obs,
            &conns,
            &[0, 1],
            &starts_of(&verts),
            &goals_of(&verts),
            &Options::default(),
            far(),
        );
        assert!(!out.timed_out);
        let xs_all: Vec<Vec<(usize, usize)>> =
            out.paths.iter().map(|p| xs(p.as_ref().unwrap())).collect();
        (
            out.resolved_sequentially,
            out.revalidated,
            out.overuse_edges,
            xs_all,
        )
    })
}

/// F4: both connections are searched against the frozen empty graph and both pick the wide gap;
/// A's commit bumps the edge versions of the gap faces, so B's path fails `path_valid`, but the
/// O(k) re-validation against the current graph finds a slot next to A's strand in every face,
/// every edge still fits and B is inserted with those slots (`revalidated == 1`,
/// `resolved_sequentially == 0`). The result equals the sequential order (A, then B searched
/// against A's path) and is identical for every pool size.
#[test]
fn batch_validation_accepts_unaffected_paths() {
    let (resolved, revalidated, overuse, batch) = wide_gap_iteration(1);
    assert_eq!((resolved, revalidated, overuse), (0, 1, 0));
    assert!(!batch[1].is_empty(), "B crosses the gap");
    for n in [2, 4] {
        let (r, v, o, x) = wide_gap_iteration(n);
        assert_eq!((r, v, o, &x), (0, 1, 0, &batch), "threads {n}");
    }
    // sequential reference: A alone, then B against A's path
    let mut b = wide_gap_board();
    let (conns, verts) = setup(&b, &[(1, "sa", "ta"), (2, "sb", "tb")]);
    let mut neg = Negotiator::new(&b.mg, conns.len());
    let (starts, goals) = (starts_of(&verts), goals_of(&verts));
    let mut seq = Vec::new();
    for i in 0..2 {
        let out = neg.iteration(
            &mut b.mg,
            &mut b.obs,
            &conns,
            &[i],
            &starts,
            &goals,
            &Options::default(),
            far(),
        );
        assert_eq!((out.resolved_sequentially, out.revalidated), (0, 0));
        seq.push(xs(out.paths[i].as_ref().unwrap()));
    }
    assert_eq!(batch, seq, "batch re-validation equals the sequential order");
}
