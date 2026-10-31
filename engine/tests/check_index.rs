use topor_engine::check::{Checker, ItemIndex};
use topor_engine::geom::TrackPrim;
use topor_engine::model::{BoardModel, Item, Options, TrackItem, ViaItem};

fn model() -> BoardModel {
    BoardModel::from_json(
        r#"{ "layers": [ { "id": 0, "routable": true }, { "id": 31, "routable": true } ],
            "nets": [ { "code": 1, "name": "A" }, { "code": 2, "name": "B" } ],
            "outline": [
              { "kind": "seg", "a": [-20000000, -20000000], "b": [20000000, -20000000] },
              { "kind": "seg", "a": [20000000, -20000000], "b": [20000000, 20000000] },
              { "kind": "seg", "a": [20000000, 20000000], "b": [-20000000, 20000000] },
              { "kind": "seg", "a": [-20000000, 20000000], "b": [-20000000, -20000000] } ],
            "rules": { "default": { "clearance": 200000 } } }"#,
    )
    .unwrap()
}

fn seg(net: i32, layer: u32, y_mm: f64, x0: f64, x1: f64) -> Item {
    let y = (y_mm * 1e6) as i32;
    Item::Track(TrackItem {
        net,
        layer,
        width: 250_000,
        prim: TrackPrim::Seg {
            a: [(x0 * 1e6) as i32, y].into(),
            b: [(x1 * 1e6) as i32, y].into(),
        },
        group: None,
        wire: None,
    })
}

fn via(net: i32, x_mm: f64, y_mm: f64, drill: i64) -> Item {
    Item::Via(ViaItem {
        kind: "via".into(),
        net,
        at: [(x_mm * 1e6) as i64, (y_mm * 1e6) as i64].into(),
        d: 600_000,
        drill,
        layers: vec![],
        group: None,
    })
}

fn setup() -> (Checker, ItemIndex) {
    let c = Checker::new(&model(), &Options::default());
    let idx = ItemIndex::new(c.cell());
    (c, idx)
}

#[test]
fn check_against_sees_indexed_items_of_other_nets() {
    let (c, mut idx) = setup();
    assert_eq!(idx.insert(&seg(2, 0, 5.0, 0.0, 10.0)), 0);
    let rep = c.check_against(&idx, &[seg(1, 0, 5.3, 0.0, 10.0)], &[]);
    assert_eq!(rep.violations.len(), 1, "{:?}", rep.violations);
    assert_eq!(rep.violations[0].against, "item:0");
    assert_eq!(rep.violations[0].item, 0);
    assert!(rep.candidates_visited > 0);
}

#[test]
fn check_against_skips_listed_ids_and_same_net() {
    let (c, mut idx) = setup();
    idx.insert(&seg(2, 0, 5.0, 0.0, 10.0));
    let near = seg(1, 0, 5.3, 0.0, 10.0);
    assert_eq!(
        c.check_against(&idx, std::slice::from_ref(&near), &[0])
            .violations
            .len(),
        0
    );
    // another id is skipped, not this one
    assert_eq!(c.check_against(&idx, &[near], &[1]).violations.len(), 1);
    // same net: exempt
    let same = seg(2, 0, 5.3, 0.0, 10.0);
    assert_eq!(c.check_against(&idx, &[same], &[]).violations.len(), 0);
    // other layer: no conflict
    let other = seg(1, 31, 5.3, 0.0, 10.0);
    assert_eq!(c.check_against(&idx, &[other], &[]).violations.len(), 0);
}

#[test]
fn remove_frees_the_slot_and_the_grid() {
    let (c, mut idx) = setup();
    let a = seg(2, 0, 5.0, 0.0, 10.0);
    let near = seg(1, 0, 5.3, 0.0, 10.0);
    let id = idx.insert(&a);
    assert_eq!((id, idx.len()), (0, 1));
    idx.remove(id);
    assert!(idx.is_empty());
    assert_eq!(
        c.check_against(&idx, std::slice::from_ref(&near), &[]).violations.len(),
        0
    );
    idx.remove(id); // double remove: no-op
    let b = seg(2, 0, 7.0, 0.0, 10.0);
    assert_eq!(idx.insert(&b), 0, "slot reused");
    assert_eq!(c.check_against(&idx, &[near], &[]).violations.len(), 0);
}

#[test]
fn via_item_in_index_blocks_tracks_on_every_layer_and_drills() {
    let (c, mut idx) = setup();
    idx.insert(&via(2, 5.0, 5.0, 300_000));
    // copper: 0.35 - 0.3 - 0.125 < 0.205, on layer 31 (via listed no layer: every layer)
    let rep = c.check_against(&idx, &[seg(1, 31, 5.35, 0.0, 10.0)], &[]);
    assert_eq!(rep.violations.len(), 1, "{:?}", rep.violations);
    assert_eq!(rep.violations[0].against, "item:0");
    // hole to hole: 0.4 - 0.3 = 0.1 < 0.255
    let rep = c.check_against(&idx, &[via(1, 5.4, 5.0, 300_000)], &[]);
    assert!(
        rep.violations
            .iter()
            .any(|v| v.against == "item:0" && v.required == 255_000 && v.gap == 100_000),
        "{:?}",
        rep.violations
    );
}

#[test]
fn check_items_equals_check_against_incremental() {
    let (c, _) = setup();
    let mut items = Vec::new();
    // deterministic pseudo-random items packed densely enough to conflict
    let mut s: u64 = 0x2545F4914F6CDD1D;
    let mut rnd = |m: u64| {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s % m
    };
    for _ in 0..20 {
        let (net, layer) = (1 + rnd(2) as i32, [0u32, 31][rnd(2) as usize]);
        let y = 5.0 + rnd(10) as f64 * 0.1;
        let x0 = rnd(4) as f64;
        items.push(if rnd(5) == 0 {
            via(net, x0 + 2.0, y, 300_000)
        } else {
            seg(net, layer, y, x0, x0 + 4.0)
        });
    }
    let all = c.check_items(&items);
    let mut idx = ItemIndex::new(c.cell());
    let (mut viol, mut visited) = (Vec::new(), 0);
    for (i, it) in items.iter().enumerate() {
        let r = c.check_against(&idx, std::slice::from_ref(it), &[]);
        visited += r.candidates_visited;
        viol.extend(r.violations.into_iter().map(|mut v| {
            v.item = i;
            v
        }));
        idx.insert(it);
    }
    assert!(!all.violations.is_empty());
    assert_eq!(all.violations, viol);
    assert_eq!(all.candidates_visited, visited);
}
