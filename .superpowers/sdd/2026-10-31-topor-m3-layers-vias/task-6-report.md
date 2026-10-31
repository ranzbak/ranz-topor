# Task 6 report
decompose.rs: TermKind::Pour(usize); TermRef.layers (terminal copper layers ∩ `layers`, sorted); Connection.layer removed;
decompose(model, opts, layers: &[u32]); Decomposition.warnings. Pours are DSU nodes; a pour with >=3-point fill outline is "usable".
In nets with a usable pour every selected cluster without a pour gets one connection cluster->Pour (nearest port to fill outlines over all usable
pours; est_len = point-to-polygon distance, 0 inside, to.at = nearest outline point (port itself when inside), to.layers = fill layers ∩ layers,
label "pour:<uuid>"); no MST among non-pour clusters; MST kept only among pour-containing clusters with ports. pour_served = usable pour && 0 conns
(old M2 formula for fill-less pours; warning "pour <uuid> has no fill outline", M2 behaviour). NetInfo.clusters excludes port-less (pour-only) clusters.
terminal_of(Pour) -> Terminal::Via { via: usize::MAX, at } (no via item). check.rs: point_in_poly pub(crate) (decompose's duplicate removed).
route.rs: decompose(&routable sorted), Ctx.layer, stub_item/bisect_stub/probe_path take `layer` (probe_path got #[allow(too_many_arguments)]),
on_layer(t, layer) = t.layers.contains, Pour connections fail early "pour completion needs vias (Task 9)", decompose warnings appended.
Tests: 7 new in decompose_nets.rs (+existing adapted); search_slots/search_sleeve/negotiate_loop Connection/TermRef literals adapted; route_probe example adapted.
Results: full `cargo test --release` green (26 suites), clippy -D warnings clean. route_probe eprom: 26 ok / 27 (same as before); the probe example does not print
decompose warnings; the eprom model JSON has no fills, so route() emits the "no fill outline" warning (route_engine e2e green).
