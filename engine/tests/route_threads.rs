//! Thread pool and determinism (M3 / T3.11): `route` gives byte-identical results for every
//! `threads` value, `threads = 0` uses `effective_threads()`, and the parallel self-check equals
//! the sequential one.
use topor_engine::check::{Checker, ItemIndex};
use topor_engine::geom::TrackPrim;
use topor_engine::model::{BoardModel, Item, Options, TrackItem, ViaItem};
use topor_engine::route::{route, RouteResult};

const MM: f64 = 1_000_000.0;

/// 12 nets, two pads each, on a 50 x 40 mm two-layer board; a mix of F.Cu / B.Cu / through-hole
/// pads so that vias and contention occur.
fn mixed_model() -> BoardModel {
    let shape = |l: u32| format!(r#""{l}": {{ "kind": "circle", "d": 900000 }}"#);
    let mut pads = Vec::new();
    for n in 1..=12i64 {
        for end in 0..2i64 {
            let x = 6 + end * 38 + (n * 7 + end * 3) % 5;
            let y = 3 + n * 3;
            let kind = (n + end * 2) % 3;
            let (layers, shapes) = match kind {
                0 => ("[0]", shape(0)),
                1 => ("[31]", shape(31)),
                _ => ("[0, 31]", format!("{}, {}", shape(0), shape(31))),
            };
            pads.push(format!(
                r#"{{ "uuid": "P{n}_{end}", "ref": "P{n}_{end}", "number": "1", "net": {n},
                  "center": [{}, {}], "layers": {layers}, "shapes": {{ {shapes} }} }}"#,
                x * 1_000_000,
                y * 1_000_000
            ));
        }
    }
    let nets: Vec<String> = (1..=12)
        .map(|n| format!(r#"{{ "code": {n}, "name": "N{n}" }}"#))
        .collect();
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {} ],
          "outline": [
            {{ "kind": "seg", "a": [0, 0], "b": [50000000, 0] }},
            {{ "kind": "seg", "a": [50000000, 0], "b": [50000000, 40000000] }},
            {{ "kind": "seg", "a": [50000000, 40000000], "b": [0, 40000000] }},
            {{ "kind": "seg", "a": [0, 40000000], "b": [0, 0] }} ],
          "pads": [ {} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000 }} }}"#,
        nets.join(","),
        pads.join(",")
    );
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"))
}

fn json_no_runtime(r: &RouteResult) -> String {
    let mut v = serde_json::to_value(r).unwrap();
    let o = v.as_object_mut().unwrap();
    o.remove("runtime_s");
    o.remove("threads_used");
    serde_json::to_string(&v).unwrap()
}

#[test]
fn route_threads_byte_identical() {
    let m = mixed_model();
    let run = |threads: usize| {
        route(
            &m,
            &Options {
                threads,
                wall_clock_s: 60.0,
                ..Options::default()
            },
        )
    };
    let r1 = run(1);
    assert_eq!(r1.threads_used, 1);
    assert!(r1.routed >= 6, "routed {}", r1.routed);
    let j1 = json_no_runtime(&r1);
    for n in [2, 4] {
        let r = run(n);
        assert_eq!(r.threads_used, n);
        assert_eq!(json_no_runtime(&r), j1, "threads {n}");
    }
    // run-to-run
    assert_eq!(json_no_runtime(&run(4)), j1);
}

#[test]
fn threads_zero_uses_available_parallelism_minus_one() {
    let m = mixed_model();
    let o = Options {
        threads: 0,
        wall_clock_s: 30.0,
        ..Options::default()
    };
    let r = route(&m, &o);
    assert_eq!(r.threads_used, o.effective_threads());
    assert!(r.threads_used >= 1);
}

#[test]
fn parallel_self_check_matches_sequential() {
    let m = BoardModel::from_json(
        r#"{ "layers": [ { "id": 0, "routable": true }, { "id": 31, "routable": true } ],
            "nets": [ { "code": 1, "name": "A" }, { "code": 2, "name": "B" } ],
            "outline": [
              { "kind": "seg", "a": [-20000000, -20000000], "b": [20000000, -20000000] },
              { "kind": "seg", "a": [20000000, -20000000], "b": [20000000, 20000000] },
              { "kind": "seg", "a": [20000000, 20000000], "b": [-20000000, 20000000] },
              { "kind": "seg", "a": [-20000000, 20000000], "b": [-20000000, -20000000] } ],
            "rules": { "default": { "clearance": 200000 } } }"#,
    )
    .unwrap();
    // deterministic pseudo-random items (LCG)
    let mut st = 12345u64;
    let mut rnd = move |lo: f64, hi: f64| {
        st = st
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        lo + (hi - lo) * ((st >> 33) as f64 / (1u64 << 31) as f64)
    };
    let mut mk = |i: usize| -> Item {
        let net = 1 + (i % 2) as i32;
        if i.is_multiple_of(5) {
            Item::Via(ViaItem {
                kind: "via".into(),
                net,
                at: [
                    (rnd(-15.0, 15.0) * MM) as i64,
                    (rnd(-15.0, 15.0) * MM) as i64,
                ]
                .into(),
                d: 600_000,
                drill: 300_000,
                layers: vec![],
                group: None,
                wire: None,
            })
        } else {
            let (x, y) = (rnd(-15.0, 10.0), rnd(-15.0, 15.0));
            Item::Track(TrackItem {
                net,
                layer: if i.is_multiple_of(2) { 0 } else { 31 },
                width: 250_000,
                prim: TrackPrim::Seg {
                    a: [(x * MM) as i32, (y * MM) as i32].into(),
                    b: [((x + rnd(1.0, 5.0)) * MM) as i32, (y * MM) as i32].into(),
                },
                group: None,
                wire: None,
            })
        }
    };
    let c = Checker::new(&m, &Options::default());
    let mut idx = ItemIndex::new(c.cell());
    for i in 0..120 {
        idx.insert(&mk(i));
    }
    let items: Vec<Item> = (0..200).map(|i| mk(1000 + i)).collect();
    let on = |n: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
            .install(|| c.check_against(&idx, &items, &[]))
    };
    let (a, b) = (on(1), on(4));
    assert_eq!(a, b);
    assert_eq!(a.checked, 200);
    assert!(!a.violations.is_empty(), "dense random items must collide");
}
