//! D3 tests (via room in the planner).

use topor_engine::topo::pack::{layer_items, pair_req, PackItem};
use topor_engine::topo::refine::{pass_layers, pass_layers_at, via_intervals};

const MM: f64 = 1e6;

fn track(net: i32, infl: f64) -> PackItem {
    PackItem { net, w: 0.25 * MM, s: 0.2 * MM, infl, disc: [0.0; 2] }
}

/// A via disc packed next to a track needs the disc's radius plus the clearance plus half the
/// track, stretched by the track's oblique factor only (a disc is round); two tracks keep the
/// larger factor of the two.
#[test]
fn a_via_disc_takes_room_in_the_packing() {
    let gap = 0.2 * MM;
    let (a, b) = (track(1, 1.0), track(2, 1.5));
    let tt = pair_req(&a, &b, gap);
    assert!((tt - 0.45 * MM * 1.5).abs() < 1.0, "{tt}");
    let via = PackItem { disc: [0.8 * MM; 2], ..a };
    let vt = pair_req(&via, &b, gap);
    assert!((vt - (0.4 + 0.125 + 0.2) * MM * 1.5).abs() < 1.0, "{vt}");
    // A disc only on the side away from the neighbour does not count against it.
    let half = PackItem { disc: [0.8 * MM, 0.0], ..a };
    assert!((pair_req(&half, &b, gap) - tt).abs() < 1.0);
    // Two discs (a disc-only item on the other: no track, factor 1): no oblique factor.
    let via2 = PackItem { w: 0.0, infl: 1.0, disc: [0.8 * MM; 2], ..track(2, 2.0) };
    assert!((pair_req(&PackItem { infl: 1.0, ..via }, &via2, gap) - 1.0 * MM).abs() < 1.0);
}

/// On a layer where the via's wire runs elsewhere, its disc is still packed (a through via
/// blocks every layer), as a disc-only item; a pass without a via on another layer is not.
#[test]
fn a_through_via_is_packed_on_every_layer() {
    let layer = |p: usize| [0, 1, 1][p];
    let disc = |p: usize| if p == 1 { [0.8 * MM; 2] } else { [0.0; 2] };
    let (ids, items) = layer_items(&[0, 1, 2], 0, layer, disc, |_| track(1, 1.0));
    assert_eq!(ids, vec![(0, false), (1, true)]);
    assert_eq!(items[1].w, 0.0);
    assert_eq!(items[1].disc, [0.8 * MM; 2]);
    let (ids, _) = layer_items(&[0, 1, 2], 1, layer, disc, |_| track(1, 1.0));
    assert_eq!(ids, vec![(1, false), (2, false)]);
}

/// The planner may put a via anywhere in its interval: the passes before it keep the first
/// layer (P3/P4 choose the traversal, the drawing tries that triangle first).
#[test]
fn a_via_moves_inside_its_interval() {
    let ev = [(1, 0.5, 1), (6, 0.5, 0)];
    assert_eq!(via_intervals(&ev), vec![(1, 6)]);
    assert_eq!(pass_layers(8, &ev, 0), vec![1, 1, 1, 1, 0, 0, 0, 0]);
    assert_eq!(pass_layers_at(8, &ev, 0, &[2]), vec![1, 1, 0, 0, 0, 0, 0, 0]);
    assert_eq!(pass_layers_at(8, &ev, 0, &[6]), vec![1, 1, 1, 1, 1, 1, 0, 0]);
}
