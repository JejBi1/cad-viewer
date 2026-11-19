//! Regression tests of the review findings: crafted small files that used to run for minutes, exhaust memory or crash,
//! and the parity items (layer 0 without a LAYER record, XCLIP, MPOLYGON, layer transparency).
use super::*;
use std::time::{Duration, Instant};

fn raw(entities: &str) -> Drawing {
    let text = format!("  0\nSECTION\n  2\nENTITIES\n{}  0\nENDSEC\n  0\nEOF\n", entities);
    load_bytes(text.as_bytes(), fonts()).expect("test DXF loads")
}

fn line_pts(sc: &Scene2d) -> Vec<[f32; 2]> {
    sc.batches.iter().flat_map(|b| b.lines.iter().copied()).collect()
}

pub(super) fn dict(handle: &str, entries: &[(&str, &str)]) -> String {
    let mut s = format!("  0\nDICTIONARY\n{}{}", t(&[(5, &handle)]), t(&[(100, &"AcDbDictionary")]));
    for (name, target) in entries {
        s += &t(&[(3, name), (360, target)]);
    }
    s
}

/// SPATIAL_FILTER with identity matrices; `boundary` are the points of the clipping path in the coordinates of the filter.
pub(super) fn spatial_filter(handle: &str, boundary: &[[f64; 2]], enabled: bool, xdict: Option<&str>) -> String {
    let mut s = format!("  0\nSPATIAL_FILTER\n{}", t(&[(5, &handle)]));
    if let Some(x) = xdict {
        s += &t(&[(102, &"{ACAD_XDICTIONARY"), (360, &x), (102, &"}")]);
    }
    s += &t(&[(100, &"AcDbFilter"), (100, &"AcDbSpatialFilter"), (70, &boundary.len())]);
    for p in boundary {
        s += &t(&[(10, &p[0]), (20, &p[1])]);
    }
    s += &t(&[(210, &0.0), (220, &0.0), (230, &1.0), (11, &0.0), (21, &0.0), (31, &0.0), (71, &i32::from(enabled)), (72, &0), (73, &0)]);
    let identity = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    for _ in 0..2 {
        for v in identity {
            s += &t(&[(40, &v)]);
        }
    }
    s
}

fn xclip_insert(x: f64, scale: f64) -> String {
    ent!("INSERT"; 5 => "A0", 8 => "0", 2 => "B", 10 => x, 20 => 0.0, 30 => 0.0, 41 => scale, 42 => scale, 43 => scale, 102 => "{ACAD_XDICTIONARY", 360 => "D1", 102 => "}")
}

/// A block "B" with one long horizontal line, an INSERT of it with a clipping path and the objects that hold the path.
fn xclip_doc(boundary: &[[f64; 2]], enabled: bool, frame: Option<i32>, x: f64, scale: f64) -> Drawing {
    let mut d = Doc::new().block("B", [0.0, 0.0], &line("0", [-100.0, 0.0], [100.0, 0.0], None)).add(xclip_insert(x, scale));
    if let Some(f) = frame {
        d.header.push_str(&t(&[(9, &"$XCLIPFRAME"), (280, &f)]));
    }
    d.objects = dict("D1", &[("ACAD_FILTER", "D2")]) + &dict("D2", &[("SPATIAL", "F1")]) + &spatial_filter("F1", boundary, enabled, None);
    d.load()
}

// ------------------------------------------------------------------------------------------------ finding 2: crafted files
#[test]
fn viewports_are_drawn_only_at_the_top_level_of_a_paper_layout() {
    // VIEWPORT entities in the model space (no group 67) reached themselves through the model space they show
    let mut d = Doc::new();
    d.entities = format!(
        "{}{}{}",
        line("0", [0.0, 0.0], [100.0, 100.0], None),
        ent!("VIEWPORT"; 5 => "C1", 8 => "0", 10 => 5.0, 20 => 5.0, 30 => 0.0, 40 => 10.0, 41 => 10.0, 68 => 2, 69 => 2, 12 => 5.0, 22 => 5.0, 16 => 0.0, 26 => 0.0, 36 => 1.0, 45 => 10.0),
        ent!("VIEWPORT"; 5 => "C0", 8 => "0", 67 => 1, 10 => 150.0, 20 => 150.0, 30 => 0.0, 40 => 100.0, 41 => 100.0, 68 => 2, 69 => 3, 12 => 50.0, 22 => 50.0, 16 => 0.0, 26 => 0.0, 36 => 1.0, 45 => 200.0)
    );
    d.objects = format!(
        "{}{}",
        ent!("LAYOUT"; 5 => "50", 100 => "AcDbLayout", 1 => "Model", 70 => 1, 71 => 0, 330 => "1F"),
        ent!("LAYOUT"; 5 => "51", 100 => "AcDbLayout", 1 => "Sheet A", 70 => 1, 71 => 1, 330 => "1E")
    );
    let drawing = d.load();
    // the model space shows its line only: the viewport entity inside it draws nothing
    let model = drawing.scene("Model");
    bounds_close(model.bounds, [0.0, 0.0, 100.0, 100.0], 1e-6);
    // the sheet shows the model space through its viewport (the one in the model space is not drawn again)
    let sheet = drawing.scene("Sheet A");
    bounds_close(sheet.bounds, [125.0, 125.0, 175.0, 175.0], 1e-6);
    assert!(!sheet.batches.is_empty());
}

#[test]
fn millions_of_binary_comments_load_with_a_small_stack() {
    // 400 000 comments before the entities (a recursive reader needed a stack frame for each)
    let mut b = dxf::tags::BINARY_SENTINEL.to_vec();
    for _ in 0..400_000 {
        b.extend_from_slice(&999i16.to_le_bytes());
        b.push(0);
    }
    let s = |b: &mut Vec<u8>, c: i16, v: &str| {
        b.extend_from_slice(&c.to_le_bytes());
        b.extend_from_slice(v.as_bytes());
        b.push(0);
    };
    s(&mut b, 0, "SECTION");
    s(&mut b, 2, "ENTITIES");
    s(&mut b, 0, "LINE");
    s(&mut b, 8, "0");
    for (c, v) in [(10i16, 0.0f64), (20, 0.0), (30, 0.0), (11, 4.0), (21, 3.0), (31, 0.0)] {
        b.extend_from_slice(&c.to_le_bytes());
        b.extend_from_slice(&v.to_le_bytes());
    }
    s(&mut b, 0, "ENDSEC");
    s(&mut b, 0, "EOF");
    let d = std::thread::Builder::new().stack_size(512 * 1024).spawn(move || load_bytes(&b, fonts()).map(|d| total_len(&d.scene("Model")))).unwrap().join().expect("no stack overflow");
    assert!((d.unwrap() - 5.0).abs() < 1e-5);
}

#[test]
fn mtext_column_counts_are_clamped() {
    // 2 000 000 000 columns asked for 30 GB of column heights
    for count in [2_000_000_000i64, 50_000_000, 100_000] {
        let d = raw(&ent!("MTEXT"; 8 => "0", 10 => 0.0, 20 => 0.0, 40 => 2.5, 41 => 100.0, 71 => 1, 1 => "hello world", 75 => 1, 76 => count, 79 => 0, 48 => 50.0, 49 => 5.0));
        let t0 = Instant::now();
        let sc = d.scene("Model");
        assert!(!sc.batches.is_empty(), "the text is drawn");
        assert!(t0.elapsed() < Duration::from_secs(20), "{:?}", t0.elapsed());
    }
    // right aligned text with many columns (the width of all columns is part of the position)
    let d = raw(&ent!("MTEXT"; 8 => "0", 10 => 0.0, 20 => 0.0, 40 => 2.5, 41 => 100.0, 71 => 3, 1 => "a\\Nb\\Nc", 75 => 2, 76 => 2_000_000_000i64, 79 => 0, 48 => 50.0, 49 => 5.0));
    assert!(!d.scene("Model").batches.is_empty());
    let opt = mtext::LayoutOptions {
        width: 10.0,
        attachment: 1,
        line_spacing: 1.0,
        base_ctx: mtext::Ctx::new(2.5, 1.0, 0.0, 256),
        columns: Some((usize::MAX / 4, 10.0, 1.0, vec![None; 3])),
    };
    let font = crate::font::default_font(&fonts());
    let laid = mtext::layout(&mtext::parse_mtext("a\\Nb\\Nc", opt.base_ctx.clone()), &opt, &|_| font.clone());
    assert!(!laid.cmds.is_empty());
}

#[test]
fn minsert_of_an_empty_block_counts_every_instance() {
    let minsert = |cols: i64, rows: i64, sp: f64| {
        Doc::new()
            .block("E", [0.0, 0.0], "")
            .add(line("0", [0.0, 0.0], [10.0, 10.0], None))
            .add(ent!("INSERT"; 8 => "0", 2 => "E", 10 => 0.0, 20 => 0.0, 30 => 0.0, 70 => cols, 71 => rows, 44 => sp, 45 => sp))
            .load()
    };
    // 2 000 000 000 x 2 000 000 000 instances of nothing is not "0 entities"
    let t0 = Instant::now();
    let d = minsert(2_000_000_000, 2_000_000_000, 0.0);
    // a zero spacing puts all instances on one spot, ezdxf draws that spot once
    assert_eq!(d.check_budget("Model").unwrap(), 2);
    assert!((total_len(&d.scene("Model")) - 200.0_f64.sqrt()).abs() < 1e-4);
    let d = minsert(2_000_000_000, 2_000_000_000, 1.0);
    let e = d.check_budget("Model").unwrap_err();
    assert!(e.0.starts_with("Výkres se rozvinutím bloků rozpadne na více než 300 000 entit"), "{}", e.0);
    assert!(d.scene("Model").batches.is_empty() && d.scene_checked("Model").is_err());
    // 30 000 x 30 000 with spacing 1: 900 000 000 instances
    assert!(minsert(30_000, 30_000, 1.0).check_budget("Model").is_err());
    // a modest array of an empty block is fine and costs its instances
    assert_eq!(minsert(10, 10, 1.0).check_budget("Model").unwrap(), 2 + 100);
    assert!(t0.elapsed() < Duration::from_secs(20), "{:?}", t0.elapsed());
}

#[test]
fn the_frontend_alone_does_not_loop_over_a_huge_array() {
    // without the entity budget in front of it (a caller that builds a Frontend itself)
    let d = Doc::new()
        .block("E", [0.0, 0.0], "")
        .add(ent!("INSERT"; 8 => "0", 2 => "E", 10 => 0.0, 20 => 0.0, 30 => 0.0, 70 => 2_000_000_000i64, 71 => 2_000_000_000i64, 44 => 1.0, 45 => 1.0))
        .load();
    let mut fe = frontend::Frontend::new(d.document(), false, true, fonts(), vec!["model".to_string(), "layout1".to_string()]);
    let t0 = Instant::now();
    fe.draw_layout("model", 0);
    assert!(fe.rec.is_over());
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
}

#[test]
fn insert_grid_has_one_position_per_location() {
    let mk = |cols, rows, cs, rs| dxf::InsertData {
        name: "A".into(),
        insert: [0.0; 3],
        scale: [1.0; 3],
        rotation: 0.0,
        col_count: cols,
        row_count: rows,
        col_spacing: cs,
        row_spacing: rs,
        attribs: Vec::new(),
        table: false,
        from_inventor: false,
        clip: None,
        xdict: 0,
    };
    assert_eq!(mk(1, 1, 5.0, 5.0).grid(), (1, 1));
    assert_eq!(mk(4, 3, 2.0, 2.0).grid(), (4, 3));
    assert_eq!(mk(4, 3, 0.0, 2.0).grid(), (1, 3));
    assert_eq!(mk(4, 3, 2.0, 0.0).grid(), (4, 1));
    assert_eq!(mk(4, 3, 0.0, 0.0).instances(), 1);
    assert_eq!(mk(i32::MAX, i32::MAX, 1.0, 1.0).instances(), (i32::MAX as usize) * (i32::MAX as usize));
}

#[test]
fn large_minsert_arrays_are_not_quadratic() {
    // 400 x 400 = 160 000 instances of a one line block, in a drawing of 2000 lines (a drawing that is nothing but one
    // array is refused as an amplification bomb): a list of the places drawn so far was searched for each of them
    let mut d = Doc::new().block("B", [0.0, 0.0], &line("0", [0.0, 0.0], [1.0, 1.0], None));
    for i in 0..2000 {
        d = d.add(line("0", [i as f64, -10.0], [i as f64, -5.0], None));
    }
    let d = d.add(ent!("INSERT"; 8 => "0", 2 => "B", 10 => 0.0, 20 => 0.0, 30 => 0.0, 70 => 400, 71 => 400, 44 => 2.0, 45 => 2.0)).load();
    let t0 = Instant::now();
    let sc = d.scene("Model");
    assert_eq!(sc.batches.iter().map(|b| b.lines.len() / 2).sum::<usize>(), 162_000);
    assert!(t0.elapsed() < Duration::from_secs(60), "{:?}", t0.elapsed());
}

/// `n` levels of blocks with two references to the next level in each, the last one holds a line.
fn bomb(kind: &str, levels: usize) -> Drawing {
    let reference = |target: &str| match kind {
        "DIMENSION" => ent!("DIMENSION"; 8 => "0", 2 => target, 100 => "AcDbDimension", 70 => 0, 10 => 0.0, 20 => 0.0, 30 => 0.0, 11 => 0.0, 21 => 0.0, 31 => 0.0),
        "TABLE" => ent!("ACAD_TABLE"; 8 => "0", 100 => "AcDbBlockReference", 2 => target, 10 => 0.0, 20 => 0.0, 30 => 0.0),
        _ => unreachable!(),
    };
    let mut d = Doc::new();
    for k in 0..levels {
        d = d.block(&format!("*D{k}"), [0.0, 0.0], &(reference(&format!("*D{}", k + 1)) + &reference(&format!("*D{}", k + 1))));
    }
    d = d.block(&format!("*D{levels}"), [0.0, 0.0], &line("0", [0.0, 0.0], [1.0, 1.0], None));
    d.add(reference("*D0")).load()
}

#[test]
fn dimension_and_table_blocks_count_towards_the_budget() {
    for kind in ["DIMENSION", "TABLE"] {
        let t0 = Instant::now();
        // 2^20 line copies from 21 blocks
        let d = bomb(kind, 20);
        assert!(d.check_budget("Model").is_err(), "{kind}");
        assert!(d.scene_checked("Model").is_err(), "{kind}");
        assert!(d.scene("Model").batches.is_empty(), "{kind}");
        // 2^40
        assert!(bomb(kind, 40).check_budget("Model").is_err(), "{kind}");
        // a shallow one is drawn: 2^3 lines
        let d = bomb(kind, 3);
        assert!(d.check_budget("Model").is_ok(), "{kind}");
        assert_eq!(d.scene("Model").batches.iter().map(|b| b.lines.len() / 2).sum::<usize>(), 8, "{kind}");
        assert!(t0.elapsed() < Duration::from_secs(30), "{kind}: {:?}", t0.elapsed());
    }
}

#[test]
fn a_dimension_block_that_contains_its_own_dimension_is_a_cycle() {
    let me = ent!("DIMENSION"; 8 => "0", 2 => "*D0", 100 => "AcDbDimension", 70 => 0, 10 => 0.0, 20 => 0.0, 30 => 0.0, 11 => 0.0, 21 => 0.0, 31 => 0.0);
    let d = Doc::new().block("*D0", [0.0, 0.0], &(line("0", [0.0, 0.0], [1.0, 1.0], None) + &me + &me)).add(me.clone()).load();
    let t0 = Instant::now();
    let e = d.check_budget("Model").unwrap_err();
    assert!(e.0.starts_with("Cyklický blok"), "{}", e.0);
    assert!(d.scene("Model").batches.is_empty());
    assert!(t0.elapsed() < Duration::from_secs(10));
}

#[test]
fn mleader_blocks_count_towards_the_budget_and_cycles_are_found() {
    // block "M" (record 4D) holds 1000 lines and the MLEADER shows it 400 times
    let mleader = |handle: &str| ent!("MLEADER"; 8 => "0", 300 => "CONTEXT_DATA{", 40 => 1.0, 296 => 1, 341 => handle, 15 => 0.0, 25 => 0.0, 35 => 0.0, 301 => "}", 170 => 0, 172 => 1);
    let block_with_record = |name: &str, record: &str, body: &str| format!("{}{}{}", ent!("BLOCK"; 2 => name, 70 => 0, 10 => 0.0, 20 => 0.0, 30 => 0.0, 330 => record), body, "  0\nENDBLK\n");
    let mut d = Doc::new();
    let lines: String = (0..1000).map(|i| line("0", [i as f64, 0.0], [i as f64, 1.0], None)).collect();
    d.blocks = block_with_record("M", "4D", &lines);
    for _ in 0..400 {
        d.entities.push_str(&mleader("4D"));
    }
    let d = d.load();
    // the content of leaders no longer counts towards the 300 000 of block references: 400 000 entities are far below the
    // limit of dimensions, tables and leaders, but 1 400 records in the file that draw 400 400 entities are an amplification
    let e = d.check_budget("Model").unwrap_err();
    assert!(e.0.starts_with("Výkres má v souboru jen 1400 entit, ale rozvinutím vnořených bloků by jich vykreslil 400400"), "{}", e.0);
    // with its own limit the content of leaders is refused as such
    let e = budget::expanded_entity_count_with(d.document(), d.layout_entities("Model"), true, budget::MAX_EXPANDED_ENTITIES, 100_000).unwrap_err();
    assert!(e.0.starts_with("Výkres se rozvinutím kót, tabulek a vícenásobných odkazů rozpadne na více než 100 000 entit"), "{}", e.0);
    // a block whose content refers to the block itself
    let mut d = Doc::new();
    d.blocks = block_with_record("S", "4E", &(line("0", [0.0, 0.0], [1.0, 1.0], None) + &mleader("4E")));
    d.entities = mleader("4E");
    let cyc = d.load();
    let e = cyc.check_budget("Model").unwrap_err();
    assert!(e.0.starts_with("Cyklický blok"), "{}", e.0);
    // one reference of a small block is drawn
    let mut d = Doc::new();
    d.blocks = block_with_record("S", "4E", &line("0", [0.0, 0.0], [1.0, 1.0], None));
    d.entities = mleader("4E");
    let ok = d.load();
    assert_eq!(ok.check_budget("Model").unwrap(), 2);
    assert!(!ok.scene("Model").batches.is_empty());
}

#[test]
fn hatch_pattern_counts_are_taken_in_f64() {
    use crate::dxf::PatternLine;
    use crate::hatchdraw::pattern_lines_render;
    let square = |s: f64| vec![vec![[-s, -s], [s, -s], [s, s], [-s, s]]];
    // 1e15 box, 1e-4 line distance: 2e19 lines, more than an i64 holds (the index difference wrapped past the limit)
    let pl = PatternLine { angle: 0.0, base: [0.0, 0.0], offset: [0.0, 1e-4], dashes: vec![] };
    let t0 = Instant::now();
    assert!(pattern_lines_render(&square(1e15), &pl, &mut Vec::new(), &crate::ctl::Ctl::default()).is_err());
    let pl = PatternLine { angle: 37.0, base: [3.0, 4.0], offset: [-1e-3, 2e-3], dashes: vec![] };
    assert!(pattern_lines_render(&square(1e300), &pl, &mut Vec::new(), &crate::ctl::Ctl::default()).is_err());
    // tiny dashes: 5 000 000 dashes on each of 4 lines of a 1000 square
    let mut out = Vec::new();
    let pl = PatternLine { angle: 0.0, base: [0.0, 0.0], offset: [0.0, 250.0], dashes: vec![0.0002, -0.0002] };
    assert!(pattern_lines_render(&square(500.0), &pl, &mut out, &crate::ctl::Ctl::default()).is_err());
    assert!(out.len() < 10, "nothing is generated before the count is known: {}", out.len());
    // the same pattern with sensible dashes works
    let pl = PatternLine { angle: 0.0, base: [0.0, 0.0], offset: [0.0, 250.0], dashes: vec![2.0, -2.0] };
    let mut out = Vec::new();
    assert!(pattern_lines_render(&square(500.0), &pl, &mut out, &crate::ctl::Ctl::default()).is_ok());
    assert!(out.len() > 200 && out.len() < 2000, "{}", out.len());
    assert!(t0.elapsed() < Duration::from_secs(10));
}

fn hatch_entity(size: f64, spacing: f64, dash: f64) -> String {
    ent!("HATCH"; 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0, 210 => 0.0, 220 => 0.0, 230 => 1.0, 2 => "X", 70 => 0, 71 => 0, 91 => 1, 92 => 3, 72 => 0, 73 => 1, 93 => 4,
        10 => 0.0, 20 => 0.0, 10 => size, 20 => 0.0, 10 => size, 20 => size, 10 => 0.0, 20 => size, 97 => 0, 75 => 0, 76 => 1, 52 => 0.0, 41 => 1.0, 77 => 0, 78 => 1,
        53 => 0.0, 43 => 0.0, 44 => 0.0, 45 => 0.0, 46 => spacing, 79 => 2, 49 => dash, 49 => -dash, 98 => 0)
}

#[test]
fn hatches_with_hopeless_patterns_are_filled_solid() {
    let t0 = Instant::now();
    // dashes of 0.0002 on a 1000 square, and a 1e15 square at distance 1e-4: filled instead of drawn line by line
    for (size, spacing, dash) in [(1000.0, 250.0, 0.0002), (1e15, 1e-4, 2.0), (1000.0, 1.0, 0.0002)] {
        let sc = raw(&hatch_entity(size, spacing, dash)).scene("Model");
        assert!(tri_area(&sc) > 0.99 * size * size, "solid fill of {size}: {}", tri_area(&sc));
        assert!(sc.batches.iter().map(|b| b.lines.len()).sum::<usize>() < 1000);
    }
    // a pattern that is fine stays a pattern: lines, no fill
    let sc = raw(&hatch_entity(100.0, 5.0, 2.0)).scene("Model");
    assert!(sc.batches.iter().map(|b| b.lines.len()).sum::<usize>() > 100);
    assert!(tri_area(&sc) < 1.0);
    assert!(t0.elapsed() < Duration::from_secs(30), "{:?}", t0.elapsed());
}

#[test]
fn lines_too_finely_dashed_are_drawn_solid() {
    let mut d = Doc::new();
    d.linetypes = ent!("LTYPE"; 2 => "TINY", 70 => 0, 3 => "x", 72 => 65, 73 => 2, 40 => 0.0002, 49 => 0.0001, 74 => 0, 49 => -0.0001, 74 => 0);
    for i in 0..3 {
        d.entities.push_str(&ent!("LINE"; 8 => "0", 6 => "TINY", 10 => 0.0, 20 => i as f64, 30 => 0.0, 11 => 1.0e7, 21 => i as f64, 31 => 0.0));
    }
    let t0 = Instant::now();
    let sc = d.load().scene("Model");
    // a dash pattern of 0.2 on 1e7 units is 5e7 dashes: one solid line each
    let n: usize = sc.batches.iter().map(|b| b.lines.len() / 2).sum();
    assert_eq!(n, 3);
    assert!((total_len(&sc) - 3.0e7).abs() < 100.0, "{}", total_len(&sc));
    assert!(t0.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_scene_over_the_limit_is_refused_with_a_message() {
    // 400 lines that are each dashed into 267 pieces: 213 600 path elements
    let dashed = || {
        let mut d = Doc::new();
        for i in 0..400 {
            d.entities.push_str(&ent!("LINE"; 8 => "0", 6 => "DASHED", 10 => 0.0, 20 => i as f64, 30 => 0.0, 11 => 100.0, 21 => i as f64, 31 => 0.0));
        }
        d.load()
    };
    // the ordinary limit is far above that
    assert!(dashed().scene_checked("Model").is_ok());
    let mut drawing = dashed();
    drawing.scene_limits = (50_000, 1_000_000);
    let e = drawing.scene_checked("Model").unwrap_err();
    assert_eq!(
        e.0,
        "Výkres se při vykreslení rozpadne na více než 50 000 bodů čar a tvarů – to je příliš (pravděpodobně velmi jemné přerušované čáry, šrafování nebo obrovský text). Výkres se nevykreslí."
    );
    assert!(drawing.scene("Model").batches.is_empty());
    // the refusal is not remembered: the limit follows the memory that is free (and is the caller's), so the next scene is
    // worked out again, and with room for it the same layout is drawn
    assert!(drawing.check_budget("Model").is_ok());
    drawing.scene_limits = (1_000_000, 1_000_000);
    assert!(drawing.scene_checked("Model").is_ok(), "a later build with room for it draws the layout");
    // a limit on the number of primitives works as well
    let mut d = Doc::new();
    for i in 0..100 {
        d.entities.push_str(&line("0", [0.0, i as f64], [1.0, i as f64], None));
    }
    let mut dr = d.load();
    dr.scene_limits = (usize::MAX, 50);
    assert!(dr.scene_checked("Model").is_err());
}

#[test]
fn the_recorder_stops_at_its_limit() {
    use crate::scene::{GKind, Recorder};
    let mut rec = Recorder::new();
    rec.set_limits(10, 100);
    let g = rec.group(GKind::S, &[0], [255, 255, 255, 255], 0.0);
    for i in 0..4 {
        rec.line(g, [0.0, i as f64], [1.0, i as f64 + 0.5]);
    }
    assert!(!rec.is_over());
    assert_eq!(rec.elems_left(), 2);
    rec.line(g, [0.0, 9.0], [1.0, 9.5]);
    rec.line(g, [0.0, 10.0], [1.0, 10.5]);
    assert!(rec.is_over(), "the sixth line does not fit");
    assert_eq!(rec.prims.len(), 5);
    rec.line(g, [0.0, 11.0], [1.0, 11.5]);
    assert_eq!(rec.prims.len(), 5, "nothing is recorded any more");
}

// ------------------------------------------------------------------------------------------------ finding 3: layer 0
#[test]
fn layers_zero_and_defpoints_exist_without_layer_records() {
    // no LAYER table at all (ezdxf creates both layers with colour 7)
    let ents = ent!("LINE"; 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0, 11 => 10.0, 21 => 10.0, 31 => 0.0)
        + &ent!("LINE"; 8 => "Defpoints", 10 => 0.0, 20 => 1.0, 30 => 0.0, 11 => 10.0, 21 => 11.0, 31 => 0.0)
        + &ent!("LINE"; 8 => "X", 10 => 0.0, 20 => 2.0, 30 => 0.0, 11 => 10.0, 21 => 12.0, 31 => 0.0);
    let d = raw(&ents);
    let light = d.scene_with("Model", &SceneOptions { light_bg: true, ..Default::default() }).unwrap();
    let colours: Vec<[u8; 4]> = light.batches.iter().map(|b| b.color).collect();
    // black on white paper for the layers 0 and Defpoints, the colour of an undefined layer (white) for X
    assert_eq!(colours.iter().filter(|c| **c == [0, 0, 0, 255]).count(), 2, "layers 0 and Defpoints: {colours:?}");
    assert!(colours.contains(&[255, 255, 255, 255]), "{colours:?}");
    let by_layer = |name: &str| light.batches.iter().find(|b| light.layers.iter().any(|l| l.id == b.layer && l.name == name)).map(|b| b.color);
    assert_eq!(by_layer("0"), Some([0, 0, 0, 255]));
    assert_eq!(by_layer("Defpoints"), Some([0, 0, 0, 255]));
    assert_eq!(by_layer("X"), Some([255, 255, 255, 255]));
    // on the dark model space background the foreground colour is white
    let dark = d.scene_with("Model", &SceneOptions::default()).unwrap();
    assert!(dark.batches.iter().all(|b| b.color == [255, 255, 255, 255]));
    // a file that has the layer record keeps behaving the same
    let with = Doc::new().add(line("0", [0.0, 0.0], [10.0, 10.0], None)).load().scene_with("Model", &SceneOptions { light_bg: true, ..Default::default() }).unwrap();
    assert_eq!(with.batches[0].color, [0, 0, 0, 255]);
}

// ------------------------------------------------------------------------------------------------ finding 4: parity with 1.9
#[test]
fn xclip_clips_the_block_with_a_rectangle_and_draws_the_frame() {
    // clipping path of two corner points, identity matrices: the block line (-100..100) is cut to -10..10
    let d = xclip_doc(&[[-10.0, -10.0], [10.0, 10.0]], true, None, 0.0, 1.0);
    let sc = d.scene("Model");
    // the default $XCLIPFRAME is 2: the frame of the path is drawn as well
    bounds_close(sc.bounds, [-10.0, -10.0, 10.0, 10.0], 1e-4);
    assert_eq!(d.layout_entities("Model").len(), 1);
    // no frame: the clipped line only
    let d = xclip_doc(&[[-10.0, -10.0], [10.0, 10.0]], true, Some(0), 0.0, 1.0);
    let sc = d.scene("Model");
    bounds_close(sc.bounds, [-10.0, 0.0, 10.0, 0.0], 1e-4);
    assert!((total_len(&sc) - 20.0).abs() < 1e-4);
    // the clipping path follows the block reference: scale 2 at x = 100 shows the line from x = 80 to 120
    let d = xclip_doc(&[[-10.0, -10.0], [10.0, 10.0]], true, Some(0), 100.0, 2.0);
    bounds_close(d.scene("Model").bounds, [80.0, 0.0, 120.0, 0.0], 1e-4);
    // a clipping path of a polygon
    let d = xclip_doc(&[[-10.0, -5.0], [10.0, -5.0], [0.0, 5.0]], true, Some(0), 0.0, 1.0);
    let sc = d.scene("Model");
    // the triangle (-10,-5) (10,-5) (0,5) cuts y = 0 at x = -5 .. 5
    bounds_close(sc.bounds, [-5.0, 0.0, 5.0, 0.0], 1e-4);
    // a clip that is switched off (group 71 = 0) shows the whole block
    let d = xclip_doc(&[[-10.0, -10.0], [10.0, 10.0]], false, None, 0.0, 1.0);
    bounds_close(d.scene("Model").bounds, [-100.0, 0.0, 100.0, 0.0], 1e-4);
}

#[test]
fn xclip_inverted_path_hides_the_inside() {
    // block line -100..100 and an inverted clip: the inside of the square +-10 is cut out, the outside up to +-1000 stays
    let mut d = Doc::new().block("B", [0.0, 0.0], &line("0", [-100.0, 0.0], [100.0, 0.0], None)).add(xclip_insert(0.0, 1.0));
    d.header.push_str(&t(&[(9, &"$XCLIPFRAME"), (280, &0)]));
    let ring = |v: f64| -> String {
        let mut s = String::new();
        for (x, y) in [(-v, -v), (v, -v), (v, v), (-v, v)] {
            s += &t(&[(10, &x), (20, &y), (30, &0.0)]);
        }
        s
    };
    let xrec = format!(
        "  0\nXRECORD\n{}{}{}{}{}{}",
        t(&[(5, &"B1")]),
        t(&[(100, &"AcDbXrecord"), (280, &1), (102, &"ACAD_INVERTEDCLIP_ROUNDTRIP")]),
        ring(10.0),
        t(&[(102, &"ACAD_INVERTEDCLIP_ROUNDTRIP_COMPARE")]),
        ring(1000.0),
        ""
    );
    d.objects = dict("D1", &[("ACAD_FILTER", "D2")])
        + &dict("D2", &[("SPATIAL", "F1")])
        + &spatial_filter("F1", &[[-1000.0, -1000.0], [1000.0, -1000.0], [1000.0, 1000.0], [-1000.0, 1000.0]], true, Some("D3"))
        + &dict("D3", &[("ACAD_XREC_ROUNDTRIP", "B1")])
        + &xrec;
    let sc = d.load().scene("Model");
    let pts = line_pts(&sc);
    assert_eq!(pts.len(), 4, "two pieces of the line: {pts:?}");
    let mut xs: Vec<f32> = pts.iter().map(|p| p[0]).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!((xs[0] + 100.0).abs() < 1e-3 && (xs[1] + 10.0).abs() < 1e-3 && (xs[2] - 10.0).abs() < 1e-3 && (xs[3] - 100.0).abs() < 1e-3, "{xs:?}");
}

#[test]
fn xclip_clips_filled_shapes_and_leaves_attributes_alone() {
    // a SOLID 40 wide in the block is cut to the path
    let mut d = Doc::new().block("B", [0.0, 0.0], &ent!("SOLID"; 8 => "0", 10 => -20.0, 20 => -5.0, 11 => 20.0, 21 => -5.0, 12 => -20.0, 22 => 5.0, 13 => 20.0, 23 => 5.0)).add(xclip_insert(0.0, 1.0));
    d.header.push_str(&t(&[(9, &"$XCLIPFRAME"), (280, &0)]));
    d.objects = dict("D1", &[("ACAD_FILTER", "D2")]) + &dict("D2", &[("SPATIAL", "F1")]) + &spatial_filter("F1", &[[-10.0, -10.0], [10.0, 10.0]], true, None);
    let sc = d.load().scene("Model");
    assert!((tri_area(&sc) - 20.0 * 10.0).abs() < 1e-3, "{}", tri_area(&sc));
    bounds_close(sc.bounds, [-10.0, -5.0, 10.0, 5.0], 1e-4);
}

fn solid_block() -> String {
    ent!("SOLID"; 8 => "0", 10 => -20.0, 20 => -5.0, 11 => 20.0, 21 => -5.0, 12 => -20.0, 22 => 5.0, 13 => 20.0, 23 => 5.0)
}

#[test]
fn xclip_with_concave_and_inverted_paths_clips_fills_exactly() {
    let doc_with = |boundary: &[[f64; 2]], inverted: Option<(f64, f64)>| -> Drawing {
        let mut d = Doc::new().block("B", [0.0, 0.0], &solid_block()).add(xclip_insert(0.0, 1.0));
        d.header.push_str(&t(&[(9, &"$XCLIPFRAME"), (280, &0)]));
        let ring = |v: f64| -> String {
            let mut s = String::new();
            for (x, y) in [(-v, -v), (v, -v), (v, v), (-v, v)] {
                s += &t(&[(10, &x), (20, &y), (30, &0.0)]);
            }
            s
        };
        d.objects = dict("D1", &[("ACAD_FILTER", "D2")]) + &dict("D2", &[("SPATIAL", "F1")]);
        match inverted {
            None => d.objects += &spatial_filter("F1", boundary, true, None),
            Some((inner, outer)) => {
                let xrec = format!(
                    "  0
XRECORD
{}{}{}{}{}",
                    t(&[(5, &"B1")]),
                    t(&[(100, &"AcDbXrecord"), (280, &1), (102, &"ACAD_INVERTEDCLIP_ROUNDTRIP")]),
                    ring(inner),
                    t(&[(102, &"ACAD_INVERTEDCLIP_ROUNDTRIP_COMPARE")]),
                    ring(outer)
                );
                d.objects += &(spatial_filter("F1", boundary, true, Some("D3")) + &dict("D3", &[("ACAD_XREC_ROUNDTRIP", "B1")]) + &xrec);
            }
        }
        d.load()
    };
    // a concave clipping path: the notch x -5..5, y 0..10 is cut out of the rectangle -10..10, -10..10; the SOLID
    // (-20..20, -5..5) keeps 20 x 10 - 10 x 5 = 150
    let sc = doc_with(&[[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [5.0, 10.0], [5.0, 0.0], [-5.0, 0.0], [-5.0, 10.0], [-10.0, 10.0]], None).scene("Model");
    assert!((tri_area(&sc) - 150.0).abs() < 1e-3, "{}", tri_area(&sc));
    bounds_close(sc.bounds, [-10.0, -5.0, 10.0, 5.0], 1e-4);
    // an inverted path (hole +-5 in the area +-1000): 400 - 100
    let sc = doc_with(&[[-1000.0, -1000.0], [1000.0, -1000.0], [1000.0, 1000.0], [-1000.0, 1000.0]], Some((5.0, 1000.0))).scene("Model");
    assert!((tri_area(&sc) - 300.0).abs() < 1e-3, "{}", tri_area(&sc));
    bounds_close(sc.bounds, [-20.0, -5.0, 20.0, 5.0], 1e-4);
}

#[test]
fn clipped_filled_paths_keep_their_closing_edge() {
    // a solid HATCH 120 x 20 in the block, clipped to its lower 5 units: the outline of the part is 2 x (110 + 5)
    let hatch = ent!("HATCH"; 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0, 210 => 0.0, 220 => 0.0, 230 => 1.0, 2 => "SOLID", 70 => 1, 71 => 0, 91 => 1, 92 => 3, 72 => 0, 73 => 1, 93 => 4,
        10 => -60.0, 20 => 40.0, 10 => 60.0, 20 => 40.0, 10 => 60.0, 20 => 60.0, 10 => -60.0, 20 => 60.0, 97 => 0, 75 => 0, 76 => 1, 98 => 0);
    let mut d = Doc::new().block("B", [0.0, 0.0], &hatch).add(xclip_insert(0.0, 1.0));
    d.header.push_str(&t(&[(9, &"$XCLIPFRAME"), (280, &0)]));
    d.objects = dict("D1", &[("ACAD_FILTER", "D2")]) + &dict("D2", &[("SPATIAL", "F1")]) + &spatial_filter("F1", &[[-55.0, -25.0], [55.0, 45.0]], true, None);
    let sc = d.load().scene("Model");
    assert!((tri_area(&sc) - 110.0 * 5.0).abs() < 1e-2, "{}", tri_area(&sc));
    assert!((total_len(&sc) - 230.0).abs() < 1e-2, "{}", total_len(&sc));
}

#[test]
fn lines_along_the_edge_of_a_concave_clipping_path_stay() {
    // the block line y = 0 runs along the bottom of the notch: ezdxf keeps it
    let sc = xclip_doc(&[[-60.0, -30.0], [60.0, -30.0], [60.0, 30.0], [10.0, 30.0], [10.0, 0.0], [-10.0, 0.0], [-10.0, 30.0], [-60.0, 30.0]], true, Some(0), 0.0, 1.0).scene("Model");
    assert!((total_len(&sc) - 120.0).abs() < 1e-3, "{}", total_len(&sc));
}

#[test]
fn mpolygon_is_drawn_with_fill_and_boundary() {
    let mpolygon = |extra: &str| {
        ent!("MPOLYGON"; 8 => "0", 100 => "AcDbMPolygon", 70 => 1, 10 => 0.0, 20 => 0.0, 30 => 0.0, 210 => 0.0, 220 => 0.0, 230 => 1.0, 2 => "SOLID", 71 => 1,
            91 => 1, 92 => 3, 72 => 0, 73 => 1, 93 => 4, 10 => 0.0, 20 => 0.0, 10 => 10.0, 20 => 0.0, 10 => 10.0, 20 => 10.0, 10 => 0.0, 20 => 10.0, 97 => 0,
            76 => 1, 73 => 0, 98 => 0, 63 => 1, 11 => 0.0, 21 => 0.0, 99 => 0)
            + extra
    };
    let d = raw(&mpolygon(""));
    assert_eq!(d.layout_entities("Model")[0].dxftype(), "MPOLYGON");
    let sc = d.scene_with("Model", &SceneOptions::default()).unwrap();
    // filled with the fill colour (ACI 1, red) and outlined in the colour of the entity (white on the dark background)
    assert!((tri_area(&sc) - 100.0).abs() < 1e-3, "{}", tri_area(&sc));
    let fill = sc.batches.iter().find(|b| !b.fills.is_empty()).expect("fill batch");
    assert_eq!(fill.color, [255, 0, 0, 255]);
    let outline = sc.batches.iter().find(|b| !b.lines.is_empty() && b.fills.is_empty()).expect("outline batch");
    assert_eq!(outline.color, [255, 255, 255, 255]);
    bounds_close(sc.bounds, [0.0, 0.0, 10.0, 10.0], 1e-4);
    // the offset vector (group 11 / 21 after the paths) shifts the boundary
    let shifted = ent!("MPOLYGON"; 8 => "0", 100 => "AcDbMPolygon", 70 => 1, 10 => 0.0, 20 => 0.0, 30 => 0.0, 2 => "SOLID", 71 => 1, 91 => 1, 92 => 3, 72 => 0, 73 => 1, 93 => 4,
        10 => 0.0, 20 => 0.0, 10 => 10.0, 20 => 0.0, 10 => 10.0, 20 => 10.0, 10 => 0.0, 20 => 10.0, 97 => 0, 76 => 1, 98 => 0, 63 => 1, 11 => 100.0, 21 => 50.0);
    bounds_close(raw(&shifted).scene("Model").bounds, [100.0, 50.0, 110.0, 60.0], 1e-4);
    // an outline only: no solid fill and no pattern name
    let open = ent!("MPOLYGON"; 8 => "0", 100 => "AcDbMPolygon", 70 => 1, 10 => 0.0, 20 => 0.0, 30 => 0.0, 2 => "", 71 => 0, 91 => 1, 92 => 3, 72 => 0, 73 => 1, 93 => 3,
        10 => 0.0, 20 => 0.0, 10 => 10.0, 20 => 0.0, 10 => 5.0, 20 => 8.0, 97 => 0, 76 => 1, 78 => 0, 98 => 0);
    let sc = raw(&open).scene("Model");
    assert!(tri_area(&sc) < 1e-9);
    assert!(sc.batches.iter().map(|b| b.lines.len()).sum::<usize>() >= 6);
    // a true colour in the gradient record is the fill colour
    let gradient = ent!("MPOLYGON"; 8 => "0", 100 => "AcDbMPolygon", 70 => 1, 10 => 0.0, 20 => 0.0, 30 => 0.0, 2 => "SOLID", 71 => 1, 91 => 1, 92 => 3, 72 => 0, 73 => 1, 93 => 4,
        10 => 0.0, 20 => 0.0, 10 => 10.0, 20 => 0.0, 10 => 10.0, 20 => 10.0, 10 => 0.0, 20 => 10.0, 97 => 0, 76 => 1, 98 => 0, 63 => 1, 11 => 0.0, 21 => 0.0,
        450 => 0, 451 => 0, 460 => 0.0, 461 => 0.0, 452 => 0, 462 => 0.0, 453 => 2, 463 => 0.0, 421 => 0x00FF00, 463 => 1.0, 421 => 0x0000FF, 470 => "LINEAR");
    let sc = raw(&gradient).scene("Model");
    assert_eq!(sc.batches.iter().find(|b| !b.fills.is_empty()).unwrap().color, [0, 255, 0, 255]);
}

#[test]
fn layer_transparency_is_read_from_the_xdata() {
    let mut d = Doc::new();
    // AcCmTransparency: 0x02000080 = alpha 128; group 440 in a LAYER record is not a transparency
    d.layers.push_str(&ent!("LAYER"; 2 => "GLASS", 70 => 0, 62 => 3, 6 => "Continuous", 1001 => "AcCmTransparency", 1071 => 0x0200_0080i64));
    d.layers.push_str(&ent!("LAYER"; 2 => "WRONG", 70 => 0, 62 => 3, 6 => "Continuous", 440 => 0x0200_0080i64));
    d.layers.push_str(&ent!("LAYER"; 2 => "SOLID", 70 => 0, 62 => 3, 6 => "Continuous", 1001 => "ACAD", 1000 => "x", 1001 => "AcCmTransparency", 1071 => 0x0200_00FFi64));
    let sc = d
        .add(line("GLASS", [0.0, 0.0], [1.0, 0.0], None))
        .add(line("WRONG", [0.0, 1.0], [1.0, 1.0], None))
        .add(line("SOLID", [0.0, 2.0], [1.0, 2.0], None))
        .load()
        .scene("Model");
    let alpha_of = |name: &str| sc.batches.iter().find(|b| sc.layers.iter().any(|l| l.id == b.layer && l.name == name)).map(|b| b.color[3]);
    assert_eq!(alpha_of("GLASS"), Some(128));
    assert_eq!(alpha_of("WRONG"), Some(255));
    assert_eq!(alpha_of("SOLID"), Some(255));
}

// ------------------------------------------------------------------------------------------------ finding 5: quadratic behaviour
#[test]
fn type_counts_use_an_index_and_keep_their_order() {
    let mk = |t: &str| dxf::Entity { c: dxf::Common::new(0, 0), d: dxf::Data::Other(t.to_string()) };
    let ents: Vec<dxf::Entity> = ["B", "A", "B", "C", "A", "B", "D"].iter().map(|t| mk(t)).collect();
    let counts = info::type_counts(&ents);
    // most common first, ties in order of first appearance
    assert_eq!(counts, vec![("B".to_string(), 3), ("A".to_string(), 2), ("C".to_string(), 1), ("D".to_string(), 1)]);
    // 200 000 distinct types
    let many: Vec<dxf::Entity> = (0..200_000).map(|i| mk(&format!("X{i}"))).collect();
    let t0 = Instant::now();
    let c = info::type_counts(&many);
    assert_eq!(c.len(), 200_000);
    assert_eq!(c[0].0, "X0");
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
}

#[test]
fn many_layers_load_fast_and_a_repeated_layer_replaces_the_first() {
    let mut text = String::from("  0\nSECTION\n  2\nTABLES\n  0\nTABLE\n  2\nLAYER\n");
    for i in 0..60_000 {
        text.push_str(&format!("  0\nLAYER\n  2\nL{i}\n 70\n0\n 62\n{}\n  6\nContinuous\n", i % 200 + 1));
    }
    text.push_str("  0\nLAYER\n  2\nL5\n 70\n0\n 62\n9\n  6\nContinuous\n");
    text.push_str("  0\nENDTAB\n  0\nENDSEC\n  0\nSECTION\n  2\nENTITIES\n  0\nLINE\n  8\nL7\n 10\n0\n 20\n0\n 11\n1\n 21\n1\n  0\nENDSEC\n  0\nEOF\n");
    let t0 = Instant::now();
    let d = load_bytes(text.as_bytes(), fonts()).unwrap();
    assert!(t0.elapsed() < Duration::from_secs(20), "{:?}", t0.elapsed());
    assert_eq!(d.info().layers, 60_000);
    let doc = d.document();
    let id = doc.layer_names.find("L5").unwrap();
    assert_eq!(doc.layer(id).unwrap().color, 9, "the later record wins");
    assert_eq!(doc.layers.iter().filter(|l| l.name == "L5").count(), 1);
    assert_eq!(doc.layer(doc.layer_names.find("L59999").unwrap()).unwrap().name, "L59999");
    assert!(d.layer_visible(id));
}

#[test]
fn spline_degrees_are_limited() {
    use crate::bspline::{BSpline, MAX_DEGREE};
    let ctrl = |n: usize| -> Vec<[f64; 3]> { (0..n).map(|i| [i as f64, ((i * 7) % 13) as f64, 0.0]).collect() };
    assert!(BSpline::new(ctrl(100), 3, vec![], vec![]).is_some());
    assert!(BSpline::new(ctrl(100), MAX_DEGREE, vec![], vec![]).is_some());
    assert!(BSpline::new(ctrl(100), MAX_DEGREE + 1, vec![], vec![]).is_none());
    assert!(BSpline::new(ctrl(8000), 400, vec![], vec![]).is_none());
    // a SPLINE entity of degree 400 with 8000 control points: not evaluated, so it is quick
    let mut s = ent!("SPLINE"; 8 => "0", 70 => 8, 71 => 400, 72 => 0, 73 => 8000, 74 => 0);
    for i in 0..8000 {
        s += &t(&[(10, &(i as f64)), (20, &(((i * 7) % 13) as f64)), (30, &0.0)]);
    }
    let t0 = Instant::now();
    let d = raw(&(s + &line("0", [0.0, 0.0], [1.0, 1.0], None)));
    let sc = d.scene("Model");
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
    assert_eq!(sc.batches.iter().map(|b| b.lines.len() / 2).sum::<usize>(), 1, "only the line is drawn");
}

#[test]
fn mtext_tab_stops_are_shared_between_tokens() {
    let tokens = mtext::parse_mtext("\\pt1,2,3,4,5;a b c d e", mtext::Ctx::new(2.5, 1.0, 0.0, 256));
    assert!(tokens.len() >= 9);
    let first = tokens[0].ctx.para.tab_stops.clone();
    assert_eq!(*first, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
    for tk in &tokens {
        assert!(std::sync::Arc::ptr_eq(&tk.ctx.para.tab_stops, &first), "a context clone must not copy the stops");
    }
    // 8000 stops and 8000 words
    let mut text = String::from("\\pt");
    text.push_str(&(1..=8000).map(|i| i.to_string()).collect::<Vec<_>>().join(","));
    text.push(';');
    text.push_str(&"a ".repeat(8000));
    let t0 = Instant::now();
    let tokens = mtext::parse_mtext(&text, mtext::Ctx::new(2.5, 1.0, 0.0, 256));
    assert!(tokens.len() >= 16_000);
    assert_eq!(tokens[0].ctx.para.tab_stops.len(), 8000);
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
}

#[test]
fn text_wrap_measures_every_word_once() {
    use std::cell::Cell;
    let calls = Cell::new(0usize);
    let chars = Cell::new(0usize);
    let width = |s: &str| {
        calls.set(calls.get() + 1);
        chars.set(chars.get() + s.chars().count());
        s.chars().count() as f64
    };
    let text = "ab ".repeat(20_000);
    // one long line: the box is wider than the text
    let lines = mtext::text_wrap(&text, Some(1e12), &width);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].chars().count(), 60_000 - 1);
    assert!(calls.get() <= 2 * 40_000 + 10, "{}", calls.get());
    assert!(chars.get() <= 2 * text.chars().count(), "every character is measured about once, not once per word: {}", chars.get());
    // the wrapping itself is unchanged
    let w = |s: &str| s.chars().count() as f64;
    assert_eq!(mtext::text_wrap("aaa bbb ccc ddd", Some(7.0), &w), vec!["aaa bbb", "ccc ddd"]);
    assert_eq!(mtext::text_wrap("  lead  x", Some(100.0), &w), vec!["  lead  x"]);
    assert_eq!(mtext::text_wrap("aaaaaaaaaa bb", Some(5.0), &w), vec!["aaaaaaaaaa", "bb"]);
    assert_eq!(mtext::text_wrap("a b\n\nc", None, &w), vec!["a b", "", "c"]);
}

#[test]
fn many_glyph_outlines_are_triangulated_without_comparing_every_pair() {
    // 60 000 small squares side by side, one of them with a hole: the nesting of the rings was found pair by pair
    let mut rings: Vec<Vec<[f64; 2]>> = Vec::new();
    for i in 0..60_000 {
        let (x, y) = ((i % 300) as f64 * 3.0, (i / 300) as f64 * 3.0);
        rings.push(vec![[x, y], [x + 2.0, y], [x + 2.0, y + 2.0], [x, y + 2.0]]);
    }
    rings.push(vec![[1000.0, 1000.0], [1010.0, 1000.0], [1010.0, 1010.0], [1000.0, 1010.0]]);
    rings.push(vec![[1002.0, 1002.0], [1004.0, 1002.0], [1004.0, 1004.0], [1002.0, 1004.0]]);
    let t0 = Instant::now();
    let tris = crate::tess::triangulate_even_odd(&rings);
    let area: f64 = tris.chunks(3).map(|t| crate::geom::ring_area(t).abs()).sum();
    assert!((area - (60_000.0 * 4.0 + 100.0 - 4.0)).abs() < 1e-6, "{area}");
    assert!(t0.elapsed() < Duration::from_secs(20), "{:?}", t0.elapsed());
    // a single line of text: 40 000 glyph outlines in one long row, every second one with a hole, as tall as the row
    let mut rings: Vec<Vec<[f64; 2]>> = Vec::new();
    for i in 0..40_000 {
        let x = i as f64 * 2.0;
        rings.push(vec![[x, 0.0], [x + 1.5, 0.0], [x + 1.5, 2.5], [x, 2.5]]);
        if i % 2 == 0 {
            rings.push(vec![[x + 0.5, 0.5], [x + 1.0, 0.5], [x + 1.0, 1.0], [x + 0.5, 1.0]]);
        }
    }
    let t0 = Instant::now();
    let tris = crate::tess::triangulate_even_odd(&rings);
    let area: f64 = tris.chunks(3).map(|t| crate::geom::ring_area(t).abs()).sum();
    assert!((area - (40_000.0 * 3.75 - 20_000.0 * 0.25)).abs() < 1e-6, "{area}");
    assert!(t0.elapsed() < Duration::from_secs(20), "{:?}", t0.elapsed());
}

#[test]
fn clip_shapes_cover_concave_and_inverted_areas() {
    use crate::clip::ClipShape;
    // an L shape (concave): a polygon spanning the notch is cut into the two arms
    let l = ClipShape::polygon(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [6.0, 10.0], [6.0, 4.0], [4.0, 4.0], [4.0, 10.0], [0.0, 10.0]]).unwrap();
    let pieces = l.clip_polygon(&[[-1.0, 6.0], [11.0, 6.0], [11.0, 9.0], [-1.0, 9.0]]);
    let area: f64 = pieces.iter().map(|p| geom::ring_area(p).abs()).sum();
    assert!((area - (4.0 + 4.0) * 3.0).abs() < 1e-9, "two arms of 4 x 3: {area}");
    // an inverted shape: the square 4..6 is a hole in the rectangle 0..10
    let inv = ClipShape::inverted(vec![[4.0, 4.0], [6.0, 4.0], [6.0, 6.0], [4.0, 6.0]], [0.0, 0.0, 10.0, 10.0]).unwrap();
    assert!(inv.contains([1.0, 1.0]) && !inv.contains([5.0, 5.0]) && !inv.contains([11.0, 5.0]));
    let segs = inv.clip_line([-5.0, 5.0], [15.0, 5.0]);
    assert_eq!(segs.len(), 2, "{segs:?}");
    assert!((segs[0].0[0] - 0.0).abs() < 1e-9 && (segs[0].1[0] - 4.0).abs() < 1e-9 && (segs[1].0[0] - 6.0).abs() < 1e-9 && (segs[1].1[0] - 10.0).abs() < 1e-9);
    let pieces = inv.clip_polygon(&[[2.0, 2.0], [8.0, 2.0], [8.0, 8.0], [2.0, 8.0]]);
    let area: f64 = pieces.iter().map(|p| geom::ring_area(p).abs()).sum();
    assert!((area - (36.0 - 4.0)).abs() < 1e-9, "{area}");
    assert!(inv.contains_box([0.5, 0.5, 3.0, 3.0]) && !inv.contains_box([3.0, 3.0, 5.0, 5.0]));
}
