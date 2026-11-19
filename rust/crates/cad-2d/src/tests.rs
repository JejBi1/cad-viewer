//! Tests that build small DXF files and check entities, layers, batches, z-order, bounds, picking and warnings.
use super::*;
use crate::geom::{Z_AXIS, V3};
use std::fmt::Display;

fn fonts() -> FontOptions {
    FontOptions { system_fonts: false, dirs: Vec::new() }
}

/// Group code / value pairs as DXF text.
fn t(pairs: &[(i32, &dyn Display)]) -> String {
    let mut s = String::new();
    for (c, v) in pairs {
        s.push_str(&format!("{:>3}\n{}\n", c, v));
    }
    s
}

macro_rules! ent {
    ($ty:expr; $($c:expr => $v:expr),* $(,)?) => {{
        let mut s = format!("  0\n{}\n", $ty);
        $( s.push_str(&t(&[($c, &$v as &dyn Display)])); )*
        s
    }};
}

struct Doc {
    header: String,
    layers: String,
    linetypes: String,
    blocks: String,
    entities: String,
    objects: String,
}

impl Doc {
    fn new() -> Doc {
        Doc {
            header: t(&[(9, &"$ACADVER"), (1, &"AC1027"), (9, &"$INSUNITS"), (70, &4)]),
            layers: String::new(),
            linetypes: String::new(),
            blocks: String::new(),
            entities: String::new(),
            objects: String::new(),
        }
    }

    fn layer(mut self, name: &str, color: i32, flags: i32) -> Doc {
        self.layers.push_str(&ent!("LAYER"; 2 => name, 70 => flags, 62 => color, 6 => "Continuous"));
        self
    }

    fn block(mut self, name: &str, base: [f64; 2], body: &str) -> Doc {
        self.blocks.push_str(&format!("{}{}{}", ent!("BLOCK"; 2 => name, 70 => 0, 10 => base[0], 20 => base[1], 30 => 0.0), body, "  0\nENDBLK\n"));
        self
    }

    fn add(mut self, e: String) -> Doc {
        self.entities.push_str(&e);
        self
    }

    fn text(&self) -> String {
        let mut s = String::new();
        s.push_str("  0\nSECTION\n  2\nHEADER\n");
        s.push_str(&self.header);
        s.push_str("  0\nENDSEC\n  0\nSECTION\n  2\nTABLES\n");
        s.push_str("  0\nTABLE\n  2\nLTYPE\n");
        s.push_str(&ent!("LTYPE"; 2 => "Continuous", 70 => 0, 3 => "Solid line", 72 => 65, 73 => 0, 40 => 0.0));
        s.push_str(&ent!("LTYPE"; 2 => "DASHED", 70 => 0, 3 => "Dashed", 72 => 65, 73 => 2, 40 => 0.75, 49 => 0.5, 74 => 0, 49 => -0.25, 74 => 0));
        s.push_str(&self.linetypes);
        s.push_str("  0\nENDTAB\n  0\nTABLE\n  2\nLAYER\n");
        s.push_str(&ent!("LAYER"; 2 => "0", 70 => 0, 62 => 7, 6 => "Continuous"));
        s.push_str(&self.layers);
        s.push_str("  0\nENDTAB\n  0\nTABLE\n  2\nSTYLE\n");
        s.push_str(&ent!("STYLE"; 2 => "Standard", 70 => 0, 40 => 0.0, 41 => 1.0, 50 => 0.0, 3 => "arial.ttf"));
        s.push_str("  0\nENDTAB\n  0\nENDSEC\n  0\nSECTION\n  2\nBLOCKS\n");
        s.push_str(&self.blocks);
        s.push_str("  0\nENDSEC\n  0\nSECTION\n  2\nENTITIES\n");
        s.push_str(&self.entities);
        s.push_str("  0\nENDSEC\n");
        if !self.objects.is_empty() {
            s.push_str("  0\nSECTION\n  2\nOBJECTS\n");
            s.push_str(&self.objects);
            s.push_str("  0\nENDSEC\n");
        }
        s.push_str("  0\nEOF\n");
        s
    }

    fn load(&self) -> Drawing {
        load_bytes(self.text().as_bytes(), fonts()).expect("test DXF loads")
    }
}

fn line(layer: &str, a: [f64; 2], b: [f64; 2], color: Option<i32>) -> String {
    let mut s = ent!("LINE"; 8 => layer, 10 => a[0], 20 => a[1], 30 => 0.0, 11 => b[0], 21 => b[1], 31 => 0.0);
    if let Some(c) = color {
        s.push_str(&t(&[(62, &c)]));
    }
    s
}

fn bounds_close(b: Option<([f64; 2], [f64; 2])>, e: [f64; 4], tol: f64) {
    let b = b.expect("bounds");
    let got = [b.0[0], b.0[1], b.1[0], b.1[1]];
    for i in 0..4 {
        assert!((got[i] - e[i]).abs() <= tol, "bounds {:?} expected {:?}", got, e);
    }
}

fn total_len(sc: &Scene2d) -> f64 {
    sc.batches
        .iter()
        .map(|b| b.lines.chunks(2).filter(|w| w.len() == 2).map(|w| (((w[1][0] - w[0][0]) as f64).powi(2) + ((w[1][1] - w[0][1]) as f64).powi(2)).sqrt()).sum::<f64>())
        .sum()
}

fn tri_area(sc: &Scene2d) -> f64 {
    sc.batches
        .iter()
        .map(|b| b.fills.chunks(3).filter(|t| t.len() == 3).map(|t| (((t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[2][0] - t[0][0]) * (t[1][1] - t[0][1])) as f64 / 2.0).abs()).sum::<f64>())
        .sum()
}

// ------------------------------------------------------------------------------------------------ entities and bounds
#[test]
fn line_circle_arc_have_exact_bounds_and_layers() {
    let d = Doc::new()
        .layer("GEOM", 1, 0)
        .add(line("GEOM", [0.0, 0.0], [100.0, 50.0], None))
        .add(ent!("CIRCLE"; 8 => "GEOM", 10 => 200.0, 20 => 100.0, 30 => 0.0, 40 => 10.0))
        .add(ent!("ARC"; 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0, 40 => 20.0, 50 => 0.0, 51 => 90.0))
        .load();
    assert_eq!(d.layouts().iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), vec!["Model", "Layout1"]);
    assert_eq!(d.default_layout(), "Model");
    let sc = d.scene("Model");
    // x: 0 .. 210 (circle right edge), y: 0 .. 110 (circle top)
    bounds_close(sc.bounds, [0.0, 0.0, 210.0, 110.0], 1e-9);
    let geom = sc.layers.iter().find(|l| l.name == "GEOM").unwrap();
    assert_eq!((geom.entities, geom.visible, geom.off_in_file), (2, true, false));
    let l0 = sc.layers.iter().find(|l| l.name == "0").unwrap();
    assert_eq!(l0.entities, 1);
    // the line is drawn in the colour of its layer (red), the arc on layer 0 with ACI 7: white on the dark model space
    let colors: Vec<[u8; 4]> = sc.batches.iter().map(|b| b.color).collect();
    assert!(colors.contains(&[255, 0, 0, 255]));
    assert!(colors.contains(&[255, 255, 255, 255]));
    // the circle is flattened into many short segments on a circle of radius 10
    let circle_batches: Vec<&DrawBatch> = sc.batches.iter().filter(|b| b.color == [255, 0, 0, 255]).collect();
    let pts: Vec<[f32; 2]> = circle_batches.iter().flat_map(|b| b.lines.iter().copied()).filter(|p| p[0] > 150.0).collect();
    assert!(pts.len() > 16);
    for p in pts {
        let r = (((p[0] - 200.0) as f64).powi(2) + ((p[1] - 100.0) as f64).powi(2)).sqrt();
        assert!((r - 10.0).abs() < 0.05, "r = {}", r);
    }
}

#[test]
fn light_background_changes_the_foreground_colour() {
    let d = Doc::new().add(line("0", [0.0, 0.0], [1.0, 1.0], None)).load();
    let dark = d.scene_with("Model", &SceneOptions { light_bg: false, ..Default::default() }).unwrap();
    assert_eq!(dark.batches[0].color, [255, 255, 255, 255]);
    assert_eq!(d.background_color(), [0x21, 0x28, 0x30]);
    let light = d.scene_with("Model", &SceneOptions { light_bg: true, ..Default::default() }).unwrap();
    assert_eq!(light.batches[0].color, [0, 0, 0, 255]);
    assert_eq!(d.background_color(), [255, 255, 255]);
}

#[test]
fn color_rules_bylayer_byblock_true_color() {
    let blk = format!(
        "{}{}",
        line("0", [0.0, 0.0], [10.0, 0.0], Some(0)),   // BYBLOCK
        line("0", [0.0, 5.0], [10.0, 5.0], None)       // BYLAYER, layer 0 -> layer of the INSERT
    );
    let d = Doc::new()
        .layer("L1", 3, 0)
        .block("B", [0.0, 0.0], &blk)
        .add(ent!("INSERT"; 2 => "B", 8 => "L1", 62 => 5, 10 => 0.0, 20 => 0.0, 30 => 0.0))
        .add({
            let mut s = line("0", [0.0, 20.0], [10.0, 20.0], None);
            s.push_str(&t(&[(420, &0x0A_C81Eu32)]));
            s
        })
        .add(line("0", [0.0, 30.0], [10.0, 30.0], Some(30)))
        .load();
    let sc = d.scene("Model");
    let by_y = |y: f64| -> [u8; 4] {
        for b in &sc.batches {
            for w in b.lines.chunks(2) {
                if ((w[0][1] as f64) - y).abs() < 1e-6 && ((w[1][1] as f64) - y).abs() < 1e-6 {
                    return b.color;
                }
            }
        }
        panic!("no segment at y = {y}");
    };
    assert_eq!(by_y(0.0), [0, 0, 255, 255], "BYBLOCK takes the colour of the INSERT (ACI 5, blue)");
    assert_eq!(by_y(5.0), [0, 255, 0, 255], "BYLAYER on layer 0 inside a block takes the layer of the INSERT (ACI 3, green)");
    assert_eq!(by_y(20.0), [0x0A, 0xC8, 0x1E, 255], "true colour");
    assert_eq!(by_y(30.0), [0xFF, 0x7F, 0x00, 255], "ACI 30 from the palette");
    // the block entities report the layer of the INSERT
    let l1 = sc.layers.iter().find(|l| l.name == "L1").unwrap().id;
    let batch = sc.batches.iter().find(|b| b.color == [0, 255, 0, 255]).unwrap();
    assert_eq!(batch.layer, l1);
}

#[test]
fn layers_off_and_frozen_are_hidden_but_drawn_and_togglable() {
    let d = Doc::new()
        .layer("OFF", 1, 0)
        .layer("FRZ", 2, 1)
        .layer("ON", 3, 0)
        .add(line("OFF", [0.0, 0.0], [10.0, 0.0], None))
        .add(line("FRZ", [0.0, 5.0], [10.0, 5.0], None))
        .add(line("ON", [0.0, 10.0], [10.0, 10.0], None))
        .load();
    // a negative colour switches the layer off in the file
    let mut doc_text = Doc::new().layer("OFF", -1, 0).layer("FRZ", 2, 1).layer("ON", 3, 0);
    doc_text.entities = line("OFF", [0.0, 0.0], [10.0, 0.0], None) + &line("FRZ", [0.0, 5.0], [10.0, 5.0], None) + &line("ON", [0.0, 10.0], [10.0, 10.0], None);
    let d2 = doc_text.load();
    drop(d);
    let sc = d2.scene("Model");
    let find = |n: &str| sc.layers.iter().find(|l| l.name == n).unwrap().clone();
    assert!(find("OFF").off_in_file && !find("OFF").visible);
    assert!(find("FRZ").off_in_file && !find("FRZ").visible);
    assert!(!find("ON").off_in_file && find("ON").visible);
    // all three lines are in the batches (the scene is drawn with every layer on), the bounds only count the shown one
    assert_eq!(sc.batches.iter().map(|b| b.lines.len() / 2).sum::<usize>(), 3);
    bounds_close(sc.bounds, [0.0, 10.0, 10.0, 10.0], 1e-6);
    assert_eq!(d2.visible_batches().iter().filter(|&&v| v).count(), 1);
    // switch a layer on: the bounds follow, no new scene is needed
    d2.set_layer_visible(find("OFF").id, true);
    bounds_close(d2.visible_bounds(), [0.0, 0.0, 10.0, 10.0], 1e-6);
    assert_eq!(d2.visible_batches().iter().filter(|&&v| v).count(), 2);
    assert!(d2.layer_visible(find("OFF").id));
    d2.reset_layer_visibility();
    assert!(!d2.layer_visible(find("OFF").id));
    d2.show_all_layers();
    bounds_close(d2.visible_bounds(), [0.0, 0.0, 10.0, 10.0], 1e-6);
    let sc = d2.scene("Model");
    assert!(sc.layers.iter().all(|l| l.visible));
}

#[test]
fn z_order_is_kept_for_overlapping_groups_and_merged_otherwise() {
    // red, blue, red over the same place: three batches in this order
    let overlap = Doc::new()
        .add(line("0", [0.0, 0.0], [10.0, 10.0], Some(1)))
        .add(line("0", [0.0, 10.0], [10.0, 0.0], Some(5)))
        .add(line("0", [0.0, 5.0], [10.0, 5.0], Some(1)))
        .load()
        .scene("Model");
    let order: Vec<[u8; 4]> = overlap.batches.iter().map(|b| b.color).collect();
    assert_eq!(order, vec![[255, 0, 0, 255], [0, 0, 255, 255], [255, 0, 0, 255]]);
    // far apart: nothing overlaps, so the reds share one batch
    let apart = Doc::new()
        .add(line("0", [0.0, 0.0], [10.0, 0.0], Some(1)))
        .add(line("0", [1000.0, 1000.0], [1010.0, 1000.0], Some(5)))
        .add(line("0", [0.0, 5.0], [10.0, 5.0], Some(1)))
        .load()
        .scene("Model");
    assert_eq!(apart.batches.len(), 2);
    let red = apart.batches.iter().find(|b| b.color == [255, 0, 0, 255]).unwrap();
    assert_eq!(red.lines.len(), 4);
}

#[test]
fn draw_order_table_reorders_entities() {
    // two overlapping entities; the SORTENTSTABLE of the model space block record puts the first one on top
    let mut d = Doc::new();
    d.header.push_str(&t(&[(9, &"$HANDSEED"), (5, &"FF")]));
    d.entities = format!("{}{}", ent!("LINE"; 5 => "A1", 330 => "1F", 8 => "0", 62 => 1, 10 => 0.0, 20 => 0.0, 30 => 0.0, 11 => 10.0, 21 => 10.0, 31 => 0.0), ent!("LINE"; 5 => "A2", 330 => "1F", 8 => "0", 62 => 5, 10 => 0.0, 20 => 10.0, 30 => 0.0, 11 => 10.0, 21 => 0.0, 31 => 0.0));
    let plain = d.load().scene("Model");
    assert_eq!(plain.batches.iter().map(|b| b.color).collect::<Vec<_>>(), vec![[255, 0, 0, 255], [0, 0, 255, 255]]);
    // BLOCK_RECORD *Model_Space = 1F, SORTENTSTABLE: A1 sorts after A2
    let mut d = Doc::new();
    d.entities = format!("{}{}", ent!("LINE"; 5 => "A1", 330 => "1F", 8 => "0", 62 => 1, 10 => 0.0, 20 => 0.0, 30 => 0.0, 11 => 10.0, 21 => 10.0, 31 => 0.0), ent!("LINE"; 5 => "A2", 330 => "1F", 8 => "0", 62 => 5, 10 => 0.0, 20 => 10.0, 30 => 0.0, 11 => 10.0, 21 => 0.0, 31 => 0.0));
    let mut text = d.text();
    text = text.replace("  0\nTABLE\n  2\nLAYER\n", &format!("  0\nTABLE\n  2\nBLOCK_RECORD\n{}  0\nENDTAB\n  0\nTABLE\n  2\nLAYER\n", ent!("BLOCK_RECORD"; 5 => "1F", 2 => "*Model_Space")));
    text.push_str("");
    let objects = format!(
        "  0\nSECTION\n  2\nOBJECTS\n{}{}  0\nENDSEC\n",
        ent!("SORTENTSTABLE"; 5 => "70", 100 => "AcDbSortentsTable", 330 => "1F", 331 => "A1", 5 => "B0", 331 => "A2", 5 => "A0"),
        ""
    );
    text = text.replace("  0\nEOF\n", &format!("{}  0\nEOF\n", objects));
    let sorted = load_bytes(text.as_bytes(), fonts()).unwrap().scene("Model");
    assert_eq!(sorted.batches.iter().map(|b| b.color).collect::<Vec<_>>(), vec![[0, 0, 255, 255], [255, 0, 0, 255]]);
}

#[test]
fn block_references_scale_rotate_and_repeat() {
    let body = line("0", [0.0, 0.0], [10.0, 0.0], None);
    let d = Doc::new()
        .block("B", [0.0, 0.0], &body)
        .add(ent!("INSERT"; 2 => "B", 8 => "0", 10 => 100.0, 20 => 100.0, 30 => 0.0, 41 => 2.0, 42 => 2.0, 43 => 1.0, 50 => 90.0))
        .load();
    let sc = d.scene("Model");
    // (0,0)-(10,0) scaled by 2 and rotated by 90 degrees about (100,100): (100,100)-(100,120)
    bounds_close(sc.bounds, [100.0, 100.0, 100.0, 120.0], 1e-6);
    // MINSERT 3 columns x 2 rows, spacing 20 / 30
    let d = Doc::new()
        .block("B", [0.0, 0.0], &body)
        .add(ent!("INSERT"; 2 => "B", 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0, 70 => 3, 71 => 2, 44 => 20.0, 45 => 30.0))
        .load();
    let sc = d.scene("Model");
    assert_eq!(sc.batches.iter().map(|b| b.lines.len() / 2).sum::<usize>(), 6);
    bounds_close(sc.bounds, [0.0, 0.0, 50.0, 30.0], 1e-6);
    // the base point of the block is the origin of the reference
    let d = Doc::new()
        .block("B", [5.0, 5.0], &line("0", [5.0, 5.0], [15.0, 5.0], None))
        .add(ent!("INSERT"; 2 => "B", 8 => "0", 10 => 100.0, 20 => 100.0, 30 => 0.0))
        .load();
    bounds_close(d.scene("Model").bounds, [100.0, 100.0, 110.0, 100.0], 1e-6);
}

#[test]
fn nested_and_missing_blocks() {
    let d = Doc::new()
        .block("INNER", [0.0, 0.0], &line("0", [0.0, 0.0], [1.0, 0.0], None))
        .block("OUTER", [0.0, 0.0], &ent!("INSERT"; 2 => "INNER", 8 => "0", 10 => 10.0, 20 => 0.0, 30 => 0.0, 41 => 5.0, 42 => 5.0, 43 => 5.0))
        .add(ent!("INSERT"; 2 => "OUTER", 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0))
        .add(ent!("INSERT"; 2 => "NOPE", 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0))
        .load();
    let sc = d.scene("Model");
    bounds_close(sc.bounds, [10.0, 0.0, 15.0, 0.0], 1e-6);
}

#[test]
fn polylines_with_bulge_and_width() {
    // a semicircle by bulge 1 from (0,0) to (10,0): radius 5, bulge towards -y for a counter-clockwise arc
    let d = Doc::new()
        .add(ent!("LWPOLYLINE"; 8 => "0", 90 => 2, 70 => 0, 10 => 0.0, 20 => 0.0, 42 => 1.0, 10 => 10.0, 20 => 0.0))
        .load();
    let sc = d.scene("Model");
    let l = total_len(&sc);
    assert!((l - std::f64::consts::PI * 5.0).abs() < 0.05, "semicircle length {}", l);
    // a polyline with constant width is a filled band: area = length * width
    let d = Doc::new().add(ent!("LWPOLYLINE"; 8 => "0", 90 => 2, 70 => 0, 43 => 2.0, 10 => 0.0, 20 => 0.0, 10 => 10.0, 20 => 0.0)).load();
    let sc = d.scene("Model");
    assert!((tri_area(&sc) - 20.0).abs() < 1e-3, "band area {}", tri_area(&sc));
    assert_eq!(total_len(&sc), 0.0);
    // classic POLYLINE with VERTEX / SEQEND
    let d = Doc::new()
        .add(format!(
            "{}{}{}{}  0\nSEQEND\n",
            ent!("POLYLINE"; 8 => "0", 66 => 1, 10 => 0.0, 20 => 0.0, 30 => 0.0, 70 => 0),
            ent!("VERTEX"; 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0),
            ent!("VERTEX"; 8 => "0", 10 => 30.0, 20 => 0.0, 30 => 0.0),
            ent!("VERTEX"; 8 => "0", 10 => 30.0, 20 => 40.0, 30 => 0.0)
        ))
        .load();
    assert!((total_len(&d.scene("Model")) - 70.0).abs() < 1e-4);
}

#[test]
fn dashed_lines_are_cut_into_dashes() {
    let d = Doc::new().add(ent!("LINE"; 8 => "0", 6 => "DASHED", 10 => 0.0, 20 => 0.0, 30 => 0.0, 11 => 7.5, 21 => 0.0, 31 => 0.0)).load();
    let sc = d.scene("Model");
    // pattern 0.5 dash / 0.25 gap on 7.5 units: 10 cycles, dashes of 0.5
    let n: usize = sc.batches.iter().map(|b| b.lines.len() / 2).sum();
    assert_eq!(n, 10);
    assert!((total_len(&sc) - 5.0).abs() < 1e-4);
    // $LTSCALE and the entity scale change the pattern
    let mut doc = Doc::new();
    doc.header.push_str(&t(&[(9, &"$LTSCALE"), (40, &2.0)]));
    doc = doc.add(ent!("LINE"; 8 => "0", 6 => "DASHED", 48 => 0.5, 10 => 0.0, 20 => 0.0, 30 => 0.0, 11 => 7.5, 21 => 0.0, 31 => 0.0));
    let sc = doc.load().scene("Model");
    assert!((total_len(&sc) - 5.0).abs() < 1e-4, "ltscale 2 * 0.5 = 1: same pattern, got {}", total_len(&sc));
}

#[test]
fn text_is_drawn_as_filled_glyphs_with_alignment() {
    let txt = |halign: i32, valign: i32| -> Scene2d {
        Doc::new()
            .add(ent!("TEXT"; 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0, 40 => 5.0, 1 => "Hello", 72 => halign, 11 => 100.0, 21 => 50.0, 31 => 0.0, 73 => valign))
            .load()
            .scene("Model")
    };
    let left = txt(0, 0);
    assert!(tri_area(&left) > 1.0);
    let b = left.bounds.unwrap();
    // LEFT / BASELINE: starts at the insert point, the baseline is y = 0, cap height 5
    assert!(b.0[0] >= -0.5 && b.0[0] < 1.0, "{:?}", b);
    assert!(b.0[1] > -2.0 && b.0[1] < 0.2, "{:?}", b);
    assert!((b.1[1] - 5.0).abs() < 0.5, "{:?}", b);
    // CENTER / MIDDLE-CENTER uses the second alignment point (100, 50)
    let c = txt(1, 2);
    let b = c.bounds.unwrap();
    assert!(((b.0[0] + b.1[0]) / 2.0 - 100.0).abs() < 1.5, "{:?}", b);
    assert!(((b.0[1] + b.1[1]) / 2.0 - 50.0).abs() < 2.0, "{:?}", b);
    // special characters
    let s = Doc::new().add(ent!("TEXT"; 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0, 40 => 5.0, 1 => "%%c10")).load();
    assert!(s.scene("Model").batches.iter().any(|b| !b.fills.is_empty()));
}

#[test]
fn czech_text_in_old_code_pages() {
    // $DWGCODEPAGE ANSI_1250: the bytes of "Čeština" are decoded as Windows-1250
    let mut head = Vec::new();
    head.extend_from_slice(b"  0\nSECTION\n  2\nHEADER\n  9\n$ACADVER\n  1\nAC1015\n  9\n$DWGCODEPAGE\n  3\nANSI_1250\n  0\nENDSEC\n");
    head.extend_from_slice(b"  0\nSECTION\n  2\nENTITIES\n  0\nTEXT\n  8\n0\n 10\n0.0\n 20\n0.0\n 30\n0.0\n 40\n3.0\n  1\n");
    head.push(0xC8); // C with caron
    head.extend_from_slice(b"e");
    head.push(0x9A); // s with caron
    head.extend_from_slice(b"tina\n  0\nENDSEC\n  0\nEOF\n");
    let d = load_bytes(&head, fonts()).unwrap();
    let e = &d.layout_entities("Model")[0];
    match &e.d {
        dxf::Data::Text(t) => assert_eq!(t.text, "Čeština"),
        other => panic!("{:?}", other),
    }
    // R2007 and newer are UTF-8, \U+ escapes of old files are decoded
    let d = Doc::new().add(ent!("TEXT"; 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0, 40 => 3.0, 1 => "P\\U+0159\\U+00EDklad ř")).load();
    match &d.layout_entities("Model")[0].d {
        dxf::Data::Text(t) => assert_eq!(t.text, "Příklad ř"),
        other => panic!("{:?}", other),
    }
}

#[test]
fn hatches_fill_with_holes_and_pattern_lines() {
    let poly = |pts: &[[f64; 2]]| -> String {
        let mut s = t(&[(92, &7), (72, &0), (73, &1), (93, &(pts.len() as i32))]);
        for p in pts {
            s.push_str(&t(&[(10, &p[0]), (20, &p[1])]));
        }
        s.push_str(&t(&[(97, &0)]));
        s
    };
    let hatch = |solid: i32, paths: String, n: i32, pattern: &str| -> String {
        format!(
            "{}{}{}{}",
            ent!("HATCH"; 8 => "0", 62 => 2, 10 => 0.0, 20 => 0.0, 30 => 0.0, 210 => 0.0, 220 => 0.0, 230 => 1.0, 2 => if solid == 1 { "SOLID" } else { "ANSI31" }, 70 => solid, 71 => 0, 91 => n),
            paths,
            t(&[(75, &0), (76, &1), (98, &0)]),
            pattern
        )
    };
    let outer = poly(&[[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]]);
    let hole = poly(&[[5.0, 5.0], [15.0, 5.0], [15.0, 15.0], [5.0, 15.0]]);
    let d = Doc::new().add(hatch(1, format!("{}{}", outer, hole), 2, "")).load();
    let sc = d.scene("Model");
    assert!((tri_area(&sc) - 300.0).abs() < 1e-3, "area {}", tri_area(&sc));
    // pattern lines: 45 degrees, spacing 5 along the normal
    let pat = t(&[(52, &0.0), (41, &1.0), (77, &0), (78, &1), (53, &45.0), (43, &0.0), (44, &0.0), (45, &-3.5355339059), (46, &3.5355339059), (79, &0)]);
    let d = Doc::new().add(hatch(0, outer.clone(), 1, &pat)).load();
    let sc = d.scene("Model");
    assert_eq!(tri_area(&sc), 0.0);
    // diagonal lines across a 20 x 20 square, 5 apart along the normal: the total length is area / spacing
    let l = total_len(&sc);
    assert!((l - 80.0).abs() < 2.0, "pattern length {}", l);
}

#[test]
fn layouts_viewports_and_default_layout() {
    // model space entity, a paper space layout with a viewport showing it
    let mut d = Doc::new();
    d.entities = format!(
        "{}{}",
        line("0", [0.0, 0.0], [100.0, 100.0], None),
        ent!("VIEWPORT"; 5 => "C0", 8 => "0", 67 => 1, 10 => 150.0, 20 => 150.0, 30 => 0.0, 40 => 100.0, 41 => 100.0, 68 => 2, 69 => 2, 12 => 50.0, 22 => 50.0, 16 => 0.0, 26 => 0.0, 36 => 1.0, 45 => 200.0)
    );
    d.objects = format!(
        "{}{}",
        ent!("LAYOUT"; 5 => "50", 100 => "AcDbLayout", 1 => "Model", 70 => 1, 71 => 0, 330 => "1F"),
        ent!("LAYOUT"; 5 => "51", 100 => "AcDbLayout", 1 => "Sheet A", 70 => 1, 71 => 1, 330 => "1E")
    );
    let drawing = d.load();
    let names: Vec<String> = drawing.layouts().iter().map(|l| l.name.clone()).collect();
    assert_eq!(names, vec!["Model", "Sheet A"]);
    assert_eq!(drawing.layouts()[0].drawable, 1);
    assert_eq!(drawing.layouts()[1].drawable, 0, "viewports do not count as drawable");
    let sc = drawing.scene("Sheet A");
    // scale 100 / 200 = 0.5: the model line (0,0)-(100,100) maps to (125,125)-(175,175) and is clipped by the 100 x 100 frame
    bounds_close(sc.bounds, [125.0, 125.0, 175.0, 175.0], 1e-6);
    assert_eq!(sc.layout, "Sheet A");
    // only paper space has content
    let mut p = Doc::new();
    p.entities = ent!("LINE"; 8 => "0", 67 => 1, 10 => 0.0, 20 => 0.0, 30 => 0.0, 11 => 5.0, 21 => 5.0, 31 => 0.0);
    let dd = p.load();
    assert_eq!(dd.default_layout(), "Layout1");
    assert_eq!(dd.scene("Layout1").batches.len(), 1);
    // the paper space is white: layer 0 colour 7 is black there
    assert_eq!(dd.scene("Layout1").batches[0].color, [0, 0, 0, 255]);
    assert_eq!(dd.background_color(), [255, 255, 255]);
}

#[test]
fn dimension_and_leader_are_drawn() {
    let dim_block = format!("{}{}", line("0", [0.0, -10.0], [100.0, -10.0], None), ent!("SOLID"; 8 => "0", 10 => 0.0, 20 => -10.0, 30 => 0.0, 11 => 2.0, 21 => -9.0, 31 => 0.0, 12 => 2.0, 22 => -11.0, 32 => 0.0, 13 => 2.0, 23 => -11.0, 33 => 0.0));
    let d = Doc::new()
        .block("*D1", [0.0, 0.0], &dim_block)
        .add(ent!("DIMENSION"; 8 => "0", 2 => "*D1", 100 => "AcDbDimension", 70 => 0, 10 => 0.0, 20 => -10.0, 30 => 0.0, 11 => 50.0, 21 => -10.0, 31 => 0.0, 13 => 0.0, 23 => 0.0, 33 => 0.0, 14 => 100.0, 24 => 0.0, 34 => 0.0, 42 => 100.0))
        .add(ent!("LEADER"; 8 => "0", 71 => 1, 72 => 0, 73 => 3, 76 => 2, 10 => 0.0, 20 => 50.0, 30 => 0.0, 10 => 20.0, 20 => 70.0, 30 => 0.0))
        .load();
    let sc = d.scene("Model");
    assert!(total_len(&sc) > 100.0);
    assert!(tri_area(&sc) > 0.0, "solid arrow of the dimension and of the leader");
    let rows = d.info_rows("Model", sc.bounds, None);
    assert!(rows.iter().any(|r| r.label == "Typy entit" && r.value.contains("DIMENSION 1") && r.value.contains("LEADER 1")));
    // measured value of the dimension is taken from the file
    let info = d.pick([50.0, -10.0], 1.0).expect("dimension line is picked");
    assert_eq!(info.dxftype, "DIMENSION");
    assert!(info.rows.iter().any(|(k, v)| k == "Naměřeno" && v == "100,000"), "{:?}", info.rows);
    assert!(info.block_rows.iter().any(|(k, _)| k == "Entita"), "the picked part is a block entity");
}

// ------------------------------------------------------------------------------------------------ picking and snapping
#[test]
fn picking_returns_the_topmost_entity_with_czech_rows() {
    let d = Doc::new()
        .layer("A", 1, 0)
        .add(ent!("LINE"; 5 => "10", 8 => "A", 10 => 0.0, 20 => 0.0, 30 => 0.0, 11 => 100.0, 21 => 0.0, 31 => 0.0))
        .add(ent!("LINE"; 5 => "11", 8 => "0", 10 => 50.0, 20 => -50.0, 30 => 0.0, 11 => 50.0, 21 => 50.0, 31 => 0.0))
        .add(ent!("CIRCLE"; 5 => "12", 8 => "0", 10 => 200.0, 20 => 0.0, 30 => 0.0, 40 => 10.0))
        .load();
    let _ = d.scene("Model");
    // the crossing point: the later drawn vertical line wins
    let p = d.pick([50.0, 0.0], 2.0).unwrap();
    assert_eq!(p.handle, "11");
    assert_eq!(p.dxftype, "LINE");
    assert_eq!(p.rows[0], ("Entita".to_string(), "LINE".to_string()));
    assert!(p.rows.contains(&("Délka".to_string(), "100,00".to_string())));
    assert!(p.rows.contains(&("Handle".to_string(), "11".to_string())));
    assert!(p.block_rows.is_empty());
    // off the line
    assert!(d.pick([30.0, 20.0], 2.0).is_none());
    // the circle outline
    let c = d.pick([210.0, 0.0], 1.0).unwrap();
    assert_eq!(c.dxftype, "CIRCLE");
    assert!(c.rows.contains(&("Poloměr".to_string(), "10,00".to_string())));
    assert!(c.rows.contains(&("Obvod".to_string(), "62,83".to_string())));
    // hidden layers cannot be picked
    let a = d.scene("Model").layers.iter().find(|l| l.name == "A").unwrap().id;
    d.set_layer_visible(a, false);
    assert_eq!(d.pick([10.0, 0.0], 1.0).map(|p| p.handle), None);
    d.set_layer_visible(a, true);
    assert_eq!(d.pick([10.0, 0.0], 1.0).map(|p| p.handle), Some("10".to_string()));
    // the highlight is orange and at least 2 px wide
    let hl = d.highlight(p.id);
    assert_eq!(hl.len(), 1);
    assert_eq!(hl[0].color, [255, 140, 0, 255]);
    assert!(hl[0].width >= 2.0);
    let bb = d.entity_bounds(p.id).unwrap();
    assert!((bb.0[0] - 50.0).abs() < 1e-9 && (bb.1[1] - 50.0).abs() < 1e-9);
}

#[test]
fn snapping_prefers_path_vertices_over_line_ends() {
    let d = Doc::new()
        .add(line("0", [0.0, 0.0], [100.0, 0.0], None))
        .add(ent!("LWPOLYLINE"; 8 => "0", 90 => 3, 70 => 0, 10 => 100.0, 20 => 3.0, 10 => 120.0, 20 => 30.0, 10 => 140.0, 20 => 3.0))
        .load();
    let _ = d.scene("Model");
    let pts = d.snap_points([90.0, -5.0], [110.0, 10.0]);
    assert_eq!(pts.len(), 2, "{:?}", pts);
    assert!(pts.iter().any(|p| p.line_end && p.pos == [100.0, 0.0]));
    assert!(pts.iter().any(|p| !p.line_end && p.pos == [100.0, 3.0]));
    // 1 unit = 1 pixel: the cursor at (100, 1) is 1 px from the line end and 2 px from the polyline vertex
    let to_px = |p: [f64; 2]| p;
    let hit = d.snap(([90.0, -9.0], [110.0, 11.0]), &to_px, [100.0, 1.0]).unwrap();
    assert_eq!(hit, [100.0, 3.0], "path vertices come first");
    assert!(d.snap(([-10.0, 40.0], [10.0, 60.0]), &to_px, [0.0, 50.0]).is_none());
    // only the line end within reach
    let hit = d.snap(([-8.0, -8.0], [8.0, 8.0]), &to_px, [1.0, 1.0]).unwrap();
    assert_eq!(hit, [0.0, 0.0]);
}

// ------------------------------------------------------------------------------------------------ limits, warnings, info
#[test]
fn cyclic_and_exploding_blocks_are_refused() {
    let cyc = Doc::new()
        .block("A", [0.0, 0.0], &ent!("INSERT"; 2 => "B", 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0))
        .block("B", [0.0, 0.0], &ent!("INSERT"; 2 => "A", 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0))
        .add(ent!("INSERT"; 2 => "A", 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0))
        .load();
    let e = cyc.check_budget("Model").unwrap_err();
    assert_eq!(e.0, "Cyklický blok: „A“ se přímo nebo nepřímo vkládá sám do sebe – výkres nelze vykreslit.");
    assert!(cyc.scene("Model").batches.is_empty());
    assert!(cyc.scene_checked("Model").is_err());
    // 10 inserts per level, 6 levels: 1 000 000 entities from a few records
    let mut d = Doc::new().block("L0", [0.0, 0.0], &line("0", [0.0, 0.0], [1.0, 0.0], None));
    for level in 1..=6 {
        let mut body = String::new();
        for _ in 0..10 {
            body.push_str(&ent!("INSERT"; 2 => format!("L{}", level - 1), 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0));
        }
        d = d.block(&format!("L{}", level), [0.0, 0.0], &body);
    }
    d = d.add(ent!("INSERT"; 2 => "L6", 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0));
    let d = d.load();
    let e = d.check_budget("Model").unwrap_err();
    assert!(e.0.starts_with("Výkres se rozvinutím bloků rozpadne na více než 300 000 entit"), "{}", e.0);
    // moderately nested drawings pass
    let ok = Doc::new()
        .block("A", [0.0, 0.0], &line("0", [0.0, 0.0], [1.0, 0.0], None))
        .add(ent!("INSERT"; 2 => "A", 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0, 70 => 10, 71 => 10, 44 => 2.0, 45 => 2.0))
        .load();
    assert_eq!(ok.check_budget("Model").unwrap(), 101);
}

#[test]
fn amplification_message_for_bombs_below_the_absolute_limit() {
    // 200 000 drawn entities from 3 records: above 50 000 and more than 100 times the stored entities
    let d = Doc::new()
        .block("A", [0.0, 0.0], &line("0", [0.0, 0.0], [1.0, 0.0], None))
        .add(ent!("INSERT"; 2 => "A", 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0, 70 => 400, 71 => 500, 44 => 2.0, 45 => 2.0))
        .load();
    let e = d.check_budget("Model").unwrap_err();
    assert!(e.0.starts_with("Výkres má v souboru jen 2 entit, ale rozvinutím vnořených bloků by jich vykreslil 200001 (100000× víc)"), "{}", e.0);
    assert!(e.0.ends_with("Výkres se nevykreslí."));
}

#[test]
fn info_rows_and_warnings() {
    let mut d = Doc::new();
    d.objects = ent!("IMAGEDEF"; 5 => "90", 1 => "\\\\server\\share\\plan.png") + &ent!("IMAGEDEF"; 5 => "91", 1 => "C:\\plans\\a.png");
    d.entities = format!(
        "{}{}{}",
        line("0", [0.0, 0.0], [10.0, 5.0], None),
        ent!("3DSOLID"; 8 => "0"),
        ent!("3DSOLID"; 8 => "0")
    );
    let drawing = d.load();
    let sc = drawing.scene("Model");
    let rows = drawing.info_rows("Model", sc.bounds, Some(1.25));
    let get = |l: &str| rows.iter().find(|r| r.label == l).cloned();
    assert_eq!(get("Verze").unwrap().value, "R2013 (AC1027)");
    assert_eq!(get("Jednotky").unwrap().value, "mm");
    assert_eq!(get("Layout").unwrap().value, "Model");
    assert_eq!(get("Entit").unwrap().value, "3");
    assert_eq!(get("Hladin / bloků").unwrap().value, "1 / 0");
    assert_eq!(get("Rozměr výkresu").unwrap().value, "10,00 × 5,00 mm");
    assert_eq!(get("Typy entit").unwrap().value, "3DSOLID 2, LINE 1");
    let acis = get("Upozornění").unwrap();
    assert_eq!(acis.value, "2× 3D těleso (ACIS) – v 2D náhledu se nezobrazí");
    assert!(acis.warning);
    let img = get("Obrázky").unwrap();
    assert_eq!(img.value, "1× obrázek ze síťové cesty nenačten (bezpečnost) – zobrazen jen rámeček");
    assert!(img.warning);
    assert_eq!(get("Načteno za").unwrap().value, "1.2 s".replace("1.2", "1.2"));
    assert!(get("Převod DWG").is_none() && get("Opraveno chyb").is_none());
    assert_eq!(drawing.layouts()[0].drawable, 1, "3DSOLID is not drawable");
    assert_eq!(drawing.info().remote_images, vec!["\\\\server\\share\\plan.png".to_string()]);
}

#[test]
fn remote_image_references_stay_blocked() {
    for p in ["\\\\evil\\share\\a.png", "//evil/share/a.png", "http://evil/a.png", "file:\\\\evil\\a.png", "\\\\?\\UNC\\evil\\a.png", "  \\\\evil\\a.png"] {
        assert!(info::is_remote_path(p), "{p}");
    }
    for p in ["C:\\plans\\a.png", "plans/a.png", "a.png", "D:/x/y.jpg"] {
        assert!(!info::is_remote_path(p), "{p}");
    }
    // a drawing with a remote image: only the frame, no file name text and no lookup of the file
    let mut d = Doc::new();
    d.objects = ent!("IMAGEDEF"; 5 => "90", 1 => "\\\\evil\\share\\plan.png");
    d.entities = ent!("IMAGE"; 8 => "0", 10 => 0.0, 20 => 0.0, 30 => 0.0, 11 => 1.0, 21 => 0.0, 31 => 0.0, 12 => 0.0, 22 => 1.0, 32 => 0.0, 13 => 100.0, 23 => 50.0, 340 => "90", 70 => 3, 280 => 0, 71 => 1);
    let sc = d.load().scene("Model");
    assert_eq!(tri_area(&sc), 0.0);
    assert_eq!(sc.batches.iter().map(|b| b.lines.len() / 2).sum::<usize>(), 4);
    bounds_close(sc.bounds, [0.0, 0.0, 100.0, 50.0], 1e-6);
}

#[test]
fn damaged_files_are_read_as_far_as_possible() {
    // garbage lines, a missing EOF and a truncated entity
    let mut s = Doc::new().add(line("0", [0.0, 0.0], [10.0, 0.0], None)).text();
    s = s.replace("  0\nEOF\n", "");
    s.push_str("garbage line\n  0\nLINE\n  8\n0\n 10\n5\n");
    let d = load_bytes(s.as_bytes(), fonts()).unwrap();
    assert!(d.info().errors >= 2, "errors counted: {}", d.info().errors);
    assert_eq!(d.layout_entities("Model").len(), 1, "the entity after ENDSEC is not part of a section");
    assert!(d.scene("Model").bounds.is_some());
    // not a DXF at all
    assert!(matches!(load_bytes(b"hello world, this is no drawing", fonts()), Err(LoadError::Invalid)));
    assert!(matches!(load_bytes(b"", fonts()), Err(LoadError::Invalid)));
}

#[test]
fn inventor_block_references_become_plain_inserts() {
    let d = Doc::new()
        .block("VIEW1", [0.0, 0.0], &line("0", [0.0, 0.0], [10.0, 0.0], None))
        .add(ent!("ACIDBLOCKREFERENCE"; 5 => "30", 8 => "0", 2 => "VIEW1", 10 => 50.0, 20 => 60.0, 30 => 0.0, 41 => 1.0, 42 => 1.0, 43 => 1.0, 50 => 0.0))
        .load();
    assert_eq!(d.info().inventor, 1);
    let sc = d.scene("Model");
    bounds_close(sc.bounds, [50.0, 60.0, 60.0, 60.0], 1e-6);
    let rows = d.info_rows("Model", sc.bounds, None);
    assert_eq!(rows.iter().find(|r| r.label == "Inventor").unwrap().value, "1 pohledů převedeno");
    assert!(rows.iter().any(|r| r.label == "Typy entit" && r.value.contains("ACIDBLOCKREFERENCE 1") && r.value.contains("INSERT 1")));
}

#[test]
fn scenes_can_be_relative_to_the_drawing_centre() {
    // coordinates like S-JTSK: f32 batches would lose centimetres
    let d = Doc::new().add(line("0", [-742_000.0, -1_043_000.0], [-741_000.0, -1_042_000.0], None)).load();
    let world = d.scene_with("Model", &SceneOptions::default()).unwrap();
    bounds_close(world.bounds, [-742_000.0, -1_043_000.0, -741_000.0, -1_042_000.0], 1e-6);
    let rel = d.scene_with("Model", &SceneOptions { origin: OriginMode::Center, ..Default::default() }).unwrap();
    let o = d.scene_origin();
    assert_eq!(o, [-741_500.0, -1_042_500.0]);
    assert_eq!(rel.batches[0].lines, vec![[-500.0, -500.0], [500.0, 500.0]]);
    bounds_close(rel.bounds, [-742_000.0, -1_043_000.0, -741_000.0, -1_042_000.0], 1e-6);
}

#[test]
fn points_follow_pdmode_and_defpoints_are_hidden() {
    let mut d = Doc::new().layer("Defpoints", 7, 0);
    d.header.push_str(&t(&[(9, &"$PDMODE"), (70, &3), (9, &"$PDSIZE"), (40, &2.0)]));
    let sc = d.add(ent!("POINT"; 8 => "0", 10 => 5.0, 20 => 5.0, 30 => 0.0)).add(ent!("POINT"; 8 => "Defpoints", 10 => 50.0, 20 => 50.0, 30 => 0.0)).load().scene("Model");
    // PDMODE 3: an X of size 2 around the point; the point on layer Defpoints is not shown
    bounds_close(sc.bounds, [3.0, 3.0, 7.0, 7.0], 1e-9);
    assert_eq!(sc.batches.iter().map(|b| b.lines.len() / 2).sum::<usize>(), 2);
    // PDMODE 0: a dimensionless point is a zero length segment
    let sc = Doc::new().add(ent!("POINT"; 8 => "0", 10 => 5.0, 20 => 6.0, 30 => 0.0)).load().scene("Model");
    assert_eq!(sc.batches[0].lines, vec![[5.0, 6.0], [5.0, 6.0]]);
}

#[test]
fn extrusion_vectors_mirror_entities() {
    // a circle with extrusion (0,0,-1) is mirrored in x: centre (10, 5) becomes (-10, 5)
    let d = Doc::new().add(ent!("CIRCLE"; 8 => "0", 10 => 10.0, 20 => 5.0, 30 => 0.0, 40 => 2.0, 210 => 0.0, 220 => 0.0, 230 => -1.0)).load();
    bounds_close(d.scene("Model").bounds, [-12.0, 3.0, -8.0, 7.0], 1e-9);
    let o = geom::Ocs::new([0.0, 0.0, -1.0]);
    let p: V3 = o.to_wcs([10.0, 5.0, 0.0]);
    assert!((p[0] + 10.0).abs() < 1e-12 && Z_AXIS[2] == 1.0);
}

#[test]
fn binary_dxf_is_read() {
    // header with the version, one LINE in the ENTITIES section
    let mut b = dxf::tags::BINARY_SENTINEL.to_vec();
    let s = |b: &mut Vec<u8>, c: i16, v: &str| {
        b.extend_from_slice(&c.to_le_bytes());
        b.extend_from_slice(v.as_bytes());
        b.push(0);
    };
    let f = |b: &mut Vec<u8>, c: i16, v: f64| {
        b.extend_from_slice(&c.to_le_bytes());
        b.extend_from_slice(&v.to_le_bytes());
    };
    s(&mut b, 0, "SECTION");
    s(&mut b, 2, "ENTITIES");
    s(&mut b, 0, "LINE");
    s(&mut b, 8, "0");
    f(&mut b, 10, 1.0);
    f(&mut b, 20, 2.0);
    f(&mut b, 30, 0.0);
    f(&mut b, 11, 11.0);
    f(&mut b, 21, 2.0);
    f(&mut b, 31, 0.0);
    s(&mut b, 0, "ENDSEC");
    s(&mut b, 0, "EOF");
    let d = load_bytes(&b, fonts()).unwrap();
    bounds_close(d.scene("Model").bounds, [1.0, 2.0, 11.0, 2.0], 1e-12);
}

#[test]
fn dxf_and_dwg_files_load_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.dxf");
    std::fs::write(&p, Doc::new().add(line("0", [0.0, 0.0], [3.0, 4.0], None)).text()).unwrap();
    let mut msgs: Vec<String> = Vec::new();
    let d = load(&p, &LoadOptions { fonts: fonts(), ..Default::default() }, &mut |m| msgs.push(m.to_string())).unwrap();
    assert_eq!(msgs, vec!["Čtu DXF…".to_string()]);
    assert!((total_len(&d.scene("Model")) - 5.0).abs() < 1e-5);
    assert_eq!(d.info().path, p);
    assert!(d.info().converter.is_none());
    // a cached conversion of a DWG is used without running a converter
    let dwg = dir.path().join("b.dwg");
    std::fs::write(&dwg, b"AC1027 not really a drawing").unwrap();
    let cache = dir.path().join("b.dxf");
    std::fs::copy(&p, &cache).unwrap();
    let mut msgs: Vec<String> = Vec::new();
    let d = load(&dwg, &LoadOptions { dxf_cache: Some(cache), fonts: fonts(), ..Default::default() }, &mut |m| msgs.push(m.to_string())).unwrap();
    assert_eq!(msgs, vec!["Čtu DXF…".to_string()]);
    assert_eq!(d.info().converter.as_deref(), Some("mezipaměť"));
    let rows = d.info_rows("Model", None, None);
    assert_eq!(rows.iter().find(|r| r.label == "Převod DWG").unwrap().value, "mezipaměť");
    // missing file
    assert!(matches!(load(&dir.path().join("none.dxf"), &LoadOptions::default(), &mut |_| {}), Err(LoadError::Io(_))));
}

#[test]
fn bundled_converter_reads_a_dwg_when_available() {
    // runs only when third_party/libredwg/dwg2dxf.exe is installed; the DWG here is not valid, so the converter fails
    // with the Czech message of the viewer
    let tp = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../third_party");
    if dwg::find_dwg2dxf(Some(&tp)).is_none() || dwg::find_oda().is_some() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.dwg");
    std::fs::write(&bad, b"this is not a dwg file").unwrap();
    let e = match load(&bad, &LoadOptions { third_party_dir: Some(tp), ..Default::default() }, &mut |_| {}) {
        Err(e) => e,
        Ok(_) => panic!("an invalid DWG must not load"),
    };
    assert!(e.to_string().starts_with("Převod DWG selhal"), "{}", e);
    // no temporary conversion folders are left behind
    assert!(std::fs::read_dir(dir.path()).unwrap().flatten().all(|f| !f.file_name().to_string_lossy().starts_with("dwg_")));
}

#[test]
fn batches_depend_on_the_layers_of_inserts_and_composite_entities() {
    // block content on layer 0 follows the layer of the INSERT, content on another layer keeps it
    let body = format!("{}{}", line("0", [0.0, 0.0], [10.0, 0.0], None), line("OWN", [0.0, 5.0], [10.0, 5.0], Some(1)));
    // the dimension is on DIMS, its block content on layer PARTS: a part is shown only when both layers are on
    let dim_block = line("PARTS", [0.0, 50.0], [10.0, 50.0], Some(2));
    let d = Doc::new()
        .layer("L1", 3, 0)
        .layer("OWN", 1, 0)
        .layer("DIMS", 4, 0)
        .layer("PARTS", 2, 0)
        .block("B", [0.0, 0.0], &body)
        .block("*D1", [0.0, 0.0], &dim_block)
        .add(ent!("INSERT"; 2 => "B", 8 => "L1", 10 => 0.0, 20 => 100.0, 30 => 0.0))
        .add(ent!("DIMENSION"; 8 => "DIMS", 2 => "*D1", 100 => "AcDbDimension", 70 => 0, 10 => 0.0, 20 => 0.0, 30 => 0.0, 11 => 5.0, 21 => 50.0, 31 => 0.0))
        .load();
    let sc = d.scene("Model");
    let id = |n: &str| sc.layers.iter().find(|l| l.name == n).unwrap().id;
    let deps = d.batch_layers();
    assert_eq!(deps.len(), sc.batches.len());
    assert!(deps.contains(&vec![id("L1")]), "layer 0 content of the INSERT: {:?}", deps);
    assert!(deps.contains(&vec![id("OWN")]), "{:?}", deps);
    assert!(deps.contains(&vec![id("DIMS"), id("PARTS")]), "dimension and its part: {:?}", deps);
    assert!(d.visible_batches().iter().all(|&v| v));
    d.set_layer_visible(id("L1"), false);
    let shown = d.visible_batches();
    for (i, dep) in deps.iter().enumerate() {
        assert_eq!(shown[i], !dep.contains(&id("L1")), "{:?}", dep);
    }
    d.set_layer_visible(id("L1"), true);
    d.set_layer_visible(id("DIMS"), false);
    let shown = d.visible_batches();
    assert_eq!(shown.iter().filter(|&&v| !v).count(), 1, "only the dimension part disappears");
}

#[test]
fn drawing_is_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Drawing>();
    check::<Scene2d>();
}

mod fix2b;
mod fixlod;
mod fixmix;
mod fixn;
mod review;
