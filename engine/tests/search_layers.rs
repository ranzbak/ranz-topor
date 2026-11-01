//! Layered slot search (M3 / T3.1): via moves at sites, pour goals, caps (note §2).
use std::collections::HashSet;
use std::time::{Duration, Instant};
use topor_engine::decompose::{Connection, TermKind, TermRef};
use topor_engine::geom::Vec2;
use topor_engine::graph::{GraphRules, Port, RouteGraph, TerminalInfo};
use topor_engine::model::{BoardModel, Options};
use topor_engine::multigraph::MultiGraph;
use topor_engine::obstacle::ObstacleSet;
use topor_engine::search::{
    default_expansion_cap, ghost, search, via_weight, Goal, SearchOutcome, SearchParams, SlotPath,
};
use topor_engine::vias::{SiteOrigin, ViaSite, ViaSize};

const MM: f64 = 1_000_000.0;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Through-hole: copper on both layers.
    Tht,
    /// SMD on F.Cu (layer 0).
    F,
    /// SMD on B.Cu (layer 31).
    B,
}

struct P {
    name: String,
    net: i32,
    x: f64,
    y: f64,
    d: f64,
    kind: Kind,
}

fn p(name: &str, net: i32, x: f64, y: f64, d: f64, kind: Kind) -> P {
    P {
        name: name.into(),
        net,
        x,
        y,
        d,
        kind,
    }
}

fn pad_json(q: &P) -> String {
    let shape = |l: u32| {
        format!(
            r#""{l}": {{ "kind": "circle", "d": {} }}"#,
            (q.d * MM) as i64
        )
    };
    let (layers, shapes) = match q.kind {
        Kind::Tht => ("[0, 31]", format!("{}, {}", shape(0), shape(31))),
        Kind::F => ("[0]", shape(0)),
        Kind::B => ("[31]", shape(31)),
    };
    format!(
        r#"{{ "uuid": "{n}", "ref": "{n}", "number": "1", "net": {net},
          "center": [{x}, {y}], "layers": {layers}, "shapes": {{ {shapes} }} }}"#,
        n = q.name,
        net = q.net,
        x = (q.x * MM) as i64,
        y = (q.y * MM) as i64
    )
}

fn model(pads: &[P], extra: &str) -> BoardModel {
    let pj: Vec<String> = pads.iter().map(pad_json).collect();
    model_raw(&pj, extra)
}

/// Rect SMD pad of net `net` on F.Cu, `w x h` mm, centre at kicad `(x, y)` mm.
fn rect_pad_json(name: &str, net: i32, x: f64, y: f64, w: f64, h: f64) -> String {
    format!(
        r#"{{ "uuid": "{name}", "ref": "{name}", "number": "1", "net": {net},
          "center": [{}, {}], "layers": [0],
          "shapes": {{ "0": {{ "kind": "rect", "w": {}, "h": {} }} }} }}"#,
        (x * MM) as i64,
        (y * MM) as i64,
        (w * MM) as i64,
        (h * MM) as i64
    )
}

fn model_raw(pj: &[String], extra: &str) -> BoardModel {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "routable": true }}, {{ "id": 31, "routable": true }} ],
          "outline": [
            {{ "kind": "seg", "a": [0, 0], "b": [50000000, 0] }},
            {{ "kind": "seg", "a": [50000000, 0], "b": [50000000, 40000000] }},
            {{ "kind": "seg", "a": [50000000, 40000000], "b": [0, 40000000] }},
            {{ "kind": "seg", "a": [0, 40000000], "b": [0, 0] }} ],
          "pads": [ {} ], {extra}
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000 }} }}"#,
        pj.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

/// Site at kicad `(x, y)` mm (math frame y = -y).
fn site(x_mm: f64, y_mm: f64) -> ViaSite {
    ViaSite {
        at: Vec2 {
            x: x_mm * MM,
            y: -y_mm * MM,
        },
        max_d: 600_000,
        max_drill: 300_000,
        origin: SiteOrigin::Face,
        net: None,
    }
}

fn size() -> ViaSize {
    ViaSize {
        d: 600_000,
        drill: 300_000,
    }
}

fn rules() -> GraphRules {
    GraphRules {
        eps: 5_000.0,
        min_clearance: 0.0,
        edge_clearance: 500_000.0,
        hole_clearance: 250_000.0,
    }
}

struct B {
    mg: MultiGraph,
    obs: ObstacleSet,
}

fn build(m: &BoardModel, sites: Vec<ViaSite>) -> B {
    build_terms(m, sites, &[])
}

/// [`build`] with explicit terminal vertices (pad centres of non-round pads), each on the given
/// layers.
fn build_terms(m: &BoardModel, sites: Vec<ViaSite>, terms: &[(TerminalInfo, Vec<u32>)]) -> B {
    let mut obs = ObstacleSet::build(m, &Options::default(), &HashSet::new(), &[]);
    let mg = MultiGraph::build_on(m, &mut obs, sites, &[0, 31], terms, &rules()).unwrap();
    B { mg, obs }
}

/// Terminal at the centre of pad `name` of `net` at kicad `(x, y)` mm (`r 0`, clearance 0.2 mm).
fn term(name: &str, net: i32, x: f64, y: f64) -> TerminalInfo {
    TerminalInfo {
        at: Vec2 {
            x: x * MM,
            y: -y * MM,
        },
        net,
        owner: format!("pad:{name}:1"),
        clr: 200_000.0,
        r: 0.0,
    }
}

/// Terminal vertex of pad `name` on layer index `li`.
fn vert(b: &B, li: usize, name: &str) -> usize {
    let label = format!("pad:{name}:1");
    let layer = b.mg.layers[li].layer;
    let ai = b
        .obs
        .layer(layer)
        .anchors
        .iter()
        .position(|a| a.label == label)
        .unwrap_or_else(|| panic!("pad {name} has no anchor on layer {layer}"));
    b.mg.layers[li].ls.cdt.vertex_index(ai)
}

fn conn(id: usize, net: i32, from: &str, to: &str) -> Connection {
    let t = |l: &str| TermRef {
        kind: TermKind::Pad(0),
        label: l.to_string(),
        at: Vec2 { x: 0.0, y: 0.0 },
        layers: vec![0, 31],
    };
    Connection {
        id,
        net,
        width: 250_000,
        clearance: 200_000,
        tier: 3,
        from: t(from),
        to: t(to),
        est_len: 0.0,
    }
}

fn goal(v: &[(usize, usize)]) -> Goal {
    Goal {
        vertices: v.to_vec(),
        pour_sites: Vec::new(),
        pour_layer: None,
    }
}

struct Ctx {
    hist: Vec<Vec<f64>>,
    site_hist: Vec<f64>,
    via_cost: f64,
    max_vias: u32,
    cap: usize,
}

impl Ctx {
    fn new() -> Ctx {
        Ctx {
            hist: Vec::new(),
            site_hist: Vec::new(),
            via_cost: 2.0 * MM * via_weight(3),
            max_vias: 4,
            cap: usize::MAX,
        }
    }
    fn params(&self) -> SearchParams<'_> {
        SearchParams {
            history: &self.hist,
            site_history: &self.site_hist,
            weight: 1.0,
            present_overuse: 4.0,
            expansion_cap: self.cap,
            deadline: Instant::now() + Duration::from_secs(30),
            eps: None,
            via_cost: self.via_cost,
            max_vias: self.max_vias,
            strict_unfit_edges: false,
        }
    }
}

fn found(o: SearchOutcome) -> SlotPath {
    match o {
        SearchOutcome::Found(p) => p,
        other => panic!("expected Found, got {other:?}"),
    }
}

/// No two strands cross inside any face (independent re-derivation of the cyclic port order).
fn assert_planar(g: &RouteGraph) {
    for (f, face) in g.faces.iter().enumerate() {
        let mut seq: Vec<Port> = Vec::new();
        for j in 0..3 {
            let v = face.v[j];
            seq.push(Port::Vertex(v));
            let e = face.e[(j + 2) % 3];
            let n = g.slot_count(e);
            let order: Vec<usize> = if g.edges[e].a == v {
                (0..n).collect()
            } else {
                (0..n).rev().collect()
            };
            seq.extend(order.into_iter().map(|i| Port::Cross(e, i)));
        }
        let pos = |p: Port| seq.iter().position(|&x| x == p);
        let mut strands = Vec::new();
        for &e in &face.e {
            for i in 0..g.slot_count(e) {
                let q = g.strand(e, i, f);
                if q == Port::Detached {
                    continue;
                }
                let (a, b) = (pos(Port::Cross(e, i)).unwrap(), pos(q).unwrap());
                strands.push((a.min(b), a.max(b)));
            }
        }
        for &(a0, a1) in &strands {
            for &(b0, b1) in &strands {
                assert!(
                    !(a0 < b0 && b0 < a1 && a1 < b1),
                    "face {f}: strands ({a0},{a1}) and ({b0},{b1}) cross"
                );
            }
        }
    }
}

/// A row of foreign F.Cu-only SMD pads (net 3) at kicad x, closing the board edge to edge.
fn wall(x: f64, first: usize) -> Vec<P> {
    (0..32)
        .map(|i| {
            p(
                &format!("w{}", first + i),
                3,
                x,
                0.5 + 1.25 * i as f64,
                1.0,
                Kind::F,
            )
        })
        .chain(std::iter::once(p(
            &format!("w{}", first + 32),
            3,
            x,
            39.5,
            1.0,
            Kind::F,
        )))
        .collect()
}

/// F.Cu-only foreign tracks (net 3) along kicad `x` over the `y` spans: a hard wall for the
/// planar search (constraint edges are not passable).
fn kept_wall(x: f64, spans: &[(f64, f64)]) -> String {
    let items: Vec<String> = spans
        .iter()
        .enumerate()
        .map(|(i, &(y0, y1))| {
            format!(
                r#"{{ "uuid": "k{i}", "net": 3, "layer": 0, "width": 250000,
                  "prim": {{ "kind": "seg", "a": [{xx}, {a}], "b": [{xx}, {b}] }} }}"#,
                xx = (x * MM) as i64,
                a = (y0 * MM) as i64,
                b = (y1 * MM) as i64
            )
        })
        .collect();
    format!(r#""kept": [ {} ],"#, items.join(","))
}

#[test]
fn tht_pads_route_on_either_layer_without_via() {
    let mut pads = vec![
        p("a", 1, 10.0, 10.0, 1.0, Kind::Tht),
        p("b", 1, 40.0, 10.0, 1.0, Kind::Tht),
    ];
    pads.extend(wall(25.0, 0));
    let m = model(&pads, "");
    let b = build(&m, vec![site(20.0, 20.0), site(30.0, 20.0)]);
    let c = conn(0, 1, "a", "b");
    let start: Vec<(usize, usize)> = (0..2).map(|l| (l, vert(&b, l, "a"))).collect();
    let g = goal(&(0..2).map(|l| (l, vert(&b, l, "b"))).collect::<Vec<_>>());
    let ctx = Ctx::new();
    let path = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    assert_eq!(path.legs.len(), 1);
    assert_eq!(path.legs[0].layer_idx, 1, "F.Cu is walled off: B.Cu");
    assert_eq!(path.vias(), 0);
    assert!(path.sites.is_empty());
    assert!((path.est_len - 30.0 * MM).abs() < 1.0, "{}", path.est_len);
    assert!((path.cost - path.est_len).abs() < 1e-6);
    // without the wall both layers tie and F.Cu (the lower index) wins
    let m2 = model(
        &[
            p("a", 1, 10.0, 10.0, 1.0, Kind::Tht),
            p("b", 1, 40.0, 10.0, 1.0, Kind::Tht),
        ],
        "",
    );
    let b2 = build(&m2, vec![site(20.0, 20.0)]);
    let start2: Vec<(usize, usize)> = (0..2).map(|l| (l, vert(&b2, l, "a"))).collect();
    let g2 = goal(&(0..2).map(|l| (l, vert(&b2, l, "b"))).collect::<Vec<_>>());
    let p2 = found(search(&b2.mg, &b2.obs, &c, &start2, &g2, &ctx.params()));
    assert_eq!((p2.legs.len(), p2.legs[0].layer_idx, p2.vias()), (1, 0, 0));
}

#[test]
fn smd_f_to_smd_b_needs_exactly_one_via() {
    let m = model(
        &[
            p("a", 1, 10.0, 10.0, 1.0, Kind::F),
            p("b", 1, 40.0, 10.0, 1.0, Kind::B),
        ],
        "",
    );
    let b = build(&m, vec![site(25.0, 12.0), site(25.0, 20.0)]);
    let c = conn(0, 1, "a", "b");
    let start = [(0, vert(&b, 0, "a"))];
    let g = goal(&[(1, vert(&b, 1, "b"))]);
    let ctx = Ctx::new();
    let path = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    assert_eq!(path.legs.len(), 2);
    assert_eq!(path.sites.len(), 1);
    assert_eq!(path.vias(), 1);
    assert_eq!((path.legs[0].layer_idx, path.legs[1].layer_idx), (0, 1));
    let (s, sz) = path.sites[0];
    assert!(b.mg.site_free(s, 1, sz));
    assert_eq!(sz, size());
    assert_eq!(path.legs[0].goal, b.mg.site_vertex[s][0]);
    assert_eq!(path.legs[1].start, b.mg.site_vertex[s][1]);
    let sum: f64 = path.legs.iter().map(|l| l.est_len).sum();
    assert!((sum - path.est_len).abs() < 1e-6);
    // 2 mm per via at tier 3, history 0 and nothing overused
    assert!(
        (path.cost - (path.est_len + 2.0 * MM)).abs() < 1.0,
        "cost {} est {}",
        path.cost,
        path.est_len
    );
    // the nearest of the two sites to the straight line
    assert_eq!(b.mg.sites[s].at.y, -12.0 * MM);
}

#[test]
fn via_cost_prefers_detour_when_cheaper() {
    // F.Cu pad -> THT pad; a foreign F.Cu track wall with a gap at y 17..23 (kicad) forces a
    // ~3.1 mm F.Cu detour (est_len counts the string through the gap's track end), while the
    // site next to the source lets the wire hop to B.Cu and run straight (30 mm + via)
    let m = model(
        &[
            p("a", 1, 10.0, 10.0, 1.0, Kind::F),
            p("b", 1, 40.0, 10.0, 1.0, Kind::Tht),
        ],
        &kept_wall(25.0, &[(0.0, 17.0), (23.0, 40.0)]),
    );
    let b = build(&m, vec![site(12.5, 10.0)]);
    let c = conn(0, 1, "a", "b");
    let start = [(0, vert(&b, 0, "a"))];
    let g = goal(&[(0, vert(&b, 0, "b")), (1, vert(&b, 1, "b"))]);
    let mut ctx = Ctx::new();
    ctx.via_cost = 4.0 * MM;
    let dear = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    assert_eq!(dear.vias(), 0, "via 4 mm: the detour is cheaper");
    assert_eq!((dear.legs.len(), dear.legs[0].layer_idx), (1, 0));
    assert!(dear.est_len > 32.0 * MM, "{}", dear.est_len);
    ctx.via_cost = 0.5 * MM;
    let cheap = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    assert_eq!(cheap.vias(), 1, "via 0.5 mm: the hop is cheaper");
    assert_eq!(cheap.legs.len(), 2);
    assert!(cheap.est_len < dear.est_len);
    assert!((cheap.est_len - 30.0 * MM).abs() < 1.0, "{}", cheap.est_len);
}

#[test]
fn owned_and_blocked_sites_are_not_used() {
    let m = model(
        &[
            p("a", 1, 10.0, 10.0, 1.0, Kind::F),
            p("b", 1, 40.0, 10.0, 1.0, Kind::B),
        ],
        "",
    );
    // two sites 0.4 mm apart: owning one blocks the other
    let mut b = build(&m, vec![site(25.0, 12.0), site(25.4, 12.0)]);
    let c = conn(0, 1, "a", "b");
    let start = [(0, vert(&b, 0, "a"))];
    let g = goal(&[(1, vert(&b, 1, "b"))]);
    let ctx = Ctx::new();
    let s0 = (0..2).find(|&s| b.mg.sites[s].at.x == 25.0 * MM).unwrap();
    let s1 = 1 - s0;
    b.mg.activate(s0, 7, 2, size(), 200_000.0, &mut b.obs);
    assert_eq!(b.mg.blocked[s1], 1);
    match search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()) {
        SearchOutcome::Blocked {
            ghost: Some(gp),
            blocked_edge,
            blocked_site,
            ..
        } => {
            assert!(gp.vias() >= 1);
            assert!(
                blocked_site.is_some() || blocked_edge.is_some(),
                "a soft-blocked element is reported"
            );
            if let Some(s) = blocked_site {
                assert!(s == s0 || s == s1);
            }
        }
        other => panic!("expected Blocked with a ghost, got {other:?}"),
    }
    let (gp, ..) = ghost(&b.mg, &b.obs, &c, &start, &g, &ctx.params()).expect("ghost");
    assert!(gp.sites.iter().all(|&(s, _)| s == s0 || s == s1));
    b.mg.release(s0, &mut b.obs);
    let ok = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    assert_eq!(ok.vias(), 1);
}

#[test]
fn max_vias_cap_blocks() {
    // F.Cu -> F.Cu behind a full F.Cu wall: down before it, up behind it = 2 vias
    let pads = vec![
        p("a", 1, 10.0, 10.0, 1.0, Kind::F),
        p("b", 1, 40.0, 10.0, 1.0, Kind::F),
    ];
    let m = model(&pads, &kept_wall(25.0, &[(0.0, 40.0)]));
    let b = build(&m, vec![site(16.0, 12.0), site(34.0, 12.0)]);
    let c = conn(0, 1, "a", "b");
    let start = [(0, vert(&b, 0, "a"))];
    let g = goal(&[(0, vert(&b, 0, "b"))]);
    let mut ctx = Ctx::new();
    ctx.max_vias = 1;
    assert!(matches!(
        search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()),
        SearchOutcome::Blocked { .. }
    ));
    ctx.max_vias = 2;
    let ok = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    assert_eq!(ok.vias(), 2);
    assert_eq!(ok.legs.len(), 3);
    assert_eq!(
        ok.legs.iter().map(|l| l.layer_idx).collect::<Vec<_>>(),
        vec![0, 1, 0]
    );
    assert_ne!(ok.sites[0].0, ok.sites[1].0, "a site is used at most once");
    assert_eq!(ok.crossings_on(1).len(), ok.legs[1].crossings.len());
}

#[test]
fn pour_goal_reaches_nearest_site_inside_pour() {
    let pour = r#""pours": [ { "uuid": "z", "net": 1, "layers": [31], "pads": [], "vias": [],
             "fills": [ { "layer": 31, "outer": [[15000000, 5000000], [30000000, 5000000],
                                                 [30000000, 15000000], [15000000, 15000000]] } ] } ],"#;
    let m = model(&[p("a", 1, 10.0, 10.0, 1.0, Kind::F)], pour);
    let b = build(
        &m,
        vec![
            site(12.0, 10.0),
            site(17.0, 10.0),
            site(25.0, 10.0),
            site(40.0, 30.0),
        ],
    );
    let in_pour: Vec<usize> = (0..b.mg.sites.len())
        .filter(|&s| b.mg.in_pour[s].contains(&0))
        .collect();
    assert_eq!(in_pour.len(), 2, "the sites at x = 17 and 25");
    let c = conn(0, 1, "a", "a");
    let start = [(0, vert(&b, 0, "a"))];
    let g = Goal {
        vertices: Vec::new(),
        pour_sites: in_pour.clone(),
        pour_layer: Some(1),
    };
    let ctx = Ctx::new();
    let path = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    // P1: the pour is reached by the via itself, no zero-length leg on the pour layer
    assert_eq!((path.legs.len(), path.sites.len()), (1, 1));
    assert_eq!(path.legs[0].layer_idx, 0);
    let s = path.sites[0].0;
    assert!(b.mg.in_pour[s].contains(&0));
    assert_eq!(
        b.mg.sites[s].at.x,
        17.0 * MM,
        "the nearest site inside the pour"
    );
    assert_eq!(path.legs[0].goal, b.mg.site_vertex[s][0]);
    assert!((path.cost - (path.est_len + 2.0 * MM)).abs() < 1.0);
    // a pour without a reachable site: Blocked, never a panic
    let none = Goal {
        pour_sites: vec![],
        ..g.clone()
    };
    assert!(matches!(
        search(&b.mg, &b.obs, &c, &start, &none, &ctx.params()),
        SearchOutcome::Blocked { .. }
    ));
}

#[test]
fn layers_stay_planar_with_via_on_foreign_wire() {
    // net 2 runs F.Cu from x2a to x2b right across the site; net 1 then wants F.Cu -> B.Cu
    // through that site (and one far away). Whatever the search decides, the layers stay planar.
    let m = model(
        &[
            p("a", 1, 10.0, 20.0, 1.0, Kind::F),
            p("b", 1, 40.0, 20.0, 1.0, Kind::B),
            p("x", 2, 20.0, 20.0, 1.0, Kind::F),
            p("y", 2, 30.0, 20.0, 1.0, Kind::F),
        ],
        "",
    );
    for far in [false, true] {
        let mut sites = vec![site(25.0, 20.0)];
        if far {
            sites.push(site(25.0, 5.0));
        }
        let mut b = build(&m, sites);
        let c2 = conn(0, 2, "x", "y");
        let (s2, t2) = (vert(&b, 0, "x"), vert(&b, 0, "y"));
        let ctx = Ctx::new();
        let wrap = found(search(
            &b.mg,
            &b.obs,
            &c2,
            &[(0, s2)],
            &goal(&[(0, t2)]),
            &ctx.params(),
        ));
        b.mg.insert_path(&c2, &wrap, &mut b.obs);
        let c1 = conn(1, 1, "a", "b");
        let out = search(
            &b.mg,
            &b.obs,
            &c1,
            &[(0, vert(&b, 0, "a"))],
            &goal(&[(1, vert(&b, 1, "b"))]),
            &ctx.params(),
        );
        match out {
            SearchOutcome::Found(path) => {
                b.mg.insert_path(&c1, &path, &mut b.obs);
                if far {
                    assert_eq!(path.vias(), 1);
                }
            }
            SearchOutcome::Blocked { .. } => {
                assert!(!far);
            }
            SearchOutcome::Timeout => panic!("timeout"),
        }
        for l in &b.mg.layers {
            assert_planar(&l.g);
        }
    }
}

#[test]
fn via_move_respects_rule_t_at_site() {
    // Note §2 counter-example: foreign terminal p adjacent to site s, third vertex x (the
    // source of our net). The foreign strand leaves p through edge (s, x). The M2 goal rule
    // rejects the via move x -> s in both faces of (s, x): the strand separates the entry
    // port from s.
    let m = model(
        &[
            p("p", 2, 20.0, 20.0, 1.0, Kind::F),
            p("x", 1, 20.0, 24.0, 1.0, Kind::F),
            p("b", 1, 40.0, 22.0, 1.0, Kind::B),
        ],
        "",
    );
    let mut b = build(&m, vec![site(22.0, 22.0)]);
    let c = conn(1, 1, "x", "b");
    let start = [(0, vert(&b, 0, "x"))];
    let g = goal(&[(1, vert(&b, 1, "b"))]);
    let ctx = Ctx::new();
    let free = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    assert_eq!(free.vias(), 1);
    assert!(
        free.legs[0].crossings.is_empty(),
        "without the strand the via is one edge away"
    );
    // hand-insert the foreign strand p -> (s, x) -> face behind it
    let (pv, xv, sv) = (vert(&b, 0, "p"), vert(&b, 0, "x"), b.mg.site_vertex[0][0]);
    let g0 = &b.mg.layers[0].g;
    let e1 = g0.edge(sv, xv).expect("edge (s, x)");
    let f = g0.edges[e1]
        .faces
        .iter()
        .flatten()
        .copied()
        .find(|&f| g0.faces[f].v.contains(&pv))
        .expect("face (p, s, x)");
    let f2 = g0.edges[e1]
        .faces
        .iter()
        .flatten()
        .copied()
        .find(|&q| q != f)
        .unwrap();
    let w = *g0.faces[f2]
        .v
        .iter()
        .find(|&&v| v != sv && v != xv)
        .unwrap();
    b.mg.layers[0]
        .g
        .insert_path(9, pv, w, &[(e1, 0)], &[f, f2], 250_000.0, 200_000.0);
    let out = search(&b.mg, &b.obs, &c, &start, &g, &ctx.params());
    match out {
        SearchOutcome::Found(path) => {
            assert_eq!(path.vias(), 1);
            assert!(
                !path.legs[0].crossings.is_empty(),
                "the direct hop x -> s would cut the strand"
            );
            b.mg.insert_path(&c, &path, &mut b.obs);
        }
        SearchOutcome::Blocked { .. } => {}
        SearchOutcome::Timeout => panic!("timeout"),
    }
    for l in &b.mg.layers {
        assert_planar(&l.g);
    }
}

#[test]
fn layered_search_is_deterministic_and_capped() {
    let m = model(
        &[
            p("a", 1, 10.0, 10.0, 1.0, Kind::F),
            p("b", 1, 40.0, 10.0, 1.0, Kind::B),
        ],
        "",
    );
    let b = build(
        &m,
        vec![
            site(20.0, 12.0),
            site(25.0, 12.0),
            site(30.0, 8.0),
            site(25.0, 30.0),
        ],
    );
    let c = conn(0, 1, "a", "b");
    let start = [(0, vert(&b, 0, "a"))];
    let g = goal(&[(1, vert(&b, 1, "b"))]);
    let mut ctx = Ctx::new();
    let key = |p: &SlotPath| {
        (
            p.legs
                .iter()
                .map(|l| (l.layer_idx, l.crossings.clone(), l.faces.clone()))
                .collect::<Vec<_>>(),
            p.sites.clone(),
            p.est_len.to_bits(),
            p.cost.to_bits(),
            p.expansions,
        )
    };
    let p1 = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    let p2 = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    assert_eq!(key(&p1), key(&p2));
    assert!(p1.expansions <= default_expansion_cap(&b.mg));
    // a one-expansion cap ends the search without a panic
    ctx.cap = 1;
    let o = search(&b.mg, &b.obs, &c, &start, &g, &ctx.params());
    assert!(
        matches!(o, SearchOutcome::Blocked { .. } | SearchOutcome::Timeout),
        "{o:?}"
    );
    // the ghost is deterministic as well
    let (g1, e1, s1, l1) = ghost(&b.mg, &b.obs, &c, &start, &g, &ctx.params()).unwrap();
    let (g2, e2, s2, l2) = ghost(&b.mg, &b.obs, &c, &start, &g, &ctx.params()).unwrap();
    assert_eq!((key(&g1), e1, s1, l1), (key(&g2), e2, s2, l2));
}

#[test]
fn insert_path_activates_sites_and_remove_conn_frees_them() {
    let m = model(
        &[
            p("a", 1, 10.0, 10.0, 1.0, Kind::F),
            p("b", 1, 40.0, 10.0, 1.0, Kind::B),
        ],
        "",
    );
    let mut b = build(&m, vec![site(25.0, 12.0)]);
    let c = conn(0, 1, "a", "b");
    let start = [(0, vert(&b, 0, "a"))];
    let g = goal(&[(1, vert(&b, 1, "b"))]);
    let ctx = Ctx::new();
    let path = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    let ids = b.mg.insert_path(&c, &path, &mut b.obs);
    assert_eq!(ids.len(), 2);
    assert_eq!(b.mg.owner[0].map(|o| o.0), Some(0));
    assert_eq!(b.mg.conns_of_legs(&ids), vec![0]);
    for l in &b.mg.layers {
        assert_planar(&l.g);
    }
    b.mg.remove_conn(0, &mut b.obs);
    assert!(b.mg.owner[0].is_none());
    let again = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    assert_eq!(again.vias(), 1);
}

/// `ghost()` result: `(path, first blocked edge, first blocked site, crossing before it)`.
type GhostOut = Option<(SlotPath, Option<(usize, usize)>, Option<usize>, Option<(usize, usize)>)>;

/// A ghost element is soft (a crossing or a site the slot search could not take as it stands).
fn ghost_soft(o: &GhostOut) -> bool {
    match o {
        Some((_, e, s, _)) => e.is_some() || s.is_some(),
        None => true,
    }
}

/// F2 (RC2a): the topological ghost is never stricter than the slot search. Wherever the search
/// finds a path, the ghost finds one too; where the search's path crosses only fitting, passable
/// edges and free sites, the ghost's has no soft element at all.
#[test]
fn ghost_is_never_stricter_than_search() {
    let ctx = Ctx::new();
    // (1) Two 0.6 x 0.3 mm rect SMD pads of one net on F.Cu, 10 mm apart: the search leaves a pad
    // through the pad's own polygon edges. Net-blind (M3) they are statically unfit (the corner
    // vertices count as clearance gaps) and were crossed as overuse, the ghost's first soft
    // element; with the own-net capacity of M4-D1 (A4: own ends charge no gap, design note
    // m4-capacity-steiner-design.md §1 row 6: tip 0.3 / need 0.205) they fit the pad's own net,
    // so search and ghost agree and the ghost has no soft element at all. (The geometric ghost
    // of M2 treated every unfit edge as hard and found no path: dcsgone ghost None for 166/231.)
    let m = model_raw(
        &[
            rect_pad_json("a", 1, 10.0, 10.0, 0.6, 0.3),
            rect_pad_json("b", 1, 20.0, 10.0, 0.6, 0.3),
        ],
        "",
    );
    let terms = [
        (term("a", 1, 10.0, 10.0), vec![0]),
        (term("b", 1, 20.0, 10.0), vec![0]),
    ];
    let b = build_terms(&m, Vec::new(), &terms);
    let va = b.mg.vertex_of(0, terms[0].0.at).expect("a");
    let vb = b.mg.vertex_of(0, terms[1].0.at).expect("b");
    let c = conn(0, 1, "a", "b");
    let g0 = &b.mg.layers[0].g;
    let spokes: Vec<usize> = g0
        .faces_of_vertex(va)
        .iter()
        .flat_map(|&f| g0.faces[f].e)
        .collect();
    assert!(
        spokes.iter().all(|&e| !g0.fits(e, 250_000.0, 200_000.0)),
        "net-blind (M3): every edge around the rect pad centre is unfit"
    );
    assert!(
        spokes.iter().all(|&e| g0.fits_for(e, 1, 250_000.0, 200_000.0)),
        "own-net: every edge around the rect pad centre fits the pad's own net"
    );
    assert!(spokes.iter().all(|&e| !g0.fits_for(e, 2, 250_000.0, 200_000.0)));
    let start = [(0, va)];
    let g = goal(&[(0, vb)]);
    let found_p = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    assert!(found_p.legs[0].crossings.len() >= 2, "{found_p:?}");
    let gh = ghost(&b.mg, &b.obs, &c, &start, &g, &ctx.params());
    assert!(!ghost_soft(&gh), "fits, ghost not soft: {gh:?}");
    let (gp, ..) = gh.as_ref().expect("the ghost must find the path the search found");
    assert!(gp.est_len >= 10.0 * MM - 1.0 && gp.est_len < 10.5 * MM, "{}", gp.est_len);
    assert_eq!(gp.candidates, 0, "no clearance candidates in the topological ghost");

    // (2) THT pads with F.Cu walled off: the search goes B.Cu without a via; so does the ghost,
    // with no soft element and the straight length (the F.Cu wall gaps are soft, B.Cu is free).
    let mut pads = vec![
        p("a", 1, 10.0, 10.0, 1.0, Kind::Tht),
        p("b", 1, 40.0, 10.0, 1.0, Kind::Tht),
    ];
    pads.extend(wall(25.0, 0));
    let m = model(&pads, "");
    let b = build(&m, vec![site(20.0, 20.0), site(30.0, 20.0)]);
    let start: Vec<(usize, usize)> = (0..2).map(|l| (l, vert(&b, l, "a"))).collect();
    let g = goal(&(0..2).map(|l| (l, vert(&b, l, "b"))).collect::<Vec<_>>());
    let found_p = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    assert_eq!(found_p.legs[0].layer_idx, 1);
    let gh = ghost(&b.mg, &b.obs, &c, &start, &g, &ctx.params());
    assert!(!ghost_soft(&gh), "{gh:?}");
    let gp = gh.unwrap().0;
    assert_eq!((gp.legs.len(), gp.legs[0].layer_idx, gp.vias()), (1, 1, 0));
    assert!((gp.est_len - 30.0 * MM).abs() < 1.0, "{}", gp.est_len);

    // (3) SMD F.Cu -> SMD B.Cu: one via for both.
    let m = model(
        &[
            p("a", 1, 10.0, 10.0, 1.0, Kind::F),
            p("b", 1, 40.0, 10.0, 1.0, Kind::B),
        ],
        "",
    );
    let b = build(&m, vec![site(25.0, 12.0), site(25.0, 30.0)]);
    let start = [(0, vert(&b, 0, "a"))];
    let g = goal(&[(1, vert(&b, 1, "b"))]);
    let found_p = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    assert_eq!(found_p.vias(), 1);
    let gh = ghost(&b.mg, &b.obs, &c, &start, &g, &ctx.params());
    assert!(!ghost_soft(&gh), "{gh:?}");
    let gp = gh.unwrap().0;
    assert_eq!((gp.vias(), gp.legs.len(), gp.sites[0].0), (1, 2, 0));
    assert!(gp.est_len < 31.0 * MM && gp.est_len > 30.0 * MM, "{}", gp.est_len);

    // (4) F.Cu -> F.Cu behind a full F.Cu wall at `max_vias 2`: down and up again.
    let m = model(
        &[
            p("a", 1, 10.0, 10.0, 1.0, Kind::F),
            p("b", 1, 40.0, 10.0, 1.0, Kind::F),
        ],
        &kept_wall(25.0, &[(0.0, 40.0)]),
    );
    let b = build(&m, vec![site(16.0, 12.0), site(34.0, 12.0)]);
    let start = [(0, vert(&b, 0, "a"))];
    let g = goal(&[(0, vert(&b, 0, "b"))]);
    let mut ctx2 = Ctx::new();
    ctx2.max_vias = 2;
    let found_p = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx2.params()));
    assert_eq!(found_p.vias(), 2);
    let gh = ghost(&b.mg, &b.obs, &c, &start, &g, &ctx2.params());
    assert!(!ghost_soft(&gh), "{gh:?}");
    let gp = gh.unwrap().0;
    assert_eq!(gp.vias(), 2);
    assert_eq!(
        gp.legs.iter().map(|l| l.layer_idx).collect::<Vec<_>>(),
        vec![0, 1, 0]
    );
    // at `max_vias 1` the wall can only be crossed softly (through the kept track)
    let gh1 = ghost(&b.mg, &b.obs, &c, &start, &g, &ctx.params());
    let _ = gh1;
}

/// F2: the topological ghost is a Dijkstra over `(layer, edge, dir, vias)` with no geometry:
/// on a board of ~1k faces per layer it pops well under 50 k nodes and examines no clearance
/// candidate at all (the geometric ghost visited the whole reachable component with
/// `O(V)` candidates per chord before conceding).
#[test]
fn ghost_cost_is_linear() {
    let mut pads = vec![
        p("a", 1, 2.0, 2.0, 1.0, Kind::F),
        p("b", 1, 48.0, 38.0, 1.0, Kind::B),
    ];
    for j in 0..19 {
        for i in 0..25 {
            pads.push(p(
                &format!("x{i}_{j}"),
                3 + i + 25 * j,
                4.0 + 1.75 * i as f64,
                4.0 + 1.75 * j as f64,
                1.0,
                if (i + j) % 2 == 0 { Kind::F } else { Kind::Tht },
            ));
        }
    }
    let m = model(&pads, "");
    let sites: Vec<ViaSite> = (0..6)
        .map(|k| site(5.0 + 7.0 * k as f64, 2.2))
        .chain((0..6).map(|k| site(6.0 + 7.0 * k as f64, 38.5)))
        .collect();
    let b = build(&m, sites);
    let faces: usize = b.mg.layers.iter().map(|l| l.g.faces.len()).sum();
    assert!(faces >= 1000, "faces {faces}");
    let c = conn(0, 1, "a", "b");
    let start = [(0, vert(&b, 0, "a"))];
    let g = goal(&[(1, vert(&b, 1, "b"))]);
    let ctx = Ctx::new();
    let t = Instant::now();
    let (gp, be, bs, _) = ghost(&b.mg, &b.obs, &c, &start, &g, &ctx.params()).expect("ghost");
    let dt = t.elapsed();
    assert_eq!((be, bs), (None, None), "{gp:?}");
    assert!(gp.vias() >= 1);
    assert!(gp.expansions < 50_000, "pops {}", gp.expansions);
    assert_eq!(gp.candidates, 0);
    assert!(dt.as_secs_f64() < 2.0, "{dt:?}");
    let (g2, ..) = ghost(&b.mg, &b.obs, &c, &start, &g, &ctx.params()).unwrap();
    assert_eq!(
        (g2.expansions, g2.est_len.to_bits(), g2.sites.clone()),
        (gp.expansions, gp.est_len.to_bits(), gp.sites.clone())
    );
}

/// Board 3.2 mm wide (kicad x 9..12.2, y 0..20) with a pinch: THT pads of nets 2 and 3 at
/// (10, 10) and (11.2, 10) leave 0.2 mm between them and no room outside, on both layers.
fn pinch_model() -> BoardModel {
    let pads = [
        p("a", 1, 10.6, 5.0, 1.0, Kind::Tht),
        p("b", 1, 10.6, 15.0, 1.0, Kind::Tht),
        p("x", 2, 10.0, 10.0, 1.0, Kind::Tht),
        p("y", 3, 11.2, 10.0, 1.0, Kind::Tht),
    ];
    let pj: Vec<String> = pads.iter().map(pad_json).collect();
    let (x0, x1, y1) = (9_000_000, 12_200_000, 20_000_000);
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "routable": true }}, {{ "id": 31, "routable": true }} ],
          "outline": [
            {{ "kind": "seg", "a": [{x0}, 0], "b": [{x1}, 0] }},
            {{ "kind": "seg", "a": [{x1}, 0], "b": [{x1}, {y1}] }},
            {{ "kind": "seg", "a": [{x1}, {y1}], "b": [{x0}, {y1}] }},
            {{ "kind": "seg", "a": [{x0}, {y1}], "b": [{x0}, 0] }} ],
          "pads": [ {} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000 }} }}"#,
        pj.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

#[test]
fn strict_unfit_edge_rejected_in_search() {
    let m = pinch_model();
    let b = build(&m, vec![]);
    let c = conn(0, 1, "a", "b");
    let start: Vec<(usize, usize)> = (0..2).map(|l| (l, vert(&b, l, "a"))).collect();
    let g = goal(&(0..2).map(|l| (l, vert(&b, l, "b"))).collect::<Vec<_>>());
    let ctx = Ctx::new();
    // the pinch edge is unfit even when empty
    let li = 0;
    let e = b.mg.layers[li]
        .g
        .edge(vert(&b, li, "x"), vert(&b, li, "y"))
        .expect("pinch edge");
    assert!(!b.mg.layers[li].g.fits_at(e, 250_000.0, 200_000.0, 5_000.0));
    // default: the unfit gap is priced as an overuse crossing, the connection is found
    let loose = found(search(&b.mg, &b.obs, &c, &start, &g, &ctx.params()));
    assert!(loose.legs.iter().any(|l| l.crossings.iter().any(|&(x, _)| x == e)));
    // strict: no unfit edge is entered, the search is blocked (the ghost keeps pricing it soft)
    let mut sp = ctx.params();
    sp.strict_unfit_edges = true;
    match search(&b.mg, &b.obs, &c, &start, &g, &sp) {
        SearchOutcome::Blocked { .. } => {}
        other => panic!("strict search must be blocked, got {other:?}"),
    }
}
