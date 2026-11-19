//! Regression tests of the third review: the highlight of a big entity is bounded, the drawing keeps one scene (handed over,
//! not copied), paper space does not depend on the white background option, and the memory a scene may take is tied to
//! the memory of the computer.
use super::*;
use crate::scene::{Built, EntityId, HIGHLIGHT_MAX_BOXES, HIGHLIGHT_MAX_POINTS};
use std::sync::atomic::AtomicBool;

/// A block of `n` short lines on a grid, inserted once.
fn block_reference(n: usize) -> Drawing {
    let body: String = (0..n).map(|i| line("0", [(i % 50) as f64 * 3.0, (i / 50) as f64 * 3.0], [(i % 50) as f64 * 3.0 + 2.0, (i / 50) as f64 * 3.0 + 1.0], None)).collect();
    Doc::new()
        .block("B", [0.0, 0.0], &body)
        .add(ent!("INSERT"; 2 => "B", 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0))
        .add(line("0", [500.0, 500.0], [510.0, 500.0], None))
        .load()
}

fn insert_id(d: &Drawing) -> EntityId {
    let hit = d.pick([1.0, 0.5], 0.5).expect("the block reference is under the point");
    assert_eq!(hit.dxftype, "INSERT");
    hit.id
}

/// The same batches, field by field (`DrawBatch` has no equality).
fn same(a: &[DrawBatch], b: &[DrawBatch]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.layer == y.layer && x.color == y.color && x.width == y.width && x.lines == y.lines && x.fills == y.fills)
}

fn points(batches: &[DrawBatch]) -> usize {
    batches.iter().map(|b| b.lines.len()).sum()
}

fn bounds_of(batches: &[DrawBatch]) -> [f32; 4] {
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for p in batches.iter().flat_map(|b| b.lines.iter()) {
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    }
    b
}

// ------------------------------------------------------------------------------------------------ highlight
#[test]
fn the_highlight_of_a_block_reference_is_bounded_and_still_covers_all_of_it() {
    let n = 600;
    let d = block_reference(n);
    d.scene("Model");
    let id = insert_id(&d);
    // as long as it is small, it is the outline of every line of the block, as it always was
    let full = d.highlight(id);
    assert_eq!(full.len(), 1, "one pen width");
    assert_eq!(points(&full), 2 * n);
    assert!(full[0].color == [255, 140, 0, 255] && full[0].width >= 2.0);
    let want = bounds_of(&full);

    let (built, vis) = d.active().expect("active scene");
    // a budget of 100 points: 50 lines are outlined, then the next 5 primitives get their rectangles (8 points each) and the
    // other 545 share one
    let capped = built.highlight_with(&vis, id, 100, 5);
    let total = points(&capped);
    assert_eq!(total, 100 + 5 * 8 + 8, "{total}");
    // nothing of the block is left without a mark: the marks cover the same area
    let got = bounds_of(&capped);
    for k in 0..4 {
        assert!((got[k] - want[k]).abs() < 1e-3, "{got:?} vs {want:?}");
    }
    // without rectangles the primitives after the budget share one rectangle
    let one = built.highlight_with(&vis, id, 100, 0);
    assert_eq!(points(&one), 100 + 8);
    // everything fits: no rectangle at all
    assert_eq!(points(&built.highlight_with(&vis, id, 2 * n, 0)), 2 * n);
    // the defaults are what the graphics card takes without trouble: 48 MB of segments at the most
    assert!(HIGHLIGHT_MAX_POINTS / 2 * 24 <= 48_000_000);
    assert!(HIGHLIGHT_MAX_BOXES * 8 <= HIGHLIGHT_MAX_POINTS);
}

#[test]
fn filled_shapes_are_outlined_without_making_their_triangles() {
    // a text is a filled path: the highlight strokes its outline and leaves the fill to the drawing
    let d = Doc::new().add(ent!("TEXT"; 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0, 40 => 5.0, 1 => "HEJ")).load();
    d.scene("Model");
    let id = d.pick([1.0, 1.0], 3.0).map(|h| h.id).expect("the text");
    let hl = d.highlight(id);
    assert!(!hl.is_empty() && hl.iter().all(|b| b.fills.is_empty() && !b.lines.is_empty()));
    assert_eq!(points(&hl) % 2, 0);
}

// ------------------------------------------------------------------------------------------------ one scene in memory
fn cached(d: &Drawing) -> Vec<(usize, bool, bool)> {
    d.state.lock().unwrap().cache.iter().map(|e| (e.key.0, e.key.1, e.geometry.is_some())).collect()
}

#[test]
fn the_viewer_takes_the_geometry_and_the_drawing_keeps_one_build_without_it() {
    let d = block_reference(300);
    let cancel = Arc::new(AtomicBool::new(false));
    let opts = SceneOptions::default();
    let s1 = d.scene_cancellable("Model", &opts, &cancel).expect("scene");
    assert!(!s1.batches.is_empty());
    // one build, and the geometry went to the caller: nothing of it is left in the drawing
    assert_eq!(cached(&d), vec![(0, false, false)]);
    {
        let st = d.state.lock().unwrap();
        assert!(st.cache[0].built.scene.batches.is_empty() && st.cache[0].built.scene.images.is_empty());
    }
    // what the drawing is asked about works without the geometry
    let id = insert_id(&d);
    assert_eq!(d.visible_batches().len(), s1.batches.len());
    assert!(d.entity_bounds(id).is_some() && !d.highlight(id).is_empty());
    assert!(d.snap_points([0.0, 0.0], [10.0, 10.0]).len() > 4);
    // the same scene again is made again (the caller has the geometry), with the same result
    let built = d.state.lock().unwrap().cache[0].built.clone();
    let s2 = d.scene_cancellable("Model", &opts, &cancel).expect("scene");
    assert!(same(&s2.batches, &s1.batches));
    assert!(!Arc::ptr_eq(&built, &d.state.lock().unwrap().cache[0].built), "built again");
    assert_eq!(cached(&d), vec![(0, false, false)]);
    // a change of the background is another scene: the old build is gone, there is still one
    let light = d.scene_cancellable("Model", &SceneOptions { light_bg: true, ..opts }, &cancel).expect("scene");
    assert_eq!(cached(&d), vec![(0, true, false)]);
    assert_eq!(light.batches.len(), s1.batches.len());
    assert!(s1.batches.iter().any(|b| b.color == [255, 255, 255, 255]) && light.batches.iter().all(|b| b.color != [255, 255, 255, 255]));
    assert_eq!(d.background_color(), [255, 255, 255]);
}

#[test]
fn a_scene_asked_for_without_taking_it_is_kept_and_not_built_again() {
    let d = block_reference(50);
    let a = d.scene_with("Model", &SceneOptions::default()).expect("scene");
    assert_eq!(cached(&d), vec![(0, false, true)]);
    let first = d.state.lock().unwrap().cache[0].built.clone();
    let b = d.scene_with("Model", &SceneOptions::default()).expect("scene");
    assert!(same(&a.batches, &b.batches));
    assert!(Arc::ptr_eq(&first, &d.state.lock().unwrap().cache[0].built), "the same build");
    // the viewer takes it from there: no new build, the copy of the drawing is gone
    let c = d.scene_cancellable("Model", &SceneOptions::default(), &Arc::new(AtomicBool::new(false))).expect("scene");
    assert!(same(&a.batches, &c.batches));
    assert!(Arc::ptr_eq(&first, &d.state.lock().unwrap().cache[0].built));
    assert_eq!(cached(&d), vec![(0, false, false)]);
}

/// Model space with a line, and a paper space layout with a line.
fn two_spaces() -> Drawing {
    let mut d = Doc::new();
    d.entities = format!(
        "{}{}",
        line("0", [0.0, 0.0], [100.0, 100.0], None),
        ent!("LINE"; 8 => "0", 67 => 1, 10 => 0.0, 20 => 0.0, 30 => 0.0, 11 => 50.0, 21 => 50.0, 31 => 0.0)
    );
    d.objects = format!(
        "{}{}",
        ent!("LAYOUT"; 5 => "50", 100 => "AcDbLayout", 1 => "Model", 70 => 1, 71 => 0, 330 => "1F"),
        ent!("LAYOUT"; 5 => "51", 100 => "AcDbLayout", 1 => "Sheet A", 70 => 1, 71 => 1, 330 => "1E")
    );
    d.load()
}

#[test]
fn a_layout_of_paper_space_is_white_with_black_pens_and_the_option_does_not_make_a_new_scene_of_it() {
    let d = two_spaces();
    let cancel = Arc::new(AtomicBool::new(false));
    let dark = SceneOptions { light_bg: false, ..Default::default() };
    let light = SceneOptions { light_bg: true, ..Default::default() };
    let a = d.scene_cancellable("Sheet A", &dark, &cancel).expect("scene");
    assert_eq!(a.batches[0].color, [0, 0, 0, 255], "black pens");
    assert_eq!(d.background_color(), [255, 255, 255], "on white paper, as 1.9 shows a sheet");
    let build = d.state.lock().unwrap().cache[0].built.clone();
    // the white background option changes nothing on a sheet: the build is the same, nothing is built or kept again
    let _b = d.scene_cancellable("Sheet A", &light, &cancel).expect("scene");
    assert_eq!(cached(&d).len(), 1);
    assert_eq!(d.state.lock().unwrap().cache[0].key, (1, false, OriginMode::World, true));
    assert_eq!(d.background_color(), [255, 255, 255]);
    let _ = build; // the geometry was taken by the first call, so the second made the sheet again: same key, same colours
    assert_eq!(_b.batches[0].color, [0, 0, 0, 255]);
    // model space follows the option
    assert_eq!(d.layout_background("Model", false), [0x21, 0x28, 0x30]);
    assert_eq!(d.layout_background("Model", true), [255, 255, 255]);
    for light_bg in [false, true] {
        assert_eq!(d.layout_background("Sheet A", light_bg), [255, 255, 255], "paper space is white either way");
        assert_eq!(d.layout_background("sheet a", light_bg), [255, 255, 255]);
    }
    // switching between the layouts keeps one build and the background of the active one
    d.scene_cancellable("Model", &dark, &cancel).expect("scene");
    assert_eq!(cached(&d), vec![(0, false, false)]);
    assert_eq!(d.background_color(), [0x21, 0x28, 0x30]);
    d.scene_cancellable("Sheet A", &dark, &cancel).expect("scene");
    assert_eq!(cached(&d), vec![(1, false, false)]);
    assert_eq!(d.background_color(), [255, 255, 255]);
}

#[test]
fn a_cancelled_build_keeps_the_scene_that_is_shown() {
    let d = two_spaces();
    let cancel = Arc::new(AtomicBool::new(false));
    d.scene_cancellable("Model", &SceneOptions::default(), &cancel).expect("scene");
    let shown = d.state.lock().unwrap().cache[0].built.clone();
    let stop = Arc::new(AtomicBool::new(true));
    assert!(matches!(d.scene_cancellable("Sheet A", &SceneOptions::default(), &stop), Err(SceneError::Cancelled)));
    // the model scene is still the active one: picking works and nothing was built on the way
    assert!(Arc::ptr_eq(&shown, &d.state.lock().unwrap().cache[0].built));
    assert_eq!(d.background_color(), [0x21, 0x28, 0x30]);
    assert!(d.pick([50.0, 50.0], 1.0).is_some());
}

#[test]
fn the_built_scene_without_its_geometry_answers_the_same_as_with_it() {
    // `Built::build` itself still holds its batches (thumbnails and tests read them); the drawing takes them out
    let d = block_reference(20);
    let sc = d.scene_with("Model", &SceneOptions::default()).expect("scene");
    let (built, _) = d.active().expect("active");
    assert!(built.scene.batches.is_empty());
    assert_eq!(sc.layers.iter().map(|l| (l.id, l.name.clone())).collect::<Vec<_>>(), built.scene.layers.iter().map(|l| (l.id, l.name.clone())).collect::<Vec<_>>());
    let _: &Built = &built;
}

// ------------------------------------------------------------------------------------------------ limits tied to memory
/// `n` short lines: strokes, which are never reduced.
fn lines(n: usize) -> Drawing {
    let mut d = Doc::new();
    for i in 0..n {
        d.entities.push_str(&line("0", [i as f64, 0.0], [i as f64, 1.0], None));
    }
    d.load()
}

fn texts(n: usize) -> Drawing {
    let mut d = Doc::new();
    for i in 0..n {
        d.entities.push_str(&ent!("TEXT"; 8 => "0", 10 => (i % 30) as f64 * 40.0, 20 => (i / 30) as f64 * 6.0, 30 => 0.0, 40 => 2.5, 1 => "Pozice 12 M12x1,5"));
    }
    d.load()
}

#[test]
fn a_scene_that_cannot_fit_even_with_reduced_detail_is_refused_before_its_geometry_is_made() {
    use crate::frontend::Frontend;
    let dr = texts(200);
    // limits far below what 200 texts make, even simplified
    let mut fe = Frontend::new(dr.document(), false, true, fonts(), vec!["model".to_string()]);
    // the box of a text is 6 vertices, 200 of them are 1 200: far above twice 400
    fe.rec.set_out_limits(100, 400);
    fe.draw_layout("model", 0);
    let rec = std::mem::take(&mut fe.rec);
    let b = Built::build(rec, 1.0, Vec::new(), &[], "Model", vec!["Model".to_string()], [0, 0, 0], OriginMode::World);
    assert!(b.over_out, "refused");
    assert_eq!(b.stats.slices, 0, "nothing was flattened: {:?}", b.stats);
    assert!(b.scene.batches.is_empty());
    // limits that the simplified detail fits are not a refusal: the same texts at 20 000 / 1 000 000
    let mut fe = Frontend::new(dr.document(), false, true, fonts(), vec!["model".to_string()]);
    fe.rec.set_out_limits(20_000, 1_000_000);
    fe.draw_layout("model", 0);
    let rec = std::mem::take(&mut fe.rec);
    let b = Built::build(rec, 1.0, Vec::new(), &[], "Model", vec!["Model".to_string()], [0, 0, 0], OriginMode::World);
    assert!(!b.over_out && b.stats.simplified > 0 && !b.scene.batches.is_empty(), "{:?}", b.stats);
}

#[test]
fn the_hard_limit_of_a_scene_follows_the_memory_of_the_computer() {
    let d = texts(40);
    // the limits a scene is built with are the configured ones, lowered by the memory that is free
    let (soft, max) = d.effective_out_limits();
    let (msoft, mmax) = crate::memory::out_limits_for(crate::memory::available_physical_bytes());
    // (the free memory of the computer is read twice and moves in between: within one per cent is the same)
    let near = |a: u64, b: u64| a.abs_diff(b) <= a.max(b) / 100;
    assert!(near(soft, d.out_limits.0.min(msoft)) && near(max, d.out_limits.1.min(mmax)), "{soft} {max} / {msoft} {mmax}");
    assert!(max <= crate::scene::MAX_OUT_VERTS && soft < max);
    // a limit that the caller sets lower is kept
    let mut d = texts(40);
    d.set_out_limits(10_000, 20_000);
    assert_eq!(d.effective_out_limits(), (10_000.min(msoft), 20_000));
}

// ------------------------------------------------------------------------------------------------ colours of the background
/// Pens of every kind of colour: BYLAYER on layer 0 (ACI 7), the ACI colour 7, a true colour that happens to be white, ACI
/// 255 (white too), red, and a BYBLOCK pen in a block that two references give the colour 7 and the colour 1.
fn pens() -> Drawing {
    let blk = format!("{}", ent!("LINE"; 8 => "0", 62 => 0, 10 => 0.0, 20 => 0.0, 30 => 0.0, 11 => 5.0, 21 => 0.0, 31 => 0.0));
    Doc::new()
        .block("B", [0.0, 0.0], &blk)
        .add(line("0", [0.0, 0.0], [10.0, 0.0], None))
        .add(format!("{}{}", line("0", [0.0, 10.0], [10.0, 10.0], None), t(&[(420, &0x00FF_FFFFi32)])))
        .add(line("0", [0.0, 20.0], [10.0, 20.0], Some(7)))
        .add(line("0", [0.0, 30.0], [10.0, 30.0], Some(255)))
        .add(line("0", [0.0, 40.0], [10.0, 40.0], Some(1)))
        .add(ent!("INSERT"; 2 => "B", 8 => "0", 62 => 7, 10 => 0.0, 20 => 50.0, 30 => 0.0))
        .add(ent!("INSERT"; 2 => "B", 8 => "0", 62 => 1, 10 => 0.0, 20 => 60.0, 30 => 0.0))
        .load()
}

/// The colour of the batch that holds the line at height `y`, and its role.
fn pen_at(d: &Drawing, sc: &Scene2d, roles: &[crate::props::Role], y: f32) -> ([u8; 4], crate::props::Role) {
    // (batches are relative to the scene origin; heights are matched with the origin of the scene)
    let k = sc
        .batches
        .iter()
        .position(|b| b.lines.chunks_exact(2).any(|w| (f64::from(w[0][1]) + sc.origin[1] - f64::from(y)).abs() < 1e-3 && (w[0][1] - w[1][1]).abs() < 1e-6))
        .unwrap_or_else(|| panic!("no line at height {y}"));
    let _ = d;
    (sc.batches[k].color, roles[k])
}

#[test]
fn the_foreground_and_the_background_are_recorded_as_roles_so_that_the_colours_can_change_without_a_new_scene() {
    use crate::props::Role;
    let d = pens();
    let dark = d.scene_with("Model", &SceneOptions { light_bg: false, ..Default::default() }).unwrap();
    let roles_dark = d.batch_roles();
    let light = d.scene_with("Model", &SceneOptions { light_bg: true, ..Default::default() }).unwrap();
    let roles_light = d.batch_roles();
    assert_eq!(roles_dark, roles_light, "the roles do not depend on the background");
    assert_eq!(roles_dark.len(), dark.batches.len());
    // (y, role, colour on the dark background, colour on the white one)
    let white = [255, 255, 255, 255];
    let black = [0, 0, 0, 255];
    let want = [
        (0.0f32, Role::Fg, white, black),          // BYLAYER, layer 0 has the colour 7
        (10.0, Role::None, white, white),          // a true colour that is white: stays white on white
        (20.0, Role::Fg, white, black),            // the colour 7
        (30.0, Role::None, white, white),          // ACI 255 is white as well, and stays
        (40.0, Role::None, [255, 0, 0, 255], [255, 0, 0, 255]),
        (50.0, Role::Fg, white, black),            // BYBLOCK in a block whose reference has the colour 7
        (60.0, Role::None, [255, 0, 0, 255], [255, 0, 0, 255]),
    ];
    for (y, role, on_dark, on_light) in want {
        let (c, r) = pen_at(&d, &dark, &roles_dark, y);
        assert_eq!((c, r), (on_dark, role), "dark, height {y}");
        let (c, r) = pen_at(&d, &light, &roles_light, y);
        assert_eq!((c, r), (on_light, role), "light, height {y}");
    }
    // so the scene of the dark background with the foreground changed is the scene of the white one
    let swap = |sc: &Scene2d, roles: &[Role], fg: [u8; 3]| -> Vec<[u8; 4]> {
        sc.batches.iter().zip(roles).map(|(b, r)| if *r == Role::Fg { [fg[0], fg[1], fg[2], b.color[3]] } else { b.color }).collect()
    };
    let recolored = swap(&dark, &roles_dark, d.layout_foreground("Model", true));
    let (mut a, mut b): (Vec<_>, Vec<_>) = (recolored.iter().zip(&dark.batches).map(|(c, x)| (x.lines.clone(), *c)).collect(), light.batches.iter().map(|x| (x.lines.clone(), x.color)).collect());
    a.sort_by(|p, q| format!("{p:?}").cmp(&format!("{q:?}")));
    b.sort_by(|p, q| format!("{p:?}").cmp(&format!("{q:?}")));
    assert_eq!(a, b, "the same geometry in the same colours");
    // a picture that was not blended with the background: the colours can change
    assert!(d.colors_follow_background());
    assert_eq!(d.layout_foreground("Model", false), [255, 255, 255]);
    assert_eq!(d.layout_foreground("Model", true), [0, 0, 0]);
}

#[test]
fn the_question_about_a_scene_is_answered_with_the_build_of_the_other_background() {
    let d = pens();
    let cancel = Arc::new(AtomicBool::new(false));
    d.scene_cancellable("Model", &SceneOptions { light_bg: false, ..Default::default() }, &cancel).expect("scene");
    let built = d.state.lock().unwrap().cache[0].built.clone();
    // the option changes: no scene is made, the same build answers (picking, bounds, roles)
    d.set_light_background(true);
    assert!(d.pick([5.0, 0.0], 1.0).is_some());
    assert_eq!(d.batch_roles().len(), built.batch_roles.len());
    assert!(Arc::ptr_eq(&built, &d.state.lock().unwrap().cache[0].built), "nothing was built");
    assert_eq!(d.layout_background("Model", true), [255, 255, 255]);
}

// ------------------------------------------------------------------------------------------------ hopeless dashes
#[test]
fn a_dashed_circle_of_a_billion_units_is_drawn_as_a_curve_not_as_a_million_segments() {
    // three dashed circles of 1e9, 2e9 and 3e9 units: 1 KB of file took 5 to 7.5 GB, each circle flattened to a million
    // segments that were then drawn solid because the dashes were too many
    let mut d = Doc::new();
    for r in [1e9, 2e9, 3e9] {
        d.entities.push_str(&ent!("CIRCLE"; 8 => "0", 6 => "DASHED", 10 => 0.0, 20 => 0.0, 30 => 0.0, 40 => r));
    }
    let d = d.load();
    let t0 = std::time::Instant::now();
    let sc = d.scene_checked("Model").expect("not refused");
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
    let verts: usize = sc.batches.iter().map(|b| b.lines.len() + b.fills.len()).sum();
    assert!(verts < 20_000, "{verts} vertices for three circles (a million segments each before)");
    // all three circles are there, solid, and the Czech note says that their dashes were too fine
    assert!(sc.batches.iter().map(|b| b.lines.len()).sum::<usize>() >= 3 * 16);
    let notes = d.scene_notes("Model");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].starts_with("3 čar má příliš hustě čárkování") && notes[0].contains("plnou čarou"), "{}", notes[0]);
    // a circle whose dashes fit is dashed, as before
    let ok = Doc::new().add(ent!("CIRCLE"; 8 => "0", 6 => "DASHED", 10 => 0.0, 20 => 0.0, 30 => 0.0, 40 => 50.0)).load();
    let sc = ok.scene_checked("Model").expect("scene");
    assert!(sc.batches.iter().map(|b| b.lines.len()).sum::<usize>() > 800, "dashed circle: many pieces");
    assert!(ok.scene_notes("Model").is_empty());
}

#[test]
fn the_elements_of_a_scene_and_its_dashes_are_limited_by_the_hard_limit_too() {
    // a recorded element makes at least one output vertex: with a hard limit of 5 000 the recording stops at 5 000 elements
    // and the scene is refused before any geometry is made (strokes: texts are boxes of six vertices and fit)
    let mut dr = lines(3_000);
    dr.set_out_limits(u64::MAX, 5_000);
    let e = dr.scene_checked("Model").unwrap_err();
    assert!(e.0.contains("5 000 bodů"), "{}", e.0);
}

// ------------------------------------------------------------------------------------------------ hatches share the budget
/// The hatch of the repro file: a pattern of dashes 150 / 100 every 0.0051 over a square of 1000, 1.5 million path elements.
const DENSE_HATCH: &str = "  0
HATCH
  8
0
 10
0
 20
0
 30
0
210
0
220
0
230
1
  2
X
 70
0
 71
0
 91
1
 92
3
 72
0
 73
1
 93
4
 10
0
 20
0
 10
1000
 20
0
 10
1000
 20
1000
 10
0
 20
1000
 97
0
 75
0
 76
1
 52
0
 41
1
 77
0
 78
1
 53
0
 43
0
 44
0
 45
0
 46
0.0051
 79
2
 49
150
 49
-100
 98
0
";

#[test]
fn dense_hatches_share_one_budget_and_the_ones_after_it_is_used_up_are_filled_solid() {
    let src = format!("  0
SECTION
  2
ENTITIES
{}  0
ENDSEC
  0
EOF
", DENSE_HATCH.repeat(4));
    let mut d = load_bytes(src.as_bytes(), fonts()).expect("drawing");
    // room for one of the hatches (1.5 million elements) and not for two
    d.dash_limit = 2_000_000;
    let sc = d.scene_checked("Model").expect("not refused: the detail is reduced instead");
    let verts: usize = sc.batches.iter().map(|b| b.lines.len() + b.fills.len()).sum();
    assert!(verts > 1_000_000, "the first hatch has its pattern: {verts}");
    assert!(verts < 4_000_000, "the others are filled solid: {verts}");
    let notes = d.scene_notes("Model");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].starts_with("3 šrafování má příliš hustý vzor") && notes[0].contains("plnou barvou"), "{}", notes[0]);
    // the solid fills are there: triangles that cover the square
    assert!(tri_area(&sc) > 2.9e6, "{}", tri_area(&sc));
}
