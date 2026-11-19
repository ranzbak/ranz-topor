//! loop-local tests (docs/dev/design/loop-local.md).
//!
//! Step A: the per-wire stretch cache of the layerings ([`layers::StretchCache`]) gives the same
//! report bit for bit as no cache, over a sequence of states and labellings in which each of the
//! inputs it keys on changes alone (labels, a wire's path, a neighbour's polyline, the crossed
//! chords of an end site, the parameters).
//!
//! Step B: the local layering ([`layers::assign_local`]) holds every wire outside the free set,
//! separates (or counts as unlayerable) every crossing of a free wire with a held one, equals the
//! whole-board layering when every wire is free, and leaves a two-layer board's vias as they were
//! for a null move; on a routed board the local judge layers the whole board only a few times
//! per cycle, and the shadow check compares it with the whole-board judge.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use topor_engine::model::BoardModel;
use topor_engine::topo::board::{self, Board, BuildOpts};
use topor_engine::topo::init::{Conn, InitWeights};
use topor_engine::control::RunControl;
use topor_engine::model::Options;
use topor_engine::route::route_with;
use topor_engine::topo::layers::{self, Held, LayerParams, LayerReport, StretchCache};
use topor_engine::topo::state::{State, WireId};

const MM: f64 = 1e6;

/// A board `w` x `h` mm with `n_layers` copper layers (2, or 4 with In1.Cu and In2.Cu), SMD rect
/// pads `(ref, net, x, y, w, h, layer)` (layer 0 = F.Cu, 2 = B.Cu, 99 = both) and nets
/// 1..=`nets` (as tests/topo_d6b.rs).
fn board_layers(w: f64, h: f64, nets: usize, pads: &[(&str, i32, f64, f64, f64, f64, u32)], n_layers: usize) -> BoardModel {
    let layers = if n_layers == 4 {
        r#"{"id":0,"name":"F.Cu","kind":"signal"},{"id":4,"name":"In1.Cu","kind":"signal"},{"id":6,"name":"In2.Cu","kind":"signal"},{"id":2,"name":"B.Cu","kind":"signal"}"#
    } else {
        r#"{"id":0,"name":"F.Cu","kind":"signal"},{"id":2,"name":"B.Cu","kind":"signal"}"#
    };
    let p = |v: f64| (v * MM) as i64;
    let c = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads: Vec<String> = pads
        .iter()
        .map(|&(r, net, x, y, w, h, l)| {
            let ls: Vec<u32> = if l == 99 { vec![0, 2] } else { vec![l] };
            let shape = format!(r#"{{"kind":"rect","w":{},"h":{}}}"#, p(w), p(h));
            let shapes: Vec<String> = ls.iter().map(|l| format!(r#""{l}":{shape}"#)).collect();
            let ls: Vec<String> = ls.iter().map(|l| l.to_string()).collect();
            format!(
                r#"{{"uuid":"{r}","ref":"{r}","number":"1","net":{net},"attr":"smd","center":[{},{}],"rotation_deg":0.0,
                    "layers":[{}],"shapes":{{{}}}}}"#,
                p(x),
                p(y),
                ls.join(","),
                shapes.join(",")
            )
        })
        .collect();
    let nets: Vec<String> = (1..=nets).map(|i| format!(r#"{{"code":{i},"name":"N{i}"}}"#)).collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{layers}],
            "nets":[{}],
            "pads":[{}],"outline":[{}],
            "rules":{{"default":{{"clearance":200000,"width":250000}},"min_connection":127000,"max_error":5000,"per_net":{{}}}}}}"#,
        nets.join(","),
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn ob(m: &BoardModel, b: &Board, r: &str) -> usize {
    let i = m.pads.iter().position(|p| p.ref_ == r).unwrap();
    b.pad_of.iter().position(|&x| x == i).unwrap()
}

fn conn(m: &BoardModel, b: &Board, net: i32, ra: &str, rb: &str) -> Conn {
    let (pa, pb) = (ob(m, b, ra), ob(m, b, rb));
    Conn { net, pa, pb, a: b.centre[pa].unwrap(), b: b.centre[pb].unwrap(), w: 0.25 * MM, s: 0.2 * MM }
}

/// The parts of a report the stretch pass writes, as exact text (Debug of f64 round-trips).
fn stretch_text(r: &LayerReport) -> String {
    format!("{:?}|{:?}|{:?}|{:?}|{}|{}", r.stretches, r.events, r.via_pts, r.slots, r.vias, r.vias_no_site)
}

/// Random labels on the events of `base` (seeded): every event gets layer 0 or 1.
fn shuffled(base: &LayerReport, rng: &mut ChaCha8Rng, flip: f64) -> Vec<Vec<(usize, f64, i32)>> {
    base.events.iter().map(|ev| ev.iter().map(|&(k, s, l)| (k, s, if rng.gen_bool(flip) { 1 - l } else { l })).collect()).collect()
}

/// A crossing fan: three west-east nets and three north-south nets on a 2-layer board, plus a
/// later net D that shares the edges of the A wires (their polylines shift when it is laid).
fn fan() -> (BoardModel, Board) {
    fan_layers(2)
}

/// [`fan`] on `n_layers` copper layers.
fn fan_layers(n_layers: usize) -> (BoardModel, Board) {
    let m = board_layers(30.0, 20.0, 8, &fan_pads(), n_layers);
    let b = board::build(&m, &BuildOpts { existing_vias: true, keepouts: true, copper_shapes: true, ..BuildOpts::default() }).unwrap();
    (m, b)
}

/// The fan's pads.
fn fan_pads() -> Vec<(&'static str, i32, f64, f64, f64, f64, u32)> {
    let mut pads = vec![
        ("A1", 1, 2.0, 6.0, 1.0, 1.0, 0),
        ("A2", 1, 28.0, 6.0, 1.0, 1.0, 0),
        ("C1", 3, 2.0, 14.0, 1.0, 1.0, 2),
        ("C2", 3, 28.0, 14.0, 1.0, 1.0, 0),
        ("E1", 5, 2.0, 10.0, 1.0, 1.0, 0),
        ("E2", 5, 28.0, 11.0, 1.0, 1.0, 2),
        ("B1", 2, 9.0, 1.5, 1.0, 1.0, 2),
        ("B2", 2, 10.0, 18.5, 1.0, 1.0, 2),
        ("F1", 6, 16.0, 1.5, 1.0, 1.0, 0),
        ("F2", 6, 15.0, 18.5, 1.0, 1.0, 2),
        ("G1", 7, 22.0, 1.5, 1.0, 1.0, 0),
        ("G2", 7, 23.0, 18.5, 1.0, 1.0, 0),
        ("D1", 4, 2.0, 7.5, 1.0, 1.0, 0),
        ("D2", 4, 28.0, 7.5, 1.0, 1.0, 0),
    ];
    // A foreign pad on each layer in the middle (no via site there).
    pads.push(("K0", 8, 12.5, 10.0, 2.0, 2.0, 0));
    pads.push(("K1", 8, 19.5, 8.0, 2.0, 2.0, 2));
    pads
}

/// Step A (Rule 9b): over a sequence of states (wires laid one by one, a wire taken out and laid
/// again, a wire laid along the A wires' edges, a snapshot restored and a wire laid under the same
/// id along another path) and, per state, random labellings (also the previous state's own
/// labels, so a polyline changes under unchanged events), both `between_crossings` settings and
/// two via sizes far apart, the cached stretch pass equals the uncached one bit for bit, and the
/// cache is both hit (untouched wires) and missed (changed ones).
#[test]
fn stretch_cache_equals_no_cache_over_mutations() {
    let (m, b) = fan();
    let wt = InitWeights::default();
    let mut st = State::new(&b.proj);
    let cache = StretchCache::default();
    let mut rng = ChaCha8Rng::seed_from_u64(7);
    let smd = |o: usize| b.smd_layer(o);
    let mut checked = 0usize;
    let mut prev: Vec<Vec<(usize, f64, i32)>> = Vec::new();
    let mut check = |st: &State, rng: &mut ChaCha8Rng, prev: &mut Vec<Vec<(usize, f64, i32)>>| {
        for (between, r) in [(true, 0.5e6), (false, 0.5e6), (true, 0.05e6), (true, 3.0e6), (true, 0.5e6)] {
            let p = LayerParams { via_between: between, via_site_r: r, ..LayerParams::default() };
            let base = layers::assign(st, &smd, &p);
            let cached = layers::assign_cached(st, &smd, &p, None, Some(&cache));
            assert_eq!(stretch_text(&base), stretch_text(&cached), "assign: between {between} r {r}");
            // The previous state's labels where they still fit the wire (same pass count).
            let old: Vec<Vec<(usize, f64, i32)>> = (0..st.wires.len())
                .map(|w| match prev.get(w) {
                    Some(e) if st.wires[w].alive && e.iter().all(|x| x.0 <= st.wires[w].passes.len()) => e.clone(),
                    _ => base.events[w].clone(),
                })
                .collect();
            let mut lists = vec![old];
            for flip in [0.0, 0.2, 0.5, 0.5] {
                lists.push(shuffled(&base, rng, flip));
            }
            for evl in &lists {
                let (mut a, mut c) = (base.clone(), base.clone());
                layers::relabel(st, evl, &p, &mut a);
                layers::relabel_cached(st, evl, &p, &mut c, Some(&cache));
                assert_eq!(stretch_text(&a), stretch_text(&c), "relabel: between {between} r {r}");
                // The counts agree with the lists (every via has its point and slot).
                assert_eq!((a.vias, a.via_pts.len()), (a.slots.len(), a.slots.len()));
                assert!(a.vias_no_site <= a.vias);
                checked += 1;
            }
            *prev = base.events.clone();
        }
    };
    let order = [(1, "A1", "A2"), (2, "B1", "B2"), (3, "C1", "C2"), (6, "F1", "F2"), (5, "E1", "E2"), (7, "G1", "G2")];
    for &(net, ra, rb) in &order {
        st.route_conn(&conn(&m, &b, net, ra, rb), &wt, 1.0).expect("routed");
        check(&st, &mut rng, &mut prev);
    }
    // Take B out and lay it again after G (its triangles and the others' polylines change).
    let wb = (0..st.wires.len()).find(|&w| st.wires[w].alive && st.wires[w].net == 2).unwrap();
    topor_engine::topo::tree::remove_wire(&mut st, wb);
    check(&st, &mut rng, &mut prev);
    st.route_conn(&conn(&m, &b, 2, "B1", "B2"), &wt, 3.0).expect("B again");
    check(&st, &mut rng, &mut prev);
    // D along the A wire: A's passes move on the shared edges, A itself is not touched.
    let t0 = st.topology();
    st.route_conn(&conn(&m, &b, 4, "D1", "D2"), &wt, 1.0).expect("D routed");
    check(&st, &mut rng, &mut prev);
    // Back to before D, and D laid again under the same wire id along a costlier path.
    st.restore(t0);
    check(&st, &mut rng, &mut prev);
    st.route_conn(&conn(&m, &b, 4, "D1", "D2"), &wt, 6.0).expect("D again");
    check(&st, &mut rng, &mut prev);
    let (hits, misses) = cache.stats();
    assert!(checked >= 200, "{checked} labellings checked");
    assert!(hits > 0 && misses > 0, "hits {hits} misses {misses}");
}

/// Step A (Rule 9b): each input the cache keys on, changed alone on one wire of a state the cache
/// has seen (its net, one triangle, a turn, one label, the polyline under unchanged events, the
/// via size), changes the uncached result for at least one wire, and the cached pass then still
/// equals the uncached one (so the key holds that input).
#[test]
fn stretch_cache_sees_each_input_alone() {
    let (m, b) = fan();
    let wt = InitWeights::default();
    let mut st = State::new(&b.proj);
    for (net, ra, rb) in [(1, "A1", "A2"), (2, "B1", "B2"), (3, "C1", "C2"), (6, "F1", "F2"), (5, "E1", "E2"), (7, "G1", "G2")] {
        st.route_conn(&conn(&m, &b, net, ra, rb), &wt, 1.0).expect("routed");
    }
    let smd = |o: usize| b.smd_layer(o);
    let p = LayerParams { via_between: true, via_site_r: 0.5e6, ..LayerParams::default() };
    let base = layers::assign(&st, &smd, &p);
    let mut rng = ChaCha8Rng::seed_from_u64(11);
    let evl = shuffled(&base, &mut rng, 0.5);
    let live: Vec<usize> = (0..st.wires.len()).filter(|&w| st.wires[w].alive).collect();
    // One check: warm the cache on (st, evl, p), then the changed input (st2, evl2, p2) must give
    // the uncached result; whether that result differs from the unchanged one.
    // `p0`: the parameters of the warm-up (the changed input's own, except for the params check).
    let differs_full = |evl0: &[Vec<(usize, f64, i32)>], st2: &State, evl2: &[Vec<(usize, f64, i32)>], p0: &LayerParams, p2: &LayerParams| -> bool {
        let cache = StretchCache::default();
        let (mut w0, mut a, mut c) = (base.clone(), base.clone(), base.clone());
        layers::relabel_cached(&st, evl0, p0, &mut w0, Some(&cache));
        layers::relabel(st2, evl2, p2, &mut a);
        layers::relabel_cached(st2, evl2, p2, &mut c, Some(&cache));
        assert_eq!(stretch_text(&a), stretch_text(&c), "cached != uncached after a change");
        stretch_text(&a) != stretch_text(&w0)
    };
    let differs_at = |st2: &State, evl2: &[Vec<(usize, f64, i32)>], p0: &LayerParams, p2: &LayerParams| differs_full(&evl, st2, evl2, p0, p2);
    let differs = |st2: &State, evl2: &[Vec<(usize, f64, i32)>], p2: &LayerParams| differs_at(st2, evl2, &p, p2);
    let mut found: Vec<&str> = Vec::new();
    // Net: another net's obstacles become own ones (the free radius changes).
    let radii = [0.05e6, 0.2e6, 0.5e6, 1.0e6, 1.5e6, 2.0e6, 3.0e6];
    if live.iter().any(|&w| [8, 1, 2, 3].iter().any(|&n| n != st.wires[w].net && radii.iter().any(|&r| {
        let mut s2 = st.clone();
        s2.wires[w].net = n;
        let pr = LayerParams { via_site_r: r, ..p };
        differs_at(&s2, &evl, &pr, &pr)
    }))) {
        found.push("net");
    }
    // One triangle of the wire (a site centroid, an end-site triangle).
    let nt = b.proj.tris.len();
    if live.iter().any(|&w| (0..st.wires[w].tris.len()).any(|k| (1..4).any(|d| {
        let mut s2 = st.clone();
        s2.wires[w].tris[k] = (s2.wires[w].tris[k] + d * 7) % nt;
        differs(&s2, &evl, &p)
    }))) {
        found.push("tris");
    }
    // A turn in a traversal (an end site skips it): a via between two events inside neighbouring
    // traversals (no traversal strictly between, so its site is an end site).
    if live.iter().any(|&w| (0..st.wires[w].tris.len().saturating_sub(1)).any(|k| {
        let mut e = evl.clone();
        e[w] = vec![(k, 0.5, 0), (k + 1, 0.5, 1)];
        let mut s2 = st.clone();
        s2.wires[w].turns = vec![(k, st.wires[w].a), (k + 1, st.wires[w].a)];
        let pr = LayerParams { via_site_r: 0.05e6, ..p };
        differs_full(&e, &s2, &e, &pr, &pr)
    })) {
        found.push("turns");
    }
    // One label.
    if live.iter().any(|&w| (0..evl[w].len()).any(|i| {
        let mut e2 = evl.clone();
        e2[w][i].2 = 1 - e2[w][i].2;
        differs(&st, &e2, &p)
    })) {
        found.push("ev");
    }
    // The via size (the cache's parameters).
    if [0.05e6, 3.0e6].iter().any(|&r| differs(&st, &evl, &LayerParams { via_site_r: r, ..p })) {
        found.push("params");
    }
    // The polyline under unchanged events: D laid along A moves A's passes.
    {
        let mut s2 = st.clone();
        s2.route_conn(&conn(&m, &b, 4, "D1", "D2"), &wt, 1.0).expect("D routed");
        let mut e2 = evl.clone();
        e2.resize(s2.wires.len(), Vec::new());
        if differs(&s2, &e2, &p) {
            found.push("poly");
        }
    }
    assert_eq!(found, ["net", "tris", "turns", "ev", "params", "poly"], "inputs whose change was seen");
}

/// The fan's wires laid (A, B, C, F, E, G) on `n_layers`.
fn laid(n_layers: usize) -> (BoardModel, &'static Board, State<'static>) {
    laid_on(n_layers, false)
}

/// [`laid`]; `both` = every fan pad on both layers (as THT pads: no event is fixed to a layer).
fn laid_on(n_layers: usize, both: bool) -> (BoardModel, &'static Board, State<'static>) {
    let (m, b) = if both {
        let pads: Vec<_> = fan_pads().into_iter().map(|(r, n, x, y, w, h, _)| (r, n, x, y, w, h, 99)).collect();
        let m = board_layers(30.0, 20.0, 8, &pads, n_layers);
        let b = board::build(&m, &BuildOpts { existing_vias: true, keepouts: true, copper_shapes: true, ..BuildOpts::default() }).unwrap();
        (m, b)
    } else {
        fan_layers(n_layers)
    };
    let b: &'static Board = Box::leak(Box::new(b));
    let wt = InitWeights::default();
    let mut st = State::new(&b.proj);
    for (net, ra, rb) in [(1, "A1", "A2"), (2, "B1", "B2"), (3, "C1", "C2"), (6, "F1", "F2"), (5, "E1", "E2"), (7, "G1", "G2")] {
        st.route_conn(&conn(&m, b, net, ra, rb), &wt, 1.0).expect("routed");
    }
    (m, b, st)
}

/// The free set of a move (the core's rule, depth 1): the new wires, the different-net wires the
/// gone ones crossed in `prev`, and the different-net wires the new ones cross.
fn free_of(st: &State, prev: &LayerReport, gone: &[WireId], new: &[WireId]) -> Vec<bool> {
    let mut free = vec![false; st.wires.len()];
    for &n in new {
        free[n] = true;
        for x in st.crossings(n) {
            if st.wires[x.other].net != st.wires[n].net {
                free[x.other] = true;
            }
        }
    }
    for &o in gone {
        for &(p, _) in prev.partner[o].iter().flatten() {
            if st.wires[p].alive && st.wires[p].net != st.wires[o].net {
                free[p] = true;
            }
        }
    }
    free
}

/// Checks the local layering `loc` of `st` against `prev` (the layering before the move) for the
/// free set `free`: held wires keep events and masks (and partners among held wires), every
/// crossing of a free wire with a held one is on two layers or unlayerable.
fn check_local(st: &State, prev: &LayerReport, loc: &LayerReport, free: &[bool], what: &str) -> usize {
    let held = |w: WireId| st.wires[w].alive && !free.get(w).copied().unwrap_or(true);
    let mut links = 0usize;
    for w in (0..st.wires.len()).filter(|&w| held(w)) {
        assert_eq!(loc.events[w], prev.events[w], "{what}: held wire {w} keeps its events");
        assert_eq!(loc.mask[w], prev.mask[w], "{what}: held wire {w} keeps its masks");
        for (i, p) in prev.partner[w].iter().enumerate() {
            if let Some((o, _)) = p.filter(|&(o, _)| held(o)) {
                assert_eq!(loc.partner[w][i], *p, "{what}: held wire {w} event {i} keeps its held partner {o}");
            }
        }
    }
    let unl = |a: WireId, c: WireId| loc.unlayerable_w.contains(&(a.min(c), a.max(c)));
    for w in (0..st.wires.len()).filter(|&w| st.wires[w].alive && !held(w)) {
        let mut to_held: Vec<WireId> = Vec::new();
        for (i, p) in loc.partner[w].iter().enumerate() {
            if let Some((h, j)) = p.filter(|&(h, _)| held(h)) {
                links += 1;
                to_held.push(h);
                let (lw, lh) = (loc.events[w][i].2, loc.events[h][j].2);
                assert!(lw != lh || unl(w, h), "{what}: free {w} event {i} and held {h} event {j} on layer {lw}");
                assert_eq!(loc.partner[h][j], Some((w, i)), "{what}: the held event points back");
            }
        }
        for x in st.crossings(w).into_iter().filter(|x| held(x.other) && st.wires[x.other].net != st.wires[w].net) {
            assert!(to_held.contains(&x.other) || unl(w, x.other), "{what}: free {w} crosses held {} without a linked event", x.other);
        }
    }
    assert_eq!(loc.same_layer, loc.unlayerable.len(), "{what}: every one-layer crossing is counted");
    assert_eq!(loc.unlayerable.len(), loc.unlayerable_w.len(), "{what}: one wire pair per unlayerable crossing");
    links
}

/// Step B: a move (B taken out and laid again along a costlier path) layered locally on 2 and 4
/// layers holds the wires outside F, links every crossing of F with a held wire, and equals the
/// whole-board layering when every wire is free.
#[test]
fn local_layering_holds_the_rest_and_equals_global_when_all_free() {
    for n_layers in [2, 4] {
        let (m, b, mut st) = laid(n_layers);
        let smd = |o: usize| b.smd_layer(o);
        let p = LayerParams { via_between: true, via_site_r: 0.5e6, ..LayerParams::default() };
        let prev = layers::assign(&st, &smd, &p);
        let wb = (0..st.wires.len()).find(|&w| st.wires[w].alive && st.wires[w].net == 2).unwrap();
        topor_engine::topo::tree::remove_wire(&mut st, wb);
        let n0 = st.wires.len();
        st.route_conn(&conn(&m, b, 2, "B1", "B2"), &InitWeights::default(), 3.0).expect("B again");
        let new: Vec<WireId> = (n0..st.wires.len()).collect();
        let free = free_of(&st, &prev, &[wb], &new);
        assert!(free.iter().filter(|&&f| f).count() < st.wires.iter().filter(|w| w.alive).count(), "{n_layers} layers: some wires held");
        let loc = layers::assign_local(&st, &smd, &p, None, None, &Held { prev: &prev, free: &free });
        let links = check_local(&st, &prev, &loc, &free, &format!("{n_layers} layers, B moved"));
        assert!(links > 0, "{n_layers} layers: the move has crossings of free and held wires");
        // F = every wire: the whole-board layering.
        let all = vec![true; st.wires.len()];
        let g = layers::assign(&st, &smd, &p);
        let la = layers::assign_local(&st, &smd, &p, None, None, &Held { prev: &prev, free: &all });
        assert_eq!(la.events, g.events, "{n_layers} layers: all free = global (events)");
        assert_eq!((la.vias, la.unlayerable.len(), la.same_layer), (g.vias, g.unlayerable.len(), g.same_layer), "{n_layers} layers: all free = global (counts)");
        assert_eq!(stretch_text(&la), stretch_text(&g), "{n_layers} layers: all free = global (stretches)");
    }
}

/// Step B: a null move (nothing changed, F = one wire and its crossing partners) on two layers
/// keeps the vias and the unlayerable crossings of the whole-board layering (the exact solve on
/// F with the rest held cannot beat the whole-board optimum and has its labels to take), and
/// holds the rest; on four layers it holds the rest.
#[test]
fn local_layering_of_a_null_move_changes_nothing() {
    for n_layers in [2, 4] {
        let (_m, b, st) = laid(n_layers);
        let smd = |o: usize| b.smd_layer(o);
        let p = LayerParams { via_between: true, via_site_r: 0.5e6, ..LayerParams::default() };
        let prev = layers::assign(&st, &smd, &p);
        let mut links = 0;
        for w in (0..st.wires.len()).filter(|&w| st.wires[w].alive) {
            let free = free_of(&st, &prev, &[w], &[w]);
            let loc = layers::assign_local(&st, &smd, &p, None, None, &Held { prev: &prev, free: &free });
            links += check_local(&st, &prev, &loc, &free, &format!("{n_layers} layers, null move of {w}"));
            if n_layers == 2 {
                assert_eq!((loc.vias, loc.unlayerable.len()), (prev.vias, prev.unlayerable.len()), "null move of wire {w}");
            }
        }
        // F holds every crossing partner of the moved wire, so links to held wires come only
        // from the partners' own crossings.
        eprintln!("{n_layers} layers: null moves, {links} links to held wires");
    }
}

/// The options of a short TopoR loop on the fan (`extra`: JSON fields).
fn loop_opts(extra: &str) -> Options {
    let mut o: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 3, "runs": 1, "seed": 3}"#).unwrap();
    let more: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&format!("{{{extra}}}")).unwrap();
    o.extend(more);
    Options::from_json(&serde_json::Value::Object(o).to_string()).unwrap()
}

/// Every TW5 option at its TW5 value (off by default until qmtech is analysed, docs/dev/design/tw5-escape.md).
const TW5: &str = r#""oscillation": "bounded", "escape": "layering", "escape_take": "always", "relay_count_fade": 0.8, "loop_history": true, "float_wire": true, "preloop_p4": false"#;

/// The TW5 escape report line ("TopoR loop escapes (TW5): ...").
fn escape_line(m: &BoardModel, o: &Options) -> String {
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let r = route_with(m, o, &ctl);
    r.warnings.iter().find(|w| w.starts_with("TopoR loop escapes")).unwrap_or_else(|| panic!("escape line: {:?}", r.warnings)).clone()
}

/// The loop judge's report line: (whole-board layerings per cycle, local layerings per cycle, the
/// line).
fn judge_line(m: &BoardModel, o: &Options) -> (f64, f64, String) {
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let r = route_with(m, o, &ctl);
    let l = r.warnings.iter().find(|w| w.starts_with("TopoR loop judge")).unwrap_or_else(|| panic!("judge line: {:?}", r.warnings)).clone();
    let num = |key: &str| -> f64 {
        let i = l.find(key).unwrap_or_else(|| panic!("{key} in {l}"));
        l[..i].trim_end().rsplit(' ').next().unwrap().parse().unwrap()
    };
    (num(" whole-board layerings"), num(" local layerings"), l)
}

/// Step B (acceptance counter): on the fan with its moves (2 and 4 layers, a three-pad net for a
/// junction), the local judge layers the whole board at most 4 times per cycle and every re-lay
/// locally; the global judge layers the whole board per re-lay; the shadow check judges each
/// move both ways and reports the comparison.
#[test]
fn local_judge_layers_the_board_a_few_times_per_cycle() {
    for n_layers in [2, 4] {
        let mut pads = fan_pads();
        pads.push(("F3", 6, 26.0, 4.0, 1.0, 1.0, 0));
        let m = board_layers(30.0, 20.0, 8, &pads, n_layers);
        let (g, gl, lg) = judge_line(&m, &loop_opts(r#""loop_judge": "global""#));
        // The per-cycle layer pass of the old loop (escape = "ties"): a few whole-board layerings per cycle.
        let (l, ll, ln) = judge_line(&m, &loop_opts(r#""loop_judge": "local", "escape": "ties""#));
        assert!(l <= 4.0, "{n_layers} layers, local: {ln}");
        // TW5 (b): the layering escapes run inside the sweep, as many as the work-unit throttle allows
        // ([optimize] escape_share 0.35 of the work), so more whole-board layerings, within the share.
        let tw5 = loop_opts(&format!(r#""loop_judge": "local", {TW5}"#));
        let (le, _, lne) = judge_line(&m, &tw5);
        let esc = escape_line(&m, &tw5);
        let share: f64 = esc.split("work share of the escapes ").nth(1).and_then(|t| t.split(' ').next()).and_then(|t| t.parse().ok())
            .unwrap_or_else(|| panic!("share in {esc}"));
        assert!(share <= 35.0 + 1e-6, "{n_layers} layers, escape share {share} % > 35 %: {esc}");
        assert!(le > 0.0 && esc.contains("escapes ("), "{n_layers} layers, escapes: {lne} / {esc}");
        assert!(ll > 0.0, "{n_layers} layers, local re-lays: {ln}");
        assert!(gl == 0.0 && g > l, "{n_layers} layers, global: {lg} vs local {ln}");
        assert!(lg.contains("loop_judge = \"global\"") && ln.contains("loop_judge = \"local\""));
        let (_, _, sh) = judge_line(&m, &loop_opts(r#""loop_judge": "local", "loop_shadow": 1"#));
        eprintln!("{n_layers} layers:\n  {lg}\n  {ln}\n  {sh}");
        assert!(sh.contains("shadow check ([optimize] loop_shadow = 1)") && sh.contains("kept alike"), "{sh}");
    }
}

/// Step B: the free wires cross held wires that all sit on F.Cu (the free wires' own pad layer
/// on most ends, so their cheapest layer): every such crossing still ends on two layers or is
/// counted unlayerable (the held layer is taken out of the free event's mask, 2 and 4 layers).
#[test]
fn local_layering_keeps_off_the_held_layer() {
    for n_layers in [2, 4] {
        let (m, b, mut st) = laid(n_layers);
        let smd = |o: usize| b.smd_layer(o);
        let p = LayerParams { via_between: true, via_site_r: 0.5e6, ..LayerParams::default() };
        let mut prev = layers::assign(&st, &smd, &p);
        for ev in &mut prev.events {
            for e in ev.iter_mut() {
                e.2 = 0;
            }
        }
        let wb = (0..st.wires.len()).find(|&w| st.wires[w].alive && st.wires[w].net == 2).unwrap();
        topor_engine::topo::tree::remove_wire(&mut st, wb);
        let n0 = st.wires.len();
        st.route_conn(&conn(&m, b, 2, "B1", "B2"), &InitWeights::default(), 3.0).expect("B again");
        let new: Vec<WireId> = (n0..st.wires.len()).collect();
        let free = free_of(&st, &prev, &[wb], &new);
        let loc = layers::assign_local(&st, &smd, &p, None, None, &Held { prev: &prev, free: &free });
        let links = check_local(&st, &prev, &loc, &free, &format!("{n_layers} layers, held on F.Cu"));
        assert!(links > 0, "{n_layers} layers: crossings of free and held wires");
        if n_layers == 4 {
            // Three other layers are open to every free event: none of its crossings with a held
            // wire is left on one layer (counted unlayerable).
            let fh: Vec<&(WireId, WireId)> = loc.unlayerable_w.iter().filter(|&&(a, c)| free.get(a).copied().unwrap_or(true) != free.get(c).copied().unwrap_or(true)).collect();
            assert!(fh.is_empty(), "4 layers: free x held crossings on one layer {fh:?}");
        }
    }
}

/// Step B: a crossing two held wires left on one layer stays counted (the local layering keeps
/// the held pairs of `prev.unlayerable` and adds its own).
#[test]
fn local_layering_keeps_the_held_unlayerable_crossings() {
    for n_layers in [2, 4] {
        let (_m, b, st) = laid(n_layers);
        let smd = |o: usize| b.smd_layer(o);
        let p = LayerParams { via_between: true, via_site_r: 0.5e6, ..LayerParams::default() };
        let mut prev = layers::assign(&st, &smd, &p);
        let live: Vec<WireId> = (0..st.wires.len()).filter(|&w| st.wires[w].alive).collect();
        let (w, h1, h2) = (live[0], live[live.len() - 2], live[live.len() - 1]);
        let mut free = vec![false; st.wires.len()];
        free[w] = true;
        assert!(!free[h1] && !free[h2]);
        let at = topor_engine::geom::Vec2 { x: 1.0, y: 2.0 };
        prev.unlayerable.push(at);
        prev.unlayerable_w.push((h1.min(h2), h1.max(h2)));
        let loc = layers::assign_local(&st, &smd, &p, None, None, &Held { prev: &prev, free: &free });
        assert!(loc.unlayerable_w.contains(&(h1.min(h2), h1.max(h2))) && loc.unlayerable.contains(&at), "{n_layers} layers: held pair kept: {:?}", loc.unlayerable_w);
    }
}

/// Step B: the local capacity model's wires are the free wires and every wire with a pass on an
/// edge a free wire passes (so the model is complete on those edges), and no other.
#[test]
fn neighbourhood_is_the_free_wires_and_their_edge_mates() {
    let (_m, _b, st) = laid(2);
    for w in (0..st.wires.len()).filter(|&w| st.wires[w].alive) {
        let mut free = vec![false; st.wires.len()];
        free[w] = true;
        let nb = layers::neighbourhood(&st, &free);
        let edges: std::collections::BTreeSet<usize> = st.wires[w].passes.iter().map(|&p| st.passes[p].edge).collect();
        for x in (0..st.wires.len()).filter(|&x| st.wires[x].alive) {
            let mate = x == w || st.wires[x].passes.iter().any(|&p| edges.contains(&st.passes[p].edge));
            assert_eq!(nb[x], mate, "wire {x} for free {w}");
        }
        assert!(nb.iter().filter(|&&f| f).count() > 1 || edges.is_empty() || st.wires[w].passes.len() < 2, "free {w} shares an edge");
    }
}

/// Step B regression (bornhack2026, tip 5e542178: `flips` read the events of a dead wire through a
/// held wire's partner link): a move that lays a branch turns its trunk at the junction, and taking
/// a branch out drops that turn, so a move changes wires besides its own. On the fan with three
/// three-pad nets (2 and 4 layers) the local judge sees such moves, lays those wires out again,
/// and every partner link of every local layering points at a live wire's event that points back
/// (`TOPO_LOCALCHECK` counts the ones that do not).
#[test]
fn local_judge_keeps_every_partner_link_with_trees() {
    // SAFETY: the variable only switches on a debug count; tests reading it all want it on.
    unsafe { std::env::set_var("TOPO_LOCALCHECK", "1") };
    for n_layers in [2, 4] {
        let mut pads = fan_pads();
        pads.push(("F3", 6, 26.0, 4.0, 1.0, 1.0, 0));
        pads.push(("B3", 2, 4.0, 17.0, 1.0, 1.0, 2));
        pads.push(("A3", 1, 15.0, 3.5, 1.0, 1.0, 0));
        let m = board_layers(30.0, 20.0, 8, &pads, n_layers);
        for seed in [1, 2, 3] {
            let (_, _, l) = judge_line(&m, &loop_opts(&format!(r#""loop_judge": "local", "topo_rounds": 4, "seed": {seed}"#)));
            assert!(l.contains("TOPO_LOCALCHECK: 0 partner faults"), "{n_layers} layers seed {seed}: {l}");
            eprintln!("{n_layers} layers seed {seed}: {l}");
        }
    }
}

/// Step B (fallback): with `[capacity] model = "cuts"` the local judge has no local model, so the
/// loop judges every move by the whole board, says so in a warning, and names the global judge in
/// its report line; no move is laid out locally.
#[test]
fn local_judge_falls_back_to_global_under_cuts() {
    let mut pads = fan_pads();
    pads.push(("F3", 6, 26.0, 4.0, 1.0, 1.0, 0));
    let m = board_layers(30.0, 20.0, 8, &pads, 2);
    let o = loop_opts(r#""loop_judge": "local", "capacity_model": "cuts""#);
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let r = route_with(&m, &o, &ctl);
    let warn = r.warnings.iter().find(|w| w.contains("prices the per-edge capacity model only")).unwrap_or_else(|| panic!("cuts fallback warning: {:?}", r.warnings));
    assert!(warn.contains("loop_judge = \"global\""), "{warn}");
    let (g, l, line) = judge_line(&m, &o);
    assert!(line.contains("loop_judge = \"global\"") && l == 0.0 && g > 0.0, "{line}");
}

/// Step B (`[optimize] local_depth`): a deeper ring frees at least as many wires per local
/// layering as depth 1 (the crossings of the crossings; more on one of the boards), and the layerings stay exact (no partner
/// fault, no unpriced held-held crossing, no stale held event), 2 and 4 layers.
#[test]
fn local_depth_two_frees_more_and_stays_exact() {
    // SAFETY: the variable only switches on a debug count; tests reading it all want it on.
    unsafe { std::env::set_var("TOPO_LOCALCHECK", "1") };
    let mean_free = |l: &str| -> f64 {
        let i = l.find(" wires laid again").unwrap_or_else(|| panic!("mean in {l}"));
        l[..i].trim_end().rsplit(' ').next().unwrap().parse().unwrap()
    };
    let (mut f1, mut f2) = (0.0, 0.0);
    for n_layers in [2, 4] {
        let mut pads = fan_pads();
        pads.push(("F3", 6, 26.0, 4.0, 1.0, 1.0, 0));
        pads.push(("B3", 2, 4.0, 17.0, 1.0, 1.0, 2));
        let m = board_layers(30.0, 20.0, 8, &pads, n_layers);
        let (_, l1, d1) = judge_line(&m, &loop_opts(r#""loop_judge": "local", "local_depth": 1"#));
        let (_, l2, d2) = judge_line(&m, &loop_opts(r#""loop_judge": "local", "local_depth": 2"#));
        eprintln!("{n_layers} layers:\n  depth 1: {d1}\n  depth 2: {d2}");
        assert!(l1 > 0.0 && l2 > 0.0, "{n_layers} layers: local re-lays {d1} / {d2}");
        assert!(mean_free(&d2) >= mean_free(&d1), "{n_layers} layers: depth 2 frees fewer: {d1} / {d2}");
        (f1, f2) = (f1 + mean_free(&d1), f2 + mean_free(&d2));
        for d in [&d1, &d2] {
            assert!(d.contains("TOPO_LOCALCHECK: 0 partner faults, 0 unpriced held-held crossings, 0 stale held events"), "{n_layers} layers: {d}");
        }
    }
    assert!(f2 > f1, "depth 2 frees more wires somewhere: {f1} vs {f2}");
}

/// The loop's `plan metrics` block "loop" -> "escape" (TW5 report, docs/dev/design/tw5-escape.md).
fn escape_json(m: &BoardModel, o: &Options) -> serde_json::Value {
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let r = route_with(m, o, &ctl);
    let line = r.warnings.iter().find(|w| w.starts_with("plan metrics:")).unwrap_or_else(|| panic!("plan metrics line: {:?}", r.warnings));
    let v: serde_json::Value = serde_json::from_str(line.trim_start_matches("plan metrics:").trim()).expect("plan metrics JSON");
    let mut e = v["loop"]["escape"].clone();
    // Clock fields differ run to run; the copper and every count must not.
    if let Some(x) = e.as_object_mut() {
        x.retain(|k, _| !k.ends_with("_s"));
    }
    e
}

/// TW5 (b): with the TW5 options the layering escapes run inside the sweep and change layers of free
/// positions (a random via-optimal layering, Polubasov 2010 figs. 8-10), deterministic per seed;
/// the default (`escape = "ties"`, before TW5) runs none. 2 and 4 layers.
#[test]
fn layering_escapes_change_free_positions_deterministically() {
    for n_layers in [2, 4] {
        let mut pads = fan_pads();
        pads.push(("F3", 6, 26.0, 4.0, 1.0, 1.0, 0));
        let m = board_layers(30.0, 20.0, 8, &pads, n_layers);
        let tw5 = loop_opts(&format!(r#""loop_judge": "local", {TW5}"#));
        let e = escape_json(&m, &tw5);
        let n = |k: &str| e[k].as_f64().unwrap_or_else(|| panic!("{k} in {e}"));
        assert_eq!(e["mode"], "layering", "{e}");
        assert!(n("escapes") > 0.0 && n("free_positions") > 0.0, "{n_layers} layers: {e}");
        assert!(n("changed_positions") > 0.0, "{n_layers} layers, the escapes moved no free position: {e}");
        assert!(n("work_share") <= 0.35 + 1e-9, "{n_layers} layers: {e}");
        // TW5 (c): the loop history is updated after every escape's whole-board measure.
        assert_eq!(n("history_updates"), n("escapes"), "{n_layers} layers: {e}");
        assert_eq!(e, escape_json(&m, &tw5), "{n_layers} layers: deterministic per seed");
        // A dear escape (escape_kappa 10: one escape's work is ten re-lays' per wire): the throttle spaces
        // the escapes so they stay within the share.
        let k = escape_json(&m, &loop_opts(&format!(r#""loop_judge": "local", "escape_kappa": 10.0, {TW5}"#)));
        assert!(k["escapes"].as_f64().unwrap_or(0.0) > 0.0 && k["work_share"].as_f64().unwrap_or(1.0) <= 0.35 + 1e-9, "{n_layers} layers, kappa 10: {k}");
        let t = escape_json(&m, &loop_opts(r#""loop_judge": "local""#));
        assert_eq!(t["escapes"].as_f64(), Some(0.0), "{n_layers} layers, default (escape = ties): {t}");
        let t = escape_json(&m, &loop_opts(&format!(r#""loop_judge": "local", {TW5}, "escape": "ties""#)));
        assert_eq!(t["escapes"].as_f64(), Some(0.0), "{n_layers} layers, TW5 with escape = ties: {t}");
    }
}

/// TW5 (b) at the layering itself: `LayerParams.walk` > 0 with random ties moves the whole-board
/// layering to another one (2 layers: the plateau walk on the domain graph; 4 layers: one random
/// layer pair re-solved with the walk) for some seeds, the same per seed. The fan's pads on both
/// layers (THT-like): no SMD pad fixes an event, so equal layerings exist (with the SMD fan the
/// 2-layer optimum is unique: 0 of 24 walked). (The walk keeps the domain graph's via optimum:
/// escape::/layers:: unit tests; here the capacity refinement follows.)
#[test]
fn the_layering_walk_moves_to_other_layerings_per_seed() {
    for n_layers in [2, 4] {
        let (_m, b, st) = laid_on(n_layers, true);
        let smd = |o: usize| b.smd_layer(o);
        let base = LayerParams { via_between: true, via_site_r: 0.5e6, ..LayerParams::default() };
        let (mut moved, mut walked) = (0, 0);
        for seed in 1..=24u64 {
            let plain = layers::assign(&st, &smd, &LayerParams { ties: seed, ..base });
            let p = LayerParams { ties: seed, walk: 4, ..base };
            let w = layers::assign(&st, &smd, &p);
            assert_eq!(w.events, layers::assign(&st, &smd, &p).events, "{n_layers} layers seed {seed}: deterministic");
            walked += usize::from(w.walked > 0);
            moved += usize::from(w.events != plain.events);
            eprintln!("{n_layers} layers seed {seed}: walked {}, vias {} (plain {})", w.walked, w.vias, plain.vias);
        }
        eprintln!("{n_layers} layers: walked {walked} of 24, other layering {moved} of 24");
        assert!(walked > 0 && moved > 0, "{n_layers} layers: walked {walked}, moved {moved} of 24");
    }
}

/// TW5 (4), Polubasov 2010 fig. 10 (p. 108): W1 (THT, horizontal) crosses W2 (THT, vertical) and
/// W3 (SMD pads on F.Cu, vertical). The optimum is all three straight with 0 vias: W3 on F.Cu,
/// W1 on B.Cu, W2 on F.Cu (25 mm).
fn fig10() -> (BoardModel, &'static Board) {
    let pads = [
        ("J1", 1, 2.0, 5.5, 1.2, 1.2, 99), ("J2", 1, 12.0, 5.5, 1.2, 1.2, 99),
        ("J3", 2, 5.0, 2.0, 1.2, 1.2, 99), ("J4", 2, 5.0, 9.0, 1.2, 1.2, 99),
        ("U1", 3, 8.5, 1.5, 0.8, 0.8, 0), ("U2", 3, 8.5, 9.5, 0.8, 0.8, 0),
    ];
    let m = board_layers(14.0, 11.0, 3, &pads, 2);
    let b = board::build(&m, &BuildOpts { existing_vias: true, keepouts: true, copper_shapes: true, ..BuildOpts::default() }).unwrap();
    (m, Box::leak(Box::new(b)))
}

/// TW5 (4) at the search: W2 and W3 laid, neither crosses anything, so the loop's layering draws
/// both on the bottom layer (W3 held to F.Cu by its pads). W1 must cross W3 on B.Cu. With
/// `free_wire_cross` = 1 (default) W2 counts as held to B.Cu, so crossing it implies a via pair and
/// W1 goes round W2's end; with a = 0 W2 counts on every layer its own stretch allows (THT ends:
/// both), crossing it is free of vias and of the crossing price (k = 0), and W1 crosses it straight.
/// The crossing price at the loop's level (crossing factor x loop share 0.25 = 5 mm).
#[test]
fn a_wire_that_crosses_nothing_is_crossed_at_its_freedom() {
    let (m, b) = fig10();
    let mut wt = InitWeights::default();
    wt.cross_mm = 5.0;
    let lay = |a: f64| {
        let mut st = State::new(&b.proj);
        let (w2, _) = st.route_conn(&conn(&m, b, 2, "J3", "J4"), &wt, 1.0).expect("W2");
        st.route_conn(&conn(&m, b, 3, "U1", "U2"), &wt, 1.0).expect("W3");
        st.cross_vias = true;
        let none: Vec<Vec<(usize, f64, i32)>> = vec![Vec::new(); st.wires.len()];
        st.layers_now = topor_engine::topo::optimise::layers_now(&st, &none, 1);
        st.free_wire_a = a;
        st.mark_free_wires(&none);
        let (w1, _) = st.route_conn(&conn(&m, b, 1, "J1", "J2"), &wt, 1.0).expect("W1");
        let over_w2 = st.crossings(w1).iter().filter(|x| x.other == w2).count();
        let len: f64 = st.taut_points(w1).windows(2).map(|p| (p[1] - p[0]).len()).sum::<f64>() / MM;
        (over_w2, len)
    };
    let (d, f, h) = (lay(1.0), lay(0.0), lay(0.5));
    eprintln!("free_wire_cross 1: W1 crosses W2 {} times, {:.2} mm; 0: {} times, {:.2} mm; 0.5: {} times, {:.2} mm", d.0, d.1, f.0, f.1, h.0, h.1);
    assert_eq!(d.0, 0, "default: the crossing-free W2 counts as held to B.Cu, W1 goes round it ({:.2} mm)", d.1);
    assert_eq!(f.0, 1, "a = 0: W1 crosses the free W2 ({:.2} mm)", f.1);
    assert!(f.1 < d.1 - 1.0 && f.1 <= 10.0 * 1.05, "a = 0: W1 straight ({:.2} mm) vs round ({:.2} mm)", f.1, d.1);
}

/// TW5 (4) routed, the fig. 10 board: with `free_wire_cross` = 0 some seed of 0..8 reaches the
/// optimum (0 vias, all straight); the default's seeds are reported (the D8 proposal of the
/// paper's board measures both: 7 of 10 seeds with a = 0, 0 of 11 with a = 1).
#[test]
fn free_wire_cross_reaches_the_fig10_optimum_for_some_seed() {
    let (m, _) = fig10();
    let run = |extra: &str| {
        let o = loop_opts(&format!(r#""topo_rounds": 16, {extra}"#));
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        let r = route_with(&m, &o, &ctl);
        let len: f64 = r.connections.iter().map(|c| c.length_nm as f64 / 1e6).sum();
        let vias: usize = r.connections.iter().map(|c| c.vias).sum();
        let routed = r.connections.iter().filter(|c| c.status == "routed").count();
        (len, vias, routed)
    };
    let (mut hit_f, mut hit_d) = (0, 0);
    for seed in 0..8 {
        let (d, f) = (run(&format!(r#""seed": {seed}"#)), run(&format!(r#""seed": {seed}, "free_wire_cross": 0.0"#)));
        eprintln!("seed {seed}: default {:.2} mm {} vias {} routed; a = 0 {:.2} mm {} vias {} routed", d.0, d.1, d.2, f.0, f.1, f.2);
        let opt = |x: (f64, usize, usize)| x.1 == 0 && x.2 == 3 && x.0 <= 25.0 * 1.05;
        hit_d += usize::from(opt(d));
        hit_f += usize::from(opt(f));
    }
    eprintln!("optimum reached: default {hit_d} of 8, free_wire_cross 0 {hit_f} of 8");
    assert!(hit_f > 0, "free_wire_cross 0: no seed of 8 reaches the optimum (default {hit_d})");
}
