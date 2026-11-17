"""mutate.py <name> apply <tree> | list : round's 49 + verifier's 9 + re-verifier's mutants for radii-m3 (d598fd8)."""
import sys
from pathlib import Path
M = {
 # spoke tips: the side of p at this crossing (alternation only as the fallback)
 "tip_side_alt": ("erbs.rs", "let a_left = p_left_of(s, c, x).unwrap_or(region0 ^ (k % 2 == 1));", "let a_left = region0 ^ (k % 2 == 1);"),
 "tip_side_first": ("erbs.rs", "let a_left = p_left_of(s, c, x).unwrap_or(region0 ^ (k % 2 == 1));", "let a_left = region0;"),
 # the along-cut test: the band's crossing before a / after b goes round that tip
 "spoke_a_off": ("erbs.rs", "let da = if at < lo - ON && !no_push(c.fp) {", "let da = if false && at < lo - ON && !no_push(c.fp) {"),
 "spoke_b_off": ("erbs.rs", "let db = if at > hi + ON && !no_push(c.fq) {", "let db = if false && at > hi + ON && !no_push(c.fq) {"),
 "spoke_always": ("erbs.rs", "let da = if at < lo - ON && !no_push(c.fp) { Detour::From(xb) } else { Detour::Never };", "let da = Detour::From(xb);"),
 # the detour loop's orientation and order
 "loop_orient": ("erbs.rs", "let nn = if pp.lab[i] == 2 { -d.perp() } else { d.perp() };", "let nn = d.perp();"),
 "detour_order": ("erbs.rs", "det.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));", "det.sort_by(|a, b| a.1.total_cmp(&b.1));"),
 "far_first_off": ("erbs.rs", "    let mut swapped = true;\n", "    let mut swapped = false;\n"),
 "merge_off": ("erbs.rs", "                        if (self.pts[i] - p).len() <= MERGE {", "                        if (self.pts[i] - p).len() <= -1.0 {"),
 "end_skip_off": ("erbs.rs", "(s, _, _) if s <= ON || s >= cum[n - 1] - ON => continue,", "(s, _, _) if false && (s <= ON || s >= cum[n - 1] - ON) => continue,"),
 "auto_off": ("erbs.rs", "            Detour::Auto => match place(p, all) {", "            Detour::Auto if true => continue,\n            Detour::Auto => match place(p, all) {"),
 # the walk
 "no_reduce": ("erbs.rs", "        if out.last() == Some(&(r, l)) {", "        if false && out.last() == Some(&(r, l)) {"),
 "exit_edge_swap": ("erbs.rs", "                if !left_of(vi, a, b) && left_of(vj, a, b) {", "                if left_of(vi, a, b) && !left_of(vj, a, b) {"),
 "vertex_leave_off": ("erbs.rs", "            if let Some(&c) = f.iter().find(|&&c| (pos(c) - a).len() <= ON) {", "            if let Some(&c) = f.iter().find(|&&c| false && (pos(c) - a).len() <= ON) {"),
 "tie_label": ("erbs.rs", "            lab(c) != 3\n", "            lab(c) == 3\n"),
 # the arc samples stand at the stack radius, on the wrapped vertex's side
 "arc_inside": ("erbs.rs", "let a = at + u * (rho * grow).max(t);", "let a = at + u * (0.5 * rho * grow).max(0.5 * t);"),
 "arc_side": ("erbs.rs", "cand.push((a, left, gi, w, Detour::From(band[k])));", "cand.push((a, !left, gi, w, Detour::From(band[k])));"),
 # features grown by the half width plus the clearance
 "feat_no_halfw": ("erbs.rs", "let r = 0.5 * s.w + cl;", "let r = cl;"),
 "feat_off": ("erbs.rs", "        for x in grown(&ft.poly, r, &dirs, grow) {", "        for x in grown(&ft.poly, r, &dirs, grow).into_iter().take(0) {"),
 "feat_side": ("erbs.rs", "                    band_side(band, cen).0\n", "                    !band_side(band, cen).0\n"),
 # the corridor reaches past the push
 "corridor_no_push": ("erbs.rs", "let reach = push + 2.0 * rmax + 0.5e6;", "let reach = 2.0 * rmax + 0.5e6;"),
 # own terminal pads' cut items crossed at the pad
 "tpad_keep": ("erbs.rs", "                if in_pad(root) && in_pad(band_cross(c, x)) {", "                if false && in_pad(root) && in_pad(band_cross(c, x)) {"),
 # conflicts counted
 "conflict_uncounted": ("erbs.rs", "                    self.conflicts += 1;\n", "                    self.conflicts += 0;\n"),
 # the time limit and the cancel flag
 "deadline_never": ("erbs.rs", "self.deadline.is_some_and(|d| std::time::Instant::now() >= d) ||", "false && self.deadline.is_some_and(|d| std::time::Instant::now() >= d) ||"),
 "stop_never": ("erbs.rs", "|| self.stop.as_ref().is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed))", "|| false && self.stop.as_ref().is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed))"),
 # a failed sleeve is a failure, not an empty ERBS
 "fail_silent": ("erbs.rs", "            Err(m) => out.failed.push((si, m)),", "            Err(_) => {}"),
 # a band through another net's feature: no obstacle (predicted)
 "through_off": ("erbs.rs", "        if band.windows(2).any(|w| seg_through(w[0], w[1], &ft.poly)) {", "        if false && band.windows(2).any(|w| seg_through(w[0], w[1], &ft.poly)) {"),
 "through_shallow": ("erbs.rs", "        point_in_poly(m, poly) && (0..poly.len()).all(|i| seg_seg_dist(m, m, poly[i], poly[(i + 1) % poly.len()]) > DEEP)", "        point_in_poly(m, poly)"),
 # the walk goes round an obstacle point on the path on the side opposite its label
 "point_walk_off": ("erbs.rs", "                if matches!(lab(c), 2 | 3) {", "                if false && matches!(lab(c), 2 | 3) {"),
 "point_walk_dir": ("erbs.rs", "                    let cw = lab(c) == 3;", "                    let cw = lab(c) != 3;"),
 "wrap_inside_off": ("erbs.rs", "            Some(&l) if wraps_inside(band, &ft.poly, l) => {", "            Some(&l) if false && wraps_inside(band, &ft.poly, l) => {"),
 # a zero spoke on an own terminal pad holds no room
 "own_tip_keep": ("erbs.rs", "            if !((a - c.p).len() < 1.0 && in_pad(c.p)) {", "            if true || !((a - c.p).len() < 1.0 && in_pad(c.p)) {"),
 # Cancel after the loop (draw through) does not stop the ERBS
 "cancel_stops_erbs": ("core.rs", "(!draw_through.get()).then(|| ctl.cancel.clone())", "Some(ctl.cancel.clone())"),
 # the drawing uses the ERBS of its own layer (kept from the verifier's list)
 "erbs_x_off": ("draw.rs", "let x = erbs_x.unwrap_or(taut_x);", "let x = { let _ = erbs_x; taut_x };"),
 "erbs_x_any_layer": ("draw.rs", "v.iter().filter(|b| b.layer == l)", "v.iter().filter(|b| b.layer == l || true)"),
 # round g: length factor (draw.rs attempt -1)
 "g_price_flat": ("draw.rs", "    le * (1.0 + k.max(0.0) * x * x)\n", "    le\n"),
 "g_price_lin": ("draw.rs", "    le * (1.0 + k.max(0.0) * x * x)\n", "    le * (1.0 + k.max(0.0) * x)\n"),
 "g_margin_off": ("draw.rs", "    if lb > 0.0 && pe <= lb * (1.0 + g) {\n", "    if false && lb > 0.0 && pe <= lb * (1.0 + g) {\n"),
 "g_never_replace": ("draw.rs", "    Some(ls.is_some_and(|ls| ls * (1.0 + g) < pe))\n", "    Some(false)\n"),
 "g_no_room": ("draw.rs", "    Some(ls.is_some_and(|ls| ls * (1.0 + g) < pe))\n", "    Some(ls.is_some_and(|ls| ls < pe))\n"),
 "g_no_compete": ("draw.rs", "                        if erbs_vs_sequential(pe, band_len, self.opts.erbs_margin, None).is_some() {", "                        if false && erbs_vs_sequential(pe, band_len, self.opts.erbs_margin, None).is_some() {"),
 "g_no_swap_back": ("draw.rs", "                            (pts, items, pneck_out, neck, src) = (k.2, k.3, k.4, k.5, -1);", "                            let _ = (k.2, k.3, k.4, k.5);"),
 "g_kept_dropped": ("draw.rs", "                        if let Some(k) = kept.take() {\n                            self.erbs_long.push((w, layer, k.1, band_len, f64::NAN, false));", "                        if let Some(k) = kept.take().filter(|_| false) {\n                            self.erbs_long.push((w, layer, k.1, band_len, f64::NAN, false));"),
 # round g rule 3: a grown point pushes only where the band crosses its spoke
 "g_dips_true": ("erbs.rs", "    band.windows(2).any(|w| seg_touch(w[0], w[1], x, root))\n", "    { let _ = (band, x, root); true }\n"),
 "g_dips_false": ("erbs.rs", "    band.windows(2).any(|w| seg_touch(w[0], w[1], x, root))\n", "    { let _ = (band, x, root); false }\n"),
 # round g rules 4/4b: no spoke detour at squeezed / pad-exit features; their violations predicted
 "g_squeezed_tip": ("erbs.rs", "let da = if at < lo - ON && !no_push(c.fp) {", "let da = if at < lo - ON {"),
 "g_exit_off": ("erbs.rs", "if cl > 0.0 && ft.poly.len() >= 3 &&", "if false && cl > 0.0 && ft.poly.len() >= 3 &&"),
 # g_exit_unpredicted: its line is gone in d598fd8, see h_exit_unpredicted
 # round g rule 5: no arcs round a feature the band runs through / wraps from inside
 "g_arc_skip_off": ("erbs.rs", "if poly.is_some_and(|p| band.windows(2).any(|w| seg_through(w[0], w[1], p)) || wraps_inside(band, p, left)) {", "if false && poly.is_some_and(|p| band.windows(2).any(|w| seg_through(w[0], w[1], p)) || wraps_inside(band, p, left)) {"),
 # --- verifier round g ---
 # rule 4 at the q end of the cut (the round's g_squeezed_tip only mutates the p end)
 "v_squeezed_tip_b": ("erbs.rs", "let db = if at > hi + ON && !no_push(c.fq) {", "let db = if at > hi + ON {"),
 # rule 4b: the exit radius without the half width
 "v_exit_radius": ("erbs.rs", "poly_dist(c, &ft.poly) < 0.5 * s.w + cl)", "poly_dist(c, &ft.poly) < cl)"),
 # rule 4b: exit features stay obstacles in 1c (grown points), only tips skip them
 "v_exit_1c": ("erbs.rs", "if cl <= 0.0 || no_push(f) || ft.poly.is_empty() {", "if cl <= 0.0 || skip.contains(&f) || ft.poly.is_empty() {"),
 # rule 4b: tips skip only squeezed features, not exit ones (p end)
 "v_exit_tip": ("erbs.rs", "let da = if at < lo - ON && !no_push(c.fp) {", "let da = if at < lo - ON && !skip.contains(&c.fp) {"),
 # rule 5: only the through part, not the wrapped-from-inside part
 "v_arc_inside_only": ("erbs.rs", "band.windows(2).any(|w| seg_through(w[0], w[1], p)) || wraps_inside(band, p, left))", "band.windows(2).any(|w| seg_through(w[0], w[1], p)))"),
 # rule 3: crossing only, no touch
 "v_touch_off": ("erbs.rs", "on(a, b, c) || on(a, b, d) || on(c, d, a) || on(c, d, b)", "false"),
 # length factor: the ERBS_LONG list loses the compared runs
 "v_long_unlisted": ("draw.rs", "self.erbs_long.push((w, layer, k.1, band_len, ls, replaced));", "let _ = (w, k.1);"),
 # length factor: a replaced ERBS counted as a check fallback
 "v_replaced_is_fallback": ("draw.rs", "if src >= 0 && band.is_some() && !replaced {", "if src >= 0 && band.is_some() {"),
 # restored ERBS re-checked (expected equivalent: nothing is committed while it waits)
 "v_restore_recheck": ("draw.rs", "let full = if restored { CheckReport::default() } else {", "let full = if false && restored { CheckReport::default() } else {"),
 # --- re-verifier (round h) ---
 # g_exit_unpredicted adapted to the new check_layer (exit features never predicted)
 "h_exit_unpredicted": ("erbs.rs", "        for &f in &e.exit {\n            pred_exit.insert((e.stretch, f));\n        }\n", ""),
 # exit violations predicted along the whole stretch again (the round-h fix undone)
 "h_exit_whole": ("erbs.rs", "predicted: pred_feat.contains(&(si, f)) || (pred_exit.contains(&(si, f)) && { let (x, y) = exit_run(i, f); k < x || k >= y }) });", "predicted: pred_feat.contains(&(si, f)) || pred_exit.contains(&(si, f)) });"),
 # exit run only from the start of the ERBS (the far end's exit not predicted)
 "h_exit_no_b": ("erbs.rs", "        let mut b = n;\n        while b > a && viol(b - 1) {\n            b -= 1;\n        }\n", "        let b = n;\n"),
 # the edge's exit run (outline as exit feature) undone
 "h_edge_exit_whole": ("erbs.rs", "predicted: pred_feat.contains(&(si, ef)) || (pred_exit.contains(&(si, ef)) && { let (x, y) = exit_run(i, ef); k < x || k >= y }) });", "predicted: pred_feat.contains(&(si, ef)) || pred_exit.contains(&(si, ef)) });"),
 # worst-per-pair map without the predicted class (a far gap hides the exit violation)
 "h_worst_one_class": ("erbs.rs", "let e = worst.entry((v.stretch, v.other, v.predicted)).or_insert(v);", "let e = worst.entry((v.stretch, v.other, false)).or_insert(v);"),
 # d598fd8 undone: the hook fails the first stretch of every layer
 "h_inject_every_layer": ("erbs.rs", "        if !le.es.is_empty() || !le.failed.is_empty() {\n            inject = false;\n        }\n", ""),
 # the no-sequential hook never set (kept-ERBS test must fail)
 "h_no_seq_hook_off": ("core.rs", "dr.test_no_sequential = ctl.test_point(\"no sequential run\");", "dr.test_no_sequential = false;"),
}
if sys.argv[1] == "list":
    print(" ".join(M)); sys.exit(0)
name, act, W = sys.argv[1], sys.argv[2], Path(sys.argv[3])
f, old, new = M[name]
p = W / "engine" / "src" / "topo" / f
s = p.read_text(); n = s.count(old)
assert n == 1, f"{name}: {n} matches"
p.write_text(s.replace(old, new, 1)); print(f"{name}: applied to {f}")
