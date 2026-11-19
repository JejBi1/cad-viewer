//! Regression tests of the fourth review: a text heavy drawing is drawn at every memory the computer may have (the last level
//! of detail is a box, a scene is refused only when even that does not fit, and the refusal is not remembered), curves are
//! never reduced, and the glyph outlines of texts have a budget when they are recorded.
use super::*;
use crate::frontend::Frontend;
use crate::geom::{Elems, V2};
use crate::scene::{Built, GKind, Recorder, FLAG_BOX, FLAG_TEXT, MAX_SCENE_PRIMS};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

const GB: u64 = 1 << 30;
const TEXT17: &str = "Pozice 12 M12x1,5";

fn text_at(x: f64, y: f64, s: &str) -> String {
    ent!("TEXT"; 8 => "0", 10 => x, 20 => y, 30 => 0.0, 40 => 2.5, 1 => s)
}

/// `n` texts of one short sentence in rows of 30, 40 apart (a text is 24 wide and 2.5 high).
fn texts(n: usize) -> Drawing {
    let mut d = Doc::new();
    for i in 0..n {
        d.entities.push_str(&text_at((i % 30) as f64 * 40.0, (i / 30) as f64 * 6.0, TEXT17));
    }
    d.load()
}

fn verts(sc: &Scene2d) -> u64 {
    sc.batches.iter().map(|b| (b.lines.len() + b.fills.len()) as u64).sum()
}

/// Whether some triangle or segment lies in the box of the text number `i` of `texts`.
fn has_geometry(sc: &Scene2d, i: usize) -> bool {
    let (x0, y0) = ((i % 30) as f64 * 40.0, (i / 30) as f64 * 6.0);
    let inside = |x: f64, y: f64| x >= x0 - 1.0 && x <= x0 + 40.0 && y >= y0 - 1.5 && y <= y0 + 4.0;
    sc.batches.iter().any(|b| {
        b.fills.chunks_exact(3).any(|t| inside(f64::from(t[0][0] + t[1][0] + t[2][0]) / 3.0, f64::from(t[0][1] + t[1][1] + t[2][1]) / 3.0))
            || b.lines.chunks_exact(2).any(|s| inside(f64::from(s[0][0] + s[1][0]) / 2.0, f64::from(s[0][1] + s[1][1]) / 2.0))
    })
}

fn stats_of(d: &Drawing) -> crate::scene::BuildStats {
    d.active().expect("active scene").0.stats
}

fn batches_equal(a: &[DrawBatch], b: &[DrawBatch]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.layer == y.layer && x.color == y.color && x.width == y.width && x.lines == y.lines && x.fills == y.fills)
}

// ------------------------------------------------------------------------------------------------ 1: the last level, the refusal
#[test]
fn texts_that_do_not_fit_even_simplified_are_drawn_as_bars_and_the_note_says_so() {
    let n = 300;
    let reference = texts(n);
    let sc_full = reference.scene_checked("Model").expect("full detail");
    assert!(reference.scene_notes("Model").is_empty());
    // the detail is full up to 20 000 vertices, the scene may have 120 000: 300 simplified texts are about 250 000
    let mut d = texts(n);
    d.set_out_limits(20_000, 120_000);
    let sc = d.scene_checked("Model").expect("not refused: what does not fit is a bar");
    assert!(verts(&sc) <= 120_000, "{}", verts(&sc));
    assert!(verts(&sc) > 60_000, "the room is used: {}", verts(&sc));
    for i in 0..n {
        assert!(has_geometry(&sc, i), "text {i} is left out");
    }
    let st = stats_of(&d);
    assert!(st.simplified > 0 && st.boxed > 0 && st.simplified + st.boxed <= n, "{st:?}");
    // the notes: the existing one for the simplified outlines, and one for the bars
    let notes = d.scene_notes("Model");
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes[0].contains("zjednodušen") && notes[0].contains("Všechny entity zůstávají zobrazeny"), "{}", notes[0]);
    assert!(notes[1].contains("obdélník") && notes[1].contains("Všechny entity zůstávají zobrazeny"), "{}", notes[1]);
    assert!(notes[1].contains(&format!("u {} tvarů", st.boxed)), "{}", notes[1]);
    let rows = d.info_rows("Model", sc.bounds, None);
    assert_eq!(rows.iter().filter(|r| r.label == "Upozornění" && r.warning).count(), 2, "{rows:?}");
    // a scene that fits has neither note
    assert!(verts(&sc_full) > verts(&sc));
}

#[test]
fn a_bigger_memory_gives_more_detail_and_every_memory_gives_a_drawing() {
    // the limits of 1, 2 and 4 GB of free memory, a thousandth of them: the same ratios on a scene a thousand times smaller
    let n = 100;
    let mut boxed = Vec::new();
    let mut shown = Vec::new();
    for gb in [1u64, 2, 4] {
        let (soft, max) = crate::memory::out_limits_for(Some(gb * GB));
        let mut d = texts(n);
        d.set_out_limits(soft / 1000, max / 1000);
        let sc = d.scene_checked("Model").unwrap_or_else(|e| panic!("{gb} GB: {e}"));
        assert!(verts(&sc) <= max / 1000, "{gb} GB: {} vertices", verts(&sc));
        for i in 0..n {
            assert!(has_geometry(&sc, i), "{gb} GB: text {i}");
        }
        let st = stats_of(&d);
        boxed.push(st.boxed);
        shown.push(st.simplified);
        assert!(!d.scene_notes("Model").is_empty(), "{gb} GB: {st:?}");
    }
    // less memory, more bars
    assert!(boxed[0] >= boxed[1] && boxed[1] >= boxed[2] && boxed[0] > boxed[2], "{boxed:?}");
    // and a computer that has the memory for all of it draws the letters
    let d = texts(n);
    d.scene_checked("Model").expect("scene");
    assert_eq!(stats_of(&d).boxed, 0);
}

#[test]
fn a_letter_is_drawn_simplified_rather_than_in_full_next_to_a_text_that_is_only_a_bar() {
    // 100 texts, a limit that all of them fit simplified but not in full: none of them is a bar, and the first ones are not
    // taken to full detail at the expense of the rest
    let mut d = texts(100);
    d.set_out_limits(u64::MAX / 4, 130_000);
    let sc = d.scene_checked("Model").expect("scene");
    let st = stats_of(&d);
    assert_eq!(st.boxed, 0, "{st:?}");
    assert!(st.simplified > 80, "{st:?}");
    assert!(verts(&sc) <= 130_000);
    // with no more room than that for the simplified texts, every text is simplified and none is full
    let mut d = texts(100);
    d.set_out_limits(u64::MAX / 4, 90_000);
    let sc = d.scene_checked("Model").expect("scene");
    let st = stats_of(&d);
    assert!(st.boxed < 10 && st.simplified > 90 - st.boxed, "{st:?}");
    assert!(verts(&sc) <= 90_000);
}

#[test]
fn a_scene_is_refused_only_when_even_the_boxes_do_not_fit() {
    // 50 texts are 300 vertices as bars
    let mut d = texts(50);
    d.set_out_limits(100, 400);
    let sc = d.scene_checked("Model").expect("fits as bars");
    assert!(verts(&sc) <= 400 && verts(&sc) >= 250, "{}", verts(&sc));
    assert_eq!(stats_of(&d).boxed, 50);
    for limit in [200u64, 100, 20] {
        let mut d = texts(50);
        d.set_out_limits(100, limit);
        let e = d.scene_checked("Model").expect_err("the bars do not fit");
        assert!(e.0.starts_with(&format!("Výkres se při vykreslení rozpadne na více než {limit} bodů")), "{}", e.0);
    }
    // a scene of strokes that does not fit is refused as before: nothing of it can be reduced
    let mut d = Doc::new();
    for i in 0..400 {
        d.entities.push_str(&line("0", [i as f64, 0.0], [i as f64, 1.0], None));
    }
    let mut d = d.load();
    d.set_out_limits(100, 500);
    assert!(d.scene_checked("Model").is_err());
}

#[test]
fn a_refusal_that_depends_on_the_free_memory_is_not_remembered() {
    // 50 texts are 250 elements and 300 vertices as boxes: 100 refuses them while they are recorded (the elements are
    // over), 280 after they are made (the vertices are over)
    for refusing in [100u64, 280] {
        let mut d = texts(50);
        d.set_out_limits(100, refusing);
        let e = d.scene_checked("Model").unwrap_err();
        assert!(e.0.contains(&format!("{refusing} bodů")), "{}", e.0);
        assert!(d.scene("Model").batches.is_empty());
        // the entity budget has nothing to say about it, and the next scene is worked out again with the limits of the moment
        assert!(d.check_budget("Model").is_ok(), "{refusing}");
        d.set_out_limits(100, 2_000);
        assert!(d.scene_checked("Model").is_ok(), "{refusing}: more memory is free, the same layout is drawn");
        // the same through the call of the viewer, which takes the geometry (a scene that is kept is not worked out again,
        // one that was handed over is, and then the limits of the moment decide)
        let mut d = texts(50);
        let cancel = Arc::new(AtomicBool::new(false));
        let opts = SceneOptions::default();
        d.set_out_limits(100, refusing);
        assert!(matches!(d.scene_cancellable("Model", &opts, &cancel), Err(crate::SceneError::TooComplex(_))), "{refusing}");
        d.set_out_limits(100, 2_000);
        let sc = d.scene_cancellable("Model", &opts, &cancel).expect("a later build re-evaluates");
        assert!(!sc.batches.is_empty());
        d.set_out_limits(100, refusing);
        assert!(d.scene_cancellable("Model", &opts, &cancel).is_err(), "{refusing}");
        d.set_out_limits(100, 2_000);
        assert!(d.scene_cancellable("Model", &opts, &cancel).is_ok(), "{refusing}");
    }
}

// ------------------------------------------------------------------------------------------------ the boxes
#[test]
fn the_bar_of_a_text_is_turned_with_the_text_and_has_its_size() {
    // a text of 17 letters turned by 45 degrees, its outlines recorded, then taken down to the last level
    let d = Doc::new().add(ent!("TEXT"; 8 => "0", 10 => 100.0, 20 => 50.0, 30 => 0.0, 40 => 2.5, 50 => 45.0, 1 => TEXT17)).add(line("0", [0.0, 0.0], [200.0, 200.0], Some(1))).load();
    let mut fe = Frontend::new(d.document(), false, true, fonts(), vec!["model".to_string()]);
    fe.rec.set_out_limits(0, 40);
    fe.draw_layout("model", 0);
    assert!(fe.rec.prims.iter().any(|p| p.flags & FLAG_TEXT != 0 && p.flags & FLAG_BOX == 0), "the outlines were recorded");
    let b = Built::build(std::mem::take(&mut fe.rec), 1.0, Vec::new(), &[], "Model", vec!["Model".into()], [0, 0, 0], OriginMode::World);
    assert!(!b.over_out);
    assert_eq!(b.stats.boxed, 1, "{:?}", b.stats);
    let bar: Vec<[f64; 2]> = b.scene.batches.iter().flat_map(|x| x.fills.iter().map(|p| [f64::from(p[0]), f64::from(p[1])])).collect();
    assert_eq!(bar.len(), 6, "two triangles");
    let area: f64 = bar.chunks(3).map(|t| ((t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[2][0] - t[0][0]) * (t[1][1] - t[0][1])).abs() / 2.0).sum();
    let (xs, ys): (Vec<f64>, Vec<f64>) = (bar.iter().map(|p| p[0]).collect(), bar.iter().map(|p| p[1]).collect());
    let spread = |v: &[f64]| v.iter().cloned().fold(f64::NEG_INFINITY, f64::max) - v.iter().cloned().fold(f64::INFINITY, f64::min);
    let aabb = spread(&xs) * spread(&ys);
    assert!(area < 0.4 * aabb, "an oriented bar, not the box of its bounds: {area} of {aabb}");
    // 17 letters of a cap height of 2.5 are about 30 wide, and 3 high with the comma under the line
    assert!((60.0..130.0).contains(&area), "{area}");
    // the long side (the first side of the first triangle) runs along 45 degrees
    let (dx, dy) = (bar[1][0] - bar[0][0], bar[1][1] - bar[0][1]);
    assert!(dx.hypot(dy) > 25.0, "the long side is the first one: {}", dx.hypot(dy));
    let angle = dy.atan2(dx).to_degrees().rem_euclid(180.0);
    assert!((angle - 45.0).abs() < 3.0, "{angle}");
}

#[test]
fn another_filled_shape_is_the_outline_of_its_bounds() {
    // a hatch-like fill of 200 corners, taken down to the last level: four segments around its bounds
    let mut rec = Recorder::new();
    let g = rec.group(GKind::F, &[0], [255, 255, 255, 255], 0.0);
    let mut e = Elems::new();
    for k in 0..200 {
        let a = k as f64 * std::f64::consts::TAU / 200.0;
        let p: V2 = [50.0 + 30.0 * a.cos(), 20.0 + 10.0 * a.sin()];
        if k == 0 {
            e.move_to(p);
        } else {
            e.line_to(p);
        }
    }
    rec.path(g, &e);
    rec.set_out_limits(0, 12);
    let b = Built::build(rec, 1.0, Vec::new(), &[], "Model", vec!["Model".into()], [0, 0, 0], OriginMode::World);
    assert!(!b.over_out && b.stats.boxed == 1, "{:?}", b.stats);
    assert_eq!(b.scene.batches.len(), 1);
    let (l, f) = (&b.scene.batches[0].lines, &b.scene.batches[0].fills);
    assert_eq!((l.len(), f.len()), (8, 0));
    let bounds = |v: &[[f32; 2]]| {
        (v.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min), v.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min), v.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max), v.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max))
    };
    let (x0, y0, x1, y1) = bounds(l);
    assert!((x0 - 20.0).abs() < 0.1 && (x1 - 80.0).abs() < 0.1 && (y0 - 10.0).abs() < 0.1 && (y1 - 30.0).abs() < 0.1, "{:?}", bounds(l));
    // every segment runs along a side
    assert!(l.chunks(2).all(|s| s[0][0] == s[1][0] || s[0][1] == s[1][1]));
    // the bounds of the scene are the same as they would be with the whole shape
    let sb = b.scene.bounds.expect("bounds");
    assert!((sb.0[0] - 20.0).abs() < 0.2 && (sb.1[0] - 80.0).abs() < 0.2);
}

#[test]
fn a_text_that_would_not_fit_the_elements_is_recorded_as_a_box_of_five_elements() {
    // elements for 4 000 (a text is 517): the outlines of the first ones, the box of every other
    let n = 30;
    let d = texts(n);
    let mut fe = Frontend::new(d.document(), false, true, fonts(), vec!["model".to_string()]);
    fe.rec.set_limits(4_000, MAX_SCENE_PRIMS);
    fe.draw_layout("model", 0);
    assert!(!fe.rec.is_over(), "nothing is refused: what does not fit is a box");
    assert_eq!(fe.rec.prims.len(), n);
    let boxes: Vec<crate::scene::Prim> = fe.rec.prims.iter().filter(|p| p.flags & FLAG_BOX != 0).copied().collect();
    assert!(boxes.len() >= 20 && boxes.len() < n, "{} boxes", boxes.len());
    assert!(boxes.iter().all(|p| p.ecount == 5 && p.flags & FLAG_TEXT != 0));
    assert!(fe.rec.e_xy.len() <= 4_000, "{}", fe.rec.e_xy.len());
    // the outlines were budgeted to three quarters of the elements
    let outlines: usize = fe.rec.prims.iter().filter(|p| p.flags & FLAG_BOX == 0).map(|p| p.ecount as usize).sum();
    assert!(outlines <= 4_000 / 4 * 3, "{outlines}");
    // and the whole drawing is a scene
    let b = Built::build(std::mem::take(&mut fe.rec), 1.0, Vec::new(), &[], "Model", vec!["Model".into()], [0, 0, 0], OriginMode::World);
    assert!(!b.over_out);
    assert_eq!(b.stats.boxed, boxes.len());
}

#[test]
fn the_box_of_a_text_has_the_position_angle_and_width_of_the_text() {
    let font = crate::font::default_font(&fonts());
    let w = font.advance_width(TEXT17, 2.5, 1.0);
    let d = Doc::new().add(ent!("TEXT"; 8 => "0", 10 => 100.0, 20 => 50.0, 30 => 0.0, 40 => 2.5, 50 => 30.0, 1 => TEXT17)).load();
    let mut fe = Frontend::new(d.document(), false, true, fonts(), vec!["model".to_string()]);
    fe.rec.set_limits(40, MAX_SCENE_PRIMS);
    fe.draw_layout("model", 0);
    assert!(!fe.rec.is_over());
    let p = fe.rec.prims[0];
    assert!(p.flags & FLAG_BOX != 0 && p.ecount == 5);
    let (_, xy) = fe.rec.elems(&p);
    let (s, c) = 30f64.to_radians().sin_cos();
    let expect = [[100.0, 50.0], [100.0 + w * c, 50.0 + w * s], [100.0 + w * c - 2.5 * s, 50.0 + w * s + 2.5 * c], [100.0 - 2.5 * s, 50.0 + 2.5 * c], [100.0, 50.0]];
    for (g, e) in xy.iter().zip(expect) {
        assert!((g[0] - e[0]).abs() < 1e-6 && (g[1] - e[1]).abs() < 1e-6, "{xy:?} expected {expect:?}");
    }
}

#[test]
fn the_budget_of_glyph_elements_is_three_quarters_of_the_scene_and_never_more_than_is_left() {
    let mut rec = Recorder::new();
    rec.set_limits(900, 1_000);
    assert_eq!(rec.glyph_elems_left(), 675);
    rec.add_glyph_elems(100);
    assert_eq!(rec.glyph_elems_left(), 575);
    // other primitives take elements too: what is left of the scene is the limit
    let g = rec.group(GKind::S, &[0], [255; 4], 0.0);
    for i in 0..390 {
        rec.line(g, [0.0, i as f64], [1.0, i as f64 + 0.5]);
    }
    assert_eq!(rec.elems_left(), 900 - 780);
    assert_eq!(rec.glyph_elems_left(), 120);
    rec.add_glyph_elems(10_000);
    assert_eq!(rec.glyph_elems_left(), 0);
}

#[test]
fn one_huge_line_of_text_is_a_box_and_makes_no_outlines_at_all() {
    // 47 000 letters in one line, 1.3 million path elements: more than a line of text gets, and far less than the scene
    let long = "Pozice 12 M12x1,5 ".repeat(2_600);
    let d = Doc::new().add(text_at(0.0, 0.0, &long)).add(text_at(0.0, 20.0, "ahoj")).load();
    let mut fe = Frontend::new(d.document(), false, true, fonts(), vec!["model".to_string()]);
    fe.draw_layout("model", 0);
    assert!(!fe.rec.is_over());
    assert_eq!(fe.rec.prims.len(), 2);
    assert!(fe.rec.prims[0].flags & FLAG_BOX != 0, "the long line is a box");
    assert!(fe.rec.prims[1].flags & FLAG_BOX == 0, "the short one has its outlines");
    assert!(fe.rec.e_xy.len() < 1_000, "{}", fe.rec.e_xy.len());
    // a text near the limit still gets its outlines
    let ok = "Pozice 12 M12x1,5 ".repeat(100);
    let d = Doc::new().add(text_at(0.0, 0.0, &ok)).load();
    let mut fe = Frontend::new(d.document(), false, true, fonts(), vec!["model".to_string()]);
    fe.draw_layout("model", 0);
    assert!(fe.rec.prims[0].flags & FLAG_BOX == 0 && fe.rec.e_xy.len() > 40_000);
}

#[test]
fn a_box_in_place_of_a_text_is_two_triangles_without_an_outline() {
    let d = Doc::new().add(text_at(0.0, 0.0, TEXT17)).load();
    let mut fe = Frontend::new(d.document(), false, true, fonts(), vec!["model".to_string()]);
    fe.rec.set_limits(40, MAX_SCENE_PRIMS);
    fe.draw_layout("model", 0);
    let b = Built::build(std::mem::take(&mut fe.rec), 1.0, Vec::new(), &[], "Model", vec!["Model".into()], [0, 0, 0], OriginMode::World);
    assert_eq!(b.scene.batches.len(), 1);
    assert_eq!(b.scene.batches[0].fills.len(), 6, "the box is its two triangles, no outline");
    assert!(b.scene.batches[0].lines.is_empty());
    assert_eq!(b.stats.boxed, 1);
}

// ------------------------------------------------------------------------------------------------ 2: curves keep their quality
/// 200 circles and 200 arcs on a grid.
fn curves() -> Doc {
    let mut d = Doc::new();
    for i in 0..200 {
        let (x, y) = ((i % 20) as f64 * 15.0, (i / 20) as f64 * 15.0);
        d.entities.push_str(&ent!("CIRCLE"; 8 => "0", 10 => x, 20 => y, 30 => 0.0, 40 => 5.0));
        d.entities.push_str(&ent!("ARC"; 8 => "0", 10 => x, 20 => y + 160.0, 30 => 0.0, 40 => 5.0, 50 => 10.0, 51 => 250.0));
    }
    d
}

fn line_verts(sc: &Scene2d) -> usize {
    sc.batches.iter().map(|b| b.lines.len()).sum()
}

#[test]
fn circles_and_arcs_are_never_reduced_however_little_memory_there_is() {
    let reference = curves().load();
    let a = reference.scene_checked("Model").expect("scene");
    assert!(reference.scene_notes("Model").is_empty());
    // a circle is not a 16-gon: every circle and arc has many segments
    assert!(line_verts(&a) > 400 * 2 * 16, "{}", line_verts(&a));
    // the detail of the scene is reduced from 100 vertices on (a small machine: the soft limit is a fifth of 8 million),
    // and the hard limit is what the machine has: the curves are the same
    for (soft, max) in [(100u64, 10_000_000u64), (0, 10_000_000), (100, 1_000_000), (5_000, 200_000)] {
        let mut d = curves().load();
        d.set_out_limits(soft, max);
        let b = d.scene_checked("Model").unwrap_or_else(|e| panic!("{soft} {max}: {e}"));
        assert!(batches_equal(&a.batches, &b.batches), "{soft} {max}: the curves differ from the full detail");
        assert!(d.scene_notes("Model").is_empty(), "{soft} {max}: {:?}", d.scene_notes("Model"));
        let st = stats_of(&d);
        assert_eq!((st.simplified, st.boxed), (0, 0), "{soft} {max}");
    }
    // the limits that the memory of a small computer gives: 256 MB free is the floor
    let (soft, max) = crate::memory::out_limits_for(Some(GB / 4));
    let mut d = curves().load();
    d.set_out_limits(soft, max);
    assert!(batches_equal(&a.batches, &d.scene_checked("Model").unwrap().batches));
}

#[test]
fn only_the_filled_shapes_are_reduced_next_to_curves() {
    // texts and circles in one drawing, over the soft limit: the texts are simplified and the circles are what they are
    let mut doc = curves();
    for i in 0..60 {
        doc.entities.push_str(&text_at((i % 20) as f64 * 15.0, 400.0 + (i / 20) as f64 * 6.0, TEXT17));
    }
    let reference = doc.load();
    let r = reference.scene_checked("Model").expect("scene");
    assert!(reference.scene_notes("Model").is_empty());
    let mut d = doc.load();
    d.set_out_limits(30_000, 10_000_000);
    let sc = d.scene_checked("Model").expect("scene");
    let st = stats_of(&d);
    assert!(st.simplified > 0 && st.boxed == 0, "{st:?}");
    // the batches that are only strokes are the same
    let strokes = |s: &Scene2d| -> Vec<[f32; 2]> { s.batches.iter().filter(|b| b.fills.is_empty()).flat_map(|b| b.lines.clone()).collect() };
    assert!(!strokes(&r).is_empty() && strokes(&r) == strokes(&sc), "the circles and arcs are not the same");
    assert!(verts(&sc) < verts(&r));
    assert_eq!(d.scene_notes("Model").len(), 1);
}

#[test]
fn strokes_never_get_a_lower_level_than_full() {
    let mut rec = Recorder::new();
    let g = rec.group(GKind::S, &[0], [255; 4], 0.0);
    for i in 0..50 {
        let mut e = Elems::new();
        e.move_to([i as f64 * 20.0 + 10.0, 0.0]);
        e.curve_to([i as f64 * 20.0 + 10.0, 5.5], [i as f64 * 20.0 + 5.5, 10.0], [i as f64 * 20.0, 10.0]);
        rec.path(g, &e);
    }
    let reference = {
        let mut r = Recorder::new();
        let g = r.group(GKind::S, &[0], [255; 4], 0.0);
        for i in 0..50 {
            let mut e = Elems::new();
            e.move_to([i as f64 * 20.0 + 10.0, 0.0]);
            e.curve_to([i as f64 * 20.0 + 10.0, 5.5], [i as f64 * 20.0 + 5.5, 10.0], [i as f64 * 20.0, 10.0]);
            r.path(g, &e);
        }
        Built::build(r, 1.0, Vec::new(), &[], "Model", vec!["Model".into()], [0, 0, 0], OriginMode::World)
    };
    rec.set_out_limits(10, 1_000_000);
    let b = Built::build(rec, 1.0, Vec::new(), &[], "Model", vec!["Model".into()], [0, 0, 0], OriginMode::World);
    assert!(b.notes.is_empty() && b.stats.simplified == 0 && b.stats.boxed == 0, "{:?}", b.stats);
    assert!(batches_equal(&b.scene.batches, &reference.scene.batches));
}
