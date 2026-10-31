#![allow(clippy::needless_range_loop)]
use std::collections::HashSet;
use topor_engine::geom::{Aabb, Vec2};
use topor_engine::graph::GraphRules;
use topor_engine::model::{BoardModel, Options};
use topor_engine::multigraph::MultiGraph;
use topor_engine::obstacle::ObstacleSet;
use topor_engine::vias::{SiteOrigin, ViaSite, ViaSize};

const MM: f64 = 1_000_000.0;

fn pad(i: usize, net: i32, x: f64, y: f64) -> String {
    format!(
        r#"{{ "uuid": "p{i}", "ref": "R{i}", "number": "1", "net": {net},
          "center": [{}, {}], "layers": [0, 31],
          "shapes": {{ "0": {{ "kind": "circle", "d": 1000000 }}, "31": {{ "kind": "circle", "d": 1000000 }} }} }}"#,
        (x * MM) as i64,
        (y * MM) as i64
    )
}

fn model(extra: &str) -> BoardModel {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "routable": true }}, {{ "id": 31, "routable": true }} ],
          "outline": [
            {{ "kind": "seg", "a": [0, 0], "b": [50000000, 0] }},
            {{ "kind": "seg", "a": [50000000, 0], "b": [50000000, 40000000] }},
            {{ "kind": "seg", "a": [50000000, 40000000], "b": [0, 40000000] }},
            {{ "kind": "seg", "a": [0, 40000000], "b": [0, 0] }} ], {extra},
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000 }} }}"#
    );
    BoardModel::from_json(&s).unwrap()
}

fn site(x_mm: f64, y_mm: f64) -> ViaSite {
    ViaSite {
        at: Vec2 {
            x: x_mm * MM,
            y: y_mm * MM,
        },
        max_d: 600_000,
        max_drill: 300_000,
        origin: SiteOrigin::Face,
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

fn build(m: &BoardModel, sites: Vec<ViaSite>) -> (MultiGraph, ObstacleSet) {
    let mut obs = ObstacleSet::build(m, &Options::default(), &HashSet::new(), &[]);
    let mg = MultiGraph::build(m, &mut obs, sites, &[0, 31], &[], &rules()).unwrap();
    (mg, obs)
}

fn two_pads() -> BoardModel {
    // math frame y = -kicad y: the pads sit at y = -10 mm
    model(&format!(
        r#""pads": [ {}, {} ]"#,
        pad(0, 1, 10.0, 10.0),
        pad(1, 2, 12.6, 10.0)
    ))
}

#[test]
fn sites_are_vertices_of_every_layer() {
    let m = two_pads();
    let sites = vec![site(11.3, -10.5), site(30.0, -20.0), site(5.0, -35.0)];
    let (mg, obs) = build(&m, sites);
    assert_eq!(mg.layers.len(), 2);
    assert_eq!(mg.layer_index(31), Some(1));
    for s in 0..3 {
        for (li, c) in mg.layers.iter().enumerate() {
            let v = mg.site_vertex[s][li];
            assert_eq!(c.g.verts[v].at, mg.sites[s].at);
            assert_eq!(c.g.verts[v].site, Some(s));
            assert_eq!(mg.vertex_of(li, mg.sites[s].at), Some(v));
            assert_eq!((c.g.verts[v].r, c.g.verts[v].clr), (0.0, 0.0));
            assert!(c.g.verts[v].nets.is_empty() && c.g.verts[v].owner.is_none());
            let a = &obs.layer(c.layer).anchors[mg.site_anchor[s][li]];
            assert!(!a.enabled && a.net == 0);
        }
    }
    assert!(MultiGraph::build(
        &m,
        &mut ObstacleSet::build(&m, &Options::default(), &HashSet::new(), &[]),
        vec![site(11.3, -10.5)],
        &[0, 31],
        &[],
        &rules()
    )
    .is_ok());
}

#[test]
fn activate_shrinks_capacity_and_blocks_neighbours() {
    let m = two_pads();
    let (mut mg, mut obs) = build(
        &m,
        vec![site(11.3, -10.5), site(11.7, -10.5), site(30.0, -20.0)],
    );
    let size = ViaSize {
        d: 600_000,
        drill: 300_000,
    };
    let before: Vec<Vec<(usize, f64, f64, u64)>> = (0..2)
        .map(|li| {
            let g = &mg.layers[li].g;
            let v = mg.site_vertex[0][li];
            (0..g.edges.len())
                .filter(|&e| g.edges[e].a == v || g.edges[e].b == v)
                .map(|e| (e, g.edges[e].cap, g.edges[e].c_e, g.edge_version[e]))
                .collect()
        })
        .collect();
    assert!(mg.site_free(0, 1, size));
    mg.activate(0, 0, 1, size, 200_000.0, &mut obs);
    assert!(!mg.site_free(0, 1, size));
    for li in 0..2 {
        let g = &mg.layers[li].g;
        let v = mg.site_vertex[0][li];
        assert_eq!(g.verts[v].nets, vec![1]);
        assert_eq!(g.verts[v].r, 300_000.0);
        assert_eq!(g.verts[v].clr, 200_000.0);
        assert!(!before[li].is_empty());
        for &(e, cap, _, ver) in &before[li] {
            assert!(
                (g.edges[e].cap - (cap - 300_000.0)).abs() < 1e-6,
                "cap drops by r"
            );
            assert!(g.edge_version[e] > ver, "edge_version bumped");
            assert!(g.edges[e].c_e >= 200_000.0);
        }
        let a = &obs
            .layer(g.verts[v].owner.as_ref().map_or(0, |_| mg.layers[li].layer))
            .anchors[mg.site_anchor[0][li]];
        assert!(a.enabled && a.net == 1 && a.r_core == 300_000.0);
    }
    // 0.4 mm away: blocked; far site: not
    assert_eq!(mg.blocked[1], 1);
    assert_eq!(mg.blocked[2], 0);
    assert!(!mg.site_free(1, 2, size));
    assert_eq!(mg.blocked_by(1), &[0]);
    assert!(mg.site_version[1] > 0);
    mg.release(0, &mut obs);
    assert_eq!(mg.blocked[1], 0);
    assert!(mg.site_free(0, 1, size) && mg.site_free(1, 1, size));
    for li in 0..2 {
        let g = &mg.layers[li].g;
        for &(e, cap, c_e, _) in &before[li] {
            assert!((g.edges[e].cap - cap).abs() < 1e-6 && g.edges[e].c_e == c_e);
        }
        let v = mg.site_vertex[0][li];
        assert!(g.verts[v].nets.is_empty() && g.verts[v].owner.is_none() && g.verts[v].r == 0.0);
        let a = &obs.layer(mg.layers[li].layer).anchors[mg.site_anchor[0][li]];
        assert!(!a.enabled && a.net == 0);
    }
}

#[test]
fn in_pour_membership_from_fills() {
    let extra = format!(
        r#""pads": [ {} ],
          "pours": [ {{ "uuid": "z", "net": 1, "layers": [31], "pads": [], "vias": [],
             "fills": [ {{ "layer": 31, "outer": [[5000000, 5000000], [20000000, 5000000],
                                                 [20000000, 20000000], [5000000, 20000000]] }} ] }} ]"#,
        pad(0, 1, 40.0, 30.0)
    );
    let m = model(&extra);
    // kicad (12, 12) -> math (12, -12); (5.1, 12): annulus 0.3 would leave the fill
    let (mg, _) = build(
        &m,
        vec![site(12.0, -12.0), site(30.0, -12.0), site(5.1, -12.0)],
    );
    let by_pos = |x: f64| (0..3).find(|&s| mg.sites[s].at.x == x * MM).unwrap();
    assert_eq!(mg.in_pour[by_pos(12.0)], vec![0]);
    assert!(mg.in_pour[by_pos(30.0)].is_empty());
    assert!(mg.in_pour[by_pos(5.1)].is_empty());
    let near = mg.sites_near(Aabb {
        min: Vec2 {
            x: 0.0,
            y: -40.0 * MM,
        },
        max: Vec2 {
            x: 13.0 * MM,
            y: 0.0,
        },
    });
    assert_eq!(near.len(), 2);
    assert!(near.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn leg_ids_are_allocated_from_the_sorted_free_list() {
    let m = two_pads();
    let (mut mg, mut obs) = build(&m, vec![site(11.3, -10.5)]);
    assert_eq!(mg.alloc_legs(0, 2), vec![0, 1]);
    assert_eq!(mg.alloc_legs(1, 1), vec![2]);
    assert_eq!(mg.conns_of_legs(&[2, 0, 1]), vec![0, 1]);
    mg.remove_conn(0, &mut obs);
    assert_eq!(mg.conns_of_legs(&[0, 1, 2]), vec![1]);
    assert_eq!(mg.alloc_legs(5, 2), vec![0, 1]);
}
