//! Regression tests of the second review of the 2D pipeline: time and memory of the scene build, reduced detail instead of
//! refusal, the budget of dimensions, cancellation and time limits, clipping along the edge of a rotated XCLIP.
use super::review::{dict, spatial_filter};
use super::*;
use crate::ctl::Ctl;
use crate::frontend::Frontend;
use crate::geom::V2;
use crate::scene::{Built, MAX_OUT_VERTS, MAX_SCENE_ELEMS};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn text_at(x: f64, y: f64, s: &str) -> String {
    ent!("TEXT"; 8 => "0", 10 => x, 20 => y, 30 => 0.0, 40 => 2.5, 1 => s)
}

/// `n` texts of one short sentence in rows of 30, 40 apart.
fn texts_doc(n: usize) -> Doc {
    let mut d = Doc::new();
    for i in 0..n {
        d.entities.push_str(&text_at((i % 30) as f64 * 40.0, (i / 30) as f64 * 6.0, "Pozice 12 M12x1,5"));
    }
    d
}

fn lines_of(sc: &Scene2d) -> Vec<(V2, V2)> {
    sc.batches
        .iter()
        .flat_map(|b| b.lines.chunks_exact(2).map(|w| ([f64::from(w[0][0]), f64::from(w[0][1])], [f64::from(w[1][0]), f64::from(w[1][1])])))
        .collect()
}

fn fill_vertices(sc: &Scene2d) -> usize {
    sc.batches.iter().map(|b| b.fills.len()).sum()
}

fn line_vertices(sc: &Scene2d) -> usize {
    sc.batches.iter().map(|b| b.lines.len()).sum()
}

// ------------------------------------------------------------------------------------------------ 2: build time and memory
#[test]
fn geometry_is_flattened_in_bounded_slices() {
    // 10 000 texts took 15 s and 1.2 GB because the flattened geometry of every text was kept until all were done (and the
    // heap of Windows gets slower with every block that is kept: the time grew with the square of the number of texts)
    let dr = texts_doc(900).load();
    let mut fe = Frontend::new(dr.document(), false, true, fonts(), vec!["model".to_string()]);
    fe.draw_layout("model", 0);
    let biggest = fe.rec.prims.iter().map(|p| p.ecount as usize).max().unwrap();
    let total: usize = fe.rec.prims.iter().map(|p| p.ecount as usize).sum();
    assert!(total > 300_000, "{total}");
    let rec = std::mem::take(&mut fe.rec);
    let b = Built::build(rec, 1.0, Vec::new(), &[], "Model", vec!["Model".to_string()], [0, 0, 0], OriginMode::World);
    assert!(b.stats.slices > 1, "{:?}", b.stats);
    assert!(b.stats.max_slice_prims <= 2048, "{:?}", b.stats);
    // a slice ends as soon as it reaches 100 000 elements: at most that and one primitive more
    assert!(b.stats.max_slice_elems < 100_000 + biggest, "{:?} (biggest primitive {biggest})", b.stats);
    // nothing was lost: every text has its fill and its outline
    let sc = &b.scene;
    assert_eq!(sc.batches.len(), 1);
    assert!(fill_vertices(sc) > 900 * 1000 && line_vertices(sc) > 900 * 500);
}

// ------------------------------------------------------------------------------------------------ 3: reduced detail
#[test]
fn constants_leave_room_far_above_the_drawings_that_must_open() {
    // 25 000 texts of 33 letters record 32 million path elements, 290 000 dashed lines 31 million
    assert!(MAX_SCENE_ELEMS >= 3 * 32_000_000);
    assert!(scene::DASH_SOFT_ELEMS >= 31_000_000);
    // the detail of texts is full up to 32 million output vertices on a computer with memory to spare (10 000 short texts
    // are 40 million, and the next ones are simplified, not left out); the hard limit is the ceiling of what an 8 GB
    // computer can spare (2.9 GB, not the 300 million vertices and 5 to 7.5 GB it used to be) and lower when less memory
    // is free; 25 000 texts of 33 letters, 59 million vertices simplified, stay under it
    assert!(scene::SOFT_OUT_VERTS >= 32_000_000 && MAX_OUT_VERTS >= 2 * scene::SOFT_OUT_VERTS);
    assert!(MAX_OUT_VERTS >= 64_000_000);
    assert!(MAX_OUT_VERTS * crate::memory::BYTES_PER_OUT_VERT < 3 << 30);
    // the detail is reduced before the scene is refused: the simplified detail is a fifth, so the limit is reached at
    // about five times the vertices that the reduction started at
    let (soft, max) = crate::memory::out_limits_for(None);
    assert!(soft < max && soft >= 32_000_000, "{soft} {max}");
}

#[test]
fn a_text_heavy_drawing_is_drawn_with_simplified_outlines_not_refused() {
    let n = 300;
    let full = texts_doc(n).load();
    let sc_full = full.scene_checked("Model").expect("full detail");
    assert!(full.scene_notes("Model").is_empty());
    // the detail stays full up to 20 000 output vertices: the first texts, then simplified ones
    let mut reduced = texts_doc(n).load();
    reduced.out_limits = (20_000, MAX_OUT_VERTS);
    let sc = reduced.scene_checked("Model").expect("not refused");
    assert!(fill_vertices(&sc) + line_vertices(&sc) < (fill_vertices(&sc_full) + line_vertices(&sc_full)) / 2, "{} {}", fill_vertices(&sc), fill_vertices(&sc_full));
    // every text is still there: a triangle in the box of each of them
    for i in 0..n {
        let (x0, y0) = ((i % 30) as f64 * 40.0, (i / 30) as f64 * 6.0);
        let found = sc.batches.iter().any(|b| {
            b.fills.chunks_exact(3).any(|t| {
                let (cx, cy) = (f64::from(t[0][0] + t[1][0] + t[2][0]) / 3.0, f64::from(t[0][1] + t[1][1] + t[2][1]) / 3.0);
                cx >= x0 - 1.0 && cx <= x0 + 40.0 && cy >= y0 - 1.5 && cy <= y0 + 4.0
            })
        });
        assert!(found, "text {i} has no geometry");
    }
    // the simplified outlines cover about the same area (the shapes are the same, only coarser)
    let ratio = tri_area(&sc) / tri_area(&sc_full);
    assert!((0.8..1.2).contains(&ratio), "{ratio}");
    // the Czech note is among the warnings of the drawing information
    let notes = reduced.scene_notes("Model");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("zjednodušen") && notes[0].contains("Všechny entity zůstávají zobrazeny"), "{}", notes[0]);
    let rows = reduced.info_rows("Model", sc.bounds, None);
    assert!(rows.iter().any(|r| r.label == "Upozornění" && r.warning && r.value == notes[0]), "{rows:?}");
    // a drawing that is not reduced has no such row
    assert!(!full.info_rows("Model", sc_full.bounds, None).iter().any(|r| r.label == "Upozornění"));
}

#[test]
fn too_many_dashes_become_solid_lines_and_every_line_stays() {
    // the DASHED pattern of the test files is 0.75 long: a line of 300 is 400 pieces (800 path elements)
    let mk = || {
        let mut d = Doc::new();
        for i in 0..40 {
            d.entities.push_str(&ent!("LINE"; 8 => "0", 6 => "DASHED", 10 => 0.0, 20 => i as f64, 30 => 0.0, 11 => 300.0, 21 => i as f64, 31 => 0.0));
        }
        d.load()
    };
    // the budget of dashes is 4 000 elements: three lines are dashed (800 elements each), the other 37 solid
    let mut dr = mk();
    dr.dash_limit = 4_000;
    let sc = dr.scene_checked("Model").expect("not refused");
    let segs = lines_of(&sc);
    for i in 0..40 {
        let row: Vec<_> = segs.iter().filter(|(a, _)| (a[1] - i as f64).abs() < 1e-4).collect();
        assert!(!row.is_empty(), "line {i} is missing");
    }
    let pieces = |i: usize| segs.iter().filter(|(a, _)| (a[1] - i as f64).abs() < 1e-4).count();
    assert!(pieces(0) > 300 && pieces(2) > 300, "the first lines are dashed: {} {}", pieces(0), pieces(2));
    assert_eq!(pieces(3), 1, "the next ones are solid");
    assert_eq!(pieces(39), 1, "the last line is solid");
    let notes = dr.scene_notes("Model");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].starts_with("37 čar má příliš hustě čárkování") && notes[0].contains("plnou čarou"), "{}", notes[0]);
    // with the ordinary budget all of them are dashed and nothing is said
    let ok = mk();
    let all = lines_of(&ok.scene_checked("Model").unwrap());
    assert!(all.len() >= 40 * 399, "{}", all.len());
    assert!(ok.scene_notes("Model").is_empty());
}

#[test]
fn the_hard_limit_of_output_vertices_is_a_last_resort_that_refuses_with_a_message() {
    // strokes cannot be reduced: 3 000 lines are 6 000 vertices, more than the limit
    let mut d = Doc::new();
    for i in 0..3_000 {
        d.entities.push_str(&line("0", [i as f64, 0.0], [i as f64, 1.0], None));
    }
    let mut dr = d.load();
    dr.out_limits = (u64::MAX, 2_000);
    let e = dr.scene_checked("Model").unwrap_err();
    assert!(e.0.starts_with("Výkres se při vykreslení rozpadne na více než 2 000 bodů čar a tvarů"), "{}", e.0);
    assert!(dr.scene("Model").batches.is_empty());
    // the limit follows the free memory, so the refusal is not remembered: with room for it the layout is drawn
    assert!(dr.check_budget("Model").is_ok());
    dr.out_limits = (u64::MAX, 1_000_000);
    assert!(dr.scene_checked("Model").is_ok());
}

// ------------------------------------------------------------------------------------------------ 4: dimensions
/// 31 000 references to one block of ten lines: `kind` "DIMENSION" or "INSERT".
fn many_references(kind: &str, n: usize) -> Drawing {
    let body: String = (0..10).map(|i| line("0", [i as f64, 0.0], [i as f64, 1.0], None)).collect();
    let mut d = Doc::new().block("*D1", [0.0, 0.0], &body);
    for i in 0..n {
        let e = match kind {
            "DIMENSION" => ent!("DIMENSION"; 8 => "0", 2 => "*D1", 100 => "AcDbDimension", 70 => 0, 10 => i as f64, 20 => 0.0, 30 => 0.0, 11 => 0.0, 21 => 0.0, 31 => 0.0),
            _ => ent!("INSERT"; 8 => "0", 2 => "*D1", 10 => i as f64, 20 => 0.0, 30 => 0.0),
        };
        d.entities.push_str(&e);
    }
    d.load()
}

#[test]
fn thousands_of_dimensions_are_not_counted_like_block_references() {
    // 31 000 x 10 = 310 000 entities of blocks: over the 300 000 of block references, which is what 1.9 counted (a dimension
    // was one entity there); real drawings have that many dimensions
    let dims = many_references("DIMENSION", 31_000);
    assert_eq!(dims.check_budget("Model").unwrap(), 31_000 + 310_000);
    assert!(many_references("INSERT", 31_000).check_budget("Model").unwrap_err().0.starts_with("Výkres se rozvinutím bloků rozpadne na více než 300 000 entit"));
    // the two limits are separate: 31 000 dimension entities count 31 000 towards the block references, their blocks count
    // towards the content limit
    let doc = dims.document();
    let ents = dims.layout_entities("Model");
    assert!(budget::expanded_entity_count_with(doc, ents, true, 100_000, 10_000_000).is_ok());
    assert!(budget::expanded_entity_count_with(doc, ents, true, 100_000, 100_000).unwrap_err().0.starts_with("Výkres se rozvinutím kót, tabulek a vícenásobných odkazů rozpadne na více než 100 000 entit"));
    // a few of them are drawn
    let few = many_references("DIMENSION", 20);
    assert_eq!(few.scene("Model").batches.iter().map(|b| b.lines.len() / 2).sum::<usize>(), 20 * 10);
}

#[test]
fn crafted_dimension_and_table_bombs_are_still_refused_at_once() {
    // 2^40 copies from 41 blocks, a dimension that contains itself, a table the same
    let reference = |kind: &str, target: &str| match kind {
        "DIMENSION" => ent!("DIMENSION"; 8 => "0", 2 => target, 100 => "AcDbDimension", 70 => 0, 10 => 0.0, 20 => 0.0, 30 => 0.0, 11 => 0.0, 21 => 0.0, 31 => 0.0),
        _ => ent!("ACAD_TABLE"; 8 => "0", 100 => "AcDbBlockReference", 2 => target, 10 => 0.0, 20 => 0.0, 30 => 0.0),
    };
    for kind in ["DIMENSION", "TABLE"] {
        let t0 = Instant::now();
        for levels in [16usize, 20, 40, 90] {
            let mut d = Doc::new();
            for k in 0..levels {
                d = d.block(&format!("*D{k}"), [0.0, 0.0], &(reference(kind, &format!("*D{}", k + 1)) + &reference(kind, &format!("*D{}", k + 1))));
            }
            d = d.block(&format!("*D{levels}"), [0.0, 0.0], &line("0", [0.0, 0.0], [1.0, 1.0], None));
            let dr = d.add(reference(kind, "*D0")).load();
            let e = dr.check_budget("Model").expect_err(&format!("{kind} {levels} levels"));
            assert!(!e.0.is_empty());
            assert!(dr.scene("Model").batches.is_empty());
        }
        let me = reference(kind, "*D0");
        let cyc = Doc::new().block("*D0", [0.0, 0.0], &(line("0", [0.0, 0.0], [1.0, 1.0], None) + &me + &me)).add(me.clone()).load();
        assert!(cyc.check_budget("Model").unwrap_err().0.starts_with("Cyklický blok"));
        assert!(t0.elapsed() < Duration::from_secs(10), "{kind}: {:?}", t0.elapsed());
    }
}

// ------------------------------------------------------------------------------------------------ 5: time limits and cancel
#[test]
fn a_cancelled_scene_build_stops_and_is_not_kept() {
    let mk = |n: usize| {
        let mut d = Doc::new();
        for i in 0..n {
            d.entities.push_str(&ent!("LINE"; 8 => "0", 6 => "DASHED", 10 => 0.0, 20 => i as f64, 30 => 0.0, 11 => 30.0, 21 => i as f64, 31 => 0.0));
        }
        d.load()
    };
    let opts = SceneOptions::default();
    let dr = mk(2_000);
    // a flag that is set already: no work at all
    let flag = Arc::new(AtomicBool::new(true));
    assert_eq!(dr.scene_cancellable("Model", &opts, &flag).unwrap_err(), SceneError::Cancelled);
    // the drawing is still fine, and the scene is the whole one
    let off = Arc::new(AtomicBool::new(false));
    let sc = dr.scene_cancellable("Model", &opts, &off).expect("scene");
    assert!(lines_of(&sc).len() >= 2_000 * 39, "{}", lines_of(&sc).len());
    // set while the build runs: it stops long before it would be done
    let dr = Arc::new(mk(30_000));
    let flag = Arc::new(AtomicBool::new(false));
    let setter = {
        let flag = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            flag.store(true, Ordering::Relaxed);
        })
    };
    let t0 = Instant::now();
    let r = dr.scene_cancellable("Model", &opts, &flag);
    setter.join().unwrap();
    assert_eq!(r.unwrap_err(), SceneError::Cancelled);
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
    // nothing of the stopped build was kept: a new build starts over and is complete
    let again = dr.scene_cancellable("Model", &opts, &Arc::new(AtomicBool::new(false))).expect("complete");
    assert!(lines_of(&again).len() >= 30_000 * 39, "{}", lines_of(&again).len());
}

#[test]
fn a_scene_that_runs_out_of_time_shows_what_it_has_and_says_so() {
    let mut dr = texts_doc(100).load();
    dr.set_scene_time_limit(Duration::ZERO);
    let sc = dr.scene_checked("Model").expect("not refused");
    assert!(sc.batches.is_empty(), "no time at all: nothing drawn");
    let notes = dr.scene_notes("Model");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].starts_with("Vykreslování výkresu překročilo časový limit (0 s)"), "{}", notes[0]);
    assert!(dr.info_rows("Model", None, None).iter().any(|r| r.label == "Upozornění" && r.warning));
    // with the time it needs everything is drawn and nothing is said
    let ok = texts_doc(100).load();
    assert!(!ok.scene_checked("Model").unwrap().batches.is_empty());
    assert!(ok.scene_notes("Model").is_empty());
}

/// A HATCH of a square with `defs` pattern definitions, each one a line every `spacing`.
fn hatch_with_defs(size: f64, spacing: f64, defs: usize) -> String {
    let mut s = ent!("HATCH"; 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0, 210 => 0.0, 220 => 0.0, 230 => 1.0, 2 => "X", 70 => 0, 71 => 0, 91 => 1, 92 => 3, 72 => 0, 73 => 1, 93 => 4,
        10 => 0.0, 20 => 0.0, 10 => size, 20 => 0.0, 10 => size, 20 => size, 10 => 0.0, 20 => size, 97 => 0, 75 => 0, 76 => 1, 52 => 0.0, 41 => 1.0, 77 => 0, 78 => defs);
    for d in 0..defs {
        s.push_str(&t(&[(53, &(d as f64)), (43, &0.0), (44, &0.0), (45, &0.0), (46, &spacing), (79, &0)]));
    }
    s.push_str(&t(&[(98, &0)]));
    s
}

#[test]
fn a_hatch_has_one_time_limit_for_all_its_pattern_lines() {
    let mut doc = Doc::new();
    doc.entities = hatch_with_defs(100.0, 5.0, 4);
    let dr = doc.load();
    let drawn = |timeout: Duration| {
        let mut fe = Frontend::new(dr.document(), false, true, fonts(), vec!["model".to_string()]);
        fe.hatch_timeout = timeout;
        fe.draw_layout("model", 0);
        (fe.hatch_solid, fe.rec.prims.len(), fe.rec.prims.iter().any(|p| matches!(fe.rec.groups[p.gid as usize].kind, scene::GKind::F)))
    };
    // time enough: four sets of pattern lines (strokes), no solid fill
    let (solid, prims, filled) = drawn(Duration::from_secs(30));
    assert_eq!((solid, filled), (0, false));
    assert!(prims >= 1);
    // no time left: the hatch is filled solid, once, however many pattern lines it has (the time was counted per pattern
    // line before: every one of the four started 30 s again)
    let (solid, _, filled) = drawn(Duration::ZERO);
    assert_eq!((solid, filled), (1, true));
}

#[test]
fn two_small_rings_far_apart_do_not_cost_all_lines_times_all_edges() {
    use crate::dxf::PatternLine;
    use crate::hatchdraw::pattern_lines_render;
    // a triangle at the origin and a ring of 20 000 edges 190 away: 190 000 lines, 40 000 edges
    let a: Vec<V2> = vec![[0.0, 0.0], [1.0, 0.0], [0.5, 1e-7]];
    let b: Vec<V2> = (0..20_000).map(|i| [i as f64 * 1e-3, 190.0 + if i % 2 == 1 { 1e-7 } else { 0.0 }]).collect();
    let pl = PatternLine { angle: 0.0, base: [0.0, 0.0], offset: [0.0, 1e-3], dashes: vec![] };
    let t0 = Instant::now();
    let mut out = Vec::new();
    let r = pattern_lines_render(&[a, b], &pl, &mut out, &Ctl::default());
    // with only the lines that reach an edge looked at, this is instant (it took minutes: 190 000 x 20 000)
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
    assert!(r.is_ok());
}

#[test]
fn pattern_lines_stop_in_the_middle_when_the_time_or_the_flag_runs_out() {
    use crate::dxf::PatternLine;
    use crate::hatchdraw::pattern_lines_render;
    // a circle of 300 000 edges and 100 000 lines through it
    let ring: Vec<V2> = (0..300_000).map(|i| {
        let a = i as f64 / 300_000.0 * std::f64::consts::TAU;
        [50.0 * a.cos(), 50.0 * a.sin()]
    }).collect();
    let pl = PatternLine { angle: 0.0, base: [0.0, 0.0], offset: [0.0, 1e-3], dashes: vec![] };
    // time: stops long before the end (the whole job takes seconds in a debug build)
    let t0 = Instant::now();
    let r = pattern_lines_render(std::slice::from_ref(&ring), &pl, &mut Vec::new(), &Ctl::with_limit(Duration::from_millis(5)));
    assert!(r.is_err(), "the time limit stops the pattern");
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
    // the flag: no work at all
    let flag = Arc::new(AtomicBool::new(true));
    let t0 = Instant::now();
    assert!(pattern_lines_render(std::slice::from_ref(&ring), &pl, &mut Vec::new(), &Ctl::new(Some(flag), None)).is_err());
    assert!(t0.elapsed() < Duration::from_millis(500), "{:?}", t0.elapsed());
    // without a limit the same pattern is drawn: one line per index, two crossings each
    let mut out = Vec::new();
    assert!(pattern_lines_render(std::slice::from_ref(&ring), &pl, &mut out, &Ctl::default()).is_ok());
    assert!(out.len() > 90_000 && out.len() < 110_000, "{}", out.len());
}

#[test]
fn pattern_lines_are_the_same_as_the_ones_found_by_testing_every_edge() {
    use crate::dxf::PatternLine;
    use crate::hatchdraw::pattern_lines_render;
    // the lines of the old algorithm: every line against every edge
    let reference = |rings: &[Vec<V2>], pl: &PatternLine| -> Vec<(V2, V2)> {
        let a = pl.angle.to_radians();
        let (d, n) = ([a.cos(), a.sin()], [-a.sin(), a.cos()]);
        let spacing = pl.offset[0] * n[0] + pl.offset[1] * n[1];
        let (mut smin, mut smax) = (f64::INFINITY, f64::NEG_INFINITY);
        for r in rings {
            for p in r {
                let s = (p[0] - pl.base[0]) * n[0] + (p[1] - pl.base[1]) * n[1];
                smin = smin.min(s);
                smax = smax.max(s);
            }
        }
        let (x, y) = (smin / spacing, smax / spacing);
        let (i0, i1) = (x.min(y).floor() as i64, x.max(y).ceil() as i64);
        let mut out = Vec::new();
        for i in i0..=i1 {
            let origin = [pl.base[0] + pl.offset[0] * i as f64, pl.base[1] + pl.offset[1] * i as f64];
            let mut ts: Vec<f64> = Vec::new();
            for r in rings {
                for k in 0..r.len() {
                    let (p, q) = (r[k], r[(k + 1) % r.len()]);
                    let sp = (p[0] - origin[0]) * n[0] + (p[1] - origin[1]) * n[1];
                    let sq = (q[0] - origin[0]) * n[0] + (q[1] - origin[1]) * n[1];
                    if (sp > 0.0) != (sq > 0.0) {
                        let t = sp / (sp - sq);
                        let x = [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
                        ts.push((x[0] - origin[0]) * d[0] + (x[1] - origin[1]) * d[1]);
                    }
                }
            }
            ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
            for pair in ts.chunks(2) {
                if pair.len() == 2 {
                    out.push(([origin[0] + d[0] * pair[0], origin[1] + d[1] * pair[0]], [origin[0] + d[0] * pair[1], origin[1] + d[1] * pair[1]]));
                }
            }
        }
        out
    };
    let rings: Vec<Vec<V2>> = vec![
        vec![[0.0, 0.0], [30.0, 2.0], [28.0, 25.0], [14.0, 12.0], [3.0, 31.0]],
        vec![[8.0, 5.0], [14.0, 6.0], [12.0, 9.0]],
        vec![[40.0, -4.0], [60.0, -3.0], [55.0, 30.0]],
    ];
    for (angle, off) in [(0.0, [0.0, 1.7]), (45.0, [-1.0, 1.3]), (30.0, [0.4, 2.2]), (90.0, [-2.5, 0.0]), (133.0, [0.3, -1.9])] {
        let pl = PatternLine { angle, base: [1.5, -0.5], offset: off, dashes: vec![] };
        let mut got = Vec::new();
        pattern_lines_render(&rings, &pl, &mut got, &Ctl::default()).unwrap();
        let want = reference(&rings, &pl);
        assert_eq!(got.len(), want.len(), "angle {angle}");
        // the same segments (the order of the lines is by index here, as before)
        let key = |s: &(V2, V2)| ((s.0[0] * 1e6).round() as i64, (s.0[1] * 1e6).round() as i64, (s.1[0] * 1e6).round() as i64, (s.1[1] * 1e6).round() as i64);
        let (mut g, mut w) = (got.clone(), want.clone());
        g.sort_by_key(key);
        w.sort_by_key(key);
        for (a, b) in g.iter().zip(&w) {
            assert!((a.0[0] - b.0[0]).abs() < 1e-9 && (a.0[1] - b.0[1]).abs() < 1e-9 && (a.1[0] - b.1[0]).abs() < 1e-9 && (a.1[1] - b.1[1]).abs() < 1e-9, "angle {angle}: {a:?} {b:?}");
        }
    }
}

#[test]
fn a_viewport_clipped_by_a_huge_circle_is_built_in_time() {
    // 3000 lines in the model space, shown through a viewport whose clipping path is a circle of radius 1e9: its ring has
    // 220 000 edges. Every line was tested against all of them (and the ring was copied for each line): 12 s for one
    // viewport at radius 1e11
    let mut d = Doc::new();
    for i in 0..3000 {
        d.entities.push_str(&line("0", [i as f64 * 0.001, 0.0], [i as f64 * 0.001, 1.0], None));
    }
    d.entities.push_str(&ent!("CIRCLE"; 5 => "A00", 67 => 1, 8 => "0", 10 => 1e9, 20 => 0.0, 40 => 1e9));
    d.entities.push_str(&ent!("VIEWPORT"; 67 => 1, 8 => "0", 10 => 5.0, 20 => 5.0, 40 => 10.0, 41 => 10.0, 68 => 2, 69 => 2, 12 => 5.0, 22 => 5.0, 45 => 10.0, 90 => 65536, 340 => "A00"));
    let dr = d.load();
    assert_eq!(dr.layouts().iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Model", "Layout1"]);
    let t0 = Instant::now();
    let sc = dr.scene_checked("Layout1").expect("scene");
    // the lines at x >= 0.001 are inside the circle (x = 0 is its tangent): all but the first
    let n = lines_of(&sc).len();
    assert!(n >= 2990, "{n}");
    assert!(t0.elapsed() < Duration::from_secs(20), "{:?}", t0.elapsed());
}

#[test]
fn cancelling_stops_between_viewports_with_huge_clipping_rings() {
    // 40 viewports, each clipped by a circle of 220 000 edges that has to be flattened before anything is drawn through it:
    // the flag is looked at before every one of them, not only between entities
    let mut d = Doc::new();
    for i in 0..300 {
        d.entities.push_str(&line("0", [i as f64 * 0.01, 0.0], [i as f64 * 0.01, 1.0], None));
    }
    for k in 0..40 {
        let handle = format!("A{k:02X}");
        d.entities.push_str(&ent!("CIRCLE"; 5 => handle, 67 => 1, 8 => "0", 10 => 1e9, 20 => 0.0, 40 => 1e9 * (1.0 + 1e-12 * k as f64)));
        d.entities.push_str(&ent!("VIEWPORT"; 67 => 1, 8 => "0", 10 => 5.0, 20 => 5.0, 40 => 10.0, 41 => 10.0, 68 => 2 + k, 69 => 2 + k, 12 => 5.0, 22 => 5.0, 45 => 10.0, 90 => 65536, 340 => handle));
    }
    let dr = Arc::new(d.load());
    let flag = Arc::new(AtomicBool::new(false));
    let setter = {
        let flag = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::Relaxed);
        })
    };
    let t0 = Instant::now();
    let r = dr.scene_cancellable("Layout1", &SceneOptions::default(), &flag);
    setter.join().unwrap();
    assert_eq!(r.unwrap_err(), SceneError::Cancelled);
    // one viewport takes about a second in a debug build, forty of them the better part of a minute
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
}

// ------------------------------------------------------------------------------------------------ 6: XCLIP of rotated inserts
/// The grid of block "B": lines every 10 over 0..100 in both directions (red horizontal, green vertical), inserted at (5, 7)
/// turned by `deg`, clipped by the square 20..60 of the block.
fn rotated_xclip(deg: f64) -> Drawing {
    let mut body = String::new();
    for i in 0..=10 {
        body.push_str(&line("0", [0.0, i as f64 * 10.0], [100.0, i as f64 * 10.0], Some(1)));
        body.push_str(&line("0", [i as f64 * 10.0, 0.0], [i as f64 * 10.0, 100.0], Some(3)));
    }
    let mut d = Doc::new().block("B", [0.0, 0.0], &body).add(ent!("INSERT"; 5 => "A0", 8 => "0", 2 => "B", 10 => 5.0, 20 => 7.0, 30 => 0.0, 50 => deg, 102 => "{ACAD_XDICTIONARY", 360 => "D1", 102 => "}"));
    d.header.push_str(&t(&[(9, &"$XCLIPFRAME"), (280, &0)]));
    d.objects = dict("D1", &[("ACAD_FILTER", "D2")]) + &dict("D2", &[("SPATIAL", "F1")]) + &spatial_filter("F1", &[[20.0, 20.0], [60.0, 20.0], [60.0, 60.0], [20.0, 60.0]], true, None);
    d.load()
}

fn length_of_color(sc: &Scene2d, color: [u8; 4]) -> f64 {
    sc.batches
        .iter()
        .filter(|b| b.color == color)
        .map(|b| b.lines.chunks_exact(2).map(|w| ((f64::from(w[1][0]) - f64::from(w[0][0])).powi(2) + (f64::from(w[1][1]) - f64::from(w[0][1])).powi(2)).sqrt()).sum::<f64>())
        .sum()
}

#[test]
fn xclip_of_a_rotated_insert_keeps_only_the_overlap_of_lines_along_the_clip_edges() {
    // ezdxf: five red and five green lines of 40 inside the square, 200 each, at every angle. The lines on the edges of the
    // square (y = 20, 60, x = 20, 60) count where they overlap the edge (it was 220 and 160 at 30 degrees)
    for deg in [0.0, 15.0, 30.0, 33.3, 45.0, 90.0] {
        let sc = rotated_xclip(deg).scene("Model");
        let red = length_of_color(&sc, [255, 0, 0, 255]);
        let green = length_of_color(&sc, [0, 255, 0, 255]);
        assert!((red - 200.0).abs() < 1e-3, "{deg} degrees: red {red}");
        assert!((green - 200.0).abs() < 1e-3, "{deg} degrees: green {green}");
    }
}
