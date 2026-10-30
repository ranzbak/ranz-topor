use std::cell::Cell;
use std::collections::HashSet;
use topor_engine::geom::{bitangent, Aabb, Circle, Orient, Prim, Vec2};
use topor_engine::model::{BoardModel, Options};
use topor_engine::obstacle::{LayerObstacles, ObstacleSet};
use topor_engine::taut::{chain_geometry, tauten, AnchorRef, DynObstacle, Env, TautError};

const MM: f64 = 1_000_000.0;
const NET: i32 = 3;
const W: f64 = 250_000.0;

fn v(x: f64, y: f64) -> Vec2 {
    Vec2 {
        x: x * MM,
        y: y * MM,
    }
}

/// Pad JSON; `(x, y)` in math-frame mm (JSON is KiCad frame, y flipped).
fn pad(net: i32, x: f64, y: f64, d: i64) -> String {
    format!(
        r#"{{ "uuid": "p", "ref": "U1", "number": "1", "net": {net},
            "center": [{}, {}], "layers": [0],
            "shapes": {{ "0": {{ "kind": "circle", "d": {d} }} }} }}"#,
        (x * MM) as i64,
        (-y * MM) as i64
    )
}

fn model(pads: &[String], outline: &str, edge_clr: i64) -> BoardModel {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0 }} ], "pads": [ {} ], "outline": [ {outline} ],
             "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }},
             "min_clearance": 0, "hole_clearance": 250000, "edge_clearance": {edge_clr} }} }}"#,
        pads.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

struct TestEnv {
    set: ObstacleSet,
    dyns: Vec<DynObstacle>,
    stack: Option<(AnchorRef, f64)>,
    flip: Option<Cell<u32>>,
}

impl TestEnv {
    fn new(m: &BoardModel) -> Self {
        TestEnv {
            set: ObstacleSet::build(m, &Options::default(), &HashSet::new(), &[]),
            dyns: Vec::new(),
            stack: None,
            flip: None,
        }
    }
}

impl Env for TestEnv {
    fn layer(&self) -> &LayerObstacles {
        self.set.layer(0)
    }
    fn rho_anchor(&self, a: usize) -> f64 {
        if self.flip.is_some() {
            return 1_000.0;
        }
        self.set.rho_anchor(0, a, NET, W)
    }
    fn rho_wall(&self, w: usize) -> f64 {
        self.set.rho_wall(0, w, NET, W)
    }
    fn exempt_anchor(&self, a: usize) -> bool {
        let o = &self.set.layer(0).anchors[a];
        ObstacleSet::exempt(o.net, &o.tie_nets, NET)
    }
    fn exempt_wall(&self, w: usize) -> bool {
        let o = &self.set.layer(0).walls[w];
        ObstacleSet::exempt(o.net, &o.tie_nets, NET)
    }
    fn dyn_near(&self, bb: Aabb) -> Vec<&DynObstacle> {
        if let Some(c) = &self.flip {
            // adversarial: the obstacle is visible on every other query only
            c.set(c.get() + 1);
            if c.get() % 2 == 0 {
                return Vec::new();
            }
        }
        self.dyns
            .iter()
            .filter(|d| d.prim.bbox().overlaps(&bb))
            .collect()
    }
    fn dyn_clearance(&self, d: &DynObstacle) -> f64 {
        200_000.0 + self.set.margin() + 0.5 * W + 0.5 * d.width
    }
    fn stack_radius(&self, a: AnchorRef) -> f64 {
        match self.stack {
            Some((s, r)) if s == a => r,
            _ => self.rho_anchor(a.anchor),
        }
    }
    fn query_grow(&self) -> f64 {
        self.set.query_grow()
    }
}

fn one_pad(x: f64, y: f64, net: i32) -> TestEnv {
    TestEnv::new(&model(&[pad(net, x, y, 800_000)], "", 200_000))
}

fn pad_ref(sigma: Orient) -> AnchorRef {
    AnchorRef { anchor: 0, sigma }
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn query_grow_and_margin_getters() {
    let e = one_pad(5.0, 0.5, 2);
    assert_eq!(e.set.margin(), 5_000.0);
    assert!(e.set.query_grow() >= 250_000.0 + 200_000.0 + 5_000.0);
}

#[test]
fn bitangent_accepts_point_terminals() {
    let c = Circle {
        c: v(5.0, 0.5),
        r: 730_000.0,
    };
    let p = Circle {
        c: v(0.0, 0.0),
        r: 0.0,
    };
    let q = Circle {
        c: v(10.0, 0.0),
        r: 0.0,
    };
    let s = bitangent(p, Orient::Ccw, c, Orient::Ccw).unwrap();
    assert!(near((s.b - c.c).len(), 730_000.0, 1e-3));
    assert!(near((s.a - p.c).len(), 0.0, 1e-9));
    let s = bitangent(p, Orient::Cw, q, Orient::Cw).unwrap();
    assert!(near(s.len(), 10.0 * MM, 1e-6));
}

#[test]
fn straight_when_nothing_blocks() {
    let e = one_pad(5.0, 3.0, 2);
    let c = tauten(v(0.0, 0.0), v(10.0, 0.0), vec![], &e).unwrap();
    assert_eq!(c.prims.len(), 1);
    assert!(matches!(c.prims[0], Prim::Seg(_)));
    assert!(near(c.length, 10.0 * MM, 1.0));
    assert_eq!(c.pattern(), "S");
}

#[test]
fn wraps_one_circle_with_exact_tangents() {
    let e = one_pad(5.0, 0.5, 2);
    let c = tauten(v(0.0, 0.0), v(10.0, 0.0), vec![pad_ref(Orient::Ccw)], &e).unwrap();
    assert_eq!(c.pattern(), "SAS");
    let Prim::Arc(a) = c.prims[1] else { panic!() };
    assert!(near(a.c.x, 5.0 * MM, 1.0) && near(a.c.y, 0.5 * MM, 1.0));
    assert!(near(a.r, 730_000.0, 1.0));
    let (s, t) = (a.start(), a.end());
    assert!(near(s.x, 4_966_350.0, 200.0) && near(s.y, -229_220.0, 200.0));
    assert!(near(t.x, 5_033_650.0, 200.0) && near(t.y, -229_220.0, 200.0));
    assert!(near(c.length, 10_010_598.0, 200.0));
}

#[test]
fn slack_anchor_is_dropped() {
    let e = one_pad(5.0, 3.0, 2);
    let c = tauten(v(0.0, 0.0), v(10.0, 0.0), vec![pad_ref(Orient::Ccw)], &e).unwrap();
    assert!(c.seq.is_empty());
    assert_eq!(c.pattern(), "S");
}

#[test]
fn penetrating_obstacle_is_added() {
    let e = one_pad(5.0, 0.5, 2);
    let c = tauten(v(0.0, 0.0), v(10.0, 0.0), vec![], &e).unwrap();
    assert_eq!(c.seq, vec![pad_ref(Orient::Ccw)]);
    assert_eq!(c.pattern(), "SAS");
    assert!(near(c.length, 10_010_598.0, 200.0));
    let e = one_pad(5.0, -0.5, 2);
    let c = tauten(v(0.0, 0.0), v(10.0, 0.0), vec![], &e).unwrap();
    assert_eq!(c.seq, vec![pad_ref(Orient::Cw)]);
}

fn wall_env(y: f64) -> TestEnv {
    let (a, b) = ((-5.0 * MM) as i64, (4.0 * MM) as i64);
    let yy = (-y * MM) as i64;
    let outline = format!(r#"{{ "kind": "seg", "a": [{a}, {yy}], "b": [{b}, {yy}] }}"#);
    TestEnv::new(&model(&[], &outline, 500_000))
}

#[test]
fn wall_clip_wraps_endpoint() {
    let (s, g) = (v(-3.0, 0.7), v(10.0, 0.3));
    let e = wall_env(-0.9);
    let c = tauten(s, g, vec![], &e).unwrap();
    assert_eq!(c.pattern(), "S");
    let e = wall_env(-0.1);
    let c = tauten(s, g, vec![], &e).unwrap();
    assert_eq!(c.pattern(), "SAS");
    assert_eq!(c.seq.len(), 1);
    assert_eq!(c.seq[0].sigma, Orient::Cw);
    let a = &e.layer().anchors[c.seq[0].anchor];
    assert!(near(a.c.x, 4.0 * MM, 1.0) && near(a.c.y, -0.1 * MM, 1.0));
    assert!(near(c.length, 13_006_475.0, 200.0), "{}", c.length);
    assert!(c.length > 13_006_152.0);
}

#[test]
fn infeasible_gap_is_error() {
    let m = model(
        &[pad(2, 5.0, 0.55, 800_000), pad(2, 5.0, -0.55, 800_000)],
        "",
        200_000,
    );
    let e = TestEnv::new(&m);
    let seq = vec![
        AnchorRef {
            anchor: 0,
            sigma: Orient::Ccw,
        },
        AnchorRef {
            anchor: 1,
            sigma: Orient::Cw,
        },
    ];
    let r = chain_geometry(v(0.0, 0.0), v(10.0, 0.0), &seq, &e);
    assert!(matches!(r, Err(TautError::Infeasible(_))), "{r:?}");
    let r = tauten(v(0.0, 0.0), v(10.0, 0.0), seq, &e);
    assert!(matches!(r, Err(TautError::Infeasible(_))));
}

#[test]
fn tauten_terminal_inside_foreign_obstacle_is_error() {
    let e = TestEnv::new(&model(&[pad(2, 0.3, 0.0, 800_000)], "", 200_000));
    match tauten(v(0.0, 0.0), v(10.0, 0.0), vec![], &e) {
        Err(TautError::InsideObstacle(l)) => assert!(l.contains("pad"), "{l}"),
        r => panic!("{r:?}"),
    }
}

#[test]
fn exempt_anchor_is_never_kept() {
    let e = one_pad(5.0, 0.5, NET);
    let c = tauten(v(0.0, 0.0), v(10.0, 0.0), vec![pad_ref(Orient::Ccw)], &e).unwrap();
    assert!(c.seq.is_empty());
    assert_eq!(c.pattern(), "S");
}

#[test]
fn dyn_arc_adds_its_anchor_with_stack_radius() {
    let mut e = one_pad(5.0, 0.5, 2);
    let arc = topor_engine::geom::Arc::from_tangent_points(
        v(5.0, 0.5),
        730_000.0,
        Orient::Ccw,
        v(4.9, -0.2),
        v(5.1, -0.2),
    );
    e.dyns.push(DynObstacle {
        prim: Prim::Arc(arc),
        width: W,
        net: 2,
        wire: 1,
        anchor: Some(pad_ref(Orient::Ccw)),
        adjacent: [None, None],
    });
    e.stack = Some((pad_ref(Orient::Ccw), 1_185_000.0));
    let c = tauten(v(0.0, 0.0), v(10.0, 0.0), vec![], &e).unwrap();
    assert_eq!(c.pattern(), "SAS");
    let Prim::Arc(a) = c.prims[1] else { panic!() };
    assert!(near(a.r, 1_185_000.0, 1.0));
    assert_eq!(c.seq, vec![pad_ref(Orient::Ccw)]);
}

#[test]
fn pass_cap_returns_err_cap() {
    // static rho ~ 0; a dyn arc visible on every other query: add, drop, add, ...
    let mut e = one_pad(5.0, 0.5, 2);
    e.stack = Some((pad_ref(Orient::Ccw), 100_000.0));
    let arc = topor_engine::geom::Arc::from_tangent_points(
        v(5.0, 0.5),
        730_000.0,
        Orient::Ccw,
        v(4.9, -0.2),
        v(5.1, -0.2),
    );
    e.dyns.push(DynObstacle {
        prim: Prim::Arc(arc),
        width: W,
        net: 2,
        wire: 1,
        anchor: Some(AnchorRef {
            anchor: 0,
            sigma: Orient::Ccw,
        }),
        adjacent: [None, None],
    });
    e.flip = Some(Cell::new(0));
    let r = tauten(v(0.0, 0.0), v(10.0, 0.0), vec![], &e);
    assert_eq!(r.map(|c| c.length), Err(TautError::Cap));
}

#[test]
fn chain_is_g1_and_alternating() {
    let e = one_pad(5.0, 0.5, 2);
    let c = tauten(v(0.0, 0.0), v(10.0, 0.0), vec![], &e).unwrap();
    let p = c.pattern();
    assert!(!p.contains("SS") && !p.contains("AA"));
    let tan = |q: &Prim, end: bool| -> Vec2 {
        match q {
            Prim::Seg(s) => s.dir().unwrap(),
            Prim::Arc(a) => {
                let pt = if end { a.end() } else { a.start() };
                let r = (pt - a.c).unit().unwrap().perp();
                if a.sigma == Orient::Ccw {
                    r
                } else {
                    -r
                }
            }
        }
    };
    let ends = |q: &Prim| -> (Vec2, Vec2) {
        match q {
            Prim::Seg(s) => (s.a, s.b),
            Prim::Arc(a) => (a.start(), a.end()),
        }
    };
    for w in c.prims.windows(2) {
        assert!((ends(&w[0]).1 - ends(&w[1]).0).len() < 1.0);
        let (t0, t1) = (tan(&w[0], true), tan(&w[1], false));
        assert!(t0.cross(t1).abs() < 1e-6 && t0.dot(t1) > 0.0);
    }
}

#[test]
fn deterministic() {
    let e = one_pad(5.0, 0.5, 2);
    let a = tauten(v(0.0, 0.0), v(10.0, 0.0), vec![], &e).unwrap();
    let b = tauten(v(0.0, 0.0), v(10.0, 0.0), vec![], &e).unwrap();
    assert_eq!(a.prims, b.prims);
    assert_eq!(a.seq, b.seq);
}

#[test]
fn negative_arc_is_dropped() {
    // eprom wire 19 (A12): the via arc's exit tangent lies behind its entry (wrap -6 deg);
    // it must be dropped, not kept as a 354 deg loop.
    let pads = [
        pad(7, 187.5, -71.0, 1_600_000),
        pad(7, 189.9, -72.4, 800_000),
        pad(7, 201.24, -77.41, 1_600_000),
        pad(2, 187.5, -73.54, 800_000),
        pad(2, 201.24, -72.33, 800_000),
    ];
    let e = TestEnv::new(&model(&pads, "", 200_000));
    let seq = vec![
        AnchorRef {
            anchor: 0,
            sigma: Orient::Cw,
        },
        AnchorRef {
            anchor: 1,
            sigma: Orient::Cw,
        },
        AnchorRef {
            anchor: 2,
            sigma: Orient::Ccw,
        },
    ];
    let c = tauten(v(186.0, -69.79), v(202.74, -78.62), seq, &e).unwrap();
    assert_eq!(c.pattern(), "SASAS", "{:?}", c.seq);
    for p in &c.prims {
        if let Prim::Arc(a) = p {
            assert!(a.sweep <= std::f64::consts::PI, "loop arc {}", a.sweep);
        }
    }
    assert!(c.length < 20.0 * MM, "{}", c.length);
}

#[test]
fn wall_grazed_mid_span_fails_fast() {
    // Wall y = 0 from x = -5 to 15. Chain wraps a pad above it and dips to y = 0.62 mm at x = 5,
    // 10 um inside rho_wall (0.63); both wall ends are ~5 mm away, so no end anchor covers it.
    let outline = format!(
        r#"{{ "kind": "seg", "a": [{}, 0], "b": [{}, 0] }}"#,
        (-5.0 * MM) as i64,
        (15.0 * MM) as i64
    );
    let m = model(&[pad(2, 5.0, 1.35, 800_000)], &outline, 500_000);
    let e = TestEnv::new(&m);
    let r = tauten(v(0.0, 0.7), v(10.0, 0.7), vec![], &e);
    match r {
        Err(TautError::Infeasible(s)) => {
            assert!(s.contains("mid-span"), "{s}");
            assert!(s.contains("nm"), "{s}");
        }
        other => panic!("{other:?}"),
    }
}
