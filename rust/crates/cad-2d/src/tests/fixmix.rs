//! Regression tests of the fifth review: a drawing that mixes texts with circles and arcs is drawn whatever the order of its
//! entities and the memory that is free (the strokes are counted as they are made, so the texts recorded before them leave
//! the room they need), and a scene that overflows although its strokes and boxes fit is made again with less detail for the
//! filled shapes instead of being refused.
use super::*;
use crate::scene::test_hooks;

const TEXT17: &str = "Pozice 12 M12x1,5";
/// Texts of `texts_and_curves`, in rows of 30, 40 apart.
const TEXTS: usize = 300;
/// Circles and arcs of `texts_and_curves`: 30 of each, of the radius of 60, on a grid 150 apart.
const CURVES: usize = 60;

fn text_at(x: f64, y: f64) -> String {
    ent!("TEXT"; 8 => "0", 10 => x, 20 => y, 30 => 0.0, 40 => 2.5, 1 => TEXT17)
}

fn add_texts(d: &mut Doc) {
    for i in 0..TEXTS {
        d.entities.push_str(&text_at((i % 30) as f64 * 40.0, (i / 30) as f64 * 6.0));
    }
}

fn add_curves(d: &mut Doc) {
    for i in 0..CURVES {
        let (x, y) = ((i % 10) as f64 * 150.0, 300.0 + (i / 10) as f64 * 150.0);
        if i % 2 == 0 {
            d.entities.push_str(&ent!("CIRCLE"; 8 => "0", 10 => x, 20 => y, 30 => 0.0, 40 => 60.0));
        } else {
            d.entities.push_str(&ent!("ARC"; 8 => "0", 10 => x, 20 => y, 30 => 0.0, 40 => 60.0, 50 => 10.0, 51 => 250.0));
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Order {
    TextsFirst,
    CurvesFirst,
    /// Half of the texts, the curves, the other half of the texts.
    Between,
}

fn texts_and_curves(order: Order) -> Doc {
    let mut d = Doc::new();
    match order {
        Order::TextsFirst => {
            add_texts(&mut d);
            add_curves(&mut d);
        }
        Order::CurvesFirst => {
            add_curves(&mut d);
            add_texts(&mut d);
        }
        Order::Between => {
            for i in 0..TEXTS / 2 {
                d.entities.push_str(&text_at((i % 30) as f64 * 40.0, (i / 30) as f64 * 6.0));
            }
            add_curves(&mut d);
            for i in TEXTS / 2..TEXTS {
                d.entities.push_str(&text_at((i % 30) as f64 * 40.0, (i / 30) as f64 * 6.0));
            }
        }
    }
    d
}

fn verts(sc: &Scene2d) -> u64 {
    sc.batches.iter().map(|b| (b.lines.len() + b.fills.len()) as u64).sum()
}

/// The points of the batches that are only strokes: the circles and arcs (the outlines of the letters are in batches with fills).
fn stroke_points(sc: &Scene2d) -> Vec<[f32; 2]> {
    sc.batches.iter().filter(|b| b.fills.is_empty()).flat_map(|b| b.lines.clone()).collect()
}

/// Whether some triangle or segment lies in the box of the text number `i`.
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

/// The scene at full detail (no limit that matters), what its strokes are and the least a scene of the drawing makes: every
/// stroke as it is and a bar for every text.
fn reference(order: Order) -> (Vec<[f32; 2]>, u64) {
    let d = texts_and_curves(order).load();
    let sc = d.scene_checked("Model").expect("scene at full detail");
    assert!(d.scene_notes("Model").is_empty());
    let strokes = stroke_points(&sc);
    // a circle or arc of the radius of 60 in a drawing of this size is far more than the five points per curve it was counted as
    assert!(strokes.len() > CURVES * 150, "{} vertices of strokes", strokes.len());
    let least = strokes.len() as u64 + 6 * TEXTS as u64;
    (strokes, least)
}

#[test]
fn texts_recorded_before_circles_and_arcs_leave_the_room_that_the_curves_need() {
    for order in [Order::TextsFirst, Order::CurvesFirst, Order::Between] {
        let (strokes, least) = reference(order);
        // every memory from "just the strokes and a bar for every text" up: the texts are as detailed as there is room for, the
        // curves are what they are, and nothing is refused
        let mut last = 0;
        for extra in [0u64, 500, 5_000, 40_000, 200_000] {
            let max = least + extra;
            let mut d = texts_and_curves(order).load();
            d.set_out_limits(max / 2, max);
            let sc = d.scene_checked("Model").unwrap_or_else(|e| panic!("{order:?}, room for {extra} more: {e}"));
            assert!(verts(&sc) <= max, "{order:?} {extra}: {} of {max}", verts(&sc));
            assert_eq!(stroke_points(&sc), strokes, "{order:?} {extra}: the curves are not the same");
            for i in 0..TEXTS {
                assert!(has_geometry(&sc, i), "{order:?} {extra}: text {i} is left out");
            }
            let st = stats_of(&d);
            assert_eq!(st.passes, 1, "{order:?} {extra}: the estimates hold, nothing is made twice: {st:?}");
            // more room, more detail
            assert!(verts(&sc) >= last, "{order:?} {extra}");
            last = verts(&sc);
        }
        assert!(last > least + 20_000, "{order:?}: the room is used for the texts: {last} of {least}");
    }
}

#[test]
fn a_scene_is_refused_only_when_the_strokes_and_the_bars_do_not_fit() {
    for order in [Order::TextsFirst, Order::CurvesFirst, Order::Between] {
        let (_, least) = reference(order);
        // one vertex too few: refused, and the refusal is the memory's, not the drawing's
        let mut d = texts_and_curves(order).load();
        d.set_out_limits(least / 2, least - 1);
        let e = d.scene_checked("Model").expect_err("the strokes and the bars do not fit");
        // (the number is written with a space between the thousands)
        assert!(e.0.starts_with("Výkres se při vykreslení rozpadne na více než ") && e.0.replace(' ', "").contains(&format!("než{}bodů", least - 1)), "{order:?}: {}", e.0);
        assert!(d.check_budget("Model").is_ok());
        // the number of vertices that it takes is exactly what the strokes and the bars make
        d.set_out_limits(least / 2, least);
        let sc = d.scene_checked("Model").unwrap_or_else(|e| panic!("{order:?}: {e}"));
        assert_eq!(verts(&sc), least, "{order:?}");
        assert_eq!(stats_of(&d).boxed, TEXTS);
    }
}

#[test]
fn the_curves_are_counted_before_the_texts_take_the_room_through_the_call_of_the_viewer() {
    // the same through `scene_cancellable`, which hands the geometry over (what the window and the thumbnail call)
    let (strokes, least) = reference(Order::TextsFirst);
    let cancel = Arc::new(AtomicBool::new(false));
    let max = least + 3_000;
    let mut d = texts_and_curves(Order::TextsFirst).load();
    d.set_out_limits(max / 2, max);
    let sc = d.scene_cancellable("Model", &SceneOptions::default(), &cancel).expect("scene");
    assert!(verts(&sc) <= max);
    assert_eq!(stroke_points(&sc), strokes);
    assert!((0..TEXTS).all(|i| has_geometry(&sc, i)));
}

// ------------------------------------------------------------------------------------------------ the safety net
#[test]
fn a_scene_that_overflows_although_the_strokes_and_the_bars_fit_is_made_again_with_less_detail_for_the_texts() {
    for order in [Order::TextsFirst, Order::Between] {
        let (strokes, least) = reference(order);
        for extra in [0u64, 4_000, 30_000] {
            let max = least + extra;
            // the estimate of the curves is eight times too low (it was five points per curve): the first pass gives the texts
            // room that the curves need and overflows
            let _lie = test_hooks::underestimate_curved_strokes(8);
            let mut d = texts_and_curves(order).load();
            d.set_out_limits(max / 2, max);
            let sc = d.scene_checked("Model").unwrap_or_else(|e| panic!("{order:?}, {extra}: refused instead of made again: {e}"));
            let st = stats_of(&d);
            assert!(st.passes > 1 && st.passes <= 4, "{order:?} {extra}: {st:?}");
            assert!(verts(&sc) <= max, "{order:?} {extra}: {} of {max}", verts(&sc));
            assert_eq!(stroke_points(&sc), strokes, "{order:?} {extra}");
            for i in 0..TEXTS {
                assert!(has_geometry(&sc, i), "{order:?} {extra}: text {i} is left out");
            }
            // what could not be drawn in full is said so
            assert!(!d.scene_notes("Model").is_empty(), "{order:?} {extra}");
        }
    }
}

#[test]
fn when_there_is_no_room_for_more_than_bars_the_passes_end_with_a_bar_for_every_text_and_a_scene_that_does_not_fit_is_refused() {
    let (strokes, least) = reference(Order::TextsFirst);
    // exactly the strokes and the bars: only a pass with every text a bar has room for it
    let _lie = test_hooks::underestimate_curved_strokes(8);
    let mut d = texts_and_curves(Order::TextsFirst).load();
    d.set_out_limits(least / 2, least);
    let sc = d.scene_checked("Model").expect("made again until it fits");
    let st = stats_of(&d);
    assert!(st.passes >= 2 && st.passes <= 4, "{st:?}");
    assert_eq!(st.boxed, TEXTS, "{st:?}");
    assert_eq!(verts(&sc), least);
    assert_eq!(stroke_points(&sc), strokes);
    // with less room than the strokes alone, nothing helps: refused, after the passes that there are
    let mut d = texts_and_curves(Order::TextsFirst).load();
    d.set_out_limits(100, strokes.len() as u64 / 2);
    assert!(d.scene_checked("Model").is_err());
}

#[test]
fn a_scene_that_fits_is_made_once() {
    let d = texts_and_curves(Order::TextsFirst).load();
    d.scene_checked("Model").expect("scene");
    assert_eq!(stats_of(&d).passes, 1);
}

// ------------------------------------------------------------------------------------------------ the notes come first
#[test]
fn the_warnings_of_the_drawing_are_the_first_rows_of_the_panel() {
    // 300 texts that do not fit in full detail: a note about the simplified outlines and one about the bars; and a 3D solid
    // that the 2D view does not show
    let mut doc = texts_and_curves(Order::TextsFirst);
    doc.entities.push_str(&ent!("3DSOLID"; 8 => "0"));
    let mut d = doc.load();
    d.set_out_limits(20_000, 120_000);
    let sc = d.scene_checked("Model").expect("scene");
    let rows = d.info_rows("Model", sc.bounds, Some(1.5));
    let warnings: Vec<&crate::InfoRow> = rows.iter().filter(|r| r.warning).collect();
    assert!(warnings.len() >= 3, "{rows:?}");
    assert!(warnings.iter().any(|r| r.value.contains("zjednodušen")) && warnings.iter().any(|r| r.value.contains("obdélník")), "{warnings:?}");
    // every warning is above every other row
    let first_plain = rows.iter().position(|r| !r.warning).expect("rows that are not warnings");
    assert_eq!(first_plain, warnings.len(), "the warnings are the first {} rows: {rows:?}", warnings.len());
    assert!(rows[..first_plain].iter().all(|r| r.warning && r.label.starts_with("Upozorn")));
    // the rest is as it was: the folder first, the entity types after the size, the load time last
    let rest: Vec<&str> = rows[first_plain..].iter().map(|r| r.label.as_str()).collect();
    assert_eq!(rest[0], "Složka");
    assert_eq!(rest[1], "Velikost");
    assert_eq!(*rest.last().unwrap(), "Načteno za");
    assert!(rest.iter().position(|l| *l == "Typy entit").unwrap() > rest.iter().position(|l| *l == "Entit").unwrap());
    // a drawing without warnings has its rows in the same order as before
    let plain = Doc::new().add(line("0", [0.0, 0.0], [10.0, 0.0], None)).load();
    let sc = plain.scene_checked("Model").expect("scene");
    let rows = plain.info_rows("Model", sc.bounds, None);
    assert!(rows.iter().all(|r| !r.warning) && rows[0].label == "Složka");
}
